//! The key line (`docs/design/caret-selection-rework.md`): each caret key's
//! one lifecycle value, from the hook holding it to its one result, with
//! its one deadline. A held key is read before it is passed on; a key the
//! hook passed sets its node's note and is judged from the node's last read.
//! Core alone decides what is evidence of a key, from the comparisons the
//! outpost reports, and chooses every word (`crate::editing`).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    CaretKey, CaretMotion, CaretRead, CaretReply, CaretReport, Change, Effect, Input, KeyFetch,
    NodeId, NoteRequest, QueryId, ReadFacts, ReadPurpose, ReviewCommand, SelectionChange,
    TextChunk, TextMovement, TextOp, TextPoint, TextPosition, TextRead, TextReply, TextUnit,
    TraceId, UtteranceSegment, had_effect,
};

use crate::driver::{Core, Cx, Signal, Wait, Woke};
use crate::editing::{self, is_grid, text_focus};
use crate::queries::{Asking, Ending, Then};
use crate::state::SrState;

/// The key's bound: 100 ms after Core passed it on, NVDA's
/// `caretMoveTimeoutMs`, and the wait of whatever is behind it.
pub(crate) const KEY_BOUND_MS: u64 = 100;

/// The longest a held key waits for the read of the caret before it,
/// counted from when the read is asked: past it the key is passed on and
/// judged from the node's last read.
pub(crate) const HELD_READ_BOUND_MS: u64 = 150;

/// A caret key the keyboard hook held or passed, with what names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HeldKey {
    /// The key's trace, which also names it to the shell that passes it on.
    pub(crate) trace_id: TraceId,
    pub(crate) key: CaretKey,
    /// When the hook saw it, on the outposts' clock.
    pub(crate) pressed_at_ms: u64,
    /// The focus with text when it was pressed: judged at its pass, it says
    /// nothing if that is no longer the focus (Dickson, 2026-11-09).
    pub(crate) pressed_on: Option<NodeId>,
}

/// A key waiting behind the current key, in press order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum WaitingKey {
    /// A caret key held back too, read in its turn.
    Held(HeldKey),
    /// A key the hook deferred behind a held key, which reaches the
    /// application after it: a caret key it passed, text typed, or a key
    /// that clears a terminal's line. Handled in its turn, as the input it
    /// came as.
    Later(Box<Input>),
    /// A review command or a script (`Input::Command`, `Input::Script`).
    Command(Box<Input>),
}

/// The key line: nothing, or the current key with the keys behind it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum KeyLine {
    #[default]
    Idle,
    Busy(Box<Busy>),
}

/// A line with a current key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Busy {
    current: CurrentKey,
    /// Behind the current key, in press order.
    waiting: VecDeque<WaitingKey>,
    /// Selection changes held for the next words.
    heard: Vec<UtteranceSegment>,
}

/// What the current key is doing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum CurrentKey {
    /// Its before read is running.
    Reading { node: NodeId, key: HeldKey },
    /// Passed on, within its bound.
    Watched(Watched),
    /// A text field's key past its bound: its final read is asked, and what
    /// was behind it has gone ahead.
    FinalReading { field: Box<FieldKey> },
    /// A sentence key past its bound with no evidence: Verbatim moves the
    /// caret by sentence, each step reusing the before read's 150 ms.
    MovingBySentence {
        passed: PassedKey,
        /// Its final read is out, whose evidence a report may still be.
        final_read: bool,
    },
    /// A review or object-navigation press whose read is running: it stays
    /// current until the answer to its last read, so it cannot end before
    /// its move. Anything behind it waits, with a time bound on the press
    /// ([`HELD_READ_BOUND_MS`]).
    Review(Press),
}

/// A key passed on, within its bound. A text field's key and a terminal's
/// are different after the release (Dickson, 2026-11-09): a terminal's has
/// no before unit and no final read, its reading rules speaking for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Watched {
    Field(Box<FieldKey>),
    Terminal(PassedKey),
}

/// What every key passed on has.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PassedKey {
    node: NodeId,
    key: HeldKey,
    /// When it was passed on, in microseconds.
    passed_at_us: u64,
    /// When a later key was pressed, once one has been.
    later_key_at_us: Option<u64>,
}

/// A review or object-navigation command the line runs
/// (`docs/design/m42-parity.md`, "Build two"): the line's record that the
/// press's function is reading (`crate::press`), which holds the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Press {
    /// The node its read is of, which the state holds.
    node: NodeId,
    /// The key's trace, which names the press.
    trace_id: TraceId,
}

/// A press of a review command: the command and how many times it was
/// repeated. What the press's reads answer is run against it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewPress {
    /// The key's trace, which also names it to the line.
    pub(crate) trace_id: TraceId,
    pub(crate) command: ReviewCommand,
    pub(crate) repeat: u8,
}

/// A key passed on in a text field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FieldKey {
    passed: PassedKey,
    before: Before,
    /// A report with no unit was its evidence: its words wait for the final
    /// read's unit.
    awaiting_unit: bool,
}

/// What the key's before read found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Before {
    Read {
        unit: TextChunk,
        line: TextChunk,
        removed: Option<String>,
    },
    NotRead,
}

impl Watched {
    /// What the key has whichever node it is on.
    fn passed(&self) -> &PassedKey {
        match self {
            Self::Field(field) => &field.passed,
            Self::Terminal(passed) => passed,
        }
    }
}

/// Whether `state`'s focus is a terminal, from its role, which Core alone
/// judges because the outpost's own test can disagree with the role.
fn focus_is_terminal(state: &SrState) -> bool {
    state
        .focus
        .object()
        .is_some_and(|focus| is_grid(focus.snapshot.role))
}

/// `passed` as a key of `state`'s focus: `before` is kept only for a text
/// field.
fn watched_on_focus(state: &SrState, passed: PassedKey, before: Before) -> Watched {
    if focus_is_terminal(state) {
        Watched::Terminal(passed)
    } else {
        Watched::Field(Box::new(FieldKey {
            passed,
            before,
            awaiting_unit: false,
        }))
    }
}

/// What the reads for a key of `key`'s motion fetch.
fn fetch_of(key: CaretKey) -> KeyFetch {
    KeyFetch {
        unit: key.motion.unit(),
        motion: key.motion,
    }
}

/// The read of a terminal asked at a key's pass, in a terminal other than
/// the console host, whose caret poll covers its reads.
fn terminal_read_now(state: &SrState, node: NodeId, trace_id: TraceId) -> Option<Effect> {
    let console = state
        .foreground
        .window()
        .is_some_and(|window| window.console);
    let terminal = match state.current() {
        Some(CurrentKey::Watched(Watched::Terminal(_))) => true,
        Some(
            CurrentKey::Watched(Watched::Field(_))
            | CurrentKey::Reading { .. }
            | CurrentKey::FinalReading { .. }
            | CurrentKey::MovingBySentence { .. }
            | CurrentKey::Review(_),
        )
        | None => false,
    };
    (terminal && !console).then(|| for_key(trace_id, Effect::ReadTerminal(node)))
}

/// `effect` carried under the key `trace_id`'s trace.
fn for_key(trace_id: TraceId, effect: Effect) -> Effect {
    Effect::ForKey(trace_id, Box::new(effect))
}

/// `effects`, each carried under `trace`'s key.
fn under(trace_id: Option<TraceId>, effects: Vec<Effect>) -> Vec<Effect> {
    match trace_id {
        Some(trace_id) => effects
            .into_iter()
            .map(|effect| match effect {
                carried @ Effect::ForKey(..) => carried,
                effect => for_key(trace_id, effect),
            })
            .collect(),
        None => effects,
    }
}

impl SrState {
    /// The current key.
    fn current(&self) -> Option<&CurrentKey> {
        match &self.key_line {
            KeyLine::Idle => None,
            KeyLine::Busy(busy) => Some(&busy.current),
        }
    }

    /// The current key and what is behind it, to change.
    fn busy_mut(&mut self) -> Option<&mut Busy> {
        match &mut self.key_line {
            KeyLine::Idle => None,
            KeyLine::Busy(busy) => Some(busy),
        }
    }

    /// Whether a key or command is in the line: Verbatim is not idle while
    /// one is.
    #[must_use]
    pub fn holds_inputs_back(&self) -> bool {
        matches!(self.key_line, KeyLine::Busy(_))
    }

    /// The current key's node, which Core holds.
    pub(crate) fn key_line_node(&self) -> Option<NodeId> {
        match self.current()? {
            CurrentKey::Reading { node, .. } => Some(*node),
            CurrentKey::Review(press) => Some(press.node),
            passed_on @ (CurrentKey::Watched(_)
            | CurrentKey::FinalReading { .. }
            | CurrentKey::MovingBySentence { .. }) => passed_on.passed().map(|passed| passed.node),
        }
    }
}

