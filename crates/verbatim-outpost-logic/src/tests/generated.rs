//! The generated test of a caret key's answer and the outpost's last read:
//! lists of actions, each made valid when it is applied, given to the
//! reducer through its entries, its programs' commits and its release, and
//! compared after every step with a much simpler reference model.
//!
//! The model is written from `docs/parity/text.md`,
//! `docs/nvda/editable-text-and-terminals.md` and Dickson's dated decisions
//! on the caret and selection rework, never from the code's fields. It holds
//! each node's text state at every moment, the newest read committed of it,
//! and each key's state at its arrival, and checks, after every step:
//!
//! - a key answers that it had an effect exactly when the state its read
//!   found differs from the state at the key's arrival, by the decisions'
//!   evidence: the caret having left where it was, the characters beside it
//!   changing, the unit at the caret changing after Delete, the caret's line
//!   changing after a key that deletes, or the selection changing;
//! - an answer with no read is taken only where nothing could be read of
//!   the key: a newer key, a focus elsewhere, a replaced element, a release
//!   or a program ended without its check (`docs/parity/text.md`, "A caret
//!   key that moves nothing"); after Core's final read it is compared with
//!   the state then, like a read's answer;
//! - a key Core has not called off is answered once, and a key it called off
//!   gets no answer after the call-off is handled;
//! - the before read's answer comes before any other of its key;
//! - one last read per node, the newest committed, gone at the node's
//!   release or the element's replacement.
//!
//! The world is an application changing a node's caret, selection and text
//! with independently timed caret events, early, late, twice or never, with
//! Core's orders (a key's request, its final read, its end), programs run,
//! finished, panicking or abandoned, a release, a replaced element and a
//! focus elsewhere arriving at any step.
//!
//! Terminals' reading rules and the console host's poll are out of scope: the
//! world's keys and caret events name only plain edit nodes, never a node of
//! a terminal, and the seam opens no console poll.
//!
//! Its limits. The programs live in
//! `crates/verbatim-outpost/src/outpost/worker.rs`, and the judgement of a
//! key's evidence and the search for its baseline in
//! `crates/verbatim-outpost/src/text/mod.rs` (`baseline`, `evidence`), which
//! this crate cannot reach. The module [`seam`] therefore carries a
//! transcription of today's judgement and is the one place that names a
//! production type, so that the rework changes that module alone: a failure
//! here shows the rule in that transcription, read against the reducer's real
//! facts, not a run of the outpost's own code. Today's code has no input for
//! Core's final read or a key's end: the seam gives the final read as the
//! watch's bound and reports that the end cannot be delivered, so the
//! call-off and before-read comparisons have nothing to compare until the
//! builder's seam sends them.

use std::collections::{BTreeMap, BTreeSet};

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestError, TestRunner};

use super::*;

/// What the application's text is about a node's caret at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TextState {
    caret: u32,
    /// The selection, 0 for none, otherwise a number for what is selected.
    selection: u32,
    /// The characters either side of the caret.
    beside: (u32, u32),
    /// The unit at the caret.
    unit: u32,
    /// The caret's line.
    line: u32,
}

/// What a key is, by what the decisions judge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyKind {
    /// A key that moves the caret or selects.
    Other,
    /// Delete, whose unit at the caret is compared and which deletes.
    Delete,
    /// Backspace, which deletes but compares no unit.
    Backspace,
}

impl KeyKind {
    fn of(kind: u8) -> Self {
        match kind % 3 {
            0 => Self::Other,
            1 => Self::Delete,
            _ => Self::Backspace,
        }
    }

    fn deletes(self) -> bool {
        self != Self::Other
    }
}

/// Whether `now` shows an effect of a key of `kind` that arrived at `arrival`
/// (the decisions' definition of evidence).
fn had_effect(kind: KeyKind, arrival: TextState, now: TextState) -> bool {
    now.caret != arrival.caret
        || now.beside != arrival.beside
        || now.selection != arrival.selection
        || (kind == KeyKind::Delete && now.unit != arrival.unit)
        || (kind.deletes() && now.line != arrival.line)
}

