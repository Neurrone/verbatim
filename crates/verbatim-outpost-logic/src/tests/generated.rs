//! The generated test of a caret key's answer and the outpost's last read:
//! lists of actions, each made valid when it is applied, given to the
//! reducer through its entries, its programs' commits and its release, and
//! compared after every step with a much simpler reference model.
//!
//! The model is written from `docs/parity/text.md`,
//! `docs/nvda/editable-text-and-terminals.md` and Dickson's dated decisions
//! on the caret and selection rework, never from the code's fields. It holds
//! each node's text state at every moment, the newest read committed of it,
//! and each key's baseline, and checks, after every step:
//!
//! - a key answers that it had an effect exactly when the state its read
//!   found differs from its baseline, by the decisions' evidence: the caret
//!   having left where it was, the characters beside it changing, the unit
//!   at the caret changing after Delete, the caret's line changing after a
//!   key that deletes, or the selection changing. A held key's baseline is
//!   the state when its before read ran; a key with no before read, not held
//!   or given up on, is judged from what the node's newest committed read
//!   found when its request arrived, so a change between that read and the
//!   key counts as the key's, the recorded limit;
//! - a key with no before read and no last read claims no effect: its one
//!   answer is an "ended" answer, that it could not be watched;
//! - an "ended" answer after Core's final read says no effect, compared
//!   with the state at that step like a read's answer;
//! - a key Core has not called off is answered once, and a key it called off
//!   gets no answer after the call-off is handled;
//! - a key ended by a newer key, a focus elsewhere, a replaced element or a
//!   check that ended without its result gets exactly one "ended" answer,
//!   never one from a read made after that; an "ended" answer with none of
//!   those causes, no last read to judge from or Core's final read, is a
//!   failure;
//! - a held key's before read's answer comes before any other of its key;
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
//! key's evidence in `crates/verbatim-outpost/src/text/mod.rs` (`baseline`,
//! `evidence`), which this crate cannot reach. The seam calls one function
//! with the facts a read yields, `seam::judge_evidence`; its body is
//! today's judgement transcribed here, and is the one thing the builder
//! replaces with the real call once the pure judgement moves into this
//! crate. Until then the history search and first check defects in
//! [`fixed`] are shown only in that transcription, read against the
//! reducer's real facts, not in a run of the outpost's own code. Today's
//! code has no input for Core's final read or a key's end, and no before
//! read's answer: the seam gives the final read as the watch's bound, the
//! closest effect, and fails with `cannot_express` for the call-off and a
//! held key's before read, so that those checks fail rather than pass
//! comparing nothing.

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

/// Whether a key is held for its before read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Holding {
    /// Not held, as in an elevated window: judged from the last read.
    NotHeld,
    /// Held, its baseline its before read.
    Held,
    /// Held, but its before read passed 150 ms and it was given up on:
    /// judged from the last read.
    GivenUp,
}

impl Holding {
    fn of(how: u8) -> Self {
        match how % 3 {
            0 => Self::NotHeld,
            1 => Self::Held,
            _ => Self::GivenUp,
        }
    }
}

/// Whether `now` shows an effect of a key of `kind` whose baseline is `baseline`
/// (the decisions' definition of evidence).
fn had_effect(kind: KeyKind, baseline: TextState, now: TextState) -> bool {
    now.caret != baseline.caret
        || now.beside != baseline.beside
        || now.selection != baseline.selection
        || (kind == KeyKind::Delete && now.unit != baseline.unit)
        || (kind.deletes() && now.line != baseline.line)
}

