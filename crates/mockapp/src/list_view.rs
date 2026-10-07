//! A real Win32 list view for the MSAA backend: a fixture node with
//! `"native": "list_view"` becomes a comctl32 `SysListView32` in the report
//! view, its `columns` (each a header and a width in pixels; a width of 0
//! hides the column) its columns, and its `list_item` children its items:
//! an item's name is its first column's text, and its value, split at each
//! `|`, the texts of the columns after it. It is made in an activation
//! context for Common Controls version 6, as real applications declare it.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    ICC_LISTVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, LVCF_SUBITEM, LVCF_TEXT,
    LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVITEMW, LVM_INSERTCOLUMNW, LVM_INSERTITEMW,
    LVM_SETITEMTEXTW, LVS_REPORT, WC_LISTVIEWW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, HMENU, SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WS_CHILD, WS_VISIBLE,
};
use windows::core::{PCWSTR, PWSTR};

use verbatim_model::Role;

use crate::fixture::FixtureNode;

/// Creates the list view `node` describes inside `parent`.
///
/// # Errors
///
/// The window creation's error.
pub(crate) fn create(parent: HWND, node: &FixtureNode) -> windows::core::Result<HWND> {
    let context = crate::common_controls::CommonControls6::activate()?;
    let created = create_in_context(parent, node);
    drop(context);
    created
}

/// [`create`], with Common Controls version 6 active.
fn create_in_context(parent: HWND, node: &FixtureNode) -> windows::core::Result<HWND> {
    let controls = INITCOMMONCONTROLSEX {
        dwSize: u32::try_from(size_of::<INITCOMMONCONTROLSEX>()).unwrap_or(0),
        dwICC: ICC_LISTVIEW_CLASSES,
    };
    // SAFETY: `controls` is fully initialized and outlives the call.
    let _ = unsafe { InitCommonControlsEx(&raw const controls) };
    let style = WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | LVS_REPORT);
    // SAFETY: comctl32's list view class; `parent` is the host window,
    // created on this thread.
    let list = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WC_LISTVIEWW,
            PCWSTR::null(),
            style,
            0,
            0,
            400,
            300,
            Some(parent),
            None::<HMENU>,
            None,
            None,
        )
    }?;
    for (index, (header, width)) in node.columns.iter().enumerate() {
        let mut text = to_wide(header);
        let column = LVCOLUMNW {
            mask: LVCF_TEXT | LVCF_WIDTH | LVCF_SUBITEM,
            cx: *width,
            pszText: PWSTR(text.as_mut_ptr()),
            iSubItem: i32::try_from(index).unwrap_or(0),
            ..Default::default()
        };
        send(
            list,
            LVM_INSERTCOLUMNW,
            index,
            std::ptr::from_ref(&column) as isize,
        );
    }
    let items = node
        .children
        .iter()
        .filter(|item| item.role == Role::ListItem);
    for (index, item) in items.enumerate() {
        let mut text = to_wide(item.name.as_deref().unwrap_or_default());
        let row = i32::try_from(index).unwrap_or(0);
        let first = LVITEMW {
            mask: LVIF_TEXT,
            iItem: row,
            pszText: PWSTR(text.as_mut_ptr()),
            ..Default::default()
        };
        send(
            list,
            LVM_INSERTITEMW,
            0,
            std::ptr::from_ref(&first) as isize,
        );
        let rest = item.value.as_deref().unwrap_or_default().split('|');
        for (offset, cell) in rest.enumerate() {
            let mut text = to_wide(cell);
            let subitem = LVITEMW {
                iSubItem: i32::try_from(offset + 1).unwrap_or(0),
                pszText: PWSTR(text.as_mut_ptr()),
                ..Default::default()
            };
            send(
                list,
                LVM_SETITEMTEXTW,
                index,
                std::ptr::from_ref(&subitem) as isize,
            );
        }
    }
    Ok(list)
}

/// Sends `msg` to the list view on this thread.
fn send(list: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: `list` is this thread's own list view; every caller passes
    // parameters the message documents, pointers to live locals included.
    unsafe { SendMessageW(list, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
