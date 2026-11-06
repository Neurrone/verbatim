//! `hold-message <message>`: the window thread blocks inside the window
//! procedure of the native control that answers the message, on the next
//! message of that kind sent to it, until `release`, printing
//! `held <message>`, so a test knows a client's message is waiting in the
//! control's window procedure and decides when it is answered. The
//! control's window procedure is replaced by [`holding_proc`], which passes
//! every message on to the one it replaced.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, FindWindowExW, GWLP_WNDPROC, SetWindowLongPtrW, WNDPROC,
};
use windows::core::{PCWSTR, w};

use crate::hits;

/// The messages `hold-message` holds, by name, with their numbers.
const MESSAGES: [(&str, u32); 2] = [("EM_GETSEL", 0x00B0), ("LVM_GETITEMTEXTW", 0x1073)];

thread_local! {
    /// Each control's own window procedure, by window, once
    /// [`holding_proc`] has replaced it.
    static ORIGINAL: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
    /// The message the next of which is held, and its name.
    static HELD: RefCell<Option<(u32, &'static str)>> = const { RefCell::new(None) };
}

/// The number of the message `name` names, if `hold-message` holds it.
pub(crate) fn message_named(name: &str) -> Option<u32> {
    MESSAGES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|&(_, number)| number)
}

/// Holds the next `message` sent to the native control of `host` that
/// answers it, an edit control for an `EM_` message and a list view for an
/// `LVM_` one, on the window thread.
///
/// # Errors
///
/// When the host has no such control, or the message is not one this holds.
pub(crate) fn hold(host: HWND, message: u32) -> Result<(), String> {
    let &(name, _) = MESSAGES
        .iter()
        .find(|&&(_, number)| number == message)
        .ok_or_else(|| format!("message {message:#x} cannot be held"))?;
    let class = if name.starts_with("EM_") {
        w!("EDIT")
    } else {
        w!("SysListView32")
    };
    // SAFETY: a local search of the host's children by class.
    let control = unsafe { FindWindowExW(Some(host), None, class, PCWSTR::null()) }
        .map_err(|_| format!("the window has no control that answers {name}"))?;
    ORIGINAL.with_borrow_mut(|original| {
        original.entry(control.0 as isize).or_insert_with(|| {
            // SAFETY: `control` is this thread's own control, and
            // `holding_proc` passes every message on to the procedure it
            // replaces.
            unsafe { SetWindowLongPtrW(control, GWLP_WNDPROC, holding_proc as *const () as isize) }
        });
    });
    hits::arm_hold();
    HELD.with_borrow_mut(|held| *held = Some((message, name)));
    Ok(())
}

/// A held control's window procedure: holds the message `hold-message`
/// named once, inside the window procedure, then passes every message on
/// to the control's own procedure.
///
/// # Safety
///
/// Installed only as a control's window procedure by [`hold`].
unsafe extern "system" fn holding_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let held = HELD.with_borrow_mut(|held| held.take_if(|(message, _)| *message == msg));
    if let Some((_, name)) = held {
        hits::held(name);
    }
    let original = ORIGINAL.with_borrow(|original| {
        original
            .get(&(hwnd.0 as isize))
            .copied()
            .unwrap_or_default()
    });
    // SAFETY: `original` is the control's own procedure, stored before this
    // one was installed.
    let procedure: WNDPROC = unsafe { std::mem::transmute::<isize, WNDPROC>(original) };
    // SAFETY: passes the message on to the procedure this one replaced.
    unsafe { CallWindowProcW(procedure, hwnd, msg, wparam, lparam) }
}
