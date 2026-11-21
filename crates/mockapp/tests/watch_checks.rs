//! A caret wait's check (`docs/design/readers-tell-core-asks-build1.md`,
//! "The watch"; the request list's design, `WaitForCaret`) against
//! mockapp's UIA text provider, through the remote program and through its
//! classic reads, which must answer alike: a check showing no effect, one
//! showing the caret moved, and one showing a deletion's change of the
//! word at the caret, each from the starting point a `Caret` held.
//! Each test drives `verbatim-outpost`'s text module on this thread as the
//! outpost's worker does once it has the node's text in hand, as
//! `text.rs` does.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`).

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;

use verbatim_model::CaretMotion;
use verbatim_model::batch::{CaretWait, EffectTest, Waited};
use verbatim_outpost::text::check::{CheckAsk, check};
use verbatim_outpost::text::ops::caret;
use verbatim_outpost::text::uia::{UiaPos, UiaText};
use verbatim_outpost::text::{Anchors, NodeText};
use verbatim_uia::Uia;
use verbatim_uia::text::Endpoint;
use verbatim_uia_rops::{RangeEnd, WatchQuery, watch_check_classic, watch_check_remote};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::TextUnit_Word;

/// When every read here begins, on the shared clock.
const READ_AT: u64 = 1;

/// The text of mockapp's "Notes" document through UIA, read remotely or
/// classically.
fn uia_notes(hwnd: HWND, remote: bool) -> UiaText {
    let uia = Uia::new().expect("a UIA client");
    let notes = common::named_element(&uia, hwnd, "Notes");
    let pattern = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    UiaText::new(notes, pattern, false).remote(remote)
}

/// mockapp with `text.json`'s "Notes" ("alpha beta\ngamma\n"), and its
/// window.
fn start(prefix: &str) -> (common::MockApp, HWND) {
    let title = common::unique_title(prefix);
    let app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    (app, hwnd)
}

/// The line read at the caret `at` held, as a batch's `ReadText` of it
/// reads it.
fn line_at(
    source: &mut UiaText,
    anchors: &mut NodeText<'_, UiaPos>,
    at: verbatim_model::TextAnchor,
) -> verbatim_model::TextChunk {
    match verbatim_outpost::text::perform(
        source,
        anchors,
        &verbatim_model::TextOp::Read(verbatim_model::TextRead {
            at: verbatim_model::TextPoint::At(verbatim_model::TextPosition::at(at)),
            movement: None,
            unit: verbatim_model::TextUnit::Line,
        }),
    ) {
        verbatim_model::TextReply::Read { chunk, .. } => chunk,
        other => panic!("a line, not {other:?}"),
    }
}

/// The wait of a key of `motion` from the `Caret` read held in
/// `anchors` now, its starting word `word`, and that held read: the
/// starting point a check compares with.
fn wait_from(
    source: &mut UiaText,
    anchors: &mut NodeText<'_, UiaPos>,
    (motion, word): (CaretMotion, Option<&str>),
) -> (CaretWait, verbatim_outpost::text::LastRead<UiaPos>) {
    let at = caret(source, anchors, READ_AT).expect("the caret");
    let line = line_at(source, anchors, at.caret.anchor);
    let start = anchors.held_read(at.held).expect("the read held");
    let wait = CaretWait {
        from: at.held,
        line: line.text,
        offset: line.offset,
        word: word.map(str::to_owned),
        effect: EffectTest {
            unit: motion.compares_unit(),
            line: motion.deletes(),
        },
        passed_at_us: READ_AT,
        poll_us: 10_000,
        bound_us: 100_000,
        events: false,
    };
    (wait, start)
}

/// Runs `body` with the text read remotely and then classically, each on
/// anchors of its own, with the caret put back at the start of the text
/// and the text as the fixture has it before each.
fn both_ways(
    app: &mut common::MockApp,
    hwnd: HWND,
    mut body: impl FnMut(
        &mut common::MockApp,
        &mut UiaText,
        &mut NodeText<'_, verbatim_outpost::text::uia::UiaPos>,
    ),
) {
    for remote in [true, false] {
        common::apply(app, hwnd, r"set-text doc alpha beta\ngamma\n");
        common::apply(app, hwnd, "caret doc 0");
        let mut source = uia_notes(hwnd, remote);
        let mut store = Anchors::new(Arc::default());
        let mut anchors = store.node(1);
        body(app, &mut source, &mut anchors);
    }
}

