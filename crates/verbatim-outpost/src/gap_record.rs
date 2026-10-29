//! The listener's record of a `WinEvent` focus no UIA focus has answered
//! (`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated
//! fact", the gap's evidence; `docs/design/listener-without-connections.md`,
//! decision 11).
//!
//! UI Automation sometimes delivers an application's focus events late or
//! never while the `WinEvent` focus still comes, or after their senders are
//! gone. The listener's desktop-wide hook receives every focus `WinEvent`
//! beside every UIA focus, so it keeps, for each application whose outpost
//! has its hook in, the oldest focus copy no UIA focus has answered, on one
//! clock, with no call and no verdict of its own. Unanswered at its bound,
//! it is taken as focus-missing, which the listener sends the application's
//! outpost, which reads its focus when its own verdict for the copy's
//! window is UIA.
//!
//! It is kept in the readers' shared state, under their lock
//! (`focus_read::Readers`), and is public so mockapp's stand-in listener
//! keeps one as the listener does.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use verbatim_model::Pid;

/// How long a record waits, from its copy's observation, for a UIA focus.
/// The UIA focus of a `WinEvent` focus usually comes within 1 to 35 ms in
/// Settings, and in the same millisecond at the median in File Explorer; a
/// newly started File Explorer process's first one came 222 to 514 ms after
/// the first item's `WinEvent` in `explorer_folder_window`, and 515 to
/// 2,694 ms after the process's first focus `WinEvent` in a UIA window in 55
/// new windows of `measure_focus_explorer` (2026-10-21), all but two under
/// a second; when UI Automation's delivery fails none comes, in Settings
/// for 14.5 seconds. A read made before a late UIA focus costs one read and
/// nothing heard twice, since Core takes the late focus as a duplicate of
/// the read's; a read made late leaves the user in silence. So the bound is
/// a second rather than above every late delivery measured.
pub const BOUND: Duration = Duration::from_secs(1);

/// How long before a copy's observation a UIA focus may have been observed
/// and still answer it: a UIA focus can reach the listener before the
/// `WinEvent` of the same focus, by up to 25.5 ms in the runs measured
/// (5 of 1,680 `WinEvent` focuses in UIA windows, `measure_focus_explorer`,
/// 2026-10-21).
pub const ANSWERED_BEFORE_US: u64 = 100_000;

/// A record whose bound has passed unanswered: the listener sends its
/// application's outpost focus-missing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Missing {
    /// The application.
    pub pid: Pid,
    /// The copy's window.
    pub window: isize,
    /// When the copy was observed, in microseconds on the run's clock.
    pub observed_at_us: u64,
}

/// What a `WinEvent` the listener's hook observed did to the records
/// ([`GapRecords::observed`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    /// Whether it answered a record elsewhere.
    pub closed: bool,
    /// Whether it opened a record with its bound armed, which the
    /// watchdog is to time.
    pub armed: bool,
    /// The copy, to be read at once: a UIA focus whose sender had no
    /// window answered it before it came ([`Answer::ReadsNow`]).
    pub missing: Option<Missing>,
}

/// What a UIA focus event's outcome did to its application's record
/// ([`GapRecords::reported`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reported {
    /// Whether it closed the record.
    pub closed: bool,
    /// The record, to be read at once in its place
    /// ([`Answer::ReadsNow`]).
    pub missing: Option<Missing>,
}

/// A `WinEvent` the listener's hook observed that bears on the records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observed {
    /// The application it is routed to.
    pub pid: Pid,
    /// Its window.
    pub window: isize,
    /// That window's top-level window.
    pub top_level: isize,
    /// When it was observed, in microseconds on the run's clock.
    pub observed_at_us: u64,
    /// Whether it is a focus event, which can open a record, rather than
    /// a foreground change, which only answers one elsewhere.
    pub focus: bool,
}

