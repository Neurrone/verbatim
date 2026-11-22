//! The batches a caret key and a caret tell send (the request list's design,
//! section 6, `StartingRead`, `Watch`, `MoveBySentence` and
//! `ReadSelection`), as Core's tests expect them, and their answers made
//! from the facts a test gives: shared by Core's unit tests and its
//! integration tests, so it names only the model's types.

#![allow(dead_code)]

use verbatim_model::batch::{
    Batch, Before, CaretAt, CaretWait, Check, Chunks, Condition, Done, EffectTest, HeldSelection,
    Missing, Op, Place, Placement, Point, PointOf, SelectionCompared, Step, StepOutcome, Subject,
    Waited,
};
use verbatim_model::{
    CaretMotion, CaretReport, EditSelection, NodeId, SelectionDifference, SentenceMoved,
    TextAnchor, TextChunk, TextDetails, TextMovement, TextProvider, TextUnit,
};

/// The caret of the starting point every test's outpost holds.
pub const POINT: TextAnchor = TextAnchor(50);

/// A step with no condition.
fn step(op: Op) -> Step {
    Step { op, when: None }
}

/// A batch of `steps` in the event queue.
fn queued(steps: Vec<Step>) -> Batch {
    Batch {
        steps,
        placement: Placement::Queue,
        spare: None,
    }
}

/// A `ReadText` of `node` at the caret of step 0, after `movement`.
fn read_at_caret(
    node: NodeId,
    movement: Option<TextMovement>,
    unit: TextUnit,
    details: TextDetails,
) -> Op {
    Op::ReadText {
        subject: Subject::Node(node),
        at: Point::Of(Place(0), PointOf::Caret),
        movement,
        unit,
        count: 1,
        details,
    }
}

/// A caret key of `motion`'s starting batch on `node`: the caret, its
/// line, the unit around the character before it for a key that deletes
/// back, and the word at it for a key whose wait compares it.
#[must_use]
pub fn starting(node: NodeId, motion: CaretMotion) -> Batch {
    let mut steps = vec![
        step(Op::Caret {
            subject: Subject::Node(node),
            adopt: None,
        }),
        step(read_at_caret(node, None, TextUnit::Line, TextDetails::NONE)),
    ];
    if motion.deletes_back() {
        steps.push(step(read_at_caret(
            node,
            Some(TextMovement {
                unit: TextUnit::Character,
                count: -1,
            }),
            motion.unit(),
            TextDetails::NONE,
        )));
    }
    if motion.compares_unit() {
        steps.push(step(read_at_caret(
            node,
            None,
            TextUnit::Word,
            TextDetails::NONE,
        )));
    }
    queued(steps)
}

/// The unit a key of `motion` speaks at its end in text read through
/// `provider`, as Core reads it.
fn key_unit(provider: TextProvider, motion: CaretMotion) -> TextUnit {
    match motion.unit() {
        TextUnit::Sentence if provider.has_sentences() => TextUnit::Paragraph,
        TextUnit::Sentence | TextUnit::Document => TextUnit::Line,
        unit => unit,
    }
}

/// The wait a key of `motion` sends on `node` from [`POINT`], its starting
/// line `line` with the caret at `offset` and its starting word `word`,
/// passed at `passed_at_us`.
#[must_use]
pub fn wait(
    motion: CaretMotion,
    (line, offset, word): (&str, u32, Option<&str>),
    (passed_at_us, poll_us, bound_us, events): (u64, u32, u32, bool),
) -> CaretWait {
    CaretWait {
        from: POINT,
        line: line.to_owned(),
        offset,
        word: word.map(str::to_owned),
        effect: EffectTest {
            unit: motion.compares_unit(),
            line: motion.deletes(),
        },
        passed_at_us,
        poll_us,
        bound_us,
        events,
    }
}

