//! The supervisor's pure decisions, kept apart from the I/O that gathers
//! their inputs so each is unit-tested on its own: when an outpost is stuck,
//! when an idle one is retired, how long a failed outpost's replacement
//! waits, how facts that arrive during a spawn are held, which facts may
//! start an outpost, and which spare outposts are kept and given.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::time::Duration;

use verbatim_model::protocol::DeliveredFact;
use verbatim_model::{OutpostId, Pid, TraceId};

/// Why the owner killed an otherwise-alive outpost (recovery ladder rung 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WedgeReason {
    /// No pong for the supervisor's `MISSED_PONG_THRESHOLD`
    /// consecutive ping intervals: the outpost stopped answering.
    MissedHeartbeats,
    /// The most recent pong reported at least
    /// the supervisor's `ABANDONED_WORKER_LIMIT` abandoned
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

/// The supervisor's limits for judging a child stuck: how often it is
/// pinged, how many pings in a row it may miss, and how many abandoned
/// workers its outpost may report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WedgeLimits {
    /// The time between pings.
    pub ping_interval: Duration,
    /// How many ping intervals may pass with no pong.
    pub missed_pongs: u32,
    /// How many abandoned workers make an outpost stuck.
    pub abandoned_workers: usize,
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

/// Whether an outpost should be killed, and why: given its last pong time
/// and now, in microseconds on the run's shared clock, and its last
/// reported count of abandoned workers. Abandoned workers are checked
/// first, as the more specific diagnosis, and do not count while the
/// application's own windows are reported hung, since a replacement
/// outpost would hang the same way.
#[must_use]
pub fn wedge_decision(
    last_pong_us: u64,
    now_us: u64,
    limits: WedgeLimits,
    abandoned: usize,
    application_hung: bool,
) -> Option<WedgeReason> {
    if abandoned >= limits.abandoned_workers && !application_hung {
        return Some(WedgeReason::AbandonedWorkers);
    }
    if since_us(last_pong_us, now_us) >= limits.ping_interval * limits.missed_pongs {
        return Some(WedgeReason::MissedHeartbeats);
    }
    None
}

/// A child's liveness judged by its pongs alone: the focus listener's, or
/// a spare outpost's while it waits, which report no abandoned workers.
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

    /// Whether it has missed its pings at `now_us`
    /// ([`wedge_decision`], with no abandoned workers).
    #[must_use]
    pub fn wedged(&self, now_us: u64, limits: WedgeLimits) -> Option<WedgeReason> {
        wedge_decision(self.last_pong_us, now_us, limits, 0, false)
    }
}

/// Whether an outpost is retired: its application has not held attention for
/// `idle_after` since `last_attention_us`, at `now_us`, on the run's shared
/// clock, and Core holds none of its nodes. Core's own outpost is never
/// retired, which the caller checks.
#[must_use]
pub fn retirement_decision(
    last_attention_us: u64,
    now_us: u64,
    idle_after: Duration,
    holds_attention: bool,
    nodes_held: bool,
) -> bool {
    !holds_attention && !nodes_held && since_us(last_attention_us, now_us) >= idle_after
}

/// The backoff between an outpost's failures (`docs/design/focus-pipeline.md`,
/// section 19.5): one application's outposts that fail while serving it, or
/// the spares that fail while they wait. The first failure is replaced at
/// once; each further one waits twice as long as the one before, from
/// the first of its backoff (the supervisor's `Timings::backoff_first`) up
/// to its limit (`Timings::backoff_limit`). Nothing is ever given up. The
/// count starts again once what failed had stayed up for
/// the time it must stay up (`STAYED_UP`), and, for an application, when it comes
/// to the foreground, which also ends a wait that runs
/// ([`foreground`](Self::foreground)).
#[derive(Debug, Default)]
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
                let doublings = (failures - 2).min(31);
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

/// The applications whose backoff has ended at `now_us`, among
/// `backoffs`, in the order their waits ended, and by process id when two
/// ended at the same time: each is given its next generation in that
/// order, so which application takes which spare is decided here. Each
/// backoff that ended is told so ([`Backoff::ended`]), so no wait runs for
/// it after this.
pub fn due_starts<'a>(
    backoffs: impl IntoIterator<Item = (Pid, &'a mut Backoff)>,
    now_us: u64,
) -> Vec<Pid> {
    let mut due: Vec<(u64, Pid)> = backoffs
        .into_iter()
        .filter_map(|(pid, backoff)| {
            let due_us = backoff.due()?;
            backoff.ended(now_us).then_some((due_us, pid))
        })
        .collect();
    due.sort_unstable();
    due.into_iter().map(|(_, pid)| pid).collect()
}

/// A focus fact routed to an outpost, with the trace and observation time
/// (`timing`'s `observed_at_us`) the listener stamped.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldFact {
    /// The trace the listener minted for it.
    pub trace_id: TraceId,
    /// Its timing, with the observation the listener stamped.
    pub timing: verbatim_model::protocol::EventTiming,
    /// The fact.
    pub fact: DeliveredFact,
    /// For an MSAA `WinEvent`'s fact, the thread that raised it and when.
    pub win_event: Option<verbatim_model::protocol::WinEventStamp>,
    /// When verbatim.exe read it from the listener.
    pub received: super::Received,
}

