//! The simulated terminal's second answerer: each generic text step of a
//! batch (`Visible`, `Move`, `ReadText` with `whole` and `trim`,
//! `ReadRange`, `Caret`, `FindText` and `ReadColours`) answered as the
//! terminal's UI Automation provider answers the step's calls
//! ([`Outpost::run`]), and the terminal's old search composed of those
//! steps ([`add_search`] with [`matches_of`]).
//!
//! Core reads a terminal by the batches of its terminal reader
//! (`verbatim_core::terminal::reader`), which a session gives this
//! answerer. `terminal_answerers.rs` drives it with those batches too and
//! compares what the reader reads back with the reference read, field by
//! field; the command search stays on its old operation, which
//! `Outpost::find` answers, and is compared as composed here (Dickson,
//! 2026-11-26, `docs/simplification-review/builds.md`).

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
    NodeId, TerminalColourRun, TerminalMatch, TerminalText, TextAnchor, TextChunk, TextMovement,
    TextPoint, TextPosition, TextUnit,
};
use verbatim_term_sim::{Position, Range};

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
                Ok(self.caret().0)
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
                let (at, reported) = self.caret();
                let caret = self.hold(at);
                Ok(Done::Caret(CaretAt {
                    caret,
                    selection: None,
                    held: caret,
                    offsets: None,
                    reported,
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

    /// Where the caret is, and whether the provider reported a selection:
    /// with none reported ([`Outpost::no_selection`]), the text's start
    /// stands in for it, as the outpost's caret read answers.
    fn caret(&self) -> (Position, bool) {
        if self.no_selection {
            (self.sim.document_range().start, false)
        } else {
            (self.sim.caret().start, true)
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
