//! A reusable list dialog: a title, a static label above a single-selection
//! list box, and a configurable row of buttons plus Cancel.
//!
//! The M3 systrayList replica presents through this component, and M6's
//! elements list will present through the same one. Callers describe the
//! dialog as data (a [`ListDialogSpec`]): items are display strings whose
//! index doubles as the opaque payload key (callers keep their own side
//! table when an item carries more than its string), and each button is a
//! Fluent label message id plus an activation callback returning a
//! [`ButtonVerdict`]. This module builds and wires the widgets once, so
//! every list-shaped dialog behaves identically: Escape and window close
//! dismiss, Enter or a double click on a list item triggers the default
//! button, and a button activation only ever runs against a selected item.
//!
//! Everything here runs on the GUI thread, like all widget code in this
//! crate: [`show_list_dialog`] must only be called from GUI-thread dispatch
//! (a [`GuiCommand`](crate::GuiCommand) handler or another widget event).

use std::rc::Rc;

use wxdragon::prelude::*;

use verbatim_i18n::messages;

use crate::plan::{accessible_name, initial_list_selection};

/// Virtual-key code for Enter in wxWidgets key-down events, matching the
/// convention in the settings dialog (wxdragon exposes no key constants).
const KEY_ENTER: i32 = 13;

/// The list box size the component standardizes on, per the M3 design.
const LIST_SIZE: Size = Size {
    width: 550,
    height: 250,
};

/// What the dialog should do after a button's activation callback ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonVerdict {
    /// Close and destroy the dialog (through the caller's close hook).
    Close,
    /// Keep the dialog open, selection intact.
    KeepOpen,
}

/// One configurable button in the dialog's button row.
pub struct ListDialogButton {
    /// Fluent message id for the button label, resolved through
    /// [`verbatim_i18n::message`]; the label may carry an `&` mnemonic.
    pub label_key: String,
    /// Called with the selected item's index when the button is activated.
    /// Never called without a selection (activation on an empty list is a
    /// no-op), so callers can index their payload table directly.
    pub on_activate: Rc<dyn Fn(usize) -> ButtonVerdict>,
}

/// Everything a caller specifies about one list dialog.
pub struct ListDialogSpec {
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

/// Builds the list dialog described by `spec` and returns its handle; the
/// caller shows it and owns any singleton bookkeeping.
///
/// `on_close` is the single dismissal path — Cancel, Escape, window close,
/// and any button whose callback returns [`ButtonVerdict::Close`] all route
/// through it — so the caller can clear its dialog slot and destroy the
/// window in one place.
///
/// GUI thread only, like every widget constructor in this crate.
pub fn show_list_dialog(
    parent: Frame,
    spec: ListDialogSpec,
    on_close: Rc<dyn Fn(Dialog)>,
) -> Dialog {
    let dialog = Dialog::builder(&parent, &spec.title)
        .with_style(DialogStyle::DefaultDialogStyle)
        .build();

    let outer = BoxSizer::builder(Orientation::Vertical).build();
    let label = StaticText::builder(&dialog).with_label(&spec.label).build();
    let list = ListBox::builder(&dialog)
        .with_style(ListBoxStyle::Default)
        .with_size(LIST_SIZE)
        .build();
    // The label control does not name the list on its own; without an
    // explicit accessible name the list announces as a bare "list".
    list.set_name(&accessible_name(&spec.label));
    for item in &spec.items {
        list.append(item);
    }
    if let Some(index) = initial_list_selection(spec.items.len()) {
        list.set_selection(u32::try_from(index).unwrap_or(0), true);
    }
    outer.add(&label, 0, SizerFlag::Left | SizerFlag::All, 8);
    outer.add(&list, 1, SizerFlag::Expand | SizerFlag::All, 8);

    let row = BoxSizer::builder(Orientation::Horizontal).build();
    let mut callbacks: Vec<Rc<dyn Fn(usize) -> ButtonVerdict>> =
        Vec::with_capacity(spec.buttons.len());
    let mut widgets = Vec::with_capacity(spec.buttons.len());
    for button in spec.buttons {
        let widget = Button::builder(&dialog)
            .with_label(&verbatim_i18n::message(&button.label_key))
            .build();
        row.add(&widget, 0, SizerFlag::All, 5);
        widgets.push(widget);
        callbacks.push(button.on_activate);
    }
    let cancel = Button::builder(&dialog)
        .with_id(ID_CANCEL)
        .with_label(&messages::button_cancel())
        .build();
    row.add(&cancel, 0, SizerFlag::All, 5);
    outer.add_sizer(&row, 0, SizerFlag::AlignRight | SizerFlag::All, 5);

    // Activation runs the chosen button's callback against the current
    // selection; with no selection (an empty list) it does nothing. The
    // default button index is clamped so a bad spec degrades to the last
    // button instead of a dead Enter key.
    let default_index = spec.default_button.min(callbacks.len().saturating_sub(1));
    let activate: Rc<dyn Fn(usize)> = {
        let on_close = on_close.clone();
        Rc::new(move |button_index: usize| {
            let Some(callback) = callbacks.get(button_index) else {
                return;
            };
            let Some(selection) = list.get_selection() else {
                return;
            };
            let Ok(selection) = usize::try_from(selection) else {
                return;
            };
            if callback(selection) == ButtonVerdict::Close {
                on_close(dialog);
            }
        })
    };
    for (index, widget) in widgets.iter().enumerate() {
        let activate = activate.clone();
        widget.on_click(move |_| activate(index));
    }
    cancel.on_click(move |_| on_close(dialog));
    {
        let activate = activate.clone();
        list.on_item_double_clicked(move |_| activate(default_index));
    }
    {
        let activate = activate.clone();
        list.on_key_down(move |event| {
            if let WindowEventData::Keyboard(key) = &event
                && key.get_key_code() == Some(KEY_ENTER)
            {
                activate(default_index);
                return;
            }
            event.skip(true);
        });
    }
    if let Some(widget) = widgets.get(default_index) {
        widget.set_default();
    }
    // Escape triggers the Cancel button's click path; window close follows
    // the same route through wxWidgets' default dialog close handling.
    dialog.set_escape_id(ID_CANCEL);

    dialog.set_sizer(outer, true);
    dialog.fit();
    dialog.centre();
    list.set_focus();
    dialog
}
