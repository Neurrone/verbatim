//! The settings dialog, mirroring NVDA's `MultiCategorySettingsDialog`.
//!
//! Structure: a category list on the left swaps the active category panel on
//! the right; OK, Cancel, and Apply sit along the bottom. M1 ships a single
//! category (Speech), but the swap is written generically over a list of
//! category descriptors with lazy panel constructors, so adding categories is
//! only a matter of pushing more descriptors.
//!
//! Everything here runs on the GUI thread. Widgets are `Copy` handles, so
//! event closures capture them directly; the only shared mutable state is the
//! small `Rc<Cell<_>>`/`Rc<RefCell<_>>` bookkeeping for the active category.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use wxdragon::prelude::*;

use verbatim_i18n::messages;
use verbatim_speech::{SettingValue, SpeechSettingsHost};

use crate::plan::{ControlPlan, accessible_name, cycle_index, plan_for};

/// Virtual-key codes reported by wxWidgets key-down events. wxWidgets does not
/// expose these as constants through wxdragon, so we name the few we route.
const KEY_TAB: i32 = 9;
const KEY_ENTER: i32 = 13;
const KEY_S: i32 = 83;

/// One settings category: its display name and a constructor that builds its
/// panel on first activation (lazy, matching NVDA's per-category panels).
struct Category {
    name: String,
    panel: Option<Panel>,
    make: Rc<dyn Fn(Panel) -> Panel>,
}

type Categories = Rc<RefCell<Vec<Category>>>;
type Activator = Rc<dyn Fn(usize, bool)>;

/// Builds the modeless settings dialog and returns its handle.
///
/// The caller (the command dispatcher in [`crate`]) owns the singleton guard
/// and stores this handle; every button and key path routes back through
/// [`crate::close_settings`] so the guard and the window stay in step.
pub(crate) fn build_settings_dialog(parent: Frame, host: &Arc<dyn SpeechSettingsHost>) -> Dialog {
    let speech_name = messages::settings_category_speech();
    let dialog = Dialog::builder(
        &parent,
        &messages::settings_title_with_category(&speech_name),
    )
    .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
    .with_size(800, 480)
    .build();
    dialog.set_min_size(Size {
        width: 520,
        height: 360,
    });

    // The category descriptors, each with a lazy panel constructor so a
    // category costs nothing until it is first shown.
    let categories: Categories = Rc::new(RefCell::new(vec![Category {
        name: speech_name,
        panel: None,
        make: {
            let host = host.clone();
            Rc::new(move |container: Panel| build_speech_panel(container, &host, dialog))
        },
    }]));
    let category_count = categories.borrow().len();
    let selected = Rc::new(Cell::new(0usize));

    let outer = BoxSizer::builder(Orientation::Vertical).build();
    let content = BoxSizer::builder(Orientation::Horizontal).build();
    let list = build_category_list(dialog, &categories, content);

    let container = Panel::builder(&dialog).build();
    let container_sizer = BoxSizer::builder(Orientation::Vertical).build();
    container.set_sizer(container_sizer, true);
    content.add(&container, 1, SizerFlag::Expand | SizerFlag::All, 5);
    outer.add_sizer(&content, 1, SizerFlag::Expand | SizerFlag::All, 0);

    let buttons = build_button_row(dialog, host, outer);

    dialog.set_sizer(outer, true);
    dialog.centre();

    let activate = make_activator(
        dialog,
        list,
        container,
        container_sizer,
        categories,
        selected.clone(),
    );

    {
        let activate = activate.clone();
        list.on_item_selected(move |event| {
            if let Ok(index) = usize::try_from(event.get_item_index()) {
                activate(index, false);
            }
        });
    }

    install_all_shortcuts(
        dialog,
        list,
        buttons,
        &activate,
        &selected,
        category_count,
        host,
    );

    // Show the first category.
    activate(0, true);
    list.set_focus();

    dialog
}

/// Builds the left-hand category list (label above a single-column, header-less,
/// single-select list) and adds it to `content`.
fn build_category_list(dialog: Dialog, categories: &Categories, content: BoxSizer) -> ListCtrl {
    let left = BoxSizer::builder(Orientation::Vertical).build();
    let categories_label = StaticText::builder(&dialog)
        .with_label(&messages::settings_categories_label())
        .build();
    let list = ListCtrl::builder(&dialog)
        .with_style(ListCtrlStyle::Report | ListCtrlStyle::SingleSel | ListCtrlStyle::NoHeader)
        .build();
    list.insert_column(0, "", ListColumnFormat::Left, 200);
    for (index, category) in categories.borrow().iter().enumerate() {
        list.insert_item(i64::try_from(index).unwrap_or(0), &category.name, None);
    }
    left.add(&categories_label, 0, SizerFlag::All, 5);
    left.add(&list, 1, SizerFlag::Expand | SizerFlag::All, 5);
    content.add_sizer(&left, 0, SizerFlag::Expand | SizerFlag::All, 0);
    list
}

