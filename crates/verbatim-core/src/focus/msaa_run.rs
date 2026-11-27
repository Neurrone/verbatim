//! A run of MSAA focus and foreground candidates at the front
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects"): walked by the one definition's run walk
//! (`super::validity::judge_run`), newest first, strictly, across
//! applications, as NVDA's pump has no grouping by application
//! (`IAccessibleHandler/__init__.py` 1087 to 1092). Refusals go on with no
//! request, a duplicate ends the run as its valid focus, and any other
//! answer asks that candidate's outpost to its depth, naming the
//! candidate's window; the answer, judged at its decode, is judged the
//! same way, and a candidate refused after its read is passed over by the
//! walk's next step. The valid focus is executed. One batch per candidate:
//! Core decides every candidate, the outpost only reporting facts
//! (Dickson, 2026-11-18).
//!
//! A candidate's batch (the request list's design, section 6, `MsaaRun`):
//! the hidden-dialog check on its top-level window when its read checks
//! (`super::shown`); a `Test` of the object at its ids whose matcher is
//! the read's depth, its focused state within it and, for the identity
//! check, what it reads as, in ADR 0004's order; a menu start's role by a
//! `Read`; and the `FocusRead` of the same object unless the test matched.

use std::fmt::Debug;

use verbatim_model::batch::{Op, Subject as On_, WindowOf};
use verbatim_model::event_kind::EventKind;
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{FocusRead, HeldIds, MsaaIds, NodeWindow, Question, Refusal};
use verbatim_model::{Backend, NodeId, Pid, Role, StateSet, TextAnchor};

use super::FOCUS_SET;
use super::execute::{Known, Origin, execute};
use super::shown::{self, Check, Hidden};
use super::validity::{self, Answered, CandidateFacts, Settled, Subject, Unread, Validity};
use super::versions::ReadBy;
use crate::batch::{Answers, Builder, Got, On, Ref, kind};
use crate::entries::{Candidate, Entries};
use crate::event_queue::{Cx, FOCUS_DUE_US, Outcome, Reader, Refused};

/// What an MSAA object reads as, which the identity check compares, each
/// value as a full read gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    /// Its states.
    pub(crate) states: StateSet,
    /// Its role.
    pub(crate) role: Role,
    /// Its position in its set.
    pub(crate) position_in_set: Option<u32>,
    /// Its set's size.
    pub(crate) set_size: Option<u32>,
    /// Its name.
    pub(crate) name: Option<String>,
    /// Its value.
    pub(crate) value: Option<String>,
}

/// A focus's turn read in full unless the object reached is at the
/// readers' last focus's ids, `ids`, held by `held_child`, and reads as
/// it does, `identity`: the identity check in the duplicate test
/// (`docs/parity/focus-events.md`, "Duplicate focus suppression"). Made
/// only by [`validity::depth`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnlessSame {
    /// The ids of the readers' last focus.
    pub(crate) ids: MsaaIds,
    /// The child id the outpost held it by.
    pub(crate) held_child: i32,
    /// What the readers' last focus reads as.
    pub(crate) identity: Identity,
}

impl UnlessSame {
    /// The identity check as a matcher, in ADR 0004's order: the ids, then
    /// the states, role, position and set size, name and value. A position
    /// the last focus had holds of an object that answers none, since the
    /// outpost reads one only for a list view's item, the one place the
    /// identity check compares it (Claude, 2026-11-21).
    pub(crate) fn matcher(&self) -> Matcher {
        let identity = &self.identity;
        let count = |id, count: Option<u32>| match count {
            Some(count) => Matcher::Any(vec![
                Matcher::Absent(id),
                Matcher::Is(id, PropertyValue::Count(count)),
            ]),
            None => Matcher::Absent(id),
        };
        let text = |id, text: &Option<String>| match text {
            Some(text) => Matcher::Is(id, PropertyValue::Text(text.clone())),
            None => Matcher::Absent(id),
        };
        Matcher::All(vec![
            Matcher::Is(
                PropertyId::MsaaAddress,
                PropertyValue::MsaaAddress(HeldIds {
                    event: self.ids,
                    held_child: self.held_child,
                }),
            ),
            Matcher::Is(PropertyId::States, PropertyValue::States(identity.states)),
            Matcher::Is(PropertyId::Role, PropertyValue::Role(identity.role)),
            count(PropertyId::PositionInSet, identity.position_in_set),
            count(PropertyId::SetSize, identity.set_size),
            text(PropertyId::Name, &identity.name),
            text(PropertyId::Value, &identity.value),
        ])
    }
}

