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
use std::mem::size_of;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VK_CONTROL,
};
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

/// Injects a bare `VK_CONTROL` down-then-up tap, satisfying Windows'
/// foreground-lock heuristic (which cares only that *some* key event was
/// just seen from this process, not what it was) before
/// [`force_foreground`] attempts `SetForegroundWindow`.
///
/// A gesture that arrived via the control plane — every E2E test, and any
/// future remote-support session — has no physical input behind it, so
/// without this nudge `SetForegroundWindow` fails the heuristic outright and
/// falls back to the slow `AttachThreadInput` path below, observed live at
/// roughly two seconds; the tap cuts that to roughly 150 to 450 ms.
/// Deliberately `VK_CONTROL`, not `VK_MENU`: a lone Alt press activates menu
/// bars and bounces foreground straight back, also confirmed live.
fn nudge_foreground_lock() {
    let down = KEYBDINPUT {
        wVk: VK_CONTROL,
        dwFlags: KEYBD_EVENT_FLAGS(0),
        ..Default::default()
    };
    let up = KEYBDINPUT {
        wVk: VK_CONTROL,
        dwFlags: KEYEVENTF_KEYUP,
        ..Default::default()
    };
    let inputs = [
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: down },
        },
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: up },
        },
    ];
    // SAFETY: `inputs` is a fully initialized, correctly sized array of
    // INPUT structures; SendInput copies from it and does not retain a
    // reference afterward.
    unsafe {
        SendInput(&inputs, i32::try_from(size_of::<INPUT>()).unwrap_or(0));
    }
}

/// Brings `hwnd` to the foreground, working around Windows' foreground lock.
///
/// `SetForegroundWindow` only succeeds for a process that already owns the
/// foreground or the last input event. A control-plane-originated gesture has
/// no physical input behind it, so [`nudge_foreground_lock`] injects one
/// first; the direct call is then tried. When it still fails, we borrow the
/// right the standard way: attaching our input queue to the current
/// foreground thread's makes Windows treat the two as one for foreground
/// purposes, so the call is then permitted. The attachment is undone
/// immediately — leaving it in place would tie our input state to another
/// process.
pub(crate) fn force_foreground(hwnd: HWND) {
    nudge_foreground_lock();
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
