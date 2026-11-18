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
    CandidateFacts, CandidateKind, Depth, FocusRead, MsaaRun, Reply, Request as Wire, RunCandidate,
};
use verbatim_model::{NodeId, OutpostId, Pid, TextAnchor};

use super::FOCUS_SET;
use super::execute::{Known, Origin, execute};
use super::validity::{self, Answered, Purpose, Reason, Settled, Subject, Validity};
use super::versions::ReadBy;
use crate::entries::{Candidate, Entries};
use crate::event_queue::{Cx, FOCUS_DUE_US, Outcome, Reader, Request, RequestWindow};

/// A depth a candidate is read to, as a request's type, with how its answer
/// is parsed at its decode.
pub(crate) trait ReadDepth: std::fmt::Debug + Unpin + 'static {
    /// The depth.
    const DEPTH: Depth;
    /// The answer, parsed.
    type Parsed: std::fmt::Debug + 'static;
    /// `answer` parsed, `None` for one that breaks the depth's rule.
    fn parse(answer: verbatim_model::request::CandidateAnswer) -> Option<Self::Parsed>;
}

/// The facts alone: a caret key's confirmation.
#[derive(Debug)]
pub(crate) struct FactsOnly;

/// The facts, and the full read when focused: a focus's turn.
#[derive(Debug)]
pub(crate) struct FullIfFocused;

/// The facts and the full read: a foreground's, a menu start's, or a
/// candidate a confirmation found focused.
#[derive(Debug)]
pub(crate) struct Full;

impl ReadDepth for FactsOnly {
    const DEPTH: Depth = Depth::Facts;
    type Parsed = CandidateFacts;
    fn parse(answer: verbatim_model::request::CandidateAnswer) -> Option<CandidateFacts> {
        validity::parse_facts(answer)
    }
}

impl ReadDepth for FullIfFocused {
    const DEPTH: Depth = Depth::FullIfFocused;
    type Parsed = Answered;
    fn parse(answer: verbatim_model::request::CandidateAnswer) -> Option<Answered> {
        Answered::parse(answer, Self::DEPTH)
    }
}

impl ReadDepth for Full {
    const DEPTH: Depth = Depth::Full;
    type Parsed = Answered;
    fn parse(answer: verbatim_model::request::CandidateAnswer) -> Option<Answered> {
        Answered::parse(answer, Self::DEPTH)
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
        match reply {
            Reply::Candidate(answer) => D::parse(answer),
            _ => None,
        }
    }
}

impl<D: ReadDepth> RunRequest<D> {
    /// The request for `candidate`, its outpost knowing the ancestors
    /// `known` sends, with `spare` kept held while it is out.
    pub(crate) fn of(
        cx: &Cx<Entries>,
        candidate: &Candidate,
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
                candidate: RunCandidate {
                    ids: candidate.ids,
                    kind,
                    verdict,
                },
                depth: D::DEPTH,
                spare,
                set: crate::details::node_set(&cx.facts(), FOCUS_SET),
                known: known.sent.clone(),
                details: crate::details::focus_details(&cx.facts()),
            },
            depth: PhantomData,
        }
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

/// Reads `candidate` in full at its turn, to `depth`: its answer parsed,
/// or the outcome that ended it.
pub(crate) async fn read_in_full(
    cx: &mut Cx<Entries>,
    candidate: &Candidate,
    depth: Depth,
    (known, due_us): (&Known, u64),
) -> Option<Answered> {
    let outcome = if depth == Depth::FullIfFocused {
        let request = RunRequest::<FullIfFocused>::of(cx, candidate, None, known);
        cx.ask(request, due_us).await
    } else {
        let request = RunRequest::<Full>::of(cx, candidate, None, known);
        cx.ask(request, due_us).await
    };
    match outcome {
        Outcome::Result(answered) => Some(answered),
        Outcome::Refused(_)
        | Outcome::NotThere
        | Outcome::NotResponding
        | Outcome::ReaderEnded
        | Outcome::NoLongerAwaited(_) => {
            cx.facts_mut().focus_read_failed();
            None
        }
    }
}

/// A refusal at a candidate's turn: one that came from a read of it ends
/// the redirect guard.
fn refused(cx: &mut Cx<Entries>, reason: Reason) {
    if reason.tried() {
        cx.facts_mut().versions.tried_and_refused();
    }
}

/// The function of a run entry, its candidates oldest first. An outcome
/// other than a result ends the run: after an abandoned try the older
/// candidates are not tried (a limit).
pub(crate) async fn run(mut cx: Cx<Entries>, candidates: Vec<Candidate>) {
    let due_us = cx.due_in(FOCUS_DUE_US);
    let newest_first: Vec<Candidate> = candidates.into_iter().rev().collect();
    let mut rest = newest_first.as_slice();
    loop {
        let walk = validity::with_view(&cx, false, |view| {
            validity::judge_run(rest, view, Purpose::Turn)
        });
        for reason in walk.refused {
            refused(&mut cx, reason);
        }
        let Some(found) = walk.found else {
            return;
        };
        let candidate = found.candidate;
        let depth = match found.answer {
            // A duplicate or the redirect's child ends the run as its valid
            // focus, which the menu slot's entry after it judges.
            Validity::Duplicate => {
                cx.facts_mut().run_executed = true;
                return;
            }
            // A candidate a confirmation found focused is read in full, its
            // new facts replacing the stored ones.
            Validity::Valid(_) if candidate.confirmation.facts().is_some() => Depth::Full,
            Validity::Valid(_) | Validity::NeedsRead(_) | Validity::Refused(_) => {
                validity::depth(Subject::Msaa(candidate))
            }
        };
        let known = known_for(&cx, candidate.tell.process);
        let Some(answered) = read_in_full(&mut cx, candidate, depth, (&known, due_us)).await else {
            return;
        };
        let (facts, read) = match answered {
            Answered::Unread(unread) => {
                refused(&mut cx, unread.reason());
                rest = found.older;
                continue;
            }
            Answered::Read { facts, read } => (facts, read),
        };
        match validity::read_at(&cx, Subject::Msaa(candidate), &facts, Purpose::Turn, false) {
            Settled::Refused(reason) => {
                refused(&mut cx, reason);
                rest = found.older;
            }
            Settled::Duplicate => {
                cx.facts_mut().run_executed = true;
                return;
            }
            Settled::Valid(_) => {
                executed(&mut cx, candidate, (&read, &facts), &known);
                return;
            }
        }
    }
}

/// Executes the valid focus `read` of `candidate`, whose facts name the
/// redirect that reached it, if any: the readers' last focus then carries
/// the guard with the candidate's fence. An MSAA answer without the ids it
/// was read by is the outpost's fault, and ends unspoken.
fn executed(
    cx: &mut Cx<Entries>,
    candidate: &Candidate,
    (read, facts): (&FocusRead, &CandidateFacts),
    known: &Known,
) {
    let Some(held) = read.msaa else {
        return;
    };
    let read_by = ReadBy::Msaa {
        event: Some(held.event),
        held_child: held.held_child,
    };
    let origin = Origin::of_tell(&candidate.tell, candidate.window, cx.foreground());
    let executed = {
        let mut facts_mut = cx.facts_mut();
        let executed = execute(&mut facts_mut, &origin, read, known, read_by);
        if executed.committed
            && let CandidateFacts::Found {
                redirect: Some((redirect, _)),
                ..
            } = facts
        {
            facts_mut.versions.guard(*redirect, candidate.fence);
        }
        facts_mut.run_executed = true;
        executed
    };
    executed.deliver(cx);
}
