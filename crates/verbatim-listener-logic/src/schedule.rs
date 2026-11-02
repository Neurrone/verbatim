//! The focus listener's read scheduler as a pure transition
//! (`docs/design/ownership.md`): which reading
//! thread reads which application's UIA event, when a read is abandoned or
//! published at its bound, when an event waiting behind abandoned reads is
//! dropped, which idle waiters may run, and whether a menu opening's
//! reading thread may compare it with its own last read.
//!
//! [`Schedule::step`] takes one [`ScheduleInput`] and the time, in
//! microseconds on the shell's monotonic clock, and returns the
//! [`ScheduleEffect`]s, in the order the shell applies them under its lock.
//! Each application's read under way is its work in flight
//! ([`InFlight`], `docs/design/logic-tests.md`, "The work in flight,
//! shared by both readers"), which decides its abandonment, its
//! publication at its bound and the half-second judgment of whether the
//! user has moved on; what it needs of Windows it asks by an effect
//! ([`ScheduleEffect::AskMovedOn`], [`ScheduleEffect::Publish`]), which
//! the shell answers by an input once the step's other effects are
//! applied, before it lets its lock go. The schedule also owns the
//! watchdog's alarm: after every step it sets it to the soonest time
//! anything is due. The schedule makes no call, reads no clock and holds
//! no lock: the shell (`verbatim_listener::focus_read::Readers`) keeps the
//! lock, the threads, their condition variables, the alarm's timer, the
//! idle waiters' closures and each read's progress. So every ordering of
//! inputs is a replay test of this module.
//!
//! The menu end's grace is a second pure transition here,
//! [`GraceState::step`], which the listener's threadpool timer and its
//! callers drive.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use verbatim_model::Pid;
use verbatim_reader_rules::Window;
use verbatim_reader_rules::in_flight::{Abandoned, Ask, Decision, InFlight, Reason, RuleWindow};

pub use verbatim_reader_rules::in_flight::Request;

use crate::focus::{
    ABANDONED_LIMIT, FocusEvent, Judged, MOVED_ON, Meta, NOT_IN_TIME, REPLACED, WAITED_TOO_LONG,
};
use verbatim_model::protocol::{EventTiming, UiaFocusRead};

/// Why an event is dropped when the listener is shutting down.
const SHUTTING_DOWN: &str = "the listener is shutting down";

/// Why an event is dropped when no thread could be had to read it.
const NO_THREAD: &str = "no thread to read it";

/// One input to the schedule.
pub enum ScheduleInput<J> {
    /// A UIA event of `pid`'s, `meta`, arrived: `job` is what its read
    /// takes, `kind` its kind as the reading says it, `key` the element it
    /// is of for NVDA's limiter (a newer event of one kind of one element
    /// replaces a waiting one), and `window` the window it is of as far as
    /// its cache says, `None` for an event never judged by the moved-on
    /// rule and `Some(0)` for one whose read is still to learn it.
    Push {
        /// The event's application.
        pid: Pid,
        /// What the callback knew of it.
        meta: Meta,
        /// What its read takes.
        job: J,
        /// Its kind, as the reading says it.
        kind: FocusEvent,
        /// The element it is of, for NVDA's limiter.
        key: Option<Vec<i32>>,
        /// The window it is of as far as its cache says, which the
        /// moved-on rule judges its read by.
        window: RuleWindow,
    },
    /// The reading thread `generation` of `pid` asks for its next event.
    Next {
        /// The thread's application.
        pid: Pid,
        /// The thread's generation.
        generation: u64,
    },
    /// A reading thread of `pid` returned from its read `request` of
    /// `meta`'s event with `judged`, and of top-level window `top`, if
    /// any.
    Returned {
        /// The thread's application.
        pid: Pid,
        /// The read, by the request its thread read it as.
        request: Request,
        /// What the callback knew of the event read.
        meta: Meta,
        /// What became of it.
        judged: Judged,
        /// The top-level window of the read, if any.
        top: Option<Window>,
    },
    /// The watchdog's alarm went off, which spends its timer.
    Tick,
    /// `pid`'s read `request` learned its window, `window`.
    Learned {
        /// The read's application.
        pid: Pid,
        /// The read.
        request: Request,
        /// Its window.
        window: Window,
    },
    /// `pid`'s read `request` passed NVDA's checks, its bound at `bound`,
    /// in the schedule's microseconds.
    Armed {
        /// The read's application.
        pid: Pid,
        /// The read.
        request: Request,
        /// Its bound.
        bound: u64,
    },
    /// The shell's answer to [`ScheduleEffect::AskMovedOn`].
    MovedOn {
        /// The read's application.
        pid: Pid,
        /// The read asked about.
        request: Request,
        /// Whether the user has left its window for another thread of its
        /// application.
        moved_on: bool,
    },
    /// The shell's answer to [`ScheduleEffect::Publish`]: the read as
    /// published from its progress, none when its walk had ended, and its
    /// top-level window, if any.
    Published {
        /// The read's application.
        pid: Pid,
        /// The read.
        request: Request,
        /// The read as published, if its progress was there to publish.
        read: Option<Box<UiaFocusRead>>,
        /// The abandonment the ask carried, made once the publication is
        /// sent.
        then_abandon: Option<Reason>,
    },
    /// A foreground event reached the listener's hook, of top-level window
    /// `top_level`; `running_tops` is, for each application with a read
    /// under way, that read's top-level window, `None` while its window is
    /// not known. For menu currentness only.
    Foreground {
        /// The foreground window's top-level window, if any.
        top_level: Option<Window>,
        /// For each application with a read under way, that read's top-level
        /// window, `None` while it is not known.
        running_tops: Vec<(Pid, Option<Window>)>,
    },
    /// The listener's `WinEvent` thread forwarded an MSAA focus of `pid`'s.
    MsaaFocus {
        /// The focus's application.
        pid: Pid,
    },
    /// `pid`'s outpost has ended.
    OutpostEnded {
        /// The outpost's application.
        pid: Pid,
    },
    /// The shell holds idle waiter `waiter`, to run once every event
    /// queued before it has its outcome.
    WhenIdle {
        /// The waiter's number.
        waiter: u64,
    },
    /// No thread could be started for `pid`'s events as `generation`.
    SpawnFailed {
        /// The application.
        pid: Pid,
        /// The generation no thread could be started as.
        generation: u64,
    },
    /// The listener is shutting down.
    Stop,
}

