//! The intake's generated test (`docs/design/logic-tests.md`, "The
//! intake"): lists of pushes, hand-outs, cancels, a noted caret kept for
//! when nothing else is ready, and the close, given to the intake alone,
//! with the reducer's one use of `push_ahead` (a barrier reached with a
//! noted caret kept goes behind it) done as the reducer does it. It checks
//! only three rules, from what the test itself knows of each entry it
//! pushed, never from the intake's own keys:
//!
//! - each entry pushed is handed out at most once, and by the end exactly
//!   once or dropped for a reason the test names: replaced by a newer
//!   entry of its object, kind and thread with no idle barrier between,
//!   past the batch limits, a focus on a window's client area removed by
//!   the window's foreground change raised after it, an older menu opening,
//!   cancelled, closed, or a marker or a drop, which is never handed out;
//! - nothing is handed out ahead of an earlier arrival, but under the
//!   documented exceptions, each named: a run of MSAA focus events and
//!   foreground changes tried at the place of its newest, a menu opening
//!   handed out at the end of its batch, a UIA focus's read taking its
//!   marker's place, and the noted caret and its barrier put ahead;
//! - a marker holds back everything that arrived after it until a read, a
//!   drop or the end of the listener's reads resolves it.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestRunner};
use verbatim_model::protocol::{UiaFocusRead, UiaSnapshotFact};

use super::*;
use crate::intake::{Intake, Marks, Planned};

/// What an entry is, as the test pushes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// An MSAA name change of a child of a window, raised by a thread.
    NameChange { window: u8, child: u8, thread: u8 },
    /// An MSAA focus on a child of window 9, raised by a thread.
    Focus { child: u8, thread: u8 },
    /// A foreground change into window 9 or 10, raised by a thread.
    Foreground { window: u8, thread: u8 },
    /// A console window's update (0) or caret (1).
    Console(u8),
    /// A UIA property change of one of two elements.
    Uia(u8),
    /// A menu opening in window 9 or 10.
    Menu(u8),
    /// Core's query.
    Query,
    /// Core's idle round.
    Round,
    /// Core's held nodes.
    NodesHeld,
    /// A UIA focus's marker.
    Marker,
    /// The read of the oldest open marker, of one of two elements.
    Read(u8),
    /// The drop of the oldest open marker.
    Dropped,
    /// A read whose marker the intake never saw, of one of two elements.
    LoneRead(u8),
    /// The end of the listener's reads.
    ReadsEnded,
    /// A held terminal's noted caret, kept apart.
    Noted,
}

/// One step of a case.
#[derive(Clone, Copy, Debug)]
enum Action {
    /// An entry arrives.
    Push(Kind),
    /// The reducer takes the next planned item, if any.
    Next,
    /// Core withdraws the oldest query not yet handed out.
    Cancel,
    /// A held terminal's caret is noted, for when nothing else is ready.
    Noted,
    /// The outpost closes.
    Close,
    /// A burst of a dozen name changes from one thread, of many children.
    Burst(u8),
}

fn kind() -> impl Strategy<Value = Kind> {
    use proptest::prelude::*;
    prop_oneof![
        4 => (0..2u8, 1..3u8, 3..5u8)
            .prop_map(|(window, child, thread)| Kind::NameChange { window, child, thread }),
        4 => (0..3u8, 3..5u8).prop_map(|(child, thread)| Kind::Focus { child, thread }),
        2 => (0..2u8, 3..5u8).prop_map(|(window, thread)| Kind::Foreground { window, thread }),
        1 => (0..2u8).prop_map(Kind::Console),
        2 => (0..2u8).prop_map(Kind::Uia),
        1 => (0..2u8).prop_map(Kind::Menu),
        2 => Just(Kind::Query),
        1 => Just(Kind::Round),
        1 => Just(Kind::NodesHeld),
        2 => Just(Kind::Marker),
        2 => (0..2u8).prop_map(Kind::Read),
        1 => (0..2u8).prop_map(Kind::LoneRead),
        1 => Just(Kind::Dropped),
        1 => Just(Kind::ReadsEnded),
    ]
}

