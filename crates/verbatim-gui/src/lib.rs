//! GUI (architecture section 11, decision D4).
//!
//! The Verbatim menu and settings UI, built on wxWidgets via wxDragon. It runs
//! on the process main thread inside Core: [`run_gui`] takes over that thread
//! with wxWidgets' event loop and hands the rest of the app a [`GuiHandle`]
//! (through the `on_ready` callback) for posting commands back in. wxWidgets
//! accessibility is proven in exactly this role — NVDA's own GUI is wxPython —
//! and the M1 prototype's defining test is Verbatim reading this GUI through a
//! real outpost over ordinary UIA, with no self-voicing side channel.
//!
//! Threading model. wxWidgets objects are not thread-safe and live only on the
//! GUI thread. Any thread may call [`GuiHandle::send`]; it sends a
//! [`GuiCommand`] down the GUI's channel and wakes the GUI thread, which
//! drains the channel and acts on each message against the thread-local
//! widget state. What a request means for the dialogs and the hidden frame
//! is decided by the pure `lifecycle` state machine. Outbound, the Exit menu item does
//! not tear down the process itself — it sends [`GuiEvent::QuitRequested`] and
//! lets the app orchestrate shutdown, which comes back as
//! [`GuiCommand::Shutdown`].

mod dialog;
mod foreground;
mod hidden_frame;
mod keys;
mod lifecycle;
pub mod list_dialog;
mod plan;
pub mod shell_items;
mod tray_list;

use std::cell::RefCell;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wxdragon::prelude::*;

use crossbeam_channel::{Receiver, Sender, unbounded};
use verbatim_i18n::messages;
use verbatim_speech::SpeechSettingsHost;

pub use plan::ControlPlan;
pub use shell_items::ShellItemKind;

use lifecycle::{Frame as FrameAfter, Lifecycle, OpenSettings, OpenShellList};