impl CurrentKey {
    /// What the key has once it has been passed on, in its bound or past it.
    fn passed(&self) -> Option<&PassedKey> {
        match self {
            Self::Reading { .. } | Self::Review(_) => None,
            Self::Watched(watched) => Some(watched.passed()),
            Self::FinalReading { field, .. } => Some(&field.passed),
            Self::MovingBySentence { passed, .. } => Some(passed),
        }
    }

    /// [`Self::passed`], to change.
    fn passed_mut(&mut self) -> Option<&mut PassedKey> {
        match self {
            Self::Reading { .. } | Self::Review(_) => None,
            Self::Watched(watched) => Some(match watched {
                Watched::Field(field) => &mut field.passed,
                Watched::Terminal(passed) => passed,
            }),
            Self::FinalReading { field, .. } => Some(&mut field.passed),
            Self::MovingBySentence { passed, .. } => Some(passed),
        }
    }

    /// The trace that names the key.
    fn trace_id(&self) -> Option<TraceId> {
        match self {
            Self::Reading { key, .. } => Some(key.trace_id),
            Self::Review(press) => Some(press.trace_id),
            passed_on @ (Self::Watched(_)
            | Self::FinalReading { .. }
            | Self::MovingBySentence { .. }) => {
                passed_on.passed().map(|passed| passed.key.trace_id)
            }
        }
    }

    /// Whether a held key or a command that arrives now joins the line. A
    /// key in its final read has let what was behind it go ahead.
    fn holds_arrivals(&self) -> bool {
        match self {
            Self::Reading { .. }
            | Self::Watched(_)
            | Self::MovingBySentence { .. }
            | Self::Review(_) => true,
            Self::FinalReading { .. } => false,
        }
    }

    /// The key once passed on, as evidence judges it, in its bound or past
    /// it. A sentence move's evidence is judged apart
    /// ([`sentence_key_at_final_read`]).
    fn watched(&self) -> Option<Watched> {
        match self {
            Self::Reading { .. } | Self::MovingBySentence { .. } | Self::Review(_) => None,
            Self::Watched(watched) => Some(watched.clone()),
            Self::FinalReading { field, .. } => Some(Watched::Field(field.clone())),
        }
    }
}

/// Makes `current` the current key, keeping what is behind the line.
fn set_current(state: &mut SrState, current: CurrentKey) {
    match &mut state.key_line {
        KeyLine::Busy(busy) => busy.current = current,
        KeyLine::Idle => {
            state.key_line = KeyLine::Busy(Box::new(Busy {
                current,
                waiting: VecDeque::new(),
                heard: Vec::new(),
            }));
        }
    }
}

/// What a key does on its way to the application, whatever became of its
/// read: say-all stops, the typed word is forgotten, and the review cursor
/// follows the caret (`crate::editing`).
fn key_taking_effect(state: &mut SrState, key: CaretKey, node: NodeId) -> Vec<Effect> {
    let effects = crate::say_all::stop(state);
    if !editing::selects(key) && key.motion != CaretMotion::Other {
        state.typed_word.clear();
    }
    editing::follow_caret(state, node);
    effects
}

/// Starts `held`: its before read is asked, bounded by
/// [`HELD_READ_BOUND_MS`], or it is passed on at once when the focus has no
/// caret to read.
fn start_held(state: &mut Core, held: HeldKey) -> Vec<Effect> {
    let Some(node) = text_focus(state) else {
        let mut effects = crate::say_all::stop(state);
        if !editing::selects(held.key) && held.key.motion != CaretMotion::Other {
            state.typed_word.clear();
        }
        effects.push(Effect::PassHeldKey(held.trace_id));
        return effects;
    };
    crate::driver::spawn(state, move |cx| {
        let key = Key {
            cx,
            trace_id: held.trace_id,
        };
        held_caret_key(key, held, node)
    })
}

/// Asks for the read of the caret before `held`, on `node`, which becomes
/// the current key, bounded by [`HELD_READ_BOUND_MS`].
fn read_before(state: &mut SrState, held: HeldKey, node: NodeId) -> (QueryId, Vec<Effect>) {
    let heard = replaced_key_heard(state, held.trace_id);
    let read = CaretRead {
        fetch: fetch_of(held.key),
        purpose: if focus_is_terminal(state) {
            ReadPurpose::BeforeTerminalKey
        } else {
            ReadPurpose::BeforeFieldKey
        },
    };
    let (query, effect) = state.queries.ask(
        Asking::Text(node, TextOp::ReadCaret(read)),
        Then::Key { node },
    );
    set_current(state, CurrentKey::Reading { node, key: held });
    // The deadline first, so a read the shell answers at once ends it.
    let mut effects = heard;
    effects.push(Effect::BoundHeldKey(Some((
        held.trace_id,
        HELD_READ_BOUND_MS,
    ))));
    effects.push(effect);
    (query, effects)
}

/// A caret key the keyboard hook held back: it starts its turn at once, or
/// waits behind the keys in the line.
pub(crate) fn held_key(
    state: &mut Core,
    trace_id: TraceId,
    key: CaretKey,
    pressed_at_ms: u64,
) -> Vec<Effect> {
    let held = HeldKey {
        trace_id,
        key,
        pressed_at_ms,
        pressed_on: text_focus(state),
    };
    if state.current().is_some_and(CurrentKey::holds_arrivals) {
        mark_later(state, pressed_at_ms.saturating_mul(1_000));
        return join(state, WaitingKey::Held(held));
    }
    if is_sentence_key(key) && state.focus.input_queued() {
        return under(Some(trace_id), pass_sentence_key(state, held));
    }
    under(Some(trace_id), start_held(state, held))
}

/// A key passed to the application that is not held, in its turn (a
/// caret key judged from the node's last read, text typed, or a key that
/// clears a terminal's line): one that finds the line holding waits behind
/// it, or runs at once while nothing is behind it.
pub(crate) fn key_arrived(state: &mut Core, input: &Input) -> Vec<Effect> {
    let as_caret_key = chord_in_terminal(state, input);
    let input = as_caret_key.as_ref().unwrap_or(input);
    let joins = match state.current() {
        None => false,
        Some(CurrentKey::Reading { .. } | CurrentKey::Review(_)) => true,
        Some(CurrentKey::MovingBySentence { .. }) => {
            if let Some(at_us) = input_pressed_at_us(input) {
                mark_later(state, at_us);
            }
            true
        }
        Some(CurrentKey::Watched(_) | CurrentKey::FinalReading { .. }) => state
            .busy_mut()
            .is_some_and(|busy| !busy.waiting.is_empty()),
    };
    if joins {
        return join(state, WaitingKey::Later(Box::new(input.clone())));
    }
    crate::reduce::key_input(state, input)
}

/// `input` as the key outside the caret table it is in a terminal, when it
/// is an editing chord pressed in one: a terminal keeps its handling of the
/// chords (Dickson, 2026-10-25 and 2026-10-29).
fn chord_in_terminal(state: &SrState, input: &Input) -> Option<Input> {
    let Input::EditingChord {
        trace_id,
        key_seq,
        pressed_at_ms,
    } = input
    else {
        return None;
    };
    focus_is_terminal(state).then_some(Input::CaretKey {
        trace_id: *trace_id,
        key_seq: *key_seq,
        key: CaretKey {
            motion: CaretMotion::Other,
            select: false,
        },
        pressed_at_ms: *pressed_at_ms,
    })
}

/// A review command or script: one that finds a key in the line that still
/// holds it waits behind it, and any other runs now.
pub(crate) fn command_arrived(state: &mut Core, input: &Input) -> Vec<Effect> {
    if !state.current().is_some_and(CurrentKey::holds_arrivals) {
        return crate::reduce::command_input(state, input);
    }
    if let Some(at_ms) = command_time(input) {
        mark_later(state, at_ms.saturating_mul(1_000));
    }
    join(state, WaitingKey::Command(Box::new(input.clone())))
}

/// `item` joins the line behind the current key. The first entry behind a
/// press arms the press's bound, so whatever waits behind it waits at most
/// [`HELD_READ_BOUND_MS`].
fn join(state: &mut SrState, item: WaitingKey) -> Vec<Effect> {
    let Some(busy) = state.busy_mut() else {
        return Vec::new();
    };
    let arms = busy.waiting.is_empty();
    busy.waiting.push_back(item);
    match &busy.current {
        CurrentKey::Review(press) if arms => vec![bound_of(press.trace_id)],
        _ => Vec::new(),
    }
}

