//! Core's reducer slice's generated test (`books/dev/src/reference/crates/verbatim-core.md`): lists of actions, each made valid when it is applied
//! by resolving its target against the model, given to [`reduce`] in
//! order. After every input the query table's invariants of the
//! enumeration beside it (`queries/tests.rs`) are checked against the
//! wire's view of the requests, and the slice's own against a reference
//! model of sets and lists: the outpost generations live and ended, the
//! ids given and ended, and the terminal models' generations in their
//! order of reading. A foreground change changes only its record and
//! whether the focus was left; which focus is stale, a duplicate or left is
//! `live_reread/generated.rs`'s model and the reducer's own tests' examples.
//! The terminal's quiet-period timer, fired at the due time the
//! state gives, is due later than that time or not at all. Each case ends
//! by answering every request still open, after which no field of the
//! state names an ended generation but those [`ENDED_MAY_REMAIN`] lists
//! (`super::ended`), and its state round-trips through its serialisation
//! after every action.
//!
//! The model leaves out words, carets, text, chains, say-all's buffers and
//! the flood policy (Dickson, 2026-11-02), and held keys, whose world and
//! reference model are the joint test's
//! (`crates/verbatim-outpost-logic/src/tests/caret_lease.rs`). A focus is
//! generated only in the front or with no window facts, from the one
//! application every generation reads, never a whole-window focus or a
//! group's.
//!
//! Review presses, among held keys and focus reports, have their own world
//! and reference model below ([`PressRun`]), written from
//! `docs/parity/review.md` and the decisions of `docs/design/m42-parity.md`.
//! A key in front of a press is opaque to that model: what a key does after
//! its turn starts is the joint test's. Recorded limit: a press with
//! something behind it that gives way at its 150 ms says nothing, so what
//! its answer would have said is lost, where NVDA would answer late, in
//! order.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestError, TestRunner};
use verbatim_model::protocol::{QueryOutcome, QueryResult};
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
    /// A caret key (0), a character typed (1) or speech cut off (2).
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

impl Action {
    /// The action as the Rust expression that makes it, as a fixed case
    /// lists it.
    fn code(self) -> String {
        format!("Action::{self:?}")
    }
}

/// The report of a failing case: `actions` as a line ready to paste into
/// the fixed cases, and the command that runs again the run that found it,
/// `again`.
fn found_report(actions: &[String], again: &str) -> String {
    format!(
        "the smallest failing case, as a line for the fixed cases in crates/verbatim-core/src/queries/generated.rs:\n        vec![{}],\n\
         the run that found it again: {again}",
        actions.join(", ")
    )
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
        6 => (0..3u8).prop_map(Action::Key),
        6 => (0..6u8).prop_map(Action::Command),
        3 => (0..2u8, prop::bool::weighted(0.3))
            .prop_map(|(window, tie)| Action::Foreground { window, tie }),
        2 => (0..2u8).prop_map(Action::OutpostEnded),
        2 => Just(Action::NewGeneration),
        3 => Just(Action::Timer),
    ]
}

/// The reference model.
#[derive(Clone, Debug, Default)]
struct Model {
    live: BTreeSet<OutpostId>,
    ended: BTreeSet<OutpostId>,
    started: u64,
    /// The front: its window and when it was observed.
    front: Option<(WindowHandle, u64)>,
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
const LISTED: [&str; 13] = [
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
    "an answer to a model no longer live",
    "a request answered from memory",
    "an ended generation checked",
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

/// The fields of Core's state that may still name an ended outpost
/// generation once everything asked of it is answered, each by its path in
/// the state's serialisation (`[]` an element of a list or tuple, `{}` a
/// map's key or an entry under a key that is not text), with why it is
/// kept. A field found that is not listed fails the case.
const ENDED_MAY_REMAIN: &[(&str, &str)] = &[
    (
        "focus.object.snapshot.id.outpost",
        "the focus keeps its copied data with its ids dead, so a replacement's \
         report of the same focus is taken silently; the next focus taken replaces it",
    ),
    (
        "focus.object.ancestors[].id.outpost",
        "the dead focus's ancestors, kept with it for the same comparison",
    ),
    (
        "focus.object.spoken.ancestors[][].outpost",
        "what Core last spoke of the dead focus's line, against which the next \
         focus's line says only what changed; the next focus taken replaces it",
    ),
    (
        "foreground.id.outpost",
        "the foreground window's object, the same object to every generation by \
         its window handle; the next foreground change replaces it",
    ),
    (
        "latest_focus.outpost",
        "the newest focus observation's stamp, which orders later focus events by \
         time; its generation only exempts that generation's own reports, and an \
         ended one sends none",
    ),
    (
        "focus_stamps[].outpost",
        "the stale rule's stamps, each a report Core accepted, which order later \
         focus events by time as the newest does; let go once two newer ordinary \
         stamps of other generations cover them",
    ),
];

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
            Effect::Activate { query_id, node_id } | Effect::ReadNode { query_id, node_id } => {
                vec![(*query_id, *node_id, key)]
            }
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

    /// Runs `actions`, then drains, and checks that nothing left names an
    /// ended generation.
    fn case(actions: &[Action]) -> Self {
        let mut run = Self::new();
        for &action in actions {
            run.act(action);
        }
        run.drain();
        run.check_ended_owners();
        run
    }

    /// Once the case is quiet, no field of the state names an ended
    /// outpost generation but those [`ENDED_MAY_REMAIN`] lists: state
    /// about an owner goes with its ending.
    fn check_ended_owners(&mut self) {
        if self.model.ended.is_empty() {
            return;
        }
        let found = super::ended::naming(&self.state, &self.model.ended);
        let left: Vec<&String> = found
            .iter()
            .filter(|path| !ENDED_MAY_REMAIN.iter().any(|(kept, _)| kept == path))
            .collect();
        assert!(
            left.is_empty(),
            "fields naming an ended generation: {left:?}: {:?}",
            self.trail
        );
        self.reach("an ended generation checked");
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
                self.focus(&input, earlier);
            }
            Action::Snapshot { growing } => {
                if let Some(node) = self
                    .state
                    .focus
                    .object()
                    .map(|focus| focus.snapshot.id)
                    .filter(|node| node.number() == 7)
                {
                    self.model.reads += 1;
                    let mut input = self.snapshot(node);
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
                    self.focus(&input, None);
                    let snapshot = self.snapshot(node_of(outpost, number));
                    self.input(&snapshot);
                }
                let input = self.focus_input(outpost, 7, None, false, 0);
                self.focus(&input, None);
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
                            tests::Asked::Text(tests::key_op()),
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
                    1 => Input::CharacterTyped {
                        trace_id: trace,
                        key_seq: KeySeq::NONE_YET,
                        text: "a".to_owned(),
                        at_ms: now / 1_000,
                    },
                    _ => Input::SpeechCancelled { at_us: now },
                };
                let effects = self.input(&input);
                // Speech cut off asks nothing.
                if which == 2 {
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
                let observed_at_us = match (tie, self.state.foreground.latest_focus()) {
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
            facts: None,
            caret: tests::caret(100, 3),
            selected_text: None,
            changes: verbatim_model::CaretChanges::default(),
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
                answers: None,
                first_cached_ancestor: None,
            },
            purpose: None,
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
            purpose: None,
        }
    }

