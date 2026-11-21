//! The request list's fusable operations on UI Automation subjects (the
//! verbatim-uia-rops crate guide, "The program builder and the
//! ordinary-calls executor"): a run of consecutive steps of one batch, its
//! subjects resolved by the outpost ([`Segment`]), run as remote programs
//! one builder generates from the operations ([`run_batch`]), or call by
//! call through the UI Automation adapter ([`run_calls`]).
//!
//! The builder puts consecutive steps in one program, each in a try block
//! with a status of its own, every status written "not reached" when the
//! program starts. A program ends after an act ([`Input::Select`]), after a
//! walk down the tree, whose later runs come before anything reads it, and
//! where its emitters' declared worst cases would pass the platform's
//! limit on one run. A step whose subject no program can take, an element
//! served only by UI Automation's proxies inside this process or a step
//! from a top-level window ([`Fused::calls_only`]), runs by calls, as do
//! `Ancestors` and a walk of the control view.
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
    Check, Condition, Edges, Missing, Place, PointOf, Relation, StepRefusal, View,
};
use verbatim_model::{NodeSnapshot, TableId};
use verbatim_uia::{
    Ancestor, AncestorStops, AncestorWalk, Cached, Fill, Interning, Node, Request, Uia,
};

use crate::caret::{Attributes, Run, SelectionTextChange};
use crate::error::Error;
use crate::text::{Movement, Position, TextFrom, TextTarget, UnitText};

pub use calls::run_calls;

/// A run of consecutive fusable steps of one batch, each subject the
/// outpost holds already resolved to its element, text pattern or held
/// range. A [`Place`] a step names is a step of this segment: what a step
/// names in an earlier segment the outpost resolves, and a condition on
/// one it decides itself ([`Input::Answered`]).
pub struct Segment<'a> {
    /// The UI Automation client.
    pub uia: &'a Uia,
    /// The steps, in batch order.
    pub steps: Vec<Fused<'a>>,
    /// Whether programs may be run: the setting allows them and the
    /// window is not marked for classic reading.
    pub remote: bool,
    /// The window the subjects are in, for the fallback's log.
    pub window: Option<isize>,
}

/// One step of a [`Segment`].
pub struct Fused<'a> {
    /// Its place in the batch.
    pub place: Place,
    /// Its condition, on a step of this segment.
    pub when: Option<Condition>,
    /// The operation, with its inputs resolved.
    pub input: Input<'a>,
    /// Whether it runs by calls whatever the segment's mode: its subject
    /// is served only by UI Automation's proxies inside this process
    /// (`Cached::served_only_by_uia_proxies`), which no program can import,
    /// or it steps from a top-level window, whose relations a program's
    /// walk does not reach.
    pub calls_only: bool,
}

