//! The memory of a terminal's rows and the alignment of each read against
//! it (`docs/design/terminal-model.md`, section 4, "Changes as alignments
//! of rows"): what each row of a read is (new, kept and from where,
//! rewritten and from which row, a lingering row returned, a reprint),
//! where each row of the last read went, how far the history rotated, and
//! where the anchors (the review position and the command marks) are now.
//! This is classification, not speech: the reading rules of section 5 are
//! built on [`Outcome`].
//!
//! A read gives no coordinates, only text, so a row's position is its
//! index in the read that found it. Reads start where the last read said
//! to start, the top of the screen as last read, so a row that did not
//! move has a known shift between two reads (its index less the rows the
//! last read held above that start). Once the history is full, the
//! buffer rotates under those fixed coordinates, which no provider
//! reports, so each read also takes a few history rows above its start,
//! the block, and the document's first row, and the rotation is the one
//! the known history and those rows agree on (section 4.3, item 5).
//!
//! Once a read is handled, the memory keeps of it only the block and the
//! rows below it, the screen and any rows written since (section 4.1): the
//! rows above the block are held as hashes in the known history. Rows keep
//! their places, counted from the read's first row, so what refers to a
//! row of the last read is unchanged by the trim.

use std::collections::{HashMap, VecDeque};
use std::ops::Range;
use std::sync::Arc;

mod align;

use align::Likeness;

/// How long a gone row lingers (section 5.14), in milliseconds.
pub const LINGERING_MS: u64 = 2000;

/// The most row hashes the known history keeps. It holds only the rows the
/// buffer still holds above the memory's (section 4.1), so the terminal's
/// history bounds it, 9,001 rows by default in both terminals, while every
/// rotation is measured; a rotation measured short, the smallest one
/// consistent with a history of repeated text (section 4.3, item 5),
/// leaves the hashes of rows that already left until a read reaches the
/// document's start, and this limit bounds those.
const KNOWN_LIMIT: usize = 32_768;

/// How many history rows above the kept start the block holds at least
/// (section 4.3, item 5).
const BLOCK: usize = 4;

/// How many of the block's rows have text at least: the block extends
/// upward past blank rows until it holds this many, or a screen's height
/// of rows.
const BLOCK_TEXT: usize = 2;

/// What the known history holds for a row that went by unread (section
/// 4.1): no row's hash, since [`hash`] never gives it.
const UNREAD: u64 = 0;

/// The most candidate rotations one read of rows by their place checks
/// (section 4.3, item 5); with more, the whole document is read instead.
/// Each is a few instructions of the read program and the block's rows.
pub const MOST_CHECKED: usize = 16;

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
    /// ([`TerminalMemory::above`], or [`Outcome::read_above`]): fewer than
    /// asked when the document's start came first.
    pub above: usize,
    /// The rows [`Outcome::check_rows`] asked for, read after this read,
    /// when the same read is given again with them; `None` otherwise.
    pub checked: Option<Checked>,
}

/// A run of history rows read by its place (section 4.3, item 5): its
/// first row is `above` rows above the start the read was asked for, before
/// the rows above that start, and it holds `rows` rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowsAbove {
    /// How many rows above the read's asked start the run begins.
    pub above: usize,
    /// How many rows it holds.
    pub rows: usize,
}

/// The rows [`Outcome::check_rows`] asked for, as the read program reads
/// them (`verbatim_uia_rops::terminal_rows`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    /// Each run's text, in the order asked, line breaks included: empty for
    /// a run whose first row the document's start came before.
    pub runs: Vec<String>,
    /// The document's first row, read after the runs: when it is not the
    /// read's, the buffer moved between the two, and the whole document is
    /// read instead.
    pub first_row: String,
}

/// A row of the memory: one row of the last read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Its text, padding left out (section 5.12): only a line's last row
    /// has padding, so the earlier rows of a wrapped line keep the spaces
    /// at their end. Shared, so the line it makes up and the reading rules'
    /// state of that line hold no copy of it.
    pub text: Arc<str>,
    /// A 64-bit hash of `text`.
    pub hash: u64,
    /// Whether its line continues onto the next row.
    pub wraps: bool,
    /// The hash of the whole line it belongs to, its rows joined.
    pub line_hash: u64,
    /// Whether it has been rewritten in place other than by growth, or
    /// taken back while lingering (section 5.2).
    pub redrawn: bool,
    /// When its text last changed, or it was first read.
    pub changed_ms: u64,
}

impl Row {
    /// Whether the row shows nothing: no text, or only the spaces of a
    /// wrapped line's row.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        crate::trim_padding(&self.text).is_empty()
    }
}

/// A remembered place (sections 4.1, 5.6 and 5.13): the review position
/// or a command mark.
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
        /// Whether it moved other than with the history: down with the
        /// end of the text, or drawn again elsewhere by its program.
        moved: bool,
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
    /// text (section 4.1).
    Returned {
        /// Whether it came back elsewhere than where it was.
        moved: bool,
    },
    /// A row the known history held above the old screen, printed again
    /// after a clear (section 4.4).
    Reprint,
    /// A row above where the memory began, which the reads before never
    /// covered: history, not new output.
    Earlier,
}

/// What became of a row of the last read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OldRow {
    /// Kept, now the read's row at this index.
    Kept(usize),
    /// Rewritten, now the read's row at this index.
    Rewritten(usize),
    /// Gone: erased or written over.
    Gone {
        /// Whether it lingers (section 4.1).
        lingering: bool,
    },
    /// Above the read: it scrolled or rotated past the read's start.
    Above,
    /// Out of the buffer: it rotated past the read's start, which is the
    /// document's start, as the full history rotated.
    Left,
}

/// A switch of screens (section 5.8), or a clear that looks like one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Switch {
    /// No switch.
    #[default]
    None,
    /// The document became one screen tall with no history: the alternate
    /// screen opened, or the history was cleared. The memory before it is
    /// kept aside.
    Opened,
    /// The document grew back and matched the memory kept aside, which is
    /// restored.
    Closed,
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
    /// wrapped onto it included, and did not move (section 5.2,
    /// "Growing"): the length in bytes of what it was, so `text[from..]`
    /// is what it gained.
    pub grew_from: Option<usize>,
}

/// The rows of the last read ([`TerminalMemory::rows`]), counted from its
/// first row: those from [`Rows::first_kept`] on are kept, and those above
/// it are held only as hashes in the known history (section 4.1).
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
    /// No row of the last screen is in this read (or the read's start was
    /// refused), so the caller reads the whole document and gives that
    /// read instead (section 4.2). Nothing else is set, and the memory is
    /// unchanged.
    pub whole_document: bool,
    /// The history may have rotated by more than one candidate rotation
    /// that the read cannot tell apart, the block of some of them lying
    /// above the read, so the caller reads these runs of rows by their place
    /// and gives the same read again with them in [`Read::checked`]
    /// (section 4.3, item 5). Empty otherwise; when not, nothing else is
    /// set, and the memory is unchanged.
    pub check_rows: Vec<RowsAbove>,
    /// The history rotated so far that its block and the old screen's top
    /// are above the read, and the rows between them and the read's start
    /// went by unread, so the caller reads again from the same start with
    /// this many rows above it, and gives that read instead (section 4.3,
    /// item 5). Nothing else is set, and the memory is unchanged.
    pub read_above: Option<usize>,
    /// The memory's first read: every row is new, and nothing was heard
    /// before it.
    pub first: bool,
    /// What each row of the read is.
    pub rows: Vec<NewRow>,
    /// What became of each row of the last read.
    pub old: Vec<OldRow>,
    /// The read's lines.
    pub lines: Vec<Line>,
    /// How many rows the history rotated: the one rotation the known
    /// history, the document's first row and the block agree on, or the
    /// topmost kept row's when the block left the buffer. A whole-document
    /// read counts it in the rows' shift instead, and gives 0.
    pub rotation: usize,
    /// More went by than the history holds (section 4.3, item 5): a
    /// whole-document read did not find the last screen in a document
    /// longer than a screen, or a read reaching the document's start did
    /// not find the block.
    pub overflowed: bool,
    /// A switch of screens.
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
/// rows, anchors, lingering rows and the caret are counted as the last
/// read's rows are.
#[derive(Clone, Debug, Default)]
struct Screen {
    rows: Vec<Row>,
    /// How many of the last read's rows lie above `rows`, trimmed once the
    /// read was handled (section 4.1): their hashes are the newest of
    /// `known`.
    skipped: usize,
    /// The screen's height at the last read.
    height: usize,
    /// Whether the last read started at the document's start.
    at_document_start: bool,
    caret: Option<usize>,
    lingering: Vec<Lingering>,
    /// The hashes of the rows above `rows`, oldest first: the known history.
    known: VecDeque<u64>,
    /// How many rows the buffer holds above the known history, when a read
    /// from the document's start told; `None` when no read since the first
    /// has started there.
    unknown_above: Option<usize>,
    review: Option<Anchor>,
    marks: Vec<Anchor>,
    /// The places the caller keeps, each by its number
    /// ([`TerminalMemory::add_place`]).
    places: Vec<(u64, Anchor)>,
    /// Whether the first row continues a line that starts above it.
    continued: bool,
    /// How many rows directly above the screen's top form the block, whose
    /// place the next read checks (section 4.3, item 5).
    block: usize,
    /// How many rows the next read takes above its start: the block, and
    /// more while the block is still to be extended.
    above: usize,
    /// For each of `rows`, whether its text was read since the switch that
    /// began this memory: what a restore of the memory kept aside checks
    /// (section 5.8).
    read_since: Vec<bool>,
}

