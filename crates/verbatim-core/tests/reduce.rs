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
        event: NormalizedEvent::FocusChanged {
            node: snapshot,
            ancestors: Vec::new(),
            selected_child: None,
        },
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
fn unpressed_toggle_button_announces_negated_pressed() {
    let state = SrState::new();
    let toggle_button = node(900, Role::ToggleButton, Some("Bold"), None, StateSet::new());

    let (_, effects) = reduce(
        &state,
        &focus_event(TraceId::mint(), Pid(1), 1, toggle_button),
    );

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Bold"),
            UtteranceSegment::new(SegmentContent::Role(Role::ToggleButton)),
            UtteranceSegment::new(SegmentContent::NegatedState(State::Pressed)),
        ]
    );
}

#[test]
fn pressed_toggle_button_announces_pressed() {
    let state = SrState::new();
    let toggle_button = node(
        901,
        Role::ToggleButton,
        Some("Bold"),
        None,
        StateSet::new().with(State::Pressed),
    );

    let (_, effects) = reduce(
        &state,
        &focus_event(TraceId::mint(), Pid(1), 1, toggle_button),
    );

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Bold"),
            UtteranceSegment::new(SegmentContent::Role(Role::ToggleButton)),
            UtteranceSegment::new(SegmentContent::State(State::Pressed)),
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
fn focus_related_states_are_never_announced_but_unselected_is() {
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
            // NVDA's rule: a selectable item that is not selected announces
            // exactly that; focused/focusable/offscreen stay silent.
            UtteranceSegment::new(SegmentContent::NegatedState(State::Selected)),
        ]
    );
}

#[test]
fn selected_items_do_not_announce_positive_selected_on_focus() {
    let state = SrState::new();
    let mut states = StateSet::new();
    states.insert(State::Selectable);
    states.insert(State::Selected);
    let item = node(7, Role::ListItem, Some("Row"), None, states);

    let (_, effects) = reduce(&state, &focus_event(TraceId::mint(), Pid(1), 1, item));

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Row"),
            UtteranceSegment::new(SegmentContent::Role(Role::ListItem)),
        ],
        "a focused item being selected is the expected default and stays silent"
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
            event: NormalizedEvent::FocusChanged {
                node: checkbox,
                ancestors: Vec::new(),
                selected_child: None,
            },
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

/// A focus event whose snapshot arrives with an ancestor chain, outermost
/// first — the enriched form outposts emit from M3 on.
fn focus_event_with_ancestors(
    trace_id: TraceId,
    source: Pid,
    version: u64,
    snapshot: NodeSnapshot,
    ancestors: Vec<NodeSnapshot>,
) -> Input {
    Input::Event {
        trace_id,
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(version),
        event: NormalizedEvent::FocusChanged {
            node: snapshot,
            ancestors,
            selected_child: None,
        },
    }
}

#[test]
fn entering_a_dialog_announces_it_before_the_control() {
    let state = SrState::new();
    let source = Pid(1);
    let window = node(
        100,
        Role::Window,
        Some("Settings - App"),
        None,
        StateSet::new(),
    );
    let dialog = node(
        101,
        Role::Dialog,
        Some("Save changes"),
        None,
        StateSet::new(),
    );
    let button = node(102, Role::Button, Some("Save"), None, StateSet::new());

    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, 1, button, vec![window, dialog]),
    );

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            // The dialog introduces itself first; the window is never
            // spoken here (the foreground announcement owns it).
            UtteranceSegment::label("Save changes"),
            UtteranceSegment::new(SegmentContent::Role(Role::Dialog)),
            UtteranceSegment::label("Save"),
            UtteranceSegment::new(SegmentContent::Role(Role::Button)),
        ]
    );
}

#[test]
fn moving_within_the_same_dialog_does_not_reannounce_it() {
    let state = SrState::new();
    let source = Pid(1);
    let dialog = node(
        101,
        Role::Dialog,
        Some("Save changes"),
        None,
        StateSet::new(),
    );
    let save = node(102, Role::Button, Some("Save"), None, StateSet::new());
    let cancel = node(103, Role::Button, Some("Cancel"), None, StateSet::new());

    let (state, _) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, 1, save, vec![dialog.clone()]),
    );
    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, 2, cancel, vec![dialog]),
    );

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Cancel"),
            UtteranceSegment::new(SegmentContent::Role(Role::Button)),
        ]
    );
}