/// What a step's operation reads, its inputs resolved by the outpost.
pub enum Input<'a> {
    /// The outpost answered it without a call.
    Answered(Unrun),
    /// [`verbatim_model::batch::Op::Read`].
    Read {
        /// The object.
        subject: Object<'a>,
        /// How its cache is filled.
        fill: &'a Fill<Node>,
    },
    /// [`verbatim_model::batch::Op::Related`].
    Related {
        /// The object stepped from.
        subject: Object<'a>,
        /// Which way.
        relation: Relation,
        /// How each object reached is filled.
        fill: &'a Fill<Node>,
    },
    /// [`verbatim_model::batch::Op::Ancestors`], always by calls
    /// ([`Uia::ancestors`]).
    Ancestors(Box<AncestorsInput<'a>>),
    /// [`verbatim_model::batch::Op::Descendants`].
    Descendants {
        /// Where the walk starts: one node, or every node of an earlier
        /// list.
        subject: Object<'a>,
        /// How it walks.
        walk: Descend<'a>,
    },
    /// [`verbatim_model::batch::Op::Caret`].
    Caret {
        /// The text.
        text: TextTarget<'a>,
    },
    /// [`verbatim_model::batch::Op::ReadText`].
    ReadText {
        /// The text.
        text: TextTarget<'a>,
        /// Where it starts.
        at: Point<'a>,
        /// What it reads.
        read: UnitsRead,
    },
    /// [`verbatim_model::batch::Op::ReadRange`].
    ReadRange {
        /// The text.
        text: TextTarget<'a>,
        /// The range's two points and, when set, the points just past the
        /// characters at them ([`Ends`]).
        ends: Ends<'a>,
        /// The most UTF-16 code units read.
        max_text: i32,
    },
    /// [`verbatim_model::batch::Op::CompareSelection`].
    CompareSelection {
        /// The text.
        text: TextTarget<'a>,
        /// What the selection is compared with.
        before: Prior<'a>,
        /// The earlier `Caret` of this segment whose selection is compared.
        after: Place,
        /// The most UTF-16 code units of each change's text read.
        max_text: i32,
    },
    /// [`verbatim_model::batch::Op::Select`], the one act.
    Select {
        /// The text.
        text: TextTarget<'a>,
        /// The range's points.
        ends: Ends<'a>,
    },
    /// [`verbatim_model::batch::Op::Locate`].
    Locate {
        /// The text.
        text: TextTarget<'a>,
        /// The point.
        at: Point<'a>,
    },
    /// One poll's check of a [`verbatim_model::batch::Op::WaitForCaret`],
    /// the first step of the poll's program, the batch's later steps fused
    /// after it under its gate ([`Found::found`]).
    WaitCheck(WaitCheck<'a>),
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
    /// The most UTF-16 code units of a line or a word read.
    pub max_text: i32,
}

/// [`Input::WaitCheck`]'s result.
#[derive(Debug)]
pub struct Checked {
    /// What the check read.
    pub answer: crate::WatchAnswer,
    /// Whether the steps gated on it read: the caret or the selection
    /// moved, or it is the check at the bound. A check that shows the
    /// effect only by the text leaves them unread.
    pub reads_after: bool,
}

/// A step's outcome the outpost decided without a call: a refusal (no
/// text, no such unit), an anchor or node it no longer holds, or a
/// condition on an earlier segment's step that did not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unrun {
    /// Refused.
    Refused(StepRefusal),
    /// Not there.
    NotThere(Missing),
    /// Skipped.
    Skipped,
}

/// The object an object operation names.
#[derive(Clone, Copy)]
pub enum Object<'a> {
    /// An element the outpost holds.
    Held {
        /// The element.
        element: &'a IUIAutomationElement,
        /// Its own native window, 0 for none.
        window: isize,
    },
    /// The node an earlier step of this segment answered, or for a walk
    /// down the tree every node of its list.
    Result(Place),
    /// One node of an earlier step's list.
    Item(Place, u32),
}

/// A point in a text step's text.
#[derive(Clone, Copy)]
pub enum Point<'a> {
    /// A point the outpost resolved: the caret, an end of the selection
    /// or of the text, or a position it holds.
    From(TextFrom<'a>),
    /// A point of an earlier step of this segment's result.
    Of(Place, PointOf),
}

/// A range's two points, in either order, and, when set, the points just
/// past the characters at them, so that the range runs through the
/// character at its later end (`verbatim_model::TextThrough`).
#[derive(Clone, Copy)]
pub struct Ends<'a> {
    /// One end.
    pub start: Point<'a>,
    /// The other end.
    pub end: Point<'a>,
    /// Just past the characters at `start` and at `end`.
    pub through: Option<(Point<'a>, Point<'a>)>,
}

/// What a [`Input::CompareSelection`] compares the selection with.
#[derive(Clone, Copy)]
pub enum Prior<'a> {
    /// A held selection baseline: its range's start to the position's
    /// end, collapsed when nothing was selected.
    Baseline(Position<'a>),
    /// The selection an earlier `Caret` of this segment read.
    SelectionOf(Place),
}

