//! The supervisor's pure decisions, kept apart from the I/O that gathers
//! their inputs so each is unit-tested on its own: when an outpost is stuck,
//! how long a failed outpost's replacement
//! waits, and which spare outposts are kept and given.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::Duration;

use verbatim_model::OutpostId;

/// Why the owner killed an otherwise-alive child (recovery ladder rung 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WedgeReason {
    /// No pong for the supervisor's `MISSED_PONG_THRESHOLD`
    /// consecutive ping intervals: the child stopped answering.
    MissedHeartbeats,
}

impl std::fmt::Display for WedgeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            WedgeReason::MissedHeartbeats => "missed heartbeats",
        })
    }
}

/// The supervisor's limits for judging a child stuck: how often it is
/// pinged and how many pings in a row it may miss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WedgeLimits {
    /// The time between pings.
    pub ping_interval: Duration,
    /// How many ping intervals may pass with no pong.
    pub missed_pongs: u32,
}

/// Microseconds from `earlier` to `later` on the run's shared clock, none
/// when `later` is before it.
fn since_us(earlier: u64, later: u64) -> Duration {
    Duration::from_micros(later.saturating_sub(earlier))
}

/// `at_us` moved on by `wait`, on the run's shared clock.
fn after_us(at_us: u64, wait: Duration) -> u64 {
    at_us.saturating_add(u64::try_from(wait.as_micros()).unwrap_or(u64::MAX))
}

/// A child's liveness judged by its pongs alone: the focus listener's, or
/// a spare outpost's while it waits. An application's outpost is judged
/// by Core's generation record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Liveness {
    /// When it last answered a ping, or was launched, on the run's shared
    /// clock.
    last_pong_us: u64,
}

impl Liveness {
    /// A child launched, or running, at `now_us`.
    #[must_use]
    pub fn new(now_us: u64) -> Self {
        Self {
            last_pong_us: now_us,
        }
    }

    /// It answered a ping at `at_us`.
    pub fn ponged(&mut self, at_us: u64) {
        self.last_pong_us = at_us;
    }

    /// Whether it has missed its pings at `now_us`, in microseconds on
    /// the run's shared clock.
    #[must_use]
    pub fn wedged(&self, now_us: u64, limits: WedgeLimits) -> Option<WedgeReason> {
        (since_us(self.last_pong_us, now_us)
            >= limits.ping_interval.saturating_mul(limits.missed_pongs))
        .then_some(WedgeReason::MissedHeartbeats)
    }
}

/// The backoff between an outpost's failures (`books/dev/src/reference/adr/0001-focus.md`, "Spare
/// outposts"): one application's outposts that fail while serving it, or
/// the spares that fail while they wait. The first failure is replaced at
/// once; each further one waits twice as long as the one before, from
/// the first of its backoff (the supervisor's `Timings::backoff_first`) up
/// to its limit (`Timings::backoff_limit`). Nothing is ever given up. The
/// count starts again once what failed had stayed up for
/// the time it must stay up (`STAYED_UP`), and, for an application, when it comes
/// to the foreground, which also ends a wait that runs
/// ([`foreground`](Self::foreground)).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Backoff {
    /// The failures counted since the count last started again.
    failures: u32,
    /// When the replacement is given or started, while a wait runs, on
    /// the run's shared clock.
    due_us: Option<u64>,
}

impl Backoff {
    /// Records, at `now_us`, a failure of an outpost that had been up for
    /// `up`, and returns how long its replacement waits: none for the first
    /// failure since the count started again, then `first`, doubling up to
    /// `limit`; the wait runs until [`due`](Self::due). A failure of one
    /// that had stayed up for `stayed_up` starts the count again.
    pub fn failed(
        &mut self,
        up: Duration,
        now_us: u64,
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
                let doublings = failures.saturating_sub(2).min(31);
                first.saturating_mul(1 << doublings).min(limit)
            }
        };
        self.due_us = (!wait.is_zero()).then(|| after_us(now_us, wait));
        wait
    }

    /// When the replacement is given or started, while a wait runs, on
    /// the run's shared clock.
    #[must_use]
    pub fn due(&self) -> Option<u64> {
        self.due_us
    }

    /// Whether the wait that ran has ended at `now_us`: once it says so,
    /// no wait runs.
    pub fn ended(&mut self, now_us: u64) -> bool {
        let ended = self.due_us.is_some_and(|due| now_us >= due);
        if ended {
            self.due_us = None;
        }
        ended
    }

    /// The application came to the foreground: the count starts again,
    /// and a wait that runs ends now. Returns whether one ran.
    pub fn foreground(&mut self) -> bool {
        self.failures = 0;
        self.due_us.take().is_some()
    }
}

