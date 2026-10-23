//! The oracle checked against readers whose faults are known
//! (`docs/design/terminal-model.md`, section 9): deliberately flawed
//! readers, a silent one, ones that miscount or misplace, and ones that
//! withhold lines for reasons whose premise is false, are caught on fixed
//! streams, with exactly the violations their faults cause; readers whose
//! reasons are true pass; and a reader that reports from the ground truth
//! by text passes those streams, random streams, every short stream of the
//! tiny alphabet and the conformance corpus.

use std::collections::{HashMap, HashSet};

use proptest::prelude::*;
use verbatim_term_sim::conformance::{Stream, history, streams};
use verbatim_term_sim::oracle::{
    Heard, Observation, Op, Reader, Reason, Reply, Seen, Size, Spoken, TINY_ALPHABET, Violation,
    Withheld, every_stream, observe, random_config, random_steps, stream,
};
use verbatim_term_sim::{Flavor, Query, Refused, Snapshot, screen_rows};

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
            above: 0,
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

/// Reads the screen, or the whole text when `whole`, and says at each read
/// what its script gives for that read, counted from the first, and
/// nothing past the script's end.
struct Scripted {
    whole: bool,
    script: Vec<Heard>,
    at: usize,
}

impl Scripted {
    fn new(script: Vec<Heard>) -> Self {
        Self {
            whole: false,
            script,
            at: 0,
        }
    }
}

impl Reader for Scripted {
    fn query(&self) -> Query {
        Query {
            whole: self.whole,
            ..Query::default()
        }
    }

    fn read(&mut self, _read: &Result<Snapshot, Refused>) -> Reply {
        let heard = self.script.get(self.at).cloned().unwrap_or_default();
        self.at += 1;
        Reply::Heard(heard)
    }
}

/// Reads the whole text and withholds every line with text as volatile.
struct HoldsEveryLine;

impl Reader for HoldsEveryLine {
    fn query(&self) -> Query {
        Whole.query()
    }

