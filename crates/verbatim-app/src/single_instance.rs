//! Single-instance startup: a newly started Verbatim replaces any running
//! instance, as NVDA does in `terminateRunningNVDA` and the startup mutex
//! after it (`nvda/source/nvda.pyw`).
//!
//! Two mechanisms cooperate. First, the new process finds the old instance's
//! hidden main window by title, checks that the window's process runs
//! Verbatim's executable, posts `WM_QUIT` so its GUI loop exits and its
//! normal teardown runs, and waits up to five seconds for its process to
//! exit (Dickson, 2026-10-10; NVDA waits four). If it is still running, it
//! is ended with `TerminateProcess` and given up to two seconds more to
//! exit, as in NVDA. Its outposts, focus listener and synthesizer hosts run
//! in its kill-on-close job objects, so they end with it, and the screen
//! reader flag it leaves set is set again by the new instance. Second, a
//! named mutex serializes full startup, so the new instance does not
//! proceed until the old one has released it, or abandoned it by being
//! ended. An old instance that cannot be ended, or does not exit once
//! ended, is left running, and the new instance shows NVDA's message box
//! saying so, worded for Verbatim, and does not start.
//!
//! Before posting `WM_QUIT`, the new process sets a named event that the
//! old instance created for itself, `Local\Verbatim.Replacing.<pid>`, so
//! the old instance knows it is being replaced rather than quit in any
//! other way: then it plays no exit sound and stops at once (Dickson,
//! 2026-10-10; NVDA plays its exit sound to the end). `WM_QUIT` cannot
//! carry this itself, since wxWidgets' event loop ends on it without
//! keeping its parameters.
//!
//! The mutex name has no per-desktop suffix yet; the secure-desktop instance
//! that needs one arrives in milestone M8.

use std::io;

use std::path::Path;

use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, SetEvent, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilter, FindWindowExW, GetWindowThreadProcessId, MB_OK, MSGFLT_ADD,
    MessageBoxW, PostMessageW, WM_QUIT,
};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};

/// The title of the hidden main frame, the rendezvous point a replacing
/// instance finds. Deliberately not localized: it must be stable across
/// locales for `FindWindowW` to work between differently configured builds.
const WINDOW_TITLE: &str = "Verbatim";

/// The named mutex serializing startup.
const MUTEX_NAME: PCWSTR = w!(r"Local\Verbatim");

/// How long a running instance gets to exit after `WM_QUIT` (Dickson,
/// 2026-10-10; NVDA's is 4000).
const REPLACED_EXIT_MS: u32 = 5000;

/// How long a running instance ended with `TerminateProcess` gets to exit,
/// as in NVDA.
const TERMINATED_EXIT_MS: u32 = 2000;

/// How long to wait for the startup mutex.
const MUTEX_WAIT_MS: u32 = 2000;

/// Holds the startup mutex for the life of this instance. Not `Send`: a
/// mutex is released only by the thread that owns it, so the guard stays on
/// the thread that acquired it.
pub struct InstanceGuard {
    mutex: windows::Win32::Foundation::HANDLE,
    /// This instance's replacement event, set by an instance replacing it;
    /// `None` if it could not be created.
    replacing: Option<HANDLE>,
}

impl InstanceGuard {
    /// Whether a newly started instance is replacing this one, that is, has
    /// set this instance's replacement event. Asked once the GUI loop has
    /// ended: the replacing instance sets the event before it posts the
    /// `WM_QUIT` that ends the loop.
    pub fn being_replaced(&self) -> bool {
        // SAFETY: polling the event handle this guard owns, without waiting.
        self.replacing
            .is_some_and(|event| unsafe { WaitForSingleObject(event, 0) } == WAIT_OBJECT_0)
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: `mutex` is the handle CreateMutexW returned, owned by
        // this thread (the guard is not `Send`), and not closed yet.
        let _ = unsafe { windows::Win32::System::Threading::ReleaseMutex(self.mutex) };
        // SAFETY: as above; closed once, here.
        let _ = unsafe { CloseHandle(self.mutex) };
        if let Some(event) = self.replacing {
            // SAFETY: the event handle this guard owns, closed once, here.
            let _ = unsafe { CloseHandle(event) };
        }
    }
}

