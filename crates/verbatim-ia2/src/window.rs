//! Safe wrappers over the Win32 window functions and window messages the
//! MSAA client uses, with window handles as `isize` (zero for none).
//!
//! The local window functions (`IsWindow`, `GetClassNameW`, `GetAncestor`,
//! `GetWindow`, and the rest) tolerate any handle value, failing on one that
//! names no window, and are not counted (`docs/performance.md`, "What counts
//! as a call"). The list view and tree view messages are sent with plain
//! `SendMessageW`, which blocks while the owning application is wedged,
//! acceptable only because every caller runs on the outpost's
//! deadline-guarded worker; each counts as one window message. Only messages
//! whose parameters are plain integers are offered, never one that carries a
//! pointer, so sending them cannot make a window procedure in this process
//! read memory it does not own. A tree view's `HTREEITEM` is such an
//! integer here, but the control treats it as a pointer into its own
//! process, so only item handles the control itself produced are sent
//! back to it.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    CCM_GETVERSION, LVM_GETITEMCOUNT, TVM_GETNEXTITEM, TVM_MAPACCIDTOHTREEITEM,
    TVM_MAPHTREEITEMTOACCID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_PARENT, GET_WINDOW_CMD, GUITHREADINFO, GetAncestor, GetClassNameW, GetDesktopWindow,
    GetGUIThreadInfo, GetTopWindow, GetWindow, GetWindowThreadProcessId, IsChild, IsWindow,
    IsWindowVisible, SendMessageW,
};

use verbatim_model::CallKind;

use crate::calls::count;

/// The `HWND` for a handle value.
fn handle(hwnd: isize) -> HWND {
    HWND(hwnd as *mut c_void)
}

/// Whether `hwnd` still names a window.
pub(crate) fn exists(hwnd: isize) -> bool {
    // SAFETY: IsWindow tolerates any handle.
    unsafe { IsWindow(Some(handle(hwnd))) }.as_bool()
}

/// Whether `hwnd` is a visible window; false for one that names no window.
pub(crate) fn is_visible(hwnd: isize) -> bool {
    // SAFETY: IsWindowVisible tolerates any handle.
    unsafe { IsWindowVisible(handle(hwnd)) }.as_bool()
}

/// Whether `child` is a window inside `parent`.
pub(crate) fn is_child(parent: isize, child: isize) -> bool {
    // SAFETY: IsChild tolerates any handles.
    unsafe { IsChild(handle(parent), handle(child)) }.as_bool()
}

/// The desktop window.
pub(crate) fn desktop() -> isize {
    // SAFETY: GetDesktopWindow has no preconditions.
    unsafe { GetDesktopWindow() }.0 as isize
}

/// The window's class name, empty when it has none or names no window.
pub(crate) fn class_name(hwnd: isize) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: GetClassNameW writes at most the local buffer's length and
    // returns the count written, zero for an invalid handle.
    let len = unsafe { GetClassNameW(handle(hwnd), &mut buffer) };
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    String::from_utf16_lossy(&buffer[..len.min(buffer.len())])
}

/// The window's parent (`GA_PARENT`), zero for none.
pub(crate) fn parent(hwnd: isize) -> isize {
    // SAFETY: GetAncestor tolerates any handle.
    unsafe { GetAncestor(handle(hwnd), GA_PARENT) }.0 as isize
}

/// The window's first child window in z-order (`GetTopWindow`), zero for
/// none.
pub(crate) fn top_child(hwnd: isize) -> isize {
    // SAFETY: GetTopWindow tolerates any handle.
    unsafe { GetTopWindow(Some(handle(hwnd))) }
        .unwrap_or_default()
        .0 as isize
}

/// The window related to `hwnd` by `command` (`GetWindow`), zero for none.
pub(crate) fn related(hwnd: isize, command: GET_WINDOW_CMD) -> isize {
    // SAFETY: GetWindow tolerates any handle, failing at an edge.
    unsafe { GetWindow(handle(hwnd), command) }
        .unwrap_or_default()
        .0 as isize
}

/// The window with the keyboard focus, or else the active window, of the
/// thread in the foreground (`GetGUIThreadInfo`), and the process that owns
/// it; `None` when there is neither.
pub(crate) fn focused() -> Option<(isize, u32)> {
    let mut info = GUITHREADINFO {
        cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: `info` is a local with `cbSize` set, which the call fills in.
    unsafe { GetGUIThreadInfo(0, &raw mut info) }.ok()?;
    let hwnd = if info.hwndFocus.0.is_null() {
        info.hwndActive
    } else {
        info.hwndFocus
    };
    if hwnd.0.is_null() {
        return None;
    }
    let mut pid = 0u32;
    // SAFETY: a window handle the call tolerates even if stale, and a local
    // out-parameter.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    Some((hwnd.0 as isize, pid))
}

/// Sends `msg` with integer parameters, counted as one window message.
///
/// # Safety
///
/// `msg` must take plain integers in both parameters, never a pointer.
unsafe fn send(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    count(CallKind::WindowMessage);
    // SAFETY: SendMessageW tolerates any handle, answering zero for an
    // invalid one; the caller's contract rules out pointer parameters.
    unsafe {
        SendMessageW(
            handle(hwnd),
            msg,
            Some(WPARAM(wparam)),
            Some(LPARAM(lparam)),
        )
    }
    .0
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
