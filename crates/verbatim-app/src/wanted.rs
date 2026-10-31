//! The requests the app holds for an application whose outpost is not
//! ready yet (`docs/design/ownership.md`): a focus-now request, asked of the replacement once
//! it is ready, and the list dialogs' requests for the shell's items or for
//! where a clicked item is, asked of the taskbar's process. One owner holds
//! both, keyed by the process whose outpost they wait for, and each entry
//! ends exactly once:
//!
//! - at its outpost's `Ready` ([`Wanted::ready`]), where it is sent and the
//!   request table owns its outcome from then on;
//! - at the application's `NotWatched` ([`Wanted::not_watched`]), where it
//!   is answered "gone";
//! - for a shell items request only, at its deadline, [`SHELL_ITEMS_DEADLINE`]
//!   after the dialog asked ([`Wanted::expire`]).
//!
//! A request waiting through a failure's backoff keeps waiting, so the
//! replacement answers it: an `Ended` with a generation following ends
//! nothing here.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use verbatim_gui::{ShellAsk, ShellItemKind};
use verbatim_model::Pid;
use verbatim_model::protocol::WALK_DEADLINE;

/// How long Core waits for the answer to a shell items or position query
/// from when it sends it, 6 seconds: the outpost's own deadline for the walk
/// ([`WALK_DEADLINE`], 5 seconds from when its worker begins the query),
/// by which it has answered the items read, in part once the walk's time
/// budget has passed, or the walk abandoned, and a second for the query's
/// and the answer's way through the pipes and the reducer thread. Core
/// gives up only after the outpost's deadline has run out, so the items
/// read by then are not lost to Core's deadline
/// (`docs/design/listener-without-connections.md`, section 10, step 11).
/// It holds when the worker begins the query as it arrives; a query
/// behind another entry of a busy worker begins later, by up to that
/// entry's own deadline. A request also waits for the taskbar's outpost
/// at most this long, from the dialog's request, so a dialog never waits
/// for an outpost that is starting, or waits out a failure's backoff,
/// longer than it would wait for a slow walk.
pub(crate) const SHELL_ITEMS_DEADLINE: Duration =
    WALK_DEADLINE.saturating_add(Duration::from_secs(1));

/// A shell items request waiting for an outpost.
pub(crate) struct ShellWanted {
    /// The surface asked about.
    pub(crate) kind: ShellItemKind,
    /// What is asked, and where the outcome goes: dropped unanswered, it
    /// answers "nothing".
    pub(crate) ask: ShellAsk,
    /// When it is answered with nothing if it is still waiting.
    pub(crate) deadline: Instant,
}

/// What waits for one process's outpost.
#[derive(Default)]
struct Waiting {
    /// The application to ask for its focus, which differs from the process
    /// for a console host, asked for its console's client.
    focus_now: Option<Pid>,
    /// The shell items requests, in the order they were made.
    shell: Vec<ShellWanted>,
}

/// The requests waiting for outposts, by the process whose outpost each
/// waits for.
#[derive(Default)]
pub(crate) struct Wanted {
    waiting: HashMap<Pid, Waiting>,
}

/// What a ready outpost is to be asked.
pub(crate) struct Asked {
    /// The application whose focus to ask for, if any.
    pub(crate) focus_now: Option<Pid>,
    /// The shell items requests to send.
    pub(crate) shell: Vec<ShellWanted>,
}

impl Wanted {
    /// Wants the focus of `application` from the outpost of `process`, once
    /// it is ready. A newer want replaces one waiting.
    pub(crate) fn want_focus_now(&mut self, process: Pid, application: Pid) {
        self.waiting.entry(process).or_default().focus_now = Some(application);
    }

    /// Wants `ask` of `kind`'s surface from the outpost of `process`, once
    /// it is ready, answered with nothing at `deadline` if it still waits.
    pub(crate) fn want_shell_items(
        &mut self,
        process: Pid,
        kind: ShellItemKind,
        ask: ShellAsk,
        deadline: Instant,
    ) {
        self.waiting
            .entry(process)
            .or_default()
            .shell
            .push(ShellWanted {
                kind,
                ask,
                deadline,
            });
    }

    /// The outpost of `process` is ready: what waits for it, taken, to be
    /// sent to it.
    pub(crate) fn ready(&mut self, process: Pid) -> Asked {
        let waiting = self.waiting.remove(&process).unwrap_or_default();
        Asked {
            focus_now: waiting.focus_now,
            shell: waiting.shell,
        }
    }

    /// Nothing watches `process` now: what waits for its outpost is
    /// answered "gone", a shell items request with nothing.
    pub(crate) fn not_watched(&mut self, process: Pid) {
        let Some(waiting) = self.waiting.remove(&process) else {
            return;
        };
        if let Some(application) = waiting.focus_now {
            tracing::info!(%process, %application, "no outpost will report the focus; it is no longer wanted");
        }
        for wanted in waiting.shell {
            tracing::warn!(
                kind = ?wanted.kind,
                %process,
                "the taskbar's process has no outpost; the shell's items are not read"
            );
            // Dropped unanswered, the request is answered "nothing".
            drop(wanted.ask);
        }
    }

