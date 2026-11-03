//! Core's reducer slice's generated test (`docs/design/logic-tests.md`,
//! "Core's reducer"): lists of actions, each made valid when it is applied
//! by resolving its target against the model, given to [`reduce`] in
//! order. After every input the query table's invariants of the
//! enumeration beside it (`queries/tests.rs`) are checked against the
//! wire's view of the requests, and the slice's own against a reference
//! model: the outpost generations live and ended, the ids given and ended,
//! the focus's lifecycle (taken, alive, left) with the front and the
//! newest focus stamp, and the terminal models' generations in their order
//! of reading. The terminal's quiet-period timer, fired at the due time the
//! state gives, is due later than that time or not at all. Each case ends
//! by answering every request still open, and its state round-trips
//! through its serialisation after every action.
//!
//! The model leaves out words, carets, text, chains, say-all's buffers and
//! the flood policy, and the held-key line, whose awaited watch alone it
//! checks (Dickson, 2026-11-02). A focus is generated only where the model
//! decides its fate plainly: in the front or with no window facts, from the
//! one application every generation reads, never a whole-window focus or a
//! group's.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestRunner};
use verbatim_model::{
    Backend, CaretKey, CaretMotion, EarlierElement, Effect, FocusText, ForegroundWindow, Input,
    KeySeq, NodeId, NormalizedEvent, OutpostId, Pid, QueryId, ReviewCommand, Role, TraceId,
    WindowFacts, WindowHandle,
};

use super::Asked as Waited;
use super::Then;
use super::tests::{self, Visit, Wire};
use crate::foreground::{Attention, Front};
use crate::reduce;
use crate::state::SrState;

/// The application every outpost generation reads.
const APPLICATION: Pid = Pid(7);

/// The two top-level windows foreground changes and focuses name.
const WINDOWS: [WindowHandle; 2] = [WindowHandle(0x100), WindowHandle(0x200)];

/// How many outpost generations a case may start.
const GENERATIONS: u64 = 4;

/// One step of a case.
#[derive(Clone, Copy, Debug)]
enum Action {
    /// A focus from a live generation (0, the oldest, or 1, the newest) on
    /// the text box (0), a button (1) or the terminal (2), with what its
    /// reader read of the element it held under that node (0 nothing, 1
    /// focused, 2 not focused, 3 gone), in the front's window or with no
    /// window facts, observed now, a little before, or at no known time.
    Focus {
        generation: u8,
        node: u8,
        earlier: u8,
        windowed: bool,
        stamp: u8,
    },
    /// The focused terminal's snapshot: its prompt alone, or (`growing`)
    /// with a ruler above it whose line number grows by one each such
    /// read, a row changed often enough that the reading rules hold it
    /// until it settles, with their quiet-period timer.
    Snapshot { growing: bool },
    /// The first snapshots of 16 other terminals, which evict the oldest
    /// models, then the focus back on the terminal.
    OtherTerminals,
    /// An answer: to the oldest (0) or newest (1) open request, to the
    /// last ended (2), to one never asked (3), or to the newest from
    /// another outpost (4), done (0), gone (1) or failed (2).
    Answer { which: u8, outcome: u8 },
    /// A caret key (0), a held key (1), the held key's due time (2), a
    /// character typed (3) or speech cut off (4).
    Key(u8),
    /// The current line read (0), the navigator's parent (1), the navigator
    /// to the focus (2), say-all (3), an activation (4) or the previous
    /// line (5).
    Command(u8),
    /// A foreground change into one of the two windows, observed now, or
    /// with `tie`, when the newest focus stamp was, unless an earlier
    /// foreground change was observed later: the listener's stamps never
    /// decrease.
    Foreground { window: u8, tie: bool },
    /// The oldest (0) or newest (1) live generation ends.
    OutpostEnded(u8),
    /// A new generation starts.
    NewGeneration,
    /// The terminal's quiet-period timer fires at its due time, as the
    /// shell reads it from the state, the clock moved on to it.
    Timer,
}

fn action() -> impl Strategy<Value = Action> {
    use proptest::prelude::*;
    prop_oneof![
        8 => (0..2u8, 0..3u8, 0..4u8, any::<bool>(), 0..3u8).prop_map(
            |(generation, node, earlier, windowed, stamp)| Action::Focus {
                generation,
                node,
                earlier,
                windowed,
                stamp,
            }
        ),
        4 => any::<bool>().prop_map(|growing| Action::Snapshot { growing }),
        1 => Just(Action::OtherTerminals),
        10 => (prop_oneof![8 => 0..2u8, 1 => 2..5u8], 0..3u8)
            .prop_map(|(which, outcome)| Action::Answer { which, outcome }),
        6 => (0..5u8).prop_map(Action::Key),
        6 => (0..6u8).prop_map(Action::Command),
        3 => (0..2u8, prop::bool::weighted(0.3))
            .prop_map(|(window, tie)| Action::Foreground { window, tie }),
        2 => (0..2u8).prop_map(Action::OutpostEnded),
        2 => Just(Action::NewGeneration),
        3 => Just(Action::Timer),
    ]
}

