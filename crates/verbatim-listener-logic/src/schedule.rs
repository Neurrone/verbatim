//! The focus listener's read scheduler as a pure transition
//! (`books/dev/src/explanation/design/ownership.md`): which reading
//! thread reads which application's UIA event, when a read is abandoned,
//! when an event waiting behind abandoned reads is
//! dropped, which idle waiters may run, and whether a menu opening's
//! reading thread may compare it with its own last read.
//!
//! [`Schedule::step`] takes one [`ScheduleInput`] and the time, in
//! microseconds on the shell's monotonic clock, and returns the
//! [`ScheduleEffect`]s, in the order the shell applies them under its lock.
//! Records are keyed by application generation ([`AppKey`]): a process's
//! exit moves its record out of the running applications, so no later
//! event reaches it. Each application's read under way is its work in flight
//! ([`InFlight`], `books/dev/src/reference/crates/verbatim-reader-rules.md`), which decides its abandonment, at
//! its deadline or by the half-second judgement of whether the user has
//! moved on; what it needs of Windows it asks by an effect
//! ([`ScheduleEffect::Judge`]), which the shell answers by an input once
//! the step's other effects are applied, before it lets its lock go. The schedule also owns the
//! watchdog's alarm: after every step it sets it to the soonest time
//! anything is due. The schedule makes no call, reads no clock and holds
//! no lock: the shell (`verbatim_listener::focus_read::Readers`) keeps the
//! lock, the threads, their condition variables, the alarm's timer, the
//! idle waiters' closures and each read's window. So every ordering of
//! inputs is a replay test of this module.
//!
//! The menu end's grace is a second pure transition here,
//! [`GraceState::step`], which the listener's threadpool timer and its
//! callers drive.

use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroUsize;
use std::time::Duration;

use verbatim_model::Pid;
use verbatim_reader_rules::Window;
use verbatim_reader_rules::in_flight::{Abandoned, Ask, Decision, InFlight, Reason, RuleWindow};

pub use verbatim_reader_rules::in_flight::Request;

use crate::focus::{
    ABANDONED_LIMIT, BAR_VERDICTS_KEPT, BarObject, BarVerdict, FocusEvent, Judged, MOVED_ON, Meta,
    NOT_IN_TIME, PROCESS_EXITED, REPLACED, WAITED_TOO_LONG,
};
use verbatim_model::protocol::EventTiming;

/// Why an event is dropped when the listener is shutting down.
const SHUTTING_DOWN: &str = "the listener is shutting down";

/// Why an event is dropped when no thread could be had to read it.
const NO_THREAD: &str = "no thread to read it";

/// One run of one process: a process id reused by a new process is a new
/// generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AppKey {
    /// The process id.
    pub pid: Pid,
    /// Which run of that id.
    pub generation: u64,
}

/// One input to the schedule.
pub enum ScheduleInput<J> {
    /// A UIA event of `pid`'s, `meta`, arrived: `job` is what its read
    /// takes, `kind` its kind as the reading says it, `key` the element it
    /// is of for NVDA's limiter (a newer event of one kind of one element
    /// replaces a waiting one), and `window` the window it is of as far as
    /// its cache says, `None` for an event never judged by the moved-on
    /// rule and `Some(0)` for one whose read is still to learn it. It joins
    /// the application running under `pid`, or starts a new generation.
    Push {
        /// The event's process.
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
    /// The reading thread `generation` of `app` asks for its next event.
    Next {
        /// The thread's application.
        app: AppKey,
        /// The thread's generation.
        generation: u64,
    },
    /// A reading thread of `app` returned from its read `request` of
    /// `meta`'s event with `judged`.
    Returned {
        /// The thread's application.
        app: AppKey,
        /// The read, by the request its thread read it as.
        request: Request,
        /// What the callback knew of the event read.
        meta: Meta,
        /// What became of it.
        judged: Judged,
    },
    /// The watchdog's alarm went off, which spends its timer.
    Tick,
    /// `app`'s read `request` learned its window, `window`.
    Learned {
        /// The read's application.
        app: AppKey,
        /// The read.
        request: Request,
        /// Its window.
        window: Window,
    },
    /// `app`'s read `request` found its top-level window, `top`: sent once
    /// per read.
    Placed {
        /// The read's application.
        app: AppKey,
        /// The read.
        request: Request,
        /// Its top-level window.
        top: Window,
    },
    /// A reading thread found what `object` of `pid`'s is: remembered in the
    /// running application's record (a process that exited has none), so
    /// the hook decides its later changes with no call.
    BarLearned {
        /// The process.
        pid: Pid,
        /// The object.
        object: BarObject,
        /// What it is.
        verdict: BarVerdict,
    },
    /// The shell's answer to [`ScheduleEffect::Judge`]: why the read is
    /// abandoned, [`Reason::MovedOn`] when the user has left its window for
    /// another thread of its application; `None` when it stays.
    Judged {
        /// The read's application.
        app: AppKey,
        /// The read asked about.
        request: Request,
        /// The verdict.
        verdict: Option<Reason>,
    },
    /// A foreground event reached the listener's hook, of top-level window
    /// `top_level`. For menu currentness only.
    Foreground {
        /// The foreground window's top-level window, if any.
        top_level: Option<Window>,
    },
    /// The listener's `WinEvent` thread forwarded an MSAA focus of `pid`'s.
    MsaaFocus {
        /// The focus's process.
        pid: Pid,
    },
    /// The outpost of the application running under `pid` has ended, or no
    /// outpost's hook is in for it: its reader is let go once it has
    /// nothing left to do, unless another event of it comes.
    OutpostEnded {
        /// The application's process.
        pid: Pid,
    },
    /// The process running under `pid` has exited
    /// (`CoreToListener::ApplicationExited`): its record leaves the running
    /// applications under its [`AppKey`].
    ApplicationExited {
        /// The process that exited.
        pid: Pid,
    },
    /// The shell holds idle waiter `waiter`, to run once every event
    /// queued before it has its outcome.
    WhenIdle {
        /// The waiter's number.
        waiter: u64,
    },
    /// No thread could be started for `app`'s events as `generation`.
    SpawnFailed {
        /// The application.
        app: AppKey,
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
    Thread(AppKey),
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
        /// The event's process.
        pid: Pid,
        /// What the callback knew of the event.
        meta: Meta,
        /// What became of it.
        judged: Judged,
    },
    /// A thread is in charge of `app`'s events as `generation`: a new one,
    /// or, with `reuse`, the thread whose abandoned read just returned.
    Spawn {
        /// The application.
        app: AppKey,
        /// The thread's generation.
        generation: u64,
        /// Whether the thread whose abandoned read just returned is reused.
        reuse: bool,
    },
    /// Wake waiting reading threads.
    Wake(Waking),
    /// `app`'s read under way was abandoned: the shell watches it no more.
    Unwatch {
        /// The read's application.
        app: AppKey,
    },
    /// The thread whose abandoned read returned forgets its last read: Core
    /// never got that focus.
    ForgetLast,
    /// `app`'s record is dropped: no thread is in charge of its events or
    /// abandoned to a read of them, and none waits, so no thread waits on
    /// what the shell keeps for it or returns to it, and the shell drops
    /// it. No later effect of the step names `app`.
    Forget {
        /// The application let go.
        app: AppKey,
    },
    /// The watchdog's alarm.
    Alarm(AlarmChange),
    /// Run the idle waiter.
    RunIdle(u64),
    /// The event the asking thread reads next.
    Job(Job<J>),
    /// Judge `window`, the window of `app`'s read `request`: whether the
    /// user has left it for another thread of its application; answered by
    /// [`ScheduleInput::Judged`] once the step's other effects are applied.
    Judge {
        /// The read's application.
        app: AppKey,
        /// The read.
        request: Request,
        /// Its window.
        window: Window,
    },
    /// A state the schedule's rules exclude was reached: a logic error, for
    /// the shell to log. The step's input changed nothing.
    LogicError(&'static str),
}

/// The event a reading thread reads next.
#[derive(Debug)]
pub struct Job<J> {
    /// The event's application.
    pub app: AppKey,
    /// The read, which its learned window and its return name.
    pub request: Request,
    /// What the callback knew of it.
    pub meta: Meta,
    /// What its read takes.
    pub work: J,
    /// The window the event is of as far as its cache says, which the
    /// moved-on rule judges its read by.
    pub window: RuleWindow,
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
struct UnderWay {
    meta: Meta,
    kind: FocusEvent,
    /// Its place in the order every event was queued in.
    place: u64,
    /// The read's top-level window, once it has found it
    /// ([`ScheduleInput::Placed`]).
    top: Option<Window>,
}

/// What a record keeps whatever its reader's state.
#[derive(Default)]
struct App {
    /// The place of the newest event that moved away from it.
    left_at: u64,
    /// The top-level window of its last read focus, if any.
    last_top: Option<Window>,
    /// The generation and request numbers a new [`Charge`]'s work in flight
    /// continues from, read only when one starts, so a thread that has lost
    /// charge can never match a later one of its record.
    counters: InFlight<UnderWay>,
    /// What the objects whose values changed were found to be, the oldest
    /// first, at most [`BAR_VERDICTS_KEPT`]. Gone with the record, as a
    /// record with nothing left is dropped.
    bar_verdicts: VecDeque<(BarObject, BarVerdict)>,
}

impl App {
    /// Remembers what `object` was found to be, forgetting the oldest
    /// answer past [`BAR_VERDICTS_KEPT`].
    fn remember_bar(&mut self, object: BarObject, verdict: BarVerdict) {
        self.bar_verdicts.retain(|(known, _)| *known != object);
        self.bar_verdicts.push_back((object, verdict));
        if self.bar_verdicts.len() > BAR_VERDICTS_KEPT {
            self.bar_verdicts.pop_front();
        }
    }
}

/// A thread in charge, fewer than [`ABANDONED_LIMIT`] abandoned.
struct Charge<J> {
    abandoned: usize,
    /// The read under way and the generation in charge.
    flight: InFlight<UnderWay>,
    queue: VecDeque<Queued<J>>,
}

impl<J> Charge<J> {
    /// A new thread's charge of `queue`, continuing `counters` and
    /// `abandoned`, and the generation it is in charge as.
    fn begin(
        counters: &mut InFlight<UnderWay>,
        abandoned: usize,
        queue: VecDeque<Queued<J>>,
    ) -> (Self, u64) {
        let mut flight = std::mem::take(counters);
        let generation = flight.succeed();
        (
            Self {
                abandoned,
                flight,
                queue,
            },
            generation,
        )
    }

    /// Whether the read under way, or an event waiting, is a focus.
    fn holds_focus(&self) -> bool {
        self.flight
            .running()
            .is_some_and(|(_, read)| read.kind == FocusEvent::Focus)
            || self
                .queue
                .iter()
                .any(|queued| queued.kind == FocusEvent::Focus)
    }

    /// The place of the oldest event it holds.
    fn oldest(&self) -> Option<u64> {
        match self.flight.running() {
            Some((_, read)) => Some(read.place),
            None => self.queue.front().map(|queued| queued.place),
        }
    }
}

/// The reader of an application still running.
#[expect(
    clippy::large_enum_variant,
    reason = "one per application with readers, never stored in bulk"
)]
enum Reader<J> {
    /// No thread in charge, nothing queued, `abandoned` readers still out,
    /// at least one.
    Idle { abandoned: NonZeroUsize },
    /// A thread in charge.
    InCharge(Charge<J>),
    /// [`ABANDONED_LIMIT`] abandoned since `since`, no thread in charge,
    /// events may wait.
    Starved {
        since: u64,
        queue: VecDeque<Queued<J>>,
    },
}

impl<J> Reader<J> {
    fn charge(&self) -> Option<&Charge<J>> {
        match self {
            Reader::InCharge(charge) => Some(charge),
            Reader::Idle { .. } | Reader::Starved { .. } => None,
        }
    }

