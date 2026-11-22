//! The request list's fusable operations on UI Automation subjects (the
//! verbatim-uia-rops crate guide, "The program builder and the
//! ordinary-calls executor"): a run of consecutive steps of one batch, as
//! the model has them ([`Segment`]), what they name outside the run
//! resolved by the outpost ([`Resolve`]), run as remote programs one
//! builder generates from the operations ([`run_batch`]), or call by call
//! through the UI Automation adapter ([`run_calls`]).
//!
//! The builder puts consecutive steps in one program, each in a try block
//! with a status of its own, every status written "not reached" when the
//! program starts. A program ends after an act (`Select`), after a walk
//! down the tree, whose later runs come before anything reads it, and
//! where its emitters' declared worst cases would pass the platform's
//! limit on one run. A step whose subject no program can take, an element
//! served only by UI Automation's proxies inside this process or a step
//! from a top-level window ([`Element::calls_only`]), runs by calls, as
//! does a walk of the control view. `Read` and `Ancestors` are not
//! fusable: the outpost reads them through its own reads (Claude,
//! 2026-11-22).
//!
//! After a program, the fallback rule of the design's section 5 decides
//! what runs again by calls, in batch order, before anything after it:
//! every rerun step is logged through [`remote_fell_back`], so an
//! end-to-end scenario that meets one fails.

mod calls;
mod program;
mod walk;

use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTextRange, TextUnit};

use verbatim_model::batch::{
    Before, Check, Edges, Gate, Missing, Op, Place, Point, Step, StepOutcome, StepRefusal, Subject,
    View, Walk,
};
use verbatim_model::{TableId, TextDetails, TextMovement, TextPoint};
use verbatim_uia::{Cached, Fill, Node, Uia};

use crate::caret::{Attributes, Run, SelectionTextChange};
use crate::error::Error;
use crate::text::{FoundPoint, Movement, Position, TextFrom, TextTarget, UnitText};

pub use calls::run_calls;

/// A run of consecutive fusable steps of one batch, the model's own, from
/// place `first`; what they name before it, a subject, a point, a
/// baseline, the outpost resolves ([`Resolve`]), and a condition on it is
/// decided from `earlier`.
pub struct Segment<'a> {
    /// The UI Automation client.
    pub uia: &'a Uia,
    /// The steps, in batch order.
    pub steps: &'a [Step],
    /// The batch place of the first of them.
    pub first: usize,
    /// The outcomes of the batch's steps before `first`.
    pub earlier: &'a [StepOutcome],
    /// What the steps name that the outpost holds.
    pub resolver: &'a dyn Resolve,
    /// Whether programs may be run: the setting allows them and the
    /// window is not marked for classic reading.
    pub remote: bool,
    /// The window the subjects are in, for the fallback's log.
    pub window: Option<isize>,
}

