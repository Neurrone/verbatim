//! The read scheduler's generated test: a world of real processes and
//! threads, and a reference model written from the parity ledger's "Reads
//! held behind an application's abandoned reads" and "A slow read the user
//! has moved on from" (`docs/parity/focus-events.md`) and the recorded
//! decisions (`docs/roadmap.md`, "State models"; the limits of
//! `docs/design/state-models.md`). Neither looks at the schedule's rules:
//! the model predicts what each step tells, and every prediction is compared
//! with what the schedule told through [`seam`], the one place that names the
//! schedule's types.
//!
//! The world: a process takes an id, raises events, and exits; its outpost
//! crashes, retires or ends with it; an id is taken again only once the
//! exit has been answered, which a step does whole; an event raised before an
//! exit can arrive after it, while the id is free, and the supervisor then
//! answers that no outpost's hook is in. Reading threads ask, return, hang
//! past their deadlines or are slow, and act only when they can: a waiting
//! thread once woken, the alarm only when set, a little early or later. A
//! case ends with every process exited and every read returned.
//!
//! The model keeps, per process (a generation of its application), its
//! events waiting in push order, the thread in charge with what it does, the
//! threads abandoned to reads, when the limit was reached and whether the
//! process has gone. It predicts, for every step, each event's outcome and
//! why, which thread starts and whether reused, what a thread that asks is
//! told, which reads are judged, when the alarm is due, which idle waiters
//! run and whether a focus is pending. It compares them exactly, the
//! outcomes per application in order.
//!
//! The recorded rules: two abandoned reads hold back an application's next
//! thread until one returns; events wait until the deadline after the second
//! was abandoned, then all are dropped and each later one as it arrives;
//! a read is judged from its half second, every half second, once its
//! window is known, and abandoned at the deadline; an outpost that ends
//! with its application running keeps the abandoned count; a new generation
//! starts with none; a process gone drops at once what waits with no thread
//! and is held no longer than its readers; every event has exactly one
//! outcome; every idle waiter runs once, when every event queued before it
//! has its outcome. The thread in charge reads what it was given, its
//! process gone or not.
//!
//! The world has no shutdown, no failed spawn and no event replaced by a
//! newer one: no recorded rule says what those do. An outpost's end with its
//! application running comes only when no thread is in charge and nothing
//! waits, since no recorded rule says what becomes of a thread then.
//!
//! The menu end's grace is tested by every sequence of up to six steps,
//! below ([`every_short_grace_sequence_keeps_the_graces_rules`]).

mod seam;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestError, TestRunner};

use self::seam::{App, How, ReadToken, Seam, ThreadToken, Told};
use super::*;

/// The read deadline, in microseconds, short enough that time passes it.
const DEADLINE: u64 = 2_000_000;

/// The half second after which a read is judged, in microseconds.
const GRACE: u64 = 500_000;

/// How many abandoned reads hold back an application's next thread.
const LIMIT: usize = 2;

/// The process ids the world's processes take.
const PIDS: [u32; 2] = [1, 2];

/// One step of the world.
#[derive(Clone, Copy, Debug)]
enum Action {
    /// A process takes the free id of `slot`.
    Start { slot: u8 },
    /// An event of the process at `slot`, or, none running, a stale event
    /// of the one that exited.
    Event { slot: u8, kind: u8, window: u8 },
    /// The thread in charge of `slot`'s oldest application that may act
    /// asks for its next event.
    Ask { slot: u8 },
    /// A read of `slot`'s applications returns.
    Return { slot: u8, which: u8, found: bool },
    /// A read of `slot`'s applications learns its window.
    Learn {
        slot: u8,
        which: u8,
        abandoned: bool,
    },
    /// Time passes, and the alarm goes off if it is due.
    Time(Passing),
    /// The outpost of `slot`'s running process ends, crashed or retired.
    OutpostGone { slot: u8 },
    /// The process at `slot` exits, its outpost ending before it (0),
    /// after it (1) or there being none (2).
    Exit { slot: u8, outpost: u8 },
    /// The supervisor answers `slot`'s stale event: no hook is in.
    NoHook { slot: u8 },
    /// An idle waiter is queued.
    WhenIdle,
}

/// How far time moves before the alarm is checked.
#[derive(Clone, Copy, Debug)]
enum Passing {
    /// This many milliseconds.
    By(u16),
    /// To the alarm, and this many milliseconds past it.
    ToAlarm(u8),
    /// The alarm's timer goes off a little early.
    Early,
    /// Past every deadline at once.
    Late,
}

/// The answer the shell gives to every judgement of a step.
#[derive(Clone, Copy, Debug)]
struct Asks {
    /// Whether the user has moved on from the read's window.
    moved_on: bool,
}

/// The window an event's cache gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cached {
    /// It is never judged by the moved-on rule.
    Never,
    /// Its read has still to learn it.
    Unknown,
    /// It is known.
    Known,
}

fn action() -> impl Strategy<Value = Action> {
    use proptest::prelude::*;
    let slot = 0..2u8;
    let passing = prop_oneof![
        3 => (0..3u8).prop_map(Passing::ToAlarm),
        2 => (0..1500u16).prop_map(Passing::By),
        1 => Just(Passing::Early),
        2 => Just(Passing::Late),
    ];
    prop_oneof![
        3 => slot.clone().prop_map(|slot| Action::Start { slot }),
        10 => (slot.clone(), 0..4u8, 0..3u8)
            .prop_map(|(slot, kind, window)| Action::Event { slot, kind, window }),
        8 => slot.clone().prop_map(|slot| Action::Ask { slot }),
        5 => (slot.clone(), 0..4u8, any::<bool>())
            .prop_map(|(slot, which, found)| Action::Return { slot, which, found }),
        4 => (slot.clone(), 0..4u8, any::<bool>())
            .prop_map(|(slot, which, abandoned)| Action::Learn { slot, which, abandoned }),
        7 => passing.prop_map(Action::Time),
        2 => slot.clone().prop_map(|slot| Action::OutpostGone { slot }),
        3 => (slot.clone(), 0..3u8).prop_map(|(slot, outpost)| Action::Exit { slot, outpost }),
        2 => slot.prop_map(|slot| Action::NoHook { slot }),
        2 => Just(Action::WhenIdle),
    ]
}

fn asks() -> impl Strategy<Value = Asks> {
    proptest::bool::weighted(0.3).prop_map(|moved_on| Asks { moved_on })
}

