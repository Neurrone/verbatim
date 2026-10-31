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
use crate::text::{
    CaretKey, CaretReport, FocusText, SelectionChange, TextPosition, TextReply, TextRequest,
};
use crate::tree::{Backend, NodeSnapshot, Rect, StateSet};
use crate::{KeySeq, NodeId, OutpostId, TraceId};

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

/// Facts about the window an event concerns, read by its outpost with local
/// calls when the event is observed. The reducer classifies every event
/// against its attention record with these (decision D14;
/// `docs/parity/focus-events.md`, "Event acceptance").
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
}

/// The window that came to the system's foreground, as the focus listener
/// learned it from the foreground `WinEvent`, or from its own local reads
/// when it starts (architecture section 5): what Core needs to decide how
/// the keyboard hook treats the keys that go to it ([`HookForeground`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForegroundWindow {
    /// The window.
    pub window: WindowHandle,
    /// The thread that raised the window's event, which for a console host
    /// window is the console host's own thread; 0 when no event has named
    /// it yet.
    pub thread: u32,
    /// Whether the window is a console host window, whose owner Windows
    /// reports as the first process attached to the console rather than
    /// the console host, so that its event's thread is the only way to
    /// know the console host's thread.
    pub console: bool,
    /// The integrity level of the window's process (its mandatory label's
    /// relative identifier, such as `0x2000` for medium and `0x3000` for
    /// high), or `None` when it cannot be read.
    pub integrity: Option<u32>,
    /// Verbatim's own integrity level, read by Verbatim's process, or
    /// `None` when it cannot be read.
    pub own_integrity: Option<u32>,
}

/// What the keyboard hook needs to know of the foreground window, decided
/// by Core from the [`ForegroundWindow`] the focus listener reported and
/// read by the hook from a lock-free snapshot (architecture section 5). The
/// hook applies it only while `window` is still the foreground window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookForeground {
    /// The foreground window this was decided for.
    pub window: WindowHandle,
    /// Whether the hook may hold keys back for this window: not when its
    /// process has higher integrity than Verbatim's, or either cannot be
    /// read, since Windows drops a key Verbatim sends again to such a
    /// window (`docs/design/key-holding.md`, "Elevated windows").
    pub holds_keys: bool,
    /// The thread whose keyboard layout translates the keys typed into
    /// this window, when it is not the thread Windows names as the
    /// window's owner: a console host window's own thread
    /// (`docs/crates/verbatim-input-windows.md`, "Typed text").
    pub keyboard_thread: Option<u32>,
}

/// Identifies one in-flight fetch so its completion can re-enter the reducer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QueryId(pub u64);

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
/// (`docs/design/focus-pipeline-rework.md`, "One object, one identity").
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

/// A focus report's place in its outpost's focus group: a run of focus and
/// foreground events the outpost tries newest first, as NVDA's `pumpAll`
/// tries a run of events, stopping at the first one processed
/// (`docs/parity/backends.md`, "How an outpost turns events into focus
/// reports"). Whether an entry is
/// processed is Core's verdict, so every report of a group carries its
/// number and rank, and Core takes a report only when it took no newer
/// report of the same group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusGroup {
    /// The group's number, counted by its outpost generation from 1: a
    /// replacement generation counts again, so Core keys a group by the
    /// generation of the report's node with this number.
    pub number: u64,
    /// The entry's rank in the group, 0 for the newest, which is tried and
    /// reported first.
    pub rank: u32,
}

