//! The reducer's replay tests (`books/dev/src/explanation/design/ownership.md`): recorded inputs given to the reducer, with its
//! asks answered as the shell answers them, and the effects it answers with asserted. Each replays one
//! ordering the outpost's threads can produce, at the times given, so an
//! ordering that once needed a real bound, a real deadline or a race to
//! reproduce is one test here.

use std::collections::BTreeSet;
use std::time::Duration;

use verbatim_model::protocol::{EventTiming, OutpostToCore, Query, QueryOutcome, QueryResult};
use verbatim_model::{
    CallCounts, CaretMotion, CaretRead, KeyFetch, NodeId, ReadPurpose, TextOp, TextReply, TextUnit,
    TraceId,
};

use super::*;

mod asked;

/// The reducer under test.
struct Replay {
    reducer: Reducer,
}

impl Replay {
    fn new() -> Self {
        Self {
            reducer: Reducer::new(),
        }
    }

    fn input(&mut self, input: Input, at_ms: u64) -> Vec<Effect> {
        self.reducer.input(input, now(at_ms))
    }

    fn tick(&mut self, at_ms: u64) -> Vec<Effect> {
        self.reducer.tick(now(at_ms))
    }

    /// Pushes `item`, observed at `at_ms`.
    fn push(&mut self, item: Item, at_ms: u64) -> Vec<Effect> {
        self.input(Input::Entry(entry(item, at_ms)), at_ms)
    }

    /// Ends `request`'s program at `at_ms`.
    fn finish(&mut self, request: Request, at_ms: u64) -> Vec<Effect> {
        self.input(
            Input::Finished {
                request,
                reported: false,
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
    }
}

/// A console caret event of window `window`: an event the outpost hears,
/// its program handling one entry.
fn console_caret(window: isize) -> Item {
    Item::Msaa {
        kind: WinKind::ConsoleCaret,
        hwnd: window,
        id_object: -8,
        id_child: 0,
        raiser: 3,
    }
}

/// A console update of window `window`.
fn console_update(window: isize) -> Item {
    Item::Msaa {
        kind: WinKind::ConsoleUpdate,
        hwnd: window,
        id_object: 3,
        id_child: 65,
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

/// The observation, in milliseconds, of the entry `program` handles.
fn handles(program: &Program) -> u64 {
    match &program.work {
        Work::Entry { entry, .. } => entry.observed_at_us / 1_000,
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
        outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Unanswered)),
        timing: EventTiming::default(),
    }
}

/// Whether `effects` abandon the worker in charge.
/// Core's request `number`, which runs as a program on the worker.
fn core_ask(number: u64) -> Input {
    Input::Ask(Box::new(verbatim_model::request::Ask {
        number: verbatim_model::request::RequestNumber(number),
        window: verbatim_model::WindowHandle(9),
        verdict: verbatim_model::request::WindowVerdict::Msaa,
        request: verbatim_model::request::Request::CurrentFocus {
            set: Vec::new(),
            known: Vec::new(),
            baseline: None,
        },
    }))
}

/// Core cancels its request `number` because the foreground moved away,
/// which abandons the worker running it.
fn cancel_for_the_foreground(number: u64) -> Input {
    Input::CancelAsk(verbatim_model::request::Cancel {
        number: verbatim_model::request::RequestNumber(number),
        reason: verbatim_model::request::CancelReason::Foreground,
    })
}

fn abandons(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::Abandon { .. }))
}

/// A commit the followed focus's state table says cannot occur reaches the
/// shell as the logic error it is.
#[test]
fn a_commit_that_cannot_occur_reaches_the_shell_as_a_logic_error() {
    let mut replay = Replay::new();
    let request = program(&replay.push(console_update(1), 1)).request;
    let effects = replay.commit(
        request,
        Change {
            mark: Some(MarkChange::ClearRedirect),
            ..Change::default()
        },
        2,
    );
    assert!(matches!(
        &effects[..],
        [Effect::LogicError(LogicError::MarkWithNothingFollowed(
            MarkChange::ClearRedirect
        ))]
    ));
}

#[test]
fn a_release_waits_for_the_program_in_flight_and_names_no_dead_request_live() {
    let mut replay = Replay::new();
    let effects = replay.input(core_ask(1), 1);
    let hung = program(&effects).request;
    let effects = replay.push(
        Item::NodesHeld {
            nodes: vec![],
            anchors: vec![],
            acknowledged: 0,
        },
        2,
    );
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, Effect::Release { .. })),
        "the release waits for the hung program"
    );
    // The program hangs in its read (8a087fe7): once its worker is
    // abandoned, the release is made, its request no longer live.
    let effects = replay.input(cancel_for_the_foreground(1), 11_000);
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

