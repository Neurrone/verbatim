//! The reducer (architecture section 2): pure state-and-effect transitions
//! from normalized accessibility events and fetch completions.
//!
//! Nothing in this module performs I/O, reads a clock, or consults
//! randomness; the only way the outside world enters is through the `Input`
//! passed to [`reduce`]. That purity is what lets the flight recorder turn a
//! captured sequence of inputs into a deterministic regression test (see
//! [`crate::replay`]).
//!
//! The behavior is specified in the focus and announcement files of
//! `docs/parity/` (listed in `docs/parity/readme.md`), written from the NVDA behavior documented in
//! `docs/nvda/events.md`.

use std::sync::Arc;

use verbatim_model::{
    ActionName, Earcon, Effect, FetchResult, Input, Message, NodeId, NodeSnapshot, NormalizedEvent,
    Notification, NotificationProcessing, OutpostId, Phrase, Pid, ProgressReading, PropertyChange,
    Query, QueryId, QueryKind, ReviewCommand, Role, SegmentContent, SpeechPriority, State,
    StateSet, TraceId, Utterance, UtteranceSegment, UtteranceSource, WindowFacts,
};
use verbatim_model::{Backend, FocusNow, FocusText, FocusValidity};

use crate::focus_rules::{
    ChangeKind, ChangeTarget, ForegroundRelation, RepeatEvidence, SelectionTarget, already_entered,
    change_target, repeats_focus, selection_target,
};
use crate::state::{
    Attention, FocusContext, LatestFocus, Navigator, NavigatorRead, PendingNavigation, SrState,
    TextFollowUp,
};
use crate::{editing, review, review_text, say_all, shell, terminal, terminal_model, text};

/// The activity id of the shell's window-snap results notification, the one
/// UIA notification spoken from any application (`docs/parity/focus-events.md`, "Event
/// acceptance"); the supervisor starts an outpost for no other notification.
pub const SNAP_RESULTS_ACTIVITY: &str = "Windows.Shell.SnapComponent.SnapHotKeyResults";

/// Advances `state` by one `input`, changing it in place, and returns the
/// effects the imperative shell must execute.
///
/// Deterministic: no I/O, no clocks, and no randomness, so the same state
/// and input always produce the same new state and effects. The state is
/// changed in place rather than copied, so a step costs nothing for the
/// parts of the state it does not touch (architecture section 2).
#[must_use]
pub fn reduce(state: &mut SrState, input: &Input) -> Vec<Effect> {
    let focus = state.focus.as_ref().map(|focus| focus.snapshot.id);
    let mut effects = reduce_input(state, input);
    // The terminals Core reads (`terminal_model`): told when the focus
    // reaches or leaves them, given the settings, and their text answered
    // from their memory.
    if !state.terminals.is_empty() {
        let now = state.focus.as_ref().map(|focus| focus.snapshot.id);
        if now != focus {
            let at_ms = match input {
                Input::Event { observed_at_ms, .. } => *observed_at_ms,
                _ => 0,
            };
            effects.extend(terminal_model::focus_moved(state, (focus, now), at_ms));
        }
        effects.extend(terminal_model::sync_settings(state));
        effects = terminal_model::intercept(state, effects, terminal_model::input_trace(input));
    }
    // Whatever cuts speech off drops the terminal output handed to it, and
    // with it the output still waiting, as a key press does in NVDA.
    if effects.iter().any(terminal::cuts_speech) {
        let cut = terminal::cut(state, 0);
        effects.extend(cut);
    }
    effects
}

/// [`reduce`]'s dispatch, by input.
fn reduce_input(state: &mut SrState, input: &Input) -> Vec<Effect> {
    match input {
        Input::Event {
            trace_id,
            observed_at_ms,
            observed_at_us,
            source,
            window,
            event,
            ..
        } => reduce_event(
            state,
            *trace_id,
            (*observed_at_ms, *observed_at_us),
            *source,
            window.as_ref(),
            event,
        ),
        Input::FetchCompleted {
            trace_id,
            query_id,
            kind,
            result,
        } => reduce_navigate_completed(state, *trace_id, *query_id, *kind, result),
        Input::OutpostEnded { outpost } => outpost_ended(state, *outpost),
        Input::Command {
            trace_id,
            command,
            repeat,
        } => reduce_command(state, *trace_id, *command, *repeat),
        Input::ActivationCompleted {
            trace_id,
            activated,
            action,
        } => reduce_activation_completed(*trace_id, *activated, action.as_ref()),
        Input::TextCompleted {
            trace_id,
            query_id,
            reply,
        } => reduce_text_completed(state, *trace_id, *query_id, reply.clone()),
        // A key pressed while a held key waits reaches the application
        // after it, so it is handled after it (`docs/design/key-holding.md`,
        // "Keys behind a held key").
        Input::CaretKey { .. } | Input::CharacterTyped { .. } | Input::ClearingKey { .. }
            if state.held_keys.busy() =>
        {
            state
                .held_keys
                .waiting
                .push_back(crate::state::WaitingKey::Later(Box::new(input.clone())));
            Vec::new()
        }
        Input::CaretKey { .. } | Input::CharacterTyped { .. } | Input::ClearingKey { .. } => {
            key_input(state, input)
        }
        Input::HeldKey {
            trace_id,
            key,
            pressed_at_ms,
        } => editing::held_key(state, *trace_id, *key, *pressed_at_ms),
        Input::HeldKeyWaitEnded { trace_id } => editing::held_wait_ended(state, *trace_id),
        Input::TerminalTimer { trace_id, at_ms } => terminal_model::timer(state, *trace_id, *at_ms),
        Input::MarkReached { mark } => {
            let mut effects = say_all::mark_reached(state, *mark);
            effects.extend(terminal::mark_reached(state, *mark));
            effects
        }
        Input::SpeechCancelled { at_ms } => {
            editing::later_key(state, *at_ms);
            let mut effects = terminal::cut(state, *at_ms);
            effects.extend(say_all::stop(state));
            effects
        }
        Input::Settings(settings) => {
            let effects =
                if state.settings.report_terminal_output && !settings.report_terminal_output {
                    crate::terminal::drop_waiting(state)
                } else {
                    Vec::new()
                };
            state.settings = *settings;
            effects
        }
        Input::Fetches(fetches) => {
            state.fetches = *fetches;
            Vec::new()
        }
        // Playback will reach none of a cut utterance's marks it had not
        // reached: say-all stops, as on any cut, and terminal output drops
        // what waits, as anything that cuts speech off does.
        Input::UtteranceCancelled { marks, at_ms } => {
            let mut effects = terminal::utterance_cancelled(state, marks, *at_ms);
            effects.extend(say_all::utterance_cancelled(state, marks));
            effects
        }
        // A failed utterance counts as heard: its marks are taken as
        // reached, though say-all moves no cursor to what was not heard.
        Input::UtteranceFailed { marks } => {
            let mut effects = say_all::utterance_failed(state, marks);
            for mark in marks {
                effects.extend(terminal::mark_reached(state, *mark));
            }
            effects
        }
        // What the shell's threads learn; `Tick` is reserved vocabulary with
        // no policy yet, which `shell::reduce` leaves alone, as it does any
        // other input.
        _ => shell::reduce(state, input),
    }
}

/// Handles a key passed to the application that is not held: a caret key
/// judged by where the caret lands, text typed, or a key that clears a
/// terminal's line. Any other input does nothing here.
pub(crate) fn key_input(state: &mut SrState, input: &Input) -> Vec<Effect> {
    // A terminal Core reads keeps the key for its reading rules.
    let mut effects = terminal_model::key_input(state, input);
    effects.extend(match input {
        Input::CaretKey {
            key, pressed_at_ms, ..
        } => editing::caret_key(state, *key, *pressed_at_ms),
        Input::CharacterTyped { trace_id, text, .. } => {
            editing::character_typed(state, *trace_id, text)
        }
        Input::ClearingKey { .. } => {
            editing::clearing_key(state);
            Vec::new()
        }
        _ => Vec::new(),
    });
    effects
}

/// The trace of a key's input, minted when the hook saw the key.
pub(crate) fn key_trace(input: &Input) -> Option<TraceId> {
    match input {
        Input::CaretKey { trace_id, .. }
        | Input::CharacterTyped { trace_id, .. }
        | Input::ClearingKey { trace_id, .. }
        | Input::HeldKey { trace_id, .. } => Some(*trace_id),
        _ => None,
    }
}

/// `input` with a caret key's press taken as no earlier than `at_ms`, when
/// that is known.
pub(crate) fn pressed_no_earlier(input: Input, at_ms: u64) -> Input {
    match input {
        Input::CaretKey {
            trace_id,
            key,
            pressed_at_ms,
        } if pressed_at_ms != 0 => Input::CaretKey {
            trace_id,
            key,
            pressed_at_ms: pressed_at_ms.max(at_ms),
        },
        Input::CharacterTyped {
            trace_id,
            text,
            at_ms: typed_at_ms,
        } if typed_at_ms != 0 => Input::CharacterTyped {
            trace_id,
            text,
            at_ms: typed_at_ms.max(at_ms),
        },
        Input::ClearingKey {
            trace_id,
            at_ms: cleared_at_ms,
        } if cleared_at_ms != 0 => Input::ClearingKey {
            trace_id,
            at_ms: cleared_at_ms.max(at_ms),
        },
        input => input,
    }
}

/// How an event relates to the attention record (`docs/parity/focus-events.md`, "Event
/// acceptance").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Acceptance {
    /// The event concerns what the user is attending to and is handled in
    /// full.
    Attended,
    /// The event comes from elsewhere but is of a kind spoken from
    /// anywhere: spoken queued, never moving focus or the navigator.
    Background,
    /// The event is dropped unheard.
    Dropped,
}

/// Classifies one event against the attention record, or, for a focus with
/// window facts, against the system's foreground window as its outpost read
/// it. A foreground change is always attended: its intake has already
/// confirmed the window is the system's foreground window. With no attention yet, everything is
/// attended, since there is nothing to compare against. `focus_source` is
/// the application the focus belongs to, which UIA notifications are judged
/// by.
fn classify(
    attention: Option<&Attention>,
    focus_source: Option<Pid>,
    source: Pid,
    window: Option<&WindowFacts>,
    event: &NormalizedEvent,
) -> Acceptance {
    let Some(attention) = attention else {
        return Acceptance::Attended;
    };
    match event {
        // A foreground change is always attended. A controlled selection is
        // spoken only while its controller is the focus, which is attended
        // wherever the controlled list's window is.
        NormalizedEvent::FocusChanged {
            foreground: true, ..
        }
        | NormalizedEvent::ControlledSelection { .. } => Acceptance::Attended,
        // Any other focus is judged against the system's foreground window
        // as its outpost found it when it published the event, not against
        // the attention record: a focus can reach Core before the
        // foreground change that moved the foreground away from its window
        // (D14, amended 2026-10-05). With no window facts, the application
        // decides. The rule is the acceptance test's own
        // ([`ForegroundRelation::related`]), made again on the facts the
        // outpost read as it reported the focus; NVDA makes it once, before
        // reading (`docs/parity/focus-events.md`, "Event acceptance").
        NormalizedEvent::FocusChanged { .. } => {
            let attended = match window {
                Some(event) => ForegroundRelation::of_facts(event).related(),
                None => window_is_attended(attention, source, None),
            };
            if attended {
                Acceptance::Attended
            } else {
                Acceptance::Dropped
            }
        }
        // A toast is spoken from anywhere.
        NormalizedEvent::Alert { .. } => {
            if window_is_attended(attention, source, window) {
                Acceptance::Attended
            } else {
                Acceptance::Background
            }
        }
        // A UIA notification is spoken only from the focus's application,
        // as NVDA drops notifications from any other ("background apps"),
        // which is not always the attended one: in Settings the focus is in
        // SystemSettings while ApplicationFrameHost holds attention. With no
        // focus yet, the attended application stands in. The shell's
        // window-snap results are spoken from anywhere and always queued,
        // as NVDA's Explorer module speaks them, even when Explorer holds
        // the focus.
        NormalizedEvent::Notification { notification, .. } => {
            if notification.activity_id.as_deref() == Some(SNAP_RESULTS_ACTIVITY) {
                Acceptance::Background
            } else if source == focus_source.unwrap_or(attention.source) {
                Acceptance::Attended
            } else {
                Acceptance::Dropped
            }
        }
        _ => {
            if window_is_attended(attention, source, window) {
                Acceptance::Attended
            } else {
                Acceptance::Dropped
            }
        }
    }
}

