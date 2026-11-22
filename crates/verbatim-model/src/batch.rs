//! The request list: every request Core sends a reader is a batch of at
//! most sixteen operations from one closed list of twenty-one, sent together
//! and answered together (ADR 0003, "Properties, event kinds and
//! requests": "A new command adds no kind of request").
//!
//! An operation names its object by a [`Subject`]; a later operation may
//! name an earlier one's result by its [`Place`], as its subject, as a
//! point in its text, or in a [`Condition`] that skips it. Each operation
//! answers one [`StepOutcome`]; the batch as a whole answers one of ADR
//! 0003's five outcomes ([`crate::request::Outcome`]), its result holding
//! one outcome per operation.
//!
//! A reader turns a batch as it arrives into a [`Parsed`] batch once
//! ([`parse()`]), refusing it whole when it breaks a bound or names
//! something it cannot; its executors take only the parsed form. The one
//! walk over a batch's node ids and the one over its outcomes' node ids
//! and anchors are here too ([`Batch::visit_nodes`],
//! [`visit_outcomes`]), as is the one list of the anchors a batch names
//! ([`Batch::anchors`]).

mod parse;
mod walk;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use crate::matcher::Matcher;
use crate::property::PropertyId;
use crate::request::{FocusRead, FollowGroup, KnownAncestor, MsaaIds, NodeRead, Route};
use crate::tell::ElementToken;
use crate::{
    ActionName, Backend, EditSelection, NodeId, SelectionDifference, SentenceMoved, TextAnchor,
    TextChunk, TextDetails, TextMovement, TextPoint, TextUnit, WindowHandle,
};

pub use parse::{Bound, LISTENER_MSAA_SET, Malformed, Parsed, ReaderKind, parse};
pub use walk::visit_outcomes;

/// The most operations a batch holds: the largest, a terminal's caret
/// key's wait with its window read and colours, and the watch's window
/// read with the search for its mark's line and the colours.
pub const MAX_STEPS: usize = 16;
/// The deepest a walk up or down the tree goes.
pub const MAX_DEPTH: u32 = 64;
/// The most nodes a walk down the tree visits.
pub const MAX_NODES: u32 = 4_096;
/// The most units one read of text reads, chunk by chunk.
pub const MAX_UNITS: u16 = 32;
/// The most units one whole read reads, as one span
/// ([`Op::ReadText`]'s `whole`): a terminal's "Lines spoken in full" at
/// their most.
pub const MAX_WHOLE_UNITS: u16 = crate::MAX_TERMINAL_LINES;
/// The most `FindText` calls one search makes.
pub const MAX_FIND_CALLS: u8 = 200;
/// The most matches one search answers.
pub const MAX_FIND_MATCHES: u8 = 20;
/// The shortest time between two checks of a caret wait, in
/// microseconds.
pub const MIN_POLL_US: u32 = 1_000;
/// The longest a caret wait looks, in microseconds.
pub const MAX_BOUND_US: u32 = 1_000_000;

/// A step's place in its batch, counted from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Place(pub u8);

impl Place {
    /// The place as an index into the batch's steps.
    #[must_use]
    pub fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// Which window a [`Subject::Window`] names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowOf {
    /// This window.
    Handle(WindowHandle),
    /// The top-level window of this window (`GA_ROOT`).
    TopLevelOf(WindowHandle),
    /// A path of window classes: the first a top-level window of the
    /// application (`FindWindow`), each later one a window inside the one
    /// before at any depth.
    ClassPath(Vec<String>),
    /// The window with the keyboard focus.
    Focused,
    /// The foreground window.
    Foreground,
}

/// How an operation names its object. Each subject is resolved once per
/// batch, at its first use, and every later use takes the same object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Subject {
    /// A node the reader holds.
    Node(NodeId),
    /// A UI Automation element the listener kept for a tell, by the
    /// listener's generation and the element's token.
    Kept {
        /// The listener's generation, since each listener mints its tokens
        /// from 1.
        listener: u64,
        /// The element's token.
        token: ElementToken,
        /// Whether the listener keeps the element as read until Core's held
        /// view lets it go, as a focus read does; otherwise it is released
        /// when the batch ends.
        keep: bool,
    },
    /// An MSAA object at its event's ids.
    Msaa {
        /// The ids.
        ids: MsaaIds,
        /// Whether NVDA's focus redirect applies; a menu start takes none
        /// (ADR 0004, "MSAA candidates").
        redirect: bool,
    },
    /// The outpost's own copy of an element the listener read, found by
    /// the runtime ids below its window.
    Route(Box<Route>),
    /// A window's own object.
    Window {
        /// Which window.
        which: WindowOf,
        /// The backend Core reads it through.
        backend: Backend,
        /// Whether only a shown window is taken: a hidden one is not there.
        visible: bool,
    },
    /// The application's focus, inside the focused window, as NVDA's
    /// focus-now read takes it.
    Focus,
    /// The node an earlier operation answered, or for a walk down the
    /// tree, every node it answered.
    Result(Place),
    /// One node of the list an earlier operation answered; an index past
    /// the list's end is not there.
    Item {
        /// The earlier operation.
        of: Place,
        /// The node's place in its list.
        index: u32,
    },
}

