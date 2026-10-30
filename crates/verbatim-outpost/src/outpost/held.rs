//! The outpost's reports whose focus node it has not released: what it
//! reads changes for (`docs/design/ownership-one-judge.md`, "The reports,
//! from the store").
//!
//! An outpost reads changes for every node it has not released, and Core's
//! targeting decides what is spoken. Core's focus line is always a set of
//! nodes the outpost carried and Core still holds, so it is always among
//! these reports' lines; while a `NodesHeld` is in flight the outpost has
//! released only what Core released at an acknowledged position, so the
//! set can be larger than Core's, never smaller: a lag costs reads, never a
//! word. A report is added at the worker's commit and removed by the
//! worker's release, under the same locks, when the store releases its
//! focus node. The outpost judges nothing against Core's focus.

use std::collections::HashSet;

use verbatim_core::focus_rules::HeldNode;
use verbatim_model::{EarlierElement, NodeId, Role, StateSet, TraceId};

/// One focus or foreground report of this outpost's, with what the outpost
/// recorded of it as it reported it.
#[derive(Clone, Debug)]
pub(super) struct Focus {
    /// The trace of the event it came from.
    pub(super) trace: TraceId,
    /// The focus.
    pub(super) node: HeldNode,
    /// Its ancestors, outermost first.
    pub(super) ancestors: Vec<HeldNode>,
    /// Its top-level window, 0 when not known.
    pub(super) top_level: isize,
    /// The top-level window that was the foreground window when the focus
    /// entered it, which NVDA holds the foreground object for
    /// (`Worker::is_foreground_window`); `None` when it was not.
    pub(super) entered_in_front: Option<isize>,
    /// The MSAA event address it came from: an MSAA focus's, or a
    /// foreground change's.
    pub(super) event: Option<(isize, i32, i32)>,
    /// Whether it is an MSAA focus acquired as a simple child, whose child
    /// id can be reused, so a focus event at its address is never a repeat
    /// of it.
    pub(super) simple_child: bool,
    /// The own text of each dialog of its line, as the report carried it:
    /// a later report whose line meets this one at or below a dialog
    /// carries the same text, with no call (`read::describe_dialogs`).
    pub(super) dialog_texts: Vec<(NodeId, Option<String>)>,
}

impl Focus {
    /// Its nodes' ids, the ancestors outermost first, then the focus.
    pub(super) fn chain_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.ancestors
            .iter()
            .chain(std::iter::once(&self.node))
            .map(|node| node.id)
    }

    /// The node `id` of its chain, when it is one.
    pub(super) fn node(&self, id: NodeId) -> Option<&HeldNode> {
        std::iter::once(&self.node)
            .chain(&self.ancestors)
            .find(|node| node.id == id)
    }

    /// Whether `other` has the same focus and the same line.
    fn same_line(&self, other: &Focus) -> bool {
        self.node.id == other.node.id && self.chain_ids().eq(other.chain_ids())
    }
}

/// What a new report's own evidence says of whether it repeats the newest
/// report: the reader's read of the element it held under a UIA node, or
/// the MSAA event address and whether it was acquired as a simple child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Evidence {
    /// A UIA focus, with what its reader read of the element it held
    /// before under its node.
    Uia(Option<EarlierElement>),
    /// An MSAA focus or a foreground change, by its event address.
    Msaa,
}

/// The reports whose focus node this outpost has not released, oldest
/// first, and the states the outpost last read of a focus.
#[derive(Default)]
pub(super) struct Held {
    /// The reports, oldest first: one per focus node and line, a newer
    /// report of the same node and line taking the older's place.
    reports: Vec<Focus>,
    /// A focus's states as this outpost last read them, so a state change
    /// tells whether it newly expanded the focus.
    states_read: Option<(NodeId, StateSet)>,
}

impl Held {
    /// The focus `focus` has been reported, with `evidence`: a report of
    /// the newest report's node whose evidence says the same element still
    /// has the focus is a repeat, which leaves the states last read as they
    /// were.
    pub(super) fn reported(&mut self, focus: Focus, evidence: Evidence) {
        if !self.repeats_newest(&focus, evidence) {
            self.states_read = Some((focus.node.id, focus.node.states));
        }
        self.reports.retain(|report| !report.same_line(&focus));
        self.reports.push(focus);
    }

    /// Whether `focus`, with `evidence`, repeats the newest report by its
    /// own evidence: the same node, and for UIA its reader read the element
    /// it held still focused or held none, for MSAA the same event address
    /// not acquired as a simple child.
    pub(super) fn repeats_newest(&self, focus: &Focus, evidence: Evidence) -> bool {
        let Some(newest) = self.reports.last() else {
            return false;
        };
        newest.node.id == focus.node.id
            && match evidence {
                Evidence::Uia(earlier) => {
                    matches!(earlier, None | Some(EarlierElement::Focused))
                }
                Evidence::Msaa => {
                    focus.event.is_some() && newest.event == focus.event && !focus.simple_child
                }
            }
    }

    /// The store released `released`, by node number: every report whose
    /// focus node is one of them is forgotten, and so are the states last
    /// read of a released focus.
    pub(super) fn release(&mut self, released: &HashSet<u64>) {
        self.reports.retain(|report| {
            let kept = !released.contains(&report.node.id.number());
            if !kept {
                tracing::debug!(
                    trace = %report.trace,
                    node = ?report.node.id,
                    "a report's focus node is released; its changes are no longer read"
                );
            }
            kept
        });
        if self
            .states_read
            .is_some_and(|(node, _)| released.contains(&node.number()))
        {
            self.states_read = None;
        }
    }

    /// The reports, oldest first.
    pub(super) fn focuses(&self) -> impl DoubleEndedIterator<Item = &Focus> {
        self.reports.iter()
    }