/// What the outpost resolves for a segment: the objects and the text its
/// steps' subjects name, the points and baselines named outside it, how a
/// text read reads, and a walk's fill and judgement. Each resolution that
/// fails answers the step's outcome; a method a segment's steps need that
/// a resolver does not give is a defect of the outpost's, logged.
pub trait Resolve {
    /// The element an object step's subject names.
    ///
    /// # Errors
    ///
    /// The step's outcome when it names none.
    fn object(&self, _subject: &Subject) -> Result<Element<'_>, Ran> {
        Err(Defect("an object step on a resolver of text").ran())
    }

    /// How each node the step at `place` reaches is filled.
    fn fill(&self, _place: Place) -> Option<&Fill<Node>> {
        None
    }

    /// Whether `node`, read by the walk at `place`, is answered, and
    /// whether its subtree is left out: the outpost's matchers, evaluated
    /// in Rust on what was read.
    fn judge(&self, _place: Place, _node: &Cached<Node>) -> Judged {
        let _ = Defect("a walk on a resolver with no matchers").ran();
        Judged {
            matched: false,
            pruned: true,
        }
    }

    /// The text a text step's subject names.
    ///
    /// # Errors
    ///
    /// The step's outcome when it names none.
    fn text(&self, _subject: &Subject) -> Result<TextTarget<'_>, Ran> {
        Err(Defect("a text step on a resolver of objects").ran())
    }

    /// The position a point names ([`TextPoint::At`]), or the point of a
    /// step before the segment ([`Point::Of`]).
    ///
    /// # Errors
    ///
    /// The step's outcome when it names a position no longer held.
    fn point(&self, _point: &Point) -> Result<HeldAt, Ran> {
        Err(Defect("a point on a resolver of objects").ran())
    }

    /// The selection a comparison starts from, held under a baseline or a
    /// `Caret` before the segment: its range's start to the point's end.
    ///
    /// # Errors
    ///
    /// The step's outcome when the baseline is no longer held.
    fn baseline(&self, _before: &Before) -> Result<FoundPoint, Ran> {
        Err(Defect("a baseline on a resolver of objects").ran())
    }

    /// How a `ReadText` of `count` units of `unit`, after `movement`,
    /// fetching `details`, reads them.
    ///
    /// # Errors
    ///
    /// The step's outcome when the text has no such unit.
    fn units(
        &self,
        _read: (Option<TextMovement>, verbatim_model::TextUnit, u8),
        _details: TextDetails,
    ) -> Result<UnitsRead, Ran> {
        Err(Defect("a text read on a resolver of objects").ran())
    }

    /// What a `WaitForCaret` poll's check compares.
    ///
    /// # Errors
    ///
    /// The step's outcome when the starting point is no longer held.
    fn wait_check(&self) -> Result<WaitCheck<'_>, Ran> {
        Err(Defect("a caret wait on a resolver that holds no wait").ran())
    }
}

/// The element an object step's subject names.
#[derive(Clone, Copy)]
pub struct Element<'a> {
    /// The element.
    pub element: &'a IUIAutomationElement,
    /// Its own native window, 0 for none.
    pub window: isize,
    /// Whether the step runs by calls whatever the segment's mode: the
    /// element is served only by UI Automation's proxies inside this
    /// process (`Cached::served_only_by_uia_proxies`), which no program
    /// can import, or the step goes from a top-level window, whose
    /// relations a program's walk does not reach.
    pub calls_only: bool,
}

/// A position the outpost holds, as a point: the position itself, or the
/// point `prefix` after it ([`TextFrom::After`]).
#[derive(Clone, Debug)]
pub struct HeldAt {
    /// The position.
    pub at: FoundPoint,
    /// The text between it and the point, empty for the position itself.
    pub prefix: Vec<u16>,
    /// The character counts `prefix` may be to the provider.
    pub counts: Vec<i32>,
}

impl HeldAt {
    /// The point as the text reads find it.
    fn from(&self) -> TextFrom<'_> {
        let position = Position {
            range: &self.at.range,
            endpoint: self.at.endpoint,
            collapsed: self.at.collapsed,
        };
        if self.prefix.is_empty() {
            TextFrom::At(position)
        } else {
            TextFrom::After {
                from: position,
                prefix: &self.prefix,
                counts: &self.counts,
            }
        }
    }
}

/// The point a model's point names that the program or the calls find
/// themselves: the caret, an end of the selection or of the text.
fn named(point: &Point) -> Option<TextFrom<'static>> {
    match point {
        Point::At(TextPoint::Caret) => Some(TextFrom::Caret),
        Point::At(TextPoint::SelectionStart) => Some(TextFrom::SelectionStart),
        Point::At(TextPoint::SelectionEnd) => Some(TextFrom::SelectionEnd),
        Point::At(TextPoint::Start) => Some(TextFrom::Start),
        Point::At(TextPoint::End) => Some(TextFrom::End),
        Point::At(TextPoint::At(_)) | Point::Of(..) => None,
    }
}

/// What a caret wait's check compares, as [`crate::WatchQuery`] takes it.
#[derive(Clone, Copy)]
pub struct WaitCheck<'a> {
    /// The text.
    pub text: TextTarget<'a>,
    /// The starting point's caret.
    pub start: Position<'a>,
    /// The starting point's selection's ends, both the caret when nothing
    /// was selected.
    pub start_selection: (Position<'a>, Position<'a>),
    /// Whether the word at the caret is read.
    pub word: bool,
    /// Whether this is the check at the bound, after which the steps gated
    /// on it read whatever it shows.
    pub last: bool,
}

