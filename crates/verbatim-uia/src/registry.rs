//! Stable [`NodeId`]s for UIA elements, and a live-element cache.
//!
//! UIA runtime IDs are arrays of integers, stable for the lifetime of an
//! element but opaque and backend-specific. The outpost maps each runtime ID
//! to a stable [`NodeId`] so nothing above the outpost sees backend identity
//! (architecture section 3). The mint counter is shared with the MSAA
//! registry through an injected [`Arc<AtomicU64>`] so the two backends never
//! hand out the same [`NodeId`] for different nodes within one outpost.
//!
//! The registry also caches the live `IUIAutomationElement` behind each
//! node, as an apartment-agile reference. Re-acquiring an element by
//! runtime id means a `FindFirst` search, which has no index behind it —
//! measured live, it walks enough of the tree to cost hundreds of
//! milliseconds against a busy desktop, and it can miss virtualized
//! elements entirely. NVDA never re-finds: it holds the live element on its
//! `NVDAObject`. Caching here gives the same property: object navigation and
//! node re-reads resolve the cached element directly, falling back to a
//! (scoped) search only when the element has died.
//!
//! Nodes stay until the outpost releases them ([`NodeIdRegistry::retain`]),
//! which it does for every node Core no longer holds (outpost redesign,
//! "Held objects"). Unlike the MSAA registry, nodes are not dropped when
//! their window is destroyed: a UIA runtime id has no documented structure
//! naming its window. So a node Core still holds after its window closed
//! (the navigator left there) is the node for any new element given the
//! same runtime id, which needs Windows to reuse the same window handle
//! value while Core holds it. NVDA, comparing elements by runtime id and
//! holding its navigator indefinitely, has the same exposure.
//!
//! A runtime id is unique only among elements alive at the same time: once
//! an element dies, its application may give the id to a new element. File
//! Explorer does, found 2026-10-07: going back from a subfolder, the parent
//! folder's item that took the focus had the runtime id of the subfolder's
//! item, destroyed with its list. Mapped to the old node, the new focus
//! read to Core as the focus it already had, and nothing was spoken. So
//! before a focus is reported under a node that already exists, the
//! outpost checks that node's element: when it no longer has the keyboard
//! focus, or cannot be read at all, the element behind the id is not the
//! one the node stood for, and [`NodeIdRegistry::reissue`] gives the id a
//! new node (`docs/parity.md`, "Duplicate focus suppression").

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::core::AgileReference;

use verbatim_model::NodeId;

/// Elements a [`NodeIdRegistry`] has released. Releasing an element can call
/// into its process, so the caller drops this after any lock it holds is
/// released.
#[must_use = "drop the released elements once no lock is held"]
pub struct Released {
    _elements: Vec<AgileReference<IUIAutomationElement>>,
}

