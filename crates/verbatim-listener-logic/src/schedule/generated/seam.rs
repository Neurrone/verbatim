//! The one seam between the read scheduler's generated test and the
//! schedule's types: what the world does, in the world's terms, turned into
//! [`ScheduleInput`]s, and what the schedule tells turned into [`Told`]. A
//! change to the schedule's inputs, effects or keys changes this file alone.
//!
//! An application is named by its process id and process number ([`App`]),
//! which the seam ties to the schedule's [`AppKey`] by the record the
//! process's first event made ([`Seam::held`]). A process's exit is told as
//! `ApplicationExited`; a supervisor's answer that no hook is in, and an
//! outpost's end, as `OutpostEnded`. Effects that only the shell acts on, naming no
//! thread and no event (`Unwatch`, `ForgetLast`, `Forget`), are not told.

use std::collections::BTreeMap;

use verbatim_model::protocol::{EventTiming, UiaFocusRead};
use verbatim_model::{Pid, TraceId};

use super::super::{
    AlarmChange, AppKey, Duration, FocusEvent, Judged, MOVED_ON, Meta, NO_THREAD, NOT_IN_TIME,
    REPLACED, Reason, Request, RuleWindow, Schedule, ScheduleEffect, ScheduleInput,
    WAITED_TOO_LONG, Waking, Window,
};
use super::Cached;

/// Why a read that found no element gives its outcome.
const READ_FAILED: &str = "its element could not be resolved";

/// A process's application: its id and which process of that id it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct App {
    /// The process id.
    pub(super) pid: u32,
    /// Which process of that id.
    pub(super) process: u32,
}

/// A reading thread, by the token the schedule gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ThreadToken(AppKey, u64);

/// A read under way or abandoned, by the token the schedule gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReadToken(AppKey, Request);

/// How an event's outcome came.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum How {
    /// It was read.
    Read,
    /// Its read found no element, and it was dropped.
    Failed,
    /// Its read was abandoned at its deadline.
    NotInTime,
    /// Its read was abandoned because the user moved on.
    MovedOn,
    /// It waited past its application's bound behind abandoned reads.
    WaitedTooLong,
    /// No thread could be started to read it.
    NoThread,
    /// A newer event of its kind and element replaced it.
    Replaced,
}

/// What the schedule told, in the world's terms.
#[derive(Debug)]
pub(super) enum Told {
    /// The event `marker` of `pid` has its outcome.
    Outcome { pid: u32, marker: u64, how: How },
    /// A thread takes charge of `pid`'s events, reused from an abandoned
    /// read when `reuse`.
    Start {
        pid: u32,
        thread: ThreadToken,
        reuse: bool,
    },
    /// The waiting thread of `pid` is woken.
    Woken(u32),
    /// The thread asking reads the event `marker`.
    Job {
        pid: u32,
        marker: u64,
        read: ReadToken,
    },
    /// The window of `pid`'s read `read` is to be judged.
    Judge { pid: u32, read: ReadToken },
    /// The idle waiter runs.
    Ran(u64),
}

/// The schedule, and what the seam must remember of it.
pub(super) struct Seam {
    schedule: Schedule<u64>,
    now: u64,
    alarm: Option<u64>,
    /// The record each process's events made.
    keys: BTreeMap<App, AppKey>,
}

impl Seam {
    /// A schedule abandoning a read past `deadline` microseconds.
    pub(super) fn new(deadline: u64) -> Self {
        Self {
            schedule: Schedule::new(Duration::from_micros(deadline)),
            now: 0,
            alarm: None,
            keys: BTreeMap::new(),
        }
    }

    /// The time of the steps that follow, microseconds.
    pub(super) fn set_now(&mut self, now: u64) {
        self.now = now;
    }

    /// The time the watchdog's alarm is set to go off.
    pub(super) fn alarm(&self) -> Option<u64> {
        self.alarm
    }

