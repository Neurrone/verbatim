//! A run of a batch's fusable text steps on one UI Automation node, through
//! the program builder (the request list's design,
//! `target/captures-2026-11-20/build8-design/design.md`, section 5): the
//! steps' points resolved against the node's anchors and the batch's
//! earlier results, run as one segment by
//! [`verbatim_uia_rops::batch::run_batch`], and each step's result made the
//! model's, its points held as anchors and its chunks minted with their
//! formatting.
//!
//! A point a step names in an earlier segment is resolved here to the
//! position it holds, so the segment names only its own steps; a step
//! whose condition or reference is on an earlier segment is decided here
//! before the segment runs. A `Caret`'s read is held as the next selection
//! baseline, with the selection's text a later `CompareSelection` of the
//! same run read for it.

#![forbid(unsafe_code)]

use windows::Win32::UI::Accessibility::IUIAutomationTextRange;

use verbatim_model::batch::{
    Before, CaretAt, Chunks, Condition, Done, Edges, Gate, HeldPoint, HeldSelection, KindMismatch,
    Located, Missing, Op, Place, Point, PointOf, RangeText, Selected, SelectionCompared, Step,
    StepOutcome, StepRefusal,
};
use verbatim_model::{
    CaretSelection, MAX_RANGE_BYTES, MAX_READ_AHEAD_TEXT, NodeId, Selection, SelectionDifference,
    TextAnchor, TextChunk, TextDetails, TextMovement, TextPoint, TextPosition, TextUnit,
};
use verbatim_uia::Uia;
use verbatim_uia::text::Endpoint;
use verbatim_uia_rops::batch::{
    Compared, Ends, Found, Fused, Input, Point as Spot, Prior, Ran, Segment, UnitsFound, UnitsRead,
    Unrun,
};
use verbatim_uia_rops::{Movement, Position, TextFrom};

use super::uia::{UiaPos, UiaText, character_counts, round, unit_text};
use super::{
    MAX_CHUNK_UNITS, MAX_RANGE_UNITS, NodeText, PointFrom, TextResult, change, change_of,
    chunk_from, keep_whole_characters, to_utf8,
};

/// The most UTF-16 code units of a selection's text a program reads
/// (`verbatim_uia_rops::MAX_COMPARED_TEXT`): a text as long is cut, and is
/// read whole by its range.
const SELECTION_TEXT_CAP: usize = 513;

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

/// A position held for a program to start from, with the source's
/// position it is, so a read from it can remember where it read.
struct HeldAt {
    pos: UiaPos,
    range: IUIAutomationTextRange,
    endpoint: Endpoint,
    collapsed: bool,
}

impl HeldAt {
    fn new(pos: UiaPos) -> TextResult<Self> {
        let (range, endpoint) = pos.range_end()?;
        let collapsed = pos.collapsed();
        Ok(Self {
            pos,
            range,
            endpoint,
            collapsed,
        })
    }

    fn position(&self) -> Position<'_> {
        Position {
            range: &self.range,
            endpoint: self.endpoint,
            collapsed: self.collapsed,
        }
    }
}

/// A point resolved for the segment.
enum Planned {
    /// A point the program finds: the caret, an end of the selection or
    /// of the text.
    Named(TextFrom<'static>),
    /// A position held.
    At(HeldAt),
    /// Some text after a position held.
    After(HeldAt, Vec<u16>, Vec<i32>),
    /// A point of a step of the run.
    Here(Place, PointOf),
}

impl Planned {
    fn spot(&self) -> Spot<'_> {
        match self {
            Self::Named(from) => Spot::From(*from),
            Self::At(held) => Spot::From(TextFrom::At(held.position())),
            Self::After(held, prefix, counts) => Spot::From(TextFrom::After {
                from: held.position(),
                prefix,
                counts,
            }),
            Self::Here(place, which) => Spot::Of(*place, *which),
        }
    }
}

/// A range's points resolved for the segment.
struct PlannedEnds {
    start: Planned,
    end: Planned,
    through: Option<(Planned, Planned)>,
}

impl PlannedEnds {
    fn ends(&self) -> Ends<'_> {
        Ends {
            start: self.start.spot(),
            end: self.end.spot(),
            through: self
                .through
                .as_ref()
                .map(|(start, end)| (start.spot(), end.spot())),
        }
    }
}

