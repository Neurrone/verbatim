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
//! does a walk of the control view. `Read`, `Test`, `IsOneOf`, `Activate`
//! and `Follow` have call arms only, and each runs alone by calls
//! ([`object`]'s module documentation says why). `Ancestors` has a call
//! arm only, the raw view's walk up with the outpost's stops between its
//! steps ([`AncestryFrom`]), which no program can judge (Claude,
//! 2026-11-22); the outpost chains what is above where it ended.
//!
//! After a program, the fallback rule of the design's section 5 decides
//! what runs again by calls, in batch order, before anything after it:
//! every rerun step is logged through [`remote_fell_back`], so an
//! end-to-end scenario that meets one fails.

mod calls;
pub(crate) mod colours;
pub mod focus;
mod object;
mod program;
mod walk;

use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTextRange, TextUnit};

use verbatim_model::batch::{
    Before, Check, Gate, KindMismatch, Known, Missing, Op, Outcomes, Place, Point, Step,
    StepOutcome, StepRefusal, Subject, View, Walk,
};
use verbatim_model::{NodeSnapshot, TableId, TextDetails, TextMovement, TextPoint};
use verbatim_uia::{Cached, Fill, Node, Uia};

use crate::caret::{Attributes, Run, SelectionTextChange};
use crate::error::Error;
use crate::text::{FoundPoint, Movement, Position, TextFrom, TextTarget, UnitText};

pub use calls::run_calls;
pub use object::{ElementRead, TestFound};

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
        _read: (Option<TextMovement>, verbatim_model::TextUnit, u16),
        _details: TextDetails,
    ) -> Result<UnitsRead, Ran> {
        Err(Defect("a text read on a resolver of objects").ran())
    }

    /// The element an object step's subject names, its cache filled by
    /// the fill of the step at `place`: one call, or the outpost's walk of
    /// the route to it when it keeps none. What a `Read`, a `Test` and an
    /// `Activate` read first.
    ///
    /// # Errors
    ///
    /// The step's outcome when it names none, or the read fails.
    fn filled(&self, _subject: &Subject, _place: Place) -> Result<Cached<Node>, Ran> {
        Err(Defect("an element read on a resolver of text").ran())
    }

    /// The window a window subject names, as the dispatcher resolved it.
    fn window(&self, _subject: &Subject) -> Option<isize> {
        None
    }

    /// Property `id` of `node`, a window's own element read as it is, as
    /// its node's snapshot holds it. Local.
    fn node_value(
        &self,
        _node: &Cached<Node>,
        _id: verbatim_model::property::PropertyId,
    ) -> Option<verbatim_model::property::PropertyValue> {
        None
    }

    /// The element a listener's route names, found and minted by the
    /// outpost, as the listener read it, with the route's window: what a
    /// `Read` of a route answers, with no read of its own.
    ///
    /// # Errors
    ///
    /// The step's outcome when the route names no element.
    fn route(&self, _subject: &Subject) -> Result<(NodeSnapshot, isize), Ran> {
        Err(Defect("a route on a resolver that holds none").ran())
    }

    /// The node an object step's subject names, as the outpost holds it.
    fn node_id(&self, _subject: &Subject) -> Option<verbatim_model::NodeId> {
        None
    }

    /// Property `id` of the subject that the outpost answers from what it
    /// holds rather than from the element's cache: its MSAA ids, whether
    /// its window is shown, its window's class, and a tree view item's
    /// children.
    fn held(
        &self,
        _subject: &Subject,
        _id: verbatim_model::property::PropertyId,
    ) -> Option<verbatim_model::property::PropertyValue> {
        None
    }

    /// Property `id` of the element `read`, as its node's snapshot holds
    /// it, the node minted by the outpost. Local.
    fn value(
        &self,
        _read: &ElementRead,
        _id: verbatim_model::property::PropertyId,
    ) -> Option<verbatim_model::property::PropertyValue> {
        None
    }

    /// Where an `Ancestors` walk of `subject` starts and how it stops.
    ///
    /// # Errors
    ///
    /// The step's outcome when the subject names no element.
    fn ancestry(&self, _subject: &Subject) -> Result<AncestryFrom<'_>, Ran> {
        Err(Defect("an ancestor walk on a resolver that starts none").ran())
    }

    /// A text summary's value, made of the UTF-16 text of the caret's
    /// line and of the selection when text is selected; `read_classically`
    /// when a program of its read could not run at all, so the window is
    /// read classically from now on.
    fn text_summary(
        &self,
        _read: (&[u16], Option<&[u16]>),
        _read_classically: bool,
    ) -> Option<verbatim_model::property::PropertyValue> {
        let _ = Defect("a text summary on a resolver that makes none").ran();
        None
    }

    /// Moves the outpost's focus registration to what `target` names, or
    /// removes it, as `Follow` asks: the registration and what it follows
    /// are the outpost's.
    ///
    /// # Errors
    ///
    /// The step's outcome when the node named is not held, or the outpost
    /// has no registration.
    fn follow(&self, _target: Option<&verbatim_model::batch::FollowTarget>) -> Result<(), Ran> {
        Err(Defect("a follow on a resolver with no registration").ran())
    }

    /// What a `WaitForCaret` poll's check compares.
    ///
    /// # Errors
    ///
    /// The step's outcome when the starting point is no longer held.
    fn wait_check(&self) -> Result<WaitCheck<'_>, Ran> {
        Err(Defect("a caret wait on a resolver that holds no wait").ran())
    }

    /// The rows and the width the last colour read of the text `subject`
    /// names found, as [`Resolve::keep_colour_size`] was given them: the
    /// size its next window read's program reads its colours at. `None`
    /// when none is kept, so the colour programs read them all.
    fn colour_size(&self, _subject: &Subject) -> Option<(u32, u32)> {
        None
    }

    /// Keeps `size`, the rows and the width a colour read of the text
    /// `subject` names found, with that text, so that it ends with it.
    fn keep_colour_size(&self, _subject: &Subject, _size: (u32, u32)) {}
}

