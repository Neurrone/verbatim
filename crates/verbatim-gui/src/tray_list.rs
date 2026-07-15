//! The system tray and taskbar items dialog, replicating NVDA's systrayList
//! add-on exactly (roadmap, milestone M3): a label over a single-selection
//! list of item names with four buttons — Left Click, Left Double Click,
//! Right Click, and Cancel — where each click action moves the pointer to
//! the center of the selected item's screen rectangle and injects the
//! matching mouse events, then closes the dialog.
//!
//! Presentation goes through the reusable [`crate::list_dialog`] component;
//! this module only supplies the titles and labels for the two dialog
//! flavors, the payload table of enumerated [`ShellItem`]s, and the click
//! injection. Everything here runs on the GUI thread; the injection itself
//! (`SetCursorPos` plus `SendInput`) is non-blocking, so it needs no worker.

use std::mem::size_of;
use std::rc::Rc;

use wxdragon::prelude::*;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

use verbatim_i18n::messages;
use verbatim_model::Rect;

use crate::list_dialog::{ButtonVerdict, ListDialogButton, ListDialogSpec, show_list_dialog};
use crate::shell_items::{ShellItem, ShellItemKind, center_of};

/// The mouse action one of the dialog's click buttons injects.
#[derive(Clone, Copy, Debug)]
enum ClickAction {
    /// One left click: left down, left up.
    Left,
    /// A left double click: two left clicks in one injection.
    DoubleLeft,
    /// One right click: right down, right up.
    Right,
}

/// Builds the tray or taskbar list dialog over the enumerated `items` and
/// returns its handle; the caller shows it and records the singleton.
///
/// Each click button's callback clicks the selected item (by its index into
/// `items`, the list dialog's opaque payload key) and then closes the
/// dialog through [`crate::close_shell_list`], the roadmap's ordering.
/// GUI thread only.
pub(crate) fn build_shell_list_dialog(
    parent: Frame,
    kind: ShellItemKind,
    items: Vec<ShellItem>,
) -> Dialog {
    let (title, label) = match kind {
        ShellItemKind::SystemTray => (messages::tray_list_title(), messages::tray_list_label()),
        ShellItemKind::Taskbar => (
            messages::taskbar_list_title(),
            messages::taskbar_list_label(),
        ),
    };
    let names = items.iter().map(|item| item.name.clone()).collect();
    let items = Rc::new(items);
    let action_button = |label_key: &str, action: ClickAction| ListDialogButton {
        label_key: label_key.to_owned(),
        on_activate: {
            let items = Rc::clone(&items);
            Rc::new(move |index| {
                if let Some(item) = items.get(index) {
                    click(action, item.rect);
                }
                ButtonVerdict::Close
            })
        },
    };
    let spec = ListDialogSpec {
        title,
        label,
        items: names,
        buttons: vec![
            action_button("tray-list-left-click", ClickAction::Left),
            action_button("tray-list-left-double-click", ClickAction::DoubleLeft),
            action_button("tray-list-right-click", ClickAction::Right),
        ],
        default_button: 0,
    };
    show_list_dialog(parent, spec, Rc::new(crate::close_shell_list))
}

/// Moves the pointer to the center of `rect` and injects the mouse events
/// for `action` in one `SendInput` batch, exactly the systrayList recipe.
fn click(action: ClickAction, rect: Rect) {
    let (x, y) = center_of(rect);
    // SAFETY: moving the pointer to absolute screen coordinates; the call
    // has no memory contract beyond its two integers.
    unsafe {
        let _ = SetCursorPos(x, y);
    }

    let flags: &[MOUSE_EVENT_FLAGS] = match action {
        ClickAction::Left => &[MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP],
        ClickAction::DoubleLeft => &[
            MOUSEEVENTF_LEFTDOWN,
            MOUSEEVENTF_LEFTUP,
            MOUSEEVENTF_LEFTDOWN,
            MOUSEEVENTF_LEFTUP,
        ],
        ClickAction::Right => &[MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP],
    };
    let inputs: Vec<INPUT> = flags
        .iter()
        .map(|&flag| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dwFlags: flag,
                    ..Default::default()
                },
            },
        })
        .collect();
    // SAFETY: `inputs` is a fully initialized, correctly sized array of
    // INPUT structures; SendInput copies from it and does not retain a
    // reference afterward.
    unsafe {
        SendInput(&inputs, i32::try_from(size_of::<INPUT>()).unwrap_or(0));
    }
    tracing::debug!(?action, x, y, "injected a shell item click");
}
