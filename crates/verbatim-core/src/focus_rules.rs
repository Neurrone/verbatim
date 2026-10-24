//! The focus rules the reducer shares with the focus listener and the
//! outposts, each defined once (decision D14, "Where the rules run"): how a
//! window stands to the foreground window, which focus repeats the focus
//! Core holds, which property changes and selections are spoken, and which
//! containers a focus newly enters. The rules ported from NVDA that only the
//! listener and the outposts make, the acceptance test and NVDA's
//! foreground tests, are defined once in `verbatim-outpost`, which both
//! share, since ported code stays in the Windows-specific crates. The reducer applies them after an event is
//! read; the focus listener and the outposts apply the same definitions
//! where a judgment must come before an element is read, because a UI
//! Automation element cannot pass to another process. Those judgments use
//! the focus Core holds, which Core sends them whenever it changes
//! ([`HeldFocus`]), never a copy of the focus they last reported.
//!
//! The behaviour is specified in `docs/parity/focus-events.md` ("Event
//! acceptance", "Duplicate focus suppression"), from which these are
//! written,
//! `docs/parity/changes-and-selection.md` and
//! `docs/parity/focus-context.md`.

use serde::{Deserialize, Serialize};
use verbatim_model::{Backend, NodeId, Role, StateSet, TraceId, WindowFacts, WindowHandle};

/// How a window stands to the system's foreground window, the facts the
/// acceptance test ends with (`docs/parity/focus-events.md`, "Event
/// acceptance"): read before an event is read, and again by an outpost
/// when it reports the event ([`WindowFacts`]), so the reducer judges a
/// focus by the same rule after the read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForegroundRelation {
    /// The window is in the foreground window: it shares the foreground
    /// window's root owner, is the foreground window or inside it, or its
    /// root owner is.
    pub in_foreground: bool,
    /// The window or its root is topmost: menus, combo box drop-downs, the
    /// task switching list.
    pub topmost: bool,
    /// For a `Windows.UI.Core` window only, whether it is the input
    /// thread's active window or inside it; `None` for any other class.
    pub under_active_window: Option<bool>,
}

impl ForegroundRelation {
    /// The relation an event's window facts record.
    #[must_use]
    pub fn of_facts(facts: &WindowFacts) -> Self {
        Self {
            in_foreground: facts.in_foreground,
            topmost: facts.topmost,
            under_active_window: facts.under_active_window,
        }
    }

    /// Whether the window is related to the foreground window: in it,
    /// topmost, or a `Windows.UI.Core` window under the active window.
    #[must_use]
    pub fn related(&self) -> bool {
        self.in_foreground || self.topmost || self.under_active_window == Some(true)
    }
}

/// One node of the focus Core holds, as the rules compare it: the focus or
/// one of its ancestors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldNode {
    /// The node.
    pub id: NodeId,
    /// The backend that read it.
    pub backend: Backend,
    /// Its role.
    pub role: Role,
    /// Its name, as last known.
    pub name: Option<String>,
    /// Its states, as last known.
    pub states: StateSet,
}

/// The focus Core holds, which Core sends the focus listener and the
/// outposts concerned whenever it changes, for the judgments they make
/// before an element is read (decision D14, "Where the rules run").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldFocus {
    /// The trace of the event Core took the focus from: the focus
    /// listener's own trace for a UIA focus it read.
    pub trace_id: TraceId,
    /// When that event was observed, in milliseconds and microseconds on
    /// the run's shared clock (0 when not known).
    pub observed: (u64, u64),
    /// The focus.
    pub focus: HeldNode,
    /// Its ancestors, outermost first.
    pub ancestors: Vec<HeldNode>,
    /// Its top-level window, when known.
    pub top_level: Option<WindowHandle>,
    /// False once the outpost that issued its node ids has ended.
    pub alive: bool,
}