/// Where an `Ancestors` walk starts and how it stops, as the outpost
/// gives it ([`Resolve::ancestry`]): the walk is
/// [`verbatim_uia::Uia::ancestors`].
pub struct AncestryFrom<'a> {
    /// The element walked from.
    pub element: IUIAutomationElement,
    /// Its own native window, 0 for none.
    pub own_window: isize,
    /// Its nearest window, 0 when not known.
    pub nearest: isize,
    /// How each ancestor's cache is filled.
    pub request: &'a verbatim_uia::Request<verbatim_uia::Ancestor>,
    /// Mints each ancestor's node.
    pub registry: &'a dyn verbatim_uia::Interning,
    /// The most hops.
    pub max_hops: u32,
    /// Where the walk stops: a window read through the other API, an
    /// ancestor Core knows, and the routes kept of each ancestor visited.
    pub stops: verbatim_uia::AncestorStops<'a>,
}

/// `Ancestors`' result, as [`verbatim_uia::Uia::ancestors`] answers it.
#[derive(Debug)]
pub struct Ancestors {
    /// The ancestors, outermost first.
    pub chain: Vec<verbatim_model::NodeSnapshot>,
    /// The window the walk stopped below, for the chain to go on from.
    pub crossed: Option<isize>,
    /// How the walk ended.
    pub walk: verbatim_uia::AncestorWalk,
    /// The automation id of the subject's raw-view parent, when the walk's
    /// first step read it.
    pub parent_automation_id: Option<String>,
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

/// How a `ReadText` reads, as `resolver` makes it of the step's unit,
/// count and details: a trimmed read, whose chunk answers no languages,
/// reads none.
fn read_of(
    resolver: &dyn Resolve,
    read: (Option<TextMovement>, verbatim_model::TextUnit, u16),
    (details, trim): (TextDetails, bool),
) -> Result<UnitsRead, Ran> {
    let read = resolver.units(read, details)?;
    Ok(UnitsRead {
        culture: read.culture && !trim,
        ..read
    })
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

/// The most UTF-16 code units a `ReadRange` reads: a trimmed read, a
/// terminal's rows, is read whole (`GetText(-1)`), as the terminal's own
/// read always was.
const fn range_limit(trim: bool) -> i32 {
    if trim { -1 } else { MAX_RANGE_TEXT }
}

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
    /// `Ancestors`'.
    Ancestors(Ancestors),
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
    /// `Read`'s: the properties of its set, in its order.
    Read(verbatim_model::property::PropertyValues),
    /// `Test`'s.
    Test(TestFound),
    /// `IsOneOf`'s: the place of the node met among those named.
    IsOneOf(Option<u32>),
    /// `Activate`'s: the action performed.
    Activate(Option<verbatim_model::ActionName>),
    /// `Follow`'s: the registration moved.
    Follow,
    /// `Read`'s of a window's own element: the element, as read.
    Window(Cached<Node>),
    /// `Read`'s of a listener's route: the element as the listener read
    /// it, and the route's window.
    Route(Box<NodeSnapshot>, isize),
    /// `Visible`'s.
    Visible(VisibleFound),
    /// `Move`'s.
    Move(MoveFound),
    /// `FindText`'s: the matches, the nearest first.
    FindText(Vec<TextMatch>),
    /// `ReadColours`' : each row's runs, from the first.
    Colours(Vec<Vec<crate::ColourRun>>),
}

/// `Visible`'s result.
#[derive(Debug)]
pub struct VisibleFound {
    /// The first visible range, or the whole text's.
    pub range: IUIAutomationTextRange,
    /// Its text, padding and line breaks as the provider gave them.
    pub text: Vec<u16>,
    /// Whether it starts at the text's start.
    pub at_text_start: bool,
}

/// `Move`'s result.
#[derive(Debug)]
pub struct MoveFound {
    /// A collapsed range at the point it moved from.
    pub from: IUIAutomationTextRange,
    /// A collapsed range where it landed.
    pub to: IUIAutomationTextRange,
    /// How many units it went.
    pub moved: i32,
}

/// One match a `FindText` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextMatch {
    /// How many units a move from the start of the match's unit to the
    /// text's end went.
    pub to_end: u32,
    /// The text read from the start of the match's unit.
    pub text: Vec<u16>,
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
            Self::Move(_) => Some(true),
            Self::Visible(_)
            | Self::FindText(_)
            | Self::Colours(_)
            | Self::ReadText(_)
            | Self::ReadRange(_)
            | Self::CompareSelection(_)
            | Self::Select
            | Self::Locate(_)
            | Self::Read(_)
            | Self::Test(_)
            | Self::IsOneOf(_)
            | Self::Activate(_)
            | Self::Follow
            | Self::Window(_)
            | Self::Route(..)
            | Self::Ancestors(_) => None,
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
    /// The selection, when text is selected, with whether its start is at
    /// the text's start.
    pub selection: Option<(IUIAutomationTextRange, bool)>,
    /// Whether the provider reported a selection, an empty one included:
    /// with none the caret is the text's start standing in.
    pub reported: bool,
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
    /// The colour read whose rows a program read and failed at, in the
    /// program or in a colour program going on from it: its rerun reads
    /// classically, as a colour program's failure always sent the read to
    /// the classic reads, with no second try of the programs.
    colours_failed: Option<usize>,
}

