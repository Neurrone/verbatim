//! A caret wait's check (`docs/design/readers-tell-core-asks-build1.md`,
//! "The watch"; the request list's design, section 5, `WaitForCaret`)
//! against mockapp's UIA text provider, each of the three ways the
//! outpost makes it, which must answer alike: a poll's program, the
//! program's steps by calls (`run_calls`, the fallback and the setting
//! with remote operations off), and the check by parts an edit control's
//! text takes. A check showing no effect, one showing the caret moved, and
//! one showing a deletion's change of the word at the caret, each from the
//! starting point a `Caret` held; and a poll that ends the wait with the
//! batch's later reads fused after the check. Each test drives
//! `verbatim-outpost`'s text module on this thread as the outpost's worker
//! does once it has the node's text in hand, as `text.rs` does.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`).

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;

use verbatim_model::batch::{
    CaretWait, Condition, Done, EffectTest, Op, Place, Point, PointOf, Step, StepOutcome, Subject,
    Waited,
};
use verbatim_model::{CaretMotion, NodeId, TextDetails, TextUnit};
use verbatim_outpost::text::check::{CheckAsk, check};
use verbatim_outpost::text::fused::poll;
use verbatim_outpost::text::ops::caret;
use verbatim_outpost::text::uia::{UiaPos, UiaText};
use verbatim_outpost::text::{Anchors, NodeText, TextResult};
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;

/// When every read here begins, on the shared clock.
const READ_AT: u64 = 1;

/// The node the anchors here are held for.
const NODE: NodeId = NodeId::new(1);

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
    let read = verbatim_outpost::text::CallStep::Read {
        at: verbatim_model::TextPoint::At(verbatim_model::TextPosition::at(at)),
        movement: None,
        unit: TextUnit::Line,
        count: 1,
    };
    match verbatim_outpost::text::by_calls(source, anchors, read) {
        Ok(Ok(verbatim_model::batch::Done::ReadText(mut read))) => read.chunks.remove(0),
        other => panic!("a line, not {other:?}"),
    }
}

/// The wait of a key of `motion` from the `Caret` read held in
/// `anchors` now, its starting word `word`: its starting texts the line
/// read there.
fn wait_from(
    source: &mut UiaText,
    anchors: &mut NodeText<'_, UiaPos>,
    (motion, word): (CaretMotion, Option<&str>),
) -> CaretWait {
    let at = caret(source, anchors, READ_AT).expect("the caret");
    let line = line_at(source, anchors, at.caret);
    CaretWait {
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
    }
}

/// How the outpost makes a check.
#[derive(Clone, Copy, Debug)]
enum Way {
    /// A poll's program.
    Program,
    /// The program's steps by calls.
    Calls,
    /// The check by parts, an edit control's.
    Parts,
}

/// The check of `wait` that `way` makes, with the word when `word`, at the
/// bound when `last`: `None` with no effect before the bound, else how the
/// wait ended.
fn checked(
    way: Way,
    (source, anchors): (&mut UiaText, &mut NodeText<'_, UiaPos>),
    wait: &CaretWait,
    (word, last): (bool, bool),
) -> TextResult<Option<Waited>> {
    match way {
        Way::Program | Way::Calls => {
            let uia = Uia::new().expect("a UIA client");
            let answer = poll(
                &uia,
                (NODE, source, anchors),
                (wait, word, last),
                &[],
                (None, READ_AT),
            )?;
            assert!(!answer.read_classically, "{way:?}");
            Ok(answer.ended.map(|(waited, _)| waited))
        }
        Way::Parts => {
            let start = anchors.held_read(wait.from).expect("the read held");
            check(source, &start, CheckAsk { wait, word, last })
        }
    }
}

/// Runs `body` each way, each on anchors of its own, with the caret put
/// back at the start of the text and the text as the fixture has it
/// before each.
fn each_way(
    app: &mut common::MockApp,
    hwnd: HWND,
    mut body: impl FnMut(&mut common::MockApp, Way, &mut UiaText, &mut NodeText<'_, UiaPos>),
) {
    for way in [Way::Program, Way::Calls, Way::Parts] {
        common::apply(app, hwnd, r"set-text doc alpha beta\ngamma\n");
        common::apply(app, hwnd, "caret doc 0");
        let mut source = uia_notes(hwnd, !matches!(way, Way::Calls));
        let mut store = Anchors::new(Arc::default());
        let mut anchors = store.node(1);
        body(app, way, &mut source, &mut anchors);
    }
}

/// A check with nothing changed since the starting point shows no effect,
/// and the check at the bound ends the wait at its bound.
fn a_check_with_nothing_changed_shows_no_effect() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-none");
    each_way(&mut app, hwnd, |_, way, source, anchors| {
        let wait = wait_from(source, anchors, (CaretMotion::NextWord, None));
        assert_eq!(
            checked(way, (source, anchors), &wait, (false, false)),
            Ok(None),
            "{way:?}"
        );
        assert_eq!(
            checked(way, (source, anchors), &wait, (false, true)),
            Ok(Some(Waited::Bound)),
            "{way:?}"
        );
    });
    app.quit();
}

/// A check after the caret moved shows the effect.
fn a_check_after_the_caret_moved_shows_the_effect() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-moved");
    each_way(&mut app, hwnd, |app, way, source, anchors| {
        let wait = wait_from(source, anchors, (CaretMotion::NextWord, None));
        common::apply(app, hwnd, "caret doc 6");
        assert_eq!(
            checked(way, (source, anchors), &wait, (false, false)),
            Ok(Some(Waited::Effect)),
            "{way:?}"
        );
    });
    app.quit();
}

