//! What the focus listener learns of the window in the foreground: each
//! window that came to the system's foreground, with the thread that raised
//! its event, whether it is a console host window, and when the listener
//! observed it, which the listener reports to Core with its process, its
//! root owner and its process's integrity level
//! ([`verbatim_model::protocol::ListenerToCore::Foreground`]). It is the
//! only report that a window came to the foreground: Core takes from it the
//! keyboard hook's view of the foreground, attention and its foreground
//! record (`docs/design/focus-pipeline-rework-remaining.md`, "One
//! foreground change, from the listener"); the hook itself hooks no event.
//!
//! The listener sends a foreground event only for a window
//! `GetForegroundWindow` names when the event is handled, a local call: an
//! event for a window not in front is stale (the Alt+Tab staging window's,
//! already passed), raised by the application before the switch (File
//! Explorer's folder window, 140 to 400 ms early, with no later event when
//! it opens in front), or refused by Windows' foreground lock
//! (`docs/parity/focus-events.md`, "Event acceptance"). Such a window is
//! kept as the pending window, replacing any kept before, and is sent once
//! a later focus or foreground event finds it in front, with that event's
//! observation; a newer foreground event replaces it. None sent replaces
//! another.
//!
//! A console host window is the one window whose thread the listener must
//! learn from an event: asked which thread owns it, Windows names the first
//! process attached to the console (a shell), whose keyboard layout is not
//! the one the console host translates keys with, while a `WinEvent`
//! always names the thread that raised it, the console host's own (NVDA's
//! issue 10113, `consoleWindowsToThreadIDs`; `docs/crates/verbatim-input-windows.md`,
//! "Typed text"). A console's foreground event names it. When the listener
//! starts with a console window already in front, no foreground event
//! comes for it, so the listener hooks every console's events until one of
//! that window's own names its thread, and then unhooks them: a console
//! busy with output raises thousands a second (`docs/design/wake-ups.md`).
//!
//! [`ForegroundWatch`] is the pure state of that, on the listener's
//! `WinEvent` thread; the listener makes the local reads and the hooking.

/// A window that came to the foreground, as the listener reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Front {
    /// The window.
    pub window: isize,
    /// The thread its event named, or for a window in front when the
    /// listener started, the thread Windows names as its owner; 0 for a
    /// console window whose thread no event has named yet.
    pub thread: u32,
    /// Whether it is a console host window.
    pub console: bool,
    /// When the listener observed the event that reports it, in
    /// microseconds on the run's shared clock: its own event's, or for a
    /// window sent once a later event found it in front, that event's.
    pub observed_at_us: u64,
}

/// What the listener does after an event: the window in front to send
/// Core, if any, and whether every console's events are now to be hooked,
/// when that changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// The window in front to report.
    pub report: Option<Front>,
    /// Hook every console's events (`Some(true)`), unhook them
    /// (`Some(false)`), or leave them as they are.
    pub console_events: Option<bool>,
}

/// The listener's state for the window in front.
#[derive(Debug, Default)]
pub struct ForegroundWatch {
    /// Whether [`ForegroundWatch::start`] has run: until then the
    /// console's events, hooked with the rest of the listener's set, are
    /// not looked at.
    started: bool,
    /// The console window in front whose thread no event has named yet.
    unnamed_console: Option<isize>,
    /// Whether every console's events are hooked.
    console_events: bool,
    /// The newest foreground event's window, when it was not in front as
    /// the event was handled: sent once a later focus or foreground event
    /// finds it in front.
    pending: Option<Front>,
}

impl ForegroundWatch {
    /// The listener's hooks are in, all of its kinds hooked, the console's
    /// events among them, and `front` is in the foreground now, as local
    /// reads found it (`None` for no foreground window): it is reported,
    /// and every console's events stay hooked only if it is a console,
    /// whose thread its own event must name.
    pub fn start(&mut self, front: Option<Front>) -> Step {
        self.started = true;
        self.console_events = true;
        let report = front.map(|front| {
            if front.console {
                self.unnamed_console = Some(front.window);
                Front { thread: 0, ..front }
            } else {
                front
            }
        });
        Step {
            report,
            console_events: self.hook_consoles(self.unnamed_console.is_some()),
        }
    }

    /// A foreground event for `front`, whose thread it names, its window
    /// in front as the event is handled when `in_front`: reported, and any
    /// wait for a console's thread is over. A window not in front replaces
    /// the pending window and is reported once a later event finds it in
    /// front ([`event_seen`](Self::event_seen)).
    pub fn foreground(&mut self, front: Front, in_front: bool) -> Step {
        if !self.started {
            // Reported by the start, which reads the foreground after this.
            return Step::default();
        }
        if in_front {
            self.pending = None;
            self.sent(front)
        } else {
            self.pending = Some(front);
            Step::default()
        }
    }

    /// Whether a window not in front at its foreground event waits to be
    /// sent, for which a focus event reads the foreground window.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// A focus event the listener received at `observed_at_us`, with
    /// `foreground` the window `GetForegroundWindow` names now: the pending
    /// window, when it is that window, is reported with this event's
    /// observation.
    pub fn event_seen(&mut self, foreground: isize, observed_at_us: u64) -> Step {
        match self.pending {
            Some(pending) if self.started && pending.window == foreground => {
                self.pending = None;
                self.sent(Front {
                    observed_at_us,
                    ..pending
                })
            }
            _ => Step::default(),
        }
    }

    /// A console's event, on `window`, raised by `thread`, observed at
    /// `observed_at_us`: when it is the console in front whose thread was
    /// unknown, that console is reported with it, and the console's events
    /// are unhooked.
    pub fn console_event(&mut self, window: isize, thread: u32, observed_at_us: u64) -> Step {
        if !self.started || self.unnamed_console != Some(window) {
            return Step::default();
        }
        self.unnamed_console = None;
        Step {
            report: Some(Front {
                window,
                thread,
                console: true,
                observed_at_us,
            }),
            console_events: self.hook_consoles(false),
        }
    }

