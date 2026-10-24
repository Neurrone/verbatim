//! The review cursor's text requests in a focused terminal
//! (`docs/design/terminal-model.md`, "The review anchor and following"), answered from the
//! terminal's memory of its rows, never from a live provider range, which
//! could point at other text once output rotates a full history: the worker
//! first takes a read, so the memory's alignment has moved every place onto
//! its text as it is now, and the request is then answered within that
//! read. A line is a row, its padding left out ("Review line ends"), so moving
//! along a row ends at its text.
//!
//! Each row a request reads is given a text anchor whose place the memory
//! keeps ([`verbatim_text::terminal_rows::TerminalMemory::add_place`]): it
//! moves with its row, so the review cursor stays on its text while output
//! scrolls it, into the history included, and a redraw elsewhere never
//! moves it. Rows above those the memory keeps, in the history, are read
//! from the provider by how far above the next read's start they lie, in a
//! snapshot of their own ([`History`]); a place there is the nearest row
//! of its line's hash from the place up to a screen above it, or else the
//! place itself ("The review anchor").

use std::collections::HashSet;

use verbatim_model::{
    MAX_RANGE_BYTES, MAX_READ_AHEAD, MAX_READ_AHEAD_TEXT, TextAnchor, TextChunk, TextMovement,
    TextOp, TextPoint, TextRead, TextReadAhead, TextReply, TextThrough, TextUnit,
};
use verbatim_text::terminal_rows::{Anchor, Row, rows_of};

use super::Terminal;
use crate::text::TextError;

/// How many places a terminal keeps beyond those whose anchors Core holds.
const KEPT_PLACES: usize = crate::text::KEPT_ANCHORS;

/// Reads the history above the rows a terminal's memory keeps.
pub trait History {
    /// The text from `above` rows above the next read's start to the
    /// document's end, or from the document's start when `whole`, and how
    /// many rows it holds above that start: fewer than `above` when the
    /// document's start came first.
    ///
    /// # Errors
    ///
    /// When the terminal cannot be read.
    fn read(&mut self, above: u32, whole: bool) -> Result<(String, usize), TextError>;
}

/// The caret as the read before the request found it: its row of the
/// memory's last read and its column in characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaretAt {
    /// Its row.
    pub row: usize,
    /// Its column, in characters.
    pub column: usize,
}

/// What answering a request needs besides the terminal: the history, the
/// caret as the read before it found it, how to number a new anchor, and
/// the anchors Core holds.
pub struct Context<'a> {
    /// The history above the memory's rows.
    pub history: &'a mut dyn History,
    /// The caret.
    pub caret: Option<CaretAt>,
    /// Numbers a new text anchor.
    pub mint: &'a mut dyn FnMut() -> u64,
    /// The anchors Core holds.
    pub held: &'a HashSet<u64>,
}

/// Whether the terminal's review answers `op` from its memory: a read of
/// the text from the caret, the start, the end, or a position at an anchor
/// the terminal minted; a request naming another anchor, such as a caret
/// report's, is answered as any text's.
#[must_use]
pub fn answers(terminal: &Terminal, op: &TextOp) -> bool {
    let known = |point: &TextPoint| match point {
        TextPoint::At(position) => terminal.place_of(position.anchor).is_some(),
        _ => true,
    };
    match op {
        TextOp::Read(TextRead { at, .. }) | TextOp::ReadAhead(TextReadAhead { at, .. }) => {
            known(at)
        }
        TextOp::ReadRange { start, end, .. } | TextOp::Select { start, end, .. } => {
            known(start) && known(end)
        }
        TextOp::Location(point) | TextOp::MoveCaret(point) => known(point),
        _ => false,
    }
}

/// Answers `op` from `terminal`'s memory, read just before ([`answers`]
/// says which requests it answers).
///
/// # Errors
///
/// When the history above the memory's rows cannot be read.
pub fn answer(
    terminal: &mut Terminal,
    op: &TextOp,
    context: Context<'_>,
) -> Result<TextReply, TextError> {
    let Context {
        history,
        caret,
        mint,
        held,
    } = context;
    let plan = {
        let mut rows = Rows {
            terminal,
            history,
            above: None,
        };
        match op {
            TextOp::Read(read) => read_unit(&mut rows, caret, read)?,
            TextOp::ReadAhead(ahead) => read_ahead(&mut rows, caret, ahead)?,
            TextOp::ReadRange {
                start,
                end,
                through,
            } => read_range(&mut rows, caret, (*start, *end), *through)?,
            // A terminal's text is not selected, nor its caret moved, by
            // Verbatim, and a place's position on the screen is not read.
            _ => Plan::Reply(TextReply::Unsupported),
        }
    };
    Ok(terminal.finish(plan, mint, held))
}

