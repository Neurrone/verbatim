//! Core's terminal reader (`verbatim_core::terminal::reader`) against
//! mockapp's UIA text provider, through a real outpost reading by remote
//! operations and one reading by calls: each read's batch is run by both,
//! read back by the reader, and must come out alike and as pinned. The
//! cases are those the old read program's test pins
//! (`terminal_snapshot.rs`): a first read; a read from the start the last
//! one named, with nothing new, with output written since, and after a
//! clear that leaves the start past the text's end; a first read again
//! with its history block; the whole text; the rows above a start, as many
//! as asked, exactly as many as the text holds and fewer; history reads by
//! place; the caret, a caret key's starting point and a provider that
//! reports no selection; the screen's colours as exact runs, read by the
//! colour programs and by the read's own program, at the size kept and
//! after a resize; and a start the outpost no longer holds.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::num::NonZeroU32;

use verbatim_core::batch::{Answers, Builder};
use verbatim_core::terminal::reader::{
    HistoryAsk, WindowAsk, WindowRead, WindowReply, history_batch, history_of, window_batch,
    window_of,
};
use verbatim_model::batch::{Batch, Done, Op, Placement, Step, StepOutcome, Subject};
use verbatim_model::request::{Answer, Outcome, Reply, WindowVerdict};
use verbatim_model::{
    NodeId, TerminalColourRun, TerminalSnapshot, TerminalText, TextAnchor, TextDetails,
    TextFailure, TextMovement, TextPoint, TextPosition, TextUnit,
};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::clock::now_us;
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;

use common::outpost::OutpostUnderTest;

/// The terminal's width in cells.
const WIDTH: usize = 10;

/// Brands this suite's own batches.
enum Probing {}

/// `rows` as mockapp's terminal holds them: each padded to [`WIDTH`] and
/// ended by a line break, so that the text ends with an empty line, which
/// mockapp counts as the screen's last row: a screen of 4 rows shows 3
/// rows of text, and the reads here name a height of 3.
fn padded(rows: &[&str]) -> String {
    lines(rows, |row| format!("{row:WIDTH$}\r\n"))
}

/// `rows`, each as `line` writes it, joined.
fn lines(rows: &[&str], line: impl Fn(&str) -> String) -> String {
    rows.iter().fold(String::new(), |mut text, row| {
        text.push_str(&line(row));
        text
    })
}

/// Rows `r 1` to `r <count>`, then the prompt.
fn numbered(count: u32) -> Vec<String> {
    (1..=count)
        .map(|row| format!("r {row}"))
        .chain(["ready>".to_owned()])
        .collect()
}

/// `rows` as a trimmed read gives them: each without its padding, with
/// its line break, and the lines' lengths with the empty one at the end.
fn text(rows: &[&str]) -> TerminalText {
    TerminalText {
        text: lines(rows, |row| format!("{row}\r\n")),
        lengths: vec![
            (
                u32::try_from(rows.len()).expect("a few rows"),
                u32::try_from(WIDTH).expect("a width"),
            ),
            (1, 0),
        ],
    }
}

/// One whole row, `row`, as a read of its line gives it.
fn row(row: &str) -> TerminalText {
    text(&[row])
}

/// What a window read gave, both outposts alike: the snapshot without its
/// times and its caret report, which are checked as they are read, the
/// caret report's line and the caret's offset in it, and whether the read
/// answered a caret key's starting point.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Read {
    snapshot: TerminalSnapshot,
    caret: (String, u32),
    point: bool,
}

/// mockapp's terminal, with an outpost reading it by remote operations
/// and one reading it by calls, each with the terminal's node.
struct Terminal {
    app: common::MockApp,
    hwnd: HWND,
    outposts: Vec<(OutpostUnderTest, NodeId)>,
    /// The UI Automation calls each outpost made for the last window read,
    /// the remote one's first: for it, the programs it ran.
    calls: std::cell::Cell<[u32; 2]>,
}

/// The anchors the two outposts hold for one place, the remote one's
/// first.
type Held = [TextAnchor; 2];

