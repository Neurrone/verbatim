//! A real Win32 edit control for the MSAA backend (milestone M4): the
//! first fixture node with `text` becomes a multi-line `EDIT` child window
//! of the host window, holding that text, so a client reads it through the
//! edit control's own window messages, as Verbatim reads the edit fields of
//! applications it otherwise reads through MSAA. MSAA has no text
//! interface, so the scripted `IAccessible` tree cannot serve text itself.
//!
//! Line breaks in the fixture's text become the control's carriage return
//! and line feed pairs.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, ES_AUTOVSCROLL, ES_MULTILINE, FindWindowExW, HMENU, SendMessageW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WS_CHILD, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

/// The edit control's window class.
const EDIT_CLASS: PCWSTR = w!("EDIT");

/// Creates the edit control inside `parent`, holding `text` (UTF-16, with
/// bare line feeds): the classic one, or with `version_6` Common Controls
/// version 6's, as a Windows Forms text box is. Both are of the class
/// `Edit` and answer the same messages, but not always alike: past the last
/// line, `EM_LINEINDEX` answers -1 from the classic control sign-extended
/// into the result and from version 6's zero-extended.
///
/// # Errors
///
/// The window creation's error, or the activation context's.
pub(crate) fn create(parent: HWND, text: &[u16], version_6: bool) -> windows::core::Result<HWND> {
    let _common_controls = if version_6 {
        Some(crate::common_controls::CommonControls6::activate()?)
    } else {
        None
    };
    let mut contents = Vec::with_capacity(text.len() + 1);
    for &unit in text {
        if unit == u16::from(b'\n') {
            contents.push(u16::from(b'\r'));
        }
        contents.push(unit);
    }
    contents.push(0);
    let style = WINDOW_STYLE(
        WS_CHILD.0
            | WS_VISIBLE.0
            | WS_VSCROLL.0
            | ES_MULTILINE.cast_unsigned()
            | ES_AUTOVSCROLL.cast_unsigned(),
    );
    // SAFETY: `EDIT` is a system class; `contents` is a live, NUL-terminated
    // buffer for the duration of the call; `parent` is the host window,
    // created on this thread.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            EDIT_CLASS,
            PCWSTR(contents.as_ptr()),
            style,
            0,
            0,
            400,
            200,
            Some(parent),
            None::<HMENU>,
            None,
            None,
        )
    }
}

/// The edit control inside `parent`, if one was created.
pub(crate) fn find(parent: HWND) -> Option<HWND> {
    // SAFETY: a local search of `parent`'s children by class.
    unsafe { FindWindowExW(Some(parent), None, EDIT_CLASS, PCWSTR::null()) }.ok()
}

/// Selects from `start` to `end` in the edit control (`EM_SETSEL`), on the
/// thread that owns it.
pub(crate) fn select(edit: HWND, start: usize, end: usize) {
    // SAFETY: EM_SETSEL takes two integers; `edit` is this thread's own
    // control.
    unsafe {
        SendMessageW(
            edit,
            EM_SETSEL,
            Some(WPARAM(start)),
            Some(LPARAM(isize::try_from(end).unwrap_or(isize::MAX))),
        );
    }
}