/// An event, as the model holds it.
#[derive(Clone, Copy, Debug)]
struct Event {
    marker: u64,
    kind: FocusEvent,
    cached: Cached,
}

/// When a read's window is judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    /// Never.
    Never,
    /// Not while its window is unknown.
    Unknown,
    /// Next at this time.
    Known(u64),
}

/// A read under way, as the model holds it.
#[derive(Clone, Copy, Debug)]
struct Reading {
    event: Event,
    /// Told when its job is given.
    read: Option<ReadToken>,
    started: u64,
    rule: Rule,
}

/// What the thread in charge does.
#[derive(Clone, Copy, Debug)]
enum Doing {
    /// About to ask for its next event.
    Asking,
    /// Waiting to be woken, or woken and about to ask.
    Parked { woken: bool },
    /// Reading.
    Reading(Reading),
}

/// The thread in charge of a generation.
#[derive(Clone, Copy, Debug)]
struct Thread {
    /// Told when it is spawned.
    id: Option<ThreadToken>,
    doing: Doing,
}

/// A thread abandoned in its read.
#[derive(Clone, Copy, Debug)]
struct Hung {
    read: ReadToken,
    event: Event,
}

/// One process, as the listener sees it: a generation of its application.
#[derive(Debug, Default)]
struct Generation {
    waiting: Vec<Event>,
    thread: Option<Thread>,
    abandoned: Vec<Hung>,
    /// When the second abandonment came, while two are out.
    limit_since: Option<u64>,
    /// Whether its process has gone, by its exit or the supervisor's
    /// answer to a stale event.
    gone: bool,
}

/// A generation's key: its slot and process number.
type Key = (usize, u32);

fn app_of(key: Key) -> App {
    App {
        pid: PIDS[key.0],
        process: key.1,
    }
}

/// What a step should tell, in the shape it is compared in: per process
/// id, in order.
#[derive(Debug, Default, PartialEq, Eq)]
struct Phase {
    outcomes: BTreeMap<u32, Vec<(u64, How)>>,
    spawns: BTreeMap<u32, Vec<bool>>,
    jobs: BTreeMap<u32, Vec<u64>>,
    judges: BTreeMap<u32, u32>,
    ran: Vec<u64>,
}

/// What the model predicts of one step.
#[derive(Debug, Default)]
struct Expected {
    phase: Phase,
    /// The generations a thread starts for, in order.
    spawned: Vec<Key>,
    /// The generation whose thread is given the job.
    job: Option<Key>,
}

impl Expected {
    fn outcome(&mut self, key: Key, marker: u64, how: How) {
        self.phase
            .outcomes
            .entry(PIDS[key.0])
            .or_default()
            .push((marker, how));
    }

    fn spawn(&mut self, key: Key, reuse: bool) {
        self.phase
            .spawns
            .entry(PIDS[key.0])
            .or_default()
            .push(reuse);
        self.spawned.push(key);
    }
}

/// Drops every event waiting behind no thread: outcomes in arrival order.
fn drop_waiting(generation: &mut Generation, key: Key, out: &mut Expected) {
    for event in generation.waiting.drain(..) {
        out.outcome(key, event.marker, How::WaitedTooLong);
    }
}

/// The thread in charge's read is abandoned: its event gets its outcome, a
/// new thread takes what waits below the limit, and at the limit the wait
/// begins, a gone process's events being dropped at once.
fn abandon(generation: &mut Generation, key: Key, how: How, now: u64, out: &mut Expected) {
    let Some(Thread {
        doing: Doing::Reading(reading),
        ..
    }) = generation.thread.take()
    else {
        panic!("an abandonment with no read under way");
    };
    let read = reading.read.expect("a read's token is told at its job");
    out.outcome(key, reading.event.marker, how);
    generation.abandoned.push(Hung {
        read,
        event: reading.event,
    });
    if generation.abandoned.len() >= LIMIT && generation.limit_since.is_none() {
        generation.limit_since = Some(now);
    }
    if generation.waiting.is_empty() {
        return;
    }
    if generation.abandoned.len() < LIMIT {
        generation.thread = Some(Thread {
            id: None,
            doing: Doing::Asking,
        });
        out.spawn(key, false);
    } else if generation.gone {
        drop_waiting(generation, key, out);
    }
}

/// The reference model.
#[derive(Debug, Default)]
struct Model {
    generations: BTreeMap<Key, Generation>,
    /// The process running at each slot.
    running: [Option<u32>; 2],
    /// The generation of a stale event not yet answered, at each slot.
    stale: [Option<u32>; 2],
    /// Whether a process has ever exited at each slot.
    exited_once: [bool; 2],
    next_process: u32,
    next_marker: u64,
    next_waiter: u64,
    /// Each event's place in push order, by marker.
    places: BTreeMap<u64, u64>,
    done: BTreeSet<u64>,
    /// The idle waiters: each with the count of events queued at it.
    waiters: Vec<(u64, u64)>,
    ran: BTreeSet<u64>,
    now: u64,
}

impl Model {
    /// When anything is next due.
    fn soonest_due(&self) -> Option<u64> {
        self.generations
            .values()
            .flat_map(|generation| {
                let reading = match generation.thread {
                    Some(Thread {
                        doing: Doing::Reading(reading),
                        ..
                    }) => {
                        let judged = match reading.rule {
                            Rule::Known(next) => Some(next),
                            Rule::Never | Rule::Unknown => None,
                        };
                        Some(judged.map_or(reading.started + DEADLINE, |next| {
                            next.min(reading.started + DEADLINE)
                        }))
                    }
                    _ => None,
                };
                let bound = (generation.thread.is_none() && !generation.waiting.is_empty())
                    .then_some(generation.limit_since.map(|since| since + DEADLINE))
                    .flatten();
                reading.into_iter().chain(bound)
            })
            .min()
    }

    /// Whether a focus is pending for a menu opening of `pid`'s.
    fn focus_pending(&self, pid: Option<u32>) -> bool {
        self.generations.iter().any(|(key, generation)| {
            let waiting = generation
                .waiting
                .iter()
                .any(|event| event.kind == FocusEvent::Focus);
            let reading = matches!(
                generation.thread,
                Some(Thread {
                    doing: Doing::Reading(reading),
                    ..
                }) if reading.event.kind == FocusEvent::Focus
            );
            reading || (waiting && (generation.thread.is_some() || Some(PIDS[key.0]) == pid))
        })
    }

