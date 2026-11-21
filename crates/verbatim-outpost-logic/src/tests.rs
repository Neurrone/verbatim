//! The reducer's replay tests (`books/dev/src/explanation/design/ownership.md`): recorded inputs given to the reducer, with its
//! asks answered as the shell answers them, and the effects it answers with asserted. Each replays one
//! ordering the outpost's threads can produce, at the times given, so an
//! ordering that once needed a real bound, a real deadline or a race to
//! reproduce is one test here.

use std::time::Duration;

use verbatim_model::hung::WindowState;
use verbatim_model::protocol::{EventTiming, OutpostToCore, UiaSnapshotFact};
use verbatim_model::tell::{Hung, WindowFacet};
use verbatim_model::{Pid, TraceId, WindowHandle};

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
                panicked: false,
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
        window: WindowState::Responsive { thread: 0 },
    }
}

/// A console update of window `window`.
fn console_update(window: isize) -> Item {
    Item::Msaa {
        kind: WinKind::ConsoleUpdate,
        hwnd: window,
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
        }
        .into(),
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
    let effects = replay.input(Input::Released { objects: true }, 11_002);
    assert!(matches!(program(&effects).work, Work::Drop));
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

fn settle() -> Item {
    Item::Barrier {
        asker: Asker::Settle,
    }
}

/// What `effects` answer as settles and dispatch, in order.
fn settled_and_dispatches(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Settled => Some("settled".to_owned()),
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
        settled_and_dispatches(&replay.push(settle(), 2)),
        [] as [String; 0]
    );
    replay.push(console_update(7), 3);
    let effects = replay.finish(busy, 4);
    assert_eq!(settled_and_dispatches(&effects), ["settled", "dispatch"]);
    assert_eq!(handles(program(&effects)), 3);
    // A settle behind the program in flight when the outpost closes is
    // dropped with the intake, unanswered.
    let next = program(&effects).request;
    replay.push(settle(), 5);
    let effects = replay.input(Input::Close, 6);
    assert_eq!(settled_and_dispatches(&effects), [] as [String; 0]);
    assert_eq!(
        settled_and_dispatches(&replay.finish(next, 7)),
        [] as [String; 0]
    );
}

/// A property change of the followed focus, its element no window itself.
fn property_change() -> Item {
    Item::Uia(UiaEvent {
        kind: UiaKind::Property {
            id: 30005,
            text_pattern: false,
            automation_id: None,
            window: Box::new(verbatim_model::tell::TellWindow::NoWindow),
        },
        parts: UiaSnapshotFact {
            runtime_id: vec![42, 1],
            role: verbatim_model::Role::ProgressBar,
            name: None,
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
        hwnd: 0,
        element: None,
    })
}

/// The report of window `window` on thread `thread`, read hung at `at_ms`.
fn hung(window: u64, thread: u32, at_ms: u64) -> Hung {
    let facet = WindowFacet {
        handle: WindowHandle(window),
        class: "Static".to_owned(),
        process: Pid(4),
        thread,
    };
    Hung {
        window: facet.clone(),
        root: facet.clone(),
        root_owner: facet,
        at_us: at_ms * 1_000,
    }
}

/// `item` observed at `at_ms`, its window read as `window`.
fn entry_in(item: Item, at_ms: u64, window: WindowState) -> Input {
    Input::Entry(Entry {
        window,
        ..entry(item, at_ms)
    })
}

/// What `effects` do, in order: each message sent and each program
/// dispatched.
fn reports_and_dispatches(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .map(|effect| match effect {
            Effect::Send(message) => match &**message {
                OutpostToCore::Hung(hung) => {
                    format!("report {} at {}", hung.window.handle.0, hung.at_us / 1_000)
                }
                other => format!("{other:?}"),
            },
            Effect::Dispatch(program) => format!("dispatch {}", handles(program)),
            other => format!("{other:?}"),
        })
        .collect()
}

/// Two property events of the followed focus arrive while its window's
/// thread is hung: the first is reported before its own tell, which its
/// program commits once dispatched, and the second only handled. A
/// responsive event unmarks the thread, so the next hung one is reported
/// again. Each event is handled as any other.
#[test]
fn a_property_event_from_a_hung_window_reports_it_once_before_its_tell() {
    let mut replay = Replay::new();
    let mut steps = Vec::new();
    let mut step = |replay: &mut Replay, input: Input, at_ms: u64| {
        let effects = replay.input(input, at_ms);
        steps.extend(reports_and_dispatches(&effects));
        let request = programs(&effects).first().map(|program| program.request);
        if let Some(request) = request {
            steps.extend(reports_and_dispatches(&replay.finish(request, at_ms)));
        }
    };
    let window = || WindowState::Hung(hung(11, 5, 1));
    step(&mut replay, entry_in(property_change(), 1, window()), 1);
    step(
        &mut replay,
        entry_in(property_change(), 2, WindowState::Hung(hung(11, 5, 2))),
        2,
    );
    step(
        &mut replay,
        entry_in(property_change(), 3, WindowState::Responsive { thread: 5 }),
        3,
    );
    step(
        &mut replay,
        entry_in(property_change(), 4, WindowState::Hung(hung(11, 5, 4))),
        4,
    );
    assert_eq!(
        steps,
        [
            "report 11 at 1",
            "dispatch 1",
            "dispatch 2",
            "dispatch 3",
            "report 11 at 4",
            "dispatch 4"
        ]
    );
}

/// A console's event heard while its console window is hung is reported
/// as a property event's is, and an event with no window to read, thread
/// 0, leaves the mark.
#[test]
fn a_console_event_from_a_hung_console_reports_it() {
    let mut replay = Replay::new();
    let effects = replay.input(
        entry_in(console_update(21), 1, WindowState::Hung(hung(21, 8, 1))),
        1,
    );
    assert_eq!(
        reports_and_dispatches(&effects),
        ["report 21 at 1", "dispatch 1"]
    );
    let effects = replay.finish(program(&effects).request, 2);
    assert_eq!(reports_and_dispatches(&effects), [] as [String; 0]);
    let effects = replay.input(
        entry_in(property_change(), 3, WindowState::Responsive { thread: 0 }),
        3,
    );
    assert_eq!(reports_and_dispatches(&effects), ["dispatch 3"]);
    let effects = replay.finish(program(&effects).request, 4);
    assert_eq!(reports_and_dispatches(&effects), [] as [String; 0]);
    let effects = replay.input(
        entry_in(console_update(21), 5, WindowState::Hung(hung(21, 8, 5))),
        5,
    );
    assert_eq!(reports_and_dispatches(&effects), ["dispatch 5"]);
}
