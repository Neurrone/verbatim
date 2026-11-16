//! Who asks Core's requests from outside the event queue, and where each
//! outcome goes (`books/dev/src/reference/crates/verbatim-app.md`,
//! `requests`): the control plane's tree dump and the notification area
//! list's two reads of the taskbar
//! (`docs/design/readers-tell-core-asks-build1.md`, "Requests in Core").
//! Core keeps each request's due time, hang check and outpost, and answers
//! it once, under the token the shell keeps its asker by
//! (`QueueEffect::Answered`); [`Outside`] holds each [`Asker`] by its token
//! until then, and [`deliver`] carries the outcome out, into a
//! control-plane reply or a dialog's responder.
//!
//! A reply to a query of Core's reducer, the queries it still sends
//! itself, re-enters the reducer as `Input::Answered` ([`reply_input`]).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::Sender;
use verbatim_model::protocol::{QueryOutcome, RequestId, Requester, Responder};
use verbatim_model::request::{OutsideOutcome, Reply};
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

/// The askers of Core's requests from outside the event queue, each by
/// the token Core answers it under, from its request until its one
/// answer.
#[derive(Default)]
pub(crate) struct Outside {
    /// The next token, never reused.
    next: u64,
    /// Each asker waiting, by its token.
    waiting: BTreeMap<u64, Asker>,
}

impl Outside {
    /// Keeps `asker` and returns the token its request is asked under.
    pub(crate) fn begin(&mut self, asker: Asker) -> u64 {
        self.next += 1;
        self.waiting.insert(self.next, asker);
        self.next
    }

    /// The asker of the request asked under `token`, taken: its one
    /// answer has come. `None` for a token no asker waits under.
    pub(crate) fn answered(&mut self, token: u64) -> Option<Asker> {
        self.waiting.remove(&token)
    }

    /// The token the tree dump named `ticket` is asked under, while its
    /// asker waits.
    pub(crate) fn dump(&self, ticket: DumpTicket) -> Option<u64> {
        self.waiting
            .iter()
            .find(|(_, asker)| matches!(asker, Asker::DumpTree { ticket: asked, .. } if *asked == ticket))
            .map(|(token, _)| *token)
    }
}

/// The reducer input for the outcome of query `id`, traced `trace_id`,
/// from `outpost`: a query of Core's reducer re-enters it as
/// `Input::Answered` under the reply's trace, a failed outcome logged
/// first, since Core does no logging. The shell asks no query of its own,
/// so a reply to any other asker is dropped, `None`.
pub(crate) fn reply_input(
    trace_id: TraceId,
    id: RequestId,
    outpost: OutpostId,
    outcome: QueryOutcome,
    at_us: u64,
) -> Option<Input> {
    if !matches!(id.asker, Requester::Reducer) {
        tracing::warn!(%id, "a reply to an asker the shell does not know is dropped");
        return None;
    }
    if !matches!(outcome, QueryOutcome::Done(_)) {
        tracing::debug!(
            reason = describe_query(&outcome),
            %id,
            %outpost,
            "a query of Core's was not answered"
        );
    }
    Some(Input::Answered {
        trace_id,
        query_id: QueryId(id.number),
        outpost,
        outcome,
        at_us,
    })
}

/// A short description of a query's outcome other than success, for logs.
fn describe_query(outcome: &QueryOutcome) -> String {
    match outcome {
        QueryOutcome::Gone => "the node or its outpost is gone".to_owned(),
        QueryOutcome::Failed(reason) => reason.clone(),
        QueryOutcome::NotStarted => "the query was withdrawn before it started".to_owned(),
        QueryOutcome::Abandoned => "the query was abandoned: it passed its deadline, its window is not responding, or the user moved on".to_owned(),
        QueryOutcome::Done(_) => "the query returned an unexpected answer".to_owned(),
    }
}

/// A short description of an outcome other than the reply wanted, for logs
/// and control-plane errors.
fn describe(outcome: &OutsideOutcome) -> String {
    match outcome {
        OutsideOutcome::Result(_) => "the request returned an unexpected answer".to_owned(),
        OutsideOutcome::Refused(refusal) => format!("the reader refused the request: {refusal:?}"),
        OutsideOutcome::Undecodable => "the answer did not decode".to_owned(),
        OutsideOutcome::NotThere => "the object is not there".to_owned(),
        OutsideOutcome::NotResponding => "the application is not responding".to_owned(),
        OutsideOutcome::ReaderEnded => "its outpost ended or the read failed".to_owned(),
        OutsideOutcome::NoLongerAwaited(reason) => {
            format!("Core no longer awaited it: {reason:?}")
        }
    }
}

