//! The outpost's reducer's generated test (`books/dev/src/reference/crates/verbatim-outpost-logic.md`): lists of actions, each made valid when it is
//! applied by resolving its target against the model, given to
//! [`Reducer::input`] and [`Reducer::tick`] with every ask answered in
//! line as the outpost's shell answers it, and checked against a reference
//! model of sets and the invariants and postconditions the design lists,
//! after every call.
//!
//! The model tracks the generation in charge and the requests issued; the
//! program in flight (its kind, the query it answers, its hang mark, its
//! deadline as dispatched and its window); the markers waiting with their
//! front checks' due times; the entries pending as a set,
//! not the intake's plan, which has its own test; the open queries,
//! settles and idle rounds with the replies and answers each got; the
//! hook's copies held, of which a release gives back some in their order
//! of arrival; owed work and stopping; and the three clocks, each
//! advancing on its own. Which deadline each kind of program gets, which
//! held copies a release keeps and the caret watch's transitions are not
//! modelled: they are the examples of `src/tests.rs`, `hold.rs` and the
//! watch's enumeration (`tests/generated.rs`).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestError, TestRunner};
use verbatim_model::protocol::{UiaFocusRead, UiaSnapshotFact};

use super::*;

/// The top-level windows the generated entries and reports name: the
/// application's two, and none.
const TOPS: [isize; 2] = [0x10, 0x20];

/// The edit control whose caret reads are cut and whose value changes a
/// cut read's finish looks for.
const EDIT: isize = 9;

/// The moved-on grace, in microseconds.
const GRACE_US: u64 = 500_000;

/// The windows the hang cases name: a program's own, found hung or
/// answering, one in front, and destroyed.
const HANGING: [isize; 2] = [0x30, 0x31];

/// What an entry is.
#[derive(Clone, Copy, Debug)]
enum What {
    /// An MSAA name change in a top-level window.
    NameChange(u8),
    /// An MSAA focus on a child of window 0x11, of top-level window 0x10.
    MsaaFocus(u8),
    /// A foreground change into a top-level window: untested (0), not in
    /// front (1) or in front (2).
    Foreground { window: u8, front: u8 },
    /// A UIA property change of an element of window 0x10.
    UiaProperty,
    /// The caret's location change of the edit control.
    EditCaret,
    /// A value change of the edit control's client area.
    ValueChange,
    /// A UIA focus's marker.
    Marker,
    /// The read of the oldest marker not yet resolved, or one whose marker
    /// never came.
    Read,
    /// The drop of the oldest marker not yet resolved.
    Dropped,
    /// The end of the listener's reads.
    ReadsEnded,
    /// Core's focus-now query.
    FocusNow,
    /// A caret key's query, on node 1.
    CaretKey,
    /// A query for one navigation step.
    Step,
    /// A shell items query whose walk's last call time has passed.
    ShellLate,
    /// Core's idle round.
    Round,
    /// A test's settle.
    Settle,
    /// The nodes Core holds.
    NodesHeld,
    /// The destroy event of one of the hang cases' windows.
    Destroy(u8),
}

/// Where an entry comes from.
#[derive(Clone, Copy, Debug)]
enum From {
    Other,
    Hook,
    Listener,
}

/// What the program in flight commits.
#[derive(Clone, Copy, Debug)]
enum Commit {
    /// Nothing but its being made.
    Nothing,
    /// The reply to the query it answers.
    Reply,
    /// A focus in a top-level window (0, 1) or none (2), a whole-window one
    /// or not, adopted and followed, owing a terminal's first read or not.
    Report { top: u8, whole: bool, owes: bool },
    /// A caret key's program opens its watch, polling or not.
    OpenWatch { console: bool },
    /// The watch on node 1 is taken.
    Take,
    /// A take of node 2, which no watch is on.
    TakeOther,
    /// The held watch's check found its evidence or nothing.
    Checked { answered: bool },
    /// A held terminal's caret noted.
    NotedCaret,
    /// The owed work done.
    OwedDone,
    /// What the program found of one of the hang cases' windows: hung at
    /// its check before a call, with no call made, its query, if any,
    /// answered abandoned when it is refused; or answering a call.
    Found {
        window: u8,
        hung: bool,
        refuse: bool,
    },
    /// What the program's kind commits as its result: a caret key's
    /// program opens its watch, polling or not, a query's program
    /// replies, the owed work is done, a program holding the watch checks
    /// it, and an event's program reports a focus.
    Own(bool),
}

/// An input of a request no longer live: the newest request that left.
#[derive(Clone, Copy, Debug)]
enum Stale {
    Commit,
    Finished,
    MovedOn,
}

/// How the clocks advance before a tick.
#[derive(Clone, Copy, Debug)]
enum Advance {
    /// To the time [`Reducer::next_timer`] gives, the observation and
    /// shared clocks each this many milliseconds further.
    ToTimer { observed: u8, wall: u8 },
    /// Each clock by its own number of milliseconds.
    By { at: u16, observed: u16, wall: u16 },
}

/// One step of a case.
#[derive(Clone, Copy, Debug)]
enum Action {
    /// An entry arrives; with `more`, the next input comes in the same
    /// drain of the mailbox, so the shell's tick waits for it.
    Entry { what: What, from: From, more: bool },
    /// The hook is in and a listener forwards.
    Hooked,
    /// No listener forwards.
    WithoutListener,
    /// The listener's forwarding ended through the newest stamp.
    ForwardingEnded,
    /// Core withdraws the oldest query still waiting, or else the oldest
    /// open one.
    Cancel,
    /// A verdict of the generation in charge or of the one before it:
    /// a probe (0 to 3 by kind) or a window's forgetting (4).
    Verdict { dead: bool, change: u8 },
    /// The program in flight commits.
    Commit(Commit),
    /// An input of a request no longer live.
    Stale(Stale),
    /// One of the hang cases' windows is seen hung: with no request (0), as
    /// the front check finds it, the program in flight's (1), or the newest
    /// request no longer live (2).
    HangSeen { window: u8, request: u8 },
    /// One of the hang cases' windows is in front and can be read.
    FrontReadable(u8),
    /// The program in flight ends: having reported a focus, panicked, or
    /// cut at its value change check.
    Finished {
        reported: bool,
        panicked: bool,
        cut: bool,
    },
    /// Time passes and the shell ticks.
    Advance(Advance),
    /// The outpost shuts down.
    Close,
}

/// The answers the shell gives the asks an action causes.
#[derive(Clone, Copy, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one answer to each kind of ask"
)]
struct Asks {
    /// Whether the user has moved on.
    moved_on: bool,
    /// The window a front check finds: another application's (0), one of
    /// the hang cases' windows hung (1, 2), or readable (3, 4).
    front: u8,
    /// Whether a release lets go of node 1, the watch's.
    release_node: bool,
    /// Whether a release lets go of objects to drop.
    objects: bool,
    /// Whether the window checked is found hung.
    hung: bool,
    /// The window a program with none at dispatch names in its charge:
    /// none (0), or one of the hang cases' windows (1, 2).
    named: u8,
}

fn what() -> impl Strategy<Value = What> {
    use proptest::prelude::*;
    prop_oneof![
        3 => (0..2u8).prop_map(What::NameChange),
        4 => (1..4u8).prop_map(What::MsaaFocus),
        3 => (0..2u8, 0..3u8).prop_map(|(window, front)| What::Foreground { window, front }),
        1 => Just(What::UiaProperty),
        2 => Just(What::EditCaret),
        1 => Just(What::ValueChange),
        2 => Just(What::Marker),
        2 => Just(What::Read),
        2 => Just(What::Dropped),
        1 => Just(What::ReadsEnded),
        2 => Just(What::FocusNow),
        3 => Just(What::CaretKey),
        2 => Just(What::Step),
        1 => Just(What::ShellLate),
        2 => Just(What::Round),
        2 => Just(What::Settle),
        2 => Just(What::NodesHeld),
        1 => (0..2u8).prop_map(What::Destroy),
    ]
}

fn commit() -> impl Strategy<Value = Commit> {
    use proptest::prelude::*;
    prop_oneof![
        1 => Just(Commit::Nothing),
        3 => Just(Commit::Reply),
        4 => (0..3u8, any::<bool>(), any::<bool>())
            .prop_map(|(top, whole, owes)| Commit::Report { top, whole, owes }),
        3 => any::<bool>().prop_map(|console| Commit::OpenWatch { console }),
        5 => Just(Commit::Take),
        3 => Just(Commit::TakeOther),
        2 => any::<bool>().prop_map(|answered| Commit::Checked { answered }),
        1 => Just(Commit::NotedCaret),
        2 => Just(Commit::OwedDone),
        3 => (0..2u8, any::<bool>(), any::<bool>())
            .prop_map(|(window, hung, refuse)| Commit::Found { window, hung, refuse }),
        8 => any::<bool>().prop_map(Commit::Own),
    ]
}