impl Subject {
    /// The nodes this subject names among the outcomes `earlier`, when it
    /// names an earlier result: every node of [`Subject::Result`], the one
    /// of [`Subject::Item`], none for an item past its list's end or a
    /// step not done. `None` for a subject that names no earlier result.
    #[must_use]
    pub fn earlier_nodes<'a>(&self, earlier: &'a [StepOutcome]) -> Option<Vec<&'a NodeRead>> {
        let done = |place: &Place| match earlier.get(place.index()) {
            Some(StepOutcome::Done(done)) => done.nodes(),
            _ => Vec::new(),
        };
        match self {
            Self::Result(place) => Some(done(place)),
            Self::Item { of, index } => Some(
                usize::try_from(*index)
                    .ok()
                    .and_then(|index| done(of).get(index).copied())
                    .into_iter()
                    .collect(),
            ),
            Self::Node(_)
            | Self::Kept { .. }
            | Self::Msaa { .. }
            | Self::Route(_)
            | Self::Window { .. }
            | Self::Focus => None,
        }
    }
}

/// Which point of an earlier operation's result a [`Point::Of`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PointOf {
    /// A [`Op::Caret`]'s caret.
    Caret,
    /// A [`Op::Caret`]'s selection start, its caret when nothing is
    /// selected.
    SelectionStart,
    /// A [`Op::Caret`]'s selection end, its caret when nothing is
    /// selected.
    SelectionEnd,
    /// The start of a [`Op::ReadText`]'s first chunk.
    ChunkStart,
    /// The start of a [`Op::Visible`]'s range.
    VisibleStart,
    /// The end of a [`Op::Visible`]'s range.
    VisibleEnd,
    /// Where a [`Op::Move`] landed.
    Moved,
}

/// A point in a subject's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Point {
    /// A point Core names itself.
    At(TextPoint),
    /// A point of an earlier operation's result.
    Of(Place, PointOf),
}

/// The points just past the characters at a range's two ends, so that it
/// runs through the character at its later end
/// (`docs/parity/review.md`, "Select then copy from the review cursor").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Through {
    /// Just past the character at the start.
    pub start: Point,
    /// Just past the character at the end.
    pub end: Point,
}

/// Which way a [`Op::Related`] steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Relation {
    /// The parent.
    Parent,
    /// The first child.
    FirstChild,
    /// The last child.
    LastChild,
    /// The next sibling.
    NextSibling,
    /// The previous sibling.
    PreviousSibling,
    /// The objects its controller-for relation names.
    ControllerFor,
    /// Its first selected item.
    FirstSelectedItem,
}

/// Which view of the tree a [`Op::Descendants`] walks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum View {
    /// Every element.
    Raw,
    /// Only control elements.
    Control,
}

/// What a [`Op::CompareSelection`] compares the selection now with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Before {
    /// A selection baseline Core holds.
    Baseline(TextAnchor),
    /// The selection an earlier [`Op::Caret`] read.
    SelectionOf(Place),
}

/// The two parts of a caret wait's effect test beyond the caret and the
/// selection moving and the characters beside the caret changing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EffectTest {
    /// Whether a change of the unit at the caret counts.
    pub unit: bool,
    /// Whether a change of the caret's line counts.
    pub line: bool,
}

/// What a [`Op::Follow`] follows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowTarget {
    /// The object followed.
    pub subject: Subject,
    /// Its event group.
    pub group: FollowGroup,
    /// An extra object followed for its value changes alone: the focus's
    /// parent when it is a slider that focuses its thumb.
    pub extra: Option<NodeId>,
}

/// A walk down the tree ([`Op::Descendants`]) from its subject: one node,
/// or every node of an earlier result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Walk {
    /// How deep it goes below its start, 1 for the children, at most
    /// [`MAX_DEPTH`].
    pub depth: u32,
    /// The view it walks.
    pub view: View,
    /// What a node must match to be answered; `None` answers every node.
    pub matcher: Option<Matcher>,
    /// What a node must match for its subtree to be left out, itself
    /// included; `None` leaves none out.
    pub prune: Option<Matcher>,
    /// Whether the start is tested and answered too.
    pub include_subject: bool,
    /// The most nodes answered, at most `nodes`: the walk stops once it
    /// has them.
    pub limit: u32,
    /// The most nodes visited, at most [`MAX_NODES`].
    pub nodes: u32,
    /// The properties read of each node answered.
    pub set: Vec<PropertyId>,
}

/// A caret wait ([`Op::WaitForCaret`]): it compares the application's
/// caret in its subject's text with the starting point every `poll_us`
/// from the pass until `bound_us` after it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretWait {
    /// The starting point: an earlier batch's caret, which holds the
    /// caret and the selection's ends.
    pub from: TextAnchor,
    /// The starting line's text.
    pub line: String,
    /// The caret's byte offset in the starting line.
    pub offset: u32,
    /// The starting word, when the unit at the caret is compared.
    pub word: Option<String>,
    /// The effect test's two parts.
    pub effect: EffectTest,
    /// When Core sent the key, in microseconds on the run's shared clock.
    pub passed_at_us: u64,
    /// How often a check runs, at least [`MIN_POLL_US`].
    pub poll_us: u32,
    /// How long after the pass the wait looks, at most [`MAX_BOUND_US`].
    pub bound_us: u32,
    /// Whether a UI Automation caret or text-change event of the subject
    /// heard soon after the pass is the key's effect.
    pub events: bool,
}