impl HeldFocus {
    /// This focus as the outpost `outpost` knows its own nodes: their ids
    /// unstamped, as it issued them, and every node another outpost issued
    /// left out; `None` when the focus itself is another outpost's.
    #[must_use]
    pub fn for_outpost(&self, outpost: verbatim_model::OutpostId) -> Option<Self> {
        let own = |node: &HeldNode| {
            (node.id.outpost() == outpost).then(|| HeldNode {
                id: node.id.unstamped(),
                ..node.clone()
            })
        };
        Some(Self {
            focus: own(&self.focus)?,
            ancestors: self.ancestors.iter().filter_map(own).collect(),
            ..self.clone()
        })
    }

    /// The node `id`, the focus or an ancestor, when it is one.
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&HeldNode> {
        std::iter::once(&self.focus)
            .chain(&self.ancestors)
            .find(|node| node.id == id)
    }
}

/// What a judge knows of one focus event against the focus Core holds
/// (`docs/parity/focus-events.md`, "Duplicate focus suppression").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatEvidence {
    /// Core has judged every focus the judge sent it, so the focus it holds
    /// is the focus it will compare the event with. Core itself always has.
    pub settled: bool,
    /// The event names the element of the focus Core holds, by the judge's
    /// evidence of identity: the node id in Core, UI Automation's
    /// comparison in the focus listener, the event's address in an outpost.
    pub same_element: bool,
    /// That element still has the focus: the node still alive in Core, its
    /// keyboard focus read live in the focus listener, its address a whole
    /// object rather than a child id that can be reused in an outpost.
    pub still_focused: bool,
}

/// Whether a focus event repeats the focus Core holds, and is dropped
/// before anything else judges it.
#[must_use]
pub fn repeats_focus(evidence: RepeatEvidence) -> bool {
    evidence.settled && evidence.same_element && evidence.still_focused
}

/// A property change, by what it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// The name.
    Name,
    /// The description.
    Description,
    /// The value.
    Value,
    /// The states.
    States,
}

/// What a spoken property change concerns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeTarget {
    /// The focus.
    Focus,
    /// One of its ancestors.
    Ancestor,
}

/// What a change of `kind` on `node` concerns, when it is spoken
/// (`docs/parity/changes-and-selection.md`): a name, description, or value
/// change only on the focus, `focus`, and a state change on the focus or on
/// one of its `ancestors` whose state changes are spoken
/// (`speaks_states`): an outpost knows of two whose are not, an ancestor
/// reached through a tree view item's logical parents and the foreground
/// window's own object. `None` for a change that is not spoken.
pub fn change_target(
    kind: ChangeKind,
    node: NodeId,
    focus: NodeId,
    ancestors: impl IntoIterator<Item = NodeId>,
    speaks_states: impl Fn(NodeId) -> bool,
) -> Option<ChangeTarget> {
    if node == focus {
        return Some(ChangeTarget::Focus);
    }
    (kind == ChangeKind::States
        && ancestors
            .into_iter()
            .any(|ancestor| ancestor == node && speaks_states(ancestor)))
    .then_some(ChangeTarget::Ancestor)
}

/// The nodes a change of `kind` can be spoken for, given the focus and its
/// ancestors: the focus, and for a state change the ancestors whose state
/// changes are spoken, by [`change_target`]. An outpost tells the event's
/// object from these by its identity before it reads any property of it.
pub fn change_candidates(
    kind: ChangeKind,
    focus: NodeId,
    ancestors: &[NodeId],
    speaks_states: impl Fn(NodeId) -> bool,
) -> Vec<NodeId> {
    ancestors
        .iter()
        .copied()
        .chain(std::iter::once(focus))
        .filter(|&node| {
            change_target(kind, node, focus, ancestors.iter().copied(), &speaks_states).is_some()
        })
        .collect()
}

/// What a selection of a node concerns, when it is spoken
/// (`docs/parity/changes-and-selection.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionTarget {
    /// The focus itself: a change of its states.
    FocusStates,
    /// One of the focus's ancestors: a change of its states.
    AncestorStates,
    /// An item inside the focus, a selection container, of the same
    /// application.
    Item,
}

/// Whether a focus of `role` is a selection container: its selection
/// events are spoken (NVDA's generic selection behaviour: arrowing through
/// a list whose focus stays on the container speaks each newly selected
/// item). A combo box is not one: its selection reaches it as a value
/// change already.
#[must_use]
pub fn is_selection_container(role: Role) -> bool {
    matches!(role, Role::List | Role::TabControl)
}