fn action() -> impl Strategy<Value = Action> {
    use proptest::prelude::*;
    let from = prop_oneof![Just(From::Other), Just(From::Hook), Just(From::Listener)];
    let stale = prop_oneof![
        Just(Stale::Commit),
        Just(Stale::Finished),
        Just(Stale::MovedOn),
    ];
    let advance = prop_oneof![
        3 => (0..3u8, 0..3u8).prop_map(|(observed, wall)| Advance::ToTimer { observed, wall }),
        1 => (0..700u16, 0..700u16, 0..700u16)
            .prop_map(|(at, observed, wall)| Advance::By { at, observed, wall }),
        // A late tick, past every bound and deadline at once.
        1 => Just(Advance::By {
            at: 12_000,
            observed: 12_000,
            wall: 12_000,
        }),
    ];
    // Weighted toward the lifecycle: entries that start programs, their
    // ends, abandonments by time, and hangs seen.
    prop_oneof![
        12 => (what(), from, prop::bool::weighted(0.2))
            .prop_map(|(what, from, more)| Action::Entry { what, from, more }),
        1 => Just(Action::Hooked),
        1 => Just(Action::WithoutListener),
        1 => Just(Action::ForwardingEnded),
        1 => Just(Action::Cancel),
        1 => (any::<bool>(), 0..5u8).prop_map(|(dead, change)| Action::Verdict { dead, change }),
        6 => commit().prop_map(Action::Commit),
        2 => stale.prop_map(Action::Stale),
        2 => (0..2u8, 0..3u8).prop_map(|(window, request)| Action::HangSeen { window, request }),
        1 => (0..2u8).prop_map(Action::FrontReadable),
        5 => (prop::bool::weighted(0.3), prop::bool::weighted(0.1), prop::bool::weighted(0.2))
            .prop_map(|(reported, panicked, cut)| Action::Finished { reported, panicked, cut }),
        6 => advance.prop_map(Action::Advance),
        1 => Just(Action::Close),
    ]
}

fn asks() -> impl Strategy<Value = Asks> {
    use proptest::prelude::*;
    (
        (prop::bool::weighted(0.3), 0..5u8),
        (
            prop::bool::weighted(0.3),
            any::<bool>(),
            prop::bool::weighted(0.2),
            0..3u8,
        ),
    )
        .prop_map(
            |((moved_on, front), (release_node, objects, hung, named))| Asks {
                moved_on,
                front,
                release_node,
                objects,
                hung,
                named,
            },
        )
}

/// What a program does, as the model follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// An entry's program: an event, a focus, or a query.
    Entry,
    /// A caret key's program, which may open the watch.
    Key,
    /// The edit control's caret event, which may be cut.
    EditCaret,
    /// A noted caret's read.
    Noted,
    /// The owed work.
    Owed,
    /// A release's drop.
    Drop,
    /// A read of the console poll.
    PollRead,
    /// The finish of a cut read.
    Finish,
}

/// The program in flight, as the model follows it.
#[derive(Clone, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of the program's, each changed by its own inputs"
)]
struct Flight {
    request: Request,
    kind: Kind,
    /// The query it answers, while it is its to answer.
    query: Option<u64>,
    /// Whether it was dispatched for a query.
    for_query: bool,
    /// The focus group it is a try of, by number.
    group: Option<u64>,
    /// Its hang mark: a hang was seen of its request, so its worker,
    /// abandoned, is parked on a hung window.
    hang_mark: bool,
    /// When it started, on the shell's clock, in microseconds.
    started: u64,
    /// Its deadline as dispatched.
    deadline: u64,
    /// Whether it is judged by its window, known or named: every program
    /// but a release's drop.
    window: bool,
    /// Its copy of the facts, with its own commits applied.
    copy: Facts,
    /// Whether its copy of the watch is compared no more: after it has
    /// committed a check of the watch, its copy may show a poll the reducer
    /// ended by its clock, which the copy does not read.
    uncompared: bool,
    /// The windows destroyed while it ran, whose hang it names afterwards
    /// opens nothing.
    destroyed: BTreeSet<isize>,
}

/// A pending entry: what it is and where it came from.
#[derive(Clone, Copy, Debug)]
struct Pending {
    what: What,
    /// The query number, for a query.
    query: Option<u64>,
}

/// The reference model.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of the model's, each changed by its own inputs"
)]
struct Model {
    generation: u64,
    /// The windows whose hang interval is open, by the reference model of
    /// hangs ([`Run::hangs`]).
    hung: BTreeSet<isize>,
    /// Every request number issued.
    numbers: BTreeSet<u64>,
    flight: Option<Flight>,
    /// The entries pushed and not yet handled, by trace.
    pending: BTreeMap<TraceId, Pending>,
    /// The hook's copies held, by trace.
    held: BTreeMap<TraceId, Pending>,
    /// The hold: not yet hooked (0), holding (1), or over (2).
    hold: u8,
    /// The newest stamp of a copy, for the end of forwarding.
    newest_stamp: u32,
    /// The hook's copies held, in the order they came.
    held_order: Vec<TraceId>,
    /// The queries recorded and not yet replied to, and every query's
    /// replies; their order of arrival, and those taken from the intake.
    queries: BTreeSet<u64>,
    replies: BTreeMap<u64, u32>,
    arrived: Vec<u64>,
    taken: BTreeSet<u64>,
    /// The late shell queries, answered without a program.
    late: BTreeSet<u64>,
    /// The settles open, and the answers of each.
    settles: BTreeSet<u64>,
    settled: BTreeMap<u64, u32>,
    /// The idle answers of each round.
    answered_rounds: BTreeMap<u64, u32>,
    /// The markers not yet resolved, oldest first, each with when the
    /// window in front is next checked while it is the first, and whether
    /// it has been checked.
    markers: Vec<(u64, u64, bool)>,
    /// The owed work: its instance, and the generation attempting it.
    owed: Option<(u64, Option<u64>)>,
    owed_tries: BTreeSet<(u64, u64)>,
    owed_count: u64,
    /// Whether a release's drop is due, and whether one is under way.
    to_drop: bool,
    releasing: bool,
    /// A noted caret, kept for when nothing else is ready.
    noted: bool,
    stopping: bool,
    /// The requests no longer live, newest last.
    dead: Vec<Request>,
    /// Whether the last program to leave was abandoned.
    abandoned: bool,
    /// The focus groups a try reported in, by number: none of their older
    /// entries is tried after.
    ended_groups: BTreeSet<u64>,
}

/// The program a dispatch gives, by kind.
fn kind_of(program: &Program) -> Kind {
    match &program.work {
        Work::Entry { entry, .. } => match &entry.item {
            Item::NotedCaret { .. } => Kind::Noted,
            Item::Query {
                query:
                    Query::Text {
                        op: TextOp::AwaitCaret(_),
                        ..
                    },
                ..
            } => Kind::Key,
            Item::Msaa {
                kind: WinKind::Caret,
                ..
            } => Kind::EditCaret,
            _ => Kind::Entry,
        },
        Work::Owed(_) => Kind::Owed,
        Work::Drop => Kind::Drop,
        Work::PollRead => Kind::PollRead,
        Work::Finish { .. } => Kind::Finish,
    }
}

/// A program's copy of the watch apart from its times.
fn untimed(watch: &Watch) -> Watch {
    match watch.clone() {
        Watch::None => Watch::None,
        Watch::Waiting { open, .. } => Watch::Waiting { open, bound: 0 },
        Watch::Polling { open, .. } => Watch::Polling {
            open,
            next: 0,
            until: 0,
        },
        Watch::Checking { open, resume } => Watch::Checking {
            open,
            resume: match resume {
                Resume::Bound(_) => Resume::Bound(0),
                Resume::Poll { .. } => Resume::Poll { next: 0, until: 0 },
                Resume::Read { .. } => Resume::Read { until: 0 },
            },
        },
    }
}

/// A hash of `text`, so each run keeps its effects' `Debug` text short.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What a call was, for its postconditions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    /// An input from the generator.
    Input,
    /// A tick.
    Tick,
    /// An ask's answer.
    Answer,
}

/// What the gate's cases reached, by name.
type Reached = BTreeMap<&'static str, u64>;

/// The lifecycle states the gate's cases must each reach.
const LISTED: [&str; 37] = [
    "an entry dispatched",
    "a focus group continued",
    "a focus group continued after an abandonment",
    "abandoned at the deadline",
    "abandoned as moved on",
    "abandoned parked on a hung window",
    "a hang opened by a check",
    "a hang opened in front",
    "a hang opened for a request no longer live",
    "a hang opened by a commit",
    "a hang ended by an answer",
    "a hang ended in front",
    "a hang ended by a destroy event",
    "a hang after its window's destroy ignored",
    "a hang in a second window unsounded",
    "a refusal answered abandoned",
    "the front checked",
    "the front checked again while its marker waits",
    "a cut read finished",
    "a program panicked",
    "a refused input",
    "owed work dispatched",
    "owed work abandoned",
    "a release answered",
    "a drop dispatched",
    "the watch opened",
    "the watch taken",
    "the watch polled",
    "a query cancelled",
    "a late shell query answered",
    "an idle round answered",
    "a settle answered",
    "a hook copy held",
    "held copies released",
    "closed with a program in flight",
    "a dead generation's verdict kept",
    "a dead generation's verdict refused",
];

/// One run of one case.
struct Run {
    reducer: Reducer,
    model: Model,
    /// The clocks: the shell's, the observation clock, the shared clock,
    /// in microseconds.
    at: u64,
    observed: u64,
    wall: u64,
    asks: Asks,
    /// Counters for traces, queries, settles, rounds and markers.
    next: u64,
    /// The hashes of every call's effects, for the double run.
    log: Vec<u64>,
    reached: Reached,
    trail: Vec<String>,
    /// Whether the call being made answers a check with the user moved
    /// on: the one abandonment before a deadline.
    moving_on: bool,
}

