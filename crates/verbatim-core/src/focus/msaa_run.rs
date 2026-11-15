//! A run of MSAA focus and foreground candidates at the front (design,
//! "Focus and foreground"): tried newest first. Each candidate is judged
//! with no read on its tell's facts, acceptance and the verdict, then by
//! its ids against the readers' last focus: a refusal drops it, and an
//! MSAA duplicate ends the run as its valid focus. The rest are tried by
//! application, the newest candidate's first: Core asks that
//! application's outpost with its candidates, newest first, and when every
//! one is refused or skipped, the next application's, a follow-up under
//! the same due time. The valid focus an outpost answers is executed.

use verbatim_model::request::{
    CandidateKind, FocusRead, MsaaIds, MsaaRun, Reply, Request as Wire, RunCandidate, RunRead,
    Tried,
};
use verbatim_model::{OutpostId, Pid};

use super::FOCUS_SET;
use super::acceptance;
use super::backend::{Judged, Judgement};
use super::duplicate::{RunJudgement, judge_candidate};
use super::execute::{Known, Origin, execute};
use super::versions::ReadBy;
use crate::entries::{Candidate, Entries};
use crate::event_queue::{Cx, FOCUS_DUE_US, Outcome, Reader, Request, RequestWindow};

/// One application's candidates of a run, asked of its outpost.
#[derive(Debug)]
pub(crate) struct RunRequest {
    /// The window of the newest candidate.
    pub(crate) window: RequestWindow,
    /// The application.
    pub(crate) process: Pid,
    /// The candidates.
    pub(crate) run: MsaaRun,
}

impl Request for RunRequest {
    type Reply = RunRead;

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
            Reply::Run(read) => Some(read),
            _ => None,
        }
    }
}

/// The candidates `candidates`, oldest first, that are tried, newest
/// first: those that pass acceptance and the verdict and are not refused
/// by ids, up to an MSAA duplicate of the readers' last focus or the child a
/// redirect reached, which ends the run as its valid focus; and whether
/// one did.
fn to_try(cx: &Cx<Entries>, candidates: Vec<Candidate>) -> (Vec<Candidate>, bool) {
    let facts = cx.facts();
    let backend = cx.backend();
    let now_us = cx.now_us();
    let in_front = cx.foreground().map(|foreground| foreground.window);
    let last = facts.versions.last.as_ref();
    let mut tried = Vec::new();
    for candidate in candidates.into_iter().rev() {
        if acceptance::accept(candidate.tell.kind, candidate.facts(), false).is_err() {
            continue;
        }
        let judged = backend.judge(
            Judged::Msaa,
            &candidate.window.judged(),
            candidate.tell.at_us,
            now_us,
        );
        if judged == Judgement::Drop || on_hidden_frame(&candidate) {
            continue;
        }
        if redirected_to(&facts.versions, &candidate.ids) {
            return (tried, true);
        }
        match judge_candidate(last, &candidate.ids, candidate.foreground(), in_front) {
            RunJudgement::Refused(_) => {}
            RunJudgement::Duplicate => return (tried, true),
            RunJudgement::Try => tried.push(candidate),
        }
    }
    (tried, false)
}

/// Whether `candidate` names Verbatim's hidden frame's own object, which
/// is dropped (Verbatim-only, design "Focus and foreground").
fn on_hidden_frame(candidate: &Candidate) -> bool {
    candidate.facts().is_some_and(|facts| {
        facts.hidden_frame
            && candidate.ids.window == facts.window.handle
            && matches!(candidate.ids.object, OBJID_WINDOW | OBJID_CLIENT)
            && candidate.ids.child == 0
    })
}

/// The object id of a window itself.
const OBJID_WINDOW: i32 = 0;
/// The object id of a window's client area.
const OBJID_CLIENT: i32 = -4;

/// Whether `ids` are the focused child a redirect from a list reached for
/// the committed focus, with no focus executed since: such a focus ends
/// the run without a read (Verbatim-only, design "Focus and foreground";
/// pinned by `theme_panel`).
fn redirected_to(versions: &super::versions::Versions, ids: &MsaaIds) -> bool {
    versions
        .committed_read()
        .is_some_and(|kept| kept.redirected_to.as_ref() == Some(ids))
}

/// `tried`, newest first, by application, in the order of each
/// application's newest candidate.
fn by_application(tried: Vec<Candidate>) -> Vec<Vec<Candidate>> {
    let mut groups: Vec<Vec<Candidate>> = Vec::new();
    for candidate in tried {
        let process = candidate.tell.process;
        match groups.iter_mut().find(|group| {
            group
                .first()
                .is_some_and(|first| first.tell.process == process)
        }) {
            Some(group) => group.push(candidate),
            None => groups.push(vec![candidate]),
        }
    }
    groups
}

/// The function of a run entry, its candidates oldest first. An outcome
/// other than a result ends the run: after an abandoned try the older
/// candidates are not tried (a limit).
pub(crate) async fn run(mut cx: Cx<Entries>, candidates: Vec<Candidate>) {
    let due_us = cx.due_in(FOCUS_DUE_US);
    let (tried, ended) = to_try(&cx, candidates);
    // A duplicate or the redirect's child ends the run as its valid focus,
    // which the menu slot's entry after it judges.
    if ended {
        cx.facts_mut().run_executed = true;
    }
    for group in by_application(tried) {
        let Some(newest) = group.first() else {
            continue;
        };
        let (window, process) = (newest.window, newest.tell.process);
        let generation = cx
            .instance(Reader::Outpost(process))
            .map(|instance| OutpostId(instance.generation));
        let known = Known::of(&cx.facts(), generation);
        let listed = {
            let backend = cx.backend();
            let now_us = cx.now_us();
            group
                .iter()
                .map(|candidate| RunCandidate {
                    ids: candidate.ids,
                    kind: if candidate.foreground() {
                        CandidateKind::Foreground
                    } else {
                        CandidateKind::Focus
                    },
                    verdict: backend.for_request(&candidate.window.judged(), now_us),
                })
                .collect()
        };
        let request = RunRequest {
            window,
            process,
            run: MsaaRun {
                candidates: listed,
                set: FOCUS_SET.to_vec(),
                known: known.sent.clone(),
            },
        };
        let Outcome::Result(answered) = cx.ask(request, due_us).await else {
            cx.facts_mut().focus_read_failed();
            return;
        };
        let valid =
            answered
                .tried
                .into_iter()
                .zip(&group)
                .find_map(|(tried, candidate)| match tried {
                    Tried::Focus(read) => Some((read, candidate)),
                    Tried::Skipped | Tried::Refused(_) | Tried::NotThere => None,
                });
        if let Some((read, candidate)) = valid {
            executed(&mut cx, candidate, &read, &known);
            return;
        }
    }
}

/// Executes the valid focus `read` of `candidate`. An MSAA answer without
/// the ids it was read by is the outpost's fault, and ends unspoken.
fn executed(cx: &mut Cx<Entries>, candidate: &Candidate, read: &FocusRead, known: &Known) {
    let Some(held) = read.msaa else {
        return;
    };
    let read_by = ReadBy::Msaa {
        event: Some(held.event),
        held_child: held.held_child,
    };
    let origin = Origin::of_tell(&candidate.tell, candidate.window);
    let executed = {
        let mut facts = cx.facts_mut();
        let executed = execute(&mut facts, &origin, read, known, read_by);
        facts.run_executed = true;
        executed
    };
    executed.deliver(cx);
}
