//! A Win32 popup menu of the application (`popup-menu`), the system's own
//! menu window (`#32768`), which MSAA's menu proxy serves: two items,
//! "First" and "Second", shown by `TrackPopupMenuEx` on a UI thread of its
//! own, owned by a hidden window there, so the host window's thread goes
//! on serving its provider and the commands while the menu's modal loop
//! runs. `menu-select <index>` selects an item as the menu's keys would
//! (`MN_SELECTITEM`), which raises its focus event, and `menu-close` ends
//! the menu (`EndMenu` on its thread), answered once its loop has returned.
//! Nothing here takes the foreground or the input.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    EndMenu, HMENU, MF_STRING, MSGF_MENU, PostMessageW, RegisterClassExW, SendMessageW,
    TPM_RETURNCMD, TrackPopupMenuEx, WINDOW_EX_STYLE, WM_APP, WM_ENTERIDLE, WNDCLASSEXW,
    WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

use crate::stdin::Command;

const CLASS_NAME: PCWSTR = w!("VerbatimMockAppMenuOwner");

/// The menu window's message that selects the item at `wParam`'s index,
/// as its arrow keys do; `windows` does not name it.
const MN_SELECTITEM: u32 = 0x01E5;

/// Posted to the owner window to end its menu on the menu's thread.
const WM_APP_END_MENU: u32 = WM_APP + 1;

/// The open menu: its window, its owner, and the channel its thread says
/// on that the menu's loop has returned.
struct Open {
    menu: isize,
    owner: isize,
    closed: Receiver<()>,
}

/// The menu open now, if any.
static OPEN: Mutex<Option<Open>> = Mutex::new(None);

/// Where the owner window's procedure sends the menu window's handle once
/// the menu is shown and idle, for the first `WM_ENTERIDLE` only.
static SHOWN: Mutex<Option<Sender<isize>>> = Mutex::new(None);

/// # Safety
///
/// Installed as the owner window's procedure via [`RegisterClassExW`].
unsafe extern "system" fn owner_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        // The menu's loop is idle: the menu is shown, its window `lParam`.
        WM_ENTERIDLE if wparam.0 == MSGF_MENU as usize => {
            if let Some(shown) = SHOWN
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                let _ = shown.send(lparam.0);
            }
            return LRESULT(0);
        }
        WM_APP_END_MENU => {
            // SAFETY: ends this thread's menu; always sound.
            let _ = unsafe { EndMenu() };
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: standard default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Applies one of the menu's commands and returns mockapp's answer to it.
pub(crate) fn command(command: &Command) -> String {
    let done = match command {
        Command::PopupMenu => {
            return open().map_or_else(
                |error| format!("rejected: {error}"),
                |menu| format!("popup-menu {menu}"),
            );
        }
        Command::MenuSelect(index) => select(*index),
        Command::MenuClose => close(),
        _ => Err("not a menu command".to_owned()),
    };
    done.map_or_else(
        |error| format!("rejected: {error}"),
        |()| "applied".to_owned(),
    )
}

/// `popup-menu`: shows the menu on a thread of its own and returns its
/// window's handle once it is shown.
fn open() -> Result<isize, String> {
    let mut open = OPEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if open.is_some() {
        return Err("a menu is already open".to_owned());
    }
    let (shown_tx, shown) = mpsc::channel();
    *SHOWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(shown_tx);
    let (owner_tx, owner) = mpsc::channel();
    let (closed_tx, closed) = mpsc::channel();
    thread::Builder::new()
        .name("mockapp-menu".to_owned())
        .spawn(move || run(&owner_tx, &closed_tx))
        .map_err(|error| error.to_string())?;
    let owner = owner
        .recv()
        .map_err(|_| "the menu's thread ended before it made its owner".to_owned())??;
    let menu = shown
        .recv()
        .map_err(|_| "the menu's thread ended before the menu was shown".to_owned())?;
    *open = Some(Open {
        menu,
        owner,
        closed,
    });
    Ok(menu)
}

/// The menu's thread: its owner window, the menu, and its loop, until the
/// menu ends.
fn run(owner_tx: &Sender<Result<isize, String>>, closed: &Sender<()>) {
    match make_owner() {
        Ok(owner) => {
            let _ = owner_tx.send(Ok(owner.0 as isize));
            track(owner);
            // SAFETY: the owner window this thread made.
            let _ = unsafe { DestroyWindow(owner) };
        }
        Err(error) => {
            let _ = owner_tx.send(Err(error));
        }
    }
    // A menu never shown leaves nobody waiting for it.
    drop(
        SHOWN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take(),
    );
    let _ = closed.send(());
}

/// The menu's hidden owner window, made on this thread.
fn make_owner() -> Result<HWND, String> {
    // SAFETY: the module handle of this process; always sound.
    let instance = unsafe { GetModuleHandleW(None) }.map_err(|error| error.to_string())?;
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>()).unwrap_or(0),
        lpfnWndProc: Some(owner_proc),
        hInstance: instance.into(),
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // Registered once per process; a second menu finds it there.
    // SAFETY: class data from statics and a procedure of this module.
    let _ = unsafe { RegisterClassExW(&raw const class) };
    // SAFETY: the class just registered, no parent, menu or creation data.
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR::null(),
            WS_OVERLAPPED,
            0,
            0,
            10,
            10,
            None,
            None::<HMENU>,
            Some(instance.into()),
            None,
        )
    }
    .map_err(|error| error.to_string())
}

/// Makes the menu, shows it owned by `owner` until it ends, and destroys
/// it.
fn track(owner: HWND) {
    // SAFETY: makes a menu this thread owns; always sound.
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        return;
    };
    for (id, item) in [(1, w!("First")), (2, w!("Second"))] {
        // SAFETY: the menu just made, and a static item string.
        let _ = unsafe { AppendMenuW(menu, MF_STRING, id, item) };
    }
    // SAFETY: the menu and its owner, both this thread's, live throughout.
    let _ = unsafe { TrackPopupMenuEx(menu, TPM_RETURNCMD.0, 0, 0, owner, None) };
    // SAFETY: the menu, no longer shown.
    let _ = unsafe { DestroyMenu(menu) };
}

/// `menu-select <index>`: selects the open menu's item at `index`.
fn select(index: usize) -> Result<(), String> {
    let open = OPEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let open = open.as_ref().ok_or("no menu is open")?;
    // SAFETY: a message to the menu's live window, answered on its thread.
    unsafe {
        SendMessageW(
            HWND(open.menu as *mut std::ffi::c_void),
            MN_SELECTITEM,
            Some(WPARAM(index)),
            Some(LPARAM(0)),
        );
    }
    Ok(())
}

/// `menu-close`: ends the open menu and returns once its loop has.
fn close() -> Result<(), String> {
    let open = OPEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .ok_or("no menu is open")?;
    // SAFETY: a payload-free message to the menu's live owner window.
    unsafe {
        PostMessageW(
            Some(HWND(open.owner as *mut std::ffi::c_void)),
            WM_APP_END_MENU,
            WPARAM(0),
            LPARAM(0),
        )
    }
    .map_err(|error| error.to_string())?;
    open.closed
        .recv()
        .map_err(|_| "the menu's thread ended without saying".to_owned())
}
