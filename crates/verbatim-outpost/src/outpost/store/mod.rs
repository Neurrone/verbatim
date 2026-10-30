//! The outpost's object store: the one owner of every application object the
//! outpost keeps, and of the identity it gives each
//! (`docs/design/ownership-outpost-reducer.md`, "The store").
//!
//! It owns, as private fields, the UIA and MSAA node registries, the text
//! objects (each node's text patterns, and its anchors' and kept
//! selection's provider ranges, with what its text is known to support),
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
//!   the interning traits of `verbatim-uia` and `verbatim-ia2`: every node
//!   issued or looked up through it is tagged with the program the calling
//!   thread runs ([`Store::enter`]), in the same hold of the store's lock.
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
//! - A text call works on a copy of its node's anchors ([`Store::uia_text`],
//!   [`Store::edit_text`]) with no lock held, and its result is put back
//!   ([`Store::put_uia_text`], [`Store::put_edit_text`]), its anchors
//!   tagged; a copy of a request no longer live is not put back, which the
//!   caller decides under the watch lock.

#![forbid(unsafe_code)]

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};

use verbatim_ia2::{
    CHILDID_SELF, Facts, Found, Held, KeptObject, MsaaKey, NodeIdRegistry as MsaaRegistry,
};
use verbatim_model::protocol::OutpostToCore;
use verbatim_model::{CaretReport, Fetches, NodeId, Role, TextPosition};
use verbatim_uia::{Agile, Cached, Kept, NodeIdRegistry as UiaRegistry};

use super::outbound::{Carried, Outbound};
use super::text_reads::Patterns;
use crate::terminal::Terminal;
use crate::terminal::reading::Reading;
use crate::text::uia::{TextSupport, UiaPos};
use crate::text::{Anchors, Forgotten, NodeTextCopy};

/// The worker generation and request a node or anchor was issued or looked
/// up under: its mint tag. Requests are numbered across generations, from
/// one counter, so no two programs have the same tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Tag {
    pub(super) generation: u64,
    pub(super) request: u64,
}

impl Tag {
    /// The tag of a thread that runs no program: no request is it, so it is
    /// never live, and what it names is kept only while a message carries
    /// it.
    pub(super) const NONE: Tag = Tag {
        generation: u64::MAX,
        request: 0,
    };
}

thread_local! {
    /// The program the calling thread runs ([`Store::enter`]).
    static PROGRAM: Cell<Tag> = const { Cell::new(Tag::NONE) };
}

/// The tag of the program the calling thread runs, [`Tag::NONE`] when it
/// runs none.
pub(super) fn current() -> Tag {
    PROGRAM.with(Cell::get)
}

/// The program the calling thread runs, until this is dropped, when the
/// thread runs the one it ran before ([`Store::enter`]).
#[must_use = "the thread runs the program only while this lives"]
pub(super) struct Program {
    before: Tag,
}

impl Drop for Program {
    fn drop(&mut self) {
        PROGRAM.with(|program| program.set(self.before));
    }
}

/// Objects the store let go of, for the caller to drop once no lock is
/// held: releasing an application's object can call into its process.
#[must_use = "drop once no lock is held"]
#[derive(Default)]
pub(super) struct Removed {
    uia: Vec<verbatim_uia::Released>,
    msaa: Vec<verbatim_ia2::Released>,
    patterns: Vec<Patterns>,
    uia_text: Option<Forgotten<UiaPos>>,
    edit_text: Option<Forgotten<u32>>,
    terminals: Vec<Terminal>,
}

impl Removed {
    fn uia_text(&mut self, forgotten: Forgotten<UiaPos>) {
        match &mut self.uia_text {
            Some(text) => text.append(forgotten),
            None => self.uia_text = Some(forgotten),
        }
    }

    fn edit_text(&mut self, forgotten: Forgotten<u32>) {
        match &mut self.edit_text {
            Some(text) => text.append(forgotten),
            None => self.edit_text = Some(forgotten),
        }
    }
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
    /// The anchors minted in UIA text and in edit controls; both number
    /// theirs from one counter.
    uia_text: Anchors<UiaPos>,
    edit_text: Anchors<u32>,
    /// Each UIA node's text patterns, once fetched.
    patterns: HashMap<u64, Patterns>,
    /// What each UIA node's text is known to support of the text
    /// attributes, learned from its answers.
    support: HashMap<u64, TextSupport>,
    /// What the outpost keeps of each focused terminal between its reads,
    /// by node (`docs/design/terminal-model.md`, "Where it runs"). A read
    /// works on a copy, which only the commit of its result puts in place.
    terminals: HashMap<u64, Terminal>,
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

    fn tag_anchors(&mut self, anchors: &[u64], tag: Tag) {
        for &anchor in anchors {
            add(self.anchors.entry(anchor).or_default(), tag);
        }
    }

