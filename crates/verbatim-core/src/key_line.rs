//! The key line (`docs/design/caret-selection-rework.md`): each caret key's
//! one lifecycle value, from the hook holding it to its one result, with
//! its one deadline. A held key is read before it is passed on; a key the
//! hook passed goes straight to its watch. The outpost judges the evidence
//! and Core chooses every word (`crate::editing`).

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    CaretKey, CaretMotion, CaretReply, Effect, Input, KeyAnswer, KeyRead, KeyRequest, KeyStart,
    KeyStarted, KeyTag, NodeId, QueryId, TextChunk, TextOp, TextReply, TraceId, UtteranceSegment,
    WatchKind, WatchOrder,
};

use crate::editing::{self, is_grid, text_focus};
use crate::queries::{Asking, Then};
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
    Reading {
        query: QueryId,
        node: NodeId,
        key: HeldKey,
    },
    /// Passed on, its watch open.
    Watched(Box<Watched>),
}

/// A key passed on, waiting for its result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Watched {
    tag: KeyTag,
    node: NodeId,
    key: HeldKey,
    kind: WatchKind,
    /// When it was passed on, in microseconds.
    passed_at_us: u64,
    before: Before,
    /// When a later key was pressed, once one has been.
    later_key_at_us: Option<u64>,
    line: Line,
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

/// Whether what is behind the key waits for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Line {
    /// The bound has not passed; keys behind wait.
    Holding,
    /// The bound has passed; keys go ahead.
    Released,
}

/// The kind of watch `state`'s focus takes, from its role.
fn watch_kind(state: &SrState) -> WatchKind {
    let terminal = state
        .focus
        .object()
        .is_some_and(|focus| is_grid(focus.snapshot.role));
    if terminal {
        WatchKind::Terminal
    } else {
        WatchKind::TextField
    }
}

