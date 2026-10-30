//! The focus listener's read scheduler as a pure transition
//! (`docs/design/ownership-one-judge.md`, "The listener"): which reading
//! thread reads which application's UIA event, when a read is abandoned or
//! published at its bound, when an event waiting behind abandoned reads is
//! dropped, which idle waiters may run, and whether a menu opening's
//! reading thread may compare it with its own last read.
//!
//! [`Schedule::step`] takes one [`ScheduleInput`] and the time, in
//! microseconds on the shell's monotonic clock, and returns the
//! [`ScheduleEffect`]s, in the order the shell applies them under its lock.
//! The schedule makes no call, reads no clock and holds no lock: the shell
//! (`focus_read::Readers`) keeps the lock, the threads, their condition
//! variables, the watchdog's alarm, the idle waiters' closures and each
//! read's progress, and gathers what a tick needs from the reads it watches.
//! So every ordering of inputs is a replay test of this module.
//!
//! The menu end's grace is a second pure transition here,
//! [`GraceState::step`], which the listener's threadpool timer and its
//! callers drive.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use verbatim_model::Pid;

use crate::focus_read::{
    ABANDONED_LIMIT, FocusEvent, Judged, MOVED_ON, Meta, NOT_IN_TIME, REPLACED, WAITED_TOO_LONG,
};
use verbatim_model::protocol::{EventTiming, UiaFocusRead};

/// Why an event is dropped when the listener is shutting down.
const SHUTTING_DOWN: &str = "the listener is shutting down";

/// Why an event is dropped when no thread could be had to read it.
const NO_THREAD: &str = "no thread to read it";

/// One input to the schedule.
pub(crate) enum ScheduleInput<J> {
    /// A UIA event of `pid`'s, `meta`, arrived: `job` is what its read
    /// takes, `kind` its kind as the reading says it, `key` the element it
    /// is of for NVDA's limiter (a newer event of one kind of one element
    /// replaces a waiting one), and `window` the window it is of as far as
    /// its cache says, `None` for an event never judged by the moved-on
    /// rule.
    Push {
        pid: Pid,
        meta: Meta,
        job: J,
        kind: FocusEvent,
        key: Option<Vec<i32>>,
        window: Option<isize>,
    },
    /// The reading thread `generation` of `pid` asks for its next event.
    Next { pid: Pid, generation: u64 },
    /// The reading thread `generation` of `pid` returned from the read of
    /// `meta`'s event with `judged`, placed in top-level window `placed`
    /// for its gap record, and of top-level window `top`, 0 for none.
    Returned {
        pid: Pid,
        generation: u64,
        meta: Meta,
        judged: Judged,
        placed: Option<isize>,
        top: isize,
    },
    /// The watchdog's look at the reads under way, each as the shell found
    /// it ([`TickRead`]).
    Tick { reads: Vec<TickRead> },
    /// A foreground event reached the listener's hook, of top-level window
    /// `top_level`; `running_tops` is, for each application with a read
    /// under way, that read's top-level window, 0 while its window is not
    /// known.
    Foreground {
        top_level: isize,
        running_tops: Vec<(Pid, isize)>,
    },
    /// The listener's `WinEvent` thread forwarded an MSAA focus of `pid`'s.
    MsaaFocus { pid: Pid },
    /// `pid`'s outpost has ended.
    OutpostEnded { pid: Pid },
    /// The shell holds idle waiter `waiter`, to run once every event
    /// queued before it has its outcome.
    WhenIdle { waiter: u64 },
    /// No thread could be started for `pid`'s events as `generation`.
    SpawnFailed { pid: Pid, generation: u64 },
    /// The listener is shutting down.
    Stop,
}

/// What the watchdog's shell found of one read under way.
pub(crate) struct TickRead {
    /// The read's application.
    pub(crate) pid: Pid,
    /// The window the moved-on answer was judged for, 0 for none known.
    pub(crate) window: isize,
    /// The read's window once the answer was made: a read that learned
    /// another window meanwhile has its answer judge nothing.
    pub(crate) window_after: isize,
    /// Whether the window in front was another thread of the read's
    /// application when the read's window became known.
    pub(crate) front_elsewhere: bool,
    /// Whether the user has moved on from `window`, asked only when
    /// [`Schedule::moved_on_wanted`] says so.
    pub(crate) moved_on: Option<bool>,
    /// The read's bound, once it has passed NVDA's checks and while no one
    /// has claimed it.
    pub(crate) bound_us: Option<u64>,
    /// The read as it would be published, made by the shell only when the
    /// bound has passed.
    pub(crate) at_bound: Option<Box<UiaFocusRead>>,
    /// The top-level window the read was placed in, 0 for none.
    pub(crate) top: isize,
}

/// Which threads to wake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Waking {
    /// The thread in charge of the application's events.
    Thread(Pid),
    /// Every thread, for the shutdown.
    All,
}

/// How the watchdog claims a read under way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Claim {
    /// Abandoned: its thread's late result is discarded.
    Abandon,
    /// Published at its bound: its thread's late result is discarded.
    Publish,
}

/// What the watchdog's alarm is to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlarmChange {
    /// Go off by this time, unless it is set to go off sooner.
    By(u64),
    /// Go off at this time, or never: the watchdog's own setting.
    Set(Option<u64>),
    /// Cancelled, no read being under way.
    Settle,
    /// Go off now.
    Now,
}

/// One effect of a step, applied by the shell in order under its lock.
pub(crate) enum ScheduleEffect<J> {
    /// What became of `pid`'s event `meta`, sent to its outpost.
    Outcome {
        pid: Pid,
        meta: Meta,
        judged: Judged,
        placed: Option<isize>,
    },
    /// A thread is in charge of `pid`'s events as `generation`: a new one,
    /// or, with `reuse`, the thread whose abandoned read just returned.
    Spawn {
        pid: Pid,
        generation: u64,
        reuse: bool,
    },
    /// Wake waiting reading threads.
    Wake(Waking),
    /// The read under way of `pid`'s is claimed.
    Claim { pid: Pid, claim: Claim },
    /// The thread whose abandoned read returned forgets its last read: Core
    /// never got that focus.
    ForgetLast,
    /// The watchdog's alarm.
    Alarm(AlarmChange),
    /// Run the idle waiter.
    RunIdle(u64),
    /// The event the asking thread reads next.
    Job(Job<J>),
}