    /// Forgets everything kept for the nodes `gone`: their text, terminal
    /// and tags, and the tags of their anchors, into `removed`.
    fn forget_nodes(&mut self, gone: &HashSet<u64>, removed: &mut Removed) {
        for &node in gone {
            self.nodes.remove(&node);
            let anchors: Vec<u64> = self
                .uia_text
                .anchors_of(node)
                .chain(self.edit_text.anchors_of(node))
                .collect();
            for anchor in anchors {
                self.anchors.remove(&anchor);
            }
            removed.uia_text(self.uia_text.forget_node(node));
            removed.edit_text(self.edit_text.forget_node(node));
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
    /// The number of the last request tagged.
    requests: AtomicU64,
    state: Mutex<State>,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    /// An empty store: both registries mint from one counter, and both
    /// anchor stores number from another.
    pub(super) fn new() -> Self {
        let ids = Arc::new(AtomicU64::new(1));
        let anchors = Arc::new(AtomicU64::new(0));
        Self {
            uia: UiaRegistry::new(Arc::clone(&ids)),
            msaa: MsaaRegistry::new(ids),
            requests: AtomicU64::new(0),
            state: Mutex::new(State {
                nodes: HashMap::new(),
                anchors: HashMap::new(),
                uia_text: Anchors::new(Arc::clone(&anchors)),
                edit_text: Anchors::new(anchors),
                patterns: HashMap::new(),
                support: HashMap::new(),
                terminals: HashMap::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The tag of a new request of worker generation `generation`.
    pub(super) fn next_tag(&self, generation: u64) -> Tag {
        Tag {
            generation,
            request: self.requests.fetch_add(1, Ordering::Relaxed) + 1,
        }
    }

    /// Runs `tag`'s program on the calling thread until the returned value
    /// is dropped: every node issued or looked up through
    /// [`nodes`](Self::nodes) on this thread meanwhile is tagged with it.
    #[expect(
        clippy::unused_self,
        reason = "a program is entered through the store whose nodes it tags"
    )]
    pub(super) fn enter(&self, tag: Tag) -> Program {
        let before = PROGRAM.with(|program| program.replace(tag));
        Program { before }
    }

    /// The store's handle for the program the calling thread runs, through
    /// which a walk issues and looks up nodes, each tagged with it.
    pub(super) fn nodes(&self) -> Nodes<'_> {
        Nodes {
            store: self,
            tag: current(),
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
    /// tagged with the program the calling thread runs, in the same hold,
    /// so no release frees it while the program may name it.
    pub(super) fn held_prefix(&self, chain: &[NodeId]) -> usize {
        let tag = current();
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

    /// Tags each of `nodes` the store holds with the program the calling
    /// thread runs: the lines it copied, which it may name.
    pub(super) fn tag_copied(&self, nodes: impl IntoIterator<Item = NodeId>) {
        let tag = current();
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
            for anchor in &anchors {
                state.anchors.remove(anchor);
            }
            let forgotten = state.uia_text.forget_anchors(&anchors);
            removed.uia_text(forgotten);
            let forgotten = state.edit_text.forget_anchors(&anchors);
            removed.edit_text(forgotten);
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
    /// them.
    pub(super) fn forget_window(&self, hwnd: isize) -> Removed {
        let mut state = self.lock();
        let (gone, released) = self.msaa.forget_window(hwnd);
        let gone: HashSet<u64> = gone.iter().map(|node| node.number()).collect();
        let mut removed = Removed::default();
        removed.msaa.push(released);
        state.forget_nodes(&gone, &mut removed);
        removed
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
        state.nodes.clear();
        state.anchors.clear();
        (nodes.len(), removed)
    }

    // Text.

    /// The text patterns kept for `node`: `None` when none were fetched,
    /// `Some(None)` when its element answered it has none. Clones.
    pub(super) fn patterns(&self, node: NodeId) -> Option<Patterns> {
        self.lock().patterns.get(&node.number()).cloned()
    }

    /// Keeps `patterns` as `node`'s, while the store holds it; returns
    /// what it replaced, or `patterns` themselves for a node released.
    pub(super) fn keep_patterns(&self, node: NodeId, patterns: Patterns) -> Removed {
        let mut removed = Removed::default();
        let mut state = self.lock();
        if self.holds(node) {
            removed
                .patterns
                .extend(state.patterns.insert(node.number(), patterns));
        } else {
            removed.patterns.push(patterns);
        }
        removed
    }

    /// Forgets that `node`'s element answered it has no text pattern;
    /// answers whether it had.
    pub(super) fn forget_no_text(&self, node: NodeId) -> bool {
        let mut state = self.lock();
        let forgotten = state
            .patterns
            .get(&node.number())
            .is_some_and(Option::is_none);
        if forgotten {
            state.patterns.remove(&node.number());
        }
        forgotten
    }

    /// Forgets `node`'s text: its patterns, what it supports, and its
    /// anchors in both backends, for a node whose runtime id names a new
    /// element.
    pub(super) fn forget_text(&self, node: NodeId) -> Removed {
        let mut removed = Removed::default();
        let mut state = self.lock();
        let number = node.number();
        removed.patterns.extend(state.patterns.remove(&number));
        state.support.remove(&number);
        let forgotten = state.uia_text.forget_node(number);
        removed.uia_text(forgotten);
        let forgotten = state.edit_text.forget_node(number);
        removed.edit_text(forgotten);
        removed
    }

    /// What `node`'s text is known to support.
    pub(super) fn support(&self, node: NodeId) -> TextSupport {
        self.lock()
            .support
            .get(&node.number())
            .copied()
            .unwrap_or_default()
    }

    /// Keeps what `node`'s text is now known to support, while the store
    /// holds it.
    pub(super) fn keep_support(&self, node: NodeId, support: TextSupport) {
        let mut state = self.lock();
        if self.holds(node) {
            state.support.insert(node.number(), support);
        }
    }

    /// A copy of `node`'s UIA text anchors, for a text call made with no
    /// lock held.
    pub(super) fn uia_text(&self, node: NodeId) -> NodeTextCopy<UiaPos> {
        self.lock().uia_text.copy(node.number())
    }

    /// A copy of `node`'s edit control anchors.
    pub(super) fn edit_text(&self, node: NodeId) -> NodeTextCopy<u32> {
        self.lock().edit_text.copy(node.number())
    }

    /// Puts `copy` back as `node`'s UIA text anchors, while the store holds
    /// the node, its anchors tagged with the program the calling thread
    /// runs. The caller has judged that program live.
    pub(super) fn put_uia_text(&self, node: NodeId, copy: NodeTextCopy<UiaPos>) -> Removed {
        let tag = current();
        let mut removed = Removed::default();
        let mut state = self.lock();
        if self.holds(node) {
            state.tag_anchors(copy.touched(), tag);
            let replaced = state.uia_text.put(node.number(), copy);
            removed.uia_text(replaced);
            drop(state);
        } else {
            drop(state);
            drop(copy);
        }
        removed
    }

    /// Puts `copy` back as `node`'s edit control anchors, as
    /// [`put_uia_text`](Self::put_uia_text) does.
    pub(super) fn put_edit_text(&self, node: NodeId, copy: NodeTextCopy<u32>) -> Removed {
        let tag = current();
        let mut removed = Removed::default();
        let mut state = self.lock();
        if self.holds(node) {
            state.tag_anchors(copy.touched(), tag);
            let replaced = state.edit_text.put(node.number(), copy);
            removed.edit_text(replaced);
        }
        removed
    }

    /// Forgets the selection `node`'s next caret report through `uia` or
    /// not would be compared with.
    pub(super) fn forget_selection(&self, node: NodeId, uia: bool) -> Removed {
        let mut removed = Removed::default();
        let mut state = self.lock();
        if uia {
            let forgotten = state.uia_text.forget_selection(node.number());
            removed.uia_text(forgotten);
        } else {
            let forgotten = state.edit_text.forget_selection(node.number());
            removed.edit_text(forgotten);
        }
        removed
    }

    /// Notes that edit control `node`'s text changed.
    pub(super) fn note_content_changed(&self, node: NodeId) {
        let mut state = self.lock();
        if self.holds(node) {
            state.edit_text.note_content_changed(node.number());
        }
    }

    /// The UIA position `position` in `node`'s text was when the outpost
    /// reported it, a clone, with no call.
    pub(super) fn reported_point(&self, node: NodeId, position: TextPosition) -> Option<UiaPos> {
        self.lock().uia_text.reported_point(node.number(), position)
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

/// The store's handle for one program: every node issued or looked up
/// through it is tagged with the program's tag, in the same hold of the
/// store's lock as the registry's change.
pub(super) struct Nodes<'a> {
    store: &'a Store,
    tag: Tag,
}

impl Nodes<'_> {
    /// The kept UIA node for `runtime_id`, if there is one, looked up.
    pub(super) fn existing(&self, runtime_id: &[i32]) -> Option<NodeId> {
        let mut state = self.store.lock();
        let node = self.store.uia.existing_id(runtime_id)?;
        state.tag_node(node, self.tag);
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
        state.tag_node(node, self.tag);
        node
    }

    /// The UIA node for `runtime_id`, issued or looked up, keeping
    /// `element` as its live element: an agile reference made already.
    pub(super) fn keep(&self, runtime_id: &[i32], element: Option<Agile<Kept>>) -> NodeId {
        let (node, replaced) = {
            let mut state = self.store.lock();
            let (node, replaced) = self.store.uia.keep(runtime_id, element);
            state.tag_node(node, self.tag);
            (node, replaced)
        };
        drop(replaced);
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
            state.tag_node(*node, self.tag);
        }
        found
    }

    fn touch(&self, id: NodeId) {
        let mut state = self.store.lock();
        if self.store.msaa.holds(id) {
            state.tag_node(id, self.tag);
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
        state.tag_node(node, self.tag);
        node
    }

    fn facts_of(&self, id: NodeId) -> Option<verbatim_ia2::Kept> {
        self.store.msaa.facts_of(id)
    }

    fn reissue(&self, id: NodeId, name: Option<String>) {
        let mut state = self.store.lock();
        if self.store.msaa.holds(id) {
            self.store.msaa.reissue(id, name);
            state.tag_node(id, self.tag);
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
