//! Core's requests through the reducer
//! (`docs/design/readers-tell-core-asks-build1.md`, Table 14): each runs
//! on the worker in charge ahead of the outpost's own work, is answered
//! once, and is cancelled as its reason says.

use verbatim_model::WindowHandle;
use verbatim_model::batch::{Batch, Done, Op, Placement, Step, StepOutcome, Subject};
use verbatim_model::request::{
    Answer, Ask, Cancel, CancelReason, Outcome, Reply, RequestNumber, WindowVerdict,
};

use super::*;

fn ask(number: u64) -> Input {
    Input::Ask(Box::new(Ask {
        number: RequestNumber(number),
        window: WindowHandle(1),
        verdict: WindowVerdict::Msaa,
        request: verbatim_model::batch::Batch {
            steps: vec![verbatim_model::batch::Step {
                op: verbatim_model::batch::Op::FocusRead {
                    subject: verbatim_model::batch::Subject::Focus,
                    known: Vec::new(),
                    set: Vec::new(),
                    details: verbatim_model::TextDetails::NONE,
                },
                when: None,
            }],
            placement: verbatim_model::batch::Placement::Queue,
            spare: None,
        }
        .into(),
    }))
}

fn cancel(number: u64, reason: CancelReason) -> Input {
    Input::CancelAsk(Cancel {
        number: RequestNumber(number),
        reason,
    })
}

fn result(number: u64) -> Answer {
    Answer {
        number: RequestNumber(number),
        outcome: Outcome::Result(Reply::Steps(vec![StepOutcome::Done(Done::Follow)])),
        probes: Vec::new(),
        detail: String::new(),
    }
}

fn ended(number: u64, detail: &str) -> Answer {
    Answer {
        number: RequestNumber(number),
        outcome: Outcome::Ended,
        probes: Vec::new(),
        detail: detail.to_owned(),
    }
}

/// The program each dispatch in `effects` runs: Core's request by number,
/// or another program by its kind.
fn dispatched(effects: &[Effect]) -> Vec<(Request, String)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Dispatch(program) => Some((
                program.request,
                match &program.work {
                    Work::Ask(ask) => format!("ask {}", ask.number.0),
                    Work::Entry { .. } => "entry".to_owned(),
                    Work::Forget(windows) => format!(
                        "forget {:?}",
                        windows.iter().map(|window| window.0).collect::<Vec<_>>()
                    ),
                    _ => "other".to_owned(),
                },
            )),
            _ => None,
        })
        .collect()
}

/// The answers to Core's requests `effects` send.
fn answers(effects: &[Effect]) -> Vec<Answer> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Send(message) => match &**message {
                OutpostToCore::Answer(answer) => Some(answer.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn finished(request: Request) -> Input {
    Input::Finished {
        request,
        panicked: false,
    }
}

/// The outpost's own work: a console's update it heard.
fn own_work() -> Input {
    Input::Entry(entry(console_update(1), 0))
}

/// A request runs at once with nothing in flight, and one sent while a
/// program runs waits, then goes ahead of the outpost's own work queued
/// before it.
#[test]
fn a_request_runs_ahead_of_the_outposts_own_work() {
    let mut reducer = Reducer::new();
    let first = dispatched(&reducer.input(ask(1), now(0)));
    assert_eq!(
        first
            .iter()
            .map(|(_, what)| what.as_str())
            .collect::<Vec<_>>(),
        ["ask 1"]
    );
    assert_eq!(dispatched(&reducer.input(own_work(), now(1))), Vec::new());
    assert_eq!(dispatched(&reducer.input(ask(2), now(2))), Vec::new());
    let request = first[0].0;
    let effects = reducer.input(
        Input::Answered {
            request,
            answer: Box::new(result(1)),
        },
        now(3),
    );
    assert_eq!(answers(&effects), [result(1)]);
    let next = dispatched(&reducer.input(finished(request), now(4)));
    assert_eq!(
        next.iter()
            .map(|(_, what)| what.as_str())
            .collect::<Vec<_>>(),
        ["ask 2"]
    );
}

/// A running request cancelled because the foreground moved away is
/// answered abandoned, its worker abandoned and replaced, and its late
/// answer refused; the next request runs on the replacement.
#[test]
fn a_running_request_cancelled_for_the_foreground_abandons_its_worker() {
    let mut reducer = Reducer::new();
    let request = dispatched(&reducer.input(ask(1), now(0)))[0].0;
    reducer.input(ask(2), now(1));
    let effects = reducer.input(cancel(1, CancelReason::Foreground), now(2));
    assert!(effects.iter().any(|effect| matches!(
        effect,
        Effect::Abandon {
            dead: 0,
            replacement: 1,
            ..
        }
    )));
    assert_eq!(answers(&effects), [ended(1, "abandoned")]);
    let next = dispatched(&effects);
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].1, "ask 2");
    assert_eq!(next[0].0.generation, 1);
    let late = reducer.input(
        Input::Answered {
            request,
            answer: Box::new(result(1)),
        },
        now(3),
    );
    assert_eq!(answers(&late), Vec::new());
}

