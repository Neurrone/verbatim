//! The request list's condition that compares two text positions
//! (`verbatim_model::batch::Test::NotAfter`), against mockapp's UIA text
//! provider, through a real outpost reading by remote operations and one
//! reading by calls, which must answer alike: each row of the condition's
//! outcome table that a provider can be made to give, an anchor no longer
//! held included; the probe that tells the text's start, a move of one
//! line back that goes nowhere there; and the fixed end, a move forward
//! from the text's end that goes nowhere, which later output stays past.
//! With them, the two generic facts the terminal's reader takes: a caret
//! read says whether the provider reported a selection at all, and a
//! trimmed read of a range is read whole, past an untrimmed read's limit.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_core::batch::{Builder, Cond, Sent};
use verbatim_model::batch::{
    Batch, Done, Missing, Op, Placement, Step, StepOutcome, StepRefusal, Subject,
};
use verbatim_model::request::{Outcome, Reply, WindowVerdict};
use verbatim_model::{
    NodeId, TextAnchor, TextDetails, TextMovement, TextPoint, TextPosition, TextUnit,
};
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;

use common::outpost::OutpostUnderTest;

/// The terminal's width in cells.
const WIDTH: usize = 8;

/// More rows than the text holds.
const FAR: i32 = 1_000_000;

/// Brands this suite's batches.
enum Comparing {}

/// One step's outcome, without the anchors an outpost mints.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ran {
    /// A move, by the lines it went.
    Moved(i32),
    /// A read of a range that was not cut, by its text.
    Text(String),
    /// A read of a range that was cut, by its text's length in bytes.
    Cut(usize),
    /// A caret read, by whether the provider reported a selection.
    Caret(bool),
    Skipped,
    Refused(StepRefusal),
    Missing(TextAnchor),
    Other(String),
}

fn lines(count: i32) -> TextMovement {
    TextMovement {
        unit: TextUnit::Line,
        count,
    }
}

/// The point Core holds under `anchor`.
fn held(anchor: TextAnchor) -> TextPoint {
    TextPoint::At(TextPosition::at(anchor))
}

/// Shows `rows` in mockapp's terminal, each padded to [`WIDTH`] and ended
/// by a line break, the caret at the text's start, raising no event.
fn show(app: &mut common::MockApp, hwnd: HWND, rows: &[&str]) {
    let padded: Vec<String> = rows.iter().map(|row| format!("{row:WIDTH$}\r\n")).collect();
    let text = padded.concat().replace('\r', "\\r").replace('\n', "\\n");
    common::apply(app, hwnd, &format!("set-text term {text}"));
    common::apply(app, hwnd, "caret term 0 0");
}

/// mockapp's terminal, with an outpost reading it by remote operations
/// and one reading it by calls, each with the terminal's node.
struct Terminal {
    app: common::MockApp,
    hwnd: HWND,
    outposts: Vec<(OutpostUnderTest, NodeId)>,
}

impl Terminal {
    fn start(name: &str, rows: &[&str]) -> Self {
        Self::of("terminal.json", name, rows)
    }

    /// mockapp's terminal of `fixture`, showing `rows`.
    fn of(fixture: &str, name: &str, rows: &[&str]) -> Self {
        common::init_com();
        let title = common::unique_title(name);
        let mut app = common::spawn(fixture, "uia", &title);
        let hwnd = common::find_window(&title);
        let uia = Uia::new().expect("a UIA client");
        let element = common::named_element(&uia, hwnd, "Terminal");
        common::apply(&mut app, hwnd, "screen term 3");
        show(&mut app, hwnd, rows);
        app.send("set-focus term");
        let outposts = [true, false]
            .into_iter()
            .map(|remote| {
                let outpost = OutpostUnderTest::with_options(
                    &app,
                    OutpostOptions {
                        remote_operations: remote,
                    },
                );
                outpost.read_focus_as(&element);
                let focus = Batch {
                    steps: vec![Step {
                        op: Op::FocusRead {
                            subject: Subject::Focus,
                            known: Vec::new(),
                            set: common::outpost::focus_set(),
                            details: TextDetails::NONE,
                        },
                        when: None,
                    }],
                    placement: Placement::Queue,
                    spare: None,
                };
                let steps = outcomes(&outpost, hwnd, focus);
                let Some(StepOutcome::Done(Done::FocusRead(read))) = steps.first() else {
                    panic!("the focus read, not {steps:?}")
                };
                let node = read.focus.read.node.expect("the terminal's node");
                (outpost, node)
            })
            .collect();
        Self {
            app,
            hwnd,
            outposts,
        }
    }

