//! The reducer's replay tests (`books/dev/src/explanation/design/ownership.md`): recorded inputs given to the reducer, with its
//! asks answered as the shell answers them, and the effects it answers with asserted. Each replays one
//! ordering the outpost's threads can produce, at the times given, so an
//! ordering that once needed a real bound, a real deadline or a race to
//! reproduce is one test here.

use std::collections::BTreeSet;
use std::time::Duration;

use verbatim_model::protocol::{
    DeliveredFact, EventTiming, OutpostToCore, Query, QueryOutcome, QueryResult, WinEventStamp,
};
use verbatim_model::{
    Backend, CallCounts, CaretMotion, CaretRead, KeyFetch, NodeId, NodeSnapshot, NormalizedEvent,
    ReadPurpose, Role, StateSet, TextOp, TextReply, TextUnit, TraceId,
};

use super::*;

mod caret_lease;
mod explore;
mod intake_order;
mod reducer;

/// The answers the shell gives the reducer's asks, as a test records them:
/// whether the user has moved on, whether the window checked is hung, and
/// the window in front with whether it is hung.
#[derive(Clone, Default)]
struct Answers {
    moved_on: bool,
    /// Whether the window checked is found hung.
    hung: bool,
    /// A window that answers whatever `hung` says, for a test of two
    /// windows, one hung.
    answering: Option<Window>,
    /// The window the program in flight's worker named, which a check of a
    /// program with no window at dispatch reads.
    named: Option<Window>,
    /// The window of the outpost's application in front, and whether it is
    /// hung, for a front check; `None` for another application's.
    front: Option<(Window, bool)>,
}

impl Answers {
    /// The answers to `effect`, if it asks, in the order the shell gives
    /// them.
    fn to(&mut self, effect: &Effect) -> Vec<Input> {
        let judged_hung = |window| self.hung && self.answering != Some(window);
        match effect {
            Effect::JudgeTold { window, waiting } => vec![Input::ToldJudged {
                window: *window,
                waiting: *waiting,
                hung: judged_hung(*window),
            }],
            Effect::CheckHung { window } => judged_hung(*window)
                .then_some(Input::HangSeen {
                    window: *window,
                    request: None,
                })
                .into_iter()
                .collect(),
            Effect::AskJudge { request, window } => {
                let Some(window) = window.or(self.named) else {
                    return vec![Input::Judged {
                        request: *request,
                        moved_on: false,
                    }];
                };
                let hang = judged_hung(window).then_some(Input::HangSeen {
                    window,
                    request: Some(*request),
                });
                hang.into_iter()
                    .chain([Input::Judged {
                        request: *request,
                        moved_on: self.moved_on,
                    }])
                    .collect()
            }
            Effect::CheckFront => match self.front {
                Some((window, true)) => vec![Input::HangSeen {
                    window,
                    request: None,
                }],
                Some((window, false)) => vec![Input::FrontReadable { window }],
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }
}

/// Gives `effects` to `answers` as the shell carries them out: each ask is
/// answered at once by the reducer's next input, at `now`, and what that
/// asks follows it in place, so the effects come back in the order the
/// shell carries them out, the asks among them.
fn with_answers(
    reducer: &mut Reducer,
    answers: &mut Answers,
    effects: Vec<Effect>,
    now: Now,
) -> Vec<Effect> {
    let mut out = Vec::new();
    for effect in effects {
        let answered = answers.to(&effect);
        out.push(effect);
        for answer in answered {
            let more = reducer.input(answer, now);
            out.extend(with_answers(reducer, answers, more, now));
        }
    }
    out
}

/// The reducer under test with the answers its asks are given.
struct Replay {
    reducer: Reducer,
    answers: Answers,
}

impl Replay {
    fn new() -> Self {
        Self {
            reducer: Reducer::new(),
            answers: Answers::default(),
        }
    }

    fn input(&mut self, input: Input, at_ms: u64) -> Vec<Effect> {
        let effects = self.reducer.input(input, now(at_ms));
        with_answers(&mut self.reducer, &mut self.answers, effects, now(at_ms))
    }

    fn tick(&mut self, at_ms: u64) -> Vec<Effect> {
        let effects = self.reducer.tick(now(at_ms));
        with_answers(&mut self.reducer, &mut self.answers, effects, now(at_ms))
    }

    /// Pushes `item`, observed at `at_ms`, from `source`.
    fn push(&mut self, item: Item, source: Source, at_ms: u64) -> Vec<Effect> {
        self.input(
            Input::Entry {
                entry: entry(item, at_ms),
                source,
            },
            at_ms,
        )
    }

    /// Ends `request`'s program at `at_ms`, reporting or not.
    fn finish(&mut self, request: Request, reported: bool, at_ms: u64) -> Vec<Effect> {
        self.input(
            Input::Finished {
                request,
                reported,
                panicked: false,
                cut: None,
            },
            at_ms,
        )
    }

    /// Commits `change` for `request` at `at_ms`.
    fn commit(&mut self, request: Request, change: Change, at_ms: u64) -> Vec<Effect> {
        self.input(
            Input::Commit {
                request,
                change: Box::new(change),
            },
            at_ms,
        )
    }
}

/// Now, `at_ms` into the outpost's life on every clock.
fn now(at_ms: u64) -> Now {
    Now {
        at: Duration::from_millis(at_ms),
        observed_us: at_ms * 1_000,
        wall_us: at_ms * 1_000,
    }
}

/// `item` observed at `at_ms`.
fn entry(item: Item, at_ms: u64) -> Entry {
    Entry {
        item,
        trace: TraceId::from_raw(at_ms),
        observed_at_us: at_ms * 1_000,
        timing: EventTiming::default(),
        front: crate::Front::Untested,
    }
}

/// An MSAA focus on child `child` of window 9, raised at `time` by thread 3.
fn msaa_focus(child: i32, time: u32) -> Item {
    Item::Fact(
        DeliveredFact::MsaaFocus {
            hwnd: 9,
            id_object: -4,
            id_child: child,
        },
        Some(WinEventStamp { thread: 3, time }),
    )
}

/// A name change of child `child` of window 9.
fn name_change(child: i32) -> Item {
    Item::Msaa {
        kind: WinKind::NameChange,
        hwnd: 9,
        id_object: -4,
        id_child: child,
        raiser: 3,
    }
}

/// A focus-now query, request `request_id`.
fn focus_now(request_id: u64) -> Item {
    Item::Query {
        request_id: RequestId::shell(request_id),
        query: Query::ReadFocus {
            purpose: verbatim_model::protocol::FocusPurpose::Plain,
        },
    }
}

/// The programs `effects` dispatch, in order.
fn programs(effects: &[Effect]) -> Vec<&Program> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Dispatch(program) => Some(&**program),
            _ => None,
        })
        .collect()
}

/// The one program `effects` dispatch.
fn program(effects: &[Effect]) -> &Program {
    let programs = programs(effects);
    assert_eq!(programs.len(), 1, "one program dispatched");
    programs[0]
}

/// The observation, in milliseconds, of the entry `program` handles, and
/// its place in a focus group.
fn handles(program: &Program) -> (u64, Option<u32>) {
    match &program.work {
        Work::Entry { entry, group, .. } => {
            (entry.observed_at_us / 1_000, group.map(|group| group.rank))
        }
        work => panic!("a program for an entry, not {work:?}"),
    }
}

/// The messages `effects` send, in order.
fn sent(effects: &[Effect]) -> Vec<&OutpostToCore> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Send(message) => Some(&**message),
            _ => None,
        })
        .collect()
}

/// The outcomes of the replies `effects` send, by request.
fn replies(effects: &[Effect]) -> Vec<(u64, &'static str)> {
    sent(effects)
        .into_iter()
        .filter_map(|message| match message {
            OutpostToCore::Reply {
                request_id,
                outcome,
                ..
            } => Some((request_id.number, outcome_name(outcome))),
            _ => None,
        })
        .collect()
}

fn outcome_name(outcome: &QueryOutcome) -> &'static str {
    match outcome {
        QueryOutcome::Done(_) => "done",
        QueryOutcome::Abandoned => "abandoned",
        QueryOutcome::NotStarted => "not started",
        QueryOutcome::Failed(_) => "failed",
        QueryOutcome::Gone => "gone",
    }
}

/// The reply to query `request_id`, done.
fn done(request_id: u64) -> OutpostToCore {
    OutpostToCore::Reply {
        trace_id: TraceId::from_raw(1),
        request_id: RequestId::shell(request_id),
        outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Done)),
        timing: EventTiming::default(),
    }
}

/// A node `id` of `role`.
fn node(id: u64, role: Role) -> NodeSnapshot {
    NodeSnapshot {
        id: NodeId::new(id),
        backend: Backend::Msaa,
        role,
        name: None,
        value: None,
        states: StateSet::new(),
        details: verbatim_model::NodeDetails::default(),
    }
}

/// The caret a terminal's report was read with, which its first read is
/// owed with.
fn reported_caret() -> verbatim_model::CaretReport {
    verbatim_model::CaretReport {
        line: verbatim_model::TextChunk {
            unit: TextUnit::Line,
            text: String::new(),
            start: verbatim_model::TextAnchor(1),
            offset: 0,
            languages: Vec::new(),
            first: true,
            last: true,
            truncated: false,
            formats: Vec::new(),
        },
        selection: None,
        read_at_us: 1,
    }
}

/// The commit that reports node `id` as the focus and follows it, owing a
/// terminal's first read when `owes`.
fn focus_change(id: u64, owes: bool) -> Change {
    let focus = node(id, if owes { Role::Terminal } else { Role::Button });
    Change {
        result: Some(OutpostToCore::Event {
            trace_id: TraceId::from_raw(2),
            observed_at_us: 0,
            backend: Backend::Msaa,
            window: None,
            application: None,
            timing: EventTiming::default(),
            event: NormalizedEvent::FocusChanged {
                node: focus.clone(),
                whole_window: false,
                ancestors: Vec::new(),
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
                answers: None,
                first_cached_ancestor: None,
            },
        }),
        focus: Some(FocusCommit::Adopt {
            reported: LastReported {
                role: focus.role,
                batch: 1,
            },
            dialog: None,
            followed: Box::new(Followed {
                object: Some(Object::Msaa(9, -4, i32::try_from(id).unwrap_or(0))),
                window: Some(9),
                terminal: Followed::terminal_mark(owes, Some(9)),
                owed: owes.then(|| {
                    Owed::due(
                        OwedWork::FirstRead(reported_caret()),
                        TraceId::from_raw(3),
                        0,
                    )
                }),
                chain: vec![focus.clone()],
                ..follow::plain_followed(focus.id, focus.role)
            }),
        }),
        ..Change::default()
    }
}

