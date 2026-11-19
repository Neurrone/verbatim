//! A run of MSAA focus and foreground candidates at the front (design,
//! "Focus and foreground"): walked by the one definition's run walk
//! (`super::validity::judge_run`), newest first, strictly, across
//! applications, as NVDA's pump has no grouping by application
//! (`IAccessibleHandler/__init__.py` 1087 to 1092). Refusals go on with no
//! request, a duplicate ends the run as its valid focus, and any other
//! answer asks that candidate's outpost to its depth, naming the
//! candidate's window; the answer, parsed at its decode, is judged the
//! same way, and a candidate refused after its read is passed over by the
//! walk's next step. The valid focus is executed. One request per
//! candidate: Core decides every candidate, the outpost only reporting
//! facts (Dickson, 2026-11-18).

use std::marker::PhantomData;

use verbatim_model::event_kind::EventKind;
use verbatim_model::request::{
    CandidateAnswer, CandidateFacts, CandidateKind, Depth, FocusRead, HiddenRoot, Identity,
    MsaaIds, MsaaRun, Reply, Request as Wire, RunCandidate,
};
use verbatim_model::{NodeId, OutpostId, Pid, TextAnchor};

use super::FOCUS_SET;
use super::execute::{Known, Origin, execute};
use super::shown::{self, Hidden};
use super::validity::{self, Answered, Reason, Settled, Subject, Validity};
use super::versions::ReadBy;
use crate::entries::{Candidate, Entries};
use crate::event_queue::{Cx, FOCUS_DUE_US, Outcome, Reader, Request, RequestWindow};

/// A depth a candidate is read to, as a request's type, with how its answer
/// is parsed at its decode.
pub(crate) trait ReadDepth: std::fmt::Debug + Unpin + 'static {
    /// The answer, parsed.
    type Parsed: std::fmt::Debug + 'static;
    /// The depth, as it is sent.
    fn depth(&self) -> Depth;
    /// Whether the outpost checks for a hidden dialog before the read
    /// (`super::shown`): a read Core waits on a show for answers it.
    const HIDDEN: HiddenRoot;
    /// `reply` parsed, `None` for one that breaks the depth's rule.
    fn parse(reply: Reply) -> Option<Self::Parsed>;
}

/// The facts alone: a caret key's confirmation.
#[derive(Debug)]
pub(crate) struct FactsOnly;

/// The facts, and the full read when focused: a focus's turn.
#[derive(Debug)]
pub(crate) struct FullIfFocused;

/// The facts and the full read: a foreground's.
#[derive(Debug)]
pub(crate) struct Full;

/// The facts and the full read, with no check for a hidden dialog: a menu
/// start's, whose popup is its own top-level window.
#[derive(Debug)]
pub(crate) struct MenuFull;

/// As [`FullIfFocused`], unless the object reached is at the readers' last
/// focus's ids, `ids`, and reads as it does, `identity`: the identity check
/// in the duplicate test (`docs/parity/focus-events.md`, "Duplicate focus
/// suppression"). Made only by [`validity::depth`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnlessSame {
    /// The ids of the readers' last focus.
    pub(crate) ids: MsaaIds,
    /// What the readers' last focus reads as.
    pub(crate) identity: Identity,
}

/// An answer to [`UnlessSame`], parsed at its decode.
#[derive(Debug)]
pub(crate) enum UnlessSameAnswer {
    /// Not the object named, or not focused: answered as to
    /// [`FullIfFocused`].
    Answered(Answered),
    /// At the ids named, focused, and reading as the identity named.
    Same,
}

/// A candidate's answer at its turn, parsed: read to its depth's rule, or,
/// at [`UnlessSame`], the object the request named.
#[derive(Debug)]
pub(crate) enum TurnAnswer {
    /// Read to its depth's rule.
    Answered(Answered),
    /// The object at the ids and with the identity the request named.
    SameAsNamed,
}

/// The candidate answer `reply` is, if it is one.
fn candidate_answer(reply: Reply) -> Option<CandidateAnswer> {
    match reply {
        Reply::Candidate(answer) => Some(answer),
        _ => None,
    }
}

impl ReadDepth for FactsOnly {
    type Parsed = CandidateFacts;
    const HIDDEN: HiddenRoot = HiddenRoot::ReadAnyway;
    fn depth(&self) -> Depth {
        Depth::Facts
    }
    fn parse(reply: Reply) -> Option<CandidateFacts> {
        validity::parse_facts(candidate_answer(reply)?)
    }
}

impl ReadDepth for FullIfFocused {
    type Parsed = Hidden<Answered>;
    const HIDDEN: HiddenRoot = HiddenRoot::Check;
    fn depth(&self) -> Depth {
        Depth::FullIfFocused
    }
    fn parse(reply: Reply) -> Option<Hidden<Answered>> {
        Hidden::parse(reply, |reply| {
            Answered::parse(candidate_answer(reply)?, &Depth::FullIfFocused)
        })
    }
}

