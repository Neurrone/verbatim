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
//! top of the last read's screen, at the provider range the last read kept,
//! so a row that did not move has a known place in it once the rotation is
//! known: the rows above the last read's screen are history, which keeps
//! its text and moves only by the rotation, and the rotation is the one
//! those rows allow that keeps the most of the last read, in the fewest
//! blocks, moving the fewest rows. When the read keeps no row with text
//! that the last read had at or above the caret, or its start was refused,
//! the whole document is read once and aligned with the remembered history
//! and the last read; when even that keeps nothing, the screen changed
//! ("Screens") or the history overflowed ("Starting again").
//!
//! Once a read is handled, the memory keeps of it only the screen above the
//! screen and the rows below it: the rows above are held as hashes in the
//! remembered history. Rows keep their places, counted from the read's
//! first row, so what refers to a row of the last read is unchanged by the
//! trim.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;
use std::sync::Arc;

mod align;

use align::Likeness;

/// How long a gone row lingers ("Lingering"), in milliseconds.
pub const LINGERING_MS: u64 = 2000;

/// The most row hashes the remembered history keeps. It holds only the rows
/// the buffer still holds above the memory's ("The memory"), so the
/// terminal's history bounds it, 9,001 rows by default in both terminals,
/// while every rotation is measured; a rotation that cannot be measured
/// leaves the hashes of rows that already left until a read reaches the
/// document's start, and this limit bounds those.
const KNOWN_LIMIT: usize = 32_768;

/// What the remembered history holds for a row that went by unread: no
/// row's hash, since [`hash`] never gives it. The alignment's histogram
/// diff takes it, and a blank row's hash, as matching nothing.
const UNREAD: u64 = 0;

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
    },
    /// A lingering row taken back: a row a read found gone, back with its
    /// text ("Lingering").
    Returned {
        /// Whether it came back elsewhere than where it was.
        moved: bool,
    },
    /// A row whose text was shown before but that carries no row of the
    /// last read: a row of the history above the last read, a row the
    /// remembered history held, or, returning to a screen set aside, a row
    /// the last read showed ("Screens").
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
    /// A new screen one screen tall, as the alternate screen opening: the
    /// last read's screen is set aside, unless a screen is aside already.
    Opened,
    /// The read kept rows of the screen set aside, which is restored.
    Closed,
    /// A new screen of another shape, as Windows Terminal's clear leaving a
    /// prompt: what is aside is dropped.
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
    /// How many rows the history rotated. A whole-document read counts it
    /// in the rows' shift instead, and gives 0.
    pub rotation: usize,
    /// More went by than the history holds ("Starting again"): every row is
    /// new.
    pub overflowed: bool,
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
struct Screen {
    rows: Vec<Row>,
    /// How many of the last read's rows lie above `rows`, trimmed once the
    /// read was handled: their hashes are the newest of `known`.
    skipped: usize,
    /// The screen's width and height at the last read.
    width: usize,
    height: usize,
    /// The last read's row at which the next read starts, the top of its
    /// screen.
    next: usize,
    /// Why the next read's start cannot be placed among the rows, when a
    /// read with no text since the last moved it where the memory cannot
    /// follow: the next read is then read whole.
    unplaced: Option<Pending>,
    /// Whether the last read started at the document's start.
    at_document_start: bool,
    caret: Option<usize>,
    /// Whether the last read's screen showed text.
    screen_text: bool,
    lingering: Vec<Lingering>,
    /// The remembered history: the hashes of the rows above `rows`, oldest
    /// first, [`UNREAD`] for a row no read showed.
    known: VecDeque<u64>,
    /// How many rows the buffer holds above the remembered history, when a
    /// read from the document's start told; `None` when no read since the
    /// first has started there.
    unknown_above: Option<usize>,
    review: Option<Anchor>,
    marks: Vec<Anchor>,
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

    /// How many rows the document held at the last read, at least: the
    /// remembered history and the rows, and the rows above them when known.
    fn document_rows(&self) -> usize {
        self.unknown_above.unwrap_or(0) + self.known.len() + self.rows.len()
    }

