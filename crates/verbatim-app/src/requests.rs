//! Who asks the shell's own queries of outposts, and where each outcome
//! goes (`books/dev/src/reference/crates/verbatim-app.md`, `requests`).
//!
//! A reply is routed by its request id's asker ([`reply_input`]): one to a
//! query of Core's reducer, which records its own queries, re-enters the
//! reducer as `Input::Answered`; one to a query of the shell's goes to the
//! request table, `verbatim-core`'s
//! [`verbatim_core::requests::RequestTable`], the one owner of each of the
//! shell's queries' outcome. It holds each [`Asker`] as a token and hands
//! it back with its one outcome, and [`deliver`] carries that outcome out,
//! into the reducer's inputs, a control-plane reply or a dialog's
//! responder.

use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::Sender;
use verbatim_model::protocol::{QueryOutcome, QueryResult, RequestId, Requester, Responder};
use verbatim_model::{Input, OutpostId, QueryId, TraceId, TreeNode};

/// The answer a tree-dump request waits for: the walked tree and whether the
/// walk was truncated, or a reason it failed.
pub(crate) type DumpTreeResult = Result<(TreeNode, bool), String>;

/// Names one control-plane tree dump, minted by the caller that waits for
/// it, so the caller can withdraw it without holding anything that keeps
/// its reply channel open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DumpTicket(u64);

impl DumpTicket {
    /// A ticket no other dump in this process has.
    pub(crate) fn mint() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// The request table, holding who asked each query
/// ([`verbatim_core::requests::RequestTable`]).
pub(crate) type RequestTable = verbatim_core::requests::RequestTable<Asker>;

/// Who in the shell asked, and therefore where the outcome goes.
pub(crate) enum Asker {
    /// A control-plane tree dump, answered on `reply`. The channel is
    /// bounded with room for the one answer, so sending never blocks; a
    /// requester that already gave up has dropped its receiver.
    DumpTree {
        /// The requester's name for the dump.
        ticket: DumpTicket,
        /// Where the answer goes.
        reply: Sender<DumpTreeResult>,
    },
    /// The tray or taskbar list dialog's request for the shell's items,
    /// whose outcome goes to `deliver`: the items, or `None` for any
    /// failure.
    ShellItems {
        /// Where the outcome goes.
        deliver: Responder<Vec<verbatim_gui::ShellItem>>,
    },
    /// The list dialog's request for where the item clicked is now, whose
    /// outcome goes to `deliver`: the buttons that may be it, or `None` for
    /// any failure.
    ShellItemPosition {
        /// Where the outcome goes.
        deliver: Responder<verbatim_model::ShellCandidates>,
    },
}

/// Where a reply goes, by its request id's asker.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Routed {
    /// To Core's reducer, which records its own queries.
    Reducer(Input),
    /// To the shell's request table, with its outcome.
    Shell(QueryOutcome),
    /// To no asker this shell knows.
    Nobody,
}

/// Routes the outcome of query `id`, traced `trace_id`, from `outpost`, by
/// its asker: a query of Core's reducer re-enters it as `Input::Answered`
/// under the reply's trace, a failed outcome logged first, since Core does
/// no logging; one of the shell's goes to its request table.
pub(crate) fn reply_input(
    trace_id: TraceId,
    id: RequestId,
    outpost: OutpostId,
    outcome: QueryOutcome,
    at_us: u64,
) -> Routed {
    match id.asker {
        Requester::Reducer => {
            if !matches!(outcome, QueryOutcome::Done(_)) {
                tracing::debug!(
                    reason = describe(&outcome),
                    %id,
                    %outpost,
                    "a query of Core's was not answered"
                );
            }
            Routed::Reducer(Input::Answered {
                trace_id,
                query_id: QueryId(id.number),
                outpost,
                outcome,
                at_us,
            })
        }
        Requester::Shell => Routed::Shell(outcome),
        _ => {
            tracing::warn!(%id, "a reply to an asker the shell does not know is dropped");
            Routed::Nobody
        }
    }
}

