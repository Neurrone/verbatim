//! The outpost's gap record: the wait, after a `WinEvent` focus copy the
//! worker dropped because it judged the copy's window UIA, for a UIA focus
//! that answers it (`docs/design/focus-pipeline-rework.md`, "Who owns each
//! duplicated fact", the gap's evidence; `docs/design/listener-without-connections.md`,
//! decision 11).
//!
//! UI Automation sometimes delivers an application's focus events late or
//! never while the `WinEvent` focus still comes, or after their senders are
//! gone. The record is kept beside the verdict that dropped the copy, in the
//! intake's shared state under its lock ([`super::intake::Intake`]), so
//! nothing judges the copy's window again, and every stamp it compares is
//! on the run's one clock. It is a plain record with no call and no timer
//! of its own: the intake applies each UIA focus's outcome to it as the
//! outcome resolves its marker, the worker its own judgements of copies and
//! foreground changes, and the outpost's timer its bound
//! ([`super::gap_timer`]).
//!
//! The answers and the look-back: a UIA focus can reach the listener up to
//! 25.5 ms before its `WinEvent` (5 of 1,680 measured), so an answer
//! observed up to [`ANSWERED_BEFORE_US`] before a copy, or after it, answers
//! it. The newest answer is kept per top-level window, pruned when the
//! window is destroyed ([`GapRecord::window_destroyed`]), and one for any
//! window, so an answer resolved before an earlier copy of another window
//! is judged cannot stand in for that copy's own. A drop for no window
//! above the sender is kept apart, its newest stamp having a copy that comes
//! after it read at once, and closing an open record it answers with the
//! record's read made at once.
//!
//! The restamp: while a record is open, a later copy the worker drops for
//! UIA in its top-level window, or an accepted foreground change into that
//! window, gives it its observation and window, so the read after the
//! user's return carries the return's stamp, and an older focus's outcome
//! does not answer a newer copy. The stamp only moves forward: an older
//! copy or change, as in a focus run tried newest first, neither restamps
//! the record nor answers it. The bound stays a second from the first copy.

use std::collections::HashMap;

use crate::protocol::GapAnswer;

/// How long a record waits, from its first copy's observation, for a UIA
/// focus, in microseconds. The UIA focus of a `WinEvent` focus usually
/// comes within 1 to 35 ms in Settings, and in the same millisecond at the
/// median in File Explorer; a newly started File Explorer process's first
/// one came 222 to 514 ms after the first item's `WinEvent` in
/// `explorer_folder_window`, and 515 to 2,694 ms after the process's first
/// focus `WinEvent` in a UIA window in 55 new windows of
/// `measure_focus_explorer` (2026-10-21), all but two under a second; when
/// UI Automation's delivery fails none comes, in Settings for 14.5 seconds.
/// A read made before a late UIA focus costs one read and nothing heard
/// twice, since Core takes the late focus as a duplicate of the read's; a
/// read made late leaves the user in silence. So the bound is a second
/// rather than above every late delivery measured.
pub(crate) const BOUND_US: u64 = 1_000_000;

/// How long before a copy's observation a UIA focus may have been observed
/// and still answer it: a UIA focus can reach the listener before the
/// `WinEvent` of the same focus, by up to 25.5 ms in the runs measured
/// (5 of 1,680 `WinEvent` focuses in UIA windows, `measure_focus_explorer`,
/// 2026-10-21).
pub(crate) const ANSWERED_BEFORE_US: u64 = 100_000;

/// The open record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Open {
    /// The newest copy's or foreground change's window.
    window: isize,
    /// That window's top-level window.
    top_level: isize,
    /// The newest copy's or foreground change's observation, which its
    /// read is stamped with.
    stamp_us: u64,
    /// When its bound passes: a second from its first copy's observation.
    due_us: u64,
}

/// The read a record that closed is to be made with: the record's window
/// and stamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GapRead {
    /// The newest copy's or foreground change's window.
    pub(crate) window: isize,
    /// Its observation, which the read's report is stamped with.
    pub(crate) stamp_us: u64,
}

