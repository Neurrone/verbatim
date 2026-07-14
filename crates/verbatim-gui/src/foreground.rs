//! Taking the foreground for popups — NVDA's `prePopup`/`postPopup`
//! (`nvda/source/gui/__init__.py`).
//!
//! A menu popped from a window that is not in the foreground gets neither the
//! foreground nor keyboard focus. That breaks Verbatim twice over: Windows
//! raises no foreground-change event, so our own supervisor never spins up an
//! outpost for our process and the menu is never read; and the menu cannot be
//! driven from the keyboard. NVDA solves this by making its main frame visible
//! and foreground around every popup, then restoring.
//!
//! wxdragon binds no `SetForegroundWindow`, so we reach the native `HWND`
//! through `WxWidget::get_handle` and call Win32 directly.

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SW_SHOW, SetForegroundWindow,
    ShowWindow,
};

/// The native window handle behind a wxWidgets window, if it has one.
pub(crate) fn hwnd_of(handle: *mut c_void) -> Option<HWND> {
    if handle.is_null() {
        None
    } else {
        Some(HWND(handle))
    }
}

/// Brings `hwnd` to the foreground, working around Windows' foreground lock.
///
/// `SetForegroundWindow` only succeeds for a process that already owns the
/// foreground or the last input event. Verbatim usually qualifies (the popup
/// follows a keypress our hook just saw), so the direct call is tried first.
/// When it fails, we borrow the right the standard way: attaching our input
/// queue to the current foreground thread's makes Windows treat the two as one
/// for foreground purposes, so the call is then permitted. The attachment is
/// undone immediately — leaving it in place would tie our input state to
/// another process.
pub(crate) fn force_foreground(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = BringWindowToTop(hwnd);
        if SetForegroundWindow(hwnd).as_bool() {
            return;
        }

        let foreground = GetForegroundWindow();
        if foreground.is_invalid() {
            return;
        }
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let our_thread = GetCurrentThreadId();
        if foreground_thread == 0 || foreground_thread == our_thread {
            return;
        }

        if AttachThreadInput(our_thread, foreground_thread, true).as_bool() {
            let _ = SetForegroundWindow(hwnd);
            let _ = BringWindowToTop(hwnd);
            let _ = AttachThreadInput(our_thread, foreground_thread, false);
        } else {
            tracing::warn!("could not attach to the foreground thread; popup may not be readable");
        }
    }
}