    /// The batch `build` writes for each outpost's node, run through that
    /// outpost: each one's outcomes, the remote one's first.
    fn run(
        &self,
        build: impl Fn(&mut Builder<Comparing>, NodeId, &[TextAnchor]),
        held: &[Vec<TextAnchor>],
    ) -> [Vec<StepOutcome>; 2] {
        let ran: Vec<Vec<StepOutcome>> = self
            .outposts
            .iter()
            .enumerate()
            .map(|(which, (outpost, node))| {
                let mut batch = Builder::<Comparing>::named();
                build(
                    &mut batch,
                    *node,
                    held.get(which).map_or(&[], Vec::as_slice),
                );
                let sent: Sent<Comparing> = batch.finish();
                outcomes(outpost, self.hwnd, sent.batch().clone())
            })
            .collect();
        ran.try_into().expect("two outposts")
    }

    /// As [`Terminal::run`], the two outposts' outcomes summarized, which
    /// must be alike.
    fn both(
        &self,
        build: impl Fn(&mut Builder<Comparing>, NodeId, &[TextAnchor]),
        held: &[Vec<TextAnchor>],
    ) -> Vec<Ran> {
        let [remote, calls] = self.run(build, held);
        let remote: Vec<Ran> = remote.iter().map(summary).collect();
        let calls: Vec<Ran> = calls.iter().map(summary).collect();
        assert_eq!(
            remote, calls,
            "the outcomes by remote operations and by calls differ"
        );
        remote
    }

    /// The anchors each outpost holds where moves of `rows` lines from the
    /// text's start land, in order.
    fn hold(&self, rows: &[i32]) -> Vec<Vec<TextAnchor>> {
        let ran = self.run(
            |batch, node, _| {
                for &row in rows {
                    batch.move_point(node, TextPoint::Start.into(), lines(row));
                }
            },
            &[],
        );
        ran.iter()
            .map(|outcomes| {
                outcomes
                    .iter()
                    .map(|outcome| match outcome {
                        StepOutcome::Done(Done::Move(moved)) => moved.to,
                        other => panic!("a move from the text's start, not {other:?}"),
                    })
                    .collect()
            })
            .collect()
    }
}

/// The step outcomes of `batch`, run through `outpost`.
fn outcomes(outpost: &OutpostUnderTest, hwnd: HWND, batch: Batch) -> Vec<StepOutcome> {
    let (answer, _) = outpost.batch(hwnd, WindowVerdict::Uia, batch);
    let Outcome::Result(Reply::Steps(steps)) = answer.outcome else {
        panic!("steps, not {:?}", answer.outcome)
    };
    steps
}

fn summary(outcome: &StepOutcome) -> Ran {
    match outcome {
        StepOutcome::Done(Done::Move(moved)) => Ran::Moved(moved.moved),
        StepOutcome::Done(Done::ReadRange(range)) if range.truncated => Ran::Cut(range.text.len()),
        StepOutcome::Done(Done::ReadRange(range)) => Ran::Text(range.text.clone()),
        StepOutcome::Done(Done::Caret(caret)) => Ran::Caret(caret.reported),
        StepOutcome::Skipped => Ran::Skipped,
        StepOutcome::Refused(refusal) => Ran::Refused(*refusal),
        StepOutcome::NotThere(Missing::Anchor(anchor)) => Ran::Missing(*anchor),
        other => Ran::Other(format!("{other:?}")),
    }
}