/// Menu item ids for the tray menu, based above the standard id range so they
/// never collide with wxWidgets' own ids.
const ID_MENU_SETTINGS: i32 = ID_HIGHEST + 1001;
const ID_MENU_EXIT: i32 = ID_HIGHEST + 1002;

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
    /// has ended, the message is dropped.
    fn post(&self, message: GuiMessage) {
        if self.sender.send(message).is_ok() {
            wxdragon::call_after(Box::new(drain_messages));
            wxdragon::wake_up_idle();
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
/// `settings_host` backs the settings dialog; `events` carries
/// [`GuiEvent`]s out to the app.
///
/// # Errors
///
/// Returns [`GuiError`] if wxWidgets fails to initialize or the event loop
/// exits abnormally.
pub fn run_gui(
    settings_host: Arc<dyn SpeechSettingsHost>,
    events: Sender<GuiEvent>,
    on_ready: impl FnOnce(GuiHandle) + Send + 'static,
) -> Result<(), GuiError> {
    let (sender, receiver) = unbounded();
    let handle = GuiHandle { sender };
    wxdragon::main(move |app| {
        init(app, settings_host, events, handle.clone(), receiver);
        on_ready(handle);
    })
    .map_err(|error| GuiError(error.to_string()))
}

/// The GUI thread's widget state. Not `Send`: it lives only in the GUI
/// thread's [`GUI`] thread-local.
struct GuiState {
    frame: Frame,
    tray: TaskBarIcon,
    /// The shared tray menu. Moved out of here for the duration of a popup (see
    /// [`show_menu`]) so the nested menu loop can re-enter this state.
    menu: Option<Menu>,
    host: Arc<dyn SpeechSettingsHost>,
    events: Sender<GuiEvent>,
    settings: Option<Dialog>,
    /// The open shell item list dialog (singleton, like `settings`).
    shell_list: Option<Dialog>,
    /// Which dialogs are open and what that means for the frame.
    lifecycle: Lifecycle,
    /// The GUI's own handle, for work it hands to other threads.
    handle: GuiHandle,
    /// The GUI's end of its channel.
    receiver: Receiver<GuiMessage>,
}

thread_local! {
    static GUI: RefCell<Option<GuiState>> = const { RefCell::new(None) };
}

/// Whether shutdown has been commanded.
///
/// Deliberately not a field of [`GuiState`]: `shutdown` sets this and then
/// calls `frame.close(true)`, which synchronously re-enters the frame's
/// close handler (wxWidgets' `Close` always invokes it; `force` only
/// controls whether the handler may veto) on the same call stack, and that
/// handler reads this flag through [`gui_is_shutting_down`]. Routing the
/// read through the `GUI` thread-local's `RefCell`, as before, collided
/// with `shutdown`'s own borrow and panicked with "already mutably
/// borrowed" on every clean quit. A plain atomic has no borrow to collide
/// with, so it is the honest type for a flag that must be legible from a
/// reentrant context.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// Builds the hidden frame, tray icon, and shared menu, then stores them in the
/// GUI thread-local. Runs once, on the GUI thread, during initialization.
fn init(
    app: App,
    host: Arc<dyn SpeechSettingsHost>,
    events: Sender<GuiEvent>,
    handle: GuiHandle,
    receiver: Receiver<GuiMessage>,
) {
    // We control the loop lifetime explicitly (Shutdown), so deleting a dialog
    // or the hidden frame must not end the loop on its own.
    app.set_exit_on_frame_delete(false);
    SHUTTING_DOWN.store(false, Ordering::SeqCst);

    // The hidden main frame doubles as the dialog parent and the single-instance
    // rendezvous window (found later by its title). It is never shown and never
    // appears in the taskbar.
    let frame = Frame::builder()
        .with_title(&messages::tray_tooltip())
        .with_size(Size {
            width: 1,
            height: 1,
        })
        .with_style(FrameStyle::Default | FrameStyle::NoTaskbar)
        .build();
    // Mark the frame so every outpost can recognize and suppress announcing
    // it (decision D9); see `hidden_frame`'s module doc.
    if let Some(hwnd) = foreground::hwnd_of(frame.get_handle()) {
        hidden_frame::mark(hwnd);
    }
    frame.on_close(move |event| {
        // Closing the hidden window hides it; only a commanded Shutdown lets it
        // be destroyed.
        if gui_is_shutting_down() {
            event.skip(true);
        } else {
            frame.hide();
            event.skip(false);
        }
    });

    // One menu object, shared as the tray's popup and popped on ShowMenu.
    let mut menu = Menu::builder()
        .append_item(ID_MENU_SETTINGS, &messages::menu_settings(), "")
        .append_separator()
        .append_item(ID_MENU_EXIT, &messages::menu_exit(), "")
        .build();
    frame.on_menu(|event| dispatch_menu(event.get_id()));

    let tray = TaskBarIcon::builder().build();
    // A blank 16x16 icon: M1 has no bundled artwork, and the tray icon's
    // appearance is not what the milestone validates.
    if let Some(icon) = Bitmap::from_rgba(&[0u8; 16 * 16 * 4], 16, 16) {
        tray.set_icon(&icon, &messages::tray_tooltip());
    }
    tray.set_popup_menu(&mut menu);
    tray.on_menu(|event| dispatch_menu(event.get_id()));
    #[cfg(target_os = "windows")]
    tray.on_left_down(|_| dispatch(GuiCommand::ShowMenu));

    GUI.with(|cell| {
        *cell.borrow_mut() = Some(GuiState {
            frame,
            tray,
            menu: Some(menu),
            host,
            events,
            settings: None,
            shell_list: None,
            lifecycle: Lifecycle::new(),
            handle,
            receiver,
        });
    });
}

/// Whether shutdown has been commanded. Safe to call from a reentrant
/// context (see [`SHUTTING_DOWN`]'s doc comment), including from inside a
/// borrow of [`GUI`].
fn gui_is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::SeqCst)
}

/// Routes a tray/menu item id to its command.
fn dispatch_menu(id: i32) {
    match id {
        ID_MENU_SETTINGS => dispatch(GuiCommand::OpenSettings),
        ID_MENU_EXIT => {
            GUI.with(|cell| {
                if let Some(state) = cell.borrow().as_ref() {
                    let _ = state.events.send(GuiEvent::QuitRequested);
                }
            });
        }
        _ => {}
    }
}

/// Drains and dispatches every message waiting in the GUI's channel. Runs
/// on the GUI thread; the borrow of the widget state ends before any
/// message is acted on, since acting on one can re-enter it.
fn drain_messages() {
    let Some(receiver) =
        GUI.with(|cell| cell.borrow().as_ref().map(|state| state.receiver.clone()))
    else {
        return;
    };
    while let Ok(message) = receiver.try_recv() {
        match message {
            GuiMessage::Command(command) => dispatch(command),
            GuiMessage::ShellItems(kind, items) => present_shell_item_list(kind, items),
        }
    }
}

/// Acts on one command against the GUI thread-local state.
fn dispatch(command: GuiCommand) {
    match command {
        GuiCommand::ShowMenu => show_menu(),
        GuiCommand::OpenSettings => open_settings(),
        GuiCommand::OpenShellItemList(kind) => open_shell_item_list(kind),
        GuiCommand::Shutdown => shutdown(),
    }
}

