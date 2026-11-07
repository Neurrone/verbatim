//! The caret watch's generated test (`books/dev/src/reference/crates/verbatim-outpost-logic.md`,
//! the caret watch): every sequence of up to
//! [`DEPTH`] steps of the inputs that move a caret key's watch, given to
//! [`Reducer::input`] and [`Reducer::tick`], enumerated without randomness. After every step it
//! checks four invariants, with no model of the watch's lifecycle:
//!
//! - each caret key's query has at most one reply, and exactly one, or is
//!   left to the outpost's end, once the sequence ends with `Close`;
//! - [`Reducer::next_timer`] gives no time for the watch while a program is
//!   in flight or a release is pending;
//! - at each accepted commit, the program's copy of the watch, with its own
//!   commits applied, equals the reducer's apart from its times;
//! - no idle answer is sent while the watch is open or the program in
//!   flight is deferred.
//!
//! A due time is also acted on as soon as nothing else is ready. The
//! watch's bound, its poll's timings and the reason each reply gives are
//! the examples of `watch.rs` and `src/tests.rs`.

use std::collections::BTreeMap;

use super::*;

/// How many steps a sequence has at most.
const DEPTH: usize = 6;

/// One step of a sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// A caret key's query arrives.
    Key,
    /// A caret event of the watch's node arrives.
    CaretEvent,
    /// The key's program opens the watch, polling or not.
    Open(bool),
    /// An event's program takes the watch.
    Take,
    /// The program holding the watch found the evidence.
    Answered,
    /// The program holding the watch found nothing.
    Watching,
    /// The program in flight reports a focus elsewhere.
    ReportElsewhere,
    /// The program in flight ends.
    Finish,
    /// The program in flight panics.
    Panic,
    /// The program holding the watch ends cut at its value change check.
    Cut,
    /// The program in flight's deadline passes.
    Deadline,
    /// The watch's due time comes.
    Due,

    /// Core's held nodes arrive.
    NodesHeld,
    /// The release they asked for lets go of the watch's node.
    Released,
    /// Core's idle round arrives.
    Round,
    /// The outpost closes.
    Close,
}

const ACTIONS: [Action; 17] = [
    Action::Key,
    Action::CaretEvent,
    Action::Open(false),
    Action::Open(true),
    Action::Take,
    Action::Answered,
    Action::Watching,
    Action::ReportElsewhere,
    Action::Finish,
    Action::Panic,
    Action::Cut,
    Action::Deadline,
    Action::Due,
    Action::NodesHeld,
    Action::Released,
    Action::Round,
    Action::Close,
];

/// What a program is, as the test follows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A caret key's program, of this query.
    Key(u64),
    /// A caret event's program.
    Event,
    /// A read of the console poll.
    PollRead,
    /// The finish of a cut check.
    Finish,
    /// Anything else.
    Other,
}

/// The program in flight, from its dispatch: its copy of the facts, with
/// its own commits applied.
#[derive(Clone, Debug)]
struct Flight {
    request: Request,
    kind: Kind,
    copy: Facts,
    /// Whether it opened a watch or committed a check's result.
    done: bool,
}

/// One sequence's state: the reducer, its recorded reads and the test's
/// own bookkeeping.
#[derive(Clone)]
struct Gen {
    reducer: Reducer,
    answers: Answers,
    at: u64,
    keys: u64,
    events: u32,
    flight: Option<Flight>,
    replies: BTreeMap<u64, u32>,
    closed: bool,
    rounds: bool,
    held_pushed: bool,
    trail: Vec<Action>,
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

/// The kind of `program`.
fn kind_of(program: &Program) -> Kind {
    match &program.work {
        Work::Entry { entry, .. } => match &entry.item {
            Item::Query { request_id, .. } => Kind::Key(request_id.number),
            Item::Msaa { .. } => Kind::Event,
            _ => Kind::Other,
        },
        Work::PollRead => Kind::PollRead,
        Work::Finish { .. } => Kind::Finish,
        Work::Owed(_) | Work::Drop => Kind::Other,
    }
}

impl Gen {
    fn new() -> Self {
        Self {
            reducer: Reducer::new(),
            answers: Answers::default(),
            at: 1,
            keys: 0,
            events: 0,
            flight: None,
            replies: BTreeMap::new(),
            closed: false,
            rounds: false,
            held_pushed: false,
            trail: Vec::new(),
        }
    }

    fn holds(&self) -> bool {
        self.flight
            .as_ref()
            .is_some_and(|flight| matches!(flight.copy.follow.watch, Watch::Checking { .. }))
    }

