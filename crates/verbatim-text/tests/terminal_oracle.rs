//! The terminal reading rules (`verbatim_text::terminal_reading`, on the
//! row memory of `verbatim_text::terminal_rows`) checked by the simulated
//! terminal's oracle (`docs/design/terminal-model.md`, "Testing"). The reader under test is the real rules: every read is given to a
//! `TerminalReader` at the stream's times (a recorded stream's, or 10
//! milliseconds apart for one written by hand, the last read 10 seconds
//! after the last chunk), its speech reported as the oracle takes it (a
//! line, growth, a redrawn row's change, a flood's summary), its silences
//! stated with the oracle's reasons, and where each row went and its
//! review position taken from the memory. It is checked over the
//! conformance corpus in both terminals, the oracle's random streams, and
//! every stream of up to four operations of the tiny alphabet.
//!
//! No violation remains, and none is pinned: a failure is a consistent
//! explanation the oracle accepts, a listed limit through its own reason,
//! or a defect in the code.

use proptest::prelude::*;
use verbatim_term_sim::conformance::{Stream, history, streams};
use verbatim_term_sim::oracle::{
    Heard, Limit, Reader, Reason, Reply, Size, Spoken, TINY_ALPHABET, Violation, Withheld,
    every_stream, observe, random_config, random_steps, stream,
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
/// starts where the last said, a screen above that start, and the whole
/// document is read at once when the rules ask.
struct Core {
    reader: TerminalReader,
    query: Query,
    /// When each chunk came, for a recorded stream.
    at_ms: Vec<u64>,
    /// The reads answered so far.
    reads: usize,
    quiet: bool,
    /// Whether the last read showed no text, so the memory did not take
    /// it ("A read with no text").
    blank: bool,
}

impl Core {
    fn new(stream: &Stream) -> Self {
        Self {
            reader: TerminalReader::new(Settings::default()),
            query: Query::default(),
            at_ms: stream.at_ms.clone(),
            reads: 0,
            quiet: false,
            blank: false,
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
                rules::Reason::WrittenBack => Reason::WrittenBack,
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
        };
        self.answer(&read, snapshot.next_start)
    }

    fn quiet(&mut self) {
        self.quiet = true;
    }
}

impl Core {
    /// What the reader makes of `read`, whose snapshot said the next read
    /// starts at `next_start`.
    fn answer(&mut self, read: &Read, next_start: verbatim_term_sim::Range) -> Reply {
        let step = self.reader.read(read, false, None);
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
        let memory = self.reader.memory();
        // A read with no text leaves the memory as it was, so where its rows
        // went and where the review position is are said of the read before
        // it, which the oracle cannot judge: the reader makes no claim then,
        // nor at the read after it.
        let blank = step.outcome.as_ref().is_some_and(|outcome| outcome.blank);
        let mut heard = heard(&step, memory.review().map(|anchor| anchor.row));
        if blank || self.blank {
            heard.moved = None;
        }
        if blank {
            heard.review = None;
            heard.limits.push(Limit::BlankScreen);
        }
        self.blank = blank;
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
    assert_eq!(found, []);
}