impl Terminal {
    /// mockapp's terminal of `terminal.json`, its screen `screen` rows
    /// high, showing `rows`, the caret at the text's start.
    fn start(name: &str, screen: u32, rows: &[String]) -> Self {
        let mut terminal = Self::of("terminal.json", name);
        common::apply(
            &mut terminal.app,
            terminal.hwnd,
            &format!("screen term {screen}"),
        );
        terminal.show(rows);
        terminal
    }

    /// mockapp's terminal of `fixture`, as the fixture shows it.
    fn of(fixture: &str, name: &str) -> Self {
        common::init_com();
        let title = common::unique_title(name);
        let mut app = common::spawn(fixture, "uia", &title);
        let hwnd = common::find_window(&title);
        let uia = Uia::new().expect("a UIA client");
        let element = common::named_element(&uia, hwnd, "Terminal");
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
                let (answer, _) = outpost.batch(hwnd, WindowVerdict::Uia, focus);
                let Outcome::Result(Reply::Steps(steps)) = answer.outcome else {
                    panic!("steps, not {:?}", answer.outcome)
                };
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
            calls: std::cell::Cell::new([0; 2]),
        }
    }

    /// Shows `rows`, the caret at the text's start, raising no event.
    fn show(&mut self, rows: &[String]) {
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let text = padded(&rows).replace('\r', "\\r").replace('\n', "\\n");
        common::apply(&mut self.app, self.hwnd, &format!("set-text term {text}"));
        common::apply(&mut self.app, self.hwnd, "caret term 0 0");
    }

    /// The answer of each outpost to the batch `build` writes for it, the
    /// remote one's first, with the clock's readings around each.
    fn answers(&self, build: impl Fn(usize, NodeId) -> Batch) -> Vec<(Answer, (u64, u64))> {
        self.outposts
            .iter()
            .enumerate()
            .map(|(which, (outpost, node))| {
                let before = now_us();
                let (answer, _) = outpost.batch(self.hwnd, WindowVerdict::Uia, build(which, *node));
                (answer, (before, now_us()))
            })
            .collect()
    }

    /// The window read each outpost's `ask` names, run by that outpost and
    /// read back by Core's reader with the answer's own times, which lie
    /// between the clock's readings around the request and are the
    /// snapshot's.
    fn window_each(
        &self,
        ask: impl Fn(usize) -> WindowAsk,
    ) -> [Result<WindowReply, TextFailure>; 2] {
        let replies: Vec<Result<WindowReply, TextFailure>> = self
            .outposts
            .iter()
            .enumerate()
            .map(|(which, (outpost, node))| {
                let ask = ask(which);
                let (sent, steps) = window_batch(*node, &ask);
                let before = now_us();
                let (answer, made) =
                    outpost.batch(self.hwnd, WindowVerdict::Uia, sent.batch().clone());
                let after = now_us();
                let mut calls = self.calls.get();
                calls[which] = made.uia;
                self.calls.set(calls);
                let (began, ended) = (answer.began_us, answer.ended_us);
                assert!(
                    before <= began && began <= ended && ended <= after,
                    "began {began} and ended {ended} between {before} and {after}"
                );
                let Outcome::Result(Reply::Steps(outcomes)) = answer.outcome else {
                    panic!("steps, not {:?}", answer.outcome)
                };
                // What the caret read held, a key's starting point.
                let held = outcomes.iter().find_map(|outcome| match outcome {
                    StepOutcome::Done(Done::Caret(caret)) => Some(caret.held),
                    _ => None,
                });
                let answers = Answers::parse(&sent, Reply::Steps(outcomes))
                    .expect("outcomes of the steps' kinds");
                let reply = window_of(&answers, &steps, &ask, (began, ended))?;
                assert_eq!(reply.point, held.filter(|_| ask.starting));
                assert_eq!(
                    (
                        reply.started_us,
                        reply.snapshot.started_ms,
                        reply.snapshot.ended_ms,
                        reply.snapshot.waited
                    ),
                    (began, began / 1_000, ended / 1_000, false)
                );
                Ok(reply)
            })
            .collect();
        replies.try_into().expect("two outposts")
    }

    /// The window read `ask` names for each outpost, which both read
    /// alike: what it gave, the anchors held at the next read's start, and
    /// the starting points.
    fn window(&self, ask: impl Fn(usize) -> WindowAsk) -> (Read, Held, Option<Held>) {
        let read = |reply: Result<WindowReply, TextFailure>| {
            let reply = reply.expect("a read");
            let mut snapshot = reply.snapshot;
            let report = snapshot.caret.take().expect("a caret report");
            assert_eq!(report.selection, None);
            (snapshot.started_ms, snapshot.ended_ms) = (0, 0);
            (
                Read {
                    snapshot,
                    caret: (report.line.text, report.line.offset),
                    point: reply.point.is_some(),
                },
                reply.start,
                reply.point,
            )
        };
        let [remote, calls] = self.window_each(ask).map(read);
        assert_eq!(
            remote.0, calls.0,
            "the reads by remote operations and by calls differ"
        );
        (
            remote.0,
            [remote.1, calls.1],
            remote.2.zip(calls.2).map(|(remote, calls)| [remote, calls]),
        )
    }

    /// The history read `ask` names for each outpost, which both read
    /// alike.
    fn history(&self, ask: impl Fn(usize) -> HistoryAsk) -> Result<(String, u32), TextFailure> {
        let read: Vec<Result<(String, u32), TextFailure>> = self
            .outposts
            .iter()
            .enumerate()
            .map(|(which, (outpost, node))| {
                let (sent, steps) = history_batch(*node, &ask(which));
                let (answer, _) =
                    outpost.batch(self.hwnd, WindowVerdict::Uia, sent.batch().clone());
                let Outcome::Result(reply) = answer.outcome else {
                    panic!("steps, not {:?}", answer.outcome)
                };
                let answers = Answers::parse(&sent, reply).expect("outcomes of the steps' kinds");
                history_of(&answers, &steps)
            })
            .collect();
        assert_eq!(
            read[0], read[1],
            "the history reads by remote operations and by calls differ"
        );
        read[0].clone()
    }

    /// How many lines the place each outpost holds at `held` lies from the
    /// text's end, which both count alike.
    fn rows_to_end(&self, held: Held) -> i32 {
        let rows: Vec<i32> = self
            .answers(|which, node| {
                let mut batch = Builder::<Probing>::named();
                batch.move_point(
                    node,
                    TextPoint::At(TextPosition::at(held[which])).into(),
                    TextMovement {
                        unit: TextUnit::Line,
                        count: 1_000_000,
                    },
                );
                batch.finish().batch().clone()
            })
            .into_iter()
            .map(|(answer, _)| match answer.outcome {
                Outcome::Result(Reply::Steps(steps)) => match steps.as_slice() {
                    [StepOutcome::Done(Done::Move(moved))] => moved.moved,
                    other => panic!("a move, not {other:?}"),
                },
                other => panic!("steps, not {other:?}"),
            })
            .collect();
        assert_eq!(rows[0], rows[1], "the two outposts' places differ");
        rows[0]
    }
}