/// The event a reading thread reads next.
pub(crate) struct Job<J> {
    pub(crate) pid: Pid,
    pub(crate) meta: Meta,
    /// What its read takes.
    pub(crate) work: J,
    /// The window the event is of as far as its cache says, `None` for an
    /// event never judged by the moved-on rule.
    pub(crate) window: Option<isize>,
    /// Whether the focus event queued before it was another
    /// application's.
    pub(crate) after_elsewhere: bool,
    /// Its place in the order every event was queued in, and the place of
    /// the newest event that moved away from its application when it was
    /// queued ([`QueuePlace`]).
    pub(crate) queue: QueuePlace,
}

/// An event's place in the order every application's events were queued
/// in, and, for menu currentness, the newest place at which the user moved
/// away from its application before it was queued: its thread's last read
/// is current for it when that read's event was queued after that place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct QueuePlace {
    /// The event's place.
    pub(crate) place: u64,
    /// Where its application was last moved away from.
    pub(crate) left_at: u64,
}

impl QueuePlace {
    /// Whether a read of the event queued at `read_place` is still current
    /// for this event: nothing moved away from the application since that
    /// event was queued.
    #[must_use]
    pub(crate) fn current_since(self, read_place: u64) -> bool {
        read_place != 0 && self.left_at < read_place
    }
}

/// One event waiting for its read.
struct Queued<J> {
    place: u64,
    /// How many foreground events had come when it was queued.
    foregrounds: u64,
    after_elsewhere: bool,
    left_at: u64,
    kind: FocusEvent,
    key: Option<Vec<i32>>,
    window: Option<isize>,
    meta: Meta,
    job: J,
}

/// The event the thread in charge of an application is reading.
struct Running {
    started: u64,
    meta: Meta,
    kind: FocusEvent,
    foregrounds: u64,
    /// Whether the watchdog published it at its bound.
    published: bool,
}

/// One application's reading.
struct App<J> {
    queue: VecDeque<Queued<J>>,
    /// The thread in charge of its events, by generation, if one is.
    live: Option<u64>,
    running: Option<Running>,
    running_place: u64,
    /// Its threads abandoned to reads that have not returned yet.
    abandoned: usize,
    /// When its abandoned threads reached [`ABANDONED_LIMIT`].
    limit_reached: Option<u64>,
    /// Whether its outpost has ended.
    ending: bool,
    /// The place of the newest event that moved away from it.
    left_at: u64,
    /// The top-level window of its last read focus, 0 for none.
    last_top: isize,
}

impl<J> App<J> {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            live: None,
            running: None,
            running_place: 0,
            abandoned: 0,
            limit_reached: None,
            ending: false,
            left_at: 0,
            last_top: 0,
        }
    }
}

/// Who drops the waiting events, for the log line.
#[derive(Clone, Copy)]
enum Dropping {
    Waited,
    AsPushed,
}

/// The read scheduler ([module docs](self)).
pub(crate) struct Schedule<J> {
    deadline: u64,
    grace: u64,
    apps: HashMap<Pid, App<J>>,
    next_generation: u64,
    stopping: bool,
    /// How many events have been queued: an event's place is the count
    /// once it is queued.
    queued: u64,
    /// The idle waiters, each with the count of events queued when it came.
    idle_waiters: VecDeque<(u64, u64)>,
    /// How many foreground events have come.
    foregrounds: u64,
    /// Whether a foreground event wakes the watchdog.
    wake_on_foreground: bool,
    /// The application of the last focus or menu opening queued.
    last_focus: Option<Pid>,
}

/// Microseconds in `duration`.
fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

impl<J> Schedule<J> {
    /// A schedule abandoning a read past `deadline`, or past `grace` once
    /// the user has moved on from its window.
    pub(crate) fn new(deadline: Duration, grace: Duration) -> Self {
        Self {
            deadline: micros(deadline),
            grace: micros(grace),
            apps: HashMap::new(),
            next_generation: 0,
            stopping: false,
            queued: 0,
            idle_waiters: VecDeque::new(),
            foregrounds: 0,
            wake_on_foreground: false,
            last_focus: None,
        }
    }

    /// Whether the listener is shutting down.
    pub(crate) fn stopping(&self) -> bool {
        self.stopping
    }

    /// Whether the thread `generation` is in charge of `pid`'s events: one
    /// that is waits for its next event, one that is not ends.
    pub(crate) fn in_charge(&self, pid: Pid, generation: u64) -> bool {
        !self.stopping
            && self
                .apps
                .get(&pid)
                .is_some_and(|app| app.live == Some(generation))
    }

    /// Whether the watchdog's shell is to ask whether the user has moved
    /// on from the window of `pid`'s read under way at `now`, given
    /// whether the window in front was elsewhere when its window became
    /// known: only past its grace and before its deadline, with a
    /// foreground event since its event or that evidence.
    pub(crate) fn moved_on_wanted(&self, pid: Pid, now: u64, front_elsewhere: bool) -> bool {
        self.apps
            .get(&pid)
            .and_then(|app| app.running.as_ref())
            .is_some_and(|running| {
                now >= running.started + self.grace
                    && now < running.started + self.deadline
                    && (running.foregrounds != self.foregrounds || front_elsewhere)
            })
    }

    /// Whether a focus event is waiting for its read or being read, for a
    /// menu opening of `pid`'s: a focus of an application whose abandoned
    /// threads are at the limit, waiting with no thread, counts for that
    /// application's own menu openings only.
    pub(crate) fn focus_pending(&self, pid: Option<Pid>) -> bool {
        self.apps.iter().any(|(owner, app)| {
            app.running
                .as_ref()
                .is_some_and(|running| running.kind == FocusEvent::Focus && !running.published)
                || (app.live.is_some() || Some(*owner) == pid)
                    && app
                        .queue
                        .iter()
                        .any(|queued| queued.kind == FocusEvent::Focus)
        })
    }

