//! The request table: the single owner of "exactly one outcome per query"
//! (`docs/crates/verbatim-app.md`, `requests::RequestTable`).
//!
//! Every query Core sends an outpost is recorded here, on the reducer thread,
//! with a request id and the outpost generation it went to. The first
//! outcome removes the entry and goes to whoever asked; any later outcome
//! for the same id is dropped, and so is an outcome arriving from a
//! different outpost than the one asked. Core creates the outcome itself
//! when a query cannot be sent ("failed") and for every query still
//! outstanding when its outpost ends ("gone").

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_channel::Sender;
use verbatim_model::protocol::{FocusNow, QueryOutcome, QueryResult};
use verbatim_model::{
    FetchResult, Input, NodeSnapshot, NormalizedEvent, OutpostId, Pid, QueryId, QueryKind,
    TextReply, TraceId, TreeNode, WindowFacts,
};

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
    /// The tray or taskbar list dialog's request for the shell's items,
    /// whose outcome goes to `deliver`: the items, or `None` for any
    /// failure.
    ShellItems {
        /// Where the outcome goes.
        deliver: verbatim_gui::DeliverShellItems,
    },
    /// The list dialog's request for where the item clicked is now, whose
    /// outcome goes to `deliver`: the buttons that may be it, or `None` for
    /// any failure.
    ShellItemPosition {
        /// Where the outcome goes.
        deliver: verbatim_gui::DeliverShellItemPosition,
    },
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
    /// Whether the request only waits for the application to act, as a
    /// caret watch waits for a caret key's evidence: it does not keep
    /// Verbatim from being idle ([`RequestTable::outstanding`]).
    passive: bool,
    /// When Core ends the request itself if the outpost has not answered
    /// ([`RequestTable::expire`]); `None` for a request only its outpost or
    /// the outpost's end answers.
    deadline: Option<Instant>,
}

/// Outstanding queries, keyed by request id. Owned by the reducer thread.
#[derive(Default)]
pub(crate) struct RequestTable {
    next: u64,
    entries: HashMap<RequestId, Entry>,
    /// How many entries have a deadline, so the reducer thread's check for
    /// one costs nothing while none is outstanding.
    timed: usize,
}

impl RequestTable {
    /// Records a query about to be sent to `outpost` and returns its id.
    pub(crate) fn begin(&mut self, outpost: OutpostId, asker: Asker) -> RequestId {
        self.insert(outpost, asker, false, None)
    }

    /// Records a query about to be sent to `outpost` that Core ends itself,
    /// as failed, if no outcome has come by `deadline`
    /// ([`expire`](Self::expire)), and returns its id.
    pub(crate) fn begin_until(
        &mut self,
        outpost: OutpostId,
        asker: Asker,
        deadline: Instant,
    ) -> RequestId {
        self.insert(outpost, asker, false, Some(deadline))
    }

    /// Records a passive request about to be sent to `outpost`, one that
    /// only waits for the application to act, as a caret watch does, and
    /// returns its id: it gets its one outcome as any other does, but is not
    /// counted [`outstanding`](Self::outstanding), so an open watch, which a
    /// key that moved nothing leaves open until its bound, does not keep
    /// Verbatim from being idle.
    pub(crate) fn begin_passive(&mut self, outpost: OutpostId, asker: Asker) -> RequestId {
        self.insert(outpost, asker, true, None)
    }

    fn insert(
        &mut self,
        outpost: OutpostId,
        asker: Asker,
        passive: bool,
        deadline: Option<Instant>,
    ) -> RequestId {
        self.next += 1;
        let id = RequestId(self.next);
        self.timed += usize::from(deadline.is_some());
        self.entries.insert(
            id,
            Entry {
                outpost,
                asker,
                passive,
                deadline,
            },
        );
        id
    }

    /// Removes request `id`, keeping the count of those with a deadline.
    fn remove(&mut self, id: RequestId) -> Option<Entry> {
        let entry = self.entries.remove(&id)?;
        self.timed -= usize::from(entry.deadline.is_some());
        Some(entry)
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
        self.remove(id)
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
            .filter_map(|id| self.remove(id))
            .flat_map(|entry| deliver(entry.asker, QueryOutcome::Gone))
            .collect()
    }