/// Whether a focus event from `outpost`, observed at `observed`, its
/// milliseconds and microseconds on the run's shared clock, was observed
/// before the newest focus applied from another outpost. Events from one
/// outpost arrive in order; across outposts they can arrive out of order,
/// which NVDA's single event queue never does. Two focuses stamped in one
/// millisecond are ordered by their microseconds when both carry them
/// ([`LatestFocus::observed_after`]). An answer to a focus-now query carries
/// the time the outpost began reading it, so it is ordered with the events
/// by that; a report with no observation time (0) is never stale.
///
/// A focus in the same top-level window as the newest focus is never stale:
/// one window can hold several applications (a Settings page's content
/// inside `ApplicationFrameHost`'s frame), and a foreground change is ordered
/// by when its window became the foreground, after the content's own focus
/// may have been observed (`docs/parity/focus-events.md`, "Stale focus events").
fn is_stale_focus(
    state: &SrState,
    outpost: OutpostId,
    observed: (u64, u64),
    window: Option<&WindowFacts>,
) -> bool {
    let top_level = window.map(|window| window.top_level);
    observed.0 != 0
        && state.latest_focus.is_some_and(|latest| {
            latest.outpost != outpost
                && latest.observed_after(observed)
                && !(top_level.is_some() && top_level == latest.top_level)
        })
}

/// Core's duplicate-focus rule (`docs/design/focus-pipeline.md`, section
/// 4.2; `docs/parity/focus-events.md`, "Duplicate focus suppression"): a focus whose
/// element is the same as the last focus Core queued, while that element
/// still has the keyboard focus, is dropped before anything else judges
/// it, so it neither moves attention nor orders later focus events. NVDA
/// has no rule across its backends like it: each backend's own duplicate
/// test, which the outposts and the focus listener make, is its only one,
/// and a foreground or menu focus has none.
///
/// The last focus Core queued is the focus it holds: every focus Core
/// accepts becomes it, and a focus it refuses (stale, not in the
/// foreground, or a foreground report for the window that already holds
/// the focus) changes nothing, as NVDA never queues those. A focus from
/// another application in between is a different focus, so the one after
/// it is announced.
///
/// The evidence of "same element, still focused" is the node id, which
/// names one element in one outpost incarnation. An outpost reports a UIA
/// focus under the node its runtime id already has only when it read,
/// live, that the element that node stands for still has the keyboard
/// focus, and otherwise issues a new node; it reports an MSAA focus only
/// once the object or an ancestor has the focused state, read live, and
/// drops an event naming the address of its last focus itself. A focus
/// the outpost could not find its element for keeps its runtime id's node
/// with no such read, so for it the node id alone is the evidence. When
/// the outpost marks a focus as its last focus still focused, read from
/// the event's own element (section 4.2's outpost half), that mark
/// replaces the node id here.
fn is_duplicate_focus(state: &SrState, node: &NodeSnapshot) -> bool {
    state.focus.as_ref().is_some_and(|focus| {
        repeats_focus(RepeatEvidence {
            settled: true,
            same_element: focus.snapshot.id == node.id,
            still_focused: focus.alive,
        })
    })
}

/// Keeps a duplicate focus's newer reading, unspoken: its states and name,
/// which an earlier report can have read mid-change, and its ancestors when
/// they were read in time. The selected item already announced is kept, so
/// a selection event for a new one is still spoken; a navigator the user
/// moved away stays where it is.
fn keep_newer_reading(
    state: &mut SrState,
    node: &NodeSnapshot,
    ancestors: &[NodeSnapshot],
    ancestors_unknown: bool,
) {
    let Some(focus) = state.focus.as_mut() else {
        return;
    };
    focus.reported_focused |= node.states.contains(State::Focused);
    focus.snapshot = node.clone();
    if !ancestors_unknown {
        focus.ancestors = Arc::from(ancestors);
    }
}

/// Whether an event from elsewhere, observed at `observed_at_ms`, was
/// observed before the newest foreground change Core applied: its speech
/// would have been queued before that change, which cancels it in NVDA
/// (`docs/design/focus-pipeline.md`, section 7). An event with no
/// observation time (0) is never judged so.
fn observed_before_foreground_change(state: &SrState, observed_at_ms: u64) -> bool {
    observed_at_ms != 0
        && state
            .foreground_changed_at
            .is_some_and(|changed_at| observed_at_ms < changed_at)
}

/// Whether an event's window is one the attention record covers: the same
/// top-level window, the same root owner, a topmost window, or a
/// `Windows.UI.Core` window under the input thread's active window — NVDA's
/// foreground test, made against the attention record — or a window its
/// outpost found in the system's foreground window when it read the event.
/// When either side has no window facts there is nothing to compare, so the
/// application decides. Focus events with window facts are judged by
/// [`ForegroundRelation::related`] instead.
fn window_is_attended(attention: &Attention, source: Pid, window: Option<&WindowFacts>) -> bool {
    match (attention.window.as_ref(), window) {
        (Some(attended), Some(event)) => {
            event.top_level == attended.top_level
                || event.root_owner == attended.root_owner
                || event.topmost
                || event.under_active_window == Some(true)
                || event.in_foreground
        }
        _ => source == attention.source,
    }
}

/// Speaks an activation's outcome, as NVDA's review activate does: the
/// action ("Activate") when something was activated, else "No action".
/// Routes the answer to a text request to whatever made it: a caret key
/// waiting for evidence, a focus's selected text, a review or text command,
/// or say-all. An answer
/// nothing waits for any more (superseded by a newer request, or a
/// say-all's caret movement) is dropped.
fn reduce_text_completed(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: QueryId,
    reply: verbatim_model::TextReply,
) -> Vec<Effect> {
    // The watch of the held key last passed on has its answer, whatever it
    // is: the held key waiting behind it may have its caret read now.
    let resume = state.held_keys.awaiting_watch == Some(query_id);
    if resume {
        state.held_keys.awaiting_watch = None;
    }
    let mut effects = text_completed(state, trace_id, query_id, reply);
    if resume {
        effects.extend(editing::held_watch_answered(state));
    }
    effects
}

/// [`reduce_text_completed`]'s dispatch, by the request answered; also
/// how Core answers a request for a terminal's text from its memory.
pub(crate) fn text_completed(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: QueryId,
    reply: verbatim_model::TextReply,
) -> Vec<Effect> {
    if let Some(reading) = state
        .held_keys
        .reading
        .take_if(|reading| reading.query_id == query_id)
    {
        return editing::held_read(state, reading, reply);
    }
    if let Some(pending) = state
        .pending_caret
        .take_if(|pending| pending.query_id == query_id)
    {
        return editing::caret_reply(state, trace_id, &pending, reply);
    }
    if let Some((_, node)) = state
        .superseded_caret
        .take_if(|(superseded, _)| *superseded == query_id)
    {
        return editing::superseded_reply(state, trace_id, node, &reply);
    }
    if let Some(pending) = state
        .pending_text
        .take_if(|pending| pending.query_id == query_id)
    {
        if matches!(
            pending.then,
            TextFollowUp::NavigatorSelection(_) | TextFollowUp::NavigatorLine(_)
        ) {
            return editing::navigator_text_reply(state, trace_id, &pending, reply);
        }
        return review_text::reply(state, trace_id, pending, reply);
    }
    if say_all::is_pending(state, query_id) {
        return say_all::reply(state, trace_id, reply);
    }
    if terminal_model::is_pending(state, query_id) {
        return terminal_model::reply(state, trace_id, query_id, reply);
    }
    if terminal::is_pending(state, query_id) {
        return terminal::reply(state, trace_id, query_id, reply);
    }
    Vec::new()
}

fn reduce_activation_completed(
    trace_id: TraceId,
    activated: bool,
    action: Option<&ActionName>,
) -> Vec<Effect> {
    let segment = match (activated, action) {
        (false, _) => UtteranceSegment::new(SegmentContent::Message(Message::NoAction)),
        (true, Some(ActionName::Named(name))) if !name.trim().is_empty() => {
            UtteranceSegment::text(name.clone())
        }
        (true, Some(ActionName::Invoke)) => {
            UtteranceSegment::new(SegmentContent::Message(Message::Invoke))
        }
        (true, _) => UtteranceSegment::new(SegmentContent::Message(Message::Activate)),
    };
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments: vec![segment],
        source: None,
        say_all: false,
        validity: None,
    })]
}

/// Whether `event` is a focus dropped before anything else judges it: a
/// duplicate of the last focus Core queued, whose newer reading is kept
/// unspoken, a stale focus from another outpost, or a focus an
/// application's rule refuses whole ([`crate::applications::refuses_focus`]).
fn focus_dropped_first(
    state: &mut SrState,
    observed: (u64, u64),
    window: Option<&WindowFacts>,
    event: &NormalizedEvent,
) -> bool {
    let NormalizedEvent::FocusChanged {
        node,
        foreground,
        ancestors,
        ancestors_unknown,
        text,
        ..
    } = event
    else {
        return false;
    };
    if !foreground && crate::applications::refuses_focus(window, node) {
        return true;
    }
    if is_duplicate_focus(state, node) {
        let (ancestors, ancestors_unknown) =
            focus_ancestors(window, node, (ancestors, *ancestors_unknown));
        keep_newer_reading(state, node, ancestors, ancestors_unknown);
        sync_navigator_with_focus(state);
        editing::keep_focus_caret(state, node.id, text.as_deref(), observed.0);
        return true;
    }
    is_stale_focus(state, node.id.outpost(), observed, window)
}

/// A focus's ancestors and whether they are unknown, as Core takes them:
/// as the event reports them, but none, and known to be none, for an item
/// whose containers an application's rule drops
/// ([`crate::applications::drops_ancestry`]).
fn focus_ancestors<'a>(
    window: Option<&WindowFacts>,
    node: &NodeSnapshot,
    (ancestors, unknown): (&'a [NodeSnapshot], bool),
) -> (&'a [NodeSnapshot], bool) {
    if crate::applications::drops_ancestry(window, node) {
        (&[], false)
    } else {
        (ancestors, unknown)
    }
}

/// A caret report of `node`: keeps Core's caret current and speaks the
/// selection changes it carries, a selection no key made
/// (`editing::selection_changed`). The focus's own caret comes with the
/// focus (`editing::focus_text`).
fn reduce_caret_moved(
    state: &mut SrState,
    (trace_id, observed_at_ms): (TraceId, u64),
    moved: &NormalizedEvent,
) -> Vec<Effect> {
    let NormalizedEvent::CaretMoved {
        node_id: node,
        caret,
        selection_changes,
        content_changed,
        new_selection,
    } = moved
    else {
        return Vec::new();
    };
    let node = *node;
    let mut effects = Vec::new();
    editing::update_caret(state, node, caret.clone(), observed_at_ms);
    let general = content_changed.then_some(editing::General {
        selection: new_selection.as_ref(),
        collapsed: caret.selection.is_none(),
    });
    effects.extend(editing::selection_changed(
        state,
        trace_id,
        node,
        (selection_changes, general),
    ));
    effects
}

/// What a focused terminal said, or one read of it, which Core's terminal
/// model aligns.
fn terminal_event(
    state: &mut SrState,
    trace_id: TraceId,
    observed_at_ms: u64,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    match event {
        NormalizedEvent::TerminalSpeech { node_id, speech } => terminal::speech(
            state,
            trace_id,
            observed_at_ms,
            *node_id,
            terminal::Said::from(speech.clone()),
        ),
        NormalizedEvent::TerminalSnapshot { node_id, snapshot } => {
            terminal_model::snapshot_event(state, trace_id, *node_id, snapshot)
        }
        _ => Vec::new(),
    }
}