    /// The events of the step are done; `out` gains the idle waiters that
    /// run.
    fn commit(&mut self, out: &mut Expected) {
        for outcomes in out.phase.outcomes.values() {
            self.done.extend(outcomes.iter().map(|(marker, _)| *marker));
        }
        let mut ran = Vec::new();
        let (done, places) = (&self.done, &self.places);
        self.waiters.retain(|&(before, waiter)| {
            let idle = places
                .iter()
                .all(|(marker, place)| *place > before || done.contains(marker));
            if idle {
                ran.push(waiter);
            }
            !idle
        });
        ran.sort_unstable();
        self.ran.extend(ran.iter().copied());
        out.phase.ran = ran;
    }
}

/// The listed things every gate run must reach.
const LISTED: [&str; 19] = [
    "an id reused with its readers out",
    "a stale event",
    "the abandoned limit reached",
    "an event waiting at the limit",
    "events dropped at their bound",
    "events dropped as one arrives",
    "a thread reused",
    "an abandonment at the deadline",
    "an abandonment for moving on",
    "a judgement asked",
    "an abandoned read returned",
    "an exit with a read under way",
    "an exit dropping waiting events",
    "a crash keeping a count",
    "a thread told to end",
    "a thread told to wait",
    "an idle waiter run",
    "an early alarm",
    "a learned window judged at once",
];

/// One case's run: the world, the schedule and the model side by side.
struct Run {
    seam: Seam,
    model: Model,
    trail: Vec<String>,
    log: Vec<String>,
    reached: BTreeMap<&'static str, u32>,
}

impl Run {
    fn new() -> Self {
        Self {
            seam: Seam::new(DEADLINE),
            model: Model::default(),
            trail: Vec::new(),
            log: Vec::new(),
            reached: BTreeMap::new(),
        }
    }

    /// Runs `actions`, then ends the case.
    fn case(actions: &[(Action, Asks)]) -> Self {
        let mut run = Self::new();
        for &(action, asks) in actions {
            run.act(action, asks);
        }
        run.drain();
        run
    }

    fn reach(&mut self, state: &'static str) {
        *self.reached.entry(state).or_default() += 1;
    }

    /// Applies `action` and checks what it told. An action the world
    /// cannot take now does nothing.
    fn act(&mut self, action: Action, asks: Asks) {
        self.trail.push(format!("{action:?} {asks:?}"));
        let acted = match action {
            Action::Start { slot } => self.start(usize::from(slot)),
            Action::Event { slot, kind, window } => {
                self.event(usize::from(slot), kind, window, asks)
            }
            Action::Ask { slot } => self.ask(usize::from(slot), asks),
            Action::Return { slot, which, found } => {
                self.read_returns(usize::from(slot), which, found, asks)
            }
            Action::Learn {
                slot,
                which,
                abandoned,
            } => self.learn(usize::from(slot), which, abandoned, asks),
            Action::Time(passing) => self.time(passing, asks),
            Action::OutpostGone { slot } => self.outpost_gone(usize::from(slot), asks),
            Action::Exit { slot, outpost } => self.exit(usize::from(slot), outpost, asks),
            Action::NoHook { slot } => self.no_hook(usize::from(slot), asks),
            Action::WhenIdle => {
                self.model.next_waiter += 1;
                let waiter = self.model.next_waiter;
                let queued = self.model.places.len() as u64;
                self.model.waiters.push((queued, waiter));
                let told = self.seam.when_idle(waiter);
                self.compare(Expected::default(), told, asks);
                true
            }
        };
        if acted {
            self.check_state();
        }
    }

    /// A step's told effects against the model's prediction, then the
    /// tokens bound and the judgements answered.
    fn compare(&mut self, mut expected: Expected, told: Vec<Told>, asks: Asks) {
        self.log.push(format!("{told:?}"));
        self.model.commit(&mut expected);
        if !expected.phase.ran.is_empty() {
            self.reach("an idle waiter run");
        }
        let mut actual = Phase::default();
        let mut started = Vec::new();
        let mut reads = Vec::new();
        let mut woken = Vec::new();
        let mut judged = Vec::new();
        for each in told {
            match each {
                Told::Outcome { pid, marker, how } => {
                    actual.outcomes.entry(pid).or_default().push((marker, how));
                }
                Told::Start { pid, thread, reuse } => {
                    actual.spawns.entry(pid).or_default().push(reuse);
                    started.push((pid, thread));
                }
                Told::Woken(pid) => woken.push(pid),
                Told::Job { pid, marker, read } => {
                    actual.jobs.entry(pid).or_default().push(marker);
                    reads.push(read);
                }
                Told::Judge { pid, read } => {
                    *actual.judges.entry(pid).or_default() += 1;
                    judged.push((pid, read));
                }
                Told::Ran(waiter) => actual.ran.push(waiter),
            }
        }
        actual.ran.sort_unstable();
        assert_eq!(
            actual, expected.phase,
            "what the step told, against the model's prediction: {:?}",
            self.trail
        );
        self.bind(&expected, &started, &reads);
        self.wake(&woken);
        for (pid, read) in judged {
            self.answer(pid, read, asks);
        }
    }

    /// Records the tokens the step told, for what the threads will send.
    fn bind(&mut self, expected: &Expected, started: &[(u32, ThreadToken)], reads: &[ReadToken]) {
        for (key, (pid, thread)) in expected.spawned.iter().zip(started) {
            assert_eq!(PIDS[key.0], *pid, "spawned in the model's order");
            let generation = self.model.generations.get_mut(key).expect("spawned");
            generation.thread.as_mut().expect("spawned").id = Some(*thread);
        }
        if let (Some(key), Some(read)) = (expected.job, reads.first()) {
            let generation = self.model.generations.get_mut(&key).expect("a job's");
            if let Some(Thread {
                doing: Doing::Reading(reading),
                ..
            }) = generation.thread.as_mut()
            {
                reading.read = Some(*read);
            }
        }
    }

    /// The woken threads may ask.
    fn wake(&mut self, woken: &[u32]) {
        for (key, generation) in &mut self.model.generations {
            if !woken.contains(&PIDS[key.0]) {
                continue;
            }
            if let Some(Thread {
                doing: Doing::Parked { woken },
                ..
            }) = generation.thread.as_mut()
            {
                *woken = true;
            }
        }
    }