/// Core's focus, as the model holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Focus {
    node: NodeId,
    alive: bool,
    left: bool,
    top: Option<WindowHandle>,
}

/// The newest focus observation that passed the stale test.
#[derive(Clone, Copy, Debug)]
struct Stamp {
    outpost: OutpostId,
    observed_at_us: u64,
    top: Option<WindowHandle>,
}

/// The reference model.
#[derive(Clone, Debug, Default)]
struct Model {
    live: BTreeSet<OutpostId>,
    ended: BTreeSet<OutpostId>,
    started: u64,
    focus: Option<Focus>,
    /// The front: its window and when it was observed.
    front: Option<(WindowHandle, u64)>,
    /// The entered window: the top-level window of the newest focus taken
    /// with window facts, each generated in the front, which it enters; a
    /// foreground change never sets it.
    entered: Option<WindowHandle>,
    latest: Option<Stamp>,
    /// The terminal models, least recently read first, as node and
    /// generation, and the newest generation made.
    models: Vec<(NodeId, u64)>,
    newest_made: Option<u64>,
    /// The ids of the queries ended.
    ended_ids: BTreeSet<QueryId>,
    /// The time, in microseconds.
    now: u64,
    /// The terminal's snapshots read so far.
    reads: u64,
    /// How many of them showed a line of output growing.
    grown: usize,
}

/// The listed states a gate's cases must reach.
const LISTED: [&str; 18] = [
    "a focus taken",
    "a duplicate focus",
    "a stale focus",
    "a focus left",
    "a focus's text forgotten",
    "an outpost ended",
    "a focus dead with its outpost",
    "a model made",
    "a model evicted",
    "a model dropped",
    "a query ended",
    "an answer from elsewhere",
    "an answer to an ended query",
    "an answer to a superseded query",
    "the held keys' watch awaited",
    "the held keys resumed",
    "an answer to a model no longer live",
    "a request answered from memory",
];

/// One case's run: the state and the model side by side.
struct Run {
    state: SrState,
    wire: Wire,
    model: Model,
    trail: Vec<Action>,
    /// Each input's effects, for the run twice.
    log: Vec<String>,
    reached: BTreeMap<&'static str, u32>,
}

/// `snapshot`, a terminal snapshot's event, with a ruler above its prompt
/// showing line `length`.
fn grow(snapshot: &mut Input, length: usize) {
    let Input::Event {
        event: NormalizedEvent::TerminalSnapshot { snapshot, .. },
        ..
    } = snapshot
    else {
        return;
    };
    let line = format!("{length},1 Top");
    let text = verbatim_model::TerminalText::unpadded(&format!("{line}\r\nPS> "));
    snapshot.text = text.clone();
    snapshot.screen = text;
    snapshot.first_row = verbatim_model::TerminalText::unpadded(&line);
}

/// The window facts of a focus in top-level window `top`.
fn facts(top: WindowHandle) -> WindowFacts {
    WindowFacts {
        top_level: top,
        root_owner: top,
        topmost: false,
        under_active_window: None,
        in_foreground: true,
        foreground_window: top,
        class: String::new(),
        application: String::new(),
        alt_held: false,
        foreground_is_shell: false,
    }
}

/// The node numbered `number` of generation `outpost`: the text box (5),
/// the button (6), the terminal (7) and other terminals (10 and over).
fn node_of(outpost: OutpostId, number: u64) -> NodeId {
    NodeId::in_outpost(outpost, number)
}

/// The requests `effects` make, each by its number and node, and the key
/// whose effect it is inside, if any.
fn requests(effects: &[Effect], key: Option<TraceId>) -> Vec<(QueryId, NodeId, Option<TraceId>)> {
    effects
        .iter()
        .flat_map(|effect| match effect {
            Effect::ForKey(key, inner) => requests(std::slice::from_ref(&**inner), Some(*key)),
            Effect::Fetch(query) => vec![(query.query_id, query.node_id, key)],
            Effect::Activate { query_id, node_id } => vec![(*query_id, *node_id, key)],
            Effect::Text(request) => vec![(request.query_id, request.node_id, key)],
            _ => Vec::new(),
        })
        .collect()
}

impl Run {
    fn new() -> Self {
        let mut run = Self {
            state: SrState::new(),
            wire: Wire::default(),
            model: Model::default(),
            trail: Vec::new(),
            log: Vec::new(),
            reached: BTreeMap::new(),
        };
        run.model.started = 1;
        run.model.now = 1_000_000;
        run.model.live.insert(OutpostId(1));
        run
    }

    /// Runs `actions`, then drains.
    fn case(actions: &[Action]) -> Self {
        let mut run = Self::new();
        for &action in actions {
            run.act(action);
        }
        run.drain();
        run
    }