/// A caret key of `motion`'s wait batch on `node` in text read through
/// `provider`: `wait`, then the caret, its line and the key's unit, the
/// reads only at the effect for a sentence key, the formatting `details`
/// on the line for a line or a character and on the unit otherwise.
#[must_use]
pub fn waiting(
    (node, provider): (NodeId, TextProvider),
    motion: CaretMotion,
    wait: CaretWait,
    details: TextDetails,
) -> Batch {
    let unit = key_unit(provider, motion);
    let by_line = matches!(unit, TextUnit::Line | TextUnit::Character);
    let sentence = matches!(
        motion,
        CaretMotion::PreviousSentence | CaretMotion::NextSentence
    );
    let mut steps = vec![
        step(Op::WaitForCaret {
            subject: Subject::Node(node),
            wait: Box::new(wait),
        }),
        Step {
            op: Op::Caret {
                subject: Subject::Node(node),
                adopt: None,
            },
            when: sentence.then_some(Condition {
                on: Place(0),
                check: Check::Found,
                negated: false,
            }),
        },
        step(Op::ReadText {
            subject: Subject::Node(node),
            at: Point::Of(Place(1), PointOf::Caret),
            movement: None,
            unit: TextUnit::Line,
            count: 1,
            details: if by_line { details } else { TextDetails::NONE },
        }),
    ];
    if !by_line {
        steps.push(step(Op::ReadText {
            subject: Subject::Node(node),
            at: Point::Of(Place(1), PointOf::Caret),
            movement: None,
            unit,
            count: 1,
            details,
        }));
    }
    queued(steps)
}

/// A terminal key's wait on `node`: the wait alone.
#[must_use]
pub fn terminal_waiting(node: NodeId, wait: CaretWait) -> Batch {
    queued(vec![step(Op::WaitForCaret {
        subject: Subject::Node(node),
        wait: Box::new(wait),
    })])
}

/// A sentence key's move on `node` from [`POINT`].
#[must_use]
pub fn sentence(node: NodeId, forward: bool, details: TextDetails) -> Batch {
    queued(vec![step(Op::MoveBySentence {
        subject: Subject::Node(node),
        from: POINT,
        forward,
        details,
    })])
}

/// A caret tell's batch on `node`: the caret, adopting `adopt`'s element;
/// its line; and the selection compared with `baseline` when there is
/// one.
#[must_use]
pub fn told(
    node: NodeId,
    baseline: Option<TextAnchor>,
    adopt: Option<verbatim_model::tell::ElementToken>,
) -> Batch {
    let mut steps = vec![
        step(Op::Caret {
            subject: Subject::Node(node),
            adopt,
        }),
        step(read_at_caret(node, None, TextUnit::Line, TextDetails::NONE)),
    ];
    if let Some(baseline) = baseline {
        steps.push(step(Op::CompareSelection {
            subject: Subject::Node(node),
            before: Before::Baseline(baseline),
            after: Place(0),
        }));
    }
    queued(steps)
}

/// What an outpost read for a caret key's or a caret tell's batch, from
/// which [`answer`] makes its outcomes.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// The caret the batch's `Caret` reads, and the line its line read
    /// reads; `None` when it is not read.
    pub caret: Option<CaretReport>,
    /// The held read a `Caret` holds, the next selection baseline.
    pub held: Option<TextAnchor>,
    /// The unit any other `ReadText` reads.
    pub unit: Option<TextChunk>,
    /// The word a `ReadText` of a word reads; the unit when `None`.
    pub word: Option<TextChunk>,
    /// How a wait ended.
    pub waited: Option<Waited>,
    /// The comparison of a `CompareSelection`: `None` for a baseline no
    /// longer held.
    pub difference: Option<SelectionDifference>,
    /// A move by sentence's result.
    pub moved: Option<SentenceMoved>,
    /// The selection's offsets in text that counts by them.
    pub offsets: Option<EditSelection>,
    /// Whether the caret, and the selection's start when something is
    /// selected, are at the text's start.
    pub at_start: bool,
}

