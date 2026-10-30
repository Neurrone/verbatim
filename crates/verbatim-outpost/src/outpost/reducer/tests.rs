//! The reducer's replay tests (`docs/design/ownership-outpost-reducer.md`,
//! "Tests"): recorded inputs given to the reducer, with recorded local
//! reads, and the effects it answers with asserted. Each replays one
//! ordering the outpost's threads can produce, at the times given, so an
//! ordering that once needed a real bound, a real deadline or a race to
//! reproduce is one test here.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use verbatim_model::protocol::{
    DeliveredFact, EventTiming, GapAnswer, OutpostToCore, Query, QueryOutcome, QueryResult,
    WinEventStamp,
};
use verbatim_model::{
    Backend, CallCounts, CaretWatch, NodeId, NodeSnapshot, NormalizedEvent, Role, StateSet, TextOp,
    TextReply, TextUnit, TraceId,
};

use super::*;

/// The local reads the reducer makes, as a test records them: the hung
/// windows, whether the user has moved on, and the publications the shell
/// would make at a walk's bound, in order.
#[derive(Default)]
struct Recorded {
    hung: HashSet<isize>,
    moved_on: bool,
    publications: VecDeque<Change>,
    published: Vec<Request>,
}

impl Locals for Recorded {
    fn window_is_hung(&self, hwnd: isize) -> bool {
        self.hung.contains(&hwnd)
    }

    fn moved_on(&self, _: isize) -> bool {
        self.moved_on
    }

    fn publication(&mut self, request: Request, _: &Facts) -> Option<Change> {
        let change = self.publications.pop_front()?;
        self.published.push(request);
        Some(change)
    }
}

/// The reducer under test with its recorded local reads.
struct Replay {
    reducer: Reducer,
    locals: Recorded,
}

impl Replay {
    fn new() -> Self {
        Self {
            reducer: Reducer::new(),
            locals: Recorded::default(),
        }
    }

    fn input(&mut self, input: Input, at_ms: u64) -> Vec<Effect> {
        self.reducer.input(input, now(at_ms), &mut self.locals)
    }

    fn tick(&mut self, at_ms: u64) -> Vec<Effect> {
        self.reducer.tick(now(at_ms), &mut self.locals)
    }

    /// Pushes `item`, observed at `at_ms`, from `source`.
    fn push(&mut self, item: Item, source: Source, at_ms: u64) -> Vec<Effect> {
        self.input(
            Input::Entry {
                entry: entry(item, at_ms),
                source,
                read_answer: None,
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
        trace: TraceId::mint(),
        observed_at_us: at_ms * 1_000,
        timing: EventTiming::default(),
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
        request_id,
        query: Query::FocusNow,
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
            } => Some((*request_id, outcome_name(outcome))),
            _ => None,
        })
        .collect()
}

fn outcome_name(outcome: &QueryOutcome) -> &'static str {
    match outcome {
        QueryOutcome::Done(QueryResult::Text(TextReply::WatchEnded)) => "watch ended",
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
        trace_id: TraceId::mint(),
        request_id,
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

/// The commit that reports node `id` as the focus and follows it, owing a
/// terminal's first read when `owes`.
fn focus_change(id: u64, owes: bool) -> Change {
    let focus = node(id, if owes { Role::Terminal } else { Role::Button });
    Change {
        result: Some(OutpostToCore::Event {
            trace_id: TraceId::mint(),
            observed_at_us: 0,
            backend: Backend::Msaa,
            window: None,
            application: None,
            timing: EventTiming::default(),
            event: NormalizedEvent::FocusChanged {
                node: focus.clone(),
                foreground: false,
                ancestors: Vec::new(),
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
            },
        }),
        follow: Some(Follow {
            node: focus.id,
            role: focus.role,
            object: Some(Object::Msaa(9, -4, i32::try_from(id).unwrap_or(0))),
            window: Some(9),
            chain: vec![focus],
            batch: 1,
            redirected: false,
            terminal: owes,
            console: false,
            target: Target::default(),
            owed: owes.then_some(OwedWork::FirstRead(None)),
            trace: TraceId::mint(),
            observed_at_us: 0,
            adopt: true,
        }),
        ..Change::default()
    }
}

/// The node the outpost follows.
fn followed(replay: &Replay) -> Option<NodeId> {
    replay.reducer.facts().follow.tracking.focus
}

/// Whether `effects` abandon the worker in charge.
fn abandons(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::Abandon { .. }))
}

