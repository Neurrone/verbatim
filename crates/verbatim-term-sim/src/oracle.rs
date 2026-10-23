//! The oracle (`docs/design/terminal-model.md`, section 9): invariants over
//! the simulated terminal's ground truth that any reader of terminals must
//! keep, whatever the stream did, checked over random streams, every short
//! stream of a small alphabet, and the recorded corpus.
//!
//! A reader implements [`Reader`]: it chooses each read's [`Query`], is
//! given what the read answered, and reports what it would say
//! ([`Heard`]). [`observe`] reads a terminal with the reader once before
//! anything is written, as a reader starts watching a terminal, then writes
//! a stream into it and reads it after each chunk, and records each read's
//! rows as they truly are ([`Observation`]); [`check`] holds what the
//! reader said against the lines the terminal printed ([`PrintedLine`])
//! and lists every [`Violation`].
//!
//! The invariants, as checked:
//!
//! - Every line of new output is reported exactly once, as a line, a
//!   redrawn row's speech or in a summary, and in order. A printed line is
//!   found by the first read taken on the main screen at or after the
//!   write that finished it, before the write that first changed it and
//!   before the write in which one of its rows left the buffer, whether or
//!   not that read's text covers it. It is new output when a read found it,
//!   it is not a reprint, and its text is not the text of a line on the
//!   screen of the read before the one that found it (the first read has
//!   none before it); it then stays required whatever happens to it later.
//!   So a redrawn footer, input box or status line is not required, and
//!   output written over the rows that held one is. The lines no read
//!   finds are exempt: those changed in the write that finished them or
//!   before the next read (written and erased, or rewritten, between two
//!   reads), those whose row left a full history or a scroll region in
//!   that time, and those finished and changed while the alternate screen
//!   was up at every read between. The order is the order reads found the
//!   lines, top to bottom among lines one read found, since a reader sees
//!   positions, not the order of writing; a line reported while it was
//!   still being written counts from that report. A line reported whole and
//!   then grown must add up to its text as found. Resizing is not modelled,
//!   so lines written in the same read as a resize are not an exemption
//!   here.
//! - Everything reported is text the read shows: a line is exactly the
//!   line starting at the row it names, padding left out; growth and a
//!   redrawn row's speech are part of that line; and every report, each
//!   row a summary counts included, names the first row of a line of the
//!   read.
//! - For every row with text that is unchanged between two reads, whose
//!   text is on no other row of either read, a reader that tracks movement
//!   maps it to the row it really became.
//! - Padding is never reported: a report that matches the read only with
//!   the spaces after a line's last character is told apart from one that
//!   does not match at all.
//! - A reader with a review position keeps it on the same row: when the
//!   row it was on at one read is among the rows of the next, and the two
//!   reads are on the same screen, it is there. No key is ever pressed and
//!   following is never on in the oracle's streams.
//!
//! What redrawn rows, keys, lists and colours say is decided speech, not a
//! property of the stream, so it is tested by unit tests that state a
//! stream and its speech, never here.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::ops::Range;

use crate::conformance::Stream;
use crate::{Flavor, PrintedLine, Query, Refused, RowId, Snapshot, Terminal};

/// A row of a read as it truly is: its identity, its text as the read
/// gives it (padding included), and whether its line wraps onto the next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// The row's identity.
    pub id: RowId,
    /// Its cells' characters as the read's text gives them.
    pub text: String,
    /// Whether its line continues onto the next row.
    pub wraps: bool,
}

/// Something a reader would say after a read. Each report names the row
/// its line starts on, counted from 0 at the read text's first row, the
/// text split at its line breaks and every width's worth of characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spoken {
    /// A line spoken as new output, as the read shows it, padding left
    /// out. A line still being written is reported once, here, and what it
    /// gains later as [`Spoken::Growth`].
    Line {
        /// The row the line starts on.
        row: usize,
        /// What is spoken.
        text: String,
    },
    /// What a line reported before has gained, spoken as it grows.
    Growth {
        /// The row the line starts on.
        row: usize,
        /// What is spoken.
        text: String,
    },
    /// Speech about a row rewritten in place, such as the words that
    /// changed.
    Redrawn {
        /// The row the line starts on.
        row: usize,
        /// What is spoken.
        text: String,
    },
    /// A flood's summary: the lines starting on `rows` are counted, not
    /// spoken.
    Summary {
        /// The rows whose lines the summary counts.
        rows: Range<usize>,
    },
}

