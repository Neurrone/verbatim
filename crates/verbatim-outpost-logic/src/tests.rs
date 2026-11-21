//! The reducer's replay tests (`books/dev/src/explanation/design/ownership.md`): recorded inputs given to the reducer, with its
//! asks answered as the shell answers them, and the effects it answers with asserted. Each replays one
//! ordering the outpost's threads can produce, at the times given, so an
//! ordering that once needed a real bound, a real deadline or a race to
//! reproduce is one test here.

use std::collections::BTreeSet;
use std::time::Duration;

use verbatim_model::TraceId;
use verbatim_model::protocol::{EventTiming, OutpostToCore};

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
        Work::Entry(entry) => entry.observed_at_us / 1_000,
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

/// Core's request `number`, which runs as a program on the worker.
fn core_ask(number: u64) -> Input {
    Input::Ask(Box::new(verbatim_model::request::Ask {
        number: verbatim_model::request::RequestNumber(number),
        window: verbatim_model::WindowHandle(9),
        verdict: verbatim_model::request::WindowVerdict::Msaa,
        request: verbatim_model::request::Request::CurrentFocus {
            hidden: verbatim_model::request::HiddenRoot::Check,
            set: Vec::new(),
            known: Vec::new(),
            details: verbatim_model::TextDetails::NONE,
            depth: verbatim_model::request::FocusDepth::Full,
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

/// Whether `effects` abandon the worker in charge.
fn abandons(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::Abandon { .. }))
}

#[test]
fn a_release_waits_for_the_program_in_flight() {
    let mut replay = Replay::new();
    let _ = replay.input(core_ask(1), 1);
    let effects = replay.push(
        Item::NodesHeld {
            nodes: vec![],
            anchors: vec![],
            told: vec![],
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
    // abandoned, the release is made.
    let effects = replay.input(cancel_for_the_foreground(1), 11_000);
    assert!(abandons(&effects));
    assert_eq!(
        effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Release { .. }))
            .count(),
        1
    );
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
    let effects = replay.push(console_update(1), 1);
    let first = program(&effects).request;
    replay.push(console_update(2), 2);
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
    replay.push(console_update(7), 3);
    // The query is reached as the program ends: it is answered before the
    // entry that arrived after it is dispatched, not once that is handled.
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
    // its own, and the entry after it is not waited for.
    assert_eq!(
        settled_and_dispatches(&replay.push(settle(1), 2)),
        [] as [String; 0]
    );
    replay.push(console_update(7), 3);
    assert_eq!(replay.reducer.settles(), [1].into_iter().collect());
    let effects = replay.finish(busy, 4);
    assert_eq!(settled_and_dispatches(&effects), ["settled 1", "dispatch"]);
    assert_eq!(handles(program(&effects)), 3);
    assert!(replay.reducer.settles().is_empty());
    // A settle behind the program in flight when the outpost closes is
    // dropped with the intake, unanswered.
    let next = program(&effects).request;
    replay.push(settle(2), 5);
    assert_eq!(replay.reducer.settles(), [2].into_iter().collect());
    let effects = replay.input(Input::Close, 6);
    assert_eq!(settled_and_dispatches(&effects), [] as [String; 0]);
    assert!(replay.reducer.settles().is_empty());
    assert_eq!(
        settled_and_dispatches(&replay.finish(next, 7)),
        [] as [String; 0]
    );
}