/// What a selection of `node` concerns, given the focus `focus` of
/// `focus_role` and its `ancestors`: the focus's or an ancestor's states
/// (an ancestor `selects_states` refuses, as an outpost refuses a tree view
/// item's logical parent, is not spoken at all), or an item when the
/// focus is a selection container of the same outpost; `None` when it is
/// not spoken.
pub fn selection_target(
    node: NodeId,
    (focus, focus_role): (NodeId, Role),
    ancestors: impl IntoIterator<Item = NodeId>,
    selects_states: impl Fn(NodeId) -> bool,
) -> Option<SelectionTarget> {
    if node == focus {
        return Some(SelectionTarget::FocusStates);
    }
    if ancestors.into_iter().any(|ancestor| ancestor == node) {
        return selects_states(node).then_some(SelectionTarget::AncestorStates);
    }
    (focus.outpost() == node.outpost() && is_selection_container(focus_role))
        .then_some(SelectionTarget::Item)
}

/// A node as [`already_entered`] compares it.
pub trait EnteredKey {
    /// Its id.
    fn key_id(&self) -> NodeId;
    /// The backend that read it.
    fn key_backend(&self) -> Backend;
    /// Its role.
    fn key_role(&self) -> Role;
    /// Its name.
    fn key_name(&self) -> Option<&str>;
}