/// Facts that arrive while an outpost is starting, held in arrival order and
/// released in that order once it is ready. A newer fact for the same object
/// and kind, and for an MSAA event other than a menu opening the same
/// thread that raised it (`DeliveredFact::key`), replaces the older one and takes its place at
/// the back (`docs/parity/backends.md`, "MSAA winevent flood control"), so
/// the release order is the order in which the surviving facts last
/// happened.
#[derive(Debug, Default)]
pub struct HeldFacts {
    facts: Vec<HeldFact>,
    /// When the first fact was held, on the run's shared clock, until
    /// they are taken.
    first_held_us: Option<u64>,
}

impl HeldFacts {
    /// Holds `fact`, replacing any older fact for the same object and kind
    /// and, for an MSAA event other than a menu opening, the same thread
    /// that raised it. `now_us` is kept only when no fact is held: the
    /// time the application's first fact was held.
    pub fn hold(&mut self, fact: HeldFact, now_us: u64) {
        if let Some(key) = fact.fact.key(fact.win_event) {
            self.facts
                .retain(|held| held.fact.key(held.win_event).as_ref() != Some(&key));
        }
        self.first_held_us.get_or_insert(now_us);
        self.facts.push(fact);
    }

    /// When the first fact still held, or taken since, was held: how long
    /// the application's first fact has waited for its outpost, on the
    /// run's shared clock.
    #[must_use]
    pub fn first_held_us(&self) -> Option<u64> {
        self.first_held_us
    }

