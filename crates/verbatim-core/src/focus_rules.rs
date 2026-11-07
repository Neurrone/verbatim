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
//! (`books/dev/src/explanation/design/ownership.md`).
//!
//! The behaviour is specified in `docs/parity/focus-events.md` ("Event
//! acceptance", "Duplicate focus suppression"), from which these are
//! written,
//! `docs/parity/changes-and-selection.md` and
//! `docs/parity/focus-context.md`.

use serde::{Deserialize, Serialize};
use verbatim_model::{Backend, NodeId, ObjectKey, Role, StateSet, WindowHandle, WindowIdentity};

/// How a window stands to the system's foreground window, the facts the
/// acceptance test ends with (`docs/parity/focus-events.md`, "Event
/// acceptance"): read by a reader before an event is read, and again by an
/// outpost when it reports the event ([`WindowFacts`]), for the attendance
/// of events other than focuses. Core judges a focus by its foreground
/// record's front instead, made from the focus listener's one report.
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
    /// that an outpost compares it by [`already_entered_ancestors`] as Core compares
    /// its snapshot. Left out of the message when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowHandle>,
    /// Whether it is a window above the process boundary described from
    /// Windows' state, a stand-in
    /// ([`NodeDetails::above_boundary`](verbatim_model::NodeDetails::above_boundary)).
    /// Left out of the message when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub above_boundary: bool,
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
            above_boundary: node.details.above_boundary,
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
    /// Its identity: its id, and the window it is when it is one, with the
    /// backend that read it and its stand-in mark
    /// ([`WindowIdentity`](verbatim_model::WindowIdentity)).
    fn object_key(&self) -> ObjectKey;
}

impl EnteredKey for HeldNode {
    fn object_key(&self) -> ObjectKey {
        ObjectKey {
            id: self.id,
            window: self.window.map(|handle| WindowIdentity {
                handle,
                backend: self.backend,
                stand_in: self.above_boundary,
            }),
        }
    }
}

impl<T: EnteredKey> EnteredKey for &T {
    fn object_key(&self) -> ObjectKey {
        (*self).object_key()
    }
}

impl EnteredKey for verbatim_model::NodeSnapshot {
    fn object_key(&self) -> ObjectKey {
        ObjectKey::of(self)
    }
}

/// Whether `old`, an entry of a previous focus line, and `new`, an entry of
/// a new one, are the same object by the one identity rule
/// ([`verbatim_model::ObjectKey::is_same_object`];
/// `docs/parity/focus-context.md`, "Focus-ancestry context"): the same
/// node; or both are windows, and the same window's object, read through
/// one backend or one a stand-in above the process boundary, so that one
/// window two outposts read is one entry. Two windows with the same role
/// and name are two objects. Where two lines meet is chosen by this alone
/// ([`lowest_common_ancestor`]).
pub fn same_object(old: &impl EnteredKey, new: &impl EnteredKey) -> bool {
    old.object_key().is_same_object(&new.object_key())
}

