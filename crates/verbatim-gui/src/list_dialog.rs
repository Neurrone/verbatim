//! A reusable list dialog: a title, a static label above a single-selection
//! list box, and a configurable row of buttons plus Cancel.
//!
//! The M3 systrayList replica presents through this component, and M6's
//! elements list will present through the same one. Callers describe the
//! dialog as data (a [`ListDialogSpec`]): items are display strings whose
//! index doubles as the opaque payload key (callers keep their own side
//! table when an item carries more than its string), and each button is a
//! resolved label plus an activation callback returning a
//! [`ButtonVerdict`]. The C++ layer builds and wires the widgets from the
//! model [`ListDialogSpec::split`] produces, so every list-shaped dialog
//! behaves identically: Escape and window close dismiss, Enter or a double
//! click on a list item triggers the default button, and a button
//! activation only ever runs against a selected item. The callbacks stay
//! in Rust, in the [`ListDialogButtons`] half.

use std::rc::Rc;

use verbatim_i18n::messages;

use crate::bridge::ffi;
use crate::plan::{accessible_name, initial_list_selection};

/// What the dialog should do after a button's activation callback ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonVerdict {
    /// Close and destroy the dialog.
    Close,
    /// Keep the dialog open, selection intact.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "every systrayList button closes; M6's elements list will keep its dialog open"
        )
    )]
    KeepOpen,
}

/// A button's activation callback: called with the selected item's index.
pub(crate) type OnActivate = Rc<dyn Fn(usize) -> ButtonVerdict>;

/// One configurable button in the dialog's button row.
pub(crate) struct ListDialogButton {
    /// The button's label, resolved, with its `&` mnemonic.
    pub label: String,
    /// Called with the selected item's index when the button is activated.
    /// Never called without a selection (activation on an empty list is a
    /// no-op), so callers can index their payload table directly.
    pub on_activate: OnActivate,
}

/// Everything a caller specifies about one list dialog.
pub(crate) struct ListDialogSpec {
    /// Window title.
    pub title: String,
    /// The static label above the list; its mnemonic-stripped form becomes
    /// the list box's accessible name.
    pub label: String,
    /// Display strings in list order. The index into this vector is the
    /// opaque payload key handed to button callbacks.
    pub items: Vec<String>,
    /// The configurable buttons, in row order. A Cancel button that
    /// dismisses the dialog is appended after these automatically.
    pub buttons: Vec<ListDialogButton>,
    /// Index into `buttons` of the default button, triggered by Enter or a
    /// double click on a list item (clamped into range).
    pub default_button: usize,
}

/// The Rust half of an open list dialog: its buttons' callbacks.
pub(crate) struct ListDialogButtons {
    callbacks: Vec<OnActivate>,
}

impl ListDialogButtons {
    /// The callback of button `index`, if there is one.
    pub(crate) fn callback(&self, index: usize) -> Option<OnActivate> {
        self.callbacks.get(index).cloned()
    }
}

impl ListDialogSpec {
    /// Splits the spec into the model C++ builds the dialog from and the
    /// callbacks Rust keeps while it is open. The default button is clamped
    /// so a bad spec degrades to the last button instead of a dead Enter
    /// key, and the first item starts selected, as NVDA's systrayList does.
    pub(crate) fn split(self) -> (ffi::ListDialog, ListDialogButtons) {
        let selection = initial_list_selection(self.items.len())
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1);
        let model = ffi::ListDialog {
            title: self.title,
            list_name: accessible_name(&self.label),
            label: self.label,
            items: self.items,
            buttons: self
                .buttons
                .iter()
                .map(|button| button.label.clone())
                .collect(),
            cancel: messages::button_cancel(),
            default_button: self
                .default_button
                .min(self.buttons.len().saturating_sub(1)),
            selection,
        };
        let callbacks = self
            .buttons
            .into_iter()
            .map(|button| button.on_activate)
            .collect();
        (model, ListDialogButtons { callbacks })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn spec(items: &[&str], buttons: usize, default_button: usize) -> ListDialogSpec {
        ListDialogSpec {
            title: "Title".into(),
            label: "&Icons:".into(),
            items: items.iter().map(|item| (*item).to_owned()).collect(),
            buttons: (0..buttons)
                .map(|index| ListDialogButton {
                    label: format!("Button {index}"),
                    on_activate: Rc::new(|_| ButtonVerdict::Close),
                })
                .collect(),
            default_button,
        }
    }

    #[test]
    fn the_list_is_named_by_its_label_and_starts_on_the_first_item() {
        let (model, _) = spec(&["Volume", "Network"], 2, 0).split();
        assert_eq!(model.list_name, "Icons:");
        assert_eq!(model.label, "&Icons:");
        assert_eq!(model.selection, 0);
        assert_eq!(model.buttons, ["Button 0", "Button 1"]);
    }

    #[test]
    fn an_empty_list_selects_nothing() {
        let (model, _) = spec(&[], 1, 0).split();
        assert_eq!(model.selection, -1);
    }

    #[test]
    fn the_default_button_is_clamped_into_range() {
        let (model, _) = spec(&["Volume"], 3, 7).split();
        assert_eq!(model.default_button, 2);
    }

    #[test]
    fn callbacks_stay_with_their_buttons() {
        let pressed = Rc::new(Cell::new(None));
        let mut spec = spec(&["Volume", "Network"], 1, 0);
        let seen = Rc::clone(&pressed);
        spec.buttons.push(ListDialogButton {
            label: "Second".into(),
            on_activate: Rc::new(move |item| {
                seen.set(Some(item));
                ButtonVerdict::KeepOpen
            }),
        });
        let (_, buttons) = spec.split();
        let second = buttons.callback(1).expect("the second button's callback");
        assert_eq!(second(1), ButtonVerdict::KeepOpen);
        assert_eq!(pressed.get(), Some(1));
        assert!(buttons.callback(2).is_none());
    }
}
