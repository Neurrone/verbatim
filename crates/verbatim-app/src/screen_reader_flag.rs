//! Windows' screen reader flag (`SPI_SETSCREENREADER`): set while Verbatim
//! runs and cleared when it exits, as NVDA does (`nvda.pyw` lines 285 and
//! 286, and 308 and 309, through `winUser.setSystemScreenReaderFlag`, which
//! updates the user's profile and tells every window). Programs change
//! their behaviour for a screen reader by it: Windows PowerShell leaves
//! `PSReadLine` out and says so when it starts.
//!
//! The flag is cleared on every exit this process can see:
//!
//! - Verbatim exiting normally, from its menu, the control plane, or a
//!   replacing instance's `WM_QUIT`, and a startup that fails: the guard is
//!   dropped as `main` returns, before the startup mutex is released, so a
//!   replacing instance sets it again only once this one has cleared it.
//! - A panic on the main thread, which unwinds through `main`.
//! - Control+C, Control+Break or the console window closed, when Verbatim
//!   runs in a console: the console's control handler clears it before the
//!   process ends.
//! - The user signing out or Windows shutting down: a hidden window of its
//!   own clears it when the session ends.
//!
//! It is left set when the process ends without running any code of its
//! own: a crash, a panic that aborts, or the process ended from outside
//! (Task Manager, or a replacing instance's `TerminateProcess` when this one
//! does not exit in time, which then sets it again itself). As with NVDA,
//! the flag is saved in the user's profile, so it stays set until a screen
//! reader next clears it.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW,
    SPI_SETSCREENREADER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SystemParametersInfoW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_ENDSESSION, WNDCLASSW,
};
use windows::core::{BOOL, w};

/// Whether this process set the flag and has not cleared it since: the
/// exits that clear it can overlap (a console closed while Verbatim shuts
/// down), and it is cleared once.
static RAISED: AtomicBool = AtomicBool::new(false);

/// Holds the screen reader flag set for this instance's life; dropping it
/// clears the flag.
pub(crate) struct ScreenReaderFlag {
    _private: (),
}

impl ScreenReaderFlag {
    /// Sets the flag and arranges for it to be cleared on the exits the
    /// module describes. A failure to set it is logged, and Verbatim runs on.
    pub(crate) fn raise() -> Self {
        set(true);
        // SAFETY: `on_console_control` has the handler routine's signature
        // and lives for the whole process; adding a handler has no other
        // requirement.
        if let Err(error) = unsafe { SetConsoleCtrlHandler(Some(on_console_control), true) } {
            tracing::warn!(%error, "the console control handler that clears the screen reader flag could not be added");
        }
        if let Err(error) = std::thread::Builder::new()
            .name("verbatim-session-end".to_owned())
            .spawn(watch_session_end)
        {
            tracing::warn!(%error, "the thread that clears the screen reader flag when the session ends could not start");
        }
        Self { _private: () }
    }
}

impl Drop for ScreenReaderFlag {
    fn drop(&mut self) {
        clear();
    }
}

/// Sets the flag, saving it in the user's profile and telling every window,
/// as NVDA does.
fn set(on: bool) {
    // SAFETY: SPI_SETSCREENREADER takes its value in `uiParam` and no
    // pointer.
    let result = unsafe {
        SystemParametersInfoW(
            SPI_SETSCREENREADER,
            u32::from(on),
            None,
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )
    };
    match result {
        Ok(()) => {
            RAISED.store(on, Ordering::SeqCst);
            tracing::info!(on, "Windows' screen reader flag set");
        }
        Err(error) => tracing::warn!(on, %error, "Windows' screen reader flag could not be set"),
    }
}

/// Clears the flag, when this process set it and has not cleared it yet.
fn clear() {
    if RAISED.swap(false, Ordering::SeqCst) {
        set(false);
    }
}

/// The console's control handler: clears the flag, then lets the event's
/// usual handling end the process.
unsafe extern "system" fn on_console_control(_event: u32) -> BOOL {
    clear();
    false.into()
}

/// Runs a hidden window of its own, which Windows tells when the session
/// ends: Verbatim's other windows belong to its GUI, and none of them
/// clears the flag first.
fn watch_session_end() {
    // SAFETY: the module handle of this executable, which outlives the
    // window.
    let instance = match unsafe { GetModuleHandleW(None) } {
        Ok(module) => module.into(),
        Err(error) => {
            tracing::warn!(%error, "no module handle for the session-end window");
            return;
        }
    };
    let class = WNDCLASSW {
        lpfnWndProc: Some(session_window),
        hInstance: instance,
        lpszClassName: w!("VerbatimSessionEnd"),
        ..WNDCLASSW::default()
    };
    // SAFETY: the class names a static string and a window procedure that
    // lives for the whole process.
    if unsafe { RegisterClassW(&raw const class) } == 0 {
        tracing::warn!(
            error = %windows::core::Error::from_thread(),
            "the session-end window's class could not be registered"
        );
        return;
    }
    // SAFETY: a hidden top-level window of the class just registered, with
    // no parent, menu or creation data.
    let created = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("VerbatimSessionEnd"),
            w!(""),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
    };
    if let Err(error) = created {
        tracing::warn!(%error, "the session-end window could not be created");
        return;
    }
    let mut message = MSG::default();
    // SAFETY: this thread's own message queue; the loop ends only with the
    // process.
    while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.as_bool() {
        // SAFETY: a message just retrieved for this thread.
        unsafe { DispatchMessageW(&raw const message) };
    }
}

/// The session-end window's procedure: the session ending clears the flag.
unsafe extern "system" fn session_window(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_ENDSESSION && wparam.0 != 0 {
        clear();
        return LRESULT(0);
    }
    // SAFETY: the arguments this procedure was called with.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}
