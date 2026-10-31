//! Safe wrappers over the window messages the MSAA client sends, with
//! window handles as `isize` (zero for none), beside the local window
//! functions it shares with the other roles (`verbatim_win32::window`),
//! re-exported here.
//!
//! The list view and tree view messages are sent with
//! `SendMessageTimeoutW`, giving up after [`MESSAGE_TIMEOUT_MS`] or at once
//! when the owning application is hung, as the list view's and the edit
//! control's own messages are (`docs/design/focus-pipeline.md`, section
//! 6.3); each answers zero when it fails, and counts as one window message.
//! Only messages
//! whose parameters are plain integers are offered, never one that carries a
//! pointer, so sending them cannot make a window procedure in this process
//! read memory it does not own. A tree view's `HTREEITEM` is such an
//! integer here, but the control treats it as a pointer into its own
//! process, so only item handles the control itself produced are sent
//! back to it.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    CCM_GETVERSION, LVM_GETITEMCOUNT, TVIS_STATEIMAGEMASK, TVM_GETITEMSTATE, TVM_GETNEXTITEM,
    TVM_MAPACCIDTOHTREEITEM, TVM_MAPHTREEITEMTOACCID,
};
use windows::Win32::UI::WindowsAndMessaging::{SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW};

use verbatim_model::CallKind;

use crate::calls::count;

pub(crate) use verbatim_win32::window::{
    desktop_window, is_child, is_enabled, is_multiline_edit, parent, rect, related, style,
    top_child, window_class_name, window_exists, window_is_visible, window_thread,
};

/// The `HWND` for a handle value.
fn handle(hwnd: isize) -> HWND {
    HWND(hwnd as *mut c_void)
}

/// How long a message waits for the window to answer: the list view's and
/// the edit control's bound.
const MESSAGE_TIMEOUT_MS: u32 = 500;

/// Sends `msg` with integer parameters, counted as one window message,
/// answering zero when the window does not answer within
/// [`MESSAGE_TIMEOUT_MS`], is hung, or is gone.
///
/// # Safety
///
/// `msg` must take plain integers in both parameters, never a pointer.
unsafe fn send(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    count(CallKind::WindowMessage);
    let mut result = 0usize;
    // SAFETY: SendMessageTimeoutW tolerates any handle, failing for an
    // invalid one; the caller's contract rules out pointer parameters, and
    // the result is a local.
    let sent = unsafe {
        SendMessageTimeoutW(
            handle(hwnd),
            msg,
            WPARAM(wparam),
            LPARAM(lparam),
            SMTO_ABORTIFHUNG | SMTO_BLOCK,
            MESSAGE_TIMEOUT_MS,
            Some(&raw mut result),
        )
    };
    if sent.0 == 0 {
        return 0;
    }
    result.cast_signed()
}

/// A list view's item count (`LVM_GETITEMCOUNT`), zero on failure.
pub(crate) fn list_view_item_count(hwnd: isize) -> isize {
    // SAFETY: LVM_GETITEMCOUNT takes no parameters.
    unsafe { send(hwnd, LVM_GETITEMCOUNT, 0, 0) }
}

/// Whether a common control is comctl32 version 6 or later
/// (`CCM_GETVERSION`), whose tree view maps MSAA child ids to items.
pub(crate) fn is_common_control_6(hwnd: isize) -> bool {
    // SAFETY: CCM_GETVERSION takes no parameters.
    unsafe { send(hwnd, CCM_GETVERSION, 0, 0) >= 6 }
}

/// A tree view item's `HTREEITEM` for an MSAA child id
/// (`TVM_MAPACCIDTOHTREEITEM`), zero when the control does not map it.
pub(crate) fn tree_view_item_for_acc_id(hwnd: isize, acc_id: usize) -> isize {
    // SAFETY: TVM_MAPACCIDTOHTREEITEM takes the child id as an integer.
    unsafe { send(hwnd, TVM_MAPACCIDTOHTREEITEM, acc_id, 0) }
}

/// A tree view item's MSAA child id for an `HTREEITEM`
/// (`TVM_MAPHTREEITEMTOACCID`), zero when the control does not map it.
pub(crate) fn tree_view_acc_id_for_item(hwnd: isize, item: usize) -> isize {
    // SAFETY: TVM_MAPHTREEITEMTOACCID takes the item handle as an integer;
    // nothing in this process is read. The control dereferences the handle
    // in its own process, so callers pass only handles it produced.
    unsafe { send(hwnd, TVM_MAPHTREEITEMTOACCID, item, 0) }
}

/// The item related to `item` by the `TVGN_` code `relation`
/// (`TVM_GETNEXTITEM`), zero for none.
pub(crate) fn tree_view_next_item(hwnd: isize, relation: u32, item: isize) -> isize {
    // SAFETY: TVM_GETNEXTITEM takes a relation code and an item handle as
    // integers; nothing in this process is read. The control dereferences
    // the handle in its own process, so callers pass only handles it
    // produced.
    unsafe { send(hwnd, TVM_GETNEXTITEM, relation as usize, item) }
}

/// The index of a tree view item's state image (`TVM_GETITEMSTATE` with
/// `TVIS_STATEIMAGEMASK`), 0 for none: the bits above the twelfth, of
/// which NVDA looks at two.
pub(crate) fn tree_view_state_image(hwnd: isize, item: isize) -> u32 {
    // SAFETY: TVM_GETITEMSTATE takes an item handle and a mask as integers;
    // nothing in this process is read. The control dereferences the handle
    // in its own process, so callers pass only handles it produced.
    let state = unsafe {
        send(
            hwnd,
            TVM_GETITEMSTATE,
            item.cast_unsigned(),
            isize::try_from(TVIS_STATEIMAGEMASK.0).unwrap_or(0),
        )
    };
    u32::try_from((state >> 12) & 3).unwrap_or(0)
}
