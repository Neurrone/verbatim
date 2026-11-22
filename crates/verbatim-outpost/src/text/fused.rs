//! A run of a batch's fusable text steps on one UI Automation node, through
//! the program builder (the verbatim-outpost crate guide, "Text and
//! dialog text"): the model's steps run as one segment by
//! [`verbatim_uia_rops::batch::run_batch`], what they name that the
//! outpost holds resolved against the node's anchors and the batch's
//! earlier results ([`TextResolver`]), and each step's result made the
//! model's, its points held as anchors and its chunks minted with their
//! formatting.
//!
//! A `Caret`'s read is held as the next selection baseline, with the
//! selection's text a later `CompareSelection` of the same run read for
//! it.

#![forbid(unsafe_code)]

use windows::Win32::UI::Accessibility::IUIAutomationTextRange;

use verbatim_model::batch::{
    Before, CaretAt, CaretWait, Check, Chunks, Condition, Done, Edges, Gate, HeldPoint,
    HeldSelection, KindMismatch, Located, Missing, Op, Place, Point, PointOf, RangeText, Selected,
    SelectionCompared, Step, StepOutcome, StepRefusal, Subject, Waited,
};
use verbatim_model::{
    CaretSelection, MAX_RANGE_BYTES, MAX_READ_AHEAD_TEXT, NodeId, Selection, SelectionDifference,
    TextAnchor, TextChunk, TextDetails, TextMovement, TextPoint, TextPosition, TextUnit,
};
use verbatim_uia::Uia;
use verbatim_uia::text::Endpoint;
use verbatim_uia_rops::batch::{
    Compared, Found, HeldAt, Ran, Resolve, Segment, UnitsFound, UnitsRead, Unrun, WaitCheck,
};
use verbatim_uia_rops::{FoundPoint, Movement, Position, TextTarget};

use super::uia::{UiaPos, UiaText, character_counts, round, unit_text};
use super::{
    CaretRead, CaretState, MAX_RANGE_UNITS, NodeText, PointFrom, SelectionStart, TextError,
    TextResult, change_of, chunk_from, keep_whole_characters, to_utf8,
};

/// What a run of steps answered: each step's outcome, in order, and
/// whether a program could not run at all, so the window is read
/// classically from now on.
pub struct FusedAnswer {
    /// Each step's outcome.
    pub outcomes: Vec<StepOutcome>,
    /// A program could not run at all.
    pub read_classically: bool,
}

/// The anchors `steps`, the run starting at place `first`, name, which the
/// copy of the node's text facts must hold: the points Core names, the
/// baselines compared with, and the points of the earlier results
/// (`earlier`) the steps take.
#[must_use]
pub fn named(steps: &[Step], first: usize, earlier: &[StepOutcome]) -> Vec<u64> {
    let mut points: Vec<Point> = Vec::new();
    let mut named = Vec::new();
    for step in steps {
        match &step.op {
            Op::ReadText { at, .. } | Op::Locate { at, .. } => points.push(*at),
            Op::ReadRange {
                start,
                end,
                through,
                ..
            }
            | Op::Select {
                start,
                end,
                through,
                ..
            } => {
                points.extend([*start, *end]);
                if let Some(through) = through {
                    points.extend([through.start, through.end]);
                }
            }
            Op::CompareSelection {
                before: Before::Baseline(anchor),
                ..
            } => named.push(anchor.0),
            Op::CompareSelection {
                before: Before::SelectionOf(place),
                ..
            } => {
                if let Some(StepOutcome::Done(Done::Caret(caret))) = earlier.get(place.index()) {
                    named.push(caret.held.0);
                }
            }
            _ => {}
        }
    }
    named.extend(
        points
            .into_iter()
            .filter_map(|point| outside_anchor(point, first, earlier))
            .map(|anchor| anchor.0),
    );
    named
}

/// The anchor a point outside the run names: a position Core names, or a
/// point of an earlier segment's result; `None` for a point of the run
/// or one with no anchor.
fn outside_anchor(point: Point, first: usize, earlier: &[StepOutcome]) -> Option<TextAnchor> {
    match point {
        Point::At(TextPoint::At(position)) => Some(position.anchor),
        Point::At(_) => None,
        Point::Of(place, _) if place.index() >= first => None,
        Point::Of(place, which) => earlier_anchor(earlier.get(place.index())?, which),
    }
}