/// The time bound on a press with something behind it.
fn bound_of(trace_id: TraceId) -> Effect {
    Effect::BoundHeldKey(Some((trace_id, HELD_READ_BOUND_MS)))
}

/// When a command or script was pressed.
fn command_time(input: &Input) -> Option<u64> {
    match input {
        Input::Command { pressed_at_ms, .. } | Input::Script { pressed_at_ms, .. } => {
            Some(*pressed_at_ms)
        }
        _ => None,
    }
}

/// Marks a later key at `at_us` on a watched key not yet marked.
pub(crate) fn mark_later(state: &mut SrState, at_us: u64) {
    if let Some(passed) = state.busy_mut().and_then(|busy| busy.current.passed_mut()) {
        passed.later_key_at_us.get_or_insert(at_us);
    }
}

/// A caret key the hook passed, in its turn: with a focus that has text it
/// becomes the current key, judged from the node's last read. Any other
/// does nothing but what a key always does.
pub(crate) fn unheld_caret_key(
    state: &mut Core,
    trace_id: TraceId,
    key: CaretKey,
    pressed_at_ms: u64,
    bound_ms: u64,
) -> Vec<Effect> {
    let Some(node) = text_focus(state) else {
        let effects = crate::say_all::stop(state);
        if !editing::selects(key) && key.motion != CaretMotion::Other {
            state.typed_word.clear();
        }
        return effects;
    };
    let held = HeldKey {
        trace_id,
        key,
        pressed_at_ms,
        pressed_on: Some(node),
    };
    let mut effects = watch_from_last_read(
        state,
        held,
        node,
        (pressed_at_ms.saturating_mul(1_000), bound_ms, true),
    );
    effects.extend(pass_rule(state));
    effects.extend(crate::driver::spawn(state, move |cx| async move {
        let _ = passed_key(&Key { cx, trace_id }).await;
    }));
    effects
}

/// `held` becomes the current key, judged from the node's last read:
/// `note` is sent first when its request has not set the node's note, so it
/// reaches the outpost before the key reaches the application; the bound
/// is `bound_ms` from now.
fn watch_from_last_read(
    state: &mut SrState,
    held: HeldKey,
    node: NodeId,
    (passed_at_us, bound_ms, sends_note): (u64, u64, bool),
) -> Vec<Effect> {
    let mut effects = replaced_key_heard(state, held.trace_id);
    effects.extend(key_taking_effect(state, held.key, node));
    effects.push(Effect::BoundHeldKey(Some((held.trace_id, bound_ms))));
    if sends_note {
        let fetch = fetch_of(held.key);
        let request = if focus_is_terminal(state) {
            NoteRequest::Terminal { node, fetch }
        } else {
            NoteRequest::Field { node, fetch }
        };
        effects.push(for_key(held.trace_id, Effect::KeyNote(request)));
    }
    let passed = PassedKey {
        node,
        key: held,
        passed_at_us,
        later_key_at_us: None,
    };
    let watched = watched_on_focus(state, passed, Before::NotRead);
    set_current(state, CurrentKey::Watched(watched));
    effects
}

/// The rule at a key's pass: typed keys at the front of what waits run at
/// once, as NVDA's typed keys are not held behind a script, stopping at the
/// first command or held key, and anything left behind it marks a later
/// key at the time it was pressed.
fn pass_rule(state: &mut Core) -> Vec<Effect> {
    let mut effects = Vec::new();
    loop {
        let Some(busy) = state.busy_mut() else {
            return effects;
        };
        // A typed key sent with the one passed on reached the program then.
        let passed_at_ms = busy
            .current
            .passed()
            .map(|passed| passed.passed_at_us.div_ceil(1_000));
        match busy.waiting.front() {
            Some(WaitingKey::Later(input)) if !is_caret_key(input) => {
                let input = pressed_no_earlier((**input).clone(), passed_at_ms);
                busy.waiting.pop_front();
                let own = crate::reduce::key_input(state, &input);
                effects.extend(under(crate::reduce::key_trace(&input), own));
            }
            _ => break,
        }
    }
    let queued = state.focus.input_queued();
    let queued_at = state.focus.queued_inputs().find_map(input_pressed_at_us);
    if let Some(busy) = state.busy_mut()
        && (queued || !busy.waiting.is_empty())
    {
        let waiting_at = busy.waiting.front().and_then(waiting_pressed_at_us);
        if let Some(passed) = busy.current.passed_mut() {
            let at = waiting_at.into_iter().chain(queued_at).min();
            passed
                .later_key_at_us
                .get_or_insert(at.unwrap_or(passed.passed_at_us));
        }
    }
    effects
}

/// When what waits as `waiting` was pressed, in microseconds.
fn waiting_pressed_at_us(waiting: &WaitingKey) -> Option<u64> {
    match waiting {
        WaitingKey::Held(held) => Some(held.pressed_at_ms.saturating_mul(1_000)),
        WaitingKey::Later(input) | WaitingKey::Command(input) => input_pressed_at_us(input),
    }
}

/// When the key or command `input` was pressed, in microseconds.
fn input_pressed_at_us(input: &Input) -> Option<u64> {
    match input {
        Input::HeldKey { pressed_at_ms, .. }
        | Input::CaretKey { pressed_at_ms, .. }
        | Input::EditingChord { pressed_at_ms, .. }
        | Input::Command { pressed_at_ms, .. }
        | Input::Script { pressed_at_ms, .. } => Some(pressed_at_ms.saturating_mul(1_000)),
        _ => None,
    }
}

/// Whether `input` is a caret key the hook passed.
fn is_caret_key(input: &Input) -> bool {
    matches!(input, Input::CaretKey { .. })
}

/// The key `state`'s line runs `input` as: the trace of the key when it
/// starts a key of its own, which then takes what waits behind it.
fn pressed_no_earlier(input: Input, at_ms: Option<u64>) -> Input {
    match at_ms {
        Some(at_ms) => crate::reduce::pressed_no_earlier(input, at_ms),
        None => input,
    }
}

/// Handles what waits behind a key, in order, until a key becomes current.
/// A key in its final read stays current until a held key replaces it. A
/// caret key the hook passed starts its bound from when it reached Core,
/// firing at once when that has passed.
fn run_waiting(
    state: &mut Core,
    mut waiting: VecDeque<WaitingKey>,
    passed_at_ms: Option<u64>,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    while let Some(item) = waiting.pop_front() {
        let (trace, own, starts_key) = match item {
            WaitingKey::Held(held)
                if is_sentence_key(held.key)
                    && (!waiting.is_empty() || state.focus.input_queued()) =>
            {
                (Some(held.trace_id), pass_sentence_key(state, held), false)
            }
            WaitingKey::Held(held) => (Some(held.trace_id), start_held(state, held), true),
            WaitingKey::Later(input) => {
                let bound = passed_at_ms
                    .zip(caret_key_pressed_at(&input))
                    .map(|(at, pressed)| KEY_BOUND_MS.saturating_sub(at.saturating_sub(pressed)));
                let input = pressed_no_earlier(*input, passed_at_ms);
                let starts_key = is_caret_key(&input);
                let trace = crate::reduce::key_trace(&input);
                let own = match (bound, &input) {
                    (
                        Some(bound),
                        Input::CaretKey {
                            trace_id,
                            key,
                            pressed_at_ms,
                            ..
                        },
                    ) => unheld_caret_key(state, *trace_id, *key, *pressed_at_ms, bound),
                    _ => crate::reduce::key_input(state, &input),
                };
                (trace, own, starts_key)
            }
            WaitingKey::Command(input) => {
                let trace = input.trace_id();
                (trace, crate::reduce::command_input(state, &input), true)
            }
        };
        // Armed before the read, so a read the shell answers at once ends
        // it.
        if let Some(Some(trace_id)) = starts_key.then(|| press_with_entries(state, trace, &waiting))
        {
            effects.push(bound_of(trace_id));
        }
        effects.extend(under(trace, own));
        if !starts_key {
            continue;
        }
        if let Some(runs) = became_current(state, trace) {
            if let Some(busy) = state.busy_mut() {
                busy.waiting.append(&mut waiting);
            }
            if runs == RunsAtPass::Yes {
                effects.extend(pass_rule(state));
            }
            break;
        }
    }
    effects
}

