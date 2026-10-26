//! The system tray and taskbar items dialog, replicating NVDA's systrayList
//! add-on (roadmap, milestone M3): a label over a single-selection list of
//! item names with four buttons — Left Click, Left Double Click, Right
//! Click, and Cancel — where each click action closes the dialog, asks
//! where the selected item is now, moves the pointer to the center of its
//! screen rectangle, and injects the matching mouse events.
//!
//! Asking where the item is at the click is where Verbatim differs from the
//! add-on, which clicks the rectangle recorded when the list was made: icons
//! that moved since then, as tray icons do when one is added or removed,
//! would put a different icon under that point. The list carries no
//! positions; the outpost walks the shell again and reads the position of
//! each button with the item's runtime id or name ([`ShellAsk::Position`]),
//! and [`refind`] picks the item from them by its runtime id, or else by a
//! name no other item had when the list was made. When it is gone, or is
//! there with no point to click (off the screen, or with no rectangle or an
//! empty one, which the list cannot see since it reads no rectangles),
//! nothing is clicked and `missed` is called with its name and which of
//! the two it was, for Verbatim to say so.
//!
//! Presentation goes through the reusable [`crate::list_dialog`] component;
//! this module only supplies the titles and labels for the two dialog
//! flavors, the payload table of enumerated [`ShellItem`]s, and the click
//! injection. The button callbacks run on the GUI thread; the position
//! is an outpost's query, asked through the [`ShellItemsHost`], whose
//! outcome arrives on another thread, and the injection (`SetCursorPos` plus `SendInput`, which work
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
use crate::shell_items::{
    ShellAsk, ShellCandidates, ShellItem, ShellItemKind, ShellItemsHost, center_of, refind,
    usable_position,
};

/// Why an item chosen was not clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Missed {
    /// It is not there, its name no longer tells it apart, or the request
    /// failed.
    Gone,
    /// It is there but has no point to click: it has no rectangle or an
    /// empty one, which the list cannot leave out since it reads no
    /// rectangles, or it has gone off the screen since the list, which
    /// leaves off-screen buttons out, was made.
    Unclickable,
}

/// Called with an item's name, and why, when the item chosen is not
/// clicked. Called on Core's reducer thread, which delivers the position
/// query's outcome.
pub(crate) type OnMissed = Arc<dyn Fn(String, Missed) + Send + Sync>;

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
/// payload key) clicked where it is now ([`click_when_found`]), or `missed`
/// called when it is not clicked.
pub(crate) fn shell_list_dialog(
    kind: ShellItemKind,
    items: Vec<ShellItem>,
    host: &Arc<dyn ShellItemsHost>,
    missed: &OnMissed,
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
            let missed = Arc::clone(missed);
            let host = Arc::clone(host);
            Rc::new(move |index| {
                if let Some(item) = items.get(index) {
                    click_when_found(
                        &*host,
                        kind,
                        item.clone(),
                        name_unique(&items, &item.name),
                        action,
                        Arc::clone(&missed),
                    );
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

/// Asks `host` where `chosen`, an item of `kind`, is now and clicks it
/// there, or calls `missed` with its name when it is not clicked
/// ([`target`]). `name_unique` says whether no other item of the list it
/// was chosen from had its name.
fn click_when_found(
    host: &dyn ShellItemsHost,
    kind: ShellItemKind,
    chosen: ShellItem,
    name_unique: bool,
    action: ClickAction,
    missed: OnMissed,
) {
    let asked = chosen.clone();
    let started = host.request(
        kind,
        ShellAsk::Position(
            asked,
            Box::new(
                move |found| match target(&chosen, name_unique, found.as_ref()) {
                    Ok(rect) => click(action, rect),
                    Err(why) => {
                        tracing::info!(
                            name = chosen.name,
                            ?why,
                            "the chosen shell item is not clicked"
                        );
                        missed(chosen.name, why);
                    }
                },
            ),
        ),
    );
    if !started {
        // Logged by the host; nothing is clicked.
        tracing::warn!("the shell item's position could not be asked for; not clicked");
    }
}

/// Whether exactly one item of `items` has the name `name`.
fn name_unique(items: &[ShellItem], name: &str) -> bool {
    items.iter().filter(|other| other.name == name).count() == 1
}

/// Where a click on `chosen` lands, given the outpost's answer `found`
/// (`None` when the request failed): the rectangle of the button
/// [`refind`] picks, or why nothing is clicked.
fn target(
    chosen: &ShellItem,
    name_unique: bool,
    found: Option<&ShellCandidates>,
) -> Result<Rect, Missed> {
    let candidate = found
        .and_then(|found| refind(chosen, name_unique, found))
        .ok_or(Missed::Gone)?;
    usable_position(candidate).ok_or(Missed::Unclickable)
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

#[cfg(test)]
mod tests {
    use verbatim_model::ShellCandidate;

    use super::*;

    fn clock(offscreen: bool, rect: Option<Rect>) -> ShellCandidates {
        ShellCandidates {
            candidates: vec![ShellCandidate {
                item: ShellItem {
                    name: "Clock".into(),
                    runtime_id: vec![42, 1],
                },
                offscreen,
                rect,
            }],
            complete: true,
        }
    }

    fn chosen() -> ShellItem {
        ShellItem {
            name: "Clock".into(),
            runtime_id: vec![42, 1],
        }
    }

    const AT: Rect = Rect {
        left: 1217,
        top: 752,
        width: 24,
        height: 48,
    };

    #[test]
    fn a_found_item_with_a_rectangle_is_clicked_there() {
        assert_eq!(
            target(&chosen(), true, Some(&clock(false, Some(AT)))),
            Ok(AT)
        );
    }

    #[test]
    fn a_found_item_with_no_point_to_click_is_unclickable_not_gone() {
        assert_eq!(
            target(&chosen(), true, Some(&clock(false, None))),
            Err(Missed::Unclickable)
        );
        assert_eq!(
            target(&chosen(), true, Some(&clock(true, Some(AT)))),
            Err(Missed::Unclickable)
        );
    }

    #[test]
    fn a_name_is_unique_only_when_one_item_has_it() {
        let item = |name: &str, id: i32| ShellItem {
            name: name.into(),
            runtime_id: vec![42, id],
        };
        let items = [item("Volume", 1), item("Clock", 2), item("Volume", 3)];
        assert!(name_unique(&items, "Clock"));
        assert!(!name_unique(&items, "Volume"));
    }

    #[test]
    fn a_missing_item_or_a_failed_request_is_gone() {
        let none = ShellCandidates {
            candidates: Vec::new(),
            complete: true,
        };
        assert_eq!(target(&chosen(), true, Some(&none)), Err(Missed::Gone));
        assert_eq!(target(&chosen(), true, None), Err(Missed::Gone));
    }
}
