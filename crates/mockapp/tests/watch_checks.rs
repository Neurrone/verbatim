//! A caret key's watch check and final read
//! (`docs/design/readers-tell-core-asks-build1.md`, "The watch") against
//! mockapp's UIA text provider, through the remote program and through its
//! classic reads, which must answer alike: a check showing no effect, one
//! showing the caret moved, one showing a deletion's change of the word at
//! the caret, and the final read's unit.
//! Each test drives `verbatim-outpost`'s text module on this thread as the
//! outpost's worker does once it has the node's text in hand, as
//! `text.rs` does.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`).

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;

use verbatim_model::{CaretMotion, TextDetails, TextUnit, WatchFinalRead, Watched, WatchedRead};
use verbatim_outpost::text::check::{CheckAsk, check, final_read};
use verbatim_outpost::text::held::starting_read;
use verbatim_outpost::text::uia::UiaText;
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

/// The check asked of a key of `motion` from `start`, its final read the
/// key's unit.
fn ask(
    point: verbatim_model::TextAnchor,
    motion: CaretMotion,
    (word, last): (bool, bool),
) -> CheckAsk {
    CheckAsk {
        point,
        motion,
        word,
        last,
        final_read: WatchFinalRead::Unit {
            unit: motion.unit(),
            details: TextDetails::ALL,
        },
    }
}

/// A final read's line, the caret's offset in it, and its unit's text.
fn summary(read: &WatchedRead) -> (String, u32, String) {
    (
        read.caret.line.text.clone(),
        read.caret.line.offset,
        read.unit.text.clone(),
    )
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

/// A check with nothing changed since the starting read shows no effect,
/// and the check at the bound reads the key's unit all the same.
fn a_check_with_nothing_changed_shows_no_effect() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-none");
    both_ways(&mut app, hwnd, |_, source, anchors| {
        let started = starting_read(
            source,
            anchors,
            READ_AT,
            verbatim_model::StartingUnit::At(TextUnit::Word),
        )
        .expect("the starting read");
        let motion = CaretMotion::NextWord;
        assert_eq!(
            check(
                source,
                anchors,
                ask(started.point, motion, (false, false)),
                READ_AT
            ),
            Ok(None)
        );
        match check(
            source,
            anchors,
            ask(started.point, motion, (false, true)),
            READ_AT,
        ) {
            Ok(Some(Watched::Bound(read))) => assert_eq!(
                summary(&read),
                ("alpha beta\n".to_owned(), 0, "alpha ".to_owned())
            ),
            other => panic!("the bound's read, not {other:?}"),
        }
    });
    app.quit();
}

/// A check after the caret moved shows the effect, with the final read
/// made from the caret it read: the line and the word at the caret.
fn a_check_after_the_caret_moved_shows_the_effect() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-moved");
    both_ways(&mut app, hwnd, |app, source, anchors| {
        let started = starting_read(
            source,
            anchors,
            READ_AT,
            verbatim_model::StartingUnit::At(TextUnit::Word),
        )
        .expect("the starting read");
        common::apply(app, hwnd, "caret doc 6");
        let checked = check(
            source,
            anchors,
            ask(started.point, CaretMotion::NextWord, (false, false)),
            READ_AT,
        );
        match checked {
            Ok(Some(Watched::Effect(read))) => assert_eq!(
                summary(&read),
                ("alpha beta\n".to_owned(), 6, "beta".to_owned())
            ),
            other => panic!("the effect, not {other:?}"),
        }
    });
    app.quit();
}

/// Delete with the caret where it was: the character after it went, so
/// the word at the caret, compared once 30 ms have passed, changed, and
/// the check shows the effect with the character now at the caret.
fn a_check_after_a_deletion_shows_the_words_change() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-deleted");
    both_ways(&mut app, hwnd, |app, source, anchors| {
        common::apply(app, hwnd, "caret doc 6");
        let started = starting_read(
            source,
            anchors,
            READ_AT,
            verbatim_model::StartingUnit::AtWithWord(TextUnit::Character),
        )
        .expect("the starting read");
        assert_eq!(
            started.word.as_ref().map(|word| word.text.as_str()),
            Some("beta")
        );
        common::apply(app, hwnd, r"set-text doc alpha eta\ngamma\n");
        common::apply(app, hwnd, "caret doc 6");
        let checked = check(
            source,
            anchors,
            ask(started.point, CaretMotion::Delete, (true, false)),
            READ_AT,
        );
        match checked {
            Ok(Some(Watched::Effect(read))) => assert_eq!(
                summary(&read),
                ("alpha eta\n".to_owned(), 6, "e".to_owned())
            ),
            other => panic!("the effect, not {other:?}"),
        }
    });
    app.quit();
}

/// The final read: the caret's line and the key's unit at the caret.
fn the_final_read_reads_the_keys_unit_at_the_caret() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-final");
    both_ways(&mut app, hwnd, |app, source, anchors| {
        common::apply(app, hwnd, "caret doc 11");
        let read =
            final_read(source, anchors, (TextUnit::Word, None), READ_AT).expect("the final read");
        assert_eq!(
            summary(&read),
            ("gamma\n".to_owned(), 0, "gamma".to_owned())
        );
        let read = final_read(source, anchors, (TextUnit::Character, None), READ_AT)
            .expect("the final read");
        assert_eq!(summary(&read), ("gamma\n".to_owned(), 0, "g".to_owned()));
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
            "the_final_read_reads_the_keys_unit_at_the_caret",
            the_final_read_reads_the_keys_unit_at_the_caret,
        ),
        (
            "the_program_and_its_classic_reads_agree",
            the_program_and_its_classic_reads_agree,
        ),
    ]);
}
