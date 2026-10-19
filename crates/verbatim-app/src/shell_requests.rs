//! The tray and taskbar list dialogs' requests for the shell's items that
//! wait for the taskbar's process to have a ready outpost.
//!
//! Each request has a deadline from the moment the dialog asked
//! ([`SHELL_ITEMS_DEADLINE`]) and gets exactly one outcome: the request it
//! becomes once the outpost is ready ([`ShellItemsWanted::take`]), whose
//! own outcome the request table owns under the same deadline; nothing
//! when the process will get no outpost ([`ShellItemsWanted::refuse`]); or
//! nothing once its deadline passes ([`ShellItemsWanted::expire`]).

use std::time::{Duration, Instant};

use verbatim_gui::{DeliverShellItems, ShellItemKind};
use verbatim_model::Pid;

/// How long a request for the shell's items may take, from the dialog's
/// request to its answer, whether it waits for the taskbar's outpost or for
/// the outpost's walk: the outpost's own deadline for the walk
/// (`WALK_DEADLINE` in `verbatim-outpost`'s worker), so a dialog never
/// waits for an outpost that is starting, or waits out a failure's
/// backoff, longer than it would wait for a slow walk.
pub(crate) const SHELL_ITEMS_DEADLINE: Duration = Duration::from_secs(5);

/// One request waiting for an outpost.
pub(crate) struct Waiting {
    /// The process whose outpost it waits for.
    pid: Pid,
    /// The items asked for.
    pub(crate) kind: ShellItemKind,
    /// Where the outcome goes.
    pub(crate) deliver: DeliverShellItems,
    /// When it is answered with nothing if it is still waiting.
    pub(crate) deadline: Instant,
}

/// The requests waiting for an outpost, in the order they were made.
#[derive(Default)]
pub(crate) struct ShellItemsWanted {
    waiting: Vec<Waiting>,
}

impl ShellItemsWanted {
    /// Records a request for `kind`, waiting for the outpost of `pid`,
    /// answered with nothing at `deadline` if it is still waiting.
    pub(crate) fn push(
        &mut self,
        pid: Pid,
        kind: ShellItemKind,
        deliver: DeliverShellItems,
        deadline: Instant,
    ) {
        self.waiting.push(Waiting {
            pid,
            kind,
            deliver,
            deadline,
        });
    }

    /// The requests waiting for the outpost of `pid`, which is ready: taken
    /// from the list, to be sent to it.
    pub(crate) fn take(&mut self, pid: Pid) -> Vec<Waiting> {
        let (taken, kept) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|waiting| waiting.pid == pid);
        self.waiting = kept;
        taken
    }

    /// Answers every request waiting for the outpost of `pid`, which will
    /// not come, with nothing.
    pub(crate) fn refuse(&mut self, pid: Pid) {
        for waiting in self.take(pid) {
            tracing::warn!(
                kind = ?waiting.kind,
                %pid,
                "the taskbar's process has no outpost; the shell's items are not listed"
            );
            (waiting.deliver)(None);
        }
    }

    /// Answers every request whose deadline is `now` or earlier with
    /// nothing.
    pub(crate) fn expire(&mut self, now: Instant) {
        if self.waiting.is_empty() {
            return;
        }
        let (expired, kept): (Vec<Waiting>, Vec<Waiting>) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|waiting| waiting.deadline <= now);
        self.waiting = kept;
        for waiting in expired {
            tracing::warn!(
                kind = ?waiting.kind,
                pid = %waiting.pid,
                "the taskbar's outpost was not ready by the request's deadline; the shell's items are not listed"
            );
            (waiting.deliver)(None);
        }
    }

    /// The earliest deadline of a waiting request.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.waiting.iter().map(|waiting| waiting.deadline).min()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use verbatim_gui::ShellItem;

    use super::*;

    /// The outcomes a request's `deliver` was given, in order.
    type Outcomes = Arc<Mutex<Vec<Option<Vec<ShellItem>>>>>;

    fn recorder() -> (Outcomes, DeliverShellItems) {
        let outcomes = Outcomes::default();
        let kept = Arc::clone(&outcomes);
        let deliver: DeliverShellItems =
            Box::new(move |items| kept.lock().expect("outcomes").push(items));
        (outcomes, deliver)
    }

    #[test]
    fn a_request_is_taken_when_its_outpost_is_ready_and_not_answered_here() {
        let mut wanted = ShellItemsWanted::default();
        let (outcomes, deliver) = recorder();
        let deadline = Instant::now() + SHELL_ITEMS_DEADLINE;
        wanted.push(Pid(7), ShellItemKind::Taskbar, deliver, deadline);
        let taken = wanted.take(Pid(7));
        assert_eq!(
            taken
                .iter()
                .map(|waiting| (waiting.kind, waiting.deadline))
                .collect::<Vec<_>>(),
            vec![(ShellItemKind::Taskbar, deadline)]
        );
        wanted.expire(deadline);
        assert_eq!(wanted.next_deadline(), None);
        assert_eq!(*outcomes.lock().expect("outcomes"), Vec::new());
    }

    #[test]
    fn a_request_for_a_process_with_no_outpost_is_answered_with_nothing_once() {
        let mut wanted = ShellItemsWanted::default();
        let (outcomes, deliver) = recorder();
        let deadline = Instant::now() + SHELL_ITEMS_DEADLINE;
        wanted.push(Pid(7), ShellItemKind::SystemTray, deliver, deadline);
        wanted.refuse(Pid(7));
        wanted.expire(deadline);
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![None]);
    }

    #[test]
    fn a_request_for_another_process_is_not_refused() {
        let mut wanted = ShellItemsWanted::default();
        let (outcomes, deliver) = recorder();
        let deadline = Instant::now() + SHELL_ITEMS_DEADLINE;
        wanted.push(Pid(7), ShellItemKind::SystemTray, deliver, deadline);
        wanted.refuse(Pid(8));
        assert_eq!(*outcomes.lock().expect("outcomes"), Vec::new());
        assert_eq!(wanted.next_deadline(), Some(deadline));
    }

    #[test]
    fn a_request_still_waiting_at_its_deadline_is_answered_with_nothing_once() {
        let mut wanted = ShellItemsWanted::default();
        let (outcomes, deliver) = recorder();
        let now = Instant::now();
        let deadline = now + SHELL_ITEMS_DEADLINE;
        wanted.push(Pid(7), ShellItemKind::SystemTray, deliver, deadline);
        wanted.expire(now);
        assert_eq!(*outcomes.lock().expect("outcomes"), Vec::new());
        wanted.expire(deadline);
        wanted.refuse(Pid(7));
        assert_eq!(*outcomes.lock().expect("outcomes"), vec![None]);
        assert!(wanted.take(Pid(7)).is_empty());
    }
}
