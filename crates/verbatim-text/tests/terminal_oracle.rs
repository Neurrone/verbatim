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
/// starts where the last said, and the whole document is read when the
/// rules ask.
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
        Self {
            reader: TerminalReader::new(Settings::default()),
            query: Query::default(),
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
                    OldRow::Gone { .. } | OldRow::Above => None,
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
    /// The alternate screen opened while the main screen had no history,
    /// so no read shows the switch (section 5.8): the reader takes the
    /// alternate screen for the main one, and when the main screen comes
    /// back holding text the alternate screen showed, as Pi's fullscreen
    /// mode prints its last frame on leaving, it keeps those rows silent.
    /// The oracle counts as shown only what reads on the main screen
    /// showed. Does text a read showed on the alternate screen count as
    /// shown, since a reader cannot tell which screen it read?
    AlternateUnseen,
    /// The rotation is measured from kept rows that stayed or moved up
    /// (section 4.3, items 3 and 5). When the last screen was blank, when
    /// its only kept rows moved down with the end of the text (a footer, a
    /// redrawn bottom region), or when its rows repeat text found elsewhere
    /// (the first rows of similar wrapped lines, a frame drawn twice), the
    /// alignment measures no rotation or a wrong one: lines rotated above
    /// the read's start are missed, or rows kept in place are taken for
    /// rewritten, so the rules hold them as volatile or as having lost
    /// text, or speak them again. Should the reader read the whole document
    /// in these cases, and how would it then tell history it never read
    /// from new output?
    RotationUnmeasured,
    /// Erasing the history looks like the alternate screen opening, and
    /// the next scroll into a history like it closing (section 5.8), so the
    /// memory kept aside at the erase is restored and lines read since are
    /// spoken again. Should the restore be refused when rows the memory
    /// read since are found in the read?
    HistoryEraseAsAlternate,
    /// A growing line's unfinished word is held until the line grows past
    /// it, the cursor leaves it, or the quiet period passes (section 5.3).
    /// When output pushes the line out of a full history between two reads
    /// before any of these, no later read shows it, so the word is never
    /// spoken, and the oracle, which does not count a row rotated out of
    /// the buffer as resolving a hold, reports the hold as false. The rules
    /// speak a held line once it is above the screen, so this needs more
    /// than a screen of output past a full history in one read: the tiny
    /// terminal's history of one row, or a flood. Should a hold be
    /// accounted for when its line leaves a full history before the quiet
    /// period, as lines that leave between two reads unseen are, or should
    /// the word not be held then?
    HeldPastHistory,
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
    // Question::AlternateUnseen: Pi's fullscreen mode opens the alternate
    // screen on a main screen with no history and prints its last frame on
    // the main screen as it leaves. In the console host two of its lines
    // are withheld as having lost text against the lines the alternate
    // screen showed, which the oracle does not count.
    // Question::RotationUnmeasured: the footer kept by a scroll region
    // over a full history is the last screen's only row kept in place, so
    // the rotation is not measured: the lines rotated above the read are
    // missed, and the region's rows kept in place are taken for rows
    // rewritten twice and held as volatile.
    let expected: Vec<StreamTally> = [
        (
            "pi-fullscreen",
            Flavor::ConsoleHost,
            &[("FalseReason LostText", 2), ("Missed", 17)][..],
        ),
        ("pi-fullscreen", Flavor::WindowsTerminal, &[("Missed", 19)]),
        (
            "rotation-footer",
            Flavor::ConsoleHost,
            &[("FalseReason Volatile", 29), ("Missed", 74)],
        ),
        (
            "rotation-footer",
            Flavor::WindowsTerminal,
            &[("FalseReason Volatile", 29), ("Missed", 74)],
        ),
    ]
    .into_iter()
    .map(|(name, flavor, kinds)| (name.to_owned(), flavor, counts(kinds)))
    .collect();
    assert_eq!(found, expected);
}

/// How many tiny streams, counted once in each terminal, push a held word
/// out of the tiny history (`Question::HeldPastHistory`).
const HELD_TINY_STREAMS: usize = 146;

