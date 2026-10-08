//! Watching for Windows Terminal windows that no launched process opened,
//! for
//! [`Request::TakeForeignTerminalWindows`](crate::protocol::Request::TakeForeignTerminalWindows).
//!
//! The end-to-end suite runs on the owner's desktop, where the owner's own
//! Windows Terminal hosts their work. The suite's terminal scenarios drive a
//! portable copy of their own, which the agent launches, and every other
//! program it starts is started with no console window; so no window of any
//! other Windows Terminal may appear while the suite runs. One that does is
//! a console handed to the default terminal application, or a scenario
//! reaching the owner's Windows Terminal, and fails the run.
//!
//! A thread installs an out-of-context `WinEvent` hook for windows being
//! shown and pumps its messages; each top-level Windows Terminal window
//! shown (the class `CASCADIA_HOSTING_WINDOW_CLASS` in `WindowsTerminal.exe`)
//! whose process is not in the job of any process the agent launched is
//! recorded. The thread starts with the first request, which returns once
//! its hook is installed.

use std::sync::mpsc;
use std::sync::{Mutex, OnceLock, PoisonError};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    CHILDID_SELF, DispatchMessageW, EVENT_OBJECT_SHOW, GA_ROOT, GetAncestor, GetMessageW, MSG,
    OBJID_WINDOW, TranslateMessage, WINEVENT_OUTOFCONTEXT,
};

use crate::protocol::WindowInfo;

/// Windows Terminal's executable.
const IMAGE: &str = "WindowsTerminal.exe";

/// The class of a Windows Terminal window.
const CLASS: &str = "CASCADIA_HOSTING_WINDOW_CLASS";

/// The windows recorded since the last request.
static SHOWN: Mutex<Vec<WindowInfo>> = Mutex::new(Vec::new());

/// Whether the watching thread started, or why it could not.
static WATCHING: OnceLock<Result<(), String>> = OnceLock::new();

/// Takes the Windows Terminal windows shown since the last call by
/// processes outside every launched job, starting the watch on the first
/// call, which then has nothing to report.
///
/// # Errors
///
/// Returns an error if the watch cannot be started.
pub(crate) fn take_foreign() -> std::io::Result<Vec<WindowInfo>> {
    WATCHING
        .get_or_init(start)
        .clone()
        .map_err(std::io::Error::other)?;
    Ok(std::mem::take(
        &mut *SHOWN.lock().unwrap_or_else(PoisonError::into_inner),
    ))
}

/// Starts the watching thread and waits until its hook is installed.
fn start() -> Result<(), String> {
    let (installed_tx, installed) = mpsc::channel();
    std::thread::Builder::new()
        .name("verbatim-agent-terminals".to_owned())
        .spawn(move || watch(&installed_tx))
        .map_err(|error| error.to_string())?;
    installed
        .recv()
        .map_err(|_| "the terminal window watch ended before its hook was installed".to_owned())?
}

/// The watching thread: installs the hook, says so, and pumps messages for
/// as long as the agent runs.
fn watch(installed: &mpsc::Sender<Result<(), String>>) {
    // SAFETY: an out-of-context hook with a callback that lives for the
    // process; it is never removed, as the thread runs for the agent's
    // life.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_SHOW,
            None,
            Some(on_show),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if hook.is_invalid() {
        let _ = installed.send(Err(
            "the window-shown hook could not be installed".to_owned()
        ));
        return;
    }
    let _ = installed.send(Ok(()));
    let mut message = MSG::default();
    // SAFETY: a standard message loop over a message owned here.
    while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.0 > 0 {
        // SAFETY: as above.
        let _ = unsafe { TranslateMessage(&raw const message) };
        // SAFETY: as above.
        unsafe { DispatchMessageW(&raw const message) };
    }
}

/// Records a top-level Windows Terminal window shown by a process outside
/// every launched job.
unsafe extern "system" fn on_show(
    _hook: HWINEVENTHOOK,
    _event: u32,
    window: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if id_object != OBJID_WINDOW.0 || id_child != i32::try_from(CHILDID_SELF).unwrap_or(0) {
        return;
    }
    // SAFETY: tolerates any handle.
    if unsafe { GetAncestor(window, GA_ROOT) } != window {
        return;
    }
    let info = crate::desktop::window_info(window);
    if !info.image.eq_ignore_ascii_case(IMAGE) || info.class != CLASS {
        return;
    }
    if crate::process::in_launched_job(info.pid) {
        return;
    }
    SHOWN
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(info);
}