/// A `WaitForCaret` poll's check's result.
#[derive(Debug)]
pub struct Checked {
    /// What the check read.
    pub answer: crate::WatchAnswer,
    /// Whether the steps gated on it read: the caret or the selection
    /// moved, or it is the check at the bound. A check that shows the
    /// effect only by the text leaves them unread.
    pub reads_after: bool,
}

/// A step's outcome decided without a call: a refusal (no text, no such
/// unit), an anchor or node no longer held, or a condition that did not
/// hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unrun {
    /// Refused.
    Refused(StepRefusal),
    /// Not there.
    NotThere(Missing),
    /// Skipped.
    Skipped,
}

/// What a `ReadText` reads, as the resolver makes it of the step's unit,
/// count and details.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitsRead {
    /// How it moves first, if at all.
    pub movement: Option<Movement>,
    /// The unit read.
    pub unit: TextUnit,
    /// How many units: more than one reads ahead.
    pub count: u32,
    /// Units after the first are read only while the text read is shorter
    /// than this.
    pub max_total: u32,
    /// Whether each unit's language is read.
    pub culture: bool,
    /// The attributes whose formatting is read over the first unit, one
    /// stretch for a character; none for no formatting.
    pub formats: Attributes,
    /// Of `formats`, those whose support is still learned: the provider's
    /// "not supported" over the unit says it lacks one
    /// ([`UnitsFound::unsupported`]).
    pub learning: Attributes,
}

/// The most UTF-16 code units of one unit's text, or of a caret wait's
/// line or word, read: one past a chunk's limit, so a longer one is known
/// cut.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "a constant far below `i32::MAX`"
)]
const MAX_UNIT_TEXT: i32 = verbatim_model::MAX_CHUNK_BYTES as i32 + 1;

/// The most UTF-16 code units of a range read: one past what fits in a
/// copy's limit, so a longer one is known cut.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "a constant far below `i32::MAX`"
)]
const MAX_RANGE_TEXT: i32 = (verbatim_model::MAX_RANGE_BYTES / 2) as i32 + 1;

/// The most UTF-16 code units of each change of a selection compared.
const MAX_CHANGE_TEXT: i32 = MAX_RANGE_TEXT - 1;

/// How a `Descendants` walks: the model's walk, with the fill and the
/// judgement the resolver gives its step.
#[derive(Clone, Copy)]
pub(crate) struct Descend<'a> {
    /// How deep below its starts it goes.
    pub(crate) depth: u32,
    /// The view it walks: a program walks the raw view only, so a walk of
    /// the control view runs by calls.
    pub(crate) view: View,
    /// Whether each start is judged and answered too.
    pub(crate) include_subject: bool,
    /// Whether the walk leaves subtrees out, so that a program's run reads
    /// only its starts' children, each judged before its own are read.
    pub(crate) prunes: bool,
    /// The most nodes answered: the walk stops once nothing it has still
    /// to read could come before the last of them.
    pub(crate) limit: u32,
    /// The most nodes visited.
    pub(crate) nodes: u32,
    /// How each node's cache is filled.
    pub(crate) fill: &'a Fill<Node>,
    /// The resolver whose matchers judge each node, and the walk's place.
    judge: (&'a dyn Resolve, Place),
}

impl<'a> Descend<'a> {
    /// `walk`, the step at `place`, with what `resolver` gives it; `None`
    /// when it gives no fill.
    fn of(walk: &Walk, resolver: &'a dyn Resolve, place: Place) -> Option<Self> {
        Some(Self {
            depth: walk.depth,
            view: walk.view,
            include_subject: walk.include_subject,
            prunes: walk.prune.is_some(),
            limit: walk.limit,
            nodes: walk.nodes,
            fill: resolver.fill(place)?,
            judge: (resolver, place),
        })
    }

    /// `node`'s judgement.
    pub(crate) fn judged(&self, node: &Cached<Node>) -> Judged {
        self.judge.0.judge(self.judge.1, node)
    }
}