/// A short description of an outcome other than success, for logs and
/// control-plane errors.
fn describe(outcome: &QueryOutcome) -> String {
    match outcome {
        QueryOutcome::Gone => "the node or its outpost is gone".to_owned(),
        QueryOutcome::Failed(reason) => reason.clone(),
        QueryOutcome::NotStarted => "the query was withdrawn before it started".to_owned(),
        QueryOutcome::Abandoned => "the query was abandoned: it passed its deadline, its window is not responding, or the user moved on".to_owned(),
        QueryOutcome::Done(_) => "the query returned an unexpected answer".to_owned(),
    }
}

/// Hands a shell item position query's outcome to the list dialog's
/// `deliver`: the buttons that may be the item, or `None` for any failure.
/// Nothing re-enters the reducer.
fn deliver_shell_item_position(
    deliver: Responder<verbatim_model::ShellCandidates>,
    outcome: QueryOutcome,
) {
    match outcome {
        QueryOutcome::Done(QueryResult::ShellItemPosition(found)) => deliver.answer(found),
        other => {
            tracing::warn!(
                reason = describe(&other),
                "the shell item's position was not read"
            );
            // Dropped unanswered: the dialog is handed nothing.
            drop(deliver);
        }
    }
}

/// Hands a shell items query's outcome to the list dialog's `deliver`:
/// the items, or `None` for any failure. Nothing re-enters the reducer.
fn deliver_shell_items(deliver: Responder<Vec<verbatim_gui::ShellItem>>, outcome: QueryOutcome) {
    match outcome {
        QueryOutcome::Done(QueryResult::ShellItems(items)) => deliver.answer(items),
        other => {
            tracing::warn!(
                reason = describe(&other),
                "the shell's items were not listed"
            );
            // Dropped unanswered: the dialog is handed nothing.
            drop(deliver);
        }
    }
}

/// Hands one outcome to its asker: a dialog or the control plane.
pub(crate) fn deliver(asker: Asker, outcome: QueryOutcome) {
    match asker {
        Asker::DumpTree { reply, .. } => {
            let answer = match outcome {
                QueryOutcome::Done(QueryResult::Tree(dumped)) => {
                    Ok((dumped.root, dumped.truncated))
                }
                other => Err(describe(&other)),
            };
            // The requester may have timed out and dropped its receiver.
            let _ = reply.try_send(answer);
        }
        Asker::ShellItems { deliver } => deliver_shell_items(deliver, outcome),
        Asker::ShellItemPosition { deliver } => deliver_shell_item_position(deliver, outcome),
    }
}

#[cfg(test)]
mod tests {
    use crossbeam_channel::bounded;
    use verbatim_model::protocol::DumpedTree;
    use verbatim_model::{Backend, NodeDetails, NodeId, NodeSnapshot, OutpostId, Role, StateSet};

    use super::*;

    /// Ends request `id` of `table` with `outcome`, from `outpost`, and
    /// carries the outcome out, as the reducer thread does.
    fn finish(
        table: &mut RequestTable,
        id: verbatim_model::protocol::RequestId,
        outpost: OutpostId,
        outcome: QueryOutcome,
    ) {
        if let Some((asker, outcome)) = table.finish(id, outpost, outcome) {
            deliver(asker, outcome);
        }
    }

    fn dump(reply: Sender<DumpTreeResult>) -> Asker {
        Asker::DumpTree {
            ticket: DumpTicket::mint(),
            reply,
        }
    }

    fn node(role: Role, name: &str) -> NodeSnapshot {
        NodeSnapshot {
            id: NodeId::new(1),
            backend: Backend::Uia,
            role,
            name: Some(name.to_owned()),
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        }
    }

