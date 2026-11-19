//! A real dialog shown after its focus, as a common file dialog is
//! (`open-dialog-shown-late <ms> [owned-box|hide-close]`), and a tray menu
//! over a hidden dialog-based main window (`tray-menu-over-hidden-dialog`):
//! the stand-ins for the hidden dialog's cases (Dickson, 2026-11-19; the
//! recordings of Import, Save As, Run and Properties showed every dialog
//! hidden at its foreground shown 14 to 221 ms later).
//!
//! `open-dialog-shown-late` makes a dialog of the system's dialog class
//! (`#32770`) from a template without `WS_VISIBLE`, owned by mockapp's
//! window, on a UI thread of its own so the host's thread goes on serving
//! its provider, and answering the messages the owned dialog sends it
//! while it waits for the dialog's handle; brings it to the foreground and focuses its "File name:"
//! Edit while it is hidden, as Import raises its foreground and then its
//! field's focus; then, after `<ms>`, shows it. `owned-box` raises a message
//! box the dialog owns after the field's focus and before the show, the
//! show following its dismissal; `hide-close` closes the dialog by
//! hiding it before its show, the foreground going back to mockapp's
//! window. Deliberate differences from Import: no `ComboBoxEx32` around
//! the field and no shell view. The answer is `open-dialog-shown-late
//! <hwnd>`, the dialog's decimal handle, once its field has the focus.
//!
//! `tray-menu-over-hidden-dialog` makes such a dialog with no owner, never
//! shown, brings it to the foreground and opens a popup menu it owns
//! (`TrackPopupMenuEx`), as a tray icon's menu opens over a dialog-based
//! main window; it answers `tray-menu-over-hidden-dialog <hwnd>` as the
//! menu opens, and the menu ends with Escape.

use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateDialogIndirectParamW, CreatePopupMenu, DLGTEMPLATE, DS_SETFONT, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetDlgItem, GetMessageW, IsDialogMessageW, IsWindow, MB_OK,
    MF_STRING, MSG, MessageBoxW, SW_HIDE, SW_SHOW, SetForegroundWindow, ShowWindow, TPM_RETURNCMD,
    TrackPopupMenuEx, TranslateMessage, WS_CAPTION, WS_CHILD, WS_POPUP, WS_SYSMENU, WS_TABSTOP,
    WS_VISIBLE,
};
use windows::core::w;

/// What `open-dialog-shown-late` does between the field's focus and the
/// show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BeforeShow {
    /// Nothing: the dialog is shown after its delay.
    Nothing,
    /// A message box the dialog owns, the show after its dismissal.
    OwnedBox,
    /// The dialog closed by hiding, never shown.
    HideClose,
}

/// The field's control id.
const FIELD: u16 = 100;
/// The label's control id.
const LABEL: u16 = 101;

/// `open-dialog-shown-late <ms> [owned-box|hide-close]` parsed.
pub(crate) fn parse(rest: &str) -> Option<(u64, BeforeShow)> {
    let mut words = rest.split_whitespace();
    let ms = words.next()?.parse().ok()?;
    let before = match words.next() {
        None => BeforeShow::Nothing,
        Some("owned-box") => BeforeShow::OwnedBox,
        Some("hide-close") => BeforeShow::HideClose,
        Some(_) => return None,
    };
    words.next().is_none().then_some((ms, before))
}

/// The answer to `command` on mockapp's window `hwnd`, when it is one of
/// this module's.
pub(crate) fn command(hwnd: HWND, command: &crate::stdin::Command) -> Option<String> {
    match command {
        crate::stdin::Command::OpenDialogShownLate(ms, before) => {
            Some(open_shown_late(hwnd, *ms, *before))
        }
        crate::stdin::Command::TrayMenuOverHiddenDialog => Some(tray_menu_over_hidden_dialog()),
        _ => None,
    }
}

/// The answer to `open-dialog-shown-late` for the window `owner`.
fn open_shown_late(owner: HWND, ms: u64, before: BeforeShow) -> String {
    let owner = owner.0 as isize;
    spawn("open-dialog-shown-late", move |made| {
        dialog_shown_late(HWND(owner as _), ms, before, made);
    })
}

/// The answer to `tray-menu-over-hidden-dialog`.
fn tray_menu_over_hidden_dialog() -> String {
    spawn("tray-menu-over-hidden-dialog", tray_menu)
}