/// The anchor point `which` of an earlier step's outcome holds.
fn earlier_anchor(outcome: &StepOutcome, which: PointOf) -> Option<TextAnchor> {
    match (outcome, which) {
        (StepOutcome::Done(Done::Caret(caret)), PointOf::Caret | PointOf::ChunkStart) => {
            Some(caret.caret.anchor)
        }
        (StepOutcome::Done(Done::Caret(caret)), PointOf::SelectionStart) => Some(
            caret
                .selection
                .map_or(caret.caret.anchor, |selection| selection.start.anchor),
        ),
        (StepOutcome::Done(Done::Caret(caret)), PointOf::SelectionEnd) => Some(
            caret
                .selection
                .map_or(caret.caret.anchor, |selection| selection.end.anchor),
        ),
        (StepOutcome::Done(Done::ReadText(read)), PointOf::ChunkStart) => {
            read.chunks.first().map(|chunk| chunk.start)
        }
        _ => None,
    }
}

/// The position a point outside the run names: a position Core names, or
/// a point of an earlier segment's result as the anchor it holds.
fn outside_position(point: &Point, earlier: &[StepOutcome]) -> Option<TextPosition> {
    match *point {
        Point::At(TextPoint::At(position)) => Some(position),
        Point::Of(place, which) => earlier
            .get(place.index())
            .and_then(|outcome| earlier_anchor(outcome, which))
            .map(TextPosition::at),
        Point::At(_) => None,
    }
}

/// A position held, as the program builder takes it.
fn found_point(pos: &UiaPos) -> Result<FoundPoint, Ran> {
    let (range, endpoint) = pos.range_end().map_err(|_| Ran::Failed)?;
    Ok(FoundPoint {
        range,
        endpoint,
        collapsed: pos.collapsed(),
    })
}

/// A defect in a batch the parse let through, logged: its step fails.
fn defect(what: &'static str) -> Ran {
    tracing::error!(target: verbatim_model::DEFECT_TARGET, defect = what, "a batch's text step the outpost cannot run");
    Ran::Failed
}

/// A caret wait's starting point, for its poll's check: the caret and the
/// selection's ends, as ranges and their ends.
struct Starting {
    points: [(IUIAutomationTextRange, Endpoint, bool); 3],
    word: bool,
    last: bool,
}

/// A held range's end as a position a check compares with.
fn position(
    (range, endpoint, collapsed): &(IUIAutomationTextRange, Endpoint, bool),
) -> Position<'_> {
    Position {
        range,
        endpoint: *endpoint,
        collapsed: *collapsed,
    }
}

/// What a run of text steps names that the outpost holds: node `node`'s
/// text and anchors, the outcomes of the batch's steps before the run,
/// and for a caret wait's poll its starting point.
struct TextResolver<'r, 'n> {
    node: NodeId,
    source: &'r UiaText,
    anchors: &'r NodeText<'n, UiaPos>,
    earlier: &'r [StepOutcome],
    starting: Option<Starting>,
}

impl Resolve for TextResolver<'_, '_> {
    fn text(&self, subject: &Subject) -> Result<TextTarget<'_>, Ran> {
        node_text(self.node, self.source, subject)
    }

    fn point(&self, point: &Point) -> Result<HeldAt, Ran> {
        let position = outside_position(point, self.earlier)
            .ok_or_else(|| defect("a point names a step that holds none"))?;
        let (pos, prefix) = match self.anchors.point_from(position) {
            None => {
                return Err(Ran::Unrun(Unrun::NotThere(Missing::Anchor(
                    position.anchor,
                ))));
            }
            Some(PointFrom::At(pos)) => (pos, Vec::new()),
            Some(PointFrom::After(pos, prefix)) => (pos, prefix),
        };
        Ok(HeldAt {
            at: found_point(&pos)?,
            counts: if prefix.is_empty() {
                Vec::new()
            } else {
                character_counts(&prefix)
            },
            prefix,
        })
    }

    /// The baseline held under the anchor, as a comparison takes it: its
    /// selection's range to its end, or its caret when nothing was
    /// selected. A held selection's two ends are the two ends of one
    /// range, as every read that holds one reads it.
    fn baseline(&self, before: &Before) -> Result<FoundPoint, Ran> {
        let anchor = baseline_anchor(before, self.earlier)
            .ok_or_else(|| defect("a comparison names no caret read"))?;
        let Some(read) = self.anchors.held_read(anchor) else {
            return Err(Ran::Unrun(Unrun::NotThere(Missing::Anchor(anchor))));
        };
        let kept = read.kept_selection();
        Ok(FoundPoint {
            collapsed: kept.collapsed,
            ..found_point(&kept.ends.1)?
        })
    }

    fn units(
        &self,
        read: (Option<TextMovement>, TextUnit, u8),
        details: TextDetails,
    ) -> Result<UnitsRead, Ran> {
        units_read(self.source, read, (details, true))
    }

    fn wait_check(&self) -> Result<WaitCheck<'_>, Ran> {
        let starting = self
            .starting
            .as_ref()
            .ok_or_else(|| defect("a caret wait outside a poll"))?;
        Ok(WaitCheck {
            text: self.source.target(),
            start: position(&starting.points[0]),
            start_selection: (position(&starting.points[1]), position(&starting.points[2])),
            word: starting.word,
            last: starting.last,
        })
    }
}