fn action() -> impl Strategy<Value = Action> {
    use proptest::prelude::*;
    prop_oneof![
        10 => kind().prop_map(Action::Push),
        6 => Just(Action::Next),
        1 => Just(Action::Cancel),
        1 => Just(Action::Noted),
        1 => Just(Action::Close),
        1 => (3..5u8).prop_map(Action::Burst),
    ]
}

/// One entry the test pushed.
#[derive(Clone, Debug)]
struct Pushed {
    kind: Kind,
    /// Its place in the order of arrival: its own, or for a read, its
    /// marker's.
    place: u64,
    /// The query's number, for a query.
    query: Option<u64>,
    /// Its place among the hand-outs, once handed out, and the batch.
    handed: Option<(u64, u64)>,
    /// The members of the focus group it was handed out in, by trace.
    group: Vec<u64>,
    /// Whether it was put ahead of everything waiting.
    ahead: bool,
    cancelled: bool,
    closed: bool,
}

/// One run of one case.
#[derive(Default)]
struct Run {
    intake: Intake,
    marks: Marks,
    /// Every entry pushed, by trace.
    pushed: BTreeMap<u64, Pushed>,
    /// The markers not yet resolved, oldest first.
    open_markers: Vec<u64>,
    /// When each marker was resolved, by its place among the hand-outs.
    next: u64,
    handouts: u64,
    closed: bool,
    /// The noted caret kept, by trace.
    noted: Option<u64>,
    /// The model: what waits, by trace in its order; the batch being handed
    /// out, by trace; the number of the last batch; and the entries dropped,
    /// with why.
    waiting: Vec<u64>,
    batch: BTreeSet<u64>,
    last_batch: u64,
    fate: BTreeMap<u64, &'static str>,
    /// Every hand-out, by its `Debug` text, for the double run.
    log: Vec<String>,
    reached: BTreeMap<&'static str, u64>,
    trail: Vec<Action>,
}

/// The lifecycle states the gate's cases must each reach.
const LISTED: [&str; 11] = [
    "a focus group of several",
    "replaced",
    "past the batch limits",
    "a client focus removed by its foreground change",
    "an older menu opening",
    "a menu opening at its batch's end",
    "cancelled",
    "closed",
    "a read in its marker's place",
    "a barrier behind its noted caret",
    "held back by a marker",
];