/// Two held points compared each way, plain and negated; a point of an
/// earlier step done, of one skipped and of one refused; and an anchor no
/// longer held, which is its step's outcome whatever the negation.
fn a_comparison_runs_or_skips_its_step_as_the_table_says() {
    let rows = ["r 1", "r 2", "r 3", "r 4", "r 5", "r 6"];
    let mut terminal = Terminal::start("mockapp-compare-table", &rows);
    // Rows 3 and 5, and row 6, which the shorter text below leaves past
    // its end.
    let held_points = terminal.hold(&[2, 4, 5]);
    let earlier_later = terminal.both(
        |batch, node, points| {
            let (third, fifth) = (held(points[0]).into(), held(points[1]).into());
            batch
                .when(Cond::not_after(third, fifth))
                .move_point(node, third, lines(-1));
            batch
                .when(!Cond::not_after(third, fifth))
                .move_point(node, third, lines(-1));
            batch
                .when(Cond::not_after(fifth, third))
                .move_point(node, fifth, lines(-1));
            batch
                .when(!Cond::not_after(fifth, third))
                .move_point(node, fifth, lines(-1));
            // A point is not after itself.
            batch
                .when(Cond::not_after(third, third))
                .move_point(node, third, lines(1));
        },
        &held_points,
    );
    assert_eq!(
        earlier_later,
        [
            Ran::Moved(-1),
            Ran::Skipped,
            Ran::Skipped,
            Ran::Moved(-1),
            Ran::Moved(1),
        ]
    );
    let missing = TextAnchor(987_654);
    show(&mut terminal.app, terminal.hwnd, &rows[..3]);
    let named_steps = terminal.both(
        |batch, node, points| {
            let third = held(points[0]).into();
            let end = batch.move_point(node, TextPoint::End.into(), lines(FAR));
            // A step done: the third row is not after the end.
            let above =
                batch
                    .when(Cond::not_after(third, end.landed()))
                    .move_point(node, third, lines(-1));
            batch
                .when(!Cond::not_after(above.landed(), end.landed()))
                .move_point(node, third, lines(-1));
            // A step skipped skips the step comparing with it, either way.
            let skipped = batch
                .when(!Cond::not_after(third, end.landed()))
                .move_point(node, third, lines(-1));
            batch
                .when(Cond::not_after(skipped.landed(), end.landed()))
                .move_point(node, third, lines(-1));
            batch
                .when(!Cond::not_after(skipped.landed(), end.landed()))
                .move_point(node, third, lines(-1));
            // A step refused, a move from a point past the text's end,
            // makes the test false.
            let refused = batch.move_point(node, held(points[2]).into(), lines(-1));
            batch
                .when(Cond::not_after(refused.landed(), end.landed()))
                .move_point(node, third, lines(-1));
            batch
                .when(!Cond::not_after(refused.landed(), end.landed()))
                .move_point(node, third, lines(-1));
            // An anchor no longer held.
            batch
                .when(Cond::not_after(held(missing).into(), end.landed()))
                .move_point(node, third, lines(-1));
            batch
                .when(!Cond::not_after(end.landed(), held(missing).into()))
                .move_point(node, third, lines(-1));
        },
        &held_points,
    );
    assert_eq!(
        named_steps,
        [
            Ran::Moved(0),
            Ran::Moved(-1),
            Ran::Skipped,
            Ran::Skipped,
            Ran::Skipped,
            Ran::Skipped,
            Ran::Refused(StepRefusal::OutsideText),
            Ran::Skipped,
            Ran::Moved(-1),
            Ran::Missing(missing),
            Ran::Missing(missing),
        ]
    );
    terminal.app.quit();
}