impl Screen {
    /// The rows from the next read's start: the screen as last read, an
    /// index of `rows`.
    fn screen_start(&self) -> usize {
        self.rows.len().saturating_sub(self.height)
    }

    /// The index in `rows` of the last read's row `row`, when it is kept.
    fn index(&self, row: usize) -> Option<usize> {
        row.checked_sub(self.skipped)
            .filter(|&index| index < self.rows.len())
    }

    /// The last read's row `row`, when it is kept.
    fn row(&self, row: usize) -> Option<&Row> {
        self.index(row).map(|index| &self.rows[index])
    }

    /// The index in `rows` of the first row kept once the read is trimmed:
    /// the block's first row.
    fn trim_point(&self) -> usize {
        self.screen_start().saturating_sub(self.block)
    }

    /// Keeps only the block and the rows below it, the hashes of those
    /// above going to the known history (section 4.1). Returns how many
    /// rows it dropped.
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
        self.read_since = self
            .read_since
            .split_off(dropped.min(self.read_since.len()));
        self.skipped += dropped;
        dropped
    }

    fn has_text(rows: &[Row]) -> bool {
        rows.iter().any(|row| !row.is_blank())
    }

    /// Takes as the block the rows directly above the screen's top: at
    /// least [`BLOCK`], extended upward past blank rows to take in
    /// [`BLOCK_TEXT`] with text, at most a screen's height, and as many as
    /// the read holds; and asks the next read for more rows above its start
    /// while the block is still short of that and the history goes on
    /// above the read.
    fn take_block(&mut self, at_document_start: bool) {
        let top = self.screen_start();
        let limit = self.height.max(BLOCK);
        let (mut count, mut texts) = (0, 0);
        while count < top && count < limit && (count < BLOCK || texts < BLOCK_TEXT) {
            count += 1;
            if !self.rows[top - count].is_blank() {
                texts += 1;
            }
        }
        let complete = (count >= BLOCK && texts >= BLOCK_TEXT)
            || count >= limit
            || (count == top && at_document_start);
        self.block = count;
        self.above = if complete {
            count
        } else {
            (count + BLOCK).min(limit)
        };
    }
}

/// Why the memory asked for the whole document, or, for a read that
/// reached it, why it was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Whole {
    /// The read's start was refused: the text shrank, as after a clear.
    Shrank,
    /// No row of the last screen was in the read.
    Lost,
    /// No candidate rotation holds, so the history rotated past the block:
    /// an old row kept, its index in the read less its index in the
    /// memory, moved down no further than this; one that did was drawn
    /// again or repeats text.
    Unfound(isize),
    /// The rotation could not be told without the whole document: several
    /// candidates hold, the known history does not reach the document's
    /// start, or a candidate is a row that went by unread (section 4.3,
    /// item 5). The whole document is then aligned as when no row of the
    /// last screen was in the read.
    Unsure,
}

/// What the memory asked for, until the read comes.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    /// The whole document.
    Whole(Whole),
    /// These runs of rows by their place, with the same read again.
    Rows(Vec<RowsAbove>),
    /// A read from further above the same start.
    Above,
}

/// The memory of one terminal (section 4.1), kept while the terminal
/// exists, across focus changes.
#[derive(Clone, Debug, Default)]
pub struct TerminalMemory {
    screen: Option<Screen>,
    /// Whether the last read's screen started at the document's start.
    alternate: bool,
    /// The main screen's memory, kept aside while the alternate screen is
    /// up.
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

    /// How many rows the next read takes above its start (section 4.3,
    /// item 5), which the read program is asked for. A first read takes
    /// [`BLOCK`] rows above the screen, so that the block is known at once.
    #[must_use]
    pub fn above(&self) -> usize {
        self.screen.as_ref().map_or(BLOCK, |screen| screen.above)
    }

    /// The rows of the last read, counted from its first row, of which
    /// the memory keeps those from the block on once the read is trimmed
    /// ([`TerminalMemory::trim`]).
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
    /// read is trimmed: the first row of the block above the screen.
    #[must_use]
    pub fn first_kept(&self) -> usize {
        self.screen
            .as_ref()
            .map_or(0, |screen| screen.skipped + screen.trim_point())
    }

    /// Keeps of the last read only what section 4.1 needs: the screen as
    /// last read, the rows written below it, and the block above it, the
    /// rows above the block held as hashes in the known history. The
    /// reading rules call it once they have handled a read, and every
    /// read first trims what the last left; the rows keep their places, so
    /// nothing counted from the last read's first row moves. Returns how
    /// many rows it dropped.
    pub fn trim(&mut self) -> usize {
        self.screen.as_mut().map_or(0, Screen::trim)
    }

    /// The hashes of the rows known above the rows the memory keeps, oldest
    /// first.
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
    /// caller read (section 5.6). Does nothing before the first read.
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
    /// position does (section 5.6), is kept aside with the memory while the
    /// alternate screen is up, and is dropped when its row cannot be
    /// followed, as when the history overflowed and its row was not kept.
    /// Before the first read it is only numbered.
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
    /// read, unless [`Outcome::whole_document`] or [`Outcome::check_rows`]
    /// asks for another read first. What the last read left untrimmed is
    /// trimmed first ([`TerminalMemory::trim`]).
    pub fn read(&mut self, read: &Read) -> Outcome {
        self.read_with(read, false)
    }

    /// Takes `read`, for which [`TerminalMemory::read`] asked for the whole
    /// document or rows by their place only to tell the rotation, without
    /// that read, as when "report new output" is off and nothing is read
    /// that is not needed (section 4.2): of the candidates the read cannot
    /// rule out, the smallest rotation, or with an incomplete known history
    /// none when the block is in place. `None` when the read was asked for
    /// another reason, such as the last screen not being in the read, which
    /// [`TerminalMemory::restart`] handles.
    pub fn settle(&mut self, read: &Read) -> Option<Outcome> {
        match self.pending {
            Some(Pending::Whole(Whole::Unsure) | Pending::Rows(_) | Pending::Above) => {
                self.pending = None;
                Some(self.read_with(read, true))
            }
            _ => None,
        }
    }

