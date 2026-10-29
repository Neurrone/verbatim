//! The gap record's bound ([`super::gap_record`]): one one-shot threadpool
//! timer per outpost, set when the record opens and cancelled when it is
//! answered (`docs/design/focus-pipeline-rework.md`, "Who owns each
//! duplicated fact", the gap's evidence). No thread of the outpost's waits
//! for it, so a record answered first costs two local calls and no
//! wake-up. When it fires, its callback pushes the bound's entry into the
//! intake ([`super::intake::Item::GapBound`]), which waits behind every
//! marker queued before it, so a UIA focus already being read decides
//! first.

use std::io;
use std::sync::{Arc, Weak};

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{
    CloseThreadpoolTimer, CreateThreadpoolTimer, PTP_CALLBACK_INSTANCE, PTP_TIMER,
    SetThreadpoolTimer, WaitForThreadpoolTimerCallbacks,
};

use super::intake::Intake;

/// What times the gap record's bound: the outpost's threadpool timer
/// ([`ThreadpoolGapTimer`]), unless a test sets its own
/// (`Outpost::set_gap_timer`), which says itself when the bound is reached
/// (`Outpost::gap_bound_reacher`).
pub trait GapTimer: Send + Sync {
    /// Has the bound reached at `due_us`, on the outpost's observation
    /// clock (`Outpost::set_observation_clock`), in place of any time set
    /// before; at once for a time already past.
    fn set(&self, due_us: u64);
    /// Has the bound not reached.
    fn cancel(&self);
}

/// A [`GapTimer`] that never reaches the bound, for an outpost whose
/// threadpool timer could not be made, which is reported as an error.
pub(crate) struct NoGapTimer;

impl GapTimer for NoGapTimer {
    fn set(&self, _: u64) {}
    fn cancel(&self) {}
}

/// The product's timer for `intake`, its due times on the run's shared
/// clock; one that never reaches the bound, with an error logged, if the
/// threadpool timer cannot be made.
pub(crate) fn for_intake(intake: &Arc<Intake>) -> Arc<dyn GapTimer> {
    match ThreadpoolGapTimer::new(intake, crate::protocol::now_us) {
        Ok(timer) => Arc::new(timer),
        Err(error) => {
            tracing::error!(%error, "the gap record's timer could not be made; no gap is read at its bound");
            Arc::new(NoGapTimer)
        }
    }
}

/// What the timer's callback is given: the intake it pushes the bound's
/// entry into.
struct Bell {
    intake: Weak<Intake>,
}

/// The product's [`GapTimer`]: a one-shot threadpool timer.
pub(crate) struct ThreadpoolGapTimer {
    timer: PTP_TIMER,
    /// The callback's target, kept alive until the timer is closed.
    bell: *const Bell,
    /// Now on the run's shared clock, which due times are on.
    now_us: fn() -> u64,
}

// SAFETY: a threadpool timer may be set, cancelled and closed from any
// thread, and its target is shared, `Sync` data kept alive by the timer.
unsafe impl Send for ThreadpoolGapTimer {}
// SAFETY: as above; `set` and `cancel` only set the timer, which the
// threadpool allows from any thread at any time.
unsafe impl Sync for ThreadpoolGapTimer {}

impl ThreadpoolGapTimer {
    /// A timer that pushes the bound's entry into `intake`, its due times
    /// read against `now_us`.
    ///
    /// # Errors
    ///
    /// The error of creating the threadpool timer.
    pub(crate) fn new(intake: &Arc<Intake>, now_us: fn() -> u64) -> io::Result<Self> {
        let bell = Arc::into_raw(Arc::new(Bell {
            intake: Arc::downgrade(intake),
        }));
        // SAFETY: the callback is given `bell`, which stays alive until the
        // timer is closed in `drop`, after its callbacks have finished.
        let timer = unsafe {
            CreateThreadpoolTimer(Some(bound_reached), Some(bell.cast_mut().cast()), None)
        };
        match timer {
            Ok(timer) => Ok(Self {
                timer,
                bell,
                now_us,
            }),
            Err(error) => {
                // SAFETY: from `Arc::into_raw` above, never given out.
                drop(unsafe { Arc::from_raw(bell) });
                Err(io::Error::other(error))
            }
        }
    }
}

impl GapTimer for ThreadpoolGapTimer {
    fn set(&self, due_us: u64) {
        let after_us = due_us.saturating_sub((self.now_us)());
        // Relative, in 100-nanosecond units.
        let due = -i64::try_from(after_us.saturating_mul(10)).unwrap_or(i64::MAX);
        let due = due.cast_unsigned();
        let due = FILETIME {
            dwLowDateTime: (due & 0xFFFF_FFFF) as u32,
            dwHighDateTime: (due >> 32) as u32,
        };
        // SAFETY: the timer is open until `drop`; the due time is read
        // during the call.
        unsafe { SetThreadpoolTimer(self.timer, Some(&raw const due), 0, Some(0)) };
    }

    fn cancel(&self) {
        // SAFETY: the timer is open until `drop`.
        unsafe { SetThreadpoolTimer(self.timer, None, 0, None) };
    }
}

impl Drop for ThreadpoolGapTimer {
    fn drop(&mut self) {
        // The timer is stopped, its callbacks waited for or cancelled, and
        // only then closed and its target released. The timer is the
        // context's, not the intake's, so no callback holds the last
        // reference to what owns it.
        // SAFETY: the timer is open.
        unsafe { SetThreadpoolTimer(self.timer, None, 0, None) };
        // SAFETY: as above.
        unsafe { WaitForThreadpoolTimerCallbacks(self.timer, true) };
        // SAFETY: as above; no callback runs or will run.
        unsafe { CloseThreadpoolTimer(self.timer) };
        // SAFETY: from `Arc::into_raw` in `new`, released once, here, with
        // no callback left to use it.
        drop(unsafe { Arc::from_raw(self.bell) });
    }
}

/// The timer's callback, on a threadpool thread: the bound's entry is
/// pushed into the intake, if the outpost is still there.
unsafe extern "system" fn bound_reached(
    _instance: PTP_CALLBACK_INSTANCE,
    bell: *mut core::ffi::c_void,
    _timer: PTP_TIMER,
) {
    // SAFETY: the timer was created with its `Bell`, which outlives every
    // callback (`ThreadpoolGapTimer::drop`).
    let bell = unsafe { &*bell.cast_const().cast::<Bell>() };
    if let Some(intake) = bell.intake.upgrade() {
        intake.push_gap_bound();
    }
}
