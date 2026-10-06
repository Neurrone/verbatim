//! GUI (architecture section 11, decision D4).
//!
//! The Verbatim menu and settings UI, built on wxWidgets through a small C++
//! layer (`cpp/gui.cpp`) that this crate's build script compiles and links
//! against static wxWidgets. Rust keeps all the logic; the C++ layer only
//! builds and wires widgets (see the `bridge` module). It runs on the process main
//! thread inside Core: [`run_gui`] takes over that thread with wxWidgets'
//! event loop and hands the rest of the app a [`GuiHandle`] (through the
//! `on_ready` callback) for posting commands back in. wxWidgets
//! accessibility is proven in exactly this role — NVDA's own GUI is wxPython
//! — and the M1 prototype's defining test is Verbatim reading this GUI
//! through a real outpost over ordinary UIA, with no self-voicing side
//! channel.
//!
//! Threading model. wxWidgets objects are not thread-safe and live only on the
//! GUI thread. Any thread may call [`GuiHandle::send`]; it sends a
//! [`GuiCommand`] down the GUI's channel and wakes the GUI thread, which
//! drains the channel and acts on each message. What a request means for the
//! menu, the dialogs, and the hidden frame is decided by the pure `lifecycle`
//! state machine. Outbound, the Exit menu item does not tear down the process
//! itself — it sends [`GuiEvent::QuitRequested`] and lets the app orchestrate
//! shutdown, which comes back as [`GuiCommand::Shutdown`].

mod bridge;
mod foreground;
mod hidden_frame;
mod keys;
mod lifecycle;
mod list_dialog;
mod plan;
mod settings;
pub mod shell_items;
mod terminal_panel;
mod theme_panel;
mod tray_list;

use std::cell::RefCell;
use std::fmt;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, unbounded};
use verbatim_i18n::messages;
use verbatim_speech::{SettingId, SettingValue, SpeechSettingsHost};

pub use plan::ControlPlan;
pub use shell_items::ShellItemKind;
pub use terminal_panel::{TerminalChange, TerminalHost};
pub use theme_panel::ThemeHost;

use bridge::ffi;
use lifecycle::{Frame, Lifecycle, OpenSettings, OpenShellList};
use list_dialog::{ButtonVerdict, ListDialogButtons};
use settings::{ControlChange, SpeechControls};
use terminal_panel::TerminalPanel;
use theme_panel::ThemePanel;

/// A command posted to the GUI thread from anywhere in the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiCommand {
    /// Pop the Verbatim menu at screen centre (the gesture-driven entry point).
    ShowMenu,
    /// Open the settings dialog, or focus it if it is already open.
    OpenSettings,
    /// Open the system tray or taskbar items list dialog (the systrayList
    /// replica): enumerate the shell items on a worker thread, then present
    /// the list. Focuses the existing dialog if one is already open.
    OpenShellItemList(ShellItemKind),
    /// Tear down the tray and exit the GUI event loop.
    Shutdown,
}

/// An event the GUI raises for the app to handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiEvent {
    /// The user chose Exit. The GUI does not exit the process; the app decides
    /// how to shut down and then sends [`GuiCommand::Shutdown`] back.
    QuitRequested,
}

/// A message for the GUI thread: a command from the app, or the outcome of
/// work the GUI started on another thread.
enum GuiMessage {
    /// A command from the app.
    Command(GuiCommand),
    /// A shell item enumeration finished: `None` when it failed or ran out
    /// of time (already logged).
    ShellItems(ShellItemKind, Option<Vec<shell_items::ShellItem>>),
}

/// A thread-safe handle for posting [`GuiCommand`]s to the GUI thread.
///
/// Cloneable and `Send`/`Sync`: hand copies to whatever threads need to drive
/// the GUI. Every [`send`](GuiHandle::send) sends the command down the GUI's
/// channel and wakes the GUI thread.
#[derive(Clone)]
pub struct GuiHandle {
    sender: Sender<GuiMessage>,
}

