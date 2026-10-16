//! The supervisor's pure decisions, kept apart from the I/O that gathers
//! their inputs so each is unit-tested on its own: when an outpost is stuck,
//! when an idle one is retired, how long a failed outpost's replacement
//! waits, how facts that arrive during a spawn are held, and which spare outposts are kept
//! and given.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use verbatim_model::{OutpostId, TraceId};

use crate::protocol::DeliveredFact;

/// Why the owner killed an otherwise-alive outpost (recovery ladder rung 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WedgeReason {
    /// No pong for [`MISSED_PONG_THRESHOLD`](super::MISSED_PONG_THRESHOLD)
    /// consecutive ping intervals: the outpost stopped answering.
    MissedHeartbeats,
    /// The most recent pong reported at least
    /// [`ABANDONED_WORKER_LIMIT`](super::ABANDONED_WORKER_LIMIT) abandoned
    /// workers.
    AbandonedWorkers,
}

impl std::fmt::Display for WedgeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            WedgeReason::MissedHeartbeats => "missed heartbeats",
            WedgeReason::AbandonedWorkers => "abandoned workers",
        })
    }
}

/// Whether an outpost should be killed, and why: given its last pong time,
/// now, and its last reported count of abandoned workers. Abandoned workers
/// are checked first, as the more specific diagnosis, and do not count while
/// the application's own windows are reported hung, since a replacement
/// outpost would hang the same way.
pub(super) fn wedge_decision(
    last_pong_at: Instant,
    now: Instant,
    ping_interval: Duration,
    missed_pong_threshold: u32,
    abandoned: usize,
    abandoned_limit: usize,
    application_hung: bool,
) -> Option<WedgeReason> {
    if abandoned >= abandoned_limit && !application_hung {
        return Some(WedgeReason::AbandonedWorkers);
    }
    if now.saturating_duration_since(last_pong_at) >= ping_interval * missed_pong_threshold {
        return Some(WedgeReason::MissedHeartbeats);
    }
    None
}

/// Whether an outpost is retired: its application has not held attention for
/// `idle_after`, and Core holds none of its nodes. Core's own outpost is
/// never retired, which the caller checks.
pub(super) fn retirement_decision(
    last_attention_at: Instant,
    now: Instant,
    idle_after: Duration,
    holds_attention: bool,
    nodes_held: bool,
) -> bool {
    !holds_attention
        && !nodes_held
        && now.saturating_duration_since(last_attention_at) >= idle_after
}

/// The backoff between an outpost's failures (`docs/design/focus-pipeline.md`,
/// section 19.5): one application's outposts that fail while serving it, or
/// the spares that fail while they wait. The first failure is replaced at
/// once; each further one waits twice as long as the one before, from
/// [`BACKOFF_FIRST`](super::BACKOFF_FIRST) up to
/// [`BACKOFF_LIMIT`](super::BACKOFF_LIMIT). Nothing is ever given up. The
/// count starts again once what failed had stayed up for
/// [`STAYED_UP`](super::STAYED_UP), and, for an application, when it comes
/// to the foreground, which also ends a wait that runs
/// ([`foreground`](Self::foreground)).
#[derive(Debug, Default)]
pub(super) struct Backoff {
    /// The failures counted since the count last started again.
    failures: u32,
    /// When the replacement is given or started, while a wait runs.
    due: Option<Instant>,
}

impl Backoff {
    /// Records, at `now`, a failure of an outpost that had been up for
    /// `up`, and returns how long its replacement waits: none for the first
    /// failure since the count started again, then `first`, doubling up to
    /// `limit`; the wait runs until [`due`](Self::due). A failure of one
    /// that had stayed up for `stayed_up` starts the count again.
    pub(super) fn failed(
        &mut self,
        up: Duration,
        now: Instant,
        (first, limit, stayed_up): (Duration, Duration, Duration),
    ) -> Duration {
        if up >= stayed_up {
            self.failures = 0;
        }
        self.failures = self.failures.saturating_add(1);
        let wait = match self.failures {
            1 => Duration::ZERO,
            failures => {
                // 2^(failures - 2) times `first`, stopped at `limit` before
                // it can overflow.
                let doublings = (failures - 2).min(31);
                first.saturating_mul(1 << doublings).min(limit)
            }
        };
        self.due = (!wait.is_zero()).then(|| now + wait);
        wait
    }

