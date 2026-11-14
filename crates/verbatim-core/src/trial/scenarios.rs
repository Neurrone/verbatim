//! One set of scenarios, each run against all three forms with the same
//! exact expected effects in the same order.

use super::async_form::AsyncCore;
use super::harness::{
    Ask, Effect, Form, Input, Kind, Outcome, Reader, Reply, Request, RequestId, Step, Stop, Window,
    replay, run,
};
use super::step::StepCore;
use super::thread_form::ThreadCore;

const APP: u32 = 1;
/// The focused window, in top-level window 1.
const EDIT: Window = Window { top: 1, id: 11 };
/// Another window in the same top-level window's tree.
const DIALOG: Window = Window { top: 1, id: 12 };
/// A window of another top-level window.
const ELSEWHERE: Window = Window { top: 2, id: 21 };
const DOWN: u32 = 40;
const DOWN_AGAIN: u32 = 41;
const DOWN_THIRD: u32 = 42;

fn focus(token: u32) -> Input {
    Input::Focus {
        app: APP,
        window: EDIT,
        token,
    }
}

fn caret_key(key: u32) -> Input {
    Input::CaretKey {
        key,
        app: APP,
        window: EDIT,
    }
}

fn element() -> Outcome {
    Outcome::Result(Reply::Element {
        name: "Save".to_owned(),
        route: 7,
    })
}

fn completed() -> Outcome {
    Outcome::Result(Reply::Completed {
        role: "button".to_owned(),
    })
}

fn caret() -> Outcome {
    Outcome::Result(Reply::Caret)
}

fn watched(line: &str) -> Outcome {
    Outcome::Result(Reply::Watched {
        line: line.to_owned(),
    })
}

fn ask(id: u64, reader: Reader, request: Request) -> Effect {
    Effect::Ask(Ask {
        id: RequestId(id),
        reader,
        window: EDIT,
        request,
    })
}

fn ask_element(id: u64, token: u32) -> Effect {
    ask(id, Reader::Listener, Request::FocusElement { token })
}

fn ask_completion(id: u64) -> Effect {
    ask(
        id,
        Reader::Outpost(APP),
        Request::CompleteFocus { route: 7 },
    )
}

fn ask_caret(id: u64) -> Effect {
    ask(id, Reader::Outpost(APP), Request::Caret)
}

fn ask_watch(id: u64) -> Effect {
    ask(id, Reader::Outpost(APP), Request::Watch { within_ms: 100 })
}

fn speak(text: &str) -> Effect {
    Effect::Speak(text.to_owned())
}

fn cancel(id: u64, reason: Stop) -> Effect {
    Effect::Cancel {
        id: RequestId(id),
        reason,
    }
}

/// A focus read in two chained requests, then spoken.
fn focus_spoken_script() -> Vec<Step> {
    vec![
        Step::In(1_000, focus(5)),
        Step::Reply(2_000, Kind::FocusElement, element()),
        Step::Reply(3_000, Kind::CompleteFocus, completed()),
    ]
}

fn focus_is_read_in_two_requests_and_spoken<F: Form>() {
    let mut core = F::new();
    let run = run(&mut core, &focus_spoken_script());
    assert_eq!(
        run.effects,
        [ask_element(1, 5), ask_completion(2), speak("Save button")]
    );
}

/// A caret key at its turn: its starting point, the key sent on, the
/// watch, the line.
fn caret_key_script() -> Vec<Step> {
    vec![
        Step::In(1_000, caret_key(DOWN)),
        Step::Reply(2_000, Kind::Caret, caret()),
        Step::Reply(40_000, Kind::Watch, watched("second line")),
    ]
}

fn caret_key_speaks_its_line<F: Form>() {
    let mut core = F::new();
    let run = run(&mut core, &caret_key_script());
    assert_eq!(
        run.effects,
        [
            ask_caret(1),
            Effect::SendKey(DOWN),
            ask_watch(2),
            speak("second line")
        ]
    );
}

/// A key arriving while a focus entry is asked waits behind it.
fn key_behind_focus_script() -> Vec<Step> {
    vec![
        Step::In(1_000, focus(5)),
        Step::In(1_500, caret_key(DOWN)),
        Step::Reply(2_000, Kind::FocusElement, element()),
        Step::Reply(3_000, Kind::CompleteFocus, completed()),
        Step::Reply(4_000, Kind::Caret, caret()),
        Step::Reply(40_000, Kind::Watch, watched("second line")),
    ]
}

fn key_waits_behind_an_asked_focus<F: Form>() {
    let mut core = F::new();
    let run = run(&mut core, &key_behind_focus_script());
    assert_eq!(
        run.effects,
        [
            ask_element(1, 5),
            ask_completion(2),
            speak("Save button"),
            ask_caret(3),
            Effect::SendKey(DOWN),
            ask_watch(4),
            speak("second line"),
        ]
    );
}