    /// Takes one input at `now`, microseconds on the shell's monotonic
    /// clock, and returns its effects in the order they are applied.
    pub(crate) fn step(&mut self, input: ScheduleInput<J>, now: u64) -> Vec<ScheduleEffect<J>> {
        let mut effects = Vec::new();
        match input {
            ScheduleInput::Push {
                pid,
                meta,
                job,
                kind,
                key,
                window,
            } => self.push(&mut effects, now, (pid, meta, job), (kind, key, window)),
            ScheduleInput::Next { pid, generation } => {
                self.next(&mut effects, now, pid, generation);
            }
            ScheduleInput::Returned {
                pid,
                generation,
                meta,
                judged,
                placed,
                top,
            } => self.returned(&mut effects, (pid, generation), meta, (judged, placed, top)),
            ScheduleInput::Tick { reads } => self.tick(&mut effects, now, reads),
            ScheduleInput::Foreground {
                top_level,
                running_tops,
            } => self.foreground(&mut effects, top_level, &running_tops),
            ScheduleInput::MsaaFocus { pid } => self.moved_away_from_all_but(pid),
            ScheduleInput::OutpostEnded { pid } => self.outpost_ended(&mut effects, pid),
            ScheduleInput::WhenIdle { waiter } => {
                if !self.stopping {
                    self.idle_waiters.push_back((self.queued, waiter));
                    self.wake_idle_waiters(&mut effects);
                }
            }
            ScheduleInput::SpawnFailed { pid, generation } => {
                self.spawn_failed(&mut effects, pid, generation);
            }
            ScheduleInput::Stop => {
                self.stopping = true;
                effects.push(ScheduleEffect::Wake(Waking::All));
                effects.push(ScheduleEffect::Alarm(AlarmChange::Now));
            }
        }
        effects
    }

    fn push(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        now: u64,
        (pid, meta, job): (Pid, Meta, J),
        (kind, key, window): (FocusEvent, Option<Vec<i32>>, Option<isize>),
    ) {
        if self.stopping {
            effects.push(ScheduleEffect::Outcome {
                pid,
                meta,
                judged: Judged::Dropped(SHUTTING_DOWN),
                placed: None,
            });
            return;
        }
        let app = self.apps.entry(pid).or_insert_with(App::new);
        // An event after its outpost ended: the application runs on.
        app.ending = false;
        // NVDA's UIA limiter keeps one waiting event per element and kind:
        // a newer one erases the older, which is reported dropped.
        if let Some(key) = &key {
            let mut kept = VecDeque::with_capacity(app.queue.len());
            for waiting in app.queue.drain(..) {
                if waiting.kind == kind && waiting.key.as_ref() == Some(key) {
                    effects.push(ScheduleEffect::Outcome {
                        pid,
                        meta: waiting.meta,
                        judged: Judged::Dropped(REPLACED),
                        placed: None,
                    });
                } else {
                    kept.push_back(waiting);
                }
            }
            app.queue = kept;
        }
        self.queued += 1;
        let place = self.queued;
        let after_elsewhere = match kind {
            FocusEvent::Focus | FocusEvent::MenuOpened => self.last_focus.replace(pid) != Some(pid),
            FocusEvent::Selection | FocusEvent::Notification => false,
        };
        // Another application's focus or menu opening moves away from
        // every other application's last read.
        if matches!(kind, FocusEvent::Focus | FocusEvent::MenuOpened) {
            self.moved_away_from_all_but(pid);
        }
        let foregrounds = self.foregrounds;
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        app.queue.push_back(Queued {
            place,
            foregrounds,
            after_elsewhere,
            left_at: app.left_at,
            kind,
            key,
            window,
            meta,
            job,
        });
        if app.live.is_some() {
            effects.push(ScheduleEffect::Wake(Waking::Thread(pid)));
        } else if app
            .limit_reached
            .is_some_and(|reached| now >= reached + self.deadline)
        {
            // Past the application's bound: dropped at once, behind the
            // events the watchdog has not yet dropped, in order.
            self.drop_waiting(effects, pid, Dropping::AsPushed);
        } else if app.abandoned >= ABANDONED_LIMIT {
            tracing::warn!(
                %pid,
                waiting = app.queue.len(),
                "a UIA focus waits: its application's reads have not returned"
            );
            // The watchdog bounds its wait.
            effects.push(ScheduleEffect::Alarm(AlarmChange::Now));
        } else {
            self.start(effects, pid, false);
        }
    }

    /// Marks every application but `pid` as moved away from at the place
    /// of the newest event queued.
    fn moved_away_from_all_but(&mut self, pid: Pid) {
        let place = self.queued;
        for (owner, app) in &mut self.apps {
            if *owner != pid {
                app.left_at = place;
            }
        }
    }

    /// Puts a thread in charge of `pid`'s events: a new one, or with
    /// `reuse` the thread asking.
    fn start(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid, reuse: bool) {
        self.next_generation += 1;
        let generation = self.next_generation;
        if let Some(app) = self.apps.get_mut(&pid) {
            app.live = Some(generation);
            effects.push(ScheduleEffect::Spawn {
                pid,
                generation,
                reuse,
            });
        }
    }

    fn next(&mut self, effects: &mut Vec<ScheduleEffect<J>>, now: u64, pid: Pid, generation: u64) {
        if self.stopping {
            return;
        }
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        if app.live != Some(generation) {
            return;
        }
        if let Some(queued) = app.queue.pop_front() {
            app.running = Some(Running {
                started: now,
                meta: queued.meta,
                kind: queued.kind,
                foregrounds: queued.foregrounds,
                published: false,
            });
            app.running_place = queued.place;
            // Checked at its deadline, or at the end of its grace if a
            // foreground event came while it waited.
            let check = if queued.foregrounds == self.foregrounds {
                now + self.deadline
            } else {
                now + self.grace
            };
            self.wake_on_foreground = true;
            effects.push(ScheduleEffect::Alarm(AlarmChange::By(check)));
            effects.push(ScheduleEffect::Job(Job {
                pid,
                meta: queued.meta,
                work: queued.job,
                window: queued.window,
                after_elsewhere: queued.after_elsewhere,
                queue: QueuePlace {
                    place: queued.place,
                    left_at: queued.left_at,
                },
            }));
            return;
        }
        if app.ending {
            app.live = None;
            if app.abandoned == 0 {
                self.apps.remove(&pid);
            }
        }
    }