    /// When the replacement is given or started, while a wait runs.
    pub(super) fn due(&self) -> Option<Instant> {
        self.due
    }

    /// Whether the wait that ran has ended at `now`: once it says so, no
    /// wait runs.
    pub(super) fn ended(&mut self, now: Instant) -> bool {
        let ended = self.due.is_some_and(|due| now >= due);
        if ended {
            self.due = None;
        }
        ended
    }

    /// The application came to the foreground: the count starts again,
    /// and a wait that runs ends now. Returns whether one ran.
    pub(super) fn foreground(&mut self) -> bool {
        self.failures = 0;
        self.due.take().is_some()
    }
}

/// A focus fact routed to an outpost, with the trace and observation time
/// the listener stamped.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct HeldFact {
    pub(super) trace_id: TraceId,
    pub(super) observed_at_ms: u64,
    pub(super) timing: crate::protocol::EventTiming,
    pub(super) fact: DeliveredFact,
    /// For an MSAA `WinEvent`'s fact, the thread that raised it and when.
    pub(super) win_event: Option<crate::protocol::WinEventStamp>,
    /// When verbatim.exe read it from the listener.
    pub(super) received: super::writer::Received,
}

/// Facts that arrive while an outpost is starting, held in arrival order and
/// released in that order once it is ready. A newer fact for the same object
/// and kind replaces the older one and takes its place at the back, NVDA's
/// limiter rule, so the release order is the order in which the surviving
/// facts last happened.
#[derive(Debug, Default)]
pub(super) struct HeldFacts {
    facts: Vec<HeldFact>,
    /// When the first fact was held, until they are taken.
    first_held_at: Option<Instant>,
}

impl HeldFacts {
    /// Holds `fact`, replacing any older fact for the same object and kind.
    pub(super) fn hold(&mut self, fact: HeldFact) {
        if let Some(key) = fact.fact.key() {
            self.facts
                .retain(|held| held.fact.key().as_ref() != Some(&key));
        }
        self.first_held_at.get_or_insert_with(Instant::now);
        self.facts.push(fact);
    }

    /// When the first fact still held, or taken since, was held: how long
    /// the application's first fact has waited for its outpost.
    pub(super) fn first_held_at(&self) -> Option<Instant> {
        self.first_held_at
    }

    /// Takes every held fact, in release order.
    pub(super) fn take(&mut self) -> Vec<HeldFact> {
        self.first_held_at = None;
        std::mem::take(&mut self.facts)
    }
}

/// The spare outposts (`docs/design/focus-pipeline.md`, section 19):
/// whether they are kept yet, which wait, oldest first, and whether each has
/// said it is ready. A spare given to an application never returns to the
/// list, and its replacement is wanted at once. A spare that fails while it
/// waits is always replaced, its replacement's start spaced from the
/// failures before it by the spares' [`Backoff`].
#[derive(Debug, Default)]
pub(super) struct Spares {
    /// Whether spares are kept: from the moment Core's own outpost is
    /// ready, or has ended without becoming ready, until Verbatim exits.
    kept: bool,
    /// The spares waiting, oldest first, each with whether it is ready.
    waiting: VecDeque<(OutpostId, bool)>,
    /// The failures of spares while they waited, and the wait one's
    /// replacement has before it starts.
    backoff: Backoff,
}

impl Spares {
    /// Starts keeping spares, unless they are kept already. Returns
    /// whether this call started it.
    pub(super) fn keep(&mut self) -> bool {
        !std::mem::replace(&mut self.kept, true)
    }