/// A running request cancelled for a hang, its due time or a withdrawal
/// runs on, and its answer is sent when it comes.
#[test]
fn a_running_request_cancelled_otherwise_runs_on_and_answers() {
    for reason in [
        CancelReason::Hung,
        CancelReason::DueTime,
        CancelReason::Withdrawn,
    ] {
        let mut reducer = Reducer::new();
        let request = dispatched(&reducer.input(ask(1), now(0)))[0].0;
        let effects = reducer.input(cancel(1, reason), now(1));
        assert!(effects.is_empty(), "{reason:?}: {effects:?}");
        let effects = reducer.input(
            Input::Answered {
                request,
                answer: Box::new(result(1)),
            },
            now(2),
        );
        assert_eq!(answers(&effects), [result(1)], "{reason:?}");
    }
}

/// A queued request cancelled is answered cancelled and never runs.
#[test]
fn a_queued_request_cancelled_never_runs() {
    let mut reducer = Reducer::new();
    let request = dispatched(&reducer.input(ask(1), now(0)))[0].0;
    reducer.input(ask(2), now(1));
    let effects = reducer.input(cancel(2, CancelReason::Withdrawn), now(2));
    assert_eq!(answers(&effects), [ended(2, "cancelled")]);
    let effects = reducer.input(finished(request), now(3));
    assert_eq!(
        answers(&effects),
        [ended(1, "the outpost failed handling the request")],
        "the first ended unanswered"
    );
    assert_eq!(dispatched(&effects), Vec::new());
}

/// A request whose program ends without answering is answered failed,
/// once; one that answered is not answered again as its program ends.
#[test]
fn a_request_ended_unanswered_is_answered_failed_once() {
    let mut reducer = Reducer::new();
    let request = dispatched(&reducer.input(ask(1), now(0)))[0].0;
    assert_eq!(
        answers(&reducer.input(finished(request), now(1))),
        [ended(1, "the outpost failed handling the request")]
    );
    assert_eq!(answers(&reducer.input(finished(request), now(2))), []);
    let request = dispatched(&reducer.input(ask(2), now(3)))[0].0;
    let effects = reducer.input(
        Input::Answered {
            request,
            answer: Box::new(result(2)),
        },
        now(4),
    );
    assert_eq!(answers(&effects), [result(2)]);
    assert_eq!(
        answers(&reducer.input(finished(request), now(5))),
        [],
        "an answered request is not answered again"
    );
}

/// At shutdown the running request's answer is not sent, nor a failure
/// as its program ends, and the request queued behind it never runs.
#[test]
fn the_running_requests_answer_after_shutdown_is_not_sent() {
    let mut reducer = Reducer::new();
    let request = dispatched(&reducer.input(ask(1), now(0)))[0].0;
    reducer.input(ask(2), now(1));
    reducer.input(Input::Close, now(2));
    let effects = reducer.input(
        Input::Answered {
            request,
            answer: Box::new(result(1)),
        },
        now(3),
    );
    assert_eq!(answers(&effects), []);
    let effects = reducer.input(finished(request), now(4));
    assert_eq!(answers(&effects), []);
    assert_eq!(dispatched(&effects), []);
}

/// A request that arrives as the outpost shuts down is not run.
#[test]
fn a_request_after_shutdown_is_not_run() {
    let mut reducer = Reducer::new();
    reducer.input(Input::Close, now(0));
    assert_eq!(dispatched(&reducer.input(ask(1), now(1))), Vec::new());
}

fn follow(number: u64, window: u64) -> Input {
    Input::Ask(Box::new(Ask {
        number: RequestNumber(number),
        window: WindowHandle(window),
        verdict: WindowVerdict::Uia,
        request: one(Op::Follow(None)).into(),
    }))
}

/// A batch of `op` alone.
fn one(op: Op) -> Batch {
    Batch {
        steps: vec![Step { op, when: None }],
        placement: Placement::Queue,
        spare: None,
    }
}

/// A `Follow` sent while an older one is queued replaces it, which is
/// answered "ended or stopped" at once (Table 10).
#[test]
fn a_newer_follow_answers_the_queued_one_ended() {
    let mut reducer = Reducer::new();
    let _ = reducer.input(ask(1), now(0));
    assert_eq!(answers(&reducer.input(follow(2, 1), now(1))), []);
    assert_eq!(
        answers(&reducer.input(follow(3, 1), now(2))),
        [ended(2, "replaced by a newer Follow")]
    );
}

/// Core's request `number` asked on `window`, of `request`.
fn asked_on(number: u64, window: u64, request: verbatim_model::request::Question) -> Input {
    Input::Ask(Box::new(Ask {
        number: RequestNumber(number),
        window: WindowHandle(window),
        verdict: WindowVerdict::Msaa,
        request,
    }))
}

