//! Stable [`NodeId`]s for UIA elements.
//!
//! UIA runtime IDs are arrays of integers, stable for the lifetime of an
//! element but opaque and backend-specific. The outpost maps each runtime ID
//! to a stable [`NodeId`] so nothing above the outpost sees backend identity
//! (architecture section 3). The mint counter is shared with the MSAA
//! registry through an injected [`Arc<AtomicU64>`] so the two backends never
//! hand out the same [`NodeId`] for different nodes within one outpost.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use verbatim_model::NodeId;

/// Maps UIA runtime IDs to stable [`NodeId`]s, and back for re-fetching.
///
/// Cloning shares the underlying tables and counter, so the focus callback
/// thread and the query-pool threads mint from one consistent namespace.
#[derive(Clone)]
pub struct NodeIdRegistry {
    counter: Arc<AtomicU64>,
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    forward: HashMap<Vec<i32>, NodeId>,
    reverse: HashMap<NodeId, Vec<i32>>,
}

impl NodeIdRegistry {
    /// Creates a registry that mints [`NodeId`]s from `counter`. Pass the same
    /// counter to every backend registry in one outpost to keep identities
    /// unique across backends.
    #[must_use]
    pub fn new(counter: Arc<AtomicU64>) -> Self {
        Self {
            counter,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Returns the stable [`NodeId`] for `runtime_id`, minting one on first
    /// sight. Empty runtime IDs (unavailable elements) still get an identity
    /// so events are never silently dropped, but they are never deduplicated.
    #[must_use]
    pub fn id_for(&self, runtime_id: &[i32]) -> NodeId {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if !runtime_id.is_empty()
            && let Some(id) = inner.forward.get(runtime_id)
        {
            return *id;
        }
        let id = NodeId::new(self.counter.fetch_add(1, Ordering::Relaxed));
        if !runtime_id.is_empty() {
            inner.forward.insert(runtime_id.to_vec(), id);
            inner.reverse.insert(id, runtime_id.to_vec());
        }
        id
    }

    /// Returns the runtime ID previously mapped to `node`, for re-fetching the
    /// node's current state on a query-pool thread.
    #[must_use]
    pub fn runtime_id_of(&self, node: NodeId) -> Option<Vec<i32>> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reverse
            .get(&node)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_runtime_id_maps_to_same_node_id() {
        let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
        let first = registry.id_for(&[42, 7]);
        let again = registry.id_for(&[42, 7]);
        let other = registry.id_for(&[42, 8]);
        assert_eq!(first, again);
        assert_ne!(first, other);
    }

    #[test]
    fn reverse_lookup_round_trips() {
        let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
        let id = registry.id_for(&[1, 2, 3]);
        assert_eq!(registry.runtime_id_of(id), Some(vec![1, 2, 3]));
    }

    #[test]
    fn shared_counter_keeps_ids_distinct_across_registries() {
        let counter = Arc::new(AtomicU64::new(1));
        let uia = NodeIdRegistry::new(counter.clone());
        let other = NodeIdRegistry::new(counter);
        let a = uia.id_for(&[1]);
        let b = other.id_for(&[1]);
        assert_ne!(a, b, "distinct registries sharing a counter never collide");
    }
}