    /// [`TerminalMemory::read`], deciding the rotation without another
    /// read when `settling` ([`TerminalMemory::settle`]).
    #[expect(
        clippy::too_many_lines,
        reason = "one read's steps in order: switches, the rotation, the whole document, the alignment, the anchors"
    )]
    fn read_with(&mut self, read: &Read, settling: bool) -> Outcome {
        self.trim();
        let last_height = self.screen.as_ref().map(|screen| screen.height);
        let (width, height) = measure(&read.screen, &read.caret_line, last_height);
        let rows = split_rows(&read.text, width, read.at_ms);
        let caret = caret_row(rows.len(), read.caret_rows_to_end);
        let pending = self.pending.take();
        let whole = match pending {
            // A whole document read to tell the rotation, asked for or instead
            // of a read the memory asked for, is aligned as one
            // read because the last screen was not in the read.
            Some(Pending::Whole(Whole::Unsure) | Pending::Rows(_) | Pending::Above)
                if read.at_document_start && !read.from_start =>
            {
                Some(Whole::Lost)
            }
            Some(Pending::Whole(cause)) if read.at_document_start && !read.from_start => {
                Some(cause)
            }
            // A refused start, the read taken from as far above where the
            // next read starts as the document's start: the whole document,
            // read at once.
            _ if read.shrank && read.at_document_start && !read.alternate => Some(Whole::Shrank),
            _ => None,
        };
        let checked = match &pending {
            Some(Pending::Rows(asked)) if read.from_start => read
                .checked
                .as_ref()
                .filter(|checked| checked.runs.len() == asked.len())
                .map(|checked| (asked.as_slice(), checked)),
            _ => None,
        };
        let shape = Outcome {
            caret,
            width,
            height,
            ..Outcome::default()
        };
        let Some(current) = self.screen.take() else {
            self.alternate = read.alternate;
            let mut screen = Screen {
                read_since: vec![false; rows.len()],
                rows,
                height,
                at_document_start: read.at_document_start,
                caret,
                unknown_above: read.at_document_start.then_some(0),
                ..Screen::default()
            };
            screen.take_block(read.at_document_start);
            let outcome = Outcome {
                first: true,
                rows: vec![NewRow::New; screen.rows.len()],
                lines: lines(&screen.rows),
                ..shape
            };
            self.screen = Some(screen);
            return outcome;
        };
        let opened = read.alternate && !self.alternate;
        let asked = Asked {
            whole,
            checked,
            settling,
        };
        let own = align(&current, &rows, read, (width, height), caret, asked);
        // A memory kept aside is not restored when the read shows, where
        // they were, rows read since the switch: the screen never changed,
        // as when the history was erased (section 5.8).
        let shows_read_since = match &own {
            Aligned::Done(classified) => classified.new.iter().any(|kind| {
                matches!(kind, NewRow::Kept { from, moved: false }
                if current.index(*from).is_some_and(|index| {
                    current.read_since[index] && !current.rows[index].is_blank()
                }))
            }),
            Aligned::Whole(_) | Aligned::Check(_) | Aligned::Above(_) => false,
        };
        let restored = self
            .aside
            .as_ref()
            .filter(|_| !read.alternate && !shows_read_since)
            .map(|aside| {
                // The read started where the alternate screen's memory said,
                // so where the kept memory's rows would be is not known.
                let unanchored = Read {
                    from_start: false,
                    ..read.clone()
                };
                let how = How {
                    base: None,
                    rotation: None,
                    whole,
                    restoring: true,
                };
                classify(aside, &rows, &unanchored, height, caret, how)
            })
            .filter(|attempt| attempt.restores);
        let switch = if restored.is_some() {
            Switch::Closed
        } else if opened {
            Switch::Opened
        } else {
            Switch::None
        };
        let classified = match (restored, own) {
            (Some(attempt), _) => attempt,
            (None, Aligned::Done(classified)) => classified,
            (None, Aligned::Whole(cause)) => {
                // The memory is unchanged until the whole document is read.
                self.pending = Some(Pending::Whole(cause));
                self.screen = Some(current);
                return Outcome {
                    whole_document: true,
                    ..shape
                };
            }
            (None, Aligned::Above(above)) => {
                // The memory is unchanged until the read is taken again.
                self.pending = Some(Pending::Above);
                self.screen = Some(current);
                return Outcome {
                    read_above: Some(above),
                    ..shape
                };
            }
            (None, Aligned::Check(runs)) => {
                // The memory is unchanged until the rows are read.
                self.pending = Some(Pending::Rows(runs.clone()));
                self.screen = Some(current);
                return Outcome {
                    check_rows: runs,
                    ..shape
                };
            }
        };
        if whole.is_none() {
            let cause = if read.shrank && !read.at_document_start {
                Some(Whole::Shrank)
            } else if classified.old_had_text && !classified.kept_text && !read.at_document_start {
                Some(Whole::Lost)
            } else {
                None
            };
            if let Some(cause) = cause {
                // The memory is unchanged until the whole document is read.
                self.pending = Some(Pending::Whole(cause));
                self.screen = Some(current);
                return Outcome {
                    whole_document: true,
                    ..shape
                };
            }
        }
        // A memory kept aside is restored when the read matches it, and is
        // replaced at the next switch when the main screen came back
        // without it, as after a clear (section 5.8).
        // The main screen back without the memory kept aside shows other
        // rows than the alternate screen did: none of its rows is one of
        // the alternate screen's rewritten.
        let unrestored = switch == Switch::None && !read.alternate && self.aside.is_some();
        let mut reference = if switch == Switch::Closed {
            self.aside.take().unwrap_or(current)
        } else {
            if !read.alternate {
                self.aside = None;
            }
            current
        };
        let Classified {
            new: mut kinds,
            old: mut fates,
            rotation,
            shift,
            overflowed,
            ..
        } = classified;
        if unrestored {
            for (to, kind) in kinds.iter_mut().enumerate() {
                if let NewRow::Rewritten { from, .. } = *kind {
                    *kind = NewRow::New;
                    if fates.get(from) == Some(&OldRow::Rewritten(to)) {
                        fates[from] = OldRow::Gone { lingering: false };
                    }
                }
            }
        }
        // The read's first row continues a line from above it when the old
        // row it was did (section 4.3, a row at a read's top).
        // A row the memory held only as a hash, found again by a read from
        // the document's start, is taken to start its line.
        let continued = match kinds.first() {
            Some(NewRow::Kept { from, .. } | NewRow::Rewritten { from, .. }) => {
                match reference.index(*from) {
                    Some(0) => reference.continued,
                    Some(index) => reference.rows[index - 1].wraps,
                    None => false,
                }
            }
            _ => false,
        };
        let lines = lines(&rows);
        let lines = lines
            .into_iter()
            .map(|mut line| {
                line.grew_from = grew_from(&line, &kinds, &reference, continued);
                line
            })
            .collect();
        // The known history moves on with the memory, except to a new
        // screen, whose memory starts without it.
        let known = if opened {
            VecDeque::new()
        } else {
            std::mem::take(&mut reference.known)
        };
        let mut next = remember(
            &reference, known, rows, &kinds, &fates, shift, rotation, read, height, caret,
        );
        next.continued = continued;
        if overflowed {
            // Where the history went is not known: each anchor is checked
            // against its line where the alignment put it, and dropped if
            // its line is gone (section 4.3, item 5), the review position
            // then going to the caret.
            next.review = reference
                .review
                .and_then(|anchor| {
                    let from = usize::try_from(anchor.row).ok()?;
                    match fates.get(from)? {
                        OldRow::Kept(to) => Some(anchor_at(&next.rows, *to, anchor.column)),
                        _ => None,
                    }
                })
                .or_else(|| caret.map(|row| anchor_at(&next.rows, row, read.caret_column)));
            next.marks = reference
                .marks
                .iter()
                .filter(|mark| mark.row >= 0)
                .filter_map(|&mark| map_mark(mark, &fates, shift, &next.rows))
                .collect();
            // A place stays only on a row that was kept.
            next.places = reference
                .places
                .iter()
                .filter_map(|&(number, anchor)| {
                    let from = usize::try_from(anchor.row).ok()?;
                    match fates.get(from)? {
                        OldRow::Kept(to) => {
                            Some((number, anchor_at(&next.rows, *to, anchor.column)))
                        }
                        _ => None,
                    }
                })
                .collect();
        } else {
            next.review = reference.review.and_then(|anchor| {
                map_anchor(
                    anchor, &reference, &kinds, &fates, shift, &next, caret, read,
                )
            });
            next.marks = reference
                .marks
                .iter()
                .filter_map(|&mark| map_mark(mark, &fates, shift, &next.rows))
                .collect();
            next.places = reference
                .places
                .iter()
                .filter_map(|&(number, anchor)| {
                    map_anchor(
                        anchor, &reference, &kinds, &fates, shift, &next, caret, read,
                    )
                    .map(|anchor| (number, anchor))
                })
                .collect();
        }
        if opened {
            // The memory before the switch is kept aside with its anchors;
            // on the new screen the review position is at the caret, and
            // every row not kept from before the switch was read since it.
            self.aside = Some(reference);
            next.review = caret.map(|row| anchor_at(&next.rows, row, read.caret_column));
            next.marks.clear();
            next.places.clear();
            next.known.clear();
            for (flag, kind) in next.read_since.iter_mut().zip(&kinds) {
                *flag = !matches!(kind, NewRow::Kept { .. });
            }
        }
        next.take_block(read.at_document_start);
        self.alternate = read.alternate;
        self.screen = Some(next);
        Outcome {
            rows: kinds,
            old: fates,
            lines,
            rotation,
            overflowed,
            switch,
            continued,
            ..shape
        }
    }

    /// Takes `read` as a first read, as when the whole document or a search
    /// for the block was asked for and the caller will not read it, with
    /// "report new output" off (section 4.2). The command marks and the
    /// review position are kept: each moved as the read's start moved,
    /// with no rotation, and kept when its line is there, or when it is
    /// above the read, where it cannot be checked; a review position whose
    /// line is not there goes to the caret. When where the memory's rows are
    /// in the read is not known, as when the read's start was refused, the
    /// marks are dropped and the review position goes to the caret.
    pub fn restart(&mut self, read: &Read) -> Outcome {
        // Each anchor moved as the read's start moved, or none when where
        // the memory's rows are in the read is not known.
        type Moved = (Vec<Anchor>, Option<Option<Anchor>>, Vec<(u64, Anchor)>);
        let (marks, review, places): Moved = self.screen.as_ref().map_or_default(|screen| {
            let base = base_of(screen, read).map(|base| base - isize_of(screen.skipped));
            let moved = |anchor: &Anchor| {
                base.map(|base| Anchor {
                    row: anchor.row + base,
                    ..*anchor
                })
            };
            (
                screen.marks.iter().filter_map(moved).collect(),
                screen.review.as_ref().map(moved),
                screen
                    .places
                    .iter()
                    .filter_map(|(number, anchor)| Some((*number, moved(anchor)?)))
                    .collect(),
            )
        });
        let next_place = self.next_place;
        *self = Self::new();
        self.next_place = next_place;
        let outcome = self.read(read);
        if let Some(screen) = &mut self.screen {
            let rows = &screen.rows;
            // The review position stays on its row when the read shows its
            // line there, or above the read when the read does not start at
            // the document's start, where it cannot be checked; otherwise it
            // goes to the caret (sections 4.3, item 5, and 5.6).
            let caret = screen.caret;
            screen.review = review.and_then(|anchor| {
                anchor
                    .and_then(|anchor| match usize::try_from(anchor.row) {
                        Ok(row) => rows
                            .get(row)
                            .filter(|found| found.line_hash == anchor.line_hash)
                            .map(|_| anchor_at(rows, row, anchor.column)),
                        Err(_) => (!read.at_document_start).then_some(anchor),
                    })
                    .or_else(|| caret.map(|row| anchor_at(rows, row, read.caret_column)))
            });
            // A place stays where the read shows its line, or above the
            // read, as the review position does; otherwise it is dropped.
            screen.places = places
                .into_iter()
                .filter(|(_, anchor)| match usize::try_from(anchor.row) {
                    Ok(row) => rows
                        .get(row)
                        .is_some_and(|found| found.line_hash == anchor.line_hash),
                    Err(_) => !read.at_document_start,
                })
                .collect();
            screen.marks = marks
                .into_iter()
                .filter(|mark| match usize::try_from(mark.row) {
                    Ok(row) => {
                        row < rows.len()
                            && line_start(rows, row) == row
                            && rows[row].line_hash == mark.line_hash
                    }
                    Err(_) => true,
                })
                .collect();
        }
        outcome
    }
}

