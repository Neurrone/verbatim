//! A real Win32 edit control for the MSAA backend (milestone M4): the
//! first fixture node with `text` becomes an `EDIT` child window ([`EditKind`])
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
use windows::Win32::UI::Controls::EM_SETSEL;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_LEFT;
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, ES_AUTOVSCROLL, ES_MULTILINE, EVENT_OBJECT_FOCUS,
    FindWindowExW, GWLP_WNDPROC, HMENU, InSendMessage, OBJID_CLIENT, SendMessageW,
    SetWindowLongPtrW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_GETTEXT, WM_GETTEXTLENGTH, WM_KEYDOWN,
    WM_SETTEXT, WNDPROC, WS_CHILD, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

thread_local! {
    /// The edit control's own window procedure, once
    /// [`stale_focus_on_left`] has put [`read_watching_proc`] in front of
    /// it.
    static EDIT_PROC: RefCell<Option<isize>> = const { RefCell::new(None) };
    /// The stale focus [`stale_focus_on_left`] raises after each Left
    /// Arrow: the window and its child id, and whether a Left Arrow armed
    /// it.
    static STALE_FOCUS: RefCell<Option<(isize, i32, bool)>> = const { RefCell::new(None) };
}

/// The edit control's window class.
const EDIT_CLASS: PCWSTR = w!("EDIT");

/// Which edit control holds a fixture's text on the MSAA backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditKind {
    /// The classic multi-line edit control.
    Classic,
    /// Common Controls version 6's multi-line edit control, as a Windows
    /// Forms text box is. Both are of the class `Edit` and answer the same
    /// messages, but not always alike: past the last line, `EM_LINEINDEX`
    /// answers -1 from the classic control sign-extended into the result
    /// and from version 6's zero-extended.
    Version6,
    /// The stand-in for the read-only Location field of a file's
    /// Properties (`read_only_field_typing`): the classic
    /// single-line edit control with that field's style, `ES_READONLY` and
    /// `ES_AUTOHSCROLL` with no `WS_TABSTOP`, so that the system's MSAA
    /// proxy answers it as editable text (role 42) with the read-only
    /// state. With `label`, a static text control holding it comes just
    /// before the field, and the proxy names the field by it, as the
    /// dialog's "Location:" label names its field.
    ReadOnly {
        /// The label's text.
        label: Option<String>,
    },
}

/// The read-only field's style, the Location field's: `WS_CHILD`,
/// `WS_VISIBLE`, `ES_READONLY` and `ES_AUTOHSCROLL`.
const READ_ONLY_STYLE: u32 = 0x5000_0880;

/// The width of a read-only field's label, which the field follows.
const LABEL_WIDTH: i32 = 100;

/// Creates the edit control `kind` inside `parent`, holding `text`
/// (UTF-16, with bare line feeds), with its label before it when it has
/// one.
///
/// # Errors
///
/// The window creation's error, or the activation context's.
pub(crate) fn create(parent: HWND, text: &[u16], kind: &EditKind) -> windows::core::Result<HWND> {
    let _common_controls = if *kind == EditKind::Version6 {
        Some(crate::common_controls::CommonControls6::activate()?)
    } else {
        None
    };
    let contents = contents(text);
    let (style, x, height) = match kind {
        EditKind::Classic | EditKind::Version6 => (
            WINDOW_STYLE(
                WS_CHILD.0
                    | WS_VISIBLE.0
                    | WS_VSCROLL.0
                    | ES_MULTILINE.cast_unsigned()
                    | ES_AUTOVSCROLL.cast_unsigned(),
            ),
            0,
            200,
        ),
        EditKind::ReadOnly { label: None } => (WINDOW_STYLE(READ_ONLY_STYLE), 0, 24),
        EditKind::ReadOnly { label: Some(label) } => {
            create_label(parent, label)?;
            (WINDOW_STYLE(READ_ONLY_STYLE), LABEL_WIDTH, 24)
        }
    };
    // SAFETY: `EDIT` is a system class; `contents` is a live, NUL-terminated
    // buffer for the duration of the call; `parent` is the host window,
    // created on this thread.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            EDIT_CLASS,
            PCWSTR(contents.as_ptr()),
            style,
            x,
            0,
            400,
            height,
            Some(parent),
            None::<HMENU>,
            None,
            None,
        )
    }
}

