//! A real Win32 tree view for the MSAA backend: a fixture node with
//! `"native": "tree_view"` becomes a comctl32 `SysTreeView32` child window of
//! the host window, its `tree_item` children the control's items, so a
//! client reads it through comctl32's own MSAA implementation and its
//! `TVM_*` messages, as Verbatim reads the tree views of real applications.
//!
//! The node's optional `window_class` registers the control under another
//! class name first, a superclass that answers `WM_GETOBJECT` with
//! comctl32's tree view proxy (`CreateStdAccessibleProxyW`), as Windows
//! Forms wraps the native control under a name like
//! `WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1`. With `state_images`,
//! every item has a state image, as in a tree view that draws its own check
//! boxes (msconfig's, say) rather than having the control's
//! (`TVS_CHECKBOXES`, whose items MSAA reports as check boxes rather than
//! tree view items): unchecked, or checked or partly checked as the item's
//! `checked` or `mixed` state says. An item's `expanded` state expands it,
//! and `selected` makes it the control's selected item.

use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{
    CreateStdAccessibleProxyW, IAccessible, LresultFromObject,
};
use windows::Win32::UI::Controls::{
    HTREEITEM, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx,
    TREE_VIEW_ITEM_STATE_FLAGS, TVE_EXPAND, TVGN_CARET, TVI_LAST, TVI_ROOT, TVIF_STATE, TVIF_TEXT,
    TVINSERTSTRUCTW, TVINSERTSTRUCTW_0, TVIS_STATEIMAGEMASK, TVITEMW, TVM_EXPAND, TVM_INSERTITEMW,
    TVM_SELECTITEM, TVM_SETITEMW, TVS_HASBUTTONS, TVS_HASLINES, WC_TREEVIEWW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, GetClassInfoExW, HMENU, OBJID_CLIENT, RegisterClassExW,
    SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_GETOBJECT, WNDCLASSEXW, WNDPROC, WS_CHILD,
    WS_VISIBLE,
};
use windows::core::{PCWSTR, PWSTR};
use windows_core::Interface;

use verbatim_model::{Role, State};

use crate::fixture::FixtureNode;

/// The comctl32 tree view's own window procedure, which the superclass
/// passes every message on to: the procedure `GetClassInfoExW` reported for
/// `SysTreeView32`, stored once the class is registered.
static TREE_VIEW_PROC: AtomicIsize = AtomicIsize::new(0);

/// Creates the tree view `node` describes inside `parent`.
///
/// # Errors
///
/// The class registration's or window creation's error.
pub(crate) fn create(parent: HWND, node: &FixtureNode) -> windows::core::Result<HWND> {
    let controls = INITCOMMONCONTROLSEX {
        dwSize: u32::try_from(size_of::<INITCOMMONCONTROLSEX>()).unwrap_or(0),
        dwICC: ICC_TREEVIEW_CLASSES,
    };
    // SAFETY: `controls` is fully initialized and outlives the call.
    let _ = unsafe { InitCommonControlsEx(&raw const controls) };
    let class: Vec<u16> = match &node.window_class {
        Some(name) => {
            let wide = to_wide(name);
            register_superclass(&wide)?;
            wide
        }
        None => Vec::new(),
    };
    let class_name = if class.is_empty() {
        WC_TREEVIEWW
    } else {
        PCWSTR(class.as_ptr())
    };
    let style = WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | TVS_HASBUTTONS | TVS_HASLINES);
    // SAFETY: the class is comctl32's, or registered above from it;
    // `parent` is the host window, created on this thread.
    let tree = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            PCWSTR::null(),
            style,
            0,
            0,
            300,
            300,
            Some(parent),
            None::<HMENU>,
            None,
            None,
        )
    }?;
    insert_items(tree, TVI_ROOT, &node.children, node.state_images);
    Ok(tree)
}