/// How many words those streams hold that are never resolved.
const HELD_TINY_WORDS: usize = 146;

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
    // Question::AlternateUnseen: a line written on the alternate screen,
    // which opened with no history, and written again on the main screen.
    let unseen = |word: &str| {
        let about = format!(
            "[(EnterAlternate, true), (Write({word:?}), true), (LeaveAlternate, true), \
             (Write({word:?}), true)]"
        );
        let missed = vec![Violation::Missed {
            read: 4,
            coordinate: 0,
            text: word.to_owned(),
        }];
        [Flavor::ConsoleHost, Flavor::WindowsTerminal]
            .map(|flavor| (about.clone(), flavor, missed.clone()))
    };
    let expected_unseen: Vec<(String, Flavor, Vec<Violation>)> =
        unseen("ab").into_iter().chain(unseen("cdefg")).collect();
    let (found_unseen, held): (Vec<_>, Vec<_>) = found
        .into_iter()
        .partition(|case| expected_unseen.contains(case));
    assert_eq!(found_unseen, expected_unseen);
    // Question::HeldPastHistory: the tiny terminal's history is one row,
    // so a clear, or a line growing past the buffer, pushes a line holding
    // its unfinished word out of it between two reads. Every other stream
    // with violations holds such a word; in four, the line grows on past
    // the buffer and its rest is missed, since what was heard of it never
    // covered its start.
    let holding = held
        .iter()
        .filter(|(_, _, violations)| {
            violations.iter().any(|violation| {
                matches!(
                    violation,
                    Violation::FalseReason {
                        reason: Reason::UnfinishedWord,
                        ..
                    }
                )
            })
        })
        .count();
    assert_eq!(
        (held.len(), holding),
        (HELD_TINY_STREAMS, HELD_TINY_STREAMS)
    );
    assert_eq!(
        tally(held.iter().flat_map(|(_, _, violations)| violations)),
        counts(&[
            ("FalseReason UnfinishedWord", HELD_TINY_WORDS),
            ("Missed", 4)
        ])
    );
}

#[test]
fn the_cores_violations_over_random_streams() {
    // The random streams of the oracle's own tests, taken in the same
    // order from the same seed, every case run rather than stopping at the
    // first failure, each case numbered in that order.
    use Question::{AlternateUnseen, HeldPastHistory, HistoryEraseAsAlternate, RotationUnmeasured};
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
    let missed = |count| vec![("Missed", count)];
    let repeated = |count| vec![("Repeated", count)];
    let both = || vec![("Missed", 1), ("Repeated", 1)];
    let held = || vec![("FalseReason UnfinishedWord", 1)];
    // Case 446 misses a line the alternate screen showed, so the growth
    // that follows it never covers its start and the word it holds is
    // never resolved; it is listed under the alternate screen.
    let pinned: Vec<Pin> = vec![
        (61, RotationUnmeasured, missed(1)),
        (65, RotationUnmeasured, missed(2)),
        (
            70,
            RotationUnmeasured,
            vec![("FalseReason LostText", 1), ("Missed", 5)],
        ),
        (71, RotationUnmeasured, both()),
        (82, AlternateUnseen, missed(1)),
        (
            126,
            RotationUnmeasured,
            vec![("FalseReason Volatile", 2), ("Missed", 3)],
        ),
        (168, RotationUnmeasured, missed(2)),
        (196, RotationUnmeasured, missed(1)),
        (212, HeldPastHistory, held()),
        (213, RotationUnmeasured, missed(1)),
        (290, RotationUnmeasured, missed(1)),
        (298, HeldPastHistory, held()),
        (318, RotationUnmeasured, missed(2)),
        (341, AlternateUnseen, missed(1)),
        (349, AlternateUnseen, missed(1)),
        (350, RotationUnmeasured, missed(1)),
        (364, RotationUnmeasured, repeated(1)),
        (379, HeldPastHistory, held()),
        (397, AlternateUnseen, missed(1)),
        (409, AlternateUnseen, missed(1)),
        (415, RotationUnmeasured, missed(5)),
        (445, HeldPastHistory, held()),
        (
            446,
            AlternateUnseen,
            vec![("FalseReason UnfinishedWord", 1), ("Missed", 1)],
        ),
        (458, RotationUnmeasured, missed(6)),
        (498, RotationUnmeasured, repeated(2)),
        (502, HeldPastHistory, held()),
        (506, HeldPastHistory, held()),
        (508, RotationUnmeasured, missed(1)),
        (510, HistoryEraseAsAlternate, repeated(3)),
    ];
    let expected: Vec<(u32, Vec<(String, usize)>)> = pinned
        .iter()
        .map(|(case, _, kinds)| (*case, counts(kinds)))
        .collect();
    assert_eq!(found, expected);
}
