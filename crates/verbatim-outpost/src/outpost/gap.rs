//! The worker's wait for a UIA focus after a `WinEvent` focus in a UIA
//! window (`docs/design/listener-without-connections.md`, decision 11).
//!
//! The worker drops a `WinEvent` focus copy whose window uses UIA, on the
//! rule that the window's UIA focus event reports the focus. UI Automation
//! sometimes delivers a newly started process's focus events late or never
//! (that design's risk 2), or after their senders are gone, so the listener
//! cannot read them (risk 13). This records the first such copy that no UIA
//! focus has answered, a gap, and says when the worker is to read its
//! application's focus itself instead: once the worker's bound has passed
//! with no marker waiting, or at once when a marker is dropped because its
//! sender has no window above it. At most one read is made for a gap.
//!
//! It is the worker's alone, kept on its thread with no lock, and costs a
//! few comparisons per focus. Its times are of two kinds: observation
//! times in milliseconds, which the copy and the UIA focus events carry and
//! which match a UIA focus to a copy, and the outpost's clock, which counts
//! the bound.

use std::time::{Duration, Instant};

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
    /// When the worker took it in, by the outpost's clock.
    pub(super) opened: Instant,
}

/// The worker's gap, if one is open, and what answers it.
#[derive(Debug, Default)]
pub(super) struct FocusGap {
    open: Option<Open>,
    /// Whether a drop for a sender with no window above it came while the
    /// gap was open: the gap is read at once.
    unreadable: bool,
    /// The newest observation of a UIA focus the worker has seen answered:
    /// a read, or a drop for another reason.
    last_answered_ms: Option<u64>,
    /// The outpost's clock as the worker last read it.
    clock_now: Option<Instant>,
}

impl FocusGap {
    /// The worker's clock as it handled its latest entry.
    pub(super) fn set_clock_now(&mut self, now: Instant) {
        self.clock_now = Some(now);
    }

    /// A copy in the UIA window `hwnd`, observed at `observed_at_ms`, which
    /// the worker dropped for its UIA focus to report: opens a gap, unless
    /// one is open or a UIA focus answered it already. Returns whether it
    /// opened one.
    pub(super) fn dropped_for_uia(
        &mut self,
        hwnd: isize,
        observed_at_ms: u64,
        now: Instant,
    ) -> bool {
        if self.open.is_some() || Self::answers(self.last_answered_ms, observed_at_ms) {
            return false;
        }
        self.open = Some(Open {
            hwnd,
            observed_at_ms,
            opened: now,
        });
        self.unreadable = false;
        true
    }

    /// A UIA focus marker observed at `observed_at_ms` was answered: read,
    /// or dropped for another reason than having no window above it.
    /// Closes the gap it answers, returning it.
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

    /// A UIA focus marker observed at `observed_at_ms` was dropped because
    /// its sender has no window above it: the gap it falls in is read at
    /// once.
    pub(super) fn sender_unreadable(&mut self, observed_at_ms: u64) {
        if let Some(open) = self.open
            && Self::answers(Some(observed_at_ms), open.observed_at_ms)
        {
            self.unreadable = true;
        }
    }

    /// The outpost reported a focus observed at `observed_at_ms`, or Core
    /// queued another application's then: a gap whose copy came no later
    /// is answered by it, and closes.
    pub(super) fn focus_after(&mut self, observed_at_ms: u64) -> Option<Open> {
        let open = self.open?;
        if observed_at_ms < open.observed_at_ms {
            return None;
        }
        self.close()
    }

    /// The gap, taken, once it is to be read at `now` by the outpost's
    /// clock: its bound has passed, or a marker of it could not be read.
    pub(super) fn take_due(&mut self, now: Instant, bound: Duration) -> Option<Open> {
        let open = self.open?;
        if !self.unreadable && now < open.opened + bound {
            return None;
        }
        self.close()
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
        assert!(gap.dropped_for_uia(7, 5_000, start));
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
    fn a_marker_dropped_for_no_window_reads_the_gap_at_once() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        gap.sender_unreadable(5_200);
        assert_eq!(gap.wake_at(start, BOUND), Some(start));
        assert_eq!(gap.take_due(start, BOUND).map(|open| open.hwnd), Some(7));
    }

    #[test]
    fn a_uia_focus_observed_just_before_the_copy_opens_no_gap() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        assert_eq!(gap.uia_answered(4_975), None);
        assert!(!gap.dropped_for_uia(7, 5_000, start));
        assert!(!gap.is_open());
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
        assert!(!gap.dropped_for_uia(8, 5_100, start));
        assert!(gap.take_due(start + BOUND, BOUND).is_some());
        assert!(gap.dropped_for_uia(9, 6_200, start + BOUND));
        assert_eq!(gap.wake_at(start + BOUND, BOUND), Some(start + BOUND * 2));
    }
}