/// A node's judgement in a walk down the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Judged {
    /// It is answered.
    pub matched: bool,
    /// Its subtree, itself included, is left out.
    pub pruned: bool,
}

/// A step's outcome, as run.
#[derive(Debug)]
pub enum Ran {
    /// Its result.
    Done(Found),
    /// Refused, not there, or skipped without a call.
    Unrun(Unrun),
    /// A call failed because the provider's element is gone.
    Gone,
    /// The provider did not answer within UI Automation's transaction
    /// timeout.
    TimedOut,
    /// A call failed in another way, after the fallback.
    Failed,
}

/// A step's result.
#[derive(Debug)]
pub enum Found {
    /// `Related`'s.
    Related(Related),
    /// `Descendants`'.
    Descendants(Descendants),
    /// `Caret`'s.
    Caret(CaretFound),
    /// `ReadText`'s.
    ReadText(UnitsFound),
    /// `ReadRange`'s: the text.
    ReadRange(Vec<u16>),
    /// `CompareSelection`'s.
    CompareSelection(Compared),
    /// `Select`'s: the selection is made.
    Select,
    /// `Locate`'s: the left and top of the character at the
    /// point, in screen pixels, when the provider tells.
    Locate(Option<(f64, f64)>),
    /// A `WaitForCaret` poll's check's.
    WaitCheck(Checked),
}

impl Found {
    /// Whether it found what [`Check::Found`] asks; `None` for a result
    /// that finds nothing.
    fn found(&self) -> Option<bool> {
        match self {
            Self::Related(related) => Some(!related.nodes.is_empty()),
            Self::Descendants(descendants) => Some(!descendants.nodes.is_empty()),
            Self::WaitCheck(checked) => Some(checked.reads_after),
            Self::Caret(caret) => Some(caret.selection.is_some()),
            Self::ReadText(_)
            | Self::ReadRange(_)
            | Self::CompareSelection(_)
            | Self::Select
            | Self::Locate(_) => None,
        }
    }
}

/// `Related`'s result.
#[derive(Debug)]
pub struct Related {
    /// The objects reached, none at an edge.
    pub nodes: Vec<Cached<Node>>,
    /// For a step through the tree, the subject's own native window or
    /// its nearest ancestor's, as `verbatim_uia::nearest_window_handle`
    /// finds it.
    pub window: Option<isize>,
    /// For a step through the tree reaching a cell, with a fill that reads
    /// cells, the cell's table.
    pub table: Option<TableId>,
}

/// `Descendants`' result.
#[derive(Debug)]
pub struct Descendants {
    /// The nodes answered, in tree order, at most the walk's limit.
    pub nodes: Vec<Descendant>,
    /// Whether the walk visited every node it was asked to: neither its
    /// node cap nor a failed read cut it.
    pub complete: bool,
}

/// One node a walk down the tree answered.
#[derive(Debug)]
pub struct Descendant {
    /// The node.
    pub node: Cached<Node>,
    /// Its parent's place in the answered list, `None` when its parent was
    /// not answered.
    pub parent: Option<u32>,
    /// Its start's place among the walk's starts.
    pub root: u32,
}

/// `Caret`'s result.
#[derive(Debug)]
pub struct CaretFound {
    /// A collapsed range at the caret: the start of the selection's first
    /// range, as NVDA's caret is, or the text's start when it has none.
    pub caret: IUIAutomationTextRange,
    /// Whether the caret is at the text's start or end.
    pub edges: Edges,
    /// The selection, when text is selected, with whether its start and its
    /// end are each at the text's start or end.
    pub selection: Option<(IUIAutomationTextRange, Edges, Edges)>,
}

/// `ReadText`'s result.
#[derive(Debug)]
pub struct UnitsFound {
    /// How far the movement went: less than asked at the text's ends.
    pub moved: i32,
    /// The units read, at least one.
    pub units: Vec<UnitText>,
    /// Reading ahead found no unit after the last one read.
    pub ended: bool,
    /// The formatting of the first unit, stretch by stretch.
    pub runs: Vec<Run>,
    /// Of the read's `learning` attributes, those the provider answered
    /// "not supported" for over the first unit; `None` when nothing was
    /// learned (a character's one read, or an empty unit's).
    pub unsupported: Option<Attributes>,
}