    /// The shell answers the judgement of `pid`'s read.
    fn answer(&mut self, pid: u32, read: ReadToken, asks: Asks) {
        let key = self
            .model
            .generations
            .iter()
            .find(|(key, generation)| {
                PIDS[key.0] == pid
                    && matches!(generation.thread, Some(Thread {
                        doing: Doing::Reading(reading),
                        ..
                    }) if reading.read == Some(read))
            })
            .map(|(key, _)| *key)
            .expect("a judgement of a read under way");
        let now = self.model.now;
        let mut expected = Expected::default();
        if asks.moved_on {
            let generation = self.model.generations.get_mut(&key).expect("found");
            abandon(generation, key, How::MovedOn, now, &mut expected);
            self.reach("an abandonment for moving on");
        }
        let told = self.seam.judged(app_of(key), read, asks.moved_on);
        self.compare(expected, told, asks);
    }

    fn start(&mut self, slot: usize) -> bool {
        if self.model.running[slot].is_some() || self.model.stale[slot].is_some() {
            return false;
        }
        self.model.next_process += 1;
        let process = self.model.next_process;
        self.model.running[slot] = Some(process);
        self.model
            .generations
            .insert((slot, process), Generation::default());
        if self
            .model
            .generations
            .keys()
            .any(|(other, older)| *other == slot && *older != process)
        {
            self.reach("an id reused with its readers out");
        }
        true
    }

    fn event(&mut self, slot: usize, kind: u8, window: u8, asks: Asks) -> bool {
        let process = match self.model.running[slot].or(self.model.stale[slot]) {
            Some(process) => process,
            None if self.model.exited_once[slot] => {
                self.model.next_process += 1;
                let process = self.model.next_process;
                self.model.stale[slot] = Some(process);
                self.model
                    .generations
                    .insert((slot, process), Generation::default());
                self.reach("a stale event");
                process
            }
            None => return false,
        };
        let key = (slot, process);
        let kind = match kind {
            0 => FocusEvent::Focus,
            1 => FocusEvent::MenuOpened,
            2 => FocusEvent::Selection,
            _ => FocusEvent::Notification,
        };
        let cached = match (kind, window) {
            (FocusEvent::Selection | FocusEvent::Notification, _) => Cached::Never,
            (_, 0) => Cached::Unknown,
            _ => Cached::Known,
        };
        self.model.next_marker += 1;
        let marker = self.model.next_marker;
        let place = self.model.places.len() as u64 + 1;
        self.model.places.insert(marker, place);
        let event = Event {
            marker,
            kind,
            cached,
        };
        let now = self.model.now;
        let mut expected = Expected::default();
        let generation = self.model.generations.get_mut(&key).expect("its record");
        generation.waiting.push(event);
        let mut waits = false;
        let mut dropped = false;
        if generation.thread.is_none() {
            if generation.abandoned.len() < LIMIT {
                generation.thread = Some(Thread {
                    id: None,
                    doing: Doing::Asking,
                });
                expected.spawn(key, false);
            } else if generation
                .limit_since
                .is_some_and(|since| now >= since + DEADLINE)
            {
                drop_waiting(generation, key, &mut expected);
                dropped = true;
            } else {
                waits = true;
            }
        }
        if waits {
            self.reach("an event waiting at the limit");
        }
        if dropped {
            self.reach("events dropped as one arrives");
        }
        let told = self.seam.event(app_of(key), marker, kind, cached);
        self.compare(expected, told, asks);
        true
    }

    fn ask(&mut self, slot: usize, asks: Asks) -> bool {
        let Some(key) = self
            .model
            .generations
            .iter()
            .find(|(key, generation)| {
                key.0 == slot
                    && matches!(
                        generation.thread,
                        Some(Thread {
                            doing: Doing::Asking | Doing::Parked { woken: true },
                            ..
                        })
                    )
            })
            .map(|(key, _)| *key)
        else {
            return false;
        };
        let now = self.model.now;
        let mut expected = Expected::default();
        let generation = self.model.generations.get_mut(&key).expect("found");
        let thread = generation.thread.as_mut().expect("found");
        let token = thread.id.expect("told at its spawn");
        let asked = if generation.waiting.is_empty() {
            if generation.gone {
                Asked::Ends
            } else {
                Asked::Waits
            }
        } else {
            Asked::Reads
        };
        match asked {
            Asked::Reads => {
                let event = generation.waiting.remove(0);
                thread.doing = Doing::Reading(Reading {
                    event,
                    read: None,
                    started: now,
                    rule: match event.cached {
                        Cached::Never => Rule::Never,
                        Cached::Unknown => Rule::Unknown,
                        Cached::Known => Rule::Known(now + GRACE),
                    },
                });
                expected
                    .phase
                    .jobs
                    .entry(PIDS[key.0])
                    .or_default()
                    .push(event.marker);
                expected.job = Some(key);
            }
            Asked::Waits => {
                thread.doing = Doing::Parked { woken: false };
                self.reach("a thread told to wait");
            }
            Asked::Ends => {
                generation.thread = None;
                self.reach("a thread told to end");
            }
        }
        let told = self.seam.next(app_of(key), token);
        self.compare(expected, told, asks);
        if asked != Asked::Reads {
            assert_eq!(
                self.seam.in_charge(app_of(key), token),
                asked == Asked::Waits,
                "the thread told to wait or to end, by the model: {:?}: {:?}",
                asked,
                self.trail
            );
        }
        true
    }

    /// `slot`'s reads out, in key order: the generation, and the abandoned
    /// read's index when not the thread in charge's.
    fn reads_out(&self, slot: usize) -> Vec<(Key, Option<usize>)> {
        self.model
            .generations
            .iter()
            .filter(|(key, _)| key.0 == slot)
            .flat_map(|(key, generation)| {
                let in_charge = matches!(
                    generation.thread,
                    Some(Thread {
                        doing: Doing::Reading(_),
                        ..
                    })
                )
                .then_some((*key, None));
                let abandoned = (0..generation.abandoned.len()).map(|index| (*key, Some(index)));
                in_charge.into_iter().chain(abandoned).collect::<Vec<_>>()
            })
            .collect()
    }