/// The tag the next query will get.
fn next_tag(state: &SrState) -> KeyTag {
    KeyTag(QueryId(state.queries.next()))
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
            CurrentKey::Watched(watched) => Some(watched.node),
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
fn start_held(state: &mut SrState, held: HeldKey) -> Vec<Effect> {
    let Some(node) = text_focus(state) else {
        let mut effects = crate::say_all::stop(state);
        if !editing::selects(held.key) && held.key.motion != CaretMotion::Other {
            state.typed_word.clear();
        }
        effects.push(Effect::PassHeldKey(held.trace_id));
        return effects;
    };
    let request = KeyRequest {
        tag: next_tag(state),
        kind: watch_kind(state),
        unit: held.key.motion.unit(),
        motion: held.key.motion,
        start: KeyStart::ReadFirst,
    };
    let (query, effect) = state
        .queries
        .ask(Asking::Text(node, TextOp::Key(request)), Then::Key { node });
    set_current(
        state,
        CurrentKey::Reading {
            query,
            node,
            key: held,
        },
    );
    // The deadline first, so a read the shell answers at once ends it.
    vec![
        Effect::BoundHeldKey(Some((held.trace_id, HELD_READ_BOUND_MS))),
        effect,
    ]
}

/// A caret key the keyboard hook held back: it starts its turn at once, or
/// waits behind the keys in the line.
pub(crate) fn held_key(
    state: &mut SrState,
    trace_id: TraceId,
    key: CaretKey,
    pressed_at_ms: u64,
) -> Vec<Effect> {
    let held = HeldKey {
        trace_id,
        key,
        pressed_at_ms,
    };
    let joins = match state.current() {
        None => false,
        Some(CurrentKey::Reading { .. }) => true,
        Some(CurrentKey::Watched(watched)) => watched.line == Line::Holding,
    };
    if joins {
        mark_later(state, pressed_at_ms.saturating_mul(1_000));
        if let Some(busy) = state.busy_mut() {
            busy.waiting.push_back(WaitingKey::Held(held));
        }
        return Vec::new();
    }
    under(Some(trace_id), start_held(state, held))
}

/// A key passed to the application that is not held, in its turn (a
/// caret key judged from the node's last read, text typed, or a key that
/// clears a terminal's line): one that finds the line holding waits behind
/// it, or runs at once while nothing is behind it.
pub(crate) fn key_arrived(state: &mut SrState, input: &Input) -> Vec<Effect> {
    let joins = match state.current() {
        None => false,
        Some(CurrentKey::Reading { .. }) => true,
        Some(CurrentKey::Watched(_)) => state
            .busy_mut()
            .is_some_and(|busy| !busy.waiting.is_empty()),
    };
    if joins {
        if let Some(busy) = state.busy_mut() {
            busy.waiting
                .push_back(WaitingKey::Later(Box::new(input.clone())));
        }
        return Vec::new();
    }
    crate::reduce::key_input(state, input)
}

/// A review command or script: one that finds a key in the line that still
/// holds it waits behind it, and any other runs now.
pub(crate) fn command_arrived(state: &mut SrState, input: &Input) -> Vec<Effect> {
    let joins = match state.current() {
        None => false,
        Some(CurrentKey::Reading { .. }) => true,
        Some(CurrentKey::Watched(watched)) => watched.line == Line::Holding,
    };
    if !joins {
        return crate::reduce::command_input(state, input);
    }
    if let Some(at_ms) = command_time(input) {
        mark_later(state, at_ms.saturating_mul(1_000));
    }
    if let Some(busy) = state.busy_mut() {
        busy.waiting
            .push_back(WaitingKey::Command(Box::new(input.clone())));
    }
    Vec::new()
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
    if let Some(busy) = state.busy_mut()
        && let CurrentKey::Watched(watched) = &mut busy.current
        && watched.later_key_at_us.is_none()
    {
        watched.later_key_at_us = Some(at_us);
    }
}

/// A caret key the hook passed, in its turn: with a focus that has text it
/// becomes the current key, judged from the node's last read. Any other
/// does nothing but what a key always does.
pub(crate) fn unheld_caret_key(
    state: &mut SrState,
    trace_id: TraceId,
    key: CaretKey,
    pressed_at_ms: u64,
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
    };
    let mut effects = watch_from_last_read(state, held, node, pressed_at_ms.saturating_mul(1_000));
    effects.extend(pass_rule(state));
    effects
}

/// `held` becomes the current key, watched from the node's last read: the
/// watch is asked for first, so it reaches the outpost before the key
/// reaches the application.
fn watch_from_last_read(
    state: &mut SrState,
    held: HeldKey,
    node: NodeId,
    passed_at_us: u64,
) -> Vec<Effect> {
    let tag = next_tag(state);
    let kind = watch_kind(state);
    let request = KeyRequest {
        tag,
        kind,
        unit: held.key.motion.unit(),
        motion: held.key.motion,
        start: KeyStart::FromLastRead,
    };
    let (_, effect) = state
        .queries
        .ask(Asking::Text(node, TextOp::Key(request)), Then::Key { node });
    let mut effects = key_taking_effect(state, held.key, node);
    effects.push(Effect::BoundHeldKey(Some((held.trace_id, KEY_BOUND_MS))));
    effects.push(for_key(held.trace_id, effect));
    set_current(
        state,
        CurrentKey::Watched(Box::new(Watched {
            tag,
            node,
            key: held,
            kind,
            passed_at_us,
            before: Before::NotRead,
            later_key_at_us: None,
            line: Line::Holding,
        })),
    );
    effects
}

