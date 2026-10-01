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
use verbatim_model::{
    FetchResult, Input, NodeSnapshot, NormalizedEvent, OutpostId, Pid, QueryId, QueryKind, TraceId,
    TreeNode, WindowFacts,
};
use verbatim_outpost::protocol::{FocusNow, QueryOutcome, QueryResult};

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
    /// The shell's focus-now query, at startup or after an outpost or the
    /// listener was replaced: its answer re-enters the reducer as a
    /// foreground change and a focus, from application `source`.
    FocusNow { source: Pid, trace_id: TraceId },
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
    /// the reducer inputs it produces. Does nothing for an id that already
    /// has its outcome, or for an outcome from an outpost other than the one
    /// asked.
    pub(crate) fn finish(
        &mut self,
        id: RequestId,
        outpost: OutpostId,
        outcome: QueryOutcome,
    ) -> Vec<Input> {
        if self
            .entries
            .get(&id)
            .is_none_or(|entry| entry.outpost != outpost)
        {
            return Vec::new();
        }
        self.entries
            .remove(&id)
            .map(|entry| deliver(entry.asker, outcome))
            .unwrap_or_default()
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
            .filter_map(|id| self.entries.remove(&id))
            .flat_map(|entry| deliver(entry.asker, QueryOutcome::Gone))
            .collect()
    }

    /// How many requests are outstanding.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// A short description of an outcome other than success, for logs and
/// control-plane errors.
fn describe(outcome: &QueryOutcome) -> String {
    match outcome {
        QueryOutcome::Gone => "the node or its outpost is gone".to_owned(),
        QueryOutcome::Failed(reason) => reason.clone(),
        QueryOutcome::NotStarted => "the query was withdrawn before it started".to_owned(),
        QueryOutcome::Abandoned => "the query passed its deadline".to_owned(),
        _ => "the query returned an unexpected answer".to_owned(),
    }
}

/// Hands one outcome to its asker.
fn deliver(asker: Asker, outcome: QueryOutcome) -> Vec<Input> {
    match asker {
        Asker::Reducer {
            query_id,
            kind,
            trace_id,
        } => {
            let result = match outcome {
                QueryOutcome::Done(QueryResult::Navigated(Some(node))) => FetchResult::Node(node),
                QueryOutcome::Done(QueryResult::Navigated(None)) => FetchResult::NoNeighbor,
                other => {
                    if !matches!(other, QueryOutcome::Gone) {
                        tracing::warn!(reason = describe(&other), "a navigation query failed");
                    }
                    FetchResult::Gone
                }
            };
            vec![Input::FetchCompleted {
                trace_id,
                query_id,
                kind,
                result,
            }]
        }
        Asker::Activation => {
            if !matches!(outcome, QueryOutcome::Done(QueryResult::Activated)) {
                tracing::warn!(reason = describe(&outcome), "activation did not complete");
            }
            Vec::new()
        }
        Asker::DumpTree(reply) => {
            let answer = match outcome {
                QueryOutcome::Done(QueryResult::Tree(dumped)) => {
                    Ok((dumped.root, dumped.truncated))
                }
                other => Err(describe(&other)),
            };
            // The requester may have timed out and dropped its receiver.
            let _ = reply.try_send(answer);
            Vec::new()
        }
        Asker::FocusNow { source, trace_id } => match outcome {
            QueryOutcome::Done(QueryResult::Focus(focus)) => focus_inputs(source, trace_id, focus),
            other => {
                tracing::warn!(reason = describe(&other), %source, "focus-now query failed");
                Vec::new()
            }
        },
    }
}

