//! The boundary between Rust and the C++ widget layer (`cpp/gui.cpp`),
//! through cxx.
//!
//! The boundary is coarse and message-shaped. Rust calls C++ to run the
//! event loop, wake it, pop the menu, open, raise, and focus a dialog, show
//! and hide the hidden frame, and shut down. C++ calls methods of the
//! opaque [`GuiCore`](crate::GuiCore) when the user acts: a menu choice, a
//! setting change, a dialog button, a dialog closing. Pages are described
//! by the shared structs below, which Rust fills and C++ only reads.
//!
//! Every string crosses already resolved: Rust looks it up through the
//! typed message functions in `verbatim-i18n`, so C++ never sees a Fluent
//! message id. Text crosses as UTF-8 and C++ converts it explicitly.
//!
//! Reentrancy: menus and modal dialogs run nested event loops, inside
//! which C++ calls back into `GuiCore`. Every `GuiCore` method therefore
//! takes `&self`, and no borrow of its interior state is held across a
//! call into C++.

use crate::{GuiCore, next_category, route_settings_key};

#[allow(
    clippy::multiple_unsafe_ops_per_block,
    reason = "cxx generates the shims' unsafe blocks; there is no hand-written unsafe code here"
)]
#[cxx::bridge(namespace = "verbatim_gui")]
pub(crate) mod ffi {
    /// The strings the hidden frame, tray icon, and menu show.
    struct ShellText {
        /// The hidden frame's title: the single-instance rendezvous.
        title: String,
        /// The tray icon's tooltip.
        tooltip: String,
        /// The menu's Settings item, with its mnemonic.
        settings_item: String,
        /// The menu's Exit item, with its mnemonic.
        exit_item: String,
    }

    /// An item of the Verbatim menu.
    #[derive(Debug)]
    enum MenuChoice {
        /// Settings: open the settings dialog.
        Settings,
        /// Exit: ask the app to shut down.
        Exit,
    }

    /// One of the GUI's modeless dialogs.
    #[derive(Debug)]
    #[repr(u8)]
    enum DialogKind {
        /// The settings dialog.
        Settings,
        /// The system tray or taskbar item list.
        ShellList,
    }

    /// A point on the screen, in pixels.
    #[derive(Debug)]
    struct ScreenPoint {
        /// Horizontal position.
        x: i32,
        /// Vertical position.
        y: i32,
    }

    /// The settings dialog's frame: its categories and buttons.
    struct SettingsDialog {
        /// The categories, in list order; the first is shown on opening.
        categories: Vec<Category>,
        /// The label above the category list, with its mnemonic.
        categories_label: String,
        /// The OK button's label.
        ok: String,
        /// The Cancel button's label.
        cancel: String,
        /// The Apply button's label.
        apply: String,
    }

    /// One settings category.
    struct Category {
        /// The name shown in the category list.
        name: String,
        /// The dialog's title while this category is shown.
        title: String,
        /// Which page the category shows.
        kind: CategoryKind,
    }

    /// The pages a settings category can show.
    #[derive(Debug)]
    enum CategoryKind {
        /// The Speech page: the synthesizer and its settings.
        Speech,
    }

    /// The Speech page: the synthesizer group and the controls generated
    /// from the settings host's descriptors.
    struct SpeechPage {
        /// Identifies this set of controls in the changes C++ reports, so a
        /// change from a replaced set is ignored.
        generation: u32,
        /// The synthesizer group's label.
        synthesizer_group: String,
        /// The current synthesizer's name, shown in a read-only field.
        synthesizer_name: String,
        /// The read-only field's accessible name: it has no label of its
        /// own, the group names it.
        synthesizer_field_name: String,
        /// The Change button's label, with its mnemonic.
        change: String,
        /// The generated controls, in order.
        controls: Vec<SettingControl>,
    }

    /// The kind of a generated setting control.
    #[derive(Debug)]
    enum ControlKind {
        /// A slider with a label above it.
        Slider,
        /// A combo box with a label above it.
        Choice,
        /// A check box.
        Toggle,
    }

    /// One generated setting control. Which fields apply depends on
    /// `kind`; the others are zero or empty.
    struct SettingControl {
        /// The kind of control.
        kind: ControlKind,
        /// The label, with its mnemonic.
        label: String,
        /// The accessible name, for a check box, which has no separate
        /// label to be named by.
        name: String,
        /// A slider's minimum.
        min: i32,
        /// A slider's maximum.
        max: i32,
        /// A slider's value.
        value: i32,
        /// A slider's arrow-key step.
        line_size: i32,
        /// A slider's Page Up and Page Down step.
        page_size: i32,
        /// A combo box's options, as shown.
        options: Vec<String>,
        /// A combo box's selected option, or -1 for none.
        selection: i32,
        /// A check box's state.
        checked: bool,
    }

    /// The Select Synthesizer dialog.
    struct SynthesizerPicker {
        /// The dialog's title.
        title: String,
        /// The label above the synthesizer list, with its mnemonic.
        label: String,
        /// The OK button's label.
        ok: String,
        /// The Cancel button's label.
        cancel: String,
        /// The synthesizers' names, in order.
        names: Vec<String>,
        /// The index of the active synthesizer.
        active: usize,
    }

    /// A list dialog: a label over a single-selection list, a row of
    /// buttons, and Cancel.
    struct ListDialog {
        /// The dialog's title.
        title: String,
        /// The label above the list, with its mnemonic.
        label: String,
        /// The list's accessible name: the label without its mnemonic.
        list_name: String,
        /// The list's items, in order.
        items: Vec<String>,
        /// The buttons' labels, in row order; Cancel follows them.
        buttons: Vec<String>,
        /// The Cancel button's label.
        cancel: String,
        /// The button Enter or a double click on an item activates.
        default_button: usize,
        /// The item selected on opening, or -1 for none.
        selection: i32,
    }