    fn reach(&mut self, state: &'static str) {
        *self.reached.entry(state).or_default() += 1;
    }

    /// The live generation `which` names: the oldest (0) or the newest.
    fn generation(&self, which: u8) -> Option<OutpostId> {
        if which == 0 {
            self.model.live.first().copied()
        } else {
            self.model.live.last().copied()
        }
    }

    /// Applies `action`, resolved against the model.
    #[expect(clippy::too_many_lines, reason = "one arm for each action")]
    fn act(&mut self, action: Action) {
        self.trail.push(action);
        self.model.now += 10_000;
        let now = self.model.now;
        let trace = TraceId::from_raw(100 + self.trail.len() as u64);
        let key = |motion| CaretKey {
            motion,
            select: false,
        };
        let command = |command| Input::Command {
            trace_id: trace,
            key_seq: KeySeq::NONE_YET,
            command,
            pressed_at_ms: now / 1_000,
            repeat: 0,
        };
        match action {
            Action::Focus {
                generation,
                node,
                earlier,
                windowed,
                stamp,
            } => {
                let Some(outpost) = self.generation(generation) else {
                    return;
                };
                let earlier = match earlier {
                    0 => None,
                    1 => Some(EarlierElement::Focused),
                    2 => Some(EarlierElement::NotFocused),
                    _ => Some(EarlierElement::Gone),
                };
                let observed_at_us = match stamp {
                    0 => now,
                    1 => now - 15_000,
                    _ => 0,
                };
                let input = self.focus_input(
                    outpost,
                    5 + u64::from(node),
                    earlier,
                    windowed,
                    observed_at_us,
                );
                self.focus(&input, outpost, earlier, observed_at_us);
            }
            Action::Snapshot { growing } => {
                if let Some(focus) = self.model.focus.filter(|focus| focus.node.number() == 7) {
                    self.model.reads += 1;
                    let mut input = self.snapshot(focus.node);
                    if growing {
                        self.model.grown += 1;
                        grow(&mut input, self.model.grown);
                    }
                    self.input(&input);
                }
            }
            Action::OtherTerminals => {
                let Some(outpost) = self.model.live.last().copied() else {
                    return;
                };
                for number in 10..26 {
                    let input = self.focus_input(outpost, number, None, false, 0);
                    self.focus(&input, outpost, None, 0);
                    let snapshot = self.snapshot(node_of(outpost, number));
                    self.input(&snapshot);
                }
                let input = self.focus_input(outpost, 7, None, false, 0);
                self.focus(&input, outpost, None, 0);
            }
            Action::Answer { which, outcome } => {
                let outcome = match outcome {
                    0 => tests::Outcome::Done,
                    1 => tests::Outcome::Gone,
                    _ => tests::Outcome::Failed,
                };
                let live = |outpost: &OutpostId| !self.model.ended.contains(outpost);
                let chosen = match which {
                    0 => self
                        .wire
                        .open
                        .iter()
                        .next()
                        .map(|(id, (outpost, asked))| (*id, *outpost, asked.clone())),
                    1 => self
                        .wire
                        .open
                        .iter()
                        .next_back()
                        .map(|(id, (outpost, asked))| (*id, *outpost, asked.clone())),
                    2 => self
                        .wire
                        .last_ended
                        .clone()
                        .filter(|(_, outpost, _)| live(outpost)),
                    3 => self.model.live.last().map(|outpost| {
                        (
                            QueryId(1_000_000),
                            *outpost,
                            tests::Asked::Text(verbatim_model::TextOp::ReadCaret),
                        )
                    }),
                    // From another live generation, or one never started.
                    _ => self
                        .wire
                        .open
                        .iter()
                        .next_back()
                        .map(|(id, (outpost, asked))| {
                            let other = self
                                .model
                                .live
                                .iter()
                                .copied()
                                .find(|other| other != outpost)
                                .unwrap_or(OutpostId(GENERATIONS + 10));
                            (*id, other, asked.clone())
                        }),
                };
                let Some((query_id, outpost, asked)) = chosen else {
                    return;
                };
                match which {
                    2 => self.reach("an answer to an ended query"),
                    4 => self.reach("an answer from elsewhere"),
                    _ => {}
                }
                let input = tests::answer(query_id, outpost, &asked, outcome);
                self.input(&input);
            }
            Action::Key(which) => {
                let input = match which {
                    0 => Input::CaretKey {
                        trace_id: trace,
                        key_seq: KeySeq::NONE_YET,
                        key: key(CaretMotion::NextCharacter),
                        pressed_at_ms: now / 1_000,
                    },
                    1 => Input::HeldKey {
                        trace_id: tests::HELD,
                        key_seq: KeySeq::NONE_YET,
                        key: key(CaretMotion::NextCharacter),
                        pressed_at_ms: now / 1_000,
                    },
                    2 => Input::HeldKeyDue {
                        trace_id: tests::HELD,
                    },
                    3 => Input::CharacterTyped {
                        trace_id: trace,
                        key_seq: KeySeq::NONE_YET,
                        text: "a".to_owned(),
                        at_ms: now / 1_000,
                    },
                    _ => Input::SpeechCancelled { at_ms: now / 1_000 },
                };
                let effects = self.input(&input);
                // Speech cut off asks nothing.
                if which == 4 {
                    assert_eq!(requests(&effects, None), [], "{:?}", self.trail);
                }
            }
            Action::Command(which) => {
                let input = command(match which {
                    0 => ReviewCommand::ReviewCurrentLine,
                    1 => ReviewCommand::Parent,
                    2 => ReviewCommand::ToFocus,
                    3 => ReviewCommand::SayAllFromCaret,
                    4 => ReviewCommand::Activate,
                    _ => ReviewCommand::ReviewPreviousLine,
                });
                self.input(&input);
            }
            Action::Foreground { window, tie } => {
                let window = WINDOWS[usize::from(window)];
                let observed_at_us = match (tie, self.model.latest) {
                    (true, Some(latest)) => latest
                        .observed_at_us
                        .max(self.model.front.map_or(0, |(_, observed)| observed)),
                    _ => now,
                };
                let change = ForegroundWindow {
                    window,
                    root_owner: window,
                    process: APPLICATION,
                    observed_at_us,
                    own: false,
                    thread: 1,
                    console: false,
                    integrity: None,
                    own_integrity: None,
                };
                self.foreground(change);
            }
            Action::OutpostEnded(which) => {
                let Some(outpost) = self.generation(which) else {
                    return;
                };
                self.outpost_ended(outpost, trace);
            }
            Action::NewGeneration => {
                if self.model.started < GENERATIONS {
                    self.model.started += 1;
                    self.model.live.insert(OutpostId(self.model.started));
                }
            }
            Action::Timer => {
                let Some(due_ms) = self.state.terminal_timer_due() else {
                    return;
                };
                self.model.now = self.model.now.max(due_ms * 1_000);
                self.reach("a terminal timer fired");
                self.input(&Input::TerminalTimer {
                    trace_id: trace,
                    at_ms: due_ms,
                });
                // The timer's due time moves past the time it fired at, or
                // the shell, which fires it whenever the due time it reads
                // is past, would fire it on every wake.
                assert!(
                    self.state
                        .terminal_timer_due()
                        .is_none_or(|due| due > due_ms),
                    "the timer is due again at its own time: {:?}",
                    self.trail
                );
            }
        }
        // The state round-trips through its serialisation.
        let json = serde_json::to_string(&self.state).expect("the state serializes");
        let read: SrState = serde_json::from_str(&json).expect("the state reads");
        assert_eq!(read, self.state, "the round trip: {:?}", self.trail);
    }

