//! The outpost's object store: the one owner of every application object the
//! outpost keeps, and of the identity it gives each
//! (`books/dev/src/explanation/design/ownership.md`).
//!
//! It owns, as private fields, the UIA and MSAA node registries, each
//! node's text facts ([`TextFacts`]), the text objects (each node's text
//! patterns, and the UIA ranges of its text positions under the keys its
//! text facts name them by, with what its text is known to support),
//! the terminals' kept ranges, the text anchors it keeps, and the
//! elements of the caret tells sent to Core. No other module holds a
//! registry. A plain window and a foreground's
//! client object read from local window data are minted by their window
//! handle, as their MSAA keys `(hwnd, OBJID_WINDOW, 0)` and
//! `(hwnd, OBJID_CLIENT, 0)`, so ids do not change and the MSAA registry's
//! map of keys is the store's map of window handles ([`Nodes::window`],
//! [`Nodes::client`]).
//!
//! - Its API returns owned clones of agile references and owned removed
//!   objects, never a guard, and takes no closure, so no caller can hold
//!   its lock across a call into the application; and no method drops a
//!   removed object, so nothing is dropped under its lock or the outbound
//!   backlog's ([`Removed`]).
//! - A walk issues and looks up nodes through [`Nodes`], which implements
//!   the interning traits of `verbatim-uia` and `verbatim-ia2`, with the
//!   node details its request names ([`Store::nodes_for`]). An element a
//!   lookup replaces is kept in the handle and dropped with it.
//! - A message is sent as a [`Recorded`], which only the store makes,
//!   listing the nodes and anchors the message names
//!   ([`OutpostToCore::named`]) and the element a caret tell names by its
//!   token; the outbound queue records them at the message's position as
//!   it numbers it at write.
//! - [`Store::release`] runs between programs, when no program can still
//!   name what it frees, and frees a node, anchor or caret tell's element
//!   Core does not hold that either no message carried, or was carried at
//!   or before the position Core acknowledged with no waiting message
//!   carrying it: by position and Core's held view alone, the rule of
//!   [`verbatim_reader_rules::numbered`]. Carried positions and waiting
//!   messages are read in one hold of the backlog's lock and then the
//!   store's, the one nesting in the outpost, so no message can be queued
//!   between the decision and the removal.
//! - A program reads a copy of the text facts taken as it starts
//!   ([`Store::text_facts`]). A text call works on clones of the UIA ranges
//!   its copy of the node's text facts names ([`Store::ranges`]), with no
//!   lock held. Its program commits what it changed, and the shell makes
//!   the commit's text writes only when the reducer accepts the commit
//!   ([`Store::write_text`]): the facts' changes, the ranges it made, under
//!   their new keys, its anchors kept, and the node's patterns and what its
//!   text supports. A range is let go of in the same hold of the lock as
//!   the change or release after which no text fact names it, or with its
//!   node. No rule of the reducer reads the text facts, so the store holds
//!   them; their type stays in `verbatim-outpost-logic`, pure data tested
//!   on any host (Dickson's confirmed default, 2026-11-20).

#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::lock::{Lock, LockGuard};

use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_ia2::{
    CHILDID_SELF, Facts, Found, Held, KeptObject, MsaaKey, NodeIdRegistry as MsaaRegistry,
};
use verbatim_model::property::NodeDetailSet;
use verbatim_model::protocol::OutpostToCore;
use verbatim_model::{NodeId, Role};
use verbatim_uia::{Agile, Cached, Kept, NodeIdRegistry as UiaRegistry};

use super::outbound::{Carried, Outbound};
use super::reducer::{RangeKey, TextChange, TextFacts};
use super::text_reads::Patterns;
use crate::text::uia::{TextSupport, UiaPos};
use verbatim_reader_rules::numbered::too_early;

pub(super) mod element;

/// Objects the store let go of, for the caller to drop once no lock is
/// held: releasing an application's object can call into its process.
#[must_use = "drop once no lock is held"]
#[derive(Default)]
pub(super) struct Removed {
    uia: Vec<verbatim_uia::Released>,
    msaa: Vec<verbatim_ia2::Released>,
    patterns: Vec<Patterns>,
    /// UIA ranges of text positions.
    ranges: Vec<UiaPos>,
    /// Objects entries came with, never taken.
    parked: Vec<Parked>,
}

