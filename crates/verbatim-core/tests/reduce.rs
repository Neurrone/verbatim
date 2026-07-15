//! Scripted-input tests for the M1 reducer: exact effect sequences for the
//! milestone's exit-criteria scenarios, plus flight-recorder replay
//! determinism.

use verbatim_core::{ReducerRecorder, SrState, reduce, replay};
use verbatim_model::{
    Backend, Effect, FetchResult, Input, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, Pid,
    PropertyChange, Query, QueryId, QueryKind, Role, SegmentContent, SnapshotVersion,
    SpeechPriority, State, StateSet, TraceId, Utterance, UtteranceSegment,
};

fn node(
    id: u64,
    role: Role,
    name: Option<&str>,
    value: Option<&str>,
    states: StateSet,
) -> NodeSnapshot {
    NodeSnapshot {
        id: NodeId::new(id),
        backend: Backend::Uia,
        role,
        name: name.map(str::to_string),
        value: value.map(str::to_string),
        states,
        details: NodeDetails::default(),
    }
}

fn focus_event(trace_id: TraceId, source: Pid, version: u64, snapshot: NodeSnapshot) -> Input {
    Input::Event {
        trace_id,
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(version),
        event: NormalizedEvent::FocusChanged { node: snapshot },
    }
}

fn speak_effects(effects: &[Effect]) -> Vec<&Utterance> {
    effects
        .iter()
        .map(|effect| match effect {
            Effect::Speak(utterance) => utterance,
            other => panic!("expected Speak effect, got {other:?}"),
        })
        .collect()
}

#[test]
fn focus_menu_item_with_popup_speaks_name_role_and_submenu() {
    let state = SrState::new();
    let source = Pid(100);
    let trace_id = TraceId::mint();
    let snapshot = node(
        1,
        Role::MenuItem,
        Some("Settings..."),
        None,
        StateSet::new().with(State::HasPopup),
    );

    let (next, effects) = reduce(&state, &focus_event(trace_id, source, 1, snapshot));

    assert_eq!(effects.len(), 1);
    let utterances = speak_effects(&effects);
    assert_eq!(utterances[0].trace_id, trace_id);
    assert_eq!(utterances[0].priority, SpeechPriority::Interrupt);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Settings..."),
            UtteranceSegment::new(SegmentContent::Role(Role::MenuItem)),
            UtteranceSegment::new(SegmentContent::State(State::HasPopup)),
        ]
    );
    assert_eq!(next.focused().map(|(pid, _)| pid), Some(source));
}

#[test]
fn focus_slider_then_drag_speaks_value_only_on_change() {
    let state = SrState::new();
    let source = Pid(200);
    let trace_1 = TraceId::mint();
    let slider = node(2, Role::Slider, Some("Rate"), Some("50"), StateSet::new());

    let (state, effects) = reduce(&state, &focus_event(trace_1, source, 1, slider));
    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Rate"),
            UtteranceSegment::new(SegmentContent::Role(Role::Slider)),
            UtteranceSegment::value("50"),
        ]
    );

    let trace_2 = TraceId::mint();
    let value_changed = Input::Event {
        trace_id: trace_2,
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(2),
        event: NormalizedEvent::ValueChanged {
            node_id: NodeId::new(2),
            value: Some("55".to_string()),
        },
    };
    let (state, effects) = reduce(&state, &value_changed);

    assert_eq!(effects.len(), 1);
    let utterances = speak_effects(&effects);
    assert_eq!(utterances[0].trace_id, trace_2);
    assert_eq!(utterances[0].priority, SpeechPriority::Interrupt);
    assert_eq!(utterances[0].segments, vec![UtteranceSegment::value("55")]);
    assert_eq!(
        state.focused().map(|(_, n)| n.value.clone()),
        Some(Some("55".to_string()))
    );
}

#[test]
fn unchecked_checkbox_announces_negated_checked() {
    let state = SrState::new();
    let checkbox = node(
        3,
        Role::CheckBox,
        Some("Remember me"),
        None,
        StateSet::new(),
    );

    let (_, effects) = reduce(&state, &focus_event(TraceId::mint(), Pid(1), 1, checkbox));

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Remember me"),
            UtteranceSegment::new(SegmentContent::Role(Role::CheckBox)),
            UtteranceSegment::new(SegmentContent::NegatedState(State::Checked)),
        ]
    );
}

#[test]
fn checked_checkbox_announces_checked() {
    let state = SrState::new();
    let checkbox = node(
        4,
        Role::CheckBox,
        Some("Remember me"),
        None,
        StateSet::new().with(State::Checked),
    );

    let (_, effects) = reduce(&state, &focus_event(TraceId::mint(), Pid(1), 1, checkbox));

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Remember me"),
            UtteranceSegment::new(SegmentContent::Role(Role::CheckBox)),
            UtteranceSegment::new(SegmentContent::State(State::Checked)),
        ]
    );
}