#[test]
fn focus_from_another_application_treats_the_chain_as_new() {
    let state = SrState::new();
    let dialog = node(101, Role::Dialog, Some("Find"), None, StateSet::new());
    let edit_a = node(
        102,
        Role::EditableText,
        Some("Find what"),
        None,
        StateSet::new(),
    );
    let edit_b = node(
        202,
        Role::EditableText,
        Some("Search"),
        None,
        StateSet::new(),
    );
    let dialog_b = node(201, Role::Dialog, Some("Open"), None, StateSet::new());

    let (state, _) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), Pid(1), 1, edit_a, vec![dialog]),
    );
    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), Pid(2), 1, edit_b, vec![dialog_b]),
    );

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments[0],
        UtteranceSegment::label("Open"),
        "a different application's chain is entirely newly entered"
    );
}

#[test]
fn nameless_groups_are_not_announced_but_named_ones_are() {
    let state = SrState::new();
    let source = Pid(1);
    let nameless = node(300, Role::Group, None, None, StateSet::new());
    let named = node(301, Role::Group, Some("Margins"), None, StateSet::new());
    let field = node(302, Role::SpinButton, Some("Top"), None, StateSet::new());

    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, 1, field, vec![nameless, named]),
    );

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Margins"),
            UtteranceSegment::new(SegmentContent::Role(Role::Group)),
            UtteranceSegment::label("Top"),
            UtteranceSegment::new(SegmentContent::Role(Role::SpinButton)),
        ]
    );
}

#[test]
fn details_speak_in_nvda_property_order() {
    let state = SrState::new();
    let mut item = node(
        400,
        Role::ListItem,
        Some("Report.txt"),
        None,
        StateSet::new(),
    );
    item.details = NodeDetails {
        description: Some("Text document".to_owned()),
        keyboard_shortcut: Some("Alt+R".to_owned()),
        position_in_set: Some(2),
        set_size: Some(5),
        level: Some(1),
        rect: None,
    };

    let (_, effects) = reduce(&state, &focus_event(TraceId::mint(), Pid(1), 1, item));

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Report.txt"),
            UtteranceSegment::new(SegmentContent::Role(Role::ListItem)),
            UtteranceSegment::new(SegmentContent::Description("Text document".to_owned())),
            UtteranceSegment::new(SegmentContent::Shortcut("Alt+R".to_owned())),
            UtteranceSegment::new(SegmentContent::Position {
                position: 2,
                set_size: Some(5),
            }),
            UtteranceSegment::new(SegmentContent::Level(1)),
        ]
    );
}

/// A focus event carrying a selection container's selected child.
fn focus_event_with_selection(
    trace_id: TraceId,
    source: Pid,
    version: u64,
    snapshot: NodeSnapshot,
    selected_child: Option<NodeSnapshot>,
) -> Input {
    Input::Event {
        trace_id,
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(version),
        event: NormalizedEvent::FocusChanged {
            node: snapshot,
            ancestors: Vec::new(),
            selected_child,
        },
    }
}

fn selection_event(trace_id: TraceId, source: Pid, version: u64, node: NodeSnapshot) -> Input {
    Input::Event {
        trace_id,
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(version),
        event: NormalizedEvent::SelectionChanged { node },
    }
}

#[test]
fn focusing_a_list_announces_its_selected_item() {
    let state = SrState::new();
    let source = Pid(1);
    let list = node(500, Role::List, Some("Categories"), None, StateSet::new());
    let item = node(501, Role::ListItem, Some("Speech"), None, StateSet::new());

    let (_, effects) = reduce(
        &state,
        &focus_event_with_selection(TraceId::mint(), source, 1, list, Some(item)),
    );

    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Categories"),
            UtteranceSegment::new(SegmentContent::Role(Role::List)),
            UtteranceSegment::label("Speech"),
            UtteranceSegment::new(SegmentContent::Role(Role::ListItem)),
        ]
    );
}

