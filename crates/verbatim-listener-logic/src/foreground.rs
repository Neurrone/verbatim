//! What the focus listener learns of the window in the foreground: each
//! window that came to the system's foreground, with the thread that raised
//! its event, whether it is a console host window, and when the listener
//! observed it, which the listener reports to Core with its process, its
//! root owner and its process's integrity level
//! ([`verbatim_model::protocol::ListenerToCore::Foreground`]). It is the
//! only report that a window came to the foreground: Core takes from it the
//! keyboard hook's view of the foreground, attention and its foreground
//! record (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the window title");
//! the hook itself hooks no event.
//!
//! The listener sends a foreground event only for a window
//! `GetForegroundWindow` names when the event is handled, a local call: an
//! event for a window not in front is stale (the Alt+Tab staging window's,
//! already passed), raised by the application before the switch (File
//! Explorer's folder window, 140 to 400 ms early, with no later event when
//! it opens in front), or refused by Windows' foreground lock
//! (`docs/parity/focus-events.md`, "Event acceptance"). Such a window is
//! the one pending foreground ([`Pending`]): every MSAA tell placed after
//! it is held behind it until `GetForegroundWindow` names its window or
//! [`FOREGROUND_HOLD_US`] pass, and the window is reported once an event,
//! or the hold's timer, finds it in front, always before the tells that
//! step releases; a newer foreground event replaces it. None sent
//! replaces another.
//!
//! A console host window is the one window whose thread the listener must
//! learn from an event: asked which thread owns it, Windows names the first
//! process attached to the console (a shell), whose keyboard layout is not
//! the one the console host translates keys with, while a `WinEvent`
//! always names the thread that raised it, the console host's own (NVDA's
//! issue 10113, `consoleWindowsToThreadIDs`; `books/dev/src/reference/crates/verbatim-input-windows.md`,
//! "Typed text"). A console's foreground event names it. When the listener
//! starts with a console window already in front, no foreground event
//! comes for it, so the listener hooks every console's events until one of
//! that window's own names its thread, and then unhooks them: a console
//! busy with output raises thousands a second (`books/dev/src/explanation/design/wake-ups.md`).
//!
//! [`ForegroundWatch`] is the pure state of the start and the consoles, on
//! the listener's `WinEvent` thread, and [`Pending`] the pending
//! foreground, under the lock every tell is placed under; the listener
//! makes the local reads, the hooking and the timer.

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
#[derive(Clone, Debug, Default)]
pub struct ForegroundWatch {
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

