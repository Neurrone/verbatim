//! The request table: the single owner of "exactly one outcome per query"
//! for the shell's own queries (`books/dev/src/reference/crates/verbatim-core.md`, "The
//! request table"); the reducer's are recorded in its own state.
//!
//! Every query the shell sends an outpost is recorded here, on the reducer
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
use verbatim_model::protocol::{QueryOutcome, RequestId};

/// The reason a request whose deadline passed ends with, as failed.
pub const DEADLINE_PASSED: &str = "no answer came by the request's deadline";

struct Entry<A> {
    outpost: OutpostId,
    asker: A,
    /// When Core ends the request itself, in microseconds on the run's
    /// shared clock, if the outpost has not answered
    /// ([`RequestTable::expire`]); `None` for a request only its outpost or
    /// the outpost's end answers.
    deadline_us: Option<u64>,
}

/// A request whose deadline passed ([`RequestTable::expire`]).
#[derive(Debug, PartialEq)]
pub struct Expired<A> {
    /// The request, to withdraw from its outpost.
    pub id: RequestId,
    /// The outpost it went to.
    pub outpost: OutpostId,
    /// Who asked.
    pub asker: A,
    /// Its outcome, failed ([`DEADLINE_PASSED`]).
    pub outcome: QueryOutcome,
}

/// Outstanding queries, keyed by request id, each with its asker `A`.
/// Owned by the reducer thread.
pub struct RequestTable<A> {
    next: u64,
    entries: BTreeMap<RequestId, Entry<A>>,
}

impl<A> Default for RequestTable<A> {
    fn default() -> Self {
        Self {
            next: 0,
            entries: BTreeMap::new(),
        }
    }
}

impl<A> RequestTable<A> {
    /// Records a query about to be sent to `outpost` and returns its id,
    /// tagged [`Requester::Shell`](verbatim_model::protocol::Requester::Shell)
    /// and unique for the life of the table, so an old reply can never
    /// satisfy a newer request; a later request has a greater number.
    pub fn begin(&mut self, outpost: OutpostId, asker: A) -> RequestId {
        self.insert(outpost, asker, None)
    }

    /// Records a query about to be sent to `outpost` that Core ends itself,
    /// as failed, if no outcome has come by `deadline_us`, in microseconds
    /// on the run's shared clock ([`expire`](Self::expire)), and returns its
    /// id.
    pub fn begin_until(&mut self, outpost: OutpostId, asker: A, deadline_us: u64) -> RequestId {
        self.insert(outpost, asker, Some(deadline_us))
    }

    fn insert(&mut self, outpost: OutpostId, asker: A, deadline_us: Option<u64>) -> RequestId {
        self.next += 1;
        let id = RequestId::shell(self.next);
        self.entries.insert(
            id,
            Entry {
                outpost,
                asker,
                deadline_us,
            },
        );
        id
    }

    /// Removes request `id`.
    fn remove(&mut self, id: RequestId) -> Option<Entry<A>> {
        self.entries.remove(&id)
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
    /// ([`DEADLINE_PASSED`]), returning each one's id and outpost, to
    /// withdraw it from, and its asker with that outcome, in the order the
    /// requests were made. A later answer from its outpost finds no request
    /// and is dropped.
    pub fn expire(&mut self, now_us: u64) -> Vec<Expired<A>> {
        let expired: Vec<RequestId> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.deadline_us.is_some_and(|deadline| deadline <= now_us))
            .map(|(id, _)| *id)
            .collect();
        expired
            .into_iter()
            .filter_map(|id| self.remove(id).map(|entry| (id, entry)))
            .map(|(id, entry)| Expired {
                id,
                outpost: entry.outpost,
                asker: entry.asker,
                outcome: QueryOutcome::Failed(DEADLINE_PASSED.to_owned()),
            })
            .collect()
    }

    /// The earliest deadline of an outstanding request, in microseconds on
    /// the run's shared clock ([`begin_until`](Self::begin_until)).
    #[must_use]
    pub fn next_deadline(&self) -> Option<u64> {
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

    /// How many requests are outstanding.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no request is outstanding.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many requests are outstanding, for the end-to-end harness's
    /// idle barrier.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.entries.len()
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
        let second = table.begin_until(OutpostId(2), "second", 4_000_000);
        assert_eq!(table.next_deadline(), Some(4_000_000));
        assert_eq!(table.expire(3_999_999), []);
        assert_eq!(
            table.expire(5_000_000),
            [
                Expired {
                    id: first,
                    outpost: OutpostId(1),
                    asker: "first",
                    outcome: deadline_passed()
                },
                Expired {
                    id: second,
                    outpost: OutpostId(2),
                    asker: "second",
                    outcome: deadline_passed()
                }
            ]
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
