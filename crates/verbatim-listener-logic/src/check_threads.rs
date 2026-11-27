//! The listener's check threads' records
//! (`books/dev/src/reference/crates/verbatim-listener-logic.md`, "The check threads"): one thread
//! per application whose UI Automation events the listener hears, its
//! record keyed by the application's process id, taking that
//! application's events in arrival order.
//!
//! The records and their queues sit under one lock, the listener's: a
//! callback pushes an event under it, starting a thread when it finds no
//! record ([`CheckThreads::push`]); a thread takes its next event under it
//! ([`CheckThreads::take`]), and exits only by removing its record under it
//! while its queue is empty, after [`IDLE_US`] with nothing to check. So no
//! event is pushed to a thread that has decided to exit. A thread's check
//! ends under the lock too ([`CheckThreads::checked`]), whose tell is
//! placed only while its record is still its own, so no tell of an ended
//! application is sent.
//!
//! The idle barrier waits for every event pushed before it to be told or
//! dropped ([`CheckThreads::when_idle`]), never for one pushed after.

use std::collections::{BTreeMap, VecDeque};

use verbatim_model::Pid;
use verbatim_model::event_kind::EventKind;

/// An event a check thread checks, which names its kind, so that a thread
/// abandoned inside a check at shutdown names what it was checking.
pub trait Kinded {
    /// The event's kind.
    fn kind(&self) -> EventKind;
}

/// How long a check thread with nothing to check waits before it exits,
/// in microseconds. A thread costs one wake when it exits; one per
/// application keeps a busy application's thread for the next event.
pub const IDLE_US: u64 = 30_000_000;

/// One application's record: which run of its thread it is, its queue
/// and whether its thread is checking or idle.
#[derive(Debug)]
struct Record<E> {
    generation: u64,
    /// The events waiting, each with the number it was pushed under.
    queue: VecDeque<(u64, E)>,
    thread: Thread,
}

/// What an application's thread is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Thread {
    /// Started, or checking the event pushed under this number, of this
    /// kind.
    Checking(Option<(u64, EventKind)>),
    /// Waiting for an event, since this time.
    Idle { since_us: u64 },
}

/// What a push did.
#[derive(Debug, PartialEq, Eq)]
pub enum Pushed<E> {
    /// No record: one was made, and a thread is to be started for the
    /// application as `generation`.
    Start {
        /// The run of the thread to start.
        generation: u64,
    },
    /// Queued behind the thread's check.
    Queued,
    /// Queued for an idle thread, which is to be woken.
    Wake,
    /// The listener is stopping: the event is dropped untold.
    Dropped(E),
}

/// What a thread takes next.
#[derive(Debug, PartialEq, Eq)]
pub enum Take<E> {
    /// Check this event.
    Check(E),
    /// Nothing to check: wait until this time, or until woken.
    Wait {
        /// When the idle time ends.
        until_us: u64,
    },
    /// The thread exits: its record is removed, idle past [`IDLE_US`],
    /// or it is no longer its own.
    Exit,
}

/// The records of every application's check thread, and the idle
/// barrier's waiters, each `W`.
#[derive(Debug)]
pub struct CheckThreads<E, W> {
    records: BTreeMap<Pid, Record<E>>,
    /// The number the next record's thread runs as.
    next_generation: u64,
    /// How many events have been pushed.
    pushed: u64,
    /// The idle barrier's waiters, each with the number of events pushed
    /// when it came.
    waiters: VecDeque<(u64, W)>,
    stopping: bool,
}

impl<E, W> Default for CheckThreads<E, W> {
    fn default() -> Self {
        Self {
            records: BTreeMap::new(),
            next_generation: 0,
            pushed: 0,
            waiters: VecDeque::new(),
            stopping: false,
        }
    }
}

impl<E, W> CheckThreads<E, W> {
    /// An event of `pid`'s arrived, of any UI Automation kind the
    /// listener hears (column A).
    pub fn push(&mut self, pid: Pid, event: E) -> Pushed<E> {
        if self.stopping {
            return Pushed::Dropped(event);
        }
        self.pushed = self.pushed.saturating_add(1);
        let number = self.pushed;
        if let Some(record) = self.records.get_mut(&pid) {
            record.queue.push_back((number, event));
            return match record.thread {
                Thread::Checking(_) => Pushed::Queued,
                Thread::Idle { .. } => {
                    record.thread = Thread::Checking(None);
                    Pushed::Wake
                }
            };
        }
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        self.records.insert(
            pid,
            Record {
                generation,
                queue: VecDeque::from([(number, event)]),
                thread: Thread::Checking(None),
            },
        );
        Pushed::Start { generation }
    }