fn focus_is_given_up_at_its_due_time<F: Form>() {
    let mut core = F::new();
    let first = run(
        &mut core,
        &[Step::In(1_000, focus(5)), Step::In(2_000, caret_key(DOWN))],
    );
    assert_eq!(first.effects, [ask_element(1, 5)]);
    assert_eq!(core.next_due(), Some(13_001_000));
    let early = run(&mut core, &[Step::In(13_000_999, Input::Tick)]);
    assert_eq!(early.effects, []);
    let due = run(&mut core, &[Step::In(13_001_000, Input::Tick)]);
    assert_eq!(due.effects, [cancel(1, Stop::DueTime), ask_caret(2)]);
    assert_eq!(core.next_due(), Some(24_001_000));
    // The late answer to the request given up finds nothing waiting.
    let late = core.feed(&super::harness::Timed {
        at_us: 13_500_000,
        input: Input::Answer {
            id: RequestId(1),
            outcome: element(),
        },
    });
    assert_eq!(late, []);
}

fn focus_is_given_up_when_the_foreground_leaves_its_window<F: Form>() {
    let mut core = F::new();
    let run = run(
        &mut core,
        &[
            Step::In(1_000, focus(5)),
            Step::Reply(2_000, Kind::FocusElement, element()),
            Step::In(3_000, Input::Foreground { window: DIALOG }),
            Step::In(4_000, Input::Foreground { window: ELSEWHERE }),
            Step::Reply(5_000, Kind::CompleteFocus, completed()),
        ],
    );
    assert_eq!(
        run.effects,
        [
            ask_element(1, 5),
            ask_completion(2),
            cancel(2, Stop::Foreground)
        ]
    );
}

fn foreground_change_ends_waiting_entries_outside_it<F: Form>() {
    let mut core = F::new();
    let run = run(
        &mut core,
        &[
            Step::In(1_000, focus(5)),
            Step::In(1_500, focus(6)),
            Step::In(1_600, caret_key(DOWN)),
            Step::In(2_000, Input::Foreground { window: ELSEWHERE }),
        ],
    );
    assert_eq!(
        run.effects,
        [
            ask_element(1, 5),
            cancel(1, Stop::Foreground),
            Effect::SendKey(DOWN)
        ]
    );
    assert_eq!(core.next_due(), None);
}

fn listener_ending_mid_request_ends_its_entries<F: Form>() {
    let mut core = F::new();
    let run = run(
        &mut core,
        &[
            Step::In(1_000, focus(5)),
            Step::In(1_500, focus(6)),
            Step::In(1_600, caret_key(DOWN)),
            Step::In(
                2_000,
                Input::ReaderEnded {
                    reader: Reader::Listener,
                },
            ),
            Step::Reply(2_500, Kind::FocusElement, element()),
        ],
    );
    assert_eq!(run.effects, [ask_element(1, 5), ask_caret(2)]);
}

fn outpost_ending_during_a_caret_read_sends_the_key_on<F: Form>() {
    let mut core = F::new();
    let run = run(
        &mut core,
        &[
            Step::In(1_000, caret_key(DOWN)),
            Step::In(
                2_000,
                Input::ReaderEnded {
                    reader: Reader::Outpost(APP),
                },
            ),
            Step::Reply(3_000, Kind::Caret, caret()),
        ],
    );
    assert_eq!(run.effects, [ask_caret(1), Effect::SendKey(DOWN)]);
    assert_eq!(core.next_due(), None);
}

fn later_key_cancels_a_watch<F: Form>() {
    let mut core = F::new();
    let run = run(
        &mut core,
        &[
            Step::In(1_000, caret_key(DOWN)),
            Step::Reply(2_000, Kind::Caret, caret()),
            Step::In(30_000, caret_key(DOWN_AGAIN)),
            Step::Reply(31_000, Kind::Caret, caret()),
            Step::Reply(60_000, Kind::Watch, watched("third line")),
        ],
    );
    assert_eq!(
        run.effects,
        [
            ask_caret(1),
            Effect::SendKey(DOWN),
            ask_watch(2),
            cancel(2, Stop::LaterKey),
            ask_caret(3),
            Effect::SendKey(DOWN_AGAIN),
            ask_watch(4),
            speak("third line"),
        ]
    );
}

fn focus_cancels_a_watch<F: Form>() {
    let mut core = F::new();
    let run = run(
        &mut core,
        &[
            Step::In(1_000, caret_key(DOWN)),
            Step::Reply(2_000, Kind::Caret, caret()),
            Step::In(30_000, focus(5)),
        ],
    );
    assert_eq!(
        run.effects,
        [
            ask_caret(1),
            Effect::SendKey(DOWN),
            ask_watch(2),
            cancel(2, Stop::PendingFocus),
            ask_element(3, 5),
        ]
    );
}