/// What a reader would say after one read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// What it says, in order.
    pub spoken: Vec<Spoken>,
    /// Where it thinks each row of its previous read went: for each of
    /// those rows, the row of this read it became, or `None` for a row it
    /// thinks gone. `None` when the reader tracks no movement.
    pub moved: Option<Vec<Option<usize>>>,
    /// The row of this read its review position is on after the read;
    /// `None` when it has none.
    pub review: Option<usize>,
}

/// A reader's answer to a read.
#[derive(Clone, Debug)]
pub enum Reply {
    /// What it would say.
    Heard(Heard),
    /// Read again at once with this query, before anything more is
    /// written, as a reader reads the whole text when its last screen is
    /// not in the read (`docs/design/terminal-model.md`, section 4.2).
    Again(Query),
}

/// A reader of terminals under test: given each read's snapshot and its
/// own memory, it says what it would speak.
pub trait Reader {
    /// The query for the next read.
    fn query(&self) -> Query;

    /// One read's answer, and what the reader makes of it.
    fn read(&mut self, read: &Result<Snapshot, Refused>) -> Reply;
}

/// One read as the oracle sees it: when it was taken, the rows its text
/// covered as they truly are, the screen and main buffer at that moment,
/// and what the reader said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// How many writes came before it.
    pub writes: u64,
    /// Whether the alternate screen was up.
    pub alternate: bool,
    /// The rows the read's text covered, first to last; empty when the read
    /// was refused.
    pub rows: Vec<Seen>,
    /// The lines with text on the screen shown, top to bottom
    /// ([`crate::Grid::screen_lines`]).
    pub screen: Vec<String>,
    /// The identities of the main buffer's rows, first to last, which
    /// order the lines a read finds.
    pub main: Vec<RowId>,
    /// What the reader said.
    pub heard: Heard,
}

/// A stream written into a terminal and read after each chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// Each read the reader answered with what it heard, in order: the
    /// first before anything was written, then one after each chunk. A read
    /// the reader asked to take again is replaced by the read taken again.
    pub reads: Vec<Observation>,
    /// The lines the terminal printed once the stream was written.
    pub printed: Vec<PrintedLine>,
}

impl Run {
    /// Every violation of the invariants in this run.
    #[must_use]
    pub fn violations(&self) -> Vec<Violation> {
        check(&self.printed, &self.reads)
    }
}

/// Reads `terminal` with `reader` once as it is, as a reader starts
/// watching a terminal, and then writes `stream` into it chunk by chunk,
/// reading it after each chunk.
///
/// # Panics
///
/// When the reader asks to read again twice at one point, which no reader
/// needs.
pub fn observe(terminal: &mut Terminal, stream: &Stream, reader: &mut dyn Reader) -> Run {
    let mut reads = vec![read(terminal, reader)];
    for chunk in &stream.chunks {
        terminal.write(chunk.as_bytes());
        reads.push(read(terminal, reader));
    }
    Run {
        reads,
        printed: terminal.printed().to_vec(),
    }
}

/// One read of `terminal` by `reader`, taken again if the reader asks.
fn read(terminal: &Terminal, reader: &mut dyn Reader) -> Observation {
    let mut query = reader.query();
    let mut again = false;
    loop {
        let (snapshot, rows) = match terminal.observe(&query) {
            Ok((snapshot, rows)) => (Ok(snapshot), rows),
            Err(refused) => (Err(refused), Vec::new()),
        };
        match reader.read(&snapshot) {
            Reply::Heard(heard) => {
                let grid = terminal.grid();
                return Observation {
                    writes: terminal.writes(),
                    alternate: grid.alternate.is_some(),
                    rows,
                    screen: grid.screen_lines(),
                    main: grid.main.rows.iter().map(|row| row.id).collect(),
                    heard,
                };
            }
            Reply::Again(next) => {
                assert!(!again, "the reader asked to read again twice at one point");
                again = true;
                query = next;
            }
        }
    }
}