/// Builds and wires the OK / Cancel / Apply row, HIG-ordered by the sizer.
///
/// Settings apply live as they change, so OK and Apply both persist via
/// `commit`; Cancel reverts the uncommitted live changes. Returns the buttons
/// so the caller can route keyboard shortcuts through them.
fn build_button_row(
    dialog: Dialog,
    host: &Arc<dyn SpeechSettingsHost>,
    outer: BoxSizer,
) -> (Button, Button, Button) {
    let buttons = StdDialogButtonSizerBuilder::new().build();
    let ok = Button::builder(&dialog)
        .with_id(ID_OK)
        .with_label(&messages::button_ok())
        .build();
    let cancel = Button::builder(&dialog)
        .with_id(ID_CANCEL)
        .with_label(&messages::button_cancel())
        .build();
    let apply = Button::builder(&dialog)
        .with_id(ID_APPLY)
        .with_label(&messages::button_apply())
        .build();
    buttons.add_button(&ok);
    buttons.add_button(&cancel);
    buttons.add_button(&apply);
    buttons.realize();
    outer.add_sizer(&buttons, 0, SizerFlag::Expand | SizerFlag::All, 5);

    {
        let host = host.clone();
        ok.on_click(move |_| {
            let _ = host.commit();
            crate::close_settings(dialog);
        });
    }
    {
        let host = host.clone();
        cancel.on_click(move |_| {
            host.revert();
            crate::close_settings(dialog);
        });
    }
    {
        let host = host.clone();
        apply.on_click(move |_| {
            let _ = host.commit();
        });
    }
    ok.set_default();
    dialog.set_affirmative_id(ID_OK);
    dialog.set_escape_id(ID_CANCEL);

    (ok, cancel, apply)
}

/// Returns the closure that activates a category: build its panel lazily, show
/// it, hide the rest, and keep the list selection and dialog title in step.
///
/// `update_list` is false when the list itself drove the change (so we do not
/// re-fire its selection event).
fn make_activator(
    dialog: Dialog,
    list: ListCtrl,
    container: Panel,
    container_sizer: BoxSizer,
    categories: Categories,
    selected: Rc<Cell<usize>>,
) -> Activator {
    Rc::new(move |index: usize, update_list: bool| {
        let mut cats = categories.borrow_mut();
        if index >= cats.len() {
            return;
        }
        if cats[index].panel.is_none() {
            let panel = (cats[index].make)(container);
            container_sizer.add(&panel, 1, SizerFlag::Expand | SizerFlag::All, 0);
            cats[index].panel = Some(panel);
        }
        for (i, category) in cats.iter().enumerate() {
            if let Some(panel) = category.panel {
                panel.show(i == index);
            }
        }
        selected.set(index);
        let title = messages::settings_title_with_category(&cats[index].name);
        drop(cats);
        if update_list {
            list.set_item_state(
                i64::try_from(index).unwrap_or(0),
                ListItemState::Selected | ListItemState::Focused,
                ListItemState::Selected | ListItemState::Focused,
            );
        }
        // Dialog has no dedicated SetTitle in wxdragon; SetLabel maps to the
        // window caption on Windows.
        dialog.set_label(&title);
        container.layout();
        dialog.layout();
    })
}

/// Binds the dialog-wide keyboard shortcuts to the widgets that commonly hold
/// focus. wxdragon has no accelerator tables and key events do not bubble from
/// focused controls, so we bind the containers and buttons individually. The
/// category `ListCtrl` cannot host the handler (it does not implement
/// `WindowEvents`); its arrow-key navigation is native, and Ctrl+Tab is
/// reachable from the surrounding widgets.
fn install_all_shortcuts(
    dialog: Dialog,
    list: ListCtrl,
    buttons: (Button, Button, Button),
    activate: &Activator,
    selected: &Rc<Cell<usize>>,
    category_count: usize,
    host: &Arc<dyn SpeechSettingsHost>,
) {
    let (ok, cancel, apply) = buttons;
    bind_shortcuts(
        &dialog,
        dialog,
        list,
        activate.clone(),
        selected.clone(),
        category_count,
        host.clone(),
    );
    bind_shortcuts(
        &ok,
        dialog,
        list,
        activate.clone(),
        selected.clone(),
        category_count,
        host.clone(),
    );
    bind_shortcuts(
        &cancel,
        dialog,
        list,
        activate.clone(),
        selected.clone(),
        category_count,
        host.clone(),
    );
    bind_shortcuts(
        &apply,
        dialog,
        list,
        activate.clone(),
        selected.clone(),
        category_count,
        host.clone(),
    );
}