impl Drop for Removed {
    /// Releasing an application's object can call into its process, so it
    /// is never made under one of the outpost's locks, checked in debug
    /// builds; the fields are dropped after this.
    fn drop(&mut self) {
        verbatim_win32::held_locks::check("the release of the store's removed objects");
    }
}

impl Removed {
    /// The objects `writes` hold, for a commit the reducer refused.
    pub(super) fn of_text(writes: Vec<TextWrite>) -> Self {
        let mut removed = Self::default();
        for write in writes {
            match write {
                TextWrite::Facts { ranges, .. } => {
                    removed
                        .ranges
                        .extend(ranges.into_iter().map(|(_, range)| range));
                }
                TextWrite::Patterns(_, patterns) => removed.patterns.push(patterns),
                TextWrite::NoTextForgotten(_) | TextWrite::Support(..) | TextWrite::Forget(_) => {}
            }
        }
        removed
    }
}

/// A program's write to the store's text, made by the shell only when the
/// reducer accepts the commit that carries it ([`Store::write_text`]).
#[expect(
    clippy::large_enum_variant,
    reason = "a commit's write, which lives as long as the commit"
)]
pub(super) enum TextWrite {
    /// A change of a node's text facts, with the UIA ranges its text call
    /// made, under the keys the change names them by, and the anchors it
    /// minted or reported, to tag with the commit's request.
    Facts {
        change: TextChange,
        ranges: Vec<(RangeKey, UiaPos)>,
        touched: Vec<u64>,
    },
    /// A UIA node's text patterns, fetched.
    Patterns(NodeId, Patterns),
    /// A UIA node's answer of no text pattern, forgotten.
    NoTextForgotten(NodeId),
    /// What a UIA node's text is known to support.
    Support(NodeId, TextSupport),
    /// A node's patterns and what its text supports, forgotten with its
    /// text facts.
    Forget(NodeId),
}

/// What a release freed ([`Store::release`]): the nodes and anchors, by
/// number, and their objects, for the caller to drop once no lock is held.
pub(super) struct Released {
    pub(super) nodes: BTreeSet<u64>,
    pub(super) anchors: BTreeSet<u64>,
    pub(super) objects: Removed,
}

/// A message, with the nodes and anchors it names, which the outbound
/// queue records as carried by it as it numbers it at write. Only the store
/// makes one ([`Store::record`]), so no message that names a node is sent
/// without them.
pub(super) struct Recorded {
    message: OutpostToCore,
    carried: Vec<Carried>,
}

impl Recorded {
    /// The message, and what it carries.
    pub(super) fn into_parts(self) -> (OutpostToCore, Vec<Carried>) {
        (self.message, self.carried)
    }
}

/// What the store keeps under its lock besides the registries.
struct State {
    /// Each node's text facts, shared with the programs' copies until a
    /// change.
    text: Arc<TextFacts>,
    /// The node of each anchor kept.
    anchor_nodes: HashMap<u64, u64>,
    /// The UIA ranges of each node's text positions, by node and by the
    /// key its text facts name each by.
    ranges: HashMap<u64, HashMap<RangeKey, UiaPos>>,
    /// Each UIA node's text patterns, once fetched.
    patterns: HashMap<u64, Patterns>,
    /// What each UIA node's text is known to support of the text
    /// attributes, learned from its answers.
    support: HashMap<u64, TextSupport>,
    /// Each UIA node's nearest window, recorded when the node is built and
    /// kept after its element dies, as NVDA's object keeps the window it
    /// found when it was made (`NVDAObjects/UIA/__init__.py` 1604 to 1607).
    nearest_windows: HashMap<u64, isize>,
    /// The route to the outpost's copy of each UIA node minted from a
    /// listener's read, by which its element is found again
    /// (`books/dev/src/reference/crates/verbatim-outpost.md`, "Reading the
    /// focus"), released with the node.
    routes: HashMap<u64, super::focus_path::Route>,
    /// The objects entries came with, parked until their program takes
    /// them, by key ([`Parked`]).
    parked: HashMap<u64, Parked>,
    /// The last key given a parked object.
    parked_taken: u64,
    /// The keys of the parked elements of the caret tells sent to Core,
    /// by their tokens: kept for Core's caret read to adopt until a
    /// release frees them, once Core has handled their tells and its held
    /// view leaves them out ([`Store::release`]).
    told: BTreeSet<u64>,
}

