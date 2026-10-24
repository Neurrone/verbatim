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
//! The oracle judges by text, as a reader can observe it, never by the
//! hidden identity of the row a program wrote to. A line of a read is a row
//! of the main buffer that the row above does not wrap onto, and the rows
//! after it while each wraps onto the next, its text their text joined
//! with the padding after its end left out; only lines with text count.
//! The main screen of a read is the main buffer from its first row to the
//! document's end, history included, whatever the read's own text covered,
//! since a reader chooses where its reads start. A line is new at a read
//! on the main screen when no earlier read showed a line with its text,
//! anywhere on the main screen or in what the read showed of the alternate
//! screen, which a reader cannot always tell from the main one (section
//! 5.8), leaving out the rows of only spaces at its start, which show
//! nothing (the console host joins a row filled with spaces to the line
//! below it). A line whose start left a full history
//! (the buffer's first row continuing a line rotated out) is new only when
//! its text ends no line shown before. A line whose start is above the
//! read's first row is reported by naming the read's first row, with the
//! text of its part in the read; it is new or not by its whole text.
//!
//! A line's place is its first row's row on the screen, counted from the
//! screen's top (negative in the history). The premises that speak of "the
//! row", and what was reported of a line that is not new, compare a line
//! with the line its row was in at the last read on the main screen: a row
//! the buffer held then is followed to where it was, its coordinate moved
//! down by the rows the buffer discarded from its front since, rotated out
//! of a full history or erased with the history, as a reader follows it by
//! the coordinates the providers report and the rotation it measures
//! (section 4.3), so it stays with its row as the history grows, rotates or
//! is erased; a row written since is compared with the line at its place,
//! since a reader sees only the screen there. This is ground truth of
//! where rows went, not a row's identity. A line is also followed through
//! a move with its neighbours, judged by text alone, as a reader's
//! alignment finds a block of rows moved by the same distance (section
//! 4.3, items 3 and 6): a row with text moved when its text is on exactly
//! one row of the main screen at the last read and at this one, at other
//! coordinates than following by the history alone gives. A line whose
//! first row moved is followed to where that row was; and a line whose
//! place by the history alone had no text at the last read, or text that
//! moved, and whose nearest row with text above it, or below its last
//! row, over rows blank in both reads, moved by some distance, is followed
//! by the same distance when a line with text started there at the last
//! read and the rows between were blank then too, as a footer's status
//! row that changed as output above it pushed the footer down a row, or a
//! row that changed while the rows of a scroll region moved up. A line
//! rewritten where it was is never followed by its neighbours' move, so a
//! header stays put while the scroll region below it moves. Where a line
//! changed from one text to another moves with the line, so a row's
//! volatility moves with it, and only with it. A row's text is its
//! text with the padding after a line's end left out, so a row that wraps
//! onto the next keeps the spaces at its end (section 5.12).
//!
//! A line no longer shown as often as the last read on the main screen
//! showed it has vanished: it was replaced, erased or moved away. Each
//! vanished line justifies one change, a redrawn row's speech, a spinner or
//! an unfinished word, matched one to one with the most claims justified.
//!
//! The invariants, as checked:
//!
//! - Every silence is accounted for. Every new line is, at that read,
//!   reported with its new text (see [`Spoken`] for what each report must
//!   say) or withheld with a [`Reason`] whose premise is true (see
//!   [`Reason`] for each). A hold is then resolved, before the last read is
//!   checked, by a report at the same or a later read that says the held
//!   text's line (a
//!   line, a summary's count, growth that completes it, or a redrawn row's
//!   speech covering its change from the text its place had before the
//!   hold, or the held line spoken as last read covering that change after
//!   it left the read), or by the held text being replaced on the main
//!   screen, written over, erased or deleted, or by the alternate screen coming up without
//!   it; the row leaving a full history as it rotates, or a report
//!   merely naming the row, resolves nothing. Lines written and erased between two
//!   reads, and lines that left a full history or a scroll region between
//!   two reads, are never shown to a read, so nothing accounts for them.
//!   A reason given for a line that is not new is not checked, since such a
//!   line needs none, and nothing is checked on the alternate screen.
//! - A line is reported once while its text stays on the screen: a line or
//!   a summary's count is repeated when as many lines with its text were
//!   reported, at reads that each showed at least that many, as the read
//!   shows; a line of the last read that the full history rotated out since,
//!   or that was erased with the history, no longer counts. While the
//!   alternate screen is up, the main screen's lines are
//!   on the screen only as often as the alternate screen shows their text.
//!   Among the new lines of one read, those reported as lines or in
//!   summaries are reported top to bottom.
//! - Everything reported is text the read shows: a line is exactly the
//!   line, or its part in the read, starting at the row it names, padding
//!   left out; growth and a redrawn row's speech are part of that line; and
//!   every report, each row a summary counts and each row withheld, names
//!   the first row of a line of the read, or the read's first row.
//! - For every row on the screen whose text is on exactly one row of each
//!   of two reads in a row, a reader that reports where rows went maps it
//!   to the row with that text, unless the screen switched between them.
//!   A switch is the alternate screen opening or closing, or what a reader
//!   cannot tell from one (section 5.8): the history going away, so the
//!   screen starts at the document's start, or coming back after that.
//! - Padding is never reported: a report that matches the read only with
//!   the spaces after a line's last character is told apart from one that
//!   does not match at all.
//! - A reader with a review position keeps it on its text: when the text
//!   of the row it was on at one read is on exactly one row of that read
//!   and of the next, on the screen in both and with no switch between
//!   them, it is on that row; and it is always on a row of the document:
//!   of the read, or of the history above it down to the document's first
//!   row. No key is ever pressed and following is never on in the oracle's
//!   streams.
//!
//! What redrawn rows, keys, lists and colours say is decided speech, not a
//! property of the stream, so it is tested by unit tests that state a
//! stream and its speech, never here.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::ops::Range;
use std::rc::Rc;

use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, RngSeed};

use crate::conformance::{Chunk, Stream};
use crate::{Flavor, Query, Refused, RowId, Snapshot, Terminal};

/// A row of a read as it truly is: its identity, its text as the read
/// gives it (padding included), and whether its line wraps onto the next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// The row's identity, which the oracle uses only to know whether a
    /// held row left the buffer.
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
    /// What a line has gained since the last read on the main screen. It
    /// says the line's new text when what was reported of the line it grew
    /// from (a line of the last read on the main screen whose text begins
    /// this one's, or, for a line whose start left the buffer, the longest
    /// end of a line reported whole that begins it), with this growth and
    /// any before it at this read, is the whole line; when it is only the
    /// line's start, the line must also be withheld. A line withheld as a
    /// spinner or as having lost text counts as reported whole. Growth also
    /// says the line's new text when it completes the line from what was
    /// said of a line withheld as having lost text (see
    /// [`Reason::LostText`]).
    Growth {
        /// The row the line starts on.
        row: usize,
        /// What is spoken.
        text: String,
    },
    /// Speech about a row rewritten, such as the words that changed. It
    /// says the line's new text when a vanished line (see the module's
    /// documentation) had other text and the speech is a part of the line
    /// that covers every character from the first that differs from that
    /// text to the last, counted from the line's end, or, when the line only
    /// lost characters, a part that touches where they were. So a row that
    /// had no text, or speech of a letter of a changed word, says nothing
    /// new. When the vanished line was withheld as having lost text, the
    /// change may instead be counted from what was said of it (see
    /// [`Reason::LostText`]).
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
    /// A line held at an earlier read that left the read before it was
    /// spoken, as when a flood pushed it out of a full history, spoken as
    /// it was last read (`docs/design/terminal-model.md`, section 4.3, item
    /// 5). It names no row; it resolves a hold still unresolved whose
    /// change from what its row had before the hold it covers, and with
    /// none, it must be the end of a line an earlier read showed.
    Held {
        /// What is spoken.
        text: String,
    },
}

