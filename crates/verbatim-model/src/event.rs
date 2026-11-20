//! Normalized events, reducer inputs, and reducer effects.
//!
//! The reducer's inputs are normalized events, fetch completions, the
//! user's keys and gestures, and what the shell's threads learn; its
//! effects are speech, fetches, and what the shell carries out for it, a
//! window shown among them. Every user input carries its key number
//! ([`KeySeq`]), and Core stamps the speech it causes with it.

use serde::{Deserialize, Serialize};

use crate::settings::ReaderSettings;
use crate::shell::ShellItemKind;
use crate::speech::{SpeechMark, Utterance};
use crate::text::{CaretKey, CaretReport, FocusText, SelectionChange};
use crate::tree::{Backend, NodeSnapshot, StateSet};
use crate::{KeySeq, NodeId, TraceId};

/// A Windows process identifier, used to name the application an outpost
/// watches and to key supervisor state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pid(pub u32);

impl std::fmt::Display for Pid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A native window handle, as an opaque number.
///
/// Window handles are the one identity that means the same thing in every
/// process, so they are how windows are compared across outposts (node ids
/// are per outpost).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowHandle(pub u64);

/// No window, the default of a window handle a message left out.
fn no_window() -> WindowHandle {
    WindowHandle(0)
}

/// Facts about the window an event concerns, read by its outpost with local
/// calls when the event is observed. The reducer classifies every event
/// against its attention record with these (decision D14;
/// `docs/parity/focus-events.md`, "Event acceptance").
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of one read, each a yes or no on the wire"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowFacts {
    /// The event's top-level window.
    pub top_level: WindowHandle,
    /// The top of the event window's owner chain.
    pub root_owner: WindowHandle,
    /// Whether the window or its top-level window is topmost (menus, combo
    /// box popups, the task switcher).
    pub topmost: bool,
    /// For a `Windows.UI.Core` window only: whether it is the input thread's
    /// active window or inside it, the test NVDA uses for UWP windows.
    /// `None` for every other window class.
    #[serde(default)]
    pub under_active_window: Option<bool>,
    /// Whether the window is in the system's foreground window when the
    /// event is read: its top-level window is the foreground window, or its
    /// root owner is the foreground window or the foreground window's root
    /// owner — NVDA's live foreground test. Windows can raise a window's
    /// foreground event while refusing it the foreground, and raises none
    /// when it is given the foreground later, so this is how the reducer
    /// learns the foreground moved without a foreground fact.
    #[serde(default)]
    pub in_foreground: bool,
    /// The system's foreground window when the event was read, from the
    /// same `GetForegroundWindow` as `in_foreground`, so no call is added:
    /// a fact of this report, by which Core decides whether the focus has
    /// entered the window in front (`docs/parity/focus-context.md`,
    /// "Window announcement on switching applications"). It is never the
    /// foreground record, which only the focus listener's foreground
    /// change sets.
    #[serde(default = "no_window")]
    pub foreground_window: WindowHandle,
    /// The class of the event's own window, which Core's rules for an
    /// application's windows match, as NVDA's app modules match a window's
    /// class (decision D6). Only a focus change, the one event those rules
    /// match, and a focus-now answer carry it; it is empty on every other
    /// event, and left out of the message, so that they carry no string.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub class: String,
    /// The application the event belongs to, by the name NVDA gives an
    /// application module: its executable's file name without the
    /// extension, lower-cased, such as `explorer` for File Explorer, and for
    /// a console window its client's. Core's rules for an application's
    /// elements match it (decision D6). Only a focus change carries it; it is
    /// empty on every other event and on a focus-now answer, and when the
    /// name could not be read.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub application: String,
    /// Whether Alt was held when the event's window facts were read: a fact
    /// of the keyboard, not of the window, read at the same time and for
    /// the same rules, since one of them changes what a focus says while
    /// Alt is held (`docs/parity/windows-and-processes.md`, "The Alt+Tab
    /// switcher's pane"). Only a focus change carries it; `false` on every
    /// other event and on a focus-now answer, and left out of the message
    /// when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alt_held: bool,
    /// Whether `foreground_window` is Program Manager's or the taskbar's
    /// (class `Progman` or `Shell_TrayWnd`), read with it: Core's title
    /// rule says no title for them, since NVDA takes no foreground event
    /// for them (`docs/parity/focus-context.md`, "Window announcement on
    /// switching applications"). Left out of the message when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub foreground_is_shell: bool,
}

/// What the keyboard hook needs to know of the foreground window, decided
/// by Core from the foreground report the focus listener sent
/// ([`crate::request::ForegroundReport`]) and
/// read by the hook from a lock-free snapshot (architecture section 5). The
/// hook applies it only while `window` is still the foreground window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookForeground {
    /// The foreground window this was decided for.
    pub window: WindowHandle,
    /// Whether the hook may hold keys back for this window: not when its
    /// process has higher integrity than Verbatim's, or either cannot be
    /// read, since Windows drops a key Verbatim sends again to such a
    /// window (`books/dev/src/explanation/design/key-holding.md`, "Elevated windows").
    pub holds_keys: bool,
    /// Whether the focus has text whose caret Verbatim reads, and whether
    /// that is a terminal's, so that the hook holds its caret keys back
    /// while `holds_keys` lets it, and Enter outside a terminal
    /// (`books/dev/src/explanation/design/key-holding.md`).
    pub focus_text: FocusTextKind,
    /// The thread whose keyboard layout translates the keys typed into
    /// this window, when it is not the thread Windows names as the
    /// window's owner: a console host window's own thread
    /// (`books/dev/src/reference/crates/verbatim-input-windows.md`, "Typed text").
    pub keyboard_thread: Option<u32>,
    /// The view's desktop part: how many times Core has handled a switch
    /// away from the user's input desktop. The hook gets no release of a
    /// key down before such a switch, so when the number changes it
    /// clears its modifier record and its line, as Core drops its held
    /// keys in the same step (`docs/design/readers-tell-core-asks-build1.md`,
    /// "The secure desktop and the lock state").
    #[serde(default)]
    pub desktop_switches: u32,
}