/// A copy of a `WinEvent` focus dropped for UIA in window 0x11 of
/// top-level window 0x10, observed at 1 ms.
fn copy_dropped() -> Change {
    Change {
        gap: vec![GapNote::CopyDropped {
            window: 0x11,
            top_level: 0x10,
            at_us: 1_000,
        }],
        ..Change::default()
    }
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
                answer: GapAnswer::LeavesOpen,
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

#[test]
fn the_gap_records_timer_follows_the_record_in_the_order_it_changes() {
    let mut replay = Replay::new();
    let effects = replay.push(name_change(1), Source::Hook, 1);
    let request = program(&effects).request;
    // The program drops a copy for UIA: the record opens, and its timer is
    // set, in the commit's own transition (f2f8209a).
    let effects = replay.commit(request, copy_dropped(), 2);
    let timers: Vec<Option<u64>> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::GapTimer(due) => Some(*due),
            _ => None,
        })
        .collect();
    assert_eq!(timers, [Some(1_001_000)]);
    replay.finish(request, false, 3);
    assert_eq!(
        replay.reducer.next_timer(now(3)),
        Some(Duration::from_millis(1_001)),
        "the record's bound"
    );
    // A UIA focus's outcome answers it: the timer is cancelled.
    let effects = replay.push(
        Item::Fact(
            DeliveredFact::UiaFocusDropped {
                marker: 9,
                focus: true,
                answer: GapAnswer::Answers(Some(0x10)),
            },
            None,
        ),
        Source::Other,
        4,
    );
    assert!(
        effects
            .iter()
            .any(|effect| matches!(effect, Effect::GapTimer(None)))
    );
    assert_eq!(replay.reducer.next_timer(now(4)), None);
}

#[test]
fn a_drop_that_leaves_the_record_open_has_it_read_at_its_bound() {
    let mut replay = Replay::new();
    let effects = replay.push(name_change(1), Source::Hook, 1);
    let request = program(&effects).request;
    replay.commit(request, copy_dropped(), 2);
    replay.finish(request, false, 3);
    // A lookup that failed fast drops its focus but leaves the record
    // open (e02c7854).
    let effects = replay.push(
        Item::Fact(
            DeliveredFact::UiaFocusDropped {
                marker: 4,
                focus: true,
                answer: GapAnswer::LeavesOpen,
            },
            None,
        ),
        Source::Other,
        4,
    );
    assert!(programs(&effects).is_empty());
    let effects = replay.tick(1_001);
    match &program(&effects).work {
        Work::Entry { entry, .. } => {
            assert!(matches!(entry.item, Item::GapRead { window: 0x11 }));
            assert_eq!(entry.observed_at_us, 1_000, "the record's stamp");
        }
        work => panic!("the gap's read, not {work:?}"),
    }
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

#[test]
fn a_walk_returned_before_its_bound_is_reported_by_its_program() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let request = program(&effects).request;
    replay.input(
        Input::Armed {
            request,
            bound: Duration::from_millis(500),
        },
        2,
    );
    replay.input(
        Input::WalkReturned {
            request,
            past: false,
        },
        100,
    );
    // The bound comes and goes: nothing is published.
    assert_eq!(sent(&replay.tick(500)), Vec::<&OutpostToCore>::new());
    let effects = replay.commit(request, focus_change(1, false), 120);
    assert_eq!(sent(&effects).len(), 1, "the program's report");
    assert_eq!(followed(&replay), Some(NodeId::new(1)));
    assert_eq!(replay.locals.published, []);
}

