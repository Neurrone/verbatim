//! The Win32 host window: registration, creation, the message loop, and
//! `WM_GETOBJECT` dispatch to whichever provider backend is active.
//!
//! The window thread is a single-threaded apartment (`COINIT_APARTMENTTHREADED`):
//! the provider COM objects it creates are apartment-bound
//! (`Agile = false`), so cross-process calls into them are marshaled back
//! onto this thread's message queue, which is why the message loop must
//! keep pumping for the whole process lifetime. Stdin commands arrive on a
//! separate thread (reading stdin blocks, which the message loop cannot
//! afford) and are delivered here over a channel, woken by a lightweight
//! posted message rather than by smuggling a pointer through `LPARAM`.

use std::fmt;
use std::io::Write as _;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use verbatim_model::Backend;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{
    IAccessible, LresultFromObject, UiaReturnRawElementProvider, UiaRootObjectId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetMessageW, GetWindowLongPtrW, HMENU, MSG, OBJID_CLIENT, PostMessageW, PostQuitMessage,
    RegisterClassExW, SetWindowLongPtrW, TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_DESTROY,
    WM_GETOBJECT, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{PCWSTR, w};
use windows_core::Interface;

use crate::stdin::{self, Command};
use crate::tree::SharedTree;
use crate::{msaa, uia};

const CLASS_NAME: PCWSTR = w!("VerbatimMockAppWindow");

/// Posted by the stdin thread to wake the message loop and check the
/// command channel; carries no payload, so nothing crosses threads except
/// the wake-up itself.
const WM_APP_COMMAND_READY: u32 = WM_APP + 1;

/// Everything the window procedure needs, reachable from `GWLP_USERDATA`.
/// Intentionally leaked (`Box::leak`) for the process's lifetime: mockapp
/// exits shortly after the message loop ends, so there is nothing to free.
struct WindowContext {
    backend: Backend,
    tree: SharedTree,
    receiver: Receiver<Command>,
}

/// Failure creating or running the host window.
#[derive(Debug)]
pub(crate) struct WindowError(String);

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for WindowError {}

impl From<windows::core::Error> for WindowError {
    fn from(error: windows::core::Error) -> Self {
        Self(error.to_string())
    }
}

/// Creates the host window, prints `ready`, and runs the message loop until
/// a `quit` command (or the window otherwise closes). `backend` decides how
/// `WM_GETOBJECT` is answered.
pub(crate) fn run(backend: Backend, tree: SharedTree, title: &str) -> Result<(), WindowError> {
    // SAFETY: called once, before any window is created on this thread.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.ok()?;

    // SAFETY: retrieves this module's own instance handle; always sound.
    let hinstance = unsafe { GetModuleHandleW(None) }?;

    register_class(hinstance.into())?;

    let title_wide = to_wide(title);
    // SAFETY: `CLASS_NAME` names a class registered above; `title_wide` is
    // a live, NUL-terminated buffer for the duration of the call.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR(title_wide.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None::<HMENU>,
            Some(hinstance.into()),
            None,
        )
    }?;

    let (sender, receiver) = mpsc::channel();
    let context = Box::leak(Box::new(WindowContext {
        backend,
        tree,
        receiver,
    }));
    // SAFETY: `hwnd` is the window just created on this thread; `context` is
    // leaked and lives for the process's remaining lifetime, so the stored
    // pointer stays valid for every later `WM_GETOBJECT`/`WM_APP_COMMAND_READY`.
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, std::ptr::from_ref(context) as isize);
    }

    println!("ready");
    std::io::stdout().flush().ok();

    let hwnd_value = hwnd.0 as isize;
    thread::Builder::new()
        .name("mockapp-stdin".to_owned())
        .spawn(move || {
            stdin::run(&sender, move || wake(hwnd_value));
        })
        .map_err(|error| WindowError(error.to_string()))?;

    let mut msg = MSG::default();
    // SAFETY: standard Win32 message loop; `msg` is written by `GetMessageW`
    // before each dispatch.
    unsafe {
        while GetMessageW(&raw mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
    Ok(())
}

fn wake(hwnd_value: isize) {
    let hwnd = HWND(hwnd_value as *mut std::ffi::c_void);
    // SAFETY: posting a payload-free message to a window that, by
    // construction, outlives the stdin thread (the process exits only after
    // the message loop returns).
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_APP_COMMAND_READY, WPARAM(0), LPARAM(0));
    }
}

