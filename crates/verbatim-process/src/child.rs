//! What every child process Verbatim launches does as it starts and ends:
//! installing its log's tracing subscriber ([`init_tracing`]), reporting
//! an error to standard error without panicking
//! ([`report_error!`](crate::report_error)), and ending without running
//! DLLs' detach code ([`end_without_detach`]).
//! The outpost and the focus listener both use them.

/// Writes a line to standard error, as `eprintln!` does, but ignores a
/// failed write instead of panicking: a full disk or a closed pipe under
/// redirected output must not take the process down.
#[macro_export]
macro_rules! report_error {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

/// Ends this process with `code` once an outpost or the listener has shut
/// down cleanly: every UIA handler and hook removed, every call finished,
/// every object released, and COM left. It ends with `TerminateProcess`
/// rather than by returning from `main`, so no DLL's process-detach code
/// runs: `UIAutomationCore.dll`'s sometimes hangs or crashes in a process
/// that has used UIA as a client (`docs/architecture.md`, "Process
/// lifetime"). Nothing is left for that code to do; standard error, the
/// log, is written unbuffered.
pub fn end_without_detach(code: u32) -> ! {
    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    // SAFETY: the calling process's pseudo-handle, which needs no closing.
    let this_process = unsafe { GetCurrentProcess() };
    // SAFETY: ends the calling process.
    let ended = unsafe { TerminateProcess(this_process, code) };
    // Unreachable unless the call failed; exit the ordinary way then.
    report_error!("the process could not end itself ({ended:?}); exiting normally");
    std::process::exit(i32::try_from(code).unwrap_or(1))
}

/// Installs a tracing subscriber writing to this process's stderr (which the
/// supervisor redirects to a log file). Respects `RUST_LOG`, defaulting to
/// `info` so the worker and watchdog diagnostics land without extra
/// configuration. Never a hard failure: if a subscriber is somehow already
/// set, `try_init` returns an error that is ignored.
pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        // A write that fails (a full disk, a closed pipe) is dropped: by
        // default the formatter reports it with `eprint!`, which panics when
        // standard error fails too.
        .log_internal_errors(false)
        .with_writer(std::io::stderr)
        .try_init();
}
