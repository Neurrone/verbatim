//! Stable [`NodeId`]s for MSAA objects, keyed by `(hwnd, object id, child id)`.
//!
//! MSAA has no runtime-id concept; an object is addressed by its window and a
//! pair of integer ids. The outpost maps that triple to a stable [`NodeId`]
//! (architecture section 3), sharing a mint counter with the UIA registry
//! through an injected [`Arc<AtomicU64>`] so the two backends never collide.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use verbatim_model::NodeId;

/// The MSAA address of one accessible: its window handle, object id, and child
/// id (`CHILDID_SELF` is zero).
pub type MsaaKey = (isize, i32, i32);

/// Maps MSAA object addresses to stable [`NodeId`]s, and back for re-fetching.
#[derive(Clone)]
pub struct NodeIdRegistry {
    counter: Arc<AtomicU64>,
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    forward: HashMap<MsaaKey, NodeId>,
    reverse: HashMap<NodeId, MsaaKey>,
}

impl NodeIdRegistry {
    /// Creates a registry minting from `counter`. Share one counter across all
    /// backend registries in an outpost to keep identities unique.
    #[must_use]
    pub fn new(counter: Arc<AtomicU64>) -> Self {
        Self {
            counter,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Returns the stable [`NodeId`] for `key`, minting one on first sight.
    #[must_use]
    pub fn id_for(&self, key: MsaaKey) -> NodeId {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(id) = inner.forward.get(&key) {
            return *id;
        }
        let id = NodeId::new(self.counter.fetch_add(1, Ordering::Relaxed));
        inner.forward.insert(key, id);
        inner.reverse.insert(id, key);
        id
    }

    /// Returns the MSAA address previously mapped to `node`, for re-fetching.
    #[must_use]
    pub fn key_of(&self, node: NodeId) -> Option<MsaaKey> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reverse
            .get(&node)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_key_maps_to_same_node_id() {
        let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
        let first = registry.id_for((10, 0, 0));
        let again = registry.id_for((10, 0, 0));
        let other = registry.id_for((10, 0, 3));
        assert_eq!(first, again);
        assert_ne!(first, other);
    }

    #[test]
    fn reverse_lookup_round_trips() {
        let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
        let id = registry.id_for((7, 0, 4));
        assert_eq!(registry.key_of(id), Some((7, 0, 4)));
    }
}