/// What a [`Input::ReadText`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnitsRead {
    /// How it moves first, if at all.
    pub movement: Option<Movement>,
    /// The unit read.
    pub unit: TextUnit,
    /// How many units: more than one reads ahead.
    pub count: u32,
    /// The most UTF-16 code units of one unit's text.
    pub max_text: i32,
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

/// How a [`Input::Descendants`] walks.
#[derive(Clone, Copy)]
pub struct Descend<'a> {
    /// How deep below its starts it goes.
    pub depth: u32,
    /// The view it walks: a program walks the raw view only, so a walk of
    /// the control view runs by calls.
    pub view: View,
    /// Whether each start is judged and answered too.
    pub include_subject: bool,
    /// Whether the walk leaves subtrees out, so that a program's run reads
    /// only its starts' children, each judged before its own are read.
    pub prunes: bool,
    /// The most nodes answered: the walk stops once nothing it has still
    /// to read could come before the last of them.
    pub limit: u32,
    /// The most nodes visited.
    pub nodes: u32,
    /// How each node's cache is filled.
    pub fill: &'a Fill<Node>,
    /// Whether a node, read, is answered, and whether its subtree is left
    /// out: the outpost's matchers, evaluated in Rust on what was read.
    pub judge: &'a dyn Fn(&Cached<Node>) -> Judged,
}

/// A node's judgement in a walk down the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Judged {
    /// It is answered.
    pub matched: bool,
    /// Its subtree, itself included, is left out.
    pub pruned: bool,
}

/// What an [`Input::Ancestors`] reads, as [`Uia::ancestors`] takes it.
pub struct AncestorsInput<'a> {
    /// The element walked from.
    pub subject: Object<'a>,
    /// Its nearest window, 0 when not known.
    pub nearest: isize,
    /// How each ancestor's cache is filled.
    pub request: &'a Request<Ancestor>,
    /// Mints each ancestor's node id.
    pub registry: &'a dyn Interning,
    /// The most hops.
    pub max_hops: u32,
    /// Where the walk stops.
    pub stops: &'a AncestorStops<'a>,
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
    /// [`Input::Read`]'s: the object, its cache filled.
    Read(Cached<Node>),
    /// [`Input::Related`]'s.
    Related(Related),
    /// [`Input::Ancestors`]'.
    Ancestors(Ancestors),
    /// [`Input::Descendants`]'.
    Descendants(Descendants),
    /// [`Input::Caret`]'s.
    Caret(CaretFound),
    /// [`Input::ReadText`]'s.
    ReadText(UnitsFound),
    /// [`Input::ReadRange`]'s: the text.
    ReadRange(Vec<u16>),
    /// [`Input::CompareSelection`]'s.
    CompareSelection(Compared),
    /// [`Input::Select`]'s: the selection is made.
    Select,
    /// [`Input::Locate`]'s: the left and top of the character at the
    /// point, in screen pixels, when the provider tells.
    Locate(Option<(f64, f64)>),
    /// [`Input::WaitCheck`]'s.
    WaitCheck(Checked),
}

impl Found {
    /// Whether it found what [`Check::Found`] asks; `None` for a result
    /// that finds nothing.
    fn found(&self) -> Option<bool> {
        match self {
            Self::Read(_) => Some(true),
            Self::Related(related) => Some(!related.nodes.is_empty()),
            Self::Ancestors(ancestors) => Some(matches!(ancestors.walk, AncestorWalk::MetKnown(_))),
            Self::Descendants(descendants) => Some(!descendants.nodes.is_empty()),
            Self::WaitCheck(checked) => Some(checked.reads_after),
            Self::Caret(_)
            | Self::ReadText(_)
            | Self::ReadRange(_)
            | Self::CompareSelection(_)
            | Self::Select
            | Self::Locate(_) => None,
        }
    }
}