/// What aligning a read with one screen's memory gave: the classification,
/// or another read to take first, of the whole document or of runs of rows
/// by their place.
enum Aligned {
    Done(Classified),
    Whole(Whole),
    Check(Vec<RowsAbove>),
    Above(usize),
}

/// What a read was asked to be, and how the rotation may be decided.
#[derive(Clone, Copy)]
struct Asked<'a> {
    /// Why the read is of the whole document, or reached its start.
    whole: Option<Whole>,
    /// The runs of rows asked by their place and their texts, when the read
    /// is given again with them.
    checked: Option<(&'a [RowsAbove], &'a Checked)>,
    /// Whether the rotation is decided without another read
    /// ([`TerminalMemory::settle`]).
    settling: bool,
}

/// Where a row of `memory` that did not move is in `read`, its index there
/// less its index in the memory, when the read started where the memory's
/// last read said to start.
fn base_of(memory: &Screen, read: &Read) -> Option<isize> {
    if !read.from_start {
        return None;
    }
    let count = memory.rows.len();
    let above = isize_of(read.above);
    if count >= memory.height || memory.at_document_start {
        Some(above - isize_of(memory.screen_start()))
    } else if read.at_document_start && read.above == 0 {
        // The start may have stopped at the document's start, above where
        // the memory's rows begin by a number of rows not known.
        None
    } else {
        Some(above + isize_of(memory.height - count))
    }
}

/// Whether the rows of `rows` from `at` hold `block`, a run of hashes; in
/// a read from the document's start, the rows above which rotated out of
/// the buffer, so `at` is negative, its rows that are left.
fn block_at(block: &[u64], rows: &[Row], at: isize, at_document_start: bool) -> bool {
    let cut = usize::try_from(-at).unwrap_or(0);
    if cut >= block.len() || (cut > 0 && !at_document_start) {
        return false;
    }
    let from = usize::try_from(at).unwrap_or(0);
    rows.get(from..from + block.len() - cut)
        .is_some_and(|found| {
            found
                .iter()
                .zip(&block[cut..])
                .all(|(row, hash)| row.hash == *hash)
        })
}

/// Where the memory's rows and its known history are in the buffer: the
/// coordinate of the known history's first row, negative when it holds rows
/// the buffer no longer does, and of the memory's first row.
#[derive(Clone, Copy)]
struct Places {
    first_known: isize,
    first_row: isize,
}

impl Places {
    /// The places when the known history reaches the document's start, as
    /// the last read left them.
    fn of(memory: &Screen) -> Option<Self> {
        memory.unknown_above.map(|unknown| Self {
            first_known: isize_of(unknown),
            first_row: isize_of(unknown + memory.known.len()),
        })
    }

    /// The places a read from the document's start shows: the memory's
    /// first row is `base` rows below it, which a rotation does not change,
    /// since the buffer's coordinates stay as it rotates.
    fn from_document_start(memory: &Screen, base: isize) -> Self {
        Self {
            first_known: base - isize_of(memory.known.len()),
            first_row: base,
        }
    }

    /// The hash of the row the history held at coordinate `coordinate` at
    /// the last read, for a coordinate above the last screen: `None` for a
    /// row that went by unread, or that the known history does not reach.
    fn history(self, memory: &Screen, coordinate: isize) -> Option<u64> {
        if coordinate < self.first_row {
            let at = usize::try_from(coordinate - self.first_known).ok()?;
            memory.known.get(at).copied().filter(|&hash| hash != UNREAD)
        } else {
            let at = usize::try_from(coordinate - self.first_row).ok()?;
            memory.rows.get(at).map(|row| row.hash)
        }
    }
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

/// The hashes of the rows of a run read by its place, split at `width`: a
/// run that does not end with a line break ends in a row its line wraps
/// from, whose padding is its own.
fn run_hashes(text: &str, width: usize) -> Vec<u64> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut rows: Vec<u64> = split_rows(text, width, 0)
        .iter()
        .map(|row| row.hash)
        .collect();
    if !text.ends_with("\r\n")
        && let Some(last) = rows.last_mut()
        && let Some(piece) = text
            .rsplit("\r\n")
            .next()
            .and_then(|line| row_pieces(line, width).last().copied())
    {
        *last = hash(piece);
    }
    rows
}

/// The smallest rotation for which every row the known history held above
/// the last screen is where a whole document read, `rows`, shows it (section
/// 4.3, item 5): the comparison of the whole buffer with the known history,
/// a row that went by unread agreeing with any. Each candidate is compared
/// from the rows nearest the old screen, which output that repeats earlier
/// output rarely repeats, so a wrong one is refuted at once.
fn whole_rotation(memory: &Screen, rows: &[Row], places: Places) -> Option<usize> {
    let span = places.first_row + isize_of(memory.screen_start());
    let first = rows.first()?.hash;
    (0..span.max(0)).find_map(|rotation| {
        let candidate = places
            .history(memory, rotation)
            .is_none_or(|hash| hash == first);
        let holds = candidate
            && (rotation..span).rev().all(|coordinate| {
                usize::try_from(coordinate - rotation)
                    .ok()
                    .and_then(|at| rows.get(at))
                    .is_some_and(|row| {
                        places
                            .history(memory, coordinate)
                            .is_none_or(|hash| hash == row.hash)
                    })
            });
        holds.then(|| usize::try_from(rotation).unwrap_or(0))
    })
}

