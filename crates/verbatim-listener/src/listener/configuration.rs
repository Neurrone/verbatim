//! The listener's configuration: what it judges its own reads under, set
//! by Core alone (`CoreToListener::Configuration`), at its launch through
//! its arguments and then on every change (D14, as amended on 2026-11-19;
//! `books/dev/src/explanation/design/ownership.md`, "No back-channel").
//!
//! One setting today: whether background progress bars are reported. While
//! they are not, the listener skips the read of a classic value change
//! whose tell's window the acceptance test refuses, and drops a UI
//! Automation bar's change once its nearest window is known and refused,
//! as NVDA's acceptance drops a background value change before it makes
//! the object (Dickson, 2026-11-19). Core still judges every tell and
//! every bar's change against its own setting: the skip makes no fact.
//! A change judged before Core's command reaches the I/O thread is judged
//! under the value one message old; the tone lost right after the setting
//! is turned on is an accepted limit (Dickson, 2026-11-19).

use std::sync::{Mutex, PoisonError};

use verbatim_model::event_kind::EventKind;
use verbatim_model::tell::AcceptanceFacts;
use verbatim_win32::window_facts::{ReadOnce, WindowPart, WindowReads};

/// Whether background progress bars are reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BackgroundBars {
    /// Reported: every value change is read, and Core judges the bar.
    Reported,
    /// Not reported: a value change whose window acceptance refuses is not
    /// read.
    NotReported,
}

impl BackgroundBars {
    /// The value Core's setting `report_background_progress_bars` names.
    pub(crate) fn of(report_background_progress_bars: bool) -> Self {
        if report_background_progress_bars {
            Self::Reported
        } else {
            Self::NotReported
        }
    }
}

/// What became of a `WinEvent`'s tell, for the value path that follows it
/// on the `WinEvent` thread.
#[derive(Debug)]
pub(crate) enum Told {
    /// Sent in this event's step, with the window facts it carries, read
    /// once for the tell and judged here a second time.
    Sent(Box<AcceptanceFacts>),
    /// Kept by the foreground hold: its facts are read at its release,
    /// possibly on the hold's timer thread, after the value path has run.
    Held,
    /// Not told: a local drop took it, or it is no tell (a window's
    /// destruction).
    Dropped,
}

/// The listener's configuration, a monitor: one setter, from the I/O
/// thread's handling of Core's command alone, and one getter, which returns
/// a copy. Neither runs a caller's code, so no guard crosses a call; the
/// `WinEvent` thread, the UI Automation callback and the reading threads
/// read it as they judge.
pub(crate) struct ListenerConfiguration {
    background_bars: Mutex<BackgroundBars>,
}

impl ListenerConfiguration {
    /// The configuration the listener was launched with.
    pub(crate) fn new(background_bars: BackgroundBars) -> Self {
        Self {
            background_bars: Mutex::new(background_bars),
        }
    }

    /// Sets the value Core's last command named.
    pub(crate) fn set(&self, background_bars: BackgroundBars) {
        *self
            .background_bars
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = background_bars;
    }