/// One operation, with its inputs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    /// The subject's properties, by set.
    Read {
        /// The object.
        subject: Subject,
        /// The properties.
        set: Vec<PropertyId>,
    },
    /// Whether the subject matches, reading its properties lazily in the
    /// matcher's order.
    Test {
        /// The object.
        subject: Subject,
        /// The test.
        matcher: Matcher,
    },
    /// The objects one step away from the subject.
    Related {
        /// The object.
        subject: Subject,
        /// Which way.
        relation: Relation,
        /// The properties of each object reached.
        set: Vec<PropertyId>,
    },
    /// The subject's ancestors, nearest first.
    Ancestors {
        /// The object.
        subject: Subject,
        /// The ancestors Core knows, at which the walk stops; empty walks
        /// to the top.
        until: Vec<KnownAncestor>,
        /// The most ancestors read, at most [`MAX_DEPTH`].
        depth: u32,
        /// The properties of each ancestor.
        set: Vec<PropertyId>,
    },
    /// The subject's descendants, in tree order.
    Descendants {
        /// Where the walk starts.
        subject: Subject,
        /// How it walks.
        walk: Box<Walk>,
    },
    /// Which of the nodes Core names the subject is.
    IsOneOf {
        /// The object.
        subject: Subject,
        /// The node met by NVDA's equality: the committed focus.
        equal: Option<NodeId>,
        /// The nodes met only at the address they were reported at: the
        /// committed focus's ancestors.
        at_address: Vec<NodeId>,
    },
    /// The subject's default action.
    Activate {
        /// The object.
        subject: Subject,
    },
    /// The outpost follows this object for its changes, keeping its
    /// registration and removing every other; `None` removes every one.
    Follow(Option<FollowTarget>),
    /// The subject read as a focus: the outpost's focus reads, with every
    /// rule they hold, as one operation.
    FocusRead {
        /// The object: an [`Subject::Msaa`], [`Subject::Focus`] or
        /// [`Subject::Route`].
        subject: Subject,
        /// The ancestors Core knows.
        known: Vec<KnownAncestor>,
        /// The properties of each node.
        set: Vec<PropertyId>,
        /// What the focus's caret read fetches beyond the text.
        details: TextDetails,
    },
    /// The caret and the selection.
    Caret {
        /// The text subject.
        subject: Subject,
        /// The element of a caret tell, which the outpost adopts as the
        /// node's when the node's own element has no text.
        adopt: Option<ElementToken>,
    },
    /// Units of text: from a point, after an optional movement, `count`
    /// units of `unit`.
    ReadText {
        /// The text subject.
        subject: Subject,
        /// Where it starts.
        at: Point,
        /// How it moves first, if at all.
        movement: Option<TextMovement>,
        /// The unit read.
        unit: TextUnit,
        /// How many units, 1 to [`MAX_UNITS`], or to [`MAX_WHOLE_UNITS`]
        /// when `whole`.
        count: u16,
        /// What it fetches beyond the text: nothing when `whole` or
        /// `trim`.
        details: TextDetails,
        /// Whether the units are read as one span from the point itself,
        /// its end moved by `count` units, in one call, with no movement,
        /// answered as one chunk of at most [`crate::MAX_RANGE_BYTES`].
        whole: bool,
        /// Whether each line of its one chunk loses the spaces at its end,
        /// its padded length answered ([`Chunks::padded`]): a read of one
        /// unit, or a whole one.
        trim: bool,
    },
    /// The text between two points, in document order.
    ReadRange {
        /// The text subject.
        subject: Subject,
        /// One end.
        start: Point,
        /// The other end.
        end: Point,
        /// When set, the text runs through the character at the later end.
        through: Option<Through>,
        /// Whether each line loses the spaces at its end, its padded
        /// length answered ([`RangeText::padded`]).
        trim: bool,
    },
    /// How the selection an earlier [`Op::Caret`] read differs from
    /// `before`.
    CompareSelection {
        /// The text subject.
        subject: Subject,
        /// What it is compared with.
        before: Before,
        /// The earlier [`Op::Caret`] whose selection is compared.
        after: Place,
    },
    /// Select the text between two points; two equal points move the
    /// caret.
    Select {
        /// The text subject.
        subject: Subject,
        /// One end.
        start: Point,
        /// The other end.
        end: Point,
        /// When set, the text runs through the character at the later end.
        through: Option<Through>,
    },
    /// Where a point is.
    Locate {
        /// The text subject.
        subject: Subject,
        /// The point.
        at: Point,
    },
    /// Move the caret by sentence from a starting point, in text whose
    /// provider has a sentence unit (ADR 0003: "A move by sentence is one
    /// request the outpost runs whole").
    MoveBySentence {
        /// The text subject.
        subject: Subject,
        /// The starting point.
        from: TextAnchor,
        /// Forward, or else back.
        forward: bool,
        /// What the read fetches beyond the text.
        details: TextDetails,
    },
    /// Wait for a key's effect on the caret.
    WaitForCaret {
        /// The text subject.
        subject: Subject,
        /// How it waits.
        wait: Box<CaretWait>,
    },
    /// The first range the provider shows on screen, or the whole text
    /// when it shows none: its ends, whether it starts at the text's
    /// start, and its text without the spaces at its lines' ends.
    Visible {
        /// The text subject.
        subject: Subject,
    },
    /// Moves a collapsed copy of a point by units from the point itself,
    /// as the provider moves a range. A point Core holds that lies past
    /// the text's end, or that the provider will not compare with it, is
    /// refused ([`StepRefusal::OutsideText`]).
    Move {
        /// The text subject.
        subject: Subject,
        /// Where it starts.
        at: Point,
        /// By how many of which unit.
        movement: TextMovement,
    },
    /// Searches the text from `at` to its end, or back from `at` to its
    /// start, for `needle`, the nearest match first, always by calls: a
    /// search that finds nothing in Windows Terminal answers a range whose
    /// calls end a remote run (the verbatim-uia-rops crate guide, "The
    /// terminal's operations"). A failed call ends the search with what it
    /// found.
    FindText {
        /// The text subject.
        subject: Subject,
        /// Where it starts.
        at: Point,
        /// The text sought, with no line break.
        needle: String,
        /// Whether it searches toward the text's start.
        backward: bool,
        /// When set, only a match that starts a unit of this kind is
        /// answered; the others are passed over.
        unit_start: Option<TextUnit>,
        /// How many units of text each match answers from its unit's
        /// start, at least one.
        read: u8,
        /// The most matches answered, at most [`MAX_FIND_MATCHES`].
        limit: u8,
        /// The most `FindText` calls, at most [`MAX_FIND_CALLS`].
        calls: u8,
    },
    /// A terminal's colours: each row of the screen an earlier
    /// [`Op::Visible`] read, from its start, as runs of one foreground and
    /// background, the rows stopping at the text's end. Each row is as
    /// many cells as an earlier [`Op::ReadText`]'s first line holds
    /// without its line break, the line at the screen's top, and the
    /// screen as many rows as its lines fill at that width
    /// ([`crate::screen_rows`]): both are sized from the same
    /// batch's answers, so a read after a resize reads the new screen
    /// (Claude, 2026-11-22). The one operation for terminals alone: the
    /// terminals answer a stretch whose last cell differs as one colour,
    /// and their format unit stops only at rows, so no read of formatting
    /// gives a row's colours (Dickson, 2026-11-22).
    ReadColours {
        /// The text subject.
        subject: Subject,
        /// The [`Op::Visible`] whose screen's rows are read.
        screen: Place,
        /// The [`Op::ReadText`] of the line at the screen's top, whose
        /// length is a row's width.
        top: Place,
    },
}