fn reduce_event(
    state: &mut SrState,
    trace_id: TraceId,
    observed: (u64, u64),
    source: Pid,
    window: Option<&WindowFacts>,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    if focus_dropped_first(state, observed, window, event) {
        return Vec::new();
    }
    let observed_at_ms = observed.0;
    match classify(
        state.attention.as_ref(),
        state.focus_source(),
        source,
        window,
        event,
    ) {
        Acceptance::Dropped => return Vec::new(),
        Acceptance::Background => return reduce_background(state, trace_id, observed_at_ms, event),
        Acceptance::Attended => {}
    }
    if let NormalizedEvent::FocusChanged { node, .. } = event
        && observed_at_ms != 0
    {
        state.latest_focus = Some(LatestFocus {
            outpost: node.id.outpost(),
            observed_at_ms,
            top_level: window.map(|window| window.top_level),
            observed_at_us: observed.1,
        });
    }

    let effects = match event {
        NormalizedEvent::FocusChanged {
            node,
            foreground,
            ancestors,
            ancestors_unknown,
            selected_child,
            text,
        } => {
            let (ancestors, ancestors_unknown) =
                focus_ancestors(window, node, (ancestors, *ancestors_unknown));
            reduce_focus_changed(
                state,
                trace_id,
                source,
                window,
                &FocusReport {
                    node,
                    observed_at_ms,
                    observed_at_us: observed.1,
                    foreground: *foreground,
                    ancestors,
                    ancestors_unknown,
                    selected_child: selected_child.as_ref(),
                    text: text.as_deref(),
                },
            )
        }
        NormalizedEvent::SelectionChanged { node } => {
            reduce_selection_changed(state, trace_id, node)
        }
        NormalizedEvent::ProgressChanged { bar } => reduce_progress_changed(state, trace_id, bar),
        NormalizedEvent::ControlledSelection { controller, node } => {
            reduce_controlled_selection(state, trace_id, *controller, node)
        }
        NormalizedEvent::Notification {
            node_id: _,
            notification,
        } => reduce_notification(trace_id, notification),
        NormalizedEvent::Alert { node, .. } => reduce_alert(trace_id, node),
        NormalizedEvent::ValueChanged { node_id, value } => {
            reduce_value_changed(state, trace_id, *node_id, value.clone())
        }
        moved @ NormalizedEvent::CaretMoved { .. } => {
            reduce_caret_moved(state, (trace_id, observed_at_ms), moved)
        }
        NormalizedEvent::TerminalSpeech { .. } | NormalizedEvent::TerminalSnapshot { .. } => {
            terminal_event(state, trace_id, observed_at_ms, event)
        }
        NormalizedEvent::PropertyChanged {
            node_id,
            change,
            child_count,
        } => match change {
            PropertyChange::Name(name) => {
                reduce_name_changed(state, trace_id, *node_id, name.as_ref())
            }
            PropertyChange::Value(value) => {
                reduce_value_changed(state, trace_id, *node_id, value.clone())
            }
            PropertyChange::States(new_states) => {
                states_event(state, trace_id, *node_id, *new_states, *child_count)
            }
            PropertyChange::Description(description) => {
                reduce_description_changed(state, trace_id, *node_id, description.as_ref())
            }
            PropertyChange::Suggestions(open) => reduce_suggestions(state, *node_id, *open),
            // `PropertyChange` is `#[non_exhaustive]`.
            _ => Vec::new(),
        },
        // A text change says nothing of its own: typing is echoed as it is
        // typed, and a terminal's outpost reads its text changes itself.
        // `NormalizedEvent` is `#[non_exhaustive]`.
        _ => Vec::new(),
    };
    sync_navigator_with_focus(state);
    effects
}

/// Keeps the navigator's copy of the focus current: a name, value, or state
/// change, or a fresher report of the same focus, applies to the navigator
/// too while it rests on the focus, so reporting the current object or
/// reviewing it reads what the object is now, as NVDA reads it live. The
/// review position is kept where it is still inside the text.
fn sync_navigator_with_focus(state: &mut SrState) {
    let (Some(focus), Some(navigator)) = (state.focus.as_ref(), state.navigator.as_mut()) else {
        return;
    };
    if navigator.object.id != focus.snapshot.id || navigator.object == focus.snapshot {
        return;
    }
    navigator.object = focus.snapshot.clone();
    let text = review::text_of(&navigator.object);
    if !text.is_char_boundary(navigator.review_offset) {
        navigator.review_offset = 0;
    }
}

/// Speaks an event accepted from outside the attention record. Background
/// events never move focus or the navigator and always queue behind current
/// speech: a toast alert, and the shell's window-snap results notification.
/// One observed before the newest foreground change is dropped unspoken
/// ([`observed_before_foreground_change`]).
fn reduce_background(
    state: &SrState,
    trace_id: TraceId,
    observed_at_ms: u64,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    if observed_before_foreground_change(state, observed_at_ms) {
        return Vec::new();
    }
    if let NormalizedEvent::Alert { node, .. } = event {
        return reduce_alert(trace_id, node);
    }
    let NormalizedEvent::Notification { notification, .. } = event else {
        return Vec::new();
    };
    let Some(text) = notification
        .display_string
        .as_ref()
        .filter(|display| !display.is_empty())
    else {
        return Vec::new();
    };
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments: vec![UtteranceSegment::text(text.clone())],
        source: None,
        say_all: false,
        validity: None,
    })]
}

/// The parts of a `FocusChanged` event the focus handling reads.
struct FocusReport<'a> {
    node: &'a NodeSnapshot,
    /// When its outpost observed it; 0 when not known.
    observed_at_ms: u64,
    /// The same time in microseconds; 0 when not known.
    observed_at_us: u64,
    foreground: bool,
    ancestors: &'a [NodeSnapshot],
    /// The outpost could not read the ancestors in time.
    ancestors_unknown: bool,
    selected_child: Option<&'a NodeSnapshot>,
    /// What the outpost's caret read found, for a focus that may have text.
    text: Option<&'a FocusText>,
}

/// Handles an accepted focus change (`docs/parity/focus-announcement.md`,
/// "Focus announcement content and property order"):
///
/// - A foreground change moves attention to its application and window, and
///   is otherwise ignored when focus is already in that window, compared by
///   window handle because the same window has different node ids in
///   different outposts. Only a foreground change moves attention, as only
///   the system's foreground window counts for NVDA: a topmost popup menu
///   takes focus without becoming the foreground, and focus returning from
///   it must still be attended.
/// - A foreground window with no name becomes the focus silently: a bare
///   "window" says nothing, and the window is not announced later.
/// - While the focus is dead (its outpost ended), a report of the same focus
///   from a replacement outpost is taken silently.
/// - A focus the same as the one already announced has been dropped before
///   this, by NVDA's duplicate rule ([`is_duplicate_focus`]).
/// - Otherwise the newly entered containers, the node, and a selection
///   container's selected item are spoken, queued behind current speech
///   (which the key press that moved focus has already cut off), and the
///   navigator follows focus. When the focus's top-level window changed,
///   speech is cut off first, and the window, its outermost ancestor, is
///   one of the entered containers, spoken by the same rule as the others
///   whether or not a foreground report came for it (`docs/parity/focus-context.md`,
///   "Window announcement on switching applications"). When the outpost
///   could not read the ancestors in time, no container is announced and
///   the previous focus's chain is kept for the next comparison, so the
///   next focus does not announce every container again.
fn reduce_focus_changed(
    state: &mut SrState,
    trace_id: TraceId,
    source: Pid,
    window: Option<&WindowFacts>,
    report: &FocusReport<'_>,
) -> Vec<Effect> {
    if report.foreground {
        state.attention = Some(Attention {
            source,
            window: window.cloned(),
        });
        if let Some(focus) = state.focus.as_ref()
            && let (Some(focus_window), Some(window)) = (focus.window.as_ref(), window)
            && focus_window.top_level == window.top_level
        {
            return Vec::new();
        }
    } else if let Some(window) = window {
        follow_an_unreported_foreground(state, source, window, report);
    }

    let ancestors = if report.ancestors_unknown {
        state
            .focus
            .as_ref()
            .map(|focus| Arc::clone(&focus.ancestors))
            .unwrap_or_default()
    } else {
        Arc::from(report.ancestors)
    };
    // A focus that arrives with no window facts was accepted only because
    // it came from the attended application (`window_is_attended` has
    // nothing else to compare), so it is in the attended window, and that
    // window is recorded as its own. It happens when an outpost builds a
    // UIA focus from its event while the application is too busy for the
    // focused element, and so its window, to be read in time. Without a
    // window, the check above cannot tell that a later foreground report for
    // that same window is the window already holding the focus, and the
    // window would replace the control as the focus and be announced over
    // it. NVDA never meets this: it always has the focused element, and
    // from it the nearest window handle.
    let focus_window = window.cloned().or_else(|| {
        state
            .attention
            .as_ref()
            .filter(|attention| attention.source == source)
            .and_then(|attention| attention.window.clone())
    });
    let new_focus = FocusContext {
        source,
        window: focus_window.clone(),
        snapshot: report.node.clone(),
        ancestors,
        last_selection: report.selected_child.map(|selected| selected.id),
        alive: true,
        reported_focused: report.node.states.contains(State::Focused),
        trace_id: Some(trace_id),
        observed: (report.observed_at_ms, report.observed_at_us),
    };

    if let Some(focus) = state.focus.as_ref()
        && !focus.alive
        && reads_the_same(
            focus,
            report.node,
            (!report.ancestors_unknown).then_some(report.ancestors),
        )
    {
        // The silent re-read: the replacement outpost reports the focus the
        // user already heard, so take its ids without speaking.
        state.focus = Some(new_focus);
        state.focuses_queued += 1;
        if state.navigator.is_none() {
            state.navigator = Some(Navigator::on(report.node.clone()));
        }
        editing::keep_focus_caret(state, report.node.id, report.text, report.observed_at_ms);
        return Vec::new();
    }

    // The review cursor follows focus (roadmap M3): every focus change snaps
    // the navigator object to the new focus and resets the review cursor to
    // its start. This deliberately does not touch `latest_navigation`: an
    // app-initiated focus event is not newer user intent than an
    // object-navigation command already in flight, so a completion for that
    // command must still be free to apply once it lands (see
    // `SrState::latest_navigation`).
    let navigator = Navigator::on(report.node.clone());
    // When speech is cut off (`docs/nvda/speech.md`, "Cancellation", and
    // `docs/nvda/events.md`): focus speech is queued, not interrupting. A
    // new foreground window cancels speech, nameless or not, and so does
    // entering a menu; speech for a focus the user has since left is
    // dropped by the speech manager, which is told where the focus now is.
    let foreground_changed =
        foreground_changed(state.focus.as_ref(), report, focus_window.as_ref());
    if foreground_changed && report.observed_at_ms != 0 {
        state.foreground_changed_at = Some(report.observed_at_ms);
    }
    if report.foreground {
        state.foreground = Some(report.node.id);
    }
    let entered = entered_ancestors(state.focus.as_ref(), window, report);
    let mut effects = vec![Effect::DropExpiredSpeech(FocusNow {
        focus: report.node.id,
        ancestors: new_focus
            .ancestors
            .iter()
            .map(|ancestor| ancestor.id)
            .collect(),
        foreground: state.foreground,
        menu_item_of_focus: menu_item_of_focus(state.focus.as_ref(), report.node),
    })];
    // Entering a menu cuts speech too, at its place among the entered
    // containers (`focus_speech`).
    end_text_activity(state, report.node.id, foreground_changed, &mut effects);
    state.focus = Some(new_focus);
    state.focuses_queued += 1;
    state.navigator = Some(navigator);
    // A nameless foreground window, and any focus in the shell's staging
    // windows, say nothing; speech was cut above if the foreground window
    // changed.
    if report.foreground && !has_text(report.node.name.as_deref())
        || crate::applications::is_shell_staging_window(window)
    {
        return effects;
    }

    // A new focus is a new object, whose formatting has not been reported.
    state.reported_format = None;
    let speech = focus_speech(state, trace_id, report, entered);
    effects.extend(speech);
    effects.extend(editing::focus_text(
        state,
        trace_id,
        report.node,
        (report.text, report.observed_at_ms),
    ));
    effects
}