    fn step(&mut self, input: ScheduleInput<u64>) -> Vec<Told> {
        let effects = self.schedule.step(input, self.now);
        effects
            .into_iter()
            .filter_map(|effect| self.told(effect))
            .collect()
    }

    fn told(&mut self, effect: ScheduleEffect<u64>) -> Option<Told> {
        match effect {
            ScheduleEffect::Outcome { pid, meta, judged } => Some(Told::Outcome {
                pid: pid.0,
                marker: meta.marker,
                how: how(&judged),
            }),
            ScheduleEffect::Spawn {
                app,
                generation,
                reuse,
            } => Some(Told::Start {
                pid: app.pid.0,
                thread: ThreadToken(app, generation),
                reuse,
            }),
            ScheduleEffect::Wake(Waking::Thread(app)) => Some(Told::Woken(app.pid.0)),
            ScheduleEffect::Job(job) => Some(Told::Job {
                pid: job.app.pid.0,
                marker: job.meta.marker,
                read: ReadToken(job.app, job.request),
            }),
            ScheduleEffect::Judge { app, request, .. } => Some(Told::Judge {
                pid: app.pid.0,
                read: ReadToken(app, request),
            }),
            ScheduleEffect::LogicError(error) => panic!("a logic error: {error}"),
            ScheduleEffect::RunIdle(waiter) => Some(Told::Ran(waiter)),
            ScheduleEffect::Alarm(AlarmChange::Set(due)) => {
                self.alarm = due;
                None
            }
            ScheduleEffect::Alarm(AlarmChange::Now) | ScheduleEffect::Wake(Waking::All) => {
                panic!("the shutdown is not in the generated world")
            }
            ScheduleEffect::Unwatch { .. }
            | ScheduleEffect::ForgetLast
            | ScheduleEffect::Forget { .. } => None,
        }
    }

    /// An event of `app` arrives.
    pub(super) fn event(
        &mut self,
        app: App,
        marker: u64,
        kind: FocusEvent,
        element: Option<u8>,
        cached: Cached,
    ) -> Vec<Told> {
        let told = self.step(ScheduleInput::Push {
            pid: Pid(app.pid),
            meta: meta(marker, kind),
            job: marker,
            kind,
            key: element.map(|element| vec![i32::from(element)]),
            window: match cached {
                Cached::Never => RuleWindow::Never,
                Cached::Unknown => RuleWindow::Unknown,
                Cached::Known => RuleWindow::Known(window()),
            },
        });
        if let Some(key) = self.schedule.current(Pid(app.pid)) {
            self.keys.insert(app, key);
        }
        told
    }

    /// The thread `thread` of `app` asks for its next event.
    pub(super) fn next(&mut self, _app: App, thread: ThreadToken) -> Vec<Told> {
        self.step(ScheduleInput::Next {
            app: thread.0,
            generation: thread.1,
        })
    }

    /// `app`'s read `read` of the event `marker` returns, having found its
    /// element or not.
    pub(super) fn returned(
        &mut self,
        _app: App,
        read: ReadToken,
        marker: u64,
        kind: FocusEvent,
        found: bool,
    ) -> Vec<Told> {
        self.step(ScheduleInput::Returned {
            app: read.0,
            request: read.1,
            meta: meta(marker, kind),
            judged: if found {
                Judged::Read(a_read())
            } else {
                Judged::Dropped(READ_FAILED)
            },
        })
    }

    /// `app`'s read `read` learns its window.
    pub(super) fn learned(&mut self, _app: App, read: ReadToken) -> Vec<Told> {
        self.step(ScheduleInput::Learned {
            app: read.0,
            request: read.1,
            window: window(),
        })
    }

    /// The window of `app`'s read `read` was judged.
    pub(super) fn judged(&mut self, _app: App, read: ReadToken, moved_on: bool) -> Vec<Told> {
        self.step(ScheduleInput::Judged {
            app: read.0,
            request: read.1,
            verdict: moved_on.then_some(Reason::MovedOn),
        })
    }

