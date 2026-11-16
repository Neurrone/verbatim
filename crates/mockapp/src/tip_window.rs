//! A scripted node hosted in a window of comctl32's own tooltip class,
//! `tooltips_class32` (`TOOLTIPS_CLASSW`), for a fixture node that names
//! that class as its `window_class` on the MSAA backend: a top-level window
//! owned by the host window, titled with the node's name, whose client
//! object is the node's scripted object.
//!
//! A help balloon and a tooltip are both windows of that class, a balloon
//! made with `TTS_BALLOON`, and NVDA speaks a show only from its six window
//! classes, `tooltips_class32` among them (`eventHandler.py` 506 to 515;
//! `NVDAObjects/IAccessible/__init__.py` 2845 to 2846 tell the two apart by
//! role). The class is the one commctrl.h names for every tooltip control,
//! checked against the class comctl32 registers for one, so a show here is
//! judged on the class a real balloon's window has.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Accessibility::{IAccessible, LresultFromObject};
use windows::Win32::UI::Controls::{
    ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, TOOLTIPS_CLASSW, TTS_ALWAYSTIP,
    TTS_BALLOON,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, CreateWindowExW, GWLP_WNDPROC, HMENU, OBJID_CLIENT, SetWindowLongPtrW,
    WINDOW_STYLE, WM_GETOBJECT, WNDPROC, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{Interface, PCWSTR};

use crate::tree::SharedTree;

/// What a tooltip window serves: the host window's tree, the node, and
/// comctl32's own window procedure, which every other message goes to.
struct Hosted {
    tree: SharedTree,
    index: usize,
    procedure: isize,
}

thread_local! {
    /// The windows made here, by handle, on the window thread that made
    /// them.
    static HOSTED: RefCell<HashMap<isize, Hosted>> = RefCell::default();
}

/// Makes a tooltip window, owned by `owner`, for every node of `tree` that
/// names the tooltip class as its window's.
///
/// # Errors
///
/// A window creation's error.
pub(crate) fn create_all(tree: &SharedTree, owner: HWND) -> windows::core::Result<()> {
    let tips: Vec<(usize, String, bool)> = tree
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.window_class.as_deref() == Some("tooltips_class32"))
        .map(|(index, node)| {
            (
                index,
                node.name.clone().unwrap_or_default(),
                node.role == verbatim_model::Role::HelpBalloon,
            )
        })
        .collect();
    for (index, name, balloon) in tips {
        create(std::sync::Arc::clone(tree), owner, (index, &name, balloon))?;
    }
    Ok(())
}

/// Makes the tooltip window hosting node `index` of `tree`, owned by
/// `owner`, a balloon's (`TTS_BALLOON`) when `balloon`, titled `name`.
///
/// # Errors
///
/// The window creation's error.
fn create(
    tree: SharedTree,
    owner: HWND,
    (index, name, balloon): (usize, &str, bool),
) -> windows::core::Result<HWND> {
    let controls = INITCOMMONCONTROLSEX {
        dwSize: u32::try_from(size_of::<INITCOMMONCONTROLSEX>()).unwrap_or(0),
        dwICC: ICC_WIN95_CLASSES,
    };
    // SAFETY: `controls` is fully initialized and outlives the call.
    let _ = unsafe { InitCommonControlsEx(&raw const controls) };
    let title: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut style = WS_POPUP.0 | TTS_ALWAYSTIP;
    if balloon {
        style |= TTS_BALLOON;
    }
    // SAFETY: comctl32's class, registered by the call above; `owner` is
    // the host window, made on this thread; `title` outlives the call.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            TOOLTIPS_CLASSW,
            PCWSTR(title.as_ptr()),
            WINDOW_STYLE(style),
            0,
            0,
            10,
            10,
            Some(owner),
            None::<HMENU>,
            None,
            None,
        )
    }?;
    // The new procedure passes every message it does not answer to the
    // old one; `window` is the window just made on this thread.
    let ours: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = procedure;
    // SAFETY: as above.
    let procedure =
        unsafe { SetWindowLongPtrW(window, GWLP_WNDPROC, (ours as usize).cast_signed()) };
    HOSTED.with(|hosted| {
        hosted.borrow_mut().insert(
            window.0 as isize,
            Hosted {
                tree,
                index,
                procedure,
            },
        );
    });
    Ok(window)
}

/// The tooltip window's procedure: its client object is the node's, and
/// every other message is comctl32's.
///
/// # Safety
///
/// Installed only by [`create`], on a window of this thread.
unsafe extern "system" fn procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let found = HOSTED.with(|hosted| {
        hosted
            .borrow()
            .get(&(hwnd.0 as isize))
            .map(|hosted| (hosted.tree.clone(), hosted.index, hosted.procedure))
    });
    let Some((tree, index, previous)) = found else {
        return LRESULT(0);
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "idObject is 32 bits by convention"
    )]
    if msg == WM_GETOBJECT && lparam.0 as i32 == OBJID_CLIENT.0 {
        let accessible: IAccessible = crate::msaa::node_accessible(&tree, hwnd, index);
        // SAFETY: `accessible` is a live object for the node.
        return unsafe { LresultFromObject(&IAccessible::IID, wparam, &accessible) };
    }
    // SAFETY: `previous` is the procedure `SetWindowLongPtrW` replaced,
    // comctl32's, whose address it returned.
    let previous: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(previous) };
    // SAFETY: passes the message on to the window's own procedure.
    unsafe { CallWindowProcW(previous, hwnd, msg, wparam, lparam) }
}