/// Creates a static text control holding `label` inside `parent`, which
/// the edit control created next follows.
fn create_label(parent: HWND, label: &str) -> windows::core::Result<HWND> {
    let text: Vec<u16> = label.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `STATIC` is a system class; `text` is a live, NUL-terminated
    // buffer for the duration of the call; `parent` is the host window,
    // created on this thread.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            PCWSTR(text.as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            0,
            0,
            LABEL_WIDTH,
            24,
            Some(parent),
            None::<HMENU>,
            None,
            None,
        )
    }
}

/// Gives the edit control the keyboard focus (`SetFocus`), on the thread
/// that owns it, as UI Automation's `SetFocus` focused the Location field
/// in the recording its stand-in follows.
pub(crate) fn focus(edit: HWND) {
    // SAFETY: `edit` is this thread's own live control; the window it had
    // the focus from is not needed.
    let _ = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit)) };
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

/// Raises `EVENT_OBJECT_FOCUS` on child `child` of `window`, a control that
/// does not have the focus, at the first read of the edit control's text or
/// selection sent from another thread after each Left Arrow it receives
/// (`stale-focus-on-left`): the stale focus of the Open dialog's folder
/// tree, raised while the "File name" field has the focus (survey
/// `np_open`). The read after the key is a reader's look for the key's
/// effect, so the focus comes while a caret key's watch is out, on that
/// evidence rather than a time.
pub(crate) fn stale_focus_on_left(edit: HWND, (window, child): (HWND, i32)) {
    STALE_FOCUS.with_borrow_mut(|stale| *stale = Some((window.0 as isize, child, false)));
    watch_messages(edit);
}

/// Puts [`read_watching_proc`] in front of the edit control's own
/// procedure, once.
fn watch_messages(edit: HWND) {
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

/// The edit control's window procedure once [`stale_focus_on_left`] is
/// used: at a Left Arrow arms the stale focus, and raises it at the next
/// read sent from another thread; then passes every message on to the
/// control's own procedure.
///
/// # Safety
///
/// Installed only as the edit control's window procedure by
/// [`watch_messages`].
unsafe extern "system" fn read_watching_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // The control's text and selection messages, `EM_GETSEL` to
    // `EM_GETIMESTATUS`, and its text.
    let is_read = matches!(msg, WM_GETTEXT | WM_GETTEXTLENGTH | 0x00B0..=0x00D9);
    if let Some((window, child, armed)) = STALE_FOCUS.with_borrow(|stale| *stale) {
        if msg == WM_KEYDOWN && wparam.0 == usize::from(VK_LEFT.0) {
            STALE_FOCUS.with_borrow_mut(|stale| *stale = Some((window, child, true)));
        } else if armed
            && is_read
            // SAFETY: InSendMessage has no preconditions.
            && unsafe { InSendMessage() }.as_bool()
        {
            STALE_FOCUS.with_borrow_mut(|stale| *stale = Some((window, child, false)));
            // SAFETY: the event names a live window of this thread, which
            // answers for its client.
            unsafe {
                NotifyWinEvent(
                    EVENT_OBJECT_FOCUS,
                    HWND(window as *mut _),
                    OBJID_CLIENT.0,
                    child,
                );
            }
        }
    }
    let original = EDIT_PROC.with_borrow(|original| original.unwrap_or_default());
    // SAFETY: `original` is the control's own procedure, stored before this
    // one was installed.
    let procedure: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(original) };
    // SAFETY: passes the message on to the procedure this one replaced.
    unsafe { CallWindowProcW(procedure, hwnd, msg, wparam, lparam) }
}