/// The node the outpost follows.
fn followed(replay: &Replay) -> Option<NodeId> {
    replay
        .reducer
        .facts()
        .follow
        .followed
        .as_ref()
        .map(|followed| followed.node)
}

/// Whether `effects` abandon the worker in charge.
fn abandons(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::Abandon { .. }))
}

#[test]
fn what_arrives_after_a_focus_marker_waits_for_its_read() {
    let mut replay = Replay::new();
    let effects = replay.push(
        Item::Fact(DeliveredFact::UiaFocusMarker { marker: 7 }, None),
        Source::Other,
        1,
    );
    assert!(programs(&effects).is_empty());
    // The copy of a `WinEvent` focus after the marker waits for its read.
    let effects = replay.push(msaa_focus(1, 2), Source::Listener, 2);
    assert!(programs(&effects).is_empty(), "the copy waits");
    let effects = replay.push(
        Item::Fact(
            DeliveredFact::UiaFocusDropped {
                marker: 7,
                focus: true,
            },
            None,
        ),
        Source::Other,
        3,
    );
    assert_eq!(
        handles(program(&effects)),
        (2, Some(0)),
        "the copy, once the marker goes"
    );
}

/// A held console's absorption cleared by a terminal read's dispatch
/// reaches the mailbox ahead of the read's program, so no update observed
/// after the read begins is dropped.
#[test]
fn a_cleared_absorption_reaches_the_mailbox_before_the_read_that_cleared_it() {
    let mut replay = Replay::new();
    let request = program(&replay.push(msaa_focus(1, 1), Source::Hook, 1)).request;
    replay.commit(request, focus_change(1, false), 2);
    replay.finish(request, false, 2);
    let effects = replay.push(name_change(1), Source::Hook, 2);
    let request = program(&effects).request;
    let effects = replay.commit(
        request,
        Change {
            mark: Some(MarkChange::Absorb(7)),
            ..Change::default()
        },
        2,
    );
    assert!(matches!(
        &effects[..],
        [Effect::Marks(Marks {
            absorbed: Some(7),
            ..
        })]
    ));
    replay.finish(request, false, 3);
    let effects = replay.push(
        Item::Query {
            request_id: RequestId::shell(5),
            query: Query::Text {
                node_id: NodeId::new(1),
                op: TextOp::TerminalHold,
            },
        },
        Source::Other,
        4,
    );
    assert!(matches!(
        &effects[..],
        [
            Effect::Marks(Marks { absorbed: None, .. }),
            Effect::Dispatch(_)
        ]
    ));
}

/// A commit the followed focus's state table says cannot occur reaches the
/// shell as the logic error it is.
#[test]
fn a_commit_that_cannot_occur_reaches_the_shell_as_a_logic_error() {
    let mut replay = Replay::new();
    let request = program(&replay.push(name_change(1), Source::Hook, 1)).request;
    let effects = replay.commit(
        request,
        Change {
            mark: Some(MarkChange::DropSystemCaret),
            ..Change::default()
        },
        2,
    );
    assert!(matches!(
        &effects[..],
        [Effect::LogicError(LogicError::MarkWithNothingFollowed(
            MarkChange::DropSystemCaret
        ))]
    ));
}

#[test]
fn a_foreground_change_and_a_focus_of_one_batch_are_one_group_tried_newest_first() {
    let mut replay = Replay::new();
    // A program in flight, so both wait for one batch.
    let effects = replay.push(name_change(5), Source::Hook, 1);
    let first = program(&effects).request;
    replay.push(msaa_focus(1, 200), Source::Hook, 2);
    replay.push(
        Item::Fact(
            DeliveredFact::Foreground { hwnd: 8 },
            Some(WinEventStamp {
                thread: 3,
                time: 184,
            }),
        ),
        Source::Hook,
        3,
    );
    let effects = replay.finish(first, false, 4);
    let focus = program(&effects);
    assert_eq!(handles(focus), (2, Some(0)), "the focus, raised last");
    let effects = replay.finish(focus.request, false, 5);
    assert_eq!(handles(program(&effects)), (3, Some(1)), "then the change");
}

/// The work a followed focus owes and a release's drop are dispatched
/// under the handling deadline, each from its dispatch.
#[test]
fn owed_work_and_a_drop_are_dispatched_under_the_handling_deadline() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let request = program(&effects).request;
    replay.commit(request, focus_change(1, true), 2);
    let effects = replay.finish(request, true, 3);
    let owed = program(&effects);
    assert!(matches!(owed.work, Work::Owed(_)));
    let owed = owed.request;
    assert_eq!(replay.reducer.flight.deadline(), Some(3_000 + 11_000_000));
    assert!(programs(&replay.finish(owed, false, 4)).is_empty());
    let effects = replay.push(
        Item::NodesHeld {
            nodes: vec![],
            anchors: vec![],
            acknowledged: 0,
        },
        Source::Other,
        5,
    );
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::Release { .. })),
        "the release asked for"
    );
    let effects = replay.input(
        Input::Released {
            nodes: BTreeSet::new(),
            anchors: BTreeSet::new(),
            objects: true,
        },
        6,
    );
    assert!(matches!(program(&effects).work, Work::Drop));
    assert_eq!(replay.reducer.flight.deadline(), Some(6_000 + 11_000_000));
}

/// What `effects` do, by kind, in order: a send, an abandonment, a
/// dispatch, or an ask.
fn kinds(effects: &[Effect]) -> Vec<&'static str> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Send(_) => Some("send"),
            Effect::Abandon { .. } => Some("abandon"),
            Effect::Dispatch(_) => Some("dispatch"),
            Effect::AskJudge { .. } => Some("judge"),
            Effect::CheckFront => Some("check front"),
            Effect::JudgeTold { .. } => Some("judge told"),
            Effect::CheckHung { .. } => Some("check hung"),
            _ => None,
        })
        .collect()
}

#[test]
fn an_owed_read_abandoned_is_not_attempted_again() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let request = program(&effects).request;
    replay.commit(request, focus_change(1, true), 2);
    let effects = replay.finish(request, true, 3);
    let owed = program(&effects);
    assert!(matches!(owed.work, Work::Owed(_)));
    // The terminal's read hangs past its deadline (6dbcca25): the worker is
    // abandoned, and the owed work, which it was attempting, is no longer
    // owed.
    let effects = replay.tick(3 + 11_000);
    assert!(abandons(&effects));
    assert!(programs(&effects).is_empty());
    assert!(
        replay
            .reducer
            .facts()
            .follow
            .followed
            .as_ref()
            .is_some_and(|followed| followed.owed.is_none())
    );
    assert_eq!(followed(&replay), Some(NodeId::new(1)));
}

#[test]
fn a_release_waits_for_the_program_in_flight_and_names_no_dead_request_live() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let hung = program(&effects).request;
    let effects = replay.push(
        Item::NodesHeld {
            nodes: vec![],
            anchors: vec![],
            acknowledged: 0,
        },
        Source::Other,
        2,
    );
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Release { .. })),
        "the release waits for the hung program"
    );
    // The program hangs in its terminal's read (8a087fe7): once its worker
    // is abandoned, the release is made, its request no longer live.
    let effects = replay.tick(1 + 11_000);
    assert!(abandons(&effects));
    let live: Vec<&Vec<Request>> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Release { live, .. } => Some(live),
            _ => None,
        })
        .collect();
    assert_eq!(live, [&Vec::<Request>::new()]);
    assert!(!live[0].contains(&hung));
    // Nothing is dispatched until the release's result is in.
    assert!(programs(&effects).is_empty());
    let effects = replay.input(
        Input::Released {
            nodes: BTreeSet::new(),
            anchors: BTreeSet::new(),
            objects: true,
        },
        11_002,
    );
    assert!(matches!(program(&effects).work, Work::Drop));
}

#[test]
fn a_caret_read_after_an_abandoned_text_call_waits_for_nothing() {
    let mut replay = Replay::new();
    let effects = replay.push(
        Item::Query {
            request_id: RequestId::shell(4),
            query: Query::Text {
                node_id: NodeId::new(1),
                op: TextOp::Read(verbatim_model::TextRead {
                    at: verbatim_model::TextPoint::Caret,
                    movement: None,
                    unit: TextUnit::Line,
                }),
            },
        },
        Source::Other,
        1,
    );
    assert_eq!(program(&effects).request.generation, 0);
    replay.push(
        Item::Msaa {
            kind: WinKind::Caret,
            hwnd: 9,
            id_object: -8,
            id_child: 0,
            raiser: 3,
        },
        Source::Hook,
        2,
    );
    // The text call hangs past its deadline: it is answered "abandoned",
    // and the caret event's read goes to the replacement at once.
    let effects = replay.tick(1 + 11_000);
    assert_eq!(replies(&effects), [(4, "abandoned")]);
    let read = program(&effects);
    assert_eq!(read.request.generation, 1);
    assert_eq!(handles(read).0, 2);
}

