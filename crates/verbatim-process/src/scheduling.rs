//! How Verbatim's processes and its latency-critical threads ask to be
//! scheduled, under whatever power plan the machine has (Dickson's decision
//! of 2026-10-08, `docs/roadmap.md`; `docs/design/communication.md`, section 6.1).
//!
//! - The threads that only move messages as an event crosses processes run
//!   at a raised priority ([`prioritize_current_thread`]), so a thread made
//!   ready by a pipe pre-empts the normal-priority threads of whatever
//!   application is busy rather than waiting out their quantum. They do
//!   microseconds of work per message and block between messages. Each
//!   caller names its level in one constant of its own. Measured on
//!   2026-10-13 (`docs/design/communication.md`, section 11, step 3), this
//!   took verbatim.exe's own stage's maximum from about 4 ms to under
//!   half a millisecond, with no measurable change in processor time or
//!   wake-ups; raising the threads that compute did not pay, and is not
//!   done.
//! - Power throttling is left to Windows: opting out keeps a process on
//!   fast cores at full clocks, which costs energy for every unit of work,
//!   and the same measurement found no latency gain from it.
//!
//! Timer resolution is not raised: nothing on the relay's path waits on a
//! timer.

use std::io;

use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY, THREAD_PRIORITY_ABOVE_NORMAL,
    THREAD_PRIORITY_HIGHEST, THREAD_PRIORITY_NORMAL, THREAD_PRIORITY_TIME_CRITICAL,
};

/// A thread's priority within its process's normal priority class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadPriority {
    /// Base priority 8, as every thread starts.
    Normal,
    /// Base priority 9.
    AboveNormal,
    /// Base priority 10: above the foreground application's boosted
    /// threads.
    Highest,
    /// Base priority 15, the highest outside the real-time class.
    TimeCritical,
}

impl ThreadPriority {
    fn value(self) -> THREAD_PRIORITY {
        match self {
            ThreadPriority::Normal => THREAD_PRIORITY_NORMAL,
            ThreadPriority::AboveNormal => THREAD_PRIORITY_ABOVE_NORMAL,
            ThreadPriority::Highest => THREAD_PRIORITY_HIGHEST,
            ThreadPriority::TimeCritical => THREAD_PRIORITY_TIME_CRITICAL,
        }
    }
}

/// Runs the calling thread at `priority`, for a thread an event waits on
/// as it crosses processes. Dynamic priority boosts stay on.
///
/// # Errors
///
/// Returns the error if Windows refuses the request.
pub fn prioritize_current_thread(priority: ThreadPriority) -> io::Result<()> {
    // SAFETY: returns a pseudo-handle and has no preconditions.
    let thread = unsafe { GetCurrentThread() };
    // SAFETY: the pseudo-handle of the calling thread.
    unsafe { SetThreadPriority(thread, priority.value()) }.map_err(io::Error::other)
}

/// [`prioritize_current_thread`] for the thread playing `role`, logging a
/// refusal as a warning: the thread then runs as it was, which is correct,
/// only less prompt.
pub fn prioritize_role(role: &str, priority: ThreadPriority) {
    if let Err(error) = prioritize_current_thread(priority) {
        tracing::warn!(%error, role, ?priority, "a thread keeps its priority");
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Threading::GetThreadPriority;

    use super::*;

    #[test]
    fn a_prioritized_thread_runs_at_its_priority() {
        std::thread::spawn(|| {
            prioritize_current_thread(ThreadPriority::Highest).expect("prioritized");
            // SAFETY: returns a pseudo-handle and has no preconditions.
            let thread = unsafe { GetCurrentThread() };
            // SAFETY: the pseudo-handle of the calling thread.
            let priority = unsafe { GetThreadPriority(thread) };
            assert_eq!(priority, THREAD_PRIORITY_HIGHEST.0);
        })
        .join()
        .expect("the thread ran");
    }
}
