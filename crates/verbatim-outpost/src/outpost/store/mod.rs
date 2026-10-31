//! The outpost's object store: the one owner of every application object the
//! outpost keeps, and of the identity it gives each
//! (`docs/design/ownership.md`).
//!
//! It owns, as private fields, the UIA and MSAA node registries, the text
//! objects (each node's text patterns, and the UIA ranges of its text
//! positions under the keys the reducer's text facts name them by, with
//! what its text is known to support),
//! the terminals' kept ranges, and the mint tag of every node and anchor:
//! the worker generation and request it was issued or looked up under.
//! No other module holds a registry. A plain window and a foreground's
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
//!   the interning traits of `verbatim-uia` and `verbatim-ia2`: a handle
//!   carries its request's tag ([`Store::nodes_for`]), and every node
//!   issued or looked up through it is tagged with it, in the same hold of
//!   the store's lock. An element a lookup replaces is kept in the handle
//!   for its holder to drop ([`Nodes::take_removed`]).
//!   The nodes of the lines a walk copies, the focus chain it may stop at
//!   and the reports' lines it compares with, are tagged as it copies them
//!   ([`Store::held_prefix`], [`Store::tag_copied`]).
//! - A message is sent as a [`Recorded`], which only the store makes,
//!   listing the nodes and anchors the message names
//!   ([`OutpostToCore::named`]); the outbound queue records them at the
//!   message's position as it numbers it at write.
//! - [`Store::release`] frees a node or anchor only when no live request can
//!   still name it: every tag it has names a dead worker generation or a
//!   claimed or ended request, and either it was never carried, or it was
//!   carried at or before the position Core acknowledged, no waiting message
//!   carries it, and Core does not hold it. Carried positions, waiting
//!   messages and tags are read in one hold of the backlog's lock and then
//!   the store's, the one nesting in the outpost, so no message can be
//!   queued between the decision and the removal.
//! - A text call works on clones of the UIA ranges its copy of the node's
//!   text facts names ([`Store::ranges`]), with no lock held. Its program
//!   commits what it changed, and the shell makes the commit's text writes
//!   only when the reducer accepts the commit ([`Store::write_text`]): the
//!   ranges it made, under their new keys, its anchors tagged with its
//!   request, and the node's patterns and what its text supports. A range
//!   is let go of when no text fact names it any more
//!   ([`Store::forget_ranges`]), or with its node.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_ia2::{
    CHILDID_SELF, Facts, Found, Held, KeptObject, MsaaKey, NodeIdRegistry as MsaaRegistry,
};
use verbatim_model::protocol::OutpostToCore;
use verbatim_model::{CaretReport, Fetches, NodeId, Role};
use verbatim_uia::{Agile, Cached, Kept, NodeIdRegistry as UiaRegistry};

use super::outbound::{Carried, Outbound};
use super::reducer::{RangeKey, TextChange};
use super::text_reads::Patterns;
use crate::terminal::Terminal;
use crate::terminal::reading::Reading;
use crate::text::uia::{TextSupport, UiaPos};

/// The worker generation and request a node or anchor was issued or looked
/// up under: its mint tag. Requests are numbered across generations, from
/// one counter, so no two programs have the same tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Tag {
    pub(super) generation: u64,
    pub(super) request: u64,
}

/// Objects the store let go of, for the caller to drop once no lock is
/// held: releasing an application's object can call into its process.
#[must_use = "drop once no lock is held"]
#[derive(Default)]
pub(super) struct Removed {
    uia: Vec<verbatim_uia::Released>,
    msaa: Vec<verbatim_ia2::Released>,
    /// Elements a lookup replaced as a node's ([`Nodes::keep`]).
    #[expect(dead_code, reason = "kept only to be dropped by the holder")]
    elements: Vec<Agile<Kept>>,
    patterns: Vec<Patterns>,
    /// UIA ranges of text positions.
    ranges: Vec<UiaPos>,
    terminals: Vec<Terminal>,
    /// Objects entries came with, never taken.
    parked: Vec<Parked>,
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