/// Runs `run` on a UI thread of its own, which sends the dialog's handle
/// once it is ready, and answers it as `<name> <hwnd>`.
fn spawn(name: &str, run: impl FnOnce(&Sender<Result<isize, String>>) + Send + 'static) -> String {
    let (made_tx, made) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name(format!("mockapp-{name}"))
        .spawn(move || run(&made_tx));
    if let Err(error) = spawned {
        return format!("rejected: {error}");
    }
    // The host's window thread answers its messages while it waits: a
    // dialog it owns sends it messages as it is made and activated, which a
    // thread blocked in the wait would never answer.
    loop {
        match made.try_recv() {
            Ok(Ok(dialog)) => return format!("{name} {dialog}"),
            Ok(Err(error)) => return format!("rejected: {error}"),
            Err(mpsc::TryRecvError::Disconnected) => {
                return format!("rejected: the {name} thread ended first");
            }
            Err(mpsc::TryRecvError::Empty) => {
                let until = std::time::Instant::now() + Duration::from_millis(1);
                if let Err(error) = crate::window::wait_pumping(until) {
                    return format!("rejected: {error}");
                }
            }
        }
    }
}

/// The dialog's thread for `open-dialog-shown-late`.
fn dialog_shown_late(
    owner: HWND,
    ms: u64,
    before: BeforeShow,
    made: &Sender<Result<isize, String>>,
) {
    let dialog = match hidden_dialog(Some(owner)) {
        Ok(dialog) => dialog,
        Err(error) => {
            let _ = made.send(Err(error));
            return;
        }
    };
    // SAFETY: a live dialog of this thread's.
    let _ = unsafe { SetForegroundWindow(dialog) };
    // SAFETY: the same dialog.
    if let Ok(field) = unsafe { GetDlgItem(Some(dialog), i32::from(FIELD)) } {
        // SAFETY: its field, a live window of this thread's.
        let _ = unsafe { SetFocus(Some(field)) };
    }
    let _ = made.send(Ok(dialog.0 as isize));
    if before == BeforeShow::OwnedBox {
        // SAFETY: a modal box this dialog owns, with static strings.
        let _ = unsafe { MessageBoxW(Some(dialog), w!("Replace it?"), w!("Confirm"), MB_OK) };
    }
    pump_for(Duration::from_millis(ms), dialog);
    match before {
        BeforeShow::HideClose => {
            // SAFETY: the dialog of this thread's, live.
            let _ = unsafe { ShowWindow(dialog, SW_HIDE) };
            // SAFETY: its owner, which takes the foreground back.
            let _ = unsafe { SetForegroundWindow(owner) };
            // SAFETY: the dialog, destroyed once.
            let _ = unsafe { DestroyWindow(dialog) };
            return;
        }
        BeforeShow::Nothing | BeforeShow::OwnedBox => {
            // SAFETY: the dialog of this thread's.
            let _ = unsafe { ShowWindow(dialog, SW_SHOW) };
        }
    }
    pump_while(dialog);
}

/// The thread of `tray-menu-over-hidden-dialog`.
fn tray_menu(made: &Sender<Result<isize, String>>) {
    let dialog = match hidden_dialog(None) {
        Ok(dialog) => dialog,
        Err(error) => {
            let _ = made.send(Err(error));
            return;
        }
    };
    // SAFETY: a live dialog of this thread's.
    let _ = unsafe { SetForegroundWindow(dialog) };
    // SAFETY: makes a menu this thread owns; always sound.
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
        let _ = made.send(Err("no popup menu".to_owned()));
        return;
    };
    for (id, item) in [(1, w!("Open")), (2, w!("Exit"))] {
        // SAFETY: the menu just made, and a static item string.
        let _ = unsafe { AppendMenuW(menu, MF_STRING, id, item) };
    }
    let _ = made.send(Ok(dialog.0 as isize));
    // SAFETY: the menu and its owner, both this thread's, live throughout.
    let _ = unsafe { TrackPopupMenuEx(menu, TPM_RETURNCMD.0, 0, 0, dialog, None) };
    // SAFETY: the menu, this thread's, no longer shown.
    let _ = unsafe { DestroyMenu(menu) };
    // SAFETY: the dialog, this thread's, destroyed once.
    let _ = unsafe { DestroyWindow(dialog) };
}

/// A dialog of the system's dialog class, titled "Import a Theme", with a
/// "File name:" label and an Edit, made hidden from an in-memory template,
/// owned by `owner`.
fn hidden_dialog(owner: Option<HWND>) -> Result<HWND, String> {
    let template = template();
    // SAFETY: a template laid out as `DLGTEMPLATE` with its items, each
    // aligned, alive for the call; no dialog procedure, as a dialog of the
    // system's class needs none.
    unsafe {
        CreateDialogIndirectParamW(
            None,
            template.as_ptr().cast::<DLGTEMPLATE>(),
            owner,
            None,
            LPARAM(0),
        )
    }
    .map_err(|error| error.to_string())
}

