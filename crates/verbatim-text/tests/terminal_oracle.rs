//! The terminal reading rules (`verbatim_text::terminal_reading`, the
//! design's section 5, on the row memory of section 4) checked by the
//! simulated terminal's oracle (`docs/design/terminal-model.md`, section
//! 9). The reader under test is the real rules: every read is given to a
//! `TerminalReader` at the stream's times (a recorded stream's, or 10
//! milliseconds apart for one written by hand, the last read 10 seconds
//! after the last chunk), its speech reported as the oracle takes it (a
//! line, growth, a redrawn row's change, a flood's summary), its silences
//! stated with the oracle's reasons, and where each row went and its
//! review position taken from the memory. It is checked over the
//! conformance corpus in both terminals, the oracle's random streams, and
//! every stream of up to four operations of the tiny alphabet.
//!
//! No violation remains but in the cases pinned below, each with its
//! violations and the design question that decides it ([`Question`]),
//! which a change of the design, not of the rules, would settle.

use proptest::prelude::*;
use verbatim_term_sim::conformance::{Stream, history, streams};
use verbatim_term_sim::oracle::{
    Heard, Op, Reader, Reason, Reply, Size, Spoken, TINY_ALPHABET, Violation, Withheld,
    every_stream, observe, random_config, random_steps, stream,
};
use verbatim_term_sim::{Flavor, Query, Range, Refused, RowsRead, Snapshot};
use verbatim_text::terminal_reading::{self as rules, Settings, Speech, Step, TerminalReader};
use verbatim_text::terminal_rows::{Checked, OldRow, Read};

/// How long after the last chunk the stream's last read comes: past the
/// quiet period and the time a row lingers.
const QUIET_MS: u64 = 10_000;

/// The time between reads of a stream written by hand, which records none.
const HAND_STEP_MS: u64 = 10;

/// The reading rules under test, read as the outpost reads: each read
/// starts where the last said, as many rows above that start as the memory
/// asks, and the whole document, or a read from further above, is read at
/// once when the rules ask.
struct Core {
    reader: TerminalReader,
    query: Query,
    /// When each chunk came, for a recorded stream.
    at_ms: Vec<u64>,
    /// The reads answered so far.
    reads: usize,
    quiet: bool,
    /// The read waiting for the rows by their place it asked for, and
    /// where its snapshot said the next read starts.
    checking: Option<(Read, Range)>,
}

impl Core {
    fn new(stream: &Stream) -> Self {
        let reader = TerminalReader::new(Settings::default());
        let above = u32::try_from(reader.memory().above()).unwrap_or(u32::MAX);
        Self {
            reader,
            query: Query {
                above,
                ..Query::default()
            },
            at_ms: stream.at_ms.clone(),
            reads: 0,
            quiet: false,
            checking: None,
        }
    }

    /// The time of the read being answered.
    fn now(&self) -> u64 {
        let last = if self.at_ms.is_empty() {
            u64::try_from(self.reads).unwrap_or(u64::MAX) * HAND_STEP_MS
        } else {
            self.reads
                .checked_sub(1)
                .and_then(|chunk| self.at_ms.get(chunk).or(self.at_ms.last()))
                .copied()
                .unwrap_or(0)
        };
        if self.quiet { last + QUIET_MS } else { last }
    }
}