    fn read(&mut self, read: &Result<Snapshot, Refused>) -> Reply {
        let snapshot = read.as_ref().expect("a simulated read");
        let text = snapshot.text.strip_suffix("\r\n").unwrap_or(&snapshot.text);
        let mut withheld = Vec::new();
        let mut row = 0;
        for line in text.split("\r\n") {
            if !line.trim_end().is_empty() {
                withheld.push(Withheld {
                    row,
                    reason: Reason::Volatile,
                });
            }
            row += line.chars().count().div_ceil(Size::SMALL.width).max(1);
        }
        Reply::Heard(Heard {
            withheld,
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

/// Reads the whole text and reports by text: on the main screen, every
/// line whose text no earlier read showed, as the read shows it, but a
/// line whose start left the buffer when its text ends one shown before;
/// every row whose text is on exactly one row of both reads where that
/// text went; and a review position that stays on its text while that text
/// is on exactly one row, and otherwise goes to the read's first row. It is
/// given the rows of the same reads taken by [`Whole`].
struct Truth {
    reads: Vec<Observation>,
    at: usize,
    /// Every line's text the main screen showed.
    shown: HashSet<String>,
    /// The text of the row the review position is on.
    review: Option<String>,
}

/// The rows' texts, padding left out, which only a line's last row has.
fn texts(rows: &[Seen]) -> Vec<String> {
    rows.iter()
        .enumerate()
        .map(|(row, seen)| {
            let trimmed = seen.text.trim_end();
            if seen.wraps && row + 1 < rows.len() && !trimmed.is_empty() {
                seen.text.clone()
            } else {
                trimmed.to_owned()
            }
        })
        .collect()
}

/// The rows of `rows` with each text that is on exactly one of them.
fn unique(rows: &[String]) -> HashMap<&str, usize> {
    let mut found: HashMap<&str, Option<usize>> = HashMap::new();
    for (row, text) in rows.iter().enumerate() {
        if !text.is_empty() {
            found
                .entry(text)
                .and_modify(|at| *at = None)
                .or_insert(Some(row));
        }
    }
    found
        .into_iter()
        .filter_map(|(text, row)| row.map(|row| (text, row)))
        .collect()
}

impl Reader for Truth {
    fn query(&self) -> Query {
        Whole.query()
    }

    fn read(&mut self, _read: &Result<Snapshot, Refused>) -> Reply {
        let read = &self.reads[self.at];
        let mut spoken = Vec::new();
        let mut lines = Vec::new();
        let mut first = 0;
        for (at, seen) in read.main.iter().enumerate() {
            if seen.wraps && at + 1 < read.main.len() {
                continue;
            }
            let rows = &read.main[first..=at];
            let text: String = rows.iter().map(|seen| seen.text.as_str()).collect();
            // A line is new or not by its text after the rows of only
            // spaces at its start.
            let blank = rows
                .iter()
                .take_while(|seen| seen.text.trim_end().is_empty())
                .map(|seen| seen.text.len())
                .sum::<usize>();
            let text = text.trim_end().to_owned();
            let key = text.get(blank..).unwrap_or_default().to_owned();
            lines.push((first, text, key));
            first = at + 1;
        }
        for (row, text, key) in &lines {
            let cut = *row == 0 && read.continued;
            let shown = self.shown.contains(key)
                || (cut && self.shown.iter().any(|shown| shown.ends_with(key.as_str())));
            if !text.is_empty() && !shown {
                spoken.push(Spoken::Line {
                    row: *row,
                    text: text.clone(),
                });
            }
        }
        self.shown.extend(lines.into_iter().map(|(_, _, key)| key));
        let rows = texts(&read.rows);
        let previous = self
            .at
            .checked_sub(1)
            .map(|previous| texts(&self.reads[previous].rows));
        let now = unique(&rows);
        let moved = previous.map(|previous| {
            let mut moved = vec![None; previous.len()];
            for (text, row) in unique(&previous) {
                moved[row] = now.get(text).copied();
            }
            moved
        });
        let review = match self.review.as_deref().and_then(|text| now.get(text)) {
            Some(&row) => Some(row),
            None => (!rows.is_empty()).then_some(0),
        };
        self.review = review.map(|row| rows[row].clone());
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
        shown: HashSet::new(),
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

/// A row written and then erased and rewritten after a carriage return,
/// read after each write.
fn rewritten(texts: &[&'static str]) -> Stream {
    let steps: Vec<(Op, bool)> = texts
        .iter()
        .enumerate()
        .flat_map(|(index, &text)| {
            let rewrite =
                (index > 0).then_some([(Op::CarriageReturn, false), (Op::EraseLine(2), false)]);
            rewrite
                .into_iter()
                .flatten()
                .chain([(Op::Write(text), true)])
        })
        .collect();
    stream(&steps)
}

fn line(row: usize, text: &str) -> Spoken {
    Spoken::Line {
        row,
        text: text.to_owned(),
    }
}

fn growth(row: usize, text: &str) -> Spoken {
    Spoken::Growth {
        row,
        text: text.to_owned(),
    }
}

fn redrawn(row: usize, text: &str) -> Spoken {
    Spoken::Redrawn {
        row,
        text: text.to_owned(),
    }
}

fn says(spoken: Vec<Spoken>) -> Heard {
    Heard {
        spoken,
        ..Heard::default()
    }
}

fn holds(spoken: Vec<Spoken>, withheld: &[(usize, Reason)]) -> Heard {
    Heard {
        spoken,
        withheld: withheld
            .iter()
            .map(|&(row, reason)| Withheld { row, reason })
            .collect(),
        ..Heard::default()
    }
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
    // The first frame is new; the frame redrawn on the same rows is not,
    // nor the frame drawn a row lower below the streamed line, which is
    // new; the frame drawn again after a read found it erased is new only
    // where its cost changed.
    assert_eq!(
        violations_in(&corpus_stream("ink"), corpus_size(), &mut Silent),
        [
            missed(1, 0, "------"),
            missed(1, 1, "> input"),
            missed(1, 2, "cost $1"),
            missed(3, 0, "streamed line"),
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
fn a_spinner_claimed_for_a_counters_tick_is_caught() {
    assert_eq!(
        violations(
            &rewritten(&["5s", "6s"]),
            &mut ClaimsEveryChange::new(Reason::Spinner)
        ),
        [false_reason(2, 0, "6s", Reason::Spinner)]
    );
}

#[test]
fn one_vanished_line_justifies_one_spinner_only() {
    let stream = stream(&[
        (Op::Write("- a"), true),
        (Op::MoveTo { row: 1, column: 1 }, false),
        (Op::Write("| a\r\n| a"), true),
    ]);
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "- a")]),
        holds(vec![], &[(0, Reason::Spinner), (1, Reason::Spinner)]),
    ]);
    assert_eq!(
        violations(&stream, &mut reader),
        [false_reason(2, 1, "| a", Reason::Spinner)]
    );
}

#[test]
fn a_spinner_claimed_beside_its_unchanged_line_is_caught() {
    let stream = stream(&[(Op::Write("- a\r\n"), true), (Op::Write("| a"), true)]);
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "- a")]),
        holds(vec![], &[(1, Reason::Spinner)]),
    ]);
    assert_eq!(
        violations(&stream, &mut reader),
        [false_reason(2, 1, "| a", Reason::Spinner)]
    );
}

#[test]
fn a_row_that_only_lost_text_is_accepted_and_one_that_gained_changed_or_lost_only_spaces_is_not() {
    let claims = |texts: &[&'static str]| {
        violations(
            &rewritten(texts),
            &mut ClaimsEveryChange::new(Reason::LostText),
        )
    };
    assert_eq!(claims(&["> foo", "  foo"]), []);
    assert_eq!(
        claims(&["foo", "> foo"]),
        [false_reason(2, 0, "> foo", Reason::LostText)]
    );
    assert_eq!(
        claims(&["abcd", "xy"]),
        [false_reason(2, 0, "xy", Reason::LostText)]
    );
    assert_eq!(
        claims(&["a b", "ab"]),
        [false_reason(2, 0, "ab", Reason::LostText)]
    );
}

