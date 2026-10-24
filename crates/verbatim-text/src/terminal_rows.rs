//! The memory of a terminal's rows and the alignment of each read against
//! it (`docs/design/terminal-model.md`, "The model", "The read" and "The
//! alignment"): what each row of a read is (new, kept and from where, grown,
//! rewritten and from which row, a lingering row returned, or a row shown
//! before that carries no history), where each row of the last read went,
//! how far the history rotated, and where the anchors (the review position
//! and the command marks) are now. This is classification, not speech: the
//! reading rules are built on [`Outcome`].
//!
//! A read gives no coordinates, only text, so a row's position is its
//! index in the read that found it. Each read starts one screen above the
//! top of the last read's screen, at the provider range the last read kept.
//! While the read's rows above the last read's screen read as the last
//! read's history block did, with at least one row with text among them,
//! the last read's rows are where they were ("Positions"); otherwise they
//! are found by their text ("The alignment"). A read that keeps nothing the
//! last read had at or above the caret, or keeps it equally well at more
//! than one place, with positions not trusted and its start not refused, is
//! lost: the whole document is read once ("A lost read"). A read that keeps
//! and pairs nothing with its start refused, or either read's screen at the
//! document's start, is a new screen ("Screens").
//!
//! The memory holds only the last read: once a read is handled, its rows
//! from a screen above the next read's start on. Rows keep their places,
//! counted from the read's first row, so what refers to a row of the last
//! read is unchanged by the trim.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

mod align;

use align::Likeness;

/// How long a gone row lingers ("Lingering"), in milliseconds.
pub const LINGERING_MS: u64 = 2000;

/// What one read of a terminal returned, as the read program gives it
/// (`verbatim_uia_rops::Snapshot`), with its text as UTF-8 and the time
/// of the read. The start for the next read is the outpost's to keep.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent answers of one read, as the read program gives them"
)]
pub struct Read {
    /// The text from the read's start to the document's end, padding and
    /// line breaks included.
    pub text: String,
    /// The screen's text, for its width and height.
    pub screen: String,
    /// Whether the read started where the last read said to start.
    pub from_start: bool,
    /// Whether that start was refused: the text shrank, or the screen
    /// switched.
    pub shrank: bool,
    /// Whether the read started at the document's start.
    pub at_document_start: bool,
    /// Whether the screen starts at the document's start: no history
    /// above it.
    pub alternate: bool,
    /// The document's first row.
    pub first_row: String,
    /// How many rows the caret's row and those below it hold, to the
    /// document's end; 0 when there is no caret.
    pub caret_rows_to_end: u32,
    /// The caret's row.
    pub caret_line: String,
    /// The caret's column in its row.
    pub caret_column: usize,
    /// When the read was taken, in milliseconds on the reader's clock.
    pub at_ms: u64,
    /// How many rows the read holds above the start it was asked for
    /// ([`TerminalMemory::above`]): fewer than asked when the document's
    /// start came first.
    pub above: usize,
}

/// A row of the memory: one row of the last read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Its text, padding left out ("Rows"): only a line's last row has
    /// padding, so the earlier rows of a wrapped line keep the spaces at
    /// their end. Shared, so the line it makes up and the reading rules'
    /// state of that line hold no copy of it.
    pub text: Arc<str>,
    /// A 64-bit hash of `text`.
    pub hash: u64,
    /// Whether its line continues onto the next row.
    pub wraps: bool,
    /// The hash of the whole line it belongs to, its rows joined.
    pub line_hash: u64,
    /// Whether a program has rewritten it, or it was taken back while
    /// lingering ("Lingering").
    pub redrawn: bool,
}

impl Row {
    /// Whether the row shows nothing: no text, or only the spaces of a
    /// wrapped line's row.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        crate::trim_padding(&self.text).is_empty()
    }
}

/// A remembered place ("The memory"): the review position or a command
/// mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    /// Its row, counted from the last read's first row; negative above
    /// it, in the history.
    pub row: isize,
    /// Its column, in characters of the row's text.
    pub column: usize,
    /// The hash of its line.
    pub line_hash: u64,
}

/// What a row of a read is, against the memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewRow {
    /// Not in the memory: new output, or a row whose text no old row had.
    New,
    /// An old row, its text unchanged.
    Kept {
        /// The old row's index.
        from: usize,
        /// Whether it moved other than with the history: drawn again
        /// elsewhere by its program.
        moved: bool,
    },
    /// An old row whose text grew where it was: its new text starts with
    /// its old ("The alignment"), and it counts as kept.
    Grown {
        /// The old row's index.
        from: usize,
    },
    /// An old row rewritten with other text, paired by likeness.
    Rewritten {
        /// The old row's index.
        from: usize,
        /// Whether it moved other than with the history, as a status row
        /// drawn a row lower with its spinner changed.
        moved: bool,
        /// Whether it moved alone, with no neighbour kept in the same move,
        /// so its change history starts afresh ("The alignment").
        alone: bool,
    },
    /// A lingering row taken back: a row a read found gone, back with its
    /// text ("Lingering").
    Returned {
        /// Whether it came back elsewhere than where it was.
        moved: bool,
    },
    /// A row whose text was shown before but that carries no row of the
    /// last read: a row of the history above the last read, a row the
    /// remembered history held, or, on a new screen, a row above the
    /// visible screen, which is history ("Screens").
    Shown,
}

/// What became of a row of the last read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OldRow {
    /// Kept, or grown, now the read's row at this index.
    Kept(usize),
    /// Rewritten, now the read's row at this index.
    Rewritten(usize),
    /// Gone: erased or written over.
    Gone {
        /// Whether it lingers ("Lingering").
        lingering: bool,
    },
    /// Above the read: the rotation carried it past the read's start.
    Above,
    /// Out of the buffer: the rotation carried it past the read's start,
    /// which is the document's start.
    Left,
}

/// A change of screens ("Screens").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Switch {
    /// No change of screens.
    #[default]
    None,
    /// A new screen: nothing of the last read survives, as a full-screen
    /// program opening or closing, or Windows Terminal's clear leaving a
    /// prompt. The visible screen's rows are new; the rows above it are
    /// history, unspoken.
    New,
}

/// A line of a read: rows joined while each wraps onto the next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// Its rows.
    pub rows: Range<usize>,
    /// Its text, padding left out: a one-row line's is its row's, shared.
    pub text: Arc<str>,
    /// The hash of `text`.
    pub hash: u64,
    /// When the line only gained at its end since the last read, rows
    /// wrapped onto it included, and did not move ("Growing lines"): the
    /// length in bytes of what it was, so `text[from..]` is what it gained.
    pub grew_from: Option<usize>,
}

/// The rows of the last read ([`TerminalMemory::rows`]), counted from its
/// first row: those from [`Rows::first_kept`] on are kept, and those above
/// it are held only as hashes in the remembered history.
#[derive(Clone, Copy, Debug)]
pub struct Rows<'a> {
    above: usize,
    kept: &'a [Row],
}

impl<'a> Rows<'a> {
    /// How many rows the last read held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.above + self.kept.len()
    }

    /// Whether the last read held no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The first row kept.
    #[must_use]
    pub fn first_kept(&self) -> usize {
        self.above
    }

    /// The rows kept, from [`Rows::first_kept`] on.
    #[must_use]
    pub fn kept(&self) -> &'a [Row] {
        self.kept
    }

    /// Row `row` of the last read, when it is kept.
    #[must_use]
    pub fn get(&self, row: usize) -> Option<&'a Row> {
        self.kept.get(row.checked_sub(self.above)?)
    }
}

