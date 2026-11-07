//! The request table: the single owner of "exactly one outcome per query"
//! for the shell's own queries (`books/dev/src/reference/crates/verbatim-core.md`, "The
//! request table"); the reducer's are recorded in its own state.
//!
//! Every query the shell sends an outpost is recorded here, on the reducer
//! thread, with a request id and the outpost generation it went to. The
//! first outcome removes the entry and hands it back with whoever asked;
//! any later outcome for the same id is dropped, and so is an outcome
//! arriving from a different outpost than the one asked. Core creates the
//! outcome itself when a query cannot be sent ("failed") and for every
//! query still outstanding when its outpost ends ("gone"). It keeps no
//! deadline of its own: an outpost answers each query by its program's
//! deadline, so a second clock here could answer a request its outpost
//! then answers too.
//!
//! The table holds each asker as a token it never calls: every outcome
//! leaves it as an `(asker, outcome)` pair, which the shell carries out, so
//! the table makes no call and reads no clock. Ids are given in increasing
//! order, so outcomes the table produces together come in the order the
//! requests were made.

use std::collections::BTreeMap;

use verbatim_model::OutpostId;
use verbatim_model::protocol::{QueryOutcome, RequestId};

struct Entry<A> {
    outpost: OutpostId,
    asker: A,
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
        self.next = self.next.saturating_add(1);
        let id = RequestId::shell(self.next);
        self.entries.insert(id, Entry { outpost, asker });
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
