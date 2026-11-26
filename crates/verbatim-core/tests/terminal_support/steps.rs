//! The simulated terminal's second answerer: each generic text step of a
//! batch (`Visible`, `Move`, `ReadText` with `whole` and `trim`,
//! `ReadRange`, `Caret`, `FindText` and `ReadColours`) answered as the
//! terminal's UI Automation provider answers the step's calls, and the
//! terminal's old operations composed of those steps ([`add_read`] with
//! [`read_of`], [`add_search`] with [`matches_of`]).
//!
//! Core reads a terminal by the old operations, which the first answerer
//! (`Outpost::read`, `history` and `find`) answers, so no request of
//! today's Core reaches this one: `terminal_answerers.rs` drives it
//! directly and compares what its steps compose with the old operations'
//! answers field by field, the comparison Core's reader built from generic
//! steps is proven with (Dickson, 2026-11-26,
//! `docs/simplification-review/builds.md`).
//!
//! The composed read's steps, in order:
//!
//! - `Visible`: the screen's text and whether it starts at the text's
//!   start;
//! - when colours are asked, the line at the screen's top, trimmed, whose
//!   padded length is the screen's width;
//! - a move from the text's end to it, which holds the end: every later
//!   step names this held end, as the old program takes the document range
//!   once;
//! - the next read's start: the held end moved up by the screen's height
//!   as last found, or the screen's top before it is known;
//! - a move from the next start to the end, and one from the held end to
//!   the text's start: the rows each lies from the end, by which the
//!   start the read named is compared with the next start, and a start is
//!   known to be the text's;
//! - the fallback read, from the next start, or the text's start for a
//!   whole read, moved up by the rows asked above it, to the held end, in
//!   one trimmed `ReadRange`;
//! - when the read names a start: a move from it to the end, refused when
//!   it lies outside the text, and, when it was not, the same read from
//!   it;
//! - the first line, the caret, its line, the caret's rows to the end;
//! - when asked, the colours.
//!
//! The read from the named start and the fallback read are both taken
//! because a step's condition cannot compare two text positions: the
//! composition picks one by the rows each start lies from the end. That
//! is one screen-sized read more than the old program makes whenever the
//! named start is used.

#![expect(
    clippy::result_large_err,
    reason = "a step's outcome is what ends it, as the outpost answers it"
)]

use verbatim_core::batch::{Answers, Builder, Got, Ref, Search, kind};
use verbatim_model::batch::{
    Batch, CaretAt, Chunks, Done, FoundText, Gate, Missing, Moved, Op, Point, PointOf, RangeText,
    StepOutcome, StepRefusal, VisibleText,
};
use verbatim_model::{
    NodeId, TerminalColourRun, TerminalMatch, TerminalSnapshot, TerminalText, TextAnchor,
    TextChunk, TextDetails, TextMovement, TextPoint, TextPosition, TextUnit,
};
use verbatim_term_sim::{Position, Query, Range};

use super::Outpost;

/// More rows than any terminal holds.
const FAR: i32 = 1_000_000;

/// A step's result, or the outcome that ends the step.
type Found<T> = Result<T, StepOutcome>;

impl Outpost {
    /// Holds `at` for the generic steps, minting its anchor.
    pub fn hold(&mut self, at: Position) -> TextAnchor {
        self.held.push(at);
        TextAnchor(u64::try_from(self.held.len()).expect("anchors"))
    }

    /// The position the generic steps hold under `anchor`.
    pub fn held(&self, anchor: TextAnchor) -> Option<Position> {
        usize::try_from(anchor.0)
            .ok()
            .and_then(|number| number.checked_sub(1))
            .and_then(|index| self.held.get(index).copied())
    }

    /// The position anchor `anchor` holds, or the step's outcome without
    /// it.
    fn held_or_missing(&self, anchor: TextAnchor) -> Found<Position> {
        self.held(anchor)
            .ok_or(StepOutcome::NotThere(Missing::Anchor(anchor)))
    }

    /// The text of `range`, as `GetText` gives it; a refused call fails
    /// the step.
    fn text(&self, range: Range) -> Found<String> {
        self.sim.text(range).map_err(|_| StepOutcome::Failed)
    }