#[test]
fn a_walk_under_way_at_its_bound_is_published_and_its_programs_report_refused() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let request = program(&effects).request;
    replay.input(
        Input::Armed {
            request,
            bound: Duration::from_millis(500),
        },
        2,
    );
    replay.locals.publications.push_back(focus_change(1, false));
    assert_eq!(
        replay.reducer.next_timer(now(2)),
        Some(Duration::from_millis(500))
    );
    let effects = replay.tick(500);
    assert_eq!(sent(&effects).len(), 1, "the publication");
    assert_eq!(replay.locals.published, [request]);
    assert_eq!(followed(&replay), Some(NodeId::new(1)));
    // The walk returns: its program's own report is not applied (2bd69a85).
    assert!(!replay.reducer.accepts(request));
    replay.input(
        Input::WalkReturned {
            request,
            past: true,
        },
        600,
    );
    assert_eq!(replay.locals.published.len(), 1, "published once");
    // The program ends having reported: the group ends.
    let effects = replay.finish(request, true, 610);
    assert!(programs(&effects).is_empty());
}

#[test]
fn a_walk_returned_past_its_bound_is_published_from_its_progress() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let request = program(&effects).request;
    replay.input(
        Input::Armed {
            request,
            bound: Duration::from_millis(500),
        },
        2,
    );
    replay.locals.publications.push_back(focus_change(1, false));
    // The walk returns past its bound before the bound's timer fired.
    let effects = replay.input(
        Input::WalkReturned {
            request,
            past: true,
        },
        501,
    );
    assert_eq!(sent(&effects).len(), 1, "the publication");
    assert!(!replay.reducer.accepts(request));
    assert_eq!(
        sent(&replay.tick(502)),
        Vec::<&OutpostToCore>::new(),
        "published once"
    );
}

#[test]
fn a_walk_whose_worker_is_abandoned_is_published_first_and_its_sends_after_refused() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let request = program(&effects).request;
    replay.input(
        Input::Armed {
            request,
            bound: Duration::from_secs(20),
        },
        2,
    );
    replay.locals.publications.push_back(focus_change(1, true));
    // The deadline comes before the bound: the walk is published, then its
    // worker abandoned, in one transition (c47d57ae).
    let effects = replay.tick(10_001);
    let order: Vec<&str> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Send(_) => Some("send"),
            Effect::Abandon { .. } => Some("abandon"),
            Effect::Dispatch(_) => Some("dispatch"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["send", "abandon", "dispatch"]);
    // The replacement attempts what the published focus owes first.
    let owed = program(&effects);
    assert!(matches!(owed.work, Work::Owed(_)));
    assert_eq!(owed.request.generation, 1);
    // What the abandoned worker sends afterwards is refused.
    assert!(!replay.reducer.accepts(request));
    assert_eq!(followed(&replay), Some(NodeId::new(1)));
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
    let effects = replay.tick(3 + 10_000);
    assert!(abandons(&effects));
    assert!(programs(&effects).is_empty());
    assert!(replay.reducer.facts().follow.owed.is_none());
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
    let effects = replay.tick(1 + 10_000);
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
            nodes: HashSet::new(),
            anchors: HashSet::new(),
            objects: true,
        },
        10_002,
    );
    assert!(matches!(program(&effects).work, Work::Drop));
}

/// A caret key's watch for `request_id` on node 1, opened at `opened_ms`,
/// polling the console's caret.
fn polling_watch(request_id: u64, opened_ms: u64) -> OpenWatch {
    let opened = Duration::from_millis(opened_ms);
    OpenWatch {
        request_id,
        trace: TraceId::mint(),
        node_id: NodeId::new(1),
        watch: CaretWatch {
            since: None,
            pressed_at_ms: 0,
            unit: TextUnit::Character,
            compare: None,
            landing: false,
            deletes: false,
        },
        opened,
        timing: EventTiming::default(),
        calls: CallCounts::default(),
        poll: Some(Poll::new(opened, 0, 0)),
    }
}

