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

#[test]
fn the_console_polls_ticks_come_while_its_watch_is_open() {
    let mut replay = Replay::new();
    let effects = replay.push(caret_key(7), Source::Other, 1);
    let key = program(&effects).request;
    let open = polling_watch(7, 1);
    let first_read = open.poll.map(|poll| poll.next);
    replay.commit(key, opens(&open), 2);
    replay.finish(key, false, 3);
    assert!(
        replay.reducer.queries().contains(&7),
        "the watch answers later"
    );
    assert_eq!(replay.reducer.next_timer(now(3)), first_read);
    // A console caret event whose check finds no move leaves the poll
    // reading (463f2ef9): it puts the watch back as it was.
    let effects = replay.push(
        Item::Msaa {
            kind: WinKind::ConsoleCaret,
            hwnd: 9,
            id_object: 0,
            id_child: 0,
            raiser: 3,
        },
        Source::Hook,
        4,
    );
    let event = program(&effects).request;
    replay.commit(
        event,
        Change {
            watch: Some(WatchChange::Take(NodeId::new(1))),
            ..Change::default()
        },
        5,
    );
    replay.commit(
        event,
        Change {
            watch: Some(WatchChange::PutBack(open)),
            ..Change::default()
        },
        5,
    );
    replay.finish(event, false, 5);
    // The tick is not lost: the poll's read is the next wake.
    assert_eq!(replay.reducer.next_timer(now(5)), first_read);
    let effects = replay.tick(6);
    match &program(&effects).work {
        Work::Entry { entry, .. } => assert!(matches!(entry.item, Item::Wake)),
        work => panic!("the poll's read, not {work:?}"),
    }
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
    assert_eq!(replies(&replay.finish(key, false, 10_032)), []);
    let effects = replay.push(caret_key(6), Source::Other, 10_040);
    let next = program(&effects).request;
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