    /// `self`, and the terminal entry a refused commit made, if any.
    pub(super) fn with_terminal(mut self, terminal: Option<Terminal>) -> Self {
        self.terminals.extend(terminal);
        self
    }
}

/// A program's write to the store's text, made by the shell only when the
/// reducer accepts the commit that carries it ([`Store::write_text`]).
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
    pub(super) nodes: HashSet<u64>,
    pub(super) anchors: HashSet<u64>,
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
    /// The tags of each node, by number: every program that issued or
    /// looked it up and was live at the last release.
    nodes: HashMap<u64, Vec<Tag>>,
    /// The tags of each anchor, likewise.
    anchors: HashMap<u64, Vec<Tag>>,
    /// The node of each anchor tagged.
    anchor_nodes: HashMap<u64, u64>,
    /// The UIA ranges of each node's text positions, by node and by the
    /// key its text facts name each by.
    ranges: HashMap<u64, HashMap<RangeKey, UiaPos>>,
    /// Each UIA node's text patterns, once fetched.
    patterns: HashMap<u64, Patterns>,
    /// What each UIA node's text is known to support of the text
    /// attributes, learned from its answers.
    support: HashMap<u64, TextSupport>,
    /// What the outpost keeps of each focused terminal between its reads,
    /// by node (`docs/design/terminal-model.md`, "Where it runs"). A read
    /// works on a copy, which only the commit of its result puts in place.
    terminals: HashMap<u64, Terminal>,
    /// The objects entries came with, parked until their program takes
    /// them, by key ([`Parked`]).
    parked: HashMap<u64, Parked>,
    /// The last key given a parked object.
    parked_taken: u64,
}

/// An application object a UIA event came with, parked in the store as the
/// event is pushed, so the mailbox's entry and the reducer hold only its
/// key (`docs/design/ownership.md`): taken
/// by the program that handles the entry, or let go of into a worker's
/// drop when the entry is removed unhandled.
pub(super) enum Parked {
    /// A property change's element.
    Property(verbatim_uia::Agile<verbatim_uia::PropertyEvent>),
    /// A caret, text, or active text position change's element.
    Text(verbatim_uia::Agile<verbatim_uia::TextEvent>),
    /// The range an active text position change carried.
    Range(windows::core::AgileReference<windows::Win32::UI::Accessibility::IUIAutomationTextRange>),
}

impl Parked {
    /// The element, on this thread, read as the `Kept` shape both event
    /// shapes cover, which is all any reader of it reads and what the node
    /// registry keeps; `None` when it cannot be resolved or is a range.
    pub(super) fn element(self) -> Option<Cached<Kept>> {
        match self {
            Self::Property(agile) => agile.resolve().ok().map(|event| event.narrow()),
            Self::Text(agile) => agile.resolve().ok().map(|event| event.narrow()),
            Self::Range(_) => None,
        }
    }

    /// The range, when it is one.
    pub(super) fn range(
        self,
    ) -> Option<
        windows::core::AgileReference<windows::Win32::UI::Accessibility::IUIAutomationTextRange>,
    > {
        match self {
            Self::Range(range) => Some(range),
            Self::Property(_) | Self::Text(_) => None,
        }
    }
}

/// Adds `tag` to `tags`, once.
fn add(tags: &mut Vec<Tag>, tag: Tag) {
    if !tags.contains(&tag) {
        tags.push(tag);
    }
}

impl State {
    fn tag_node(&mut self, node: NodeId, tag: Tag) {
        add(self.nodes.entry(node.number()).or_default(), tag);
    }

    fn tag_anchors(&mut self, node: u64, anchors: &[u64], tag: Tag) {
        for &anchor in anchors {
            add(self.anchors.entry(anchor).or_default(), tag);
            self.anchor_nodes.insert(anchor, node);
        }
    }