/// A row changing once, then held as volatile twice, the first hold
/// resolved by the second's text replacing it; at the quiet read the
/// reader says `settled`.
fn volatile(settled: Heard) -> Vec<Heard> {
    vec![
        Heard::default(),
        says(vec![line(0, "50%")]),
        says(vec![redrawn(0, "60%")]),
        holds(vec![], &[(0, Reason::Volatile)]),
        holds(vec![], &[(0, Reason::Volatile)]),
        settled,
    ]
}

#[test]
fn a_volatile_row_held_until_it_settles_is_accepted() {
    assert_eq!(
        violations(
            &rewritten(&["50%", "60%", "70%", "80%"]),
            &mut Scripted::new(volatile(says(vec![redrawn(0, "80%")])))
        ),
        []
    );
}

#[test]
fn a_volatile_hold_never_resolved_or_resolved_only_by_naming_the_row_is_caught() {
    let stream = rewritten(&["50%", "60%", "70%", "80%"]);
    let unresolved = [false_reason(4, 0, "80%", Reason::Volatile)];
    assert_eq!(
        violations(&stream, &mut Scripted::new(volatile(Heard::default()))),
        unresolved
    );
    assert_eq!(
        violations(
            &stream,
            &mut Scripted::new(volatile(says(vec![redrawn(0, "%")])))
        ),
        unresolved
    );
}

#[test]
fn a_volatile_row_whose_text_had_not_changed_before_is_caught() {
    // Held at its first change, and replaced and spoken at the next.
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "50%")]),
        holds(vec![], &[(0, Reason::Volatile)]),
        says(vec![redrawn(0, "70%")]),
    ]);
    assert_eq!(
        violations(&rewritten(&["50%", "60%", "70%"]), &mut reader),
        [false_reason(2, 0, "60%", Reason::Volatile)]
    );
    // The row changed before, but was blank at the last read, so its text
    // did not change now.
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "50%")]),
        says(vec![redrawn(0, "60%")]),
        Heard::default(),
        holds(vec![], &[(0, Reason::Volatile)]),
    ]);
    assert_eq!(
        violations(&rewritten(&["50%", "60%", "", "70%"]), &mut reader),
        [false_reason(4, 0, "70%", Reason::Volatile)]
    );
}

#[test]
fn a_held_row_that_leaves_the_buffer_is_not_resolved() {
    let stream = stream(&[
        (Op::Write("50%"), true),
        (Op::CarriageReturn, false),
        (Op::Write("60%"), true),
        (Op::CarriageReturn, false),
        (Op::Write("70%"), true),
        (Op::Newline, false),
        (
            Op::Flood {
                lines: 12,
                word: "b",
            },
            true,
        ),
    ]);
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![line(0, "50%")]),
            says(vec![redrawn(0, "60%")]),
            holds(vec![], &[(0, Reason::Volatile)]),
            says(vec![Spoken::Summary { rows: 0..10 }]),
        ])
    };
    assert_eq!(
        violations(&stream, &mut reader),
        [false_reason(3, 0, "70%", Reason::Volatile)]
    );
}

#[test]
fn a_volatile_row_held_and_said_at_the_same_read_needs_no_later_report() {
    // As a reader says a held row at once when it has scrolled above the
    // screen.
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "50%")]),
        says(vec![redrawn(0, "60%")]),
        holds(vec![redrawn(0, "70%")], &[(0, Reason::Volatile)]),
    ]);
    assert_eq!(
        violations(&rewritten(&["50%", "60%", "70%"]), &mut reader),
        []
    );
}

/// `lines` lines of "a" and then `then`, which fills the buffer's rows
/// and ends with a newline and "b" that rotates the history by one row.
fn rotated_after(lines: u8, then: &[&[(Op, bool)]]) -> Stream {
    let mut steps = vec![(Op::Flood { lines, word: "a" }, true)];
    steps.extend(then.iter().flat_map(|step| step.iter().copied()));
    stream(&steps)
}

#[test]
fn a_volatile_row_rewritten_as_the_full_history_rotates_is_followed_up() {
    // "50%" changes to "60%", then to "70%" just before a newline rotates
    // the history, which moves the row up from row 10 to row 9.
    let stream = rotated_after(
        10,
        &[
            &[(Op::Write("50%"), true)],
            &[
                (Op::CarriageReturn, false),
                (Op::EraseLine(2), false),
                (Op::Write("60%"), true),
            ],
            &[
                (Op::CarriageReturn, false),
                (Op::EraseLine(2), false),
                (Op::Write("70%"), false),
                (Op::Newline, false),
                (Op::Write("b"), true),
            ],
        ],
    );
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![Spoken::Summary { rows: 0..10 }]),
            says(vec![line(10, "50%")]),
            says(vec![redrawn(10, "60%")]),
            holds(vec![line(10, "b")], &[(9, Reason::Volatile)]),
            says(vec![redrawn(9, "70%")]),
        ])
    };
    assert_eq!(violations(&stream, &mut reader), []);
}