/// A step resolved for the segment, owning what its input borrows.
enum Plan {
    /// Decided without a call.
    Answered(Unrun),
    /// Failed without a call: a defect of the batch's.
    Failed,
    Caret,
    ReadText {
        at: Planned,
        read: UnitsRead,
        kind: TextUnit,
        details: TextDetails,
    },
    ReadRange(PlannedEnds),
    Compare {
        before: PlannedPrior,
        after: Place,
    },
    Select(PlannedEnds),
    Locate(Planned),
}

/// What a comparison compares with, resolved for the segment.
enum PlannedPrior {
    /// A held baseline: its selection's range, the end that is its end,
    /// whether nothing was selected, and its text as it was read.
    Baseline(HeldAt, Option<Vec<u16>>),
    /// The selection a `Caret` of the run read.
    SelectionOf(Place),
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
    let mut whens = Vec::with_capacity(steps.len());
    let plans: Vec<Plan> = steps
        .iter()
        .map(|step| {
            let (plan, when) = plan(source, anchors, step, first, earlier);
            whens.push(when);
            plan
        })
        .collect();
    let target = source.target();
    let fused: Vec<Fused<'_>> = plans
        .iter()
        .zip(&whens)
        .enumerate()
        .map(|(index, (plan, when))| Fused {
            place: Place(u8::try_from(first + index).unwrap_or(u8::MAX)),
            when: *when,
            input: match plan {
                Plan::Answered(unrun) => Input::Answered(*unrun),
                // Its failure replaces whatever the segment answers.
                Plan::Failed => Input::Answered(Unrun::Skipped),
                Plan::Caret => Input::Caret { text: target },
                Plan::ReadText { at, read, .. } => Input::ReadText {
                    text: target,
                    at: at.spot(),
                    read: *read,
                },
                Plan::ReadRange(ends) => Input::ReadRange {
                    text: target,
                    ends: ends.ends(),
                    max_text: i32::try_from(MAX_RANGE_UNITS + 1).unwrap_or(i32::MAX),
                },
                Plan::Compare { before, after } => Input::CompareSelection {
                    text: target,
                    before: match before {
                        PlannedPrior::Baseline(held, _) => Prior::Baseline(held.position()),
                        PlannedPrior::SelectionOf(place) => Prior::SelectionOf(*place),
                    },
                    after: *after,
                    max_text: i32::try_from(MAX_RANGE_UNITS).unwrap_or(i32::MAX),
                },
                Plan::Select(ends) => Input::Select {
                    text: target,
                    ends: ends.ends(),
                },
                Plan::Locate(at) => Input::Locate {
                    text: target,
                    at: at.spot(),
                },
            },
            calls_only: false,
        })
        .collect();
    let segment = Segment {
        uia,
        steps: fused,
        remote: source.tries_remote(),
        window,
    };
    let began_us = verbatim_process::clock::now_us();
    let answer = verbatim_uia_rops::batch::run_batch(&segment);
    drop(segment);
    let mut made = Made {
        node,
        source,
        anchors,
        first,
        read_at_us,
        carets: Vec::new(),
    };
    let texts = selection_texts(&plans, &answer.steps, first);
    let outcomes = plans
        .into_iter()
        .zip(answer.steps)
        .enumerate()
        .map(|(index, (plan, ran))| made.outcome(index, plan, ran, (&texts, began_us)))
        .collect();
    FusedAnswer {
        outcomes,
        read_classically: answer.read_classically,
    }
}

