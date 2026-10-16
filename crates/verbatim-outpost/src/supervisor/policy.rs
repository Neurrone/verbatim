//! The supervisor's pure decisions, kept apart from the I/O that gathers
//! their inputs so each is unit-tested on its own: when an outpost is wedged,
//! when an idle one is retired, when crashes stop respawns, how facts
//! that arrive during a spawn are held, and which spare outposts are kept
//! and given.

#![forbid(unsafe_code)]

use std::collections::{HashMap, VecDeque};
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

/// Recent crashes of one application's outposts. After
/// [`CRASH_LIMIT`](super::CRASH_LIMIT) crashes within
/// [`CRASH_WINDOW`](super::CRASH_WINDOW) respawning stops until the next
/// foreground change to that application.
#[derive(Debug, Default)]
pub(super) struct CrashHistory {
    crashes: VecDeque<Instant>,
    stopped: bool,
}

impl CrashHistory {
    /// Records a crash at `now` and reports whether respawning has now
    /// stopped.
    pub(super) fn record(&mut self, now: Instant, limit: usize, window: Duration) -> bool {
        self.crashes.push_back(now);
        while self
            .crashes
            .front()
            .is_some_and(|&crash| now.saturating_duration_since(crash) > window)
        {
            self.crashes.pop_front();
        }
        if self.crashes.len() >= limit {
            self.stopped = true;
        }
        self.stopped
    }

    /// Whether respawning has stopped.
    pub(super) fn stopped(&self) -> bool {
        self.stopped
    }

    /// Clears the history: a foreground change to the application earns it
    /// another try.
    pub(super) fn reset(&mut self) {
        self.crashes.clear();
        self.stopped = false;
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

/// Whether the supervisor keeps spare outposts
/// (`docs/design/focus-pipeline.md`, section 19).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Keeping {
    /// Not yet: they are started once Core's own outpost is ready, or has
    /// ended without becoming ready.
    NotYet,
    /// Kept: a start takes one, and one is started whenever fewer than
    /// [`SPARES`](super::SPARES) wait and are owed.
    Kept,
    /// Stopped after repeated failures, until Verbatim restarts: every
    /// outpost is started for its application directly.
    Stopped,
}

/// The spare outposts: which wait, oldest first, whether each has said it
/// is ready, which given ones still owe their replacement, and their recent
/// failures. A spare given to an application never returns to the list.
/// Its replacement is owed until the outpost it became answers its first
/// idle query, or the limit passes, so the replacement's start does not
/// compete with the application's first focus.
#[derive(Debug)]
pub(super) struct Spares {
    keeping: Keeping,
    /// The spares waiting, oldest first, each with whether it is ready.
    waiting: VecDeque<(OutpostId, bool)>,
    /// The given spares whose replacements are owed, with when each was
    /// given.
    owed: HashMap<OutpostId, Instant>,
    /// Recent failures, with why each failed.
    failures: VecDeque<(Instant, String)>,
}

impl Default for Spares {
    fn default() -> Self {
        Self {
            keeping: Keeping::NotYet,
            waiting: VecDeque::new(),
            owed: HashMap::new(),
            failures: VecDeque::new(),
        }
    }
}

impl Spares {
    /// Starts keeping spares, unless they are kept already or stopped.
    /// Returns whether this call started it.
    pub(super) fn keep(&mut self) -> bool {
        if self.keeping != Keeping::NotYet {
            return false;
        }
        self.keeping = Keeping::Kept;
        true
    }

    /// Whether spares are kept, not yet, or stopped.
    #[cfg(test)]
    fn keeping(&self) -> Keeping {
        self.keeping
    }

    /// How many spares to start now so that `count` wait or are owed.
    pub(super) fn wanted(&self, count: usize) -> usize {
        if self.keeping != Keeping::Kept {
            return 0;
        }
        count.saturating_sub(self.waiting.len() + self.owed.len())
    }

    /// Lists a spare just started, at the back.
    pub(super) fn add(&mut self, spare: OutpostId) {
        self.waiting.push_back((spare, false));
    }

