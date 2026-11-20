//! The focus rules the reducer applies after an event is read, each
//! defined once: which property changes are spoken, which slider gives the
//! focus to its thumb, and where a new focus line meets the old one.
//!
//! The behaviour is specified in `docs/parity/changes-and-selection.md`
//! and `docs/parity/focus-context.md`, from which these are written.

use verbatim_model::{Backend, NodeId, ObjectKey};

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

/// A node as [`same_object`] compares it.
pub trait EnteredKey {
    /// Its identity: its id, and the window it is when it is one, with the
    /// backend that read it and its stand-in mark
    /// ([`WindowIdentity`](verbatim_model::WindowIdentity)).
    fn object_key(&self) -> ObjectKey;
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

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