/// Moves attention and the foreground to a focus in the system's
/// foreground window that is unrelated to the attention window: the
/// foreground moved without a foreground fact, so attention follows. The
/// foreground object moves with it, to the top of the focus's ancestry,
/// its top-level window, so speech for the window left behind is no
/// longer held valid as the foreground's; NVDA asks for the real
/// foreground window instead and takes the ancestry only when that fails
/// (`docs/nvda/events.md`, step 3 of a focus change).
/// With the ancestry unread, the new foreground is not known.
fn follow_an_unreported_foreground(
    state: &mut SrState,
    source: Pid,
    window: &WindowFacts,
    report: &FocusReport<'_>,
) {
    let attended = state
        .attention
        .as_ref()
        .and_then(|attention| attention.window.as_ref())
        .is_some_and(|attended| {
            attended.top_level == window.top_level || attended.root_owner == window.root_owner
        });
    if !window.in_foreground || attended {
        return;
    }
    state.attention = Some(Attention {
        source,
        window: Some(window.clone()),
    });
    state.foreground = if report.ancestors_unknown {
        None
    } else {
        report.ancestors.first().map(|top| top.id)
    };
}

/// Ends what the previous focus's text was doing when the focus moves to
/// `node`: a say-all reading it (its speech is cut off too, since say-all's
/// speech is not focus speech the speech manager would drop), a caret key
/// still waiting for evidence (the focus announcement wins, as NVDA's wait
/// gives way to a pending focus event), and the typing being echoed. Speech
/// is cut off when `cut` says so or say-all was reading.
fn end_text_activity(state: &mut SrState, node: NodeId, cut: bool, effects: &mut Vec<Effect>) {
    let reading = state.say_all.is_some();
    effects.extend(say_all::stop(state));
    if cut || reading {
        effects.push(Effect::StopSpeech);
    }
    state.pending_caret = None;
    state.typed_word.clear();
    crate::terminal::focus_moved(state, node);
    if state.caret.as_ref().is_some_and(|caret| caret.node != node) {
        state.forget_caret();
    }
}

/// Whether a new focus brings a new foreground window, which cancels
/// speech: a foreground report, or a focus in another top-level window
/// than the previous focus (NVDA's foreground event, run when the top of
/// the focus ancestry changes).
fn foreground_changed(
    previous: Option<&FocusContext>,
    report: &FocusReport<'_>,
    window: Option<&WindowFacts>,
) -> bool {
    let previous_top = previous
        .and_then(|focus| focus.window.as_ref())
        .map(|window| window.top_level);
    report.foreground
        || match (previous_top, window.map(|window| window.top_level)) {
            (Some(old), Some(new)) => old != new,
            (None, Some(_)) => true,
            _ => false,
        }
}

/// The speech for a new focus: each entered container as its own
/// utterance, outermost first, valid while the focus is inside it, an
/// entered menu cutting speech in its place instead ([`is_menu`]), then the
/// focus, valid while it is the focus. When the focus has entered another
/// top-level window, the window is the outermost entered container, spoken
/// first whenever it is presentable, whether or not a foreground report
/// came for it, as NVDA speaks a window it took from the focus's ancestry
/// ([`is_presentable_container`]). A focus that may have text leaves its
/// value out, since its text follows (`editing::focus_text`).
fn focus_speech(
    state: &mut SrState,
    trace_id: TraceId,
    report: &FocusReport<'_>,
    entered: Vec<&NodeSnapshot>,
) -> Vec<Effect> {
    let last_level = &mut state.last_tree_level;
    let reads_text = editing::may_have_text(report.node.role);
    let mut effects = Vec::new();
    for container in entered {
        // Entering a menu bar, menu, or menu item cuts speech and says
        // nothing, in its place among the entered containers, outermost
        // first: speech for a container entered outside the menu is cut
        // too (`docs/parity/changes-and-selection.md`, "Entered menus").
        if is_menu(container) {
            effects.push(Effect::StopSpeech);
            continue;
        }
        if !is_presentable_container(container) {
            continue;
        }
        let segments = container_segments(container);
        if segments.is_empty() {
            continue;
        }
        effects.push(Effect::Speak(Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(source_of(container)),
            say_all: false,
            validity: Some(FocusValidity {
                node: container.id,
                had_focus: false,
            }),
        }));
    }
    let mut segments = if reads_text && report.node.value.is_some() {
        let mut node = report.node.clone();
        node.value = None;
        node_segments(&node, Reason::Focus, last_level)
    } else {
        node_segments(report.node, Reason::Focus, last_level)
    };
    // A selection container introduces its selected item right after
    // itself — the roadmap's "announce a focused list's selected item".
    if let Some(selected) = report.selected_child {
        segments.extend(node_segments(selected, Reason::Focus, last_level));
    }
    effects.push(Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments,
        source: Some(source_of(report.node)),
        say_all: false,
        validity: Some(FocusValidity {
            node: report.node.id,
            had_focus: true,
        }),
    }));
    effects
}

/// The focus the user is leaving, `previous`, when its speech stays valid
/// although the focus moves to `node`: an MSAA menu item whose ancestors
/// hold `node`, an MSAA popup menu, as when an application focuses a
/// submenu's first item and then the submenu itself (`docs/nvda/speech.md`,
/// "Expired focus speech").
fn menu_item_of_focus(previous: Option<&FocusContext>, node: &NodeSnapshot) -> Option<NodeId> {
    let previous = previous?;
    let item = &previous.snapshot;
    (item.backend == Backend::Msaa
        && item.role == Role::MenuItem
        && node.backend == Backend::Msaa
        && node.role == Role::Menu
        && previous
            .ancestors
            .iter()
            .any(|ancestor| ancestor.id == node.id))
    .then_some(item.id)
}

/// Whether `node` is a menu bar, a menu, or a menu item, whose entering
/// cuts speech and says nothing.
fn is_menu(node: &NodeSnapshot) -> bool {
    matches!(node.role, Role::MenuBar | Role::Menu | Role::MenuItem)
}

/// Whether a name or description has real, non-whitespace text.
fn has_text(text: Option<&str>) -> bool {
    text.is_some_and(|text| !text.trim().is_empty())
}

/// Whether a reported focus reads the same as the dead focus's kept copy:
/// role, name, value, and states, and the ancestors' names and roles. Node
/// ids are not compared, since a replacement outpost issues new ones.
/// Ancestors reported unknown (`None`) count as unchanged, as the focus
/// keeps the previous chain when its ancestors are unknown
/// (`docs/parity/focus-events.md`, "Recovery after an outpost is replaced"
/// and "A focus whose element is gone before its read").
fn reads_the_same(
    focus: &FocusContext,
    node: &NodeSnapshot,
    ancestors: Option<&[NodeSnapshot]>,
) -> bool {
    let kept = &focus.snapshot;
    kept.role == node.role
        && kept.name == node.name
        && kept.value == node.value
        && kept.states == node.states
        && ancestors.is_none_or(|ancestors| {
            focus.ancestors.len() == ancestors.len()
                && focus
                    .ancestors
                    .iter()
                    .zip(ancestors)
                    .all(|(old, new)| old.role == new.role && old.name == new.name)
        })
}

/// Handles the end of an outpost incarnation: the focus keeps its copied
/// data but its ids are dead, and a navigator or pending navigation in that
/// outpost is cleared. Navigation then does nothing until focus is reported
/// again.
fn outpost_ended(state: &mut SrState, outpost: OutpostId) -> Vec<Effect> {
    if let Some(focus) = state.focus.as_mut()
        && focus.snapshot.id.outpost() == outpost
    {
        focus.alive = false;
    }
    // Text positions die with the outpost that minted them.
    let mut effects = Vec::new();
    if state
        .say_all
        .as_ref()
        .is_some_and(|say_all| say_all.node.outpost() == outpost)
    {
        effects = say_all::stop(state);
    }
    if state
        .caret
        .as_ref()
        .is_some_and(|caret| caret.node.outpost() == outpost)
    {
        state.forget_caret();
    }
    if state
        .pending_caret
        .as_ref()
        .is_some_and(|pending| pending.node.outpost() == outpost)
    {
        state.pending_caret = None;
    }
    if state
        .pending_text
        .as_ref()
        .is_some_and(|pending| pending.node.outpost() == outpost)
    {
        state.pending_text = None;
    }
    terminal_model::outpost_ended(state, outpost);
    if state
        .start_marker
        .is_some_and(|marker| marker.node().outpost() == outpost)
    {
        state.start_marker = None;
    }
    if state
        .navigator
        .as_ref()
        .is_some_and(|navigator| navigator.object.id.outpost() == outpost)
    {
        state.navigator = None;
    }
    if state
        .latest_navigation
        .is_some_and(|pending| pending.from.outpost() == outpost)
    {
        state.latest_navigation = None;
    }
    effects
}

/// A state change of `node_id`, spoken on the focus or an ancestor
/// ([`change_on`]), and on nothing else.
fn states_event(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    new_states: StateSet,
    child_count: Option<u32>,
) -> Vec<Effect> {
    match change_on(state, ChangeKind::States, node_id) {
        Some(ChangeTarget::Focus) => {
            reduce_states_changed(state, trace_id, node_id, new_states, child_count)
        }
        Some(ChangeTarget::Ancestor) => {
            reduce_ancestor_states_changed(state, trace_id, node_id, new_states)
        }
        None => Vec::new(),
    }
}

/// What a change of `kind` on `node_id` concerns, when it is spoken, judged
/// against the live focus Core holds by the rule the outposts judge it by
/// before they read it ([`change_target`]).
fn change_on(state: &SrState, kind: ChangeKind, node_id: NodeId) -> Option<ChangeTarget> {
    let focus = state.focus.as_ref().filter(|focus| focus.alive)?;
    change_target(
        kind,
        node_id,
        focus.snapshot.id,
        focus.ancestors.iter().map(|ancestor| ancestor.id),
        |_| true,
    )
}

/// Handles a name change: when the focused node's name changes, the new name
/// alone is spoken, queued behind current speech, as NVDA does. A name
/// change on any other node, including an ancestor of the focus, is silent.
fn reduce_name_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    name: Option<&String>,
) -> Vec<Effect> {
    if change_on(state, ChangeKind::Name, node_id) != Some(ChangeTarget::Focus) {
        return Vec::new();
    }
    let Some(focus) = state.focus.as_mut() else {
        return Vec::new();
    };
    if focus.snapshot.name.as_ref() == name {
        return Vec::new();
    }
    focus.snapshot.name = name.cloned();
    let Some(text) = name.filter(|name| !name.trim().is_empty()) else {
        return Vec::new();
    };
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments: vec![UtteranceSegment::label(text.clone())],
        source: Some(source_of(&focus.snapshot)),
        say_all: false,
        validity: None,
    })]
}

/// Speaks a toast: the alerting object announced in full, queued behind
/// current speech, as NVDA's notification behavior speaks it. A toast never
/// moves focus or the navigator.
fn reduce_alert(trace_id: TraceId, node: &NodeSnapshot) -> Vec<Effect> {
    // A toast is never a tree or list item, so the level spoken before it
    // plays no part.
    vec![Effect::Speak(announce_node(
        trace_id,
        SpeechPriority::Queued,
        node,
        (Reason::Focus, &mut None),
    ))]
}

/// Handles a UIA `AutomationNotification` event (`docs/parity/alerts.md`,
/// "UIA notification events") from the focus's application: announce the
/// application-supplied display string, if any, and nothing when there is
/// none — a notification with no text has nothing to say. The processing
/// hint sets the priority: `MostRecent` and `ImportantMostRecent` supersede
/// earlier speech and so interrupt; every other kind queues behind current
/// speech, NVDA's exact split. The notification kind and activity id are not
/// used here; the activity id matters only for acceptance (see
/// [`classify`]).
fn reduce_notification(trace_id: TraceId, notification: &Notification) -> Vec<Effect> {
    let Some(text) = notification
        .display_string
        .as_ref()
        .filter(|display| !display.is_empty())
    else {
        return Vec::new();
    };
    let priority = match notification.processing {
        NotificationProcessing::MostRecent | NotificationProcessing::ImportantMostRecent => {
            SpeechPriority::Interrupt
        }
        _ => SpeechPriority::Queued,
    };
    vec![Effect::Speak(Utterance {
        trace_id,
        priority,
        segments: vec![UtteranceSegment::text(text.clone())],
        source: None,
        say_all: false,
        validity: None,
    })]
}

