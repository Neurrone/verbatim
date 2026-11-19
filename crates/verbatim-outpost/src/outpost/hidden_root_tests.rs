//! The hidden-dialog check of a focus read (Dickson, 2026-11-19): a read in
//! a window whose top-level window is a hidden dialog is answered with
//! that window; a shown root, or a hidden one that is no dialog, is read
//! as it is, as File Explorer's hidden Static before Run was recorded; and
//! a hidden
//! root whose client role cannot be read is not there, never "not a
//! dialog". The roles are the ones oleacc's own proxies give these
//! classes.

use verbatim_model::{Pid, Role, WindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, SW_SHOWNA, ShowWindow, WINDOW_EX_STYLE, WS_CHILD,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use super::asked::{Root, hidden_root, hidden_root_read};

/// A popup of `class`, of this thread's, with no size and never taking the
/// foreground, hidden; destroyed by the caller on this thread.
fn top_level(class: PCWSTR) -> HWND {
    // SAFETY: a window with no parent, made on this thread.
    unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class,
            w!(""),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
        )
    }
    .expect("a top-level window")
}

/// A visible Edit inside `parent`.
fn field_in(parent: HWND) -> HWND {
    // SAFETY: a child of a live window of this thread's.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("Edit"),
            w!(""),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            0,
            0,
            Some(parent),
            None,
            None,
            None,
        )
    }
    .expect("a field")
}

fn destroy(window: HWND) {
    // SAFETY: a window this thread made, destroyed once.
    unsafe { DestroyWindow(window) }.expect("the window destroyed");
}

fn raw(window: HWND) -> isize {
    window.0 as isize
}

/// A field inside a hidden dialog is answered with the dialog's window.
#[test]
fn a_field_in_a_hidden_dialog_answers_the_dialogs_window() {
    let dialog = top_level(w!("#32770"));
    let field = field_in(dialog);
    let Root::HiddenDialog(window) = hidden_root(raw(field)) else {
        panic!("a hidden dialog");
    };
    assert_eq!(
        (window.handle, window.top_level, window.process),
        (
            WindowHandle(u64::try_from(raw(dialog)).expect("a handle")),
            WindowHandle(u64::try_from(raw(dialog)).expect("a handle")),
            Pid(std::process::id())
        )
    );
    destroy(dialog);
}

/// Once shown, the same dialog is read as it is.
#[test]
fn a_shown_dialog_is_read_as_it_is() {
    let dialog = top_level(w!("#32770"));
    let field = field_in(dialog);
    // SAFETY: a live window of this thread's.
    let _ = unsafe { ShowWindow(dialog, SW_SHOWNA) };
    assert_eq!(hidden_root(raw(field)), Root::Read);
    destroy(dialog);
}

/// Hidden roots that are no dialog, a Static as File Explorer's before Run
/// was recorded, and an Edit: each is read as it is.
#[test]
fn a_hidden_root_that_is_no_dialog_is_read_as_it_is() {
    for class in [w!("Static"), w!("Edit")] {
        let root = top_level(class);
        assert_eq!(hidden_root(raw(root)), Root::Read);
        destroy(root);
    }
}

/// A hidden root whose client role cannot be read is not there, as a failed
/// acquire is answered; a role that is no dialog's is read, never failed.
#[test]
fn a_hidden_root_whose_role_fails_is_not_there() {
    let dialog = top_level(w!("#32770"));
    assert_eq!(hidden_root_read(raw(dialog), |_| None), Root::NotThere);
    assert_eq!(
        hidden_root_read(raw(dialog), |_| Some(Role::Window)),
        Root::Read
    );
    assert!(matches!(
        hidden_root_read(raw(dialog), |_| Some(Role::Alert)),
        Root::HiddenDialog(_)
    ));
    destroy(dialog);
}

/// The check's cost: a shown root costs no call into its application, its
/// root and visibility read locally; a hidden one costs the client
/// object's acquisition and its role, two MSAA calls.
#[test]
fn the_check_costs_nothing_for_a_shown_root_and_two_calls_for_a_hidden_one() {
    let dialog = top_level(w!("#32770"));
    let field = field_in(dialog);
    let _ = verbatim_ia2::calls::take();
    let _ = hidden_root(raw(field));
    assert_eq!(verbatim_ia2::calls::take().msaa, 2);
    // SAFETY: a live window of this thread's.
    let _ = unsafe { ShowWindow(dialog, SW_SHOWNA) };
    let _ = hidden_root(raw(field));
    assert_eq!(
        verbatim_ia2::calls::take(),
        verbatim_model::CallCounts {
            uia: 0,
            msaa: 0,
            window_messages: 0,
            connections: 0,
        }
    );
    destroy(dialog);
}

/// Verbatim's own hidden tray frame, wxWidgets' `wxWindowNR`, in front at
/// every tray menu's opening as recorded: no dialog, so read as it is.
#[test]
fn verbatims_hidden_tray_frame_is_read_as_it_is() {
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, RegisterClassW, WNDCLASSW};

    unsafe extern "system" fn procedure(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: the default handling of this class's messages.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }
    // SAFETY: this process's own module handle.
    let instance = unsafe { GetModuleHandleW(None) }.expect("the module");
    let class = WNDCLASSW {
        lpfnWndProc: Some(procedure),
        hInstance: instance.into(),
        lpszClassName: w!("wxWindowNR"),
        ..Default::default()
    };
    // SAFETY: a class of static data and a procedure of this module,
    // registered again harmlessly by a rerun in the same process.
    let _ = unsafe { RegisterClassW(&raw const class) };
    let frame = top_level(w!("wxWindowNR"));
    assert_eq!(hidden_root(raw(frame)), Root::Read);
    destroy(frame);
}
