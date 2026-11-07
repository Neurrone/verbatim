//! The intake's generated test (`books/dev/src/reference/crates/verbatim-outpost-logic.md`): lists of pushes, hand-outs, cancels, a noted caret kept for
//! when nothing else is ready, and the close, given to the intake alone,
//! with the reducer's one use of `push_ahead` (a barrier reached with a
//! noted caret kept goes behind it) done as the reducer does it. It checks
//! these rules, from what the test knows of each entry it pushed and which
//! entries each call left waiting, never from the intake's own keys, and
//! never by planning a batch itself:
//!
//! - each entry pushed is handed out at most once, and by the end exactly
//!   once or dropped for a reason the test names: replaced by a newer
//!   entry of its object, kind and thread with no idle barrier between,
//!   past the batch limits, a focus on a window's client area removed by
//!   the window's foreground change raised after it, an older menu opening,
//!   cancelled, closed, or a marker or a drop, which is never handed out;
//! - a push leaves no two waiting entries of one key, nor a window's
//!   foreground change and a focus on its client area from its thread,
//!   without an idle query between them;
//! - a batch is what waited before the first marker, through the first idle
//!   query; the entries it keeps are among those, within the limiter's
//!   counts (the newest four focus events and foreground changes, the
//!   newest ten other events of each thread, a foreground change kept by
//!   either), an entry dropped only when each of its counts is reached by
//!   newer entries kept, and of its menu openings the newest alone;
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
use proptest::test_runner::{Config, TestError, TestRunner};
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
    /// The batch being handed out, by trace, as the intake planned it; the
    /// number of the last batch; and the entries the intake dropped, with
    /// the reason each drop was checked against.
    batch: BTreeSet<u64>,
    last_batch: u64,
    fate: BTreeMap<u64, &'static str>,
    /// Every hand-out, by its `Debug` text, for the double run.
    log: Vec<String>,
    reached: BTreeMap<&'static str, u64>,
    trail: Vec<Action>,
}