/// The spare outposts (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"):
/// whether they are kept yet, which wait, oldest first, and whether each has
/// said it is ready. A spare given to an application never returns to the
/// list, and its replacement is wanted at once. A spare that fails while it
/// waits is always replaced, its replacement's start spaced from the
/// failures before it by the spares' [`Backoff`].
#[derive(Debug, Default)]
pub struct Spares {
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
    pub fn keep(&mut self) -> bool {
        !std::mem::replace(&mut self.kept, true)
    }

    /// How many spares to start at `now_us` so that `count` wait: none
    /// while spares are not kept, or a failure's backoff runs.
    pub fn wanted(&mut self, count: usize, now_us: u64) -> usize {
        if !self.kept {
            return 0;
        }
        if self.backoff.due().is_some() && !self.backoff.ended(now_us) {
            return 0;
        }
        count.saturating_sub(self.waiting.len())
    }

    /// When the next spare may start, while a failure's backoff runs, on
    /// the run's shared clock.
    #[must_use]
    pub fn due(&self) -> Option<u64> {
        self.backoff.due()
    }

    /// Lists a spare just started, at the back.
    pub fn add(&mut self, spare: OutpostId) {
        self.waiting.push_back((spare, false));
    }

    /// Whether `spare` is a spare waiting.
    #[cfg(test)]
    fn contains(&self, spare: OutpostId) -> bool {
        self.waiting.iter().any(|&(waiting, _)| waiting == spare)
    }

    /// Notes that `spare` said it is ready, and returns how many waiting
    /// spares are ready now; `None` when it is not waiting.
    pub fn ready(&mut self, spare: OutpostId) -> Option<usize> {
        let entry = self
            .waiting
            .iter_mut()
            .find(|(waiting, _)| *waiting == spare)?;
        entry.1 = true;
        Some(self.waiting.iter().filter(|(_, ready)| *ready).count())
    }

    /// Gives the oldest spare waiting, ready or not, while spares are
    /// kept.
    pub fn take(&mut self) -> Option<OutpostId> {
        if !self.kept {
            return None;
        }
        self.waiting.pop_front().map(|(spare, _)| spare)
    }

    /// Takes `spare` off the list: it has ended while waiting. Returns
    /// whether it was listed.
    pub fn remove(&mut self, spare: OutpostId) -> bool {
        let before = self.waiting.len();
        self.waiting.retain(|&(waiting, _)| waiting != spare);
        self.waiting.len() != before
    }