impl ReadDepth for Full {
    type Parsed = Hidden<Answered>;
    const HIDDEN: HiddenRoot = HiddenRoot::Check;
    fn depth(&self) -> Depth {
        Depth::Full
    }
    fn parse(reply: Reply) -> Option<Hidden<Answered>> {
        Hidden::parse(reply, |reply| {
            Answered::parse(candidate_answer(reply)?, &Depth::Full)
        })
    }
}

impl ReadDepth for MenuFull {
    type Parsed = Answered;
    const HIDDEN: HiddenRoot = HiddenRoot::ReadAnyway;
    fn depth(&self) -> Depth {
        Depth::Full
    }
    fn parse(reply: Reply) -> Option<Answered> {
        Answered::parse(candidate_answer(reply)?, &Depth::Full)
    }
}

impl ReadDepth for UnlessSame {
    type Parsed = Hidden<UnlessSameAnswer>;
    const HIDDEN: HiddenRoot = HiddenRoot::Check;
    fn depth(&self) -> Depth {
        Depth::FullUnlessSame {
            ids: self.ids,
            identity: Box::new(self.identity.clone()),
        }
    }
    fn parse(reply: Reply) -> Option<Hidden<UnlessSameAnswer>> {
        Hidden::parse(reply, |reply| match reply {
            Reply::CandidateSame => Some(UnlessSameAnswer::Same),
            reply => Answered::parse(candidate_answer(reply)?, &Depth::FullIfFocused)
                .map(UnlessSameAnswer::Answered),
        })
    }
}

/// One MSAA candidate asked of its application's outpost, read to `D`.
#[derive(Debug)]
pub(crate) struct RunRequest<D: ReadDepth> {
    /// The candidate's window.
    pub(crate) window: RequestWindow,
    /// The application.
    pub(crate) process: Pid,
    /// The candidate and the read's depth.
    pub(crate) run: MsaaRun,
    depth: PhantomData<D>,
}

impl<D: ReadDepth> Request for RunRequest<D> {
    type Reply = D::Parsed;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::MsaaRun(self.run.clone())
    }

    fn decode(reply: Reply) -> Option<Self::Reply> {
        D::parse(reply)
    }
}

impl<D: ReadDepth> RunRequest<D> {
    /// The request for `candidate` read to `depth`, its outpost knowing the
    /// ancestors `known` sends, with `spare` kept held while it is out.
    pub(crate) fn of(
        cx: &Cx<Entries>,
        (candidate, depth): (&Candidate, &D),
        spare: Option<(NodeId, TextAnchor)>,
        known: &Known,
    ) -> Self {
        let verdict = cx
            .backend()
            .for_request(&candidate.window.judged(), cx.now_us());
        let kind = match candidate.tell.kind {
            EventKind::Foreground => CandidateKind::Foreground,
            EventKind::MenuStart | EventKind::PopupStart => CandidateKind::MenuStart,
            _ => CandidateKind::Focus,
        };
        Self {
            window: candidate.window,
            process: candidate.tell.process,
            run: MsaaRun {
                hidden: D::HIDDEN,
                candidate: RunCandidate {
                    ids: candidate.ids,
                    kind,
                    verdict,
                },
                depth: depth.depth(),
                spare,
                set: crate::details::focus_set(&cx.facts(), FOCUS_SET),
                known: known.sent.clone(),
                details: crate::details::focus_details(&cx.facts()),
            },
            depth: PhantomData,
        }
    }

    /// The request reading as it is, hidden or not, when `hidden` says so:
    /// a key ended the wait for its dialog's show.
    pub(crate) fn hidden(mut self, hidden: HiddenRoot) -> Self {
        if hidden == HiddenRoot::ReadAnyway {
            self.run.hidden = hidden;
        }
        self
    }
}

/// The ancestors Core knows, as a focus request to `process`'s outpost
/// sends them.
pub(crate) fn known_for(cx: &Cx<Entries>, process: Pid) -> Known {
    let generation = cx
        .instance(Reader::Outpost(process))
        .map(|instance| OutpostId(instance.generation));
    Known::of(&cx.facts(), generation)
}

/// How far a candidate is read at its turn ([`validity::depth`]).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TurnDepth {
    /// [`FullIfFocused`].
    FullIfFocused,
    /// [`Full`].
    Full,
    /// [`UnlessSame`].
    UnlessSame(UnlessSame),
}

/// Reads `candidate` at its turn, to `depth`, read again once its hidden
/// dialog is shown (`super::shown`): its answer parsed, with the ancestors
/// Core knew as its last read was sent, or `None` for the outcome that
/// ended it ([`answer_of`]).
pub(crate) async fn read_in_full(
    cx: &mut Cx<Entries>,
    candidate: &Candidate,
    depth: TurnDepth,
    due_us: u64,
) -> Option<(TurnAnswer, Known)> {
    let process = candidate.tell.process;
    let outcome = match depth {
        TurnDepth::FullIfFocused => shown::read_shown(cx, due_us, |cx, hidden| {
            let known = known_for(cx, process);
            let request =
                RunRequest::of(cx, (candidate, &FullIfFocused), None, &known).hidden(hidden);
            (request, known)
        })
        .await
        .map(|(answered, known)| (TurnAnswer::Answered(answered), known)),
        TurnDepth::Full => shown::read_shown(cx, due_us, |cx, hidden| {
            let known = known_for(cx, process);
            let request = RunRequest::of(cx, (candidate, &Full), None, &known).hidden(hidden);
            (request, known)
        })
        .await
        .map(|(answered, known)| (TurnAnswer::Answered(answered), known)),
        TurnDepth::UnlessSame(named) => shown::read_shown(cx, due_us, |cx, hidden| {
            let known = known_for(cx, process);
            let request = RunRequest::of(cx, (candidate, &named), None, &known).hidden(hidden);
            (request, known)
        })
        .await
        .map(|(answer, known)| {
            let answer = match answer {
                UnlessSameAnswer::Answered(answered) => TurnAnswer::Answered(answered),
                UnlessSameAnswer::Same => TurnAnswer::SameAsNamed,
            };
            (answer, known)
        }),
    };
    answer_of(cx, outcome)
}

