//! Stable [`NodeId`]s for MSAA objects, and the objects behind them.
//!
//! MSAA has no runtime ids: an event names an object by its window handle,
//! object id, and child id. The outpost maps each object it reports to a
//! [`NodeId`] (architecture section 4), sharing a mint counter with the UIA
//! registry through an injected [`Arc<AtomicU64>`] so the two backends never
//! collide.
//!
//! The registry also keeps the accessible object each node was read from
//! (`docs/crates/verbatim-outpost.md`, held objects), so navigation, activation, and
//! re-reads reach the object that was announced, not whatever now sits at its
//! address. Deciding whether a new sighting is a node already issued, NVDA's
//! comparison order, needs COM calls, so [`acquire`](crate::acquire) makes it;
//! the registry only stores and looks up, and never makes a COM call under
//! its lock. Nodes stay until the outpost releases them ([`retain`]) or their
//! window is destroyed ([`forget_window`]). A walk issues and looks up nodes
//! through [`Interning`], which the outpost's object store implements to
//! tag each with the request that named it; the registry implements it
//! too, untagged, for a reader with no store.
//!
//! An address is only an identity when the object was acquired at it, from
//! an event or a window, or when the object is one of a window's own
//! standard objects, whose identity string names its address. Any other
//! object reached through `accParent` or as a child object gets an address
//! made up from its window, which it may share with other objects; such a
//! node is never found by its address, only as the same COM object.
//!
//! [`retain`]: NodeIdRegistry::retain
//! [`forget_window`]: NodeIdRegistry::forget_window

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows::Win32::UI::Accessibility::IAccessible;
use windows::core::AgileReference;

use verbatim_model::{Fetches, NodeId, Role};

/// The MSAA address of one accessible: its window handle, object id, and child
/// id (`CHILDID_SELF` is zero).
pub type MsaaKey = (isize, i32, i32);

/// The accessible object a node was read from: the object and the child id
/// on it, with the address of the object's canonical `IUnknown`, which
/// identifies the object for as long as it is kept.
pub struct Held {
    pub(crate) object: AgileReference<IAccessible>,
    pub(crate) child: i32,
    pub(crate) identity: usize,
}

struct Node {
    key: MsaaKey,
    held: Option<Held>,
    /// The role read when the node was issued; `None` for a node issued from
    /// local window data.
    role: Option<Role>,
    /// Whether the object was acquired at `key`, so the address identifies
    /// it (module doc).
    at_address: bool,
    /// What NVDA's equality compares of the node as it was read
    /// ([`Facts`]); `None` for a node issued from local window data, which
    /// is never compared.
    facts: Option<Facts>,
}

/// What NVDA's `IAccessible._isEqual` compares of an MSAA object, as the
/// node was read when it was issued, but its name, refreshed when a new
/// sighting is found to be it (`crate::acquire`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// The event window, the event object id when known, and the event
    /// child id, fixed when the object was made: an event's address for an
    /// object acquired at it, else its window, no object id, and its child
    /// id.
    pub(crate) event: (isize, Option<i32>, i32),
    /// Its identity string, `None` when it was not asked for, and
    /// `Some(None)` when the object answered none.
    #[expect(
        clippy::option_option,
        reason = "absence counts as a value only where it was asked for"
    )]
    pub(crate) identity: Option<Option<Vec<u8>>>,
    /// Its raw `IAccessible` role, as read.
    pub(crate) raw_role: Option<i32>,
    /// Its name, normalised as a snapshot's name is.
    pub(crate) name: Option<String>,
}

/// A kept node as a new sighting is compared with it
/// ([`NodeIdRegistry::facts_of`]).
pub struct Kept {
    pub(crate) key: MsaaKey,
    pub(crate) object: Option<KeptObject>,
    pub(crate) role: Option<Role>,
    pub(crate) facts: Facts,
}

/// Nodes a [`NodeIdRegistry`] has released, with their objects. Releasing a
/// COM object can call into its process, so the caller drops this after
/// any lock it holds is released.
#[must_use = "drop the released objects once no lock is held"]
pub struct Released {
    _nodes: Vec<Node>,
}

/// A kept object and the child id on it.
pub type KeptObject = (AgileReference<IAccessible>, i32);

/// What a lookup for a new sighting found.
pub enum Found {
    /// A kept node was the same COM object with the same child id, if its
    /// kept object still has that identity.
    Object(NodeId, Option<KeptObject>),
    /// A kept node has the same address, with its object (if kept) and role,
    /// for the caller to compare.
    Key(NodeId, Option<KeptObject>, Option<Role>),
    /// Nothing matches.
    Nothing,
}