/// What the rules said, as the oracle takes reports: a flood's summary is
/// split into runs of rows that each start a line.
fn heard(step: &Step, review: Option<isize>) -> Heard {
    let mut heard = Heard::default();
    for speech in &step.speech {
        match speech {
            Speech::Line { row, text } => heard.spoken.push(Spoken::Line {
                row: *row,
                text: text.clone(),
            }),
            Speech::Growth { row, text } => heard.spoken.push(Spoken::Growth {
                row: *row,
                text: text.clone(),
            }),
            Speech::Changed { row, text } => heard.spoken.push(Spoken::Redrawn {
                row: *row,
                text: text.clone(),
            }),
            Speech::Held { text } => heard.spoken.push(Spoken::Held { text: text.clone() }),
            Speech::Skipped { rows, .. } => {
                let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
                for &row in rows {
                    match runs.last_mut() {
                        Some(run) if run.end == row => run.end += 1,
                        _ => runs.push(row..row + 1),
                    }
                }
                heard
                    .spoken
                    .extend(runs.into_iter().map(|rows| Spoken::Summary { rows }));
            }
            other => panic!("the oracle's streams press no keys, but the rules said {other:?}"),
        }
    }
    heard.withheld = step
        .withheld
        .iter()
        .map(|withheld| Withheld {
            row: withheld.row,
            reason: match withheld.reason {
                rules::Reason::Volatile => Reason::Volatile,
                rules::Reason::UnfinishedWord => Reason::UnfinishedWord,
                rules::Reason::HeldForKeys => Reason::HeldForKeys,
                rules::Reason::Spinner => Reason::Spinner,
                rules::Reason::LostText => Reason::LostText,
                rules::Reason::HeldTyping => Reason::HeldTyping,
                rules::Reason::KeyAnswer => Reason::KeyAnswer,
            },
        })
        .collect();
    if let Some(outcome) = step.outcome.as_ref().filter(|outcome| !outcome.first) {
        heard.moved = Some(
            outcome
                .old
                .iter()
                .map(|fate| match fate {
                    OldRow::Kept(to) | OldRow::Rewritten(to) => Some(*to),
                    OldRow::Gone { .. } | OldRow::Above | OldRow::Left => None,
                })
                .collect(),
        );
    }
    heard.review = review;
    heard
}

impl Reader for Core {
    fn query(&self) -> Query {
        self.query
    }

    fn read(&mut self, read: &Result<Snapshot, Refused>) -> Reply {
        let snapshot = read
            .as_ref()
            .expect("the simulated terminal answers every read");
        let read = Read {
            text: snapshot.text.clone(),
            screen: snapshot.screen.clone(),
            from_start: snapshot.from_start,
            shrank: snapshot.shrank,
            at_document_start: snapshot.at_document_start,
            alternate: snapshot.alternate,
            first_row: snapshot.first_row.clone(),
            caret_rows_to_end: snapshot.caret_rows_to_end,
            caret_line: snapshot.caret_line.clone(),
            caret_column: snapshot.caret_column,
            at_ms: self.now(),
            above: usize::try_from(snapshot.above).unwrap_or(usize::MAX),
            checked: None,
        };
        self.answer(read, snapshot.next_start)
    }

    fn rows(&mut self, rows: &Result<RowsRead, Refused>) -> Reply {
        let rows = rows
            .as_ref()
            .expect("the simulated terminal answers every read");
        let (mut read, next_start) = self
            .checking
            .take()
            .expect("rows by their place come after the read that asked for them");
        read.checked = Some(Checked {
            runs: rows.runs.clone(),
            first_row: rows.first_row.clone(),
        });
        self.answer(read, next_start)
    }

    fn quiet(&mut self) {
        self.quiet = true;
    }
}

impl Core {
    /// What the reader makes of `read`, whose snapshot said the next read
    /// starts at `next_start`.
    fn answer(&mut self, read: Read, next_start: Range) -> Reply {
        let step = self.reader.read(&read, false, None);
        let height = step.outcome.as_ref().map_or(0, |outcome| outcome.height);
        let screen_rows = u32::try_from(height).unwrap_or(u32::MAX);
        if step.whole_document {
            return Reply::Again(Query {
                start: None,
                screen_rows,
                whole: true,
                above: 0,
            });
        }
        if let Some(above) = step.read_above {
            return Reply::Again(Query {
                above: u32::try_from(above).unwrap_or(u32::MAX),
                ..self.query
            });
        }
        if !step.check_rows.is_empty() {
            let runs = step
                .check_rows
                .iter()
                .map(|run| {
                    (
                        u32::try_from(run.above).unwrap_or(u32::MAX),
                        u32::try_from(run.rows).unwrap_or(u32::MAX),
                    )
                })
                .collect();
            self.checking = Some((read, next_start));
            return Reply::Rows(runs);
        }
        let memory = self.reader.memory();
        let review = memory.review().map(|anchor| anchor.row);
        let heard = heard(&step, review);
        self.query = Query {
            start: Some(next_start),
            screen_rows,
            whole: false,
            above: u32::try_from(memory.above()).unwrap_or(u32::MAX),
        };
        self.reads += 1;
        Reply::Heard(heard)
    }
}

