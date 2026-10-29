//! The focus Core holds, as an outpost judges its application's events
//! against it before it reads them (decision D14, "Where the rules run").
//!
//! Core sends an outpost the focus it holds whenever it judges one of the
//! outpost's focus events, taken or refused, and whenever its focus moves
//! to or from the outpost's application (`SupervisorToOutpost::CoreFocus`),
//! with the node ids as the outpost issued them. The rules themselves are
//! `verbatim_core::focus_rules`'.
//!
//! Between a focus report and Core's judgement of it, Core's focus can yet
//! become the focus reported, so the outpost keeps the focuses it reported
//! that Core has not judged ([`Held::unjudged`]) beside the focus Core
//! holds, and judges an event against them all: a change is read when it
//! can be spoken for any of them, since the reducer decides after the read
//! with the focus it holds then; a focus or foreground change is dropped
//! as a repeat only when it repeats every one of them, since Core's focus
//! will be one of them whatever Core judges. Once Core has judged them,
//! the focus it holds is the only one.

use std::collections::VecDeque;

use verbatim_core::focus_rules::{HeldFocus, HeldNode};
use verbatim_model::{NodeId, Role, StateSet, TraceId};

use crate::protocol::observed_no_earlier;

/// One focus as the outpost judges against it: the focus Core holds, or
/// one it reported that Core has not judged yet, with what the outpost
/// recorded of it as it reported it.
#[derive(Clone, Debug)]
pub(super) struct Focus {
    /// The trace of the event it came from, by which Core names it.
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
    /// foreground change's, which NVDA's second foreground test compares.
    pub(super) event: Option<(isize, i32, i32)>,
    /// Whether it is an MSAA focus acquired as a simple child, whose child
    /// id can be reused, so a focus event at its address is never a repeat
    /// of it.
    pub(super) simple_child: bool,
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
}

/// The focus Core holds and the focuses reported since that Core has not
/// judged, as the outpost knows them.
#[derive(Default)]
pub(super) struct Held {
    /// The focus Core holds, when it is this outpost's.
    core: Option<Focus>,
    /// When the focus Core holds was observed, when it is another
    /// outpost's, in microseconds; 0 for none.
    elsewhere: u64,
    /// The focuses this outpost reported that Core has not judged, oldest
    /// first.
    unjudged: VecDeque<Focus>,
    /// A focus's states as this outpost last read them, so a state change
    /// tells whether it newly expanded the focus.
    states_read: Option<(NodeId, StateSet)>,
}

impl Held {
    /// The focus `focus` has been reported: Core has not judged it yet.
    pub(super) fn reported(&mut self, focus: Focus) {
        self.states_read = Some((focus.node.id, focus.node.states));
        self.unjudged.push_back(focus);
    }

    /// Core judged this outpost's focus event traced `judged`, if any, and
    /// every one reported before it, and holds `focus`, this outpost's,
    /// or another outpost's observed at `observed_at_us`
    /// (`SupervisorToOutpost::CoreFocus`).
    pub(super) fn core_focus(
        &mut self,
        judged: Option<TraceId>,
        focus: Option<HeldFocus>,
        observed_at_us: u64,
    ) {
        let mut judged_focuses = Vec::new();
        if let Some(judged) = judged
            && let Some(at) = self.unjudged.iter().position(|focus| focus.trace == judged)
        {
            judged_focuses.extend(self.unjudged.drain(..=at));
        }
        let Some(held) = focus else {
            self.core = None;
            // A focus with no observation time is taken as now, as it is
            // newer than any the outpost waits on.
            let observed = if observed_at_us == 0 {
                crate::protocol::now_us()
            } else {
                observed_at_us
            };
            if observed_no_earlier(observed, self.elsewhere) {
                self.elsewhere = observed;
            }
            return;
        };
        // What this outpost recorded of the focus as it reported it.
        let recorded = judged_focuses
            .into_iter()
            .rev()
            .chain(self.core.take())
            .find(|focus| focus.trace == held.trace_id);
        let top_level = held
            .top_level
            .map_or(0, |top| isize::from_ne_bytes(top.0.to_ne_bytes()));
        self.core = Some(Focus {
            trace: held.trace_id,
            node: held.focus,
            ancestors: held.ancestors,
            top_level,
            entered_in_front: recorded.as_ref().and_then(|focus| focus.entered_in_front),
            event: recorded.as_ref().and_then(|focus| focus.event),
            simple_child: recorded.is_some_and(|focus| focus.simple_child),
        });
    }

    /// When the focus Core holds was observed, when it is another
    /// outpost's; 0 for none.
    pub(super) fn elsewhere_observed(&self) -> u64 {
        self.elsewhere
    }

    /// The focus Core holds, when it is this outpost's.
    pub(super) fn core(&self) -> Option<&Focus> {
        self.core.as_ref()
    }

    /// Whether Core has judged every focus this outpost reported.
    #[cfg(test)]
    pub(super) fn settled(&self) -> bool {
        self.unjudged.is_empty()
    }

    /// The focuses an event is judged against: the focus Core holds, when
    /// it is this outpost's, and those Core has not judged yet.
    pub(super) fn focuses(&self) -> impl Iterator<Item = &Focus> {
        self.core.iter().chain(&self.unjudged)
    }