/// Maps MSAA objects to stable [`NodeId`]s and keeps the object behind each.
#[derive(Clone)]
pub struct NodeIdRegistry {
    counter: Arc<AtomicU64>,
    inner: Arc<Mutex<Inner>>,
    /// What a node read includes; every read goes through the registry.
    fetches: Arc<Mutex<Fetches>>,
}

#[derive(Default)]
struct Inner {
    nodes: HashMap<NodeId, Node>,
    by_key: HashMap<MsaaKey, NodeId>,
    by_object: HashMap<(usize, i32), NodeId>,
}

/// How a walk issues and looks up the nodes of the objects it reads: the
/// outpost's object store, which tags every node issued or looked up with
/// the request that named it, or a [`NodeIdRegistry`] itself, untagged.
/// Each method is the registry's of the same name.
pub trait Interning {
    /// [`NodeIdRegistry::fetches`].
    fn fetches(&self) -> Fetches;
    /// [`NodeIdRegistry::id_for`]: issues or looks up.
    fn id_for(&self, key: MsaaKey) -> NodeId;
    /// [`NodeIdRegistry::find`]: a node found is looked up.
    fn find(&self, key: MsaaKey, identity: Option<usize>, child: i32, at_address: bool) -> Found;
    /// Looks up `id`, a node [`find`](Self::find) found and the caller
    /// confirmed; nothing for the registry itself.
    fn touch(&self, id: NodeId);
    /// [`NodeIdRegistry::insert`]: issues.
    fn insert(
        &self,
        key: MsaaKey,
        held: Option<Held>,
        role_facts: (Role, Facts),
        at_address: bool,
    ) -> NodeId;
    /// [`NodeIdRegistry::facts_of`].
    fn facts_of(&self, id: NodeId) -> Option<Kept>;
    /// [`NodeIdRegistry::reissue`]: looks up.
    fn reissue(&self, id: NodeId, name: Option<String>);
    /// [`NodeIdRegistry::key_of`].
    fn key_of(&self, node: NodeId) -> Option<MsaaKey>;
    /// [`NodeIdRegistry::locate`].
    fn locate(&self, node: NodeId) -> Option<(MsaaKey, Option<KeptObject>, bool)>;
}

impl Interning for NodeIdRegistry {
    fn fetches(&self) -> Fetches {
        NodeIdRegistry::fetches(self)
    }

    fn id_for(&self, key: MsaaKey) -> NodeId {
        NodeIdRegistry::id_for(self, key)
    }

    fn find(&self, key: MsaaKey, identity: Option<usize>, child: i32, at_address: bool) -> Found {
        NodeIdRegistry::find(self, key, identity, child, at_address)
    }

    fn touch(&self, _id: NodeId) {}

    fn insert(
        &self,
        key: MsaaKey,
        held: Option<Held>,
        role_facts: (Role, Facts),
        at_address: bool,
    ) -> NodeId {
        NodeIdRegistry::insert(self, key, held, role_facts, at_address)
    }

    fn facts_of(&self, id: NodeId) -> Option<Kept> {
        NodeIdRegistry::facts_of(self, id)
    }

    fn reissue(&self, id: NodeId, name: Option<String>) {
        NodeIdRegistry::reissue(self, id, name);
    }

    fn key_of(&self, node: NodeId) -> Option<MsaaKey> {
        NodeIdRegistry::key_of(self, node)
    }

    fn locate(&self, node: NodeId) -> Option<(MsaaKey, Option<KeptObject>, bool)> {
        NodeIdRegistry::locate(self, node)
    }
}

impl NodeIdRegistry {
    /// Creates a registry minting from `counter`. Share one counter across all
    /// backend registries in an outpost to keep identities unique.
    #[must_use]
    pub fn new(counter: Arc<AtomicU64>) -> Self {
        Self {
            counter,
            inner: Arc::new(Mutex::new(Inner::default())),
            fetches: Arc::new(Mutex::new(Fetches::default())),
        }
    }