/// A way a reader broke an invariant. Lines are numbered by their place in
/// the printed lines, reads and rows from 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    /// A line of new output was never reported.
    Missed {
        /// The line.
        line: usize,
        /// Its text.
        text: String,
    },
    /// A line of new output was reported again.
    Repeated {
        /// The line.
        line: usize,
        /// Its text.
        text: String,
        /// The read it was reported again at.
        read: usize,
    },
    /// A line of new output was reported after a line found after it.
    OutOfOrder {
        /// The line.
        line: usize,
        /// Its text.
        text: String,
        /// The read it was reported at.
        read: usize,
    },
    /// What was reported of a line, the line and its growth, does not add
    /// up to the line as found.
    Incomplete {
        /// The line.
        line: usize,
        /// Its text as found.
        text: String,
        /// What was reported of it.
        heard: String,
    },
    /// A report is not the text of the line at the row it names.
    Misquoted {
        /// The read.
        read: usize,
        /// The row it named.
        row: usize,
        /// What it said.
        said: String,
        /// The line there, padding left out.
        shown: String,
    },
    /// A report includes padding.
    Padded {
        /// The read.
        read: usize,
        /// The row it named.
        row: usize,
        /// What it said.
        said: String,
    },
    /// A report, or a row a summary counts, names a row that is not the
    /// first row of a line, or no row of the read.
    NotALine {
        /// The read.
        read: usize,
        /// The row it named.
        row: usize,
    },
    /// A row unchanged between two reads, its text on no other row of
    /// either, was mapped somewhere other than where it went.
    Mismapped {
        /// The read the row moved into.
        read: usize,
        /// The row in the previous read.
        row: usize,
        /// The row it became.
        went: usize,
        /// The row the reader said it became.
        said: Option<usize>,
    },
    /// The review position left the row it was on, though that row is in
    /// the read and the screen did not switch.
    ReviewMoved {
        /// The read.
        read: usize,
        /// The row the review position's row became.
        went: usize,
        /// Where the reader said its review position is.
        said: Option<usize>,
    },
    /// The review position is on no row of the read.
    ReviewOutside {
        /// The read.
        read: usize,
        /// The row the reader named.
        row: usize,
    },
}

/// The line starting at `row`: its rows' text, joined while each wraps
/// onto the next, padding included.
fn line_at(rows: &[Seen], row: usize) -> String {
    let mut text = String::new();
    for seen in &rows[row..] {
        text.push_str(&seen.text);
        if !seen.wraps {
            break;
        }
    }
    text
}

/// Whether `read` shows `line` as it was finished: on the main screen, at
/// or after the write that finished it, before it changed or a row of it
/// left the buffer.
fn present(line: &PrintedLine, read: &Observation) -> bool {
    !read.alternate
        && line.finished <= read.writes
        && line.changed.is_none_or(|changed| changed > read.writes)
        && line.left.is_none_or(|left| left > read.writes)
}

/// The lines reported so far, and the order they were reported in.
struct Tally<'a> {
    printed: &'a [PrintedLine],
    reads: &'a [Observation],
    /// The printed lines by their first row, in the order finished.
    by_first: HashMap<RowId, Vec<usize>>,
    /// Where each line was found: the read, and its first row's place in
    /// that read's main buffer.
    found: Vec<Option<(usize, usize)>>,
    /// Whether each line is new output.
    required: Vec<bool>,
    reported: Vec<bool>,
    /// What was reported of each line reported whole.
    heard: Vec<Option<String>>,
    /// The latest place of a line reported so far: the read and the place
    /// in its main buffer it counts from.
    latest: Option<(usize, usize)>,
    violations: Vec<Violation>,
}

