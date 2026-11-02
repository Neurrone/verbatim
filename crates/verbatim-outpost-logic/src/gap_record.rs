//! The outpost's gap record: the wait, after a `WinEvent` focus copy the
//! worker dropped because it judged the copy's window UIA, or a foreground
//! change into one of the application's windows, for the application's
//! focus to be told to Core (`docs/design/focus-pipeline-rework-remaining.md`,
//! "The gap, closed by the outpost's own reports";
//! `docs/design/listener-without-connections.md`, decision 11).
//!
//! UI Automation sometimes delivers an application's focus events late or
//! never while the `WinEvent` focus still comes, or after their senders are
//! gone, and an application whose thread is held raises no focus at all.
//! The record is kept beside the verdict that dropped the copy, in the
//! outpost's reducer ([`super`]), so nothing judges the copy's window again,
//! and every stamp it compares is on the run's one clock. It is a plain
//! record with no call and no timer of its own: the reducer applies to it a
//! program's judgements of copies, each foreground change as the reducer
//! takes it in, each report the outpost sends Core, the one fact a drop
//! carries, and its bound as one of the reducer's timers.
//!
//! What closes it: a report, every focus the outpost sends Core but a
//! whole-window focus in the record's own top-level window, while the
//! record waits for the application's focus ([`Awaits::Focus`]); any report
//! in its top-level window, a whole-window focus included, while it waits
//! for a window's title ([`Awaits::Title`]). A report in the record's
//! top-level window closes it when observed no earlier than
//! [`ANSWERED_BEFORE_US`] before the record's stamp, since a UIA focus can
//! reach the listener up to 25.5 ms before its `WinEvent` (5 of 1,680
//! measured); one in another top-level window, or with no window facts,
//! only when observed after it. The newest report per top-level window,
//! a whole-window focus never among them, is kept for the look-back, so a
//! copy judged after a report of its own focus opens nothing; it is pruned
//! when the window is destroyed ([`GapRecord::window_destroyed`]). A drop
//! for no window above the sender is kept apart: a copy that comes after
//! it is read at once, and an open record it answers is read at once.
//!
//! The restamp: while a record is open, a later copy the worker drops for
//! UIA in its top-level window gives it its observation and window,
//! leaving its bound; a later foreground change into it does too, and arms
//! a new bound a second from the change, so every visit to a held
//! application says its title a second after it. The stamp only moves
//! forward: an older copy or change, as in a focus run tried newest first,
//! neither restamps the record nor answers it.
//!
//! The bound: when it passes, the reducer has the shell test the record's
//! top-level window against the foreground ([`GapRecord::at_bound`]); an
//! unrelated window closes the record. Otherwise the record stays open
//! until a report closes it, its bound passed ([`Bound::Passed`]), so it
//! is not waited for again until a foreground change arms it anew.

use std::collections::BTreeMap;

use verbatim_model::TraceId;

/// How long a record waits, from the observation that opened or armed it,
/// for the application's focus, in microseconds. The UIA focus of a
/// `WinEvent` focus usually comes within 1 to 35 ms in Settings, and in the
/// same millisecond at the median in File Explorer; a newly started File
/// Explorer process's first one came 222 to 514 ms after the first item's
/// `WinEvent` in `explorer_folder_window`, and 515 to 2,694 ms after the
/// process's first focus `WinEvent` in a UIA window in 55 new windows of
/// `measure_focus_explorer` (2026-10-21), all but two under a second; when
/// UI Automation's delivery fails none comes, in Settings for 14.5 seconds.
/// A read made before a late UIA focus costs one read and nothing heard
/// twice, since Core takes the late focus as a duplicate of the read's; a
/// read made late leaves the user in silence. So the bound is a second
/// rather than above every late delivery measured: the silence budget. It
/// is the UIA focus wait both readers share
/// ([`verbatim_reader_rules::UIA_FOCUS_WAIT`]).
#[expect(
    clippy::cast_possible_truncation,
    reason = "a second in microseconds fits a u64"
)]
pub const BOUND_US: u64 = verbatim_reader_rules::UIA_FOCUS_WAIT.as_micros() as u64;

/// How long before a record's stamp a report in its top-level window may
/// have been observed and still close it: a UIA focus can reach the
/// listener before the `WinEvent` of the same focus, by up to 25.5 ms in
/// the runs measured (5 of 1,680 `WinEvent` focuses in UIA windows,
/// `measure_focus_explorer`, 2026-10-21).
pub const ANSWERED_BEFORE_US: u64 = 100_000;

