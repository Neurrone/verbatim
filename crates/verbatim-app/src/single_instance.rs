//! Single-instance startup: a newly started Verbatim replaces any running
//! instance, following NVDA's algorithm (`nvda/source/nvda.pyw`) but for
//! its fallback.
//!
//! Two mechanisms cooperate. First, the new process finds the old instance's
//! hidden main window by title, checks that the window's process runs
//! Verbatim's executable, posts `WM_QUIT` so its GUI loop exits and its
//! normal teardown runs, and waits up to five seconds for its process to
//! exit (Dickson, 2026-10-10). NVDA ends a process that has not exited
//! after four seconds with `TerminateProcess` (`nvda.pyw` lines 110 to
//! 137); Verbatim never does, since that cut the old instance's teardown
//! short, killing its outposts rather than shutting them down and leaving
//! the screen reader flag set, and the process's exit is the evidence that
//! the teardown finished (Dickson, 2026-10-09, coherence review). Second, a
//! named mutex serializes full startup, so the new instance does not
//! proceed until the old one's teardown has released it (or abandoned it by
//! dying). An old instance still running when the five seconds pass still
//! holds the mutex, so the new instance's startup fails with an error and
//! the old one is left running.
//!
//! The mutex name has no per-desktop suffix yet; the secure-desktop instance
//! that needs one arrives in milestone M8.

use std::io;

use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilter, FindWindowExW, GetWindowThreadProcessId, MSGFLT_ADD, PostMessageW,
    WM_QUIT,
};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

/// The title of the hidden main frame, the rendezvous point a replacing
/// instance finds. Deliberately not localized: it must be stable across
/// locales for `FindWindowW` to work between differently configured builds.
const WINDOW_TITLE: &str = "Verbatim";

/// The named mutex serializing startup.
const MUTEX_NAME: PCWSTR = w!(r"Local\Verbatim");

/// How long a running instance gets to exit after `WM_QUIT` (Dickson,
/// 2026-10-10).
const REPLACED_EXIT_MS: u32 = 5000;

/// How long to wait for the startup mutex.
const MUTEX_WAIT_MS: u32 = 2000;

/// Holds the startup mutex for the life of this instance. Not `Send`: a
/// mutex is released only by the thread that owns it, so the guard stays on
/// the thread that acquired it.
pub struct InstanceGuard {
    mutex: windows::Win32::Foundation::HANDLE,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: `mutex` is the handle CreateMutexW returned, owned by
        // this thread (the guard is not `Send`), and not closed yet.
        let _ = unsafe { windows::Win32::System::Threading::ReleaseMutex(self.mutex) };
        // SAFETY: as above; closed once, here.
        let _ = unsafe { CloseHandle(self.mutex) };
    }
}

/// Replaces any running instance, then acquires the startup mutex.
///
/// # Errors
///
/// Returns an error when the mutex cannot be created or another instance
/// still holds it after the replacement attempt and the wait.
pub fn acquire_replacing() -> io::Result<InstanceGuard> {
    replace_running_instance();
    allow_quit_across_integrity_levels();

    // SAFETY: a constant name and default security; the handle is owned
    // by the returned guard, or closed below.
    let mutex = unsafe { CreateMutexW(None, false, MUTEX_NAME) }
        .map_err(|error| io::Error::other(format!("creating startup mutex: {error}")))?;
    // SAFETY: waiting on the mutex handle just created.
    let wait = unsafe { WaitForSingleObject(mutex, MUTEX_WAIT_MS) };
    if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
        // WAIT_ABANDONED means the previous instance died without
        // releasing; ownership still transfers to us.
        if wait == WAIT_ABANDONED {
            tracing::warn!("previous instance abandoned the startup mutex (crash?)");
        }
        Ok(InstanceGuard { mutex })
    } else {
        // SAFETY: the handle created above, not returned, closed once.
        let _ = unsafe { CloseHandle(mutex) };
        Err(io::Error::other(
            "another Verbatim instance is still running and did not exit in time",
        ))
    }
}