/// Runs a review or object-navigation command (roadmap M3) against the
/// navigator object and its review cursor. A command with no navigator yet
/// (nothing has ever been focused) does nothing. Object-navigation moves
/// (`Parent`, `NextSibling`, `PreviousSibling`, `FirstChild`) are async:
/// they emit a `Fetch` to the owning outpost and the completion moves the
/// navigator; everything else — report, activate, review text motion — is
/// synchronous over state the reducer already holds.
fn reduce_command(
    state: &mut SrState,
    trace_id: TraceId,
    command: ReviewCommand,
    repeat: u8,
) -> Vec<Effect> {
    // Any command stops say-all, as any key does in NVDA; say-all's own
    // commands start it afresh.
    let mut stopped = say_all::stop(state);
    let mut effects = match command {
        ReviewCommand::ToggleFollowCaret
        | ReviewCommand::ToggleTypedCharacters
        | ReviewCommand::ToggleTypedWords => toggle_setting(state, trace_id, command),
        ReviewCommand::ToggleReportNewOutput => terminal::toggle(state, trace_id),
        ReviewCommand::SayAllFromCaret | ReviewCommand::ReportCaretLocation => {
            caret_command(state, trace_id, command)
        }
        ReviewCommand::ReportFocus => report_focus(state, trace_id, repeat),
        _ => navigator_command(state, trace_id, command, repeat),
    };
    stopped.append(&mut effects);
    stopped
}

/// Toggles a reader setting from its key, says its new value as NVDA does,
/// and reports the settings for the shell to save.
fn toggle_setting(state: &mut SrState, trace_id: TraceId, command: ReviewCommand) -> Vec<Effect> {
    let settings = &mut state.settings;
    let segment = match command {
        ReviewCommand::ToggleFollowCaret => {
            settings.follow_caret = !settings.follow_caret;
            SegmentContent::Message(if settings.follow_caret {
                Message::CaretMovesReview
            } else {
                Message::CaretDoesNotMoveReview
            })
        }
        ReviewCommand::ToggleTypedCharacters => {
            settings.speak_typed_characters = settings.speak_typed_characters.next();
            SegmentContent::Phrase(verbatim_model::Phrase::SpeakTypedCharacters(
                settings.speak_typed_characters,
            ))
        }
        _ => {
            settings.speak_typed_words = settings.speak_typed_words.next();
            SegmentContent::Phrase(verbatim_model::Phrase::SpeakTypedWords(
                settings.speak_typed_words,
            ))
        }
    };
    vec![
        editing::speak(trace_id, vec![UtteranceSegment::new(segment)]),
        Effect::SettingsChanged(state.settings),
    ]
}

/// Runs a command on the focus's caret: say-all from the caret, or the
/// caret's location. A focus that cannot have text has no caret.
fn caret_command(state: &mut SrState, trace_id: TraceId, command: ReviewCommand) -> Vec<Effect> {
    let Some(focus) = state.focus.as_ref().filter(|focus| focus.alive) else {
        return vec![editing::speak(
            trace_id,
            review_text::message(Message::NoCaret),
        )];
    };
    let node = focus.snapshot.id;
    let has_text = editing::may_have_text(focus.snapshot.role)
        || state.caret.as_ref().is_some_and(|caret| caret.node == node);
    if !has_text {
        return vec![editing::speak(
            trace_id,
            review_text::message(Message::NotSupported),
        )];
    }
    if command == ReviewCommand::SayAllFromCaret {
        return say_all::start(state, node, true, verbatim_model::TextPoint::Caret);
    }
    let query_id = state.allocate_query_id();
    state.pending_text = Some(crate::state::PendingText {
        query_id,
        node,
        then: crate::state::TextFollowUp::Location,
    });
    vec![Effect::Text(verbatim_model::TextRequest {
        query_id,
        node_id: node,
        op: verbatim_model::TextOp::Location(verbatim_model::TextPoint::Caret),
    })]
}

/// Runs a command on the navigator object and its review cursor.
fn navigator_command(
    state: &mut SrState,
    trace_id: TraceId,
    command: ReviewCommand,
    repeat: u8,
) -> Vec<Effect> {
    // "To focus" is meaningful even with the navigator already on focus;
    // handle it before the navigator-present guard so it can seed one.
    if command == ReviewCommand::ToFocus {
        // NVDA says "Move to focus" before the object.
        let mut effects = navigator_to_focus(state, trace_id);
        if let Some(Effect::Speak(utterance)) = effects.first_mut() {
            utterance.segments.insert(
                0,
                UtteranceSegment::new(SegmentContent::Message(Message::MoveToFocus)),
            );
        }
        return effects;
    }
    let Some(navigator) = state.navigator.as_ref() else {
        return vec![Effect::Speak(Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::new(SegmentContent::Message(
                Message::NoNavigatorObject,
            ))],
            source: None,
            say_all: false,
            validity: None,
        })];
    };

    match command {
        ReviewCommand::ToFocus => unreachable!("handled above"),
        ReviewCommand::ReportObject => report_object(state, trace_id, repeat),
        ReviewCommand::Activate => vec![Effect::Activate {
            node_id: navigator.object.id,
        }],
        ReviewCommand::Parent => navigate(state, trace_id, QueryKind::Parent),
        ReviewCommand::NextSibling => navigate(state, trace_id, QueryKind::NextSibling),
        ReviewCommand::PreviousSibling => navigate(state, trace_id, QueryKind::PreviousSibling),
        ReviewCommand::FirstChild => navigate(state, trace_id, QueryKind::FirstChild),
        command if review_text::handles(command) => {
            match review_text::run(state, trace_id, command, repeat) {
                review_text::Outcome::Done(effects) => effects,
                review_text::Outcome::Flat => flat_review_command(state, trace_id, command, repeat),
            }
        }
        _ => Vec::new(),
    }
}

/// Snaps the navigator (and review cursor) back to the current focus and
/// reports it, and clears `latest_navigation` so a navigation still pending
/// cannot override this explicit return to focus. Does nothing else with
/// nothing focused, or with a focus whose outpost has ended. Also reused to
/// re-seed the navigator when a navigation fetch reports `FetchResult::Gone`
/// (the outpost could not reach the navigator's node): the same "fall back to
/// focus and announce it" behavior applies there too.
fn navigator_to_focus(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    state.latest_navigation = None;
    let Some(focus) = state.focus.as_ref().filter(|focus| focus.alive) else {
        return Vec::new();
    };
    let object = focus.snapshot.clone();
    state.navigator = Some(Navigator::on(object.clone()));
    announce_navigator(state, trace_id, &object, Reason::Focus)
}

/// Announces the navigator object `object` after object navigation or on
/// request: its full announcement, except that an object that may have
/// text leaves its value out and says its text after the rest
/// ([`editing::navigator_text`]), as a focus does.
fn announce_navigator(
    state: &mut SrState,
    trace_id: TraceId,
    object: &NodeSnapshot,
    reason: Reason,
) -> Vec<Effect> {
    let levels = (reason, &mut state.last_tree_level);
    let utterance = if editing::may_have_text(object.role) && object.value.is_some() {
        let mut announced = object.clone();
        announced.value = None;
        announce_node(trace_id, SpeechPriority::Queued, &announced, levels)
    } else {
        announce_node(trace_id, SpeechPriority::Queued, object, levels)
    };
    let mut effects = vec![Effect::Speak(utterance)];
    effects.extend(editing::navigator_text(
        state,
        trace_id,
        object,
        NavigatorRead::Announce,
    ));
    effects
}

/// Reports the navigator object: on the first press its full announcement,
/// on the second its text spelled character by character, on the third its
/// name and value copied to the clipboard (NVDA's multi-press semantics).
fn report_object(state: &mut SrState, trace_id: TraceId, repeat: u8) -> Vec<Effect> {
    let Some(navigator) = state.navigator.as_ref() else {
        return Vec::new();
    };
    match repeat {
        0 => {
            let object = navigator.object.clone();
            announce_navigator(state, trace_id, &object, Reason::Query)
        }
        // An object with text spells or copies its name and the text it
        // would announce, the selection or the caret's line, as NVDA does
        // (`docs/nvda/speech.md`, "What an object with text says").
        1 | 2 if editing::may_have_text(navigator.object.role) => {
            let object = navigator.object.clone();
            let read = if repeat == 1 {
                NavigatorRead::Spell
            } else {
                NavigatorRead::Copy
            };
            editing::navigator_text(state, trace_id, &object, read)
        }
        1 => {
            // NVDA spells the name and value joined by a space.
            let segments = spelled(&clipboard_text(&navigator.object));
            if segments.is_empty() {
                return Vec::new();
            }
            vec![Effect::Speak(Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments,
                source: Some(source_of(&navigator.object)),
                say_all: false,
                validity: None,
            })]
        }
        _ => {
            // Copy the object's name and value; the shell's clipboard
            // helper owns the spoken confirmation (see the memory on a
            // single shared copy path).
            let text = clipboard_text(&navigator.object);
            if text.is_empty() {
                return Vec::new();
            }
            vec![Effect::CopyToClipboard(text)]
        }
    }
}

/// Reports the focus object, leaving the navigator where it is
/// (`docs/nvda/focus-and-navigator.md`, "Reporting the focus"): on the
/// first press as reporting the current object reports the navigator, its
/// text included; on the second its name spelled, "blank" when it has none;
/// on the third and later its name spelled with character descriptions.
/// Says "No focus" with no focus whose outpost is still running.
fn report_focus(state: &mut SrState, trace_id: TraceId, repeat: u8) -> Vec<Effect> {
    let Some(object) = state
        .focus
        .as_ref()
        .filter(|focus| focus.alive)
        .map(|focus| focus.snapshot.clone())
    else {
        return vec![editing::speak(
            trace_id,
            review_text::message(Message::NoFocus),
        )];
    };
    if repeat == 0 {
        return announce_navigator(state, trace_id, &object, Reason::Query);
    }
    let segments =
        review_text::spell_or_blank(object.name.as_deref().unwrap_or_default(), repeat > 1, None);
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments,
        source: Some(source_of(&object)),
        say_all: false,
        validity: None,
    })]
}

/// The text the report-object copy press puts on the clipboard: the
/// object's name and value joined by a space, each included only when
/// present, matching what a user reading the object would expect to paste.
pub(crate) fn clipboard_text(node: &NodeSnapshot) -> String {
    let mut parts = Vec::new();
    if let Some(name) = node.name.as_ref().filter(|name| !name.is_empty()) {
        parts.push(name.clone());
    }
    if let Some(value) = node.value.as_ref().filter(|value| !value.is_empty()) {
        parts.push(value.clone());
    }
    parts.join(" ")
}

/// Emits a navigation fetch for the navigator object's neighbor in the
/// direction `kind` names; the completion (`reduce_fetch_completed`) moves
/// the navigator and announces the result, or reports the edge when there
/// is no such neighbor. Records this fetch's query id as the latest
/// navigation: a second navigation command issued before this one completes
/// supersedes it here, so the first's eventual completion is dropped as
/// stale (see `SrState::latest_navigation`).
fn navigate(state: &mut SrState, _trace_id: TraceId, kind: QueryKind) -> Vec<Effect> {
    let Some(navigator) = state.navigator.as_ref() else {
        return Vec::new();
    };
    let node_id = navigator.object.id;
    let query_id = state.allocate_query_id();
    state.latest_navigation = Some(PendingNavigation {
        query_id,
        from: node_id,
    });
    vec![Effect::Fetch(Query {
        query_id,
        node_id,
        kind,
    })]
}

/// `text` spelled character by character, a space spoken as "space".
pub(crate) fn spelled(text: &str) -> Vec<UtteranceSegment> {
    text::spelled(text, false, None)
}