    /// A focus of node `number` of `outpost`, read with `earlier`, in the
    /// front's window or else the first, or with no window facts,
    /// observed at `observed_at_us`.
    fn focus_input(
        &self,
        outpost: OutpostId,
        number: u64,
        earlier: Option<EarlierElement>,
        windowed: bool,
        observed_at_us: u64,
    ) -> Input {
        let role = match number {
            5 => Role::EditableText,
            6 => Role::Button,
            _ => Role::Terminal,
        };
        let snapshot = tests::node(outpost, number, role, "Element");
        let text = (number == 5).then(|| FocusText::Caret {
            caret: tests::caret(100, 3),
            selected_text: None,
        });
        let top = self.model.front.map_or(WINDOWS[0], |(window, _)| window);
        Input::Event {
            trace_id: TraceId::from_raw(200 + self.trail.len() as u64),
            observed_at_us,
            source: APPLICATION,
            backend: Backend::Uia,
            window: windowed.then(|| facts(top)),
            event: NormalizedEvent::FocusChanged {
                node: snapshot,
                whole_window: false,
                ancestors: vec![tests::node(outpost, 1, Role::Window, "Window")],
                selected_child: None,
                text: text.map(Box::new),
                earlier,
                group: None,
            },
        }
    }

    /// The snapshot of terminal `node`, the model's next read.
    fn snapshot(&self, node: NodeId) -> Input {
        Input::Event {
            trace_id: TraceId::from_raw(300 + self.trail.len() as u64),
            observed_at_us: 0,
            source: APPLICATION,
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::TerminalSnapshot {
                node_id: node,
                snapshot: Box::new(tests::console_read(
                    self.model.reads + 1,
                    self.model.now / 1_000,
                )),
            },
        }
    }