impl Tally<'_> {
    /// The line read `index` shows starting on row `id`.
    fn shown(&self, index: usize, id: RowId) -> Option<usize> {
        let read = &self.reads[index];
        self.by_first
            .get(&id)?
            .iter()
            .copied()
            .find(|&line| present(&self.printed[line], read))
    }

    /// The line still being written that starts on row `id` at read
    /// `index`, where the read shows `shown`: the first finished on that
    /// row after it, when it only grows from what the read shows.
    fn coming(&self, index: usize, id: RowId, shown: &str) -> Option<usize> {
        let writes = self.reads[index].writes;
        self.by_first
            .get(&id)?
            .iter()
            .copied()
            .find(|&line| self.printed[line].finished > writes)
            .filter(|&line| !shown.is_empty() && self.printed[line].text.starts_with(shown))
    }

    /// Line `line` was reported at read `index`, on the row `id`; whether
    /// for the first time. Lines that are not new output are not tallied.
    fn report(&mut self, line: usize, index: usize, id: RowId) -> bool {
        if !self.required[line] {
            return false;
        }
        let text = self.printed[line].text.clone();
        if self.reported[line] {
            self.violations.push(Violation::Repeated {
                line,
                text,
                read: index,
            });
            return false;
        }
        self.reported[line] = true;
        let place = match self.found[line] {
            Some(found) if found.0 <= index => found,
            _ => {
                let at = self.reads[index]
                    .main
                    .iter()
                    .position(|&row| row == id)
                    .unwrap_or(usize::MAX);
                (index, at)
            }
        };
        if self.latest.is_some_and(|latest| latest > place) {
            self.violations.push(Violation::OutOfOrder {
                line,
                text,
                read: index,
            });
        } else {
            self.latest = Some(place);
        }
        true
    }
}

/// Every violation of the invariants by the reads of a run, given the
/// lines the terminal printed: in the order of the reads, each read's
/// reports in order, then its mapping, then its review position, and then
/// the lines missed or reported incompletely, in line order.
#[must_use]
pub fn check(printed: &[PrintedLine], reads: &[Observation]) -> Vec<Violation> {
    let mut by_first: HashMap<RowId, Vec<usize>> = HashMap::new();
    for (index, line) in printed.iter().enumerate() {
        by_first.entry(line.rows[0]).or_default().push(index);
    }
    let mut found = vec![None; printed.len()];
    for (index, read) in reads.iter().enumerate() {
        for (at, id) in read.main.iter().enumerate() {
            for &line in by_first.get(id).into_iter().flatten() {
                if found[line].is_none() && present(&printed[line], read) {
                    found[line] = Some((index, at));
                }
            }
        }
    }
    let required = printed
        .iter()
        .zip(&found)
        .map(|(line, found)| {
            found.is_some_and(|(index, _)| {
                !line.reprint
                    && index
                        .checked_sub(1)
                        .is_none_or(|before| !reads[before].screen.contains(&line.text))
            })
        })
        .collect();
    let mut tally = Tally {
        printed,
        reads,
        by_first,
        found,
        required,
        reported: vec![false; printed.len()],
        heard: vec![None; printed.len()],
        latest: None,
        violations: Vec::new(),
    };
    for (index, read) in reads.iter().enumerate() {
        for spoken in &read.heard.spoken {
            check_spoken(&mut tally, index, spoken);
        }
        let previous = index.checked_sub(1).map(|previous| &reads[previous]);
        if let Some(moved) = &read.heard.moved
            && let Some(previous) = previous
        {
            check_moved(&mut tally.violations, index, previous, read, moved);
        }
        check_review(&mut tally.violations, index, previous, read);
    }
    for (line, out) in printed.iter().enumerate() {
        if !tally.required[line] {
            continue;
        }
        if !tally.reported[line] {
            tally.violations.push(Violation::Missed {
                line,
                text: out.text.clone(),
            });
        } else if let Some(heard) = &tally.heard[line]
            && *heard != out.text
        {
            tally.violations.push(Violation::Incomplete {
                line,
                text: out.text.clone(),
                heard: heard.clone(),
            });
        }
    }
    tally.violations
}