    /// Keeps only a screen above the next read's start and the rows below
    /// it, the hashes of those above going to the remembered history.
    /// Returns how many rows it dropped.
    fn trim(&mut self) -> usize {
        let dropped = self.trim_point();
        if dropped == 0 {
            return 0;
        }
        self.known
            .extend(self.rows[..dropped].iter().map(|row| row.hash));
        let over = self.known.len().saturating_sub(KNOWN_LIMIT);
        self.known.drain(..over);
        self.unknown_above = self.unknown_above.map(|unknown| unknown + over);
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
            screen_text: !crate::trim_padding(&read.screen).trim().is_empty(),
            unknown_above: read.at_document_start.then_some(0),
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

/// Why the memory asked for the whole document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    /// The read's start was refused: the text shrank, as after a clear, so
    /// the rows above the last read may be printed again.
    Refused,
    /// The read kept no row with text that the last read had at or above
    /// the caret, or its start could not be used.
    Lost,
}

/// The memory of one terminal ("The memory"), kept while the terminal
/// exists, across focus changes.
#[derive(Clone, Debug, Default)]
pub struct TerminalMemory {
    screen: Option<Screen>,
    /// The screen set aside ("Screens").
    aside: Option<Screen>,
    /// What was asked for, until it is read.
    pending: Option<Pending>,
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
    /// the next read's start and the rows below it, the rows above held as
    /// hashes in the remembered history. The reading rules call it once
    /// they have handled a read, and every read first trims what the last
    /// left; the rows keep their places, so nothing counted from the last
    /// read's first row moves. Returns how many rows it dropped.
    pub fn trim(&mut self) -> usize {
        self.screen.as_mut().map_or(0, Screen::trim)
    }

    /// The hashes of the rows remembered above the rows the memory keeps,
    /// oldest first.
    #[must_use]
    pub fn known_history(&self) -> Vec<u64> {
        self.screen
            .as_ref()
            .map_or_else(Vec::new, |screen| screen.known.iter().copied().collect())
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
    /// position does, goes aside with its screen, and is dropped when its
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
        let pending = self.pending.take();
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
            return self.start_again(&current, rows, read, shape, Start::Resize);
        }
        // A read whose screen shows no text where the last read's had text
        // is a read between a program's clear and its reprint: it changes
        // nothing ("A read with no text"), but where the next read starts,
        // which the caller takes from every read.
        if current.screen_text && crate::trim_padding(&read.screen).trim().is_empty() {
            let mut current = current;
            if read.from_start && !read.shrank && current.unplaced.is_none() {
                // The read's row `above` is the last read's next start.
                let next = next_of(rows.len(), read.above, current.height);
                current.next = (current.next + next).saturating_sub(read.above);
            } else {
                current.unplaced = Some(if read.shrank {
                    Pending::Refused
                } else {
                    current.unplaced.unwrap_or(Pending::Lost)
                });
            }
            self.screen = Some(current);
            return Outcome {
                blank: true,
                ..shape
            };
        }
        // A read from the document's start other than from the kept start
        // is whole: the whole document asked for, or a refused start that
        // reached the document's start.
        let whole = read.at_document_start && !read.from_start;
        let refused =
            read.shrank || pending == Some(Pending::Refused) || current.unplaced == Some(Pending::Refused);
        if !whole && (!read.from_start || read.shrank || current.unplaced.is_some()) {
            self.pending = Some(if refused {
                Pending::Refused
            } else {
                Pending::Lost
            });
            self.screen = Some(current);
            return Outcome {
                whole_document: true,
                ..shape
            };
        }
        let aligned = if whole {
            align_whole(&current, (&rows, height), read, caret, refused)
        } else {
            align_read(&current, &rows, read, caret)
        };
        if aligned.lost() {
            // A read that reached the document's start is whole already.
            if !whole && !read.at_document_start {
                self.pending = Some(Pending::Lost);
                self.screen = Some(current);
                return Outcome {
                    whole_document: true,
                    ..shape
                };
            }
            return self.screens(current, rows, read, shape);
        }
        let next = remember(&current, rows, &aligned, read, (width, height), caret);
        self.finish(&current, next, aligned, shape, Switch::None)
    }