    /// `at` moved on by the characters of `offset` bytes of the text from
    /// it, as the outpost finds a position by its offset.
    fn advanced(&self, at: Position, offset: u32) -> Found<Position> {
        if offset == 0 {
            return Ok(at);
        }
        let row = self.sim.row_of(at).map_err(|_| StepOutcome::Failed)?;
        let text = self.text(Range {
            start: at,
            end: row.end,
        })?;
        let offset = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(text.len());
        let columns = text
            .get(..offset)
            .map_or(0, |before| before.chars().count());
        Ok(Position {
            row: at.row,
            column: at.column + columns,
        })
    }

    /// The position `point` names, the outcomes `earlier` answering a point
    /// of an earlier step's result.
    fn point(&self, point: &Point, earlier: &[StepOutcome]) -> Found<Position> {
        match point {
            Point::At(TextPoint::Start) => Ok(self.sim.document_range().start),
            Point::At(TextPoint::End) => Ok(self.sim.document_range().end),
            Point::At(TextPoint::Caret | TextPoint::SelectionStart | TextPoint::SelectionEnd) => {
                Ok(self.sim.caret().start)
            }
            Point::At(TextPoint::At(position)) => {
                self.advanced(self.held_or_missing(position.anchor)?, position.offset)
            }
            Point::Of(place, which) => {
                let anchor = match (earlier.get(place.index()), which) {
                    (Some(StepOutcome::Done(Done::Visible(visible))), PointOf::VisibleStart) => {
                        visible.start
                    }
                    (Some(StepOutcome::Done(Done::Visible(visible))), PointOf::VisibleEnd) => {
                        visible.end
                    }
                    (Some(StepOutcome::Done(Done::Move(moved))), PointOf::Moved) => moved.to,
                    (
                        Some(StepOutcome::Done(Done::Caret(caret))),
                        PointOf::Caret | PointOf::SelectionStart | PointOf::SelectionEnd,
                    ) => caret.caret,
                    (Some(StepOutcome::Done(Done::ReadText(read))), PointOf::ChunkStart) => read
                        .chunks
                        .first()
                        .map(|chunk| chunk.start)
                        .ok_or(StepOutcome::Failed)?,
                    // A point of a step that did not run: the step naming
                    // it does not run either.
                    (Some(_), _) => return Err(StepOutcome::Skipped),
                    (None, _) => panic!("a point of a step not yet run: {point:?}"),
                };
                self.held_or_missing(anchor)
            }
        }
    }

    /// `text` trimmed, with its padded lengths, when `trim`.
    fn trimmed(text: String, trim: bool) -> (String, Option<Vec<(u32, u32)>>) {
        if trim {
            let unpadded = TerminalText::unpadded(&text);
            (unpadded.text, Some(unpadded.lengths))
        } else {
            (text, None)
        }
    }

    /// A chunk of a line, `text`, held at `start`, the point `offset` bytes
    /// into it.
    fn chunk(&mut self, start: Position, text: String, offset: usize) -> TextChunk {
        let last = self.sim.document_range().end.row;
        TextChunk {
            unit: TextUnit::Line,
            start: self.hold(start),
            offset: u32::try_from(offset).unwrap_or(u32::MAX),
            first: start.row == 0,
            last: start.row == last,
            text,
            languages: Vec::new(),
            truncated: false,
            formats: Vec::new(),
        }
    }