/// The lowest common ancestor of a new focus ancestry and the previous one,
/// where Core joins them, as NVDA joins its focus ancestry when the focus
/// moves (`docs/parity/focus-context.md`, "Focus-ancestry context"): each
/// ancestry is a focus's ancestors, outermost first, the desktop left out,
/// and then the focus, given as the two. The new ancestry is walked from
/// its focus outward, and at each entry the previous one from its focus
/// outward, and the first pair that is the same object ([`same_object`])
/// is the lowest common ancestor: `Some((old, new))`, their indexes in the
/// two, the focus's being its ancestors' count. The focus ancestry that
/// results is the previous one up to and including `old`, then the new
/// one's entries after `new`, the entered ancestors. `None` when the two
/// share no object. Nothing is allocated.
pub fn lowest_common_ancestor<O: EnteredKey, N: EnteredKey>(
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

/// How many of `new`'s entries, a new focus ancestry's ancestors outermost
/// first and then its focus, are already entered, given `previous`, an
/// ancestry the focus may have been in, given the same way: those above or
/// at their lowest common ancestor ([`lowest_common_ancestor`]), since
/// what is below it is entered, as NVDA's focus ancestry is joined
/// (`docs/parity/focus-context.md`, "Focus-ancestry context"); none when
/// they share no object. An outpost judges by it whether a dialog is newly
/// entered, and so gathers its own text. Nothing is allocated.
pub fn already_entered_ancestors<O: EnteredKey, N: EnteredKey>(
    previous: (&[O], &O),
    new: (&[N], &N),
) -> usize {
    lowest_common_ancestor(previous, new).map_or(0, |(_, at_new)| at_new.saturating_add(1))
}

/// The index of a report's innermost cached ancestor, its
/// `first_cached_ancestor`, as its focus reader computes it from the
/// entries it emits, `ancestors` outermost first, after its filters and
/// rules: the innermost of them `is_cached` names, the node of an ancestor
/// it took from its ancestry cache rather than read live. Where a filter
/// left the innermost cached ancestor out, this is the next cached one
/// outward that remains. `None` when none remains.
pub fn first_cached_ancestor(
    ancestors: &[NodeId],
    is_cached: impl Fn(NodeId) -> bool,
) -> Option<u32> {
    ancestors
        .iter()
        .rposition(|&ancestor| is_cached(ancestor))
        .and_then(|index| u32::try_from(index).ok())
}

/// Core's check that a report would speak an ancestor its focus reader
/// took from its ancestry cache (`books/dev/src/reference/crates/verbatim-core.md`,
/// "The ancestry cache"): `first_cached`, its `first_cached_ancestor`, the
/// index of its innermost cached ancestor among its ancestors outermost
/// first, so that every entry up to it is cached; `entered_from`, the
/// index of its first entered ancestor, below the lowest common ancestor
/// (its ancestors' count when it enters none); and `title`, the index of
/// the entry the title rule would speak, if any. A cached ancestor is
/// spoken when it is entered, or when it is the title rule's entry; the
/// entered ancestors are spoken first, so they are the reason when both
/// hold. `None` when no cached ancestor would be spoken, and for a report
/// with no cached ancestor. Nothing is allocated.
#[must_use]
pub fn needs_live_ancestry(
    first_cached: Option<usize>,
    entered_from: usize,
    title: Option<usize>,
) -> Option<verbatim_model::LiveRereadReason> {
    let first_cached = first_cached?;
    if entered_from <= first_cached {
        Some(verbatim_model::LiveRereadReason::Entered)
    } else if title.is_some_and(|title| title <= first_cached) {
        Some(verbatim_model::LiveRereadReason::Title)
    } else {
        None
    }
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
            above_boundary: false,
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
            already_entered_ancestors(previous, ([dialog].as_slice(), &other_button)),
            1
        );
        let same_name = held(4, Backend::Msaa, Role::Dialog, "Remove");
        assert_eq!(
            already_entered_ancestors(previous, ([same_name].as_slice(), &other_button)),
            0,
            "another dialog of the same role and name"
        );
    }

    #[test]
    fn two_windows_are_one_container_exactly_when_they_are_one_windows_object() {
        let window = |id, handle| HeldNode {
            window: Some(WindowHandle(handle)),
            ..held(id, Backend::Msaa, Role::Dialog, "Remove")
        };
        let focus = held(5, Backend::Msaa, Role::Button, "OK");
        let old_line = [window(1, 20)];
        let previous = (old_line.as_slice(), &focus);
        let other = held(6, Backend::Msaa, Role::Button, "Cancel");
        assert_eq!(
            already_entered_ancestors(previous, ([window(2, 20)].as_slice(), &other)),
            1,
            "the same window, from another outpost"
        );
        assert_eq!(
            already_entered_ancestors(previous, ([window(2, 21)].as_slice(), &other)),
            0,
            "another window of the same role and name"
        );
        let through_uia = HeldNode {
            backend: Backend::Uia,
            ..window(2, 20)
        };
        assert_eq!(
            already_entered_ancestors(previous, ([through_uia.clone()].as_slice(), &other)),
            0,
            "the same window's object through the other backend"
        );
        let stand_in = HeldNode {
            above_boundary: true,
            ..window(3, 20)
        };
        assert_eq!(
            already_entered_ancestors(
                ([stand_in].as_slice(), &focus),
                ([through_uia].as_slice(), &other)
            ),
            1,
            "a stand-in above the process boundary, for the same window"
        );
    }
}