/// How far a candidate is read at its turn ([`validity::depth`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TurnDepth {
    /// The full read only when the object or an ancestor has the focused
    /// state.
    FullIfFocused,
    /// The full read.
    Full,
    /// As [`TurnDepth::FullIfFocused`], unless the object is the one named.
    UnlessSame(UnlessSame),
}

/// How far a candidate's batch reads it.
#[derive(Clone)]
enum Reach {
    /// Its facts alone: a caret key's confirmation.
    Facts,
    /// At its turn.
    Turn(TurnDepth),
}

/// The matcher that holds of an object neither it nor an ancestor of
/// which has the focused state.
pub(crate) fn unfocused() -> Matcher {
    Matcher::Is(PropertyId::FocusedWithin, PropertyValue::Flag(false))
}

/// Whether an object or an ancestor has the focused state, as `values`
/// read it; `None` where it is not read, a UI Automation object's.
pub(crate) fn focused_within(values: &PropertyValues) -> Option<bool> {
    match values
        .answer(PropertyId::FocusedWithin)
        .and_then(|answer| answer.value.as_ref())
    {
        Some(PropertyValue::Flag(focused)) => Some(*focused),
        _ => None,
    }
}

/// The step that reads a candidate's facts.
enum FactsStep<B> {
    /// A focus's focused state, with the identity check.
    Tested(Ref<B, kind::Test>),
    /// A menu start's role, or a foreground's object found.
    Read(Ref<B, kind::Read>),
    /// None: a foreground's read in full finds it.
    None,
}

/// A candidate's batch, by its steps.
struct Steps<B> {
    hidden: Option<Ref<B, kind::Test>>,
    facts: FactsStep<B>,
    read: Option<Ref<B, kind::FocusRead>>,
}

/// What a candidate's read found.
enum Found {
    /// Its top-level window is a hidden dialog, with nothing more read.
    HiddenDialog(NodeWindow),
    /// Not read further, refused by its facts.
    Unread(Unread),
    /// At the ids and with the identity the read named.
    Same,
    /// Its facts, read alone.
    Facts(CandidateFacts),
    /// Found and read in full.
    Read(CandidateFacts, Box<FocusRead>),
}

/// A candidate's answer at its turn: read to its depth's rule, or, at
/// [`TurnDepth::UnlessSame`], the object the read named.
#[derive(Debug)]
pub(crate) enum TurnAnswer {
    /// Read to its depth's rule.
    Answered(Answered),
    /// The object at the ids and with the identity the read named.
    SameAsNamed,
}

/// Writes `candidate`'s batch into `batch`, to `reach`, with the
/// hidden-dialog check first when `hidden`; its full read by the ancestors
/// Core knows, `known`, and the settings' set and details.
fn build<B>(
    cx: &Cx<Entries>,
    batch: &mut Builder<B>,
    (candidate, reach, hidden): (&Candidate, &Reach, bool),
    known: &Known,
) -> Steps<B> {
    let ids = candidate.ids;
    // A menu start's object is its event's own, with no redirect, as
    // NVDA's (`IAccessibleHandler/__init__.py` 942 to 973).
    let menu_start = matches!(
        candidate.tell.kind,
        EventKind::MenuStart | EventKind::PopupStart
    );
    let object = || On::msaa(ids, !menu_start);
    let hidden = hidden.then(|| {
        let window = On::window(WindowOf::TopLevelOf(ids.window), Backend::Msaa, false);
        let check = batch.test(window, shown::check());
        batch.when(!check.matched());
        check
    });
    let facts = match (menu_start, candidate.tell.kind, reach) {
        (true, _, _) => FactsStep::Read(batch.read(object(), &[PropertyId::Role])),
        (false, EventKind::Foreground, Reach::Facts) => FactsStep::Read(batch.read(object(), &[])),
        (false, EventKind::Foreground, Reach::Turn(_)) => FactsStep::None,
        (false, _, Reach::Turn(TurnDepth::UnlessSame(named))) => FactsStep::Tested(
            batch.test(object(), Matcher::Any(vec![unfocused(), named.matcher()])),
        ),
        (false, _, _) => FactsStep::Tested(batch.test(object(), unfocused())),
    };
    let read = match reach {
        Reach::Facts => None,
        Reach::Turn(depth) => {
            match (&facts, hidden) {
                (FactsStep::Tested(test), _) if *depth != TurnDepth::Full => {
                    batch.when(!test.matched());
                }
                (_, Some(check)) => {
                    batch.when(!check.matched());
                }
                _ => {}
            }
            let facts = cx.facts();
            Some(batch.focus_read(
                object(),
                known.sent.clone(),
                &crate::details::focus_set(&facts, FOCUS_SET),
                crate::details::focus_details(&facts),
            ))
        }
    };
    Steps {
        hidden,
        facts,
        read,
    }
}

