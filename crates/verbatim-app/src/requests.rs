//! The request table: the single owner of "exactly one outcome per query"
//! (outpost redesign, "The app shell").
//!
//! Every query Core sends an outpost is recorded here, on the reducer thread,
//! with a request id and the outpost incarnation it went to. The first
//! outcome removes the entry and goes to whoever asked; any later outcome
//! for the same id is dropped, and so is an outcome arriving from a
//! different outpost than the one asked. Core creates the outcome itself
//! when a query cannot be sent ("failed") and for every query still
//! outstanding when its outpost ends ("gone").

use std::collections::HashMap;

use crossbeam_channel::Sender;
use verbatim_model::{FetchResult, Input, OutpostId, QueryId, QueryKind, TraceId, TreeNode};

/// The answer a tree-dump request waits for: the walked tree and whether the
/// walk was truncated, or a reason it failed.
pub(crate) type DumpTreeResult = Result<(TreeNode, bool), String>;

/// Names one query Core sent an outpost. Unique for the life of the table,
/// so an old reply can never satisfy a newer request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RequestId(pub(crate) u64);

/// Who asked, and therefore where the outcome goes.
pub(crate) enum Asker {
    /// The reducer's object-navigation query; its outcome re-enters the
    /// reducer as `Input::FetchCompleted` under the reducer's own query id.
    Reducer {
        /// The reducer's id for the query.
        query_id: QueryId,
        /// What it asked for, echoed back to the reducer.
        kind: QueryKind,
        /// The trace the query belongs to.
        trace_id: TraceId,
    },
    /// An activation: the reducer does not wait for it, but it still gets
    /// exactly one outcome, which is logged.
    Activation,
    /// A control-plane tree dump, answered on this channel. The channel is
    /// bounded with room for the one answer, so sending never blocks; a
    /// requester that already gave up has dropped its receiver.
    DumpTree(Sender<DumpTreeResult>),
}

/// What became of a query.
pub(crate) enum Outcome {
    /// An object-navigation step finished.
    Fetched(FetchResult),
    /// A tree dump finished, or failed with a reason.
    Dumped(DumpTreeResult),
    /// An activation was invoked, or failed with a reason.
    Activated(Result<(), String>),
    /// The outpost ended before answering.
    Gone,
    /// The query could not be sent.
    Failed(String),
}

struct Entry {
    outpost: OutpostId,
    asker: Asker,
}

/// Outstanding queries, keyed by request id. Owned by the reducer thread.
#[derive(Default)]
pub(crate) struct RequestTable {
    next: u64,
    entries: HashMap<RequestId, Entry>,
}

impl RequestTable {
    /// Records a query about to be sent to `outpost` and returns its id.
    pub(crate) fn begin(&mut self, outpost: OutpostId, asker: Asker) -> RequestId {
        self.next += 1;
        let id = RequestId(self.next);
        self.entries.insert(id, Entry { outpost, asker });
        id
    }

    /// Delivers the outcome of request `id`, reported by `outpost`. Returns
    /// the reducer input it produces, if the asker was the reducer. Does
    /// nothing for an id that already has its outcome, or for an outcome
    /// from an outpost other than the one asked.
    pub(crate) fn finish(
        &mut self,
        id: RequestId,
        outpost: OutpostId,
        outcome: Outcome,
    ) -> Option<Input> {
        if self
            .entries
            .get(&id)
            .is_none_or(|entry| entry.outpost != outpost)
        {
            return None;
        }
        let entry = self.entries.remove(&id)?;
        deliver(entry.asker, outcome)
    }

    /// Ends every request still outstanding at `outpost` with "gone",
    /// returning the reducer inputs that produces.
    pub(crate) fn outpost_ended(&mut self, outpost: OutpostId) -> Vec<Input> {
        let ended: Vec<RequestId> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.outpost == outpost)
            .map(|(id, _)| *id)
            .collect();
        ended
            .into_iter()
            .filter_map(|id| {
                let entry = self.entries.remove(&id)?;
                deliver(entry.asker, Outcome::Gone)
            })
            .collect()
    }

    /// How many requests are outstanding.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Hands one outcome to its asker.