/// Why a reader withholds a new line, each the silence of a rule of
/// `docs/design/terminal-model.md`, sections 4 and 5, with the premise the
/// oracle checks against the ground truth. Two are holds, which the same or
/// a later read must resolve (see the module's documentation), the last read,
/// taken after the quiet period, included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// A hold: a volatile row is not spoken while it changes, and is
    /// spoken once it settles (section 5.4). Premise: the line starting at
    /// its row at the last read on the main screen had other text, and at
    /// an earlier read the line there had changed from one text to another
    /// too, the row followed to where it was, through the history's growth
    /// and rotation and through moves with its neighbours, at each read
    /// since (see the module's documentation).
    Volatile,
    /// A hold: a growing line's unfinished last word waits for the line to
    /// grow past it, the cursor to leave, or the quiet period (section
    /// 5.3). Premise: the line grew: a line of the last read on the main
    /// screen whose text begins this one's and is shorter is no longer
    /// shown as often, each such line justifying one claim.
    UnfinishedWord,
    /// A hold: a row a key changed away from the caret waits while keys
    /// keep coming, and is spoken once they pause (section 5.7). Premise: a
    /// key was pressed; the oracle's streams press none, so this premise is
    /// never true yet.
    HeldForKeys,
    /// A spinner's symbol replaced alone is silent (section 5.4). Premise:
    /// a line of the last read on the main screen that is no longer shown
    /// as often differs from this one in exactly one character, neither the
    /// old nor the new one a letter or a digit, so a counter's tick is not
    /// a spinner; each such line justifies one claim.
    Spinner,
    /// A row that only lost text is silent, as a list entry that lost its
    /// marker or a wrapped line whose last row was erased (section 5.7).
    /// Premise: compared without spaces, the line covering its row at the
    /// last read on the main screen, followed to where it was (see the
    /// module's documentation), or a vanished line, as when a line is split
    /// in two, contains this one's text and is longer. What was reported of
    /// each such longer line is what was said of this one, kept while a
    /// line with its text is shown: when the line gains text again, the
    /// text it gets back is not new, so a redrawn row's speech or growth
    /// says the line's new text when it says what the line gained beyond
    /// what was said of it.
    LostText,
    /// The caret's row showing characters the user typed, which typing
    /// echo speaks (section 5.5). Premise: a key was typed; the oracle's
    /// streams type none, so this premise is never true yet.
    HeldTyping,
    /// The row's change is a key's answer, spoken as that key's answer or
    /// silent by the rules for keys, such as the text Control+W erased
    /// (sections 5.5 and 5.7). Premise: a key reached the program within
    /// the key's window before the read; the oracle's streams press none,
    /// so this premise is never true yet.
    KeyAnswer,
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
    /// The row of this read its review position is on after the read,
    /// counted from the read's first row, negative above it in the
    /// history; `None` when it has none.
    pub review: Option<isize>,
}