#[test]
fn a_volatile_row_moved_up_by_the_rotation_to_where_another_row_changed_is_caught() {
    // "50%" above "x" changes to "60%"; then "x" changes to "y" as a
    // newline rotates the history, which moves "y" up to the row "60%"
    // had, while "60%" moves up too: the row of "y" never changed before.
    let stream = rotated_after(
        9,
        &[
            &[
                (Op::Write("50%"), false),
                (Op::Newline, false),
                (Op::Write("x"), true),
            ],
            &[
                (Op::MoveRows(-1), false),
                (Op::CarriageReturn, false),
                (Op::EraseLine(2), false),
                (Op::Write("60%"), true),
            ],
            &[
                (Op::MoveRows(1), false),
                (Op::CarriageReturn, false),
                (Op::EraseLine(2), false),
                (Op::Write("y"), false),
                (Op::Newline, false),
                (Op::Write("b"), true),
            ],
        ],
    );
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![Spoken::Summary { rows: 0..9 }]),
            says(vec![line(9, "50%"), line(10, "x")]),
            says(vec![redrawn(9, "60%")]),
            holds(vec![line(10, "b")], &[(9, Reason::Volatile)]),
            says(vec![redrawn(9, "y")]),
        ])
    };
    assert_eq!(
        violations(&stream, &mut reader),
        [false_reason(4, 9, "y", Reason::Volatile)]
    );
}

#[test]
fn a_volatile_row_rewritten_as_the_history_grows_is_followed() {
    // "50%" on the screen's last row changes to "60%", then to "70%" just
    // before a newline grows the history, which moves the screen down a
    // row while the row keeps its coordinate.
    let stream = stream(&[
        (Op::Write("a\r\nb\r\nc\r\nd\r\n50%"), true),
        (Op::CarriageReturn, false),
        (Op::EraseLine(2), false),
        (Op::Write("60%"), true),
        (Op::CarriageReturn, false),
        (Op::EraseLine(2), false),
        (Op::Write("70%"), false),
        (Op::Newline, false),
        (Op::Write("e"), true),
    ]);
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![
                line(0, "a"),
                line(1, "b"),
                line(2, "c"),
                line(3, "d"),
                line(4, "50%"),
            ]),
            says(vec![redrawn(4, "60%")]),
            holds(vec![line(5, "e")], &[(4, Reason::Volatile)]),
            says(vec![redrawn(4, "70%")]),
        ])
    };
    assert_eq!(violations(&stream, &mut reader), []);
}

#[test]
fn a_volatile_row_whose_place_another_row_changed_as_the_history_grows_is_caught() {
    // "50%" above "x" changes to "60%"; then "x" changes to "y" as a
    // newline grows the history, which brings "y" to the place on the
    // screen "60%" had: the row of "y" never changed before.
    let stream = stream(&[
        (Op::Write("a\r\nb\r\nc\r\n50%\r\nx"), true),
        (Op::MoveRows(-1), false),
        (Op::CarriageReturn, false),
        (Op::EraseLine(2), false),
        (Op::Write("60%"), true),
        (Op::MoveRows(1), false),
        (Op::CarriageReturn, false),
        (Op::EraseLine(2), false),
        (Op::Write("y"), false),
        (Op::Newline, false),
        (Op::Write("e"), true),
    ]);
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![
                line(0, "a"),
                line(1, "b"),
                line(2, "c"),
                line(3, "50%"),
                line(4, "x"),
            ]),
            says(vec![redrawn(3, "60%")]),
            holds(vec![line(5, "e")], &[(4, Reason::Volatile)]),
            says(vec![redrawn(4, "y")]),
        ])
    };
    assert_eq!(
        violations(&stream, &mut reader),
        [false_reason(3, 4, "y", Reason::Volatile)]
    );
}

#[test]
fn a_row_that_lost_text_as_the_full_history_rotates_is_followed_up() {
    // "> foo" loses its marker just before a newline rotates the history,
    // which moves it up from row 9 to row 8, where "y" was; its old text
    // is still on the screen as often, so no line vanished.
    let stream = rotated_after(
        7,
        &[
            &[
                (Op::Write("> foo"), false),
                (Op::Newline, false),
                (Op::Write("y"), false),
                (Op::Newline, false),
                (Op::Write("> foo"), false),
                (Op::Newline, false),
                (Op::Write("x"), true),
            ],
            &[
                (Op::MoveRows(-1), false),
                (Op::CarriageReturn, false),
                (Op::EraseLine(2), false),
                (Op::Write("  foo"), false),
                (Op::MoveRows(1), false),
                (Op::Newline, false),
                (Op::Write("> foo"), true),
            ],
        ],
    );
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![Spoken::Summary { rows: 0..7 }]),
            says(vec![
                line(7, "> foo"),
                line(8, "y"),
                line(9, "> foo"),
                line(10, "x"),
            ]),
            holds(vec![], &[(8, Reason::LostText)]),
        ])
    };
    assert_eq!(violations(&stream, &mut reader), []);
}