/// `CompareSelection`'s result.
#[derive(Debug)]
pub struct Compared {
    /// What became selected and unselected, by the text protocol's rule.
    pub changes: Vec<SelectionTextChange>,
    /// The selection's text now, up to [`crate::MAX_COMPARED_TEXT`], when
    /// something is selected.
    pub selection_text: Option<Vec<u16>>,
    /// The text between the earlier selection's ends as they are now, up to
    /// [`crate::MAX_COMPARED_TEXT`], when it held something.
    pub previous_text: Option<Vec<u16>>,
}

/// A segment's answer.
#[derive(Debug)]
pub struct Answer {
    /// Each step's outcome, in the segment's order.
    pub steps: Vec<Ran>,
    /// A program could not run at all, an import refused or an instruction
    /// unsupported: the outpost reads the window classically from now on.
    pub read_classically: bool,
}

/// Logs that `program`, tried for an element of `window` (`None` when not
/// known), failed with `error` and was read by the classic calls instead:
/// the one place a fallback is logged, to
/// [`verbatim_model::REMOTE_FALLBACK_TARGET`].
pub fn remote_fell_back(program: &'static str, window: Option<isize>, error: &Error) {
    tracing::warn!(
        target: verbatim_model::REMOTE_FALLBACK_TARGET,
        program,
        ?window,
        %error,
        "a remote operation failed; read the classic way"
    );
}

/// Runs `segment`'s steps as remote programs, each program's failures
/// rerun by calls as the fallback rule says (the module's documentation),
/// or all by calls when the segment may run no program.
#[must_use]
pub fn run_batch(segment: &Segment<'_>) -> Answer {
    let mut state = State::new(segment);
    let mut at = 0;
    while at < segment.steps.len() {
        if !segment.remote {
            state.calls(at..segment.steps.len());
            break;
        }
        at = match program::cut(&state, at) {
            Cut::Calls(index) => {
                state.call(index);
                index + 1
            }
            Cut::Program(range) => state.program(range),
        };
    }
    state.answer()
}

/// What runs next ([`program::cut`]).
enum Cut {
    /// One step by calls.
    Calls(usize),
    /// A program of these steps.
    Program(std::ops::Range<usize>),
}

/// A segment's run under way: each step's outcome once known.
struct State<'s, 'a> {
    segment: &'s Segment<'a>,
    ran: Vec<Option<Ran>>,
    read_classically: bool,
}

impl<'s, 'a> State<'s, 'a> {
    fn new(segment: &'s Segment<'a>) -> Self {
        Self {
            segment,
            ran: segment.steps.iter().map(|_| None).collect(),
            read_classically: false,
        }
    }