    fn charge_mut(&mut self) -> Option<&mut Charge<J>> {
        match self {
            Reader::InCharge(charge) => Some(charge),
            Reader::Idle { .. } | Reader::Starved { .. } => None,
        }
    }

    /// The events waiting, when any can.
    fn queue_mut(&mut self) -> Option<&mut VecDeque<Queued<J>>> {
        match self {
            Reader::InCharge(charge) => Some(&mut charge.queue),
            Reader::Starved { queue, .. } => Some(queue),
            Reader::Idle { .. } => None,
        }
    }
}

/// The reader of a process that has exited: no event can reach it, and only
/// its readers are left.
#[expect(
    clippy::large_enum_variant,
    reason = "one per application with readers, never stored in bulk"
)]
enum Draining<J> {
    /// No thread in charge, `abandoned` readers still out, at least one.
    Idle { abandoned: NonZeroUsize },
    /// A thread in charge.
    InCharge(Charge<J>),
}

impl<J> Draining<J> {
    fn charge(&self) -> Option<&Charge<J>> {
        match self {
            Draining::InCharge(charge) => Some(charge),
            Draining::Idle { .. } => None,
        }
    }

    fn charge_mut(&mut self) -> Option<&mut Charge<J>> {
        match self {
            Draining::InCharge(charge) => Some(charge),
            Draining::Idle { .. } => None,
        }
    }
}

/// A running application's life: `Ending` is the supervisor's request to
/// let go of the reader when it has nothing to do.
enum Life<J> {
    Live(Reader<J>),
    Ending(Reader<J>),
}

impl<J> Life<J> {
    fn reader(&self) -> &Reader<J> {
        match self {
            Life::Live(reader) | Life::Ending(reader) => reader,
        }
    }

    fn reader_mut(&mut self) -> &mut Reader<J> {
        match self {
            Life::Live(reader) | Life::Ending(reader) => reader,
        }
    }

    /// Whether the supervisor asked that the reader be let go.
    fn ending(&self) -> bool {
        matches!(self, Life::Ending(_))
    }

    /// Replaces the value by `to` of its reader.
    fn change(&mut self, to: impl FnOnce(Reader<J>) -> Life<J>) {
        // The placeholder is never seen: `to` makes the value that replaces
        // it before anything can read it.
        let placeholder = Life::Live(Reader::Idle {
            abandoned: NonZeroUsize::MIN,
        });
        let reader = match std::mem::replace(self, placeholder) {
            Life::Live(reader) | Life::Ending(reader) => reader,
        };
        *self = to(reader);
    }
}

/// A running application, one per process id.
struct Running<J> {
    generation: u64,
    life: Life<J>,
    app: App,
}

/// A process that has exited, kept for its readers alone.
struct Exited<J> {
    draining: Draining<J>,
    app: App,
}

/// A record found by its key, for the rules that hold in both maps.
enum Slot<'a, J> {
    Running {
        reader: &'a mut Reader<J>,
        app: &'a mut App,
        ending: bool,
    },
    Exited {
        draining: &'a mut Draining<J>,
        app: &'a mut App,
    },
}

impl<J> Slot<'_, J> {
    fn charge(&mut self) -> Option<&mut Charge<J>> {
        match self {
            Slot::Running { reader, .. } => reader.charge_mut(),
            Slot::Exited { draining, .. } => draining.charge_mut(),
        }
    }

    fn charge_of(&mut self, generation: u64) -> Option<&mut Charge<J>> {
        self.charge()
            .filter(|charge| charge.flight.generation() == generation)
    }

    /// Whether the reader is let go once it has nothing left to do.
    fn lets_go(&self) -> bool {
        match self {
            Slot::Running { ending, .. } => *ending,
            Slot::Exited { .. } => true,
        }
    }

    fn app(&mut self) -> &mut App {
        match self {
            Slot::Running { app, .. } | Slot::Exited { app, .. } => app,
        }
    }
}

/// `ABANDONED_LIMIT`, as the count of an idle record at the limit.
const AT_LIMIT: NonZeroUsize = match NonZeroUsize::new(ABANDONED_LIMIT) {
    Some(count) => count,
    None => panic!("the abandoned limit is above 0"),
};

/// One less than `ABANDONED_LIMIT`: a return from the limit leaves readers
/// out, as the limit is at least 2.
const BELOW_LIMIT: NonZeroUsize = match NonZeroUsize::new(ABANDONED_LIMIT - 1) {
    Some(count) => count,
    None => panic!("the abandoned limit is above 1"),
};

/// The logic error of a read that returned of a record that is not held: a
/// thread out keeps its record.
const RETURN_UNHELD: &str = "a read returned of a record no longer held";

/// The logic error of a report of a read of a record that is not held.
const REPORT_UNHELD: &str = "a read's report came for a record no longer held";

/// The logic error of an abandoned read's return with none abandoned.
const NONE_ABANDONED: &str = "an abandoned read returned with none abandoned";

/// The logic error of a read abandoned with no thread in charge.
const ABANDONED_UNCHARGED: &str = "a read was abandoned with no thread in charge";

/// The logic error of a read abandoned of a record that is not held.
const ABANDONED_UNHELD: &str = "a read was abandoned of a record no longer held";

/// The logic error of an in-charge read's return that is not running.
const RETURN_NOT_RUNNING: &str = "a read returned that its thread was not reading";

/// The logic error of a charge ended where none is.
const NO_CHARGE: &str = "a charge ended where none is";

/// Who drops the waiting events, for the log line.
#[derive(Clone, Copy)]
enum Dropping {
    Waited,
    AsPushed,
}