/// What text the focus has, as the keyboard hook holds keys for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FocusTextKind {
    /// None whose caret Verbatim reads: no key is held.
    #[default]
    None,
    /// A text field other than a terminal: the caret keys and Enter are
    /// held.
    Field,
    /// A terminal: the caret keys are held, and Enter, which a terminal
    /// takes as typed text, is not (NVDA's terminals announce no new
    /// line).
    Terminal,
}

impl FocusTextKind {
    /// Whether the focus has text whose caret Verbatim reads.
    #[must_use]
    pub fn has_text(self) -> bool {
        self != Self::None
    }
}

/// A property change carried by [`NormalizedEvent::PropertyChanged`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PropertyChange {
    /// The accessible name changed.
    Name(Option<String>),
    /// The value changed (also has a dedicated event; kept here for
    /// backends that report it as a generic property change).
    Value(Option<String>),
    /// The state set changed, carrying the complete new set (not a delta) —
    /// sourced from MSAA `EVENT_OBJECT_STATECHANGE` and UIA state-bearing
    /// property changes. The reducer diffs against its stored snapshot to
    /// decide what to announce (a check box toggling, a control becoming
    /// unavailable).
    States(StateSet),
    /// The description changed (MSAA `EVENT_OBJECT_DESCRIPTIONCHANGE`),
    /// carrying the new description; `None` also when the active theme does
    /// not read descriptions.
    Description(Option<String>),
    /// An editable field's `ControllerFor` relation changed: `true` when
    /// it now names elements, a list of suggestions having appeared, and
    /// `false` when it names none, the list gone (`docs/parity/changes-and-selection.md`,
    /// "Suggestions appearing and going away").
    Suggestions(bool),
}

/// What a reader read live, with a focus, of the element it held before
/// under the focus's identity, a UI Automation runtime id an application
/// may give to a new element once the one it named dies
/// (`books/dev/src/reference/adr/0001-focus.md`, "Object identity").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EarlierElement {
    /// It still has the keyboard focus: the focus is that element.
    Focused,
    /// It was read as not having the keyboard focus, or, in a remote
    /// program, its keyboard-focus read answered no boolean: it is not
    /// taken as the focus, though it may be alive.
    NotFocused,
    /// Its keyboard focus could not be read, other than by time: it is
    /// gone, and the focus is another element.
    Gone,
}