    /// Whether `action` can be the next step.
    fn applies(&self, action: Action) -> bool {
        let flight = self.flight.as_ref();
        let kind = flight.map(|flight| flight.kind);
        let fresh = flight.is_some_and(|flight| !flight.done);
        match action {
            Action::Key => !self.closed && self.keys < 2,
            Action::CaretEvent => !self.closed && self.events < 2,
            Action::Open(_) => matches!(kind, Some(Kind::Key(_))) && fresh,
            Action::Take => kind == Some(Kind::Event) && fresh && !self.holds(),
            Action::Answered | Action::Watching => fresh && self.holds(),
            Action::ReportElsewhere => matches!(kind, Some(Kind::Event | Kind::Other)),
            Action::Finish | Action::Panic => flight.is_some(),
            // Nothing is abandoned once the outpost closes.
            Action::Deadline => flight.is_some() && !self.closed,
            Action::Cut => kind == Some(Kind::Event) && fresh && self.holds(),
            Action::Due => {
                flight.is_none()
                    && !self.closed
                    && !self.reducer.releasing
                    && self.reducer.facts.follow.watch.due().is_some()
            }

            Action::NodesHeld => !self.closed && !self.held_pushed,
            Action::Released => self.reducer.releasing,
            Action::Round => !self.closed && !self.rounds,
            Action::Close => !self.closed,
        }
    }

    fn now(&self) -> Now {
        now(self.at)
    }

    fn input(&mut self, input: Input) -> Vec<Effect> {
        let now = self.now();
        let effects = self.reducer.input(input, now);
        with_answers(&mut self.reducer, &mut self.answers, effects, now)
    }

    fn tick(&mut self, now: Now) -> Vec<Effect> {
        let effects = self.reducer.tick(now);
        with_answers(&mut self.reducer, &mut self.answers, effects, now)
    }

    fn push(&mut self, item: Item) -> Vec<Effect> {
        let at = self.at;
        self.input(Input::Entry {
            entry: entry(item, at),
            source: Source::Other,
        })
    }

    /// Commits `change` as the program in flight's, checking first that
    /// its copy agrees with the reducer when the reducer accepts it.
    fn commit(&mut self, change: Change) -> Vec<Effect> {
        let flight = self.flight.as_mut().expect("a program in flight");
        let request = flight.request;
        let accepted = self.reducer.accepts(request);
        assert!(accepted, "{:?}", self.trail);
        if accepted {
            assert_eq!(
                untimed(&flight.copy.follow.watch),
                untimed(&self.reducer.facts.follow.watch),
                "the program's copy and the reducer disagree after {:?}",
                self.trail
            );
        }
        let _ = apply_local(&mut flight.copy, request.generation, &change);
        self.input(Input::Commit {
            request,
            change: Box::new(change),
        })
    }

    fn finish(&mut self, panicked: bool, cut: Option<Box<Cut>>) -> Vec<Effect> {
        let flight = self.flight.take().expect("a program in flight");
        self.input(Input::Finished {
            request: flight.request,
            reported: false,
            panicked,
            cut,
        })
    }

    /// Takes `action`, and checks the step.
    fn step(&mut self, action: Action) {
        self.trail.push(action);
        self.at += 1;
        let effects = match action {
            Action::Key => {
                self.keys += 1;
                self.push(caret_key(self.keys))
            }
            Action::CaretEvent => {
                self.events += 1;
                self.push(edit_caret())
            }
            Action::Open(console) => {
                self.flight_mut().done = true;
                let Some(Kind::Key(key)) = self.flight.as_ref().map(|flight| flight.kind) else {
                    unreachable!("a key's program")
                };
                self.commit(opens(key, console))
            }
            Action::Take => self.commit(takes()),
            Action::Answered => {
                self.flight_mut().done = true;
                self.commit(answered())
            }
            Action::Watching => {
                self.flight_mut().done = true;
                self.commit(watching(1))
            }
            Action::ReportElsewhere => self.commit(focus_change(2, false)),
            Action::Finish | Action::Panic => self.finish(action == Action::Panic, None),
            Action::Cut => self.finish(
                false,
                Some(cut(Finish::Watch {
                    read: AnswerCut {
                        caret: 1,
                        selection: None,
                        read_at_ms: 1,
                        line: None,
                        unit: TextUnit::Character,
                        same_line: None,
                        compared: KeptSelection {
                            ends: (0, 5),
                            collapsed: false,
                            text: None,
                            report: 1,
                        },
                    },
                })),
            ),
            Action::Deadline => {
                let deadline = self.reducer.flight.deadline().expect("in flight");
                self.at = deadline / 1_000;
                self.flight = None;
                let now = self.now();
                self.tick(now)
            }
            Action::Due => {
                let due = self.reducer.facts.follow.watch.due().expect("a due time");
                self.at = self.at.max(due / 1_000);
                let now = self.now();
                self.tick(now)
            }

            Action::NodesHeld => {
                self.held_pushed = true;
                self.push(Item::NodesHeld {
                    nodes: vec![],
                    anchors: vec![],
                    acknowledged: 0,
                })
            }
            Action::Released => self.input(Input::Released {
                nodes: [1].into_iter().collect(),
                anchors: BTreeSet::new(),
                objects: false,
            }),
            Action::Round => {
                self.rounds = true;
                self.push(round(9))
            }
            Action::Close => {
                self.closed = true;
                self.input(Input::Close)
            }
        };
        self.check(&effects);
    }

