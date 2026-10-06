//! The reducer (architecture section 2): pure state-and-effect transitions
//! from normalized accessibility events and fetch completions.
//!
//! Nothing in this module performs I/O, reads a clock, or consults
//! randomness; the only way the outside world enters is through the `Input`
//! passed to [`reduce`]. That purity is what lets the flight recorder turn a
//! captured sequence of inputs into a deterministic regression test (see
//! [`crate::replay`]).
//!
//! The behavior is specified in `docs/parity.md` under "Focus and
//! announcements", written from the NVDA behavior documented in
//! `docs/nvda/events.md`.

use std::sync::Arc;

use verbatim_model::{
    ActionName, Effect, FetchResult, Input, Message, NodeId, NodeSnapshot, NormalizedEvent,
    Notification, NotificationProcessing, OutpostId, Pid, PropertyChange, Query, QueryId,
    QueryKind, ReviewCommand, Role, SegmentContent, SpeechPriority, State, StateSet, TraceId,
    Utterance, UtteranceSegment, UtteranceSource, WindowFacts,
};
use verbatim_model::{FocusNow, FocusValidity};

use crate::state::{Attention, FocusContext, Navigator, PendingNavigation, SrState, TextFollowUp};
use crate::{editing, review, review_text, say_all, terminal, text};

/// The activity id of the shell's window-snap results notification, the one
/// UIA notification spoken from any application (`docs/parity.md`, "Event
/// acceptance").
const SNAP_RESULTS_ACTIVITY: &str = "Windows.Shell.SnapComponent.SnapHotKeyResults";

/// Advances `state` by one `input`, changing it in place, and returns the
/// effects the imperative shell must execute.
///
/// Deterministic: no I/O, no clocks, and no randomness, so the same state
/// and input always produce the same new state and effects. The state is
/// changed in place rather than copied, so a step costs nothing for the
/// parts of the state it does not touch (architecture section 2).
#[must_use]
pub fn reduce(state: &mut SrState, input: &Input) -> Vec<Effect> {
    let effects = reduce_input(state, input);
    // Whatever cuts speech off drops the terminal output handed to it, and
    // with it the output still waiting, as a key press does in NVDA.
    if effects.iter().any(terminal::cuts_speech) {
        terminal::cut(state);
    }
    effects
}

/// [`reduce`]'s dispatch, by input.
fn reduce_input(state: &mut SrState, input: &Input) -> Vec<Effect> {
    match input {
        Input::Event {
            trace_id,
            observed_at_ms,
            source,
            window,
            event,
            ..
        } => reduce_event(state, *trace_id, *observed_at_ms, *source, *window, event),
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
        Input::CaretKey {
            key, pressed_at_ms, ..
        } => editing::caret_key(state, *key, *pressed_at_ms),
        Input::CharacterTyped { trace_id, text } => {
            editing::character_typed(state, *trace_id, text)
        }
        Input::MarkReached { mark } => {
            let mut effects = say_all::mark_reached(state, *mark);
            effects.extend(terminal::mark_reached(state, *mark));
            effects
        }
        Input::SpeechCancelled => {
            terminal::cut(state);
            say_all::stop(state)
        }
        Input::Settings(settings) => {
            if state.settings.report_terminal_output && !settings.report_terminal_output {
                crate::terminal::drop_waiting(state);
            }
            state.settings = *settings;
            Vec::new()
        }
        Input::Fetches(fetches) => {
            state.fetches = *fetches;
            Vec::new()
        }
        // `Tick` is reserved vocabulary with no policy yet; `Input` is also
        // `#[non_exhaustive]`, so this arm doubles as the catch-all for
        // variants added by later milestones, until each grows a real
        // policy.
        _ => Vec::new(),
    }
}