/// Whether `row` of `rows` starts a line.
fn starts_line(rows: &[Seen], row: usize) -> bool {
    row < rows.len() && (row == 0 || !rows[row - 1].wraps)
}

/// Checks one report of read `index`.
fn check_spoken(tally: &mut Tally, index: usize, spoken: &Spoken) {
    let reads = tally.reads;
    let rows = &reads[index].rows;
    let (row, text) = match spoken {
        Spoken::Line { row, text }
        | Spoken::Growth { row, text }
        | Spoken::Redrawn { row, text } => (*row, text),
        Spoken::Summary { rows: counted } => {
            for row in counted.clone() {
                if !starts_line(rows, row) {
                    tally
                        .violations
                        .push(Violation::NotALine { read: index, row });
                    continue;
                }
                let id = rows[row].id;
                let shown = line_at(rows, row);
                if let Some(line) = tally
                    .shown(index, id)
                    .or_else(|| tally.coming(index, id, shown.trim_end()))
                {
                    tally.report(line, index, id);
                }
            }
            return;
        }
    };
    if !starts_line(rows, row) {
        tally
            .violations
            .push(Violation::NotALine { read: index, row });
        return;
    }
    let padded = line_at(rows, row);
    let shown = padded.trim_end();
    let whole = matches!(spoken, Spoken::Line { .. });
    let quoted = if whole {
        text == shown
    } else {
        !text.is_empty() && shown.contains(text.as_str())
    };
    if !quoted {
        let with_padding = if whole {
            text.trim_end() == shown
        } else {
            !text.is_empty() && padded.contains(text.as_str())
        };
        tally.violations.push(if with_padding {
            Violation::Padded {
                read: index,
                row,
                said: text.clone(),
            }
        } else {
            Violation::Misquoted {
                read: index,
                row,
                said: text.clone(),
                shown: shown.to_owned(),
            }
        });
        return;
    }
    let id = rows[row].id;
    match spoken {
        Spoken::Line { .. } => {
            // A line still being written counts as the line it becomes
            // when that line only grows from what was reported.
            let line = tally
                .shown(index, id)
                .or_else(|| tally.coming(index, id, shown));
            if let Some(line) = line
                && tally.report(line, index, id)
            {
                tally.heard[line] = Some(text.clone());
            }
        }
        Spoken::Growth { .. } => {
            let line = tally
                .shown(index, id)
                .filter(|&line| tally.heard[line].is_some())
                .or_else(|| tally.coming(index, id, shown));
            if let Some(heard) = line.and_then(|line| tally.heard[line].as_mut()) {
                heard.push_str(text);
            }
        }
        Spoken::Redrawn { .. } => {
            if let Some(line) = tally.shown(index, id) {
                tally.report(line, index, id);
            }
        }
        Spoken::Summary { .. } => {}
    }
}

/// Checks a reader's mapping of `previous`'s rows onto `read`'s.
fn check_moved(
    violations: &mut Vec<Violation>,
    index: usize,
    previous: &Observation,
    read: &Observation,
    moved: &[Option<usize>],
) {
    let counts = |rows: &[Seen]| {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for seen in rows {
            *counts.entry(seen.text.trim_end().to_owned()).or_default() += 1;
        }
        counts
    };
    let (before, after) = (counts(&previous.rows), counts(&read.rows));
    let now: HashMap<RowId, usize> = read
        .rows
        .iter()
        .enumerate()
        .map(|(row, seen)| (seen.id, row))
        .collect();
    for (row, seen) in previous.rows.iter().enumerate() {
        let text = seen.text.trim_end();
        let Some(&went) = now.get(&seen.id) else {
            continue;
        };
        if text.is_empty()
            || read.rows[went].text.trim_end() != text
            || before.get(text) != Some(&1)
            || after.get(text) != Some(&1)
        {
            continue;
        }
        let said = moved.get(row).copied().flatten();
        if said != Some(went) {
            violations.push(Violation::Mismapped {
                read: index,
                row,
                went,
                said,
            });
        }
    }
}

