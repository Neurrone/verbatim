//! A reader's marks of the hung threads it has reported (ADR 0003, "A
//! frozen window"): each reader reports a hung window to Core once per
//! stall of the window's thread, since Windows judges a window hung by its
//! thread (`IsHungAppWindow`), so every window of a stalled thread hangs
//! and answers together. The listener keeps one [`Marks`] per application,
//! in its record of the application, so the application's exit drops
//! them; an outpost keeps one, for its one application.

use std::collections::BTreeSet;

use crate::tell::Hung;

/// The threads a reader has reported hung and not yet seen answer: one
/// report per stall of a thread.
///
/// | A thread's mark | Input | Result |
/// |---|---|---|
/// | Unmarked | An event of a window on it, read hung | Report the window; mark the thread |
/// | Marked | The same | Nothing |
/// | Marked | An event of a window on it, read responsive | Unmark |
/// | Any | A foreground event ([`Marks::foreground`]) | Every mark cleared |
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Marks {
    threads: BTreeSet<u32>,
}

/// What a reader read of an event's window as the event arrived, with
/// calls to Windows only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowState {
    /// The window answers: its thread, 0 when there was no window to read,
    /// which marks and unmarks nothing.
    Responsive {
        /// The thread that owns the window.
        thread: u32,
    },
    /// The window is hung or ghosted, with the report Core is sent of it
    /// when its thread is not marked.
    Hung(Hung),
}

impl Marks {
    /// An event whose window was read as `state`: the report to send, for
    /// a hung window whose thread was not marked, which marks it; a
    /// responsive window's thread is unmarked.
    pub fn step(&mut self, state: WindowState) -> Option<Hung> {
        match state {
            WindowState::Responsive { thread } => {
                self.threads.remove(&thread);
                None
            }
            WindowState::Hung(hung) => self.threads.insert(hung.window.thread).then_some(hung),
        }
    }

    /// A foreground event: every mark is cleared, since Core drops a
    /// window from its recovery set when its application loses attention,
    /// so a hung window brought back to the front is reported again.
    pub fn foreground(&mut self) {
        self.threads.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tell::WindowFacet;
    use crate::{Pid, WindowHandle};

    /// Verbatim's GUI thread in the incident (`theme_panel`, GitHub run
    /// 44883440325), and its process.
    const GUI: u32 = 672;
    const VERBATIM: Pid = Pid(9052);

    fn facet(window: u64, thread: u32) -> WindowFacet {
        WindowFacet {
            handle: WindowHandle(window),
            class: "#32770".to_owned(),
            process: Some(VERBATIM),
            thread,
        }
    }

    /// `window`, on `thread`, read hung at `at_us`.
    fn hung(window: u64, thread: u32, at_us: u64) -> WindowState {
        WindowState::Hung(Hung {
            window: facet(window, thread),
            root: facet(window, thread),
            root_owner: facet(window, thread),
            at_us,
        })
    }

    /// The windows `states`, stepped in order, report.
    fn reported(marks: &mut Marks, states: Vec<WindowState>) -> Vec<u64> {
        states
            .into_iter()
            .filter_map(|state| marks.step(state))
            .map(|hung| hung.window.handle.0)
            .collect()
    }

    /// The design's counterexample: a UI thread in a 20 s loop updates its
    /// focused progress bar 20 times while Windows calls it hung.
    #[test]
    fn twenty_drops_from_one_hung_thread_report_once() {
        let mut marks = Marks::default();
        let drops = (0..20).map(|at| hung(1_000, GUI, at)).collect();
        assert_eq!(reported(&mut marks, drops), [1_000]);
    }

    #[test]
    fn a_responsive_event_unmarks_its_thread() {
        let mut marks = Marks::default();
        let states = vec![
            hung(1_000, GUI, 1),
            WindowState::Responsive { thread: 99 },
            hung(1_000, GUI, 2),
            WindowState::Responsive { thread: GUI },
            hung(2_000, GUI, 3),
        ];
        assert_eq!(reported(&mut marks, states), [1_000, 2_000]);
    }

    /// No window to read marks and unmarks nothing.
    #[test]
    fn an_event_with_no_window_leaves_the_marks_as_they_are() {
        let mut marks = Marks::default();
        let states = vec![
            hung(1_000, GUI, 1),
            WindowState::Responsive { thread: 0 },
            hung(1_000, GUI, 2),
        ];
        assert_eq!(reported(&mut marks, states), [1_000]);
    }

    #[test]
    fn a_foreground_event_clears_every_mark_then_reports_its_own_drop() {
        let mut marks = Marks::default();
        assert_eq!(
            reported(&mut marks, vec![hung(1_000, GUI, 1), hung(3_000, 7, 2)]),
            [1_000, 3_000]
        );
        marks.foreground();
        assert_eq!(
            reported(
                &mut marks,
                vec![hung(1_000, GUI, 3), hung(3_000, 7, 4), hung(1_000, GUI, 5)]
            ),
            [1_000, 3_000]
        );
    }

    /// Every `WinEvent` of `listener.log` 823 to 861 in the incident that
    /// reached Core as no tell (the flight recorder's inputs 464 to 476),
    /// as the listener steps its marks of Verbatim's application: thread
    /// 672's windows read hung, the ghost 262832 as the Theme dialog
    /// 2359904 it stands for, and the two foreground events (the ghost's at
    /// 30.690 and the task dialog's at 31.486) clearing the marks first.
    /// The destroys, which are never dropped as hung, are left out.
    #[test]
    fn the_incident_drops_report_2359904_twice_and_131776_once() {
        let mut marks = Marks::default();
        let mut reports = Vec::new();
        let at = |ms: u64| 1_795_249_200_000_000 + ms * 1_000;
        let mut drops = |marks: &mut Marks, windows: &[(u64, u64)]| {
            for &(window, ms) in windows {
                reports.extend(
                    marks
                        .step(hung(window, GUI, at(ms)))
                        .map(|hung| (hung.window.handle.0, hung.at_us)),
                );
            }
        };
        // The Theme dialog's state change, 131776's name change, the hides
        // of 524872 and 2359904, and the ghost's show.
        drops(
            &mut marks,
            &[
                (2_359_904, 30_675),
                (131_776, 30_677),
                (524_872, 30_679),
                (2_359_904, 30_686),
                (2_359_904, 30_689),
            ],
        );
        // The ghost's foreground event.
        marks.foreground();
        // Its own drop, and its focus and name change, then the name
        // changes of thread 672's windows to 31.485.
        drops(
            &mut marks,
            &[
                (2_359_904, 30_690),
                (2_359_904, 30_692),
                (2_359_904, 30_696),
                (7_537_276, 30_973),
                (7_537_276, 31_477),
                (66_262, 31_480),
                (66_266, 31_481),
                (7_537_276, 31_482),
                (66_270, 31_483),
                (66_270, 31_483),
                (66_274, 31_484),
                (66_274, 31_484),
                (66_274, 31_484),
            ],
        );
        // The task dialog's foreground event.
        marks.foreground();
        // Its own drop, the state changes of 1573328 and the three focus
        // events.
        drops(
            &mut marks,
            &[
                (131_776, 31_486),
                (1_573_328, 31_487),
                (1_573_328, 31_487),
                (1_573_328, 31_488),
                (131_776, 31_488),
                (7_537_276, 31_488),
                (66_274, 31_489),
            ],
        );
        assert_eq!(
            reports,
            [
                (2_359_904, at(30_675)),
                (2_359_904, at(30_690)),
                (131_776, at(31_486)),
            ]
        );
    }
}
