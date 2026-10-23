//! The oracle (`docs/design/terminal-model.md`, section 9): invariants over
//! the simulated terminal's ground truth that any reader of terminals must
//! keep, whatever the stream did, checked over random streams, every short
//! stream of a small alphabet, and the recorded corpus.
//!
//! A reader implements [`Reader`]: it chooses each read's [`Query`], is
//! given what the read answered, and reports what it would say and what it
//! withholds ([`Heard`]). [`observe`] reads a terminal with the reader once
//! before anything is written, as a reader starts watching a terminal, then
//! writes a stream into it and reads it after each chunk, and then, once
//! the quiet period has passed with nothing more written, reads it a last
//! time; it records each read's rows and the main buffer's rows as they
//! truly are ([`Observation`]). [`check`] lists every [`Violation`].
//!
//! The ground truth is the rows' identities. A line of a read is a row of
//! the main buffer that the row above does not wrap onto, and the rows
//! after it while each wraps onto the next, its text their text joined
//! with the padding after its end left out; only lines with text count. A
//! read on the main screen shows a line for the first time when one of the
//! line's rows has a text, padding left out, that no earlier read on the
//! main screen showed on that row. So a row whose text was on another row
//! before is shown for the first time, a row drawn again with a text it had
//! is not, a row that moved with its text is not, and a line a wrap joins
//! or splits is shown for the first time only where a row's text is new.
//! The whole main buffer counts, from its first row to the document's end,
//! whatever the read's own text covered, since a reader chooses where its
//! reads start.
//!
//! The invariants, as checked:
//!
//! - Every silence is accounted for. Every line a read shows for the first
//!   time is, at that read, reported (as a [`Spoken::Line`], its
//!   [`Spoken::Growth`], a [`Spoken::Redrawn`] row's speech, or counted in
//!   a [`Spoken::Summary`]) or withheld with a [`Reason`] whose premise is
//!   true (see [`Reason`] for each premise). Growth accounts for a line when
//!   what was reported of it with the growth begins the line's text; the
//!   rest is spoken by the last read, or the line is replaced first. A
//!   withholding on the alternate screen, where nothing must be accounted
//!   for, is not checked. Lines
//!   written and erased between two reads, and lines that left a full
//!   history or a scroll region between two reads, are never shown to a
//!   read, so nothing accounts for them.
//! - Lines are reported once, and in the order their rows stand: a line or
//!   summary reporting a line all of whose rows' texts were reported before
//!   repeats it, and among the lines one read shows for the first time,
//!   those reported as lines or in summaries are reported top to bottom.
//! - Everything reported is text the read shows: a line is exactly the
//!   line starting at the row it names, padding left out; growth and a
//!   redrawn row's speech are part of that line; and every report, each
//!   row a summary counts and each row withheld included, names the first
//!   row of a line of the read.
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

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::ops::Range;

use crate::conformance::Stream;
use crate::{Flavor, Query, Refused, RowId, Snapshot, Terminal};

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

/// Why a reader withholds a line a read shows for the first time, each
/// the silence of a rule of `docs/design/terminal-model.md`, sections 4
/// and 5, with the premise the oracle checks against the ground truth.
/// Three are holds, whose premise is that a later read reports the line
/// or no longer shows its text (it changed, its row went, or the alternate
/// screen is up), the last read, taken after the quiet period, included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// A hold: a volatile row is not spoken while it changes, and is
    /// spoken once it settles (section 5.4).
    Volatile,
    /// A hold: a growing line's unfinished last word waits for the line to
    /// grow past it, the cursor to leave, or the quiet period (section
    /// 5.3).
    UnfinishedWord,
    /// A hold: a row a key changed away from the caret waits while keys
    /// keep coming, and is spoken once they pause (section 5.7). Only the
    /// hold is checked, not that a key was pressed.
    HeldForKeys,
    /// A spinner's symbol replaced alone is silent (section 5.4). Premise:
    /// the last earlier read on the main screen showed a line that this
    /// read no longer shows on its row and whose text differs from this
    /// one in exactly one character, neither the old nor the new one a
    /// letter or a digit, so a counter's digit is not a spinner.
    Spinner,
    /// A reprint after a clear, or the input reprinted below a list, repeats
    /// rows the user heard (sections 4.4 and 5.3). Premise: the lines this
    /// read withholds as a reprint, top to bottom, are found in that order
    /// among the lines of one earlier read on the main screen.
    Reprint,
    /// A kept row is silent wherever it moved, and a program that draws its
    /// rows again a row lower or higher moves them (sections 4.3 and 5.3).
    /// Premise: the last earlier read on the main screen showed a line with
    /// this text starting on another row, which this read no longer shows
    /// there.
    Moved,
    /// A lingering row taken back is the same row returning (section 4.1).
    /// Premise: an earlier read on the main screen, within the 2 seconds a
    /// row lingers when the stream records when its chunks came, showed a
    /// line with this text starting on another row, which this read no
    /// longer shows there.
    Lingering,
    /// The caret's row showing characters the user typed, which typing
    /// echo speaks (section 5.5). Premise: a key was typed; the oracle's
    /// streams type none, so this premise is never true yet.
    HeldTyping,
    /// The row's change is a key's answer, spoken as that key's answer or
    /// silent by the rules for keys, such as the text Control+W erased or a
    /// list entry that only lost its marker (sections 5.5 and 5.7).
    /// Premise: a key reached the program within the key's window before
    /// the read; the oracle's streams press none, so this premise is never
    /// true yet.
    KeyAnswer,
}