/// What the memory made of one read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent findings of one read"
)]
pub struct Outcome {
    /// The read keeps no row with text that the last read had at or above
    /// the caret, or its start was refused, so the caller reads the whole
    /// document and gives that read instead ("The read"). Nothing else is
    /// set, and the memory is unchanged.
    pub whole_document: bool,
    /// The memory's first read: every row is new, and nothing was heard
    /// before it.
    pub first: bool,
    /// The read's screen shows no text where the last read's had text ("A
    /// read with no text"): nothing else is set, and the memory is
    /// unchanged.
    pub blank: bool,
    /// What each row of the read is.
    pub rows: Vec<NewRow>,
    /// What became of each row of the last read.
    pub old: Vec<OldRow>,
    /// The read's lines.
    pub lines: Vec<Line>,
    /// How many rows the last read's rows moved up, as the alignment found
    /// them.
    pub rotation: usize,
    /// A lost read, read whole ("A lost read"): how what went by is counted.
    pub lost: Option<Lost>,
    /// Whether the last read's rows were found by their text, positions not
    /// trusted ("Positions").
    pub by_text: bool,
    /// A change of screens.
    pub switch: Switch,
    /// The caret's row, when it is in the read.
    pub caret: Option<usize>,
    /// Whether the read's first row continues a line that starts above the
    /// read, so the first of [`Outcome::lines`] is that line's rest.
    pub continued: bool,
    /// The screen's width in cells.
    pub width: usize,
    /// The screen's height in rows.
    pub height: usize,
}

/// How the lines of a lost read that went by are counted ("A lost read").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lost {
    /// The last read's rows were found at one place, by their text: the
    /// rows below them are new, and their count is "about".
    About,
    /// The last read's rows were nowhere: every row above the visible
    /// screen is new, and more went by than the buffer holds.
    MoreThan,
    /// The last read's rows fit at more than one place: only the visible
    /// screen is aligned, and what went by has no count.
    Uncounted,
}

/// A row that lingers: gone, kept for a while with its history.
#[derive(Clone, Debug)]
struct Lingering {
    row: Row,
    /// When it was found gone.
    since_ms: u64,
    /// Where it would be now had it stayed, counted as the last read's
    /// rows are.
    at: isize,
}

/// One screen's memory. Its rows are the last read's from `skipped` on;
/// the next read's start, the caret, lingering rows and the anchors are
/// counted as the last read's rows are.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of the last read"
)]
struct Screen {
    rows: Vec<Row>,
    /// How many of the last read's rows lie above `rows`, trimmed once the
    /// read was handled.
    skipped: usize,
    /// The screen's width and height at the last read.
    width: usize,
    height: usize,
    /// The last read's row at which the next read starts, the top of its
    /// screen.
    next: usize,
    /// Whether the last read started at the document's start.
    at_document_start: bool,
    caret: Option<usize>,
    /// Whether the last read's screen showed text.
    shows_text: bool,
    /// Whether the last read's screen started at the document's start, so
    /// that its first row was a row of the screen, not of the history.
    no_history: bool,
    lingering: Vec<Lingering>,
    review: Option<Anchor>,
    marks: Vec<Anchor>,
    /// Command marks whose line no read since a new screen has shown, each
    /// with how many rows the read that lost it held: found again by their
    /// line's hash when a later read shows it ("The review cursor").
    lost_marks: Vec<(Anchor, usize)>,
    /// The places the caller keeps, each by its number
    /// ([`TerminalMemory::add_place`]).
    places: Vec<(u64, Anchor)>,
    /// Whether the first row continues a line that starts above it.
    continued: bool,
}

impl Screen {
    /// The index in `rows` of the last read's row `row`, when it is kept.
    fn index(&self, row: usize) -> Option<usize> {
        row.checked_sub(self.skipped)
            .filter(|&index| index < self.rows.len())
    }

    /// The last read's row `row`, when it is kept.
    fn row(&self, row: usize) -> Option<&Row> {
        self.index(row).map(|index| &self.rows[index])
    }

    /// How many rows the last read held.
    fn total(&self) -> usize {
        self.skipped + self.rows.len()
    }

    /// The index in `rows` of the first row kept once the read is trimmed:
    /// a screen above the next read's start.
    fn trim_point(&self) -> usize {
        self.next
            .saturating_sub(self.skipped)
            .saturating_sub(self.height)
            .min(self.rows.len())
    }

    /// The index in `rows` of the screen's first row as the next read
    /// counts it: the rows above are history.
    fn screen_start(&self) -> usize {
        self.rows.len().saturating_sub(self.height)
    }

    /// Places each lost command mark whose line a row of the read starts,
    /// nearest its old place counted from the read's end ("The review
    /// cursor").
    fn find_lost_marks(&mut self) {
        let rows = &self.rows;
        let (found, lost): (Vec<_>, Vec<_>) = self
            .lost_marks
            .drain(..)
            .map(|(mark, total)| (refind(mark, total, mark.column, rows), (mark, total)))
            .partition(|(found, _)| found.is_some());
        self.marks
            .extend(found.into_iter().filter_map(|(mark, _)| mark));
        self.lost_marks = lost.into_iter().map(|(_, mark)| mark).collect();
    }

    /// Keeps only a screen above the next read's start and the rows below
    /// it. Returns how many rows it dropped.
    fn trim(&mut self) -> usize {
        let dropped = self.trim_point();
        if dropped == 0 {
            return 0;
        }
        self.continued = self.rows[dropped - 1].wraps;
        self.rows = self.rows.split_off(dropped);
        self.skipped += dropped;
        dropped
    }

    /// A memory of `rows`, a read whose query gave the screen's height as
    /// `asked`, 0 for a first read, with no history of its own.
    fn fresh(rows: Vec<Row>, read: &Read, (width, height): (usize, usize), asked: usize) -> Self {
        let caret = caret_row(rows.len(), read.caret_rows_to_end);
        Self {
            next: next_of(rows.len(), read.above, asked),
            rows,
            width,
            height,
            at_document_start: read.at_document_start,
            caret,
            shows_text: !crate::trim_padding(&read.screen).trim().is_empty(),
            no_history: read.alternate,
            ..Self::default()
        }
    }
}

/// The last read's row at which the next read starts, for a read of `rows`
/// rows, `above` of them above its start, asked with the screen's height
/// `asked`: the read program places it the screen's height up from the
/// document's end, or, with no height known, at the screen's top, where the
/// read started.
fn next_of(rows: usize, above: usize, asked: usize) -> usize {
    if asked == 0 {
        above
    } else {
        rows.saturating_sub(asked)
    }
}

/// The memory of one terminal ("The memory"), kept while the terminal
/// exists, across focus changes.
#[derive(Clone, Debug, Default)]
pub struct TerminalMemory {
    screen: Option<Screen>,
    /// Whether the whole document was asked for, until it is read.
    asked_whole: bool,
    /// The number the next place gets ([`TerminalMemory::add_place`]).
    next_place: u64,
}

impl TerminalMemory {
    /// An empty memory, before the terminal's first read.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many rows the next read takes above its start ("The read"): one
    /// screen, as the last read found it; none before the first.
    #[must_use]
    pub fn above(&self) -> usize {
        self.screen.as_ref().map_or(0, |screen| screen.height)
    }