impl EnteredKey for HeldNode {
    fn key_id(&self) -> NodeId {
        self.id
    }
    fn key_backend(&self) -> Backend {
        self.backend
    }
    fn key_role(&self) -> Role {
        self.role
    }
    fn key_name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl EnteredKey for verbatim_model::NodeSnapshot {
    fn key_id(&self) -> NodeId {
        self.id
    }
    fn key_backend(&self) -> Backend {
        self.backend
    }
    fn key_role(&self) -> Role {
        self.role
    }
    fn key_name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

/// Whether the focus had already entered `node`, a container of a new
/// focus, given the previous focus and its ancestors, `previous`
/// (`docs/parity/focus-context.md`, "Focus-ancestry context"): `node` is
/// one of them; or, when the new focus is in the same top-level window as
/// the previous one (`same_window`), one of them has `node`'s role and
/// name and is not the same element under another node, which only a UIA
/// node of the same outpost can be. A newly entered container is spoken,
/// and a newly entered dialog gathers its own text.
pub fn already_entered<'a, P: EnteredKey + 'a>(
    previous: impl IntoIterator<Item = &'a P>,
    same_window: bool,
    node: &impl EnteredKey,
) -> bool {
    previous.into_iter().any(|old| {
        let one_identity = old.key_id().outpost() == node.key_id().outpost()
            && old.key_backend() == Backend::Uia
            && node.key_backend() == Backend::Uia;
        old.key_id() == node.key_id()
            || (same_window
                && !one_identity
                && old.key_role() == node.key_role()
                && old.key_name() == node.key_name())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::OutpostId;

    #[test]
    fn a_repeat_needs_every_piece_of_evidence() {
        let all = RepeatEvidence {
            settled: true,
            same_element: true,
            still_focused: true,
        };
        assert!(repeats_focus(all));
        assert!(!repeats_focus(RepeatEvidence {
            settled: false,
            ..all
        }));
        assert!(!repeats_focus(RepeatEvidence {
            same_element: false,
            ..all
        }));
        assert!(!repeats_focus(RepeatEvidence {
            still_focused: false,
            ..all
        }));
    }

    #[test]
    fn a_name_change_is_spoken_on_the_focus_alone_and_a_state_change_on_ancestors_too() {
        let (focus, parent, other) = (NodeId::new(1), NodeId::new(2), NodeId::new(3));
        let speaks = |_| true;
        assert_eq!(
            change_target(ChangeKind::Name, focus, focus, [parent], speaks),
            Some(ChangeTarget::Focus)
        );
        assert_eq!(
            change_target(ChangeKind::Name, parent, focus, [parent], speaks),
            None
        );
        assert_eq!(
            change_target(ChangeKind::States, parent, focus, [parent], speaks),
            Some(ChangeTarget::Ancestor)
        );
        assert_eq!(
            change_target(ChangeKind::States, parent, focus, [parent], |_| false),
            None,
            "an ancestor whose state changes are not spoken"
        );
        assert_eq!(
            change_target(ChangeKind::States, other, focus, [parent], speaks),
            None
        );
        assert_eq!(
            change_candidates(ChangeKind::States, focus, &[parent], speaks),
            vec![parent, focus]
        );
        assert_eq!(
            change_candidates(ChangeKind::Value, focus, &[parent], speaks),
            vec![focus]
        );
    }

    #[test]
    fn a_selection_is_spoken_on_the_focus_its_ancestors_or_inside_a_focused_list() {
        let focus = NodeId::new(1).with_outpost(OutpostId(2));
        let parent = NodeId::new(2).with_outpost(OutpostId(2));
        let item = NodeId::new(3).with_outpost(OutpostId(2));
        let elsewhere = NodeId::new(3).with_outpost(OutpostId(4));
        let all = |_| true;
        assert_eq!(
            selection_target(focus, (focus, Role::Button), [parent], all),
            Some(SelectionTarget::FocusStates)
        );
        assert_eq!(
            selection_target(parent, (focus, Role::Button), [parent], all),
            Some(SelectionTarget::AncestorStates)
        );
        assert_eq!(
            selection_target(parent, (focus, Role::List), [parent], |_| false),
            None,
            "an ancestor refused is not spoken, even in a list"
        );
        assert_eq!(
            selection_target(item, (focus, Role::Button), [parent], all),
            None
        );
        assert_eq!(
            selection_target(item, (focus, Role::List), [parent], all),
            Some(SelectionTarget::Item)
        );
        assert_eq!(
            selection_target(elsewhere, (focus, Role::List), [parent], all),
            None,
            "another application's item"
        );
    }

    fn held(id: u64, backend: Backend, role: Role, name: &str) -> HeldNode {
        HeldNode {
            id: NodeId::new(id).with_outpost(OutpostId(1)),
            backend,
            role,
            name: Some(name.to_owned()),
            states: StateSet::default(),
        }
    }

    #[test]
    fn a_container_is_entered_unless_the_previous_focus_was_in_it() {
        let dialog = held(1, Backend::Msaa, Role::Dialog, "Remove");
        let previous = [dialog.clone()];
        assert!(already_entered(&previous, false, &dialog));
        let same_name = held(2, Backend::Msaa, Role::Dialog, "Remove");
        assert!(
            already_entered(&previous, true, &same_name),
            "in the same window"
        );
        assert!(
            !already_entered(&previous, false, &same_name),
            "in another window"
        );
        let uia_previous = [held(1, Backend::Uia, Role::Dialog, "Remove")];
        let uia_same_name = held(2, Backend::Uia, Role::Dialog, "Remove");
        assert!(
            !already_entered(&uia_previous, true, &uia_same_name),
            "one outpost's UIA node is one element"
        );
    }

    #[test]
    fn a_focus_for_an_outpost_keeps_its_own_nodes_unstamped() {
        let ours = |id| NodeId::new(id).with_outpost(OutpostId(3));
        let node = |id: NodeId| HeldNode {
            id,
            backend: Backend::Msaa,
            role: Role::Pane,
            name: None,
            states: StateSet::default(),
        };
        let focus = HeldFocus {
            trace_id: TraceId::mint(),
            observed: (1, 0),
            focus: node(ours(5)),
            ancestors: vec![
                node(NodeId::new(9).with_outpost(OutpostId(4))),
                node(ours(4)),
            ],
            top_level: None,
            alive: true,
        };
        let mine = focus.for_outpost(OutpostId(3)).expect("the focus is ours");
        assert_eq!(mine.focus.id, NodeId::new(5));
        assert_eq!(
            mine.ancestors
                .iter()
                .map(|node| node.id)
                .collect::<Vec<_>>(),
            vec![NodeId::new(4)]
        );
        assert_eq!(focus.for_outpost(OutpostId(4)), None);
    }
}
