//! A real Win32 edit control for the MSAA backend (milestone M4): the
//! first fixture node with `text` becomes a multi-line `EDIT` child window
//! of the host window, holding that text, so a client reads it through the
//! edit control's own window messages, as Verbatim reads the edit fields of
//! applications it otherwise reads through MSAA. MSAA has no text
//! interface, so the scripted `IAccessible` tree cannot serve text itself.
//!
//! Line breaks in the fixture's text become the control's carriage return
//! and line feed pairs.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Accessibility::NotifyWinEvent;
use windows::Win32::UI::Controls::{EM_REPLACESEL, EM_SETSEL};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, ES_AUTOVSCROLL, ES_MULTILINE, EVENT_OBJECT_VALUECHANGE,
    FindWindowExW, GWLP_WNDPROC, HMENU, InSendMessage, OBJID_CLIENT, SendMessageW,
    SetWindowLongPtrW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_GETTEXT, WM_GETTEXTLENGTH, WM_SETTEXT,
    WNDPROC, WS_CHILD, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

thread_local! {
    /// The edit control's own window procedure, once [`write_on_read`]
    /// has put [`read_watching_proc`] in front of it.
    static EDIT_PROC: RefCell<Option<isize>> = const { RefCell::new(None) };
    /// The text [`write_on_read`] waits to type, as another thread next
    /// reads the control's text.
    static WRITE_ON_READ: RefCell<Option<String>> = const { RefCell::new(None) };
}

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
    let contents = contents(text);
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

/// `text` (UTF-16, with bare line feeds) as the control holds it, each line
/// feed after a carriage return, NUL-terminated.
fn contents(text: &[u16]) -> Vec<u16> {
    let mut contents = Vec::with_capacity(text.len() + 1);
    for &unit in text {
        if unit == u16::from(b'\n') {
            contents.push(u16::from(b'\r'));
        }
        contents.push(unit);
    }
    contents.push(0);
    contents
}

/// Replaces the edit control's text with `text`, line feeds becoming
/// carriage return and line feed pairs (`WM_SETTEXT`), on the thread that
/// owns it.
pub(crate) fn set_text(edit: HWND, text: &str) {
    let text: Vec<u16> = text.encode_utf16().collect();
    let contents = contents(&text);
    // SAFETY: `contents` is a live, NUL-terminated buffer for the duration
    // of the call; `edit` is this thread's own control.
    unsafe {
        SendMessageW(
            edit,
            WM_SETTEXT,
            None,
            Some(LPARAM(contents.as_ptr() as isize)),
        );
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

/// Types `text` over the edit control's selection the next time another
/// thread reads its text or selection (`write-on-read`), before the read
/// is answered, and raises the control's value change: a typed character
/// the application handles while a client's read of the caret event
/// before it is on its way, the value change raised during that read.
pub(crate) fn write_on_read(edit: HWND, text: &str) {
    WRITE_ON_READ.with_borrow_mut(|waiting| *waiting = Some(text.to_owned()));
    EDIT_PROC.with_borrow_mut(|original| {
        if original.is_none() {
            // SAFETY: `edit` is this thread's own control, and
            // `read_watching_proc` passes every message on to the
            // procedure it replaces.
            *original = Some(unsafe {
                SetWindowLongPtrW(edit, GWLP_WNDPROC, read_watching_proc as *const () as isize)
            });
        }
    });
}

/// The edit control's window procedure once [`write_on_read`] is used:
/// types the waiting text at a read sent from another thread, then passes
/// every message on to the control's own procedure.
///
/// # Safety
///
/// Installed only as the edit control's window procedure by
/// [`write_on_read`].
unsafe extern "system" fn read_watching_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // The control's text and selection messages, `EM_GETSEL` to
    // `EM_GETIMESTATUS`, and its text.
    let is_read = matches!(msg, WM_GETTEXT | WM_GETTEXTLENGTH | 0x00B0..=0x00D9);
    if is_read
        // SAFETY: InSendMessage has no preconditions.
        && unsafe { InSendMessage() }.as_bool()
        && let Some(text) = WRITE_ON_READ.with_borrow_mut(Option::take)
    {
        let typed: Vec<u16> = text.encode_utf16().collect();
        let contents = contents(&typed);
        // SAFETY: `contents` is a live, NUL-terminated buffer for the
        // duration of the call; `hwnd` is this thread's own control.
        unsafe {
            SendMessageW(
                hwnd,
                EM_REPLACESEL,
                Some(WPARAM(0)),
                Some(LPARAM(contents.as_ptr() as isize)),
            );
        }
        // SAFETY: `hwnd` is this thread's own control, which answers for
        // its client object.
        unsafe { NotifyWinEvent(EVENT_OBJECT_VALUECHANGE, hwnd, OBJID_CLIENT.0, 0) };
    }
    let original = EDIT_PROC.with_borrow(|original| original.unwrap_or_default());
    // SAFETY: `original` is the control's own procedure, stored before this
    // one was installed.
    let procedure: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(original) };
    // SAFETY: passes the message on to the procedure this one replaced.
    unsafe { CallWindowProcW(procedure, hwnd, msg, wparam, lparam) }
}