/// An accessibility event, normalized by an outpost from either backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum NormalizedEvent {
    /// Keyboard focus moved to a node.
    FocusChanged {
        /// Snapshot of the newly focused node.
        node: NodeSnapshot,
        /// Whether this is a whole-window focus: `node` is a top-level
        /// window standing for the application's focus in it, no control in
        /// it read, as an outpost reports a foreground change into a window
        /// it reads through MSAA, following NVDA, whose foreground event
        /// is a focus on the window, and the held UIA application's title.
        /// Core cannot tell one from its node, since an MSAA focus on a
        /// window's client object reads as the same node; it judges it by
        /// its foreground record, never moves attention for it, and
        /// refuses it when its focus is in that window and not left
        /// (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the window title").
        #[serde(default)]
        whole_window: bool,
        /// The focused node's ancestors, outermost first, complete to the
        /// desktop, which is not one of them: walked by the outpost before
        /// emitting, and where a walk was cut by its bound, failed or met a
        /// window it does not call into, completed by plain windows read
        /// from Windows' own state (`books/dev/src/reference/adr/0001-focus.md`, "Object
        /// identity"). The readers leave out
        /// the ancestors that are not presentable, layout containers among
        /// them, so it is empty for a top-level window's own focus and can
        /// be for any focus whose ancestors are all such; the focus's
        /// window facts name its top-level window. Carried on the event rather than
        /// fetched afterward so the reducer can speak entered containers
        /// *before* the control, in NVDA's order, joining this chain to
        /// the previous focus's where they meet.
        ancestors: Vec<NodeSnapshot>,
        /// For a focus that may have text, what the outpost's caret read
        /// found, made while it handled the focus, so the line at the caret
        /// comes in the same message as the name, role and states and
        /// nothing is spoken between them, as NVDA's object announcement
        /// reads an object's text within it (`docs/nvda/speech.md`, "What
        /// an object with text says"). `None` for a focus that has no
        /// text and for a foreground change.
        /// Boxed, as the rest of the message is large already.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<Box<FocusText>>,
        /// For a UI Automation focus whose node the reader held an element
        /// for, what it read of that element with this focus; `None` when
        /// it held none, or its read was cut before that element's.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        earlier: Option<EarlierElement>,
    },
    /// A property of a node changed.
    PropertyChanged {
        /// The node whose property changed.
        node_id: NodeId,
        /// Which property, with its new value.
        change: PropertyChange,
        /// The node's number of children, read with the change only where
        /// speech needs it: a change of states that leaves a Win32 tree
        /// view item expanded, whose count is spoken when the change makes
        /// it expanded (`docs/nvda/speech.md`, "How many items an expanded
        /// tree view item holds"). `None` everywhere else.
        child_count: Option<u32>,
    },
    /// The value of a node changed (slider drag, combo selection, text edit).
    ValueChanged {
        /// The node whose value changed.
        node_id: NodeId,
        /// The new value.
        value: Option<String>,
    },
    /// The caret or the selection moved in a node with text (UIA's text
    /// selection changed event, an edit control's caret event), carrying
    /// the line at the caret and the selection (milestone M4). The outpost
    /// sends one on every caret or selection change in the focus,
    /// coalesced so a burst sends the latest only; the focus's own caret
    /// comes with the focus ([`FocusText`]). It keeps Core's copy of the
    /// caret current, and speaks only the selection changes it carries, a
    /// selection no key made
    /// (the mouse's or the application's), as NVDA reports any selection
    /// change of the focus after its caret event
    /// (`docs/nvda/editable-text-and-terminals.md`, "Selection changes").
    CaretMoved {
        /// The node whose caret moved.
        node_id: NodeId,
        /// Where the caret now is.
        caret: CaretReport,
        /// How the selection changed from the one the outpost last reported
        /// for the node, by the rule of
        /// [`SelectionChange`](crate::SelectionChange). Always empty for a terminal, and for the
        /// first report after Verbatim itself selected or moved the caret.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        selection_changes: Vec<SelectionChange>,
    },
    /// The text of a node changed (UIA's text changed event, an edit
    /// control's change notification). Characters typed into a terminal
    /// wait for this before they are spoken, so a password prompt that
    /// shows nothing speaks nothing (`docs/parity/terminals.md`, "Typing").
    TextChanged {
        /// The node whose text changed.
        node_id: NodeId,
    },
    /// One read of a focused terminal (`books/dev/src/explanation/design/terminal-model.md`,
    /// "Where it runs"): its text, screen and caret, which Core's terminal
    /// model aligns and its reading rules judge. Sent in place of
    /// [`TextChanged`](Self::TextChanged) for a terminal the outpost reads
    /// live, unless it is identical to the last read sent.
    TerminalSnapshot {
        /// The terminal.
        node_id: NodeId,
        /// The read.
        snapshot: Box<crate::TerminalSnapshot>,
    },
}

/// A toast's identity: its UIA runtime id, which Core compares with the
/// last toast's to drop a repeat, as NVDA compares the runtime id itself
/// (`docs/parity/alerts.md`, "Toasts").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToastId(pub crate::property::RuntimeId);

/// What kind of change a UI Automation notification reports: UIA's
/// `NotificationKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum NotificationKind {
    /// An item was added.
    ItemAdded,
    /// An item was removed.
    ItemRemoved,
    /// An action completed.
    ActionCompleted,
    /// An action was aborted.
    ActionAborted,
    /// Any other kind of notification.
    Other,
}

/// How urgently a UI Automation notification should be processed: UIA's
/// `NotificationProcessing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum NotificationProcessing {
    /// Important; process every notification of this kind.
    ImportantAll,
    /// Important; only the most recent notification of this kind matters.
    ImportantMostRecent,
    /// Process every notification of this kind.
    All,
    /// Only the most recent notification of this kind matters.
    MostRecent,
    /// Process the current notification, then only the most recent of any
    /// further ones that arrive while it is being processed.
    CurrentThenMostRecent,
}

/// What to fetch: one object-navigation step from a node to a neighbor
/// (roadmap M3). Later milestones add text ranges and subtree queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum QueryKind {
    /// The node's parent.
    Parent,
    /// The node's next sibling in tree order.
    NextSibling,
    /// The node's previous sibling in tree order.
    PreviousSibling,
    /// The node's first child.
    FirstChild,
}

