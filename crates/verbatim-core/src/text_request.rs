//! A read or move in a node's text, written as a batch of the shared list's
//! text operations (the request list's design, section 6, the `TextOp`
//! rows) and judged once into what the command that asked acts on. Each
//! is asked of the node's outpost on the node's window, or answered by
//! Core itself from text it holds: flat text and a terminal's rows
//! ([`answer_steps`]). The commands that read the committed focus's text
//! resolve it here at their turn.

use std::ops::Range;

use verbatim_model::batch::{Batch, Located, Op, Point, PointOf, StepOutcome, StepRefusal};
use verbatim_model::request::Reply;
use verbatim_model::{
    NodeId, Pid, Role, SelectionChange, TextAnchor, TextChunk, TextDetails, TextFailure,
    TextMovement, TextPoint, TextPosition, TextReply, TextUnit,
};

use crate::batch::{Answers, At, Baseline, Got, Sent, batch};
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

/// Which word a review word command reads (`docs/parity/review.md`,
/// "Review words by the provider's word").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WordStep {
    /// The word at the point.
    Current,
    /// The word before it.
    Previous,
    /// The word after it.
    Next,
}

/// What a command reads or does in a node's text, the batch it is written
/// as named by its shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextAsk {
    /// `count` units of `unit` from `at`, after `movement`: one `ReadText`.
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
    /// A review word command at `at`, by the text's own word, one batch a
    /// press (the request list's design, section 6, "`Read`, review's word
    /// commands"): the current word one `ReadText`; the previous word the
    /// word at the point, the word before it and the line it is on; the
    /// next word the word after the point, the text between the point and
    /// it, and its line.
    Word {
        /// The review position.
        at: TextPoint,
        /// Which word.
        step: WordStep,
    },
    /// The text of a span: one `ReadRange`.
    Range(Span),
    /// A span selected, said by how the selection changed: the caret, the
    /// selection, the caret again and the comparison of the two.
    Select(Span),
    /// The caret moved to a point, with the selection baseline the move
    /// leaves: a `Select` of two equal points, then the caret.
    MoveCaret(TextPoint),
    /// Where a point is: one `Locate`.
    Locate(TextPoint),
}

impl TextAsk {
    /// The points Core names in the text.
    pub(crate) fn points(&self) -> Vec<TextPoint> {
        match *self {
            Self::Read { at, .. }
            | Self::Word { at, .. }
            | Self::MoveCaret(at)
            | Self::Locate(at) => vec![at],
            Self::Range(span) | Self::Select(span) => {
                let mut points = vec![span.start, span.end];
                points.extend(span.through.into_iter().flat_map(|(a, b)| [a, b]));
                points
            }
        }
    }
}

/// A word command's answer: the word to say, and, where the review cursor
/// moves, the line it lands on, its offset at the word's start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Worded {
    /// The word said.
    pub(crate) word: TextChunk,
    /// The line landed on, `None` for the current word and for a move
    /// that met the text's edge.
    pub(crate) landed: Option<TextChunk>,
}

/// What a [`TextAsk`] answered, judged once from its batch's outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TextAnswer {
    /// The units read, and how far the movement went.
    Read {
        /// Units moved, signed like the movement.
        moved: i32,
        /// The units.
        chunks: Vec<TextChunk>,
    },
    /// A word command's answer.
    Word(Box<Worded>),
    /// The text of a span.
    Range(String),
    /// A span selected: how the selection changed, with the selection
    /// baseline it left.
    Selected {
        /// The changes.
        changes: Vec<SelectionChange>,
        /// The baseline.
        baseline: TextAnchor,
    },
    /// The caret moved, leaving this selection baseline.
    Moved(TextAnchor),
    /// Where a point is.
    Located(Located),
    /// The text refused it.
    Refused(StepRefusal),
    /// A position it named is no longer held.
    Lost,
    /// The node is gone.
    Gone,
    /// A read failed, or a step was not answered.
    Failed,
}

impl TextAnswer {
    /// A line read's `outcome` as Core's own answers give it, a terminal's
    /// command mark move.
    pub(crate) fn of_line(outcome: StepOutcome) -> Self {
        match outcome {
            StepOutcome::Done(verbatim_model::batch::Done::ReadText(read)) => Self::Read {
                moved: read.moved,
                chunks: read.chunks,
            },
            StepOutcome::Refused(refusal) => Self::Refused(refusal),
            StepOutcome::NotThere(verbatim_model::batch::Missing::Anchor(_)) => Self::Lost,
            StepOutcome::NotThere(_) => Self::Gone,
            StepOutcome::Done(_) | StepOutcome::Failed | StepOutcome::Skipped => Self::Failed,
        }
    }

