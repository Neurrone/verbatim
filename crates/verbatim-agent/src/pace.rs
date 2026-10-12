//! The pacing of [`Request::SendKeysPaced`](crate::protocol::Request::SendKeysPaced)
//! and [`Request::TypeTextPaced`](crate::protocol::Request::TypeTextPaced),
//! which inject one stroke every interval, as a typist or keyboard
//! auto-repeat would, for the end-to-end harness's measurement workloads.
//!
//! The pace is the workload being measured, a rate of key presses, not a
//! wait for evidence: nothing here waits for Verbatim or an application.
//! Each stroke is due a whole number of intervals after the first, so a
//! late stroke does not delay the ones after it. The wait uses a
//! high-resolution waitable timer, whose resolution is well under a
//! millisecond, rather than the system timer's 15.6 ms tick.

use std::io;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::PCWSTR;

/// Paces strokes `interval` apart from the moment it was made.
pub struct Pacer {
    timer: HANDLE,
    started: Instant,
    interval: Duration,
}

impl Pacer {
    /// A pacer with strokes `interval_ms` apart, starting now.
    ///
    /// # Errors
    ///
    /// Returns an error when the timer cannot be made.
    pub fn new(interval_ms: u32) -> io::Result<Self> {
        // SAFETY: no attributes and no name; the flags and access rights are
        // documented values. The handle is closed when the pacer drops.
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .map_err(io::Error::other)?;
        Ok(Self {
            timer,
            started: Instant::now(),
            interval: Duration::from_millis(u64::from(interval_ms)),
        })
    }

    /// Returns once stroke `index` (from 0) is due: at once when it is
    /// already due.
    ///
    /// # Errors
    ///
    /// Returns an error when the timer cannot be set or waited on.
    pub fn wait_for(&mut self, index: usize) -> io::Result<()> {
        let due = self.started + self.interval * u32::try_from(index).unwrap_or(u32::MAX);
        let now = Instant::now();
        if due <= now {
            return Ok(());
        }
        // Relative due times are negative, in 100-nanosecond units.
        let relative = -i64::try_from((due - now).as_nanos() / 100).unwrap_or(i64::MAX);
        // SAFETY: `self.timer` is the live timer this pacer made; the due
        // time is read during the call only; no completion routine.
        unsafe { SetWaitableTimer(self.timer, &raw const relative, 0, None, None, false) }
            .map_err(io::Error::other)?;
        // SAFETY: waits on the live timer just set, which signals at the
        // due time.
        let waited = unsafe { WaitForSingleObject(self.timer, INFINITE) };
        if waited == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        // SAFETY: closes the handle this pacer owns, once.
        let _ = unsafe { CloseHandle(self.timer) };
    }
}
