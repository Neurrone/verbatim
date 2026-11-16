//! The requests the app holds for the taskbar's process while its outpost
//! is not ready yet (`books/dev/src/explanation/design/ownership.md`): the
//! list dialogs' requests for the shell's items or for where a clicked item
//! is. Core's event queue waits for an outpost itself for each request of
//! its own. Each entry here, keyed by the process whose outpost it waits
//! for, ends exactly once:
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
use verbatim_model::Pid;

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
    /// Which surface's items.
    pub(crate) kind: ShellItemKind,
    /// The dialog's request, answered exactly once.
    pub(crate) ask: ShellAsk,
    /// Its times.
    pub(crate) times: ShellTimes,
}

/// What waits for each process's outpost.
#[derive(Default)]
pub(crate) struct Wanted {
    waiting: HashMap<Pid, Vec<ShellWanted>>,
}

impl Wanted {
    /// Holds the dialog's request `ask` of the surface `kind` for
    /// `process`'s outpost.
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
            .push(ShellWanted { kind, ask, times });
    }

    /// `process`'s outpost is ready at `now_us`: the requests to send it,
    /// each one whose deadline has passed answered with nothing instead.
    pub(crate) fn ready(&mut self, process: Pid, now_us: u64) -> Vec<ShellWanted> {
        let waiting = self.waiting.remove(&process).unwrap_or_default();
        let (late, shell): (Vec<ShellWanted>, Vec<ShellWanted>) = waiting
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
        shell
    }

    /// No outpost will serve `process`: each request waiting for it is
    /// answered with nothing.
    pub(crate) fn not_watched(&mut self, process: Pid) {
        for wanted in self.waiting.remove(&process).unwrap_or_default() {
            tracing::warn!(
                kind = ?wanted.kind,
                %process,
                "the taskbar's process has no outpost; the shell's items are not read"
            );
            // Dropped unanswered, the request is answered "nothing".
            drop(wanted.ask);
        }
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

    #[test]
    fn a_shell_items_request_is_answered_nothing_at_not_watched_once() {
        let mut wanted = Wanted::default();
        let (outcomes, ask) = recorder();
        wanted.want_shell_items(PROCESS, ShellItemKind::SystemTray, ask, ASKED);
        wanted.not_watched(Pid(9));
        assert_eq!(*outcomes.lock().expect("outcomes"), Vec::new(), "another's");
        wanted.not_watched(PROCESS);
        assert!(wanted.ready(PROCESS, ASKED.asked_us).is_empty());
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![None]);
    }

    /// A request whose outpost is ready before the outpost's deadline for
    /// it is taken to be sent, with its times, and not answered here.
    #[test]
    fn a_shell_items_request_ready_before_its_deadline_is_taken_and_not_answered_here() {
        let mut wanted = Wanted::default();
        let (outcomes, ask) = recorder();
        wanted.want_shell_items(PROCESS, ShellItemKind::Taskbar, ask, ASKED);
        let taken = wanted.ready(PROCESS, ASKED.answer_by_us() - 1);
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
        assert!(wanted.ready(PROCESS, ASKED.answer_by_us()).is_empty());
        wanted.not_watched(PROCESS);
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![None]);
    }
}