    /// The value in force now.
    pub(crate) fn background_bars(&self) -> BackgroundBars {
        *self
            .background_bars
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Whether a classic value change whose tell went as `told` is read, under
/// `bars`: one not told is never read, as its local drop says (a window of
/// Windows Live Messenger's hidden class must not be sent `WM_GETOBJECT`;
/// an invalid, hung or unreadable window's change the value path drops
/// itself); one held by the foreground hold is read, its facts not yet
/// read; one sent is read while background bars are reported, and
/// otherwise only when acceptance takes its window, judged by Core's own
/// function on the facts its tell carries.
pub(crate) fn reads_value_change(bars: BackgroundBars, told: &Told) -> bool {
    match (told, bars) {
        (Told::Dropped, _) => false,
        (Told::Held, _) | (Told::Sent(_), BackgroundBars::Reported) => true,
        (Told::Sent(facts), BackgroundBars::NotReported) => {
            verbatim_core::accepts(EventKind::ValueChange, Some(facts), false)
        }
    }
}

/// Whether a UI Automation bar's change in `window`, its nearest window, is
/// read, under `bars`: always while background bars are reported, with no
/// window fact read; otherwise only when acceptance takes the window, its
/// facts read now through `reads`, each once, with no call into the
/// application, as NVDA judges acceptance once the nearest window is known
/// and before it makes the object (Dickson, 2026-11-19).
pub(crate) fn admits_bar(bars: BackgroundBars, reads: &impl WindowReads, window: isize) -> bool {
    match bars {
        BackgroundBars::Reported => true,
        BackgroundBars::NotReported => {
            let reads = ReadOnce::new(reads);
            let facts = WindowPart::read(&reads, window, None).with_foreground(&reads);
            verbatim_core::accepts(EventKind::ValueChange, Some(&facts), false)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[test]
    fn the_configuration_returns_what_its_setter_last_set() {
        let configuration = ListenerConfiguration::new(BackgroundBars::NotReported);
        assert_eq!(configuration.background_bars(), BackgroundBars::NotReported);
        configuration.set(BackgroundBars::Reported);
        assert_eq!(configuration.background_bars(), BackgroundBars::Reported);
        configuration.set(BackgroundBars::NotReported);
        assert_eq!(configuration.background_bars(), BackgroundBars::NotReported);
    }

    /// The window in front.
    const FRONT: isize = 0x10;
    /// A window under a topmost root.
    const UNDER_TOPMOST: isize = 0x20;
    /// The topmost root of [`UNDER_TOPMOST`].
    const TOPMOST_ROOT: isize = 0x21;
    /// A window unrelated to the foreground, not topmost.
    const BACKGROUND: isize = 0x30;
    /// The desktop window.
    const DESKTOP: isize = 0x01;

    /// A desktop of three windows, each read counted.
    #[derive(Default)]
    struct Desk {
        reads: Cell<u32>,
    }

    impl Desk {
        fn count<T>(&self, value: T) -> T {
            self.reads.set(self.reads.get() + 1);
            value
        }
    }

    impl WindowReads for Desk {
        fn class(&self, _window: isize) -> String {
            self.count("Bar host".to_owned())
        }
        fn owner(&self, _window: isize) -> (u32, u32) {
            self.count((1, 2))
        }
        fn console_host(&self, _window: isize) -> Option<u32> {
            self.count(None)
        }
        fn parent(&self, _window: isize) -> isize {
            self.count(DESKTOP)
        }
        fn root(&self, window: isize) -> isize {
            self.count(if window == UNDER_TOPMOST {
                TOPMOST_ROOT
            } else {
                window
            })
        }
        fn root_owner(&self, window: isize) -> isize {
            self.count(if window == UNDER_TOPMOST {
                TOPMOST_ROOT
            } else {
                window
            })
        }
        fn is_child(&self, _parent: isize, _child: isize) -> bool {
            self.count(false)
        }
        fn topmost(&self, window: isize) -> bool {
            self.count(window == TOPMOST_ROOT)
        }
        fn desktop(&self) -> isize {
            self.count(DESKTOP)
        }
        fn foreground(&self) -> isize {
            self.count(FRONT)
        }
        fn active_window(&self) -> isize {
            self.count(FRONT)
        }
        fn hidden_frame(&self, _window: isize) -> bool {
            self.count(false)
        }
    }

    /// The UI Automation callback's gate for a bar with its own window,
    /// whose job is pushed exactly when it admits the change: the window
    /// in front, and one under a topmost root, are admitted in both
    /// states; a background window only while background bars are
    /// reported. While they are, no window fact is read; while they are
    /// not, the gate's reads are the cost the performance ledger records.
    #[test]
    fn the_callbacks_gate_admits_a_bar_by_its_windows_acceptance() {
        let judged = |bars, window| {
            let desk = Desk::default();
            let admitted = admits_bar(bars, &desk, window);
            (admitted, desk.reads.get())
        };
        assert_eq!(
            [FRONT, UNDER_TOPMOST, BACKGROUND]
                .map(|window| judged(BackgroundBars::Reported, window)),
            [(true, 0), (true, 0), (true, 0)]
        );
        assert_eq!(
            [FRONT, UNDER_TOPMOST, BACKGROUND]
                .map(|window| judged(BackgroundBars::NotReported, window)),
            [(true, 11), (true, 19), (false, 15)]
        );
    }

    #[test]
    fn a_value_change_is_read_as_its_tell_went() {
        let facts = |window| {
            let desk = Desk::default();
            Told::Sent(Box::new(
                WindowPart::read(&desk, window, None).with_foreground(&desk),
            ))
        };
        let read = |bars| {
            [
                reads_value_change(bars, &facts(FRONT)),
                reads_value_change(bars, &facts(UNDER_TOPMOST)),
                reads_value_change(bars, &facts(BACKGROUND)),
                reads_value_change(bars, &Told::Held),
                reads_value_change(bars, &Told::Dropped),
            ]
        };
        assert_eq!(
            read(BackgroundBars::Reported),
            [true, true, true, true, false]
        );
        assert_eq!(
            read(BackgroundBars::NotReported),
            [true, true, false, true, false]
        );
    }
}