/// The read scheduler ([module docs](self)).
pub struct Schedule<J> {
    deadline: u64,
    /// The running applications, one per process id. An event looks here
    /// alone, so it cannot reach an exited record.
    running: BTreeMap<Pid, Running<J>>,
    /// The processes that have exited with readers still out.
    exited: BTreeMap<AppKey, Exited<J>>,
    /// The last application generation given.
    generations: u64,
    stopping: bool,
    /// How many events have been queued: an event's place is the count
    /// once it is queued.
    queued: u64,
    /// The idle waiters, each with the count of events queued when it came.
    idle_waiters: VecDeque<(u64, u64)>,
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
            running: BTreeMap::new(),
            exited: BTreeMap::new(),
            generations: 0,
            stopping: false,
            queued: 0,
            idle_waiters: VecDeque::new(),
            alarm_due: None,
        }
    }

    /// Whether the listener is shutting down.
    #[must_use]
    pub fn stopping(&self) -> bool {
        self.stopping
    }

    /// The markers of the events it holds whose outcome is still to be
    /// sent, in order: those waiting, and those being read.
    #[must_use]
    pub fn markers(&self) -> Vec<u64> {
        fn of_charge<J>(charge: &Charge<J>) -> impl Iterator<Item = u64> + '_ {
            charge
                .queue
                .iter()
                .map(|queued| queued.meta.marker)
                .chain(charge.flight.running().map(|(_, read)| read.meta.marker))
        }
        let mut markers: Vec<u64> = Vec::new();
        for running in self.running.values() {
            match running.life.reader() {
                Reader::InCharge(charge) => markers.extend(of_charge(charge)),
                Reader::Starved { queue, .. } => {
                    markers.extend(queue.iter().map(|queued| queued.meta.marker));
                }
                Reader::Idle { .. } => {}
            }
        }
        for exited in self.exited.values() {
            if let Some(charge) = exited.draining.charge() {
                markers.extend(of_charge(charge));
            }
        }
        markers.sort_unstable();
        markers
    }

    /// The records it holds: one record each in the shell, dropped at its
    /// [`ScheduleEffect::Forget`].
    pub fn held(&self) -> impl Iterator<Item = AppKey> + '_ {
        self.running
            .iter()
            .map(|(pid, running)| AppKey {
                pid: *pid,
                generation: running.generation,
            })
            .chain(self.exited.keys().copied())
    }

    /// The key of the application running under `pid`, if any.
    #[must_use]
    pub fn current(&self, pid: Pid) -> Option<AppKey> {
        self.running.get(&pid).map(|running| AppKey {
            pid,
            generation: running.generation,
        })
    }

    /// What the running application `pid` remembers `object` to be, for
    /// the listener's hook to drop a known non-bar with no call; `None`
    /// with nothing remembered, or no record.
    #[must_use]
    pub fn bar_verdict(&self, pid: Pid, object: BarObject) -> Option<BarVerdict> {
        let verdicts = &self.running.get(&pid)?.app.bar_verdicts;
        verdicts
            .iter()
            .find(|(known, _)| *known == object)
            .map(|&(_, verdict)| verdict)
    }

    /// The charge of `app`, when it has a thread in charge.
    fn charge_of(&self, app: AppKey) -> Option<&Charge<J>> {
        self.running
            .get(&app.pid)
            .filter(|running| running.generation == app.generation)
            .and_then(|running| running.life.reader().charge())
            .or_else(|| {
                self.exited
                    .get(&app)
                    .and_then(|exited| exited.draining.charge())
            })
    }

    /// Whether the thread `generation` is in charge of `app`'s events: one
    /// that is waits for its next event, one that is not ends.
    #[must_use]
    pub fn in_charge(&self, app: AppKey, generation: u64) -> bool {
        !self.stopping
            && self
                .charge_of(app)
                .is_some_and(|charge| charge.flight.generation() == generation)
    }

    /// Whether a focus event is waiting for its read or being read, for a
    /// menu opening of `pid`'s: a focus of an application whose abandoned
    /// threads are at the limit, waiting with no thread, counts for that
    /// application's own menu openings only.
    #[must_use]
    pub fn focus_pending(&self, pid: Option<Pid>) -> bool {
        let running = self
            .running
            .iter()
            .any(|(owner, running)| match running.life.reader() {
                Reader::InCharge(charge) => charge.holds_focus(),
                Reader::Starved { queue, .. } => {
                    Some(*owner) == pid
                        && queue.iter().any(|queued| queued.kind == FocusEvent::Focus)
                }
                Reader::Idle { .. } => false,
            });
        running
            || self
                .exited
                .values()
                .filter_map(|exited| exited.draining.charge())
                .any(Charge::holds_focus)
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
            ScheduleInput::Next { app, generation } => {
                self.next(&mut effects, now, app, generation);
            }
            ScheduleInput::Returned {
                app,
                request,
                meta,
                judged,
            } => self.returned(&mut effects, (app, request), meta, judged),
            ScheduleInput::Tick => {
                // The alarm went off, and its timer goes off once: set it
                // again below to whatever is still due, even a time that
                // has not changed, as a timer can go off a little early.
                self.alarm_due = None;
                self.tick(&mut effects, now);
            }
            ScheduleInput::Learned {
                app,
                request,
                window,
            } => {
                // Nothing is judged once the listener stops, as a tick
                // judges nothing then.
                if !self.stopping {
                    self.report(&mut effects, now, app, |flight| {
                        flight.learned(request, window, now)
                    });
                }
            }
            ScheduleInput::Placed { app, request, top } => {
                self.placed(&mut effects, app, request, top);
            }
            ScheduleInput::Judged {
                app,
                request,
                verdict,
            } => {
                self.report(&mut effects, now, app, |flight| {
                    flight.judged(request, verdict)
                });
            }
            ScheduleInput::Foreground { top_level } => self.foreground(top_level),
            ScheduleInput::MsaaFocus { pid } => self.moved_away_from_all_but(pid),
            ScheduleInput::BarLearned {
                pid,
                object,
                verdict,
            } => {
                if let Some(running) = self.running.get_mut(&pid) {
                    running.app.remember_bar(object, verdict);
                }
            }
            ScheduleInput::OutpostEnded { pid } => self.outpost_ended(&mut effects, pid),
            ScheduleInput::ApplicationExited { pid } => self.application_exited(&mut effects, pid),
            ScheduleInput::WhenIdle { waiter } => {
                if !self.stopping {
                    self.idle_waiters.push_back((self.queued, waiter));
                    self.wake_idle_waiters(&mut effects);
                }
            }
            ScheduleInput::SpawnFailed { app, generation } => {
                self.spawn_failed(&mut effects, app, generation);
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

    /// The record of `app`, in either map.
    fn slot(&mut self, app: AppKey) -> Option<Slot<'_, J>> {
        if let Some(running) = self
            .running
            .get_mut(&app.pid)
            .filter(|running| running.generation == app.generation)
        {
            let ending = running.life.ending();
            return Some(Slot::Running {
                reader: running.life.reader_mut(),
                app: &mut running.app,
                ending,
            });
        }
        self.exited.get_mut(&app).map(|exited| Slot::Exited {
            draining: &mut exited.draining,
            app: &mut exited.app,
        })
    }

    /// A read of `app` reported something `ask` gives its work in flight,
    /// and what that decided is done: nothing with no thread in charge,
    /// whose read has already gone.
    fn report(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        now: u64,
        app: AppKey,
        ask: impl FnOnce(&mut InFlight<UnderWay>) -> Decision<UnderWay>,
    ) {
        let Some(mut slot) = self.slot(app) else {
            effects.push(ScheduleEffect::LogicError(REPORT_UNHELD));
            return;
        };
        if let Some(charge) = slot.charge() {
            let decision = ask(&mut charge.flight);
            self.decided(effects, now, app, decision);
        }
    }

    /// Sets the watchdog's alarm to the soonest time anything is due, when
    /// that differs from the time it is set to (none once it went off):
    /// each read's next wake ([`InFlight::next_wake`]), and the end of each
    /// application's wait at the abandoned limit.
    fn set_alarm(&mut self, effects: &mut Vec<ScheduleEffect<J>>) {
        if self.stopping {
            return;
        }
        let deadline = self.deadline;
        let running = self
            .running
            .values()
            .filter_map(|running| match running.life.reader() {
                Reader::InCharge(charge) => charge.flight.next_wake(),
                Reader::Starved { since, queue } if !queue.is_empty() => {
                    Some(since.saturating_add(deadline))
                }
                Reader::Starved { .. } | Reader::Idle { .. } => None,
            });
        let exited = self
            .exited
            .values()
            .filter_map(|exited| exited.draining.charge())
            .filter_map(|charge| charge.flight.next_wake());
        let due = running.chain(exited).min();
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
        // NVDA's UIA limiter keeps one waiting event per element and kind:
        // a newer one erases the older, which is reported dropped.
        let mut replaced = false;
        // A progress bar's change never starts or holds an outpost: it is
        // read and the record then goes, whatever else the record holds.
        let bar = matches!(kind, FocusEvent::UiaBar | FocusEvent::ClassicBar);
        if let Some(running) = self.running.get_mut(&pid) {
            // An event while its outpost's end is asked: the application
            // runs on.
            if running.life.ending() && !bar {
                running.life.change(Life::Live);
            }
            let app = AppKey {
                pid,
                generation: running.generation,
            };
            if let Some(key) = &key
                && let Some(queue) = running.life.reader_mut().queue_mut()
            {
                let mut kept = VecDeque::with_capacity(queue.len());
                for waiting in queue.drain(..) {
                    if waiting.kind == kind && waiting.key.as_ref() == Some(key) {
                        effects.push(ScheduleEffect::Outcome {
                            pid: app.pid,
                            meta: waiting.meta,
                            judged: Judged::Dropped(REPLACED),
                        });
                        replaced = true;
                    } else {
                        kept.push_back(waiting);
                    }
                }
                *queue = kept;
            }
        }
        // The event replaced may be the last an idle waiter waited for.
        if replaced {
            self.wake_idle_waiters(effects);
        }
        self.queued = self.queued.saturating_add(1);
        let place = self.queued;
        // Another application's focus or menu opening moves away from
        // every other application's last read.
        if matches!(kind, FocusEvent::Focus | FocusEvent::MenuOpened) {
            self.moved_away_from_all_but(pid);
        }
        let queued = |left_at| Queued {
            place,
            left_at,
            kind,
            key,
            window,
            meta,
            job,
        };
        let Some(running) = self.running.get_mut(&pid) else {
            self.start_new_generation(effects, pid, queued(0), bar);
            return;
        };
        let app = AppKey {
            pid,
            generation: running.generation,
        };
        let queued = queued(running.app.left_at);
        match running.life.reader_mut() {
            Reader::Idle { abandoned } => {
                let (charge, generation) = Charge::begin(
                    &mut running.app.counters,
                    abandoned.get(),
                    VecDeque::from([queued]),
                );
                *running.life.reader_mut() = Reader::InCharge(charge);
                effects.push(ScheduleEffect::Spawn {
                    app,
                    generation,
                    reuse: false,
                });
            }
            Reader::InCharge(charge) => {
                charge.queue.push_back(queued);
                effects.push(ScheduleEffect::Wake(Waking::Thread(app)));
            }
            Reader::Starved { since, queue } => {
                queue.push_back(queued);
                if now >= since.saturating_add(self.deadline) {
                    // Past the application's bound: dropped at once, behind
                    // the events the watchdog has not yet dropped, in order.
                    self.drop_waiting(effects, pid, Dropping::AsPushed);
                } else {
                    // The alarm bounds its wait.
                    tracing::warn!(
                        %pid,
                        waiting = queue.len(),
                        "a UIA focus waits: its application's reads have not returned"
                    );
                }
            }
        }
    }

    /// The first event of a process no running record names: a new
    /// generation, with no abandoned readers, and a thread for it. A bar's
    /// change, `bar`, makes a record for a process that may have no outpost,
    /// which no supervisor exit would end, so the record is let go as soon
    /// as it has nothing left to do.
    fn start_new_generation(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        pid: Pid,
        queued: Queued<J>,
        bar: bool,
    ) {
        self.generations = self.generations.saturating_add(1);
        let generation = self.generations;
        let mut app = App::default();
        let (charge, in_charge) = Charge::begin(&mut app.counters, 0, VecDeque::from([queued]));
        let reader = Reader::InCharge(charge);
        self.running.insert(
            pid,
            Running {
                generation,
                life: if bar {
                    Life::Ending(reader)
                } else {
                    Life::Live(reader)
                },
                app,
            },
        );
        effects.push(ScheduleEffect::Spawn {
            app: AppKey { pid, generation },
            generation: in_charge,
            reuse: false,
        });
    }

    /// Marks every application but `pid` as moved away from at the place
    /// of the newest event queued.
    fn moved_away_from_all_but(&mut self, pid: Pid) {
        let place = self.queued;
        for (owner, running) in &mut self.running {
            if *owner != pid {
                running.app.left_at = place;
            }
        }
        for (key, exited) in &mut self.exited {
            if key.pid != pid {
                exited.app.left_at = place;
            }
        }
    }

    fn next(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        now: u64,
        app: AppKey,
        generation: u64,
    ) {
        if self.stopping {
            return;
        }
        let deadline = self.deadline;
        let Some(mut slot) = self.slot(app) else {
            return;
        };
        let Some(charge) = slot.charge_of(generation) else {
            return;
        };
        if let Some(queued) = charge.queue.pop_front() {
            let read = UnderWay {
                meta: queued.meta,
                kind: queued.kind,
                place: queued.place,
                top: None,
            };
            let request = charge.flight.start(read, deadline, queued.window, now);
            effects.push(ScheduleEffect::Job(Job {
                app,
                request,
                meta: queued.meta,
                work: queued.job,
                window: queued.window,
                queue: QueuePlace {
                    place: queued.place,
                    left_at: queued.left_at,
                },
            }));
            return;
        }
        if slot.lets_go() {
            self.charge_over(effects, app);
        }
    }

    /// The thread in charge of `app` is in charge no more and nothing
    /// waits: its record is dropped with no reader out, otherwise idle.
    fn charge_over(&mut self, effects: &mut Vec<ScheduleEffect<J>>, app: AppKey) {
        let Some(slot) = self.slot(app) else {
            effects.push(ScheduleEffect::LogicError(NO_CHARGE));
            return;
        };
        let abandoned = match slot {
            Slot::Running { reader, app, .. } => {
                let Reader::InCharge(charge) = &mut *reader else {
                    effects.push(ScheduleEffect::LogicError(NO_CHARGE));
                    return;
                };
                app.counters = std::mem::take(&mut charge.flight);
                let abandoned = NonZeroUsize::new(charge.abandoned);
                if let Some(abandoned) = abandoned {
                    *reader = Reader::Idle { abandoned };
                }
                abandoned
            }
            Slot::Exited { draining, app } => {
                let Draining::InCharge(charge) = &mut *draining else {
                    effects.push(ScheduleEffect::LogicError(NO_CHARGE));
                    return;
                };
                app.counters = std::mem::take(&mut charge.flight);
                let abandoned = NonZeroUsize::new(charge.abandoned);
                if let Some(abandoned) = abandoned {
                    *draining = Draining::Idle { abandoned };
                }
                abandoned
            }
        };
        if abandoned.is_none() {
            self.let_go(effects, app);
        }
    }

    fn returned(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        (app, request): (AppKey, Request),
        meta: Meta,
        judged: Judged,
    ) {
        let stopping = self.stopping;
        let Some(mut slot) = self.slot(app) else {
            effects.push(ScheduleEffect::LogicError(RETURN_UNHELD));
            return;
        };
        let exited = matches!(slot, Slot::Exited { .. });
        if let Some(charge) = slot.charge_of(request.generation) {
            let read = matches!(judged, Judged::Read(_));
            let Some(under_way) = charge.flight.end(request) else {
                tracing::error!(%app.pid, "a read returned that its thread was not reading");
                effects.push(ScheduleEffect::LogicError(RETURN_NOT_RUNNING));
                return;
            };
            if read && under_way.top.is_some() {
                slot.app().last_top = under_way.top;
            }
            // A new process may have the id once the exit has been
            // answered, and what this one read must not reach it.
            let judged = if exited && !matches!(judged, Judged::Dropped(_)) {
                Judged::Dropped(PROCESS_EXITED)
            } else {
                judged
            };
            effects.push(ScheduleEffect::Outcome {
                pid: app.pid,
                meta,
                judged,
            });
            self.wake_idle_waiters(effects);
            return;
        }
        // Abandoned while it read: its event was dropped, and its read is
        // a focus Core never got.
        tracing::info!(%app.pid, "an abandoned UIA focus read returned; its result is discarded");
        effects.push(ScheduleEffect::ForgetLast);
        let dropped = match slot {
            Slot::Running {
                reader,
                app: record,
                ..
            } => match reader {
                Reader::Idle { abandoned } => one_fewer(abandoned),
                Reader::InCharge(charge) => {
                    lower(charge, effects);
                    false
                }
                Reader::Starved { queue, .. } if queue.is_empty() => {
                    *reader = Reader::Idle {
                        abandoned: BELOW_LIMIT,
                    };
                    false
                }
                Reader::Starved { queue, .. } if stopping => {
                    // What waits is not read once the listener stops.
                    let waiting = std::mem::take(queue);
                    *reader = Reader::Idle {
                        abandoned: BELOW_LIMIT,
                    };
                    for queued in waiting {
                        effects.push(ScheduleEffect::Outcome {
                            pid: app.pid,
                            meta: queued.meta,
                            judged: Judged::Dropped(SHUTTING_DOWN),
                        });
                    }
                    false
                }
                Reader::Starved { queue, .. } => {
                    let waiting = std::mem::take(queue);
                    let (charge, generation) =
                        Charge::begin(&mut record.counters, BELOW_LIMIT.get(), waiting);
                    *reader = Reader::InCharge(charge);
                    effects.push(ScheduleEffect::Spawn {
                        app,
                        generation,
                        reuse: true,
                    });
                    false
                }
            },
            Slot::Exited { draining, .. } => match draining {
                Draining::Idle { abandoned } => one_fewer(abandoned),
                Draining::InCharge(charge) => {
                    lower(charge, effects);
                    false
                }
            },
        };
        if dropped {
            self.let_go(effects, app);
        }
    }

    /// Runs each waiter for which every event queued before it has its
    /// outcome sent.
    fn wake_idle_waiters(&mut self, effects: &mut Vec<ScheduleEffect<J>>) {
        if self.idle_waiters.is_empty() {
            return;
        }
        let running = self
            .running
            .values()
            .filter_map(|running| match running.life.reader() {
                Reader::InCharge(charge) => charge.oldest(),
                Reader::Starved { queue, .. } => queue.front().map(|queued| queued.place),
                Reader::Idle { .. } => None,
            });
        let exited = self
            .exited
            .values()
            .filter_map(|exited| exited.draining.charge())
            .filter_map(Charge::oldest);
        let oldest = running.chain(exited).min();
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
        let deadline = self.deadline;
        // Oldest generation of an id first, so what one tick drops is sent
        // in the order the events came.
        let mut keys: Vec<AppKey> = self.held().collect();
        keys.sort_unstable();
        for app in keys {
            let Some(mut slot) = self.slot(app) else {
                continue;
            };
            if let Some(charge) = slot.charge() {
                if charge.flight.running().is_some() {
                    let decision = charge.flight.tick(now);
                    self.decided(effects, now, app, decision);
                }
            } else if matches!(&slot, Slot::Running { reader: Reader::Starved { since, queue }, .. }
                if !queue.is_empty() && now >= since.saturating_add(deadline))
            {
                self.drop_waiting(effects, app.pid, Dropping::Waited);
            }
        }
    }

    /// Does what `app`'s work in flight decided: asks the shell, or
    /// carries out an abandonment.
    fn decided(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        now: u64,
        app: AppKey,
        decision: Decision<UnderWay>,
    ) {
        match decision {
            Decision::Nothing => {}
            Decision::Ask(Ask::Judge {
                request,
                window: Some(window),
            }) => {
                effects.push(ScheduleEffect::Judge {
                    app,
                    request,
                    window,
                });
            }
            // A read's window is known or learned, never named by its
            // thread, so it is never judged without one.
            Decision::Ask(Ask::Judge { window: None, .. }) => {
                effects.push(ScheduleEffect::LogicError("a read judged with no window"));
            }
            Decision::Abandon(abandoned) => self.abandon(effects, now, app, abandoned),
        }
    }

    /// `app`'s read was abandoned: its thread is in charge no more, its
    /// event is dropped, and a new thread takes the events waiting, unless
    /// the application's abandoned threads are at the limit.
    fn abandon(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        now: u64,
        app: AppKey,
        abandoned: Abandoned<UnderWay>,
    ) {
        let Abandoned {
            reason,
            item: under_way,
            ..
        } = abandoned;
        effects.push(ScheduleEffect::Unwatch { app });
        let Some(mut slot) = self.slot(app) else {
            effects.push(ScheduleEffect::LogicError(ABANDONED_UNHELD));
            return;
        };
        let Some(charge) = slot.charge() else {
            effects.push(ScheduleEffect::LogicError(ABANDONED_UNCHARGED));
            return;
        };
        let count = NonZeroUsize::MIN.saturating_add(charge.abandoned);
        let queue = std::mem::take(&mut charge.queue);
        let mut flight = std::mem::take(&mut charge.flight);
        effects.push(ScheduleEffect::Outcome {
            pid: app.pid,
            meta: under_way.meta,
            judged: Judged::Dropped(abandonment(app, count.get(), reason)),
        });
        // The one place the count rises: it reaches the limit only here.
        if count.get() >= ABANDONED_LIMIT {
            let waiting = !queue.is_empty();
            match slot {
                Slot::Running {
                    reader,
                    app: record,
                    ..
                } => {
                    record.counters = flight;
                    *reader = Reader::Starved { since: now, queue };
                    if waiting {
                        tracing::warn!(
                            %app.pid,
                            "UIA focuses wait: its application's reads have not returned"
                        );
                    }
                }
                Slot::Exited {
                    draining,
                    app: record,
                } => {
                    record.counters = flight;
                    *draining = Draining::Idle {
                        abandoned: AT_LIMIT,
                    };
                    for queued in queue {
                        effects.push(ScheduleEffect::Outcome {
                            pid: app.pid,
                            meta: queued.meta,
                            judged: Judged::Dropped(WAITED_TOO_LONG),
                        });
                    }
                }
            }
        } else if queue.is_empty() {
            let abandoned = count;
            match slot {
                Slot::Running {
                    reader,
                    app: record,
                    ..
                } => {
                    record.counters = flight;
                    *reader = Reader::Idle { abandoned };
                }
                Slot::Exited {
                    draining,
                    app: record,
                } => {
                    record.counters = flight;
                    *draining = Draining::Idle { abandoned };
                }
            }
        } else {
            // A new thread takes what waits.
            let generation = flight.succeed();
            let charge = Charge {
                abandoned: count.get(),
                flight,
                queue,
            };
            match slot {
                Slot::Running { reader, .. } => *reader = Reader::InCharge(charge),
                Slot::Exited { draining, .. } => *draining = Draining::InCharge(charge),
            }
            effects.push(ScheduleEffect::Spawn {
                app,
                generation,
                reuse: false,
            });
        }
        self.wake_idle_waiters(effects);
    }

    /// Drops every event of the application running under `pid` waiting
    /// with no thread to read it, in the order they arrived.
    fn drop_waiting(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid, dropping: Dropping) {
        let Some(running) = self.running.get_mut(&pid) else {
            return;
        };
        let app = AppKey {
            pid,
            generation: running.generation,
        };
        let Reader::Starved { queue, .. } = running.life.reader_mut() else {
            return;
        };
        if queue.is_empty() {
            return;
        }
        let dropped = queue.len();
        match dropping {
            Dropping::Waited => tracing::warn!(
                %pid,
                abandoned = ABANDONED_LIMIT,
                dropped,
                "every waiting UIA event of the application, of whatever kind, waited past its bound behind its abandoned reads; they are dropped"
            ),
            Dropping::AsPushed => tracing::warn!(
                %pid,
                abandoned = ABANDONED_LIMIT,
                dropped,
                "every waiting UIA event of the application, of whatever kind, is dropped as one arrives: it is past its bound behind its abandoned reads"
            ),
        }
        for queued in queue.drain(..) {
            effects.push(ScheduleEffect::Outcome {
                pid: app.pid,
                meta: queued.meta,
                judged: Judged::Dropped(WAITED_TOO_LONG),
            });
        }
        self.wake_idle_waiters(effects);
    }

    /// `app`'s read `request` has found its top-level window, `top`.
    fn placed(
        &mut self,
        effects: &mut Vec<ScheduleEffect<J>>,
        app: AppKey,
        request: Request,
        top: Window,
    ) {
        let Some(mut slot) = self.slot(app) else {
            effects.push(ScheduleEffect::LogicError(REPORT_UNHELD));
            return;
        };
        if let Some(charge) = slot.charge()
            && charge.flight.accepts(request)
            && let Some(read) = charge.flight.item_mut()
        {
            read.top = Some(top);
        }
    }

    /// A foreground event of top-level window `top_level`: for menu
    /// currentness, it moves away from each application whose read under
    /// way, once it knows its window, or else whose last read, is of
    /// another top-level window; a read under way whose window is unknown
    /// counts as moving away. It judges nothing else.
    fn foreground(&mut self, top_level: Option<Window>) {
        fn mark<J>(
            charge: Option<&Charge<J>>,
            app: &mut App,
            place: u64,
            top_level: Option<Window>,
        ) {
            let reference = match charge.and_then(|charge| charge.flight.running()) {
                Some((_, read)) => read.top,
                None => app.last_top,
            };
            if reference.is_none() || reference != top_level {
                app.left_at = place;
            }
        }
        let place = self.queued;
        for running in self.running.values_mut() {
            mark(
                running.life.reader().charge(),
                &mut running.app,
                place,
                top_level,
            );
        }
        for exited in self.exited.values_mut() {
            mark(exited.draining.charge(), &mut exited.app, place, top_level);
        }
    }

    /// Lets `app`'s record go, and has the shell forget what it keeps for
    /// it ([`ScheduleEffect::Forget`]): the one place a record is removed.
    fn let_go(&mut self, effects: &mut Vec<ScheduleEffect<J>>, app: AppKey) {
        if self
            .running
            .get(&app.pid)
            .is_some_and(|running| running.generation == app.generation)
        {
            self.running.remove(&app.pid);
        } else {
            self.exited.remove(&app);
        }
        effects.push(ScheduleEffect::Forget { app });
    }

    fn outpost_ended(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid) {
        let Some(running) = self.running.get_mut(&pid) else {
            return;
        };
        if matches!(running.life, Life::Ending(_)) {
            return;
        }
        running.life.change(Life::Ending);
        if matches!(running.life.reader(), Reader::InCharge(_)) {
            effects.push(ScheduleEffect::Wake(Waking::Thread(AppKey {
                pid,
                generation: running.generation,
            })));
        }
    }

    /// The process running under `pid` has exited: its record moves out of
    /// the running applications, its waiting events dropped when no thread
    /// is in charge of them, and is kept for its readers alone.
    fn application_exited(&mut self, effects: &mut Vec<ScheduleEffect<J>>, pid: Pid) {
        let Some(running) = self.running.remove(&pid) else {
            return;
        };
        let app = AppKey {
            pid,
            generation: running.generation,
        };
        let ending = running.life.ending();
        let reader = match running.life {
            Life::Live(reader) | Life::Ending(reader) => reader,
        };
        let draining = match reader {
            Reader::Idle { abandoned } => Draining::Idle { abandoned },
            Reader::InCharge(charge) => {
                if !ending {
                    effects.push(ScheduleEffect::Wake(Waking::Thread(app)));
                }
                Draining::InCharge(charge)
            }
            Reader::Starved { queue, .. } => {
                for queued in queue {
                    effects.push(ScheduleEffect::Outcome {
                        pid: app.pid,
                        meta: queued.meta,
                        judged: Judged::Dropped(WAITED_TOO_LONG),
                    });
                }
                Draining::Idle {
                    abandoned: AT_LIMIT,
                }
            }
        };
        self.exited.insert(
            app,
            Exited {
                draining,
                app: running.app,
            },
        );
        self.wake_idle_waiters(effects);
    }

    fn spawn_failed(&mut self, effects: &mut Vec<ScheduleEffect<J>>, app: AppKey, generation: u64) {
        let Some(mut slot) = self.slot(app) else {
            return;
        };
        let Some(charge) = slot.charge_of(generation) else {
            return;
        };
        tracing::error!(%app.pid, "no thread to read UIA focus events; they are dropped");
        for queued in charge.queue.drain(..) {
            effects.push(ScheduleEffect::Outcome {
                pid: app.pid,
                meta: queued.meta,
                judged: Judged::Dropped(NO_THREAD),
            });
        }
        self.charge_over(effects, app);
        self.wake_idle_waiters(effects);
    }
}