/// A rotation the read does not rule out (section 4.3, item 5), with where
/// it puts the block: the read's index of the block's first row the buffer
/// still holds, negative above the read, how many of its rows the rotation
/// took out of the buffer, and the read's index of the row below it, the
/// old screen's first.
#[derive(Clone, Copy)]
struct Candidate {
    rotation: usize,
    first: isize,
    cut: usize,
    end: isize,
}

impl Candidate {
    /// The run of rows above `read` that holds the block's rows there.
    fn run(self, read: &Read) -> RowsAbove {
        RowsAbove {
            above: read.above + self.first.unsigned_abs(),
            rows: (self.end.min(0) - self.first).unsigned_abs(),
        }
    }

    /// When the old screen's first row is above `read` too, the rows between
    /// the old screen's new place and the read's start went by unread, so
    /// the read is taken again from the block's place, which shows them and
    /// the block: how many rows above the read's asked start.
    fn further(self, read: &Read) -> Option<usize> {
        (self.end < 0).then(|| self.run(read).above)
    }
}

/// Aligns the rows of `read` with `memory`, finding how far the history
/// rotated (section 4.3, item 5) exactly as comparing the whole buffer with
/// the known history would: the candidates are the rotations for which the
/// known history's row has the document's first row's text; each predicts
/// every history row, which the read checks where it holds them, and the
/// block's place, which runs of rows read by their place check when it lies
/// above the read. One candidate holding is the rotation; several, a
/// known history that does not reach the document's start, or a candidate
/// on a row that went by unread are told by the whole document; none means
/// the history rotated past the block, which the topmost kept row measures
/// when it can be trusted, and otherwise the whole document is read.
#[expect(
    clippy::too_many_lines,
    reason = "the rotation's cases in the design's order, each short"
)]
fn align(
    memory: &Screen,
    rows: &[Row],
    read: &Read,
    (width, height): (usize, usize),
    caret: Option<usize>,
    asked: Asked<'_>,
) -> Aligned {
    let Asked {
        whole,
        checked,
        settling,
    } = asked;
    let base = base_of(memory, read);
    let how = |rotation| How {
        base,
        rotation,
        whole,
        restoring: false,
    };
    let block = memory.block;
    let top = memory.screen_start();
    let hashes: Vec<u64> = memory.rows[top - block..top]
        .iter()
        .map(|row| row.hash)
        .collect();
    if whole == Some(Whole::Lost) && block > 0 {
        // The whole document, read because no row of the last screen was in
        // the read or the rotation could not be told otherwise: the rotation
        // is the smallest that the whole known history agrees with, and the
        // block anchors the old rows. Without a complete known history, the
        // block where it is lowest does; no row of the history moves down,
        // so when the last read began at the document's start, the block is
        // no lower than it was.
        let found = if let Some(places) = Places::of(memory) {
            whole_rotation(memory, rows, places)
                .map(|rotation| places.first_row + isize_of(top - block) - isize_of(rotation))
        } else {
            let lowest = if memory.at_document_start {
                isize_of(memory.skipped + top - block)
            } else {
                isize_of(rows.len()) - 1
            };
            (1 - isize_of(block)..=lowest)
                .rev()
                .find(|&at| block_at(&hashes, rows, at, true))
        };
        if let Some(at) = found {
            let how = How {
                base: Some(at + isize_of(block) - isize_of(top)),
                rotation: Some(0),
                whole,
                restoring: false,
            };
            return Aligned::Done(classify(memory, rows, read, height, caret, how));
        }
    }
    if whole == Some(Whole::Shrank)
        && block > 0
        && let Some(unknown) = memory.unknown_above
    {
        // The whole document, read because it shrank: when the block is
        // still where it was, counted from the document's start, the history
        // was not cleared, only rows of the screen erased, and the old rows
        // above the screen are where they were, not a reprint (section 4.4).
        let first = isize_of(unknown + memory.known.len());
        if block_at(&hashes, rows, first + isize_of(top - block), true) {
            let how = How {
                base: Some(first),
                rotation: Some(0),
                whole,
                restoring: false,
            };
            return Aligned::Done(classify(memory, rows, read, height, caret, how));
        }
    }
    let Some(base) = base.filter(|_| block > 0 && whole.is_none() && !read.alternate) else {
        return Aligned::Done(classify(memory, rows, read, height, caret, how(None)));
    };
    let places = if read.at_document_start {
        Some(Places::from_document_start(memory, base))
    } else {
        Places::of(memory)
    };
    let Some(places) = places else {
        // The known history does not reach the document's start, so which
        // rotations it allows is not known: the whole document is read once,
        // which completes it. Without that read, the block in place means no
        // rotation, and otherwise the kept rows tell.
        if !settling {
            return Aligned::Whole(Whole::Unsure);
        }
        let rotation = block_at(&hashes, rows, base + isize_of(top - block), false).then_some(0);
        return Aligned::Done(classify(memory, rows, read, height, caret, how(rotation)));
    };
    let span = places.first_row + isize_of(top);
    let first = if read.at_document_start {
        rows.first().map_or([0; 2], |row| [row.hash; 2])
    } else {
        first_row_hashes(&read.first_row)
    };
    let total = isize_of(rows.len());
    // The read's index of the document's first row: the read's first row is
    // `places.first_row - base` rows below it.
    let document_start = base - places.first_row;
    let mut candidates: Vec<Candidate> = Vec::new();
    for rotation in 0..span.max(0) {
        match places.history(memory, rotation) {
            Some(hash) if first.contains(&hash) => {}
            Some(_) => continue,
            None if settling => continue,
            None => return Aligned::Whole(Whole::Unsure),
        }
        // The read's row at `to` holds the memory's row `to - shift`: every
        // one above the last screen is compared.
        let shift = base - rotation;
        let holds = (0..(isize_of(top) + shift).clamp(0, total)).all(|to| {
            places
                .history(memory, places.first_row + to - shift)
                .is_none_or(|hash| Some(hash) == usize::try_from(to).ok().map(|to| rows[to].hash))
        });
        if holds {
            // The block's rows the rotation took out of the buffer are cut.
            let end = isize_of(top) + shift;
            let lowest = end - isize_of(block);
            let first = lowest.max(document_start);
            candidates.push(Candidate {
                rotation: usize::try_from(rotation).unwrap_or(0),
                first,
                cut: (first - lowest).unsigned_abs(),
                end,
            });
        }
    }
    // A candidate whose block is in the read, as far as the buffer holds
    // it, is decided; the others' blocks are read by their place.
    let decided = candidates
        .iter()
        .filter(|candidate| candidate.first >= 0)
        .count();
    if decided > 1 && !settling {
        return Aligned::Whole(Whole::Unsure);
    }
    let unshown: Vec<Candidate> = candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.first < 0)
        .collect();
    if !unshown.is_empty() && !settling {
        if let [candidate] = candidates.as_slice()
            && let Some(above) = candidate.further(read)
        {
            return Aligned::Above(above);
        }
        let runs: Vec<RowsAbove> = unshown
            .iter()
            .map(|candidate| candidate.run(read))
            .collect();
        if let Some((_, texts)) = checked.filter(|(asked, _)| *asked == runs.as_slice()) {
            // The buffer moved between the read and the rows read after it
            // when its first row is not the read's.
            if texts.first_row != read.first_row {
                return Aligned::Whole(Whole::Unsure);
            }
            let refuted: Vec<usize> = unshown
                .iter()
                .zip(&runs)
                .zip(&texts.runs)
                .filter(|((candidate, run), text)| {
                    run_hashes(text, width) != hashes[candidate.cut..candidate.cut + run.rows]
                })
                .map(|((candidate, _), _)| candidate.rotation)
                .collect();
            candidates.retain(|candidate| !refuted.contains(&candidate.rotation));
        } else {
            // No row of the last screen kept means the whole document is
            // read anyway (section 4.2).
            let free = classify(memory, rows, read, height, caret, how(None));
            if free.old_had_text && !free.kept_text && !read.at_document_start {
                return Aligned::Done(free);
            }
            if runs.len() > MOST_CHECKED {
                return Aligned::Whole(Whole::Unsure);
            }
            return Aligned::Check(runs);
        }
    }
    match candidates.as_slice() {
        [candidate, rest @ ..] if rest.is_empty() || settling => {
            if !settling && let Some(above) = candidate.further(read) {
                return Aligned::Above(above);
            }
            Aligned::Done(classify(
                memory,
                rows,
                read,
                height,
                caret,
                how(Some(candidate.rotation)),
            ))
        }
        [_, ..] => Aligned::Whole(Whole::Unsure),
        [] => {
            // No candidate holds, so the history rotated past the block, by
            // at least `span` rows: the topmost kept row tells how far when
            // its text is unique and it moved that far.
            let free = classify(memory, rows, read, height, caret, how(None));
            let lost = free.old_had_text && !free.kept_text && !read.at_document_start;
            if lost || settling || free.trusted && isize_of(free.rotation) >= span {
                return Aligned::Done(free);
            }
            // The block is not even partly in the buffer, so the old
            // screen's first row, just below it, is now above the read's
            // second row.
            let unfound = Whole::Unfound(-isize_of(top));
            if !read.at_document_start {
                return Aligned::Whole(unfound);
            }
            let how = How {
                base: None,
                rotation: None,
                whole: Some(unfound),
                restoring: false,
            };
            Aligned::Done(classify(memory, rows, read, height, caret, how))
        }
    }
}