    /// A shell items outcome reaches the dialog, and a request its outpost
    /// ended with, answered gone, gives it nothing.
    #[test]
    fn a_shell_items_answer_reaches_the_dialog_and_an_ended_outpost_gives_none() {
        let outcomes = std::sync::Arc::default();
        let mut table = RequestTable::default();
        let answered = table.begin(OutpostId(1), shell_items(&outcomes));
        table.begin(OutpostId(1), shell_items(&outcomes));
        finish(
            &mut table,
            answered,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::ShellItems(Vec::new())),
        );
        for (asker, outcome) in table.outpost_ended(OutpostId(1)) {
            deliver(asker, outcome);
        }
        assert_eq!(
            *outcomes.lock().expect("outcomes"),
            vec![Some(Vec::new()), None]
        );
    }

    /// A shell items request whose outcomes are recorded in order.
    fn shell_items(
        outcomes: &std::sync::Arc<std::sync::Mutex<Vec<Option<Vec<verbatim_gui::ShellItem>>>>>,
    ) -> Asker {
        let kept = std::sync::Arc::clone(outcomes);
        Asker::ShellItems {
            deliver: Responder::new(move |items| kept.lock().expect("outcomes").push(items)),
        }
    }

    #[test]
    fn a_shell_item_position_answer_reaches_the_dialog_and_a_failure_gives_none() {
        let outcomes: std::sync::Arc<
            std::sync::Mutex<Vec<Option<verbatim_model::ShellCandidates>>>,
        > = std::sync::Arc::default();
        let mut table = RequestTable::default();
        let asker = |outcomes: &std::sync::Arc<std::sync::Mutex<Vec<_>>>| {
            let kept = std::sync::Arc::clone(outcomes);
            Asker::ShellItemPosition {
                deliver: Responder::new(move |position| {
                    kept.lock().expect("outcomes").push(position);
                }),
            }
        };
        let found = verbatim_model::ShellCandidates {
            candidates: vec![verbatim_model::ShellCandidate {
                item: verbatim_model::ShellItem {
                    name: "Clock".to_owned(),
                    runtime_id: vec![42, 1],
                },
                offscreen: false,
                rect: Some(verbatim_model::Rect {
                    left: 1162,
                    top: 752,
                    width: 24,
                    height: 48,
                }),
            }],
            complete: true,
        };
        let answered = table.begin(OutpostId(1), asker(&outcomes));
        let failed = table.begin(OutpostId(1), asker(&outcomes));
        finish(
            &mut table,
            answered,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::ShellItemPosition(found.clone())),
        );
        finish(
            &mut table,
            failed,
            OutpostId(1),
            QueryOutcome::Failed("no client".into()),
        );
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![Some(found), None]);
        assert!(table.is_empty());
    }

    /// A reply to a query of Core's reducer re-enters it as its answer,
    /// under the reply's trace and the reducer's own number; one to a query
    /// of the shell's goes to the request table.
    #[test]
    fn a_reducer_reply_goes_to_the_reducer_and_a_shell_reply_to_the_table() {
        let trace_id = TraceId::from_raw(9);
        assert_eq!(
            reply_input(
                trace_id,
                RequestId::reducer(7),
                OutpostId(2),
                QueryOutcome::Gone,
                5
            ),
            Routed::Reducer(Input::Answered {
                trace_id,
                query_id: QueryId(7),
                outpost: OutpostId(2),
                outcome: QueryOutcome::Gone,
                at_us: 5,
            })
        );
        assert_eq!(
            reply_input(
                trace_id,
                RequestId::shell(7),
                OutpostId(2),
                QueryOutcome::Gone,
                5
            ),
            Routed::Shell(QueryOutcome::Gone)
        );
    }

    #[test]
    fn an_old_dump_reply_never_satisfies_a_newer_request() {
        let mut table = RequestTable::default();
        let (old_tx, old_rx) = bounded(1);
        let old = table.begin(OutpostId(1), dump(old_tx));
        // The first requester timed out and went away; a second asks.
        drop(old_rx);
        let (new_tx, new_rx) = bounded(1);
        let new = table.begin(OutpostId(1), dump(new_tx));

        // The old request's late reply arrives first.
        finish(
            &mut table,
            old,
            OutpostId(1),
            QueryOutcome::Failed("old".to_owned()),
        );
        assert!(new_rx.try_recv().is_err(), "the newer request is untouched");

        let tree = DumpedTree {
            root: TreeNode {
                snapshot: node(Role::Window, "new"),
                children: Vec::new(),
            },
            truncated: false,
            locations: false,
        };
        finish(
            &mut table,
            new,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::Tree(tree)),
        );
        let (root, _) = new_rx.try_recv().expect("answered").expect("a tree");
        assert_eq!(root.snapshot.name.as_deref(), Some("new"));
    }
}
