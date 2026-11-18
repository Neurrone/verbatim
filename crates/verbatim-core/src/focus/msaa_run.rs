//! A run of MSAA focus and foreground candidates at the front (design,
//! "Focus and foreground"): walked newest first, strictly, across
//! applications, as NVDA's pump has no grouping by application
//! (`IAccessibleHandler/__init__.py` 1087 to 1092). Each candidate is
//! judged by the one definition (`super::validity`) on the facts Core holds
//! and any a caret key's confirmation stored on it: a refusal goes on with
//! no request, a duplicate ends the run as its valid focus, and any other
//! answer asks that candidate's outpost to its depth, naming the
//! candidate's window, then judges the answer the same way. The valid focus
//! is executed. One request per candidate: Core decides every candidate,
//! the outpost only reporting facts (Dickson, 2026-11-18).

use verbatim_model::request::{
    CandidateAnswer, CandidateFacts, CandidateKind, Depth, FocusRead, MsaaRun, Reply,
    Request as Wire, RunCandidate,
};
use verbatim_model::{OutpostId, Pid};

use super::FOCUS_SET;
use super::execute::{Known, Origin, execute};
use super::validity::{self, Purpose, Subject, Validity};
use super::versions::ReadBy;
use crate::entries::{Candidate, Entries};
use crate::event_queue::{Cx, FOCUS_DUE_US, Outcome, Reader, Request, RequestWindow};

/// One MSAA candidate asked of its application's outpost.
#[derive(Debug)]
pub(crate) struct RunRequest {
    /// The candidate's window.
    pub(crate) window: RequestWindow,
    /// The application.
    pub(crate) process: Pid,
    /// The candidate and the read's depth.
    pub(crate) run: MsaaRun,
}

impl Request for RunRequest {
    type Reply = CandidateAnswer;

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
            Reply::Candidate(answer) => Some(answer),
            _ => None,
        }
    }
}

impl RunRequest {
    /// The request for `candidate` at `depth`, its outpost knowing the
    /// ancestors `known` sends, with `spare` kept held while it is out.
    pub(crate) fn of(
        cx: &Cx<Entries>,
        candidate: &Candidate,
        (depth, spare): (
            Depth,
            Option<(verbatim_model::NodeId, verbatim_model::TextAnchor)>,
        ),
        known: &Known,
    ) -> Self {
        let verdict = cx
            .backend()
            .for_request(&candidate.window.judged(), cx.now_us());
        let kind = match candidate.tell.kind {
            verbatim_model::event_kind::EventKind::Foreground => CandidateKind::Foreground,
            verbatim_model::event_kind::EventKind::MenuStart
            | verbatim_model::event_kind::EventKind::PopupStart => CandidateKind::MenuStart,
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
                depth,
                spare,
                set: crate::details::node_set(&cx.facts(), FOCUS_SET),
                known: known.sent.clone(),
                details: crate::details::focus_details(&cx.facts()),
            },
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

/// What one candidate's turn came to.
enum Turn {
    /// Refused: the run goes on.
    Refused,
    /// The run ends: a duplicate, a valid focus executed, or a read that
    /// failed.
    Ended,
}

/// The function of a run entry, its candidates oldest first. An outcome
/// other than a result ends the run: after an abandoned try the older
/// candidates are not tried (a limit).
pub(crate) async fn run(mut cx: Cx<Entries>, candidates: Vec<Candidate>) {
    let due_us = cx.due_in(FOCUS_DUE_US);
    for candidate in candidates.iter().rev() {
        match turn(&mut cx, candidate, due_us).await {
            Turn::Refused => {}
            Turn::Ended => return,
        }
    }
}

/// One candidate's turn.
async fn turn(cx: &mut Cx<Entries>, candidate: &Candidate, due_us: u64) -> Turn {
    let stored = candidate.confirmation.facts();
    match judged(cx, candidate, stored) {
        Validity::Refused(reason) => {
            if reason.tried() {
                cx.facts_mut().versions.tried_and_refused();
            }
            return Turn::Refused;
        }
        Validity::Duplicate => {
            // A duplicate or the redirect's child ends the run as its valid
            // focus, which the menu slot's entry after it judges.
            cx.facts_mut().run_executed = true;
            return Turn::Ended;
        }
        Validity::Valid(_) | Validity::NeedsRead(_) => {}
    }
    // A candidate a confirmation found focused is read in full, its new
    // facts replacing the stored ones.
    let depth = if stored.is_some() {
        Depth::Full
    } else {
        validity::depth(Subject::Msaa(candidate))
    };
    let known = known_for(cx, candidate.tell.process);
    let request = RunRequest::of(cx, candidate, (depth, None), &known);
    let Outcome::Result(answered) = cx.ask(request, due_us).await else {
        cx.facts_mut().focus_read_failed();
        return Turn::Ended;
    };
    match judged(cx, candidate, Some(&answered.facts)) {
        Validity::Refused(reason) => {
            if reason.tried() {
                cx.facts_mut().versions.tried_and_refused();
            }
            Turn::Refused
        }
        Validity::Duplicate => {
            cx.facts_mut().run_executed = true;
            Turn::Ended
        }
        // Facts answered leave nothing to read: the outpost's fault.
        Validity::NeedsRead(_) => Turn::Ended,
        Validity::Valid(_) => {
            // A valid answer with no full read: the outpost's fault, and
            // the run ends unspoken.
            let Some(read) = answered.read else {
                return Turn::Ended;
            };
            executed(cx, candidate, (&read, &answered.facts), &known);
            Turn::Ended
        }
    }
}

/// `candidate` judged at its turn with `facts`.
fn judged(cx: &Cx<Entries>, candidate: &Candidate, facts: Option<&CandidateFacts>) -> Validity {
    validity::at(cx, Subject::Msaa(candidate), facts, Purpose::Turn, false)
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