#[test]
fn selection_changes_in_the_focused_list_announce_each_new_item_once() {
    let state = SrState::new();
    let source = Pid(1);
    let list = node(500, Role::List, Some("Categories"), None, StateSet::new());
    let speech = node(501, Role::ListItem, Some("Speech"), None, StateSet::new());
    let keyboard = node(502, Role::ListItem, Some("Keyboard"), None, StateSet::new());

    let (state, _) = reduce(
        &state,
        &focus_event_with_selection(TraceId::mint(), source, 1, list, Some(speech)),
    );

    // Arrowing to another item announces it.
    let (state, effects) = reduce(
        &state,
        &selection_event(TraceId::mint(), source, 2, keyboard.clone()),
    );
    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::label("Keyboard"),
            UtteranceSegment::new(SegmentContent::Role(Role::ListItem)),
        ]
    );

    // A duplicate selection event for the same item stays silent.
    let (_, effects) = reduce(
        &state,
        &selection_event(TraceId::mint(), source, 3, keyboard),
    );
    assert!(effects.is_empty(), "the same selection is not spoken twice");
}

#[test]
fn the_focus_events_own_selected_item_is_not_reannounced_by_a_selection_event() {
    let state = SrState::new();
    let source = Pid(1);
    let list = node(500, Role::List, Some("Categories"), None, StateSet::new());
    let speech = node(501, Role::ListItem, Some("Speech"), None, StateSet::new());

    let (state, _) = reduce(
        &state,
        &focus_event_with_selection(TraceId::mint(), source, 1, list, Some(speech.clone())),
    );

    // Platforms often raise a selection event right after focus lands; the
    // focus announcement already spoke this item.
    let (_, effects) = reduce(&state, &selection_event(TraceId::mint(), source, 2, speech));
    assert!(effects.is_empty());
}

#[test]
fn selection_changes_outside_a_focused_container_stay_silent() {
    let state = SrState::new();
    let button = node(600, Role::Button, Some("OK"), None, StateSet::new());
    let item = node(601, Role::ListItem, Some("Row"), None, StateSet::new());

    // Focus on a non-container: selection noise elsewhere is not announced.
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), Pid(1), 1, button));
    let (state, effects) = reduce(
        &state,
        &selection_event(TraceId::mint(), Pid(1), 2, item.clone()),
    );
    assert!(effects.is_empty(), "focus is not on a selection container");

    // A selection event from a different application is not announced
    // either.
    let list = node(602, Role::List, Some("Files"), None, StateSet::new());
    let (state, _) = reduce(
        &state,
        &focus_event_with_selection(TraceId::mint(), Pid(1), 3, list, None),
    );
    let (_, effects) = reduce(&state, &selection_event(TraceId::mint(), Pid(2), 1, item));
    assert!(effects.is_empty(), "another application's selection");
}

fn notification_event(
    trace_id: TraceId,
    source: Pid,
    version: u64,
    processing: verbatim_model::NotificationProcessing,
    display: Option<&str>,
) -> Input {
    Input::Event {
        trace_id,
        source,
        backend: Backend::Uia,
        version: SnapshotVersion(version),
        event: NormalizedEvent::Notification {
            node_id: NodeId::new(1),
            notification: verbatim_model::Notification {
                kind: verbatim_model::NotificationKind::Other,
                processing,
                display_string: display.map(str::to_owned),
                activity_id: None,
            },
        },
    }
}

#[test]
fn notification_with_text_is_announced_and_priority_follows_processing() {
    use verbatim_model::NotificationProcessing;

    // MostRecent supersedes: Interrupt.
    let (_, effects) = reduce(
        &SrState::new(),
        &notification_event(
            TraceId::mint(),
            Pid(1),
            1,
            NotificationProcessing::MostRecent,
            Some("Snap layout available"),
        ),
    );
    let utterances = speak_effects(&effects);
    assert_eq!(utterances[0].priority, SpeechPriority::Interrupt);
    assert_eq!(
        utterances[0].segments,
        vec![UtteranceSegment::text("Snap layout available")]
    );

    // All: queued behind current speech.
    let (_, effects) = reduce(
        &SrState::new(),
        &notification_event(
            TraceId::mint(),
            Pid(1),
            2,
            NotificationProcessing::All,
            Some("Download complete"),
        ),
    );
    let utterances = speak_effects(&effects);
    assert_eq!(utterances[0].priority, SpeechPriority::Queued);
}