/// A reader's answer to a read.
#[derive(Clone, Debug)]
pub enum Reply {
    /// What it would say.
    Heard(Heard),
    /// Read again at once with this query, before anything more is
    /// written, as a reader reads the whole text when its last screen is
    /// not in the read (`docs/design/terminal-model.md`, section 4.2), or
    /// reads from further above its start to find how far a full history
    /// rotated (section 4.3, item 5).
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
    /// Whether the alternate screen was up.
    pub alternate: bool,
    /// The coordinate of the first row the read's text covered.
    pub start: usize,
    /// The rows the read's text covered, first to last; empty when the read
    /// was refused.
    pub rows: Vec<Seen>,
    /// The main buffer's rows from its first to the document's end, their
    /// text as a read gives it; empty while the alternate screen is up.
    pub main: Vec<Seen>,
    /// The coordinate of the main screen's first row.
    pub screen_top: usize,
    /// Whether the main buffer's first row continues a line whose start
    /// left the buffer.
    pub continued: bool,
    /// The identities of the rows the main buffer discarded as its full
    /// history rotated since the previous read: rows that left the buffer
    /// with their text, as opposed to rows deleted or erased.
    pub rotated: Vec<RowId>,
    /// How many rows the main buffer erased from its front with the
    /// history since the previous read.
    pub erased: usize,
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
/// When the reader asks to read again more than [`AGAIN_LIMIT`] times at
/// one point, which no reader needs.
pub fn observe(terminal: &mut Terminal, stream: &Stream, reader: &mut dyn Reader) -> Run {
    let mut gone = (0, 0);
    let mut reads = vec![read(terminal, reader, &mut gone)];
    for chunk in &stream.chunks {
        terminal.write(chunk.as_bytes());
        reads.push(read(terminal, reader, &mut gone));
    }
    reader.quiet();
    reads.push(read(terminal, reader, &mut gone));
    Run { reads }
}

/// The most times a reader may ask to read again at one point: a reader
/// searching upward from a screen above its start, four times as far each
/// time, reaches the start of any buffer the oracle writes in far fewer.
pub const AGAIN_LIMIT: usize = 16;

/// One read of `terminal` by `reader`, taken again as often as the reader asks;
/// `gone` counts the rows the main buffer rotated out, and those it erased
/// with the history, before it.
fn read(terminal: &Terminal, reader: &mut dyn Reader, gone: &mut (usize, usize)) -> Observation {
    let mut query = reader.query();
    let mut again = 0;
    loop {
        let (snapshot, start, rows) = match terminal.observe(&query) {
            Ok((snapshot, start, rows)) => (Ok(snapshot), start, rows),
            Err(refused) => (Err(refused), 0, Vec::new()),
        };
        match reader.read(&snapshot) {
            Reply::Heard(heard) => {
                let grid = terminal.grid();
                let left = grid.main.rotated[gone.0..].to_vec();
                let erased = grid.main.erased - gone.1;
                *gone = (grid.main.rotated.len(), grid.main.erased);
                return Observation {
                    alternate: grid.alternate.is_some(),
                    start,
                    rows,
                    main: terminal.document(),
                    screen_top: grid.main.top,
                    continued: grid.main.continued,
                    rotated: left,
                    erased,
                    heard,
                };
            }
            Reply::Again(next) => {
                again += 1;
                assert!(
                    again <= AGAIN_LIMIT,
                    "the reader asked to read again more than {AGAIN_LIMIT} times at one point"
                );
                query = next;
            }
        }
    }
}

/// A way a reader broke an invariant. Reads and rows are numbered from 0;
/// a coordinate is a row's place in the main buffer, from its first row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Violation {
    /// A new line was neither reported with its new text nor withheld.
    Missed {
        /// The read.
        read: usize,
        /// The coordinate of the line's first row.
        coordinate: usize,
        /// Its text.
        text: String,
    },
    /// A line was withheld for a reason whose premise is false, or held
    /// and never resolved.
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
    /// A line was reported as a line or in a summary while its text stayed
    /// on the main screen since it was reported before.
    Repeated {
        /// The read.
        read: usize,
        /// The row it starts on.
        row: usize,
        /// Its text.
        text: String,
    },
    /// A new line was reported after a new line below it.
    OutOfOrder {
        /// The read.
        read: usize,
        /// The row it starts on.
        row: usize,
        /// Its text.
        text: String,
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
    /// A row whose text is on exactly one row of each of two reads was
    /// mapped somewhere other than the row with that text.
    Mismapped {
        /// The read the row moved into.
        read: usize,
        /// The row in the previous read.
        row: usize,
        /// The row with its text.
        went: usize,
        /// The row the reader said it became.
        said: Option<usize>,
    },
    /// The review position left its text, though that text is on exactly
    /// one row of the read and the screen did not switch.
    ReviewMoved {
        /// The read.
        read: usize,
        /// The row with the review position's text.
        went: usize,
        /// Where the reader said its review position is.
        said: Option<isize>,
    },
    /// A held line was spoken as last read, but what was said covers the
    /// change of no hold still unresolved, and ends no line an earlier read
    /// showed.
    Unheld {
        /// The read.
        read: usize,
        /// What it said.
        said: String,
    },
    /// The review position is on no row of the document: below the read's
    /// last row, or above the document's first.
    ReviewOutside {
        /// The read.
        read: usize,
        /// The row the reader named, counted from the read's first row.
        row: isize,
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

/// A line with text of the main screen at a read.
#[derive(Clone, Debug)]
struct Line {
    /// Its first row's identity.
    first: RowId,
    /// Its rows' coordinates.
    rows: Range<usize>,
    /// Its place: its first row's row on the screen.
    place: isize,
    /// Its text, padding left out.
    text: String,
    /// Its text without the rows of only spaces at its start, by which it
    /// is new or not.
    key: String,
    /// Whether its start left the buffer.
    cut: bool,
}

/// The lines with text of a read's main screen, top to bottom.
#[derive(Clone, Debug, Default)]
struct Lines {
    list: Vec<Line>,
    /// The line starting at each place.
    at: HashMap<isize, usize>,
    /// The lines with each text.
    texts: HashMap<String, Vec<usize>>,
}

impl Lines {
    /// The lines of a read's main screen.
    fn of(read: &Observation) -> Self {
        Self::from_rows(&read.main, 0, read.screen_top, read.continued)
    }

    /// The lines a read on the alternate screen showed, its screen's top at
    /// its first row.
    fn alternate(read: &Observation) -> Self {
        Self::from_rows(&read.rows, read.start, 0, false)
    }

    /// The lines of `main`, rows from coordinate `from`, the screen's top at
    /// coordinate `top`; `continued` when the first row continues a line
    /// whose start left the buffer.
    fn from_rows(main: &[Seen], from: usize, top: usize, continued: bool) -> Self {
        let top = isize::try_from(top).unwrap_or(isize::MAX);
        let from = isize::try_from(from).unwrap_or(isize::MAX);
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
                // Rows of only spaces at the line's start show nothing.
                let blank = main[first..=at]
                    .iter()
                    .take_while(|seen| seen.text.trim_end().is_empty())
                    .map(|seen| seen.text.len())
                    .sum::<usize>();
                lines.push(Line {
                    key: text[blank..].to_owned(),
                    first: main[first].id,
                    rows: first..at + 1,
                    place: isize::try_from(first).unwrap_or(isize::MAX) + from - top,
                    text: text.to_owned(),
                    cut: first == 0 && continued,
                });
            }
            first = at + 1;
        }
        let at = lines
            .iter()
            .enumerate()
            .map(|(number, line)| (line.place, number))
            .collect();
        let mut texts: HashMap<String, Vec<usize>> = HashMap::new();
        for (number, line) in lines.iter().enumerate() {
            texts.entry(line.text.clone()).or_default().push(number);
        }
        Self {
            list: lines,
            at,
            texts,
        }
    }

    /// The line holding the row at `coordinate`.
    fn holding(&self, coordinate: usize) -> Option<usize> {
        let after = self
            .list
            .partition_point(|line| line.rows.start <= coordinate);
        after
            .checked_sub(1)
            .filter(|&number| self.list[number].rows.contains(&coordinate))
    }

    /// The text of the line starting at `place`.
    fn text_at(&self, place: isize) -> Option<&str> {
        self.at
            .get(&place)
            .map(|&number| self.list[number].text.as_str())
    }

    /// The line whose rows cover `place`.
    fn covering(&self, place: isize) -> Option<usize> {
        let number = self
            .list
            .partition_point(|line| line.place <= place)
            .checked_sub(1)?;
        let line = &self.list[number];
        let end = line.place + isize::try_from(line.rows.len()).unwrap_or(isize::MAX);
        (place < end).then_some(number)
    }

    /// Whether a line with text starts at coordinate `coordinate`.
    fn starts_at(&self, coordinate: usize) -> bool {
        self.holding(coordinate)
            .is_some_and(|number| self.list[number].rows.start == coordinate)
    }

    /// How many lines have `text`.
    fn count(&self, text: &str) -> usize {
        self.texts.get(text).map_or(0, Vec::len)
    }

    /// The lines with `text`.
    fn with(&self, text: &str) -> &[usize] {
        self.texts.get(text).map_or(&[], Vec::as_slice)
    }
}

/// A line a reader holds, waiting to be reported or replaced.
struct Hold {
    read: usize,
    row: usize,
    /// The identity of the line's first row, to know whether it left the
    /// buffer.
    first: RowId,
    text: String,
    /// The text the line's row had at the last read on the main screen
    /// before the hold, followed to where it was, which a redrawn row's
    /// speech resolving it covers the change from.
    before: Option<String>,
    reason: Reason,
}

/// What the check carries from read to read.
struct Check<'a> {
    reads: &'a [Observation],
    /// Whether the main screen's history went away, as when the alternate
    /// screen opens or the history is erased, which a reader cannot tell
    /// apart (`docs/design/terminal-model.md`, section 5.8), and has not
    /// come back.
    aside: bool,
    /// Each read's lines; `None` for a read on the alternate screen.
    lines: Vec<Option<Rc<Lines>>>,
    /// The lines the last read showed on the alternate screen, when it was
    /// up and no read showed the switch to it, since the main screen had no
    /// history: a reader cannot tell it from the main screen (section 5.8),
    /// so the next read is judged against it.
    alternate: Option<Rc<Lines>>,
    /// Every text a line had at the reads checked so far: on the main
    /// screen, its lines; on the alternate screen, the lines the read
    /// showed.
    shown: HashSet<String>,
    /// The last read on the main screen checked so far.
    last: Option<usize>,
    /// What was reported of each line of that read, from its start.
    heard: Vec<String>,
    /// For each text, how many lines with it were reported while it stayed
    /// on the main screen.
    covered: HashMap<String, usize>,
    /// Where the line changed from one text to another at a read checked
    /// so far: its first row's coordinate plus the rows the main buffer had
    /// discarded from its front by that read, which stays with the row as
    /// the history grows, rotates or is erased.
    changed: HashSet<usize>,
    /// The lines of the read checked that moved with their neighbours since
    /// the last read on the main screen (see the module's documentation):
    /// each line's first row's coordinate, and the coordinate it is
    /// followed to at that read.
    block: HashMap<usize, usize>,
    /// The rows of the last read on the main screen with each text that is
    /// on exactly one of its rows.
    unique_last: HashMap<&'a str, usize>,
    /// What was said of each text of a line withheld as having lost text,
    /// while a line with that text is shown (see [`Reason::LostText`]).
    lost: HashMap<String, Vec<String>>,
    /// The rows the main buffer rotated out up to the read checked.
    rotated: HashSet<RowId>,
    /// How many rows the main buffer discarded from its front, rotated out
    /// or erased with the history, up to the read checked.
    discarded: usize,
    /// How many it had discarded up to the last read on the main screen.
    discarded_at_last: usize,
    holds: Vec<Hold>,
    violations: Vec<Violation>,
}

/// What one read's check accumulates, for each line of a read on the main
/// screen.
struct ReadState {
    /// Whether the line is new.
    new: Vec<bool>,
    /// Whether it is reported with its new text or withheld.
    accounted: Vec<bool>,
    /// Whether a report said its new text.
    said: Vec<bool>,
    /// What was reported of it, from its start.
    heard: Vec<String>,
    /// The redrawn rows' speech: the line, the row named, and what was
    /// said.
    redrawn: Vec<(usize, usize, String)>,
    /// The lines withheld as having lost text whose premise holds, and what
    /// was said of each.
    lost: Vec<(usize, Vec<String>)>,
    /// The lowest coordinate a new line reported next may have.
    latest: usize,
}