    /// Answers every shell items request whose deadline is `now` or
    /// earlier with nothing.
    pub(crate) fn expire(&mut self, now: Instant) {
        for (process, waiting) in &mut self.waiting {
            let (expired, kept): (Vec<ShellWanted>, Vec<ShellWanted>) =
                std::mem::take(&mut waiting.shell)
                    .into_iter()
                    .partition(|wanted| wanted.deadline <= now);
            waiting.shell = kept;
            for wanted in expired {
                tracing::warn!(
                    kind = ?wanted.kind,
                    %process,
                    "the taskbar's outpost was not ready by the request's deadline; the shell's items are not read"
                );
                drop(wanted.ask);
            }
        }
        self.waiting
            .retain(|_, waiting| waiting.focus_now.is_some() || !waiting.shell.is_empty());
    }

    /// The earliest deadline of a waiting shell items request.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.waiting
            .values()
            .flat_map(|waiting| waiting.shell.iter().map(|wanted| wanted.deadline))
            .min()
    }

    /// How many applications' focus is wanted.
    pub(crate) fn focus_now_count(&self) -> usize {
        self.waiting
            .values()
            .filter(|waiting| waiting.focus_now.is_some())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use verbatim_gui::ShellItem;
    use verbatim_model::protocol::Responder;

    use super::*;

    /// The outcomes a request's responder gave, in order.
    type Outcomes = Arc<Mutex<Vec<Option<Vec<ShellItem>>>>>;

    fn recorder() -> (Outcomes, ShellAsk) {
        let outcomes = Outcomes::default();
        let kept = Arc::clone(&outcomes);
        let ask = ShellAsk::Items(Responder::new(move |items| {
            kept.lock().expect("outcomes").push(items);
        }));
        (outcomes, ask)
    }

    const PROCESS: Pid = Pid(7);

    #[test]
    fn a_focus_now_request_is_gone_at_not_watched_and_ends_once() {
        let mut wanted = Wanted::default();
        wanted.want_focus_now(PROCESS, Pid(8));
        assert_eq!(wanted.focus_now_count(), 1);
        wanted.not_watched(PROCESS);
        assert_eq!(wanted.focus_now_count(), 0);
        assert_eq!(
            wanted.ready(PROCESS).focus_now,
            None,
            "never again at a Ready"
        );
    }

    /// A focus-now request has no deadline: it waits, through a backoff
    /// too, for its outpost's `Ready`, where it is taken once.
    #[test]
    fn a_focus_now_request_has_no_deadline_and_is_taken_once_at_its_ready() {
        let mut wanted = Wanted::default();
        wanted.want_focus_now(PROCESS, Pid(8));
        wanted.expire(Instant::now() + SHELL_ITEMS_DEADLINE);
        assert_eq!(wanted.focus_now_count(), 1, "kept; it has no deadline");
        assert_eq!(wanted.ready(PROCESS).focus_now, Some(Pid(8)));
        assert_eq!(wanted.ready(PROCESS).focus_now, None, "taken once");
        assert_eq!(wanted.focus_now_count(), 0);
    }

    #[test]
    fn a_shell_items_request_is_answered_nothing_at_not_watched_once() {
        let mut wanted = Wanted::default();
        let (outcomes, ask) = recorder();
        let deadline = Instant::now() + SHELL_ITEMS_DEADLINE;
        wanted.want_shell_items(PROCESS, ShellItemKind::SystemTray, ask, deadline);
        wanted.not_watched(Pid(9));
        assert_eq!(*outcomes.lock().expect("outcomes"), Vec::new(), "another's");
        wanted.not_watched(PROCESS);
        wanted.expire(deadline);
        assert!(wanted.ready(PROCESS).shell.is_empty());
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![None]);
    }

    #[test]
    fn a_shell_items_request_kept_until_its_ready_is_taken_and_not_answered_here() {
        let mut wanted = Wanted::default();
        let (outcomes, ask) = recorder();
        let deadline = Instant::now() + SHELL_ITEMS_DEADLINE;
        wanted.want_shell_items(PROCESS, ShellItemKind::Taskbar, ask, deadline);
        let taken = wanted.ready(PROCESS).shell;
        assert_eq!(
            taken
                .iter()
                .map(|wanted| (wanted.kind, wanted.deadline))
                .collect::<Vec<_>>(),
            vec![(ShellItemKind::Taskbar, deadline)]
        );
        wanted.expire(deadline);
        wanted.not_watched(PROCESS);
        assert_eq!(wanted.next_deadline(), None);
        assert_eq!(*outcomes.lock().expect("outcomes"), Vec::new());
        drop(taken);
    }

    #[test]
    fn a_shell_items_request_still_waiting_at_its_deadline_is_answered_nothing_once() {
        let mut wanted = Wanted::default();
        let (outcomes, ask) = recorder();
        let now = Instant::now();
        let deadline = now + SHELL_ITEMS_DEADLINE;
        wanted.want_shell_items(PROCESS, ShellItemKind::SystemTray, ask, deadline);
        assert_eq!(wanted.next_deadline(), Some(deadline));
        wanted.expire(now);
        assert_eq!(*outcomes.lock().expect("outcomes"), Vec::new());
        wanted.expire(deadline);
        wanted.not_watched(PROCESS);
        assert!(wanted.ready(PROCESS).shell.is_empty());
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![None]);
    }
}
