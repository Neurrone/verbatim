//! Core's terminal reader (`verbatim_core::terminal::reader`) compared
//! with the simulated terminal's reference read: for one state of the
//! terminal, the reader's window batch, answered step by step as the
//! terminal's provider answers each step's calls (`terminal_support`'s
//! `steps`), reads back as the read the reference read
//! (`verbatim_term_sim::Terminal::snapshot`, the read the conformance
//! expectations record) gives, field by field; and its history batch reads back as the
//! reference's text and rows. The reader's command search batch is compared
//! with the old search, which Core still sends (Dickson, 2026-11-26,
//! `docs/simplification-review/builds.md`).
//!
//! The states are every chunk of every stream of the conformance corpus,
//! in both terminals, and the shapes Core's terminal tests use: a full
//! history and its rotation, a clear that leaves a start past the text's
//! end, padding and leading spaces, a blank caret row, a caret at the
//! right edge, the alternate screen, and a screen height named that the
//! screen no longer has, as after a resize (the simulated terminal itself
//! keeps its size).

#[path = "support/readers.rs"]
mod readers;
mod terminal_support;

use std::fmt::Write as _;
use std::num::NonZeroU32;

use terminal_support::{Outpost, node};
use verbatim_core::batch::Answers;
use verbatim_core::terminal::reader::{
    HistoryAsk, SearchAsk, WindowAsk, WindowRead, WindowReply, history_batch, history_of,
    search_batch, search_of, window_batch, window_of,
};
use verbatim_model::request::Reply;
use verbatim_model::{TerminalMatch, TerminalText, TextAnchor, TextFailure, TextReply};
use verbatim_term_sim::conformance::{history, streams};
use verbatim_term_sim::{Flavor, Query, Range};

const FLAVORS: [Flavor; 2] = [Flavor::ConsoleHost, Flavor::WindowsTerminal];

/// When the reads compared began and ended, in microseconds.
const TIMES: (u64, u64) = (5_000, 7_999);

/// The window read `ask` names as Core's reader reads it: its batch,
/// answered by the simulated terminal step by step, read back.
fn window(outpost: &mut Outpost, ask: &WindowAsk) -> Result<WindowReply, TextFailure> {
    let (sent, steps) = window_batch(node(), ask);
    let outcomes = outpost.run(sent.batch());
    let answers =
        Answers::parse(&sent, Reply::Steps(outcomes)).expect("outcomes of the steps' kinds");
    window_of(&answers, &steps, ask, TIMES)
}

/// The history read `ask` names as Core's reader reads it.
fn history_read(outpost: &mut Outpost, ask: &HistoryAsk) -> Result<(String, u32), TextFailure> {
    let (sent, steps) = history_batch(node(), ask);
    let outcomes = outpost.run(sent.batch());
    let answers =
        Answers::parse(&sent, Reply::Steps(outcomes)).expect("outcomes of the steps' kinds");
    history_of(&answers, &steps)
}

/// The reference read's query for the window read `read`, as the old
/// operation asked it: a start named and a history block begin a screen
/// above their start, a first read and a whole one at it.
fn query_of(outpost: &Outpost, read: WindowRead) -> Query {
    let (start, screen_rows, whole, above) = match read {
        WindowRead::First => (None, 0, false, 0),
        WindowRead::From { start, height } => (Some(start), height, false, height),
        WindowRead::HistoryBlock { height } => (None, height.get(), false, height.get()),
        WindowRead::Whole { height } => (None, height, true, 0),
    };
    Query {
        start: start.map(|start| Range::at(outpost.held(start).expect("a held start"))),
        screen_rows,
        whole,
        above,
    }
}