    /// `count` lines from `at`, after a move of `movement` lines from the
    /// start of `at`'s line, as the outpost's classic read takes them.
    fn read_lines(
        &mut self,
        at: Position,
        movement: Option<i32>,
        (count, trim): (u16, bool),
    ) -> Found<Done> {
        let failed = |_| StepOutcome::Failed;
        let (point, moved, on_start) = match movement {
            None => (at, 0, false),
            Some(by) => {
                let start = self.sim.row_of(at).map_err(failed)?.start;
                let (landed, moved) = self
                    .sim
                    .move_rows(start, isize::try_from(by).unwrap_or(0))
                    .map_err(failed)?;
                (landed, i32::try_from(moved).unwrap_or(0), true)
            }
        };
        let mut chunks = Vec::new();
        let mut padded;
        let mut row = self.sim.row_of(point).map_err(failed)?;
        loop {
            let text = self.text(row)?;
            let offset = if chunks.is_empty() && !on_start {
                let before = self.text(Range {
                    start: row.start,
                    end: point,
                })?;
                before.len().min(text.len())
            } else {
                0
            };
            let (text, lengths) = Self::trimmed(text, trim);
            padded = lengths;
            let offset = offset.min(text.len());
            chunks.push(self.chunk(row.start, text, offset));
            if chunks.len() >= usize::from(count) {
                break;
            }
            let (next, went) = self.sim.move_rows(row.start, 1).map_err(failed)?;
            let next = self.sim.row_of(next).map_err(failed)?;
            if went == 0 || next.start.row <= row.start.row {
                break;
            }
            row = next;
        }
        Ok(Done::ReadText(Chunks {
            moved,
            chunks,
            padded,
        }))
    }

    /// The matches of `needle` from `at` to the text's end, or back to its
    /// start, the nearest first, as the outpost's classic search finds
    /// them: each occurrence one call of at most `most`, a match not at
    /// its row's start passed over when `starts`, at most `limit` kept,
    /// each read for `read` rows.
    fn search(
        &self,
        at: Position,
        (needle, backward): (&str, bool),
        (starts, read): (bool, u8),
        (limit, most): (u8, u8),
    ) -> Found<Vec<FoundText>> {
        let failed = |_| StepOutcome::Failed;
        let last = self.sim.document_range().end.row;
        let wanted: Vec<char> = needle.chars().collect();
        let mut hits = Vec::new();
        for row in 0..=last {
            let start = Position { row, column: 0 };
            let text = self.text(self.sim.row_of(start).map_err(failed)?)?;
            let cells: Vec<char> = text.trim_end_matches("\r\n").chars().collect();
            if wanted.is_empty() || wanted.len() > cells.len() {
                continue;
            }
            for column in 0..=cells.len() - wanted.len() {
                if cells[column..column + wanted.len()] == wanted[..] {
                    hits.push((
                        Position { row, column },
                        Position {
                            row,
                            column: column + wanted.len(),
                        },
                    ));
                }
            }
        }
        let ordered: Vec<(Position, Position)> = if backward {
            hits.into_iter()
                .rev()
                .filter(|(_, end)| *end <= at)
                .collect()
        } else {
            hits.into_iter().filter(|(start, _)| *start >= at).collect()
        };
        let mut found = Vec::new();
        for (start, _) in ordered.into_iter().take(usize::from(most)) {
            if found.len() >= usize::from(limit) {
                break;
            }
            if starts && start.column != 0 {
                continue;
            }
            let row_start = Position {
                row: start.row,
                column: 0,
            };
            let far = isize::try_from(FAR).expect("rows");
            let to_end = self.sim.move_rows(row_start, far).map_err(failed)?.1;
            let (end, _) = self
                .sim
                .move_rows(row_start, isize::from(read.max(1)))
                .map_err(failed)?;
            found.push(FoundText {
                to_end: u32::try_from(to_end.unsigned_abs()).unwrap_or(u32::MAX),
                text: self.text(Range {
                    start: row_start,
                    end,
                })?,
            });
        }
        Ok(found)
    }