impl Check<'_> {
    /// The lines of read `index`, which is on the main screen.
    fn lines_of(&self, index: usize) -> &Lines {
        self.lines[index]
            .as_ref()
            .expect("a read on the main screen")
    }

    /// The lines of read `index`, which is on the main screen, shared.
    fn shared(&self, index: usize) -> Rc<Lines> {
        Rc::clone(
            self.lines[index]
                .as_ref()
                .expect("a read on the main screen"),
        )
    }

    /// The lines of the last read on the main screen before this one, or of
    /// the alternate screen when no read showed the switch to it.
    fn last_lines(&self) -> Option<&Lines> {
        self.alternate
            .as_deref()
            .or_else(|| self.last.and_then(|last| self.lines[last].as_deref()))
    }

    /// The lines of read `index`, on the main screen, that moved with their
    /// neighbours since the last read on the main screen, as
    /// [`Check::block`] holds them (see the module's documentation).
    fn block_moves(&self, index: usize, unique: &HashMap<&str, usize>) -> HashMap<usize, usize> {
        let mut block = HashMap::new();
        let Some(last) = self.last.filter(|_| self.alternate.is_none()) else {
            return block;
        };
        let (old, new) = (&self.reads[last].main, &self.reads[index].main);
        let (old_lines, new_lines) = (self.lines_of(last), self.lines_of(index));
        let signed = |at: usize| isize::try_from(at).unwrap_or(isize::MAX);
        // Following by the history alone: the coordinate at the last read is
        // this read's moved down by the rows discarded since.
        let history = signed(self.discarded - self.discarded_at_last);
        // How far a row with text moved, as its coordinate at the last read
        // less its coordinate now, when its text is on exactly one row of
        // each read and it moved otherwise than by the history alone.
        let moved = |row: usize| {
            let text = row_text(new, row);
            unique.get(text)?;
            let &then = self.unique_last.get(text)?;
            let by = signed(then) - signed(row);
            (by != history).then_some(by)
        };
        // The row of the last read `by` from `row`, when it is one.
        let then = |row: usize, by: isize| {
            usize::try_from(signed(row) + by)
                .ok()
                .filter(|&then| then < old.len())
        };
        let blank_then = |rows: Range<usize>, by: isize| {
            rows.into_iter()
                .all(|row| then(row, by).is_some_and(|then| row_text(old, then).is_empty()))
        };
        // Whether the row of the last read at `row` by the history alone
        // left that place: it had no text, or its text moved.
        let vacated = |row: usize| {
            then(row, history).is_none_or(|then| {
                let text = row_text(old, then);
                text.is_empty()
                    || (self.unique_last.contains_key(text)
                        && unique
                            .get(text)
                            .is_some_and(|&now| signed(now) + history != signed(then)))
            })
        };
        for line in &new_lines.list {
            let start = line.rows.start;
            // The nearest row with text above the line, over blank rows.
            let above = || {
                let above = (0..start)
                    .rev()
                    .find(|&row| !row_text(new, row).is_empty())?;
                let by = moved(above)?;
                blank_then(above + 1..start, by).then_some(by)
            };
            // The nearest row with text below the line's last row, over
            // blank rows, and the rows between it and the end of the line
            // that started where this line's first row was.
            let below = || {
                let below =
                    (line.rows.end..new.len()).find(|&row| !row_text(new, row).is_empty())?;
                let by = moved(below)?;
                let was = old_lines.holding(then(start, by)?)?;
                let end = old_lines.list[was].rows.end;
                let below_then = then(below, by)?;
                (below_then >= end && (end..below_then).all(|row| row_text(old, row).is_empty()))
                    .then_some(by)
            };
            // A line rewritten where it was is not followed by its
            // neighbours' move.
            let by =
                moved(start).or_else(|| vacated(start).then(|| above().or_else(below)).flatten());
            if let Some(by) = by
                && let Some(then) = then(start, by)
                && old_lines.starts_at(then)
            {
                block.insert(start, then);
            }
        }
        block
    }

    /// Whether `said`, speech of a redrawn row now showing `text`, says
    /// what it gained beyond what was said of a line with `old`, a text of
    /// a line withheld as having lost text (see [`Reason::LostText`]).
    fn regained(&self, old: &str, text: &str, said: &str) -> bool {
        self.lost.get(old).is_some_and(|before| {
            before
                .iter()
                .any(|before| covers_change(before, text, said))
        })
    }

    /// Whether `growth` completes `text` from what was said of a line
    /// withheld as having lost text (see [`Reason::LostText`]).
    fn regrown(&self, text: &str, growth: &str) -> bool {
        self.lost
            .values()
            .flatten()
            .any(|said| !said.is_empty() && format!("{said}{growth}") == text)
    }

    /// Whether `line` is new: no read checked so far showed its text.
    fn is_new(&self, line: &Line) -> bool {
        !self.shown.contains(&line.key)
            && (!line.cut || !self.shown.iter().any(|shown| shown.ends_with(&line.key)))
    }

    /// What was reported of the line `line` grew from: the longest of
    /// what was reported of the lines of the last read on the main screen
    /// whose text begins `line`'s and is shorter.
    fn grown_from(&self, line: &Line) -> String {
        let Some(last) = self.last_lines() else {
            return String::new();
        };
        let grown = line
            .text
            .char_indices()
            .skip(1)
            .flat_map(|(end, _)| last.with(&line.text[..end]))
            .map(|&number| self.heard[number].clone());
        // A line whose start left the buffer grew from what is left of a
        // line reported whole: the longest end of it that begins this one.
        let cut = last
            .list
            .iter()
            .zip(&self.heard)
            .filter(|(old, heard)| line.cut && **heard == old.text)
            .filter_map(|(old, _)| {
                old.text
                    .char_indices()
                    .map(|(start, _)| &old.text[start..])
                    .find(|rest| rest.len() < line.text.len() && line.text.starts_with(rest))
                    .map(str::to_owned)
            });
        grown.chain(cut).max_by_key(String::len).unwrap_or_default()
    }

    /// What was reported of `line`, a line that is not new: what was
    /// reported of a line with its text at the last read on the main
    /// screen, the one at its place, followed (see [`Check::followed`]),
    /// first, or else its whole text.
    fn carried(&self, line: &Line) -> String {
        let Some(last) = self.last_lines() else {
            return line.text.clone();
        };
        let place = self.followed(line);
        let same = last.with(&line.text);
        same.iter()
            .find(|&&number| last.list[number].place == place)
            .or_else(|| same.first())
            .map_or_else(|| line.text.clone(), |&number| self.heard[number].clone())
    }

    /// The coordinate at the last read on the main screen of the first row
    /// of `line`, a line of the read checked, when the buffer held that row
    /// then: its coordinate moved down by the rows the buffer discarded
    /// from its front since, rotated out or erased with the history, which
    /// is where the row was, as a reader follows it by the coordinates the
    /// providers report and the rotation it measures (section 4.3), or
    /// where it was before it moved with its neighbours. `None` for a row
    /// written since, and when the last read is the alternate screen's.
    fn held_at_last(&self, line: &Line) -> Option<usize> {
        let last = self.last.filter(|_| self.alternate.is_none())?;
        if let Some(&then) = self.block.get(&line.rows.start) {
            return Some(then);
        }
        let then = line.rows.start + (self.discarded - self.discarded_at_last);
        (then < self.reads[last].main.len()).then_some(then)
    }

    /// The place at the last read on the main screen of `line`, a line of
    /// the read checked: where its first row was then (see
    /// [`Check::held_at_last`]), counted from that read's screen's top,
    /// and otherwise its place, as for a row written since, which a reader
    /// sees only on the screen there.
    fn followed(&self, line: &Line) -> isize {
        match (self.held_at_last(line), self.last) {
            (Some(then), Some(last)) => {
                isize::try_from(then).unwrap_or(isize::MAX)
                    - isize::try_from(self.reads[last].screen_top).unwrap_or(isize::MAX)
            }
            _ => line.place,
        }
    }

    /// The key in `changed` of the row `line`, a line of the read checked,
    /// is compared with at the last read on the main screen: the row there
    /// at its place, followed (see [`Check::followed`]), or the line
    /// itself when the last read is the alternate screen's.
    fn followed_key(&self, line: &Line) -> usize {
        match self.last.filter(|_| self.alternate.is_none()) {
            Some(last) => {
                let top = isize::try_from(self.reads[last].screen_top).unwrap_or(isize::MAX);
                usize::try_from(self.followed(line) + top)
                    .map_or(usize::MAX, |then| then + self.discarded_at_last)
            }
            None => line.rows.start + self.discarded,
        }
    }

    /// `number`, a line of read `index`, reported with its new text, as a
    /// line or in a summary when `counted`, naming row `row`.
    fn say(
        &mut self,
        index: usize,
        row: usize,
        number: usize,
        counted: bool,
        state: &mut ReadState,
    ) {
        let line = self.lines_of(index).list[number].clone();
        let shown = self.lines_of(index).count(&line.text);
        let covered = self.covered.entry(line.text.clone()).or_default();
        if *covered >= shown {
            if counted {
                self.violations.push(Violation::Repeated {
                    read: index,
                    row,
                    text: line.text,
                });
                return;
            }
        } else {
            *covered += 1;
        }
        if counted && state.new[number] && !state.accounted[number] {
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
        state.said[number] = true;
        state.heard[number] = line.text;
    }
}