#[test]
fn lost_text_claimed_for_a_row_the_rotation_moved_up_to_where_a_longer_row_was_is_caught() {
    // "x" below "> foo" changes to "foo" as a newline rotates the history,
    // which moves "foo" up to the row "> foo" had: its own row did not
    // lose text.
    let stream = rotated_after(
        9,
        &[
            &[
                (Op::Write("> foo"), false),
                (Op::Newline, false),
                (Op::Write("x"), true),
            ],
            &[
                (Op::CarriageReturn, false),
                (Op::EraseLine(2), false),
                (Op::Write("foo"), false),
                (Op::Newline, false),
                (Op::Write("b"), true),
            ],
        ],
    );
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![Spoken::Summary { rows: 0..9 }]),
            says(vec![line(9, "> foo"), line(10, "x")]),
            holds(vec![line(10, "b")], &[(9, Reason::LostText)]),
        ])
    };
    assert_eq!(
        violations(&stream, &mut reader),
        [false_reason(3, 9, "foo", Reason::LostText)]
    );
}

#[test]
fn a_line_carried_through_the_full_historys_rotation_grows_from_what_was_said_of_it() {
    // "hel" grows to "hello wor", its last word held, as "hello wor" is
    // written whole on the row above; a newline then rotates the history,
    // moving both up a row, and the growth that completes the held line is
    // judged against what was said of that line, not of the line that
    // moved into its place.
    let stream = rotated_after(
        9,
        &[
            &[(Op::Newline, false), (Op::Write("hel"), true)],
            &[
                (Op::Write("lo wor"), false),
                (Op::MoveRows(-1), false),
                (Op::CarriageReturn, false),
                (Op::Write("hello wor"), true),
            ],
            &[
                (Op::MoveRows(1), false),
                (Op::Newline, false),
                (Op::Write("b"), true),
            ],
        ],
    );
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![Spoken::Summary { rows: 0..9 }]),
            says(vec![line(10, "hel")]),
            holds(
                vec![line(9, "hello wor"), growth(10, "lo ")],
                &[(10, Reason::UnfinishedWord)],
            ),
            says(vec![line(10, "b"), growth(9, "wor")]),
        ])
    };
    assert_eq!(violations(&stream, &mut reader), []);
}

#[test]
fn growth_judged_against_a_line_the_rotation_moved_into_its_place_is_caught() {
    // "hel" grows to "hello wor", its last word held, as "hello wor" is
    // written whole on the row below; a newline then rotates the history,
    // moving the whole line up to the held line's place. Growth said of
    // the whole line says nothing new, so the hold is never resolved.
    let stream = rotated_after(
        9,
        &[
            &[(Op::Write("hel"), true)],
            &[
                (Op::Write("lo wor"), false),
                (Op::Newline, false),
                (Op::Write("hello wor"), true),
            ],
            &[(Op::Newline, false), (Op::Write("b"), true)],
        ],
    );
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![Spoken::Summary { rows: 0..9 }]),
            says(vec![line(9, "hel")]),
            holds(
                vec![growth(9, "lo "), line(10, "hello wor")],
                &[(9, Reason::UnfinishedWord)],
            ),
            says(vec![line(10, "b"), growth(9, "wor")]),
        ])
    };
    assert_eq!(
        violations(&stream, &mut reader),
        [false_reason(3, 9, "hello wor", Reason::UnfinishedWord)]
    );
}

/// "50%", "60%" and "70%" written over each other, then `then`.
fn held_then(then: Op) -> Stream {
    let rewrite = |text| {
        [
            (Op::CarriageReturn, false),
            (Op::EraseLine(2), false),
            (Op::Write(text), true),
        ]
    };
    let mut steps = vec![(Op::Write("50%"), true)];
    steps.extend(rewrite("60%"));
    steps.extend(rewrite("70%"));
    steps.push((then, true));
    stream(&steps)
}

/// The reader for [`held_then`]: "70%" held as volatile, then silent.
fn holding_seventy() -> Scripted {
    Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "50%")]),
        says(vec![redrawn(0, "60%")]),
        holds(vec![], &[(0, Reason::Volatile)]),
        Heard::default(),
        Heard::default(),
    ])
}

#[test]
fn a_held_row_deleted_is_resolved_by_its_text_going() {
    assert_eq!(
        violations(&held_then(Op::DeleteRows(1)), &mut holding_seventy()),
        []
    );
}

#[test]
fn a_held_row_the_alternate_screen_covers_is_resolved() {
    assert_eq!(
        violations(&held_then(Op::EnterAlternate), &mut holding_seventy()),
        []
    );
}