/// Prepares for a popup, NVDA's `prePopup`: make the frame visible and take the
/// foreground.
///
/// A menu or dialog popped from a hidden, background window gets neither the
/// foreground nor keyboard focus — Windows raises no foreground event, so
/// Verbatim's own supervisor never targets our process and never reads the
/// popup. Showing the (1x1) frame and forcing it foreground fixes both.
fn pre_popup(frame: Frame) {
    frame.show(true);
    frame.raise();
    if let Some(hwnd) = foreground::hwnd_of(frame.get_handle()) {
        foreground::force_foreground(hwnd);
    }
}

/// Cleans up after a popup, NVDA's `postPopup`: hide the frame again so we keep
/// no visible window, and let the foreground fall back to the previous app.
///
/// Skipped while any owned dialog (settings or the shell item list) is
/// open, as the lifecycle decides: hiding an owner can take its owned
/// windows with it. [`close_settings`] and [`close_shell_list`] hide the
/// frame once the last dialog is gone.
fn post_popup(frame: Frame, after: FrameAfter) {
    if after == FrameAfter::Hide {
        frame.hide();
    }
}

/// Pops the Verbatim menu at screen centre.
fn show_menu() {
    // The menu must leave the thread-local before the popup: `popup_menu` runs a
    // nested event loop that dispatches the menu selection, and that handler
    // re-enters the thread-local (to open settings). Holding the borrow across
    // the popup would panic.
    let Some((frame, mut menu)) = GUI.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let state = borrow.as_mut()?;
        Some((state.frame, state.menu.take()?))
    }) else {
        return;
    };

    // wxdragon exposes no display-metrics call, so centring the frame and
    // reading its position stands in for the screen centre.
    frame.center_on_screen();
    let position = frame.get_position();
    pre_popup(frame);
    // Info, not debug: whether the popup actually showed is the first fact
    // needed when diagnosing a menu that opened silently or not at all
    // (the cold-guest first-launch investigation), and it fires only on an
    // explicit user gesture, so it cannot flood the log.
    let shown = frame.popup_menu(&mut menu, Some(position));
    tracing::info!(
        shown,
        x = position.x,
        y = position.y,
        "popped the Verbatim menu"
    );

    let after = GUI.with(|cell| {
        cell.borrow_mut().as_mut().map(|state| {
            state.menu = Some(menu);
            state.lifecycle.frame_after_popup()
        })
    });
    if let Some(after) = after {
        post_popup(frame, after);
    }
}

/// Opens the settings dialog, or focuses the existing one (singleton guard).
///
/// Reads out what it needs and drops the borrow before calling into wx, on
/// the same principle as [`show_menu`] and [`shutdown`]: none of
/// `pre_popup`, `build_settings_dialog`, `dialog.show`, or
/// `focus_foreground` re-enter `GUI` today, but holding the borrow across
/// calls into wx is exactly the shape that panicked in `shutdown` once one
/// of them did.
fn open_settings() {
    let Some((frame, action, existing, host)) = GUI.with(|cell| {
        cell.borrow_mut().as_mut().map(|state| {
            (
                state.frame,
                state.lifecycle.request_settings(),
                state.settings,
                state.host.clone(),
            )
        })
    }) else {
        return;
    };

    match action {
        OpenSettings::Ignore => {}
        OpenSettings::FocusExisting => {
            if let Some(existing) = existing {
                focus_foreground(existing);
            }
        }
        OpenSettings::Create => {
            // Take the foreground before the dialog exists, so the process
            // already owns it when the dialog asks for it.
            pre_popup(frame);
            let dialog = dialog::build_settings_dialog(frame, &host);
            dialog.show(true);
            focus_foreground(dialog);
            GUI.with(|cell| {
                if let Some(state) = cell.borrow_mut().as_mut() {
                    state.settings = Some(dialog);
                }
            });
        }
    }
}

/// Opens the shell item list dialog for `kind`: focuses the existing one
/// when open (singleton, mirroring [`open_settings`]), otherwise starts an
/// enumeration on a worker thread and presents the list when the results
/// arrive through the GUI's channel. The GUI thread never blocks on the
/// shell — see [`shell_items`]' module documentation for the threading and
/// deadline story.
fn open_shell_item_list(kind: ShellItemKind) {
    let Some((action, existing, handle)) = GUI.with(|cell| {
        cell.borrow_mut().as_mut().map(|state| {
            (
                state.lifecycle.request_shell_list(),
                state.shell_list,
                state.handle.clone(),
            )
        })
    }) else {
        return;
    };
    match action {
        OpenShellList::Ignore => {
            tracing::debug!("a shell item enumeration is already in flight; request dropped");
        }
        OpenShellList::FocusExisting => {
            if let Some(existing) = existing {
                focus_foreground(existing);
            }
        }
        OpenShellList::Enumerate => {
            shell_items::request_shell_items(kind, move |items| {
                handle.post(GuiMessage::ShellItems(kind, items));
            });
        }
    }
}