/// One row of the terminal's text.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Line {
    text: String,
    wraps: bool,
    hash: u64,
}

impl From<&Row> for Line {
    fn from(row: &Row) -> Self {
        Self {
            text: row.text.to_string(),
            wraps: row.wraps,
            hash: row.line_hash,
        }
    }
}

/// A row to send as a chunk.
#[derive(Debug)]
struct Chunked {
    row: isize,
    line: Line,
    /// The byte offset of the point in its text.
    offset: usize,
    first: bool,
    last: bool,
}

/// An answer before its rows' anchors are minted.
#[derive(Debug)]
enum Plan {
    /// A read: how far it moved, and the row it landed on, where the
    /// review position goes.
    Read(i32, Chunked),
    /// A read ahead: how far it moved, and the rows.
    Ahead(i32, Vec<Chunked>),
    /// Any other answer.
    Reply(TextReply),
}

impl Terminal {
    /// The place the terminal keeps for text anchor `anchor`, if it minted
    /// it.
    fn place_of(&self, anchor: TextAnchor) -> Option<u64> {
        self.places
            .iter()
            .find(|(kept, _)| *kept == anchor.0)
            .map(|(_, place)| *place)
    }

    /// Keeps a place at `anchor` for the text anchor numbered `number`,
    /// forgetting the oldest places whose anchors Core does not hold beyond
    /// [`KEPT_PLACES`].
    fn keep_place(&mut self, number: u64, anchor: Anchor, held: &HashSet<u64>) {
        let place = self.reader.memory_mut().add_place(anchor);
        self.places.push_back((number, place));
        while self.places.len() > KEPT_PLACES {
            let Some(at) = self
                .places
                .iter()
                .position(|(kept, _)| !held.contains(kept))
            else {
                break;
            };
            if let Some((_, place)) = self.places.remove(at) {
                self.reader.memory_mut().drop_place(place);
            }
        }
    }

    /// Mints the anchors of `plan`'s rows and keeps their places, and puts
    /// the review position where a read landed.
    fn finish(
        &mut self,
        plan: Plan,
        mint: &mut dyn FnMut() -> u64,
        held: &HashSet<u64>,
    ) -> TextReply {
        let mut chunk_of = |terminal: &mut Self, chunked: Chunked| {
            let number = mint();
            terminal.keep_place(
                number,
                Anchor {
                    row: chunked.row,
                    column: 0,
                    line_hash: chunked.line.hash,
                },
                held,
            );
            TextChunk {
                unit: TextUnit::Line,
                start: TextAnchor(number),
                offset: u32::try_from(chunked.offset).unwrap_or(u32::MAX),
                text: chunked.line.text,
                languages: Vec::new(),
                first: chunked.first,
                last: chunked.last,
                truncated: false,
                formats: Vec::new(),
            }
        };
        match plan {
            Plan::Read(moved, chunked) => {
                let review = Anchor {
                    row: chunked.row,
                    column: chunked.line.text[..chunked.offset].chars().count(),
                    line_hash: chunked.line.hash,
                };
                let chunk = chunk_of(self, chunked);
                self.reader.memory_mut().set_review_anchor(review);
                TextReply::Read { moved, chunk }
            }
            Plan::Ahead(moved, rows) => {
                let chunks = rows
                    .into_iter()
                    .map(|chunked| chunk_of(self, chunked))
                    .collect();
                TextReply::Chunks { moved, chunks }
            }
            Plan::Reply(reply) => reply,
        }
    }
}

/// The rows a request reads: the memory's, and the history's as it is
/// read, each by its row of the memory's last read.
struct Rows<'a> {
    terminal: &'a Terminal,
    history: &'a mut dyn History,
    /// The history read so far: its first row, its rows, and whether it
    /// reached the document's start.
    above: Option<(isize, Vec<Line>, bool)>,
}