    /// Takes `read`, for which [`TerminalMemory::read`] asked for the whole
    /// document, without that read, as when "report new output" is off and
    /// nothing is read that is not needed ("The read"): the memory starts
    /// again from it ("Starting again").
    pub fn restart(&mut self, read: &Read) -> Outcome {
        self.pending = None;
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
        match self.screen.take() {
            Some(current) => self.start_again(&current, rows, read, shape, Start::Restart),
            None => {
                let outcome = Outcome {
                    first: true,
                    rows: vec![NewRow::New; rows.len()],
                    lines: lines(&rows),
                    ..shape
                };
                self.screen = Some(Screen::fresh(rows, read, (width, height), 0));
                outcome
            }
        }
    }

    /// The memory after a read aligned with `current`, as `next`, and the
    /// outcome.
    fn finish(
        &mut self,
        current: &Screen,
        mut next: Screen,
        aligned: Aligned,
        shape: Outcome,
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
                NewRow::Rewritten { from, moved } => NewRow::Rewritten {
                    from: from + skip,
                    moved,
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
            ..shape
        };
        self.screen = Some(next);
        outcome
    }

    /// The whole document keeps no row with text that the last read had at
    /// or above the caret ("Screens"): it returns to the screen set aside
    /// when it keeps rows of it; otherwise a document one screen tall is a
    /// new screen, setting the last read's aside unless one is aside
    /// already; one longer than a screen and no shorter than before means
    /// the history overflowed; and any other is a new screen, dropping what
    /// is aside. On a new screen every row is new and the review position is
    /// at the caret.
    fn screens(&mut self, current: Screen, rows: Vec<Row>, read: &Read, shape: Outcome) -> Outcome {
        let (width, height, caret) = (shape.width, shape.height, shape.caret);
        if let Some(aside) = self.aside.take() {
            let mut attempt = align_whole(&aside, (&rows, height), read, caret, false);
            if attempt.kept_text {
                // Aligned against the screen set aside and the last read
                // together: only rows in neither are new.
                let last: HashSet<u64> = current
                    .rows
                    .iter()
                    .filter(|row| !row.is_blank())
                    .map(|row| row.hash)
                    .collect();
                for (to, kind) in attempt.new.iter_mut().enumerate() {
                    if *kind == NewRow::New && last.contains(&rows[to].hash) {
                        *kind = NewRow::Shown;
                    }
                }
                let next = remember(&aside, rows, &attempt, read, (width, height), caret);
                return self.finish(&aside, next, attempt, shape, Switch::Closed);
            }
            self.aside = Some(aside);
        }
        let old = vec![OldRow::Gone { lingering: false }; current.total()];
        if rows.len() == height {
            // The last read's screen goes aside with its anchors, unless a
            // screen is aside already.
            let mut next = Screen::fresh(rows, read, (width, height), current.height);
            next.review = caret.map(|row| anchor_at(&next.rows, row, read.caret_column));
            let outcome = Outcome {
                rows: vec![NewRow::New; next.rows.len()],
                old,
                lines: lines(&next.rows),
                switch: Switch::Opened,
                ..shape
            };
            if self.aside.is_none() {
                self.aside = Some(current);
            }
            self.screen = Some(next);
            return outcome;
        }
        if rows.len() > height && rows.len() >= current.document_rows() {
            return self.start_again(&current, rows, read, shape, Start::Overflow);
        }
        self.aside = None;
        let mut next = Screen::fresh(rows, read, (width, height), current.height);
        next.review = caret.map(|row| anchor_at(&next.rows, row, read.caret_column));
        let outcome = Outcome {
            rows: vec![NewRow::New; next.rows.len()],
            old,
            lines: lines(&next.rows),
            switch: Switch::New,
            ..shape
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
        shape: Outcome,
        start: Start,
    ) -> Outcome {
        let mut next = Screen::fresh(
            rows,
            read,
            (shape.width, shape.height),
            current.height,
        );
        let total = current.total();
        let found = |anchor: &Anchor| refind(*anchor, total, &next.rows);
        next.review = current
            .review
            .as_ref()
            .and_then(found)
            .or_else(|| shape.caret.map(|row| anchor_at(&next.rows, row, read.caret_column)));
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
            overflowed: start == Start::Overflow,
            ..shape
        };
        self.screen = Some(next);
        outcome
    }
}

/// Why the memory starts again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Start {
    /// The screen's width or height changed.
    Resize,
    /// More went by than the history holds.
    Overflow,
    /// The read keeps nothing with "report new output" off.
    Restart,
}

