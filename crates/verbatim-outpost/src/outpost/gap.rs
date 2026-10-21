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
//! when a focus marker is dropped because its sender has no window above
//! it. Only a focus marker's resolution answers a gap
//! ([`DropEvidence`](crate::protocol::DropEvidence)): a drop that ends the
//! wait closes it, a drop that could not establish the focus leaves it to
//! its bound, a drop for no window reads it at once, and a selection's, a
//! notification's or a menu opening's neither closes it nor reads it
//! (Fable, 2026-10-21, pending Dickson's acceptance). At most one read is
//! made for a gap.
//!
//! A read answers only a copy in its own top-level window: a UIA focus in
//! another window of the application, such as the folder window a new File
//! Explorer window was opened from, establishes nothing of the copy's focus
//! (Fable, 2026-10-21, pending Dickson's acceptance). The top-level window,
//! not the copy's own, is compared, since a copy can name a window above
//! the focus's own: Windows Terminal's first copy names its top-level
//! window, its UIA focus the input site inside it. A drop carries no
//! window, so it is matched by its observation alone, a limit: a verdict on
//! a focus in another window, such as a repeat there, still closes the gap.
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
    /// The copy's top-level window, which a read must share to answer it.
    pub(super) top_level: isize,
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
    /// Whether a focus marker that answers the gap was dropped because its
    /// sender had no window above it: the gap is read at once.
    unreadable: bool,
    /// The newest observation of a UIA focus the worker has seen answered,
    /// with the top-level window of a read: a read, or a drop that ends the
    /// wait, which carries no window (`None`).
    last_answered: Option<Answered>,
    /// The newest observation of a UIA focus the worker has seen dropped
    /// because its sender had no window above it.
    last_no_window_ms: Option<u64>,
    /// The outpost's clock as the worker last read it.
    clock_now: Option<Instant>,
}

/// A UIA focus that answers a copy observed no later than
/// [`ANSWERED_BEFORE_MS`] after it, in its top-level window when it has one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Answered {
    observed_at_ms: u64,
    top_level: Option<isize>,
}

impl Answered {
    /// Whether it answers a copy observed at `copy_ms` in the top-level
    /// window `copy_top_level`.
    fn answers(self, copy_ms: u64, copy_top_level: isize) -> bool {
        self.observed_at_ms.saturating_add(ANSWERED_BEFORE_MS) >= copy_ms
            && self.top_level.is_none_or(|top| top == copy_top_level)
    }
}

impl FocusGap {
    /// The worker's clock as it handled its latest entry.
    pub(super) fn set_clock_now(&mut self, now: Instant) {
        self.clock_now = Some(now);
    }