/// Every violation of the invariants by the reads of a run: in the order of
/// the reads, each read's reports in order, then what it withheld, then
/// the lines it missed, then its mapping, then its review position; and
/// then the holds no later read resolved.
#[must_use]
pub fn check(reads: &[Observation]) -> Vec<Violation> {
    let lines = reads
        .iter()
        .map(|read| (!read.alternate).then(|| Rc::new(Lines::of(read))))
        .collect();
    let mut check = Check {
        reads,
        lines,
        shown: HashSet::new(),
        last: None,
        heard: Vec::new(),
        covered: HashMap::new(),
        changed: HashSet::new(),
        block: HashMap::new(),
        unique_last: HashMap::new(),
        lost: HashMap::new(),
        rotated: HashSet::new(),
        discarded: 0,
        discarded_at_last: 0,
        aside: false,
        alternate: None,
        holds: Vec::new(),
        violations: Vec::new(),
    };
    for index in 0..reads.len() {
        check_read(&mut check, index);
    }
    let Check {
        holds,
        mut violations,
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
    violations
}

/// Checks read `index`.
fn check_read(check: &mut Check, index: usize) {
    let reads = check.reads;
    let read = &reads[index];
    check.rotated.extend(read.rotated.iter().copied());
    check.discarded += read.rotated.len() + read.erased;
    let main = check.lines[index].is_some();
    let mut state = ReadState {
        new: Vec::new(),
        accounted: Vec::new(),
        said: Vec::new(),
        heard: Vec::new(),
        redrawn: Vec::new(),
        lost: Vec::new(),
        latest: 0,
    };
    check.block.clear();
    if main {
        let unique = unique_rows(&read.main);
        check.block = check.block_moves(index, &unique);
        let lines = check.shared(index);
        for line in &lines.list {
            let new = check.is_new(line);
            state.new.push(new);
            state.heard.push(if new {
                check.grown_from(line)
            } else {
                check.carried(line)
            });
        }
        state.accounted = vec![false; lines.list.len()];
        state.said = vec![false; lines.list.len()];
        // A line of the last read on the main screen that the full history
        // rotated out since, or that was erased with the history, took its
        // text off the screen, so its report no longer covers a line with
        // that text printed again. The history is erased from the buffer's
        // front, so the rows erased since are the last read's first rows.
        let erased: usize = match (&check.alternate, check.last) {
            (None, Some(last)) => reads[last + 1..=index].iter().map(|read| read.erased).sum(),
            _ => 0,
        };
        let left: Vec<String> = check
            .last_lines()
            .map(|last| {
                last.list
                    .iter()
                    .filter(|line| check.rotated.contains(&line.first) || line.rows.end <= erased)
                    .map(|line| line.text.clone())
                    .collect()
            })
            .unwrap_or_default();
        for text in left {
            if let Some(covered) = check.covered.get_mut(&text) {
                *covered = covered.saturating_sub(1);
            }
        }
        let now = check.shared(index);
        uncover(&mut check.covered, |text| now.count(text));
        for spoken in &read.heard.spoken {
            check_spoken(check, index, spoken, &mut state);
        }
        check_changes(check, index, &mut state);
        finish_main(check, index, &state);
        check.unique_last = unique;
    } else {
        // While the alternate screen is up the main screen's lines are not
        // on the screen, but those the alternate screen also shows.
        let shown = alternate_lines(&read.rows);
        uncover(&mut check.covered, |text| {
            shown.get(text).copied().unwrap_or_default()
        });
        // The alternate screen replaces the main screen's text on the
        // screen, so a hold whose text it does not show is resolved.
        check.holds.retain(|hold| shown.contains_key(&hold.text));
        // What the alternate screen shows counts as shown: a reader cannot
        // always tell the screens apart (section 5.8), so a line it showed
        // here that a program prints again on the main screen is not new.
        check.shown.extend(alternate_keys(&read.rows));
        // With no history on the main screen no read shows the switch, so
        // what this read showed is what the next is judged against, as if
        // all of it was heard.
        let unseen = check.last.is_none_or(|last| reads[last].screen_top == 0);
        check.alternate = unseen.then(|| Rc::new(Lines::alternate(read)));
        if let Some(lines) = &check.alternate {
            check.heard = lines.list.iter().map(|line| line.text.clone()).collect();
        }
        // Nothing on the alternate screen must be accounted for; only
        // what is reported is checked against the read.
        for spoken in &read.heard.spoken {
            check_spoken(check, index, spoken, &mut state);
        }
        for withheld in &read.heard.withheld {
            if !starts_line(&read.rows, withheld.row) {
                check.violations.push(Violation::NotALine {
                    read: index,
                    row: withheld.row,
                });
            }
        }
    }
    let previous = index.checked_sub(1).map(|previous| &reads[previous]);
    let switched = previous.is_some_and(|previous| {
        let opened = !at_top(previous) && at_top(read);
        let closed = check.aside && at_top(previous) && !at_top(read);
        if opened {
            check.aside = true;
        } else if closed {
            check.aside = false;
        }
        previous.alternate != read.alternate || opened || closed
    });
    if let Some(previous) = previous.filter(|_| !switched) {
        if let Some(moved) = &read.heard.moved {
            check_moved(&mut check.violations, index, previous, read, moved);
        }
        check_review(&mut check.violations, index, Some(previous), read);
    } else {
        check_review(&mut check.violations, index, None, read);
    }
}

/// Whether the screen of `read` starts at the document's start, with
/// nothing above it: so the alternate screen, and the main screen before
/// it has a history or after the history is erased.
fn at_top(read: &Observation) -> bool {
    read.alternate || read.screen_top == 0
}

/// Lines no longer on the screen are no longer covered: each text's
/// count is at most `count` gives for it.
fn uncover(covered: &mut HashMap<String, usize>, count: impl Fn(&str) -> usize) {
    covered.retain(|text, covered| {
        *covered = (*covered).min(count(text));
        *covered > 0
    });
}

/// How many lines with text of `rows`, an alternate screen's rows, have
/// each text.
fn alternate_lines(rows: &[Seen]) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut text = String::new();
    for (at, seen) in rows.iter().enumerate() {
        text.push_str(&seen.text);
        if seen.wraps && at + 1 < rows.len() {
            continue;
        }
        let line = std::mem::take(&mut text);
        let line = line.trim_end();
        if !line.is_empty() {
            *counts.entry(line.to_owned()).or_default() += 1;
        }
    }
    counts
}

/// The texts by which the lines with text of `rows`, an alternate
/// screen's rows, are new or not: each line's text without the rows of only
/// spaces at its start, as a main screen's line's.
fn alternate_keys(rows: &[Seen]) -> Vec<String> {
    let mut keys = Vec::new();
    let mut first = 0;
    for (at, seen) in rows.iter().enumerate() {
        if seen.wraps && at + 1 < rows.len() {
            continue;
        }
        let text: String = rows[first..=at]
            .iter()
            .map(|seen| seen.text.as_str())
            .collect();
        let text = text.trim_end();
        if !text.is_empty() {
            let blank = rows[first..=at]
                .iter()
                .take_while(|seen| seen.text.trim_end().is_empty())
                .map(|seen| seen.text.len())
                .sum::<usize>();
            keys.push(text[blank..].to_owned());
        }
        first = at + 1;
    }
    keys
}