impl Run {
    fn new() -> Self {
        Self {
            reducer: Reducer::new(),
            model: Model::default(),
            at: 1_000,
            observed: 1_000,
            wall: 1_000,
            asks: Asks {
                moved_on: false,
                front: 0,
                release_node: false,
                objects: false,
                hung: false,
                named: 0,
            },
            next: 0,
            log: Vec::new(),
            reached: Reached::new(),
            trail: Vec::new(),
            moving_on: false,
        }
    }

    fn reach(&mut self, what: &'static str) {
        *self.reached.entry(what).or_default() += 1;
    }

    fn now(&self) -> Now {
        Now {
            at: Duration::from_micros(self.at),
            observed_us: self.observed,
            wall_us: self.wall,
        }
    }

    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    /// Runs one case: its actions, each with its asks' answers, then the
    /// drain.
    fn case(actions: &[(Action, Asks)]) -> Self {
        let mut run = Self::new();
        for &(action, asks) in actions {
            run.asks = asks;
            run.step(action);
        }
        run.drain();
        run
    }

    /// Takes `action`, then the shell's tick after the drain, unless more
    /// of the drain is to come.
    fn step(&mut self, action: Action) {
        self.trail.push(format!("{action:?}"));
        let tick = match action {
            Action::Entry { what, from, more } => {
                self.entry(what, from);
                !more
            }
            Action::Hooked => {
                if self.model.hold == 0 {
                    self.model.hold = 1;
                }
                self.input(Input::Hooked);
                true
            }
            Action::WithoutListener => {
                let released =
                    (self.model.hold == 1).then(|| self.reducer.hold.clone().without_listener());
                self.release_hold(true, released);
                self.input(Input::WithoutListener);
                true
            }
            Action::ForwardingEnded => {
                let through = self.model.newest_stamp;
                let released = self.reducer.hold.clone().forwarding_ended(through);
                if self.model.hold == 0 {
                    // Released with no hold: nothing is held from then on.
                    self.model.hold = 2;
                } else {
                    self.release_hold(false, released);
                }
                self.input(Input::ForwardingEnded(through));
                true
            }
            Action::Cancel => {
                self.cancel();
                true
            }
            Action::Verdict { dead, change } => {
                self.verdict(dead, change);
                true
            }
            Action::Commit(commit) => {
                self.commit(commit);
                true
            }
            Action::Stale(stale) => {
                self.stale(stale);
                true
            }
            Action::HangSeen { window, request } => {
                let request = match request {
                    0 => None,
                    1 => self.model.flight.as_ref().map(|flight| flight.request),
                    _ => self.model.dead.last().copied(),
                };
                let window =
                    Window::new(HANGING[usize::from(window)]).expect("a hang case's window");
                self.input(Input::HangSeen { window, request });
                true
            }
            Action::FrontReadable(window) => {
                let window =
                    Window::new(HANGING[usize::from(window)]).expect("a hang case's window");
                self.input(Input::FrontReadable { window });
                true
            }
            Action::Finished {
                reported,
                panicked,
                cut,
            } => {
                self.finished(reported, panicked, cut);
                true
            }
            Action::Advance(advance) => {
                self.advance(advance);
                false
            }
            Action::Close => {
                if self.model.flight.is_some() && !self.model.stopping {
                    self.reach("closed with a program in flight");
                }
                self.model.stopping = true;
                self.model.pending.clear();
                self.model.held.clear();
                self.model.markers.clear();
                self.model.settles.clear();
                self.model.noted = false;
                self.input(Input::Close);
                true
            }
        };
        if tick {
            self.call(None, Call::Tick);
        }
        self.compare();
    }

    /// An entry of `what` from `from`, now.
    #[expect(clippy::too_many_lines, reason = "one arm for each kind of entry")]
    fn entry(&mut self, what: What, from: From) {
        let trace = self.fresh();
        let observed = self.observed;
        let stamp = Some(WinEventStamp {
            thread: 3,
            time: u32::try_from(self.at / 1_000).unwrap_or(u32::MAX),
        });
        let mut front = Front::Untested;
        let mut query = None;
        let mut source = Source::Other;
        let item = match what {
            What::NameChange(window) => {
                source = source_of(from);
                Item::Msaa {
                    kind: WinKind::NameChange,
                    hwnd: TOPS[usize::from(window)],
                    id_object: -4,
                    id_child: 0,
                    raiser: 3,
                }
            }
            What::Destroy(window) => {
                source = source_of(from);
                Item::Msaa {
                    kind: WinKind::Destroy,
                    hwnd: HANGING[usize::from(window)],
                    id_object: 0,
                    id_child: 0,
                    raiser: 3,
                }
            }
            What::MsaaFocus(child) => {
                source = source_of(from);
                front = Front::Accepted(true);
                Item::Fact(
                    DeliveredFact::MsaaFocus {
                        hwnd: 0x11,
                        id_object: -4,
                        id_child: i32::from(child),
                    },
                    stamp,
                )
            }
            What::Foreground {
                window,
                front: tested,
            } => {
                // Never the hook's held copy.
                source = match from {
                    From::Hook | From::Listener => Source::Listener,
                    From::Other => Source::Other,
                };
                front = match tested {
                    0 => Front::Untested,
                    1 => Front::Foreground {
                        in_front_at_us: None,
                    },
                    _ => Front::Foreground {
                        in_front_at_us: Some(observed),
                    },
                };
                Item::Fact(
                    DeliveredFact::Foreground {
                        hwnd: TOPS[usize::from(window)],
                    },
                    stamp,
                )
            }
            What::UiaProperty => Item::Uia(UiaEvent {
                kind: UiaKind::Property(30_005),
                parts: snapshot(vec![42, 1]),
                hwnd: 0x10,
                focus_window: 0,
                element: None,
            }),
            What::EditCaret => {
                source = source_of(from);
                Item::Msaa {
                    kind: WinKind::Caret,
                    hwnd: EDIT,
                    id_object: -8,
                    id_child: 0,
                    raiser: 3,
                }
            }
            What::ValueChange => {
                source = source_of(from);
                Item::Msaa {
                    kind: WinKind::ValueChange,
                    hwnd: EDIT,
                    id_object: -4,
                    id_child: 0,
                    raiser: 3,
                }
            }
            What::Marker => {
                let marker = self.fresh();
                Item::Fact(DeliveredFact::UiaFocusMarker { marker }, None)
            }
            What::Read => {
                let marker = if self.model.markers.is_empty() {
                    self.fresh()
                } else {
                    self.model.markers.remove(0).0
                };
                Item::Fact(focus_read(marker), None)
            }
            What::Dropped => {
                let marker = if self.model.markers.is_empty() {
                    self.fresh()
                } else {
                    self.model.markers.remove(0).0
                };
                Item::Fact(
                    DeliveredFact::UiaFocusDropped {
                        marker,
                        focus: true,
                    },
                    None,
                )
            }
            What::ReadsEnded => Item::Fact(DeliveredFact::UiaFocusReadsEnded, None),
            What::FocusNow => {
                let number = self.fresh();
                query = Some(number);
                Item::Query {
                    request_id: RequestId::shell(number),
                    query: Query::ReadFocus,
                }
            }
            What::CaretKey => {
                let number = self.fresh();
                query = Some(number);
                caret_key(number)
            }
            What::Step => {
                let number = self.fresh();
                query = Some(number);
                Item::Query {
                    request_id: RequestId::shell(number),
                    query: Query::Navigate {
                        node_id: NodeId::new(1),
                        kind: verbatim_model::QueryKind::NextSibling,
                    },
                }
            }
            What::ShellLate => {
                let number = self.fresh();
                query = Some(number);
                Item::Query {
                    request_id: RequestId::shell(number),
                    query: Query::ShellItems {
                        kind: verbatim_model::ShellItemKind::Taskbar,
                        answer_by_us: self.wall,
                    },
                }
            }
            What::Round => Item::Barrier {
                asker: Asker::Round(trace),
                behind_noted: false,
            },
            What::Settle => Item::Barrier {
                asker: Asker::Settle(trace),
                behind_noted: false,
            },
            What::NodesHeld => Item::NodesHeld {
                nodes: Vec::new(),
                anchors: Vec::new(),
                acknowledged: trace,
            },
        };
        // The model, as the entry is taken in.
        let model = &mut self.model;
        if let Item::Fact(_, Some(stamp)) = &item {
            model.newest_stamp = model.newest_stamp.max(stamp.time);
        }
        // The hold keeps the hook's copies after the close too.
        if model.stopping && source == Source::Hook && model.hold == 1 {
            model
                .held
                .insert(TraceId::from_raw(trace), Pending { what, query });
            model.held_order.push(TraceId::from_raw(trace));
        }
        if !model.stopping {
            if let Some(number) = query {
                model.queries.insert(number);
                model.arrived.push(number);
                if matches!(what, What::ShellLate) {
                    model.late.insert(number);
                }
            }
            if matches!(what, What::Settle) {
                model.settles.insert(trace);
            }
            if let What::Marker = what
                && let Item::Fact(DeliveredFact::UiaFocusMarker { marker }, _) = &item
            {
                model.markers.push((*marker, self.at + GRACE_US, false));
            }
            // The end of the listener's reads resolves every marker.
            if let What::ReadsEnded = what {
                model.markers.clear();
            }
            let held = source == Source::Hook && model.hold == 1;
            if held {
                model
                    .held
                    .insert(TraceId::from_raw(trace), Pending { what, query });
                model.held_order.push(TraceId::from_raw(trace));
                self.reach("a hook copy held");
            } else {
                self.take_in(what, trace, query);
            }
        }
        let entry = Entry {
            item,
            trace: TraceId::from_raw(trace),
            observed_at_us: observed,
            timing: EventTiming::default(),
            front,
        };
        if matches!(what, What::ReadsEnded) && self.model.hold == 1 {
            let released = self.reducer.hold.clone().reads_ended();
            self.release_hold(false, released);
        }
        self.input(Input::Entry { entry, source });
    }

