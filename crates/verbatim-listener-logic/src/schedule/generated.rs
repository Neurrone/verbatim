//! The read scheduler's generated test (`books/dev/src/reference/crates/verbatim-listener-logic.md`): lists of actions, each made valid when it is
//! applied by resolving its target against the model, given to
//! [`Schedule::step`] as the listener's shell gives them: every effect
//! applied in order, then each failed spawn stepped, and then the step's
//! asks answered, each as a step of its own. Each effect is checked as
//! permitted by what the model holds, and after every action the
//! invariants the design lists are checked. Each case ends by draining:
//! every read returned, every waiting event read, every alarm reached;
//! then every marker has had exactly one outcome and every idle waiter has
//! run once.
//!
//! The model is lists and sets: per application, the markers waiting in
//! push order with their kind and key, its read under way, the thread it
//! runs, whether a thread is in charge, its abandoned threads, whether its
//! outpost ended, and the requests given; globally, each
//! marker's place and outcomes, the idle waiters and the alarm as the
//! schedule last set it. It predicts no replacement, drop, place or alarm
//! time: it follows the outcomes the schedule emits and checks each was
//! permitted, a deadline abandonment at or after its deadline, a moved-on
//! one after an answer of true, no read surviving a tick at or past its
//! deadline, and the alarm never later than a read's next wake and later
//! than each tick. Which place an event is given, its moving away, the
//! bound of a wait at the abandoned limit and whether a focus is pending
//! are the schedule's own tests' examples.
//!
//! The menu end's grace is tested by every sequence of up to six steps,
//! below ([`every_short_grace_sequence_keeps_the_graces_rules`]).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestError, TestRunner};
use verbatim_model::TraceId;

use super::*;

/// The read deadline the schedule is made with, in microseconds: short
/// enough that the generated time passes it.
const DEADLINE: u64 = 2_000_000;

/// The moved-on grace, in microseconds.
const GRACE: u64 = 500_000;

/// The applications' processes.
const PIDS: [u32; 3] = [1, 2, 3];

/// The top-level window numbered `index`.
fn window(index: u8) -> Window {
    Window::new(0x10 * (isize::from(index % 2) + 1)).expect("never 0")
}

/// One step of a case.
#[derive(Clone, Copy, Debug)]
enum Action {
    /// An event of application `app` arrives: a focus (0), a menu opening
    /// (1), a selection (2) or a notification (3), of element `key`, a
    /// notification's none, its window unknown (0) or known (1, 2), a
    /// selection's and a notification's never judged.
    Push {
        app: u8,
        kind: u8,
        key: Option<u8>,
        window: u8,
    },
    /// The application's thread, waiting with no read under way, asks for
    /// its next event.
    Next { app: u8 },
    /// A read of the application returns: the read of its thread in
    /// charge, or the oldest of its abandoned threads', read or dropped,
    /// with a top-level window (1, 2) or none (0).
    Return {
        app: u8,
        stray: bool,
        read: bool,
        top: u8,
    },
    /// The application's read under way, or the request before it, learns
    /// its window.
    Learned { app: u8, stale: bool, window: u8 },
    /// The application's read under way, or the request before it, is
    /// armed, its bound this many milliseconds ahead.
    Armed { app: u8, stale: bool, after: u16 },
    /// Time passes and the watchdog ticks.
    Advance(Advance),
    /// A foreground event of a top-level window (1, 2) or none (0), each
    /// application's read under way being of the top-level window its
    /// entry in `tops` names.
    Foreground { top: u8, tops: [u8; 3] },
    /// An MSAA focus of the application's is forwarded.
    MsaaFocus { app: u8 },
    /// The application's outpost ends.
    OutpostEnded { app: u8 },
    /// An idle waiter is queued.
    WhenIdle,
    /// The listener shuts down.
    Stop,
}

/// How far time moves before a tick.
#[derive(Clone, Copy, Debug)]
enum Advance {
    /// To the watchdog's timer, if one is set, and this many
    /// milliseconds past it.
    ToTimer(u8),
    /// This many milliseconds.
    By(u16),
    /// Past every deadline and bound at once.
    Late,
}

/// The shell's answers to the asks and spawns an action causes.
#[derive(Clone, Copy, Debug)]
struct Asks {
    /// Whether the user has moved on from a read's window.
    moved_on: bool,
    /// Whether a read's progress was there to publish.
    publish: bool,
    /// Whether a new thread fails to start.
    spawn_fails: bool,
}

fn action() -> impl Strategy<Value = Action> {
    use proptest::prelude::*;
    let app = 0..3u8;
    let advance = prop_oneof![
        3 => (0..3u8).prop_map(Advance::ToTimer),
        2 => (0..700u16).prop_map(Advance::By),
        1 => Just(Advance::Late),
    ];
    // Weighted toward the lifecycle: events that start reads, the reads'
    // returns, abandonments by time, and outposts' ends.
    prop_oneof![
        10 => (app.clone(), 0..4u8, prop::option::weighted(0.6, 0..2u8), 0..3u8)
            .prop_map(|(app, kind, key, window)| Action::Push { app, kind, key, window }),
        8 => app.clone().prop_map(|app| Action::Next { app }),
        6 => (app.clone(), prop::bool::weighted(0.4), any::<bool>(), 0..3u8)
            .prop_map(|(app, stray, read, top)| Action::Return { app, stray, read, top }),
        2 => (app.clone(), prop::bool::weighted(0.2), 1..3u8)
            .prop_map(|(app, stale, window)| Action::Learned { app, stale, window }),
        2 => (app.clone(), prop::bool::weighted(0.2), 0..900u16)
            .prop_map(|(app, stale, after)| Action::Armed { app, stale, after }),
        6 => advance.prop_map(Action::Advance),
        2 => (0..3u8, [0..3u8, 0..3u8, 0..3u8])
            .prop_map(|(top, tops)| Action::Foreground { top, tops }),
        1 => app.clone().prop_map(|app| Action::MsaaFocus { app }),
        2 => app.prop_map(|app| Action::OutpostEnded { app }),
        2 => Just(Action::WhenIdle),
        1 => Just(Action::Stop),
    ]
}

fn asks() -> impl Strategy<Value = Asks> {
    use proptest::prelude::*;
    (
        prop::bool::weighted(0.4),
        prop::bool::weighted(0.6),
        prop::bool::weighted(0.1),
    )
        .prop_map(|(moved_on, publish, spawn_fails)| Asks {
            moved_on,
            publish,
            spawn_fails,
        })
}

/// An event waiting for its read, as the model holds it.
#[derive(Clone, Debug)]
struct Waiting {
    marker: u64,
    kind: FocusEvent,
    key: Option<u8>,
    window: RuleWindow,
}

/// A read under way, as the model holds it.
#[derive(Clone, Debug)]
struct Read {
    marker: u64,
    kind: FocusEvent,
    request: Request,
    start: u64,
    window: RuleWindow,
    armed: Option<u64>,
    published: bool,
}

/// A thread abandoned in its read, which has not returned.
#[derive(Clone, Copy, Debug)]
struct Stray {
    request: Request,
    marker: u64,
    kind: FocusEvent,
}

