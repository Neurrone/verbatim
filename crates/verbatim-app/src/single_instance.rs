//! Single-instance startup: a newly started Verbatim replaces any running
//! instance, following NVDA's algorithm (`nvda/source/nvda.pyw`).
//!
//! Two mechanisms cooperate. First, the new process finds the old instance's
//! hidden main window by title, checks that the window's process runs
//! Verbatim's executable, posts `WM_QUIT` so its GUI loop exits and its
//! normal teardown runs, waits up to four seconds, and falls back to
//! `TerminateProcess` with a further two-second wait. Second, a named mutex
//! serializes full startup, so the new instance does not proceed until the
//! old one's teardown has released it (or abandoned it by dying).
//!
//! The mutex name has no per-desktop suffix yet; the secure-desktop instance
//! that needs one arrives in milestone M8.

use std::io;

use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW, TerminateProcess,
    WaitForSingleObject,
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

/// How long the old instance gets to exit cleanly after `WM_QUIT`.
const GRACEFUL_EXIT_MS: u32 = 4000;
/// How long the old instance gets after `TerminateProcess`.
const TERMINATE_WAIT_MS: u32 = 2000;
/// How long to wait for the startup mutex.
const MUTEX_WAIT_MS: u32 = 2000;

/// Holds the startup mutex for the life of this instance.
pub struct InstanceGuard {
    mutex: windows::Win32::Foundation::HANDLE,
}

// SAFETY: the wrapped mutex handle is only used by Drop and is valid for the
// process's lifetime; kernel handles may be closed from any thread.
unsafe impl Send for InstanceGuard {}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: `mutex` is the handle CreateMutexW returned and has not
        // been closed elsewhere.
        unsafe {
            let _ = windows::Win32::System::Threading::ReleaseMutex(self.mutex);
            let _ = CloseHandle(self.mutex);
        }
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

    // SAFETY: standard mutex creation and wait; the handle is owned by the
    // returned guard.
    unsafe {
        let mutex = CreateMutexW(None, false, MUTEX_NAME)
            .map_err(|error| io::Error::other(format!("creating startup mutex: {error}")))?;
        let wait = WaitForSingleObject(mutex, MUTEX_WAIT_MS);
        if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
            // WAIT_ABANDONED means the previous instance died without
            // releasing; ownership still transfers to us.
            if wait == WAIT_ABANDONED {
                tracing::warn!("previous instance abandoned the startup mutex (crash?)");
            }
            Ok(InstanceGuard { mutex })
        } else {
            let _ = CloseHandle(mutex);
            Err(io::Error::other(
                "another Verbatim instance is still running and did not exit in time",
            ))
        }
    }
}

/// Finds a running instance's hidden main window and shuts that instance
/// down: `WM_QUIT` first, `TerminateProcess` as the fallback. A no-op when no
/// instance is running.
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
            PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
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
        // SAFETY: posting to a window of the process just identified, and
        // waiting on and terminating that process through its open handle.
        unsafe {
            let _ = PostMessageW(
                Some(hwnd),
                WM_QUIT,
                WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );
            if WaitForSingleObject(process, GRACEFUL_EXIT_MS) != WAIT_OBJECT_0 {
                tracing::warn!(old_pid = pid, "old instance ignored WM_QUIT; terminating");
                let _ = TerminateProcess(process, 1);
                let _ = WaitForSingleObject(process, TERMINATE_WAIT_MS);
            }
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