    /// One step, `op`, after the outcomes `earlier`.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per operation, in the list's order"
    )]
    fn step(&mut self, op: &Op, earlier: &[StepOutcome]) -> Found<Done> {
        let failed = |_| StepOutcome::Failed;
        match op {
            Op::Visible { .. } => {
                let screen = self.sim.visible();
                let document = self.sim.document_range();
                let (text, padded) = Self::trimmed(self.text(screen)?, true);
                let at_text_start = self
                    .sim
                    .compare(document.start, screen.start)
                    .map_err(failed)?
                    == 0;
                Ok(Done::Visible(VisibleText {
                    start: self.hold(screen.start),
                    end: self.hold(screen.end),
                    at_text_start,
                    text,
                    padded: padded.unwrap_or_default(),
                }))
            }
            Op::ReadText {
                at,
                movement,
                unit,
                count,
                whole,
                trim,
                ..
            } => {
                if *unit != TextUnit::Line
                    || movement.is_some_and(|movement| movement.unit != TextUnit::Line)
                {
                    return Err(StepOutcome::Refused(StepRefusal::NoSuchUnit));
                }
                let from = self.point(at, earlier)?;
                if *whole {
                    let (end, _) = self
                        .sim
                        .move_rows(from, isize::try_from(*count).unwrap_or(isize::MAX))
                        .map_err(failed)?;
                    let (text, padded) =
                        Self::trimmed(self.text(Range { start: from, end })?, *trim);
                    let chunk = self.chunk(from, text, 0);
                    return Ok(Done::ReadText(Chunks {
                        moved: 0,
                        chunks: vec![chunk],
                        padded,
                    }));
                }
                self.read_lines(
                    from,
                    movement.map(|movement| movement.count),
                    (*count, *trim),
                )
            }
            Op::ReadRange {
                start,
                end,
                trim,
                through,
                ..
            } => {
                assert!(through.is_none(), "a terminal's range runs to its end");
                let (a, b) = (self.point(start, earlier)?, self.point(end, earlier)?);
                let (start, end) = if a <= b { (a, b) } else { (b, a) };
                let (text, padded) = Self::trimmed(self.text(Range { start, end })?, *trim);
                Ok(Done::ReadRange(RangeText {
                    text,
                    truncated: false,
                    padded,
                }))
            }
            Op::Move { at, movement, .. } => {
                if movement.unit != TextUnit::Line {
                    return Err(StepOutcome::Refused(StepRefusal::NoSuchUnit));
                }
                // A held range that lies past the text's end, or that the
                // provider will not compare, is outside the text.
                let held = matches!(at, Point::At(TextPoint::At(_)));
                let outside = StepOutcome::Refused(StepRefusal::OutsideText);
                let from = match self.point(at, earlier) {
                    Err(StepOutcome::Failed) if held => return Err(outside),
                    other => other?,
                };
                if held {
                    let end = self.sim.document_range().end;
                    match self.sim.compare(from, end) {
                        Ok(order) if order <= 0 => {}
                        Ok(_) | Err(_) => return Err(outside),
                    }
                }
                let (to, moved) = self
                    .sim
                    .move_rows(from, isize::try_from(movement.count).unwrap_or(0))
                    .map_err(failed)?;
                Ok(Done::Move(Moved {
                    moved: i32::try_from(moved).unwrap_or(0),
                    from: self.hold(from),
                    to: self.hold(to),
                }))
            }
            Op::Caret { .. } => {
                let caret = self.hold(self.sim.caret().start);
                Ok(Done::Caret(CaretAt {
                    caret,
                    selection: None,
                    held: caret,
                    offsets: None,
                }))
            }
            Op::FindText {
                at,
                needle,
                backward,
                unit_start,
                read,
                limit,
                calls,
                ..
            } => {
                let from = self.point(at, earlier)?;
                self.search(
                    from,
                    (needle, *backward),
                    (unit_start.is_some(), *read),
                    (*limit, *calls),
                )
                .map(Done::FindText)
            }
            Op::ReadColours { .. } => {
                let rows = self.sim.program_colours().map_err(failed)?;
                Ok(Done::ReadColours(
                    rows.into_iter()
                        .map(|row| {
                            row.into_iter()
                                .map(|run| TerminalColourRun {
                                    column: u32::try_from(run.column).unwrap_or(u32::MAX),
                                    length: u32::try_from(run.length).unwrap_or(u32::MAX),
                                    foreground: run.foreground,
                                    background: run.background,
                                })
                                .collect()
                        })
                        .collect(),
                ))
            }
            other => panic!("a step the simulated terminal does not answer: {other:?}"),
        }
    }

    /// The outcome of `op`, run.
    fn ran(&mut self, op: &Op, earlier: &[StepOutcome]) -> StepOutcome {
        match self.step(op, earlier) {
            Ok(done) => StepOutcome::Done(done),
            Err(outcome) => outcome,
        }
    }

    /// Whether `point` lies at or before `than`, as a reader decides a
    /// comparison of two points (the request list's condition table): an
    /// anchor no longer held is the step's outcome, and a point that
    /// cannot be found or a comparison the provider refuses is false.
    fn not_after(&self, point: &Point, than: &Point, earlier: &[StepOutcome]) -> Found<bool> {
        let found = |point| match self.point(point, earlier) {
            Err(missing @ StepOutcome::NotThere(_)) => Err(missing),
            other => Ok(other.ok()),
        };
        let (Some(point), Some(than)) = (found(point)?, found(than)?) else {
            return Ok(false);
        };
        Ok(self.sim.compare(point, than).is_ok_and(|order| order <= 0))
    }

    /// Every step of `batch`, in order, each run when its condition lets
    /// it, answered as the provider answers its calls.
    pub fn run(&mut self, batch: &Batch) -> Vec<StepOutcome> {
        let mut outcomes = Vec::new();
        for step in &batch.steps {
            let outcome = match step.gate(&outcomes).expect("a gate the support can judge") {
                Gate::Skip => StepOutcome::Skipped,
                Gate::Run => self.ran(&step.op, &outcomes),
                Gate::Compare {
                    point,
                    than,
                    negated,
                } => match self.not_after(&point, &than, &outcomes) {
                    Err(missing) => missing,
                    Ok(holds) if holds == negated => StepOutcome::Skipped,
                    Ok(_) => self.ran(&step.op, &outcomes),
                },
            };
            outcomes.push(outcome);
        }
        outcomes
    }
}

