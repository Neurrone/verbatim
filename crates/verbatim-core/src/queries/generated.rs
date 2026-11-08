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
//! the flood policy (Dickson, 2026-11-02), and held keys, which [`key_line`]
//! below has as a world and a reference model of their own. A focus is
//! generated only in the front or with no window facts, from the one
//! application every generation reads, never a whole-window focus or a
//! group's.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use proptest::strategy::Strategy;
use proptest::test_runner::{Config, TestError, TestRunner};
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
fn found_report(actions: &[Action], again: &str) -> String {
    let actions: Vec<String> = actions.iter().map(|action| action.code()).collect();
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
                    1 => Input::CharacterTyped {
                        trace_id: trace,
                        key_seq: KeySeq::NONE_YET,
                        text: "a".to_owned(),
                        at_ms: now / 1_000,
                    },
                    _ => Input::SpeechCancelled { at_ms: now / 1_000 },
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

    /// Ends generation `outpost`.
    fn outpost_ended(&mut self, outpost: OutpostId, trace: TraceId) {
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
    let mut reached = key_line::run(config.clone(), again);
    reached.append(&mut run_slice(config, again));
    reached
}

/// Runs the slice's generated cases `config` draws; what they reached.
fn run_slice(config: Config, again: &str) -> BTreeMap<&'static str, u32> {
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
    let reached = run_slice(
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

/// Core's held keys, generated: a world of an editor and the keys pressed
/// into it, and a reference model with one record per key press, written
/// from `docs/parity/text.md`, `docs/nvda/editable-text-and-terminals.md`,
/// ADR 0001 and Dickson's decisions in
/// `docs/design/caret-selection-rework.md`. After every step, in terms of
/// keys and speech: every key reaches the application once, in press
/// order, and within its bounds; exit delivers every key still held, in
/// press order, since a key the user pressed is never swallowed, so
/// nothing is withheld after it, and nothing is let go or said after it;
/// the caret is
/// the one read most recently begun among those delivered; each selection
/// change is heard once and after the words of the key that made it; a key
/// speaks at most once, and not at all once a later key was pressed, except
/// for that key's selection changes. The world and the model name no
/// production type; everything that turns the world into Core's inputs
/// and reads its effects is [`seam`].
mod key_line {
    use std::collections::{BTreeMap, BTreeSet};

    use proptest::strategy::Strategy;
    use proptest::test_runner::{Config, TestError, TestRunner};

    use seam::{Core, Observed};

    /// A key the world presses.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Kind {
        Right,
        Left,
        Delete,
        Backspace,
        WordBackspace,
        ShiftRight,
    }

    /// A key press, by its place in the order pressed.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct KeyId(usize);

    /// A selection change the editor reports once, whose text names it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Change {
        key: usize,
        made_at: u64,
        selected: bool,
        text: String,
    }

    /// What the application answered to one read of the caret.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Read {
        began: u64,
        node: u64,
        line: String,
        at: u32,
        changes: Vec<Change>,
    }

    /// Where a focus report goes.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FocusTo {
        /// A button in the same top-level window.
        Elsewhere,
        /// A control in a new top-level window.
        Window,
        /// Back to the text field.
        Back,
        /// Another text field, its report waiting for a live re-read.
        Waits,
        /// That re-read's answer, which executes the report.
        Answer,
    }

    /// One thing heard, in terms of keys and speech.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum Heard {
        /// A key's unit, or text Backspace removed.
        Unit(String),
        /// A selection change.
        Selection { selected: bool, text: String },
        /// Speech of a focus.
        Focus,
        /// Speech cut.
        Cut,
    }

    /// The text field every generation reads, and the second a waiting
    /// focus lands on.
    const FIELD: u64 = 5;
    const SECOND_FIELD: u64 = 8;

    /// Everything that turns the world into Core's inputs and reads its
    /// effects, written for today's inputs and effects: the one part the
    /// caret and selection rework's builder changes.
    mod seam {
        use std::collections::BTreeMap;

        use verbatim_model::protocol::{QueryOutcome, QueryResult};
        use verbatim_model::{
            Backend, CaretKey, CaretMotion, CaretReply, CaretReport, Effect, FirstCachedAncestor,
            FocusReaderKind, FocusText, ForegroundWindow, Input, KeySeq, Message, NodeId,
            NormalizedEvent, OutpostId, Phrase, QueryId, Role, SegmentContent, SelectionChange,
            SelectionText, TextOp, TextReply, TraceId, Utterance,
        };

        use super::super::tests;
        use super::super::{APPLICATION, WINDOWS, facts};
        use super::{Change, FIELD, FocusTo, Heard, KeyId, Kind, Read, SECOND_FIELD};
        use crate::reduce;
        use crate::state::SrState;

        const OUTPOST: OutpostId = OutpostId(1);
        const ANCHOR: u64 = 100;

        /// What one step did: the keys let go, and what was heard.
        #[derive(Default, Debug)]
        pub(super) struct Observed {
            pub(super) passed: Vec<KeyId>,
            pub(super) heard: Vec<Heard>,
        }

        impl Observed {
            fn and(mut self, more: Self) -> Self {
                self.passed.extend(more.passed);
                self.heard.extend(more.heard);
                self
            }
        }

        fn key_trace(key: KeyId) -> TraceId {
            TraceId::from_raw(100_000 + key.0 as u64)
        }

        fn key_of(trace: TraceId) -> KeyId {
            let raw: u64 = trace.to_string().parse().expect("a trace's number");
            KeyId(usize::try_from(raw - 100_000).expect("a key's trace"))
        }

        fn node(number: u64) -> NodeId {
            NodeId::in_outpost(OUTPOST, number)
        }

        fn caret_key(kind: Kind) -> CaretKey {
            let (motion, select) = match kind {
                Kind::Right => (CaretMotion::NextCharacter, false),
                Kind::Left => (CaretMotion::PreviousCharacter, false),
                Kind::Delete => (CaretMotion::Delete, false),
                Kind::Backspace => (CaretMotion::Backspace, false),
                Kind::WordBackspace => (CaretMotion::BackspaceWord, false),
                Kind::ShiftRight => (CaretMotion::NextCharacter, true),
            };
            CaretKey { motion, select }
        }

        fn report(read: &Read) -> CaretReport {
            CaretReport {
                line: tests::line(&read.line, ANCHOR, read.at),
                selection: None,
                selection_line: None,
            }
        }

        fn changes(read: &Read) -> Vec<SelectionChange> {
            read.changes
                .iter()
                .map(|change: &Change| SelectionChange {
                    selected: change.selected,
                    text: change.text.clone(),
                    characters: 2,
                })
                .collect()
        }

        fn reply(read: &Read, moved: bool) -> TextReply {
            TextReply::Caret(Box::new(CaretReply {
                moved,
                caret: report(read),
                read_at_ms: read.began,
                unit: None,
                selection_changes: changes(read),
                content_changed: false,
                new_selection: None,
                same_line: None,
                removed: None,
            }))
        }

        fn heard(utterance: &Utterance) -> Heard {
            if utterance.validity.is_some() {
                return Heard::Focus;
            }
            let [segment] = &utterance.segments[..] else {
                return Heard::Focus;
            };
            let spoken = |text: &SelectionText| match text {
                SelectionText::Text(text) | SelectionText::Character(text) => text.clone(),
                SelectionText::Characters(count) => count.to_string(),
            };
            match &segment.content {
                SegmentContent::Character(text) | SegmentContent::Text(text) => {
                    Heard::Unit(text.clone())
                }
                SegmentContent::Message(Message::Blank) => Heard::Unit("blank".to_owned()),
                SegmentContent::Phrase(Phrase::Selected(text)) => Heard::Selection {
                    selected: true,
                    text: spoken(text),
                },
                SegmentContent::Phrase(Phrase::Unselected(text)) => Heard::Selection {
                    selected: false,
                    text: spoken(text),
                },
                _ => Heard::Focus,
            }
        }

        /// Core, its focus on the text field, and what Core has open for
        /// the world to answer.
        pub(super) struct Core {
            state: SrState,
            trace: u64,
            /// Each key's before read: its request and the node it reads.
            reading: BTreeMap<KeyId, (QueryId, u64)>,
            /// Each key's watch for evidence: its request and node.
            watching: BTreeMap<KeyId, (QueryId, u64)>,
            /// The shell's one held-key deadline: its key and due time.
            due: Option<(TraceId, u64)>,
            /// The report set aside for a live re-read, and its request.
            waiting: Option<(Input, Option<u64>)>,
            /// Each input's effects, for the run twice.
            pub(super) log: Vec<String>,
        }

        impl Core {
            pub(super) fn new(read: &Read, now: u64) -> Self {
                let mut core = Self {
                    state: SrState::new(),
                    trace: 1,
                    reading: BTreeMap::new(),
                    watching: BTreeMap::new(),
                    due: None,
                    waiting: None,
                    log: Vec::new(),
                };
                let change = Self::foreground(WINDOWS[0], now - 2);
                core.give(&change, now);
                let focus = core.focus_report(FIELD, Some(read), (now - 1, 0), (None, None));
                core.give(&focus, now);
                core
            }

            fn foreground(window: verbatim_model::WindowHandle, at_ms: u64) -> Input {
                Input::Foreground(ForegroundWindow {
                    window,
                    root_owner: window,
                    process: APPLICATION,
                    observed_at_us: at_ms * 1_000,
                    own: false,
                    thread: 1,
                    console: false,
                    integrity: None,
                    own_integrity: None,
                })
            }

            fn next_trace(&mut self) -> TraceId {
                self.trace += 1;
                TraceId::from_raw(self.trace)
            }

            /// The focus report of node `number`, observed at `at`, in the
            /// top-level window `at.1` numbers, with `read` for a text
            /// field, and its cached ancestor and answer, if any.
            fn focus_report(
                &mut self,
                number: u64,
                read: Option<&Read>,
                at: (u64, usize),
                (cached, answers): (Option<u32>, Option<u64>),
            ) -> Input {
                let role = if read.is_some() {
                    Role::EditableText
                } else {
                    Role::Button
                };
                let mut ancestors = vec![tests::node(
                    OUTPOST,
                    1 + at.1 as u64,
                    Role::Window,
                    if at.1 == 0 { "Window" } else { "Second" },
                )];
                if number == SECOND_FIELD {
                    ancestors.push(tests::node(OUTPOST, 9, Role::Pane, "Tools"));
                }
                Input::Event {
                    trace_id: self.next_trace(),
                    observed_at_us: at.0 * 1_000,
                    source: APPLICATION,
                    backend: Backend::Uia,
                    window: Some(facts(WINDOWS[at.1])),
                    event: NormalizedEvent::FocusChanged {
                        node: tests::node(OUTPOST, number, role, "Element"),
                        whole_window: false,
                        ancestors,
                        selected_child: None,
                        text: read.map(|read| {
                            Box::new(FocusText::Caret {
                                caret: report(read),
                                selected_text: None,
                            })
                        }),
                        earlier: None,
                        group: None,
                        first_cached_ancestor: cached.map(|index| FirstCachedAncestor {
                            index,
                            reader: FocusReaderKind::Listener,
                        }),
                        answers,
                    },
                    purpose: None,
                }
            }

            fn give(&mut self, input: &Input, now: u64) -> Observed {
                let effects = reduce(&mut self.state, input);
                self.log.push(format!("{effects:?}"));
                let mut observed = Observed::default();
                let mut awaited = None;
                self.observe(&effects, None, now, &mut awaited, &mut observed);
                observed
            }

            fn observe(
                &mut self,
                effects: &[Effect],
                key: Option<TraceId>,
                now: u64,
                awaited: &mut Option<(QueryId, u64)>,
                out: &mut Observed,
            ) {
                for effect in effects {
                    match effect {
                        Effect::ForKey(key, inner) => {
                            self.observe(
                                std::slice::from_ref(&**inner),
                                Some(*key),
                                now,
                                awaited,
                                out,
                            );
                        }
                        Effect::PassHeldKey(trace) => {
                            let passed = key_of(*trace);
                            if let Some(watch) = awaited.take() {
                                self.watching.insert(passed, watch);
                            }
                            out.passed.push(passed);
                        }
                        Effect::BoundHeldKey(bound) => {
                            self.due = bound.map(|(trace, ms)| (trace, now + ms));
                        }
                        Effect::Text(request) => {
                            let asked = (request.query_id, request.node_id.number());
                            match (&request.op, key) {
                                (TextOp::ReadCaret, Some(key)) => {
                                    self.reading.insert(key_of(key), asked);
                                }
                                (TextOp::AwaitCaret(_), Some(key)) => {
                                    self.watching.insert(key_of(key), asked);
                                }
                                (TextOp::AwaitCaret(_), None) => *awaited = Some(asked),
                                _ => {}
                            }
                        }
                        Effect::Withdraw { query_id, .. } => {
                            self.reading.retain(|_, (open, _)| open != query_id);
                        }
                        Effect::Speak(utterance, _) => out.heard.push(heard(utterance)),
                        Effect::StopSpeech => out.heard.push(Heard::Cut),
                        Effect::LiveReread(reread) => {
                            let (report, _) = self.waiting.take().expect("a report set aside");
                            self.waiting = Some((report, Some(reread.request)));
                        }
                        _ => {}
                    }
                }
            }

            /// Whether `key`'s before read is open, and the node it reads.
            pub(super) fn reading(&self, key: KeyId) -> Option<u64> {
                self.reading.get(&key).map(|(_, node)| *node)
            }

            /// The node `key`'s open watch reads.
            pub(super) fn watching(&self, key: KeyId) -> Option<u64> {
                self.watching.get(&key).map(|(_, node)| *node)
            }

            pub(super) fn press(&mut self, key: KeyId, kind: Kind, now: u64) -> Observed {
                // The hook cuts speech for a key before it reports the key.
                let cut = self.give(&Input::SpeechCancelled { at_ms: now }, now);
                let held = self.state.holds_caret_keys() || self.state.holds_inputs_back();
                let (trace_id, key_seq, key_of_press) =
                    (key_trace(key), KeySeq::NONE_YET, caret_key(kind));
                let input = if held {
                    Input::HeldKey {
                        trace_id,
                        key_seq,
                        key: key_of_press,
                        pressed_at_ms: now,
                    }
                } else {
                    Input::CaretKey {
                        trace_id,
                        key_seq,
                        key: key_of_press,
                        pressed_at_ms: now,
                    }
                };
                let mut observed = cut.and(self.give(&input, now));
                // A key the hook did not hold has reached the application.
                if !held {
                    observed.passed.push(key);
                }
                observed
            }

            /// The outpost answers `key`'s before read.
            pub(super) fn before_read(&mut self, key: KeyId, read: &Read, now: u64) -> Observed {
                let Some((query_id, _)) = self.reading.remove(&key) else {
                    return Observed::default();
                };
                self.answered(query_id, reply(read, false), now)
            }

            fn answered(&mut self, query_id: QueryId, reply: TextReply, now: u64) -> Observed {
                let input = Input::Answered {
                    trace_id: self.next_trace(),
                    query_id,
                    outpost: OUTPOST,
                    outcome: QueryOutcome::Done(QueryResult::Text(reply)),
                };
                self.give(&input, now)
            }

            /// The outpost's watch finds `key`'s evidence.
            pub(super) fn evidence(&mut self, key: KeyId, read: &Read, now: u64) -> Observed {
                let Some((query_id, _)) = self.watching.remove(&key) else {
                    return Observed::default();
                };
                self.answered(query_id, reply(read, true), now)
            }

            /// `key`'s 100 ms have passed with no evidence, and the outpost
            /// reads the caret again at the bound. Today nothing tells
            /// Core: its watch stays open, it answers nothing at the
            /// bound, and the read reaches no one, so this gives no input
            /// and says the read was not delivered.
            #[expect(
                clippy::unused_self,
                reason = "the seam's shape, which the rework's Core input fills"
            )]
            pub(super) fn bound_passed(
                &mut self,
                _key: KeyId,
                _read: &Read,
                _now: u64,
            ) -> (Observed, bool) {
                (Observed::default(), false)
            }

            /// A caret event for `node`.
            pub(super) fn caret_event(
                &mut self,
                node_number: u64,
                read: &Read,
                now: u64,
            ) -> Observed {
                let input = Input::Event {
                    trace_id: self.next_trace(),
                    observed_at_us: read.began * 1_000,
                    source: APPLICATION,
                    backend: Backend::Uia,
                    window: None,
                    event: NormalizedEvent::CaretMoved {
                        node_id: node(node_number),
                        caret: report(read),
                        selection_changes: changes(read),
                        content_changed: false,
                        new_selection: None,
                    },
                    purpose: None,
                };
                self.give(&input, now)
            }

            /// Time reaches `now`: any held-key deadline due fires.
            pub(super) fn time(&mut self, now: u64) -> Observed {
                let mut observed = Observed::default();
                while let Some((trace_id, due)) = self.due
                    && due <= now
                {
                    self.due = None;
                    let more = self.give(&Input::HeldKeyDue { trace_id }, due);
                    observed = observed.and(more);
                }
                observed
            }

            /// A focus report, per `to`.
            pub(super) fn focus(&mut self, to: FocusTo, read: Option<&Read>, now: u64) -> Observed {
                match to {
                    FocusTo::Elsewhere => {
                        let report = self.focus_report(6, None, (now, 0), (None, None));
                        self.give(&report, now)
                    }
                    FocusTo::Window => {
                        let change = Self::foreground(WINDOWS[1], now - 1);
                        let first = self.give(&change, now);
                        let report = self.focus_report(7, None, (now, 1), (None, None));
                        first.and(self.give(&report, now))
                    }
                    FocusTo::Back => {
                        let report = self.focus_report(FIELD, read, (now, 0), (None, None));
                        self.give(&report, now)
                    }
                    FocusTo::Waits => {
                        let report =
                            self.focus_report(SECOND_FIELD, read, (now, 0), (Some(1), None));
                        self.waiting = Some((report.clone(), None));
                        let observed = self.give(&report, now);
                        assert!(
                            self.waiting
                                .as_ref()
                                .is_some_and(|(_, request)| request.is_some()),
                            "the report was not set aside"
                        );
                        observed
                    }
                    FocusTo::Answer => {
                        let (_, request) = self.waiting.take().expect("a report waits");
                        let request = request.expect("the re-read was asked");
                        let report =
                            self.focus_report(SECOND_FIELD, read, (now, 0), (None, Some(request)));
                        self.give(&report, now)
                    }
                }
            }

            pub(super) fn outpost_ended(&mut self, now: u64) -> Observed {
                let input = Input::OutpostEnded {
                    trace_id: self.next_trace(),
                    outpost: OUTPOST,
                };
                let observed = self.give(&input, now);
                self.reading.clear();
                self.watching.clear();
                self.waiting = None;
                observed
            }

            pub(super) fn exit(&mut self, now: u64) -> Observed {
                self.give(&Input::Exiting { replaced: false }, now)
            }

            /// The caret position Core holds for its focus.
            pub(super) fn caret_at(&self) -> Option<u32> {
                self.state
                    .caret
                    .as_ref()
                    .filter(|caret| self.state.focus_matches(caret.node))
                    .map(|caret| caret.line.offset)
            }

            /// Whether Core still holds a key or command back.
            pub(super) fn withholding(&self) -> bool {
                self.state.holds_inputs_back()
            }
        }
    }

    /// The editor: one line of distinct characters, a caret, how many
    /// characters behind the caret are selected, and what the application
    /// has done, each state stamped by when it began.
    struct Editor {
        line: Vec<char>,
        pos: usize,
        selected: usize,
        history: Vec<(u64, Vec<char>, usize)>,
        pending: Vec<Change>,
        used: BTreeSet<u64>,
        made: u32,
    }

    impl Editor {
        fn new() -> Self {
            let line: Vec<char> = ('a'..='z').chain('0'..='9').collect();
            Self {
                pos: 10,
                selected: 0,
                history: vec![(0, line.clone(), 10)],
                line,
                pending: Vec::new(),
                used: BTreeSet::new(),
                made: 0,
            }
        }

        /// A read of the caret of `node`, begun `ago` ms before `now` but
        /// no earlier than `floor`, and carrying with `carry` the selection
        /// changes it can see; each read begins at a different time.
        fn read(&mut self, node: u64, now: u64, (ago, floor): (u64, u64), carry: bool) -> Read {
            let mut began = now.saturating_sub(ago).max(floor);
            while !self.used.insert(began) {
                began -= 1;
            }
            let (_, line, at) = self
                .history
                .iter()
                .rev()
                .find(|(time, _, _)| *time <= began)
                .expect("the first state began at zero");
            let (mut changes, mut later) = (Vec::new(), Vec::new());
            for change in self.pending.drain(..) {
                if carry && change.made_at <= began {
                    changes.push(change);
                } else {
                    later.push(change);
                }
            }
            self.pending = later;
            Read {
                began,
                node,
                line: line.iter().collect(),
                at: u32::try_from(*at).expect("a short line"),
                changes,
            }
        }

        fn change(&mut self, key: usize, made_at: u64, selected: bool) {
            self.made += 1;
            self.pending.push(Change {
                key,
                made_at,
                selected,
                text: format!("s{}", self.made),
            });
        }

        /// The application handles `kind` at `now`: what it removed, and
        /// whether anything changed.
        fn apply(
            &mut self,
            key: usize,
            kind: Kind,
            moves: bool,
            now: u64,
        ) -> (Option<String>, bool) {
            let (len, pos) = (self.line.len(), self.pos);
            let mut removed = None;
            let changed = match kind {
                _ if !moves => false,
                Kind::Right if pos < len => {
                    self.pos += 1;
                    true
                }
                Kind::Left if pos > 0 => {
                    self.pos -= 1;
                    true
                }
                Kind::Delete if pos < len => {
                    self.line.remove(pos);
                    true
                }
                Kind::Backspace if pos > 0 => {
                    removed = Some(self.line.remove(pos - 1).to_string());
                    self.pos -= 1;
                    true
                }
                Kind::WordBackspace if pos > 0 => {
                    removed = Some(self.line.drain(..pos).collect());
                    self.pos = 0;
                    true
                }
                Kind::ShiftRight if pos < len => {
                    self.pos += 1;
                    self.selected += 1;
                    self.change(key, now, true);
                    true
                }
                _ => false,
            };
            if changed && kind != Kind::ShiftRight && self.selected > 0 {
                self.selected = 0;
                self.change(key, now, false);
            }
            if changed {
                self.history.push((now, self.line.clone(), self.pos));
            }
            (removed, changed)
        }
    }

    /// One record per key press.
    #[derive(Debug)]
    #[expect(clippy::struct_excessive_bools, reason = "each is a fact of the press")]
    struct Key {
        kind: Kind,
        moves: bool,
        pressed: u64,
        delivered: Option<u64>,
        handled: bool,
        applied: Option<u64>,
        removed: Option<String>,
        /// The read the key is judged from: its before read, or the last
        /// read when it was passed on without one.
        judged: Option<Read>,
        /// No read or bound may start before this: the end of a focus
        /// report's wait it pressed behind.
        floor: u64,
        evidence: Option<u64>,
        /// Whether the key's words are settled, said or not.
        resolved: bool,
        /// Whether the key says nothing now.
        silent: bool,
        words_at: Option<usize>,
        later: Option<u64>,
        /// The focus moved after the key was passed on: its watch ended, and
        /// no answer of it comes.
        dropped: bool,
    }

    #[derive(Debug)]
    struct ChangeRecord {
        key: usize,
        heard: u32,
        delivered: bool,
        dead: bool,
    }

    /// The reference model.
    #[derive(Debug, Default)]
    struct Model {
        keys: Vec<Key>,
        focus_node: Option<u64>,
        last_field: Option<u64>,
        queued_focus: bool,
        ended: bool,
        exited: bool,
        caret: Option<(u64, u32)>,
        last_read: Option<Read>,
        changes: BTreeMap<String, ChangeRecord>,
        heard_count: usize,
    }

    /// A key's unit at `read`, from the kind's rule.
    fn unit(kind: Kind, read: &Read, removed: Option<&str>) -> Option<String> {
        let at = read.at as usize;
        match kind {
            Kind::Right | Kind::Left | Kind::Delete => Some(
                read.line
                    .chars()
                    .nth(at)
                    .map_or_else(|| "blank".to_owned(), |character| character.to_string()),
            ),
            Kind::Backspace | Kind::WordBackspace => removed.map(str::to_owned),
            Kind::ShiftRight => None,
        }
    }

    /// A key's unit as NVDA's script would say it from `read`, the read
    /// before the key: what is at the caret, what Backspace would remove.
    fn unit_before(kind: Kind, read: &Read) -> Option<String> {
        let at = read.at as usize;
        match kind {
            Kind::Backspace => read
                .line
                .chars()
                .nth(at.checked_sub(1)?)
                .map(|character| character.to_string()),
            Kind::WordBackspace => (at > 0).then(|| read.line.chars().take(at).collect()),
            _ => unit(kind, read, None),
        }
    }

    impl Model {
        fn text(&self) -> bool {
            self.focus_node.is_some()
        }

        fn take_read(&mut self, read: &Read) {
            // After exit no later input does anything.
            if self.exited {
                return;
            }
            if Some(read.node) == self.focus_node
                && self.caret.is_none_or(|(began, _)| read.began > began)
            {
                self.caret = Some((read.began, read.at));
                self.last_read = Some(read.clone());
            }
            for change in &read.changes {
                self.changes.insert(
                    change.text.clone(),
                    ChangeRecord {
                        key: change.key,
                        heard: 0,
                        delivered: true,
                        dead: false,
                    },
                );
            }
        }

        fn press(&mut self, kind: Kind, moves: bool, now: u64) {
            for key in &mut self.keys {
                key.later.get_or_insert(now);
            }
            self.keys.push(Key {
                kind,
                moves,
                pressed: now,
                delivered: None,
                handled: false,
                applied: None,
                removed: None,
                judged: None,
                floor: now,
                evidence: None,
                resolved: false,
                silent: false,
                words_at: None,
                later: None,
                dropped: false,
            });
        }

        fn may_speak(&self, key: usize, began: u64) -> bool {
            let key = &self.keys[key];
            !self.exited
                && !self.ended
                && !key.silent
                && !key.resolved
                && self.text()
                && key.later.is_none_or(|later| began < later)
        }

        fn before_read(&mut self, key: usize, read: &Read) {
            self.keys[key].judged = Some(read.clone());
            self.take_read(read);
        }

        /// The evidence for `key` found by `read`: the words it says.
        fn evidence(&mut self, key: usize, read: &Read, now: u64) -> Option<(usize, String)> {
            let may = self.may_speak(key, read.began);
            self.take_read(read);
            let record = &mut self.keys[key];
            record.evidence = Some(now);
            record.resolved = true;
            let words = unit(record.kind, read, record.removed.as_deref());
            words.filter(|_| may).map(|words| (key, words))
        }

        /// `key`'s bound passed with no evidence: the words it says. The
        /// read at the bound is taken as the newest caret when `delivered`.
        fn bound(&mut self, key: usize, read: &Read, delivered: bool) -> Option<(usize, String)> {
            let may = self.may_speak(key, read.began);
            if delivered {
                self.take_read(read);
            }
            let record = &mut self.keys[key];
            record.resolved = true;
            let removed = record
                .removed
                .as_deref()
                .filter(|_| record.applied.is_some_and(|at| at <= read.began));
            let words = unit(record.kind, read, removed);
            words.filter(|_| may).map(|words| (key, words))
        }

        /// The focus moves at `now`, a read running on `reading`: what the
        /// keys say first.
        fn focus_moves(&mut self, now: u64, reading: &[usize]) -> Vec<(usize, String)> {
            let mut words = Vec::new();
            let exited = self.exited;
            for (index, key) in self.keys.iter_mut().enumerate() {
                if key.delivered.is_none() {
                    key.silent |= reading.contains(&index);
                    continue;
                }
                // A focus change drops a watched key's answer.
                key.dropped = true;
                if !key.resolved {
                    key.resolved = true;
                    let within = key.delivered.is_some_and(|at| now < at + 100);
                    if !exited && !key.silent && key.later.is_none() && within {
                        let spoken = key
                            .judged
                            .as_ref()
                            .and_then(|read| unit_before(key.kind, read));
                        words.extend(spoken.map(|spoken| (index, spoken)));
                    }
                }
            }
            for change in self.changes.values_mut() {
                change.dead |= change.heard == 0;
            }
            words
        }

        /// The keys Core let go at `now`: each once, in press order, and
        /// never after exit.
        fn delivered(&mut self, passed: &[KeyId], now: u64, trail: &str) {
            let last = self.last_read.clone();
            let text = self.text();
            assert!(
                !self.exited || passed.is_empty(),
                "a key let go after exit: {passed:?}: {trail}"
            );
            for id in passed {
                assert!(
                    self.keys[..id.0].iter().all(|key| key.delivered.is_some()),
                    "{id:?} passed before a key pressed earlier: {trail}"
                );
                let key = &mut self.keys[id.0];
                assert!(key.delivered.is_none(), "{id:?} let go twice: {trail}");
                key.delivered = Some(now);
                key.judged = key.judged.take().or(last.clone());
                // Pressed where nothing can be read, it has no words.
                key.resolved |= !text || self.ended;
            }
        }

        /// What this step said, against `expected`, and each selection
        /// change against its key's words.
        fn compare(
            &mut self,
            expected: &[(usize, String)],
            focus: Option<bool>,
            observed: &Observed,
            trail: &str,
        ) {
            let units: Vec<String> = observed
                .heard
                .iter()
                .filter_map(|heard| match heard {
                    Heard::Unit(words) => Some(words.clone()),
                    _ => None,
                })
                .collect();
            let wanted: Vec<String> = expected.iter().map(|(_, words)| words.clone()).collect();
            assert_eq!(
                units, wanted,
                "a key's words: heard {:?}: {trail}",
                observed.heard
            );
            let position = |target: &Heard| observed.heard.iter().position(|heard| heard == target);
            let last_unit = observed
                .heard
                .iter()
                .rposition(|heard| matches!(heard, Heard::Unit(_)));
            let cut = position(&Heard::Cut);
            let focus_at = position(&Heard::Focus);
            match focus {
                Some(new_window) => {
                    assert!(
                        focus_at.is_some(),
                        "the focus unheard: {:?}: {trail}",
                        observed.heard
                    );
                    if let Some(last) = last_unit {
                        assert!(
                            focus_at > Some(last),
                            "a key's words after the focus: {:?}: {trail}",
                            observed.heard
                        );
                        if new_window {
                            assert!(
                                cut.is_some_and(|at| at > last && Some(at) < focus_at),
                                "a new window's focus does not cut the key's words: {:?}: {trail}",
                                observed.heard
                            );
                        }
                    }
                }
                None => assert!(
                    focus_at.is_none() && cut.is_none(),
                    "focus speech with no focus: {:?}: {trail}",
                    observed.heard
                ),
            }
            let mut said = expected.iter();
            for heard in &observed.heard {
                let at = self.heard_count;
                self.heard_count += 1;
                match heard {
                    Heard::Unit(_) => {
                        let (key, _) = said.next().expect("the words compared above");
                        self.keys[*key].words_at = Some(at);
                    }
                    Heard::Selection { text, .. } => self.selection_heard(text, at, trail),
                    Heard::Focus | Heard::Cut => {}
                }
            }
        }

        fn selection_heard(&mut self, text: &str, at: usize, trail: &str) {
            let change = self.changes.get_mut(text);
            let Some(change) = change else {
                panic!("a selection change never reported was heard: {text}: {trail}");
            };
            assert!(
                !change.dead,
                "a selection change heard after its focus left: {text}: {trail}"
            );
            change.heard += 1;
            assert_eq!(
                change.heard, 1,
                "a selection change heard twice: {text}: {trail}"
            );
            let key = change.key;
            let record = &self.keys[key];
            assert!(
                record.resolved || record.silent,
                "a selection change before its key's words: {text}: {trail}"
            );
            if let Some(words_at) = record.words_at {
                assert!(
                    words_at < at,
                    "a selection change before its key's words: {text}: {trail}"
                );
            }
        }

        /// The keys Core may still hold at `now`: only the first undelivered,
        /// within 150 ms of the time it could be read, which is its
        /// press, the end of the focus wait it pressed behind, or the
        /// evidence or 100 ms bound of the key ahead.
        fn check_bounds(&self, now: u64, trail: &str) {
            if self.queued_focus || self.exited {
                return;
            }
            let Some(index) = self.keys.iter().position(|key| key.delivered.is_none()) else {
                return;
            };
            let key = &self.keys[index];
            let ahead = index
                .checked_sub(1)
                .map(|at| &self.keys[at])
                .and_then(|ahead| ahead.delivered.map(|at| (ahead, at)));
            let turn = ahead.map_or(0, |(ahead, delivered)| {
                ahead.evidence.unwrap_or(u64::MAX).min(delivered + 100)
            });
            let ready = key.pressed.max(key.floor).max(turn);
            assert!(
                now < ready + 150,
                "key {index} still held past its bounds at {now}, ready at {ready}: {trail}"
            );
        }

        fn check_caret(&self, core: &Core, trail: &str) {
            if self.text() && !self.queued_focus {
                assert_eq!(
                    core.caret_at(),
                    self.caret.map(|(_, at)| at),
                    "Core's caret is not the newest read delivered: {:?}: {trail}",
                    self.caret
                );
            }
        }

        /// The end of a case: every change reported on a field still
        /// focused was heard.
        fn finish(&self, trail: &str) {
            for (text, change) in &self.changes {
                if change.delivered && !change.dead {
                    assert_eq!(change.heard, 1, "selection change {text} unheard: {trail}");
                }
            }
        }
    }

    /// One step of the world.
    #[derive(Clone, Copy, Debug)]
    enum Step {
        /// A key press: whether the application then moves anything.
        Press { kind: Kind, moves: bool },
        /// The outpost answers the oldest open before read, begun `ago` ms
        /// before.
        BeforeRead { ago: u8 },
        /// The application handles the next key it was given.
        Handle,
        /// The newest (or oldest) key with an effect finds its evidence, in
        /// a read begun `ago` ms before.
        Evidence { newest: bool, ago: u8 },
        /// A caret event, early, late or a repeat of one.
        Report { ago: u8 },
        /// A caret report for the field the focus left.
        Behind,
        /// Time passes by this many ms.
        Time(u8),
        /// A focus report.
        Focus(FocusTo),
        /// The outpost ends.
        OutpostEnded,
        /// Verbatim exits.
        Exit,
    }

    fn step() -> impl Strategy<Value = Step> {
        use proptest::prelude::*;
        let kind = prop_oneof![
            Just(Kind::Right),
            Just(Kind::Left),
            Just(Kind::Delete),
            Just(Kind::Backspace),
            Just(Kind::WordBackspace),
            Just(Kind::ShiftRight),
        ];
        let target = prop_oneof![
            Just(FocusTo::Elsewhere),
            Just(FocusTo::Window),
            Just(FocusTo::Back),
            Just(FocusTo::Waits),
            Just(FocusTo::Answer),
        ];
        let ago = prop_oneof![Just(0u8), Just(3u8), Just(40u8), Just(120u8)];
        prop_oneof![
            8 => (kind, prop::bool::weighted(0.7)).prop_map(|(kind, moves)| Step::Press { kind, moves }),
            8 => ago.clone().prop_map(|ago| Step::BeforeRead { ago }),
            8 => Just(Step::Handle),
            8 => (any::<bool>(), ago.clone()).prop_map(|(newest, ago)| Step::Evidence { newest, ago }),
            4 => ago.prop_map(|ago| Step::Report { ago }),
            1 => Just(Step::Behind),
            10 => prop_oneof![Just(5u8), Just(40u8), Just(90u8), Just(110u8), Just(160u8)]
                .prop_map(Step::Time),
            4 => target.prop_map(Step::Focus),
            1 => Just(Step::OutpostEnded),
            1 => Just(Step::Exit),
        ]
    }

    /// One case's run.
    struct Run {
        core: Core,
        editor: Editor,
        model: Model,
        now: u64,
        trail: Vec<Step>,
        reached: BTreeMap<&'static str, u32>,
    }

    impl Run {
        fn new() -> Self {
            let mut editor = Editor::new();
            let now = 1_000_000;
            let read = editor.read(FIELD, now - 1, (0, 0), false);
            let core = Core::new(&read, now);
            let model = Model {
                focus_node: Some(FIELD),
                last_field: Some(FIELD),
                caret: Some((read.began, read.at)),
                last_read: Some(read),
                ..Model::default()
            };
            Self {
                core,
                editor,
                model,
                now,
                trail: Vec::new(),
                reached: BTreeMap::new(),
            }
        }

        fn reach(&mut self, state: &'static str) {
            *self.reached.entry(state).or_default() += 1;
        }

        fn trail(&self) -> String {
            format!("{:?}", self.trail)
        }

        /// Compares what a step did with what the model expected.
        fn check(
            &mut self,
            expected: &[(usize, String)],
            focus: Option<bool>,
            observed: &Observed,
        ) {
            let trail = self.trail();
            self.model.delivered(&observed.passed, self.now, &trail);
            if !self.model.exited {
                self.model.compare(expected, focus, observed, &trail);
            }
            self.model.check_bounds(self.now, &trail);
            self.model.check_caret(&self.core, &trail);
            if self.model.exited {
                assert!(
                    !self.core.withholding(),
                    "a key withheld after exit: {trail}"
                );
            }
        }

        /// Keys whose 100 ms have passed at the clock `now`, in press order.
        fn bounds_due(&self) -> Vec<(usize, u64)> {
            self.model
                .keys
                .iter()
                .enumerate()
                .filter(|(_, key)| !key.resolved && key.delivered.is_some())
                .filter_map(|(index, key)| key.delivered.map(|at| (index, at + 100)))
                .filter(|(_, bound)| *bound <= self.now)
                .collect()
        }

        fn act(&mut self, step: Step) {
            self.trail.push(step);
            self.now += 1;
            let now = self.now;
            match step {
                Step::Press { kind, moves } => {
                    if self.model.exited {
                        return;
                    }
                    // Deleting and collapsing keys over a selection are not
                    // generated: only the plain keys leave one.
                    let kind = if self.editor.selected > 0
                        && matches!(kind, Kind::Delete | Kind::Backspace | Kind::WordBackspace)
                    {
                        Kind::Right
                    } else {
                        kind
                    };
                    let id = KeyId(self.model.keys.len());
                    self.model.press(kind, moves, now);
                    let observed = self.core.press(id, kind, now);
                    self.check(&[], None, &observed);
                }
                Step::BeforeRead { ago } => self.before_read(u64::from(ago), now),
                Step::Handle => self.handle(now),
                Step::Evidence { newest, ago } => self.evidence(newest, u64::from(ago), now),
                Step::Report { ago } => {
                    if let Some(node) = self.model.focus_node {
                        let read = self.editor.read(node, now, (u64::from(ago), 0), true);
                        self.model.take_read(&read);
                        let observed = self.core.caret_event(node, &read, now);
                        self.check(&[], None, &observed);
                    }
                }
                Step::Behind => {
                    if let Some(left) = self
                        .model
                        .last_field
                        .filter(|left| Some(*left) != self.model.focus_node)
                    {
                        let read = self.editor.read(left, now, (0, 0), false);
                        let observed = self.core.caret_event(left, &read, now);
                        self.reach("a report behind the focus");
                        self.check(&[], None, &observed);
                    }
                }
                Step::Time(ms) => self.time(u64::from(ms)),
                Step::Focus(to) => self.focus(to, now),
                Step::OutpostEnded => {
                    if self.model.ended || self.model.exited {
                        return;
                    }
                    self.model.ended = true;
                    self.model.queued_focus = false;
                    self.model.focus_node = None;
                    self.model.caret = None;
                    for key in &mut self.model.keys {
                        key.resolved = true;
                    }
                    for change in self.model.changes.values_mut() {
                        change.dead |= change.heard == 0;
                    }
                    self.editor.pending.clear();
                    self.reach("an outpost ended");
                    let observed = self.core.outpost_ended(now);
                    self.check(&[], None, &observed);
                }
                Step::Exit => {
                    if self.model.exited {
                        return;
                    }
                    self.reach("exit");
                    let held: Vec<KeyId> = (0..self.model.keys.len())
                        .filter(|&index| self.model.keys[index].delivered.is_none())
                        .map(KeyId)
                        .collect();
                    if !held.is_empty() {
                        self.reach("exit with a key in line");
                    }
                    let observed = self.core.exit(now);
                    let trail = self.trail();
                    assert_eq!(
                        observed.passed, held,
                        "exit delivers every key still held, in press order: {trail}"
                    );
                    self.model.delivered(&observed.passed, now, &trail);
                    self.model.exited = true;
                    self.check(&[], None, &Observed::default());
                }
            }
        }

        fn before_read(&mut self, ago: u64, now: u64) {
            let Some((index, node)) = (0..self.model.keys.len())
                .find_map(|index| self.core.reading(KeyId(index)).map(|node| (index, node)))
            else {
                return;
            };
            let read = self.editor.read(node, now, (ago, 0), false);
            self.model.before_read(index, &read);
            let observed = self.core.before_read(KeyId(index), &read, now);
            self.check(&[], None, &observed);
        }

        fn handle(&mut self, now: u64) {
            let Some(index) = self
                .model
                .keys
                .iter()
                .position(|key| key.delivered.is_some() && !key.handled)
            else {
                return;
            };
            let (kind, moves) = (self.model.keys[index].kind, self.model.keys[index].moves);
            let (removed, changed) = self.editor.apply(index, kind, moves, now);
            let key = &mut self.model.keys[index];
            key.handled = true;
            key.removed = removed;
            key.applied = changed.then_some(now);
        }

        fn evidence(&mut self, newest: bool, ago: u64, now: u64) {
            let mut open = (0..self.model.keys.len()).filter(|&index| {
                let key = &self.model.keys[index];
                key.applied.is_some() && !key.dropped && self.core.watching(KeyId(index)).is_some()
            });
            let chosen = if newest {
                open.next_back()
            } else {
                open.next()
            };
            let Some(index) = chosen else {
                return;
            };
            let node = self.core.watching(KeyId(index)).expect("a watch");
            let floor = self.model.keys[index].applied.expect("applied");
            let read = self.editor.read(node, now, (ago, floor), true);
            let words = self.model.evidence(index, &read, now);
            if self.model.keys[index].later.is_some() {
                self.reach("an answer to a key superseded by a later one");
            }
            let observed = self.core.evidence(KeyId(index), &read, now);
            let expected: Vec<(usize, String)> = words.into_iter().collect();
            self.check(&expected, None, &observed);
        }

        fn time(&mut self, ms: u64) {
            self.now += ms;
            let now = self.now;
            let mut expected = Vec::new();
            let mut observed = Observed::default();
            for (index, bound) in self.bounds_due() {
                let node = self.core.watching(KeyId(index)).unwrap_or(FIELD);
                let read = self.editor.read(node, bound, (0, 0), false);
                let (more, delivered) = self.core.bound_passed(KeyId(index), &read, now);
                expected.extend(self.model.bound(index, &read, delivered));
                self.reach("a key's bound passed");
                observed.passed.extend(more.passed);
                observed.heard.extend(more.heard);
            }
            let more = self.core.time(now);
            observed.passed.extend(more.passed);
            observed.heard.extend(more.heard);
            self.check(&expected, None, &observed);
        }

        fn focus(&mut self, to: FocusTo, now: u64) {
            let model = &self.model;
            let allowed = !model.ended
                && !model.exited
                && match to {
                    FocusTo::Elsewhere | FocusTo::Window => model.text() && !model.queued_focus,
                    // The second field's container is entered from the first.
                    FocusTo::Waits => model.focus_node == Some(FIELD) && !model.queued_focus,
                    FocusTo::Back => !model.text() && !model.queued_focus,
                    FocusTo::Answer => model.queued_focus,
                };
            if !allowed {
                return;
            }
            if to == FocusTo::Waits {
                let read = self.editor.read(SECOND_FIELD, now, (0, 0), false);
                self.model.queued_focus = true;
                self.reach("a focus report waiting");
                let observed = self.core.focus(to, Some(&read), now);
                self.check(&[], None, &observed);
                return;
            }
            let reading: Vec<usize> = (0..self.model.keys.len())
                .filter(|&index| self.core.reading(KeyId(index)).is_some())
                .collect();
            let expected = self.model.focus_moves(now, &reading);
            self.editor.pending.clear();
            let node = match to {
                FocusTo::Back => Some(FIELD),
                FocusTo::Answer => Some(SECOND_FIELD),
                _ => None,
            };
            let read = node.map(|node| self.editor.read(node, now, (0, 0), false));
            if !expected.is_empty() {
                self.reach("a focus moving within a key's bound");
            }
            if self.model.queued_focus {
                self.reach("keys behind a focus report");
                self.model.queued_focus = false;
                for key in self
                    .model
                    .keys
                    .iter_mut()
                    .filter(|key| key.delivered.is_none())
                {
                    key.floor = now;
                }
            }
            self.model.last_field = self.model.focus_node.or(self.model.last_field);
            self.model.focus_node = node;
            self.model.caret = read.as_ref().map(|read| (read.began, read.at));
            if let Some(read) = &read {
                self.model.last_read = Some(read.clone());
            }
            let observed = self.core.focus(to, read.as_ref(), now);
            self.check(&expected, Some(to == FocusTo::Window), &observed);
        }

        fn case(steps: &[Step]) -> Self {
            let mut run = Self::new();
            for &step in steps {
                run.act(step);
            }
            run.act(Step::Time(160));
            run.act(Step::Time(160));
            let trail = run.trail();
            run.model.finish(&trail);
            run
        }
    }

    /// The states the generated cases must reach.
    const LISTED: [&str; 8] = [
        "a key's bound passed",
        "a focus moving within a key's bound",
        "keys behind a focus report",
        "a focus report waiting",
        "a report behind the focus",
        "an answer to a key superseded by a later one",
        "exit with a key in line",
        "an outpost ended",
    ];

    /// The report of a failing case, ready to paste into [`found`].
    fn found_report(steps: &[Step], again: &str) -> String {
        format!(
            "the smallest failing case, as a line for `found()` in crates/verbatim-core/src/queries/generated.rs:\n        vec!{steps:?},\n\
             the run that found it again: {again}"
        )
    }

    /// Runs the generated cases `config` draws, each twice, shrinking a
    /// failure to its smallest; what the cases reached.
    pub(super) fn run(config: Config, again: &str) -> BTreeMap<&'static str, u32> {
        let reached = std::cell::RefCell::new(BTreeMap::new());
        let mut runner = TestRunner::new(config);
        let strategy = proptest::collection::vec(step(), 1..30);
        let result = runner.run(&strategy, |steps| {
            let first = Run::case(&steps);
            let second = Run::case(&steps);
            assert_eq!(
                first.core.log, second.core.log,
                "two runs of one case differ"
            );
            let mut reached = reached.borrow_mut();
            for (state, count) in first.reached {
                *reached.entry(state).or_default() += count;
            }
            Ok(())
        });
        match result {
            Ok(()) => reached.into_inner(),
            Err(TestError::Fail(reason, steps)) => {
                panic!("{reason}\n{}", found_report(&steps, again))
            }
            Err(error) => panic!("{error}"),
        }
    }

    /// The gate's seed and case count.
    const SEED: u64 = 20_261_109;
    const CASES: u32 = 400;

    /// Generated cases keep the held keys' rules after every step, against
    /// the model, and reach every listed state.
    #[test]
    fn generated_cases_keep_the_key_line_rules() {
        let reached = run(
            super::super::explore::config(SEED, CASES),
            "cargo test -p verbatim-core --lib -- --exact queries::generated::key_line::generated_cases_keep_the_key_line_rules",
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

    /// The known defects' cases, each a case the generated test found or a
    /// sequence from the roadmap's list, kept explicit.
    fn found() -> Vec<(&'static str, Vec<Step>)> {
        use Step::{BeforeRead, Evidence, Exit, Handle, Press, Time};
        let right = |moves| Press {
            kind: Kind::Right,
            moves,
        };
        vec![
            (
                "a key that moves nothing is silent",
                vec![right(false), BeforeRead { ago: 0 }, Time(110)],
            ),
            (
                "a superseded key's answer leaves Core's caret stale",
                vec![
                    right(true),
                    right(false),
                    BeforeRead { ago: 120 },
                    Handle,
                    BeforeRead { ago: 3 },
                    Evidence {
                        newest: false,
                        ago: 0,
                    },
                ],
            ),
            (
                "a key held at exit is not delivered",
                vec![right(true), Exit],
            ),
            (
                "a key behind a waiting focus report waits out its press's bound again",
                vec![
                    right(false),
                    Step::Focus(FocusTo::Waits),
                    BeforeRead { ago: 0 },
                    right(false),
                    Time(90),
                    Step::Focus(FocusTo::Answer),
                    Time(160),
                ],
            ),
        ]
    }

    /// Every found case keeps the rules: each fails today, on its defect,
    /// and all are named when any does.
    #[test]
    fn found_cases_keep_the_key_line_rules() {
        let failing: Vec<(&str, String)> = found()
            .into_iter()
            .filter_map(|(name, steps)| {
                std::panic::catch_unwind(|| {
                    Run::case(&steps);
                })
                .err()
                .map(|failure| {
                    let reason = failure
                        .downcast_ref::<String>()
                        .cloned()
                        .unwrap_or_default();
                    (name, reason)
                })
            })
            .collect();
        assert_eq!(failing, Vec::<(&str, String)>::new(), "the found cases");
    }
}