/// What a UIA focus event's outcome says of its application's record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// It answers a copy in the top-level window given, or, with `None`,
    /// a copy in any window of the application.
    Answers(Option<isize>),
    /// It answers a copy in any window of the application, which is to be
    /// read at once in its place: the event came, and its sender has no
    /// window above it, as a sender gone before its read has, so the event
    /// for where the focus went may never come
    /// (`docs/design/listener-without-connections.md`, risk 13).
    ReadsNow,
    /// It leaves the record open.
    LeavesOpen,
}

/// The records of every application whose outpost has its hook in.
#[derive(Debug, Default)]
pub struct GapRecords {
    /// The applications whose outposts have their hooks in.
    hooked: HashSet<Pid>,
    /// Each hooked application's record and its newest answer.
    apps: HashMap<Pid, App>,
    /// How many records have been opened, each numbered by the count once
    /// it is opened, for the idle barrier ([`Self::open_through`]).
    opened: u64,
}

#[derive(Debug, Default)]
struct App {
    open: Option<Record>,
    /// The newest UIA focus observation that answered or would answer a
    /// copy, so a copy that one observed up to [`ANSWERED_BEFORE_US`]
    /// before it already answers opens no record.
    last_answered: Option<Answered>,
    /// The newest observation of a UIA focus whose sender had no window
    /// above it ([`Answer::ReadsNow`]), so a copy it answers, coming after
    /// its drop, is read at once.
    last_no_window_us: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
struct Record {
    window: isize,
    top_level: isize,
    observed_at_us: u64,
    /// When the copy was observed, on the clock the bound is timed by.
    opened: Instant,
    /// Whether the bound is armed: the application had no read running or
    /// waiting when the copy came, or its reads have finished since.
    armed: bool,
    number: u64,
}

#[derive(Clone, Copy, Debug)]
struct Answered {
    observed_at_us: u64,
    top_level: Option<isize>,
}

impl Answered {
    /// Whether it answers a copy observed at `copy_us` in `copy_top_level`.
    fn answers(self, copy_us: u64, copy_top_level: isize) -> bool {
        self.observed_at_us.saturating_add(ANSWERED_BEFORE_US) >= copy_us
            && self.top_level.is_none_or(|top| top == copy_top_level)
    }
}

impl GapRecords {
    /// `pid`'s outpost has its hook in: its focus copies are recorded from
    /// now on.
    pub fn hook_installed(&mut self, pid: Pid) {
        self.hooked.insert(pid);
    }

    /// `pid` has no outpost with its hook in any more: its record goes.
    /// Returns whether one was open.
    pub fn hook_removed(&mut self, pid: Pid) -> bool {
        self.hooked.remove(&pid);
        self.apps.remove(&pid).is_some_and(|app| app.open.is_some())
    }

    /// The listener's hook observed `event`, `reads_idle` saying whether
    /// its application has no UIA event waiting for its read or being read,
    /// at `now` on the bound's clock: every record it is elsewhere from, in
    /// another application or another top-level window of the same one, is
    /// answered, and a focus of a hooked application with no record open
    /// opens one, unless a UIA focus already answered it, armed when its
    /// reads are idle; or, when a UIA focus whose sender had no window
    /// already answered it, it is to be read at once.
    pub fn observed(&mut self, event: Observed, reads_idle: bool, now: Instant) -> Observation {
        let mut closed = false;
        for (pid, app) in &mut self.apps {
            if app
                .open
                .is_some_and(|open| *pid != event.pid || open.top_level != event.top_level)
            {
                app.open = None;
                closed = true;
            }
        }
        let mut observation = Observation {
            closed,
            armed: false,
            missing: None,
        };
        if !event.focus || !self.hooked.contains(&event.pid) {
            return observation;
        }
        let app = self.apps.entry(event.pid).or_default();
        if app.open.is_some()
            || app
                .last_answered
                .is_some_and(|answered| answered.answers(event.observed_at_us, event.top_level))
        {
            return observation;
        }
        if app
            .last_no_window_us
            .is_some_and(|at| at.saturating_add(ANSWERED_BEFORE_US) >= event.observed_at_us)
        {
            observation.missing = Some(Missing {
                pid: event.pid,
                window: event.window,
                observed_at_us: event.observed_at_us,
            });
            return observation;
        }
        self.opened += 1;
        app.open = Some(Record {
            window: event.window,
            top_level: event.top_level,
            observed_at_us: event.observed_at_us,
            opened: now,
            armed: reads_idle,
            number: self.opened,
        });
        observation.armed = reads_idle;
        observation
    }

