//! Core's requests through the reducer
//! (`docs/design/readers-tell-core-asks-build1.md`, Table 14): each runs
//! on the worker in charge ahead of the outpost's own work, is answered
//! once, and is cancelled as its reason says.

use verbatim_model::WindowHandle;
use verbatim_model::request::{
    Answer, Ask, Cancel, CancelReason, Outcome, Reply, Request as CoreRequest, RequestNumber,
    WindowVerdict,
};

use super::*;

fn ask(number: u64) -> Input {
    Input::Ask(Box::new(Ask {
        number: RequestNumber(number),
        window: WindowHandle(1),
        verdict: WindowVerdict::Msaa,
        request: CoreRequest::CurrentFocus {
            set: Vec::new(),
            known: Vec::new(),
            details: verbatim_model::TextDetails::NONE,
            depth: verbatim_model::request::Depth::Full,
        },
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
        outcome: Outcome::Result(Reply::Followed),
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
        reported: false,
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
        request: CoreRequest::Follow(verbatim_model::request::Follow { node: None }),
    }))
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

/// A window destroyed answers Core's requests queued on it "not there",
/// at once, and leaves the one running (Table 14).
#[test]
fn a_destroyed_window_answers_the_requests_queued_on_it() {
    let mut reducer = Reducer::new();
    let _ = reducer.input(follow(1, 7), now(0));
    let _ = reducer.input(follow(2, 7), now(1));
    let effects = reducer.input(Input::WindowDestroyed(WindowHandle(7)), now(2));
    assert_eq!(
        answers(&effects)
            .into_iter()
            .map(|answer| (answer.number.0, answer.outcome))
            .collect::<Vec<_>>(),
        [(2, Outcome::NotThere)]
    );
}
