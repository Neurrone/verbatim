//! The parse of a batch as it arrives: one function every reader runs
//! once, turning a [`Batch`] into a [`Parsed`] one or refusing it whole,
//! so that none of a batch that breaks a bound runs and no executor checks
//! a bound again.

use serde::{Deserialize, Serialize};

use super::{
    Batch, Before, Check, MAX_BOUND_US, MAX_DEPTH, MAX_NODES, MAX_STEPS, MAX_UNITS, MIN_POLL_US,
    Op, OpKind, Place, Point, PointOf, Step, Subject, Through,
};
use crate::TextPoint;
use crate::matcher::Matcher;
use crate::property::PropertyId;
use crate::tell::ElementToken;

/// Which reader a batch arrives at, which decides the operations it runs
/// (`books/dev/src/explanation/architecture.md`, the readers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReaderKind {
    /// The focus listener: [`Op::Read`], [`Op::Ancestors`],
    /// [`Op::Descendants`] and [`Op::IsOneOf`] of kept elements, call by
    /// call, and [`Op::Read`] of an MSAA object at its event's address, of
    /// [`LISTENER_MSAA_SET`] alone.
    Listener,
    /// An application's outpost: every operation.
    Outpost,
    /// Core's plain reader, which answers from the shell's facts: every
    /// operation but [`Op::Ancestors`] and [`Op::FocusRead`].
    Plain,
}

/// The properties the listener reads of an MSAA object at its event's
/// address: name, role, value, states and rectangle.
pub const LISTENER_MSAA_SET: [PropertyId; 5] = [
    PropertyId::Name,
    PropertyId::Role,
    PropertyId::Value,
    PropertyId::States,
    PropertyId::Rect,
];

/// A bound a step broke, with the value it asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Bound {
    /// A walk's depth over [`MAX_DEPTH`].
    Depth(u32),
    /// A walk's node cap over [`MAX_NODES`].
    Nodes(u32),
    /// A walk's limit over its node cap.
    Limit(u32),
    /// A read's count outside 1 to [`MAX_UNITS`].
    Count(u8),
    /// A caret wait's poll under [`MIN_POLL_US`].
    Poll(u32),
    /// A caret wait's bound over [`MAX_BOUND_US`].
    Wait(u32),
}

/// Why a batch was refused whole, a defect of its sender's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Malformed {
    /// It has no step (Claude, 2026-11-21).
    Empty,
    /// It has more than [`MAX_STEPS`] steps.
    TooLong(usize),
    /// A step names a step that is not before it.
    Reference {
        /// The step.
        step: Place,
        /// The place it names.
        to: Place,
    },
    /// A step names an earlier step whose result is of a kind it cannot
    /// take there.
    WrongKind {
        /// The step.
        step: Place,
        /// The place it names.
        to: Place,
        /// That step's kind.
        kind: OpKind,
    },
    /// A step breaks a bound.
    Bound {
        /// The step.
        step: Place,
        /// The bound.
        bound: Bound,
    },
    /// A caret wait is not the batch's first step, so a batch holds at
    /// most one.
    WaitNotFirst(Place),
    /// A matcher compares a property with a value not of its type.
    MatcherValue {
        /// The step.
        step: Place,
        /// The property.
        property: PropertyId,
    },
    /// Two steps name one kept element with different marks, which one
    /// read of it cannot both take.
    KeepMarks(ElementToken),
    /// The receiving reader does not run this operation on this subject.
    NotRun {
        /// The step.
        step: Place,
        /// Its operation.
        op: OpKind,
    },
    /// The operation does not take a subject of this form: a text
    /// operation takes a node or an earlier result, a focus read an MSAA
    /// object, the focus or a route.
    Subject(Place),
    /// A step names a text anchor of a subject that is no node, though an
    /// anchor means something only with the node it was minted for (Claude, 2026-11-21).
    AnchorWithoutNode(Place),
}

/// A batch that parsed: every bound kept, every reference to an earlier
/// step of a kind that can be taken there, and every operation one the
/// reader runs. Only [`parse`] makes one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    batch: Batch,
}

impl Parsed {
    /// The batch.
    #[must_use]
    pub const fn batch(&self) -> &Batch {
        &self.batch
    }

    /// Its steps, in order.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.batch.steps
    }

    /// The batch, given up.
    #[must_use]
    pub fn into_batch(self) -> Batch {
        self.batch
    }
}

