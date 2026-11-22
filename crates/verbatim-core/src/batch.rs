//! Core's builder of batches (`verbatim_model::batch`): a function writes
//! a batch from the request list and reads each step's outcome typed by
//! its operation's result, so that no behaviour builds a request kind, a
//! reply or a decoder of its own.
//!
//! A handle to a step, [`Ref`], carries the step's place, the kind of its
//! result and the batch's brand as types. The brand is the type of a
//! closure written where the batch is built (`cx.batch(|| ())`): every
//! closure expression has a type of its own, so the brands of two call
//! sites differ, and the brand travels through the await inside
//! [`Answers`]. A handle exists only once its step is added, so a forward
//! reference cannot be written; a method that takes an earlier result
//! accepts only its own brand and a kind that can be taken there.
//!
//! A handle of a kind with no caret is no point:
//!
//! ```compile_fail,E0599
//! use verbatim_core::batch::{Builder, batch};
//! use verbatim_model::{NodeId, TextDetails, TextPoint, TextUnit};
//!
//! let node = NodeId::new(12);
//! let mut start = batch(|| ());
//! let line = start.read_text(node, TextPoint::Caret.into(), None, TextUnit::Line, 1, TextDetails::NONE);
//! let _ = start.read_text(node, line.caret(), None, TextUnit::Word, 1, TextDetails::NONE);
//! ```
//!
//! Another call site's handle is no input:
//!
//! ```compile_fail,E0308
//! use verbatim_core::batch::batch;
//! use verbatim_model::{NodeId, TextDetails, TextUnit};
//!
//! let node = NodeId::new(12);
//! let mut start = batch(|| ());
//! let caret = start.caret(node, None);
//! let mut watch = batch(|| ());
//! let _ = watch.read_text(node, caret.caret(), None, TextUnit::Line, 1, TextDetails::NONE);
//! ```
//!
//! and cannot be read from another call site's answers:
//!
//! ```compile_fail,E0308
//! use verbatim_core::batch::{Answers, batch};
//! use verbatim_model::NodeId;
//! use verbatim_model::batch::StepOutcome;
//! use verbatim_model::request::Reply;
//!
//! let node = NodeId::new(12);
//! let mut start = batch(|| ());
//! let _ = start.caret(node, None);
//! let sent = start.finish();
//! let answers = Answers::parse(&sent, Reply::Steps(vec![StepOutcome::Failed])).unwrap();
//! let mut watch = batch(|| ());
//! let after = watch.caret(node, None);
//! let _ = answers.get(after);
//! ```
//!
//! The one call site that builds batches in a loop, whose batches share
//! the closure's brand, numbers them too
//! (`cx.batch_numbered(|| ())`): a handle given to, or
//! read from the answers of, a batch of another number is a defect,
//! logged and answered failed. A batch held on Core's want list, whose
//! handles cannot cross the step that wants it, takes a brand type named
//! for its one call site ([`Builder::named`], [`Ref::named`]), and its
//! answer is parsed against the batch held with the want.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

use verbatim_model::batch::{
    Batch, Before, Check, Condition, FollowTarget, MAX_STEPS, Missing, Op, Place, Placement, Point,
    PointOf, Relation, Step, StepOutcome, StepRefusal, Subject, Through, Walk, WindowOf,
};
use verbatim_model::matcher::Matcher;
use verbatim_model::property::PropertyId;
use verbatim_model::request::{Front, KnownAncestor, MsaaIds, Question, QueueEffect, Reply, Route};
use verbatim_model::tell::ElementToken;
use verbatim_model::{Backend, NodeId, TextAnchor, TextDetails, TextMovement, TextPoint, TextUnit};

use crate::event_queue::{Reader, Request, RequestWindow};

/// The result kinds, one per operation, each naming the type of its
/// result.
pub mod kind {
    use verbatim_model::batch::{
        Ancestry, CaretAt, Chunks, Descended, Done, Located, RangeText, Selected,
        SelectionCompared, Tested, Waited,
    };
    use verbatim_model::request::{FocusRead as FocusAnswer, NodeRead};
    use verbatim_model::{ActionName, SentenceMoved};

    /// A result kind.
    pub trait Kind {
        /// Its result's type.
        type Out;
        /// `done` as this kind's result, `None` for another kind's.
        fn of(done: &Done) -> Option<&Self::Out>;
    }

    /// A kind whose result is one node, which a later step can take as its
    /// subject.
    pub trait OneNode: Kind {}

    /// A kind whose result is a list of nodes, of which a later step can
    /// take one, or a walk down the tree all.
    pub trait Nodes: Kind {}

    /// A kind whose result a "found" condition tests.
    pub trait Finds: Kind {}

