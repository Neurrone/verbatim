//! The anchored diff on simulated terminal text: a buffer of padded rows,
//! an anchor that keeps its row while the text moves beneath it, and the
//! same reads the remote program makes, run through [`read_new`].

use super::*;

/// The columns every simulated row is padded to.
const WIDTH: usize = 20;

/// How many of the newest lines a read takes.
const WANTED: u32 = 5;

/// How far up the simulated search looks.
const SEARCH: usize = 4;

/// A terminal's text: rows padded to the width, an anchor row, a
/// scrollback that keeps at most `capacity` rows, and whether the anchor
/// still compares with the text (it does not after a switch of screens).
struct Sim {
    rows: Vec<String>,
    capacity: usize,
    anchor: Option<usize>,
    comparable: bool,
}

fn padded(text: &str) -> String {
    format!("{text:<WIDTH$}\r\n")
}

impl Sim {
    fn new(capacity: usize, rows: &[&str]) -> Self {
        let mut sim = Self {
            rows: Vec::new(),
            capacity,
            anchor: None,
            comparable: true,
        };
        sim.push(rows);
        sim
    }

    /// Writes rows at the end, discarding the oldest past the capacity.
    fn push(&mut self, rows: &[&str]) {
        self.rows.extend(rows.iter().map(|row| padded(row)));
        let excess = self.rows.len().saturating_sub(self.capacity);
        self.rows.drain(..excess);
    }

    /// Rewrites the last row in place.
    fn rewrite_last(&mut self, text: &str) {
        if let Some(last) = self.rows.last_mut() {
            *last = padded(text);
        }
    }

    /// Replaces every row, as clearing the screen or switching screens does.
    fn replace(&mut self, rows: &[&str], comparable: bool) {
        self.rows = rows.iter().map(|row| padded(row)).collect();
        self.comparable = comparable;
    }

    fn row(&self, index: usize) -> String {
        self.rows.get(index).cloned().unwrap_or_default()
    }

    /// The reads after the line at `from`: the count to the last line, the
    /// last lines, and the line above them; the anchor moves to the last.
    fn tail(&mut self, from: usize, count_from: bool, wanted: u32) -> (u32, Vec<String>, String) {
        let rows = self.rows.len();
        let last = rows.saturating_sub(1);
        let count = if count_from {
            last.saturating_sub(from)
        } else {
            rows
        };
        let reading = count.min(usize::try_from(wanted).unwrap_or(usize::MAX));
        let first = rows - reading;
        let lines = self.rows[first..].to_vec();
        let above = if reading > 0 && first > 0 {
            self.row(first - 1)
        } else {
            String::new()
        };
        self.anchor = Some(last);
        self.comparable = true;
        (u32::try_from(count).unwrap_or(u32::MAX), lines, above)
    }
}

impl TailSource for Sim {
    type Error = ();

    fn anchored(&mut self, memory: &Memory, wanted: u32) -> Result<Option<TailText>, ()> {
        let Some(anchor) = self
            .anchor
            .filter(|&row| self.comparable && row < self.rows.len())
        else {
            return Ok(None);
        };
        let line = self.row(anchor);
        let previous = anchor
            .checked_sub(1)
            .map(|row| self.row(row))
            .unwrap_or_default();
        let (found, at) = if previous == memory.previous {
            (Found::AtAnchor, anchor)
        } else {
            (1..=SEARCH)
                .filter_map(|shift| anchor.checked_sub(shift).map(|row| (shift, row)))
                .find(|&(_, row)| {
                    let above = row
                        .checked_sub(1)
                        .map(|row| self.row(row))
                        .unwrap_or_default();
                    self.row(row) == memory.line && above == memory.previous
                })
                .map_or((Found::NotFound, anchor), |(shift, row)| {
                    (Found::Moved(u32::try_from(shift).unwrap_or(0)), row)
                })
        };
        let (count, lines, above) = self.tail(at, true, wanted);
        Ok(Some(TailText {
            found,
            line,
            previous,
            count,
            lines,
            above,
        }))
    }

    fn fresh(&mut self, wanted: u32) -> Result<TailText, ()> {
        let (count, lines, above) = self.tail(0, false, wanted);
        Ok(TailText {
            found: Found::Afresh,
            line: String::new(),
            previous: String::new(),
            count,
            lines,
            above,
        })
    }
}

/// A terminal being read, with what the outpost remembers.
struct Reader {
    sim: Sim,
    memory: Option<Memory>,
}

impl Reader {
    /// Starts with a baseline read, which speaks nothing.
    fn new(sim: Sim) -> Self {
        let mut reader = Self { sim, memory: None };
        let output = reader.read_with(true);
        assert!(output.is_empty(), "a baseline speaks nothing: {output:?}");
        reader
    }

    fn read_with(&mut self, baseline: bool) -> TerminalOutput {
        let (output, memory) =
            read_new(&mut self.sim, self.memory.as_ref(), baseline, WANTED).expect("reads");
        self.memory = Some(memory);
        output
    }

    fn read(&mut self) -> TerminalOutput {
        self.read_with(false)
    }
}

fn lines(texts: &[&str]) -> Vec<String> {
    texts.iter().map(|&text| text.to_owned()).collect()
}