/// One application, as the model holds it.
#[derive(Clone, Debug, Default)]
struct App {
    queue: Vec<Waiting>,
    /// Whether a thread is in charge, as the schedule says it.
    threaded: bool,
    /// The generation of the last spawn.
    generation: u64,
    /// The thread of `generation`, while it runs.
    thread: Option<u64>,
    /// Its read, while one is under way.
    read: Option<Read>,
    strays: Vec<Stray>,
    ending: bool,
    /// The highest generation spawned in its life.
    highest: u64,
    /// The requests its jobs were given in its life.
    requests: BTreeSet<Request>,
}

impl App {
    /// Whether the application's outpost has ended and it has nothing
    /// left: no thread in charge, no abandoned thread, no event.
    fn spent(&self) -> bool {
        self.ending
            && !self.threaded
            && self.strays.is_empty()
            && self.queue.is_empty()
            && self.read.is_none()
    }
}

/// The reference model.
#[derive(Debug, Default)]
struct Model {
    apps: BTreeMap<u32, App>,
    stopping: bool,
    queued: u64,
    /// The idle waiters, each with the place counter when it came.
    waiters: Vec<(u64, u64)>,
    next_waiter: u64,
    next_marker: u64,
    /// Each event's place, by marker.
    places: BTreeMap<u64, u64>,
    /// How many outcomes each marker has had.
    outcomes: BTreeMap<u64, u32>,
    /// The markers pushed after the stop.
    after_stop: BTreeSet<u64>,
    /// How many times each idle waiter has run.
    ran: BTreeMap<u64, u32>,
    /// The time the watchdog's timer is set to.
    timer: Option<u64>,
    now: u64,
}

impl Model {
    /// Whether waiter `before` may run: every event queued at or before
    /// place `before` has had its outcome.
    fn idle(&self, before: u64) -> bool {
        self.places
            .iter()
            .all(|(marker, place)| *place > before || self.outcomes.contains_key(marker))
    }

    /// The markers whose outcome is owed: those waiting, and those read
    /// and not published.
    fn open(&self) -> Vec<u64> {
        let mut open: Vec<u64> = self
            .apps
            .values()
            .flat_map(|app| {
                app.queue.iter().map(|queued| queued.marker).chain(
                    app.read
                        .iter()
                        .filter(|read| !read.published)
                        .map(|read| read.marker),
                )
            })
            .collect();
        open.sort_unstable();
        open
    }
}

/// What an outermost step was given, for the checks of its effects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Given {
    Push { pid: u32, marker: u64 },
    Next(u32),
    Returned { stray: bool },
    Tick,
    Learned,
    Armed,
    MovedOn { moved_on: bool },
    Published { then_abandon: Option<Reason> },
    Foreground,
    MsaaFocus,
    OutpostEnded(u32),
    WhenIdle,
    SpawnFailed(u32),
    Stop,
}

/// What one step of the schedule's effects showed, for the checks at its
/// end.
#[derive(Debug, Default)]
struct Seen {
    asks: BTreeMap<u32, u32>,
    /// The reads the step abandoned: each one's application, marker and
    /// whether it was published, and whether its outcome came.
    abandoned: Vec<(u32, u64, bool, bool)>,
    /// The read the step's return ended, whether it was published, and
    /// whether its outcome came.
    returned: Option<(u64, bool, bool)>,
    forgets: u32,
}

/// The listed states a gate's cases must reach.
const LISTED: [&str; 27] = [
    "a job given",
    "a job after another application's focus",
    "an event replaced",
    "a spawn",
    "a spawn failed",
    "a thread reused",
    "an abandonment at the deadline",
    "an abandonment for moving on",
    "a read published at its bound",
    "a published read abandoned",
    "a published read returned",
    "an abandoned read returned",
    "the abandoned limit reached",
    "an event waiting at the limit",
    "events dropped at their bound",
    "events dropped as one arrives",
    "an idle waiter run",
    "an idle waiter run at a later step",
    "an application removed at its outpost's end",
    "an application forgotten",
    "an ended application read on",
    "a stale input",
    "a moved-on ask",
    "a foreground event moving away",
    "an outcome after the stop",
    "a push after the stop",
    "an empty queue at a thread's ask",
];

/// One case's run: the schedule and the model side by side.
struct Run {
    schedule: Schedule<u64>,
    model: Model,
    trail: Vec<String>,
    /// Each step's effects, for the run twice.
    log: Vec<String>,
    reached: BTreeMap<&'static str, u32>,
    /// The read a thread in charge returns, for its step: its marker and
    /// whether it was published.
    returning: Option<(u64, bool)>,
    /// The applications the model held as the action began.
    held_before: Vec<u32>,
    /// The applications the action's steps let go
    /// ([`ScheduleEffect::Forget`]), in order.
    forgotten: Vec<u32>,
}