/// Runs a review-cursor text command over the navigator object's review
/// text (see [`review`]), as NVDA's review commands do: moves the cursor and
/// speaks the line, word, or character it lands on. A motion that cannot
/// move says "Top", "Bottom", "Left", or "Right" and reads the current unit;
/// character motions stop at the ends of the line; an empty unit is
/// "blank". Pressed twice, the current line or word is spelled, and three
/// times spelled with character descriptions; the current character
/// pressed twice gives its description, and three times its code in
/// decimal and hexadecimal. Commands flat text has nothing for (pages, the
/// selection, location) say "Not supported in this document"; the start
/// marker and select then copy work on the flat text
/// (`review_text::flat_extra`).
#[expect(
    clippy::too_many_lines,
    reason = "one motion table and one speech table, which read best together"
)]
pub(crate) fn flat_review_command(
    state: &mut SrState,
    trace_id: TraceId,
    command: ReviewCommand,
    repeat: u8,
) -> Vec<Effect> {
    if let Some(effects) = review_text::flat_extra(state, trace_id, command, repeat) {
        return effects;
    }
    let Some(navigator) = state.navigator.as_mut() else {
        return Vec::new();
    };
    let text = review::text_of(&navigator.object);
    let offset = navigator.review_offset.min(text.len());
    let line = review::line_span(&text, offset);
    let char_at = |at: usize| review::char_span(&text, at).unwrap_or((at, at));

    let (new_offset, spoken, edge) = match command {
        ReviewCommand::ReviewTop => (0, review::line_span(&text, 0), None),
        ReviewCommand::ReviewBottom => {
            let start = review::line_span(&text, text.len()).0;
            (start, review::line_span(&text, start), None)
        }
        ReviewCommand::ReviewPreviousLine => match review::previous_char(&text, line.0) {
            Some(target) => {
                let span = review::line_span(&text, target);
                (span.0, span, None)
            }
            None => (line.0, line, Some(Message::Top)),
        },
        ReviewCommand::ReviewNextLine => match review::next_line_span(&text, line.0) {
            Some(span) => (span.0, span, None),
            None => (line.0, line, Some(Message::Bottom)),
        },
        // Current-line and start-of-line both land the cursor at the line
        // start and read the whole line; the only difference a text model
        // (M4) will draw between them is the reported position, not the
        // spoken text.
        ReviewCommand::ReviewCurrentLine | ReviewCommand::ReviewStartOfLine => (line.0, line, None),
        ReviewCommand::ReviewEndOfLine => (line.1, line, None),
        ReviewCommand::ReviewPreviousWord
        | ReviewCommand::ReviewNextWord
        | ReviewCommand::ReviewCurrentWord => {
            let words = review::words(&text);
            let current = text::word_at(&words, offset)
                .map_or((offset, offset), |word| (word.start, word.end));
            let target = match command {
                ReviewCommand::ReviewPreviousWord => review::previous_word(&words, offset),
                ReviewCommand::ReviewNextWord => review::next_word(&words, offset),
                _ => Some(current.0..current.1),
            };
            match target {
                Some(word) => (word.start, (word.start, word.end), None),
                None if command == ReviewCommand::ReviewPreviousWord => {
                    (offset, current, Some(Message::Top))
                }
                None => (offset, current, Some(Message::Bottom)),
            }
        }
        ReviewCommand::ReviewPreviousCharacter => match review::previous_char(&text, offset) {
            Some(target) if target >= line.0 => (target, char_at(target), None),
            _ => (offset, char_at(offset), Some(Message::Left)),
        },
        ReviewCommand::ReviewNextCharacter => match review::char_span(&text, offset) {
            Some((_, next)) if next < line.1 => (next, char_at(next), None),
            _ => (offset, char_at(offset), Some(Message::Right)),
        },
        ReviewCommand::ReviewCurrentCharacter => (offset, char_at(offset), None),
        _ => return Vec::new(),
    };

    navigator.review_offset = new_offset;
    let (start, end) = spoken;
    let slice = text.get(start..end).unwrap_or("");
    let mut segments: Vec<UtteranceSegment> = edge
        .map(|edge| UtteranceSegment::new(SegmentContent::Message(edge)))
        .into_iter()
        .collect();
    let repeated_current = matches!(
        command,
        ReviewCommand::ReviewCurrentLine
            | ReviewCommand::ReviewCurrentWord
            | ReviewCommand::ReviewCurrentCharacter
    ) && repeat > 0;
    let by_character = matches!(
        command,
        ReviewCommand::ReviewPreviousCharacter
            | ReviewCommand::ReviewNextCharacter
            | ReviewCommand::ReviewCurrentCharacter
    );
    // A unit with nothing to read is "blank"; spelled, or read as one
    // character, a space is "space", as with a text pattern.
    let blank = slice.is_empty() || (!repeated_current && !by_character && text::is_blank(slice));
    if blank {
        segments.push(UtteranceSegment::new(SegmentContent::Message(
            Message::Blank,
        )));
    } else if repeated_current && command == ReviewCommand::ReviewCurrentCharacter && repeat > 1 {
        // The character's code, in decimal and then spelled in hexadecimal.
        for ch in slice.chars() {
            let code = u32::from(ch);
            segments.push(UtteranceSegment::text(format!("{code},")));
            segments.extend(spelled(&format!("{code:#x}")));
        }
    } else if repeated_current && command == ReviewCommand::ReviewCurrentCharacter {
        // The character's description, from the character table.
        for range in verbatim_text::graphemes(slice) {
            segments.push(UtteranceSegment::new(SegmentContent::CharacterDescription(
                slice[range].to_owned(),
            )));
        }
    } else if repeated_current {
        // Spelled, and on a third press spelled with descriptions.
        segments.extend(text::spelled(slice, repeat > 1, None));
    } else if by_character {
        // A single character is spoken as NVDA spells it, so a capital is
        // raised in pitch.
        segments.extend(spelled(slice));
    } else if matches!(
        command,
        ReviewCommand::ReviewPreviousWord
            | ReviewCommand::ReviewNextWord
            | ReviewCommand::ReviewCurrentWord
    ) {
        // A word of one character, a punctuation mark, by its name.
        segments.extend(text::word_segments(slice, None));
    } else {
        segments.push(UtteranceSegment::text(slice.to_owned()));
    }
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments,
        source: None,
        say_all: false,
        validity: None,
    })]
}

/// Handles a `SelectionChanged` event: a node was selected within its
/// container. Announced only when the selection happened under the focused
/// container — the event's outpost is the focused node's, focus
/// sits on a selection container, and the selected node is neither the
/// focused node itself nor the item most recently announced (the focus
/// event's own `selected_child`, or the previous selection event). This is
/// NVDA's generic selection behavior: arrowing through a list whose focus
/// stays on the container speaks each newly selected item, and everything
/// else stays quiet.
fn reduce_selection_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node: &NodeSnapshot,
) -> Vec<Effect> {
    let Some(focus) = state.focus.as_mut().filter(|focus| focus.alive) else {
        return Vec::new();
    };
    let target = selection_target(
        node.id,
        (focus.snapshot.id, focus.snapshot.role),
        focus.ancestors.iter().map(|ancestor| ancestor.id),
        |_| true,
    );
    match target {
        // Selecting the focused item itself is a change of its state, as
        // NVDA handles a selection on the focus: "selected" is spoken.
        Some(SelectionTarget::FocusStates) => {
            let states = node.states;
            return reduce_states_changed(state, trace_id, node.id, states, None);
        }
        // So is selecting one of the focus's ancestors, as NVDA's base
        // state change handler speaks it.
        Some(SelectionTarget::AncestorStates) => {
            return reduce_ancestor_states_changed(state, trace_id, node.id, node.states);
        }
        Some(SelectionTarget::Item) if focus.last_selection != Some(node.id) => {}
        _ => return Vec::new(),
    }
    focus.last_selection = Some(node.id);
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments: node_segments(node, Reason::Focus, &mut state.last_tree_level),
        source: Some(source_of(node)),
        say_all: false,
        validity: None,
    })]
}

/// A node selected inside an element the focus controls ("Selection in a
/// list the focus controls" in `docs/nvda/events.md`): spoken as a focus,
/// interrupting, with the navigator moved to it, while `controller` is
/// still the live focus. The focus itself does not move.
fn reduce_controlled_selection(
    state: &mut SrState,
    trace_id: TraceId,
    controller: NodeId,
    node: &NodeSnapshot,
) -> Vec<Effect> {
    if !state
        .focus
        .as_ref()
        .is_some_and(|focus| focus.alive && focus.snapshot.id == controller)
    {
        return Vec::new();
    }
    state.navigator = Some(Navigator::on(node.clone()));
    vec![Effect::Speak(announce_node(
        trace_id,
        SpeechPriority::Interrupt,
        node,
        (Reason::Focus, &mut state.last_tree_level),
    ))]
}

/// Shared handling for `ValueChanged` and `PropertyChanged(Value(..))`: both
/// speak the bare new value, only when the changed node is the focused one,
/// the value differs from the one last known, and the role speaks its
/// value at all. An edit field or document never speaks its changes of
/// value ("When values and descriptions are spoken" in
/// `docs/nvda/speech.md`).
fn reduce_value_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    value: Option<String>,
) -> Vec<Effect> {
    if change_on(state, ChangeKind::Value, node_id) != Some(ChangeTarget::Focus) {
        return Vec::new();
    }
    let Some(focus) = state.focus.as_mut() else {
        return Vec::new();
    };
    if focus.snapshot.value == value {
        return Vec::new();
    }
    focus.snapshot.value.clone_from(&value);
    let role = focus.snapshot.role;
    let Some(text) = value.filter(|_| speaks_value(role) && !reports_text_itself(role)) else {
        return Vec::new();
    };
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments: vec![UtteranceSegment::value(text)],
        source: Some(source_of(&focus.snapshot)),
        say_all: false,
        validity: None,
    })]
}

/// The percentage a progress bar's value reads as, as NVDA reads it: a
/// number once any percent signs and null characters at either end are
/// removed, held between 0 and 100. `None` for anything else.
fn progress_percentage(value: Option<&str>) -> Option<f64> {
    let number = value?
        .trim_matches(|c| c == '%' || c == '\0')
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|number| number.is_finite())?;
    Some(number.clamp(0.0, 100.0))
}

/// A progress bar's value changed, whether or not it is the focus
/// (`docs/nvda/object-model.md`, "How a progress bar reports its value"):
/// its percentage is indicated at once (`Earcon::Progress`, a tone by
/// default) when it differs by at least one percent from the last one
/// indicated for a progress bar at the same place on the screen, and its
/// value is not spoken as a value change, focus or not. An off-screen
/// progress bar, or one whose value is not a number, is an ordinary value
/// change instead. Background progress bars never get here: their events
/// are not attended.
fn reduce_progress_changed(
    state: &mut SrState,
    trace_id: TraceId,
    progress: &ProgressReading,
) -> Vec<Effect> {
    let percentage = progress_percentage(progress.value.as_deref()).filter(|_| !progress.offscreen);
    let Some(percentage) = percentage else {
        return reduce_value_changed(state, trace_id, progress.id, progress.value.clone());
    };
    if change_on(state, ChangeKind::Value, progress.id) == Some(ChangeTarget::Focus)
        && let Some(focus) = state.focus.as_mut()
    {
        focus.snapshot.value.clone_from(&progress.value);
    }
    let place = progress.rect.map_or((0, 0), |rect| {
        (rect.left + rect.width / 2, rect.top + rect.height / 2)
    });
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a percentage held between 0 and 100"
    )]
    let hundredths = (percentage * 100.0).round() as u32;
    let last = state
        .progress_reported
        .iter()
        .position(|(at, _)| *at == place);
    if let Some(index) = last {
        let (_, reported) = state.progress_reported.remove(index);
        if reported.abs_diff(hundredths) < 100 {
            state.progress_reported.push((place, reported));
            return Vec::new();
        }
    }
    state.progress_reported.push((place, hundredths));
    if state.progress_reported.len() > crate::state::PROGRESS_BARS_KEPT {
        state.progress_reported.remove(0);
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a percentage held between 0 and 100"
    )]
    let whole = percentage as u8;
    vec![Effect::PlayEarcon(Earcon::Progress(whole))]
}

