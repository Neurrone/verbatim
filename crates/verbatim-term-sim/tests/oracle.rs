//! The oracle checked against readers whose faults are known
//! (`docs/design/terminal-model.md`, section 9): two deliberately simple
//! readers, a silent one and one that miscounts, are caught on fixed
//! streams, with exactly the violations their faults cause, and a reader
//! that reads the ground truth passes those streams, random streams, every
//! short stream of the tiny alphabet and the conformance corpus.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, RngSeed};
use verbatim_term_sim::conformance::{Stream, history, streams};
use verbatim_term_sim::oracle::{
    Heard, Observation, Op, Reader, Reply, Size, Spoken, TINY_ALPHABET, Violation, WORDS,
    every_stream, observe, stream,
};
use verbatim_term_sim::{Flavor, PrintedLine, Query, Refused, RowId, Snapshot, screen_rows};

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
/// at the same place in the previous read, as if rows never moved; its
/// review position is on the read's last row.
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
        let review = rows.len().checked_sub(1);
        self.previous = rows;
        Reply::Heard(Heard {
            spoken,
            moved,
            review,
        })
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
            review: None,
        })
    }
}

/// Reads the screen and says nothing.
struct Silent;

impl Reader for Silent {
    fn query(&self) -> Query {
        Query::default()
    }

    fn read(&mut self, _read: &Result<Snapshot, Refused>) -> Reply {
        Reply::Heard(Heard::default())
    }
}

/// Reads the screen and counts every row of each read, and one row more,
/// in a summary.
struct CountsEveryRow;

impl Reader for CountsEveryRow {
    fn query(&self) -> Query {
        Query::default()
    }

