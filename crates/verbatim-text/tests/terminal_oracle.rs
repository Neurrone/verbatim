//! The terminal memory's alignment checked by the simulated terminal's
//! oracle (`docs/design/terminal-model.md`, section 9). A reader built on
//! `TerminalMemory` speaks a line with a new row as a line, a line that
//! only gained at its end as its growth, and a line with a rewritten row
//! as a redrawn change; it is silent on a line whose rows were all kept,
//! taken back while lingering, reprinted or found in the history, since a
//! read showed their text before, but withholds such a line as one that
//! only lost text when the line its first row was in had more, and speaks
//! it as a redrawn change when its rows were joined into a line no read
//! held. It reports where each row went and its review position. It is
//! checked over the conformance corpus in both terminals, the oracle's
//! random streams, and every stream of up to four operations of the tiny
//! alphabet.
//!
//! No violation remains but in the cases pinned below, each with its
//! violations and the design question that decides it ([`Question`]),
//! which a change of the design, not of the reader, would settle.

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
    /// For each row of the last read, the text of its line.
    lines: Vec<String>,
    /// The texts of the rows of the last read.
    old_texts: Vec<String>,
    /// For each lingering row's text, the text of its line when it went.
    gone: std::collections::HashMap<String, String>,
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
            lines: Vec::new(),
            old_texts: Vec::new(),
            gone: std::collections::HashMap::new(),
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
    #[expect(
        clippy::too_many_lines,
        reason = "each kind of line in turn: changed, then silent, then the mapping and review"
    )]
    fn heard(&mut self, outcome: &Outcome) -> Heard {
        let mut heard = Heard::default();
        for (from, fate) in outcome.old.iter().enumerate() {
            if *fate == (OldRow::Gone { lingering: true })
                && let (Some(text), Some(line)) = (self.old_texts.get(from), self.lines.get(from))
            {
                self.gone.insert(text.clone(), line.clone());
            }
        }
        let mut said = vec![None; outcome.rows.len()];
        for line in &outcome.lines {
            let start = line.rows.start;
            let kinds = &outcome.rows[line.rows.clone()];
            // The line's first row with text says where it came from.
            let first = kinds
                .iter()
                .zip(&self.texts[line.rows.clone()])
                .find(|(_, text)| !text.trim_end().is_empty())
                .map_or(kinds[0], |(kind, _)| *kind);
            let from = match first {
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
                // A line continued from above the read grows from its part
                // in the last read, which was said with the line it began.
                let above = start == 0 && outcome.continued;
                heard.spoken.push(match (line.grew_from, &before) {
                    (Some(was), Some(before)) if text[..was] == *before => Spoken::Growth {
                        row: start,
                        text: text[was..].to_owned(),
                    },
                    (Some(was), _) if above => Spoken::Growth {
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
            // A line none of whose rows with text is new or rewritten is
            // silent: its rows were kept, returned, reprinted or are
            // history, so a read showed their text before. When the line
            // its first kept row was in had other text, the line only lost
            // text, as when a wrapped line's last row is erased or a line
            // break splits it, or rows were joined into a line no read
            // showed, which is spoken as a redrawn change.
            let was = kinds
                .iter()
                .zip(&self.texts[line.rows.clone()])
                .find(|(_, text)| !text.trim_end().is_empty())
                .and_then(|(kind, text)| match kind {
                    NewRow::Kept { from, .. } => self.lines.get(*from),
                    NewRow::Returned { .. } => self.gone.get(text),
                    _ => None,
                });
            let Some(was) = was else {
                continue;
            };
            if *was == line.text {
                continue;
            }
            let squeezed =
                |text: &str| -> String { text.chars().filter(|c| !c.is_whitespace()).collect() };
            let (was, now) = (squeezed(was), squeezed(&line.text));
            if was.len() > now.len() && was.contains(&now) {
                heard.withheld.push(Withheld {
                    row: start,
                    reason: Reason::LostText,
                });
            } else {
                heard.spoken.push(Spoken::Redrawn {
                    row: start,
                    text: line.text.clone(),
                });
                said[start] = Some(line.text.clone());
            }
        }
        self.said = said;
        self.lines = vec![String::new(); outcome.rows.len()];
        for line in &outcome.lines {
            for row in line.rows.clone() {
                self.lines[row].clone_from(&line.text);
            }
        }
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
        self.old_texts = std::mem::take(&mut self.texts);
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

/// A question about the design that decides a pinned case: the reader
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
    /// new and spoken again. Should the reader read the whole document in
    /// these cases, and how would it then tell history it never read from
    /// new output?
    RotationUnmeasured,
    /// Erasing the history looks like the alternate screen opening, and
    /// the next scroll into a history like it closing (section 5.8), so the
    /// memory kept aside at the erase is restored and lines read since are
    /// spoken again. Should the restore be refused when rows the memory
    /// read since are found in the read?
    HistoryEraseAsAlternate,
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
    let expected: Vec<StreamTally> = [
        (
            "pi-fullscreen",
            Flavor::ConsoleHost,
            &[("FalseReason LostText", 2), ("Missed", 17)][..],
        ),
        ("pi-fullscreen", Flavor::WindowsTerminal, &[("Missed", 19)]),
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
    let expected: Vec<(String, Flavor, Vec<Violation>)> =
        unseen("ab").into_iter().chain(unseen("cdefg")).collect();
    assert_eq!(found, expected);
}

#[test]
fn the_cores_violations_over_random_streams() {
    // The random streams of the oracle's own tests, taken in the same
    // order from the same seed, every case run rather than stopping at the
    // first failure, each case numbered in that order.
    use Question::{AlternateUnseen, HistoryEraseAsAlternate, RotationUnmeasured};
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
    // Case 384 needs both the alternate screen seen and the rotation
    // measured; it is listed under the first.
    let pinned: Vec<Pin> = vec![
        (61, RotationUnmeasured, missed(1)),
        (65, RotationUnmeasured, missed(2)),
        (70, RotationUnmeasured, missed(5)),
        (71, RotationUnmeasured, both()),
        (82, AlternateUnseen, missed(1)),
        (126, RotationUnmeasured, missed(3)),
        (168, RotationUnmeasured, missed(2)),
        (196, RotationUnmeasured, missed(1)),
        (213, RotationUnmeasured, missed(1)),
        (290, RotationUnmeasured, missed(1)),
        (301, RotationUnmeasured, repeated(1)),
        (318, RotationUnmeasured, missed(2)),
        (341, AlternateUnseen, missed(1)),
        (349, AlternateUnseen, missed(1)),
        (350, RotationUnmeasured, missed(1)),
        (364, RotationUnmeasured, repeated(1)),
        (384, AlternateUnseen, missed(1)),
        (397, AlternateUnseen, missed(1)),
        (409, AlternateUnseen, missed(1)),
        (415, RotationUnmeasured, missed(5)),
        (446, AlternateUnseen, missed(1)),
        (458, RotationUnmeasured, missed(6)),
        (498, RotationUnmeasured, repeated(2)),
        (508, RotationUnmeasured, both()),
        (510, HistoryEraseAsAlternate, repeated(3)),
    ];
    let expected: Vec<(u32, Vec<(String, usize)>)> = pinned
        .iter()
        .map(|(case, _, kinds)| (*case, counts(kinds)))
        .collect();
    assert_eq!(found, expected);
}