/// A shell item position query reached at its deadline is too late to
/// walk: it is answered at once with no candidates and incomplete, and no
/// program is dispatched for it, so no deadline abandons a worker.
#[test]
fn a_shell_query_reached_at_its_deadline_is_answered_without_a_walk() {
    let mut replay = Replay::new();
    let busy = program(&replay.push(console_update(5), 1)).request;
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
        2,
    );
    // The query is reached at 2 s, behind the event's program.
    let effects = replay.finish(busy, 2_000);
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

/// Window 0x20's verdict in `verdicts` for an event observed at `at_us`.
fn verdict_of(verdicts: &Verdicts, at_us: u64) -> Option<Probed> {
    verdicts.kept_at(0x20, 5, at_us)
}

#[test]
fn a_probe_is_recorded_when_committed_and_reaches_the_next_program() {
    let mut replay = Replay::new();
    let effects = replay.push(console_caret(1), 1);
    let request = program(&effects).request;
    let effects = replay.verdict(request, probe(Probed::Uia), 2);
    assert_eq!(
        verdict_of(&replay.reducer.facts().verdicts, 1_000),
        Some(Probed::Uia)
    );
    assert_eq!(effects.len(), 0, "a probe asks nothing of the shell");
    // The next program is given the same snapshot.
    replay.finish(request, 3);
    let effects = replay.push(console_update(1), 4);
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
    let effects = replay.input(core_ask(1), 1);
    let dead = program(&effects).request;
    assert!(abandons(
        &replay.input(cancel_for_the_foreground(1), 11_000)
    ));
    let effects = replay.verdict(dead, probe(Probed::Uia), 11_002);
    assert_eq!(verdict_of(&replay.reducer.facts().verdicts, 1_000), None);
    assert!(effects.is_empty());
}

#[test]
fn a_dead_generations_timed_out_probe_is_recorded() {
    let mut replay = Replay::new();
    let effects = replay.input(core_ask(1), 1);
    let dead = program(&effects).request;
    assert!(abandons(
        &replay.input(cancel_for_the_foreground(1), 11_000)
    ));
    replay.verdict(dead, probe(Probed::TimedOut(11_502_000)), 11_002);
    assert_eq!(
        verdict_of(&replay.reducer.facts().verdicts, 1_000),
        Some(Probed::TimedOut(11_502_000))
    );
}

#[test]
fn a_timed_out_probe_committed_before_its_worker_is_abandoned_stays_recorded() {
    let mut replay = Replay::new();
    let effects = replay.input(core_ask(1), 1);
    let request = program(&effects).request;
    // The probe returns at 3 s, timed out, and is committed at once; the
    // program then hangs in its next call until Core cancels its request
    // because the foreground moved away.
    replay.verdict(request, probe(Probed::TimedOut(3_501_000)), 3_001);
    let effects = replay.input(cancel_for_the_foreground(1), 11_000);
    assert!(abandons(&effects));
    assert_eq!(
        verdict_of(&replay.reducer.facts().verdicts, 1_000),
        Some(Probed::TimedOut(3_501_000))
    );
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
    let effects = replay.push(focus_now(1), 1);
    let first = program(&effects).request;
    replay.push(focus_now(2), 2);
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
    let effects = replay.finish(first, 5);
    let text = &program(&effects).facts.text;
    assert_eq!(text.edit(1), Some(&edit_part(3, 4)));
    assert_eq!(text.edit(2), None);
}