/// Handles a complete state-set replacement on the focused node (MSAA
/// `EVENT_OBJECT_STATECHANGE` and equivalent UIA property changes carry the
/// whole new set, not a delta). Diffs against the stored snapshot and
/// announces, queued, the gained states and the lost states
/// spoken by their absence, by the rules and in the order of "Which states
/// are spoken, and in what order" in `docs/nvda/speech.md`. Ignored for any
/// node other than the focused one; a no-op if nothing speakable changed.
/// When the change makes the focus expanded and carries the node's number
/// of children, which the outpost reads only for a Win32 tree view item,
/// the number follows as an utterance of its own ("How many items an
/// expanded tree view item holds" in `docs/nvda/speech.md`).
fn reduce_states_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    new_states: StateSet,
    child_count: Option<u32>,
) -> Vec<Effect> {
    if change_on(state, ChangeKind::States, node_id) != Some(ChangeTarget::Focus) {
        return Vec::new();
    }
    let Some(focus) = state.focus.as_mut() else {
        return Vec::new();
    };
    let old_states = focus.snapshot.states;
    if old_states == new_states {
        return Vec::new();
    }
    let role = focus.snapshot.role;
    let utterance_source = source_of(&focus.snapshot);
    focus.snapshot.states = new_states;
    // NVDA speaks a state change only while the changed object is its
    // focus, which a focus event changes when NVDA handles it, in the order
    // the events came (`docs/nvda/events.md`, "The focus gate"). An outpost
    // can deliver the state change of the control the focus has just left
    // before the slower focus event for the control it went to, such as a
    // button disabled once the focus has moved off it. The object's own
    // states tell: one that reported itself focused and no longer does has
    // lost the focus, and NVDA, having handled the focus event first, would
    // not speak its change.
    if focus.reported_focused && !new_states.contains(State::Focused) {
        return Vec::new();
    }

    let gained = StateSet::from_iter(
        new_states
            .iter()
            .filter(|state| !old_states.contains(*state)),
    );
    let segments = state_change_segments(role, old_states, new_states);
    let items = child_count
        .filter(|_| gained.contains(State::Expanded))
        .map(|count| {
            vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::Items(count),
            ))]
        });
    [
        Some(segments).filter(|segments| !segments.is_empty()),
        items,
    ]
    .into_iter()
    .flatten()
    .map(|segments| {
        Effect::Speak(Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(utterance_source),
            say_all: false,
            validity: None,
        })
    })
    .collect()
}

/// What a change of a node's states from `old_states` to `new_states` speaks:
/// the gained states and the lost states spoken by their absence, by the
/// rules and in the order of "Which states are spoken, and in what order" in
/// `docs/nvda/speech.md`.
fn state_change_segments(
    role: Role,
    old_states: StateSet,
    new_states: StateSet,
) -> Vec<UtteranceSegment> {
    let gained = StateSet::from_iter(
        new_states
            .iter()
            .filter(|state| !old_states.contains(*state)),
    );
    let lost = StateSet::from_iter(
        old_states
            .iter()
            .filter(|state| !new_states.contains(*state)),
    );
    let positive = intersect(spoken_states(role, new_states, StateReason::Change), gained);
    let mut negative = intersect(negated_states(role, new_states, StateReason::Change), lost);
    // Losing half checked without becoming checked is a change to "not
    // checked".
    if lost.contains(State::Mixed) && !new_states.contains(State::Checked) {
        negative.insert(State::Checked);
    }
    ordered_state_segments(positive, negative)
}

/// A state change on an ancestor of the focus, spoken as NVDA's base state
/// change handler speaks one on the focus's ancestors ("The focus gate" in
/// `docs/nvda/events.md`): pressing a focused button can change the state of
/// a container above it, such as the sort state of a column header. The
/// change is diffed against the ancestor's states as the focus was reported
/// with them, which then become the new ones; queued, as on the focus.
/// Ignored for a node that is neither the focus nor one of its ancestors.
fn reduce_ancestor_states_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    new_states: StateSet,
) -> Vec<Effect> {
    let Some(focus) = state.focus.as_mut().filter(|focus| focus.alive) else {
        return Vec::new();
    };
    let Some(index) = focus
        .ancestors
        .iter()
        .position(|ancestor| ancestor.id == node_id)
    else {
        return Vec::new();
    };
    let ancestor = &focus.ancestors[index];
    let (role, old_states, source) = (ancestor.role, ancestor.states, source_of(ancestor));
    if old_states == new_states {
        return Vec::new();
    }
    let mut ancestors = focus.ancestors.to_vec();
    ancestors[index].states = new_states;
    focus.ancestors = Arc::from(ancestors);
    let segments = state_change_segments(role, old_states, new_states);
    if segments.is_empty() {
        return Vec::new();
    }
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments,
        source: Some(source),
        say_all: false,
        validity: None,
    })]
}

/// A description change on the focus, spoken as NVDA's base handler speaks
/// it ("The focus gate" in `docs/nvda/events.md`): the new description
/// alone, queued, when it differs from the one last known and has text, and
/// is not the focus's name over again. Ignored for any other node.
/// An editable field's suggestions appeared (`open`) or went away: the
/// focus's opening plays the suggestions-opened indication, and any other
/// change the suggestions-closed one, the focus test applying only to an
/// opening (`docs/parity/changes-and-selection.md`, "Suggestions appearing and going away").
fn reduce_suggestions(state: &SrState, node_id: NodeId, open: bool) -> Vec<Effect> {
    if open && state.focus_matches(node_id) {
        vec![Effect::PlayEarcon(Earcon::SuggestionsOpened)]
    } else {
        vec![Effect::PlayEarcon(Earcon::SuggestionsClosed)]
    }
}

fn reduce_description_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    description: Option<&String>,
) -> Vec<Effect> {
    if change_on(state, ChangeKind::Description, node_id) != Some(ChangeTarget::Focus) {
        return Vec::new();
    }
    let Some(focus) = state.focus.as_mut() else {
        return Vec::new();
    };
    if focus.snapshot.details.description.as_ref() == description {
        return Vec::new();
    }
    focus.snapshot.details.description = description.cloned();
    let Some(text) = description
        .filter(|text| !text.trim().is_empty())
        .filter(|text| focus.snapshot.name.as_ref() != Some(*text))
    else {
        return Vec::new();
    };
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments: vec![UtteranceSegment::new(SegmentContent::Description(
            text.clone(),
        ))],
        source: Some(source_of(&focus.snapshot)),
        say_all: false,
        validity: None,
    })]
}

/// Completes an object-navigation fetch.
///
/// Applied if and only if `query_id` is still [`SrState::latest_navigation`]
/// — the most recently issued navigation command, tracked independently of
/// the navigator's identity. A focus event arriving between the command and
/// its completion snaps the navigator (review follows focus) but does not
/// touch `latest_navigation`, so the user's own pending navigation still
/// lands. A second navigation issued before the first completes replaces
/// `latest_navigation`, so the first's late completion is dropped as stale;
/// `ToFocus` clears it outright, so a late completion cannot override the
/// user's explicit return to focus; and the end of the outpost it was sent
/// to clears it too.
///
/// On `FetchResult::Node`, moves the navigator to the returned neighbor and
/// announces it. On `FetchResult::NoNeighbor`, leaves the navigator put and
/// speaks the direction's edge message — NVDA's wording: "No next", "No
/// previous", "No containing object", "No objects inside". Live testing
/// found silence indistinguishable from a broken command, exactly as NVDA's
/// spoken messages predict. On `FetchResult::Gone` — the navigator's node
/// could no longer be reached, distinct from a genuine tree edge — re-seeds
/// the navigator from the current focus and announces it (via
/// [`navigator_to_focus`]) rather than staying silent; if nothing live is
/// focused either, that stays silent too.
fn reduce_navigate_completed(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: QueryId,
    kind: QueryKind,
    result: &FetchResult,
) -> Vec<Effect> {
    if state
        .latest_navigation
        .is_none_or(|pending| pending.query_id != query_id)
    {
        // Superseded by a newer navigation, or cleared by `ToFocus` or the
        // outpost's end: this completion no longer describes user intent
        // worth acting on.
        return Vec::new();
    }
    match result {
        FetchResult::Node(snapshot) => {
            state.latest_navigation = None;
            state.navigator = Some(Navigator::on(snapshot.clone()));
            announce_navigator(state, trace_id, snapshot, Reason::Focus)
        }
        FetchResult::Gone => navigator_to_focus(state, trace_id),
        // The application did not answer: the navigator stays where it is,
        // and nothing is known to speak.
        FetchResult::Unanswered => {
            state.latest_navigation = None;
            Vec::new()
        }
        // A genuine tree edge: the navigator stays put and the edge is
        // spoken (NVDA's wording, chosen per direction). Any future
        // `FetchResult` variant added under `#[non_exhaustive]` stays
        // silent until given a meaning here.
        _ => {
            state.latest_navigation = None;
            let Some(message) = edge_message_of(kind) else {
                return Vec::new();
            };
            vec![Effect::Speak(Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments: vec![UtteranceSegment::new(SegmentContent::Message(message))],
                source: None,
                say_all: false,
                validity: None,
            })]
        }
    }
}

/// The edge message a navigation `QueryKind` speaks when there is no
/// neighbor in its direction — NVDA's messages, one per command.
fn edge_message_of(kind: QueryKind) -> Option<verbatim_model::Message> {
    use verbatim_model::Message;
    match kind {
        QueryKind::Parent => Some(Message::NoContainingObject),
        QueryKind::NextSibling => Some(Message::NoNextObject),
        QueryKind::PreviousSibling => Some(Message::NoPreviousObject),
        QueryKind::FirstChild => Some(Message::NoObjectsInside),
        _ => None,
    }
}

/// The utterance-source metadata describing `node`, for presentation
/// themes (decision D12).
pub(crate) fn source_of(node: &NodeSnapshot) -> UtteranceSource {
    UtteranceSource {
        role: node.role,
        rect: node.details.rect,
    }
}

/// The ancestors of a newly focused node worth announcing as entered
/// context, outermost first: presentable containers (see
/// [`is_presentable_container`]) that the previous focus was not already
/// inside — NVDA's focus-ancestry behavior, where tabbing within one dialog
/// stays quiet about the dialog but entering it announces it.
///
/// An ancestor counts as already entered when the previous focus or one of
/// its ancestors is the same node. Two UI Automation nodes from one outpost
/// are the same node only when their ids are equal, since the outpost gives
/// one element one id, as NVDA compares UI Automation elements by their
/// identity alone: an element replaced by a new one with the same role and
/// name, such as File Explorer's items view list after a folder is opened,
/// is entered anew (`docs/parity/focus-context.md`, "Focus-ancestry
/// context"). Otherwise, when the new focus is in the same top-level window
/// as the previous one, a previous node with the same role and name also
/// counts: node ids are comparable only within one outpost, and one
/// top-level window can hold elements of two processes (a Settings page's
/// frame belongs to `ApplicationFrameHost.exe`), and an MSAA object reached
/// by another path can have another id, where NVDA's MSAA comparison weighs
/// the role and name as well. A focus in a different top-level window, or
/// with no previous focus, enters its whole chain.
///
/// Entered menu bars, menus, and menu items are never announced: NVDA
/// cancels speech and stays silent for them. Only the presentable ones
/// ([`is_presentable_container`]) are spoken. None are entered when the
/// outpost could not read the ancestors in time.
fn entered_ancestors<'a>(
    previous: Option<&FocusContext>,
    window: Option<&WindowFacts>,
    report: &FocusReport<'a>,
) -> Vec<&'a NodeSnapshot> {
    if report.ancestors_unknown {
        return Vec::new();
    }
    let same_window = previous.is_some_and(|previous| {
        matches!((previous.window.as_ref(), window), (Some(old), Some(new)) if old.top_level == new.top_level)
    });
    report
        .ancestors
        .iter()
        .filter(|ancestor| {
            !previous.is_some_and(|previous| {
                already_entered(
                    previous
                        .ancestors
                        .iter()
                        .chain(std::iter::once(&previous.snapshot)),
                    same_window,
                    *ancestor,
                )
            })
        })
        .collect()
}