    fn returned(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        (pid, generation): (Pid, u64),
        meta: Meta,
        (judged, placed, top): (Judged, Option<isize>, isize),
    ) {
        let stopping = self.stopping;
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        if app.live == Some(generation) {
            let read = matches!(judged, Judged::Read(_));
            if read && top != 0 {
                app.last_top = top;
            }
            match app.running.take() {
                Some(running) if running.published => {
                    tracing::info!(
                        %pid,
                        "a UIA focus read published at its bound returned; its result is discarded"
                    );
                }
                _ => effects.push(ScheduleEffect::Outcome {
                    pid,
                    meta,
                    judged,
                    placed,
                }),
            }
            self.settle_alarm(effects);
            self.wake_idle_waiters(effects);
            return;
        }
        // Abandoned while it read: its event was dropped, and its read is
        // a focus Core never got.
        app.abandoned = app.abandoned.saturating_sub(1);
        if app.abandoned < ABANDONED_LIMIT {
            app.limit_reached = None;
        }
        tracing::info!(%pid, "an abandoned UIA focus read returned; its result is discarded");
        effects.push(ScheduleEffect::ForgetLast);
        if app.live.is_some() || app.queue.is_empty() || stopping {
            return;
        }
        self.start(effects, pid, true);
    }

    /// Cancels the alarm once no read is under way and no application's
    /// events wait at the abandoned limit.
    fn settle_alarm(&self, effects: &mut Vec<ScheduleEffect<J>>) {
        if self
            .apps
            .values()
            .all(|app| app.running.is_none() && app.limit_reached.is_none())
        {
            effects.push(ScheduleEffect::Alarm(AlarmChange::Settle));
        }
    }

    /// Runs each waiter for which every event queued before it has its
    /// outcome sent.
    fn wake_idle_waiters(&mut self, effects: &mut Vec<ScheduleEffect<J>>) {
        if self.idle_waiters.is_empty() {
            return;
        }
        let oldest = self
            .apps
            .values()
            .filter_map(|app| {
                if app
                    .running
                    .as_ref()
                    .is_some_and(|running| !running.published)
                {
                    Some(app.running_place)
                } else {
                    app.queue.front().map(|queued| queued.place)
                }
            })
            .min();
        while let Some(&(before, waiter)) = self.idle_waiters.front()
            && oldest.is_none_or(|oldest| before < oldest)
        {
            self.idle_waiters.pop_front();
            effects.push(ScheduleEffect::RunIdle(waiter));
        }
    }

    fn tick(&mut self, effects: &mut Vec<ScheduleEffect<J>>, now: u64, reads: Vec<TickRead>) {
        if self.stopping {
            return;
        }
        let mut reads: HashMap<Pid, TickRead> =
            reads.into_iter().map(|read| (read.pid, read)).collect();
        let mut due: Option<u64> = None;
        let mut wake_on_foreground = false;
        let mut abandoned = Vec::new();
        let mut published = Vec::new();
        let mut waited = Vec::new();
        let mut stale = false;
        let mut pids: Vec<Pid> = self.apps.keys().copied().collect();
        pids.sort_unstable();
        for pid in pids {
            let Some(app) = self.apps.get(&pid) else {
                continue;
            };
            let Some(running) = &app.running else {
                // Events waiting with no thread, the application's
                // abandoned threads at the limit, are dropped at its bound.
                if app.live.is_none()
                    && !app.queue.is_empty()
                    && let Some(reached) = app.limit_reached
                {
                    let end = reached + self.deadline;
                    if now >= end {
                        waited.push(pid);
                    } else {
                        due = Some(due.map_or(end, |due| due.min(end)));
                    }
                }
                continue;
            };
            let read = reads.remove(&pid);
            let (window, front_elsewhere, moved_on, bound, top) =
                read.as_ref().map_or((0, false, None, None, 0), |read| {
                    (
                        read.window,
                        read.front_elsewhere,
                        read.moved_on,
                        read.bound_us,
                        read.top,
                    )
                });
            if read
                .as_ref()
                .is_some_and(|read| read.window != read.window_after)
            {
                // The read learned another window while the answer was
                // made: the answer judges nothing, and the watchdog looks
                // again at once.
                stale = true;
                continue;
            }
            let grace_end = running.started + self.grace;
            let deadline = running.started + self.deadline;
            let armed = running.foregrounds != self.foregrounds || front_elsewhere;
            let reason = if now >= deadline {
                Some(NOT_IN_TIME)
            } else if window != 0 && now >= grace_end && armed && moved_on == Some(true) {
                Some(MOVED_ON)
            } else {
                None
            };
            if let Some(reason) = reason {
                abandoned.push((pid, window, reason));
                continue;
            }
            let bound = bound.filter(|_| !running.published);
            if bound.is_some_and(|bound| now >= bound)
                && let Some(at_bound) = read.and_then(|read| read.at_bound)
            {
                published.push((pid, at_bound, top));
                continue;
            }
            let next = if now < grace_end && armed {
                grace_end
            } else {
                wake_on_foreground = true;
                deadline
            };
            let next = bound.map_or(next, |bound| bound.min(next));
            due = Some(due.map_or(next, |due| due.min(next)));
        }
        if !abandoned.is_empty() || !published.is_empty() || !waited.is_empty() || stale {
            for (pid, window, reason) in abandoned {
                self.abandon(effects, now, (pid, window), reason);
            }
            for (pid, read, top) in published {
                self.publish(effects, pid, read, top);
            }
            for pid in waited {
                self.drop_waiting(effects, pid, Dropping::Waited);
            }
            effects.push(ScheduleEffect::Alarm(AlarmChange::Now));
            return;
        }
        self.wake_on_foreground = wake_on_foreground;
        effects.push(ScheduleEffect::Alarm(AlarmChange::Set(due)));
    }