/// The caret's location change of edit control 9, raised by thread 3.
fn edit_caret() -> Item {
    console_caret(9)
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

/// A cut read's finish is dispatched as the read part ends, ahead of an
/// entry that arrived meanwhile, and is told no value change waits: none
/// reaches the outpost, Core's event queue judging them.
#[test]
fn a_cut_reads_finish_runs_first_and_finds_no_value_change_waiting() {
    let mut replay = Replay::new();
    let read = program(&replay.push(edit_caret(), 1)).request;
    let effects = replay.finish_cut(read, cut(report_cut()), 2);
    let finish = program(&effects);
    assert_eq!(finishes(finish), ("report", false));
    assert!(
        programs(&replay.push(console_update(9), 3)).is_empty(),
        "the finish is in flight"
    );
    let effects = replay.finish(finish.request, 4);
    assert_eq!(handles(program(&effects)), 3);
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
    let effects = replay.push(console_update(1), 1);
    let busy = program(&effects).request;
    replay.push(
        Item::Barrier {
            asker: Asker::Round(5),
        },
        2,
    );
    replay.push(focus_now(1), 3);
    // The query is reached as the program ends: it is answered before the
    // query that arrived after it is dispatched, not once that is handled.
    let effects = replay.finish(busy, 4);
    assert_eq!(answers_and_dispatches(&effects), ["answer", "dispatch"]);
    assert_eq!(sent(&effects), [&OutpostToCore::IdleAnswer { round: 5 }]);
    assert_eq!(handles(program(&effects)), 3);
}

fn settle(id: u64) -> Item {
    Item::Barrier {
        asker: Asker::Settle(id),
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
    let effects = replay.push(console_update(1), 1);
    let busy = program(&effects).request;
    // Pushed while a program is in flight: it waits, with no program of
    // its own, and the query after it is not waited for.
    assert_eq!(
        settled_and_dispatches(&replay.push(settle(1), 2)),
        [] as [String; 0]
    );
    replay.push(focus_now(1), 3);
    assert_eq!(replay.reducer.settles(), [1].into_iter().collect());
    let effects = replay.finish(busy, 4);
    assert_eq!(settled_and_dispatches(&effects), ["settled 1", "dispatch"]);
    assert_eq!(handles(program(&effects)), 3);
    assert!(replay.reducer.settles().is_empty());
    // A settle behind the program in flight when the outpost closes is
    // dropped with the intake, unanswered.
    let query = program(&effects).request;
    replay.push(settle(2), 5);
    assert_eq!(replay.reducer.settles(), [2].into_iter().collect());
    let effects = replay.input(Input::Close, 6);
    assert_eq!(settled_and_dispatches(&effects), [] as [String; 0]);
    assert!(replay.reducer.settles().is_empty());
    assert_eq!(
        settled_and_dispatches(&replay.finish(query, 7)),
        [] as [String; 0]
    );
}

#[test]
fn a_query_is_answered_once_whatever_ends_it() {
    // By its program.
    let mut replay = Replay::new();
    let effects = replay.push(focus_now(1), 1);
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
    assert_eq!(replies(&replay.finish(request, 3)), []);
    assert!(replay.reducer.queries().is_empty());

    // By a program that ended without answering it.
    let effects = replay.push(focus_now(3), 10_010);
    let request = program(&effects).request;
    let effects = replay.finish(request, 10_011);
    assert_eq!(replies(&effects), [(3, "failed")]);
    assert!(replay.reducer.queries().is_empty());

    // Withdrawn before it started.
    let effects = replay.push(console_update(1), 10_020);
    let busy = program(&effects).request;
    replay.push(focus_now(4), 10_021);
    let effects = replay.input(Input::Cancel(RequestId::shell(4)), 10_022);
    assert_eq!(replies(&effects), [(4, "not started")]);
    assert!(
        matches!(
            sent(&effects)[..],
            [OutpostToCore::Reply { trace_id, .. }] if *trace_id == TraceId::from_raw(10_021)
        ),
        "the reply carries the query's trace"
    );
    assert!(programs(&replay.finish(busy, 10_023)).is_empty());
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
    let effects = replay.push(console_update(1), 1);
    let busy = program(&effects).request;
    assert!(programs(&replay.push(read_caret(5, ReadPurpose::BeforeFieldKey), 2)).is_empty());
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
    assert!(programs(&replay.push(read_caret(6, ReadPurpose::BeforeFieldKey), 3)).is_empty());
    assert!(programs(&replay.push(read_caret(7, ReadPurpose::Final), 4)).is_empty());
    assert!(
        note_at(&replay, 4).is_some(),
        "the final read clears it when it commits, not as it arrives"
    );
    // The final read goes ahead of the queued read.
    let effects = replay.finish(busy, 5);
    let next = program(&effects);
    assert!(matches!(
        &next.work,
        Work::Entry { entry, .. }
            if matches!(entry.item, Item::Query { request_id: RequestId { number: 7, .. }, .. })
    ));
}

#[test]
fn a_verbatim_caret_move_clears_the_notes_of_its_node() {
    let mut replay = Replay::new();
    replay.input(
        Input::KeyNote {
            node: NodeId::new(1),
            fetch: fetch(),
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
    let effects = replay.push(moved, 2);
    let request = program(&effects).request;
    assert_eq!(note_at(&replay, 2), None);
    replay.finish(request, 3);
}