/// The text of node `node` that a text step's `subject` names.
fn node_text<'s>(
    node: NodeId,
    source: &'s UiaText,
    subject: &Subject,
) -> Result<TextTarget<'s>, Ran> {
    match subject {
        Subject::Node(named) if *named == node => Ok(source.target()),
        _ => Err(defect("a text step of another subject in a node's run")),
    }
}

/// A read's units as the program reads them, each unit's language read
/// when `culture`; the refusal when the text has no such unit.
pub(crate) fn units_read(
    source: &UiaText,
    (movement, unit, count): (Option<TextMovement>, TextUnit, u8),
    (details, culture): (TextDetails, bool),
) -> Result<UnitsRead, Ran> {
    let no_such_unit = Ran::Unrun(Unrun::Refused(StepRefusal::NoSuchUnit));
    // UI Automation has no sentence unit, and a document is not read as
    // a unit.
    if matches!(unit, TextUnit::Document | TextUnit::Sentence) {
        return Err(no_such_unit);
    }
    let Some(by) = source.provider_unit(unit) else {
        return Err(no_such_unit);
    };
    let movement = match movement {
        None => None,
        Some(TextMovement {
            unit: TextUnit::Document,
            count,
        }) => Some(Movement::Document(count)),
        Some(TextMovement { unit, count }) => match source.provider_unit(unit) {
            Some(unit) => Some(Movement::By(unit, count)),
            None => return Err(no_such_unit),
        },
    };
    let (formats, learning) = source.format_reads(details);
    Ok(UnitsRead {
        movement,
        unit: by,
        count: u32::from(count.max(1)),
        max_total: u32::try_from(MAX_READ_AHEAD_TEXT).unwrap_or(u32::MAX),
        culture,
        formats,
        learning,
    })
}

/// What a focus's caret line names: the node's text alone, its line read
/// with no language, which a caret report does not carry.
struct LineResolver<'r> {
    node: NodeId,
    source: &'r UiaText,
}

impl Resolve for LineResolver<'_> {
    fn text(&self, subject: &Subject) -> Result<TextTarget<'_>, Ran> {
        node_text(self.node, self.source, subject)
    }

    fn units(
        &self,
        read: (Option<TextMovement>, TextUnit, u8),
        details: TextDetails,
    ) -> Result<UnitsRead, Ran> {
        units_read(self.source, read, (details, false))
    }
}

