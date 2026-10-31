//! The focus rules the reducer shares with the focus listener and the
//! outposts, each defined once (decision D14, "Where the rules run"): how a
//! window stands to the foreground window, which focus repeats the focus
//! Core holds, which property changes and selections are spoken, and which
//! containers a focus newly enters. The rules ported from NVDA that only the
//! listener and the outposts make, the acceptance test and NVDA's
//! foreground tests, are defined once in `verbatim-reader-rules`, which both
//! share, since ported code stays in the GPL-tier crates. The reducer applies them after an event is
//! read; the focus listener and the outposts apply the same definitions
//! where a judgment must come before an element is read, because a UI
//! Automation element cannot pass to another process. Those judgments
//! never guess Core's focus: an outpost reads changes for every node it has
//! not released, and Core's targeting decides what is spoken
//! (`docs/design/ownership.md`).
//!
//! The behaviour is specified in `docs/parity/focus-events.md` ("Event
//! acceptance", "Duplicate focus suppression"), from which these are
//! written,
//! `docs/parity/changes-and-selection.md` and
//! `docs/parity/focus-context.md`.

use serde::{Deserialize, Serialize};
use verbatim_model::{Backend, NodeId, Role, StateSet, WindowFacts, WindowHandle};

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
    /// The window it is, when it is one
    /// ([`NodeDetails::window`](verbatim_model::NodeDetails::window)), so
    /// that an outpost compares it by [`already_entered`] as Core compares
    /// its snapshot. Left out of the message when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowHandle>,
}

impl HeldNode {
    /// `node` as the rules compare it.
    #[must_use]
    pub fn of(node: &verbatim_model::NodeSnapshot) -> Self {
        Self {
            id: node.id,
            backend: node.backend,
            role: node.role,
            name: node.name.clone(),
            states: node.states,
            window: node.details.window,
        }
    }
}

/// What Core knows of one focus event against the focus it holds
/// (`docs/parity/focus-events.md`, "Duplicate focus suppression"). Core is
/// the one judge: every reader reads a repeat in full.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatEvidence {
    /// The event names the element of the focus Core holds: the same node
    /// id.
    pub same_element: bool,
    /// That element still has the focus: what its reader read of the
    /// element it held under the node, or else the node still alive.
    pub still_focused: bool,
}