/// Whether `batch` opens with a text step this answerer answers, so that
/// the session gives it the batch.
pub fn answers(batch: &Batch) -> bool {
    matches!(
        batch.steps.first().map(|step| &step.op),
        Some(
            Op::Visible { .. }
                | Op::ReadText { .. }
                | Op::ReadRange { .. }
                | Op::Move { .. }
                | Op::FindText { .. }
                | Op::ReadColours { .. }
        )
    )
}

/// A move by `count` lines.
fn lines(count: i32) -> TextMovement {
    TextMovement {
        unit: TextUnit::Line,
        count,
    }
}

/// What an old read asks, as the generic steps name it: the old
/// operation's query with its start a held anchor.
#[derive(Clone, Copy, Debug)]
pub struct ReadAsk {
    /// The start the last read named, held; `None` for a first read.
    pub start: Option<TextAnchor>,
    /// The screen's height as the last read found it; 0 when not known.
    pub height: u32,
    /// Whether to read the whole text.
    pub whole: bool,
    /// How many rows above the start to begin the read.
    pub above: u32,
    /// Whether to read the screen's colours.
    pub colours: bool,
}

/// One candidate start's steps: the move up by the rows asked above it,
/// when any are, and the text from where it landed to the held end.
struct Candidate<B> {
    up: Option<Ref<B, kind::Move>>,
    text: Ref<B, kind::ReadRange>,
}

/// The steps of a composed read ([`add_read`]).
pub struct ReadSteps<B> {
    ask: ReadAsk,
    visible: Ref<B, kind::Visible>,
    next: Option<Ref<B, kind::Move>>,
    next_to_end: Ref<B, kind::Move>,
    document: Ref<B, kind::Move>,
    fallback: Candidate<B>,
    named: Option<(Ref<B, kind::Move>, Candidate<B>)>,
    first: Ref<B, kind::ReadText>,
    caret: Ref<B, kind::Caret>,
    caret_line: Ref<B, kind::ReadText>,
    caret_rows: Ref<B, kind::Move>,
    colours: Option<Ref<B, kind::ReadColours>>,
}