/// A caret key's query, request `request_id`, on node 1.
fn caret_key(request_id: u64) -> Item {
    Item::Query {
        request_id,
        query: Query::Text {
            node_id: NodeId::new(1),
            op: TextOp::AwaitCaret(polling_watch(request_id, 0).watch),
        },
    }
}

/// The change that opens `open`.
fn opens(open: &OpenWatch) -> Change {
    Change {
        watch: Some(WatchChange::Open(open.clone(), wake_for(open))),
        ..Change::default()
    }
}

/// The console caret event of window 9.
fn console_caret() -> Item {
    Item::Msaa {
        kind: WinKind::ConsoleCaret,
        hwnd: 9,
        id_object: 0,
        id_child: 0,
        raiser: 3,
    }
}

/// The console poll `effects` dispatch, for the watch of which request.
fn polls(effects: &[Effect]) -> (Request, u64) {
    let poll = program(effects);
    match poll.work {
        Work::Poll(watch) => (poll.request, watch),
        ref work => panic!("the console poll, not {work:?}"),
    }
}

/// A replay whose caret key, request 7, opened a polling watch at 1 ms,
/// and whose poll was dispatched at 3 ms; the poll's request.
fn polling() -> (Replay, Request) {
    let mut replay = Replay::new();
    let effects = replay.push(caret_key(7), Source::Other, 1);
    let key = program(&effects).request;
    replay.commit(key, opens(&polling_watch(7, 1)), 2);
    let (poll, watch) = polls(&replay.finish(key, false, 3));
    assert_eq!(watch, 7);
    (replay, poll)
}

/// The change of a poll of the watch for request 7 yielding at 5 ms,
/// after one read.
fn yields() -> Change {
    Change {
        watch: Some(WatchChange::Yielded {
            request_id: 7,
            poll: Poll {
                next: Duration::from_millis(11),
                until: Duration::from_millis(51),
            },
            calls: CallCounts {
                uia: 2,
                ..CallCounts::default()
            },
        }),
        ..Change::default()
    }
}

#[test]
fn the_console_poll_is_dispatched_only_when_nothing_else_is_ready() {
    let mut replay = Replay::new();
    let effects = replay.push(caret_key(7), Source::Other, 1);
    let key = program(&effects).request;
    assert!(programs(&replay.push(name_change(1), Source::Hook, 2)).is_empty());
    replay.commit(key, opens(&polling_watch(7, 1)), 3);
    // The name change waits: it goes first.
    let effects = replay.finish(key, false, 4);
    let name = program(&effects);
    assert_eq!(handles(name), (2, None));
    let (_, watch) = polls(&replay.finish(name.request, false, 5));
    assert_eq!(watch, 7);
    assert_eq!(*replay.reducer.queries(), [7].into_iter().collect());
}

#[test]
fn a_program_is_dispatched_beside_the_poll_its_deadline_starting_when_the_poll_ends() {
    let (mut replay, poll) = polling();
    // The poll's deadline is the only timer.
    assert_eq!(
        replay.reducer.next_timer(now(3)),
        Some(Duration::from_millis(3 + 10_000))
    );
    let effects = replay.push(console_caret(), Source::Hook, 4);
    let event = program(&effects).request;
    assert_eq!(replay.reducer.live(), [event, poll]);
    // The event's program waits behind the poll: its deadline and its
    // grace have not started.
    assert_eq!(
        replay.reducer.next_timer(now(4)),
        Some(Duration::from_millis(3 + 10_000))
    );
    replay.commit(poll, yields(), 5);
    assert!(programs(&replay.finish(poll, false, 6)).is_empty());
    // Its grace ends 500 ms after the poll ended.
    assert_eq!(
        replay.reducer.next_timer(now(6)),
        Some(Duration::from_millis(6 + 500))
    );
    assert_eq!(replay.reducer.live(), [event]);
}