/// Hands one outcome to its asker: a dialog or the control plane. Nothing
/// re-enters the reducer.
pub(crate) fn deliver(asker: Asker, outcome: OutsideOutcome) {
    match (asker, outcome) {
        (Asker::DumpTree { reply, .. }, OutsideOutcome::Result(Reply::Tree(dumped))) => {
            // The requester may have timed out and dropped its receiver.
            let _ = reply.try_send(Ok((dumped.root, dumped.truncated)));
        }
        (Asker::DumpTree { reply, .. }, other) => {
            let _ = reply.try_send(Err(describe(&other)));
        }
        (Asker::ShellItems { deliver }, OutsideOutcome::Result(Reply::ShellItems(items))) => {
            deliver.answer(items);
        }
        (
            Asker::ShellItemPosition { deliver },
            OutsideOutcome::Result(Reply::ShellItemPosition(found)),
        ) => deliver.answer(found),
        (Asker::ShellItems { deliver }, other) => {
            tracing::warn!(
                reason = describe(&other),
                "the shell's items were not listed"
            );
            // Dropped unanswered: the dialog is handed nothing.
            drop(deliver);
        }
        (Asker::ShellItemPosition { deliver }, other) => {
            tracing::warn!(
                reason = describe(&other),
                "the shell item's position was not read"
            );
            // Dropped unanswered: the dialog is handed nothing.
            drop(deliver);
        }
    }
}

#[cfg(test)]
mod tests {
    use crossbeam_channel::bounded;
    use verbatim_model::protocol::DumpedTree;
    use verbatim_model::request::CancelReason;
    use verbatim_model::{Backend, NodeDetails, NodeId, NodeSnapshot, OutpostId, Role, StateSet};

    use super::*;

    /// Answers the request asked under `token` of `outside` with
    /// `outcome`, as the reducer thread does at `QueueEffect::Answered`.
    fn answer(outside: &mut Outside, token: u64, outcome: OutsideOutcome) {
        if let Some(asker) = outside.answered(token) {
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

    /// A shell items outcome reaches the dialog, and a request whose
    /// outpost ended, answered "reader ended", gives it nothing.
    #[test]
    fn a_shell_items_answer_reaches_the_dialog_and_an_ended_outpost_gives_none() {
        let outcomes = std::sync::Arc::default();
        let mut outside = Outside::default();
        let answered = outside.begin(shell_items(&outcomes));
        let ended = outside.begin(shell_items(&outcomes));
        answer(
            &mut outside,
            answered,
            OutsideOutcome::Result(Reply::ShellItems(Vec::new())),
        );
        answer(&mut outside, ended, OutsideOutcome::ReaderEnded);
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
        let mut outside = Outside::default();
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
        let answered = outside.begin(asker(&outcomes));
        let failed = outside.begin(asker(&outcomes));
        answer(
            &mut outside,
            answered,
            OutsideOutcome::Result(Reply::ShellItemPosition(found.clone())),
        );
        answer(&mut outside, failed, OutsideOutcome::NotResponding);
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![Some(found), None]);
        assert!(outside.answered(answered).is_none(), "answered once");
    }

    /// A reply to a query of Core's reducer re-enters it as its answer,
    /// under the reply's trace and the reducer's own number; one to a query
    /// of the shell's, which asks none, goes nowhere.
    #[test]
    fn a_reducer_reply_goes_to_the_reducer_and_a_shell_reply_nowhere() {
        let trace_id = TraceId::from_raw(9);
        assert_eq!(
            reply_input(
                trace_id,
                RequestId::reducer(7),
                OutpostId(2),
                QueryOutcome::Gone,
                5
            ),
            Some(Input::Answered {
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
            None
        );
    }

    #[test]
    fn an_old_dump_answer_never_satisfies_a_newer_request() {
        let mut outside = Outside::default();
        let (old_tx, old_rx) = bounded(1);
        let old = outside.begin(dump(old_tx));
        // The first requester timed out and went away; a second asks.
        drop(old_rx);
        let (new_tx, new_rx) = bounded(1);
        let new = outside.begin(dump(new_tx));

        // The old request's late answer arrives first.
        answer(
            &mut outside,
            old,
            OutsideOutcome::NoLongerAwaited(CancelReason::Withdrawn),
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
        answer(
            &mut outside,
            new,
            OutsideOutcome::Result(Reply::Tree(Box::new(tree))),
        );
        let (root, _) = new_rx.try_recv().expect("answered").expect("a tree");
        assert_eq!(root.snapshot.name.as_deref(), Some("new"));
    }

    /// Giving up a tree dump finds the dump with that ticket and no other.
    #[test]
    fn a_dump_given_up_matches_only_its_ticket() {
        let mut outside = Outside::default();
        let (reply, _answer) = bounded(1);
        let ticket = DumpTicket::mint();
        let token = outside.begin(Asker::DumpTree { ticket, reply });
        assert_eq!(outside.dump(ticket), Some(token));
        assert_eq!(outside.dump(DumpTicket::mint()), None);
    }
}
