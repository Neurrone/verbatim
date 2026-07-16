//! The reducer (architecture section 2): pure state-and-effect transitions
//! from normalized accessibility events and fetch completions.
//!
//! Nothing in this module performs I/O, reads a clock, or consults
//! randomness; the only way the outside world enters is through the `Input`
//! passed to [`reduce`]. That purity is what lets the flight recorder turn a
//! captured sequence of inputs into a deterministic regression test (see
//! [`crate::replay`]).

use verbatim_model::{
    Effect, FetchResult, Input, NodeId, NodeSnapshot, NormalizedEvent, Notification,
    NotificationProcessing, Pid, PropertyChange, Query, QueryKind, ReviewCommand, Role,
    SegmentContent, SnapshotVersion, SpeechPriority, State, StateSet, TraceId, Utterance,
    UtteranceSegment, UtteranceSource,
};

use crate::review;
use crate::state::{FetchReason, FocusContext, Navigator, PendingFetch, SrState};

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
            backend: _,
            version,
            event,
        } => reduce_event(&mut next, *trace_id, *source, *version, event),
        Input::FetchCompleted {
            trace_id,
            query_id,
            result,
        } => reduce_fetch_completed(&mut next, *trace_id, *query_id, result),
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

fn reduce_event(
    state: &mut SrState,
    trace_id: TraceId,
    source: Pid,
    version: SnapshotVersion,
    event: &NormalizedEvent,
) -> Vec<Effect> {
    if state.is_stale(source, version) {
        return refetch_focus(state);
    }
    state.record_version(source, version);

    match event {
        NormalizedEvent::FocusChanged {
            node,
            ancestors,
            selected_child,
        } => {
            // Suppress a focus event identical to the one already announced
            // from the same application, back to back: the UIA focus
            // callback and a foreground re-announcement can both emit a
            // FocusChanged for one control, and rapid duplicate foreground
            // events re-report an unchanged focus. This is NVDA's
            // already-the-focus early return. It never suppresses a genuine
            // return to a window after visiting another: that path focuses
            // the other application's control in between, so this event is
            // no longer identical to the last announced one.
            if let Some(focus) = state.focus.as_ref()
                && focus.source == source
                && &focus.last_announced == node
                && focus.ancestors == *ancestors
                && focus.last_selection == selected_child.as_ref().map(|selected| selected.id)
            {
                return Vec::new();
            }
            let mut segments = Vec::new();
            for container in entered_containers(state.focus.as_ref(), source, ancestors) {
                segments.extend(container_segments(container));
            }
            segments.extend(node_segments(node));
            // A selection container introduces its selected item right
            // after itself — the roadmap's "announce a focused list's
            // selected item".
            if let Some(selected) = selected_child {
                segments.extend(node_segments(selected));
            }
            let utterance = Utterance {
                trace_id,
                priority: SpeechPriority::Interrupt,
                segments,
                source: Some(source_of(node)),
            };
            state.focus = Some(FocusContext {
                source,
                snapshot: node.clone(),
                last_announced: node.clone(),
                ancestors: ancestors.clone(),
                last_selection: selected_child.as_ref().map(|selected| selected.id),
            });
            // The review cursor follows focus (roadmap M3): every focus
            // change snaps the navigator object to the new focus and resets
            // the review cursor to its start. This deliberately does not
            // touch `latest_navigation`: an app-initiated focus event is not
            // newer user intent than an object-navigation command already
            // in flight, so a completion for that command must still be
            // free to apply once it lands (see `SrState::latest_navigation`).
            state.navigator = Some(Navigator {
                source,
                object: node.clone(),
                review_offset: 0,
            });
            vec![Effect::Speak(utterance)]
        }
        NormalizedEvent::SelectionChanged { node } => {
            reduce_selection_changed(state, trace_id, source, node)
        }
        NormalizedEvent::Notification {
            node_id: _,
            notification,
        } => reduce_notification(trace_id, notification),
        NormalizedEvent::ValueChanged { node_id, value } => {
            reduce_value_changed(state, trace_id, source, *node_id, value.clone())
        }
        NormalizedEvent::PropertyChanged { node_id, change } => match change {
            PropertyChange::Name(name) => {
                if state.focus_matches(source, *node_id)
                    && let Some(focus) = state.focus.as_mut()
                {
                    focus.snapshot.name.clone_from(name);
                }
                Vec::new()
            }
            PropertyChange::Value(value) => {
                reduce_value_changed(state, trace_id, source, *node_id, value.clone())
            }
            PropertyChange::States(new_states) => {
                reduce_states_changed(state, trace_id, source, *node_id, *new_states)
            }
            // `PropertyChange` is `#[non_exhaustive]`.
            _ => Vec::new(),
        },
        // `NormalizedEvent` is `#[non_exhaustive]`.
        _ => Vec::new(),
    }
}