impl<'s, 'a> State<'s, 'a> {
    fn new(segment: &'s Segment<'a>) -> Self {
        Self {
            segment,
            ran: segment.steps.iter().map(|_| None).collect(),
            read_classically: false,
            colours_failed: None,
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

    /// Whether the step at `index` runs, by the model's one rule
    /// ([`Step::judge`]) over the outcomes before it. `Err` when it names a
    /// step it cannot, a defect of the segment's.
    fn gate(&self, index: usize) -> Result<Gate, Defect> {
        self.step(index)
            .judge(&Earlier {
                state: self,
                by: index,
            })
            .map(|(gate, _)| gate)
    }

    /// What the step at `by` knows of the step at `place`, which runs
    /// before it, by the batch's earlier outcomes or by calls.
    fn known<P>(&self, place: Place, by: usize) -> Result<Known<P>, Defect> {
        if self.before(place) {
            return Ok(Known::of(self.segment.earlier.get(place.index())).widen());
        }
        match self.named(place, by) {
            Some(Ran::Done(_)) => Ok(Known::Done),
            Some(Ran::Unrun(Unrun::Skipped)) => Ok(Known::Skipped),
            Some(_) => Ok(Known::Unrun),
            None => Err(Defect("a step names one not before it in its segment")),
        }
    }

    /// What the done step at `place`, before the step at `by`, answers
    /// `check`: a match is tested only of a step before the segment.
    fn check(&self, place: Place, by: usize, check: Check) -> Option<bool> {
        if self.before(place) {
            return Outcomes::check(self.segment.earlier, place, check);
        }
        match (self.named(place, by), check) {
            (Some(Ran::Done(found)), Check::Found) => found.found(),
            _ => None,
        }
    }

    /// Runs the step at `index` by calls.
    fn call(&mut self, index: usize) {
        let ran = match self.gate(index) {
            Ok(Gate::Run) => calls::run(self, index),
            Ok(Gate::Skip) => Ran::Unrun(Unrun::Skipped),
            Ok(Gate::Compare {
                point,
                than,
                negated,
            }) => match calls::not_after(self, index, (&point, &than)) {
                Err(answered) => answered,
                Ok(holds) if holds == negated => Ran::Unrun(Unrun::Skipped),
                Ok(_) => calls::run(self, index),
            },
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

    /// Reads whole the new selection of each comparison whose read of it
    /// was cut at [`crate::MAX_COMPARED_TEXT`]: by its range, up to a
    /// copy's limit (one call), after every step of the segment, as the
    /// comparison's result is made.
    fn whole_selections(&mut self) {
        use verbatim_uia::text::TextRangeExt;
        for index in 0..self.ran.len() {
            let Op::CompareSelection { after, .. } = &self.step(index).op else {
                continue;
            };
            let cut = matches!(
                &self.ran[index],
                Some(Ran::Done(Found::CompareSelection(Compared {
                    selection_text: Some(text),
                    ..
                }))) if text.len() >= crate::MAX_COMPARED_TEXT.unsigned_abs() as usize
            );
            let selection = match self.named(*after, index) {
                Some(Ran::Done(Found::Caret(CaretFound {
                    selection: Some((selection, ..)),
                    ..
                }))) if cut => selection.clone(),
                _ => continue,
            };
            let whole = selection.text(MAX_CHANGE_TEXT);
            match (&mut self.ran[index], whole) {
                (Some(Ran::Done(Found::CompareSelection(compared))), Ok(whole)) => {
                    compared.selection_text = Some(whole);
                }
                (ran, Err(error)) => *ran = Some(failed(&error.into())),
                _ => {}
            }
        }
    }

    fn answer(mut self) -> Answer {
        self.whole_selections();
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

/// The outcomes before the step at `by`, as its gate judges them: every
/// step named has run, so none is left to test.
struct Earlier<'v, 's, 'a> {
    state: &'v State<'s, 'a>,
    by: usize,
}

impl Outcomes for Earlier<'_, '_, '_> {
    type Pending = std::convert::Infallible;
    type Error = Defect;

    fn known(&self, place: Place) -> Result<Known<Self::Pending>, Defect> {
        self.state.known(place, self.by)
    }

    fn check(&self, place: Place, check: Check) -> Option<bool> {
        self.state.check(place, self.by, check)
    }
}

/// A defect in a segment the outpost built, which no call can answer.
#[derive(Clone, Copy, Debug)]
struct Defect(&'static str);

impl From<KindMismatch> for Defect {
    fn from(_: KindMismatch) -> Self {
        Self("a condition names a result it cannot test")
    }
}

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
        Op::Read { .. } => "read",
        Op::Test { .. } => "test",
        Op::IsOneOf { .. } => "is one of",
        Op::Activate { .. } => "activate",
        Op::Follow(_) => "follow",
        Op::Ancestors { .. } => "ancestors",
        Op::MoveBySentence { .. } => "move by sentence",
        Op::Visible { .. } => "visible",
        Op::Move { .. } => "move",
        Op::FindText { .. } => "find text",
        Op::ReadColours { .. } => "read colours",
        Op::FocusRead { .. } => "not fusable",
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