/// The fields in which the read Core's reader makes for `ask` differs from
/// the reference read, each with both values; with the anchor the reader
/// answers for the next read's start.
fn read_differences(outpost: &mut Outpost, ask: &WindowAsk) -> (Vec<String>, TextAnchor) {
    let query = query_of(outpost, ask.read);
    let (_, old, old_next) = outpost.snapshot(&query, ask.colours, 0);
    let caret = outpost.sim.caret().start;
    let reply = window(outpost, ask).unwrap_or_else(|failure| panic!("{ask:?}: {failure:?}"));
    let new = &reply.snapshot;
    let mut differences = Vec::new();
    macro_rules! field {
        ($($name:ident),*) => {$(
            if old.$name != new.$name {
                differences.push(format!(
                    "{}: the reference {:?}, the reader {:?}",
                    stringify!($name), old.$name, new.$name
                ));
            }
        )*};
    }
    field!(
        text,
        screen,
        from_start,
        shrank,
        at_document_start,
        alternate,
        first_row,
        caret_rows_to_end,
        caret_line,
        caret_column,
        above,
        colours
    );
    let mut same = |name: &str, old: String, new: String| {
        if old != new {
            differences.push(format!("{name}: expected {old}, the reader {new}"));
        }
    };
    let held = |anchor| format!("{:?}", outpost.held(anchor));
    same(
        "next start",
        format!("{:?}", Some(old_next.start)),
        held(reply.start),
    );
    // The read's times are its answer's, and a starting read alone
    // answers the key's starting point, held at the caret.
    same(
        "times",
        "(5, 7, false, 5000)".to_owned(),
        format!(
            "{:?}",
            (new.started_ms, new.ended_ms, new.waited, reply.started_us)
        ),
    );
    same(
        "starting point",
        format!("{:?}", ask.starting.then_some(Some(caret))),
        format!("{:?}", reply.point.map(|point| outpost.held(point))),
    );
    // The caret report's line is the caret's line with its padding, the
    // caret at its column.
    let report = new.caret.as_ref().expect("a caret report");
    same(
        "caret report",
        format!(
            "{:?}",
            (old.caret_line.padded(), Some(old.caret_column), None::<()>)
        ),
        format!(
            "{:?}",
            (
                &report.line.text,
                usize::try_from(report.line.offset)
                    .ok()
                    .and_then(|offset| report.line.text.get(..offset))
                    .and_then(|before| u32::try_from(before.chars().count()).ok()),
                report.selection.as_ref().map(|_| ()),
            )
        ),
    );
    (differences, reply.start)
}

/// The fields in which the history read Core's reader makes of `above`
/// rows above `start` at `height` differs from the reference read.
fn history_differences(
    outpost: &mut Outpost,
    (start, above, height): (TextAnchor, u32, u32),
) -> Vec<String> {
    let query = Query {
        start: Some(Range::at(outpost.held(start).expect("a held start"))),
        screen_rows: height,
        whole: false,
        above,
    };
    let old = outpost.sim.snapshot(&query).expect("a simulated read");
    let ask = HistoryAsk::Above {
        start,
        above,
        height: NonZeroU32::new(height).expect("a height"),
    };
    let new = history_read(outpost, &ask).unwrap_or_else(|failure| panic!("{ask:?}: {failure:?}"));
    if (old.text.as_str(), old.above) == (new.0.as_str(), new.1) {
        Vec::new()
    } else {
        vec![format!(
            "{ask:?}: the reference {:?}, the reader {new:?}",
            (&old.text, old.above)
        )]
    }
}

/// Compares every window read Core asks in the terminal's present state:
/// a first read, a first read again with its history block, the whole
/// document, and, from the start `kept` an earlier read named, a read from
/// it, each with its colours, as a caret key's starting read and not; and
/// the history reads from `kept`, with no rows above it, a screen of them
/// and more than the text holds. Returns the next start for a later state,
/// with what differed.
fn compare_reads(
    outpost: &mut Outpost,
    height: u32,
    kept: Option<TextAnchor>,
    state: &str,
    differences: &mut Vec<String>,
) -> TextAnchor {
    let rows = NonZeroU32::new(height).expect("a height");
    let mut reads = vec![
        WindowRead::First,
        WindowRead::HistoryBlock { height: rows },
        WindowRead::Whole { height },
    ];
    if let Some(start) = kept {
        reads.push(WindowRead::From { start, height });
        for above in [0, height, 100_000] {
            differences.extend(
                history_differences(outpost, (start, above, height))
                    .into_iter()
                    .map(|difference| format!("{state}: {difference}")),
            );
        }
    }
    let mut next = None;
    for read in reads {
        for starting in [false, true] {
            let ask = WindowAsk {
                read,
                colours: true,
                starting,
                output: false,
            };
            let (found, held) = read_differences(outpost, &ask);
            differences.extend(
                found
                    .into_iter()
                    .map(|difference| format!("{state}, {ask:?}: {difference}")),
            );
            // The start a read at the screen's height names is the one
            // Core keeps for the next read.
            if matches!(read, WindowRead::HistoryBlock { .. }) {
                next = Some(held);
            }
        }
    }
    next.expect("a read at the screen's height")
}

