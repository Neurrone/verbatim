//! An alarm: a high-resolution waitable timer and an event, waited on
//! together by one thread. Setting and cancelling the timer are system
//! calls that wake no thread.
//!
//! The watchdog's alarm is a timer the worker sets when it starts an entry
//! and cancels when it publishes, and an event for the times the watchdog
//! must look now (a foreground fact, the outpost's shutdown), so the
//! watchdog wakes only when a deadline or a grace passes, or the event is
//! set, not for every entry the worker takes (`docs/design/wake-ups.md`,
//! item 3). The intake's alarm is the worker's wait for work: the event is
//! set when something is pushed while the worker waits, and the timer is
//! set to the wake the worker asked for, at a resolution finer than the
//! system's 15.6 ms timer ([`super::intake`]).

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::time::Instant;

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateEventW,
    CreateWaitableTimerExW, INFINITE, SetEvent, SetWaitableTimer, TIMER_ALL_ACCESS,
    WaitForMultipleObjects,
};

/// A waitable timer and an event, waited on together.
pub(crate) struct Alarm {
    timer: OwnedHandle,
    event: OwnedHandle,
}

impl Default for Alarm {
    /// A timer that is not set and an event that is not set.
    ///
    /// # Panics
    ///
    /// Panics if Windows cannot make either, which means the process is out
    /// of kernel resources, as a thread that cannot be spawned does.
    fn default() -> Self {
        // SAFETY: an unnamed, unsecured high-resolution synchronization
        // timer; the handle returned is owned below.
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .expect("a waitable timer for an alarm");
        // SAFETY: an unnamed auto-reset event, not set; owned below.
        let event =
            unsafe { CreateEventW(None, false, false, None) }.expect("an event for an alarm");
        Self {
            // SAFETY: valid handles just returned, owned by nothing else.
            timer: unsafe { OwnedHandle::from_raw_handle(timer.0 as RawHandle) },
            // SAFETY: as above.
            event: unsafe { OwnedHandle::from_raw_handle(event.0 as RawHandle) },
        }
    }
}

impl Alarm {
    /// Sets the timer to go off at `at`, replacing any time set before, and
    /// takes back a time that went off and was not waited for; a time
    /// already past makes it go off at once.
    pub(crate) fn set(&self, at: Instant) {
        let left = at.saturating_duration_since(Instant::now());
        // Negative: relative to now, in 100-nanosecond units; at least one,
        // since zero would mean an absolute time.
        let due = -i64::try_from(left.as_nanos() / 100)
            .unwrap_or(i64::MAX)
            .max(1);
        // SAFETY: the timer is open; `due` lives across the call; no period
        // and no completion routine.
        if let Err(error) = unsafe {
            SetWaitableTimer(
                self.handle(&self.timer),
                &raw const due,
                0,
                None,
                None,
                false,
            )
        } {
            tracing::warn!(%error, "an alarm's timer could not be set");
        }
    }

    /// Cancels the timer, if it is set. A time that has already gone off
    /// stays signalled until a wait takes it, and the waiter finds nothing
    /// behind it.
    pub(crate) fn cancel(&self) {
        // SAFETY: the timer is open.
        let _ = unsafe { CancelWaitableTimer(self.handle(&self.timer)) };
    }

    /// Ends the wait now, or the next one if no thread waits.
    pub(crate) fn wake(&self) {
        // SAFETY: the event is open.
        let _ = unsafe { SetEvent(self.handle(&self.event)) };
    }

    /// Waits until the timer goes off or the event is set, and answers
    /// whether the timer ended the wait.
    pub(crate) fn wait(&self) -> bool {
        let handles = [self.handle(&self.timer), self.handle(&self.event)];
        // SAFETY: both handles are open for the wait's duration, owned by
        // this alarm, which the caller borrows.
        let ended = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        ended == WAIT_OBJECT_0
    }

    /// Waits until the event is set, whatever the timer does.
    pub(crate) fn wait_for_event(&self) {
        let handles = [self.handle(&self.event)];
        // SAFETY: as for `wait`.
        let _ = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
    }

    #[expect(clippy::unused_self, reason = "kept beside the handles it reads")]
    fn handle(&self, handle: &OwnedHandle) -> HANDLE {
        HANDLE(handle.as_raw_handle())
    }
}
