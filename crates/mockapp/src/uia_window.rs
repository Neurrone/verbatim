//! A child window that is a native UIA provider of its own: a fixture node
//! with `"native": "uia_window"` becomes a child window of the host, and
//! the node and its children are served as a scripted tree of their own,
//! rooted at that window, through the same UIA provider as mockapp's own
//! window ([`crate::uia`]), whatever the host answers. In a plain Win32 host
//! (`--backend win32`) it is a UIA window inside a window read through
//! MSAA, as a XAML island is inside a Win32 application. A fixture has at
//! most one: its window class is registered once.

use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, HMENU, RegisterClassExW,
    SetWindowLongPtrW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_GETOBJECT, WNDCLASSEXW, WS_CHILD,
    WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use crate::fixture::FixtureNode;
use crate::tree::{SharedTree, Tree};

const CLASS_NAME: PCWSTR = w!("VerbatimMockAppUiaWindow");

/// Creates `node`'s window in `parent`, serving `node` and its children.
///
/// # Errors
///
/// The class registration's or the window creation's error.
pub(crate) fn create(parent: HWND, node: FixtureNode) -> windows::core::Result<()> {
    // SAFETY: retrieves this module's own instance handle; always sound.
    let hinstance = unsafe { GetModuleHandleW(None) }?;
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(0),
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialized above and outlives the call.
    if unsafe { RegisterClassExW(&raw const class) } == 0 {
        return Err(windows::core::Error::from_thread());
    }
    // Leaked for the process's lifetime, as the host window's context is:
    // mockapp exits shortly after its message loop ends.
    let tree: &'static SharedTree = Box::leak(Box::new(Arc::new(Mutex::new(Tree::build(node)))));
    // SAFETY: the class was registered above; `parent` is the host window,
    // created on this thread; the title is a static string.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!(""),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
            10,
            10,
            300,
            200,
            Some(parent),
            None::<HMENU>,
            Some(hinstance.into()),
            None,
        )
    }?;
    // SAFETY: `hwnd` is the window just created on this thread; `tree` is
    // leaked, so the stored pointer stays valid for every later message.
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, std::ptr::from_ref(tree) as isize);
    }
    Ok(())
}

/// # Safety
///
/// Must only be installed as a window procedure via [`RegisterClassExW`];
/// Windows guarantees the parameters describe a live window and message.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_GETOBJECT {
        // `idObject` is 32 bits, zero-extended into `lParam`, as the host
        // window's procedure reads it.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "idObject is 32 bits by convention"
        )]
        let obj_id = lparam.0 as i32;
        // SAFETY: reads this window's own user data, which is zero until
        // `create` stores the tree.
        let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
        if obj_id == UiaRootObjectId && raw != 0 {
            // SAFETY: `raw` was written by `create` from a leaked, `'static`
            // `SharedTree`.
            let tree = unsafe { &*(raw as *const SharedTree) };
            if let Ok(simple) = crate::uia::root_provider(Arc::clone(tree), hwnd) {
                // SAFETY: `simple` is a live provider for the window's root.
                return unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, &simple) };
            }
        }
    }
    // SAFETY: default handling for everything else, an MSAA request
    // included, which Windows' own window objects answer.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