    fn read_returns(&mut self, slot: usize, which: u8, found: bool, asks: Asks) -> bool {
        let reads = self.reads_out(slot);
        if reads.is_empty() {
            return false;
        }
        let (key, hung) = reads[usize::from(which) % reads.len()];
        let mut expected = Expected::default();
        let generation = self.model.generations.get_mut(&key).expect("its record");
        let (read, event) = if let Some(index) = hung {
            let Hung { read, event } = generation.abandoned.remove(index);
            if generation.abandoned.len() < LIMIT {
                generation.limit_since = None;
            }
            // An abandoned thread's return takes what waits with no thread.
            if generation.thread.is_none() && !generation.waiting.is_empty() {
                generation.thread = Some(Thread {
                    id: None,
                    doing: Doing::Asking,
                });
                expected.spawn(key, true);
                self.reach("a thread reused");
            }
            self.reach("an abandoned read returned");
            (read, event)
        } else {
            let thread = generation.thread.as_mut().expect("under way");
            let Doing::Reading(reading) = thread.doing else {
                panic!("a read's return with none under way");
            };
            thread.doing = Doing::Asking;
            let how = if found { How::Read } else { How::Failed };
            expected.outcome(key, reading.event.marker, how);
            (reading.read.expect("told at its job"), reading.event)
        };
        let told = self
            .seam
            .returned(app_of(key), read, event.marker, event.kind, found);
        self.compare(expected, told, asks);
        true
    }

    fn learn(&mut self, slot: usize, which: u8, abandoned: bool, asks: Asks) -> bool {
        let reads: Vec<(Key, Option<usize>)> = self
            .reads_out(slot)
            .into_iter()
            .filter(|(_, hung)| hung.is_some() == abandoned)
            .collect();
        if reads.is_empty() {
            return false;
        }
        let (key, hung) = reads[usize::from(which) % reads.len()];
        let now = self.model.now;
        let generation = self.model.generations.get_mut(&key).expect("its record");
        let mut expected = Expected::default();
        let mut at_once = false;
        let read = if let Some(index) = hung {
            generation.abandoned[index].read
        } else {
            let Some(Thread {
                doing: Doing::Reading(reading),
                ..
            }) = generation.thread.as_mut()
            else {
                panic!("a read's learning with none under way");
            };
            let read = reading.read.expect("told at its job");
            if reading.rule == Rule::Unknown {
                let end = reading.started + GRACE;
                reading.rule = Rule::Known(end.max(now));
                if now >= end {
                    reading.rule = Rule::Known(now + GRACE);
                    *expected.phase.judges.entry(PIDS[key.0]).or_default() += 1;
                    at_once = true;
                }
            }
            read
        };
        if at_once {
            self.reach("a learned window judged at once");
        }
        let told = self.seam.learned(app_of(key), read);
        self.compare(expected, told, asks);
        true
    }

    fn time(&mut self, passing: Passing, asks: Asks) -> bool {
        let alarm = self.seam.alarm();
        let now = self.model.now;
        self.model.now = match passing {
            Passing::By(ms) => now + u64::from(ms) * 1_000,
            Passing::ToAlarm(past) => match alarm {
                Some(alarm) => alarm.max(now) + u64::from(past) * 1_000,
                None => now,
            },
            Passing::Early => now,
            Passing::Late => now + 3 * DEADLINE,
        };
        self.seam.set_now(self.model.now);
        let fires = match (passing, alarm) {
            (_, None) => false,
            (Passing::Early, Some(alarm)) => alarm > self.model.now,
            (_, Some(alarm)) => alarm <= self.model.now,
        };
        if fires {
            if matches!(passing, Passing::Early) {
                self.reach("an early alarm");
            }
            self.tick(asks);
        }
        true
    }

    /// The alarm goes off: at the model's time, what is due is done, each
    /// read's deadline before its judgement.
    fn tick(&mut self, asks: Asks) {
        let now = self.model.now;
        let mut expected = Expected::default();
        let mut abandoned = 0;
        let mut dropped = false;
        let mut judged = 0;
        for (key, generation) in &mut self.model.generations {
            match generation.thread {
                Some(Thread {
                    doing: Doing::Reading(reading),
                    ..
                }) => {
                    if now >= reading.started + DEADLINE {
                        abandon(generation, *key, How::NotInTime, now, &mut expected);
                        abandoned += 1;
                    } else if let Rule::Known(next) = reading.rule
                        && now >= next
                    {
                        if let Some(Thread {
                            doing: Doing::Reading(reading),
                            ..
                        }) = generation.thread.as_mut()
                        {
                            reading.rule = Rule::Known(now + GRACE);
                        }
                        *expected.phase.judges.entry(PIDS[key.0]).or_default() += 1;
                        judged += 1;
                    }
                }
                Some(_) => {}
                None => {
                    if !generation.waiting.is_empty()
                        && generation
                            .limit_since
                            .is_some_and(|since| now >= since + DEADLINE)
                    {
                        drop_waiting(generation, *key, &mut expected);
                        dropped = true;
                    }
                }
            }
        }
        for _ in 0..abandoned {
            self.reach("an abandonment at the deadline");
            if self
                .model
                .generations
                .values()
                .any(|generation| generation.abandoned.len() == LIMIT)
            {
                self.reach("the abandoned limit reached");
            }
        }
        for _ in 0..judged {
            self.reach("a judgement asked");
        }
        if dropped {
            self.reach("events dropped at their bound");
        }
        let told = self.seam.tick();
        self.compare(expected, told, asks);
    }

    /// An outpost ends with its application running: nothing changes for
    /// the listener's record. The world has this only with no thread in
    /// charge and nothing waiting.
    fn outpost_gone(&mut self, slot: usize, asks: Asks) -> bool {
        let Some(process) = self.model.running[slot] else {
            return false;
        };
        let key = (slot, process);
        let generation = self.model.generations.get(&key).expect("its record");
        if generation.thread.is_some() || !generation.waiting.is_empty() {
            return false;
        }
        if !generation.abandoned.is_empty() {
            self.reach("a crash keeping a count");
        }
        let told = self.seam.outpost_gone(app_of(key));
        self.compare(Expected::default(), told, asks);
        true
    }