/// The window read `read` with no colours, not a starting read.
fn plain(read: WindowRead) -> WindowAsk {
    WindowAsk {
        read,
        colours: false,
        starting: false,
        output: false,
    }
}

/// A read from `start` at a screen of 3 rows of text.
fn from(start: TextAnchor) -> WindowAsk {
    plain(WindowRead::From { start, height: 3 })
}

fn height(rows: u32) -> NonZeroU32 {
    NonZeroU32::new(rows).expect("a height")
}

/// The read of the 8 numbered rows and the prompt, the screen its last 3
/// rows, the caret at the text's start, 9 rows from the empty line at its
/// end: `text` and the flags as given.
fn eight_rows(text: TerminalText, (from_start, above): (bool, u32)) -> Read {
    Read {
        snapshot: TerminalSnapshot {
            text,
            screen: self::text(&["r 7", "r 8", "ready>"]),
            from_start,
            shrank: false,
            at_document_start: false,
            alternate: false,
            first_row: row("r 1"),
            caret_rows_to_end: 9,
            caret_line: row("r 1"),
            caret_column: 0,
            above,
            started_ms: 0,
            ended_ms: 0,
            waited: false,
            colours: None,
            caret: None,
        },
        caret: (format!("{:WIDTH$}\r\n", "r 1"), 0),
        point: false,
    }
}

