//! Process creation for outposts and the focus listener, through
//! `verbatim-process` (the job object, the inherited pipes, and the per-role
//! log file), and two local process and window queries the owner uses for
//! its decisions.

use std::io;
use std::path::Path;

use windows::Win32::Foundation::STILL_ACTIVE;
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

use verbatim_model::Pid;
pub(super) use verbatim_process::ChildPipes;
use verbatim_process::{ChildSpec, Contained};

/// A 200 MB per-outpost memory cap: past it a leaking outpost's allocations
/// fail, the resulting abort ends it, and the supervisor replaces it.
const OUTPOST_MEMORY_CAP: usize = 200 * 1024 * 1024;

/// What to launch.
#[derive(Clone, Copy, Debug)]
pub(super) enum Role {
    /// An outpost watching one application for its whole life (decision D9).
    Outpost(Pid),
    /// The focus listener (decision D13).
    Listener,
}

/// A launched process: its job, which kills it when dropped.
pub(super) struct Launched {
    /// The process in its job: waited on for a clean exit, and killed
    /// through the job when it does not exit in time. Every ending, for
    /// any reason, closes the job handle, which kills the process if it is
    /// still running.
    pub(super) contained: Contained,
    /// The process's own id, for logs.
    pub(super) process_id: Pid,
}

/// Launches `verbatim-outpost.exe` from `exe_path` in `role`, contained in
/// a kill-on-close job with the memory cap (see `verbatim-process`). An
/// outpost is told `options` on its command line (`--classic-uia` when
/// remote operations are off); the listener reads no application and takes
/// none.
pub(super) fn launch(
    exe_path: &Path,
    role: Role,
    options: crate::OutpostOptions,
) -> io::Result<(Launched, ChildPipes)> {
    let classic_uia = if options.remote_operations {
        ""
    } else {
        " --classic-uia"
    };
    let (log_stem, arguments): (String, Box<dyn Fn(usize, usize) -> String>) = match role {
        Role::Outpost(pid) => (
            match image_stem(pid) {
                Some(image) => format!("outpost-{image}-{}", pid.0),
                None => format!("outpost-{}", pid.0),
            },
            Box::new(move |pipe_in, pipe_out| {
                format!(
                    "--pipe-in {pipe_in} --pipe-out {pipe_out} --target-pid {}{classic_uia}",
                    pid.0
                )
            }),
        ),
        Role::Listener => (
            "listener".to_owned(),
            Box::new(|pipe_in, pipe_out| {
                format!("--listener --pipe-in {pipe_in} --pipe-out {pipe_out}")
            }),
        ),
    };
    let (contained, pipes) = verbatim_process::launch(&ChildSpec {
        exe: exe_path,
        arguments: &*arguments,
        log_stem: &log_stem,
        memory_cap: Some(OUTPOST_MEMORY_CAP),
        from_child_buffer: 0,
    })?;
    let process_id = Pid(contained.pid);
    Ok((
        Launched {
            contained,
            process_id,
        },
        pipes,
    ))
}

/// Whether `pid` names a process that is still running. Pid reuse is a
/// known, accepted imprecision, the same trade every Win32 API taking a bare
/// pid makes.
pub(super) fn process_is_alive(pid: Pid) -> bool {
    // SAFETY: OpenProcess with a query-only access right fails safely on an
    // invalid or inaccessible pid; the handle is closed before returning.
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid.0) })
    else {
        return false;
    };
    let mut exit_code = 0u32;
    // SAFETY: `handle` is open with query access; `exit_code` is a local.
    let alive = unsafe { GetExitCodeProcess(handle, &raw mut exit_code) }.is_ok()
        && exit_code == STILL_ACTIVE.0.cast_unsigned();
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
    alive
}

/// Whether any visible top-level window of `pid` is reported hung by the
/// system (`IsHungAppWindow`, a local call). While it is, abandoned workers in
/// that application's outpost are expected, and a replacement outpost would
/// hang the same way.
pub(super) fn application_is_hung(pid: Pid) -> bool {
    use crate::outpost::window::{top_level_windows, window_is_hung, window_is_visible};
    top_level_windows(pid.0)
        .into_iter()
        .any(|window| window_is_visible(window) && window_is_hung(window))
}

/// `pid`'s executable name without its extension, lower-cased, to name its
/// outpost's log: `notepad` for Notepad. `None` when it cannot be read.
fn image_stem(pid: Pid) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: OpenProcess with a query-only right fails safely; the handle
    // is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid.0) }.ok()?;
    // SAFETY: `handle` is open with query access; the buffer outlives the
    // call, which writes at most `length` units.
    let read = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
    read.ok()?;
    let path = String::from_utf16_lossy(&buffer[..usize::try_from(length).ok()?]);
    Path::new(&path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_lowercase)
}