/// `answers` to a candidate's batch of `steps`, read to `reach`, judged:
/// `None` for an answer no correct outpost gives.
fn judge<B>(answers: &Answers<B>, steps: &Steps<B>, reach: &Reach) -> Option<Found> {
    if let Some(check) = steps.hidden {
        match shown::judge(&answers.get(check))? {
            Check::Dialog(window) => return Some(Found::HiddenDialog(window)),
            Check::Gone => return Some(Found::Unread(Unread::NotThere)),
            Check::Shown => {}
        }
    }
    let gone = Some(Found::Unread(Unread::NotThere));
    let (focused, role) = match &steps.facts {
        FactsStep::Tested(test) => match answers.get(*test) {
            Got::Done(tested) => {
                let focused = focused_within(&tested.node.values);
                match (reach, tested.matched, focused) {
                    (Reach::Turn(TurnDepth::Full), _, _) | (_, false, _) => (focused, None),
                    (Reach::Facts, true, _) => (Some(false), None),
                    (Reach::Turn(_), true, Some(false)) => {
                        return Some(Found::Unread(Unread::NotFocused));
                    }
                    (Reach::Turn(_), true, _) => return Some(Found::Same),
                }
            }
            Got::NotThere(_) | Got::Failed => return gone,
            Got::Refused(_) | Got::Skipped => return None,
        },
        FactsStep::Read(read) => match answers.get(*read) {
            Got::Done(read) => match read
                .values
                .answer(PropertyId::Role)
                .and_then(|answer| answer.value.as_ref())
            {
                Some(PropertyValue::Role(role)) => (None, Some(*role)),
                _ => (None, None),
            },
            Got::NotThere(_) | Got::Failed => return gone,
            Got::Refused(_) | Got::Skipped => return None,
        },
        FactsStep::None => (None, None),
    };
    let facts = CandidateFacts::Found { focused, role };
    let Some(read) = steps.read else {
        return Some(Found::Facts(facts));
    };
    match answers.get(read) {
        Got::Done(read) => Some(Found::Read(facts, Box::new(read.clone()))),
        Got::NotThere(_) | Got::Failed => gone,
        Got::Refused(_) | Got::Skipped => None,
    }
}

/// Asks `candidate`'s batch to `reach` of its outpost, naming its window,
/// with the hidden-dialog check when `hidden` and `spare` kept held while
/// it is out: what it found as `narrow` takes it, `None` from it being a
/// malformed answer; `skipped` for a candidate whose window is read
/// through UI Automation; or the outcome that ended it.
async fn ask<T: Debug + 'static>(
    cx: &mut Cx<Entries>,
    (candidate, reach, hidden): (&Candidate, Reach, bool),
    (spare, known, due_us): (Option<(NodeId, TextAnchor)>, &Known, u64),
    (narrow, skipped): (fn(Found) -> Option<T>, T),
) -> Outcome<T> {
    let mut batch = cx.batch_numbered(|| ());
    if let Some((node, anchor)) = spare {
        batch.spare(node, anchor);
    }
    let steps = build(cx, &mut batch, (candidate, &reach, hidden), known);
    let request = batch
        .finish()
        .of(Reader::Outpost(candidate.tell.process), candidate.window)
        .judged(move |answers| judge(answers, &steps, &reach).and_then(narrow));
    match cx.ask(request, due_us).await {
        // Read through UI Automation, whose own focus speaks for it, as
        // NVDA drops a native UI Automation window's `WinEvent`.
        Outcome::Refused(Refused::Reader(Refusal::ReadThroughUia)) => Outcome::Result(skipped),
        outcome => outcome,
    }
}

