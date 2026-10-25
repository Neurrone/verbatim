//! The outpost incarnations the reducer thread treats as live (outpost
//! redesign, "The app shell"): started and not yet ended, with the
//! application each watches, whether it is ready, the bookkeeping for the
//! nodes held in it and the details it was told to read, and whether its
//! application has stopped responding.
//!
//! This is the one owner of the rule that a message from an outpost that
//! has ended never reaches the reducer, even when it arrives after its
//! replacement for the same application has started: the supervisor can
//! still deliver messages the old incarnation wrote before it ended, and
//! its node ids are dead.

use std::collections::{BTreeSet, HashMap};

use verbatim_model::{Fetches, OutpostId, Pid};

/// A live outpost incarnation.
pub(crate) struct Live {
    /// The process it watches.
    pub(crate) target_pid: Pid,
    /// The application it reports for when that is not its target: a
    /// console host's console's client, which its events name.
    application: Option<Pid>,
    /// Whether it has sent `Ready`.
    pub(crate) ready: bool,
    /// The position of the last of its messages handled here.
    pub(crate) position: u64,
    /// The held nodes and text anchors last left for it, and the position
    /// acknowledged then ([`Live::held_changed`]).
    held_left: (BTreeSet<u64>, BTreeSet<u64>, u64),
    /// Whether its last query passed its deadline with no answer since:
    /// the application is not responding.
    stalled: bool,
    /// The details to read last sent to it, `None` before the first.
    pub(crate) fetches_sent: Option<Fetches>,
}