/// The twenty-one operations, by name: each one's result kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OpKind {
    /// [`Op::Read`].
    Read,
    /// [`Op::Test`].
    Test,
    /// [`Op::Related`].
    Related,
    /// [`Op::Ancestors`].
    Ancestors,
    /// [`Op::Descendants`].
    Descendants,
    /// [`Op::IsOneOf`].
    IsOneOf,
    /// [`Op::Activate`].
    Activate,
    /// [`Op::Follow`].
    Follow,
    /// [`Op::FocusRead`].
    FocusRead,
    /// [`Op::Caret`].
    Caret,
    /// [`Op::ReadText`].
    ReadText,
    /// [`Op::ReadRange`].
    ReadRange,
    /// [`Op::CompareSelection`].
    CompareSelection,
    /// [`Op::Select`].
    Select,
    /// [`Op::Locate`].
    Locate,
    /// [`Op::MoveBySentence`].
    MoveBySentence,
    /// [`Op::WaitForCaret`].
    WaitForCaret,
    /// [`Op::Visible`].
    Visible,
    /// [`Op::Move`].
    Move,
    /// [`Op::FindText`].
    FindText,
    /// [`Op::ReadColours`].
    ReadColours,
}

impl OpKind {
    /// Whether an operation of this kind reads or moves text, so its
    /// subject is a text subject.
    #[must_use]
    pub const fn is_text(self) -> bool {
        matches!(
            self,
            Self::Caret
                | Self::ReadText
                | Self::ReadRange
                | Self::CompareSelection
                | Self::Select
                | Self::Locate
                | Self::MoveBySentence
                | Self::WaitForCaret
                | Self::Visible
                | Self::Move
                | Self::FindText
                | Self::ReadColours
        )
    }

    /// Whether a [`Check::Found`] condition can name a result of this
    /// kind: a node, a list, a met ancestor, a place, an effect or a
    /// selection.
    #[must_use]
    pub const fn finds(self) -> bool {
        matches!(
            self,
            Self::Read
                | Self::Test
                | Self::Related
                | Self::Ancestors
                | Self::Descendants
                | Self::IsOneOf
                | Self::FocusRead
                | Self::Caret
                | Self::WaitForCaret
                | Self::Move
        )
    }
}

