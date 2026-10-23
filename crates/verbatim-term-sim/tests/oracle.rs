//! The oracle checked against readers whose faults are known
//! (`docs/design/terminal-model.md`, section 9): deliberately flawed
//! readers, a silent one, ones that miscount or misplace, and ones that
//! withhold lines for reasons whose premise is false, are caught on fixed
//! streams, with exactly the violations their faults cause, and a reader
//! that reads the ground truth passes those streams, random streams, every
//! short stream of the tiny alphabet and the conformance corpus.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;
use verbatim_term_sim::conformance::{Stream, history, streams};
use verbatim_term_sim::oracle::{
    Heard, Observation, Op, Reader, Reason, Reply, Size, Spoken, TINY_ALPHABET, Violation,
    Withheld, every_stream, observe, random_config, random_steps, stream,
};
use verbatim_term_sim::{Flavor, Query, Refused, RowId, Snapshot, screen_rows};

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
            ..Heard::default()
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
            ..Heard::default()
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

/// Reads the screen, speaks a line on a row that was blank at the same
/// place in the previous read, and withholds for `reason` every line whose
/// text differs from the other text the row at the same place had.
struct ClaimsEveryChange {
    reason: Reason,
    previous: Vec<String>,
}

impl ClaimsEveryChange {
    fn new(reason: Reason) -> Self {
        Self {
            reason,
            previous: Vec::new(),
        }
    }
}

impl Reader for ClaimsEveryChange {
    fn query(&self) -> Query {
        Query::default()
    }

