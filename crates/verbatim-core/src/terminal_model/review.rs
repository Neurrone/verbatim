//! The review cursor's text requests in a terminal Core reads
//! (`books/dev/src/explanation/design/terminal-model.md`, "The review cursor"), answered from
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
//! from the outpost by how far above the next read's start they lie, with
//! a screen more for output that scrolls meanwhile: the answer asks for
//! them ([`Need`]), and is worked out again once they come, placed among
//! the memory's rows only when its last read is the one they were counted
//! from ([`placed_history`]). A place there is the nearest row of its
//! line's hash from a screen above the place to a screen below it, or else
//! the place itself.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use verbatim_model::batch::{Batch, Chunks, Done, Missing, RangeText, StepOutcome, StepRefusal};
use verbatim_model::{
    MAX_RANGE_BYTES, MAX_READ_AHEAD, MAX_READ_AHEAD_TEXT, TerminalMatch, TextAnchor, TextChunk,
    TextMovement, TextPoint, TextUnit,
};
use verbatim_text::terminal_rows::{Anchor, Row, rows_of};

use super::{HistoryRead, TERMINAL_ANCHORS, TerminalModel};
use crate::terminal::reader::SearchAsk;
use crate::text_request::{LocalStep, Span};

/// What a history read asked for: rows from `above` rows above the start,
/// or the whole document when `whole`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HistoryAsked {
    pub(crate) above: u32,
    pub(crate) whole: bool,
}

/// How many places a terminal keeps beyond those whose anchors Core holds.
const KEPT_PLACES: usize = 64;

/// Rows of the history an answer needs that have not been read: from
/// `above` rows above the start of the memory's last read to the
/// document's end, or the whole document when `whole`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Need {
    pub(crate) above: u32,
    pub(crate) whole: bool,
}

/// The caret as the read before the request found it: its row of the
/// memory's last read and its column in characters.
pub(crate) use verbatim_text::terminal_rows::ReadCaret;

/// Whether Core answers a request naming `points` from `model`'s memory:
/// each is the caret, the start, the end, or a position at an anchor Core
/// minted for the terminal; a request naming another anchor, such as a
/// caret report's, goes to the outpost and is answered as any text's.
pub(crate) fn answers(model: &TerminalModel, points: &[TextPoint]) -> bool {
    points.iter().all(|point| match point {
        TextPoint::At(position) => place_of(model, position.anchor).is_some(),
        _ => true,
    })
}

/// Answers `batch` from `model`'s memory ([`answers`] says which requests
/// it answers), step by step ([`crate::text_request::answer_steps`]), with
/// `history`, the rows above the memory the command read, numbering new
/// anchors from `next_anchor` and keeping the places of the anchors in
/// `held`. A line or a screen's rows read at a time; a word by Core's own
/// words ([`crate::text_request::local_word`]); a range. A terminal's text
/// is not selected, nor its caret moved, by Verbatim, and a place's
/// position on the screen is not read: those steps are refused.
///
/// # Errors
///
/// [`Need`] when the answer needs rows of the history that have not been
/// read: the answer is worked out again, whole, once they come.
pub(crate) fn answer(
    model: &mut TerminalModel,
    (batch, history): (&Batch, Option<&HistoryRead>),
    next_anchor: &mut u64,
    held: &BTreeSet<TextAnchor>,
) -> Result<Vec<StepOutcome>, Need> {
    crate::text_request::answer_steps(batch, |step| {
        answer_step(model, (step, history), next_anchor, held)
    })
}

/// The caret's line in `model`'s memory as a review command reading it
/// there answers it, its anchor minted from `next_anchor` and its place
/// kept beside those of the anchors in `held`: where the review cursor
/// goes when it follows the caret (rule 24, "The review anchor").
/// `None` when the memory's last read showed no caret.
pub(crate) fn at_caret(
    model: &mut TerminalModel,
    next_anchor: &mut u64,
    held: &BTreeSet<TextAnchor>,
) -> Option<TextChunk> {
    let step = LocalStep::Read {
        at: TextPoint::Caret,
        movement: None,
        unit: TextUnit::Line,
        count: 1,
    };
    match answer_step(model, (step, None), next_anchor, held) {
        Ok(StepOutcome::Done(Done::ReadText(Chunks { mut chunks, .. }))) if chunks.len() == 1 => {
            chunks.pop()
        }
        _ => None,
    }
}