fn deliver(asker: Asker, outcome: Outcome) -> Option<Input> {
    match asker {
        Asker::Reducer {
            query_id,
            kind,
            trace_id,
        } => {
            let result = match outcome {
                Outcome::Fetched(result) => result,
                Outcome::Failed(reason) => {
                    tracing::warn!(reason, "a navigation query failed");
                    FetchResult::Gone
                }
                _ => FetchResult::Gone,
            };
            Some(Input::FetchCompleted {
                trace_id,
                query_id,
                kind,
                result,
            })
        }
        Asker::Activation => {
            match outcome {
                Outcome::Activated(Ok(())) => {}
                Outcome::Activated(Err(reason)) | Outcome::Failed(reason) => {
                    tracing::warn!(reason, "activation failed");
                }
                _ => tracing::warn!("activation got no answer: its outpost ended"),
            }
            None
        }
        Asker::DumpTree(reply) => {
            let answer = match outcome {
                Outcome::Dumped(answer) => answer,
                Outcome::Failed(reason) => Err(format!("could not reach the outpost: {reason}")),
                _ => Err("the outpost ended before answering".to_owned()),
            };
            // The requester may have timed out and dropped its receiver.
            let _ = reply.try_send(answer);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use crossbeam_channel::bounded;

    use super::*;

    fn navigation(query: u64) -> Asker {
        Asker::Reducer {
            query_id: QueryId(query),
            kind: QueryKind::Parent,
            trace_id: TraceId::mint(),
        }
    }

    #[test]
    fn the_first_outcome_wins_and_a_second_is_dropped() {
        let mut table = RequestTable::default();
        let id = table.begin(OutpostId(1), navigation(7));

        let first = table.finish(id, OutpostId(1), Outcome::Fetched(FetchResult::NoNeighbor));
        assert!(matches!(
            first,
            Some(Input::FetchCompleted {
                query_id: QueryId(7),
                result: FetchResult::NoNeighbor,
                ..
            })
        ));
        assert!(
            table
                .finish(id, OutpostId(1), Outcome::Fetched(FetchResult::Gone))
                .is_none()
        );
    }

    #[test]
    fn an_outcome_from_another_outpost_is_dropped() {
        let mut table = RequestTable::default();
        let id = table.begin(OutpostId(1), navigation(7));
        assert!(
            table
                .finish(id, OutpostId(2), Outcome::Fetched(FetchResult::NoNeighbor))
                .is_none()
        );
        assert_eq!(table.len(), 1, "the real answer can still arrive");
    }

    #[test]
    fn an_ended_outpost_answers_its_outstanding_queries_gone() {
        let mut table = RequestTable::default();
        table.begin(OutpostId(1), navigation(7));
        table.begin(OutpostId(2), navigation(8));

        let inputs = table.outpost_ended(OutpostId(1));
        assert!(matches!(
            inputs.as_slice(),
            [Input::FetchCompleted {
                query_id: QueryId(7),
                result: FetchResult::Gone,
                ..
            }]
        ));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn an_old_dump_reply_never_satisfies_a_newer_request() {
        let mut table = RequestTable::default();
        let (old_tx, old_rx) = bounded(1);
        let old = table.begin(OutpostId(1), Asker::DumpTree(old_tx));
        // The first requester timed out and went away; a second asks.
        drop(old_rx);
        let (new_tx, new_rx) = bounded(1);
        let new = table.begin(OutpostId(1), Asker::DumpTree(new_tx));

        // The old request's late reply arrives first.
        table.finish(old, OutpostId(1), Outcome::Dumped(Err("old".to_owned())));
        assert!(new_rx.try_recv().is_err(), "the newer request is untouched");

        table.finish(new, OutpostId(1), Outcome::Dumped(Err("new".to_owned())));
        assert_eq!(new_rx.try_recv().expect("answered"), Err("new".to_owned()));
    }
}