impl Op {
    /// Its kind.
    #[must_use]
    pub const fn kind(&self) -> OpKind {
        match self {
            Self::Read { .. } => OpKind::Read,
            Self::Test { .. } => OpKind::Test,
            Self::Related { .. } => OpKind::Related,
            Self::Ancestors { .. } => OpKind::Ancestors,
            Self::Descendants { .. } => OpKind::Descendants,
            Self::IsOneOf { .. } => OpKind::IsOneOf,
            Self::Activate { .. } => OpKind::Activate,
            Self::Follow(_) => OpKind::Follow,
            Self::FocusRead { .. } => OpKind::FocusRead,
            Self::Caret { .. } => OpKind::Caret,
            Self::ReadText { .. } => OpKind::ReadText,
            Self::ReadRange { .. } => OpKind::ReadRange,
            Self::CompareSelection { .. } => OpKind::CompareSelection,
            Self::Select { .. } => OpKind::Select,
            Self::Locate { .. } => OpKind::Locate,
            Self::MoveBySentence { .. } => OpKind::MoveBySentence,
            Self::WaitForCaret { .. } => OpKind::WaitForCaret,
            Self::Visible { .. } => OpKind::Visible,
            Self::Move { .. } => OpKind::Move,
            Self::FindText { .. } => OpKind::FindText,
            Self::ReadColours { .. } => OpKind::ReadColours,
        }
    }

    /// Its subject, `None` for a [`Op::Follow`] that removes every
    /// registration.
    #[must_use]
    pub fn subject(&self) -> Option<&Subject> {
        match self {
            Self::Read { subject, .. }
            | Self::Test { subject, .. }
            | Self::Related { subject, .. }
            | Self::Ancestors { subject, .. }
            | Self::IsOneOf { subject, .. }
            | Self::Activate { subject }
            | Self::FocusRead { subject, .. }
            | Self::Caret { subject, .. }
            | Self::ReadText { subject, .. }
            | Self::ReadRange { subject, .. }
            | Self::CompareSelection { subject, .. }
            | Self::Select { subject, .. }
            | Self::Locate { subject, .. }
            | Self::MoveBySentence { subject, .. }
            | Self::Descendants { subject, .. }
            | Self::WaitForCaret { subject, .. }
            | Self::Visible { subject }
            | Self::Move { subject, .. }
            | Self::FindText { subject, .. }
            | Self::ReadColours { subject, .. } => Some(subject),
            Self::Follow(target) => target.as_ref().map(|target| &target.subject),
        }
    }
}

/// What a [`Condition`] tests of the step it names: a condition carries
/// no matcher of its own, a `Test` step holds it (Claude, 2026-11-21).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Check {
    /// A [`Op::Test`] matched.
    Matched,
    /// A node or a non-empty list was found, an ancestor Core knows met,
    /// a place found, an effect seen, or text selected at a caret
    /// ([`OpKind::finds`]), so a read of the selection runs only when its
    /// ends differ (Claude, 2026-11-22).
    Found,
}

/// A test of one earlier step, which skips the step carrying it when it
/// does not hold. A step whose named step was skipped is skipped too; one
/// whose named step was refused, not there or failed takes its check as
/// false, so "unless" runs after a failure, which Core then judges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Condition {
    /// The earlier step.
    pub on: Place,
    /// What is tested.
    pub check: Check,
    /// Whether the test's negation is what must hold.
    pub negated: bool,
}

/// Whether a step runs, by its condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// It runs.
    Run,
    /// It is skipped.
    Skip,
}

/// A step's outcome is not of the kind its condition tests: the executor
/// answered it with another operation's result, a defect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindMismatch;

impl Condition {
    /// Whether the step carrying this condition runs, given the outcome
    /// of the step it names.
    ///
    /// # Errors
    ///
    /// [`KindMismatch`] when that outcome is a result with no answer to
    /// the check.
    pub fn gate(&self, named: &StepOutcome) -> Result<Gate, KindMismatch> {
        let holds = match named {
            StepOutcome::Skipped => return Ok(Gate::Skip),
            StepOutcome::Refused(_) | StepOutcome::NotThere(_) | StepOutcome::Failed => false,
            StepOutcome::Done(done) => match self.check {
                Check::Matched => done.matched(),
                Check::Found => done.found(),
            }
            .ok_or(KindMismatch)?,
        };
        Ok(if holds == self.negated {
            Gate::Skip
        } else {
            Gate::Run
        })
    }
}

/// One step of a batch: an operation, and the condition it runs under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// The operation.
    pub op: Op,
    /// Its condition; `None` runs it unless a step it refers to was not
    /// done.
    pub when: Option<Condition>,
}