impl GuiHandle {
    /// Posts a command to the GUI thread. Safe to call from any thread.
    pub fn send(&self, command: GuiCommand) {
        self.post(GuiMessage::Command(command));
    }

    /// Sends a message and wakes the GUI thread to drain it. Once the GUI
    /// has ended, the message is dropped and the wake does nothing.
    fn post(&self, message: GuiMessage) {
        if self.sender.send(message).is_ok() {
            ffi::wake_event_loop();
        }
    }
}

/// An error from starting or running the GUI.
#[derive(Debug)]
pub struct GuiError(String);

impl fmt::Display for GuiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GUI error: {}", self.0)
    }
}

impl std::error::Error for GuiError {}

/// Runs the GUI on the calling thread, blocking until the event loop exits.
///
/// The app calls this from the process main thread. Once the hidden main frame
/// and tray icon exist, `on_ready` is invoked with a [`GuiHandle`] so the app
/// can begin posting commands; `on_ready` runs on the GUI thread during
/// initialization, so it should hand the handle off rather than block.
///
/// `settings_host` backs the settings dialog's Speech page,
/// `theme_host` its Theme page, and `terminal_host` its Terminal page;
/// `events` carries [`GuiEvent`]s out to the app.
///
/// The GUI runs at most once per process: wxWidgets keeps one application
/// in process-wide state and does not support starting again after it
/// ends, so a second call fails.
///
/// # Errors
///
/// Returns [`GuiError`] if wxWidgets fails to initialize, or if the GUI has
/// already run in this process.
pub fn run_gui(
    settings_host: Arc<dyn SpeechSettingsHost>,
    theme_host: Arc<dyn ThemeHost>,
    terminal_host: Arc<dyn TerminalHost>,
    events: Sender<GuiEvent>,
    on_ready: impl FnOnce(GuiHandle) + Send + 'static,
) -> Result<(), GuiError> {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return Err(GuiError(
            "the GUI has already run in this process".to_owned(),
        ));
    }
    let (sender, receiver) = unbounded();
    let core = GuiCore {
        host: settings_host,
        theme_host,
        theme: RefCell::new(None),
        terminal_host,
        terminal: RefCell::new(None),
        events,
        handle: GuiHandle { sender },
        receiver,
        on_ready: RefCell::new(Some(Box::new(on_ready))),
        lifecycle: RefCell::new(Lifecycle::new()),
        speech: RefCell::new(SpeechControls::default()),
        list_buttons: RefCell::new(None),
    };
    // The hidden main frame doubles as the dialog parent and the
    // single-instance rendezvous window, found by this title. It is never
    // shown except around popups and never appears in the taskbar.
    let text = ffi::ShellText {
        title: messages::tray_tooltip(),
        tooltip: messages::tray_tooltip(),
        settings_item: messages::menu_settings(),
        exit_item: messages::menu_exit(),
    };
    match ffi::run_event_loop(&core, &text) {
        -1 => Err(GuiError("wxWidgets could not start".to_owned())),
        _ => Ok(()),
    }
}

/// The callback `run_gui` hands the GUI handle to once the GUI is up.
type OnReady = Box<dyn FnOnce(GuiHandle) + Send>;