/// Whether the key `trace` has become current, and if so whether it has
/// reached its pass.
fn became_current(state: &SrState, trace: Option<TraceId>) -> Option<RunsAtPass> {
    match state.current() {
        Some(CurrentKey::Reading { key, .. }) if Some(key.trace_id) == trace => {
            Some(RunsAtPass::No)
        }
        Some(CurrentKey::Watched(watched)) if Some(watched.passed().key.trace_id) == trace => {
            Some(RunsAtPass::Yes)
        }
        Some(CurrentKey::Review(press)) if Some(press.trace_id) == trace => Some(RunsAtPass::No),
        Some(
            CurrentKey::Reading { .. }
            | CurrentKey::Watched(_)
            | CurrentKey::FinalReading { .. }
            | CurrentKey::MovingBySentence { .. }
            | CurrentKey::Review(_),
        )
        | None => None,
    }
}

/// The trace of the press `trace` has just become current, when entries
/// wait behind it.
fn press_with_entries(
    state: &SrState,
    trace: Option<TraceId>,
    waiting: &VecDeque<WaitingKey>,
) -> Option<TraceId> {
    let Some(CurrentKey::Review(press)) = state.current() else {
        return None;
    };
    (Some(press.trace_id) == trace && !waiting.is_empty()).then_some(press.trace_id)
}

/// Whether the key that has just become current has reached its pass, so
/// that the pass rule runs now.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RunsAtPass {
    Yes,
    No,
}

/// Whether `key` moves by sentence.
fn is_sentence_key(key: CaretKey) -> bool {
    matches!(
        key.motion,
        CaretMotion::PreviousSentence | CaretMotion::NextSentence
    )
}

/// A sentence key with others already waiting behind it is only passed on,
/// as NVDA's script does when another waits
/// (`docs/design/caret-selection-rework.md`, "Moving by sentence").
fn pass_sentence_key(state: &mut SrState, held: HeldKey) -> Vec<Effect> {
    let mut effects = crate::say_all::stop(state);
    state.typed_word.clear();
    effects.push(Effect::PassHeldKey(held.trace_id));
    effects
}

/// Advances the line: runs what waits, a key still current staying until a
/// held key replaces it.
fn advance(state: &mut Core, at_ms: Option<u64>) -> Vec<Effect> {
    let Some(busy) = state.busy_mut() else {
        return Vec::new();
    };
    let waiting = std::mem::take(&mut busy.waiting);
    run_waiting(state, waiting, at_ms)
}

/// Ends the current key: clears its deadline, says what is left in `heard`,
/// and advances. With no key left
/// the line is `Idle`.
fn end_current(state: &mut Core, trace_id: TraceId, passed_at_ms: Option<u64>) -> Vec<Effect> {
    let KeyLine::Busy(busy) = std::mem::take(&mut state.key_line) else {
        return Vec::new();
    };
    let Busy { waiting, heard, .. } = *busy;
    let mut effects = vec![Effect::BoundHeldKey(None)];
    // Said in the order they happened, before anything that waited behind
    // the key (Dickson, 2026-11-09).
    effects.extend(editing::speak_each(trace_id, heard));
    // The key's function, when the reducer and not it ended the key.
    effects.extend(crate::driver::signal(state, Signal::KeyEnded, 0));
    effects.extend(run_waiting(state, waiting, passed_at_ms));
    effects
}

/// A list `controller` controls moved its selection, as a search box's
/// suggestions do for Down Arrow: a key passed on from `controller` ends
/// with no words of its own, as NVDA's line for the key is cut off by the
/// selection's report (`docs/parity/changes-and-selection.md`).
pub(crate) fn controlled_selection(state: &mut Core, controller: NodeId) -> Vec<Effect> {
    let passed = match state.current() {
        Some(CurrentKey::Watched(watched)) => watched.passed(),
        Some(CurrentKey::FinalReading { field, .. }) => &field.passed,
        Some(
            CurrentKey::Reading { .. }
            | CurrentKey::MovingBySentence { .. }
            | CurrentKey::Review(_),
        )
        | None => {
            return Vec::new();
        }
    };
    if passed.node != controller {
        return Vec::new();
    }
    let (trace_id, passed_ms) = (passed.key.trace_id, passed.passed_at_us.div_ceil(1_000));
    end_current(state, trace_id, Some(passed_ms))
}

/// The selection changes held for a key that `trace_id`'s key replaces,
/// said as that key ends (Dickson, 2026-11-09).
fn replaced_key_heard(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let Some(busy) = state.busy_mut() else {
        return Vec::new();
    };
    if busy.current.trace_id() == Some(trace_id) {
        return Vec::new();
    }
    editing::speak_each(trace_id, std::mem::take(&mut busy.heard))
}

/// The press time of a caret key the hook passed.
fn caret_key_pressed_at(input: &Input) -> Option<u64> {
    match input {
        Input::CaretKey { pressed_at_ms, .. } => Some(*pressed_at_ms),
        _ => None,
    }
}

/// A key's read answered: the key's function goes on with it while it
/// waits for it, and a reply no longer waited for, of a read given up or of
/// a key that has ended, is an ordinary report.
pub(crate) fn request_answered(
    state: &mut Core,
    trace_id: TraceId,
    (query, node): (QueryId, NodeId),
    reply: TextReply,
    at_us: u64,
) -> Vec<Effect> {
    if crate::driver::awaits(state, query) {
        return crate::driver::answered(state, query, Ending::Reply(reply), trace_id, at_us);
    }
    late_read(state, trace_id, node, reply)
}

/// `reply`, a key's read no longer the current key's: an ordinary report.
fn late_read(state: &mut Core, trace_id: TraceId, node: NodeId, reply: TextReply) -> Vec<Effect> {
    let TextReply::Caret(read) = reply else {
        return Vec::new();
    };
    report_read(state, trace_id, node, *read)
}

/// A read of `node` with its facts, a query's reply no longer current or a
/// report: the key's evidence if it is, else an ordinary report.
fn report_read(state: &mut Core, trace_id: TraceId, node: NodeId, read: CaretReply) -> Vec<Effect> {
    let (changes, general) = editing::changes_of_reply(&read);
    if let Some(watched) = current_watched(state, node)
        && let Some(evidence) = evidence_of(watched.passed(), &read.facts)
    {
        return result(state, trace_id, &watched, &read, Some(evidence));
    }
    if let Some(passed) = sentence_key_at_final_read(state, node)
        && let Some(evidence) = evidence_of(&passed, &read.facts)
    {
        return sentence_moved_by_application(state, trace_id, &passed, &read, evidence);
    }
    editing::update_caret(state, node, read.caret);
    editing::selection_changed(state, trace_id, node, (&changes, general.as_general()))
}

/// The passed-on key whose node is `node`, if there is one.
fn current_watched(state: &SrState, node: NodeId) -> Option<Watched> {
    state
        .current()?
        .watched()
        .filter(|watched| watched.passed().node == node)
}

/// `facts` as the evidence of the key `passed`: the comparison, when the
/// read ended at or after the key was passed on and its comparison shows
/// the key's effect.
fn evidence_of<'a>(passed: &PassedKey, facts: &'a ReadFacts) -> Option<Evidence<'a>> {
    let Change::Compared {
        compared,
        previous_line,
        previous_selection,
        on_previous_line,
    } = &facts.change
    else {
        return None;
    };
    (facts.read_ended_at_us >= passed.passed_at_us && had_effect(*compared, passed.key.key.motion))
        .then_some(Evidence {
            previous_line,
            previous_selection: previous_selection.as_ref(),
            on_previous_line: *on_previous_line,
        })
}

/// What a report that is a key's evidence compared with.
#[derive(Clone, Copy)]
pub(crate) struct Evidence<'a> {
    /// The caret's line in the last read.
    pub(crate) previous_line: &'a TextChunk,
    /// The selection in the last read.
    pub(crate) previous_selection: Option<&'a verbatim_model::CaretSelection>,
    /// Whether the caret is on the line it was on.
    pub(crate) on_previous_line: bool,
}

