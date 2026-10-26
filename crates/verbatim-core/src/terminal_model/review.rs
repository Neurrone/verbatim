//! The review cursor's text requests in a terminal Core reads
//! (`docs/design/terminal-model.md`, "The review cursor"), answered from
//! the terminal's memory of its rows, never from a live provider range,
//! which could point at other text once output rotates a full history:
//! Core first takes a fresh read, so the memory's alignment has moved every
//! place onto its text as it is now, and the request is then answered
//! within that read. A line is a row, its padding left out, so moving along
//! a row ends at its text.
//!
//! Each row a request reads is given a text anchor from the range Core
//! keeps for itself ([`super::TERMINAL_ANCHORS`]), whose place the memory
//! keeps ([`verbatim_text::terminal_rows::TerminalMemory::add_place`]): it
//! moves with its row, so the review cursor stays on its text while output
//! scrolls it, into the history included, and a redraw elsewhere never
//! moves it. Rows above those the memory keeps, in the history, are read
//! from the outpost by how far above the next read's start they lie: the
//! answer asks for them ([`Need`]), and is worked out again once they come
//! ([`super::HistoryRead`]). A place there is the nearest row of its
//! line's hash from a screen above the place to a screen below it, or else
//! the place itself.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    MAX_RANGE_BYTES, MAX_READ_AHEAD, MAX_READ_AHEAD_TEXT, TextAnchor, TextChunk, TextMovement,
    TextOp, TextPoint, TextRead, TextReadAhead, TextReply, TextThrough, TextUnit,
};
use verbatim_text::terminal_rows::{Anchor, Row, rows_of};

use super::{HistoryRead, TERMINAL_ANCHORS, TerminalModel};

/// How many places a terminal keeps beyond those whose anchors Core holds.
const KEPT_PLACES: usize = 64;

/// Rows of the history an answer needs that have not been read: from
/// `above` rows above the next read's start to the document's end, or the
/// whole document when `whole`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Need {
    pub(crate) above: u32,
    pub(crate) whole: bool,
}

/// The caret as the read before the request found it: its row of the
/// memory's last read and its column in characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CaretAt {
    /// Its row.
    pub(crate) row: usize,
    /// Its column, in characters.
    pub(crate) column: usize,
}