/// Maps UIA runtime IDs to stable [`NodeId`]s, and back for re-fetching,
/// with a live-element cache per node (module doc).
///
/// Cloning shares the underlying tables and counter, so the focus callback
/// thread and the worker mint from one consistent namespace.
#[derive(Clone)]
pub struct NodeIdRegistry {
    counter: Arc<AtomicU64>,
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    forward: HashMap<Vec<i32>, NodeId>,
    reverse: HashMap<NodeId, Vec<i32>>,
    elements: HashMap<NodeId, AgileReference<IUIAutomationElement>>,
    /// Nodes issued or looked up since the outpost last took them.
    touched: Vec<NodeId>,
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
        Self::id_for_locked(&self.counter, &mut inner, runtime_id)
    }

    /// [`id_for`](Self::id_for), and additionally caches `element` as the
    /// live element behind the returned node (module doc), refreshing any
    /// previously cached one — the newest sighting is the most likely to
    /// still be alive. The element it replaces is released after the lock,
    /// since releasing an element can call into its process.
    #[must_use]
    pub fn id_for_element(&self, runtime_id: &[i32], element: &IUIAutomationElement) -> NodeId {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let id = Self::id_for_locked(&self.counter, &mut inner, runtime_id);
        let mut replaced = None;
        if !runtime_id.is_empty()
            && let Ok(agile) = AgileReference::new(element)
        {
            replaced = inner.elements.insert(id, agile);
        }
        drop(inner);
        drop(replaced);
        id
    }

    fn id_for_locked(counter: &AtomicU64, inner: &mut Inner, runtime_id: &[i32]) -> NodeId {
        if !runtime_id.is_empty()
            && let Some(&id) = inner.forward.get(runtime_id)
        {
            inner.touched.push(id);
            return id;
        }
        let id = NodeId::new(counter.fetch_add(1, Ordering::Relaxed));
        if !runtime_id.is_empty() {
            inner.forward.insert(runtime_id.to_vec(), id);
            inner.reverse.insert(id, runtime_id.to_vec());
            inner.touched.push(id);
        }
        id
    }

    /// The kept node for `runtime_id`, if there is one, without issuing a
    /// node or recording it as reported.
    #[must_use]
    pub fn existing_id(&self, runtime_id: &[i32]) -> Option<NodeId> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .forward
            .get(runtime_id)
            .copied()
    }

    /// Every kept node.
    #[must_use]
    pub fn ids(&self) -> Vec<NodeId> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reverse
            .keys()
            .copied()
            .collect()
    }

    /// Takes the nodes issued or looked up since the last call, so the
    /// outpost can record which message reported them.
    #[must_use]
    pub fn take_touched(&self) -> Vec<NodeId> {
        std::mem::take(
            &mut self
                .inner
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .touched,
        )
    }

    /// Keeps only the nodes for which `keep` returns `true`. The others are
    /// forgotten at once and their elements returned, for the caller to drop
    /// once no lock is held: releasing an element can call into its process.
    pub fn retain(&self, mut keep: impl FnMut(NodeId) -> bool) -> Released {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let gone: Vec<NodeId> = inner
            .reverse
            .keys()
            .copied()
            .filter(|&id| !keep(id))
            .collect();
        let elements = gone
            .iter()
            .filter_map(|id| {
                if let Some(runtime_id) = inner.reverse.remove(id) {
                    inner.forward.remove(&runtime_id);
                }
                inner.elements.remove(id)
            })
            .collect();
        Released {
            _elements: elements,
        }
    }

    /// Returns the runtime ID previously mapped to `node`, for re-fetching the
    /// node's current state on the worker.
    #[must_use]
    pub fn runtime_id_of(&self, node: NodeId) -> Option<Vec<i32>> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .reverse
            .get(&node)
            .cloned()
    }

    /// Returns the cached live element behind `node`, if one is cached. The
    /// caller must treat a failing COM call on the resolved element as "the
    /// element died" and fall back to a search — the cache is a fast path,
    /// never a liveness guarantee.
    #[must_use]
    pub fn element_of(&self, node: NodeId) -> Option<AgileReference<IUIAutomationElement>> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elements
            .get(&node)
            .cloned()
    }

    /// Drops the cached live element behind `node` — called when a resolved
    /// element turns out dead, so the next lookup goes straight to the
    /// search fallback instead of retrying a corpse. The element is released
    /// after the lock, since releasing it can call into its process.
    pub fn evict_element(&self, node: NodeId) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let evicted = inner.elements.remove(&node);
        drop(inner);
        drop(evicted);
    }

    /// Forgets the node `runtime_id` names, so the next
    /// [`id_for`](Self::id_for) or [`id_for_element`](Self::id_for_element)
    /// mints a new one: for a runtime id an application has given to a new
    /// element after the one it named died (module doc). The old node is
    /// forgotten whole, so a query for it answers that it is gone rather
    /// than reaching the new element. Its element is returned, for the
    /// caller to drop once no lock is held.
    pub fn reissue(&self, runtime_id: &[i32]) -> Released {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let element = inner.forward.remove(runtime_id).and_then(|old| {
            inner.reverse.remove(&old);
            inner.elements.remove(&old)
        });
        Released {
            _elements: element.into_iter().collect(),
        }
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
    fn released_nodes_are_forgotten_and_their_runtime_id_issues_anew() {
        let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
        let kept = registry.id_for(&[1]);
        let released = registry.id_for(&[2]);
        assert_eq!(registry.take_touched(), vec![kept, released]);
        drop(registry.retain(|id| id == kept));
        assert_eq!(registry.ids(), vec![kept]);
        assert_eq!(registry.runtime_id_of(released), None);
        assert_ne!(registry.id_for(&[2]), released);
    }

    #[test]
    fn a_reissued_runtime_id_names_a_new_node_and_the_old_one_is_forgotten() {
        let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
        let dead = registry.id_for(&[42, 7]);
        let other = registry.id_for(&[42, 8]);
        drop(registry.reissue(&[42, 7]));
        assert_eq!(registry.existing_id(&[42, 7]), None);
        assert_eq!(registry.runtime_id_of(dead), None, "the old node is gone");
        let new = registry.id_for(&[42, 7]);
        assert_ne!(new, dead);
        assert_eq!(registry.runtime_id_of(new), Some(vec![42, 7]));
        assert_eq!(registry.id_for(&[42, 7]), new, "the new node is kept");
        assert_eq!(
            registry.id_for(&[42, 8]),
            other,
            "other nodes are untouched"
        );
        let mut ids = registry.ids();
        ids.sort_by_key(|id| id.number());
        assert_eq!(ids, vec![other, new]);
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