    /// Takes every held fact, in release order.
    pub fn take(&mut self) -> Vec<HeldFact> {
        self.first_held_us = None;
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

/// Whether `fact` may start an outpost for an application that has none
/// (architecture section 1, "The focus listener (D13)"): focus, foreground,
/// menu, and alert facts may, a UIA focus's marker and read among them,
/// and the shell's window-snap results notification, the one Core speaks
/// from any application. A selection without an outpost is dropped, and so
/// is any other notification: Core speaks a notification only from the
/// focus's application, and an application with no outpost cannot hold the
/// focus (`docs/parity/focus-events.md`, "Background events"). A focus's
/// drop and the end of a listener's reads resolve only markers an outpost
/// already holds.
#[must_use]
pub fn may_start_outpost(fact: &DeliveredFact) -> bool {
    match fact {
        DeliveredFact::UiaSelection { .. }
        | DeliveredFact::UiaFocusDropped { .. }
        | DeliveredFact::UiaFocusReadsEnded => false,
        DeliveredFact::UiaNotification { notification, .. } => {
            notification.activity_id.as_deref() == Some(crate::SNAP_RESULTS_ACTIVITY)
        }
        DeliveredFact::UiaFocusMarker { .. }
        | DeliveredFact::UiaFocusRead { .. }
        | DeliveredFact::UiaMarked { .. }
        | DeliveredFact::MsaaFocus { .. }
        | DeliveredFact::Foreground { .. }
        | DeliveredFact::MenuPopup { .. }
        | DeliveredFact::Alert { .. }
        | DeliveredFact::Show { .. } => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The supervisor's limits: a ping every 3 s, 3 missed, 8 workers.
    const LIMITS: WedgeLimits = WedgeLimits {
        ping_interval: Duration::from_secs(3),
        missed_pongs: 3,
        abandoned_workers: 8,
    };

    /// A start on the shared clock, in microseconds.
    const START: u64 = 1_000_000_000;
    /// One second, in microseconds.
    const SECOND: u64 = 1_000_000;

    #[test]
    fn a_recent_pong_with_few_abandoned_workers_is_healthy() {
        assert_eq!(
            wedge_decision(START, START + SECOND, LIMITS, 7, false),
            None
        );
    }

    #[test]
    fn nine_seconds_without_a_pong_is_a_wedge() {
        let nine = START + 9 * SECOND;
        assert_eq!(wedge_decision(START, nine - 1, LIMITS, 0, false), None);
        assert_eq!(
            wedge_decision(START, nine, LIMITS, 0, false),
            Some(WedgeReason::MissedHeartbeats)
        );
    }

    #[test]
    fn eight_abandoned_workers_are_a_wedge_unless_the_application_is_hung() {
        assert_eq!(
            wedge_decision(START, START, LIMITS, 8, false),
            Some(WedgeReason::AbandonedWorkers)
        );
        assert_eq!(wedge_decision(START, START, LIMITS, 8, true), None);
        // A hung application does not excuse an outpost that stopped
        // answering pings altogether.
        assert_eq!(
            wedge_decision(START, START + 9 * SECOND, LIMITS, 8, true),
            Some(WedgeReason::MissedHeartbeats)
        );
    }

    #[test]
    fn a_now_before_the_last_pong_never_panics() {
        let earlier = START - 5 * SECOND;
        assert_eq!(wedge_decision(START, earlier, LIMITS, 0, false), None);
    }

    #[test]
    fn retirement_needs_two_idle_minutes_without_attention_or_held_nodes() {
        let idle = Duration::from_mins(2);
        let later = START + 120 * SECOND;
        assert!(retirement_decision(START, later, idle, false, false));
        assert!(!retirement_decision(START, later, idle, true, false));
        assert!(!retirement_decision(START, later, idle, false, true));
        assert!(!retirement_decision(
            START,
            START + 119 * SECOND,
            idle,
            false,
            false
        ));
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
    #[test]
    fn due_starts_are_in_the_order_their_backoffs_ended_then_by_process_id() {
        let young = Duration::from_secs(1);
        // A backoff whose wait, from a failure at `at`, is one second.
        let waiting = |at: u64| {
            let mut backoff = Backoff::default();
            let _ = backoff.failed(young, at, TIMES);
            let _ = backoff.failed(young, at, TIMES);
            backoff
        };
        let mut late = waiting(START + 2 * SECOND);
        let mut tie_high = waiting(START + SECOND);
        let mut early = waiting(START);
        let mut tie_low = waiting(START + SECOND);
        let mut running = waiting(START + 9 * SECOND);
        let mut none = Backoff::default();
        let now = START + 3 * SECOND;
        let due = due_starts(
            [
                (Pid(5), &mut late),
                (Pid(9), &mut tie_high),
                (Pid(7), &mut early),
                (Pid(3), &mut tie_low),
                (Pid(1), &mut running),
                (Pid(2), &mut none),
            ],
            now,
        );
        assert_eq!(due, [Pid(7), Pid(3), Pid(9), Pid(5)]);
        for ended in [&late, &tie_high, &early, &tie_low] {
            assert_eq!(ended.due(), None);
        }
        assert_eq!(running.due(), Some(START + 10 * SECOND));
        assert_eq!(none.due(), None);
    }

    fn held(observed_at_us: u64, fact: DeliveredFact) -> HeldFact {
        HeldFact {
            trace_id: TraceId::from_raw(observed_at_us),
            timing: verbatim_model::protocol::EventTiming {
                observed_at_us,
                ..verbatim_model::protocol::EventTiming::default()
            },
            fact,
            win_event: None,
            received: super::super::Received { at_us: 0 },
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

    #[test]
    fn held_facts_release_in_arrival_order_and_a_repeat_moves_to_the_back() {
        let mut facts = HeldFacts::default();
        facts.hold(held(1, DeliveredFact::Foreground { hwnd: 7 }), START);
        facts.hold(held(2, msaa_focus(8)), START);
        facts.hold(
            held(
                3,
                DeliveredFact::MenuPopup {
                    hwnd: 9,
                    id_object: -4,
                    id_child: 0,
                },
            ),
            START,
        );
        // Focus returns to the same object after the menu: the older entry
        // is replaced and the newer one is released last.
        facts.hold(held(4, msaa_focus(8)), START);
        let released: Vec<u64> = facts
            .take()
            .iter()
            .map(|fact| fact.timing.observed_at_us)
            .collect();
        assert_eq!(released, vec![1, 3, 4]);
        assert!(facts.take().is_empty(), "nothing is left held");
    }

    #[test]
    fn a_selection_or_a_background_notification_may_not_start_an_outpost() {
        use verbatim_model::protocol::UiaSnapshotFact;
        use verbatim_model::{
            NodeDetails, Notification, NotificationKind, NotificationProcessing, Role, StateSet,
        };
        let snapshot = UiaSnapshotFact {
            runtime_id: vec![1],
            role: Role::ListItem,
            name: None,
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        };
        let selection = DeliveredFact::UiaSelection {
            hwnd: 0,
            focus_window: 0,
            snapshot: snapshot.clone(),
        };
        assert!(!may_start_outpost(&selection));
        let notification = |activity_id: &str| DeliveredFact::UiaNotification {
            hwnd: 0,
            snapshot: snapshot.clone(),
            notification: Notification {
                kind: NotificationKind::ActionCompleted,
                processing: NotificationProcessing::ImportantAll,
                display_string: Some("Copied".into()),
                activity_id: Some(activity_id.into()),
            },
        };
        assert!(!may_start_outpost(&notification("Clipboard")));
        // A focus's drop, and the end of a listener's reads, only resolve
        // markers an outpost holds.
        assert!(!may_start_outpost(&DeliveredFact::UiaFocusDropped {
            marker: 1,
            focus: true,
            no_window: false,
        }));
        assert!(!may_start_outpost(&DeliveredFact::UiaFocusReadsEnded));
        for fact in [
            notification(crate::SNAP_RESULTS_ACTIVITY),
            DeliveredFact::Foreground { hwnd: 1 },
            DeliveredFact::UiaFocusMarker { marker: 1 },
            DeliveredFact::Alert {
                hwnd: 1,
                id_object: -4,
                id_child: 0,
            },
        ] {
            assert!(may_start_outpost(&fact), "{fact:?}");
        }
    }
}