    /// Ends every request whose deadline is `now` or earlier as failed,
    /// returning the reducer inputs that produces. A later answer from its
    /// outpost finds no request and is dropped.
    pub(crate) fn expire(&mut self, now: Instant) -> Vec<Input> {
        if self.timed == 0 {
            return Vec::new();
        }
        let expired: Vec<RequestId> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.deadline.is_some_and(|deadline| deadline <= now))
            .map(|(id, _)| *id)
            .collect();
        expired
            .into_iter()
            .filter_map(|id| self.remove(id))
            .flat_map(|entry| {
                deliver(
                    entry.asker,
                    QueryOutcome::Failed("no answer came by the request's deadline".to_owned()),
                )
            })
            .collect()
    }

    /// The earliest deadline of an outstanding request
    /// ([`begin_until`](Self::begin_until)).
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        if self.timed == 0 {
            return None;
        }
        self.entries
            .values()
            .filter_map(|entry| entry.deadline)
            .min()
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

    /// How many requests are outstanding, for the end-to-end harness's
    /// idle barrier: passive ones ([`begin_passive`](Self::begin_passive))
    /// are not counted.
    pub(crate) fn outstanding(&self) -> usize {
        self.entries.values().filter(|entry| !entry.passive).count()
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
        QueryOutcome::Done(_) => "the query returned an unexpected answer".to_owned(),
    }
}

/// Hands a shell item position query's outcome to the list dialog's
/// `deliver`: the buttons that may be the item, or `None` for any failure.
/// Nothing re-enters the reducer.
fn deliver_shell_item_position(
    deliver: verbatim_gui::DeliverShellItemPosition,
    outcome: QueryOutcome,
) -> Vec<Input> {
    match outcome {
        QueryOutcome::Done(QueryResult::ShellItemPosition(found)) => deliver(Some(found)),
        other => {
            tracing::warn!(
                reason = describe(&other),
                "the shell item's position was not read"
            );
            deliver(None);
        }
    }
    Vec::new()
}

/// Hands a shell items query's outcome to the list dialog's `deliver`:
/// the items, or `None` for any failure. Nothing re-enters the reducer.
fn deliver_shell_items(
    deliver: verbatim_gui::DeliverShellItems,
    outcome: QueryOutcome,
) -> Vec<Input> {
    match outcome {
        QueryOutcome::Done(QueryResult::ShellItems(items)) => deliver(Some(items)),
        other => {
            tracing::warn!(
                reason = describe(&other),
                "the shell's items were not listed"
            );
            deliver(None);
        }
    }
    Vec::new()
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
        Asker::ShellItems { deliver } => deliver_shell_items(deliver, outcome),
        Asker::ShellItemPosition { deliver } => deliver_shell_item_position(deliver, outcome),
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
                observed_at_us,
                ..
            })) => focus_inputs(
                source,
                trace_id,
                FocusNow {
                    window: None,
                    focus,
                    observed_at_us,
                },
            ),
            other => {
                tracing::warn!(reason = describe(&other), %source, "fake-focus query failed");
                Vec::new()
            }
        },
    }
}