    macro_rules! kinds {
        ($($(#[$doc:meta])* $name:ident => $out:ty, $variant:ident;)*) => {
            $(
                $(#[$doc])*
                #[derive(Debug)]
                pub enum $name {}

                impl Kind for $name {
                    type Out = $out;

                    fn of(done: &Done) -> Option<&$out> {
                        match done {
                            Done::$variant(out) => Some(out),
                            _ => None,
                        }
                    }
                }
            )*
        };
    }

    kinds! {
        /// A read's: the node.
        Read => NodeRead, Read;
        /// A test's.
        Test => Tested, Test;
        /// A step to related objects': the objects reached.
        Related => Vec<NodeRead>, Related;
        /// A walk up the tree's.
        Ancestors => Ancestry, Ancestors;
        /// A walk down the tree's.
        Descendants => Descended, Descendants;
        /// Which of the nodes named the subject is.
        IsOneOf => Option<u32>, IsOneOf;
        /// An activation's: the action's name.
        Activate => Option<ActionName>, Activate;
        /// The caret's.
        Caret => CaretAt, Caret;
        /// A read of text's.
        ReadText => Chunks, ReadText;
        /// A read of a range's.
        ReadRange => RangeText, ReadRange;
        /// A comparison of the selection's.
        CompareSelection => SelectionCompared, CompareSelection;
        /// A selection's.
        Select => Selected, Select;
        /// A point's place.
        Locate => Located, Locate;
        /// A move by sentence's.
        MoveBySentence => SentenceMoved, MoveBySentence;
        /// A caret wait's.
        WaitForCaret => Waited, WaitForCaret;
        /// A terminal's window read's.
        TerminalRead => Box<verbatim_model::TerminalWindow>, TerminalRead;
    }

    /// A follow's: applied, with nothing read.
    #[derive(Debug)]
    pub enum Follow {}

    impl Kind for Follow {
        type Out = ();

        fn of(done: &Done) -> Option<&()> {
            matches!(done, Done::Follow).then_some(&())
        }
    }

    /// A focus read's.
    #[derive(Debug)]
    pub enum FocusRead {}

    impl Kind for FocusRead {
        type Out = FocusAnswer;

        fn of(done: &Done) -> Option<&FocusAnswer> {
            match done {
                Done::FocusRead(read) => Some(read),
                _ => None,
            }
        }
    }

    impl OneNode for Read {}
    impl OneNode for Test {}
    impl OneNode for FocusRead {}
    impl Nodes for Related {}
    impl Nodes for Ancestors {}
    impl Nodes for Descendants {}
    impl Finds for Read {}
    impl Finds for Test {}
    impl Finds for Related {}
    impl Finds for Ancestors {}
    impl Finds for Descendants {}
    impl Finds for IsOneOf {}
    impl Finds for FocusRead {}
    impl Finds for WaitForCaret {}
}

use kind::Kind;

/// The numbers given to numbered batches, and the defects their handles
/// found, which the queue's driver takes into its effects after each
/// poll of the front's function.
#[derive(Clone, Debug, Default)]
pub(crate) struct Ledger(Rc<Book>);

#[derive(Debug, Default)]
struct Book {
    last: Cell<u64>,
    defects: RefCell<Vec<QueueEffect>>,
}

impl Ledger {
    /// The defects found since the last take.
    pub(crate) fn take(&self) -> Vec<QueueEffect> {
        self.0.defects.take()
    }

    fn next(&self) -> u64 {
        let next = self.0.last.get().saturating_add(1);
        self.0.last.set(next);
        next
    }

    fn foreign(&self, batch: Option<u64>, handle: Option<u64>) {
        self.0
            .defects
            .borrow_mut()
            .push(QueueEffect::ForeignHandle { batch, handle });
    }
}

/// A batch's number, when it has one, and the ledger its defects go to.
#[derive(Clone, Debug, Default)]
struct Numbering(Option<(u64, Ledger)>);

impl Numbering {
    fn number(&self) -> Option<u64> {
        self.0.as_ref().map(|(number, _)| *number)
    }

    /// Whether `handle` belongs to this batch; a handle of another is
    /// logged.
    fn owns(&self, handle: Handle) -> bool {
        if handle.number == self.number() {
            return true;
        }
        if let Some((number, ledger)) = &self.0 {
            ledger.foreign(Some(*number), handle.number);
        }
        false
    }
}

/// A handle's place and its batch's number, as an input carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Handle {
    place: u8,
    number: Option<u64>,
}

/// A handle to a step of the batch branded `B`, whose result is of kind
/// `K`.
pub struct Ref<B, K> {
    handle: Handle,
    brand: PhantomData<fn() -> (B, K)>,
}

impl<B, K> Clone for Ref<B, K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B, K> Copy for Ref<B, K> {}

impl<B, K> fmt::Debug for Ref<B, K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "step {}", self.handle.place)
    }
}