/// An application object a UIA event came with, parked in the store as the
/// event is pushed, so the mailbox's entry and the reducer hold only its
/// key (`books/dev/src/explanation/design/ownership.md`): taken
/// by the program that handles the entry, or let go of into a worker's
/// drop when the entry is removed unhandled.
pub(super) enum Parked {
    /// A property change's element.
    Property(verbatim_uia::Agile<verbatim_uia::PropertyEvent>),
    /// A caret or text change's element.
    Text(verbatim_uia::Agile<verbatim_uia::TextEvent>),
}

impl Parked {
    /// The element, on this thread, read as the `Kept` shape both event
    /// shapes cover, which is all any reader of it reads and what the node
    /// registry keeps; `None` when it cannot be resolved.
    pub(super) fn element(self) -> Option<Cached<Kept>> {
        match self {
            Self::Property(agile) => agile.resolve().ok().map(|event| event.narrow()),
            Self::Text(agile) => agile.resolve().ok().map(|event| event.narrow()),
        }
    }
}

impl State {
    /// Forgets everything kept for the nodes `gone`: their text facts and
    /// objects, terminal and anchors, into `removed`.
    fn forget_nodes(&mut self, gone: &BTreeSet<u64>, removed: &mut Removed) {
        if !gone.is_empty() {
            // The nodes' ranges go with them below.
            let _ = Arc::make_mut(&mut self.text).release(gone, &BTreeSet::new());
        }
        self.anchor_nodes.retain(|_, node| !gone.contains(node));
        for &node in gone {
            if let Some(ranges) = self.ranges.remove(&node) {
                removed.ranges.extend(ranges.into_values());
            }
            removed.patterns.extend(self.patterns.remove(&node));
            self.support.remove(&node);
            self.nearest_windows.remove(&node);
            self.routes.remove(&node);
        }
    }

    /// Lets go of the UIA ranges `ranges` names, by node and key, which no
    /// text fact names any more, into `removed`.
    fn forget_ranges(&mut self, ranges: &[(u64, RangeKey)], removed: &mut Removed) {
        for (node, key) in ranges {
            if let Some(kept) = self.ranges.get_mut(node) {
                removed.ranges.extend(kept.remove(key));
                if kept.is_empty() {
                    self.ranges.remove(node);
                }
            }
        }
    }
}

/// The outpost's object store (the module's documentation).
pub(super) struct Store {
    uia: UiaRegistry,
    msaa: MsaaRegistry,
    /// The counter anchors in both backends are numbered from.
    anchor_numbers: Arc<AtomicU64>,
    /// The last key given a UIA range.
    range_keys: AtomicU64,
    state: Lock<State>,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    /// An empty store: both registries mint from one counter, and anchors
    /// are numbered from another.
    pub(super) fn new() -> Self {
        let ids = Arc::new(AtomicU64::new(1));
        let anchors = Arc::new(AtomicU64::new(0));
        Self {
            uia: UiaRegistry::new(Arc::clone(&ids)),
            msaa: MsaaRegistry::new(ids),
            anchor_numbers: anchors,
            range_keys: AtomicU64::new(0),
            state: Lock::new(State {
                text: Arc::default(),
                anchor_nodes: HashMap::new(),
                ranges: HashMap::new(),
                patterns: HashMap::new(),
                support: HashMap::new(),
                nearest_windows: HashMap::new(),
                routes: HashMap::new(),
                parked: HashMap::new(),
                parked_taken: 0,
                told: BTreeSet::new(),
            }),
        }
    }