/// Installs the dialog-wide shortcut key-down handler on one widget.
#[allow(clippy::too_many_arguments)]
fn bind_shortcuts<W: WindowEvents>(
    widget: &W,
    dialog: Dialog,
    list: ListCtrl,
    activate: Activator,
    selected: Rc<Cell<usize>>,
    category_count: usize,
    host: Arc<dyn SpeechSettingsHost>,
) {
    widget.on_key_down(move |event| {
        if let WindowEventData::Keyboard(key) = &event {
            let code = key.get_key_code().unwrap_or(0);
            if key.control_down() {
                match code {
                    KEY_TAB => {
                        let next = cycle_index(selected.get(), category_count, !key.shift_down());
                        activate(next, true);
                        list.set_focus();
                        return;
                    }
                    KEY_S => {
                        let _ = host.commit();
                        return;
                    }
                    _ => {}
                }
            } else if code == KEY_ENTER {
                // Enter anywhere not consumed by a control activates OK.
                let _ = host.commit();
                crate::close_settings(dialog);
                return;
            }
        }
        event.skip(true);
    });
}

/// Builds the Speech category panel: the synthesizer group plus the
/// driver-generated controls below it.
fn build_speech_panel(
    container: Panel,
    host: &Arc<dyn SpeechSettingsHost>,
    dialog: Dialog,
) -> Panel {
    let panel = Panel::builder(&container).build();
    let vsizer = BoxSizer::builder(Orientation::Vertical).build();

    // Synthesizer group: a read-only name field plus a Change... button.
    let group = StaticBoxSizerBuilder::new_with_label(
        Orientation::Vertical,
        &panel,
        &messages::speech_synthesizer_group(),
    )
    .build();
    let row = BoxSizer::builder(Orientation::Horizontal).build();
    let name = TextCtrl::builder(&panel)
        .with_style(TextCtrlStyle::ReadOnly)
        .with_value(&host.active_synthesizer().display_name)
        .build();
    // The field has no label control of its own — the group box names it — so
    // give it that name explicitly or it is announced as a bare "edit".
    name.set_name(&accessible_name(&messages::speech_synthesizer_group()));
    let change = Button::builder(&panel)
        .with_label(&messages::speech_change_synth())
        .build();
    row.add(&name, 1, SizerFlag::Expand | SizerFlag::All, 5);
    row.add(&change, 0, SizerFlag::All, 5);
    group.add_sizer(&row, 0, SizerFlag::Expand | SizerFlag::All, 5);
    vsizer.add_sizer(&group, 0, SizerFlag::Expand | SizerFlag::All, 5);

    // The driver-generated controls live in their own sub-panel so switching
    // synthesizers can destroy and regenerate them wholesale.
    let dynamic = build_dynamic_controls(panel, host);
    vsizer.add(&dynamic, 1, SizerFlag::Expand | SizerFlag::All, 5);
    let dynamic_cell = Rc::new(RefCell::new(dynamic));

    panel.set_sizer(vsizer, true);

    // Opening the Select Synthesizer dialog, then rebuilding on success. Shared
    // by the Change... button and Enter on the read-only name field.
    let rebuild: Rc<dyn Fn()> = {
        let host = host.clone();
        Rc::new(move || {
            if change_synthesizer(dialog, &host) {
                let old = *dynamic_cell.borrow();
                old.destroy();
                let fresh = build_dynamic_controls(panel, &host);
                vsizer.add(&fresh, 1, SizerFlag::Expand | SizerFlag::All, 5);
                *dynamic_cell.borrow_mut() = fresh;
                name.set_value(&host.active_synthesizer().display_name);
                panel.layout();
            }
        })
    };

    {
        let rebuild = rebuild.clone();
        change.on_click(move |_| rebuild());
    }
    {
        let rebuild = rebuild.clone();
        name.on_key_down(move |event| {
            if let WindowEventData::Keyboard(key) = &event
                && key.get_key_code() == Some(KEY_ENTER)
            {
                rebuild();
                return;
            }
            event.skip(true);
        });
    }

    panel
}