impl<B, K> Ref<B, K> {
    /// The handle at `place` of a batch built with a brand type named for
    /// its one call site ([`Builder::named`]), as a constant the site's
    /// later step reads its answer by.
    #[must_use]
    pub const fn named(place: u8) -> Self {
        Self::at(Handle {
            place,
            number: None,
        })
    }

    const fn at(handle: Handle) -> Self {
        Self {
            handle,
            brand: PhantomData,
        }
    }

    fn point(self, which: PointOf) -> At<B> {
        At(Target::Of(self.handle, which), PhantomData)
    }
}

impl<B, K: kind::OneNode> Ref<B, K> {
    /// The node this step answered, as a later step's subject.
    #[must_use]
    pub fn node(self) -> On<B> {
        On(Object::Result(self.handle), PhantomData)
    }
}

impl<B, K: kind::Nodes> Ref<B, K> {
    /// The node at `index` of the list this step answered, as a later
    /// step's subject: not there when the list is shorter.
    #[must_use]
    pub fn item(self, index: u32) -> On<B> {
        On(Object::Item(self.handle, index), PhantomData)
    }

    /// Every node of the list this step answered, as a walk down the
    /// tree's start.
    #[must_use]
    pub fn every(self) -> Start<B> {
        Start(On(Object::Result(self.handle), PhantomData))
    }
}

impl<B, K: kind::Finds> Ref<B, K> {
    /// The condition that this step found what it looked for.
    #[must_use]
    pub fn found(self) -> Cond<B> {
        Cond::new(self.handle, Check::Found)
    }
}

impl<B> Ref<B, kind::Test> {
    /// The condition that this test matched.
    #[must_use]
    pub fn matched(self) -> Cond<B> {
        Cond::new(self.handle, Check::Matched)
    }
}

impl<B> Ref<B, kind::Caret> {
    /// The caret this step read.
    #[must_use]
    pub fn caret(self) -> At<B> {
        self.point(PointOf::Caret)
    }

    /// The selection's start this step read, its caret when nothing is
    /// selected.
    #[must_use]
    pub fn selection_start(self) -> At<B> {
        self.point(PointOf::SelectionStart)
    }

    /// The selection's end this step read, its caret when nothing is
    /// selected.
    #[must_use]
    pub fn selection_end(self) -> At<B> {
        self.point(PointOf::SelectionEnd)
    }
}

impl<B> Ref<B, kind::ReadText> {
    /// The start of the first chunk this step read.
    #[must_use]
    pub fn start(self) -> At<B> {
        self.point(PointOf::ChunkStart)
    }
}

/// A condition on an earlier step of the batch branded `B`.
pub struct Cond<B> {
    handle: Handle,
    check: Check,
    negated: bool,
    brand: PhantomData<fn() -> B>,
}

impl<B> Cond<B> {
    fn new(handle: Handle, check: Check) -> Self {
        Self {
            handle,
            check,
            negated: false,
            brand: PhantomData,
        }
    }
}

impl<B> std::ops::Not for Cond<B> {
    type Output = Self;

    /// The condition's negation.
    fn not(self) -> Self {
        Self {
            negated: !self.negated,
            ..self
        }
    }
}

/// What a subject names before it is resolved against its batch.
#[derive(Clone, Debug)]
enum Object {
    Given(Subject),
    Result(Handle),
    Item(Handle, u32),
}

/// An object subject of the batch branded `B`: one Core names, or an
/// earlier step's node.
pub struct On<B>(Object, PhantomData<fn() -> B>);

impl<B> Clone for On<B> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<B> From<NodeId> for On<B> {
    fn from(node: NodeId) -> Self {
        Self::given(Subject::Node(node))
    }
}

impl<B> On<B> {
    fn given(subject: Subject) -> Self {
        Self(Object::Given(subject), PhantomData)
    }

    /// An element the listener of generation `listener` kept for a tell,
    /// kept as read once the batch ends when `keep`.
    #[must_use]
    pub fn kept(listener: u64, token: ElementToken, keep: bool) -> Self {
        Self::given(Subject::Kept {
            listener,
            token,
            keep,
        })
    }

    /// An MSAA object at `ids`, through NVDA's focus redirect when
    /// `redirect`.
    #[must_use]
    pub fn msaa(ids: MsaaIds, redirect: bool) -> Self {
        Self::given(Subject::Msaa { ids, redirect })
    }

    /// The outpost's copy of the element the listener read by `route`.
    #[must_use]
    pub fn route(route: Route) -> Self {
        Self::given(Subject::Route(Box::new(route)))
    }

