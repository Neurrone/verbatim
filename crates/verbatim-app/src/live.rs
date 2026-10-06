//! The outpost incarnations the reducer thread treats as live (outpost
//! redesign, "The app shell"): started and not yet ended, with the
//! application each watches, whether it is ready, and the bookkeeping for
//! the nodes held in it.
//!
//! This is the one owner of the rule that a message from an outpost that
//! has ended never reaches the reducer, even when it arrives after its
//! replacement for the same application has started: the supervisor can
//! still deliver messages the old incarnation wrote before it ended, and
//! its node ids are dead.

use std::collections::{BTreeSet, HashMap};

use verbatim_model::{OutpostId, Pid};

/// A live outpost incarnation.
pub(crate) struct Live {
    /// The application it watches.
    pub(crate) target_pid: Pid,
    /// Whether it has sent `Ready`.
    pub(crate) ready: bool,
    /// The position of the last of its messages handled here.
    pub(crate) position: u64,
    /// The held nodes and text anchors last sent to it, and the position
    /// acknowledged then.
    pub(crate) held_sent: (BTreeSet<u64>, BTreeSet<u64>, u64),
}

/// The live outpost incarnations, by outpost id.
#[derive(Default)]
pub(crate) struct LiveOutposts {
    outposts: HashMap<OutpostId, Live>,
}

impl LiveOutposts {
    /// Records that `outpost` started, watching `target_pid`.
    pub(crate) fn started(&mut self, outpost: OutpostId, target_pid: Pid) {
        self.outposts.insert(
            outpost,
            Live {
                target_pid,
                ready: false,
                position: 0,
                held_sent: (BTreeSet::new(), BTreeSet::new(), 0),
            },
        );
    }

    /// Whether a message from `outpost` may reach the reducer: only while
    /// that incarnation is live. An accepted message's `position` is
    /// recorded as handled.
    pub(crate) fn accept(&mut self, outpost: OutpostId, position: u64) -> bool {
        match self.outposts.get_mut(&outpost) {
            Some(live) => {
                live.position = position;
                true
            }
            None => false,
        }
    }

    /// Records that `outpost` sent `Ready`.
    pub(crate) fn mark_ready(&mut self, outpost: OutpostId) {
        if let Some(live) = self.outposts.get_mut(&outpost) {
            live.ready = true;
        }
    }

    /// Forgets `outpost`, which ended. Whether it was live.
    pub(crate) fn ended(&mut self, outpost: OutpostId) -> bool {
        self.outposts.remove(&outpost).is_some()
    }

    /// `pid`'s newest live incarnation, ready or not.
    pub(crate) fn newest(&self, pid: Pid) -> Option<OutpostId> {
        self.of(pid, false)
    }

    /// `pid`'s newest incarnation that is ready.
    pub(crate) fn ready(&self, pid: Pid) -> Option<OutpostId> {
        self.of(pid, true)
    }

    fn of(&self, pid: Pid, must_be_ready: bool) -> Option<OutpostId> {
        self.outposts
            .iter()
            .filter(|(_, live)| live.target_pid == pid && (live.ready || !must_be_ready))
            .map(|(outpost, _)| *outpost)
            .max()
    }

    /// Every live incarnation, for sending the nodes held in each.
    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = (&OutpostId, &mut Live)> {
        self.outposts.iter_mut()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_from_a_replaced_incarnation_never_reaches_the_reducer() {
        let mut live = LiveOutposts::default();
        live.started(OutpostId(3), Pid(40));
        live.mark_ready(OutpostId(3));
        assert!(live.accept(OutpostId(3), 1));
        assert!(live.ended(OutpostId(3)));
        live.started(OutpostId(4), Pid(40));

        assert!(
            !live.accept(OutpostId(3), 2),
            "written before it ended, delivered after its replacement started"
        );
        assert!(live.accept(OutpostId(4), 1));
        assert!(!live.ended(OutpostId(3)), "already gone");
    }

    #[test]
    fn the_ready_outpost_is_the_newest_ready_incarnation() {
        let mut live = LiveOutposts::default();
        live.started(OutpostId(3), Pid(40));
        live.mark_ready(OutpostId(3));
        live.started(OutpostId(5), Pid(41));
        live.mark_ready(OutpostId(5));
        assert_eq!(live.ready(Pid(40)), Some(OutpostId(3)));

        live.ended(OutpostId(3));
        live.started(OutpostId(6), Pid(40));
        assert_eq!(
            live.ready(Pid(40)),
            None,
            "the replacement is still starting"
        );
        assert_eq!(live.newest(Pid(40)), Some(OutpostId(6)));
        live.mark_ready(OutpostId(6));
        assert_eq!(live.ready(Pid(40)), Some(OutpostId(6)));
    }

    #[test]
    fn an_accepted_message_records_its_position() {
        let mut live = LiveOutposts::default();
        live.started(OutpostId(2), Pid(7));
        live.accept(OutpostId(2), 41);
        let (_, record) = live.iter_mut().next().expect("one live outpost");
        assert_eq!(record.position, 41);
    }
}