    /// Gives `input`, a focus read with `earlier`, generated in the front
    /// or with no window facts.
    fn focus(&mut self, input: &Input, earlier: Option<EarlierElement>) {
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
        if matches!(
            earlier,
            Some(EarlierElement::NotFocused | EarlierElement::Gone)
        ) && self.model.models.iter().any(|(model, _)| *model == node.id)
        {
            self.reach("a focus's text forgotten");
        }
        self.input(input);
    }

    /// Gives the foreground change `change`, which changes nothing of the
    /// slice but its record and whether the focus was left.
    fn foreground(&mut self, change: ForegroundWindow) {
        if self
            .state
            .foreground
            .latest_focus()
            .is_some_and(|latest| latest.observed_at_us == change.observed_at_us)
        {
            self.reach("a foreground change at the newest focus's stamp");
        }
        self.model.front = Some((change.window, change.observed_at_us));
        let before = self.state.clone();
        let input = Input::Foreground(change);
        let effects = self.input(&input);
        // It speaks nothing, asks nothing, and changes nothing but the
        // foreground record and whether the focus was left: the record
        // holds the change as the hook's view, the front and attention, and
        // keeps the entered window, its object and its stamps.
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
                was.entered(),
            ),
            "a foreground change's record: {:?}",
            self.trail
        );
        assert_eq!(
            (record.object(), record.changed_at(), record.latest_focus()),
            (was.object(), was.changed_at(), was.latest_focus()),
            "a foreground change kept the record's object and stamps: {:?}",
            self.trail
        );
        let mut expected = before;
        // Each part of the record is checked above.
        expected.foreground = self.state.foreground.clone();
        // The change applies to the last queued focus, its left mark.
        expected.focus.take_changes_of(&self.state.focus);
        assert_eq!(
            expected, self.state,
            "a foreground change: {:?}",
            self.trail
        );
    }

    /// Ends generation `outpost`, returning what that did.
    fn outpost_ended(&mut self, outpost: OutpostId, trace: TraceId) -> Vec<Effect> {
        self.model.live.remove(&outpost);
        self.model.ended.insert(outpost);
        if self
            .state
            .focus
            .object()
            .is_some_and(|focus| focus.alive && focus.snapshot.id.outpost() == outpost)
        {
            self.reach("a focus dead with its outpost");
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
                .object()
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
                .focus
                .object()
                .filter(|focus| focus.caret.is_some())
                .is_none_or(|focus| focus.snapshot.id.outpost() != outpost),
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
        effects
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
        tests::check(
            &[],
            &Visit {
                before: &before,
                input,
                state: &self.state,
                effects: &effects,
                wire: &self.wire,
            },
        );
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
        // The held nodes name no ended generation.
        for outpost in state.held_nodes().keys() {
            assert!(
                !model.ended.contains(outpost),
                "a held node of an ended outpost: {trail}"
            );
        }
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
            if open.asked != Waited::Memory {
                continue;
            }
            let node = match &open.then {
                Then::Text(pending) => pending.node,
                // A review read carries no node: the press it serves, the
                // current key, has it.
                Then::Review(_) => state
                    .key_line_node()
                    .expect("a review read's press is the current key"),
                _ => continue,
            };
            assert!(
                model.models.iter().any(|(each, _)| *each == node),
                "{trail}"
            );
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
                    assert_eq!(effects, [], "{trail}");
                    assert_eq!(expected, self.state, "{trail}");
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
pub(super) fn run_generated(config: Config, again: &str) -> BTreeMap<&'static str, u32> {
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
    match result {
        Ok(()) => reached.into_inner(),
        Err(TestError::Fail(reason, actions)) => {
            let actions: Vec<String> = actions.iter().map(|action| action.code()).collect();
            panic!("{reason}\n{}", found_report(&actions, again))
        }
        Err(error) => panic!("{error}"),
    }
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
/// ended query, to none), keys, commands, foreground changes,
/// outposts' ends and new generations keep the query table's invariants and
/// the slice's against the model after every input, round-trip through the
/// state's serialisation, end with nothing waited for left open, give the
/// same effects when run twice, and reach every listed state.
#[test]
fn generated_cases_keep_the_reducers_rules() {
    let reached = run_generated(
        super::explore::config(SEED, CASES),
        "cargo test -p verbatim-core --lib -- --exact queries::generated::generated_cases_keep_the_reducers_rules",
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

// Review presses among keys and focus reports: their own world and reference
// model, written from `docs/parity/review.md` and the decisions of
// `docs/design/m42-parity.md`, reusing [`Run`] for the table's invariants.

/// The review commands a case presses.
const PRESS_COMMANDS: [ReviewCommand; 6] = [
    ReviewCommand::ReviewCurrentLine,
    ReviewCommand::ReviewNextLine,
    ReviewCommand::ReportObject,
    ReviewCommand::Parent,
    ReviewCommand::NextSibling,
    ReviewCommand::ToFocus,
];

/// The states the press world's gate must reach.
const LISTED_PRESS: [&str; 13] = [
    "a press ran behind a key",
    "a press armed by what joined it",
    "a press armed as it started",
    "a press gave way at its bound",
    "a press ended by a focus",
    "a press's outpost ended",
    "a press answered gone",
    "a press unanswered",
    "a press with a follow-up read",
    "a waiting press ran on a moved navigator",
    "an answer to a press that gave way",
    "a key ran behind a press",
    "exit passed a held key",
];

/// One step of a press case.
#[derive(Clone, Copy, Debug)]
enum PressAction {
    /// A focus report, observed now, on the text box (0) or the button (1)
    /// of the newest live generation.
    Focus(u8),
    /// The review command at this place in [`PRESS_COMMANDS`] is pressed.
    Press(u8),
    /// A caret key the hook held back.
    Held,
    /// A caret key the hook passed, while a press is current.
    Passed,
    /// A character typed, while a press is current.
    Typed,
    /// The oldest (0) or newest (1) open read answered: done (0), gone (1)
    /// or unanswered (2).
    Answer { which: u8, outcome: u8 },
    /// The line's one deadline, when one is set, passes.
    Deadline,
    /// The newest live generation ends.
    OutpostEnded,
    /// A new generation starts, when none is live.
    NewGeneration,
    /// Verbatim exits; the case's later actions do nothing.
    Exit,
}

impl PressAction {
    /// The action as the Rust expression that makes it.
    fn code(self) -> String {
        format!("PressAction::{self:?}")
    }
}

fn press_action() -> impl Strategy<Value = PressAction> {
    use proptest::prelude::*;
    prop_oneof![
        4 => (0..2u8).prop_map(PressAction::Focus),
        9 => (0..6u8).prop_map(PressAction::Press),
        3 => Just(PressAction::Held),
        1 => Just(PressAction::Passed),
        1 => Just(PressAction::Typed),
        8 => (0..2u8, 0..3u8)
            .prop_map(|(which, outcome)| PressAction::Answer { which, outcome }),
        3 => Just(PressAction::Deadline),
        1 => Just(PressAction::OutpostEnded),
        2 => Just(PressAction::NewGeneration),
        1 => Just(PressAction::Exit),
    ]
}

/// An entry of the model's line, as the world pressed it.
#[derive(Clone, Copy, Debug)]
enum Entry {
    /// A review command, with the navigator when it was pressed.
    Press {
        trace: TraceId,
        command: ReviewCommand,
        navigator: Option<NodeId>,
    },
    Held(TraceId),
    Passed(TraceId),
    Typed(TraceId),
}

impl Entry {
    fn trace(self) -> TraceId {
        match self {
            Self::Press { trace, .. }
            | Self::Held(trace)
            | Self::Passed(trace)
            | Self::Typed(trace) => trace,
        }
    }
}

/// The press whose read is running.
#[derive(Clone, Copy, Debug)]
struct CurrentPress {
    trace: TraceId,
    command: ReviewCommand,
    node: NodeId,
    query: QueryId,
}

/// The reference model of the line: what was pressed in order, the press
/// reading now, and the navigator and focus as the inputs left them, by the
/// rules of `docs/parity/review.md`: one ordered line (NVDA's scripts are
/// one queue); only a focus, the to-focus command and a navigation move the
/// navigator; a press acts on the navigator as it is when its turn comes.
#[derive(Debug, Default)]
struct LineModel {
    /// Pressed and not yet started, in press order.
    queue: VecDeque<Entry>,
    current: Option<CurrentPress>,
    navigator: Option<NodeId>,
    focus: Option<NodeId>,
    /// The reads of presses that gave way: their answers run nothing.
    given_way: BTreeSet<QueryId>,
    /// Held keys not yet passed on.
    unpassed: BTreeSet<TraceId>,
    /// Every press, and those that read, with how many steps each spoke in.
    presses: BTreeSet<TraceId>,
    readers: BTreeMap<TraceId, u32>,
    /// A key that has started and may still hold what is behind it: what it
    /// does next is outside this model, so a later entry starting is the
    /// only evidence it has left the line.
    key_ahead: bool,
    exited: bool,
    /// Whether a to-focus press ran in this step: it sets the navigator afresh.
    to_focus_ran: bool,
}

/// What a step did to the model's line.
#[derive(Clone, Copy)]
enum Move {
    Arrive(Entry),
    Focus(NodeId),
    Answer(QueryId, tests::Outcome),
    Deadline(TraceId),
    OutpostEnded(OutpostId),
    Exit,
}

/// The kind of a read a press asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadKind {
    Navigation,
    Node,
    Text,
}

/// A read one step asked.
#[derive(Clone, Copy, Debug)]
struct Read {
    query: QueryId,
    node: NodeId,
    trace: TraceId,
    kind: ReadKind,
}

/// What a step did to the line's one deadline.
#[derive(Clone, Copy)]
enum Bound {
    Set(TraceId, u64),
    Cleared,
}

/// What a step's effects show, each under the trace it runs under.
#[derive(Default)]
struct Seen {
    /// Every trace an effect ran under, in effect order.
    order: Vec<TraceId>,
    reads: Vec<Read>,
    spoke: Vec<TraceId>,
    passed: Vec<TraceId>,
    bounds: Vec<(TraceId, u64)>,
    withdrawn: Vec<QueryId>,
    /// The line's one deadline, as the shell would leave it, when the step
    /// set or cleared it.
    deadline: Option<Bound>,
}

impl Seen {
    fn of(effects: &[Effect], trace: Option<TraceId>) -> Self {
        let mut seen = Self::default();
        seen.take(effects, trace);
        seen
    }

    fn read(&mut self, query: QueryId, node: NodeId, trace: Option<TraceId>, kind: ReadKind) {
        if let Some(trace) = trace {
            self.order.push(trace);
            self.reads.push(Read {
                query,
                node,
                trace,
                kind,
            });
        }
    }

    fn take(&mut self, effects: &[Effect], trace: Option<TraceId>) {
        for effect in effects {
            match effect {
                Effect::ForKey(key, inner) => {
                    self.order.push(*key);
                    self.take(std::slice::from_ref(&**inner), Some(*key));
                }
                Effect::Fetch(query) => {
                    self.read(query.query_id, query.node_id, trace, ReadKind::Navigation);
                }
                Effect::ReadNode { query_id, node_id } => {
                    self.read(*query_id, *node_id, trace, ReadKind::Node);
                }
                Effect::Text(request) => {
                    self.read(request.query_id, request.node_id, trace, ReadKind::Text);
                }
                Effect::Speak(utterance, _) => {
                    self.order.push(utterance.trace_id);
                    self.spoke.push(utterance.trace_id);
                }
                Effect::PassHeldKey(key) => {
                    self.order.push(*key);
                    self.passed.push(*key);
                }
                Effect::BoundHeldKey(Some((key, ms))) => {
                    self.order.push(*key);
                    self.bounds.push((*key, *ms));
                    self.deadline = Some(Bound::Set(*key, *ms));
                }
                Effect::BoundHeldKey(None) => self.deadline = Some(Bound::Cleared),
                Effect::Withdraw { query_id, .. } => self.withdrawn.push(*query_id),
                _ => {}
            }
        }
    }

    fn spoke(&self, trace: TraceId) -> bool {
        self.spoke.contains(&trace)
    }

    fn reads_of(&self, trace: TraceId) -> Vec<Read> {
        self.reads
            .iter()
            .copied()
            .filter(|read| read.trace == trace)
            .collect()
    }
}

/// A press case: the reducer's slice ([`Run`]) with the line's model beside
/// it.
struct PressRun {
    run: Run,
    line: LineModel,
    trail: Vec<PressAction>,
    /// Who asked each read and of which node, as the wire knows it.
    asked: BTreeMap<QueryId, (TraceId, NodeId)>,
    /// The line's one deadline as the shell holds it.
    deadline: Option<(TraceId, u64)>,
}

/// The review position a navigator holds: its object, its offset in flat
/// text, and its place in the object's text once one has been read.
/// `ReviewText::Unknown`, `Flat` and `Caret` hold no place of their own: a
/// press behind one that gave way may find out which it is when it starts.
fn position(
    navigator: &crate::state::Navigator,
) -> (NodeId, usize, Option<crate::state::ReviewText>) {
    let text = matches!(
        navigator.text,
        crate::state::ReviewText::At(_) | crate::state::ReviewText::Point(_)
    )
    .then(|| navigator.text.clone());
    (navigator.object.id, navigator.review_offset, text)
}

/// The role of a node the press world names.
fn press_role(number: u64) -> Role {
    match number {
        5 => Role::EditableText,
        6 => Role::Button,
        _ => Role::Window,
    }
}

impl PressRun {
    fn new() -> Self {
        Self {
            run: Run::new(),
            line: LineModel::default(),
            trail: Vec::new(),
            asked: BTreeMap::new(),
            deadline: None,
        }
    }

    fn case(actions: &[PressAction]) -> Self {
        let mut run = Self::new();
        for &action in actions {
            run.act(action);
        }
        run.drain();
        run.run.check_ended_owners();
        run
    }

    #[expect(clippy::too_many_lines, reason = "one arm for each action")]
    fn act(&mut self, action: PressAction) {
        self.trail.push(action);
        if self.line.exited {
            return;
        }
        self.run.model.now += 10_000;
        let now = self.run.model.now;
        let trace = TraceId::from_raw(1_000 + self.trail.len() as u64);
        let key = CaretKey {
            motion: CaretMotion::NextCharacter,
            select: false,
        };
        let pressing = self.line.current.is_some();
        match action {
            PressAction::Focus(which) => {
                let Some(outpost) = self.run.model.live.last().copied() else {
                    return;
                };
                let number = 5 + u64::from(which);
                let node = node_of(outpost, number);
                // A repeat of the focus in place is a duplicate, the rules
                // of `live_reread/generated.rs`; this world makes none.
                if self.line.focus == Some(node) {
                    return;
                }
                let input = self.run.focus_input(outpost, number, None, false, now);
                self.step(&input, Move::Focus(node));
            }
            PressAction::Press(which) => {
                let command = PRESS_COMMANDS[usize::from(which)];
                // The to-focus command needs a focus; one whose outpost has
                // ended is outside this model's rules.
                if command == ReviewCommand::ToFocus && self.line.focus.is_none() {
                    return;
                }
                let input = Input::Command {
                    trace_id: trace,
                    key_seq: KeySeq::NONE_YET,
                    command,
                    pressed_at_ms: now / 1_000,
                    repeat: 0,
                };
                let navigator = self.line.navigator;
                let entry = Entry::Press {
                    trace,
                    command,
                    navigator,
                };
                self.step(&input, Move::Arrive(entry));
            }
            PressAction::Held => {
                let input = Input::HeldKey {
                    trace_id: trace,
                    key_seq: KeySeq::NONE_YET,
                    key,
                    pressed_at_ms: now / 1_000,
                };
                self.step(&input, Move::Arrive(Entry::Held(trace)));
            }
            PressAction::Passed if pressing => {
                let input = Input::CaretKey {
                    trace_id: trace,
                    key_seq: KeySeq::NONE_YET,
                    key,
                    pressed_at_ms: now / 1_000,
                };
                self.step(&input, Move::Arrive(Entry::Passed(trace)));
            }
            PressAction::Typed if pressing => {
                let input = Input::CharacterTyped {
                    trace_id: trace,
                    key_seq: KeySeq::NONE_YET,
                    text: "a".to_owned(),
                    at_ms: now / 1_000,
                };
                self.step(&input, Move::Arrive(Entry::Typed(trace)));
            }
            PressAction::Passed | PressAction::Typed => {}
            PressAction::Answer { which, outcome } => {
                let open = if which == 0 {
                    self.run.wire.open.iter().next()
                } else {
                    self.run.wire.open.iter().next_back()
                };
                let Some((query_id, (outpost, asked))) = open.map(|(id, open)| (*id, open.clone()))
                else {
                    return;
                };
                let outcome = match outcome {
                    0 => tests::Outcome::Done,
                    1 => tests::Outcome::Gone,
                    _ => tests::Outcome::Failed,
                };
                self.answer(query_id, outpost, &asked, outcome);
            }
            PressAction::Deadline => self.deadline_passes(),
            PressAction::OutpostEnded => {
                let Some(outpost) = self.run.model.live.last().copied() else {
                    return;
                };
                // A to-focus waiting needs its focus alive when its turn
                // comes (see the press above).
                if self.line.queue.iter().any(|entry| {
                    matches!(
                        entry,
                        Entry::Press {
                            command: ReviewCommand::ToFocus,
                            ..
                        }
                    )
                }) {
                    return;
                }
                let before = self.run.state.clone();
                let effects = self.run.outpost_ended(outpost, trace);
                self.observe(Move::OutpostEnded(outpost), &before, &effects, Some(trace));
            }
            PressAction::NewGeneration => {
                if self.run.model.live.is_empty() && self.run.model.started < GENERATIONS {
                    self.run.model.started += 1;
                    self.run
                        .model
                        .live
                        .insert(OutpostId(self.run.model.started));
                }
            }
            PressAction::Exit => {
                let input = Input::Exiting { replaced: false };
                self.step(&input, Move::Exit);
                self.line.exited = true;
            }
        }
    }

    /// Answers `query_id` as the outpost does: under the trace it was asked
    /// under, a live read of a node with that node.
    fn answer(
        &mut self,
        query_id: QueryId,
        outpost: OutpostId,
        asked: &tests::Asked,
        outcome: tests::Outcome,
    ) {
        let (trace, node) = self.asked[&query_id];
        let mut input = tests::answer(query_id, outpost, asked, outcome);
        if let Input::Answered {
            trace_id,
            outcome: answered,
            ..
        } = &mut input
        {
            *trace_id = trace;
            if matches!(asked, tests::Asked::ReadNode) && matches!(outcome, tests::Outcome::Done) {
                *answered = QueryOutcome::Done(QueryResult::Node(tests::node(
                    outpost,
                    node.number(),
                    press_role(node.number()),
                    "Element",
                )));
            }
        }
        self.step(&input, Move::Answer(query_id, outcome));
    }

    /// The line's one deadline passes, if one is set.
    fn deadline_passes(&mut self) {
        let Some((trace_id, ms)) = self.deadline.take() else {
            return;
        };
        self.run.model.now += ms * 1_000;
        let input = Input::HeldKeyDue {
            trace_id,
            at_us: self.run.model.now,
        };
        self.step(&input, Move::Deadline(trace_id));
    }

    /// Gives `input` and checks it against the line's model.
    fn step(&mut self, input: &Input, change: Move) {
        let before = self.run.state.clone();
        let effects = self.run.input(input);
        self.observe(change, &before, &effects, input.trace_id());
    }

    /// Takes `change` into the model and compares it with what the step
    /// did: `effects`, and the navigator in the state.
    #[expect(clippy::too_many_lines, reason = "one rule for each kind of step")]
    fn observe(
        &mut self,
        change: Move,
        before: &SrState,
        effects: &[Effect],
        trace: Option<TraceId>,
    ) {
        let seen = Seen::of(effects, trace);
        let trail = format!("{:?}", self.trail);
        for read in &seen.reads {
            self.asked.insert(read.query, (read.trace, read.node));
        }
        match seen.deadline {
            Some(Bound::Set(key, ms)) => self.deadline = Some((key, ms)),
            Some(Bound::Cleared) => self.deadline = None,
            None => {}
        }
        let navigator_was = self.line.navigator;
        self.line.to_focus_ran = false;
        let mut armed: Vec<TraceId> = Vec::new();
        let mut ended = false;
        let mut position_kept = false;
        let mut speaker = None;
        let mut must_start = false;
        let mut exiting = false;
        match change {
            Move::Arrive(entry) => {
                if let Entry::Held(trace) = entry {
                    self.line.unpassed.insert(trace);
                }
                if let Entry::Press { trace, .. } = entry {
                    self.line.presses.insert(trace);
                }
                if let Some(current) = self.line.current
                    && self.line.queue.is_empty()
                {
                    armed.push(current.trace);
                    self.run.reach("a press armed by what joined it");
                }
                must_start = self.line.current.is_none()
                    && !self.line.key_ahead
                    && self.line.queue.is_empty();
                self.line.queue.push_back(entry);
            }
            Move::Focus(node) => {
                self.line.focus = Some(node);
                if self.line.navigator != Some(node) {
                    self.line.navigator = Some(node);
                    if let Some(current) = self.line.current.take() {
                        assert!(
                            seen.withdrawn.contains(&current.query),
                            "a focus did not withdraw {current:?}'s read: {trail}"
                        );
                        assert!(
                            !seen.spoke(current.trace),
                            "{current:?} spoke as a focus ended it: {trail}"
                        );
                        self.line.given_way.insert(current.query);
                        ended = true;
                        self.run.reach("a press ended by a focus");
                    }
                }
            }
            Move::Answer(query, outcome) => {
                if self.line.given_way.remove(&query) {
                    self.run.reach("an answer to a press that gave way");
                    assert_eq!(effects, [], "a late answer did something: {trail}");
                    let mut expected = before.clone();
                    let _ = expected.queries.end(query);
                    assert_eq!(expected, self.run.state, "a late answer: {trail}");
                } else if self.line.current.map(|current| current.query) == Some(query) {
                    let current = self.line.current.expect("the press whose read this is");
                    let reads = seen.reads_of(current.trace);
                    speaker = Some(current.trace);
                    match outcome {
                        tests::Outcome::Done if !reads.is_empty() => {
                            // A follow-up read: the press stays current.
                            let [read] = reads[..] else {
                                panic!("{current:?} asked several reads: {reads:?}: {trail}")
                            };
                            assert_eq!(read.node, current.node, "{current:?}: {trail}");
                            assert_eq!(
                                read.kind,
                                ReadKind::Text,
                                "only text reads follow up: {trail}"
                            );
                            self.line.current = Some(CurrentPress {
                                query: read.query,
                                ..current
                            });
                            self.run.reach("a press with a follow-up read");
                        }
                        tests::Outcome::Done => {
                            assert!(
                                seen.spoke(current.trace),
                                "an answered {current:?} said nothing: {trail}"
                            );
                            if matches!(
                                current.command,
                                ReviewCommand::Parent | ReviewCommand::NextSibling
                            ) {
                                self.line.navigator = Some(node_of(current.node.outpost(), 1));
                            }
                            self.line.current = None;
                            ended = true;
                        }
                        tests::Outcome::Gone => {
                            assert_eq!(reads.len(), 0, "{current:?}: {trail}");
                            // Report Current Object says nothing on a dead
                            // object; every other command runs on empty
                            // text or says its edge.
                            assert_eq!(
                                seen.spoke(current.trace),
                                current.command != ReviewCommand::ReportObject,
                                "{current:?} gone: {trail}"
                            );
                            self.line.current = None;
                            ended = true;
                            self.run.reach("a press answered gone");
                        }
                        tests::Outcome::Failed => {
                            assert_eq!(reads.len(), 0, "{current:?}: {trail}");
                            assert!(
                                !seen.spoke(current.trace),
                                "{current:?} spoke unanswered: {trail}"
                            );
                            self.line.current = None;
                            ended = true;
                            position_kept = true;
                            self.run.reach("a press unanswered");
                        }
                    }
                }
            }
            Move::Deadline(trace) => {
                if let Some(current) = self.line.current
                    && current.trace == trace
                {
                    assert!(
                        seen.withdrawn.contains(&current.query),
                        "{current:?}'s deadline did not withdraw its read: {trail}"
                    );
                    assert!(!seen.spoke(trace), "{current:?} spoke giving way: {trail}");
                    self.line.given_way.insert(current.query);
                    self.line.current = None;
                    ended = true;
                    position_kept = true;
                    self.run.reach("a press gave way at its bound");
                }
            }
            Move::OutpostEnded(outpost) => {
                if self
                    .line
                    .navigator
                    .is_some_and(|node| node.outpost() == outpost)
                {
                    self.line.navigator = None;
                }
                if self
                    .line
                    .focus
                    .is_some_and(|node| node.outpost() == outpost)
                {
                    self.line.focus = None;
                }
                if let Some(current) = self.line.current
                    && current.node.outpost() == outpost
                {
                    assert!(
                        !seen.spoke(current.trace),
                        "{current:?} spoke as its outpost ended: {trail}"
                    );
                    self.line.current = None;
                    ended = true;
                    self.run.reach("a press's outpost ended");
                }
            }
            Move::Exit => {
                exiting = true;
                if let Some(current) = self.line.current.take() {
                    self.line.given_way.insert(current.query);
                }
                self.line.queue.clear();
                self.line.key_ahead = false;
                if !self.line.unpassed.is_empty() {
                    self.run.reach("exit passed a held key");
                }
            }
        }
        for key in &seen.passed {
            assert!(
                self.line.unpassed.remove(key),
                "{key:?} passed on twice, or never held: {trail}"
            );
        }
        if exiting {
            assert_eq!(
                self.line.unpassed,
                BTreeSet::new(),
                "held keys exit left unpassed: {trail}"
            );
        }
        // What this step started, in press order.
        let mut started = BTreeSet::new();
        for trace in seen.order.iter().copied() {
            if started.insert(trace) && self.line.queue.iter().any(|entry| entry.trace() == trace) {
                self.start(trace, &seen, &mut armed, &trail);
            }
        }
        // A press ended with nothing ahead of it: what waits goes on.
        if ended && !self.line.key_ahead && self.line.current.is_none() {
            while matches!(
                self.line.queue.front(),
                Some(Entry::Typed(_) | Entry::Passed(_))
            ) {
                self.line.queue.pop_front();
            }
            assert_eq!(
                self.line.queue.front().map(|entry| entry.trace()),
                None,
                "the line did not go on: {trail}"
            );
        }
        if must_start {
            assert_eq!(
                self.line.queue.len(),
                0,
                "an entry on an idle line did not run: {trail}"
            );
        }
        // Only the rules' 150 ms arms a press.
        let bounds: Vec<(TraceId, u64)> = seen
            .bounds
            .iter()
            .copied()
            .filter(|(key, _)| self.line.presses.contains(key))
            .collect();
        let expected: Vec<(TraceId, u64)> = armed.iter().map(|trace| (*trace, 150)).collect();
        assert_eq!(bounds, expected, "a press's bound: {trail}");
        // A press that reads speaks only as its read is answered, in one
        // step.
        let spoken: BTreeSet<TraceId> = seen.spoke.iter().copied().collect();
        for trace in spoken {
            if let Some(steps) = self.line.readers.get_mut(&trace) {
                assert_eq!(speaker, Some(trace), "{trace:?} spoke unasked: {trail}");
                *steps += 1;
                assert_eq!(*steps, 1, "{trace:?} answered twice: {trail}");
            }
        }
        assert_eq!(
            self.run.state.navigator.as_ref().map(|nav| nav.object.id),
            self.line.navigator,
            "the navigator: {trail}"
        );
        if position_kept && self.line.navigator == navigator_was && !self.line.to_focus_ran {
            assert_eq!(
                self.run.state.navigator.as_ref().map(position),
                before.navigator.as_ref().map(position),
                "a press that said nothing moved the position: {trail}"
            );
        }
    }

    /// `trace`, somewhere in the queue, has started: everything pressed
    /// before it must have run already, and no press may be reading.
    fn start(&mut self, trace: TraceId, seen: &Seen, armed: &mut Vec<TraceId>, trail: &str) {
        while let Some(front) = self.line.queue.front().copied() {
            if front.trace() == trace {
                break;
            }
            assert!(
                matches!(front, Entry::Typed(_) | Entry::Passed(_)),
                "{trace:?} ran before {front:?}, pressed earlier: {trail}"
            );
            self.line.queue.pop_front();
        }
        let entry = self.line.queue.pop_front().expect("in the queue");
        assert!(
            self.line.current.is_none(),
            "{entry:?} ran with {:?} still reading: {trail}",
            self.line.current
        );
        match entry {
            Entry::Typed(_) => {}
            Entry::Held(key) => {
                self.line.key_ahead = !seen.reads_of(key).is_empty();
                self.run.reach("a key ran behind a press");
            }
            Entry::Passed(_) => self.line.key_ahead = true,
            Entry::Press {
                trace,
                command,
                navigator,
            } => {
                if self.line.key_ahead {
                    self.run.reach("a press ran behind a key");
                }
                self.line.key_ahead = false;
                if navigator != self.line.navigator {
                    self.run.reach("a waiting press ran on a moved navigator");
                }
                if command == ReviewCommand::ToFocus {
                    self.line.navigator = self.line.focus;
                    self.line.to_focus_ran = true;
                    assert!(seen.spoke(trace), "{trace:?} said nothing: {trail}");
                    return;
                }
                let Some(node) = self.line.navigator else {
                    assert_eq!(seen.reads_of(trace).len(), 0, "{trail}");
                    assert!(seen.spoke(trace), "{trace:?} said nothing: {trail}");
                    return;
                };
                let reads = seen.reads_of(trace);
                let [read] = reads[..] else {
                    panic!("{entry:?} asked {reads:?}: {trail}")
                };
                assert_eq!(read.node, node, "{entry:?} read a stale navigator: {trail}");
                let kind = match command {
                    ReviewCommand::Parent | ReviewCommand::NextSibling => ReadKind::Navigation,
                    ReviewCommand::ReportObject => ReadKind::Node,
                    _ if node.number() == 5 => ReadKind::Text,
                    _ => ReadKind::Node,
                };
                assert_eq!(read.kind, kind, "{entry:?}: {trail}");
                self.line.current = Some(CurrentPress {
                    trace,
                    command,
                    node,
                    query: read.query,
                });
                self.line.readers.insert(trace, 0);
                if !self.line.queue.is_empty() {
                    armed.push(trace);
                    self.run.reach("a press armed as it started");
                }
            }
        }
    }

    /// Answers every read still open, oldest first, and fires each deadline
    /// the shell would: the line ends empty.
    fn drain(&mut self) {
        // Exit ends the process: nothing answers it after, and the line is
        // already empty.
        if self.line.exited {
            assert!(!self.run.state.holds_inputs_back(), "{:?}", self.trail);
            return;
        }
        for _ in 0..400 {
            if let Some((query_id, (outpost, asked))) = self
                .run
                .wire
                .open
                .iter()
                .next()
                .map(|(id, open)| (*id, open.clone()))
            {
                self.answer(query_id, outpost, &asked, tests::Outcome::Done);
            } else if self.deadline.is_some() {
                self.deadline_passes();
            } else {
                break;
            }
        }
        let trail = format!("{:?}", self.trail);
        assert!(self.run.wire.open.is_empty(), "requests left: {trail}");
        assert!(self.line.current.is_none(), "a press left: {trail}");
        assert!(
            self.line
                .queue
                .iter()
                .all(|entry| matches!(entry, Entry::Typed(_) | Entry::Passed(_))),
            "entries left: {:?}: {trail}",
            self.line.queue
        );
        assert!(!self.run.state.holds_inputs_back(), "{trail}");
    }
}

/// Runs the press cases `config` draws, each twice from a new state,
/// shrinking a failure to its smallest case; what the cases reached.
///
/// # Panics
///
/// On a failing case, naming the smallest.
pub(super) fn run_press_generated(config: Config, again: &str) -> BTreeMap<&'static str, u32> {
    let reached = RefCell::new(BTreeMap::new());
    let mut runner = TestRunner::new(config);
    let strategy = proptest::collection::vec(press_action(), 1..40);
    let result = runner.run(&strategy, |actions| {
        let first = PressRun::case(&actions);
        let second = PressRun::case(&actions);
        assert_eq!(first.run.log, second.run.log, "two runs of one case differ");
        let mut reached = reached.borrow_mut();
        for (state, count) in first.run.reached {
            *reached.entry(state).or_default() += count;
        }
        Ok(())
    });
    match result {
        Ok(()) => reached.into_inner(),
        Err(TestError::Fail(reason, actions)) => {
            let actions: Vec<String> = actions.iter().map(|action| action.code()).collect();
            panic!("{reason}\n{}", found_report(&actions, again))
        }
        Err(error) => panic!("{error}"),
    }
}

/// The press gate's case count.
const PRESS_CASES: u32 = 300;

/// Review presses among keys and focus reports keep the line's rules
/// against the reference model after every input: every press answered at
/// most once and in press order among all input, a press waiting when the
/// navigator moves acting on the navigator as it is when its turn comes,
/// and the rest of `LineModel`'s rules; every listed state reached.
#[test]
fn generated_presses_keep_the_line_rules() {
    let reached = run_press_generated(
        super::explore::config(SEED, PRESS_CASES),
        "cargo test -p verbatim-core --lib -- --exact queries::generated::generated_presses_keep_the_line_rules",
    );
    let missed: Vec<&str> = LISTED_PRESS
        .into_iter()
        .filter(|state| !reached.contains_key(state))
        .collect();
    assert_eq!(
        missed,
        Vec::<&str>::new(),
        "states never reached; reached {reached:?}"
    );
}