    /// A window's own object, read through `backend`; only a shown one
    /// when `visible`.
    #[must_use]
    pub fn window(which: WindowOf, backend: Backend, visible: bool) -> Self {
        Self::given(Subject::Window {
            which,
            backend,
            visible,
        })
    }

    /// The application's focus.
    #[must_use]
    pub fn focus() -> Self {
        Self::given(Subject::Focus)
    }
}

/// Where a walk down the tree of the batch branded `B` starts: one object,
/// or every node of an earlier list.
pub struct Start<B>(On<B>);

impl<B> From<On<B>> for Start<B> {
    fn from(on: On<B>) -> Self {
        Self(on)
    }
}

impl<B> From<NodeId> for Start<B> {
    fn from(node: NodeId) -> Self {
        Self(On::from(node))
    }
}

#[derive(Clone, Copy, Debug)]
enum Target {
    Given(TextPoint),
    Of(Handle, PointOf),
}

/// A point in a text subject's text of the batch branded `B`: one Core
/// names, or a point of an earlier step's result.
pub struct At<B>(Target, PhantomData<fn() -> B>);

impl<B> Clone for At<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B> Copy for At<B> {}

impl<B> From<TextPoint> for At<B> {
    fn from(point: TextPoint) -> Self {
        Self(Target::Given(point), PhantomData)
    }
}

/// What a selection comparison of the batch branded `B` compares with.
#[derive(Debug)]
pub enum Baseline<B> {
    /// A selection baseline Core holds.
    Held(TextAnchor),
    /// The selection an earlier caret read.
    Read(Ref<B, kind::Caret>),
}

impl<B> Clone for Baseline<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B> Copy for Baseline<B> {}

/// An input of a step was another batch's handle, or named a step left
/// off for that, so the step is left off too, and read as failed (Claude, 2026-11-21).
struct LeftOff;

/// A batch being built, branded `B`.
pub struct Builder<B> {
    steps: Vec<Step>,
    /// Each step's place on the wire, by its place in the builder; `None`
    /// for a step left off because an input was another batch's handle.
    places: Vec<Option<u8>>,
    placement: Placement,
    spare: Option<(NodeId, TextAnchor)>,
    numbering: Numbering,
    pending: Option<Cond<B>>,
    /// The steps added, and those put on the wire.
    added: u8,
    sent: u8,
}

/// A builder branded by the type of `brand`, a closure written where the
/// batch is built: `cx.batch(|| ())` for an entry's
/// function.
pub fn batch<B: FnOnce()>(brand: B) -> Builder<B> {
    let _ = brand;
    Builder::named()
}

impl<B> Builder<B> {
    /// A builder branded by a type named for its one call site, for a
    /// batch held on Core's want list.
    #[must_use]
    pub fn named() -> Self {
        Self {
            steps: Vec::new(),
            places: Vec::new(),
            placement: Placement::Queue,
            spare: None,
            numbering: Numbering::default(),
            pending: None,
            added: 0,
            sent: 0,
        }
    }

    /// A builder whose batch has its own number from `ledger`.
    pub(crate) fn numbered(ledger: &Ledger) -> Self {
        Self {
            numbering: Numbering(Some((ledger.next(), ledger.clone()))),
            ..Self::named()
        }
    }

    /// The batch is asked from outside the event queue.
    pub fn outside(&mut self) -> &mut Self {
        self.placement = Placement::Outside;
        self
    }

    /// The reader keeps `node`'s `anchor` held while the batch is out.
    pub fn spare(&mut self, node: NodeId, anchor: TextAnchor) -> &mut Self {
        self.spare = Some((node, anchor));
        self
    }

    /// The next step added runs only when `condition` holds (Claude, 2026-11-21).
    pub fn when(&mut self, condition: Cond<B>) -> &mut Self {
        self.pending = Some(condition);
        self
    }

    /// The batch, built.
    ///
    /// # Panics
    ///
    /// When a condition was given and no step after it, a defect in the
    /// function building the batch.
    #[must_use]
    pub fn finish(self) -> Sent<B> {
        assert!(
            self.pending.is_none(),
            "a condition is given for the step after it"
        );
        Sent {
            batch: Batch {
                steps: self.steps,
                placement: self.placement,
                spare: self.spare,
            },
            places: self.places,
            numbering: self.numbering,
            brand: PhantomData,
        }
    }

    /// The wire place of `handle`'s step, `None` when it is another
    /// batch's or its step was left off.
    fn place(&self, handle: Handle) -> Option<Place> {
        if !self.numbering.owns(handle) {
            return None;
        }
        self.places
            .get(usize::from(handle.place))
            .copied()
            .flatten()
            .map(Place)
    }