#[test]
fn the_poll_is_dispatched_again_after_it_yields() {
    let (mut replay, poll) = polling();
    let effects = replay.push(console_caret(), Source::Hook, 4);
    let event = program(&effects).request;
    replay.commit(poll, yields(), 5);
    let yielded = replay
        .reducer
        .facts()
        .follow
        .caret_watch
        .clone()
        .expect("the watch stays open");
    assert_eq!(
        yielded.poll,
        Some(Poll {
            next: Duration::from_millis(11),
            until: Duration::from_millis(51),
        })
    );
    assert_eq!(yielded.calls.uia, 2);
    replay.finish(poll, false, 5);
    // The console caret event's check finds no move (463f2ef9): it puts
    // the watch back as it was, and the poll goes on from where it was.
    replay.commit(
        event,
        Change {
            watch: Some(WatchChange::Take(NodeId::new(1))),
            ..Change::default()
        },
        6,
    );
    replay.commit(
        event,
        Change {
            watch: Some(WatchChange::PutBack(yielded.clone())),
            ..Change::default()
        },
        7,
    );
    let effects = replay.finish(event, false, 8);
    let (again, watch) = polls(&effects);
    assert_eq!(watch, 7);
    assert_ne!(again, poll);
    let copied = program(&effects)
        .facts
        .follow
        .caret_watch
        .as_ref()
        .and_then(|open| open.poll);
    assert_eq!(copied, yielded.poll);
}

#[test]
fn a_newer_keys_request_ends_the_old_watch_and_its_poll() {
    let (mut replay, poll) = polling();
    let effects = replay.push(caret_key(8), Source::Other, 4);
    let next = program(&effects).request;
    replay.commit(poll, yields(), 5);
    replay.finish(poll, false, 5);
    let effects = replay.commit(
        next,
        Change {
            watch: Some(WatchChange::End("the next caret key replaced it")),
            ..Change::default()
        },
        6,
    );
    assert_eq!(replies(&effects), [(7, "watch ended")]);
    let effects = replay.commit(
        next,
        Change {
            result: Some(done(8)),
            ..Change::default()
        },
        7,
    );
    assert_eq!(replies(&effects), [(8, "done")]);
    assert!(programs(&replay.finish(next, false, 8)).is_empty());
    assert!(replay.reducer.queries().is_empty());
}

#[test]
fn the_poll_ends_its_watch_by_its_answer_or_at_its_last_read() {
    let (mut replay, poll) = polling();
    let answer = OutpostToCore::Reply {
        trace_id: TraceId::mint(),
        request_id: 7,
        outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Done)),
        timing: EventTiming::default(),
    };
    let effects = replay.commit(
        poll,
        Change {
            aside: Some((7, answer)),
            watch: Some(WatchChange::Answered(7)),
            ..Change::default()
        },
        10,
    );
    assert_eq!(replies(&effects), [(7, "done")]);
    assert!(replay.reducer.facts().follow.caret_watch.is_none());
    assert!(programs(&replay.finish(poll, false, 11)).is_empty());

    let effects = replay.push(caret_key(9), Source::Other, 20);
    let key = program(&effects).request;
    replay.commit(key, opens(&polling_watch(9, 20)), 21);
    let (poll, _) = polls(&replay.finish(key, false, 22));
    let effects = replay.commit(
        poll,
        Change {
            watch: Some(WatchChange::Ended(9, "its poll's bound passed")),
            ..Change::default()
        },
        70,
    );
    assert_eq!(replies(&effects), [(9, "watch ended")]);
    assert!(programs(&replay.finish(poll, false, 71)).is_empty());
    assert!(replay.reducer.queries().is_empty());
}