/// What an open record waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Awaits {
    /// The application's focus: a record opened by a copy, or by a
    /// foreground change into a window the outpost judges UIA by a kept
    /// verdict or the class lists. A whole-window focus does not close it.
    Focus,
    /// The window's title: a record opened by a foreground change into any
    /// other window, whose own report is owed by the worker. Any report in
    /// its top-level window closes it, and no gap read is made for it.
    Title,
}

/// Where an open record's bound stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bound {
    /// Not reached: the reducer's timer waits for it.
    Waiting,
    /// Reached and being handled: its test asked, or its entry queued.
    Queued,
    /// Handled: the record waits, with no timer, for a report or a
    /// foreground change.
    Passed,
}

/// The open record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Open {
    /// The newest copy's or foreground change's window.
    window: isize,
    /// That window's top-level window.
    top_level: isize,
    /// The newest copy's or foreground change's observation, which its
    /// read and its title are stamped with.
    stamp_us: u64,
    /// The newest copy's or foreground change's trace, which its bound's
    /// entry and its title carry.
    trace: TraceId,
    /// When its bound passes: a second from its first copy's observation,
    /// or from its newest foreground change's.
    due_us: u64,
    /// Where its bound stands.
    bound: Bound,
    /// What it waits for.
    awaits: Awaits,
}

/// The read a record's bound or a drop for no window has made, with the
/// record's window and stamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GapRead {
    /// The newest copy's or foreground change's window.
    pub window: isize,
    /// Its observation, which the read's report is stamped with.
    pub stamp_us: u64,
}

/// An open record at its bound, for the shell's test of its top-level
/// window against the foreground.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtBound {
    /// The record's top-level window.
    pub top_level: isize,
    /// Its stamp, which its title is stamped with.
    pub stamp_us: u64,
    /// Its trace, which its title carries.
    pub trace: TraceId,
    /// What it waits for.
    pub awaits: Awaits,
}

/// What the worker's drop of a copy because its window is UIA did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyDropped {
    /// A report observed with it, or a newer copy of the record already,
    /// stands for it: nothing opens.
    Answered,
    /// A drop for no window above the sender observed with it has its
    /// focus read at once, in its place, stamped with its observation.
    ReadNow,
    /// It opened the record, whose bound passes at `due_us`; an open record
    /// of another top-level window, if any, was closed first.
    Opened {
        /// When the bound passes.
        due_us: u64,
    },
    /// It restamped the record open in its top-level window.
    Restamped,
}

/// The record, and the reports kept for the look-back.
#[cfg_attr(test, derive(Clone))]
#[derive(Debug, Default)]
pub struct GapRecord {
    open: Option<Open>,
    /// The newest report for each top-level window, by its observation,
    /// a whole-window focus never among them.
    reported: BTreeMap<isize, u64>,
    /// The newest observation of a UIA focus whose sender had no window
    /// above it.
    last_no_window_us: Option<u64>,
}

/// Whether a report observed at `reported_us` answers a copy observed at
/// `copy_us` in its own top-level window: up to [`ANSWERED_BEFORE_US`]
/// before it, or after it.
fn answers(reported_us: u64, copy_us: u64) -> bool {
    reported_us.saturating_add(ANSWERED_BEFORE_US) >= copy_us
}

impl GapRecord {
    /// Whether a record is open.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Whether an open record's bound has not been handled yet, which the
    /// idle answer waits for: past it, the record waits for the
    /// application with no timer, as long as the application takes.
    pub fn pending(&self) -> bool {
        self.open.is_some_and(|open| open.bound != Bound::Passed)
    }

    /// What the open record waits for.
    pub fn awaits(&self) -> Option<Awaits> {
        self.open.map(|open| open.awaits)
    }

    /// The open record's trace: its newest copy's or foreground change's.
    pub fn trace(&self) -> Option<TraceId> {
        self.open.map(|open| open.trace)
    }

    /// When the open record's bound passes, on the observation clock.
    pub fn due_us(&self) -> Option<u64> {
        self.open.map(|open| open.due_us)
    }

    /// When the open record's bound passes, while it is waited for: what a
    /// timer waits for.
    pub fn waiting_due_us(&self) -> Option<u64> {
        self.open
            .filter(|open| open.bound == Bound::Waiting)
            .map(|open| open.due_us)
    }

