//! Real Win32 buttons for the MSAA backend: a fixture node with
//! `"native": "group_box"` becomes a standard group box (a `Button` window
//! with `BS_GROUPBOX`) with its `button` children as push buttons inside its
//! rectangle, and one with `"native": "button"` a push button outside any
//! group box. In a Win32 dialog a group box is the sibling of the controls
//! inside it, created before them and, as a dialog places each control
//! below the last, the window before them in z-order, which is how a client
//! finds it (NVDA's `findGroupboxObject`). Each button here is placed
//! below the last the same way.
//! Their accessible objects are the system's own button proxies.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    BS_GROUPBOX, BS_PUSHBUTTON, CreateWindowExW, HMENU, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetWindowPos, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CHILD, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use crate::fixture::FixtureNode;

/// Where the group box sits in the host window, and how big it is.
const GROUP: (i32, i32, i32, i32) = (10, 10, 300, 200);

/// Creates `node`'s group box, and its buttons inside it, in `parent`.
///
/// # Errors
///
/// A window creation's error.
pub(crate) fn create_group_box(parent: HWND, node: &FixtureNode) -> windows::core::Result<()> {
    let (left, top, width, height) = GROUP;
    create(
        parent,
        node.name.as_deref().unwrap_or_default(),
        BS_GROUPBOX,
        (left, top, width, height),
    )?;
    for (index, child) in node.children.iter().enumerate() {
        let offset = i32::try_from(index).unwrap_or(0) * 40;
        create(
            parent,
            child.name.as_deref().unwrap_or_default(),
            BS_PUSHBUTTON,
            (left + 20, top + 30 + offset, 120, 30),
        )?;
    }
    Ok(())
}

/// Creates `node`'s push button in `parent`, to the right of the group box.
///
/// # Errors
///
/// The window creation's error.
pub(crate) fn create_button(parent: HWND, node: &FixtureNode) -> windows::core::Result<()> {
    let (left, top, width, _) = GROUP;
    create(
        parent,
        node.name.as_deref().unwrap_or_default(),
        BS_PUSHBUTTON,
        (left + width + 20, top + 30, 120, 30),
    )
}

/// Creates a `Button` window of `style` titled `text` at `rect` in `parent`.
fn create(
    parent: HWND,
    text: &str,
    style: i32,
    (left, top, width, height): (i32, i32, i32, i32),
) -> windows::core::Result<()> {
    let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let style = WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style.cast_unsigned());
    // SAFETY: `Button` is a system class; `text` is a live, NUL-terminated
    // buffer for the call; `parent` is the host window, created on this
    // thread.
    let button = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("Button"),
            PCWSTR(text.as_ptr()),
            style,
            left,
            top,
            width,
            height,
            Some(parent),
            None::<HMENU>,
            None,
            None,
        )
    }?;
    // SAFETY: the button just created on this thread, moved only in
    // z-order.
    unsafe {
        SetWindowPos(
            button,
            Some(HWND_BOTTOM),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    }
}
