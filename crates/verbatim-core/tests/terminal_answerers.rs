//! The simulated terminal's two answerers compared (`terminal_support`):
//! for one state of the terminal, what the generic text steps answer
//! composes to the same read, history and search as the terminal's old
//! operations answer, field by field. Core's reader built from generic
//! steps is proven against the old snapshot with this comparison
//! (Dickson, 2026-11-26, `docs/simplification-review/builds.md`).
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

use terminal_support::steps::{
    Composed, ReadAsk, add_read, add_search, matches_of, query_of, read_of,
};
use terminal_support::{Outpost, node};
use verbatim_core::batch::{Answers, Builder};
use verbatim_model::request::Reply;
use verbatim_model::{TerminalMatch, TerminalSnapshot, TerminalText, TextAnchor, TextReply};
use verbatim_term_sim::conformance::{history, streams};
use verbatim_term_sim::{Flavor, Range};

/// The brand of this file's batches.
enum Compared {}

const FLAVORS: [Flavor; 2] = [Flavor::ConsoleHost, Flavor::WindowsTerminal];

/// The read `ask` names, composed of the generic steps' answers.
fn composed(outpost: &mut Outpost, ask: &ReadAsk) -> Composed {
    let mut batch = Builder::<Compared>::named();
    let steps = add_read(&mut batch, node(), ask);
    let sent = batch.finish();
    let outcomes = outpost.run(sent.batch());
    let answers = Answers::parse(&sent, Reply::Steps(outcomes.clone()))
        .expect("outcomes of the steps' kinds");
    read_of(&answers, &steps)
        .unwrap_or_else(|| panic!("a read composed for {ask:?}, not {outcomes:#?}"))
}

/// The fields in which the read the generic steps compose for `ask`
/// differs from the old operation's answer, each with both values; with
/// the anchor the generic steps hold at the next read's start.
fn read_differences(outpost: &mut Outpost, ask: &ReadAsk) -> (Vec<String>, TextAnchor) {
    let query = query_of(outpost, ask);
    let (_, old, old_next): (_, TerminalSnapshot, Range) = outpost.snapshot(&query, ask.colours, 0);
    let Composed {
        snapshot: new,
        next,
    } = composed(outpost, ask);
    let mut differences = Vec::new();
    macro_rules! field {
        ($($name:ident),*) => {$(
            if old.$name != new.$name {
                differences.push(format!(
                    "{}: the old operation {:?}, the steps {:?}",
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
    let new_next = outpost.held(next).expect("the next start held");
    if old_next != Range::at(new_next) {
        differences.push(format!(
            "next start: the old operation {old_next:?}, the steps {new_next:?}"
        ));
    }
    (differences, next)
}

/// Compares every kind of read Core asks in the terminal's present state:
/// a first read, a read at `height` with no start, a first read again with
/// its history block, the whole document, and, from the start `kept` an
/// earlier read named, a read with and without the rows above it, each
/// with its colours. Returns the next start for a later state, with what
/// differed.
fn compare_reads(
    outpost: &mut Outpost,
    height: u32,
    kept: Option<TextAnchor>,
    state: &str,
    differences: &mut Vec<String>,
) -> TextAnchor {
    let ask = |start, height, whole, above| ReadAsk {
        start,
        height,
        whole,
        above,
        colours: true,
    };
    let mut asks = vec![
        ask(None, 0, false, 0),
        ask(None, height, false, 0),
        ask(None, height, false, height),
        ask(None, height, true, 0),
    ];
    if kept.is_some() {
        asks.push(ask(kept, height, false, 0));
        asks.push(ask(kept, height, false, height));
        asks.push(ask(kept, height, true, 0));
    }
    let mut next = None;
    for ask in asks {
        let (found, held) = read_differences(outpost, &ask);
        differences.extend(
            found
                .into_iter()
                .map(|difference| format!("{state}, {ask:?}: {difference}")),
        );
        // The start a read at the screen's height names is the one Core
        // keeps for the next read.
        if ask.start.is_none() && ask.height == height && ask.above == 0 && !ask.whole {
            next = Some(held);
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
fn the_steps_compose_the_old_read_after_every_chunk_of_the_corpus() {
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
fn the_steps_compose_the_old_read_in_the_shapes_the_terminal_tests_use() {
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

/// The old history read and the read the steps compose for the same
/// start, rows above it and height.
#[test]
fn the_steps_compose_the_old_history_read() {
    for flavor in FLAVORS {
        let mut outpost = Outpost::new(flavor, 20, 4, 100);
        outpost.sim.write(written("line", 1..=40).as_bytes());
        let start = outpost.sim.visible().start;
        let (old, new) = (outpost.mint(Range::at(start)), outpost.hold(start));
        for (above, whole) in [(13, false), (31, false), (500, false), (0, true)] {
            let TextReply::TerminalHistory { text, above: rows } =
                outpost.history(old, (above, whole, 4))
            else {
                panic!("a history read's reply");
            };
            let composed = composed(
                &mut outpost,
                &ReadAsk {
                    start: Some(new),
                    height: 4,
                    whole,
                    above,
                    colours: false,
                },
            );
            let state = format!("{flavor:?}, {above} rows above, whole {whole}");
            assert_eq!(
                composed.snapshot.text,
                TerminalText::unpadded(&text),
                "{state}"
            );
            assert_eq!(composed.snapshot.above, rows, "{state}");
        }
    }
}

/// The search the steps compose for `needle`, outward from the row `from`
/// rows above `start`, each match read for `rows` rows.
fn searched(
    outpost: &mut Outpost,
    start: TextAnchor,
    needle: &str,
    (from, rows): (i32, u8),
) -> Vec<TerminalMatch> {
    let mut batch = Builder::<Compared>::named();
    let steps = add_search(&mut batch, node(), (start, from), (needle, rows));
    let sent = batch.finish();
    let outcomes = outpost.run(sent.batch());
    let answers = Answers::parse(&sent, Reply::Steps(outcomes.clone()))
        .expect("outcomes of the steps' kinds");
    matches_of(&answers, &steps).unwrap_or_else(|| panic!("matches, not {outcomes:#?}"))
}

/// The old search and the search the steps compose, for a needle that
/// starts rows above and below the row searched from, one that starts
/// more rows than a search keeps, and one that starts none.
#[test]
fn the_steps_compose_the_old_search() {
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
