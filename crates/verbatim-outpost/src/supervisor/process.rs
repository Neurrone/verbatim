//! Process creation for outposts and the focus listener, through
//! `verbatim-process` (the job object, the inherited pipes, and the per-role
//! log file), and two local process and window queries the owner uses for
//! its decisions.

use std::io;
use std::path::Path;

use windows::Win32::Foundation::{HWND, LPARAM, STILL_ACTIVE};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsHungAppWindow, IsWindowVisible,
};
use windows::core::{BOOL, PWSTR};

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
    /// Held so the kernel kills the process when this handle closes. Every
    /// ending, for any reason, closes it.
    pub(super) _contained: Contained,
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
            _contained: contained,
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
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid.0) else {
            return false;
        };
        let mut exit_code = 0u32;
        let alive = GetExitCodeProcess(handle, &raw mut exit_code).is_ok()
            && exit_code == STILL_ACTIVE.0.cast_unsigned();
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        alive
    }
}

/// Whether any visible top-level window of `pid` is reported hung by the
/// system (`IsHungAppWindow`, a local call). While it is, abandoned workers in
/// that application's outpost are expected, and a replacement outpost would
/// hang the same way.
pub(super) fn application_is_hung(pid: Pid) -> bool {
    struct Search {
        pid: u32,
        hung: bool,
    }
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Search` passed below, alive for the call.
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut owner = 0u32;
        // SAFETY: both calls tolerate any window handle.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&raw mut owner));
            if owner == search.pid
                && IsWindowVisible(hwnd).as_bool()
                && IsHungAppWindow(hwnd).as_bool()
            {
                search.hung = true;
                return false.into();
            }
        }
        true.into()
    }
    let mut search = Search {
        pid: pid.0,
        hung: false,
    };
    // SAFETY: `visit` reads the search state passed here and nothing else; the
    // state outlives the synchronous EnumWindows call.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM((&raw mut search) as isize));
    }
    search.hung
}

/// `pid`'s executable name without its extension, lower-cased, to name its
/// outpost's log: `notepad` for Notepad. `None` when it cannot be read.
fn image_stem(pid: Pid) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: OpenProcess with a query-only right fails safely; the buffer
    // outlives the call, which writes at most `length` units; the handle is
    // closed before returning.
    let read = unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid.0).ok()?;
        let read = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        );
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        read
    };
    read.ok()?;
    let path = String::from_utf16_lossy(&buffer[..usize::try_from(length).ok()?]);
    Path::new(&path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_lowercase)
}