    fn subject(&self, on: On<B>) -> Option<Subject> {
        match on.0 {
            Object::Given(subject) => Some(subject),
            Object::Result(handle) => self.place(handle).map(Subject::Result),
            Object::Item(handle, index) => self.place(handle).map(|of| Subject::Item { of, index }),
        }
    }

    fn point(&self, at: At<B>) -> Option<Point> {
        match at.0 {
            Target::Given(point) => Some(Point::At(point)),
            Target::Of(handle, which) => self.place(handle).map(|place| Point::Of(place, which)),
        }
    }

    fn through(&self, through: Option<(At<B>, At<B>)>) -> Result<Option<Through>, LeftOff> {
        match through {
            None => Ok(None),
            Some((start, end)) => match (self.point(start), self.point(end)) {
                (Some(start), Some(end)) => Ok(Some(Through { start, end })),
                _ => Err(LeftOff),
            },
        }
    }

    /// Adds `op`, `None` when an input did not resolve, under the pending
    /// condition.
    ///
    /// # Panics
    ///
    /// When the batch already holds [`MAX_STEPS`] steps, a defect in the
    /// function building it.
    fn push<K>(&mut self, op: Option<Op>) -> Ref<B, K> {
        let when = self.pending.take().map(|condition| {
            self.place(condition.handle).map(|on| Condition {
                on,
                check: condition.check,
                negated: condition.negated,
            })
        });
        assert!(
            self.places.len() < MAX_STEPS,
            "a batch holds at most {MAX_STEPS} steps"
        );
        let place = self.added;
        self.added = self.added.saturating_add(1);
        let step = match (op, when) {
            (Some(op), None) => Some(Step { op, when: None }),
            (Some(op), Some(Some(condition))) => Some(Step {
                op,
                when: Some(condition),
            }),
            (None, _) | (_, Some(None)) => None,
        };
        let wire = step.map(|step| {
            let wire = self.sent;
            self.sent = self.sent.saturating_add(1);
            self.steps.push(step);
            wire
        });
        self.places.push(wire);
        Ref::at(Handle {
            place,
            number: self.numbering.number(),
        })
    }

    /// Reads `on`'s properties of `set`.
    pub fn read(&mut self, on: impl Into<On<B>>, set: &[PropertyId]) -> Ref<B, kind::Read> {
        let op = self.subject(on.into()).map(|subject| Op::Read {
            subject,
            set: set.to_vec(),
        });
        self.push(op)
    }

    /// Tests `on` against `matcher`.
    pub fn test(&mut self, on: impl Into<On<B>>, matcher: Matcher) -> Ref<B, kind::Test> {
        let op = self
            .subject(on.into())
            .map(|subject| Op::Test { subject, matcher });
        self.push(op)
    }

    /// Steps from `on` by `relation`, reading `set` of each object reached.
    pub fn related(
        &mut self,
        on: impl Into<On<B>>,
        relation: Relation,
        set: &[PropertyId],
    ) -> Ref<B, kind::Related> {
        let op = self.subject(on.into()).map(|subject| Op::Related {
            subject,
            relation,
            set: set.to_vec(),
        });
        self.push(op)
    }

    /// Walks up from `on` until an ancestor of `until`, at most `depth`
    /// ancestors, reading `set` of each.
    pub fn ancestors(
        &mut self,
        on: impl Into<On<B>>,
        until: Vec<KnownAncestor>,
        depth: u32,
        set: &[PropertyId],
    ) -> Ref<B, kind::Ancestors> {
        let op = self.subject(on.into()).map(|subject| Op::Ancestors {
            subject,
            until,
            depth,
            set: set.to_vec(),
        });
        self.push(op)
    }

    /// Walks down from `start` as `walk` says.
    pub fn descendants(
        &mut self,
        start: impl Into<Start<B>>,
        walk: Walk,
    ) -> Ref<B, kind::Descendants> {
        let op = self.subject(start.into().0).map(|subject| Op::Descendants {
            subject,
            walk: Box::new(walk),
        });
        self.push(op)
    }

    /// Which of `equal`, met by NVDA's equality, and `at_address`, met at
    /// their addresses, `on` is.
    pub fn is_one_of(
        &mut self,
        on: impl Into<On<B>>,
        equal: Option<NodeId>,
        at_address: Vec<NodeId>,
    ) -> Ref<B, kind::IsOneOf> {
        let op = self.subject(on.into()).map(|subject| Op::IsOneOf {
            subject,
            equal,
            at_address,
        });
        self.push(op)
    }