/// The reducer inputs a focus-now answer becomes: a foreground change to the
/// window, when the application holds the foreground, then a focus on the
/// focused control. The same rules as live events then apply, so a report of
/// the focus the user already heard after a replacement is taken silently.
fn focus_inputs(source: Pid, trace_id: TraceId, focus: FocusNow) -> Vec<Input> {
    let event =
        |node: NodeSnapshot, window: Option<WindowFacts>, foreground, ancestors, selected_child| {
            Input::Event {
                trace_id,
                observed_at_ms: 0,
                source,
                backend: node.backend,
                window,
                event: NormalizedEvent::FocusChanged {
                    node,
                    foreground,
                    ancestors,
                    selected_child,
                },
            }
        };
    let mut inputs = Vec::new();
    if let Some((window, facts)) = focus.window {
        inputs.push(event(window, Some(facts), true, Vec::new(), None));
    }
    if let Some(control) = focus.focus {
        inputs.push(event(
            control.node,
            control.window,
            false,
            control.ancestors,
            control.selected_child,
        ));
    }
    inputs
}

#[cfg(test)]
mod tests {
    use crossbeam_channel::bounded;
    use verbatim_model::{Backend, NodeDetails, NodeId, Role, StateSet};
    use verbatim_outpost::protocol::{DumpedTree, FocusedControl};

    use super::*;

    fn navigation(query: u64) -> Asker {
        Asker::Reducer {
            query_id: QueryId(query),
            kind: QueryKind::Parent,
            trace_id: TraceId::mint(),
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

    #[test]
    fn the_first_outcome_wins_and_a_second_is_dropped() {
        let mut table = RequestTable::default();
        let id = table.begin(OutpostId(1), navigation(7));

        let first = table.finish(
            id,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::Navigated(None)),
        );
        assert!(matches!(
            first.as_slice(),
            [Input::FetchCompleted {
                query_id: QueryId(7),
                result: FetchResult::NoNeighbor,
                ..
            }]
        ));
        assert!(
            table
                .finish(id, OutpostId(1), QueryOutcome::Gone)
                .is_empty()
        );
    }

    #[test]
    fn an_outcome_from_another_outpost_is_dropped() {
        let mut table = RequestTable::default();
        let id = table.begin(OutpostId(1), navigation(7));
        assert!(
            table
                .finish(id, OutpostId(2), QueryOutcome::Gone)
                .is_empty()
        );
        assert_eq!(table.len(), 1, "the real answer can still arrive");
    }

    #[test]
    fn an_abandoned_or_unstarted_navigation_reads_as_gone() {
        for outcome in [QueryOutcome::Abandoned, QueryOutcome::NotStarted] {
            let mut table = RequestTable::default();
            let id = table.begin(OutpostId(1), navigation(7));
            assert!(matches!(
                table.finish(id, OutpostId(1), outcome).as_slice(),
                [Input::FetchCompleted {
                    result: FetchResult::Gone,
                    ..
                }]
            ));
        }
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
        table.finish(old, OutpostId(1), QueryOutcome::Failed("old".to_owned()));
        assert!(new_rx.try_recv().is_err(), "the newer request is untouched");

        let tree = DumpedTree {
            root: TreeNode {
                snapshot: node(Role::Window, "new"),
                children: Vec::new(),
            },
            truncated: false,
        };
        table.finish(
            new,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::Tree(tree)),
        );
        let (root, _) = new_rx.try_recv().expect("answered").expect("a tree");
        assert_eq!(root.snapshot.name.as_deref(), Some("new"));
    }

    #[test]
    fn a_focus_now_answer_becomes_a_foreground_change_then_a_focus() {
        let mut table = RequestTable::default();
        let id = table.begin(
            OutpostId(1),
            Asker::FocusNow {
                source: Pid(5),
                trace_id: TraceId::mint(),
            },
        );
        let answer = FocusNow {
            window: None,
            focus: Some(FocusedControl {
                node: node(Role::Button, "OK"),
                ancestors: Vec::new(),
                selected_child: None,
                window: None,
            }),
        };
        let inputs = table.finish(
            id,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::Focus(answer)),
        );
        assert!(matches!(
            inputs.as_slice(),
            [Input::Event {
                source: Pid(5),
                event: NormalizedEvent::FocusChanged {
                    foreground: false,
                    ..
                },
                ..
            }]
        ));
    }
}