    /// The rows of the last read, counted from its first row, of which
    /// the memory keeps those from a screen above the next read's start on
    /// once the read is trimmed ([`TerminalMemory::trim`]).
    #[must_use]
    pub fn rows(&self) -> Rows<'_> {
        self.screen.as_ref().map_or(
            Rows {
                above: 0,
                kept: &[],
            },
            |screen| Rows {
                above: screen.skipped,
                kept: &screen.rows,
            },
        )
    }

    /// The row of the last read from which the memory keeps rows once the
    /// read is trimmed: a screen above the next read's start.
    #[must_use]
    pub fn first_kept(&self) -> usize {
        self.screen
            .as_ref()
            .map_or(0, |screen| screen.skipped + screen.trim_point())
    }

    /// Keeps of the last read only what the next read needs: a screen above
    /// the next read's start and the rows below it. The reading rules call
    /// it once
    /// they have handled a read, and every read first trims what the last
    /// left; the rows keep their places, so nothing counted from the last
    /// read's first row moves. Returns how many rows it dropped.
    pub fn trim(&mut self) -> usize {
        self.screen.as_mut().map_or(0, Screen::trim)
    }

    /// The review position.
    #[must_use]
    pub fn review(&self) -> Option<Anchor> {
        self.screen.as_ref().and_then(|screen| screen.review)
    }

    /// Puts the review position on row `row` of the last read, at
    /// `column`. Does nothing before the first read or on a row the
    /// memory does not keep.
    pub fn set_review(&mut self, row: usize, column: usize) {
        if let Some(screen) = &mut self.screen
            && let Some(found) = screen.row(row)
        {
            screen.review = Some(Anchor {
                row: isize_of(row),
                column,
                line_hash: found.line_hash,
            });
        }
    }

    /// Puts the review position at `anchor`, which may be on a row above
    /// the rows the memory keeps, in the history, whose line's hash the
    /// caller read ("The review anchor and following"). Does nothing before
    /// the first read.
    pub fn set_review_anchor(&mut self, anchor: Anchor) {
        if let Some(screen) = &mut self.screen {
            screen.review = Some(anchor);
        }
    }

    /// An anchor on row `row` of the last read at `column`, clamped to the
    /// row's text, when the memory keeps the row.
    #[must_use]
    pub fn anchor(&self, row: usize, column: usize) -> Option<Anchor> {
        let screen = self.screen.as_ref()?;
        let found = screen.row(row)?;
        Some(Anchor {
            row: isize_of(row),
            column: column.min(found.text.chars().count()),
            line_hash: found.line_hash,
        })
    }

    /// Keeps a place at `anchor`, a position the caller holds in the
    /// terminal's text, such as the start of a row of text it sent on, and
    /// returns its number. A place moves with its row as the review
    /// position does, goes to the caret's row on a new screen, and is dropped when its
    /// row cannot be followed, as when the history overflowed and its line
    /// was not found again. Before the first read it is only numbered.
    pub fn add_place(&mut self, anchor: Anchor) -> u64 {
        self.next_place += 1;
        let number = self.next_place;
        if let Some(screen) = &mut self.screen {
            screen.places.push((number, anchor));
        }
        number
    }

    /// Where place `number` is now, `None` once it was dropped
    /// ([`TerminalMemory::add_place`]).
    #[must_use]
    pub fn place(&self, number: u64) -> Option<Anchor> {
        let screen = self.screen.as_ref()?;
        screen
            .places
            .iter()
            .find(|(kept, _)| *kept == number)
            .map(|(_, anchor)| *anchor)
    }

    /// Forgets place `number`.
    pub fn drop_place(&mut self, number: u64) {
        if let Some(screen) = &mut self.screen {
            screen.places.retain(|(kept, _)| *kept != number);
        }
    }

    /// The command marks, in the order they were set.
    #[must_use]
    pub fn marks(&self) -> &[Anchor] {
        self.screen.as_ref().map_or(&[], |screen| &screen.marks)
    }

    /// Sets a command mark on the line holding row `row` of the last read,
    /// at the line's first row, or the first row kept of it. Does nothing
    /// before the first read or on a row the memory does not keep.
    pub fn add_mark(&mut self, row: usize) {
        if let Some(screen) = &mut self.screen
            && let Some(index) = screen.index(row)
        {
            let first = line_start(&screen.rows, index);
            let line_hash = screen.rows[first].line_hash;
            screen.marks.push(Anchor {
                row: isize_of(screen.skipped + first),
                column: 0,
                line_hash,
            });
        }
    }

    /// Aligns `read` with the memory, and takes it as the memory's last
    /// read, unless [`Outcome::whole_document`] asks for the whole document
    /// first or [`Outcome::blank`] says its screen shows no text. What the
    /// last read left untrimmed is trimmed first ([`TerminalMemory::trim`]).
    pub fn read(&mut self, read: &Read) -> Outcome {
        self.trim();
        let last_height = self.screen.as_ref().map(|screen| screen.height);
        let (width, height) = measure(&read.screen, &read.caret_line, last_height);
        let rows = split_rows(&read.text, width);
        let caret = caret_row(rows.len(), read.caret_rows_to_end);
        let shape = Outcome {
            caret,
            width,
            height,
            ..Outcome::default()
        };
        let asked_whole = std::mem::take(&mut self.asked_whole);
        let Some(current) = self.screen.take() else {
            let outcome = Outcome {
                first: true,
                rows: vec![NewRow::New; rows.len()],
                lines: lines(&rows),
                ..shape
            };
            self.screen = Some(Screen::fresh(rows, read, (width, height), 0));
            return outcome;
        };
        // A changed width or height rewraps every row: the memory starts
        // again ("Starting again").
        if (width, height) != (current.width, current.height) {
            return self.start_again(&current, rows, read, &shape);
        }
        // A read whose screen shows no text where the last read's had text
        // is a read between a program's clear and its reprint: it changes
        // nothing ("A read with no text"), where the next read starts
        // included, which the caller keeps as the last read placed it.
        if current.shows_text && crate::trim_padding(&read.screen).trim().is_empty() {
            self.screen = Some(current);
            return Outcome {
                blank: true,
                ..shape
            };
        }
        let had_text = current.rows.iter().any(|row| !row.is_blank());
        // The whole document a lost read asked for ("A lost read").
        if asked_whole && read.at_document_start && !read.from_start {
            let (aligned, lost) = align_whole(&current, (&rows, height), read, caret);
            let shape = Outcome {
                lost: Some(lost),
                by_text: true,
                ..shape
            };
            if had_text && !aligned.kept_text && !aligned.paired && lost == Lost::About {
                return self.screens(&current, rows, read, &shape);
            }
            let next = remember(&current, rows, &aligned, read, (width, height), caret);
            return self.finish(&current, next, &aligned, &shape, Switch::None);
        }
        let refused = read.shrank || !read.from_start;
        let trusted = !refused && positions(&current, &rows, read);
        let aligned = align_read(&current, &rows, read, caret, trusted);
        let shape = Outcome {
            by_text: !trusted,
            ..shape
        };
        // Whether this is a different screen: nothing of the last read
        // survives, rows below the caret included ("Screens").
        let nothing = had_text && !aligned.kept_text && !aligned.paired;
        if nothing && (refused || current.no_history || read.alternate) {
            return self.screens(&current, rows, read, &shape);
        }
        // A lost read: positions not trusted, the start not refused, and
        // nothing the last read had at or above the caret kept, or kept
        // equally well at more than one place ("A lost read").
        if !trusted && !refused && had_text && (aligned.lost() || aligned.ambiguous) {
            self.asked_whole = true;
            self.screen = Some(current);
            return Outcome {
                whole_document: true,
                ..shape
            };
        }
        let next = remember(&current, rows, &aligned, read, (width, height), caret);
        self.finish(&current, next, &aligned, &shape, Switch::None)
    }

    /// Takes `read`, for which [`TerminalMemory::read`] asked for the whole
    /// document, without that read, as when "report new output" is off and
    /// nothing is read that is not needed ("The read"): the memory starts
    /// again from it ("Starting again").
    pub fn restart(&mut self, read: &Read) -> Outcome {
        self.asked_whole = false;
        let last_height = self.screen.as_ref().map(|screen| screen.height);
        let (width, height) = measure(&read.screen, &read.caret_line, last_height);
        let rows = split_rows(&read.text, width);
        let caret = caret_row(rows.len(), read.caret_rows_to_end);
        let shape = Outcome {
            caret,
            width,
            height,
            ..Outcome::default()
        };
        if let Some(current) = self.screen.take() {
            return self.start_again(&current, rows, read, &shape);
        }
        let outcome = Outcome {
            first: true,
            rows: vec![NewRow::New; rows.len()],
            lines: lines(&rows),
            ..shape
        };
        self.screen = Some(Screen::fresh(rows, read, (width, height), 0));
        outcome
    }

    /// The memory after a read aligned with `current`, as `next`, and the
    /// outcome.
    fn finish(
        &mut self,
        current: &Screen,
        mut next: Screen,
        aligned: &Aligned,
        shape: &Outcome,
        switch: Switch,
    ) -> Outcome {
        // The read's first row continues a line from above it when the old
        // row it was did.
        let continued = match aligned.new.first() {
            Some(
                NewRow::Kept { from, .. } | NewRow::Grown { from } | NewRow::Rewritten { from, .. },
            ) => match *from {
                0 => current.continued,
                index => current.rows[index - 1].wraps,
            },
            _ => false,
        };
        next.continued = continued;
        let skip = current.skipped;
        let kinds: Vec<NewRow> = aligned
            .new
            .iter()
            .map(|kind| match *kind {
                NewRow::Kept { from, moved } => NewRow::Kept {
                    from: from + skip,
                    moved,
                },
                NewRow::Grown { from } => NewRow::Grown { from: from + skip },
                NewRow::Rewritten { from, moved, alone } => NewRow::Rewritten {
                    from: from + skip,
                    moved,
                    alone,
                },
                other => other,
            })
            .collect();
        let lines = lines(&next.rows)
            .into_iter()
            .map(|mut line| {
                line.grew_from = grew_from(&line, &aligned.new, current, continued);
                line
            })
            .collect();
        let outcome = Outcome {
            rows: kinds,
            old: aligned.fates(current),
            lines,
            rotation: aligned.rotation,
            switch,
            continued,
            ..*shape
        };
        self.screen = Some(next);
        outcome
    }

    /// A read that keeps nothing of the last read and pairs nothing, a new
    /// screen ("Screens"): its visible rows are new and its rows above them
    /// silent. The review position goes to the caret, and the command marks
    /// wait for their lines.
    fn screens(
        &mut self,
        current: &Screen,
        rows: Vec<Row>,
        read: &Read,
        shape: &Outcome,
    ) -> Outcome {
        let (width, height, caret) = (shape.width, shape.height, shape.caret);
        let old = vec![OldRow::Gone { lingering: false }; current.total()];
        let mut next = Screen::fresh(rows, read, (width, height), current.height);
        next.review = caret.map(|row| anchor_at(&next.rows, row, read.caret_column));
        let total = current.total();
        next.lost_marks.clone_from(&current.lost_marks);
        next.lost_marks
            .extend(current.marks.iter().map(|&mark| (mark, total)));
        next.find_lost_marks();
        let screen = next.screen_start();
        let kinds = (0..next.rows.len())
            .map(|row| {
                if row < screen {
                    NewRow::Shown
                } else {
                    NewRow::New
                }
            })
            .collect();
        let outcome = Outcome {
            rows: kinds,
            old,
            lines: lines(&next.rows),
            switch: Switch::New,
            lost: None,
            ..*shape
        };
        self.screen = Some(next);
        outcome
    }

    /// The memory started again from `read` ("Starting again"): every row's
    /// history begins afresh and nothing is aligned; each anchor is found
    /// by its line's text nearest its old place, or else the review
    /// position goes to the caret and a mark or a place is dropped.
    fn start_again(
        &mut self,
        current: &Screen,
        rows: Vec<Row>,
        read: &Read,
        shape: &Outcome,
    ) -> Outcome {
        let mut next = Screen::fresh(rows, read, (shape.width, shape.height), current.height);
        let total = current.total();
        let found =
            |anchor: &Anchor| refind(*anchor, total, line_offset(current, anchor), &next.rows);
        next.review = current.review.as_ref().and_then(found).or_else(|| {
            shape
                .caret
                .map(|row| anchor_at(&next.rows, row, read.caret_column))
        });
        next.marks = current.marks.iter().filter_map(found).collect();
        next.places = current
            .places
            .iter()
            .filter_map(|(number, anchor)| Some((*number, found(anchor)?)))
            .collect();
        let outcome = Outcome {
            rows: vec![NewRow::New; next.rows.len()],
            old: vec![OldRow::Gone { lingering: false }; total],
            lines: lines(&next.rows),
            ..*shape
        };
        self.screen = Some(next);
        outcome
    }
}