/// The GUI's Rust half: the state behind the widgets, and the methods the
/// C++ layer calls when the user acts.
///
/// It lives on `run_gui`'s stack for the whole event loop and is used only
/// on the GUI thread. Every method takes `&self`, because a call into C++
/// can run a nested event loop that calls back in; for the same reason no
/// `RefCell` borrow is ever held across a call into C++.
pub(crate) struct GuiCore {
    host: Arc<dyn SpeechSettingsHost>,
    /// What the Theme page reads and changes.
    theme_host: Arc<dyn ThemeHost>,
    /// The Theme page's state while the settings dialog is open and the
    /// page has been shown.
    theme: RefCell<Option<ThemePanel>>,
    /// What the Terminal page reads and changes.
    terminal_host: Arc<dyn TerminalHost>,
    /// The Terminal page's state while the settings dialog is open and the
    /// page has been shown.
    terminal: RefCell<Option<TerminalPanel>>,
    events: Sender<GuiEvent>,
    /// The GUI's own handle, for work it hands to other threads.
    handle: GuiHandle,
    /// The GUI's end of its channel.
    receiver: Receiver<GuiMessage>,
    on_ready: RefCell<Option<OnReady>>,
    lifecycle: RefCell<Lifecycle>,
    /// The Speech page's current controls.
    speech: RefCell<SpeechControls>,
    /// The open list dialog's button callbacks.
    list_buttons: RefCell<Option<ListDialogButtons>>,
}

impl GuiCore {
    /// The frame, tray icon, and menu exist: marks the frame and hands the
    /// app its handle.
    fn ready(&self) {
        // Mark the frame so every outpost can recognize and suppress
        // announcing it (decision D9); see `hidden_frame`'s module doc.
        if let Some(hwnd) = foreground::hwnd_of(ffi::frame_handle()) {
            hidden_frame::mark(hwnd);
        }
        let on_ready = self.on_ready.borrow_mut().take();
        if let Some(on_ready) = on_ready {
            on_ready(self.handle.clone());
        }
    }