    fn abandon(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        now: u64,
        (pid, window): (Pid, isize),
        reason: &'static str,
    ) {
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        let Some(running) = app.running.take() else {
            return;
        };
        effects.push(ScheduleEffect::Claim {
            pid,
            claim: Claim::Abandon,
        });
        app.live = None;
        app.abandoned += 1;
        let abandoned = app.abandoned;
        if abandoned >= ABANDONED_LIMIT && app.limit_reached.is_none() {
            app.limit_reached = Some(now);
        }
        if reason == MOVED_ON {
            tracing::warn!(
                %pid,
                abandoned,
                window,
                "a UIA focus read was slow and the user moved on to another thread of its application; its thread is abandoned"
            );
        } else {
            tracing::warn!(%pid, abandoned, "a UIA focus read did not return in time; its thread is abandoned");
        }
        let waiting = !app.queue.is_empty();
        // A read published at its bound has had its outcome sent.
        if !running.published {
            effects.push(ScheduleEffect::Outcome {
                pid,
                meta: running.meta,
                judged: Judged::Dropped(reason),
                placed: None,
            });
        }
        if waiting {
            if abandoned >= ABANDONED_LIMIT {
                tracing::warn!(
                    %pid,
                    "UIA focuses wait: its application's reads have not returned"
                );
            } else {
                self.start(effects, pid, false);
            }
        }
        self.wake_idle_waiters(effects);
    }

    fn publish(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        pid: Pid,
        read: Box<UiaFocusRead>,
        top: isize,
    ) {
        let Some(running) = self.apps.get_mut(&pid).and_then(|app| app.running.as_mut()) else {
            return;
        };
        running.published = true;
        let meta = running.meta;
        tracing::info!(
            %pid,
            ancestors = read.ancestors.len(),
            "a UIA focus read's call was still under way at its bound; the focus is published from what it read"
        );
        effects.push(ScheduleEffect::Claim {
            pid,
            claim: Claim::Publish,
        });
        effects.push(ScheduleEffect::Outcome {
            pid,
            meta,
            judged: Judged::Read(read),
            placed: Some(top).filter(|&top| top != 0),
        });
        self.wake_idle_waiters(effects);
    }

    /// Drops every event of `pid`'s waiting with no thread to read it, in
    /// the order they arrived.
    fn drop_waiting(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid, dropping: Dropping) {
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        if app.live.is_some() || app.queue.is_empty() {
            return;
        }
        let (abandoned, dropped) = (app.abandoned, app.queue.len());
        match dropping {
            Dropping::Waited => tracing::warn!(
                %pid,
                abandoned,
                dropped,
                "every waiting UIA event of the application, of whatever kind, waited past its bound behind its abandoned reads; they are dropped"
            ),
            Dropping::AsPushed => tracing::warn!(
                %pid,
                abandoned,
                dropped,
                "every waiting UIA event of the application, of whatever kind, is dropped as one arrives: it is past its bound behind its abandoned reads"
            ),
        }
        for queued in app.queue.drain(..) {
            effects.push(ScheduleEffect::Outcome {
                pid,
                meta: queued.meta,
                judged: Judged::Dropped(WAITED_TOO_LONG),
                placed: None,
            });
        }
        self.wake_idle_waiters(effects);
    }

    fn foreground(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        top_level: isize,
        running_tops: &[(Pid, isize)],
    ) {
        self.foregrounds += 1;
        let place = self.queued;
        for (pid, app) in &mut self.apps {
            // The read under way once it knows its window, else the last
            // read; a read under way whose window is unknown counts as
            // moving away.
            let reference = if app.running.is_some() {
                running_tops
                    .iter()
                    .find(|(owner, _)| owner == pid)
                    .map_or(0, |&(_, top)| top)
            } else {
                app.last_top
            };
            if reference == 0 || reference != top_level {
                app.left_at = place;
            }
        }
        if self.wake_on_foreground && self.apps.values().any(|app| app.running.is_some()) {
            effects.push(ScheduleEffect::Alarm(AlarmChange::Now));
        }
    }

    fn outpost_ended(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid) {
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        if app.live.is_none() && app.abandoned == 0 && app.queue.is_empty() {
            self.apps.remove(&pid);
            return;
        }
        app.ending = true;
        effects.push(ScheduleEffect::Wake(Waking::Thread(pid)));
    }

    fn spawn_failed(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid, generation: u64) {
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        if app.live != Some(generation) {
            return;
        }
        tracing::error!(%pid, "no thread to read UIA focus events; they are dropped");
        app.live = None;
        for queued in app.queue.drain(..) {
            effects.push(ScheduleEffect::Outcome {
                pid,
                meta: queued.meta,
                judged: Judged::Dropped(NO_THREAD),
                placed: None,
            });
        }
        self.wake_idle_waiters(effects);
    }
}

/// What waits for a menu end's grace to pass (`listener`'s `MenuEnd`).
#[derive(Default)]
pub(crate) struct GraceState {
    /// The end: when it ended, its stamps, and when its grace is over, in
    /// microseconds on the shell's monotonic clock.
    end: Option<(u64, EventTiming, u64)>,
    /// The idle barrier's rounds answered once the end is sent.
    answers: Vec<u64>,
    /// Whether the timer is being closed, so it is set no more.
    closed: bool,
}

/// One input to the menu end's grace.
#[derive(Clone, Copy)]
pub(crate) enum GraceInput {
    /// A menu or the switcher ended at `ended_at_us`, its grace of `grace`
    /// microseconds starting at `now`.
    Ended {
        ended_at_us: u64,
        timing: EventTiming,
        grace: u64,
    },
    /// The grace's timer went off.
    Fired,
    /// The idle barrier's round `round` is to be answered.
    IdleAnswer(u64),
    /// The timer is being closed.
    Close,
}

/// One effect of the grace's step.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GraceEffect {
    /// Send the end.
    SendEnd {
        ended_at_us: u64,
        timing: EventTiming,
    },
    /// Send the idle barrier's answer for the round.
    SendIdleAnswer(u64),
    /// Set the timer to go off this many microseconds from now.
    SetTimer(u64),
}