/// An anchor found again by its line's hash, on the line with it whose
/// first row is nearest its old place, counted from the end of a read of
/// `total` rows, in `rows`, at `offset`, its character position within its
/// line, which a resize keeps ("Starting again").
fn refind(anchor: Anchor, total: usize, offset: usize, rows: &[Row]) -> Option<Anchor> {
    let from_end = isize_of(total) - anchor.row;
    let (mut row, _) = rows
        .iter()
        .enumerate()
        .filter(|(at, row)| row.line_hash == anchor.line_hash && line_start(rows, *at) == *at)
        .min_by_key(|(at, _)| (isize_of(rows.len()) - isize_of(*at) - from_end).abs())?;
    let mut column = offset;
    while rows[row].wraps && row + 1 < rows.len() {
        let length = rows[row].text.chars().count();
        if column < length {
            break;
        }
        column -= length;
        row += 1;
    }
    Some(anchor_at(rows, row, column))
}

/// The character position of `anchor` within its line in `memory`: the
/// characters of its line's rows above its row and its column; its column
/// alone when its row is not kept.
fn line_offset(memory: &Screen, anchor: &Anchor) -> usize {
    let Some(index) = usize::try_from(anchor.row)
        .ok()
        .and_then(|row| memory.index(row))
    else {
        return anchor.column;
    };
    let first = line_start(&memory.rows, index);
    memory.rows[first..index]
        .iter()
        .map(|row| row.text.chars().count())
        .sum::<usize>()
        + anchor.column
}

/// A kept pair: an old row's index and the new row's it became.
type Pair = (usize, usize);

/// What aligning a read with one screen's memory gave. Old rows are counted
/// as the memory's rows are, from its first kept row.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent findings of one alignment"
)]
struct Aligned {
    new: Vec<NewRow>,
    old: Vec<OldRow>,
    /// Where a memory row that did not move is in the read: its index there
    /// less its index in the memory.
    fixed: isize,
    /// Where a memory row's place is in the read, as the read's start was
    /// placed: what an anchor above the last read keeps ("The review
    /// anchor").
    place: isize,
    rotation: usize,
    /// Whether a row with text the last read had at or above the caret was
    /// kept.
    kept_caret: bool,
    /// Whether a row with text of the memory was kept.
    kept_text: bool,
    /// Whether a row of the memory was paired as rewritten.
    paired: bool,
    /// Whether the last read's rows at or above the caret with text fit the
    /// read equally well at more than one place ("A lost read").
    ambiguous: bool,
    /// Whether the read started at the document's start, so a row the
    /// rotation carried above it left the buffer.
    left: bool,
}

impl Aligned {
    /// Whether the read keeps no row with text that the last read had at or
    /// above the caret ("The read"), which holds too when the last read had
    /// none.
    fn lost(&self) -> bool {
        !self.kept_caret
    }

    /// What became of each row of the last read, counted from its first:
    /// those `current` trimmed, held as hashes, moved by the rotation like
    /// any history row.
    fn fates(&self, current: &Screen) -> Vec<OldRow> {
        let skip = current.skipped;
        let mut fates: Vec<OldRow> = (0..skip)
            .map(|row| {
                let to = isize_of(row) - isize_of(skip) + self.place;
                match usize::try_from(to) {
                    Ok(to) if to < self.new.len() => OldRow::Kept(to),
                    Ok(_) => OldRow::Gone { lingering: false },
                    Err(_) if self.left => OldRow::Left,
                    Err(_) => OldRow::Above,
                }
            })
            .collect();
        fates.extend(self.old.iter().copied());
        fates
    }
}

/// How to finish an alignment of a read with a memory, once its kept rows
/// are found ([`classify`]).
struct How {
    /// Where a memory row that did not move is in the read.
    fixed: isize,
    /// Where a memory row's place is in the read.
    place: isize,
    rotation: usize,
    /// The memory's rows from which on rows are not history.
    history: usize,
    /// Whether the old rows' places in the read are free: found by their
    /// text, not by position ("Positions").
    free: bool,
    /// Whether the read is the whole document, whose history is `shown`.
    whole: bool,
    /// The read's rows that are history: above the visible screen, not
    /// new ("A lost read").
    shown: Vec<usize>,
}

