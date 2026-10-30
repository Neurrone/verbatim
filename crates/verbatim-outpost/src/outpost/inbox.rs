//! A worker's inbox (`docs/design/ownership-outpost-reducer.md`, "The caret
//! watch and the console poll"): a channel of jobs with the worker's own
//! [`Alarm`] as its doorbell, so the worker can wait for its next job and
//! for a time of its own in one high-resolution wait.
//!
//! The sender sends a job and then sets the alarm's event; letting go of
//! the sender drops the channel's sender first and then sets the event, so
//! the woken worker finds the inbox closed. The worker takes jobs without
//! waiting and waits on the alarm when none is there. The event is
//! auto-reset, so a job sent between a look that found nothing and the
//! wait ends that wait at once. A channel's own timed receive would park at
//! the system's timer resolution, 15.6 ms by default, which would stretch
//! the console poll's 5 ms reads.

use std::sync::Arc;
use std::time::Instant;

use super::alarm::Alarm;

/// A new inbox: its sending side and its receiving side.
pub(super) fn inbox<T>() -> (Sender<T>, Inbox<T>) {
    let (sender, receiver) = crossbeam_channel::unbounded();
    let alarm = Arc::new(Alarm::default());
    (
        Sender {
            jobs: Some(sender),
            alarm: Arc::clone(&alarm),
        },
        Inbox {
            jobs: receiver,
            alarm,
        },
    )
}

/// The sending side of an inbox.
pub(super) struct Sender<T> {
    /// Always `Some` until the sender is dropped.
    jobs: Option<crossbeam_channel::Sender<T>>,
    alarm: Arc<Alarm>,
}

impl<T> Sender<T> {
    /// Sends `job` and rings the doorbell; gives `job` back when the inbox
    /// is gone.
    pub(super) fn send(&self, job: T) -> Result<(), T> {
        let Some(jobs) = &self.jobs else {
            return Err(job);
        };
        jobs.send(job)
            .map_err(crossbeam_channel::SendError::into_inner)?;
        self.alarm.wake();
        Ok(())
    }
}

impl<T> Drop for Sender<T> {
    /// Closes the inbox, then rings the doorbell, so the worker sees the
    /// inbox closed when it wakes.
    fn drop(&mut self) {
        drop(self.jobs.take());
        self.alarm.wake();
    }
}

/// What a look at an inbox finds ([`Inbox::try_take`]).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Taken<T> {
    /// The next job.
    Job(T),
    /// Nothing waits.
    Empty,
    /// The sender is gone and nothing waits.
    Closed,
}

/// The receiving side of an inbox, its worker's.
pub(super) struct Inbox<T> {
    jobs: crossbeam_channel::Receiver<T>,
    alarm: Arc<Alarm>,
}

impl<T> Inbox<T> {
    /// The next job, without waiting.
    pub(super) fn try_take(&self) -> Taken<T> {
        match self.jobs.try_recv() {
            Ok(job) => Taken::Job(job),
            Err(crossbeam_channel::TryRecvError::Empty) => Taken::Empty,
            Err(crossbeam_channel::TryRecvError::Disconnected) => Taken::Closed,
        }
    }

    /// The next job, waiting for one; `None` once the sender is gone and
    /// nothing waits.
    pub(super) fn take(&self) -> Option<T> {
        loop {
            match self.try_take() {
                Taken::Job(job) => return Some(job),
                Taken::Closed => return None,
                Taken::Empty => self.alarm.wait_for_event(),
            }
        }
    }

    /// Waits until `at` or until the doorbell rings, whichever comes first.
    /// A wake can come with nothing new to take, so the caller looks again.
    pub(super) fn wait_until(&self, at: Instant) {
        self.alarm.set(at);
        // A ring that ends the wait early leaves the due time set;
        // cancelling it spares the kernel a timer interrupt no one waits
        // for.
        if !self.alarm.wait() {
            self.alarm.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_job_sent_is_taken_once_in_order() {
        let (sender, inbox) = inbox();
        assert_eq!(sender.send(1), Ok(()));
        assert_eq!(sender.send(2), Ok(()));
        assert_eq!(inbox.try_take(), Taken::Job(1));
        assert_eq!(inbox.take(), Some(2));
        assert_eq!(inbox.try_take(), Taken::Empty);
    }

    #[test]
    fn the_inbox_closes_after_what_was_sent_before_the_sender_went() {
        let (sender, inbox) = inbox();
        assert_eq!(sender.send(1), Ok(()));
        drop(sender);
        assert_eq!(inbox.take(), Some(1));
        assert_eq!(inbox.take(), None);
        assert_eq!(inbox.try_take(), Taken::Closed);
    }

    #[test]
    fn a_job_sent_from_another_thread_ends_the_workers_wait() {
        let (sender, inbox) = inbox();
        let sending = std::thread::spawn(move || {
            assert_eq!(sender.send(7), Ok(()));
            sender
        });
        assert_eq!(inbox.take(), Some(7));
        drop(sending.join().expect("the sending thread"));
        assert_eq!(inbox.take(), None, "the sender's drop rings the doorbell");
    }

    #[test]
    fn a_ring_before_the_wait_ends_a_timed_wait_at_once() {
        let (sender, inbox) = inbox();
        assert_eq!(sender.send(3), Ok(()));
        // An hour away: only the ring can end the wait.
        inbox.wait_until(Instant::now() + Duration::from_secs(3_600));
        assert_eq!(inbox.try_take(), Taken::Job(3));
    }

    #[test]
    fn a_job_sent_to_a_gone_inbox_comes_back() {
        let (sender, inbox) = inbox();
        drop(inbox);
        assert_eq!(sender.send(4), Err(4));
    }
}