/// An accessibility event, normalized by an outpost from either backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "an event is built once per observation and moved, as its input is"
)]
pub enum NormalizedEvent {
    /// Keyboard focus moved to a node.
    FocusChanged {
        /// Snapshot of the newly focused node.
        node: NodeSnapshot,
        /// Whether this focus is a foreground change: the window became the
        /// system's foreground window, and `node` is that window. Following
        /// NVDA, a foreground change is a focus on the window; the reducer
        /// always accepts it and ignores it when focus is already inside
        /// that window (`docs/parity/focus-context.md`, "Window announcement on switching
        /// applications").
        #[serde(default)]
        foreground: bool,
        /// The focused node's ancestors, outermost first, complete to the
        /// desktop, which is not one of them: walked by the outpost before
        /// emitting, and where a walk was cut by its bound, failed or met a
        /// window it does not call into, completed by plain windows read
        /// from Windows' own state (`docs/design/focus-pipeline-rework.md`,
        /// "A window's name, and the one fallback"). The readers leave out
        /// the ancestors that are not presentable, layout containers among
        /// them, so it is empty for a top-level window's own focus and can
        /// be for any focus whose ancestors are all such; the focus's
        /// window facts name its top-level window. Carried on the event rather than
        /// fetched afterward so the reducer can speak entered containers
        /// *before* the control, in NVDA's order, joining this chain to
        /// the previous focus's where they meet. `#[serde(default)]` keeps
        /// events recorded before this field existed deserializing
        /// unchanged.
        #[serde(default)]
        ancestors: Vec<NodeSnapshot>,
        /// The selected child of a newly focused selection container (a
        /// list's selected item, a tab control's active tab), fetched by
        /// the outpost alongside the ancestors — only for container roles,
        /// `None` otherwise or when nothing is selected or the query
        /// failed. Carried on the event for the same reason the ancestors
        /// are: the reducer speaks it immediately after the container
        /// without a round trip. `#[serde(default)]` for wire
        /// compatibility.
        #[serde(default)]
        selected_child: Option<NodeSnapshot>,
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
        /// The report's place in its outpost's focus group, when the
        /// outpost tried a group's entries for it ([`FocusGroup`]); `None`
        /// for a report of no group.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<FocusGroup>,
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
        /// `#[serde(default)]` keeps events recorded before this field
        /// existed deserializing unchanged.
        #[serde(default)]
        child_count: Option<u32>,
    },
    /// The value of a node changed (slider drag, combo selection, text edit).
    ValueChanged {
        /// The node whose value changed.
        node_id: NodeId,
        /// The new value.
        value: Option<String>,
    },
    /// A node was selected within its container (MSAA `EVENT_OBJECT_SELECTION`
    /// and its `SELECTIONADD`/`SELECTIONREMOVE`/`SELECTIONWITHIN` siblings;
    /// UIA `SelectionItem_ElementSelected`). The reducer announces it while
    /// focus rests on a selection container, once per newly selected item.
    SelectionChanged {
        /// Snapshot of the selected node.
        node: NodeSnapshot,
    },
    /// A progress bar's value changed, whether or not it has the focus
    /// (`docs/nvda/object-model.md`, "How a progress bar reports its
    /// value"); reported in place of [`NormalizedEvent::ValueChanged`] for
    /// a visible progress bar.
    ProgressChanged {
        /// The progress bar as its value change reads it: its new value,
        /// whether it is off screen, and its location, by whose centre
        /// the last report is remembered.
        bar: ProgressReading,
    },
    /// A node was selected inside an element the focus controls (its UIA
    /// `ControllerFor` relation), such as a search result while the focus
    /// stays in the search box. The reducer speaks it as a focus and moves
    /// the navigator to it, while `controller` is still the focus.
    ControlledSelection {
        /// The focus that controls the list the node is in.
        controller: NodeId,
        /// Snapshot of the selected node.
        node: NodeSnapshot,
    },
    /// A UIA `AutomationNotification` event: an app-initiated announcement
    /// (for example Windows 11's snap-layout hints) carried through
    /// verbatim. The reducer speaks its display string, if any, interrupting
    /// for `MostRecent`/`ImportantMostRecent` processing and queuing
    /// otherwise (`docs/parity/alerts.md`, "UIA notification events").
    Notification {
        /// The node the notification concerns.
        node_id: NodeId,
        /// The notification payload.
        notification: Notification,
    },
    /// A toast notification (an MSAA `EVENT_SYSTEM_ALERT` from a window whose
    /// parent has the class `ToastChildWindowClass`): the alerting object,
    /// spoken queued and accepted from any application (decision D14), as
    /// NVDA's notification behavior speaks it.
    Alert {
        /// The alerting object.
        node: NodeSnapshot,
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
    /// (`docs/nvda/editable-text-and-terminals.md`, "Selection changes"). A
    /// caret key's speech comes from the reply to Core's own
    /// [`TextOp::AwaitCaret`](crate::TextOp::AwaitCaret) request, whose
    /// watch takes the key's caret event, so no `CaretMoved` is sent for it
    /// unless the outpost handled the event before the watch arrived.
    CaretMoved {
        /// The node whose caret moved.
        node_id: NodeId,
        /// Where the caret now is.
        caret: CaretReport,
        /// How the selection changed from the one the outpost last reported
        /// for the node, by the rule of [`CaretReply`](crate::CaretReply)'s
        /// `selection_changes`. Always empty for a terminal, and for the
        /// first report after Verbatim itself selected or moved the caret.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        selection_changes: Vec<SelectionChange>,
        /// The node's text changed since the selection compared with was
        /// reported, so the change is spoken in general terms, as
        /// [`CaretReply`](crate::CaretReply)'s `content_changed` says.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        content_changed: bool,
        /// With `content_changed`, the whole new selection's text, as
        /// [`CaretReply`](crate::CaretReply)'s `new_selection` says.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_selection: Option<SelectionChange>,
    },
    /// The text of a node changed (UIA's text changed event, an edit
    /// control's change notification). Characters typed into a terminal
    /// wait for this before they are spoken, so a password prompt that
    /// shows nothing speaks nothing (`docs/parity/terminals.md`, "Typing").
    TextChanged {
        /// The node whose text changed.
        node_id: NodeId,
    },
    /// One read of a focused terminal (`docs/design/terminal-model.md`,
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
    /// The place being read in a node's text moved without the caret: the
    /// application scrolled to it, as a document does to an in-page link's
    /// target (UIA's active text position changed event). Core speaks
    /// nothing for it until it has a browse mode, which moves its own caret
    /// there, as NVDA's does; outside browse mode NVDA does nothing with it
    /// either (`docs/parity/backends.md`, "The active text position").
    ActiveTextPositionChanged {
        /// The node whose text it is.
        node_id: NodeId,
        /// The start of the text now active.
        position: TextPosition,
    },
}

impl NormalizedEvent {
    /// Stamps `outpost` on every node id this event carries, the stamp Core
    /// applies according to the pipe the event arrived on.
    pub fn assign_outpost(&mut self, outpost: OutpostId) {
        match self {
            NormalizedEvent::FocusChanged {
                node,
                ancestors,
                selected_child,
                ..
            } => {
                node.assign_outpost(outpost);
                for ancestor in ancestors {
                    ancestor.assign_outpost(outpost);
                }
                if let Some(selected) = selected_child {
                    selected.assign_outpost(outpost);
                }
            }
            NormalizedEvent::SelectionChanged { node } | NormalizedEvent::Alert { node } => {
                node.assign_outpost(outpost);
            }
            NormalizedEvent::ProgressChanged { bar } => {
                bar.id = bar.id.with_outpost(outpost);
            }
            NormalizedEvent::ControlledSelection { controller, node } => {
                *controller = controller.with_outpost(outpost);
                node.assign_outpost(outpost);
            }
            NormalizedEvent::PropertyChanged { node_id, .. }
            | NormalizedEvent::ValueChanged { node_id, .. }
            | NormalizedEvent::Notification { node_id, .. }
            | NormalizedEvent::CaretMoved { node_id, .. }
            | NormalizedEvent::TextChanged { node_id }
            | NormalizedEvent::TerminalSnapshot { node_id, .. }
            | NormalizedEvent::ActiveTextPositionChanged { node_id, .. } => {
                *node_id = node_id.with_outpost(outpost);
            }
        }
    }
}

/// A progress bar as a change of its value reads it, for
/// [`NormalizedEvent::ProgressChanged`]: only the properties a progress
/// bar's report uses, so a property that was not read is never mistaken
/// for one that is empty, as it would be in a [`NodeSnapshot`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressReading {
    /// The progress bar.
    pub id: NodeId,
    /// Its new value.
    pub value: Option<String>,
    /// Whether it is off screen, when its percentage is not indicated.
    pub offscreen: bool,
    /// Its location on the screen, when it has one.
    pub rect: Option<Rect>,
}