    /// The open record at its bound, whose bound is now being handled, so
    /// it is not waited for again ([`Bound::Queued`]); `None` with no record
    /// waiting for its bound.
    pub fn at_bound(&mut self) -> Option<AtBound> {
        let open = self
            .open
            .as_mut()
            .filter(|open| open.bound == Bound::Waiting)?;
        open.bound = Bound::Queued;
        Some(AtBound {
            top_level: open.top_level,
            stamp_us: open.stamp_us,
            trace: open.trace,
            awaits: open.awaits,
        })
    }

    /// The open record's bound is handled with no read of its own: it waits
    /// for a report with no timer.
    pub fn bound_passed(&mut self) {
        if let Some(open) = self.open.as_mut() {
            open.bound = Bound::Passed;
        }
    }

    /// Closes the open record, its window found unrelated to the
    /// foreground at its bound.
    pub fn close(&mut self) {
        self.open = None;
    }

    /// The worker dropped a copy in `window`, of top-level window
    /// `top_level`, observed at `at_us`, because it judges the window UIA.
    /// With a record open in its top-level window it restamps it, when
    /// newer, to wait for the application's focus, its bound unchanged; an
    /// older copy changes nothing. A newer copy of another top-level window
    /// closes the record open there first. With no record open, the
    /// look-back: a report for its top-level window observed with it stands
    /// for it; otherwise a drop for no window observed with it has it read
    /// at once; otherwise it opens the record.
    pub fn copy_dropped(
        &mut self,
        window: isize,
        top_level: isize,
        (at_us, trace): (u64, TraceId),
    ) -> CopyDropped {
        if let Some(open) = self.open {
            if at_us <= open.stamp_us {
                return CopyDropped::Answered;
            }
            if open.top_level == top_level {
                self.open = Some(Open {
                    window,
                    stamp_us: at_us,
                    trace,
                    awaits: Awaits::Focus,
                    ..open
                });
                return CopyDropped::Restamped;
            }
            self.open = None;
        }
        if self
            .reported
            .get(&top_level)
            .is_some_and(|&reported_us| answers(reported_us, at_us))
        {
            return CopyDropped::Answered;
        }
        if self
            .last_no_window_us
            .is_some_and(|no_window_us| answers(no_window_us, at_us))
        {
            return CopyDropped::ReadNow;
        }
        let due_us = at_us.saturating_add(BOUND_US);
        self.open = Some(Open {
            window,
            top_level,
            stamp_us: at_us,
            trace,
            due_us,
            bound: Bound::Waiting,
            awaits: Awaits::Focus,
        });
        CopyDropped::Opened { due_us }
    }

    /// A foreground change into `window`, a top-level window of the
    /// application in front by the outpost's test, observed at `at_us`,
    /// waiting for what `awaits` says: it opens the record, closing one
    /// open in another top-level window, or restamps the one open in its
    /// own; either way it arms a new bound a second from the change. A
    /// record waiting for a focus keeps waiting for one. An older change
    /// changes nothing.
    pub fn foreground(&mut self, window: isize, (at_us, trace): (u64, TraceId), awaits: Awaits) {
        let awaits = match self.open {
            Some(open) if at_us <= open.stamp_us => return,
            Some(open) if open.top_level == window && open.awaits == Awaits::Focus => Awaits::Focus,
            _ => awaits,
        };
        self.open = Some(Open {
            window,
            top_level: window,
            stamp_us: at_us,
            trace,
            due_us: at_us.saturating_add(BOUND_US),
            bound: Bound::Waiting,
            awaits,
        });
    }

    /// The outpost sent Core a focus observed at `at_us`, in top-level
    /// window `top_level` when its window facts name one, a whole-window
    /// focus when `whole_window`: kept for the look-back unless a
    /// whole-window focus, and closing the open record it answers. Whether
    /// it closed the record.
    pub fn reported(&mut self, top_level: Option<isize>, at_us: u64, whole_window: bool) -> bool {
        if !whole_window && let Some(top_level) = top_level {
            let newest = self.reported.entry(top_level).or_insert(at_us);
            *newest = (*newest).max(at_us);
        }
        let Some(open) = self.open else {
            return false;
        };
        let closes = if top_level == Some(open.top_level) {
            answers(at_us, open.stamp_us) && (!whole_window || open.awaits == Awaits::Title)
        } else {
            at_us > open.stamp_us
        };
        if closes {
            self.open = None;
        }
        closes
    }