    /// How many spares to start at `now` so that `count` wait: none while
    /// spares are not kept, or a failure's backoff runs.
    pub(super) fn wanted(&mut self, count: usize, now: Instant) -> usize {
        if !self.kept {
            return 0;
        }
        if self.backoff.due().is_some() && !self.backoff.ended(now) {
            return 0;
        }
        count.saturating_sub(self.waiting.len())
    }

    /// When the next spare may start, while a failure's backoff runs.
    pub(super) fn due(&self) -> Option<Instant> {
        self.backoff.due()
    }

    /// Lists a spare just started, at the back.
    pub(super) fn add(&mut self, spare: OutpostId) {
        self.waiting.push_back((spare, false));
    }

    /// Whether `spare` is a spare waiting.
    #[cfg(test)]
    fn contains(&self, spare: OutpostId) -> bool {
        self.waiting.iter().any(|&(waiting, _)| waiting == spare)
    }

    /// Notes that `spare` said it is ready, and returns how many waiting
    /// spares are ready now; `None` when it is not waiting.
    pub(super) fn ready(&mut self, spare: OutpostId) -> Option<usize> {
        let entry = self
            .waiting
            .iter_mut()
            .find(|(waiting, _)| *waiting == spare)?;
        entry.1 = true;
        Some(self.waiting.iter().filter(|(_, ready)| *ready).count())
    }

    /// Gives the oldest spare waiting, ready or not, while spares are
    /// kept.
    pub(super) fn take(&mut self) -> Option<OutpostId> {
        if !self.kept {
            return None;
        }
        self.waiting.pop_front().map(|(spare, _)| spare)
    }

    /// Takes `spare` off the list: it has ended while waiting. Returns
    /// whether it was listed.
    pub(super) fn remove(&mut self, spare: OutpostId) -> bool {
        let before = self.waiting.len();
        self.waiting.retain(|&(waiting, _)| waiting != spare);
        self.waiting.len() != before
    }