/// Delete with the caret where it was: the character after it went, so
/// the word at the caret, compared once 30 ms have passed, changed from
/// the starting word, and the check shows the effect.
fn a_check_after_a_deletion_shows_the_words_change() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-deleted");
    each_way(&mut app, hwnd, |app, way, source, anchors| {
        common::apply(app, hwnd, "caret doc 6");
        let wait = wait_from(source, anchors, (CaretMotion::Delete, Some("beta")));
        assert_eq!(
            checked(way, (source, anchors), &wait, (true, false)),
            Ok(None),
            "{way:?}: nothing deleted yet"
        );
        common::apply(app, hwnd, r"set-text doc alpha eta\ngamma\n");
        common::apply(app, hwnd, "caret doc 6");
        assert_eq!(
            checked(way, (source, anchors), &wait, (true, false)),
            Ok(Some(Waited::Effect)),
            "{way:?}"
        );
    });
    app.quit();
}

/// A word key's wait batch's later steps, the caret and the word at it
/// read after the check: a poll that finds nothing reads none of them; at
/// the bound, a sentence's, under the wait's effect, are skipped; and the
/// poll that finds the caret moved reads them in its own program, by the
/// program or by calls alike.
fn a_poll_that_ends_the_wait_reads_the_later_steps() {
    common::init_com();
    let (mut app, hwnd) = start("mockapp-watch-later");
    let node = Subject::Node(NODE);
    let later = |when| {
        vec![
            Step {
                op: Op::Caret {
                    subject: node.clone(),
                    adopt: None,
                },
                when,
            },
            Step {
                op: Op::ReadText {
                    trim: false,
                    whole: false,
                    subject: node.clone(),
                    at: Point::Of(Place(1), PointOf::Caret),
                    movement: None,
                    unit: TextUnit::Word,
                    count: 1,
                    details: TextDetails::NONE,
                },
                when: None,
            },
        ]
    };
    let on_effect = Some(Condition::step(
        Place(0),
        verbatim_model::batch::Check::Found,
        false,
    ));
    for remote in [true, false] {
        common::apply(&mut app, hwnd, "caret doc 0");
        let uia = Uia::new().expect("a UIA client");
        let mut source = uia_notes(hwnd, remote);
        let mut store = Anchors::new(Arc::default());
        let mut anchors = store.node(1);
        let wait = wait_from(&mut source, &mut anchors, (CaretMotion::NextWord, None));
        let mut run = |later: &[Step], last| {
            poll(
                &uia,
                (NODE, &mut source, &mut anchors),
                (&wait, false, last),
                later,
                (None, READ_AT),
            )
            .expect("the poll")
            .ended
        };
        assert_eq!(run(&later(None), false), None, "nothing moved yet");
        assert_eq!(
            run(&later(on_effect), true),
            Some((
                Waited::Bound,
                Some(vec![StepOutcome::Skipped, StepOutcome::Skipped])
            ))
        );
        common::apply(&mut app, hwnd, "caret doc 6");
        let Some((Waited::Effect, Some(read))) = run(&later(None), false) else {
            panic!("the effect, with the later reads");
        };
        let word = match &read[..] {
            [
                StepOutcome::Done(Done::Caret(_)),
                StepOutcome::Done(Done::ReadText(word)),
            ] => word.chunks[0].text.clone(),
            other => panic!("the caret and the word: {other:?}"),
        };
        assert_eq!(word, "beta", "remote {remote}");
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
            "a_poll_that_ends_the_wait_reads_the_later_steps",
            a_poll_that_ends_the_wait_reads_the_later_steps,
        ),
    ]);
}