/// A caret report of `node`, the key's evidence or an ordinary report.
pub(crate) fn caret_report(
    state: &mut Core,
    trace_id: TraceId,
    node: NodeId,
    caret: CaretReport,
    (changes, general): (&[SelectionChange], Option<editing::General<'_>>),
    facts: Option<&ReadFacts>,
) -> Vec<Effect> {
    if let Some(facts) = facts
        && let Some(watched) = current_watched(state, node)
        && let Some(evidence) = evidence_of(watched.passed(), facts)
    {
        let read = editing::report_of(caret, changes, general, facts);
        return result(state, trace_id, &watched, &read, Some(evidence));
    }
    if let Some(facts) = facts
        && let Some(passed) = sentence_key_at_final_read(state, node)
        && let Some(evidence) = evidence_of(&passed, facts)
    {
        let read = editing::report_of(caret, changes, general, facts);
        return sentence_moved_by_application(state, trace_id, &passed, &read, evidence);
    }
    editing::update_caret(state, node, caret);
    editing::selection_changed(state, trace_id, node, (changes, general))
}

/// A repeated focus on the current key's node that carries the key's
/// evidence, which takes the evidence row; `None` when it does not.
pub(crate) fn repeat_evidence(
    state: &mut Core,
    trace_id: TraceId,
    node: NodeId,
    text: Option<&verbatim_model::FocusText>,
) -> Option<Vec<Effect>> {
    let verbatim_model::FocusText::Caret {
        caret,
        changes,
        facts: Some(facts),
        ..
    } = text?
    else {
        return None;
    };
    let watched = current_watched(state, node)?;
    let evidence = evidence_of(watched.passed(), facts)?;
    let general = changes.content_changed.then_some(editing::General {
        selection: changes.new_selection.as_ref(),
        collapsed: caret.selection.is_none(),
    });
    let read = editing::report_of(caret.clone(), &changes.selection_changes, general, facts);
    Some(result(state, trace_id, &watched, &read, Some(evidence)))
}

/// The before read of the current key answered: the caret taken and the
/// key passed on, or, with nothing read or the focus moved on, the key
/// passed on saying nothing. The pass is `passed_at_us`, when Core passes it
/// on: a read that ended before that is not the key's.
fn before_read_answered(
    state: &mut Core,
    trace_id: TraceId,
    (node, held): (NodeId, HeldKey),
    reply: TextReply,
    passed_at_us: u64,
) -> Vec<Effect> {
    let TextReply::Caret(read) = reply else {
        // Nothing was read: the key is passed on, and says nothing.
        let mut effects = crate::say_all::stop(state);
        effects.push(Effect::PassHeldKey(held.trace_id));
        effects.extend(end_current(state, held.trace_id, None));
        return effects;
    };
    let read = *read;
    if !state.focus_matches(node) || held.pressed_on != Some(node) {
        // The focus moved on while the key was read or waited: its words
        // would follow the focus's. Its read is still a report of the focus
        // it read.
        let (changes, general) = editing::changes_of_reply(&read);
        editing::update_caret(state, node, read.caret);
        let mut effects =
            editing::selection_changed(state, trace_id, node, (&changes, general.as_general()));
        effects.extend(crate::say_all::stop(state));
        effects.push(Effect::PassHeldKey(held.trace_id));
        effects.extend(end_current(state, held.trace_id, None));
        return effects;
    }
    let (changes, general) = editing::changes_of_reply(&read);
    let before = editing::before_of_read(state, node, held.key.motion, &read);
    editing::update_caret(state, node, read.caret);
    let mut effects =
        editing::selection_changed(state, trace_id, node, (&changes, general.as_general()));
    let tk = crate::terminal::caret_key_kind(held.key);
    effects.extend(crate::terminal_model::key(
        state,
        tk,
        passed_at_us.div_ceil(1_000),
        held.trace_id,
    ));
    effects.extend(key_taking_effect(state, held.key, node));
    effects.push(Effect::BoundHeldKey(Some((held.trace_id, KEY_BOUND_MS))));
    let passed = PassedKey {
        node,
        key: held,
        passed_at_us,
        later_key_at_us: None,
    };
    let before = Before::Read {
        unit: before.unit,
        line: before.line,
        removed: before.removed,
    };
    let watched = watched_on_focus(state, passed, before);
    set_current(state, CurrentKey::Watched(watched));
    effects.push(for_key(held.trace_id, Effect::PassHeldKey(held.trace_id)));
    effects.extend(terminal_read_now(state, node, held.trace_id));
    effects.extend(pass_rule(state));
    effects
}

/// The line's one deadline has passed, at `at_us`: the wait it bounds is
/// over, for the key's function or the press's. Nothing for a key no
/// longer current, or one with no deadline.
pub(crate) fn deadline_passed(state: &mut Core, trace_id: TraceId, at_us: u64) -> Vec<Effect> {
    match state.current() {
        Some(key) if key.trace_id() != Some(trace_id) => Vec::new(),
        None | Some(CurrentKey::FinalReading { .. }) => Vec::new(),
        Some(CurrentKey::Review(_)) => crate::driver::signal(state, Signal::PressGivesWay, 0),
        Some(
            CurrentKey::Reading { .. }
            | CurrentKey::Watched(_)
            | CurrentKey::MovingBySentence { .. },
        ) => crate::driver::signal(state, Signal::KeyBound, at_us),
    }
}

/// A press that gives way withdraws its read `query`, whose answer,
/// whenever it comes, runs nothing: its bound has passed, or a focus moved
/// the navigator while it read. It then ends saying nothing, the review
/// cursor staying where it was.
pub(crate) fn withdraw_press_read(
    state: &mut SrState,
    trace_id: TraceId,
    query: QueryId,
) -> Vec<Effect> {
    let effects = state
        .queries
        .withdraw_kept(query)
        .into_iter()
        .map(|effect| for_key(trace_id, effect))
        .collect();
    state.queries.forget_query(query);
    effects
}

/// The press `trace_id` is over: when it read, and so is the current key,
/// it ends and the line moves on. Only a press with something behind it
/// was bound, so one with nothing behind it has no bound to clear.
pub(crate) fn press_ended(state: &mut Core, trace_id: TraceId) -> Vec<Effect> {
    if current_press(state).is_none_or(|press| press.trace_id != trace_id) {
        return Vec::new();
    }
    let bounded = state
        .busy_mut()
        .is_some_and(|busy| !busy.waiting.is_empty());
    let mut effects = end_current(state, trace_id, None);
    if !bounded {
        effects.retain(|effect| !matches!(effect, Effect::BoundHeldKey(None)));
    }
    effects
}

/// The press that is the current key.
fn current_press(state: &SrState) -> Option<Press> {
    match state.current() {
        Some(CurrentKey::Review(press)) => Some(*press),
        Some(
            CurrentKey::Reading { .. }
            | CurrentKey::Watched(_)
            | CurrentKey::FinalReading { .. }
            | CurrentKey::MovingBySentence { .. },
        )
        | None => None,
    }
}

/// The press `trace_id` reads `node`: it becomes the current key, or,
/// already current, goes on with this follow-up read. A held key's or
/// passed key's replacement is said as it ends. Returns what was heard.
pub(crate) fn press_reads(state: &mut SrState, node: NodeId, trace_id: TraceId) -> Vec<Effect> {
    let heard = replaced_key_heard(state, trace_id);
    set_current(state, CurrentKey::Review(Press { node, trace_id }));
    heard
}

/// A focus or a controlled selection has moved the navigator: a press still
/// reading says nothing and ends, its words would follow the focus's. What
/// is behind it runs on the navigator as it then is.
pub(crate) fn navigator_moved(state: &mut Core) -> Vec<Effect> {
    let Some(press) = current_press(state) else {
        return Vec::new();
    };
    if state
        .navigator
        .as_ref()
        .is_some_and(|navigator| navigator.object.id == press.node)
    {
        return Vec::new();
    }
    crate::driver::signal(state, Signal::PressGivesWay, 0)
}

/// The before read of the current key passed 150 ms: the key is passed on
/// and judged from the last read, the note its request set still live, or,
/// with the focus moved on, says nothing and ends.
fn before_read_given_up(
    state: &mut Core,
    trace_id: TraceId,
    (query, node, key): (QueryId, NodeId, HeldKey),
    at_us: u64,
) -> Vec<Effect> {
    let mut effects: Vec<Effect> = state
        .queries
        .withdraw_kept(query)
        .into_iter()
        .map(|effect| for_key(trace_id, effect))
        .collect();
    if !state.focus_matches(node) || key.pressed_on != Some(node) {
        effects.extend(crate::say_all::stop(state));
        effects.push(Effect::PassHeldKey(trace_id));
        effects.extend(end_current(state, trace_id, None));
        return effects;
    }
    let tk = crate::terminal::caret_key_kind(key.key);
    effects.extend(crate::terminal_model::key(
        state,
        tk,
        at_us / 1_000,
        trace_id,
    ));
    effects.extend(watch_from_last_read(
        state,
        key,
        node,
        (at_us, KEY_BOUND_MS, false),
    ));
    effects.push(for_key(trace_id, Effect::PassHeldKey(trace_id)));
    effects.extend(terminal_read_now(state, node, trace_id));
    effects.extend(pass_rule(state));
    effects
}

/// What the current key's bound passing with no evidence leads to.
enum AtBound {
    /// The key is over, or a key that waited has taken its place.
    Over(Vec<Effect>),
    /// A text field's key: what waited has gone ahead, and its final read,
    /// this query, is asked.
    FinalRead(QueryId, Vec<Effect>),
    /// A sentence key: Verbatim moves the caret by sentence, with what is
    /// behind it waiting for the move.
    Sentence(PassedKey),
}

/// The current key's bound passed with no evidence: a terminal's key ends;
/// for a text field's what waits goes ahead, and the key, if still current,
/// has its final read asked, and a sentence key goes on to move by
/// sentence.
fn bound_passed(state: &mut Core, watched: Watched) -> AtBound {
    let (trace_id, pass_ms) = (
        watched.passed().key.trace_id,
        Some(watched.passed().passed_at_us.div_ceil(1_000)),
    );
    // A terminal's key ends here, so what the line heard comes before what
    // waited behind it.
    let field = match watched {
        Watched::Field(field) => field,
        Watched::Terminal(_) => return AtBound::Over(end_current(state, trace_id, pass_ms)),
    };
    if is_sentence_key(field.passed.key.key) && !field.awaiting_unit {
        return AtBound::Sentence(field.passed);
    }
    let mut effects = advance(state, pass_ms);
    let field = match state.current() {
        Some(CurrentKey::Watched(Watched::Field(field)))
            if field.passed.key.trace_id == trace_id =>
        {
            field.clone()
        }
        Some(
            CurrentKey::Watched(_)
            | CurrentKey::Reading { .. }
            | CurrentKey::FinalReading { .. }
            | CurrentKey::MovingBySentence { .. }
            | CurrentKey::Review(_),
        )
        | None => return AtBound::Over(effects),
    };
    let node = field.passed.node;
    let read = CaretRead {
        fetch: fetch_of(field.passed.key.key),
        purpose: ReadPurpose::Final,
    };
    let (query, effect) = state.queries.ask(
        Asking::Text(node, TextOp::ReadCaret(read)),
        Then::Key { node },
    );
    set_current(state, CurrentKey::FinalReading { field });
    effects.push(for_key(trace_id, effect));
    AtBound::FinalRead(query, effects)
}

/// The sentence key waiting for its final read on `node`, whose evidence a
/// report may still be.
fn sentence_key_at_final_read(state: &SrState, node: NodeId) -> Option<PassedKey> {
    match state.current() {
        Some(CurrentKey::MovingBySentence {
            passed,
            final_read: true,
        }) if passed.node == node => Some(passed.clone()),
        Some(
            CurrentKey::MovingBySentence { .. }
            | CurrentKey::Reading { .. }
            | CurrentKey::Watched(_)
            | CurrentKey::FinalReading { .. }
            | CurrentKey::Review(_),
        )
        | None => None,
    }
}

/// The application moved the caret for a sentence key: its line unless a
/// later key is marked, then the changes, and it ends.
fn sentence_moved_by_application(
    state: &mut Core,
    trace_id: TraceId,
    passed: &PassedKey,
    read: &CaretReply,
    evidence: Evidence<'_>,
) -> Vec<Effect> {
    let speaks = passed.later_key_at_us.is_none();
    let words = if speaks {
        editing::evidence_words(state, passed.node, passed.key.key, (read, evidence))
    } else {
        Vec::new()
    };
    finish(state, trace_id, passed, read, (speaks, words))
}

/// The final read showed no effect: the caret is taken, its changes wait
/// for the key's words, and the caret's paragraph is read from the read's
/// own position, which this returns, as NVDA's move works from the position
/// it read.
fn final_read_found_no_effect(state: &mut SrState, node: NodeId, read: CaretReply) -> TextPoint {
    let (changes, general) = editing::changes_of_reply(&read);
    let segments = editing::selection_segments(&changes, general.as_general());
    let at = TextPoint::At(TextPosition {
        anchor: read.caret.line.start,
        offset: read.caret.line.offset,
    });
    editing::update_caret(state, node, read.caret);
    if let Some(busy) = state.busy_mut() {
        busy.heard.extend(segments);
    }
    at
}

/// Ends the sentence move, saying `words` unless a later key is marked.
fn sentence_said(
    state: &mut Core,
    passed: &PassedKey,
    words: Vec<UtteranceSegment>,
) -> Vec<Effect> {
    let trace_id = passed.key.trace_id;
    let mut effects = Vec::new();
    if passed.later_key_at_us.is_none() && !words.is_empty() {
        effects.push(editing::speak(trace_id, words));
    }
    effects.extend(end_current(
        state,
        trace_id,
        Some(passed.passed_at_us.div_ceil(1_000)),
    ));
    effects
}

/// The sentence move past its 150 ms, or left by the focus: its query is
/// withdrawn and it ends, saying nothing, the caret staying.
fn sentence_dropped(state: &mut Core, passed: &PassedKey, query: QueryId) -> Vec<Effect> {
    let trace_id = passed.key.trace_id;
    let mut effects: Vec<Effect> = state
        .queries
        .withdraw_kept(query)
        .into_iter()
        .map(|effect| for_key(trace_id, effect))
        .collect();
    effects.extend(sentence_said(state, passed, Vec::new()));
    effects
}

/// A caret key's function's handle: the driver's, and the key's trace,
/// which names it in the line.
struct Key {
    cx: Cx,
    trace_id: TraceId,
}

impl Key {
    /// The key as the line holds it now, while it is still the current key:
    /// the reducer writes the line while the function waits (a later key is
    /// marked, a report without its unit is taken, another key takes its
    /// place), so every step reads it again.
    fn current(&self) -> Option<CurrentKey> {
        self.cx.with(|core| {
            core.current()
                .filter(|current| current.trace_id() == Some(self.trace_id))
                .cloned()
        })
    }

    /// The key while it moves by sentence.
    fn sentence(&self) -> Option<PassedKey> {
        match self.current()? {
            CurrentKey::MovingBySentence { passed, .. } => Some(passed),
            _ => None,
        }
    }

    /// Says `effects`, the key's last.
    #[expect(clippy::unnecessary_wraps, reason = "a key's way out, like its waits")]
    fn say(&self, effects: Vec<Effect>) -> Option<()> {
        self.cx.emit(effects);
        Some(())
    }

    /// Ends the sentence move, saying `words` unless a later key is marked.
    fn sentence_said(&self, words: Vec<UtteranceSegment>) -> Option<()> {
        let passed = self.sentence()?;
        self.say(self.cx.with(|core| sentence_said(core, &passed, words)))
    }
}

/// A held caret key on `node`, from the read of the caret before it to its
/// result: the read, for at most 150 ms; then the key passed on, with that
/// caret or, the read given up, judged from the node's last read.
async fn held_caret_key(key: Key, held: HeldKey, node: NodeId) {
    let (query, effects) = key.cx.with(|core| read_before(core, held, node));
    key.cx.emit(effects);
    let wait = Wait {
        query: Some(query),
        signals: &[Signal::KeyBound],
        due_us: None,
    };
    match key.cx.wait(wait).await {
        Woke::Answered {
            ending,
            trace_id,
            at_us,
        } => {
            let reply = ending.reply();
            key.cx
                .run(|core| before_read_answered(core, trace_id, (node, held), reply, at_us));
        }
        Woke::Signalled(_, at_us) => key
            .cx
            .run(|core| before_read_given_up(core, held.trace_id, (query, node, held), at_us)),
        Woke::Due | Woke::Lost => return,
    }
    let _ = passed_key(&key).await;
}

/// A key passed on, held or not, to its result. Within its bound the
/// reducer ends it: a report that is its evidence, a focus elsewhere, a
/// controlled selection or its outpost's end. At its bound a terminal's key
/// ends, a sentence key goes on to move by sentence, and a text field's
/// lets what waited go ahead and asks its final read, whose answer is its
/// result, or, another key having taken its place, an ordinary report.
async fn passed_key(key: &Key) -> Option<()> {
    let watching = Wait {
        query: None,
        signals: &[Signal::KeyBound, Signal::KeyEnded],
        due_us: None,
    };
    let CurrentKey::Watched(_) = key.current()? else {
        return None;
    };
    let Woke::Signalled(Signal::KeyBound, _) = key.cx.wait(watching).await else {
        return None;
    };
    let CurrentKey::Watched(watched) = key.current()? else {
        return None;
    };
    let node = watched.passed().node;
    let query = match key.cx.with(|core| bound_passed(core, watched)) {
        AtBound::Over(effects) => return key.say(effects),
        AtBound::Sentence(passed) => return sentence_move(key, passed).await,
        AtBound::FinalRead(query, effects) => {
            key.cx.emit(effects);
            query
        }
    };
    let wait = Wait {
        query: Some(query),
        signals: &[],
        due_us: None,
    };
    let Woke::Answered {
        ending, trace_id, ..
    } = key.cx.wait(wait).await
    else {
        return None;
    };
    let reply = ending.reply();
    match key.current() {
        Some(CurrentKey::FinalReading { field }) => key.say(
            key.cx
                .with(|core| final_read_answered(core, trace_id, &field, reply)),
        ),
        _ => key.say(key.cx.with(|core| late_read(core, trace_id, node, reply))),
    }
}

/// A sentence key past its bound with no evidence: Verbatim moves the caret
/// by sentence (`docs/design/caret-selection-rework.md`, "Moving by
/// sentence"). The final read may show that the application moved the
/// caret after all; if not, the caret's paragraph is read for the sentence
/// after or before the caret, then the paragraph beside it when it has
/// none, and the caret is moved there and the sentence said. A step that
/// fails (text with no sentences, a read or move that could not be made)
/// ends the move saying nothing.
#[expect(
    clippy::too_many_lines,
    reason = "one sequence of four reads, which reads best whole"
)]
async fn sentence_move(key: &Key, passed: PassedKey) -> Option<()> {
    let node = passed.node;
    let forward = passed.key.key.motion == CaretMotion::NextSentence;
    let read = CaretRead {
        fetch: fetch_of(passed.key.key),
        purpose: ReadPurpose::Final,
    };
    key.cx.with(|core| {
        let current = CurrentKey::MovingBySentence {
            passed,
            final_read: true,
        };
        set_current(core, current);
    });
    let (reply, trace_id) = sentence_step(key, node, TextOp::ReadCaret(read)).await?;
    let TextReply::Caret(read) = reply else {
        return key.sentence_said(Vec::new());
    };
    let passed = key.sentence()?;
    if let Some(evidence) = evidence_of(&passed, &read.facts) {
        return key.say(
            key.cx.with(|core| {
                sentence_moved_by_application(core, trace_id, &passed, &read, evidence)
            }),
        );
    }
    let caret = key
        .cx
        .with(|core| final_read_found_no_effect(core, node, *read));
    let sentence_at = |at, movement| {
        TextOp::Read(TextRead {
            at,
            movement,
            unit: TextUnit::Sentence,
        })
    };
    let (reply, _) = sentence_step(key, node, sentence_at(caret, None)).await?;
    let TextReply::Read { chunk, .. } = reply else {
        return key.sentence_said(Vec::new());
    };
    // At the text's start the caret stays, and the first sentence is said.
    let stays = |paragraph: &TextChunk| {
        let sentence = editing::first_sentence(paragraph);
        key.sentence_said(editing::sentence_words(paragraph, &sentence.text))
    };
    let found = if forward {
        editing::next_sentence(&chunk)
    } else {
        editing::sentence_before(&chunk)
    };
    // The paragraph the caret goes to and the sentence in it, or, from the
    // text's last sentence, the text's end, where no sentence is.
    let (paragraph, sentence) = match found {
        Some(sentence) => (chunk, Some(sentence)),
        None if forward && chunk.last => (chunk, None),
        None if !forward && chunk.first => return stays(&chunk),
        None => {
            let movement = TextMovement {
                unit: TextUnit::Sentence,
                count: if forward { 1 } else { -1 },
            };
            let from = TextPoint::At(TextPosition::at(chunk.start));
            let (reply, _) = sentence_step(key, node, sentence_at(from, Some(movement))).await?;
            let TextReply::Read {
                moved,
                chunk: beside,
            } = reply
            else {
                return key.sentence_said(Vec::new());
            };
            match (moved, forward) {
                (0, true) => (chunk, None),
                (0, false) => return stays(&chunk),
                (_, true) => {
                    let sentence = editing::first_sentence(&beside);
                    (beside, Some(sentence))
                }
                (_, false) => {
                    let sentence = editing::last_sentence(&beside);
                    (beside, Some(sentence))
                }
            }
        }
    };
    let (point, line, said) = match sentence {
        Some(sentence) => (
            TextPoint::At(TextPosition {
                anchor: paragraph.start,
                offset: sentence.offset,
            }),
            TextChunk {
                offset: sentence.offset,
                ..paragraph
            },
            sentence.text,
        ),
        None => (
            TextPoint::End,
            TextChunk {
                offset: u32::try_from(paragraph.text.len()).unwrap_or(u32::MAX),
                ..paragraph
            },
            String::new(),
        ),
    };
    let (reply, _) = sentence_step(key, node, TextOp::MoveCaret(point)).await?;
    let TextReply::Moved { read_at_us } = reply else {
        return key.sentence_said(Vec::new());
    };
    let words = editing::sentence_words(&line, &said);
    key.cx
        .with(|core| editing::caret_moved_by_core(core, node, line, read_at_us));
    key.sentence_said(words)
}