    fn read(&mut self, read: &Result<Snapshot, Refused>) -> Reply {
        let rows = rows(read, Size::SMALL.width);
        let mut heard = Heard::default();
        for (row, text) in rows.iter().enumerate() {
            let before = self.previous.get(row).map_or("", String::as_str);
            if text.is_empty() || text == before {
                continue;
            }
            if before.is_empty() {
                heard.spoken.push(Spoken::Line {
                    row,
                    text: text.clone(),
                });
            } else {
                heard.withheld.push(Withheld {
                    row,
                    reason: self.reason,
                });
            }
        }
        self.previous = rows;
        Reply::Heard(heard)
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

/// Reads the whole text and reports from the ground truth: on the main
/// screen, every line of the main buffer one of whose rows has a text the
/// reader has not reported on that row, as the read shows it; every row
/// where it really went; and a review position that stays on its row while
/// the row is in the read and otherwise goes to the read's first row. It is
/// given the rows of the same reads taken by [`Whole`].
struct Truth {
    reads: Vec<Observation>,
    at: usize,
    /// The texts reported on each row.
    reported: HashMap<RowId, HashSet<String>>,
    review: Option<RowId>,
}

impl Reader for Truth {
    fn query(&self) -> Query {
        Whole.query()
    }

    fn read(&mut self, _read: &Result<Snapshot, Refused>) -> Reply {
        let read = &self.reads[self.at];
        let now: HashMap<RowId, usize> = read
            .rows
            .iter()
            .enumerate()
            .map(|(row, seen)| (seen.id, row))
            .collect();
        let mut spoken = Vec::new();
        let mut first = 0;
        for (at, seen) in read.main.iter().enumerate() {
            if seen.wraps && at + 1 < read.main.len() {
                continue;
            }
            let line = &read.main[first..=at];
            first = at + 1;
            let texts: Vec<(RowId, &str)> = line
                .iter()
                .map(|seen| (seen.id, seen.text.trim_end()))
                .filter(|(_, text)| !text.is_empty())
                .collect();
            let new = texts.iter().any(|(id, text)| {
                !self
                    .reported
                    .get(id)
                    .is_some_and(|reported| reported.contains(*text))
            });
            if !new {
                continue;
            }
            for (id, text) in texts {
                self.reported.entry(id).or_default().insert(text.to_owned());
            }
            let row = now[&line[0].id];
            let mut text: String = read.rows[row..]
                .iter()
                .take(line.len())
                .map(|seen| seen.text.as_str())
                .collect();
            text.truncate(text.trim_end().len());
            spoken.push(Spoken::Line { row, text });
        }
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
            ..Heard::default()
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
    let mut truth = Truth {
        reads: learned.reads,
        at: 0,
        reported: HashMap::new(),
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

/// A row written and then rewritten after a carriage return, read after
/// each write.
fn rewritten(texts: &[&'static str]) -> Stream {
    let steps: Vec<(Op, bool)> = texts
        .iter()
        .enumerate()
        .flat_map(|(index, &text)| {
            let rewrite = (index > 0).then_some((Op::CarriageReturn, false));
            rewrite.into_iter().chain([(Op::Write(text), true)])
        })
        .collect();
    stream(&steps)
}

fn missed(read: usize, coordinate: usize, text: &str) -> Violation {
    Violation::Missed {
        read,
        coordinate,
        text: text.to_owned(),
    }
}

fn false_reason(read: usize, row: usize, text: &str, reason: Reason) -> Violation {
    Violation::FalseReason {
        read,
        row,
        text: text.to_owned(),
        reason,
    }
}

#[test]
fn the_same_coordinate_reader_repeats_mismaps_and_moves_its_review_with_a_scroll() {
    let repeated = |row: usize, text: &str| Violation::Repeated {
        read: 2,
        row,
        text: text.to_owned(),
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
            repeated(0, "c"),
            repeated(1, "d"),
            repeated(2, "e"),
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
            missed(1, 0, "a1"),
            missed(1, 1, "a2"),
            missed(1, 2, "a3"),
            missed(1, 3, "a4"),
            missed(1, 4, "a5"),
            missed(1, 5, "a6"),
            missed(2, 2, "b1"),
            missed(2, 3, "b2"),
            missed(2, 4, "b3"),
            missed(2, 5, "b4"),
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
            missed(2, 2, "b1"),
            missed(2, 3, "b2"),
            missed(2, 4, "b3"),
            missed(2, 5, "b4"),
        ]
    );
}

#[test]
fn a_silent_reader_misses_plain_output() {
    assert_eq!(
        violations(&scroll(), &mut Silent),
        [
            missed(1, 0, "a"),
            missed(1, 1, "b"),
            missed(1, 2, "c"),
            missed(1, 3, "d"),
            missed(1, 4, "e"),
            missed(2, 5, "f"),
        ]
    );
}

#[test]
fn a_silent_reader_misses_inks_frames_and_the_line_streamed_over_them() {
    // The first frame is new; the frame redrawn on the same rows is not;
    // the streamed line written over the frame's first row is, and so is
    // the frame drawn a row lower, each row's text new to its row; the
    // frame drawn again after a read found it erased is new only where its
    // cost changed.
    assert_eq!(
        violations_in(&corpus_stream("ink"), corpus_size(), &mut Silent),
        [
            missed(1, 0, "------"),
            missed(1, 1, "> input"),
            missed(1, 2, "cost $1"),
            missed(3, 0, "streamed line"),
            missed(3, 1, "------"),
            missed(3, 2, "> input"),
            missed(3, 3, "cost $1"),
            missed(5, 3, "cost $2"),
        ]
    );
}

#[test]
fn a_summary_naming_a_wrapped_lines_second_row_or_a_row_past_the_read_is_caught() {
    let stream = stream(&[(Op::Write("wrapping-word"), false), (Op::Newline, true)]);
    let not_a_line = |read: usize, row: usize| Violation::NotALine { read, row };
    // The last read, after the quiet period, counts the line again.
    assert_eq!(
        violations(&stream, &mut CountsEveryRow),
        [
            not_a_line(0, 1),
            not_a_line(1, 1),
            not_a_line(1, 3),
            Violation::Repeated {
                read: 2,
                row: 0,
                text: "wrapping-word".to_owned()
            },
            not_a_line(2, 1),
            not_a_line(2, 3),
        ]
    );
}

#[test]
fn a_spinner_claimed_for_a_changed_word_is_caught_and_one_for_a_symbol_is_not() {
    assert_eq!(
        violations(
            &rewritten(&["- cat", "| cat", "| dog"]),
            &mut ClaimsEveryChange::new(Reason::Spinner)
        ),
        [false_reason(3, 0, "| dog", Reason::Spinner)]
    );
}

#[test]
fn a_reprint_claimed_for_new_text_is_caught_and_one_for_text_shown_before_is_not() {
    assert_eq!(
        violations(
            &rewritten(&["ok", "no", "ok"]),
            &mut ClaimsEveryChange::new(Reason::Reprint)
        ),
        [false_reason(2, 0, "no", Reason::Reprint)]
    );
}

#[test]
fn a_volatile_hold_never_reported_nor_replaced_is_caught_and_one_replaced_is_not() {
    assert_eq!(
        violations(
            &rewritten(&["50%", "60%", "70%"]),
            &mut ClaimsEveryChange::new(Reason::Volatile)
        ),
        [false_reason(3, 0, "70%", Reason::Volatile)]
    );
}

#[test]
fn the_ground_truth_reader_passes_the_fixed_streams() {
    let fixed = [
        ("scroll", scroll()),
        ("footer", footer()),
        ("flood", flood()),
        ("reprint", reprint()),
        ("rewritten", rewritten(&["- cat", "| cat", "| dog"])),
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

proptest! {
    #![proptest_config(random_config())]

    #[test]
    fn the_ground_truth_reader_passes_random_streams(
        steps in random_steps(),
        windows_terminal in any::<bool>(),
    ) {
        let flavor = if windows_terminal { Flavor::WindowsTerminal } else { Flavor::ConsoleHost };
        prop_assert_eq!(truth_violations(&stream(&steps), flavor, Size::SMALL), []);
    }
}