    #[track_caller]
    fn lock(&self) -> LockGuard<'_, State> {
        self.state.lock()
    }

    /// Parks `object`, returning the key it is parked under.
    pub(super) fn park(&self, object: Parked) -> u64 {
        let mut state = self.lock();
        state.parked_taken += 1;
        let key = state.parked_taken;
        state.parked.insert(key, object);
        key
    }

    /// Takes the object parked under `key`, if it is still parked.
    pub(super) fn take_parked(&self, key: u64) -> Option<Parked> {
        self.lock().parked.remove(&key)
    }

    /// Keeps the element parked under `key` for the caret tell whose
    /// token it is, until a release frees it ([`Store::release`]).
    pub(super) fn keep_told(&self, key: u64) {
        self.lock().told.insert(key);
    }

    /// The store's handle for a request whose reads fetch `details`,
    /// through which a walk issues and looks up nodes.
    pub(super) fn nodes_for(&self, details: NodeDetailSet) -> Nodes<'_> {
        Nodes {
            store: self,
            replaced: Lock::new(Vec::new()),
            details,
        }
    }

    // Reads, which issue nothing and tag nothing.

    /// Whether either registry keeps `node`.
    pub(super) fn holds(&self, node: NodeId) -> bool {
        self.uia.holds(node) || self.msaa.holds(node)
    }

    /// The runtime id of the UIA node `node`.
    pub(super) fn runtime_id_of(&self, node: NodeId) -> Option<Vec<i32>> {
        self.uia.runtime_id_of(node)
    }

    /// The live element kept for the UIA node `node`, a clone: private to
    /// the store and its one way to a node's element ([`element`]), so no
    /// reader takes the kept element and misses one its route finds.
    fn element_of(&self, node: NodeId) -> Option<Agile<Kept>> {
        self.uia.element_of(node)
    }

    /// The UIA node issued for the element `runtime_id` names, if one is.
    pub(super) fn uia_node_of(&self, runtime_id: &[i32]) -> Option<NodeId> {
        self.uia.existing_id(runtime_id)
    }

    /// The MSAA address of the MSAA node `node`.
    pub(super) fn key_of(&self, node: NodeId) -> Option<MsaaKey> {
        self.msaa.key_of(node)
    }

    /// The MSAA address of the MSAA node `node` when it was acquired at that
    /// address, so the address identifies it: none for an object reached
    /// through its parent or as a child object, whose address is made up.
    pub(super) fn address_of(&self, node: NodeId) -> Option<MsaaKey> {
        self.held_at(node)
            .and_then(|(key, at_address)| at_address.then_some(key))
    }

    /// The key the MSAA node `node` is kept under, and whether it was
    /// acquired at that address ([`Self::address_of`]), in one read.
    pub(super) fn held_at(&self, node: NodeId) -> Option<(MsaaKey, bool)> {
        self.msaa
            .locate(node)
            .map(|(key, _, at_address)| (key, at_address))
    }

    /// The nearest window recorded for the UIA node `node`
    /// ([`Self::record_nearest_window`]).
    pub(super) fn nearest_window_of(&self, node: NodeId) -> Option<isize> {
        self.lock().nearest_windows.get(&node.number()).copied()
    }

    /// Records `window` as the nearest window of the UIA node `node`, as the
    /// node is built, forgotten with the node.
    pub(super) fn record_nearest_window(&self, node: NodeId, window: isize) {
        self.lock().nearest_windows.insert(node.number(), window);
    }

    /// Records `route` as the way to the UIA node `node`'s element, minted
    /// from a listener's read, forgotten with the node.
    pub(super) fn record_route(&self, node: NodeId, route: super::focus_path::Route) {
        self.lock().routes.insert(node.number(), route);
    }

    /// The route recorded for the element `runtime_id` names
    /// ([`Self::record_route`]).
    pub(super) fn route_to(&self, runtime_id: &[i32]) -> Option<super::focus_path::Route> {
        self.lock()
            .routes
            .values()
            .find(|route| route.runtime_id == runtime_id)
            .cloned()
    }

    /// Takes the element kept for `node` out if it is still `element`, for
    /// the caller to drop ([`element::let_go`]).
    fn evict_element_if(&self, node: NodeId, element: &Agile<Kept>) -> Option<Agile<Kept>> {
        self.uia.evict_element_if(node, element)
    }

    // Messages and release.

    /// `message`, with the nodes and anchors it names and the element a
    /// caret tell names by its token, as the outbound queue records them.
    #[expect(
        clippy::unused_self,
        reason = "only the store makes a `Recorded`, so a message is sent through it"
    )]
    pub(super) fn record(&self, message: OutpostToCore) -> Recorded {
        let named = message.named();
        let mut carried: Vec<Carried> = named
            .nodes
            .iter()
            .map(|node| Carried::Node(node.number()))
            .chain(named.anchors.iter().map(|anchor| Carried::Anchor(anchor.0)))
            .collect();
        if let OutpostToCore::Tell(tell) = &message
            && let verbatim_model::tell::EventObject::Uia { token, .. } = &tell.object
        {
            carried.push(Carried::Told(token.0));
        }
        let mut seen = HashSet::new();
        carried.retain(|carried| seen.insert(*carried));
        Recorded { message, carried }
    }

    /// Frees every node, anchor and caret tell's element Core does not hold
    /// and has handled every message carrying (the module's documentation),
    /// for Core's held view `held`, which acknowledges position
    /// `acknowledged`. Returns the position the queue has reached, for the
    /// log, and what was freed, `None` when nothing was.
    pub(super) fn release(
        &self,
        outbound: &Outbound,
        held: &HashSet<Carried>,
        acknowledged: u64,
    ) -> (u64, Option<Released>) {
        outbound.with_pending(|pending| {
            let numbered = pending.numbered();
            let mut state = self.lock();
            // A release runs between programs, so what no message carried
            // no program can still name.
            let free = |carried: Carried| {
                let report = numbered.report(carried);
                !held.contains(&carried) && (report.is_none() || !too_early(report, acknowledged))
            };
            let nodes: BTreeSet<u64> = self
                .uia
                .ids()
                .into_iter()
                .chain(self.msaa.ids())
                .map(NodeId::number)
                .filter(|&node| free(Carried::Node(node)))
                .collect();
            let anchors: BTreeSet<u64> = state
                .anchor_nodes
                .keys()
                .copied()
                .filter(|&anchor| free(Carried::Anchor(anchor)))
                .collect();
            let told: Vec<u64> = state
                .told
                .iter()
                .copied()
                .filter(|&key| free(Carried::Told(key)))
                .collect();
            if nodes.is_empty() && anchors.is_empty() && told.is_empty() {
                return (numbered.position(), None);
            }
            let mut removed = Removed::default();
            removed
                .uia
                .push(self.uia.retain(|id| !nodes.contains(&id.number())));
            removed
                .msaa
                .push(self.msaa.retain(|id| !nodes.contains(&id.number())));
            state.forget_nodes(&nodes, &mut removed);
            for anchor in &anchors {
                state.anchor_nodes.remove(anchor);
            }
            if !anchors.is_empty() {
                let unnamed = Arc::make_mut(&mut state.text).release(&BTreeSet::new(), &anchors);
                state.forget_ranges(&unnamed, &mut removed);
            }
            for key in told {
                state.told.remove(&key);
                removed.parked.extend(state.parked.remove(&key));
            }
            // What the queue recorded of anything no longer kept goes, the
            // anchors of a freed node with it.
            numbered.forget_where(|carried| match carried {
                Carried::Node(node) => !self.holds(NodeId::new(node)),
                Carried::Anchor(anchor) => !state.anchor_nodes.contains_key(&anchor),
                Carried::Told(key) => !state.told.contains(&key),
            });
            let position = numbered.position();
            drop(state);
            (
                position,
                Some(Released {
                    nodes,
                    anchors,
                    objects: removed,
                }),
            )
        })
    }

    /// Frees every node of window `hwnd`, which has been destroyed, so a
    /// reused window handle never inherits them, with everything kept for
    /// them, their text facts included. Returns what it let go of.
    pub(super) fn forget_window(&self, hwnd: isize) -> Removed {
        let mut state = self.lock();
        let (gone, released) = self.msaa.forget_window(hwnd);
        let numbers: BTreeSet<u64> = gone.iter().map(|node| node.number()).collect();
        let mut removed = Removed::default();
        removed.msaa.push(released);
        state.forget_nodes(&numbers, &mut removed);
        removed
    }

    /// Frees everything, for the outpost's shutdown once no worker runs.
    /// Returns how many nodes there were.
    pub(super) fn release_everything(&self) -> (usize, Removed) {
        let mut state = self.lock();
        let nodes: BTreeSet<u64> = self
            .uia
            .ids()
            .into_iter()
            .chain(self.msaa.ids())
            .map(NodeId::number)
            .collect();
        let mut removed = Removed::default();
        removed.uia.push(self.uia.retain(|_| false));
        removed.msaa.push(self.msaa.retain(|_| false));
        state.forget_nodes(&nodes, &mut removed);
        removed
            .patterns
            .extend(state.patterns.drain().map(|(_, patterns)| patterns));
        state.support.clear();
        removed.ranges.extend(
            state
                .ranges
                .drain()
                .flat_map(|(_, ranges)| ranges.into_values()),
        );
        state.anchor_nodes.clear();
        state.text = Arc::default();
        removed
            .parked
            .extend(state.parked.drain().map(|(_, parked)| parked));
        state.told.clear();
        (nodes.len(), removed)
    }

    // Text.

    /// The text facts, a copy for a program as it starts: only the program
    /// in flight's commits change them, and a release runs between
    /// programs, so they are the facts as the program's dispatch left them.
    pub(super) fn text_facts(&self) -> Arc<TextFacts> {
        Arc::clone(&self.lock().text)
    }

    /// The text patterns kept for `node`: `None` when none were fetched,
    /// `Some(None)` when its element answered it has none. Clones.
    pub(super) fn patterns(&self, node: NodeId) -> Option<Patterns> {
        self.lock().patterns.get(&node.number()).cloned()
    }

    /// What `node`'s text is known to support.
    pub(super) fn support(&self, node: NodeId) -> TextSupport {
        self.lock()
            .support
            .get(&node.number())
            .copied()
            .unwrap_or_default()
    }

    /// The counter anchors are numbered from, which a text call's copy
    /// mints from.
    pub(super) fn anchor_numbers(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.anchor_numbers)
    }

    /// A new key for a UIA range a text call made.
    pub(super) fn next_range_key(&self) -> RangeKey {
        self.range_keys.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Clones of the UIA ranges of `node`'s text kept under `keys`, those
    /// it keeps, in one hold, for a text call made with no lock held.
    pub(super) fn ranges(&self, node: NodeId, keys: &[RangeKey]) -> Vec<(RangeKey, UiaPos)> {
        let state = self.lock();
        let Some(kept) = state.ranges.get(&node.number()) else {
            return Vec::new();
        };
        keys.iter()
            .filter_map(|&key| Some((key, kept.get(&key)?.clone())))
            .collect()
    }

    /// Makes `writes`, the text writes of a commit the reducer accepts, in
    /// order. A change that adds to a node's facts is made only while the
    /// store holds the node: its ranges and its anchors are kept. The UIA
    /// ranges no text fact names once the changes are made are let go of.
    /// Returns what the writes replaced or did not keep, for the caller to
    /// drop once no lock is held.
    pub(super) fn write_text(&self, writes: Vec<TextWrite>) -> Removed {
        let mut removed = Removed::default();
        let mut unnamed = Vec::new();
        let mut state = self.lock();
        for write in writes {
            match write {
                TextWrite::Facts {
                    change,
                    ranges,
                    touched,
                } => {
                    let node = change.node();
                    if change.adds() && !self.holds(NodeId::new(node)) {
                        removed
                            .ranges
                            .extend(ranges.into_iter().map(|(_, range)| range));
                        continue;
                    }
                    for anchor in touched {
                        state.anchor_nodes.insert(anchor, node);
                    }
                    if !ranges.is_empty() {
                        let kept = state.ranges.entry(node).or_default();
                        for (key, range) in ranges {
                            removed.ranges.extend(kept.insert(key, range));
                        }
                    }
                    unnamed.extend(Arc::make_mut(&mut state.text).apply(&change));
                }
                TextWrite::Patterns(node, patterns) => {
                    if self.holds(node) {
                        removed
                            .patterns
                            .extend(state.patterns.insert(node.number(), patterns));
                    } else {
                        removed.patterns.push(patterns);
                    }
                }
                TextWrite::NoTextForgotten(node) => {
                    if state
                        .patterns
                        .get(&node.number())
                        .is_some_and(Option::is_none)
                    {
                        state.patterns.remove(&node.number());
                    }
                }
                TextWrite::Support(node, support) => {
                    if self.holds(node) {
                        state.support.insert(node.number(), support);
                    }
                }
                TextWrite::Forget(node) => {
                    removed
                        .patterns
                        .extend(state.patterns.remove(&node.number()));
                    state.support.remove(&node.number());
                }
            }
        }
        state.forget_ranges(&unnamed, &mut removed);
        removed
    }

    /// Whether the caret tell's element under `key` is kept, for a test.
    #[cfg(test)]
    pub(super) fn keeps_told(&self, key: u64) -> bool {
        self.lock().told.contains(&key)
    }

    /// Whether anchor `anchor` is kept, for a test.
    #[cfg(test)]
    pub(super) fn keeps_anchor(&self, anchor: u64) -> bool {
        self.lock().anchor_nodes.contains_key(&anchor)
    }
}

