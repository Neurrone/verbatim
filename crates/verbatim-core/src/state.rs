//! Reducer state (architecture section 2).
//!
//! [`SrState`] is the state threaded through [`crate::reduce`]: the
//! currently focused node (if any), how recently each source application's
//! event stream has been observed, and any staleness re-fetches still in
//! flight. It is cheap to clone; the reducer never mutates a caller's state
//! in place, it produces a new one.

use std::collections::HashMap;

use verbatim_model::{NodeId, NodeSnapshot, Pid, QueryId, QueryKind, SnapshotVersion};

/// What the focused node looked like the last time the reducer actually
/// spoke about it.
///
/// Kept separate from the live snapshot: a silent update (a name change on
/// the focused node produces no announcement in M1) must not mask a real
/// content change once a staleness re-fetch comes back and the reducer has
/// to decide whether anything worth announcing actually changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FocusContext {
    pub(crate) source: Pid,
    /// Milliseconds since the Unix epoch when the OS event behind this focus
    /// was observed, carried from the `FocusChanged` input. A later
    /// `FocusChanged` from the same source whose own observation is strictly
    /// earlier than this is dropped (last-observation-wins), so a window and a
    /// control announcement racing on two outpost threads cannot leave the
    /// reducer on the earlier-observed one. Zero when the input carried no
    /// timestamp (an older flight-recorder stream).
    pub(crate) observed_at_ms: u64,
    pub(crate) snapshot: NodeSnapshot,
    pub(crate) last_announced: NodeSnapshot,
    /// The focused node's ancestors, outermost first, as the `FocusChanged`
    /// event carried them. The next focus change diffs its own ancestry
    /// against this to announce only newly entered containers (NVDA's
    /// focus-ancestry behavior); empty when the outpost's walk found
    /// nothing or timed out.
    pub(crate) ancestors: Vec<NodeSnapshot>,
    /// The most recently announced selected item within the focused
    /// container — seeded by the focus event's own `selected_child`, then
    /// advanced by each announced `SelectionChanged` — so a selection event
    /// for the item that was just spoken is not spoken twice.
    pub(crate) last_selection: Option<NodeId>,
}

/// Why the reducer asked an outpost to re-read a node.
///
/// The type keeps the pending-fetch table self-describing as milestones add
/// more reasons (browse-mode expansion, and so on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FetchReason {
    /// An event arrived with a version older than the last one seen for its
    /// source, so the reducer distrusts the data it carried and asked for a
    /// fresh read instead of announcing it.
    Staleness,
    /// An object-navigation command asked the outpost for the navigator
    /// object's neighbor in some direction; the completion moves the
    /// navigator there and announces it (roadmap M3).
    Navigate,
}

/// The navigator object and its review cursor (roadmap M3): the object
/// object-navigation commands walk, independent of keyboard focus. It
/// follows focus by default (every focus change resets it), and the
/// "to focus" command snaps it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Navigator {
    pub(crate) source: Pid,
    pub(crate) object: NodeSnapshot,
    /// The review cursor's character offset into the object's review text
    /// (see `review::text_of`). Always a valid boundary within that text.
    pub(crate) review_offset: usize,
}

/// One outstanding fetch the reducer is waiting on: which node it asked
/// about, why, and what kind of query it was — a navigation completion
/// needs the kind back to speak the right edge message ("No next" for a
/// sibling move, "No containing object" for a parent move) when there is
/// no neighbor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingFetch {
    pub(crate) source: Pid,
    pub(crate) node_id: NodeId,
    pub(crate) reason: FetchReason,
    pub(crate) kind: QueryKind,
}

/// Reducer state: focus context, per-source staleness tracking, and
/// in-flight fetches.
#[derive(Clone, Debug, Default)]
pub struct SrState {
    pub(crate) focus: Option<FocusContext>,
    pub(crate) versions: HashMap<Pid, SnapshotVersion>,
    pub(crate) next_query_id: u64,
    pub(crate) pending_fetches: HashMap<QueryId, PendingFetch>,
    /// The navigator object and review cursor (roadmap M3). `None` until
    /// the first focus lands; from then it tracks focus unless an
    /// object-navigation command moves it away.
    pub(crate) navigator: Option<Navigator>,
    /// The `QueryId` of the most recently issued object-navigation fetch, if
    /// its completion has not landed yet.
    ///
    /// A completion is applied only when it is this query: a later
    /// navigation command supersedes an earlier still-pending one, so a
    /// stale completion arriving after it is dropped rather than clobbering
    /// where the user has since moved. A `FocusChanged` event snaps the
    /// navigator to the new focus (review follows focus) but deliberately
    /// leaves this field alone — an app-initiated focus event must not be
    /// able to discard the user's own, more recent, in-flight navigation.
    /// `ToFocus` clears it explicitly, since it is itself the user's
    /// explicit, newer intent superseding whatever navigation was pending.
    pub(crate) latest_navigation: Option<QueryId>,
}

impl SrState {
    /// An initial state with no focus, no version history, and no pending
    /// fetches.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently focused node and the application it came from, if
    /// anything is focused.
    #[must_use]
    pub fn focused(&self) -> Option<(Pid, &NodeSnapshot)> {
        self.focus.as_ref().map(|f| (f.source, &f.snapshot))
    }

    /// The most recent event version observed from `source`, if any event
    /// has been seen from it yet.
    #[must_use]
    pub fn last_seen_version(&self, source: Pid) -> Option<SnapshotVersion> {
        self.versions.get(&source).copied()
    }

    /// Number of fetches the reducer is currently waiting on.
    #[must_use]
    pub fn pending_fetch_count(&self) -> usize {
        self.pending_fetches.len()
    }

    /// Whether `version` is older than the last version observed from
    /// `source` — the out-of-order-delivery case the reducer must not
    /// announce data for.
    pub(crate) fn is_stale(&self, source: Pid, version: SnapshotVersion) -> bool {
        self.versions
            .get(&source)
            .is_some_and(|&last| version < last)
    }

    /// Records `version` as the most recent one seen from `source`. Callers
    /// only invoke this once [`SrState::is_stale`] has ruled out
    /// out-of-order delivery.
    pub(crate) fn record_version(&mut self, source: Pid, version: SnapshotVersion) {
        self.versions.insert(source, version);
    }

    /// Whether the focused node is exactly `(source, node_id)`.
    pub(crate) fn focus_matches(&self, source: Pid, node_id: NodeId) -> bool {
        self.focus
            .as_ref()
            .is_some_and(|f| f.source == source && f.snapshot.id == node_id)
    }

    /// Allocates a fresh, process-unique-within-this-state `QueryId`.
    pub(crate) fn allocate_query_id(&mut self) -> QueryId {
        let id = self.next_query_id;
        self.next_query_id += 1;
        QueryId(id)
    }
}