    /// The watchdog's alarm goes off.
    pub(super) fn tick(&mut self) -> Vec<Told> {
        self.alarm = None;
        self.step(ScheduleInput::Tick)
    }

    /// `app`'s outpost ended, crashed or retired, its application running.
    pub(super) fn outpost_gone(&mut self, app: App) -> Vec<Told> {
        self.step(ScheduleInput::OutpostEnded { pid: Pid(app.pid) })
    }

    /// `app`'s process has exited.
    pub(super) fn exited(&mut self, app: App) -> Vec<Told> {
        self.step(ScheduleInput::ApplicationExited { pid: Pid(app.pid) })
    }

    /// The supervisor answered a stale event of `app`'s that no outpost's
    /// hook is in (`HookRemoved`), told as its outpost's end.
    pub(super) fn no_hook(&mut self, app: App) -> Vec<Told> {
        self.step(ScheduleInput::OutpostEnded { pid: Pid(app.pid) })
    }

    /// No thread could be started as `thread` for `app`'s events.
    pub(super) fn spawn_failed(&mut self, _app: App, thread: ThreadToken) -> Vec<Told> {
        self.step(ScheduleInput::SpawnFailed {
            app: thread.0,
            generation: thread.1,
        })
    }

    /// An idle waiter is queued.
    pub(super) fn when_idle(&mut self, waiter: u64) -> Vec<Told> {
        self.step(ScheduleInput::WhenIdle { waiter })
    }

    /// Whether `thread` is in charge of `app`'s events.
    pub(super) fn in_charge(&self, _app: App, thread: ThreadToken) -> bool {
        self.schedule.in_charge(thread.0, thread.1)
    }

    /// Whether a focus is pending, for a menu opening of `pid`'s.
    pub(super) fn focus_pending(&self, pid: Option<u32>) -> bool {
        self.schedule.focus_pending(pid.map(Pid))
    }

    /// Whether the schedule holds the record `app`'s events made.
    pub(super) fn held(&self, app: App) -> bool {
        self.keys
            .get(&app)
            .is_some_and(|key| self.schedule.held().any(|held| held == *key))
    }
}

/// The window the world's events are of, and the one a read learns.
fn window() -> Window {
    Window::new(0x10).expect("not 0")
}

fn how(judged: &Judged) -> How {
    match judged {
        Judged::Read(_) => How::Read,
        Judged::Dropped(reason) if *reason == NOT_IN_TIME => How::NotInTime,
        Judged::Dropped(reason) if *reason == MOVED_ON => How::MovedOn,
        Judged::Dropped(reason) if *reason == WAITED_TOO_LONG => How::WaitedTooLong,
        Judged::Dropped(reason) if *reason == READ_FAILED => How::Failed,
        Judged::Dropped(reason) if *reason == NO_THREAD => How::NoThread,
        Judged::Dropped(reason) if *reason == REPLACED => How::Replaced,
        Judged::Dropped(reason) => panic!("an outcome the world cannot cause: {reason}"),
        Judged::Passed(_) => panic!("the world's reads pass nothing on"),
    }
}

fn meta(marker: u64, kind: FocusEvent) -> Meta {
    Meta {
        marker,
        kind,
        trace: TraceId::from_raw(marker),
        timing: EventTiming::default(),
    }
}

/// A focus read, as a reading thread returns one.
fn a_read() -> Box<UiaFocusRead> {
    Box::new(UiaFocusRead {
        hwnd: 0,
        snapshot: verbatim_model::protocol::UiaSnapshotFact {
            runtime_id: vec![42],
            role: verbatim_model::Role::Button,
            name: None,
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
        ancestors: Vec::new(),
        ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
        selected_child: None,
        value_from_selection: false,
        window: None,
        same_as_last: None,
        answers: None,
        cached_from: None,
    })
}