    /// A spare that had been up for `up` failed while it waited, at `now`:
    /// its replacement starts at once, or when the backoff
    /// ([`Backoff::failed`], with `times`) says, which this returns.
    pub(super) fn failed(
        &mut self,
        up: Duration,
        now: Instant,
        times: (Duration, Duration, Duration),
    ) -> Duration {
        self.backoff.failed(up, now, times)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PING: Duration = Duration::from_secs(3);

    #[test]
    fn a_recent_pong_with_few_abandoned_workers_is_healthy() {
        let start = Instant::now();
        assert_eq!(
            wedge_decision(start, start + Duration::from_secs(1), PING, 3, 7, 8, false),
            None
        );
    }

    #[test]
    fn nine_seconds_without_a_pong_is_a_wedge() {
        let start = Instant::now();
        let just_before = (start + PING * 3)
            .checked_sub(Duration::from_millis(1))
            .expect("computable");
        assert_eq!(
            wedge_decision(start, just_before, PING, 3, 0, 8, false),
            None
        );
        assert_eq!(
            wedge_decision(start, start + PING * 3, PING, 3, 0, 8, false),
            Some(WedgeReason::MissedHeartbeats)
        );
    }

    #[test]
    fn eight_abandoned_workers_are_a_wedge_unless_the_application_is_hung() {
        let start = Instant::now();
        assert_eq!(
            wedge_decision(start, start, PING, 3, 8, 8, false),
            Some(WedgeReason::AbandonedWorkers)
        );
        assert_eq!(wedge_decision(start, start, PING, 3, 8, 8, true), None);
        // A hung application does not excuse an outpost that stopped
        // answering pings altogether.
        assert_eq!(
            wedge_decision(start, start + PING * 3, PING, 3, 8, 8, true),
            Some(WedgeReason::MissedHeartbeats)
        );
    }

    #[test]
    fn a_now_before_the_last_pong_never_panics() {
        let start = Instant::now();
        let earlier = start.checked_sub(Duration::from_secs(5)).unwrap_or(start);
        assert_eq!(wedge_decision(start, earlier, PING, 3, 0, 8, false), None);
    }

    #[test]
    fn retirement_needs_two_idle_minutes_without_attention_or_held_nodes() {
        let start = Instant::now();
        let idle = Duration::from_mins(2);
        let later = start + idle;
        assert!(retirement_decision(start, later, idle, false, false));
        assert!(!retirement_decision(start, later, idle, true, false));
        assert!(!retirement_decision(start, later, idle, false, true));
        assert!(!retirement_decision(
            start,
            start + Duration::from_secs(119),
            idle,
            false,
            false
        ));
    }

    /// The backoff's times as the supervisor sets them: 1 s first, 30 s
    /// at most, starting again after 60 s up.
    const TIMES: (Duration, Duration, Duration) = (
        Duration::from_secs(1),
        Duration::from_secs(30),
        Duration::from_secs(60),
    );

    #[test]
    fn repeated_failures_wait_one_two_four_seconds_up_to_thirty_and_never_give_up() {
        let now = Instant::now();
        let mut backoff = Backoff::default();
        let young = Duration::from_secs(1);
        let waits: Vec<u64> = (0..9)
            .map(|_| backoff.failed(young, now, TIMES).as_secs())
            .collect();
        assert_eq!(waits, [0, 1, 2, 4, 8, 16, 30, 30, 30]);
        for _ in 0..100 {
            assert_eq!(backoff.failed(young, now, TIMES), Duration::from_secs(30));
        }
        assert_eq!(backoff.due(), Some(now + Duration::from_secs(30)));
    }

    #[test]
    fn a_wait_runs_until_it_is_due() {
        let now = Instant::now();
        let mut backoff = Backoff::default();
        let young = Duration::from_secs(1);
        assert_eq!(backoff.failed(young, now, TIMES), Duration::ZERO);
        assert_eq!(backoff.due(), None, "the first is replaced at once");
        assert_eq!(backoff.failed(young, now, TIMES), Duration::from_secs(1));
        let due = now + Duration::from_secs(1);
        assert_eq!(backoff.due(), Some(due));
        let just_before = due
            .checked_sub(Duration::from_millis(1))
            .expect("computable");
        assert!(!backoff.ended(just_before));
        assert!(backoff.ended(due));
        assert_eq!(backoff.due(), None);
        assert!(!backoff.ended(due), "ended once");
    }

    #[test]
    fn the_count_starts_again_after_a_minute_up() {
        let now = Instant::now();
        let mut backoff = Backoff::default();
        let young = Duration::from_secs(59);
        assert_eq!(backoff.failed(young, now, TIMES), Duration::ZERO);
        assert_eq!(backoff.failed(young, now, TIMES), Duration::from_secs(1));
        assert_eq!(backoff.failed(young, now, TIMES), Duration::from_secs(2));
        // One that stayed up a minute fails as the first.
        assert_eq!(
            backoff.failed(Duration::from_secs(60), now, TIMES),
            Duration::ZERO
        );
        assert_eq!(backoff.due(), None);
        assert_eq!(backoff.failed(young, now, TIMES), Duration::from_secs(1));
    }

    #[test]
    fn the_foreground_starts_the_count_again_and_ends_a_wait_that_runs() {
        let now = Instant::now();
        let mut backoff = Backoff::default();
        let young = Duration::from_secs(1);
        for _ in 0..3 {
            let _ = backoff.failed(young, now, TIMES);
        }
        assert_eq!(backoff.due(), Some(now + Duration::from_secs(2)));
        assert!(backoff.foreground(), "a wait ran");
        assert_eq!(backoff.due(), None);
        assert!(!backoff.foreground(), "none runs now");
        assert_eq!(backoff.failed(young, now, TIMES), Duration::ZERO);
        assert_eq!(backoff.failed(young, now, TIMES), Duration::from_secs(1));
    }

    #[test]
    fn a_failed_spares_replacement_waits_for_its_backoff() {
        let now = Instant::now();
        let mut spares = kept(2);
        let young = Duration::from_secs(1);
        assert!(spares.remove(OutpostId(1)));
        assert_eq!(spares.failed(young, now, TIMES), Duration::ZERO);
        assert_eq!(spares.due(), None);
        assert_eq!(spares.wanted(2, now), 1, "the first failure's at once");
        spares.add(OutpostId(3));
        assert!(spares.remove(OutpostId(3)));
        assert_eq!(spares.failed(young, now, TIMES), Duration::from_secs(1));
        let due = now + Duration::from_secs(1);
        assert_eq!(spares.due(), Some(due));
        let just_before = due
            .checked_sub(Duration::from_millis(1))
            .expect("computable");
        assert_eq!(spares.wanted(2, just_before), 0, "not before its backoff");
        assert_eq!(spares.wanted(2, due), 1);
        assert_eq!(spares.due(), None);
    }

    fn held(observed_at_ms: u64, fact: DeliveredFact) -> HeldFact {
        HeldFact {
            trace_id: TraceId::mint(),
            observed_at_ms,
            timing: crate::protocol::EventTiming::default(),
            fact,
            win_event: None,
            received: crate::supervisor::writer::Received::now(),
        }
    }

    fn msaa_focus(hwnd: isize) -> DeliveredFact {
        DeliveredFact::MsaaFocus {
            hwnd,
            id_object: -4,
            id_child: 0,
        }
    }

    /// Spares kept, with `count` started, numbered from 1.
    fn kept(count: u64) -> Spares {
        let mut spares = Spares::default();
        assert!(spares.keep());
        for id in 1..=count {
            spares.add(OutpostId(id));
        }
        spares
    }

    #[test]
    fn no_spare_is_started_or_given_before_they_are_kept() {
        let now = Instant::now();
        let mut spares = Spares::default();
        assert_eq!(spares.wanted(2, now), 0);
        spares.add(OutpostId(1));
        assert_eq!(spares.take(), None);
        assert!(spares.keep());
        assert!(!spares.keep(), "kept once");
        assert_eq!(spares.wanted(2, now), 1);
    }

    #[test]
    fn the_oldest_spare_is_given_ready_or_not_and_never_listed_again() {
        let mut spares = kept(2);
        assert_eq!(
            spares.ready(OutpostId(2)),
            Some(1),
            "the newer one is ready"
        );
        assert_eq!(spares.take(), Some(OutpostId(1)), "the older, not ready");
        assert!(!spares.contains(OutpostId(1)));
        assert_eq!(
            spares.ready(OutpostId(1)),
            None,
            "a given spare is not listed"
        );
        assert!(spares.contains(OutpostId(2)));
        assert_eq!(spares.take(), Some(OutpostId(2)));
        assert_eq!(spares.take(), None);
    }

    #[test]
    fn a_given_spares_replacement_is_wanted_at_once() {
        let mut spares = kept(2);
        let _ = spares.take().expect("a spare");
        assert_eq!(spares.wanted(2, Instant::now()), 1);
    }

    #[test]
    fn a_spare_that_ends_waiting_is_replaced() {
        let mut spares = kept(2);
        assert!(spares.remove(OutpostId(1)));
        assert!(!spares.remove(OutpostId(1)), "removed once");
        assert_eq!(spares.wanted(2, Instant::now()), 1);
    }

    #[test]
    fn held_facts_release_in_arrival_order_and_a_repeat_moves_to_the_back() {
        let mut facts = HeldFacts::default();
        facts.hold(held(1, DeliveredFact::Foreground { hwnd: 7 }));
        facts.hold(held(2, msaa_focus(8)));
        facts.hold(held(
            3,
            DeliveredFact::MenuPopup {
                hwnd: 9,
                id_object: -4,
                id_child: 0,
            },
        ));
        // Focus returns to the same object after the menu: the older entry
        // is replaced and the newer one is released last.
        facts.hold(held(4, msaa_focus(8)));
        let released: Vec<u64> = facts
            .take()
            .iter()
            .map(|fact| fact.observed_at_ms)
            .collect();
        assert_eq!(released, vec![1, 3, 4]);
        assert!(facts.take().is_empty(), "nothing is left held");
    }
}