/// A shell items query is abandoned at the deadline it carries, on the
/// shared clock from the dialog's request, not a walk's deadline from when
/// its worker begins it: one that waited behind other work has only what
/// is left.
#[test]
fn a_shell_items_query_is_abandoned_at_its_answer_by_time() {
    let mut replay = Replay::new();
    let busy = program(&replay.push(name_change(5), Source::Hook, 1)).request;
    replay.push(
        Item::Query {
            request_id: RequestId::shell(4),
            query: Query::ShellItems {
                kind: verbatim_model::ShellItemKind::Taskbar,
                answer_by_us: 2_500_000,
            },
        },
        Source::Other,
        2,
    );
    // The query begins at 1.5 s, behind the event's program.
    let effects = replay.finish(busy, false, 1_500);
    assert_eq!(program(&effects).request.generation, 0);
    assert_eq!(replies(&replay.tick(2_499)), []);
    let effects = replay.tick(2_500);
    assert_eq!(replies(&effects), [(4, "abandoned")]);
    assert!(abandons(&effects));
}

/// A shell item position query reached at its deadline is too late to
/// walk: it is answered at once with no candidates and incomplete, and no
/// program is dispatched for it, so no deadline abandons a worker.
#[test]
fn a_shell_query_reached_at_its_deadline_is_answered_without_a_walk() {
    let mut replay = Replay::new();
    let busy = program(&replay.push(name_change(5), Source::Hook, 1)).request;
    replay.push(
        Item::Query {
            request_id: RequestId::shell(4),
            query: Query::ShellItemPosition {
                kind: verbatim_model::ShellItemKind::Taskbar,
                item: verbatim_model::ShellItem {
                    name: "Mail".to_owned(),
                    runtime_id: Vec::new(),
                },
                answer_by_us: 2_000_000,
            },
        },
        Source::Other,
        2,
    );
    // The query is reached at 2 s, behind the event's program.
    let effects = replay.finish(busy, false, 2_000);
    assert!(programs(&effects).is_empty());
    let answers: Vec<_> = sent(&effects)
        .into_iter()
        .filter_map(|message| match message {
            OutpostToCore::Reply {
                request_id,
                outcome,
                ..
            } => Some((request_id.number, outcome.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        answers,
        [(
            4,
            QueryOutcome::Done(QueryResult::ShellItemPosition(
                verbatim_model::ShellCandidates {
                    candidates: Vec::new(),
                    complete: false,
                }
            ))
        )]
    );
    let effects = replay.tick(2_500);
    assert_eq!(replies(&effects), []);
    assert!(!abandons(&effects));
}

#[test]
fn a_focus_groups_older_entries_are_tried_after_an_abandoned_try() {
    let mut replay = Replay::new();
    let effects = replay.push(name_change(5), Source::Hook, 1);
    let first = program(&effects).request;
    replay.push(msaa_focus(1, 10), Source::Hook, 2);
    replay.push(msaa_focus(2, 11), Source::Hook, 3);
    let effects = replay.finish(first, false, 4);
    let newest = program(&effects);
    assert_eq!(handles(newest), (3, Some(0)));
    // The newest try hangs and its worker is abandoned: the older entry is
    // tried by the replacement.
    let effects = replay.tick(4 + 11_000);
    assert!(abandons(&effects));
    let older = program(&effects);
    assert_eq!(handles(older), (2, Some(1)));
    assert_eq!(older.request.generation, 1);
}

#[test]
fn the_hooks_copies_are_held_until_forwarding_ends_and_released_as_one_group() {
    let mut replay = Replay::new();
    replay.input(Input::Hooked, 0);
    // The hook's copies are held.
    assert!(programs(&replay.push(msaa_focus(1, 100), Source::Hook, 1)).is_empty());
    assert!(programs(&replay.push(msaa_focus(2, 120), Source::Hook, 2)).is_empty());
    assert!(programs(&replay.push(msaa_focus(3, 140), Source::Hook, 3)).is_empty());
    // The listener's copy of the first is handled as it comes.
    let effects = replay.push(msaa_focus(1, 100), Source::Listener, 4);
    let listener = program(&effects);
    assert_eq!(handles(listener), (4, Some(0)));
    replay.finish(listener.request, true, 5);
    // The listener forwarded what was raised at or before 100: the hook's
    // copy of it is dropped, the rest tried as one group, newest first.
    let effects = replay.input(Input::ForwardingEnded(100), 6);
    let newest = program(&effects);
    assert_eq!(handles(newest), (3, Some(0)));
    let effects = replay.finish(newest.request, false, 7);
    assert_eq!(handles(program(&effects)), (2, Some(1)));
}

#[test]
fn the_end_of_the_listeners_reads_releases_the_hold() {
    let mut replay = Replay::new();
    replay.input(Input::Hooked, 0);
    replay.push(msaa_focus(1, 100), Source::Hook, 1);
    let effects = replay.push(msaa_focus(1, 100), Source::Listener, 2);
    let listener = program(&effects).request;
    replay.finish(listener, true, 3);
    replay.push(msaa_focus(2, 120), Source::Hook, 4);
    let effects = replay.push(
        Item::Fact(DeliveredFact::UiaFocusReadsEnded, None),
        Source::Other,
        5,
    );
    assert_eq!(
        handles(program(&effects)),
        (4, Some(0)),
        "the copy raised after the newest the listener sent"
    );
}

#[test]
fn a_program_waiting_on_a_window_the_user_left_is_abandoned_once_its_grace_ends() {
    let mut replay = Replay::new();
    let effects = replay.push(name_change(1), Source::Hook, 1);
    assert_eq!(program(&effects).request.generation, 0);
    replay.answers.moved_on = true;
    // Before the grace ends nothing is judged.
    assert!(!abandons(&replay.tick(400)));
    assert_eq!(
        replay.reducer.next_timer(now(400)),
        Some(Duration::from_millis(501))
    );
    assert!(abandons(&replay.tick(501)));
}

/// Past the grace, the moved-on rule is judged again every half second,
/// with no foreground fact; a wake for anything else, or a foreground fact,
/// judges nothing.
#[test]
fn the_moved_on_rule_is_judged_again_every_half_second() {
    let mut replay = Replay::new();
    replay.push(name_change(1), Source::Hook, 1);
    // At the grace's end the user has not moved on.
    assert!(!abandons(&replay.tick(501)));
    assert_eq!(
        replay.reducer.next_timer(now(501)),
        Some(Duration::from_millis(1_001))
    );
    replay.answers.moved_on = true;
    assert!(!abandons(&replay.push(name_change(2), Source::Hook, 600)));
    assert!(!abandons(&replay.push(
        Item::Fact(DeliveredFact::Foreground { hwnd: 8 }, None),
        Source::Listener,
        700,
    )));
    assert!(!abandons(&replay.tick(1_000)));
    assert!(abandons(&replay.tick(1_001)));
}

/// A tree dump and the shell item queries, dispatched with no window, are
/// checked by the window their worker names, half a second after their
/// start, as every other program is: a hang found there is told to Core
/// once, and the query stays in flight.
#[test]
fn a_tree_dump_or_a_shell_query_whose_window_hangs_tells_its_hang_once() {
    let item = verbatim_model::ShellItem {
        name: "Mail".to_owned(),
        runtime_id: vec![42, 1],
    };
    for (query, deadline) in [
        (Query::DumpTree { locations: false }, HANDLING_DEADLINE),
        (
            Query::ShellItems {
                kind: verbatim_model::ShellItemKind::Taskbar,
                answer_by_us: 3_001_000,
            },
            Duration::from_secs(3),
        ),
        (
            Query::ShellItemPosition {
                kind: verbatim_model::ShellItemKind::Taskbar,
                item,
                answer_by_us: 3_001_000,
            },
            Duration::from_secs(3),
        ),
    ] {
        let mut replay = Replay::new();
        let request = program(&replay.push(
            Item::Query {
                request_id: RequestId::shell(4),
                query: query.clone(),
            },
            Source::Other,
            1,
        ))
        .request;
        assert_eq!(
            replay.reducer.next_timer(now(1)),
            Some(Duration::from_millis(501)),
            "{query:?}"
        );
        assert_eq!(
            replay.reducer.flight.deadline(),
            Some(1_000 + micros(deadline))
        );
        replay.answers.named = Some(HUNG);
        replay.answers.hung = true;
        let effects = replay.tick(501);
        assert_eq!(kinds(&effects), ["judge", "send"], "{query:?}");
        assert_eq!(not_responding(&effects), [HUNG.get()], "{query:?}");
        assert_eq!(kinds(&replay.tick(1_001)), ["judge"], "{query:?}");
        assert!(replay.reducer.accepts(request), "{query:?}");
    }
}

/// A tree dump and the shell item queries are not about the focus, so the
/// user moving on from the window their worker names abandons neither:
/// each is bounded by its deadline alone, as NVDA's watchdog judges a
/// moved-on user against the focus object only.
#[test]
fn a_tree_dump_or_a_shell_query_is_not_abandoned_when_the_user_moves_on() {
    let item = verbatim_model::ShellItem {
        name: "Mail".to_owned(),
        runtime_id: vec![42, 1],
    };
    for query in [
        Query::DumpTree { locations: false },
        Query::ShellItems {
            kind: verbatim_model::ShellItemKind::Taskbar,
            answer_by_us: 3_001_000,
        },
        Query::ShellItemPosition {
            kind: verbatim_model::ShellItemKind::Taskbar,
            item: item.clone(),
            answer_by_us: 3_001_000,
        },
    ] {
        let mut replay = Replay::new();
        let request = program(&replay.push(
            Item::Query {
                request_id: RequestId::shell(4),
                query: query.clone(),
            },
            Source::Other,
            1,
        ))
        .request;
        replay.answers.named = Some(HUNG);
        replay.answers.moved_on = true;
        let effects = replay.tick(501);
        assert_eq!(kinds(&effects), ["judge"], "{query:?}");
        assert_eq!(replies(&effects), [], "{query:?}");
        assert!(!abandons(&effects), "{query:?}");
        assert!(replay.reducer.accepts(request), "{query:?}");
    }
}

/// The probe of window 0x20, owned by thread 5, judging from 1 ms, that
/// found `probed`.
fn probe(probed: Probed) -> VerdictChange {
    VerdictChange::Probed(Probe {
        hwnd: 0x20,
        owner: 5,
        from_us: 1_000,
        probed,
    })
}

impl Replay {
    /// Posts `change`, a verdict of `request`'s program, at `at_ms`.
    fn verdict(&mut self, request: Request, change: VerdictChange, at_ms: u64) -> Vec<Effect> {
        self.input(
            Input::Verdict {
                generation: request.generation,
                change,
            },
            at_ms,
        )
    }
}

/// The push-time copies of the verdicts `effects` replace, in order.
fn verdict_copies(effects: &[Effect]) -> Vec<&Arc<Verdicts>> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Verdicts(verdicts) => Some(verdicts),
            _ => None,
        })
        .collect()
}