    /// Activates `on`.
    pub fn activate(&mut self, on: impl Into<On<B>>) -> Ref<B, kind::Activate> {
        let op = self
            .subject(on.into())
            .map(|subject| Op::Activate { subject });
        self.push(op)
    }

    /// Follows `on` with its event group and extra object, or nothing.
    pub fn follow(
        &mut self,
        target: Option<(On<B>, verbatim_model::request::FollowGroup, Option<NodeId>)>,
    ) -> Ref<B, kind::Follow> {
        let op = match target {
            None => Some(Op::Follow(None)),
            Some((on, group, extra)) => self.subject(on).map(|subject| {
                Op::Follow(Some(FollowTarget {
                    subject,
                    group,
                    extra,
                }))
            }),
        };
        self.push(op)
    }

    /// Reads `on` as a focus: an MSAA object, the focus or a route.
    pub fn focus_read(
        &mut self,
        on: On<B>,
        known: Vec<KnownAncestor>,
        set: &[PropertyId],
        details: TextDetails,
    ) -> Ref<B, kind::FocusRead> {
        let op = self.subject(on).map(|subject| Op::FocusRead {
            subject,
            known,
            set: set.to_vec(),
            details,
        });
        self.push(op)
    }

    /// Reads the caret and the selection of `node`'s text, adopting the
    /// element of a caret tell when `adopt` names one.
    pub fn caret(&mut self, node: NodeId, adopt: Option<ElementToken>) -> Ref<B, kind::Caret> {
        self.push(Some(Op::Caret {
            subject: Subject::Node(node),
            adopt,
        }))
    }

    /// Reads `count` units of `unit` of `node`'s text from `at`, after
    /// `movement`.
    pub fn read_text(
        &mut self,
        node: NodeId,
        at: At<B>,
        movement: Option<TextMovement>,
        unit: TextUnit,
        count: u8,
        details: TextDetails,
    ) -> Ref<B, kind::ReadText> {
        let op = self.point(at).map(|at| Op::ReadText {
            subject: Subject::Node(node),
            at,
            movement,
            unit,
            count,
            details,
        });
        self.push(op)
    }

    /// Reads `node`'s text between `start` and `end`, through the
    /// character at the later end when `through` names the points past
    /// them.
    pub fn read_range(
        &mut self,
        node: NodeId,
        (start, end): (At<B>, At<B>),
        through: Option<(At<B>, At<B>)>,
    ) -> Ref<B, kind::ReadRange> {
        let op = match (self.point(start), self.point(end), self.through(through)) {
            (Some(start), Some(end), Ok(through)) => Some(Op::ReadRange {
                subject: Subject::Node(node),
                start,
                end,
                through,
            }),
            _ => None,
        };
        self.push(op)
    }

    /// Compares the selection `after` read with `before`.
    pub fn compare_selection(
        &mut self,
        node: NodeId,
        before: Baseline<B>,
        after: Ref<B, kind::Caret>,
    ) -> Ref<B, kind::CompareSelection> {
        let before = match before {
            Baseline::Held(anchor) => Some(Before::Baseline(anchor)),
            Baseline::Read(caret) => self.place(caret.handle).map(Before::SelectionOf),
        };
        let op = match (before, self.place(after.handle)) {
            (Some(before), Some(after)) => Some(Op::CompareSelection {
                subject: Subject::Node(node),
                before,
                after,
            }),
            _ => None,
        };
        self.push(op)
    }

    /// Selects `node`'s text between `start` and `end`; equal points move
    /// the caret.
    pub fn select(
        &mut self,
        node: NodeId,
        (start, end): (At<B>, At<B>),
        through: Option<(At<B>, At<B>)>,
    ) -> Ref<B, kind::Select> {
        let op = match (self.point(start), self.point(end), self.through(through)) {
            (Some(start), Some(end), Ok(through)) => Some(Op::Select {
                subject: Subject::Node(node),
                start,
                end,
                through,
            }),
            _ => None,
        };
        self.push(op)
    }

    /// Locates `at` in `node`'s text.
    pub fn locate(&mut self, node: NodeId, at: At<B>) -> Ref<B, kind::Locate> {
        let op = self.point(at).map(|at| Op::Locate {
            subject: Subject::Node(node),
            at,
        });
        self.push(op)
    }

    /// Moves the caret in `node`'s text by sentence from `from`.
    pub fn move_by_sentence(
        &mut self,
        node: NodeId,
        from: TextAnchor,
        forward: bool,
        details: TextDetails,
    ) -> Ref<B, kind::MoveBySentence> {
        self.push(Some(Op::MoveBySentence {
            subject: Subject::Node(node),
            from,
            forward,
            details,
        }))
    }