/// Presents an enumeration outcome, when the lifecycle says to: builds and
/// shows the dialog on success. A failed or timed-out enumeration (already
/// logged by the worker) presents nothing. Runs on the GUI thread.
fn present_shell_item_list(kind: ShellItemKind, items: Option<Vec<shell_items::ShellItem>>) {
    let Some((frame, present)) = GUI.with(|cell| {
        cell.borrow_mut().as_mut().map(|state| {
            (
                state.frame,
                state.lifecycle.shell_items_arrived(items.is_some()),
            )
        })
    }) else {
        return;
    };
    let (true, Some(items)) = (present, items) else {
        return;
    };

    // Same discipline as open_settings: take the foreground before the
    // dialog exists, so the process already owns it when the dialog asks.
    pre_popup(frame);
    let dialog = tray_list::build_shell_list_dialog(frame, kind, items);
    dialog.show(true);
    focus_foreground(dialog);
    GUI.with(|cell| {
        if let Some(state) = cell.borrow_mut().as_mut() {
            state.shell_list = Some(dialog);
        }
    });
}

/// Raises a dialog, takes the foreground for it, and focuses it, so its controls
/// are read as they gain focus.
fn focus_foreground(dialog: Dialog) {
    dialog.raise();
    if let Some(hwnd) = foreground::hwnd_of(dialog.get_handle()) {
        foreground::force_foreground(hwnd);
    }
    dialog.set_focus();
}

/// Tears down the tray and settings dialog and exits the event loop.
///
/// Takes what it needs out of the thread-local and drops the borrow before
/// calling into wx, matching [`show_menu`]'s discipline: `frame.close`
/// below synchronously re-enters the close handler registered in [`init`]
/// on this same call stack (see [`SHUTTING_DOWN`]'s doc comment), and
/// holding a borrow of `GUI` across that call is exactly what used to
/// panic here on every clean quit.
fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    let torn_down = GUI.with(|cell| {
        cell.borrow_mut().as_mut().map(|state| {
            state.lifecycle.shut_down();
            state.tray.remove_icon();
            (state.frame, state.settings.take(), state.shell_list.take())
        })
    });
    if let Some((frame, settings, shell_list)) = torn_down {
        for dialog in [settings, shell_list].into_iter().flatten() {
            if dialog.is_valid() {
                dialog.destroy();
            }
        }
        if let Some(hwnd) = foreground::hwnd_of(frame.get_handle()) {
            hidden_frame::unmark(hwnd);
        }
        frame.close(true);
    }
    if let Some(app) = wxdragon::get_app_instance() {
        app.exit_main_loop();
    }
}

/// Closes the settings dialog and clears the singleton so the next open builds
/// a fresh one. Called by the dialog's own OK, Cancel, and Enter/Escape paths.
///
/// With the dialog gone, the deferred `postPopup` hide happens here — unless
/// the shell item list dialog is still open and needs the frame as its
/// visible owner; then [`close_shell_list`] hides it later.
pub(crate) fn close_settings(dialog: Dialog) {
    close(dialog, lifecycle::Dialog::Settings);
}

/// Closes the shell item list dialog and clears its singleton: the shared
/// dismissal path for Cancel, Escape, window close, and every click button.
/// The deferred `postPopup` hide happens here on the same terms as
/// [`close_settings`], deferring to the settings dialog when it is open.
pub(crate) fn close_shell_list(dialog: Dialog) {
    close(dialog, lifecycle::Dialog::ShellList);
}

/// The shared close path: records the close, destroys the dialog, and
/// hides the frame when nothing needs it any more.
fn close(dialog: Dialog, which: lifecycle::Dialog) {
    let after = GUI.with(|cell| {
        cell.borrow_mut().as_mut().map(|state| {
            match which {
                lifecycle::Dialog::Settings => state.settings = None,
                lifecycle::Dialog::ShellList => state.shell_list = None,
            }
            (state.frame, state.lifecycle.closed(which))
        })
    });
    dialog.destroy();
    if let Some((frame, after)) = after {
        post_popup(frame, after);
    }
}