/// One input to the reducer. Strictly accessibility-shaped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Input {
    /// A review or object-navigation command, from a bound gesture
    /// (roadmap M3). The imperative shell translates a keyboard script
    /// into this; the reducer runs it against its navigator object and
    /// review cursor.
    Command {
        /// Trace ID minted when the triggering key was observed, carried
        /// through so a command's speech joins the latency timeline.
        trace_id: TraceId,
        /// The gesture's key number, which its speech carries.
        key_seq: KeySeq,
        /// Which command.
        command: ReviewCommand,
        /// Milliseconds on the run's shared clock when the hook saw the
        /// gesture, which bounds its wait behind a held key's watch; 0 when
        /// unknown.
        #[serde(default)]
        pressed_at_ms: u64,
        /// How many times the gesture was pressed in quick succession, zero
        /// for the first press: report-current-object reports on 0, spells
        /// on 1, copies on 2 (NVDA's multi-press semantics); the
        /// current-line, current-word, and current-character review
        /// commands spell on a repeat, and a third press of current
        /// character speaks its character code. Other commands ignore it.
        repeat: u8,
    },
    /// A bound gesture whose script is not a review command: the menu, the
    /// time, or the shell's lists. Run in the order pressed, behind a held
    /// key as a review command is.
    Script {
        /// Trace ID minted when the gesture was observed.
        trace_id: TraceId,
        /// The gesture's key number, which its speech carries.
        key_seq: KeySeq,
        /// How many times the gesture was pressed in quick succession, zero
        /// for the first press: the time on 0 and the date after; the
        /// system tray on 0 and the taskbar after.
        repeat: u8,
        /// Which script.
        script: Script,
        /// As [`Input::Command`]'s.
        #[serde(default)]
        pressed_at_ms: u64,
    },
    /// A lock key reached the operating system: its new state is announced
    /// 30 ms after `pressed_at_ms`, once Windows has changed it: the shell
    /// reads the key's state at the due time Core's state gives, then
    /// enters [`Input::LockKeyToggled`].
    ToggleKeyPressed {
        /// Trace ID minted when the key was observed.
        trace_id: TraceId,
        /// The key's number. The announcement does not carry it, so a key
        /// pressed within the 30 ms does not silence it.
        key_seq: KeySeq,
        /// Which key.
        toggle: LockKey,
        /// Milliseconds on the run's shared clock when the hook saw it.
        pressed_at_ms: u64,
    },
    /// An input to the event queue (ADR 0003), with the trace it runs
    /// under and its time on the run's shared clock.
    Queue {
        /// The trace minted where it was observed, or by the shell for
        /// its own inputs.
        trace_id: TraceId,
        /// When, in microseconds on the run's shared clock.
        at_us: u64,
        /// The input.
        input: crate::request::QueueInput,
    },
    /// Periodic timer tick, for time-based policies. Unused by M1 logic but
    /// part of the frozen vocabulary so adding policies is not a breaking
    /// change.
    Tick,
    /// A caret key was pressed and passed to the application, which moves
    /// the caret itself (milestone M4). The keyboard hook reports it as it
    /// passes it on; the reducer waits for evidence of what it did and
    /// speaks the result when the focus has text.
    CaretKey {
        /// Trace ID minted when the key was observed.
        trace_id: TraceId,
        /// The key's number, which its speech carries.
        key_seq: KeySeq,
        /// Which key.
        key: CaretKey,
        /// Milliseconds since the Unix epoch when the hook saw the key,
        /// before the application could, on the run's shared clock that an
        /// event's `observed_at_us` reads, so an outpost can tell whether a
        /// caret it read came before the key. 0 when unknown.
        #[serde(default)]
        pressed_at_ms: u64,
    },
    /// A caret key the keyboard hook held back from the application, as
    /// NVDA's caret scripts hold theirs (`books/dev/src/explanation/design/key-holding.md`): the
    /// reducer has the focus's outpost read the caret before the key, then
    /// passes the key on ([`Effect::PassHeldKey`]) and watches for its
    /// evidence from that exact caret, as for [`Input::CaretKey`]. The key
    /// is always passed on, whatever the read's outcome.
    HeldKey {
        /// Trace ID minted when the key was observed; it also names the
        /// key to pass on.
        trace_id: TraceId,
        /// The key's number, which its speech carries.
        key_seq: KeySeq,
        /// Which key.
        key: CaretKey,
        /// As [`Input::CaretKey`]'s: when the hook saw the key, on the
        /// outposts' clock; 0 when unknown.
        #[serde(default)]
        pressed_at_ms: u64,
    },
    /// Text was typed into the focused application: one character, or
    /// several at once when an input method commits a composition (one key
    /// press need not be one character). Sourced on the platform side,
    /// from the keyboard hook's translation of a key to text or from the
    /// application's own text-edit events; the reducer echoes it by the
    /// typing echo settings.
    CharacterTyped {
        /// Trace ID minted when the typing was observed.
        trace_id: TraceId,
        /// The number of the key that typed it, which its speech carries.
        key_seq: KeySeq,
        /// The text typed, a tab as a tab character and Enter as a carriage
        /// return.
        text: String,
        /// Milliseconds on the run's shared clock when the hook saw the
        /// key, the clock of a caret key's `pressed_at_ms`; 0 when unknown.
        #[serde(default)]
        at_ms: u64,
    },
    /// A key typed a control character into the focused application, one
    /// other than a tab, a carriage return, Backspace's and
    /// Control+Backspace's (Control+C, Control+Enter; never Escape): the word
    /// being typed ends and is spoken by the typing echo settings, and
    /// nothing is spelled (`docs/parity/input.md`, "Typed-character
    /// echo"). It follows the key's other inputs.
    ControlTyped {
        /// Trace ID minted when the key was observed.
        trace_id: TraceId,
        /// The key's number, which its speech carries.
        key_seq: KeySeq,
        /// As [`Input::CharacterTyped`]'s.
        at_ms: u64,
    },
    /// A key that ends or clears the command line being typed, Escape,
    /// Control+C, Control+D, or Control+Break, was passed to the focused
    /// application. A terminal forgets the typing it holds, which it never
    /// showed (a password perhaps), as NVDA forgets its queued characters
    /// (`docs/parity/terminals.md`, "New terminal output"). These keys type no
    /// text, so they never arrive as [`Input::CharacterTyped`].
    ClearingKey {
        /// Trace ID minted when the key was observed.
        trace_id: TraceId,
        /// The key's number, which its speech carries.
        key_seq: KeySeq,
        /// As [`Input::CharacterTyped`]'s: when the hook saw the key; 0
        /// when unknown.
        #[serde(default)]
        at_ms: u64,
    },
    /// An editing chord, Control+Z, Control+Y, Control+X, Control+V or
    /// Escape, was passed to the focused application. In a text field it
    /// says nothing for the key, its selection changes being spoken from the
    /// application's caret event; in a terminal it is any other key passed
    /// (`docs/design/caret-selection-rework.md`).
    EditingChord {
        /// Trace ID minted when the key was observed.
        trace_id: TraceId,
        /// The key's number, which its speech carries.
        key_seq: KeySeq,
        /// As [`Input::CaretKey`]'s: when the hook saw the key.
        pressed_at_ms: u64,
    },
    /// The reader settings, at startup and whenever the user changes them.
    Settings(ReaderSettings),
    /// The details the active theme reports ([`crate::ReportedDetails`]),
    /// at startup and whenever the theme in use changes: Core chooses each
    /// read's details from them.
    Reported(crate::ReportedDetails),
    /// An utterance the reducer placed index marks in was cut off before
    /// playback reached all of them (a key, an interrupting utterance, the
    /// focus moving on, a synthesizer switch). Reported by the speech
    /// pipeline for every such utterance, however it was cut, so nothing
    /// the reducer waits on is lost by a cut: what waited on it stops.
    UtteranceCancelled {
        /// The marks the utterance carried, in order; those playback had
        /// already reached were reported as
        /// [`QueueInput::MarkReached`](crate::request::QueueInput::MarkReached) first.
        marks: Vec<SpeechMark>,
        /// When it ended, in milliseconds since the Unix epoch, the clock
        /// outposts stamp events with; 0 when unknown.
        #[serde(default)]
        at_ms: u64,
    },
    /// An utterance the reducer placed index marks in failed in synthesis
    /// or audio output before playback reached all of them. It counts as
    /// heard: what waited on its marks carries on past it, as NVDA's speech
    /// manager does when a later index arrives (Dickson, 2026-10-24).
    UtteranceFailed {
        /// The marks the utterance carried, in order; those playback had
        /// already reached were reported as
        /// [`QueueInput::MarkReached`](crate::request::QueueInput::MarkReached) first.
        marks: Vec<SpeechMark>,
    },
    /// Verbatim has started: its speech pipeline is up and the keyboard
    /// hook installed.
    Started,
    /// Verbatim is exiting, its GUI loop ended; `replaced` when a new
    /// instance is replacing it.
    Exiting {
        /// Whether a new instance is replacing this one.
        replaced: bool,
    },
    /// An error was logged, on any thread.
    ErrorLogged,
    /// A lock key's state changed: read once the key had reached Windows
    /// and Windows had changed it.
    LockKeyToggled {
        /// Trace ID minted when the state was read.
        trace_id: TraceId,
        /// Which key.
        key: LockKey,
        /// Whether it is now on.
        on: bool,
    },
    /// The report-time gesture was pressed: the current time and date, as
    /// the operating system formats them for the user's locale, read as
    /// the gesture arrived; `None` for one that could not be formatted.
    ClockRead {
        /// Trace ID minted when the key was observed.
        trace_id: TraceId,
        /// How many times the gesture was pressed in quick succession, zero
        /// for the first press.
        repeat: u8,
        /// The time, without seconds.
        time: Option<String>,
        /// The long date.
        date: Option<String>,
    },
    /// A copy to the clipboard the reducer asked for
    /// ([`Effect::CopyToClipboard`]) was made, or failed.
    ClipboardCopied {
        /// Trace ID of the input that asked for the copy.
        trace_id: TraceId,
        /// The text copied.
        text: String,
        /// Whether the clipboard holds the text, as read back.
        copied: bool,
    },
    /// The shell item list dialog's chosen item was gone when it would have
    /// been clicked.
    ShellItemGone {
        /// Trace ID minted when the dialog found it gone.
        trace_id: TraceId,
        /// The item's name, as the list showed it.
        name: String,
    },
    /// The shell item list dialog's chosen item was there when it would
    /// have been clicked, but off the screen or with no rectangle or an
    /// empty one, so it was not clicked.
    ShellItemUnclickable {
        /// Trace ID minted when the dialog found it unclickable.
        trace_id: TraceId,
        /// The item's name, as the list showed it.
        name: String,
    },
    /// The settings dialog or Exit was asked for from the GUI while a
    /// modal message box is up, and refused.
    BlockedByMessageBox {
        /// Trace ID minted when the GUI refused it.
        trace_id: TraceId,
    },
    /// The settings dialog's theme page previews an indication spoken: a
    /// sample, in the theme's words.
    PreviewSpeech {
        /// Trace ID minted when the preview was asked for.
        trace_id: TraceId,
        /// The sample.
        segments: Vec<crate::UtteranceSegment>,
    },
    /// The settings dialog's theme page previews an event, as the theme
    /// reports it.
    PreviewEarcon(Earcon),
    /// The quiet-period timer of the focused terminal's reading rules came
    /// due, at the due time Core's state gives, which the shell reads after
    /// each step: the rules are given the keys that reached the program by
    /// `at_ms`, then the timer.
    TerminalTimer {
        /// Trace ID minted when the shell found it due.
        trace_id: TraceId,
        /// When it came due, in milliseconds on the clock every process of
        /// a run shares.
        at_ms: u64,
    },
    /// A progress bar's value changed, from the focus listener, which is
    /// the one owner of every bar's changes.
    Progress {
        /// A trace the shell mints for it, which its indication carries.
        trace_id: TraceId,
        /// What changed.
        change: BarChange,
    },
}