    /// The model takes in an entry the reducer pushes into its intake.
    fn take_in(&mut self, what: What, trace: u64, query: Option<u64>) {
        if !matches!(
            what,
            What::Round
                | What::Settle
                | What::NodesHeld
                | What::Marker
                | What::Dropped
                | What::ReadsEnded
        ) {
            self.model
                .pending
                .insert(TraceId::from_raw(trace), Pending { what, query });
        }
    }

    /// The hold is over: `released`, what the reducer's hold gives back, is
    /// some of the copies held in the order they came, every one of them
    /// when `all`, with no listener to have forwarded any; those are taken
    /// in.
    fn release_hold(&mut self, all: bool, released: Option<Vec<Entry>>) {
        let holding = self.model.hold == 1;
        self.model.hold = 2;
        let held = std::mem::take(&mut self.model.held);
        let order = std::mem::take(&mut self.model.held_order);
        if !holding {
            assert!(
                released.is_none(),
                "a release with no hold: {:?}",
                self.trail
            );
            return;
        }
        self.reach("held copies released");
        let kept: Vec<TraceId> = released
            .expect("a release of the hold")
            .iter()
            .map(|entry| entry.trace)
            .collect();
        let mut rest = order.iter();
        assert!(
            kept.iter().all(|trace| rest.any(|held| held == trace)),
            "the copies a release gives back, {kept:?}, not of {order:?} in order: {:?}",
            self.trail
        );
        if all {
            assert_eq!(kept, order, "a release with no listener: {:?}", self.trail);
        }
        if !self.model.stopping {
            for trace in kept {
                self.model.pending.insert(trace, held[&trace]);
            }
        }
    }

    fn cancel(&mut self) {
        let Some(&number) = self
            .model
            .arrived
            .iter()
            .find(|number| {
                self.model.queries.contains(number) && !self.model.taken.contains(number)
            })
            .or_else(|| {
                self.model
                    .arrived
                    .iter()
                    .find(|number| self.model.queries.contains(number))
            })
        else {
            return;
        };
        let request_id = RequestId::shell(number);
        let waiting = !self.model.taken.contains(&number) && !self.model.stopping;
        let effects = self.input(Input::Cancel(request_id));
        let replies: Vec<_> = replies(&effects);
        if waiting {
            assert_eq!(
                replies,
                [(number, "not started")],
                "a waiting query is withdrawn: {:?}",
                self.trail
            );
            self.reach("a query cancelled");
        } else {
            assert!(
                replies.iter().all(|(replied, _)| *replied != number),
                "a query taken is not withdrawn: {:?}",
                self.trail
            );
        }
    }

