//! What the UIA fact binaries share (`uia-facts/readme.md`, "Shared
//! support"):
//!
//! - [`provider`]: the provider program's client side, launched from the
//!   fact binary's own folder in a job object, with its requests and logs.
//! - [`role`]: a second client process, the fact binary started again with
//!   `--role`, in a job object, reporting lines on a pipe.
//! - [`report`]: the header, one line per claim, the verdicts and the exit
//!   code; [`stats`] the medians, ranges and Wilson intervals they use.
//! - [`rop`]: the remote operation the facts send.
//! - [`uia`]: a client object on a thread of the multithreaded apartment,
//!   and the few calls every fact makes.
//! - [`precondition`]: an unlocked input desktop and cdb.
//! - [`events`]: an event handler that reports what it hears.
//! - [`args`]: `--arm`, `--trials`, `--pause` and `--role`.

pub mod args;
pub mod events;
pub mod job;
pub mod precondition;
pub mod provider;
pub mod report;
pub mod role;
pub mod rop;
pub mod stats;
pub mod uia;

use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// Microseconds on `QueryPerformanceCounter`'s clock, which every process
/// shares, so a fact binary's times and the provider program's log can be
/// compared.
#[must_use]
pub fn now_us() -> u64 {
    let mut count = 0i64;
    let mut frequency = 0i64;
    // SAFETY: a live `i64` to fill.
    let _ = unsafe { QueryPerformanceCounter(&raw mut count) };
    // SAFETY: as above.
    let _ = unsafe { QueryPerformanceFrequency(&raw mut frequency) };
    u64::try_from(i128::from(count) * 1_000_000 / i128::from(frequency)).unwrap_or(0)
}

/// Ends this process at once with `code`, without running UIA's detach
/// code, which can hang or crash (`books/dev/src/reference/uia-bugs.md`,
/// "A process that used UIA can hang or crash as it exits").
pub fn exit_now(code: u32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: no precondition.
    let process = unsafe { windows::Win32::System::Threading::GetCurrentProcess() };
    // SAFETY: the pseudo-handle of this process.
    let _ = unsafe { windows::Win32::System::Threading::TerminateProcess(process, code) };
    unreachable!("TerminateProcess of this process returns only on failure")
}
