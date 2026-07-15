//! The reducer (architecture section 2): pure state-and-effect transitions
//! from normalized accessibility events and fetch completions.
//!
//! Nothing in this module performs I/O, reads a clock, or consults
//! randomness; the only way the outside world enters is through the `Input`
//! passed to [`reduce`]. That purity is what lets the flight recorder turn a
//! captured sequence of inputs into a deterministic regression test (see
//! [`crate::replay`]).

use verbatim_model::{
    Effect, FetchResult, Input, NodeId, NodeSnapshot, NormalizedEvent, Pid, PropertyChange, Query,
    QueryKind, Role, SegmentContent, SnapshotVersion, SpeechPriority, State, StateSet, TraceId,
    Utterance, UtteranceSegment, UtteranceSource,
};

use crate::state::{FetchReason, FocusContext, PendingFetch, SrState};

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
        NormalizedEvent::FocusChanged { node, ancestors } => {
            let mut segments = Vec::new();
            for container in entered_containers(state.focus.as_ref(), source, ancestors) {
                segments.extend(container_segments(container));
            }
            segments.extend(node_segments(node));
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
            });
            vec![Effect::Speak(utterance)]
        }
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
/// (using the same order and exclusions as [`announce_node`]), plus the
/// check-box/radio-button "toggled off" case — losing `Checked` with no
/// `Mixed` present announces `NegatedState(Checked)`, since that transition
/// would otherwise be silent. Ignored for any node other than the focused
/// one; a no-op if the set did not actually change.
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

    for candidate in [
        State::Mixed,
        State::Pressed,
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
                // A re-fetch refreshes the node, not its ancestry; the
                // chain the focus event carried stays authoritative.
                let ancestors = focus.ancestors.clone();
                state.focus = Some(FocusContext {
                    source: pending.source,
                    snapshot: snapshot.clone(),
                    last_announced: snapshot.clone(),
                    ancestors,
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
/// and radio buttons that carry neither checked nor mixed), mixed, pressed,
/// selected, expanded, collapsed, has-popup, default, read-only, disabled,
/// busy. Focus-related states (focused, focusable, selectable, offscreen)
/// are never announced — they describe capability, not content.
fn state_segments(role: Role, states: StateSet) -> Vec<UtteranceSegment> {
    let mut segments = Vec::new();

    if states.contains(State::Checked) {
        segments.push(UtteranceSegment::new(SegmentContent::State(State::Checked)));
    } else if matches!(role, Role::CheckBox | Role::RadioButton) && !states.contains(State::Mixed) {
        segments.push(UtteranceSegment::new(SegmentContent::NegatedState(
            State::Checked,
        )));
    }

    for state in [
        State::Mixed,
        State::Pressed,
        State::Selected,
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

    segments
}
