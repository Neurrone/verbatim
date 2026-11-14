//! The async form's own rules: every request has one outcome however its
//! entry ends, a result that does not decode never reaches the entry, and
//! only `ask` is awaited.
//!
//! The `ending_` tests take every way an entry ends in the trial, each
//! request it sent having had exactly one outcome and its function resumed
//! by it: the driver asserts at each entry's end that no request is out and
//! none was let go, and after every input that a paused function has its
//! request out. Waiting entries that a foreground change or the listener's
//! ending drops never had a function.

use super::{
    APP, AsyncCore, DOWN, DOWN_AGAIN, DOWN_THIRD, ELSEWHERE, Form, Input, Kind, Outcome, Reader,
    Step, ask_caret, caret, caret_key, caret_key_script, completed, element, focus,
    focus_spoken_script, run,
};
use crate::trial::harness::Effect;

/// How a focus entry for `token` ended, with its answers as the dump
/// writes them.
fn focus_ended(token: u32, answers: &str) -> String {
    format!(
        "Ended {{ entry: Focus {{ app: 1, window: Window {{ top: 1, id: 11 }}, token: {token} }}, \
         answers: {answers} }}"
    )
}

/// How a caret key entry ended, with its answers as the dump writes them.
fn key_ended(key: u32, silent: bool, answers: &str) -> String {
    format!(
        "Ended {{ entry: CaretKey {{ key: {key}, app: 1, window: Window {{ top: 1, id: 11 }}, \
         silent: {silent} }}, answers: {answers} }}"
    )
}

/// The entries `core` has ended, in order.
fn endings(core: &AsyncCore) -> Vec<String> {
    core.ended()
        .iter()
        .map(|ended| format!("{ended:?}"))
        .collect()
}

/// The entries ended by `script`, in order.
fn endings_of(script: &[Step]) -> Vec<String> {
    let mut core = AsyncCore::new();
    let _ = run(&mut core, script);
    endings(&core)
}

/// The element read's result, as the dump writes it.
const ELEMENT_READ: &str = r#"Result(Element { name: "Save", route: 7 })"#;

