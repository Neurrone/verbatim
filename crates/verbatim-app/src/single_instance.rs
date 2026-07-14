//! Single-instance startup: a newly started Verbatim replaces any running
//! instance, following NVDA's algorithm (`nvda/source/nvda.pyw`).
//!
//! Two mechanisms cooperate. First, the new process finds the old instance's
//! hidden main window by title, posts `WM_QUIT` so its GUI loop exits and its
//! normal teardown runs, waits up to four seconds, and falls back to
//! `TerminateProcess` with a further two-second wait. Second, a named mutex
//! serializes full startup, so the new instance does not proceed until the
//! old one's teardown has released it (or abandoned it by dying).
//!
//! The mutex name has no per-desktop suffix yet; the secure-desktop instance
//! that needs one arrives in milestone M8.

use std::io;

use windows::Win32::Foundation::{CloseHandle, HWND, WAIT_ABANDONED, WAIT_OBJECT_0, WPARAM};
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess,
    WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilter, FindWindowW, GetWindowThreadProcessId, MSGFLT_ADD, PostMessageW,
    WM_QUIT,
};
use windows::core::{HSTRING, PCWSTR, w};

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
fn replace_running_instance() {
    let title = HSTRING::from(WINDOW_TITLE);
    // wxWidgets registers its own window classes whose names have varied
    // across versions, so match by title alone and verify the process below.
    // SAFETY: FindWindowW with borrowed wide strings; the returned handle is
    // used immediately.
    let hwnd = unsafe { FindWindowW(PCWSTR::null(), &title) }.unwrap_or(HWND(std::ptr::null_mut()));
    if hwnd.0.is_null() {
        return;
    }

    let mut pid: u32 = 0;
    // SAFETY: hwnd came from FindWindowW just above; pid is a valid out
    // pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    if pid == 0 || pid == std::process::id() {
        return;
    }
    tracing::info!(old_pid = pid, "replacing running Verbatim instance");

    // SAFETY: OpenProcess/PostMessageW/WaitForSingleObject/TerminateProcess
    // on a process we just identified; the handle is closed on every path.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid) else {
            return;
        };
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
        let _ = CloseHandle(process);
    }
}

/// Lets a future lower-integrity replacer's `WM_QUIT` reach this process
/// (NVDA does the same); harmless when it fails.
fn allow_quit_across_integrity_levels() {
    // SAFETY: process-wide message-filter adjustment with constant arguments.
    unsafe {
        let _ = ChangeWindowMessageFilter(WM_QUIT, MSGFLT_ADD);
    }
}
