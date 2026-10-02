//! The supervisor's pure decisions, kept apart from the I/O that gathers
//! their inputs so each is unit-tested on its own: when an outpost is wedged,
//! when an idle one is retired, when crashes stop respawns, and how facts
//! that arrive during a spawn are held.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use verbatim_model::TraceId;

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
    pub(super) fact: DeliveredFact,
}

/// Facts that arrive while an outpost is starting, held in arrival order and
/// released in that order once it is ready. A newer fact for the same object
/// and kind replaces the older one and takes its place at the back, NVDA's
/// limiter rule, so the release order is the order in which the surviving
/// facts last happened.
#[derive(Debug, Default)]
pub(super) struct HeldFacts {
    facts: Vec<HeldFact>,
}

impl HeldFacts {
    /// Holds `fact`, replacing any older fact for the same object and kind.
    pub(super) fn hold(&mut self, fact: HeldFact) {
        if let Some(key) = fact.fact.key() {
            self.facts
                .retain(|held| held.fact.key().as_ref() != Some(&key));
        }
        self.facts.push(fact);
    }

    /// Takes every held fact, in release order.
    pub(super) fn take(&mut self) -> Vec<HeldFact> {
        std::mem::take(&mut self.facts)
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
            fact,
        }
    }

    fn msaa_focus(hwnd: isize) -> DeliveredFact {
        DeliveredFact::MsaaFocus {
            hwnd,
            id_object: -4,
            id_child: 0,
        }
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