#[test]
fn mixed_checkbox_does_not_announce_negated_checked() {
    let state = SrState::new();
    let checkbox = node(
        5,
        Role::CheckBox,
        Some("Some of these"),
        None,
        StateSet::new().with(State::Mixed),
    );

    let (_, effects) = reduce(&state, &focus_event(TraceId::mint(), Pid(1), 1, checkbox));

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Some of these"),
            UtteranceSegment::new(SegmentContent::Role(Role::CheckBox)),
            UtteranceSegment::new(SegmentContent::State(State::Mixed)),
        ]
    );
}

#[test]
fn disabled_button_announces_unavailable_state() {
    let state = SrState::new();
    let button = node(
        6,
        Role::Button,
        Some("OK"),
        None,
        StateSet::new().with(State::Disabled),
    );

    let (_, effects) = reduce(&state, &focus_event(TraceId::mint(), Pid(1), 1, button));

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("OK"),
            UtteranceSegment::new(SegmentContent::Role(Role::Button)),
            UtteranceSegment::new(SegmentContent::State(State::Disabled)),
        ]
    );
}

#[test]
fn focus_related_states_are_never_announced() {
    let state = SrState::new();
    let mut states = StateSet::new();
    states.insert(State::Focused);
    states.insert(State::Focusable);
    states.insert(State::Selectable);
    states.insert(State::Offscreen);
    let item = node(7, Role::ListItem, Some("Row"), None, states);

    let (_, effects) = reduce(&state, &focus_event(TraceId::mint(), Pid(1), 1, item));

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Row"),
            UtteranceSegment::new(SegmentContent::Role(Role::ListItem)),
        ]
    );
}

#[test]
fn value_changed_for_non_focused_node_produces_no_effects() {
    let state = SrState::new();
    let source = Pid(1);
    let focused = node(
        8,
        Role::EditableText,
        Some("Name"),
        Some("a"),
        StateSet::new(),
    );
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), source, 1, focused));

    let other_value_changed = Input::Event {
        trace_id: TraceId::mint(),
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(2),
        event: NormalizedEvent::ValueChanged {
            node_id: NodeId::new(9),
            value: Some("changed".to_string()),
        },
    };
    let (state, effects) = reduce(&state, &other_value_changed);

    assert!(effects.is_empty());
    assert_eq!(
        state.focused().map(|(_, n)| n.value.clone()),
        Some(Some("a".to_string()))
    );
}

#[test]
fn property_changed_name_on_focused_node_updates_silently() {
    let state = SrState::new();
    let source = Pid(1);
    let node_id = NodeId::new(10);
    let focused = NodeSnapshot {
        id: node_id,
        backend: Backend::Uia,
        role: Role::EditableText,
        name: Some("Old name".to_string()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    };
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), source, 1, focused));

    let name_changed = Input::Event {
        trace_id: TraceId::mint(),
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(2),
        event: NormalizedEvent::PropertyChanged {
            node_id,
            change: PropertyChange::Name(Some("New name".to_string())),
        },
    };
    let (state, effects) = reduce(&state, &name_changed);

    assert!(effects.is_empty(), "name changes are not announced in M1");
    assert_eq!(
        state.focused().map(|(_, n)| n.name.clone()),
        Some(Some("New name".to_string()))
    );
}

fn states_changed_input(
    trace_id: TraceId,
    source: Pid,
    node_id: NodeId,
    states: StateSet,
) -> Input {
    Input::Event {
        trace_id,
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(2),
        event: NormalizedEvent::PropertyChanged {
            node_id,
            change: PropertyChange::States(states),
        },
    }
}

#[test]
fn states_changed_checkbox_toggle_on_announces_checked() {
    let source = Pid(1);
    let node_id = NodeId::new(20);
    let checkbox = node(20, Role::CheckBox, Some("Agree"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, 1, checkbox),
    );

    let trace_id = TraceId::mint();
    let toggled_on = states_changed_input(
        trace_id,
        source,
        node_id,
        StateSet::new().with(State::Checked),
    );
    let (state, effects) = reduce(&state, &toggled_on);

    let utterances = speak_effects(&effects);
    assert_eq!(utterances.len(), 1);
    assert_eq!(utterances[0].trace_id, trace_id);
    assert_eq!(utterances[0].priority, SpeechPriority::Interrupt);
    assert_eq!(
        utterances[0].segments,
        vec![UtteranceSegment::new(SegmentContent::State(State::Checked))]
    );
    assert_eq!(
        state.focused().map(|(_, n)| n.states),
        Some(StateSet::new().with(State::Checked))
    );
}

