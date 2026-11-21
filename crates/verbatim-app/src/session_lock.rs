//! The session's lock state for Core (`books/dev/src/reference/crates/verbatim-core.md`, "The
//! secure desktop and the lock state"): queried once at
//! startup and then followed by Windows' session notifications, each
//! entering Core's event queue as `QueueInput::Lock`. Core counts unknown
//! as locked (security mitigation B3).
//!
//! A thread of its own makes a message-only window, registers it for the
//! session's notifications (`WTSRegisterSessionNotification`), then
//! queries the state (`WTSQuerySessionInformationW` with
//! `WTSSessionInfoEx`, as NVDA queries it, `winAPI/sessionTracking.py` 167
//! to 182 and 263 to 281) and posts the answer before it takes any
//! notification, so no lock or unlock between the two is lost and the
//! reducer takes them in the order they happened. The thread waits in
//! `GetMessageW` and wakes only for a notification.

use std::cell::RefCell;

use verbatim_model::TraceId;
use verbatim_model::request::{QueueInput, SessionLock};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{
    NOTIFY_FOR_THIS_SESSION, WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK,
    WTS_SESSIONSTATE_UNLOCK, WTSFreeMemory, WTSINFOEXW, WTSQuerySessionInformationW,
    WTSRegisterSessionNotification, WTSSessionInfoEx,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
    RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_WTSSESSION_CHANGE, WNDCLASSW,
    WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
};
use windows::core::{PWSTR, w};

use crate::{Commands, ShellCommand};
use verbatim_model::Input;

thread_local! {
    /// The reducer's port, for the window procedure on this module's
    /// thread.
    static PORT: RefCell<Option<Commands>> = const { RefCell::new(None) };
}

/// Starts the thread that reports the lock state to Core through
/// `commands`. A failure to start it, or to make or register its window,
/// is logged, the startup query still answering when it can, and
/// Verbatim runs on with the state Core last heard.
pub(crate) fn start(commands: Commands) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("verbatim-session-lock".to_owned())
        .spawn(move || run(commands))
        .map(drop)
}

/// The thread: the window and its registration, the startup query, then
/// the notifications.
fn run(commands: Commands) {
    let window = notification_window();
    post(&commands, query());
    if window.is_none() {
        return;
    }
    PORT.with(|port| *port.borrow_mut() = Some(commands));
    let mut message = MSG::default();
    // SAFETY: this thread's own message queue; the loop ends only with the
    // process.
    while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.as_bool() {
        // SAFETY: a message just retrieved for this thread.
        unsafe { DispatchMessageW(&raw const message) };
    }
}

/// Makes this thread's message-only window and registers it for the
/// session's notifications, `None` when either fails.
fn notification_window() -> Option<HWND> {
    // SAFETY: the module handle of this process, never freed.
    let instance = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module.into(),
        Err(error) => {
            tracing::warn!(%error, "no module handle for the session lock window");
            return None;
        }
    };
    let class = WNDCLASSW {
        lpfnWndProc: Some(session_window),
        hInstance: instance,
        lpszClassName: w!("VerbatimSessionLock"),
        ..WNDCLASSW::default()
    };
    // SAFETY: the class names a static string and a window procedure that
    // lives for the whole process.
    if unsafe { RegisterClassW(&raw const class) } == 0 {
        tracing::warn!(
            error = %windows::core::Error::from_thread(),
            "the session lock window's class could not be registered"
        );
        return None;
    }
    // SAFETY: a message-only window of the class just registered, with no
    // menu or creation data.
    let window = match unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("VerbatimSessionLock"),
            w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
    } {
        Ok(window) => window,
        Err(error) => {
            tracing::warn!(%error, "the session lock window could not be created");
            return None;
        }
    };
    // SAFETY: a window this thread owns, registered for this session only.
    if let Err(error) = unsafe { WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) } {
        tracing::warn!(%error, "the session lock notifications could not be registered");
        return None;
    }
    Some(window)
}

/// The session's lock state now: locked, unlocked, or unknown when the
/// query fails or its flags name neither, as NVDA reads a mixed or
/// unexpected value (`winAPI/sessionTracking.py` 263 to 281).
fn query() -> SessionLock {
    let mut buffer = PWSTR::null();
    let mut bytes = 0u32;
    // SAFETY: the current session on this server; the buffer is the
    // system's, freed below.
    if let Err(error) = unsafe {
        WTSQuerySessionInformationW(
            Some(WTS_CURRENT_SERVER_HANDLE),
            WTS_CURRENT_SESSION,
            WTSSessionInfoEx,
            &raw mut buffer,
            &raw mut bytes,
        )
    } {
        tracing::warn!(%error, "the session's lock state could not be queried; counted as unknown");
        return SessionLock::Unknown;
    }
    let size = usize::try_from(bytes).unwrap_or(0);
    let lock = if buffer.is_null() || size < size_of::<WTSINFOEXW>() {
        SessionLock::Unknown
    } else {
        // SAFETY: the query answered a buffer of at least one
        // `WTSINFOEXW`, which `WTSSessionInfoEx` names; it is read
        // unaligned, since the buffer is typed as a string.
        let info = unsafe { buffer.0.cast::<WTSINFOEXW>().read_unaligned() };
        if info.Level == 1 {
            // SAFETY: level 1 names the union's level-1 member.
            lock_of(unsafe { info.Data.WTSInfoExLevel1 }.SessionFlags)
        } else {
            SessionLock::Unknown
        }
    };
    // SAFETY: the buffer the query allocated, freed once.
    unsafe { WTSFreeMemory(buffer.0.cast()) };
    lock
}

/// The lock state a session's flags name.
fn lock_of(flags: i32) -> SessionLock {
    match u32::try_from(flags) {
        Ok(WTS_SESSIONSTATE_LOCK) => SessionLock::Locked,
        Ok(WTS_SESSIONSTATE_UNLOCK) => SessionLock::Unlocked,
        _ => SessionLock::Unknown,
    }
}

/// Posts `lock` to Core's event queue.
fn post(commands: &Commands, lock: SessionLock) {
    let _ = commands.send(ShellCommand::Input(Box::new(Input::Queue {
        trace_id: TraceId::mint(),
        at_us: verbatim_process::clock::now_us(),
        input: QueueInput::Lock(lock),
    })));
}

/// The window's procedure: a lock or unlock of this session is posted.
unsafe extern "system" fn session_window(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_WTSSESSION_CHANGE {
        let lock = match u32::try_from(wparam.0) {
            Ok(WTS_SESSION_LOCK) => Some(SessionLock::Locked),
            Ok(WTS_SESSION_UNLOCK) => Some(SessionLock::Unlocked),
            _ => None,
        };
        if let Some(lock) = lock {
            PORT.with(|port| {
                if let Some(commands) = port.borrow().as_ref() {
                    post(commands, lock);
                }
            });
        }
        return LRESULT(0);
    }
    // SAFETY: the arguments this procedure was called with.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session's flags: lock and unlock name their states, and the
    /// unknown flag or a mixed value is unknown.
    #[test]
    fn a_sessions_flags_name_its_lock_state() {
        assert_eq!(lock_of(0), SessionLock::Locked);
        assert_eq!(lock_of(1), SessionLock::Unlocked);
        assert_eq!(lock_of(-1), SessionLock::Unknown);
        assert_eq!(lock_of(3), SessionLock::Unknown);
    }
}