/// One step of [`answer`].
fn answer_step(
    model: &mut TerminalModel,
    (step, history): (LocalStep, Option<&HistoryRead>),
    next_anchor: &mut u64,
    held: &BTreeSet<TextAnchor>,
) -> Result<StepOutcome, Need> {
    let caret = model.caret();
    let plan = {
        let mut rows = Rows {
            model,
            above: history,
        };
        match step {
            LocalStep::Read {
                at,
                movement,
                unit: TextUnit::Word,
                count: 1,
            } => read_word(&mut rows, caret, (at, movement))?,
            LocalStep::Read {
                at,
                movement,
                unit,
                count: 1,
            } => read_unit(&mut rows, caret, (at, movement, unit))?,
            LocalStep::Read {
                at,
                movement,
                unit,
                count,
            } => read_ahead(&mut rows, caret, (at, movement, unit), count)?,
            LocalStep::Range(span) => read_range(&mut rows, caret, span)?,
            LocalStep::Other => Plan::Outcome(StepOutcome::Refused(StepRefusal::Unsupported)),
        }
    };
    let mut mint = || {
        *next_anchor = next_anchor.saturating_add(1);
        TERMINAL_ANCHORS | *next_anchor
    };
    Ok(finish(model, plan, &mut mint, held))
}

/// A row to send as a chunk.
#[derive(Debug)]
struct Chunked {
    row: isize,
    line: Row,
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
    /// A word read: how far it moved, the row it is on, its byte range in
    /// the row's text, and the point's offset in it.
    Word {
        moved: i32,
        row: isize,
        line: Row,
        word: std::ops::Range<usize>,
        offset: usize,
    },
    /// Any other answer.
    Outcome(StepOutcome),
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
fn keep_place(model: &mut TerminalModel, number: u64, anchor: Anchor, held: &BTreeSet<TextAnchor>) {
    let place = model.rules().memory_mut().add_place(anchor);
    model.places.push_back((number, place));
    while model.places.len() > KEPT_PLACES {
        let Some(at) = model
            .places
            .iter()
            .position(|(kept, _)| !held.contains(&TextAnchor(*kept)))
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
    held: &BTreeSet<TextAnchor>,
) -> StepOutcome {
    let mut chunk_of = |model: &mut TerminalModel, chunked: Chunked| {
        let number = mint();
        keep_place(
            model,
            number,
            Anchor {
                row: chunked.row,
                column: 0,
                line_hash: chunked.line.line_hash,
            },
            held,
        );
        TextChunk {
            unit: TextUnit::Line,
            start: TextAnchor(number),
            offset: u32::try_from(chunked.offset).unwrap_or(u32::MAX),
            text: chunked.line.text.to_string(),
            languages: Vec::new(),
            first: chunked.first,
            last: chunked.last,
            truncated: false,
            formats: Vec::new(),
        }
    };
    match plan {
        Plan::Read(moved, chunked) => {
            let chunk = chunk_of(model, chunked);
            StepOutcome::Done(Done::ReadText(Chunks {
                padded: None,
                moved,
                chunks: vec![chunk],
            }))
        }
        Plan::Ahead(moved, rows) => {
            let chunks = rows
                .into_iter()
                .map(|chunked| chunk_of(model, chunked))
                .collect();
            StepOutcome::Done(Done::ReadText(Chunks {
                padded: None,
                moved,
                chunks,
            }))
        }
        Plan::Word {
            moved,
            row,
            line,
            word,
            offset,
        } => {
            // The word's anchor is a place at its first character, so a
            // read from the word's start begins there.
            let number = mint();
            let column = verbatim_text::proven::text_in(&line.text, ..word.start)
                .chars()
                .count();
            keep_place(
                model,
                number,
                Anchor {
                    row,
                    column,
                    line_hash: line.line_hash,
                },
                held,
            );
            let text = verbatim_text::proven::text_in(&line.text, word).to_owned();
            StepOutcome::Done(Done::ReadText(Chunks {
                padded: None,
                moved,
                chunks: vec![TextChunk {
                    unit: TextUnit::Word,
                    start: TextAnchor(number),
                    offset: u32::try_from(offset).unwrap_or(u32::MAX),
                    text,
                    languages: Vec::new(),
                    first: false,
                    last: false,
                    truncated: false,
                    formats: Vec::new(),
                }],
            }))
        }
        Plan::Outcome(outcome) => outcome,
    }
}

/// The rows a request reads: the memory's, and the history's as it came,
/// each by its row of the memory's last read.
struct Rows<'a> {
    model: &'a TerminalModel,
    /// The history read last ([`super::TerminalModel::history`]): its
    /// first row, its rows, and whether it reached the document's start.
    above: Option<&'a HistoryRead>,
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
        isize_of(usize::try_from(self.model.height()).unwrap_or(0))
    }

