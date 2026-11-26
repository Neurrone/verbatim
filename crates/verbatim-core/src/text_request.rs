//! A read or move in a node's text: each command builds its batch of the
//! shared list's text operations and judges each step's result itself,
//! through the one rule here for a step not done ([`got`]). Each batch is
//! asked of the node's outpost on the node's window, or answered by Core
//! itself from text it holds: flat text and a terminal's rows
//! ([`answer_steps`]). The commands that read the committed focus's text
//! resolve it here at their turn.

use std::ops::Range;

use verbatim_model::batch::{Batch, Missing, Op, Point, PointOf, StepOutcome, StepRefusal};
use verbatim_model::request::Reply;
use verbatim_model::{
    NodeId, Pid, Role, TextFailure, TextMovement, TextPoint, TextPosition, TextReply, TextUnit,
};

use crate::batch::kind::Kind;
use crate::batch::{Answers, Got, Ref, Sent};
use crate::entries::Entries;
use crate::event_queue::{Cx, Outcome, Reader, RequestWindow};
use crate::state::SrState;

/// A text request's result, parsed: the op's own reply, which `own` takes,
/// or one of the failures any text request can be answered
/// ([`TextFailure`]), so every answer a correct outpost gives decodes, and
/// the function that asked handles each. `None` only for a result that is
/// no text reply, or another op's own reply: a malformed answer. The
/// terminal's own reads use it until they become batches.
pub(crate) fn decode_text<T>(
    reply: Reply,
    own: impl FnOnce(TextReply) -> Option<T>,
) -> Option<Result<T, TextFailure>> {
    let Reply::Text(reply) = reply else {
        return None;
    };
    match reply.into_failure() {
        Ok(failure) => Some(Err(failure)),
        Err(reply) => own(reply).map(Ok),
    }
}

/// Two points of a text, and, to run through the character at the later
/// of them, the points just past the characters at each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    /// One end.
    pub(crate) start: TextPoint,
    /// The other end.
    pub(crate) end: TextPoint,
    /// The points just past the characters at `start` and at `end`.
    pub(crate) through: Option<(TextPoint, TextPoint)>,
}

/// Why a text step gave no result, as each command that reads text judges
/// it: the one rule for a step not done, so every command reads an answer
/// the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Missed {
    /// The text refused it.
    Refused(StepRefusal),
    /// A position it named is no longer held.
    Lost,
    /// The node is gone.
    Gone,
    /// A read failed, or the step was not run.
    Failed,
}

/// The answers to a text batch, or why none came: [`Missed::Gone`] for
/// the text not there.
pub(crate) type TextAnswers<B> = Result<Answers<B>, Missed>;

/// The result of `handle`'s step in `answers`, or why it has none.
pub(crate) fn got<B, K: Kind>(
    answers: &TextAnswers<B>,
    handle: Ref<B, K>,
) -> Result<&K::Out, Missed> {
    match answers.as_ref().map_err(|missed| *missed)?.get(handle) {
        Got::Done(done) => Ok(done),
        Got::Refused(refusal) => Err(Missed::Refused(refusal)),
        Got::NotThere(Missing::Anchor(_)) => Err(Missed::Lost),
        Got::NotThere(_) => Err(Missed::Gone),
        Got::Failed | Got::Skipped => Err(Missed::Failed),
    }
}

/// Who answers a text request.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Answerer<'a> {
    /// The node's outpost, on this window.
    Outpost(RequestWindow),
    /// Core, from this flat text ([`crate::review_text::flat_steps`]).
    Flat(&'a str),
    /// Core, from the rows of the focused terminal, this node, on this
    /// window for the history reads its answer may need.
    Terminal(RequestWindow, NodeId),
}

/// Asks `sent`, a batch of text steps, of `by`, the text's answerer, under
/// the due time `due_us`: its answers, or [`Missed::Gone`] for the text
/// not there; `None` when the request was stopped, refused or not
/// answered, which says nothing, or a local answer did not fit the batch,
/// a defect.
pub(crate) async fn ask<B: 'static>(
    cx: &mut Cx<Entries>,
    by: Answerer<'_>,
    sent: Sent<B>,
    due_us: u64,
) -> Option<TextAnswers<B>> {
    let outcomes = match by {
        Answerer::Outpost(window) => {
            let asked = sent.of(Reader::Outpost(window.process), window);
            return match cx.ask(asked, due_us).await {
                Outcome::Result(answers) => Some(Ok(answers)),
                Outcome::NotThere => Some(Err(Missed::Gone)),
                _ => None,
            };
        }
        Answerer::Flat(text) => crate::review_text::flat_steps(text, sent.batch()),
        Answerer::Terminal(window, node) => {
            crate::terminal::commands::answer(cx, (window, node), sent.batch(), due_us).await?
        }
    };
    // Core's answers give each step one outcome of its own kind, so they
    // always fit the batch.
    let parsed = Answers::parse(&sent, Reply::Steps(outcomes));
    debug_assert!(parsed.is_some(), "Core's own answer fits its text batch");
    parsed.map(Ok)
}

/// One text step as Core answers it from text it holds, its points
/// resolved: a point of an earlier `ReadText`'s chunk is the position at
/// that chunk's start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalStep {
    /// `ReadText`.
    Read {
        /// Where it starts.
        at: TextPoint,
        /// How it moves first.
        movement: Option<TextMovement>,
        /// The unit read.
        unit: TextUnit,
        /// How many units.
        count: u8,
    },
    /// `ReadRange`.
    Range(Span),
    /// Any other operation, which text Core holds cannot do.
    Other,
}