/// What a step of the seam reports back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// A read of a node committed, with its number.
    ReadCommitted { node: u64, read: u64 },
    /// A key's before read answered: today's reducer has no such answer.
    #[expect(dead_code, reason = "the rework's seam reports it")]
    BeforeAnswered { key: u64 },
    /// A key answered: the read its answer rests on, if it had one, and
    /// whether it says the key had an effect.
    Answered {
        key: u64,
        read: Option<u64>,
        effect: bool,
    },
}

/// The adapter between this test's vocabulary and the reducer: the only code
/// here that names how a key's request, a program's commit or a reply is
/// built, or what the reducer's facts hold, so that the rework changes this
/// module alone. Its baseline judgement is today's, transcribed, and is
/// replaced with the rework's.
mod seam {
    use std::sync::Arc;

    use verbatim_model::protocol::{QueryOutcome, QueryResult};
    use verbatim_model::{CaretWatch, TextAnchor, TextPosition, TextReply, TextUnit};

    use super::*;
    use crate::{Anchor, CaretSeen, KeptSelection, NodeAnchors};

    /// The window of an edit node's caret events, by node number.
    fn window_of(node: u64) -> isize {
        isize::try_from(8 + node).expect("a small node number")
    }

    /// The node a caret event of `window` is of, the inverse of
    /// [`window_of`].
    fn node_of(window: isize) -> u64 {
        u64::try_from(window).expect("a window this test made") - 8
    }

    /// What the program in flight does, as the world runs it.
    #[derive(Clone, Debug)]
    enum Kind {
        /// A caret key's first check.
        Key {
            key: u64,
            node: u64,
            kind: KeyKind,
            watch: CaretWatch,
        },
        /// A caret event's check and report.
        Event { node: u64 },
        /// Any other program.
        Other,
    }

    #[derive(Clone, Debug)]
    struct Flight {
        request: Request,
        kind: Kind,
        done: bool,
    }

    /// The reducer under test with the programs the world runs for it.
    #[derive(Clone)]
    pub(super) struct Seam {
        reducer: Reducer,
        answers: Answers,
        at: u64,
        flight: Option<Flight>,
        /// Each key's kind, for the judgement of its later checks.
        kinds: BTreeMap<u64, KeyKind>,
        /// What each read found, by its number.
        reads: BTreeMap<u64, TextState>,
        /// Each node's newest read Core has been told of.
        core_knows: BTreeMap<u64, u64>,
        /// The read the commit being made rests on.
        pending_read: Option<u64>,
    }

    impl Seam {
        pub(super) fn new() -> Self {
            Self {
                reducer: Reducer::new(),
                answers: Answers::default(),
                at: 1,
                flight: None,
                kinds: BTreeMap::new(),
                reads: BTreeMap::new(),
                core_knows: BTreeMap::new(),
                pending_read: None,
            }
        }

        /// Whether a program is in flight.
        pub(super) fn busy(&self) -> bool {
            self.flight.is_some()
        }

        /// Whether the program in flight has not yet made its read.
        pub(super) fn fresh(&self) -> bool {
            self.flight.as_ref().is_some_and(|flight| !flight.done)
        }

        /// Whether the program in flight is an event's and has not yet read,
        /// the only program that can report a focus.
        pub(super) fn fresh_event(&self) -> bool {
            self.flight
                .as_ref()
                .is_some_and(|flight| !flight.done && matches!(flight.kind, Kind::Event { .. }))
        }

        /// Whether the program in flight can run out of its deadline.
        pub(super) fn can_abandon(&self) -> bool {
            self.flight.is_some() && self.reducer.flight.deadline().is_some()
        }

        /// The number of the newest read the reducer keeps of `node`.
        pub(super) fn last_read(&self, node: u64) -> Option<u64> {
            self.reducer
                .facts
                .text
                .edit(node)
                .and_then(|facts| facts.carets.back())
                .map(|seen| seen.caret.anchor.0)
        }

        fn tick_clock(&mut self) {
            self.at += 1;
        }

        fn drive(&mut self, input: Input) -> Vec<Seen> {
            let now = now(self.at);
            let effects = self.reducer.input(input, now);
            let effects = with_answers(&mut self.reducer, &mut self.answers, effects, now);
            self.effects(effects)
        }

        fn push(&mut self, item: Item) -> Vec<Seen> {
            let at = self.at;
            self.drive(Input::Entry {
                entry: entry(item, at),
                source: Source::Other,
            })
        }