impl Step {
    /// Every earlier step whose result it takes, as a subject, a point or
    /// a selection: each must be done for it to run.
    #[must_use]
    pub fn references(&self) -> Vec<Place> {
        let point = |point: &Point| match point {
            Point::Of(place, _) => Some(*place),
            Point::At(_) => None,
        };
        let mut places: Vec<Option<Place>> = match self.op.subject() {
            Some(Subject::Result(place) | Subject::Item { of: place, .. }) => vec![Some(*place)],
            _ => Vec::new(),
        };
        match &self.op {
            Op::ReadText { at, .. }
            | Op::Locate { at, .. }
            | Op::Move { at, .. }
            | Op::FindText { at, .. } => places.push(point(at)),
            Op::ReadColours { screen, top, .. } => places.extend([Some(*screen), Some(*top)]),
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
                places.extend([point(start), point(end)]);
                if let Some(through) = through {
                    places.extend([point(&through.start), point(&through.end)]);
                }
            }
            Op::CompareSelection { before, after, .. } => {
                places.push(Some(*after));
                if let Before::SelectionOf(place) = before {
                    places.push(Some(*place));
                }
            }
            _ => {}
        }
        places.into_iter().flatten().collect()
    }

    /// Whether it runs, given the outcomes of the steps before it: every
    /// step it takes a result from is done, and its condition holds (the
    /// design's condition table).
    ///
    /// # Errors
    ///
    /// [`KindMismatch`] when its condition names a result with no answer
    /// to the check, a defect of the executor's; a step naming one not
    /// yet run, which the parse rules out, is skipped.
    pub fn gate(&self, earlier: &[StepOutcome]) -> Result<Gate, KindMismatch> {
        let done = |place: Place| matches!(earlier.get(place.index()), Some(StepOutcome::Done(_)));
        if !self.references().into_iter().all(done) {
            return Ok(Gate::Skip);
        }
        match &self.when {
            None => Ok(Gate::Run),
            Some(condition) => earlier
                .get(condition.on.index())
                .map_or(Ok(Gate::Skip), |named| condition.gate(named)),
        }
    }
}

/// Where a reader takes a batch among its requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Placement {
    /// In the event queue's order.
    #[default]
    Queue,
    /// Outside the event queue, after every request of the queue's waiting
    /// with it: a tree dump, the taskbar list's reads, a terminal's reads
    /// for its output.
    Outside,
}

/// A batch as it travels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    /// The steps, in order, at most [`MAX_STEPS`].
    pub steps: Vec<Step>,
    /// Where the reader takes it.
    pub placement: Placement,
    /// A node and text anchor of another request Core keeps held while
    /// this one is out, a caret key's starting point while it confirms a
    /// focus; the reader reads nothing of it.
    pub spare: Option<(NodeId, TextAnchor)>,
}

impl Batch {
    /// Whether its first step reads the focused window or the
    /// application's focus: the one batch the shell, finding its window
    /// hung, answers unsent with what is in front, where it answers any
    /// other "not responding".
    #[must_use]
    pub fn reads_focus_first(&self) -> bool {
        matches!(
            self.steps.first().and_then(|step| step.op.subject()),
            Some(
                Subject::Focus
                    | Subject::Window {
                        which: WindowOf::Focused,
                        ..
                    }
            )
        )
    }

    /// Whether it tests the application's focus for the focused state
    /// before reading it: the check after a menu, popup or switcher end,
    /// NVDA's fake focus, which the shell answers unsent with the desktop
    /// when no window is in front.
    #[must_use]
    pub fn reads_focus_if_focused(&self) -> bool {
        self.steps.iter().any(|step| {
            matches!(
                &step.op,
                Op::Test {
                    subject: Subject::Focus,
                    matcher,
                } if matcher.properties().contains(&PropertyId::FocusedWithin)
            )
        })
    }
}

/// A [`Op::Test`]'s result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tested {
    /// Whether it matched.
    pub matched: bool,
    /// The node, its nearest window, and the values the test read.
    pub node: NodeRead,
}

/// Where a [`Op::Ancestors`] walk ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AncestorsEnd {
    /// It met the ancestor Core knows at this place in its list, which is
    /// not among the ancestors.
    Met(u32),
    /// It reached the top.
    Top,
    /// It reached a window below the process boundary: the chain goes on
    /// above it by plain windows.
    ProcessBoundary,
    /// A step failed or found none, or reached a ghost's or a hung
    /// application's window: the chain goes on from this window's own
    /// object.
    ContinueFrom(WindowHandle),
    /// It was cut by its depth: the chain goes on by plain windows from
    /// this window, included.
    PlainFrom(WindowHandle),
    /// It was cut by its depth with no window to go on from.
    DepthLimit,
}

/// A [`Op::Ancestors`]' result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ancestry {
    /// The ancestors, nearest first.
    pub ancestors: Vec<NodeRead>,
    /// Where the walk ended.
    pub end: AncestorsEnd,
}

/// One node a walk down the tree answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descendant {
    /// The node.
    pub node: NodeRead,
    /// Its parent's place in the list, `None` for a node whose parent was
    /// not answered.
    pub parent: Option<u32>,
    /// Which start it lies under: its place among the nodes the walk
    /// started from.
    pub root: u32,
}

/// A [`Op::Descendants`]' result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descended {
    /// The nodes answered, in tree order.
    pub nodes: Vec<Descendant>,
    /// Whether the walk visited every node it was asked to: neither its
    /// node cap nor a failed step cut it.
    pub complete: bool,
}

/// A selection's two ends as a [`Op::Caret`] read them, each held by the
/// outpost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldSelection {
    /// The anchor its start is held at.
    pub start: TextAnchor,
    /// The anchor its end is held at.
    pub end: TextAnchor,
    /// Whether its start is at the text's start, read only when something
    /// is selected, so that a caret report knows the selection starts on
    /// the text's first line (Dickson, 2026-11-22).
    pub at_text_start: bool,
}

