//! The request table: the single owner of "exactly one outcome per query"
//! (`docs/crates/verbatim-core.md`, "The request table").
//!
//! Every query Core sends an outpost is recorded here, on the reducer
//! thread, with a request id and the outpost generation it went to. The
//! first outcome removes the entry and hands it back with whoever asked;
//! any later outcome for the same id is dropped, and so is an outcome
//! arriving from a different outpost than the one asked. Core creates the
//! outcome itself when a query cannot be sent ("failed"), for every query
//! still outstanding when its outpost ends ("gone"), and for a request whose
//! deadline passed ("failed").
//!
//! The table holds each asker as a token it never calls: every outcome
//! leaves it as an `(asker, outcome)` pair, which the shell carries out, so
//! the table makes no call and reads no clock. Ids are given in increasing
//! order, so outcomes the table produces together come in the order the
//! requests were made.

use std::collections::BTreeMap;

use verbatim_model::OutpostId;
use verbatim_model::protocol::QueryOutcome;

/// Names one query Core sent an outpost. Unique for the life of the table,
/// so an old reply can never satisfy a newer request; a later request has a
/// greater id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

/// The reason a request whose deadline passed ends with, as failed.
pub const DEADLINE_PASSED: &str = "no answer came by the request's deadline";

struct Entry<A> {
    outpost: OutpostId,
    asker: A,
    /// Whether the request only waits for the application to act, as a
    /// caret watch waits for a caret key's evidence: it does not keep
    /// Verbatim from being idle ([`RequestTable::outstanding`]).
    passive: bool,
    /// When Core ends the request itself, in microseconds on the run's
    /// shared clock, if the outpost has not answered
    /// ([`RequestTable::expire`]); `None` for a request only its outpost or
    /// the outpost's end answers.
    deadline_us: Option<u64>,
}

/// Outstanding queries, keyed by request id, each with its asker `A`.
/// Owned by the reducer thread.
pub struct RequestTable<A> {
    next: u64,
    entries: BTreeMap<RequestId, Entry<A>>,
    /// How many entries have a deadline, so the reducer thread's check for
    /// one costs nothing while none is outstanding.
    timed: usize,
}

impl<A> Default for RequestTable<A> {
    fn default() -> Self {
        Self {
            next: 0,
            entries: BTreeMap::new(),
            timed: 0,
        }
    }
}

impl<A> RequestTable<A> {
    /// Records a query about to be sent to `outpost` and returns its id.
    pub fn begin(&mut self, outpost: OutpostId, asker: A) -> RequestId {
        self.insert(outpost, asker, false, None)
    }

    /// Records a query about to be sent to `outpost` that Core ends itself,
    /// as failed, if no outcome has come by `deadline_us`, in microseconds
    /// on the run's shared clock ([`expire`](Self::expire)), and returns its
    /// id.
    pub fn begin_until(&mut self, outpost: OutpostId, asker: A, deadline_us: u64) -> RequestId {
        self.insert(outpost, asker, false, Some(deadline_us))
    }

    /// Records a passive request about to be sent to `outpost`, one that
    /// only waits for the application to act, as a caret watch does, and
    /// returns its id: it gets its one outcome as any other does, but is not
    /// counted [`outstanding`](Self::outstanding), so an open watch, which a
    /// key that moved nothing leaves open until its bound, does not keep
    /// Verbatim from being idle.
    pub fn begin_passive(&mut self, outpost: OutpostId, asker: A) -> RequestId {
        self.insert(outpost, asker, true, None)
    }

    fn insert(
        &mut self,
        outpost: OutpostId,
        asker: A,
        passive: bool,
        deadline_us: Option<u64>,
    ) -> RequestId {
        self.next += 1;
        let id = RequestId(self.next);
        self.timed += usize::from(deadline_us.is_some());
        self.entries.insert(
            id,
            Entry {
                outpost,
                asker,
                passive,
                deadline_us,
            },
        );
        id
    }

    /// Removes request `id`, keeping the count of those with a deadline.
    fn remove(&mut self, id: RequestId) -> Option<Entry<A>> {
        let entry = self.entries.remove(&id)?;
        self.timed -= usize::from(entry.deadline_us.is_some());
        Some(entry)
    }

    /// Ends request `id` with `outcome`, reported by `outpost`, returning
    /// its asker with the outcome to carry out. Returns nothing for an id
    /// that already has its outcome, or for an outcome from an outpost other
    /// than the one asked.
    pub fn finish(
        &mut self,
        id: RequestId,
        outpost: OutpostId,
        outcome: QueryOutcome,
    ) -> Option<(A, QueryOutcome)> {
        if self
            .entries
            .get(&id)
            .is_none_or(|entry| entry.outpost != outpost)
        {
            return None;
        }
        self.remove(id).map(|entry| (entry.asker, outcome))
    }