    fn read(&mut self, read: &Result<Snapshot, Refused>) -> Reply {
        let rows = rows(read, Size::SMALL.width).len();
        Reply::Heard(Heard {
            spoken: vec![Spoken::Summary { rows: 0..rows + 1 }],
            ..Heard::default()
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

/// Reads the whole text and reports from the ground truth: each printed
/// line when a read first shows it as it was finished, every row where it
/// really went, and a review position that stays on its row while the row
/// is in the read and otherwise goes to the read's first row. It is given
/// the rows of the same reads taken by [`Whole`].
struct Truth {
    printed: Vec<PrintedLine>,
    /// The printed lines by their first row.
    by_first: HashMap<RowId, Vec<usize>>,
    reads: Vec<Observation>,
    at: usize,
    reported: HashSet<usize>,
    review: Option<RowId>,
}

impl Reader for Truth {
    fn query(&self) -> Query {
        Whole.query()
    }

    fn read(&mut self, _read: &Result<Snapshot, Refused>) -> Reply {
        let read = &self.reads[self.at];
        let mut spoken = Vec::new();
        for (row, seen) in read.rows.iter().enumerate() {
            let shown = self.by_first.get(&seen.id).and_then(|lines| {
                lines.iter().copied().find(|&line| {
                    let line = &self.printed[line];
                    !read.alternate
                        && line.finished <= read.writes
                        && line.changed.is_none_or(|changed| changed > read.writes)
                        && line.left.is_none_or(|left| left > read.writes)
                })
            });
            if let Some(line) = shown
                && self.reported.insert(line)
            {
                // A line's last rows can lie past the document's end, when
                // they are blank and below the cursor.
                let mut text: String = read.rows[row..]
                    .iter()
                    .take(self.printed[line].rows.len())
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
        if self.review.is_none_or(|id| !now.contains_key(&id)) {
            self.review = read.rows.first().map(|seen| seen.id);
        }
        let review = self.review.and_then(|id| now.get(&id).copied());
        self.at += 1;
        Reply::Heard(Heard {
            spoken,
            moved,
            review,
        })
    }
}

/// The violations of a reader over `stream` in a terminal of `size`.
fn violations_in(stream: &Stream, size: Size, reader: &mut dyn Reader) -> Vec<Violation> {
    let mut terminal = size.terminal(Flavor::ConsoleHost);
    observe(&mut terminal, stream, reader).violations()
}

/// The violations of a reader over `stream` in a small console host.
fn violations(stream: &Stream, reader: &mut dyn Reader) -> Vec<Violation> {
    violations_in(stream, Size::SMALL, reader)
}

/// The violations of the ground-truth reader over `stream`.
fn truth_violations(stream: &Stream, flavor: Flavor, size: Size) -> Vec<Violation> {
    let learned = observe(&mut size.terminal(flavor), stream, &mut Whole);
    let mut by_first: HashMap<RowId, Vec<usize>> = HashMap::new();
    for (index, line) in learned.printed.iter().enumerate() {
        by_first.entry(line.rows[0]).or_default().push(index);
    }
    let mut truth = Truth {
        printed: learned.printed,
        by_first,
        reads: learned.reads,
        at: 0,
        reported: HashSet::new(),
        review: None,
    };
    observe(&mut size.terminal(flavor), stream, &mut truth).violations()
}

/// The conformance corpus's terminal in the console host.
fn corpus_size() -> Size {
    Size {
        width: 120,
        height: 30,
        history: history(Flavor::ConsoleHost),
    }
}

/// The stream of the corpus named `name`.
fn corpus_stream(name: &str) -> Stream {
    streams()
        .into_iter()
        .find_map(|(found, stream)| (found == name).then_some(stream))
        .expect("the stream is in the corpus")
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

/// A line, a clear that a read catches, and the line printed again.
fn reprint() -> Stream {
    stream(&[
        (Op::Write("ok\r\n"), true),
        (Op::Clear, true),
        (Op::Write("ok\r\n"), true),
    ])
}

fn missed(line: usize, text: &str) -> Violation {
    Violation::Missed {
        line,
        text: text.to_owned(),
    }
}

#[test]
fn the_same_coordinate_reader_repeats_mismaps_and_moves_its_review_with_a_scroll() {
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
            Violation::ReviewMoved {
                read: 1,
                went: 0,
                said: Some(4)
            },
            repeated(2, "c"),
            repeated(3, "d"),
            mismapped(2, 0),
            mismapped(3, 1),
            mismapped(4, 2),
            Violation::ReviewMoved {
                read: 2,
                went: 2,
                said: Some(4)
            },
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
fn a_silent_reader_misses_new_output_but_not_a_line_the_footer_showed_before() {
    assert_eq!(violations(&footer(), &mut Silent), [missed(0, "cat")]);
}

#[test]
fn a_silent_reader_misses_inks_frames_and_the_line_streamed_over_them() {
    // The first frame is new, and stays required although the next frame
    // erases and redraws it; the frames redrawn with text the screen
    // showed are not; the streamed line written over the frame's first
    // row is; and the frame drawn after a read found it erased is new
    // again.
    assert_eq!(
        violations_in(&corpus_stream("ink"), corpus_size(), &mut Silent),
        [
            missed(0, "------"),
            missed(1, "> input"),
            missed(2, "cost $1"),
            missed(6, "streamed line"),
            missed(10, "------"),
            missed(11, "> input"),
            missed(12, "cost $2"),
        ]
    );
}

#[test]
fn a_silent_reader_need_not_speak_a_reprint_after_a_read_found_the_screen_cleared() {
    assert_eq!(violations(&reprint(), &mut Silent), [missed(0, "ok")]);
}

#[test]
fn a_summary_naming_a_wrapped_lines_second_row_or_a_row_past_the_read_is_caught() {
    let stream = stream(&[(Op::Write("wrapping-word"), false), (Op::Newline, true)]);
    let not_a_line = |read: usize, row: usize| Violation::NotALine { read, row };
    assert_eq!(
        violations(&stream, &mut CountsEveryRow),
        [not_a_line(0, 1), not_a_line(1, 1), not_a_line(1, 3)]
    );
}

#[test]
fn the_ground_truth_reader_passes_the_fixed_streams() {
    let fixed = [
        ("scroll", scroll()),
        ("footer", footer()),
        ("flood", flood()),
        ("reprint", reprint()),
    ];
    let failures: Vec<(&str, Vec<Violation>)> = fixed
        .iter()
        .map(|(name, stream)| {
            (
                *name,
                truth_violations(stream, Flavor::ConsoleHost, Size::SMALL),
            )
        })
        .filter(|(_, violations)| !violations.is_empty())
        .collect();
    assert_eq!(failures, []);
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

/// The random streams' configuration, every field written out:
/// `Config::default()` reads `PROPTEST_` environment variables, which would
/// let the machine change what is checked. A fixed seed makes every run
/// check the same streams, and no failure files are kept: a failure prints
/// its shrunk steps.
fn config() -> Config {
    Config {
        cases: 512,
        max_local_rejects: 65_536,
        max_global_rejects: 1024,
        max_flat_map_regens: 1_000_000,
        failure_persistence: None,
        source_file: None,
        test_name: None,
        fork: false,
        timeout: 0,
        max_shrink_time: 0,
        max_shrink_iters: u32::MAX,
        max_default_size_range: 100,
        result_cache: proptest::test_runner::noop_result_cache,
        verbose: 0,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(20_261_023),
        _non_exhaustive: (),
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn the_ground_truth_reader_passes_random_streams(
        steps in prop::collection::vec((op(), any::<bool>()), 1..40),
        windows_terminal in any::<bool>(),
    ) {
        let flavor = if windows_terminal { Flavor::WindowsTerminal } else { Flavor::ConsoleHost };
        prop_assert_eq!(truth_violations(&stream(&steps), flavor, Size::SMALL), []);
    }
}