/// A [`Op::Caret`]'s result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaretAt {
    /// The anchor the caret is held at.
    pub caret: TextAnchor,
    /// The selection, when something is selected.
    pub selection: Option<HeldSelection>,
    /// The selection's range as held, the next selection baseline.
    pub held: TextAnchor,
    /// The selection's character offsets, where the text counts by
    /// offsets (an edit control's).
    pub offsets: Option<EditSelection>,
}

/// A [`Op::ReadText`]'s result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunks {
    /// How many units the movement went, signed as asked: fewer at the
    /// text's ends.
    pub moved: i32,
    /// The units read, in order, at least one.
    pub chunks: Vec<TextChunk>,
    /// For a trimmed read, each line's length with its padding, in
    /// characters, as runs of lines of one length: how many lines, and
    /// their length.
    #[serde(default)]
    pub padded: Option<Vec<(u32, u32)>>,
}

/// A [`Op::ReadRange`]'s result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RangeText {
    /// The text, cut to [`crate::MAX_RANGE_BYTES`].
    pub text: String,
    /// Whether it was cut.
    pub truncated: bool,
    /// For a trimmed read, each line's padded length, as
    /// [`Chunks::padded`].
    #[serde(default)]
    pub padded: Option<Vec<(u32, u32)>>,
}

/// A [`Op::Visible`]'s result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisibleText {
    /// Its start, held.
    pub start: TextAnchor,
    /// Its end, held.
    pub end: TextAnchor,
    /// Whether it starts at the text's start.
    pub at_text_start: bool,
    /// Its text, each line without the spaces at its end.
    pub text: String,
    /// Each line's padded length, as [`Chunks::padded`].
    pub padded: Vec<(u32, u32)>,
}

/// A [`Op::Move`]'s result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moved {
    /// How many units it went, signed as asked: fewer at the text's ends.
    pub moved: i32,
    /// The point it started from, held.
    pub from: TextAnchor,
    /// Where it landed, held.
    pub to: TextAnchor,
}

/// One match a [`Op::FindText`] answered.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundText {
    /// How many units a move from the start of the match's unit to the
    /// text's end went.
    pub to_end: u32,
    /// The text read from the start of the match's unit, as the provider
    /// gave it.
    pub text: String,
}

/// A [`Op::CompareSelection`]'s result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionCompared {
    /// What became selected and unselected, and the whole new selection.
    pub difference: SelectionDifference,
    /// Whether the text changed since the selection compared with.
    pub text_changed: bool,
}

/// A [`Op::Select`]'s result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selected {
    /// When the selection began, in microseconds on the run's shared
    /// clock.
    pub began_us: u64,
}

/// A point's place in text that counts by offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextOffset {
    /// The point's offset from the text's start.
    pub offset: u32,
    /// The text's length.
    pub length: u32,
}

/// A [`Op::Locate`]'s result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Located {
    /// The point on the screen, in pixels, when the provider can tell.
    pub screen: Option<(i32, i32)>,
    /// Its offset, where the text counts by offsets.
    pub offset: Option<TextOffset>,
}

/// A [`Op::WaitForCaret`]'s result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Waited {
    /// The key's effect was seen.
    Effect,
    /// The bound passed with no effect seen.
    Bound,
}

/// An operation's result, of its own kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Done {
    /// [`Op::Read`]'s.
    Read(NodeRead),
    /// [`Op::Test`]'s.
    Test(Tested),
    /// [`Op::Related`]'s: the objects reached, none at an edge.
    Related(Vec<NodeRead>),
    /// [`Op::Ancestors`]'.
    Ancestors(Ancestry),
    /// [`Op::Descendants`]'.
    Descendants(Descended),
    /// [`Op::IsOneOf`]'s: the place of the node it is, the equal node
    /// first, then the nodes at their addresses; `None` for none.
    IsOneOf(Option<u32>),
    /// [`Op::Activate`]'s: the action's name, when it has one.
    Activate(Option<ActionName>),
    /// [`Op::Follow`]'s: applied. The node followed is the one Core named,
    /// so the result copies nothing back (Claude, 2026-11-21).
    Follow,
    /// [`Op::FocusRead`]'s.
    FocusRead(Box<FocusRead>),
    /// [`Op::Caret`]'s.
    Caret(CaretAt),
    /// [`Op::ReadText`]'s.
    ReadText(Chunks),
    /// [`Op::ReadRange`]'s.
    ReadRange(RangeText),
    /// [`Op::CompareSelection`]'s.
    CompareSelection(SelectionCompared),
    /// [`Op::Select`]'s.
    Select(Selected),
    /// [`Op::Locate`]'s.
    Locate(Located),
    /// [`Op::MoveBySentence`]'s.
    MoveBySentence(SentenceMoved),
    /// [`Op::WaitForCaret`]'s.
    WaitForCaret(Waited),
    /// [`Op::Visible`]'s.
    Visible(VisibleText),
    /// [`Op::Move`]'s.
    Move(Moved),
    /// [`Op::FindText`]'s: the matches, the nearest first.
    FindText(Vec<FoundText>),
    /// [`Op::ReadColours`]'s: each row's runs, from the first.
    ReadColours(Vec<Vec<crate::TerminalColourRun>>),
}