/// The violations of the core's reader over `stream` in a terminal of
/// `size`.
fn violations(stream: &Stream, flavor: Flavor, size: Size) -> Vec<Violation> {
    let mut terminal = size.terminal(flavor);
    observe(&mut terminal, stream, &mut Core::new(stream)).violations()
}

/// A corpus stream's violations in one terminal, counted by kind.
type StreamTally = (String, Flavor, Vec<(String, usize)>);

/// A violation's kind, with the reason for a false one.
fn kind(violation: &Violation) -> String {
    match violation {
        Violation::FalseReason { reason, .. } => format!("FalseReason {reason:?}"),
        other => format!("{other:?}")
            .split([' ', '{'])
            .next()
            .unwrap_or_default()
            .to_owned(),
    }
}

/// How many violations of each kind `violations` holds, by kind.
fn tally<'a>(violations: impl IntoIterator<Item = &'a Violation>) -> Vec<(String, usize)> {
    let mut counts = std::collections::BTreeMap::new();
    for violation in violations {
        *counts.entry(kind(violation)).or_insert(0) += 1;
    }
    counts.into_iter().collect()
}

fn counts(expected: &[(&str, usize)]) -> Vec<(String, usize)> {
    expected
        .iter()
        .map(|&(kind, count)| (kind.to_owned(), count))
        .collect()
}

/// A question about the design that decides a pinned case: the rules
/// cannot reach zero violations there without a change of the design or
/// of the oracle's specification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Question {
    /// The last read had no history, so the memory keeps no block, and the
    /// next read finds the buffer filled and rotated (section 4.3, item 5):
    /// with no block the rotation is taken from kept rows, and a row whose
    /// text a flood wrote again in its place is taken as kept or rewritten.
    /// A line holding its unfinished word is then carried by a line that
    /// replaced it, so the word is never spoken. Should a
    /// read whose document grew past a screen from one with no history be
    /// taken as the whole document's, searched for the old screen?
    RotationWithoutBlock,
    /// The alignment pairs a volatile row alone with a new row elsewhere
    /// that is alike to it (section 4.3, item 4), as Pi's input row
    /// "/model" with its model selector's "Model catalogs refreshed." seven
    /// rows away, and carries the row's changes to it, so the new row is
    /// held as volatile and never spoken once the selector erases it. A
    /// row's change history moves only with a row kept in place or moved
    /// with its neighbours as part of a block (section 4.3, item 6, decided
    /// by Dickson, 2026-10-24), and the oracle follows a row only so, so
    /// this is the core's to fix.
    VolatileMovedByRewrite,
    /// A row that vanished lingers and is taken back silently when its
    /// text comes back (section 4.1), but the memory takes back rows, not
    /// lines: a row is taken back when its text returns as a row of
    /// another line, the end of a wrapped line written again as a line of
    /// its own, or a line's first row written again with more text after
    /// it. The line it is now in was never shown, so it is missed, or only
    /// its change from the lingering row is spoken. Should a lingering row
    /// be taken back only when it comes back as the line it was in?
    LingeringInAnotherLine,
    /// The screen erased into the history as the full history rotates,
    /// with the same text on more than one row (section 4.4): the rows are
    /// paired with the wrong copies of their text, and a line that stayed
    /// on the screen is spoken again. Should a clear be aligned by the
    /// rows' order as well as their text?
    ClearedOverRepeatedText,
}

/// A stream pinned as the smallest that shows a question: the question,
/// the terminal, the operations and read points, on the random streams'
/// terminal, and its violations counted by kind.
type SmallestPin = (
    Question,
    Flavor,
    Vec<(Op, bool)>,
    Vec<(&'static str, usize)>,
);

/// A corpus stream pinned in one terminal: its name, the question that
/// decides it, and its violations counted by kind.
type CorpusPin = (&'static str, Flavor, Question, Vec<(&'static str, usize)>);