/// Inserts `items` under `parent`, depth first, then expands and selects
/// them as their states say.
fn insert_items(tree: HWND, parent: HTREEITEM, items: &[FixtureNode], state_images: bool) {
    for item in items.iter().filter(|item| item.role == Role::TreeItem) {
        let mut text = to_wide(item.name.as_deref().unwrap_or_default());
        let insert = TVINSERTSTRUCTW {
            hParent: parent,
            hInsertAfter: TVI_LAST,
            Anonymous: TVINSERTSTRUCTW_0 {
                item: TVITEMW {
                    mask: TVIF_TEXT,
                    pszText: PWSTR(text.as_mut_ptr()),
                    ..Default::default()
                },
            },
        };
        let handle = send(
            tree,
            TVM_INSERTITEMW,
            0,
            std::ptr::from_ref(&insert) as isize,
        );
        let handle = HTREEITEM(handle);
        if state_images {
            // The state image, as a check box drawn by the application
            // reads: 1 unchecked, 2 checked, 3 partly checked.
            let image: u32 = if item.states.contains(State::Mixed) {
                3
            } else if item.states.contains(State::Checked) {
                2
            } else {
                1
            };
            let update = TVITEMW {
                mask: TVIF_STATE,
                hItem: handle,
                state: TREE_VIEW_ITEM_STATE_FLAGS(image << 12),
                stateMask: TVIS_STATEIMAGEMASK,
                ..Default::default()
            };
            send(tree, TVM_SETITEMW, 0, std::ptr::from_ref(&update) as isize);
        }
        insert_items(tree, handle, &item.children, state_images);
        if item.states.contains(State::Expanded) {
            send(tree, TVM_EXPAND, TVE_EXPAND.0 as usize, handle.0);
        }
        if item.states.contains(State::Selected) {
            send(tree, TVM_SELECTITEM, TVGN_CARET as usize, handle.0);
        }
    }
}

/// Registers `class` as a superclass of comctl32's tree view.
fn register_superclass(class: &[u16]) -> windows::core::Result<()> {
    let mut info = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(0),
        ..Default::default()
    };
    // SAFETY: `info` is a local with `cbSize` set, which the call fills in;
    // the tree view class was registered by `InitCommonControlsEx`.
    unsafe { GetClassInfoExW(None, WC_TREEVIEWW, &raw mut info) }?;
    let original = info
        .lpfnWndProc
        .map_or(0, |procedure| (procedure as usize).cast_signed());
    TREE_VIEW_PROC.store(original, Ordering::Relaxed);
    // SAFETY: retrieves this module's own instance handle; always sound.
    info.hInstance = unsafe { GetModuleHandleW(None) }?.into();
    info.lpszClassName = PCWSTR(class.as_ptr());
    info.lpfnWndProc = Some(superclass_proc);
    // SAFETY: `info` is fully initialized and `class` outlives the call.
    let atom = unsafe { RegisterClassExW(&raw const info) };
    if atom == 0 {
        Err(windows::core::Error::from_thread())
    } else {
        Ok(())
    }
}

/// The superclass's window procedure: comctl32's tree view proxy for the
/// client object, as Windows Forms answers it, and comctl32's own procedure
/// for everything else.
///
/// # Safety
///
/// Installed only as a window procedure by [`register_superclass`].
unsafe extern "system" fn superclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "idObject is 32 bits by convention"
    )]
    if msg == WM_GETOBJECT && lparam.0 as i32 == OBJID_CLIENT.0 {
        let mut proxy: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: `hwnd` is this live tree view; the proxy is written to
        // the local out-parameter.
        let made = unsafe {
            CreateStdAccessibleProxyW(
                hwnd,
                WC_TREEVIEWW,
                OBJID_CLIENT.0,
                &IAccessible::IID,
                &raw mut proxy,
            )
        };
        if made.is_ok() && !proxy.is_null() {
            // SAFETY: the call returned an owned `IAccessible` pointer.
            let accessible = unsafe { IAccessible::from_raw(proxy) };
            // SAFETY: `accessible` is a live object for this window.
            return unsafe { LresultFromObject(&IAccessible::IID, wparam, &accessible) };
        }
    }
    let original = TREE_VIEW_PROC.load(Ordering::Relaxed);
    // SAFETY: `original` is comctl32's tree view procedure, stored before
    // any window of the superclass existed.
    let procedure: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(original) };
    // SAFETY: passes the message on to the procedure the class wraps.
    unsafe { CallWindowProcW(procedure, hwnd, msg, wparam, lparam) }
}

/// Sends `msg` to the tree view on this thread.
fn send(tree: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: `tree` is this thread's own tree view; every caller passes
    // parameters the message documents, pointers to live locals included.
    unsafe { SendMessageW(tree, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