    /// Gives `input`, a focus of `outpost`'s read with `earlier` and
    /// observed at `observed_at_us`, predicting its fate: stale, a
    /// duplicate, or taken.
    fn focus(
        &mut self,
        input: &Input,
        outpost: OutpostId,
        earlier: Option<EarlierElement>,
        observed_at_us: u64,
    ) {
        let Input::Event {
            window,
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } = input
        else {
            unreachable!("a focus");
        };
        let top = window.as_ref().map(|facts| facts.top_level);
        // Generated only attended: in the front, or with no window facts.
        assert!(
            top.is_none()
                || self
                    .model
                    .front
                    .is_none_or(|(window, _)| Some(window) == top),
            "a focus generated outside the front"
        );
        let stale = observed_at_us != 0
            && self.model.latest.is_some_and(|latest| {
                latest.outpost != outpost
                    && observed_at_us < latest.observed_at_us
                    && !(top.is_some() && top == latest.top)
            });
        let expected = if stale {
            self.reach("a stale focus");
            self.model.focus
        } else {
            if observed_at_us != 0 {
                self.model.latest = Some(Stamp {
                    outpost,
                    observed_at_us,
                    top,
                });
            }
            let duplicate = self.model.focus.is_some_and(|focus| {
                !focus.left
                    && focus.node == node.id
                    && earlier.map_or(focus.alive, |earlier| earlier == EarlierElement::Focused)
            });
            if duplicate {
                self.reach("a duplicate focus");
                self.model.focus
            } else {
                self.reach("a focus taken");
                // Generated in the front, a focus with window facts enters
                // its window.
                if top.is_some() {
                    self.model.entered = top;
                }
                Some(Focus {
                    node: node.id,
                    alive: true,
                    left: false,
                    top,
                })
            }
        };
        self.model.focus = expected;
        if matches!(
            earlier,
            Some(EarlierElement::NotFocused | EarlierElement::Gone)
        ) && self.model.models.iter().any(|(model, _)| *model == node.id)
        {
            self.reach("a focus's text forgotten");
        }
        self.input(input);
    }

    /// Gives the foreground change `change`, which leaves the focus when it
    /// is into another window after the newest focus stamp, and changes
    /// nothing else of the slice.
    fn foreground(&mut self, change: ForegroundWindow) {
        let entered = self.model.entered;
        if let Some(focus) = self.model.focus.as_mut()
            && let Some(top) = focus.top
            && self
                .model
                .latest
                .is_none_or(|latest| latest.observed_at_us < change.observed_at_us)
            && change.window != top
            && Some(change.window) != entered
            && change.root_owner != top
        {
            if !focus.left {
                *self.reached.entry("a focus left").or_default() += 1;
            }
            focus.left = true;
        }
        if self
            .model
            .latest
            .is_some_and(|latest| latest.observed_at_us == change.observed_at_us)
        {
            *self
                .reached
                .entry("a foreground change at the newest focus's stamp")
                .or_default() += 1;
        }
        self.model.front = Some((change.window, change.observed_at_us));
        let before = self.state.clone();
        let input = Input::Foreground(change);
        let effects = self.input(&input);
        // It speaks nothing, asks nothing, and changes nothing but the
        // foreground record and whether the focus was left: the record
        // holds the change as the hook's view, the front and attention,
        // keeps the entered window the model predicts, and keeps its
        // object and stamps.
        assert_eq!(
            effects,
            [],
            "a foreground change's effects: {:?}",
            self.trail
        );
        let record = &self.state.foreground;
        let was = &before.foreground;
        assert_eq!(
            (
                record.window(),
                record.front(),
                record.attention(),
                record.entered(),
            ),
            (
                Some(change),
                Some(Front {
                    window: change.window,
                    root_owner: change.root_owner,
                    observed_at_us: change.observed_at_us,
                }),
                // The hook's view of the window: in front, and nothing
                // else of it known.
                Some(&Attention {
                    source: change.process,
                    window: Some(WindowFacts {
                        top_level: change.window,
                        root_owner: change.root_owner,
                        topmost: false,
                        under_active_window: None,
                        in_foreground: true,
                        foreground_window: change.window,
                        class: String::new(),
                        application: String::new(),
                        alt_held: false,
                        foreground_is_shell: false,
                    }),
                }),
                self.model.entered,
            ),
            "a foreground change's record: {:?}",
            self.trail
        );
        assert_eq!(
            (
                record.object(),
                record.changed_at(),
                record.latest_focus(),
                record.applied_at()
            ),
            (
                was.object(),
                was.changed_at(),
                was.latest_focus(),
                was.applied_at()
            ),
            "a foreground change kept the record's object and stamps: {:?}",
            self.trail
        );
        let mut expected = before;
        // Each part of the record is checked above.
        expected.foreground = self.state.foreground.clone();
        if let (Some(focus), Some(model)) = (expected.focus.as_mut(), self.model.focus) {
            focus.left = model.left;
        }
        assert_eq!(
            expected, self.state,
            "a foreground change: {:?}",
            self.trail
        );
    }