        /// Reads the effects: programs dispatched, and keys answered.
        fn effects(&mut self, effects: Vec<Effect>) -> Vec<Seen> {
            let mut seen = Vec::new();
            for effect in effects {
                match effect {
                    Effect::Dispatch(program) => {
                        self.flight = Some(Flight {
                            request: program.request,
                            kind: self.kind_of(&program),
                            done: false,
                        });
                    }
                    Effect::Abandon { .. } => self.flight = None,
                    Effect::Send(message) => {
                        if let OutpostToCore::Reply {
                            request_id,
                            outcome,
                            ..
                        } = *message
                        {
                            let evidence = matches!(
                                outcome,
                                QueryOutcome::Done(QueryResult::Text(TextReply::Done))
                            );
                            seen.push(Seen::Answered {
                                key: request_id.number,
                                read: if evidence {
                                    self.pending_read.take()
                                } else {
                                    None
                                },
                                effect: evidence,
                            });
                        }
                    }
                    _ => {}
                }
            }
            seen
        }

        fn kind_of(&self, program: &Program) -> Kind {
            match &program.work {
                Work::Entry { entry, .. } => match &entry.item {
                    Item::Query {
                        request_id,
                        query:
                            Query::Text {
                                node_id,
                                op: TextOp::AwaitCaret(watch),
                            },
                    } => Kind::Key {
                        key: request_id.number,
                        node: node_id.number(),
                        kind: self.kinds[&request_id.number],
                        watch: watch.clone(),
                    },
                    Item::Msaa { hwnd, .. } => Kind::Event {
                        node: node_of(*hwnd),
                    },
                    _ => Kind::Other,
                },
                _ => Kind::Other,
            }
        }

        /// A key's request reaches the outpost.
        pub(super) fn press(&mut self, key: u64, node: u64, kind: KeyKind) -> Vec<Seen> {
            self.tick_clock();
            self.kinds.insert(key, kind);
            let since = self.core_knows.get(&node).map(|&read| TextPosition {
                anchor: TextAnchor(read),
                offset: self.reads[&read].caret,
            });
            let watch = CaretWatch {
                since,
                pressed_at_ms: self.at,
                unit: TextUnit::Character,
                compare: (kind == KeyKind::Delete).then(String::new),
                landing: false,
                deletes: kind.deletes(),
            };
            self.push(Item::Query {
                request_id: RequestId::shell(key),
                query: Query::Text {
                    node_id: NodeId::new(node),
                    op: TextOp::AwaitCaret(watch),
                },
            })
        }

        /// Core's final read of a key arrives: today, the watch's bound.
        pub(super) fn final_read(&mut self) -> Vec<Seen> {
            self.tick_clock();
            let Some(due) = self.reducer.facts.follow.watch.due() else {
                return Vec::new();
            };
            self.at = self.at.max(due / 1_000);
            let now = now(self.at);
            let effects = self.reducer.tick(now);
            let effects = with_answers(&mut self.reducer, &mut self.answers, effects, now);
            self.effects(effects)
        }

        /// Core calls a key off. Today nothing carries it, so it is not
        /// handled; the builder's seam sends the order and returns `true`.
        pub(super) fn call_off(&mut self, _key: u64) -> bool {
            self.tick_clock();
            false
        }

        /// A caret event of `node` arrives.
        pub(super) fn event(&mut self, node: u64) -> Vec<Seen> {
            self.tick_clock();
            self.push(Item::Msaa {
                kind: WinKind::Caret,
                hwnd: window_of(node),
                id_object: -8,
                id_child: 0,
                raiser: 3,
            })
        }

        /// The change that records `read` as `node`'s newest read.
        fn report(&self, node: u64, read: u64, state: TextState) -> TextChange {
            let mut part = self
                .reducer
                .facts
                .text
                .edit(node)
                .cloned()
                .unwrap_or_else(NodeAnchors::default);
            part.reports += 1;
            let report = part.reports;
            if part.carets.len() == 8 {
                part.carets.pop_front();
            }
            part.carets.push_back(CaretSeen {
                read_at_ms: read,
                caret: TextPosition {
                    anchor: TextAnchor(read),
                    offset: state.caret,
                },
                selection: None,
                report,
            });
            part.anchors.insert(
                read,
                Anchor {
                    pos: state.caret,
                    text: Arc::from("line"),
                },
            );
            part.selection = Some(KeptSelection {
                ends: (state.caret, state.caret),
                collapsed: state.selection == 0,
                text: None,
                report,
            });
            TextChange::Edit {
                node,
                part,
                copied: BTreeSet::new(),
            }
        }

