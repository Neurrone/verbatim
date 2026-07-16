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
}

/// What to fetch. M1 supports re-reading one node's snapshot; later
/// milestones add ancestors, text ranges, and subtree queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum QueryKind {
    /// Re-read the node's name, role, value, and states.
    NodeSnapshot,
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
    /// Periodic timer tick, for time-based policies. Unused by M1 logic but
    /// part of the frozen vocabulary so adding policies is not a breaking
    /// change.
    Tick,
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
}