/// A read of the application's current focus, as the re-take and the
/// recovery read ask it.
fn current_focus() -> verbatim_model::batch::Batch {
    verbatim_model::batch::Batch {
        steps: vec![verbatim_model::batch::Step {
            op: verbatim_model::batch::Op::FocusRead {
                subject: verbatim_model::batch::Subject::Focus,
                known: Vec::new(),
                set: Vec::new(),
                details: verbatim_model::TextDetails::NONE,
            },
            when: None,
        }],
        placement: verbatim_model::batch::Placement::Queue,
        spare: None,
    }
}

/// A read of `subject`.
fn read_object(subject: Subject) -> Batch {
    one(Op::Read {
        subject,
        set: Vec::new(),
    })
}

/// A candidate's read at MSAA ids in `window`.
fn run_in(window: u64) -> verbatim_model::batch::Batch {
    verbatim_model::batch::Batch {
        steps: vec![verbatim_model::batch::Step {
            op: verbatim_model::batch::Op::FocusRead {
                subject: verbatim_model::batch::Subject::Msaa {
                    ids: verbatim_model::request::MsaaIds {
                        window: WindowHandle(window),
                        object: -4,
                        child: 0,
                    },
                    redirect: true,
                },
                known: Vec::new(),
                set: Vec::new(),
                details: verbatim_model::TextDetails::NONE,
            },
            when: None,
        }],
        placement: verbatim_model::batch::Placement::Queue,
        spare: None,
    }
}

/// The one program `effects` dispatched ended.
fn ended_program(effects: &[Effect]) -> Input {
    let [(request, _)] = dispatched(effects)[..] else {
        panic!("one program dispatched: {effects:?}");
    };
    finished(request)
}

/// A window destroyed while a program runs (Table 14, 2026-11-19): the
/// queued reads its MSAA ids name, a read by ids and a run's candidate,
/// are answered "not there" at once; a re-take's and a recovery read's
/// current focus, a `Follow` and a read of a node, all asked on the
/// window, stay queued; when the program ends, the window's forget runs
/// first, then they run in order, and a request Core sent after the
/// destroy runs after the forget.
#[test]
fn a_destroyed_window_answers_the_reads_its_ids_name_and_forgets_it_before_the_rest() {
    let mut reducer = Reducer::new();
    let running = reducer.input(ask(1), now(0));
    assert_eq!(dispatched(&running)[0].1, "ask 1");
    let node = verbatim_model::NodeId::new(4);
    for (number, request) in [
        (2, current_focus().into()),
        (3, current_focus().into()),
        (4, one(Op::Follow(None)).into()),
        (5, read_object(Subject::Node(node)).into()),
        (
            6,
            read_object(Subject::Msaa {
                ids: verbatim_model::request::MsaaIds {
                    window: WindowHandle(7),
                    object: -4,
                    child: 0,
                },
                redirect: false,
            })
            .into(),
        ),
        (7, run_in(7).into()),
    ] {
        assert_eq!(reducer.input(asked_on(number, 7, request), now(1)).len(), 0);
    }
    let effects = reducer.input(Input::WindowDestroyed(WindowHandle(7)), now(2));
    assert_eq!(
        answers(&effects)
            .into_iter()
            .map(|answer| (answer.number.0, answer.outcome))
            .collect::<Vec<_>>(),
        [(6, Outcome::NotThere), (7, Outcome::NotThere)]
    );
    assert_eq!(dispatched(&effects), []);
    assert_eq!(dispatched(&reducer.input(ask(8), now(3))), []);
    let mut order = Vec::new();
    let mut effects = reducer.input(ended_program(&running), now(4));
    for at in 5..11 {
        let [(_, work)] = &dispatched(&effects)[..] else {
            break;
        };
        order.push(work.clone());
        effects = reducer.input(ended_program(&effects), now(at));
    }
    assert_eq!(
        order,
        ["forget [7]", "ask 2", "ask 3", "ask 4", "ask 5", "ask 8"]
    );
}

/// The destroys the shell drains together, twenty in one feed, are
/// forgotten by one program, dispatched at the shell's tick after the
/// drain, the windows in the order named, a window named twice once.
#[test]
fn a_burst_of_destroys_is_forgotten_by_one_program() {
    let mut reducer = Reducer::new();
    for window in 1..=20 {
        assert_eq!(
            dispatched(&reducer.input(Input::WindowDestroyed(WindowHandle(window)), now(0))),
            []
        );
    }
    assert_eq!(
        dispatched(&reducer.input(Input::WindowDestroyed(WindowHandle(3)), now(0))),
        []
    );
    let effects = reducer.tick(now(1));
    assert_eq!(
        dispatched(&effects)
            .into_iter()
            .map(|(_, work)| work)
            .collect::<Vec<_>>(),
        [format!("forget {:?}", (1..=20).collect::<Vec<u64>>())]
    );
    assert_eq!(
        dispatched(&reducer.input(ended_program(&effects), now(2))),
        []
    );
}