#[test]
fn appended_lines_are_read_and_a_prompt_that_grew_speaks_what_it_gained() {
    // The prompt's own trailing space reads as padding: it is uncertain.
    let mut reader = Reader::new(Sim::new(100, &["welcome", "ready> "]));
    reader.sim.rewrite_last("ready> echo hi");
    let output = reader.read();
    assert_eq!(
        output.changed,
        Some(LineChange {
            text: " echo hi".to_owned(),
            line: "ready> echo hi".to_owned(),
            appended: true,
            uncertain: 1,
        })
    );
    assert_eq!(output.lines, Vec::<String>::new());

    reader.sim.push(&["hi", "ready>"]);
    let output = reader.read();
    assert_eq!(output.changed, None);
    assert_eq!(output.skipped, None);
    assert_eq!(output.lines, lines(&["hi", "ready>"]));
}

#[test]
fn more_output_than_the_read_limit_counts_the_rest_as_skipped() {
    let mut reader = Reader::new(Sim::new(100, &["ready>"]));
    let flood: Vec<String> = (1..=12).map(|n| format!("line {n}")).collect();
    let flood: Vec<&str> = flood.iter().map(String::as_str).collect();
    reader.sim.push(&flood);
    let output = reader.read();
    assert_eq!(output.skipped, Some(Skipped::Count(7)));
    assert_eq!(
        output.lines,
        lines(&["line 8", "line 9", "line 10", "line 11", "line 12"])
    );
}

#[test]
fn a_line_rewritten_in_place_speaks_from_the_word_that_changed() {
    let mut reader = Reader::new(Sim::new(100, &["fetching", "progress 10% done"]));
    reader.sim.rewrite_last("progress 50% done");
    let output = reader.read();
    assert_eq!(
        output.changed,
        Some(LineChange {
            text: "50% done".to_owned(),
            line: "progress 50% done".to_owned(),
            appended: false,
            uncertain: 0,
        })
    );
    // Backspace shortens the line: nothing is spoken for it.
    reader.sim.rewrite_last("progress 50%");
    assert!(reader.read().is_empty());
}

#[test]
fn a_redraw_with_the_same_text_speaks_nothing() {
    let mut reader = Reader::new(Sim::new(100, &["a", "b", "ready>"]));
    let rows = reader.sim.rows.clone();
    reader.sim.rows = rows;
    assert!(reader.read().is_empty());
}

#[test]
fn a_full_scrollback_shifting_beneath_the_anchor_is_followed_exactly() {
    let mut reader = Reader::new(Sim::new(6, &["one", "two", "three", "four", "five", "six"]));
    // The scrollback is full: two new rows discard the two oldest, and the
    // anchor's row now holds other text.
    reader.sim.push(&["seven", "eight"]);
    let output = reader.read();
    assert_eq!(output.changed, None);
    assert_eq!(output.skipped, None);
    assert_eq!(output.lines, lines(&["seven", "eight"]));
}

#[test]
fn a_shift_past_the_search_says_lines_were_skipped_without_a_count() {
    let mut reader = Reader::new(Sim::new(8, &["a", "b", "c", "d", "e", "f", "g", "h"]));
    let flood: Vec<String> = (1..=20).map(|n| format!("flood {n}")).collect();
    let flood: Vec<&str> = flood.iter().map(String::as_str).collect();
    reader.sim.push(&flood);
    let output = reader.read();
    assert_eq!(output.skipped, Some(Skipped::Uncounted));
    assert_eq!(
        output.lines,
        lines(&["flood 16", "flood 17", "flood 18", "flood 19", "flood 20"])
    );
}

#[test]
fn a_cleared_screen_speaks_what_is_on_it_now() {
    let mut reader = Reader::new(Sim::new(100, &["old 1", "old 2", "ready> cls"]));
    reader.sim.replace(&["hello", "ready>"], true);
    let output = reader.read();
    assert_eq!(output.skipped, None);
    assert_eq!(output.lines, lines(&["hello", "ready>"]));
}

#[test]
fn the_alternate_screen_is_compared_line_by_line() {
    let mut reader = Reader::new(Sim::new(100, &["ready> vim notes"]));
    // A full-screen program switches screens: the anchor no longer compares
    // with the text, and the new screen is read.
    reader
        .sim
        .replace(&["first line", "~", "~", "notes 1L"], false);
    let output = reader.read();
    assert_eq!(output.skipped, None);
    assert_eq!(output.lines, lines(&["first line", "~", "~", "notes 1L"]));
    // It redraws one row above the anchor: only that row is spoken.
    reader.sim.rows[1] = padded("second line");
    let output = reader.read();
    assert_eq!(output.lines, lines(&["second line"]));
    // Leaving it, the main screen returns.
    reader.sim.replace(&["ready> vim notes", "ready>"], false);
    let output = reader.read();
    assert_eq!(output.lines, lines(&["ready> vim notes", "ready>"]));
}

#[test]
fn padding_of_any_white_space_is_trimmed() {
    assert_eq!(trimmed("total 42      \r\n"), "total 42");
    // An ideographic space pads as well as an ASCII one; inner spaces stay.
    assert_eq!(trimmed("合計 42\u{3000}\u{3000}\n"), "合計 42");
    let long = "é".repeat(MAX_TERMINAL_LINE_BYTES);
    assert!(trimmed(&long).len() <= MAX_TERMINAL_LINE_BYTES);
}

#[test]
fn line_changes_are_worked_out_character_by_character() {
    assert_eq!(line_change("ready>  ", "ready>   \r\n"), None);
    assert_eq!(line_change("ready> ls", "ready> l"), None);
    let grew = line_change("ready> l", "ready> ls").expect("a change");
    assert_eq!((grew.text.as_str(), grew.appended), ("s", true));
    let rewritten = line_change("[###   ] 30%", "[####  ] 40%").expect("a change");
    assert_eq!(
        (rewritten.text.as_str(), rewritten.appended),
        ("[####  ] 40%", false)
    );
}