    /// `pid`'s thread `generation` takes its next event at `now_us`, after
    /// it started, after a check ([`Self::checked`]) or when its wait ends
    /// (columns B and C).
    pub fn take(&mut self, pid: Pid, generation: u64, now_us: u64) -> Take<E>
    where
        E: Kinded,
    {
        if self.stopping {
            return Take::Exit;
        }
        let Some(record) = self
            .records
            .get_mut(&pid)
            .filter(|record| record.generation == generation)
        else {
            return Take::Exit;
        };
        if let Some((number, event)) = record.queue.pop_front() {
            record.thread = Thread::Checking(Some((number, event.kind())));
            return Take::Check(event);
        }
        let since_us = match record.thread {
            Thread::Idle { since_us } => since_us,
            Thread::Checking(_) => {
                record.thread = Thread::Idle { since_us: now_us };
                now_us
            }
        };
        let until_us = since_us.saturating_add(IDLE_US);
        if now_us >= until_us {
            self.records.remove(&pid);
            return Take::Exit;
        }
        Take::Wait { until_us }
    }

    /// `pid`'s thread `generation` ended its check (column B): whether its
    /// tell is placed, which it is while the record is its own and the
    /// listener is not stopping, and the idle barrier's waiters that the
    /// check's end lets run, in order. Called under the lock, which the
    /// tell and the waiters are placed under.
    pub fn checked(&mut self, pid: Pid, generation: u64) -> (bool, Vec<W>) {
        let own = !self.stopping
            && self
                .records
                .get_mut(&pid)
                .filter(|record| record.generation == generation)
                .map(|record| {
                    if let Thread::Checking(in_hand) = &mut record.thread {
                        *in_hand = None;
                    }
                })
                .is_some();
        (own, self.waiters_due())
    }

    /// `pid`'s process generation ended (column D): its record is removed,
    /// its queued events dropped untold and handed back, and its thread
    /// exits at its next take, a running check's tell dropped.
    pub fn ended(&mut self, pid: Pid) -> (Vec<E>, Vec<W>) {
        let dropped = self
            .records
            .remove(&pid)
            .map(|record| record.queue.into_iter().map(|(_, event)| event).collect())
            .unwrap_or_default();
        (dropped, self.waiters_due())
    }

    /// The listener shuts down (column E): every queued event is dropped
    /// untold and handed back, every thread exits at its next take, one
    /// inside a call abandoned rather than waited for, and the idle
    /// barrier's waiters are dropped unrun. Returns the dropped events and
    /// the applications whose thread is abandoned inside a check, each
    /// with the number its checked event was pushed under and its kind.
    pub fn stop(&mut self) -> (Vec<E>, Vec<(Pid, u64, EventKind)>) {
        self.stopping = true;
        self.waiters.clear();
        let mut dropped = Vec::new();
        let mut abandoned = Vec::new();
        for (pid, record) in std::mem::take(&mut self.records) {
            if let Thread::Checking(Some((number, kind))) = record.thread {
                abandoned.push((pid, number, kind));
            }
            dropped.extend(record.queue.into_iter().map(|(_, event)| event));
        }
        (dropped, abandoned)
    }

    /// The idle barrier's `waiter`: handed back to run now when no event
    /// pushed before it waits or is checked, else kept until none does.
    /// Dropped once the listener is stopping.
    pub fn when_idle(&mut self, waiter: W) -> Option<W> {
        if self.stopping {
            return None;
        }
        if self.before_done(self.pushed) {
            return Some(waiter);
        }
        self.waiters.push_back((self.pushed, waiter));
        None
    }

    /// Whether no event pushed under `number` or before waits or is being
    /// checked.
    fn before_done(&self, number: u64) -> bool {
        self.records.values().all(|record| {
            let waiting = record
                .queue
                .front()
                .is_some_and(|&(queued, _)| queued <= number);
            let checking = matches!(record.thread, Thread::Checking(Some((at, _))) if at <= number);
            !waiting && !checking
        })
    }

    /// The waiters whose events are all done, in the order they came.
    fn waiters_due(&mut self) -> Vec<W> {
        let mut due = Vec::new();
        while let Some(&(number, _)) = self.waiters.front() {
            if !self.before_done(number) {
                break;
            }
            if let Some((_, waiter)) = self.waiters.pop_front() {
                due.push(waiter);
            }
        }
        due
    }