        /// Today's judgement, transcribed from `baseline` and `evidence` in
        /// `crates/verbatim-outpost/src/text/mod.rs`: the newest caret the
        /// outpost reported from a read before the key, else where Core
        /// knew it; with neither, a caret event alone is evidence.
        fn judge(
            &self,
            node: u64,
            watch: &CaretWatch,
            kind: KeyKind,
            now: TextState,
            caret_event: bool,
        ) -> bool {
            let facts = self.reducer.facts.text.edit(node);
            let reported = facts.and_then(|facts| {
                facts
                    .carets
                    .iter()
                    .rev()
                    .find(|seen| seen.read_at_ms < watch.pressed_at_ms)
            });
            let baseline = reported
                .map(|seen| seen.caret.anchor.0)
                .or(watch.since.map(|position| position.anchor.0));
            let Some(baseline) = baseline else {
                return caret_event;
            };
            let then = self.reads[&baseline];
            then.caret != now.caret
                || then.selection != now.selection
                || then.beside != now.beside
                || (kind == KeyKind::Delete && then.unit != now.unit)
                || (watch.deletes && reported.is_some() && then.line != now.line)
        }

        fn take_read(&mut self, node: u64, state: TextState) -> u64 {
            let read = self.at;
            self.reads.insert(read, state);
            self.core_knows.insert(node, read);
            read
        }

        fn commit(&mut self, request: Request, change: Change) -> Vec<Seen> {
            self.drive(Input::Commit {
                request,
                change: Box::new(change),
            })
        }

        /// The program in flight makes its read of `app` and commits.
        pub(super) fn run(&mut self, app: &impl Fn(u64) -> TextState) -> Vec<Seen> {
            self.tick_clock();
            let Some(flight) = self.flight.as_mut().filter(|flight| !flight.done) else {
                return Vec::new();
            };
            flight.done = true;
            let request = flight.request;
            match flight.kind.clone() {
                Kind::Key {
                    key,
                    node,
                    kind,
                    watch,
                } => {
                    let state = app(node);
                    let read = self.take_read(node, state);
                    let report = self.report(node, read, state);
                    let mut seen = vec![Seen::ReadCommitted { node, read }];
                    let evidence = self.judge(node, &watch, kind, state, false);
                    let change = if evidence {
                        self.pending_read = Some(read);
                        Change {
                            result: Some(reply(
                                RequestId::shell(key),
                                TraceId::from_raw(4),
                                QueryOutcome::Done(QueryResult::Text(TextReply::Done)),
                            )),
                            text: vec![report],
                            ..Change::default()
                        }
                    } else {
                        let open = OpenWatch {
                            request_id: RequestId::shell(key),
                            trace: TraceId::from_raw(4),
                            node_id: NodeId::new(node),
                            watch,
                            timing: EventTiming::default(),
                            calls: CallCounts::default(),
                        };
                        Change {
                            watch: Some(WatchChange::Open {
                                open,
                                console: false,
                            }),
                            text: vec![report],
                            ..Change::default()
                        }
                    };
                    seen.extend(self.commit(request, change));
                    seen
                }
                Kind::Event { node } => {
                    let mut seen = self.commit(
                        request,
                        Change {
                            watch: Some(WatchChange::Take(NodeId::new(node))),
                            ..Change::default()
                        },
                    );
                    let held = match &self.reducer.facts.follow.watch {
                        Watch::Checking { open, .. } if open.node_id.number() == node => {
                            Some((open.watch.clone(), open.request_id.number))
                        }
                        _ => None,
                    };
                    let state = app(node);
                    let read = self.take_read(node, state);
                    let report = self.report(node, read, state);
                    seen.push(Seen::ReadCommitted { node, read });
                    let watch = held.map(|(watch, key)| {
                        let kind = self.kinds[&key];
                        let evidence = self.judge(node, &watch, kind, state, true);
                        if evidence {
                            self.pending_read = Some(read);
                            WatchChange::Checked(Checked::Answered {
                                reply: TextReply::Done,
                                calls: CallCounts::default(),
                                awaited_at_us: 0,
                            })
                        } else {
                            WatchChange::Checked(Checked::Watching {
                                calls: CallCounts::default(),
                            })
                        }
                    });
                    seen.extend(self.commit(
                        request,
                        Change {
                            watch,
                            text: vec![report],
                            ..Change::default()
                        },
                    ));
                    seen
                }
                Kind::Other => Vec::new(),
            }
        }