/// A focus's caret, its line with the formatting the source's details
/// name, and the selection's text, read as one segment of the program
/// builder: the request list's `Caret`, `ReadText` of the line at the
/// caret and `ReadRange` of the selection, the last only when text is
/// selected, so an empty range, whose text some providers fail to give,
/// is never read (Claude, 2026-11-22), its steps naming `node`, with
/// where the selection starts, the caret being its start (the
/// verbatim-outpost crate guide, "Text and dialog text"), so no caret
/// program is written by hand (Claude, 2026-11-22).
///
/// # Errors
///
/// [`TextError::Gone`] when the node is gone; [`TextError::Failed`] when a
/// step could not be read.
pub fn caret_line(
    uia: &Uia,
    (node, source): (NodeId, &mut UiaText),
) -> TextResult<CaretRead<UiaPos>> {
    let subject = Subject::Node(node);
    let of = |which| Point::Of(Place(0), which);
    let step = |op| Step { op, when: None };
    let steps = [
        step(Op::Caret {
            subject: subject.clone(),
            adopt: None,
        }),
        step(Op::ReadText {
            subject: subject.clone(),
            at: of(PointOf::Caret),
            movement: None,
            unit: TextUnit::Line,
            count: 1,
            details: source.details_read(),
        }),
        Step {
            op: Op::ReadRange {
                subject,
                start: of(PointOf::SelectionStart),
                end: of(PointOf::SelectionEnd),
                through: None,
            },
            when: Some(Condition {
                on: Place(0),
                check: Check::Found,
                negated: false,
            }),
        },
    ];
    let (_, learning) = source.format_reads(source.details_read());
    let answer = {
        let resolver = LineResolver { node, source };
        verbatim_uia_rops::batch::run_batch(&Segment {
            uia,
            steps: &steps,
            first: 0,
            earlier: &[],
            resolver: &resolver,
            remote: source.tries_remote(),
            window: source.window_of(),
        })
    };
    source.note_read_classically(answer.read_classically);
    let failed = |ran: Ran| match ran {
        Ran::Gone => TextError::Gone,
        other => TextError::Failed(format!("a focus's caret line was not read: {other:?}")),
    };
    let mut ran = answer.steps.into_iter();
    let (Some(caret), Some(line), Some(selected)) = (ran.next(), ran.next(), ran.next()) else {
        return Err(TextError::Failed(
            "a focus's caret line answered no steps".to_owned(),
        ));
    };
    let Ran::Done(Found::Caret(caret)) = caret else {
        return Err(failed(caret));
    };
    let Ran::Done(Found::ReadText(mut line)) = line else {
        return Err(failed(line));
    };
    // The selection is read exactly when the caret found one.
    let selected = match (caret.selection.is_some(), selected) {
        (true, Ran::Done(Found::ReadRange(mut selected))) => {
            selected.truncate(MAX_RANGE_UNITS);
            Some(selected)
        }
        (false, Ran::Unrun(Unrun::Skipped)) => None,
        (_, selected) => return Err(failed(selected)),
    };
    source.learned(learning, line.unsupported);
    let selection = match &caret.selection {
        Some((range, start, _)) => Some((
            (
                UiaPos::new(range, Endpoint::Start, false)?,
                UiaPos::new(range, Endpoint::End, false)?,
            ),
            start.start,
        )),
        None => None,
    };
    let unit = line.units.swap_remove(0);
    // The selection starts at the caret, whose place in the line the read
    // gives before it is cut to a chunk's limit.
    let in_line = selection.as_ref().map(|_| unit.offset);
    let read = unit_text(unit)?;
    Ok(CaretRead {
        state: CaretState {
            caret: UiaPos::new(&caret.caret, Endpoint::Start, true)?,
            selection: selection.as_ref().map(|(ends, _)| ends.clone()),
        },
        line: (read.unit, read.offset),
        formats: UiaText::formats_read(line.runs, source.details_read(), None),
        selection_text: selected,
        selection_start: SelectionStart {
            in_line,
            first: selection.is_some_and(|(_, first)| first),
        },
    })
}

/// The anchor of the baseline a comparison starts from outside the run:
/// one Core names, or the read an earlier segment's `Caret` held.
fn baseline_anchor(before: &Before, earlier: &[StepOutcome]) -> Option<TextAnchor> {
    match *before {
        Before::Baseline(anchor) => Some(anchor),
        Before::SelectionOf(place) => match earlier.get(place.index()) {
            Some(StepOutcome::Done(Done::Caret(caret))) => Some(caret.held),
            _ => None,
        },
    }
}