/// Window 0x20's verdict in `verdicts` for an event observed at `at_us`.
fn verdict_of(verdicts: &Verdicts, at_us: u64) -> Option<Probed> {
    verdicts.kept_at(0x20, 5, at_us)
}

#[test]
fn a_probe_is_recorded_when_committed_and_reaches_the_event_thread_then_the_next_program() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let request = program(&effects).request;
    let effects = replay.verdict(request, probe(Probed::Uia), 2);
    assert_eq!(
        verdict_of(&replay.reducer.facts().verdicts, 1_000),
        Some(Probed::Uia)
    );
    // The push-time copy is the reducer's own snapshot, then the event
    // thread is nudged to decide its hook on it.
    let copies = verdict_copies(&effects);
    assert_eq!(copies.len(), 1);
    assert!(Arc::ptr_eq(copies[0], &replay.reducer.facts().verdicts));
    let order: Vec<&str> = effects
        .iter()
        .map(|effect| match effect {
            Effect::Verdicts(_) => "copy",
            Effect::Narrow => "narrow",
            _ => "other",
        })
        .collect();
    assert_eq!(order, ["copy", "narrow"]);
    // The next program is given the same snapshot.
    replay.finish(request, false, 3);
    let effects = replay.push(name_change(1), Source::Hook, 4);
    let next = program(&effects);
    assert!(Arc::ptr_eq(
        &next.facts.verdicts,
        &replay.reducer.facts().verdicts
    ));
    assert_eq!(verdict_of(&next.facts.verdicts, 1_000), Some(Probed::Uia));
}

#[test]
fn a_dead_generations_probe_is_not_recorded() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let dead = program(&effects).request;
    assert!(abandons(&replay.tick(1 + 11_000)));
    let effects = replay.verdict(dead, probe(Probed::Uia), 11_002);
    assert_eq!(verdict_of(&replay.reducer.facts().verdicts, 1_000), None);
    assert!(effects.is_empty());
}

#[test]
fn a_dead_generations_timed_out_probe_is_recorded() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let dead = program(&effects).request;
    assert!(abandons(&replay.tick(1 + 11_000)));
    let effects = replay.verdict(dead, probe(Probed::TimedOut(11_502_000)), 11_002);
    assert_eq!(
        verdict_of(&replay.reducer.facts().verdicts, 1_000),
        Some(Probed::TimedOut(11_502_000))
    );
    assert_eq!(verdict_copies(&effects).len(), 1);
}

#[test]
fn a_timed_out_probe_committed_before_its_worker_is_abandoned_stays_recorded() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let request = program(&effects).request;
    // The probe returns at 3 s, timed out, and is committed at once; the
    // program then hangs in its next call until its deadline.
    replay.verdict(request, probe(Probed::TimedOut(3_501_000)), 3_001);
    let effects = replay.tick(1 + 11_000);
    assert!(abandons(&effects));
    assert_eq!(
        verdict_of(&replay.reducer.facts().verdicts, 1_000),
        Some(Probed::TimedOut(3_501_000))
    );
    assert_eq!(verdict_copies(&effects).len(), 0);
}

/// Found by the outpost's generated test (`books/dev/src/reference/crates/verbatim-outpost-logic.md`, invariant 13): a probe that leaves the record
/// as it was, the same verdict from the same observation, sends no copy
/// and does not narrow the hook again.
#[test]
fn a_probe_that_changes_nothing_sends_no_copy() {
    let mut replay = Replay::new();
    let request = Request {
        generation: 0,
        number: 1,
    };
    let effects = replay.verdict(request, probe(Probed::Uia), 1);
    assert_eq!(verdict_copies(&effects).len(), 1, "the first verdict");
    // The verdict before it is kept for earlier observations, as itself.
    let effects = replay.verdict(request, probe(Probed::Uia), 1);
    assert_eq!(verdict_copies(&effects).len(), 1, "kept as the one before");
    let effects = replay.verdict(request, probe(Probed::Uia), 1);
    assert!(effects.is_empty(), "{effects:?}");
}

/// An edit control's text facts with one anchor, `anchor`, at offset
/// `offset`, as a text call leaves them.
fn edit_part(anchor: u64, offset: u32) -> NodeAnchors<u32> {
    let mut part = NodeAnchors::default();
    part.anchors.insert(
        anchor,
        Anchor {
            pos: offset,
            text: Arc::from("line"),
        },
    );
    part
}

/// A commit of `part` as node `node`'s edit control text facts.
fn edit_commit(node: u64, part: NodeAnchors<u32>) -> Change {
    Change {
        text: vec![TextChange::Edit {
            node,
            part,
            copied: std::collections::BTreeSet::new(),
        }],
        ..Change::default()
    }
}

/// A program's change of text facts reaches the copies of the program
/// dispatched after it once its commit is applied, while a commit the
/// reducer refuses, a dead generation's, changes nothing of them.
#[test]
fn a_programs_text_change_reaches_the_next_programs_copy() {
    let mut replay = Replay::new();
    let effects = replay.push(focus_now(1), Source::Other, 1);
    let first = program(&effects).request;
    replay.push(focus_now(2), Source::Other, 2);
    let dead = Request {
        generation: 7,
        number: 99,
    };
    assert!(
        replay
            .commit(dead, edit_commit(2, edit_part(5, 9)), 3)
            .is_empty()
    );
    replay.commit(first, edit_commit(1, edit_part(3, 4)), 4);
    let effects = replay.finish(first, false, 5);
    let text = &program(&effects).facts.text;
    assert_eq!(text.edit(1), Some(&edit_part(3, 4)));
    assert_eq!(text.edit(2), None);
}

/// The caret's location change of edit control 9, raised by thread 3.
fn edit_caret() -> Item {
    Item::Msaa {
        kind: WinKind::Caret,
        hwnd: 9,
        id_object: -8,
        id_child: 0,
        raiser: 3,
    }
}

/// The value change of edit control 9's client area, raised by thread 3.
fn edit_value_change() -> Item {
    Item::Msaa {
        kind: WinKind::ValueChange,
        hwnd: 9,
        id_object: intake::OBJID_CLIENT,
        id_child: 0,
        raiser: 3,
    }
}

/// A caret read of edit control 9's client area, node 1, cut at its value
/// change check for `finish`.
fn cut(finish: Finish) -> Box<Cut> {
    Box::new(Cut {
        edit: 9,
        node: NodeId::new(1),
        length: None,
        read_at: Some(1_000),
        timing: EventTiming::default(),
        calls: CallCounts::default(),
        finish,
    })
}

/// What a caret read cut over the selection of "hello" found: the caret
/// after "h", nothing selected.
fn report_cut() -> Finish {
    Finish::Report {
        trace: TraceId::from_raw(5),
        observed_at_us: 1_000,
        read: ReportCut {
            caret: 1,
            selection: None,
            moved: false,
            read_at_us: 1,
            last: KeptSelection {
                ends: (0, 5),
                collapsed: false,
                text: None,
            },
            fetch: None,
            previous: None,
        },
    }
}

impl Replay {
    /// Ends `request`'s program at `at_ms`, a caret read cut as `cut`.
    fn finish_cut(&mut self, request: Request, cut: Box<Cut>, at_ms: u64) -> Vec<Effect> {
        self.input(
            Input::Finished {
                request,
                reported: false,
                panicked: false,
                cut: Some(cut),
            },
            at_ms,
        )
    }
}

/// The control raises its value change while it answers the caret read,
/// before the read part's sync returns, so the change is ahead of the read
/// part's end: the finish of the caret event's report is dispatched at
/// once, told the change waits, and the change is handled after it.
#[test]
fn a_value_change_ahead_of_a_cut_read_is_found_waiting() {
    let mut replay = Replay::new();
    let read = program(&replay.push(edit_caret(), Source::Hook, 1)).request;
    assert!(
        programs(&replay.push(edit_value_change(), Source::Hook, 2)).is_empty(),
        "the read part is in flight"
    );
    let effects = replay.finish_cut(read, cut(report_cut()), 3);
    let finish = program(&effects);
    assert_eq!(finish.request.number, read.number + 1);
    assert_eq!(finishes(finish), ("report", true));
    let effects = replay.finish(finish.request, false, 4);
    assert_eq!(handles(program(&effects)), (2, None));
}

/// A value change raised after the read part's sync is behind its end:
/// the finish is told nothing waits, as the change reached nothing the
/// read part asked about.
#[test]
fn a_value_change_behind_a_cut_read_is_not_found_waiting() {
    let mut replay = Replay::new();
    let read = program(&replay.push(edit_caret(), Source::Hook, 1)).request;
    let effects = replay.finish_cut(read, cut(report_cut()), 2);
    let finish = program(&effects);
    assert_eq!(finishes(finish), ("report", false));
    assert!(
        programs(&replay.push(edit_value_change(), Source::Hook, 3)).is_empty(),
        "the finish is in flight"
    );
    let effects = replay.finish(finish.request, false, 4);
    assert_eq!(handles(program(&effects)), (3, None));
}

/// What `effects` send as the idle barrier's answer and dispatch, in
/// order.
fn answers_and_dispatches(effects: &[Effect]) -> Vec<&'static str> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Send(message) if matches!(**message, OutpostToCore::IdleAnswer { .. }) => {
                Some("answer")
            }
            Effect::Dispatch(_) => Some("dispatch"),
            _ => None,
        })
        .collect()
}