    /// Whether [`start`](Self::start) has run: until then a window found
    /// in front is not reported, since the start reads the foreground
    /// itself.
    #[must_use]
    pub fn started(&self) -> bool {
        self.started
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

    /// A window in front whose thread its event named was reported: any
    /// wait for a console's thread is over, and the consoles' events are
    /// unhooked, when that changes them.
    pub fn sent(&mut self) -> Option<bool> {
        self.unnamed_console = None;
        self.hook_consoles(false)
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

/// How long a pending foreground holds the tells placed behind it while
/// `GetForegroundWindow` does not yet name its window: NVDA's two
/// deferrals of its pump, each of up to 10 ms
/// (`IAccessibleHandler/internalWinEventHandler.py` 30, 233 to 263;
/// `core.py` 55), in microseconds.
pub const FOREGROUND_HOLD_US: u64 = 20_000;

/// The pending foreground: none, a foreground event's window that
/// `GetForegroundWindow` did not name as the event was handled, holding
/// every item placed since behind it, in order, until it does or
/// [`FOREGROUND_HOLD_US`] pass, or such a window whose hold is over,
/// awaited only to be reported. A newer foreground event replaces it and
/// counts its time again, as NVDA sets the deferred window and resets its
/// count at each foreground event
/// (`IAccessibleHandler/internalWinEventHandler.py` 175 to 177, 233 to
/// 263). Every MSAA item goes through it, so the order of those tells is
/// the order they were placed in; a UI Automation tell passes it by, as
/// NVDA's hold defers its `WinEvent` pump alone, and is sent at once
/// (Dickson, 2026-11-17). Its window is reported only by a step of the
/// `WinEvent` thread ([`step`](Self::step)), always before the items that
/// step releases, so Core takes the window as the foreground before it
/// judges them; an item placed from another thread
/// ([`place_behind`](Self::place_behind)) releases nothing.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum Pending<T> {
    /// No foreground is pending.
    #[default]
    Clear,
    /// A foreground event's window not yet in front, holding what was
    /// placed since.
    Holding {
        /// The window, as its event named it.
        front: Front,
        /// When the hold ends whatever the foreground, in microseconds.
        until_us: u64,
        /// What it holds, in order.
        held: Vec<T>,
    },
    /// A foreground event's window not in front when its hold ended,
    /// reported once an event finds it in front.
    Awaiting {
        /// The window, as its event named it.
        front: Front,
    },
}

/// What a `WinEvent`-thread step of the pending foreground is given.
#[derive(Debug)]
pub enum Seen<T> {
    /// A foreground event for `front`, with its tell, `None` for one a
    /// local drop took, which still awaits its window, as NVDA sets the
    /// deferred window before it judges the event.
    Foreground {
        /// The window, its thread and its observation.
        front: Front,
        /// Its tell.
        item: Option<T>,
    },
    /// The shell's foreground event (Program Manager or the taskbar), for
    /// `front`: reported for the keyboard hook's view as a foreground
    /// event's window is, and replacing the window awaited, but holding
    /// nothing, as NVDA defers its pump for no shell window
    /// (`IAccessibleHandler/internalWinEventHandler.py` 164 to 172).
    Shell {
        /// The window, its thread and its observation.
        front: Front,
        /// Its tell.
        item: Option<T>,
    },
    /// Any other `WinEvent`, with its tell, `None` for one not told.
    Other {
        /// Its tell.
        item: Option<T>,
        /// When the listener observed it.
        observed_at_us: u64,
    },
    /// The hold's timer went off, or a stale `WM_TIMER` that `KillTimer`
    /// left posted arrived.
    Timer,
}

/// How the hold's one timer changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimerChange {
    /// As it is.
    Keep,
    /// Set to go off this many microseconds from now.
    Arm(u64),
    /// Stopped: no hold waits for it.
    Disarm,
}

/// What a step of the pending foreground does, in order: the window to
/// report, then what is sent, then the timer's change.
#[derive(Debug, PartialEq, Eq)]
pub struct Placed<T> {
    /// The window in front to report, before what is sent.
    pub report: Option<Front>,
    /// What is sent, in order.
    pub send: Vec<T>,
    /// The timer.
    pub timer: TimerChange,
}

impl<T> Placed<T> {
    /// `send` sent, nothing reported, the timer kept.
    fn sent(send: Vec<T>) -> Self {
        Self {
            report: None,
            send,
            timer: TimerChange::Keep,
        }
    }
}

impl<T> Pending<T> {
    /// An item placed from any thread, the idle barrier's answer: held
    /// behind a hold in force, else given back to be sent now. It releases
    /// nothing, since only the `WinEvent` thread reports a window.
    pub fn place_behind(&mut self, item: T) -> Option<T> {
        if let Self::Holding { held, .. } = self {
            held.push(item);
            None
        } else {
            Some(item)
        }
    }

    /// A step of the `WinEvent` thread, `seen`, with `in_front` the window
    /// `GetForegroundWindow` names now and `now` the time. `started` says
    /// whether the foreground's watch has started: until then nothing is
    /// reported, since the start reports the window in front itself, and
    /// the hold keeps its order and its timer as after.
    pub fn step(&mut self, seen: Seen<T>, in_front: isize, now: u64, started: bool) -> Placed<T> {
        let mut placed = match seen {
            Seen::Foreground { front, item } => self.foreground(front, item, in_front, now),
            Seen::Shell { front, item } => {
                let mut placed = self.other(item, in_front, front.observed_at_us, now);
                if !matches!(self, Self::Holding { .. }) {
                    *self = if front.window == in_front {
                        placed.report = Some(front);
                        Self::Clear
                    } else {
                        Self::Awaiting { front }
                    };
                }
                placed
            }
            Seen::Other {
                item,
                observed_at_us,
            } => self.other(item, in_front, observed_at_us, now),
            Seen::Timer => self.timer(in_front, now),
        };
        if !started {
            placed.report = None;
        }
        placed
    }

    /// A foreground event for `front`: reported with whatever was held and
    /// its tell when its window is in front, else held, awaited for
    /// [`FOREGROUND_HOLD_US`] from now.
    fn foreground(
        &mut self,
        front: Front,
        item: Option<T>,
        in_front: isize,
        now: u64,
    ) -> Placed<T> {
        let (had, mut held) = match std::mem::take(self) {
            Self::Holding { held, .. } => (true, held),
            Self::Clear | Self::Awaiting { .. } => (false, Vec::new()),
        };
        held.extend(item);
        if front.window == in_front {
            return Placed {
                report: Some(front),
                send: held,
                timer: if had {
                    TimerChange::Disarm
                } else {
                    TimerChange::Keep
                },
            };
        }
        *self = Self::Holding {
            front,
            until_us: now.saturating_add(FOREGROUND_HOLD_US),
            held,
        };
        Placed {
            report: None,
            send: Vec::new(),
            timer: TimerChange::Arm(FOREGROUND_HOLD_US),
        }
    }