/// The dialog's template: `DLGTEMPLATE`, its menu, class, title and font,
/// then each `DLGITEMTEMPLATE` on a four-byte boundary with its class
/// (by atom), its text and no creation data.
fn template() -> Vec<u16> {
    let mut words = Vec::new();
    let style = (WS_POPUP | WS_CAPTION | WS_SYSMENU).0 | DS_SETFONT as u32;
    push_u32(&mut words, style);
    push_u32(&mut words, 0);
    words.push(2);
    for value in [0_i16, 0, 200, 60] {
        words.push(value.cast_unsigned());
    }
    words.push(0);
    words.push(0);
    push_text(&mut words, "Import a Theme");
    words.push(9);
    push_text(&mut words, "Segoe UI");
    item(
        &mut words,
        (WS_CHILD | WS_VISIBLE).0,
        (10, 10, 50, 12),
        (LABEL, 0x0082, "File name:"),
    );
    item(
        &mut words,
        (WS_CHILD | WS_VISIBLE | WS_TABSTOP).0 | 0x0080,
        (65, 10, 120, 12),
        (FIELD, 0x0081, ""),
    );
    words
}

fn item(words: &mut Vec<u16>, style: u32, place: (i16, i16, i16, i16), what: (u16, u16, &str)) {
    if words.len() % 2 == 1 {
        words.push(0);
    }
    push_u32(words, style);
    push_u32(words, 0);
    for value in [place.0, place.1, place.2, place.3] {
        words.push(value.cast_unsigned());
    }
    let (id, class, text) = what;
    words.push(id);
    words.push(0xFFFF);
    words.push(class);
    push_text(words, text);
    words.push(0);
}

fn push_u32(words: &mut Vec<u16>, value: u32) {
    words.push((value & 0xFFFF) as u16);
    words.push((value >> 16) as u16);
}

fn push_text(words: &mut Vec<u16>, text: &str) {
    words.extend(text.encode_utf16());
    words.push(0);
}

/// Pumps this thread's messages for `length`, the dialog's among them.
fn pump_for(length: Duration, dialog: HWND) {
    let started = std::time::Instant::now();
    let mut message = MSG::default();
    while started.elapsed() < length {
        // SAFETY: a message this function owns.
        let any = unsafe {
            windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
                &raw mut message,
                None,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::PM_REMOVE,
            )
        };
        if any.as_bool() {
            dispatch(dialog, &message);
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Pumps this thread's messages while `dialog` lives.
fn pump_while(dialog: HWND) {
    let mut message = MSG::default();
    // SAFETY: a live or destroyed window handle; `IsWindow` tolerates both.
    while unsafe { IsWindow(Some(dialog)) }.as_bool() {
        // SAFETY: a message this function owns.
        let got = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        if got.0 <= 0 {
            return;
        }
        dispatch(dialog, &message);
    }
}

fn dispatch(dialog: HWND, message: &MSG) {
    // SAFETY: a message just taken off this thread's queue.
    if unsafe { IsDialogMessageW(dialog, message) }.as_bool() {
        return;
    }
    // SAFETY: as above.
    let _ = unsafe { TranslateMessage(message) };
    // SAFETY: as above.
    unsafe { DispatchMessageW(message) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_takes_a_delay_and_what_comes_before_the_show() {
        assert_eq!(parse("120"), Some((120, BeforeShow::Nothing)));
        assert_eq!(parse("90 owned-box"), Some((90, BeforeShow::OwnedBox)));
        assert_eq!(parse("90 hide-close"), Some((90, BeforeShow::HideClose)));
        assert_eq!(parse("90 other"), None);
        assert_eq!(parse(""), None);
    }

    /// The template makes a hidden dialog of the system's class, with its
    /// field.
    #[test]
    fn the_template_makes_a_hidden_dialog_with_its_field() {
        let dialog = hidden_dialog(None).expect("a dialog");
        let raw = dialog.0 as isize;
        assert_eq!(verbatim_win32::window::window_class_name(raw), "#32770");
        assert!(!verbatim_win32::window::window_is_visible(raw));
        // SAFETY: the dialog just made, on this thread.
        let field = unsafe { GetDlgItem(Some(dialog), i32::from(FIELD)) }.expect("its field");
        assert_eq!(
            verbatim_win32::window::window_class_name(field.0 as isize),
            "Edit"
        );
        // SAFETY: the dialog made above, on this thread.
        unsafe { DestroyWindow(dialog) }.expect("the dialog destroyed");
    }
}