/// Checks the review position the reader reported after read `index`
/// against where it was after `previous`.
fn check_review(
    violations: &mut Vec<Violation>,
    index: usize,
    previous: Option<&Observation>,
    read: &Observation,
) {
    let said = read.heard.review;
    if let Some(row) = said
        && row >= read.rows.len()
    {
        violations.push(Violation::ReviewOutside { read: index, row });
        return;
    }
    let Some(previous) = previous.filter(|previous| previous.alternate == read.alternate) else {
        return;
    };
    let Some(id) = previous
        .heard
        .review
        .and_then(|row| previous.rows.get(row))
        .map(|seen| seen.id)
    else {
        return;
    };
    if let Some(went) = read.rows.iter().position(|seen| seen.id == id)
        && said != Some(went)
    {
        violations.push(Violation::ReviewMoved {
            read: index,
            went,
            said,
        });
    }
}

/// A terminal's size for the oracle's streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    /// Its width in cells.
    pub width: usize,
    /// Its screen's height in rows.
    pub height: usize,
    /// The rows of history above the screen.
    pub history: usize,
}

impl Size {
    /// The random streams' terminal: small enough that floods fill its
    /// history and words wrap.
    pub const SMALL: Self = Self {
        width: 12,
        height: 5,
        history: 6,
    };

    /// The exhaustive cases' terminal.
    pub const TINY: Self = Self {
        width: 4,
        height: 3,
        history: 1,
    };

    /// An empty terminal of this size.
    #[must_use]
    pub fn terminal(self, flavor: Flavor) -> Terminal {
        Terminal::new(flavor, self.width, self.height, self.history)
    }
}

/// The words the random streams write: repeated texts, a space inside a
/// line, and a word longer than the small terminal's width.
pub const WORDS: [&str; 6] = ["ok", "cat", "dog", "hello", "a b", "wrapping-word"];