impl Run {
    fn fresh(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn reach(&mut self, what: &'static str) {
        *self.reached.entry(what).or_default() += 1;
    }

    fn case(actions: &[Action]) -> Self {
        let mut run = Self::default();
        for &action in actions {
            run.step(action);
        }
        run.drain();
        run
    }

    fn step(&mut self, action: Action) {
        self.trail.push(action);
        match action {
            Action::Push(kind) => self.push(kind),
            Action::Burst(thread) => {
                for at in 0..12u8 {
                    self.push(Kind::NameChange {
                        window: at % 2,
                        child: 1 + at / 2,
                        thread,
                    });
                }
            }
            Action::Next => self.take(),
            Action::Cancel => {
                let oldest = self
                    .pushed
                    .iter()
                    .find(|(_, pushed)| {
                        pushed.query.is_some() && pushed.handed.is_none() && !pushed.cancelled
                    })
                    .map(|(&trace, pushed)| (trace, pushed.query, pushed.closed));
                let Some((trace, Some(number), closed)) = oldest else {
                    return;
                };
                let found = self.intake.cancel(RequestId::shell(number));
                if !closed {
                    assert_eq!(
                        found,
                        Some(TraceId::from_raw(trace)),
                        "a query waiting is withdrawn: {:?}",
                        self.trail
                    );
                    self.pushed.get_mut(&trace).expect("pushed").cancelled = true;
                    self.waiting.retain(|&waiting| waiting != trace);
                    self.batch.remove(&trace);
                }
            }
            Action::Noted => {
                let trace = self.fresh();
                let place = self.next;
                self.pushed.insert(
                    trace,
                    Pushed {
                        kind: Kind::Noted,
                        place,
                        query: None,
                        handed: None,
                        group: Vec::new(),
                        ahead: true,
                        cancelled: false,
                        closed: self.closed,
                    },
                );
                // A newer note replaces the one kept.
                if let Some(old) = self.noted.replace(trace) {
                    self.fate.insert(old, "a noted caret replaced");
                }
                self.intake.when_idle(Entry::own(
                    Item::NotedCaret {
                        node_id: NodeId::new(3),
                    },
                    TraceId::from_raw(trace),
                    trace,
                ));
            }
            Action::Close => {
                self.closed = true;
                let _ = self.intake.close();
                for pushed in self.pushed.values_mut() {
                    if pushed.handed.is_none() {
                        pushed.closed = true;
                    }
                }
                self.noted = None;
                self.waiting.clear();
                self.batch.clear();
                self.reach("closed");
            }
        }
    }

    #[expect(clippy::too_many_lines, reason = "one arm for each kind of entry")]
    fn push(&mut self, kind: Kind) {
        let trace = self.fresh();
        let raised = u32::try_from(trace).expect("a small trace");
        let mut query = None;
        let mut place = trace;
        let mut resolved = None;
        let item = match kind {
            Kind::NameChange {
                window,
                child,
                thread,
            } => Item::Msaa {
                kind: WinKind::NameChange,
                hwnd: 9 + isize::from(window),
                id_object: -4,
                id_child: i32::from(child),
                raiser: u32::from(thread),
            },
            Kind::Focus { child, thread } => Item::Fact(
                DeliveredFact::MsaaFocus {
                    hwnd: 9,
                    id_object: -4,
                    id_child: i32::from(child),
                },
                Some(WinEventStamp {
                    thread: u32::from(thread),
                    time: raised,
                }),
            ),
            Kind::Foreground { window, thread } => Item::Fact(
                DeliveredFact::Foreground {
                    hwnd: 9 + isize::from(window),
                },
                Some(WinEventStamp {
                    thread: u32::from(thread),
                    time: raised,
                }),
            ),
            Kind::Console(which) => Item::Msaa {
                kind: if which == 0 {
                    WinKind::ConsoleUpdate
                } else {
                    WinKind::ConsoleCaret
                },
                hwnd: 20,
                id_object: 0,
                id_child: 0,
                raiser: 5,
            },
            Kind::Uia(element) => Item::Uia(UiaEvent {
                kind: UiaKind::Property(30_005),
                parts: snapshot(vec![1, i32::from(element)]),
                hwnd: 0,
                focus_window: 0,
                element: None,
            }),
            Kind::Menu(window) => Item::Fact(
                DeliveredFact::MenuPopup {
                    hwnd: 9 + isize::from(window),
                    id_object: -4,
                    id_child: 0,
                },
                None,
            ),
            Kind::Query => {
                query = Some(trace);
                Item::Query {
                    request_id: RequestId::shell(trace),
                    query: Query::FocusNow,
                }
            }
            Kind::Round => Item::Barrier {
                asker: Asker::Round(trace),
                behind_noted: false,
            },
            Kind::NodesHeld => Item::NodesHeld {
                nodes: Vec::new(),
                anchors: Vec::new(),
                acknowledged: trace,
            },
            Kind::Marker => {
                if !self.closed {
                    self.open_markers.push(trace);
                }
                Item::Fact(DeliveredFact::UiaFocusMarker { marker: trace }, None)
            }
            Kind::Read(element) => {
                resolved = self.resolve_oldest();
                let marker = resolved.unwrap_or_else(|| self.fresh());
                if let Some(resolved) = self.pushed.get(&marker) {
                    place = resolved.place;
                    self.reach("a read in its marker's place");
                }
                Item::Fact(focus_read(marker, element), None)
            }
            Kind::Dropped => {
                resolved = self.resolve_oldest();
                let marker = resolved.unwrap_or_else(|| self.fresh());
                Item::Fact(
                    DeliveredFact::UiaFocusDropped {
                        marker,
                        focus: true,
                        no_window: false,
                    },
                    None,
                )
            }
            Kind::LoneRead(element) => {
                let marker = self.fresh();
                Item::Fact(focus_read(marker, element), None)
            }
            Kind::Noted => unreachable!("a noted caret is kept apart"),
            Kind::ReadsEnded => {
                if !self.closed {
                    self.open_markers.clear();
                }
                Item::Fact(DeliveredFact::UiaFocusReadsEnded, None)
            }
        };
        self.pushed.insert(
            trace,
            Pushed {
                kind,
                place,
                query,
                handed: None,
                group: Vec::new(),
                ahead: false,
                cancelled: false,
                closed: self.closed,
            },
        );
        if !self.closed {
            self.queue(trace, kind, resolved);
        }
        let _ = self.intake.push(
            Entry {
                item,
                trace: TraceId::from_raw(trace),
                observed_at_us: trace,
                timing: EventTiming::default(),
                front: Front::Untested,
            },
            &self.marks,
        );
    }

    /// The oldest open marker, resolved now.
    fn resolve_oldest(&mut self) -> Option<u64> {
        if self.closed || self.open_markers.is_empty() {
            return None;
        }
        Some(self.open_markers.remove(0))
    }

    /// The model's part of a push, of entry `trace` of `kind`, resolving the
    /// open marker `resolved`, if any: a drop or the end of the listener's
    /// reads removes markers; a read takes its marker's place; a keyed entry
    /// replaces the waiting entry of its key that no idle barrier divides
    /// from its place, nearest before it first; and a foreground change of
    /// window 9 removes the waiting focus on its client area from its
    /// thread, raised before it, as the intake's documentation says.
    fn queue(&mut self, trace: u64, kind: Kind, resolved: Option<u64>) {
        match kind {
            Kind::Dropped => {
                self.waiting.retain(|&waiting| Some(waiting) != resolved);
                return;
            }
            Kind::ReadsEnded => {
                let pushed = &self.pushed;
                self.waiting
                    .retain(|waiting| pushed[waiting].kind != Kind::Marker);
                return;
            }
            _ => {}
        }
        let mut at = match (kind, resolved) {
            (Kind::Read(_), Some(marker)) => {
                self.waiting.iter().position(|&waiting| waiting == marker)
            }
            _ => None,
        };
        if let Some(wanted) = key(kind) {
            let from = at.unwrap_or(self.waiting.len());
            if let Some(found) =
                self.find_waiting(from, |pushed| key(pushed.kind) == Some(wanted.clone()))
            {
                let old = self.waiting.remove(found);
                self.fate.insert(old, "replaced");
                if let Some(at) = at.as_mut()
                    && found < *at
                {
                    *at -= 1;
                }
            }
            if let Kind::Foreground { window: 0, thread } = kind {
                let from = self.waiting.len();
                if let Some(found) = self.find_waiting(from, |pushed| {
                    pushed.kind == Kind::Focus { child: 0, thread }
                }) {
                    let old = self.waiting.remove(found);
                    self.fate
                        .insert(old, "a client focus removed by its foreground change");
                }
            }
        }
        match at {
            Some(at) => self.waiting[at] = trace,
            None => self.waiting.push(trace),
        }
    }

    /// The place of the first waiting entry `wanted` names among those no
    /// idle barrier divides from place `at`: nearest before it, then from it
    /// on.
    fn find_waiting(&self, at: usize, wanted: impl Fn(&Pushed) -> bool) -> Option<usize> {
        let pushed = |place: usize| &self.pushed[&self.waiting[place]];
        let before = (0..at)
            .rev()
            .take_while(|&place| pushed(place).kind != Kind::Round)
            .find(|&place| wanted(pushed(place)));
        before.or_else(|| {
            (at..self.waiting.len())
                .take_while(|&place| pushed(place).kind != Kind::Round)
                .find(|&place| wanted(pushed(place)))
        })
    }

    /// What is ready to be planned: everything waiting before the first
    /// open marker, ending at the first idle barrier, which it includes.
    fn ready(&self) -> Vec<u64> {
        let mut ready = Vec::new();
        for &trace in &self.waiting {
            match self.pushed[&trace].kind {
                Kind::Marker => break,
                Kind::Round => {
                    ready.push(trace);
                    break;
                }
                _ => ready.push(trace),
            }
        }
        ready
    }

    /// Plans the batch `ready`, by the limiter's counts: of the MSAA focus
    /// events and foreground changes the newest four, of the other MSAA
    /// events the newest ten of each thread that raised them, a foreground
    /// change counted among both and kept by either; then of the menu
    /// openings kept, the newest alone.
    fn plan(&mut self, ready: &[u64]) {
        let mut focus = 0;
        let mut per_thread: BTreeMap<u8, usize> = BTreeMap::new();
        let mut kept = vec![true; ready.len()];
        for (at, trace) in ready.iter().enumerate().rev() {
            let mut count = |thread: u8| {
                let count = per_thread.entry(thread).or_default();
                *count += 1;
                *count
            };
            kept[at] = match self.pushed[trace].kind {
                Kind::Focus { .. } => {
                    focus += 1;
                    focus <= 4
                }
                Kind::Foreground { thread, .. } => {
                    focus += 1;
                    let among = focus <= 4;
                    count(thread) <= 10 || among
                }
                Kind::NameChange { thread, .. } => count(thread) <= 10,
                Kind::Console(_) => count(5) <= 10,
                _ => true,
            };
        }
        let newest_menu = ready
            .iter()
            .zip(&kept)
            .rposition(|(trace, &kept)| kept && matches!(self.pushed[trace].kind, Kind::Menu(_)));
        for (at, (&trace, &kept)) in ready.iter().zip(&kept).enumerate() {
            if !kept {
                self.fate.insert(trace, "past the batch limits");
            } else if matches!(self.pushed[&trace].kind, Kind::Menu(_)) && Some(at) != newest_menu {
                self.fate.insert(trace, "an older menu opening");
            } else {
                self.batch.insert(trace);
            }
        }
    }

    /// Takes the next planned item, checking it against what arrived
    /// before it.
    #[expect(
        clippy::too_many_lines,
        reason = "one check after another of a hand-out"
    )]
    fn take(&mut self) {
        let Some((planned, batch)) = self.intake.next(&self.marks) else {
            assert!(
                self.closed || (self.batch.is_empty() && self.ready().is_empty()),
                "nothing handed out with entries ready: {:?}",
                self.trail
            );
            return;
        };
        assert!(!self.closed, "a hand-out after close: {:?}", self.trail);
        assert_eq!(self.intake.batch_number(), batch, "{:?}", self.trail);
        if batch != self.last_batch {
            assert_eq!(
                batch,
                self.last_batch + 1,
                "batches numbered one by one, a non-empty batch each: {:?}",
                self.trail
            );
            assert!(
                self.batch.is_empty(),
                "a batch planned before the last was handed out: {:?}",
                self.trail
            );
            let ready = self.ready();
            assert!(
                !ready.is_empty(),
                "a batch of nothing ready: {:?}",
                self.trail
            );
            self.waiting.drain(..ready.len());
            self.plan(&ready);
            self.last_batch = batch;
        }
        self.log.push(format!("{planned:?} {batch}"));
        let (entries, menu) = match &planned {
            Planned::Run(entry) => (vec![entry.clone()], false),
            Planned::Menu(entry) => (vec![entry.clone()], true),
            Planned::Focus(entries) => (entries.clone(), false),
        };
        if menu {
            self.reach("a menu opening at its batch's end");
        }
        if entries.len() > 1 {
            self.reach("a focus group of several");
        }
        let traces: Vec<u64> = entries.iter().map(|entry| raw(entry.trace)).collect();
        // The reducer puts a barrier reached with a noted caret kept
        // directly behind it.
        if let [entry] = entries.as_slice()
            && let Item::Barrier {
                asker,
                behind_noted: false,
            } = entry.item
            && let Some(noted) = self.intake.take_idle()
        {
            assert!(self.batch.remove(&traces[0]), "{:?}", self.trail);
            let kept = self.noted.take().expect("a noted caret kept");
            self.waiting.insert(0, kept);
            self.waiting.insert(1, traces[0]);
            self.pushed.get_mut(&kept).expect("pushed").ahead = true;
            let barrier = Entry {
                item: Item::Barrier {
                    asker,
                    behind_noted: true,
                },
                ..entry.clone()
            };
            self.pushed.get_mut(&traces[0]).expect("pushed").ahead = true;
            self.intake.push_ahead([noted, barrier]);
            self.reach("a barrier behind its noted caret");
            return;
        }
        self.handouts += 1;
        for &trace in &traces {
            let pushed = self
                .pushed
                .get_mut(&trace)
                .unwrap_or_else(|| panic!("handed out {trace}, never pushed"));
            assert!(
                pushed.handed.is_none(),
                "entry {trace} handed out twice: {:?}",
                self.trail
            );
            assert!(
                !pushed.cancelled && !pushed.closed,
                "entry {trace} handed out after its cancel or the close: {:?}",
                self.trail
            );
            pushed.handed = Some((self.handouts, batch));
            pushed.group.clone_from(&traces);
            if !self.batch.remove(&trace) {
                // Only the noted caret kept apart is handed out outside a
                // batch, and only when nothing else is ready.
                assert!(
                    self.noted == Some(trace) && self.batch.is_empty() && self.ready().is_empty(),
                    "entry {trace} handed out outside its batch: {:?}",
                    self.trail
                );
                self.noted = None;
            }
        }
        // Nothing that arrived after an open marker is handed out, but the
        // noted caret, kept apart and handed out when nothing else is ready.
        for &trace in &traces {
            if self.pushed[&trace].ahead {
                continue;
            }
            let place = self.pushed[&trace].place;
            if let Some(&marker) = self
                .open_markers
                .iter()
                .find(|&&marker| self.pushed.get(&marker).is_some_and(|m| m.place < place))
            {
                panic!(
                    "entry {trace} handed out past open marker {marker}: {:?}",
                    self.trail
                );
            }
        }
        if self
            .open_markers
            .first()
            .is_some_and(|marker| self.pushed[marker].place > 0)
        {
            self.reach("held back by a marker");
        }
    }

    /// Ends the case: the listener's reads end, resolving every marker, and
    /// everything planned is handed out; then each entry's fate is judged.
    fn drain(&mut self) {
        self.step(Action::Push(Kind::ReadsEnded));
        for _ in 0..400 {
            let before = self.handouts;
            let noted = self.noted;
            self.step(Action::Next);
            if self.handouts == before && self.noted == noted {
                break;
            }
        }
        assert!(
            self.intake.next(&self.marks).is_none(),
            "the drain ended with something ready: {:?}",
            self.trail
        );
        self.judge();
    }

    /// Each entry was handed out once, or dropped for a reason named; and
    /// no hand-out overtook an earlier arrival but by an exception named.
    fn judge(&mut self) {
        let pushed: Vec<(u64, Pushed)> = self
            .pushed
            .iter()
            .map(|(&trace, pushed)| (trace, pushed.clone()))
            .collect();
        for (trace, entry) in &pushed {
            if entry.handed.is_some() {
                assert!(
                    !self.fate.contains_key(trace),
                    "entry {trace} handed out though {}: {:?}",
                    self.fate[trace],
                    self.trail
                );
                continue;
            }
            let reason = self.fate.get(trace).copied().or(
                if matches!(entry.kind, Kind::Marker | Kind::Dropped | Kind::ReadsEnded) {
                    Some("a marker or a drop")
                } else if entry.cancelled {
                    Some("cancelled")
                } else if entry.closed {
                    Some("closed")
                } else {
                    None
                },
            );
            let Some(reason) = reason else {
                panic!(
                    "entry {trace} ({:?}) neither handed out nor dropped for a reason: {:?}",
                    entry.kind, self.trail
                );
            };
            self.reach(reason);
        }
        for (trace, later) in &pushed {
            let Some((handed, batch)) = later.handed else {
                continue;
            };
            for (earlier_trace, earlier) in &pushed {
                let Some((earlier_handed, earlier_batch)) = earlier.handed else {
                    continue;
                };
                if earlier.place >= later.place || earlier_handed <= handed {
                    continue;
                }
                // `earlier` arrived first and was handed out after `later`.
                let run = earlier.group.len() > 1
                    && earlier
                        .group
                        .iter()
                        .any(|member| self.pushed[member].place > later.place);
                let menu = matches!(earlier.kind, Kind::Menu(_)) && earlier_batch == batch;
                let ahead = later.ahead || earlier.ahead;
                assert!(
                    run || menu || ahead,
                    "entry {trace} ({:?}) handed out ahead of {earlier_trace} ({:?}), which arrived first: {:?}",
                    later.kind,
                    earlier.kind,
                    self.trail
                );
            }
        }
    }
}