/// A random case pinned: its number, the question that decides it, and
/// its violations counted by kind.
type Pin = (u32, Question, Vec<(&'static str, usize)>);

#[test]
fn the_cores_violations_over_the_corpus() {
    let corpus = streams();
    assert!(!corpus.is_empty(), "the corpus has no streams");
    let found: Vec<StreamTally> = corpus
        .iter()
        .flat_map(|(name, stream)| {
            [Flavor::ConsoleHost, Flavor::WindowsTerminal].map(|flavor| {
                let size = Size {
                    width: 120,
                    height: 30,
                    history: history(flavor),
                };
                (
                    name.clone(),
                    flavor,
                    tally(&violations(stream, flavor, size)),
                )
            })
        })
        .filter(|(_, _, tally)| !tally.is_empty())
        .collect();
    let pinned: Vec<CorpusPin> = vec![(
        "pi-regular",
        Flavor::ConsoleHost,
        Question::VolatileMovedByRewrite,
        vec![("FalseReason Volatile", 1)],
    )];
    let expected: Vec<StreamTally> = pinned
        .iter()
        .map(|(name, flavor, _, kinds)| ((*name).to_owned(), *flavor, counts(kinds)))
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn the_cores_violations_over_every_stream_of_four_tiny_operations() {
    let streams = every_stream(&TINY_ALPHABET, 4);
    assert_eq!(streams.len(), 9 + 81 + 729 + 6561);
    let found: Vec<(String, Flavor, Vec<Violation>)> = streams
        .iter()
        .flat_map(|stream| {
            [Flavor::ConsoleHost, Flavor::WindowsTerminal].map(|flavor| {
                (
                    stream.about.clone(),
                    flavor,
                    violations(stream, flavor, Size::TINY),
                )
            })
        })
        .filter(|(_, _, violations)| !violations.is_empty())
        .collect();
    assert_eq!(found, []);
}

#[test]
fn the_cores_violations_over_random_streams() {
    // The random streams of the oracle's own tests, taken in the same
    // order from the same seed, every case run rather than stopping at the
    // first failure, each case numbered in that order.
    use Question::{
        ClearedOverRepeatedText, LingeringInAnotherLine, RotationWithoutBlock,
        VolatileMovedByRewrite,
    };
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let config = random_config();
    let cases = config.cases;
    let mut runner = TestRunner::new(config);
    let strategy = (random_steps(), any::<bool>());
    let mut found = Vec::new();
    for case in 0..cases {
        let (steps, windows_terminal) = strategy
            .new_tree(&mut runner)
            .expect("the random streams' strategy makes a stream")
            .current();
        let flavor = if windows_terminal {
            Flavor::WindowsTerminal
        } else {
            Flavor::ConsoleHost
        };
        let violations = violations(&stream(&steps), flavor, Size::SMALL);
        if !violations.is_empty() {
            found.push((case, tally(&violations)));
        }
    }
    let missed = || vec![("Missed", 1)];
    let pinned: Vec<Pin> = vec![
        (36, RotationWithoutBlock, missed()),
        (39, LingeringInAnotherLine, missed()),
        (54, LingeringInAnotherLine, missed()),
        (57, LingeringInAnotherLine, missed()),
        (
            69,
            VolatileMovedByRewrite,
            vec![("FalseReason Volatile", 1)],
        ),
        (
            101,
            LingeringInAnotherLine,
            vec![("FalseReason LostText", 1), ("Missed", 1)],
        ),
        (183, LingeringInAnotherLine, missed()),
        (
            221,
            RotationWithoutBlock,
            vec![("FalseReason UnfinishedWord", 1)],
        ),
        (246, ClearedOverRepeatedText, vec![("Repeated", 1)]),
        (289, LingeringInAnotherLine, missed()),
        (318, LingeringInAnotherLine, missed()),
        (333, LingeringInAnotherLine, missed()),
        (377, RotationWithoutBlock, vec![("FalseReason Volatile", 1)]),
        (440, RotationWithoutBlock, vec![("FalseReason Volatile", 1)]),
        (469, LingeringInAnotherLine, missed()),
        (510, LingeringInAnotherLine, missed()),
    ];
    let expected: Vec<(u32, Vec<(String, usize)>)> = pinned
        .iter()
        .map(|(case, _, kinds)| (*case, counts(kinds)))
        .collect();
    assert_eq!(found, expected);
}

/// The smallest streams that show each question, as the random streams'
/// shrinking left them.
#[expect(
    clippy::too_many_lines,
    reason = "a list of streams, each written out as operations"
)]
fn smallest_pins() -> Vec<SmallestPin> {
    use Question::{
        ClearedOverRepeatedText, LingeringInAnotherLine, RotationWithoutBlock,
        VolatileMovedByRewrite,
    };
    let word = "wrapping-word";
    vec![
        // A line holding its unfinished word, a flood filling the buffer
        // from no history.
        (
            RotationWithoutBlock,
            Flavor::ConsoleHost,
            vec![
                (Op::Write(word), false),
                (Op::Write("ok"), true),
                (Op::Write("dog"), true),
                (Op::Flood { lines: 10, word }, false),
            ],
            vec![("FalseReason UnfinishedWord", 1)],
        ),
        // A header's row changed twice, a flood filling the buffer from no
        // history.
        (
            RotationWithoutBlock,
            Flavor::ConsoleHost,
            vec![
                (Op::Header("\u{2819} 1s"), true),
                (Op::Write("a b"), false),
                (Op::MoveRows(-3), true),
                (
                    Op::Flood {
                        lines: 14,
                        word: "a b",
                    },
                    false,
                ),
            ],
            vec![("FalseReason Volatile", 1)],
        ),
        // A status row's timer ticking twice, then the status row drawn a
        // row lower as the output above it grows.
        (
            VolatileMovedByRewrite,
            Flavor::ConsoleHost,
            vec![
                (Op::Write("1s"), true),
                (Op::Rewrite("2s"), true),
                (Op::Rewrite("3s"), true),
                (Op::Rewrite("ok"), false),
                (Op::Newline, false),
                (Op::Write("4s"), true),
            ],
            vec![("FalseReason Volatile", 1)],
        ),
        // A wrapped line's second row, erased, written again as a line of
        // its own.
        (
            LingeringInAnotherLine,
            Flavor::WindowsTerminal,
            vec![
                (Op::Write("cat"), false),
                (Op::MoveColumns(6), false),
                (Op::Write("a b"), false),
                (Op::Write("a b"), false),
                (Op::MoveColumns(3), true),
                (Op::EraseLines(3), false),
                (Op::Newline, true),
                (Op::Write("a b"), false),
            ],
            vec![("Missed", 1)],
        ),
        // A line's first row, deleted, written again with the rest of its
        // word after it.
        (
            LingeringInAnotherLine,
            Flavor::WindowsTerminal,
            vec![
                (Op::Write(word), false),
                (Op::DeleteRows(1), true),
                (Op::InsertAboveFooter, false),
                (Op::Bytes(b"\x93"), true),
                (
                    Op::Restyle {
                        rows: 2,
                        text: word,
                    },
                    false,
                ),
            ],
            vec![("Missed", 1)],
        ),
        (
            ClearedOverRepeatedText,
            Flavor::WindowsTerminal,
            vec![
                (Op::Redraw("hello"), false),
                (Op::Newline, false),
                (
                    Op::WordDown {
                        column: 9,
                        word: "cat",
                    },
                    false,
                ),
                (Op::Rewrite("hello"), false),
                (Op::LeaveAlternate, true),
                (Op::Clear, false),
                (Op::Write("a b"), false),
                (Op::Repaint("cat"), false),
                (Op::Newline, false),
            ],
            vec![("Repeated", 1)],
        ),
    ]
}

#[test]
fn the_cores_violations_over_the_smallest_streams_of_each_question() {
    let pinned = smallest_pins();
    let found: Vec<Vec<(String, usize)>> = pinned
        .iter()
        .map(|(_, flavor, steps, _)| tally(&violations(&stream(steps), *flavor, Size::SMALL)))
        .collect();
    let expected: Vec<Vec<(String, usize)>> = pinned
        .iter()
        .map(|(_, _, _, kinds)| counts(kinds))
        .collect();
    assert_eq!(found, expected);
}