#[test]
fn a_line_the_alternate_screen_showed_is_not_new_on_the_main_screen() {
    // With no history on the main screen no read shows the switch, so a
    // reader cannot tell the screens apart (section 5.8).
    let shown_there = |main: &'static str| {
        stream(&[
            (Op::EnterAlternate, true),
            (Op::Write("ab"), true),
            (Op::LeaveAlternate, true),
            (Op::Write(main), true),
        ])
    };
    assert_eq!(violations(&shown_there("ab"), &mut Silent), []);
    assert_eq!(
        violations(&shown_there("cd"), &mut Silent),
        [missed(4, 0, "cd")]
    );
}

#[test]
fn a_line_growing_from_what_the_alternate_screen_showed_is_reported_by_its_growth() {
    // With no history on the main screen no read shows the switch, so the
    // main screen is judged against what the alternate screen showed.
    let stream = stream(&[
        (Op::EnterAlternate, false),
        (Op::Write("ab"), true),
        (Op::LeaveAlternate, false),
        (Op::Write("abc"), true),
    ]);
    let script =
        |grown: Heard| Scripted::new(vec![Heard::default(), says(vec![line(0, "ab")]), grown]);
    assert_eq!(
        violations(&stream, &mut script(says(vec![growth(0, "c")]))),
        []
    );
    assert_eq!(
        violations(&stream, &mut script(Heard::default())),
        [missed(2, 0, "abc")]
    );
}

/// The lines of a flood of `count` lines of "cat" at rows from `from`, each
/// spoken.
fn cats(from: usize, count: usize) -> Vec<Spoken> {
    (1..=count)
        .map(|number| line(from + number - 1, &format!("cat{number}")))
        .collect()
}

#[test]
fn a_line_printed_again_after_its_text_left_the_full_history_may_be_reported() {
    // "dog" leaves the small terminal's full buffer of eleven rows as the
    // flood rotates it, and is printed again on the last row.
    let stream = stream(&[
        (Op::Write("dog"), false),
        (Op::Newline, true),
        (
            Op::Flood {
                lines: 10,
                word: "cat",
            },
            false,
        ),
        (Op::Write("dog"), true),
    ]);
    let mut again = cats(0, 10);
    again.push(line(10, "dog"));
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![line(0, "dog")]),
            says(again),
        ])
    };
    assert_eq!(violations(&stream, &mut reader), []);
}

#[test]
fn a_held_line_spoken_as_last_read_after_it_left_the_full_history_is_resolved() {
    // "abc" grows from "ab" with its word held, and the flood rotates it
    // out of the small terminal's full buffer before the next read.
    let stream = stream(&[
        (Op::Write("ab"), true),
        (Op::Write("c"), true),
        (Op::Newline, false),
        (
            Op::Flood {
                lines: 10,
                word: "cat",
            },
            true,
        ),
    ]);
    let script = |held: &str| {
        let mut flood = vec![Spoken::Held {
            text: held.to_owned(),
        }];
        flood.extend(cats(0, 10));
        Scripted {
            whole: true,
            ..Scripted::new(vec![
                Heard::default(),
                says(vec![line(0, "ab")]),
                holds(vec![], &[(0, Reason::UnfinishedWord)]),
                says(flood),
            ])
        }
    };
    assert_eq!(violations(&stream, &mut script("c")), []);
    assert_eq!(
        violations(&stream, &mut script("xyz")),
        [
            Violation::Unheld {
                read: 3,
                said: "xyz".to_owned()
            },
            false_reason(2, 0, "abc", Reason::UnfinishedWord)
        ]
    );
}

#[test]
fn the_reader_holding_every_line_of_a_flood_as_volatile_is_caught() {
    let mut expected: Vec<Violation> = (1..=10)
        .map(|line| false_reason(1, line - 1, &format!("a{line}"), Reason::Volatile))
        .collect();
    expected.extend(
        (1..=8).map(|line| false_reason(2, line + 1, &format!("b{line}"), Reason::Volatile)),
    );
    assert_eq!(violations(&flood(), &mut HoldsEveryLine), expected);
}

/// A line that grows to "hello wor" in two reads.
fn growing() -> Vec<(Op, bool)> {
    vec![(Op::Write("hel"), true), (Op::Write("lo wor"), true)]
}

#[test]
fn an_unfinished_word_held_until_the_line_grows_past_it_is_accepted() {
    let mut steps = growing();
    steps.push((Op::Write("ld"), true));
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "hel")]),
        holds(vec![growth(0, "lo ")], &[(0, Reason::UnfinishedWord)]),
        says(vec![growth(0, "world")]),
    ]);
    assert_eq!(violations(&stream(&steps), &mut reader), []);
}