        fn finished(&mut self, panicked: bool) -> Vec<Seen> {
            self.tick_clock();
            let flight = self.flight.take().expect("a program in flight");
            self.drive(Input::Finished {
                request: flight.request,
                reported: false,
                panicked,
                cut: None,
            })
        }

        /// The program in flight ends.
        pub(super) fn finish(&mut self) -> Vec<Seen> {
            self.finished(false)
        }

        /// The program in flight panics.
        pub(super) fn panic(&mut self) -> Vec<Seen> {
            self.finished(true)
        }

        /// The program in flight's deadline passes and it is abandoned.
        pub(super) fn abandon(&mut self) -> Vec<Seen> {
            let deadline = self.reducer.flight.deadline().expect("in flight");
            self.at = self.at.max(deadline / 1_000);
            self.flight = None;
            let now = now(self.at);
            let effects = self.reducer.tick(now);
            let effects = with_answers(&mut self.reducer, &mut self.answers, effects, now);
            self.effects(effects)
        }

        /// The program in flight learns `node`'s element was replaced.
        pub(super) fn replace(&mut self, node: u64) -> Vec<Seen> {
            self.tick_clock();
            let request = self.flight.as_ref().expect("a program in flight").request;
            self.commit(
                request,
                Change {
                    replaced: Some(NodeId::new(node)),
                    text: vec![TextChange::Forget(node)],
                    ..Change::default()
                },
            )
        }

        /// The program in flight reports a focus on another node.
        pub(super) fn focus_elsewhere(&mut self) -> Vec<Seen> {
            self.tick_clock();
            let request = self.flight.as_ref().expect("a program in flight").request;
            self.commit(request, focus_change(3, false))
        }

        /// Core holds `node` no longer, and the release it asks for ends.
        pub(super) fn release(&mut self, node: u64) -> Vec<Seen> {
            self.tick_clock();
            let mut seen = self.push(Item::NodesHeld {
                nodes: vec![],
                anchors: vec![],
                acknowledged: 0,
            });
            seen.extend(self.drive(Input::Released {
                nodes: [node].into_iter().collect(),
                anchors: BTreeSet::new(),
                objects: false,
            }));
            seen
        }
    }
}

use seam::Seam;

/// One step of a case, each made valid when it is applied.
#[derive(Clone, Copy, Debug)]
enum Action {
    /// A key's request; its node, and whether it deletes, by the numbers.
    Press { kind: u8, node: u8 },
    /// Core's final read of the key numbered by the index.
    FinalRead(u8),
    /// Core calls the key numbered by the index off.
    End(u8),
    /// The application moves a node's caret, raising no event yet.
    Move { node: u8 },
    /// The application changes a node's selection.
    Select { node: u8 },
    /// The application changes the characters beside a node's caret.
    EditBeside { node: u8 },
    /// The application changes the unit at a node's caret.
    EditUnit { node: u8 },
    /// The application changes a node's caret's line.
    EditLine { node: u8 },
    /// A caret event of a node arrives, whatever the application did.
    Event { node: u8 },
    /// The program in flight reads and commits.
    Run,
    /// The program in flight ends.
    Finish,
    /// The program in flight panics.
    Panic,
    /// The program in flight's deadline passes.
    Abandon,
    /// A node's element is replaced by a new one, as a program learns.
    Replace { node: u8 },
    /// The program in flight reports a focus on another node.
    FocusElsewhere,
    /// A node is released.
    Release { node: u8 },
}

/// What the model holds of a key.
#[derive(Clone, Copy, Debug)]
struct KeyRecord {
    node: u64,
    kind: KeyKind,
    arrival: TextState,
    answers: u32,
    before_seen: bool,
    called_off: bool,
}