impl Rows<'_> {
    /// How many rows the memory's last read held.
    fn len(&self) -> isize {
        isize_of(self.terminal.reader.memory().rows().len())
    }

    /// The first row the memory keeps.
    fn first_kept(&self) -> isize {
        isize_of(self.terminal.reader.memory().rows().first_kept())
    }

    /// The row the next read starts at: the screen's top.
    fn start_row(&self) -> isize {
        let height = usize::try_from(self.terminal.screen_rows).unwrap_or(0);
        (self.len() - isize_of(height)).max(0)
    }

    /// The last row.
    fn last(&self) -> isize {
        self.len() - 1
    }

    /// The document's first row, when it is known: the memory's last read
    /// started there, or the history's read reached it.
    fn known_first(&self) -> Option<isize> {
        if self.terminal.at_document_start {
            return Some(0);
        }
        match &self.above {
            Some((first, _, true)) => Some(*first),
            _ => None,
        }
    }

    /// Reads the history from row `row` to the document's end, or the
    /// whole document when `row` is `None`.
    fn read_history(&mut self, row: Option<isize>) -> Result<(), TextError> {
        let start = self.start_row();
        let wanted = row.map_or(0, |row| usize::try_from(start - row).unwrap_or(0));
        let (text, above) = self
            .history
            .read(u32::try_from(wanted).unwrap_or(u32::MAX), row.is_none())?;
        let rows: Vec<Line> = rows_of(&text, self.terminal.width)
            .iter()
            .map(Line::from)
            .collect();
        // The history's rows end where the memory's do, at the document's
        // end; a whole document's read counts nothing above the start.
        let (first, reached) = match row {
            None => (self.len() - isize_of(rows.len()), true),
            Some(_) => (start - isize_of(above), above < wanted),
        };
        self.above = Some((first, rows, reached));
        Ok(())
    }

    /// The document's first row, the whole document read when it is not
    /// known.
    fn first(&mut self) -> Result<isize, TextError> {
        if let Some(first) = self.known_first() {
            return Ok(first);
        }
        self.read_history(None)?;
        Ok(self.known_first().unwrap_or(0))
    }

    /// Row `row`, read from the history when the memory does not keep it;
    /// `None` when there is no such row.
    fn row(&mut self, row: isize) -> Result<Option<Line>, TextError> {
        if row > self.last() {
            return Ok(None);
        }
        if row >= self.first_kept() {
            let index = usize::try_from(row).unwrap_or(0);
            return Ok(self
                .terminal
                .reader
                .memory()
                .rows()
                .get(index)
                .map(Line::from));
        }
        if self.known_first().is_some_and(|first| row < first) {
            return Ok(None);
        }
        let covered = matches!(&self.above, Some((first, _, _)) if *first <= row);
        if !covered {
            self.read_history(Some(row))?;
        }
        Ok(self.above.as_ref().and_then(|(first, rows, _)| {
            usize::try_from(row - first)
                .ok()
                .and_then(|at| rows.get(at))
                .cloned()
        }))
    }

    /// Where a place on row `row`, of a line with hash `line_hash`, is
    /// now ("The review anchor"): the row itself where the memory keeps
    /// it, since its alignment moved it; above the memory's rows, the
    /// nearest row of a line with its hash from the place up to a screen
    /// above it, since a full history only moves rows up, the rows read by
    /// place in one read; with none, the place itself and the row there
    /// now.
    fn found(&mut self, row: isize, line_hash: u64) -> Result<Option<isize>, TextError> {
        if row >= self.first_kept() {
            return Ok((row <= self.last()).then_some(row));
        }
        let screen = isize_of(usize::try_from(self.terminal.screen_rows.max(1)).unwrap_or(1));
        let top = self
            .known_first()
            .map_or(row - screen, |first| (row - screen).max(first));
        // One read from a screen above the place down covers the search.
        self.row(top)?;
        for candidate in (top..=row).rev() {
            if self
                .row(candidate)?
                .is_some_and(|line| line.hash == line_hash)
            {
                return Ok(Some(candidate));
            }
        }
        Ok(self.row(row)?.map(|_| row))
    }

    /// Whether `row` is the document's first row, as far as is known.
    fn is_first(&self, row: isize) -> bool {
        self.known_first() == Some(row)
    }

    /// The chunk of row `row`, with the point at `offset`.
    fn chunked(&mut self, row: isize, offset: usize) -> Result<Option<Chunked>, TextError> {
        let Some(line) = self.row(row)? else {
            return Ok(None);
        };
        let offset = floor_boundary(&line.text, offset);
        Ok(Some(Chunked {
            row,
            first: self.is_first(row),
            last: row == self.last(),
            line,
            offset,
        }))
    }
}

