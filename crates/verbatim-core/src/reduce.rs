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

use verbatim_model::protocol::FocusPurpose;
use verbatim_model::{
    ActionName, AlertOrigin, BarChange, Earcon, EarlierElement, Effect, FetchResult,
    ForegroundWindow, Input, Message, NodeId, NodeSnapshot, NormalizedEvent, Notification,
    NotificationProcessing, OutpostId, Percent, Phrase, Pid, ProgressReading, PropertyChange,
    QueryKind, ReviewCommand, Role, SegmentContent, SpeechPriority, State, StateSet, TraceId,
    Utterance, UtteranceSegment, UtteranceSource, WindowFacts,
};
use verbatim_model::{
    Backend, FirstCachedAncestor, FocusGroup, FocusNow, FocusReaderKind, FocusText, FocusValidity,
    LiveReread, LiveRereadLog, LiveRereadReason, ObjectKey, WindowHandle, WindowIdentity,
};

use crate::driver::Core;
use crate::focus_queue::{Entry, Front, Report};
use crate::focus_rules::{
    ChangeKind, ChangeTarget, RepeatEvidence, SelectionTarget, change_target, is_sensitive_slider,
    lowest_common_ancestor, needs_live_ancestry, repeats_focus, selection_target,
};
use crate::foreground::{Attention, ForegroundFact, LatestFocus};
use crate::key_line::ReviewPress;
use crate::queries::Then;
use crate::state::PendingText;
use crate::state::{
    CellMemory, FocusContext, Navigator, NavigatorRead, SpokenRecord, SrState, TextFollowUp,
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
pub fn reduce(state: &mut Core, input: &Input) -> Vec<Effect> {
    #[cfg(test)]
    state.note(input);
    // Once Verbatim is exiting nothing more is said or asked.
    if state.exiting {
        return Vec::new();
    }
    let focus = state.focus.object().map(|focus| focus.snapshot.id);
    let was_left = state.focus.object_left();
    let first_query = state.queries.next();
    // A user input's number, recorded by its trace before anything it
    // causes is spoken (`crate::keys`).
    crate::keys::record(state, input);
    let mut effects = reduce_input(state, input);
    // The terminals Core reads (`terminal_model`): told when the focus
    // reaches or leaves them, given the settings, and their text answered
    // from their memory.
    if !state.terminals.is_empty() {
        let now = state.focus.object().map(|focus| focus.snapshot.id);
        // A left focus taken again is blurred and focused anew, as the same
        // switch with no stall leaves it (`crate::focus_queue`, the left mark).
        let taken_again = was_left && state.focus.applied_unmarked();
        if now != focus || taken_again {
            // The terminal's reading rules count in milliseconds.
            let at_ms = match input {
                Input::Event { observed_at_us, .. } => whole_ms(*observed_at_us),
                _ => 0,
            };
            terminal_model::focus_moved(state, (focus, now), at_ms);
        }
        terminal_model::sync_settings(state);
        effects = terminal_model::intercept(state, effects, terminal_model::input_trace(input));
    }
    // A function whose read ended with no answer, its terminal's model
    // gone, is woken with it lost: a press says nothing and ends.
    effects.extend(crate::driver::reap(state));
    // Whatever cuts speech off drops the terminal output handed to it, and
    // with it the output still waiting, as a key press does in NVDA.
    if effects.iter().any(terminal::cuts_speech) {
        let cut = terminal::cut(state, 0);
        effects.extend(cut);
    }
    // The queries asked run under the trace the shell runs their requests
    // under (`crate::queries`).
    state
        .queries
        .set_traces(first_query, &effects, input.trace_id());
    // Speech caused by a key carries the key's number, last of all.
    crate::keys::stamp(state, input, &mut effects);
    effects
}

/// [`reduce`]'s dispatch, by input.
fn reduce_input(state: &mut Core, input: &Input) -> Vec<Effect> {
    match input {
        Input::Event {
            trace_id,
            observed_at_us,
            source,
            window,
            event,
            purpose,
            ..
        } => reduce_event_keeping_press(
            state,
            (*trace_id, *purpose),
            *observed_at_us,
            *source,
            window.as_ref(),
            event,
        ),
        Input::Answered {
            trace_id,
            query_id,
            outpost,
            outcome,
            at_us,
        } => crate::queries::answered(state, *trace_id, *query_id, *outpost, outcome, *at_us),
        Input::OutpostEnded { trace_id, outpost } => {
            let mut effects = outpost_ended(state, *outpost);
            effects.extend(crate::queries::outpost_gone(state, *trace_id, *outpost));
            // A press reading there says nothing and ends.
            effects.extend(crate::driver::reap(state));
            effects.extend(crate::key_line::outpost_ended(state, *outpost));
            effects
        }
        // A key, a key command or script, or a menu's or the switcher's
        // end, that reaches Core while focus reports wait is handled after
        // them, in order, as NVDA queues gestures with events (Dickson,
        // 2026-11-08).
        Input::Command { .. }
        | Input::Script { .. }
        | Input::CaretKey { .. }
        | Input::CharacterTyped { .. }
        | Input::ClearingKey { .. }
        | Input::EditingChord { .. }
        | Input::HeldKey { .. }
        | Input::MenuOrSwitchEnded { .. }
            if state.focus.waiting() =>
        {
            state.focus.queue_input(input.clone());
            Vec::new()
        }
        Input::Command { .. } | Input::Script { .. } => command_arrived(state, input),
        Input::CaretKey { .. }
        | Input::CharacterTyped { .. }
        | Input::ClearingKey { .. }
        | Input::EditingChord { .. } => key_arrived(state, input),
        Input::HeldKey {
            trace_id,
            key,
            pressed_at_ms,
            ..
        } => crate::key_line::held_key(state, *trace_id, *key, *pressed_at_ms),
        Input::HeldKeyDue { trace_id, at_us } => {
            crate::key_line::deadline_passed(state, *trace_id, *at_us)
        }
        Input::TerminalTimer { trace_id, at_ms } => terminal_model::timer(state, *trace_id, *at_ms),
        Input::MarkReached { mark } => {
            let mut effects = say_all::mark_reached(state, *mark);
            effects.extend(terminal::mark_reached(state, *mark));
            effects
        }
        Input::SpeechCancelled { at_us } => speech_cancelled(state, *at_us),
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
        Input::Foreground(change) => foreground_changed(state, *change),
        Input::LiveRereadGone { .. } | Input::ListenerReplaced => {
            crate::live_reread::ended(state, input)
        }
        Input::ReadFocusUnanswered { .. } => crate::report_focus::ended(state, input),
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

/// A key press cut speech off at `at_us`: a key in the line marks it, a
/// terminal drops what waits, and say-all stops.
fn speech_cancelled(state: &mut SrState, at_us: u64) -> Vec<Effect> {
    crate::key_line::mark_later(state, at_us);
    let mut effects = terminal::cut(state, at_us / 1_000);
    effects.extend(say_all::stop(state));
    effects
}

/// Handles a key passed to the application that is not held: a caret key
/// judged by where the caret lands, text typed, or a key that clears a
/// terminal's line. Any other input does nothing here, an editing chord in
/// a text field among them: the hook's cut of speech was all it does.
pub(crate) fn key_input(state: &mut SrState, input: &Input) -> Vec<Effect> {
    // A terminal Core reads keeps the key for its reading rules.
    let mut effects = terminal_model::key_input(state, input);
    effects.extend(match input {
        Input::CaretKey {
            trace_id,
            key,
            pressed_at_ms,
            ..
        } => crate::key_line::unheld_caret_key(
            state,
            *trace_id,
            *key,
            *pressed_at_ms,
            crate::key_line::KEY_BOUND_MS,
        ),
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

/// Runs a review command or a script. Any other input does nothing here.
pub(crate) fn command_input(state: &mut Core, input: &Input) -> Vec<Effect> {
    match input {
        Input::Command {
            trace_id,
            command,
            repeat,
            pressed_at_ms,
            ..
        } => reduce_command(state, (*trace_id, *pressed_at_ms), *command, *repeat),
        Input::Script { repeat, script, .. } => shell::script(*script, *repeat),
        _ => Vec::new(),
    }
}

/// The trace of a key's input, minted when the hook saw the key.
pub(crate) fn key_trace(input: &Input) -> Option<TraceId> {
    match input {
        Input::CaretKey { trace_id, .. }
        | Input::CharacterTyped { trace_id, .. }
        | Input::ClearingKey { trace_id, .. }
        | Input::EditingChord { trace_id, .. }
        | Input::HeldKey { trace_id, .. } => Some(*trace_id),
        _ => None,
    }
}

/// `input` with its press taken as no earlier than `at_ms`: a key behind
/// a held key reaches the application only after it was passed on.
pub(crate) fn pressed_no_earlier(input: Input, at_ms: u64) -> Input {
    match input {
        Input::CaretKey {
            trace_id,
            key_seq,
            key,
            pressed_at_ms,
        } => Input::CaretKey {
            trace_id,
            key_seq,
            key,
            pressed_at_ms: pressed_at_ms.max(at_ms),
        },
        Input::CharacterTyped {
            trace_id,
            key_seq,
            text,
            at_ms: typed_at_ms,
        } => Input::CharacterTyped {
            trace_id,
            key_seq,
            text,
            at_ms: typed_at_ms.max(at_ms),
        },
        Input::ClearingKey {
            trace_id,
            key_seq,
            at_ms: cleared_at_ms,
        } => Input::ClearingKey {
            trace_id,
            key_seq,
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

/// Classifies one event, observed at `observed_at_us`, against the
/// attention record, or, for a focus with window facts, against the
/// record's front. With no attention yet, everything is attended, since
/// there is nothing to compare against. The focus Core holds names the
/// application UIA notifications are judged by.
fn classify(
    state: &SrState,
    (source, observed_at_us): (Pid, u64),
    window: Option<&WindowFacts>,
    event: &NormalizedEvent,
) -> Acceptance {
    let Some(attention) = state.foreground.attention() else {
        return Acceptance::Attended;
    };
    let focus_source = state.focus_source();
    match event {
        // A controlled selection is spoken only while its controller is the
        // focus, which is attended wherever the controlled list's window
        // is.
        NormalizedEvent::ControlledSelection { .. } => Acceptance::Attended,
        // Every focus with window facts, a whole-window focus among them,
        // is judged against the record's front, the window the focus
        // listener last reported in front: dropped when the front was
        // observed after it and is a window it is unrelated to, so a
        // popup, an owned dialog and a focus in an owned window pass. The
        // reader's own test at the event cannot see a foreground change
        // after it, and NVDA tests when it handles the event, so a focus
        // whose read was held past a switch to an unrelated window is
        // refused there (`docs/parity/focus-events.md`, "Event
        // acceptance"). With no window facts, the application decides.
        NormalizedEvent::FocusChanged { .. } => {
            let attended = match window {
                Some(facts) => !state.foreground.front().is_some_and(|front| {
                    front.observed_after(observed_at_us) && !front.relates(facts)
                }),
                None => window_is_attended(state, attention, source, None),
            };
            if attended {
                Acceptance::Attended
            } else {
                Acceptance::Dropped
            }
        }
        // A toast is spoken from anywhere.
        NormalizedEvent::Alert { .. } => {
            if window_is_attended(state, attention, source, window) {
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
            if window_is_attended(state, attention, source, window) {
                Acceptance::Attended
            } else {
                Acceptance::Dropped
            }
        }
    }
}

/// Whether a focus event from outpost generation `outpost`, observed at
/// `observed_at_us`, microseconds on the run's shared clock, was observed
/// before the newest focus observation from another generation, the
/// ordering stamp ([`LatestFocus::observed_after`]). Events from one
/// generation arrive in order; across generations they can arrive out of
/// order, which NVDA's single event queue never does. A replacement
/// generation's report is judged against its predecessor's newest focus as
/// another outpost's, which is safe because its first report, the
/// focus-now answer, carries the time its read began, after the
/// predecessor's last observation. An answer to a focus-now query is
/// ordered with the events by that time; a report with no observation time
/// (0) is never stale.
///
/// A focus is not made stale by a stamp a whole-window report of its own
/// top-level window set: a foreground change is ordered by when its window
/// became the foreground, after a focus inside it may have been observed,
/// as a Settings page's content is inside `ApplicationFrameHost`'s frame.
/// Two ordinary reports in one window shared by two processes follow the
/// rule against each other (Dickson, 2026-11-08;
/// `docs/parity/focus-events.md`, "Stale focus events").
fn is_stale_focus(
    state: &SrState,
    outpost: OutpostId,
    observed_at_us: u64,
    window: Option<&WindowFacts>,
) -> bool {
    observed_at_us != 0
        && state.foreground.stale(
            outpost,
            observed_at_us,
            window.map(|window| window.top_level),
        )
}

/// Core's duplicate-focus rule (`books/dev/src/explanation/design/focus.md`, "One UI Automation
/// focus, end to end"; `docs/parity/focus-events.md`, "Duplicate focus suppression"): a focus whose
/// element is the same as the last queued focus, while that element still
/// has the keyboard focus, is no new focus. NVDA has no rule across its
/// backends like it: each backend's own duplicate test, which the outposts
/// and the focus listener make, is its only one, and a foreground or menu
/// focus has none.
///
/// The last queued focus is NVDA's (`crate::focus_queue`): the focus
/// object, or the report set aside for a live re-read while one waits. A
/// focus Core refuses (stale, not in the foreground, or a foreground report
/// for the window the last queued focus is in) changes nothing, as NVDA
/// never queues those. A focus from another application in between is a
/// different focus, so the one after it is announced.
///
/// The evidence of "same element" is the node id, which names one object
/// in one outpost generation; a UI Automation runtime id keeps its node
/// even when its application gave it to a new element after the one it
/// named died (`books/dev/src/reference/adr/0001-focus.md`, "Object identity"). So "still focused"
/// is `earlier`, what the reader read live
/// with the focus of the element it held under that node: a duplicate only
/// when that element still has the keyboard focus. With no such read, the
/// reader holding no element for the node or its read cut before it, the
/// report itself is the evidence, read while its generation ran even when
/// that generation ended before Core decided it. An outpost reports an MSAA
/// focus only once the object or an ancestor has the focused state, read
/// live, and drops an event naming the address of its last focus itself.
///
/// A report judged `left`, the user having gone from the last queued focus
/// to another top-level window since, has no duplicate: the return is
/// spoken as the same switch with no stall is.
///
/// A repeat of a report waiting for its live re-read is no duplicate of
/// it, as NVDA's would be, because the reader answers a live re-read only
/// for the read it holds last, which the repeat replaced: the repeat is
/// queued behind it as a new focus, and is executed against the focus
/// spoken before it, so the user hears the focus once.
fn is_duplicate(
    state: &SrState,
    node: &NodeSnapshot,
    earlier: Option<EarlierElement>,
    left: bool,
) -> bool {
    let Some(last) = state.focus.last_queued() else {
        return false;
    };
    !left
        && repeats_focus(RepeatEvidence {
            same_element: last.node.id == node.id,
            still_focused: earlier.is_none_or(|earlier| earlier == EarlierElement::Focused),
        })
}

/// Whether a whole-window focus in `window`, judged `left` or not, is
/// refused as one of the window the last queued focus is in
/// (`crate::focus_queue`), compared by window handle because the same
/// window has different node ids in different outposts. NVDA drops a
/// foreground event when the last focus it queued is in that window
/// (`docs/nvda/events.md`, "Foreground windows").
fn refused_whole_window(state: &SrState, window: Option<&WindowFacts>, left: bool) -> bool {
    let Some(window) = window else {
        return false;
    };
    !left
        && state.focus.last_queued().is_some_and(|last| {
            last.window
                .is_some_and(|focus_window| focus_window.top_level == window.top_level)
        })
}

/// Forgets what Core keeps of the text of `node`, whose reader read that
/// the element it held under that node lost the focus or is gone: its
/// caret, a caret key waiting on it, and its terminal's model, so the
/// focus's text is taken afresh, as a new object's is
/// (`docs/parity/focus-events.md`, "Duplicate focus suppression").
fn forget_text_of(state: &mut SrState, node: NodeId) {
    if state.focus_caret(node).is_some() {
        state.forget_caret();
    }
    state.drop_models(|model| model.node == node);
}

/// Keeps a duplicate focus's newer reading, unspoken: its states and name,
/// which an earlier report can have read mid-change, and those of the
/// entries of its chain, `ancestors`, that are nodes of the focus ancestry,
/// but for its cached ancestors, which the caller leaves out: an entry its
/// focus reader took from its ancestry cache never overwrites one of
/// Core's own.
/// Its chain does not replace the focus line, as NVDA keeps its line when
/// the focus matches itself (`docs/parity/focus-context.md`,
/// "Focus-ancestry context"), so a duplicate whose chain was cut at its
/// bound leaves the containers the focus is in where they were. What was
/// last spoken of the focus stays as it was ([`SpokenRecord`]), so a change
/// the repeat carried in is still spoken when its own event comes. The
/// selected item already announced is kept, so a selection event for a
/// new one is still spoken; a navigator the user moved away stays where it
/// is.
fn keep_newer_reading(state: &mut SrState, node: &NodeSnapshot, ancestors: &[NodeSnapshot]) {
    let Some(focus) = state.focus.object_mut() else {
        return;
    };
    focus.reported_focused |= node.states.contains(State::Focused);
    focus.snapshot = node.clone();
    // An entry is matched as the lines are joined: by node, or by window
    // handle for a window, which keeps its node.
    let newer = |old: &NodeSnapshot| {
        ancestors
            .iter()
            .find(|new| crate::focus_rules::same_object(old, *new))
            .map(|new| NodeSnapshot {
                id: old.id,
                ..new.clone()
            })
    };
    if focus
        .ancestors
        .iter()
        .any(|old| newer(old).is_some_and(|new| new != *old))
    {
        focus.ancestors = focus
            .ancestors
            .iter()
            .map(|old| newer(old).unwrap_or_else(|| old.clone()))
            .collect();
    }
}

/// Whether an event from elsewhere, observed at `observed_at_us`, was
/// observed before the newest foreground change Core applied: its speech
/// would have been queued before that change, which cancels it in NVDA
/// (`books/dev/src/reference/adr/0001-focus.md`, "One input to Core, in arrival order"). An event
/// with no
/// observation time (0) is never judged so.
fn observed_before_foreground_change(state: &SrState, observed_at_us: u64) -> bool {
    observed_at_us != 0
        && state
            .foreground
            .changed_at()
            .is_some_and(|changed_at| observed_at_us < changed_at)
}

/// `observed_at_us` in whole milliseconds, for the caret history and the
/// terminal's reading rules, which compare it with a key's `pressed_at_ms`
/// and the terminal's own times in milliseconds.
fn whole_ms(observed_at_us: u64) -> u64 {
    observed_at_us / 1_000
}

/// Whether an event's window is one the attention record covers: the same
/// top-level window, the same root owner, a topmost window, or a
/// `Windows.UI.Core` window under the input thread's active window — NVDA's
/// foreground test, made against the attention record — or a window its
/// outpost found in the system's foreground window when it read the event.
/// When either side has no window facts there is nothing to compare, so the
/// application decides: attention's, or that of Core's focus while that
/// focus is in the attention window, as a Settings page's content is in
/// `ApplicationFrameHost`'s frame, which attention names (`docs/parity/focus-events.md`,
/// "Event acceptance"). Focus events with window facts are judged by the
/// record's front instead ([`classify`]).
fn window_is_attended(
    state: &SrState,
    attention: &Attention,
    source: Pid,
    window: Option<&WindowFacts>,
) -> bool {
    match (attention.window.as_ref(), window) {
        (Some(attended), Some(event)) => {
            event.top_level == attended.top_level
                || event.root_owner == attended.root_owner
                || event.topmost
                || event.under_active_window == Some(true)
                || event.in_foreground
        }
        _ => process_is_attended(state, attention, source),
    }
}

/// Whether `process` is the application attention covers: attention's own
/// process, or that of Core's focus while that focus is in the attention
/// window, as a Settings page's content (`SystemSettings`) is in
/// `ApplicationFrameHost`'s frame, which attention names
/// (`docs/parity/focus-events.md`, "Event acceptance").
pub(crate) fn process_is_attended(state: &SrState, attention: &Attention, process: Pid) -> bool {
    process == attention.source
        || state.focus.object().is_some_and(|focus| {
            focus.source == process
                && matches!(
                    (focus.window.as_ref(), attention.window.as_ref()),
                    (Some(focus), Some(attended)) if focus.top_level == attended.top_level
                )
        })
}

/// The focus listener's report that a window came to the foreground, the
/// only report of one (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the window
/// title"): it gives the keyboard
/// hook's view of the foreground and, unless the window is one of
/// Verbatim's own, attention and the record's front, and it applies to the
/// last queued focus, the left mark's one home (`crate::focus_queue`): the
/// reports Core judges after it that were observed after it find that focus
/// left for its window when the window is unrelated to it. It speaks
/// nothing, cuts nothing, sets no ordering stamp, and changes no node,
/// focus group or navigator: nothing is heard while the user is in an
/// application that does not answer.
fn foreground_changed(state: &mut SrState, change: ForegroundWindow) -> Vec<Effect> {
    state.foreground.apply(ForegroundFact::Changed(change));
    state.focus.foreground_changed(&change);
    Vec::new()
}

/// Speaks what an activation did: the action's name, NVDA's "Activate" for
/// an action with none, or "No action" when nothing was `activated`.
pub(crate) fn activation_completed(
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
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![segment],
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// Whether `event` is a focus dropped before anything else judges it: an
/// older entry of a focus group whose newer report Core took
/// ([`verbatim_model::FocusGroup`]), a focus an application's rule refuses
/// whole ([`crate::applications::refuses_focus`]), or a stale focus from
/// another outpost generation. The duplicate rule comes after the
/// foreground test, so a duplicate that passes both still records its
/// observation ([`focus_is_a_duplicate`]).
fn focus_dropped_first(
    state: &mut SrState,
    observed_at_us: u64,
    window: Option<&WindowFacts>,
    event: &NormalizedEvent,
) -> bool {
    let NormalizedEvent::FocusChanged {
        node,
        whole_window,
        earlier,
        group,
        ..
    } = event
    else {
        return false;
    };
    if matches!(
        earlier,
        Some(EarlierElement::NotFocused | EarlierElement::Gone)
    ) {
        forget_text_of(state, node.id);
    }
    if group.is_some_and(|group| older_than_taken(state, node.id.outpost(), group)) {
        return true;
    }
    if !whole_window && crate::applications::refuses_focus(window, node) {
        return true;
    }
    is_stale_focus(state, node.id.outpost(), observed_at_us, window)
}

/// Whether `group`, of outpost generation `outpost`, is an older entry of
/// the focus group whose report Core took last: NVDA's `pumpAll` processes
/// no older event of a run once one is processed.
fn older_than_taken(state: &SrState, outpost: OutpostId, group: FocusGroup) -> bool {
    state.group_taken.is_some_and(|(taken_outpost, taken)| {
        taken_outpost == outpost && taken.number == group.number && taken.rank < group.rank
    })
}

/// Records that the focus of `report` was taken: its focus group, which ends the group for its older entries, and
/// Verbatim+Tab's waiting press when
/// it was observed after the press, whose answer would then be late
/// (`crate::report_focus`).
fn record_taken(state: &mut SrState, report: &FocusReport<'_>) {
    record_group(state, report);
    crate::report_focus::focus_applied(state, report.observed_at_us);
}

/// Records `report`'s focus group as taken, which ends the group for its
/// older entries: for a report taken, and for one set aside for a live
/// re-read, whose answer stands for it.
fn record_group(state: &mut SrState, report: &FocusReport<'_>) {
    if let Some(group) = report.group {
        state.group_taken = Some((report.node.id.outpost(), group));
    }
}

/// Whether Core's duplicate rule judges a focus of `node`, read for
/// `purpose`: every report and a plain focus query's answer; a fake
/// focus's answer only when it was read through MSAA, as NVDA tests a
/// fake focus for duplicates only for MSAA objects; the desktop's return
/// never, as NVDA queues that focus with no test
/// (`books/dev/src/reference/adr/0001-focus.md`, "A focus UI Automation does not deliver").
fn duplicate_rule_applies(purpose: Option<FocusPurpose>, node: &NodeSnapshot) -> bool {
    match purpose {
        None | Some(FocusPurpose::Plain | FocusPurpose::Report) => true,
        Some(FocusPurpose::Fake) => node.backend == Backend::Msaa,
        Some(FocusPurpose::Again) => false,
    }
}

/// Keeps a duplicate's newer reading, unspoken, on the focus object when it
/// is the duplicate's node, with its caret.
fn keep_duplicate(state: &mut Core, report: &Report, cached: Option<CachedFrom>) -> Vec<Effect> {
    if state
        .focus
        .object()
        .is_none_or(|focus| focus.snapshot.id != report.node.id)
    {
        return Vec::new();
    }
    // The entries below the innermost cached one were read live, and so
    // was a window above the reader's process boundary, which is always
    // described from Windows' state and never cached.
    let live: Vec<NodeSnapshot> = report
        .ancestors
        .iter()
        .enumerate()
        .filter(|(index, ancestor)| {
            cached.is_none_or(|cached| *index > cached.index_usize())
                || ancestor.details.above_boundary
        })
        .map(|(_, ancestor)| ancestor.clone())
        .collect();
    keep_newer_reading(state, &report.node, &live);
    sync_navigator_with_focus(state);
    if let Some(effects) = crate::key_line::repeat_evidence(
        state,
        report.trace_id,
        report.node.id,
        report.text.as_ref(),
    ) {
        return effects;
    }
    editing::keep_focus_caret(state, report.node.id, report.text.as_ref());
    let Some(FocusText::Caret { changes, caret, .. }) = report.text.as_ref() else {
        return Vec::new();
    };
    let general = changes.content_changed.then_some(editing::General {
        selection: changes.new_selection.as_ref(),
        collapsed: caret.selection.is_none(),
    });
    editing::selection_changed(
        state,
        report.trace_id,
        report.node.id,
        (&changes.selection_changes, general),
    )
}

/// The ordering stamps, which the stale rule judges: the observation of
/// every report Core accepted into its order, whatever is then decided of
/// it (Dickson, 2026-11-08).
fn stamp_observed(state: &mut SrState, report: &Report) {
    if report.observed_at_us != 0 {
        state
            .foreground
            .apply(ForegroundFact::Observed(LatestFocus {
                outpost: report.node.id.outpost(),
                observed_at_us: report.observed_at_us,
                top_level: report.window.as_ref().map(|window| window.top_level),
                whole_window: report.whole_window,
            }));
    }
}

/// A report's [`FirstCachedAncestor`], when it names one of `ancestors`,
/// the ancestors Core takes ([`focus_ancestors`]): void when nothing was
/// cached, or when an application's rule dropped the chain.
fn cached_ancestors(
    first_cached: Option<FirstCachedAncestor>,
    ancestors: &[NodeSnapshot],
) -> Option<CachedFrom> {
    first_cached
        .map(CachedFrom)
        .filter(|cached| cached.index_usize() < ancestors.len())
}

/// A report's cached ancestors as Core judges them: up to and including
/// its innermost cached ancestor, by its index in the ancestors Core takes.
#[derive(Clone, Copy, Debug)]
struct CachedFrom(FirstCachedAncestor);

impl CachedFrom {
    /// The index of the innermost cached ancestor.
    fn index_usize(self) -> usize {
        usize::try_from(self.0.index).unwrap_or(usize::MAX)
    }
}

/// A focus's ancestors as Core takes them: as the event reports them, but
/// none for an item whose containers an application's rule drops
/// ([`crate::applications::drops_ancestry`]).
fn focus_ancestors<'a>(
    window: Option<&WindowFacts>,
    node: &NodeSnapshot,
    ancestors: &'a [NodeSnapshot],
) -> &'a [NodeSnapshot] {
    if crate::applications::drops_ancestry(window, node) {
        &[]
    } else {
        ancestors
    }
}

/// A caret report of `node`: keeps Core's caret current and speaks the
/// selection changes it carries, a selection no key made
/// (`editing::selection_changed`). The focus's own caret comes with the
/// focus (`editing::focus_text`).
fn reduce_caret_moved(state: &mut Core, trace_id: TraceId, moved: &NormalizedEvent) -> Vec<Effect> {
    let NormalizedEvent::CaretMoved {
        node_id: node,
        caret,
        selection_changes,
        content_changed,
        new_selection,
        facts,
    } = moved
    else {
        return Vec::new();
    };
    let general = content_changed.then_some(editing::General {
        selection: new_selection.as_ref(),
        collapsed: caret.selection.is_none(),
    });
    crate::key_line::caret_report(
        state,
        trace_id,
        *node,
        caret.clone(),
        (selection_changes, general),
        facts.as_deref(),
    )
}

/// A live re-read's answer, `event` (`crate::live_reread`): taken only
/// while its report waits for it, and judged by no rule its report already
/// passed, since the report took its place in the order when it was
/// queued. An answer from a window the user has left by then is refused,
/// ending the report unanswered; any other is its report's reading, ready
/// in its turn. Either way the queue runs on. An answer to no waiting
/// report says nothing.
fn live_reread_answered(
    state: &mut Core,
    (trace_id, source): (TraceId, Pid),
    observed_at_us: u64,
    window: Option<&WindowFacts>,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    let NormalizedEvent::FocusChanged {
        node,
        whole_window,
        ancestors,
        selected_child,
        text,
        earlier,
        group,
        answers: Some(request),
        ..
    } = event
    else {
        return Vec::new();
    };
    let outpost = node.id.outpost();
    if !state.focus.waits_for(outpost, *request) {
        return Vec::new();
    }
    if matches!(
        earlier,
        Some(EarlierElement::NotFocused | EarlierElement::Gone)
    ) {
        forget_text_of(state, node.id);
    }
    if classify(state, (source, observed_at_us), window, event) == Acceptance::Dropped {
        state.focus.unanswered(outpost, *request);
    } else {
        let answer = Report {
            trace_id,
            source,
            observed_at_us,
            window: window.cloned(),
            node: node.clone(),
            whole_window: *whole_window,
            ancestors: Arc::from(focus_ancestors(window, node, ancestors)),
            selected_child: selected_child.clone(),
            text: text.as_deref().cloned(),
            earlier: *earlier,
            group: *group,
            alive: true,
        };
        state.focus.answered(outpost, *request, answer);
    }
    run_queue(state)
}

/// Decides the reports at the queue's front in order, each against the
/// last queued focus and the focus object the one before it left: an
/// answered one is executed, one that ended unanswered becomes the last
/// queued focus, and a pending one is judged and planned
/// ([`decide_report`]), until one waits for its live re-read or none is
/// left (`crate::focus_queue`).
pub(crate) fn run_queue(state: &mut Core) -> Vec<Effect> {
    let mut effects = Vec::new();
    loop {
        match state.focus.front() {
            Front::Wait => break,
            Front::Execute(report, left, arrival) => {
                effects.extend(execute_report(state, &report, left, arrival));
            }
            Front::Decide(entry, cached) => {
                if decide_report(state, *entry, cached.map(CachedFrom), &mut effects) {
                    break;
                }
            }
            Front::Input(input) => effects.extend(waited_input(state, &input)),
        }
    }
    let floor = state.foreground.stale_floor();
    state.focus.let_go_of_changes(floor);
    state.focus.restart_count_when_idle();
    effects
}

/// A key command or script with no focus report waiting ahead of it: one
/// pressed while a held key waits, or while the watch of the held key last
/// passed on is open, runs after it, as NVDA runs scripts one at a time in
/// the order queued (`books/dev/src/explanation/design/ownership.md`); any
/// other runs now.
fn command_arrived(state: &mut Core, input: &Input) -> Vec<Effect> {
    crate::key_line::command_arrived(state, input)
}

/// An input that waited behind focus reports, handled in its turn
/// (`crate::focus_queue`): a key, key command or script acts on the focus
/// executed before it, behind a held key if one waits then, its effects
/// under its own trace; a menu's or the switcher's end judges the focus
/// taken since it.
fn waited_input(state: &mut Core, input: &Input) -> Vec<Effect> {
    match input {
        Input::Command { .. } | Input::Script { .. } => {
            under_key(input.trace_id(), command_arrived(state, input))
        }
        Input::CaretKey { .. }
        | Input::CharacterTyped { .. }
        | Input::ClearingKey { .. }
        | Input::EditingChord { .. } => under_key(key_trace(input), key_arrived(state, input)),
        // The held-key line carries a held key's effects under its trace.
        Input::HeldKey {
            trace_id,
            key,
            pressed_at_ms,
            ..
        } => crate::key_line::held_key(state, *trace_id, *key, *pressed_at_ms),
        _ => shell::reduce(state, input),
    }
}

/// A key with no focus report waiting ahead of it: one pressed while a held
/// key waits reaches the application after it, so it is handled after it
/// (`books/dev/src/explanation/design/key-holding.md`, "Keys behind a held
/// key"); any other now.
fn key_arrived(state: &mut SrState, input: &Input) -> Vec<Effect> {
    crate::key_line::key_arrived(state, input)
}

/// `effects`, of a key or command handled after the input it reached Core
/// with, carried under its own trace, as the held-key line carries them.
fn under_key(trace_id: Option<TraceId>, effects: Vec<Effect>) -> Vec<Effect> {
    match trace_id {
        Some(trace_id) => effects
            .into_iter()
            .map(|effect| match effect {
                carried @ Effect::ForKey(..) => carried,
                effect => Effect::ForKey(trace_id, Box::new(effect)),
            })
            .collect(),
        None => effects,
    }
}

/// Decides `entry`, the report at the queue's front, with its cached
/// ancestors from `cached`, as NVDA handles a focus at its place in its
/// queue, every report ahead of it decided:
///
/// - Its verdict: whether a foreground change left the last queued focus
///   for a window other than the report's own (the left mark).
/// - A whole-window focus is refused when the last queued focus is in its
///   window and the report is not judged left ([`refused_whole_window`]);
///   it still makes its window the foreground object, as NVDA's is while
///   its focus is in that window ([`change_on`]). It never moves
///   attention, which only the focus listener's foreground change moves
///   (D14).
/// - A duplicate of the last queued focus is dropped, its newer reading
///   kept ([`is_duplicate`], [`keep_duplicate`]). A repeat of the last
///   queued focus that reached Core while that one still waited for its
///   live re-read is no duplicate of it.
/// - Any other is planned against the focus object ([`plan_focus`]): set
///   aside for a live re-read when the plan would speak an ancestor its
///   focus reader took from its ancestry cache, as entered or as the title
///   ([`set_aside`]), which every report behind it waits for, returning
///   true, unless its outpost generation has ended, when no re-read can be
///   asked and it ends unanswered, the last queued focus, unspoken;
///   otherwise executed.
fn decide_report(
    state: &mut Core,
    entry: Entry,
    cached: Option<CachedFrom>,
    effects: &mut Vec<Effect>,
) -> bool {
    let report = &entry.report;
    let left = state.focus.left_against(&entry);
    let repeats = state.focus.last_queued().is_some_and(|last| {
        last.node.id == report.node.id && last.waited.is_some_and(|ended| ended > entry.arrival)
    });
    if report.whole_window && refused_whole_window(state, report.window.as_ref(), left) {
        state
            .foreground
            .apply(ForegroundFact::Object(ObjectKey::of(&report.node)));
        return false;
    }
    if duplicate_rule_applies(entry.purpose, &report.node)
        && !repeats
        && is_duplicate(state, &report.node, report.earlier, left)
    {
        effects.extend(keep_duplicate(state, report, cached));
        return false;
    }
    // What the plan asks: nothing, a live re-read, or, when one is due but
    // the report's outpost generation has ended, its end unanswered.
    let asked = {
        let view = report.view(cached);
        let found = state
            .focus
            .object()
            .cloned()
            .map(|focus| Line { focus, left });
        let plan = plan_focus(state, report.window.as_ref(), &view, found.as_ref());
        match (plan.live_reread, cached) {
            (Some(_), Some(_)) if !report.alive => Some(None),
            (Some(reason), Some(cached)) => {
                // Its answer stands for it, so the group's older entries
                // are not then taken.
                record_group(state, &view);
                Some(Some(set_aside(state, report, (&plan, reason, cached))))
            }
            _ => None,
        }
    };
    if let Some(asked) = asked {
        // No live re-read is asked of an ended outpost generation: the
        // report ends unanswered (Claude, 2026-11-08, accepted by Dickson).
        match asked {
            Some((reader, effect)) => {
                effects.push(effect);
                state.focus.reread_first(entry, left, reader);
                return true;
            }
            None => state.focus.unanswered_now(entry),
        }
        false
    } else {
        effects.extend(execute_report(state, report, left, entry.arrival));
        false
    }
}

/// [`reduce_event`], keeping a waiting Verbatim+Tab press across a focus
/// query's answer. The answer is stamped with the time its read began, not
/// when Core asked, so it cannot be placed against the press: only a focus
/// event observed after the press ends it, as NVDA runs NVDA+Tab after the
/// focus events queued before it and speaks both
/// ([`crate::report_focus::focus_applied`]).
fn reduce_event_keeping_press(
    state: &mut Core,
    (trace_id, purpose): (TraceId, Option<FocusPurpose>),
    observed_at_us: u64,
    source: Pid,
    window: Option<&WindowFacts>,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    let press = purpose
        .is_some_and(|purpose| purpose != FocusPurpose::Report)
        .then_some(state.report_press)
        .flatten();
    let effects = reduce_event(
        state,
        (trace_id, purpose),
        observed_at_us,
        source,
        window,
        event,
    );
    if press.is_some() && state.report_press.is_none() {
        state.report_press = press;
    }
    effects
}

fn reduce_event(
    state: &mut Core,
    (trace_id, purpose): (TraceId, Option<FocusPurpose>),
    observed_at_us: u64,
    source: Pid,
    window: Option<&WindowFacts>,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    // Verbatim+Tab's answer is only spoken (`crate::report_focus`).
    if purpose == Some(FocusPurpose::Report) {
        return crate::report_focus::answered(state, trace_id, event);
    }
    if let NormalizedEvent::FocusChanged {
        answers: Some(_), ..
    } = event
    {
        let effects =
            live_reread_answered(state, (trace_id, source), observed_at_us, window, event);
        sync_navigator_with_focus(state);
        return effects;
    }
    if focus_dropped_first(state, observed_at_us, window, event) {
        return Vec::new();
    }
    match classify(state, (source, observed_at_us), window, event) {
        Acceptance::Dropped => return Vec::new(),
        Acceptance::Background => return reduce_background(state, trace_id, observed_at_us, event),
        Acceptance::Attended => {}
    }

    let effects = match event {
        NormalizedEvent::FocusChanged {
            node,
            whole_window,
            ancestors,
            selected_child,
            text,
            earlier,
            group,
            first_cached_ancestor,
            ..
        } => {
            let ancestors = focus_ancestors(window, node, ancestors);
            let cached = cached_ancestors(*first_cached_ancestor, ancestors);
            let report = Report {
                trace_id,
                source,
                observed_at_us,
                window: window.cloned(),
                node: node.clone(),
                whole_window: *whole_window,
                ancestors: Arc::from(ancestors),
                selected_child: selected_child.clone(),
                text: text.as_deref().cloned(),
                earlier: *earlier,
                group: *group,
                alive: true,
            };
            focus_reported(state, purpose, report, cached)
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
        NormalizedEvent::Alert { node, origin } => {
            reduce_alert(state, (trace_id, observed_at_us), node, *origin)
        }
        NormalizedEvent::ValueChanged { node_id, value } => {
            reduce_value_changed(state, trace_id, *node_id, value.clone())
        }
        moved @ NormalizedEvent::CaretMoved { .. } => reduce_caret_moved(state, trace_id, moved),
        NormalizedEvent::TerminalSnapshot { node_id, snapshot } => {
            terminal_model::snapshot_event(state, trace_id, *node_id, snapshot)
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
    let (Some(focus), Some(navigator)) = (state.focus.object(), state.navigator.as_mut()) else {
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
    state: &mut SrState,
    trace_id: TraceId,
    observed_at_us: u64,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    if observed_before_foreground_change(state, observed_at_us) {
        return Vec::new();
    }
    if let NormalizedEvent::Alert { node, origin } = event {
        return reduce_alert(state, (trace_id, observed_at_us), node, *origin);
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
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::text(text.clone())],
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// The parts of a `FocusChanged` event the focus handling reads.
struct FocusReport<'a> {
    node: &'a NodeSnapshot,
    /// When its outpost observed it, in microseconds; 0 when not known.
    observed_at_us: u64,
    /// Whether it is a whole-window focus.
    whole_window: bool,
    /// Its ancestors, outermost first, complete to the desktop.
    ancestors: &'a [NodeSnapshot],
    selected_child: Option<&'a NodeSnapshot>,
    /// What the outpost's caret read found, for a focus that may have text.
    text: Option<&'a FocusText>,
    /// What its reader read of the element it held before under its node.
    earlier: Option<EarlierElement>,
    /// Its place in its outpost's focus group.
    group: Option<FocusGroup>,
    /// Where its cached ancestors begin, when it has any and is no live
    /// re-read's answer ([`cached_ancestors`]).
    cached: Option<CachedFrom>,
}

impl FocusReport<'_> {
    /// Whether it is a new element under the node of the previous focus,
    /// `previous`: its reader read that the element it held before under
    /// that node is gone. One that only lost the focus is still the same
    /// object, so the lines meet at it (`docs/parity/focus-events.md`,
    /// "Duplicate focus suppression").
    fn replaces(&self, previous: &NodeSnapshot) -> bool {
        self.node.id == previous.id && self.earlier == Some(EarlierElement::Gone)
    }
}

impl Report {
    /// The report as the focus handling reads it, `cached` naming where its
    /// cached ancestors begin.
    fn view(&self, cached: Option<CachedFrom>) -> FocusReport<'_> {
        FocusReport {
            node: &self.node,
            observed_at_us: self.observed_at_us,
            whole_window: self.whole_window,
            ancestors: &self.ancestors,
            selected_child: self.selected_child.as_ref(),
            text: self.text.as_ref(),
            earlier: self.earlier,
            group: self.group,
            cached,
        }
    }
}

/// Takes a focus `report` that passed the stale and foreground tests, read
/// for `purpose`, into Core's order (`crate::focus_queue`): its
/// observation is an ordering stamp, and it waits behind the reports not
/// yet decided, with its cached ancestors from `cached`; then the queue
/// runs.
fn focus_reported(
    state: &mut Core,
    purpose: Option<FocusPurpose>,
    report: Report,
    cached: Option<CachedFrom>,
) -> Vec<Effect> {
    stamp_observed(state, &report);
    state
        .focus
        .queue(report, purpose, cached.map(|cached| cached.0));
    run_queue(state)
}

/// Executes `report`, judged `left` or not, against the focus object, as
/// NVDA executes a focus against the one before it
/// (`docs/parity/focus-announcement.md`, "Focus announcement content and
/// property order"):
///
/// - A whole-window focus makes its window the foreground object.
/// - A whole-window focus with no name becomes the focus silently: a bare
///   "window" says nothing, and the window is not announced later.
/// - While the focus is dead (its outpost ended) and the report is not
///   judged left, a report of the same focus from a replacement outpost is
///   taken silently.
/// - Otherwise the new chain is joined to the focus line where they meet
///   ([`focus_ancestry`]), and the containers below the meeting point, the
///   newly entered ones, the node, and a selection container's selected
///   item are spoken, queued behind current speech (which the key press
///   that moved focus has already cut off), and the navigator follows
///   focus. When the two lines differ just below the desktop, speech is cut
///   off first, and the new top-level window, its outermost ancestor, is
///   one of the entered containers, spoken by the same rule as the others
///   (`docs/parity/focus-context.md`, "Window announcement on switching
///   applications"). Speech queued for an earlier focus expires as NVDA's
///   speech manager expires it (`Effect::DropExpiredSpeech`).
/// - The title rule: a focus that enters the window in front, one that is
///   not the entered window or that is judged left, says the window's
///   title when its line does not already speak it ([`window_title`]).
/// - A report judged left is executed as a focus from another top-level
///   window is (the left mark, `crate::focus_queue`).
fn execute_report(state: &mut Core, owned: &Report, left: bool, arrival: u64) -> Vec<Effect> {
    let node = owned.node.id;
    let first = crate::key_line::before_focus(state, node);
    let mut effects = execute_focus(state, owned, (left, arrival), first);
    effects.extend(crate::key_line::focus_executed(state, node));
    effects.extend(crate::key_line::navigator_moved(state));
    effects
}

/// [`execute_report`], the words of a key whose focus moved first said
/// after the expired speech is dropped and before a new window cuts them.
fn execute_focus(
    state: &mut SrState,
    owned: &Report,
    (left, arrival): (bool, u64),
    first: Vec<Effect>,
) -> Vec<Effect> {
    let report = &owned.view(None);
    let window = owned.window.as_ref();
    let (trace_id, source) = (owned.trace_id, owned.source);
    let found = state
        .focus
        .object()
        .cloned()
        .map(|focus| Line { focus, left });
    let line = focus_ancestry(found.as_ref(), window, report);
    if report.whole_window {
        state
            .foreground
            .apply(ForegroundFact::Object(ObjectKey::of(report.node)));
    }
    let previous_left = found.as_ref().is_some_and(|found| found.left);
    let title = enter_window(state, window, report, previous_left);

    // A left focus is followed as a focus from another top-level window is:
    // nothing of the line left behind carries over.
    let previous = found
        .as_ref()
        .filter(|found| !found.left)
        .map(|found| &found.focus);
    let mut new_focus = focus_context(
        (trace_id, source),
        window,
        report,
        (previous, Arc::clone(&line.ancestors)),
    );
    new_focus.alive = owned.alive;

    if let Some(focus) = previous
        && !focus.alive
        && reads_the_same(focus, report.node, report.ancestors)
    {
        // The silent re-read: the replacement outpost reports the focus the
        // user already heard, so take its ids without speaking.
        state.focus.execute(new_focus, arrival);
        record_taken(state, report);
        if state.navigator.is_none() {
            state.navigator = Some(Navigator::on(report.node.clone()));
        }
        editing::keep_focus_caret(state, report.node.id, report.text);
        return first;
    }

    // The review cursor follows focus (roadmap M3): every focus change snaps
    // the navigator object to the new focus and resets the review cursor to
    // its start.
    let navigator = Navigator::on(report.node.clone());
    // When speech is cut off (`docs/nvda/speech.md`, "Cancellation", and
    // `docs/nvda/events.md`): focus speech is queued, not interrupting. A
    // focus line that differs from the previous one just below the desktop
    // brings a new foreground window, which cancels speech, nameless or
    // not, and so does entering a menu; speech for a focus the user has
    // since left is dropped by the speech manager, which is told where the
    // focus now is.
    let foreground_changed = line.cut;
    let line_met = line.met.is_some();
    if foreground_changed && report.observed_at_us != 0 {
        state
            .foreground
            .apply(ForegroundFact::LineCut(report.observed_at_us));
    }
    let entered: Vec<&NodeSnapshot> = line.entered.iter().collect();
    let mut effects = vec![Effect::DropExpiredSpeech(FocusNow {
        focus: ObjectKey::of(report.node),
        ancestors: new_focus.ancestors.iter().map(ObjectKey::of).collect(),
        foreground: state.foreground.object(),
        menu_item_of_focus: menu_item_of_focus(previous, report.node),
    })];
    effects.extend(first);
    // Entering a menu cuts speech too, at its place among the entered
    // containers (`focus_speech`).
    end_text_activity(
        state,
        report.node.id,
        (foreground_changed, previous_left),
        &mut effects,
    );
    state.focus.execute(new_focus, arrival);
    record_taken(state, report);
    state.navigator = Some(navigator);
    // A nameless whole-window focus, and any focus in the shell's staging
    // windows, say nothing; speech was cut above if the foreground window
    // changed.
    // The virtual desktop's name kept is said by the focus taken, as NVDA
    // says it before any focus (`crate::desktop`).
    let desktop_name = crate::desktop::name_for_focus(state, trace_id);
    if report.whole_window && !has_text(report.node.name.as_deref())
        || crate::applications::is_shell_staging_window(window)
    {
        effects.extend(desktop_name);
        return effects;
    }

    // NVDA says the window in front only when the focus line changed just
    // below the desktop; lines that meet at an object they share, such as a
    // window and the one it owns across processes, add no title.
    let title = title
        .filter(|_| !line_met)
        .and_then(|top_level| window_title(top_level, report.ancestors, &entered));
    let speech = focus_speech(state, trace_id, report, (title, entered, desktop_name));
    effects.extend(speech);
    effects.extend(editing::focus_text(
        state,
        trace_id,
        report.node,
        report.text,
    ));
    effects
}

/// The focus `report` becomes when taken, or stands for while it waits as
/// the queued report: its node under `ancestors`, its line joined with
/// `previous`, the line it is planned against, not left.
fn focus_context(
    (trace_id, source): (TraceId, Pid),
    window: Option<&WindowFacts>,
    report: &FocusReport<'_>,
    (previous, ancestors): (Option<&FocusContext>, Arc<[NodeSnapshot]>),
) -> FocusContext {
    FocusContext {
        source,
        window: window.cloned(),
        snapshot: report.node.clone(),
        spoken: spoken_record(previous, report, &ancestors),
        ancestors,
        last_selection: report.selected_child.map(|selected| selected.id),
        alive: true,
        reported_focused: report.node.states.contains(State::Focused),
        trace_id: Some(trace_id),
        observed_at_us: report.observed_at_us,
        reported_format: verbatim_model::TextAttributes::default(),
        caret: previous
            .filter(|previous| previous.snapshot.id == report.node.id)
            .and_then(|previous| previous.caret.clone()),
    }
}

/// The focus line a report is planned or executed against, owned so the
/// plan can write the state: a focus, and the report's verdict, whether a
/// foreground change left the last queued focus for a window other than
/// the report's before it (`crate::focus_queue`). A report from the window
/// the focus was left for is not judged left, so its line is compared with
/// the focus's as it is, as NVDA compares its focus line with the one
/// before, and a window owned across processes meets its owner both ways.
#[derive(Clone, Debug)]
struct Line {
    focus: FocusContext,
    left: bool,
}

/// The entered window as a focus Core takes changes it, and the window
/// whose title the focus is to say, if any (`docs/parity/focus-context.md`,
/// "Window announcement on switching applications"): a whole-window focus
/// enters its window, saying no title but its own; a focus whose own
/// top-level window is the window in front when it was published
/// (`WindowFacts::foreground_window`) enters it when it is not the entered
/// window or the line planned against was left, making its chain's outermost entry the
/// foreground object, as NVDA's `doPreGainFocus` moves its foreground
/// object from the focus's ancestry, and says its title. A focus in a
/// popup, whose top-level window is not the foreground window, changes
/// nothing. A foreground change never sets the entered window: a report
/// read in a window as another came in front carries the window in front
/// at its publication, and setting the entered window from it would lose
/// that window's title.
fn enter_window(
    state: &mut SrState,
    window: Option<&WindowFacts>,
    report: &FocusReport<'_>,
    previous_left: bool,
) -> Option<WindowHandle> {
    let window = window?;
    if report.whole_window {
        state
            .foreground
            .apply(ForegroundFact::Entered(window.top_level));
        return None;
    }
    entered_window(state, Some(window), report, previous_left)?;
    state.foreground.apply(ForegroundFact::EnteredBy(
        window.top_level,
        report.ancestors.first().map(ObjectKey::of),
    ));
    // Program Manager and the taskbar are entered with no title, as NVDA
    // takes no foreground event for them.
    (!window.foreground_is_shell).then_some(window.top_level)
}

/// The top-level window a focus that is not a whole-window focus enters,
/// with no write: its own, when it is the window in front at its
/// publication and is not the entered window or the line planned against was left
/// ([`enter_window`]).
fn entered_window(
    state: &SrState,
    window: Option<&WindowFacts>,
    report: &FocusReport<'_>,
    previous_left: bool,
) -> Option<WindowHandle> {
    let window = window?;
    let enters = !report.whole_window
        && window.top_level == window.foreground_window
        && (state.foreground.entered() != Some(window.top_level) || previous_left);
    enters.then_some(window.top_level)
}

/// What Core does with a focus report, computed once, purely, before
/// anything is written ([`plan_focus`]).
struct FocusPlan<'a> {
    /// The report joined to the line it is planned against
    /// ([`focus_ancestry`]).
    ancestry: JoinedAncestry<'a>,
    /// Why the report is set aside for a live re-read, when a cached
    /// ancestor would be spoken (`focus_rules::needs_live_ancestry`).
    live_reread: Option<LiveRereadReason>,
}

/// The plan for `report`, in `window` (`books/dev/src/reference/crates/verbatim-core.md`,
/// "The ancestry cache"): its ancestry joined to `previous`, the line the
/// reports before it will leave ([`provisional_line`]), with its verdict,
/// the entry the title rule would speak, and whether a cached ancestor
/// would be spoken, as entered or as that title. A report that says
/// nothing, taken silently, speaks no cached ancestor. Writes nothing.
fn plan_focus<'a>(
    state: &SrState,
    window: Option<&WindowFacts>,
    report: &FocusReport<'a>,
    previous: Option<&Line>,
) -> FocusPlan<'a> {
    let ancestry = focus_ancestry(previous, window, report);
    let previous_left = previous.is_some_and(|line| line.left);
    let silent = report.cached.is_none()
        || (report.whole_window && !has_text(report.node.name.as_deref()))
        || crate::applications::is_shell_staging_window(window)
        || previous.filter(|line| !line.left).is_some_and(|line| {
            !line.focus.alive && reads_the_same(&line.focus, report.node, report.ancestors)
        });
    let live_reread = if silent {
        None
    } else {
        let entered: Vec<&NodeSnapshot> = ancestry.entered.iter().collect();
        let title = entered_window(state, window, report, previous_left)
            .filter(|_| window.is_some_and(|window| !window.foreground_is_shell))
            .filter(|_| ancestry.met.is_none())
            .and_then(|top_level| window_title(top_level, report.ancestors, &entered))
            .and_then(|title| {
                report
                    .ancestors
                    .iter()
                    .position(|ancestor| std::ptr::eq(ancestor, title))
            });
        let entered_from = report
            .ancestors
            .len()
            .saturating_sub(ancestry.entered.len());
        needs_live_ancestry(
            report.cached.map(CachedFrom::index_usize),
            entered_from,
            title,
        )
    };
    FocusPlan {
        ancestry,
        live_reread,
    }
}

/// Sets `report` aside for a live re-read of its element by the focus
/// reader that read it, since `plan` found that a cached ancestor would be
/// spoken, for `reason`: the live re-read is counted and asked for, with
/// what the shell logs of it, and the report waits for its answer at the
/// queue's front (`crate::focus_queue`). Nothing is said. Returns the reader
/// and request, and the effect.
fn set_aside(
    state: &mut SrState,
    report: &Report,
    (plan, reason, cached): (&FocusPlan<'_>, LiveRereadReason, CachedFrom),
) -> (((OutpostId, FocusReaderKind), u64), Effect) {
    let outpost = report.node.id.outpost();
    let reader = cached.0.reader;
    let request = state.live_rereads.ask();
    let join = report
        .ancestors
        .get(cached.index_usize().saturating_add(1))
        .unwrap_or(&report.node);
    let lowest_common_ancestor = plan
        .ancestry
        .met
        .map(|at_new| report.ancestors.get(at_new).unwrap_or(&report.node).id);
    let effect = Effect::LiveReread(LiveReread {
        request,
        outpost,
        reader,
        trace_id: report.trace_id,
        window: report.window.as_ref().map(|window| window.top_level),
        logged: LiveRereadLog {
            focus: report.node.id,
            cache_join: (join.id, join.name.clone()),
            lowest_common_ancestor,
            reason,
        },
    });
    (((outpost, reader), request), effect)
}

/// The title a focus entering the top-level window `top_level` says: the
/// window's own entry in its chain, `ancestors`, the outermost entry with
/// that window handle, which stands for the window (its UIA element, MSAA
/// window or client object, or plain window), unless its line already
/// speaks it among the `entered` containers. An entry with no name, and a
/// chain with no entry for the window, add nothing.
fn window_title<'a>(
    top_level: WindowHandle,
    ancestors: &'a [NodeSnapshot],
    entered: &[&NodeSnapshot],
) -> Option<&'a NodeSnapshot> {
    let entry = ancestors
        .iter()
        .find(|ancestor| ancestor.details.window == Some(top_level))?;
    let spoken = entered.iter().any(|container| {
        container.id == entry.id
            && !is_menu(container)
            && (is_sensitive_slider(container) || is_presentable_container(container))
            && !container_segments(container).is_empty()
    });
    (!spoken && has_text(entry.name.as_deref())).then_some(entry)
}

/// The spoken record of a new focus, `report`'s node under `ancestors`
/// ([`SpokenRecord`]): the node as it is about to be spoken, and for each
/// ancestor the states last spoken of it in the previous focus line, the
/// previous focus included, or else its reading, from which an entered
/// container is spoken.
fn spoken_record(
    previous: Option<&FocusContext>,
    report: &FocusReport<'_>,
    ancestors: &[NodeSnapshot],
) -> SpokenRecord {
    let entries = ancestors
        .iter()
        .map(|ancestor| {
            let spoken = previous.and_then(|previous| previous.spoken_states(ancestor.id));
            (ancestor.id, spoken.unwrap_or(ancestor.states))
        })
        .collect();
    SpokenRecord::of(report.node, entries)
}

/// Ends what the previous focus's text was doing when the focus moves to
/// `node`: a say-all reading it (its speech is cut off too, since say-all's
/// speech is not focus speech the speech manager would drop), a caret key
/// still waiting for evidence (the focus announcement wins, as NVDA's wait
/// gives way to a pending focus event), and the typing being echoed. Speech
/// is cut off when `cut` says so or say-all was reading. When the focus
/// was `left`, what is kept of its node is reset as for another node: the
/// terminal's speech state and its caret, as the same switch with no stall
/// leaves them (the left mark, `crate::focus_queue`).
fn end_text_activity(
    state: &mut SrState,
    node: NodeId,
    (cut, left): (bool, bool),
    effects: &mut Vec<Effect>,
) {
    let reading = state.say_all.is_some();
    effects.extend(say_all::stop(state));
    if cut || reading {
        effects.push(Effect::StopSpeech);
    }
    state.typed_word.clear();
    crate::terminal::focus_moved(state, node, left);
}

/// The speech for a new focus: the window's title the title rule adds, if
/// any, spoken with the words of a whole-window focus of the window
/// ([`window_title`]), then each entered container as its own utterance,
/// outermost first, valid while the focus is inside it, an entered menu
/// cutting speech in its place instead ([`is_menu`]), then the focus, valid
/// while it is the focus. When the focus has entered another top-level
/// window, the window is the outermost entered container, spoken first
/// whenever it is presentable, as NVDA speaks a window it took from the
/// focus's ancestry ([`is_presentable_container`]). A focus that may have
/// text leaves its value out, since its text follows
/// (`editing::focus_text`).
fn focus_speech(
    state: &mut SrState,
    trace_id: TraceId,
    report: &FocusReport<'_>,
    (title, entered, desktop_name): (Option<&NodeSnapshot>, Vec<&NodeSnapshot>, Option<Effect>),
) -> Vec<Effect> {
    let last_level = &mut state.last_tree_level;
    let cell_memory = &mut state.cell_memory;
    let reads_text = editing::may_have_text(report.node.role);
    let mut effects = Vec::new();
    if let Some(title) = title {
        effects.push(Effect::Speak(
            Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments: unremembered_segments(title, Reason::Focus),
                source: Some(source_of(title)),
                say_all: false,
                validity: Some(FocusValidity {
                    node: title.id,
                    had_focus: false,
                    window: WindowIdentity::of(title),
                }),
            },
            None,
        ));
    }
    // The virtual desktop's name, after the foreground's title and before
    // the entered containers, in NVDA's order
    // (`books/dev/src/reference/adr/0001-focus.md`, "The desktop's return and name").
    effects.extend(desktop_name);
    for container in entered {
        // Entering a menu bar, menu, or menu item cuts speech and says
        // nothing, in its place among the entered containers, outermost
        // first: speech for a container entered outside the menu is cut
        // too (`docs/parity/changes-and-selection.md`, "Entered menus").
        let sensitive_slider = is_sensitive_slider(container);
        if is_menu(container) && !sensitive_slider {
            effects.push(Effect::StopSpeech);
            continue;
        }
        if !sensitive_slider && !is_presentable_container(container) {
            continue;
        }
        let segments = container_segments(container);
        if segments.is_empty() {
            continue;
        }
        effects.push(Effect::Speak(
            Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments,
                source: Some(source_of(container)),
                say_all: false,
                validity: Some(FocusValidity {
                    node: container.id,
                    had_focus: false,
                    window: WindowIdentity::of(container),
                }),
            },
            None,
        ));
    }
    let mut segments = if reads_text && report.node.value.is_some() {
        let mut node = report.node.clone();
        node.value = None;
        node_segments(&node, Reason::Focus, last_level, cell_memory)
    } else {
        node_segments(report.node, Reason::Focus, last_level, cell_memory)
    };
    // A selection container introduces its selected item right after
    // itself — the roadmap's "announce a focused list's selected item".
    if let Some(selected) = report.selected_child {
        segments.extend(node_segments(
            selected,
            Reason::Focus,
            last_level,
            cell_memory,
        ));
    }
    effects.push(Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(source_of(report.node)),
            say_all: false,
            validity: Some(FocusValidity {
                node: report.node.id,
                had_focus: true,
                window: WindowIdentity::of(report.node),
            }),
        },
        None,
    ));
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

/// Whether a reported focus, from another outpost generation, reads the
/// same as the dead focus's kept copy: role, name, value, and states, and
/// the ancestors' names and roles. Node ids are not compared, since a
/// replacement outpost issues new ones (`docs/parity/focus-events.md`,
/// "Recovery after an outpost is replaced"); a report of the ended
/// generation itself, waiting when it ended, is no replacement's.
fn reads_the_same(focus: &FocusContext, node: &NodeSnapshot, ancestors: &[NodeSnapshot]) -> bool {
    let kept = &focus.snapshot;
    kept.id.outpost() != node.id.outpost()
        && kept.role == node.role
        && kept.name == node.name
        && kept.value == node.value
        && kept.states == node.states
        && focus.ancestors.len() == ancestors.len()
        && focus
            .ancestors
            .iter()
            .zip(ancestors)
            .all(|(old, new)| old.role == new.role && old.name == new.name)
}

/// Handles the end of an outpost generation: the focus and the reports
/// waiting to be executed keep their copied data but their ids are dead,
/// the reports waiting for its focus readers' live re-reads end unanswered
/// and the queue runs on, and a navigator or pending navigation in that
/// outpost is cleared. Navigation then does nothing until focus is reported
/// again.
fn outpost_ended(state: &mut Core, outpost: OutpostId) -> Vec<Effect> {
    state.focus.outpost_ended(outpost);
    let mut effects = run_queue(state);
    // Text positions die with the outpost that minted them.
    if state
        .say_all
        .as_ref()
        .is_some_and(|say_all| say_all.node.outpost() == outpost)
    {
        effects.extend(say_all::stop(state));
    }
    if state
        .focus
        .object()
        .is_some_and(|focus| focus.snapshot.id.outpost() == outpost)
    {
        state.forget_caret();
    }
    state.queries.forget(|then| match then {
        Then::Text(PendingText { node, .. }) => node.outpost() == outpost,
        _ => false,
    });
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
    crate::driver::forget_queries_at(state, outpost);
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
/// before they read it ([`change_target`]). The foreground object is never
/// a speaking ancestor: NVDA's event on the foreground window resolves to
/// its foreground object, not to the ancestor instance
/// (`docs/parity/focus-announcement.md`, "A top-level window's state
/// change"), which an outpost reads for when the window is the focus of a
/// report Core refused.
fn change_on(state: &SrState, kind: ChangeKind, node_id: NodeId) -> Option<ChangeTarget> {
    let focus = state.focus.object().filter(|focus| focus.alive)?;
    let foreground = state.foreground.object();
    change_target(
        kind,
        node_id,
        focus.snapshot.id,
        focus.ancestors.iter().map(|ancestor| ancestor.id),
        |id| {
            !foreground.is_some_and(|object| {
                focus.ancestors.iter().any(|ancestor| {
                    ancestor.id == id && ObjectKey::of(ancestor).is_same_object(&object)
                })
            })
        },
    )
}

/// A value change of `node_id` that is not the focus's: spoken, the value
/// alone, when the node is the focus's parent, its innermost ancestor, and
/// a slider that focuses its thumb ([`is_sensitive_slider`]), and the value
/// differs from the one it was last read with in the focus line, as NVDA's
/// slider speaks its value while its thumb is the focus
/// (`docs/parity/focus-context.md`, "A slider that focuses its thumb").
fn slider_value_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    value: Option<String>,
) -> Vec<Effect> {
    let Some(focus) = state.focus.object_mut().filter(|focus| focus.alive) else {
        return Vec::new();
    };
    let Some(parent) = focus
        .ancestors
        .last()
        .filter(|parent| parent.id == node_id && is_sensitive_slider(parent))
    else {
        return Vec::new();
    };
    if parent.value == value {
        return Vec::new();
    }
    let mut updated = parent.clone();
    updated.value.clone_from(&value);
    let source = source_of(&updated);
    let mut ancestors = focus.ancestors.to_vec();
    if let Some(last) = ancestors.last_mut() {
        *last = updated;
    }
    focus.ancestors = Arc::from(ancestors);
    let Some(text) = value else {
        return Vec::new();
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::value(text)],
            source: Some(source),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// Handles a name change: when the focused node's name differs from the one
/// last spoken of it ([`SpokenRecord`]), the new name alone is spoken,
/// queued behind current speech, as NVDA does. A name change on any other
/// node, including an ancestor of the focus, is silent.
fn reduce_name_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    name: Option<&String>,
) -> Vec<Effect> {
    if change_on(state, ChangeKind::Name, node_id) != Some(ChangeTarget::Focus) {
        return Vec::new();
    }
    let Some(focus) = state.focus.object_mut() else {
        return Vec::new();
    };
    let spoken = focus.spoken();
    let unchanged = spoken.name.as_ref() == name;
    spoken.name = name.cloned();
    focus.snapshot.name = name.cloned();
    if unchanged {
        return Vec::new();
    }
    let Some(text) = name.filter(|name| !name.trim().is_empty()) else {
        return Vec::new();
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::label(text.clone())],
            source: Some(source_of(&focus.snapshot)),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// Speaks a toast: the alerting object announced in full, queued behind
/// current speech, as NVDA's notification behavior speaks it. A toast never
/// moves focus or the navigator.
fn reduce_alert(
    state: &mut SrState,
    (trace_id, observed_at_us): (TraceId, u64),
    node: &NodeSnapshot,
    origin: AlertOrigin,
) -> Vec<Effect> {
    if let AlertOrigin::ToastWindowOpened(id) = origin {
        // A repeat of the last toast within a second is dropped, leaving
        // the record, as NVDA records only one it speaks
        // (`docs/parity/alerts.md`, "Toasts").
        if state.last_toast.is_some_and(|last| {
            last.id == id && observed_at_us.saturating_sub(last.at_us) < TOAST_REPEAT_US
        }) {
            return Vec::new();
        }
        state.last_toast = Some(crate::state::LastToast {
            id,
            at_us: observed_at_us,
        });
    }
    // A toast is never a tree or list item, so the level spoken before it
    // plays no part.
    vec![Effect::Speak(
        announce_node(
            trace_id,
            SpeechPriority::Queued,
            node,
            (Reason::Focus, &mut None, &mut CellMemory::default()),
        ),
        None,
    )]
}

/// How long after a toast the same toast is dropped as a repeat: one second
/// (`docs/parity/alerts.md`, "Toasts").
const TOAST_REPEAT_US: u64 = 1_000_000;

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
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority,
            segments: vec![UtteranceSegment::text(text.clone())],
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// Runs a review or object-navigation command (roadmap M3) against the
/// navigator object and its review cursor. A command with no navigator yet
/// (nothing has ever been focused) does nothing. Object-navigation moves
/// (`Parent`, `NextSibling`, `PreviousSibling`, `FirstChild`) are async:
/// they emit a `Fetch` to the owning outpost and the completion moves the
/// navigator; everything else — report, activate, review text motion — is
/// synchronous over state the reducer already holds.
fn reduce_command(
    state: &mut Core,
    (trace_id, pressed_at_ms): (TraceId, u64),
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
        ReviewCommand::ReportFocus => {
            crate::report_focus::pressed(state, (trace_id, pressed_at_ms), repeat)
        }
        ReviewCommand::ReportStatusBar => {
            terminal_model::report_status_bar(state, trace_id, repeat)
        }
        ReviewCommand::PreviousCommandMark | ReviewCommand::NextCommandMark => crate::press::start(
            state,
            ReviewPress {
                trace_id,
                command,
                repeat,
            },
        ),
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
    let Some(focus) = state.focus.object().filter(|focus| focus.alive) else {
        return vec![editing::speak(
            trace_id,
            review_text::message(Message::NoCaret),
        )];
    };
    let node = focus.snapshot.id;
    let has_text = editing::may_have_text(focus.snapshot.role) || state.focus_caret(node).is_some();
    if !has_text {
        return vec![editing::speak(
            trace_id,
            review_text::message(Message::NotSupported),
        )];
    }
    if command == ReviewCommand::SayAllFromCaret {
        return say_all::start(state, node, true, verbatim_model::TextPoint::Caret);
    }
    state.queries.forget(|then| matches!(then, Then::Text(_)));
    let (_, effect) = state.queries.ask(
        crate::queries::Asking::Text(
            node,
            verbatim_model::TextOp::Location(verbatim_model::TextPoint::Caret),
        ),
        Then::Text(PendingText {
            node,
            then: TextFollowUp::Location,
            about: None,
        }),
    );
    vec![effect]
}

/// Runs a command on the navigator object and its review cursor.
fn navigator_command(
    state: &mut Core,
    trace_id: TraceId,
    command: ReviewCommand,
    repeat: u8,
) -> Vec<Effect> {
    // "To focus" is meaningful even with the navigator already on focus;
    // handle it before the navigator-present guard so it can seed one.
    if command == ReviewCommand::ToFocus {
        // NVDA says "Move to focus" before the object.
        let mut effects = navigator_to_focus(state, trace_id);
        if let Some(Effect::Speak(utterance, _)) = effects.first_mut() {
            utterance.segments.insert(
                0,
                UtteranceSegment::new(SegmentContent::Message(Message::MoveToFocus)),
            );
        }
        return effects;
    }
    let Some(navigator) = state.navigator.as_ref() else {
        return vec![Effect::Speak(
            Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments: vec![UtteranceSegment::new(SegmentContent::Message(
                    Message::NoNavigatorObject,
                ))],
                source: None,
                say_all: false,
                validity: None,
            },
            None,
        )];
    };

    let press = ReviewPress {
        trace_id,
        command,
        repeat,
    };
    match command {
        #[expect(clippy::unreachable, reason = "moving to the focus returned above")]
        ReviewCommand::ToFocus => unreachable!("handled above"),
        ReviewCommand::Activate => {
            let node = navigator.object.id;
            vec![
                state
                    .queries
                    .ask(crate::queries::Asking::Activate(node), Then::Activation)
                    .1,
            ]
        }
        ReviewCommand::ReportObject
        | ReviewCommand::Parent
        | ReviewCommand::NextSibling
        | ReviewCommand::PreviousSibling
        | ReviewCommand::FirstChild => crate::press::start(state, press),
        command if review_text::handles(command) => crate::press::start(state, press),
        _ => Vec::new(),
    }
}

/// Snaps the navigator (and review cursor) back to the current focus and
/// reports it. Does nothing else with nothing focused, or with a focus
/// whose outpost has ended.
fn navigator_to_focus(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let Some(focus) = state.focus.object().filter(|focus| focus.alive) else {
        return Vec::new();
    };
    let object = focus.snapshot.clone();
    state.navigator = Some(Navigator::on(object.clone()));
    let mut effects = vec![announce_focus_object(
        state,
        trace_id,
        &object,
        Reason::Focus,
    )];
    effects.extend(editing::navigator_text(
        state,
        trace_id,
        &object,
        NavigatorRead::Announce,
    ));
    effects
}

/// The announcement of the navigator object `object` after object
/// navigation or on request: its full announcement, except that an object
/// that may have text leaves its value out, its text said after the rest
/// ([`editing::navigator_text`]), as a focus's is.
pub(crate) fn announce_navigator(
    state: &mut SrState,
    trace_id: TraceId,
    object: &NodeSnapshot,
    reason: Reason,
) -> Effect {
    let levels = (reason, &mut state.last_tree_level, &mut state.cell_memory);
    let utterance = if editing::may_have_text(object.role) && object.value.is_some() {
        let mut announced = object.clone();
        announced.value = None;
        announce_node(trace_id, SpeechPriority::Queued, &announced, levels)
    } else {
        announce_node(trace_id, SpeechPriority::Queued, object, levels)
    };
    Effect::Speak(utterance, None)
}

/// Announces `object` as [`announce_navigator`] does and, when it is the
/// focus, moves what was last spoken of the focus to what is spoken now
/// ([`SpokenRecord`]): a property is remembered whenever it is spoken of the
/// object, whatever the reason (`docs/parity/changes-and-selection.md`,
/// "Name change on the focus"; "When values and descriptions are spoken" in
/// `docs/nvda/speech.md`), so a change a report has already spoken is not
/// spoken again when its event comes. The ancestors' entries stay, since
/// the announcement speaks none of them.
///
/// `object` is the focus only when it equals the focus's reading. A
/// navigation that lands on the focus's node fetches a separate object with
/// the same id, which may differ from the reading, and speaking that says
/// nothing of what was spoken of the focus. A navigator seeded from the
/// focus is kept equal to the reading after every event
/// (`sync_navigator_with_focus`), and a fetched copy equal to the reading
/// speaks the reading, so moving the record to it is right as well.
pub(crate) fn announce_focus_object(
    state: &mut SrState,
    trace_id: TraceId,
    object: &NodeSnapshot,
    reason: Reason,
) -> Effect {
    if let Some(focus) = state
        .focus
        .object_mut()
        .filter(|focus| focus.snapshot == *object)
    {
        let ancestors = Arc::clone(&focus.spoken().ancestors);
        focus.spoken = SpokenRecord::of(object, ancestors);
    }
    announce_navigator(state, trace_id, object, reason)
}

/// What reporting `object`, the navigator object, says on press `repeat`
/// after the first, of an object with no text to read: on the second its
/// name and value spelled, on the third copied to the clipboard (NVDA's
/// multi-press semantics).
pub(crate) fn report_object_again(
    trace_id: TraceId,
    object: &NodeSnapshot,
    repeat: u8,
) -> Vec<Effect> {
    if repeat == 1 {
        // NVDA spells the name and value joined by a space.
        let segments = spelled(&clipboard_text(object));
        if segments.is_empty() {
            return Vec::new();
        }
        return vec![Effect::Speak(
            Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments,
                source: Some(source_of(object)),
                say_all: false,
                validity: None,
            },
            None,
        )];
    }
    // Copy the object's name and value; the shell's clipboard helper owns
    // the spoken confirmation (see the memory on a single shared copy
    // path).
    let text = clipboard_text(object);
    if text.is_empty() {
        return Vec::new();
    }
    vec![Effect::CopyToClipboard(text)]
}

/// The utterance that announces `object` for a query, as
/// [`announce_navigator`] announces it, without its text: an object that
/// may have text leaves its value out, its text said after the rest.
/// Verbatim+Tab speaks its answer so (`crate::report_focus`). An answer of
/// the focus Core holds moves what was last spoken of it to what is spoken
/// now, as [`announce_focus_object`] does, so a change the answer spoke is
/// not spoken again when its event comes; nothing else of the focus
/// changes.
pub(crate) fn announce_query(
    state: &mut SrState,
    trace_id: TraceId,
    object: &NodeSnapshot,
) -> Utterance {
    if let Some(focus) = state
        .focus
        .object_mut()
        .filter(|focus| focus.snapshot.id == object.id)
    {
        let ancestors = Arc::clone(&focus.spoken().ancestors);
        focus.spoken = SpokenRecord::of(object, ancestors);
    }
    let levels = (
        Reason::Query,
        &mut state.last_tree_level,
        &mut state.cell_memory,
    );
    if editing::may_have_text(object.role) && object.value.is_some() {
        let mut announced = object.clone();
        announced.value = None;
        announce_node(trace_id, SpeechPriority::Queued, &announced, levels)
    } else {
        announce_node(trace_id, SpeechPriority::Queued, object, levels)
    }
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
    text: &str,
) -> Vec<Effect> {
    if let Some(effects) = review_text::flat_extra(state, trace_id, command, repeat, text) {
        return effects;
    }
    let Some(navigator) = state.navigator.as_mut() else {
        return Vec::new();
    };
    let offset = navigator.review_offset.min(text.len());
    let line = review::line_span(text, offset);
    let char_at = |at: usize| review::char_span(text, at).unwrap_or((at, at));

    let (new_offset, spoken, edge) = match command {
        ReviewCommand::ReviewTop => (0, review::line_span(text, 0), None),
        ReviewCommand::ReviewBottom => {
            let start = review::line_span(text, text.len()).0;
            (start, review::line_span(text, start), None)
        }
        ReviewCommand::ReviewPreviousLine => match review::previous_char(text, line.0) {
            Some(target) => {
                let span = review::line_span(text, target);
                (span.0, span, None)
            }
            None => (line.0, line, Some(Message::Top)),
        },
        ReviewCommand::ReviewNextLine => match review::next_line_span(text, line.0) {
            Some(span) => (span.0, span, None),
            None => (line.0, line, Some(Message::Bottom)),
        },
        // The current line leaves the position where it was, as NVDA's sets
        // none (`globalCommands.py` 1975 to 1990); start of line lands at
        // the line's start, reading the whole line.
        ReviewCommand::ReviewCurrentLine => (offset, line, None),
        ReviewCommand::ReviewStartOfLine => (line.0, line, None),
        ReviewCommand::ReviewEndOfLine => (line.1, line, None),
        ReviewCommand::ReviewPreviousWord
        | ReviewCommand::ReviewNextWord
        | ReviewCommand::ReviewCurrentWord => {
            let words = review::words(text);
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
        ReviewCommand::ReviewPreviousCharacter => match review::previous_char(text, offset) {
            Some(target) if target >= line.0 => (target, char_at(target), None),
            _ => (offset, char_at(offset), Some(Message::Left)),
        },
        ReviewCommand::ReviewNextCharacter => match review::char_span(text, offset) {
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
        for grapheme in verbatim_text::graphemes(slice)
            .into_iter()
            .filter_map(|range| slice.get(range))
        {
            segments.push(UtteranceSegment::new(SegmentContent::CharacterDescription(
                grapheme.to_owned(),
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
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: None,
            say_all: false,
            validity: None,
        },
        None,
    )]
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
    let Some(focus) = state.focus.object_mut().filter(|focus| focus.alive) else {
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
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: node_segments(
                node,
                Reason::Focus,
                &mut state.last_tree_level,
                &mut state.cell_memory,
            ),
            source: Some(source_of(node)),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// A node selected inside an element the focus controls ("Selection in a
/// list the focus controls" in `docs/nvda/events.md`): spoken as a focus,
/// interrupting, with the navigator moved to it, while `controller` is
/// still the live focus. The focus itself does not move.
fn reduce_controlled_selection(
    state: &mut Core,
    trace_id: TraceId,
    controller: NodeId,
    node: &NodeSnapshot,
) -> Vec<Effect> {
    if !state
        .focus
        .object()
        .is_some_and(|focus| focus.alive && focus.snapshot.id == controller)
    {
        return Vec::new();
    }
    // The state alone, so that two of its fields can be lent at once.
    let plain: &mut SrState = state;
    plain.navigator = Some(Navigator::on(node.clone()));
    let mut effects = vec![Effect::Speak(
        announce_node(
            trace_id,
            SpeechPriority::Interrupt,
            node,
            (
                Reason::Focus,
                &mut plain.last_tree_level,
                &mut plain.cell_memory,
            ),
        ),
        None,
    )];
    effects.extend(crate::key_line::controlled_selection(state, controller));
    effects.extend(crate::key_line::navigator_moved(state));
    effects
}

/// Shared handling for `ValueChanged` and `PropertyChanged(Value(..))`: both
/// speak the bare new value, only when the changed node is the focused one,
/// the value differs from the one last spoken of it ([`SpokenRecord`]), and
/// the role speaks its
/// value at all. An edit field or document never speaks its changes of
/// value ("When values and descriptions are spoken" in
/// `docs/nvda/speech.md`). A slider that focuses its thumb speaks its own
/// change of value while it is the focus's parent
/// ([`slider_value_changed`]).
fn reduce_value_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    value: Option<String>,
) -> Vec<Effect> {
    if change_on(state, ChangeKind::Value, node_id) != Some(ChangeTarget::Focus) {
        return slider_value_changed(state, trace_id, node_id, value);
    }
    let Some(focus) = state.focus.object_mut() else {
        return Vec::new();
    };
    let spoken = focus.spoken();
    let unchanged = spoken.value == value;
    spoken.value.clone_from(&value);
    focus.snapshot.value.clone_from(&value);
    // A focused progress bar with a number, on screen, is heard only as a
    // bar, from the listener's change (`docs/design/progress-bars.md`,
    // "Rules"); off screen, or with no number, it is an ordinary value
    // change, as NVDA's behaviour falls through.
    if focus.snapshot.role == Role::ProgressBar
        && !focus.snapshot.states.contains(State::Offscreen)
        && value.as_deref().and_then(Percent::parse).is_some()
    {
        return Vec::new();
    }
    if unchanged {
        return Vec::new();
    }
    let role = focus.snapshot.role;
    let Some(text) = value.filter(|_| speaks_value(role) && !reports_text_itself(role)) else {
        return Vec::new();
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::value(text)],
            source: Some(source_of(&focus.snapshot)),
            say_all: false,
            validity: None,
        },
        None,
    )]
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
    let percentage = progress
        .value
        .as_deref()
        .and_then(Percent::parse)
        .filter(|_| !progress.offscreen);
    let Some(percentage) = percentage else {
        return reduce_value_changed(state, trace_id, progress.id, progress.value.clone());
    };
    if change_on(state, ChangeKind::Value, progress.id) == Some(ChangeTarget::Focus)
        && let Some(focus) = state.focus.object_mut()
    {
        focus.snapshot.value.clone_from(&progress.value);
    }
    let place = progress.rect.map_or((0, 0), |rect| {
        (
            rect.left.saturating_add(rect.width / 2),
            rect.top.saturating_add(rect.height / 2),
        )
    });
    indicate_progress(state, place, percentage)
}

/// A progress bar's value changed, reported by the listener
/// (`docs/design/progress-bars.md`, "Rules"): heard when its window is the
/// foreground window or beneath it, or anywhere when the setting asks for
/// background bars, by the rule of [`reduce_progress_changed`].
pub(crate) fn reduce_bar_change(state: &mut SrState, change: &BarChange) -> Vec<Effect> {
    if !change.in_foreground && !state.settings.report_background_progress_bars {
        return Vec::new();
    }
    indicate_progress(state, change.place, change.percent)
}

/// The theme's progress indication for the bar at `place` at `percent`,
/// once the percentage has moved by at least a percent from the last one
/// indicated at that place (`docs/nvda/object-model.md`, "How a progress
/// bar reports its value").
fn indicate_progress(state: &mut SrState, place: (i32, i32), percent: Percent) -> Vec<Effect> {
    let hundredths = percent.hundredths();
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
    vec![Effect::PlayEarcon(Earcon::Progress(percent.whole()))]
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
    let Some(focus) = state.focus.object_mut() else {
        return Vec::new();
    };
    let role = focus.snapshot.role;
    let utterance_source = source_of(&focus.snapshot);
    let old_states = focus.spoken().states;
    focus.snapshot.states = new_states;
    if old_states == new_states {
        return Vec::new();
    }
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
    focus.spoken().states = new_states;

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
        Effect::Speak(
            Utterance {
                trace_id,
                priority: SpeechPriority::Queued,
                segments,
                source: Some(utterance_source),
                say_all: false,
                validity: None,
            },
            None,
        )
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
/// change is diffed against the states last spoken of the ancestor
/// ([`SpokenRecord`]), which then become the new ones, as does its reading;
/// queued, as on the focus.
/// Ignored for a node that is neither the focus nor one of its ancestors.
fn reduce_ancestor_states_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    new_states: StateSet,
) -> Vec<Effect> {
    let Some(focus) = state.focus.object_mut().filter(|focus| focus.alive) else {
        return Vec::new();
    };
    let Some(index) = focus
        .ancestors
        .iter()
        .position(|ancestor| ancestor.id == node_id)
    else {
        return Vec::new();
    };
    let Some(ancestor) = focus.ancestors.get(index) else {
        return Vec::new();
    };
    let (role, source) = (ancestor.role, source_of(ancestor));
    let old_states = focus.spoken_states(node_id).unwrap_or(ancestor.states);
    if ancestor.states != new_states {
        let mut ancestors = focus.ancestors.to_vec();
        if let Some(ancestor) = ancestors.get_mut(index) {
            ancestor.states = new_states;
        }
        focus.ancestors = Arc::from(ancestors);
    }
    if old_states == new_states {
        return Vec::new();
    }
    let spoken = focus.spoken();
    if let Some(entry) = spoken.ancestors.iter().position(|(id, _)| *id == node_id) {
        let mut entries = spoken.ancestors.to_vec();
        if let Some((_, states)) = entries.get_mut(entry) {
            *states = new_states;
        }
        spoken.ancestors = Arc::from(entries);
    }
    let segments = state_change_segments(role, old_states, new_states);
    if segments.is_empty() {
        return Vec::new();
    }
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments,
            source: Some(source),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// A description change on the focus, spoken as NVDA's base handler speaks
/// it ("The focus gate" in `docs/nvda/events.md`): the new description
/// alone, queued, when it differs from the one last spoken of it
/// ([`SpokenRecord`]) and has text, and
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
    let Some(focus) = state.focus.object_mut() else {
        return Vec::new();
    };
    let spoken = focus.spoken();
    let unchanged = spoken.description.as_ref() == description;
    spoken.description = description.cloned();
    focus.snapshot.details.description = description.cloned();
    if unchanged {
        return Vec::new();
    }
    let Some(text) = description
        .filter(|text| !text.trim().is_empty())
        .filter(|text| focus.snapshot.name.as_ref() != Some(*text))
    else {
        return Vec::new();
    };
    vec![Effect::Speak(
        Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::new(SegmentContent::Description(
                text.clone(),
            ))],
            source: Some(source_of(&focus.snapshot)),
            say_all: false,
            validity: None,
        },
        None,
    )]
}

/// What an object-navigation press's answer did.
#[expect(
    clippy::large_enum_variant,
    reason = "built once and moved into the press that asked"
)]
pub(crate) enum Navigated {
    /// The navigator moved to this neighbor, which is announced.
    To(NodeSnapshot),
    /// It stayed put, and this was said.
    Said(Vec<Effect>),
}

/// Takes the answer to an object-navigation press (`crate::press`).
///
/// On `FetchResult::Node`, moves the navigator to the returned neighbor.
/// On `FetchResult::NoNeighbor` or `FetchResult::Gone`, the navigator stays
/// put and the direction's edge message is spoken, NVDA's wording: "No
/// next", "No previous", "No containing object", "No objects inside"
/// (`docs/parity/review.md`, "Dead objects"). Live testing found silence
/// indistinguishable from a broken command, exactly as NVDA's spoken
/// messages predict.
pub(crate) fn navigated(
    state: &mut SrState,
    trace_id: TraceId,
    kind: QueryKind,
    result: FetchResult,
) -> Navigated {
    match result {
        FetchResult::Node(snapshot) => {
            state.navigator = Some(Navigator::on(snapshot.clone()));
            Navigated::To(snapshot)
        }
        // The application did not answer: the navigator stays where it is,
        // and nothing is known to speak.
        FetchResult::Unanswered => Navigated::Said(Vec::new()),
        // A tree edge or a dead object: the navigator stays put and the
        // edge is spoken (NVDA's wording, chosen per direction). Any future
        // `FetchResult` variant added under `#[non_exhaustive]` stays silent
        // until given a meaning here.
        _ => {
            let Some(message) = edge_message_of(kind) else {
                return Navigated::Said(Vec::new());
            };
            Navigated::Said(vec![Effect::Speak(
                Utterance {
                    trace_id,
                    priority: SpeechPriority::Queued,
                    segments: vec![UtteranceSegment::new(SegmentContent::Message(message))],
                    source: None,
                    say_all: false,
                    validity: None,
                },
                None,
            )])
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

/// A new focus's place in Core's focus ancestry ([`focus_ancestry`]).
struct JoinedAncestry<'a> {
    /// The focus's ancestors as Core keeps them, outermost first: the
    /// previous focus ancestry up to the lowest common ancestor, then the
    /// new chain's entries below it.
    ancestors: Arc<[NodeSnapshot]>,
    /// The entered ancestors: the new chain's entries below the lowest
    /// common ancestor, outermost first, the containers the focus has newly
    /// entered.
    entered: &'a [NodeSnapshot],
    /// The two ancestries differ just below the desktop, sharing no entry:
    /// the focus is in a new foreground window, and speech is cut.
    cut: bool,
    /// The lowest common ancestor's index in the new chain, its ancestors
    /// outermost first and then the focus; `None` when the two share none.
    /// When they meet at an object they share, the focus has not moved to
    /// a new foreground window as NVDA judges it, by its focus difference
    /// level, and no window title is added for it ([`window_title`]).
    met: Option<usize>,
}

/// Joins a new focus's chain, `report`'s ancestors and node, to the
/// previous focus ancestry, `previous`'s ancestors and focus, at their
/// lowest common ancestor ([`lowest_common_ancestor`]), as NVDA builds its
/// focus ancestry when the focus moves
/// (`docs/parity/focus-context.md`, "Focus-ancestry context"). The new
/// chain is walked from the focus outward and stops at the first entry that
/// is the same object as one in the previous line: by node id, or by window
/// handle when both are windows, so that one window two outposts read is
/// one entry, such as the Settings frame its own process reports as its
/// focus and the page's process as a pane above the page. The previous
/// line's entries are kept to that point, as NVDA keeps its old objects,
/// so a window matched by its handle keeps the name it was entered with;
/// the new entries below it are the containers newly entered, an element
/// replaced by a new one with the same role and name, such as File
/// Explorer's items view list after a folder is opened, among them; the
/// outpost gives an MSAA object reached by another path the id NVDA's
/// equality finds it to have. The readers leave out of a chain the entries that are not presentable,
/// among them an unnamed UI Automation top-level window, an MSAA window
/// object and every layout container, which NVDA's line holds, so the
/// focus's top-level window, which its window facts name, stands for the
/// entry just below the desktop ([`ancestry_top`]): with no object of the
/// chains shared, two lines in one top-level window meet there, empty
/// chains or not, the whole chain is entered and speech is not cut, and
/// lines in two top-level windows differ just below the desktop: the whole
/// chain is entered and speech is cut. With no previous focus, the first
/// focus Core takes, the whole chain is entered and speech is not cut, as
/// NVDA, starting, makes the focus its focus object before it handles the
/// focus, so the two lines meet at the focus itself.
fn focus_ancestry<'a>(
    previous: Option<&Line>,
    window: Option<&WindowFacts>,
    report: &FocusReport<'a>,
) -> JoinedAncestry<'a> {
    let Some(line) = previous else {
        return JoinedAncestry {
            ancestors: Arc::from(report.ancestors),
            entered: report.ancestors,
            cut: false,
            met: None,
        };
    };
    // A left focus is followed as a focus from another top-level window is:
    // the lines meet only at the desktop, and speech is cut.
    if line.left {
        return JoinedAncestry {
            ancestors: Arc::from(report.ancestors),
            entered: report.ancestors,
            cut: true,
            met: None,
        };
    }
    let previous = &line.focus;
    let old_line = || {
        previous
            .ancestors
            .iter()
            .chain(std::iter::once(&previous.snapshot))
    };
    // A new element under the previous focus's node, the one it stood for
    // gone, is not that focus: the lines meet above it, if anywhere.
    let met = if report.replaces(&previous.snapshot) {
        previous.ancestors.split_last().and_then(|(last, above)| {
            lowest_common_ancestor((above, last), (report.ancestors, report.node))
        })
    } else {
        lowest_common_ancestor(
            (&previous.ancestors, &previous.snapshot),
            (report.ancestors, report.node),
        )
    };
    let Some((at_old, at_new)) = met else {
        let old_top = ancestry_top(&previous.snapshot, previous.window.as_ref());
        let new_top = ancestry_top(report.node, window);
        let cut = old_top.is_none() || old_top != new_top;
        return JoinedAncestry {
            ancestors: Arc::from(report.ancestors),
            entered: report.ancestors,
            cut,
            met: None,
        };
    };
    // The previous line is kept through the meeting entry, the previous
    // focus among them when they meet there, unless the meeting entry
    // stands for the new focus itself.
    let kept = if at_new == report.ancestors.len() {
        at_old
    } else {
        at_old.saturating_add(1)
    };
    let entered = report
        .ancestors
        .get(at_new.saturating_add(1)..)
        .unwrap_or_default();
    JoinedAncestry {
        ancestors: old_line().take(kept).chain(entered).cloned().collect(),
        entered,
        cut: false,
        met: Some(at_new),
    }
}

/// The top-level window just below the desktop in the focus line of
/// `focus`, as its window facts `window` name it, the entry a reader can
/// leave out of the chain as not presentable ([`focus_ancestry`]): `None` with
/// no facts, and for a focus whose containers an application's rule drops,
/// whose container is the desktop, as the task switcher's item's is
/// ([`crate::applications::drops_ancestry`]).
fn ancestry_top(focus: &NodeSnapshot, window: Option<&WindowFacts>) -> Option<WindowHandle> {
    let window = window?;
    (!crate::applications::drops_ancestry(Some(window), focus)).then_some(window.top_level)
}

/// Whether a focus ancestor adds context worth speaking when focus first
/// enters it. The rule is exclusion-based: an ancestor speaks unless it is
/// one of the following.
///
/// - An item-level or text-entry role (`TreeItem`, `ListItem`,
///   `EditableText`). Focus lands on these; they are never context.
/// - A structural node with no semantics of its own (`Unknown`, `Pane`,
///   `Border`, `Whitespace`), which NVDA counts as layout whatever its
///   name (`docs/nvda/object-model.md`, "Presentation classification"),
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
/// - Any node that is invisible ([`State::Invisible`]), which NVDA counts
///   unavailable, never presented in the focus ancestry.
///
/// Whitespace-only names and descriptions count as absent. Everything else
/// speaks on entry whether named or not: dialogs, toolbars, and an unnamed
/// tree, which announces as a bare "tree view", the same as NVDA. The
/// behavior is recorded in `docs/parity/focus-context.md`, "Focus-ancestry
/// context".
fn is_presentable_container(node: &NodeSnapshot) -> bool {
    if node.states.contains(State::Invisible) {
        return false;
    }
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
        | Role::Border
        | Role::Whitespace
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
/// `docs/nvda/speech.md`). A slider that focuses its thumb is spoken as a
/// focus, whole ([`is_sensitive_slider`]).
fn container_segments(node: &NodeSnapshot) -> Vec<UtteranceSegment> {
    if is_sensitive_slider(node) {
        return unremembered_segments(node, Reason::Focus);
    }
    let mut entered = node.clone();
    entered.value = None;
    entered.details.level = None;
    if entered.role != Role::List {
        entered.details.keyboard_shortcut = None;
    }
    // An entered container speaks no level and is read without a cell, so
    // nothing is remembered.
    unremembered_segments(&entered, Reason::Focus)
}

/// [`node_segments`] for what remembers nothing it says: a window's title or
/// an entered container.
fn unremembered_segments(node: &NodeSnapshot, reason: Reason) -> Vec<UtteranceSegment> {
    node_segments(node, reason, &mut None, &mut CellMemory::default())
}

/// Why a node is being spoken, which decides whether its role is
/// ("When the role is spoken" in `docs/nvda/speech.md`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
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
        || node.details.cell.is_some();
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
/// the level goes" in `docs/nvda/speech.md`). A cell's row or column is left
/// out when unchanged within one table ([`CellMemory::said`]).
fn node_segments(
    node: &NodeSnapshot,
    reason: Reason,
    last_level: &mut Option<u32>,
    cell_memory: &mut CellMemory,
) -> Vec<UtteranceSegment> {
    let mut segments = Vec::new();
    if node.details.desktop_window {
        segments.push(UtteranceSegment::new(SegmentContent::Message(
            verbatim_model::Message::Desktop,
        )));
    } else if let Some(name) = &node.name {
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
    if let Some(cell) = &node.details.cell {
        let (row, column) = cell_memory.said(cell);
        if let Some(row) = row {
            segments.push(UtteranceSegment::new(SegmentContent::Row(row)));
        }
        if let Some(column) = column {
            segments.push(UtteranceSegment::new(SegmentContent::Column(column)));
        }
    }
    if let Some(counts) = node.details.table_counts {
        // NVDA's counts are said for a query only, and leave the table
        // forgotten, so the next cell says both its row and its column
        // (`docs/parity/focus-announcement.md`, "Table cell coordinates").
        segments.push(UtteranceSegment::new(SegmentContent::Phrase(
            verbatim_model::Phrase::TableCounts {
                rows: counts.rows,
                columns: counts.columns,
            },
        )));
        cell_memory.forget_table();
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
    (reason, last_level, cell_memory): (Reason, &mut Option<u32>, &mut CellMemory),
) -> Utterance {
    Utterance {
        trace_id,
        priority,
        segments: node_segments(node, reason, last_level, cell_memory),
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