    /// The newest report: what the outpost reads of a focus's element for
    /// an event, such as the list it controls, it reads of this one.
    pub(super) fn expected(&self) -> Option<&Focus> {
        self.reports.last()
    }

    /// The newest report whose top-level window is `top_level`, not 0.
    pub(super) fn newest_in(&self, top_level: isize) -> Option<&Focus> {
        (top_level != 0)
            .then(|| {
                self.reports
                    .iter()
                    .rev()
                    .find(|report| report.top_level == top_level)
            })
            .flatten()
    }

    /// Whether `node` is the focus of any report.
    pub(super) fn is_a_focus(&self, node: NodeId) -> bool {
        self.focuses().any(|focus| focus.node.id == node)
    }

    /// The role `node` was read with, of any report's line, the newest
    /// first.
    pub(super) fn role_of(&self, node: NodeId) -> Option<Role> {
        self.reports
            .iter()
            .rev()
            .find_map(|focus| focus.node(node))
            .map(|node| node.role)
    }

    /// The text a report's line carried for dialog `node`, the newest
    /// report first, when one did.
    pub(super) fn dialog_text(&self, node: NodeId) -> Option<Option<String>> {
        self.reports.iter().rev().find_map(|focus| {
            focus
                .dialog_texts
                .iter()
                .find(|(dialog, _)| *dialog == node)
                .map(|(_, text)| text.clone())
        })
    }

    /// Whether `node` was expanded when its states were last read, and
    /// records `states` as its states now: a report's focus's; `None` for
    /// any other node.
    pub(super) fn replace_states(&mut self, node: NodeId, states: StateSet) -> Option<bool> {
        let before = self.expanded_before(node)?;
        self.states_read = Some((node, states));
        Some(before)
    }

    /// Whether `node` was expanded when its states were last read, as
    /// [`replace_states`](Self::replace_states) answers it, without
    /// recording anything: read before a state change's call, whose commit
    /// records the states then.
    pub(super) fn expanded_before(&self, node: NodeId) -> Option<bool> {
        if !self.is_a_focus(node) {
            return None;
        }
        let before = match self.states_read {
            Some((read, before)) if read == node => before,
            _ => self.role_states(node)?,
        };
        Some(before.contains(verbatim_model::State::Expanded))
    }

    /// The states `node`, a report's focus, was reported with, the newest
    /// first.
    fn role_states(&self, node: NodeId) -> Option<StateSet> {
        self.reports
            .iter()
            .rev()
            .find(|focus| focus.node.id == node)
            .map(|focus| focus.node.states)
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{Backend, State};

    use super::*;

    fn node(id: u64, role: Role) -> HeldNode {
        HeldNode {
            id: NodeId::new(id),
            backend: Backend::Msaa,
            role,
            name: None,
            states: StateSet::default(),
            window: None,
        }
    }

    fn reported(id: u64) -> Focus {
        Focus {
            trace: TraceId::mint(),
            node: node(id, Role::Button),
            ancestors: vec![node(100, Role::Dialog)],
            top_level: 7,
            entered_in_front: Some(7),
            event: Some((7, -4, i32::try_from(id).unwrap_or(0))),
            simple_child: false,
            dialog_texts: Vec::new(),
        }
    }

    fn released(numbers: &[u64]) -> HashSet<u64> {
        numbers.iter().copied().collect()
    }

    /// A report stays until the store releases its focus node, whatever
    /// reports come after it, and is then forgotten.
    #[test]
    fn a_report_stays_until_its_focus_node_is_released() {
        let mut held = Held::default();
        held.reported(reported(1), Evidence::Msaa);
        held.reported(reported(2), Evidence::Msaa);
        assert_eq!(
            held.focuses()
                .map(|focus| focus.node.id)
                .collect::<Vec<_>>(),
            vec![NodeId::new(1), NodeId::new(2)]
        );
        held.release(&released(&[100]));
        assert_eq!(
            held.focuses().count(),
            2,
            "an ancestor's release keeps both"
        );
        held.release(&released(&[1]));
        assert_eq!(
            held.focuses()
                .map(|focus| focus.node.id)
                .collect::<Vec<_>>(),
            vec![NodeId::new(2)]
        );
        assert_eq!(
            held.expected().map(|focus| focus.node.id),
            Some(NodeId::new(2))
        );
    }

    /// A state change tells whether it newly expanded a report's focus.
    #[test]
    fn a_state_change_tells_whether_it_newly_expanded_a_focus() {
        let mut held = Held::default();
        held.reported(reported(1), Evidence::Msaa);
        let expanded = StateSet::default().with(State::Expanded);
        assert_eq!(held.replace_states(NodeId::new(1), expanded), Some(false));
        assert_eq!(held.replace_states(NodeId::new(1), expanded), Some(true));
        assert_eq!(held.replace_states(NodeId::new(3), expanded), None);
    }

    /// A repeat of the newest report by its own evidence leaves the states
    /// last read as they were; a report of the same node whose reader read
    /// another element under it does not.
    #[test]
    fn a_repeat_by_its_own_evidence_leaves_the_states_read() {
        let mut held = Held::default();
        held.reported(reported(1), Evidence::Uia(None));
        let expanded = StateSet::default().with(State::Expanded);
        assert_eq!(held.replace_states(NodeId::new(1), expanded), Some(false));
        held.reported(reported(1), Evidence::Uia(Some(EarlierElement::Focused)));
        assert_eq!(
            held.expanded_before(NodeId::new(1)),
            Some(true),
            "the repeat kept the expanded state read since"
        );
        held.reported(reported(1), Evidence::Uia(Some(EarlierElement::NotFocused)));
        assert_eq!(
            held.expanded_before(NodeId::new(1)),
            Some(false),
            "a new element under the node"
        );
    }
}