/// Logs that `app`'s read was abandoned for `reason`, `count` now out, and
/// answers why its event is dropped.
fn abandonment(app: AppKey, count: usize, reason: Reason) -> &'static str {
    match reason {
        Reason::MovedOn => {
            tracing::warn!(
                %app.pid,
                abandoned = count,
                "a UIA focus read was slow and the user moved on to another thread of its application; its thread is abandoned"
            );
            MOVED_ON
        }
        Reason::Deadline => {
            tracing::warn!(%app.pid, abandoned = count, "a UIA focus read did not return in time; its thread is abandoned");
            NOT_IN_TIME
        }
    }
}

/// One abandoned reader fewer, its count kept at least one: whether that
/// was the last, the record then being dropped.
fn one_fewer(abandoned: &mut NonZeroUsize) -> bool {
    match NonZeroUsize::new(abandoned.get() - 1) {
        Some(less) => {
            *abandoned = less;
            false
        }
        None => true,
    }
}

/// An abandoned read of `charge`'s application returned.
fn lower<J>(charge: &mut Charge<J>, effects: &mut Vec<ScheduleEffect<J>>) {
    match charge.abandoned.checked_sub(1) {
        Some(less) => charge.abandoned = less,
        None => effects.push(ScheduleEffect::LogicError(NONE_ABANDONED)),
    }
}

