//! The real tree view of `tests/fixtures/tree_view.json`: finding it, and
//! its items' MSAA child ids, through the control's own messages, sent
//! from the test with plain integer parameters.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{
    TVGN_CHILD, TVGN_NEXT, TVGN_ROOT, TVM_GETNEXTITEM, TVM_MAPHTREEITEMTOACCID,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, OBJID_CLIENT, SendMessageW};
use windows::core::{PCWSTR, w};

use verbatim_outpost::protocol::DeliveredFact;

use super::outpost::{OutpostUnderTest, Reported};

/// The tree view's items, depth first, in the fixture's order.
pub const ITEMS: [&str; 8] = [
    "Hardware", "Disks", "Display", "Software", "Drivers", "Services", "Tasks", "Settings",
];

/// The fixture's tree view, registered under a Windows Forms class name,
/// inside mockapp's window `host`.
pub fn tree_view(host: HWND) -> HWND {
    // SAFETY: a local search of `host`'s children by class.
    unsafe {
        FindWindowExW(
            Some(host),
            None,
            w!("WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"),
            PCWSTR::null(),
        )
    }
    .expect("mockapp made the tree view")
}

/// Sends a tree view message whose parameters are plain integers.
pub fn send(tree: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: every caller sends a message whose parameters are integers,
    // item handles included, which the control produced.
    unsafe { SendMessageW(tree, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
}

/// The item handle and MSAA child id of every item, in [`ITEMS`] order.
fn items(tree: HWND) -> Vec<(isize, i32)> {
    fn walk(tree: HWND, first: isize, items: &mut Vec<(isize, i32)>) {
        let mut item = first;
        while item != 0 {
            let id = send(tree, TVM_MAPHTREEITEMTOACCID, item.cast_unsigned(), 0);
            items.push((item, i32::try_from(id).expect("a child id")));
            let child = send(tree, TVM_GETNEXTITEM, TVGN_CHILD as usize, item);
            walk(tree, child, items);
            item = send(tree, TVM_GETNEXTITEM, TVGN_NEXT as usize, item);
        }
    }
    let mut found = Vec::new();
    walk(
        tree,
        send(tree, TVM_GETNEXTITEM, TVGN_ROOT as usize, 0),
        &mut found,
    );
    assert_eq!(found.len(), ITEMS.len(), "every item is in the tree");
    found
}

/// The index of the item named `name` in [`ITEMS`].
fn index_of(name: &str) -> usize {
    ITEMS
        .iter()
        .position(|item| *item == name)
        .expect("a fixture item")
}

/// The MSAA child id of the item named `name`.
pub fn child_id(tree: HWND, name: &str) -> i32 {
    items(tree)[index_of(name)].1
}

/// The item handle of the item named `name`.
pub fn item(tree: HWND, name: &str) -> isize {
    items(tree)[index_of(name)].0
}

/// Hands `outpost` a focus on the tree item `name`, as the listener would
/// for the control's own focus event, and returns what it reports.
pub fn focus_item(outpost: &OutpostUnderTest, tree: HWND, name: &str) -> Reported {
    let id_child = child_id(tree, name);
    outpost.focus(DeliveredFact::MsaaFocus {
        hwnd: tree.0 as isize,
        id_object: OBJID_CLIENT.0,
        id_child,
    })
}