    /// A key the settings dialog routes.
    #[derive(Debug)]
    enum SettingsKey {
        /// Enter on the main keyboard or the numeric keypad.
        Enter,
        /// Tab.
        Tab,
        /// The S key.
        S,
        /// Any other key.
        Other,
    }

    /// The settings dialog's control that has focus.
    #[derive(Debug)]
    enum SettingsFocus {
        /// The OK button.
        Ok,
        /// The Cancel button.
        Cancel,
        /// The Apply button.
        Apply,
        /// The synthesizer group's Change button.
        ChangeSynthesizer,
        /// The read-only field naming the synthesizer.
        SynthesizerName,
        /// Any other control.
        Other,
    }

    /// What the settings dialog does with a key.
    #[derive(Debug)]
    enum SettingsKeyAction {
        /// Leave the key to the focused control.
        PassThrough,
        /// Show the next category.
        NextCategory,
        /// Show the previous category.
        PreviousCategory,
        /// Activate OK.
        Ok,
        /// Activate Cancel.
        Cancel,
        /// Activate Apply.
        Apply,
        /// Activate the Change button.
        ChangeSynthesizer,
    }

    extern "Rust" {
        /// The GUI's Rust half, which C++ calls when the user acts.
        type GuiCore;

        /// The frame, tray icon, and menu exist.
        fn ready(self: &GuiCore);
        /// Acts on every message waiting in the GUI's channel.
        fn drain(self: &GuiCore);
        /// The tray icon was clicked with the left button.
        fn tray_clicked(self: &GuiCore);
        /// A menu item was chosen.
        fn menu_chosen(self: &GuiCore, choice: MenuChoice);
        /// A dialog closed and was destroyed.
        fn dialog_closed(self: &GuiCore, dialog: DialogKind);

        /// The Speech page, built afresh from the settings host.
        fn speech_page(self: &GuiCore) -> SpeechPage;
        /// A slider moved.
        fn slider_changed(self: &GuiCore, generation: u32, control: usize, value: i32);
        /// A combo box selection changed.
        fn choice_changed(self: &GuiCore, generation: u32, control: usize, option: usize);
        /// A check box was toggled.
        fn toggle_changed(self: &GuiCore, generation: u32, control: usize, checked: bool);
        /// OK or Apply: keep the live changes.
        fn commit_settings(self: &GuiCore);
        /// Cancel: undo the live changes.
        fn revert_settings(self: &GuiCore);
        /// The Select Synthesizer dialog's contents.
        fn synthesizer_picker(self: &GuiCore) -> SynthesizerPicker;
        /// The user chose a synthesizer; true when the active one changed,
        /// so the Speech page's controls must be rebuilt.
        fn choose_synthesizer(self: &GuiCore, index: usize) -> bool;

        /// A list dialog button was activated with an item selected; true
        /// when the dialog should close.
        fn list_button(self: &GuiCore, button: usize, item: usize) -> bool;

        /// What a key does in the settings dialog.
        fn route_settings_key(
            key: SettingsKey,
            control: bool,
            shift: bool,
            focus: SettingsFocus,
        ) -> SettingsKeyAction;
        /// The category after `current` among `count`, wrapping, or the one
        /// before it when `forward` is false.
        fn next_category(current: usize, count: usize, forward: bool) -> usize;
    }

    // SAFETY: every function here except `wake_event_loop` reads the C++
    // side's unsynchronized shell state and drives wxWidgets, so it is
    // called only on the GUI thread: `run_event_loop` from `run_gui`, which
    // runs once per process, and the rest from `GuiCore` methods while that
    // loop runs. This module is private to the crate, and `GuiCore` is not
    // `Sync`, so no other thread can reach a `GuiCore` to call them from.
    unsafe extern "C++" {
        include!("verbatim-gui/cpp/gui.h");

        /// Runs wxWidgets' event loop on this thread until it ends,
        /// calling back into `core` meanwhile. Returns the loop's exit
        /// code, or -1 when wxWidgets could not start.
        fn run_event_loop(core: &GuiCore, text: &ShellText) -> i32;
        /// Asks the event loop to call `GuiCore::drain`. Callable from any
        /// thread; does nothing when the loop is not running.
        fn wake_event_loop();

        /// The hidden frame's native window handle.
        fn frame_handle() -> usize;
        /// Centres the hidden frame on the screen and returns its position.
        fn centre_frame() -> ScreenPoint;
        /// Shows and raises the hidden frame.
        fn show_frame();
        /// Hides the hidden frame.
        fn hide_frame();
        /// Pops the Verbatim menu from the hidden frame at `at`, running a
        /// nested loop until it closes. True when it was shown.
        fn popup_menu(at: ScreenPoint) -> bool;

        /// Builds and shows the settings dialog.
        fn open_settings_dialog(dialog: &SettingsDialog);
        /// Builds and shows a list dialog.
        fn open_list_dialog(dialog: &ListDialog);
        /// A dialog's native window handle, or 0 when it is not open.
        fn dialog_handle(dialog: DialogKind) -> usize;
        /// Raises a dialog above other windows.
        fn raise_dialog(dialog: DialogKind);
        /// Gives a dialog the keyboard focus.
        fn focus_dialog(dialog: DialogKind);

        /// Removes the tray icon, destroys the dialogs and the frame, and
        /// ends the event loop.
        fn shut_down();
    }
}