impl Done {
    /// Its kind.
    #[must_use]
    pub const fn kind(&self) -> OpKind {
        match self {
            Self::Read(_) => OpKind::Read,
            Self::Test(_) => OpKind::Test,
            Self::Related(_) => OpKind::Related,
            Self::Ancestors(_) => OpKind::Ancestors,
            Self::Descendants(_) => OpKind::Descendants,
            Self::IsOneOf(_) => OpKind::IsOneOf,
            Self::Activate(_) => OpKind::Activate,
            Self::Follow => OpKind::Follow,
            Self::FocusRead(_) => OpKind::FocusRead,
            Self::Caret(_) => OpKind::Caret,
            Self::ReadText(_) => OpKind::ReadText,
            Self::ReadRange(_) => OpKind::ReadRange,
            Self::CompareSelection(_) => OpKind::CompareSelection,
            Self::Select(_) => OpKind::Select,
            Self::Locate(_) => OpKind::Locate,
            Self::MoveBySentence(_) => OpKind::MoveBySentence,
            Self::WaitForCaret(_) => OpKind::WaitForCaret,
            Self::Visible(_) => OpKind::Visible,
            Self::Move(_) => OpKind::Move,
            Self::FindText(_) => OpKind::FindText,
            Self::ReadColours(_) => OpKind::ReadColours,
        }
    }

    /// The nodes it answered, in order: one for a node's result, the list
    /// for a list's, none for a result that names no node. A later step
    /// names them by [`Subject::Result`] and [`Subject::Item`].
    #[must_use]
    pub fn nodes(&self) -> Vec<&NodeRead> {
        match self {
            Self::Read(node) => vec![node],
            Self::Test(tested) => vec![&tested.node],
            Self::FocusRead(focus) => vec![&focus.focus.read],
            Self::Related(nodes) => nodes.iter().collect(),
            Self::Ancestors(ancestry) => ancestry.ancestors.iter().collect(),
            Self::Descendants(descended) => descended.nodes.iter().map(|each| &each.node).collect(),
            Self::IsOneOf(_)
            | Self::Activate(_)
            | Self::Follow
            | Self::Caret(_)
            | Self::ReadText(_)
            | Self::ReadRange(_)
            | Self::CompareSelection(_)
            | Self::Select(_)
            | Self::Locate(_)
            | Self::MoveBySentence(_)
            | Self::WaitForCaret(_)
            | Self::Visible(_)
            | Self::Move(_)
            | Self::FindText(_)
            | Self::ReadColours(_) => Vec::new(),
        }
    }

    /// Whether a [`Op::Test`] matched; `None` for any other result.
    #[must_use]
    pub const fn matched(&self) -> Option<bool> {
        match self {
            Self::Test(tested) => Some(tested.matched),
            _ => None,
        }
    }

    /// Whether the result found what a [`Check::Found`] asks; `None` for a
    /// result of a kind that finds nothing ([`OpKind::finds`]).
    #[must_use]
    pub fn found(&self) -> Option<bool> {
        match self {
            Self::Read(_) | Self::Test(_) | Self::FocusRead(_) | Self::Move(_) => Some(true),
            Self::Related(nodes) => Some(!nodes.is_empty()),
            Self::Ancestors(ancestry) => Some(matches!(ancestry.end, AncestorsEnd::Met(_))),
            Self::Descendants(descended) => Some(!descended.nodes.is_empty()),
            Self::IsOneOf(place) => Some(place.is_some()),
            Self::WaitForCaret(waited) => Some(*waited == Waited::Effect),
            Self::Caret(caret) => Some(caret.selection.is_some()),
            Self::Activate(_)
            | Self::Follow
            | Self::ReadText(_)
            | Self::ReadRange(_)
            | Self::CompareSelection(_)
            | Self::Select(_)
            | Self::Locate(_)
            | Self::MoveBySentence(_)
            | Self::Visible(_)
            | Self::FindText(_)
            | Self::ReadColours(_) => None,
        }
    }
}

/// Why an operation was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StepRefusal {
    /// The subject has no text interface.
    NoText,
    /// The text has no such unit (UI Automation has no sentence).
    NoSuchUnit,
    /// The subject cannot do what was asked.
    Unsupported,
    /// A point Core holds lies past the text's end, or the provider will
    /// not compare it with the text.
    OutsideText,
}

/// What an operation found not there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Missing {
    /// A node the reader no longer holds.
    Node(NodeId),
    /// A text anchor the reader no longer holds.
    Anchor(TextAnchor),
    /// The object a subject names that holds no node id: a kept element,
    /// an MSAA object, a route, a window, the focus, or an item past its
    /// list's end (Claude, 2026-11-21).
    Object,
}

/// One operation's outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepOutcome {
    /// Its result.
    Done(Done),
    /// Refused, with the reason.
    Refused(StepRefusal),
    /// What it names is not there.
    NotThere(Missing),
    /// A call failed in a window not reported hung, after the fallback.
    Failed,
    /// Not run: its condition did not hold, or a step it names was not
    /// done.
    Skipped,
}
