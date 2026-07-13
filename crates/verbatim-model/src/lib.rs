//! Normalized accessibility model (architecture section 3).
//!
//! Verbatim's own vocabulary — roles, states, properties, text ranges,
//! relationships, node identity — defined as a superset that every backend
//! (UIA, MSAA/IA2, later JAB) maps into. The reducer, browse mode,
//! extensions, and all tests speak this model only. This crate has no I/O
//! dependencies by design.
//!
//! M0 ships only the identity types; the tree, event, and effect vocabulary
//! lands with milestone M1.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// Correlates one observed OS event or keypress with everything it causes.
///
/// A `TraceId` is minted the moment an OS event or keypress is first
/// observed and is carried through the outpost, the reducer, the speech
/// queue, the synth, and audio submission, so the full timeline of any
/// utterance — event observed, speech queued, audio started — is a single
/// query (architecture section 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TraceId(u64);

impl TraceId {
    /// Mints a trace ID that is unique within this process and strictly
    /// greater than every ID minted before it.
    #[must_use]
    pub fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Stable identity of one node in an outpost's normalized tree fragment.
///
/// Backend runtime identifiers (UIA runtime IDs, MSAA object and child IDs)
/// are mapped to `NodeId`s by the owning outpost; the reducer and everything
/// above it never see backend identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
