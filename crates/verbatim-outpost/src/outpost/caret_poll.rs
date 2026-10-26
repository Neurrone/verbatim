//! The console host's caret poll: a documented workaround to waiting only
//! for evidence (`docs/design/key-holding.md`, "The console host's caret
//! poll"). The console host raises its caret events only on its cursor's
//! 530 ms blink timer, so a caret key's watch there that finds no move on
//! its first check re-reads the caret every [`INTERVAL`] until the caret
//! has moved, the console's caret event arrives, or [`BOUND`] after the
//! key was passed on (Dickson, 2026-10-26).
//!
//! The worker makes the reads; this thread only ends the worker's wait for
//! work at each tick, on a high-resolution waitable timer, since the
//! intake's own wait keeps the system's timer resolution, 15.6 ms by
//! default. The timer is set only while a watch polls, so the thread wakes
//! for nothing else.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::Context;
use super::alarm::Alarm;

/// How long after one of the poll's reads the next is made.
pub(super) const INTERVAL: Duration = Duration::from_millis(5);

/// How long after the key was passed on the poll's last read is made; a
/// watch that has found no evidence by then ends, as at its own bound.
pub(super) const BOUND: Duration = Duration::from_millis(50);

/// A caret key's watch's poll, kept in the watch it belongs to.
#[derive(Clone, Copy, Debug)]
pub(super) struct Poll {
    /// When the next read is due.
    pub(super) next: Instant,
    /// When the last read is due.
    pub(super) until: Instant,
}

impl Poll {
    /// The poll of a watch opened at `opened` for a key passed on at
    /// `passed_at_ms`, by the clock of `now_ms` (0 when unknown, taken as
    /// `opened`).
    pub(super) fn new(opened: Instant, passed_at_ms: u64, now_ms: u64) -> Self {
        let until = if passed_at_ms == 0 {
            opened + BOUND
        } else {
            let bound_ms = u64::try_from(BOUND.as_millis()).unwrap_or(u64::MAX);
            opened + Duration::from_millis((passed_at_ms + bound_ms).saturating_sub(now_ms))
        };
        Self {
            next: (opened + INTERVAL).min(until),
            until,
        }
    }

    /// The poll after a read made at `now` found no evidence, or `None`
    /// when that read was its last.
    pub(super) fn after_read(self, now: Instant) -> Option<Self> {
        (now < self.until).then(|| Self {
            next: (now + INTERVAL).min(self.until),
            until: self.until,
        })
    }
}

/// The poll's timer and the thread that waits on it.
#[derive(Default)]
pub(super) struct Ticker {
    alarm: Alarm,
    stopping: AtomicBool,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Ticker {
    /// Starts the thread that gives the worker [`Item::Wake`] at each
    /// tick.
    ///
    /// [`Item::Wake`]: super::intake::Item::Wake
    pub(super) fn start(context: &Arc<Context>) {
        let ticked = Arc::clone(context);
        let thread = thread::Builder::new()
            .name("verbatim-caret-poll".to_owned())
            .stack_size(64 * 1024)
            .spawn(move || {
                loop {
                    ticked.caret_poll.alarm.wait();
                    if ticked.caret_poll.stopping.load(Ordering::Acquire) {
                        return;
                    }
                    ticked.intake.wake_at(Some(Instant::now()));
                }
            })
            .expect("spawn the caret poll's thread");
        *context
            .caret_poll
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(thread);
    }

    /// Ends the thread and waits for it.
    pub(super) fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.alarm.wake();
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }

    /// Sets the next tick for `at`, replacing any set before.
    pub(super) fn set(&self, at: Instant) {
        self.alarm.set(at);
    }

    /// Cancels the next tick, if one is set.
    pub(super) fn cancel(&self) {
        self.alarm.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_poll_ends_its_bound_after_the_key_was_passed_on() {
        let opened = Instant::now();
        // Opened 3 ms after the key was passed on.
        let poll = Poll::new(opened, 1_000, 1_003);
        assert_eq!(poll.until, opened + Duration::from_millis(47));
        assert_eq!(poll.next, opened + INTERVAL);
    }

    #[test]
    fn a_poll_without_the_keys_time_ends_its_bound_after_it_opened() {
        let opened = Instant::now();
        let poll = Poll::new(opened, 0, 1_003);
        assert_eq!(poll.until, opened + BOUND);
    }

    #[test]
    fn each_read_is_followed_by_the_next_until_the_bound() {
        let opened = Instant::now();
        let poll = Poll::new(opened, 1_000, 1_000);
        let read_at = opened + Duration::from_millis(47);
        let next = poll.after_read(read_at).expect("a read before the bound");
        assert_eq!(next.next, opened + BOUND);
        assert!(next.after_read(opened + BOUND).is_none());
    }
}