    /// Any other event: its tell sent, unless a hold in force keeps it;
    /// the pending window, found in front, reported with this event's
    /// observation, `observed_at_us`, before what it releases; a hold
    /// whose time is over releases what it held and awaits its window.
    fn other(
        &mut self,
        item: Option<T>,
        in_front: isize,
        observed_at_us: u64,
        now: u64,
    ) -> Placed<T> {
        match std::mem::take(self) {
            Self::Clear => Placed::sent(item.into_iter().collect()),
            Self::Awaiting { front } if front.window == in_front => Placed {
                report: Some(Front {
                    observed_at_us,
                    ..front
                }),
                send: item.into_iter().collect(),
                timer: TimerChange::Keep,
            },
            awaiting @ Self::Awaiting { .. } => {
                *self = awaiting;
                Placed::sent(item.into_iter().collect())
            }
            Self::Holding {
                front,
                until_us,
                mut held,
            } => {
                held.extend(item);
                if front.window == in_front {
                    Placed {
                        report: Some(Front {
                            observed_at_us,
                            ..front
                        }),
                        send: held,
                        timer: TimerChange::Disarm,
                    }
                } else if now >= until_us {
                    *self = Self::Awaiting { front };
                    Placed {
                        report: None,
                        send: held,
                        timer: TimerChange::Disarm,
                    }
                } else {
                    *self = Self::Holding {
                        front,
                        until_us,
                        held,
                    };
                    Placed::sent(Vec::new())
                }
            }
        }
    }