/// A point of the text: a row and a byte offset in its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Point {
    row: isize,
    offset: usize,
}

/// The byte offset of character `column` of `text`, or its end.
fn byte_at(text: &str, column: usize) -> usize {
    text.char_indices()
        .nth(column)
        .map_or(text.len(), |(at, _)| at)
}

/// The largest character boundary of `text` at or before `offset`.
fn floor_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Where `point` is, or the reply that ends the request: a place gone
/// with its row is an anchor lost.
fn resolve(
    rows: &mut Rows<'_>,
    caret: Option<CaretAt>,
    point: TextPoint,
) -> Result<Result<Point, TextReply>, TextError> {
    let at_caret = |rows: &mut Rows<'_>| -> Result<Point, TextError> {
        let Some(caret) = caret else {
            return Ok(Point {
                row: rows.last().max(0),
                offset: 0,
            });
        };
        let row = isize_of(caret.row);
        let offset = rows
            .row(row)?
            .map_or(0, |line| byte_at(&line.text, caret.column));
        Ok(Point { row, offset })
    };
    Ok(Ok(match point {
        TextPoint::Caret | TextPoint::SelectionStart | TextPoint::SelectionEnd => at_caret(rows)?,
        TextPoint::Start => Point {
            row: rows.first()?,
            offset: 0,
        },
        TextPoint::End => {
            let row = rows.last().max(0);
            let offset = rows.row(row)?.map_or(0, |line| line.text.len());
            Point { row, offset }
        }
        TextPoint::At(position) => {
            let place = rows
                .terminal
                .place_of(position.anchor)
                .and_then(|place| rows.terminal.reader.memory().place(place));
            let Some(place) = place else {
                return Ok(Err(TextReply::AnchorLost));
            };
            let Some(row) = rows.found(place.row, place.line_hash)? else {
                return Ok(Err(TextReply::AnchorLost));
            };
            Point {
                row,
                offset: position.offset as usize,
            }
        }
    }))
}

/// `point` moved by `movement`, a line or a screen's rows at a time, onto
/// the start of a row, never past the text's ends, and how far it went in
/// units; the reply that ends the request for any other unit.
fn moved(
    rows: &mut Rows<'_>,
    point: Point,
    movement: Option<TextMovement>,
) -> Result<Result<(Point, i32), TextReply>, TextError> {
    let Some(movement) = movement else {
        return Ok(Ok((point, 0)));
    };
    let per = match movement.unit {
        TextUnit::Line => 1,
        TextUnit::Page => isize_of(usize::try_from(rows.terminal.screen_rows).unwrap_or(1)).max(1),
        unit => return Ok(Err(TextReply::UnsupportedUnit(unit))),
    };
    let wanted = point.row + isize::try_from(movement.count).unwrap_or(0) * per;
    let target = if wanted > point.row {
        wanted.min(rows.last())
    } else if wanted < point.row && wanted < rows.first_kept() {
        // Rows above the memory's are read until the document's start.
        let _ = rows.row(wanted)?;
        rows.known_first().map_or(wanted, |first| wanted.max(first))
    } else {
        wanted
    };
    let rows_moved = target - point.row;
    let units = if per == 1 {
        rows_moved
    } else {
        // Pages moved, counting a part of one at an end.
        if rows_moved == 0 {
            0
        } else {
            rows_moved.signum() * (rows_moved.abs() + per - 1) / per
        }
    };
    let offset = if target == point.row { point.offset } else { 0 };
    Ok(Ok((
        Point {
            row: target,
            offset,
        },
        i32::try_from(units).unwrap_or(0),
    )))
}