#[test]
fn an_unfinished_word_spoken_after_the_quiet_period_is_accepted_and_one_never_spoken_is_caught() {
    let script = |quiet: Heard| {
        Scripted::new(vec![
            Heard::default(),
            says(vec![line(0, "hel")]),
            holds(vec![growth(0, "lo ")], &[(0, Reason::UnfinishedWord)]),
            quiet,
        ])
    };
    assert_eq!(
        violations(
            &stream(&growing()),
            &mut script(says(vec![growth(0, "wor")]))
        ),
        []
    );
    assert_eq!(
        violations(&stream(&growing()), &mut script(Heard::default())),
        [false_reason(2, 0, "hello wor", Reason::UnfinishedWord)]
    );
}

#[test]
fn an_unfinished_word_claimed_for_a_line_that_did_not_grow_is_caught() {
    let mut reader = Scripted::new(vec![
        Heard::default(),
        holds(vec![], &[(0, Reason::UnfinishedWord)]),
    ]);
    assert_eq!(
        violations(&stream(&[(Op::Write("cat"), true)]), &mut reader),
        [false_reason(1, 0, "cat", Reason::UnfinishedWord)]
    );
}

#[test]
fn an_unfinished_word_claimed_beside_the_unchanged_line_it_extends_is_caught() {
    let stream = stream(&[(Op::Write("ab\r\n"), true), (Op::Write("abc"), true)]);
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "ab")]),
        holds(vec![], &[(1, Reason::UnfinishedWord)]),
    ]);
    assert_eq!(
        violations(&stream, &mut reader),
        [false_reason(2, 1, "abc", Reason::UnfinishedWord)]
    );
}

#[test]
fn one_line_that_grew_justifies_one_unfinished_word_only() {
    let stream = stream(&[
        (Op::Write("ab"), true),
        (Op::CarriageReturn, false),
        (Op::Write("abc\r\nabd"), true),
    ]);
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "ab")]),
        holds(
            vec![],
            &[(0, Reason::UnfinishedWord), (1, Reason::UnfinishedWord)],
        ),
        says(vec![line(0, "abc")]),
    ]);
    assert_eq!(
        violations(&stream, &mut reader),
        [false_reason(2, 1, "abd", Reason::UnfinishedWord)]
    );
}

#[test]
fn reasons_that_need_a_key_are_caught() {
    for reason in [Reason::HeldForKeys, Reason::HeldTyping, Reason::KeyAnswer] {
        assert_eq!(
            violations(
                &rewritten(&["cat", "dog"]),
                &mut ClaimsEveryChange::new(reason)
            ),
            [false_reason(2, 0, "dog", reason)],
            "{reason:?}"
        );
    }
}

#[test]
fn a_redrawn_rows_speech_saying_what_changed_is_accepted() {
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "Working 1s")]),
        says(vec![redrawn(0, "2s")]),
    ]);
    assert_eq!(
        violations(&rewritten(&["Working 1s", "Working 2s"]), &mut reader),
        []
    );
}

#[test]
fn a_redrawn_rows_speech_of_one_letter_of_a_changed_word_is_caught() {
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "cat")]),
        says(vec![redrawn(0, "d")]),
    ]);
    assert_eq!(
        violations(&rewritten(&["cat", "dog"]), &mut reader),
        [missed(2, 0, "dog")]
    );
}

#[test]
fn a_redrawn_rows_speech_for_a_row_that_had_no_text_is_caught() {
    let mut reader = Scripted::new(vec![Heard::default(), says(vec![redrawn(0, "cat")])]);
    assert_eq!(
        violations(&stream(&[(Op::Write("cat"), true)]), &mut reader),
        [missed(1, 0, "cat")]
    );
}

#[test]
fn a_row_changed_back_and_spoken_again_is_not_repeated() {
    assert_eq!(
        violations(
            &rewritten(&["cat", "dog", "cat"]),
            &mut SameCoordinate {
                previous: Vec::new()
            }
        ),
        []
    );
}

#[test]
fn a_line_repeating_a_text_on_the_screen_may_be_reported_once() {
    let stream = stream(&[(Op::Write("ok\r\n"), true), (Op::Write("ok\r\n"), true)]);
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "ok")]),
        says(vec![line(1, "ok"), line(1, "ok")]),
    ]);
    assert_eq!(
        violations(&stream, &mut reader),
        [Violation::Repeated {
            read: 2,
            row: 1,
            text: "ok".to_owned()
        }]
    );
}

#[test]
fn a_line_starting_above_the_read_is_reported_from_the_reads_first_row() {
    let long = "x".repeat(70);
    let stream = Stream {
        about: "a line of 70 characters, longer than the screen".to_owned(),
        chunks: vec![long.clone()],
        at_ms: Vec::new(),
    };
    // The screen holds the line's last 58 characters.
    let mut reader = Scripted::new(vec![Heard::default(), says(vec![line(0, &long[12..])])]);
    assert_eq!(violations(&stream, &mut reader), []);
    assert_eq!(violations(&stream, &mut Silent), [missed(1, 0, &long)]);
}