/// What the worker's drop of a copy because its window is UIA did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CopyDropped {
    /// An answer observed with it, or a newer copy of the record already,
    /// stands for it: nothing opens.
    Answered,
    /// A drop for no window above the sender observed with it has its
    /// focus read at once, in its place, stamped with its observation.
    ReadNow,
    /// It opened the record, whose bound passes at `due_us`; an open record
    /// of another top-level window, if any, was answered first.
    Opened {
        /// When the bound passes.
        due_us: u64,
    },
    /// It restamped the record open in its top-level window.
    Restamped,
}

/// What an event the worker accepted did to the record, besides a copy it
/// dropped for UIA ([`GapRecord::copy_dropped`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Accepted {
    /// Nothing: no record is open, the event is older than its stamp, or a
    /// copy handled through MSAA in the record's own top-level window.
    Unchanged,
    /// It restamped the record.
    Restamped,
    /// It answered the record, which closed.
    Answered,
}

/// What a UIA focus's outcome did to the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// It left the record as it was.
    Unchanged,
    /// It answered the record, which closed.
    Answered,
    /// It answered the record, which closed, to be read at once in the
    /// outcome's place.
    ReadNow(GapRead),
}

/// The record, and the answers kept for the look-back.
#[derive(Debug, Default)]
pub(crate) struct GapRecord {
    open: Option<Open>,
    /// The newest answer for each top-level window, by its observation.
    answered: HashMap<isize, u64>,
    /// The newest answer for any window.
    answered_anywhere: Option<u64>,
    /// The newest observation of a UIA focus whose sender had no window
    /// above it.
    last_no_window_us: Option<u64>,
}

/// Whether an answer observed at `answered_us` answers a copy observed at
/// `copy_us`: up to [`ANSWERED_BEFORE_US`] before it, or after it.
fn answers(answered_us: u64, copy_us: u64) -> bool {
    answered_us.saturating_add(ANSWERED_BEFORE_US) >= copy_us
}