/// Adds the read `ask` names of terminal `node` to `batch`, as the
/// module's documentation lists its steps.
pub fn add_read<B>(batch: &mut Builder<B>, node: NodeId, ask: &ReadAsk) -> ReadSteps<B> {
    let visible = batch.visible(node);
    let top = ask
        .colours
        .then(|| batch.read_trimmed(node, visible.visible_start(), TextUnit::Line, (1, false)));
    let end = batch.move_point(node, TextPoint::End.into(), lines(FAR));
    let next = (ask.height > 0).then(|| {
        let up = i32::try_from(ask.height).map_or(-FAR, |height| -height);
        batch.move_point(node, end.landed(), lines(up))
    });
    let next_start = next.map_or(visible.visible_start(), Ref::landed);
    let next_to_end = batch.move_point(node, next_start, lines(FAR));
    let document = batch.move_point(node, end.landed(), lines(-FAR));
    let above = (ask.above > 0 && !ask.whole)
        .then(|| i32::try_from(ask.above).map_or(-FAR, |above| -above));
    let fallback = {
        let start = if ask.whole {
            TextPoint::Start.into()
        } else {
            next_start
        };
        let up = above.map(|above| batch.move_point(node, start, lines(above)));
        let from = up.map_or(start, Ref::landed);
        Candidate {
            up,
            text: batch.read_range_trimmed(node, (from, end.landed())),
        }
    };
    let named = ask.start.filter(|_| !ask.whole).map(|start| {
        let start = TextPoint::At(TextPosition::at(start)).into();
        let to_end = batch.move_point(node, start, lines(FAR));
        let up = above.map(|above| {
            batch.when(to_end.found());
            batch.move_point(node, start, lines(above))
        });
        let from = up.map_or(start, Ref::landed);
        batch.when(to_end.found());
        let text = batch.read_range_trimmed(node, (from, end.landed()));
        (to_end, Candidate { up, text })
    });
    let first = batch.read_trimmed(node, TextPoint::Start.into(), TextUnit::Line, (1, false));
    let caret = batch.caret(node, None);
    let caret_line = batch.read_text(
        node,
        caret.caret(),
        None,
        TextUnit::Line,
        1,
        TextDetails::NONE,
    );
    let caret_rows = batch.move_point(node, caret.caret(), lines(FAR));
    let colours = top.map(|top| batch.read_colours(node, visible, top));
    ReadSteps {
        ask: *ask,
        visible,
        next,
        next_to_end,
        document,
        fallback,
        named,
        first,
        caret,
        caret_line,
        caret_rows,
        colours,
    }
}

/// An old read composed of generic steps' answers: the old operation's
/// snapshot, its times zero, and the anchor held at the next read's start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composed {
    /// The snapshot.
    pub snapshot: TerminalSnapshot,
    /// The next read's start, held.
    pub next: TextAnchor,
}

/// A step's result, when it was done.
fn done<'a, T>(got: &Got<'a, T>) -> Option<&'a T> {
    match got {
        Got::Done(done) => Some(done),
        _ => None,
    }
}

/// How many rows a move went.
fn rows(moved: &Moved) -> u32 {
    moved.moved.unsigned_abs()
}

/// The one line a read of one line gave, without its padding.
fn line(read: &Chunks) -> Option<TerminalText> {
    let chunk = read.chunks.first()?;
    Some(match &read.padded {
        Some(lengths) => TerminalText {
            text: chunk.text.clone(),
            lengths: lengths.clone(),
        },
        None => TerminalText::unpadded(&chunk.text),
    })
}

/// The old read the answers to `steps` compose; `None` when a step the
/// read needs was not done.
pub fn read_of<B>(answers: &Answers<B>, steps: &ReadSteps<B>) -> Option<Composed> {
    let ask = &steps.ask;
    let visible = done(&answers.get(steps.visible))?;
    let next = match steps.next {
        Some(next) => done(&answers.get(next))?.to,
        None => visible.start,
    };
    let next_to_end = rows(done(&answers.get(steps.next_to_end))?);
    let document = rows(done(&answers.get(steps.document))?);
    // The named start is used when it lies at or above the next start:
    // at least as many rows from the end. A move from it refused, or a
    // start below the next one, is the text shrunk under the read.
    let named = match &steps.named {
        None => None,
        Some((to_end, candidate)) => Some(
            match answers.get(*to_end) {
                Got::Done(moved) => Some(rows(moved)),
                Got::Refused(StepRefusal::OutsideText) => None,
                _ => return None,
            }
            .filter(|&to_end| to_end >= next_to_end)
            .map(|to_end| (to_end, candidate)),
        ),
    };
    let from_start = matches!(named, Some(Some(_)));
    let shrank = matches!(named, Some(None));
    let (to_end, candidate) = named.flatten().unwrap_or((next_to_end, &steps.fallback));
    let above = match candidate.up {
        Some(up) => rows(done(&answers.get(up))?),
        None => 0,
    };
    let text = done(&answers.get(candidate.text))?;
    let caret_line = done(&answers.get(steps.caret_line))?;
    let chunk = caret_line.chunks.first()?;
    let before = chunk.text.get(..usize::try_from(chunk.offset).ok()?)?;
    let colours = match steps.colours {
        Some(colours) => Some(done(&answers.get(colours))?.clone()),
        None => None,
    };
    let _held: &CaretAt = done(&answers.get(steps.caret))?;
    Some(Composed {
        snapshot: TerminalSnapshot {
            text: TerminalText {
                text: text.text.clone(),
                lengths: text.padded.clone()?,
            },
            screen: TerminalText {
                text: visible.text.clone(),
                lengths: visible.padded.clone(),
            },
            from_start,
            shrank,
            at_document_start: ask.whole || to_end + above == document,
            alternate: visible.at_text_start,
            first_row: line(done(&answers.get(steps.first))?)?,
            caret_rows_to_end: rows(done(&answers.get(steps.caret_rows))?),
            caret_line: line(caret_line)?,
            caret_column: u32::try_from(before.chars().count()).ok()?,
            above,
            started_ms: 0,
            ended_ms: 0,
            waited: false,
            colours,
            caret: None,
        },
        next,
    })
}

