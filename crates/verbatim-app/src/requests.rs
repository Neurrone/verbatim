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
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::Sender;
use verbatim_model::{
    FetchResult, Input, NodeSnapshot, NormalizedEvent, OutpostId, Pid, QueryId, QueryKind,
    TextReply, TraceId, TreeNode, WindowFacts,
};
use verbatim_outpost::protocol::{FocusNow, QueryOutcome, QueryResult};

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
    /// An activation: the reducer does not wait for it, but it gets exactly
    /// one outcome, which it speaks.
    Activation {
        /// The command's trace.
        trace_id: TraceId,
    },
    /// A control-plane tree dump, answered on `reply`. The channel is
    /// bounded with room for the one answer, so sending never blocks; a
    /// requester that already gave up has dropped its receiver.
    DumpTree {
        /// The requester's name for the dump.
        ticket: DumpTicket,
        /// Where the answer goes.
        reply: Sender<DumpTreeResult>,
    },
    /// The shell's focus-now query, at startup or after an outpost or the
    /// listener was replaced: its answer re-enters the reducer as a
    /// foreground change and a focus, from application `source`.
    FocusNow { source: Pid, trace_id: TraceId },
    /// The focus-now query sent when a menu or the Alt+Tab switcher closed
    /// and no focus event followed: only the focused control re-enters the
    /// reducer, as NVDA's fake focus queues a focus on it and nothing for
    /// its window.
    FakeFocus { source: Pid, trace_id: TraceId },
    /// The reducer's text request (`Effect::Text`): its outcome re-enters
    /// the reducer as `Input::TextCompleted` under the reducer's own query
    /// id, a failure of any kind as the protocol's "unanswered".
    Text {
        /// The reducer's id for the request.
        query_id: QueryId,
        /// The trace the request belongs to.
        trace_id: TraceId,
    },
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

    /// The outstanding tree dump named `ticket`, and the outpost it went
    /// to: the request a control-plane caller who stopped waiting for it can
    /// withdraw.
    pub(crate) fn dump_tree_request(&self, ticket: DumpTicket) -> Option<(RequestId, OutpostId)> {
        self.entries
            .iter()
            .find_map(|(id, entry)| match &entry.asker {
                Asker::DumpTree { ticket: asked, .. } if *asked == ticket => {
                    Some((*id, entry.outpost))
                }
                _ => None,
            })
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
                QueryOutcome::Gone => FetchResult::Gone,
                other => {
                    tracing::warn!(reason = describe(&other), "a navigation query failed");
                    FetchResult::Unanswered
                }
            };
            vec![Input::FetchCompleted {
                trace_id,
                query_id,
                kind,
                result,
            }]
        }
        Asker::Activation { trace_id } => {
            let (activated, action) = match outcome {
                QueryOutcome::Done(QueryResult::Activated(action)) => (true, action),
                // The activation started and passed its deadline: whether it
                // happened is unknown, so nothing is said either way.
                // Whatever it caused, such as a dialog taking focus,
                // announces itself.
                QueryOutcome::Abandoned => {
                    tracing::warn!(
                        "activation passed its deadline; whether it happened is unknown"
                    );
                    return Vec::new();
                }
                other => {
                    tracing::warn!(reason = describe(&other), "activation did not complete");
                    (false, None)
                }
            };
            vec![Input::ActivationCompleted {
                trace_id,
                activated,
                action,
            }]
        }
        Asker::DumpTree { reply, .. } => {
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
        Asker::Text { query_id, trace_id } => {
            let reply = match outcome {
                QueryOutcome::Done(QueryResult::Text(reply)) => reply,
                QueryOutcome::Gone => TextReply::Gone,
                other => {
                    tracing::debug!(reason = describe(&other), "a text request was not answered");
                    TextReply::Unanswered
                }
            };
            vec![Input::TextCompleted {
                trace_id,
                query_id,
                reply,
            }]
        }
        Asker::FakeFocus { source, trace_id } => match outcome {
            QueryOutcome::Done(QueryResult::Focus(FocusNow {
                focus,
                observed_at_ms,
                ..
            })) => focus_inputs(
                source,
                trace_id,
                FocusNow {
                    window: None,
                    focus,
                    observed_at_ms,
                },
            ),
            other => {
                tracing::warn!(reason = describe(&other), %source, "fake-focus query failed");
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
                observed_at_ms: focus.observed_at_ms,
                source,
                backend: node.backend,
                window,
                event: NormalizedEvent::FocusChanged {
                    node,
                    foreground,
                    ancestors,
                    ancestors_unknown: false,
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
    use verbatim_model::{Backend, NodeDetails, NodeId, Role, StateSet, WindowHandle};
    use verbatim_outpost::protocol::{DumpedTree, FocusedControl};

    use super::*;

    fn navigation(query: u64) -> Asker {
        Asker::Reducer {
            query_id: QueryId(query),
            kind: QueryKind::Parent,
            trace_id: TraceId::mint(),
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
        assert_eq!(
            table.finish(id, OutpostId(1), QueryOutcome::Gone),
            [] as [verbatim_model::Input; 0]
        );
    }

    #[test]
    fn an_outcome_from_another_outpost_is_dropped() {
        let mut table = RequestTable::default();
        let id = table.begin(OutpostId(1), navigation(7));
        assert_eq!(
            table.finish(id, OutpostId(2), QueryOutcome::Gone),
            [] as [verbatim_model::Input; 0]
        );
        assert_eq!(table.len(), 1, "the real answer can still arrive");
    }

    #[test]
    fn an_unanswered_navigation_reads_as_unanswered_and_only_a_gone_node_as_gone() {
        for outcome in [
            QueryOutcome::Abandoned,
            QueryOutcome::NotStarted,
            QueryOutcome::Failed("the read failed".to_owned()),
        ] {
            let mut table = RequestTable::default();
            let id = table.begin(OutpostId(1), navigation(7));
            assert!(matches!(
                table.finish(id, OutpostId(1), outcome).as_slice(),
                [Input::FetchCompleted {
                    result: FetchResult::Unanswered,
                    ..
                }]
            ));
        }
        let mut table = RequestTable::default();
        let id = table.begin(OutpostId(1), navigation(7));
        assert!(matches!(
            table
                .finish(id, OutpostId(1), QueryOutcome::Gone)
                .as_slice(),
            [Input::FetchCompleted {
                result: FetchResult::Gone,
                ..
            }]
        ));
    }

    #[test]
    fn an_activation_outcome_reaches_the_reducer() {
        for (outcome, expected) in [
            (QueryOutcome::Done(QueryResult::Activated(None)), true),
            (QueryOutcome::Failed("no action".to_owned()), false),
            (QueryOutcome::NotStarted, false),
            (QueryOutcome::Gone, false),
        ] {
            let mut table = RequestTable::default();
            let id = table.begin(
                OutpostId(1),
                Asker::Activation {
                    trace_id: TraceId::mint(),
                },
            );
            assert!(matches!(
                table.finish(id, OutpostId(1), outcome).as_slice(),
                [Input::ActivationCompleted { activated, .. }] if *activated == expected
            ));
        }
    }

    #[test]
    fn an_activation_that_passed_its_deadline_is_not_reported_either_way() {
        let mut table = RequestTable::default();
        let id = table.begin(
            OutpostId(1),
            Asker::Activation {
                trace_id: TraceId::mint(),
            },
        );
        assert_eq!(
            table.finish(id, OutpostId(1), QueryOutcome::Abandoned),
            [] as [verbatim_model::Input; 0]
        );
        assert_eq!(
            table.len(),
            0,
            "the abandoned outcome is still its one outcome"
        );
    }

    #[test]
    fn a_text_reply_reaches_the_reducer_and_any_failure_reads_as_unanswered() {
        let text = |outcome| {
            let mut table = RequestTable::default();
            let id = table.begin(
                OutpostId(1),
                Asker::Text {
                    query_id: QueryId(4),
                    trace_id: TraceId::mint(),
                },
            );
            match table.finish(id, OutpostId(1), outcome).as_slice() {
                [
                    Input::TextCompleted {
                        query_id: QueryId(4),
                        reply,
                        ..
                    },
                ] => reply.clone(),
                other => panic!("one text completion, not {other:?}"),
            }
        };
        assert_eq!(
            text(QueryOutcome::Done(QueryResult::Text(TextReply::NoText))),
            TextReply::NoText
        );
        assert_eq!(text(QueryOutcome::Gone), TextReply::Gone);
        for failure in [
            QueryOutcome::Abandoned,
            QueryOutcome::NotStarted,
            QueryOutcome::Failed("the read failed".to_owned()),
        ] {
            assert_eq!(text(failure), TextReply::Unanswered);
        }
    }

    #[test]
    fn a_dump_request_is_found_by_its_ticket_and_by_nothing_else() {
        let mut table = RequestTable::default();
        let (ticket, reply) = (DumpTicket::mint(), bounded(1).0);
        let first = table.begin(OutpostId(1), Asker::DumpTree { ticket, reply });
        table.begin(OutpostId(2), dump(bounded(1).0));
        table.begin(OutpostId(1), navigation(7));

        assert_eq!(table.dump_tree_request(ticket), Some((first, OutpostId(1))));
        table.finish(first, OutpostId(1), QueryOutcome::NotStarted);
        assert_eq!(table.dump_tree_request(ticket), None);
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
        let old = table.begin(OutpostId(1), dump(old_tx));
        // The first requester timed out and went away; a second asks.
        drop(old_rx);
        let (new_tx, new_rx) = bounded(1);
        let new = table.begin(OutpostId(1), dump(new_tx));

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
        let trace_id = TraceId::mint();
        let id = table.begin(
            OutpostId(1),
            Asker::FocusNow {
                source: Pid(5),
                trace_id,
            },
        );
        let window = node(Role::Window, "Untitled - Notepad");
        let window_facts = WindowFacts {
            top_level: WindowHandle(0x10),
            root_owner: WindowHandle(0x10),
            topmost: false,
            under_active_window: None,
            in_foreground: true,
        };
        // The control's own window facts, told apart from the window's.
        let control_facts = WindowFacts {
            top_level: WindowHandle(0x10),
            root_owner: WindowHandle(0x10),
            topmost: false,
            under_active_window: Some(true),
            in_foreground: true,
        };
        let control = node(Role::Button, "OK");
        let ancestors = vec![node(Role::Pane, "Buttons")];
        let selected_child = Some(node(Role::ListItem, "First"));
        let answer = FocusNow {
            window: Some((window.clone(), window_facts)),
            focus: Some(FocusedControl {
                node: control.clone(),
                ancestors: ancestors.clone(),
                selected_child: selected_child.clone(),
                window: Some(control_facts),
            }),
            observed_at_ms: 1234,
        };
        let inputs = table.finish(
            id,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::Focus(answer)),
        );
        assert_eq!(
            inputs,
            [
                Input::Event {
                    trace_id,
                    observed_at_ms: 1234,
                    source: Pid(5),
                    backend: Backend::Uia,
                    window: Some(window_facts),
                    event: NormalizedEvent::FocusChanged {
                        node: window,
                        foreground: true,
                        ancestors: Vec::new(),
                        ancestors_unknown: false,
                        selected_child: None,
                    },
                },
                Input::Event {
                    trace_id,
                    observed_at_ms: 1234,
                    source: Pid(5),
                    backend: Backend::Uia,
                    window: Some(control_facts),
                    event: NormalizedEvent::FocusChanged {
                        node: control,
                        foreground: false,
                        ancestors,
                        ancestors_unknown: false,
                        selected_child,
                    },
                },
            ]
        );
    }

    #[test]
    fn a_focus_now_answer_outside_the_foreground_is_only_a_focus() {
        let mut table = RequestTable::default();
        let trace_id = TraceId::mint();
        let id = table.begin(
            OutpostId(1),
            Asker::FocusNow {
                source: Pid(5),
                trace_id,
            },
        );
        let control = node(Role::Button, "OK");
        let answer = FocusNow {
            window: None,
            focus: Some(FocusedControl {
                node: control.clone(),
                ancestors: Vec::new(),
                selected_child: None,
                window: None,
            }),
            observed_at_ms: 1234,
        };
        let inputs = table.finish(
            id,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::Focus(answer)),
        );
        assert_eq!(
            inputs,
            [Input::Event {
                trace_id,
                observed_at_ms: 1234,
                source: Pid(5),
                backend: Backend::Uia,
                window: None,
                event: NormalizedEvent::FocusChanged {
                    node: control,
                    foreground: false,
                    ancestors: Vec::new(),
                    ancestors_unknown: false,
                    selected_child: None,
                },
            }]
        );
    }
}