#[test]
fn notification_without_text_is_silent() {
    use verbatim_model::NotificationProcessing;

    let (_, effects) = reduce(
        &SrState::new(),
        &notification_event(
            TraceId::mint(),
            Pid(1),
            1,
            NotificationProcessing::All,
            None,
        ),
    );
    assert!(
        effects.is_empty(),
        "a notification with no display text says nothing"
    );
}

#[test]
fn identical_back_to_back_focus_is_suppressed() {
    let source = Pid(1);
    let button = node(700, Role::Button, Some("OK"), None, StateSet::new());

    let (state, effects) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, 1, button.clone()),
    );
    assert_eq!(effects.len(), 1, "first focus is announced");

    // The exact same node focusing again from the same app: suppressed.
    let (_, effects) = reduce(&state, &focus_event(TraceId::mint(), source, 2, button));
    assert!(
        effects.is_empty(),
        "a redundant identical focus event is not re-announced"
    );
}

#[test]
fn returning_to_a_window_after_visiting_another_is_announced() {
    let a = node(700, Role::Button, Some("OK"), None, StateSet::new());
    let b = node(800, Role::Button, Some("Cancel"), None, StateSet::new());

    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), Pid(1), 1, a.clone()),
    );
    // Visit another control (different app), then come back to the first.
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), Pid(2), 1, b));
    let (_, effects) = reduce(&state, &focus_event(TraceId::mint(), Pid(1), 2, a));
    assert_eq!(
        effects.len(),
        1,
        "focus that differs from the last announced one is announced, even if seen earlier"
    );
}

// ---- Object navigation and review cursor (M3 reducer item 4) ----

use verbatim_model::ReviewCommand;

fn command(trace_id: TraceId, cmd: ReviewCommand, repeat: u8) -> Input {
    Input::Command {
        trace_id,
        command: cmd,
        repeat,
    }
}

/// Focus a node so the navigator is seeded, returning the resulting state.
fn focused(source: Pid, snapshot: NodeSnapshot) -> SrState {
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, 1, snapshot),
    );
    state
}

#[test]
fn report_object_announces_spells_then_copies() {
    let source = Pid(1);
    let edit = node(
        10,
        Role::EditableText,
        Some("Name"),
        Some("Ann"),
        StateSet::new(),
    );
    let state = focused(source, edit);

    // First press: full announcement.
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    let utterances = speak_effects(&effects);
    assert_eq!(utterances[0].segments[0], UtteranceSegment::label("Name"));

    // Second press: spell the review text (the value "Ann").
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 1),
    );
    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![
            UtteranceSegment::text("A"),
            UtteranceSegment::text("n"),
            UtteranceSegment::text("n"),
        ]
    );

    // Third press: copy name and value to the clipboard.
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 2),
    );
    assert_eq!(effects.len(), 1);
    match &effects[0] {
        Effect::CopyToClipboard(text) => assert_eq!(text, "Name Ann"),
        other => panic!("expected CopyToClipboard, got {other:?}"),
    }
}

#[test]
fn navigate_to_parent_fetches_then_moves_and_announces() {
    let source = Pid(1);
    let button = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);

    // The command emits a navigation fetch.
    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = match &effects[0] {
        Effect::Fetch(query) => *query,
        other => panic!("expected Fetch, got {other:?}"),
    };
    assert_eq!(query.kind, QueryKind::Parent);
    assert_eq!(query.node_id, NodeId::new(10));

    // The completion moves the navigator to the parent and announces it.
    let parent = node(11, Role::Group, Some("Buttons"), None, StateSet::new());
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        result: FetchResult::Node(parent),
    };
    let (_, effects) = reduce(&state, &completion);
    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments[0],
        UtteranceSegment::label("Buttons")
    );
}

#[test]
fn navigate_at_a_tree_edge_speaks_the_edge_message_and_stays_put() {
    let source = Pid(1);
    let root = node(10, Role::Window, Some("App"), None, StateSet::new());
    let state = focused(source, root);

    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = match &effects[0] {
        Effect::Fetch(query) => *query,
        other => panic!("expected Fetch, got {other:?}"),
    };
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        result: FetchResult::NoNeighbor,
    };
    let (after, effects) = reduce(&state, &completion);
    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments,
        vec![UtteranceSegment::new(SegmentContent::Message(
            verbatim_model::Message::NoContainingObject
        ))],
        "a parent edge speaks NVDA's no-containing-object message"
    );
    // The navigator stays put: reporting the object re-announces the root.
    let (_, effects) = reduce(
        &after,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    let utterances = speak_effects(&effects);
    assert_eq!(utterances[0].segments[0], UtteranceSegment::label("App"));
}