/// The reducer inputs a focus-now answer becomes: a focus on the focused
/// control, with its ancestors, or, when the application has no focused
/// control, a foreground change to its window, the window then being the
/// focus. NVDA, starting, takes the system's focus and announces it with
/// its newly entered ancestors by the rules for any focus, the foreground
/// window among them only when it is presentable; it does not speak the
/// foreground window as a focus of its own. So the window is not reported
/// before a control: the control's focus in the system's foreground window
/// moves attention and the foreground with it, cuts speech off as a new
/// top-level window does, and announces the window only as an entered
/// ancestor (a desktop's Program Manager, a pane, says nothing, as NVDA's
/// captures of 2026-10-11 show). A control without window facts of its own
/// takes the window's. The same rules as live events then apply, so a
/// report of the focus the user already heard after a replacement is taken
/// silently.
fn focus_inputs(source: Pid, trace_id: TraceId, focus: FocusNow) -> Vec<Input> {
    let event = |node: NodeSnapshot,
                 window: Option<WindowFacts>,
                 foreground,
                 ancestors: Vec<NodeSnapshot>,
                 selected_child,
                 text| {
        Input::Event {
            trace_id,
            observed_at_us: focus.observed_at_us,
            source,
            backend: node.backend,
            window,
            event: NormalizedEvent::FocusChanged {
                node,
                foreground,
                ancestors,
                selected_child,
                text,
                earlier: None,
                group: None,
            },
        }
    };
    match (focus.focus, focus.window) {
        (Some(control), window) => vec![event(
            control.node,
            control.window.or(window.map(|(_, facts)| facts)),
            false,
            control.ancestors,
            control.selected_child,
            control.text,
        )],
        (None, Some((window, facts))) => {
            vec![event(window, Some(facts), true, Vec::new(), None, None)]
        }
        (None, None) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use crossbeam_channel::bounded;
    use verbatim_model::protocol::{DumpedTree, FocusedControl};
    use verbatim_model::{Backend, FocusText, NodeDetails, NodeId, Role, StateSet, WindowHandle};

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
    fn a_passive_request_is_not_outstanding_but_gets_its_outcome() {
        let mut table = RequestTable::default();
        let text = || Asker::Text {
            query_id: QueryId(4),
            trace_id: TraceId::mint(),
        };
        let watch = table.begin_passive(OutpostId(1), text());
        let read = table.begin(OutpostId(1), text());
        assert_eq!(table.outstanding(), 1, "the watch is not outstanding");
        assert_eq!(
            table.finish(watch, OutpostId(1), QueryOutcome::Gone).len(),
            1
        );
        assert_eq!(table.outstanding(), 1);
        assert_eq!(
            table.finish(read, OutpostId(1), QueryOutcome::Gone).len(),
            1
        );
        assert_eq!(table.outstanding(), 0);
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

    /// A shell items request whose outcomes are recorded in order.
    fn shell_items(
        outcomes: &std::sync::Arc<std::sync::Mutex<Vec<Option<Vec<verbatim_gui::ShellItem>>>>>,
    ) -> Asker {
        let kept = std::sync::Arc::clone(outcomes);
        Asker::ShellItems {
            deliver: Box::new(move |items| kept.lock().expect("outcomes").push(items)),
        }
    }

    #[test]
    fn a_shell_items_request_answered_before_its_deadline_is_not_expired() {
        let outcomes = std::sync::Arc::default();
        let mut table = RequestTable::default();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let id = table.begin_until(OutpostId(1), shell_items(&outcomes), deadline);
        assert_eq!(table.next_deadline(), Some(deadline));
        table.finish(
            id,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::ShellItems(Vec::new())),
        );
        assert_eq!(table.expire(deadline), [] as [verbatim_model::Input; 0]);
        assert_eq!(table.next_deadline(), None);
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![Some(Vec::new())]);
    }

    #[test]
    fn a_shell_items_request_unanswered_at_its_deadline_gets_nothing_and_its_late_answer_is_dropped()
     {
        let outcomes = std::sync::Arc::default();
        let mut table = RequestTable::default();
        let now = Instant::now();
        let deadline = now + std::time::Duration::from_secs(5);
        let id = table.begin_until(OutpostId(1), shell_items(&outcomes), deadline);
        table.begin(OutpostId(1), navigation(7));
        table.expire(now);
        assert_eq!(*outcomes.lock().expect("outcomes"), Vec::new());
        table.expire(deadline);
        table.finish(
            id,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::ShellItems(Vec::new())),
        );
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![None]);
        assert_eq!(table.len(), 1, "a request without a deadline is kept");
    }

    #[test]
    fn a_shell_item_position_answer_reaches_the_dialog_and_a_failure_gives_none() {
        let outcomes: std::sync::Arc<
            std::sync::Mutex<Vec<Option<verbatim_model::ShellCandidates>>>,
        > = std::sync::Arc::default();
        let mut table = RequestTable::default();
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let asker = |outcomes: &std::sync::Arc<std::sync::Mutex<Vec<_>>>| {
            let kept = std::sync::Arc::clone(outcomes);
            Asker::ShellItemPosition {
                deliver: Box::new(move |position| kept.lock().expect("outcomes").push(position)),
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
        let answered = table.begin_until(OutpostId(1), asker(&outcomes), deadline);
        let failed = table.begin_until(OutpostId(1), asker(&outcomes), deadline);
        table.finish(
            answered,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::ShellItemPosition(found.clone())),
        );
        table.finish(
            failed,
            OutpostId(1),
            QueryOutcome::Failed("no client".into()),
        );
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![Some(found), None]);
        assert_eq!(table.next_deadline(), None);
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
            locations: false,
        };
        table.finish(
            new,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::Tree(tree)),
        );
        let (root, _) = new_rx.try_recv().expect("answered").expect("a tree");
        assert_eq!(root.snapshot.name.as_deref(), Some("new"));
    }

    fn notepad_facts(under_active_window: Option<bool>) -> WindowFacts {
        WindowFacts {
            top_level: WindowHandle(0x10),
            root_owner: WindowHandle(0x10),
            topmost: false,
            under_active_window,
            in_foreground: true,
            class: "Notepad".to_owned(),
            application: String::new(),
            alt_held: false,
        }
    }

    /// A focus-now answer with a focused control is that control's focus
    /// alone, with its ancestors and what its text read found: the
    /// foreground window is not spoken as a focus of its own, as NVDA does
    /// not speak it when it starts, and is announced only as an entered
    /// ancestor when it is presentable.
    #[test]
    fn a_focus_now_answer_with_a_control_is_the_controls_focus_alone() {
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
        // The control's own window facts, told apart from the window's.
        let control_facts = notepad_facts(Some(true));
        let control = node(Role::Button, "OK");
        let ancestors = vec![node(Role::Pane, "Buttons")];
        let selected_child = Some(node(Role::ListItem, "First"));
        let answer = FocusNow {
            window: Some((window, notepad_facts(None))),
            focus: Some(FocusedControl {
                node: control.clone(),
                ancestors: ancestors.clone(),
                selected_child: selected_child.clone(),
                window: Some(control_facts.clone()),
                text: Some(Box::new(FocusText::NoText)),
            }),
            observed_at_us: 1234,
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
                observed_at_us: 1234,
                source: Pid(5),
                backend: Backend::Uia,
                window: Some(control_facts),
                event: NormalizedEvent::FocusChanged {
                    node: control,
                    foreground: false,
                    ancestors,
                    selected_child,
                    text: Some(Box::new(FocusText::NoText)),
                    earlier: None,
                    group: None,
                },
            }]
        );
    }

    /// With no focused control, the foreground window is the focus, a
    /// foreground change; a control without window facts of its own takes
    /// the foreground window's.
    #[test]
    fn a_focus_now_answer_without_a_control_is_the_windows_focus() {
        let mut table = RequestTable::default();
        let trace_id = TraceId::mint();
        let begin = |table: &mut RequestTable| {
            table.begin(
                OutpostId(1),
                Asker::FocusNow {
                    source: Pid(5),
                    trace_id,
                },
            )
        };
        let window = node(Role::Window, "Untitled - Notepad");
        let facts = notepad_facts(None);
        let id = begin(&mut table);
        let inputs = table.finish(
            id,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::Focus(FocusNow {
                window: Some((window.clone(), facts.clone())),
                focus: None,
                observed_at_us: 1234,
            })),
        );
        assert_eq!(
            inputs,
            [Input::Event {
                trace_id,
                observed_at_us: 1234,
                source: Pid(5),
                backend: Backend::Uia,
                window: Some(facts.clone()),
                event: NormalizedEvent::FocusChanged {
                    node: window.clone(),
                    foreground: true,
                    ancestors: Vec::new(),
                    selected_child: None,
                    text: None,
                    earlier: None,
                    group: None,
                },
            }]
        );

        let control = node(Role::Button, "OK");
        let id = begin(&mut table);
        let inputs = table.finish(
            id,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::Focus(FocusNow {
                window: Some((window, facts.clone())),
                focus: Some(FocusedControl {
                    node: control.clone(),
                    ancestors: Vec::new(),
                    selected_child: None,
                    window: None,
                    text: None,
                }),
                observed_at_us: 1234,
            })),
        );
        assert_eq!(
            inputs,
            [Input::Event {
                trace_id,
                observed_at_us: 1234,
                source: Pid(5),
                backend: Backend::Uia,
                window: Some(facts),
                event: NormalizedEvent::FocusChanged {
                    node: control,
                    foreground: false,
                    ancestors: Vec::new(),
                    selected_child: None,
                    text: None,
                    earlier: None,
                    group: None,
                },
            }]
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
                text: None,
            }),
            observed_at_us: 1234,
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
                observed_at_us: 1234,
                source: Pid(5),
                backend: Backend::Uia,
                window: None,
                event: NormalizedEvent::FocusChanged {
                    node: control,
                    foreground: false,
                    ancestors: Vec::new(),
                    selected_child: None,
                    text: None,
                    earlier: None,
                    group: None,
                },
            }]
        );
    }

    /// A focus-now answer, asked for at the start or for a fake focus,
    /// reaches Core with its stamp in microseconds, by
    /// which Core orders it against another application's focus.
    #[test]
    fn a_focus_now_answer_reaches_core_with_its_microseconds() {
        let mut table = RequestTable::default();
        let trace_id = TraceId::mint();
        let control = node(Role::Button, "OK");
        for asker in [
            Asker::FocusNow {
                source: Pid(5),
                trace_id,
            },
            Asker::FakeFocus {
                source: Pid(5),
                trace_id,
            },
        ] {
            let id = table.begin(OutpostId(1), asker);
            let answer = FocusNow {
                window: None,
                focus: Some(FocusedControl {
                    node: control.clone(),
                    ancestors: Vec::new(),
                    selected_child: None,
                    window: None,
                    text: None,
                }),
                observed_at_us: 1_234_567,
            };
            let inputs = table.finish(
                id,
                OutpostId(1),
                QueryOutcome::Done(QueryResult::Focus(answer)),
            );
            let stamps: Vec<_> = inputs
                .iter()
                .map(|input| match input {
                    Input::Event { observed_at_us, .. } => *observed_at_us,
                    other => panic!("not a focus: {other:?}"),
                })
                .collect();
            assert_eq!(stamps, [1_234_567]);
        }
    }
}