/// Handles a UIA `AutomationNotification` event (NVDA's
/// `event_UIA_notification`): announce the application-supplied display
/// string, if any, and nothing when there is none — a notification with no
/// text has nothing to say. Foreground gating already happened in the
/// shell (only the foreground application's events reach the reducer), so
/// this needs no application check of its own. The processing hint sets the
/// priority: `MostRecent` and `ImportantMostRecent` supersede earlier
/// speech and so interrupt; every other kind queues behind current speech,
/// NVDA's exact split. The notification kind and activity id are not used
/// yet — they exist for later per-kind policy and for correlating an
/// activity's notifications, which M3 does not need.
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
            source: navigator.source,
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
/// reports it. A no-op with nothing focused — including when a navigation
/// fetch is still pending, in which case its eventual completion must not
/// override this explicit return to focus, so this also clears
/// `latest_navigation`. Also reused to re-seed the navigator when a
/// navigation fetch reports `FetchResult::Gone` (the outpost could not
/// re-acquire the navigator's node): the same "fall back to focus and
/// announce it" behavior applies there too.
fn navigator_to_focus(state: &mut SrState, trace_id: TraceId) -> Vec<Effect> {
    let Some(focus) = state.focus.as_ref() else {
        return Vec::new();
    };
    let object = focus.snapshot.clone();
    let source = focus.source;
    let utterance = announce_node(trace_id, SpeechPriority::Interrupt, &object);
    state.navigator = Some(Navigator {
        source,
        object,
        review_offset: 0,
    });
    state.latest_navigation = None;
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
    let source = navigator.source;
    let node_id = navigator.object.id;
    let query_id = state.allocate_query_id();
    state.pending_fetches.insert(
        query_id,
        PendingFetch {
            source,
            node_id,
            reason: FetchReason::Navigate,
            kind,
        },
    );
    state.latest_navigation = Some(query_id);
    vec![Effect::Fetch(Query {
        query_id,
        source,
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
/// container — the event's source application is the focused one, focus
/// sits on a selection container, and the selected node is neither the
/// focused node itself nor the item most recently announced (the focus
/// event's own `selected_child`, or the previous selection event). This is
/// NVDA's generic selection behavior: arrowing through a list whose focus
/// stays on the container speaks each newly selected item, and everything
/// else stays quiet.
fn reduce_selection_changed(
    state: &mut SrState,
    trace_id: TraceId,
    source: Pid,
    node: &NodeSnapshot,
) -> Vec<Effect> {
    let Some(focus) = state.focus.as_mut() else {
        return Vec::new();
    };
    if focus.source != source
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
        segments: node_segments(node),
        source: Some(source_of(node)),
    })]
}

/// Shared handling for `ValueChanged` and `PropertyChanged(Value(..))`: both
/// speak the bare new value, only when the changed node is the focused one.
fn reduce_value_changed(
    state: &mut SrState,
    trace_id: TraceId,
    source: Pid,
    node_id: NodeId,
    value: Option<String>,
) -> Vec<Effect> {
    if !state.focus_matches(source, node_id) {
        return Vec::new();
    }
    let Some(focus) = state.focus.as_mut() else {
        return Vec::new();
    };
    focus.snapshot.value.clone_from(&value);
    focus.last_announced.value.clone_from(&value);
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
    source: Pid,
    node_id: NodeId,
    new_states: StateSet,
) -> Vec<Effect> {
    if !state.focus_matches(source, node_id) {
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
    focus.last_announced.states = new_states;

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

/// Emits a `Fetch` for the currently focused node, recording it as a
/// staleness re-fetch so the eventual `FetchCompleted` is handled correctly.
/// A no-op when nothing is focused: there is nothing to re-read.
fn refetch_focus(state: &mut SrState) -> Vec<Effect> {
    let Some(focus) = state.focus.as_ref() else {
        return Vec::new();
    };
    let source = focus.source;
    let node_id = focus.snapshot.id;
    let query_id = state.allocate_query_id();
    state.pending_fetches.insert(
        query_id,
        PendingFetch {
            source,
            node_id,
            reason: FetchReason::Staleness,
            kind: QueryKind::NodeSnapshot,
        },
    );
    vec![Effect::Fetch(Query {
        query_id,
        source,
        node_id,
        kind: QueryKind::NodeSnapshot,
    })]
}

fn reduce_fetch_completed(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: verbatim_model::QueryId,
    result: &FetchResult,
) -> Vec<Effect> {
    let Some(pending) = state.pending_fetches.remove(&query_id) else {
        return Vec::new();
    };
    match pending.reason {
        FetchReason::Staleness => reduce_staleness_completed(state, trace_id, &pending, result),
        FetchReason::Navigate => {
            reduce_navigate_completed(state, trace_id, &pending, query_id, result)
        }
    }
}

/// Completes an object-navigation fetch.
///
/// Applied if and only if `query_id` is still [`SrState::latest_navigation`]
/// — the most recently issued navigation command, tracked independently of
/// the navigator's identity. This is what a focus event arriving between
/// the command and its completion used to break: focus snaps the navigator
/// (review follows focus) but no longer touches `latest_navigation`, so the
/// user's own pending navigation still lands. A second navigation issued
/// before the first completes replaces `latest_navigation`, so the first's
/// late completion is dropped as stale; `ToFocus` clears it outright, so a
/// late completion cannot override the user's explicit return to focus.
///
/// On `FetchResult::Node`, moves the navigator to the returned neighbor and
/// announces it. On `FetchResult::NoNeighbor`, leaves the navigator put and
/// speaks the direction's edge message — NVDA's wording: "No next", "No
/// previous", "No containing object", "No objects inside". An earlier
/// revision stayed silent here (with an M11 earcon planned on top); live
/// testing found silence indistinguishable from a broken command, exactly
/// as NVDA's spoken messages predict, so the messages are the behavior now
/// and M11's earcon becomes an addition rather than the only feedback.
/// On `FetchResult::Gone` — the navigator's node could no longer be
/// re-acquired, distinct from a genuine tree edge — re-seeds the navigator
/// from the current focus and announces it (via [`navigator_to_focus`])
/// rather than staying silent, so a dead navigator object never presents as
/// the command having done nothing; if nothing is focused either, that stays
/// silent too.
fn reduce_navigate_completed(
    state: &mut SrState,
    trace_id: TraceId,
    pending: &PendingFetch,
    query_id: verbatim_model::QueryId,
    result: &FetchResult,
) -> Vec<Effect> {
    if state.latest_navigation != Some(query_id) {
        // Superseded by a newer navigation, or cleared by `ToFocus`: this
        // completion no longer describes user intent worth acting on.
        return Vec::new();
    }
    match result {
        FetchResult::Node(snapshot) => {
            state.latest_navigation = None;
            let utterance = announce_node(trace_id, SpeechPriority::Interrupt, snapshot);
            state.navigator = Some(Navigator {
                source: pending.source,
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
            let Some(message) = edge_message_of(pending.kind) else {
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
/// neighbor in its direction — NVDA's messages, one per command. `None`
/// for kinds that are not navigations (a plain re-read has no edge).
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

/// Completes a staleness re-fetch of the focused node (the original M1
/// path): announce only a real change against what was last announced, and
/// drop a result whose focus has moved on.
fn reduce_staleness_completed(
    state: &mut SrState,
    trace_id: TraceId,
    pending: &PendingFetch,
    result: &FetchResult,
) -> Vec<Effect> {
    if !state.focus_matches(pending.source, pending.node_id) {
        // Focus moved on while the fetch was in flight; the result no
        // longer describes anything the reducer should announce.
        return Vec::new();
    }

    match result {
        FetchResult::Gone => {
            state.focus = None;
            Vec::new()
        }
        FetchResult::Node(snapshot) => {
            let Some(focus) = state.focus.as_ref() else {
                return Vec::new();
            };
            let changed = snapshot.name != focus.last_announced.name
                || snapshot.value != focus.last_announced.value
                || snapshot.states != focus.last_announced.states;

            if changed {
                let utterance = announce_node(trace_id, SpeechPriority::Interrupt, snapshot);
                // A re-fetch refreshes the node, not its ancestry or its
                // selection history; what the focus event carried stays
                // authoritative.
                let ancestors = focus.ancestors.clone();
                let last_selection = focus.last_selection;
                state.focus = Some(FocusContext {
                    source: pending.source,
                    snapshot: snapshot.clone(),
                    last_announced: snapshot.clone(),
                    ancestors,
                    last_selection,
                });
                vec![Effect::Speak(utterance)]
            } else {
                if let Some(focus) = state.focus.as_mut() {
                    focus.snapshot = snapshot.clone();
                }
                Vec::new()
            }
        }
        // `FetchResult` is `#[non_exhaustive]`.
        _ => Vec::new(),
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
/// [`is_presentable_container`]) that were not already in the previous
/// focus's ancestry — NVDA's focus-ancestry behavior, where tabbing within
/// one dialog stays quiet about the dialog but entering it announces it.
/// A focus change from a different application treats the whole chain as
/// newly entered.
fn entered_containers<'a>(
    previous: Option<&FocusContext>,
    source: Pid,
    ancestors: &'a [NodeSnapshot],
) -> Vec<&'a NodeSnapshot> {
    ancestors
        .iter()
        .filter(|ancestor| is_presentable_container(ancestor))
        .filter(|ancestor| match previous {
            Some(prev) if prev.source == source => {
                !prev.ancestors.iter().any(|old| old.id == ancestor.id)
            }
            _ => true,
        })
        .collect()
}

/// Whether a focus ancestor is worth announcing when first entered: dialogs
/// always, groupings and property pages only when they carry a name (a
/// nameless group adds nothing). Top-level windows are deliberately never
/// announced here — the foreground-change announcement (the outpost's
/// `AnnounceFocus` window step) owns the window, and repeating it on every
/// cross-application focus change would double-speak every switch.
fn is_presentable_container(node: &NodeSnapshot) -> bool {
    match node.role {
        Role::Dialog => true,
        Role::Group | Role::PropertyPage => node.name.as_ref().is_some_and(|name| !name.is_empty()),
        _ => false,
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

/// The full announcement for a node, in NVDA's property order: name, role,
/// value, states, description, keyboard shortcut, position in set, level —
/// each as its semantic span kind, never anonymous text (decision D12).
/// Detail spans simply do not appear when the backend reported nothing.
fn node_segments(node: &NodeSnapshot) -> Vec<UtteranceSegment> {
    let mut segments = Vec::new();
    if let Some(name) = &node.name {
        segments.push(UtteranceSegment::label(name.clone()));
    }
    segments.push(UtteranceSegment::new(SegmentContent::Role(node.role)));
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
/// prefix) — the staleness re-fetch path's announcement.
fn announce_node(trace_id: TraceId, priority: SpeechPriority, node: &NodeSnapshot) -> Utterance {
    Utterance {
        trace_id,
        priority,
        segments: node_segments(node),
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