impl GapRecord {
    /// Whether a record is open.
    pub(crate) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The worker dropped a copy in `window`, of top-level window
    /// `top_level`, observed at `at_us`, because it judges the window UIA.
    /// With a record open in its top-level window it restamps it, when
    /// newer; an older copy changes nothing. A newer copy of another
    /// top-level window answers the record open there first. With no
    /// record open, the look-back: an answer for its top-level window or
    /// for any window, observed with it, stands for it; otherwise a drop
    /// for no window observed with it has it read at once; otherwise it
    /// opens the record.
    pub(crate) fn copy_dropped(
        &mut self,
        window: isize,
        top_level: isize,
        at_us: u64,
    ) -> CopyDropped {
        if let Some(open) = self.open {
            if at_us <= open.stamp_us {
                return CopyDropped::Answered;
            }
            if open.top_level == top_level {
                self.open = Some(Open {
                    window,
                    stamp_us: at_us,
                    ..open
                });
                return CopyDropped::Restamped;
            }
            self.open = None;
        }
        let answered = self.answered.get(&top_level).copied().into_iter();
        if answered
            .chain(self.answered_anywhere)
            .any(|answered_us| answers(answered_us, at_us))
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
            due_us,
        });
        CopyDropped::Opened { due_us }
    }

    /// The worker accepted an event in `window`, of top-level window
    /// `top_level`, observed at `at_us`, other than a copy it dropped for
    /// UIA: a foreground change into the window (`restamps`), or a copy it
    /// handles through MSAA. A newer one in the record's top-level window
    /// restamps it, a foreground change only; a newer one in another
    /// top-level window answers it.
    pub(crate) fn accepted(
        &mut self,
        window: isize,
        top_level: isize,
        at_us: u64,
        restamps: bool,
    ) -> Accepted {
        let Some(open) = self.open else {
            return Accepted::Unchanged;
        };
        if at_us <= open.stamp_us {
            return Accepted::Unchanged;
        }
        if open.top_level != top_level {
            self.open = None;
            return Accepted::Answered;
        }
        if !restamps {
            return Accepted::Unchanged;
        }
        self.open = Some(Open {
            window,
            stamp_us: at_us,
            ..open
        });
        Accepted::Restamped
    }

    /// A UIA focus's outcome, observed at `at_us`, said `answer`: an answer
    /// is kept for the look-back, and answers the record when observed up
    /// to [`ANSWERED_BEFORE_US`] before its stamp, or after it, in its
    /// top-level window, or for any window; a drop for no window is kept,
    /// and has the record it answers read at once.
    pub(crate) fn outcome(&mut self, answer: GapAnswer, at_us: u64) -> Outcome {
        match answer {
            GapAnswer::LeavesOpen => Outcome::Unchanged,
            GapAnswer::Answers(top_level) => {
                let newest = match top_level {
                    Some(top_level) => self.answered.entry(top_level).or_insert(at_us),
                    None => self.answered_anywhere.get_or_insert(at_us),
                };
                *newest = (*newest).max(at_us);
                match self.open {
                    Some(open)
                        if top_level.is_none_or(|top| top == open.top_level)
                            && answers(at_us, open.stamp_us) =>
                    {
                        self.open = None;
                        Outcome::Answered
                    }
                    _ => Outcome::Unchanged,
                }
            }
            GapAnswer::ReadsNow => {
                let newest = self.last_no_window_us.get_or_insert(at_us);
                *newest = (*newest).max(at_us);
                match self.open {
                    Some(open) if answers(at_us, open.stamp_us) => {
                        self.open = None;
                        Outcome::ReadNow(GapRead {
                            window: open.window,
                            stamp_us: open.stamp_us,
                        })
                    }
                    _ => Outcome::Unchanged,
                }
            }
        }
    }

    /// The bound's entry is handled at `now_us`: the record still open, its
    /// bound passed, closes, and is to be read; `Err` with its due time
    /// when the bound has not passed yet, as when the timer fired a little
    /// early, or for an earlier record; `Ok(None)` with no record open.
    pub(crate) fn bound_reached(&mut self, now_us: u64) -> Result<Option<GapRead>, u64> {
        match self.open {
            None => Ok(None),
            Some(open) if now_us < open.due_us => Err(open.due_us),
            Some(open) => {
                self.open = None;
                Ok(Some(GapRead {
                    window: open.window,
                    stamp_us: open.stamp_us,
                }))
            }
        }
    }

    /// `hwnd` was destroyed: the answer kept for it as a top-level window
    /// goes.
    pub(crate) fn window_destroyed(&mut self, hwnd: isize) {
        self.answered.remove(&hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP: isize = 0x10;
    const OTHER_TOP: isize = 0x20;
    const WINDOW: isize = 0x11;

    /// A UIA focus's drop observed just after its copy, because its element
    /// no longer has the keyboard focus, as in fast navigation, says
    /// nothing of the copy's focus: the record stays open, and at its bound
    /// it is read with the copy's window and stamp.
    #[test]
    fn a_drop_for_no_keyboard_focus_leaves_the_gap_record_open() {
        let mut record = GapRecord::default();
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, 1_000),
            CopyDropped::Opened {
                due_us: 1_000 + BOUND_US
            }
        );
        assert_eq!(
            record.outcome(GapAnswer::LeavesOpen, 2_000),
            Outcome::Unchanged
        );
        assert_eq!(record.bound_reached(BOUND_US), Err(1_000 + BOUND_US));
        assert_eq!(
            record.bound_reached(1_000 + BOUND_US),
            Ok(Some(GapRead {
                window: WINDOW,
                stamp_us: 1_000
            }))
        );
        assert!(!record.is_open());
    }

    /// A windowless sender's read is placed by the top-level window of its
    /// nearest window: in another top-level window than the copy's, it
    /// leaves the record open; in the copy's, it answers it.
    #[test]
    fn a_windowless_senders_read_answers_the_record_in_its_nearest_windows_top_level_window() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, 1_000);
        assert_eq!(
            record.outcome(GapAnswer::Answers(Some(OTHER_TOP)), 2_000),
            Outcome::Unchanged
        );
        assert!(record.is_open());
        assert_eq!(
            record.outcome(GapAnswer::Answers(Some(TOP)), 3_000),
            Outcome::Answered
        );
        assert!(!record.is_open());
    }

    /// A UIA focus's answer resolved before the worker judges its copy,
    /// observed up to 100 ms before the copy, stands for it: the copy opens
    /// no record. One observed earlier still does not.
    #[test]
    fn an_answer_resolved_before_its_copy_is_judged_opens_no_record() {
        let mut record = GapRecord::default();
        record.outcome(GapAnswer::Answers(Some(TOP)), 1_000_000);
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, 1_000_000 + ANSWERED_BEFORE_US),
            CopyDropped::Answered
        );
        assert!(!record.is_open());
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, 1_000_001 + ANSWERED_BEFORE_US),
            CopyDropped::Opened {
                due_us: 1_000_001 + ANSWERED_BEFORE_US + BOUND_US
            }
        );
    }

    /// An answer in another top-level window, resolved after a copy's own,
    /// leaves the copy's own answer standing for it.
    #[test]
    fn an_answer_in_another_top_level_window_leaves_a_copys_own_answer_standing() {
        let mut record = GapRecord::default();
        record.outcome(GapAnswer::Answers(Some(TOP)), 1_000);
        record.outcome(GapAnswer::Answers(Some(OTHER_TOP)), 5_000);
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, 2_000),
            CopyDropped::Answered
        );
    }

    /// A newer copy the worker accepts in another top-level window of the
    /// application, handled through MSAA, or a newer accepted foreground
    /// change into one, answers the record; a copy handled through MSAA in
    /// the record's own top-level window does not.
    #[test]
    fn an_accepted_focus_in_another_top_level_window_answers_the_record() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, 1_000);
        assert_eq!(
            record.accepted(WINDOW, TOP, 2_000, false),
            Accepted::Unchanged
        );
        assert_eq!(
            record.accepted(0x21, OTHER_TOP, 3_000, false),
            Accepted::Answered
        );
        assert!(!record.is_open());
        record.copy_dropped(WINDOW, TOP, 4_000);
        assert_eq!(
            record.accepted(OTHER_TOP, OTHER_TOP, 5_000, true),
            Accepted::Answered
        );
        assert!(!record.is_open());
    }

    /// A later copy the worker drops for UIA in the record's top-level
    /// window restamps it, its bound unchanged: an outcome observed more
    /// than 100 ms before the new stamp no longer answers it, and its read
    /// is stamped at the new copy.
    #[test]
    fn a_later_copy_in_the_records_window_restamps_it() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, 0);
        assert_eq!(
            record.copy_dropped(0x12, TOP, 150_000),
            CopyDropped::Restamped
        );
        assert_eq!(
            record.outcome(GapAnswer::Answers(Some(TOP)), 5_000),
            Outcome::Unchanged
        );
        assert_eq!(
            record.bound_reached(BOUND_US),
            Ok(Some(GapRead {
                window: 0x12,
                stamp_us: 150_000
            }))
        );
    }

    /// The stamp only moves forward: an older copy, as in a focus run
    /// tried newest first, neither restamps the record nor answers it, in
    /// either top-level window, and neither does an older foreground
    /// change.
    #[test]
    fn an_older_copy_neither_restamps_nor_answers_the_record() {
        let mut record = GapRecord::default();
        record.copy_dropped(WINDOW, TOP, 50_000);
        assert_eq!(
            record.copy_dropped(0x12, TOP, 0),
            CopyDropped::Answered,
            "an older copy in the record's window opens nothing"
        );
        assert_eq!(
            record.copy_dropped(0x21, OTHER_TOP, 10_000),
            CopyDropped::Answered,
            "an older copy in another window answers nothing"
        );
        assert_eq!(
            record.accepted(0x21, OTHER_TOP, 20_000, false),
            Accepted::Unchanged
        );
        assert_eq!(record.accepted(TOP, TOP, 30_000, true), Accepted::Unchanged);
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
        assert_eq!(record.outcome(GapAnswer::ReadsNow, 0), Outcome::Unchanged);
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, 20_000),
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
        record.copy_dropped(WINDOW, TOP, 0);
        assert_eq!(
            record.outcome(GapAnswer::ReadsNow, 10_000),
            Outcome::ReadNow(GapRead {
                window: WINDOW,
                stamp_us: 0
            })
        );
        assert!(!record.is_open());
    }

    /// An answer kept for a top-level window goes when the window is
    /// destroyed, so a later copy in a window reusing its handle opens a
    /// record.
    #[test]
    fn a_destroyed_windows_answer_is_pruned() {
        let mut record = GapRecord::default();
        record.outcome(GapAnswer::Answers(Some(TOP)), 1_000);
        record.window_destroyed(TOP);
        assert_eq!(
            record.copy_dropped(WINDOW, TOP, 2_000),
            CopyDropped::Opened {
                due_us: 2_000 + BOUND_US
            }
        );
    }
}