    /// The row the next read starts at: the screen's top.
    fn start_row(&self) -> isize {
        self.len().saturating_sub(self.height()).max(0)
    }

    /// The last row.
    fn last(&self) -> isize {
        self.len().saturating_sub(1)
    }

    /// The document's first row, when it is known: the memory's last read
    /// started there, or the history's read reached it.
    fn known_first(&self) -> Option<isize> {
        if self.model.at_document_start() {
            return Some(0);
        }
        match &self.above {
            Some(HistoryRead {
                first,
                reached: true,
                ..
            }) => Some(*first),
            _ => None,
        }
    }

    /// The history from row `row` to the document's end, or the whole
    /// document when `row` is `None`, which the history read last does not
    /// hold.
    fn need(&self, row: Option<isize>) -> Need {
        let wanted = row.map_or(0, |row| {
            usize::try_from(self.start_row().saturating_sub(row)).unwrap_or(0)
        });
        Need {
            above: u32::try_from(wanted).unwrap_or(u32::MAX),
            whole: row.is_none(),
        }
    }

    /// The document's first row, the whole document read when it is not
    /// known.
    fn first(&mut self) -> Result<isize, Need> {
        self.known_first().ok_or_else(|| self.need(None))
    }

    /// Row `row`, read from the history when the memory does not keep it;
    /// `None` when there is no such row.
    fn row(&mut self, row: isize) -> Result<Option<Row>, Need> {
        if row > self.last() {
            return Ok(None);
        }
        if row >= self.first_kept() {
            let index = usize::try_from(row).unwrap_or(0);
            return Ok(self.model.reader.memory().rows().get(index).cloned());
        }
        if self.known_first().is_some_and(|first| row < first) {
            return Ok(None);
        }
        let covered = matches!(self.above, Some(read) if read.first <= row);
        if !covered {
            return Err(self.need(Some(row)));
        }
        Ok(self.above.and_then(|read| {
            usize::try_from(row.saturating_sub(read.first))
                .ok()
                .and_then(|at| read.rows.get(at))
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
            .map_or(row.saturating_sub(screen), |first| {
                row.saturating_sub(screen).max(first)
            });
        let bottom = row.saturating_add(screen).min(self.last());
        // One read from a screen above the place to the document's end
        // covers the search.
        self.row(top)?;
        for distance in 0..=screen {
            for candidate in [row.saturating_sub(distance), row.saturating_add(distance)] {
                if (top..=bottom).contains(&candidate)
                    && self
                        .row(candidate)?
                        .is_some_and(|line| line.line_hash == line_hash)
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
        let offset = verbatim_text::floor_boundary(&line.text, offset);
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

/// The outcome of a read at `at` whose point is not there: the anchor it
/// names lost; a point with no anchor, whose row is always there, failed.
fn lost(at: TextPoint) -> StepOutcome {
    match at {
        TextPoint::At(position) => StepOutcome::NotThere(Missing::Anchor(position.anchor)),
        _ => StepOutcome::Failed,
    }
}

/// Where `point` is, or the outcome that ends the step: a place gone with
/// its row is an anchor lost. A position counts from its anchor's place,
/// a row's start or a word's first character.
fn resolve(
    rows: &mut Rows<'_>,
    caret: Option<ReadCaret>,
    point: TextPoint,
) -> Result<Result<Point, StepOutcome>, Need> {
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
                return Ok(Err(lost(point)));
            };
            let Some(row) = rows.found(place.row, place.line_hash)? else {
                return Ok(Err(lost(point)));
            };
            let column = rows
                .row(row)?
                .map_or(0, |line| byte_at(&line.text, place.column));
            Point {
                row,
                offset: column.saturating_add(crate::text::byte_offset(position.offset)),
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
) -> Result<Result<(Point, i32), StepOutcome>, Need> {
    let Some(movement) = movement else {
        return Ok(Ok((point, 0)));
    };
    let per = match movement.unit {
        TextUnit::Line => 1,
        TextUnit::Page => rows.height().max(1),
        _ => return Ok(Err(StepOutcome::Refused(StepRefusal::NoSuchUnit))),
    };
    let wanted = point.row.saturating_add(
        isize::try_from(movement.count)
            .unwrap_or(0)
            .saturating_mul(per),
    );
    let target = if wanted > point.row {
        wanted.min(rows.last())
    } else if wanted < point.row && wanted < rows.first_kept() {
        // Rows above the memory's are read until the document's start.
        let _ = rows.row(wanted)?;
        rows.known_first().map_or(wanted, |first| wanted.max(first))
    } else {
        wanted
    };
    let rows_moved = target.saturating_sub(point.row);
    let units = if per == 1 {
        rows_moved
    } else {
        // Pages moved, counting a part of one at an end.
        if rows_moved == 0 {
            0
        } else {
            let pages = rows_moved.unsigned_abs().div_ceil(per.unsigned_abs());
            isize::try_from(pages)
                .unwrap_or(isize::MAX)
                .saturating_mul(rows_moved.signum())
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

/// A read of one unit, a row.
fn read_unit(
    rows: &mut Rows<'_>,
    caret: Option<ReadCaret>,
    (at, movement, unit): (TextPoint, Option<TextMovement>, TextUnit),
) -> Result<Plan, Need> {
    if unit != TextUnit::Line {
        return Ok(Plan::Outcome(StepOutcome::Refused(StepRefusal::NoSuchUnit)));
    }
    let point = match resolve(rows, caret, at)? {
        Ok(point) => point,
        Err(outcome) => return Ok(Plan::Outcome(outcome)),
    };
    let (point, count) = match moved(rows, point, movement)? {
        Ok(moved) => moved,
        Err(outcome) => return Ok(Plan::Outcome(outcome)),
    };
    Ok(match rows.chunked(point.row, point.offset)? {
        Some(chunked) => Plan::Read(count, chunked),
        None => Plan::Outcome(lost(at)),
    })
}

/// Rows read ahead, for say-all.
fn read_ahead(
    rows: &mut Rows<'_>,
    caret: Option<ReadCaret>,
    (at, movement, unit): (TextPoint, Option<TextMovement>, TextUnit),
    count: u8,
) -> Result<Plan, Need> {
    if unit != TextUnit::Line {
        return Ok(Plan::Outcome(StepOutcome::Refused(StepRefusal::NoSuchUnit)));
    }
    let point = match resolve(rows, caret, at)? {
        Ok(point) => point,
        Err(outcome) => return Ok(Plan::Outcome(outcome)),
    };
    let (point, moved_by) = match moved(rows, point, movement)? {
        Ok(moved) => moved,
        Err(outcome) => return Ok(Plan::Outcome(outcome)),
    };
    let wanted = usize::from(count.clamp(1, MAX_READ_AHEAD));
    let mut chunks: Vec<Chunked> = Vec::new();
    let mut bytes: usize = 0;
    let mut row = point.row;
    while chunks.len() < wanted && bytes < MAX_READ_AHEAD_TEXT {
        let offset = if chunks.is_empty() { point.offset } else { 0 };
        let Some(chunked) = rows.chunked(row, offset)? else {
            break;
        };
        bytes = bytes.saturating_add(chunked.line.text.len());
        let last = chunked.last;
        chunks.push(chunked);
        if last {
            break;
        }
        row = row.saturating_add(1);
    }
    if chunks.is_empty() {
        return Ok(Plan::Outcome(lost(at)));
    }
    Ok(Plan::Ahead(moved_by, chunks))
}

/// A word, by Core's own words over each row's text, as review read it
/// before words were the provider's ([`crate::text_request::local_word`]):
/// the word at `at`, moved by `movement`, a move by word.
fn read_word(
    rows: &mut Rows<'_>,
    caret: Option<ReadCaret>,
    (at, movement): (TextPoint, Option<TextMovement>),
) -> Result<Plan, Need> {
    let count = match movement {
        None => 0,
        Some(TextMovement {
            unit: TextUnit::Word,
            count,
        }) => count,
        Some(_) => return Ok(Plan::Outcome(StepOutcome::Refused(StepRefusal::NoSuchUnit))),
    };
    let point = match resolve(rows, caret, at)? {
        Ok(point) => point,
        Err(outcome) => return Ok(Plan::Outcome(outcome)),
    };
    let found = crate::text_request::local_word(
        |row| Ok(rows.row(row)?.map(|line| line.text.to_string())),
        (point.row, point.offset),
        count,
    )?;
    let Some((moved, row, word)) = found else {
        return Ok(Plan::Outcome(lost(at)));
    };
    let Some(line) = rows.row(row)? else {
        return Ok(Plan::Outcome(lost(at)));
    };
    let offset = if moved == 0 && word.contains(&point.offset) {
        point.offset.saturating_sub(word.start)
    } else {
        0
    };
    Ok(Plan::Word {
        moved,
        row,
        line,
        word,
        offset,
    })
}

/// The text between two points, in document order whichever comes
/// first; with the points past their characters, through the character at
/// the later end. Rows of one line are joined as they are, and lines with
/// a line break between them.
fn read_range(
    rows: &mut Rows<'_>,
    caret: Option<ReadCaret>,
    Span {
        start,
        end,
        through,
    }: Span,
) -> Result<Plan, Need> {
    let a = match resolve(rows, caret, start)? {
        Ok(point) => point,
        Err(outcome) => return Ok(Plan::Outcome(outcome)),
    };
    let b = match resolve(rows, caret, end)? {
        Ok(point) => point,
        Err(outcome) => return Ok(Plan::Outcome(outcome)),
    };
    let (from, mut to) = if (a.row, a.offset) <= (b.row, b.offset) {
        (a, b)
    } else {
        (b, a)
    };
    if through.is_some()
        && let Some(line) = rows.row(to.row)?
    {
        let at = verbatim_text::floor_boundary(&line.text, to.offset);
        to.offset = verbatim_text::grapheme_at(&line.text, at).map_or(at, |range| range.end);
    }
    let mut text = String::new();
    let mut truncated = false;
    for row in from.row..=to.row {
        let Some(line) = rows.row(row)? else {
            break;
        };
        let begin = if row == from.row {
            verbatim_text::floor_boundary(&line.text, from.offset)
        } else {
            0
        };
        let end = if row == to.row {
            verbatim_text::floor_boundary(&line.text, to.offset).max(begin)
        } else {
            line.text.len()
        };
        text.push_str(verbatim_text::proven::text_in(&line.text, begin..end));
        if row < to.row && !line.wraps {
            text.push_str("\r\n");
        }
        if text.len() > MAX_RANGE_BYTES {
            let cut = verbatim_text::floor_boundary(&text, MAX_RANGE_BYTES);
            text.truncate(cut);
            truncated = true;
            break;
        }
    }
    Ok(Plan::Outcome(StepOutcome::Done(Done::ReadRange(
        RangeText {
            text,
            truncated,
            padded: None,
        },
    ))))
}

/// How many searches one move between command marks makes at most: a mark
/// whose line is gone costs one search before it is dropped, and a press
/// that reaches this bound without finding a mark says nothing, the marks
/// not yet tried kept for the next press. What a search costs, and so how
/// long a press waits for its searches, is in
/// `books/dev/src/reference/performance.md`, "A terminal's command marks".
pub(crate) const MARK_SEARCHES: u32 = 3;

/// The search a command mark's move needs for a mark above the rows Core
/// keeps: the request, and the hash of the mark's line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Search {
    pub(crate) ask: SearchAsk,
    pub(crate) hash: u64,
}

/// A search's answer, its rows placed among the memory's when it came
/// ([`placed`]): the hash of the line sought, and the rows of its matches
/// whose lines have that hash, each with its first row; `None` when the
/// rows could not be placed, the row of the start the search counted from
/// having left the memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Placed {
    pub(crate) hash: u64,
    pub(crate) rows: Option<Vec<(isize, Row)>>,
}

/// What became of one command mark a move looked at.
enum Resolved {
    /// Its line is on this row, whose first row's text is this.
    Found(isize, Row),
    /// Its line is nowhere: the mark is dropped.
    Gone,
}

/// Moves the review cursor to the nearest command mark strictly above
/// (`forward` false) or below the review position at `from`, or the caret
/// (`books/dev/src/explanation/design/terminal-model.md`, "Features and settings"), after the
/// fresh read: each mark, nearest first, is found by its line's text,
/// within the memory near its place, and above the memory by the search
/// the outpost runs ([`SearchAsk`]), each way from its place; a
/// mark not found is dropped, and the next one tried, with nothing said.
/// The answer is a line move's (a line read at the start of the mark's
/// row), the review position going there; `None` when no mark is
/// left that way, when the move has made [`MARK_SEARCHES`] searches
/// (`searches` counts them), or when a search's rows could not be placed,
/// the marks not found kept. `found` is the search that came for this
/// move, taken when its mark is tried.
///
/// # Errors
///
/// The search a mark above the memory needs, when it has not been
/// answered: the marks dropped so far stay dropped, and the move is worked
/// out again once the answer comes.
pub(crate) fn mark(
    model: &mut TerminalModel,
    (forward, from): (bool, &TextPoint),
    (found, searches): (&mut Option<Placed>, &mut u32),
    next_anchor: &mut u64,
    held: &BTreeSet<TextAnchor>,
) -> Result<Option<StepOutcome>, Search> {
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
        .or_else(|| model.caret().map(|caret| isize_of(caret.row)))
        .unwrap_or(len.saturating_sub(1));
    // Up goes above the review position's line, not its own first row.
    let mut review_start = review;
    while review_start > first_kept && wraps_at(model, review_start.saturating_sub(1)) {
        review_start = review_start.saturating_sub(1);
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
        let resolved = match within_memory(model, candidate) {
            Ok(resolved) => resolved,
            // No start to count a search from, as before the history block
            // the first read asked for: the move ends, the mark kept.
            Err(None) => return Ok(None),
            Err(Some(ask)) => match found.take() {
                Some(found) if found.hash == candidate.line_hash => match found.rows {
                    Some(rows) => from_search(&rows, candidate),
                    // Counted from a start whose row left the memory: the
                    // move ends, saying nothing, the mark kept.
                    None => return Ok(None),
                },
                _ if *searches >= MARK_SEARCHES => return Ok(None),
                _ => {
                    *searches = searches.saturating_add(1);
                    return Err(Search {
                        ask,
                        hash: candidate.line_hash,
                    });
                }
            },
        };
        match resolved {
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
                    first: model.at_document_start() && row == 0,
                    last: row == len.saturating_sub(1),
                    line,
                    offset: 0,
                };
                let moved = if forward { 1 } else { -1 };
                let mut mint = || {
                    *next_anchor = next_anchor.saturating_add(1);
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

/// Whether a line with hash `hash` starts at row `row` of the memory.
fn starts_at(model: &TerminalModel, row: isize, hash: u64) -> bool {
    let rows = model.reader.memory().rows();
    let first_kept = isize_of(rows.first_kept());
    usize::try_from(row)
        .ok()
        .and_then(|at| rows.get(at))
        .is_some_and(|found| found.line_hash == hash)
        && (row == first_kept || !wraps_at(model, row.saturating_sub(1)))
}

/// Where command mark `mark`'s line is now, within the memory: the line
/// starting at its row with its hash, or else the nearest line start with
/// it, the higher at one distance; gone when the memory holds the whole
/// document or its line's text is not known. Otherwise the search for it
/// above the memory ([`mark_search`]), `None` when the model names no
/// start to count it from.
fn within_memory(model: &TerminalModel, mark: Anchor) -> Result<Resolved, Option<SearchAsk>> {
    let rows = model.reader.memory().rows();
    let first_kept = isize_of(rows.first_kept());
    let len = isize_of(rows.len());
    if mark.row >= first_kept {
        let nearest = (0..len)
            .flat_map(|distance| {
                [
                    mark.row.saturating_sub(distance),
                    mark.row.saturating_add(distance),
                ]
            })
            .find(|&row| row >= first_kept && row < len && starts_at(model, row, mark.line_hash));
        let line = |row: isize| {
            usize::try_from(row)
                .ok()
                .and_then(|at| rows.get(at))
                .cloned()
        };
        if let Some((row, line)) = nearest.and_then(|row| Some((row, line(row)?))) {
            return Ok(Resolved::Found(row, line));
        }
        if first_kept == 0 && model.at_document_start() {
            return Ok(Resolved::Gone);
        }
    } else if model.at_document_start() && mark.row < 0 {
        return Ok(Resolved::Gone);
    }
    let Some(sought) = model
        .mark_lines
        .iter()
        .find(|line| line.hash == mark.line_hash)
    else {
        return Ok(Resolved::Gone);
    };
    Err(mark_search(model, mark.row, sought))
}

/// Where command mark `mark`'s line is among the rows a search for it
/// found, placed when it came ([`placed`]): the row nearest the mark's
/// place, the higher at one distance; gone when there is none.
fn from_search(rows: &[(isize, Row)], mark: Anchor) -> Resolved {
    rows.iter()
        .min_by_key(|(row, _)| (row.abs_diff(mark.row), *row))
        .map_or(Resolved::Gone, |(row, first)| {
            Resolved::Found(*row, first.clone())
        })
}

/// The search for command mark `sought`'s line, whose mark is on row `row`
/// above the memory: its first row's text, each way from the mark's row,
/// counted in rows above the start the model names, which the search
/// names too.
fn mark_search(model: &TerminalModel, row: isize, sought: &super::MarkLine) -> Option<SearchAsk> {
    Some(SearchAsk::new(
        model.start?,
        sought.text.clone(),
        i32::try_from(start_row(model).saturating_sub(row)).unwrap_or(i32::MAX),
        sought.rows,
    ))
}

/// The row of the memory's last read where the start it names lies: the
/// screen's top.
pub(crate) fn start_row(model: &TerminalModel) -> isize {
    let screen = isize_of(usize::try_from(model.height()).unwrap_or(0));
    isize_of(model.reader.memory().rows().len())
        .saturating_sub(screen)
        .max(0)
}

/// The history read `asked` for, its padded `text` holding `rows_above`
/// rows above its start, placed among the memory's rows as they are when it
/// comes, its start on row `start`, where the anchor Core put at that
/// start's row is now (`docs/design/readers-tell-core-asks-build3.md`,
/// Table 5). A whole document's rows end where the memory's do, at the
/// document's end.
pub(crate) fn placed_history(
    model: &TerminalModel,
    asked: HistoryAsked,
    text: &str,
    (rows_above, start): (u32, isize),
) -> HistoryRead {
    let rows = rows_of(text, model.width());
    if asked.whole {
        let len = isize_of(model.reader.memory().rows().len());
        return HistoryRead {
            first: len.saturating_sub(isize_of(rows.len())),
            rows,
            reached: true,
        };
    }
    HistoryRead {
        first: start.saturating_sub(isize::try_from(rows_above).unwrap_or(isize::MAX)),
        rows,
        reached: rows_above < asked.above,
    }
}

/// The answer to the search for the line with hash `hash`, its `matches`
/// counted from the start on row `start`, where the anchor Core put at the
/// start's row is now (Table 5), placed among the memory's rows as they are
/// when it comes: each match whose line has the hash, on its row of the
/// memory's last read, a match within the memory only where a line starts;
/// `None` for `start` when the anchor's row left the memory.
pub(crate) fn placed(
    model: &TerminalModel,
    hash: u64,
    matches: &[TerminalMatch],
    start: Option<isize>,
) -> Placed {
    let Some(start) = start else {
        return Placed { hash, rows: None };
    };
    let first_kept = isize_of(model.reader.memory().rows().first_kept());
    let rows = matches
        .iter()
        .filter_map(|found| {
            let first = rows_of(&found.text, model.width()).into_iter().next()?;
            let row = start.saturating_sub(isize::try_from(found.above).unwrap_or(0));
            (first.line_hash == hash && (row < first_kept || starts_at(model, row, hash)))
                .then_some((row, first))
        })
        .collect();
    Placed {
        hash,
        rows: Some(rows),
    }
}

/// The footer "Report status bar" reads (`books/dev/src/explanation/design/terminal-model.md`,
/// "Features and settings"), as the last read found the screen: the lines
/// below the caret's line; with none, the run of rows at the screen's
/// bottom that a program has rewritten, trailing blank rows skipped; with
/// neither, the last line with text. Each line is its rows joined, top to
/// bottom, its padding left out, and blank lines are left out.
pub(crate) fn footer(model: &TerminalModel) -> Vec<String> {
    let rows = model.reader.memory().rows();
    let len = rows.len();
    let height = usize::try_from(model.height()).unwrap_or(usize::MAX);
    let top = len.saturating_sub(height).max(rows.first_kept());
    let blank = |row: usize| rows.get(row).is_none_or(Row::is_blank);
    let wraps = |row: usize| rows.get(row).is_some_and(|found| found.wraps);
    // The first row of the line holding `row`, on the screen.
    let start_of = |mut row: usize| {
        while row > top && wraps(row.saturating_sub(1)) {
            row = row.saturating_sub(1);
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
            if !found.wraps || row.saturating_add(1) == to {
                let text = verbatim_text::trim_padding(&line);
                if !text.trim().is_empty() {
                    lines.push(text.to_owned());
                }
                line.clear();
            }
        }
        lines
    };
    if let Some(caret) = model.caret() {
        let mut end = caret.row;
        while wraps(end) && end.saturating_add(1) < len {
            end = end.saturating_add(1);
        }
        let below = lines(end.saturating_add(1), len);
        if !below.is_empty() {
            return below;
        }
    }
    let mut last = len;
    while last > top && blank(last.saturating_sub(1)) {
        last = last.saturating_sub(1);
    }
    let mut first = last;
    while first > top
        && rows
            .get(first.saturating_sub(1))
            .is_some_and(|found| found.redrawn)
    {
        first = first.saturating_sub(1);
    }
    if first < last {
        return lines(start_of(first), last);
    }
    if last > top {
        return lines(start_of(last.saturating_sub(1)), last);
    }
    Vec::new()
}

fn isize_of(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}