/// Runs `steps`, the run of fusable text steps starting at place `first`,
/// on `source`, node `node`'s text whose anchors are `anchors`, `earlier`
/// being the outcomes of the batch's steps before it; `window` names the
/// window for the fallback's log, and `read_at_us` is when the reads
/// began.
pub fn run(
    uia: &Uia,
    (node, source, anchors): (NodeId, &mut UiaText, &mut NodeText<'_, UiaPos>),
    (steps, first, earlier): (&[Step], usize, &[StepOutcome]),
    (window, read_at_us): (Option<isize>, u64),
) -> FusedAnswer {
    let began_us = verbatim_process::clock::now_us();
    let answer = {
        let resolver = TextResolver {
            node,
            source,
            anchors,
            earlier,
            starting: None,
        };
        verbatim_uia_rops::batch::run_batch(&Segment {
            uia,
            steps,
            first,
            earlier,
            resolver: &resolver,
            remote: source.tries_remote(),
            window,
        })
    };
    let mut made = Made {
        node,
        source,
        anchors,
        first,
        earlier,
        read_at_us,
        carets: Vec::new(),
    };
    let texts = selection_texts(steps, &answer.steps, first);
    let outcomes = steps
        .iter()
        .zip(answer.steps)
        .enumerate()
        .map(|(index, (step, ran))| made.outcome(index, step, ran, (&texts, began_us)))
        .collect();
    FusedAnswer {
        outcomes,
        read_classically: answer.read_classically,
    }
}

/// How a poll of a caret wait ended it, and whether a program could not
/// run at all, so the window is read classically from now on.
pub struct PollAnswer {
    /// `None` when the check showed no effect before the bound; else how
    /// the wait ended, with the outcomes of the batch's later steps when
    /// the poll's program read them.
    pub ended: Option<(Waited, Option<Vec<StepOutcome>>)>,
    /// A program could not run at all.
    pub read_classically: bool,
}

/// One poll of a caret wait (the request list's design, section 5,
/// `WaitForCaret`): one program, the check of the caret against the
/// starting point held under `wait`'s anchor followed by the fused run of
/// `later`, the batch's steps after the wait, which read only when the caret or the
/// selection moved or at the bound (`last`), so the poll that ends the
/// wait takes no program of its own for them. The check is judged by `wait`'s effect test
/// ([`super::check::effect_of`]); a check whose caret moved while it read
/// runs again, as the check by parts does. The later steps' results are
/// taken only when the wait ends, each under its own condition on how it
/// ended; with the effect shown only by the text, their reads did not
/// run, and the caller runs them. Later steps that are not all fusable
/// text steps of `node` without an act, under no condition but on the
/// wait's effect, are left to the caller whole.
///
/// # Errors
///
/// [`TextError::Gone`] when the node or the starting point is gone;
/// [`TextError::Failed`] when the check could not be read, which counts
/// as no effect.
pub fn poll(
    uia: &Uia,
    (node, source, anchors): (NodeId, &mut UiaText, &mut NodeText<'_, UiaPos>),
    (wait, word, last): (&CaretWait, bool, bool),
    later: &[Step],
    (window, read_at_us): (Option<isize>, u64),
) -> TextResult<PollAnswer> {
    let start = anchors.held_read(wait.from).ok_or(TextError::Gone)?;
    let (was_start, was_end) = start.selection.as_ref().map_or_else(
        || (start.caret.clone(), start.caret.clone()),
        |selection| (selection.start.clone(), selection.end.clone()),
    );
    let point = |pos: &UiaPos| -> TextResult<(IUIAutomationTextRange, Endpoint, bool)> {
        let (range, endpoint) = pos.range_end()?;
        Ok((range, endpoint, pos.collapsed()))
    };
    let starting = Starting {
        points: [point(&start.caret)?, point(&was_start)?, point(&was_end)?],
        word,
        last,
    };
    let fused_later = later.iter().all(|step| fusable_in_poll(step, node));
    let gate = Condition {
        on: Place(0),
        check: Check::Found,
        negated: false,
    };
    let mut polled = vec![Step {
        op: Op::WaitForCaret {
            subject: Subject::Node(node),
            wait: Box::new(wait.clone()),
        },
        when: None,
    }];
    if fused_later {
        polled.extend(later.iter().map(|step| Step {
            op: step.op.clone(),
            when: Some(gate),
        }));
    }
    let (ended, read_classically) = {
        let resolver = TextResolver {
            node,
            source,
            anchors,
            earlier: &[],
            starting: Some(starting),
        };
        run_poll(
            &Segment {
                uia,
                steps: &polled,
                first: 0,
                earlier: &[],
                resolver: &resolver,
                remote: source.tries_remote(),
                window,
            },
            wait,
            last,
        )?
    };
    let Some(PollRan {
        waited,
        checked,
        ran,
        began_us,
    }) = ended
    else {
        return Ok(PollAnswer {
            ended: None,
            read_classically,
        });
    };
    if !fused_later || !checked.reads_after {
        return Ok(PollAnswer {
            ended: Some((waited, None)),
            read_classically,
        });
    }
    let mut made = Made {
        node,
        source,
        anchors,
        first: 1,
        earlier: &[],
        read_at_us,
        carets: Vec::new(),
    };
    let outcomes = made.later(waited, later, (ran, began_us));
    Ok(PollAnswer {
        ended: Some((waited, Some(outcomes))),
        read_classically,
    })
}

/// A poll's program that ended the wait.
struct PollRan {
    waited: Waited,
    checked: verbatim_uia_rops::batch::Checked,
    /// The later steps' outcomes as run.
    ran: Vec<Ran>,
    /// When the program began.
    began_us: u64,
}

/// Runs a poll's `segment`, again while its caret moved as it read, at
/// most [`super::check::TORN_READS`] times: the run that ended the wait
/// judged by `wait`'s effect test, `None` while the wait goes on, and
/// whether a program could not run at all.
fn run_poll(
    segment: &Segment<'_>,
    wait: &CaretWait,
    last: bool,
) -> TextResult<(Option<PollRan>, bool)> {
    let mut read_classically = false;
    let mut tries = 0;
    loop {
        let began_us = verbatim_process::clock::now_us();
        let answer = verbatim_uia_rops::batch::run_batch(segment);
        read_classically |= answer.read_classically;
        let mut ran = answer.steps.into_iter();
        let checked = match ran.next() {
            Some(Ran::Done(Found::WaitCheck(checked))) => checked,
            Some(Ran::Gone) => return Err(TextError::Gone),
            other => {
                return Err(TextError::Failed(format!(
                    "the wait's check was not read: {other:?}"
                )));
            }
        };
        let read = &checked.answer;
        let effect = super::check::effect_of(
            wait,
            (read.caret_moved, read.selection_moved),
            (&read.line.text, read.line.offset),
            read.word.as_ref().map(|word| word.text.as_slice()),
        );
        if !effect && !last {
            return Ok((None, read_classically));
        }
        if read.moved_while_read {
            tries += 1;
            if tries < super::check::TORN_READS {
                continue;
            }
            if !last {
                return Ok((None, read_classically));
            }
        }
        let waited = if effect {
            Waited::Effect
        } else {
            Waited::Bound
        };
        let ended = PollRan {
            waited,
            checked,
            ran: ran.collect(),
            began_us,
        };
        return Ok((Some(ended), read_classically));
    }
}

/// Whether `step`, a step of a caret wait's batch after the wait, is read
/// in each poll's program: a fusable text step of `node` that is no act,
/// under no condition but the wait's effect.
fn fusable_in_poll(step: &Step, node: NodeId) -> bool {
    let on_node = matches!(step.op.subject(), Some(Subject::Node(subject)) if *subject == node);
    let fusable = matches!(
        step.op,
        Op::Caret { .. }
            | Op::ReadText { .. }
            | Op::ReadRange { .. }
            | Op::CompareSelection { .. }
            | Op::Locate { .. }
    );
    let condition = match step.when {
        None => true,
        Some(Condition {
            on,
            check: Check::Found,
            negated: false,
        }) => on == Place(0),
        Some(_) => false,
    };
    on_node && fusable && condition
}

/// The selection text a later `CompareSelection` read for each `Caret` of
/// the run, by the caret's index in the run.
fn selection_texts(steps: &[Step], ran: &[Ran], first: usize) -> Vec<Option<Vec<u16>>> {
    let mut texts = vec![None; steps.len()];
    for (step, ran) in steps.iter().zip(ran) {
        if let (
            Op::CompareSelection { after, .. },
            Ran::Done(Found::CompareSelection(Compared {
                selection_text: Some(text),
                ..
            })),
        ) = (&step.op, ran)
            && let Some(slot) = after
                .index()
                .checked_sub(first)
                .and_then(|index| texts.get_mut(index))
        {
            *slot = Some(text.clone());
        }
    }
    texts
}

/// A `Caret`'s caret and selection, as the source's positions.
type CaretHeld = (UiaPos, Option<(UiaPos, UiaPos)>);

/// The results of a run made the model's.
struct Made<'m, 'n> {
    node: NodeId,
    source: &'m mut UiaText,
    anchors: &'m mut NodeText<'n, UiaPos>,
    first: usize,
    earlier: &'m [StepOutcome],
    read_at_us: u64,
    /// Each `Caret` of the run's caret and selection, by its index in the
    /// run, for the points later steps read from and the comparisons'
    /// whole new selection.
    carets: Vec<Option<CaretHeld>>,
}

impl Made<'_, '_> {
    /// The outcome of `step`, at `index` in the run, that ran as `ran`;
    /// `texts` holds the selections' texts later comparisons read, and
    /// `began_us` when the run began.
    fn outcome(
        &mut self,
        index: usize,
        step: &Step,
        ran: Ran,
        (texts, began_us): (&[Option<Vec<u16>>], u64),
    ) -> StepOutcome {
        self.carets.push(None);
        let done = match (&step.op, ran) {
            (_, Ran::TimedOut | Ran::Failed) => return StepOutcome::Failed,
            (_, Ran::Unrun(Unrun::Refused(refusal))) => return StepOutcome::Refused(refusal),
            (_, Ran::Unrun(Unrun::NotThere(missing))) => return StepOutcome::NotThere(missing),
            (_, Ran::Unrun(Unrun::Skipped)) => return StepOutcome::Skipped,
            (_, Ran::Gone) => return StepOutcome::NotThere(Missing::Node(self.node)),
            (Op::Caret { .. }, Ran::Done(Found::Caret(found))) => {
                self.caret(index, &found, texts.get(index).cloned().flatten())
            }
            (
                Op::ReadText {
                    at, unit, details, ..
                },
                Ran::Done(Found::ReadText(found)),
            ) => self.read_text(at, *unit, *details, found),
            (Op::ReadRange { .. }, Ran::Done(Found::ReadRange(units))) => {
                Ok(Done::ReadRange(range_text(&units)))
            }
            (Op::CompareSelection { before, .. }, Ran::Done(Found::CompareSelection(compared))) => {
                Ok(self.compared(before, compared))
            }
            (Op::Select { .. }, Ran::Done(Found::Select)) => {
                Ok(Done::Select(Selected { began_us }))
            }
            (Op::Locate { .. }, Ran::Done(Found::Locate(screen))) => Ok(Done::Locate(Located {
                screen: screen.map(|(x, y)| (round(x), round(y))),
                offset: None,
            })),
            (_, Ran::Done(found)) => {
                tracing::error!(target: verbatim_model::DEFECT_TARGET, ?found, "a text step answered with another operation's result");
                return StepOutcome::Failed;
            }
        };
        done.map_or_else(
            |error| {
                tracing::debug!(?error, "a batch's text result could not be held");
                StepOutcome::Failed
            },
            StepOutcome::Done,
        )
    }

    /// The outcomes of a caret wait's `later` steps, which a poll that
    /// ended the wait `waited` ran as `ran` from `began_us`: each read under
    /// the check's gate in the program, decided now under its own condition
    /// on how the wait ended.
    fn later(
        &mut self,
        waited: Waited,
        later: &[Step],
        (ran, began_us): (Vec<Ran>, u64),
    ) -> Vec<StepOutcome> {
        let texts = selection_texts(later, &ran, self.first);
        let mut outcomes = vec![StepOutcome::Done(Done::WaitForCaret(waited))];
        for (index, (step, ran)) in later.iter().zip(ran).enumerate() {
            let outcome = match step.gate(&outcomes) {
                Ok(Gate::Run) => self.outcome(index, step, ran, (&texts, began_us)),
                Ok(Gate::Skip) => {
                    self.carets.push(None);
                    StepOutcome::Skipped
                }
                Err(KindMismatch) => {
                    self.carets.push(None);
                    tracing::error!(target: verbatim_model::DEFECT_TARGET, ?step, "a step's condition names a result it cannot test");
                    StepOutcome::Failed
                }
            };
            outcomes.push(outcome);
        }
        outcomes.remove(0);
        outcomes
    }

    /// A `Caret`'s result: its points held, and its read held as the next
    /// selection baseline with the selection's `text` when read.
    fn caret(
        &mut self,
        index: usize,
        found: &verbatim_uia_rops::batch::CaretFound,
        text: Option<Vec<u16>>,
    ) -> TextResult<Done> {
        let caret = UiaPos::new(&found.caret, Endpoint::Start, true)?;
        let selection = match &found.selection {
            Some((range, start, end)) => Some((
                (
                    UiaPos::new(range, Endpoint::Start, false)?,
                    UiaPos::new(range, Endpoint::End, false)?,
                ),
                (*start, *end),
            )),
            None => None,
        };
        let held_point =
            |anchors: &mut NodeText<'_, UiaPos>, pos: &UiaPos, edges: Edges| HeldPoint {
                anchor: anchors.hold_point(pos.clone()),
                edges,
            };
        let caret_point = held_point(self.anchors, &caret, found.edges);
        let held_selection = selection
            .as_ref()
            .map(|((start, end), (start_edges, end_edges))| HeldSelection {
                start: held_point(self.anchors, start, *start_edges),
                end: held_point(self.anchors, end, *end_edges),
            });
        let ends = selection.as_ref().map(|(ends, _)| ends.clone());
        let kept = ends.clone().zip(held_selection).map(|(ends, held)| {
            let report = CaretSelection {
                range: Selection {
                    start: TextPosition::at(held.start.anchor),
                    end: TextPosition::at(held.end.anchor),
                },
            };
            (ends, report, text)
        });
        let held = self
            .anchors
            .hold_caret(caret.clone(), kept, self.read_at_us);
        if let Some(slot) = self.carets.get_mut(index) {
            *slot = Some((caret, ends));
        }
        Ok(Done::Caret(CaretAt {
            caret: caret_point,
            selection: held_selection,
            held,
            offsets: None,
        }))
    }

    /// The source's position of a point a step of the run read from, when
    /// known without a call.
    fn known(&self, at: &Point) -> Option<UiaPos> {
        match at {
            Point::Of(place, which) if place.index() >= self.first => {
                let (caret, selection) = self.carets.get(place.index() - self.first)?.as_ref()?;
                Some(match (which, selection) {
                    (PointOf::SelectionStart, Some((start, _))) => start.clone(),
                    (PointOf::SelectionEnd, Some((_, end))) => end.clone(),
                    (PointOf::ChunkStart, _) => return None,
                    _ => caret.clone(),
                })
            }
            point => match self
                .anchors
                .point_from(outside_position(point, self.earlier)?)?
            {
                PointFrom::At(pos) => Some(pos),
                PointFrom::After(..) => None,
            },
        }
    }

    /// A `ReadText`'s result: each unit a chunk reported as `kind`, its
    /// anchor minted, the first with its formatting read for `details`.
    fn read_text(
        &mut self,
        at: &Point,
        kind: TextUnit,
        details: TextDetails,
        found: UnitsFound,
    ) -> TextResult<Done> {
        let (formats_read, learning) = self.source.format_reads(details);
        let read_formats = !formats_read.is_empty();
        self.source.learned(learning, found.unsupported);
        let from = if found.moved == 0 {
            self.known(at)
        } else {
            None
        };
        let mut runs = Some(found.runs);
        let mut chunks: Vec<TextChunk> = Vec::with_capacity(found.units.len());
        for (index, unit) in found.units.into_iter().enumerate() {
            let read = unit_text(unit)?;
            let point = if index == 0 {
                from.clone()
            } else {
                Some(read.unit.start.clone())
            };
            let formats = match runs.take() {
                Some(runs) if read_formats => UiaText::formats_read(
                    runs,
                    details,
                    (kind == TextUnit::Character).then_some(read.unit.text.len()),
                ),
                _ => Vec::new(),
            };
            chunks.push(chunk_from(
                self.source,
                self.anchors,
                (&read.unit, kind),
                (read.offset, point),
                (read.languages, &formats),
            ));
        }
        if found.ended
            && let Some(last) = chunks.last_mut()
        {
            last.last = true;
        }
        Ok(Done::ReadText(Chunks {
            moved: found.moved,
            chunks,
        }))
    }

    /// A `CompareSelection`'s result: the changes, the whole new selection
    /// (read whole by its range when the program's read was cut), and
    /// whether the text the baseline selected changed since it was read.
    fn compared(&self, before: &Before, compared: Compared) -> Done {
        let changes = compared
            .changes
            .iter()
            .map(|change| change_of(change.selected, &change.text))
            .collect();
        // The executor read the new selection whole when its first read
        // was cut.
        let new_selection = compared
            .selection_text
            .map(|units| change_of(true, &units))
            .filter(|selected| !selected.text.is_empty());
        // The text the baseline selected as it was read, when it was held
        // outside the run and selected something.
        let was = match before {
            Before::SelectionOf(place) if place.index() >= self.first => None,
            before => baseline_anchor(before, self.earlier)
                .and_then(|anchor| self.anchors.held_read(anchor))
                .and_then(|read| {
                    let kept = read.kept_selection();
                    (!kept.collapsed).then_some(kept.text).flatten()
                }),
        };
        let text_changed = match (was, compared.previous_text) {
            (Some(was), Some(now)) => change_of(false, &now).text != change_of(false, &was).text,
            _ => false,
        };
        Done::CompareSelection(SelectionCompared {
            difference: SelectionDifference {
                changes,
                new_selection,
            },
            text_changed,
        })
    }
}

/// A `ReadRange`'s text, `units` read up to one past the range's limit, as
/// a copy's: cut at the limit between whole characters.
fn range_text(units: &[u16]) -> RangeText {
    let read_cut = units.len() > MAX_RANGE_UNITS;
    let units = &units[..units.len().min(MAX_RANGE_UNITS)];
    let (mut text, _, cut) = to_utf8(units, MAX_RANGE_BYTES, &[]);
    if read_cut && !cut {
        keep_whole_characters(&mut text);
    }
    RangeText {
        text,
        truncated: read_cut || cut,
    }
}