/// Reads the waiting `candidate`'s facts alone, for a caret key's
/// confirmation, `spare` held while it is out: its facts, or the outcome
/// that ended the read.
pub(crate) async fn facts(
    cx: &mut Cx<Entries>,
    candidate: &Candidate,
    spare: Option<(NodeId, TextAnchor)>,
    due_us: u64,
) -> Outcome<CandidateFacts> {
    let known = known_for(cx, candidate.tell.process);
    let narrow = |found| match found {
        Found::Facts(facts) => Some(facts),
        Found::Unread(Unread::NotThere) => Some(CandidateFacts::NotThere),
        _ => None,
    };
    ask(
        cx,
        (candidate, Reach::Facts, false),
        (spare, &known, due_us),
        (narrow, CandidateFacts::Skipped),
    )
    .await
}

/// Whether `question` reads a candidate's facts alone, a caret key's
/// confirmation: a batch on an MSAA object with no full read.
pub(crate) fn is_confirmation(question: &Question) -> bool {
    let Question::Batch(batch) = question;
    matches!(
        batch.steps.first().and_then(|step| step.op.subject()),
        Some(On_::Msaa { .. })
    ) && !batch
        .steps
        .iter()
        .any(|step| matches!(step.op, Op::FocusRead { .. }))
}

/// The ancestors Core knows, as a focus request to `process`'s outpost
/// sends them.
pub(crate) fn known_for(cx: &Cx<Entries>, process: Pid) -> Known {
    let generation = cx
        .instance(Reader::Outpost(process))
        .map(verbatim_model::request::Instance::outpost_id);
    Known::of(&cx.facts(), generation)
}

/// A read's answer as its depth's rule takes it, `None` for another.
fn answered(found: Found) -> Option<Answered> {
    match found {
        Found::Unread(unread) => Some(Answered::Unread(unread)),
        Found::Read(facts, read) => Some(Answered::Read { facts, read }),
        Found::HiddenDialog(_) | Found::Same | Found::Facts(_) => None,
    }
}

/// Reads `candidate` at its turn, to `depth`: its answer, with the
/// ancestors Core knew as the read was sent, or `None` for the outcome
/// that ended it ([`answer_of`]) or a hidden dialog it postponed
/// (`super::shown`).
pub(crate) async fn read_in_full(
    cx: &mut Cx<Entries>,
    candidate: &Candidate,
    depth: TurnDepth,
    due_us: u64,
) -> Option<(TurnAnswer, Known)> {
    let known = known_for(cx, candidate.tell.process);
    let narrow = |found| match found {
        Found::HiddenDialog(window) => Some(Hidden::Dialog(window)),
        Found::Same => Some(Hidden::Read(TurnAnswer::SameAsNamed)),
        found => answered(found).map(|answered| Hidden::Read(TurnAnswer::Answered(answered))),
    };
    let skipped = Hidden::Read(TurnAnswer::Answered(Answered::Unread(Unread::Skipped)));
    let outcome = ask(
        cx,
        (candidate, Reach::Turn(depth), true),
        (None, &known, due_us),
        (narrow, skipped),
    )
    .await;
    let outcome = shown::postpone(cx, outcome)?;
    answer_of(cx, outcome).map(|answer| (answer, known))
}

/// Reads the menu start `candidate` in full, with no check for a hidden
/// dialog, since its popup is its own top-level window: its answer, or
/// `None` for the outcome that ended it ([`answer_of`]).
pub(crate) async fn read_full(
    cx: &mut Cx<Entries>,
    candidate: &Candidate,
    (known, due_us): (&Known, u64),
) -> Option<Answered> {
    let outcome = ask(
        cx,
        (candidate, Reach::Turn(TurnDepth::Full), false),
        (None, known, due_us),
        (answered, Answered::Unread(Unread::Skipped)),
    )
    .await;
    answer_of(cx, outcome)
}