/// One case's run: the reducer and the model beside it.
struct World {
    seam: Seam,
    apps: BTreeMap<u64, TextState>,
    keys: BTreeMap<u64, KeyRecord>,
    newest: BTreeMap<u64, u64>,
    released: BTreeSet<u64>,
    /// What each committed read found, by its number, in order.
    reads: BTreeMap<u64, TextState>,
    /// A number for the next change the application makes.
    change: u32,
    log: Vec<String>,
}

fn node_number(node: u8) -> u64 {
    u64::from(node % 2) + 1
}

impl World {
    fn new() -> Self {
        Self {
            seam: Seam::new(),
            apps: BTreeMap::new(),
            keys: BTreeMap::new(),
            newest: BTreeMap::new(),
            released: BTreeSet::new(),
            reads: BTreeMap::new(),
            change: 0,
            log: Vec::new(),
        }
    }

    fn app(&self, node: u64) -> TextState {
        self.apps.get(&node).copied().unwrap_or_default()
    }

    fn change(&mut self, node: u64, apply: impl FnOnce(&mut TextState, u32)) {
        self.change += 1;
        let change = self.change;
        let state = self.apps.entry(node).or_default();
        apply(state, change);
    }

    fn key_number(&self, index: u8) -> Option<u64> {
        if self.keys.is_empty() {
            return None;
        }
        let at = usize::from(index) % self.keys.len();
        self.keys.keys().nth(at).copied()
    }

