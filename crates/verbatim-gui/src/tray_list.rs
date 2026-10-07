//! The system tray and taskbar items dialog, replicating NVDA's systrayList
//! add-on (roadmap, milestone M3): a label over a single-selection list of
//! item names with four buttons — Left Click, Left Double Click, Right
//! Click, and Cancel — where each click action closes the dialog, finds the
//! selected item again, moves the pointer to the center of its screen
//! rectangle, and injects the matching mouse events.
//!
//! Finding the item again is where Verbatim differs from the add-on, which
//! clicks the rectangle recorded when the list was made: icons that moved
//! since then, as tray icons do when one is added or removed, would put a
//! different icon under that point. The shell is enumerated afresh and the
//! item found by its runtime id, or else by a name no other item has
//! ([`refind`]); when it is gone, nothing is clicked and `gone` is called
//! with its name, for Verbatim to say so.
//!
//! Presentation goes through the reusable [`crate::list_dialog`] component;
//! this module only supplies the titles and labels for the two dialog
//! flavors, the payload table of enumerated [`ShellItem`]s, and the click
//! injection. The button callbacks run on the GUI thread; the enumeration
//! makes cross-process calls, so it runs on the worker [`request_shell_items`]
//! starts, and the injection (`SetCursorPos` plus `SendInput`, which work
//! from any thread) is made on the thread that receives its outcome.

use std::mem::size_of;
use std::rc::Rc;
use std::sync::Arc;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

use verbatim_i18n::messages;
use verbatim_model::Rect;

use crate::list_dialog::{ButtonVerdict, ListDialogButton, ListDialogSpec};
use crate::shell_items::{ShellItem, ShellItemKind, center_of, refind, request_shell_items};

/// Called with an item's name when the item chosen is gone by the time it
/// would be clicked. Called on the enumeration's guard thread.
pub(crate) type OnGone = Arc<dyn Fn(String) + Send + Sync>;

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

/// Describes the tray or taskbar list dialog over the enumerated `items`.
///
/// Each click button's callback asks for the dialog to close and has the
/// selected item (by its index into `items`, the list dialog's opaque
/// payload key) found again and clicked ([`click_when_found`]), or `gone`
/// called when it is not there.
pub(crate) fn shell_list_dialog(
    kind: ShellItemKind,
    items: Vec<ShellItem>,
    gone: &OnGone,
) -> ListDialogSpec {
    let (title, label) = match kind {
        ShellItemKind::SystemTray => (messages::tray_list_title(), messages::tray_list_label()),
        ShellItemKind::Taskbar => (
            messages::taskbar_list_title(),
            messages::taskbar_list_label(),
        ),
    };
    let names = items.iter().map(|item| item.name.clone()).collect();
    let items = Rc::new(items);
    let action_button = |label: String, action: ClickAction| ListDialogButton {
        label,
        on_activate: {
            let items = Rc::clone(&items);
            let gone = Arc::clone(gone);
            Rc::new(move |index| {
                if let Some(item) = items.get(index) {
                    click_when_found(kind, item.clone(), action, Arc::clone(&gone));
                }
                ButtonVerdict::Close
            })
        },
    };
    ListDialogSpec {
        title,
        label,
        items: names,
        buttons: vec![
            action_button(messages::tray_list_left_click(), ClickAction::Left),
            action_button(
                messages::tray_list_left_double_click(),
                ClickAction::DoubleLeft,
            ),
            action_button(messages::tray_list_right_click(), ClickAction::Right),
        ],
        default_button: 0,
    }
}

/// Enumerates `kind` afresh on a worker thread and clicks `chosen` where it
/// is now, or calls `gone` with its name when it cannot be found: it was
/// removed, its name no longer tells it apart, or the enumeration failed.
fn click_when_found(kind: ShellItemKind, chosen: ShellItem, action: ClickAction, gone: OnGone) {
    let started = request_shell_items(kind, move |fresh| {
        if let Some(item) = fresh.as_deref().and_then(|fresh| refind(&chosen, fresh)) {
            click(action, item.rect);
        } else {
            tracing::info!(
                name = chosen.name,
                "the chosen shell item is gone; not clicked"
            );
            gone(chosen.name);
        }
    });
    if !started {
        // Logged by `request_shell_items`; nothing is clicked.
        tracing::warn!("the shell item could not be found again; not clicked");
    }
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