#[test]
fn the_idle_answer_waits_for_nothing_that_arrived_after_its_query() {
    let mut replay = Replay::new();
    let effects = replay.push(name_change(1), Source::Hook, 1);
    let busy = program(&effects).request;
    replay.push(
        Item::Barrier {
            asker: Asker::Round(5),
            behind_noted: false,
        },
        Source::Other,
        2,
    );
    replay.push(focus_now(1), Source::Other, 3);
    // The query is reached as the program ends: it is answered before the
    // query that arrived after it is dispatched, not once that is handled.
    let effects = replay.finish(busy, false, 4);
    assert_eq!(answers_and_dispatches(&effects), ["answer", "dispatch"]);
    assert_eq!(sent(&effects), [&OutpostToCore::IdleAnswer { round: 5 }]);
    assert_eq!(handles(program(&effects)), (3, None));
}

#[test]
fn a_noted_caret_is_read_with_its_events_timing() {
    let mut replay = Replay::new();
    let effects = replay.push(name_change(1), Source::Hook, 1);
    let request = program(&effects).request;
    let timing = EventTiming {
        observed_at_us: 1_000,
        ..EventTiming::default()
    };
    replay.commit(
        request,
        Change {
            noted_caret: Some((NodeId::new(4), TraceId::from_raw(6), 1_000, timing)),
            ..Change::default()
        },
        2,
    );
    // Read once nothing else is ready, judged by its event's observation:
    // a caret read since that event covers it, and no earlier one does.
    let effects = replay.finish(request, false, 3);
    match &program(&effects).work {
        Work::Entry { entry, .. } => {
            assert!(
                matches!(entry.item, Item::NotedCaret { node_id } if node_id == NodeId::new(4)),
                "the noted caret's read, not {:?}",
                entry.item
            );
            assert_eq!(entry.timing.observed_at_us, 1_000);
        }
        work => panic!("the noted caret's read, not {work:?}"),
    }
}

#[test]
fn the_idle_answer_waits_for_the_hooks_copies_held_at_the_start() {
    let mut replay = Replay::new();
    replay.input(Input::Hooked, 0);
    assert!(programs(&replay.push(name_change(1), Source::Hook, 1)).is_empty());
    let effects = replay.push(
        Item::Barrier {
            asker: Asker::Round(5),
            behind_noted: false,
        },
        Source::Other,
        2,
    );
    assert!(
        sent(&effects).is_empty(),
        "no answer while the copy is held"
    );
    let effects = replay.input(Input::ForwardingEnded(0), 3);
    assert_eq!(answers_and_dispatches(&effects), ["dispatch", "answer"]);
    assert_eq!(sent(&effects), [&OutpostToCore::IdleAnswer { round: 5 }]);
}

/// A test's settle, number `id`.
fn settle(id: u64) -> Item {
    Item::Barrier {
        asker: Asker::Settle(id),
        behind_noted: false,
    }
}

