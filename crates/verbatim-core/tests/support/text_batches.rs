//! The batches a text command sends (the request list's design, section 6,
//! the `TextOp` rows: a review, navigator or say-all read, a copy or the
//! selected text, review select, say-all's caret move, a location, and the
//! review word rows), as Core's tests expect them, and their answers:
//! shared by Core's unit tests and its integration tests, so it names only
//! the model's types.

#![allow(dead_code)]

use verbatim_model::batch::{
    Batch, Before, CaretAt, Chunks, Done, Located, Missing, Op, Place, Placement, Point, PointOf,
    RangeText, Selected, SelectionCompared, Step, StepOutcome, StepRefusal, Subject, Through,
};
use verbatim_model::request::Reply;
use verbatim_model::{
    NodeId, SelectionChange, SelectionDifference, TextAnchor, TextChunk, TextDetails, TextMovement,
    TextPoint, TextPosition, TextUnit,
};

/// A step with no condition.
fn step(op: Op) -> Step {
    Step { op, when: None }
}

/// A batch of `ops` in the event queue.
fn queued(ops: Vec<Op>) -> Batch {
    Batch {
        steps: ops.into_iter().map(step).collect(),
        placement: Placement::Queue,
        spare: None,
    }
}

/// A `ReadText` op of `node`.
fn read_op(node: NodeId, at: Point, movement: Option<TextMovement>, unit: TextUnit) -> Op {
    Op::ReadText {
        trim: false,
        whole: false,
        subject: Subject::Node(node),
        at,
        movement,
        unit,
        count: 1,
        details: TextDetails::NONE,
    }
}

/// A movement of `count` units of `unit`.
pub fn by(unit: TextUnit, count: i32) -> TextMovement {
    TextMovement { unit, count }
}

/// The position at `anchor`, `offset` bytes on.
pub fn at(anchor: u64, offset: u32) -> TextPoint {
    TextPoint::At(TextPosition {
        anchor: TextAnchor(anchor),
        offset,
    })
}

/// `count` units of `unit` of `node`'s text from `at`, after `movement`.
pub fn read(
    node: NodeId,
    at: TextPoint,
    movement: Option<TextMovement>,
    unit: TextUnit,
    count: u8,
) -> Batch {
    queued(vec![Op::ReadText {
        trim: false,
        whole: false,
        subject: Subject::Node(node),
        at: Point::At(at),
        movement,
        unit,
        count: count.into(),
        details: TextDetails::NONE,
    }])
}

/// The line of `node`'s text at `at`, after `movement`.
pub fn line(node: NodeId, at: TextPoint, movement: Option<TextMovement>) -> Batch {
    read(node, at, movement, TextUnit::Line, 1)
}

/// The points past the characters at a span's two ends.
fn through(through: Option<(TextPoint, TextPoint)>) -> Option<Through> {
    through.map(|(start, end)| Through {
        start: Point::At(start),
        end: Point::At(end),
    })
}

/// The text of `node` between `start` and `end`.
pub fn range(
    node: NodeId,
    (start, end): (TextPoint, TextPoint),
    past: Option<(TextPoint, TextPoint)>,
) -> Batch {
    queued(vec![Op::ReadRange {
        trim: false,
        subject: Subject::Node(node),
        start: Point::At(start),
        end: Point::At(end),
        through: through(past),
    }])
}

/// The caret of `node`'s text.
fn caret(node: NodeId) -> Op {
    Op::Caret {
        subject: Subject::Node(node),
        adopt: None,
    }
}

/// Review select of `node`'s text between `start` and `end`: the caret,
/// the selection, the caret again and the two compared.
pub fn select(
    node: NodeId,
    (start, end): (TextPoint, TextPoint),
    past: Option<(TextPoint, TextPoint)>,
) -> Batch {
    queued(vec![
        caret(node),
        Op::Select {
            subject: Subject::Node(node),
            start: Point::At(start),
            end: Point::At(end),
            through: through(past),
        },
        caret(node),
        Op::CompareSelection {
            subject: Subject::Node(node),
            before: Before::SelectionOf(Place(0)),
            after: Place(2),
        },
    ])
}

/// Say-all's caret move in `node`'s text to `to`, then the caret.
pub fn move_caret(node: NodeId, to: TextPoint) -> Batch {
    queued(vec![
        Op::Select {
            subject: Subject::Node(node),
            start: Point::At(to),
            end: Point::At(to),
            through: None,
        },
        caret(node),
    ])
}

/// Where `at` is in `node`'s text.
pub fn locate(node: NodeId, at: TextPoint) -> Batch {
    queued(vec![Op::Locate {
        subject: Subject::Node(node),
        at: Point::At(at),
    }])
}

/// Review current word at `at` of `node`'s text.
pub fn current_word(node: NodeId, at: TextPoint) -> Batch {
    queued(vec![read_op(node, Point::At(at), None, TextUnit::Word)])
}