    /// The step at `index`.
    fn step(&self, index: usize) -> &'a Step {
        &self.segment.steps[index]
    }

    /// The batch place of the step at `index`.
    fn place(&self, index: usize) -> Place {
        Place(u8::try_from(self.segment.first + index).unwrap_or(u8::MAX))
    }

    /// Whether `place` is a step before the segment's.
    fn before(&self, place: Place) -> bool {
        place.index() < self.segment.first
    }

    /// The segment index of `place`, a step of the segment before `before`.
    fn index(&self, place: Place, before: usize) -> Option<usize> {
        place
            .index()
            .checked_sub(self.segment.first)
            .filter(|&index| index < before)
    }

    /// The outcome of the step at `place`, named by the step at `by`.
    fn named(&self, place: Place, by: usize) -> Option<&Ran> {
        self.index(place, by)
            .and_then(|index| self.ran[index].as_ref())
    }

    /// Whether the step at `index` runs: every step it refers to is done,
    /// and its condition holds (the design's condition table). `Err` when
    /// it names a step it cannot, a defect of the segment's.
    fn gate(&self, index: usize) -> Result<bool, Defect> {
        for place in self.step(index).references() {
            if self.before(place) {
                if !matches!(
                    self.segment.earlier.get(place.index()),
                    Some(StepOutcome::Done(_))
                ) {
                    return Ok(false);
                }
                continue;
            }
            match self.named(place, index) {
                Some(Ran::Done(_)) => {}
                Some(_) => return Ok(false),
                None => return Err(Defect("a step names one not before it in its segment")),
            }
        }
        self.holds(index)
    }

    /// Whether the condition of the step at `index` holds, its named step's
    /// outcome known.
    fn holds(&self, index: usize) -> Result<bool, Defect> {
        let Some(condition) = self.step(index).when else {
            return Ok(true);
        };
        if self.before(condition.on) {
            return match self
                .segment
                .earlier
                .get(condition.on.index())
                .map_or(Ok(Gate::Skip), |named| condition.gate(named))
            {
                Ok(gate) => Ok(gate == Gate::Run),
                Err(_) => Err(Defect("a condition names a result it cannot test")),
            };
        }
        let holds = match self.named(condition.on, index) {
            None => {
                return Err(Defect(
                    "a condition names a step not before it in its segment",
                ));
            }
            Some(Ran::Unrun(Unrun::Skipped)) => return Ok(false),
            Some(Ran::Done(found)) => match condition.check {
                Check::Found => found
                    .found()
                    .ok_or(Defect("a condition tests a result that finds nothing"))?,
                Check::Matched => return Err(Defect("a condition in a segment tests a match")),
            },
            Some(_) => false,
        };
        Ok(holds != condition.negated)
    }

    /// Runs the step at `index` by calls.
    fn call(&mut self, index: usize) {
        let ran = match self.gate(index) {
            Ok(true) => calls::run(self, index),
            Ok(false) => Ran::Unrun(Unrun::Skipped),
            Err(defect) => defect.ran(),
        };
        self.ran[index] = Some(ran);
    }

    /// Runs the steps of `range` by calls, in order.
    fn calls(&mut self, range: std::ops::Range<usize>) {
        for index in range {
            self.call(index);
        }
    }

    /// Reruns the steps of `range` by calls after a program's `error`,
    /// each logged as a fallback.
    fn rerun(&mut self, range: std::ops::Range<usize>, error: &Error) {
        for index in range {
            remote_fell_back(kind_name(&self.step(index).op), self.segment.window, error);
            self.call(index);
        }
    }

    fn answer(self) -> Answer {
        Answer {
            steps: self
                .ran
                .into_iter()
                .map(|ran| ran.unwrap_or(Ran::Unrun(Unrun::Skipped)))
                .collect(),
            read_classically: self.read_classically,
        }
    }
}

/// A defect in a segment the outpost built, which no call can answer.
#[derive(Clone, Copy, Debug)]
struct Defect(&'static str);

impl Defect {
    /// The defect logged, and its step answered as failed.
    fn ran(self) -> Ran {
        tracing::error!(target: verbatim_model::DEFECT_TARGET, defect = self.0, "a batch segment the executor cannot run");
        Ran::Failed
    }
}

/// The operation's name, for the fallback's log.
fn kind_name(op: &Op) -> &'static str {
    match op {
        Op::Related { .. } => "related",
        Op::Descendants { .. } => "descendants",
        Op::Caret { .. } => "caret",
        Op::ReadText { .. } => "read text",
        Op::ReadRange { .. } => "read range",
        Op::CompareSelection { .. } => "compare selection",
        Op::Select { .. } => "select",
        Op::Locate { .. } => "locate",
        Op::WaitForCaret { .. } => "wait check",
        _ => "not fusable",
    }
}

/// The outcome a failed call gives: gone, timed out, or failed.
fn failed(error: &Error) -> Ran {
    use windows::Win32::UI::Accessibility::{UIA_E_ELEMENTNOTAVAILABLE, UIA_E_TIMEOUT};
    match error.hresult().map(|code| code.0.cast_unsigned()) {
        Some(UIA_E_ELEMENTNOTAVAILABLE) => Ran::Gone,
        Some(UIA_E_TIMEOUT) => Ran::TimedOut,
        _ => Ran::Failed,
    }
}