    /// Ends every request still outstanding at `outpost` with "gone",
    /// returning their askers with that outcome, in the order the requests
    /// were made.
    pub fn outpost_ended(&mut self, outpost: OutpostId) -> Vec<(A, QueryOutcome)> {
        let ended: Vec<RequestId> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.outpost == outpost)
            .map(|(id, _)| *id)
            .collect();
        ended
            .into_iter()
            .filter_map(|id| self.remove(id))
            .map(|entry| (entry.asker, QueryOutcome::Gone))
            .collect()
    }

    /// Ends every request whose deadline is `now_us` or earlier as failed
    /// ([`DEADLINE_PASSED`]), returning their askers with that outcome, in
    /// the order the requests were made. A later answer from its outpost
    /// finds no request and is dropped.
    pub fn expire(&mut self, now_us: u64) -> Vec<(A, QueryOutcome)> {
        if self.timed == 0 {
            return Vec::new();
        }
        let expired: Vec<RequestId> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.deadline_us.is_some_and(|deadline| deadline <= now_us))
            .map(|(id, _)| *id)
            .collect();
        expired
            .into_iter()
            .filter_map(|id| self.remove(id))
            .map(|entry| {
                (
                    entry.asker,
                    QueryOutcome::Failed(DEADLINE_PASSED.to_owned()),
                )
            })
            .collect()
    }

    /// The earliest deadline of an outstanding request, in microseconds on
    /// the run's shared clock ([`begin_until`](Self::begin_until)).
    #[must_use]
    pub fn next_deadline(&self) -> Option<u64> {
        if self.timed == 0 {
            return None;
        }
        self.entries
            .values()
            .filter_map(|entry| entry.deadline_us)
            .min()
    }

    /// The first outstanding request, in the order they were made, whose
    /// asker `which` picks, and the outpost it went to: a request whose
    /// caller stopped waiting for it, to withdraw from its outpost while it
    /// keeps its one outcome.
    pub fn find(&self, which: impl Fn(&A) -> bool) -> Option<(RequestId, OutpostId)> {
        self.entries
            .iter()
            .find(|(_, entry)| which(&entry.asker))
            .map(|(id, entry)| (*id, entry.outpost))
    }

    /// Ends the first outstanding request, in the order they were made,
    /// whose asker `which` picks, with no outcome, so nothing waits for it
    /// and its outcome when it comes is dropped: the request, the outpost to
    /// withdraw it from, and its asker, which the caller drops.
    pub fn withdraw(&mut self, which: impl Fn(&A) -> bool) -> Option<(RequestId, OutpostId, A)> {
        let (id, _) = self.find(which)?;
        self.remove(id)
            .map(|entry| (id, entry.outpost, entry.asker))
    }

    /// How many requests are outstanding, passive ones included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no request is outstanding, passive ones included.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many requests are outstanding, for the end-to-end harness's
    /// idle barrier: passive ones ([`begin_passive`](Self::begin_passive))
    /// are not counted.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.entries.values().filter(|entry| !entry.passive).count()
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::protocol::QueryResult;

    use super::*;

    /// A request's asker in these tests: a name for it.
    type Asker = &'static str;

    fn done() -> QueryOutcome {
        QueryOutcome::Done(QueryResult::Navigated(None))
    }

    fn deadline_passed() -> QueryOutcome {
        QueryOutcome::Failed(DEADLINE_PASSED.to_owned())
    }

    #[test]
    fn the_first_outcome_wins_and_a_second_is_dropped() {
        let mut table = RequestTable::<Asker>::default();
        let id = table.begin(OutpostId(1), "navigation");
        assert_eq!(
            table.finish(id, OutpostId(1), done()),
            Some(("navigation", done()))
        );
        assert_eq!(table.finish(id, OutpostId(1), QueryOutcome::Gone), None);
        assert!(table.is_empty());
    }

    #[test]
    fn an_outcome_from_another_outpost_is_dropped() {
        let mut table = RequestTable::<Asker>::default();
        let id = table.begin(OutpostId(1), "navigation");
        assert_eq!(table.finish(id, OutpostId(2), QueryOutcome::Gone), None);
        assert_eq!(table.len(), 1, "the real answer can still arrive");
    }

    #[test]
    fn a_passive_request_is_not_outstanding_but_gets_its_outcome() {
        let mut table = RequestTable::<Asker>::default();
        let watch = table.begin_passive(OutpostId(1), "watch");
        let read = table.begin(OutpostId(1), "read");
        assert_eq!(table.outstanding(), 1, "the watch is not outstanding");
        assert_eq!(
            table.finish(watch, OutpostId(1), QueryOutcome::Gone),
            Some(("watch", QueryOutcome::Gone))
        );
        assert_eq!(table.outstanding(), 1);
        table.finish(read, OutpostId(1), QueryOutcome::Gone);
        assert_eq!(table.outstanding(), 0);
    }

    #[test]
    fn a_withdrawn_request_is_not_outstanding_and_its_late_outcome_is_dropped() {
        let mut table = RequestTable::<Asker>::default();
        let read = table.begin(OutpostId(1), "read");
        assert_eq!(table.withdraw(|asker| *asker == "other"), None);
        assert_eq!(
            table.withdraw(|asker| *asker == "read"),
            Some((read, OutpostId(1), "read"))
        );
        assert_eq!(table.outstanding(), 0, "the idle condition is clear");
        assert_eq!(
            table.finish(read, OutpostId(1), QueryOutcome::NotStarted),
            None
        );
    }

    #[test]
    fn a_request_is_found_by_its_asker_and_keeps_its_outcome() {
        let mut table = RequestTable::<Asker>::default();
        let first = table.begin(OutpostId(1), "dump");
        table.begin(OutpostId(2), "other dump");
        assert_eq!(
            table.find(|asker| *asker == "dump"),
            Some((first, OutpostId(1)))
        );
        assert_eq!(table.len(), 2, "finding a request leaves it outstanding");
        table.finish(first, OutpostId(1), QueryOutcome::NotStarted);
        assert_eq!(table.find(|asker| *asker == "dump"), None);
    }

    /// A generation's `Ended` answers its queries "gone", whether a next
    /// generation follows or not, in the order they were asked, however
    /// many there are; another outpost's are kept.
    #[test]
    fn an_ended_outpost_answers_its_outstanding_queries_gone_in_request_order() {
        let mut table = RequestTable::<Asker>::default();
        let names = [
            "first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth",
        ];
        for name in names {
            table.begin(OutpostId(1), name);
            table.begin(OutpostId(2), "elsewhere");
        }
        assert_eq!(
            table.outpost_ended(OutpostId(1)),
            names.map(|name| (name, QueryOutcome::Gone))
        );
        assert_eq!(table.len(), names.len());
    }

    #[test]
    fn a_request_answered_before_its_deadline_is_not_expired() {
        let mut table = RequestTable::<Asker>::default();
        let id = table.begin_until(OutpostId(1), "items", 5_000_000);
        assert_eq!(table.next_deadline(), Some(5_000_000));
        table.finish(id, OutpostId(1), done());
        assert_eq!(table.expire(5_000_000), []);
        assert_eq!(table.next_deadline(), None);
    }

    /// Requests whose deadlines pass on one wake end failed in the order
    /// they were made, a request not yet due and one without a deadline
    /// are kept, and a late answer finds nothing.
    #[test]
    fn requests_past_their_deadline_end_failed_in_request_order() {
        let mut table = RequestTable::<Asker>::default();
        let later = table.begin_until(OutpostId(1), "later", 9_000_000);
        let first = table.begin_until(OutpostId(1), "first", 5_000_000);
        table.begin(OutpostId(1), "untimed");
        table.begin_until(OutpostId(2), "second", 4_000_000);
        assert_eq!(table.next_deadline(), Some(4_000_000));
        assert_eq!(table.expire(3_999_999), []);
        assert_eq!(
            table.expire(5_000_000),
            [("first", deadline_passed()), ("second", deadline_passed())]
        );
        assert_eq!(table.next_deadline(), Some(9_000_000));
        assert_eq!(table.finish(first, OutpostId(1), done()), None);
        assert_eq!(table.len(), 2, "the later and the untimed are kept");
        table.finish(later, OutpostId(1), done());
        assert_eq!(table.next_deadline(), None);
    }

    /// An old reply never satisfies a newer request, since every request
    /// has an id of its own.
    #[test]
    fn an_old_reply_never_satisfies_a_newer_request() {
        let mut table = RequestTable::<Asker>::default();
        let old = table.begin(OutpostId(1), "old");
        let new = table.begin(OutpostId(1), "new");
        assert_eq!(
            table.finish(old, OutpostId(1), QueryOutcome::Gone),
            Some(("old", QueryOutcome::Gone))
        );
        assert_eq!(
            table.find(|asker| *asker == "new"),
            Some((new, OutpostId(1)))
        );
    }
}
