//! Safe wrappers over the window messages the MSAA client sends, with
//! window handles as `isize` (zero for none), beside the local window
//! functions it shares with the other roles (`verbatim_win32::window`),
//! re-exported here.
//!
//! Every window message this crate sends, the list view's, tree view's and
//! edit control's, goes through [`send_message`]: the window's readability
//! is tested first, so a hung or ghosted window fails at once and a
//! destroyed one is gone, and the message is then sent with no time limit
//! of its own, its program's deadline and the judgement that its window is
//! hung its only bounds (`books/dev/src/reference/crates/verbatim-model.md`,
//! "Time limits"). Each counts as one window message. The integer messages
//! here answer zero when they fail. Only messages
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
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindow, SMTO_BLOCK, SMTO_ERRORONEXIT, SendMessageTimeoutW,
};

use verbatim_model::CallKind;

use crate::calls::count;

pub(crate) use verbatim_win32::window::{
    desktop_window, is_child, is_enabled, is_multiline_edit, parent, rect, related, style,
    top_child, window_class_name, window_exists, window_is_visible,
};

/// The `HWND` for a handle value.
fn handle(hwnd: isize) -> HWND {
    HWND(hwnd as *mut c_void)
}

/// Why a window message was not answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unanswered {
    /// The window is gone.
    Gone,
    /// The window is hung or ghosted: nothing was sent.
    NotResponding,
    /// The send failed for another reason.
    Failed,
    /// The worker sending it was abandoned: nothing was sent
    /// (`verbatim_win32::calls`).
    Abandoned,
}

/// The time limit a message is sent with: the largest that waits, about 24.8
/// days, so the send returns only once the window procedure has answered
/// or, with `SMTO_ERRORONEXIT`, once the window's thread has ended, the
/// one way to tell when a message was answered, and so when a buffer it
/// carries may be freed. `SendMessageTimeoutW` takes a limit above
/// `i32::MAX` milliseconds as passed at once: with `u32::MAX` and one less
/// it failed with `ERROR_TIMEOUT` before a held message was answered
/// (mockapp's `window_messages`, 2026-11-06).
const NO_TIME_LIMIT: u32 = 0x7FFF_FFFF;

/// Sends `msg` to window `hwnd`, counted as one window message, and
/// answers its result. The window's readability is tested first, by local
/// calls, and a hung or ghosted window fails at once, unsent, as a
/// destroyed one is gone. `buffer` is the address of a buffer in the
/// target's memory the message carries, if any, which the fault points
/// see. The window's process is not checked against the buffer's: a
/// window handle carries a 16-bit reuse counter, so a handle's value
/// recurs only after about 65,536 reuses of its slot, no practical risk
/// (Dickson, 2026-11-07). The message is sent with
/// `SMTO_BLOCK | SMTO_ERRORONEXIT` and no time limit
/// ([`NO_TIME_LIMIT`]), without `SMTO_ABORTIFHUNG`, so nothing relies on
/// when that flag returns; a failure is read through `IsWindow` as gone,
/// since `SMTO_ERRORONEXIT` fails a send whose window's thread ended.
///
/// # Safety
///
/// `wparam` and `lparam` must be what `msg` takes: integers, addresses in
/// the target's own memory, or pointers to memory of this process that
/// Windows marshals for `msg` and that outlives the call.
pub(crate) unsafe fn send_message(
    hwnd: isize,
    msg: u32,
    (wparam, lparam): (usize, isize),
    #[cfg_attr(not(any(test, feature = "faults")), expect(unused_variables))] buffer: Option<usize>,
) -> Result<isize, Unanswered> {
    match verbatim_win32::calls::message_readability(hwnd) {
        verbatim_win32::window::Readability::Readable => {}
        verbatim_win32::window::Readability::Gone => return Err(Unanswered::Gone),
        verbatim_win32::window::Readability::Ghosted
        | verbatim_win32::window::Readability::Hung => return Err(Unanswered::NotResponding),
    }
    count(CallKind::WindowMessage).map_err(|_| Unanswered::Abandoned)?;
    #[cfg(any(test, feature = "faults"))]
    verbatim_faults::reach(
        verbatim_faults::Fault::WindowMessageSent,
        &(hwnd, msg, None::<u32>, buffer),
    );
    let mut result = 0usize;
    // SAFETY: SendMessageTimeoutW tolerates any handle, failing for an
    // invalid one; the caller's contract covers the parameters, and the
    // result is a local.
    let sent = unsafe {
        SendMessageTimeoutW(
            handle(hwnd),
            msg,
            WPARAM(wparam),
            LPARAM(lparam),
            SMTO_BLOCK | SMTO_ERRORONEXIT,
            NO_TIME_LIMIT,
            Some(&raw mut result),
        )
    };
    if sent.0 == 0 {
        // SAFETY: IsWindow tolerates any handle.
        return Err(if unsafe { IsWindow(Some(handle(hwnd))) }.as_bool() {
            Unanswered::Failed
        } else {
            Unanswered::Gone
        });
    }
    Ok(result.cast_signed())
}

/// Sends `msg` with integer parameters ([`send_message`]), answering zero
/// when it is not answered.
///
/// # Safety
///
/// `msg` must take plain integers in both parameters, never a pointer.
unsafe fn send(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: the caller's contract rules out pointer parameters.
    unsafe { send_message(hwnd, msg, (wparam, lparam), None) }.unwrap_or(0)
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