/// A check with nothing changed since the starting point shows no effect,
/// and the check at the bound ends the wait at its bound.
fn a_check_with_nothing_changed_shows_no_effect() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-none");
    both_ways(&mut app, hwnd, |_, source, anchors| {
        let (wait, start) = wait_from(source, anchors, (CaretMotion::NextWord, None));
        let ask = |last| CheckAsk {
            wait: &wait,
            word: false,
            last,
        };
        assert_eq!(check(source, &start, ask(false)), Ok(None));
        assert_eq!(check(source, &start, ask(true)), Ok(Some(Waited::Bound)));
    });
    app.quit();
}

/// A check after the caret moved shows the effect.
fn a_check_after_the_caret_moved_shows_the_effect() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-moved");
    both_ways(&mut app, hwnd, |app, source, anchors| {
        let (wait, start) = wait_from(source, anchors, (CaretMotion::NextWord, None));
        common::apply(app, hwnd, "caret doc 6");
        let ask = CheckAsk {
            wait: &wait,
            word: false,
            last: false,
        };
        assert_eq!(check(source, &start, ask), Ok(Some(Waited::Effect)));
    });
    app.quit();
}

/// Delete with the caret where it was: the character after it went, so
/// the word at the caret, compared once 30 ms have passed, changed from
/// the starting word, and the check shows the effect.
fn a_check_after_a_deletion_shows_the_words_change() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-deleted");
    both_ways(&mut app, hwnd, |app, source, anchors| {
        common::apply(app, hwnd, "caret doc 6");
        let (wait, start) = wait_from(source, anchors, (CaretMotion::Delete, Some("beta")));
        let ask = CheckAsk {
            wait: &wait,
            word: true,
            last: false,
        };
        assert_eq!(check(source, &start, ask), Ok(None), "nothing deleted yet");
        common::apply(app, hwnd, r"set-text doc alpha eta\ngamma\n");
        common::apply(app, hwnd, "caret doc 6");
        assert_eq!(check(source, &start, ask), Ok(Some(Waited::Effect)));
    });
    app.quit();
}

/// The program and its classic reads find the same: the caret moved, the
/// selection's ends with it, the word at the caret, and the key's unit
/// read because the caret moved; and with the caret where it was, no unit
/// read until the check at the bound.
fn the_program_and_its_classic_reads_agree() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-agree");
    let uia = Uia::new().expect("a UIA client");
    let notes = common::named_element(&uia, hwnd, "Notes");
    let pattern = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    common::apply(&mut app, hwnd, "caret doc 0");
    let before = verbatim_uia::text::TextPatternExt::selection(&pattern)
        .expect("the selection")
        .remove(0);
    let start = RangeEnd {
        range: &before,
        endpoint: Endpoint::Start,
    };
    let query = |last: bool| WatchQuery {
        element: &notes,
        pattern: &pattern,
        start,
        start_selection: (start, start),
        word: true,
        unit: Some(TextUnit_Word),
        last,
        formats: None,
        attributes: verbatim_uia_rops::Attributes::NONE,
        max_text: 1024,
    };
    let found = |answer: &verbatim_uia_rops::WatchAnswer| {
        (
            answer.caret_moved,
            answer.selection_moved,
            String::from_utf16_lossy(&answer.line.text),
            answer.line.offset,
            answer
                .word
                .as_ref()
                .map(|word| String::from_utf16_lossy(&word.text)),
            answer
                .unit
                .as_ref()
                .map(|unit| String::from_utf16_lossy(&unit.text)),
            answer.moved_while_read,
        )
    };
    let still = (
        false,
        false,
        "alpha beta\n".to_owned(),
        0,
        Some("alpha ".to_owned()),
        None,
        false,
    );
    for run in [watch_check_remote, watch_check_classic] {
        assert_eq!(found(&run(&query(false)).expect("the check")), still);
        assert_eq!(
            found(&run(&query(true)).expect("the check")),
            (
                false,
                false,
                "alpha beta\n".to_owned(),
                0,
                Some("alpha ".to_owned()),
                Some("alpha ".to_owned()),
                false,
            )
        );
    }
    common::apply(&mut app, hwnd, "caret doc 6");
    for run in [watch_check_remote, watch_check_classic] {
        assert_eq!(
            found(&run(&query(false)).expect("the check")),
            (
                true,
                true,
                "alpha beta\n".to_owned(),
                6,
                Some("beta".to_owned()),
                Some("beta".to_owned()),
                false,
            )
        );
    }
    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_check_with_nothing_changed_shows_no_effect",
            a_check_with_nothing_changed_shows_no_effect,
        ),
        (
            "a_check_after_the_caret_moved_shows_the_effect",
            a_check_after_the_caret_moved_shows_the_effect,
        ),
        (
            "a_check_after_a_deletion_shows_the_words_change",
            a_check_after_a_deletion_shows_the_words_change,
        ),
        (
            "the_program_and_its_classic_reads_agree",
            the_program_and_its_classic_reads_agree,
        ),
    ]);
}