/// What a step of the seam reports back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// A read of a node committed, with its number.
    ReadCommitted {
        node: u64,
        read: u64,
        state: TextState,
    },
    /// A key's request began to be handled.
    KeyHandled { key: u64 },
    /// A key's before read answered: today's reducer has no such answer.
    #[expect(dead_code, reason = "the rework's seam reports it")]
    BeforeAnswered { key: u64, state: TextState },
    /// A key answered from a read: the read, and whether it says the key had
    /// an effect.
    Answered { key: u64, read: u64, effect: bool },
    /// A key's "ended" answer, resting on no read.
    Ended { key: u64 },
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

    /// Fails the case at an order or answer today's code cannot express, so
    /// that its check fails rather than comparing nothing.
    fn cannot_express(what: &str) -> ! {
        panic!("today's code cannot express {what}: the rework's seam delivers it")
    }

    /// How a read compares with one earlier read, each by the comparisons
    /// only the provider can make.
    #[derive(Clone, Copy, Debug)]
    #[expect(
        clippy::struct_excessive_bools,
        reason = "each is one comparison the provider makes"
    )]
    struct Compared {
        caret_moved: bool,
        selection_moved: bool,
        beside_changed: bool,
        unit_changed: bool,
        line_changed: bool,
    }

    /// The facts a read yields, for the judgement of a key's evidence.
    struct ReadFacts {
        /// The read against each caret the outpost reported of the node,
        /// oldest first, with when each was read.
        reported: Vec<(u64, Compared)>,
        /// The read against where Core knew the caret.
        since: Option<Compared>,
        pressed_at_ms: u64,
        /// The key compares the unit at the caret.
        compares_unit: bool,
        deletes: bool,
        /// A caret event prompted the check.
        caret_event: bool,
    }

    /// Whether `read` is the key's evidence: today's judgement, transcribed
    /// from `baseline` and `evidence` in
    /// `crates/verbatim-outpost/src/text/mod.rs` (the newest caret the
    /// outpost reported from a read before the key, else where Core knew it;
    /// with neither, a caret event alone is evidence). This body is the one
    /// thing the builder replaces with the real call.
    fn judge_evidence(read: &ReadFacts) -> bool {
        let reported = read
            .reported
            .iter()
            .rev()
            .find(|(read_at_ms, _)| *read_at_ms < read.pressed_at_ms)
            .map(|(_, compared)| compared);
        let Some(compared) = reported.or(read.since.as_ref()) else {
            return read.caret_event;
        };
        compared.caret_moved
            || compared.selection_moved
            || compared.beside_changed
            || (read.compares_unit && compared.unit_changed)
            || (read.deletes && reported.is_some() && compared.line_changed)
    }

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
        /// An event's program has taken the watch of its node, if any.
        taken: bool,
        /// The key whose check this program holds, ended if it ends first.
        held: Option<u64>,
        /// An event's program has read and not yet committed.
        read: Option<(u64, TextState)>,
    }

    /// The reducer under test with the programs the world runs for it.
    #[derive(Clone)]
    pub(super) struct Seam {
        reducer: Reducer,
        answers: Answers,
        at: u64,
        flight: Option<Flight>,
        /// What each read found, by its number.
        reads: BTreeMap<u64, TextState>,
        /// Each node's newest read Core has been told of.
        core_knows: BTreeMap<u64, u64>,
        /// The read the commit being made rests on.
        pending_read: Option<u64>,
    }

    impl Seam {
        pub(super) fn new() -> Self {
            let mut reducer = Reducer::new();
            // A program's deadline is short, so that an abandonment's clock
            // does not reach the watch's own bound, which is not its cause.
            let _ = reducer.input(Input::HandlingDeadline(Duration::from_millis(1)), now(1));
            Self {
                reducer,
                answers: Answers::default(),
                at: 1,
                flight: None,
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

        /// The seam's clock, which read numbers are taken from.
        pub(super) fn clock(&self) -> u64 {
            self.at
        }

        /// The key whose check the program in flight holds.
        pub(super) fn holds(&self) -> Option<u64> {
            self.flight.as_ref().and_then(|flight| flight.held)
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
            // A key's request is handled before the end of the older key
            // its dispatch sends.
            for effect in &effects {
                if let Effect::Dispatch(program) = effect
                    && let Kind::Key { key, .. } = Self::kind_of(program)
                {
                    seen.push(Seen::KeyHandled { key });
                }
            }
            for effect in effects {
                match effect {
                    Effect::Dispatch(program) => {
                        let kind = Self::kind_of(&program);
                        let held = match kind {
                            Kind::Key { key, .. } => Some(key),
                            _ => None,
                        };
                        self.flight = Some(Flight {
                            request: program.request,
                            kind,
                            done: false,
                            taken: false,
                            held,
                            read: None,
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
                            let key = request_id.number;
                            seen.push(match outcome {
                                QueryOutcome::Done(QueryResult::Text(TextReply::Done)) => {
                                    let read = self.pending_read.take().expect("its read");
                                    Seen::Answered {
                                        key,
                                        read,
                                        effect: true,
                                    }
                                }
                                // Today's closest effect of an "ended" answer:
                                // the watch's end, or an abandoned or failed
                                // check.
                                _ => Seen::Ended { key },
                            });
                        }
                    }
                    _ => {}
                }
            }
            seen
        }

        fn kind_of(program: &Program) -> Kind {
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
        pub(super) fn press(
            &mut self,
            key: u64,
            node: u64,
            kind: KeyKind,
            holding: Holding,
        ) -> Vec<Seen> {
            if holding == Holding::Held {
                cannot_express("a held key's before read");
            }
            self.tick_clock();
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

        /// Core's final read of `key` arrives: today, its watch's bound.
        pub(super) fn final_read(&mut self, key: u64) -> Vec<Seen> {
            self.tick_clock();
            let watch = &self.reducer.facts.follow.watch;
            if watch
                .open()
                .is_none_or(|open| open.request_id.number != key)
            {
                return Vec::new();
            }
            let Some(due) = watch.due() else {
                return Vec::new();
            };
            self.at = self.at.max(due / 1_000);
            let now = now(self.at);
            let effects = self.reducer.tick(now);
            let effects = with_answers(&mut self.reducer, &mut self.answers, effects, now);
            self.effects(effects)
        }

        /// Core calls a key off: today nothing carries the order.
        #[expect(
            clippy::unused_self,
            reason = "the rework's seam sends the order through the reducer"
        )]
        pub(super) fn call_off(&mut self, _key: u64) -> Vec<Seen> {
            cannot_express("Core calling a key off")
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

        /// The facts a read of `node` finding `now` yields for `watch`, and
        /// the judgement of them.
        fn judge(&self, node: u64, watch: &CaretWatch, now: TextState, caret_event: bool) -> bool {
            let compared = |read: u64| {
                let then = self.reads[&read];
                Compared {
                    caret_moved: then.caret != now.caret,
                    selection_moved: then.selection != now.selection,
                    beside_changed: then.beside != now.beside,
                    unit_changed: then.unit != now.unit,
                    line_changed: then.line != now.line,
                }
            };
            let reported = self
                .reducer
                .facts
                .text
                .edit(node)
                .map_or_else(Vec::new, |facts| {
                    facts
                        .carets
                        .iter()
                        .map(|seen| (seen.read_at_ms, compared(seen.caret.anchor.0)))
                        .collect()
                });
            judge_evidence(&ReadFacts {
                reported,
                since: watch.since.map(|position| compared(position.anchor.0)),
                pressed_at_ms: watch.pressed_at_ms,
                compares_unit: watch.compare.is_some(),
                deletes: watch.deletes,
                caret_event,
            })
        }

        fn take_read(&mut self, state: TextState) -> u64 {
            let read = self.at;
            self.reads.insert(read, state);
            read
        }

        fn commit(&mut self, request: Request, change: Change) -> Vec<Seen> {
            self.drive(Input::Commit {
                request,
                change: Box::new(change),
            })
        }

        /// The watch on `node` the program in flight holds.
        fn held_watch(&self, node: u64) -> Option<(CaretWatch, u64)> {
            match &self.reducer.facts.follow.watch {
                Watch::Checking { open, .. } if open.node_id.number() == node => {
                    Some((open.watch.clone(), open.request_id.number))
                }
                _ => None,
            }
        }

        /// The program in flight makes its next step: an event's takes its
        /// node's watch, reads `app`, then commits, a step each; the rest
        /// read and commit at once.
        pub(super) fn run(&mut self, app: &impl Fn(u64) -> TextState) -> Vec<Seen> {
            self.tick_clock();
            let Some(flight) = self.flight.as_mut().filter(|flight| !flight.done) else {
                return Vec::new();
            };
            let request = flight.request;
            let taken = flight.taken;
            let kind = flight.kind.clone();
            if !matches!(kind, Kind::Event { .. }) {
                flight.done = true;
                flight.held = None;
            }
            match kind {
                Kind::Key { key, node, watch } => {
                    let state = app(node);
                    let read = self.take_read(state);
                    self.core_knows.insert(node, read);
                    let report = self.report(node, read, state);
                    let mut seen = vec![Seen::ReadCommitted { node, read, state }];
                    let change = if self.judge(node, &watch, state, false) {
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
                Kind::Event { node } => self.run_event(request, node, taken, app),
                Kind::Other => Vec::new(),
            }
        }

        fn run_event(
            &mut self,
            request: Request,
            node: u64,
            taken: bool,
            app: &impl Fn(u64) -> TextState,
        ) -> Vec<Seen> {
            let mut seen = Vec::new();
            if !taken {
                seen = self.commit(
                    request,
                    Change {
                        watch: Some(WatchChange::Take(NodeId::new(node))),
                        ..Change::default()
                    },
                );
                let held = self.held_watch(node).map(|(_, key)| key);
                let flight = self.flight.as_mut().expect("the event's program");
                flight.taken = true;
                flight.held = held;
                if held.is_some() {
                    return seen;
                }
            }
            // The read and its commit are steps apart, so a key can arrive
            // between them.
            let pending = self.flight.as_ref().and_then(|flight| flight.read);
            let Some((read, state)) = pending else {
                let state = app(node);
                let read = self.take_read(state);
                self.flight.as_mut().expect("the event's program").read = Some((read, state));
                return seen;
            };
            let flight = self.flight.as_mut().expect("the event's program");
            flight.done = true;
            flight.held = None;
            self.core_knows.insert(node, read);
            let report = self.report(node, read, state);
            seen.push(Seen::ReadCommitted { node, read, state });
            let watch = self.held_watch(node).map(|(watch, _)| {
                if self.judge(node, &watch, state, true) {
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
    /// A key's request; its node, whether it deletes and how it is held,
    /// by the numbers.
    Press { kind: u8, node: u8, how: u8 },
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
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is a separate fact of the key, read by its own rule"
)]
struct KeyRecord {
    node: u64,
    kind: KeyKind,
    /// What it is judged from; `None` before a held key's before read, and
    /// for a key with no before read and no last read.
    baseline: Option<TextState>,
    /// Whether the key's before read is answered before any other.
    held: bool,
    answers: u32,
    before_seen: bool,
    called_off: bool,
    /// Its request began to be handled.
    handled: bool,
    /// What ended it, a newer key, a focus elsewhere, a replaced element or
    /// a check ended without its result, with the seam's clock then.
    ended: Option<u64>,
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

    /// Records `cause` as what ended every key it names, of those handled
    /// and not yet answered.
    fn ended_by(&mut self, cause: impl Fn(u64, &KeyRecord) -> bool) {
        let clock = self.seam.clock();
        for (key, record) in &mut self.keys {
            if record.handled
                && record.answers == 0
                && record.ended.is_none()
                && cause(*key, record)
            {
                record.ended = Some(clock);
            }
        }
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
            Action::Press { kind, node, how } => {
                let node = node_number(node);
                if self.released.contains(&node) {
                    return;
                }
                let key = u64::try_from(self.keys.len()).expect("few keys") + 1;
                let kind = KeyKind::of(kind);
                let holding = Holding::of(how);
                // A held key's baseline comes with its before read.
                let baseline = match holding {
                    Holding::Held => None,
                    Holding::NotHeld | Holding::GivenUp => {
                        self.newest.get(&node).map(|read| self.reads[read])
                    }
                };
                self.keys.insert(
                    key,
                    KeyRecord {
                        node,
                        kind,
                        baseline,
                        held: holding == Holding::Held,
                        answers: 0,
                        before_seen: false,
                        called_off: false,
                        handled: false,
                        ended: None,
                    },
                );
                (self.seam.press(key, node, kind, holding), None)
            }
            Action::FinalRead(index) => {
                if self.seam.busy() {
                    return;
                }
                let Some(key) = self.key_number(index) else {
                    return;
                };
                (self.seam.final_read(key), Some(key))
            }
            Action::End(index) => {
                let Some(key) = self.key_number(index) else {
                    return;
                };
                let record = self.keys.get_mut(&key).expect("a key");
                if record.answers != 0 || record.called_off {
                    return;
                }
                record.called_off = true;
                (self.seam.call_off(key), None)
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
                let held = self.seam.holds();
                self.ended_by(|key, _| Some(key) == held);
                (self.seam.finish(), None)
            }
            Action::Panic => {
                if !self.seam.busy() {
                    return;
                }
                let held = self.seam.holds();
                self.ended_by(|key, _| Some(key) == held);
                (self.seam.panic(), None)
            }
            Action::Abandon => {
                if !self.seam.can_abandon() {
                    return;
                }
                let held = self.seam.holds();
                self.ended_by(|key, _| Some(key) == held);
                (self.seam.abandon(), None)
            }
            Action::Replace { node } => {
                if !self.seam.fresh_event() {
                    return;
                }
                let node = node_number(node);
                self.ended_by(|_, record| record.node == node);
                self.newest.remove(&node);
                (self.seam.replace(node), None)
            }
            Action::FocusElsewhere => {
                if !self.seam.fresh_event() {
                    return;
                }
                self.ended_by(|_, _| true);
                (self.seam.focus_elsewhere(), None)
            }
            Action::Release { node } => {
                let node = node_number(node);
                // Core holds a node while a key on it is unanswered.
                let open = self
                    .keys
                    .values()
                    .any(|record| record.node == node && record.answers == 0 && !record.called_off);
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

    /// Takes in the answers and reads a step reported; `compared_key` names
    /// the key of Core's final read when this step was one.
    fn observe(&mut self, seen: &[Seen], compared_key: Option<u64>) {
        for item in seen {
            self.log.push(format!("{item:?}"));
            match *item {
                Seen::ReadCommitted { node, read, state } => {
                    self.reads.insert(read, state);
                    self.newest.insert(node, read);
                }
                Seen::KeyHandled { key } => {
                    // A newer key ends the keys older than it.
                    self.ended_by(|older, _| older < key);
                    self.keys.get_mut(&key).expect("a key").handled = true;
                }
                Seen::BeforeAnswered { key, state } => {
                    let record = self.keys.get_mut(&key).expect("a key");
                    assert!(
                        record.answers == 0 && !record.before_seen,
                        "key {key}'s before read's answer came after another: {:?}",
                        self.log
                    );
                    record.before_seen = true;
                    record.baseline = Some(state);
                }
                Seen::Answered { key, read, effect } => {
                    let record = self.count_answer(key);
                    assert!(
                        record.ended.is_none_or(|ended| read <= ended),
                        "key {key} answered from read {read}, made after it was ended: {:?}",
                        self.log
                    );
                    let Some(baseline) = record.baseline else {
                        panic!(
                            "key {key} has no last read to judge from, yet answered from read {read}: {:?}",
                            self.log
                        );
                    };
                    let state = self.reads[&read];
                    let expected = had_effect(record.kind, baseline, state);
                    assert_eq!(
                        effect, expected,
                        "key {key}'s answer says effect {effect}, but its state then {state:?} against its baseline {baseline:?} says {expected}: {:?}",
                        self.log
                    );
                }
                Seen::Ended { key } => {
                    let record = self.count_answer(key);
                    // With no last read the key could not be watched, and
                    // this is its one answer; otherwise something ended it,
                    // or Core's final read found no effect.
                    if let (None, Some(baseline)) = (record.ended, record.baseline) {
                        assert_eq!(
                            compared_key,
                            Some(key),
                            "key {key} ended with nothing that ends it: {:?}",
                            self.log
                        );
                        let state = self.app(record.node);
                        assert!(
                            !had_effect(record.kind, baseline, state),
                            "key {key}'s final read says no effect, but its state {state:?} differs from its baseline {baseline:?}: {:?}",
                            self.log
                        );
                    }
                }
            }
        }
    }

    /// Counts one answer of `key`, after the rules every answer keeps: none
    /// after Core called it off, none twice, and a held key's before read's
    /// answer first.
    fn count_answer(&mut self, key: u64) -> KeyRecord {
        let record = self.keys.get_mut(&key).expect("a key");
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
        assert!(
            !record.held || record.before_seen,
            "held key {key} answered before its before read's answer: {:?}",
            self.log
        );
        record.answers += 1;
        *record
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
                if self.seam.fresh() {
                    self.step(Action::Run);
                } else {
                    self.step(Action::Finish);
                }
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
            if !record.called_off {
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
        (0_u8..3, node.clone(), 0_u8..3).prop_map(|(kind, node, how)| Action::Press {
            kind,
            node,
            how
        }),
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
         the fixed cases alone: cargo test -p verbatim-outpost-logic --lib -- --exact tests::generated::fixed_cases_answer_each_key_by_its_baseline\n\
         the run that found it again: {again}",
        steps.join(", ")
    )
}

/// Cases of an application's independently timed changes and events, with
/// Core's orders arriving at any step, keep what a key's answer and the last
/// read exist to decide, compared with the reference model after every step.
#[test]
fn generated_cases_answer_each_key_by_its_baseline() {
    run_generated(
        super::explore::config(SEED, CASES),
        "cargo test -p verbatim-outpost-logic --lib -- --exact tests::generated::generated_cases_answer_each_key_by_its_baseline",
    );
}

/// The cases the generated test failed on against today's code, each kept
/// as its actions. The history search and first check cases fail in the
/// transcription at `seam::judge_evidence`, not in a run of the outpost's
/// own code; the held key and call-off cases fail where the seam says
/// today's code cannot express them.
fn fixed() -> Vec<Vec<Action>> {
    let press = |how| Action::Press {
        kind: 0,
        node: 1,
        how,
    };
    vec![
        // The history search: a read begun before the key and committed after
        // its request arrived is taken as the baseline, where the newest
        // committed read when it arrived is, so the change between the two
        // reads, the key's by the recorded limit, is not an effect.
        vec![
            Action::Event { node: 1 },
            Action::Run,
            Action::Run,
            Action::Finish,
            Action::Move { node: 1 },
            Action::Event { node: 1 },
            Action::Run,
            press(0),
            Action::Run,
            Action::Finish,
            Action::Run,
            Action::Finish,
        ],
        // The first check: a key with no last read finds nothing at its
        // first check, and the next caret event of the node, raised for
        // nothing, is taken as evidence.
        vec![
            press(0),
            Action::Run,
            Action::Finish,
            Action::Event { node: 1 },
            Action::Run,
            Action::Run,
            Action::Run,
            Action::Finish,
        ],
        // The final read: the application changed the characters beside the
        // caret after a key with a last read, raising no event, and the
        // watch's bound answers no effect.
        vec![
            Action::Event { node: 1 },
            Action::Run,
            Action::Run,
            Action::Finish,
            press(0),
            Action::Run,
            Action::Finish,
            Action::EditBeside { node: 1 },
        ],
        // A held key's before read: the seam cannot express it today.
        vec![press(1)],
        // Core calling a key off: the seam cannot express it today.
        vec![press(0), Action::End(0)],
    ]
}

/// The found cases keep the key's answers. Every case is run, whichever
/// fail first, so a run names each that fails.
#[test]
fn fixed_cases_answer_each_key_by_its_baseline() {
    let failing: Vec<usize> = fixed()
        .iter()
        .enumerate()
        .filter(|(_, case)| {
            std::panic::catch_unwind(|| {
                let _ = World::case(case);
            })
            .is_err()
        })
        .map(|(number, _)| number)
        .collect();
    assert_eq!(failing, Vec::<usize>::new(), "the fixed cases that fail");
}