/// Whether a focus event repeats the focus Core holds, and is dropped
/// before anything else judges it.
#[must_use]
pub fn repeats_focus(evidence: RepeatEvidence) -> bool {
    evidence.same_element && evidence.still_focused
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

/// The UI Automation class of a slider that gives the keyboard focus to
/// its thumb (`docs/parity/focus-context.md`, "A slider that focuses its
/// thumb").
const SENSITIVE_SLIDER_CLASS: &str = "SensitiveSlider";

/// Whether `node` is a UI Automation slider that gives the focus to its
/// thumb: entered, it is spoken as a focus is, its value included, and its
/// value change is spoken while it is the focus's parent, so an outpost
/// follows its value changes while it is
/// (`docs/parity/focus-context.md`, "A slider that focuses its thumb").
#[must_use]
pub fn is_sensitive_slider(node: &verbatim_model::NodeSnapshot) -> bool {
    node.backend == Backend::Uia
        && node.details.class_name.as_deref() == Some(SENSITIVE_SLIDER_CLASS)
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

/// A node as [`same_object`] compares it.
pub trait EnteredKey {
    /// Its id.
    fn key_id(&self) -> NodeId;
    /// The window it is, when it is one
    /// ([`NodeDetails::window`](verbatim_model::NodeDetails::window)).
    fn key_window(&self) -> Option<WindowHandle>;
}

impl EnteredKey for HeldNode {
    fn key_id(&self) -> NodeId {
        self.id
    }
    fn key_window(&self) -> Option<WindowHandle> {
        self.window
    }
}

impl<T: EnteredKey> EnteredKey for &T {
    fn key_id(&self) -> NodeId {
        (*self).key_id()
    }
    fn key_window(&self) -> Option<WindowHandle> {
        (*self).key_window()
    }
}

impl EnteredKey for verbatim_model::NodeSnapshot {
    fn key_id(&self) -> NodeId {
        self.id
    }
    fn key_window(&self) -> Option<WindowHandle> {
        self.details.window
    }
}

/// Whether `old`, an entry of a previous focus line, and `new`, an entry of
/// a new one, are the same object by the one identity rule
/// ([`verbatim_model::ObjectKey::is_same_object`];
/// `docs/parity/focus-context.md`, "Focus-ancestry context"): the same
/// node; or both are windows, and they are the same window, so that one
/// window two outposts read is one entry. Two windows with the same role
/// and name are two objects. Where two lines meet is chosen by this alone
/// ([`meeting`]).
pub fn same_object(old: &impl EnteredKey, new: &impl EnteredKey) -> bool {
    key_of(old).is_same_object(&key_of(new))
}

/// `node`'s identity, as [`same_object`] compares it.
fn key_of(node: &impl EnteredKey) -> verbatim_model::ObjectKey {
    verbatim_model::ObjectKey {
        id: node.key_id(),
        window: node.key_window(),
    }
}

/// Where a new focus line meets the previous one, as NVDA joins them when
/// the focus moves (`docs/parity/focus-context.md`, "Focus-ancestry
/// context"): each line is a focus's ancestors, outermost first, the
/// desktop left out, and then the focus, given as the two. The new line is
/// walked from its focus outward, and at each entry the previous line from
/// its focus outward, and the first pair that is the same object
/// ([`same_object`]) is the meeting: `Some((old, new))`, their indexes in
/// the two lines, the focus's being its ancestors' count. The focus line
/// that results is the previous line up to and including `old`, then the
/// new line's entries after `new`; those entries below the meeting point
/// are the ones the focus has newly entered. `None` when the lines share
/// no object. Nothing is allocated.
pub fn meeting<O: EnteredKey, N: EnteredKey>(
    (previous, previous_focus): (&[O], &O),
    (new, new_focus): (&[N], &N),
) -> Option<(usize, usize)> {
    let old_at = |index: usize| previous.get(index).unwrap_or(previous_focus);
    let new_at = |index: usize| new.get(index).unwrap_or(new_focus);
    (0..=new.len()).rev().find_map(|at_new| {
        (0..=previous.len())
            .rev()
            .find(|&at_old| same_object(old_at(at_old), new_at(at_new)))
            .map(|at_old| (at_old, at_new))
    })
}

/// How many of `new`'s entries, a new focus line's ancestors outermost
/// first and then its focus, are already entered, given `previous`, a line
/// the focus may have been in, given the same way: those above or at where
/// the two lines meet ([`meeting`]), since what is below the meeting point
/// is entered, as NVDA's focus line is joined (`docs/parity/focus-context.md`,
/// "Focus-ancestry context"); none when they share no object. An outpost
/// judges by it whether a dialog is newly entered, and so gathers its own
/// text. Nothing is allocated.
pub fn already_entered<O: EnteredKey, N: EnteredKey>(
    previous: (&[O], &O),
    new: (&[N], &N),
) -> usize {
    meeting(previous, new).map_or(0, |(_, at_new)| at_new + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::OutpostId;

    #[test]
    fn a_repeat_needs_every_piece_of_evidence() {
        let all = RepeatEvidence {
            same_element: true,
            still_focused: true,
        };
        assert!(repeats_focus(all));
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
            window: None,
        }
    }

    /// A dialog of the line the focus was in is already entered by a new
    /// line that meets that line at it or below it, and one of the same
    /// role and name that is another object is not.
    #[test]
    fn a_container_is_entered_unless_the_lines_meet_at_or_below_it() {
        let dialog = held(1, Backend::Msaa, Role::Dialog, "Remove");
        let button = held(2, Backend::Msaa, Role::Button, "OK");
        let other_button = held(3, Backend::Msaa, Role::Button, "Cancel");
        let old_line = [dialog.clone()];
        let previous = (old_line.as_slice(), &button);
        assert_eq!(
            already_entered(previous, ([dialog].as_slice(), &other_button)),
            1
        );
        let same_name = held(4, Backend::Msaa, Role::Dialog, "Remove");
        assert_eq!(
            already_entered(previous, ([same_name].as_slice(), &other_button)),
            0,
            "another dialog of the same role and name"
        );
    }

    #[test]
    fn two_windows_are_one_container_exactly_when_their_handles_are_equal() {
        let window = |id, handle| HeldNode {
            window: Some(WindowHandle(handle)),
            ..held(id, Backend::Msaa, Role::Dialog, "Remove")
        };
        let focus = held(5, Backend::Msaa, Role::Button, "OK");
        let old_line = [window(1, 20)];
        let previous = (old_line.as_slice(), &focus);
        let other = held(6, Backend::Msaa, Role::Button, "Cancel");
        assert_eq!(
            already_entered(previous, ([window(2, 20)].as_slice(), &other)),
            1,
            "the same window, from another outpost"
        );
        assert_eq!(
            already_entered(previous, ([window(2, 21)].as_slice(), &other)),
            0,
            "another window of the same role and name"
        );
    }
}
