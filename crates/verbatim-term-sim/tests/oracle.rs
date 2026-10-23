//! The oracle checked against readers whose faults are known
//! (`docs/design/terminal-model.md`, section 9): two deliberately simple
//! readers are caught on fixed streams, with exactly the violations their
//! faults cause, and a reader that reads the ground truth passes those
//! streams, random streams, every short stream of the tiny alphabet and the
//! conformance corpus.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngSeed};
use verbatim_term_sim::conformance::Stream;
use verbatim_term_sim::conformance::{history, streams};
use verbatim_term_sim::oracle::{
    Heard, Observation, Op, Reader, Reply, Size, Spoken, TINY_ALPHABET, Violation, WORDS,
    every_stream, observe, stream,
};
use verbatim_term_sim::{Flavor, OutputLine, Query, Refused, RowId, Snapshot, screen_rows};

/// A read's text as rows of `width`, padding left out.
fn rows(snapshot: &Result<Snapshot, Refused>, width: usize) -> Vec<String> {
    let text = &snapshot.as_ref().expect("a simulated read").text;
    let text = text.strip_suffix("\r\n").unwrap_or(text);
    text.split("\r\n")
        .flat_map(|line| {
            let chars: Vec<char> = line.chars().collect();
            chars
                .chunks(width)
                .map(|row| row.iter().collect::<String>().trim_end().to_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Reads the screen and speaks every row whose text differs from the row
/// at the same place in the previous read, as if rows never moved.
struct SameCoordinate {
    previous: Vec<String>,
}

impl Reader for SameCoordinate {
    fn query(&self) -> Query {
        Query::default()
    }

    fn read(&mut self, read: &Result<Snapshot, Refused>) -> Reply {
        let rows = rows(read, Size::SMALL.width);
        let spoken = rows
            .iter()
            .enumerate()
            .filter(|(row, text)| !text.is_empty() && self.previous.get(*row) != Some(*text))
            .map(|(row, text)| Spoken::Line {
                row,
                text: text.clone(),
            })
            .collect();
        let moved = Some((0..self.previous.len()).map(Some).collect());
        self.previous = rows;
        Reply::Heard(Heard { spoken, moved })
    }
}

/// Reads from where the read before said to start, as the read program
/// does, and speaks the lines with text that a longest common subsequence
/// of the two reads' lines with text leaves unmatched.
struct TextDiff {
    query: Query,
    previous: Vec<String>,
}

impl Reader for TextDiff {
    fn query(&self) -> Query {
        self.query
    }

    fn read(&mut self, read: &Result<Snapshot, Refused>) -> Reply {
        let rows = rows(read, Size::SMALL.width);
        let lines = |rows: &[String]| -> Vec<usize> {
            (0..rows.len())
                .filter(|&row| !rows[row].is_empty())
                .collect()
        };
        let (old, new) = (lines(&self.previous), lines(&rows));
        let mut longest = vec![vec![0_usize; new.len() + 1]; old.len() + 1];
        for i in (0..old.len()).rev() {
            for j in (0..new.len()).rev() {
                longest[i][j] = if self.previous[old[i]] == rows[new[j]] {
                    longest[i + 1][j + 1] + 1
                } else {
                    longest[i + 1][j].max(longest[i][j + 1])
                };
            }
        }
        let mut moved = vec![None; self.previous.len()];
        let mut matched = HashSet::new();
        let (mut i, mut j) = (0, 0);
        while i < old.len() && j < new.len() {
            if self.previous[old[i]] == rows[new[j]] {
                moved[old[i]] = Some(new[j]);
                matched.insert(new[j]);
                i += 1;
                j += 1;
            } else if longest[i + 1][j] >= longest[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
        let spoken = new
            .iter()
            .filter(|row| !matched.contains(*row))
            .map(|&row| Spoken::Line {
                row,
                text: rows[row].clone(),
            })
            .collect();
        let snapshot = read.as_ref().expect("a simulated read");
        self.query = Query {
            start: Some(snapshot.next_start),
            screen_rows: screen_rows(&snapshot.screen),
            whole: false,
        };
        self.previous = rows;
        Reply::Heard(Heard {
            spoken,
            moved: Some(moved),
        })
    }
}

/// Reads the whole text and hears nothing, to learn which rows each read
/// covers.
struct Whole;

impl Reader for Whole {
    fn query(&self) -> Query {
        Query {
            whole: true,
            ..Query::default()
        }
    }

    fn read(&mut self, _read: &Result<Snapshot, Refused>) -> Reply {
        Reply::Heard(Heard::default())
    }
}

/// Reads the whole text and reports from the ground truth: each output
/// line when a read first finds it finished, and every row where it
/// really went. It is given the rows of the same reads taken by [`Whole`].
struct Truth {
    output: Vec<OutputLine>,
    /// Each output line's index by its first row.
    firsts: HashMap<RowId, usize>,
    reads: Vec<Observation>,
    at: usize,
    reported: HashSet<usize>,
}

impl Reader for Truth {
    fn query(&self) -> Query {
        Whole.query()
    }

    fn read(&mut self, _read: &Result<Snapshot, Refused>) -> Reply {
        let read = &self.reads[self.at];
        let mut spoken = Vec::new();
        for (row, seen) in read.rows.iter().enumerate() {
            let found = self.firsts.get(&seen.id).copied().filter(|&line| {
                let line = &self.output[line];
                !read.alternate
                    && line.finished <= read.writes
                    && line.left.is_none_or(|left| left > read.writes)
            });
            if let Some(line) = found
                && self.reported.insert(line)
            {
                // A line's last rows can lie past the document's end, when
                // they are blank and below the cursor.
                let mut text: String = read.rows[row..]
                    .iter()
                    .take(self.output[line].rows.len())
                    .map(|seen| seen.text.as_str())
                    .collect();
                text.truncate(text.trim_end().len());
                spoken.push(Spoken::Line { row, text });
            }
        }
        let now: HashMap<RowId, usize> = read
            .rows
            .iter()
            .enumerate()
            .map(|(row, seen)| (seen.id, row))
            .collect();
        let moved = self.at.checked_sub(1).map(|previous| {
            self.reads[previous]
                .rows
                .iter()
                .map(|old| now.get(&old.id).copied())
                .collect()
        });
        self.at += 1;
        Reply::Heard(Heard { spoken, moved })
    }
}

/// The violations of a reader over `stream` in a small console host.
fn violations(stream: &Stream, reader: &mut dyn Reader) -> Vec<Violation> {
    let mut terminal = Size::SMALL.terminal(Flavor::ConsoleHost);
    observe(&mut terminal, stream, reader).violations()
}

/// The violations of the ground-truth reader over `stream`.
fn truth_violations(stream: &Stream, flavor: Flavor, size: Size) -> Vec<Violation> {
    let learned = observe(&mut size.terminal(flavor), stream, &mut Whole);
    let mut truth = Truth {
        firsts: learned
            .output
            .iter()
            .enumerate()
            .map(|(index, line)| (line.rows[0], index))
            .collect(),
        output: learned.output,
        reads: learned.reads,
        at: 0,
        reported: HashSet::new(),
    };
    observe(&mut size.terminal(flavor), stream, &mut truth).violations()
}

/// Five lines on the five-row screen, then two more, which scroll the
/// screen up two rows.
fn scroll() -> Stream {
    stream(&[
        (Op::Write("a\r\nb\r\nc\r\nd\r\ne"), true),
        (Op::Write("\r\nf\r\n"), true),
    ])
}

/// A footer on the screen's last row, drawn with the cursor saved and
/// restored, below the scroll region the output scrolls in; it shows "dog"
/// before the program prints "dog" as output, and then "eel".
fn footer() -> Stream {
    stream(&[
        (Op::Region { top: 1, bottom: 4 }, false),
        (Op::Footer("dog"), false),
        (Op::Write("cat\r\n"), true),
        (Op::Write("dog\r\n"), false),
        (Op::Footer("eel"), true),
    ])
}

/// Ten lines, which fill the history, then eight more, which rotate it.
fn flood() -> Stream {
    stream(&[
        (
            Op::Flood {
                lines: 10,
                word: "a",
            },
            true,
        ),
        (
            Op::Flood {
                lines: 8,
                word: "b",
            },
            true,
        ),
    ])
}

fn missed(line: usize, text: &str) -> Violation {
    Violation::Missed {
        line,
        text: text.to_owned(),
    }
}

#[test]
fn the_same_coordinate_reader_repeats_and_mismaps_the_rows_a_scroll_moved() {
    let repeated = |line: usize, text: &str| Violation::Repeated {
        line,
        text: text.to_owned(),
        read: 2,
    };
    let mismapped = |row: usize, went: usize| Violation::Mismapped {
        read: 2,
        row,
        went,
        said: Some(row),
    };
    assert_eq!(
        violations(
            &scroll(),
            &mut SameCoordinate {
                previous: Vec::new()
            }
        ),
        [
            repeated(2, "c"),
            repeated(3, "d"),
            repeated(4, "e"),
            mismapped(2, 0),
            mismapped(3, 1),
            mismapped(4, 2),
        ]
    );
}

#[test]
fn the_same_coordinate_reader_misses_the_lines_a_flood_pushed_into_the_history() {
    assert_eq!(
        violations(
            &flood(),
            &mut SameCoordinate {
                previous: Vec::new()
            }
        ),
        [
            missed(0, "a1"),
            missed(1, "a2"),
            missed(2, "a3"),
            missed(3, "a4"),
            missed(4, "a5"),
            missed(5, "a6"),
            missed(10, "b1"),
            missed(11, "b2"),
            missed(12, "b3"),
            missed(13, "b4"),
        ]
    );
}

#[test]
fn the_text_diff_reader_takes_an_output_line_for_the_footer_that_showed_its_text() {
    assert_eq!(
        violations(
            &footer(),
            &mut TextDiff {
                query: Query::default(),
                previous: Vec::new()
            }
        ),
        [missed(1, "dog")]
    );
}

#[test]
fn the_text_diff_reader_misses_the_lines_a_flood_rotated_above_its_kept_start() {
    assert_eq!(
        violations(
            &flood(),
            &mut TextDiff {
                query: Query::default(),
                previous: Vec::new()
            }
        ),
        [
            missed(10, "b1"),
            missed(11, "b2"),
            missed(12, "b3"),
            missed(13, "b4"),
        ]
    );
}

#[test]
fn the_ground_truth_reader_passes_the_scroll() {
    assert_eq!(
        truth_violations(&scroll(), Flavor::ConsoleHost, Size::SMALL),
        []
    );
}

#[test]
fn the_ground_truth_reader_passes_the_footer() {
    assert_eq!(
        truth_violations(&footer(), Flavor::ConsoleHost, Size::SMALL),
        []
    );
}

#[test]
fn the_ground_truth_reader_passes_the_flood() {
    assert_eq!(
        truth_violations(&flood(), Flavor::ConsoleHost, Size::SMALL),
        []
    );
}

#[test]
fn the_ground_truth_reader_passes_every_stream_of_four_tiny_operations() {
    let streams = every_stream(&TINY_ALPHABET, 4);
    assert_eq!(streams.len(), 9 + 81 + 729 + 6561);
    let failures: Vec<(String, Flavor, Vec<Violation>)> = streams
        .iter()
        .flat_map(|stream| {
            [Flavor::ConsoleHost, Flavor::WindowsTerminal].map(|flavor| {
                (
                    stream.about.clone(),
                    flavor,
                    truth_violations(stream, flavor, Size::TINY),
                )
            })
        })
        .filter(|(_, _, violations)| !violations.is_empty())
        .collect();
    assert_eq!(failures, []);
}

#[test]
fn the_ground_truth_reader_passes_the_corpus() {
    let corpus = streams();
    assert!(!corpus.is_empty(), "the corpus has no streams");
    let failures: Vec<(String, Flavor, Vec<Violation>)> = corpus
        .iter()
        .flat_map(|(name, stream)| {
            [Flavor::ConsoleHost, Flavor::WindowsTerminal].map(|flavor| {
                let size = Size {
                    width: 120,
                    height: 30,
                    history: history(flavor),
                };
                (name.clone(), flavor, truth_violations(stream, flavor, size))
            })
        })
        .filter(|(_, _, violations)| !violations.is_empty())
        .collect();
    assert_eq!(failures, []);
}

fn op() -> impl Strategy<Value = Op> {
    let word = || prop::sample::select(&WORDS[..]);
    prop_oneof![
        4 => word().prop_map(Op::Write),
        4 => Just(Op::Newline),
        1 => Just(Op::LineFeed),
        1 => Just(Op::CarriageReturn),
        1 => (1..=5_u8, 1..=12_u8).prop_map(|(row, column)| Op::MoveTo { row, column }),
        1 => (-3..=3_i8).prop_map(Op::MoveRows),
        1 => (-6..=6_i8).prop_map(Op::MoveColumns),
        1 => (0..=2_u8).prop_map(Op::EraseLine),
        1 => Just(Op::EraseBelow),
        1 => word().prop_map(Op::Footer),
        1 => word().prop_map(Op::Redraw),
        1 => (1..=5_u8, 1..=5_u8).prop_map(|(top, bottom)| Op::Region { top, bottom }),
        1 => Just(Op::ResetRegion),
        1 => (1..=2_u8).prop_map(Op::InsertRows),
        1 => (1..=2_u8).prop_map(Op::DeleteRows),
        1 => (8..=16_u8, word()).prop_map(|(lines, word)| Op::Flood { lines, word }),
        1 => Just(Op::Clear),
        1 => Just(Op::ClearHistory),
        1 => Just(Op::EnterAlternate),
        1 => Just(Op::LeaveAlternate),
    ]
}

proptest! {
    // A fixed seed, so every run checks the same streams, and no
    // regression files: a failure prints its shrunk steps.
    #![proptest_config(Config {
        cases: 512,
        rng_seed: RngSeed::Fixed(20_261_023),
        failure_persistence: None,
        ..Config::default()
    })]

    #[test]
    fn the_ground_truth_reader_passes_random_streams(
        steps in prop::collection::vec((op(), any::<bool>()), 1..40),
        windows_terminal in any::<bool>(),
    ) {
        let flavor = if windows_terminal { Flavor::WindowsTerminal } else { Flavor::ConsoleHost };
        prop_assert_eq!(truth_violations(&stream(&steps), flavor, Size::SMALL), []);
    }
}