    fn flight_mut(&mut self) -> &mut Flight {
        self.flight.as_mut().expect("a program in flight")
    }

    /// Follows the programs the step's effects dispatch, counts the
    /// replies, and checks the invariants.
    fn check(&mut self, effects: &[Effect]) {
        let trail = &self.trail;
        for effect in effects {
            match effect {
                Effect::Dispatch(program) => {
                    self.flight = Some(Flight {
                        request: program.request,
                        kind: kind_of(program),
                        copy: program.facts.clone(),
                        done: false,
                    });
                }
                Effect::Abandon { .. } => self.flight = None,
                Effect::Send(message) => {
                    if let OutpostToCore::Reply { request_id, .. } = &**message {
                        let count = self.replies.entry(request_id.number).or_default();
                        *count += 1;
                        assert_eq!(*count, 1, "a second reply to {request_id} after {trail:?}");
                    }
                    if matches!(**message, OutpostToCore::IdleAnswer { .. }) {
                        assert!(
                            self.reducer.facts.follow.watch.is_none()
                                && !self
                                    .reducer
                                    .flight
                                    .running()
                                    .is_some_and(|(_, current, _)| current.deferred),
                            "an idle answer with deferred work after {trail:?}"
                        );
                    }
                }
                _ => {}
            }
        }
        let watch = &self.reducer.facts.follow.watch;
        // A due time is acted on as soon as nothing else is ready.
        if !self.reducer.busy() && !self.reducer.releasing && !self.reducer.stopping {
            assert!(
                watch.due().is_none_or(|due| due > self.at * 1_000),
                "a due time not acted on after {trail:?}"
            );
        }
        // No time for the watch while a program or a release is in flight.
        if self.reducer.busy() || self.reducer.releasing {
            let mut without = self.reducer.clone();
            without.facts.follow.watch = Watch::None;
            assert_eq!(
                self.reducer.next_timer(self.now()),
                without.next_timer(self.now()),
                "the watch timed while busy after {trail:?}"
            );
        }
    }

    /// Ends the sequence: the outpost closes, and its programs end.
    fn close(mut self) {
        if !self.closed {
            self.step(Action::Close);
        }
        while self.flight.is_some() {
            self.step(Action::Finish);
        }
        for key in 1..=self.keys {
            let replied = self.replies.get(&key).copied().unwrap_or(0);
            let left = self.reducer.queries().contains(&RequestId::shell(key));
            assert!(
                (replied == 1) != left,
                "key {key} replied {replied} times, left to the end {left}, after {:?}",
                self.trail
            );
        }
    }
}

/// Every sequence from `generation` up to `depth` steps, depth first; a
/// failing sequence is printed as a line for [`found`] before the failure.
fn enumerate(generation: &Gen, depth: usize) {
    reported(&generation.trail, || generation.clone().close());
    if generation.trail.len() == depth {
        return;
    }
    for action in ACTIONS {
        if generation.applies(action) {
            let mut next = generation.clone();
            let trail: Vec<Action> = generation.trail.iter().copied().chain([action]).collect();
            reported(&trail, || next.step(action));
            enumerate(&next, depth);
        }
    }
}

/// Runs `check`, the last step of the sequence `trail`; when it fails,
/// prints `trail` as a line ready to paste into [`found`] and the command
/// that runs the found sequences, then fails as it did.
fn reported(trail: &[Action], check: impl FnOnce()) {
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(check)) {
        let actions: Vec<String> = trail
            .iter()
            .map(|action| format!("Action::{action:?}"))
            .collect();
        eprintln!(
            "the failing sequence, as a line for `found()` in crates/verbatim-outpost-logic/src/tests/generated.rs:\n        vec![{}],\n\
             the found sequences alone: cargo test -p verbatim-outpost-logic --lib -- --exact tests::generated::found_sequences_answer_each_key_once_and_keep_the_watchs_rules",
            actions.join(", ")
        );
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn every_short_sequence_answers_each_key_once_and_keeps_the_watchs_rules() {
    // Each depth in turn, so the sequence a failure prints is one of the
    // shortest that fail.
    for depth in 1..=DEPTH {
        enumerate(&Gen::new(), depth);
    }
}

/// The sequences the enumeration failed on, each kept as its list of
/// actions, as the failure's report gives it.
fn found() -> Vec<Vec<Action>> {
    Vec::new()
}

/// Every found sequence applies, step by step, and answers each key once
/// and keeps the watch's rules through its close.
#[test]
fn found_sequences_answer_each_key_once_and_keep_the_watchs_rules() {
    for sequence in found() {
        let mut generation = Gen::new();
        for action in sequence {
            assert!(
                generation.applies(action),
                "{action:?} after {:?}",
                generation.trail
            );
            generation.step(action);
        }
        generation.close();
    }
}