/// The meta of event `marker` of `kind`.
fn meta(marker: u64, kind: FocusEvent) -> Meta {
    Meta {
        marker,
        kind,
        trace: TraceId::from_raw(marker),
        timing: verbatim_model::protocol::EventTiming::default(),
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

/// The reason an outcome gives, or `"read"` for a read.
fn reason(judged: &Judged) -> &'static str {
    match judged {
        Judged::Read(_) => "read",
        Judged::Dropped(reason) => reason,
        Judged::Passed(_) => "passed",
    }
}

impl Run {
    fn new() -> Self {
        Self {
            schedule: Schedule::new(Duration::from_micros(DEADLINE)),
            model: Model::default(),
            trail: Vec::new(),
            log: Vec::new(),
            reached: BTreeMap::new(),
            returning: None,
            held_before: Vec::new(),
            forgotten: Vec::new(),
        }
    }

    /// Runs `actions`, then drains.
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

    /// Applies `action`, resolved against the model, with `asks`.
    #[expect(clippy::too_many_lines, reason = "one arm for each action")]
    fn act(&mut self, action: Action, asks: Asks) {
        self.trail.push(format!("{action:?} {asks:?}"));
        self.held_before = self.model.apps.keys().copied().collect();
        self.forgotten.clear();
        let pid_of = |app: u8| PIDS[usize::from(app)];
        match action {
            Action::Push {
                app,
                kind,
                key,
                window: index,
            } => {
                let pid = pid_of(app);
                let kind = match kind {
                    0 => FocusEvent::Focus,
                    1 => FocusEvent::MenuOpened,
                    2 => FocusEvent::Selection,
                    _ => FocusEvent::Notification,
                };
                // A notification has no key; a selection and a notification
                // are never judged by the moved-on rule.
                let key = key.filter(|_| kind != FocusEvent::Notification);
                let rule = match (kind, index) {
                    (FocusEvent::Selection | FocusEvent::Notification, _) => RuleWindow::Never,
                    (_, 0) => RuleWindow::Unknown,
                    (_, index) => RuleWindow::Known(window(index)),
                };
                self.model.next_marker += 1;
                let marker = self.model.next_marker;
                self.pushed(pid, marker, kind, key, rule);
                self.shell(
                    ScheduleInput::Push {
                        pid: Pid(pid),
                        meta: meta(marker, kind),
                        job: marker,
                        kind,
                        key: key.map(|key| vec![i32::from(key)]),
                        window: rule,
                    },
                    Given::Push { pid, marker },
                    asks,
                );
            }
            Action::Next { app } => {
                let pid = pid_of(app);
                let Some(generation) = self
                    .model
                    .apps
                    .get(&pid)
                    .filter(|app| app.read.is_none())
                    .and_then(|app| app.thread)
                else {
                    return;
                };
                self.shell(
                    ScheduleInput::Next {
                        pid: Pid(pid),
                        generation,
                    },
                    Given::Next(pid),
                    asks,
                );
                self.next_asked(pid);
            }
            Action::Return {
                app,
                stray,
                read,
                top,
            } => {
                let pid = pid_of(app);
                let Some(model) = self.model.apps.get(&pid) else {
                    return;
                };
                let (request, marker, kind) = if stray {
                    let Some(stray) = model.strays.first() else {
                        return;
                    };
                    (stray.request, stray.marker, stray.kind)
                } else {
                    let Some(read) = model.read.as_ref() else {
                        return;
                    };
                    (read.request, read.marker, read.kind)
                };
                let top = (top > 0).then(|| window(top));
                self.returning(pid, stray);
                self.shell(
                    ScheduleInput::Returned {
                        pid: Pid(pid),
                        request,
                        meta: meta(marker, kind),
                        judged: if read {
                            Judged::Read(a_read())
                        } else {
                            Judged::Dropped("its element could not be resolved")
                        },
                        top,
                    },
                    Given::Returned { stray },
                    asks,
                );
                self.thread_returned(pid, stray);
            }
            Action::Learned {
                app,
                stale,
                window: index,
            } => {
                let pid = pid_of(app);
                let Some(request) = self.target(pid, stale) else {
                    return;
                };
                let learned = window(index);
                if !self.model.stopping
                    && let Some(read) = self.read_of(pid, request)
                    && read.window == RuleWindow::Unknown
                {
                    read.window = RuleWindow::Known(learned);
                }
                self.shell(
                    ScheduleInput::Learned {
                        pid: Pid(pid),
                        request,
                        window: learned,
                    },
                    Given::Learned,
                    asks,
                );
            }
            Action::Armed { app, stale, after } => {
                let pid = pid_of(app);
                let Some(request) = self.target(pid, stale) else {
                    return;
                };
                let bound = self.model.now + u64::from(after) * 1_000;
                if let Some(read) = self.read_of(pid, request)
                    && !read.published
                {
                    read.armed = Some(bound);
                }
                self.shell(
                    ScheduleInput::Armed {
                        pid: Pid(pid),
                        request,
                        bound,
                    },
                    Given::Armed,
                    asks,
                );
            }
            Action::Advance(advance) => {
                let now = self.model.now;
                self.model.now = match advance {
                    Advance::ToTimer(past) => self
                        .model
                        .timer
                        .map_or(now, |timer| timer.max(now) + u64::from(past) * 1_000),
                    Advance::By(ms) => now + u64::from(ms) * 1_000,
                    Advance::Late => now + 3 * DEADLINE,
                };
                self.tick(asks);
            }
            Action::Foreground { top, tops } => {
                let top_level = (top > 0).then(|| window(top));
                // The applications with reads under way, exactly, each
                // with its read's top-level window as far as it is known.
                let running_tops: Vec<(Pid, Option<Window>)> = self
                    .model
                    .apps
                    .iter()
                    .filter(|(_, app)| app.read.is_some())
                    .map(|(pid, _)| {
                        let at = PIDS.iter().position(|each| each == pid).unwrap_or(0);
                        (Pid(*pid), (tops[at] > 0).then(|| window(tops[at])))
                    })
                    .collect();
                let left_at = |schedule: &Schedule<u64>| -> Vec<u64> {
                    schedule.apps.values().map(|app| app.left_at).collect()
                };
                let before = left_at(&self.schedule);
                self.shell(
                    ScheduleInput::Foreground {
                        top_level,
                        running_tops,
                    },
                    Given::Foreground,
                    asks,
                );
                if left_at(&self.schedule) != before {
                    self.reach("a foreground event moving away");
                }
            }
            Action::MsaaFocus { app } => {
                let pid = pid_of(app);
                self.shell(
                    ScheduleInput::MsaaFocus { pid: Pid(pid) },
                    Given::MsaaFocus,
                    asks,
                );
            }
            Action::OutpostEnded { app } => {
                let pid = pid_of(app);
                if let Some(model) = self.model.apps.get_mut(&pid) {
                    if !model.threaded && model.strays.is_empty() && model.queue.is_empty() {
                        self.model.apps.remove(&pid);
                        self.reach("an application removed at its outpost's end");
                    } else {
                        model.ending = true;
                    }
                }
                self.shell(
                    ScheduleInput::OutpostEnded { pid: Pid(pid) },
                    Given::OutpostEnded(pid),
                    asks,
                );
            }
            Action::WhenIdle => {
                // The shell steps no waiter once the readers stop.
                if self.model.stopping {
                    return;
                }
                self.model.next_waiter += 1;
                let waiter = self.model.next_waiter;
                self.model.waiters.push((self.model.queued, waiter));
                self.shell(ScheduleInput::WhenIdle { waiter }, Given::WhenIdle, asks);
                if self.model.waiters.iter().any(|(_, each)| *each == waiter) {
                    self.reach("an idle waiter waits");
                }
            }
            Action::Stop => {
                if self.model.stopping {
                    return;
                }
                self.model.stopping = true;
                self.shell(ScheduleInput::Stop, Given::Stop, asks);
            }
        }
        self.settle();
        // The shell forgets exactly the applications the action let go,
        // each once.
        let mut forgotten = self.forgotten.clone();
        forgotten.sort_unstable();
        let gone: Vec<u32> = self
            .held_before
            .iter()
            .copied()
            .filter(|pid| !self.model.apps.contains_key(pid))
            .collect();
        assert_eq!(
            forgotten, gone,
            "the applications forgotten: {:?}",
            self.trail
        );
        if !gone.is_empty() {
            self.reach("an application forgotten");
        }
        self.compare();
    }

    /// The model's part of a push of event `marker`, before its step.
    fn pushed(
        &mut self,
        pid: u32,
        marker: u64,
        kind: FocusEvent,
        key: Option<u8>,
        rule: RuleWindow,
    ) {
        let model = &mut self.model;
        if model.stopping {
            model.after_stop.insert(marker);
            self.reach("a push after the stop");
            return;
        }
        model.queued += 1;
        let place = model.queued;
        model.places.insert(marker, place);
        let app = model.apps.entry(pid).or_default();
        app.ending = false;
        app.queue.push(Waiting {
            marker,
            kind,
            key,
            window: rule,
        });
    }

    /// The request an input of `pid`'s names: its read's, or, `stale`, the
    /// newest before it.
    fn target(&mut self, pid: u32, stale: bool) -> Option<Request> {
        let app = self.model.apps.get(&pid)?;
        if stale {
            let current = app.read.as_ref().map(|read| read.request);
            let stale = app
                .requests
                .iter()
                .rev()
                .copied()
                .find(|request| Some(*request) != current)?;
            self.reach("a stale input");
            Some(stale)
        } else {
            // Only a read judged by the moved-on rule is told its window
            // or its bound.
            app.read
                .as_ref()
                .filter(|read| read.window != RuleWindow::Never)
                .map(|read| read.request)
        }
    }

    /// `pid`'s read under way, if it is `request`'s.
    fn read_of(&mut self, pid: u32, request: Request) -> Option<&mut Read> {
        self.model
            .apps
            .get_mut(&pid)?
            .read
            .as_mut()
            .filter(|read| read.request == request)
    }

    /// The model's part of a return, before its step: the read leaves its
    /// thread, or an abandoned thread leaves the list.
    fn returning(&mut self, pid: u32, stray: bool) {
        let Some(app) = self.model.apps.get_mut(&pid) else {
            return;
        };
        if stray {
            app.strays.remove(0);
        } else if let Some(read) = app.read.take() {
            self.returning = Some((read.marker, read.published));
        }
    }

    /// After a thread in charge's return: it reads on, or ends.
    fn thread_returned(&mut self, pid: u32, stray: bool) {
        let stopping = self.model.stopping;
        if let Some(app) = self.model.apps.get_mut(&pid)
            && !stray
            && (stopping || !app.threaded)
        {
            app.thread = None;
        }
    }

    /// After a thread's ask for its next event: with nothing waiting, an
    /// ended application's thread ends, and a thread not in charge ends.
    fn next_asked(&mut self, pid: u32) {
        let stopping = self.model.stopping;
        let Some(app) = self.model.apps.get_mut(&pid) else {
            return;
        };
        if app.read.is_none() && !stopping && app.queue.is_empty() {
            *self
                .reached
                .entry("an empty queue at a thread's ask")
                .or_default() += 1;
            if app.ending {
                app.threaded = false;
            }
        }
        if stopping || !app.threaded {
            app.thread = None;
        }
    }

    /// The watchdog's tick at the model's time.
    fn tick(&mut self, asks: Asks) {
        // The tick spends the timer.
        self.model.timer = None;
        self.shell(ScheduleInput::Tick, Given::Tick, asks);
        let now = self.model.now;
        if !self.model.stopping {
            for (pid, app) in &self.model.apps {
                if let Some(read) = &app.read {
                    assert!(
                        read.start + DEADLINE > now,
                        "{pid}'s read survived a tick past its deadline: {:?}",
                        self.trail
                    );
                }
            }
            // The tick acted on everything due by now, so the alarm is
            // left unset or later: never a wake it will not act on.
            assert!(
                self.model.timer.is_none_or(|due| due > now),
                "the alarm at {:?} after a tick at {now}: {:?}",
                self.model.timer,
                self.trail
            );
        }
    }

    /// Steps the schedule with `input` as the shell does: each effect
    /// applied in order, then each failed spawn stepped, then each
    /// ask answered as a step of its own.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm for each effect the shell handles itself"
    )]
    fn shell(&mut self, input: ScheduleInput<u64>, given: Given, asks: Asks) {
        let now = self.model.now;
        let stopping_before = self.model.stopping && given != Given::Stop;
        let effects = self.schedule.step(input, now);
        self.log.push(format!("{effects:?}"));
        self.postconditions(given, &effects);
        let mut seen = Seen {
            returned: self
                .returning
                .take()
                .map(|(marker, published)| (marker, published, false)),
            ..Seen::default()
        };
        let mut answers = Vec::new();
        let mut failed = Vec::new();
        let timer_before = self.model.timer;
        for effect in effects {
            if stopping_before {
                assert!(
                    matches!(
                        effect,
                        ScheduleEffect::Outcome { .. }
                            | ScheduleEffect::Wake(_)
                            | ScheduleEffect::ForgetLast
                            | ScheduleEffect::Forget { .. }
                            | ScheduleEffect::RunIdle(_)
                    ),
                    "after the stop: {effect:?}, {:?}",
                    self.trail
                );
            }
            match effect {
                ScheduleEffect::AskMovedOn {
                    pid,
                    request,
                    window,
                } => {
                    self.asked(&mut seen, pid.0, request);
                    let read = self.model.apps[&pid.0].read.as_ref().expect("asked");
                    assert_eq!(read.window, RuleWindow::Known(window), "{:?}", self.trail);
                    assert!(now >= read.start + GRACE, "{:?}", self.trail);
                    self.reach("a moved-on ask");
                    answers.push((
                        ScheduleInput::MovedOn {
                            pid,
                            request,
                            moved_on: asks.moved_on,
                        },
                        Given::MovedOn {
                            moved_on: asks.moved_on,
                        },
                    ));
                }
                ScheduleEffect::Publish {
                    pid,
                    request,
                    then_abandon,
                } => {
                    self.asked(&mut seen, pid.0, request);
                    let trail = format!("{:?}", self.trail);
                    let read = self
                        .model
                        .apps
                        .get_mut(&pid.0)
                        .and_then(|app| app.read.as_mut())
                        .expect("asked");
                    assert!(!read.published, "{trail}");
                    let armed = read.armed.take();
                    match then_abandon {
                        None => assert!(armed.is_some_and(|bound| bound <= now), "{trail}"),
                        Some(Reason::Deadline) => {
                            assert!(armed.is_some() && now >= read.start + DEADLINE, "{trail}");
                        }
                        Some(Reason::MovedOn) => assert!(
                            armed.is_some() && given == Given::MovedOn { moved_on: true },
                            "{trail}"
                        ),
                        Some(Reason::Hung) => panic!("a read was judged hung: {trail}"),
                    }
                    answers.push((
                        ScheduleInput::Published {
                            pid,
                            request,
                            read: asks.publish.then(a_read),
                            then_abandon,
                        },
                        Given::Published { then_abandon },
                    ));
                }
                ScheduleEffect::Spawn {
                    pid,
                    generation,
                    reuse,
                } => {
                    let fails = !reuse && asks.spawn_fails;
                    self.spawned(given, pid.0, generation, reuse, fails);
                    if fails {
                        failed.push((pid, generation));
                    }
                }
                ScheduleEffect::Alarm(change) => match change {
                    AlarmChange::Set(due) => {
                        assert_ne!(
                            due, timer_before,
                            "an alarm set to what it was: {:?}",
                            self.trail
                        );
                        self.model.timer = due;
                    }
                    AlarmChange::Now => {
                        assert_eq!(given, Given::Stop, "{:?}", self.trail);
                    }
                },
                effect => self.apply(given, &mut seen, effect),
            }
        }
        // An abandoned read's outcome comes unless it was published; a
        // returned read's unless it was published.
        for (pid, marker, published, outcome) in &seen.abandoned {
            assert_eq!(
                *outcome, !published,
                "{pid}'s abandoned read {marker}: {:?}",
                self.trail
            );
        }
        if let Some((marker, published, outcome)) = seen.returned {
            assert_eq!(
                outcome, !published,
                "returned read {marker}: {:?}",
                self.trail
            );
            if published {
                self.reach("a published read returned");
            }
        }
        // One forget-last per return from an abandoned thread.
        let stray = matches!(given, Given::Returned { stray: true });
        assert_eq!(seen.forgets, u32::from(stray), "{:?}", self.trail);
        // A failed spawn is stepped once the step's other effects are
        // applied, ahead of its answers.
        for (pid, generation) in failed {
            // No thread is in charge, and its events are dropped.
            if let Some(app) = self.model.apps.get_mut(&pid.0) {
                app.threaded = false;
            }
            self.reach("a spawn failed");
            self.shell(
                ScheduleInput::SpawnFailed { pid, generation },
                Given::SpawnFailed(pid.0),
                asks,
            );
        }
        for (answer, given) in answers {
            self.shell(answer, given, asks);
        }
    }

    /// Checks the exact effects of the inputs whose effects the model
    /// decides alone.
    fn postconditions(&mut self, given: Given, effects: &[ScheduleEffect<u64>]) {
        let trail = &self.trail;
        let kinds: Vec<String> = effects.iter().map(effect_name).collect();
        match given {
            Given::Push { marker, .. } if self.model.after_stop.contains(&marker) => {
                assert_eq!(
                    kinds,
                    [format!("outcome {marker} {SHUTTING_DOWN}")],
                    "{trail:?}"
                );
            }
            Given::Push { pid, .. } if self.model.apps[&pid].threaded => {
                // A push to an application with a thread in charge wakes it.
                assert!(kinds.contains(&format!("wake {pid}")), "{trail:?}");
            }
            Given::Stop => assert_eq!(kinds, ["wake all", "alarm now"], "{trail:?}"),
            Given::Foreground | Given::MsaaFocus => {
                assert_eq!(kinds, Vec::<String>::new(), "{trail:?}");
            }
            Given::Armed => assert!(
                kinds.iter().all(|kind| kind.starts_with("alarm ")),
                "{trail:?}"
            ),
            Given::OutpostEnded(pid) => {
                let expected: Vec<String> = if self.model.apps.contains_key(&pid) {
                    vec![format!("wake {pid}")]
                } else if self.held_before.contains(&pid) {
                    vec![format!("forget {pid}")]
                } else {
                    Vec::new()
                };
                assert_eq!(kinds, expected, "{trail:?}");
            }
            Given::SpawnFailed(pid) => {
                // Its waiting events dropped in order, then the waiters
                // they held run, then the alarm.
                let queue: Vec<String> = self.model.apps[&pid]
                    .queue
                    .iter()
                    .map(|queued| format!("outcome {} {NO_THREAD}", queued.marker))
                    .collect();
                assert_eq!(
                    &kinds[..queue.len().min(kinds.len())],
                    &queue[..],
                    "{trail:?}"
                );
            }
            _ => {}
        }
        // Nothing of an application follows its forgetting in the step.
        for (at, effect) in effects.iter().enumerate() {
            if let ScheduleEffect::Forget { pid } = effect {
                assert!(
                    effects[at + 1..]
                        .iter()
                        .all(|later| effect_pid(later) != Some(*pid)),
                    "an effect of {pid} after its forgetting: {kinds:?} {trail:?}"
                );
            }
        }
    }

    /// The model's check of a step's ask of `pid`'s read `request`.
    fn asked(&mut self, seen: &mut Seen, pid: u32, request: Request) {
        let count = seen.asks.entry(pid).or_default();
        *count += 1;
        assert_eq!(
            *count, 1,
            "a second ask of {pid} in a step: {:?}",
            self.trail
        );
        assert!(
            !self.model.stopping,
            "an ask after the stop: {:?}",
            self.trail
        );
        assert_eq!(
            self.model.apps[&pid].read.as_ref().map(|read| read.request),
            Some(request),
            "an ask of a request not under way: {:?}",
            self.trail
        );
    }

    /// The model's part of a spawn.
    fn spawned(&mut self, given: Given, pid: u32, generation: u64, reuse: bool, fails: bool) {
        let trail = format!("{:?}", self.trail);
        assert!(!self.model.stopping, "a spawn after the stop: {trail}");
        let app = self
            .model
            .apps
            .get_mut(&pid)
            .expect("a spawn of an application held");
        assert!(
            !app.threaded,
            "a spawn while a thread is in charge: {trail}"
        );
        assert!(
            generation > app.highest,
            "a spawn's generation {generation} not above {}: {trail}",
            app.highest
        );
        assert!(
            app.strays.len() < ABANDONED_LIMIT,
            "a new thread while {} are abandoned: {trail}",
            app.strays.len()
        );
        assert!(
            !app.queue.is_empty(),
            "a spawn with nothing waiting: {trail}"
        );
        assert_eq!(
            reuse,
            matches!(given, Given::Returned { stray: true }),
            "a thread reused only as an abandoned one returns: {trail}"
        );
        assert!(
            app.thread.is_none(),
            "a spawn beside a running thread: {trail}"
        );
        app.threaded = true;
        app.generation = generation;
        app.highest = generation;
        app.thread = (!fails).then_some(generation);
        self.reach(if reuse { "a thread reused" } else { "a spawn" });
    }

    /// Carries out `effect` in the model, checking it.
    #[expect(clippy::too_many_lines, reason = "one arm for each kind of effect")]
    fn apply(&mut self, given: Given, seen: &mut Seen, effect: ScheduleEffect<u64>) {
        let trail = format!("{:?}", self.trail);
        let now = self.model.now;
        match effect {
            ScheduleEffect::Outcome { pid, meta, judged } => {
                let (pid, marker, why) = (pid.0, meta.marker, reason(&judged));
                let count = self.model.outcomes.entry(marker).or_default();
                *count += 1;
                assert_eq!(*count, 1, "a second outcome for {marker}: {trail}");
                if self.model.after_stop.contains(&marker) {
                    assert!(
                        why == SHUTTING_DOWN
                            && matches!(given, Given::Push { marker: pushed, .. } if pushed == marker),
                        "{trail}"
                    );
                    return;
                }
                if self.model.stopping && given != Given::Stop {
                    self.reach("an outcome after the stop");
                }
                if let Some((_, _, published, outcome)) = seen
                    .abandoned
                    .iter_mut()
                    .find(|(owner, abandoned, ..)| *owner == pid && *abandoned == marker)
                {
                    assert!(!*published, "a published read dropped: {trail}");
                    *outcome = true;
                    match why {
                        NOT_IN_TIME => self.reach("an abandonment at the deadline"),
                        MOVED_ON => {
                            assert!(
                                matches!(
                                    given,
                                    Given::MovedOn { moved_on: true }
                                        | Given::Published {
                                            then_abandon: Some(Reason::MovedOn)
                                        }
                                ),
                                "a moved-on abandonment with no answer of true: {trail}"
                            );
                            self.reach("an abandonment for moving on");
                        }
                        other => panic!("an abandoned read dropped for {other}: {trail}"),
                    }
                    return;
                }
                if let Some((returned, published, outcome)) = seen.returned.as_mut()
                    && *returned == marker
                {
                    assert!(!*published, "a published read's second outcome: {trail}");
                    *outcome = true;
                    return;
                }
                let app = self.model.apps.get_mut(&pid);
                if let Some(app) = app {
                    if let Some(read) = app.read.as_mut().filter(|read| read.marker == marker) {
                        // A publication from the read's progress.
                        assert!(
                            matches!(given, Given::Published { .. }) && why == "read",
                            "{trail}"
                        );
                        assert!(
                            !read.published,
                            "a published read's second outcome: {trail}"
                        );
                        read.published = true;
                        self.reach("a read published at its bound");
                        return;
                    }
                    let Some(at) = app.queue.iter().position(|queued| queued.marker == marker)
                    else {
                        panic!("an outcome for {marker}, not open: {trail}");
                    };
                    let queued = app.queue.remove(at);
                    match why {
                        REPLACED => {
                            let Given::Push { marker: pushed, .. } = given else {
                                panic!("a replacement but by a push: {trail}");
                            };
                            let new = app
                                .queue
                                .iter()
                                .find(|each| each.marker == pushed)
                                .expect("the new event queued");
                            assert!(
                                new.kind == queued.kind
                                    && new.key.is_some()
                                    && new.key == queued.key,
                                "a replacement of another element or kind: {trail}"
                            );
                            self.reach("an event replaced");
                        }
                        WAITED_TOO_LONG => {
                            assert!(
                                !app.threaded && app.strays.len() >= ABANDONED_LIMIT,
                                "an event dropped while not at the abandoned limit: {trail}"
                            );
                            match given {
                                Given::Tick => self.reach("events dropped at their bound"),
                                Given::Push { .. } => self.reach("events dropped as one arrives"),
                                _ => panic!("a drop at the bound but at a tick or a push: {trail}"),
                            }
                        }
                        NO_THREAD => {
                            assert_eq!(given, Given::SpawnFailed(pid), "{trail}");
                        }
                        other => panic!("a waiting event dropped for {other}: {trail}"),
                    }
                } else {
                    panic!("an outcome for {marker} of an application not held: {trail}");
                }
            }
            ScheduleEffect::Wake(Waking::Thread(pid)) => {
                assert!(
                    given == Given::OutpostEnded(pid.0)
                        || matches!(given, Given::Push { pid: pushed, .. } if pushed == pid.0),
                    "{trail}"
                );
            }
            ScheduleEffect::Wake(Waking::All) => assert_eq!(given, Given::Stop, "{trail}"),
            ScheduleEffect::Unwatch { pid } => {
                let pid = pid.0;
                assert!(
                    matches!(
                        given,
                        Given::Tick
                            | Given::Learned
                            | Given::MovedOn { moved_on: true }
                            | Given::Published {
                                then_abandon: Some(_)
                            }
                    ),
                    "an abandonment of what no judgement decided: {trail}"
                );
                let app = self.model.apps.get_mut(&pid).expect("an application held");
                let read = app.read.take().expect("an abandonment of a read under way");
                if matches!(
                    given,
                    Given::Tick
                        | Given::Learned
                        | Given::Published {
                            then_abandon: Some(Reason::Deadline)
                        }
                ) {
                    assert!(
                        now >= read.start + DEADLINE,
                        "a deadline abandonment before its deadline: {trail}"
                    );
                }
                app.strays.push(Stray {
                    request: read.request,
                    marker: read.marker,
                    kind: read.kind,
                });
                app.thread = None;
                app.threaded = false;
                if app.strays.len() == ABANDONED_LIMIT {
                    self.reach("the abandoned limit reached");
                }
                if read.published {
                    self.reach("a published read abandoned");
                }
                seen.abandoned
                    .push((pid, read.marker, read.published, false));
            }
            ScheduleEffect::ForgetLast => {
                seen.forgets += 1;
                self.reach("an abandoned read returned");
            }
            ScheduleEffect::Forget { pid } => {
                assert!(
                    self.held_before.contains(&pid.0) && !self.forgotten.contains(&pid.0),
                    "{pid} forgotten, not held or forgotten twice: {trail}"
                );
                self.forgotten.push(pid.0);
            }
            ScheduleEffect::RunIdle(waiter) => {
                let at = self
                    .model
                    .waiters
                    .iter()
                    .position(|(_, each)| *each == waiter)
                    .unwrap_or_else(|| panic!("waiter {waiter} run, not waiting: {trail}"));
                let (before, _) = self.model.waiters.remove(at);
                assert!(
                    self.model.idle(before),
                    "waiter {waiter} run before its events' outcomes: {trail}"
                );
                *self.model.ran.entry(waiter).or_default() += 1;
                self.reach("an idle waiter run");
                if given != Given::WhenIdle {
                    self.reach("an idle waiter run at a later step");
                }
            }
            ScheduleEffect::Job(job) => {
                let pid = job.pid.0;
                assert_eq!(given, Given::Next(pid), "{trail}");
                let app = self
                    .model
                    .apps
                    .get_mut(&pid)
                    .expect("a job of an application held");
                assert!(app.read.is_none(), "a second read at once: {trail}");
                assert!(!app.queue.is_empty(), "a job with nothing waiting: {trail}");
                // In push order, less those replaced or dropped.
                let queued = app.queue.remove(0);
                assert_eq!(job.meta.marker, queued.marker, "{trail}");
                assert_eq!(job.work, queued.marker, "{trail}");
                assert_eq!(job.window, queued.window, "{trail}");
                assert!(
                    app.threaded && app.thread == Some(app.generation),
                    "a job but to the thread in charge: {trail}"
                );
                assert_eq!(
                    job.request.generation, app.generation,
                    "a job to a generation not in charge: {trail}"
                );
                assert!(
                    app.requests.insert(job.request),
                    "a request given twice: {trail}"
                );
                app.read = Some(Read {
                    marker: queued.marker,
                    kind: queued.kind,
                    request: job.request,
                    start: now,
                    window: queued.window,
                    armed: None,
                    published: false,
                });
                self.reach("a job given");
                if job.after_elsewhere {
                    self.reach("a job after another application's focus");
                }
                if self.model.apps[&pid].ending {
                    self.reach("an ended application read on");
                }
            }
            effect => panic!("{effect:?} applied as an ask: {trail}"),
        }
    }

    /// The model's rules at the end of an action: an application whose
    /// outpost has ended and has nothing left is no longer held.
    fn settle(&mut self) {
        let spent: Vec<u32> = self
            .model
            .apps
            .iter()
            .filter(|(_, app)| app.spent())
            .map(|(pid, _)| *pid)
            .collect();
        for pid in spent {
            self.model.apps.remove(&pid);
        }
        let waiting: Vec<u32> = self
            .model
            .apps
            .iter()
            .filter(|(_, app)| !app.threaded && !app.queue.is_empty())
            .map(|(pid, _)| *pid)
            .collect();
        if !waiting.is_empty() && !self.model.stopping {
            self.reach("an event waiting at the limit");
        }
    }

    /// The schedule's open markers, applications and queues are the
    /// model's lists, and the invariants hold, after an action.
    fn compare(&self) {
        let trail = &self.trail;
        let model = &self.model;
        let schedule = &self.schedule;
        assert_eq!(schedule.stopping(), model.stopping, "stopping: {trail:?}");
        assert_eq!(
            schedule.markers(),
            model.open(),
            "the open markers: {trail:?}"
        );
        assert_eq!(
            schedule.apps.keys().map(|pid| pid.0).collect::<Vec<_>>(),
            model.apps.keys().copied().collect::<Vec<_>>(),
            "the applications held: {trail:?}"
        );
        for (pid, app) in &model.apps {
            let real = &schedule.apps[&Pid(*pid)];
            assert_eq!(
                real.queue
                    .iter()
                    .map(|queued| queued.meta.marker)
                    .collect::<Vec<_>>(),
                app.queue
                    .iter()
                    .map(|queued| queued.marker)
                    .collect::<Vec<_>>(),
                "{pid}'s queue: {trail:?}"
            );
            assert_eq!(
                real.flight.running().map(|(request, running, published)| (
                    request,
                    running.meta.marker,
                    published
                )),
                app.read
                    .as_ref()
                    .map(|read| (read.request, read.marker, read.published)),
                "{pid}'s read: {trail:?}"
            );
            assert_eq!(
                (real.threaded, real.ending, real.abandoned),
                (app.threaded, app.ending, app.strays.len()),
                "{pid}'s thread in charge, ending and abandoned threads: {trail:?}"
            );
            for stray in &app.strays {
                assert!(
                    !schedule.in_charge(Pid(*pid), stray.request.generation),
                    "an abandoned thread in charge: {trail:?}"
                );
            }
            if !model.stopping {
                // A thread in charge runs; an event waits with no thread
                // only at the abandoned limit.
                assert_eq!(app.threaded, app.thread.is_some(), "{pid}: {trail:?}");
                assert!(
                    app.threaded || app.queue.is_empty() || app.strays.len() >= ABANDONED_LIMIT,
                    "{pid}'s events wait with no thread below the limit: {trail:?}"
                );
                // The alarm is never later than a read's next wake.
                if let Some(wake) = real.flight.next_wake() {
                    assert!(
                        model.timer.is_some_and(|due| due <= wake),
                        "the alarm {:?} later than {pid}'s read's wake {wake}: {trail:?}",
                        model.timer
                    );
                }
            }
        }
        // Each idle waiter runs in the step in which every event queued
        // before it has its outcome.
        for (before, waiter) in &model.waiters {
            assert!(
                !model.idle(*before),
                "waiter {waiter} not run once its events had their outcomes: {trail:?}"
            );
        }
    }

    /// Ends the case: every read returned, every waiting event read and
    /// every alarm reached; then, with the listener running, every marker
    /// has had exactly one outcome and every idle waiter has run once,
    /// and, stopped, every marker pushed after the stop was dropped and
    /// each other has had at most one.
    fn drain(&mut self) {
        let plain = Asks {
            moved_on: false,
            publish: false,
            spawn_fails: false,
        };
        for _ in 0..400 {
            let reading = self.model.apps.iter().find_map(|(pid, app)| {
                app.read
                    .as_ref()
                    .map(|_| (*pid, false))
                    .or_else(|| (!app.strays.is_empty()).then_some((*pid, true)))
            });
            let asking = self
                .model
                .apps
                .iter()
                .find(|(_, app)| app.thread.is_some() && (!app.queue.is_empty() || app.ending))
                .map(|(pid, _)| *pid);
            let app_of = |pid: u32| {
                u8::try_from(PIDS.iter().position(|each| *each == pid).unwrap_or(0)).unwrap_or(0)
            };
            if let Some((pid, stray)) = reading {
                self.act(
                    Action::Return {
                        app: app_of(pid),
                        stray,
                        read: false,
                        top: 0,
                    },
                    plain,
                );
            } else if let Some(pid) = asking.filter(|_| !self.model.stopping) {
                self.act(Action::Next { app: app_of(pid) }, plain);
            } else if self.model.timer.is_some() && !self.model.stopping {
                self.act(Action::Advance(Advance::ToTimer(0)), plain);
            } else {
                break;
            }
        }
        let trail = &self.trail;
        let model = &self.model;
        for marker in model.places.keys().chain(&model.after_stop) {
            let outcomes = model.outcomes.get(marker).copied().unwrap_or(0);
            if model.stopping {
                assert!(outcomes <= 1, "marker {marker}: {trail:?}");
            } else {
                assert_eq!(outcomes, 1, "marker {marker}'s outcomes: {trail:?}");
            }
        }
        for waiter in 1..=model.next_waiter {
            let ran = model.ran.get(&waiter).copied().unwrap_or(0);
            if model.stopping {
                assert!(ran <= 1, "waiter {waiter}: {trail:?}");
            } else {
                assert_eq!(ran, 1, "waiter {waiter} ran {ran} times: {trail:?}");
            }
        }
    }
}