/// Review previous word from `at` of `node`'s text: the word there, the
/// word before it, and that word's line.
pub fn previous_word(node: NodeId, at: TextPoint) -> Batch {
    let start = |place| Point::Of(Place(place), PointOf::ChunkStart);
    queued(vec![
        read_op(node, Point::At(at), None, TextUnit::Word),
        read_op(node, start(0), Some(by(TextUnit::Word, -1)), TextUnit::Word),
        read_op(node, start(1), None, TextUnit::Line),
    ])
}

/// Review next word from `at` of `node`'s text: the word after it, the
/// text between, and that word's line.
pub fn next_word(node: NodeId, at: TextPoint) -> Batch {
    let start = Point::Of(Place(0), PointOf::ChunkStart);
    queued(vec![
        read_op(
            node,
            Point::At(at),
            Some(by(TextUnit::Word, 1)),
            TextUnit::Word,
        ),
        Op::ReadRange {
            trim: false,
            subject: Subject::Node(node),
            start: Point::At(at),
            end: start,
            through: None,
        },
        read_op(node, start, None, TextUnit::Line),
    ])
}

/// A chunk of `unit`, `text`, starting at anchor `anchor`, with the point
/// at byte `offset`.
pub fn chunk(unit: TextUnit, text: &str, anchor: u64, offset: u32) -> TextChunk {
    TextChunk {
        unit,
        text: text.to_owned(),
        start: TextAnchor(anchor),
        offset,
        languages: Vec::new(),
        first: false,
        last: false,
        truncated: false,
        formats: Vec::new(),
    }
}

/// A `ReadText` done: `moved`, and `chunks`.
pub fn chunks(moved: i32, chunks: Vec<TextChunk>) -> StepOutcome {
    StepOutcome::Done(Done::ReadText(Chunks {
        padded: None,
        moved,
        chunks,
    }))
}

/// A `ReadRange` done with `text`.
pub fn range_text(text: &str) -> StepOutcome {
    StepOutcome::Done(Done::ReadRange(RangeText {
        padded: None,
        text: text.to_owned(),
        truncated: false,
    }))
}

/// The answer of a one-step batch: `outcome`.
pub fn one(outcome: StepOutcome) -> Reply {
    Reply::Steps(vec![outcome])
}

/// A step refused for `refusal`.
pub fn refused(refusal: StepRefusal) -> StepOutcome {
    StepOutcome::Refused(refusal)
}

/// A step whose anchor `anchor` is no longer held.
pub fn lost(anchor: u64) -> StepOutcome {
    StepOutcome::NotThere(Missing::Anchor(TextAnchor(anchor)))
}

/// A caret read with nothing selected, its read held under `held`.
fn caret_at(held: u64) -> StepOutcome {
    StepOutcome::Done(Done::Caret(CaretAt {
        caret: TextAnchor(held),
        selection: None,
        held: TextAnchor(held),
        offsets: None,
        reported: true,
    }))
}

/// Review select's answer: the selection changed by `changes`, the read
/// after it held under `baseline`.
pub fn selected(changes: Vec<SelectionChange>, baseline: u64) -> Reply {
    Reply::Steps(vec![
        caret_at(baseline.saturating_sub(1)),
        StepOutcome::Done(Done::Select(Selected { began_us: 0 })),
        caret_at(baseline),
        StepOutcome::Done(Done::CompareSelection(SelectionCompared {
            difference: SelectionDifference {
                changes,
                new_selection: None,
            },
            text_changed: false,
        })),
    ])
}

/// Review select's answer when the text cannot be selected.
pub fn cannot_select() -> Reply {
    Reply::Steps(vec![
        caret_at(1),
        refused(StepRefusal::Unsupported),
        caret_at(2),
        StepOutcome::Skipped,
    ])
}

/// Say-all's caret move answered, the caret's read held under
/// `baseline`.
pub fn moved(baseline: u64) -> Reply {
    Reply::Steps(vec![
        StepOutcome::Done(Done::Select(Selected { began_us: 0 })),
        caret_at(baseline),
    ])
}

/// A location answered.
pub fn located(located: Located) -> Reply {
    one(StepOutcome::Done(Done::Locate(located)))
}

/// A word step's answer: the word, `text`, at anchor `anchor`, moved
/// `moved` words.
pub fn word(moved: i32, text: &str, anchor: u64) -> StepOutcome {
    chunks(moved, vec![chunk(TextUnit::Word, text, anchor, 0)])
}

/// A line step's answer: the line `text` at anchor `anchor`, the point at
/// byte `offset`.
pub fn line_at(text: &str, anchor: u64, offset: u32) -> StepOutcome {
    chunks(0, vec![chunk(TextUnit::Line, text, anchor, offset)])
}