/// The store's handle for one request, through which its walks issue and
/// look up nodes. The elements its lookups replaced are kept in it and
/// dropped with it.
pub(super) struct Nodes<'a> {
    store: &'a Store,
    replaced: Lock<Vec<Agile<Kept>>>,
    /// The node details the request's reads fetch, as its property set
    /// names them.
    details: NodeDetailSet,
}

impl Nodes<'_> {
    /// The kept UIA node for `runtime_id`, if there is one, looked up.
    pub(super) fn existing(&self, runtime_id: &[i32]) -> Option<NodeId> {
        self.store.uia.existing_id(runtime_id)
    }

    /// The node of window `hwnd` read from local window data, a plain
    /// window, by its handle, issued or looked up: its MSAA key
    /// `(hwnd, OBJID_WINDOW, 0)`, so a window's node is the same however it
    /// is reached.
    pub(super) fn window(&self, hwnd: isize) -> NodeId {
        self.msaa_key((hwnd, OBJID_WINDOW.0, CHILDID_SELF))
    }

    /// The client object of window `hwnd` read from local window data, a
    /// foreground's, by its handle, issued or looked up: its MSAA key
    /// `(hwnd, OBJID_CLIENT, 0)`.
    pub(super) fn client(&self, hwnd: isize) -> NodeId {
        self.msaa_key((hwnd, OBJID_CLIENT.0, CHILDID_SELF))
    }

    /// The MSAA node at `key` with no object, issued or looked up.
    fn msaa_key(&self, key: MsaaKey) -> NodeId {
        self.store.msaa.id_for(key)
    }

    /// The UIA node for `runtime_id`, issued or looked up, keeping
    /// `element` as its live element: an agile reference made already. The
    /// element it replaces is kept in the handle.
    pub(super) fn keep(&self, runtime_id: &[i32], element: Option<Agile<Kept>>) -> NodeId {
        let (node, replaced) = self.store.uia.keep(runtime_id, element);
        self.replaced.lock().extend(replaced);
        node
    }
}