    /// Acts on every message waiting in the channel.
    fn drain(&self) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                GuiMessage::Command(command) => self.dispatch(command),
                GuiMessage::ShellItems(kind, items) => self.present_shell_items(kind, items),
            }
        }
    }

    /// Acts on one command.
    fn dispatch(&self, command: GuiCommand) {
        match command {
            GuiCommand::ShowMenu => self.show_menu(),
            GuiCommand::OpenSettings => self.open_settings(),
            GuiCommand::OpenShellItemList(kind) => self.open_shell_item_list(kind),
            GuiCommand::Shutdown => self.shut_down(),
        }
    }

    fn tray_clicked(&self) {
        self.show_menu();
    }

    fn menu_chosen(&self, choice: ffi::MenuChoice) {
        match choice {
            ffi::MenuChoice::Settings => self.open_settings(),
            ffi::MenuChoice::Exit => {
                let _ = self.events.send(GuiEvent::QuitRequested);
            }
            _ => {}
        }
    }

    /// Prepares for a popup, NVDA's `prePopup`: make the frame visible and
    /// take the foreground.
    ///
    /// A menu or dialog popped from a hidden, background window gets neither
    /// the foreground nor keyboard focus — Windows raises no foreground
    /// event, so Verbatim's own supervisor never targets our process and
    /// never reads the popup. Showing the (1x1) frame and forcing it
    /// foreground fixes both.
    fn pre_popup() {
        ffi::show_frame();
        if let Some(hwnd) = foreground::hwnd_of(ffi::frame_handle()) {
            foreground::force_foreground(hwnd);
        }
    }

    /// Cleans up after a popup, NVDA's `postPopup`: hide the frame again so
    /// we keep no visible window and the foreground falls back to the
    /// previous application — unless the lifecycle says a dialog still
    /// needs the frame as its visible owner.
    fn post_popup(after: Frame) {
        if after == Frame::Hide {
            ffi::hide_frame();
        }
    }

    /// Pops the Verbatim menu at screen centre.
    fn show_menu(&self) {
        let pop = self.lifecycle.borrow_mut().request_menu();
        if !pop {
            return;
        }
        let at = ffi::centre_frame();
        let (x, y) = (at.x, at.y);
        Self::pre_popup();
        // The menu's nested loop dispatches the chosen item (which may open
        // settings) before this returns.
        let shown = ffi::popup_menu();
        // Info, not debug: whether the popup actually showed is the first
        // fact needed when diagnosing a menu that opened silently or not at
        // all, and it fires only on an explicit user gesture, so it cannot
        // flood the log.
        tracing::info!(shown, x, y, "popped the Verbatim menu");
        let after = self.lifecycle.borrow_mut().menu_closed();
        Self::post_popup(after);
    }

    /// Opens the settings dialog, or focuses the existing one.
    fn open_settings(&self) {
        let action = self.lifecycle.borrow_mut().request_settings();
        match action {
            OpenSettings::Ignore => {}
            OpenSettings::FocusExisting => Self::focus_foreground(ffi::DialogKind::Settings),
            OpenSettings::Create => {
                // Take the foreground before the dialog exists, so the
                // process already owns it when the dialog asks for it.
                Self::pre_popup();
                ffi::open_settings_dialog(&settings::dialog());
                Self::focus_foreground(ffi::DialogKind::Settings);
            }
        }
    }

    /// Opens the shell item list dialog for `kind`: focuses the existing
    /// one when open, otherwise starts an enumeration on a worker thread
    /// whose outcome comes back through the GUI's channel. The GUI thread
    /// never blocks on the shell — see [`shell_items`]' module
    /// documentation for the threading and deadline story.
    fn open_shell_item_list(&self, kind: ShellItemKind) {
        let action = self.lifecycle.borrow_mut().request_shell_list();
        match action {
            OpenShellList::Ignore => {
                tracing::debug!("a shell item enumeration is already in flight; request dropped");
            }
            OpenShellList::FocusExisting => Self::focus_foreground(ffi::DialogKind::ShellList),
            OpenShellList::Enumerate => {
                let handle = self.handle.clone();
                let started = shell_items::request_shell_items(kind, move |items| {
                    handle.post(GuiMessage::ShellItems(kind, items));
                });
                if !started {
                    // No outcome will ever arrive: end the enumeration now,
                    // as one that found nothing, so later requests are not
                    // ignored as if it were still in flight.
                    self.lifecycle.borrow_mut().shell_items_arrived(false);
                }
            }
        }
    }

    /// Presents an enumeration outcome when the lifecycle says to. A failed
    /// or timed-out enumeration (already logged by the worker) presents
    /// nothing.
    fn present_shell_items(&self, kind: ShellItemKind, items: Option<Vec<shell_items::ShellItem>>) {
        let present = self
            .lifecycle
            .borrow_mut()
            .shell_items_arrived(items.is_some());
        let (true, Some(items)) = (present, items) else {
            return;
        };
        let (model, buttons) = tray_list::shell_list_dialog(kind, items).split();
        *self.list_buttons.borrow_mut() = Some(buttons);
        // Same discipline as settings: take the foreground before the
        // dialog exists, so the process already owns it when the dialog
        // asks.
        Self::pre_popup();
        ffi::open_list_dialog(&model);
        Self::focus_foreground(ffi::DialogKind::ShellList);
    }

    /// Raises a dialog, takes the foreground for it, and focuses it, so its
    /// controls are read as they gain focus.
    fn focus_foreground(dialog: ffi::DialogKind) {
        ffi::raise_dialog(dialog);
        if let Some(hwnd) = foreground::hwnd_of(ffi::dialog_handle(dialog)) {
            foreground::force_foreground(hwnd);
        }
        ffi::focus_dialog(dialog);
    }

    /// Tears down the tray, the dialogs, and the frame, and ends the event
    /// loop.
    fn shut_down(&self) {
        self.lifecycle.borrow_mut().shut_down();
        self.list_buttons.borrow_mut().take();
        if let Some(hwnd) = foreground::hwnd_of(ffi::frame_handle()) {
            hidden_frame::unmark(hwnd);
        }
        ffi::shut_down();
    }

    fn dialog_closed(&self, dialog: ffi::DialogKind) {
        let which = match dialog {
            ffi::DialogKind::Settings => {
                self.theme.borrow_mut().take();
                self.terminal.borrow_mut().take();
                lifecycle::Dialog::Settings
            }
            ffi::DialogKind::ShellList => {
                self.list_buttons.borrow_mut().take();
                lifecycle::Dialog::ShellList
            }
            _ => return,
        };
        let after = self.lifecycle.borrow_mut().closed(which);
        Self::post_popup(after);
    }

    fn speech_page(&self) -> ffi::SpeechPage {
        self.speech.borrow_mut().rebuild(self.host.as_ref())
    }

    /// Applies a change to one of the Speech page's controls live.
    fn apply_change(&self, generation: u32, control: usize, change: ControlChange) {
        let setting = self
            .speech
            .borrow()
            .setting_for(generation, control, change);
        if let Some((id, value)) = setting {
            self.set_setting(&id, value);
        }
    }

    fn set_setting(&self, id: &SettingId, value: SettingValue) {
        if let Err(error) = self.host.set_setting(id, value) {
            tracing::warn!(%error, setting = %id, "could not apply a setting");
        }
    }

    fn slider_changed(&self, generation: u32, control: usize, value: i32) {
        self.apply_change(generation, control, ControlChange::Number(value));
    }

    fn choice_changed(&self, generation: u32, control: usize, option: usize) {
        self.apply_change(generation, control, ControlChange::Option(option));
    }

    fn toggle_changed(&self, generation: u32, control: usize, checked: bool) {
        self.apply_change(generation, control, ControlChange::Toggle(checked));
    }

    fn commit_settings(&self) {
        if let Err(error) = self.host.commit() {
            tracing::warn!(%error, "could not save the settings");
        }
        if let Some(theme) = self.theme.borrow_mut().as_mut() {
            let failures = theme.apply();
            if !failures.is_empty() {
                tracing::warn!(failures, "could not save the theme settings");
            }
        }
        if let Some(terminal) = self.terminal.borrow_mut().as_mut() {
            terminal.apply();
        }
    }

    fn revert_settings(&self) {
        self.host.revert();
        if let Some(theme) = self.theme.borrow_mut().as_mut() {
            theme.cancel();
        }
        if let Some(terminal) = self.terminal.borrow_mut().as_mut() {
            terminal.cancel();
        }
    }

    /// Runs `act` on the Theme page's state, opening it from the
    /// configuration the first time the page is used.
    fn with_theme<R>(&self, act: impl FnOnce(&mut ThemePanel) -> R) -> R {
        let mut theme = self.theme.borrow_mut();
        let panel = theme.get_or_insert_with(|| ThemePanel::open(Arc::clone(&self.theme_host)));
        act(panel)
    }

    fn theme_page(&self) -> ffi::ThemePage {
        self.with_theme(|theme| theme.page())
    }

    fn theme_tree(&self) -> Vec<ffi::ThemeTreeCategory> {
        self.with_theme(|theme| theme.tree())
    }

    fn indication_controls(&self) -> ffi::IndicationControls {
        self.with_theme(|theme| theme.controls())
    }

    fn theme_chosen(&self, index: usize) {
        self.with_theme(|theme| theme.choose(index));
    }

    fn theme_filter_changed(&self, text: &str) {
        self.with_theme(|theme| theme.set_filter(text));
    }

    fn indication_selected(&self, indication: i64) {
        let index = usize::try_from(indication).ok();
        self.with_theme(|theme| theme.select(index));
    }

    fn report_changed(&self, option: usize) -> ffi::ThemeEdit {
        self.with_theme(|theme| theme.set_report(option))
    }

    fn sound_changed(&self, option: usize) -> ffi::ThemeEdit {
        self.with_theme(|theme| theme.set_sound(option))
    }

    fn sound_browsed(&self, path: &str) -> ffi::ThemeEdit {
        self.with_theme(|theme| theme.add_sound(std::path::Path::new(path)))
    }

    fn words_changed(&self, text: &str) -> ffi::ThemeEdit {
        self.with_theme(|theme| theme.set_words(text))
    }

    fn voice_changed(&self, option: usize) -> ffi::ThemeEdit {
        self.with_theme(|theme| theme.set_voice(option))
    }

    fn reset_indication(&self) -> ffi::ThemeEdit {
        self.with_theme(ThemePanel::reset)
    }

    fn theme_named(&self, name: &str, accepted: bool) -> ffi::ThemeEdit {
        self.with_theme(|theme| theme.named(name, accepted))
    }

    fn preview_indication(&self) {
        self.with_theme(|theme| theme.preview());
    }

    fn play_indication_sound(&self) {
        self.with_theme(|theme| theme.play_sound());
    }

    fn volume_changed(&self, volume: i32) {
        self.with_theme(|theme| theme.set_volume(volume));
    }

    fn say_all_changed(&self, checked: bool) {
        self.with_theme(|theme| theme.set_say_all(checked));
    }

    fn speak_sounded_changed(&self, checked: bool) {
        self.with_theme(|theme| theme.set_speak_sounded(checked));
    }

    fn new_theme(&self, name: &str) -> String {
        self.with_theme(|theme| theme.new_theme(name))
    }

    fn rename_theme(&self, name: &str) -> String {
        self.with_theme(|theme| theme.rename(name))
    }

    fn import_theme(&self, path: &str) -> String {
        self.with_theme(|theme| theme.import(std::path::Path::new(path)))
    }

    fn export_theme(&self, path: &str) -> String {
        self.with_theme(|theme| theme.export(std::path::Path::new(path)))
    }

    fn remove_theme(&self) -> String {
        self.with_theme(ThemePanel::remove)
    }

    /// Runs `act` on the Terminal page's state, opening it from the reader
    /// settings as they are now the first time the page is used.
    fn with_terminal<R>(&self, act: impl FnOnce(&mut TerminalPanel) -> R) -> R {
        let mut terminal = self.terminal.borrow_mut();
        let panel =
            terminal.get_or_insert_with(|| TerminalPanel::open(Arc::clone(&self.terminal_host)));
        act(panel)
    }

    fn terminal_page(&self) -> ffi::TerminalPage {
        self.with_terminal(|terminal| terminal.page())
    }

    fn terminal_report_output_changed(&self, checked: bool) {
        self.with_terminal(|terminal| terminal.set_report_output(checked));
    }

    fn terminal_full_lines_changed(&self, value: i32) {
        self.with_terminal(|terminal| terminal.set_full_lines(value));
    }

    fn terminal_last_lines_changed(&self, value: i32) {
        self.with_terminal(|terminal| terminal.set_last_lines(value));
    }

    fn terminal_speak_passwords_changed(&self, checked: bool) {
        self.with_terminal(|terminal| terminal.set_speak_passwords(checked));
    }

    fn synthesizer_picker(&self) -> ffi::SynthesizerPicker {
        settings::synthesizer_picker(self.host.as_ref())
    }

    fn choose_synthesizer(&self, index: usize) -> bool {
        let synthesizers = self.host.synthesizers();
        let active = self.host.active_synthesizer().id;
        let Some(chosen) = settings::synthesizer_to_switch_to(&synthesizers, &active, index) else {
            return false;
        };
        match self.host.set_active_synthesizer(&chosen) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(%error, "failed to switch synthesizer; keeping the current one");
                false
            }
        }
    }

    fn list_button(&self, button: usize, item: usize) -> bool {
        // The callback is taken out of the borrow before it runs.
        let callback = self
            .list_buttons
            .borrow()
            .as_ref()
            .and_then(|buttons| buttons.callback(button));
        callback.is_some_and(|callback| callback(item) == ButtonVerdict::Close)
    }
}