#[test]
fn an_abandoned_poll_ends_its_watch_and_the_program_behind_it_goes_to_the_replacement() {
    let (mut replay, poll) = polling();
    let effects = replay.push(console_caret(), Source::Hook, 4);
    let behind = program(&effects).request;
    // The poll's read hangs past its deadline.
    let effects = replay.tick(3 + 10_000);
    assert!(abandons(&effects));
    assert_eq!(replies(&effects), [(7, "abandoned")]);
    assert!(replay.reducer.facts().follow.caret_watch.is_none());
    let again = program(&effects);
    assert_eq!(again.request.generation, 1);
    assert_eq!(handles(again), (4, None));
    assert!(!replay.reducer.accepts(poll));
    assert!(!replay.reducer.accepts(behind));
    assert!(replay.reducer.accepts(again.request));
}

#[test]
fn a_caret_read_after_an_abandoned_text_call_waits_for_nothing() {
    let mut replay = Replay::new();
    let effects = replay.push(
        Item::Query {
            request_id: 4,
            query: Query::Text {
                node_id: NodeId::new(1),
                op: TextOp::ReadCaret,
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
    let effects = replay.tick(1 + 2_000);
    assert_eq!(replies(&effects), [(4, "abandoned")]);
    let read = program(&effects);
    assert_eq!(read.request.generation, 1);
    assert_eq!(handles(read).0, 2);
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
    let effects = replay.tick(4 + 10_000);
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
    let effects = replay.tick(4 + 10_000);
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
    let effects = replay.input(Input::Cancel(4), 10_022);
    assert_eq!(replies(&effects), [(4, "not started")]);
    assert!(programs(&replay.finish(busy, false, 10_023)).is_empty());
    assert!(replay.reducer.queries().is_empty());

    // A caret key's watch, ended by the next key.
    let effects = replay.push(caret_key(5), Source::Other, 10_030);
    let key = program(&effects).request;
    replay.commit(key, opens(&polling_watch(5, 10_030)), 10_031);
    let effects = replay.finish(key, false, 10_032);
    assert_eq!(replies(&effects), []);
    let (poll, _) = polls(&effects);
    let effects = replay.push(caret_key(6), Source::Other, 10_040);
    let next = program(&effects).request;
    assert_eq!(replies(&replay.finish(poll, false, 10_040)), []);
    let effects = replay.commit(
        next,
        Change {
            watch: Some(WatchChange::End("the next caret key replaced it")),
            ..Change::default()
        },
        10_041,
    );
    assert_eq!(replies(&effects), [(5, "watch ended")]);
    assert_eq!(*replay.reducer.queries(), [6].into_iter().collect());
}

#[test]
fn a_program_waiting_on_a_window_the_user_left_is_abandoned_once_its_grace_ends() {
    let mut replay = Replay::new();
    let effects = replay.push(name_change(1), Source::Hook, 1);
    assert_eq!(program(&effects).request.generation, 0);
    replay.locals.moved_on = true;
    // Before the grace ends nothing is judged.
    assert!(!abandons(&replay.tick(400)));
    assert_eq!(
        replay.reducer.next_timer(now(400)),
        Some(Duration::from_millis(501))
    );
    assert!(abandons(&replay.tick(501)));
}

#[test]
fn a_foreground_fact_after_the_grace_judges_the_moved_on_rule_again() {
    let mut replay = Replay::new();
    replay.push(name_change(1), Source::Hook, 1);
    // At the grace's end the user has not moved on.
    assert!(!abandons(&replay.tick(501)));
    // A wake for anything else judges nothing.
    replay.locals.moved_on = true;
    assert!(!abandons(&replay.push(name_change(2), Source::Hook, 600)));
    // A foreground fact does.
    let effects = replay.push(
        Item::Fact(DeliveredFact::Foreground { hwnd: 8 }, None),
        Source::Listener,
        700,
    );
    assert!(abandons(&effects));
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

/// The commit of `change` alone.
fn verdict_commit(change: VerdictChange) -> Change {
    Change {
        verdicts: vec![change],
        ..Change::default()
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
    let effects = replay.commit(request, verdict_commit(probe(Probed::Uia)), 2);
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
    assert!(abandons(&replay.tick(1 + 10_000)));
    let effects = replay.commit(dead, verdict_commit(probe(Probed::Uia)), 10_002);
    assert_eq!(verdict_of(&replay.reducer.facts().verdicts, 1_000), None);
    assert!(effects.is_empty());
}

#[test]
fn a_dead_generations_timed_out_probe_is_recorded() {
    let mut replay = Replay::new();
    let effects = replay.push(msaa_focus(1, 1), Source::Hook, 1);
    let dead = program(&effects).request;
    assert!(abandons(&replay.tick(1 + 10_000)));
    let effects = replay.commit(
        dead,
        verdict_commit(probe(Probed::TimedOut(10_502_000))),
        10_002,
    );
    assert_eq!(
        verdict_of(&replay.reducer.facts().verdicts, 1_000),
        Some(Probed::TimedOut(10_502_000))
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
    replay.commit(
        request,
        verdict_commit(probe(Probed::TimedOut(3_501_000))),
        3_001,
    );
    let effects = replay.tick(1 + 10_000);
    assert!(abandons(&effects));
    assert_eq!(
        verdict_of(&replay.reducer.facts().verdicts, 1_000),
        Some(Probed::TimedOut(3_501_000))
    );
    assert_eq!(verdict_copies(&effects).len(), 0);
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
        trace: TraceId::mint(),
        observed_at_us: 1_000,
        read: ReportCut {
            caret: 1,
            selection: None,
            moved: false,
            read_at_ms: 1,
            last: KeptSelection {
                ends: (0, 5),
                collapsed: false,
                text: None,
                report: 1,
            },
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

/// The finish `program` is: what it finishes, and whether the reducer
/// found the edit control's value change waiting.
fn finishes(program: &Program) -> (&'static str, bool) {
    match &program.work {
        Work::Finish { cut, waiting } => {
            let what = match cut.finish {
                Finish::Report { .. } => "report",
                Finish::Key { .. } => "key",
                Finish::Watch { .. } => "watch",
            };
            (what, *waiting)
        }
        work => panic!("a finish, not {work:?}"),
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

/// The open watch's check, cut at its value change check, is finished by
/// a program that takes over the read part's deadline and the watch's
/// query, which the read part took out of the follow state: abandoned at
/// the read part's deadline, it answers that query "abandoned".
#[test]
fn a_cut_watch_check_is_finished_within_its_reads_deadline_and_query() {
    let mut replay = Replay::new();
    let key = program(&replay.push(caret_key(7), Source::Other, 1)).request;
    let open = OpenWatch {
        poll: None,
        ..polling_watch(7, 1)
    };
    replay.commit(key, opens(&open), 2);
    assert!(programs(&replay.finish(key, false, 3)).is_empty());
    let read = program(&replay.push(edit_caret(), Source::Hook, 10)).request;
    replay.commit(
        read,
        Change {
            watch: Some(WatchChange::Take(NodeId::new(1))),
            ..Change::default()
        },
        11,
    );
    let finish = cut(Finish::Watch {
        open: Box::new(open),
        read: AnswerCut {
            caret: 1,
            selection: None,
            read_at_ms: 11,
            line: None,
            unit: TextUnit::Character,
            same_line: None,
            compared: KeptSelection {
                ends: (0, 5),
                collapsed: false,
                text: None,
                report: 1,
            },
        },
    });
    let effects = replay.finish_cut(read, finish, 12);
    let finish = program(&effects);
    assert_eq!(finishes(finish), ("watch", false));
    assert!(replay.tick(10 + 9_999).is_empty());
    let effects = replay.tick(10 + 10_000);
    assert!(abandons(&effects));
    assert_eq!(replies(&effects), [(7, "abandoned")]);
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
        Item::IdleQuery {
            round: 5,
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
            noted_caret: Some((NodeId::new(4), TraceId::mint(), 1_000, timing)),
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