    /// The details a node read through this registry includes, as the
    /// active theme wants them (`Fetches`): a detail whose indication is
    /// off is not read, which saves its cross-process call. Every detail
    /// until [`set_fetches`](Self::set_fetches) says otherwise.
    #[must_use]
    pub fn fetches(&self) -> Fetches {
        *self.fetches.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes the details nodes read from now on include.
    pub fn set_fetches(&self, fetches: Fetches) {
        *self.fetches.lock().unwrap_or_else(PoisonError::into_inner) = fetches;
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The node at `key`, issuing one without an object on first sight. For
    /// a node read from local window data, which has no accessible object.
    #[must_use]
    pub fn id_for(&self, key: MsaaKey) -> NodeId {
        let mut inner = self.lock();
        if let Some(&id) = inner.by_key.get(&key) {
            return id;
        }
        self.insert_locked(&mut inner, key, None, (None, None), true)
    }

    /// Looks up a new sighting of `key`: first a kept node that was the same
    /// COM object (`identity`) with the same child id in the same window,
    /// then, if the object was acquired at `key`, a kept node acquired at the
    /// same address.
    #[must_use]
    pub fn find(
        &self,
        key: MsaaKey,
        identity: Option<usize>,
        child: i32,
        at_address: bool,
    ) -> Found {
        let inner = self.lock();
        if let Some(identity) = identity
            && let Some(&id) = inner.by_object.get(&(identity, child))
            && let Some(node) = inner.nodes.get(&id)
            && node.key.0 == key.0
        {
            let held = node
                .held
                .as_ref()
                .map(|held| (held.object.clone(), held.child));
            return Found::Object(id, held);
        }
        if !at_address {
            return Found::Nothing;
        }
        let Some(&id) = inner.by_key.get(&key) else {
            return Found::Nothing;
        };
        let node = &inner.nodes[&id];
        let held = node
            .held
            .as_ref()
            .map(|held| (held.object.clone(), held.child));
        Found::Key(id, held, node.role)
    }

    /// Whether `node` is kept.
    #[must_use]
    pub fn holds(&self, node: NodeId) -> bool {
        self.lock().nodes.contains_key(&node)
    }

    /// Issues a new node for `key`, keeping `held` as its object. If the
    /// object was acquired at `key`, the address names the new node from now
    /// on.
    #[must_use]
    pub fn insert(
        &self,
        key: MsaaKey,
        held: Option<Held>,
        (role, facts): (Role, Facts),
        at_address: bool,
    ) -> NodeId {
        let mut inner = self.lock();
        self.insert_locked(&mut inner, key, held, (Some(role), Some(facts)), at_address)
    }

    /// The kept node `id` as a new sighting is compared with it: its
    /// address, kept object, role and facts; `None` when it is not kept or
    /// has no facts.
    #[must_use]
    pub fn facts_of(&self, id: NodeId) -> Option<Kept> {
        let inner = self.lock();
        let node = inner.nodes.get(&id)?;
        Some(Kept {
            key: node.key,
            object: node
                .held
                .as_ref()
                .map(|held| (held.object.clone(), held.child)),
            role: node.role,
            facts: node.facts.clone()?,
        })
    }

    /// A new sighting was found to be the kept node `id`: it keeps its
    /// object, address and event parameters, as NVDA keeps its old objects
    /// through the meeting point, and takes the sighting's name.
    pub fn reissue(&self, id: NodeId, name: Option<String>) {
        if let Some(facts) = self
            .lock()
            .nodes
            .get_mut(&id)
            .and_then(|node| node.facts.as_mut())
        {
            facts.name = name;
        }
    }

    fn insert_locked(
        &self,
        inner: &mut Inner,
        key: MsaaKey,
        held: Option<Held>,
        (role, facts): (Option<Role>, Option<Facts>),
        at_address: bool,
    ) -> NodeId {
        let id = NodeId::new(self.counter.fetch_add(1, Ordering::Relaxed));
        if let Some(held) = &held {
            inner.by_object.insert((held.identity, held.child), id);
        }
        if at_address {
            inner.by_key.insert(key, id);
        }
        inner.nodes.insert(
            id,
            Node {
                key,
                held,
                role,
                at_address,
                facts,
            },
        );
        id
    }

    /// Returns the MSAA address `node` was issued for.
    #[must_use]
    pub fn key_of(&self, node: NodeId) -> Option<MsaaKey> {
        self.lock().nodes.get(&node).map(|node| node.key)
    }

    /// The address and kept object of `node`, and whether the object was
    /// acquired at that address, or `None` if the node is not kept.
    #[must_use]
    pub fn locate(&self, node: NodeId) -> Option<(MsaaKey, Option<KeptObject>, bool)> {
        let inner = self.lock();
        let node = inner.nodes.get(&node)?;
        Some((
            node.key,
            node.held
                .as_ref()
                .map(|held| (held.object.clone(), held.child)),
            node.at_address,
        ))
    }

    /// Every kept node.
    #[must_use]
    pub fn ids(&self) -> Vec<NodeId> {
        self.lock().nodes.keys().copied().collect()
    }

    /// Keeps only the nodes for which `keep` returns `true`. The others are
    /// forgotten at once and returned with their objects, for the caller to
    /// drop once no lock is held.
    pub fn retain(&self, mut keep: impl FnMut(NodeId) -> bool) -> Released {
        let mut inner = self.lock();
        let gone: Vec<NodeId> = inner
            .nodes
            .keys()
            .copied()
            .filter(|&id| !keep(id))
            .collect();
        Released {
            _nodes: remove(&mut inner, &gone),
        }
    }

    /// Releases every node in window `hwnd`, which has been destroyed, so a
    /// reused window handle never inherits them: their ids, and the nodes
    /// with their objects, for the caller to drop once no lock is held.
    pub fn forget_window(&self, hwnd: isize) -> (Vec<NodeId>, Released) {
        let mut inner = self.lock();
        let gone: Vec<NodeId> = inner
            .nodes
            .iter()
            .filter(|(_, node)| node.key.0 == hwnd)
            .map(|(&id, _)| id)
            .collect();
        let nodes = remove(&mut inner, &gone);
        (gone, Released { _nodes: nodes })
    }
}

/// Removes `gone` from every map, returning the removed nodes for the caller
/// to drop outside the lock.
fn remove(inner: &mut Inner, gone: &[NodeId]) -> Vec<Node> {
    gone.iter()
        .filter_map(|id| {
            let node = inner.nodes.remove(id)?;
            if inner.by_key.get(&node.key) == Some(id) {
                inner.by_key.remove(&node.key);
            }
            if let Some(held) = &node.held
                && inner.by_object.get(&(held.identity, held.child)) == Some(id)
            {
                inner.by_object.remove(&(held.identity, held.child));
            }
            Some(node)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The facts of a node read at no address in window 7.
    fn facts() -> Facts {
        Facts {
            event: (7, None, 0),
            identity: None,
            raw_role: None,
            name: None,
        }
    }

    fn registry() -> NodeIdRegistry {
        NodeIdRegistry::new(Arc::new(AtomicU64::new(1)))
    }

    #[test]
    fn same_key_maps_to_same_node_id() {
        let registry = registry();
        let first = registry.id_for((10, 0, 0));
        let again = registry.id_for((10, 0, 0));
        let other = registry.id_for((10, 0, 3));
        assert_eq!(first, again);
        assert_ne!(first, other);
    }

    #[test]
    fn reverse_lookup_round_trips() {
        let registry = registry();
        let id = registry.id_for((7, 0, 4));
        assert_eq!(registry.key_of(id), Some((7, 0, 4)));
    }

    #[test]
    fn a_new_node_at_an_address_takes_it_over() {
        let registry = registry();
        let old = registry.id_for((7, 0, 4));
        let new = registry.insert((7, 0, 4), None, (Role::Button, facts()), true);
        assert_ne!(old, new);
        assert_eq!(
            registry.key_of(old),
            Some((7, 0, 4)),
            "the old node is kept"
        );
        assert!(
            matches!(registry.find((7, 0, 4), None, 4, true), Found::Key(id, None, Some(Role::Button)) if id == new)
        );
    }

    #[test]
    fn a_node_reached_another_way_never_takes_over_an_address() {
        let registry = registry();
        let window = registry.id_for((7, -4, 0));
        let ancestor = registry.insert((7, -4, 0), None, (Role::Group, facts()), false);
        assert_ne!(window, ancestor);
        assert_eq!(registry.id_for((7, -4, 0)), window);
        assert!(matches!(
            registry.find((7, -4, 0), None, 0, false),
            Found::Nothing
        ));
    }

    #[test]
    fn released_nodes_are_forgotten_and_their_address_issues_anew() {
        let registry = registry();
        let kept = registry.id_for((1, 0, 0));
        let released = registry.id_for((2, 0, 0));
        drop(registry.retain(|id| id == kept));
        assert_eq!(registry.ids(), vec![kept]);
        assert_eq!(registry.key_of(released), None);
        assert_ne!(registry.id_for((2, 0, 0)), released);
    }

    #[test]
    fn a_destroyed_window_forgets_its_nodes() {
        let registry = registry();
        let inside = registry.id_for((5, 0, 1));
        let outside = registry.id_for((6, 0, 1));
        let (forgotten, released) = registry.forget_window(5);
        drop(released);
        assert_eq!(forgotten, vec![inside]);
        assert_eq!(registry.key_of(inside), None);
        assert_eq!(registry.key_of(outside), Some((6, 0, 1)));
    }
}