    /// A UIA focus event of `pid`'s observed at `observed_at_us`, whose
    /// outcome says `answer`, was reported: it answers the record it
    /// answers, and is kept to answer a copy that comes after it. Returns
    /// whether it closed a record, and the record to be read at once in
    /// its place, for an answer that [`Answer::ReadsNow`].
    pub fn reported(&mut self, pid: Pid, observed_at_us: u64, answer: Answer) -> Reported {
        let unanswered = Reported {
            closed: false,
            missing: None,
        };
        if answer == Answer::LeavesOpen || !self.hooked.contains(&pid) {
            return unanswered;
        }
        let app = self.apps.entry(pid).or_default();
        let Answer::Answers(top_level) = answer else {
            app.last_no_window_us = Some(
                app.last_no_window_us
                    .map_or(observed_at_us, |at| at.max(observed_at_us)),
            );
            return match app.open {
                Some(open)
                    if observed_at_us.saturating_add(ANSWERED_BEFORE_US) >= open.observed_at_us =>
                {
                    app.open = None;
                    Reported {
                        closed: true,
                        missing: Some(Missing {
                            pid,
                            window: open.window,
                            observed_at_us: open.observed_at_us,
                        }),
                    }
                }
                _ => unanswered,
            };
        };
        if app
            .last_answered
            .is_none_or(|last| last.observed_at_us <= observed_at_us)
        {
            app.last_answered = Some(Answered {
                observed_at_us,
                top_level,
            });
        }
        let answered = Answered {
            observed_at_us,
            top_level,
        };
        if app
            .open
            .is_some_and(|open| answered.answers(open.observed_at_us, open.top_level))
        {
            app.open = None;
            return Reported {
                closed: true,
                missing: None,
            };
        }
        unanswered
    }

    /// A UIA event of `pid`'s is waiting for its read: its record's bound
    /// waits for its reads to finish.
    pub fn reads_queued(&mut self, pid: Pid) {
        if let Some(open) = self.apps.get_mut(&pid).and_then(|app| app.open.as_mut()) {
            open.armed = false;
        }
    }

    /// `pid`'s reads have finished: its record, still open, has its bound
    /// armed. Returns whether one was armed.
    pub fn reads_finished(&mut self, pid: Pid) -> bool {
        match self.apps.get_mut(&pid).and_then(|app| app.open.as_mut()) {
            Some(open) if !open.armed => {
                open.armed = true;
                true
            }
            _ => false,
        }
    }

    /// The records whose bound has passed at `now`, armed, taken.
    pub fn take_due(&mut self, now: Instant) -> Vec<Missing> {
        let mut due = Vec::new();
        for (pid, app) in &mut self.apps {
            if let Some(open) = app.open
                && open.armed
                && now >= open.opened + BOUND
            {
                app.open = None;
                due.push(Missing {
                    pid: *pid,
                    window: open.window,
                    observed_at_us: open.observed_at_us,
                });
            }
        }
        due
    }

    /// When the earliest armed record's bound passes.
    #[must_use]
    pub fn next_due(&self) -> Option<Instant> {
        self.apps
            .values()
            .filter_map(|app| app.open.filter(|open| open.armed))
            .map(|open| open.opened + BOUND)
            .min()
    }

    /// How many records have been opened so far.
    #[must_use]
    pub fn opened(&self) -> u64 {
        self.opened
    }

    /// Whether a record among the first `count` opened is still open.
    #[must_use]
    pub fn open_through(&self, count: u64) -> bool {
        self.apps
            .values()
            .any(|app| app.open.is_some_and(|open| open.number <= count))
    }

    /// Whether `pid` has a record open.
    #[must_use]
    pub fn is_open(&self, pid: Pid) -> bool {
        self.apps.get(&pid).is_some_and(|app| app.open.is_some())
    }
}