/// What waits on the listener's one timer (`listener`'s `MenuEnd`): a menu
/// end's grace, the user's desktop's return, 200 ms after its switch, as
/// NVDA calls `_handleUserDesktop` later, and the virtual desktop's name's
/// due time, 250 ms after its event, as NVDA calls
/// `handlePossibleDesktopNameChange` later
/// (`books/dev/src/reference/adr/0001-focus.md`, "The desktop's return and name"). Each is sent
/// when its time is over,
/// soonest first, and the idle barrier's answers wait for all of them.
#[derive(Clone, Debug, Default)]
pub struct GraceState {
    /// The end: when it ended, its stamps, and when its grace is over, in
    /// microseconds on the shell's monotonic clock.
    end: Option<(u64, EventTiming, u64)>,
    /// The desktop's return, with its stamps and when it is due, a newer
    /// switch's replacing it.
    returned: Option<(EventTiming, u64)>,
    /// The desktop name's due time, with its stamps and when it is due, a
    /// newer name's replacing it.
    name_due: Option<(EventTiming, u64)>,
    /// The idle barrier's rounds answered once nothing waits.
    answers: Vec<u64>,
    /// Whether the timer is being closed, so it is set no more.
    closed: bool,
}

/// One input to the timer's state.
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
    /// The user's desktop returned, to be told `after` microseconds from
    /// `now`.
    Returned {
        /// The switch's timing, for the latency log.
        timing: EventTiming,
        /// How long after now it is told, in microseconds.
        after: u64,
    },
    /// The virtual desktop's name was sent; its due time is `after`
    /// microseconds from `now`, or now when that is past.
    NameSent {
        /// The name change's timing, for the latency log.
        timing: EventTiming,
        /// How long after now it is due, in microseconds.
        after: u64,
    },
    /// The timer went off.
    Fired,
    /// The idle barrier's round `round` is to be answered.
    IdleAnswer(u64),
    /// The timer is being closed.
    Close,
}