/// The object, kind and thread an entry of `kind` is replaced by a newer
/// one of, as the intake's documentation says: none for a query, a round,
/// a marker or a drop.
fn key(kind: Kind) -> Option<String> {
    Some(match kind {
        Kind::NameChange {
            window,
            child,
            thread,
        } => format!("name {window} {child} {thread}"),
        Kind::Focus { child, thread } => format!("focus {child} {thread}"),
        Kind::Foreground { window, thread } => format!("foreground {window} {thread}"),
        Kind::Console(which) => format!("console {which}"),
        Kind::Uia(element) => format!("uia {element}"),
        Kind::Menu(window) => format!("menu {window}"),
        Kind::NodesHeld => "held".to_owned(),
        Kind::Read(element) | Kind::LoneRead(element) => format!("read {element}"),
        Kind::Query
        | Kind::Round
        | Kind::Marker
        | Kind::Dropped
        | Kind::ReadsEnded
        | Kind::Noted => {
            return None;
        }
    })
}

/// The number a trace was minted from.
fn raw(trace: TraceId) -> u64 {
    format!("{trace}").parse().expect("a trace's number")
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

/// The listener's read of a UIA focus on element `element`, resolving
/// `marker`.
fn focus_read(marker: u64, element: u8) -> DeliveredFact {
    DeliveredFact::UiaFocusRead {
        marker,
        read: Box::new(UiaFocusRead {
            hwnd: 0,
            snapshot: snapshot(vec![2, i32::from(element)]),
            ancestors: Vec::new(),
            ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
            selected_child: None,
            value_from_selection: false,
            window: Some(9),
            same_as_last: None,
            bound_us: 2_000_000,
        }),
        focus: true,
    }
}

/// Runs the generated cases `config` draws, each twice from a new intake,
/// shrinking a failure to its smallest case; what the cases reached.
///
/// # Panics
///
/// On a failing case, naming the smallest.
pub(super) fn run_generated(config: Config) -> BTreeMap<&'static str, u64> {
    let reached = RefCell::new(BTreeMap::new());
    let mut runner = TestRunner::new(config);
    let strategy = proptest::collection::vec(action(), 1..80);
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
    if let Err(error) = result {
        panic!("{error}");
    }
    reached.into_inner()
}

/// The gate's seed and case count.
const SEED: u64 = 20_261_103;
const CASES: u32 = 800;

/// Generated cases of pushes of every kind of entry the intake orders,
/// hand-outs, cancels, a noted caret and the close hand each entry out
/// once or drop it for a reason named, let nothing overtake an earlier
/// arrival but by a named exception, hold everything behind an open
/// marker, give the same hand-outs when run twice, and reach every listed
/// state.
#[test]
fn generated_cases_keep_the_intakes_order() {
    let reached = run_generated(super::explore::config(SEED, CASES));
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