/// Whether a focus ancestor adds context worth speaking when focus first
/// enters it. The rule is exclusion-based: an ancestor speaks unless it is
/// one of the following.
///
/// - An item-level or text-entry role (`TreeItem`, `ListItem`,
///   `EditableText`). Focus lands on these; they are never context.
/// - A structural node with no semantics of its own (`Unknown`, `Pane`),
///   except a pane that is the client area of a top-level window with a
///   system menu ([`verbatim_model::NodeDetails::top_level_client`]): it
///   stands for its window, which is presented as any entered window is,
///   and is spoken by its title alone, since a pane's role is silent on
///   focus.
/// - A menu bar, menu, or menu item: entering one is silent, as in NVDA.
/// - A `Window`, `Group`, or `PropertyPage` with neither a name nor a
///   description, which would speak as a bare role. NVDA treats these as
///   layout; a named window is announced on entry like any container.
/// - `StaticText` with no text, which is nothing.
///
/// Whitespace-only names and descriptions count as absent. Everything else
/// speaks on entry whether named or not: dialogs, toolbars, and an unnamed
/// tree, which announces as a bare "tree view", the same as NVDA. The
/// behavior is recorded in `docs/parity/focus-context.md`, "Focus-ancestry
/// context".
fn is_presentable_container(node: &NodeSnapshot) -> bool {
    let named = has_text(node.name.as_deref());
    let described = has_text(node.details.description.as_deref());
    match node.role {
        // Never context: item-level and text-entry roles, structural roles
        // with no semantics, and menus. See this function's doc for the rule.
        Role::TreeItem
        | Role::ListItem
        | Role::EditableText
        | Role::ProgressBar
        | Role::BusyIndicator
        | Role::TitleBar
        | Role::Unknown
        | Role::MenuBar
        | Role::Menu
        | Role::MenuItem => false,
        // A pane is structure, unless it is a top-level window's client
        // area read through MSAA, which stands for its window.
        Role::Pane => node.details.top_level_client,
        // Layout-when-unlabeled roles: content only when named or described.
        Role::Window | Role::Group | Role::PropertyPage => named || described,
        // Static text: content only when it has real, non-whitespace text.
        Role::StaticText => named,
        // Every other role is context, named or not.
        _ => true,
    }
}

/// The spoken introduction for one entered container: spoken as a focus
/// is, without its value, its level, or (except for a list) its keyboard
/// shortcut, which NVDA leaves out for an entered container ("When the role
/// is spoken" and "When values and descriptions are spoken" in
/// `docs/nvda/speech.md`).
fn container_segments(node: &NodeSnapshot) -> Vec<UtteranceSegment> {
    let mut entered = node.clone();
    entered.value = None;
    entered.details.level = None;
    if entered.role != Role::List {
        entered.details.keyboard_shortcut = None;
    }
    // An entered container speaks no level, so none is remembered.
    node_segments(&entered, Reason::Focus, &mut None)
}

/// Why a node is being spoken, which decides whether its role is
/// ("When the role is spoken" in `docs/nvda/speech.md`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reason {
    /// Focus moved to it, it was selected in a list the focus controls, it
    /// is a toast, or object navigation moved the navigator to it.
    Focus,
    /// The user asked for it, as with reporting the current object.
    Query,
}

/// Roles left unspoken on focus when the node has a name or a value: an
/// item says its name, not "list item" after it. The rule and the role set
/// are described under "When the role is spoken" in `docs/nvda/speech.md`.
fn is_silent_on_focus(role: Role) -> bool {
    matches!(
        role,
        Role::Pane
            | Role::Unknown
            | Role::Application
            | Role::Cell
            | Role::ListItem
            | Role::MenuItem
            | Role::TreeItem
            | Role::StaticText
    )
}

/// Whether a role speaks its value: a check box, radio button, link, menu
/// item, or application does not ("When values and descriptions are
/// spoken" in `docs/nvda/speech.md`).
fn speaks_value(role: Role) -> bool {
    !matches!(
        role,
        Role::CheckBox
            | Role::RadioButton
            | Role::Link
            | Role::MenuItem
            | Role::Application
            | Role::BusyIndicator
    )
}

/// Whether a role reports its own text as it changes, so a change of value
/// is not spoken: an edit field or a document.
fn reports_text_itself(role: Role) -> bool {
    matches!(role, Role::EditableText | Role::Document)
}

/// Whether a node announced for `reason` speaks its role: always, unless
/// the reason is focus, the node has a name, a value, or a row or column in
/// a table to hear instead, and its role is one left silent on focus.
fn speaks_role(node: &NodeSnapshot, reason: Reason) -> bool {
    let something_else = node.name.as_deref().is_some_and(|name| !name.is_empty())
        || node.value.as_deref().is_some_and(|value| !value.is_empty())
        || node.details.row.is_some()
        || node.details.column.is_some();
    !(reason == Reason::Focus && something_else && is_silent_on_focus(node.role))
}

/// The full announcement for a node, in NVDA's property order: name, role,
/// value, states, description, keyboard shortcut, row and column in a table
/// (`docs/parity/focus-announcement.md`, "Table cell coordinates"),
/// position in set, level —
/// each as its semantic span kind, never anonymous text (decision D12).
/// Detail spans simply do not appear when the backend reported nothing; the
/// role does not appear when `reason` leaves it silent ([`speaks_role`]).
///
/// A tree or list item's level goes first instead when it differs from
/// `last_level`, the last level put first, which it then becomes ("Where
/// the level goes" in `docs/nvda/speech.md`).
fn node_segments(
    node: &NodeSnapshot,
    reason: Reason,
    last_level: &mut Option<u32>,
) -> Vec<UtteranceSegment> {
    let mut segments = Vec::new();
    if let Some(name) = &node.name {
        segments.push(UtteranceSegment::label(name.clone()));
    }
    if speaks_role(node, reason) {
        segments.push(UtteranceSegment::new(SegmentContent::Role(node.role)));
    }
    if let Some(value) = node.value.as_ref().filter(|_| speaks_value(node.role)) {
        segments.push(UtteranceSegment::value(value.clone()));
    }
    segments.extend(state_segments(node.role, node.states, reason));
    // A description that only repeats the name is dropped.
    if let Some(description) = node
        .details
        .description
        .as_ref()
        .filter(|description| node.name.as_ref() != Some(*description))
    {
        segments.push(UtteranceSegment::new(SegmentContent::Description(
            description.clone(),
        )));
    }
    if let Some(shortcut) = &node.details.keyboard_shortcut {
        segments.push(UtteranceSegment::new(SegmentContent::Shortcut(
            shortcut.clone(),
        )));
    }
    if let Some(row) = node.details.row {
        segments.push(UtteranceSegment::new(SegmentContent::Row(row)));
    }
    if let Some(column) = node.details.column {
        segments.push(UtteranceSegment::new(SegmentContent::Column(column)));
    }
    if let Some(position) = node.details.position_in_set {
        segments.push(UtteranceSegment::new(SegmentContent::Position {
            position,
            set_size: node.details.set_size,
        }));
    }
    if let Some(level) = node.details.level {
        let segment = UtteranceSegment::new(SegmentContent::Level(level));
        if matches!(node.role, Role::TreeItem | Role::ListItem) && *last_level != Some(level) {
            *last_level = Some(level);
            segments.insert(0, segment);
        } else {
            segments.push(segment);
        }
    }
    segments
}

/// Builds a complete announcement utterance for a node (no entered-context
/// prefix): navigation, toast, and report-object announcements.
fn announce_node(
    trace_id: TraceId,
    priority: SpeechPriority,
    node: &NodeSnapshot,
    (reason, last_level): (Reason, &mut Option<u32>),
) -> Utterance {
    Utterance {
        trace_id,
        priority,
        segments: node_segments(node, reason, last_level),
        source: Some(source_of(node)),
        say_all: false,
        validity: None,
    }
}

/// Why states are being spoken, which decides which of them are ("Which
/// states are spoken, and in what order" in `docs/nvda/speech.md`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum StateReason {
    /// Focus moved to the node, or the navigator did.
    Focus,
    /// The user asked for the node.
    Query,
    /// The focused node's states changed.
    Change,
}

impl From<Reason> for StateReason {
    fn from(reason: Reason) -> Self {
        match reason {
            Reason::Focus => Self::Focus,
            Reason::Query => Self::Query,
        }
    }
}

/// The order states are spoken in, positive or negated alike.
const STATE_ORDER: [State; 18] = [
    State::Disabled,
    State::Focused,
    State::Selected,
    State::Busy,
    State::Pressed,
    State::Checked,
    State::Mixed,
    State::ReadOnly,
    State::Multiline,
    State::Expanded,
    State::Collapsed,
    State::Visited,
    State::Linked,
    State::HasPopup,
    State::Protected,
    State::Required,
    State::InvalidEntry,
    State::Offscreen,
];

/// The states in `a` that are also in `b`.
fn intersect(a: StateSet, b: StateSet) -> StateSet {
    StateSet::from_iter(a.iter().filter(|state| b.contains(*state)))
}

/// Of a node's `states`, the ones spoken as present for `reason`.
fn spoken_states(role: Role, states: StateSet, reason: StateReason) -> StateSet {
    let mut spoken = states;
    // Never worth hearing.
    spoken.remove(State::Selectable);
    spoken.remove(State::Focusable);
    spoken.remove(State::Checkable);
    // A combo box always has a popup.
    if role == Role::ComboBox {
        spoken.remove(State::HasPopup);
    }
    // Only a link says it was visited.
    if role != Role::Link {
        spoken.remove(State::Visited);
    }
    if reason == StateReason::Query {
        return spoken;
    }
    spoken.remove(State::Focused);
    // Linked is said only when it changes.
    if reason != StateReason::Change {
        spoken.remove(State::Linked);
    }
    spoken.remove(State::Offscreen);
    // Selection is the expected state of a focused item.
    if reason != StateReason::Change
        && matches!(
            role,
            Role::ListItem | Role::TreeItem | Role::MenuItem | Role::Row | Role::CheckBox
        )
        && states.contains(State::Selectable)
    {
        spoken.remove(State::Selected);
    }
    if !matches!(role, Role::EditableText | Role::CheckBox) {
        spoken.remove(State::ReadOnly);
    }
    if role != Role::EditableText {
        spoken.remove(State::Multiline);
    }
    if role == Role::CheckBox {
        spoken.remove(State::Pressed);
    }
    // Whether a submenu is open is not worth hearing.
    if role == Role::MenuItem && spoken.contains(State::HasPopup) {
        spoken.remove(State::Expanded);
        spoken.remove(State::Collapsed);
    }
    spoken
}

/// The states whose absence from a node is spoken for `reason`. A change
/// speaks "not selected" and "not checked" only while the node still
/// reports itself focused: the item the focus is leaving loses its
/// selection before the focus event for the next one arrives, and that
/// loss is not worth hearing.
fn negated_states(role: Role, states: StateSet, reason: StateReason) -> StateSet {
    let mut negated = StateSet::new();
    let focused = reason != StateReason::Change || states.contains(State::Focused);
    if states.contains(State::Selectable)
        && states.contains(State::Focusable)
        && reason != StateReason::Query
        && focused
        && matches!(
            role,
            Role::ListItem
                | Role::TreeItem
                | Role::Row
                | Role::Cell
                | Role::ColumnHeader
                | Role::RowHeader
                | Role::CheckBox
        )
    {
        negated.insert(State::Selected);
    }
    if (matches!(role, Role::CheckBox | Role::RadioButton) || states.contains(State::Checkable))
        && !states.contains(State::Mixed)
        && focused
    {
        negated.insert(State::Checked);
    }
    if role == Role::ToggleButton {
        negated.insert(State::Pressed);
    }
    if reason == StateReason::Change {
        return negated;
    }
    StateSet::from_iter(negated.iter().filter(|state| !states.contains(*state)))
}

/// `positive` and `negative` as segments, in [`STATE_ORDER`].
fn ordered_state_segments(positive: StateSet, negative: StateSet) -> Vec<UtteranceSegment> {
    STATE_ORDER
        .into_iter()
        .filter_map(|state| {
            if positive.contains(state) {
                Some(UtteranceSegment::new(SegmentContent::State(state)))
            } else if negative.contains(state) {
                Some(UtteranceSegment::new(SegmentContent::NegatedState(state)))
            } else {
                None
            }
        })
        .collect()
}

/// The spoken states of a node announced for `reason`, by the rules and in
/// the order of "Which states are spoken, and in what order" in
/// `docs/nvda/speech.md`.
fn state_segments(role: Role, states: StateSet, reason: Reason) -> Vec<UtteranceSegment> {
    let reason = StateReason::from(reason);
    ordered_state_segments(
        spoken_states(role, states, reason),
        negated_states(role, states, reason),
    )
}