/// [`Input::Related`]'s result.
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

/// [`Input::Ancestors`]' result, as [`Uia::ancestors`] answers it.
#[derive(Debug)]
pub struct Ancestors {
    /// The ancestors, outermost first.
    pub chain: Vec<NodeSnapshot>,
    /// The window the walk stopped below, to go on through the other API.
    pub crossed: Option<isize>,
    /// How the walk ended.
    pub walk: AncestorWalk,
    /// The automation id of the subject's raw-view parent, when the first
    /// hop read it.
    pub parent_automation_id: Option<String>,
}

/// [`Input::Descendants`]' result.
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

/// [`Input::Caret`]'s result.
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

/// [`Input::ReadText`]'s result.
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

/// [`Input::CompareSelection`]'s result.
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

    /// The segment index of `place`, a step of the segment before `before`.
    fn index(&self, place: Place, before: usize) -> Option<usize> {
        let first = self.segment.steps.first()?.place.index();
        place
            .index()
            .checked_sub(first)
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
        let step = &self.segment.steps[index];
        for place in references(&step.input) {
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
        let Some(condition) = self.segment.steps[index].when else {
            return Ok(true);
        };
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
        let ran = match &self.segment.steps[index].input {
            Input::Answered(unrun) => Ran::Unrun(*unrun),
            input => match self.gate(index) {
                Ok(true) => calls::run(self, index, input),
                Ok(false) => Ran::Unrun(Unrun::Skipped),
                Err(defect) => defect.ran(),
            },
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
            if !matches!(self.segment.steps[index].input, Input::Answered(_)) {
                remote_fell_back(
                    kind_name(&self.segment.steps[index].input),
                    self.segment.window,
                    error,
                );
            }
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

/// Every earlier step `input` refers to, whose result it needs done.
fn references(input: &Input<'_>) -> Vec<Place> {
    let object = |object: &Object<'_>| match *object {
        Object::Result(place) | Object::Item(place, _) => Some(place),
        Object::Held { .. } => None,
    };
    let point = |point: &Point<'_>| match *point {
        Point::Of(place, _) => Some(place),
        Point::From(_) => None,
    };
    let ends = |ends: &Ends<'_>| {
        let mut places = vec![point(&ends.start), point(&ends.end)];
        if let Some((start, end)) = &ends.through {
            places.extend([point(start), point(end)]);
        }
        places
    };
    let places = match input {
        Input::Answered(_) | Input::Caret { .. } | Input::WaitCheck(_) => Vec::new(),
        Input::Read { subject, .. }
        | Input::Related { subject, .. }
        | Input::Descendants { subject, .. } => vec![object(subject)],
        Input::Ancestors(ancestors) => vec![object(&ancestors.subject)],
        Input::ReadText { at, .. } | Input::Locate { at, .. } => vec![point(at)],
        Input::ReadRange { ends: range, .. } | Input::Select { ends: range, .. } => ends(range),
        Input::CompareSelection { before, after, .. } => vec![
            match before {
                Prior::SelectionOf(place) => Some(*place),
                Prior::Baseline(_) => None,
            },
            Some(*after),
        ],
    };
    places.into_iter().flatten().collect()
}

/// The operation's name, for the fallback's log.
fn kind_name(input: &Input<'_>) -> &'static str {
    match input {
        Input::Answered(_) => "answered",
        Input::Read { .. } => "read",
        Input::Related { .. } => "related",
        Input::Ancestors(_) => "ancestors",
        Input::Descendants { .. } => "descendants",
        Input::Caret { .. } => "caret",
        Input::ReadText { .. } => "read text",
        Input::ReadRange { .. } => "read range",
        Input::CompareSelection { .. } => "compare selection",
        Input::Select { .. } => "select",
        Input::Locate { .. } => "locate",
        Input::WaitCheck(_) => "wait check",
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