/// A progress bar's percentage, 0 to 100, kept in hundredths of a percent
/// and always in range: made only by parsing a bar's value at the boundary
/// (`docs/parity/changes-and-selection.md`, "Progress bars"), so what
/// receives one has nothing left to check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u16", into = "u16")]
pub struct Percent(u16);

impl TryFrom<u16> for Percent {
    type Error = String;

    fn try_from(hundredths: u16) -> Result<Self, String> {
        if hundredths <= Self::FULL {
            Ok(Self(hundredths))
        } else {
            Err(format!("{hundredths} hundredths is past 100 percent"))
        }
    }
}

impl From<Percent> for u16 {
    fn from(percent: Percent) -> u16 {
        percent.0
    }
}

impl Percent {
    /// 100 percent, in hundredths of a percent.
    const FULL: u16 = 10_000;

    /// The percentage `fraction` is of the whole, in hundredths, held
    /// between 0 and 100; `None` for one that is not a finite number.
    fn of_fraction(fraction: f64) -> Option<Self> {
        if !fraction.is_finite() {
            return None;
        }
        let hundredths = (fraction * f64::from(Self::FULL)).round();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::as_conversions,
            reason = "clamped to 0 and 10,000 just before, so the conversion is exact"
        )]
        let hundredths = hundredths.clamp(0.0, f64::from(Self::FULL)) as u16;
        Some(Self(hundredths))
    }

    /// The percentage `value` is along the range from `minimum` to
    /// `maximum`, rounded to a whole percent, as NVDA's progress bar overlay
    /// reads a UIA bar's range value; with no range, `value` itself, as the
    /// pattern gave it, as that overlay uses it. `None` for values that are
    /// not finite numbers.
    #[must_use]
    pub fn of_range(value: f64, minimum: f64, maximum: f64) -> Option<Self> {
        #[expect(
            clippy::float_cmp,
            reason = "an exact comparison, as NVDA's overlay compares them"
        )]
        let percentage = if minimum == maximum {
            value
        } else {
            (value - minimum) / (maximum - minimum) * 100.0
        };
        Self::of_fraction(percentage.round() / 100.0)
    }

    /// The percentage `position` is along the integers from `low` to `high`,
    /// as a classic progress bar reports its range; `None` for an empty or
    /// reversed range.
    #[must_use]
    pub fn of_position(position: i32, low: i32, high: i32) -> Option<Self> {
        if high <= low {
            return None;
        }
        let span = f64::from(high) - f64::from(low);
        Self::of_fraction((f64::from(position) - f64::from(low)) / span)
    }

    /// The percentage a bar's text value says, as NVDA reads it: a number
    /// once any percent signs and null characters at either end are
    /// removed, held between 0 and 100. `None` for anything else.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let number = text
            .trim_matches(|c| c == '%' || c == '\0')
            .trim()
            .parse::<f64>()
            .ok()?;
        Self::of_fraction(number / 100.0)
    }

    /// The percentage in hundredths of a percent.
    #[must_use]
    pub fn hundredths(self) -> u32 {
        u32::from(self.0)
    }

    /// The percentage as a whole number, rounded down.
    #[must_use]
    pub fn whole(self) -> u8 {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::as_conversions,
            reason = "at most 100, by the type's invariant"
        )]
        let whole = (self.0 / 100) as u8;
        whole
    }
}