/// The holds a report at read `index` or the main screen's text resolves:
/// a report said the held text's line (a redrawn row's speech covering its
/// change from the text its place had before the hold), or no line has the
/// held text and its row did not leave the buffer as the full history
/// rotated, which takes it away with its text; a row written over, erased
/// or deleted had its text replaced.
fn resolve_holds(check: &mut Check, index: usize, state: &ReadState) {
    let lines = check.lines[index]
        .as_ref()
        .expect("a read on the main screen");
    let rotated = &check.rotated;
    let said: HashSet<&str> = lines
        .list
        .iter()
        .zip(&state.said)
        .filter(|(_, said)| **said)
        .map(|(line, _)| line.text.as_str())
        .collect();
    check.holds.retain(|hold| {
        let redrawn = state.redrawn.iter().any(|(number, _, text)| {
            let line = &lines.list[*number].text;
            *line == hold.text
                && hold
                    .before
                    .as_ref()
                    .is_some_and(|before| before != line && covers_change(before, line, text))
        });
        let resolved = said.contains(hold.text.as_str())
            || redrawn
            || (lines.count(&hold.text) == 0 && !rotated.contains(&hold.first));
        !resolved
    });
}

/// The lines read `index` missed, and what the next read on the main
/// screen is checked against.
fn finish_main(check: &mut Check, index: usize, state: &ReadState) {
    let lines = check.shared(index);
    for (number, line) in lines.list.iter().enumerate() {
        if state.new[number] && !state.accounted[number] {
            check.violations.push(Violation::Missed {
                read: index,
                coordinate: line.rows.start,
                text: line.text.clone(),
            });
        }
    }
    // Where a line changed moves with a line that moved with its
    // neighbours, and the row it moved to keeps only that line's.
    let moves: Vec<(usize, usize)> = check
        .block
        .iter()
        .map(|(&now, &then)| (then + check.discarded_at_last, now + check.discarded))
        .collect();
    let carried: Vec<usize> = moves
        .iter()
        .filter(|(then, _)| check.changed.contains(then))
        .map(|&(_, now)| now)
        .collect();
    for (then, now) in &moves {
        check.changed.remove(then);
        check.changed.remove(now);
    }
    check.changed.extend(carried);
    // What was said of a line that lost text is kept while a line with its
    // text is shown.
    check.lost.retain(|text, _| lines.count(text) > 0);
    for (number, said) in &state.lost {
        check
            .lost
            .entry(lines.list[*number].text.clone())
            .or_default()
            .extend(said.iter().cloned());
    }
    let changes: Vec<usize> = match check.last_lines() {
        Some(last) => lines
            .list
            .iter()
            .filter(|line| {
                last.text_at(check.followed(line))
                    .is_some_and(|before| before != line.text)
            })
            .map(|line| line.rows.start + check.discarded)
            .collect(),
        None => Vec::new(),
    };
    check.changed.extend(changes);
    check.discarded_at_last = check.discarded;
    check
        .shown
        .extend(lines.list.iter().map(|line| line.key.clone()));
    check.last = Some(index);
    check.alternate = None;
    check.heard.clone_from(&state.heard);
}

/// Whether `row` of `rows` starts a line.
fn starts_line(rows: &[Seen], row: usize) -> bool {
    row < rows.len() && (row == 0 || !rows[row - 1].wraps)
}

/// The number of the line of read `index`, on the main screen, holding the
/// read's row `row`, a row that starts a line of the read.
fn line_number(check: &Check, index: usize, row: usize) -> Option<usize> {
    check.lines[index]
        .as_ref()?
        .holding(check.reads[index].start + row)
}