    /// `front` is reported, a window in front whose thread its event named:
    /// any wait for a console's thread is over.
    fn sent(&mut self, front: Front) -> Step {
        self.unnamed_console = None;
        Step {
            report: Some(front),
            console_events: self.hook_consoles(false),
        }
    }

    /// Whether the console's events change to `hooked`: `Some(hooked)`
    /// when they do.
    fn hook_consoles(&mut self, hooked: bool) -> Option<bool> {
        (self.console_events != hooked).then(|| {
            self.console_events = hooked;
            hooked
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EDITOR: Front = Front {
        window: 10,
        thread: 100,
        console: false,
        observed_at_us: 1_000,
    };
    const CONSOLE: Front = Front {
        window: 20,
        thread: 200,
        console: true,
        observed_at_us: 2_000,
    };
    const EXPLORER: Front = Front {
        window: 30,
        thread: 300,
        console: false,
        observed_at_us: 3_000,
    };

    #[test]
    fn a_window_in_front_at_the_start_is_reported_and_the_consoles_unhooked() {
        let mut watch = ForegroundWatch::default();
        assert_eq!(
            watch.start(Some(EDITOR)),
            Step {
                report: Some(EDITOR),
                console_events: Some(false),
            }
        );
        // A console's event is not looked at.
        assert_eq!(watch.console_event(20, 201, 5_000), Step::default());
    }

    #[test]
    fn no_window_in_front_at_the_start_reports_nothing() {
        assert_eq!(
            ForegroundWatch::default().start(None),
            Step {
                report: None,
                console_events: Some(false),
            }
        );
    }

    #[test]
    fn a_console_in_front_at_the_start_waits_for_its_own_event_to_name_its_thread() {
        let mut watch = ForegroundWatch::default();
        assert_eq!(
            watch.start(Some(CONSOLE)),
            Step {
                report: Some(Front {
                    thread: 0,
                    ..CONSOLE
                }),
                console_events: None,
            },
            "the consoles' events stay hooked, as installed"
        );
        // Another console's event names nothing.
        assert_eq!(watch.console_event(30, 300, 5_000), Step::default());
        assert_eq!(
            watch.console_event(20, 201, 6_000),
            Step {
                report: Some(Front {
                    window: 20,
                    thread: 201,
                    console: true,
                    observed_at_us: 6_000,
                }),
                console_events: Some(false),
            }
        );
        // Named once: later events name nothing more.
        assert_eq!(watch.console_event(20, 202, 7_000), Step::default());
    }

    #[test]
    fn a_foreground_event_names_its_thread_and_ends_any_wait() {
        let mut watch = ForegroundWatch::default();
        assert_eq!(
            watch.foreground(EDITOR, true),
            Step::default(),
            "before the start, which reads the foreground itself"
        );
        watch.start(Some(CONSOLE));
        assert_eq!(
            watch.foreground(CONSOLE, true),
            Step {
                report: Some(CONSOLE),
                console_events: Some(false),
            },
            "a console's foreground event names its thread"
        );
        assert_eq!(watch.console_event(20, 201, 5_000), Step::default());
        assert_eq!(
            watch.foreground(EDITOR, true),
            Step {
                report: Some(EDITOR),
                console_events: None,
            }
        );
    }

    /// Every foreground event in front is reported with its own
    /// observation, in the order handled, a change back to an earlier
    /// window included: none replaces another.
    #[test]
    fn every_foreground_event_is_sent_with_its_stamp_none_replaced() {
        let mut watch = ForegroundWatch::default();
        watch.start(Some(EDITOR));
        let returned = Front {
            observed_at_us: 4_000,
            ..EDITOR
        };
        let reports: Vec<Option<Front>> = [EXPLORER, returned, EXPLORER]
            .into_iter()
            .map(|front| watch.foreground(front, true).report)
            .collect();
        assert_eq!(reports, [Some(EXPLORER), Some(returned), Some(EXPLORER)]);
    }

    /// A foreground event whose window is not in front as it is handled is
    /// sent once a later focus event finds the window in front, with that
    /// event's observation, and not before.
    #[test]
    fn a_foreground_event_for_a_window_not_in_front_is_sent_when_a_later_event_finds_it_in_front() {
        let mut watch = ForegroundWatch::default();
        watch.start(Some(EDITOR));
        assert_eq!(watch.foreground(EXPLORER, false), Step::default());
        assert!(watch.has_pending());
        assert_eq!(watch.event_seen(10, 3_500), Step::default());
        assert_eq!(
            watch.event_seen(30, 3_600),
            Step {
                report: Some(Front {
                    observed_at_us: 3_600,
                    ..EXPLORER
                }),
                console_events: None,
            }
        );
        assert!(!watch.has_pending());
        assert_eq!(watch.event_seen(30, 3_700), Step::default(), "sent once");
    }

    /// A newer foreground event replaces the pending window, whether or not
    /// its own window is in front, so the replaced one is never sent.
    #[test]
    fn a_refused_foreground_event_replaced_by_a_newer_one_is_never_sent() {
        let mut watch = ForegroundWatch::default();
        watch.start(Some(EDITOR));
        watch.foreground(EXPLORER, false);
        watch.foreground(CONSOLE, false);
        assert_eq!(watch.event_seen(30, 3_600), Step::default());
        assert_eq!(
            watch.foreground(EDITOR, true).report,
            Some(EDITOR),
            "a newer event in front"
        );
        assert_eq!(watch.event_seen(20, 3_700), Step::default());
    }
}