    /// Whether `spare` is a spare waiting.
    pub(super) fn contains(&self, spare: OutpostId) -> bool {
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
    /// kept, and owes its replacement from `now`.
    pub(super) fn take(&mut self, now: Instant) -> Option<OutpostId> {
        if self.keeping != Keeping::Kept {
            return None;
        }
        let (spare, _) = self.waiting.pop_front()?;
        self.owed.insert(spare, now);
        Some(spare)
    }

    /// Takes `spare` off the list: it has ended while waiting. Returns
    /// whether it was listed.
    pub(super) fn remove(&mut self, spare: OutpostId) -> bool {
        let before = self.waiting.len();
        self.waiting.retain(|&(waiting, _)| waiting != spare);
        self.waiting.len() != before
    }

    /// Whether the given outpost `outpost` still owes its replacement.
    pub(super) fn owes(&self, outpost: OutpostId) -> bool {
        self.owed.contains_key(&outpost)
    }

    /// The given outpost `outpost` has answered an idle query, or has
    /// ended: its replacement is owed no longer. Returns whether it was.
    pub(super) fn settled(&mut self, outpost: OutpostId) -> bool {
        self.owed.remove(&outpost).is_some()
    }

    /// The given outposts whose replacements have been owed for `limit` or
    /// longer at `now`, owed no longer.
    pub(super) fn overdue(&mut self, now: Instant, limit: Duration) -> Vec<OutpostId> {
        let overdue: Vec<OutpostId> = self
            .owed
            .iter()
            .filter(|&(_, &given)| now.saturating_duration_since(given) >= limit)
            .map(|(&outpost, _)| outpost)
            .collect();
        for outpost in &overdue {
            self.owed.remove(outpost);
        }
        overdue
    }

    /// Records a spare's failure at `now`, for `why`. After `limit` failures
    /// within `window`, stops keeping spares and returns every recent
    /// failure's reason, with the spares still waiting, which are ended:
    /// they are no longer kept.
    pub(super) fn failed(
        &mut self,
        now: Instant,
        why: String,
        (limit, window): (usize, Duration),
    ) -> Option<(Vec<String>, Vec<OutpostId>)> {
        self.failures.push_back((now, why));
        while self
            .failures
            .front()
            .is_some_and(|&(at, _)| now.saturating_duration_since(at) > window)
        {
            self.failures.pop_front();
        }
        if self.failures.len() < limit || self.keeping == Keeping::Stopped {
            return None;
        }
        self.keeping = Keeping::Stopped;
        self.owed.clear();
        let reasons = self.failures.iter().map(|(_, why)| why.clone()).collect();
        let waiting = self.waiting.drain(..).map(|(spare, _)| spare).collect();
        Some((reasons, waiting))
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

    #[test]
    fn three_crashes_within_a_minute_stop_respawning_until_reset() {
        let start = Instant::now();
        let window = Duration::from_mins(1);
        let mut history = CrashHistory::default();
        assert!(!history.record(start, 3, window));
        // A crash more than a minute after the first one leaves two in the
        // window, not three.
        assert!(!history.record(start + Duration::from_secs(30), 3, window));
        assert!(!history.record(start + Duration::from_secs(70), 3, window));
        assert!(history.record(start + Duration::from_secs(80), 3, window));
        assert!(history.stopped());
        history.reset();
        assert!(!history.stopped());
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

    const SPARE_FAILURES: (usize, Duration) = (3, Duration::from_mins(1));

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
        let mut spares = Spares::default();
        assert_eq!(spares.wanted(2), 0);
        spares.add(OutpostId(1));
        assert_eq!(spares.take(Instant::now()), None);
        assert!(spares.keep());
        assert!(!spares.keep(), "kept once");
        assert_eq!(spares.wanted(2), 1);
    }

    #[test]
    fn the_oldest_spare_is_given_ready_or_not_and_never_listed_again() {
        let now = Instant::now();
        let mut spares = kept(2);
        assert_eq!(
            spares.ready(OutpostId(2)),
            Some(1),
            "the newer one is ready"
        );
        assert_eq!(spares.take(now), Some(OutpostId(1)), "the older, not ready");
        assert!(!spares.contains(OutpostId(1)));
        assert_eq!(
            spares.ready(OutpostId(1)),
            None,
            "a given spare is not listed"
        );
        assert!(spares.contains(OutpostId(2)));
        assert_eq!(spares.take(now), Some(OutpostId(2)));
        assert_eq!(spares.take(now), None);
    }

    #[test]
    fn a_given_spares_replacement_is_owed_until_its_first_idle_answer() {
        let now = Instant::now();
        let mut spares = kept(2);
        let given = spares.take(now).expect("a spare");
        assert!(spares.owes(given));
        assert_eq!(spares.wanted(2), 0, "the replacement waits for the answer");
        assert!(spares.settled(given), "the first idle answer");
        assert!(!spares.settled(given), "only the first counts");
        assert_eq!(spares.wanted(2), 1, "the replacement is started now");
    }

    #[test]
    fn a_replacement_owed_past_the_limit_is_started() {
        let now = Instant::now();
        let limit = Duration::from_secs(10);
        let mut spares = kept(2);
        let given = spares.take(now).expect("a spare");
        let just_before = (now + limit)
            .checked_sub(Duration::from_millis(1))
            .expect("computable");
        assert_eq!(spares.overdue(just_before, limit), []);
        assert_eq!(spares.overdue(now + limit, limit), [given]);
        assert!(!spares.owes(given));
        assert_eq!(spares.wanted(2), 1);
    }

    #[test]
    fn a_spare_that_ends_waiting_is_replaced() {
        let mut spares = kept(2);
        assert!(spares.remove(OutpostId(1)));
        assert!(!spares.remove(OutpostId(1)), "removed once");
        assert_eq!(spares.wanted(2), 1);
    }

    #[test]
    fn three_spare_failures_within_a_minute_stop_the_spares() {
        let start = Instant::now();
        let mut spares = kept(2);
        assert_eq!(spares.failed(start, "one".to_owned(), SPARE_FAILURES), None);
        // More than a minute after the first: two in the window, not three.
        assert_eq!(
            spares.failed(
                start + Duration::from_secs(30),
                "two".to_owned(),
                SPARE_FAILURES
            ),
            None
        );
        assert_eq!(
            spares.failed(
                start + Duration::from_secs(70),
                "three".to_owned(),
                SPARE_FAILURES
            ),
            None
        );
        assert_eq!(spares.keeping(), Keeping::Kept);
        assert_eq!(
            spares.failed(
                start + Duration::from_secs(80),
                "four".to_owned(),
                SPARE_FAILURES
            ),
            Some((
                vec!["two".to_owned(), "three".to_owned(), "four".to_owned()],
                vec![OutpostId(1), OutpostId(2)]
            ))
        );
        assert_eq!(spares.keeping(), Keeping::Stopped);
        assert_eq!(spares.wanted(2), 0);
        assert_eq!(spares.take(start), None);
        assert!(!spares.keep(), "stopped until Verbatim restarts");
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