/// A first read takes the screen alone and names its top; a read from
/// there, a screen of 3 rows above it, uses the start while it lies at or
/// before the next read's, with nothing new and with output written
/// since; and after a clear that leaves the start past the text's end, the
/// start is refused and the read starts where the next will.
fn a_kept_start_is_used_until_the_text_clears_below_it_both_ways() {
    let mut terminal = Terminal::start("mockapp-reader-kept", 4, &numbered(8));
    let (first, start, point) = terminal.window(|_| plain(WindowRead::First));
    assert_eq!(
        first,
        eight_rows(text(&["r 7", "r 8", "ready>"]), (false, 0))
    );
    assert_eq!(point, None);
    assert_eq!(terminal.rows_to_end(start), 3, "the screen's top");
    // With nothing new, the start is exactly where the next read starts,
    // and is used: the read begins 3 rows above it, at the fourth row.
    let (again, start, _) = terminal.window(|which| from(start[which]));
    assert_eq!(
        again,
        eight_rows(
            text(&["r 4", "r 5", "r 6", "r 7", "r 8", "ready>"]),
            (true, 3)
        )
    );
    assert_eq!(terminal.rows_to_end(start), 3, "the next start");
    let mut rows = numbered(8);
    rows.pop();
    rows.extend(["ready> ls", "a", "b", "ready>"].map(str::to_owned));
    terminal.show(&rows);
    // Output written since: the read runs from 3 rows above the old
    // screen's top to the end, past the screen.
    let (since, start, _) = terminal.window(|which| from(start[which]));
    assert_eq!(
        since,
        Read {
            snapshot: TerminalSnapshot {
                screen: text(&["a", "b", "ready>"]),
                caret_rows_to_end: 12,
                ..eight_rows(
                    text(&[
                        "r 4",
                        "r 5",
                        "r 6",
                        "r 7",
                        "r 8",
                        "ready> ls",
                        "a",
                        "b",
                        "ready>"
                    ]),
                    (true, 3)
                )
                .snapshot
            },
            ..eight_rows(TerminalText::default(), (true, 3))
        }
    );
    assert_eq!(terminal.rows_to_end(start), 3, "the next start");
    terminal.show(&["cleared".to_owned(), "ready>".to_owned()]);
    // The text cleared below the kept start: the start is refused, and the
    // read starts where the next will, the text's start.
    let (cleared, _, _) = terminal.window(|which| from(start[which]));
    terminal.app.quit();
    assert_eq!(
        cleared,
        Read {
            snapshot: TerminalSnapshot {
                text: text(&["cleared", "ready>"]),
                screen: text(&["cleared", "ready>"]),
                from_start: false,
                shrank: true,
                at_document_start: true,
                alternate: true,
                first_row: row("cleared"),
                caret_rows_to_end: 2,
                caret_line: row("cleared"),
                caret_column: 0,
                above: 0,
                started_ms: 0,
                ended_ms: 0,
                waited: false,
                colours: None,
                caret: None,
            },
            caret: (format!("{:WIDTH$}\r\n", "cleared"), 0),
            point: false,
        }
    );
}