#[test]
fn states_changed_checkbox_toggle_off_announces_negated_checked() {
    let source = Pid(1);
    let node_id = NodeId::new(21);
    let checkbox = node(
        21,
        Role::CheckBox,
        Some("Agree"),
        None,
        StateSet::new().with(State::Checked),
    );
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, 1, checkbox),
    );

    let trace_id = TraceId::mint();
    let toggled_off = states_changed_input(trace_id, source, node_id, StateSet::new());
    let (state, effects) = reduce(&state, &toggled_off);

    let utterances = speak_effects(&effects);
    assert_eq!(utterances.len(), 1);
    assert_eq!(utterances[0].trace_id, trace_id);
    assert_eq!(utterances[0].priority, SpeechPriority::Interrupt);
    assert_eq!(
        utterances[0].segments,
        vec![UtteranceSegment::new(SegmentContent::NegatedState(
            State::Checked
        ))]
    );
    assert_eq!(
        state.focused().map(|(_, n)| n.states),
        Some(StateSet::new())
    );
}

#[test]
fn states_changed_disabled_appearing_announces_unavailable() {
    let source = Pid(1);
    let node_id = NodeId::new(22);
    let button = node(22, Role::Button, Some("Submit"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, 1, button),
    );

    let disabled = states_changed_input(
        TraceId::mint(),
        source,
        node_id,
        StateSet::new().with(State::Disabled),
    );
    let (_, effects) = reduce(&state, &disabled);

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![UtteranceSegment::new(SegmentContent::State(
            State::Disabled
        ))]
    );
}

#[test]
fn states_changed_identical_set_is_silent() {
    let source = Pid(1);
    let node_id = NodeId::new(23);
    let states = StateSet::new().with(State::Checked);
    let checkbox = node(23, Role::CheckBox, Some("Agree"), None, states);
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, 1, checkbox),
    );

    let same = states_changed_input(TraceId::mint(), source, node_id, states);
    let (_, effects) = reduce(&state, &same);

    assert!(effects.is_empty());
}

#[test]
fn states_changed_for_non_focused_node_is_ignored() {
    let source = Pid(1);
    let other_id = NodeId::new(25);
    let focused = node(24, Role::CheckBox, Some("Agree"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, 1, focused),
    );

    let other_changed = states_changed_input(
        TraceId::mint(),
        source,
        other_id,
        StateSet::new().with(State::Checked),
    );
    let (state, effects) = reduce(&state, &other_changed);

    assert!(effects.is_empty());
    assert_eq!(
        state.focused().map(|(_, n)| n.states),
        Some(StateSet::new())
    );
}

/// A focused node plus a state that has already seen version 5 from its
/// source, ready for the out-of-order-delivery scenarios below.
fn focused_static_text() -> (SrState, Pid, NodeId, NodeSnapshot) {
    let source = Pid(1);
    let node_id = NodeId::new(11);
    let snapshot = NodeSnapshot {
        id: node_id,
        backend: Backend::Uia,
        role: Role::StaticText,
        name: Some("Status".to_string()),
        value: Some("Ready".to_string()),
        states: StateSet::new(),
        details: NodeDetails::default(),
    };
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, 5, snapshot.clone()),
    );
    (state, source, node_id, snapshot)
}

/// Sends an event with an older-than-seen version for `node_id` and returns
/// the resulting state and the `QueryId` of the `Fetch` effect it produced.
fn trigger_staleness(
    state: &SrState,
    source: Pid,
    node_id: NodeId,
    older_version: u64,
) -> (SrState, QueryId) {
    let event = Input::Event {
        trace_id: TraceId::mint(),
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(older_version),
        event: NormalizedEvent::ValueChanged {
            node_id,
            value: Some("Value from the reordered event".to_string()),
        },
    };
    let (next, effects) = reduce(state, &event);
    assert_eq!(
        effects.len(),
        1,
        "a stale event must produce exactly one Fetch"
    );
    let query_id = match &effects[0] {
        Effect::Fetch(query) => query.query_id,
        other => panic!("expected Fetch effect, got {other:?}"),
    };
    (next, query_id)
}