fn later_keys_silence_the_keys_ahead<F: Form>() {
    let mut core = F::new();
    let run = run(
        &mut core,
        &[
            Step::In(1_000, caret_key(DOWN)),
            Step::In(1_100, caret_key(DOWN_AGAIN)),
            Step::In(1_200, caret_key(DOWN_THIRD)),
            Step::Reply(2_000, Kind::Caret, caret()),
            Step::Reply(3_000, Kind::Caret, caret()),
            Step::Reply(40_000, Kind::Watch, watched("fourth line")),
        ],
    );
    assert_eq!(
        run.effects,
        [
            ask_caret(1),
            Effect::SendKey(DOWN),
            Effect::SendKey(DOWN_AGAIN),
            ask_caret(2),
            Effect::SendKey(DOWN_THIRD),
            ask_watch(3),
            speak("fourth line"),
        ]
    );
}

fn not_responding_plays_the_sound_and_ends_the_entry<F: Form>() {
    let mut core = F::new();
    let run = run(
        &mut core,
        &[
            Step::In(1_000, focus(5)),
            Step::In(1_500, caret_key(DOWN)),
            Step::Reply(2_000, Kind::FocusElement, Outcome::NotResponding),
        ],
    );
    assert_eq!(
        run.effects,
        [ask_element(1, 5), Effect::NotRespondingSound, ask_caret(2)]
    );
}

fn other_answers_end_the_entry_unspoken<F: Form>() {
    let mut core = F::new();
    let run = run(
        &mut core,
        &[
            Step::In(1_000, focus(5)),
            Step::In(1_500, caret_key(DOWN)),
            Step::Reply(2_000, Kind::FocusElement, element()),
            Step::Reply(
                3_000,
                Kind::CompleteFocus,
                Outcome::Refused("no window".to_owned()),
            ),
            Step::Reply(4_000, Kind::Caret, Outcome::NotThere),
        ],
    );
    assert_eq!(
        run.effects,
        [
            ask_element(1, 5),
            ask_completion(2),
            ask_caret(3),
            Effect::SendKey(DOWN)
        ]
    );
    assert_eq!(core.next_due(), None);
}

/// The inputs of a run fed to a fresh instance give the same effects.
fn replay_from_the_start_is_exact<F: Form>() {
    let mut recorded = F::new();
    let run = run(&mut recorded, &key_behind_focus_script());
    let mut fresh = F::new();
    assert_eq!(replay(&mut fresh, &run.inputs), run.effects);
}

/// After each input of the focus-then-key script, whether the form can
/// take a checkpoint, and that a run resumed from each checkpoint ends
/// with the same effects as the run it was taken from.
fn checkpoints<F: Form>() -> Vec<bool> {
    let mut recorded = F::new();
    let run = run(&mut recorded, &key_behind_focus_script());
    let mut core = F::new();
    let mut fed = Vec::new();
    let mut taken = Vec::new();
    for (index, input) in run.inputs.iter().enumerate() {
        fed.extend(core.feed(input));
        let checkpoint = core.checkpoint();
        taken.push(checkpoint.is_some());
        if let Some(mut resumed) = checkpoint {
            let rest = &run.inputs[index + 1..];
            assert_eq!(replay(&mut resumed, rest), run.effects[fed.len()..]);
        }
    }
    taken
}

fn a_checkpoint_mid_entry_resumes_exactly() {
    assert_eq!(checkpoints::<StepCore>(), [true; 6]);
}

fn no_checkpoint_mid_entry<F: Form>() {
    // Only after the last input does the queue have no front entry.
    assert_eq!(
        checkpoints::<F>(),
        [false, false, false, false, false, true]
    );
}

/// The dump while a caret key waits for its watch.
fn dump_watching<F: Form>() -> String {
    let mut core = F::new();
    let _ = run(
        &mut core,
        &[
            Step::In(1_000, caret_key(DOWN)),
            Step::Reply(2_000, Kind::Caret, caret()),
        ],
    );
    core.dump()
}

/// The dump while a focus waits for its second request.
fn dump_completing<F: Form>() -> String {
    let mut core = F::new();
    let _ = run(
        &mut core,
        &[
            Step::In(1_000, focus(5)),
            Step::In(1_500, caret_key(DOWN)),
            Step::Reply(2_000, Kind::FocusElement, element()),
        ],
    );
    core.dump()
}

