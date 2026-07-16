//! Normalized events, reducer inputs, and reducer effects.
//!
//! The reducer is strictly accessibility-shaped: its inputs are normalized
//! events, fetch completions, and timer ticks; its effects are speech and
//! fetches. Imperative concerns — showing the menu, quitting — never appear
//! here; they are routed by the shell's gesture router (architecture
//! section 2, amended during M1 planning).

use serde::{Deserialize, Serialize};

use crate::speech::Utterance;
use crate::tree::{Backend, NodeSnapshot, StateSet};
use crate::{NodeId, TraceId};

/// A Windows process identifier, used to name the application an outpost
/// watches and to key supervisor state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pid(pub u32);

impl std::fmt::Display for Pid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Version of an outpost's tree snapshot at the moment an event was
/// produced. The reducer detects stale reads by comparing versions and
/// re-fetches (architecture section 3).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct SnapshotVersion(pub u64);

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
}

/// An accessibility event, normalized by an outpost from either backend.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum NormalizedEvent {
    /// Keyboard focus moved to a node.
    FocusChanged {
        /// Snapshot of the newly focused node.
        node: NodeSnapshot,
        /// The focused node's ancestors, outermost first, walked by the
        /// outpost before emitting (deadline-guarded; empty when the walk
        /// timed out or the backend could not answer). Carried on the event
        /// rather than fetched afterward so the reducer can speak entered
        /// containers *before* the control, in NVDA's order, without
        /// holding an Interrupt announcement hostage to an async round
        /// trip. `#[serde(default)]` keeps events recorded before this
        /// field existed deserializing unchanged.
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
    },
    /// A property of a node changed.
    PropertyChanged {
        /// The node whose property changed.
        node_id: NodeId,
        /// Which property, with its new value.
        change: PropertyChange,
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
    /// A UIA `AutomationNotification` event: an app-initiated announcement
    /// (for example Windows 11's snap-layout hints) carried through
    /// verbatim. The reducer speaks its display string, if any, interrupting
    /// for `MostRecent`/`ImportantMostRecent` processing and queuing
    /// otherwise (NVDA's `event_UIA_notification`).
    Notification {
        /// The node the notification concerns.
        node_id: NodeId,
        /// The notification payload.
        notification: Notification,
    },
}

/// What kind of change a [`NormalizedEvent::Notification`] reports —
/// UIA's `NotificationKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// What to fetch. Re-reading one node's snapshot (staleness re-fetch), or
/// navigating one step from a node to a neighbor (object navigation,
/// roadmap M3). Later milestones add text ranges and subtree queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum QueryKind {
    /// Re-read the node's name, role, value, and states.
    NodeSnapshot,
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
    /// The application (and therefore outpost) to ask.
    pub source: Pid,
    /// The node to read.
    pub node_id: NodeId,
    /// What to read.
    pub kind: QueryKind,
}

/// One input to the reducer. Strictly accessibility-shaped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Input {
    /// A normalized accessibility event from an outpost.
    Event {
        /// Trace ID minted when the OS event was first observed.
        trace_id: TraceId,
        /// Milliseconds since the Unix epoch when the OS event was first
        /// observed — the same stamp the outpost put on the wire. The reducer
        /// keeps focus state last-observation-wins: a `FocusChanged` observed
        /// strictly earlier than the focus currently held (same source) is
        /// dropped, since two focus announcements can race on different outpost
        /// threads and the later-observed one is the real focus. Defaults to 0
        /// for flight-recorder streams recorded before this field existed, and
        /// a zero always proceeds (it can never be "strictly earlier").
        #[serde(default)]
        observed_at_ms: u64,
        /// The application the event came from.
        source: Pid,
        /// Which backend sourced the event (diagnostics only).
        backend: Backend,
        /// Outpost snapshot version at event time.
        version: SnapshotVersion,
        /// The event itself.
        event: NormalizedEvent,
    },
    /// A previously requested fetch finished.
    FetchCompleted {
        /// Trace ID of the input that caused the fetch.
        trace_id: TraceId,
        /// The request this result answers.
        query_id: QueryId,
        /// What the outpost found.
        result: FetchResult,
    },
    /// A review or object-navigation command, from a bound gesture
    /// (roadmap M3). The imperative shell translates a keyboard script
    /// into this; the reducer runs it against its navigator object and
    /// review cursor.
    Command {
        /// Trace ID minted when the triggering key was observed, carried
        /// through so a command's speech joins the latency timeline.
        trace_id: TraceId,
        /// Which command.
        command: ReviewCommand,
        /// How many times the gesture was pressed in quick succession, zero
        /// for the first press: report-current-object reports on 0, spells
        /// on 1, copies on 2 (NVDA's multi-press semantics). Other commands
        /// ignore it.
        repeat: u8,
    },
    /// Periodic timer tick, for time-based policies. Unused by M1 logic but
    /// part of the frozen vocabulary so adding policies is not a breaking
    /// change.
    Tick,
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
    /// Report the review cursor's current line.
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
}

/// A non-speech sound the reducer can ask for, named semantically so
/// presentation themes (milestone M11) decide what it actually sounds like.
///
/// Reserved vocabulary grows variant by variant as policies land; the first
/// consumer is the recovery ladder's not-responding cue (milestone M3
/// outpost hardening).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Earcon {
    /// A deadline expired inside a cross-process accessibility call: the
    /// application is not responding and the reducer proceeded with stale
    /// data (architecture section 1, recovery ladder rung one).
    AppNotResponding,
}

/// One effect emitted by the reducer and executed by the imperative shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Effect {
    /// Queue an utterance in the speech pipeline.
    Speak(Utterance),
    /// Cancel current and queued speech.
    StopSpeech,
    /// Ask an outpost for more data; completion re-enters as
    /// [`Input::FetchCompleted`].
    Fetch(Query),
    /// Play a non-speech sound (decision D12; themed in milestone M11).
    PlayEarcon(Earcon),
    /// Activate a node — invoke, toggle, or its default action — in the
    /// application that owns it. Fire-and-forget from the reducer's view;
    /// the shell routes it to the outpost.
    Activate {
        /// The application (and outpost) that owns the node.
        source: Pid,
        /// The node to activate.
        node_id: NodeId,
    },
    /// Copy text to the system clipboard through the shell's shared
    /// clipboard helper, which owns the spoken confirmation. The reducer
    /// stays pure — it never touches the clipboard itself — so the
    /// report-object triple-press emits this rather than doing the copy.
    CopyToClipboard(String),
}