#[test]
fn every_navigation_direction_speaks_its_own_edge_message() {
    use verbatim_model::Message;
    let cases = [
        (ReviewCommand::Parent, Message::NoContainingObject),
        (ReviewCommand::NextSibling, Message::NoNextObject),
        (ReviewCommand::PreviousSibling, Message::NoPreviousObject),
        (ReviewCommand::FirstChild, Message::NoObjectsInside),
    ];
    for (command_kind, expected) in cases {
        let source = Pid(1);
        let root = node(10, Role::Window, Some("App"), None, StateSet::new());
        let state = focused(source, root);
        let (state, effects) = reduce(&state, &command(TraceId::mint(), command_kind, 0));
        let query = match &effects[0] {
            Effect::Fetch(query) => *query,
            other => panic!("expected Fetch, got {other:?}"),
        };
        let completion = Input::FetchCompleted {
            trace_id: TraceId::mint(),
            query_id: query.query_id,
            result: FetchResult::NoNeighbor,
        };
        let (_, effects) = reduce(&state, &completion);
        let utterances = speak_effects(&effects);
        assert_eq!(
            utterances[0].segments,
            vec![UtteranceSegment::new(SegmentContent::Message(expected))],
            "edge message for {command_kind:?}"
        );
    }
}

#[test]
fn navigate_completion_after_an_intervening_focus_event_still_applies() {
    let source = Pid(1);
    let button = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);

    // Issue the navigation command; its completion is still in flight.
    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = match &effects[0] {
        Effect::Fetch(query) => *query,
        other => panic!("expected Fetch, got {other:?}"),
    };

    // A focus event for a different node in the same application arrives
    // before the completion does. Review follows focus, so the navigator
    // snaps to it, but this must not discard the user's still-pending
    // navigation.
    let elsewhere = node(20, Role::Button, Some("Cancel"), None, StateSet::new());
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), source, 2, elsewhere));

    let parent = node(11, Role::Group, Some("Buttons"), None, StateSet::new());
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        result: FetchResult::Node(parent),
    };
    let (_, effects) = reduce(&state, &completion);
    let utterances = speak_effects(&effects);
    assert_eq!(
        utterances[0].segments[0],
        UtteranceSegment::label("Buttons"),
        "a navigation completion must still land after an intervening focus event"
    );
}

#[test]
fn a_second_navigation_supersedes_the_first_pending_one() {
    let source = Pid(1);
    let button = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);

    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let first_query = match &effects[0] {
        Effect::Fetch(query) => *query,
        other => panic!("expected Fetch, got {other:?}"),
    };

    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::NextSibling, 0),
    );
    let second_query = match &effects[0] {
        Effect::Fetch(query) => *query,
        other => panic!("expected Fetch, got {other:?}"),
    };

    // The first (now stale) completion is dropped.
    let stale_parent = node(11, Role::Group, Some("Buttons"), None, StateSet::new());
    let stale_completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: first_query.query_id,
        result: FetchResult::Node(stale_parent),
    };
    let (state, effects) = reduce(&state, &stale_completion);
    assert!(
        effects.is_empty(),
        "a completion for a superseded navigation is dropped"
    );

    // The second completion applies.
    let sibling = node(12, Role::Button, Some("Cancel"), None, StateSet::new());
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: second_query.query_id,
        result: FetchResult::Node(sibling),
    };
    let (_, effects) = reduce(&state, &completion);
    let utterances = speak_effects(&effects);
    assert_eq!(utterances[0].segments[0], UtteranceSegment::label("Cancel"));
}

#[test]
fn to_focus_after_a_navigation_drops_its_late_completion() {
    let source = Pid(1);
    let button = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);

    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = match &effects[0] {
        Effect::Fetch(query) => *query,
        other => panic!("expected Fetch, got {other:?}"),
    };

    // The user explicitly returns to focus before the navigation's
    // completion arrives; that explicit intent must win.
    let (state, _) = reduce(&state, &command(TraceId::mint(), ReviewCommand::ToFocus, 0));

    let parent = node(11, Role::Group, Some("Buttons"), None, StateSet::new());
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        result: FetchResult::Node(parent),
    };
    let (_, effects) = reduce(&state, &completion);
    assert!(
        effects.is_empty(),
        "a navigation completion after an explicit ToFocus is dropped"
    );
}

