//! Who asks the shell's own queries of outposts, and where each outcome
//! goes (`docs/crates/verbatim-app.md`, `requests`).
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
use verbatim_model::protocol::{
    FocusNow, QueryOutcome, QueryResult, RequestId, Requester, Responder,
};
use verbatim_model::{
    Input, NodeSnapshot, NormalizedEvent, OutpostId, Pid, QueryId, TraceId, TreeNode, WindowFacts,
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
        QueryOutcome::Abandoned => "the query passed its deadline".to_owned(),
        QueryOutcome::Done(_) => "the query returned an unexpected answer".to_owned(),
    }
}

/// Hands a shell item position query's outcome to the list dialog's
/// `deliver`: the buttons that may be the item, or `None` for any failure.
/// Nothing re-enters the reducer.
fn deliver_shell_item_position(
    deliver: Responder<verbatim_model::ShellCandidates>,
    outcome: QueryOutcome,
) -> Vec<Input> {
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
    Vec::new()
}

/// Hands a shell items query's outcome to the list dialog's `deliver`:
/// the items, or `None` for any failure. Nothing re-enters the reducer.
fn deliver_shell_items(
    deliver: Responder<Vec<verbatim_gui::ShellItem>>,
    outcome: QueryOutcome,
) -> Vec<Input> {
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
    Vec::new()
}

/// Hands one outcome to its asker, returning the reducer inputs it
/// produces: an answer to a dialog or the control plane is given here, and
/// produces none.
pub(crate) fn deliver(asker: Asker, outcome: QueryOutcome) -> Vec<Input> {
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
    use verbatim_model::{
        Backend, FocusText, NodeDetails, NodeId, OutpostId, Role, StateSet, WindowHandle,
    };

    use super::*;

    /// Ends request `id` of `table` with `outcome`, from `outpost`, and
    /// carries the outcome out, as the reducer thread does.
    fn finish(
        table: &mut RequestTable,
        id: verbatim_model::protocol::RequestId,
        outpost: OutpostId,
        outcome: QueryOutcome,
    ) -> Vec<Input> {
        table
            .finish(id, outpost, outcome)
            .map(|(asker, outcome)| deliver(asker, outcome))
            .unwrap_or_default()
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

    /// A shell items outcome reaches the dialog, and a request ended at its
    /// deadline, failed, gives it nothing and is named with its outpost to
    /// be withdrawn.
    #[test]
    fn a_shell_items_answer_reaches_the_dialog_and_a_passed_deadline_gives_none() {
        let outcomes = std::sync::Arc::default();
        let mut table = RequestTable::default();
        let answered = table.begin_until(OutpostId(1), shell_items(&outcomes), 5_000_000);
        let expired = table.begin_until(OutpostId(1), shell_items(&outcomes), 5_000_000);
        finish(
            &mut table,
            answered,
            OutpostId(1),
            QueryOutcome::Done(QueryResult::ShellItems(Vec::new())),
        );
        let ended = table.expire(5_000_000);
        assert_eq!(
            ended
                .iter()
                .map(|ended| (ended.id, ended.outpost))
                .collect::<Vec<_>>(),
            [(expired, OutpostId(1))]
        );
        for ended in ended {
            assert_eq!(deliver(ended.asker, ended.outcome), [] as [Input; 0]);
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
        let deadline = 5_000_000;
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
        let answered = table.begin_until(OutpostId(1), asker(&outcomes), deadline);
        let failed = table.begin_until(OutpostId(1), asker(&outcomes), deadline);
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
        assert_eq!(table.next_deadline(), None);
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
                QueryOutcome::Gone
            ),
            Routed::Reducer(Input::Answered {
                trace_id,
                query_id: QueryId(7),
                outpost: OutpostId(2),
                outcome: QueryOutcome::Gone,
            })
        );
        assert_eq!(
            reply_input(
                trace_id,
                RequestId::shell(7),
                OutpostId(2),
                QueryOutcome::Gone
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
        let inputs = finish(
            &mut table,
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
        let inputs = finish(
            &mut table,
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
        let inputs = finish(
            &mut table,
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
        let inputs = finish(
            &mut table,
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
            let inputs = finish(
                &mut table,
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