/// Which threads to wake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Waking {
    /// The thread in charge of the application's events.
    Thread(Pid),
    /// Every thread, for the shutdown.
    All,
}

/// What the watchdog's alarm is to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlarmChange {
    /// Go off at this time, or never.
    Set(Option<u64>),
    /// Go off now, for the shutdown.
    Now,
}

/// One effect of a step, applied by the shell in order under its lock.
#[derive(Debug)]
pub enum ScheduleEffect<J> {
    /// What became of `pid`'s event `meta`, sent to its outpost.
    Outcome {
        /// The event's application.
        pid: Pid,
        /// What the callback knew of the event.
        meta: Meta,
        /// What became of it.
        judged: Judged,
    },
    /// A thread is in charge of `pid`'s events as `generation`: a new one,
    /// or, with `reuse`, the thread whose abandoned read just returned.
    Spawn {
        /// The application.
        pid: Pid,
        /// The thread's generation.
        generation: u64,
        /// Whether the thread whose abandoned read just returned is reused.
        reuse: bool,
    },
    /// Wake waiting reading threads.
    Wake(Waking),
    /// `pid`'s read under way was abandoned: the shell watches it no more.
    Unwatch {
        /// The read's application.
        pid: Pid,
    },
    /// The thread whose abandoned read returned forgets its last read: Core
    /// never got that focus.
    ForgetLast,
    /// The watchdog's alarm.
    Alarm(AlarmChange),
    /// Run the idle waiter.
    RunIdle(u64),
    /// The event the asking thread reads next.
    Job(Job<J>),
    /// Ask whether the user has left `window`, the window of `pid`'s read
    /// `request`, for another thread of its application: answered by
    /// [`ScheduleInput::MovedOn`] once the step's other effects are
    /// applied.
    AskMovedOn {
        /// The read's application.
        pid: Pid,
        /// The read.
        request: Request,
        /// Its window.
        window: Window,
    },
    /// Publish `pid`'s read `request` at its bound from its progress, read
    /// and not taken: answered by [`ScheduleInput::Published`], which
    /// carries `then_abandon` back, once the step's other effects are
    /// applied.
    Publish {
        /// The read's application.
        pid: Pid,
        /// The read.
        request: Request,
        /// Why the read is abandoned once published, when an abandonment
        /// asked for the publication.
        then_abandon: Option<Reason>,
    },
}

/// The event a reading thread reads next.
#[derive(Debug)]
pub struct Job<J> {
    /// The event's application.
    pub pid: Pid,
    /// The read, which its learned window, its bound and its return name.
    pub request: Request,
    /// What the callback knew of it.
    pub meta: Meta,
    /// What its read takes.
    pub work: J,
    /// The window the event is of as far as its cache says, which the
    /// moved-on rule judges its read by.
    pub window: RuleWindow,
    /// Whether the focus event queued before it was another
    /// application's.
    pub after_elsewhere: bool,
    /// Its place in the order every event was queued in, and the place of
    /// the newest event that moved away from its application when it was
    /// queued ([`QueuePlace`]).
    pub queue: QueuePlace,
}

/// An event's place in the order every application's events were queued
/// in, and, for menu currentness, the newest place at which the user moved
/// away from its application before it was queued: its thread's last read
/// is current for it when that read's event was queued after that place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueuePlace {
    /// The event's place.
    pub place: u64,
    /// Where its application was last moved away from.
    pub left_at: u64,
}

impl QueuePlace {
    /// Whether a read of the event queued at `read_place` is still current
    /// for this event: nothing moved away from the application since that
    /// event was queued.
    #[must_use]
    pub fn current_since(self, read_place: u64) -> bool {
        read_place != 0 && self.left_at < read_place
    }
}

/// One event waiting for its read.
struct Queued<J> {
    place: u64,
    after_elsewhere: bool,
    left_at: u64,
    kind: FocusEvent,
    key: Option<Vec<i32>>,
    window: RuleWindow,
    meta: Meta,
    job: J,
}

/// What the schedule keeps of the event an application's thread is
/// reading, beside what its work in flight keeps.
#[derive(Clone, Debug)]
struct Running {
    meta: Meta,
    kind: FocusEvent,
    /// Its place in the order every event was queued in.
    place: u64,
}

/// One application's reading.
struct App<J> {
    queue: VecDeque<Queued<J>>,
    /// Whether a thread is in charge of its events, as the generation its
    /// work in flight has in charge.
    threaded: bool,
    /// Its read under way, and the generation in charge.
    flight: InFlight<Running>,
    /// Its threads abandoned to reads that have not returned yet.
    abandoned: usize,
    /// When its abandoned threads reached [`ABANDONED_LIMIT`].
    limit_reached: Option<u64>,
    /// Whether its outpost has ended.
    ending: bool,
    /// The place of the newest event that moved away from it.
    left_at: u64,
    /// The top-level window of its last read focus, if any.
    last_top: Option<Window>,
}

impl<J> App<J> {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            threaded: false,
            flight: InFlight::new(),
            abandoned: 0,
            limit_reached: None,
            ending: false,
            left_at: 0,
            last_top: None,
        }
    }

    /// Whether the thread `generation` is the one in charge.
    fn in_charge(&self, generation: u64) -> bool {
        self.threaded && self.flight.generation() == generation
    }

    /// Whether its outpost has ended and nothing of it is left: no thread in
    /// charge, none abandoned, no event waiting. Such an application is no
    /// longer held, so that what the schedule keeps does not grow with
    /// every process the user runs.
    fn spent(&self) -> bool {
        self.ending && !self.threaded && self.abandoned == 0 && self.queue.is_empty()
    }

    /// When its events waiting with no thread, its abandoned threads at the
    /// limit, are dropped.
    fn waiting_end(&self, deadline: u64) -> Option<u64> {
        if self.threaded || self.queue.is_empty() {
            return None;
        }
        self.limit_reached.map(|reached| reached + deadline)
    }
}