#[test]
fn the_parts_of_a_line_split_in_two_only_lost_text() {
    // A row inserted below a wrapped line's first row, in Windows
    // Terminal, which does not carry the wrap onto it: "wrapping-wor" keeps
    // its place, "d" moves down a row, and the line they made is gone.
    let stream = stream(&[
        (Op::Write("wrapping-word"), true),
        (Op::InsertRows(1), true),
    ]);
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "wrapping-word")]),
        holds(vec![], &[(0, Reason::LostText), (2, Reason::LostText)]),
    ]);
    let mut terminal = Size::SMALL.terminal(Flavor::WindowsTerminal);
    assert_eq!(
        observe(&mut terminal, &stream, &mut reader).violations(),
        []
    );
}

#[test]
fn a_row_that_lost_text_while_its_old_text_is_shown_elsewhere_only_lost_text() {
    let stream = stream(&[
        (Op::Write("> foo\r\n> foo\r\n"), true),
        (Op::MoveTo { row: 1, column: 1 }, false),
        (Op::Write("  foo"), false),
        (Op::MoveTo { row: 3, column: 1 }, false),
        (Op::Write("> foo"), true),
    ]);
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "> foo"), line(1, "> foo")]),
        holds(vec![], &[(0, Reason::LostText)]),
    ]);
    assert_eq!(violations(&stream, &mut reader), []);
}

#[test]
fn a_line_that_lost_text_grows_from_its_shorter_text() {
    let stream = stream(&[
        (Op::Write("hello world!x"), true),
        (Op::CarriageReturn, false),
        (Op::EraseLine(0), true),
        (Op::Write("yz"), true),
    ]);
    let mut reader = Scripted::new(vec![
        Heard::default(),
        says(vec![line(0, "hello world!x")]),
        holds(vec![], &[(0, Reason::LostText)]),
        says(vec![growth(0, "yz")]),
    ]);
    assert_eq!(violations(&stream, &mut reader), []);
}

#[test]
fn a_line_joined_to_a_row_of_spaces_above_it_is_not_new() {
    // The console host marks a row filled to its last column as wrapping,
    // spaces included, so the row of spaces joins the line below it.
    let stream = Stream {
        about: "a line, then a row of spaces written above it".to_owned(),
        chunks: vec!["\r\nab".to_owned(), format!("\x1b[H{}", " ".repeat(12))],
        at_ms: Vec::new(),
    };
    let mut reader = Scripted::new(vec![Heard::default(), says(vec![line(1, "ab")])]);
    assert_eq!(violations(&stream, &mut reader), []);
}

/// A line of 14 characters on the tiny terminal, which fills its buffer,
/// then `more`.
fn rotated(more: &str) -> Stream {
    Stream {
        about: format!("a line filling the tiny buffer, then {more:?}"),
        chunks: vec!["cdefgcdefgcdef".to_owned(), more.to_owned()],
        at_ms: Vec::new(),
    }
}

#[test]
fn a_line_whose_start_left_the_history_is_not_new() {
    let mut reader = Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![line(0, "cdefgcdefgcdef")]),
        ])
    };
    assert_eq!(violations_in(&rotated("\r\n"), Size::TINY, &mut reader), []);
}

#[test]
fn a_line_whose_start_left_the_history_grows_from_what_is_left() {
    let script = |grown: Heard| Scripted {
        whole: true,
        ..Scripted::new(vec![
            Heard::default(),
            says(vec![line(0, "cdefgcdefgcdef")]),
            grown,
        ])
    };
    assert_eq!(
        violations_in(
            &rotated("ghij"),
            Size::TINY,
            &mut script(says(vec![growth(0, "ghij")]))
        ),
        []
    );
    assert_eq!(
        violations_in(&rotated("ghij"), Size::TINY, &mut script(Heard::default())),
        [missed(2, 0, "gcdefgcdefghij")]
    );
}

#[test]
fn a_review_position_is_not_judged_where_the_history_was_erased() {
    // Erasing the history looks like the alternate screen opening, where
    // the review position goes to the caret (section 5.8).
    let lines = "a\r\nb\r\nc\r\nd\r\ne\r\nf";
    let reviewed = |second: Op| {
        let mut reader = Scripted::new(vec![
            Heard::default(),
            Heard {
                review: Some(1),
                ..says(vec![
                    line(0, "b"),
                    line(1, "c"),
                    line(2, "d"),
                    line(3, "e"),
                    line(4, "f"),
                ])
            },
            Heard {
                review: Some(4),
                ..Heard::default()
            },
            Heard {
                review: Some(4),
                ..Heard::default()
            },
        ]);
        violations(
            &stream(&[(Op::Write(lines), true), (second, true)]),
            &mut reader,
        )
    };
    assert_eq!(reviewed(Op::ClearHistory), [missed(1, 0, "a")]);
    assert_eq!(
        reviewed(Op::Write("")),
        [
            missed(1, 0, "a"),
            Violation::ReviewMoved {
                read: 2,
                went: 1,
                said: Some(4)
            }
        ]
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