    /// Runs `actions` from a new world, each made valid when it is applied,
    /// compared with the model after every step; ends with every key's
    /// answer (`drain`).
    fn case(actions: &[Action]) -> Self {
        let mut world = Self::new();
        for action in actions {
            world.step(*action);
        }
        world.drain();
        world
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm for each action of the world, read in one place"
    )]
    fn step(&mut self, action: Action) {
        self.log.push(format!("{action:?}"));
        let (seen, compared_key) = match action {
            Action::Press { kind, node } => {
                let node = node_number(node);
                if self.released.contains(&node) {
                    return;
                }
                let key = u64::try_from(self.keys.len()).expect("few keys") + 1;
                let kind = KeyKind::of(kind);
                self.keys.insert(
                    key,
                    KeyRecord {
                        node,
                        kind,
                        arrival: self.app(node),
                        answers: 0,
                        before_seen: false,
                        called_off: false,
                    },
                );
                (self.seam.press(key, node, kind), None)
            }
            Action::FinalRead(index) => {
                if self.seam.busy() {
                    return;
                }
                (self.seam.final_read(), self.key_number(index))
            }
            Action::End(index) => {
                let Some(key) = self.key_number(index) else {
                    return;
                };
                if self.seam.call_off(key) {
                    self.keys.get_mut(&key).expect("a key").called_off = true;
                }
                (Vec::new(), None)
            }
            Action::Move { node } => {
                self.change(node_number(node), |state, _| state.caret += 1);
                return;
            }
            Action::Select { node } => {
                self.change(node_number(node), |state, change| state.selection = change);
                return;
            }
            Action::EditBeside { node } => {
                self.change(node_number(node), |state, change| {
                    state.beside = (change, change);
                });
                return;
            }
            Action::EditUnit { node } => {
                self.change(node_number(node), |state, change| state.unit = change);
                return;
            }
            Action::EditLine { node } => {
                self.change(node_number(node), |state, change| state.line = change);
                return;
            }
            Action::Event { node } => {
                let node = node_number(node);
                if self.released.contains(&node) {
                    return;
                }
                (self.seam.event(node), None)
            }
            Action::Run => {
                if !self.seam.fresh() {
                    return;
                }
                let apps = self.apps.clone();
                (
                    self.seam
                        .run(&|node| apps.get(&node).copied().unwrap_or_default()),
                    None,
                )
            }
            Action::Finish => {
                if !self.seam.busy() {
                    return;
                }
                (self.seam.finish(), None)
            }
            Action::Panic => {
                if !self.seam.busy() {
                    return;
                }
                (self.seam.panic(), None)
            }
            Action::Abandon => {
                if !self.seam.can_abandon() {
                    return;
                }
                (self.seam.abandon(), None)
            }
            Action::Replace { node } => {
                if !self.seam.fresh() {
                    return;
                }
                let node = node_number(node);
                self.newest.remove(&node);
                (self.seam.replace(node), None)
            }
            Action::FocusElsewhere => {
                if !self.seam.fresh_event() {
                    return;
                }
                (self.seam.focus_elsewhere(), None)
            }
            Action::Release { node } => {
                let node = node_number(node);
                // Core holds a node while a key on it is unanswered.
                let open = self
                    .keys
                    .values()
                    .any(|record| record.node == node && record.answers == 0);
                if self.seam.busy() || self.released.contains(&node) || open {
                    return;
                }
                self.released.insert(node);
                self.newest.remove(&node);
                (self.seam.release(node), None)
            }
        };
        self.observe(&seen, compared_key);
        self.compare();
    }

    /// Takes in the answers and reads a step reported. A no-read answer is
    /// compared with the state at this step when `compared_key` names the
    /// key, that is when the step was Core's final read.
    fn observe(&mut self, seen: &[Seen], compared_key: Option<u64>) {
        for item in seen {
            self.log.push(format!("{item:?}"));
            match *item {
                Seen::ReadCommitted { node, read } => {
                    self.reads.insert(read, self.app(node));
                    self.newest.insert(node, read);
                }
                Seen::BeforeAnswered { key } => {
                    let record = self.keys.get_mut(&key).expect("a key");
                    assert!(
                        record.answers == 0 && !record.before_seen,
                        "key {key}'s before read's answer came after another: {:?}",
                        self.log
                    );
                    record.before_seen = true;
                }
                Seen::Answered { key, read, effect } => {
                    self.answered(key, read, effect, compared_key);
                }
            }
        }
    }

    fn answered(&mut self, key: u64, read: Option<u64>, effect: bool, compared_key: Option<u64>) {
        let Some(record) = self.keys.get(&key).copied() else {
            return;
        };
        assert!(
            !record.called_off,
            "key {key} answered after Core called it off: {:?}",
            self.log
        );
        assert_eq!(
            record.answers, 0,
            "key {key} answered twice: {:?}",
            self.log
        );
        self.keys.get_mut(&key).expect("a key").answers += 1;
        let state = match read {
            Some(read) => Some(self.reads[&read]),
            // After Core's final read the key's answer is a comparison of
            // the state then, even when it carries no read.
            None if compared_key == Some(key) => Some(self.app(record.node)),
            None => None,
        };
        if let Some(state) = state {
            let expected = had_effect(record.kind, record.arrival, state);
            assert_eq!(
                effect, expected,
                "key {key}'s answer says effect {effect}, but its state then {state:?} against its arrival {:?} says {expected}: {:?}",
                record.arrival, self.log
            );
        }
    }

    /// The last read of each node the reducer keeps is the model's.
    fn compare(&self) {
        for node in [1, 2] {
            let expected = if self.released.contains(&node) {
                None
            } else {
                self.newest.get(&node).copied()
            };
            assert_eq!(
                self.seam.last_read(node),
                expected,
                "the last read of node {node}: {:?}",
                self.log
            );
        }
    }

    /// Ends the case: every program ends, every key still open has Core's
    /// final read, and each key Core did not call off was answered once.
    fn drain(&mut self) {
        for _ in 0..64 {
            while self.seam.busy() {
                self.step(Action::Run);
                self.step(Action::Finish);
            }
            let open: Vec<u64> = self
                .keys
                .iter()
                .filter(|(_, record)| record.answers == 0 && !record.called_off)
                .map(|(key, _)| *key)
                .collect();
            let Some(&key) = open.first() else {
                break;
            };
            let index = self.keys.keys().position(|other| *other == key);
            let index = u8::try_from(index.expect("a key")).expect("few keys");
            self.step(Action::FinalRead(index));
        }
        for (key, record) in &self.keys {
            if record.called_off {
                assert_eq!(
                    record.answers, 0,
                    "key {key}, called off, was answered: {:?}",
                    self.log
                );
            } else {
                assert_eq!(
                    record.answers, 1,
                    "key {key} was answered {} times: {:?}",
                    record.answers, self.log
                );
            }
        }
    }
}