    /// A copy in the UIA window `hwnd`, whose top-level window is
    /// `top_level`, observed at `observed_at_ms`, which the worker dropped
    /// for its UIA focus to report at `now` by the outpost's clock, `now_ms`
    /// by the system's: opens a gap, unless one is open or a UIA focus
    /// answered it already, to be read at once when a drop for no window
    /// answers it. Returns whether it opened one.
    pub(super) fn dropped_for_uia(
        &mut self,
        (hwnd, top_level): (isize, isize),
        observed_at_ms: u64,
        (now, now_ms): (Instant, u64),
    ) -> bool {
        if self.open.is_some()
            || self
                .last_answered
                .is_some_and(|answered| answered.answers(observed_at_ms, top_level))
        {
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
            top_level,
            observed_at_ms,
            opened: now.checked_sub(age).unwrap_or(now),
        });
        self.unreadable = Self::answers(self.last_no_window_ms, observed_at_ms);
        true
    }

    /// What became of the focus markers the intake resolved without a read
    /// since the worker last took its news: the gap a drop that ends the
    /// wait closed, if it closed one.
    pub(super) fn marker_news(&mut self, news: MarkerNews) -> Option<Open> {
        let closed = news
            .answered_at_ms
            .and_then(|at| self.uia_answered(at, None));
        if let Some(at) = news.no_window_at_ms {
            self.focus_without_window(at);
        }
        closed
    }

    /// A UIA focus marker observed at `observed_at_ms` was answered: read,
    /// in the top-level window `top_level`, or dropped in a way that ends
    /// the wait, with no window (`None`). Closes the gap it answers,
    /// returning it.
    pub(super) fn uia_answered(
        &mut self,
        observed_at_ms: u64,
        top_level: Option<isize>,
    ) -> Option<Open> {
        let answered = Answered {
            observed_at_ms,
            top_level,
        };
        if self
            .last_answered
            .is_none_or(|last| last.observed_at_ms <= observed_at_ms)
        {
            self.last_answered = Some(answered);
        }
        let open = self.open?;
        if !answered.answers(open.observed_at_ms, open.top_level) {
            return None;
        }
        self.close()
    }

    /// A UIA focus marker observed at `observed_at_ms` was dropped because
    /// its sender had no window above it: the gap it answers is read at
    /// once.
    fn focus_without_window(&mut self, observed_at_ms: u64) {
        self.last_no_window_ms = Some(
            self.last_no_window_ms
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
    /// bound has passed, or a focus marker of it was dropped for no window.
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

    /// Whether a focus marker of the gap was dropped for no window.
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
        assert!(gap.dropped_for_uia((7, 70), 5_000, (start, 5_000)));
    }

    fn no_window_at(at: u64) -> MarkerNews {
        MarkerNews {
            answered_at_ms: None,
            no_window_at_ms: Some(at),
        }
    }

    #[test]
    fn a_uia_focus_within_the_bound_closes_the_gap_and_nothing_is_read() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        assert_eq!(
            gap.uia_answered(5_030, Some(70)),
            Some(Open {
                hwnd: 7,
                top_level: 70,
                observed_at_ms: 5_000,
                opened: start
            })
        );
        assert_eq!(gap.take_due(start + BOUND, BOUND), None);
    }

    /// A read in another top-level window of the application, as the
    /// folder window a new File Explorer window was opened from, neither
    /// closes the gap nor answers a later copy; a drop, which carries no
    /// window, is matched by its observation alone.
    #[test]
    fn a_read_in_another_top_level_window_answers_nothing_of_the_copy() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        assert_eq!(gap.uia_answered(5_030, Some(80)), None);
        assert!(gap.is_open());
        assert_eq!(
            gap.take_due(start + BOUND, BOUND).map(|open| open.hwnd),
            Some(7)
        );
        assert!(gap.dropped_for_uia((7, 70), 5_040, (start + BOUND, 5_040)));
        assert_eq!(
            gap.marker_news(MarkerNews {
                answered_at_ms: Some(5_050),
                no_window_at_ms: None,
            })
            .map(|open| open.hwnd),
            Some(7)
        );
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
    fn a_focus_marker_dropped_for_no_window_reads_the_gap_at_once() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        gap.marker_news(no_window_at(5_200));
        assert_eq!(gap.wake_at(start, BOUND), Some(start));
        assert_eq!(gap.take_due(start, BOUND).map(|open| open.hwnd), Some(7));
    }

    #[test]
    fn a_uia_focus_observed_just_before_the_copy_opens_no_gap() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        assert_eq!(gap.uia_answered(4_975, Some(70)), None);
        assert!(!gap.dropped_for_uia((7, 70), 5_000, (start, 5_000)));
        assert!(!gap.is_open());
    }

    #[test]
    fn a_uia_focus_observed_more_than_100_ms_before_the_copy_leaves_it_to_open_a_gap() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        assert_eq!(gap.uia_answered(4_899, Some(70)), None);
        assert!(gap.dropped_for_uia((7, 70), 5_000, (start, 5_000)));
    }

    #[test]
    fn a_read_observed_more_than_100_ms_before_the_gaps_copy_does_not_close_it() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        opened(&mut gap, start);
        assert_eq!(gap.uia_answered(4_899, Some(70)), None);
        assert!(gap.is_open());
    }

    #[test]
    fn the_bound_counts_from_the_copys_observation() {
        let start = Instant::now();
        let mut gap = FocusGap::default();
        // Taken up 400 ms after it was observed.
        assert!(gap.dropped_for_uia((7, 70), 5_000, (start, 5_400)));
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
        assert!(!gap.dropped_for_uia((8, 70), 5_100, (start, 5_100)));
        assert!(gap.take_due(start + BOUND, BOUND).is_some());
        assert!(gap.dropped_for_uia((9, 70), 6_200, (start + BOUND, 6_200)));
        assert_eq!(gap.wake_at(start + BOUND, BOUND), Some(start + BOUND * 2));
    }
}