/// How an event relates to the attention record (`docs/parity.md`, "Event
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
    window: Option<WindowFacts>,
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
        // Any other focus is judged by NVDA's own test against the system's
        // foreground window, made by its outpost when it read the event,
        // not against the attention record: a focus can reach Core before
        // the foreground change that moved the foreground away from its
        // window (D14, amended 2026-10-05). With no window facts, the
        // application decides.
        NormalizedEvent::FocusChanged { .. } => {
            let attended = match window {
                Some(event) => event_window_is_foreground(event),
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

/// Whether a focus event from `outpost`, observed at `observed_at_ms`, was
/// observed before the newest focus applied from another outpost. Events
/// from one outpost arrive in order; across outposts they can arrive out of
/// order, which NVDA's single event queue never does. An answer to a
/// focus-now query carries the time the outpost began reading it, so it is
/// ordered with the events by that; a report with no observation time
/// (0) is never stale.
///
/// A focus in the same top-level window as the newest focus is never stale:
/// one window can hold several applications (a Settings page's content
/// inside `ApplicationFrameHost`'s frame), and a foreground change is ordered
/// by when its window became the foreground, after the content's own focus
/// may have been observed (`docs/parity.md`, "Stale focus events").
fn is_stale_focus(
    state: &SrState,
    outpost: OutpostId,
    observed_at_ms: u64,
    window: Option<WindowFacts>,
) -> bool {
    let top_level = window.map(|window| window.top_level);
    observed_at_ms != 0
        && state.latest_focus.is_some_and(|(latest, at, latest_top)| {
            latest != outpost
                && observed_at_ms < at
                && !(top_level.is_some() && top_level == latest_top)
        })
}

/// Whether an event's window was in the system's foreground when its outpost
/// read it, by NVDA's test for accepting an event: inside the foreground
/// window or sharing its root owner, a topmost window, or a
/// `Windows.UI.Core` window under the input thread's active window.
fn event_window_is_foreground(event: WindowFacts) -> bool {
    event.in_foreground || event.topmost || event.under_active_window == Some(true)
}

/// Whether an event's window is one the attention record covers: the same
/// top-level window, the same root owner, a topmost window, or a
/// `Windows.UI.Core` window under the input thread's active window — NVDA's
/// foreground test, made against the attention record — or a window its
/// outpost found in the system's foreground window when it read the event.
/// When either side has no window facts there is nothing to compare, so the
/// application decides. Focus events with window facts are judged by
/// [`event_window_is_foreground`] instead.
fn window_is_attended(attention: &Attention, source: Pid, window: Option<WindowFacts>) -> bool {
    match (attention.window, window) {
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
    if let Some(pending) = state
        .pending_caret
        .take_if(|pending| pending.query_id == query_id)
    {
        return editing::caret_reply(state, trace_id, &pending, reply);
    }
    if let Some(focus_text) = state
        .focus_text
        .take_if(|pending| pending.selection_query == Some(query_id))
    {
        return editing::focus_selection(state, trace_id, focus_text.node, reply);
    }
    if let Some(pending) = state
        .pending_text
        .take_if(|pending| pending.query_id == query_id)
    {
        if matches!(
            pending.then,
            TextFollowUp::NavigatorSelection | TextFollowUp::NavigatorLine
        ) {
            return editing::navigator_text_reply(state, trace_id, &pending, reply);
        }
        return review_text::reply(state, trace_id, pending, reply);
    }
    if say_all::is_pending(state, query_id) {
        return say_all::reply(state, trace_id, reply);
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

fn reduce_event(
    state: &mut SrState,
    trace_id: TraceId,
    observed_at_ms: u64,
    source: Pid,
    window: Option<WindowFacts>,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    if let NormalizedEvent::FocusChanged { node, .. } = event
        && is_stale_focus(state, node.id.outpost(), observed_at_ms, window)
    {
        return Vec::new();
    }
    match classify(
        state.attention.as_ref(),
        state.focus_source(),
        source,
        window,
        event,
    ) {
        Acceptance::Dropped => return Vec::new(),
        Acceptance::Background => return reduce_background(trace_id, event),
        Acceptance::Attended => {}
    }
    if let NormalizedEvent::FocusChanged { node, .. } = event
        && observed_at_ms != 0
    {
        state.latest_focus = Some((
            node.id.outpost(),
            observed_at_ms,
            window.map(|window| window.top_level),
        ));
    }

    let effects = match event {
        NormalizedEvent::FocusChanged {
            node,
            foreground,
            ancestors,
            ancestors_unknown,
            selected_child,
        } => reduce_focus_changed(
            state,
            trace_id,
            source,
            window,
            &FocusReport {
                node,
                foreground: *foreground,
                ancestors,
                ancestors_unknown: *ancestors_unknown,
                selected_child: selected_child.as_ref(),
            },
        ),
        NormalizedEvent::SelectionChanged { node } => {
            reduce_selection_changed(state, trace_id, node)
        }
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
        NormalizedEvent::CaretMoved { node_id, caret } => {
            editing::update_caret(state, *node_id, caret.clone());
            editing::focus_caret(state, trace_id, *node_id)
        }
        NormalizedEvent::NoText { node_id } => editing::focus_value(state, trace_id, *node_id),
        NormalizedEvent::TextChanged { node_id } => {
            editing::text_changed(state, trace_id, *node_id)
        }
        NormalizedEvent::TerminalOutput { node_id, output } => {
            terminal::output(state, trace_id, *node_id, output)
        }
        NormalizedEvent::PropertyChanged { node_id, change } => match change {
            PropertyChange::Name(name) => {
                reduce_name_changed(state, trace_id, *node_id, name.as_ref())
            }
            PropertyChange::Value(value) => {
                reduce_value_changed(state, trace_id, *node_id, value.clone())
            }
            PropertyChange::States(new_states) => {
                reduce_states_changed(state, trace_id, *node_id, *new_states)
            }
            // `PropertyChange` is `#[non_exhaustive]`.
            _ => Vec::new(),
        },
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
fn reduce_background(trace_id: TraceId, event: &NormalizedEvent) -> Vec<Effect> {
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
    foreground: bool,
    ancestors: &'a [NodeSnapshot],
    /// The outpost could not read the ancestors in time.
    ancestors_unknown: bool,
    selected_child: Option<&'a NodeSnapshot>,
}

/// Handles an accepted focus change (`docs/parity.md`, "Focus and
/// announcements"):
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
/// - A focus identical to the one already announced is not spoken again
///   (NVDA's already-the-focus early return): the UIA focus callback and a
///   delivered fact can both report one control.
/// - Otherwise the newly entered containers, the node, and a selection
///   container's selected item are spoken, queued behind current speech
///   (which the key press that moved focus has already cut off), and the
///   navigator follows focus. When the outpost could not read the
///   ancestors in time, no container is announced and the previous focus's
///   chain is kept for the next comparison, so the next focus does not
///   announce every container again.
fn reduce_focus_changed(
    state: &mut SrState,
    trace_id: TraceId,
    source: Pid,
    window: Option<WindowFacts>,
    report: &FocusReport<'_>,
) -> Vec<Effect> {
    if report.foreground {
        state.attention = Some(Attention { source, window });
        if let Some(focus) = state.focus.as_ref()
            && let (Some(focus_window), Some(window)) = (focus.window, window)
            && focus_window.top_level == window.top_level
        {
            return Vec::new();
        }
    } else if let Some(window) = window
        && window.in_foreground
        && !state
            .attention
            .and_then(|attention| attention.window)
            .is_some_and(|attended| {
                attended.top_level == window.top_level || attended.root_owner == window.root_owner
            })
    {
        // Focus in the system's foreground window, unrelated to the attention
        // window: the foreground moved without a foreground fact, so
        // attention follows, as NVDA takes the foreground from the focus's
        // ancestry.
        state.attention = Some(Attention {
            source,
            window: Some(window),
        });
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
    let focus_window = window.or_else(|| {
        state
            .attention
            .filter(|attention| attention.source == source)
            .and_then(|attention| attention.window)
    });
    let new_focus = FocusContext {
        source,
        window: focus_window,
        snapshot: report.node.clone(),
        ancestors,
        last_selection: report.selected_child.map(|selected| selected.id),
        alive: true,
    };

    if let Some(focus) = state.focus.as_ref() {
        if focus.alive {
            // Already the focus: nothing is spoken, and a navigator the user
            // moved away stays where it is. As in NVDA, the focus is compared
            // by identity: not its states or name, which a second report can
            // read mid-change, and not its ancestors, which can read
            // differently from one report to the next (a window title still
            // being filled in, or a parent object read afresh through MSAA),
            // and not the selected item inside a list, which a selection
            // event announces. The newer reading is kept, except the
            // selected item already announced, so a selection event for a
            // new one is still spoken.
            if focus.snapshot.id == report.node.id {
                let mut kept = focus.clone();
                kept.snapshot = new_focus.snapshot;
                kept.ancestors = new_focus.ancestors;
                state.focus = Some(kept);
                return Vec::new();
            }
        } else if reads_the_same(focus, report.node, report.ancestors) {
            // The silent re-read: the replacement outpost reports the focus
            // the user already heard, so take its ids without speaking.
            state.focus = Some(new_focus);
            if state.navigator.is_none() {
                state.navigator = Some(Navigator::on(report.node.clone()));
            }
            return Vec::new();
        }
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
    let foreground_changed = foreground_changed(state.focus.as_ref(), report, focus_window);
    if report.foreground {
        state.foreground = Some(report.node.id);
    }
    let mut entered = entered_ancestors(state.focus.as_ref(), window, report);
    let foreground_window =
        unannounced_foreground_window(report, foreground_changed, window, &mut entered);
    let entering_menu = entered
        .iter()
        .any(|ancestor| matches!(ancestor.role, Role::MenuBar | Role::Menu | Role::MenuItem));
    let mut effects = vec![Effect::DropExpiredSpeech(FocusNow {
        focus: report.node.id,
        ancestors: new_focus
            .ancestors
            .iter()
            .map(|ancestor| ancestor.id)
            .collect(),
        foreground: state.foreground,
    })];
    end_text_activity(
        state,
        report.node.id,
        foreground_changed || entering_menu,
        &mut effects,
    );
    state.focus = Some(new_focus);
    state.navigator = Some(navigator);
    if report.foreground && !has_text(report.node.name.as_deref()) {
        return effects;
    }

    editing::await_focus_text(state, report.node);
    effects.extend(focus_speech(trace_id, report, foreground_window, entered));
    effects
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
    state.focus_text = None;
    state.typed_word.clear();
    state.held_typing.clear();
    crate::terminal::focus_moved(state, node);
    if state.caret.as_ref().is_some_and(|caret| caret.node != node) {
        state.caret = None;
    }
}

/// The top-level window to announce with a focus that moved into another
/// top-level window, the system's foreground window, when no foreground
/// report announced it: Windows can raise a new window's foreground event
/// while still refusing it the foreground, and raise none when it gets the
/// foreground later. NVDA takes the foreground window from the focus's
/// ancestry then (`eventHandler.doPreGainFocus`) and announces it as an
/// entered ancestor, so it is the outermost ancestor, which is the
/// top-level window, whatever its role. It is removed from `entered`, so it
/// is not spoken twice.
fn unannounced_foreground_window<'a>(
    report: &FocusReport<'a>,
    foreground_changed: bool,
    window: Option<WindowFacts>,
    entered: &mut Vec<&'a NodeSnapshot>,
) -> Option<&'a NodeSnapshot> {
    if report.foreground
        || !foreground_changed
        || !window.is_some_and(|window| window.in_foreground)
    {
        return None;
    }
    let top = report
        .ancestors
        .first()
        .filter(|top| has_text(top.name.as_deref()))?;
    entered.retain(|ancestor| ancestor.id != top.id);
    Some(top)
}

/// Whether a new focus brings a new foreground window, which cancels
/// speech: a foreground report, or a focus in another top-level window
/// than the previous focus (NVDA's foreground event, run when the top of
/// the focus ancestry changes).
fn foreground_changed(
    previous: Option<&FocusContext>,
    report: &FocusReport<'_>,
    window: Option<WindowFacts>,
) -> bool {
    let previous_top = previous
        .and_then(|focus| focus.window)
        .map(|window| window.top_level);
    report.foreground
        || match (previous_top, window.map(|window| window.top_level)) {
            (Some(old), Some(new)) => old != new,
            (None, Some(_)) => true,
            _ => false,
        }
}

/// The speech for a new focus: an unannounced foreground window, then each
/// entered container as its own utterance, valid while the focus is inside
/// it, then the focus, valid while it is the focus. A focus that may have
/// text leaves its value out, since its text follows
/// (`editing::await_focus_text`).
fn focus_speech(
    trace_id: TraceId,
    report: &FocusReport<'_>,
    foreground_window: Option<&NodeSnapshot>,
    entered: Vec<&NodeSnapshot>,
) -> Vec<Effect> {
    let reads_text = editing::may_have_text(report.node.role);
    let mut effects = Vec::new();
    if let Some(top) = foreground_window {
        effects.push(Effect::Speak(Utterance {
            trace_id,
            priority: SpeechPriority::Queued,
            segments: node_segments(top, Reason::Focus),
            source: Some(source_of(top)),
            say_all: false,
            validity: Some(FocusValidity {
                node: top.id,
                had_focus: false,
            }),
        }));
    }
    for container in entered
        .into_iter()
        .filter(|ancestor| is_presentable_container(ancestor))
    {
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
        node_segments(&node, Reason::Focus)
    } else {
        node_segments(report.node, Reason::Focus)
    };
    // A selection container introduces its selected item right after
    // itself — the roadmap's "announce a focused list's selected item".
    if let Some(selected) = report.selected_child {
        segments.extend(node_segments(selected, Reason::Focus));
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

/// Whether a name or description has real, non-whitespace text.
fn has_text(text: Option<&str>) -> bool {
    text.is_some_and(|text| !text.trim().is_empty())
}

/// Whether a reported focus reads the same as the dead focus's kept copy:
/// role, name, value, and states, and the ancestors' names and roles. Node
/// ids are not compared, since a replacement outpost issues new ones.
fn reads_the_same(focus: &FocusContext, node: &NodeSnapshot, ancestors: &[NodeSnapshot]) -> bool {
    let kept = &focus.snapshot;
    kept.role == node.role
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
        state.caret = None;
    }
    if state
        .pending_caret
        .as_ref()
        .is_some_and(|pending| pending.node.outpost() == outpost)
    {
        state.pending_caret = None;
    }
    if state
        .focus_text
        .is_some_and(|pending| pending.node.outpost() == outpost)
    {
        state.focus_text = None;
    }
    if state
        .pending_text
        .as_ref()
        .is_some_and(|pending| pending.node.outpost() == outpost)
    {
        state.pending_text = None;
    }
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

/// Handles a name change: when the focused node's name changes, the new name
/// alone is spoken, queued behind current speech, as NVDA does. A name
/// change on any other node, including an ancestor of the focus, is silent.
fn reduce_name_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    name: Option<&String>,
) -> Vec<Effect> {
    if !state.focus_matches(node_id) {
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
    vec![Effect::Speak(announce_node(
        trace_id,
        SpeechPriority::Queued,
        node,
        Reason::Focus,
    ))]
}

/// Handles a UIA `AutomationNotification` event (NVDA's
/// `event_UIA_notification`) from the focus's application: announce the
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
    let utterance = if editing::may_have_text(object.role) && object.value.is_some() {
        let mut announced = object.clone();
        announced.value = None;
        announce_node(trace_id, SpeechPriority::Queued, &announced, reason)
    } else {
        announce_node(trace_id, SpeechPriority::Queued, object, reason)
    };
    let mut effects = vec![Effect::Speak(utterance)];
    effects.extend(editing::navigator_text(state, trace_id, object));
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

/// The text the report-object copy press puts on the clipboard: the
/// object's name and value joined by a space, each included only when
/// present, matching what a user reading the object would expect to paste.
fn clipboard_text(node: &NodeSnapshot) -> String {
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
fn spelled(text: &str) -> Vec<UtteranceSegment> {
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
        ReviewCommand::ReviewPreviousWord => match review::previous_word_start(&text, offset) {
            Some(target) => (target, review::word_span(&text, target), None),
            None => (offset, review::word_span(&text, offset), Some(Message::Top)),
        },
        ReviewCommand::ReviewNextWord => match review::next_word_start(&text, offset) {
            Some(target) => (target, review::word_span(&text, target), None),
            None => (
                offset,
                review::word_span(&text, offset),
                Some(Message::Bottom),
            ),
        },
        ReviewCommand::ReviewCurrentWord => {
            let span = review::word_span(&text, offset);
            (span.0, span, None)
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
    // A unit with nothing to read is "blank"; spelled, a space is "space".
    let blank = slice.is_empty() || (!repeated_current && slice.trim().is_empty());
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
    } else if matches!(
        command,
        ReviewCommand::ReviewPreviousCharacter
            | ReviewCommand::ReviewNextCharacter
            | ReviewCommand::ReviewCurrentCharacter
    ) {
        // A single character is spoken as NVDA spells it, so a capital is
        // raised in pitch.
        segments.extend(spelled(slice));
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

/// Whether a focused role is a selection container whose interior selection
/// changes are announced — the reducer-side twin of the outpost's
/// enrichment filter, kept to the same roles: lists (a list box, a category
/// list) and tab controls (an Explorer or Settings tab strip). Combo boxes
/// are deliberately excluded: their selection reaches the reducer as a
/// value change already, and announcing both would double-speak every
/// pick.
fn is_selection_container(role: Role) -> bool {
    matches!(role, Role::List | Role::TabControl)
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
    // Selecting the focused item itself is a change of its state, as NVDA
    // handles a selection on the focus: "selected" is spoken.
    if node.id == focus.snapshot.id {
        let states = node.states;
        return reduce_states_changed(state, trace_id, node.id, states);
    }
    if focus.snapshot.id.outpost() != node.id.outpost()
        || !is_selection_container(focus.snapshot.role)
        || focus.last_selection == Some(node.id)
    {
        return Vec::new();
    }
    focus.last_selection = Some(node.id);
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments: node_segments(node, Reason::Focus),
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
        Reason::Focus,
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
    if !state.focus_matches(node_id) {
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

/// Handles a complete state-set replacement on the focused node (MSAA
/// `EVENT_OBJECT_STATECHANGE` and equivalent UIA property changes carry the
/// whole new set, not a delta). Diffs against the stored snapshot and
/// announces, queued, the gained states and the lost states
/// spoken by their absence, by the rules and in the order of "Which states
/// are spoken, and in what order" in `docs/nvda/speech.md`. Ignored for any
/// node other than the focused one; a no-op if nothing speakable changed.
fn reduce_states_changed(
    state: &mut SrState,
    trace_id: TraceId,
    node_id: NodeId,
    new_states: StateSet,
) -> Vec<Effect> {
    if !state.focus_matches(node_id) {
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
    let segments = ordered_state_segments(positive, negative);
    if segments.is_empty() {
        return Vec::new();
    }

    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Queued,
        segments,
        source: Some(utterance_source),
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
/// its ancestors is the same node. Node ids are comparable only within one
/// outpost, and one top-level window can hold elements of two processes (a
/// Settings page's frame belongs to `ApplicationFrameHost.exe`), so when the
/// new focus is in the same top-level window as the previous one, a previous
/// node with the same role and name also counts. A focus in a different
/// top-level window, or with no previous focus, enters its whole chain.
///
/// Entered menu bars, menus, and menu items are never announced: NVDA
/// cancels speech and stays silent for them. Only the presentable ones
/// ([`is_presentable_container`]) are spoken. None are entered when the
/// outpost could not read the ancestors in time.
fn entered_ancestors<'a>(
    previous: Option<&FocusContext>,
    window: Option<WindowFacts>,
    report: &FocusReport<'a>,
) -> Vec<&'a NodeSnapshot> {
    if report.ancestors_unknown {
        return Vec::new();
    }
    let same_window = previous.is_some_and(|previous| {
        matches!((previous.window, window), (Some(old), Some(new)) if old.top_level == new.top_level)
    });
    let already_entered = |ancestor: &NodeSnapshot| {
        let Some(previous) = previous else {
            return false;
        };
        previous
            .ancestors
            .iter()
            .chain(std::iter::once(&previous.snapshot))
            .any(|old| {
                old.id == ancestor.id
                    || (same_window && old.role == ancestor.role && old.name == ancestor.name)
            })
    };
    report
        .ancestors
        .iter()
        .filter(|ancestor| !already_entered(ancestor))
        .collect()
}

/// Whether a focus ancestor adds context worth speaking when focus first
/// enters it. The rule is exclusion-based: an ancestor speaks unless it is
/// one of the following.
///
/// - An item-level or text-entry role (`TreeItem`, `ListItem`,
///   `EditableText`). Focus lands on these; they are never context.
/// - A structural node with no semantics of its own (`Unknown`, `Pane`).
/// - A menu bar, menu, or menu item: entering one is silent, as in NVDA.
/// - A `Window`, `Group`, or `PropertyPage` with neither a name nor a
///   description, which would speak as a bare role. NVDA treats these as
///   layout; a named window is announced on entry like any container.
/// - `StaticText` with no text, which is nothing.
///
/// Whitespace-only names and descriptions count as absent. Everything else
/// speaks on entry whether named or not: dialogs, toolbars, and an unnamed
/// tree, which announces as a bare "tree view", the same as NVDA. The
/// behavior is recorded under focus-ancestry context in `docs/parity.md`.
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
        | Role::TitleBar
        | Role::Unknown
        | Role::Pane
        | Role::MenuBar
        | Role::Menu
        | Role::MenuItem => false,
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
    node_segments(&entered, Reason::Focus)
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
        Role::CheckBox | Role::RadioButton | Role::Link | Role::MenuItem | Role::Application
    )
}

/// Whether a role reports its own text as it changes, so a change of value
/// is not spoken: an edit field or a document.
fn reports_text_itself(role: Role) -> bool {
    matches!(role, Role::EditableText | Role::Document)
}

/// Whether a node announced for `reason` speaks its role: always, unless
/// the reason is focus, the node has a name or a value to hear instead, and
/// its role is one left silent on focus.
fn speaks_role(node: &NodeSnapshot, reason: Reason) -> bool {
    let something_else = node.name.as_deref().is_some_and(|name| !name.is_empty())
        || node.value.as_deref().is_some_and(|value| !value.is_empty());
    !(reason == Reason::Focus && something_else && is_silent_on_focus(node.role))
}

/// The full announcement for a node, in NVDA's property order: name, role,
/// value, states, description, keyboard shortcut, position in set, level —
/// each as its semantic span kind, never anonymous text (decision D12).
/// Detail spans simply do not appear when the backend reported nothing; the
/// role does not appear when `reason` leaves it silent ([`speaks_role`]).
fn node_segments(node: &NodeSnapshot, reason: Reason) -> Vec<UtteranceSegment> {
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
    if let Some(position) = node.details.position_in_set {
        segments.push(UtteranceSegment::new(SegmentContent::Position {
            position,
            set_size: node.details.set_size,
        }));
    }
    if let Some(level) = node.details.level {
        segments.push(UtteranceSegment::new(SegmentContent::Level(level)));
    }
    segments
}

/// Builds a complete announcement utterance for a node (no entered-context
/// prefix): navigation, toast, and report-object announcements.
fn announce_node(
    trace_id: TraceId,
    priority: SpeechPriority,
    node: &NodeSnapshot,
    reason: Reason,
) -> Utterance {
    Utterance {
        trace_id,
        priority,
        segments: node_segments(node, reason),
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
const STATE_ORDER: [State; 15] = [
    State::Disabled,
    State::Focused,
    State::Selected,
    State::Busy,
    State::Pressed,
    State::Checked,
    State::Mixed,
    State::ReadOnly,
    State::Expanded,
    State::Collapsed,
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
    if reason == StateReason::Query {
        return spoken;
    }
    spoken.remove(State::Focused);
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
