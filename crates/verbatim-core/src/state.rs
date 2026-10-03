//! Reducer state (architecture section 2).
//!
//! [`SrState`] is the state threaded through [`crate::reduce`]: the focus,
//! the attention record, the navigator, and the one object-navigation query
//! still in flight. It is cheap to clone; the reducer never mutates a
//! caller's state in place, it produces a new one.

use std::collections::{BTreeMap, BTreeSet};

use verbatim_model::{NodeId, NodeSnapshot, OutpostId, Pid, QueryId, WindowFacts};

/// The focused node and what the reducer knows about where it sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FocusContext {
    /// The application the focus belongs to.
    pub(crate) source: Pid,
    /// The focus event's window facts, when it carried any. The top-level
    /// window is what a foreground change is compared against.
    pub(crate) window: Option<WindowFacts>,
    /// The focused node as last announced or updated.
    pub(crate) snapshot: NodeSnapshot,
    /// The focused node's ancestors, outermost first, as the `FocusChanged`
    /// event carried them. The next focus change diffs its own ancestry
    /// against these and the focus itself to announce only newly entered
    /// containers (NVDA's focus-ancestry behavior); empty when the outpost's
    /// walk found nothing or timed out.
    pub(crate) ancestors: Vec<NodeSnapshot>,
    /// The most recently announced selected item within the focused
    /// container — seeded by the focus event's own `selected_child`, then
    /// advanced by each announced `SelectionChanged` — so a selection event
    /// for the item that was just spoken is not spoken twice.
    pub(crate) last_selection: Option<NodeId>,
    /// False once the outpost that issued these node ids has ended. The
    /// copied data stays, so a replacement outpost's report of the same
    /// focus can be taken silently (`docs/parity.md`, "Recovery after an
    /// outpost is replaced"), but the ids name nothing any more.
    pub(crate) alive: bool,
}

/// The application and window of the most recent foreground change (decision
/// D14 as amended by the outpost redesign): the reducer's stand-in for the
/// system's foreground window, which is what NVDA classifies events against.
/// Every event other than a foreground change is classified against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Attention {
    pub(crate) source: Pid,
    pub(crate) window: Option<WindowFacts>,
}

/// The navigator object and its review cursor (roadmap M3): the object
/// object-navigation commands walk, independent of keyboard focus. It
/// follows focus by default (every focus change resets it), and the
/// "to focus" command snaps it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Navigator {
    pub(crate) object: NodeSnapshot,
    /// The review cursor's character offset into the object's review text
    /// (see `review::text_of`). Always a valid boundary within that text.
    pub(crate) review_offset: usize,
}

/// The most recently issued object-navigation query whose completion has not
/// landed yet, and the node it navigates from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingNavigation {
    pub(crate) query_id: QueryId,
    pub(crate) from: NodeId,
}

/// Reducer state: focus, attention, navigator, and the latest navigation.
#[derive(Clone, Debug, Default)]
pub struct SrState {
    pub(crate) focus: Option<FocusContext>,
    pub(crate) attention: Option<Attention>,
    pub(crate) next_query_id: u64,
    /// The navigator object and review cursor (roadmap M3). `None` until
    /// the first focus lands; from then it tracks focus unless an
    /// object-navigation command moves it away.
    pub(crate) navigator: Option<Navigator>,
    /// The most recently issued object-navigation query, if its completion
    /// has not landed yet.
    ///
    /// A completion is applied only when it answers this query: a later
    /// navigation command supersedes an earlier still-pending one, so a
    /// completion arriving after it is dropped rather than clobbering where
    /// the user has since moved. A `FocusChanged` event snaps the navigator
    /// to the new focus (review follows focus) but deliberately leaves this
    /// field alone — an app-initiated focus event must not be able to
    /// discard the user's own, more recent, in-flight navigation. `ToFocus`
    /// clears it explicitly, since it is itself the user's explicit, newer
    /// intent superseding whatever navigation was pending; so does the end of
    /// the outpost it was sent to.
    pub(crate) latest_navigation: Option<PendingNavigation>,
    /// The outpost and observation time of the newest focus event applied.
    /// Each outpost keeps its own events in order, but two outposts can
    /// deliver theirs out of the order they were observed in, where NVDA
    /// handles every event in one queue; a focus event from another outpost
    /// observed before this one is stale and dropped.
    pub(crate) latest_focus: Option<(OutpostId, u64)>,
}

impl SrState {
    /// An initial state with no focus, no attention, and nothing pending.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently focused node and the application it came from, if
    /// anything is focused and its outpost is still running.
    #[must_use]
    pub fn focused(&self) -> Option<(Pid, &NodeSnapshot)> {
        self.focus
            .as_ref()
            .filter(|focus| focus.alive)
            .map(|focus| (focus.source, &focus.snapshot))
    }

    /// The application holding attention, if any foreground change has been
    /// seen yet. The shell derives the supervisor's view of attention from this.
    #[must_use]
    pub fn attention(&self) -> Option<Pid> {
        self.attention.map(|attention| attention.source)
    }

    /// The application the focus belongs to; `None` before any focus.
    #[must_use]
    pub fn focus_source(&self) -> Option<Pid> {
        self.focus.as_ref().map(|focus| focus.source)
    }

    /// When the newest focus the reducer applied was observed, in
    /// milliseconds since the Unix epoch; `None` before any.
    #[must_use]
    pub fn latest_focus_observed_at(&self) -> Option<u64> {
        self.latest_focus.map(|(_, observed_at_ms)| observed_at_ms)
    }

    /// Every node the state refers to, grouped by the outpost that issued
    /// it: the focus, its ancestors, its last announced selection, the
    /// navigator, and the node the latest navigation starts from. The shell
    /// sends each outpost its own set, so the outpost keeps exactly these
    /// nodes' live objects. A dead focus contributes nothing.
    #[must_use]
    pub fn held_nodes(&self) -> BTreeMap<OutpostId, BTreeSet<NodeId>> {
        let mut held: BTreeMap<OutpostId, BTreeSet<NodeId>> = BTreeMap::new();
        let mut insert = |id: NodeId| {
            held.entry(id.outpost()).or_default().insert(id);
        };
        if let Some(focus) = self.focus.as_ref().filter(|focus| focus.alive) {
            insert(focus.snapshot.id);
            for ancestor in &focus.ancestors {
                insert(ancestor.id);
            }
            if let Some(selected) = focus.last_selection {
                insert(selected);
            }
        }
        if let Some(navigator) = &self.navigator {
            insert(navigator.object.id);
        }
        if let Some(pending) = self.latest_navigation {
            insert(pending.from);
        }
        held
    }

    /// Whether the focused node is exactly `node_id` and still alive.
    pub(crate) fn focus_matches(&self, node_id: NodeId) -> bool {
        self.focus
            .as_ref()
            .is_some_and(|focus| focus.alive && focus.snapshot.id == node_id)
    }

    /// Allocates a fresh, process-unique-within-this-state `QueryId`.
    pub(crate) fn allocate_query_id(&mut self) -> QueryId {
        let id = self.next_query_id;
        self.next_query_id += 1;
        QueryId(id)
    }
}