/// Whether positions are trusted ("Positions"): the read's rows above the
/// last read's screen read exactly as the last read's history block did in
/// the same places, and at least one of them has text. A read with no rows
/// above the last read's screen, at the document's start, compares none.
fn positions(memory: &Screen, rows: &[Row], read: &Read) -> bool {
    let screen = isize_of(memory.next.saturating_sub(memory.skipped));
    let block = read.above.min(rows.len());
    if block == 0 {
        return true;
    }
    let mut text = false;
    for (to, row) in rows[..block].iter().enumerate() {
        let from = screen - isize_of(block) + isize_of(to);
        let Some(old) = usize::try_from(from)
            .ok()
            .and_then(|from| memory.rows.get(from))
        else {
            return false;
        };
        if old.hash != row.hash {
            return false;
        }
        text |= !row.is_blank();
    }
    text
}

/// Whether the last read's rows with text at or above its caret fit `rows`
/// equally well at more than one place: the shift that puts most of them
/// on rows with their text is not the only one ("A lost read").
fn ambiguous(memory: &Screen, rows: &[Row], within: Range<usize>) -> bool {
    let old = &memory.rows;
    let caret = memory
        .caret
        .and_then(|row| row.checked_sub(memory.skipped))
        .unwrap_or(old.len());
    let mut at: HashMap<u64, Vec<usize>> = HashMap::new();
    for to in within {
        if !rows[to].is_blank() {
            at.entry(rows[to].hash).or_default().push(to);
        }
    }
    let mut votes: HashMap<isize, usize> = HashMap::new();
    for (from, row) in old.iter().enumerate().take(caret + 1) {
        if row.is_blank() {
            continue;
        }
        for &to in at.get(&row.hash).into_iter().flatten() {
            *votes.entry(isize_of(to) - isize_of(from)).or_default() += 1;
        }
    }
    let best = votes.values().copied().max().unwrap_or(0);
    best > 0 && votes.values().filter(|&&count| count == best).count() > 1
}

/// Aligns `rows`, a read from the kept start, with `memory` ("The
/// alignment"): the longest common subsequence of rows with text, in the
/// fewest blocks, moving the fewest rows from their places. With positions
/// `trusted`, the last read's rows are where they were: its history block
/// is kept in place and every row below the end of the last read is new;
/// otherwise the rows are found by their text wherever they went.
fn align_read(
    memory: &Screen,
    rows: &[Row],
    read: &Read,
    caret: Option<usize>,
    trusted: bool,
) -> Aligned {
    let old = &memory.rows;
    let next = isize_of(memory.next.saturating_sub(memory.skipped));
    let base = isize_of(read.above) - next;
    let history = memory.screen_start();
    let old_text: Vec<usize> = (0..old.len())
        .filter(|&from| !old[from].is_blank())
        .collect();
    let old_hashes: HashSet<u64> = old_text.iter().map(|&from| old[from].hash).collect();
    // A row whose text is once among the old rows and once in the read is
    // kept where that text went, so it does not grow elsewhere and no other
    // row grows into it.
    let count = |hash: u64, rows: &[Row]| rows.iter().filter(|row| row.hash == hash).count();
    let went: HashSet<u64> = old_text
        .iter()
        .map(|&from| old[from].hash)
        .filter(|&hash| count(hash, old) == 1 && count(hash, rows) == 1)
        .collect();
    let grows = |from: usize, to: usize| {
        trusted
            && from >= history
            && from < old.len()
            && !old[from].is_blank()
            && !went.contains(&old[from].hash)
            && !went.contains(&rows[to].hash)
            && rows[to].text.len() > old[from].text.len()
            && rows[to].text.starts_with(&*old[from].text)
    };
    let window: Vec<usize> = (0..rows.len())
        .filter(|&to| {
            !rows[to].is_blank()
                && (old_hashes.contains(&rows[to].hash)
                    || usize::try_from(isize_of(to) - base).is_ok_and(|from| grows(from, to)))
        })
        .collect();
    let matches = |from: usize, to: usize| {
        let shift = isize_of(to) - isize_of(from);
        if trusted && from < history {
            shift == base && rows[to].hash == old[from].hash
        } else {
            rows[to].hash == old[from].hash || (shift == base && grows(from, to))
        }
    };
    let pairs = align::keep(&old_text, &window, matches, base);
    // Found by their text, the rows did not move where most of them went.
    let fixed = if trusted {
        base
    } else {
        commonest_shift(&pairs, base)
    };
    let how = How {
        fixed,
        place: base,
        rotation: usize::try_from(base - fixed).unwrap_or(0),
        history,
        free: !trusted,
        whole: false,
        shown: Vec::new(),
    };
    let mut aligned = classify(memory, rows, read, caret, &pairs, how);
    aligned.ambiguous = !trusted && ambiguous(memory, rows, 0..rows.len());
    aligned
}

/// The shift most of `pairs` share, the topmost on a tie: where a row that
/// did not move went. `base` when nothing was kept.
fn commonest_shift(pairs: &[(usize, usize)], base: isize) -> isize {
    let mut shifts: HashMap<isize, (usize, usize)> = HashMap::new();
    for (at, &(from, to)) in pairs.iter().enumerate() {
        let entry = shifts
            .entry(isize_of(to) - isize_of(from))
            .or_insert((0, at));
        entry.0 += 1;
    }
    shifts
        .into_iter()
        .max_by_key(|&(_, (count, first))| (count, std::cmp::Reverse(first)))
        .map_or(base, |(shift, _)| shift)
}

/// Aligns `rows`, the whole document a lost read asked for, with `memory`
/// ("A lost read"), and says how what went by is counted: when the last
/// read's rows fit at more than one place, only the visible screen is
/// aligned and the rows above it are history, uncounted; otherwise the
/// alignment is the longest common subsequence of rows with text, the rows
/// above the topmost row kept are history, and the rows below the last
/// read's are new, counted as about; when nothing of the last read is
/// found, every row above the visible screen is new, counted as more than.
fn align_whole(
    memory: &Screen,
    (rows, height): (&[Row], usize),
    read: &Read,
    caret: Option<usize>,
) -> (Aligned, Lost) {
    let old = &memory.rows;
    let history = memory.screen_start();
    let screen = rows.len().saturating_sub(height);
    let uncounted = ambiguous(memory, rows, 0..rows.len());
    let within = if uncounted { screen } else { 0 };
    let old_text: Vec<usize> = (0..old.len())
        .filter(|&from| !old[from].is_blank())
        .collect();
    let hashes: HashSet<u64> = old_text.iter().map(|&from| old[from].hash).collect();
    let window: Vec<usize> = (within..rows.len())
        .filter(|&to| !rows[to].is_blank() && hashes.contains(&rows[to].hash))
        .collect();
    let pairs = align::keep(
        &old_text,
        &window,
        |from, to| rows[to].hash == old[from].hash,
        0,
    );
    // The last read's rows found are those at or above its caret, as in
    // the test that made the read lost: a footer below the caret stays
    // where it is while the output passes it.
    let caret_row = memory
        .caret
        .and_then(|row| row.checked_sub(memory.skipped))
        .unwrap_or(old.len());
    let found = pairs.iter().find(|&&(from, _)| from <= caret_row);
    let lost = match found {
        _ if uncounted => Lost::Uncounted,
        None => Lost::MoreThan,
        Some(_) => Lost::About,
    };
    // History, silent: above the visible screen when only it is aligned,
    // and above the topmost row found otherwise.
    let top = match (lost, found) {
        (Lost::Uncounted, _) => screen,
        (Lost::About, Some(&(_, to))) => to.min(screen),
        _ => 0,
    };
    let shown: Vec<usize> = (0..top).collect();
    let fixed = commonest_shift(&pairs, 0);
    // A place keeps its distance from the document's end, the last read
    // having read to it too.
    let place = isize_of(rows.len()) - isize_of(memory.rows.len());
    let how = How {
        fixed,
        place,
        rotation: 0,
        history,
        free: true,
        whole: true,
        shown,
    };
    (classify(memory, rows, read, caret, &pairs, how), lost)
}