/// Answers `batch` step by step with `answer`, as Core answers a text
/// request on text it holds (flat text and a terminal's rows): a step
/// whose condition fails, or that names an earlier step not done, is
/// skipped, as a reader skips it.
///
/// # Errors
///
/// The first error `answer` gives, which ends the answer.
pub(crate) fn answer_steps<E>(
    batch: &Batch,
    mut answer: impl FnMut(LocalStep) -> Result<StepOutcome, E>,
) -> Result<Vec<StepOutcome>, E> {
    let mut outcomes: Vec<StepOutcome> = Vec::with_capacity(batch.steps.len());
    for step in &batch.steps {
        let runs = matches!(step.gate(&outcomes), Ok(verbatim_model::batch::Gate::Run));
        let outcome = if runs {
            match local_step(&step.op, &outcomes) {
                Some(local) => answer(local)?,
                None => StepOutcome::Failed,
            }
        } else {
            StepOutcome::Skipped
        };
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

/// `op` with its points resolved against the `earlier` outcomes; `None`
/// for a point of a result that has none, which the gate rules out.
fn local_step(op: &Op, earlier: &[StepOutcome]) -> Option<LocalStep> {
    let point = |point: &Point| match *point {
        Point::At(point) => Some(point),
        Point::Of(place, PointOf::ChunkStart) => match earlier.get(place.index()) {
            Some(StepOutcome::Done(verbatim_model::batch::Done::ReadText(read))) => read
                .chunks
                .first()
                .map(|chunk| TextPoint::At(TextPosition::at(chunk.start))),
            _ => None,
        },
        // Text Core holds has no caret it reads by a step.
        Point::Of(..) => None,
    };
    Some(match op {
        Op::ReadText {
            at,
            movement,
            unit,
            count,
            ..
        } => LocalStep::Read {
            at: point(at)?,
            movement: *movement,
            unit: *unit,
            count: u8::try_from(*count).ok()?,
        },
        Op::ReadRange {
            start,
            end,
            through,
            ..
        } => LocalStep::Range(Span {
            start: point(start)?,
            end: point(end)?,
            through: match through {
                Some(through) => Some((point(&through.start)?, point(&through.end)?)),
                None => None,
            },
        }),
        _ => LocalStep::Other,
    })
}

/// The word Core finds itself in text it answers (flat text and a
/// terminal's rows) by its own words ([`crate::text::words`]), a line with
/// none being one empty word at its start: from the word at byte `offset`
/// of line `line`, `count` words on (back when negative), never past the
/// text's ends. `content(line)` is a line's content, `None` past the
/// text. Answers how far it moved, and the line and byte range of the word
/// it reached; `None` when line `line` is not there.
///
/// # Errors
///
/// The first error `content` gives.
pub(crate) fn local_word<E>(
    mut content: impl FnMut(isize) -> Result<Option<String>, E>,
    (line, offset): (isize, usize),
    count: i32,
) -> Result<Option<(i32, isize, Range<usize>)>, E> {
    let words = |text: &str| crate::text::words(text, None);
    let Some(text) = content(line)? else {
        return Ok(None);
    };
    let mut on = words(&text);
    let mut at = (line, crate::text::word_at(&on, offset).unwrap_or(0..0));
    let mut moved = 0;
    while moved != count {
        let forward = count > 0;
        let in_line = if forward {
            on.iter().find(|word| word.start > at.1.start)
        } else {
            on.iter().rev().find(|word| word.start < at.1.start)
        };
        if let Some(word) = in_line {
            at.1 = word.clone();
        } else {
            let next = if forward {
                at.0.saturating_add(1)
            } else {
                at.0.saturating_sub(1)
            };
            let Some(text) = content(next)? else {
                break;
            };
            on = words(&text);
            let landed = if forward { on.first() } else { on.last() };
            at = (next, landed.cloned().unwrap_or(0..0));
        }
        moved = moved.saturating_add(count.signum());
    }
    Ok(Some((moved, at.0, at.1)))
}

/// A node with text: its application, window and role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextNode {
    /// The application whose outpost holds the node.
    pub(crate) process: Pid,
    /// The node's window.
    pub(crate) window: RequestWindow,
    /// The node.
    pub(crate) node: NodeId,
    /// Its role.
    pub(crate) role: Role,
}

/// The committed focus's text, as a caret command finds it at its turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaretTarget {
    /// No focus a reader holds: nothing committed, its reader ended, or
    /// Core's own focus. "No caret".
    NoCaret,
    /// A focus that cannot have text. "Not supported".
    NoText,
    /// The focus's node.
    Text(TextNode),
}

impl CaretTarget {
    /// What the committed focus of `state` gives a caret command: a focus
    /// that may have text by its role, or that Core holds a caret on, has
    /// a caret.
    pub(crate) fn of(state: &SrState) -> Self {
        let Some(focus) = state.versions.live() else {
            return Self::NoCaret;
        };
        let (node, role) = (focus.context.snapshot.id, focus.context.snapshot.role);
        if !crate::editing::may_have_text(role) && state.focus_caret(node).is_none() {
            return Self::NoText;
        }
        Self::Text(TextNode {
            process: focus.context.source,
            window: focus.kept.asked,
            node,
            role,
        })
    }
}