impl GraceState {
    /// Takes one input at `now`, microseconds on the shell's monotonic
    /// clock: a callback that runs before the end's grace is over, one of
    /// a grace an earlier end began, or a timer that fired a little early,
    /// sends nothing and sets the timer again for the rest of the grace.
    pub(crate) fn step(&mut self, input: GraceInput, now: u64) -> Vec<GraceEffect> {
        match input {
            GraceInput::Ended {
                ended_at_us,
                timing,
                grace,
            } => {
                self.end = Some((ended_at_us, timing, now + grace));
                vec![GraceEffect::SetTimer(grace)]
            }
            GraceInput::Fired => {
                if let Some((_, _, over)) = self.end
                    && now < over
                {
                    return if self.closed {
                        Vec::new()
                    } else {
                        vec![GraceEffect::SetTimer(over - now)]
                    };
                }
                let mut effects = Vec::new();
                if let Some((ended_at_us, timing, _)) = self.end.take() {
                    effects.push(GraceEffect::SendEnd {
                        ended_at_us,
                        timing,
                    });
                }
                effects.extend(self.answers.drain(..).map(GraceEffect::SendIdleAnswer));
                effects
            }
            GraceInput::IdleAnswer(round) => {
                if self.end.is_some() {
                    self.answers.push(round);
                    Vec::new()
                } else {
                    vec![GraceEffect::SendIdleAnswer(round)]
                }
            }
            GraceInput::Close => {
                self.closed = true;
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::TraceId;

    use super::*;

    const DEADLINE: u64 = 10_000_000;
    const GRACE: u64 = 1_000_000;

    fn schedule() -> Schedule<u64> {
        Schedule::new(
            Duration::from_micros(DEADLINE),
            Duration::from_micros(GRACE),
        )
    }

    fn meta(marker: u64, kind: FocusEvent) -> Meta {
        Meta {
            marker,
            kind,
            trace: TraceId::mint(),
            timing: EventTiming::default(),
        }
    }

    fn push(pid: u32, marker: u64, kind: FocusEvent) -> ScheduleInput<u64> {
        ScheduleInput::Push {
            pid: Pid(pid),
            meta: meta(marker, kind),
            job: marker,
            kind,
            key: None,
            window: Some(0x10),
        }
    }

    fn focus(pid: u32, marker: u64) -> ScheduleInput<u64> {
        push(pid, marker, FocusEvent::Focus)
    }

    /// One effect as these tests compare it: an outcome by its marker and
    /// reason, a job by its marker and queue place, the rest whole.
    #[derive(Debug, PartialEq, Eq)]
    enum Seen {
        Outcome(u32, u64, &'static str),
        Spawn(u32, u64, bool),
        Wake(Waking),
        Claim(u32, Claim),
        ForgetLast,
        Alarm(AlarmChange),
        RunIdle(u64),
        Job(u32, u64, QueuePlace),
    }

    fn seen(effects: Vec<ScheduleEffect<u64>>) -> Vec<Seen> {
        effects
            .into_iter()
            .map(|effect| match effect {
                ScheduleEffect::Outcome {
                    pid, meta, judged, ..
                } => Seen::Outcome(
                    pid.0,
                    meta.marker,
                    match judged {
                        Judged::Dropped(reason) => reason,
                        Judged::Read(_) => "read",
                        Judged::Passed(_) => "passed",
                    },
                ),
                ScheduleEffect::Spawn {
                    pid,
                    generation,
                    reuse,
                } => Seen::Spawn(pid.0, generation, reuse),
                ScheduleEffect::Wake(waking) => Seen::Wake(waking),
                ScheduleEffect::Claim { pid, claim } => Seen::Claim(pid.0, claim),
                ScheduleEffect::ForgetLast => Seen::ForgetLast,
                ScheduleEffect::Alarm(change) => Seen::Alarm(change),
                ScheduleEffect::RunIdle(waiter) => Seen::RunIdle(waiter),
                ScheduleEffect::Job(job) => Seen::Job(job.pid.0, job.meta.marker, job.queue),
            })
            .collect()
    }

    fn step(schedule: &mut Schedule<u64>, input: ScheduleInput<u64>, now: u64) -> Vec<Seen> {
        seen(schedule.step(input, now))
    }

    fn returned(pid: u32, generation: u64, marker: u64) -> ScheduleInput<u64> {
        ScheduleInput::Returned {
            pid: Pid(pid),
            generation,
            meta: meta(marker, FocusEvent::Focus),
            judged: Judged::Dropped("read"),
            placed: None,
            top: 0,
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
            bound_us: 2_000_000,
        })
    }

    fn read_of(pid: u32, window: isize) -> TickRead {
        TickRead {
            pid: Pid(pid),
            window,
            window_after: window,
            front_elsewhere: false,
            moved_on: None,
            bound_us: None,
            at_bound: None,
            top: 0,
        }
    }

    fn tick(reads: Vec<TickRead>) -> ScheduleInput<u64> {
        ScheduleInput::Tick { reads }
    }

    /// Takes application 1's two reads past their deadlines, abandoning
    /// both threads so the application is at the abandoned limit, its
    /// third event waiting with no thread.
    fn at_the_limit(schedule: &mut Schedule<u64>) {
        assert_eq!(step(schedule, focus(1, 1), 0), [Seen::Spawn(1, 1, false)]);
        let _ = step(
            schedule,
            ScheduleInput::Next {
                pid: Pid(1),
                generation: 1,
            },
            0,
        );
        assert_eq!(
            step(schedule, focus(1, 2), 0),
            [Seen::Wake(Waking::Thread(Pid(1)))]
        );
        assert_eq!(
            step(schedule, tick(vec![read_of(1, 0x10)]), DEADLINE),
            [
                Seen::Claim(1, Claim::Abandon),
                Seen::Outcome(1, 1, NOT_IN_TIME),
                Seen::Spawn(1, 2, false),
                Seen::Alarm(AlarmChange::Now),
            ]
        );
        let _ = step(
            schedule,
            ScheduleInput::Next {
                pid: Pid(1),
                generation: 2,
            },
            DEADLINE,
        );
        assert_eq!(
            step(schedule, tick(vec![read_of(1, 0x10)]), 2 * DEADLINE),
            [
                Seen::Claim(1, Claim::Abandon),
                Seen::Outcome(1, 2, NOT_IN_TIME),
                Seen::Alarm(AlarmChange::Now),
            ]
        );
        assert_eq!(
            step(schedule, focus(1, 3), 2 * DEADLINE),
            [Seen::Alarm(AlarmChange::Now)],
            "it waits: the application is at the limit"
        );
    }

    /// An event at the abandoned limit is read by the first abandoned
    /// thread that returns, which forgets its own last read and goes on in
    /// charge of the application's events.
    #[test]
    fn an_event_at_the_abandoned_limit_is_read_by_a_returning_thread() {
        let mut schedule = schedule();
        at_the_limit(&mut schedule);
        assert_eq!(
            step(&mut schedule, returned(1, 1, 1), 2 * DEADLINE + 1),
            [Seen::ForgetLast, Seen::Spawn(1, 3, true)]
        );
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::Next {
                    pid: Pid(1),
                    generation: 3
                },
                2 * DEADLINE + 1
            ),
            [
                Seen::Alarm(AlarmChange::By(3 * DEADLINE + 1)),
                Seen::Job(
                    1,
                    3,
                    QueuePlace {
                        place: 3,
                        left_at: 0
                    }
                ),
            ]
        );
    }

    /// An abandoned read's late return sends no outcome, its drop having
    /// been sent, and has its thread forget its last read.
    #[test]
    fn an_abandoned_reads_late_return_sends_no_outcome_and_forgets_its_read() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(
            &mut schedule,
            ScheduleInput::Next {
                pid: Pid(1),
                generation: 1,
            },
            0,
        );
        let _ = step(&mut schedule, tick(vec![read_of(1, 0x10)]), DEADLINE);
        assert_eq!(
            step(&mut schedule, returned(1, 1, 1), DEADLINE + 5),
            [Seen::ForgetLast]
        );
        assert!(!schedule.in_charge(Pid(1), 1), "the thread ends");
    }