/// One effect of the timer's step.
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
    /// Send that the user's desktop returned.
    SendReturned(EventTiming),
    /// Send the desktop name's due time.
    SendNameDue(EventTiming),
    /// Send the idle barrier's answer for the round.
    SendIdleAnswer(u64),
    /// Set the timer to go off this many microseconds from now.
    SetTimer(u64),
}

impl GraceState {
    /// Takes one input at `now`, microseconds on the shell's monotonic
    /// clock: a callback that runs before anything waiting is due, one of a
    /// time an earlier input set, or a timer that fired a little early,
    /// sends nothing and sets the timer again for the soonest still to
    /// come.
    pub fn step(&mut self, input: GraceInput, now: u64) -> Vec<GraceEffect> {
        match input {
            GraceInput::Ended {
                ended_at_us,
                timing,
                grace,
            } => {
                self.end = Some((ended_at_us, timing, now.saturating_add(grace)));
                self.timer(now).into_iter().collect()
            }
            GraceInput::Returned { timing, after } => {
                self.returned = Some((timing, now.saturating_add(after)));
                self.timer(now).into_iter().collect()
            }
            GraceInput::NameSent { timing, after } => {
                self.name_due = Some((timing, now.saturating_add(after)));
                self.timer(now).into_iter().collect()
            }
            GraceInput::Fired => {
                let mut due: Vec<(u64, GraceEffect)> = Vec::new();
                if let Some((ended_at_us, timing, over)) = self.end
                    && over <= now
                {
                    self.end = None;
                    due.push((
                        over,
                        GraceEffect::SendEnd {
                            ended_at_us,
                            timing,
                        },
                    ));
                }
                if let Some((timing, over)) = self.returned
                    && over <= now
                {
                    self.returned = None;
                    due.push((over, GraceEffect::SendReturned(timing)));
                }
                if let Some((timing, over)) = self.name_due
                    && over <= now
                {
                    self.name_due = None;
                    due.push((over, GraceEffect::SendNameDue(timing)));
                }
                due.sort_by_key(|(over, _)| *over);
                let mut effects: Vec<GraceEffect> =
                    due.into_iter().map(|(_, effect)| effect).collect();
                match self.timer(now) {
                    Some(timer) => effects.push(timer),
                    None if self.waits() => {}
                    None => {
                        effects.extend(self.answers.drain(..).map(GraceEffect::SendIdleAnswer));
                    }
                }
                effects
            }
            GraceInput::IdleAnswer(round) => {
                if self.waits() {
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

    /// Whether anything waits for its time.
    fn waits(&self) -> bool {
        self.end.is_some() || self.returned.is_some() || self.name_due.is_some()
    }

    /// The timer set for the soonest time still to come, unless the timer
    /// is being closed or nothing waits.
    fn timer(&self, now: u64) -> Option<GraceEffect> {
        if self.closed {
            return None;
        }
        [
            self.end.map(|(_, _, over)| over),
            self.returned.map(|(_, over)| over),
            self.name_due.map(|(_, over)| over),
        ]
        .into_iter()
        .flatten()
        .min()
        .map(|over| GraceEffect::SetTimer(over.saturating_sub(now)))
    }
}

#[cfg(test)]
mod explore;
#[cfg(test)]
mod generated;

#[cfg(test)]
mod tests {
    use verbatim_model::TraceId;
    use verbatim_model::protocol::UiaFocusRead;

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

    /// `pid`'s application, by process id: [`step`] fills in its current
    /// generation.
    fn key(pid: u32) -> AppKey {
        AppKey {
            pid: Pid(pid),
            generation: 0,
        }
    }

    /// The input with its application's generation filled in.
    fn resolved(schedule: &Schedule<u64>, input: ScheduleInput<u64>) -> ScheduleInput<u64> {
        let current = |app: AppKey| schedule.current(app.pid).unwrap_or(app);
        match input {
            ScheduleInput::Next { app, generation } => ScheduleInput::Next {
                app: current(app),
                generation,
            },
            ScheduleInput::Returned {
                app,
                request,
                meta,
                judged,
            } => ScheduleInput::Returned {
                app: current(app),
                request,
                meta,
                judged,
            },
            ScheduleInput::Learned {
                app,
                request,
                window,
            } => ScheduleInput::Learned {
                app: current(app),
                request,
                window,
            },
            ScheduleInput::Placed { app, request, top } => ScheduleInput::Placed {
                app: current(app),
                request,
                top,
            },
            ScheduleInput::Judged {
                app,
                request,
                verdict,
            } => ScheduleInput::Judged {
                app: current(app),
                request,
                verdict,
            },
            ScheduleInput::SpawnFailed { app, generation } => ScheduleInput::SpawnFailed {
                app: current(app),
                generation,
            },
            other => other,
        }
    }

    /// Whether the thread `generation` is in charge of `pid`'s events.
    fn in_charge(schedule: &Schedule<u64>, pid: u32, generation: u64) -> bool {
        schedule
            .current(Pid(pid))
            .is_some_and(|app| schedule.in_charge(app, generation))
    }

    fn next(pid: u32, generation: u64) -> ScheduleInput<u64> {
        ScheduleInput::Next {
            app: key(pid),
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
        Woken(u32),
        WokenAll,
        Unwatch(u32),
        ForgetLast,
        Forget(u32),
        Alarm(AlarmChange),
        RunIdle(u64),
        Job(u32, u64, QueuePlace),
        Judge(u32, Request),
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
                    app,
                    generation,
                    reuse,
                } => Seen::Spawn(app.pid.0, generation, reuse),
                ScheduleEffect::Wake(Waking::Thread(app)) => Seen::Woken(app.pid.0),
                ScheduleEffect::Wake(Waking::All) => Seen::WokenAll,
                ScheduleEffect::Unwatch { app } => Seen::Unwatch(app.pid.0),
                ScheduleEffect::ForgetLast => Seen::ForgetLast,
                ScheduleEffect::Forget { app } => Seen::Forget(app.pid.0),
                ScheduleEffect::Alarm(change) => Seen::Alarm(change),
                ScheduleEffect::RunIdle(waiter) => Seen::RunIdle(waiter),
                ScheduleEffect::Job(job) => Seen::Job(job.app.pid.0, job.meta.marker, job.queue),
                ScheduleEffect::Judge {
                    app,
                    request,
                    window,
                } => {
                    assert_eq!(window, WINDOW);
                    Seen::Judge(app.pid.0, request)
                }
                ScheduleEffect::LogicError(error) => panic!("a logic error: {error}"),
            })
            .collect()
    }

    fn step(schedule: &mut Schedule<u64>, input: ScheduleInput<u64>, now: u64) -> Vec<Seen> {
        let input = resolved(schedule, input);
        seen(schedule.step(input, now))
    }

    fn set(due: u64) -> Seen {
        Seen::Alarm(AlarmChange::Set(Some(due)))
    }

    const CANCEL: Seen = Seen::Alarm(AlarmChange::Set(None));

    fn returned(pid: u32, request: Request, marker: u64) -> ScheduleInput<u64> {
        ScheduleInput::Returned {
            app: key(pid),
            request,
            meta: meta(marker, FocusEvent::Focus),
            judged: Judged::Dropped("read"),
        }
    }

    fn placed(pid: u32, request: Request, top: Window) -> ScheduleInput<u64> {
        ScheduleInput::Placed {
            app: key(pid),
            request,
            top,
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
        assert_eq!(step(schedule, focus(1, 2), 0), [Seen::Woken(1)]);
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
            [Seen::ForgetLast, Seen::Forget(1)],
            "nothing is left of the record"
        );
        assert!(!in_charge(&schedule, 1, 1), "the thread ends");
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
                },
                2,
            ),
            [],
            "a foreground event judges nothing and wakes nothing"
        );
        let asked = request(1, 1);
        assert_eq!(
            step(&mut schedule, ScheduleInput::Tick, GRACE),
            [Seen::Judge(1, asked), set(2 * GRACE)]
        );
        let judged = |verdict| ScheduleInput::Judged {
            app: key(1),
            request: asked,
            verdict,
        };
        assert_eq!(step(&mut schedule, judged(None), GRACE), []);
        assert_eq!(
            step(&mut schedule, ScheduleInput::Tick, 2 * GRACE),
            [Seen::Judge(1, asked), set(3 * GRACE)]
        );
        assert_eq!(
            step(&mut schedule, judged(Some(Reason::MovedOn)), 2 * GRACE),
            [
                Seen::Unwatch(1),
                Seen::Outcome(1, 1, MOVED_ON),
                Seen::Spawn(1, 3, false),
                CANCEL,
            ]
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
            let _ = step(&mut schedule, placed(1, request(1, 1), WINDOW), 1);
            let _ = step(
                &mut schedule,
                ScheduleInput::Returned {
                    app: key(1),
                    request: request(1, 1),
                    meta: meta(1, FocusEvent::Focus),
                    judged: Judged::Read(a_read()),
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
                    app: key(1),
                    request: request(1, 2),
                    window: WINDOW,
                },
                DEADLINE,
            ),
            []
        );
    }

    /// Found by the read scheduler's generated test
    /// (`books/dev/src/reference/crates/verbatim-listener-logic.md`): an idle
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
                Seen::Woken(1),
            ]
        );
    }

    /// Found by the read scheduler's generated test
    /// (`books/dev/src/reference/crates/verbatim-listener-logic.md`): an
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
            [],
            "held while its abandoned thread is out"
        );
        assert_eq!(
            step(&mut schedule, returned(1, request(1, 1), 1), DEADLINE + 1),
            [Seen::ForgetLast, Seen::Forget(1)]
        );
        assert!(schedule.held().next().is_none(), "let go");
    }

    /// A thread started for an application's events, before any outpost's
    /// hook was in, stays in charge while nothing says the application has
    /// no outpost, and ends at the release the supervisor sends from every
    /// stage of a generation and when it declines to watch the application.
    #[test]
    fn a_thread_waits_for_its_applications_release_and_then_ends() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        let _ = step(&mut schedule, returned(1, request(1, 1), 1), 1);
        assert_eq!(step(&mut schedule, next(1, 1), 1), []);
        assert!(in_charge(&schedule, 1, 1), "waits with nothing queued");
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::OutpostEnded { pid: Pid(1) },
                2
            ),
            [Seen::Woken(1)]
        );
        assert_eq!(step(&mut schedule, next(1, 1), 2), [Seen::Forget(1)]);
        assert!(!in_charge(&schedule, 1, 1), "the thread ends");
        assert!(schedule.held().next().is_none(), "let go");
    }

    /// A bar's change in a process no record names makes a record that is
    /// let go as soon as it has nothing left, since no supervisor exit ends
    /// a process with no outpost; a focus of it keeps the record alive.
    #[test]
    fn a_bars_record_is_let_go_when_idle_and_a_focus_keeps_it() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, push(1, 1, FocusEvent::ClassicBar), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        let _ = step(&mut schedule, returned(1, request(1, 1), 1), 1);
        assert_eq!(step(&mut schedule, next(1, 1), 1), [Seen::Forget(1)]);
        assert!(schedule.held().next().is_none(), "let go when idle");

        let _ = step(&mut schedule, push(1, 2, FocusEvent::UiaBar), 2);
        let _ = step(&mut schedule, focus(1, 3), 2);
        let _ = step(&mut schedule, next(1, 1), 2);
        let _ = step(&mut schedule, returned(1, request(1, 1), 2), 3);
        assert_eq!(
            step(&mut schedule, next(1, 1), 3).len(),
            2,
            "the focus's job"
        );
        assert!(in_charge(&schedule, 1, 1), "kept by the focus");
    }

    /// A bar's change never moves an application's record from its end
    /// back to running.
    #[test]
    fn a_bars_change_does_not_revive_an_ending_application() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        let _ = step(&mut schedule, returned(1, request(1, 1), 1), 1);
        let _ = step(
            &mut schedule,
            ScheduleInput::OutpostEnded { pid: Pid(1) },
            2,
        );
        let _ = step(&mut schedule, push(1, 2, FocusEvent::ClassicBar), 2);
        let _ = step(&mut schedule, next(1, 1), 2);
        let _ = step(&mut schedule, returned(1, request(1, 2), 2), 3);
        assert_eq!(step(&mut schedule, next(1, 1), 3), [Seen::Forget(1)]);
    }

    /// What a read found an object to be stays with the running record,
    /// answered by the hook with no call, the oldest forgotten past the cap.
    #[test]
    fn a_records_bar_verdicts_are_kept_to_the_cap() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let object = |child| BarObject {
            window: 0x10,
            object: -4,
            child,
        };
        let learn = |schedule: &mut Schedule<u64>, child, verdict| {
            let _ = step(
                schedule,
                ScheduleInput::BarLearned {
                    pid: Pid(1),
                    object: object(child),
                    verdict,
                },
                1,
            );
        };
        learn(&mut schedule, 0, BarVerdict::NotABar);
        learn(&mut schedule, 1, BarVerdict::Bar);
        assert_eq!(
            schedule.bar_verdict(Pid(1), object(0)),
            Some(BarVerdict::NotABar)
        );
        assert_eq!(
            schedule.bar_verdict(Pid(1), object(1)),
            Some(BarVerdict::Bar)
        );
        assert_eq!(schedule.bar_verdict(Pid(2), object(0)), None, "no record");
        for child in 2..=i32::try_from(BAR_VERDICTS_KEPT).expect("small") {
            learn(&mut schedule, child, BarVerdict::NotABar);
        }
        assert_eq!(
            schedule.bar_verdict(Pid(1), object(0)),
            None,
            "the oldest went"
        );
        assert_eq!(
            schedule.bar_verdict(Pid(1), object(1)),
            Some(BarVerdict::Bar)
        );
    }

    /// A bar's read abandoned at the deadline counts in its own
    /// application's record, so a stuck window holds up only itself, and
    /// the record goes once the abandoned read returns.
    #[test]
    fn an_abandoned_bar_read_counts_in_its_record_and_is_let_go() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, push(1, 1, FocusEvent::ClassicBar), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        assert_eq!(
            step(&mut schedule, ScheduleInput::Tick, DEADLINE),
            [Seen::Unwatch(1), Seen::Outcome(1, 1, NOT_IN_TIME)]
        );
        assert_eq!(
            step(&mut schedule, returned(1, request(1, 1), 1), DEADLINE + 1),
            [Seen::ForgetLast, Seen::Forget(1)]
        );
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
            [Seen::Woken(1)]
        );
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::SpawnFailed {
                    app: key(1),
                    generation: 1
                },
                0
            ),
            [Seen::Outcome(1, 1, NO_THREAD), Seen::Forget(1)]
        );
        assert!(schedule.held().next().is_none(), "let go");
    }

    /// A focus is pending while one is read, but a menu opening's read is
    /// not one.
    #[test]
    fn a_focus_is_pending_while_a_focus_is_read_and_not_while_a_menu_opening_is() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, push(1, 1, FocusEvent::MenuOpened), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        assert!(!schedule.focus_pending(None), "a menu opening read");
        let _ = step(&mut schedule, returned(1, request(1, 1), 1), 0);
        let _ = step(&mut schedule, focus(1, 2), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        assert!(schedule.focus_pending(None), "a focus read");
    }

    /// A focus waiting behind a menu opening's read, in an application
    /// with a thread, is pending for every application's menu openings.
    #[test]
    fn a_focus_queued_with_a_thread_is_pending_for_every_application() {
        let mut schedule = schedule();
        assert_eq!(
            step(&mut schedule, push(1, 1, FocusEvent::MenuOpened), 0),
            [Seen::Spawn(1, 1, false)]
        );
        let _ = step(&mut schedule, next(1, 1), 0);
        assert_eq!(step(&mut schedule, focus(1, 2), 0), [Seen::Woken(1)]);
        assert!(schedule.focus_pending(None));
        assert!(schedule.focus_pending(Some(Pid(2))));
    }

    /// What a generation told exited read is dropped: a new process may
    /// have its id.
    #[test]
    fn a_read_returning_after_its_process_exited_is_dropped() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        let app = schedule.current(Pid(1)).expect("running");
        let _ = step(
            &mut schedule,
            ScheduleInput::ApplicationExited { pid: Pid(1) },
            1,
        );
        assert_eq!(
            step(
                &mut schedule,
                ScheduleInput::Returned {
                    app,
                    request: request(1, 1),
                    meta: meta(1, FocusEvent::Focus),
                    judged: Judged::Read(a_read()),
                },
                2,
            ),
            [Seen::Outcome(1, 1, PROCESS_EXITED), CANCEL]
        );
    }

    /// Only a read focus gives its application its last top-level window:
    /// after a dropped read, a foreground event of the window it named
    /// still moves away from the application.
    #[test]
    fn a_dropped_reads_window_is_not_its_applications_last() {
        let mut schedule = schedule();
        let _ = step(&mut schedule, focus(1, 1), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        let _ = step(&mut schedule, placed(1, request(1, 1), WINDOW), 1);
        let _ = step(
            &mut schedule,
            ScheduleInput::Returned {
                app: key(1),
                request: request(1, 1),
                meta: meta(1, FocusEvent::Focus),
                judged: Judged::Dropped("its element could not be resolved"),
            },
            1,
        );
        assert_eq!(schedule.running[&Pid(1)].app.last_top, None);
        let _ = schedule.step(
            ScheduleInput::Foreground {
                top_level: Some(WINDOW),
            },
            1,
        );
        assert_eq!(schedule.running[&Pid(1)].app.left_at, 1, "moved away from");
    }

    /// An abandoned thread's return below the limit ends the wait at it.
    #[test]
    fn a_return_below_the_abandoned_limit_ends_the_limits_wait() {
        let mut schedule = schedule();
        at_the_limit(&mut schedule);
        assert!(
            matches!(
                schedule.running[&Pid(1)].life.reader(),
                Reader::Starved { since, .. } if *since == 2 * DEADLINE
            ),
            "the limit reached at the second abandonment"
        );
        let _ = step(
            &mut schedule,
            returned(1, request(1, 1), 1),
            2 * DEADLINE + 1,
        );
        assert!(matches!(
            schedule.running[&Pid(1)].life.reader(),
            Reader::InCharge(_)
        ));
    }

    /// A foreground event judges each application with a read under way by
    /// that application's own read's top-level window.
    #[test]
    fn a_foreground_event_judges_each_read_by_its_own_window() {
        let other = Window::new(0x20).expect("not 0");
        let mut schedule = schedule();
        // Application 1's focus moves away from 2 at place 2; its
        // selection, queued last at place 3, moves away from nothing, so
        // only the foreground event moves away from 2 at place 3.
        let _ = step(&mut schedule, focus(2, 1), 0);
        let _ = step(&mut schedule, next(2, 1), 0);
        let _ = step(&mut schedule, focus(1, 2), 0);
        let _ = step(&mut schedule, next(1, 1), 0);
        let _ = step(&mut schedule, push(1, 3, FocusEvent::Selection), 0);
        assert_eq!(schedule.running[&Pid(2)].app.left_at, 2);
        let _ = step(&mut schedule, placed(1, request(1, 1), WINDOW), 1);
        let _ = step(&mut schedule, placed(2, request(1, 1), other), 1);
        let _ = schedule.step(
            ScheduleInput::Foreground {
                top_level: Some(WINDOW),
            },
            1,
        );
        assert_eq!(
            (
                schedule.running[&Pid(1)].app.left_at,
                schedule.running[&Pid(2)].app.left_at,
            ),
            (0, 3),
            "the first stays in its window, the second is moved away from"
        );
    }
}