/// A kept pair: an old row's index and the new row's it became.
type Pair = (usize, usize);

/// What the alignment made of a read against one screen's memory.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent findings of one alignment"
)]
struct Classified {
    new: Vec<NewRow>,
    old: Vec<OldRow>,
    rotation: usize,
    /// The shift of a row that did not move: new index less old index.
    shift: isize,
    /// Whether the old screen had a row with text.
    old_had_text: bool,
    /// Whether a row with text of the old screen was kept.
    kept_text: bool,
    /// Whether every row with text of the old screen was kept, so a memory
    /// kept aside matches the read.
    restores: bool,
    overflowed: bool,
    /// Whether the rotation was measured from the topmost kept row and
    /// that row is trustworthy: it moved up, and its text is on no other
    /// row of the old screen or of the read.
    trusted: bool,
}

/// How a read is aligned with a memory.
#[derive(Clone, Copy)]
struct How {
    /// Where a row that did not move is in the read: its index there less
    /// its index in the memory; `None` when not known.
    base: Option<isize>,
    /// How far the history rotated, when the block showed it.
    rotation: Option<usize>,
    /// Why the read is of the whole document, or reached its start.
    whole: Option<Whole>,
    /// Whether the memory is one kept aside, matched against the read.
    restoring: bool,
}