    /// A spare that had been up for `up` failed while it waited, at
    /// `now_us`:
    /// its replacement starts at once, or when the backoff
    /// ([`Backoff::failed`], with `times`) says, which this returns.
    pub fn failed(
        &mut self,
        up: Duration,
        now_us: u64,
        times: (Duration, Duration, Duration),
    ) -> Duration {
        self.backoff.failed(up, now_us, times)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The supervisor's limits: a ping every 3 s, 3 missed.
    const LIMITS: WedgeLimits = WedgeLimits {
        ping_interval: Duration::from_secs(3),
        missed_pongs: 3,
    };

    /// A start on the shared clock, in microseconds.
    const START: u64 = 1_000_000_000;
    /// One second, in microseconds.
    const SECOND: u64 = 1_000_000;

    #[test]
    fn a_now_before_the_last_pong_never_panics() {
        let liveness = Liveness::new(START);
        assert_eq!(liveness.wedged(START - 5 * SECOND, LIMITS), None);
    }

    #[test]
    fn a_listener_or_spare_is_wedged_nine_seconds_after_its_last_pong() {
        let mut liveness = Liveness::new(START);
        liveness.ponged(START + 2 * SECOND);
        assert_eq!(liveness.wedged(START + 11 * SECOND - 1, LIMITS), None);
        assert_eq!(
            liveness.wedged(START + 11 * SECOND, LIMITS),
            Some(WedgeReason::MissedHeartbeats)
        );
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
        let now = START;
        let mut backoff = Backoff::default();
        let young = Duration::from_secs(1);
        let waits: Vec<u64> = (0..9)
            .map(|_| backoff.failed(young, now, TIMES).as_secs())
            .collect();
        assert_eq!(waits, [0, 1, 2, 4, 8, 16, 30, 30, 30]);
        for _ in 0..100 {
            assert_eq!(backoff.failed(young, now, TIMES), Duration::from_secs(30));
        }
        assert_eq!(backoff.due(), Some(now + 30 * SECOND));
    }

    #[test]
    fn a_wait_runs_until_it_is_due() {
        let now = START;
        let mut backoff = Backoff::default();
        let young = Duration::from_secs(1);
        assert_eq!(backoff.failed(young, now, TIMES), Duration::ZERO);
        assert_eq!(backoff.due(), None, "the first is replaced at once");
        assert_eq!(backoff.failed(young, now, TIMES), Duration::from_secs(1));
        let due = now + SECOND;
        assert_eq!(backoff.due(), Some(due));
        let just_before = due - 1;
        assert!(!backoff.ended(just_before));
        assert!(backoff.ended(due));
        assert_eq!(backoff.due(), None);
        assert!(!backoff.ended(due), "ended once");
    }

    #[test]
    fn the_count_starts_again_after_a_minute_up() {
        let now = START;
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
        let now = START;
        let mut backoff = Backoff::default();
        let young = Duration::from_secs(1);
        for _ in 0..3 {
            let _ = backoff.failed(young, now, TIMES);
        }
        assert_eq!(backoff.due(), Some(now + 2 * SECOND));
        assert!(backoff.foreground(), "a wait ran");
        assert_eq!(backoff.due(), None);
        assert!(!backoff.foreground(), "none runs now");
        assert_eq!(backoff.failed(young, now, TIMES), Duration::ZERO);
        assert_eq!(backoff.failed(young, now, TIMES), Duration::from_secs(1));
    }

    #[test]
    fn a_failed_spares_replacement_waits_for_its_backoff() {
        let now = START;
        let mut spares = kept(2);
        let young = Duration::from_secs(1);
        assert!(spares.remove(OutpostId(1)));
        assert_eq!(spares.failed(young, now, TIMES), Duration::ZERO);
        assert_eq!(spares.due(), None);
        assert_eq!(spares.wanted(2, now), 1, "the first failure's at once");
        spares.add(OutpostId(3));
        assert!(spares.remove(OutpostId(3)));
        assert_eq!(spares.failed(young, now, TIMES), Duration::from_secs(1));
        let due = now + SECOND;
        assert_eq!(spares.due(), Some(due));
        let just_before = due - 1;
        assert_eq!(spares.wanted(2, just_before), 0, "not before its backoff");
        assert_eq!(spares.wanted(2, due), 1);
        assert_eq!(spares.due(), None);
    }

    /// Backoffs that ended start in the order they ended, a tie by process
    /// id, whatever order they are given in; one still running, or with no
    /// wait, is left, and each that ended runs no more.
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
        let now = START;
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
        assert_eq!(spares.wanted(2, START), 1);
    }

    #[test]
    fn a_spare_that_ends_waiting_is_replaced() {
        let mut spares = kept(2);
        assert!(spares.remove(OutpostId(1)));
        assert!(!spares.remove(OutpostId(1)), "removed once");
        assert_eq!(spares.wanted(2, START), 1);
    }
}