/// A progress bar's value changed, as the focus listener reports it
/// (`docs/design/progress-bars.md`): the bar is on screen and its value a
/// number, since a report is made of nothing else, and Core judges only
/// whether it is heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BarChange {
    /// Its percentage.
    pub percent: Percent,
    /// The centre of the bar, by which its last percentage is kept.
    pub place: (i32, i32),
    /// Whether its window is the foreground window or beneath it.
    pub in_foreground: bool,
}

/// What a [`Input::Script`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Script {
    /// Open the Verbatim menu.
    ShowMenu,
    /// Report the time, or the date on a quick second press.
    ReportTime,
    /// List the system tray's items, or the taskbar's on a quick second
    /// press.
    ShellList,
}

/// A window the reducer asks the GUI to show ([`Effect::Show`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuiRequest {
    /// The Verbatim menu.
    Menu,
    /// The list of the shell surface's items.
    ShellList(ShellItemKind),
}

impl Input {
    /// The trace the input carries, for the inputs that carry one: the
    /// trace the shell executes the input's effects under, and Core's
    /// stamp looks a key number up by.
    #[must_use]
    pub fn trace_id(&self) -> Option<TraceId> {
        match self {
            Self::Command { trace_id, .. }
            | Self::Script { trace_id, .. }
            | Self::ToggleKeyPressed { trace_id, .. }
            | Self::CaretKey { trace_id, .. }
            | Self::HeldKey { trace_id, .. }
            | Self::CharacterTyped { trace_id, .. }
            | Self::ControlTyped { trace_id, .. }
            | Self::ClearingKey { trace_id, .. }
            | Self::EditingChord { trace_id, .. }
            | Self::LockKeyToggled { trace_id, .. }
            | Self::ClockRead { trace_id, .. }
            | Self::ClipboardCopied { trace_id, .. }
            | Self::ShellItemGone { trace_id, .. }
            | Self::ShellItemUnclickable { trace_id, .. }
            | Self::BlockedByMessageBox { trace_id }
            | Self::PreviewSpeech { trace_id, .. }
            | Self::Progress { trace_id, .. }
            | Self::Queue { trace_id, .. } => Some(*trace_id),
            _ => None,
        }
    }
}