impl Reason {
    /// Whether the reason holds the line to be spoken later.
    fn holds(self) -> bool {
        matches!(
            self,
            Self::Volatile | Self::UnfinishedWord | Self::HeldForKeys
        )
    }
}

/// A line a reader withholds at a read, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Withheld {
    /// The row the line starts on, counted as a report's row is.
    pub row: usize,
    /// Why it is not spoken.
    pub reason: Reason,
}

/// What a reader would say after one read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heard {
    /// What it says, in order.
    pub spoken: Vec<Spoken>,
    /// The lines it does not speak, and why.
    pub withheld: Vec<Withheld>,
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

    /// The quiet period has passed since the last write, longer than any
    /// hold waits, and nothing more will be written: the read that follows
    /// is the stream's last.
    fn quiet(&mut self) {}
}

/// One read as the oracle sees it: the rows its text covered and the main
/// buffer's rows, as they truly are, and what the reader said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    /// When the chunk read after came, in milliseconds from the stream's
    /// start, for a stream that records it; `None` for the first and last
    /// reads and for a stream written by hand.
    pub at_ms: Option<u64>,
    /// Whether the alternate screen was up.
    pub alternate: bool,
    /// The rows the read's text covered, first to last; empty when the read
    /// was refused.
    pub rows: Vec<Seen>,
    /// The main buffer's rows from its first to the document's end, their
    /// text as a read gives it; empty while the alternate screen is up.
    pub main: Vec<Seen>,
    /// What the reader said.
    pub heard: Heard,
}

/// A stream written into a terminal and read after each chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// Each read the reader answered with what it heard, in order: the
    /// first before anything was written, then one after each chunk, then
    /// the last after the quiet period. A read the reader asked to take
    /// again is replaced by the read taken again.
    pub reads: Vec<Observation>,
}

impl Run {
    /// Every violation of the invariants in this run.
    #[must_use]
    pub fn violations(&self) -> Vec<Violation> {
        check(&self.reads)
    }
}

/// Reads `terminal` with `reader` once as it is, as a reader starts
/// watching a terminal, then writes `stream` into it chunk by chunk,
/// reading it after each chunk, and then tells the reader the quiet period
/// has passed and reads it a last time.
///
/// # Panics
///
/// When the reader asks to read again twice at one point, which no reader
/// needs.
pub fn observe(terminal: &mut Terminal, stream: &Stream, reader: &mut dyn Reader) -> Run {
    let mut reads = vec![read(terminal, reader, None)];
    for (index, chunk) in stream.chunks.iter().enumerate() {
        terminal.write(chunk.as_bytes());
        reads.push(read(terminal, reader, stream.at_ms.get(index).copied()));
    }
    reader.quiet();
    reads.push(read(terminal, reader, None));
    Run { reads }
}