    /// Ends generation `outpost`.
    fn outpost_ended(&mut self, outpost: OutpostId, trace: TraceId) {
        self.model.live.remove(&outpost);
        self.model.ended.insert(outpost);
        if let Some(focus) = self.model.focus.as_mut()
            && focus.node.outpost() == outpost
        {
            focus.alive = false;
            self.reached
                .entry("a focus dead with its outpost")
                .and_modify(|count| *count += 1)
                .or_insert(1);
        }
        self.reach("an outpost ended");
        let before = self.state.clone();
        let input = Input::OutpostEnded {
            trace_id: trace,
            outpost,
        };
        let effects = self.input(&input);
        let trail = &self.trail;
        // Nothing is asked of it, and nothing of it is left: its entries,
        // its models with their memory entries, and its focus's life, its
        // navigator, caret, say-all and start marker.
        assert!(
            requests(&effects, None)
                .iter()
                .all(|(_, node, _)| node.outpost() != outpost),
            "{trail:?}"
        );
        assert!(
            self.state
                .queries
                .entries()
                .all(|(_, open)| open.outpost != outpost),
            "{trail:?}"
        );
        assert!(
            self.state
                .terminals
                .iter()
                .all(|model| model.node.outpost() != outpost),
            "{trail:?}"
        );
        let state = &self.state;
        assert!(
            state
                .focus
                .as_ref()
                .is_none_or(|focus| focus.snapshot.id.outpost() != outpost || !focus.alive),
            "{trail:?}"
        );
        assert!(
            state
                .navigator
                .as_ref()
                .is_none_or(|navigator| navigator.object.id.outpost() != outpost),
            "{trail:?}"
        );
        assert!(
            state
                .caret
                .as_ref()
                .is_none_or(|caret| caret.node.outpost() != outpost),
            "{trail:?}"
        );
        assert!(
            state
                .say_all
                .as_ref()
                .is_none_or(|say_all| say_all.node.outpost() != outpost),
            "{trail:?}"
        );
        assert!(
            state
                .start_marker
                .is_none_or(|marker| marker.node().outpost() != outpost),
            "{trail:?}"
        );
        // Another outpost's entries end only by a withdrawal.
        let withdrawn: BTreeSet<QueryId> = effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Withdraw { query_id, .. } => Some(*query_id),
                _ => None,
            })
            .collect();
        for (query_id, open) in before.queries.entries() {
            if open.outpost != outpost && open.asked != Waited::Memory {
                assert!(
                    self.state.queries.entries().any(|(id, _)| id == query_id)
                        || withdrawn.contains(&query_id),
                    "{query_id:?} of another outpost ended: {trail:?}"
                );
            }
        }
    }

    /// Gives `input` and checks it: the table's invariants of the
    /// enumeration, then the slice's against the model. Its effects.
    #[expect(
        clippy::too_many_lines,
        reason = "one check for each part of the slice"
    )]
    fn input(&mut self, input: &Input) -> Vec<Effect> {
        let before = self.state.clone();
        let effects = reduce(&mut self.state, input);
        self.log.push(format!("{effects:?}"));
        self.wire.apply(input, &effects);
        let resumed = tests::check(
            &[],
            &Visit {
                before: &before,
                input,
                state: &self.state,
                effects: &effects,
                wire: &self.wire,
            },
        );
        if resumed {
            self.reach("the held keys resumed");
        }
        let trail = format!("{:?}, {input:?}", self.trail);
        let state = &self.state;
        let model = &mut self.model;
        // Every open entry but Core's own names a live generation, and no
        // effect asks an ended one.
        for (query_id, open) in state.queries.entries() {
            if open.asked != Waited::Memory {
                assert!(
                    !model.ended.contains(&open.outpost),
                    "{query_id:?} of an ended outpost: {trail}"
                );
            }
        }
        let asked = requests(&effects, None);
        for (query_id, node, _) in &asked {
            assert!(
                !model.ended.contains(&node.outpost()),
                "{query_id:?} asks an ended outpost: {trail}"
            );
        }
        // Ids only rise and an ended id never returns; each new entry's
        // trace is its input's, or the key's inside a key's effects, and an
        // older entry's trace is as it was.
        let old: BTreeMap<QueryId, Option<TraceId>> = before
            .queries
            .entries()
            .map(|(id, open)| (id, open.trace_id))
            .collect();
        assert!(state.queries.next() >= before.queries.next(), "{trail}");
        for (query_id, open) in state.queries.entries() {
            if let Some(trace) = old.get(&query_id) {
                assert_eq!(
                    open.trace_id, *trace,
                    "{query_id:?}'s trace changed: {trail}"
                );
                continue;
            }
            assert!(
                query_id.0 >= before.queries.next(),
                "{query_id:?} not new: {trail}"
            );
            assert!(
                !model.ended_ids.contains(&query_id),
                "{query_id:?} ended, back: {trail}"
            );
            if let Some((_, _, key)) = asked.iter().find(|(id, ..)| *id == query_id) {
                assert_eq!(
                    open.trace_id,
                    key.or_else(|| input.trace_id()),
                    "{query_id:?}'s trace: {trail}"
                );
            }
        }
        let mut ended = 0;
        for query_id in old.keys() {
            if !state.queries.entries().any(|(id, _)| id == *query_id) {
                model.ended_ids.insert(*query_id);
                ended += 1;
            }
        }
        // The held keys' awaited watch is none or names an open entry.
        if let Some(watch) = state.held_keys.awaiting_watch {
            assert!(
                state.queries.entries().any(|(id, _)| id == watch),
                "{trail}"
            );
        }
        // The held nodes name no ended generation.
        for outpost in state.held_nodes().keys() {
            assert!(
                !model.ended.contains(outpost),
                "a held node of an ended outpost: {trail}"
            );
        }
        // The focus's lifecycle.
        assert_eq!(
            state.focus.as_ref().map(|focus| Focus {
                node: focus.snapshot.id,
                alive: focus.alive,
                left: focus.left,
                top: focus.window.as_ref().map(|facts| facts.top_level),
            }),
            model.focus,
            "the focus: {trail}"
        );
        // The terminal models: at most 16, each generation new when made
        // and none of an ended outpost; one goes only when the oldest is
        // evicted for a new one, its node's text is forgotten or its
        // outpost ends; the others keep their order, one read moving to
        // the back.
        let models: Vec<(NodeId, u64)> = state
            .terminals
            .iter()
            .map(|terminal| (terminal.node, terminal.generation))
            .collect();
        assert!(models.len() <= 16, "{trail}");
        let made: Vec<(NodeId, u64)> = models
            .iter()
            .copied()
            .filter(|model_made| !model.models.contains(model_made))
            .collect();
        for (node, generation) in &made {
            assert!(
                model.newest_made.is_none_or(|newest| *generation > newest),
                "{trail}"
            );
            assert!(!model.ended.contains(&node.outpost()), "{trail}");
            model.newest_made = Some(*generation);
        }
        let forgotten = match input {
            Input::Event {
                event:
                    NormalizedEvent::FocusChanged {
                        node,
                        earlier: Some(EarlierElement::NotFocused | EarlierElement::Gone),
                        ..
                    },
                ..
            } => Some(node.id),
            _ => None,
        };
        let ended_outpost = match input {
            Input::OutpostEnded { outpost, .. } => Some(*outpost),
            _ => None,
        };
        let gone: Vec<(NodeId, u64)> = model
            .models
            .iter()
            .copied()
            .filter(|before_model| !models.contains(before_model))
            .collect();
        for (at, (node, _)) in gone.iter().enumerate() {
            if Some(*node) == forgotten || Some(node.outpost()) == ended_outpost {
                *self.reached.entry("a model dropped").or_default() += 1;
            } else {
                assert!(
                    !made.is_empty()
                        && model.models.len() == 16
                        && model.models.get(at) == Some(&(*node, gone[at].1)),
                    "a model gone but evicted: {trail}"
                );
                *self.reached.entry("a model evicted").or_default() += 1;
            }
        }
        let kept: Vec<(NodeId, u64)> = model
            .models
            .iter()
            .copied()
            .filter(|kept| models.contains(kept))
            .collect();
        let mut rest = models
            .iter()
            .copied()
            .filter(|each| !made.contains(each))
            .peekable();
        for each in &kept {
            if rest.peek() == Some(each) {
                rest.next();
            }
        }
        let moved: Vec<(NodeId, u64)> = rest.collect();
        let tail_starts = models.len() - moved.len() - made.len();
        assert!(
            models[tail_starts..]
                .iter()
                .all(|each| moved.contains(each) || made.contains(each)),
            "the models' order: {:?} after {:?}: {trail}",
            models,
            model.models
        );
        if !made.is_empty() {
            *self.reached.entry("a model made").or_default() += 1;
        }
        model.models = models;
        // Every memory entry is a live model's.
        for (_, open) in state.queries.entries() {
            if open.asked == Waited::Memory
                && let Then::Text(pending) = &open.then
            {
                assert!(
                    model.models.iter().any(|(node, _)| *node == pending.node),
                    "{trail}"
                );
            }
        }
        if ended > 0 {
            *self.reached.entry("a query ended").or_default() += 1;
        }
        if before
            .queries
            .entries()
            .any(|(_, open)| open.asked == Waited::Memory)
            && !matches!(input, Input::OutpostEnded { .. })
            && before
                .queries
                .entries()
                .filter(|(_, open)| open.asked == Waited::Memory)
                .any(|(id, _)| !state.queries.entries().any(|(now, _)| now == id))
        {
            *self
                .reached
                .entry("a request answered from memory")
                .or_default() += 1;
        }
        if state.held_keys.awaiting_watch.is_some() {
            *self
                .reached
                .entry("the held keys' watch awaited")
                .or_default() += 1;
        }
        // An answer: to an entry nothing waits for, or to a model no
        // longer live, runs nothing.
        if let Input::Answered {
            query_id, outpost, ..
        } = input
            && let Some((_, open)) = before.queries.entries().find(|(id, _)| id == query_id)
            && open.outpost == *outpost
            && open.asked != Waited::Memory
        {
            match &open.then {
                Then::Nothing => {
                    *self
                        .reached
                        .entry("an answer to a superseded query")
                        .or_default() += 1;
                }
                Then::TerminalModel {
                    node, generation, ..
                } if !before
                    .terminals
                    .iter()
                    .any(|model| model.node == *node && model.generation == *generation) =>
                {
                    *self
                        .reached
                        .entry("an answer to a model no longer live")
                        .or_default() += 1;
                    if before.terminals.iter().any(|model| model.node == *node) {
                        *self
                            .reached
                            .entry("an answer to a replaced model")
                            .or_default() += 1;
                    }
                    let mut expected = before.clone();
                    let _ = expected.queries.end(*query_id);
                    if expected.held_keys.awaiting_watch != Some(*query_id) {
                        assert_eq!(effects, [], "{trail}");
                        assert_eq!(expected, self.state, "{trail}");
                    }
                }
                _ => {}
            }
        }
        effects
    }

    /// Ends the case: every request still open answered, oldest first,
    /// until none is; then nothing the idle barrier waits for is open, and
    /// every entry left is Core's own, of a live model.
    fn drain(&mut self) {
        for _ in 0..200 {
            let Some((query_id, (outpost, asked))) = self
                .wire
                .open
                .iter()
                .next()
                .map(|(id, open)| (*id, open.clone()))
            else {
                break;
            };
            let input = tests::answer(query_id, outpost, &asked, tests::Outcome::Done);
            self.input(&input);
        }
        assert!(self.wire.open.is_empty(), "requests left: {:?}", self.trail);
        assert_eq!(self.state.outstanding_queries(), 0, "{:?}", self.trail);
        for (query_id, open) in self.state.queries.entries() {
            assert_eq!(
                open.asked,
                Waited::Memory,
                "{query_id:?} left: {:?}",
                self.trail
            );
        }
    }
}