/// The lifecycle states the gate's cases must each reach.
const LISTED: [&str; 13] = [
    "a focus group of several",
    "replaced",
    "past the batch limits",
    "a focus event dropped behind four kept",
    "a same-key pair across an idle query",
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
        let before = self.waiting();
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
        if !self.closed {
            self.check_push(trace, kind, resolved, &before);
        }
    }

    /// The entries waiting in the intake, by trace, in their order.
    fn waiting(&self) -> Vec<u64> {
        self.intake.traces().0.into_iter().map(raw).collect()
    }

    /// The entries the intake has planned and not yet handed out, by trace.
    fn planned(&self) -> Vec<u64> {
        self.intake.traces().1.into_iter().map(raw).collect()
    }

    /// Whether an idle query stands in `list` strictly between places `a`
    /// and `b`.
    fn divided(&self, list: &[u64], a: usize, b: usize) -> bool {
        let (low, high) = (a.min(b), a.max(b));
        high > low
            && list[low + 1..high]
                .iter()
                .any(|trace| self.pushed[trace].kind == Kind::Round)
    }

    /// Checks what the push of entry `trace` of `kind`, resolving the open
    /// marker `resolved`, if any, did to what waited (`before`), as the
    /// intake's documentation says: it removed only the markers it
    /// resolves, at most one waiting entry of its key with no idle query
    /// between it and the new entry's place (its marker's, for a read), and
    /// for a foreground change of window 9 at most one focus on its client
    /// area from its thread with none between; and after it no waiting entry
    /// of its key, nor such a focus, stands beside it with no idle query
    /// between. Each entry removed is given its reason.
    fn check_push(&mut self, trace: u64, kind: Kind, resolved: Option<u64>, before: &[u64]) {
        let after = self.waiting();
        let trail = format!("{:?}", self.trail);
        let at = resolved
            .and_then(|marker| before.iter().position(|&waiting| waiting == marker))
            .unwrap_or(before.len());
        let client = |was: Kind| matches!((kind, was), (Kind::Foreground { window: 0, thread }, Kind::Focus { child: 0, thread: of }) if thread == of);
        let (mut replaced, mut removed) = (0, 0);
        for (place, &gone) in before.iter().enumerate() {
            if after.contains(&gone) {
                continue;
            }
            let was = self.pushed[&gone].kind;
            let undivided = !self.divided(before, place, at);
            if was == Kind::Marker {
                assert!(
                    Some(gone) == resolved || kind == Kind::ReadsEnded,
                    "marker {gone} removed by {trace} ({kind:?}), which does not resolve it: {trail}"
                );
            } else if undivided && key(was).is_some() && key(was) == key(kind) {
                replaced += 1;
                self.fate.insert(gone, "replaced");
            } else if undivided && client(was) {
                removed += 1;
                self.fate
                    .insert(gone, "a client focus removed by its foreground change");
            } else {
                panic!(
                    "entry {gone} ({was:?}) removed by the push of {trace} ({kind:?}) for no documented reason: {trail}"
                );
            }
        }
        assert!(
            replaced <= 1 && removed <= 1,
            "the push of {trace} removed {replaced} of its key and {removed} client focuses: {trail}"
        );
        let Some(new) = after.iter().position(|&waiting| waiting == trace) else {
            return;
        };
        let mut across = false;
        for (place, &other) in after.iter().enumerate() {
            if other == trace {
                continue;
            }
            if self.divided(&after, place, new) {
                across |= key(kind).is_some() && key(self.pushed[&other].kind) == key(kind);
                continue;
            }
            let was = self.pushed[&other].kind;
            assert!(
                key(kind).is_none() || key(was) != key(kind),
                "entries {other} and {trace} of one key both wait with no idle query between: {trail}"
            );
            assert!(
                !client(was),
                "the focus {other} on the client area waits beside its window's foreground change {trace}: {trail}"
            );
        }
        // An entry of its key waits only beyond an idle query, so a search
        // for one to replace that did not stop at idle queries would find it.
        if across && replaced == 0 {
            self.reach("a same-key pair across an idle query");
        }
    }

    /// Checks the batch the intake planned from `waiting`, what waited
    /// before, keeping `kept`: it is everything before the first marker,
    /// through the first idle query; it keeps only what it took; each entry
    /// the limiter counts is kept exactly when, in one of its groups, fewer
    /// newer entries than the group's limit are kept; and of its menu
    /// openings it keeps the newest alone. Each entry dropped is given its
    /// reason.
    fn check_batch(&mut self, waiting: &[u64], kept: &BTreeSet<u64>) {
        let after = self.waiting();
        let trail = format!("{:?}", self.trail);
        assert!(
            after.len() < waiting.len() && waiting.ends_with(&after),
            "a batch is planned from the front of what waits, and takes something: {trail}"
        );
        let ready = &waiting[..waiting.len() - after.len()];
        let kind = |trace: &u64| self.pushed[trace].kind;
        let round = ready.iter().position(|trace| kind(trace) == Kind::Round);
        assert!(
            !ready.iter().any(|trace| kind(trace) == Kind::Marker)
                && round.is_none_or(|at| at + 1 == ready.len())
                && (round.is_some()
                    || after
                        .first()
                        .is_none_or(|first| kind(first) == Kind::Marker)),
            "a batch is what waits before the first marker, through the first idle query: {trail}"
        );
        assert!(
            kept.iter().all(|trace| ready.contains(trace)),
            "a batch keeps an entry it did not take: {trail}"
        );
        let mut fates = Vec::new();
        let mut reach_focus = false;
        for (at, trace) in ready.iter().enumerate() {
            let counted = groups(kind(trace));
            let newer = &ready[at + 1..];
            let within = counted.iter().any(|&(group, limit)| {
                newer
                    .iter()
                    .filter(|newer| {
                        kept.contains(newer)
                            && groups(kind(newer)).iter().any(|&(of, _)| of == group)
                    })
                    .count()
                    < limit
            });
            if kept.contains(trace) {
                assert!(
                    counted.is_empty() || within,
                    "entry {trace} ({:?}) kept with each of its limits reached by newer entries: {trail}",
                    kind(trace)
                );
            } else if matches!(kind(trace), Kind::Menu(_)) {
                assert!(
                    newer
                        .iter()
                        .any(|newer| kept.contains(newer) && matches!(kind(newer), Kind::Menu(_))),
                    "the menu opening {trace} dropped with no newer one kept: {trail}"
                );
                fates.push((*trace, "an older menu opening"));
            } else {
                assert!(
                    !counted.is_empty() && !within,
                    "entry {trace} ({:?}) dropped within its limits: {trail}",
                    kind(trace)
                );
                fates.push((*trace, "past the batch limits"));
                // Kept by a batch keeping five focus events, it would be
                // the fifth.
                let focus_kept = newer
                    .iter()
                    .filter(|newer| kept.contains(newer) && groups(kind(newer)).contains(&(0, 4)))
                    .count();
                if matches!(kind(trace), Kind::Focus { .. }) && focus_kept == 4 {
                    reach_focus = true;
                }
            }
        }
        assert!(
            kept.iter()
                .filter(|trace| matches!(kind(trace), Kind::Menu(_)))
                .count()
                <= 1,
            "two menu openings kept in one batch: {trail}"
        );
        self.fate.extend(fates);
        if reach_focus {
            self.reach("a focus event dropped behind four kept");
        }
    }

    /// The oldest open marker, resolved now.
    fn resolve_oldest(&mut self) -> Option<u64> {
        if self.closed || self.open_markers.is_empty() {
            return None;
        }
        Some(self.open_markers.remove(0))
    }

    /// Takes the next planned item, checking it against what arrived
    /// before it.
    #[expect(
        clippy::too_many_lines,
        reason = "one check after another of a hand-out"
    )]
    fn take(&mut self) {
        let waiting = self.waiting();
        // Nothing is ready when nothing is planned and what waits, if
        // anything, waits behind a marker.
        let idle = self.planned().is_empty()
            && waiting
                .first()
                .is_none_or(|first| self.pushed[first].kind == Kind::Marker);
        let Some((planned, batch)) = self.intake.next(&self.marks) else {
            assert!(
                self.closed || idle,
                "nothing handed out with entries ready: {:?}",
                self.trail
            );
            return;
        };
        assert!(!self.closed, "a hand-out after close: {:?}", self.trail);
        assert_eq!(self.intake.batch_number(), batch, "{:?}", self.trail);
        self.log.push(format!("{planned:?} {batch}"));
        let (entries, menu) = match &planned {
            Planned::Run(entry) => (vec![entry.clone()], false),
            Planned::Menu(entry) => (vec![entry.clone()], true),
            Planned::Focus(entries) => (entries.clone(), false),
        };
        let traces: Vec<u64> = entries.iter().map(|entry| raw(entry.trace)).collect();
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
            let mut kept: BTreeSet<u64> = self.planned().into_iter().collect();
            kept.extend(&traces);
            self.check_batch(&waiting, &kept);
            self.batch = kept;
            self.last_batch = batch;
        }
        if menu {
            self.reach("a menu opening at its batch's end");
        }
        if entries.len() > 1 {
            self.reach("a focus group of several");
        }
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
                    self.noted == Some(trace) && self.batch.is_empty() && idle,
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