/// A move of one line back goes nowhere from the text's start and one
/// line from anywhere else; a move forward from the text's end goes
/// nowhere, and a read to where it landed leaves out what is written
/// after.
fn the_probe_tells_the_texts_start_and_the_fixed_end_stays_before_later_output() {
    let rows = ["r 1", "r 2", "r 3"];
    let mut terminal = Terminal::start("mockapp-compare-probe", &rows);
    let second = terminal.hold(&[1]);
    let probes = terminal.both(
        |batch, node, points| {
            batch.move_point(node, TextPoint::Start.into(), lines(-1));
            batch.move_point(node, held(points[0]).into(), lines(-1));
            let end = batch.move_point(node, TextPoint::End.into(), lines(FAR));
            batch.read_range_trimmed(node, (end.landed(), TextPoint::End.into()));
        },
        &second,
    );
    assert_eq!(
        probes,
        [
            Ran::Moved(0),
            Ran::Moved(-1),
            Ran::Moved(0),
            Ran::Text(String::new()),
        ]
    );
    let ends: Vec<Vec<TextAnchor>> = terminal
        .run(
            |batch, node, _| {
                batch.move_point(node, TextPoint::End.into(), lines(FAR));
            },
            &[],
        )
        .iter()
        .map(|outcomes| match outcomes.first() {
            Some(StepOutcome::Done(Done::Move(moved))) => vec![moved.to],
            other => panic!("the move from the end, not {other:?}"),
        })
        .collect();
    show(
        &mut terminal.app,
        terminal.hwnd,
        &["r 1", "r 2", "r 3", "later"],
    );
    let read = terminal.both(
        |batch, node, points| {
            batch.read_range_trimmed(node, (TextPoint::Start.into(), held(points[0]).into()));
            batch.read_range_trimmed(node, (TextPoint::Start.into(), TextPoint::End.into()));
        },
        &ends,
    );
    assert_eq!(
        read,
        [
            Ran::Text("r 1\r\nr 2\r\nr 3\r\n".to_owned()),
            Ran::Text("r 1\r\nr 2\r\nr 3\r\nlater\r\n".to_owned()),
        ]
    );
    terminal.app.quit();
}

/// A provider that answers no selection at all (`terminal_no_selection.json`)
/// is told from one whose caret is at the text's start, though both
/// carets are read there: each is as many lines from the text's end.
fn a_caret_read_says_whether_the_provider_reported_a_selection() {
    let rows = ["r 1", "r 2", "r 3"];
    let read = |terminal: &Terminal| {
        terminal.both(
            |batch, node, _| {
                let caret = batch.caret(node, None);
                batch.move_point(node, caret.caret(), lines(FAR));
            },
            &[],
        )
    };
    let none = Terminal::of("terminal_no_selection.json", "mockapp-caret-none", &rows);
    assert_eq!(read(&none), [Ran::Caret(false), Ran::Moved(3)]);
    none.app.quit();
    let at_start = Terminal::start("mockapp-caret-start", &rows);
    assert_eq!(read(&at_start), [Ran::Caret(true), Ran::Moved(3)]);
    at_start.app.quit();
}

/// A text longer than an untrimmed read of a range takes: the trimmed
/// read answers every row, and the untrimmed one is cut at its limit.
fn a_trimmed_range_is_read_whole_past_the_untrimmed_limit() {
    // Each row is ten UTF-16 units with its line break.
    const ROWS: usize = 53_000;
    let rows = vec!["x"; ROWS];
    let terminal = Terminal::start("mockapp-trimmed-whole", &rows);
    let read = terminal.both(
        |batch, node, _| {
            let ends = (TextPoint::Start.into(), TextPoint::End.into());
            batch.read_range_trimmed(node, ends);
            batch.read_range(node, ends, None);
        },
        &[],
    );
    assert_eq!(
        read,
        [
            Ran::Text("x\r\n".repeat(ROWS)),
            // The limit in UTF-16 units, less the last character, which
            // the cut drops as possibly part of one.
            Ran::Cut(verbatim_model::MAX_RANGE_BYTES / 2 - 1),
        ]
    );
    terminal.app.quit();
}

fn main() {
    harness::run_isolated(&[
        (
            "a_caret_read_says_whether_the_provider_reported_a_selection",
            a_caret_read_says_whether_the_provider_reported_a_selection,
        ),
        (
            "a_trimmed_range_is_read_whole_past_the_untrimmed_limit",
            a_trimmed_range_is_read_whole_past_the_untrimmed_limit,
        ),
        (
            "a_comparison_runs_or_skips_its_step_as_the_table_says",
            a_comparison_runs_or_skips_its_step_as_the_table_says,
        ),
        (
            "the_probe_tells_the_texts_start_and_the_fixed_end_stays_before_later_output",
            the_probe_tells_the_texts_start_and_the_fixed_end_stays_before_later_output,
        ),
    ]);
}