/// What kind of change a [`NormalizedEvent::Notification`] reports —
/// UIA's `NotificationKind`.
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

/// How urgently a [`NormalizedEvent::Notification`] should be processed —
/// UIA's `NotificationProcessing`.
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

/// The payload of a UIA `AutomationNotification` event, normalized
/// (architecture section 4). Announced by the reducer through
/// [`NormalizedEvent::Notification`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    /// What kind of change this reports.
    pub kind: NotificationKind,
    /// How urgently it should be processed.
    pub processing: NotificationProcessing,
    /// The human-readable text the source app supplied, if any.
    pub display_string: Option<String>,
    /// An opaque id the source app uses to correlate related notifications,
    /// if any.
    pub activity_id: Option<String>,
}

/// What a completed fetch produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "a result is built once per fetch and moved, never stored in bulk"
)]
pub enum FetchResult {
    /// A fresh snapshot of the requested node.
    Node(NodeSnapshot),
    /// The node no longer exists.
    Gone,
    /// A navigation query found no node in the requested direction — a
    /// root's parent, a last child's next sibling, a leaf's first child.
    /// A first-class outcome, distinct from `Gone` (the starting node is
    /// fine, the neighbor simply does not exist).
    NoNeighbor,
    /// The application did not answer in time, or the read failed for
    /// another reason: nothing is known about the node or its neighbor, so
    /// the navigator stays where it is, as NVDA's stays when a call to a
    /// busy application is cancelled.
    Unanswered,
}