/// Who drops the waiting events, for the log line.
#[derive(Clone, Copy)]
enum Dropping {
    Waited,
    AsPushed,
}

/// The read scheduler ([module docs](self)).
pub struct Schedule<J> {
    deadline: u64,
    apps: BTreeMap<Pid, App<J>>,
    stopping: bool,
    /// How many events have been queued: an event's place is the count
    /// once it is queued.
    queued: u64,
    /// The idle waiters, each with the count of events queued when it came.
    idle_waiters: VecDeque<(u64, u64)>,
    /// The application of the last focus or menu opening queued.
    last_focus: Option<Pid>,
    /// When the watchdog's alarm is set to go off.
    alarm_due: Option<u64>,
}

/// Microseconds in `duration`.
fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

impl<J> Schedule<J> {
    /// A schedule abandoning a read past `deadline`, or past the moved-on
    /// grace once the user has moved on from its window.
    #[must_use]
    pub fn new(deadline: Duration) -> Self {
        Self {
            deadline: micros(deadline),
            apps: BTreeMap::new(),
            stopping: false,
            queued: 0,
            idle_waiters: VecDeque::new(),
            last_focus: None,
            alarm_due: None,
        }
    }

    /// Whether the listener is shutting down.
    #[must_use]
    pub fn stopping(&self) -> bool {
        self.stopping
    }

    /// The markers of the events it holds whose outcome is still to be
    /// sent, in order: those waiting, and those being read and not
    /// published at their bound.
    #[must_use]
    pub fn markers(&self) -> Vec<u64> {
        let mut markers: Vec<u64> = self
            .apps
            .values()
            .flat_map(|app| {
                app.queue.iter().map(|queued| queued.meta.marker).chain(
                    app.flight
                        .running()
                        .filter(|(_, _, published)| !published)
                        .map(|(_, running, _)| running.meta.marker),
                )
            })
            .collect();
        markers.sort_unstable();
        markers
    }

    /// Whether the thread `generation` is in charge of `pid`'s events: one
    /// that is waits for its next event, one that is not ends.
    #[must_use]
    pub fn in_charge(&self, pid: Pid, generation: u64) -> bool {
        !self.stopping
            && self
                .apps
                .get(&pid)
                .is_some_and(|app| app.in_charge(generation))
    }

    /// Whether a focus event is waiting for its read or being read, for a
    /// menu opening of `pid`'s: a focus of an application whose abandoned
    /// threads are at the limit, waiting with no thread, counts for that
    /// application's own menu openings only.
    #[must_use]
    pub fn focus_pending(&self, pid: Option<Pid>) -> bool {
        self.apps.iter().any(|(owner, app)| {
            app.flight.running().is_some_and(|(_, running, published)| {
                running.kind == FocusEvent::Focus && !published
            }) || (app.threaded || Some(*owner) == pid)
                && app
                    .queue
                    .iter()
                    .any(|queued| queued.kind == FocusEvent::Focus)
        })
    }

