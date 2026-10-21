//! The worker's wait for a UIA focus after a `WinEvent` focus in a UIA
//! window (`docs/design/listener-without-connections.md`, decision 11).
//!
//! The worker drops a `WinEvent` focus copy whose window uses UIA, on the
//! rule that the window's UIA focus event reports the focus. UI Automation
//! sometimes delivers a newly started process's focus events late or never
//! (that design's risk 2), or after their senders are gone, so the listener
//! cannot read them (risk 13). This records the first such copy that no UIA
//! focus has answered, a gap, and says when the worker is to read its
//! application's focus itself instead: once the worker's bound has passed,
//! counted from the copy's observation, with no marker waiting, or at once
//! when a focus marker is dropped as stale, its sender having no window
//! above it or no longer having the keyboard focus. Only a focus marker's
//! resolution answers a gap; a selection's, a notification's, or a menu
//! opening's neither closes it nor reads it (Fable, 2026-10-21, pending
//! Dickson's acceptance). At most one read is made for a gap.
//!
//! It is the worker's alone, kept on its thread with no lock, and costs a
//! few comparisons per focus. Its times are of two kinds: observation
//! times in milliseconds, which the copy and the UIA focus events carry and
//! which match a UIA focus to a copy, and the outpost's clock, which counts
//! the bound.

use std::time::{Duration, Instant};

use super::intake::MarkerNews;

/// How long before a copy's observation a UIA focus may have been observed
/// and still answer it: a UIA focus can reach the listener before the
/// `WinEvent` of the same focus, by up to 25.5 ms in the runs measured
/// (5 of 1,680 `WinEvent` focuses in UIA windows, `measure_focus_explorer`,
/// 2026-10-21).
pub(super) const ANSWERED_BEFORE_MS: u64 = 100;

/// The first `WinEvent` focus copy in a UIA window that no UIA focus has
/// answered yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Open {
    /// The copy's window.
    pub(super) hwnd: isize,
    /// When the copy was observed, in milliseconds.
    pub(super) observed_at_ms: u64,
    /// When the copy was observed, put on the outpost's clock: the bound
    /// counts from there, not from when the worker took the copy up.
    pub(super) opened: Instant,
}

/// The worker's gap, if one is open, and what answers it.
#[derive(Debug, Default)]
pub(super) struct FocusGap {
    open: Option<Open>,
    /// Whether a focus marker that answers the gap was dropped as stale:
    /// the gap is read at once.
    unreadable: bool,
    /// The newest observation of a UIA focus the worker has seen answered:
    /// a read, or a drop for a reason that is not staleness.
    last_answered_ms: Option<u64>,
    /// The newest observation of a UIA focus the worker has seen dropped as
    /// stale.
    last_stale_ms: Option<u64>,
    /// The outpost's clock as the worker last read it.
    clock_now: Option<Instant>,
}

impl FocusGap {
    /// The worker's clock as it handled its latest entry.
    pub(super) fn set_clock_now(&mut self, now: Instant) {
        self.clock_now = Some(now);
    }

    /// A copy in the UIA window `hwnd`, observed at `observed_at_ms`, which
    /// the worker dropped for its UIA focus to report at `now` by the
    /// outpost's clock, `now_ms` by the system's: opens a gap, unless one is
    /// open or a UIA focus answered it already, to be read at once when a
    /// stale drop answers it. Returns whether it opened one.
    pub(super) fn dropped_for_uia(
        &mut self,
        hwnd: isize,
        observed_at_ms: u64,
        (now, now_ms): (Instant, u64),
    ) -> bool {
        if self.open.is_some() || Self::answers(self.last_answered_ms, observed_at_ms) {
            return false;
        }
        // A copy that carries no observation, which only a test's delivery
        // does, counts from now; so does an age beyond the clock's range,
        // which no observation has.
        let age = if observed_at_ms == 0 {
            Duration::ZERO
        } else {
            Duration::from_millis(now_ms.saturating_sub(observed_at_ms))
        };
        self.open = Some(Open {
            hwnd,
            observed_at_ms,
            opened: now.checked_sub(age).unwrap_or(now),
        });
        self.unreadable = Self::answers(self.last_stale_ms, observed_at_ms);
        true
    }

    /// What became of the focus markers the intake resolved without a read
    /// since the worker last took its news.
    pub(super) fn marker_news(&mut self, news: MarkerNews) {
        if let Some(at) = news.answered_at_ms {
            self.uia_answered(at);
        }
        if let Some(at) = news.stale_at_ms {
            self.focus_stale(at);
        }
    }

    /// A UIA focus marker observed at `observed_at_ms` was answered: read,
    /// or dropped for a reason that is not staleness. Closes the gap it
    /// answers, returning it.
    pub(super) fn uia_answered(&mut self, observed_at_ms: u64) -> Option<Open> {
        self.last_answered_ms = Some(
            self.last_answered_ms
                .map_or(observed_at_ms, |at| at.max(observed_at_ms)),
        );
        let open = self.open?;
        if !Self::answers(Some(observed_at_ms), open.observed_at_ms) {
            return None;
        }
        self.close()
    }

    /// A UIA focus marker observed at `observed_at_ms` was dropped as
    /// stale: the gap it answers is read at once.
    fn focus_stale(&mut self, observed_at_ms: u64) {
        self.last_stale_ms = Some(
            self.last_stale_ms
                .map_or(observed_at_ms, |at| at.max(observed_at_ms)),
        );
        if let Some(open) = self.open
            && Self::answers(Some(observed_at_ms), open.observed_at_ms)
        {
            self.unreadable = true;
        }
    }

