//! The terminal memory's alignment checked by the simulated terminal's
//! oracle (`docs/design/terminal-model.md`, section 9): a reader built on
//! `TerminalMemory` reports every new row's line as a line, a line with a
//! rewritten row as a redrawn change, a line that only gained at its end
//! as growth, and withholds kept rows that moved, lingering rows taken
//! back elsewhere and reprints for those reasons, with where each row went
//! and its review position, over the conformance corpus in both
//! terminals, the random streams, and every stream of up to four
//! operations of the tiny alphabet.
//!
//! Each test asserts the exact violations that remain, counted by kind.
//! They are not yet zero: each remaining one is a case where the oracle
//! judges by the simulator's row identities what a reader of text cannot
//! see, awaiting Dickson's decision on whether the oracle or the design
//! changes:
//!
//! - `FalseReason Moved` and `FalseReason Lingering`: a row moved or
//!   returned with its identity (rows inserted or deleted, a scroll region
//!   that does not start at the screen's top, a footer below a region that
//!   does) is withheld as moved or lingering, as section 5.3 makes every
//!   kept row silent wherever it moved, but the oracle checks the reason
//!   on a line it does not count as shown for the first time and finds the
//!   text was on the same row. From text, such a row cannot be told from
//!   one a program drew again elsewhere, which must be withheld.
//! - `ReviewMoved`: section 5.6 carries the review position with its text
//!   when a kept row moves, as when Ink draws its frame a row lower or Pi
//!   repaints its transcript higher; the oracle keeps it on the same row
//!   identity.
//! - `Repeated`: a row erased and drawn again in place with the same text
//!   after it stopped lingering (a list closed and opened again, hint rows
//!   back after a list closes) is new by section 4.1; the oracle never
//!   allows a text to be said again on the same row identity.
//! - `Mismapped`: the alignment keeps the most rows (section 4.3, item 3),
//!   which can prefer rows moved over fewer rows that stayed in place, as
//!   when a list is redrawn around two entries that did not move, or a
//!   wrapped line is written again from its second row.
//! - `Missed` and `Incomplete`: a line whose first row is above the read
//!   (a wrapped line longer than the screen, its start rotated above the
//!   read's start) can only be reported from rows the read holds; growth
//!   is credited by the oracle to the row that started the line, which a
//!   rotation from the document's start replaces; the alternate screen in
//!   a terminal with no history cannot be seen opening, so text it showed
//!   and the main screen shows after is kept; and a row destroyed and
//!   drawn again with the same text in the same place is unchanged to a
//!   read.
//! - `FalseReason Reprint`: rows found again in the known history are
//!   matched row by row, the oracle's premise line by line.

use proptest::prelude::*;
use verbatim_term_sim::conformance::{Stream, history, streams};
use verbatim_term_sim::oracle::{
    Heard, Reader, Reason, Reply, Size, Spoken, TINY_ALPHABET, Violation, Withheld, every_stream,
    observe, random_config, random_steps, stream,
};
use verbatim_term_sim::{Flavor, Query, Refused, Snapshot};
use verbatim_text::terminal_rows::{NewRow, OldRow, Outcome, Read, TerminalMemory};

/// How long after the last chunk the stream's last read comes: past the
/// quiet period and the time a row lingers.
const QUIET_MS: u64 = 10_000;

/// The time between reads of a stream written by hand, which records none.
const HAND_STEP_MS: u64 = 10;

/// A reader on the terminal memory.
struct Core {
    memory: TerminalMemory,
    query: Query,
    /// For each row of the last read starting a line, what was reported
    /// of that line.
    said: Vec<Option<String>>,
    /// When each chunk came, for a recorded stream.
    at_ms: Vec<u64>,
    /// The reads answered so far.
    reads: usize,
    /// The texts of the rows of the read being answered.
    texts: Vec<String>,
    quiet: bool,
}