/// The name of the event an instance replacing process `pid` sets.
fn replacing_event_name(pid: u32) -> HSTRING {
    HSTRING::from(format!(r"Local\Verbatim.Replacing.{pid}"))
}

/// Creates this process's replacement event, unset. Without it, a
/// replacing instance cannot say so, and this one plays its exit sound as
/// on any other quit.
fn create_replacing_event() -> Option<HANDLE> {
    let name = replacing_event_name(std::process::id());
    // SAFETY: a manual-reset event, initially unset, with default security
    // and a borrowed name; the handle is owned by the guard.
    match unsafe { CreateEventW(None, true, false, &name) } {
        Ok(event) => Some(event),
        Err(error) => {
            tracing::warn!(%error, "creating the replacement event failed");
            None
        }
    }
}

/// Tells process `pid` that it is being replaced, by setting its
/// replacement event. An instance without one is not told, and plays its
/// exit sound.
fn tell_being_replaced(pid: u32) {
    // SAFETY: opens an existing named event for setting; the handle is
    // checked and closed below.
    match unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &replacing_event_name(pid)) } {
        Ok(event) => {
            // SAFETY: `event` was just opened with EVENT_MODIFY_STATE.
            if let Err(error) = unsafe { SetEvent(event) } {
                tracing::warn!(old_pid = pid, %error, "setting the replacement event failed");
            }
            // SAFETY: the handle just opened, closed once.
            let _ = unsafe { CloseHandle(event) };
        }
        Err(error) => {
            tracing::warn!(old_pid = pid, %error, "opening the replacement event failed");
        }
    }
}

/// Says that a running instance could not be ended and this one will not
/// start, as NVDA does (`nvda.pyw` lines 157 to 166), and returns once the
/// box is dismissed.
fn show_not_ended() {
    let text = HSTRING::from(verbatim_i18n::messages::replace_failed());
    let title = HSTRING::from(verbatim_i18n::messages::replace_failed_title());
    // SAFETY: a modal message box with no owner and borrowed strings that
    // outlive the call.
    unsafe { MessageBoxW(None, &text, &title, MB_OK) };
}

/// Replaces any running instance, then acquires the startup mutex.
///
/// # Errors
///
/// Returns an error when a running instance could not be ended, after
/// saying so in a message box, when the mutex cannot be created, or when
/// another instance still holds it after the wait.
pub fn acquire_replacing() -> io::Result<InstanceGuard> {
    if replace_running_instance() == Replacement::StillRunning {
        // NVDA's message box, then no start (Dickson, 2026-10-10). Never
        // shown when the running instance exited or was ended.
        show_not_ended();
        return Err(io::Error::other(
            "another Verbatim instance is still running and could not be ended",
        ));
    }
    allow_quit_across_integrity_levels();

    // SAFETY: a constant name and default security; the handle is owned
    // by the returned guard, or closed below.
    let mutex = unsafe { CreateMutexW(None, false, MUTEX_NAME) }
        .map_err(|error| io::Error::other(format!("creating startup mutex: {error}")))?;
    // SAFETY: waiting on the mutex handle just created.
    let wait = unsafe { WaitForSingleObject(mutex, MUTEX_WAIT_MS) };
    if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
        // WAIT_ABANDONED means the previous instance ended without
        // releasing, by crashing or by being ended above; ownership still
        // transfers to us.
        if wait == WAIT_ABANDONED {
            tracing::warn!("previous instance abandoned the startup mutex (crashed or ended)");
        }
        Ok(InstanceGuard {
            mutex,
            replacing: create_replacing_event(),
        })
    } else {
        // SAFETY: the handle created above, not returned, closed once.
        let _ = unsafe { CloseHandle(mutex) };
        Err(io::Error::other(
            "another Verbatim instance is still running and did not exit in time",
        ))
    }
}

/// What became of a running instance.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Replacement {
    /// None was running, or it has exited.
    Gone,
    /// It could not be ended, or did not exit once ended.
    StillRunning,
}