    /// The focus Core will hold once it has judged what this outpost
    /// reported, as far as the outpost can tell: the newest it reported
    /// that Core has not judged, else the focus Core holds. What the
    /// outpost reads of a focus's element for an event, such as the list it
    /// controls, it reads of this one.
    pub(super) fn expected(&self) -> Option<&Focus> {
        self.unjudged.back().or(self.core.as_ref())
    }

    /// Whether `node` is the focus of any of the focuses events are judged
    /// against ([`focuses`](Self::focuses)).
    pub(super) fn is_a_focus(&self, node: NodeId) -> bool {
        self.focuses().any(|focus| focus.node.id == node)
    }

    /// The role `node` was read with, of any of the focuses events are
    /// judged against, the newest first.
    pub(super) fn role_of(&self, node: NodeId) -> Option<Role> {
        self.unjudged
            .iter()
            .rev()
            .chain(self.core.iter())
            .find_map(|focus| focus.node(node))
            .map(|node| node.role)
    }

    /// Whether `node` was expanded when its states were last read, and
    /// records `states` as its states now: a focus's, when it is one of the
    /// focuses events are judged against; `None` for any other node.
    pub(super) fn replace_states(&mut self, node: NodeId, states: StateSet) -> Option<bool> {
        if !self.is_a_focus(node) {
            return None;
        }
        let before = match self.states_read {
            Some((read, before)) if read == node => before,
            _ => self.role_states(node)?,
        };
        self.states_read = Some((node, states));
        Some(before.contains(verbatim_model::State::Expanded))
    }

    /// The states `node`, a focus, was reported or taken with, the newest
    /// first.
    fn role_states(&self, node: NodeId) -> Option<StateSet> {
        self.unjudged
            .iter()
            .rev()
            .chain(self.core.iter())
            .find(|focus| focus.node.id == node)
            .map(|focus| focus.node.states)
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{Backend, State, WindowHandle};

    use super::*;

    fn node(id: u64, role: Role) -> HeldNode {
        HeldNode {
            id: NodeId::new(id),
            backend: Backend::Msaa,
            role,
            name: None,
            states: StateSet::default(),
        }
    }

    fn reported(trace: TraceId, id: u64) -> Focus {
        Focus {
            trace,
            node: node(id, Role::Button),
            ancestors: vec![node(100, Role::Dialog)],
            top_level: 7,
            entered_in_front: Some(7),
            event: Some((7, -4, 0)),
            simple_child: false,
        }
    }

    fn held_focus(trace: TraceId, id: u64) -> HeldFocus {
        HeldFocus {
            trace_id: trace,
            observed_at_us: 1,
            focus: node(id, Role::Button),
            ancestors: vec![node(100, Role::Dialog)],
            top_level: Some(WindowHandle(7)),
            alive: true,
        }
    }

    #[test]
    fn a_focus_reported_is_judged_against_until_core_judges_it() {
        let mut held = Held::default();
        let (first, second) = (TraceId::mint(), TraceId::mint());
        held.reported(reported(first, 1));
        held.reported(reported(second, 2));
        assert!(!held.settled());
        assert_eq!(held.expected().map(|focus| focus.trace), Some(second));
        held.core_focus(Some(first), Some(held_focus(first, 1)), 1);
        assert!(!held.settled(), "the second still waits");
        assert!(held.is_a_focus(1.into_node()) && held.is_a_focus(2.into_node()));
        held.core_focus(Some(second), Some(held_focus(first, 1)), 1);
        assert!(held.settled());
        assert!(
            !held.is_a_focus(2.into_node()),
            "Core refused the second: its focus is the first"
        );
        assert_eq!(
            held.core()
                .map(|focus| (focus.entered_in_front, focus.event)),
            Some((Some(7), Some((7, -4, 0)))),
            "what the outpost recorded of the focus Core took"
        );
    }

    #[test]
    fn a_focus_elsewhere_records_when_it_was_observed() {
        let mut held = Held::default();
        let trace = TraceId::mint();
        held.reported(reported(trace, 1));
        held.core_focus(Some(trace), Some(held_focus(trace, 1)), 5);
        held.core_focus(None, None, 9);
        assert!(held.core().is_none());
        assert_eq!(held.elsewhere_observed(), 9);
    }

    /// Core's focus elsewhere is kept by its newest observation, ordered to
    /// the microsecond as Core orders two focuses in one millisecond.
    #[test]
    fn a_focus_elsewhere_in_the_same_millisecond_is_ordered_by_microseconds() {
        let mut held = Held::default();
        held.core_focus(None, None, 6_543_800);
        held.core_focus(None, None, 6_543_200);
        assert_eq!(held.elsewhere, 6_543_800);
        held.core_focus(None, None, 6_544_000);
        assert_eq!(held.elsewhere_observed(), 6_544_000);
    }

    #[test]
    fn a_state_change_tells_whether_it_newly_expanded_a_focus() {
        let mut held = Held::default();
        held.reported(reported(TraceId::mint(), 1));
        let expanded = StateSet::default().with(State::Expanded);
        assert_eq!(held.replace_states(1.into_node(), expanded), Some(false));
        assert_eq!(held.replace_states(1.into_node(), expanded), Some(true));
        assert_eq!(held.replace_states(3.into_node(), expanded), None);
    }

    trait IntoNode {
        fn into_node(self) -> NodeId;
    }

    impl IntoNode for u64 {
        fn into_node(self) -> NodeId {
            NodeId::new(self)
        }
    }
}