/// What a key does in the settings dialog: the bridge's form of
/// [`keys::route_key`].
fn route_settings_key(
    key: ffi::SettingsKey,
    control: bool,
    shift: bool,
    focus: ffi::SettingsFocus,
) -> ffi::SettingsKeyAction {
    use keys::{DialogButton, FocusedControl, Key, KeyAction, KeyPress};

    let key = match key {
        ffi::SettingsKey::Enter => Key::Enter,
        ffi::SettingsKey::Tab => Key::Tab,
        ffi::SettingsKey::S => Key::S,
        ffi::SettingsKey::Space => Key::Space,
        _ => Key::Other,
    };
    let focused = match focus {
        ffi::SettingsFocus::Ok => FocusedControl::Button(DialogButton::Ok),
        ffi::SettingsFocus::Cancel => FocusedControl::Button(DialogButton::Cancel),
        ffi::SettingsFocus::Apply => FocusedControl::Button(DialogButton::Apply),
        ffi::SettingsFocus::ChangeSynthesizer => {
            FocusedControl::Button(DialogButton::ChangeSynthesizer)
        }
        ffi::SettingsFocus::SynthesizerName => FocusedControl::SynthesizerName,
        ffi::SettingsFocus::SoundChoice => FocusedControl::SoundChoice,
        ffi::SettingsFocus::OtherButton => FocusedControl::OtherButton,
        _ => FocusedControl::Other,
    };
    match keys::route_key(
        KeyPress {
            key,
            control,
            shift,
        },
        focused,
    ) {
        KeyAction::PassThrough => ffi::SettingsKeyAction::PassThrough,
        KeyAction::CycleCategory { forward: true } => ffi::SettingsKeyAction::NextCategory,
        KeyAction::CycleCategory { forward: false } => ffi::SettingsKeyAction::PreviousCategory,
        KeyAction::Activate(DialogButton::Ok) => ffi::SettingsKeyAction::Ok,
        KeyAction::Activate(DialogButton::Cancel) => ffi::SettingsKeyAction::Cancel,
        KeyAction::Activate(DialogButton::Apply) => ffi::SettingsKeyAction::Apply,
        KeyAction::Activate(DialogButton::ChangeSynthesizer) => {
            ffi::SettingsKeyAction::ChangeSynthesizer
        }
        KeyAction::ActivateFocused => ffi::SettingsKeyAction::ActivateFocused,
        KeyAction::PlaySound => ffi::SettingsKeyAction::PlaySound,
    }
}