impl FetchResult {
    /// Stamps `outpost` on the node id a found node carries.
    pub fn assign_outpost(&mut self, outpost: OutpostId) {
        if let FetchResult::Node(node) = self {
            node.assign_outpost(outpost);
        }
    }
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

/// A fetch request emitted by the reducer and executed by an outpost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// Correlates the eventual [`FetchResult`] back to this request.
    pub query_id: QueryId,
    /// The node to read. Its outpost is the one asked.
    pub node_id: NodeId,
    /// What to read.
    pub kind: QueryKind,
}

/// One input to the reducer. Strictly accessibility-shaped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "an input is built once per event and moved, never stored in bulk"
)]
pub enum Input {
    /// A normalized accessibility event from an outpost.
    Event {
        /// Trace ID minted when the OS event was first observed.
        trace_id: TraceId,
        /// Microseconds since the Unix epoch on the run's shared clock when
        /// the OS event was first observed, the stamp the outpost put on the
        /// wire; 0 when unknown. The reducer orders by it, to drop a focus
        /// observed before the focus it already applied
        /// (`docs/parity/focus-events.md`, "Stale focus events").
        #[serde(default)]
        observed_at_us: u64,
        /// The application the event came from: information about its
        /// source, for attention and logs. Node ids carry the outpost.
        source: Pid,
        /// Which backend sourced the event (diagnostics only).
        backend: Backend,
        /// Facts about the window the event concerns, or `None` when the
        /// event has no window to read them from.
        #[serde(default)]
        window: Option<WindowFacts>,
        /// The event itself.
        event: NormalizedEvent,
    },
    /// A previously requested fetch finished.
    FetchCompleted {
        /// Trace ID of the input that caused the fetch.
        trace_id: TraceId,
        /// The request this result answers.
        query_id: QueryId,
        /// What the request asked for, echoed by the outpost.
        kind: QueryKind,
        /// What the outpost found.
        result: FetchResult,
    },
    /// An outpost generation ended: it exited, was killed, or was retired.
    /// Every node id it issued is dead from now on.
    OutpostEnded {
        /// The generation that ended.
        outpost: OutpostId,
    },
    /// A review or object-navigation command, from a bound gesture
    /// (roadmap M3). The imperative shell translates a keyboard script
    /// into this; the reducer runs it against its navigator object and
    /// review cursor.
    Command {
        /// Trace ID minted when the triggering key was observed, carried
        /// through so a command's speech joins the latency timeline.
        trace_id: TraceId,
        /// The gesture's key number, which its speech carries.
        #[serde(default = "KeySeq::none_yet")]
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
        #[serde(default = "KeySeq::none_yet")]
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
    /// 30 ms after `pressed_at_ms`, once Windows has changed it
    /// ([`Effect::ReadToggleKey`], then [`Input::LockKeyToggled`]).
    ToggleKeyPressed {
        /// Trace ID minted when the key was observed.
        trace_id: TraceId,
        /// The key's number. The announcement does not carry it, so a key
        /// pressed within the 30 ms does not silence it.
        #[serde(default = "KeySeq::none_yet")]
        key_seq: KeySeq,
        /// Which key.
        toggle: LockKey,
        /// Milliseconds on the run's shared clock when the hook saw it.
        pressed_at_ms: u64,
    },
    /// An activation the reducer asked for finished: `activated` is whether
    /// the navigator object, or one of its ancestors, was activated, and
    /// `action` the name of the action performed, if it has one. The
    /// reducer speaks the action's name, NVDA's "Activate" for an action
    /// with none, or "No action".
    ActivationCompleted {
        /// Trace ID of the command that asked for the activation.
        trace_id: TraceId,
        /// Whether anything was activated.
        activated: bool,
        /// The name of the action performed; `None` for an unnamed one.
        action: Option<ActionName>,
    },
    /// Periodic timer tick, for time-based policies. Unused by M1 logic but
    /// part of the frozen vocabulary so adding policies is not a breaking
    /// change.
    Tick,
    /// A text request the reducer made ([`Effect::Text`]) was answered.
    TextCompleted {
        /// Trace ID of the input that caused the request.
        trace_id: TraceId,
        /// The request this answers.
        query_id: QueryId,
        /// The answer.
        reply: TextReply,
    },
    /// A caret key was pressed and passed to the application, which moves
    /// the caret itself (milestone M4). The keyboard hook reports it as it
    /// passes it on; the reducer waits for evidence of what it did and
    /// speaks the result when the focus has text.
    CaretKey {
        /// Trace ID minted when the key was observed.
        trace_id: TraceId,
        /// The key's number, which its speech carries.
        #[serde(default = "KeySeq::none_yet")]
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
    /// NVDA's caret scripts hold theirs (`docs/design/key-holding.md`): the
    /// reducer has the focus's outpost read the caret before the key, then
    /// passes the key on ([`Effect::PassHeldKey`]) and watches for its
    /// evidence from that exact caret, as for [`Input::CaretKey`]. The key
    /// is always passed on, whatever the read's outcome.
    HeldKey {
        /// Trace ID minted when the key was observed; it also names the
        /// key to pass on.
        trace_id: TraceId,
        /// The key's number, which its speech carries.
        #[serde(default = "KeySeq::none_yet")]
        key_seq: KeySeq,
        /// Which key.
        key: CaretKey,
        /// As [`Input::CaretKey`]'s: when the hook saw the key, on the
        /// outposts' clock; 0 when unknown.
        #[serde(default)]
        pressed_at_ms: u64,
    },
    /// The bound on a held key's wait for the key ahead of it has passed
    /// ([`Effect::BoundHeldKeyWait`]): if the held key with this trace ID
    /// still waits for the watch of the key ahead to be answered, its caret
    /// is read now, as if that watch had been answered.
    HeldKeyWaitEnded {
        /// The trace ID of the held key whose wait was bounded.
        trace_id: TraceId,
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
        #[serde(default = "KeySeq::none_yet")]
        key_seq: KeySeq,
        /// The text typed, a tab as a tab character and Enter as a carriage
        /// return.
        text: String,
        /// Milliseconds on the run's shared clock when the hook saw the
        /// key, the clock of a caret key's `pressed_at_ms`; 0 when unknown.
        #[serde(default)]
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
        #[serde(default = "KeySeq::none_yet")]
        key_seq: KeySeq,
        /// As [`Input::CharacterTyped`]'s: when the hook saw the key; 0
        /// when unknown.
        #[serde(default)]
        at_ms: u64,
    },
    /// Playback reached an index mark the reducer placed in an utterance
    /// ([`crate::SegmentContent::Mark`]), reported by the speech pipeline
    /// when the device has played the audio before it.
    MarkReached {
        /// The mark.
        mark: SpeechMark,
    },
    /// Speech was cut off outside the reducer: a key press cancelled it, as
    /// the keyboard hook does for nearly every key. Say-all stops here, as
    /// NVDA's stops on any key.
    SpeechCancelled {
        /// When it was cut off, in milliseconds since the Unix epoch, the
        /// clock outposts stamp events with; 0 when unknown.
        #[serde(default)]
        at_ms: u64,
    },
    /// The reader settings, at startup and whenever the user changes them.
    Settings(ReaderSettings),
    /// The details the active theme wants fetched ([`crate::Fetches`]), at
    /// startup and whenever the theme in use changes.
    Fetches(crate::Fetches),
    /// An utterance the reducer placed index marks in was cut off before
    /// playback reached all of them (a key, an interrupting utterance, the
    /// focus moving on, a synthesizer switch). Reported by the speech
    /// pipeline for every such utterance, however it was cut, so nothing
    /// the reducer waits on is lost by a cut: what waited on it stops.
    UtteranceCancelled {
        /// The marks the utterance carried, in order; those playback had
        /// already reached were reported as [`Input::MarkReached`] first.
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
        /// already reached were reported as [`Input::MarkReached`] first.
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
    /// An application stopped responding: its outpost's watchdog abandoned
    /// a query that passed its deadline, the first in this stall (the shell
    /// tells of each stall once, until the outpost answers again).
    ApplicationNotResponding {
        /// The outpost whose application stopped responding.
        outpost: OutpostId,
    },
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
    /// A menu or the Alt+Tab switcher closed, at `ended_at_us`, as the focus
    /// listener tells it, 50 ms later.
    MenuOrSwitchEnded {
        /// When it closed, in microseconds since the Unix epoch on the
        /// clock events' `observed_at_us` use.
        ended_at_us: u64,
    },
    /// The quiet-period timer of the focused terminal's reading rules came
    /// due ([`Effect::TerminalTimer`]): the rules are given the keys that
    /// reached the program by `at_ms`, then the timer.
    TerminalTimer {
        /// Trace ID minted when the shell found it due.
        trace_id: TraceId,
        /// When it came due, in milliseconds on the clock every process of
        /// a run shares.
        at_ms: u64,
    },
    /// A window came to the foreground, as the focus listener tells it
    /// (architecture section 5): Core decides from it what the keyboard
    /// hook reads of the foreground ([`HookForeground`]).
    ForegroundWindow(ForegroundWindow),
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
            Self::Event { trace_id, .. }
            | Self::FetchCompleted { trace_id, .. }
            | Self::Command { trace_id, .. }
            | Self::Script { trace_id, .. }
            | Self::ToggleKeyPressed { trace_id, .. }
            | Self::TextCompleted { trace_id, .. }
            | Self::CaretKey { trace_id, .. }
            | Self::HeldKey { trace_id, .. }
            | Self::CharacterTyped { trace_id, .. }
            | Self::ClearingKey { trace_id, .. }
            | Self::LockKeyToggled { trace_id, .. }
            | Self::ClockRead { trace_id, .. }
            | Self::ClipboardCopied { trace_id, .. }
            | Self::ShellItemGone { trace_id, .. }
            | Self::ShellItemUnclickable { trace_id, .. }
            | Self::PreviewSpeech { trace_id, .. } => Some(*trace_id),
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
    /// (`docs/design/terminal-model.md`, "Features and settings"); spelled
    /// on the second press in a streak, copied to the clipboard on the
    /// third.
    ReportStatusBar,
    /// Move the review cursor to the previous command mark in a terminal
    /// and speak its line (`docs/design/terminal-model.md`, "Features and
    /// settings").
    PreviousCommandMark,
    /// Move the review cursor to the next command mark in a terminal.
    NextCommandMark,
}

/// An event Verbatim indicates at once, outside the speech queue, named
/// semantically so the active theme decides how it is reported: a sound
/// played immediately on its own mixer source, words spoken, both, or
/// nothing (`docs/crates/verbatim-model.md`, "Themes"). Each is an indication of the
/// catalogue's Events category ([`crate::Indication::of_earcon`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
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
#[non_exhaustive]
pub enum Effect {
    /// Queue an utterance in the speech pipeline, with the number of the
    /// user input that caused it, if one did: Core stamps it as its last
    /// step, and every other construction gives `None`.
    Speak(Utterance, Option<KeySeq>),
    /// Cancel current and queued speech.
    StopSpeech,
    /// The focus has changed: stop focus speech being spoken that is no
    /// longer valid ([`crate::FocusValidity`]), and judge waiting focus
    /// speech against this focus when its turn comes.
    DropExpiredSpeech(crate::FocusNow),
    /// Ask an outpost for more data; completion re-enters as
    /// [`Input::FetchCompleted`].
    Fetch(Query),
    /// Indicate an event at once, as the active theme reports it: a sound
    /// on its own mixer source, words, both, or nothing.
    PlayEarcon(Earcon),
    /// Activate a node — invoke, toggle, or its default action — in the
    /// application that owns it. Fire-and-forget from the reducer's view;
    /// the shell routes it to the outpost.
    Activate {
        /// The node to activate. Its outpost is the one asked.
        node_id: NodeId,
    },
    /// Copy text to the system clipboard through the shell's shared
    /// clipboard helper, whose result comes back as
    /// [`Input::ClipboardCopied`], which the reducer confirms in speech.
    /// The reducer stays pure — it never touches the clipboard itself — so
    /// the report-object triple-press emits this rather than doing the copy.
    CopyToClipboard(String),
    /// Ask an outpost to read or act on a node's text; the answer re-enters
    /// as [`Input::TextCompleted`] (milestone M4).
    Text(TextRequest),
    /// Keep the display on (true) while say-all reads, or let it turn off
    /// again (false), NVDA's "Prevent display from turning off during say
    /// all". Every true is followed by a false when say-all ends.
    KeepDisplayOn(bool),
    /// The reducer changed a reader setting itself, by a toggle key; the
    /// shell saves the new settings.
    SettingsChanged(ReaderSettings),
    /// Pass the caret key the hook held back as [`Input::HeldKey`] with
    /// this trace ID on to the application, re-sent as the user pressed
    /// it.
    PassHeldKey(TraceId),
    /// The held key with this trace ID waits for the watch of the key ahead
    /// of it to be answered: if it still waits `within_ms` from now, the
    /// shell enters [`Input::HeldKeyWaitEnded`] with its trace ID. A later
    /// one replaces it (`docs/design/key-holding.md`, "Keys behind a held
    /// key").
    BoundHeldKeyWait {
        /// The waiting held key's trace ID.
        key: TraceId,
        /// How long from now the wait may last, in milliseconds.
        within_ms: u64,
    },
    /// The effect on behalf of the key with this trace ID, whatever input
    /// caused it: a key that waited behind a held key is handled while the
    /// reducer handles the held key's read (the read before a held key, or
    /// a later key's own effects), and its requests and speech must carry
    /// its own key's trace, so that its answers and speech are that key's.
    ForKey(TraceId, Box<Effect>),
    /// Arm the focused terminal's quiet-period timer for `due_ms`, in
    /// milliseconds on the clock every process of a run shares, replacing
    /// any armed before, or disarm it (`None`). When it passes, the shell
    /// enters [`Input::TerminalTimer`].
    TerminalTimer {
        /// When it is due.
        due_ms: Option<u64>,
    },
    /// Ask the application in front for its focus, reported as a focus on
    /// the control alone, as NVDA's fake focus after a menu or the Alt+Tab
    /// switcher closes; an application whose outpost is not ready is asked
    /// for its focus as at a foreground change.
    ReadForegroundFocus,
    /// Read the time and date, for the report-time gesture: the shell
    /// enters [`Input::ClockRead`] with the trace it executed this under.
    ReadClock {
        /// The gesture's repeat count, handed back on the input.
        repeat: u8,
    },
    /// Show a window of the GUI.
    Show(GuiRequest),
    /// Read a lock key's state at `due_ms`, on the run's shared clock, and
    /// enter [`Input::LockKeyToggled`] with it.
    ReadToggleKey {
        /// Which key.
        toggle: LockKey,
        /// When to read it.
        due_ms: u64,
    },
}