    /// The answer a step that was not done gives, `None` for one done.
    fn of_step<T>(got: &Got<'_, T>) -> Option<Self> {
        Some(match got {
            Got::Done(_) => return None,
            Got::Refused(refusal) => Self::Refused(*refusal),
            Got::NotThere(verbatim_model::batch::Missing::Anchor(_)) => Self::Lost,
            Got::NotThere(_) => Self::Gone,
            Got::Failed | Got::Skipped => Self::Failed,
        })
    }
}

/// Who answers a text request.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Answerer<'a> {
    /// The node's outpost, on this window.
    Outpost(RequestWindow),
    /// Core, from this flat text ([`crate::review_text::flat_steps`]).
    Flat(&'a str),
    /// Core, from the focused terminal's rows, on this window for the
    /// history reads its answer may need.
    Terminal(RequestWindow),
}

/// Asks `ask` of `node`'s text through `by` under the due time `due_us`:
/// its answer, or `None` when the request was stopped, refused or not
/// answered, which says nothing.
pub(crate) async fn ask(
    cx: &mut Cx<Entries>,
    (node, by): (NodeId, Answerer<'_>),
    ask: TextAsk,
    due_us: u64,
) -> Option<TextAnswer> {
    let plain = TextDetails::NONE;
    match ask {
        TextAsk::Read {
            at: from,
            movement,
            unit,
            count,
        } => {
            let mut sent = batch(|| ());
            let read = sent.read_text(node, At::from(from), movement, unit, count, plain);
            let answers = match answers(cx, (node, by), sent.finish(), due_us).await? {
                Ok(answers) => answers,
                Err(answer) => return Some(answer),
            };
            Some(match answers.get(read) {
                Got::Done(read) => TextAnswer::Read {
                    moved: read.moved,
                    chunks: read.chunks.clone(),
                },
                other => TextAnswer::of_step(&other)?,
            })
        }
        TextAsk::Word { at: from, step } => word(cx, (node, by), (from, step), due_us).await,
        TextAsk::Range(span) => {
            let mut sent = batch(|| ());
            let range = sent.read_range(
                node,
                (At::from(span.start), At::from(span.end)),
                span.through
                    .map(|(start, end)| (At::from(start), At::from(end))),
            );
            let answers = match answers(cx, (node, by), sent.finish(), due_us).await? {
                Ok(answers) => answers,
                Err(answer) => return Some(answer),
            };
            Some(match answers.get(range) {
                Got::Done(range) => TextAnswer::Range(range.text.clone()),
                other => TextAnswer::of_step(&other)?,
            })
        }
        TextAsk::Select(span) => {
            // The request list's design, section 5: the reads before the
            // act and the act are one program, the reads after it another.
            let mut sent = batch(|| ());
            let before = sent.caret(node, None);
            let select = sent.select(
                node,
                (At::from(span.start), At::from(span.end)),
                span.through
                    .map(|(start, end)| (At::from(start), At::from(end))),
            );
            let after = sent.caret(node, None);
            let compared = sent.compare_selection(node, Baseline::Read(before), after);
            let answers = match answers(cx, (node, by), sent.finish(), due_us).await? {
                Ok(answers) => answers,
                Err(answer) => return Some(answer),
            };
            Some(match (answers.get(after), answers.get(compared)) {
                (Got::Done(after), Got::Done(compared)) => TextAnswer::Selected {
                    changes: compared.difference.changes.clone(),
                    baseline: after.held,
                },
                _ => TextAnswer::of_step(&answers.get(select))
                    .or_else(|| TextAnswer::of_step(&answers.get(before)))
                    .unwrap_or(TextAnswer::Failed),
            })
        }
        TextAsk::MoveCaret(to) => {
            let mut sent = batch(|| ());
            let select = sent.select(node, (At::from(to), At::from(to)), None);
            let caret = sent.caret(node, None);
            let answers = match answers(cx, (node, by), sent.finish(), due_us).await? {
                Ok(answers) => answers,
                Err(answer) => return Some(answer),
            };
            Some(match answers.get(caret) {
                Got::Done(caret) => TextAnswer::Moved(caret.held),
                other => TextAnswer::of_step(&answers.get(select))
                    .or_else(|| TextAnswer::of_step(&other))
                    .unwrap_or(TextAnswer::Failed),
            })
        }
        TextAsk::Locate(point) => {
            let mut sent = batch(|| ());
            let located = sent.locate(node, At::from(point));
            let answers = match answers(cx, (node, by), sent.finish(), due_us).await? {
                Ok(answers) => answers,
                Err(answer) => return Some(answer),
            };
            Some(match answers.get(located) {
                Got::Done(located) => TextAnswer::Located(*located),
                other => TextAnswer::of_step(&other)?,
            })
        }
    }
}

/// A review word command's batch, asked and judged (the request list's
/// design, section 6, "The word rows' speech"): a move that went no
/// word, or a next word that does not start after the review position
/// (a provider that moves onto the text's end), keeps the position and
/// says step 0's word.
async fn word(
    cx: &mut Cx<Entries>,
    (node, by): (NodeId, Answerer<'_>),
    (from, step): (TextPoint, WordStep),
    due_us: u64,
) -> Option<TextAnswer> {
    let plain = TextDetails::NONE;
    let by_word = |count| {
        Some(TextMovement {
            unit: TextUnit::Word,
            count,
        })
    };
    let mut sent = batch(|| ());
    let first = match step {
        WordStep::Current | WordStep::Previous => {
            sent.read_text(node, from.into(), None, TextUnit::Word, 1, plain)
        }
        WordStep::Next => sent.read_text(node, from.into(), by_word(1), TextUnit::Word, 1, plain),
    };
    let (moved_to, between, line) = match step {
        WordStep::Current => (None, None, None),
        WordStep::Previous => {
            let before = sent.read_text(node, first.start(), by_word(-1), TextUnit::Word, 1, plain);
            let line = sent.read_text(node, before.start(), None, TextUnit::Line, 1, plain);
            (Some(before), None, Some(line))
        }
        WordStep::Next => {
            let between = sent.read_range(node, (from.into(), first.start()), None);
            let line = sent.read_text(node, first.start(), None, TextUnit::Line, 1, plain);
            (None, Some(between), Some(line))
        }
    };
    let answers = match answers(cx, (node, by), sent.finish(), due_us).await? {
        Ok(answers) => answers,
        Err(answer) => return Some(answer),
    };
    let chunk = |got: Got<'_, verbatim_model::batch::Chunks>| match got {
        Got::Done(read) => read
            .chunks
            .first()
            .map(|chunk| (read.moved, chunk.clone()))
            .ok_or(TextAnswer::Failed),
        other => Err(TextAnswer::of_step(&other).unwrap_or(TextAnswer::Failed)),
    };
    let judged = (|| {
        let (first_moved, first) = chunk(answers.get(first))?;
        let Some(line) = line else {
            return Ok(Worded {
                word: first,
                landed: None,
            });
        };
        let (moved, word) = match moved_to {
            Some(moved_to) => chunk(answers.get(moved_to))?,
            None => (first_moved, first.clone()),
        };
        // NVDA's check against a provider that moves onto the text's end:
        // the word reached starts after the review position.
        let after = match between.map(|between| answers.get(between)) {
            None => true,
            Some(Got::Done(range)) => !range.text.is_empty(),
            Some(other) => {
                return Err(TextAnswer::of_step(&other).unwrap_or(TextAnswer::Failed));
            }
        };
        if moved == 0 || !after {
            return Ok(Worded {
                word: first,
                landed: None,
            });
        }
        let (_, line) = chunk(answers.get(line))?;
        Ok(Worded {
            word,
            landed: Some(line),
        })
    })();
    Some(judged.map_or_else(|answer| answer, |worded| TextAnswer::Word(Box::new(worded))))
}

/// `sent`'s answers from `by`, `node`'s answerer: `Err` with the answer
/// for the text not there; `None` when the request was stopped, refused
/// or not answered, or a local answer did not fit the batch, a defect.
async fn answers<B: 'static>(
    cx: &mut Cx<Entries>,
    (node, by): (NodeId, Answerer<'_>),
    sent: Sent<B>,
    due_us: u64,
) -> Option<Result<Answers<B>, TextAnswer>> {
    let outcomes = match by {
        Answerer::Outpost(window) => {
            let asked = sent.of(Reader::Outpost(window.process), window);
            return match cx.ask(asked, due_us).await {
                Outcome::Result(answers) => Some(Ok(answers)),
                Outcome::NotThere => Some(Err(TextAnswer::Gone)),
                _ => None,
            };
        }
        Answerer::Flat(text) => crate::review_text::flat_steps(text, sent.batch()),
        Answerer::Terminal(window) => {
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
            count: *count,
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