    /// Takes one input at `now`, microseconds on the shell's monotonic
    /// clock, and returns its effects in the order they are applied; the
    /// last, while the listener runs, sets the watchdog's alarm when the
    /// soonest time anything is due has changed, or after a tick, which
    /// spends the alarm's timer.
    pub fn step(&mut self, input: ScheduleInput<J>, now: u64) -> Vec<ScheduleEffect<J>> {
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
                request,
                meta,
                judged,
                top,
            } => self.returned(&mut effects, (pid, request), meta, (judged, top)),
            ScheduleInput::Tick => {
                // The alarm went off, and its timer goes off once: set it
                // again below to whatever is still due, even a time that
                // has not changed, as a timer can go off a little early.
                self.alarm_due = None;
                self.tick(&mut effects, now);
            }
            ScheduleInput::Learned {
                pid,
                request,
                window,
            } => {
                // Nothing is judged once the listener stops, as a tick
                // judges nothing then.
                if !self.stopping
                    && let Some(app) = self.apps.get_mut(&pid)
                {
                    let decision = app.flight.learned(request, window, now);
                    self.decided(&mut effects, now, pid, decision);
                }
            }
            ScheduleInput::Armed {
                pid,
                request,
                bound,
            } => {
                if let Some(app) = self.apps.get_mut(&pid) {
                    app.flight.armed(request, bound);
                }
            }
            ScheduleInput::MovedOn {
                pid,
                request,
                moved_on,
            } => {
                if let Some(app) = self.apps.get_mut(&pid) {
                    let decision = app.flight.moved_on(request, moved_on);
                    self.decided(&mut effects, now, pid, decision);
                }
            }
            ScheduleInput::Published {
                pid,
                request,
                read,
                then_abandon,
            } => self.published(&mut effects, now, (pid, request), read, then_abandon),
            ScheduleInput::Foreground {
                top_level,
                running_tops,
            } => self.foreground(top_level, &running_tops),
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
        self.set_alarm(&mut effects);
        effects
    }

    /// Sets the watchdog's alarm to the soonest time anything is due, when
    /// that differs from the time it is set to (none once it went off):
    /// each read's next wake ([`InFlight::next_wake`]), and the end of each
    /// application's wait at the abandoned limit.
    fn set_alarm(&mut self, effects: &mut Vec<ScheduleEffect<J>>) {
        if self.stopping {
            return;
        }
        let due = self
            .apps
            .values()
            .flat_map(|app| {
                app.flight
                    .next_wake()
                    .into_iter()
                    .chain(app.waiting_end(self.deadline))
            })
            .min();
        if due != self.alarm_due {
            self.alarm_due = due;
            effects.push(ScheduleEffect::Alarm(AlarmChange::Set(due)));
        }
    }

    fn push(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        now: u64,
        (pid, meta, job): (Pid, Meta, J),
        (kind, key, window): (FocusEvent, Option<Vec<i32>>, RuleWindow),
    ) {
        if self.stopping {
            effects.push(ScheduleEffect::Outcome {
                pid,
                meta,
                judged: Judged::Dropped(SHUTTING_DOWN),
            });
            return;
        }
        let app = self.apps.entry(pid).or_insert_with(App::new);
        // An event after its outpost ended: the application runs on.
        app.ending = false;
        // NVDA's UIA limiter keeps one waiting event per element and kind:
        // a newer one erases the older, which is reported dropped.
        let mut replaced = false;
        if let Some(key) = &key {
            let mut kept = VecDeque::with_capacity(app.queue.len());
            for waiting in app.queue.drain(..) {
                if waiting.kind == kind && waiting.key.as_ref() == Some(key) {
                    effects.push(ScheduleEffect::Outcome {
                        pid,
                        meta: waiting.meta,
                        judged: Judged::Dropped(REPLACED),
                    });
                    replaced = true;
                } else {
                    kept.push_back(waiting);
                }
            }
            app.queue = kept;
        }
        // The event replaced may be the last an idle waiter waited for.
        if replaced {
            self.wake_idle_waiters(effects);
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
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        app.queue.push_back(Queued {
            place,
            after_elsewhere,
            left_at: app.left_at,
            kind,
            key,
            window,
            meta,
            job,
        });
        if app.threaded {
            effects.push(ScheduleEffect::Wake(Waking::Thread(pid)));
        } else if app
            .limit_reached
            .is_some_and(|reached| now >= reached + self.deadline)
        {
            // Past the application's bound: dropped at once, behind the
            // events the watchdog has not yet dropped, in order.
            self.drop_waiting(effects, pid, Dropping::AsPushed);
        } else if app.abandoned >= ABANDONED_LIMIT {
            // The alarm bounds its wait ([`App::waiting_end`]).
            tracing::warn!(
                %pid,
                waiting = app.queue.len(),
                "a UIA focus waits: its application's reads have not returned"
            );
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

    /// Puts a thread in charge of `pid`'s events under a new generation:
    /// a new thread, or with `reuse` the thread asking. A thread no longer
    /// in charge, ending or abandoned, never is again.
    fn start(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid, reuse: bool) {
        if let Some(app) = self.apps.get_mut(&pid) {
            let generation = app.flight.succeed();
            app.threaded = true;
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
        if !app.in_charge(generation) {
            return;
        }
        if let Some(queued) = app.queue.pop_front() {
            let running = Running {
                meta: queued.meta,
                kind: queued.kind,
                place: queued.place,
            };
            let request = app.flight.start(running, self.deadline, queued.window, now);
            effects.push(ScheduleEffect::Job(Job {
                pid,
                request,
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
            app.threaded = false;
            if app.spent() {
                self.apps.remove(&pid);
            }
        }
    }

    fn returned(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        (pid, request): (Pid, Request),
        meta: Meta,
        (judged, top): (Judged, Option<Window>),
    ) {
        let stopping = self.stopping;
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        if app.in_charge(request.generation) {
            let read = matches!(judged, Judged::Read(_));
            if read && top.is_some() {
                app.last_top = top;
            }
            match app.flight.end(request) {
                Some((_, true)) => {
                    tracing::info!(
                        %pid,
                        "a UIA focus read published at its bound returned; its result is discarded"
                    );
                }
                Some((_, false)) => effects.push(ScheduleEffect::Outcome { pid, meta, judged }),
                None => {
                    tracing::error!(%pid, "a read returned that its thread was not reading");
                    debug_assert!(false, "a read returned that its thread was not reading");
                }
            }
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
        if app.spent() {
            self.apps.remove(&pid);
            return;
        }
        if app.threaded || app.queue.is_empty() || stopping {
            return;
        }
        self.start(effects, pid, true);
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
            .filter_map(|app| match app.flight.running() {
                Some((_, running, false)) => Some(running.place),
                _ => app.queue.front().map(|queued| queued.place),
            })
            .min();
        while let Some(&(before, waiter)) = self.idle_waiters.front()
            && oldest.is_none_or(|oldest| before < oldest)
        {
            self.idle_waiters.pop_front();
            effects.push(ScheduleEffect::RunIdle(waiter));
        }
    }

    /// The watchdog's alarm went off at `now`: each read under way judges
    /// what is due ([`InFlight::tick`]), and events waiting past their
    /// application's bound at the abandoned limit are dropped.
    fn tick(&mut self, effects: &mut Vec<ScheduleEffect<J>>, now: u64) {
        if self.stopping {
            return;
        }
        let pids: Vec<Pid> = self.apps.keys().copied().collect();
        for pid in pids {
            let Some(app) = self.apps.get_mut(&pid) else {
                continue;
            };
            if app.flight.running().is_some() {
                let decision = app.flight.tick(now);
                self.decided(effects, now, pid, decision);
            } else if app.waiting_end(self.deadline).is_some_and(|end| now >= end) {
                self.drop_waiting(effects, pid, Dropping::Waited);
            }
        }
    }

    /// Does what `pid`'s work in flight decided: asks the shell, or
    /// carries out an abandonment.
    fn decided(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        now: u64,
        pid: Pid,
        decision: Decision<Running>,
    ) {
        match decision {
            Decision::Nothing => {}
            Decision::Ask(Ask::MovedOn { request, window }) => {
                effects.push(ScheduleEffect::AskMovedOn {
                    pid,
                    request,
                    window,
                });
            }
            Decision::Ask(Ask::Publish {
                request,
                then_abandon,
            }) => effects.push(ScheduleEffect::Publish {
                pid,
                request,
                then_abandon,
            }),
            Decision::Abandon(abandoned) => self.abandon(effects, now, pid, abandoned),
        }
    }

    /// `pid`'s read was abandoned: its thread is in charge no more, its
    /// event is dropped unless it was published, and a new thread takes
    /// the events waiting, unless the application's abandoned threads are
    /// at the limit.
    fn abandon(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        now: u64,
        pid: Pid,
        abandoned: Abandoned<Running>,
    ) {
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        let Abandoned {
            reason,
            item: running,
            published,
            ..
        } = abandoned;
        effects.push(ScheduleEffect::Unwatch { pid });
        app.threaded = false;
        app.abandoned += 1;
        let count = app.abandoned;
        if count >= ABANDONED_LIMIT && app.limit_reached.is_none() {
            app.limit_reached = Some(now);
        }
        let why = match reason {
            Reason::MovedOn => {
                tracing::warn!(
                    %pid,
                    abandoned = count,
                    "a UIA focus read was slow and the user moved on to another thread of its application; its thread is abandoned"
                );
                MOVED_ON
            }
            Reason::Deadline => {
                tracing::warn!(%pid, abandoned = count, "a UIA focus read did not return in time; its thread is abandoned");
                NOT_IN_TIME
            }
        };
        let waiting = !app.queue.is_empty();
        // A read published at its bound has had its outcome sent.
        if !published {
            effects.push(ScheduleEffect::Outcome {
                pid,
                meta: running.meta,
                judged: Judged::Dropped(why),
            });
        }
        if waiting {
            if count >= ABANDONED_LIMIT {
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

    /// The shell's answer to a publication asked of `pid`'s read
    /// `request`: the read, when its progress was there, is sent as its
    /// outcome while the read is under way and not published, which
    /// publishes it. Then the work in flight makes the abandonment the ask
    /// carried, or, with none, judges what is left at `now`.
    fn published(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        now: u64,
        (pid, request): (Pid, Request),
        read: Option<Box<UiaFocusRead>>,
        then_abandon: Option<Reason>,
    ) {
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        let meta = app
            .flight
            .running()
            .filter(|(running, ..)| app.flight.accepts(*running) && *running == request)
            .map(|(_, running, _)| running.meta);
        let got = match (meta, read) {
            (Some(meta), Some(read)) => {
                tracing::info!(
                    %pid,
                    ancestors = read.ancestors.len(),
                    "a UIA focus read's call was still under way at its bound; the focus is published from what it read"
                );
                effects.push(ScheduleEffect::Outcome {
                    pid,
                    meta,
                    judged: Judged::Read(read),
                });
                true
            }
            _ => false,
        };
        let decision = app.flight.published(request, got, then_abandon, now);
        if got {
            self.wake_idle_waiters(effects);
        }
        self.decided(effects, now, pid, decision);
    }

    /// Drops every event of `pid`'s waiting with no thread to read it, in
    /// the order they arrived.
    fn drop_waiting(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid, dropping: Dropping) {
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        if app.threaded || app.queue.is_empty() {
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
            });
        }
        self.wake_idle_waiters(effects);
    }

    /// A foreground event of top-level window `top_level`: for menu
    /// currentness, it moves away from each application whose read under
    /// way, once it knows its window, or else whose last read, is of
    /// another top-level window; a read under way whose window is unknown
    /// counts as moving away. It judges nothing else.
    fn foreground(&mut self, top_level: Option<Window>, running_tops: &[(Pid, Option<Window>)]) {
        let place = self.queued;
        for (pid, app) in &mut self.apps {
            let reference = if app.flight.running().is_some() {
                running_tops
                    .iter()
                    .find(|(owner, _)| owner == pid)
                    .and_then(|&(_, top)| top)
            } else {
                app.last_top
            };
            if reference.is_none() || reference != top_level {
                app.left_at = place;
            }
        }
    }

    fn outpost_ended(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid) {
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        app.ending = true;
        if app.spent() {
            self.apps.remove(&pid);
            return;
        }
        effects.push(ScheduleEffect::Wake(Waking::Thread(pid)));
    }

    fn spawn_failed(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid, generation: u64) {
        let Some(app) = self.apps.get_mut(&pid) else {
            return;
        };
        if !app.in_charge(generation) {
            return;
        }
        tracing::error!(%pid, "no thread to read UIA focus events; they are dropped");
        app.threaded = false;
        for queued in app.queue.drain(..) {
            effects.push(ScheduleEffect::Outcome {
                pid,
                meta: queued.meta,
                judged: Judged::Dropped(NO_THREAD),
            });
        }
        if app.spent() {
            self.apps.remove(&pid);
        }
        self.wake_idle_waiters(effects);
    }
}

/// What waits for a menu end's grace to pass (`listener`'s `MenuEnd`).
#[derive(Clone, Debug, Default)]
pub struct GraceState {
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
pub enum GraceInput {
    /// A menu or the switcher ended at `ended_at_us`, its grace of `grace`
    /// microseconds starting at `now`.
    Ended {
        /// When the menu or the switcher ended, in microseconds on the shared
        /// clock.
        ended_at_us: u64,
        /// The end's timing, for the latency log.
        timing: EventTiming,
        /// The grace, in microseconds.
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
pub enum GraceEffect {
    /// Send the end.
    SendEnd {
        /// When the menu or the switcher ended, in microseconds on the shared
        /// clock.
        ended_at_us: u64,
        /// The end's timing, for the latency log.
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
    pub fn step(&mut self, input: GraceInput, now: u64) -> Vec<GraceEffect> {
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
mod explore;
#[cfg(test)]
mod generated;

#[cfg(test)]
mod tests {
    use verbatim_model::TraceId;

    use super::*;

    const DEADLINE: u64 = 10_000_000;

    /// The moved-on grace, in microseconds.
    const GRACE: u64 = 500_000;

    /// The window the tests' events are of.
    const WINDOW: Window = match Window::new(0x10) {
        Some(window) => window,
        None => panic!("0x10 is not 0"),
    };

    fn schedule() -> Schedule<u64> {
        Schedule::new(Duration::from_micros(DEADLINE))
    }

    fn meta(marker: u64, kind: FocusEvent) -> Meta {
        Meta {
            marker,
            kind,
            trace: TraceId::from_raw(1),
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
            window: RuleWindow::Known(WINDOW),
        }
    }

    fn focus(pid: u32, marker: u64) -> ScheduleInput<u64> {
        push(pid, marker, FocusEvent::Focus)
    }

    fn next(pid: u32, generation: u64) -> ScheduleInput<u64> {
        ScheduleInput::Next {
            pid: Pid(pid),
            generation,
        }
    }

    fn request(generation: u64, number: u64) -> Request {
        Request { generation, number }
    }

    /// One effect as these tests compare it: an outcome by its marker and
    /// reason, a job by its marker and queue place, the rest whole.
    #[derive(Debug, PartialEq, Eq)]
    enum Seen {
        Outcome(u32, u64, &'static str),
        Spawn(u32, u64, bool),
        Wake(Waking),
        Unwatch(u32),
        ForgetLast,
        Alarm(AlarmChange),
        RunIdle(u64),
        Job(u32, u64, QueuePlace),
        AskMovedOn(u32, Request),
        Publish(u32, Request, Option<Reason>),
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
                ScheduleEffect::Unwatch { pid } => Seen::Unwatch(pid.0),
                ScheduleEffect::ForgetLast => Seen::ForgetLast,
                ScheduleEffect::Alarm(change) => Seen::Alarm(change),
                ScheduleEffect::RunIdle(waiter) => Seen::RunIdle(waiter),
                ScheduleEffect::Job(job) => Seen::Job(job.pid.0, job.meta.marker, job.queue),
                ScheduleEffect::AskMovedOn {
                    pid,
                    request,
                    window,
                } => {
                    assert_eq!(window, WINDOW);
                    Seen::AskMovedOn(pid.0, request)
                }
                ScheduleEffect::Publish {
                    pid,
                    request,
                    then_abandon,
                } => Seen::Publish(pid.0, request, then_abandon),
            })
            .collect()
    }

    fn step(schedule: &mut Schedule<u64>, input: ScheduleInput<u64>, now: u64) -> Vec<Seen> {
        seen(schedule.step(input, now))
    }

    fn set(due: u64) -> Seen {
        Seen::Alarm(AlarmChange::Set(Some(due)))
    }

    const CANCEL: Seen = Seen::Alarm(AlarmChange::Set(None));

    fn returned(pid: u32, request: Request, marker: u64) -> ScheduleInput<u64> {
        ScheduleInput::Returned {
            pid: Pid(pid),
            request,
            meta: meta(marker, FocusEvent::Focus),
            judged: Judged::Dropped("read"),
            top: None,
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

    /// The queue place of the job among `effects`.
    fn job_place(effects: &[Seen], marker: u64) -> QueuePlace {
        effects
            .iter()
            .find_map(|effect| match effect {
                Seen::Job(_, job, queue) if *job == marker => Some(*queue),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no job of {marker}: {effects:?}"))
    }

    /// Takes application 1's two reads past their deadlines, abandoning
    /// both threads so the application is at the abandoned limit, its
    /// third event waiting with no thread.
    fn at_the_limit(schedule: &mut Schedule<u64>) {
        assert_eq!(step(schedule, focus(1, 1), 0), [Seen::Spawn(1, 1, false)]);
        let _ = step(schedule, next(1, 1), 0);
        assert_eq!(
            step(schedule, focus(1, 2), 0),
            [Seen::Wake(Waking::Thread(Pid(1)))]
        );
        assert_eq!(
            step(schedule, ScheduleInput::Tick, DEADLINE),
            [
                Seen::Unwatch(1),
                Seen::Outcome(1, 1, NOT_IN_TIME),
                Seen::Spawn(1, 3, false),
            ],
            "the alarm went off and nothing else is due: it is left unset"
        );
        let _ = step(schedule, next(1, 3), DEADLINE);
        assert_eq!(
            step(schedule, ScheduleInput::Tick, 2 * DEADLINE),
            [Seen::Unwatch(1), Seen::Outcome(1, 2, NOT_IN_TIME)]
        );
        assert_eq!(
            step(schedule, focus(1, 3), 2 * DEADLINE),
            [set(3 * DEADLINE)],
            "it waits, the alarm set to the application's bound: the application is at the limit"
        );
    }

    /// An event at the abandoned limit is read by the first abandoned
    /// thread that returns, under a new generation, which forgets its own
    /// last read and goes on in charge of the application's events.
    #[test]
    fn an_event_at_the_abandoned_limit_is_read_by_a_returning_thread() {
        let mut schedule = schedule();
        at_the_limit(&mut schedule);
        assert_eq!(
            step(
                &mut schedule,
                returned(1, request(1, 1), 1),
                2 * DEADLINE + 1
            ),
            [Seen::ForgetLast, Seen::Spawn(1, 5, true), CANCEL]
        );
        assert_eq!(
            step(&mut schedule, next(1, 5), 2 * DEADLINE + 1),
            [
                Seen::Job(
                    1,
                    3,
                    QueuePlace {
                        place: 3,
                        left_at: 0
                    }
                ),
                set(2 * DEADLINE + 1 + GRACE),
            ]
        );
    }

    /// An abandoned read's late return sends no outcome, its drop having
    /// been sent, and has its thread forget its last read.
    #[test]
    fn an_abandoned_reads_late_return_sends_no_outcome_and_forgets_its_read() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        let _ = step(&mut schedule, ScheduleInput::Tick, DEADLINE);
        assert_eq!(
            step(&mut schedule, returned(1, request(1, 1), 1), DEADLINE + 5),
            [Seen::ForgetLast]
        );
        assert!(!schedule.in_charge(Pid(1), 1), "the thread ends");
    }

    /// A read's start sets the alarm to its grace's end, and a quick read's
    /// end moves it back to whatever else is due, another application's
    /// read under way, or cancels it.
    #[test]
    fn a_quick_reads_end_moves_the_alarm_back_while_another_read_is_under_way() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(2, 1), 0);
        assert_eq!(
            step(&mut schedule, next(2, 1), 0),
            [
                Seen::Job(
                    2,
                    1,
                    QueuePlace {
                        place: 1,
                        left_at: 0
                    }
                ),
                set(GRACE)
            ]
        );
        let _ = step(&mut schedule, focus(1, 2), 100);
        assert_eq!(
            step(&mut schedule, next(1, 1), 100),
            [Seen::Job(
                1,
                2,
                QueuePlace {
                    place: 2,
                    left_at: 0
                }
            )],
            "the alarm is already due sooner"
        );
        assert_eq!(
            step(&mut schedule, returned(2, request(1, 1), 1), 200),
            [Seen::Outcome(2, 1, "read"), set(100 + GRACE)]
        );
        assert_eq!(
            step(&mut schedule, returned(1, request(1, 1), 2), 300),
            [Seen::Outcome(1, 2, "read"), CANCEL]
        );
    }

    /// A tick spends the watchdog's one-shot timer: one a little before the
    /// time due, as the timer can go off against the shell's clock, judges
    /// nothing and sets the alarm again to the same time.
    #[test]
    fn an_early_tick_sets_the_alarm_again() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        assert_eq!(
            step(&mut schedule, ScheduleInput::Tick, GRACE - 1),
            [set(GRACE)]
        );
    }

    /// A read is judged at its grace's end and every grace after, with no
    /// foreground event; a foreground event judges nothing. The user moved
    /// on: the read is abandoned, and a new thread takes the next event.
    #[test]
    fn a_read_is_judged_every_half_second_and_abandoned_once_the_user_moved_on() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        let _ = step(&mut schedule, focus(1, 2), 1);
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::Foreground {
                    top_level: Window::new(0x20),
                    running_tops: vec![(Pid(1), Some(WINDOW))],
                },
                2,
            ),
            [],
            "a foreground event judges nothing and wakes nothing"
        );
        let asked = request(1, 1);
        assert_eq!(
            step(&mut schedule, ScheduleInput::Tick, GRACE),
            [Seen::AskMovedOn(1, asked), set(2 * GRACE)]
        );
        let moved_on = |moved_on| ScheduleInput::MovedOn {
            pid: Pid(1),
            request: asked,
            moved_on,
        };
        assert_eq!(step(&mut schedule, moved_on(false), GRACE), []);
        assert_eq!(
            step(&mut schedule, ScheduleInput::Tick, 2 * GRACE),
            [Seen::AskMovedOn(1, asked), set(3 * GRACE)]
        );
        assert_eq!(
            step(&mut schedule, moved_on(true), 2 * GRACE),
            [
                Seen::Unwatch(1),
                Seen::Outcome(1, 1, MOVED_ON),
                Seen::Spawn(1, 3, false),
                CANCEL,
            ]
        );
    }

    /// A read whose walk is armed is published before it is abandoned when
    /// the user moves on: its outcome is the read, no drop follows, and its
    /// late return sends nothing.
    #[test]
    fn a_read_is_published_before_it_is_abandoned_for_moving_on() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        let asked = request(1, 1);
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::Armed {
                    pid: Pid(1),
                    request: asked,
                    bound: 2_000_000,
                },
                10,
            ),
            []
        );
        let _ = step(&mut schedule, ScheduleInput::Tick, GRACE);
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::MovedOn {
                    pid: Pid(1),
                    request: asked,
                    moved_on: true,
                },
                GRACE,
            ),
            [Seen::Publish(1, asked, Some(Reason::MovedOn))]
        );
        assert_eq!(schedule.markers(), [1], "not yet published");
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::Published {
                    pid: Pid(1),
                    request: asked,
                    read: Some(a_read()),
                    then_abandon: Some(Reason::MovedOn),
                },
                GRACE,
            ),
            [Seen::Outcome(1, 1, "read"), Seen::Unwatch(1), CANCEL]
        );
        assert_eq!(schedule.markers(), Vec::<u64>::new());
        assert_eq!(
            step(&mut schedule, returned(1, asked, 1), 3_000_000),
            [Seen::ForgetLast]
        );
    }

    /// A walk under way at its bound is published, its read's outcome sent
    /// then; its return sends nothing more, and its thread reads on.
    #[test]
    fn a_walk_under_way_at_its_bound_is_published_and_its_return_discarded() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        let asked = request(1, 1);
        let _ = step(
            &mut schedule,
            ScheduleInput::Armed {
                pid: Pid(1),
                request: asked,
                bound: 300_000,
            },
            10,
        );
        assert_eq!(
            step(&mut schedule, ScheduleInput::Tick, 300_000),
            [Seen::Publish(1, asked, None), set(GRACE)]
        );
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::Published {
                    pid: Pid(1),
                    request: asked,
                    read: Some(a_read()),
                    then_abandon: None,
                },
                300_000,
            ),
            [Seen::Outcome(1, 1, "read")]
        );
        assert!(!schedule.focus_pending(None), "published");
        assert_eq!(
            step(&mut schedule, returned(1, asked, 1), 400_000),
            [CANCEL]
        );
        assert!(schedule.in_charge(Pid(1), 1), "its thread reads on");
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
            step(&mut schedule, ScheduleInput::Tick, 2 * DEADLINE + 1),
            [set(3 * DEADLINE)],
            "an early tick drops nothing and sets the alarm again"
        );
        assert_eq!(
            step(&mut schedule, ScheduleInput::Tick, 3 * DEADLINE),
            [Seen::Outcome(1, 3, WAITED_TOO_LONG)]
        );
    }

    /// An idle waiter runs once every event queued before it has its
    /// outcome, whatever was queued after it.
    #[test]
    fn an_idle_waiter_runs_with_later_events_queued() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        assert_eq!(
            step(&mut schedule, ScheduleInput::WhenIdle { waiter: 7 }, 0),
            []
        );
        let _ = step(&mut schedule, focus(1, 2), 1);
        assert_eq!(
            step(&mut schedule, returned(1, request(1, 1), 1), 2),
            [Seen::Outcome(1, 1, "read"), Seen::RunIdle(7), CANCEL]
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
            window: RuleWindow::Known(WINDOW),
        };
        effects.extend(step(&mut schedule, focus(1, 1), 0));
        effects.extend(step(&mut schedule, next(1, 1), 0));
        effects.extend(step(&mut schedule, keyed(2), 1));
        effects.extend(step(&mut schedule, keyed(3), 2));
        effects.extend(step(&mut schedule, ScheduleInput::Tick, DEADLINE));
        effects.extend(step(&mut schedule, next(1, 3), DEADLINE));
        effects.extend(step(
            &mut schedule,
            returned(1, request(1, 1), 1),
            DEADLINE + 1,
        ));
        effects.extend(step(
            &mut schedule,
            returned(1, request(3, 2), 3),
            DEADLINE + 2,
        ));
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
            let _ = step(&mut schedule, next(1, 1), 0);
            let _ = step(
                &mut schedule,
                ScheduleInput::Returned {
                    pid: Pid(1),
                    request: request(1, 1),
                    meta: meta(1, FocusEvent::Focus),
                    judged: Judged::Read(a_read()),
                    top: Some(WINDOW),
                },
                1,
            );
            moving(&mut schedule);
            let _ = step(&mut schedule, push(1, 9, FocusEvent::MenuOpened), 2);
            let effects = step(&mut schedule, next(1, 1), 2);
            job_place(&effects, 9).current_since(1)
        };
        assert!(opening_after(&|_| {}), "nothing moved away");
        assert!(
            opening_after(&|schedule| {
                let _ = schedule.step(
                    ScheduleInput::Foreground {
                        top_level: Some(WINDOW),
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
                        top_level: Window::new(0x30),
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
        let _ = step(&mut schedule, next(1, 1), 0);
        let _ = step(
            &mut schedule,
            ScheduleInput::Foreground {
                top_level: Some(WINDOW),
                running_tops: vec![(Pid(1), None)],
            },
            1,
        );
        let _ = step(&mut schedule, push(1, 9, FocusEvent::MenuOpened), 2);
        let _ = step(&mut schedule, returned(1, request(1, 1), 1), 3);
        let effects = step(&mut schedule, next(1, 1), 3);
        assert!(
            !job_place(&effects, 9).current_since(1),
            "a read whose window is unknown counts as moved away from"
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
    /// Once the listener stops nothing is judged: a window a read learns
    /// past its grace asks nothing.
    #[test]
    fn a_window_learned_while_stopping_is_not_judged() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, push(1, 1, FocusEvent::Focus), 0);
        let _ = step(
            &mut schedule,
            ScheduleInput::Push {
                pid: Pid(1),
                meta: meta(2, FocusEvent::Focus),
                job: 2,
                kind: FocusEvent::Focus,
                key: None,
                window: RuleWindow::Unknown,
            },
            0,
        );
        let _ = step(&mut schedule, next(1, 1), 0);
        let _ = step(&mut schedule, returned(1, request(1, 1), 1), 1);
        let _ = step(&mut schedule, next(1, 1), 1);
        let _ = step(&mut schedule, ScheduleInput::Stop, 2);
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::Learned {
                    pid: Pid(1),
                    request: request(1, 2),
                    window: WINDOW,
                },
                DEADLINE,
            ),
            []
        );
    }

    /// Found by the read scheduler's generated test
    /// (`docs/design/logic-tests.md`, "The listener's scheduler"): an idle
    /// waiter runs in the push whose replacement gives the one event
    /// queued before it its outcome, not at a later step.
    #[test]
    fn a_waiter_runs_as_a_push_replaces_the_event_it_waited_for() {
        let mut schedule = schedule();
        let keyed = |marker| ScheduleInput::Push {
            pid: Pid(1),
            meta: meta(marker, FocusEvent::MenuOpened),
            job: marker,
            kind: FocusEvent::MenuOpened,
            key: Some(vec![0]),
            window: RuleWindow::Unknown,
        };
        assert_eq!(step(&mut schedule, keyed(1), 0), [Seen::Spawn(1, 1, false)]);
        assert_eq!(
            step(&mut schedule, ScheduleInput::WhenIdle { waiter: 7 }, 0),
            []
        );
        assert_eq!(
            step(&mut schedule, keyed(2), 1),
            [
                Seen::Outcome(1, 1, REPLACED),
                Seen::RunIdle(7),
                Seen::Wake(Waking::Thread(Pid(1))),
            ]
        );
    }

    /// Found by the read scheduler's generated test
    /// (`docs/design/logic-tests.md`, "The listener's scheduler"): an
    /// application whose outpost has ended is let go once its last
    /// abandoned read returns with nothing waiting.
    #[test]
    fn an_ended_application_is_let_go_once_its_last_abandoned_read_returns() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        assert_eq!(
            step(&mut schedule, ScheduleInput::Tick, DEADLINE),
            [Seen::Unwatch(1), Seen::Outcome(1, 1, NOT_IN_TIME)]
        );
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::OutpostEnded { pid: Pid(1) },
                DEADLINE
            ),
            [Seen::Wake(Waking::Thread(Pid(1)))],
            "held while its abandoned thread is out"
        );
        assert_eq!(
            step(&mut schedule, returned(1, request(1, 1), 1), DEADLINE + 1),
            [Seen::ForgetLast]
        );
        assert!(schedule.apps.is_empty(), "let go");
    }

    /// Found by the read scheduler's generated test: an application whose
    /// outpost has ended is let go once its thread cannot start and its
    /// waiting events are dropped.
    #[test]
    fn an_ended_application_whose_thread_cannot_start_is_let_go() {
        let mut schedule = schedule();
        assert_eq!(
            step(&mut schedule, focus(1, 1), 0),
            [Seen::Spawn(1, 1, false)]
        );
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::OutpostEnded { pid: Pid(1) },
                0
            ),
            [Seen::Wake(Waking::Thread(Pid(1)))]
        );
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::SpawnFailed {
                    pid: Pid(1),
                    generation: 1
                },
                0
            ),
            [Seen::Outcome(1, 1, NO_THREAD)]
        );
        assert!(schedule.apps.is_empty(), "let go");
    }
}