/// Aligns the rows of `read` with `memory` (section 4.3). The alignment
/// counts old rows as indices of the memory's rows; what it returns counts
/// them as the last read's rows.
#[expect(
    clippy::too_many_lines,
    reason = "the alignment's steps in the design's order, each short"
)]
fn classify(
    memory: &Screen,
    rows: &[Row],
    read: &Read,
    height: usize,
    caret: Option<usize>,
    how: How,
) -> Classified {
    let How {
        base,
        whole,
        restoring,
        ..
    } = how;
    let old = &memory.rows;
    let skip = memory.skipped;
    let (count, total) = (old.len(), rows.len());
    // The old rows aligned: the old screen; when the read started
    // elsewhere than the last read said, as after a refused start, rows
    // above it may be in the read too, so a screen more, except in a
    // whole-document read, which is searched for the old screen.
    let start = if base.is_none() && whole.is_none() {
        count.saturating_sub(memory.height * 2)
    } else {
        memory.screen_start()
    };
    let old_screen = &old[start..];
    // Where a row that did not move is when the block showed the rotation.
    let known = base
        .zip(how.rotation)
        .map(|(base, rotation)| base - isize_of(rotation));
    let guess = known
        .or(base)
        .unwrap_or_else(|| isize_of(total) - isize_of(count));
    // The windows (item 2): the old screen and a screen more at the top,
    // from where the old screen would be, and a screen at the end; a
    // whole-document read is aligned whole.
    let window: Vec<usize> = if whole.is_some() {
        (0..total).collect()
    } else {
        let at = base.map_or(0, |_| usize::try_from(guess + isize_of(start)).unwrap_or(0));
        let top = total.min(at + old_screen.len() + height);
        let bottom = top.max(total.saturating_sub(height));
        (0..top).chain(bottom..total).collect()
    };
    let in_window = |index: usize| window.binary_search(&index).is_ok();
    // Rows with text are aligned; a blank row is kept only in place, below.
    let old_text: Vec<(usize, u64)> = (start..count)
        .filter(|&from| !old[from].is_blank())
        .map(|from| (from, old[from].hash))
        .collect();
    let new_hashes: Vec<u64> = rows.iter().map(|row| row.hash).collect();
    // When the block showed how far the history rotated, a row now above
    // the screen is an old screen row only where that rotation put it: no
    // program writes into the history, so the old screen's rows found
    // elsewhere there repeat text.
    let history_end = total.saturating_sub(height);
    let in_place = |to: usize| {
        known.is_some_and(|shift| {
            usize::try_from(isize_of(to) - shift)
                .is_ok_and(|from| from >= start && from < count && old[from].hash == rows[to].hash)
        })
    };
    let text_window: Vec<usize> = window
        .iter()
        .copied()
        .filter(|&to| {
            !rows[to].is_blank() && (known.is_none() || to >= history_end || in_place(to))
        })
        .collect();
    let kept = align::keep(&old_text, &new_hashes, &text_window, guess);
    let topmost = kept
        .first()
        .map(|&(from, to)| isize_of(to) - isize_of(from));
    // The alternate screen, which has no history above it, never rotates:
    // rows a scroll moved up moved, and those it moved off the top are
    // gone. Only the read's alternate flag is checked: on the main screen,
    // rows moved off the top went into the history, or out of a full one,
    // which is a rotation even when the history holds no rows.
    let (rotation, shift) = match (base, how.rotation, topmost) {
        (Some(base), Some(rotation), _) => (rotation, base - isize_of(rotation)),
        (Some(base), None, Some(top)) if top < base && !read.alternate => {
            (usize::try_from(base - top).unwrap_or(0), top)
        }
        (Some(base), None, _) => (0, base),
        (None, _, Some(top)) => (0, top),
        (None, _, None) => (0, guess),
    };
    // Whether the text of an old row is on no other row of the old screen
    // or of the read.
    let unique = |from: usize| {
        let hash = old[from].hash;
        old_text.iter().filter(|(_, old)| *old == hash).count() == 1
            && rows.iter().filter(|row| row.hash == hash).count() == 1
    };
    // The topmost kept row tells the rotation only when it moved up and
    // its text is unique.
    let trusted = base
        .zip(kept.first())
        .is_some_and(|(base, &(from, to))| isize_of(to) - isize_of(from) < base && unique(from));
    let mut new = vec![NewRow::New; total];
    let mut fates = vec![OldRow::Gone { lingering: false }; count];
    for &(from, to) in &kept {
        new[to] = NewRow::Kept {
            from,
            moved: isize_of(to) - isize_of(from) != shift,
        };
        fates[from] = OldRow::Kept(to);
    }
    // A row whose text is on no other row of the old rows or of the read
    // is kept where that text went, even across rows the longest common
    // subsequence kept, as a list entry moved past others (item 6: the
    // alignment maps every old row to its new place).
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
    for from in start..count {
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
                moved: isize_of(to) - isize_of(from) != shift,
            };
            fates[from] = OldRow::Kept(to);
        }
    }
    // A blank row is kept where the kept row above it, or else below it,
    // puts it.
    let mut placed = kept.iter().peekable();
    let mut shift_above = None;
    for from in start..count {
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
        let row_shift = shift_above.or(below).unwrap_or(shift);
        let found = usize::try_from(isize_of(from) + row_shift)
            .ok()
            .filter(|&to| to < total && rows[to].is_blank() && new[to] == NewRow::New);
        if let Some(to) = found {
            new[to] = NewRow::Kept {
                from,
                moved: row_shift != shift,
            };
            fates[from] = OldRow::Kept(to);
        }
    }
    // Rows the last read held above its screen were not aligned: each
    // follows the topmost kept row's shift where its text is found there.
    // So do the rows above the memory's, held only as hashes: one found is
    // marked history until the old rows are counted as the last read's
    // below.
    let first_dropped = memory.known.len().checked_sub(skip);
    let mut above_fates = Vec::with_capacity(skip);
    for dropped in 0..skip {
        let to = isize_of(dropped) - isize_of(skip) + shift;
        let hash = first_dropped.and_then(|first| memory.known.get(first + dropped));
        let found = usize::try_from(to)
            .ok()
            .filter(|&to| to < total && new[to] == NewRow::New && Some(&rows[to].hash) == hash);
        above_fates.push(match found {
            Some(to) => {
                new[to] = NewRow::Earlier;
                OldRow::Kept(to)
            }
            None if to < 0 && read.at_document_start => OldRow::Left,
            None if to < 0 => OldRow::Above,
            None => OldRow::Gone { lingering: false },
        });
    }
    for from in 0..start {
        let to = isize_of(from) + shift;
        let found = usize::try_from(to)
            .ok()
            .filter(|&to| to < total && new[to] == NewRow::New && rows[to].hash == old[from].hash);
        fates[from] = match found {
            Some(to) => {
                new[to] = NewRow::Kept { from, moved: false };
                OldRow::Kept(to)
            }
            None if to < 0 && read.at_document_start => OldRow::Left,
            None if to < 0 => OldRow::Above,
            None => OldRow::Gone { lingering: false },
        };
    }
    // The rows left between kept rows, paired by likeness (item 4); a
    // blank row is alike to none.
    let mut bounds: Vec<(Option<Pair>, Option<Pair>)> = Vec::new();
    let mut previous = None;
    for &pair in &kept {
        bounds.push((previous, Some(pair)));
        previous = Some(pair);
    }
    bounds.push((previous, None));
    for (above, below) in bounds {
        let old_range =
            above.map_or(start, |(from, _)| from + 1)..below.map_or(count, |(from, _)| from);
        let new_range = above.map_or(0, |(_, to)| to + 1)..below.map_or(total, |(_, to)| to);
        let prefer = below
            .or(above)
            .map_or(shift, |(from, to)| isize_of(to) - isize_of(from));
        let old_gap: Vec<usize> = old_range
            .filter(|&from| {
                isize_of(from) + shift >= 0
                    && fates[from] == OldRow::Gone { lingering: false }
                    && !old[from].is_blank()
            })
            .collect();
        let (Some(&first_old), Some(&last_old)) = (old_gap.first(), old_gap.last()) else {
            continue;
        };
        // A gap of a whole-document read is compared only near where its
        // old rows would be, so the work stays bounded.
        let near = |to: usize| {
            let to = isize_of(to);
            whole.is_none()
                || (to >= isize_of(first_old) + prefer - isize_of(height)
                    && to <= isize_of(last_old) + prefer + isize_of(height))
        };
        let new_gap: Vec<usize> = new_range
            .filter(|&to| {
                in_window(to) && new[to] == NewRow::New && !rows[to].is_blank() && near(to)
            })
            .collect();
        if new_gap.is_empty() {
            continue;
        }
        let old_likeness: Vec<(usize, Likeness)> = old_gap
            .iter()
            .map(|&from| (from, Likeness::of(&old[from].text)))
            .collect();
        let new_likeness: Vec<(usize, Likeness)> = new_gap
            .iter()
            .map(|&to| (to, Likeness::of(&rows[to].text)))
            .collect();
        let old_gap: Vec<(usize, &Likeness)> =
            old_likeness.iter().map(|(at, like)| (*at, like)).collect();
        let new_gap: Vec<(usize, &Likeness)> =
            new_likeness.iter().map(|(at, like)| (*at, like)).collect();
        for (from, to) in align::pair(&old_gap, &new_gap, prefer) {
            new[to] = NewRow::Rewritten {
                from,
                moved: isize_of(to) - isize_of(from) != shift,
            };
            fates[from] = OldRow::Rewritten(to);
        }
    }
    let old_had_text = Screen::has_text(old_screen);
    let kept_text = !kept.is_empty();
    let restores = old_screen
        .iter()
        .enumerate()
        .all(|(at, row)| row.is_blank() || matches!(fates[start + at], OldRow::Kept(_)));
    // Rows with text that no old row became, above the topmost kept row
    // (every row, when none was kept): history the memory never covered,
    // or rows the known history held, which a whole-document read finds
    // again and a reprint repeats (section 4.4).
    let top_kept = kept.first().map_or(total, |&(_, to)| to);
    // Rows above found again by a whole-document read, or above a memory
    // kept aside and restored, are history; above a normal read, a reprint.
    // Rows above where the old rows begin, in a read that started where the
    // last said, are history too: no program writes into the history, and
    // they are older than any row the memory read. After a read reaching
    // the document's start did not find the block, the rows above a kept
    // row now in the history, whose text is on no other row of the old
    // screen or of the read, are older than it; with no such row, as when
    // only a footer on the screen was kept, they may be output that went by
    // unread, and how much went by is not known: the history overflowed.
    let unfound = matches!(whole, Some(Whole::Unfound(_)));
    let history_kept = match whole {
        Some(Whole::Unfound(furthest)) => kept
            .iter()
            .find(|&&(from, to)| {
                to + height < total && unique(from) && isize_of(to) - isize_of(from) <= furthest
            })
            .map(|&(_, to)| to),
        _ => None,
    };
    let earlier = whole == Some(Whole::Lost) || restoring;
    let mut candidates = Vec::new();
    for to in 0..top_kept {
        if new[to] != NewRow::New || rows[to].is_blank() {
            continue;
        }
        if unfound {
            if history_kept.is_some_and(|kept| to < kept) {
                new[to] = NewRow::Earlier;
            }
        } else if (earlier && kept_text || base.is_some()) && isize_of(to) < shift {
            new[to] = NewRow::Earlier;
        } else {
            candidates.push(to);
        }
    }
    // A reprint needs the old screen found again below it, a row of it
    // with text kept: output that only repeats rows the known history
    // held, as after a clear that took the old screen, is new (section
    // 4.4). An old screen with no text has nothing to find, so a
    // whole-document read after it may still find a reprint, and one that
    // lost the old screen finds history again only to count what went by.
    let found_screen = kept_text || whole == Some(Whole::Lost) || whole.is_some() && !old_had_text;
    let mut found_known = false;
    if found_screen && !unfound && !candidates.is_empty() {
        let first_kept = kept.first().map_or(count, |&(from, _)| from);
        let mut known: Vec<u64> = memory.known.iter().copied().collect();
        known.extend(old[..first_kept].iter().map(|row| row.hash));
        // A reprint repeats the rows directly above the old screen; a
        // whole-document read holds whatever of the history is left.
        let tail = if whole.is_some() {
            0
        } else {
            known.len().saturating_sub(candidates.len() + height)
        };
        let hashes: Vec<u64> = candidates.iter().map(|&to| rows[to].hash).collect();
        for at in align::reprinted(&hashes, &known[tail..]) {
            found_known = true;
            new[candidates[at]] = if earlier {
                NewRow::Earlier
            } else {
                NewRow::Reprint
            };
        }
    }
    // A read of the whole document that finds no row of the old screen in a
    // document longer than a screen: more went by than the history holds.
    // So does a read from the kept start reaching the document's start
    // whose only old rows kept moved down with the end of the text, as a
    // footer does: the rows above it went by.
    let stayed = base.is_some_and(|base| {
        kept.iter()
            .any(|&(from, to)| isize_of(to) - isize_of(from) <= base)
    });
    let whole_read = whole == Some(Whole::Lost) && !kept_text
        || whole.is_none() && base.is_some() && read.at_document_start && !restoring && !stayed;
    let overflowed = unfound && history_kept.is_none()
        || whole_read && old_had_text && !found_known && total > height;
    // Gone rows that linger, and lingering rows taken back (section 4.1).
    let blank = !Screen::has_text(rows);
    let old_caret = memory.caret.map(|row| row.saturating_sub(skip));
    for from in start..count {
        if fates[from] != (OldRow::Gone { lingering: false }) {
            continue;
        }
        let at = isize_of(from) + shift;
        // A row that went past the read's start is in the history, or out
        // of the buffer when the read starts at the document's start.
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
        if old[from].redrawn || below_caret || blank {
            fates[from] = OldRow::Gone { lingering: true };
        }
    }
    let mut lingering: Vec<(isize, u64)> = memory
        .lingering
        .iter()
        .filter(|row| read.at_ms.saturating_sub(row.since_ms) <= LINGERING_MS)
        .map(|row| (row.at - isize_of(skip) + shift, row.row.hash))
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
    // The old rows counted as the last read's.
    for kind in &mut new {
        if let NewRow::Kept { from, .. } | NewRow::Rewritten { from, .. } = kind {
            *from += skip;
        }
    }
    for (from, fate) in above_fates.iter().enumerate() {
        if let OldRow::Kept(to) = *fate {
            new[to] = NewRow::Kept { from, moved: false };
        }
    }
    above_fates.extend(fates);
    Classified {
        new,
        old: above_fates,
        rotation,
        shift: shift - isize_of(skip),
        old_had_text,
        kept_text,
        restores,
        overflowed,
        trusted,
    }
}