impl Core {
    fn new(stream: &Stream) -> Self {
        Self {
            memory: TerminalMemory::new(),
            query: Query::default(),
            said: Vec::new(),
            at_ms: stream.at_ms.clone(),
            reads: 0,
            texts: Vec::new(),
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

    /// What the reader says of a read the memory aligned.
    fn heard(&mut self, outcome: &Outcome) -> Heard {
        let mut heard = Heard::default();
        let mut said = vec![None; outcome.rows.len()];
        for line in &outcome.lines {
            let start = line.rows.start;
            let kinds = &outcome.rows[line.rows.clone()];
            let from = match kinds[0] {
                NewRow::Kept { from, .. } | NewRow::Rewritten { from, .. } => Some(from),
                _ => None,
            };
            let before = from.and_then(|from| self.said.get(from).cloned().flatten());
            if line.text.is_empty() {
                continue;
            }
            // A blank row wrapped onto a kept row adds nothing to its line.
            let changed = kinds
                .iter()
                .zip(&self.texts[line.rows.clone()])
                .any(|(kind, text)| {
                    !text.trim_end().is_empty()
                        && matches!(kind, NewRow::New | NewRow::Rewritten { .. })
                });
            if changed {
                let text = line.text.clone();
                heard.spoken.push(match (line.grew_from, &before) {
                    (Some(was), Some(before)) if text[..was] == *before => Spoken::Growth {
                        row: start,
                        text: text[was..].to_owned(),
                    },
                    _ if from.is_some() => Spoken::Redrawn {
                        row: start,
                        text: text.clone(),
                    },
                    _ => Spoken::Line {
                        row: start,
                        text: text.clone(),
                    },
                });
                said[start] = Some(text);
                continue;
            }
            said[start] = before;
            // A line is withheld for a reason its rows with text all share;
            // one whose rows were kept, returned or reprinted differently is
            // a line the rows of no earlier read made, and is redrawn.
            let shown: Vec<NewRow> = kinds
                .iter()
                .zip(&self.texts[line.rows.clone()])
                .filter(|(_, text)| !text.trim_end().is_empty())
                .map(|(kind, _)| *kind)
                .collect();
            let all = |test: fn(&NewRow) -> bool| shown.iter().all(test);
            let reason = if all(|kind| *kind == NewRow::Reprint) {
                Some(Reason::Reprint)
            } else if all(|kind| *kind == NewRow::Returned { moved: true }) {
                Some(Reason::Lingering)
            } else if all(|kind| matches!(kind, NewRow::Kept { moved: true, .. })) {
                Some(Reason::Moved)
            } else if all(|kind| {
                matches!(
                    kind,
                    NewRow::Kept { moved: false, .. }
                        | NewRow::Returned { moved: false }
                        | NewRow::Earlier
                )
            }) {
                None
            } else {
                heard.spoken.push(Spoken::Redrawn {
                    row: start,
                    text: line.text.clone(),
                });
                said[start] = Some(line.text.clone());
                None
            };
            if let Some(reason) = reason {
                heard.withheld.push(Withheld { row: start, reason });
            }
        }
        self.said = said;
        if !outcome.first {
            heard.moved = Some(
                outcome
                    .old
                    .iter()
                    .map(|fate| match fate {
                        OldRow::Kept(to) | OldRow::Rewritten(to) => Some(*to),
                        OldRow::Gone { .. } | OldRow::Above => None,
                    })
                    .collect(),
            );
        }
        heard.review = self
            .memory
            .review()
            .and_then(|anchor| usize::try_from(anchor.row).ok())
            .filter(|&row| row < outcome.rows.len());
        heard
    }
}

impl Reader for Core {
    fn query(&self) -> Query {
        self.query
    }

    fn read(&mut self, read: &Result<Snapshot, Refused>) -> Reply {
        let snapshot = read
            .as_ref()
            .expect("the simulated terminal answers every read");
        let outcome = self.memory.read(&Read {
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
        });
        let screen_rows = u32::try_from(outcome.height).unwrap_or(u32::MAX);
        if outcome.whole_document {
            return Reply::Again(Query {
                start: None,
                screen_rows,
                whole: true,
            });
        }
        if outcome.first {
            self.memory
                .set_review(outcome.caret.unwrap_or(0), snapshot.caret_column);
        }
        self.texts = self
            .memory
            .rows()
            .iter()
            .map(|row| row.text.clone())
            .collect();
        let heard = self.heard(&outcome);
        self.query = Query {
            start: Some(snapshot.next_start),
            screen_rows,
            whole: false,
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
    let console = Flavor::ConsoleHost;
    let windows = Flavor::WindowsTerminal;
    let expected: Vec<StreamTally> = [
        (
            "claude-code",
            console,
            &[("Mismapped", 2), ("Repeated", 3)][..],
        ),
        ("claude-code", windows, &[("Mismapped", 2), ("Repeated", 3)]),
        ("footer", console, &[("FalseReason Moved", 2)]),
        ("footer", windows, &[("FalseReason Moved", 2)]),
        ("ink", console, &[("ReviewMoved", 1)]),
        ("ink", windows, &[("ReviewMoved", 1)]),
        ("pending-wrap", console, &[("FalseReason Moved", 1)]),
        ("pending-wrap", windows, &[("FalseReason Moved", 1)]),
        (
            "pi-fullscreen",
            console,
            &[("Missed", 19), ("ReviewMoved", 1)],
        ),
        (
            "pi-fullscreen",
            windows,
            &[("Missed", 19), ("ReviewMoved", 1)],
        ),
        (
            "pi-regular",
            console,
            &[("FalseReason Moved", 3), ("Repeated", 11)],
        ),
        ("pi-regular", windows, &[("Repeated", 10)]),
        ("rows-edit", console, &[("FalseReason Moved", 6)]),
        ("rows-edit", windows, &[("FalseReason Moved", 6)]),
    ]
    .into_iter()
    .map(|(name, flavor, kinds)| (name.to_owned(), flavor, counts(kinds)))
    .collect();
    assert_eq!(found, expected);
}

#[test]
fn the_cores_violations_over_every_stream_of_four_tiny_operations() {
    let streams = every_stream(&TINY_ALPHABET, 4);
    assert_eq!(streams.len(), 9 + 81 + 729 + 6561);
    let found: Vec<Violation> = streams
        .iter()
        .flat_map(|stream| {
            [Flavor::ConsoleHost, Flavor::WindowsTerminal]
                .into_iter()
                .flat_map(|flavor| violations(stream, flavor, Size::TINY))
        })
        .collect();
    assert_eq!(
        tally(&found),
        counts(&[
            ("FalseReason Moved", 3),
            ("Incomplete", 2),
            ("Missed", 10),
            ("Repeated", 1),
            ("ReviewMoved", 48),
        ])
    );
}

#[test]
fn the_cores_violations_over_random_streams() {
    // The random streams of the oracle's own tests, taken in the same
    // order from the same seed, every case run rather than stopping at the
    // first failure.
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    let config = random_config();
    let cases = config.cases;
    let mut runner = TestRunner::new(config);
    let strategy = (random_steps(), any::<bool>());
    let mut found = Vec::new();
    for _ in 0..cases {
        let (steps, windows_terminal) = strategy
            .new_tree(&mut runner)
            .expect("the random streams' strategy makes a stream")
            .current();
        let flavor = if windows_terminal {
            Flavor::WindowsTerminal
        } else {
            Flavor::ConsoleHost
        };
        found.extend(violations(&stream(&steps), flavor, Size::SMALL));
    }
    assert_eq!(
        tally(&found),
        counts(&[
            ("FalseReason Lingering", 12),
            ("FalseReason Moved", 76),
            ("FalseReason Reprint", 2),
            ("Incomplete", 1),
            ("Mismapped", 20),
            ("Missed", 95),
            ("Repeated", 22),
            ("ReviewMoved", 72),
        ])
    );
}
