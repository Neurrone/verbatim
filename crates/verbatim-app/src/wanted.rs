//! The requests the app holds for an application whose outpost is not
//! ready yet (`books/dev/src/explanation/design/ownership.md`): a focus-now request, asked of the replacement once
//! it is ready, and the list dialogs' requests for the shell's items or for
//! where a clicked item is, asked of the taskbar's process. One owner holds
//! both, keyed by the process whose outpost they wait for, and each entry
//! ends exactly once:
//!
//! - at its outpost's `Ready` ([`Wanted::ready`]), where it is sent and the
//!   request table owns its outcome from then on, or, for a shell items
//!   request whose outpost deadline has passed by then, answered with
//!   nothing;
//! - at the application's `NotWatched` ([`Wanted::not_watched`]), where it
//!   is answered "gone".
//!
//! The supervisor ends every start of an outpost with one of the two, so
//! no timer of the app's ends a request here: a timer would be a second
//! path to the same end. A request waiting through a failure's backoff
//! keeps waiting, so the replacement answers it: an `Ended` with a
//! generation following ends nothing here.

use std::collections::HashMap;
use std::time::Duration;

use verbatim_gui::{ShellAsk, ShellItemKind};
use verbatim_model::protocol::FocusPurpose;
use verbatim_model::{Pid, TraceId};

/// The list dialog's whole wait for its list or a clicked item's position
/// from its request, whether the taskbar's outpost is starting, the query
/// waits behind other work in the outpost, or the walk is slow
/// (`books/dev/src/explanation/design/listener-without-connections.md`, section 10, step 11),
/// the one clock of the request: the outpost's deadline for the query,
/// carried in it as its `answer_by_us`, by which the outpost answers it,
/// with its answer, with no items or abandoned. It is 63 seconds (Dickson,
/// 2026-11-07): four focus read deadlines
/// ([`FOCUS_READ_DEADLINE`](verbatim_model::FOCUS_READ_DEADLINE)),
/// the longest a listener read's marker holds the query behind it when the
/// taskbar and a folder window share one `explorer.exe`, three deadlines of
/// abandoned reads and one report, and the program's own
/// ([`HANDLING_DEADLINE`](verbatim_model::HANDLING_DEADLINE)), so an answer
/// that comes is never thrown away.
pub(crate) const SHELL_ITEMS_WAIT: Duration = verbatim_model::FOCUS_READ_DEADLINE
    .saturating_mul(4)
    .saturating_add(verbatim_model::HANDLING_DEADLINE);

// The wait holds the marker's longest hold and the query's own program.
const _: () = assert!(
    SHELL_ITEMS_WAIT.as_micros()
        >= 4 * verbatim_model::FOCUS_READ_DEADLINE.as_micros()
            + verbatim_model::HANDLING_DEADLINE.as_micros()
);

/// The times of one shell items request, all on the run's shared clock and
/// fixed when the dialog asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShellTimes {
    /// When the dialog asked, in microseconds.
    pub(crate) asked_us: u64,
}

impl ShellTimes {
    /// The outpost's deadline for the query, past which it is abandoned:
    /// [`SHELL_ITEMS_WAIT`] from the request. A request not sent by then is
    /// answered with nothing.
    pub(crate) fn answer_by_us(self) -> u64 {
        self.asked_us.saturating_add(micros(SHELL_ITEMS_WAIT))
    }
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// A shell items request waiting for an outpost.
pub(crate) struct ShellWanted {
    /// The surface asked about.
    pub(crate) kind: ShellItemKind,
    /// What is asked, and where the outcome goes: dropped unanswered, it
    /// answers "nothing".
    pub(crate) ask: ShellAsk,
    /// Its times, from when the dialog asked.
    pub(crate) times: ShellTimes,
}

/// A focus query waiting for an outpost: the application to ask for its
/// focus, which differs from the process for a console host, asked for its
/// console's client, why, and the trace it is asked under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FocusWanted {
    /// The application whose focus is asked for.
    pub(crate) application: Pid,
    /// Why ([`FocusPurpose`]).
    pub(crate) purpose: FocusPurpose,
    /// The trace it is asked under: Verbatim+Tab's press's for a
    /// `Report`.
    pub(crate) trace_id: TraceId,
}

/// What waits for one process's outpost.
#[derive(Default)]
struct Waiting {
    /// The focus query.
    focus_now: Option<FocusWanted>,
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
    /// The focus query to send, if any.
    pub(crate) focus_now: Option<FocusWanted>,
    /// The shell items requests to send.
    pub(crate) shell: Vec<ShellWanted>,
}

impl Wanted {
    /// Wants the focus query `wanted` of the outpost of `process`, once it
    /// is ready. A newer want replaces one waiting, which is returned, its
    /// asker to be told it ends unanswered.
    pub(crate) fn want_focus_now(
        &mut self,
        process: Pid,
        wanted: FocusWanted,
    ) -> Option<FocusWanted> {
        self.waiting
            .entry(process)
            .or_default()
            .focus_now
            .replace(wanted)
    }

    /// Wants `ask` of `kind`'s surface from the outpost of `process`, once
    /// it is ready, or answered with nothing at its `NotWatched`.
    pub(crate) fn want_shell_items(
        &mut self,
        process: Pid,
        kind: ShellItemKind,
        ask: ShellAsk,
        times: ShellTimes,
    ) {
        self.waiting
            .entry(process)
            .or_default()
            .shell
            .push(ShellWanted { kind, ask, times });
    }