    /// The process at `slot` exits: its generation is gone, and what waits
    /// with no thread is dropped at once.
    fn exit(&mut self, slot: usize, outpost: u8, asks: Asks) -> bool {
        let Some(process) = self.model.running[slot] else {
            return false;
        };
        let key = (slot, process);
        self.model.running[slot] = None;
        self.model.exited_once[slot] = true;
        let mut expected = Expected::default();
        let generation = self.model.generations.get_mut(&key).expect("its record");
        generation.gone = true;
        let reading = matches!(
            generation.thread,
            Some(Thread {
                doing: Doing::Reading(_),
                ..
            })
        );
        let dropping = generation.thread.is_none() && !generation.waiting.is_empty();
        if dropping {
            drop_waiting(generation, key, &mut expected);
        }
        if reading {
            self.reach("an exit with a read under way");
        }
        if dropping {
            self.reach("an exit dropping waiting events");
        }
        let mut told = Vec::new();
        if outpost == 0 {
            told.extend(self.seam.no_hook(app_of(key)));
        }
        told.extend(self.seam.exited(app_of(key)));
        if outpost == 1 {
            told.extend(self.seam.no_hook(app_of(key)));
        }
        self.compare(expected, told, asks);
        true
    }

    /// The supervisor answers a stale event's generation: its process is
    /// gone.
    fn no_hook(&mut self, slot: usize, asks: Asks) -> bool {
        let Some(process) = self.model.stale[slot].take() else {
            return false;
        };
        let key = (slot, process);
        let generation = self.model.generations.get_mut(&key).expect("its record");
        generation.gone = true;
        let told = self.seam.no_hook(app_of(key));
        self.compare(Expected::default(), told, asks);
        true
    }

    /// After an action: the alarm, the pending focus, and that nothing is
    /// held for a generation whose process has gone and whose readers have
    /// all returned.
    fn check_state(&mut self) {
        assert_eq!(
            self.seam.alarm(),
            self.model.soonest_due(),
            "when the alarm is due: {:?}",
            self.trail
        );
        for pid in std::iter::once(None).chain(PIDS.iter().map(|pid| Some(*pid))) {
            assert_eq!(
                self.seam.focus_pending(pid),
                self.model.focus_pending(pid),
                "whether a focus is pending, for {pid:?}: {:?}",
                self.trail
            );
        }
        let spent: Vec<Key> = self
            .model
            .generations
            .iter()
            .filter(|(_, generation)| {
                generation.gone
                    && generation.thread.is_none()
                    && generation.abandoned.is_empty()
                    && generation.waiting.is_empty()
            })
            .map(|(key, _)| *key)
            .collect();
        for key in spent {
            assert!(
                !self.seam.held(app_of(key)),
                "a generation held after its process and readers have gone: {key:?}: {:?}",
                self.trail
            );
            self.model.generations.remove(&key);
        }
    }

    /// Ends the case: every process exits, every thread acts and every
    /// read returns; then every event has had its outcome, every waiter
    /// has run, and nothing is held.
    fn drain(&mut self) {
        let plain = Asks { moved_on: false };
        for slot in 0..2u8 {
            self.act(Action::Exit { slot, outpost: 2 }, plain);
        }
        for _ in 0..1_000 {
            let asking = self.model.generations.iter().find(|(_, generation)| {
                matches!(
                    generation.thread,
                    Some(Thread {
                        doing: Doing::Asking | Doing::Parked { woken: true },
                        ..
                    })
                )
            });
            if let Some((key, _)) = asking {
                let slot = u8::try_from(key.0).expect("two slots");
                self.act(Action::Ask { slot }, plain);
                continue;
            }
            let reading = (0..2).find(|slot| !self.reads_out(*slot).is_empty());
            if let Some(slot) = reading {
                let slot = u8::try_from(slot).expect("two slots");
                let action = Action::Return {
                    slot,
                    which: 0,
                    found: false,
                };
                self.act(action, plain);
                continue;
            }
            if let Some(slot) = (0..2).find(|slot| self.model.stale[*slot].is_some()) {
                let slot = u8::try_from(slot).expect("two slots");
                self.act(Action::NoHook { slot }, plain);
                continue;
            }
            if self.seam.alarm().is_some() {
                self.act(Action::Time(Passing::ToAlarm(0)), plain);
                continue;
            }
            break;
        }
        let trail = &self.trail;
        let model = &self.model;
        for marker in model.places.keys() {
            assert!(
                model.done.contains(marker),
                "event {marker} had no outcome: {trail:?}"
            );
        }
        for waiter in 1..=model.next_waiter {
            assert!(
                model.ran.contains(&waiter),
                "waiter {waiter} never ran: {trail:?}"
            );
        }
        assert!(
            model.generations.is_empty(),
            "held at the end, with every process exited: {:?}: {trail:?}",
            model.generations.keys().collect::<Vec<_>>()
        );
    }
}

/// What a thread that asks is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    /// To read the event at the queue's head.
    Reads,
    /// To wait.
    Waits,
    /// To end.
    Ends,
}

/// Runs the generated cases `config` draws, each twice from a new
/// schedule, shrinking a failure to its smallest case; what the cases
/// reached.
///
/// # Panics
///
/// On a failing case, naming the smallest.
pub(super) fn run_generated(config: Config, again: &str) -> BTreeMap<&'static str, u32> {
    let reached = RefCell::new(BTreeMap::new());
    let mut runner = TestRunner::new(config);
    let strategy = proptest::collection::vec((action(), asks()), 1..40);
    let result = runner.run(&strategy, |actions| {
        let first = Run::case(&actions);
        let second = Run::case(&actions);
        assert_eq!(first.log, second.log, "two runs of one case differ");
        let mut reached = reached.borrow_mut();
        for (state, count) in first.reached {
            *reached.entry(state).or_default() += count;
        }
        Ok(())
    });
    match result {
        Ok(()) => reached.into_inner(),
        Err(TestError::Fail(reason, case)) => panic!("{reason}\n{}", fixed_report(&case, again)),
        Err(error) => panic!("{error}"),
    }
}

impl Action {
    /// The action as the Rust expression that makes it, as a fixed case
    /// lists it.
    fn code(self) -> String {
        match self {
            Action::Time(passing) => format!("Action::Time(Passing::{passing:?})"),
            other => format!("Action::{other:?}"),
        }
    }
}