/// One step of a sentence move: asks `op` of `node` under the key, with
/// the step's 150 ms, armed first so that a read the shell answers at once
/// ends it, and waits. `None` when the step passed its 150 ms or the focus
/// left, the read withdrawn and the move ended saying nothing, or when the
/// reducer ended the key meanwhile.
async fn sentence_step(key: &Key, node: NodeId, op: TextOp) -> Option<(TextReply, TraceId)> {
    let final_read = matches!(op, TextOp::ReadCaret(_));
    let query = key.cx.with(|core| {
        if let Some(busy) = core.busy_mut()
            && let CurrentKey::MovingBySentence { final_read: at, .. } = &mut busy.current
        {
            *at = final_read;
        }
        core.queries.ask(Asking::Text(node, op), Then::Key { node })
    });
    let (query, effect) = query;
    key.cx.emit([
        Effect::BoundHeldKey(Some((key.trace_id, HELD_READ_BOUND_MS))),
        for_key(key.trace_id, effect),
    ]);
    let wait = Wait {
        query: Some(query),
        signals: &[Signal::KeyBound, Signal::KeyEnded],
        due_us: None,
    };
    match key.cx.wait(wait).await {
        Woke::Answered {
            ending, trace_id, ..
        } => {
            let reply = ending.reply();
            if key.sentence().is_none() {
                // The key has ended: its read is an ordinary report.
                key.cx.run(|core| late_read(core, trace_id, node, reply));
                return None;
            }
            Some((reply, trace_id))
        }
        Woke::Signalled(Signal::KeyBound, _) => {
            let passed = key.sentence()?;
            key.cx.run(|core| sentence_dropped(core, &passed, query));
            None
        }
        Woke::Signalled(..) | Woke::Due | Woke::Lost => None,
    }
}

