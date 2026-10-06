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
        /// The Theme page: the theme in use and its indications.
        Theme,
        /// The Terminal page: new output and its limits, and passwords.
        Terminal,
    }

    /// The Terminal page: its labels and the settings as they are now.
    struct TerminalPage {
        /// The "Report new output" check box's label, with its mnemonic.
        report_output_label: String,
        /// Its accessible name, the label without its mnemonic.
        report_output_name: String,
        /// Whether it is checked.
        report_output: bool,
        /// The "Lines spoken in full" slider's label.
        full_lines_label: String,
        /// Its value.
        full_lines: i32,
        /// The "Last lines to speak" slider's label.
        last_lines_label: String,
        /// Its value.
        last_lines: i32,
        /// Both sliders' minimum.
        min_lines: i32,
        /// Both sliders' maximum.
        max_lines: i32,
        /// The check box for speaking passwords typed in terminals.
        speak_passwords_label: String,
        /// Its accessible name.
        speak_passwords_name: String,
        /// Whether it is checked.
        speak_passwords: bool,
    }

    /// The Theme page: its labels, the themes, the selected theme's
    /// description, and the settings that go with it. Rebuilt after every
    /// change; C++ updates only the widgets whose contents differ.
    struct ThemePage {
        /// The theme list's label, with its mnemonic.
        theme_label: String,
        /// The installed themes' names, the built-in default first.
        themes: Vec<String>,
        /// The selected theme's index.
        selected: i32,
        /// The description field's label.
        description_label: String,
        /// The selected theme's description, author, and problems.
        description: String,
        /// The sound volume slider's label.
        volume_label: String,
        /// The sound volume, 0 to 100.
        volume: i32,
        /// The "play sounds during say all" check box's label.
        say_all_label: String,
        /// Its accessible name, the label without its mnemonic.
        say_all_name: String,
        /// Whether it is checked.
        say_all: bool,
        /// The "also speak indications that play a sound" check box's label.
        speak_sounded_label: String,
        /// Its accessible name.
        speak_sounded_name: String,
        /// Whether it is checked.
        speak_sounded: bool,
        /// The find field's label.
        find_label: String,
        /// The indications tree's label.
        indications_label: String,
        /// The labels of the selected indication's controls: report as,
        /// sound, words, voice.
        report_label: String,
        /// The sound choice's label.
        sound_label: String,
        /// The words field's label.
        words_label: String,
        /// The voice choice's label.
        voice_label: String,
        /// The Preview button.
        preview: String,
        /// The Reset button.
        reset: String,
        /// The New Theme button.
        new_theme: String,
        /// The Rename button.
        rename: String,
        /// The Import button.
        import_label: String,
        /// The Export button.
        export_label: String,
        /// The Remove button.
        remove: String,
        /// Whether the selected theme can be renamed and exported: not the
        /// built-in one.
        can_rename: bool,
        /// Whether the selected theme can be removed: not the built-in
        /// one, and not the one the configuration uses.
        can_remove: bool,
        /// The prompts and file dialogs the page's buttons open.
        prompts: ThemePrompts,
    }

    /// The titles, prompts, and filters of the dialogs the Theme page
    /// opens, resolved for the selected theme.
    struct ThemePrompts {
        /// The New Theme prompt's title.
        new_title: String,
        /// The New Theme prompt.
        new_prompt: String,
        /// The name it suggests.
        new_name: String,
        /// The Rename prompt's title.
        rename_title: String,
        /// The Rename prompt.
        rename_prompt: String,
        /// The selected theme's name, which the Rename prompt starts with.
        name: String,
        /// The Import file dialog's title.
        import_title: String,
        /// The Export file dialog's title.
        export_title: String,
        /// The file name the Export file dialog suggests.
        export_file: String,
        /// The wildcard for theme packages.
        package_filter: String,
        /// The Browse sound file dialog's title.
        sound_title: String,
        /// The wildcard for sound files.
        sound_filter: String,
        /// The Remove confirmation's title.
        remove_title: String,
        /// The Remove confirmation's question.
        remove_question: String,
        /// The title of a message reporting a failed operation.
        error_title: String,
    }

    /// One category of the indications tree.
    struct ThemeTreeCategory {
        /// The category's name.
        label: String,
        /// Its indications that match the find field, in catalogue order.
        items: Vec<ThemeTreeItem>,
    }

    /// One indication in the tree.
    struct ThemeTreeItem {
        /// Its name and setting: "link: speech and sound", and ", changed"
        /// when it differs from the default theme.
        label: String,
        /// Its index in the catalogue, which identifies it to Rust.
        indication: usize,
    }

    /// The selected indication's controls.
    struct IndicationControls {
        /// The "report as" options: off, speech, sound, speech and sound.
        report_options: Vec<String>,
        /// The selected option, or -1 with no indication selected.
        report: i32,
        /// Whether the choice is enabled.
        report_enabled: bool,
        /// The sound options: none, a tone, each sound file, Browse.
        sound_options: Vec<String>,
        /// The selected sound, or -1.
        sound: i32,
        /// The index of "Browse...", which asks for a file instead of
        /// choosing one.
        sound_browse: i32,
        /// Whether the sound choice is enabled.
        sound_enabled: bool,
        /// The replacement words, empty for the default.
        words: String,
        /// Whether the words field is enabled.
        words_enabled: bool,
        /// The voice options: default, then each voice style.
        voice_options: Vec<String>,
        /// The selected voice, or -1.
        voice: i32,
        /// Whether the voice choice is enabled.
        voice_enabled: bool,
        /// Whether Preview is enabled.
        preview_enabled: bool,
        /// Whether Reset is enabled.
        reset_enabled: bool,
    }

    /// What became of a change to an indication.
    struct ThemeEdit {
        /// The selected theme is built in, so the change waits for a name
        /// for a new theme to make it in: C++ asks for one and answers with
        /// `theme_named`.
        needs_name: bool,
        /// The name prompt's title.
        prompt_title: String,
        /// The name prompt.
        prompt: String,
        /// The name it suggests.
        suggested_name: String,
        /// Why the change failed, or empty.
        error: String,
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
        /// The space bar.
        Space,
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
        /// The Theme page's sound choice.
        SoundChoice,
        /// Any other button, such as the Theme page's.
        OtherButton,
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
        /// Activate the focused button.
        ActivateFocused,
        /// Play the sound the Theme page's sound choice shows.
        PlaySound,
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

        /// The Theme page, built afresh from the theme panel's state.
        fn theme_page(self: &GuiCore) -> ThemePage;
        /// The indications tree, filtered by the find field.
        fn theme_tree(self: &GuiCore) -> Vec<ThemeTreeCategory>;
        /// The selected indication's controls.
        fn indication_controls(self: &GuiCore) -> IndicationControls;
        /// A theme was chosen in the list: it applies at once.
        fn theme_chosen(self: &GuiCore, index: usize);
        /// The find field changed.
        fn theme_filter_changed(self: &GuiCore, text: &str);
        /// An indication was selected in the tree, by its catalogue index,
        /// or a category or nothing (-1).
        fn indication_selected(self: &GuiCore, indication: i64);
        /// "Report as" changed.
        fn report_changed(self: &GuiCore, option: usize) -> ThemeEdit;
        /// The sound choice changed to an option other than Browse.
        fn sound_changed(self: &GuiCore, option: usize) -> ThemeEdit;
        /// A sound file was chosen through Browse.
        fn sound_browsed(self: &GuiCore, path: &str) -> ThemeEdit;
        /// The words field changed.
        fn words_changed(self: &GuiCore, text: &str) -> ThemeEdit;
        /// The voice choice changed.
        fn voice_changed(self: &GuiCore, option: usize) -> ThemeEdit;
        /// Reset: the indication goes back to the default theme's setting.
        fn reset_indication(self: &GuiCore) -> ThemeEdit;
        /// The answer to a `ThemeEdit`'s name prompt: the change waiting
        /// for it is made in a new theme of that name, or dropped when the
        /// prompt was cancelled.
        fn theme_named(self: &GuiCore, name: &str, accepted: bool) -> ThemeEdit;
        /// Preview: a sample of the indication, through the theme.
        fn preview_indication(self: &GuiCore);
        /// Space on the sound choice: plays the sound it shows.
        fn play_indication_sound(self: &GuiCore);
        /// The sound volume slider moved.
        fn volume_changed(self: &GuiCore, volume: i32);
        /// The "play sounds during say all" check box was toggled.
        fn say_all_changed(self: &GuiCore, checked: bool);
        /// The "also speak indications that play a sound" check box was
        /// toggled.
        fn speak_sounded_changed(self: &GuiCore, checked: bool);
        /// New theme based on the selected one, named `name`. Returns why
        /// it failed, or an empty string.
        fn new_theme(self: &GuiCore, name: &str) -> String;
        /// Renames the selected theme. Returns why it failed, or empty.
        fn rename_theme(self: &GuiCore, name: &str) -> String;
        /// Installs the theme package at `path`. Returns why it failed, or
        /// empty.
        fn import_theme(self: &GuiCore, path: &str) -> String;
        /// Writes the selected theme as a package at `path`. Returns why it
        /// failed, or empty.
        fn export_theme(self: &GuiCore, path: &str) -> String;
        /// Removes the selected theme. Returns why it failed, or empty.
        fn remove_theme(self: &GuiCore) -> String;

        /// The Terminal page, as its state has it.
        fn terminal_page(self: &GuiCore) -> TerminalPage;
        /// "Report new output" was toggled.
        fn terminal_report_output_changed(self: &GuiCore, checked: bool);
        /// "Lines spoken in full" moved.
        fn terminal_full_lines_changed(self: &GuiCore, value: i32);
        /// "Last lines to speak" moved.
        fn terminal_last_lines_changed(self: &GuiCore, value: i32);
        /// The check box for speaking passwords typed in terminals was
        /// toggled.
        fn terminal_speak_passwords_changed(self: &GuiCore, checked: bool);

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
        /// Pops the Verbatim menu at the hidden frame's own origin (screen
        /// centre, once [`centre_frame`] has run), running a nested loop
        /// until it closes. True when it was shown.
        fn popup_menu() -> bool;

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