/// Finds a running instance's hidden main window and shuts that instance
/// down with `WM_QUIT`, waiting up to [`REPLACED_EXIT_MS`] for its process
/// to exit, then ends it if it has not. [`Replacement::Gone`] when no
/// instance is running.
///
/// The title alone is not Verbatim's: Windows matches it without regard to
/// case, and a File Explorer window on a folder named "verbatim" has it too.
/// So every top-level window with the title is considered, and only one
/// whose process runs an executable of this one's file name is acted on.
fn replace_running_instance() -> Replacement {
    let title = HSTRING::from(WINDOW_TITLE);
    let own_name = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(std::ffi::OsStr::to_os_string));
    let Some(own_name) = own_name else {
        return Replacement::Gone;
    };
    let mut after: Option<HWND> = None;
    loop {
        // SAFETY: FindWindowExW with borrowed wide strings; `after` is a
        // window it returned on the previous pass (or none), and a window
        // that has since closed only ends the search early.
        let found = unsafe { FindWindowExW(None, after, PCWSTR::null(), &title) };
        let Ok(hwnd) = found else {
            return Replacement::Gone;
        };
        if hwnd.0.is_null() {
            return Replacement::Gone;
        }
        if let Some(replacement) = shut_down_if_verbatim(hwnd, &own_name) {
            return replacement;
        }
        after = Some(hwnd);
    }
}

/// Shuts down `hwnd`'s process if it is another Verbatim, that is, runs an
/// executable named `own_name`, ending it if it does not exit in time;
/// `None` if it is not one.
fn shut_down_if_verbatim(hwnd: HWND, own_name: &std::ffi::OsStr) -> Option<Replacement> {
    let mut pid: u32 = 0;
    // SAFETY: `hwnd` came from FindWindowExW; `pid` is a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    if pid == 0 || pid == std::process::id() {
        return None;
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
        return None;
    };
    let is_verbatim = image_path(process)
        .as_deref()
        .and_then(Path::file_name)
        .is_some_and(|name| name.eq_ignore_ascii_case(own_name));
    let mut replacement = None;
    if is_verbatim {
        tracing::info!(old_pid = pid, "replacing running Verbatim instance");
        // Set before WM_QUIT, so it is set by the time the old instance's
        // GUI loop has ended and it asks.
        tell_being_replaced(pid);
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
        replacement = Some(if wait == WAIT_OBJECT_0 {
            tracing::info!(old_pid = pid, "the running instance has exited");
            Replacement::Gone
        } else {
            if wait == WAIT_TIMEOUT {
                tracing::warn!(
                    old_pid = pid,
                    limit_ms = REPLACED_EXIT_MS,
                    "the running instance did not exit in time; ending it"
                );
            } else {
                tracing::warn!(
                    old_pid = pid,
                    "waiting for the running instance failed; ending it"
                );
            }
            // `process` stays open meanwhile, so `pid` names the same
            // process when it is opened again to end it.
            end_process(pid)
        });
    }
    // SAFETY: `process` is the handle opened above, closed once.
    unsafe {
        let _ = CloseHandle(process);
    }
    replacement
}

/// Ends process `pid` with `TerminateProcess` and waits up to
/// [`TERMINATED_EXIT_MS`] for it to exit, as NVDA's `terminateRunningNVDA`
/// does after its wait.
fn end_process(pid: u32) -> Replacement {
    // SAFETY: opening a process by id; the handle is closed below.
    let opened = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) };
    let process = match opened {
        Ok(process) => process,
        Err(error) => {
            tracing::error!(old_pid = pid, %error, "the running instance cannot be ended");
            return Replacement::StillRunning;
        }
    };
    // SAFETY: the handle just opened, with terminate access.
    if let Err(error) = unsafe { TerminateProcess(process, 1) } {
        tracing::warn!(old_pid = pid, %error, "ending the running instance failed");
    }
    // SAFETY: waiting on the process through its open handle, with
    // synchronize access; it is signalled when the process exits.
    let wait = unsafe { WaitForSingleObject(process, TERMINATED_EXIT_MS) };
    // SAFETY: the handle opened above, closed once.
    unsafe {
        let _ = CloseHandle(process);
    }
    if wait == WAIT_OBJECT_0 {
        tracing::info!(old_pid = pid, "the running instance was ended");
        Replacement::Gone
    } else {
        tracing::error!(
            old_pid = pid,
            limit_ms = TERMINATED_EXIT_MS,
            "the running instance did not exit once ended"
        );
        Replacement::StillRunning
    }
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
