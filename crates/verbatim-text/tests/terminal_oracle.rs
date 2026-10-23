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
    Heard, Reader, Reason, Reply, Size, Spoken, TINY_ALPHABET, Violation, Withheld, every_stream,
    observe, random_config, random_steps, stream,
};
use verbatim_term_sim::{Flavor, Query, Refused, Snapshot};
use verbatim_text::terminal_reading::{self as rules, Settings, Speech, Step, TerminalReader};
use verbatim_text::terminal_rows::{OldRow, Read};

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
fn heard(step: &Step, review: Option<usize>, rows: usize) -> Heard {
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
    heard.review = review.filter(|&row| row < rows);
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
        let step = self.reader.read(
            &Read {
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
            },
            false,
            None,
        );
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
        if let Some(above) = step.search_above {
            return Reply::Again(Query {
                above: u32::try_from(above).unwrap_or(u32::MAX),
                ..self.query
            });
        }
        let memory = self.reader.memory();
        let review = memory
            .review()
            .and_then(|anchor| usize::try_from(anchor.row).ok());
        let heard = heard(&step, review, memory.rows().len());
        self.query = Query {
            start: Some(snapshot.next_start),
            screen_rows,
            whole: false,
            above: u32::try_from(memory.above()).unwrap_or(u32::MAX),
        };
        self.reads += 1;
        Reply::Heard(heard)
    }

    fn quiet(&mut self) {
        self.quiet = true;
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
    /// The history rotated between two reads, and the block of history rows
    /// above the kept start shows the smallest rotation consistent with the
    /// text (section 4.3, item 5), but output repeating earlier output
    /// rotated it further: the first rows of the document changed above
    /// the read, which no read covered, and their lines are missed. The
    /// document's first row, which every read takes, changed too. Should a
    /// changed first row refuse a block found in place?
    PeriodicHistory,
    /// The last read had no history, so the memory keeps no block, and the
    /// next read finds the buffer filled and rotated (section 4.3, item 5):
    /// with no block the rotation is taken from kept rows, and a row whose
    /// text a flood wrote again in its place is taken as kept or rewritten.
    /// A line holding its unfinished word is then carried by a line that
    /// replaced it, so the word is never spoken, or a row is held as
    /// volatile where the oracle finds no change at its place. Should a
    /// read whose document grew past a screen from one with no history be
    /// taken as the whole document's, searched for the old screen?
    RotationWithoutBlock,
    /// A row rewritten and then rotated into the history between two reads
    /// is the same row to the reader, which follows it by the rotation the
    /// block measured (section 4.3, item 6), and holds it as volatile when
    /// it changed before; the oracle's premise compares the lines at one
    /// place counted from the screen's top, which does not follow the
    /// rotation, and finds no change there. Should the oracle's places
    /// follow the history's rotation for rows that moved with it?
    VolatileAcrossRotation,
}

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
    assert_eq!(found, []);
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
    use Question::{PeriodicHistory, RotationWithoutBlock, VolatileAcrossRotation};
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
    let pinned: Vec<Pin> = vec![
        (
            126,
            VolatileAcrossRotation,
            vec![("FalseReason Volatile", 2)],
        ),
        (
            212,
            RotationWithoutBlock,
            vec![("FalseReason UnfinishedWord", 1)],
        ),
        (213, RotationWithoutBlock, vec![("FalseReason Volatile", 1)]),
        (508, PeriodicHistory, vec![("Missed", 1)]),
    ];
    let expected: Vec<(u32, Vec<(String, usize)>)> = pinned
        .iter()
        .map(|(case, _, kinds)| (*case, counts(kinds)))
        .collect();
    assert_eq!(found, expected);
}