/// Finds a running instance's hidden main window and shuts that instance
/// down with `WM_QUIT`, waiting up to [`REPLACED_EXIT_MS`] for its process
/// to exit. A no-op when no instance is running.
///
/// The title alone is not Verbatim's: Windows matches it without regard to
/// case, and a File Explorer window on a folder named "verbatim" has it too.
/// So every top-level window with the title is considered, and only one
/// whose process runs an executable of this one's file name is acted on.
fn replace_running_instance() {
    let title = HSTRING::from(WINDOW_TITLE);
    let own_name = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(std::ffi::OsStr::to_os_string));
    let Some(own_name) = own_name else {
        return;
    };
    let mut after: Option<HWND> = None;
    loop {
        // SAFETY: FindWindowExW with borrowed wide strings; `after` is a
        // window it returned on the previous pass (or none), and a window
        // that has since closed only ends the search early.
        let found = unsafe { FindWindowExW(None, after, PCWSTR::null(), &title) };
        let Ok(hwnd) = found else {
            return;
        };
        if hwnd.0.is_null() {
            return;
        }
        if shut_down_if_verbatim(hwnd, &own_name) {
            return;
        }
        after = Some(hwnd);
    }
}

/// Shuts down `hwnd`'s process if it is another Verbatim, that is, runs an
/// executable named `own_name`; returns whether it was one.
fn shut_down_if_verbatim(hwnd: HWND, own_name: &std::ffi::OsStr) -> bool {
    let mut pid: u32 = 0;
    // SAFETY: `hwnd` came from FindWindowExW; `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    if pid == 0 || pid == std::process::id() {
        return false;
    }
    // SAFETY: opening a process by id; the handle is closed on every path.
    let opened = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    };
    let Ok(process) = opened else {
        return false;
    };
    let is_verbatim = image_path(process)
        .as_deref()
        .and_then(Path::file_name)
        .is_some_and(|name| name.eq_ignore_ascii_case(own_name));
    if is_verbatim {
        tracing::info!(old_pid = pid, "replacing running Verbatim instance");
        // SAFETY: a message with no pointer parameters, posted to a window
        // of the process just identified.
        let _ = unsafe {
            PostMessageW(
                Some(hwnd),
                WM_QUIT,
                WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            )
        };
        tracing::info!(
            old_pid = pid,
            "waiting for the running instance's teardown to finish"
        );
        // SAFETY: waiting on the process through its open handle, with
        // synchronize access; it is signalled when the process exits.
        let wait = unsafe { WaitForSingleObject(process, REPLACED_EXIT_MS) };
        if wait == WAIT_OBJECT_0 {
            tracing::info!(old_pid = pid, "the running instance has exited");
        } else if wait == WAIT_TIMEOUT {
            tracing::warn!(
                old_pid = pid,
                limit_ms = REPLACED_EXIT_MS,
                "the running instance did not exit in time; it is left running"
            );
        } else {
            tracing::warn!(old_pid = pid, "waiting for the running instance failed");
        }
    }
    // SAFETY: `process` is the handle opened above, closed once.
    unsafe {
        let _ = CloseHandle(process);
    }
    is_verbatim
}

/// The full path of `process`'s executable, or `None` if it cannot be read.
fn image_path(process: HANDLE) -> Option<std::path::PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: `buffer` holds `length` UTF-16 units and outlives the call,
    // which writes at most that many and updates `length`.
    unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    }
    .ok()?;
    buffer.truncate(usize::try_from(length).ok()?);
    Some(std::path::PathBuf::from(String::from_utf16_lossy(&buffer)))
}

/// Lets a future lower-integrity replacer's `WM_QUIT` reach this process
/// (NVDA does the same); harmless when it fails.
fn allow_quit_across_integrity_levels() {
    // SAFETY: process-wide message-filter adjustment with constant arguments.
    unsafe {
        let _ = ChangeWindowMessageFilter(WM_QUIT, MSGFLT_ADD);
    }
}
