//! The console host's caret poll: a documented workaround to waiting only
//! for evidence (`docs/design/key-holding.md`, "The console host's caret
//! poll"). The console host raises its caret events only on its cursor's
//! 530 ms blink timer, so a caret key's watch there that finds no move on
//! its first check re-reads the caret every [`INTERVAL`] until a check
//! answers the key, its own or one an event makes, the console's caret
//! event included, or [`BOUND`] after the key was passed on (Dickson,
//! 2026-10-26). An event whose check finds no evidence leaves the poll
//! reading: a blink tick left over from an earlier cursor set can come
//! before the key's move.
//!
//! The worker makes the reads, on [`Item::Wake`]: it asks the intake to
//! wake it at the open watch's next read ([`wake_for`]), and the intake's
//! own wait for work ends then, on a high-resolution timer, since a wait
//! at the system's timer resolution, 15.6 ms by default, would miss most
//! reads. No other thread is involved, so each read costs one wake-up of
//! the worker, and the timer is not set for the poll while no watch polls.
//!
//! [`Item::Wake`]: super::intake::Item::Wake

use std::time::{Duration, Instant};

use super::text_reads::{CARET_WATCH_BOUND, OpenWatch};

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

/// When the worker is to be woken for `open`: at its poll's next read, or
/// at its bound ([`CARET_WATCH_BOUND`]) when it does not poll.
pub(super) fn wake_for(open: &OpenWatch) -> Instant {
    let bound = open.opened + CARET_WATCH_BOUND;
    open.poll.map_or(bound, |poll| poll.next.min(bound))
}

/// What a poll read that found no evidence leaves of the watch it was made
/// for ([`after_read`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AfterRead {
    /// That watch is no longer open, or no longer polls: answered, ended or
    /// replaced meanwhile, by this worker or by the one that replaced it
    /// after an abandonment. The watch open now, if any, is left alone.
    Gone,
    /// The watch's next read is set.
    Again,
    /// The read was the poll's last: the watch is to end.
    Last,
}

/// Sets the next read of the watch for request `request_id`, if it is the
/// one in `open`, after a read of it at `now` found no evidence. The watch
/// is matched by its request, not its node: a newer watch on the same node
/// is another key's.
pub(super) fn after_read(open: &mut Option<OpenWatch>, request_id: u64, now: Instant) -> AfterRead {
    let Some(open) = open.as_mut().filter(|open| open.request_id == request_id) else {
        return AfterRead::Gone;
    };
    let Some(poll) = open.poll else {
        return AfterRead::Gone;
    };
    open.poll = poll.after_read(now);
    if open.poll.is_some() {
        AfterRead::Again
    } else {
        AfterRead::Last
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{CallCounts, CaretWatch, NodeId, TextUnit, TraceId};

    use super::*;
    use verbatim_model::protocol::EventTiming;

    /// A watch for request `request_id` on node 1, opened at `opened`,
    /// polling as `poll` says.
    fn watch(request_id: u64, opened: Instant, poll: Option<Poll>) -> OpenWatch {
        OpenWatch {
            request_id,
            trace: TraceId::mint(),
            node_id: NodeId::new(1),
            watch: CaretWatch {
                since: None,
                pressed_at_ms: 0,
                unit: TextUnit::Character,
                compare: None,
                landing: false,
                deletes: false,
            },
            opened,
            timing: EventTiming::default(),
            calls: CallCounts::default(),
            poll,
        }
    }

    #[test]
    fn a_polling_watch_wakes_the_worker_at_its_next_read() {
        let opened = Instant::now();
        let poll = Poll::new(opened, 0, 0);
        assert_eq!(wake_for(&watch(7, opened, Some(poll))), opened + INTERVAL);
        assert_eq!(
            wake_for(&watch(7, opened, None)),
            opened + CARET_WATCH_BOUND
        );
    }

    #[test]
    fn a_read_for_a_replaced_watch_leaves_the_newer_one_on_its_node_alone() {
        let opened = Instant::now();
        let newer = Poll::new(opened, 0, 0);
        let mut open = Some(watch(8, opened, Some(newer)));
        // The read for request 7, the older key's, returns after the newer
        // key's watch on the same node opened, at that watch's bound.
        assert_eq!(after_read(&mut open, 7, opened + BOUND), AfterRead::Gone);
        let open = open.expect("the newer watch stays open");
        assert_eq!(open.poll.map(|poll| poll.next), Some(newer.next));
    }

    #[test]
    fn a_read_at_the_bound_is_the_watchs_last() {
        let opened = Instant::now();
        let mut open = Some(watch(7, opened, Some(Poll::new(opened, 0, 0))));
        assert_eq!(
            after_read(&mut open, 7, opened + INTERVAL),
            AfterRead::Again
        );
        assert_eq!(after_read(&mut open, 7, opened + BOUND), AfterRead::Last);
    }

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