/// Fails with every difference found, the first twenty shown.
fn assert_none(differences: &[String]) {
    assert!(
        differences.is_empty(),
        "{} differences, the first:\n{}",
        differences.len(),
        differences
            .iter()
            .take(20)
            .fold(String::new(), |mut shown, difference| {
                let _ = writeln!(shown, "{difference}");
                shown
            })
    );
}

#[test]
fn the_reader_reads_the_reference_read_after_every_chunk_of_the_corpus() {
    let mut differences = Vec::new();
    for (name, stream) in streams() {
        for flavor in FLAVORS {
            let mut outpost = Outpost::new(flavor, 120, 30, history(flavor));
            let mut kept = None;
            for (chunk, bytes) in stream.chunks.iter().enumerate() {
                outpost.sim.write(bytes.as_bytes());
                let state = format!("{name} in {flavor:?}, chunk {chunk}");
                kept = Some(compare_reads(
                    &mut outpost,
                    30,
                    kept,
                    &state,
                    &mut differences,
                ));
            }
        }
    }
    assert_none(&differences);
}

/// Lines `range` named `name`, each ended.
fn written(name: &str, range: std::ops::RangeInclusive<u32>) -> String {
    range.fold(String::new(), |mut text, line| {
        let _ = write!(text, "{name} {line}\r\n");
        text
    })
}

