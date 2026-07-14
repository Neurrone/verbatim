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
}
