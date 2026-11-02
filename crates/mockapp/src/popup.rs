//! An owned popup window hosting one node of the host window's tree
//! (`popup <id>`): a top-level window of its own, owned by the host window,
//! whose `WM_GETOBJECT` answers the node's provider, while the node's
//! provider names the popup as its host and its window handle
//! ([`crate::uia`]). UIA then moves the popup from the desktop's children
//! to the node's place under its parent, as a combo box's drop-down list,
//! a top-level window of its own, is the combo box's child: the popup's
//! accessibility parent is in its owner.

use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, HMENU, IsWindowVisible,
    RegisterClassExW, SW_SHOWNOACTIVATE, SetWindowLongPtrW, ShowWindow, WM_GETOBJECT, WNDCLASSEXW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::tree::SharedTree;

const CLASS_NAME: PCWSTR = w!("VerbatimMockAppPopup");

/// What a popup's window procedure serves: the host window's tree, the
/// host window, and the node the popup hosts.
struct Hosted {
    tree: SharedTree,
    owner: HWND,
    index: usize,
}

/// Creates a popup owned by `owner`, the host window, hosting node `index`
/// of `tree`, shown without activation when the host window is shown and
/// hidden otherwise, and returns it.
///
/// # Errors
///
/// The class registration's or the window creation's error.
pub(crate) fn create(tree: SharedTree, owner: HWND, index: usize) -> windows::core::Result<HWND> {
    // SAFETY: retrieves this module's own instance handle; always sound.
    let hinstance = unsafe { GetModuleHandleW(None) }?;
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(0),
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance.into(),
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialized above and outlives the call. A
    // second popup finds the class registered by the first.
    if unsafe { RegisterClassExW(&raw const class) } == 0
        // SAFETY: reads this thread's last error; always sound.
        && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
    {
        return Err(windows::core::Error::from_thread());
    }
    // Leaked for the process's lifetime, as the host window's context is:
    // mockapp exits shortly after its message loop ends.
    let hosted: &'static Hosted = Box::leak(Box::new(Hosted { tree, owner, index }));
    // SAFETY: the class was registered above; `owner` is the host window,
    // created on this thread; the title is a static string.
    let popup = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS_NAME,
            w!(""),
            WS_POPUP,
            20,
            20,
            200,
            150,
            Some(owner),
            None::<HMENU>,
            Some(hinstance.into()),
            None,
        )
    }?;
    // SAFETY: `popup` is the window just created on this thread; `hosted`
    // is leaked, so the stored pointer stays valid for every later message.
    unsafe {
        SetWindowLongPtrW(popup, GWLP_USERDATA, std::ptr::from_ref(hosted) as isize);
    }
    // SAFETY: `owner` is this thread's live window.
    if unsafe { IsWindowVisible(owner) }.as_bool() {
        // SAFETY: `popup` is this thread's live window. The return value
        // is only whether it was visible before.
        let _ = unsafe { ShowWindow(popup, SW_SHOWNOACTIVATE) };
    }
    Ok(popup)
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
        // `create` stores what it hosts.
        let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
        if obj_id == UiaRootObjectId && raw != 0 {
            // SAFETY: `raw` was written by `create` from a leaked, `'static`
            // `Hosted`.
            let hosted = unsafe { &*(raw as *const Hosted) };
            if let Ok(simple) = crate::uia::node_provider(
                std::sync::Arc::clone(&hosted.tree),
                hosted.owner,
                hosted.index,
            ) {
                // SAFETY: `simple` is a live provider for the hosted node.
                return unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, &simple) };
            }
        }
    }
    // SAFETY: default handling for everything else, an MSAA request
    // included, which Windows' own window objects answer.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