/// The shapes Core's terminal tests use, each a name, the screen's width
/// and height, the history's rows (`None` for the terminal's default),
/// and what is written, the terminal compared after each write.
type Shape = (&'static str, (usize, usize, Option<usize>), Vec<String>);

fn shapes() -> Vec<Shape> {
    vec![
        (
            "a full history that rotates",
            (120, 30, None),
            vec![
                format!("{}ready> command", written("fill line", 1..=9100)),
                format!("\r\n{}", written("flood line", 1..=12)),
                format!("{}ready> ", written("flood line", 13..=2000)),
                format!("{}ready> ", written("after line", 1..=12_000)),
            ],
        ),
        (
            "a history filling from empty",
            (120, 30, None),
            vec![
                "ready> command".to_owned(),
                format!("\r\n{}ready> ", written("fill line", 1..=9100)),
            ],
        ),
        (
            "a clear that leaves the last start past the text's end",
            (120, 30, None),
            vec![
                written("old line", 1..=200),
                format!("\x1b[H\x1b[2J\x1b[3J{}", written("cleared line", 1..=3)),
                format!("\x1b[H\x1b[2J\x1b[3J{}", written("cleared line", 1..=100)),
            ],
        ),
        (
            "padding, leading spaces and a wrapped line",
            (20, 4, Some(100)),
            vec![
                "   indented\r\ntrailing   \r\n".to_owned(),
                "a line of forty cells that wraps twice..\r\n".to_owned(),
                "\ttabbed\r\n  $ ".to_owned(),
                written("line", 1..=30),
            ],
        ),
        (
            "a blank caret row, and a caret moved into the text",
            (20, 4, Some(100)),
            vec![
                "ab\r\ncd\r\n".to_owned(),
                "\r\n\r\n".to_owned(),
                "ready> abc\x1b[1;4H".to_owned(),
                "\x1b[4;1H".to_owned(),
                written("line", 1..=120),
                "\r\n\r\n\r\n".to_owned(),
            ],
        ),
        (
            "a caret at the right edge",
            (20, 4, Some(10)),
            vec![
                "exactly twenty cells".to_owned(),
                "!".to_owned(),
                written("line", 1..=20),
                "exactly twenty cells".to_owned(),
            ],
        ),
        (
            "the alternate screen and back",
            (120, 30, None),
            vec![
                format!("{}ready> command", written("line", 1..=50)),
                "\r\n\x1b[?1049h\x1b[H\x1b[2Jscreen row 1\r\nscreen row 2".to_owned(),
                "\x1b[30;1Hstatus".to_owned(),
                "\x1b[?1049lclosed\r\nready> ".to_owned(),
            ],
        ),
        (
            "a footer below a scroll region",
            (120, 30, None),
            vec![
                "\r\n\x1b[2J\x1b[1;29r\x1b[30;1Hstatus: busy\x1b[29;1H".to_owned(),
                written("flood line", 1..=40),
                "\x1b[r\x1b[2J\x1b[Hfooter gone\r\nready> ".to_owned(),
            ],
        ),
    ]
}

#[test]
fn the_reader_reads_the_reference_read_in_the_shapes_the_terminal_tests_use() {
    let mut differences = Vec::new();
    for (name, (width, height, rows), writes) in shapes() {
        for flavor in FLAVORS {
            let rows = rows.unwrap_or_else(|| history(flavor));
            let mut outpost = Outpost::new(flavor, width, height, rows);
            let mut kept = None;
            for (index, write) in writes.iter().enumerate() {
                outpost.sim.write(write.as_bytes());
                // The screen's own height, then the heights a resize
                // leaves named: a shorter and a taller screen's.
                let own = u32::try_from(height).expect("a small screen");
                for named in [own, own / 2, own * 2] {
                    let state = format!("{name} in {flavor:?}, write {index}, height {named}");
                    let next = compare_reads(&mut outpost, named, kept, &state, &mut differences);
                    if named == own {
                        kept = Some(next);
                    }
                }
            }
        }
    }
    assert_none(&differences);
}

/// The reference history read and the history read Core's reader makes
/// for the same start, rows above it and height, text and rows alike; and
/// the whole history read, the whole document.
#[test]
fn the_reader_reads_the_reference_history_read() {
    for flavor in FLAVORS {
        let mut outpost = Outpost::new(flavor, 20, 4, 100);
        outpost.sim.write(written("line", 1..=40).as_bytes());
        let start = outpost.sim.visible().start;
        let (old, new) = (outpost.mint(Range::at(start)), outpost.hold(start));
        for (above, whole) in [(13, false), (31, false), (500, false), (0, true)] {
            let (text, rows) = outpost.history(old, (above, whole, 4));
            let ask = if whole {
                HistoryAsk::Whole
            } else {
                HistoryAsk::Above {
                    start: new,
                    above,
                    height: NonZeroU32::new(4).expect("a height"),
                }
            };
            assert_eq!(
                history_read(&mut outpost, &ask),
                Ok((text, rows)),
                "{flavor:?}, {ask:?}"
            );
        }
    }
}

/// A terminal of 20 by 4 cells in each flavor, `lines` lines written, with
/// the anchor held at the top of its screen.
fn small(lines: u32) -> Vec<(Outpost, TextAnchor)> {
    FLAVORS
        .into_iter()
        .map(|flavor| {
            let mut outpost = Outpost::new(flavor, 20, 4, 100);
            outpost.sim.write(written("line", 1..=lines).as_bytes());
            let top = outpost.sim.visible().start;
            let start = outpost.hold(top);
            (outpost, start)
        })
        .collect()
}

/// A read from `start` at a screen of 4 rows.
fn from(start: TextAnchor) -> WindowAsk {
    WindowAsk {
        read: WindowRead::From { start, height: 4 },
        colours: false,
        starting: false,
        output: false,
    }
}

/// A read that takes exactly the rows above its start that the text
/// holds starts at the text's start, which its probe finds, and one that
/// leaves a row above it does not: 7 lines and the empty row after them
/// put the screen's top 4 rows down, and 8 put it 5 down.
#[test]
fn a_read_that_takes_every_row_above_its_start_starts_at_the_texts_start() {
    for (lines, at_start, first) in [(7, true, "line 1"), (8, false, "line 2")] {
        for (mut outpost, start) in small(lines) {
            let reply = window(&mut outpost, &from(start)).expect("a read");
            let snapshot = reply.snapshot;
            assert_eq!(
                (
                    snapshot.from_start,
                    snapshot.above,
                    snapshot.at_document_start,
                    snapshot.text.text.lines().next(),
                ),
                (true, 4, at_start, Some(first)),
                "{lines} lines"
            );
        }
    }
}

/// A provider that reports no selection has no caret: the read counts no
/// rows from it, where the text's start, which stands in for it, lies 2
/// rows from the end, 2 lines written and the caret sent home; with a
/// selection reported at that same place, the rows are counted. Its line
/// is read as any caret's line.
#[test]
fn a_read_with_no_selection_reported_counts_no_caret_rows() {
    for (mut outpost, start) in small(2) {
        outpost.sim.write(b"\x1b[H");
        let rows = |reply: WindowReply| {
            (
                reply.snapshot.caret_rows_to_end,
                reply.snapshot.caret_column,
                reply.snapshot.caret_line.text.trim_end().to_owned(),
            )
        };
        let home = window(&mut outpost, &from(start)).expect("a read");
        assert_eq!(rows(home), (2, 0, "line 1".to_owned()));
        outpost.no_selection = true;
        let none = window(&mut outpost, &from(start)).expect("a read");
        assert_eq!(rows(none), (0, 0, "line 1".to_owned()));
    }
}

/// A start Core names that the outpost no longer holds is the terminal
/// gone, for a window read and for a history read.
#[test]
fn a_start_no_longer_held_is_the_terminal_gone() {
    for (mut outpost, _) in small(8) {
        let unheld = TextAnchor(9_999);
        assert_eq!(window(&mut outpost, &from(unheld)), Err(TextFailure::Gone));
        let history = HistoryAsk::Above {
            start: unheld,
            above: 3,
            height: NonZeroU32::new(4).expect("a height"),
        };
        assert_eq!(history_read(&mut outpost, &history), Err(TextFailure::Gone));
    }
}

/// A start the text no longer reaches, after a clear, is refused: the
/// read starts a screen above the next read's start and says the text
/// shrank; and its history read counts from the next read's start too.
#[test]
fn a_start_past_the_texts_end_reads_from_the_next_start() {
    for (mut outpost, start) in small(40) {
        outpost.sim.write(b"\x1b[H\x1b[2J\x1b[3Jcleared\r\n");
        let reply = window(&mut outpost, &from(start)).expect("a read");
        let snapshot = reply.snapshot;
        assert_eq!(
            (
                snapshot.from_start,
                snapshot.shrank,
                snapshot.at_document_start
            ),
            (false, true, true)
        );
        assert_eq!(snapshot.text.text.lines().next(), Some("cleared"));
    }
}

/// The reader's search for `needle`, each way from the row `from` rows
/// above `start`, each match read for `rows` rows.
fn searched(
    outpost: &mut Outpost,
    start: TextAnchor,
    needle: &str,
    (from, rows): (i32, u8),
) -> Vec<TerminalMatch> {
    let ask = SearchAsk::new(start, needle.to_owned(), from, u32::from(rows));
    let (sent, steps) = search_batch(node(), &ask);
    let outcomes = outpost.run(sent.batch());
    let answers = Answers::parse(&sent, Reply::Steps(outcomes.clone()))
        .expect("outcomes of the steps' kinds");
    search_of(&answers, &steps).unwrap_or_else(|failure| panic!("{failure:?}: {outcomes:#?}"))
}

/// The old search and the reader's search batch answer alike, for a
/// needle that starts rows above and below the row searched from, one
/// that starts more rows than a search keeps, one that starts none, and
/// one found only within rows.
#[test]
fn the_readers_search_answers_as_the_old_search() {
    for flavor in FLAVORS {
        let mut outpost = Outpost::new(flavor, 30, 4, 200);
        let mut text = String::new();
        for line in 1..=30 {
            let _ = write!(text, "$ echo {line}\r\noutput of $ echo {line}\r\n");
        }
        text.push_str("  $ echo indented\r\n$ ");
        outpost.sim.write(text.as_bytes());
        let start = outpost.sim.visible().start;
        let (old, new) = (outpost.mint(Range::at(start)), outpost.hold(start));
        for needle in ["$ echo 7", "$ echo 2", "$ echo", "$ nothing", "output"] {
            for (from, rows) in [(0, 1), (19, 1), (40, 2), (-2, 1), (500, 1)] {
                let TextReply::TerminalFound { matches } =
                    outpost.find(old, needle, (from, u32::from(rows)))
                else {
                    panic!("a search's reply");
                };
                let composed = searched(&mut outpost, new, needle, (from, rows));
                let unpadded = |matches: &[TerminalMatch]| -> Vec<(i32, String)> {
                    matches
                        .iter()
                        .map(|found| (found.above, TerminalText::unpadded(&found.text).text))
                        .collect()
                };
                assert_eq!(
                    unpadded(&composed),
                    unpadded(&matches),
                    "{flavor:?}, {needle:?} from {from}, {rows} rows"
                );
            }
        }
    }
}
