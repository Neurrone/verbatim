//! The app's own facts of each outpost generation started and not yet
//! ended: the position of its last message handled and the nodes held in
//! it last left for it. Which generation is current, starting or ready is
//! Core's record's alone.

use std::collections::{BTreeSet, HashMap};

use verbatim_model::OutpostId;

/// A live outpost generation.
pub(crate) struct Live {
    /// The position of the last of its messages handled here.
    pub(crate) position: u64,
    /// The held nodes and text anchors last left for it, and the position
    /// acknowledged then ([`Live::held_changed`]).
    held_left: (BTreeSet<u64>, BTreeSet<u64>, u64),
}

impl Live {
    /// Whether `nodes` and `anchors`, the nodes and text anchors the state
    /// holds in this outpost, with the position of its last message handled
    /// here, differ from what was last left for it; if so, they are noted
    /// as left. A position that moved on is a change: the outpost keeps
    /// every node it reported until a list acknowledges the message, so
    /// the list goes again even when the state holds the same nodes. When
    /// nothing changed, nothing is left, and the outpost hears nothing.
    pub(crate) fn held_changed(
        &mut self,
        nodes: &BTreeSet<u64>,
        anchors: &BTreeSet<u64>,
    ) -> HeldChange {
        let (left, left_anchors, acknowledged) = &self.held_left;
        let change = if left != nodes {
            HeldChange::Nodes
        } else if left_anchors != anchors || *acknowledged != self.position {
            HeldChange::Other
        } else {
            return HeldChange::None;
        };
        self.held_left = (nodes.clone(), anchors.clone(), self.position);
        change
    }
}

/// What changed of what the state holds in an outpost since it was last
/// left for it ([`Live::held_changed`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeldChange {
    /// Nothing.
    None,
    /// The set of nodes.
    Nodes,
    /// The text anchors or the position of the last message handled, and
    /// not the nodes.
    Other,
}

/// The live outpost generations, by outpost id.
#[derive(Default)]
pub(crate) struct LiveOutposts {
    outposts: HashMap<OutpostId, Live>,
}

impl LiveOutposts {
    /// Records that `outpost` started.
    pub(crate) fn started(&mut self, outpost: OutpostId) {
        self.outposts.insert(
            outpost,
            Live {
                position: 0,
                held_left: (BTreeSet::new(), BTreeSet::new(), 0),
            },
        );
    }

    /// Records `position` as the last of `outpost`'s messages handled. The
    /// supervisor reports a generation started before any of its messages
    /// and ended after the last, so a message of one not started here is
    /// a logic bug, logged.
    pub(crate) fn handled(&mut self, outpost: OutpostId, position: u64) {
        if let Some(live) = self.outposts.get_mut(&outpost) {
            live.position = position;
        } else {
            tracing::error!(target: verbatim_model::DEFECT_TARGET,
                %outpost,
                "a message of an outpost generation not started or already ended, a logic bug"
            );
        }
    }

    /// Forgets `outpost`, which ended. Whether it was live.
    pub(crate) fn ended(&mut self, outpost: OutpostId) -> bool {
        self.outposts.remove(&outpost).is_some()
    }

    /// Every live generation, for sending the nodes held in each.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&OutpostId, &mut Live)> {
        self.outposts.iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handled_message_records_its_position() {
        let mut live = LiveOutposts::default();
        live.started(OutpostId(2));
        live.handled(OutpostId(2), 41);
        let (_, record) = live.iter_mut().next().expect("one live outpost");
        assert_eq!(record.position, 41);
    }

    /// The held nodes are left for an outpost only when they, its anchors,
    /// or the position of its last message handled changed since they were
    /// last left, a change of the nodes told from one of the rest, which
    /// the app writes at once: nothing is sent when nothing changed.
    #[test]
    fn held_nodes_are_left_only_when_they_or_the_position_changed() {
        let mut live = LiveOutposts::default();
        live.started(OutpostId(2));
        let nodes = |numbers: &[u64]| numbers.iter().copied().collect::<BTreeSet<u64>>();
        let mut changes = Vec::new();
        let mut step =
            |live: &mut LiveOutposts, position: Option<u64>, held: &[u64], anchors: &[u64]| {
                if let Some(position) = position {
                    live.handled(OutpostId(2), position);
                }
                let (_, record) = live.iter_mut().next().expect("one live outpost");
                changes.push(record.held_changed(&nodes(held), &nodes(anchors)));
            };
        step(&mut live, None, &[], &[]);
        step(&mut live, Some(1), &[5], &[]);
        step(&mut live, None, &[5], &[]);
        step(&mut live, None, &[5], &[8]);
        step(&mut live, None, &[5], &[8]);
        step(&mut live, Some(2), &[5], &[8]);
        step(&mut live, None, &[6], &[8]);
        assert_eq!(
            changes,
            [
                HeldChange::None,
                HeldChange::Nodes,
                HeldChange::None,
                HeldChange::Other,
                HeldChange::None,
                HeldChange::Other,
                HeldChange::Nodes
            ],
            "nothing yet; a message; the same again; an anchor; the same              again; a message holding the same; another node"
        );
    }
}