/// Spoken, after its results.
#[test]
fn ending_spoken_after_its_results() {
    assert_eq!(
        endings_of(&focus_spoken_script()),
        [focus_ended(
            5,
            &format!(r#"[{ELEMENT_READ}, Result(Completion {{ role: "button" }})]"#)
        )]
    );
    assert_eq!(
        endings_of(&caret_key_script()),
        [key_ended(
            DOWN,
            false,
            r#"[Result(CaretPosition), Result(Line { text: "second line" })]"#
        )]
    );
}

/// Refused, and not there.
#[test]
fn ending_refused_and_not_there() {
    assert_eq!(
        endings_of(&[
            Step::In(1_000, focus(5)),
            Step::In(1_500, caret_key(DOWN)),
            Step::Reply(2_000, Kind::FocusElement, element()),
            Step::Reply(
                3_000,
                Kind::CompleteFocus,
                Outcome::Refused("no window".to_owned())
            ),
            Step::Reply(4_000, Kind::Caret, Outcome::NotThere),
        ]),
        [
            focus_ended(5, &format!(r#"[{ELEMENT_READ}, Refused("no window")]"#)),
            key_ended(DOWN, false, "[NotThere]"),
        ]
    );
}

/// Not responding.
#[test]
fn ending_not_responding() {
    assert_eq!(
        endings_of(&[
            Step::In(1_000, focus(5)),
            Step::Reply(2_000, Kind::FocusElement, Outcome::NotResponding),
        ]),
        [focus_ended(5, "[NotResponding]")]
    );
}

/// The reader ended: the listener during the element read, then the
/// outpost during the caret read.
#[test]
fn ending_the_reader_ended() {
    assert_eq!(
        endings_of(&[
            Step::In(1_000, focus(5)),
            Step::In(
                2_000,
                Input::ReaderEnded {
                    reader: Reader::Listener,
                },
            ),
            Step::In(2_500, caret_key(DOWN)),
            Step::In(
                3_000,
                Input::ReaderEnded {
                    reader: Reader::Outpost(APP),
                },
            ),
        ]),
        [
            focus_ended(5, "[ReaderEnded]"),
            key_ended(DOWN, false, "[ReaderEnded]"),
        ]
    );
}

/// Given up at the due time.
#[test]
fn ending_given_up_at_the_due_time() {
    assert_eq!(
        endings_of(&[Step::In(1_000, focus(5)), Step::In(13_001_000, Input::Tick),]),
        [focus_ended(5, "[GivenUp(DueTime)]")]
    );
}

/// Given up at a foreground change.
#[test]
fn ending_given_up_at_a_foreground_change() {
    assert_eq!(
        endings_of(&[
            Step::In(1_000, focus(5)),
            Step::Reply(2_000, Kind::FocusElement, element()),
            Step::In(3_000, Input::Foreground { window: ELSEWHERE }),
        ]),
        [focus_ended(
            5,
            &format!("[{ELEMENT_READ}, GivenUp(Foreground)]")
        )]
    );
}

/// A watch cancelled by a later key, and by a focus.
#[test]
fn ending_a_watch_cancelled_by_a_later_key_or_a_focus() {
    assert_eq!(
        endings_of(&[
            Step::In(1_000, caret_key(DOWN)),
            Step::Reply(2_000, Kind::Caret, caret()),
            Step::In(30_000, caret_key(DOWN_AGAIN)),
        ]),
        [key_ended(
            DOWN,
            true,
            "[Result(CaretPosition), GivenUp(LaterKey)]"
        )]
    );
    assert_eq!(
        endings_of(&[
            Step::In(1_000, caret_key(DOWN)),
            Step::Reply(2_000, Kind::Caret, caret()),
            Step::In(30_000, focus(5)),
        ]),
        [key_ended(
            DOWN,
            false,
            "[Result(CaretPosition), GivenUp(PendingFocus)]"
        )]
    );
}

/// Ended by the function between requests: silenced during its caret
/// read, silenced before its turn, and a focus waiting when its caret
/// read answers.
#[test]
fn ending_ended_by_its_function_between_requests() {
    assert_eq!(
        endings_of(&[
            Step::In(1_000, caret_key(DOWN)),
            Step::In(1_100, caret_key(DOWN_AGAIN)),
            Step::In(1_200, caret_key(DOWN_THIRD)),
            Step::Reply(2_000, Kind::Caret, caret()),
        ]),
        [
            key_ended(DOWN, true, "[Result(CaretPosition)]"),
            key_ended(DOWN_AGAIN, true, "[]"),
        ]
    );
    assert_eq!(
        endings_of(&[
            Step::In(1_000, caret_key(DOWN)),
            Step::In(1_500, focus(5)),
            Step::Reply(2_000, Kind::Caret, caret()),
        ]),
        [key_ended(DOWN, false, "[Result(CaretPosition)]")]
    );
}

/// A result that does not decode as its request's reply is a protocol
/// error the queue handles: the request ends as "the reader ended", and
/// the function never sees the reply.
#[test]
fn an_undecodable_result_ends_the_request_as_the_reader_ended() {
    let mut core = AsyncCore::new();
    let run = run(
        &mut core,
        &[
            Step::In(1_000, caret_key(DOWN)),
            Step::Reply(2_000, Kind::Caret, completed()),
        ],
    );
    assert_eq!(run.effects, [ask_caret(1), Effect::SendKey(DOWN)]);
    assert_eq!(endings(&core), [key_ended(DOWN, false, "[ReaderEnded]")]);
}

/// The async form's sources, as the rule that only `ask` is awaited reads
/// them.
const SOURCES: [(&str, &str); 4] = [
    ("async_form/mod.rs", include_str!("../async_form/mod.rs")),
    (
        "async_form/driver.rs",
        include_str!("../async_form/driver.rs"),
    ),
    (
        "async_form/logic.rs",
        include_str!("../async_form/logic.rs"),
    ),
    (
        "async_form/request.rs",
        include_str!("../async_form/request.rs"),
    ),
];

/// Every line of the async form's sources that contains `pattern`, with
/// its file, trimmed.
fn lines_with(pattern: &str) -> Vec<(&'static str, &'static str)> {
    SOURCES
        .iter()
        .flat_map(|(file, source)| {
            source
                .lines()
                .filter(move |line| line.contains(pattern))
                .map(move |line| (*file, line.trim()))
        })
        .collect()
}

/// Only `ask` is awaited: the one future the async form implements is
/// `Asking`, every `.await` is on `cx.ask`, the only async functions are
/// the entries', and the only other future named is the one the driver's
/// test uses to show that a function waiting on anything else is caught.
/// A later author adding another awaitable changes these lists.
#[test]
fn only_ask_is_awaited() {
    assert_eq!(
        lines_with("Future for"),
        [(
            "async_form/driver.rs",
            "impl<R: Request> Future for Asking<'_, R> {"
        )]
    );
    assert_eq!(
        lines_with(".await"),
        [
            (
                "async_form/logic.rs",
                "let element = cx.ask(ElementRead { token }, window, due_us).await;"
            ),
            (
                "async_form/logic.rs",
                "let completion = cx.ask(CompleteFocus { app, route }, window, due_us).await;"
            ),
            (
                "async_form/logic.rs",
                "let start = cx.ask(Caret { app }, window, due_us).await;"
            ),
            (
                "async_form/logic.rs",
                "if let Outcome::Result(Line { text }) = cx.ask(watch, window, due_us).await {"
            ),
        ]
    );
    assert_eq!(
        lines_with("std::future"),
        [
            ("async_form/driver.rs", "use std::future::Future;"),
            (
                "async_form/driver.rs",
                "core.task = Some(Box::pin(std::future::pending::<()>()));"
            ),
            ("async_form/logic.rs", "use std::future::Future;"),
        ]
    );
    assert_eq!(lines_with("async {"), []);
    assert_eq!(lines_with("async move"), []);
    assert_eq!(
        lines_with("async fn"),
        [
            (
                "async_form/logic.rs",
                "async fn focus(mut cx: Cx, app: u32, window: Window, token: u32) {"
            ),
            (
                "async_form/logic.rs",
                "async fn caret_key(mut cx: Cx, key: u32, app: u32, window: Window) {"
            ),
        ]
    );
}