/// A first read again with its history block counts its start from the
/// text's end, not the screen's top, and takes as many rows above it; a
/// whole read takes the text from its start.
fn a_read_with_no_start_takes_its_history_block_or_the_whole_text_both_ways() {
    let terminal = Terminal::start("mockapp-reader-unkept", 4, &numbered(8));
    // A screen of 2 rows named: the next read starts 2 rows from the end,
    // and this one 2 rows above that.
    let (block, start, _) =
        terminal.window(|_| plain(WindowRead::HistoryBlock { height: height(2) }));
    assert_eq!(
        block,
        eight_rows(text(&["r 6", "r 7", "r 8", "ready>"]), (false, 2))
    );
    assert_eq!(terminal.rows_to_end(start), 2, "two rows from the end");
    let (whole, start, _) = terminal.window(|_| plain(WindowRead::Whole { height: 3 }));
    let rows_to_end = terminal.rows_to_end(start);
    terminal.app.quit();
    let rows = numbered(8);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    assert_eq!(
        whole,
        Read {
            snapshot: TerminalSnapshot {
                at_document_start: true,
                ..eight_rows(text(&rows), (false, 0)).snapshot
            },
            ..eight_rows(TerminalText::default(), (false, 0))
        }
    );
    assert_eq!(rows_to_end, 3, "the next start, a screen from the end");
}

/// A read takes the rows asked above its start as far as the text's start:
/// with 5 rows and the prompt, the screen's top is 3 rows down, so a screen
/// of 3 rows above it is exactly the text's start, which the probe finds;
/// with 4, only 2 rows lie above it. History reads take any count of rows
/// above the start, as many as asked and as far as the text's start, and
/// the whole text.
fn a_read_takes_the_rows_above_its_start_as_far_as_the_texts_start_both_ways() {
    let read = |count: u32| {
        let terminal = Terminal::start("mockapp-reader-above", 4, &numbered(count));
        let (_, start, _) = terminal.window(|_| plain(WindowRead::First));
        let (read, _, _) = terminal.window(|which| from(start[which]));
        (terminal, start, read.snapshot)
    };
    let (exact, _, snapshot) = read(5);
    exact.app.quit();
    assert_eq!(
        (
            snapshot.text,
            snapshot.from_start,
            snapshot.above,
            snapshot.at_document_start
        ),
        (
            text(&["r 1", "r 2", "r 3", "r 4", "r 5", "ready>"]),
            true,
            3,
            true
        )
    );
    let (fewer, _, snapshot) = read(4);
    fewer.app.quit();
    assert_eq!(
        (
            snapshot.text,
            snapshot.from_start,
            snapshot.above,
            snapshot.at_document_start
        ),
        (text(&["r 1", "r 2", "r 3", "r 4", "ready>"]), true, 2, true)
    );
    let (terminal, start, _) = read(8);
    let above = |above| {
        terminal.history(|which| HistoryAsk::Above {
            start: start[which],
            above,
            height: height(3),
        })
    };
    let rows = numbered(8);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let (none, two, past) = (above(0), above(2), above(20));
    let whole = terminal.history(|_| HistoryAsk::Whole);
    terminal.app.quit();
    assert_eq!(none, Ok((padded(&rows[6..]), 0)));
    // Two rows above the kept start.
    assert_eq!(two, Ok((padded(&rows[4..]), 2)));
    // More rows than lie above the start: the read stops at the text's
    // start, 6 rows up.
    assert_eq!(past, Ok((padded(&rows), 6)));
    assert_eq!(whole, Ok((padded(&rows), 0)));
}