impl verbatim_uia::Interning for Nodes<'_> {
    fn id_for(&self, runtime_id: &[i32]) -> NodeId {
        self.keep(runtime_id, None)
    }

    fn id_for_element(&self, runtime_id: &[i32], element: &Cached<Kept>) -> NodeId {
        // Made before the lock: making an agile reference can call into
        // the element's process.
        self.keep(runtime_id, element.agile().ok())
    }
}

impl verbatim_ia2::Interning for Nodes<'_> {
    fn details(&self) -> NodeDetailSet {
        self.details
    }

    fn id_for(&self, key: MsaaKey) -> NodeId {
        self.msaa_key(key)
    }

    fn find(&self, key: MsaaKey, identity: Option<usize>, child: i32, at_address: bool) -> Found {
        self.store.msaa.find(key, identity, child, at_address)
    }

    fn touch(&self, _: NodeId) {}

    fn insert(
        &self,
        key: MsaaKey,
        held: Option<Held>,
        role_facts: (Role, Facts),
        at_address: bool,
    ) -> NodeId {
        self.store.msaa.insert(key, held, role_facts, at_address)
    }

    fn facts_of(&self, id: NodeId) -> Option<verbatim_ia2::Kept> {
        self.store.msaa.facts_of(id)
    }

    fn reissue(&self, id: NodeId, name: Option<String>) {
        if self.store.msaa.holds(id) {
            self.store.msaa.reissue(id, name);
        }
    }

    fn key_of(&self, node: NodeId) -> Option<MsaaKey> {
        self.store.msaa.key_of(node)
    }

    fn locate(&self, node: NodeId) -> Option<(MsaaKey, Option<KeptObject>, bool)> {
        self.store.msaa.locate(node)
    }
}

#[cfg(test)]
mod tests;
