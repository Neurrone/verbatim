//! Normalized accessibility model (architecture section 3).
//!
//! Verbatim's own vocabulary — roles, states, properties, node identity,
//! events, reducer inputs and effects, structured utterances, and gesture
//! identifiers — defined as a superset that every backend (UIA, MSAA/IA2,
//! later JAB) maps into. The reducer, browse mode, extensions, and all tests
//! speak this model only. This crate has no I/O dependencies by design;
//! serde derives exist so the same types travel over the Core-outpost pipe,
//! the control plane, and the flight recorder unchanged.

mod event;
mod gesture;
mod speech;
mod tree;

pub use event::{
    Effect, FetchResult, Input, NormalizedEvent, Pid, PropertyChange, Query, QueryId, QueryKind,
    SnapshotVersion,
};
pub use gesture::{GestureId, GestureParseError};
pub use speech::{SegmentContent, SpeechPriority, Utterance, UtteranceSegment};
pub use tree::{Backend, NodeSnapshot, Role, State, StateSet, TreeNode};

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Correlates one observed OS event or keypress with everything it causes.
///
/// A `TraceId` is minted the moment an OS event or keypress is first
/// observed and is carried through the outpost, the reducer, the speech
/// queue, the synth, and audio submission, so the full timeline of any
/// utterance — event observed, speech queued, audio started — is a single
/// query (architecture section 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TraceId(u64);

/// The mint counter behind [`TraceId::mint`].
static NEXT_TRACE_ID: AtomicU64 = AtomicU64::new(1);

impl TraceId {
    /// Mints a trace ID that is unique within this process and strictly
    /// greater than every ID minted before it.
    #[must_use]
    pub fn mint() -> Self {
        Self(NEXT_TRACE_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Namespaces this process's trace IDs by seeding the mint counter with
    /// the process id in the high 32 bits. Verbatim runs as several
    /// processes (Core and one outpost per application), each minting trace
    /// IDs independently; their IDs meet in Core's latency ledger and the
    /// flight recorder, so every process calls this once at startup, before
    /// any ID is minted, to keep the ID spaces disjoint.
    pub fn namespace(pid: u32) {
        NEXT_TRACE_ID.store((u64::from(pid) << 32) | 1, Ordering::Relaxed);
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Name of the window property `verbatim-gui` stamps on Core's hidden 1x1
/// main frame (decision D9) so every outpost can recognize and suppress it.
///
/// The frame transits real focus during the prePopup show/raise/foreground
/// dance around the Verbatim menu and the settings dialog; without this
/// marker it can be announced as a nameless "Verbatim" window with role
/// unknown, a race described in `docs/roadmap.md`'s M3 section.
/// `verbatim-gui` sets this property (`SetPropW`) on the frame at creation
/// and clears it (`RemovePropW`) at shutdown; every outpost checks it
/// (`GetPropW`) before emitting any `FocusChanged` — the MSAA event path,
/// the UIA focus callback, and the synthetic focus query alike.
pub const HIDDEN_FRAME_WINDOW_PROP: &str = "VerbatimHiddenFrame";

/// Stable identity of one node in an outpost's normalized tree fragment.
///
/// Backend runtime identifiers (UIA runtime IDs, MSAA object and child IDs)
/// are mapped to `NodeId`s by the owning outpost; the reducer and everything
/// above it never see backend identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(u64);

impl NodeId {
    /// Wraps a raw outpost-assigned identifier.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_ids_are_unique_and_increasing() {
        let first = TraceId::mint();
        let second = TraceId::mint();
        assert!(second > first);
    }

    #[test]
    fn node_ids_compare_by_value() {
        assert_eq!(NodeId::new(7), NodeId::new(7));
        assert_ne!(NodeId::new(7), NodeId::new(8));
    }
}