/// The selection text a later `CompareSelection` read for each `Caret` of
/// the run, by the caret's index in the run.
fn selection_texts(plans: &[Plan], ran: &[Ran], first: usize) -> Vec<Option<Vec<u16>>> {
    let mut texts = vec![None; plans.len()];
    for (plan, ran) in plans.iter().zip(ran) {
        if let (
            Plan::Compare { after, .. },
            Ran::Done(Found::CompareSelection(Compared {
                selection_text: Some(text),
                ..
            })),
        ) = (plan, ran)
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

/// `step` resolved for the segment, and the condition it carries into it:
/// a step whose reference or condition is on an earlier segment's step is
/// decided here.
fn plan(
    source: &UiaText,
    anchors: &NodeText<'_, UiaPos>,
    step: &Step,
    first: usize,
    earlier: &[StepOutcome],
) -> (Plan, Option<Condition>) {
    let outside = |place: Place| place.index() < first;
    let done = |place: Place| matches!(earlier.get(place.index()), Some(StepOutcome::Done(_)));
    if step
        .references()
        .into_iter()
        .any(|place| outside(place) && !done(place))
    {
        return (Plan::Answered(Unrun::Skipped), None);
    }
    let when = match step.when {
        Some(condition) if outside(condition.on) => {
            match earlier
                .get(condition.on.index())
                .map_or(Ok(Gate::Skip), |named| condition.gate(named))
            {
                Ok(Gate::Run) => None,
                Ok(Gate::Skip) => return (Plan::Answered(Unrun::Skipped), None),
                Err(KindMismatch) => {
                    tracing::error!(target: verbatim_model::DEFECT_TARGET, ?step, "a step's condition names a result it cannot test");
                    return (Plan::Failed, None);
                }
            }
        }
        when => when,
    };
    let planned = planned_step(source, anchors, &step.op, first, earlier);
    (planned.unwrap_or_else(Unplanned::plan), when)
}

/// `op` resolved for the segment, or the plan that answers it unrun.
fn planned_step(
    source: &UiaText,
    anchors: &NodeText<'_, UiaPos>,
    op: &Op,
    first: usize,
    earlier: &[StepOutcome],
) -> Result<Plan, Unplanned> {
    let point = |point: &Point| resolve(anchors, *point, first, earlier);
    let ends = |start: &Point, end: &Point, through: Option<&verbatim_model::batch::Through>| {
        Ok::<_, Unplanned>(PlannedEnds {
            start: point(start)?,
            end: point(end)?,
            through: match through {
                Some(through) => Some((point(&through.start)?, point(&through.end)?)),
                None => None,
            },
        })
    };
    Ok(match op {
        Op::Caret { .. } => Plan::Caret,
        Op::ReadText {
            at,
            movement,
            unit,
            count,
            details,
            ..
        } => {
            let (read, kind) = units_read(source, (*movement, *unit, *count), *details)?;
            Plan::ReadText {
                at: point(at)?,
                read,
                kind,
                details: *details,
            }
        }
        Op::ReadRange {
            start,
            end,
            through,
            ..
        } => Plan::ReadRange(ends(start, end, through.as_ref())?),
        Op::Select {
            start,
            end,
            through,
            ..
        } => Plan::Select(ends(start, end, through.as_ref())?),
        Op::Locate { at, .. } => Plan::Locate(point(at)?),
        Op::CompareSelection { before, after, .. } => Plan::Compare {
            before: match before {
                Before::Baseline(anchor) => baseline(anchors, *anchor)?,
                Before::SelectionOf(place) if place.index() >= first => {
                    PlannedPrior::SelectionOf(*place)
                }
                // The selection of an earlier segment's `Caret` is the
                // read it held.
                Before::SelectionOf(place) => match earlier.get(place.index()) {
                    Some(StepOutcome::Done(Done::Caret(caret))) => baseline(anchors, caret.held)?,
                    _ => return Err(defect("a comparison names no caret read")),
                },
            },
            after: if after.index() >= first {
                *after
            } else {
                return Err(defect("a comparison's caret read is not in its run"));
            },
        },
        _ => return Err(defect("a step that is not a fusable text step")),
    })
}

/// Why a step is not planned for the segment.
#[derive(Clone, Copy)]
enum Unplanned {
    /// Its outcome, decided without a call.
    Unrun(Unrun),
    /// It failed without a call.
    Failed,
}

impl Unplanned {
    fn plan(self) -> Plan {
        match self {
            Self::Unrun(unrun) => Plan::Answered(unrun),
            Self::Failed => Plan::Failed,
        }
    }
}

/// A defect in a batch the parse let through, logged: its step fails.
fn defect(what: &'static str) -> Unplanned {
    tracing::error!(target: verbatim_model::DEFECT_TARGET, defect = what, "a batch's text step the outpost cannot run");
    Unplanned::Failed
}

/// A read's units as the program reads them, and the unit its chunks are
/// reported as; the refusal when the text has no such unit.
fn units_read(
    source: &UiaText,
    (movement, unit, count): (Option<TextMovement>, TextUnit, u8),
    details: TextDetails,
) -> Result<(UnitsRead, TextUnit), Unplanned> {
    let no_such_unit = Unplanned::Unrun(Unrun::Refused(StepRefusal::NoSuchUnit));
    // UI Automation has no sentence unit, and a document is not read as a
    // unit.
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
    Ok((
        UnitsRead {
            movement,
            unit: by,
            count: u32::from(count.max(1)),
            max_text: i32::try_from(MAX_CHUNK_UNITS + 1).unwrap_or(i32::MAX),
            max_total: u32::try_from(MAX_READ_AHEAD_TEXT).unwrap_or(u32::MAX),
            culture: true,
            formats,
            learning,
        },
        unit,
    ))
}

/// The baseline held under `anchor`, as a comparison takes it: its
/// selection's range to its end, or its caret when nothing was selected.
/// A held selection's two ends are the two ends of one range, as every
/// read that holds one reads it.
fn baseline(anchors: &NodeText<'_, UiaPos>, anchor: TextAnchor) -> Result<PlannedPrior, Unplanned> {
    let Some(read) = anchors.held_read(anchor) else {
        return Err(Unplanned::Unrun(Unrun::NotThere(Missing::Anchor(anchor))));
    };
    let kept = read.kept_selection();
    let held = HeldAt::new(kept.ends.1).map_err(|_| Unplanned::Failed)?;
    let held = HeldAt {
        collapsed: kept.collapsed,
        ..held
    };
    let text = if kept.collapsed {
        None
    } else {
        kept.text.map(|text| text.to_vec())
    };
    Ok(PlannedPrior::Baseline(held, text))
}

/// `point` resolved: a point of the run named by its place, any other to
/// what the program finds or the position held.
fn resolve(
    anchors: &NodeText<'_, UiaPos>,
    point: Point,
    first: usize,
    earlier: &[StepOutcome],
) -> Result<Planned, Unplanned> {
    let text_point = match point {
        Point::Of(place, which) if place.index() >= first => {
            return Ok(Planned::Here(place, which));
        }
        Point::Of(place, which) => {
            let Some(anchor) = earlier
                .get(place.index())
                .and_then(|outcome| earlier_anchor(outcome, which))
            else {
                return Err(defect("a point names a step that holds none"));
            };
            TextPoint::At(TextPosition::at(anchor))
        }
        Point::At(point) => point,
    };
    Ok(match text_point {
        TextPoint::Caret => Planned::Named(TextFrom::Caret),
        TextPoint::SelectionStart => Planned::Named(TextFrom::SelectionStart),
        TextPoint::SelectionEnd => Planned::Named(TextFrom::SelectionEnd),
        TextPoint::Start => Planned::Named(TextFrom::Start),
        TextPoint::End => Planned::Named(TextFrom::End),
        TextPoint::At(position) => {
            let failed = |_| Unplanned::Failed;
            match anchors.point_from(position) {
                None => {
                    return Err(Unplanned::Unrun(Unrun::NotThere(Missing::Anchor(
                        position.anchor,
                    ))));
                }
                Some(PointFrom::After(pos, prefix)) => {
                    let counts = character_counts(&prefix);
                    Planned::After(HeldAt::new(pos).map_err(failed)?, prefix, counts)
                }
                Some(PointFrom::At(pos)) => Planned::At(HeldAt::new(pos).map_err(failed)?),
                Some(_) => return Err(defect("a held position resolves to no position")),
            }
        }
    })
}

/// A `Caret`'s caret and selection, as the source's positions.
type CaretRead = (UiaPos, Option<(UiaPos, UiaPos)>);

/// The results of a run made the model's.
struct Made<'m, 'n> {
    node: NodeId,
    source: &'m mut UiaText,
    anchors: &'m mut NodeText<'n, UiaPos>,
    first: usize,
    read_at_us: u64,
    /// Each `Caret` of the run's caret and selection, by its index in the
    /// run, for the points later steps read from and the comparisons'
    /// whole new selection.
    carets: Vec<Option<CaretRead>>,
}

impl Made<'_, '_> {
    /// The outcome of the step at `index`, planned as `plan`, that ran as
    /// `ran`; `texts` holds the selections' texts later comparisons read,
    /// and `began_us` when the run began.
    fn outcome(
        &mut self,
        index: usize,
        plan: Plan,
        ran: Ran,
        (texts, began_us): (&[Option<Vec<u16>>], u64),
    ) -> StepOutcome {
        self.carets.push(None);
        let done = match (plan, ran) {
            (Plan::Failed, _) | (_, Ran::TimedOut | Ran::Failed) => return StepOutcome::Failed,
            (_, Ran::Unrun(Unrun::Refused(refusal))) => return StepOutcome::Refused(refusal),
            (_, Ran::Unrun(Unrun::NotThere(missing))) => return StepOutcome::NotThere(missing),
            (_, Ran::Unrun(Unrun::Skipped)) => return StepOutcome::Skipped,
            (_, Ran::Gone) => return StepOutcome::NotThere(Missing::Node(self.node)),
            (Plan::Caret, Ran::Done(Found::Caret(found))) => {
                self.caret(index, &found, texts.get(index).cloned().flatten())
            }
            (
                Plan::ReadText {
                    at, kind, details, ..
                },
                Ran::Done(Found::ReadText(found)),
            ) => self.read_text(&at, kind, details, found),
            (Plan::ReadRange(_), Ran::Done(Found::ReadRange(units))) => {
                Ok(Done::ReadRange(range_text(&units)))
            }
            (Plan::Compare { before, after }, Ran::Done(Found::CompareSelection(compared))) => {
                self.compared(&before, after, compared)
            }
            (Plan::Select(_), Ran::Done(Found::Select)) => Ok(Done::Select(Selected { began_us })),
            (Plan::Locate(_), Ran::Done(Found::Locate(screen))) => Ok(Done::Locate(Located {
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
    fn known(&self, at: &Planned) -> Option<UiaPos> {
        match at {
            Planned::At(held) => Some(held.pos.clone()),
            Planned::Here(place, which) => {
                let (caret, selection) = self
                    .carets
                    .get(place.index().checked_sub(self.first)?)?
                    .as_ref()?;
                Some(match (which, selection) {
                    (PointOf::SelectionStart, Some((start, _))) => start.clone(),
                    (PointOf::SelectionEnd, Some((_, end))) => end.clone(),
                    (PointOf::ChunkStart, _) => return None,
                    _ => caret.clone(),
                })
            }
            Planned::Named(_) | Planned::After(..) => None,
        }
    }

    /// A `ReadText`'s result: each unit a chunk reported as `kind`, its
    /// anchor minted, the first with its formatting read for `details`.
    fn read_text(
        &mut self,
        at: &Planned,
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
    fn compared(
        &mut self,
        before: &PlannedPrior,
        after: Place,
        compared: Compared,
    ) -> TextResult<Done> {
        let changes = compared
            .changes
            .iter()
            .map(|change| change_of(change.selected, &change.text))
            .collect();
        let selection = after
            .index()
            .checked_sub(self.first)
            .and_then(|index| self.carets.get(index))
            .and_then(|caret| caret.as_ref())
            .and_then(|(_, selection)| selection.clone());
        let new_selection = match (compared.selection_text, selection) {
            (Some(units), _) if units.len() < SELECTION_TEXT_CAP => Some(change_of(true, &units)),
            (Some(_), Some((start, end))) => Some(change(self.source, true, &start, &end)?),
            (Some(units), None) => Some(change_of(true, &units)),
            (None, _) => None,
        }
        .filter(|selected| !selected.text.is_empty());
        let text_changed = match (before, compared.previous_text) {
            (PlannedPrior::Baseline(_, Some(was)), Some(now)) => {
                change_of(false, &now).text != change_of(false, was).text
            }
            _ => false,
        };
        Ok(Done::CompareSelection(SelectionCompared {
            difference: SelectionDifference {
                changes,
                new_selection,
            },
            text_changed,
        }))
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