/// Finishes the alignment of `rows` with `memory` from its kept pairs
/// `kept` ("The alignment"): a row whose text is once among the old rows
/// and once in the read is kept where it went; a row whose text grew where
/// it was is kept; a blank row is kept where a kept row beside it puts it;
/// the rows left between two kept rows, or a kept row and the read's edge,
/// are paired as rewritten when they share half the longer row's
/// characters in order, and a read with no kept row pairs nothing; a gone
/// row that a program had rewritten, or that was at or below the caret,
/// lingers; and a new row with a lingering row's text is that row back.
#[expect(
    clippy::too_many_lines,
    reason = "the alignment's steps in the design's order, each short"
)]
fn classify(
    memory: &Screen,
    rows: &[Row],
    read: &Read,
    caret: Option<usize>,
    kept: &[(usize, usize)],
    how: How,
) -> Aligned {
    let How {
        fixed,
        place: place_shift,
        rotation,
        history,
        free,
        whole,
        shown,
    } = how;
    let old = &memory.rows;
    let (count, total) = (old.len(), rows.len());
    let place = |from: usize| isize_of(from) + fixed;
    let mut new = vec![NewRow::New; total];
    let mut fates = vec![OldRow::Gone { lingering: false }; count];
    for &(from, to) in kept {
        new[to] = if rows[to].hash == old[from].hash {
            NewRow::Kept {
                from,
                moved: isize_of(to) != place(from),
            }
        } else {
            NewRow::Grown { from }
        };
        fates[from] = OldRow::Kept(to);
    }
    for &to in &shown {
        if new[to] == NewRow::New {
            new[to] = NewRow::Shown;
        }
    }
    // A row whose text is on no other row of the old rows or of the read is
    // kept where that text went, even across rows the subsequence kept, as
    // a list entry moved past others.
    let mut once_new: HashMap<u64, Option<usize>> = HashMap::new();
    for (to, row) in rows.iter().enumerate() {
        if !row.is_blank() {
            once_new
                .entry(row.hash)
                .and_modify(|at| *at = None)
                .or_insert(Some(to));
        }
    }
    let mut once_old: HashMap<u64, Option<usize>> = HashMap::new();
    for (from, row) in old.iter().enumerate() {
        if !row.is_blank() {
            once_old
                .entry(row.hash)
                .and_modify(|at| *at = None)
                .or_insert(Some(from));
        }
    }
    for from in history..count {
        if fates[from] != (OldRow::Gone { lingering: false })
            || once_old.get(&old[from].hash) != Some(&Some(from))
        {
            continue;
        }
        if let Some(&Some(to)) = once_new.get(&old[from].hash)
            && new[to] == NewRow::New
        {
            new[to] = NewRow::Kept {
                from,
                moved: isize_of(to) != place(from),
            };
            fates[from] = OldRow::Kept(to);
        }
    }
    // A row whose text grew where it was is kept, as the subsequence over
    // places found by text, which compares hashes, cannot find; found by
    // text, a row's place is known only from the rows kept around it.
    if free && !kept.is_empty() {
        for from in history..count {
            if fates[from] != (OldRow::Gone { lingering: false }) || old[from].is_blank() {
                continue;
            }
            if let Ok(to) = usize::try_from(place(from))
                && to < total
                && new[to] == NewRow::New
                && rows[to].text.len() > old[from].text.len()
                && rows[to].text.starts_with(&*old[from].text)
            {
                new[to] = NewRow::Grown { from };
                fates[from] = OldRow::Kept(to);
            }
        }
    }
    // A blank row is kept where the kept row above it, or else below it,
    // puts it.
    let mut placed = kept.iter().peekable();
    let mut shift_above = None;
    for from in 0..count {
        while let Some(&&(kept_from, kept_to)) = placed.peek() {
            if kept_from > from {
                break;
            }
            shift_above = Some(isize_of(kept_to) - isize_of(kept_from));
            placed.next();
        }
        if !old[from].is_blank() {
            continue;
        }
        let below = placed
            .peek()
            .map(|&&(kept_from, kept_to)| isize_of(kept_to) - isize_of(kept_from));
        let row_shift = shift_above.or(below).unwrap_or(fixed);
        let found = usize::try_from(isize_of(from) + row_shift)
            .ok()
            .filter(|&to| to < total && rows[to].is_blank() && new[to] == NewRow::New);
        if let Some(to) = found {
            new[to] = NewRow::Kept {
                from,
                moved: row_shift != fixed,
            };
            fates[from] = OldRow::Kept(to);
        }
    }
    // Rows above where the memory's first row is are history in a read
    // from the kept start; in a whole document, the rows `shown` are.
    // Found by text, a row paired with nothing is new wherever it is; but
    // a refused start reads from a screen above the screen, and the rows
    // it holds above where the last read's first row went are history,
    // which no program writes ("The read").
    let top = if whole || (free && read.from_start) {
        0
    } else {
        usize::try_from(fixed).unwrap_or(0).min(total)
    };
    for kind in &mut new[..top] {
        if *kind == NewRow::New {
            *kind = NewRow::Shown;
        }
    }
    // The rows left between two kept rows, or a kept row and the read's
    // edge, paired by likeness ("The alignment"); a blank row is alike to
    // none, and a history row is never rewritten. A read with no kept row
    // pairs nothing.
    if !kept.is_empty() {
        let mut bounds: Vec<(Option<Pair>, Option<Pair>)> = Vec::new();
        let mut previous = None;
        for &pair in kept {
            bounds.push((previous, Some(pair)));
            previous = Some(pair);
        }
        bounds.push((previous, None));
        for (above, below) in bounds {
            let old_range =
                above.map_or(0, |(from, _)| from + 1)..below.map_or(count, |(from, _)| from);
            let new_range = above.map_or(0, |(_, to)| to + 1)..below.map_or(total, |(_, to)| to);
            let prefer = below
                .or(above)
                .map_or(fixed, |(from, to)| isize_of(to) - isize_of(from));
            let old_gap: Vec<(usize, Likeness)> = old_range
                .filter(|&from| {
                    from >= history
                        && place(from) >= 0
                        && fates[from] == OldRow::Gone { lingering: false }
                        && !old[from].is_blank()
                })
                .map(|from| (from, Likeness::of(&old[from].text)))
                .collect();
            if old_gap.is_empty() {
                continue;
            }
            let new_gap: Vec<(usize, Likeness)> = new_range
                .filter(|&to| new[to] == NewRow::New && !rows[to].is_blank())
                .map(|to| (to, Likeness::of(&rows[to].text)))
                .collect();
            let old_gap: Vec<(usize, &Likeness)> =
                old_gap.iter().map(|(at, like)| (*at, like)).collect();
            let new_gap: Vec<(usize, &Likeness)> =
                new_gap.iter().map(|(at, like)| (*at, like)).collect();
            for (from, to) in align::pair(&old_gap, &new_gap, prefer) {
                new[to] = NewRow::Rewritten {
                    from,
                    moved: isize_of(to) != place(from),
                    alone: false,
                };
                fates[from] = OldRow::Rewritten(to);
            }
        }
    }
    // The caret's line is identified by its first row ("Keys"): when the
    // caret's line now starts where the last read's caret line, which had
    // text, started, and that row is otherwise new, it is that line
    // rewritten.
    if let (Some(old_row), Some(caret)) = (
        memory
            .caret
            .and_then(|row| row.checked_sub(memory.skipped))
            .filter(|&row| row < count),
        caret,
    ) {
        let first = line_start(old, old_row);
        let to = line_start(rows, caret);
        if !free
            && isize_of(to) == place(first)
            && !old[first].is_blank()
            && fates[first] == (OldRow::Gone { lingering: false })
            && new[to] == NewRow::New
        {
            new[to] = NewRow::Rewritten {
                from: first,
                moved: false,
                alone: false,
            };
            fates[first] = OldRow::Rewritten(to);
        }
    }
    // A row rewritten elsewhere than its place moved with its neighbours
    // when a row next to it was kept in the same move.
    for to in 0..new.len() {
        let NewRow::Rewritten {
            from, moved: true, ..
        } = new[to]
        else {
            continue;
        };
        let distance = isize_of(to) - place(from);
        let with_neighbour = [to.checked_sub(1), Some(to + 1)]
            .into_iter()
            .flatten()
            .any(|next| match new.get(next) {
                Some(NewRow::Kept { from: kept, .. } | NewRow::Grown { from: kept }) => {
                    isize_of(next) - place(*kept) == distance
                }
                _ => false,
            });
        new[to] = NewRow::Rewritten {
            from,
            moved: true,
            alone: !with_neighbour,
        };
    }
    // Gone rows that linger, and lingering rows taken back.
    let old_caret = memory.caret.and_then(|row| row.checked_sub(memory.skipped));
    for from in 0..count {
        if fates[from] != (OldRow::Gone { lingering: false }) {
            continue;
        }
        let at = place(from);
        // A row the rotation carried past the read's start is in the
        // history, or out of the buffer when the read starts at the
        // document's start.
        if at < 0 {
            fates[from] = if read.at_document_start {
                OldRow::Left
            } else {
                OldRow::Above
            };
            continue;
        }
        if old[from].is_blank() {
            continue;
        }
        let below_caret = old_caret.is_some_and(|row| from >= row)
            || caret.is_some_and(|row| at >= isize_of(row));
        if old[from].redrawn || below_caret {
            fates[from] = OldRow::Gone { lingering: true };
        }
    }
    let skip = isize_of(memory.skipped);
    let mut lingering: Vec<(isize, u64)> = memory
        .lingering
        .iter()
        .filter(|row| read.at_ms.saturating_sub(row.since_ms) <= LINGERING_MS)
        .map(|row| (row.at - skip + fixed, row.row.hash))
        .collect();
    for to in 0..total {
        if new[to] != NewRow::New || rows[to].is_blank() {
            continue;
        }
        let nearest = lingering
            .iter()
            .enumerate()
            .filter(|(_, (_, hash))| *hash == rows[to].hash)
            .min_by_key(|(_, (at, _))| (at - isize_of(to)).abs())
            .map(|(index, &(at, _))| (index, at));
        if let Some((index, at)) = nearest {
            new[to] = NewRow::Returned {
                moved: at != isize_of(to),
            };
            lingering.remove(index);
        }
    }
    let caret_rows = |from: &usize| old_caret.is_none_or(|row| *from <= row);
    let kept_caret = (0..count).filter(caret_rows).any(|from| {
        !old[from].is_blank()
            && matches!(fates[from], OldRow::Kept(to) if !matches!(new[to], NewRow::Shown))
    });
    let kept_text =
        (0..count).any(|from| !old[from].is_blank() && matches!(fates[from], OldRow::Kept(_)));
    let paired = fates
        .iter()
        .any(|fate| matches!(fate, OldRow::Rewritten(_)));
    Aligned {
        new,
        old: fates,
        fixed,
        place: place_shift,
        rotation,
        kept_caret,
        kept_text,
        paired,
        ambiguous: false,
        left: read.at_document_start,
    }
}