    /// Reads terminal `node`'s window as `ask` names it.
    pub fn terminal_read(
        &mut self,
        node: NodeId,
        ask: verbatim_model::WindowAsk,
    ) -> Ref<B, kind::TerminalRead> {
        self.push(Some(Op::TerminalRead {
            subject: Subject::Node(node),
            ask: Box::new(ask),
        }))
    }

    /// Waits for a key's effect on the caret in `node`'s text.
    pub fn wait_for_caret(
        &mut self,
        node: NodeId,
        wait: verbatim_model::batch::CaretWait,
    ) -> Ref<B, kind::WaitForCaret> {
        self.push(Some(Op::WaitForCaret {
            subject: Subject::Node(node),
            wait: Box::new(wait),
        }))
    }
}

/// A built batch, branded `B`.
pub struct Sent<B> {
    batch: Batch,
    places: Vec<Option<u8>>,
    numbering: Numbering,
    brand: PhantomData<fn() -> B>,
}

impl<B> fmt::Debug for Sent<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Sent").field(&self.batch).finish()
    }
}

impl<B> Sent<B> {
    /// The batch as it travels.
    #[must_use]
    pub fn batch(&self) -> &Batch {
        &self.batch
    }

    /// The batch as a request of the shared list.
    #[must_use]
    pub fn question(&self) -> Question {
        Question::Batch(self.batch.clone())
    }

    /// The batch, asked of `reader` naming `window`.
    pub(crate) fn of(self, reader: Reader, window: RequestWindow) -> Asked<B> {
        Asked {
            sent: self,
            reader,
            window,
        }
    }
}

/// A built batch asked of a reader: the one request of the event queue
/// that a batch is.
pub(crate) struct Asked<B> {
    sent: Sent<B>,
    reader: Reader,
    window: RequestWindow,
}

impl<B> fmt::Debug for Asked<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Asked")
            .field("batch", &self.sent.batch)
            .field("reader", &self.reader)
            .finish_non_exhaustive()
    }
}

impl<B: 'static> Request for Asked<B> {
    type Reply = Answers<B>;

    fn reader(&self) -> Reader {
        self.reader
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Question {
        self.sent.question()
    }

    fn decode(&self, reply: Reply) -> Option<Answers<B>> {
        Answers::parse(&self.sent, reply)
    }

    /// Only a batch whose first step reads the focused window or the
    /// application's focus takes what the shell read in front.
    fn decode_in_front(&self, front: Front) -> Option<Answers<B>> {
        self.sent.batch.reads_focus_first().then(|| Answers {
            answered: Answered::InFront(front),
            places: self.sent.places.clone(),
            numbering: self.sent.numbering.clone(),
            brand: PhantomData,
        })
    }
}

impl<B> Asked<B> {
    /// The batch asked, its answers judged by `judge` as they arrive:
    /// `None` from it is a malformed answer, as an answer that does not
    /// parse against the batch is.
    pub(crate) fn judged<T>(
        self,
        judge: impl Fn(&Answers<B>) -> Option<T> + 'static,
    ) -> Judged<B, T> {
        Judged {
            asked: self,
            judge: Box::new(judge),
        }
    }
}

/// How a function judges a batch's answers into what it acts on, `None`
/// for answers no correct reader gives.
type Judge<B, T> = Box<dyn Fn(&Answers<B>) -> Option<T>>;

/// A batch asked, whose answers its function judges once, as they are
/// decoded, into what it acts on.
pub(crate) struct Judged<B, T> {
    asked: Asked<B>,
    judge: Judge<B, T>,
}

impl<B, T> fmt::Debug for Judged<B, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.asked.fmt(formatter)
    }
}

impl<B: 'static, T: fmt::Debug + 'static> Request for Judged<B, T> {
    type Reply = T;

    fn reader(&self) -> Reader {
        self.asked.reader
    }

    fn window(&self) -> RequestWindow {
        self.asked.window
    }

    fn wire(&self) -> Question {
        self.asked.wire()
    }

    fn decode(&self, reply: Reply) -> Option<T> {
        self.asked
            .decode(reply)
            .and_then(|answers| (self.judge)(&answers))
    }

    fn decode_in_front(&self, front: Front) -> Option<T> {
        self.asked
            .decode_in_front(front)
            .and_then(|answers| (self.judge)(&answers))
    }
}

#[derive(Debug)]
enum Answered {
    Steps(Vec<StepOutcome>),
    InFront(Front),
}

/// A batch's answers, parsed once against the batch sent.
pub struct Answers<B> {
    answered: Answered,
    places: Vec<Option<u8>>,
    numbering: Numbering,
    brand: PhantomData<fn() -> B>,
}

impl<B> fmt::Debug for Answers<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Answers")
            .field(&self.answered)
            .finish()
    }
}