    /// The outpost reported a focus observed at `observed_at_ms`, not a
    /// foreground change, which says nothing of the focus inside its
    /// window; or Core queued another application's then: a gap whose copy
    /// came no later is answered by it, and closes.
    pub(super) fn focus_after(&mut self, observed_at_ms: u64) -> Option<Open> {
        let open = self.open?;
        if observed_at_ms < open.observed_at_ms {
            return None;
        }
        self.close()
    }

    /// Whether the gap is to be read at `now` by the outpost's clock: its
    /// bound has passed, or a focus marker of it was dropped as stale.
    pub(super) fn is_due(&self, now: Instant, bound: Duration) -> bool {
        self.open
            .is_some_and(|open| self.unreadable || now >= open.opened + bound)
    }

    /// The gap, taken, once it is due ([`is_due`](Self::is_due)).
    pub(super) fn take_due(&mut self, now: Instant, bound: Duration) -> Option<Open> {
        if !self.is_due(now, bound) {
            return None;
        }
        self.close()
    }

    /// Whether a focus marker of the gap was dropped as stale.
    pub(super) fn unreadable(&self) -> bool {
        self.unreadable
    }

    /// When, in the system's time, the worker is to wake for the open gap
    /// with nothing else to do: its bound from when it opened by the
    /// outpost's clock, put on `real_now` from the clock as last read.
    pub(super) fn wake_at(&self, real_now: Instant, bound: Duration) -> Option<Instant> {
        let open = self.open?;
        if self.unreadable {
            return Some(real_now);
        }
        let clock_now = self.clock_now.unwrap_or(open.opened);
        let remaining = (open.opened + bound).saturating_duration_since(clock_now);
        Some(real_now + remaining)
    }

    /// Whether a gap is open.
    pub(super) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    fn close(&mut self) -> Option<Open> {
        self.unreadable = false;
        self.open.take()
    }

    /// Whether a UIA focus observed at `answered` answers a copy observed
    /// at `copy`.
    fn answers(answered: Option<u64>, copy: u64) -> bool {
        answered.is_some_and(|at| at.saturating_add(ANSWERED_BEFORE_MS) >= copy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUND: Duration = Duration::from_secs(1);

    fn opened(gap: &mut FocusGap, start: Instant) {
        assert!(gap.dropped_for_uia(7, 5_000, (start, 5_000)));
    }

    fn stale_at(at: u64) -> MarkerNews {
        MarkerNews {
            answered_at_ms: None,
            stale_at_ms: Some(at),
        }
    }

    #[test]
    fn a_uia_focus_within_the_bound_closes_the_gap_and_nothing_is_read() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        assert_eq!(
            gap.uia_answered(5_030),
            Some(Open {
                hwnd: 7,
                observed_at_ms: 5_000,
                opened: start
            })
        );
        assert_eq!(gap.take_due(start + BOUND, BOUND), None);
    }

    #[test]
    fn an_unanswered_gap_is_read_once_at_its_bound_and_not_before() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        assert_eq!(
            gap.take_due(start + Duration::from_millis(999), BOUND),
            None
        );
        assert_eq!(
            gap.take_due(start + BOUND, BOUND).map(|open| open.hwnd),
            Some(7)
        );
        assert_eq!(gap.take_due(start + BOUND * 2, BOUND), None);
    }

    #[test]
    fn a_focus_marker_dropped_as_stale_reads_the_gap_at_once() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        gap.marker_news(stale_at(5_200));
        assert_eq!(gap.wake_at(start, BOUND), Some(start));
        assert_eq!(gap.take_due(start, BOUND).map(|open| open.hwnd), Some(7));
    }

    #[test]
    fn a_uia_focus_observed_just_before_the_copy_opens_no_gap() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        assert_eq!(gap.uia_answered(4_975), None);
        assert!(!gap.dropped_for_uia(7, 5_000, (start, 5_000)));
        assert!(!gap.is_open());
    }

    #[test]
    fn a_uia_focus_observed_more_than_100_ms_before_the_copy_leaves_it_to_open_a_gap() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        assert_eq!(gap.uia_answered(4_899), None);
        assert!(gap.dropped_for_uia(7, 5_000, (start, 5_000)));
    }

    #[test]
    fn a_read_observed_more_than_100_ms_before_the_gaps_copy_does_not_close_it() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        assert_eq!(gap.uia_answered(4_899), None);
        assert!(gap.is_open());
    }

    #[test]
    fn the_bound_counts_from_the_copys_observation() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        // Taken up 400 ms after it was observed.
        assert!(gap.dropped_for_uia(7, 5_000, (start, 5_400)));
        let bound_passed = start + Duration::from_millis(600);
        assert!(!gap.is_due(start + Duration::from_millis(599), BOUND));
        assert_eq!(
            gap.take_due(bound_passed, BOUND).map(|open| open.hwnd),
            Some(7)
        );
    }

    #[test]
    fn a_focus_queued_after_the_copy_closes_the_gap() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        assert_eq!(gap.focus_after(4_999), None);
        assert_eq!(gap.focus_after(5_400).map(|open| open.hwnd), Some(7));
        assert_eq!(gap.take_due(start + BOUND, BOUND), None);
    }

    #[test]
    fn after_a_read_the_next_unanswered_copy_opens_a_new_gap() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        assert!(!gap.dropped_for_uia(8, 5_100, (start, 5_100)));
        assert!(gap.take_due(start + BOUND, BOUND).is_some());
        assert!(gap.dropped_for_uia(9, 6_200, (start + BOUND, 6_200)));
        assert_eq!(gap.wake_at(start + BOUND, BOUND), Some(start + BOUND * 2));
    }
}