/// The memory after a read aligned with `memory`: its rows, each with its
/// history carried from the old row it was, the lingering rows, the
/// remembered history and the anchors.
fn remember(
    memory: &Screen,
    mut rows: Vec<Row>,
    aligned: &Aligned,
    read: &Read,
    (width, height): (usize, usize),
    caret: Option<usize>,
) -> Screen {
    let now = read.at_ms;
    let skip = isize_of(memory.skipped);
    // Where a row of the last read that did not move is now, counted as
    // the last read's rows are.
    let shift = aligned.fixed - skip;
    let mut lingering: Vec<Lingering> = memory
        .lingering
        .iter()
        .filter(|row| now.saturating_sub(row.since_ms) <= LINGERING_MS)
        .map(|row| Lingering {
            at: row.at + shift,
            ..row.clone()
        })
        .collect();
    for (to, kind) in aligned.new.iter().enumerate() {
        let row = &mut rows[to];
        match *kind {
            NewRow::Kept { from, .. } | NewRow::Grown { from } => {
                row.redrawn = memory.rows[from].redrawn;
            }
            NewRow::Rewritten { .. } => row.redrawn = true,
            NewRow::Returned { .. } => {
                // The lingering row it is, taken back.
                if let Some(index) = lingering
                    .iter()
                    .enumerate()
                    .filter(|(_, gone)| gone.row.hash == row.hash)
                    .min_by_key(|(_, gone)| (gone.at - isize_of(to)).abs())
                    .map(|(index, _)| index)
                {
                    lingering.remove(index);
                }
                row.redrawn = true;
            }
            NewRow::New | NewRow::Shown => {}
        }
    }
    for (from, fate) in aligned.old.iter().enumerate() {
        if *fate == (OldRow::Gone { lingering: true }) {
            lingering.push(Lingering {
                row: memory.rows[from].clone(),
                since_ms: now,
                at: isize_of(from) + aligned.fixed,
            });
        }
    }
    let mut next = Screen {
        next: next_of(rows.len(), read.above, memory.height),
        rows,
        skipped: 0,
        width,
        height,
        at_document_start: read.at_document_start,
        caret,
        shows_text: !crate::trim_padding(&read.screen).trim().is_empty(),
        no_history: read.alternate,
        lingering,
        review: None,
        marks: Vec::new(),
        lost_marks: memory.lost_marks.clone(),
        places: Vec::new(),
        continued: false,
    };
    let fates = aligned.fates(memory);
    next.review = memory
        .review
        .and_then(|anchor| map_anchor(anchor, memory, aligned, &fates, &next, read));
    next.marks = memory
        .marks
        .iter()
        .filter_map(|&mark| map_mark(mark, &fates, shift, &next.rows))
        .collect();
    next.places = memory
        .places
        .iter()
        .filter_map(|&(number, anchor)| {
            map_anchor(anchor, memory, aligned, &fates, &next, read).map(|anchor| (number, anchor))
        })
        .collect();
    next
}

/// An anchor on row `row` of `rows`, at `column` clamped to its text.
fn anchor_at(rows: &[Row], row: usize, column: usize) -> Anchor {
    let found = &rows[row];
    Anchor {
        row: isize_of(row),
        column: column.min(found.text.chars().count()),
        line_hash: found.line_hash,
    }
}

/// The review position, or a place, after a read ("The review anchor and
/// following"): a kept row carries it to the row's new place, a rewritten
/// row keeps it with its column clamped to the row's text, and so does a
/// blank row written over in place, which the alignment, pairing no blank
/// row, finds gone; a gone row moves it to the nearest surviving row above,
/// or to the caret when nothing above survived; above the read, it moves
/// with the history, and a row there survives only while the buffer holds
/// it: not above a last read, `memory`'s, that started at the document's
/// start, nor above as many rows as the buffer holds above the read, when
/// `next`, the memory after the read, knows how many.
fn map_anchor(
    anchor: Anchor,
    memory: &Screen,
    aligned: &Aligned,
    fates: &[OldRow],
    next: &Screen,
    read: &Read,
) -> Option<Anchor> {
    let rows = &next.rows;
    let shift = aligned.fixed - isize_of(memory.skipped);
    // Above the last read an anchor keeps its place ("The review anchor").
    let place = aligned.place - isize_of(memory.skipped);
    // Where row `row` of the last read is now, when it survived.
    let surviving = |row: isize| -> Option<isize> {
        let to = match usize::try_from(row) {
            Err(_) if memory.at_document_start => return None,
            Err(_) => row + place,
            Ok(from) => match fates.get(from)? {
                OldRow::Kept(to) | OldRow::Rewritten(to) => return Some(isize_of(*to)),
                OldRow::Above => row + place,
                OldRow::Gone { .. } => {
                    let to = row + shift;
                    let written = memory.row(from).is_some_and(Row::is_blank)
                        && usize::try_from(to)
                            .is_ok_and(|to| aligned.new.get(to) == Some(&NewRow::New));
                    return written.then_some(to);
                }
                OldRow::Left => return None,
            },
        };
        // A read from the document's start holds every place there is: a
        // place above it left the buffer, and the nearest is its first row.
        Some(if read.at_document_start {
            to.max(0)
        } else {
            to
        })
    };
    let mut row = anchor.row;
    let found = loop {
        if let Some(to) = surviving(row) {
            break Some(to);
        }
        // Rows further above the read than one that left are gone too.
        if row < 0 {
            break None;
        }
        row -= 1;
    };
    match found {
        Some(to) => Some(match usize::try_from(to) {
            Ok(to) if to < rows.len() => anchor_at(rows, to, anchor.column),
            _ => Anchor { row: to, ..anchor },
        }),
        None => next
            .caret
            .map(|row| anchor_at(rows, row, read.caret_column)),
    }
}