/// How each way of a composed search bounds it: the matches that start a
/// row, at most 20 over at most 100 `FindText` calls, as the search of
/// `crates/mockapp/tests/common/window_steps.rs` bounds it.
fn way(read: u8) -> Search {
    Search {
        unit_start: Some(TextUnit::Line),
        read,
        limit: 20,
        calls: 100,
    }
}

/// The steps of a composed search ([`add_search`]).
pub struct SearchSteps<B> {
    down: Ref<B, kind::FindText>,
    up: Ref<B, kind::FindText>,
    to_end: Ref<B, kind::Move>,
}

/// Adds to `batch` the old search for the rows of terminal `node` that
/// start with `needle`, outward from the row `from` rows above the held
/// start `start`, each read for `read` rows: a move to that row, a search
/// down from it and one up, and a move from the start to the text's end,
/// which the matches are counted against.
pub fn add_search<B>(
    batch: &mut Builder<B>,
    node: NodeId,
    (start, from): (TextAnchor, i32),
    (needle, read): (&str, u8),
) -> SearchSteps<B> {
    let mark = || TextPoint::At(TextPosition::at(start)).into();
    let row = batch.move_point(node, mark(), lines(from.checked_neg().expect("rows")));
    let down = batch.find_text(node, row.landed(), (needle.to_owned(), false), way(read));
    let up = batch.find_text(node, row.landed(), (needle.to_owned(), true), way(read));
    let to_end = batch.move_point(node, mark(), lines(FAR));
    SearchSteps { down, up, to_end }
}

/// The old search's matches the answers to `steps` compose: alternately
/// the next below the row searched from and the next above it, one way's
/// when the other has no more, at most 20, each counted in rows above the
/// start; `None` when a step was not done.
pub fn matches_of<B>(answers: &Answers<B>, steps: &SearchSteps<B>) -> Option<Vec<TerminalMatch>> {
    let down = done(&answers.get(steps.down))?;
    let up = done(&answers.get(steps.up))?;
    let from_end = i64::from(rows(done(&answers.get(steps.to_end))?));
    let (mut down, mut up) = (down.iter(), up.iter());
    let mut taken = Vec::new();
    let mut going_down = true;
    while taken.len() < 20 {
        let next = if going_down {
            down.next().or_else(|| up.next())
        } else {
            up.next().or_else(|| down.next())
        };
        let Some(found) = next else {
            break;
        };
        taken.push(TerminalMatch {
            above: i32::try_from(i64::from(found.to_end) - from_end).ok()?,
            text: found.text.clone(),
        });
        going_down = !going_down;
    }
    Some(taken)
}

/// The old operation's query for `ask`, its start the range the generic
/// steps hold.
pub fn query_of(outpost: &Outpost, ask: &ReadAsk) -> Query {
    Query {
        start: ask
            .start
            .map(|start| Range::at(outpost.held(start).expect("a held start"))),
        screen_rows: ask.height,
        whole: ask.whole,
        above: ask.above,
    }
}
