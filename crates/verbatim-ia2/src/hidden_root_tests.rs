//! The hidden-dialog check of a focus read (Dickson, 2026-11-19), the
//! `Test` of a top-level window's own object that Core asks first, run by
//! the MSAA executor's window arm ([`crate::batch`]): a window whose
//! top-level window is a hidden dialog matches; a shown root, or a hidden
//! one that is no dialog, does not, as File
//! Explorer's hidden Static before Run was recorded; and a hidden root
//! whose client role cannot be read answers its role absent, never "not a
//! dialog", which Core judges not there. The roles are the ones oleacc's
//! own proxies give these classes.

use verbatim_model::Role;
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyId, PropertyValue};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, SW_SHOWNA, ShowWindow, WINDOW_EX_STYLE, WS_CHILD,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use crate::batch::window_property;

/// The hidden-dialog check as Core asks it (`verbatim-core`'s
/// `focus::shown::check`).
fn check() -> Matcher {
    Matcher::All(vec![
        Matcher::Is(PropertyId::WindowShown, PropertyValue::Flag(false)),
        Matcher::IsAnyOf(
            PropertyId::Role,
            [Role::Dialog, Role::Alert, Role::PropertyPage]
                .into_iter()
                .map(PropertyValue::Role)
                .collect(),
        ),
    ])
}

/// The check of the top-level window of `window`, its client role read by
/// `client_role`: whether it matched, and what it read, in order.
fn checked(
    window: isize,
    client_role: impl Fn(isize) -> Option<Role>,
) -> (bool, Vec<(PropertyId, Option<PropertyValue>)>) {
    let top = verbatim_win32::window::top_level_of(window);
    let mut read = Vec::new();
    let Ok(holds) = check().evaluate::<std::convert::Infallible>(&mut |id| {
        let value = window_property(top, id, &client_role)
            .expect("the check reads the window's own properties");
        read.push((id, value.clone()));
        Ok(value)
    });
    (holds, read)
}

/// [`checked`], the role read as the outpost reads it.
fn hidden_root(window: isize) -> bool {
    checked(window, crate::acquire::client_role).0
}

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

/// A field inside a hidden dialog matches: its top-level window is a
/// hidden dialog.
#[test]
fn a_field_in_a_hidden_dialog_matches() {
    let dialog = top_level(w!("#32770"));
    let field = field_in(dialog);
    assert_eq!(
        checked(raw(field), crate::acquire::client_role),
        (
            true,
            vec![
                (PropertyId::WindowShown, Some(PropertyValue::Flag(false))),
                (PropertyId::Role, Some(PropertyValue::Role(Role::Dialog))),
            ]
        )
    );
    destroy(dialog);
}

/// Once shown, the same dialog does not match, its role not read.
#[test]
fn a_shown_dialog_does_not_match() {
    let dialog = top_level(w!("#32770"));
    let field = field_in(dialog);
    // SAFETY: a live window of this thread's.
    let _ = unsafe { ShowWindow(dialog, SW_SHOWNA) };
    assert_eq!(
        checked(raw(field), crate::acquire::client_role),
        (
            false,
            vec![(PropertyId::WindowShown, Some(PropertyValue::Flag(true)))]
        )
    );
    destroy(dialog);
}

/// Hidden roots that are no dialog, a Static as File Explorer's before Run
/// was recorded, and an Edit: neither matches.
#[test]
fn a_hidden_root_that_is_no_dialog_does_not_match() {
    for class in [w!("Static"), w!("Edit")] {
        let root = top_level(class);
        assert!(!hidden_root(raw(root)));
        destroy(root);
    }
}

/// A hidden root whose client role cannot be read answers it absent, which
/// Core judges not there; a role that is no dialog's does not match; an
/// alert's does.
#[test]
fn a_hidden_roots_role_unread_is_answered_absent() {
    let dialog = top_level(w!("#32770"));
    assert_eq!(
        checked(raw(dialog), |_| None),
        (
            false,
            vec![
                (PropertyId::WindowShown, Some(PropertyValue::Flag(false))),
                (PropertyId::Role, None),
            ]
        )
    );
    assert!(!checked(raw(dialog), |_| Some(Role::Window)).0);
    assert!(checked(raw(dialog), |_| Some(Role::Alert)).0);
    destroy(dialog);
}

/// The check's cost: a shown root costs no call into its application, its
/// root and visibility read locally; a hidden one costs the client
/// object's acquisition and its role, two MSAA calls.
#[test]
fn the_check_costs_nothing_for_a_shown_root_and_two_calls_for_a_hidden_one() {
    let dialog = top_level(w!("#32770"));
    let field = field_in(dialog);
    let _ = verbatim_win32::calls::take();
    let _ = hidden_root(raw(field));
    assert_eq!(verbatim_win32::calls::take().msaa, 2);
    // SAFETY: a live window of this thread's.
    let _ = unsafe { ShowWindow(dialog, SW_SHOWNA) };
    let _ = hidden_root(raw(field));
    assert_eq!(
        verbatim_win32::calls::take(),
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
/// every tray menu's opening as recorded: no dialog, so it does not match.
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
    assert!(!hidden_root(raw(frame)));
    destroy(frame);
}