/// An anchor found again by its line's hash, on the row starting a line
/// with it nearest its old place, counted from the end of a read of `total`
/// rows, in `rows`.
fn refind(anchor: Anchor, total: usize, rows: &[Row]) -> Option<Anchor> {
    let from_end = isize_of(total) - anchor.row;
    rows.iter()
        .enumerate()
        .filter(|(at, row)| row.line_hash == anchor.line_hash && line_start(rows, *at) == *at)
        .min_by_key(|(at, _)| (isize_of(rows.len()) - isize_of(*at) - from_end).abs())
        .map(|(at, _)| anchor_at(rows, at, anchor.column))
}

/// What aligning a read with one screen's memory gave. Old rows are counted
/// as the memory's rows are, from its first kept row.
struct Aligned {
    new: Vec<NewRow>,
    old: Vec<OldRow>,
    /// Where a memory row that did not move is in the read: its index there
    /// less its index in the memory.
    fixed: isize,
    rotation: usize,
    /// Whether the last read had a row with text at or above the caret, and
    /// whether one of them was kept.
    had_caret_text: bool,
    kept_caret: bool,
    /// Whether a row with text of the memory was kept.
    kept_text: bool,
    /// Whether the read started at the document's start, so a row the
    /// rotation carried above it left the buffer.
    left: bool,
}

impl Aligned {
    /// Whether the read keeps no row with text that the last read had at or
    /// above the caret ("The read").
    fn lost(&self) -> bool {
        self.had_caret_text && !self.kept_caret
    }