macro_rules! each_form {
    ($($scenario:ident),* $(,)?) => {
        mod step_form {
            $(#[test]
            fn $scenario() {
                super::$scenario::<super::StepCore>();
            })*
        }
        mod async_form {
            $(#[test]
            fn $scenario() {
                super::$scenario::<super::AsyncCore>();
            })*
        }
        mod thread_form {
            $(#[test]
            fn $scenario() {
                super::$scenario::<super::ThreadCore>();
            })*
        }
    };
}

each_form!(
    focus_is_read_in_two_requests_and_spoken,
    caret_key_speaks_its_line,
    key_waits_behind_an_asked_focus,
    focus_is_given_up_at_its_due_time,
    focus_is_given_up_when_the_foreground_leaves_its_window,
    foreground_change_ends_waiting_entries_outside_it,
    listener_ending_mid_request_ends_its_entries,
    outpost_ending_during_a_caret_read_sends_the_key_on,
    later_key_cancels_a_watch,
    focus_cancels_a_watch,
    later_keys_silence_the_keys_ahead,
    not_responding_plays_the_sound_and_ends_the_entry,
    other_answers_end_the_entry_unspoken,
    replay_from_the_start_is_exact,
);

#[test]
fn step_form_checkpoint_mid_entry_resumes_exactly() {
    a_checkpoint_mid_entry_resumes_exactly();
}

#[test]
fn async_form_takes_no_checkpoint_mid_entry() {
    no_checkpoint_mid_entry::<AsyncCore>();
}

#[test]
fn thread_form_takes_no_checkpoint_mid_entry() {
    no_checkpoint_mid_entry::<ThreadCore>();
}

#[test]
fn step_form_dumps() {
    assert_eq!(
        dump_watching::<StepCore>(),
        "StepCore { now_us: 2000, next_request: 2, asked: Some(Asked { id: RequestId(2), \
         reader: Outpost(1), window: Window { top: 1, id: 11 }, due_us: 11001000, stage: \
         CaretWatch }), waiting: [] }"
    );
    assert_eq!(
        dump_completing::<StepCore>(),
        "StepCore { now_us: 2000, next_request: 2, asked: Some(Asked { id: RequestId(2), \
         reader: Outpost(1), window: Window { top: 1, id: 11 }, due_us: 13001000, stage: \
         FocusCompletion { name: \"Save\" } }), waiting: [CaretKey { key: 40, app: 1, window: \
         Window { top: 1, id: 11 }, silent: false }] }"
    );
}

#[test]
fn async_form_dumps() {
    assert_eq!(
        dump_watching::<AsyncCore>(),
        "AsyncCore { queue: Queue { now_us: 2000, next_request: 2, front: Some(Front { entry: \
         CaretKey { key: 40, app: 1, window: Window { top: 1, id: 11 }, silent: false }, out: \
         Some(Out { id: RequestId(2), reader: Outpost(1), window: Window { top: 1, id: 11 }, \
         request: Watch { within_ms: 100 }, due_us: 11001000 }) }), waiting: [], emitted: [], \
         ending: None }, task: Some(\"paused\") }"
    );
    assert_eq!(
        dump_completing::<AsyncCore>(),
        "AsyncCore { queue: Queue { now_us: 2000, next_request: 2, front: Some(Front { entry: \
         Focus { app: 1, window: Window { top: 1, id: 11 }, token: 5 }, out: Some(Out { id: \
         RequestId(2), reader: Outpost(1), window: Window { top: 1, id: 11 }, request: \
         CompleteFocus { route: 7 }, due_us: 13001000 }) }), waiting: [CaretKey { key: 40, app: \
         1, window: Window { top: 1, id: 11 }, silent: false }], emitted: [], ending: None }, \
         task: Some(\"paused\") }"
    );
}

#[test]
fn thread_form_dumps() {
    assert_eq!(
        dump_watching::<ThreadCore>(),
        "Queue { now_us: 2000, next_request: 2, front: Some(Front { entry: CaretKey { key: 40, \
         app: 1, window: Window { top: 1, id: 11 }, silent: false }, out: Some(Out { id: \
         RequestId(2), reader: Outpost(1), window: Window { top: 1, id: 11 }, request: Watch { \
         within_ms: 100 }, due_us: 11001000 }) }), waiting: [] }"
    );
    assert_eq!(
        dump_completing::<ThreadCore>(),
        "Queue { now_us: 2000, next_request: 2, front: Some(Front { entry: Focus { app: 1, \
         window: Window { top: 1, id: 11 }, token: 5 }, out: Some(Out { id: RequestId(2), \
         reader: Outpost(1), window: Window { top: 1, id: 11 }, request: CompleteFocus { route: \
         7 }, due_us: 13001000 }) }), waiting: [CaretKey { key: 40, app: 1, window: Window { \
         top: 1, id: 11 }, silent: false }] }"
    );
}