/// Runs the generated cases `config` draws, each twice from a new state,
/// shrinking a failure to its smallest case; what the cases reached.
///
/// # Panics
///
/// On a failing case, naming the smallest.
pub(super) fn run_generated(config: Config) -> BTreeMap<&'static str, u32> {
    let reached = RefCell::new(BTreeMap::new());
    let mut runner = TestRunner::new(config);
    let strategy = proptest::collection::vec(action(), 1..40);
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
    if let Err(error) = result {
        panic!("{error}");
    }
    reached.into_inner()
}

/// A focus of the terminal of the oldest generation, with what its reader
/// read of the element it held (0 nothing, 2 not focused).
const fn terminal(earlier: u8) -> Action {
    Action::Focus {
        generation: 0,
        node: 2,
        earlier,
        windowed: false,
        stamp: 0,
    }
}

/// The cases kept beside the generated ones, each reaching what the
/// generated cases reach too seldom: a model's request answered after its
/// model was replaced by a newer one of the same terminal, a foreground
/// change observed when the newest focus was, and the terminal's timer
/// fired after a row changed often enough for the rules to hold it.
fn fixed() -> Vec<(Vec<Action>, &'static str)> {
    vec![
        (
            vec![
                terminal(0),
                Action::Snapshot { growing: false },
                Action::Command(0),
                terminal(2),
                Action::Snapshot { growing: false },
                Action::Answer {
                    which: 0,
                    outcome: 0,
                },
            ],
            "an answer to a replaced model",
        ),
        (
            vec![
                Action::Focus {
                    generation: 0,
                    node: 0,
                    earlier: 0,
                    windowed: true,
                    stamp: 0,
                },
                Action::Foreground {
                    window: 1,
                    tie: true,
                },
            ],
            "a foreground change at the newest focus's stamp",
        ),
        (
            vec![
                terminal(0),
                Action::Snapshot { growing: true },
                Action::Snapshot { growing: true },
                Action::Snapshot { growing: true },
                Action::Timer,
            ],
            "a terminal timer fired",
        ),
    ]
}

/// Every fixed case keeps the slice's rules, gives the same effects when
/// run twice, and reaches what it is kept for.
#[test]
fn fixed_cases_keep_the_reducers_rules() {
    for (case, state) in fixed() {
        let first = Run::case(&case);
        assert_eq!(first.log, Run::case(&case).log, "{case:?}");
        assert!(first.reached.contains_key(state), "{state}: {case:?}");
    }
}

/// The gate's seed and case count.
const SEED: u64 = 20_261_103;
const CASES: u32 = 300;

/// Generated cases of focuses from live outpost generations, terminal
/// snapshots, answers of every kind (from the outpost asked, another, to an
/// ended query, to none), keys and held keys, commands, foreground changes,
/// outposts' ends and new generations keep the query table's invariants and
/// the slice's against the model after every input, round-trip through the
/// state's serialisation, end with nothing waited for left open, give the
/// same effects when run twice, and reach every listed state.
#[test]
fn generated_cases_keep_the_reducers_rules() {
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