/// The result of a candidate's read, or `None` for an outcome that ended
/// it, judged by [`validity::failed_read`].
fn answer_of<T>(cx: &mut Cx<Entries>, outcome: Outcome<T>) -> Option<T> {
    match outcome {
        Outcome::Result(answer) => Some(answer),
        outcome => {
            validity::failed_read(&mut cx.facts_mut(), &outcome);
            None
        }
    }
}

/// The function of a run entry, its candidates oldest first. An outcome
/// other than a result ends the run: after an abandoned try the older
/// candidates are not tried (a limit).
pub(crate) async fn run(mut cx: Cx<Entries>, candidates: Vec<Candidate>) {
    walk(&mut cx, candidates).await;
}

/// The run's walk, newest first, until a candidate executes or none is
/// left.
async fn walk(cx: &mut Cx<Entries>, candidates: Vec<Candidate>) {
    let due_us = cx.due_in(FOCUS_DUE_US);
    let newest_first: Vec<Candidate> = candidates.into_iter().rev().collect();
    let mut rest = newest_first.as_slice();
    loop {
        let walk = validity::with_view(cx, false, |view| validity::judge_run(rest, view));
        for reason in walk.refused {
            validity::refused(&mut cx.facts_mut(), reason);
        }
        let Some(found) = walk.found else {
            return;
        };
        let candidate = found.candidate;
        if found.answer == Validity::Duplicate {
            // A duplicate ends the run as its valid focus, which the menu
            // slot's entry after it judges.
            cx.facts_mut().run_executed = true;
            return;
        }
        let depth = validity::with_view(cx, false, |view| {
            let facts = cx.facts();
            let committed = facts.versions.focus().map(|focus| &focus.context.snapshot);
            validity::depth(Subject::Msaa(candidate), view, committed)
        });
        let Some((answer, known)) = read_in_full(cx, candidate, depth, due_us).await else {
            return;
        };
        let (facts, read) = match answer {
            TurnAnswer::SameAsNamed => {
                match validity::with_view(cx, false, |view| {
                    validity::before_duplicate(candidate, view)
                }) {
                    // The object named, which a duplicate ends the run as.
                    None => cx.facts_mut().run_executed = true,
                    Some(reason) => {
                        validity::refused(&mut cx.facts_mut(), reason);
                        rest = found.older;
                        continue;
                    }
                }
                return;
            }
            TurnAnswer::Answered(Answered::Unread(unread)) => {
                validity::refused(&mut cx.facts_mut(), unread.reason());
                rest = found.older;
                continue;
            }
            TurnAnswer::Answered(Answered::Read { facts, read }) => (facts, read),
        };
        match validity::read_at(cx, Subject::Msaa(candidate), &facts, false) {
            Settled::Refused(reason) => {
                validity::refused(&mut cx.facts_mut(), reason);
                rest = found.older;
            }
            Settled::Duplicate => {
                cx.facts_mut().run_executed = true;
                return;
            }
            Settled::Valid(_) => {
                if executed(cx, candidate, &read, &known) {
                    cx.facts_mut().run_executed = true;
                }
                return;
            }
        }
    }
}

/// Executes the valid focus `read` of `candidate`, for the candidate's own
/// speech fence, and says whether it did. An MSAA answer without the ids
/// it was read by is the outpost's fault, and ends unspoken.
pub(crate) fn executed(
    cx: &mut Cx<Entries>,
    candidate: &Candidate,
    read: &FocusRead,
    known: &Known,
) -> bool {
    let Some(held) = read.msaa else {
        return false;
    };
    let read_by = ReadBy::Msaa {
        event: Some(held.event),
        held_child: held.held_child,
    };
    let origin = Origin::of_tell(&candidate.tell, candidate.window, candidate.fence);
    let executed = execute(&mut cx.facts_mut(), &origin, read, known, &read_by);
    let fence = crate::entries::front_fence(cx);
    executed.deliver(cx, fence);
    true
}