/// The memory after a read: its rows, each with its history carried from
/// the old row it was, the lingering rows, the known history.
#[expect(clippy::too_many_arguments, reason = "the parts of one read")]
fn remember(
    memory: &Screen,
    known: VecDeque<u64>,
    mut rows: Vec<Row>,
    kinds: &[NewRow],
    fates: &[OldRow],
    shift: isize,
    rotation: usize,
    read: &Read,
    height: usize,
    caret: Option<usize>,
) -> Screen {
    let now = read.at_ms;
    let mut lingering: Vec<Lingering> = memory
        .lingering
        .iter()
        .filter(|row| now.saturating_sub(row.since_ms) <= LINGERING_MS)
        .map(|row| Lingering {
            at: row.at + shift,
            ..row.clone()
        })
        .collect();
    for (to, kind) in kinds.iter().enumerate() {
        let row = &mut rows[to];
        match *kind {
            // A row held only as a hash, found again, starts its history
            // anew.
            NewRow::Kept { from, .. } => {
                if let Some(old) = memory.row(from) {
                    row.redrawn = old.redrawn;
                    row.changed_ms = old.changed_ms;
                }
            }
            NewRow::Rewritten { from, .. } => {
                if let Some(old) = memory.row(from) {
                    row.redrawn = old.redrawn || !row.text.starts_with(&*old.text);
                }
            }
            NewRow::Returned { .. } => {
                let back = lingering
                    .iter()
                    .enumerate()
                    .filter(|(_, gone)| gone.row.hash == row.hash)
                    .min_by_key(|(_, gone)| (gone.at - isize_of(to)).abs())
                    .map(|(index, _)| index);
                if let Some(index) = back {
                    let gone = lingering.remove(index);
                    row.changed_ms = gone.row.changed_ms;
                }
                row.redrawn = true;
            }
            NewRow::New | NewRow::Reprint | NewRow::Earlier => {}
        }
    }
    for (from, fate) in fates.iter().enumerate() {
        if *fate == (OldRow::Gone { lingering: true })
            && let Some(old) = memory.row(from)
        {
            lingering.push(Lingering {
                row: old.clone(),
                since_ms: now,
                at: isize_of(from) + shift,
            });
        }
    }
    let (known, unknown_above) = known_after(memory, known, fates, (rotation, shift), read);
    // A row kept carries whether it was read since the switch that began
    // the memory; any other row with text was read now.
    let read_since = kinds
        .iter()
        .map(|kind| match *kind {
            NewRow::Kept { from, .. } => memory
                .index(from)
                .and_then(|index| memory.read_since.get(index))
                .copied()
                .unwrap_or(false),
            NewRow::New | NewRow::Rewritten { .. } | NewRow::Returned { .. } => true,
            NewRow::Reprint | NewRow::Earlier => false,
        })
        .collect();
    Screen {
        rows,
        skipped: 0,
        height,
        at_document_start: read.at_document_start,
        caret,
        lingering,
        known,
        unknown_above,
        review: None,
        marks: Vec::new(),
        places: Vec::new(),
        continued: false,
        block: 0,
        above: 0,
        read_since,
    }
}

/// The known history after a read, and how many rows the buffer holds
/// above it when that is known (section 4.1): by coordinate, every row the
/// buffer holds above the read, the rows the last read showed there with
/// their hashes when they went above the read with the history, and
/// [`UNREAD`] for a row that went by unread or a coordinate whose row the
/// last read showed but that now holds another; none above a read from the
/// document's start, whose rows above the block the trim puts there.
/// `shift` is where a row of the last read that did not move is now, its
/// index in the read less its index in the last read.
fn known_after(
    memory: &Screen,
    mut known: VecDeque<u64>,
    fates: &[OldRow],
    (rotation, shift): (usize, isize),
    read: &Read,
) -> (VecDeque<u64>, Option<usize>) {
    let mut unknown_above = memory.unknown_above;
    if read.at_document_start {
        known.clear();
        return (known, Some(0));
    }
    // Coordinates are counted from the known history's first row before the
    // rotation, which moves every row up alike: the last read's row `k` was
    // at `last + k`, and the read starts at the row the last read held at
    // `end`.
    let skip = memory.skipped.min(fates.len());
    let held = known.len().min(skip);
    let trimmed: Vec<u64> = known.drain(known.len() - held..).collect();
    let last = isize_of(known.len() + held) - isize_of(skip);
    let end = last - shift;
    let mut coordinate = isize_of(known.len());
    known.truncate(usize::try_from(end).unwrap_or(0));
    while coordinate < end {
        let hash = usize::try_from(coordinate - last)
            .ok()
            .filter(|&row| fates.get(row) == Some(&OldRow::Above))
            .and_then(|row| {
                if row < skip {
                    trimmed.get(row + held - skip).copied()
                } else {
                    memory.rows.get(row - skip).map(|row| row.hash)
                }
            });
        known.push_back(hash.unwrap_or(UNREAD));
        coordinate += 1;
    }
    // The rotation took the buffer's first rows: those above the known
    // history first, when how many is known, then its oldest. When it is
    // not known, its oldest go, which may still be in the buffer.
    let mut left = rotation;
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

/// The review position after a read (section 5.6): a kept row carries it
/// to the row's new place, a rewritten row keeps it with its column
/// clamped to the row's text, and so does a blank row written over in
/// place, which the alignment, pairing no blank row, finds gone; a gone
/// row moves it to the nearest surviving row above, or to the caret when
/// nothing above survived; above the read, it moves with the history, and
/// a row there survives only while the buffer holds it: not above a last
/// read, `memory`'s, that started at the document's start, nor above as
/// many rows as the buffer holds above the read, when `next`, the memory
/// after the read, knows how many.
#[expect(clippy::too_many_arguments, reason = "the parts of one read")]
fn map_anchor(
    anchor: Anchor,
    memory: &Screen,
    kinds: &[NewRow],
    fates: &[OldRow],
    shift: isize,
    next: &Screen,
    caret: Option<usize>,
    read: &Read,
) -> Option<Anchor> {
    let rows = &next.rows;
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
                        && usize::try_from(to).is_ok_and(|to| kinds.get(to) == Some(&NewRow::New));
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
        None => caret.map(|row| anchor_at(rows, row, read.caret_column)),
    }
}

/// A command mark after a read (section 5.13): it moves with its line,
/// and is dropped when its line is gone.
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
/// above the read the row that started its part in the old read, and did
/// not move, and its text begins with that line's and is longer.
fn grew_from(line: &Line, kinds: &[NewRow], memory: &Screen, continued: bool) -> Option<usize> {
    let old = &memory.rows;
    let from = match kinds.get(line.rows.start)? {
        NewRow::Kept { from, moved: false } | NewRow::Rewritten { from, moved: false } => {
            memory.index(*from)?
        }
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

/// The screen's width and height (section 4.3, item 1). Every line's
/// padded length is a whole number of rows, except Windows Terminal's last
/// row of the buffer, cut one cell short (section 2.1), so the width is
/// the narrowest of the screen's lines but its last, in cells. When each
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
/// (section 5.12).
fn split_rows(text: &str, width: usize, now: u64) -> Vec<Row> {
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
                changed_ms: now,
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
/// memory splits a read (section 4.3, item 1): its lines split at line
/// breaks, each into rows of `width` cells, padding trimmed from each
/// line's last row only. For a caller that reads rows the memory does not
/// keep, such as the history above them, at the width a read found.
#[must_use]
pub fn rows_of(text: &str, width: usize) -> Vec<Row> {
    split_rows(text, width, 0)
}

/// A 64-bit hash of `text` (FNV-1a): the same in every process and build,
/// so hashes kept in the memory compare across reads.
#[must_use]
pub fn hash(text: &str) -> u64 {
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    // The known history's mark for a row that went by unread is no row's.
    if hash == UNREAD { 1 } else { hash }
}

fn isize_of(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

#[cfg(test)]
mod tests;