/// The report of a failing case: `case` as a line ready to paste into
/// [`fixed`], and the commands that run the fixed cases and the run that
/// found it, `again`.
fn fixed_report(case: &[(Action, Asks)], again: &str) -> String {
    let steps: Vec<String> = case
        .iter()
        .map(|(action, asks)| format!("({}, {asks:?})", action.code()))
        .collect();
    format!(
        "the smallest failing case, as a line for `fixed()` in crates/verbatim-listener-logic/src/schedule/generated.rs:\n        vec![{}],\n\
         the fixed cases alone: cargo test -p verbatim-listener-logic --lib -- --exact schedule::generated::fixed_cases_keep_the_schedules_rules\n\
         the run that found it again: {again}",
        steps.join(", ")
    )
}

/// The gate's seed and case count.
const SEED: u64 = 20_261_109;
const CASES: u32 = 5_000;

/// Generated cases of the world (processes starting, exiting and reusing an
/// id, stale events, every kind of event, threads asking, returning, hanging
/// and slow, windows learned, outposts ending, alarms going off, waiters) are
/// compared with the reference model after every step, each run twice for
/// the same effects, and reach every listed state.
#[test]
fn generated_cases_keep_the_schedules_rules() {
    let reached = run_generated(
        super::explore::config(SEED, CASES),
        "cargo test -p verbatim-listener-logic --lib -- --exact schedule::generated::generated_cases_keep_the_schedules_rules",
    );
    let missed: Vec<&str> = LISTED
        .into_iter()
        .filter(|state| !reached.contains_key(state))
        .collect();
    assert_eq!(
        missed,
        Vec::<&str>::new(),
        "states never reached; reached {reached:?}"
    );
}

/// The cases the generated test found, each the smallest a failure shrank
/// to or that case made explicit, kept as lists of actions. In order: a new
/// process of a reused id, whose first event must have a thread, inheriting
/// its predecessor's two abandoned reads; a stale event of an exited
/// process, joining the exited process's record instead of starting a
/// generation of its own, as an event clears `ending`; and events waiting
/// at the abandoned limit when the process exits, dropped at once.
fn fixed() -> Vec<Vec<(Action, Asks)>> {
    let plain = |action| (action, Asks { moved_on: false });
    let start = plain(Action::Start { slot: 0 });
    let focus = plain(Action::Event {
        slot: 0,
        kind: 0,
        window: 0,
    });
    let ask = plain(Action::Ask { slot: 0 });
    let late = plain(Action::Time(Passing::Late));
    let exit = plain(Action::Exit {
        slot: 0,
        outpost: 0,
    });
    vec![
        vec![
            start, focus, focus, ask, late, ask, late, exit, start, focus,
        ],
        vec![start, focus, exit, focus],
        vec![start, focus, focus, ask, late, ask, focus, late],
    ]
}

/// Every fixed case keeps the schedule's rules, and gives the same effects
/// when run twice.
#[test]
fn fixed_cases_keep_the_schedules_rules() {
    for case in fixed() {
        let first = Run::case(&case);
        assert_eq!(first.log, Run::case(&case).log, "{case:?}");
    }
}

/// One step of a sequence of the listener's timer: a menu end's grace,
/// the desktop's return and the desktop name's due time.
#[derive(Clone, Copy, Debug)]
enum Grace {
    /// A menu ends, its grace 300 microseconds.
    Ended,
    /// The user's desktop returns, told 200 microseconds later.
    Returned,
    /// A desktop name is sent, its due time 250 microseconds later.
    NameSent,
    /// The timer goes off, as set or early.
    Fired,
    /// An idle round is to be answered.
    IdleAnswer,
    /// The timer is closed.
    Close,
    /// Time passes, this many microseconds.
    Wait(u64),
}

/// Each step a grace sequence may take.
const GRACE_STEPS: [Grace; 8] = [
    Grace::Ended,
    Grace::Returned,
    Grace::NameSent,
    Grace::Fired,
    Grace::IdleAnswer,
    Grace::Close,
    Grace::Wait(100),
    Grace::Wait(300),
];

/// The grace's length the sequences give.
const GRACE_US: u64 = 300;

/// The desktop return's delay the sequences give.
const RETURN_US: u64 = 200;

/// The desktop name's due time the sequences give.
const NAME_DUE_US: u64 = 250;

/// The three things that wait on the timer, as the model numbers them.
const END: usize = 0;
const RETURN: usize = 1;
const NAME: usize = 2;

/// A grace sequence, with what it has sent: the reference model, which
/// keeps each of the three waiting things' due time on its own.
#[derive(Clone, Debug, Default)]
struct GraceRun {
    grace: GraceState,
    trail: Vec<Grace>,
    now: u64,
    /// How many ends and rounds have come.
    ends: u64,
    rounds: u64,
    /// When each waiting thing is due, while it has not been sent since
    /// its newest arming: the end, the return and the name's due time.
    pending: [Option<u64>; 3],
    /// The rounds not answered as they came.
    held: Vec<u64>,
    closed: bool,
    /// The ends sent, and the rounds answered.
    sent: Vec<u64>,
    answered: Vec<u64>,
}

impl GraceRun {
    /// Takes `step`, checking its effects against the timer's rules.
    fn step(&mut self, step: Grace) {
        self.trail.push(step);
        let now = self.now;
        let input = match step {
            Grace::Ended => {
                self.ends += 1;
                self.pending[END] = Some(now + GRACE_US);
                GraceInput::Ended {
                    ended_at_us: self.ends,
                    timing: EventTiming::default(),
                    grace: GRACE_US,
                }
            }
            Grace::Returned => {
                self.pending[RETURN] = Some(now + RETURN_US);
                GraceInput::Returned {
                    timing: EventTiming::default(),
                    after: RETURN_US,
                }
            }
            Grace::NameSent => {
                self.pending[NAME] = Some(now + NAME_DUE_US);
                GraceInput::NameSent {
                    timing: EventTiming::default(),
                    after: NAME_DUE_US,
                }
            }
            Grace::Fired => GraceInput::Fired,
            Grace::IdleAnswer => {
                self.rounds += 1;
                GraceInput::IdleAnswer(self.rounds)
            }
            Grace::Close => {
                self.closed = true;
                GraceInput::Close
            }
            Grace::Wait(us) => {
                self.now += us;
                return;
            }
        };
        let effects = self.grace.step(input, now);
        self.check(step, now, &effects);
    }