/// Builds a fresh sub-panel of the driver-generated controls, wiring each to
/// apply its value live via the host.
fn build_dynamic_controls(parent: Panel, host: &Arc<dyn SpeechSettingsHost>) -> Panel {
    let panel = Panel::builder(&parent).build();
    let sizer = BoxSizer::builder(Orientation::Vertical).build();

    for descriptor in host.setting_descriptors() {
        let id = descriptor.id().clone();
        let label = verbatim_i18n::message(descriptor.label_key());
        match plan_for(&descriptor, host.setting(&id)) {
            ControlPlan::Slider {
                min, max, initial, ..
            } => {
                let text = StaticText::builder(&panel).with_label(&label).build();
                let slider = Slider::builder(&panel)
                    .with_min_value(min)
                    .with_max_value(max)
                    .with_value(initial)
                    .build();
                sizer.add(&text, 0, SizerFlag::Left | SizerFlag::Top, 3);
                sizer.add(&slider, 0, SizerFlag::Expand | SizerFlag::All, 3);
                let host = host.clone();
                let id = id.clone();
                slider.on_slider(move |event| {
                    let _ = host.set_setting(&id, SettingValue::Number(event.get_value()));
                });
            }
            ControlPlan::Choice {
                options, selected, ..
            } => {
                let text = StaticText::builder(&panel).with_label(&label).build();
                let choice = Choice::builder(&panel).build();
                for (_, display) in &options {
                    choice.append(display);
                }
                if let Some(index) = selected {
                    choice.set_selection(u32::try_from(index).unwrap_or(0));
                }
                sizer.add(&text, 0, SizerFlag::Left | SizerFlag::Top, 3);
                sizer.add(&choice, 0, SizerFlag::Expand | SizerFlag::All, 3);
                let option_ids: Vec<String> = options
                    .into_iter()
                    .map(|(option_id, _)| option_id)
                    .collect();
                let host = host.clone();
                let id = id.clone();
                choice.on_selection_changed(move |_| {
                    if let Some(index) = choice.get_selection()
                        && let Some(option_id) = option_ids.get(index as usize)
                    {
                        let _ = host.set_setting(&id, SettingValue::Choice(option_id.clone()));
                    }
                });
            }
            ControlPlan::Toggle { initial, .. } => {
                let check = CheckBox::builder(&panel)
                    .with_label(&label)
                    .with_value(initial)
                    .build();
                // A check box carries its own label, so it gets no StaticText to
                // borrow a name from; without this it falls back to wxWidgets'
                // default window name and is announced as "check".
                check.set_name(&accessible_name(&label));
                sizer.add(&check, 0, SizerFlag::All, 3);
                let host = host.clone();
                let id = id.clone();
                check.on_toggled(move |event| {
                    let _ = host.set_setting(&id, SettingValue::Toggle(event.is_checked()));
                });
            }
        }
    }

    panel.set_sizer(sizer, true);
    panel
}

/// Runs the modal Select Synthesizer dialog. Returns `true` when the active
/// synthesizer actually changed, so the caller knows to rebuild its controls.
fn change_synthesizer(parent: Dialog, host: &Arc<dyn SpeechSettingsHost>) -> bool {
    let synthesizers = host.synthesizers();
    let active = host.active_synthesizer();

    let dialog = Dialog::builder(&parent, &messages::select_synth_title())
        .with_style(DialogStyle::DefaultDialogStyle)
        .build();
    let outer = BoxSizer::builder(Orientation::Vertical).build();
    let label = StaticText::builder(&dialog)
        .with_label(&messages::select_synth_label())
        .build();
    let choice = Choice::builder(&dialog).build();
    let mut active_index = 0u32;
    for (index, synth) in synthesizers.iter().enumerate() {
        choice.append(&synth.display_name);
        if synth.id == active.id {
            active_index = u32::try_from(index).unwrap_or(0);
        }
    }
    choice.set_selection(active_index);
    outer.add(&label, 0, SizerFlag::All, 8);
    outer.add(&choice, 0, SizerFlag::Expand | SizerFlag::All, 8);

    let buttons = StdDialogButtonSizerBuilder::new().build();
    let ok = Button::builder(&dialog)
        .with_id(ID_OK)
        .with_label(&messages::button_ok())
        .build();
    let cancel = Button::builder(&dialog)
        .with_id(ID_CANCEL)
        .with_label(&messages::button_cancel())
        .build();
    buttons.add_button(&ok);
    buttons.add_button(&cancel);
    buttons.realize();
    outer.add_sizer(&buttons, 0, SizerFlag::Expand | SizerFlag::All, 8);

    dialog.set_sizer(outer, true);
    dialog.set_affirmative_id(ID_OK);
    dialog.set_escape_id(ID_CANCEL);
    ok.set_default();
    dialog.fit();
    dialog.centre();

    let changed = if dialog.show_modal() == ID_OK {
        match choice
            .get_selection()
            .and_then(|i| synthesizers.get(i as usize))
        {
            Some(chosen) if chosen.id != active.id => match host.set_active_synthesizer(&chosen.id)
            {
                Ok(()) => true,
                Err(error) => {
                    tracing::warn!(%error, "failed to switch synthesizer; keeping the current one");
                    false
                }
            },
            _ => false,
        }
    } else {
        false
    };
    dialog.destroy();
    changed
}