/// The caret's line, its column and its rows to the text's end are the
/// caret's, read first for a caret key's starting read, which alone
/// answers the key's starting point, what its caret read held; and a provider
/// that reports no selection (`terminal_no_selection.json`) has no caret
/// rows, though its caret is read at the text's start, 9 rows from the
/// end.
fn the_caret_is_read_with_its_line_and_a_starting_read_holds_it_both_ways() {
    let mut terminal = Terminal::start("mockapp-reader-caret", 4, &numbered(8));
    // The third row's fourth cell: each row is 12 units with its break.
    common::apply(&mut terminal.app, terminal.hwnd, "caret term 27 27");
    let moved = |starting| Read {
        snapshot: TerminalSnapshot {
            caret_rows_to_end: 7,
            caret_line: row("r 3"),
            caret_column: 3,
            ..eight_rows(text(&["r 7", "r 8", "ready>"]), (false, 0)).snapshot
        },
        caret: (format!("{:WIDTH$}\r\n", "r 3"), 3),
        point: starting,
    };
    let (read, _, point) = terminal.window(|_| plain(WindowRead::First));
    assert_eq!((read, point), (moved(false), None));
    let (read, _, point) = terminal.window(|_| WindowAsk {
        starting: true,
        ..plain(WindowRead::First)
    });
    assert_eq!(read, moved(true));
    assert!(point.is_some(), "a starting point");
    terminal.app.quit();
    let mut none = Terminal::of("terminal_no_selection.json", "mockapp-reader-no-selection");
    common::apply(&mut none.app, none.hwnd, "screen term 4");
    none.show(&numbered(8));
    let (read, _, _) = none.window(|_| plain(WindowRead::First));
    none.app.quit();
    assert_eq!(
        read,
        Read {
            snapshot: TerminalSnapshot {
                caret_rows_to_end: 0,
                ..eight_rows(text(&["r 7", "r 8", "ready>"]), (false, 0)).snapshot
            },
            ..eight_rows(TerminalText::default(), (false, 0))
        }
    );
}

/// A run of `length` cells from `column`, its foreground and background as
/// given.
fn run(column: u32, length: u32, foreground: u32, background: u32) -> TerminalColourRun {
    TerminalColourRun {
        column,
        length,
        foreground,
        background,
    }
}

/// A read that takes the colours gives each row of the screen as exact
/// runs, from a provider that answers colours over a range without its
/// last character, as both terminals do (`terminal-colours.json`): a plain
/// row; two red cells first; only the last cell's background green; and
/// one blue cell at the seventeenth.
///
/// By remote operations, an outpost's first coloured read is two programs,
/// the read's and the colours'. Its next reads the colours in the read's
/// own program, at the size the first found, and gives the same runs. On a
/// screen of another width the size kept is wrong, so the colours the
/// read's program took are left and the colour program reads them: two
/// programs once, and one again from the read after. By calls the read is
/// the same calls each time.
fn colours_are_read_as_exact_runs_both_ways() {
    let mut terminal = Terminal::of("terminal-colours.json", "mockapp-reader-colours");
    let (white, red, blue, green) = (0x00FF_FFFF, 0x0000_00FF, 0x00FF_0000, 0x0000_FF00);
    let colours = |_| WindowAsk {
        colours: true,
        ..plain(WindowRead::First)
    };
    let (coloured, _, _) = terminal.window(colours);
    let first_calls = terminal.calls.get();
    let (again, _, _) = terminal.window(colours);
    let again_calls = terminal.calls.get();
    let (plain_read, _, _) = terminal.window(|_| plain(WindowRead::First));
    // Five rows of 16 cells: the styles stay at their characters.
    common::apply(
        &mut terminal.app,
        terminal.hwnd,
        r"set-text term plain row       \n     ab in red  \n         last ce\nll on green  red\n at sixteen     \n",
    );
    let (resized, _, _) = terminal.window(colours);
    let resized_calls = terminal.calls.get();
    let (resized_again, _, _) = terminal.window(colours);
    let resized_again_calls = terminal.calls.get();
    terminal.app.quit();
    assert_eq!(
        coloured.snapshot.colours,
        Some(vec![
            vec![run(0, 20, 0, white)],
            vec![run(0, 2, red, white), run(2, 18, 0, white)],
            vec![run(0, 19, 0, white), run(19, 1, 0, green)],
            vec![
                run(0, 16, 0, white),
                run(16, 1, blue, white),
                run(17, 3, 0, white),
            ],
        ])
    );
    assert_eq!(again, coloured, "the read's own program reads the same");
    assert_eq!(
        resized.snapshot.colours,
        Some(vec![
            vec![run(0, 16, 0, white)],
            vec![
                run(0, 4, 0, white),
                run(4, 2, red, white),
                run(6, 10, 0, white)
            ],
            vec![run(0, 16, 0, white)],
            vec![
                run(0, 10, 0, white),
                run(10, 1, 0, green),
                run(11, 5, 0, white)
            ],
            vec![
                run(0, 11, 0, white),
                run(11, 1, blue, white),
                run(12, 4, 0, white)
            ],
        ])
    );
    assert_eq!(resized_again, resized, "at the new size, the same");
    assert_eq!(
        [first_calls, again_calls, resized_calls, resized_again_calls],
        [[2, 181], [1, 181], [2, 214], [1, 214]],
        "the programs the remote outpost ran, and the calls the other made"
    );
    // But for its colours, the read is the read without them.
    assert_eq!(
        Read {
            snapshot: TerminalSnapshot {
                colours: None,
                ..coloured.snapshot
            },
            ..coloured
        },
        plain_read
    );
}