/// A command mark after a read ("Moving by command"): it moves with its
/// line, and is dropped when its line is gone.
fn map_mark(mark: Anchor, fates: &[OldRow], shift: isize, rows: &[Row]) -> Option<Anchor> {
    let to = match usize::try_from(mark.row) {
        Err(_) => {
            return Some(Anchor {
                row: mark.row + shift,
                ..mark
            });
        }
        Ok(from) => match fates.get(from)? {
            OldRow::Kept(to) => *to,
            OldRow::Above => {
                return Some(Anchor {
                    row: mark.row + shift,
                    ..mark
                });
            }
            OldRow::Rewritten(_) | OldRow::Gone { .. } | OldRow::Left => return None,
        },
    };
    let first = line_start(rows, to);
    (rows[first].line_hash == mark.line_hash).then(|| Anchor {
        row: isize_of(first),
        column: 0,
        line_hash: mark.line_hash,
    })
}

/// The first row of the line holding row `row`.
fn line_start(rows: &[Row], row: usize) -> usize {
    let mut first = row;
    while first > 0 && rows[first - 1].wraps {
        first -= 1;
    }
    first
}

/// The lines of `rows`, without their growth, which [`grew_from`] adds.
fn lines(rows: &[Row]) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut first = 0;
    for (at, row) in rows.iter().enumerate() {
        if row.wraps && at + 1 < rows.len() {
            continue;
        }
        // A line's last row has no padding, so a one-row line's text is
        // its row's.
        let (text, line_hash) = if first == at {
            (Arc::clone(&row.text), row.hash)
        } else {
            let text = line_text(&rows[first..=at]);
            let line_hash = hash(&text);
            (Arc::from(text), line_hash)
        };
        lines.push(Line {
            rows: first..at + 1,
            hash: line_hash,
            text,
            grew_from: None,
        });
        first = at + 1;
    }
    lines
}

/// The text of a line's rows, joined, padding left out.
fn line_text(rows: &[Row]) -> String {
    let joined: String = rows.iter().map(|row| &*row.text).collect();
    crate::trim_padding(&joined).to_owned()
}

/// Whether `line` only gained at its end since the last read: its first
/// row is an old row that started a line, or for a line continued from
/// above the read the row that started its part in the old read, kept or
/// grown where it was, and its text begins with that line's and is longer.
/// `kinds` count old rows as `memory`'s rows are.
fn grew_from(line: &Line, kinds: &[NewRow], memory: &Screen, continued: bool) -> Option<usize> {
    let old = &memory.rows;
    let (NewRow::Kept { from, moved: false }
    | NewRow::Grown { from }
    | NewRow::Rewritten {
        from, moved: false, ..
    }) = *kinds.get(line.rows.start)?
    else {
        return None;
    };
    let starts = if from == 0 {
        !memory.continued
    } else {
        !old[from - 1].wraps
    };
    if !(starts || continued && line.rows.start == 0) {
        return None;
    }
    let end = (from..old.len())
        .find(|&at| !old[at].wraps)
        .unwrap_or(old.len() - 1);
    let was = line_text(&old[from..=end]);
    (!was.is_empty() && line.text.len() > was.len() && line.text.starts_with(&was))
        .then_some(was.len())
}

/// The screen's width and height ("Rows and the width"). Every line's
/// padded length is a whole number of rows, except Windows Terminal's last
/// row of the buffer, cut one cell short ("The terminals"), so the width
/// is the narrowest of the screen's lines but its last, in cells. When each
/// of those lines wraps, the narrowest is more than a row, so the caret's
/// row, which the read gives as one row, is taken instead when its length,
/// or one cell more if it is the row Windows Terminal cuts, fits every
/// line; when both fit, the one that gives the screen the height it had
/// at the last read.
fn measure(screen: &str, caret_row: &str, last_height: Option<usize>) -> (usize, usize) {
    let screen = screen.strip_suffix("\r\n").unwrap_or(screen);
    let lines: Vec<usize> = screen.split("\r\n").map(crate::cell_width).collect();
    let (last, others) = lines
        .split_last()
        .map_or((0, &[][..]), |(last, others)| (*last, others));
    let height = |width: usize| -> usize {
        lines
            .iter()
            .map(|&cells| cells.div_ceil(width).max(1))
            .sum()
    };
    let fits = |width: usize| {
        width > 0
            && others.iter().all(|cells| cells % width == 0)
            && (last % width == 0 || (last + 1) % width == 0)
    };
    let caret = crate::cell_width(caret_row.strip_suffix("\r\n").unwrap_or(caret_row));
    let fitting: Vec<usize> = [caret, caret + 1]
        .into_iter()
        .filter(|&width| caret > 0 && fits(width))
        .collect();
    let width = fitting
        .iter()
        .copied()
        .find(|&width| Some(height(width)) == last_height)
        .or_else(|| fitting.first().copied())
        .or_else(|| others.iter().copied().filter(|&cells| cells > 0).min())
        .unwrap_or(last);
    if width == 0 {
        return (0, lines.len());
    }
    (width, height(width))
}

/// The rows of a read's text: its lines split at line breaks, each into
/// rows of `width` cells, padding trimmed from each line's last row only
/// ("Rows").
fn split_rows(text: &str, width: usize) -> Vec<Row> {
    let text = text.strip_suffix("\r\n").unwrap_or(text);
    let mut rows = Vec::new();
    for line in text.split("\r\n") {
        let pieces = row_pieces(line, width);
        let last = pieces.len() - 1;
        // Padding is the spaces after the line's last character ("Rows"):
        // a wrapped row keeps the spaces where it wrapped, but not those
        // after the line's last character.
        let end = crate::trim_padding(line).len();
        let line_hash = hash(&line[..end]);
        let mut offset = 0;
        for (at, piece) in pieces.into_iter().enumerate() {
            let start = offset;
            offset += piece.len();
            let piece = if at == last {
                crate::trim_padding(piece)
            } else {
                &piece[..piece.len().min(end.saturating_sub(start))]
            };
            rows.push(Row {
                hash: hash(piece),
                text: Arc::from(piece),
                wraps: at < last,
                line_hash,
                redrawn: false,
            });
        }
    }
    rows
}

/// One line of a read split into rows of `width` cells, padding kept: at
/// least one row, empty for an empty line.
fn row_pieces(line: &str, width: usize) -> Vec<&str> {
    let mut pieces: Vec<&str> = Vec::new();
    if line.is_ascii() && width > 0 {
        // Every printable ASCII character takes one cell.
        let mut at = 0;
        loop {
            let end = (at + width).min(line.len());
            pieces.push(&line[at..end]);
            at = end;
            if at >= line.len() {
                break;
            }
        }
    } else {
        let (mut start, mut cells) = (0, 0);
        for (at, c) in line.char_indices() {
            let cell = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if width > 0 && cells + cell > width && at > start {
                pieces.push(&line[start..at]);
                (start, cells) = (at, 0);
            }
            cells += cell;
        }
        pieces.push(&line[start..]);
    }
    pieces
}

/// The caret's row in a read of `rows` rows, from how many rows the
/// caret's row and those below it hold.
fn caret_row(rows: usize, to_end: u32) -> Option<usize> {
    let to_end = usize::try_from(to_end).ok()?;
    (to_end > 0 && to_end <= rows).then(|| rows - to_end)
}

/// The rows of `text`, a read's text from a row's start, split as the
/// memory splits a read ("Rows"): its lines split at line breaks, each
/// into rows of `width` cells, padding trimmed from each line's last row
/// only. For a caller that reads rows the memory does not keep, such as the
/// history above them, at the width a read found.
#[must_use]
pub fn rows_of(text: &str, width: usize) -> Vec<Row> {
    split_rows(text, width)
}

/// A 64-bit hash of `text` (FNV-1a): the same in every process and build,
/// so hashes kept in the memory compare across reads.
#[must_use]
pub fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn isize_of(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

#[cfg(test)]
mod tests;