    /// Checks `effects`, those of `step` taken at `now`, against the
    /// timer's rules and the model, which it moves on.
    fn check(&mut self, step: Grace, now: u64, effects: &[GraceEffect]) {
        // The due times of what this step sent, in the order sent.
        let mut sent_dues = Vec::new();
        let mut timers = Vec::new();
        for effect in effects {
            let thing = match effect {
                GraceEffect::SendEnd { ended_at_us, .. } => {
                    // Only the newest end is sent, and once.
                    assert_eq!(*ended_at_us, self.ends, "{:?}", self.trail);
                    assert!(
                        !self.sent.contains(ended_at_us),
                        "an end sent twice: {:?}",
                        self.trail
                    );
                    self.sent.push(*ended_at_us);
                    END
                }
                GraceEffect::SendReturned(_) => RETURN,
                GraceEffect::SendNameDue(_) => NAME,
                GraceEffect::SendIdleAnswer(round) => {
                    assert!(
                        !self.answered.contains(round),
                        "a round answered twice: {:?}",
                        self.trail
                    );
                    // A round is answered only once nothing waits.
                    assert_eq!(
                        self.pending, [None; 3],
                        "round {round} answered while something waits: {:?}",
                        self.trail
                    );
                    self.held.retain(|held| held != round);
                    self.answered.push(*round);
                    continue;
                }
                GraceEffect::SetTimer(after) => {
                    timers.push(*after);
                    continue;
                }
            };
            // Each thing is sent only while it waits, once its time is
            // over, and once.
            let due = self.pending[thing].take().unwrap_or_else(|| {
                panic!("{effect:?} sent with nothing waiting: {:?}", self.trail)
            });
            assert!(
                due <= now,
                "{effect:?} sent before its time: {:?}",
                self.trail
            );
            sent_dues.push(due);
        }
        assert!(
            sent_dues.is_sorted(),
            "sent out of the order of their times: {:?}",
            self.trail
        );
        if matches!(step, Grace::Fired) {
            assert!(
                self.pending.iter().flatten().all(|&due| due > now),
                "a thing due left unsent: {:?}",
                self.trail
            );
        }
        // The timer is set for the soonest thing waiting, after a step that
        // can change it, and never while closed.
        let soonest = self.pending.iter().flatten().min().copied();
        let expected = match (step, soonest) {
            (Grace::IdleAnswer | Grace::Close, _) | (_, None) => None,
            _ if self.closed => None,
            (_, Some(due)) => Some(due.saturating_sub(now)),
        };
        assert_eq!(
            timers,
            expected.into_iter().collect::<Vec<_>>(),
            "the timer: {:?}",
            self.trail
        );
        if matches!(step, Grace::IdleAnswer) && !self.answered.contains(&self.rounds) {
            self.held.push(self.rounds);
        }
        // No round is held while nothing waits.
        assert!(
            self.pending.iter().any(Option::is_some) || self.held.is_empty(),
            "a round held with nothing waiting: {:?}",
            self.trail
        );
    }

    /// Ends the sequence as the timer would: it fires at each waiting
    /// thing's time, unless closed. Then every round is answered once,
    /// the newest end sent once and nothing waits; closed, at most once
    /// each.
    fn drain(mut self) {
        while !self.closed
            && let Some(&due) = self.pending.iter().flatten().min()
        {
            self.now = self.now.max(due);
            self.step(Grace::Fired);
        }
        if !self.closed {
            let mut answered = self.answered.clone();
            answered.sort_unstable();
            assert_eq!(
                answered,
                (1..=self.rounds).collect::<Vec<_>>(),
                "every round answered: {:?}",
                self.trail
            );
            if self.ends > 0 {
                assert_eq!(
                    self.sent.last(),
                    Some(&self.ends),
                    "the newest end sent: {:?}",
                    self.trail
                );
            }
        }
    }
}

/// How many steps a grace sequence has at most.
const GRACE_DEPTH: usize = 6;

/// Runs every sequence that extends `run` up to `depth` steps, draining
/// each; a failing sequence is printed as a line for [`found_graces`]
/// before the failure.
fn every_grace_sequence(run: &GraceRun, depth: usize) {
    grace_reported(&run.trail, || run.clone().drain());
    if run.trail.len() == depth {
        return;
    }
    for step in GRACE_STEPS {
        let mut next = run.clone();
        let trail: Vec<Grace> = run.trail.iter().copied().chain([step]).collect();
        grace_reported(&trail, || next.step(step));
        every_grace_sequence(&next, depth);
    }
}

/// Runs `check`, the last step of the grace sequence `trail`; when it
/// fails, prints `trail` as a line ready to paste into [`found_graces`]
/// and the command that runs the found sequences, then fails as it did.
fn grace_reported(trail: &[Grace], check: impl FnOnce()) {
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(check)) {
        let steps: Vec<String> = trail
            .iter()
            .map(|step| format!("Grace::{step:?}"))
            .collect();
        eprintln!(
            "the failing sequence, as a line for `found_graces()` in crates/verbatim-listener-logic/src/schedule/generated.rs:\n        vec![{}],\n\
             the found sequences alone: cargo test -p verbatim-listener-logic --lib -- --exact schedule::generated::found_grace_sequences_keep_the_graces_rules",
            steps.join(", ")
        );
        std::panic::resume_unwind(payload);
    }
}

/// The grace sequences the enumeration failed on, each kept as its list of
/// steps, as the failure's report gives it.
fn found_graces() -> Vec<Vec<Grace>> {
    Vec::new()
}

/// Every found grace sequence keeps the grace's rules through its drain.
#[test]
fn found_grace_sequences_keep_the_graces_rules() {
    for sequence in found_graces() {
        let mut run = GraceRun::default();
        for step in sequence {
            run.step(step);
        }
        run.drain();
    }
}

/// Every sequence of up to six steps of the listener's timer (menu ends,
/// the desktop's returns, desktop names sent, the timer going off as set
/// or early, idle rounds, the close and time) keeps its rules against a
/// reference model that keeps each waiting thing's time on its own: each
/// is sent once, only once its time is over, the newest end alone, those
/// a firing sends in the order of their times and none due left unsent;
/// the timer is set for the soonest thing waiting; no round is answered
/// while anything waits, none twice and none held with nothing waiting;
/// and once drained every round has its answer and the newest end was
/// sent, unless the timer was closed.
#[test]
fn every_short_grace_sequence_keeps_the_graces_rules() {
    // Each depth in turn, so the sequence a failure prints is one of the
    // shortest that fail.
    for depth in 1..=GRACE_DEPTH {
        every_grace_sequence(&GraceRun::default(), depth);
    }
}