/// An effect's name, with what the postconditions compare.
fn effect_name(effect: &ScheduleEffect<u64>) -> String {
    match effect {
        ScheduleEffect::Outcome { meta, judged, .. } => {
            format!("outcome {} {}", meta.marker, reason(judged))
        }
        ScheduleEffect::Spawn { pid, .. } => format!("spawn {}", pid.0),
        ScheduleEffect::Wake(Waking::Thread(pid)) => format!("wake {}", pid.0),
        ScheduleEffect::Wake(Waking::All) => "wake all".to_owned(),
        ScheduleEffect::Unwatch { pid } => format!("unwatch {}", pid.0),
        ScheduleEffect::ForgetLast => "forget last".to_owned(),
        ScheduleEffect::Forget { pid } => format!("forget {}", pid.0),
        ScheduleEffect::Alarm(AlarmChange::Now) => "alarm now".to_owned(),
        ScheduleEffect::Alarm(AlarmChange::Set(due)) => format!("alarm {due:?}"),
        ScheduleEffect::RunIdle(waiter) => format!("run {waiter}"),
        ScheduleEffect::Job(job) => format!("job {}", job.meta.marker),
        ScheduleEffect::AskMovedOn { pid, .. } => format!("ask moved on {}", pid.0),
        ScheduleEffect::Publish { pid, .. } => format!("publish {}", pid.0),
    }
}