/// The facts of a caret key's or a caret tell's read: the caret on `line`
/// with `unit` read at it.
#[must_use]
pub fn read(caret: CaretReport, unit: TextChunk) -> Facts {
    Facts {
        caret: Some(caret),
        unit: Some(unit),
        ..Facts::default()
    }
}

/// The `Caret` result the caret `report` gives, holding `held`, its
/// selection's start at the text's start when `at_start`.
fn caret_at(report: &CaretReport, held: TextAnchor, at_start: bool) -> CaretAt {
    CaretAt {
        caret: report.line.start,
        selection: report.selection.as_ref().map(|selection| HeldSelection {
            start: selection.range.start.anchor,
            end: selection.range.end.anchor,
            at_text_start: at_start,
        }),
        held,
        offsets: None,
    }
}

/// One chunk, read where no movement went.
fn chunks(chunk: TextChunk, moved: i32) -> StepOutcome {
    StepOutcome::Done(Done::ReadText(Chunks {
        moved,
        chunks: vec![chunk],
    }))
}

/// The outcomes of `batch` answered from `facts`; a step whose fact is
/// missing, or whose condition does not hold, is skipped.
#[must_use]
pub fn answer(batch: &Batch, facts: &Facts) -> Vec<StepOutcome> {
    let mut outcomes: Vec<StepOutcome> = Vec::new();
    for step in &batch.steps {
        let gated = step.when.is_some_and(|when| {
            let found = matches!(
                outcomes.get(when.on.index()),
                Some(StepOutcome::Done(Done::WaitForCaret(Waited::Effect)))
            );
            found == when.negated
        });
        let outcome = match &step.op {
            _ if gated => StepOutcome::Skipped,
            Op::WaitForCaret { .. } => facts.waited.map_or(StepOutcome::Failed, |waited| {
                StepOutcome::Done(Done::WaitForCaret(waited))
            }),
            Op::Caret { .. } => facts.caret.as_ref().map_or(StepOutcome::Skipped, |caret| {
                StepOutcome::Done(Done::Caret(CaretAt {
                    offsets: facts.offsets,
                    ..caret_at(caret, facts.held.unwrap_or(POINT), facts.at_start)
                }))
            }),
            Op::ReadText {
                at: Point::Of(place, _),
                ..
            } if !matches!(
                outcomes.get(place.index()),
                Some(StepOutcome::Done(Done::Caret(_)))
            ) =>
            {
                StepOutcome::Skipped
            }
            Op::ReadText {
                unit: TextUnit::Line,
                ..
            } => facts
                .caret
                .as_ref()
                .map_or(StepOutcome::Skipped, |caret| chunks(caret.line.clone(), 0)),
            Op::ReadText {
                unit: TextUnit::Word,
                movement: None,
                ..
            } if facts.word.is_some() => facts
                .word
                .clone()
                .map_or(StepOutcome::Skipped, |word| chunks(word, 0)),
            Op::ReadText { movement, .. } => {
                facts.unit.as_ref().map_or(StepOutcome::Skipped, |unit| {
                    let moved = match movement {
                        Some(_) if !unit.text.is_empty() => -1,
                        _ => 0,
                    };
                    chunks(unit.clone(), moved)
                })
            }
            Op::CompareSelection { before, .. } => match (&facts.difference, before) {
                (Some(difference), _) => {
                    StepOutcome::Done(Done::CompareSelection(SelectionCompared {
                        difference: difference.clone(),
                        text_changed: false,
                    }))
                }
                (None, Before::Baseline(anchor)) => StepOutcome::NotThere(Missing::Anchor(*anchor)),
                (None, Before::SelectionOf(_)) => StepOutcome::Failed,
            },
            Op::MoveBySentence { .. } => facts.moved.clone().map_or(StepOutcome::Failed, |moved| {
                StepOutcome::Done(Done::MoveBySentence(moved))
            }),
            other => panic!("a step a caret key or tell does not send: {other:?}"),
        };
        outcomes.push(outcome);
    }
    outcomes
}