#[test]
fn navigate_completion_gone_reseeds_the_navigator_to_focus() {
    let source = Pid(1);
    let button = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);

    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = match &effects[0] {
        Effect::Fetch(query) => *query,
        other => panic!("expected Fetch, got {other:?}"),
    };

    // The outpost could not re-acquire the navigator's node: distinct from
    // a tree edge, so this must not stay silent. It falls back to
    // announcing whatever is currently focused.
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        result: FetchResult::Gone,
    };
    let (_, effects) = reduce(&state, &completion);
    let utterances = speak_effects(&effects);
    assert_eq!(utterances[0].segments[0], UtteranceSegment::label("OK"));
}

#[test]
fn activate_emits_activate_for_the_navigator_object() {
    let source = Pid(7);
    let button = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);

    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::Activate, 0),
    );
    match &effects[0] {
        Effect::Activate { source: s, node_id } => {
            assert_eq!(*s, source);
            assert_eq!(*node_id, NodeId::new(10));
        }
        other => panic!("expected Activate, got {other:?}"),
    }
}

#[test]
fn review_cursor_walks_lines_words_and_characters() {
    let source = Pid(1);
    let edit = node(
        10,
        Role::EditableText,
        Some("Body"),
        Some("first line\nsecond line"),
        StateSet::new(),
    );
    let state = focused(source, edit);

    // Current line at the start.
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewCurrentLine, 0),
    );
    assert_eq!(
        speak_effects(&effects)[0].segments,
        vec![UtteranceSegment::text("first line")]
    );

    // Next line.
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewNextLine, 0),
    );
    assert_eq!(
        speak_effects(&effects)[0].segments,
        vec![UtteranceSegment::text("second line")]
    );

    // Next line at the bottom: stays put, re-reads.
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewNextLine, 0),
    );
    assert_eq!(
        speak_effects(&effects)[0].segments,
        vec![UtteranceSegment::text("second line")]
    );

    // Top, then first word, then next word.
    let (state, _) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewTop, 0),
    );
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewCurrentWord, 0),
    );
    assert_eq!(
        speak_effects(&effects)[0].segments,
        vec![UtteranceSegment::text("first")]
    );
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewNextWord, 0),
    );
    assert_eq!(
        speak_effects(&effects)[0].segments,
        vec![UtteranceSegment::text("line")]
    );

    // First character of the current position ("line" -> 'l').
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(
        speak_effects(&effects)[0].segments,
        vec![UtteranceSegment::text("l")]
    );
}

#[test]
fn navigator_follows_focus_and_returns_to_focus() {
    let source = Pid(1);
    let first = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, first);

    // Move the navigator to the parent.
    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = match &effects[0] {
        Effect::Fetch(q) => *q,
        other => panic!("expected Fetch, got {other:?}"),
    };
    let parent = node(11, Role::Group, Some("Group"), None, StateSet::new());
    let (state, _) = reduce(
        &state,
        &Input::FetchCompleted {
            trace_id: TraceId::mint(),
            query_id: query.query_id,
            result: FetchResult::Node(parent),
        },
    );

    // A new focus event snaps the navigator back to focus.
    let second = node(
        20,
        Role::EditableText,
        Some("Field"),
        Some("x"),
        StateSet::new(),
    );
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), source, 2, second));
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    assert_eq!(
        speak_effects(&effects)[0].segments[0],
        UtteranceSegment::label("Field"),
        "the navigator followed focus to the new control"
    );

    // Explicit "to focus" also reports the focused control after wandering.
    let (state, _) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let (_, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::ToFocus, 0));
    assert_eq!(
        speak_effects(&effects)[0].segments[0],
        UtteranceSegment::label("Field"),
        "to-focus snaps the navigator back regardless of where it wandered"
    );
}

#[test]
fn commands_with_no_navigator_yet_do_nothing() {
    let state = SrState::new();
    let (_, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    assert!(effects.is_empty());
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    assert!(effects.is_empty());
}