/// A start Core names that the outpost no longer holds is the terminal
/// gone, for a window read and for a history read, by remote operations
/// and by calls.
fn a_start_no_longer_held_is_the_terminal_gone_both_ways() {
    let terminal = Terminal::start("mockapp-reader-unheld", 4, &numbered(8));
    let unheld = TextAnchor(9_999);
    let window = terminal.window_each(|_| from(unheld));
    let history = terminal.history(|_| HistoryAsk::Above {
        start: unheld,
        above: 2,
        height: height(3),
    });
    terminal.app.quit();
    assert_eq!(window, [Err(TextFailure::Gone), Err(TextFailure::Gone)]);
    assert_eq!(history, Err(TextFailure::Gone));
}

/// A text whose last row has no line break after it, as the console host
/// gives a prompt: the held end is the text's end, so every read ends with
/// that row. Both real terminals leave a range at the text's end there
/// when it is moved forward by lines, 0 lines moved (probed 2026-11-26 in
/// the console host and Windows Terminal).
fn a_last_row_with_no_line_break_is_read_both_ways() {
    let mut terminal = Terminal::of("terminal.json", "mockapp-reader-no-break");
    common::apply(&mut terminal.app, terminal.hwnd, "screen term 2");
    let shown = format!("{}ready>", padded(&["r 1", "r 2", "r 3"]))
        .replace('\r', "\\r")
        .replace('\n', "\\n");
    common::apply(
        &mut terminal.app,
        terminal.hwnd,
        &format!("set-text term {shown}"),
    );
    common::apply(&mut terminal.app, terminal.hwnd, "caret term 0 0");
    let texts: Vec<String> = [
        WindowRead::First,
        WindowRead::HistoryBlock { height: height(2) },
        WindowRead::Whole { height: 2 },
    ]
    .into_iter()
    .map(|read| terminal.window(|_| plain(read)).0.snapshot.text.text)
    .collect();
    terminal.app.quit();
    assert_eq!(
        texts,
        [
            "r 3\r\nready>",
            "r 1\r\nr 2\r\nr 3\r\nready>",
            "r 1\r\nr 2\r\nr 3\r\nready>",
        ]
    );
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_kept_start_is_used_until_the_text_clears_below_it_both_ways",
            a_kept_start_is_used_until_the_text_clears_below_it_both_ways,
        ),
        (
            "a_read_with_no_start_takes_its_history_block_or_the_whole_text_both_ways",
            a_read_with_no_start_takes_its_history_block_or_the_whole_text_both_ways,
        ),
        (
            "a_read_takes_the_rows_above_its_start_as_far_as_the_texts_start_both_ways",
            a_read_takes_the_rows_above_its_start_as_far_as_the_texts_start_both_ways,
        ),
        (
            "the_caret_is_read_with_its_line_and_a_starting_read_holds_it_both_ways",
            the_caret_is_read_with_its_line_and_a_starting_read_holds_it_both_ways,
        ),
        (
            "colours_are_read_as_exact_runs_both_ways",
            colours_are_read_as_exact_runs_both_ways,
        ),
        (
            "a_start_no_longer_held_is_the_terminal_gone_both_ways",
            a_start_no_longer_held_is_the_terminal_gone_both_ways,
        ),
        (
            "a_last_row_with_no_line_break_is_read_both_ways",
            a_last_row_with_no_line_break_is_read_both_ways,
        ),
    ]);
}