impl Live {
    /// Whether `nodes` and `anchors`, the nodes and text anchors the state
    /// holds in this outpost, with the position of its last message handled
    /// here, differ from what was last left for it; if so, they are noted
    /// as left. A position that moved on is a change: the outpost keeps
    /// every node it reported until a list acknowledges the message, so
    /// the list goes again even when the state holds the same nodes. When
    /// nothing changed, nothing is left, and the outpost hears nothing.
    pub(crate) fn held_changed(&mut self, nodes: &BTreeSet<u64>, anchors: &BTreeSet<u64>) -> bool {
        let (left, left_anchors, acknowledged) = &self.held_left;
        if left == nodes && left_anchors == anchors && *acknowledged == self.position {
            return false;
        }
        self.held_left = (nodes.clone(), anchors.clone(), self.position);
        true
    }
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
                application: None,
                ready: false,
                position: 0,
                held_left: (BTreeSet::new(), BTreeSet::new(), 0),
                stalled: false,
                fetches_sent: None,
            },
        );
    }

    /// Records that a query to `outpost` passed its deadline (the outpost's
    /// watchdog abandoned it): whether the application has only now stopped
    /// responding, so "not responding" is reported once per stall rather
    /// than for every query that times out during it.
    pub(crate) fn query_abandoned(&mut self, outpost: OutpostId) -> bool {
        self.outposts
            .get_mut(&outpost)
            .is_some_and(|live| !std::mem::replace(&mut live.stalled, true))
    }

    /// Records that `outpost` answered: its application responds again.
    pub(crate) fn answered(&mut self, outpost: OutpostId) {
        if let Some(live) = self.outposts.get_mut(&outpost) {
            live.stalled = false;
        }
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

    /// How many live incarnations have not sent `Ready` yet.
    pub(crate) fn starting(&self) -> usize {
        self.outposts.values().filter(|live| !live.ready).count()
    }

    /// Records that `outpost`'s events belong to `application`, which is not
    /// its target: the console's client, for a console host's outpost
    /// (`docs/design/focus-pipeline.md`, section 5.4).
    pub(crate) fn reports_for(&mut self, outpost: OutpostId, application: Pid) {
        if let Some(live) = self.outposts.get_mut(&outpost) {
            live.application = Some(application);
        }
    }

    /// The application `outpost` reports for: the one its events last
    /// named, else its target.
    pub(crate) fn application_of(&self, outpost: OutpostId) -> Option<Pid> {
        self.outposts
            .get(&outpost)
            .map(|live| live.application.unwrap_or(live.target_pid))
    }

    /// The process whose outpost serves the application `pid`: the target
    /// of the newest live incarnation reporting for it, else `pid` itself.
    pub(crate) fn target_for(&self, pid: Pid) -> Pid {
        self.newest(pid)
            .and_then(|outpost| self.outposts.get(&outpost))
            .map_or(pid, |live| live.target_pid)
    }

    /// The newest live incarnation serving the application `pid`, ready or
    /// not.
    pub(crate) fn newest(&self, pid: Pid) -> Option<OutpostId> {
        self.of(pid, false)
    }

    /// The newest ready incarnation serving the application `pid`.
    pub(crate) fn ready(&self, pid: Pid) -> Option<OutpostId> {
        self.of(pid, true)
    }

    /// An incarnation serving `pid`: one whose events name `pid` as their
    /// application, a console host's for its console's client, else one
    /// watching `pid` itself; the newest of them.
    fn of(&self, pid: Pid, must_be_ready: bool) -> Option<OutpostId> {
        let newest = |serves: &dyn Fn(&Live) -> bool| {
            self.outposts
                .iter()
                .filter(|(_, live)| serves(live) && (live.ready || !must_be_ready))
                .map(|(outpost, _)| *outpost)
                .max()
        };
        newest(&|live| live.application == Some(pid))
            .or_else(|| newest(&|live| live.target_pid == pid))
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
    fn a_console_hosts_outpost_serves_its_consoles_client() {
        // The console host, 30, hosts the console of the client, 20, which
        // also has an outpost of its own.
        let mut live = LiveOutposts::default();
        live.started(OutpostId(3), Pid(30));
        live.mark_ready(OutpostId(3));
        live.started(OutpostId(4), Pid(20));
        live.mark_ready(OutpostId(4));
        assert_eq!(live.ready(Pid(20)), Some(OutpostId(4)), "not yet known");

        live.reports_for(OutpostId(3), Pid(20));
        assert_eq!(live.ready(Pid(20)), Some(OutpostId(3)));
        assert_eq!(live.newest(Pid(20)), Some(OutpostId(3)));
        assert_eq!(live.target_for(Pid(20)), Pid(30));
        assert_eq!(live.application_of(OutpostId(3)), Some(Pid(20)));
        assert_eq!(live.application_of(OutpostId(4)), Some(Pid(20)));
        assert_eq!(live.target_for(Pid(41)), Pid(41), "no outpost serves it");
    }

    #[test]
    fn a_stall_is_reported_once_until_the_outpost_answers_again() {
        let mut live = LiveOutposts::default();
        live.started(OutpostId(2), Pid(7));
        assert!(live.query_abandoned(OutpostId(2)), "the stall begins");
        assert!(
            !live.query_abandoned(OutpostId(2)),
            "a second timeout in the same stall"
        );
        live.answered(OutpostId(2));
        assert!(live.query_abandoned(OutpostId(2)), "a new stall");
        assert!(
            !live.query_abandoned(OutpostId(9)),
            "an outpost that is not live"
        );
    }

    #[test]
    fn an_accepted_message_records_its_position() {
        let mut live = LiveOutposts::default();
        live.started(OutpostId(2), Pid(7));
        live.accept(OutpostId(2), 41);
        let (_, record) = live.iter_mut().next().expect("one live outpost");
        assert_eq!(record.position, 41);
    }

    /// The held nodes are left for an outpost only when they, its anchors,
    /// or the position of its last message handled changed since they were
    /// last left: nothing is sent when nothing changed.
    #[test]
    fn held_nodes_are_left_only_when_they_or_the_position_changed() {
        let mut live = LiveOutposts::default();
        live.started(OutpostId(2), Pid(7));
        let nodes = |numbers: &[u64]| numbers.iter().copied().collect::<BTreeSet<u64>>();
        let mut changes = Vec::new();
        let mut step =
            |live: &mut LiveOutposts, position: Option<u64>, held: &[u64], anchors: &[u64]| {
                if let Some(position) = position {
                    live.accept(OutpostId(2), position);
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
            [false, true, false, true, false, true, true],
            "nothing yet; a message; the same again; an anchor; the same              again; a message holding the same; another node"
        );
    }
}