/// Reads the menu start `candidate` in full: its answer parsed, or `None`
/// for the outcome that ended it ([`answer_of`]).
pub(crate) async fn read_full(
    cx: &mut Cx<Entries>,
    candidate: &Candidate,
    (known, due_us): (&Known, u64),
) -> Option<Answered> {
    let request = RunRequest::of(cx, (candidate, &MenuFull), None, known);
    let outcome = cx.ask(request, due_us).await;
    answer_of(cx, outcome)
}

/// The result of a candidate's read, or `None` for an outcome that ended
/// it, which clears Core's caret baseline. A read answered not there, or
/// whose reader ended, tried the focus and found it gone: the readers'
/// last focus's fence ends ([`super::versions::Versions::tried_and_refused`]).
fn answer_of<T>(cx: &mut Cx<Entries>, outcome: Outcome<T>) -> Option<T> {
    let tried = match outcome {
        Outcome::Result(answer) => return Some(answer),
        Outcome::NotThere | Outcome::ReaderEnded => true,
        Outcome::Refused(_) | Outcome::NotResponding | Outcome::NoLongerAwaited(_) => false,
    };
    let mut facts = cx.facts_mut();
    if tried {
        facts.versions.tried_and_refused();
    }
    facts.focus_read_failed();
    None
}

/// A refusal at a candidate's turn: one that came from a read of it ends
/// the readers' last focus's fence.
fn refused(cx: &mut Cx<Entries>, reason: Reason) {
    if reason.tried() {
        cx.facts_mut().versions.tried_and_refused();
    }
}

/// The function of a run entry, its candidates oldest first. An outcome
/// other than a result ends the run: after an abandoned try the older
/// candidates are not tried (a limit).
pub(crate) async fn run(mut cx: Cx<Entries>, candidates: Vec<Candidate>) {
    let ended_wait = candidates.iter().find_map(|candidate| candidate.ended_wait);
    walk(&mut cx, candidates).await;
    if let Some(dialog) = ended_wait {
        let executed = cx.facts().run_executed;
        shown::after_ended_wait(&mut cx, dialog, executed);
    }
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
            refused(cx, reason);
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
            let committed = facts.focus.object().map(|focus| &focus.snapshot);
            validity::depth(Subject::Msaa(candidate), view, committed)
        });
        let Some((answer, known)) = read_in_full(cx, candidate, depth, due_us).await else {
            return;
        };
        let (facts, read) = match answer {
            TurnAnswer::SameAsNamed => {
                match validity::with_view(cx, false, |view| validity::judge_same(candidate, view)) {
                    // The object named, which a duplicate ends the run as.
                    None => cx.facts_mut().run_executed = true,
                    Some(reason) => {
                        refused(cx, reason);
                        rest = found.older;
                        continue;
                    }
                }
                return;
            }
            TurnAnswer::Answered(Answered::Unread(unread)) => {
                refused(cx, unread.reason());
                rest = found.older;
                continue;
            }
            TurnAnswer::Answered(Answered::Read { facts, read }) => (facts, read),
        };
        match validity::read_at(cx, Subject::Msaa(candidate), &facts, false) {
            Settled::Refused(reason) => {
                refused(cx, reason);
                rest = found.older;
            }
            Settled::Duplicate => {
                cx.facts_mut().run_executed = true;
                return;
            }
            Settled::Valid(_) => {
                executed(cx, candidate, &read, &known);
                return;
            }
        }
    }
}

/// Executes the valid focus `read` of `candidate`, for the candidate's own
/// speech fence. An MSAA answer without the ids it was read by is the
/// outpost's fault, and ends unspoken.
fn executed(cx: &mut Cx<Entries>, candidate: &Candidate, read: &FocusRead, known: &Known) {
    let Some(held) = read.msaa else {
        return;
    };
    let read_by = ReadBy::Msaa {
        event: Some(held.event),
        held_child: held.held_child,
    };
    let origin = Origin::of_tell(
        &candidate.tell,
        candidate.window,
        cx.foreground(),
        candidate.fence,
    );
    let executed = {
        let mut facts_mut = cx.facts_mut();
        let executed = execute(&mut facts_mut, &origin, read, known, read_by);
        facts_mut.run_executed = true;
        executed
    };
    executed.deliver(cx);
}