/// Checks one report of read `index`.
fn check_spoken(check: &mut Check, index: usize, spoken: &Spoken, state: &mut ReadState) {
    let rows = &check.reads[index].rows;
    let (row, text) = match spoken {
        Spoken::Held { text } => {
            // The held line itself is reported: what is said covers its
            // change from what its row had before the hold.
            let held = check.holds.iter().position(|hold| {
                !text.is_empty()
                    && covers_change(hold.before.as_deref().unwrap_or_default(), &hold.text, text)
            });
            match held {
                Some(at) => {
                    check.holds.remove(at);
                }
                // A line held that was not new needed no hold, but what is
                // said of it is still the end of a line a read showed.
                None if !text.is_empty()
                    && check
                        .shown
                        .iter()
                        .any(|shown| shown.ends_with(text.as_str())) => {}
                None => check.violations.push(Violation::Unheld {
                    read: index,
                    said: text.clone(),
                }),
            }
            return;
        }
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
                if let Some(number) = line_number(check, index, row) {
                    check.say(index, row, number, true, state);
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
    let Some(number) = line_number(check, index, row) else {
        return;
    };
    let line = check.lines_of(index).list[number].clone();
    match spoken {
        Spoken::Line { .. } => check.say(index, row, number, true, state),
        Spoken::Growth { .. } => {
            let before = &state.heard[number];
            let grown = format!("{before}{text}");
            if !before.is_empty() && *before != line.text && line.text.starts_with(&grown) {
                if grown == line.text {
                    check.say(index, row, number, false, state);
                } else {
                    state.heard[number] = grown;
                }
            } else if check.regrown(&line.text, text) {
                // A line that lost text and gained it back, and more.
                check.say(index, row, number, false, state);
            }
        }
        Spoken::Redrawn { .. } => state.redrawn.push((number, row, text.clone())),
        Spoken::Summary { .. } | Spoken::Held { .. } => {}
    }
}

/// Whether `said`, a part of `after`, covers every character of `after`
/// from the first that differs from `before` to the last, counted from the
/// end, or touches where characters were taken out when `after` only lost
/// some.
fn covers_change(before: &str, after: &str, said: &str) -> bool {
    let (before, after, said): (Vec<char>, Vec<char>, Vec<char>) = (
        before.chars().collect(),
        after.chars().collect(),
        said.chars().collect(),
    );
    let prefix = before
        .iter()
        .zip(&after)
        .take_while(|(a, b)| a == b)
        .count();
    let longest_suffix = before.len().min(after.len()) - prefix;
    let suffix = before
        .iter()
        .rev()
        .zip(after.iter().rev())
        .take(longest_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let end = after.len() - suffix;
    (0..=after.len().saturating_sub(said.len())).any(|start| {
        after[start..].starts_with(&said) && start <= prefix && start + said.len() >= end
    })
}

/// Whether `old` and `new` differ in exactly one character, neither a
/// letter or a digit.
fn spinner_step(old: &str, new: &str) -> bool {
    let (old, new): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    old.len() == new.len() && {
        let differing: Vec<(char, char)> = old
            .iter()
            .zip(&new)
            .filter(|(a, b)| a != b)
            .map(|(&a, &b)| (a, b))
            .collect();
        matches!(differing[..], [(a, b)] if !a.is_alphanumeric() && !b.is_alphanumeric())
    }
}

/// `text` with its spaces left out.
fn without_spaces(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// A change a line of the last read on the main screen that is no longer
/// shown as often must justify, one each.
enum Claim {
    /// A redrawn row's speech of a new line: the line, the row named, and
    /// what was said.
    Redrawn(usize, usize, String),
    /// A spinner or an unfinished word withheld, and the line.
    Withheld(Withheld, usize),
}

/// The vanished lines' texts (see the module's documentation), one for
/// each time a text of the last read on the main screen is no longer shown
/// in `lines`, a read's.
fn vanished(check: &Check, lines: &Lines) -> Vec<String> {
    check.last_lines().map_or_else(Vec::new, |last| {
        let mut texts: Vec<(&String, usize)> = last
            .texts
            .iter()
            .map(|(text, numbers)| (text, numbers.len()))
            .collect();
        texts.sort();
        texts
            .into_iter()
            .flat_map(|(text, count)| {
                std::iter::repeat_n(text.clone(), count.saturating_sub(lines.count(text)))
            })
            .collect()
    })
}

/// Checks the redrawn rows' speech and what read `index` withheld, each
/// reason's premise, and resolves the holds of earlier reads; holds made
/// now wait for later reads.
fn check_changes(check: &mut Check, index: usize, state: &mut ReadState) {
    let reads = check.reads;
    let read = &reads[index];
    let lines = check.shared(index);
    let mut claims: Vec<Claim> = state
        .redrawn
        .iter()
        .filter(|(number, _, _)| state.new[*number])
        .map(|(number, row, text)| Claim::Redrawn(*number, *row, text.clone()))
        .collect();
    // Withholdings judged at once: the withholding, the line, the premise.
    let mut settled: Vec<(Withheld, usize, bool)> = Vec::new();
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
        if !state.new[number] {
            continue;
        }
        let premise = match withheld.reason {
            Reason::UnfinishedWord | Reason::Spinner => {
                claims.push(Claim::Withheld(*withheld, number));
                continue;
            }
            reason => premise(check, &lines, &lines.list[number], reason),
        };
        settled.push((*withheld, number, premise));
    }
    if !claims.is_empty() {
        let vanished = vanished(check, &lines);
        let edges: Vec<Vec<usize>> = claims
            .iter()
            .map(|claim| {
                vanished
                    .iter()
                    .enumerate()
                    .filter(|(_, old)| match claim {
                        Claim::Redrawn(number, _, said) => {
                            let text = &lines.list[*number].text;
                            // A line that lost text is judged by what was
                            // said of it.
                            covers_change(old, text, said) || check.regained(old, text, said)
                        }
                        Claim::Withheld(withheld, number) => {
                            let text = &lines.list[*number].text;
                            if withheld.reason == Reason::Spinner {
                                spinner_step(old, text)
                            } else {
                                text.len() > old.len() && text.starts_with(old.as_str())
                            }
                        }
                    })
                    .map(|(slot, _)| slot)
                    .collect()
            })
            .collect();
        let matched = matching(&edges, vanished.len());
        for (claim, matched) in claims.into_iter().zip(matched) {
            match claim {
                Claim::Redrawn(number, row, _) => {
                    if matched.is_some() {
                        check.say(index, row, number, false, state);
                    }
                }
                Claim::Withheld(withheld, number) => {
                    settled.push((withheld, number, matched.is_some()));
                }
            }
        }
    }
    resolve_holds(check, index, state);
    for (withheld, number, premise) in settled {
        // A line whose silence needs no later report counts as heard
        // whole, so what grows from it is its growth.
        if premise && matches!(withheld.reason, Reason::Spinner | Reason::LostText) {
            state.heard[number].clone_from(&lines.list[number].text);
        }
        if premise && withheld.reason == Reason::LostText {
            let said = lost_from(check, &lines, &lines.list[number]);
            state.lost.push((number, said));
        }
        settle(
            check,
            index,
            withheld,
            &lines.list[number],
            premise,
            state.said[number],
        );
    }
}

/// Whether the premise of `reason`, a reason judged at once, holds for
/// `line`, a new line of `lines`.
fn premise(check: &Check, lines: &Lines, line: &Line, reason: Reason) -> bool {
    let Some(last) = check.last_lines() else {
        return false;
    };
    match reason {
        // A new line's text differs from what its row had, the row followed
        // to where it was.
        Reason::Volatile => {
            last.text_at(check.followed(line)).is_some()
                && check.changed.contains(&check.followed_key(line))
        }
        Reason::LostText => !lost_from(check, lines, line).is_empty(),
        Reason::UnfinishedWord
        | Reason::Spinner
        | Reason::HeldForKeys
        | Reason::HeldTyping
        | Reason::KeyAnswer => false,
    }
}

/// What was reported of each line of the last read on the main screen that
/// `line`, a new line of `lines`, only lost text from: compared without
/// spaces, the line that covered its row, followed to where it was, or one
/// no longer shown as often, as the parts of a line split in two, that
/// contains its text and is longer. Empty when it gained or changed text.
fn lost_from(check: &Check, lines: &Lines, line: &Line) -> Vec<String> {
    let Some(last) = check.last_lines() else {
        return Vec::new();
    };
    let now = without_spaces(&line.text);
    let shorter = |before: &str| {
        let before = without_spaces(before);
        before.len() > now.len() && before.contains(&now)
    };
    let covering = last
        .covering(check.followed(line))
        .filter(|&number| shorter(&last.list[number].text));
    let vanished = last
        .texts
        .iter()
        .filter(|(before, numbers)| numbers.len() > lines.count(before) && shorter(before))
        .flat_map(|(_, numbers)| numbers.iter().copied());
    covering
        .into_iter()
        .chain(vanished)
        .map(|number| check.heard[number].clone())
        .collect()
}

/// A withholding at read `index` of `line` whose premise is `premise`: a
/// violation when false, and a hold to resolve when true, a hold, and the
/// line not `said` with its new text at the same read, as a reader says a
/// held row at once when it has scrolled above the screen.
fn settle(
    check: &mut Check,
    index: usize,
    withheld: Withheld,
    line: &Line,
    premise: bool,
    said: bool,
) {
    if !premise {
        check.violations.push(Violation::FalseReason {
            read: index,
            row: withheld.row,
            text: line.text.clone(),
            reason: withheld.reason,
        });
    } else if !said && matches!(withheld.reason, Reason::Volatile | Reason::UnfinishedWord) {
        let before = check
            .last_lines()
            .and_then(|last| last.text_at(check.followed(line)))
            .map(str::to_owned);
        check.holds.push(Hold {
            read: index,
            row: withheld.row,
            first: line.first,
            text: line.text.clone(),
            before,
            reason: withheld.reason,
        });
    }
}

/// A largest matching of claims to slots, each claim's slots listed in
/// `edges`: the slot each claim is matched to, the earlier claims
/// preferred when not every claim can be matched.
fn matching(edges: &[Vec<usize>], slots: usize) -> Vec<Option<usize>> {
    fn augment(
        claim: usize,
        edges: &[Vec<usize>],
        owner: &mut [Option<usize>],
        seen: &mut [bool],
    ) -> bool {
        for &slot in &edges[claim] {
            if seen[slot] {
                continue;
            }
            seen[slot] = true;
            if owner[slot].is_none_or(|other| augment(other, edges, owner, seen)) {
                owner[slot] = Some(claim);
                return true;
            }
        }
        false
    }
    let mut owner = vec![None; slots];
    for claim in 0..edges.len() {
        let mut seen = vec![false; slots];
        augment(claim, edges, &mut owner, &mut seen);
    }
    let mut matched = vec![None; edges.len()];
    for (slot, claim) in owner.into_iter().enumerate() {
        if let Some(claim) = claim {
            matched[claim] = Some(slot);
        }
    }
    matched
}

/// The text of row `row` of `rows`: padding left out, which only a line's
/// last row has, so a row that wraps onto the next keeps the spaces at its
/// end (`docs/design/terminal-model.md`, section 5.12); empty for a row
/// that shows nothing.
fn row_text(rows: &[Seen], row: usize) -> &str {
    let seen = &rows[row];
    let trimmed = seen.text.trim_end();
    if seen.wraps && row + 1 < rows.len() && !trimmed.is_empty() {
        &seen.text
    } else {
        trimmed
    }
}

/// The rows of `rows` with each text that is on exactly one of them.
fn unique_rows(rows: &[Seen]) -> HashMap<&str, usize> {
    let mut found: HashMap<&str, Option<usize>> = HashMap::new();
    for row in 0..rows.len() {
        let text = row_text(rows, row);
        if text.is_empty() {
            continue;
        }
        found
            .entry(text)
            .and_modify(|at| *at = None)
            .or_insert(Some(row));
    }
    found
        .into_iter()
        .filter_map(|(text, row)| row.map(|row| (text, row)))
        .collect()
}

/// Whether row `row` of `read` is on its screen rather than in the
/// history above it.
fn on_screen(read: &Observation, row: usize) -> bool {
    read.alternate || read.start + row >= read.screen_top
}

/// Checks a reader's mapping of `previous`'s rows onto `read`'s, both on
/// the same screen.
fn check_moved(
    violations: &mut Vec<Violation>,
    index: usize,
    previous: &Observation,
    read: &Observation,
    moved: &[Option<usize>],
) {
    let now = unique_rows(&read.rows);
    let mut before: Vec<(&str, usize)> = unique_rows(&previous.rows).into_iter().collect();
    before.sort_by_key(|&(_, row)| row);
    for (text, row) in before {
        let Some(&went) = now.get(text) else {
            continue;
        };
        if !on_screen(previous, row) || !on_screen(read, went) {
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
/// against where it was after `previous`, a read on the same screen.
fn check_review(
    violations: &mut Vec<Violation>,
    index: usize,
    previous: Option<&Observation>,
    read: &Observation,
) {
    let said = read.heard.review;
    // Rows above the read are in the history, down to the document's
    // first row; none is below the read's last.
    if let Some(row) = said
        && (usize::try_from(row).is_ok_and(|row| row >= read.rows.len())
            || row.unsigned_abs() > read.start && row < 0)
    {
        violations.push(Violation::ReviewOutside { read: index, row });
        return;
    }
    let Some(previous) = previous else {
        return;
    };
    let Some(row) = previous
        .heard
        .review
        .and_then(|row| usize::try_from(row).ok())
        .filter(|&row| row < previous.rows.len())
    else {
        return;
    };
    let text = row_text(&previous.rows, row);
    let (before, now) = (unique_rows(&previous.rows), unique_rows(&read.rows));
    if before.contains_key(text)
        && on_screen(previous, row)
        && let Some(&went) = now.get(text)
        && on_screen(read, went)
        && said != Some(isize::try_from(went).unwrap_or(isize::MAX))
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

/// The status rows the random streams draw as a footer or a header: a
/// spinner whose symbol alone changes, and a timer that ticks.
pub const STATUSES: [&str; 3] = ["\u{280b} 1s", "\u{2819} 1s", "\u{2819} 2s"];

/// Pieces of text split inside a character's encoding, as a program
/// streaming bytes writes them: "café" split inside its "é", and a check
/// mark split after its second byte.
pub const PIECES: [&[u8]; 4] = [b"caf\xc3", b"\xa9", b"ok \xe2\x9c", b"\x93"];

/// One of the operations programs do to a terminal
/// (`docs/design/terminal-model.md`, section 3.2), as the oracle's streams
/// write them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Text written at the cursor.
    Write(&'static str),
    /// Bytes written at the cursor, which may end or start inside a
    /// character's encoding.
    Bytes(&'static [u8]),
    /// The row erased and the cursor returned to its start, then text
    /// written, as a program rewrites its last line for each token.
    Rewrite(&'static str),
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
    /// A header drawn on the screen's first row with the cursor saved and
    /// restored around it.
    Header(&'static str),
    /// Rows erased upward from the cursor's, the cursor left at the start
    /// of the highest, as Ink erases its region before drawing it again.
    EraseLines(u8),
    /// The end of the row from a column erased and a word written on the
    /// next row, as a program that wraps its own text moves a partly
    /// written word down.
    WordDown {
        /// The column the moved word started at, counted from 1.
        column: u8,
        /// The word, written again on the next row.
        word: &'static str,
    },
    /// A row inserted above a footer on the screen's last row: the
    /// screen's first row deleted, and a row inserted above the footer,
    /// the cursor left at its start.
    InsertAboveFooter,
    /// The screen erased and text written from its top, as a program
    /// repaints its screen each frame.
    Repaint(&'static str),
    /// A row above the cursor's rewritten in bold with the cursor saved and
    /// restored around it, as a Markdown renderer restyles a line once its
    /// markup closes.
    Restyle {
        /// How many rows above the cursor's.
        rows: u8,
        /// The row's new text.
        text: &'static str,
    },
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
    pub fn bytes(self) -> Vec<u8> {
        match self {
            Self::Bytes(bytes) => bytes.to_vec(),
            other => other.text().into_bytes(),
        }
    }

    /// What a program writes for an operation that writes text.
    fn text(self) -> String {
        match self {
            Self::Write(text) => text.to_owned(),
            Self::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            Self::Rewrite(text) => format!("\r\x1b[2K{text}"),
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
            Self::Header(text) => format!("\x1b7\x1b[1;1H\x1b[2K{text}\x1b8"),
            Self::EraseLines(rows) => {
                "\x1b[2K\x1b[1A".repeat(usize::from(rows.saturating_sub(1))) + "\x1b[2K\x1b[G"
            }
            Self::WordDown { column, word } => format!("\x1b[{column}G\x1b[K\r\n{word}"),
            Self::InsertAboveFooter => "\x1b[H\x1b[M\x1b[99;1H\x1b[A\x1b[L".to_owned(),
            Self::Repaint(text) => format!("\x1b[H\x1b[2J{text}"),
            Self::Restyle { rows, text } => {
                format!("\x1b7\x1b[{rows}A\r\x1b[2K\x1b[1m{text}\x1b[m\x1b8")
            }
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
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut chunk: Vec<u8> = Vec::new();
    for (op, read) in steps {
        chunk.extend_from_slice(&op.bytes());
        if *read {
            chunks.push(std::mem::take(&mut chunk).into());
        }
    }
    if !chunk.is_empty() {
        chunks.push(chunk.into());
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

/// One operation of the random streams, with their weights: words written
/// and lines ended most often, and every other operation of [`Op`] with
/// arguments on [`Size::SMALL`]; footers and headers draw words or
/// [`STATUSES`].
pub fn random_op() -> impl Strategy<Value = Op> {
    let word = || prop::sample::select(&WORDS[..]);
    let status = || prop::sample::select([&WORDS[..], &STATUSES[..]].concat());
    prop_oneof![
        4 => word().prop_map(Op::Write),
        1 => prop::sample::select(&PIECES[..]).prop_map(Op::Bytes),
        1 => word().prop_map(Op::Rewrite),
        4 => Just(Op::Newline),
        1 => Just(Op::LineFeed),
        1 => Just(Op::CarriageReturn),
        1 => (1..=5_u8, 1..=12_u8).prop_map(|(row, column)| Op::MoveTo { row, column }),
        1 => (-3..=3_i8).prop_map(Op::MoveRows),
        1 => (-6..=6_i8).prop_map(Op::MoveColumns),
        1 => (0..=2_u8).prop_map(Op::EraseLine),
        1 => Just(Op::EraseBelow),
        1 => status().prop_map(Op::Footer),
        1 => word().prop_map(Op::Redraw),
        1 => status().prop_map(Op::Header),
        1 => (1..=3_u8).prop_map(Op::EraseLines),
        1 => (1..=12_u8, word()).prop_map(|(column, word)| Op::WordDown { column, word }),
        1 => Just(Op::InsertAboveFooter),
        1 => word().prop_map(Op::Repaint),
        1 => (1..=3_u8, word()).prop_map(|(rows, text)| Op::Restyle { rows, text }),
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

/// The steps of a random stream: 1 to 39 operations of [`random_op`], each
/// followed by a read or not, for [`stream`].
pub fn random_steps() -> impl Strategy<Value = Vec<(Op, bool)>> {
    prop::collection::vec((random_op(), any::<bool>()), 1..40)
}

/// The random streams' configuration, every field written out:
/// `Config::default()` reads `PROPTEST_` environment variables, which would
/// let the machine change what is checked. A fixed seed makes every run
/// check the same streams, and no failure files are kept: a failure prints
/// its shrunk steps.
#[must_use]
pub fn random_config() -> Config {
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
