//! The terminal reading rules (`verbatim_text::terminal_reading`, on the
//! row memory of `verbatim_text::terminal_rows`) checked by the simulated
//! terminal's oracle (`docs/design/terminal-model.md`, "Testing"). The
//! reader under test is the real rules: every read is given to a
//! `TerminalReader` at the stream's times (a recorded stream's, or 10
//! milliseconds apart for one written by hand, the last read 10 seconds
//! after the last chunk), its speech reported as the oracle takes it (a
//! line, growth, a redrawn row's change; floods are Core's to split, so
//! every new line is reported), its silences stated with the oracle's
//! reasons, the listed limits it meets stated, and where each row went and
//! its review position taken from the memory. It is checked over the
//! conformance corpus in both terminals, the oracle's random streams, and
//! every stream of up to four operations of the tiny alphabet.
//!
//! No violation remains, and none is pinned: a failure is a consistent
//! explanation the oracle accepts, a listed limit through its own reason,
//! or a defect in the code.

use proptest::prelude::*;
use verbatim_term_sim::conformance::{Stream, history, streams};
use verbatim_term_sim::oracle::{
    Heard, Limit, Op, Reader, Reason, Reply, Size, Spoken, TINY_ALPHABET, Violation, Withheld,
    every_stream, observe, random_config, random_steps, stream,
};
use verbatim_term_sim::{Flavor, Query, Refused, Snapshot};
use verbatim_text::terminal_reading::{self as rules, Settings, Speech, Step, TerminalReader};
use verbatim_text::terminal_rows::{OldRow, Read, ReadText};

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
            // A blank line counts in Core's flood policy, and says nothing.
            Speech::Line { text, .. } if text.is_empty() => {}
            Speech::Line { row, text } => heard.spoken.push(Spoken::Line {
                row: *row,
                text: text.to_string(),
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
            // The lines after it are all new, and Core counts those it
            // skips.
            Speech::LostRead(_) => {}
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
                rules::Reason::WrittenBack => Reason::WrittenBack,
            },
        })
        .collect();
    // The listed limits the read may meet ("Limits"), each accepted by the
    // oracle only where its precondition holds.
    if let Some(outcome) = &step.outcome {
        if outcome.by_text {
            heard.limits.push(Limit::RepeatedText);
        }
        if outcome.lost.is_some() {
            heard.limits.push(Limit::LostRead);
        }
        if outcome.rewritten {
            heard.limits.push(Limit::Rewritten);
        }
    }
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
            text: ReadText::unpadded(&snapshot.text),
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
        if step.history_block {
            return Reply::Again(Query {
                start: None,
                screen_rows,
                whole: false,
                above: screen_rows,
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
        // A read with no text leaves where the next read starts as the last
        // read placed it ("A read with no text").
        self.query = Query {
            start: if blank {
                self.query.start
            } else {
                Some(next_start)
            },
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

/// The core's reader with one listed limit never stated, to show that a
/// stream's limit is what the oracle accepts it through.
struct Without {
    core: Core,
    limit: Limit,
}

impl Reader for Without {
    fn query(&self) -> Query {
        self.core.query()
    }

    fn read(&mut self, read: &Result<Snapshot, Refused>) -> Reply {
        let mut reply = self.core.read(read);
        if let Reply::Heard(heard) = &mut reply {
            heard.limits.retain(|limit| *limit != self.limit);
        }
        reply
    }

    fn quiet(&mut self) {
        self.core.quiet();
    }
}

/// The violations of the core's reader over `stream` in a terminal of
/// `size`, with `limit` never stated.
fn violations_without(stream: &Stream, flavor: Flavor, size: Size, limit: Limit) -> Vec<Violation> {
    let mut terminal = size.terminal(flavor);
    let mut reader = Without {
        core: Core::new(stream),
        limit,
    };
    observe(&mut terminal, stream, &mut reader).violations()
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

#[test]
fn the_cores_reader_meets_each_listed_limit_it_states_on_its_smallest_stream() {
    // Each a case the design lists under "Limits", accepted only through
    // its own reason, and each failing without it:
    // - a screen left blank by a clear the console host pushed into the
    //   history ("A read with no text");
    // - a wrapped line's last row erased and printed again as a line of its
    //   own ("Lingering").
    let blank = stream(&[
        (Op::Write("ok"), true),
        (Op::Write("\r\nhello"), false),
        (Op::Clear, true),
        (Op::Write("cat"), true),
    ]);
    let written_back = stream(&[
        (Op::Write("wrapping-word"), true),
        (Op::EraseLine(2), true),
        (Op::Write("\r\nd"), true),
    ]);
    for stream in [blank, written_back] {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            assert_eq!(
                violations(&stream, flavor, Size::SMALL),
                [],
                "{} {flavor:?}",
                stream.about
            );
        }
    }
}

#[test]
fn the_cores_reader_meets_the_random_streams_limits_on_their_smallest_streams() {
    // Each a case the design lists under "Limits" for the random streams,
    // accepted only through its own reason, and each failing without it:
    // - a flood after two identical rows, which fit the read at two places,
    //   so the lost read says "skipped lines" (repeated text);
    // - a flood larger than the read covers, after a full history, whose
    //   lines above the screen a lost read counts (a lost read);
    // - a volatile row held when only reads with no text follow, which the
    //   reader's timer speaks (a read with no text).
    let repeated = stream(&[
        (Op::MoveTo { row: 4, column: 3 }, false),
        (
            Op::WordDown {
                column: 4,
                word: "ok",
            },
            false,
        ),
        (Op::Newline, false),
        (Op::Write("ok"), true),
        (
            Op::Flood {
                lines: 8,
                word: "a b",
            },
            false,
        ),
    ]);
    let lost = stream(&[
        (
            Op::Restyle {
                rows: 1,
                text: "dog",
            },
            false,
        ),
        (Op::Newline, false),
        (Op::Newline, false),
        (Op::Newline, false),
        (
            Op::WordDown {
                column: 1,
                word: "ok",
            },
            false,
        ),
        (Op::Newline, true),
        (
            Op::Flood {
                lines: 9,
                word: "ok",
            },
            false,
        ),
        (
            Op::WordDown {
                column: 1,
                word: "dog",
            },
            false,
        ),
    ]);
    let held = stream(&[
        (Op::Write("ok"), true),
        (Op::InsertAboveFooter, false),
        (Op::Repaint("cat"), true),
        (
            Op::Restyle {
                rows: 1,
                text: "dog",
            },
            true,
        ),
        (
            Op::WordDown {
                column: 4,
                word: "ok",
            },
            false,
        ),
        (Op::Clear, false),
    ]);
    meet_only_through_their_limits(&[
        (repeated, Limit::RepeatedText),
        (lost, Limit::LostRead),
        (held, Limit::BlankScreen),
    ]);
}

#[test]
fn a_flood_and_a_repaint_that_refuse_the_start_are_a_lost_read_with_every_line_reported() {
    // In Windows Terminal, a flood and a repaint, which erases the screen
    // and shrinks the document, before the next read refuse its start and
    // keep nothing: a lost read, whose whole document has nothing of the
    // last read, so every line is new and reported, with no limit needed
    // ("Screens", "A lost read"). The console host's erase pushes the
    // screen into the history, refusing nothing, so the stream is Windows
    // Terminal's alone.
    let refused = stream(&[
        (
            Op::Flood {
                lines: 12,
                word: "hello",
            },
            true,
        ),
        (
            Op::Flood {
                lines: 10,
                word: "dog",
            },
            false,
        ),
        (Op::Repaint("cat"), true),
    ]);
    let flavor = Flavor::WindowsTerminal;
    assert_eq!(
        violations_without(&refused, flavor, Size::SMALL, Limit::LostRead),
        []
    );
}

#[test]
fn the_cores_reader_meets_the_rewritten_history_limit_on_its_smallest_stream() {
    // A limit ("Limits"), accepted only through its own reason and failing
    // without it: twenty lines in a 12 by 5 terminal with 30 rows of
    // history, then the screen and the history erased and printed again
    // with "new" near the top and lines 3 to 9 left out, as Pi's regular
    // mode prints its transcript again on Control+O. The read keeps the
    // last read's rows, and "new", above the read's first row, is not
    // heard.
    let rewritten = stream(&[
        (
            Op::Write(
                "l1\r\nl2\r\nl3\r\nl4\r\nl5\r\nl6\r\nl7\r\nl8\r\nl9\r\nl10\r\n\
                 l11\r\nl12\r\nl13\r\nl14\r\nl15\r\nl16\r\nl17\r\nl18\r\nl19\r\nl20",
            ),
            true,
        ),
        (
            Op::Write(
                "\x1b[2J\x1b[3J\x1b[Hl1\r\nl2\r\nnew\r\nl10\r\nl11\r\nl12\r\nl13\r\n\
                 l14\r\nl15\r\nl16\r\nl17\r\nl18\r\nl19\r\nl20",
            ),
            true,
        ),
    ]);
    let size = Size {
        width: 12,
        height: 5,
        history: 30,
    };
    for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
        assert_eq!(violations(&rewritten, flavor, size), [], "{flavor:?}");
        assert_ne!(
            violations_without(&rewritten, flavor, size, Limit::Rewritten),
            [],
            "{flavor:?}"
        );
    }
}

#[test]
fn the_cores_reader_meets_the_rewritten_history_limit_for_text_printed_again_lower() {
    // The same limit's other case: ten lines and "hello", then eight lines
    // of the same text from the end of "hello", in a full 6-row history.
    // Most of the first copy left the buffer, and the second puts the text
    // of the last read's rows two rows lower than they were, its screen's
    // rows with them, which reads exactly as a program printing its history
    // again longer: the history block is found there, and the line above
    // it, "hellook1", is not heard.
    let again = stream(&[
        (
            Op::Flood {
                lines: 10,
                word: "ok",
            },
            true,
        ),
        (Op::Write("hello"), true),
        (
            Op::Flood {
                lines: 8,
                word: "ok",
            },
            true,
        ),
    ]);
    meet_only_through_their_limits(&[(again, Limit::Rewritten)]);
}

#[test]
fn pis_transcript_printed_again_on_control_o_needs_only_the_rewritten_history_limit() {
    // Pi 1.4.0's regular mode erases the screen and the history on
    // Control+O and prints its transcript again, longer and then shorter,
    // with lines added above the screen (pi-changed-above recording). The
    // reader speaks what changed within the last read's reach, and the
    // lines added above the read are accepted only through the limit for a
    // rewritten history (Dickson, 2026-10-25).
    let corpus = streams();
    let (_, recorded) = corpus
        .iter()
        .find(|(name, _)| name == "pi-changed-above")
        .expect("the pi-changed-above recording");
    for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
        let size = Size {
            width: 120,
            height: 30,
            history: history(flavor),
        };
        assert_eq!(violations(recorded, flavor, size), [], "{flavor:?}");
        assert_ne!(
            violations_without(recorded, flavor, size, Limit::Rewritten),
            [],
            "{flavor:?}"
        );
    }
}

/// Asserts that each stream shows no violation in either terminal, and
/// some once its limit is never stated.
fn meet_only_through_their_limits(streams: &[(Stream, Limit)]) {
    for (stream, limit) in streams {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            assert_eq!(
                violations(stream, flavor, Size::SMALL),
                [],
                "{} {flavor:?}",
                stream.about
            );
            assert_ne!(
                violations_without(stream, flavor, Size::SMALL, *limit),
                [],
                "{} {flavor:?} without {limit:?}",
                stream.about
            );
        }
    }
}

#[test]
fn a_flood_repeating_the_last_reads_text_lower_down_is_not_taken_for_its_rows() {
    // Between two reads every row of the first left the buffer, and a
    // second flood printed "ok4" to "ok8" again four rows lower than the
    // first read had them. Found by text, rows of the history block are
    // never found lower than their place ("The alignment"), so the new
    // lines above them are not taken for history.
    let flood = stream(&[
        (
            Op::Flood {
                lines: 8,
                word: "ok",
            },
            false,
        ),
        (Op::Write("dog\r\nhello\r\n\r\nwrapping-word\r\n"), true),
        (
            Op::Flood {
                lines: 9,
                word: "ok",
            },
            false,
        ),
        (Op::EraseLines(2), true),
    ]);
    for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
        assert_eq!(violations(&flood, flavor, Size::SMALL), [], "{flavor:?}");
    }
}

#[test]
fn an_input_box_sent_and_drawn_again_lower_keeps_both_its_borders() {
    // Claude Code's input box sent with a short question: the question's
    // one row stays where it was, unchanged, and the box is drawn again
    // lower, past it. The box's two borders read the same; the subsequence
    // keeps the question and one border, and the other border is kept at
    // the nearest new row with its text, so neither is spoken ("The
    // alignment"; the claude-code-reply recording).
    let sent = stream(&[
        (Op::Write("x\r\n====\r\n> a\r\n====\x1b[3;4H"), true),
        (
            Op::Write("\x1b[2;1H\x1b[J\r\n> a\r\n\r\n* y\r\n====\r\n>\r\n====\x1b[A"),
            true,
        ),
    ]);
    for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
        assert_eq!(violations(&sent, flavor, Size::SMALL), [], "{flavor:?}");
    }
}