/// A read of one unit, a row ([`TextOp::Read`]).
fn read_unit(
    rows: &mut Rows<'_>,
    caret: Option<CaretAt>,
    read: &TextRead,
) -> Result<Plan, TextError> {
    if read.unit != TextUnit::Line {
        return Ok(Plan::Reply(TextReply::UnsupportedUnit(read.unit)));
    }
    let point = match resolve(rows, caret, read.at)? {
        Ok(point) => point,
        Err(reply) => return Ok(Plan::Reply(reply)),
    };
    let (point, count) = match moved(rows, point, read.movement)? {
        Ok(moved) => moved,
        Err(reply) => return Ok(Plan::Reply(reply)),
    };
    Ok(match rows.chunked(point.row, point.offset)? {
        Some(chunked) => Plan::Read(count, chunked),
        None => Plan::Reply(TextReply::AnchorLost),
    })
}

/// Rows read ahead, for say-all ([`TextOp::ReadAhead`]).
fn read_ahead(
    rows: &mut Rows<'_>,
    caret: Option<CaretAt>,
    ahead: &TextReadAhead,
) -> Result<Plan, TextError> {
    if ahead.unit != TextUnit::Line {
        return Ok(Plan::Reply(TextReply::UnsupportedUnit(ahead.unit)));
    }
    let point = match resolve(rows, caret, ahead.at)? {
        Ok(point) => point,
        Err(reply) => return Ok(Plan::Reply(reply)),
    };
    let (point, count) = match moved(rows, point, ahead.movement)? {
        Ok(moved) => moved,
        Err(reply) => return Ok(Plan::Reply(reply)),
    };
    let wanted = usize::from(ahead.count.clamp(1, MAX_READ_AHEAD));
    let mut chunks: Vec<Chunked> = Vec::new();
    let mut bytes = 0;
    let mut row = point.row;
    while chunks.len() < wanted && bytes < MAX_READ_AHEAD_TEXT {
        let offset = if chunks.is_empty() { point.offset } else { 0 };
        let Some(chunked) = rows.chunked(row, offset)? else {
            break;
        };
        bytes += chunked.line.text.len();
        let last = chunked.last;
        chunks.push(chunked);
        if last {
            break;
        }
        row += 1;
    }
    if chunks.is_empty() {
        return Ok(Plan::Reply(TextReply::AnchorLost));
    }
    Ok(Plan::Ahead(count, chunks))
}

/// The text between two points ([`TextOp::ReadRange`]), in document
/// order whichever comes first; with `through`, through the character at
/// the later end. Rows of one line are joined as they are, and lines with
/// a line break between them.
fn read_range(
    rows: &mut Rows<'_>,
    caret: Option<CaretAt>,
    (start, end): (TextPoint, TextPoint),
    through: Option<TextThrough>,
) -> Result<Plan, TextError> {
    let a = match resolve(rows, caret, start)? {
        Ok(point) => point,
        Err(reply) => return Ok(Plan::Reply(reply)),
    };
    let b = match resolve(rows, caret, end)? {
        Ok(point) => point,
        Err(reply) => return Ok(Plan::Reply(reply)),
    };
    let (from, mut to) = if (a.row, a.offset) <= (b.row, b.offset) {
        (a, b)
    } else {
        (b, a)
    };
    if through.is_some()
        && let Some(line) = rows.row(to.row)?
    {
        let at = floor_boundary(&line.text, to.offset);
        to.offset = verbatim_text::grapheme_at(&line.text, at).map_or(at, |range| range.end);
    }
    let mut text = String::new();
    let mut truncated = false;
    for row in from.row..=to.row {
        let Some(line) = rows.row(row)? else {
            break;
        };
        let begin = if row == from.row {
            floor_boundary(&line.text, from.offset)
        } else {
            0
        };
        let end = if row == to.row {
            floor_boundary(&line.text, to.offset).max(begin)
        } else {
            line.text.len()
        };
        text.push_str(&line.text[begin..end]);
        if row < to.row && !line.wraps {
            text.push_str("\r\n");
        }
        if text.len() > MAX_RANGE_BYTES {
            let cut = floor_boundary(&text, MAX_RANGE_BYTES);
            text.truncate(cut);
            truncated = true;
            break;
        }
    }
    Ok(Plan::Reply(TextReply::Range { text, truncated }))
}

fn isize_of(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}
