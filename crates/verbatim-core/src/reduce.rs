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

use verbatim_model::{
    Effect, FetchResult, Input, NodeId, NodeSnapshot, NormalizedEvent, Notification,
    NotificationProcessing, OutpostId, Pid, PropertyChange, Query, QueryId, QueryKind,
    ReviewCommand, Role, SegmentContent, SpeechPriority, State, StateSet, TraceId, Utterance,
    UtteranceSegment, UtteranceSource, WindowFacts,
};

use crate::review;
use crate::state::{Attention, FocusContext, Navigator, PendingNavigation, SrState};

/// The activity id of the shell's window-snap results notification, the one
/// UIA notification spoken from any application (`docs/parity.md`, "Event
/// acceptance").
const SNAP_RESULTS_ACTIVITY: &str = "Windows.Shell.SnapComponent.SnapHotKeyResults";

/// Advances `state` by one `input`, returning the new state and the effects
/// the imperative shell must execute.
///
/// Pure: no I/O, no clocks, no randomness, and no mutation of `state` itself
/// — the returned `SrState` is a new value.
#[must_use]
pub fn reduce(state: &SrState, input: &Input) -> (SrState, Vec<Effect>) {
    let mut next = state.clone();
    let effects = match input {
        Input::Event {
            trace_id,
            source,
            window,
            event,
            ..
        } => reduce_event(&mut next, *trace_id, *source, *window, event),
        Input::FetchCompleted {
            trace_id,
            query_id,
            kind,
            result,
        } => reduce_navigate_completed(&mut next, *trace_id, *query_id, *kind, result),
        Input::OutpostEnded { outpost } => {
            outpost_ended(&mut next, *outpost);
            Vec::new()
        }
        Input::Command {
            trace_id,
            command,
            repeat,
        } => reduce_command(&mut next, *trace_id, *command, *repeat),
        // `Tick` is reserved vocabulary with no policy yet; `Input` is also
        // `#[non_exhaustive]`, so this arm doubles as the catch-all for
        // variants added by later milestones, until each grows a real
        // policy.
        _ => Vec::new(),
    };
    (next, effects)
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

/// Classifies one event against the attention record. A foreground change is
/// always attended: its intake has already confirmed the window is the
/// system's foreground window. With no attention yet, everything is
/// attended, since there is nothing to compare against.
fn classify(
    attention: Option<&Attention>,
    source: Pid,
    window: Option<WindowFacts>,
    event: &NormalizedEvent,
) -> Acceptance {
    let Some(attention) = attention else {
        return Acceptance::Attended;
    };
    match event {
        NormalizedEvent::FocusChanged {
            foreground: true, ..
        } => Acceptance::Attended,
        // UIA notifications are filtered by application, not window, as
        // NVDA filters them.
        // A toast is spoken from anywhere.
        NormalizedEvent::Alert { .. } => {
            if window_is_attended(attention, source, window) {
                Acceptance::Attended
            } else {
                Acceptance::Background
            }
        }
        NormalizedEvent::Notification { notification, .. } => {
            if source == attention.source {
                Acceptance::Attended
            } else if notification.activity_id.as_deref() == Some(SNAP_RESULTS_ACTIVITY) {
                Acceptance::Background
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

/// Whether an event's window is one the attention record covers: the same
/// top-level window, the same root owner, a topmost window, or a
/// `Windows.UI.Core` window under the input thread's active window — NVDA's
/// foreground test, made against the attention record — or a window its
/// outpost found in the system's foreground window when it read the event. When either side has no window facts there is nothing to
/// compare, so the application decides.
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

fn reduce_event(
    state: &mut SrState,
    trace_id: TraceId,
    source: Pid,
    window: Option<WindowFacts>,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    match classify(state.attention.as_ref(), source, window, event) {
        Acceptance::Dropped => return Vec::new(),
        Acceptance::Background => return reduce_background(trace_id, event),
        Acceptance::Attended => {}
    }

    match event {
        NormalizedEvent::FocusChanged {
            node,
            foreground,
            ancestors,
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
                selected_child: selected_child.as_ref(),
            },
        ),
        NormalizedEvent::SelectionChanged { node } => {
            reduce_selection_changed(state, trace_id, node)
        }
        NormalizedEvent::Notification {
            node_id: _,
            notification,
        } => reduce_notification(trace_id, notification),
        NormalizedEvent::Alert { node, .. } => reduce_alert(trace_id, node),
        NormalizedEvent::ValueChanged { node_id, value } => {
            reduce_value_changed(state, trace_id, *node_id, value.clone())
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
    })]
}

/// The parts of a `FocusChanged` event the focus handling reads.
struct FocusReport<'a> {
    node: &'a NodeSnapshot,
    foreground: bool,
    ancestors: &'a [NodeSnapshot],
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
///   container's selected item are spoken, interrupting current speech, and
///   the navigator follows focus.
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

    let new_focus = FocusContext {
        source,
        window,
        snapshot: report.node.clone(),
        ancestors: report.ancestors.to_vec(),
        last_selection: report.selected_child.map(|selected| selected.id),
        alive: true,
    };

    if let Some(focus) = state.focus.as_ref() {
        if focus.alive {
            // Already the focus: nothing changes, and a navigator the user
            // moved away stays where it is.
            if focus.snapshot == *report.node
                && focus.ancestors == report.ancestors
                && focus.last_selection == new_focus.last_selection
            {
                return Vec::new();
            }
        } else if reads_the_same(focus, report.node, report.ancestors) {
            // The silent re-read: the replacement outpost reports the focus
            // the user already heard, so take its ids without speaking.
            state.focus = Some(new_focus);
            if state.navigator.is_none() {
                state.navigator = Some(Navigator {
                    object: report.node.clone(),
                    review_offset: 0,
                });
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
    let navigator = Navigator {
        object: report.node.clone(),
        review_offset: 0,
    };
    if report.foreground && !has_text(report.node.name.as_deref()) {
        state.focus = Some(new_focus);
        state.navigator = Some(navigator);
        return Vec::new();
    }

    let mut segments = Vec::new();
    for container in entered_containers(state.focus.as_ref(), window, report.ancestors) {
        segments.extend(container_segments(container));
    }
    segments.extend(node_segments(report.node, Reason::Focus));
    // A selection container introduces its selected item right after
    // itself — the roadmap's "announce a focused list's selected item".
    if let Some(selected) = report.selected_child {
        segments.extend(node_segments(selected, Reason::Focus));
    }
    let utterance = Utterance {
        trace_id,
        priority: SpeechPriority::Interrupt,
        segments,
        source: Some(source_of(report.node)),
    };
    state.focus = Some(new_focus);
    state.navigator = Some(navigator);
    vec![Effect::Speak(utterance)]
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
fn outpost_ended(state: &mut SrState, outpost: OutpostId) {
    if let Some(focus) = state.focus.as_mut()
        && focus.snapshot.id.outpost() == outpost
    {
        focus.alive = false;
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
/// `event_UIA_notification`) from the attention application: announce the
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
    // "To focus" is meaningful even with the navigator already on focus;
    // handle it before the navigator-present guard so it can seed one.
    if command == ReviewCommand::ToFocus {
        return navigator_to_focus(state, trace_id);
    }
    let Some(navigator) = state.navigator.as_ref() else {
        return Vec::new();
    };

    match command {
        ReviewCommand::ToFocus => unreachable!("handled above"),
        ReviewCommand::ReportObject => report_object(navigator, trace_id, repeat),
        ReviewCommand::Activate => vec![Effect::Activate {
            node_id: navigator.object.id,
        }],
        ReviewCommand::Parent => navigate(state, trace_id, QueryKind::Parent),
        ReviewCommand::NextSibling => navigate(state, trace_id, QueryKind::NextSibling),
        ReviewCommand::PreviousSibling => navigate(state, trace_id, QueryKind::PreviousSibling),
        ReviewCommand::FirstChild => navigate(state, trace_id, QueryKind::FirstChild),
        _ => review_text_command(state, trace_id, command),
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
    let utterance = announce_node(trace_id, SpeechPriority::Interrupt, &object, Reason::Focus);
    state.navigator = Some(Navigator {
        object,
        review_offset: 0,
    });
    vec![Effect::Speak(utterance)]
}

/// Reports the navigator object: on the first press its full announcement,
/// on the second its text spelled character by character, on the third its
/// name and value copied to the clipboard (NVDA's multi-press semantics).
fn report_object(navigator: &Navigator, trace_id: TraceId, repeat: u8) -> Vec<Effect> {
    match repeat {
        0 => vec![Effect::Speak(announce_node(
            trace_id,
            SpeechPriority::Interrupt,
            &navigator.object,
            Reason::Query,
        ))],
        1 => {
            let text = review::text_of(&navigator.object);
            let segments = text
                .chars()
                .map(|ch| UtteranceSegment::text(ch.to_string()))
                .collect::<Vec<_>>();
            if segments.is_empty() {
                return Vec::new();
            }
            vec![Effect::Speak(Utterance {
                trace_id,
                priority: SpeechPriority::Interrupt,
                segments,
                source: Some(source_of(&navigator.object)),
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

/// Runs a review-cursor text command over the navigator object's review
/// text (see [`review`]). Moves the cursor and announces the line, word, or
/// character it lands on. At a text boundary the motion stays put and
/// re-reads the current unit, matching how a screen reader reports the edge.
fn review_text_command(
    state: &mut SrState,
    trace_id: TraceId,
    command: ReviewCommand,
) -> Vec<Effect> {
    let Some(navigator) = state.navigator.as_mut() else {
        return Vec::new();
    };
    let text = review::text_of(&navigator.object);
    let offset = navigator.review_offset.min(text.len());

    let (new_offset, spoken) = match command {
        ReviewCommand::ReviewTop => (0, review::line_span(&text, 0)),
        ReviewCommand::ReviewBottom => {
            let start = review::line_span(&text, text.len()).0;
            (start, review::line_span(&text, start))
        }
        ReviewCommand::ReviewPreviousLine => {
            let (start, _) = review::line_span(&text, offset);
            let target = review::previous_char(&text, start).unwrap_or(start);
            let span = review::line_span(&text, target);
            (span.0, span)
        }
        ReviewCommand::ReviewNextLine => {
            let (_, end) = review::line_span(&text, offset);
            if end >= text.len() {
                (
                    review::line_span(&text, offset).0,
                    review::line_span(&text, offset),
                )
            } else {
                let span = review::line_span(&text, end + 1);
                (span.0, span)
            }
        }
        // Current-line and start-of-line both land the cursor at the line
        // start and read the whole line; the only difference a text model
        // (M4) will draw between them is the reported position, not the
        // spoken text.
        ReviewCommand::ReviewCurrentLine | ReviewCommand::ReviewStartOfLine => {
            let span = review::line_span(&text, offset);
            (span.0, span)
        }
        ReviewCommand::ReviewEndOfLine => {
            let span = review::line_span(&text, offset);
            (span.1, span)
        }
        ReviewCommand::ReviewPreviousWord => {
            let target = review::previous_word_start(&text, offset).unwrap_or(offset);
            (target, review::word_span(&text, target))
        }
        ReviewCommand::ReviewNextWord => match review::next_word_start(&text, offset) {
            Some(target) => (target, review::word_span(&text, target)),
            None => (offset, review::word_span(&text, offset)),
        },
        ReviewCommand::ReviewCurrentWord => {
            let span = review::word_span(&text, offset);
            (span.0, span)
        }
        ReviewCommand::ReviewPreviousCharacter => {
            let target = review::previous_char(&text, offset).unwrap_or(offset);
            (
                target,
                review::char_span(&text, target).unwrap_or((target, target)),
            )
        }
        ReviewCommand::ReviewNextCharacter => match review::char_span(&text, offset) {
            Some((_, next)) if next < text.len() => {
                (next, review::char_span(&text, next).unwrap_or((next, next)))
            }
            _ => (
                offset,
                review::char_span(&text, offset).unwrap_or((offset, offset)),
            ),
        },
        ReviewCommand::ReviewCurrentCharacter => {
            let span = review::char_span(&text, offset).unwrap_or((offset, offset));
            (offset, span)
        }
        _ => return Vec::new(),
    };

    navigator.review_offset = new_offset;
    let (start, end) = spoken;
    let slice = text.get(start..end).unwrap_or("");
    if slice.is_empty() {
        return Vec::new();
    }
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Interrupt,
        segments: vec![UtteranceSegment::text(slice.to_owned())],
        source: None,
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
    if focus.snapshot.id.outpost() != node.id.outpost()
        || !is_selection_container(focus.snapshot.role)
        || node.id == focus.snapshot.id
        || focus.last_selection == Some(node.id)
    {
        return Vec::new();
    }
    focus.last_selection = Some(node.id);
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Interrupt,
        segments: node_segments(node, Reason::Focus),
        source: Some(source_of(node)),
    })]
}

/// Shared handling for `ValueChanged` and `PropertyChanged(Value(..))`: both
/// speak the bare new value, only when the changed node is the focused one.
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
    focus.snapshot.value.clone_from(&value);
    let Some(text) = value else {
        return Vec::new();
    };
    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Interrupt,
        segments: vec![UtteranceSegment::value(text)],
        source: Some(source_of(&focus.snapshot)),
    })]
}

/// Handles a complete state-set replacement on the focused node (MSAA
/// `EVENT_OBJECT_STATECHANGE` and equivalent UIA property changes carry the
/// whole new set, not a delta). Diffs against the stored snapshot and
/// announces, Interrupt priority: every newly gained announceable state
/// (using the same order and exclusions as [`announce_node`]), plus two
/// "toggled off" cases that would otherwise be silent — losing `Checked`
/// with no `Mixed` present on a check box or radio button announces
/// `NegatedState(Checked)`; losing `Pressed` on a toggle button announces
/// `NegatedState(Pressed)`. Ignored for any node other than the focused one;
/// a no-op if the set did not actually change.
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

    let mut segments = Vec::new();

    let gained_checked =
        new_states.contains(State::Checked) && !old_states.contains(State::Checked);
    let lost_checked_unchecked = matches!(role, Role::CheckBox | Role::RadioButton)
        && old_states.contains(State::Checked)
        && !new_states.contains(State::Checked)
        && !new_states.contains(State::Mixed);
    if gained_checked {
        segments.push(UtteranceSegment::new(SegmentContent::State(State::Checked)));
    } else if lost_checked_unchecked {
        segments.push(UtteranceSegment::new(SegmentContent::NegatedState(
            State::Checked,
        )));
    }

    let gained_pressed =
        new_states.contains(State::Pressed) && !old_states.contains(State::Pressed);
    let lost_pressed = role == Role::ToggleButton
        && old_states.contains(State::Pressed)
        && !new_states.contains(State::Pressed);
    if gained_pressed {
        segments.push(UtteranceSegment::new(SegmentContent::State(State::Pressed)));
    } else if lost_pressed {
        segments.push(UtteranceSegment::new(SegmentContent::NegatedState(
            State::Pressed,
        )));
    }

    for candidate in [
        State::Mixed,
        State::Selected,
        State::Expanded,
        State::Collapsed,
        State::HasPopup,
        State::DefaultControl,
        State::ReadOnly,
        State::Disabled,
        State::Busy,
    ] {
        if new_states.contains(candidate) && !old_states.contains(candidate) {
            segments.push(UtteranceSegment::new(SegmentContent::State(candidate)));
        }
    }

    if segments.is_empty() {
        return Vec::new();
    }

    vec![Effect::Speak(Utterance {
        trace_id,
        priority: SpeechPriority::Interrupt,
        segments,
        source: Some(utterance_source),
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
            let utterance =
                announce_node(trace_id, SpeechPriority::Interrupt, snapshot, Reason::Focus);
            state.navigator = Some(Navigator {
                object: snapshot.clone(),
                review_offset: 0,
            });
            vec![Effect::Speak(utterance)]
        }
        FetchResult::Gone => navigator_to_focus(state, trace_id),
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
                priority: SpeechPriority::Interrupt,
                segments: vec![UtteranceSegment::new(SegmentContent::Message(message))],
                source: None,
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
fn source_of(node: &NodeSnapshot) -> UtteranceSource {
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
/// cancels speech and stays silent for them, and the focus announcement
/// that follows interrupts current speech anyway.
fn entered_containers<'a>(
    previous: Option<&FocusContext>,
    window: Option<WindowFacts>,
    ancestors: &'a [NodeSnapshot],
) -> Vec<&'a NodeSnapshot> {
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
    ancestors
        .iter()
        .filter(|ancestor| is_presentable_container(ancestor))
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

/// The spoken introduction for one entered container: label, role, and
/// description when present.
fn container_segments(node: &NodeSnapshot) -> Vec<UtteranceSegment> {
    let mut segments = Vec::new();
    if let Some(name) = &node.name {
        segments.push(UtteranceSegment::label(name.clone()));
    }
    segments.push(UtteranceSegment::new(SegmentContent::Role(node.role)));
    if let Some(description) = &node.details.description {
        segments.push(UtteranceSegment::new(SegmentContent::Description(
            description.clone(),
        )));
    }
    segments
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
            | Role::ListItem
            | Role::MenuItem
            | Role::TreeItem
            | Role::StaticText
    )
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
    if let Some(value) = &node.value {
        segments.push(UtteranceSegment::value(value.clone()));
    }
    segments.extend(state_segments(node.role, node.states));
    if let Some(description) = &node.details.description {
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
    }
}

/// Announcement order for states: checked (or its negation for check boxes
/// and radio buttons that carry neither checked nor mixed), pressed (or its
/// negation "not pressed" for a toggle button that does not carry it),
/// mixed, expanded, collapsed, has-popup, default, read-only, disabled,
/// busy, and finally "not selected" for a selectable node that is not
/// selected. Focus-related states (focused, focusable, offscreen) are never
/// announced — they describe capability, not content. Positive `Selected`
/// is never announced on a node announcement either, matching NVDA: a
/// focused item being selected is the expected default, so only its
/// notable absence is spoken. Selection *changes* still announce "selected"
/// through the state-change diff, which keeps its own list.
fn state_segments(role: Role, states: StateSet) -> Vec<UtteranceSegment> {
    let mut segments = Vec::new();

    if states.contains(State::Checked) {
        segments.push(UtteranceSegment::new(SegmentContent::State(State::Checked)));
    } else if matches!(role, Role::CheckBox | Role::RadioButton) && !states.contains(State::Mixed) {
        segments.push(UtteranceSegment::new(SegmentContent::NegatedState(
            State::Checked,
        )));
    }

    if states.contains(State::Pressed) {
        segments.push(UtteranceSegment::new(SegmentContent::State(State::Pressed)));
    } else if role == Role::ToggleButton {
        segments.push(UtteranceSegment::new(SegmentContent::NegatedState(
            State::Pressed,
        )));
    }

    for state in [
        State::Mixed,
        State::Expanded,
        State::Collapsed,
        State::HasPopup,
        State::DefaultControl,
        State::ReadOnly,
        State::Disabled,
        State::Busy,
    ] {
        if states.contains(state) {
            segments.push(UtteranceSegment::new(SegmentContent::State(state)));
        }
    }

    if states.contains(State::Selectable) && !states.contains(State::Selected) {
        segments.push(UtteranceSegment::new(SegmentContent::NegatedState(
            State::Selected,
        )));
    }

    segments
}