    /// With no foreground event since its event, a read's alarm is its
    /// deadline, not the end of its grace.
    #[test]
    fn no_grace_end_alarm_without_a_foreground_event() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(
            &mut schedule,
            ScheduleInput::Next {
                pid: Pid(1),
                generation: 1,
            },
            0,
        );
        assert_eq!(
            step(&mut schedule, tick(vec![read_of(1, 0x10)]), 5),
            [Seen::Alarm(AlarmChange::Set(Some(DEADLINE)))]
        );
        assert!(!schedule.moved_on_wanted(Pid(1), GRACE, false));
        let _ = step(
            &mut schedule,
            ScheduleInput::Foreground {
                top_level: 0x20,
                running_tops: vec![(Pid(1), 0x10)],
            },
            6,
        );
        assert_eq!(
            step(&mut schedule, tick(vec![read_of(1, 0x10)]), 7),
            [Seen::Alarm(AlarmChange::Set(Some(GRACE)))],
            "a foreground event since has it checked at its grace's end"
        );
    }

    /// Events waiting at the limit are dropped at the application's bound,
    /// and none of them is a pending focus for another application's menu
    /// openings.
    #[test]
    fn an_event_waiting_at_the_limit_is_dropped_at_the_deadline() {
        let mut schedule = schedule();
        at_the_limit(&mut schedule);
        assert!(schedule.focus_pending(Some(Pid(1))));
        assert!(!schedule.focus_pending(Some(Pid(2))));
        assert_eq!(
            step(&mut schedule, tick(Vec::new()), 2 * DEADLINE + 1),
            [Seen::Alarm(AlarmChange::Set(Some(3 * DEADLINE)))]
        );
        assert_eq!(
            step(&mut schedule, tick(Vec::new()), 3 * DEADLINE),
            [
                Seen::Outcome(1, 3, WAITED_TOO_LONG),
                Seen::Alarm(AlarmChange::Now)
            ]
        );
    }

    /// An idle waiter runs once every event queued before it has its
    /// outcome, whatever was queued after it.
    #[test]
    fn an_idle_waiter_runs_with_later_events_queued() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(
            &mut schedule,
            ScheduleInput::Next {
                pid: Pid(1),
                generation: 1,
            },
            0,
        );
        assert_eq!(
            step(&mut schedule, ScheduleInput::WhenIdle { waiter: 7 }, 0),
            []
        );
        let _ = step(&mut schedule, focus(1, 2), 1);
        assert_eq!(
            step(&mut schedule, returned(1, 1, 1), 2),
            [
                Seen::Outcome(1, 1, "read"),
                Seen::Alarm(AlarmChange::Settle),
                Seen::RunIdle(7),
            ]
        );
    }

    /// Every event's marker gets exactly one outcome, through a
    /// replacement, an abandonment and a late return.
    #[test]
    fn every_marker_gets_one_outcome() {
        let mut schedule = schedule();
        let mut effects = Vec::new();
        let keyed = |marker| ScheduleInput::Push {
            pid: Pid(1),
            meta: meta(marker, FocusEvent::Focus),
            job: marker,
            kind: FocusEvent::Focus,
            key: Some(vec![42]),
            window: Some(0x10),
        };
        effects.extend(step(&mut schedule, focus(1, 1), 0));
        effects.extend(step(
            &mut schedule,
            ScheduleInput::Next {
                pid: Pid(1),
                generation: 1,
            },
            0,
        ));
        effects.extend(step(&mut schedule, keyed(2), 1));
        effects.extend(step(&mut schedule, keyed(3), 2));
        effects.extend(step(&mut schedule, tick(vec![read_of(1, 0x10)]), DEADLINE));
        effects.extend(step(
            &mut schedule,
            ScheduleInput::Next {
                pid: Pid(1),
                generation: 2,
            },
            DEADLINE,
        ));
        effects.extend(step(&mut schedule, returned(1, 1, 1), DEADLINE + 1));
        effects.extend(step(&mut schedule, returned(1, 2, 3), DEADLINE + 2));
        let mut outcomes: Vec<u64> = effects
            .iter()
            .filter_map(|effect| match effect {
                Seen::Outcome(_, marker, _) => Some(*marker),
                _ => None,
            })
            .collect();
        outcomes.sort_unstable();
        assert_eq!(outcomes, [1, 2, 3]);
    }

    /// A menu opening's thread's last read is current only while nothing
    /// moved away from its application since that read's event: another
    /// application's focus or menu opening queued, another process's MSAA
    /// focus, or a foreground event of another top-level window than the
    /// last read's, or one during a read whose window is unknown.
    #[test]
    fn menu_currentness_is_lost_by_each_kind_of_moving_away() {
        // The last read, application 1's focus at place 1 in window 0x10,
        // then each kind of moving away before its menu opening.
        let opening_after = |moving: &dyn Fn(&mut Schedule<u64>)| {
            let mut schedule = schedule();
            let _ = step(&mut schedule, focus(1, 1), 0);
            let _ = step(
                &mut schedule,
                ScheduleInput::Next {
                    pid: Pid(1),
                    generation: 1,
                },
                0,
            );
            let _ = step(
                &mut schedule,
                ScheduleInput::Returned {
                    pid: Pid(1),
                    generation: 1,
                    meta: meta(1, FocusEvent::Focus),
                    judged: Judged::Read(a_read()),
                    placed: Some(0x10),
                    top: 0x10,
                },
                1,
            );
            moving(&mut schedule);
            let _ = step(&mut schedule, push(1, 9, FocusEvent::MenuOpened), 2);
            let effects = step(
                &mut schedule,
                ScheduleInput::Next {
                    pid: Pid(1),
                    generation: 1,
                },
                2,
            );
            match effects.as_slice() {
                [Seen::Alarm(_), Seen::Job(1, 9, queue)] => queue.current_since(1),
                other => panic!("no job: {other:?}"),
            }
        };
        assert!(opening_after(&|_| {}), "nothing moved away");
        assert!(
            opening_after(&|schedule| {
                let _ = schedule.step(
                    ScheduleInput::Foreground {
                        top_level: 0x10,
                        running_tops: Vec::new(),
                    },
                    1,
                );
            }),
            "a foreground event of the last read's own window"
        );
        assert!(
            !opening_after(&|schedule| {
                let _ = schedule.step(focus(2, 5), 1);
            }),
            "another application's focus"
        );
        assert!(
            !opening_after(&|schedule| {
                let _ = schedule.step(push(2, 5, FocusEvent::MenuOpened), 1);
            }),
            "another application's menu opening"
        );
        assert!(
            !opening_after(&|schedule| {
                let _ = schedule.step(ScheduleInput::MsaaFocus { pid: Pid(2) }, 1);
            }),
            "another process's MSAA focus"
        );
        assert!(
            !opening_after(&|schedule| {
                let _ = schedule.step(
                    ScheduleInput::Foreground {
                        top_level: 0x30,
                        running_tops: Vec::new(),
                    },
                    1,
                );
            }),
            "a foreground event of another top-level window"
        );
    }

    /// A foreground event during a read whose window is not known yet
    /// counts as moving away from its application, whatever the window.
    #[test]
    fn a_foreground_event_during_a_read_of_an_unknown_window_moves_away() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(
            &mut schedule,
            ScheduleInput::Next {
                pid: Pid(1),
                generation: 1,
            },
            0,
        );
        let _ = step(
            &mut schedule,
            ScheduleInput::Foreground {
                top_level: 0x10,
                running_tops: vec![(Pid(1), 0)],
            },
            1,
        );
        let _ = step(&mut schedule, push(1, 9, FocusEvent::MenuOpened), 2);
        let _ = step(&mut schedule, returned(1, 1, 1), 3);
        match step(
            &mut schedule,
            ScheduleInput::Next {
                pid: Pid(1),
                generation: 1,
            },
            3,
        )
        .as_slice()
        {
            [Seen::Alarm(_), Seen::Job(1, 9, queue)] => assert!(
                !queue.current_since(1),
                "a read whose window is unknown counts as moved away from"
            ),
            other => panic!("no job: {other:?}"),
        }
    }

    /// A moved-on answer judged for a window the read has since left
    /// judges nothing, and the watchdog looks again at once.
    #[test]
    fn a_stale_tick_answer_rearms_the_alarm() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(
            &mut schedule,
            ScheduleInput::Next {
                pid: Pid(1),
                generation: 1,
            },
            0,
        );
        let _ = step(
            &mut schedule,
            ScheduleInput::Foreground {
                top_level: 0x20,
                running_tops: vec![(Pid(1), 0x10)],
            },
            1,
        );
        let stale = TickRead {
            window: 0x10,
            window_after: 0x30,
            moved_on: Some(true),
            ..read_of(1, 0x10)
        };
        assert_eq!(
            step(&mut schedule, tick(vec![stale]), GRACE),
            [Seen::Alarm(AlarmChange::Now)]
        );
        assert!(schedule.in_charge(Pid(1), 1), "the read is not abandoned");
        let answered = TickRead {
            moved_on: Some(true),
            ..read_of(1, 0x30)
        };
        assert_eq!(
            step(&mut schedule, tick(vec![answered]), GRACE),
            [
                Seen::Claim(1, Claim::Abandon),
                Seen::Outcome(1, 1, MOVED_ON),
                Seen::Alarm(AlarmChange::Now),
            ]
        );
    }

    /// A grace callback that runs before the end's grace is over sends
    /// nothing and sets the timer for the rest of it; once over, the end is
    /// sent, then the idle answers that waited for it.
    #[test]
    fn a_grace_callback_before_its_end_rearms() {
        let mut grace = GraceState::default();
        assert_eq!(
            grace.step(
                GraceInput::Ended {
                    ended_at_us: 5,
                    timing: EventTiming::default(),
                    grace: 300,
                },
                1_000
            ),
            [GraceEffect::SetTimer(300)]
        );
        assert_eq!(grace.step(GraceInput::IdleAnswer(4), 1_100), []);
        assert_eq!(
            grace.step(GraceInput::Fired, 1_200),
            [GraceEffect::SetTimer(100)]
        );
        assert_eq!(
            grace.step(GraceInput::Fired, 1_300),
            [
                GraceEffect::SendEnd {
                    ended_at_us: 5,
                    timing: EventTiming::default()
                },
                GraceEffect::SendIdleAnswer(4),
            ]
        );
        assert_eq!(
            grace.step(GraceInput::IdleAnswer(5), 1_400),
            [GraceEffect::SendIdleAnswer(5)]
        );
    }
}