    /// The listener dropped a UIA focus observed at `at_us` because its
    /// sender had no window above it: kept, and the open record it answers
    /// is closed, to be read at once.
    pub fn no_window(&mut self, at_us: u64) -> Option<GapRead> {
        let newest = self.last_no_window_us.get_or_insert(at_us);
        *newest = (*newest).max(at_us);
        let open = self.open.filter(|open| answers(at_us, open.stamp_us))?;
        self.open = None;
        Some(GapRead {
            window: open.window,
            stamp_us: open.stamp_us,
        })
    }

    /// The bound's entry is handled at `now_us`: the record still open, its
    /// bound passed, is to be read, and stays open until a report closes
    /// it; `Err` with its due time when the bound has not passed yet, as
    /// when the timer fired a little early, or a foreground change armed a
    /// new bound since, and the bound is waited for again; `Ok(None)` with
    /// no record open.
    pub fn bound_reached(&mut self, now_us: u64) -> Result<Option<GapRead>, u64> {
        match self.open.as_mut() {
            // A second entry, queued at a bound a foreground change armed
            // while the first waited: the first's read is the record's one.
            None => Ok(None),
            Some(open) if open.bound == Bound::Passed => Ok(None),
            Some(open) if now_us < open.due_us => {
                open.bound = Bound::Waiting;
                Err(open.due_us)
            }
            Some(open) => {
                open.bound = Bound::Passed;
                Ok(Some(GapRead {
                    window: open.window,
                    stamp_us: open.stamp_us,
                }))
            }
        }
    }

    /// `hwnd` was destroyed: the report kept for it as a top-level window
    /// goes.
    pub fn window_destroyed(&mut self, hwnd: isize) {
        self.reported.remove(&hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP: isize = 0x10;
    const OTHER_TOP: isize = 0x20;
    const WINDOW: isize = 0x11;

    /// An event observed at `at_us`, its trace numbered by its observation.
    fn at(at_us: u64) -> (u64, TraceId) {
        (at_us, TraceId::from_raw(at_us))
    }

    /// The open record's trace is its newest copy's or foreground change's,
    /// for its bound's entry; an older copy leaves it.
    #[test]
    fn the_records_trace_is_its_newest_stamps() {
        let mut record = GapRecord::default();
        assert_eq!(record.trace(), None);
        record.copy_dropped(WINDOW, TOP, at(1_000));
        assert_eq!(record.trace(), Some(TraceId::from_raw(1_000)));
        record.copy_dropped(0x12, TOP, at(2_000));
        assert_eq!(record.trace(), Some(TraceId::from_raw(2_000)));
        record.copy_dropped(0x12, TOP, at(1_500));
        assert_eq!(record.trace(), Some(TraceId::from_raw(2_000)));
        record.foreground(TOP, at(4_000), Awaits::Title);
        assert_eq!(record.trace(), Some(TraceId::from_raw(4_000)));
    }

    /// A record whose focus never comes is read at its bound with its
    /// window and stamp, and stays open until a report closes it, with no
    /// timer: its bound is passed, which the idle answer no longer waits
    /// for.
    #[test]
    fn a_record_read_at_its_bound_stays_open_until_a_report_closes_it() {
        let mut record = GapRecord::default();
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, at(1_000)),
            CopyDropped::Opened {
                due_us: 1_000 + BOUND_US
            }
        );
        assert_eq!(record.bound_reached(BOUND_US), Err(1_000 + BOUND_US));
        assert_eq!(
            record.bound_reached(1_000 + BOUND_US),
            Ok(Some(GapRead {
                window: WINDOW,
                stamp_us: 1_000
            }))
        );
        assert!(record.is_open());
        assert!(!record.pending());
        assert_eq!(record.waiting_due_us(), None);
        assert!(record.reported(Some(TOP), 1_000, false));
        assert!(!record.is_open());
    }