    /// Whether no event waits or is being checked.
    #[must_use]
    pub fn idle(&self) -> bool {
        self.before_done(self.pushed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Threads = CheckThreads<&'static str, u32>;

    /// A test event named "menu..." is a menu's opening, any other a focus.
    impl Kinded for &'static str {
        fn kind(&self) -> EventKind {
            if self.starts_with("menu") {
                EventKind::UiaMenuOpened
            } else {
                EventKind::UiaFocus
            }
        }
    }

    const APP: Pid = Pid(10);
    const OTHER: Pid = Pid(11);

    /// The first event of an application starts its thread; the thread
    /// takes it, and one pushed during its check waits behind it, in
    /// order.
    #[test]
    fn an_applications_events_are_checked_in_order_on_one_thread() {
        let mut threads = Threads::default();
        assert_eq!(threads.push(APP, "a"), Pushed::Start { generation: 0 });
        assert_eq!(threads.take(APP, 0, 0), Take::Check("a"));
        assert_eq!(threads.push(APP, "b"), Pushed::Queued);
        assert_eq!(threads.push(APP, "c"), Pushed::Queued);
        assert_eq!(threads.checked(APP, 0), (true, Vec::new()));
        assert_eq!(threads.take(APP, 0, 1), Take::Check("b"));
        assert_eq!(threads.checked(APP, 0), (true, Vec::new()));
        assert_eq!(threads.take(APP, 0, 2), Take::Check("c"));
        // Another application's event starts its own thread.
        assert_eq!(threads.push(OTHER, "x"), Pushed::Start { generation: 1 });
    }

    /// An idle thread is woken by the next event, and exits by removing
    /// its record once the idle time passes with nothing pushed; the next
    /// event then starts a new thread.
    #[test]
    fn an_idle_thread_is_woken_or_exits_after_the_idle_time() {
        let mut threads = Threads::default();
        let _ = threads.push(APP, "a");
        let _ = threads.take(APP, 0, 0);
        let _ = threads.checked(APP, 0);
        assert_eq!(
            threads.take(APP, 0, 100),
            Take::Wait {
                until_us: 100 + IDLE_US
            }
        );
        assert_eq!(threads.push(APP, "b"), Pushed::Wake);
        assert_eq!(threads.take(APP, 0, 200), Take::Check("b"));
        let _ = threads.checked(APP, 0);
        assert_eq!(
            threads.take(APP, 0, 300),
            Take::Wait {
                until_us: 300 + IDLE_US
            }
        );
        // A spurious wake before the idle time waits again to the same end.
        assert_eq!(
            threads.take(APP, 0, 400),
            Take::Wait {
                until_us: 300 + IDLE_US
            }
        );
        assert_eq!(threads.take(APP, 0, 300 + IDLE_US), Take::Exit);
        assert_eq!(threads.push(APP, "c"), Pushed::Start { generation: 1 });
        assert_eq!(
            threads.take(APP, 0, 0),
            Take::Exit,
            "the old thread's record is gone"
        );
        assert_eq!(threads.take(APP, 1, 0), Take::Check("c"));
    }

    /// The application's end drops its queued events untold, and the
    /// running check's tell; its thread exits; a new process under the
    /// same id gets a thread of its own.
    #[test]
    fn the_applications_end_drops_its_queue_and_its_running_checks_tell() {
        let mut threads = Threads::default();
        let _ = threads.push(APP, "a");
        let _ = threads.take(APP, 0, 0);
        let _ = threads.push(APP, "b");
        assert_eq!(threads.ended(APP), (vec!["b"], Vec::new()));
        assert_eq!(threads.checked(APP, 0), (false, Vec::new()));
        assert_eq!(threads.take(APP, 0, 1), Take::Exit);
        assert_eq!(threads.push(APP, "c"), Pushed::Start { generation: 1 });
        // The old thread's late end does not touch the new record.
        assert_eq!(threads.checked(APP, 0), (false, Vec::new()));
        assert_eq!(threads.take(APP, 1, 2), Take::Check("c"));
    }

    /// At shutdown every queued event is dropped untold, every thread
    /// exits, a check in hand places no tell and its thread is named as
    /// abandoned, and nothing more is taken.
    #[test]
    fn shutdown_drops_every_queue_and_ends_every_thread() {
        let mut threads = Threads::default();
        let _ = threads.push(APP, "menu");
        let _ = threads.take(APP, 0, 0);
        let _ = threads.push(APP, "b");
        let _ = threads.push(OTHER, "x");
        let (mut dropped, abandoned) = threads.stop();
        dropped.sort_unstable();
        assert_eq!(dropped, ["b", "x"]);
        // Only APP's thread is inside a check, of the menu's opening pushed
        // first; OTHER's thread has not yet taken its event.
        assert_eq!(abandoned, [(APP, 1, EventKind::UiaMenuOpened)]);
        assert_eq!(threads.checked(APP, 0), (false, Vec::new()));
        assert_eq!(threads.take(APP, 0, 1), Take::Exit);
        assert_eq!(threads.push(APP, "c"), Pushed::Dropped("c"));
        assert_eq!(threads.when_idle(1), None);
    }

    /// The idle barrier's waiter runs once every event pushed before it is
    /// told or dropped, and never waits for one pushed after it.
    #[test]
    fn the_idle_waiter_waits_for_the_events_before_it_alone() {
        let mut threads = Threads::default();
        assert_eq!(threads.when_idle(1), Some(1));
        let _ = threads.push(APP, "a");
        let _ = threads.push(OTHER, "x");
        assert_eq!(threads.when_idle(2), None);
        let _ = threads.take(APP, 0, 0);
        let _ = threads.push(APP, "b");
        assert_eq!(threads.checked(APP, 0), (true, Vec::new()));
        let _ = threads.take(APP, 0, 1);
        // The other application's end drops its event: the waiter's last.
        assert_eq!(threads.ended(OTHER), (vec!["x"], vec![2]));
        assert!(!threads.idle());
        assert_eq!(threads.checked(APP, 0), (true, Vec::new()));
        assert!(threads.idle());
    }
}