/// One read of `terminal` by `reader`, taken again if the reader asks.
fn read(terminal: &Terminal, reader: &mut dyn Reader, at_ms: Option<u64>) -> Observation {
    let mut query = reader.query();
    let mut again = false;
    loop {
        let (snapshot, rows) = match terminal.observe(&query) {
            Ok((snapshot, rows)) => (Ok(snapshot), rows),
            Err(refused) => (Err(refused), Vec::new()),
        };
        match reader.read(&snapshot) {
            Reply::Heard(heard) => {
                return Observation {
                    at_ms,
                    alternate: terminal.grid().alternate.is_some(),
                    rows,
                    main: terminal.document(),
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

/// A way a reader broke an invariant. Reads and rows are numbered from 0;
/// a coordinate is a row's place in the main buffer, from its first row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    /// A line the read showed for the first time was neither reported nor
    /// withheld.
    Missed {
        /// The read.
        read: usize,
        /// The coordinate of the line's first row.
        coordinate: usize,
        /// Its text.
        text: String,
    },
    /// A line was withheld for a reason whose premise is false.
    FalseReason {
        /// The read it was withheld at.
        read: usize,
        /// The row it starts on.
        row: usize,
        /// Its text.
        text: String,
        /// The reason given.
        reason: Reason,
    },
    /// A line was reported as a line or in a summary when every row's text
    /// had been reported before.
    Repeated {
        /// The read.
        read: usize,
        /// The row it starts on.
        row: usize,
        /// Its text.
        text: String,
    },
    /// A line the read showed for the first time was reported after a line
    /// below it that the read also showed for the first time.
    OutOfOrder {
        /// The read.
        read: usize,
        /// The row it starts on.
        row: usize,
        /// Its text.
        text: String,
    },
    /// At the last read, what was reported of a line, the line and its
    /// growth, is only the start of its text.
    Incomplete {
        /// The coordinate of the line's first row.
        coordinate: usize,
        /// Its text.
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
    /// A report, a row a summary counts or a row withheld names a row that
    /// is not the first row of a line, or no row of the read.
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

/// A line with text of the main buffer at a read.
#[derive(Clone, Debug)]
struct Line {
    /// Its first row's identity.
    id: RowId,
    /// Its rows' coordinates.
    rows: Range<usize>,
    /// Its text, padding left out.
    text: String,
}

/// The lines with text of a read's main buffer, top to bottom, and the
/// line starting on each row.
#[derive(Clone, Debug, Default)]
struct Lines {
    lines: Vec<Line>,
    by_first: HashMap<RowId, usize>,
}

impl Lines {
    fn of(main: &[Seen]) -> Self {
        let mut lines = Vec::new();
        let mut first = 0;
        for (at, seen) in main.iter().enumerate() {
            if seen.wraps && at + 1 < main.len() {
                continue;
            }
            let text: String = main[first..=at]
                .iter()
                .map(|seen| seen.text.as_str())
                .collect();
            let text = text.trim_end();
            if !text.is_empty() {
                lines.push(Line {
                    id: main[first].id,
                    rows: first..at + 1,
                    text: text.to_owned(),
                });
            }
            first = at + 1;
        }
        let by_first = lines
            .iter()
            .enumerate()
            .map(|(index, line)| (line.id, index))
            .collect();
        Self { lines, by_first }
    }

    /// The line starting on row `id`.
    fn starting(&self, id: RowId) -> Option<&Line> {
        self.by_first.get(&id).map(|&index| &self.lines[index])
    }

    /// Whether row `id` still starts a line with `text`.
    fn shows(&self, id: RowId, text: &str) -> bool {
        self.starting(id).is_some_and(|line| line.text == text)
    }
}

/// A line a reader holds, waiting to be reported or replaced.
struct Hold {
    read: usize,
    row: usize,
    id: RowId,
    text: String,
    reason: Reason,
}

/// What the check carries from read to read.
struct Check<'a> {
    reads: &'a [Observation],
    /// Each read's lines; `None` for a read on the alternate screen.
    lines: Vec<Option<Lines>>,
    /// The texts each row showed, padding left out, at the reads checked
    /// so far.
    shown: HashMap<RowId, HashSet<String>>,
    /// The texts each row had when a report named its line.
    reported: HashMap<RowId, HashSet<String>>,
    /// What was reported of each line, by its first row, while the line
    /// still begins with it.
    heard: HashMap<RowId, String>,
    holds: Vec<Hold>,
    violations: Vec<Violation>,
}

/// What one read's check accumulates.
struct ReadState {
    /// Whether each line is shown for the first time.
    first_time: Vec<bool>,
    /// Whether each line is reported or withheld.
    accounted: Vec<bool>,
    /// The lowest coordinate a line reported for the first time may have.
    latest: usize,
    /// The first rows of the lines a report named.
    named: HashSet<RowId>,
}

impl Check<'_> {
    /// The rows' texts of `line` in read `index`'s main buffer.
    fn row_texts<'r>(
        &'r self,
        index: usize,
        line: &Line,
    ) -> impl Iterator<Item = (RowId, &'r str)> {
        self.reads[index].main[line.rows.clone()]
            .iter()
            .map(|seen| (seen.id, seen.text.trim_end()))
            .filter(|(_, text)| !text.is_empty())
    }

    /// Marks `line` of read `index` reported, with its rows' texts.
    fn mark_reported(&mut self, index: usize, line: &Line) {
        let texts: Vec<(RowId, String)> = self
            .row_texts(index, line)
            .map(|(id, text)| (id, text.to_owned()))
            .collect();
        for (id, text) in texts {
            self.reported.entry(id).or_default().insert(text);
        }
    }

    /// `line`, the line `number` of read `index`, reported as a line or in a
    /// summary naming row `row`.
    fn report(&mut self, index: usize, row: usize, number: usize, state: &mut ReadState) {
        let line = self.lines[index].as_ref().expect("a main read").lines[number].clone();
        let repeated = self.row_texts(index, &line).all(|(id, text)| {
            self.reported
                .get(&id)
                .is_some_and(|texts| texts.contains(text))
        });
        if repeated {
            self.violations.push(Violation::Repeated {
                read: index,
                row,
                text: line.text,
            });
            return;
        }
        if state.first_time[number] && !state.accounted[number] {
            if line.rows.start < state.latest {
                self.violations.push(Violation::OutOfOrder {
                    read: index,
                    row,
                    text: line.text.clone(),
                });
            } else {
                state.latest = line.rows.start;
            }
        }
        state.accounted[number] = true;
        self.mark_reported(index, &line);
    }
}

/// Every violation of the invariants by the reads of a run: in the order of
/// the reads, each read's reports in order, then what it withheld, then
/// the lines it missed, then its mapping, then its review position; and
/// then the holds no later read resolved and the lines reported only in
/// part at the last read.
#[must_use]
pub fn check(reads: &[Observation]) -> Vec<Violation> {
    let lines = reads
        .iter()
        .map(|read| (!read.alternate).then(|| Lines::of(&read.main)))
        .collect();
    let mut check = Check {
        reads,
        lines,
        shown: HashMap::new(),
        reported: HashMap::new(),
        heard: HashMap::new(),
        holds: Vec::new(),
        violations: Vec::new(),
    };
    for index in 0..reads.len() {
        check_read(&mut check, index);
    }
    let Check {
        holds,
        mut violations,
        heard,
        lines,
        ..
    } = check;
    for hold in holds {
        violations.push(Violation::FalseReason {
            read: hold.read,
            row: hold.row,
            text: hold.text,
            reason: hold.reason,
        });
    }
    if let Some(Some(last)) = lines.last() {
        for line in &last.lines {
            if let Some(said) = heard.get(&line.id)
                && *said != line.text
            {
                violations.push(Violation::Incomplete {
                    coordinate: line.rows.start,
                    text: line.text.clone(),
                    heard: said.clone(),
                });
            }
        }
    }
    violations
}

/// Checks read `index`.
fn check_read(check: &mut Check, index: usize) {
    let reads = check.reads;
    let read = &reads[index];
    let count = check.lines[index]
        .as_ref()
        .map_or(0, |lines| lines.lines.len());
    let mut state = ReadState {
        first_time: vec![false; count],
        accounted: vec![false; count],
        latest: 0,
        named: HashSet::new(),
    };
    if let Some(lines) = &check.lines[index] {
        for (number, line) in lines.lines.iter().enumerate() {
            state.first_time[number] = check.row_texts(index, line).any(|(id, text)| {
                !check
                    .shown
                    .get(&id)
                    .is_some_and(|texts| texts.contains(text))
            });
        }
    }
    let lines = check.lines[index].clone();
    check.heard.retain(|id, said| {
        lines
            .as_ref()
            .and_then(|lines| lines.starting(*id))
            .is_some_and(|line| line.text.starts_with(said.as_str()))
    });
    for spoken in &read.heard.spoken {
        check_spoken(check, index, spoken, &mut state);
    }
    check.holds.retain(|hold| {
        let resolved = lines
            .as_ref()
            .is_none_or(|lines| !lines.shows(hold.id, &hold.text))
            || state.named.contains(&hold.id);
        !resolved
    });
    check_withheld(check, index, &mut state);
    if let Some(lines) = &lines {
        for (number, line) in lines.lines.iter().enumerate() {
            if state.first_time[number] && !state.accounted[number] {
                check.violations.push(Violation::Missed {
                    read: index,
                    coordinate: line.rows.start,
                    text: line.text.clone(),
                });
            }
        }
        for line in &lines.lines {
            let texts: Vec<(RowId, String)> = check
                .row_texts(index, line)
                .map(|(id, text)| (id, text.to_owned()))
                .collect();
            for (id, text) in texts {
                check.shown.entry(id).or_default().insert(text);
            }
        }
    }
    let previous = index.checked_sub(1).map(|previous| &reads[previous]);
    if let Some(moved) = &read.heard.moved
        && let Some(previous) = previous
    {
        check_moved(&mut check.violations, index, previous, read, moved);
    }
    check_review(&mut check.violations, index, previous, read);
}

/// Whether `row` of `rows` starts a line.
fn starts_line(rows: &[Seen], row: usize) -> bool {
    row < rows.len() && (row == 0 || !rows[row - 1].wraps)
}

/// The number of the line of read `index` starting on the read's row
/// `row`, if the read is on the main screen and a line with text starts
/// there.
fn line_number(check: &Check, index: usize, row: usize) -> Option<usize> {
    let id = check.reads[index].rows[row].id;
    check.lines[index].as_ref()?.by_first.get(&id).copied()
}

/// Checks one report of read `index`.
fn check_spoken(check: &mut Check, index: usize, spoken: &Spoken, state: &mut ReadState) {
    let rows = &check.reads[index].rows;
    let (row, text) = match spoken {
        Spoken::Line { row, text }
        | Spoken::Growth { row, text }
        | Spoken::Redrawn { row, text } => (*row, text),
        Spoken::Summary { rows: counted } => {
            for row in counted.clone() {
                if !starts_line(rows, row) {
                    check
                        .violations
                        .push(Violation::NotALine { read: index, row });
                    continue;
                }
                state.named.insert(rows[row].id);
                if let Some(number) = line_number(check, index, row) {
                    check.report(index, row, number, state);
                    let line = &check.lines[index].as_ref().expect("a main read").lines[number];
                    check.heard.insert(line.id, line.text.clone());
                }
            }
            return;
        }
    };
    if !starts_line(rows, row) {
        check
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
        check.violations.push(if with_padding {
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
    state.named.insert(rows[row].id);
    let Some(number) = line_number(check, index, row) else {
        return;
    };
    let line = check.lines[index].as_ref().expect("a main read").lines[number].clone();
    match spoken {
        Spoken::Line { .. } => {
            check.report(index, row, number, state);
            check.heard.insert(line.id, text.clone());
        }
        Spoken::Growth { .. } => {
            if let Some(said) = check.heard.get_mut(&line.id) {
                let grown = format!("{said}{text}");
                if line.text.starts_with(&grown) {
                    *said = grown;
                    state.accounted[number] = true;
                    check.mark_reported(index, &line);
                }
            }
        }
        Spoken::Redrawn { .. } => {
            state.accounted[number] = true;
            check.mark_reported(index, &line);
            check.heard.insert(line.id, line.text.clone());
        }
        Spoken::Summary { .. } => {}
    }
}

/// Checks what read `index` withheld, and each reason's premise that can
/// be judged at once; holds wait for later reads.
fn check_withheld(check: &mut Check, index: usize, state: &mut ReadState) {
    let reads = check.reads;
    let read = &reads[index];
    let mut reprints = Vec::new();
    for withheld in &read.heard.withheld {
        let row = withheld.row;
        if !starts_line(&read.rows, row) {
            check
                .violations
                .push(Violation::NotALine { read: index, row });
            continue;
        }
        let Some(number) = line_number(check, index, row) else {
            continue;
        };
        state.accounted[number] = true;
        let line = check.lines[index].as_ref().expect("a main read").lines[number].clone();
        let reason = withheld.reason;
        let true_now = match reason {
            _ if reason.holds() => {
                check.holds.push(Hold {
                    read: index,
                    row,
                    id: line.id,
                    text: line.text.clone(),
                    reason,
                });
                true
            }
            Reason::Spinner => spinner(check, index, &line.text),
            Reason::Moved => returned(check, index, &line, true),
            Reason::Lingering => returned(check, index, &line, false),
            Reason::Reprint => {
                reprints.push((line.rows.start, row, line.text.clone()));
                true
            }
            _ => false,
        };
        if !true_now {
            check.violations.push(Violation::FalseReason {
                read: index,
                row,
                text: line.text,
                reason,
            });
        }
    }
    reprints.sort_unstable();
    for (row, text) in reprint_unmatched(check, index, &reprints) {
        check.violations.push(Violation::FalseReason {
            read: index,
            row,
            text,
            reason: Reason::Reprint,
        });
    }
}

/// Whether `text` at read `index` is a spinner's change: the last earlier
/// read on the main screen showed a line, which this read no longer shows
/// on its row, whose text differs from `text` in exactly one character,
/// neither the old nor the new one a letter or a digit.
fn spinner(check: &Check, index: usize, text: &str) -> bool {
    let now = check.lines[index].as_ref().expect("a main read");
    let Some(before) = check.lines[..index].iter().rev().flatten().next() else {
        return false;
    };
    before.lines.iter().any(|old| {
        if now.shows(old.id, &old.text) {
            return false;
        }
        let (old, new): (Vec<char>, Vec<char>) =
            (old.text.chars().collect(), text.chars().collect());
        old.len() == new.len() && {
            let differing: Vec<(char, char)> = old
                .iter()
                .zip(&new)
                .filter(|(a, b)| a != b)
                .map(|(&a, &b)| (a, b))
                .collect();
            matches!(differing[..], [(a, b)] if !a.is_alphanumeric() && !b.is_alphanumeric())
        }
    })
}

/// How long a gone row lingers (`docs/design/terminal-model.md`, section
/// 5.14).
const LINGERING_MS: u64 = 2000;

/// Whether `line` at read `index` is a row that returned elsewhere: an
/// earlier read on the main screen showed a line with its text starting on
/// another row, which this read no longer shows there. For a moved row
/// that read is the last earlier one on the main screen; for a lingering
/// row any earlier one within the time a row lingers, when both reads'
/// times are known.
fn returned(check: &Check, index: usize, line: &Line, moved: bool) -> bool {
    let now = check.lines[index].as_ref().expect("a main read");
    let at = check.reads[index].at_ms;
    let last = (0..index)
        .rev()
        .find(|&earlier| check.lines[earlier].is_some());
    (0..index).any(|earlier| {
        let eligible = if moved {
            Some(earlier) == last
        } else {
            match (check.reads[earlier].at_ms, at) {
                (Some(then), Some(at)) => at.saturating_sub(then) <= LINGERING_MS,
                _ => true,
            }
        };
        eligible
            && check.lines[earlier].as_ref().is_some_and(|before| {
                before.lines.iter().any(|old| {
                    old.text == line.text && old.id != line.id && !now.shows(old.id, &old.text)
                })
            })
    })
}

/// The lines of `claims` (coordinate, row, text, top to bottom) that read
/// `index` withholds as a reprint and that are not found, in that order,
/// among the lines of the earlier read on the main screen that holds the
/// most of them.
fn reprint_unmatched(
    check: &Check,
    index: usize,
    claims: &[(usize, usize, String)],
) -> Vec<(usize, String)> {
    if claims.is_empty() {
        return Vec::new();
    }
    let unmatched = |before: &Lines| -> Vec<usize> {
        let mut texts = before.lines.iter().map(|line| line.text.as_str());
        let mut missing = Vec::new();
        for (claim, (_, _, text)) in claims.iter().enumerate() {
            let mut rest = texts.clone();
            if rest.any(|old| old == text) {
                texts = rest;
            } else {
                missing.push(claim);
            }
        }
        missing
    };
    let best = check.lines[..index]
        .iter()
        .flatten()
        .map(unmatched)
        .min_by_key(Vec::len)
        .unwrap_or_else(|| (0..claims.len()).collect());
    best.into_iter()
        .map(|claim| (claims[claim].1, claims[claim].2.clone()))
        .collect()
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