/// What `effects` answer as settles and dispatch, in order.
fn settled_and_dispatches(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Settled(id) => Some(format!("settled {id}")),
            Effect::Dispatch(_) => Some("dispatch".to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_settle_is_answered_as_it_is_reached_and_a_close_drops_it() {
    let mut replay = Replay::new();
    let effects = replay.push(name_change(1), Source::Hook, 1);
    let busy = program(&effects).request;
    // Pushed while a program is in flight: it waits, with no program of
    // its own, and the query after it is not waited for.
    assert_eq!(
        settled_and_dispatches(&replay.push(settle(1), Source::Other, 2)),
        [] as [String; 0]
    );
    replay.push(focus_now(1), Source::Other, 3);
    assert_eq!(replay.reducer.settles(), [1].into_iter().collect());
    let effects = replay.finish(busy, false, 4);
    assert_eq!(settled_and_dispatches(&effects), ["settled 1", "dispatch"]);
    assert_eq!(handles(program(&effects)), (3, None));
    assert!(replay.reducer.settles().is_empty());
    // A settle behind the program in flight when the outpost closes is
    // dropped with the intake, unanswered.
    let query = program(&effects).request;
    replay.push(settle(2), Source::Other, 5);
    assert_eq!(replay.reducer.settles(), [2].into_iter().collect());
    let effects = replay.input(Input::Close, 6);
    assert_eq!(settled_and_dispatches(&effects), [] as [String; 0]);
    assert!(replay.reducer.settles().is_empty());
    assert_eq!(
        settled_and_dispatches(&replay.finish(query, false, 7)),
        [] as [String; 0]
    );
}

/// The commit that reports node `id` as a focus in top-level window 7,
/// with `in_front`, what the program read of the window being in front.
fn reported_in_window(id: u64, in_front: InFront) -> Change {
    let held_node = verbatim_core::focus_rules::HeldNode {
        id: NodeId::new(id),
        backend: Backend::Msaa,
        role: Role::Button,
        name: None,
        states: StateSet::new(),
        window: None,
        above_boundary: false,
    };
    let focus = Focus {
        node: held_node,
        ancestors: Vec::new(),
        top_level: Window::new(7),
        in_front,
        entered_in_front: None,
        event: None,
        simple_child: false,
        dialog_texts: Vec::new(),
        value_from_selection: false,
    };
    Change {
        report: Some(Report::Reported(focus, Evidence::Msaa)),
        ..Change::default()
    }
}

/// What the newest report in window 7 entered in front, as the reducer
/// holds it, after the program's own copy, given the same commit, agrees.
fn commit_in_window(
    replay: &mut Replay,
    request: Request,
    change: Change,
    at_ms: u64,
) -> Option<isize> {
    let mut copy = replay.reducer.facts().clone();
    let _ = apply_local(&mut copy, request.generation, &change);
    replay.commit(request, change, at_ms);
    let held = replay
        .reducer
        .facts()
        .held
        .newest_in(Window::new(7))
        .map(|focus| focus.entered_in_front);
    let copied = copy
        .held
        .newest_in(Window::new(7))
        .map(|focus| focus.entered_in_front);
    assert_eq!(copied, held, "the program's copy and the reducer agree");
    held.flatten().map(Window::get)
}

#[test]
fn a_focus_inherits_its_windows_entered_in_front_at_commit() {
    let mut replay = Replay::new();
    let request = program(&replay.push(msaa_focus(1, 1), Source::Hook, 1)).request;
    // Read in front as the focus entered the window.
    let entered = commit_in_window(
        &mut replay,
        request,
        reported_in_window(1, InFront::Read(Window::new(7))),
        2,
    );
    assert_eq!(entered, Some(7));
    replay.finish(request, true, 3);
    // A later focus in the same window, read while another window was in
    // front, keeps what the window entered with.
    let request = program(&replay.push(msaa_focus(2, 4), Source::Hook, 4)).request;
    let entered = commit_in_window(
        &mut replay,
        request,
        reported_in_window(2, InFront::Read(None)),
        5,
    );
    assert_eq!(entered, Some(7));
}

#[test]
fn a_foreground_change_after_a_background_focus_in_the_same_window_sets_it() {
    let mut replay = Replay::new();
    let request = program(&replay.push(msaa_focus(1, 1), Source::Hook, 1)).request;
    // The window is in the background as its focus is reported.
    let entered = commit_in_window(
        &mut replay,
        request,
        reported_in_window(1, InFront::Read(None)),
        2,
    );
    assert_eq!(entered, None);
    // The user switches to it: its foreground change does not inherit.
    let entered = commit_in_window(
        &mut replay,
        request,
        reported_in_window(3, InFront::Foreground(Window::new(7))),
        3,
    );
    assert_eq!(entered, Some(7));
    // And the focus after it inherits from the foreground change.
    let entered = commit_in_window(
        &mut replay,
        request,
        reported_in_window(4, InFront::Read(None)),
        4,
    );
    assert_eq!(entered, Some(7));
}

/// An MSAA focus carries the outpost's acceptance test, made as the
/// reducer took it in, to its program unchanged, though the program runs
/// only once the program before it ends: the worker makes no test against
/// the foreground itself, and a focus refused there is dropped by its
/// program unread.
#[test]
fn an_msaa_focus_is_tested_for_acceptance_as_it_is_taken_in() {
    let mut replay = Replay::new();
    let first = program(&replay.push(focus_now(1), Source::Other, 0)).request;
    let mut tested = entry(msaa_focus(1, 5), 5);
    tested.front = Front::Accepted(false);
    let waiting = replay.input(
        Input::Entry {
            entry: tested,
            source: Source::Other,
        },
        5,
    );
    assert!(programs(&waiting).is_empty(), "the focus-now runs");
    let effects = replay.finish(first, false, 6);
    let Work::Entry { entry, .. } = &program(&effects).work else {
        panic!("the focus's program");
    };
    assert_eq!(
        (entry.observed_at_us, entry.front),
        (5_000, Front::Accepted(false))
    );
}

/// A navigation query, request `request_id`.
fn navigation(request_id: u64) -> Item {
    Item::Query {
        request_id: RequestId::shell(request_id),
        query: Query::Navigate {
            node_id: NodeId::new(1),
            kind: verbatim_model::QueryKind::NextSibling,
        },
    }
}

/// The windows `effects` tell Core stopped responding, in order.
fn not_responding(effects: &[Effect]) -> Vec<isize> {
    sent(effects)
        .into_iter()
        .filter_map(|message| match message {
            OutpostToCore::NotResponding { window } => Some(*window),
            _ => None,
        })
        .collect()
}

/// The window the hang tests' queries name.
const HUNG: Window = match Window::new(0x40) {
    Some(window) => window,
    None => panic!("0x40 is not 0"),
};

/// A commit naming window `window` hung, with its query's reply, if it
/// answers one.
fn found(window: Window, answers: Option<(u64, QueryOutcome)>) -> Change {
    Change {
        result: answers.map(|(request_id, outcome)| {
            reply(
                RequestId::shell(request_id),
                TraceId::from_raw(request_id),
                outcome,
            )
        }),
        found: Some(window),
        ..Change::default()
    }
}

/// A query whose worker named a window its check finds hung tells the
/// window's hang to Core once and stays in flight, checked again half a
/// second later with nothing more told; once it answers, a second query on
/// the window, refused by its check before its first call, is answered
/// abandoned and tells nothing, nor does a third.
#[test]
fn a_query_judged_hung_tells_its_windows_hang_once() {
    let mut replay = Replay::new();
    let first = program(&replay.push(navigation(1), Source::Other, 1)).request;
    replay.answers.named = Some(HUNG);
    replay.answers.hung = true;
    let effects = replay.tick(1 + 500);
    assert_eq!(kinds(&effects), ["judge", "send"]);
    assert_eq!(not_responding(&effects), [HUNG.get()]);
    assert!(replay.reducer.accepts(first), "the query still in flight");
    let effects = replay.tick(1 + 1_000);
    assert_eq!(kinds(&effects), ["judge"], "the same hang");
    assert!(replay.reducer.accepts(first));
    let effects = replay.commit(
        first,
        Change {
            result: Some(reply(
                RequestId::shell(1),
                TraceId::from_raw(1),
                QueryOutcome::Gone,
            )),
            ..Change::default()
        },
        1_200,
    );
    assert_eq!(replies(&effects), [(1, "gone")]);
    replay.finish(first, false, 1_201);
    replay.answers.named = None;
    for request_id in [2, 3] {
        let refused = program(&replay.push(navigation(request_id), Source::Other, 600)).request;
        let effects = replay.commit(
            refused,
            found(HUNG, Some((request_id, QueryOutcome::Abandoned))),
            601,
        );
        assert_eq!(replies(&effects), [(request_id, "abandoned")]);
        assert_eq!(
            not_responding(&effects),
            Vec::<isize>::new(),
            "the same hang"
        );
        replay.finish(refused, false, 602);
    }
}

/// A commit naming a window hung counts only if the window is hung now,
/// and the hang it tells is told once.
#[test]
fn a_commit_naming_a_window_hung_is_confirmed_live() {
    let mut replay = Replay::new();
    let stale = program(&replay.push(navigation(1), Source::Other, 1)).request;
    let effects = replay.commit(stale, found(HUNG, None), 2);
    assert_eq!(kinds(&effects), ["check hung"]);
    assert_eq!(not_responding(&effects), Vec::<isize>::new());
    assert_eq!(replay.reducer.hang, Hang::None);
    replay.finish(stale, false, 3);
    replay.answers.hung = true;
    let live = program(&replay.push(navigation(2), Source::Other, 4)).request;
    let effects = replay.commit(live, found(HUNG, None), 5);
    assert_eq!(not_responding(&effects), [HUNG.get()]);
    assert_eq!(
        kinds(&replay.commit(live, found(HUNG, None), 6)),
        ["check hung", "judge told"],
        "the same hang, judged"
    );
}

/// The told window is judged on each entry taken and each commit, and a
/// commit does not end its hang unless it is found not hung then; once it
/// is, its next freeze is a new hang.
#[test]
fn a_told_windows_hang_ends_when_it_is_judged_not_hung() {
    let mut replay = Replay::new();
    replay.answers.hung = true;
    let hang = program(&replay.push(navigation(1), Source::Other, 1)).request;
    replay.commit(hang, found(HUNG, None), 2);
    assert_eq!(
        kinds(&replay.commit(hang, Change::default(), 3)),
        ["judge told"]
    );
    assert_eq!(replay.reducer.hang, Hang::Told { window: HUNG });
    replay.finish(hang, false, 4);
    replay.answers.hung = false;
    let next = program(&replay.push(navigation(2), Source::Other, 5)).request;
    assert_eq!(replay.reducer.hang, Hang::None, "judged as the entry came");
    replay.answers.hung = true;
    assert_eq!(
        not_responding(&replay.commit(next, found(HUNG, None), 6)),
        [HUNG.get()],
        "a new hang"
    );
}

/// A second window met hung while the told window is hung is the same hang;
/// once the told window answers, the window still hung is a new one, told
/// when it is next seen, and replaces it.
#[test]
fn a_second_hung_window_is_told_only_after_the_told_window_recovers() {
    let other = Window::new(0x41).expect("not 0");
    let mut replay = Replay::new();
    replay.answers.hung = true;
    let first = program(&replay.push(navigation(1), Source::Other, 1)).request;
    replay.commit(first, found(HUNG, None), 2);
    replay.finish(first, false, 3);
    let second = program(&replay.push(navigation(2), Source::Other, 4)).request;
    assert_eq!(
        not_responding(&replay.commit(second, found(other, None), 5)),
        Vec::<isize>::new(),
        "the same hang of the application"
    );
    replay.answers.answering = Some(HUNG);
    assert_eq!(
        not_responding(&replay.commit(second, found(other, None), 6)),
        [other.get()],
        "the told window answers and the other is told"
    );
    assert_eq!(replay.reducer.hang, Hang::Told { window: other });
}

/// An answer for a window no longer told changes nothing, with or without
/// a window waiting on it.
#[test]
fn an_answer_for_a_window_no_longer_told_changes_nothing() {
    let other = Window::new(0x41).expect("not 0");
    let mut replay = Replay::new();
    replay.answers.hung = true;
    replay.input(
        Input::HangSeen {
            window: other,
            request: None,
        },
        1,
    );
    for waiting in [None, Some(HUNG)] {
        let effects = replay.input(
            Input::ToldJudged {
                window: HUNG,
                waiting,
                hung: false,
            },
            2,
        );
        assert_eq!(not_responding(&effects), Vec::<isize>::new());
        assert_eq!(replay.reducer.hang, Hang::Told { window: other });
    }
}

/// A destroy ends a hang told of its window as it arrives, even as a hook
/// copy the hand-over's hold keeps back; another window's destroy does not.
#[test]
fn a_destroy_ends_its_windows_hang_as_it_arrives() {
    let destroy = |hwnd| Item::Msaa {
        kind: WinKind::Destroy,
        hwnd,
        id_object: 0,
        id_child: 0,
        raiser: 3,
    };
    let mut replay = Replay::new();
    replay.answers.hung = true;
    replay.input(
        Input::HangSeen {
            window: HUNG,
            request: None,
        },
        1,
    );
    replay.input(Input::Hooked, 2);
    assert!(programs(&replay.push(destroy(0x41), Source::Hook, 3)).is_empty());
    assert_eq!(replay.reducer.hang, Hang::Told { window: HUNG });
    assert!(programs(&replay.push(destroy(HUNG.get()), Source::Hook, 4)).is_empty());
    assert!(replay.reducer.hold.holding());
    assert_eq!(replay.reducer.hang, Hang::None);
}

/// Nothing is told after the outpost closes, not even of a program still
/// in flight.
#[test]
fn nothing_is_told_after_close() {
    let mut replay = Replay::new();
    replay.answers.hung = true;
    let running = program(&replay.push(navigation(1), Source::Other, 1)).request;
    replay.input(Input::Close, 2);
    assert_eq!(
        kinds(&replay.commit(running, found(HUNG, None), 3)),
        Vec::<&str>::new()
    );
    assert_eq!(replay.reducer.hang, Hang::None);
}

/// A listener read whose window hangs while its program reads it tells Core
/// of the hang at its check, and is reported once the application answers:
/// its program is neither dropped nor abandoned.
#[test]
fn a_listener_read_in_a_hung_window_is_reported_and_tells_its_hang() {
    let mut replay = Replay::new();
    let read = verbatim_model::protocol::UiaFocusRead {
        hwnd: HUNG.get(),
        snapshot: verbatim_model::protocol::UiaSnapshotFact {
            runtime_id: vec![42, 2],
            role: Role::Button,
            name: None,
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
        ancestors: Vec::new(),
        ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
        selected_child: None,
        value_from_selection: false,
        window: Some(HUNG.get()),
        same_as_last: None,
        answers: None,
        cached_from: None,
    };
    let effects = replay.push(
        Item::Fact(
            DeliveredFact::UiaFocusRead {
                marker: 1,
                read: Box::new(read),
                focus: true,
            },
            None,
        ),
        Source::Listener,
        1,
    );
    let reading = program(&effects);
    assert_eq!(reading.window, Some(HUNG));
    let request = reading.request;
    replay.answers.hung = true;
    let effects = replay.tick(1 + 500);
    assert_eq!(kinds(&effects), ["judge", "send"]);
    assert_eq!(not_responding(&effects), [HUNG.get()]);
    assert!(replay.reducer.accepts(request), "the read still in flight");
    replay.answers.hung = false;
    let effects = replay.commit(request, focus_change(2, false), 4_000);
    assert_eq!(
        kinds(&effects),
        ["send", "judge told"],
        "the focus reported"
    );
    assert_eq!(followed(&replay), Some(NodeId::new(2)));
}

/// A UIA focus's marker, `marker`.
fn marker(marker: u64) -> Item {
    Item::Fact(DeliveredFact::UiaFocusMarker { marker }, None)
}

/// A marker waiting for its read past the grace has the window in front
/// checked: first half a second after it arrived, then every half second
/// while it waits, and never sooner.
#[test]
fn a_marker_waiting_past_the_grace_asks_for_the_front_check_every_half_second() {
    let mut replay = Replay::new();
    assert!(programs(&replay.push(marker(1), Source::Other, 10)).is_empty());
    assert_eq!(
        replay.reducer.next_timer(now(10)),
        Some(Duration::from_millis(510))
    );
    assert_eq!(kinds(&replay.tick(509)), Vec::<&str>::new());
    assert_eq!(kinds(&replay.tick(510)), ["check front"]);
    assert_eq!(
        replay.reducer.next_timer(now(510)),
        Some(Duration::from_millis(1_010))
    );
    assert_eq!(kinds(&replay.tick(1_009)), Vec::<&str>::new());
    assert_eq!(kinds(&replay.tick(1_010)), ["check front"]);
}

/// A hung window of the outpost's application in front while a marker
/// waits is told to Core once, however many checks find it; a readable
/// one in front ends the hang, so the next hang is told.
#[test]
fn a_hang_found_in_front_while_a_marker_waits_is_told_once() {
    let mut replay = Replay::new();
    replay.push(marker(1), Source::Other, 0);
    replay.answers.front = Some((HUNG, true));
    let effects = replay.tick(500);
    assert_eq!(kinds(&effects), ["check front", "send"]);
    assert_eq!(not_responding(&effects), [HUNG.get()]);
    assert_eq!(kinds(&replay.tick(1_000)), ["check front"], "the same hang");
    replay.answers.front = Some((HUNG, false));
    assert_eq!(kinds(&replay.tick(1_500)), ["check front"]);
    assert_eq!(replay.reducer.hang, Hang::None, "it answers");
    replay.answers.front = Some((HUNG, true));
    assert_eq!(
        not_responding(&replay.tick(2_000)),
        [HUNG.get()],
        "a new hang"
    );
}

/// Once the marker's read comes no front check is asked, nor timed.
#[test]
fn no_front_check_is_asked_once_the_marker_resolves() {
    let mut replay = Replay::new();
    replay.push(marker(1), Source::Other, 0);
    replay.answers.front = Some((HUNG, true));
    let effects = replay.push(
        Item::Fact(
            DeliveredFact::UiaFocusDropped {
                marker: 1,
                focus: true,
            },
            None,
        ),
        Source::Other,
        300,
    );
    assert_eq!(kinds(&effects), Vec::<&str>::new());
    assert_eq!(replay.reducer.next_timer(now(300)), None);
    assert_eq!(kinds(&replay.tick(500)), Vec::<&str>::new());
    assert_eq!(replay.reducer.hang, Hang::None);
}

/// A program whose check found its window hung, abandoned at its deadline,
/// is counted parked on a hung window; one whose check never did is not.
#[test]
fn a_program_abandoned_after_its_hang_was_seen_is_parked_on_a_hung_window() {
    let on_hung = |effects: &[Effect]| {
        effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Abandon { on_hung, .. } => Some(*on_hung),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let mut replay = Replay::new();
    replay.push(navigation(1), Source::Other, 1);
    replay.answers.named = Some(HUNG);
    replay.answers.hung = true;
    replay.tick(1 + 500);
    replay.answers.hung = false;
    assert_eq!(on_hung(&replay.tick(1 + 11_000)), [true]);
    replay.push(navigation(2), Source::Other, 11_002);
    assert_eq!(on_hung(&replay.tick(11_002 + 11_000)), [false]);
}

/// The listener's read of the button `runtime` in window 0x10, answering
/// Core's live re-read `answers` when it is given.
fn listener_read(runtime: i32, answers: Option<u64>) -> Item {
    let read = verbatim_model::protocol::UiaFocusRead {
        hwnd: 0x10,
        snapshot: verbatim_model::protocol::UiaSnapshotFact {
            runtime_id: vec![42, runtime],
            role: Role::Button,
            name: None,
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
        ancestors: Vec::new(),
        ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
        selected_child: None,
        value_from_selection: false,
        window: Some(0x10),
        same_as_last: None,
        answers,
        cached_from: None,
    };
    Item::Fact(
        DeliveredFact::UiaFocusRead {
            marker: 99,
            read: Box::new(read),
            focus: true,
        },
        None,
    )
}

/// The live re-reads `effects` answer gone, by request.
fn gone(effects: &[Effect]) -> Vec<u64> {
    sent(effects)
        .into_iter()
        .filter_map(|message| match message {
            OutpostToCore::LiveRereadGone { request } => Some(*request),
            _ => None,
        })
        .collect()
}

/// `deadline` in milliseconds.
fn millis(deadline: Duration) -> u64 {
    u64::try_from(deadline.as_millis()).unwrap_or(u64::MAX)
}

/// A live re-read's answer the outpost drops is answered gone, on every
/// drop path, so Core's pending live re-read, which holds the idle barrier,
/// always ends: a listener's read whose program ends without reporting it
/// (a rule dropped it, or its element was gone), one whose program
/// panicked, one abandoned at its deadline, and Core's live re-read of an
/// MSAA report abandoned at its deadline. Each is answered once.
#[test]
fn a_live_re_read_answer_dropped_by_the_outpost_is_answered_gone() {
    let mut replay = Replay::new();
    let dropped = program(&replay.push(listener_read(1, Some(4)), Source::Other, 1)).request;
    assert_eq!(gone(&replay.finish(dropped, false, 2)), [4]);

    let panicked = program(&replay.push(listener_read(1, Some(5)), Source::Other, 3)).request;
    let effects = replay.input(
        Input::Finished {
            request: panicked,
            reported: false,
            panicked: true,
            cut: None,
        },
        4,
    );
    assert_eq!(gone(&effects), [5]);

    replay.push(listener_read(1, Some(6)), Source::Other, 5);
    let effects = replay.tick(5 + millis(FOCUS_READ_DEADLINE));
    assert!(abandons(&effects));
    assert_eq!(gone(&effects), [6]);

    let at = 5 + millis(FOCUS_READ_DEADLINE) + 1;
    replay.push(
        Item::Fact(
            DeliveredFact::MsaaLiveReread {
                request: 7,
                trace_id: TraceId::from_raw(3),
            },
            None,
        ),
        Source::Other,
        at,
    );
    let effects = replay.tick(at + millis(HANDLING_DEADLINE));
    assert!(abandons(&effects));
    assert_eq!(gone(&effects), [7]);
}

/// Core's live re-read of an MSAA report the user has moved on from, its
/// window named by its worker, is abandoned by the moved-on rule once its
/// grace ends and answered gone, so it never holds Core's focus queue to
/// its deadline.
#[test]
fn a_live_re_read_the_user_moved_on_from_is_answered_gone() {
    let mut replay = Replay::new();
    let effects = replay.push(
        Item::Fact(
            DeliveredFact::MsaaLiveReread {
                request: 7,
                trace_id: TraceId::from_raw(3),
            },
            None,
        ),
        Source::Other,
        1,
    );
    assert_eq!(programs(&effects).len(), 1);
    // Its worker names its report's window.
    replay.answers.named = Window::new(0x30);
    replay.answers.moved_on = true;
    assert!(!abandons(&replay.tick(400)), "nothing judged in the grace");
    let effects = replay.tick(501);
    assert!(abandons(&effects));
    assert_eq!(gone(&effects), [7]);
}

/// A live re-read answered by its report is not answered again when its
/// program is abandoned after sending it.
#[test]
fn a_live_re_read_answered_before_its_abandonment_is_not_answered_gone() {
    let mut replay = Replay::new();
    let request = program(&replay.push(listener_read(1, Some(4)), Source::Other, 1)).request;
    let mut answer = focus_change(1, false);
    if let Some(OutpostToCore::Event {
        event: NormalizedEvent::FocusChanged { answers, .. },
        ..
    }) = &mut answer.result
    {
        *answers = Some(4);
    }
    assert_eq!(kinds(&replay.commit(request, answer, 2)), ["send"]);
    let effects = replay.tick(1 + millis(FOCUS_READ_DEADLINE));
    assert!(abandons(&effects));
    assert_eq!(gone(&effects), Vec::<u64>::new());
}

/// A newer focus of the same element arriving while a live re-read's
/// answer waits does not replace it: both are handled, the answer first,
/// so its program alone answers the live re-read.
#[test]
fn a_newer_focus_of_the_same_element_never_replaces_a_live_re_read_answer() {
    let answers = |program: &Program| match &program.work {
        Work::Entry {
            entry:
                Entry {
                    item: Item::Fact(DeliveredFact::UiaFocusRead { read, .. }, _),
                    ..
                },
            ..
        } => read.answers,
        work => panic!("a listener read's program, not {work:?}"),
    };
    let mut replay = Replay::new();
    let held = program(&replay.push(msaa_focus(1, 1), Source::Hook, 1)).request;
    assert!(programs(&replay.push(listener_read(2, Some(4)), Source::Other, 2)).is_empty());
    assert!(programs(&replay.push(listener_read(2, None), Source::Other, 3)).is_empty());
    let answer = program(&replay.finish(held, false, 4)).clone();
    assert_eq!(answers(&answer), Some(4), "the answer first");
    let newer = program(&replay.finish(answer.request, false, 5)).clone();
    assert_eq!(answers(&newer), None, "then the newer focus");
}

/// The foreground window's element is found by one program, whose ending
/// answers it; a window's change while that program runs starts another
/// once it ends, and its late answer lets its token go.
#[test]
fn the_foreground_windows_element_is_found_by_one_program_at_a_time() {
    use verbatim_model::protocol::FrontWindow;
    let mut replay = Replay::new();
    let first = replay.input(Input::Front(FrontWindow::In(10)), 0);
    let first_program = program(&first);
    assert!(matches!(first_program.work, Work::FindBar(10)));
    let request = first_program.request;
    // While it runs the window changes: no second program yet.
    let changed = replay.input(Input::Front(FrontWindow::In(11)), 1);
    assert!(
        changed
            .iter()
            .all(|effect| !matches!(effect, Effect::Dispatch(_)))
    );
    // Its answer is of the old window: its token goes.
    let late = replay.commit(
        request,
        Change {
            bar_found: Some((10, 7)),
            ..Change::default()
        },
        2,
    );
    assert!(
        late.iter()
            .any(|effect| matches!(effect, Effect::Unfollow(tokens) if tokens == &vec![7]))
    );
    let finished = replay.input(
        Input::Finished {
            request,
            reported: false,
            panicked: false,
            cut: None,
        },
        3,
    );
    // The new window's program, and the watch is not ended by the old one.
    let second = program(&finished);
    assert!(matches!(second.work, Work::FindBar(11)));
    assert_eq!(
        replay.reducer.facts().follow.bar,
        BarWatch::Finding { window: 11 }
    );
    // Its program ending with no element found ends the watch.
    let request = second.request;
    replay.input(
        Input::Finished {
            request,
            reported: false,
            panicked: false,
            cut: None,
        },
        4,
    );
    assert_eq!(replay.reducer.facts().follow.bar, BarWatch::None);
}

/// A lookup program abandoned at its deadline answers the watch too, so
/// the watch does not wait for ever.
#[test]
fn an_abandoned_lookup_of_the_foreground_windows_element_ends_the_watch() {
    use verbatim_model::protocol::FrontWindow;
    let mut replay = Replay::new();
    let _ = replay.input(Input::Front(FrontWindow::In(10)), 0);
    assert!(replay.reducer.busy());
    let deadline = u64::try_from(HANDLING_DEADLINE.as_millis()).expect("milliseconds fit");
    let _ = replay.tick(deadline + 1);
    assert_eq!(replay.reducer.facts().follow.bar, BarWatch::None);
}

/// Every entry is given the handling deadline but two, focus-now and a
/// listener read's report, which are given the bounded walk's: what is
/// left until its answer is due for a shell query.
#[test]
fn every_entry_is_given_the_handling_deadline_but_focus_now_and_a_listener_reads_report() {
    let query = |query: Query| Item::Query {
        request_id: RequestId::shell(4),
        query,
    };
    let deadline = |item: Item| budget(&entry(item, 1), now(1_000), HANDLING_DEADLINE).0;
    assert_eq!(deadline(focus_now(4)), FOCUS_READ_DEADLINE);
    let read = verbatim_model::protocol::UiaFocusRead {
        hwnd: 0x10,
        snapshot: verbatim_model::protocol::UiaSnapshotFact {
            runtime_id: vec![42, 2],
            role: Role::ListItem,
            name: None,
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
        ancestors: Vec::new(),
        ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
        selected_child: None,
        value_from_selection: false,
        window: Some(0x10),
        same_as_last: None,
        answers: None,
        cached_from: None,
    };
    assert_eq!(
        deadline(Item::Fact(
            DeliveredFact::UiaFocusRead {
                marker: 1,
                read: Box::new(read),
                focus: true,
            },
            None,
        )),
        FOCUS_READ_DEADLINE
    );
    assert_eq!(deadline(msaa_focus(1, 5)), HANDLING_DEADLINE);
    assert_eq!(deadline(name_change(1)), HANDLING_DEADLINE);
    assert_eq!(
        deadline(read_caret(4, ReadPurpose::BeforeFieldKey)),
        HANDLING_DEADLINE
    );
    assert_eq!(
        deadline(query(Query::ShellItems {
            kind: verbatim_model::ShellItemKind::Taskbar,
            answer_by_us: 1_300_000,
        })),
        Duration::from_millis(300),
        "the answer is due 300 ms after the shell's clock's now"
    );
    assert_eq!(
        deadline(query(Query::ShellItems {
            kind: verbatim_model::ShellItemKind::Taskbar,
            answer_by_us: 900_000,
        })),
        Duration::ZERO,
        "an answer already due has none left"
    );
    assert_eq!(
        deadline(query(Query::Ancestors {
            node_id: NodeId::new(1),
        })),
        HANDLING_DEADLINE
    );
    assert_eq!(
        deadline(query(Query::DumpTree { locations: false })),
        HANDLING_DEADLINE
    );
    assert_eq!(
        deadline(Item::NodesHeld {
            nodes: Vec::new(),
            anchors: Vec::new(),
            acknowledged: 1,
        }),
        HANDLING_DEADLINE
    );
    assert_eq!(
        deadline(query(Query::Activate {
            node_id: NodeId::new(1),
        })),
        HANDLING_DEADLINE
    );
    assert_eq!(
        deadline(query(Query::Navigate {
            node_id: NodeId::new(1),
            kind: verbatim_model::QueryKind::NextSibling,
        })),
        HANDLING_DEADLINE
    );
}

#[test]
fn a_query_is_answered_once_whatever_ends_it() {
    // By its program.
    let mut replay = Replay::new();
    let effects = replay.push(focus_now(1), Source::Other, 1);
    let request = program(&effects).request;
    let effects = replay.commit(
        request,
        Change {
            result: Some(done(1)),
            ..Change::default()
        },
        2,
    );
    assert_eq!(replies(&effects), [(1, "done")]);
    assert_eq!(replies(&replay.finish(request, false, 3)), []);
    assert!(replay.reducer.queries().is_empty());

    // At its deadline, its late result refused.
    let effects = replay.push(focus_now(2), Source::Other, 4);
    let request = program(&effects).request;
    let effects = replay.tick(4 + 13_000);
    assert_eq!(replies(&effects), [(2, "abandoned")]);
    assert!(!replay.reducer.accepts(request));
    assert!(replay.reducer.queries().is_empty());

    // By a program that ended without answering it.
    let effects = replay.push(focus_now(3), Source::Other, 10_010);
    let request = program(&effects).request;
    let effects = replay.finish(request, false, 10_011);
    assert_eq!(replies(&effects), [(3, "failed")]);
    assert!(replay.reducer.queries().is_empty());

    // Withdrawn before it started.
    let effects = replay.push(name_change(1), Source::Hook, 10_020);
    let busy = program(&effects).request;
    replay.push(focus_now(4), Source::Other, 10_021);
    let effects = replay.input(Input::Cancel(RequestId::shell(4)), 10_022);
    assert_eq!(replies(&effects), [(4, "not started")]);
    assert!(
        matches!(
            sent(&effects)[..],
            [OutpostToCore::Reply { trace_id, .. }] if *trace_id == TraceId::from_raw(10_021)
        ),
        "the reply carries the query's trace"
    );
    assert!(programs(&replay.finish(busy, false, 10_023)).is_empty());
    assert!(replay.reducer.queries().is_empty());
}

/// The finish `program` is: what it finishes, and whether the reducer
/// found the edit control's value change waiting.
fn finishes(program: &Program) -> (&'static str, bool) {
    match &program.work {
        Work::Finish { cut, waiting } => {
            let what = match cut.finish {
                Finish::Report { .. } => "report",
                Finish::Before { .. } => "key",
                Finish::Note { .. } => "note",
            };
            (what, *waiting)
        }
        work => panic!("a finish, not {work:?}"),
    }
}

/// A caret read for a key, request `key`, on node 1.
fn read_caret(key: u64, purpose: ReadPurpose) -> Item {
    Item::Query {
        request_id: RequestId::shell(key),
        query: Query::Text {
            node_id: NodeId::new(1),
            op: TextOp::ReadCaret(CaretRead {
                fetch: fetch(),
                purpose,
            }),
        },
    }
}

/// A key's fetch: a character to the right, in a text field.
fn fetch() -> KeyFetch {
    KeyFetch {
        unit: TextUnit::Character,
        motion: CaretMotion::NextCharacter,
    }
}

/// The note on node 1, as the reducer holds it at `at_ms`.
fn note_at(replay: &Replay, at_ms: u64) -> Option<KeyNote> {
    replay.reducer.facts().text.live_note(1, at_ms * 1_000)
}

#[test]
fn a_before_read_sets_the_note_as_it_arrives_and_a_final_read_goes_ahead_of_the_queue() {
    let mut replay = Replay::new();
    let effects = replay.push(name_change(1), Source::Hook, 1);
    let busy = program(&effects).request;
    assert!(
        programs(&replay.push(read_caret(5, ReadPurpose::BeforeFieldKey), Source::Other, 2))
            .is_empty()
    );
    assert_eq!(
        note_at(&replay, 2),
        Some(KeyNote {
            fetch: NoteFetch {
                unit: TextUnit::Character,
                motion: CaretMotion::NextCharacter,
            },
            until_us: 302_000,
            reads: NoteReads::Field(FieldReads::CaretAndText),
        })
    );
    assert!(
        programs(&replay.push(read_caret(6, ReadPurpose::BeforeFieldKey), Source::Other, 3))
            .is_empty()
    );
    assert!(programs(&replay.push(read_caret(7, ReadPurpose::Final), Source::Other, 4)).is_empty());
    assert!(
        note_at(&replay, 4).is_some(),
        "the final read clears it when it commits, not as it arrives"
    );
    // The final read goes ahead of the queued read.
    let effects = replay.finish(busy, false, 5);
    let next = program(&effects);
    assert!(matches!(
        &next.work,
        Work::Entry { entry, .. }
            if matches!(entry.item, Item::Query { request_id: RequestId { number: 7, .. }, .. })
    ));
}

#[test]
fn a_console_note_is_polled_when_nothing_else_is_ready_and_stops_at_its_end() {
    let mut replay = Replay::new();
    replay.input(
        Input::KeyNote {
            node: NodeId::new(1),
            fetch: fetch(),
            on: NoteOn::ConsoleHost,
        },
        1,
    );
    assert_eq!(
        replay.reducer.next_timer(now(1)),
        Some(Duration::from_millis(6))
    );
    let effects = replay.tick(6);
    let read = program(&effects);
    assert!(matches!(read.work, Work::NoteRead(_)));
    let request = read.request;
    // In flight, no poll is timed; a poll after it is due 5 ms later.
    assert_eq!(replay.finish(request, false, 7).len(), 0);
    assert_eq!(
        replay.reducer.next_timer(now(7)),
        Some(Duration::from_millis(11))
    );
    // The read due at the poll's bound is made, and the one reached after
    // that bound is not: the note stops polling.
    let effects = replay.tick(51);
    let read = program(&effects).request;
    assert!(programs(&replay.finish(read, false, 52)).is_empty());
    assert_eq!(replay.reducer.next_timer(now(53)), None);
    assert!(matches!(
        note_at(&replay, 53).map(|note| note.reads),
        Some(NoteReads::Terminal(TerminalReads::CaretAndText))
    ));
}

#[test]
fn a_verbatim_caret_move_clears_the_notes_of_its_node() {
    let mut replay = Replay::new();
    replay.input(
        Input::KeyNote {
            node: NodeId::new(1),
            fetch: fetch(),
            on: NoteOn::Field,
        },
        1,
    );
    assert!(note_at(&replay, 1).is_some());
    let moved = Item::Query {
        request_id: RequestId::shell(9),
        query: Query::Text {
            node_id: NodeId::new(1),
            op: TextOp::MoveCaret(verbatim_model::TextPoint::Start),
        },
    };
    let effects = replay.push(moved, Source::Other, 2);
    let request = program(&effects).request;
    assert_eq!(note_at(&replay, 2), None);
    replay.finish(request, false, 3);
}