fn register_class(hinstance: windows::Win32::Foundation::HINSTANCE) -> windows::core::Result<()> {
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(0),
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialized above; registering the same class
    // name twice in one process is harmless (mockapp only ever calls this
    // once), and a real failure surfaces as `RegisterClassExW` returning 0.
    let atom = unsafe { RegisterClassExW(&raw const class) };
    if atom == 0 {
        Err(windows::core::Error::from_thread())
    } else {
        Ok(())
    }
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
    match msg {
        WM_GETOBJECT => {
            // SAFETY: `GWLP_USERDATA` was set right after `CreateWindowExW`
            // and never cleared, so any `WM_GETOBJECT` reaching this window
            // has a valid context pointer.
            if let Some(context) = unsafe { context_for(hwnd) } {
                return handle_get_object(hwnd, wparam, lparam, context);
            }
        }
        WM_APP_COMMAND_READY => {
            // SAFETY: see above.
            if let Some(context) = unsafe { context_for(hwnd) } {
                drain_commands(hwnd, context);
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            // SAFETY: posting a plain quit message.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: standard default handling for every message mockapp does not
    // special-case.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// # Safety
///
/// `hwnd` must be a window created by [`run`], so its `GWLP_USERDATA` either
/// holds a valid `WindowContext` pointer or is still zero (before setup).
unsafe fn context_for(hwnd: HWND) -> Option<&'static WindowContext> {
    // SAFETY: forwarded to the caller's contract.
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) };
    if raw == 0 {
        None
    } else {
        // SAFETY: `raw` was written by `run` from a `Box::leak`'d, `'static`
        // `WindowContext`.
        Some(unsafe { &*(raw as *const WindowContext) })
    }
}

fn handle_get_object(
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
    context: &WindowContext,
) -> LRESULT {
    // `lParam` carries `idObject` zero-extended into the full pointer width
    // (Windows does not sign-extend it), so recovering the standard negative
    // object ids (`OBJID_CLIENT` and friends) requires truncating back to
    // 32 bits and reinterpreting, not a checked conversion — `try_from`
    // would reject exactly the values this function needs to recognize.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "idObject is 32 bits by convention"
    )]
    let obj_id = lparam.0 as i32;
    match context.backend {
        Backend::Uia => {
            if obj_id == UiaRootObjectId {
                let provider = uia::root_provider(context.tree.clone(), hwnd);
                let simple: windows::Win32::UI::Accessibility::IRawElementProviderSimple =
                    provider.into();
                // SAFETY: `simple` is a live provider for the root node.
                return unsafe { UiaReturnRawElementProvider(hwnd, wparam, lparam, &simple) };
            }
        }
        Backend::Msaa => {
            if obj_id == OBJID_CLIENT.0 {
                let accessible: IAccessible =
                    msaa::node_accessible(context.tree.clone(), hwnd, 0).into();
                // SAFETY: `accessible` is a live provider for the root node.
                return unsafe { LresultFromObject(&IAccessible::IID, wparam, &accessible) };
            }
            if let Some(index) = msaa::index_from_objid(obj_id) {
                let valid = context
                    .tree
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .nodes
                    .len()
                    > index;
                if valid {
                    let accessible: IAccessible =
                        msaa::node_accessible(context.tree.clone(), hwnd, index).into();
                    // SAFETY: `accessible` is a live provider for a validated node.
                    return unsafe { LresultFromObject(&IAccessible::IID, wparam, &accessible) };
                }
            }
        }
    }
    // SAFETY: falls back to default handling for any object id mockapp does
    // not answer itself (in particular, the MSAA backend must never answer
    // the UIA root object id, so the arbitration probe sees no provider).
    unsafe { DefWindowProcW(hwnd, WM_GETOBJECT, wparam, lparam) }
}

fn drain_commands(hwnd: HWND, context: &WindowContext) {
    while let Ok(command) = context.receiver.try_recv() {
        if matches!(command, Command::Quit) {
            // SAFETY: `hwnd` is this window's own live handle.
            let _ = unsafe { DestroyWindow(hwnd) };
            return;
        }
        match context.backend {
            Backend::Uia => uia::apply_command(&context.tree, hwnd, command),
            Backend::Msaa => msaa::apply_command(&context.tree, hwnd, command),
        }
    }
}

fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