/// One step's outcome, its result of the step's kind.
#[derive(Debug, PartialEq, Eq)]
pub enum Got<'a, T> {
    /// Its result.
    Done(&'a T),
    /// Refused, with the reason.
    Refused(StepRefusal),
    /// What it names is not there.
    NotThere(Missing),
    /// A call failed; or its handle was another batch's, a defect.
    Failed,
    /// Not run: its condition did not hold, a step it names was not done,
    /// or the shell answered the batch unsent with what is in front (Claude, 2026-11-21).
    Skipped,
}

/// One step's outcome, owned, as a function that asked for one result
/// acts on it: the result, its object not there, or neither (refused,
/// failed or skipped).
#[derive(Debug)]
pub(crate) enum Single<T> {
    /// The result.
    Done(T),
    /// What the step names is not there.
    NotThere,
    /// Refused, failed or skipped.
    Unanswered,
}

impl<T: Clone> Got<'_, T> {
    /// The outcome, owned.
    pub(crate) fn owned(self) -> Single<T> {
        match self {
            Self::Done(done) => Single::Done(done.clone()),
            Self::NotThere(_) => Single::NotThere,
            Self::Refused(_) | Self::Failed | Self::Skipped => Single::Unanswered,
        }
    }
}

/// The outcome of a request for one step's result as the outcome of the
/// result itself: a step whose object is not there is not there, and one
/// refused, failed or skipped ended the request unanswered.
pub(crate) fn flat<T>(
    outcome: crate::event_queue::Outcome<Single<T>>,
) -> crate::event_queue::Outcome<T> {
    use crate::event_queue::Outcome;
    match outcome.split() {
        Ok(Single::Done(done)) => Outcome::Result(done),
        Ok(Single::NotThere) => Outcome::NotThere,
        Ok(Single::Unanswered) => Outcome::ReaderEnded,
        Err(other) => other,
    }
}

impl<B: 'static> Asked<B> {
    /// The batch asked for the one result of `handle`'s step.
    pub(crate) fn one<K: Kind + 'static>(self, handle: Ref<B, K>) -> Judged<B, Single<K::Out>>
    where
        K::Out: Clone + fmt::Debug,
    {
        self.judged(move |answers| Some(answers.get(handle).owned()))
    }
}

impl<B> Answers<B> {
    /// `reply` as the answers to `sent`: one outcome per step, each result
    /// of its step's kind; `None` for any other reply, a malformed answer.
    #[must_use]
    pub fn parse(sent: &Sent<B>, reply: Reply) -> Option<Self> {
        let Reply::Steps(outcomes) = reply else {
            return None;
        };
        let fits = outcomes.len() == sent.batch.steps.len()
            && sent
                .batch
                .steps
                .iter()
                .zip(&outcomes)
                .all(|(step, outcome)| match outcome {
                    StepOutcome::Done(done) => done.kind() == step.op.kind(),
                    StepOutcome::Refused(_)
                    | StepOutcome::NotThere(_)
                    | StepOutcome::Failed
                    | StepOutcome::Skipped => true,
                });
        fits.then(|| Self {
            answered: Answered::Steps(outcomes),
            places: sent.places.clone(),
            numbering: sent.numbering.clone(),
            brand: PhantomData,
        })
    }

    /// What the shell read in front, when it answered the batch unsent.
    #[must_use]
    pub fn in_front(&self) -> Option<&Front> {
        match &self.answered {
            Answered::InFront(front) => Some(front),
            Answered::Steps(_) => None,
        }
    }

    /// The outcome of `handle`'s step.
    #[must_use]
    pub fn get<K: Kind>(&self, handle: Ref<B, K>) -> Got<'_, K::Out> {
        if !self.numbering.owns(handle.handle) {
            return Got::Failed;
        }
        let Some(place) = self
            .places
            .get(usize::from(handle.handle.place))
            .copied()
            .flatten()
        else {
            // Left off the batch: an input was another batch's handle.
            return Got::Failed;
        };
        let outcomes = match &self.answered {
            Answered::Steps(outcomes) => outcomes,
            Answered::InFront(_) => return Got::Skipped,
        };
        match outcomes.get(usize::from(place)) {
            // The parse took every result of its step's kind, which is the
            // handle's.
            Some(StepOutcome::Done(done)) => K::of(done).map_or(Got::Failed, Got::Done),
            Some(StepOutcome::Refused(refusal)) => Got::Refused(*refusal),
            Some(StepOutcome::NotThere(missing)) => Got::NotThere(*missing),
            Some(StepOutcome::Skipped) => Got::Skipped,
            Some(StepOutcome::Failed) | None => Got::Failed,
        }
    }
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;