    /// A record at its bound is not waited for again while its bound is
    /// handled; its entry handled before the bound passes waits for it
    /// again; a copy's restamp keeps the bound.
    #[test]
    fn a_bound_being_handled_is_waited_for_again_when_handled_early() {
        let mut record = GapRecord::default();
        let _ = record.copy_dropped(WINDOW, TOP, at(1_000));
        assert_eq!(record.waiting_due_us(), Some(1_000 + BOUND_US));
        assert_eq!(
            record.at_bound(),
            Some(AtBound {
                top_level: TOP,
                stamp_us: 1_000,
                trace: TraceId::from_raw(1_000),
                awaits: Awaits::Focus,
            })
        );
        assert_eq!(record.waiting_due_us(), None);
        assert!(record.pending());
        let _ = record.copy_dropped(WINDOW, TOP, at(2_000));
        assert_eq!(record.waiting_due_us(), None, "the restamp keeps it");
        assert_eq!(record.due_us(), Some(1_000 + BOUND_US));
        assert_eq!(record.bound_reached(BOUND_US), Err(1_000 + BOUND_US));
        assert_eq!(record.waiting_due_us(), Some(1_000 + BOUND_US));
    }

    /// A report placed by its window facts' top-level window closes the
    /// record open there, and one in another top-level window only when
    /// observed after the record's stamp.
    #[test]
    fn a_report_is_placed_by_its_window_facts_top_level_window() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, at(200_000));
        assert!(!record.reported(Some(OTHER_TOP), 150_000, false));
        assert!(record.is_open());
        assert!(record.reported(Some(TOP), 150_000, false), "within 100 ms");
        assert!(!record.is_open());
    }

    /// A report in another top-level window, or with no window facts,
    /// closes only a record stamped before it.
    #[test]
    fn a_report_of_another_window_closes_only_an_older_record() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, at(1_000));
        assert!(!record.reported(Some(OTHER_TOP), 1_000, false));
        assert!(!record.reported(None, 900, false));
        assert!(record.is_open());
        assert!(record.reported(None, 1_001, false));
        assert!(!record.is_open());
        record.copy_dropped(WINDOW, TOP, at(5_000));
        assert!(record.reported(Some(OTHER_TOP), 5_001, false));
    }

    /// A whole-window focus in a record's own top-level window neither
    /// closes a record waiting for the application's focus nor stands for
    /// a copy in the look-back, so a frame's report does not stand for its
    /// list's copy; one waiting for the window's title it closes.
    #[test]
    fn a_whole_window_focus_neither_closes_nor_stands_for_its_own_windows_record() {
        let mut record = GapRecord::default();
        assert!(!record.reported(Some(TOP), 1_000, true));
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, at(1_050)),
            CopyDropped::Opened {
                due_us: 1_050 + BOUND_US
            },
            "the frame's report stands for nothing"
        );
        assert!(!record.reported(Some(TOP), 1_100, true));
        assert!(record.is_open());
        record.close();
        record.foreground(TOP, at(2_000), Awaits::Title);
        assert!(record.reported(Some(TOP), 2_000, true));
        assert!(!record.is_open());
    }

    /// A report of a copy's own focus resolved before the copy is judged,
    /// observed up to 100 ms before the copy, stands for it: the copy opens
    /// no record. One observed earlier still does not.
    #[test]
    fn a_report_before_its_copy_is_judged_opens_no_record() {
        let mut record = GapRecord::default();
        record.reported(Some(TOP), 1_000_000, false);
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, at(1_000_000 + ANSWERED_BEFORE_US)),
            CopyDropped::Answered
        );
        assert!(!record.is_open());
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, at(1_000_001 + ANSWERED_BEFORE_US)),
            CopyDropped::Opened {
                due_us: 1_000_001 + ANSWERED_BEFORE_US + BOUND_US
            }
        );
    }

    /// A report in another top-level window, sent after a copy's own,
    /// leaves the copy's own report standing for it.
    #[test]
    fn a_report_in_another_top_level_window_leaves_a_copys_own_report_standing() {
        let mut record = GapRecord::default();
        record.reported(Some(TOP), 1_000, false);
        record.reported(Some(OTHER_TOP), 5_000, false);
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, at(2_000)),
            CopyDropped::Answered
        );
    }

    /// A give-up of the listener's, as every drop but one for no window,
    /// carries nothing for the record, which stays open to its bound.
    #[test]
    fn a_give_up_leaves_the_record_to_its_bound() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, at(1_000));
        assert!(record.pending());
        assert_eq!(
            record.bound_reached(1_000 + BOUND_US),
            Ok(Some(GapRead {
                window: WINDOW,
                stamp_us: 1_000
            }))
        );
    }

    /// A later copy the worker drops for UIA in the record's top-level
    /// window restamps it, its bound unchanged: a report observed more than
    /// 100 ms before the new stamp no longer closes it, and its read is
    /// stamped at the new copy.
    #[test]
    fn a_later_copy_in_the_records_window_restamps_it() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, at(0));
        assert_eq!(
            record.copy_dropped(0x12, TOP, at(150_000)),
            CopyDropped::Restamped
        );
        assert!(!record.reported(Some(TOP), 5_000, false));
        assert_eq!(
            record.bound_reached(BOUND_US),
            Ok(Some(GapRead {
                window: 0x12,
                stamp_us: 150_000
            }))
        );
    }

    /// A foreground change opens a record waiting for what it says, and a
    /// later one into the same window restamps it and arms a new bound a
    /// second from it, its bound waited for again, while a record waiting
    /// for a focus keeps waiting for one; an older change changes nothing.
    #[test]
    fn a_foreground_change_opens_or_restamps_the_record_and_arms_its_bound() {
        let mut record = GapRecord::default();
        record.foreground(TOP, at(1_000), Awaits::Title);
        assert_eq!(record.waiting_due_us(), Some(1_000 + BOUND_US));
        record.bound_passed();
        assert_eq!(record.waiting_due_us(), None);
        record.foreground(TOP, at(5_000), Awaits::Title);
        assert_eq!(record.waiting_due_us(), Some(5_000 + BOUND_US));
        record.foreground(TOP, at(4_000), Awaits::Title);
        assert_eq!(record.trace(), Some(TraceId::from_raw(5_000)));
        record.copy_dropped(WINDOW, TOP, at(6_000));
        record.foreground(TOP, at(7_000), Awaits::Title);
        assert_eq!(
            record.at_bound().map(|at| at.awaits),
            Some(Awaits::Focus),
            "a copy showed the window UIA"
        );
    }

    /// The stamp only moves forward: an older copy, as in a focus run
    /// tried newest first, neither restamps the record nor answers it, in
    /// either top-level window.
    #[test]
    fn an_older_copy_neither_restamps_nor_answers_the_record() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, at(50_000));
        assert_eq!(
            record.copy_dropped(0x12, TOP, at(0)),
            CopyDropped::Answered,
            "an older copy in the record's window opens nothing"
        );
        assert_eq!(
            record.copy_dropped(0x21, OTHER_TOP, at(10_000)),
            CopyDropped::Answered,
            "an older copy in another window answers nothing"
        );
        assert_eq!(
            record.bound_reached(50_000 + BOUND_US),
            Ok(Some(GapRead {
                window: WINDOW,
                stamp_us: 50_000
            }))
        );
    }

    /// A drop for no window above the sender, resolved before its copy is
    /// judged and observed up to 100 ms before it, has the copy read at
    /// once in its place, and no record opens.
    #[test]
    fn a_drop_for_no_window_before_its_copy_has_the_copy_read_at_once() {
        let mut record = GapRecord::default();
        assert_eq!(record.no_window(0), None);
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, at(20_000)),
            CopyDropped::ReadNow
        );
        assert!(!record.is_open());
    }

    /// A drop for no window above the sender, resolved after its copy
    /// opened the record, closes it, to be read at once, stamped with the
    /// copy's observation.
    #[test]
    fn a_drop_for_no_window_after_its_copy_closes_the_record_and_reads_at_once() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, at(0));
        assert_eq!(
            record.no_window(10_000),
            Some(GapRead {
                window: WINDOW,
                stamp_us: 0
            })
        );
        assert!(!record.is_open());
    }

    /// A report kept for a top-level window goes when the window is
    /// destroyed, so a later copy in a window reusing its handle opens a
    /// record.
    #[test]
    fn a_destroyed_windows_report_is_pruned() {
        let mut record = GapRecord::default();
        record.reported(Some(TOP), 1_000, false);
        record.window_destroyed(TOP);
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, at(2_000)),
            CopyDropped::Opened {
                due_us: 2_000 + BOUND_US
            }
        );
    }

    /// A bound's entry handled after the record's bound has passed, as a
    /// second entry queued at a bound a later foreground change armed,
    /// reads nothing: the record has one read.
    #[test]
    fn a_second_bound_entry_reads_nothing() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, at(1_000));
        assert!(
            record
                .bound_reached(1_000 + BOUND_US)
                .is_ok_and(|read| read.is_some())
        );
        assert_eq!(record.bound_reached(2_000 + BOUND_US), Ok(None));
        assert!(record.is_open());
    }
}