/// Whether Core answers `op` from `model`'s memory: a read of the text
/// from the caret, the start, the end, or a position at an anchor Core
/// minted for the terminal; a request naming another anchor, such as a
/// caret report's, goes to the outpost and is answered as any text's.
pub(crate) fn answers(model: &TerminalModel, op: &TextOp) -> bool {
    let known = |point: &TextPoint| match point {
        TextPoint::At(position) => place_of(model, position.anchor).is_some(),
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

/// Answers `op` from `model`'s memory ([`answers`] says which requests it
/// answers), numbering new anchors from `next_anchor` and keeping the
/// places of the anchors in `held`.
///
/// # Errors
///
/// [`Need`] when the answer needs rows of the history that have not been
/// read: nothing is changed, and the answer is worked out again once they
/// come.
pub(crate) fn answer(
    model: &mut TerminalModel,
    op: &TextOp,
    next_anchor: &mut u64,
    held: &HashSet<u64>,
) -> Result<TextReply, Need> {
    let caret = model.caret;
    let plan = {
        let mut rows = Rows { model, above: None };
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
    let mut mint = || {
        *next_anchor += 1;
        TERMINAL_ANCHORS | *next_anchor
    };
    Ok(finish(model, plan, &mut mint, held))
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

/// The place the terminal keeps for text anchor `anchor`, if Core minted
/// it.
fn place_of(model: &TerminalModel, anchor: TextAnchor) -> Option<u64> {
    model
        .places
        .iter()
        .find(|(kept, _)| *kept == anchor.0)
        .map(|(_, place)| *place)
}

/// Keeps a place at `anchor` for the text anchor numbered `number`,
/// forgetting the oldest places whose anchors Core does not hold beyond
/// [`KEPT_PLACES`].
fn keep_place(model: &mut TerminalModel, number: u64, anchor: Anchor, held: &HashSet<u64>) {
    let place = model.rules().memory_mut().add_place(anchor);
    model.places.push_back((number, place));
    while model.places.len() > KEPT_PLACES {
        let Some(at) = model
            .places
            .iter()
            .position(|(kept, _)| !held.contains(kept))
        else {
            break;
        };
        if let Some((_, place)) = model.places.remove(at) {
            model.rules().memory_mut().drop_place(place);
        }
    }
}

/// Mints the anchors of `plan`'s rows and keeps their places, and puts the
/// review position where a read landed.
fn finish(
    model: &mut TerminalModel,
    plan: Plan,
    mint: &mut dyn FnMut() -> u64,
    held: &HashSet<u64>,
) -> TextReply {
    let mut chunk_of = |model: &mut TerminalModel, chunked: Chunked| {
        let number = mint();
        keep_place(
            model,
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
            let chunk = chunk_of(model, chunked);
            model.rules().memory_mut().set_review_anchor(review);
            TextReply::Read { moved, chunk }
        }
        Plan::Ahead(moved, rows) => {
            let chunks = rows
                .into_iter()
                .map(|chunked| chunk_of(model, chunked))
                .collect();
            TextReply::Chunks { moved, chunks }
        }
        Plan::Reply(reply) => reply,
    }
}

/// The rows a request reads: the memory's, and the history's as it came,
/// each by its row of the memory's last read.
struct Rows<'a> {
    model: &'a TerminalModel,
    /// The history read so far: its first row, its rows, and whether it
    /// reached the document's start.
    above: Option<(isize, Vec<Line>, bool)>,
}

impl Rows<'_> {
    /// How many rows the memory's last read held.
    fn len(&self) -> isize {
        isize_of(self.model.reader.memory().rows().len())
    }

    /// The first row the memory keeps.
    fn first_kept(&self) -> isize {
        isize_of(self.model.reader.memory().rows().first_kept())
    }

    /// The screen's height, in rows.
    fn height(&self) -> isize {
        isize_of(usize::try_from(self.model.height).unwrap_or(0))
    }

    /// The row the next read starts at: the screen's top.
    fn start_row(&self) -> isize {
        (self.len() - self.height()).max(0)
    }

    /// The last row.
    fn last(&self) -> isize {
        self.len() - 1
    }

    /// The document's first row, when it is known: the memory's last read
    /// started there, or the history's read reached it.
    fn known_first(&self) -> Option<isize> {
        if self.model.at_document_start {
            return Some(0);
        }
        match &self.above {
            Some((first, _, true)) => Some(*first),
            _ => None,
        }
    }

    /// The history from row `row` to the document's end, or the whole
    /// document when `row` is `None`, as it came.
    fn read_history(&mut self, row: Option<isize>) -> Result<(), Need> {
        let start = self.start_row();
        let wanted = row.map_or(0, |row| usize::try_from(start - row).unwrap_or(0));
        let need = Need {
            above: u32::try_from(wanted).unwrap_or(u32::MAX),
            whole: row.is_none(),
        };
        let read: &HistoryRead = self
            .model
            .history
            .iter()
            .find(|read| read.above == need.above && read.whole == need.whole)
            .ok_or(need)?;
        let above = usize::try_from(read.rows_above).unwrap_or(usize::MAX);
        let rows: Vec<Line> = rows_of(&read.text, self.model.width)
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
    fn first(&mut self) -> Result<isize, Need> {
        if let Some(first) = self.known_first() {
            return Ok(first);
        }
        self.read_history(None)?;
        Ok(self.known_first().unwrap_or(0))
    }

    /// Row `row`, read from the history when the memory does not keep it;
    /// `None` when there is no such row.
    fn row(&mut self, row: isize) -> Result<Option<Line>, Need> {
        if row > self.last() {
            return Ok(None);
        }
        if row >= self.first_kept() {
            let index = usize::try_from(row).unwrap_or(0);
            return Ok(self.model.reader.memory().rows().get(index).map(Line::from));
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
    /// nearest row of a line with its hash from a screen above the place
    /// to a screen below it, the higher first at one distance, since a full
    /// history moves rows up and a program printing its history again
    /// longer moves them down, the rows read by place in one read; with
    /// none, the place itself and the row there now.
    fn found(&mut self, row: isize, line_hash: u64) -> Result<Option<isize>, Need> {
        if row >= self.first_kept() {
            return Ok((row <= self.last()).then_some(row));
        }
        let screen = self.height().max(1);
        let top = self
            .known_first()
            .map_or(row - screen, |first| (row - screen).max(first));
        let bottom = (row + screen).min(self.last());
        // One read from a screen above the place to the document's end
        // covers the search.
        self.row(top)?;
        for distance in 0..=screen {
            for candidate in [row - distance, row + distance] {
                if (top..=bottom).contains(&candidate)
                    && self
                        .row(candidate)?
                        .is_some_and(|line| line.hash == line_hash)
                {
                    return Ok(Some(candidate));
                }
            }
        }
        Ok(self.row(row)?.map(|_| row))
    }

    /// Whether `row` is the document's first row, as far as is known.
    fn is_first(&self, row: isize) -> bool {
        self.known_first() == Some(row)
    }

    /// The chunk of row `row`, with the point at `offset`.
    fn chunked(&mut self, row: isize, offset: usize) -> Result<Option<Chunked>, Need> {
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
) -> Result<Result<Point, TextReply>, Need> {
    let at_caret = |rows: &mut Rows<'_>| -> Result<Point, Need> {
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
            let place = place_of(rows.model, position.anchor)
                .and_then(|place| rows.model.reader.memory().place(place));
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
) -> Result<Result<(Point, i32), TextReply>, Need> {
    let Some(movement) = movement else {
        return Ok(Ok((point, 0)));
    };
    let per = match movement.unit {
        TextUnit::Line => 1,
        TextUnit::Page => rows.height().max(1),
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
fn read_unit(rows: &mut Rows<'_>, caret: Option<CaretAt>, read: &TextRead) -> Result<Plan, Need> {
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
) -> Result<Plan, Need> {
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
) -> Result<Plan, Need> {
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

/// What became of one command mark a move looked at.
enum Resolved {
    /// Its line is on this row, whose first row's text is this.
    Found(isize, Line),
    /// Its line is nowhere: the mark is dropped.
    Gone,
    /// The search for its line above the memory has not been answered.
    Need(TextOp),
}

/// Moves the review cursor to the nearest command mark strictly above
/// (`forward` false) or below the review position at `from`, or the caret
/// (`docs/design/terminal-model.md`, "Features and settings"), after the
/// fresh read: each mark, nearest first, is found by its line's text,
/// within the memory near its place, and above the memory by the search
/// the outpost runs ([`TextOp::TerminalFind`]); a mark not found is
/// dropped, and the next one tried, with nothing said. The answer is a
/// line move's ([`TextReply::Read`] at the start of the mark's row), the
/// review position going there; `None` when no mark is left that way.
///
/// # Errors
///
/// The search a mark above the memory needs, when it has not been
/// answered: the marks dropped so far stay dropped, and the move is worked
/// out again once the answer comes.
pub(crate) fn mark(
    model: &mut TerminalModel,
    (forward, from): (bool, &TextPoint),
    next_anchor: &mut u64,
    held: &HashSet<u64>,
) -> Result<Option<TextReply>, TextOp> {
    let review = match from {
        TextPoint::At(position) => place_of(model, position.anchor)
            .and_then(|place| model.reader.memory().place(place))
            .map(|anchor| anchor.row),
        _ => None,
    };
    let rows = model.reader.memory().rows();
    let len = isize_of(rows.len());
    let first_kept = isize_of(rows.first_kept());
    let review = review
        .or_else(|| model.caret.map(|caret| isize_of(caret.row)))
        .unwrap_or(len - 1);
    // Up goes above the review position's line, not its own first row.
    let mut review_start = review;
    while review_start > first_kept && wraps_at(model, review_start - 1) {
        review_start -= 1;
    }
    let beyond = |row: isize| {
        if forward {
            row > review
        } else {
            row < review_start
        }
    };
    let mut candidates: Vec<Anchor> = model
        .reader
        .memory()
        .marks()
        .iter()
        .copied()
        .filter(|mark| beyond(mark.row))
        .collect();
    if forward {
        candidates.sort_by_key(|mark| mark.row);
    } else {
        candidates.sort_by_key(|mark| std::cmp::Reverse(mark.row));
    }
    for candidate in candidates {
        match resolve_mark(model, candidate, forward) {
            Resolved::Found(row, line) if beyond(row) => {
                let mark = Anchor {
                    row,
                    column: 0,
                    line_hash: candidate.line_hash,
                };
                model
                    .rules()
                    .memory_mut()
                    .replace_mark(candidate, Some(mark));
                let chunked = Chunked {
                    row,
                    first: model.at_document_start && row == 0,
                    last: row == len - 1,
                    line,
                    offset: 0,
                };
                let moved = if forward { 1 } else { -1 };
                let mut mint = || {
                    *next_anchor += 1;
                    TERMINAL_ANCHORS | *next_anchor
                };
                return Ok(Some(finish(
                    model,
                    Plan::Read(moved, chunked),
                    &mut mint,
                    held,
                )));
            }
            // Its line is on the other side of the review position: the
            // mark is kept there, and the next one tried.
            Resolved::Found(row, _) => {
                let mark = Anchor {
                    row,
                    column: 0,
                    line_hash: candidate.line_hash,
                };
                model
                    .rules()
                    .memory_mut()
                    .replace_mark(candidate, Some(mark));
            }
            Resolved::Gone => model.rules().memory_mut().replace_mark(candidate, None),
            Resolved::Need(op) => return Err(op),
        }
    }
    Ok(None)
}

/// Whether row `row` of the memory continues onto the next.
fn wraps_at(model: &TerminalModel, row: isize) -> bool {
    usize::try_from(row)
        .ok()
        .and_then(|row| model.reader.memory().rows().get(row))
        .is_some_and(|found| found.wraps)
}

/// Where command mark `mark`'s line is now: within the memory, the line
/// starting at its row with its hash, or else the nearest line start with
/// it, the higher at one distance; otherwise, unless the memory holds the
/// whole document, the first match of the outpost's search for its first
/// row whose line has its hash ([`mark_search`]), searched backward from a
/// screen below its place for a move up and forward from a screen above it
/// for a move down.
fn resolve_mark(model: &TerminalModel, mark: Anchor, forward: bool) -> Resolved {
    let rows = model.reader.memory().rows();
    let first_kept = isize_of(rows.first_kept());
    let len = isize_of(rows.len());
    let starts = |row: isize| {
        usize::try_from(row)
            .ok()
            .and_then(|at| rows.get(at))
            .is_some_and(|found| found.line_hash == mark.line_hash)
            && (row == first_kept || !wraps_at(model, row - 1))
    };
    let line = |row: isize| {
        usize::try_from(row)
            .ok()
            .and_then(|at| rows.get(at))
            .map(Line::from)
    };
    if mark.row >= first_kept {
        let nearest = (0..len)
            .flat_map(|distance| [mark.row - distance, mark.row + distance])
            .find(|&row| row >= first_kept && row < len && starts(row));
        if let Some((row, line)) = nearest.and_then(|row| Some((row, line(row)?))) {
            return Resolved::Found(row, line);
        }
        if first_kept == 0 && model.at_document_start {
            return Resolved::Gone;
        }
    } else if model.at_document_start && mark.row < 0 {
        return Resolved::Gone;
    }
    let Some(sought) = model
        .mark_lines
        .iter()
        .find(|line| line.hash == mark.line_hash)
    else {
        return Resolved::Gone;
    };
    let op = mark_search(model, mark.row, sought, forward);
    let Some((_, matches)) = model.found.iter().find(|(asked, _)| *asked == op) else {
        return Resolved::Need(op);
    };
    let start = isize_of(model.reader.memory().rows().len())
        - isize_of(usize::try_from(model.height).unwrap_or(0));
    let start = start.max(0);
    matches
        .iter()
        .find_map(|found| {
            let first = rows_of(&found.text, model.width).into_iter().next()?;
            (first.line_hash == mark.line_hash)
                .then(|| (start - isize::try_from(found.above).unwrap_or(0), first))
        })
        .map_or(Resolved::Gone, |(row, first)| {
            if row >= first_kept && !starts(row) {
                Resolved::Gone
            } else {
                Resolved::Found(row, Line::from(&first))
            }
        })
}

/// The search for command mark `sought`'s line, whose mark is on row `row`
/// above the memory: its first row's text, from a screen below the place
/// backward for a move up, or from a screen above it forward for a move
/// down, counted in rows above where the next read starts.
fn mark_search(
    model: &TerminalModel,
    row: isize,
    sought: &super::MarkLine,
    forward: bool,
) -> TextOp {
    let screen = isize_of(usize::try_from(model.height).unwrap_or(0)).max(1);
    let start = (isize_of(model.reader.memory().rows().len()) - screen).max(0);
    let boundary = if forward {
        row - screen
    } else {
        row + screen + 1
    };
    TextOp::TerminalFind {
        needle: sought.text.clone(),
        from: i32::try_from(start - boundary).unwrap_or(i32::MAX),
        backward: !forward,
        rows: sought.rows,
    }
}

/// The footer "Report status bar" reads (`docs/design/terminal-model.md`,
/// "Features and settings"), as the last read found the screen: the lines
/// below the caret's line; with none, the run of rows at the screen's
/// bottom that a program has rewritten, trailing blank rows skipped; with
/// neither, the last line with text. Each line is its rows joined, top to
/// bottom, its padding left out, and blank lines are left out.
pub(crate) fn footer(model: &TerminalModel) -> Vec<String> {
    let rows = model.reader.memory().rows();
    let len = rows.len();
    let height = usize::try_from(model.height).unwrap_or(usize::MAX);
    let top = len.saturating_sub(height).max(rows.first_kept());
    let blank = |row: usize| rows.get(row).is_none_or(Row::is_blank);
    let wraps = |row: usize| rows.get(row).is_some_and(|found| found.wraps);
    // The first row of the line holding `row`, on the screen.
    let start_of = |mut row: usize| {
        while row > top && wraps(row - 1) {
            row -= 1;
        }
        row
    };
    // The lines of rows `from` to `to`, joined, the blank ones left out.
    let lines = |from: usize, to: usize| {
        let mut lines = Vec::new();
        let mut line = String::new();
        for row in from..to {
            let Some(found) = rows.get(row) else {
                break;
            };
            line.push_str(found.text.as_str());
            if !found.wraps || row + 1 == to {
                let text = verbatim_text::trim_padding(&line);
                if !text.trim().is_empty() {
                    lines.push(text.to_owned());
                }
                line.clear();
            }
        }
        lines
    };
    if let Some(caret) = model.caret {
        let mut end = caret.row;
        while wraps(end) && end + 1 < len {
            end += 1;
        }
        let below = lines(end + 1, len);
        if !below.is_empty() {
            return below;
        }
    }
    let mut last = len;
    while last > top && blank(last - 1) {
        last -= 1;
    }
    let mut first = last;
    while first > top && rows.get(first - 1).is_some_and(|found| found.redrawn) {
        first -= 1;
    }
    if first < last {
        return lines(start_of(first), last);
    }
    if last > top {
        return lines(start_of(last - 1), last);
    }
    Vec::new()
}

fn isize_of(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}