/// The rule at a key's pass: typed keys at the front of what waits run at
/// once, as NVDA's typed keys are not held behind a script, stopping at the
/// first command or held key, and anything left behind it marks a later
/// key at the pass.
fn pass_rule(state: &mut SrState) -> Vec<Effect> {
    let mut effects = Vec::new();
    loop {
        let Some(busy) = state.busy_mut() else {
            return effects;
        };
        // A typed key sent with the one passed on reached the program then.
        let passed_at_ms = match &busy.current {
            CurrentKey::Watched(watched) => Some(watched.passed_at_us.div_ceil(1_000)),
            CurrentKey::Reading { .. } => None,
        };
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
    if let Some(busy) = state.busy_mut()
        && (queued || !busy.waiting.is_empty())
        && let CurrentKey::Watched(watched) = &mut busy.current
    {
        watched.later_key_at_us.get_or_insert(watched.passed_at_us);
    }
    effects
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
/// A key still `Released` stays current until a held key replaces it.
fn run_waiting(
    state: &mut SrState,
    mut waiting: VecDeque<WaitingKey>,
    passed_at_ms: Option<u64>,
) -> Vec<Effect> {
    let mut effects = Vec::new();
    while let Some(item) = waiting.pop_front() {
        let (trace, own, starts_key) = match item {
            WaitingKey::Held(held) => (Some(held.trace_id), start_held(state, held), true),
            WaitingKey::Later(input) => {
                let input = pressed_no_earlier(*input, passed_at_ms);
                let starts_key = is_caret_key(&input);
                let trace = crate::reduce::key_trace(&input);
                (trace, crate::reduce::key_input(state, &input), starts_key)
            }
            WaitingKey::Command(input) => {
                let trace = input.trace_id();
                (trace, crate::reduce::command_input(state, &input), false)
            }
        };
        effects.extend(under(trace, own));
        if starts_key
            && let Some(busy) = state.busy_mut()
            && matches!(&busy.current, CurrentKey::Reading { key, .. } if Some(key.trace_id) == trace)
        {
            busy.waiting.append(&mut waiting);
            break;
        }
        if starts_key
            && let Some(busy) = state.busy_mut()
            && matches!(&busy.current, CurrentKey::Watched(watched)
                if Some(watched.key.trace_id) == trace && watched.line == Line::Holding)
        {
            busy.waiting.append(&mut waiting);
            effects.extend(pass_rule(state));
            break;
        }
    }
    effects
}

/// Advances the line: runs what waits, a key still current staying until a
/// held key replaces it.
fn advance(state: &mut SrState, at_ms: Option<u64>) -> Vec<Effect> {
    let Some(busy) = state.busy_mut() else {
        return Vec::new();
    };
    let waiting = std::mem::take(&mut busy.waiting);
    run_waiting(state, waiting, at_ms)
}

/// Ends the current key: clears its deadline, says what is left in `heard`,
/// and advances. With no key left
/// the line is `Idle`.
fn end_current(state: &mut SrState, trace_id: TraceId, passed_at_ms: Option<u64>) -> Vec<Effect> {
    let KeyLine::Busy(busy) = std::mem::take(&mut state.key_line) else {
        return Vec::new();
    };
    let Busy { waiting, heard, .. } = *busy;
    let mut effects = vec![Effect::BoundHeldKey(None)];
    effects.extend(editing::speak_each(trace_id, heard));
    effects.extend(run_waiting(state, waiting, passed_at_ms));
    effects
}

/// The current key's answer: passes the key on with what its before read
/// found, or ends it, as what came.
pub(crate) fn request_answered(
    state: &mut SrState,
    trace_id: TraceId,
    (query, node): (QueryId, NodeId),
    reply: TextReply,
) -> Vec<Effect> {
    match state.current() {
        Some(CurrentKey::Reading { query: asked, .. }) if *asked == query => {
            before_read_answered(state, trace_id, reply)
        }
        Some(CurrentKey::Watched(watched)) if watched.tag == KeyTag(query) => {
            if matches!(reply, TextReply::Key(_)) {
                return Vec::new();
            }
            let key = watched.key.trace_id;
            let at_ms = Some(watched.passed_at_us.div_ceil(1_000));
            end_current(state, key, at_ms)
        }
        _ => stale_read(state, trace_id, node, reply),
    }
}

/// A key request's answer that is no longer the current key's: with a read
/// it is an ordinary report.
fn stale_read(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    reply: TextReply,
) -> Vec<Effect> {
    let TextReply::Key(started) = reply else {
        return Vec::new();
    };
    let KeyStarted::Read(read) = *started else {
        return Vec::new();
    };
    ordinary_report(state, trace_id, node, *read)
}

/// `read` of `node` as a report no key's answer is: the caret is taken if
/// newest, and its changes are said as any report's are.
fn ordinary_report(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    read: CaretReply,
) -> Vec<Effect> {
    let (changes, general) = editing::changes_of_reply(&read);
    editing::update_caret(state, node, read.caret);
    editing::selection_changed(state, trace_id, node, (&changes, general.as_general()))
}

/// The before read of the current key answered.
fn before_read_answered(state: &mut SrState, trace_id: TraceId, reply: TextReply) -> Vec<Effect> {
    let Some(CurrentKey::Reading { query, node, key }) = state.current().cloned() else {
        return Vec::new();
    };
    let held = key;
    let read = match reply {
        TextReply::Key(started) => match *started {
            KeyStarted::Read(read) => Some(read),
            KeyStarted::Watching => None,
        },
        _ => None,
    };
    let Some(read) = read else {
        // Nothing was read: the key is passed on, and says nothing.
        let mut effects = crate::say_all::stop(state);
        effects.push(Effect::PassHeldKey(held.trace_id));
        effects.extend(end_current(state, held.trace_id, None));
        return effects;
    };
    if !state.focus_matches(node) {
        // The focus moved on while the key was read: its words would follow
        // the focus's.
        let mut effects = crate::say_all::stop(state);
        effects.push(Effect::PassHeldKey(held.trace_id));
        effects.push(Effect::WatchOrder {
            node,
            tag: KeyTag(query),
            order: WatchOrder::End,
        });
        effects.extend(end_current(state, held.trace_id, None));
        return effects;
    }
    let read = *read;
    let read_at_us = read.caret.read_at_us;
    let (changes, general) = editing::changes_of_reply(&read);
    let before = editing::before_of_read(state, node, held.key.motion, &read);
    editing::update_caret(state, node, read.caret);
    let mut effects =
        editing::selection_changed(state, trace_id, node, (&changes, general.as_general()));
    let passed_at_us = read_at_us.saturating_add(1);
    let kind = watch_kind(state);
    let tk = crate::terminal::caret_key_kind(held.key);
    effects.extend(crate::terminal_model::key(
        state,
        tk,
        passed_at_us.div_ceil(1_000),
        held.trace_id,
    ));
    effects.extend(key_taking_effect(state, held.key, node));
    effects.push(Effect::BoundHeldKey(Some((held.trace_id, KEY_BOUND_MS))));
    set_current(
        state,
        CurrentKey::Watched(Box::new(Watched {
            tag: KeyTag(query),
            node,
            key: held,
            kind,
            passed_at_us,
            before: Before::Read {
                unit: before.unit,
                line: before.line,
                removed: before.removed,
            },
            later_key_at_us: None,
            line: Line::Holding,
        })),
    );
    effects.push(for_key(held.trace_id, Effect::PassHeldKey(held.trace_id)));
    effects.extend(pass_rule(state));
    effects
}

/// The line's one deadline has passed.
pub(crate) fn deadline_passed(state: &mut SrState, trace_id: TraceId, at_us: u64) -> Vec<Effect> {
    match state.current() {
        Some(CurrentKey::Reading { key, .. }) if key.trace_id == trace_id => {
            before_read_given_up(state, trace_id, at_us)
        }
        Some(CurrentKey::Watched(watched))
            if watched.key.trace_id == trace_id && watched.line == Line::Holding =>
        {
            bound_passed(state)
        }
        _ => Vec::new(),
    }
}

/// The before read of the current key passed 150 ms: the key is passed on
/// and judged from the last read, or, with the focus moved on, says nothing
/// and ends.
fn before_read_given_up(state: &mut SrState, trace_id: TraceId, at_us: u64) -> Vec<Effect> {
    let Some(CurrentKey::Reading { query, node, key }) = state.current().cloned() else {
        return Vec::new();
    };
    let mut effects: Vec<Effect> = state
        .queries
        .withdraw_kept(query)
        .into_iter()
        .map(|effect| for_key(trace_id, effect))
        .collect();
    if !state.focus_matches(node) {
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
    effects.extend(watch_from_last_read(state, key, node, at_us));
    effects.push(for_key(trace_id, Effect::PassHeldKey(trace_id)));
    effects.extend(pass_rule(state));
    effects
}

/// The current key's bound passed with no evidence: what waits goes ahead,
/// and the final read follows if the key is still current, as a key that
/// replaced it ended its watch with its request.
fn bound_passed(state: &mut SrState) -> Vec<Effect> {
    let Some(busy) = state.busy_mut() else {
        return Vec::new();
    };
    let CurrentKey::Watched(watched) = &mut busy.current else {
        return Vec::new();
    };
    watched.line = Line::Released;
    let (node, tag, trace_id, passed_at_us, kind) = (
        watched.node,
        watched.tag,
        watched.key.trace_id,
        watched.passed_at_us,
        watched.kind,
    );
    let mut effects = advance(state, Some(passed_at_us.div_ceil(1_000)));
    let still_current =
        matches!(state.current(), Some(CurrentKey::Watched(watched)) if watched.tag == tag);
    if kind == WatchKind::TextField && still_current {
        effects.push(for_key(
            trace_id,
            Effect::WatchOrder {
                node,
                tag,
                order: WatchOrder::FinalRead,
            },
        ));
    }
    effects
}

/// A key's tagged answer arrived.
pub(crate) fn key_answered(
    state: &mut SrState,
    trace_id: TraceId,
    node: NodeId,
    tag: KeyTag,
    answer: KeyAnswer,
) -> Vec<Effect> {
    let is_current = matches!(
        state.current(),
        Some(CurrentKey::Watched(watched)) if watched.tag == tag
    );
    if !is_current {
        return match answer {
            KeyAnswer::Evidence(read) | KeyAnswer::BoundAnswer(read) => {
                let read = *read;
                let (changes, general) = editing::changes_of_key_read(&read);
                editing::update_caret(state, node, read.caret);
                editing::selection_changed(state, trace_id, node, (&changes, general.as_general()))
            }
            KeyAnswer::Ended => Vec::new(),
        };
    }
    let Some(CurrentKey::Watched(watched)) = state.current().cloned() else {
        return Vec::new();
    };
    let key_trace = watched.key.trace_id;
    let passed_at_ms = Some(watched.passed_at_us.div_ceil(1_000));
    match answer {
        KeyAnswer::Ended => end_current(state, key_trace, passed_at_ms),
        KeyAnswer::Evidence(read) => result(state, trace_id, &watched, *read, true),
        KeyAnswer::BoundAnswer(read) => result(state, trace_id, &watched, *read, false),
    }
}

/// A read that is the current key's result: takes the caret, says the key's
/// words, then what the line heard, then this read's changes, and ends the
/// key.
fn result(
    state: &mut SrState,
    trace_id: TraceId,
    watched: &Watched,
    read: KeyRead,
    evidence: bool,
) -> Vec<Effect> {
    let read_at_us = read.caret.read_at_us;
    // Evidence read before the later key is this key's; a bound answer with
    // any later key says nothing, as NVDA's helper returns while a script
    // waits (`editableText.py` 166).
    let speaks = match watched.later_key_at_us {
        Some(later) => evidence && read_at_us < later,
        None => true,
    };
    let words = if watched.kind == WatchKind::TextField && speaks {
        if evidence {
            editing::evidence_words(state, watched.node, watched.key.key, &read)
        } else {
            editing::bound_words(state, watched.node, watched.key.key, &read)
        }
    } else {
        Vec::new()
    };
    let (changes, general) = editing::changes_of_key_read(&read);
    let segments = editing::selection_segments(&changes, general.as_general());
    editing::update_caret(state, watched.node, read.caret);
    let mut effects = Vec::new();
    if !words.is_empty() {
        effects.push(editing::speak(trace_id, words));
    }
    if let Some(busy) = state.busy_mut() {
        let heard = std::mem::take(&mut busy.heard);
        effects.extend(editing::speak_each(trace_id, heard));
    }
    effects.extend(editing::speak_each(trace_id, segments));
    effects.extend(end_current(
        state,
        watched.key.trace_id,
        Some(watched.passed_at_us.div_ceil(1_000)),
    ));
    effects
}

/// A focus report of node `node` is about to execute: the words of the
/// current key, from its before read, as NVDA's script speaks them before
/// the focus; empty unless the focus moves off the key's node within its
/// bound, with no later key.
pub(crate) fn before_focus(state: &mut SrState, node: NodeId) -> Vec<Effect> {
    let (trace_id, words) = match state.current() {
        Some(CurrentKey::Watched(watched)) if watched.node != node => {
            (watched.key.trace_id, first_words(state, watched))
        }
        Some(CurrentKey::Reading {
            node: read, key, ..
        }) if *read != node => (key.trace_id, Vec::new()),
        _ => return Vec::new(),
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
/// its before read.
fn first_words(state: &SrState, watched: &Watched) -> Vec<UtteranceSegment> {
    if watched.kind != WatchKind::TextField
        || watched.line != Line::Holding
        || watched.later_key_at_us.is_some()
    {
        return Vec::new();
    }
    let Before::Read {
        unit,
        line,
        removed,
    } = &watched.before
    else {
        return Vec::new();
    };
    editing::focus_first_words(state, watched.key.key, (unit, line, removed.as_deref()))
}

/// A focus of node `node` has executed.
pub(crate) fn focus_executed(state: &mut SrState, node: NodeId) -> Vec<Effect> {
    let Some(CurrentKey::Watched(watched)) = state.current().cloned() else {
        return Vec::new();
    };
    if watched.node == node {
        return Vec::new();
    }
    let trace = watched.key.trace_id;
    let mut effects = Vec::new();
    if watched.kind == WatchKind::TextField && watched.line == Line::Holding {
        effects.push(for_key(
            trace,
            Effect::WatchOrder {
                node: watched.node,
                tag: watched.tag,
                order: WatchOrder::End,
            },
        ));
    }
    effects.extend(end_current(
        state,
        trace,
        Some(watched.passed_at_us.div_ceil(1_000)),
    ));
    effects
}

/// The current key's node's outpost has ended: a watched key says nothing
/// and ends; a key being read is ended by its request's "gone".
pub(crate) fn outpost_ended(
    state: &mut SrState,
    outpost: verbatim_model::OutpostId,
) -> Vec<Effect> {
    let Some(CurrentKey::Watched(watched)) = state.current().cloned() else {
        return Vec::new();
    };
    if watched.node.outpost() != outpost {
        return Vec::new();
    }
    end_current(state, watched.key.trace_id, None)
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
    if matches!(&busy.current, CurrentKey::Watched(watched) if watched.node == node) {
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
        if let CurrentKey::Reading { key, .. } = current {
            effects.push(Effect::PassHeldKey(key.trace_id));
        }
        for item in waiting {
            if let WaitingKey::Held(held) = item {
                effects.push(Effect::PassHeldKey(held.trace_id));
            }
        }
        effects.push(Effect::BoundHeldKey(None));
    }
    effects.extend(state.focus.queued_held_keys().map(Effect::PassHeldKey));
    effects
}