    /// The outpost of `process` is ready at `now_us`, on the run's shared
    /// clock: what waits for it, taken, to be sent to it. A shell items
    /// request whose outpost deadline is `now_us` or earlier is answered
    /// with nothing instead.
    pub(crate) fn ready(&mut self, process: Pid, now_us: u64) -> Asked {
        let waiting = self.waiting.remove(&process).unwrap_or_default();
        let (late, shell): (Vec<ShellWanted>, Vec<ShellWanted>) = waiting
            .shell
            .into_iter()
            .partition(|wanted| wanted.times.answer_by_us() <= now_us);
        for wanted in late {
            tracing::warn!(
                kind = ?wanted.kind,
                %process,
                "the taskbar's outpost was ready too late for its query; the shell's items are not read"
            );
            drop(wanted.ask);
        }
        Asked {
            focus_now: waiting.focus_now,
            shell,
        }
    }

    /// Nothing watches `process` now: what waits for its outpost is
    /// answered "gone", a shell items request with nothing; the focus query
    /// is returned, its asker to be told it ends unanswered.
    pub(crate) fn not_watched(&mut self, process: Pid) -> Option<FocusWanted> {
        let waiting = self.waiting.remove(&process)?;
        if let Some(wanted) = waiting.focus_now {
            tracing::info!(%process, application = %wanted.application, "no outpost will report the focus; it is no longer wanted");
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
        waiting.focus_now
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

    /// The dialog asked at 10 s on the shared clock.
    const ASKED: ShellTimes = ShellTimes {
        asked_us: 10_000_000,
    };

    #[test]
    fn a_shell_items_requests_times_are_fixed_from_the_dialogs_request() {
        assert_eq!(ASKED.answer_by_us(), 73_000_000);
    }

    /// A plain focus query for application 8.
    const PLAIN: FocusWanted = FocusWanted {
        application: Pid(8),
        purpose: FocusPurpose::Plain,
        trace_id: TraceId::from_raw(3),
    };

    #[test]
    fn a_focus_now_request_is_gone_at_not_watched_and_ends_once() {
        let mut wanted = Wanted::default();
        assert_eq!(wanted.want_focus_now(PROCESS, PLAIN), None);
        assert_eq!(wanted.focus_now_count(), 1);
        assert_eq!(wanted.not_watched(PROCESS), Some(PLAIN));
        assert_eq!(wanted.focus_now_count(), 0);
        assert_eq!(
            wanted.ready(PROCESS, 0).focus_now,
            None,
            "never again at a Ready"
        );
    }

    /// A focus-now request has no deadline: it waits, through a backoff
    /// too, for its outpost's `Ready`, where it is taken once.
    #[test]
    fn a_focus_now_request_has_no_deadline_and_is_taken_once_at_its_ready() {
        let mut wanted = Wanted::default();
        assert_eq!(wanted.want_focus_now(PROCESS, PLAIN), None);
        assert_eq!(wanted.focus_now_count(), 1, "kept; it has no deadline");
        assert_eq!(wanted.ready(PROCESS, 0).focus_now, Some(PLAIN));
        assert_eq!(wanted.ready(PROCESS, 0).focus_now, None, "taken once");
        assert_eq!(wanted.focus_now_count(), 0);
    }

    #[test]
    fn a_shell_items_request_is_answered_nothing_at_not_watched_once() {
        let mut wanted = Wanted::default();
        let (outcomes, ask) = recorder();
        wanted.want_shell_items(PROCESS, ShellItemKind::SystemTray, ask, ASKED);
        wanted.not_watched(Pid(9));
        assert_eq!(*outcomes.lock().expect("outcomes"), Vec::new(), "another's");
        wanted.not_watched(PROCESS);
        assert!(wanted.ready(PROCESS, ASKED.asked_us).shell.is_empty());
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![None]);
    }

    /// A request whose outpost is ready before the outpost's deadline for
    /// it is taken to be sent, with its times, and not answered here.
    #[test]
    fn a_shell_items_request_ready_before_its_deadline_is_taken_and_not_answered_here() {
        let mut wanted = Wanted::default();
        let (outcomes, ask) = recorder();
        wanted.want_shell_items(PROCESS, ShellItemKind::Taskbar, ask, ASKED);
        let taken = wanted.ready(PROCESS, ASKED.answer_by_us() - 1).shell;
        assert_eq!(
            taken
                .iter()
                .map(|wanted| (wanted.kind, wanted.times))
                .collect::<Vec<_>>(),
            vec![(ShellItemKind::Taskbar, ASKED)]
        );
        wanted.not_watched(PROCESS);
        assert_eq!(*outcomes.lock().expect("outcomes"), Vec::new());
        drop(taken);
    }

    /// A request whose outpost is ready at the outpost's deadline for it is
    /// answered with nothing once and not sent.
    #[test]
    fn a_shell_items_request_ready_at_its_deadline_is_answered_nothing_and_not_sent() {
        let mut wanted = Wanted::default();
        let (outcomes, ask) = recorder();
        wanted.want_shell_items(PROCESS, ShellItemKind::Taskbar, ask, ASKED);
        assert!(wanted.ready(PROCESS, ASKED.answer_by_us()).shell.is_empty());
        wanted.not_watched(PROCESS);
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![None]);
    }
}