    /// Forgets everything kept for the nodes `gone`: their text, terminal
    /// and tags, and the tags of their anchors, into `removed`.
    fn forget_nodes(&mut self, gone: &HashSet<u64>, removed: &mut Removed) {
        let anchors = &mut self.anchors;
        self.anchor_nodes.retain(|anchor, node| {
            let kept = !gone.contains(node);
            if !kept {
                anchors.remove(anchor);
            }
            kept
        });
        for &node in gone {
            self.nodes.remove(&node);
            if let Some(ranges) = self.ranges.remove(&node) {
                removed.ranges.extend(ranges.into_values());
            }
            removed.patterns.extend(self.patterns.remove(&node));
            self.support.remove(&node);
            removed.terminals.extend(self.terminals.remove(&node));
        }
    }
}

/// The outpost's object store (the module's documentation).
pub(super) struct Store {
    uia: UiaRegistry,
    msaa: MsaaRegistry,
    /// The number of the last request a test tagged.
    #[cfg(test)]
    requests: AtomicU64,
    /// The counter anchors in both backends are numbered from.
    anchor_numbers: Arc<AtomicU64>,
    /// The last key given a UIA range.
    range_keys: AtomicU64,
    state: Mutex<State>,
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
            #[cfg(test)]
            requests: AtomicU64::new(0),
            anchor_numbers: anchors,
            range_keys: AtomicU64::new(0),
            state: Mutex::new(State {
                nodes: HashMap::new(),
                anchors: HashMap::new(),
                anchor_nodes: HashMap::new(),
                ranges: HashMap::new(),
                patterns: HashMap::new(),
                support: HashMap::new(),
                terminals: HashMap::new(),
                parked: HashMap::new(),
                parked_taken: 0,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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

    /// The tag of a new request of worker generation `generation`, for a
    /// test: the reducer numbers its requests itself.
    #[cfg(test)]
    pub(super) fn next_tag(&self, generation: u64) -> Tag {
        Tag {
            generation,
            request: self.requests.fetch_add(1, Ordering::Relaxed) + 1,
        }
    }

    /// The store's handle for the request `tag`, through which a walk
    /// issues and looks up nodes, each tagged with it.
    pub(super) fn nodes_for(&self, tag: Tag) -> Nodes<'_> {
        Nodes {
            store: self,
            generation: tag.generation,
            request: AtomicU64::new(tag.request),
            replaced: Mutex::new(Vec::new()),
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

    /// The live element kept for the UIA node `node`, a clone.
    pub(super) fn element_of(&self, node: NodeId) -> Option<Agile<Kept>> {
        self.uia.element_of(node)
    }

    /// The MSAA address of the MSAA node `node`.
    pub(super) fn key_of(&self, node: NodeId) -> Option<MsaaKey> {
        self.msaa.key_of(node)
    }

    /// The details a node read includes.
    pub(super) fn set_fetches(&self, fetches: Fetches) {
        self.msaa.set_fetches(fetches);
    }

    /// Takes the element kept for `node` out, for the caller to drop.
    pub(super) fn evict_element(&self, node: NodeId) -> Option<Agile<Kept>> {
        self.uia.evict_element(node)
    }

    /// Takes the element kept for `node` out if it is still `element`, for
    /// the caller to drop.
    pub(super) fn evict_element_if(
        &self,
        node: NodeId,
        element: &Agile<Kept>,
    ) -> Option<Agile<Kept>> {
        self.uia.evict_element_if(node, element)
    }

    // The lines a program copies.

    /// How many of `chain`, outermost first, a walk may stop at: those
    /// above the outermost one the store no longer holds. Each of them is
    /// tagged with request `tag`, in the same hold, so no release frees it
    /// while the request may name it.
    pub(super) fn held_prefix(&self, tag: Tag, chain: &[NodeId]) -> usize {
        let mut state = self.lock();
        let held = chain
            .iter()
            .position(|&node| !self.holds(node))
            .unwrap_or(chain.len());
        for &node in &chain[..held] {
            state.tag_node(node, tag);
        }
        held
    }

    /// Tags each of `nodes` the store holds with request `tag`: the lines
    /// it copied, which it may name.
    pub(super) fn tag_copied(&self, tag: Tag, nodes: impl IntoIterator<Item = NodeId>) {
        let mut state = self.lock();
        for node in nodes {
            if self.holds(node) {
                state.tag_node(node, tag);
            }
        }
    }

    // Messages and release.

    /// `message`, with the nodes and anchors it names, as the outbound
    /// queue records them.
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
        let mut seen = HashSet::new();
        carried.retain(|carried| seen.insert(*carried));
        Recorded { message, carried }
    }

    /// Frees every node and anchor no live request can still name (the
    /// module's documentation), for Core's held list `held`, which
    /// acknowledges position `acknowledged`, with `live` the tags of the
    /// requests still live. Returns the position the queue has reached,
    /// for the log, and what was freed, `None` when nothing was.
    pub(super) fn release(
        &self,
        outbound: &Outbound,
        held: &HashSet<Carried>,
        acknowledged: u64,
        live: &[Tag],
    ) -> (u64, Option<Released>) {
        outbound.with_pending(|pending| {
            let mut state = self.lock();
            let free = |carried: Carried, tags: &mut Vec<Tag>| {
                tags.retain(|tag| live.contains(tag));
                tags.is_empty()
                    && pending.report(carried).is_none_or(|(position, waiting)| {
                        waiting == 0 && position <= acknowledged && !held.contains(&carried)
                    })
            };
            let nodes: HashSet<u64> = state
                .nodes
                .iter_mut()
                .filter_map(|(&node, tags)| free(Carried::Node(node), tags).then_some(node))
                .collect();
            let anchors: HashSet<u64> = state
                .anchors
                .iter_mut()
                .filter_map(|(&anchor, tags)| free(Carried::Anchor(anchor), tags).then_some(anchor))
                .collect();
            if nodes.is_empty() && anchors.is_empty() {
                return (pending.position(), None);
            }
            let mut removed = Removed::default();
            removed
                .uia
                .push(self.uia.retain(|id| !nodes.contains(&id.number())));
            removed
                .msaa
                .push(self.msaa.retain(|id| !nodes.contains(&id.number())));
            state.forget_nodes(&nodes, &mut removed);
            // A freed anchor's ranges stay until the reducer, given the
            // anchors freed, says no fact names them ([`Store::forget_ranges`]).
            for anchor in &anchors {
                state.anchors.remove(anchor);
                state.anchor_nodes.remove(anchor);
            }
            // What the queue recorded of anything no longer kept goes, the
            // anchors of a freed node with it.
            pending.forget(|carried| match carried {
                Carried::Node(node) => !state.nodes.contains_key(&node),
                Carried::Anchor(anchor) => !state.anchors.contains_key(&anchor),
            });
            let position = pending.position();
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
    /// them. Returns what it let go of and the nodes it freed, whose text
    /// facts the caller has the reducer forget.
    pub(super) fn forget_window(&self, hwnd: isize) -> (Removed, Vec<NodeId>) {
        let mut state = self.lock();
        let (gone, released) = self.msaa.forget_window(hwnd);
        let numbers: HashSet<u64> = gone.iter().map(|node| node.number()).collect();
        let mut removed = Removed::default();
        removed.msaa.push(released);
        state.forget_nodes(&numbers, &mut removed);
        (removed, gone)
    }

    /// Frees everything, for the outpost's shutdown once no worker runs.
    /// Returns how many nodes there were.
    pub(super) fn release_everything(&self) -> (usize, Removed) {
        let mut state = self.lock();
        let nodes: HashSet<u64> = self
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
        removed
            .terminals
            .extend(state.terminals.drain().map(|(_, terminal)| terminal));
        removed.ranges.extend(
            state
                .ranges
                .drain()
                .flat_map(|(_, ranges)| ranges.into_values()),
        );
        state.nodes.clear();
        state.anchors.clear();
        state.anchor_nodes.clear();
        removed
            .parked
            .extend(state.parked.drain().map(|(_, parked)| parked));
        (nodes.len(), removed)
    }

    // Text.

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

    /// Makes `writes`, the text writes of a commit of request `tag` the
    /// reducer accepts, in order. A change that adds to a node's facts is
    /// made only while the store holds the node: its ranges are kept and
    /// its anchors tagged. Returns the facts' changes made, for the
    /// reducer, and what the writes replaced or did not keep, for the
    /// caller to drop once no lock is held.
    pub(super) fn write_text(
        &self,
        tag: Tag,
        writes: Vec<TextWrite>,
    ) -> (Vec<TextChange>, Removed) {
        let mut removed = Removed::default();
        let mut changes = Vec::new();
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
                    state.tag_anchors(node, &touched, tag);
                    if !ranges.is_empty() {
                        let kept = state.ranges.entry(node).or_default();
                        for (key, range) in ranges {
                            removed.ranges.extend(kept.insert(key, range));
                        }
                    }
                    changes.push(change);
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
        (changes, removed)
    }

    /// Lets go of the UIA ranges `ranges` names, by node and key, which no
    /// text fact names any more.
    pub(super) fn forget_ranges(&self, ranges: &[(u64, RangeKey)]) -> Removed {
        let mut removed = Removed::default();
        let mut state = self.lock();
        for (node, key) in ranges {
            if let Some(kept) = state.ranges.get_mut(node) {
                removed.ranges.extend(kept.remove(key));
                if kept.is_empty() {
                    state.ranges.remove(node);
                }
            }
        }
        removed
    }

    /// Whether anchor `anchor` is tagged, for a test.
    #[cfg(test)]
    pub(super) fn tags_anchor(&self, anchor: u64) -> bool {
        self.lock().anchors.contains_key(&anchor)
    }

    // Terminals.

    /// A copy of terminal `node`'s entry, for a read made with no lock
    /// held.
    pub(super) fn terminal(&self, node: NodeId) -> Option<Terminal> {
        self.lock().terminals.get(&node.number()).cloned()
    }

    /// Terminal `node`'s on-demand reading, when it has an entry.
    pub(super) fn terminal_reading(&self, node: NodeId) -> Option<Reading> {
        self.lock()
            .terminals
            .get(&node.number())
            .map(|terminal| terminal.reading)
    }

    /// Whether terminal `node`'s last read covers a text change observed at
    /// `observed_at_us` ([`Terminal::covers`]).
    pub(super) fn terminal_covers(&self, node: NodeId, observed_at_us: u64) -> bool {
        self.lock()
            .terminals
            .get(&node.number())
            .is_some_and(|terminal| terminal.covers(observed_at_us))
    }

    /// Whether terminal `node` has an entry.
    pub(super) fn has_terminal(&self, node: NodeId) -> bool {
        self.lock().terminals.contains_key(&node.number())
    }

    /// Puts `entry` in place as terminal `node`'s, while the store holds
    /// the node; returns what it replaced, or `entry` for a node released.
    pub(super) fn put_terminal(&self, node: NodeId, entry: Terminal) -> Removed {
        let mut removed = Removed::default();
        let mut state = self.lock();
        if self.holds(node) {
            removed
                .terminals
                .extend(state.terminals.insert(node.number(), entry));
        } else {
            removed.terminals.push(entry);
        }
        removed
    }

    /// Notes `caret` as the caret last sent to Core for terminal `node`,
    /// while the store holds it: in a new entry when `create` and there is
    /// none, else only in the entry there is. A caret is plain data.
    pub(super) fn set_sent_caret(&self, node: NodeId, caret: Option<CaretReport>, create: bool) {
        let mut state = self.lock();
        if !self.holds(node) {
            return;
        }
        if create {
            state.terminals.entry(node.number()).or_default().sent_caret = caret;
        } else if let Some(entry) = state.terminals.get_mut(&node.number()) {
            entry.sent_caret = caret;
        }
    }

    /// Takes terminal `node`'s entry out, for the caller to drop.
    pub(super) fn remove_terminal(&self, node: NodeId) -> Removed {
        let mut removed = Removed::default();
        removed
            .terminals
            .extend(self.lock().terminals.remove(&node.number()));
        removed
    }
}

/// The store's handle for one request: every node issued or looked up
/// through it is tagged with the request's tag, in the same hold of the
/// store's lock as the registry's change. The elements its lookups replaced
/// are kept in it, dropped with it or taken by its holder
/// ([`take_removed`](Self::take_removed)): a handle used under a lock, the
/// watchdog's, has them taken before the lock is let go.
pub(super) struct Nodes<'a> {
    store: &'a Store,
    /// The request's tag, its generation and its number, which
    /// [`retag`](Self::retag) changes when the worker starts owed work.
    generation: u64,
    request: AtomicU64,
    replaced: Mutex<Vec<Agile<Kept>>>,
}

impl Nodes<'_> {
    /// The tag of the request this handle tags with.
    pub(super) fn tag(&self) -> Tag {
        Tag {
            generation: self.generation,
            request: self.request.load(Ordering::Relaxed),
        }
    }

    /// The elements this handle's lookups replaced, for the caller to drop
    /// once no lock is held.
    pub(super) fn take_removed(&self) -> Removed {
        Removed {
            elements: std::mem::take(
                &mut *self.replaced.lock().unwrap_or_else(PoisonError::into_inner),
            ),
            ..Removed::default()
        }
    }

    /// The kept UIA node for `runtime_id`, if there is one, looked up.
    pub(super) fn existing(&self, runtime_id: &[i32]) -> Option<NodeId> {
        let mut state = self.store.lock();
        let node = self.store.uia.existing_id(runtime_id)?;
        state.tag_node(node, self.tag());
        Some(node)
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
        let mut state = self.store.lock();
        let node = self.store.msaa.id_for(key);
        state.tag_node(node, self.tag());
        node
    }

    /// The UIA node for `runtime_id`, issued or looked up, keeping
    /// `element` as its live element: an agile reference made already. The
    /// element it replaces is kept in the handle.
    pub(super) fn keep(&self, runtime_id: &[i32], element: Option<Agile<Kept>>) -> NodeId {
        let (node, replaced) = {
            let mut state = self.store.lock();
            let (node, replaced) = self.store.uia.keep(runtime_id, element);
            state.tag_node(node, self.tag());
            (node, replaced)
        };
        self.replaced
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(replaced);
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
    fn fetches(&self) -> Fetches {
        self.store.msaa.fetches()
    }

    fn id_for(&self, key: MsaaKey) -> NodeId {
        self.msaa_key(key)
    }

    fn find(&self, key: MsaaKey, identity: Option<usize>, child: i32, at_address: bool) -> Found {
        let mut state = self.store.lock();
        let found = self.store.msaa.find(key, identity, child, at_address);
        if let Found::Object(node, _) | Found::Key(node, _, _) = &found {
            state.tag_node(*node, self.tag());
        }
        found
    }

    fn touch(&self, id: NodeId) {
        let mut state = self.store.lock();
        if self.store.msaa.holds(id) {
            state.tag_node(id, self.tag());
        }
    }

    fn insert(
        &self,
        key: MsaaKey,
        held: Option<Held>,
        role_facts: (Role, Facts),
        at_address: bool,
    ) -> NodeId {
        let mut state = self.store.lock();
        let node = self.store.msaa.insert(key, held, role_facts, at_address);
        state.tag_node(node, self.tag());
        node
    }

    fn facts_of(&self, id: NodeId) -> Option<verbatim_ia2::Kept> {
        self.store.msaa.facts_of(id)
    }

    fn reissue(&self, id: NodeId, name: Option<String>) {
        let mut state = self.store.lock();
        if self.store.msaa.holds(id) {
            self.store.msaa.reissue(id, name);
            state.tag_node(id, self.tag());
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