    fn verdict(&mut self, dead: bool, change: u8) {
        let generation = if dead {
            let Some(generation) = self.model.generation.checked_sub(1) else {
                return;
            };
            generation
        } else {
            self.model.generation
        };
        let change = match change {
            4 => VerdictChange::Forget(0x20),
            probed => VerdictChange::Probed(Probe {
                hwnd: 0x20,
                owner: 5,
                from_us: self.observed,
                probed: match probed {
                    0 => Probed::Uia,
                    1 => Probed::NotUia(self.observed + 1_000_000),
                    2 => Probed::TimedOut(self.observed + 1_000_000),
                    _ => Probed::Excluded,
                },
            }),
        };
        let kept = !dead || change.kept_from_any_generation();
        let before = Arc::clone(&self.reducer.facts.verdicts);
        let effects = self.input(Input::Verdict { generation, change });
        let changed = *before != *self.reducer.facts.verdicts;
        if dead {
            self.reach(if kept {
                "a dead generation's verdict kept"
            } else {
                "a dead generation's verdict refused"
            });
        }
        assert!(
            kept || !changed,
            "a dead generation's verdict kept: {:?}",
            self.trail
        );
        let copies = effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Verdicts(_)))
            .count();
        assert_eq!(
            copies,
            usize::from(changed),
            "the verdicts are sent exactly when the record changed: {:?}",
            self.trail
        );
    }

    /// The change `commit` is for the program in flight, `flight`, if it
    /// can make it.
    fn change(&mut self, commit: Commit, flight: &Flight) -> Option<Change> {
        let commit = match commit {
            Commit::Own(flag) => match flight.kind {
                Kind::Key if flight.query.is_some() => Commit::OpenWatch { console: flag },
                _ if flight.query.is_some() => Commit::Reply,
                Kind::Owed => Commit::OwedDone,
                _ if matches!(self.reducer.facts.follow.watch, Watch::Checking { .. }) => {
                    Commit::Checked { answered: flag }
                }
                _ => Commit::Report {
                    top: 0,
                    whole: false,
                    owes: flag,
                },
            },
            commit => commit,
        };
        Some(match commit {
            Commit::Nothing => Change::default(),
            Commit::Reply => Change {
                result: Some(OutpostToCore::Reply {
                    trace_id: TraceId::from_raw(1),
                    request_id: RequestId::shell(flight.query?),
                    outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Done)),
                    timing: EventTiming::default(),
                }),
                ..Change::default()
            },
            Commit::Report { top, whole, owes } => {
                // A query's program commits its reply as its one message.
                if flight.for_query
                    || !matches!(
                        flight.kind,
                        Kind::Entry | Kind::EditCaret | Kind::Owed | Kind::Noted
                    )
                {
                    return None;
                }
                let top = TOPS.get(usize::from(top)).copied();
                report(self.fresh(), top, self.observed, whole, owes)
            }
            Commit::OpenWatch { console } if flight.kind == Kind::Key => {
                opens(flight.query?, console)
            }
            Commit::Take if matches!(flight.kind, Kind::Entry | Kind::EditCaret) => takes(),
            Commit::TakeOther if matches!(flight.kind, Kind::Entry | Kind::EditCaret) => Change {
                watch: Some(WatchChange::Take(NodeId::new(2))),
                ..Change::default()
            },
            Commit::Checked { answered: true } => answered(),
            Commit::Checked { answered: false } => watching(1),
            Commit::NotedCaret => Change {
                noted_caret: Some((
                    NodeId::new(3),
                    TraceId::from_raw(self.fresh()),
                    self.observed,
                    EventTiming::default(),
                )),
                ..Change::default()
            },
            Commit::Found {
                window,
                hung,
                refuse,
            } => {
                let window =
                    Window::new(HANGING[usize::from(window)]).expect("a hang case's window");
                // A refused query is answered abandoned, as its check
                // before its first call answers it.
                let result = (hung && refuse)
                    .then_some(flight.query)
                    .flatten()
                    .map(|query| OutpostToCore::Reply {
                        trace_id: TraceId::from_raw(1),
                        request_id: RequestId::shell(query),
                        outcome: QueryOutcome::Abandoned,
                        timing: EventTiming::default(),
                    });
                Change {
                    result,
                    found: Some((window, if hung { Found::Hung } else { Found::Answered })),
                    ..Change::default()
                }
            }
            Commit::OwedDone if flight.kind == Kind::Owed => Change {
                owed_done: Some(NodeId::new(
                    self.reducer.facts.follow.owed.as_ref()?.node.number(),
                )),
                ..Change::default()
            },
            Commit::OpenWatch { .. }
            | Commit::Take
            | Commit::TakeOther
            | Commit::OwedDone
            | Commit::Own(_) => {
                return None;
            }
        })
    }

    fn commit(&mut self, commit: Commit) {
        let Some(flight) = self.model.flight.clone() else {
            return;
        };
        let Some(change) = self.change(commit, &flight) else {
            return;
        };
        assert!(
            self.reducer.accepts(flight.request),
            "a commit of the program in flight is accepted: {:?}",
            self.trail
        );
        // The program's copy of the watch agrees with the reducer's.
        if !flight.uncompared {
            assert_eq!(
                untimed(&flight.copy.follow.watch),
                untimed(&self.reducer.facts.follow.watch),
                "the program's copy and the reducer disagree: {:?}",
                self.trail
            );
        }
        self.commit_model(&change);
        if let Some(flight) = self.model.flight.as_mut() {
            let _ = apply_local(&mut flight.copy, flight.request.generation, &change);
            flight.uncompared |= matches!(change.watch, Some(WatchChange::Checked(_)));
        }
        let result = change.result.is_some();
        let watch_change = change.watch.clone();
        let effects = self.input(Input::Commit {
            request: flight.request,
            change: Box::new(change),
        });
        // A watch opened for the program's query answers it from then on.
        let watch = &self.reducer.facts.follow.watch;
        match watch_change {
            Some(WatchChange::Open { .. })
                if watch.open().map(|open| open.request_id.number) == flight.query =>
            {
                if let Some(flight) = self.model.flight.as_mut() {
                    flight.query = None;
                }
                self.reach("the watch opened");
            }
            Some(WatchChange::Take(_)) if matches!(watch, Watch::Checking { .. }) => {
                self.reach("the watch taken");
            }
            _ => {}
        }
        // Its messages first, but for the marks.
        if result {
            let first = effects
                .iter()
                .find(|effect| !matches!(effect, Effect::Marks(_)));
            assert!(
                matches!(first, Some(Effect::Send(_))),
                "a commit's message goes first: {:?}",
                self.trail
            );
        }
    }

    /// The model's part of an accepted commit of `change`.
    fn commit_model(&mut self, change: &Change) {
        let model = &mut self.model;
        // A followed focus replaces the owed work with its own, if any.
        if let Some(follow) = &change.follow {
            model.owed = follow.owed.as_ref().map(|_| {
                model.owed_count += 1;
                (model.owed_count, None)
            });
        }
        if change.owed_done.is_some()
            && let Some((_, attempting)) = model.owed
            && attempting == Some(model.generation)
        {
            model.owed = None;
        }
        if change.noted_caret.is_some() {
            model.noted = true;
        }
    }

    /// Gives `input`, which the reducer must refuse: no effect, and no
    /// change of its state.
    fn refused(&mut self, input: Input) {
        let before = format!("{:?}", self.reducer);
        let effects = self.input(input);
        assert!(
            effects.is_empty(),
            "a refused input gives no effect: {effects:?}, {:?}",
            self.trail
        );
        assert_eq!(
            format!("{:?}", self.reducer),
            before,
            "a refused input changes nothing: {:?}",
            self.trail
        );
        self.reach("a refused input");
    }

    fn stale(&mut self, stale: Stale) {
        let Some(&request) = self.model.dead.last() else {
            return;
        };
        let input = match stale {
            Stale::Commit => Input::Commit {
                request,
                change: Box::new(report(
                    self.fresh(),
                    Some(0x10),
                    self.observed,
                    false,
                    false,
                )),
            },
            Stale::Finished => Input::Finished {
                request,
                reported: true,
                panicked: false,
                cut: None,
            },
            Stale::MovedOn => Input::Judged {
                request,
                moved_on: true,
            },
        };
        self.refused(input);
    }

    fn finished(&mut self, reported: bool, panicked: bool, cut: bool) {
        let Some(flight) = self.model.flight.clone() else {
            return;
        };
        let cut = (cut && flight.kind == Kind::EditCaret).then(|| self::cut(report_cut()));
        let cutting = cut.is_some() && !panicked;
        let watch = &self.reducer.facts.follow.watch;
        let (held, watch) = (
            matches!(watch, Watch::Checking { .. }),
            watch.open().map(|open| open.request_id.number),
        );
        if !cutting {
            self.model.flight = None;
            self.model.dead.push(flight.request);
            if let (true, Some(group)) = (reported, flight.group) {
                self.model.ended_groups.insert(group);
            }
        }
        if panicked {
            self.reach("a program panicked");
        }
        let effects = self.input(Input::Finished {
            request: flight.request,
            reported,
            panicked,
            cut,
        });
        if cutting {
            assert!(
                matches!(effects.as_slice(), [Effect::Dispatch(program)] if matches!(program.work, Work::Finish { .. })),
                "a cut read's end dispatches its finish alone: {:?}",
                self.trail
            );
            self.reach("a cut read finished");
        } else {
            // The watch's answer and the query's reply, as failed, come
            // before the next program.
            let dispatched = effects
                .iter()
                .position(|effect| matches!(effect, Effect::Dispatch(_)))
                .unwrap_or(effects.len());
            let before: Vec<(u64, &'static str)> = replies(&effects[..dispatched]);
            if let Some(query) = flight.query {
                assert!(
                    before.contains(&(query, "failed")),
                    "query {query} answered as failed at its program's end: {:?}",
                    self.trail
                );
            }
            if let (true, Some(watch)) = (held, watch) {
                assert!(
                    before.contains(&(watch, "watch ended")),
                    "the held watch ends with its program: {:?}",
                    self.trail
                );
            }
        }
    }

    /// Advances the clocks and ticks.
    fn advance(&mut self, advance: Advance) {
        match advance {
            Advance::ToTimer { observed, wall } => {
                let Some(due) = self.reducer.next_timer(self.now()) else {
                    return;
                };
                let due = u64::try_from(due.as_micros()).expect("a time in reach");
                let delta = due.saturating_sub(self.at);
                self.at += delta;
                self.observed += delta + u64::from(observed) * 1_000;
                self.wall += delta + u64::from(wall) * 1_000;
                let before = format!("{:?}", self.reducer);
                let effects = self.call(None, Call::Tick);
                assert!(
                    !effects.is_empty() || format!("{:?}", self.reducer) != before,
                    "a tick at the time next_timer gave does nothing: {:?}",
                    self.trail
                );
            }
            Advance::By { at, observed, wall } => {
                self.at += u64::from(at) * 1_000;
                self.observed += u64::from(observed) * 1_000;
                self.wall += u64::from(wall) * 1_000;
                self.call(None, Call::Tick);
            }
        }
    }

    /// Gives `input`, with the asks it causes answered in line; every
    /// effect, in the order the shell carries them out.
    fn input(&mut self, input: Input) -> Vec<Effect> {
        self.call(Some(input), Call::Input)
    }

    /// Gives `input`, or ticks, then checks the call and carries out its
    /// effects as the shell does, answering each ask as it is reached;
    /// the hashes of every call's effects.
    fn call(&mut self, input: Option<Input>, kind: Call) -> Vec<Effect> {
        let now = self.now();
        let marks = self.reducer.facts.marks.clone();
        let flight_before = self.model.flight.clone();
        // The query of the watch a program holds.
        let watch_before = match &self.reducer.facts.follow.watch {
            watch @ Watch::Checking { .. } => watch.open().map(|open| open.request_id.number),
            _ => None,
        };
        let hangs = self.hangs(input.as_ref());
        self.moving_on = matches!(input, Some(Input::Judged { moved_on: true, .. }));
        let effects = match input {
            Some(input) => self.reducer.input(input, now),
            None => self.reducer.tick(now),
        };
        let told: Vec<isize> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Send(message) => match &**message {
                    OutpostToCore::NotResponding { window } => Some(*window),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(
            told, hangs,
            "Core is told of a hang exactly as it opens: {:?}",
            self.trail
        );
        self.log.push(hash(&format!("{effects:?}")));
        self.check_call(kind, &effects, &marks, flight_before.as_ref(), watch_before);
        // The shell carries out every effect of the call before any answer
        // comes back.
        for effect in &effects {
            self.apply(effect);
        }
        let mut answered = Vec::new();
        for effect in effects {
            let answers = self.answer(&effect);
            answered.push(effect);
            for answer in answers {
                let more = self.call(Some(answer), Call::Answer);
                answered.extend(more);
            }
        }
        answered
    }

    /// The reference model of each window's hang intervals, moved on by
    /// `input` before the reducer takes it: an interval opens at the first
    /// evidence of a hang, a hang seen by a check, of the program in flight,
    /// of a request no longer live or of the window in front, or an
    /// accepted commit naming its window hung, and closes only at evidence
    /// that the window answers, an accepted commit naming it answered or
    /// the window readable in front, or its destroy event; evidence of a
    /// hang from the program in flight in a window destroyed since it
    /// started opens nothing. Core is told of the window whose interval
    /// opens while no other is open, once per hang of the application; one
    /// opened while another is open is told nothing. A hang seen by the
    /// program in flight's check also sets its hang mark.
    fn hangs(&mut self, input: Option<&Input>) -> Vec<isize> {
        let mut opened = Vec::new();
        let mut open =
            |model: &mut Model, (window, of_flight): (Window, bool), reached: &mut Reached, how| {
                if of_flight
                    && model
                        .flight
                        .as_ref()
                        .is_some_and(|flight| flight.destroyed.contains(&window.get()))
                {
                    *reached
                        .entry("a hang after its window's destroy ignored")
                        .or_default() += 1;
                } else if model.hung.is_empty() {
                    model.hung.insert(window.get());
                    opened.push(window.get());
                    *reached.entry(how).or_default() += 1;
                } else if model.hung.insert(window.get()) {
                    *reached
                        .entry("a hang in a second window unsounded")
                        .or_default() += 1;
                }
            };
        match input {
            Some(Input::HangSeen { window, request }) if !self.reducer.stopping => {
                let of_flight = request.is_some()
                    && self.model.flight.as_ref().map(|flight| flight.request) == *request;
                if of_flight && let Some(flight) = self.model.flight.as_mut() {
                    flight.hang_mark = true;
                }
                let how = match (request, of_flight) {
                    (None, _) => "a hang opened in front",
                    (Some(_), true) => "a hang opened by a check",
                    (Some(_), false) => "a hang opened for a request no longer live",
                };
                open(
                    &mut self.model,
                    (*window, of_flight),
                    &mut self.reached,
                    how,
                );
            }
            Some(Input::FrontReadable { window }) if !self.reducer.stopping => {
                if self.model.hung.remove(&window.get()) {
                    self.reach("a hang ended in front");
                }
            }
            Some(Input::Commit { request, change }) if self.reducer.accepts(*request) => {
                if change.result.as_ref().is_some_and(|result| {
                    matches!(
                        result,
                        OutpostToCore::Reply {
                            outcome: QueryOutcome::Abandoned,
                            ..
                        }
                    )
                }) {
                    self.reach("a refusal answered abandoned");
                }
                match change.found {
                    Some((window, Found::Hung)) => open(
                        &mut self.model,
                        (window, true),
                        &mut self.reached,
                        "a hang opened by a commit",
                    ),
                    Some((window, Found::Answered)) if self.model.hung.remove(&window.get()) => {
                        self.reach("a hang ended by an answer");
                    }
                    Some((_, Found::Answered)) | None => {}
                }
            }
            Some(Input::Entry {
                entry:
                    Entry {
                        item:
                            Item::Msaa {
                                kind: WinKind::Destroy,
                                hwnd,
                                ..
                            },
                        ..
                    },
                ..
            }) => {
                if let Some(flight) = self.model.flight.as_mut() {
                    flight.destroyed.insert(*hwnd);
                }
                if self.model.hung.remove(hwnd) {
                    self.reach("a hang ended by a destroy event");
                }
            }
            _ => {}
        }
        opened
    }

    /// Checks one call's effects as a whole, against the state before it:
    /// the invariants a call keeps, and the order of a tick's effects.
    #[expect(clippy::too_many_lines, reason = "one check per invariant")]
    fn check_call(
        &mut self,
        kind: Call,
        effects: &[Effect],
        marks: &Marks,
        flight: Option<&Flight>,
        watch_before: Option<u64>,
    ) {
        let trail = &self.trail;
        let reducer = &self.reducer;
        // A poll read holds the watch it reads.
        if effects.iter().any(|effect| {
            matches!(effect, Effect::Dispatch(program) if matches!(program.work, Work::PollRead))
        }) {
            assert!(
                matches!(reducer.facts.follow.watch, Watch::Checking { .. }),
                "a poll read with no watch to read: {trail:?}"
            );
        }
        // The marks, when given, first and only when they changed.
        let given: Vec<usize> = positions(effects, |effect| matches!(effect, Effect::Marks(_)));
        let changed = reducer.facts.marks != *marks;
        assert_eq!(
            given,
            if changed { vec![0] } else { vec![] },
            "the marks are given first, exactly when they changed: {trail:?}"
        );
        // At most one ask of each kind, and at most one program.
        let asks = positions(effects, |effect| matches!(effect, Effect::AskJudge { .. }));
        assert!(asks.len() <= 1, "two checks in one transition: {trail:?}");
        let front_checks = positions(effects, |effect| matches!(effect, Effect::CheckFront));
        // The window in front is checked exactly when a tick finds the
        // first marker's check due, and never by any other call.
        let front_due = kind == Call::Tick
            && !self.model.stopping
            && self
                .model
                .markers
                .first()
                .is_some_and(|&(_, due, _)| self.at >= due);
        assert_eq!(
            front_checks.len(),
            usize::from(front_due),
            "the front checked exactly when the first marker's check is due: {trail:?}"
        );
        let dispatches = positions(effects, |effect| matches!(effect, Effect::Dispatch(_)));
        assert!(
            dispatches.len() <= 1,
            "two dispatches in one transition: {trail:?}"
        );
        // No dispatch after a release, in the call that asked for it.
        if let Some(&release) =
            positions(effects, |effect| matches!(effect, Effect::Release { .. })).first()
        {
            assert!(
                dispatches.iter().all(|&dispatch| dispatch < release),
                "a dispatch while releasing: {trail:?}"
            );
        }
        // The idle answer only with no deferred work: the program in flight
        // and the watch as they stand after the call, unless a program was
        // dispatched after the answer, for an entry that came after its
        // round.
        if let Some(&idle) = positions(effects, |effect| {
            matches!(effect, Effect::Send(message) if matches!(**message, OutpostToCore::IdleAnswer { .. }))
        })
        .first()
        {
            assert!(
                !reducer.hold.holding(),
                "an idle answer while holding: {trail:?}"
            );
            if dispatches.iter().all(|&dispatch| dispatch < idle) {
                assert!(
                    reducer.facts.follow.watch.is_none()
                        && !reducer
                            .flight
                            .running()
                            .is_some_and(|(_, current)| current.deferred),
                    "an idle answer with the watch open or a deferred program: {trail:?}"
                );
            }
        }
        // After close, no tick abandons or asks.
        if self.model.stopping && kind == Call::Tick {
            assert!(
                !effects.iter().any(|effect| matches!(
                    effect,
                    Effect::Abandon { .. } | Effect::AskJudge { .. } | Effect::CheckFront
                )),
                "a tick after close abandons or asks: {trail:?}"
            );
        }
        // A tick decides the program in flight's deadline, then its check.
        if kind == Call::Tick
            && !self.model.stopping
            && let Some(flight) = flight
        {
            let decided = effects.iter().position(|effect| {
                matches!(effect, Effect::Abandon { .. } | Effect::AskJudge { .. })
            });
            let decision = decided.map(|at| &effects[at]);
            if self.at >= flight.deadline {
                assert!(
                    matches!(decision, Some(Effect::Abandon { .. })),
                    "a tick past the deadline abandons: {trail:?}"
                );
            } else {
                assert!(
                    matches!(decision, None | Some(Effect::AskJudge { .. })),
                    "a tick before the deadline does not abandon: {trail:?}"
                );
                if decision.is_some() {
                    assert!(
                        flight.window && self.at >= flight.started + GRACE_US,
                        "the moved-on rule judged before its grace or with no window: {trail:?}"
                    );
                }
            }
        }
        // An abandonment answers the watch its program held and the query
        // it answered before anything is dispatched.
        if let Some(abandoned) = effects
            .iter()
            .position(|effect| matches!(effect, Effect::Abandon { .. }))
        {
            let flight = flight.expect("an abandonment of a program in flight");
            let replied = replies(&effects[abandoned..]);
            if let Some(query) = flight.query {
                assert!(
                    replied.contains(&(query, "abandoned")),
                    "an abandonment answers its program's query: {trail:?}"
                );
            }
            // Unless the publication the abandonment waited for ended it
            // first, in the same call.
            if let Some(watch) = watch_before
                && !replies(&effects[..abandoned])
                    .iter()
                    .any(|(replied, _)| *replied == watch)
            {
                assert!(
                    replied.contains(&(watch, "abandoned")),
                    "an abandonment answers the watch its program held: {trail:?}"
                );
            }
            assert!(
                dispatches.iter().all(|&dispatch| dispatch > abandoned),
                "a dispatch before the abandonment: {trail:?}"
            );
            if let Effect::Abandon { dead, .. } = &effects[abandoned] {
                assert!(
                    reducer
                        .facts
                        .follow
                        .owed
                        .as_ref()
                        .is_none_or(|owed| owed.attempting != Some(*dead)),
                    "owed work names the dead generation as attempting it: {trail:?}"
                );
            }
        }
        // No time for the watch while a program, a release or the close is
        // under way.
        if !reducer.facts.follow.watch.is_none()
            && (reducer.busy() || reducer.releasing || reducer.stopping)
        {
            let mut without = reducer.clone();
            without.facts.follow.watch = Watch::None;
            assert_eq!(
                reducer.next_timer(self.now()),
                without.next_timer(self.now()),
                "the watch timed while busy, releasing or stopping: {trail:?}"
            );
        }
        // The watch is held only by the program in flight.
        assert!(
            !matches!(reducer.facts.follow.watch, Watch::Checking { .. }) || reducer.busy(),
            "the watch held with no program in flight: {trail:?}"
        );
    }

    /// Carries out `effect` in the model, checking it against the model.
    #[expect(clippy::too_many_lines, reason = "one arm for each kind of effect")]
    fn apply(&mut self, effect: &Effect) {
        let trail = self.trail.clone();
        match effect {
            Effect::Dispatch(program) => self.dispatched(program),
            Effect::Send(message) => match &**message {
                OutpostToCore::Reply {
                    request_id,
                    outcome,
                    ..
                } => {
                    let number = request_id.number;
                    let count = self.model.replies.entry(number).or_default();
                    *count += 1;
                    assert_eq!(*count, 1, "a second reply to {number}: {trail:?}");
                    assert!(
                        self.model.queries.remove(&number),
                        "a reply to {number}, not open: {trail:?}"
                    );
                    if self.model.late.contains(&number) && outcome_name(outcome) != "not started" {
                        // Answered with nothing read, never walked.
                        assert_eq!(
                            outcome,
                            &QueryOutcome::Done(QueryResult::ShellItems(Vec::new())),
                            "{trail:?}"
                        );
                        self.took(number);
                        self.reach("a late shell query answered");
                    }
                    if let Some(flight) = self.model.flight.as_mut()
                        && flight.query == Some(number)
                    {
                        flight.query = None;
                    }
                }
                OutpostToCore::IdleAnswer { round } => {
                    let count = self.model.answered_rounds.entry(*round).or_default();
                    *count += 1;
                    assert_eq!(
                        *count, 1,
                        "a second idle answer to round {round}: {trail:?}"
                    );
                    self.reach("an idle round answered");
                }
                _ => {}
            },
            Effect::Abandon {
                dead,
                replacement,
                on_hung,
                ..
            } => {
                let flight = self
                    .model
                    .flight
                    .take()
                    .expect("an abandonment of a program in flight");
                assert_eq!(
                    (*dead, *replacement),
                    (self.model.generation, self.model.generation + 1),
                    "an abandonment names the generation in charge and the next: {trail:?}"
                );
                assert_eq!(
                    *on_hung, flight.hang_mark,
                    "an abandoned worker is parked on a hung window exactly when its program's hang mark is set: {trail:?}"
                );
                self.model.generation += 1;
                self.model.dead.push(flight.request);
                if let Some((_, attempting)) = self.model.owed
                    && attempting == Some(*dead)
                {
                    self.model.owed = None;
                    if flight.kind == Kind::Owed {
                        self.reach("owed work abandoned");
                    }
                }
                self.model.abandoned = true;
                if *on_hung {
                    self.reach("abandoned parked on a hung window");
                }
                if self.at >= flight.deadline {
                    self.reach("abandoned at the deadline");
                } else {
                    assert!(
                        flight.window && self.moving_on,
                        "abandoned before its deadline but by the user moving on: {trail:?}"
                    );
                    self.reach("abandoned as moved on");
                }
            }
            Effect::Release { live, .. } => {
                assert!(
                    self.model.flight.is_none() && live.is_empty(),
                    "a release with a program in flight: {trail:?}"
                );
                self.model.releasing = true;
            }
            Effect::AskJudge { request, .. } => {
                let flight = self.model.flight.as_ref().expect("a program asked about");
                assert_eq!(*request, flight.request, "{trail:?}");
                assert!(
                    flight.window,
                    "asked about a program with no window: {trail:?}"
                );
            }
            Effect::CheckFront => {
                let at = self.at;
                let marker = self
                    .model
                    .markers
                    .first_mut()
                    .expect("a front check with a marker waiting");
                let again = marker.2;
                marker.1 = at + GRACE_US;
                marker.2 = true;
                self.reach("the front checked");
                if again {
                    self.reach("the front checked again while its marker waits");
                }
            }
            Effect::Settled(id) => {
                assert!(
                    self.model.settles.remove(id),
                    "settle {id} answered, not open: {trail:?}"
                );
                let count = self.model.settled.entry(*id).or_default();
                *count += 1;
                assert_eq!(*count, 1, "a settle answered twice: {trail:?}");
                self.reach("a settle answered");
            }
            _ => {}
        }
    }

    /// Notes a request issued: numbered after every request issued before
    /// it, so no number recurs.
    fn issued(&mut self, request: Request) {
        assert!(
            self.model
                .numbers
                .last()
                .is_none_or(|&last| request.number > last),
            "request {request:?} numbered before one issued earlier: {:?}",
            self.trail
        );
        self.model.numbers.insert(request.number);
    }

    /// Notes query `number` taken from the intake: every query that arrived
    /// before it has been taken, or replied to.
    fn took(&mut self, number: u64) {
        for earlier in self.model.arrived.iter().take_while(|&&q| q != number) {
            assert!(
                self.model.taken.contains(earlier) || self.model.replies.contains_key(earlier),
                "query {number} taken before {earlier}: {:?}",
                self.trail
            );
        }
        self.model.taken.insert(number);
    }

    /// The model's part of a dispatch.
    #[expect(clippy::too_many_lines, reason = "one arm for each kind of program")]
    fn dispatched(&mut self, program: &Program) {
        let trail = self.trail.clone();
        let kind = kind_of(program);
        assert!(
            !self.model.releasing,
            "a dispatch while releasing: {trail:?}"
        );
        if kind == Kind::Finish {
            let flight = self
                .model
                .flight
                .as_mut()
                .expect("a finish of the program in flight");
            assert_eq!(program.request.generation, flight.request.generation);
            let old = std::mem::replace(&mut flight.request, program.request);
            flight.kind = Kind::Finish;
            self.model.dead.push(old);
            self.issued(program.request);
            assert_eq!(
                self.reducer.flight.deadline(),
                self.model.flight.as_ref().map(|flight| flight.deadline),
                "a finish keeps its read's deadline: {trail:?}"
            );
            return;
        }
        assert!(
            self.model.flight.is_none(),
            "a dispatch with a program in flight: {trail:?}"
        );
        assert!(
            !self.model.stopping,
            "a dispatch from the intake after close: {trail:?}"
        );
        if self
            .model
            .owed
            .is_some_and(|(_, attempting)| attempting.is_none())
        {
            assert_eq!(kind, Kind::Owed, "owed work goes first: {trail:?}");
        }
        assert_eq!(
            program.request.generation, self.model.generation,
            "a dispatch under a generation not in charge: {trail:?}"
        );
        self.issued(program.request);
        let mut query = None;
        // Every program is judged but a release's drop: by the window its
        // entry names, or by the one its worker names.
        let window = !matches!(kind, Kind::Drop);
        let mut group = None;
        match kind {
            Kind::Entry | Kind::Key | Kind::EditCaret => {
                let Work::Entry {
                    entry,
                    group: group_place,
                    ..
                } = &program.work
                else {
                    unreachable!("an entry's program");
                };
                let trace = entry.trace;
                let pending =
                    self.model.pending.remove(&trace).unwrap_or_else(|| {
                        panic!("dispatched entry {trace}, not pending: {trail:?}")
                    });
                assert!(
                    !matches!(pending.what, What::ShellLate),
                    "a late shell query walked: {trail:?}"
                );
                if let Some(number) = pending.query {
                    self.took(number);
                    query = Some(number);
                }
                if let Some(tried) = group_place {
                    assert!(
                        !self.model.ended_groups.contains(&tried.number),
                        "a try of a group already reported or published: {trail:?}"
                    );
                    group = Some(tried.number);
                }
                if group_place.is_some_and(|group| group.rank > 0) {
                    self.reach("a focus group continued");
                    if self.model.abandoned {
                        self.reach("a focus group continued after an abandonment");
                    }
                }
                self.reach("an entry dispatched");
            }
            Kind::Noted => {
                assert!(self.model.noted, "a noted caret read, not noted: {trail:?}");
                self.model.noted = false;
            }
            Kind::Owed => {
                let (instance, attempting) = self.model.owed.expect("owed work");
                assert_eq!(attempting, None, "owed work tried twice: {trail:?}");
                assert!(
                    self.model
                        .owed_tries
                        .insert((instance, self.model.generation)),
                    "owed work tried twice by one generation: {trail:?}"
                );
                self.model.owed = Some((instance, Some(self.model.generation)));
                self.reach("owed work dispatched");
            }
            Kind::Drop => {
                assert!(self.model.to_drop, "a drop with nothing to drop: {trail:?}");
                self.model.to_drop = false;
                self.reach("a drop dispatched");
            }
            Kind::PollRead => self.reach("the watch polled"),
            Kind::Finish => unreachable!("handled above"),
        }
        let deadline = self
            .reducer
            .flight
            .deadline()
            .expect("a program dispatched under a deadline");
        assert!(
            deadline > self.at,
            "a program dispatched under a deadline already past: {trail:?}"
        );
        self.model.abandoned = false;
        self.model.flight = Some(Flight {
            request: program.request,
            kind,
            query,
            for_query: query.is_some(),
            group,
            hang_mark: false,
            started: self.at,
            deadline,
            window,
            copy: program.facts.clone(),
            uncompared: false,
            destroyed: BTreeSet::new(),
        });
    }

    /// The shell's answers to `effect`, if it asks, from the action's
    /// answers, in the order the shell gives them.
    fn answer(&mut self, effect: &Effect) -> Vec<Input> {
        match effect {
            Effect::AskJudge { request, window } => {
                let named = match self.asks.named {
                    0 => None,
                    named => Window::new(HANGING[usize::from(named - 1)]),
                };
                let Some(window) = window.or(named) else {
                    return vec![Input::Judged {
                        request: *request,
                        moved_on: false,
                    }];
                };
                let hang = self.asks.hung.then_some(Input::HangSeen {
                    window,
                    request: Some(*request),
                });
                hang.into_iter()
                    .chain([Input::Judged {
                        request: *request,
                        moved_on: self.asks.moved_on,
                    }])
                    .collect()
            }
            Effect::CheckFront => {
                let window = |index: u8| {
                    Window::new(HANGING[usize::from(index % 2)]).expect("a hang case's window")
                };
                match self.asks.front {
                    0 => Vec::new(),
                    hung @ 1..=2 => vec![Input::HangSeen {
                        window: window(hung - 1),
                        request: None,
                    }],
                    readable => vec![Input::FrontReadable {
                        window: window(readable - 3),
                    }],
                }
            }
            Effect::Release { .. } => {
                self.model.releasing = false;
                self.model.to_drop |= self.asks.objects;
                self.reach("a release answered");
                vec![Input::Released {
                    nodes: if self.asks.release_node {
                        [1].into_iter().collect()
                    } else {
                        BTreeSet::new()
                    },
                    anchors: BTreeSet::new(),
                    objects: self.asks.objects,
                }]
            }
            _ => Vec::new(),
        }
    }

    /// The reducer and the model agree, after a step.
    fn compare(&self) {
        let trail = &self.trail;
        let reducer = &self.reducer;
        let model = &self.model;
        assert_eq!(reducer.generation(), model.generation, "{trail:?}");
        assert_eq!(
            reducer
                .queries()
                .iter()
                .map(|request_id| request_id.number)
                .collect::<BTreeSet<_>>(),
            model.queries,
            "the open queries: {trail:?}"
        );
        assert_eq!(
            reducer.settles(),
            model.settles,
            "the open settles: {trail:?}"
        );
        assert_eq!(
            reducer
                .flight
                .running()
                .map(|(request, current)| (request, current.hang_mark)),
            model
                .flight
                .as_ref()
                .map(|flight| (flight.request, flight.hang_mark)),
            "the program in flight, its hang mark, and no walk published: {trail:?}"
        );
        if !model.stopping
            && let Some(&(_, front, _)) = model.markers.first()
        {
            assert!(
                reducer
                    .next_timer(self.now())
                    .is_some_and(|due| due <= Duration::from_micros(front)),
                "a marker waiting has its front check timed: {trail:?}"
            );
        }
        assert_eq!(
            reducer
                .facts
                .follow
                .owed
                .as_ref()
                .map(|owed| owed.attempting),
            model.owed.map(|(_, attempting)| attempting),
            "the owed work: {trail:?}"
        );
        assert_eq!(
            reducer.hold.holding(),
            model.hold == 1,
            "the hold: {trail:?}"
        );
        assert!(!reducer.releasing, "a release left unanswered: {trail:?}");
    }

    /// Ends the case: every program in flight finished, every marker
    /// dropped, since the window in front is checked for as long as one
    /// waits, and every deadline ticked past, then the close and its last
    /// programs; then every query
    /// has had exactly one reply or is left to the shell's token, and
    /// every settle exactly one answer or was dropped by the close.
    fn drain(&mut self) {
        let finish = Action::Finished {
            reported: false,
            panicked: false,
            cut: false,
        };
        for _ in 0..400 {
            if self.model.flight.is_some() {
                self.step(finish);
            } else if !self.model.stopping && !self.model.markers.is_empty() {
                self.step(Action::Entry {
                    what: What::Dropped,
                    from: From::Other,
                    more: false,
                });
            } else if !self.model.stopping && self.reducer.next_timer(self.now()).is_some() {
                self.step(Action::Advance(Advance::ToTimer {
                    observed: 0,
                    wall: 0,
                }));
            } else if !self.model.stopping {
                self.step(Action::Close);
            } else {
                break;
            }
        }
        assert!(
            !self.reducer.busy(),
            "a program left in flight: {:?}",
            self.trail
        );
        for number in &self.model.arrived {
            let replied = self.model.replies.get(number).copied().unwrap_or(0);
            let left = self.reducer.queries().contains(&RequestId::shell(*number));
            // Only a query never taken from the intake, or the watch's, is
            // left to the shell's token at the close.
            assert!(
                !left
                    || !self.model.taken.contains(number)
                    || self
                        .reducer
                        .facts
                        .follow
                        .watch
                        .open()
                        .map(|open| open.request_id.number)
                        == Some(*number),
                "query {number} taken and left unanswered: {:?}",
                self.trail
            );
            assert!(
                (replied == 1) != left,
                "query {number} replied {replied} times, left to the token {left}: {:?}",
                self.trail
            );
        }
        assert!(self.reducer.settles().is_empty(), "{:?}", self.trail);
    }
}

/// The positions in `effects` of those `wanted` matches.
fn positions(effects: &[Effect], wanted: impl Fn(&Effect) -> bool) -> Vec<usize> {
    effects
        .iter()
        .enumerate()
        .filter_map(|(at, effect)| wanted(effect).then_some(at))
        .collect()
}

/// Runs the generated cases `config` draws, each twice from a new reducer,
/// shrinking a failure to its smallest case; what the cases reached.
///
/// # Panics
///
/// On a failing case, naming the smallest.
pub(super) fn run_generated(config: Config, again: &str) -> Reached {
    let reached = RefCell::new(Reached::new());
    let mut runner = TestRunner::new(config);
    let strategy = proptest::collection::vec((action(), asks()), 1..48);
    let result = runner.run(&strategy, |actions| {
        let first = Run::case(&actions);
        let second = Run::case(&actions);
        assert_eq!(first.log, second.log, "two runs of one case differ");
        let mut reached = reached.borrow_mut();
        for (what, count) in first.reached {
            *reached.entry(what).or_default() += count;
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
            Action::Entry { what, from, more } => format!(
                "Action::Entry {{ what: What::{what:?}, from: From::{from:?}, more: {more} }}"
            ),
            Action::Commit(commit) => format!("Action::Commit(Commit::{commit:?})"),
            Action::Stale(stale) => format!("Action::Stale(Stale::{stale:?})"),
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
        "the smallest failing case, as a line for `fixed()` in crates/verbatim-outpost-logic/src/tests/reducer.rs:\n        vec![{}],\n\
         the fixed cases alone: cargo test -p verbatim-outpost-logic --lib -- --exact tests::reducer::fixed_cases_keep_the_reducers_rules\n\
         the run that found it again: {again}",
        steps.join(", ")
    )
}

/// The gate's seed and case count.
const SEED: u64 = 20_261_102;
const CASES: u32 = 1_500;

/// Generated cases of entries of every kind from every source, commits,
/// hangs seen by a check, in front or of a request no longer live, readable
/// windows in front, programs ended, cut and panicked, inputs of
/// requests no longer live, verdicts, the hold, cancels, the close and
/// time on three clocks, with every ask answered in line, keep the
/// reference model's state and every invariant and postcondition the
/// design lists after every call, end with every query answered once or
/// left to the shell's token, give the same effects when run twice, and
/// reach every listed state.
#[test]
fn generated_cases_keep_the_reducers_rules() {
    let reached = run_generated(
        super::explore::config(SEED, CASES),
        "cargo test -p verbatim-outpost-logic --lib -- --exact tests::reducer::generated_cases_keep_the_reducers_rules",
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
    front: 0,
    release_node: false,
    objects: false,
    hung: false,
    named: 0,
};

/// An entry of `what` from the listener's facts or Core, alone in its
/// drain.
const fn arrives(what: What) -> (Action, Asks) {
    (
        Action::Entry {
            what,
            from: From::Other,
            more: false,
        },
        PLAIN,
    )
}

/// The cases the generated tests found, each the smallest a failure
/// shrank to, kept as lists of actions: a probe that left the verdicts as
/// they were sent as a change
/// (`books/dev/src/reference/crates/verbatim-outpost-logic.md`); and from
/// the mutation baseline, a take of a node no watch is on while one waits.
fn fixed() -> Vec<Vec<(Action, Asks)>> {
    let uia = |dead| Action::Verdict { dead, change: 0 };
    let finished = Action::Finished {
        reported: false,
        panicked: false,
        cut: false,
    };
    vec![
        vec![
            arrives(What::CaretKey),
            (Action::Commit(Commit::Own(false)), PLAIN),
            (finished, PLAIN),
            arrives(What::NameChange(0)),
            (Action::Commit(Commit::TakeOther), PLAIN),
        ],
        vec![
            (uia(false), PLAIN),
            (Action::Close, PLAIN),
            (uia(false), PLAIN),
            (uia(false), PLAIN),
        ],
    ]
}

/// Every fixed case keeps the reducer's rules, and gives the same effects
/// when run twice.
#[test]
fn fixed_cases_keep_the_reducers_rules() {
    for case in fixed() {
        assert_eq!(Run::case(&case).log, Run::case(&case).log, "{case:?}");
    }
}

/// The source an entry from `from` is given as.
fn source_of(from: From) -> Source {
    match from {
        From::Other => Source::Other,
        From::Hook => Source::Hook,
        From::Listener => Source::Listener,
    }
}

/// A UIA element's cached parts.
fn snapshot(runtime_id: Vec<i32>) -> UiaSnapshotFact {
    UiaSnapshotFact {
        runtime_id,
        role: Role::ListItem,
        name: None,
        value: None,
        states: StateSet::new(),
        details: verbatim_model::NodeDetails::default(),
    }
}

/// The listener's read of a UIA focus in window 0x10, resolving `marker`.
fn focus_read(marker: u64) -> DeliveredFact {
    DeliveredFact::UiaFocusRead {
        marker,
        read: Box::new(UiaFocusRead {
            hwnd: 0x10,
            snapshot: snapshot(vec![42, 2]),
            ancestors: Vec::new(),
            ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
            selected_child: None,
            value_from_selection: false,
            window: Some(0x10),
            same_as_last: None,
        }),
        focus: true,
    }
}

/// A focus report of node `id` in top-level window `top`, observed at
/// `at_us`, a whole-window focus or not, adopted and followed, owing a
/// terminal's first read when `owes`.
fn report(id: u64, top: Option<isize>, at_us: u64, whole: bool, owes: bool) -> Change {
    let mut change = focus_change(id, owes);
    if let Some(OutpostToCore::Event {
        window,
        observed_at_us,
        event: NormalizedEvent::FocusChanged { whole_window, .. },
        ..
    }) = change.result.as_mut()
    {
        *window = top.map(facts_in);
        *observed_at_us = at_us;
        *whole_window = whole;
    }
    change
}
