//! What the focus listener learns of the window in the foreground for the
//! keyboard hook (architecture section 5): the window, the thread that
//! raised its event, and whether it is a console host window, which the
//! listener reports to Core with its process's integrity level
//! ([`verbatim_model::protocol::ListenerToCore::ForegroundWindow`]). Core
//! decides from it whether the hook may hold keys back for the window and
//! which thread's keyboard layout translates its keys, and publishes that
//! in the snapshot the hook reads; the hook itself hooks no event.
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

/// The window in front as the listener reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Front {
    /// The window.
    pub(crate) window: isize,
    /// The thread its event named, or for a window in front when the
    /// listener started, the thread Windows names as its owner; 0 for a
    /// console window whose thread no event has named yet.
    pub(crate) thread: u32,
    /// Whether it is a console host window.
    pub(crate) console: bool,
}

/// What the listener does after an event: the report to send Core, if any,
/// and whether every console's events are now to be hooked, when that
/// changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Step {
    /// The window in front to report.
    pub(crate) report: Option<Front>,
    /// Hook every console's events (`Some(true)`), unhook them
    /// (`Some(false)`), or leave them as they are.
    pub(crate) console_events: Option<bool>,
}

/// The listener's state for the window in front.
#[derive(Debug, Default)]
pub(crate) struct ForegroundWatch {
    /// Whether [`ForegroundWatch::start`] has run: until then the
    /// console's events, hooked with the rest of the listener's set, are
    /// not looked at.
    started: bool,
    /// The console window in front whose thread no event has named yet.
    unnamed_console: Option<isize>,
    /// Whether every console's events are hooked.
    console_events: bool,
}

impl ForegroundWatch {
    /// The listener's hooks are in, all of its kinds hooked, the console's
    /// events among them, and `front` is in the foreground now, as local
    /// reads found it (`None` for no foreground window): it is reported,
    /// and every console's events stay hooked only if it is a console,
    /// whose thread its own event must name.
    pub(crate) fn start(&mut self, front: Option<Front>) -> Step {
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

    /// A foreground event for `front`, whose thread it names: reported,
    /// and any wait for a console's thread is over.
    pub(crate) fn foreground(&mut self, front: Front) -> Step {
        if !self.started {
            // Reported by the start, which reads the foreground after this.
            return Step::default();
        }
        self.unnamed_console = None;
        Step {
            report: Some(front),
            console_events: self.hook_consoles(false),
        }
    }

    /// A console's event, on `window`, raised by `thread`: when it is the
    /// console in front whose thread was unknown, that console is reported
    /// with it, and the console's events are unhooked.
    pub(crate) fn console_event(&mut self, window: isize, thread: u32) -> Step {
        if !self.started || self.unnamed_console != Some(window) {
            return Step::default();
        }
        self.unnamed_console = None;
        Step {
            report: Some(Front {
                window,
                thread,
                console: true,
            }),
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
    };
    const CONSOLE: Front = Front {
        window: 20,
        thread: 200,
        console: true,
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
        assert_eq!(watch.console_event(20, 201), Step::default());
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
        assert_eq!(watch.console_event(30, 300), Step::default());
        assert_eq!(
            watch.console_event(20, 201),
            Step {
                report: Some(Front {
                    window: 20,
                    thread: 201,
                    console: true,
                }),
                console_events: Some(false),
            }
        );
        // Named once: later events name nothing more.
        assert_eq!(watch.console_event(20, 202), Step::default());
    }

    #[test]
    fn a_foreground_event_names_its_thread_and_ends_any_wait() {
        let mut watch = ForegroundWatch::default();
        assert_eq!(
            watch.foreground(EDITOR),
            Step::default(),
            "before the start, which reads the foreground itself"
        );
        watch.start(Some(CONSOLE));
        assert_eq!(
            watch.foreground(CONSOLE),
            Step {
                report: Some(CONSOLE),
                console_events: Some(false),
            },
            "a console's foreground event names its thread"
        );
        assert_eq!(watch.console_event(20, 201), Step::default());
        assert_eq!(
            watch.foreground(EDITOR),
            Step {
                report: Some(EDITOR),
                console_events: None,
            }
        );
    }
}