/// The limiter's groups an entry of `kind` is counted in, each with how
/// many of its newest a batch keeps, as the intake's documentation says:
/// the MSAA focus events and foreground changes (group 0), four; each
/// thread's other MSAA events, by the thread, the console host's counted
/// by its own thread 5, ten; a foreground change counted in both, and kept
/// by either. Nothing else is counted.
fn groups(kind: Kind) -> Vec<(u8, usize)> {
    match kind {
        Kind::Focus { .. } => vec![(0, 4)],
        Kind::Foreground { thread, .. } => vec![(0, 4), (thread, 10)],
        Kind::NameChange { thread, .. } => vec![(thread, 10)],
        Kind::Console(_) => vec![(5, 10)],
        _ => Vec::new(),
    }
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
pub(super) fn run_generated(config: Config, again: &str) -> BTreeMap<&'static str, u64> {
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
    match result {
        Ok(()) => reached.into_inner(),
        Err(TestError::Fail(reason, case)) => panic!("{reason}\n{}", found_report(&case, again)),
        Err(error) => panic!("{error}"),
    }
}

impl Action {
    /// The action as the Rust expression that makes it, as a found case
    /// lists it.
    fn code(self) -> String {
        match self {
            Action::Push(kind) => format!("Action::Push(Kind::{kind:?})"),
            other => format!("Action::{other:?}"),
        }
    }
}

/// The report of a failing case: `case` as a line ready to paste into
/// [`found`], and the commands that run the found cases and the run that
/// found it, `again`.
fn found_report(case: &[Action], again: &str) -> String {
    let actions: Vec<String> = case.iter().map(|action| action.code()).collect();
    format!(
        "the smallest failing case, as a line for `found()` in crates/verbatim-outpost-logic/src/tests/intake_order.rs:\n        vec![{}],\n\
         the found cases alone: cargo test -p verbatim-outpost-logic --lib -- --exact tests::intake_order::found_cases_keep_the_intakes_order\n\
         the run that found it again: {again}",
        actions.join(", ")
    )
}

/// The cases failures of the generated test or the exploration shrank to,
/// each kept as its list of actions, as the failure's report gives it.
fn found() -> Vec<Vec<Action>> {
    Vec::new()
}

/// Every found case keeps the intake's order, and gives the same
/// hand-outs when run twice.
#[test]
fn found_cases_keep_the_intakes_order() {
    for case in found() {
        assert_eq!(Run::case(&case).log, Run::case(&case).log, "{case:?}");
    }
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
    let reached = run_generated(
        super::explore::config(SEED, CASES),
        "cargo test -p verbatim-outpost-logic --lib -- --exact tests::intake_order::generated_cases_keep_the_intakes_order",
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