/// A lock key whose state Verbatim announces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LockKey {
    /// Caps Lock.
    CapsLock,
    /// Num Lock.
    NumLock,
    /// Scroll Lock.
    ScrollLock,
}

/// The name of an action an activation performed, which NVDA speaks after
/// performing it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ActionName {
    /// UIA's Invoke pattern, which NVDA names "invoke".
    Invoke,
    /// The application's own name for an object's default action (MSAA's
    /// default action, such as "Press"), spoken as it is.
    Named(String),
}

/// A review-cursor or object-navigation command (roadmap M3), the
/// model-level vocabulary the keyboard layer's scripts map onto so the
/// reducer never depends on input-crate types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReviewCommand {
    /// Report the current navigator object (spell on the second press in a
    /// streak, copy its name and value on the third).
    ReportObject,
    /// Move the navigator object to its parent.
    Parent,
    /// Move the navigator object to its next sibling.
    NextSibling,
    /// Move the navigator object to its previous sibling.
    PreviousSibling,
    /// Move the navigator object to its first child.
    FirstChild,
    /// Move the navigator object (and review cursor) back to the focus.
    ToFocus,
    /// Activate the current navigator object (invoke, toggle, or default
    /// action).
    Activate,
    /// Move the review cursor to the first line of the navigator object.
    ReviewTop,
    /// Move the review cursor to the previous line.
    ReviewPreviousLine,
    /// Report the review cursor's current line: spelled on a second press,
    /// spelled with character descriptions on a third.
    ReviewCurrentLine,
    /// Move the review cursor to the next line.
    ReviewNextLine,
    /// Move the review cursor to the previous word.
    ReviewPreviousWord,
    /// Report the review cursor's current word.
    ReviewCurrentWord,
    /// Move the review cursor to the next word.
    ReviewNextWord,
    /// Move the review cursor to the start of the current line.
    ReviewStartOfLine,
    /// Move the review cursor to the previous character.
    ReviewPreviousCharacter,
    /// Report the review cursor's current character.
    ReviewCurrentCharacter,
    /// Move the review cursor to the next character.
    ReviewNextCharacter,
    /// Move the review cursor to the end of the current line.
    ReviewEndOfLine,
    /// Move the review cursor to the last line of the navigator object.
    ReviewBottom,
    /// Move the review cursor to the previous page, where the text has
    /// pages.
    ReviewPreviousPage,
    /// Move the review cursor to the next page.
    ReviewNextPage,
    /// Move the review cursor to the first character of the selection.
    ReviewSelectionStart,
    /// Move the review cursor to the last character of the selection.
    ReviewSelectionEnd,
    /// Read from the review cursor to the end, moving it as speech goes.
    SayAllFromReview,
    /// Read from the caret to the end, moving the caret as speech goes.
    SayAllFromCaret,
    /// Mark the review cursor's position as the start of a select then copy.
    SetStartMarker,
    /// Move the review cursor to the start marker.
    MoveToStartMarker,
    /// Select from the start marker to the review cursor; on a second press,
    /// copy that text to the clipboard.
    SelectThenCopy,
    /// Toggle whether the review cursor follows the caret.
    ToggleFollowCaret,
    /// Cycle the "Speak typed characters" setting.
    ToggleTypedCharacters,
    /// Cycle the "Speak typed words" setting.
    ToggleTypedWords,
    /// Toggle "Report new output" in terminals.
    ToggleReportNewOutput,
    /// Report where the caret is on the screen.
    ReportCaretLocation,
    /// Report where the review cursor is on the screen.
    ReportReviewLocation,
    /// Report the focus object, wherever the navigator is: spell its name
    /// on the second press in a streak, with character descriptions on the
    /// third and later.
    ReportFocus,
    /// Report the status bar: in a terminal, its footer
    /// (`books/dev/src/explanation/design/terminal-model.md`, "Features and settings"); spelled
    /// on the second press in a streak, copied to the clipboard on the
    /// third.
    ReportStatusBar,
    /// Move the review cursor to the previous command mark in a terminal
    /// and speak its line (`books/dev/src/explanation/design/terminal-model.md`, "Features and
    /// settings").
    PreviousCommandMark,
    /// Move the review cursor to the next command mark in a terminal.
    NextCommandMark,
}

/// An event Verbatim indicates at once, outside the speech queue, named
/// semantically so the active theme decides how it is reported: a sound
/// played immediately on its own mixer source, words spoken, both, or
/// nothing (`books/dev/src/reference/crates/verbatim-model.md`, "Themes"). Each is an indication of the
/// catalogue's Events category ([`crate::Indication::of_earcon`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Earcon {
    /// A deadline expired inside a cross-process accessibility call: the
    /// application is not responding and the reducer proceeded with stale
    /// data (architecture section 1, recovery ladder rung one).
    AppNotResponding,
    /// Verbatim has started.
    Start,
    /// Verbatim is exiting.
    Exit,
    /// An error was logged.
    Error,
    /// Browse mode was turned on.
    BrowseMode,
    /// Focus mode was turned on.
    FocusMode,
    /// A list of suggestions appeared for the focused field.
    SuggestionsOpened,
    /// The list of suggestions went away.
    SuggestionsClosed,
    /// A progress bar's value changed; the percentage, from 0 to 100, sets
    /// the pitch of its tone.
    Progress(u8),
}