/// The final read of the current key answered: a read showing an effect is
/// its evidence, any other read the bound answer, and anything else ends it
/// saying nothing.
fn final_read_answered(
    state: &mut Core,
    trace_id: TraceId,
    field: &FieldKey,
    reply: TextReply,
) -> Vec<Effect> {
    let TextReply::Caret(read) = reply else {
        let key_trace = field.passed.key.trace_id;
        return end_current(
            state,
            key_trace,
            Some(field.passed.passed_at_us.div_ceil(1_000)),
        );
    };
    let evidence = evidence_of(&field.passed, &read.facts);
    field_result(state, trace_id, field, &read, evidence)
}

/// `read` as the evidence of a word key whose words read a unit it did not
/// fetch, a read dispatched before the key's note arrived: its caret and
/// its changes are taken, and the key waits for the final read at its
/// bound, which fetches the unit, adding no call.
fn awaiting_its_unit(
    state: &mut SrState,
    field: &FieldKey,
    read: &CaretReply,
    (speaks, evidence): (bool, Option<Evidence<'_>>),
) -> Option<Vec<Effect>> {
    if evidence.is_none()
        || !speaks
        || read.facts.unit.is_some()
        || !editing::words_read_the_unit(field.passed.key.key)
    {
        return None;
    }
    let (changes, general) = editing::changes_of_reply(read);
    let segments = editing::selection_segments(&changes, general.as_general());
    editing::update_caret(state, field.passed.node, read.caret.clone());
    let busy = state.busy_mut()?;
    busy.heard.extend(segments);
    match &mut busy.current {
        CurrentKey::Watched(watched) => match watched {
            Watched::Field(field) => field.awaiting_unit = true,
            Watched::Terminal(_) => {}
        },
        CurrentKey::FinalReading { field, .. } => field.awaiting_unit = true,
        CurrentKey::Reading { .. }
        | CurrentKey::MovingBySentence { .. }
        | CurrentKey::Review(_) => {}
    }
    Some(Vec::new())
}

/// A read that is the current key's result, by which node the key is on.
fn result(
    state: &mut Core,
    trace_id: TraceId,
    watched: &Watched,
    read: &CaretReply,
    evidence: Option<Evidence<'_>>,
) -> Vec<Effect> {
    match watched {
        Watched::Field(field) => field_result(state, trace_id, field, read, evidence),
        Watched::Terminal(passed) => {
            let speaks = result_speaks(passed, read, evidence.is_some());
            finish(state, trace_id, passed, read, (speaks, Vec::new()))
        }
    }
}

/// Whether a result of the key `passed` speaks. Evidence read before the
/// later key is this key's; a bound answer with any later key says nothing,
/// as NVDA's helper returns while a script waits (`editableText.py` 166).
fn result_speaks(passed: &PassedKey, read: &CaretReply, is_evidence: bool) -> bool {
    match passed.later_key_at_us {
        Some(later) => is_evidence && read.caret.read_at_us < later,
        None => true,
    }
}

/// A read that is a text field's key's result: `evidence` when it is the
/// key's evidence, else the bound answer.
fn field_result(
    state: &mut Core,
    trace_id: TraceId,
    field: &FieldKey,
    read: &CaretReply,
    evidence: Option<Evidence<'_>>,
) -> Vec<Effect> {
    let (node, key) = (field.passed.node, field.passed.key.key);
    let speaks = result_speaks(&field.passed, read, evidence.is_some());
    if let Some(effects) = awaiting_its_unit(state, field, read, (speaks, evidence)) {
        return effects;
    }
    let words = match (speaks, evidence) {
        (false, _) => Vec::new(),
        (true, Some(evidence)) => editing::evidence_words(state, node, key, (read, evidence)),
        (true, None) if field.awaiting_unit => editing::unit_words(state, node, key, read),
        (true, None) => editing::bound_words(state, node, key, read),
    };
    finish(state, trace_id, &field.passed, read, (speaks, words))
}

/// Finishes a key with its result: takes the caret, says the key's `words`,
/// then what the line heard, then this read's changes, and ends the key.
fn finish(
    state: &mut Core,
    trace_id: TraceId,
    passed: &PassedKey,
    read: &CaretReply,
    (speaks, words): (bool, Vec<UtteranceSegment>),
) -> Vec<Effect> {
    let (changes, general) = editing::changes_of_reply(read);
    let segments = editing::selection_segments(&changes, general.as_general());
    editing::update_caret(state, passed.node, read.caret.clone());
    let mut effects = Vec::new();
    if !words.is_empty() {
        effects.push(editing::speak(trace_id, words));
    }
    if speaks {
        if let Some(busy) = state.busy_mut() {
            let heard = std::mem::take(&mut busy.heard);
            effects.extend(editing::speak_each(trace_id, heard));
        }
        effects.extend(editing::speak_each(trace_id, segments));
    } else if let Some(busy) = state.busy_mut() {
        busy.heard.extend(segments);
    }
    effects.extend(end_current(
        state,
        passed.key.trace_id,
        Some(passed.passed_at_us.div_ceil(1_000)),
    ));
    effects
}

/// A focus report of node `node` is about to execute: the words of the
/// current key, from its before read, as NVDA's script speaks them before
/// the focus; empty unless the focus moves off the key's node within its
/// bound, with no later key.
pub(crate) fn before_focus(state: &mut SrState, node: NodeId) -> Vec<Effect> {
    let (trace_id, words) = match state.current() {
        Some(CurrentKey::Watched(watched)) if watched.passed().node != node => {
            (watched.passed().key.trace_id, first_words(state, watched))
        }
        Some(CurrentKey::FinalReading { field, .. }) if field.passed.node != node => {
            (field.passed.key.trace_id, Vec::new())
        }
        Some(CurrentKey::Reading {
            node: read, key, ..
        }) if *read != node => (key.trace_id, Vec::new()),
        Some(CurrentKey::MovingBySentence { passed, .. }) if passed.node != node => {
            (passed.key.trace_id, Vec::new())
        }
        Some(
            CurrentKey::Watched(_)
            | CurrentKey::FinalReading { .. }
            | CurrentKey::Reading { .. }
            | CurrentKey::MovingBySentence { .. }
            | CurrentKey::Review(_),
        )
        | None => return Vec::new(),
    };
    let mut effects = Vec::new();
    if !words.is_empty() {
        effects.push(editing::speak(trace_id, words));
    }
    if let Some(busy) = state.busy_mut() {
        let heard = std::mem::take(&mut busy.heard);
        effects.extend(editing::speak_each(trace_id, heard));
    }
    effects
}

/// The words of `watched` when a focus on another node comes first, from
/// a text field's before read.
fn first_words(state: &SrState, watched: &Watched) -> Vec<UtteranceSegment> {
    let field = match watched {
        Watched::Field(field) => field,
        Watched::Terminal(_) => return Vec::new(),
    };
    let (unit, line, removed) = match &field.before {
        Before::Read {
            unit,
            line,
            removed,
        } => (unit, line, removed),
        Before::NotRead => return Vec::new(),
    };
    if field.passed.later_key_at_us.is_some() {
        return Vec::new();
    }
    editing::focus_first_words(
        state,
        field.passed.key.key,
        (unit, line, removed.as_deref()),
    )
}

/// A focus of node `node` has executed. A key still being read that the
/// focus has left is no longer pressed on it, even if the focus comes back
/// before its read answers.
pub(crate) fn focus_executed(state: &mut Core, node: NodeId) -> Vec<Effect> {
    if let Some(busy) = state.busy_mut()
        && let CurrentKey::Reading {
            node: read, key, ..
        } = &mut busy.current
        && *read != node
    {
        key.pressed_on = None;
    }
    let Some(passed) = state.current().and_then(CurrentKey::passed).cloned() else {
        return Vec::new();
    };
    if passed.node == node {
        return Vec::new();
    }
    // A sentence move the focus left is dropped by its function, which
    // holds its read.
    if matches!(state.current(), Some(CurrentKey::MovingBySentence { .. })) {
        return crate::driver::signal(state, Signal::KeyBound, 0);
    }
    end_current(
        state,
        passed.key.trace_id,
        Some(passed.passed_at_us.div_ceil(1_000)),
    )
}

/// The current key's node's outpost has ended: a watched key says nothing
/// and ends; a key being read is ended by its request's "gone", and a
/// press by its function, woken with its read lost (`crate::driver::reap`).
pub(crate) fn outpost_ended(state: &mut Core, outpost: verbatim_model::OutpostId) -> Vec<Effect> {
    let Some(passed) = state.current().and_then(CurrentKey::passed).cloned() else {
        return Vec::new();
    };
    if passed.node.outpost() != outpost {
        return Vec::new();
    }
    end_current(state, passed.key.trace_id, None)
}

/// Selection changes `segments` of a report on `node`: the line holds them
/// for a watched key, and otherwise they are said, after what the line
/// heard.
pub(crate) fn changes_heard(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    segments: Vec<UtteranceSegment>,
) -> Vec<Effect> {
    let Some(busy) = state.busy_mut() else {
        return editing::speak_each(trace_id, segments);
    };
    let holds = match &busy.current {
        CurrentKey::Watched(watched) => watched.passed().node == node,
        CurrentKey::FinalReading { field, .. } => field.passed.node == node,
        CurrentKey::Reading { .. }
        | CurrentKey::MovingBySentence { .. }
        | CurrentKey::Review(_) => false,
    };
    if holds {
        busy.heard.extend(segments);
        return Vec::new();
    }
    let mut effects = editing::speak_each(trace_id, std::mem::take(&mut busy.heard));
    effects.extend(editing::speak_each(trace_id, segments));
    effects
}

/// Exit: every held key is passed on, in press order.
pub(crate) fn exit(state: &mut SrState) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let KeyLine::Busy(busy) = std::mem::take(&mut state.key_line) {
        let Busy {
            current, waiting, ..
        } = *busy;
        match current {
            CurrentKey::Reading { key, .. } => effects.push(Effect::PassHeldKey(key.trace_id)),
            CurrentKey::Watched(_)
            | CurrentKey::FinalReading { .. }
            | CurrentKey::MovingBySentence { .. }
            | CurrentKey::Review(_) => {}
        }
        for item in waiting {
            match item {
                WaitingKey::Held(held) => effects.push(Effect::PassHeldKey(held.trace_id)),
                WaitingKey::Later(_) | WaitingKey::Command(_) => {}
            }
        }
        effects.push(Effect::BoundHeldKey(None));
    }
    effects.extend(state.focus.queued_held_keys().map(Effect::PassHeldKey));
    effects
}