    /// The timer went off: a hold whose window is in front reports it with
    /// its own event's observation and releases what it held; one whose
    /// time is over releases what it held and awaits its window; one that
    /// went off early is set again for the rest. With no hold in force the
    /// `WM_TIMER` is a stale one, and does nothing.
    fn timer(&mut self, in_front: isize, now: u64) -> Placed<T> {
        let pending = std::mem::take(self);
        let Self::Holding {
            front,
            until_us,
            held,
        } = pending
        else {
            *self = pending;
            return Placed::sent(Vec::new());
        };
        if front.window == in_front {
            Placed {
                report: Some(front),
                send: held,
                timer: TimerChange::Disarm,
            }
        } else if now >= until_us {
            *self = Self::Awaiting { front };
            Placed {
                report: None,
                send: held,
                timer: TimerChange::Disarm,
            }
        } else {
            *self = Self::Holding {
                front,
                until_us,
                held,
            };
            Placed {
                report: None,
                send: Vec::new(),
                timer: TimerChange::Arm(until_us.saturating_sub(now)),
            }
        }
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

    /// A foreground event's step for `front`, its tell named `item`.
    fn foreground(front: Front, item: &'static str) -> Seen<&'static str> {
        Seen::Foreground {
            front,
            item: Some(item),
        }
    }

    /// Any other event's step, observed at `at`, its tell named `item`.
    fn other(item: &'static str, at: u64) -> Seen<&'static str> {
        Seen::Other {
            item: Some(item),
            observed_at_us: at,
        }
    }

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

    /// Before the start nothing is reported, the window in front at a
    /// foreground event included (the start reads the foreground itself);
    /// after it, a console's foreground event reported names its thread
    /// and ends the wait for it.
    #[test]
    fn a_foreground_event_names_its_thread_and_ends_any_wait() {
        let mut pending = Pending::default();
        let mut watch = ForegroundWatch::default();
        assert_eq!(
            pending.step(foreground(EDITOR, "editor"), 10, 1_000, watch.started()),
            Placed {
                report: None,
                send: vec!["editor"],
                timer: TimerChange::Keep,
            },
            "before the start, which reads the foreground itself"
        );
        watch.start(Some(CONSOLE));
        let placed = pending.step(foreground(CONSOLE, "console"), 20, 2_000, watch.started());
        assert_eq!(placed.report, Some(CONSOLE));
        assert_eq!(watch.sent(), Some(false), "the consoles' events unhooked");
        assert_eq!(watch.console_event(20, 201, 5_000), Step::default());
        let placed = pending.step(foreground(EDITOR, "editor"), 10, 6_000, watch.started());
        assert_eq!(placed.report, Some(EDITOR));
        assert_eq!(watch.sent(), None);
    }

    /// Every foreground event in front is reported with its own
    /// observation, before its tell, in the order handled, a change back
    /// to an earlier window included: none replaces another.
    #[test]
    fn every_foreground_event_is_sent_with_its_stamp_none_replaced() {
        let mut pending = Pending::default();
        let returned = Front {
            observed_at_us: 4_000,
            ..EDITOR
        };
        let placed: Vec<Placed<&str>> = [EXPLORER, returned, EXPLORER]
            .into_iter()
            .map(|front| pending.step(foreground(front, "tell"), front.window, 0, true))
            .collect();
        let reports: Vec<Option<Front>> = placed.iter().map(|placed| placed.report).collect();
        assert_eq!(reports, [Some(EXPLORER), Some(returned), Some(EXPLORER)]);
        assert!(placed.iter().all(|placed| placed.send == ["tell"]));
        assert_eq!(pending, Pending::Clear);
    }

    /// A foreground event whose window is not in front as it is handled
    /// holds what follows; once its hold is over it is reported when a
    /// later event finds it in front, with that event's observation, and
    /// not before.
    #[test]
    fn a_foreground_event_for_a_window_not_in_front_is_sent_when_a_later_event_finds_it_in_front() {
        let mut pending = Pending::default();
        pending.step(foreground(EXPLORER, "foreground"), 10, 3_000, true);
        assert_eq!(
            pending.step(other("change", 30_000), 10, 30_000, true),
            Placed {
                report: None,
                send: vec!["foreground", "change"],
                timer: TimerChange::Disarm,
            },
            "the hold's time is over: released, the window still awaited"
        );
        assert_eq!(pending, Pending::Awaiting { front: EXPLORER });
        assert_eq!(
            pending.step(other("focus", 30_600), 30, 30_600, true),
            Placed {
                report: Some(Front {
                    observed_at_us: 30_600,
                    ..EXPLORER
                }),
                send: vec!["focus"],
                timer: TimerChange::Keep,
            }
        );
        assert_eq!(
            pending.step(other("next", 30_700), 30, 30_700, true).report,
            None,
            "sent once"
        );
    }

    /// A newer foreground event replaces the pending window, whether or not
    /// its own window is in front, so the replaced one is never reported,
    /// and nothing held behind either is lost.
    #[test]
    fn a_refused_foreground_event_replaced_by_a_newer_one_is_never_sent() {
        let mut pending = Pending::default();
        pending.step(foreground(EXPLORER, "explorer"), 10, 0, true);
        pending.step(foreground(CONSOLE, "console"), 10, 1_000, true);
        assert_eq!(
            pending.step(other("change", 2_000), 30, 2_000, true),
            Placed::sent(Vec::new()),
            "the replaced window in front releases nothing"
        );
        assert_eq!(
            pending.step(foreground(EDITOR, "editor"), 10, 3_000, true),
            Placed {
                report: Some(EDITOR),
                send: vec!["explorer", "console", "change", "editor"],
                timer: TimerChange::Disarm,
            },
            "a newer event in front"
        );
        assert_eq!(
            pending.step(other("later", 4_000), 20, 4_000, true).report,
            None
        );
    }

    /// The defect this record removes: a hold released by its timer with
    /// the window in front reports the window before the tells it releases,
    /// so Core accepts the foreground tell. Source-derived, approved by
    /// Dickson on 2026-11-21: NVDA processes the event at its deferred pump
    /// once `GetForegroundWindow` names the window
    /// (`nvda/source/IAccessibleHandler/internalWinEventHandler.py` 233 to
    /// 263; `nvda/source/IAccessibleHandler/__init__.py` 834 to 840).
    #[test]
    fn the_timer_with_the_window_in_front_reports_it_before_the_tells_it_releases() {
        let mut pending = Pending::default();
        assert_eq!(
            pending.step(foreground(EXPLORER, "foreground"), 10, 3_000, true),
            Placed {
                report: None,
                send: vec![],
                timer: TimerChange::Arm(FOREGROUND_HOLD_US),
            }
        );
        pending.step(other("old window's change", 5_000), 10, 5_000, true);
        assert_eq!(
            pending.step(Seen::Timer, 30, 12_000, true),
            Placed {
                report: Some(EXPLORER),
                send: vec!["foreground", "old window's change"],
                timer: TimerChange::Disarm,
            }
        );
        assert_eq!(pending, Pending::Clear);
    }

    /// The timer early, with the window not yet in front, is set again for
    /// the rest; at the hold's end it releases what it held and the window
    /// is awaited.
    #[test]
    fn the_timer_early_is_set_again_and_at_the_end_releases_what_was_held() {
        let mut pending = Pending::default();
        pending.step(foreground(EXPLORER, "foreground"), 10, 1_000, true);
        assert_eq!(
            pending.step(Seen::Timer, 10, 15_000, true),
            Placed {
                report: None,
                send: vec![],
                timer: TimerChange::Arm(6_000),
            }
        );
        assert_eq!(
            pending.step(Seen::Timer, 10, 21_000, true),
            Placed {
                report: None,
                send: vec!["foreground"],
                timer: TimerChange::Disarm,
            }
        );
        assert_eq!(pending, Pending::Awaiting { front: EXPLORER });
    }

    /// A stale `WM_TIMER`, left posted by `KillTimer`, does nothing with no
    /// hold in force, whether no foreground is pending or one is awaited.
    #[test]
    fn a_stale_timer_does_nothing_with_no_hold() {
        let mut pending: Pending<&str> = Pending::default();
        assert_eq!(
            pending.step(Seen::Timer, 30, 1_000, true),
            Placed::sent(vec![])
        );
        assert_eq!(pending, Pending::Clear);
        let mut pending: Pending<&str> = Pending::Awaiting { front: EXPLORER };
        assert_eq!(
            pending.step(Seen::Timer, 30, 1_000, true),
            Placed::sent(vec![])
        );
        assert_eq!(pending, Pending::Awaiting { front: EXPLORER });
    }

    /// Before the watch's start, the hold keeps its order and its timer
    /// releases what it held, but reports nothing: the start reports the
    /// window in front.
    #[test]
    fn before_the_start_a_release_reports_nothing() {
        let mut pending = Pending::default();
        pending.step(foreground(EXPLORER, "foreground"), 10, 0, false);
        assert_eq!(
            pending.step(Seen::Timer, 30, 5_000, false),
            Placed {
                report: None,
                send: vec!["foreground"],
                timer: TimerChange::Disarm,
            }
        );
    }

    /// An item placed from another thread, the idle barrier's answer, is
    /// held behind a hold in force and releases nothing, even with the
    /// window in front; with no hold it goes at once.
    #[test]
    fn an_item_placed_from_another_thread_is_held_and_releases_nothing() {
        let mut pending = Pending::default();
        assert_eq!(pending.place_behind("idle"), Some("idle"));
        pending.step(foreground(EXPLORER, "foreground"), 10, 0, true);
        assert_eq!(pending.place_behind("idle"), None);
        assert_eq!(
            pending.step(other("focus", 1_000), 30, 1_000, true),
            Placed {
                report: Some(Front {
                    observed_at_us: 1_000,
                    ..EXPLORER
                }),
                send: vec!["foreground", "idle", "focus"],
                timer: TimerChange::Disarm,
            }
        );
        pending = Pending::Awaiting { front: EXPLORER };
        assert_eq!(pending.place_behind("idle"), Some("idle"));
    }

    /// A foreground event not in front while a window is awaited holds
    /// again, for its own window; one in front reports it and ends the
    /// wait.
    #[test]
    fn a_foreground_event_while_a_window_is_awaited_replaces_it() {
        let mut pending = Pending::Awaiting { front: EXPLORER };
        assert_eq!(
            pending
                .step(foreground(CONSOLE, "console"), 10, 0, true)
                .timer,
            TimerChange::Arm(FOREGROUND_HOLD_US)
        );
        let mut pending = Pending::Awaiting { front: EXPLORER };
        assert_eq!(
            pending.step(foreground(EDITOR, "editor"), 10, 0, true),
            Placed {
                report: Some(EDITOR),
                send: vec!["editor"],
                timer: TimerChange::Keep,
            }
        );
        assert_eq!(pending, Pending::Clear);
    }

    /// The shell's foreground holds nothing: it is reported in front, and
    /// awaited when it is not, replacing the window awaited, while a hold
    /// in force keeps holding.
    #[test]
    fn the_shells_foreground_holds_nothing() {
        let shell = |item| Seen::Shell {
            front: EDITOR,
            item: Some(item),
        };
        let mut pending = Pending::Awaiting { front: EXPLORER };
        assert_eq!(
            pending.step(shell("shell"), 10, 0, true),
            Placed {
                report: Some(EDITOR),
                send: vec!["shell"],
                timer: TimerChange::Keep,
            }
        );
        assert_eq!(pending, Pending::Clear);
        assert_eq!(pending.step(shell("shell"), 30, 0, true).send, ["shell"]);
        assert_eq!(pending, Pending::Awaiting { front: EDITOR });
        let mut pending = Pending::default();
        pending.step(foreground(EXPLORER, "foreground"), 10, 0, true);
        assert_eq!(
            pending.step(shell("shell"), 10, 1_000, true).send,
            Vec::<&str>::new()
        );
        assert!(matches!(pending, Pending::Holding { .. }));
    }
}