/// One effect emitted by the reducer and executed by the imperative shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    /// Queue an utterance in the speech pipeline, with the number of the
    /// user input that caused it, if one did: Core stamps it as its last
    /// step, and every other construction gives `None`.
    Speak(Utterance, Option<KeySeq>),
    /// Cancel current and queued speech.
    StopSpeech,
    /// What Core's event queue asks of the shell.
    Queue(crate::request::QueueEffect),
    /// The focus has changed: stop focus speech being spoken that is no
    /// longer valid ([`crate::FocusValidity`]), and judge waiting focus
    /// speech against this focus when its turn comes.
    DropExpiredSpeech(crate::FocusNow),
    /// Indicate an event at once, as the active theme reports it: a sound
    /// on its own mixer source, words, both, or nothing.
    PlayEarcon(Earcon),
    /// Copy text to the system clipboard through the shell's shared
    /// clipboard helper, whose result comes back as
    /// [`Input::ClipboardCopied`], which the reducer confirms in speech.
    /// The reducer stays pure — it never touches the clipboard itself — so
    /// the report-object triple-press emits this rather than doing the copy.
    CopyToClipboard(String),
    /// Keep the display on (true) while say-all reads, or let it turn off
    /// again (false), NVDA's "Prevent display from turning off during say
    /// all". Every true is followed by a false when say-all ends.
    KeepDisplayOn(bool),
    /// The reducer changed a reader setting itself, by a toggle key; the
    /// shell saves the new settings.
    SettingsChanged(ReaderSettings),
    /// The listener's configuration: what it judges its own reads under,
    /// sent at every [`Input::Settings`] whatever its value, carrying the
    /// value as it stands, so the last to arrive is current. No toggle key
    /// changes the setting it carries; one that did would have to emit it
    /// too. Whether background progress bars are
    /// reported, by which the listener skips the read of a value change
    /// whose window the acceptance test refuses (Dickson, 2026-11-19).
    ListenerConfiguration {
        /// [`ReaderSettings::report_background_progress_bars`].
        report_background_progress_bars: bool,
    },
    /// The effect on behalf of the key with this trace ID, whatever input
    /// caused it: a key that waited behind a held key is handled while the
    /// reducer handles the held key's read (the read before a held key, or
    /// a later key's own effects), and its requests and speech must carry
    /// its own key's trace, so that its answers and speech are that key's.
    ForKey(TraceId, Box<Effect>),
    /// Read the time and date, for the report-time gesture: the shell
    /// enters [`Input::ClockRead`] with the trace it executed this under.
    ReadClock {
        /// The gesture's repeat count, handed back on the input.
        repeat: u8,
    },
    /// Show a window of the GUI.
    Show(GuiRequest),
    /// An event Core took, for the control plane's event stream, which
    /// carries it as version 4 of the control protocol did: the shell
    /// fans it out to the connections subscribed to events, and does
    /// nothing else with it.
    Observed(Box<Observed>),
}

/// An event Core took ([`Effect::Observed`]), as the control plane's event
/// stream carries it: the event, under the trace of the input that caused
/// it, the process it is of, the backend that read it, and its window's
/// facts when Core has them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observed {
    /// The trace of the input that caused it.
    pub trace_id: TraceId,
    /// The process it is of.
    pub source: Pid,
    /// The backend that read it.
    pub backend: Backend,
    /// Its window's facts, when Core has them.
    pub window: Option<WindowFacts>,
    /// The event.
    pub event: NormalizedEvent,
}

impl Observed {
    /// [`Effect::Observed`] of `event`, of `source`, read by `backend`,
    /// under `trace_id`, with `window`'s facts.
    #[must_use]
    pub fn effect(
        trace_id: TraceId,
        (source, backend): (Pid, Backend),
        window: Option<WindowFacts>,
        event: NormalizedEvent,
    ) -> Effect {
        Effect::Observed(Box::new(Self {
            trace_id,
            source,
            backend,
            window,
            event,
        }))
    }
}

#[cfg(test)]
mod percent_tests {
    use super::Percent;

    /// A UIA bar's range: a range of 0 to 1000 at 500 is 50 percent, and with
    /// no range the value itself stands, as NVDA's overlay reads it.
    #[test]
    fn a_ranged_value_is_made_a_percentage() {
        let of = |value, minimum, maximum| Percent::of_range(value, minimum, maximum);
        assert_eq!(of(500.0, 0.0, 1000.0), Percent::parse("50"));
        assert_eq!(of(50.0, 50.0, 50.0), Percent::parse("50"));
        assert_eq!(of(f64::NAN, 0.0, 100.0), None);
    }

    /// A classic bar's position and range, an empty one dropped.
    #[test]
    fn a_classic_position_is_made_a_percentage() {
        assert_eq!(Percent::of_position(30, 10, 50), Percent::parse("50"));
        assert_eq!(Percent::of_position(10, 10, 10), None);
    }

    /// A value's text, within its percent signs, held between 0 and 100,
    /// and anything else refused.
    #[test]
    fn a_text_value_is_parsed_and_held_in_range() {
        assert_eq!(Percent::parse("41%").map(Percent::hundredths), Some(4100));
        assert_eq!(Percent::parse("250").map(Percent::hundredths), Some(10000));
        assert_eq!(Percent::parse("nearly"), None);
    }

    /// A value past 100 percent cannot be deserialized into one.
    #[test]
    fn a_percentage_past_100_is_refused_at_the_boundary() {
        assert!(Percent::try_from(10_001).is_err());
    }
}