/// The strategy of one action.
fn action() -> impl Strategy<Value = Action> {
    use proptest::prelude::*;
    let node = 0_u8..2;
    prop_oneof![
        (0_u8..3, node.clone()).prop_map(|(kind, node)| Action::Press { kind, node }),
        (0_u8..4).prop_map(Action::FinalRead),
        (0_u8..4).prop_map(Action::End),
        node.clone().prop_map(|node| Action::Move { node }),
        node.clone().prop_map(|node| Action::Select { node }),
        node.clone().prop_map(|node| Action::EditBeside { node }),
        node.clone().prop_map(|node| Action::EditUnit { node }),
        node.clone().prop_map(|node| Action::EditLine { node }),
        node.clone().prop_map(|node| Action::Event { node }),
        Just(Action::Run),
        Just(Action::Finish),
        Just(Action::Panic),
        Just(Action::Abandon),
        node.clone().prop_map(|node| Action::Replace { node }),
        Just(Action::FocusElsewhere),
        node.prop_map(|node| Action::Release { node }),
    ]
}

/// The seed and case count of the gate's run.
const SEED: u64 = 20_261_109;
const CASES: u32 = 2_048;

/// Runs the generated cases `config` draws, each twice from a new reducer,
/// shrinking a failure to its smallest case.
///
/// # Panics
///
/// On a failing case, naming the smallest.
pub(super) fn run_generated(config: Config, again: &str) {
    let mut runner = TestRunner::new(config);
    let strategy = proptest::collection::vec(action(), 1..40);
    let result = runner.run(&strategy, |actions| {
        let first = World::case(&actions);
        let second = World::case(&actions);
        assert_eq!(first.log, second.log, "two runs of one case differ");
        Ok(())
    });
    match result {
        Ok(()) => {}
        Err(TestError::Fail(reason, case)) => panic!("{reason}\n{}", fixed_report(&case, again)),
        Err(error) => panic!("{error}"),
    }
}

/// The report of a failing case: `case` as a line ready to paste into
/// [`fixed`], with the commands that run the fixed cases and the run that
/// found it.
fn fixed_report(case: &[Action], again: &str) -> String {
    let steps: Vec<String> = case
        .iter()
        .map(|action| format!("Action::{action:?}"))
        .collect();
    format!(
        "the smallest failing case, as a line for `fixed()` in crates/verbatim-outpost-logic/src/tests/generated.rs:\n        vec![{}],\n\
         the fixed cases alone: cargo test -p verbatim-outpost-logic --lib -- --exact tests::generated::fixed_cases_answer_each_key_by_its_arrival\n\
         the run that found it again: {again}",
        steps.join(", ")
    )
}

/// Cases of an application's independently timed changes and events, with
/// Core's orders arriving at any step, keep what a key's answer and the last
/// read exist to decide, compared with the reference model after every step.
#[test]
fn generated_cases_answer_each_key_by_its_arrival() {
    run_generated(
        super::explore::config(SEED, CASES),
        "cargo test -p verbatim-outpost-logic --lib -- --exact tests::generated::generated_cases_answer_each_key_by_its_arrival",
    );
}

/// The cases the generated test failed on against today's code, each kept
/// as its actions.
fn fixed() -> Vec<Vec<Action>> {
    vec![
        // The history search: the application moved the caret after the last
        // report and before the key arrived, with no event raised; the
        // baseline, the report read before the key, is not the state at
        // arrival, so a key that moved nothing is answered as having moved.
        vec![
            Action::Event { node: 1 },
            Action::Run,
            Action::Finish,
            Action::Move { node: 1 },
            Action::Press { kind: 0, node: 1 },
            Action::Run,
            Action::Finish,
        ],
        // The first check: a key with no baseline finds nothing at its first
        // check, and the next caret event of the node, raised for nothing,
        // is taken as evidence.
        vec![
            Action::Press { kind: 0, node: 1 },
            Action::Run,
            Action::Finish,
            Action::Event { node: 1 },
            Action::Run,
            Action::Finish,
        ],
        // The final read: the application changed the characters beside the
        // caret with no event, and the watch's bound answers no effect.
        vec![
            Action::Press { kind: 0, node: 1 },
            Action::EditBeside { node: 1 },
        ],
    ]
}

/// The found cases keep the key's answers.
#[test]
fn fixed_cases_answer_each_key_by_its_arrival() {
    for case in fixed() {
        let _ = World::case(&case);
    }
}