    /// What became of each row of the last read, counted from its first:
    /// those `current` trimmed, held as hashes, moved by the rotation like
    /// any history row.
    fn fates(&self, current: &Screen) -> Vec<OldRow> {
        let skip = current.skipped;
        let mut fates: Vec<OldRow> = (0..skip)
            .map(|row| {
                let to = isize_of(row) - isize_of(skip) + self.fixed;
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
    rotation: usize,
    /// The memory's rows from which on rows are not history.
    history: usize,
    /// For a whole document, whether its start was refused, so that rows
    /// above the topmost kept row may be printed again; `None` for a read
    /// from the kept start.
    whole: Option<bool>,
    /// The read's first row of its screen: rows above it are history.
    screen: usize,
    /// The read's rows the remembered history held.
    shown: Vec<usize>,
}

/// Aligns `rows`, a read from the kept start, with `memory` ("The
/// alignment"): the longest common subsequence of rows with text, in the
/// fewest blocks, moving the fewest rows, over the rotations the history
/// rows allow, the rows above the last read's screen kept only where the
/// rotation puts them. Ties go to the rotation that brings the remembered
/// history's row to the document's first row, then to the smallest.
fn align_read(memory: &Screen, rows: &[Row], read: &Read, caret: Option<usize>) -> Aligned {
    let old = &memory.rows;
    let next = isize_of(memory.next.saturating_sub(memory.skipped));
    let base = isize_of(read.above) - next;
    let history = memory.screen_start();
    let total = isize_of(rows.len());
    // The rotations the history rows allow: each such row the read holds is
    // where the rotation puts it, with its text. A screen with no history
    // above it, as the alternate screen, never rotates: a row scrolled off
    // its top is gone.
    let highest = if read.alternate {
        0
    } else {
        usize::try_from(base + isize_of(old.len()) - 1).unwrap_or(0)
    };
    let allowed: Vec<usize> = (0..=highest)
        .filter(|&rotation| {
            (0..history).all(|from| {
                let to = isize_of(from) + base - isize_of(rotation);
                to < 0 || (to < total && rows[usize::try_from(to).unwrap_or(0)].hash == old[from].hash)
            })
        })
        // The rotation is right only if the remembered history held the
        // document's first row's text as many rows below the document's
        // start; a row it does not hold rules nothing out.
        .filter(|&rotation| first_row(memory, rows, read, rotation) != Some(false))
        .collect();
    let old_text: Vec<usize> = (0..old.len()).filter(|&from| !old[from].is_blank()).collect();
    let old_hashes: HashSet<u64> = old_text.iter().map(|&from| old[from].hash).collect();
    // A row whose text is once among the old rows and once in the read is
    // kept where that text went, so it does not grow elsewhere.
    let count = |hash: u64, rows: &[Row]| rows.iter().filter(|row| row.hash == hash).count();
    let went: HashSet<u64> = old_text
        .iter()
        .map(|&from| old[from].hash)
        .filter(|&hash| count(hash, old) == 1 && count(hash, rows) == 1)
        .collect();
    let grows = |from: usize, to: usize| {
        from >= history
            && from < old.len()
            && !old[from].is_blank()
            && !went.contains(&old[from].hash)
            && rows[to].text.len() > old[from].text.len()
            && rows[to].text.starts_with(&*old[from].text)
    };
    let alignment = |rotation: usize| {
        let fixed = base - isize_of(rotation);
        let window: Vec<usize> = (0..rows.len())
            .filter(|&to| {
                !rows[to].is_blank()
                    && (old_hashes.contains(&rows[to].hash)
                        || usize::try_from(isize_of(to) - fixed).is_ok_and(|from| grows(from, to)))
            })
            .collect();
        let matches = |from: usize, to: usize| {
            let shift = isize_of(to) - isize_of(from);
            if from < history {
                shift == fixed && rows[to].hash == old[from].hash
            } else {
                rows[to].hash == old[from].hash || (shift == fixed && grows(from, to))
            }
        };
        let pairs = align::keep(&old_text, &window, matches, fixed);
        // The history rows the rotation carries above the read count as
        // kept; above a read from the document's start, they left the
        // buffer.
        let carried = (0..history)
            .filter(|&from| {
                !read.at_document_start && !old[from].is_blank() && isize_of(from) + fixed < 0
            })
            .count();
        let (kept, blocks, moved) = align::score(&pairs, fixed);
        ((kept + carried, blocks, moved), pairs)
    };
    // The candidates: the smallest rotation allowed, the rotations that keep
    // a block of the screen's rows where they were, and those that bring
    // the remembered history's row to the document's first row.
    let mut candidates: Vec<usize> = allowed.first().copied().into_iter().collect();
    if allowed.len() > 1 {
        let screen_text: Vec<usize> = old_text
            .iter()
            .copied()
            .filter(|&from| from >= history)
            .collect();
        let window: Vec<usize> = (0..rows.len())
            .filter(|&to| !rows[to].is_blank() && old_hashes.contains(&rows[to].hash))
            .collect();
        let free = align::keep(
            &screen_text,
            &window,
            |from, to| rows[to].hash == old[from].hash,
            base - isize_of(allowed[0]),
        );
        for (from, to) in free {
            if let Ok(rotation) = usize::try_from(base - (isize_of(to) - isize_of(from)))
                && allowed.binary_search(&rotation).is_ok()
            {
                candidates.push(rotation);
            }
        }
        candidates.extend(
            allowed
                .iter()
                .copied()
                .filter(|&rotation| first_row(memory, rows, read, rotation) == Some(true)),
        );
    }
    candidates.sort_unstable();
    candidates.dedup();
    let (rotation, pairs) = candidates
        .into_iter()
        .map(|rotation| {
            let (worth, pairs) = alignment(rotation);
            (rotation, worth, pairs)
        })
        .max_by_key(|(rotation, worth, _)| {
            // On a tie, a rotation the document's first row confirms, then
            // one it does not refute, then the smallest.
            let told = match first_row(memory, rows, read, *rotation) {
                Some(true) => 2,
                None => 1,
                Some(false) => 0,
            };
            (*worth, told, std::cmp::Reverse(*rotation))
        })
        .map_or((0, Vec::new()), |(rotation, _, pairs)| (rotation, pairs));
    let how = How {
        fixed: base - isize_of(rotation),
        rotation,
        history,
        whole: None,
        screen: rows.len().saturating_sub(memory.height),
        shown: Vec::new(),
    };
    classify(memory, rows, read, caret, &pairs, how)
}

/// Whether the document's first row, as `read` gives it, is the row the
/// history held `rotation` rows below the document's start at the last
/// read, in the remembered history or above the last read's screen: `None`
/// when the history did not hold that row, as for a row of the screen,
/// which may have been written over before it scrolled.
fn first_row(memory: &Screen, rows: &[Row], read: &Read, rotation: usize) -> Option<bool> {
    let at = rotation.checked_sub(memory.unknown_above?)?;
    let held = match at.checked_sub(memory.known.len()) {
        None => memory.known[at],
        Some(row) if row < memory.screen_start() => memory.rows[row].hash,
        Some(_) => return None,
    };
    if held == UNREAD {
        return None;
    }
    let first = if read.at_document_start {
        rows.first().map_or([0; 2], |row| [row.hash; 2])
    } else {
        first_row_hashes(&read.first_row)
    };
    Some(first.contains(&held))
}

/// Aligns `rows`, the whole document, with `memory`'s rows and remembered
/// history ("The alignment"): the rows with a text of the last read by the
/// longest common subsequence, and the rows above them by a histogram diff
/// over the remembered history's hashes, a row of it found again shown, not
/// new; rows above the topmost row of the memory kept and above the screen
/// are history the rotation carried, unless the document's start was
/// `refused`, as after a clear, when only those the remembered history held
/// are not new.
fn align_whole(
    memory: &Screen,
    (rows, height): (&[Row], usize),
    read: &Read,
    caret: Option<usize>,
    refused: bool,
) -> Aligned {
    let old = &memory.rows;
    let history = memory.screen_start();
    // The last read's rows: the longest common subsequence of rows with
    // text, in the fewest blocks, as in a read from the kept start, over
    // the rows of the document that have a text of them.
    let old_text: Vec<usize> = (0..old.len()).filter(|&from| !old[from].is_blank()).collect();
    let hashes: HashSet<u64> = old_text.iter().map(|&from| old[from].hash).collect();
    let window: Vec<usize> = (0..rows.len())
        .filter(|&to| !rows[to].is_blank() && hashes.contains(&rows[to].hash))
        .collect();
    let pairs = align::keep(
        &old_text,
        &window,
        |from, to| rows[to].hash == old[from].hash,
        0,
    );
    // The remembered history, above the topmost row kept, by a histogram
    // diff over the hashes: its blank rows match nothing.
    let top = pairs.first().map_or(rows.len(), |&(_, to)| to);
    let blank = hash("");
    let known: Vec<u64> = memory
        .known
        .iter()
        .map(|&hash| if hash == blank { UNREAD } else { hash })
        .collect();
    let new: Vec<u64> = rows[..top]
        .iter()
        .map(|row| if row.is_blank() { UNREAD } else { row.hash })
        .collect();
    let shown: Vec<usize> = align::histogram(&known, &new, UNREAD)
        .into_iter()
        .map(|(_, to)| to)
        .collect();
    // A row that did not move is where most rows went, so that the fewest
    // rows move; on a tie, where the topmost of them went.
    let mut shifts: HashMap<isize, (usize, usize)> = HashMap::new();
    for (at, &(from, to)) in pairs.iter().enumerate() {
        let entry = shifts
            .entry(isize_of(to) - isize_of(from))
            .or_insert((0, at));
        entry.0 += 1;
    }
    let fixed = shifts
        .into_iter()
        .max_by_key(|&(_, (count, first))| (count, std::cmp::Reverse(first)))
        .map_or(0, |(shift, _)| shift);
    let how = How {
        fixed,
        rotation: 0,
        history,
        whole: Some(refused),
        screen: rows.len().saturating_sub(height),
        shown,
    };
    classify(memory, rows, read, caret, &pairs, how)
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
        rotation,
        history,
        whole,
        screen,
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
    // A row whose text grew where it was is kept, as the subsequence of a
    // whole document, which compares hashes, cannot find.
    if whole.is_some() {
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
    // Rows above the topmost row of the memory kept are history: above
    // where its first row is, in a read from the kept start; above the
    // topmost row kept and the screen, in a whole document whose start was
    // not refused, since no program writes into the history.
    let top = match whole {
        None => usize::try_from(fixed).unwrap_or(0).min(total),
        Some(false) => kept.first().map_or(0, |&(_, to)| to).min(screen),
        Some(true) => 0,
    };
    for kind in &mut new[..top] {
        if *kind == NewRow::New {
            *kind = NewRow::Shown;
        }
    }
    // The rows left between kept rows, paired by likeness; a blank row is
    // alike to none, and a history row is never rewritten.
    if !kept.is_empty() {
        let mut bounds: Vec<(Option<(usize, usize)>, Option<(usize, usize)>)> = Vec::new();
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
                };
                fates[from] = OldRow::Rewritten(to);
            }
        }
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
    let had_caret_text = (0..count)
        .filter(caret_rows)
        .any(|from| !old[from].is_blank());
    let kept_caret = (0..count).filter(caret_rows).any(|from| {
        !old[from].is_blank()
            && matches!(fates[from], OldRow::Kept(to) if !matches!(new[to], NewRow::Shown))
    });
    let kept_text = (0..count).any(|from| !old[from].is_blank() && matches!(fates[from], OldRow::Kept(_)));
    Aligned {
        new,
        old: fates,
        fixed,
        rotation,
        had_caret_text,
        kept_caret,
        kept_text,
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
    let (known, unknown_above) = known_after(memory, aligned, read);
    let mut next = Screen {
        next: next_of(rows.len(), read.above, memory.height),
        rows,
        skipped: 0,
        width,
        height,
        at_document_start: read.at_document_start,
        caret,
        screen_text: !crate::trim_padding(&read.screen).trim().is_empty(),
        lingering,
        known,
        unknown_above,
        review: None,
        marks: Vec::new(),
        places: Vec::new(),
        continued: false,
        unplaced: None,
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

/// The remembered history after a read ("The memory"), and how many rows
/// the buffer holds above it when that is known: the rows the rotation
/// carried above the read are added to it, those the read shows again are
/// taken from it, and the rotation drops the oldest; a read from the
/// document's start holds every row, so none is above it.
fn known_after(memory: &Screen, aligned: &Aligned, read: &Read) -> (VecDeque<u64>, Option<usize>) {
    if read.at_document_start {
        return (VecDeque::new(), Some(0));
    }
    let mut known = memory.known.clone();
    let mut unknown_above = memory.unknown_above;
    // The read's first row is the memory's row `-fixed`.
    match usize::try_from(-aligned.fixed) {
        Ok(carried) => {
            let carried = carried.min(memory.rows.len());
            known.extend(memory.rows[..carried].iter().map(|row| row.hash));
        }
        Err(_) => {
            let shown = usize::try_from(aligned.fixed).unwrap_or(0);
            let taken = shown.min(known.len());
            known.truncate(known.len() - taken);
            if shown > taken {
                unknown_above = unknown_above.map(|unknown| unknown.saturating_sub(shown - taken));
            }
        }
    }
    let mut left = aligned.rotation;
    if let Some(unknown) = &mut unknown_above {
        let gone = left.min(*unknown);
        *unknown -= gone;
        left -= gone;
    }
    known.drain(..left.min(known.len()));
    let over = known.len().saturating_sub(KNOWN_LIMIT);
    known.drain(..over);
    (known, unknown_above.map(|unknown| unknown + over))
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
    let held_above = next.unknown_above.map(|unknown| unknown + next.known.len());
    // Where row `row` of the last read is now, when it survived.
    let surviving = |row: isize| -> Option<isize> {
        let to = match usize::try_from(row) {
            Err(_) if memory.at_document_start => return None,
            Err(_) => row + shift,
            Ok(from) => match fates.get(from)? {
                OldRow::Kept(to) | OldRow::Rewritten(to) => return Some(isize_of(*to)),
                OldRow::Above => row + shift,
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
        (to >= 0 || held_above.is_none_or(|held| to.unsigned_abs() <= held)).then_some(to)
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
    let from = match *kinds.get(line.rows.start)? {
        NewRow::Kept { from, moved: false }
        | NewRow::Grown { from }
        | NewRow::Rewritten { from, moved: false } => from,
        _ => return None,
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

/// The hashes the document's first row may have: a read gives the row
/// without saying whether its line wraps onto the next, so both its text
/// as a line's last row, padding left out, and as a wrapped row.
fn first_row_hashes(text: &str) -> [u64; 2] {
    let text = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text);
    [hash(crate::trim_padding(text)), hash(text)]
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
        let line_hash = hash(crate::trim_padding(line));
        for (at, piece) in pieces.into_iter().enumerate() {
            let piece = if at == last {
                crate::trim_padding(piece)
            } else {
                piece
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
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    // The remembered history's mark for a row that went by unread is no
    // row's.
    if hash == UNREAD { 1 } else { hash }
}

fn isize_of(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

#[cfg(test)]
mod tests;