/// Parses `batch` as it arrives at a reader of kind `reader`.
///
/// # Errors
///
/// [`Malformed`], the first defect found; the batch is refused whole.
pub fn parse(batch: Batch, reader: ReaderKind) -> Result<Parsed, Malformed> {
    if batch.steps.is_empty() {
        return Err(Malformed::Empty);
    }
    if batch.steps.len() > MAX_STEPS {
        return Err(Malformed::TooLong(batch.steps.len()));
    }
    let mut kinds = Vec::with_capacity(batch.steps.len());
    let mut kept: Vec<(u64, ElementToken, bool)> = Vec::new();
    for (index, step) in batch.steps.iter().enumerate() {
        let place = Place(u8::try_from(index).map_err(|_| Malformed::TooLong(batch.steps.len()))?);
        let checking = Checking {
            place,
            earlier: &kinds,
        };
        checking.step(step, reader, &mut kept)?;
        kinds.push(step.op.kind());
    }
    Ok(Parsed { batch })
}

/// One step's checks, against the kinds of the steps before it.
struct Checking<'a> {
    place: Place,
    earlier: &'a [OpKind],
}

impl Checking<'_> {
    fn step(
        &self,
        step: &Step,
        reader: ReaderKind,
        kept: &mut Vec<(u64, ElementToken, bool)>,
    ) -> Result<(), Malformed> {
        let kind = step.op.kind();
        if let Some(subject) = step.op.subject() {
            self.subject(kind, subject, kept)?;
        }
        self.runs(reader, &step.op)?;
        self.op(&step.op)?;
        if let Some(condition) = &step.when {
            match condition.check {
                Check::Matched => self.names(condition.on, |kind| kind == OpKind::Test)?,
                Check::Found => self.names(condition.on, OpKind::finds)?,
            }
        }
        Ok(())
    }

    /// The step at `to`, which must be before this one and of a kind
    /// `takes` accepts.
    fn names(&self, to: Place, takes: impl Fn(OpKind) -> bool) -> Result<(), Malformed> {
        let kind = self
            .earlier
            .get(to.index())
            .copied()
            .ok_or(Malformed::Reference {
                step: self.place,
                to,
            })?;
        if takes(kind) {
            Ok(())
        } else {
            Err(Malformed::WrongKind {
                step: self.place,
                to,
                kind,
            })
        }
    }

    fn subject(
        &self,
        op: OpKind,
        subject: &Subject,
        kept: &mut Vec<(u64, ElementToken, bool)>,
    ) -> Result<(), Malformed> {
        let fits = match subject {
            Subject::Node(_) | Subject::Result(_) | Subject::Item { .. } => op != OpKind::FocusRead,
            Subject::Msaa { .. } | Subject::Route(_) | Subject::Focus => !op.is_text(),
            Subject::Kept { .. } | Subject::Window { .. } => {
                !op.is_text() && op != OpKind::FocusRead
            }
        };
        if !fits {
            return Err(Malformed::Subject(self.place));
        }
        match subject {
            // A walk down the tree starts from every node of a list; any
            // other operation takes one node.
            Subject::Result(to) => self.names(*to, |kind| {
                one_node(kind) || (op == OpKind::Descendants && many_nodes(kind))
            }),
            Subject::Item { of, .. } => self.names(*of, many_nodes),
            Subject::Kept {
                listener,
                token,
                keep,
            } => {
                match kept
                    .iter()
                    .find(|(other, held, _)| other == listener && held == token)
                {
                    Some((_, _, mark)) if mark != keep => Err(Malformed::KeepMarks(*token)),
                    Some(_) => Ok(()),
                    None => {
                        kept.push((*listener, *token, *keep));
                        Ok(())
                    }
                }
            }
            Subject::Node(_)
            | Subject::Msaa { .. }
            | Subject::Route(_)
            | Subject::Window { .. }
            | Subject::Focus => Ok(()),
        }
    }

    /// Whether the reader runs `op`.
    fn runs(&self, reader: ReaderKind, op: &Op) -> Result<(), Malformed> {
        let runs = match reader {
            ReaderKind::Outpost => true,
            ReaderKind::Plain => !matches!(
                op.kind(),
                OpKind::Ancestors | OpKind::FocusRead | OpKind::TerminalRead
            ),
            ReaderKind::Listener => match op {
                Op::Read {
                    subject: Subject::Msaa { .. },
                    set,
                } => set.iter().all(|id| LISTENER_MSAA_SET.contains(id)),
                Op::Read { subject, .. }
                | Op::Ancestors { subject, .. }
                | Op::IsOneOf { subject, .. }
                // Only on an element it kept: its read answers that
                // element's own values, never an earlier step's node.
                | Op::Descendants { subject, .. } => matches!(subject, Subject::Kept { .. }),
                _ => false,
            },
        };
        if runs {
            Ok(())
        } else {
            Err(Malformed::NotRun {
                step: self.place,
                op: op.kind(),
            })
        }
    }

    /// The operation's own inputs: its points, bounds and matchers.
    fn op(&self, op: &Op) -> Result<(), Malformed> {
        let on_node = matches!(op.subject(), Some(Subject::Node(_)));
        let anchor = |named: bool| {
            if named && !on_node {
                Err(Malformed::AnchorWithoutNode(self.place))
            } else {
                Ok(())
            }
        };
        let bound = |broken: bool, bound: Bound| {
            if broken {
                Err(Malformed::Bound {
                    step: self.place,
                    bound,
                })
            } else {
                Ok(())
            }
        };
        match op {
            Op::Test { matcher, .. } => self.matcher(matcher),
            Op::Ancestors { depth, .. } => bound(*depth > MAX_DEPTH, Bound::Depth(*depth)),
            Op::Descendants { walk: descend, .. } => {
                bound(descend.depth > MAX_DEPTH, Bound::Depth(descend.depth))?;
                bound(descend.nodes > MAX_NODES, Bound::Nodes(descend.nodes))?;
                bound(descend.limit > descend.nodes, Bound::Limit(descend.limit))?;
                for matcher in descend.matcher.iter().chain(&descend.prune) {
                    self.matcher(matcher)?;
                }
                Ok(())
            }
            Op::ReadText { at, count, .. } => {
                bound(!(1..=MAX_UNITS).contains(count), Bound::Count(*count))?;
                anchor(self.point(*at)?)
            }
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
                let mut named = self.point(*start)? | self.point(*end)?;
                if let Some(Through { start, end }) = through {
                    named |= self.point(*start)? | self.point(*end)?;
                }
                anchor(named)
            }
            Op::Locate { at, .. } => anchor(self.point(*at)?),
            Op::CompareSelection { before, after, .. } => {
                self.names(*after, |kind| kind == OpKind::Caret)?;
                match before {
                    Before::Baseline(_) => anchor(true),
                    Before::SelectionOf(to) => self.names(*to, |kind| kind == OpKind::Caret),
                }
            }
            Op::MoveBySentence { .. } => anchor(true),
            Op::TerminalRead { ask, .. } => anchor(ask.end.is_some() || ask.aside.is_some()),
            Op::WaitForCaret { wait, .. } => {
                if self.place != Place(0) {
                    return Err(Malformed::WaitNotFirst(self.place));
                }
                bound(wait.poll_us < MIN_POLL_US, Bound::Poll(wait.poll_us))?;
                bound(wait.bound_us > MAX_BOUND_US, Bound::Wait(wait.bound_us))?;
                anchor(true)
            }
            Op::Read { .. }
            | Op::Related { .. }
            | Op::IsOneOf { .. }
            | Op::Activate { .. }
            | Op::Follow(_)
            | Op::FocusRead { .. }
            | Op::Caret { .. } => Ok(()),
        }
    }

    /// Checks a point's reference; whether it names an anchor.
    fn point(&self, point: Point) -> Result<bool, Malformed> {
        match point {
            Point::At(TextPoint::At(_)) => Ok(true),
            Point::At(_) => Ok(false),
            Point::Of(to, PointOf::ChunkStart) => {
                self.names(to, |kind| kind == OpKind::ReadText)?;
                Ok(false)
            }
            Point::Of(to, PointOf::Caret | PointOf::SelectionStart | PointOf::SelectionEnd) => {
                self.names(to, |kind| kind == OpKind::Caret)?;
                Ok(false)
            }
        }
    }

    /// Every value a matcher compares with is of its property's type.
    fn matcher(&self, matcher: &Matcher) -> Result<(), Malformed> {
        match matcher
            .values()
            .into_iter()
            .find(|(id, value)| value.kind() != id.entry().kind)
        {
            Some((property, _)) => Err(Malformed::MatcherValue {
                step: self.place,
                property,
            }),
            None => Ok(()),
        }
    }
}

/// Whether a result of `kind` is one node, which [`Subject::Result`]
/// names; a list's node is named by [`Subject::Item`], and only a walk down
/// the tree takes a whole list as its start (Claude, 2026-11-21).
fn one_node(kind: OpKind) -> bool {
    matches!(kind, OpKind::Read | OpKind::Test | OpKind::FocusRead)
}

/// Whether a result of `kind` is a list of nodes.
fn many_nodes(kind: OpKind) -> bool {
    matches!(
        kind,
        OpKind::Related | OpKind::Ancestors | OpKind::Descendants
    )
}