/// The application an effect names, if any.
fn effect_pid(effect: &ScheduleEffect<u64>) -> Option<Pid> {
    match effect {
        ScheduleEffect::Outcome { pid, .. }
        | ScheduleEffect::Spawn { pid, .. }
        | ScheduleEffect::Wake(Waking::Thread(pid))
        | ScheduleEffect::Unwatch { pid }
        | ScheduleEffect::Forget { pid }
        | ScheduleEffect::AskMovedOn { pid, .. }
        | ScheduleEffect::Publish { pid, .. } => Some(*pid),
        ScheduleEffect::Job(job) => Some(job.pid),
        ScheduleEffect::Wake(Waking::All)
        | ScheduleEffect::ForgetLast
        | ScheduleEffect::Alarm(_)
        | ScheduleEffect::RunIdle(_) => None,
    }
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
            Action::Advance(advance) => format!("Action::Advance(Advance::{advance:?})"),
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
const SEED: u64 = 20_261_103;
const CASES: u32 = 2_000;

/// Generated cases of events of every kind from three applications, the
/// threads' asks and returns, abandoned threads' returns, windows learned
/// and walks armed, stale among them, time, foreground events, MSAA
/// focuses, outposts' ends, idle waiters, failed spawns and the stop, with
/// every ask answered after its step's effects, keep the model's state and
/// every invariant and postcondition the design lists after every step,
/// end with every marker given one outcome and every waiter run once,
/// give the same effects when run twice, and reach every listed state.
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

/// The answers of a step that asks nothing the case relies on.
const PLAIN: Asks = Asks {
    moved_on: false,
    publish: false,
    spawn_fails: false,
};

/// A focus of application `app`, keyed by no element, its window unknown.
const fn focus(app: u8) -> (Action, Asks) {
    (
        Action::Push {
            app,
            kind: 0,
            key: None,
            window: 0,
        },
        PLAIN,
    )
}

/// `action`, with the plain answers.
const fn plain(action: Action) -> (Action, Asks) {
    (action, PLAIN)
}

/// The cases the generated test found, each the smallest a failure shrank
/// to, kept as lists of actions: a push replacing the one event an idle
/// waiter waited for; an ended application whose last abandoned read
/// returned; and a failed start in a tick that abandons another
/// application's read after it, with an idle waiter queued before.
fn fixed() -> Vec<Vec<(Action, Asks)>> {
    let menu = plain(Action::Push {
        app: 0,
        kind: 1,
        key: Some(0),
        window: 0,
    });
    vec![
        vec![menu, plain(Action::WhenIdle), menu],
        vec![
            focus(0),
            plain(Action::Next { app: 0 }),
            plain(Action::Advance(Advance::Late)),
            plain(Action::OutpostEnded { app: 0 }),
            plain(Action::Return {
                app: 0,
                stray: true,
                read: false,
                top: 0,
            }),
        ],
        vec![
            focus(2),
            focus(0),
            focus(0),
            plain(Action::Next { app: 0 }),
            plain(Action::Next { app: 2 }),
            plain(Action::WhenIdle),
            (
                Action::Advance(Advance::Late),
                Asks {
                    spawn_fails: true,
                    ..PLAIN
                },
            ),
        ],
    ]
}

/// Every fixed case keeps the schedule's rules, and gives the same
/// effects when run twice.
#[test]
fn fixed_cases_keep_the_schedules_rules() {
    for case in fixed() {
        let first = Run::case(&case);
        assert_eq!(first.log, Run::case(&case).log, "{case:?}");
    }
}

/// One step of a sequence of the menu end's grace.
#[derive(Clone, Copy, Debug)]
enum Grace {
    /// A menu ends, its grace 300 microseconds.
    Ended,
    /// The grace's timer goes off, as set or early.
    Fired,
    /// An idle round is to be answered.
    IdleAnswer,
    /// The timer is closed.
    Close,
    /// Time passes, this many microseconds.
    Wait(u64),
}

/// Each step a grace sequence may take.
const GRACE_STEPS: [Grace; 6] = [
    Grace::Ended,
    Grace::Fired,
    Grace::IdleAnswer,
    Grace::Close,
    Grace::Wait(100),
    Grace::Wait(300),
];

/// The grace's length the sequences give.
const GRACE_US: u64 = 300;

/// A grace sequence, with what it has sent.
#[derive(Clone, Debug, Default)]
struct GraceRun {
    grace: GraceState,
    trail: Vec<Grace>,
    now: u64,
    /// How many ends and rounds have come.
    ends: u64,
    rounds: u64,
    /// When the newest end's grace is over, while no end has been sent
    /// since it came.
    pending: Option<u64>,
    /// The rounds not answered as they came, each with the newest end
    /// when it came.
    held: Vec<(u64, u64)>,
    closed: bool,
    /// The ends sent, and the rounds answered.
    sent: Vec<u64>,
    answered: Vec<u64>,
}

impl GraceRun {
    /// Takes `step`, checking its effects against the grace's rules.
    fn step(&mut self, step: Grace) {
        self.trail.push(step);
        let now = self.now;
        let input = match step {
            Grace::Ended => {
                self.ends += 1;
                self.pending = Some(now + GRACE_US);
                GraceInput::Ended {
                    ended_at_us: self.ends,
                    timing: EventTiming::default(),
                    grace: GRACE_US,
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
        for effect in &effects {
            match effect {
                GraceEffect::SendEnd { ended_at_us, .. } => {
                    // Only the newest end is sent, and once.
                    assert_eq!(*ended_at_us, self.ends, "{:?}", self.trail);
                    assert!(
                        !self.sent.contains(ended_at_us),
                        "an end sent twice: {:?}",
                        self.trail
                    );
                    self.sent.push(*ended_at_us);
                    self.pending = None;
                }
                GraceEffect::SendIdleAnswer(round) => {
                    assert!(
                        !self.answered.contains(round),
                        "a round answered twice: {:?}",
                        self.trail
                    );
                    // A round that waited is answered after an end at
                    // least as new as the one it waited for.
                    if let Some(&(_, end)) = self.held.iter().find(|(held, _)| held == round) {
                        assert!(
                            self.sent.last().is_some_and(|sent| *sent >= end),
                            "round {round} answered before end {end}: {:?}",
                            self.trail
                        );
                    }
                    self.held.retain(|(held, _)| held != round);
                    self.answered.push(*round);
                }
                GraceEffect::SetTimer(_) => {}
            }
        }
        if matches!(step, Grace::IdleAnswer) && !self.answered.contains(&self.rounds) {
            self.held.push((self.rounds, self.ends));
        }
        // No round is held while no end is pending.
        assert!(
            self.pending.is_some() || self.held.is_empty(),
            "a round held with no end pending: {:?}",
            self.trail
        );
    }

    /// Ends the sequence as the timer would: once the grace is over, it
    /// fires, unless closed. Then every round is answered once and the
    /// newest end sent once; closed, at most once each.
    fn drain(mut self) {
        if !self.closed
            && let Some(over) = self.pending
        {
            self.now = self.now.max(over);
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

/// Every sequence of up to six steps of the menu end's grace (ends, the
/// timer going off as set or early, idle rounds, the close and time)
/// keeps the grace's rules: only the newest end is sent, no end or round
/// is sent twice, no round is held with no end pending, a round that
/// waited is answered after the end it waited for, and once drained every
/// round has its answer and the newest end was sent, unless the timer was
/// closed. The timer's exact settings are the grace's own tests'
/// examples.
#[test]
fn every_short_grace_sequence_keeps_the_graces_rules() {
    // Each depth in turn, so the sequence a failure prints is one of the
    // shortest that fail.
    for depth in 1..=GRACE_DEPTH {
        every_grace_sequence(&GraceRun::default(), depth);
    }
}