/// The category after `current` among `count`, wrapping: the bridge's form
/// of [`plan::cycle_index`].
fn next_category(current: usize, count: usize, forward: bool) -> usize {
    plan::cycle_index(current, count, forward)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bridge_routes_keys_as_the_router_does() {
        use ffi::{SettingsFocus as Focus, SettingsKey as Key, SettingsKeyAction as Action};
        assert_eq!(
            route_settings_key(Key::Enter, false, false, Focus::Cancel),
            Action::Cancel
        );
        assert_eq!(
            route_settings_key(Key::Enter, false, false, Focus::SynthesizerName),
            Action::ChangeSynthesizer
        );
        assert_eq!(
            route_settings_key(Key::Enter, false, false, Focus::Other),
            Action::Ok
        );
        assert_eq!(
            route_settings_key(Key::S, true, false, Focus::Other),
            Action::Apply
        );
        assert_eq!(
            route_settings_key(Key::Tab, true, true, Focus::Other),
            Action::PreviousCategory
        );
        assert_eq!(
            route_settings_key(Key::Tab, false, false, Focus::Other),
            Action::PassThrough
        );
        assert_eq!(
            route_settings_key(Key::Enter, false, false, Focus::OtherButton),
            Action::ActivateFocused
        );
        assert_eq!(
            route_settings_key(Key::Space, false, false, Focus::SoundChoice),
            Action::PlaySound
        );
    }
}