#[test]
fn out_of_order_version_triggers_fetch_for_focused_node() {
    let (state, source, node_id, _) = focused_static_text();
    assert_eq!(state.last_seen_version(source), Some(SnapshotVersion(5)));

    let older = Input::Event {
        trace_id: TraceId::mint(),
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(3),
        event: NormalizedEvent::ValueChanged {
            node_id,
            value: Some("Stale value".to_string()),
        },
    };
    let (state, effects) = reduce(&state, &older);

    assert_eq!(effects.len(), 1);
    match &effects[0] {
        Effect::Fetch(Query {
            source: query_source,
            node_id: query_node,
            kind,
            ..
        }) => {
            assert_eq!(*query_source, source);
            assert_eq!(*query_node, node_id);
            assert_eq!(*kind, QueryKind::NodeSnapshot);
        }
        other => panic!("expected Fetch effect, got {other:?}"),
    }
    // The stale event must not overwrite the last-seen version or the
    // stored value with untrustworthy data.
    assert_eq!(state.last_seen_version(source), Some(SnapshotVersion(5)));
    assert_eq!(
        state.focused().map(|(_, n)| n.value.clone()),
        Some(Some("Ready".to_string()))
    );
}

#[test]
fn fetch_completed_with_changed_data_announces_it() {
    let (state, source, node_id, focused) = focused_static_text();
    let (state, query_id) = trigger_staleness(&state, source, node_id, 3);

    let changed = NodeSnapshot {
        value: Some("Busy".to_string()),
        ..focused
    };
    let trace_id = TraceId::mint();
    let completed = Input::FetchCompleted {
        trace_id,
        query_id,
        result: FetchResult::Node(changed),
    };
    let (_, effects) = reduce(&state, &completed);

    assert_eq!(effects.len(), 1);
    let utterances = speak_effects(&effects);
    assert_eq!(utterances[0].trace_id, trace_id);
    assert_eq!(utterances[0].priority, SpeechPriority::Interrupt);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Status"),
            UtteranceSegment::new(SegmentContent::Role(Role::StaticText)),
            UtteranceSegment::value("Busy"),
        ]
    );
}

#[test]
fn fetch_completed_with_unchanged_data_does_not_announce() {
    let (state, source, node_id, focused) = focused_static_text();
    let (state, query_id) = trigger_staleness(&state, source, node_id, 3);

    let completed = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id,
        result: FetchResult::Node(focused),
    };
    let (state, effects) = reduce(&state, &completed);

    assert!(
        effects.is_empty(),
        "unchanged data from a re-fetch must not be announced"
    );
    assert_eq!(state.pending_fetch_count(), 0);
}

#[test]
fn fetch_completed_gone_clears_focus_without_announcing() {
    let (state, source, node_id, _) = focused_static_text();
    let (state, query_id) = trigger_staleness(&state, source, node_id, 3);

    let completed = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id,
        result: FetchResult::Gone,
    };
    let (state, effects) = reduce(&state, &completed);

    assert!(effects.is_empty());
    assert!(state.focused().is_none());
}

#[test]
fn fetch_completed_for_unknown_query_id_is_ignored() {
    let state = SrState::new();
    let completed = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: QueryId(9999),
        result: FetchResult::Gone,
    };
    let (state, effects) = reduce(&state, &completed);
    assert!(effects.is_empty());
    assert!(state.focused().is_none());
}

fn sample_script() -> Vec<Input> {
    let source = Pid(1);
    let node_id = NodeId::new(1);
    let button = node(1, Role::Button, Some("OK"), None, StateSet::new());
    let checkbox = NodeSnapshot {
        id: node_id,
        ..node(1, Role::CheckBox, Some("Agree"), None, StateSet::new())
    };
    vec![
        focus_event(TraceId::mint(), source, 1, button),
        Input::Event {
            trace_id: TraceId::mint(),
            source,
            backend: Backend::Uia,
            version: SnapshotVersion(2),
            event: NormalizedEvent::FocusChanged { node: checkbox },
        },
        Input::Event {
            trace_id: TraceId::mint(),
            source,
            backend: Backend::Uia,
            version: SnapshotVersion(1),
            event: NormalizedEvent::ValueChanged {
                node_id,
                value: Some("x".to_string()),
            },
        },
        Input::Tick,
    ]
}

#[test]
fn replay_is_deterministic() {
    let initial = SrState::new();
    let script = sample_script();

    let first = replay(&initial, &script);
    let second = replay(&initial, &script);

    assert_eq!(first, second);
    assert_eq!(first.len(), script.len());
}

#[test]
fn flight_recorder_dump_replays_to_the_same_effects_as_live_reduction() {
    let mut recorder = ReducerRecorder::new(16);
    let mut state = SrState::new();
    let script = sample_script();

    let mut live_effects = Vec::new();
    for input in &script {
        let (next, effects) = reduce(&state, input);
        recorder.record_input(input.clone(), effects.len());
        live_effects.push(effects);
        state = next;
    }

    let dumped = recorder.dump_inputs();
    assert_eq!(dumped, script);

    let replayed = replay(&SrState::new(), &dumped);
    assert_eq!(replayed, live_effects);
}