/// One of the operations programs do to a terminal
/// (`docs/design/terminal-model.md`, section 3.2), as the oracle's streams
/// write them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Text written at the cursor.
    Write(&'static str),
    /// A line feed alone.
    LineFeed,
    /// A carriage return alone.
    CarriageReturn,
    /// A carriage return and a line feed, as programs end a line.
    Newline,
    /// The cursor moved to a row and column, counted from 1.
    MoveTo {
        /// The row.
        row: u8,
        /// The column.
        column: u8,
    },
    /// The cursor moved down (positive) or up (negative) by rows.
    MoveRows(i8),
    /// The cursor moved right (positive) or left (negative) by columns.
    MoveColumns(i8),
    /// Erase in line: 0 from the cursor, 1 to it, 2 the whole row.
    EraseLine(u8),
    /// Erase from the cursor to the screen's end.
    EraseBelow,
    /// A footer drawn on the screen's last row with the cursor saved and
    /// restored around it.
    Footer(&'static str),
    /// The screen's bottom two rows erased and drawn again, as Ink redraws
    /// its region, leaving the cursor at their end.
    Redraw(&'static str),
    /// A scroll region from row `top` to row `bottom`, counted from 1.
    Region {
        /// Its first row.
        top: u8,
        /// Its last row.
        bottom: u8,
    },
    /// The scroll region reset to the whole screen.
    ResetRegion,
    /// Rows inserted at the cursor.
    InsertRows(u8),
    /// Rows deleted at the cursor.
    DeleteRows(u8),
    /// `lines` lines of `word` and a number, each ended with a newline.
    Flood {
        /// How many lines.
        lines: u8,
        /// The word each starts with.
        word: &'static str,
    },
    /// The cursor sent home and the screen erased, as `cls` does.
    Clear,
    /// The history erased.
    ClearHistory,
    /// The alternate screen entered, the cursor saved (mode 1049).
    EnterAlternate,
    /// The alternate screen left, the cursor restored.
    LeaveAlternate,
}

impl Op {
    /// What a program writes for this operation.
    #[must_use]
    pub fn bytes(self) -> String {
        match self {
            Self::Write(text) => text.to_owned(),
            Self::LineFeed => "\n".to_owned(),
            Self::CarriageReturn => "\r".to_owned(),
            Self::Newline => "\r\n".to_owned(),
            Self::MoveTo { row, column } => format!("\x1b[{row};{column}H"),
            Self::MoveRows(rows) => match rows {
                0 => String::new(),
                1.. => format!("\x1b[{rows}B"),
                _ => format!("\x1b[{}A", rows.unsigned_abs()),
            },
            Self::MoveColumns(columns) => match columns {
                0 => String::new(),
                1.. => format!("\x1b[{columns}C"),
                _ => format!("\x1b[{}D", columns.unsigned_abs()),
            },
            Self::EraseLine(mode) => format!("\x1b[{mode}K"),
            Self::EraseBelow => "\x1b[J".to_owned(),
            Self::Footer(text) => format!("\x1b7\x1b[99;1H\x1b[2K{text}\x1b8"),
            Self::Redraw(text) => format!("\x1b[99;1H\x1b[A\r\x1b[J{text}\r\n~{text}"),
            Self::Region { top, bottom } => format!("\x1b[{top};{bottom}r"),
            Self::ResetRegion => "\x1b[r".to_owned(),
            Self::InsertRows(rows) => format!("\x1b[{rows}L"),
            Self::DeleteRows(rows) => format!("\x1b[{rows}M"),
            Self::Flood { lines, word } => (1..=lines).fold(String::new(), |mut flood, line| {
                let _ = write!(flood, "{word}{line}\r\n");
                flood
            }),
            Self::Clear => "\x1b[H\x1b[2J".to_owned(),
            Self::ClearHistory => "\x1b[3J".to_owned(),
            Self::EnterAlternate => "\x1b[?1049h".to_owned(),
            Self::LeaveAlternate => "\x1b[?1049l".to_owned(),
        }
    }
}

/// A stream of operations, read after each step marked `true` and after
/// the last: each chunk is the operations up to a read, so the stream has
/// the corpus's shape, and its `about` lists the steps.
#[must_use]
pub fn stream(steps: &[(Op, bool)]) -> Stream {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    for (op, read) in steps {
        chunk.push_str(&op.bytes());
        if *read {
            chunks.push(std::mem::take(&mut chunk));
        }
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    Stream {
        about: format!("{steps:?}"),
        chunks,
        at_ms: Vec::new(),
    }
}

/// The exhaustive cases' alphabet, for [`Size::TINY`]: a short word, one
/// that wraps, ending a line, a carriage return, the cursor up a row,
/// erasing to the row's end, clearing the screen, and both switches of
/// screen.
pub const TINY_ALPHABET: [Op; 9] = [
    Op::Write("ab"),
    Op::Write("cdefg"),
    Op::Newline,
    Op::CarriageReturn,
    Op::MoveRows(-1),
    Op::EraseLine(0),
    Op::Clear,
    Op::EnterAlternate,
    Op::LeaveAlternate,
];

/// Every stream of 1 to `longest` operations of `alphabet`, read after each
/// operation, shortest first.
#[must_use]
pub fn every_stream(alphabet: &[Op], longest: usize) -> Vec<Stream> {
    let mut streams = Vec::new();
    let mut sequences: Vec<Vec<Op>> = vec![Vec::new()];
    for _ in 0..longest {
        sequences = sequences
            .iter()
            .flat_map(|sequence| {
                alphabet.iter().map(move |&op| {
                    let mut longer = sequence.clone();
                    longer.push(op);
                    longer
                })
            })
            .collect();
        streams.extend(sequences.iter().map(|sequence| {
            let steps: Vec<(Op, bool)> = sequence.iter().map(|&op| (op, true)).collect();
            stream(&steps)
        }));
    }
    streams
}
