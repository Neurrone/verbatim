//! Scripted-input tests for the M1 reducer: exact effect sequences for the
//! milestone's exit-criteria scenarios, plus flight-recorder replay
//! determinism.

use verbatim_core::{ReducerRecorder, SrState, replay};
use verbatim_model::{
    Backend, Earcon, EarlierElement, Effect, FetchResult, FocusGroup, Input, NodeDetails, NodeId,
    NodeSnapshot, NormalizedEvent, OutpostId, Phrase, Pid, PropertyChange, QueryId, QueryKind,
    Role, SegmentContent, SpeechPriority, State, StateSet, TraceId, Utterance, UtteranceSegment,
    WindowFacts, WindowHandle,
};

/// The outpost standing for application `source` in these tests: one per
/// pid, as in production.
fn outpost_of(source: Pid) -> OutpostId {
    OutpostId(u64::from(source.0))
}

/// Runs the reducer the way Core feeds it: an event's node ids are stamped
/// with the outpost of the pipe it arrived on, here the outpost of its pid.
fn reduce(state: &SrState, input: &Input) -> (SrState, Vec<Effect>) {
    let mut input = input.clone();
    if let Input::Event { source, event, .. } = &mut input {
        event.assign_outpost(outpost_of(*source));
    }
    let mut next = state.clone();
    let effects = verbatim_core::reduce(&mut next, &input);
    (next, effects)
}

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

fn focus_event(trace_id: TraceId, source: Pid, snapshot: NodeSnapshot) -> Input {
    Input::Event {
        observed_at_us: 0,
        trace_id,
        source,
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::FocusChanged {
            foreground: false,
            node: snapshot,
            ancestors: Vec::new(),
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
        },
    }
}

/// Window facts for a plain top-level window: its own top-level window and
/// root owner, not topmost.
fn window(handle: u64) -> WindowFacts {
    WindowFacts {
        top_level: WindowHandle(handle),
        root_owner: WindowHandle(handle),
        topmost: false,
        under_active_window: None,
        in_foreground: false,
        class: String::new(),
        application: String::new(),
        alt_held: false,
    }
}

/// The facts of window `handle` as its outpost reads them while it is in the
/// system's foreground window.
fn foreground_window(handle: u64) -> WindowFacts {
    WindowFacts {
        in_foreground: true,
        ..window(handle)
    }
}

/// An event from `source` concerning the window described by `facts`.
fn event_in(source: Pid, facts: Option<WindowFacts>, event: NormalizedEvent) -> Input {
    Input::Event {
        observed_at_us: 0,
        trace_id: TraceId::mint(),
        source,
        backend: Backend::Uia,
        window: facts,
        event,
    }
}

/// A focus change in the window described by `facts`.
fn focus_in(
    source: Pid,
    facts: WindowFacts,
    snapshot: NodeSnapshot,
    ancestors: Vec<NodeSnapshot>,
) -> Input {
    event_in(
        source,
        Some(facts),
        NormalizedEvent::FocusChanged {
            node: snapshot,
            foreground: false,
            ancestors,
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
        },
    )
}

/// [`focus_in`] for a UIA focus whose reader read `earlier` of the element
/// it held before under the focus's runtime id.
fn focus_in_after(
    source: Pid,
    facts: WindowFacts,
    snapshot: NodeSnapshot,
    ancestors: Vec<NodeSnapshot>,
    earlier: EarlierElement,
) -> Input {
    event_in(
        source,
        Some(facts),
        NormalizedEvent::FocusChanged {
            node: snapshot,
            foreground: false,
            ancestors,
            selected_child: None,
            text: None,
            earlier: Some(earlier),
            group: None,
        },
    )
}

/// A foreground change: the window `snapshot` of application `source`
/// became the system's foreground window. The window's node carries its
/// handle, as every reader's node for a window does, unless the test gave
/// it one.
fn foreground_in(source: Pid, facts: WindowFacts, mut snapshot: NodeSnapshot) -> Input {
    snapshot.details.window.get_or_insert(facts.top_level);
    event_in(
        source,
        Some(facts),
        NormalizedEvent::FocusChanged {
            node: snapshot,
            foreground: true,
            ancestors: Vec::new(),
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
        },
    )
}

/// A focus read on request is ordered by the time its read began: a late
/// event from another outpost observed before that read is stale and
/// dropped, as NVDA's queue would have handled it first.
#[test]
fn a_focus_read_on_request_is_ordered_by_its_read_time() {
    let edit = node(
        7001,
        Role::EditableText,
        Some("Name"),
        None,
        StateSet::new(),
    );
    let mut read = focus_event(TraceId::mint(), Pid(7), edit);
    if let Input::Event { observed_at_us, .. } = &mut read {
        *observed_at_us = 2_000;
    }
    let (state, _) = reduce_from(&SrState::new(), &read, OutpostId(7));

    let button = node(8001, Role::Button, Some("OK"), None, StateSet::new());
    let mut late = focus_event(TraceId::mint(), Pid(8), button);
    if let Input::Event { observed_at_us, .. } = &mut late {
        *observed_at_us = 1_500;
    }
    let (state, effects) = reduce_from(&state, &late, OutpostId(8));
    assert!(effects.is_empty(), "observed before the read, so stale");
    assert_eq!(
        state.focused().map(|(_, node)| node.id.number()),
        Some(7001)
    );
}

/// Switches to application `source`: a foreground change to a window of its
/// own, so attention moves there as it does when the user switches
/// applications.
fn switch_to(state: &SrState, source: Pid) -> SrState {
    let handle = u64::from(source.0) * 1000;
    let window_node = node(handle, Role::Window, Some("App"), None, StateSet::new());
    reduce(state, &foreground_in(source, window(handle), window_node)).0
}

/// One effect as these tests compare it. Every effect the reducer returns
/// becomes exactly one of these, so comparing the whole list checks every
/// effect an input produces, in order: an utterance by its priority and
/// segments, where the focus now is by its nodes, and every other effect
/// whole. The windows the speech manager's keys carry are asserted by the
/// tests of the identity rule.
#[derive(Debug, Clone, PartialEq)]
enum Heard {
    /// `Effect::DropExpiredSpeech`, with where the focus now is, each key
    /// by its node alone.
    Expire(FocusNow),
    /// `Effect::StopSpeech`.
    Stop,
    /// `Effect::Speak`: the utterance's priority and segments.
    Say(SpeechPriority, Vec<UtteranceSegment>),
    /// Any other effect, compared whole.
    Other(Effect),
}

/// Every effect in `effects`, in order, as [`Heard`].
fn heard(effects: &[Effect]) -> Vec<Heard> {
    effects
        .iter()
        .map(|effect| match effect {
            Effect::DropExpiredSpeech(now) => {
                let node = |key: &ObjectKey| ObjectKey::of_id(key.id);
                Heard::Expire(FocusNow {
                    focus: node(&now.focus),
                    ancestors: now.ancestors.iter().map(node).collect(),
                    foreground: now.foreground.as_ref().map(node),
                    menu_item_of_focus: now.menu_item_of_focus,
                })
            }
            Effect::StopSpeech => Heard::Stop,
            Effect::Speak(utterance) => Heard::Say(utterance.priority, utterance.segments.clone()),
            other => Heard::Other(other.clone()),
        })
        .collect()
}

/// A queued utterance of `segments`.
fn queued(segments: Vec<UtteranceSegment>) -> Heard {
    Heard::Say(SpeechPriority::Queued, segments)
}

/// Where the focus is, as a focus change tells the speech manager: node
/// `focus` and its `ancestors` (outermost first) in `outpost`, and the
/// foreground window, each a node number in the outpost named.
fn focus_now(
    outpost: OutpostId,
    focus: u64,
    ancestors: &[u64],
    foreground: Option<(OutpostId, u64)>,
) -> FocusNow {
    let key = |outpost, node| ObjectKey::of_id(NodeId::in_outpost(outpost, node));
    FocusNow {
        focus: key(outpost, focus),
        ancestors: ancestors
            .iter()
            .map(|&ancestor| key(outpost, ancestor))
            .collect(),
        foreground: foreground.map(|(outpost, node)| key(outpost, node)),
        menu_item_of_focus: None,
    }
}

/// Where the focus is when it moves to node `focus` of application 1 with
/// no ancestors and no foreground window known: the plain focus change
/// most of these tests make.
fn plain_focus(focus: u64) -> FocusNow {
    focus_now(OutpostId(1), focus, &[], None)
}

/// The effects of the focus moving to `now`, announced by the utterances
/// `said`, each queued, with no speech cut off.
fn focus_heard(now: FocusNow, said: Vec<Vec<UtteranceSegment>>) -> Vec<Heard> {
    let mut expected = vec![Heard::Expire(now)];
    expected.extend(said.into_iter().map(queued));
    expected
}

/// The effects of the focus moving to `now` in a focus line that shares
/// nothing with the previous one below the desktop: speech is cut off, then
/// the utterances `said` are queued.
fn cut_focus_heard(now: FocusNow, said: Vec<Vec<UtteranceSegment>>) -> Vec<Heard> {
    let mut expected = vec![Heard::Expire(now), Heard::Stop];
    expected.extend(said.into_iter().map(queued));
    expected
}

/// The utterances among `effects`, after asserting that `effects` holds
/// nothing else.
fn only_speech(effects: &[Effect]) -> Vec<&Utterance> {
    effects
        .iter()
        .map(|effect| match effect {
            Effect::Speak(utterance) => utterance,
            other => panic!("expected only Speak effects, got {other:?} in {effects:?}"),
        })
        .collect()
}

#[test]
fn focus_menu_item_with_popup_speaks_name_and_submenu() {
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

    let (next, effects) = reduce(&state, &focus_event(trace_id, source, snapshot));

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(100), 1, &[], None),
            vec![vec![
                UtteranceSegment::label("Settings..."),
                UtteranceSegment::new(SegmentContent::State(State::HasPopup)),
            ]]
        )
    );
    let Effect::Speak(utterance) = &effects[1] else {
        panic!("the announcement follows the drop of expired speech");
    };
    assert_eq!(utterance.trace_id, trace_id);
    assert_eq!(next.focused().map(|(pid, _)| pid), Some(source));
}

#[test]
fn focus_slider_then_drag_speaks_value_only_on_change() {
    let state = SrState::new();
    let source = Pid(200);
    let trace_1 = TraceId::mint();
    let slider = node(2, Role::Slider, Some("Rate"), Some("50"), StateSet::new());

    let (state, effects) = reduce(&state, &focus_event(trace_1, source, slider));
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(200), 2, &[], None),
            vec![vec![
                UtteranceSegment::label("Rate"),
                UtteranceSegment::new(SegmentContent::Role(Role::Slider)),
                UtteranceSegment::value("50"),
            ]]
        )
    );

    let trace_2 = TraceId::mint();
    let value_changed = Input::Event {
        observed_at_us: 0,
        trace_id: trace_2,
        source,
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::ValueChanged {
            node_id: NodeId::new(2),
            value: Some("55".to_string()),
        },
    };
    let (state, effects) = reduce(&state, &value_changed);

    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::value("55")])]
    );
    assert_eq!(only_speech(&effects)[0].trace_id, trace_2);
    assert_eq!(
        state.focused().map(|(_, n)| n.value.clone()),
        Some(Some("55".to_string()))
    );
}

#[test]
fn unchecked_checkbox_announces_negated_checked() {
    let checkbox = node(
        3,
        Role::CheckBox,
        Some("Remember me"),
        None,
        StateSet::new(),
    );

    assert_eq!(
        focus_segments(checkbox),
        vec![
            UtteranceSegment::label("Remember me"),
            UtteranceSegment::new(SegmentContent::Role(Role::CheckBox)),
            UtteranceSegment::new(SegmentContent::NegatedState(State::Checked)),
        ]
    );
}

#[test]
fn checked_checkbox_announces_checked() {
    let checkbox = node(
        4,
        Role::CheckBox,
        Some("Remember me"),
        None,
        StateSet::new().with(State::Checked),
    );

    assert_eq!(
        focus_segments(checkbox),
        vec![
            UtteranceSegment::label("Remember me"),
            UtteranceSegment::new(SegmentContent::Role(Role::CheckBox)),
            UtteranceSegment::new(SegmentContent::State(State::Checked)),
        ]
    );
}

#[test]
fn mixed_checkbox_does_not_announce_negated_checked() {
    let checkbox = node(
        5,
        Role::CheckBox,
        Some("Some of these"),
        None,
        StateSet::new().with(State::Mixed),
    );

    assert_eq!(
        focus_segments(checkbox),
        vec![
            UtteranceSegment::label("Some of these"),
            UtteranceSegment::new(SegmentContent::Role(Role::CheckBox)),
            UtteranceSegment::new(SegmentContent::State(State::Mixed)),
        ]
    );
}

#[test]
fn unpressed_toggle_button_announces_negated_pressed() {
    let toggle_button = node(900, Role::ToggleButton, Some("Bold"), None, StateSet::new());

    assert_eq!(
        focus_segments(toggle_button),
        vec![
            UtteranceSegment::label("Bold"),
            UtteranceSegment::new(SegmentContent::Role(Role::ToggleButton)),
            UtteranceSegment::new(SegmentContent::NegatedState(State::Pressed)),
        ]
    );
}

#[test]
fn pressed_toggle_button_announces_pressed() {
    let toggle_button = node(
        901,
        Role::ToggleButton,
        Some("Bold"),
        None,
        StateSet::new().with(State::Pressed),
    );

    assert_eq!(
        focus_segments(toggle_button),
        vec![
            UtteranceSegment::label("Bold"),
            UtteranceSegment::new(SegmentContent::Role(Role::ToggleButton)),
            UtteranceSegment::new(SegmentContent::State(State::Pressed)),
        ]
    );
}

#[test]
fn disabled_button_announces_unavailable_state() {
    let button = node(
        6,
        Role::Button,
        Some("OK"),
        None,
        StateSet::new().with(State::Disabled),
    );

    assert_eq!(
        focus_segments(button),
        vec![
            UtteranceSegment::label("OK"),
            UtteranceSegment::new(SegmentContent::Role(Role::Button)),
            UtteranceSegment::new(SegmentContent::State(State::Disabled)),
        ]
    );
}

#[test]
fn focus_related_states_are_never_announced_but_unselected_is() {
    let mut states = StateSet::new();
    states.insert(State::Focused);
    states.insert(State::Focusable);
    states.insert(State::Selectable);
    states.insert(State::Offscreen);
    let item = node(7, Role::ListItem, Some("Row"), None, states);

    assert_eq!(
        focus_segments(item),
        vec![
            UtteranceSegment::label("Row"),
            // NVDA's rule: a selectable item that is not selected announces
            // exactly that; focused/focusable/offscreen stay silent.
            UtteranceSegment::new(SegmentContent::NegatedState(State::Selected)),
        ]
    );
}

#[test]
fn selected_items_do_not_announce_positive_selected_on_focus() {
    let mut states = StateSet::new();
    states.insert(State::Selectable);
    states.insert(State::Selected);
    let item = node(7, Role::ListItem, Some("Row"), None, states);

    assert_eq!(
        focus_segments(item),
        vec![UtteranceSegment::label("Row"),],
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
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), source, focused));

    let other_value_changed = Input::Event {
        observed_at_us: 0,
        trace_id: TraceId::mint(),
        source,
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::ValueChanged {
            node_id: NodeId::new(9),
            value: Some("changed".to_string()),
        },
    };
    let (state, effects) = reduce(&state, &other_value_changed);

    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
    assert_eq!(
        state.focused().map(|(_, n)| n.value.clone()),
        Some(Some("a".to_string()))
    );
}

#[test]
fn a_name_change_on_the_focus_speaks_the_new_name_alone_queued() {
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
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), source, focused));

    let name_changed = Input::Event {
        observed_at_us: 0,
        trace_id: TraceId::mint(),
        source,
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::PropertyChanged {
            node_id,
            change: PropertyChange::Name(Some("New name".to_string())),
            child_count: None,
        },
    };
    let (state, effects) = reduce(&state, &name_changed);

    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::label("New name")])],
        "the new name alone, as NVDA speaks it"
    );
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
    states_changed_with_children(trace_id, source, node_id, states, None)
}

fn states_changed_with_children(
    trace_id: TraceId,
    source: Pid,
    node_id: NodeId,
    states: StateSet,
    child_count: Option<u32>,
) -> Input {
    Input::Event {
        observed_at_us: 0,
        trace_id,
        source,
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::PropertyChanged {
            node_id,
            change: PropertyChange::States(states),
            child_count,
        },
    }
}

/// An ancestor's state change spoken just before the focus moves into
/// another top-level window is cut off by that move, as NVDA's foreground
/// change cancels speech (`docs/parity/focus-events.md`, "When speech is
/// cut off, and cancellation of expired focus speech"): the outpost
/// reports the change ahead of the focus change it was observed before, so
/// this is the outcome however the two were batched.
#[test]
fn an_ancestors_change_before_a_focus_in_another_window_is_cut_off() {
    let source = Pid(1);
    let dialog = node(31, Role::Dialog, Some("Settings"), None, StateSet::new());
    let remove = node(
        32,
        Role::Button,
        Some("Remove"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    let (reader, _) = reduce(
        &SrState::new(),
        &focus_in(source, window(10), remove, vec![dialog]),
    );
    let (reader, effects) = reduce(
        &reader,
        &states_changed_input(
            TraceId::mint(),
            source,
            NodeId::new(31),
            states(&[State::Disabled]),
        ),
    );
    assert_eq!(heard(&effects), vec![queued(vec![state(State::Disabled)])]);
    let no = node(
        42,
        Role::Button,
        Some("No"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    let (_, effects) = reduce(&reader, &focus_in(source, window(20), no, vec![]));
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 42, &[], None)),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("No"), role(Role::Button)]),
        ],
        "speech stops before the new window's focus is spoken"
    );
}

/// A state change on an ancestor of the focus is spoken, as NVDA's base
/// state change handler speaks one (a focused button that changes the
/// state of the container above it), diffed against the ancestor's states
/// as the focus was reported with them; one on a node that is neither the
/// focus nor an ancestor stays silent.
#[test]
fn a_state_change_on_an_ancestor_of_the_focus_is_spoken() {
    let source = Pid(1);
    let header = node(
        31,
        Role::ColumnHeader,
        Some("Name"),
        None,
        states(&[State::Collapsed]),
    );
    let button = node(
        32,
        Role::Button,
        Some("Sort"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    let (reader, _) = reduce(
        &SrState::new(),
        &event_in(
            source,
            None,
            NormalizedEvent::FocusChanged {
                node: button,
                foreground: false,
                ancestors: vec![header],
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
            },
        ),
    );

    let (reader, effects) = reduce(
        &reader,
        &states_changed_input(
            TraceId::mint(),
            source,
            NodeId::new(31),
            states(&[State::Expanded]),
        ),
    );
    assert_eq!(heard(&effects), vec![queued(vec![state(State::Expanded)])]);

    let (reader, effects) = reduce(
        &reader,
        &states_changed_input(
            TraceId::mint(),
            source,
            NodeId::new(31),
            states(&[State::Expanded]),
        ),
    );
    assert_eq!(heard(&effects), vec![], "the same states again say nothing");

    let (_, effects) = reduce(
        &reader,
        &states_changed_input(
            TraceId::mint(),
            source,
            NodeId::new(99),
            states(&[State::Checked]),
        ),
    );
    assert_eq!(heard(&effects), vec![], "another node's change is silent");
}

/// Visited is said only of a link, and linked only when it changes, as
/// NVDA says them; a busy indicator does not say its value.
#[test]
fn visited_is_said_of_a_link_and_linked_only_as_a_change() {
    let link = node(
        61,
        Role::Link,
        Some("Home"),
        None,
        states(&[State::Visited, State::Linked]),
    );
    assert_eq!(
        focus_segments(link),
        vec![
            UtteranceSegment::label("Home"),
            role(Role::Link),
            state(State::Visited)
        ]
    );
    let button = node(
        62,
        Role::Button,
        Some("Home"),
        None,
        states(&[State::Visited, State::Focused]),
    );
    assert_eq!(
        focus_segments(button),
        vec![UtteranceSegment::label("Home"), role(Role::Button)]
    );
    let busy = node(
        63,
        Role::BusyIndicator,
        Some("Loading"),
        Some("50"),
        StateSet::new(),
    );
    assert_eq!(
        focus_segments(busy),
        vec![
            UtteranceSegment::label("Loading"),
            role(Role::BusyIndicator)
        ]
    );

    let source = Pid(1);
    let item = node(
        64,
        Role::ListItem,
        Some("Part"),
        None,
        states(&[State::Focused]),
    );
    let (reader, _) = reduce(&SrState::new(), &focus_event(TraceId::mint(), source, item));
    let (_, effects) = reduce(
        &reader,
        &states_changed_input(
            TraceId::mint(),
            source,
            NodeId::new(64),
            states(&[State::Focused, State::Linked]),
        ),
    );
    assert_eq!(heard(&effects), vec![queued(vec![state(State::Linked)])]);
}

/// A progress bar indicates its percentage by NVDA's rules (`docs/nvda/
/// object-model.md`, "How a progress bar reports its value"): focused or
/// not, as an indication rather than a spoken value, once it moves by a
/// percent or more from the last one indicated for a progress bar at the
/// same place; an off-screen one, or one whose value is no number, is an
/// ordinary value change.
#[test]
fn a_progress_bar_indicates_its_percentage() {
    let source = Pid(1);
    let at = |id: u64, value: &str, left: i32, offscreen: bool| {
        let progress = verbatim_model::ProgressReading {
            id: node(id, Role::ProgressBar, None, None, StateSet::new()).id,
            value: Some(value.to_owned()),
            offscreen,
            rect: Some(verbatim_model::Rect {
                left,
                top: 0,
                width: 100,
                height: 10,
            }),
        };
        event_in(
            source,
            None,
            NormalizedEvent::ProgressChanged { bar: progress },
        )
    };
    let bar = node(
        51,
        Role::ProgressBar,
        Some("Copying"),
        Some("0"),
        states(&[State::Focusable, State::Focused]),
    );
    let (reader, _) = reduce(&SrState::new(), &focus_event(TraceId::mint(), source, bar));

    let (reader, effects) = reduce(&reader, &at(51, "20", 0, false));
    assert_eq!(
        heard(&effects),
        vec![Heard::Other(Effect::PlayEarcon(Earcon::Progress(20)))],
        "the focused progress bar's value is not spoken"
    );
    let (reader, effects) = reduce(&reader, &at(51, "20.9%", 0, false));
    assert_eq!(heard(&effects), vec![], "less than a percent");
    let (reader, effects) = reduce(&reader, &at(52, "21", 0, false));
    assert_eq!(
        heard(&effects),
        vec![Heard::Other(Effect::PlayEarcon(Earcon::Progress(21)))],
        "remembered by place, so a new bar in the same place carries on"
    );
    let (reader, effects) = reduce(&reader, &at(53, "21", 500, false));
    assert_eq!(
        heard(&effects),
        vec![Heard::Other(Effect::PlayEarcon(Earcon::Progress(21)))],
        "another place has a memory of its own"
    );
    let (reader, effects) = reduce(&reader, &at(53, "90", 500, true));
    assert_eq!(heard(&effects), vec![], "off screen, and not the focus");
    let (_, effects) = reduce(&reader, &at(51, "Paused", 0, false));
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::value("Paused")])],
        "not a number: the focus's ordinary value change"
    );
}

/// A description change on the focus speaks the new description alone, as
/// NVDA's base handler does; the same description again, one that only
/// repeats the name, or another node's change says nothing.
#[test]
fn a_description_change_on_the_focus_is_spoken() {
    let source = Pid(1);
    let field = node(
        41,
        Role::EditableText,
        Some("Password"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, field),
    );
    let described = |node: u64, description: &str| Input::Event {
        observed_at_us: 0,
        trace_id: TraceId::mint(),
        source,
        backend: Backend::Msaa,
        window: None,
        event: NormalizedEvent::PropertyChanged {
            node_id: NodeId::new(node),
            change: PropertyChange::Description(Some(description.to_owned())),
            child_count: None,
        },
    };

    let (state, effects) = reduce(&state, &described(41, "Too short"));
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::new(
            SegmentContent::Description("Too short".to_owned())
        )])]
    );
    let (state, effects) = reduce(&state, &described(41, "Too short"));
    assert_eq!(heard(&effects), vec![], "unchanged");
    let (state, effects) = reduce(&state, &described(41, "Password"));
    assert_eq!(heard(&effects), vec![], "only the name again");
    let (_, effects) = reduce(&state, &described(42, "Elsewhere"));
    assert_eq!(heard(&effects), vec![], "not the focus");
}

/// A field's suggestions appearing play the suggestions-opened indication
/// only while the field is the focus; their going away, or a change of
/// another node, plays the suggestions-closed one
/// (`docs/parity/changes-and-selection.md`, "Suggestions appearing and going away").
#[test]
fn suggestions_opening_on_the_focus_and_closing_play_their_indications() {
    let source = Pid(1);
    let field = node(
        41,
        Role::EditableText,
        Some("Search"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, field),
    );
    let suggestions = |node: u64, open: bool| Input::Event {
        observed_at_us: 0,
        trace_id: TraceId::mint(),
        source,
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::PropertyChanged {
            node_id: NodeId::new(node),
            change: PropertyChange::Suggestions(open),
            child_count: None,
        },
    };

    let (state, effects) = reduce(&state, &suggestions(41, true));
    assert_eq!(effects, vec![Effect::PlayEarcon(Earcon::SuggestionsOpened)]);
    let (state, effects) = reduce(&state, &suggestions(41, false));
    assert_eq!(effects, vec![Effect::PlayEarcon(Earcon::SuggestionsClosed)]);
    let (_, effects) = reduce(&state, &suggestions(42, true));
    assert_eq!(
        effects,
        vec![Effect::PlayEarcon(Earcon::SuggestionsClosed)],
        "not the focus"
    );
}

#[test]
fn expanding_a_tree_view_item_says_how_many_items_it_holds() {
    let source = Pid(1);
    let node_id = NodeId::new(21);
    let focusable = StateSet::new()
        .with(State::Focusable)
        .with(State::Focused)
        .with(State::Selectable);
    let collapsed = focusable.with(State::Selected).with(State::Collapsed);
    let item = node(21, Role::TreeItem, Some("Roles"), None, collapsed);
    let (state, _) = reduce(&SrState::new(), &focus_event(TraceId::mint(), source, item));

    let expanded = focusable.with(State::Selected).with(State::Expanded);
    let (state, effects) = reduce(
        &state,
        &states_changed_with_children(TraceId::mint(), source, node_id, expanded, Some(52)),
    );
    assert_eq!(
        heard(&effects),
        vec![
            queued(vec![UtteranceSegment::new(SegmentContent::State(
                State::Expanded
            ))]),
            queued(vec![UtteranceSegment::new(SegmentContent::Phrase(
                Phrase::Items(52)
            ))]),
        ],
        "the state, then the count on its own"
    );

    // A further change while it stays expanded says no count, even with
    // one on the event: losing the selection says "not selected" alone.
    let unselected = focusable.with(State::Expanded);
    let (_, effects) = reduce(
        &state,
        &states_changed_with_children(TraceId::mint(), source, node_id, unselected, Some(52)),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![not(State::Selected)])],
        "only a change that makes the item expanded says the count"
    );
}

#[test]
fn expanding_without_a_count_says_only_expanded() {
    let source = Pid(1);
    let node_id = NodeId::new(22);
    let collapsed = StateSet::new().with(State::Focused).with(State::Collapsed);
    let item = node(22, Role::TreeItem, Some("Folder"), None, collapsed);
    let (state, _) = reduce(&SrState::new(), &focus_event(TraceId::mint(), source, item));

    let expanded = StateSet::new().with(State::Focused).with(State::Expanded);
    let (_, effects) = reduce(
        &state,
        &states_changed_input(TraceId::mint(), source, node_id, expanded),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::new(SegmentContent::State(
            State::Expanded
        ))])],
        "a tree item read through UIA says no count"
    );
}

#[test]
fn states_changed_checkbox_toggle_on_announces_checked() {
    let source = Pid(1);
    let node_id = NodeId::new(20);
    let checkbox = node(20, Role::CheckBox, Some("Agree"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, checkbox),
    );

    let trace_id = TraceId::mint();
    let toggled_on = states_changed_input(
        trace_id,
        source,
        node_id,
        StateSet::new().with(State::Checked),
    );
    let (state, effects) = reduce(&state, &toggled_on);

    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::new(SegmentContent::State(
            State::Checked
        ))])]
    );
    assert_eq!(only_speech(&effects)[0].trace_id, trace_id);
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
        StateSet::new().with(State::Focused).with(State::Checked),
    );
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, checkbox),
    );

    let trace_id = TraceId::mint();
    let toggled_off = states_changed_input(
        trace_id,
        source,
        node_id,
        StateSet::new().with(State::Focused),
    );
    let (state, effects) = reduce(&state, &toggled_off);

    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::new(
            SegmentContent::NegatedState(State::Checked)
        )])]
    );
    assert_eq!(only_speech(&effects)[0].trace_id, trace_id);
    assert_eq!(
        state.focused().map(|(_, n)| n.states),
        Some(StateSet::new().with(State::Focused))
    );
}

#[test]
fn states_changed_as_the_focus_leaves_an_item_is_silent() {
    // The list item the focus is leaving loses its selection and the
    // focus before the focus event for the next item arrives: no longer
    // focused, its "not selected" is not spoken.
    let source = Pid(1);
    let node_id = NodeId::new(26);
    let selectable = StateSet::new()
        .with(State::Focusable)
        .with(State::Selectable);
    let item = node(
        26,
        Role::ListItem,
        Some("Speech"),
        None,
        selectable.with(State::Focused).with(State::Selected),
    );
    let (state, _) = reduce(&SrState::new(), &focus_event(TraceId::mint(), source, item));

    let left = states_changed_input(TraceId::mint(), source, node_id, selectable);
    let (_, effects) = reduce(&state, &left);

    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
}

#[test]
fn states_changed_unselecting_the_focused_item_announces_not_selected() {
    let source = Pid(1);
    let node_id = NodeId::new(27);
    let focused = StateSet::new()
        .with(State::Focused)
        .with(State::Focusable)
        .with(State::Selectable);
    let item = node(
        27,
        Role::ListItem,
        Some("Speech"),
        None,
        focused.with(State::Selected),
    );
    let (state, _) = reduce(&SrState::new(), &focus_event(TraceId::mint(), source, item));

    let unselected = states_changed_input(TraceId::mint(), source, node_id, focused);
    let (_, effects) = reduce(&state, &unselected);

    assert_eq!(heard(&effects), vec![queued(vec![not(State::Selected)])]);
}

#[test]
fn states_changed_disabled_appearing_announces_unavailable() {
    let source = Pid(1);
    let node_id = NodeId::new(22);
    let button = node(22, Role::Button, Some("Submit"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, button),
    );

    let disabled = states_changed_input(
        TraceId::mint(),
        source,
        node_id,
        StateSet::new().with(State::Disabled),
    );
    let (_, effects) = reduce(&state, &disabled);

    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::new(SegmentContent::State(
            State::Disabled
        ))])]
    );
}

#[test]
fn a_button_disabled_after_the_focus_left_it_is_silent() {
    // The Reset button hands the focus to the tree, then is disabled; its
    // state change arrives before the tree's focus event. It no longer
    // reports itself focused, so "unavailable" is not spoken, nor is any
    // further change before the focus event.
    let source = Pid(1);
    let node_id = NodeId::new(28);
    let focusable = StateSet::new().with(State::Focusable);
    let button = node(
        28,
        Role::Button,
        Some("Reset"),
        None,
        focusable.with(State::Focused),
    );
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, button),
    );

    let disabled = StateSet::new().with(State::Disabled);
    let (state, effects) = reduce(
        &state,
        &states_changed_input(TraceId::mint(), source, node_id, disabled),
    );
    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
    assert_eq!(state.focused().map(|(_, n)| n.states), Some(disabled));

    let (_, effects) = reduce(
        &state,
        &states_changed_input(TraceId::mint(), source, node_id, focusable),
    );
    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
}

#[test]
fn a_button_disabled_while_still_focused_says_unavailable() {
    let source = Pid(1);
    let node_id = NodeId::new(29);
    let focused = StateSet::new().with(State::Focusable).with(State::Focused);
    let button = node(29, Role::Button, Some("Reset"), None, focused);
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, button),
    );

    let (_, effects) = reduce(
        &state,
        &states_changed_input(
            TraceId::mint(),
            source,
            node_id,
            focused.with(State::Disabled),
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::new(SegmentContent::State(
            State::Disabled
        ))])]
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
        &focus_event(TraceId::mint(), source, checkbox),
    );

    let same = states_changed_input(TraceId::mint(), source, node_id, states);
    let (_, effects) = reduce(&state, &same);

    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
}

#[test]
fn states_changed_for_non_focused_node_is_ignored() {
    let source = Pid(1);
    let other_id = NodeId::new(25);
    let focused = node(24, Role::CheckBox, Some("Agree"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, focused),
    );

    let other_changed = states_changed_input(
        TraceId::mint(),
        source,
        other_id,
        StateSet::new().with(State::Checked),
    );
    let (state, effects) = reduce(&state, &other_changed);

    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
    assert_eq!(
        state.focused().map(|(_, n)| n.states),
        Some(StateSet::new())
    );
}

#[test]
fn fetch_completed_for_unknown_query_id_is_ignored() {
    let state = SrState::new();
    let completed = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: QueryId(9999),
        kind: QueryKind::Parent,
        result: FetchResult::Gone,
    };
    let (state, effects) = reduce(&state, &completed);
    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
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
        focus_event(TraceId::mint(), source, button),
        Input::Event {
            observed_at_us: 0,
            trace_id: TraceId::mint(),
            source,
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::FocusChanged {
                foreground: false,
                node: checkbox,
                ancestors: Vec::new(),
                selected_child: None,
                text: None,
                earlier: None,
                group: None,
            },
        },
        Input::Event {
            observed_at_us: 0,
            trace_id: TraceId::mint(),
            source,
            backend: Backend::Uia,
            window: None,
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
    let mut recorder = ReducerRecorder::with_default_bounds(SrState::new());
    let mut state = SrState::new();
    let script = sample_script();

    let mut live_effects = Vec::new();
    for input in &script {
        let effects = verbatim_core::reduce(&mut state, input);
        recorder.record_input(input.clone(), effects.len(), &state);
        live_effects.push(effects);
    }

    let dumped = recorder.dump_inputs();
    assert_eq!(dumped, script);

    let replayed = replay(recorder.checkpoint(), &dumped);
    assert_eq!(replayed, live_effects);
}

/// A window that has dropped its oldest inputs replays from the checkpoint
/// the recorder keeps with it, through a dump written and read back, to
/// exactly the effects the live session produced for the inputs it kept.
/// The focus that makes the kept value changes speak was set by an input
/// long since dropped, so a replay from an empty state would say nothing.
#[test]
fn a_window_that_dropped_its_start_replays_from_its_checkpoint() {
    let slider = node(1, Role::Slider, Some("Rate"), Some("0"), StateSet::new());
    let mut script = vec![focus_event(TraceId::mint(), Pid(1), slider)];
    script.extend((1..=20).map(|value| value_changed(1, &value.to_string())));

    let mut recorder = ReducerRecorder::new(4, 1 << 20, SrState::new());
    let mut state = SrState::new();
    let mut live_effects = Vec::new();
    for input in &script {
        let effects = verbatim_core::reduce(&mut state, input);
        recorder.record_input(input.clone(), effects.len(), &state);
        live_effects.push(effects);
    }
    let kept = recorder.dump_inputs();
    assert!(
        kept.len() < script.len(),
        "the window has dropped its start"
    );
    let live_tail = &live_effects[script.len() - kept.len()..];
    // Input `index` of the script sets the slider to `index`, which is
    // spoken as the new value alone.
    let expected_tail: Vec<Vec<Heard>> = (script.len() - kept.len()..script.len())
        .map(|index| vec![queued(vec![UtteranceSegment::value(index.to_string())])])
        .collect();
    assert_eq!(
        live_tail
            .iter()
            .map(|effects| heard(effects))
            .collect::<Vec<_>>(),
        expected_tail
    );

    let entries: Vec<_> = recorder.entries().cloned().collect();
    let mut buffer = Vec::new();
    verbatim_core::write_dump(&mut buffer, "test", "now", recorder.checkpoint(), &entries)
        .expect("writes");
    let contents = verbatim_core::read_dump(&mut buffer.as_slice()).expect("reads");

    assert_eq!(replay(&contents.base, &kept), live_tail);
    assert_ne!(replay(&SrState::new(), &kept), live_tail);
}

/// A focus event whose snapshot arrives with an ancestor chain, outermost
/// first Ã¢â‚¬â€ the enriched form outposts emit from M3 on.
fn focus_event_with_ancestors(
    trace_id: TraceId,
    source: Pid,
    snapshot: NodeSnapshot,
    ancestors: Vec<NodeSnapshot>,
) -> Input {
    Input::Event {
        observed_at_us: 0,
        trace_id,
        source,
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::FocusChanged {
            foreground: false,
            node: snapshot,
            ancestors,
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
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
        &focus_event_with_ancestors(TraceId::mint(), source, button, vec![window, dialog]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 102, &[100, 101], None),
            vec![
                // The named window and then the dialog introduce themselves,
                // outermost first, each in an utterance of its own.
                vec![
                    UtteranceSegment::label("Settings - App"),
                    UtteranceSegment::new(SegmentContent::Role(Role::Window)),
                ],
                vec![
                    UtteranceSegment::label("Save changes"),
                    UtteranceSegment::new(SegmentContent::Role(Role::Dialog)),
                ],
                vec![
                    UtteranceSegment::label("Save"),
                    UtteranceSegment::new(SegmentContent::Role(Role::Button)),
                ],
            ]
        )
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
        &focus_event_with_ancestors(TraceId::mint(), source, save, vec![dialog.clone()]),
    );
    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, cancel, vec![dialog]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 103, &[101], None),
            vec![vec![
                UtteranceSegment::label("Cancel"),
                UtteranceSegment::new(SegmentContent::Role(Role::Button)),
            ]]
        )
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
        &focus_event_with_ancestors(TraceId::mint(), Pid(1), edit_a, vec![dialog]),
    );
    let state = switch_to(&state, Pid(2));
    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(
            TraceId::mint(),
            Pid(2),
            edit_b,
            vec![
                window_node(2000, Backend::Uia, Role::Window, "App", 2000),
                dialog_b,
            ],
        ),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(2), 202, &[2000, 201], Some((OutpostId(2), 2000))),
            vec![
                vec![UtteranceSegment::label("Open"), role(Role::Dialog)],
                vec![UtteranceSegment::label("Search"), role(Role::EditableText)],
            ]
        ),
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
        &focus_event_with_ancestors(TraceId::mint(), source, field, vec![nameless, named]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 302, &[300, 301], None),
            vec![
                vec![UtteranceSegment::label("Margins"), role(Role::Group)],
                vec![UtteranceSegment::label("Top"), role(Role::SpinButton)],
            ]
        )
    );
}

#[test]
fn a_named_list_ancestor_is_announced_as_entered_context() {
    // The settings dialog's category list ("Categories:") must be spoken when
    // focus enters it Ã¢â‚¬â€ NVDA presents a named list ancestor.
    let state = SrState::new();
    let source = Pid(1);
    let list = node(600, Role::List, Some("Categories"), None, StateSet::new());
    let item = node(601, Role::ListItem, Some("Speech"), None, StateSet::new());

    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, item, vec![list]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 601, &[600], None),
            vec![
                vec![UtteranceSegment::label("Categories"), role(Role::List)],
                vec![UtteranceSegment::label("Speech")],
            ]
        )
    );
}

#[test]
fn an_unnamed_tree_ancestor_is_still_announced() {
    // NVDA treats a tree as content regardless of name; an unnamed tree
    // ancestor announces as a bare "tree view".
    let state = SrState::new();
    let source = Pid(1);
    let tree = node(610, Role::Tree, None, None, StateSet::new());
    let item = node(611, Role::TreeItem, Some("Home"), None, StateSet::new());

    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, item, vec![tree]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 611, &[610], None),
            vec![
                vec![role(Role::Tree)],
                vec![UtteranceSegment::label("Home")]
            ]
        )
    );
}

#[test]
fn a_tree_view_taking_the_focus_on_its_item_is_spoken_as_the_items_ancestor() {
    // A Win32 tree view taking the focus is reported by its focused item,
    // with the tree as its ancestor (`docs/parity/windows-and-processes.md`, "A control's own
    // focus with a focused child"): NVDA says the tree's name and role,
    // without its shortcut, and then the item.
    let state = SrState::new();
    let mut tree = node(630, Role::Tree, Some("Categories"), None, StateSet::new());
    tree.details.keyboard_shortcut = Some("Alt+i".to_owned());
    let mut item = tree_item(631, "General", 0);
    item.details.position_in_set = Some(1);
    item.details.set_size = Some(5);

    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), Pid(1), item, vec![tree]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 631, &[630], None),
            vec![
                vec![UtteranceSegment::label("Categories"), role(Role::Tree)],
                vec![
                    UtteranceSegment::new(SegmentContent::Level(0)),
                    UtteranceSegment::label("General"),
                    UtteranceSegment::new(SegmentContent::Position {
                        position: 1,
                        set_size: Some(5),
                    }),
                ],
            ]
        )
    );
}

#[test]
fn an_unnamed_group_ancestor_is_dropped() {
    let state = SrState::new();
    let source = Pid(1);
    let group = node(620, Role::Group, None, None, StateSet::new());
    let button = node(621, Role::Button, Some("OK"), None, StateSet::new());

    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, button, vec![group]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 621, &[620], None),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        ),
        "a nameless group adds nothing and is not announced"
    );
}

#[test]
fn a_named_window_ancestor_is_announced_as_entered_context() {
    // NVDA presents a named window entered as an ancestor like any other
    // container; an unnamed one is layout.
    let state = SrState::new();
    let source = Pid(1);
    let window = node(
        630,
        Role::Window,
        Some("App - Window"),
        None,
        StateSet::new(),
    );
    let button = node(631, Role::Button, Some("OK"), None, StateSet::new());

    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, button, vec![window]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 631, &[630], None),
            vec![
                vec![UtteranceSegment::label("App - Window"), role(Role::Window)],
                vec![UtteranceSegment::label("OK"), role(Role::Button)],
            ]
        )
    );
}

#[test]
fn an_unnamed_window_ancestor_is_not_announced() {
    let window_node = node(632, Role::Window, Some("  "), None, StateSet::new());
    let button = node(633, Role::Button, Some("OK"), None, StateSet::new());

    let (_, effects) = reduce(
        &SrState::new(),
        &focus_event_with_ancestors(TraceId::mint(), Pid(1), button, vec![window_node]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 633, &[632], None),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        )
    );
}

#[test]
fn a_border_or_white_space_ancestor_is_not_announced() {
    // NVDA counts both roles as layout, named or not.
    let border = node(634, Role::Border, Some("Frame"), None, StateSet::new());
    let space = node(635, Role::Whitespace, Some("Gap"), None, StateSet::new());
    let button = node(636, Role::Button, Some("OK"), None, StateSet::new());

    let (_, effects) = reduce(
        &SrState::new(),
        &focus_event_with_ancestors(TraceId::mint(), Pid(1), button, vec![border, space]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 636, &[634, 635], None),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        )
    );
}

#[test]
fn list_item_and_editable_text_ancestors_are_dropped() {
    // NVDA's focus-ancestry exclusions: item and editable-text roles never
    // announce as entered context, even when named.
    let state = SrState::new();
    let source = Pid(1);
    let list_item = node(640, Role::ListItem, Some("Row"), None, StateSet::new());
    let edit = node(
        641,
        Role::EditableText,
        Some("Field"),
        None,
        StateSet::new(),
    );
    let button = node(642, Role::Button, Some("Go"), None, StateSet::new());

    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, button, vec![list_item, edit]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 642, &[640, 641], None),
            vec![vec![UtteranceSegment::label("Go"), role(Role::Button)]]
        )
    );
}

#[test]
fn details_speak_in_nvda_property_order() {
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
        top_level_client: false,
        class_name: None,
        row: Some(3),
        column: Some(1),
        parent_automation_id: None,
        window: None,
    };

    // The first level spoken goes first ("Where the level goes" in
    // `docs/nvda/speech.md`); everything else keeps NVDA's order, a table
    // cell's row and column after the shortcut and before the position
    // (`docs/parity/focus-announcement.md`, "Table cell coordinates").
    assert_eq!(
        focus_segments(item),
        vec![
            UtteranceSegment::new(SegmentContent::Level(1)),
            UtteranceSegment::label("Report.txt"),
            UtteranceSegment::new(SegmentContent::Description("Text document".to_owned())),
            UtteranceSegment::new(SegmentContent::Shortcut("Alt+R".to_owned())),
            UtteranceSegment::new(SegmentContent::Row(3)),
            UtteranceSegment::new(SegmentContent::Column(1)),
            UtteranceSegment::new(SegmentContent::Position {
                position: 2,
                set_size: Some(5),
            }),
        ]
    );
}

/// A table cell with no name says its row and column without its role,
/// which they stand in for as a name would (`docs/parity/focus-announcement.md`,
/// "Table cell coordinates").
#[test]
fn a_nameless_cell_says_its_row_and_column_without_its_role() {
    let mut cell = node(400, Role::Cell, None, None, StateSet::new());
    cell.details.row = Some(4);
    cell.details.column = Some(2);
    assert_eq!(
        focus_segments(cell),
        vec![
            UtteranceSegment::new(SegmentContent::Row(4)),
            UtteranceSegment::new(SegmentContent::Column(2)),
        ]
    );
}

/// A tree item at `level`, named `name`, as an MSAA tree view reports it.
fn tree_item(id: u64, name: &str, level: u32) -> NodeSnapshot {
    let mut item = node(id, Role::TreeItem, Some(name), None, StateSet::new());
    item.details.level = Some(level);
    item
}

#[test]
fn a_tree_items_level_goes_first_only_when_it_changes() {
    let level = |level| UtteranceSegment::new(SegmentContent::Level(level));
    let mut state = SrState::new();
    let mut steps = Vec::new();
    for (id, name, depth) in [
        (500, "Hardware Resources", 1),
        (501, "Components", 1),
        (502, "System Summary", 0),
        (503, "Software Environment", 1),
    ] {
        let (next, effects) = reduce(
            &state,
            &focus_event_with_ancestors(
                TraceId::mint(),
                Pid(1),
                tree_item(id, name, depth),
                vec![window_node(499, Backend::Msaa, Role::Window, "", 499)],
            ),
        );
        state = next;
        steps.push(heard(&effects));
    }
    assert_eq!(
        steps,
        vec![
            focus_heard(
                focus_now(OutpostId(1), 500, &[499], None),
                vec![vec![
                    level(1),
                    UtteranceSegment::label("Hardware Resources")
                ]]
            ),
            focus_heard(
                focus_now(OutpostId(1), 501, &[499], None),
                vec![vec![UtteranceSegment::label("Components"), level(1)]]
            ),
            focus_heard(
                focus_now(OutpostId(1), 502, &[499], None),
                vec![vec![level(0), UtteranceSegment::label("System Summary")]]
            ),
            focus_heard(
                focus_now(OutpostId(1), 503, &[499], None),
                vec![vec![
                    level(1),
                    UtteranceSegment::label("Software Environment")
                ]]
            ),
        ]
    );
}

/// A focus event carrying a selection container's selected child.
fn focus_event_with_selection(
    trace_id: TraceId,
    source: Pid,
    snapshot: NodeSnapshot,
    selected_child: Option<NodeSnapshot>,
) -> Input {
    Input::Event {
        observed_at_us: 0,
        trace_id,
        source,
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::FocusChanged {
            foreground: false,
            node: snapshot,
            ancestors: Vec::new(),
            selected_child,
            text: None,
            earlier: None,
            group: None,
        },
    }
}

/// A repeated focus on the same list is silent even when its selected item
/// differs, as NVDA compares a focus by identity alone; the new item is then
/// announced by its selection event.
#[test]
fn a_repeated_focus_with_another_selected_item_is_silent_until_the_selection_event() {
    let app = Pid(1);
    let list = node(10, Role::List, Some("Files"), None, StateSet::new());
    let first = node(11, Role::ListItem, Some("a.txt"), None, StateSet::new());
    let second = node(12, Role::ListItem, Some("b.txt"), None, StateSet::new());
    let (state, effects) = reduce(
        &SrState::new(),
        &focus_event_with_selection(TraceId::mint(), app, list.clone(), Some(first)),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            plain_focus(10),
            vec![vec![
                UtteranceSegment::label("Files"),
                role(Role::List),
                UtteranceSegment::label("a.txt"),
            ]]
        )
    );

    let (state, effects) = reduce(
        &state,
        &focus_event_with_selection(TraceId::mint(), app, list, Some(second.clone())),
    );
    assert!(effects.is_empty(), "the same focus is not announced again");

    let (_, effects) = reduce(&state, &selection_event(TraceId::mint(), app, second));
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::label("b.txt")])],
        "the new item is announced by its selection event"
    );
}

fn selection_event(trace_id: TraceId, source: Pid, node: NodeSnapshot) -> Input {
    Input::Event {
        observed_at_us: 0,
        trace_id,
        source,
        backend: Backend::Uia,
        window: None,
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
        &focus_event_with_selection(TraceId::mint(), source, list, Some(item)),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            plain_focus(500),
            vec![vec![
                UtteranceSegment::label("Categories"),
                role(Role::List),
                UtteranceSegment::label("Speech"),
            ]]
        )
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
        &focus_event_with_selection(TraceId::mint(), source, list, Some(speech)),
    );

    // Arrowing to another item announces it.
    let (state, effects) = reduce(
        &state,
        &selection_event(TraceId::mint(), source, keyboard.clone()),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::label("Keyboard")])]
    );

    // A duplicate selection event for the same item stays silent.
    let (_, effects) = reduce(&state, &selection_event(TraceId::mint(), source, keyboard));
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
        &focus_event_with_selection(TraceId::mint(), source, list, Some(speech.clone())),
    );

    // Platforms often raise a selection event right after focus lands; the
    // focus announcement already spoke this item.
    let (_, effects) = reduce(&state, &selection_event(TraceId::mint(), source, speech));
    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
}

#[test]
fn selection_changes_outside_a_focused_container_stay_silent() {
    let state = SrState::new();
    let button = node(600, Role::Button, Some("OK"), None, StateSet::new());
    let item = node(601, Role::ListItem, Some("Row"), None, StateSet::new());

    // Focus on a non-container: selection noise elsewhere is not announced.
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), Pid(1), button));
    let (state, effects) = reduce(
        &state,
        &selection_event(TraceId::mint(), Pid(1), item.clone()),
    );
    assert!(effects.is_empty(), "focus is not on a selection container");

    // A selection event from a different application is not announced
    // either.
    let list = node(602, Role::List, Some("Files"), None, StateSet::new());
    let (state, _) = reduce(
        &state,
        &focus_event_with_selection(TraceId::mint(), Pid(1), list, None),
    );
    let (_, effects) = reduce(&state, &selection_event(TraceId::mint(), Pid(2), item));
    assert!(effects.is_empty(), "another application's selection");
}

fn notification_event(
    trace_id: TraceId,
    source: Pid,
    processing: verbatim_model::NotificationProcessing,
    display: Option<&str>,
) -> Input {
    Input::Event {
        observed_at_us: 0,
        trace_id,
        source,
        backend: Backend::Uia,
        window: None,
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
            NotificationProcessing::MostRecent,
            Some("Snap layout available"),
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![Heard::Say(
            SpeechPriority::Interrupt,
            vec![UtteranceSegment::text("Snap layout available")]
        )]
    );

    // All: queued behind current speech.
    let (_, effects) = reduce(
        &SrState::new(),
        &notification_event(
            TraceId::mint(),
            Pid(1),
            NotificationProcessing::All,
            Some("Download complete"),
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::text("Download complete")])]
    );
}

#[test]
fn notification_without_text_is_silent() {
    use verbatim_model::NotificationProcessing;

    let (_, effects) = reduce(
        &SrState::new(),
        &notification_event(TraceId::mint(), Pid(1), NotificationProcessing::All, None),
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
        &focus_event(TraceId::mint(), source, button.clone()),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            plain_focus(700),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        ),
        "first focus is announced"
    );

    // The exact same node focusing again from the same app: suppressed.
    let (_, effects) = reduce(&state, &focus_event(TraceId::mint(), source, button));
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
        &focus_event(TraceId::mint(), Pid(1), a.clone()),
    );
    // Visit another control (different app), then come back to the first.
    let state = switch_to(&state, Pid(2));
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), Pid(2), b));
    let state = switch_to(&state, Pid(1));
    // Its chain meets the line at its window, which the return brought
    // forward.
    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(
            TraceId::mint(),
            Pid(1),
            a,
            vec![window_node(1000, Backend::Uia, Role::Window, "App", 1000)],
        ),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 700, &[1000], Some((OutpostId(1), 1000))),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        ),
        "focus that differs from the last announced one is announced, even if seen earlier"
    );
}

// ---- Object navigation and review cursor (M3 reducer item 4) ----

use verbatim_model::{FocusNow, FocusValidity, ObjectKey, ReviewCommand};

fn command(trace_id: TraceId, cmd: ReviewCommand, repeat: u8) -> Input {
    Input::Command {
        trace_id,
        command: cmd,
        repeat,
    }
}

/// The query of the one fetch that is all of `effects`.
fn only_fetch(effects: &[Effect]) -> verbatim_model::Query {
    match effects {
        [Effect::Fetch(query)] => *query,
        other => panic!("expected one Fetch and nothing else, got {other:?}"),
    }
}

/// Focus a node so the navigator is seeded, returning the resulting state.
fn focused(source: Pid, snapshot: NodeSnapshot) -> SrState {
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, snapshot),
    );
    state
}

#[test]
fn report_object_announces_spells_then_copies() {
    let source = Pid(1);
    let edit = node(
        10,
        Role::ComboBox,
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
    assert_eq!(
        heard(&effects),
        vec![queued(vec![
            UtteranceSegment::label("Name"),
            role(Role::ComboBox),
            UtteranceSegment::value("Ann"),
        ])]
    );

    // Second press: spell the name and value, as NVDA does, the space
    // spoken as "space" and the capitals marked for a raised pitch.
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 1),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![
            UtteranceSegment::new(SegmentContent::SpelledCapital("N".to_owned())),
            UtteranceSegment::text("a"),
            UtteranceSegment::text("m"),
            UtteranceSegment::text("e"),
            UtteranceSegment::new(SegmentContent::Message(verbatim_model::Message::Space)),
            UtteranceSegment::new(SegmentContent::SpelledCapital("A".to_owned())),
            UtteranceSegment::text("n"),
            UtteranceSegment::text("n"),
        ])]
    );

    // Third press: copy name and value to the clipboard.
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 2),
    );
    assert_eq!(
        effects,
        vec![Effect::CopyToClipboard("Name Ann".to_owned())]
    );
}

#[test]
fn navigate_to_parent_fetches_then_moves_and_announces() {
    let source = Pid(1);
    let button = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);

    // The command emits a navigation fetch.
    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = only_fetch(&effects);
    assert_eq!(query.kind, QueryKind::Parent);
    assert_eq!(query.node_id, NodeId::in_outpost(outpost_of(source), 10));

    // The completion moves the navigator to the parent and announces it.
    let parent = node(11, Role::Group, Some("Buttons"), None, StateSet::new());
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        kind: query.kind,
        result: FetchResult::Node(parent),
    };
    let (_, effects) = reduce(&state, &completion);
    assert_eq!(
        said(&effects),
        vec![UtteranceSegment::label("Buttons"), role(Role::Group)]
    );
}

#[test]
fn a_named_item_leaves_its_role_unspoken_on_focus_but_an_unnamed_one_speaks_it() {
    // NVDA's rule ("When the role is spoken" in docs/nvda/speech.md): on
    // focus, a list item with a name says its name alone; with nothing else
    // to hear, it still says "list item".
    let named = node(7, Role::ListItem, Some("alpha.txt"), None, StateSet::new());
    assert_eq!(
        focus_segments(named),
        vec![UtteranceSegment::label("alpha.txt")]
    );

    let unnamed = node(8, Role::ListItem, None, None, StateSet::new());
    assert_eq!(
        focus_segments(unnamed),
        vec![UtteranceSegment::new(SegmentContent::Role(Role::ListItem))]
    );
}

#[test]
fn the_same_focus_reported_again_after_its_window_was_renamed_is_silent() {
    // File Explorer fills in its window title while the first file already
    // has focus, and reports that focus twice.
    let source = Pid(1);
    let window = node(
        30,
        Role::Dialog,
        Some("vbtest - File"),
        None,
        StateSet::new(),
    );
    let item = node(31, Role::ListItem, Some("alpha.txt"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event_with_ancestors(TraceId::mint(), source, item.clone(), vec![window]),
    );
    let renamed = node(
        30,
        Role::Dialog,
        Some("vbtest - File Explorer"),
        None,
        StateSet::new(),
    );
    let (state, effects) = reduce(
        &state,
        &focus_event_with_ancestors(TraceId::mint(), source, item, vec![renamed]),
    );
    assert!(effects.is_empty(), "already the focus");
    // The repeat updates the reading Core holds, which review reads; what
    // Core spoke stays as it was.
    let (_, ancestors, _) = state.focus_report();
    assert_eq!(
        ancestors
            .iter()
            .map(|ancestor| ancestor.name.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("vbtest - File Explorer")],
        "the reading takes the new title"
    );
}

#[test]
fn a_name_change_after_a_repeat_carried_it_is_spoken() {
    // A repeat of the focus reads the new name before the name change's own
    // event comes; the change is compared with the name last spoken.
    let source = Pid(1);
    let first = node(42, Role::Button, Some("Connect"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, first),
    );
    let renamed = node(42, Role::Button, Some("Disconnect"), None, StateSet::new());
    let (state, effects) = reduce(&state, &focus_event(TraceId::mint(), source, renamed));
    assert_eq!(heard(&effects), vec![], "already the focus");
    let name_changed = event_in(
        source,
        None,
        NormalizedEvent::PropertyChanged {
            node_id: NodeId::new(42),
            change: PropertyChange::Name(Some("Disconnect".to_string())),
            child_count: None,
        },
    );
    let (_, effects) = reduce(&state, &name_changed);
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::label("Disconnect")])]
    );
}

#[test]
fn a_name_change_the_report_of_the_focus_spoke_is_silent() {
    // A repeat carries the new name in; the report-focus command speaks it,
    // which moves what was last spoken of the focus, so the name change's
    // own event finds it already spoken.
    let source = Pid(1);
    let first = node(42, Role::Button, Some("Connect"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, first),
    );
    let renamed = node(42, Role::Button, Some("Disconnect"), None, StateSet::new());
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), source, renamed));
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportFocus, 0),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![
            UtteranceSegment::label("Disconnect"),
            role(Role::Button),
        ])]
    );
    let name_changed = event_in(
        source,
        None,
        NormalizedEvent::PropertyChanged {
            node_id: NodeId::new(42),
            change: PropertyChange::Name(Some("Disconnect".to_string())),
            child_count: None,
        },
    );
    let (_, effects) = reduce(&state, &name_changed);
    assert_eq!(heard(&effects), vec![]);
}

#[test]
fn a_name_change_the_navigator_s_return_to_the_focus_spoke_is_silent() {
    // Moving the navigator to the focus announces the focus, which moves
    // what was last spoken of it.
    let source = Pid(1);
    let first = node(44, Role::Button, Some("Play"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, first),
    );
    let renamed = node(44, Role::Button, Some("Pause"), None, StateSet::new());
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), source, renamed));
    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::ToFocus, 0));
    assert_eq!(
        heard(&effects),
        vec![queued(vec![
            message(verbatim_model::Message::MoveToFocus),
            UtteranceSegment::label("Pause"),
            role(Role::Button),
        ])]
    );
    let name_changed = event_in(
        source,
        None,
        NormalizedEvent::PropertyChanged {
            node_id: NodeId::new(44),
            change: PropertyChange::Name(Some("Pause".to_string())),
            child_count: None,
        },
    );
    let (_, effects) = reduce(&state, &name_changed);
    assert_eq!(heard(&effects), vec![]);
}

#[test]
fn a_state_change_the_report_of_the_object_on_the_focus_spoke_is_silent() {
    // The navigator rests on the focus, so reporting the current object
    // speaks the focus's states, which moves what was last spoken of it.
    let source = Pid(1);
    let unchecked = node(43, Role::CheckBox, Some("Wrap"), None, StateSet::new());
    let (reader, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, unchecked),
    );
    let checked = node(
        43,
        Role::CheckBox,
        Some("Wrap"),
        None,
        states(&[State::Checked]),
    );
    let (reader, _) = reduce(&reader, &focus_event(TraceId::mint(), source, checked));
    let (reader, effects) = reduce(
        &reader,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![
            UtteranceSegment::label("Wrap"),
            role(Role::CheckBox),
            state(State::Checked),
        ])]
    );
    let (_, effects) = reduce(
        &reader,
        &states_changed_input(
            TraceId::mint(),
            source,
            NodeId::new(43),
            states(&[State::Checked]),
        ),
    );
    assert_eq!(heard(&effects), vec![]);
}

#[test]
fn a_name_change_a_report_of_a_fetched_copy_of_the_focus_spoke_is_spoken() {
    // The application renames the focus before any event; navigating to the
    // parent and back reads the focus's node afresh, a separate object, so
    // reporting it leaves what was last spoken of the focus alone and the
    // name change's own event is still spoken.
    let source = Pid(1);
    let first = node(45, Role::Button, Some("Connect"), None, StateSet::new());
    let state = focused(source, first);
    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = only_fetch(&effects);
    let window = node(10, Role::Window, Some("App"), None, StateSet::new());
    let (state, _) = reduce(
        &state,
        &Input::FetchCompleted {
            trace_id: TraceId::mint(),
            query_id: query.query_id,
            kind: query.kind,
            result: FetchResult::Node(window),
        },
    );
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::FirstChild, 0),
    );
    let query = only_fetch(&effects);
    // The outpost answers with the id it gave the focus.
    let mut fetched = node(45, Role::Button, Some("Disconnect"), None, StateSet::new());
    fetched.assign_outpost(outpost_of(source));
    let (state, effects) = reduce(
        &state,
        &Input::FetchCompleted {
            trace_id: TraceId::mint(),
            query_id: query.query_id,
            kind: query.kind,
            result: FetchResult::Node(fetched),
        },
    );
    assert_eq!(
        said(&effects),
        vec![UtteranceSegment::label("Disconnect"), role(Role::Button)]
    );
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![
            UtteranceSegment::label("Disconnect"),
            role(Role::Button),
        ])]
    );
    let name_changed = event_in(
        source,
        None,
        NormalizedEvent::PropertyChanged {
            node_id: NodeId::new(45),
            change: PropertyChange::Name(Some("Disconnect".to_string())),
            child_count: None,
        },
    );
    let (_, effects) = reduce(&state, &name_changed);
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::label("Disconnect")])]
    );
}

#[test]
fn an_expand_of_the_focus_left_reported_after_the_new_focus_is_spoken() {
    // Alt+Down on a Settings combo box: the item's focus, whose ancestor is
    // the combo box already expanded, reaches Core before the combo box's
    // own state change (settings_combo_box_expanded, 2 of 10 runs,
    // 2026-10-30). The change is compared with what was last spoken of the
    // combo box, its collapsed state as the focus.
    let source = Pid(1);
    let collapsed = node(
        50,
        Role::ComboBox,
        Some("Theme"),
        None,
        states(&[State::Collapsed]),
    );
    let (reader, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, collapsed),
    );
    let expanded = node(
        50,
        Role::ComboBox,
        Some("Theme"),
        None,
        states(&[State::Expanded]),
    );
    let item = node(51, Role::ListItem, Some("Dark"), None, StateSet::new());
    let (reader, effects) = reduce(
        &reader,
        &focus_event_with_ancestors(TraceId::mint(), source, item, vec![expanded]),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 51, &[50], None),
            vec![vec![UtteranceSegment::label("Dark")]]
        ),
        "the combo box is not entered anew"
    );
    let (_, effects) = reduce(
        &reader,
        &states_changed_input(
            TraceId::mint(),
            source,
            NodeId::new(50),
            states(&[State::Expanded]),
        ),
    );
    assert_eq!(heard(&effects), vec![queued(vec![state(State::Expanded)])]);
}

#[test]
fn the_same_focus_reported_again_with_other_states_is_silent_and_kept() {
    // Notepad reports its edit control twice as it settles, the second time
    // without one state; NVDA compares the focus by identity.
    let source = Pid(1);
    let first = node(
        40,
        Role::EditableText,
        Some("Text editor"),
        None,
        StateSet::new().with(State::Focusable),
    );
    let (state, _) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, first),
    );
    let again = node(
        40,
        Role::EditableText,
        Some("Text editor"),
        None,
        StateSet::new(),
    );
    let (state, effects) = reduce(&state, &focus_event(TraceId::mint(), source, again));
    assert!(effects.is_empty(), "already the focus");
    assert_eq!(
        state.focused().map(|(_, node)| node.states),
        Some(StateSet::new()),
        "the newer reading is kept"
    );
}

/// `input` with its observation time set to `us`, in microseconds on the
/// run's shared clock.
fn observed_at(mut input: Input, us: u64) -> Input {
    if let Input::Event { observed_at_us, .. } = &mut input {
        *observed_at_us = us;
    }
    input
}

/// The text editor of application 1, node 1 in its outpost.
fn text_editor() -> NodeSnapshot {
    node(
        1,
        Role::EditableText,
        Some("Text editor"),
        None,
        StateSet::new(),
    )
}

/// The repeat hole's arrival order: the editor's focus A at 1,000, its
/// repeat at 1,200, whose reader read the element it held still focused,
/// and then another application's focus B, observed at 1,100 between them.
/// NVDA's in-order queue would hold A, B, A and end on A; the repeat
/// records its observation though it is dropped as a duplicate, so B,
/// observed before it, is stale and silent, and Core stays on A
/// (`docs/parity/focus-events.md`, "Stale focus events").
#[test]
fn a_duplicate_focus_makes_a_focus_observed_before_it_stale() {
    let (state, _) = reduce(
        &SrState::new(),
        &observed_at(focus_in(Pid(1), window(10), text_editor(), vec![]), 1_000),
    );
    let (state, effects) = reduce(
        &state,
        &observed_at(
            focus_in_after(
                Pid(1),
                window(10),
                text_editor(),
                vec![],
                EarlierElement::Focused,
            ),
            1_200,
        ),
    );
    assert!(effects.is_empty(), "the same element, still focused");
    let button = node(1, Role::Button, Some("OK"), None, StateSet::new());
    let (state, effects) = reduce(
        &state,
        &observed_at(focus_in(Pid(2), window(20), button, vec![]), 1_100),
    );
    assert_eq!(heard(&effects), vec![], "observed before the repeat");
    assert_eq!(
        state.focused().map(|(source, node)| (source, node.id)),
        Some((Pid(1), NodeId::in_outpost(OutpostId(1), 1)))
    );
}

/// The repeat hole in the order observed: the editor's focus A at 1,000,
/// another application's focus B at 1,100, and A again at 1,200 with its
/// element still focused. B came between, so A is no duplicate and is
/// spoken, as NVDA's queue ends on A.
#[test]
fn the_repeat_hole_in_the_order_observed_speaks_the_return() {
    let (state, _) = reduce(
        &SrState::new(),
        &observed_at(focus_in(Pid(1), window(10), text_editor(), vec![]), 1_000),
    );
    let button = node(1, Role::Button, Some("OK"), None, StateSet::new());
    let (state, _) = reduce(
        &state,
        &observed_at(focus_in(Pid(2), window(20), button, vec![]), 1_100),
    );
    let (_, effects) = reduce(
        &state,
        &observed_at(
            focus_in_after(
                Pid(1),
                window(10),
                text_editor(),
                vec![],
                EarlierElement::Focused,
            ),
            1_200,
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 1, &[], None)),
            Heard::Stop,
            queued(vec![
                UtteranceSegment::label("Text editor"),
                role(Role::EditableText)
            ]),
        ]
    );
}

/// A foreground report of the window that holds Core's focus is refused,
/// but records its observation: another application's focus observed
/// before it and arriving after it is stale, and Core stays in the window,
/// as NVDA's in-order queue ends on the window's foreground event.
#[test]
fn a_refused_foreground_report_makes_a_focus_observed_before_it_stale() {
    let check_box = node(2, Role::CheckBox, Some("Wrap"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &observed_at(
            focus_in(Pid(1), foreground_window(10), check_box, vec![]),
            1_000,
        ),
    );
    let window_node = node(3, Role::Window, Some("Options"), None, StateSet::new());
    let (state, effects) = reduce(
        &state,
        &observed_at(
            foreground_in(Pid(1), foreground_window(10), window_node),
            2_000,
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![],
        "the focus is already in the window"
    );
    let button = node(1, Role::Button, Some("OK"), None, StateSet::new());
    let (state, effects) = reduce(
        &state,
        &observed_at(focus_in(Pid(2), window(20), button, vec![]), 1_900),
    );
    assert_eq!(
        heard(&effects),
        vec![],
        "observed before the window's report"
    );
    assert_eq!(
        state.focused().map(|(_, node)| node.id),
        Some(NodeId::in_outpost(OutpostId(1), 2))
    );
}

/// An MSAA repeat of the focus Core holds, alive, which its outpost now
/// reads in full: silent, with its newer states kept.
#[test]
fn an_msaa_repeat_of_the_alive_focus_is_silent_and_keeps_its_states() {
    let mut check_box = node(2, Role::CheckBox, Some("Wrap"), None, StateSet::new());
    check_box.backend = Backend::Msaa;
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(Pid(1), window(10), check_box.clone(), vec![]),
    );
    check_box.states = StateSet::new().with(State::Checked);
    let (state, effects) = reduce(&state, &focus_in(Pid(1), window(10), check_box, vec![]));
    assert_eq!(heard(&effects), vec![], "the same object, still the focus");
    assert_eq!(
        state.focused().map(|(_, node)| node.states),
        Some(StateSet::new().with(State::Checked))
    );
}

/// A name change of a node that is no longer Core's focus, which its
/// outpost reads while it has not released the node, is silent.
#[test]
fn a_change_off_the_focus_is_silent() {
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(Pid(1), window(10), text_editor(), vec![]),
    );
    let button = node(2, Role::Button, Some("OK"), None, StateSet::new());
    let (state, _) = reduce(&state, &focus_in(Pid(1), window(10), button, vec![]));
    let (_, effects) = reduce(
        &state,
        &event_in(
            Pid(1),
            Some(window(10)),
            NormalizedEvent::PropertyChanged {
                node_id: NodeId::new(1),
                change: PropertyChange::Name(Some("Notes".to_owned())),
                child_count: None,
            },
        ),
    );
    assert_eq!(heard(&effects), vec![]);
}

/// `input`, a focus report, as the entry of rank `rank` in its outpost's
/// focus group `number`.
fn in_group(mut input: Input, number: u64, rank: u32) -> Input {
    if let Input::Event {
        event: NormalizedEvent::FocusChanged { group, .. },
        ..
    } = &mut input
    {
        *group = Some(FocusGroup { number, rank });
    }
    input
}

/// A focus group of window 10's foreground change and the focus beside it,
/// "Three": with Core's focus a control of window 10, the change is
/// refused, and the older entry is taken and spoken.
#[test]
fn a_groups_older_focus_is_taken_when_its_newer_change_is_refused() {
    let one = node(2, Role::ListItem, Some("One"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(Pid(1), foreground_window(10), one, vec![]),
    );
    let window_node = node(3, Role::Window, Some("Numbers"), None, StateSet::new());
    let (state, effects) = reduce(
        &state,
        &in_group(
            foreground_in(Pid(1), foreground_window(10), window_node),
            1,
            0,
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![],
        "the focus is already in the window"
    );
    let three = node(4, Role::ListItem, Some("Three"), None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &in_group(focus_in(Pid(1), foreground_window(10), three, vec![]), 1, 1),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 4, &[], None),
            vec![vec![UtteranceSegment::label("Three")]]
        )
    );
}

/// The same group with Core's focus in another application: the change is
/// taken and its window said, and the older entry of the group is dropped;
/// another generation's group of the same number is not.
#[test]
fn a_groups_older_focus_is_dropped_once_its_newer_change_is_taken() {
    let other = node(1, Role::Button, Some("OK"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(Pid(2), foreground_window(20), other, vec![]),
    );
    let window_node = node(3, Role::Window, Some("Numbers"), None, StateSet::new());
    let (state, effects) = reduce(
        &state,
        &in_group(
            foreground_in(Pid(1), foreground_window(10), window_node),
            1,
            0,
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 3, &[], Some((OutpostId(1), 3)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Numbers"), role(Role::Window)]),
        ]
    );
    let three = node(4, Role::ListItem, Some("Three"), None, StateSet::new());
    let older = in_group(focus_in(Pid(1), foreground_window(10), three, vec![]), 1, 1);
    let (_, effects) = reduce(&state, &older);
    assert_eq!(heard(&effects), vec![], "the group ended at the change");
    let (_, effects) = reduce_from(&state, &older, OutpostId(5));
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(5), 4, &[], Some((OutpostId(1), 3))),
            vec![vec![UtteranceSegment::label("Three")]]
        ),
        "a replacement generation counts its groups again"
    );
}

/// With Core's focus another outpost's node inside window 10, the
/// window's foreground change is refused, and its outpost ends the group
/// at the change, so no older entry follows it.
#[test]
fn a_groups_change_of_the_window_holding_another_outposts_focus_is_refused() {
    let content = node(1, Role::Button, Some("Apply"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(Pid(2), foreground_window(10), content, vec![]),
    );
    let window_node = node(3, Role::Window, Some("Settings"), None, StateSet::new());
    let (state, effects) = reduce(
        &state,
        &in_group(
            foreground_in(Pid(1), foreground_window(10), window_node),
            1,
            0,
        ),
    );
    assert_eq!(heard(&effects), vec![]);
    assert_eq!(
        state.focused().map(|(source, node)| (source, node.id)),
        Some((Pid(2), NodeId::in_outpost(OutpostId(2), 1)))
    );
}

/// The duplicate rule compares with the last focus Core queued, of any
/// application: once another application's focus came between, a focus
/// on the earlier element is no duplicate and is announced.
#[test]
fn the_same_focus_after_another_applications_focus_is_announced() {
    let edit = node(
        1,
        Role::EditableText,
        Some("Text editor"),
        None,
        StateSet::new(),
    );
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(Pid(1), window(10), edit.clone(), vec![]),
    );
    let button = node(1, Role::Button, Some("OK"), None, StateSet::new());
    let (state, _) = reduce(&state, &focus_in(Pid(2), window(20), button, vec![]));
    let (_, effects) = reduce(&state, &focus_in(Pid(1), window(10), edit, vec![]));
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 1, &[], None)),
            Heard::Stop,
            queued(vec![
                UtteranceSegment::label("Text editor"),
                role(Role::EditableText)
            ]),
        ]
    );
}

/// Speech from elsewhere observed before a foreground change, but reaching
/// Core after it, is dropped, as NVDA's foreground change cancels speech
/// queued before it; the same speech observed after the change is spoken.
#[test]
fn a_toast_observed_before_a_foreground_change_is_dropped() {
    let state = switch_to(&SrState::new(), Pid(3));
    let notepad = node(1000, Role::Window, Some("Notepad"), None, StateSet::new());
    let (state, _) = reduce(
        &state,
        &observed_at(foreground_in(Pid(1), window(1000), notepad), 1_000_600),
    );

    // Both toasts are observed in the foreground change's millisecond.
    let (_, effects) = reduce(
        &state,
        &observed_at(toast_in(Pid(2), window(20)), 1_000_300),
    );
    assert!(effects.is_empty(), "observed before the foreground change");

    let (_, effects) = reduce(
        &state,
        &observed_at(toast_in(Pid(2), window(20)), 1_000_900),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![
            UtteranceSegment::label("Download complete"),
            role(Role::Window)
        ])]
    );
}

#[test]
fn a_focus_observed_before_the_latest_from_another_outpost_is_stale() {
    // Notepad's outpost delivers a focus observed before Verbatim's menu
    // opened only after the menu's focus: NVDA's single queue would have
    // handled it first.
    let menu_item = node(
        1,
        Role::MenuItem,
        Some("Settings..."),
        None,
        StateSet::new(),
    );
    // The menu's window, unnamed, which both its items are in.
    let menu = window_node(3, Backend::Msaa, Role::Window, "", 50);
    let (state, effects) = reduce_from(
        &SrState::new(),
        &observed_at(
            focus_event_with_ancestors(TraceId::mint(), Pid(1), menu_item, vec![menu.clone()]),
            2_000,
        ),
        OutpostId(1),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 1, &[3], None),
            vec![vec![UtteranceSegment::label("Settings...")]]
        )
    );
    let edit = node(
        1,
        Role::EditableText,
        Some("Text editor"),
        None,
        StateSet::new(),
    );
    let late = observed_at(focus_event(TraceId::mint(), Pid(2), edit), 1_990);
    let (_, effects) = reduce_from(&state, &late, OutpostId(2));
    assert!(effects.is_empty(), "observed before the menu's focus");

    // The same outpost keeps its own order, and a focus-now answer has no
    // observation time: neither is ever stale.
    let other_item = node(2, Role::MenuItem, Some("Exit"), None, StateSet::new());
    let (_, effects) = reduce_from(
        &state,
        &observed_at(
            focus_event_with_ancestors(TraceId::mint(), Pid(1), other_item, vec![menu]),
            1_990,
        ),
        OutpostId(1),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 2, &[3], None),
            vec![vec![UtteranceSegment::label("Exit")]]
        )
    );
    let edit = node(
        1,
        Role::EditableText,
        Some("Text editor"),
        None,
        StateSet::new(),
    );
    let (_, effects) = reduce_from(
        &state,
        &focus_event(TraceId::mint(), Pid(2), edit),
        OutpostId(2),
    );
    assert_eq!(
        heard(&effects),
        cut_focus_heard(
            focus_now(OutpostId(2), 1, &[], None),
            vec![vec![
                UtteranceSegment::label("Text editor"),
                role(Role::EditableText)
            ]]
        ),
        "another application's window"
    );
}

/// Two focuses from different applications stamped in one millisecond are
/// ordered by their microseconds. In the Alt+Tab check (2026-10-24, switch
/// 3) the focus listener read the switcher's pane and then the console's
/// terminal in one millisecond, and the switcher's reached Core second. A
/// tie is no earlier, so the later arrival is applied.
#[test]
fn focuses_stamped_in_one_millisecond_are_ordered_by_their_microseconds() {
    let explorer = Pid(6620);
    let console = Pid(12284);
    let switcher = WindowFacts {
        topmost: true,
        ..window(131_700)
    };
    let terminal = node(2, Role::Terminal, None, None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &observed_at(
            focus_in(console, foreground_window(9_373_252), terminal, vec![]),
            27_736_400,
        ),
    );
    let pane = node(1, Role::Pane, None, None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &observed_at(
            focus_in(explorer, switcher.clone(), pane.clone(), vec![]),
            27_736_100,
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![],
        "observed first, by its microseconds"
    );

    let (state, effects) = reduce(
        &state,
        &observed_at(focus_in(explorer, switcher, pane, vec![]), 27_736_400),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(outpost_of(explorer), 1, &[], None)),
            Heard::Stop,
            queued(vec![role(Role::Pane)]),
        ],
        "a tie is no earlier"
    );
    assert_eq!(
        state.focused().map(|(source, node)| (source, node.id)),
        Some((explorer, NodeId::in_outpost(outpost_of(explorer), 1)))
    );
}

/// A focus from another application observed before the newest focus, in
/// the millisecond before it, is stale though its window is topmost, whose
/// focus is otherwise applied when it is not the foreground window.
#[test]
fn a_topmost_focus_observed_before_the_newest_is_stale() {
    let terminal = node(2, Role::Terminal, None, None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &observed_at(
            focus_in(Pid(2), foreground_window(30), terminal, vec![]),
            27_737_000,
        ),
    );
    // A topmost window, as the Alt+Tab switcher's is, whose focus is
    // applied though it is not the foreground window.
    let topmost = WindowFacts {
        topmost: true,
        ..window(40)
    };
    let pane = node(1, Role::Pane, None, None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &observed_at(focus_in(Pid(3), topmost, pane, vec![]), 27_736_900),
    );
    assert_eq!(heard(&effects), vec![], "observed before the newest focus");
}

#[test]
fn a_foreground_report_that_changes_nothing_still_orders_later_arrivals() {
    // Escape closes Verbatim's menu: Notepad's foreground report, observed
    // after the menu's last focus, arrives first; the menu's arrives later
    // and is stale.
    let source = Pid(2);
    let facts = foreground_window(30);
    let client = window_node(2, Backend::Msaa, Role::Pane, "Untitled - Notepad", 30);
    let foreground = |us| Input::Event {
        observed_at_us: us,
        trace_id: TraceId::mint(),
        source,
        backend: Backend::Msaa,
        window: Some(facts.clone()),
        event: NormalizedEvent::FocusChanged {
            node: client.clone(),
            foreground: true,
            ancestors: vec![],
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
        },
    };
    let (state, _) = reduce_from(&SrState::new(), &foreground(900), OutpostId(2));
    let edit = node(
        1,
        Role::EditableText,
        Some("Text editor"),
        None,
        StateSet::new(),
    );
    let (state, effects) = reduce_from(
        &state,
        &observed_at(
            focus_in(source, facts.clone(), edit, vec![client.clone()]),
            1_000,
        ),
        OutpostId(2),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(2), 1, &[2], Some((OutpostId(2), 2))),
            vec![vec![
                UtteranceSegment::label("Text editor"),
                role(Role::EditableText)
            ]]
        ),
        "the edit is spoken"
    );
    let (state, effects) = reduce_from(&state, &foreground(3_000), OutpostId(2));
    assert!(effects.is_empty(), "focus is already in that window");
    let menu_item = node(
        1,
        Role::MenuItem,
        Some("Settings..."),
        None,
        StateSet::new(),
    );
    let (_, effects) = reduce_from(
        &state,
        &observed_at(focus_event(TraceId::mint(), Pid(1), menu_item), 2_900),
        OutpostId(1),
    );
    assert!(
        effects.is_empty(),
        "observed before Notepad's foreground report"
    );
}

/// A focus in window `facts` of application `source`, observed at `us` and
/// delivered by `outpost`, with `ancestors` outermost first.
fn focus_observed(
    (source, outpost): (Pid, OutpostId),
    facts: &WindowFacts,
    (focus, ancestors): (NodeSnapshot, Vec<NodeSnapshot>),
    us: u64,
) -> (Input, OutpostId) {
    (
        observed_at(focus_in(source, facts.clone(), focus, ancestors), us),
        outpost,
    )
}

/// `second_application_and_verbatim_menu`'s switch to Notepad (b1917eed,
/// 2026-10-05): Windows raised Notepad's foreground event while the desktop
/// was still the foreground window, and the desktop's list raised two focus
/// events before Notepad's window was confirmed in front. Those reach Core
/// first; Notepad's foreground report, ordered by the confirmation, is not
/// stale against them, and "Untitled - Notepad" is said.
#[test]
fn notepads_foreground_report_after_the_desktops_last_focuses_is_spoken() {
    let desktop = (Pid(3), OutpostId(3));
    let desktop_window = foreground_window(10);
    let list = node(2, Role::List, Some("Desktop"), None, StateSet::new());
    let item = |id, name| node(id, Role::ListItem, Some(name), None, StateSet::new());
    let mut state = SrState::new();
    for (focus, us) in [(item(1, "Recycle Bin"), 900), (item(3, "This PC"), 1_040)] {
        let (input, outpost) =
            focus_observed(desktop, &desktop_window, (focus, vec![list.clone()]), us);
        state = reduce_from(&state, &input, outpost).0;
    }
    let (input, outpost) = focus_observed(
        desktop,
        &desktop_window,
        (item(1, "Recycle Bin"), vec![list.clone()]),
        1_080,
    );
    let (state, effects) = reduce_from(&state, &input, outpost);
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(3), 1, &[2], Some((OutpostId(3), 2))),
            vec![vec![UtteranceSegment::label("Recycle Bin")]]
        )
    );

    let notepad_window = foreground_window(20);
    let window_node = node(
        1,
        Role::Window,
        Some("Untitled - Notepad"),
        None,
        StateSet::new(),
    );
    let foreground = observed_at(
        foreground_in(Pid(2), notepad_window.clone(), window_node.clone()),
        1_130,
    );
    let (state, effects) = reduce_from(&state, &foreground, OutpostId(2));
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 1, &[], Some((OutpostId(2), 1)))),
            Heard::Stop,
            queued(vec![
                UtteranceSegment::label("Untitled - Notepad"),
                role(Role::Window)
            ]),
        ],
        "the window is said"
    );
    let edit = node(
        2,
        Role::EditableText,
        Some("Text editor"),
        None,
        StateSet::new(),
    );
    let (input, outpost) = focus_observed(
        (Pid(2), OutpostId(2)),
        &notepad_window,
        (edit, vec![window_node]),
        1_150,
    );
    let (_, effects) = reduce_from(&state, &input, outpost);
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(2), 2, &[1], Some((OutpostId(2), 1))),
            vec![vec![
                UtteranceSegment::label("Text editor"),
                role(Role::EditableText)
            ]]
        ),
        "the edit follows"
    );
}

/// A Settings page opened from the desktop: `SystemSettings`' content raises
/// its focus before `ApplicationFrameHost`'s frame, the same top-level
/// window, is confirmed in front, and reaches Core after the frame's
/// foreground report. The content's focus is spoken.
#[test]
fn a_settings_pages_content_focus_observed_before_its_frame_came_in_front_is_spoken() {
    let desktop_item = node(
        1,
        Role::ListItem,
        Some("Recycle Bin"),
        None,
        StateSet::new(),
    );
    let (input, outpost) = focus_observed(
        (Pid(3), OutpostId(3)),
        &foreground_window(10),
        (desktop_item, vec![]),
        900,
    );
    let (state, _) = reduce_from(&SrState::new(), &input, outpost);
    let facts = foreground_window(40);
    let frame = observed_at(
        foreground_in(
            Pid(10),
            facts.clone(),
            node(1, Role::Window, Some("Settings"), None, StateSet::new()),
        ),
        1_130,
    );
    let (state, effects) = reduce_from(&state, &frame, OutpostId(1));
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 1, &[], Some((OutpostId(1), 1)))),
            Heard::Stop,
            queued(vec![
                UtteranceSegment::label("Settings"),
                role(Role::Window)
            ]),
        ],
        "the frame is said"
    );
    let toggle = node(1, Role::CheckBox, Some("Bluetooth"), None, StateSet::new());
    // The page's outpost reads the frame above its process boundary.
    let frame_from_page = window_node(5, Backend::Msaa, Role::Pane, "Settings", 40);
    let (input, outpost) = focus_observed(
        (Pid(11), OutpostId(2)),
        &facts,
        (toggle, vec![frame_from_page]),
        1_040,
    );
    let (_, effects) = reduce_from(&state, &input, outpost);
    assert_eq!(
        heard(&effects),
        focus_heard(
            FocusNow {
                focus: ObjectKey::of_id(NodeId::in_outpost(OutpostId(2), 1)),
                ancestors: vec![ObjectKey::of_id(NodeId::in_outpost(OutpostId(1), 1))],
                foreground: Some(ObjectKey::of_id(NodeId::in_outpost(OutpostId(1), 1))),
                menu_item_of_focus: None,
            },
            vec![vec![
                UtteranceSegment::label("Bluetooth"),
                role(Role::CheckBox),
                not(State::Checked)
            ]]
        ),
        "the content focus is spoken"
    );
}

#[test]
fn an_entered_container_is_spoken_as_a_focus_is() {
    // A named static text entered as context says its name alone; a list,
    // which is not a silent role, still says "list".
    let label = node(20, Role::StaticText, Some("Options"), None, StateSet::new());
    let list = node(21, Role::List, Some("Files"), None, StateSet::new());
    let item = node(22, Role::Button, Some("OK"), None, StateSet::new());
    let (_, effects) = reduce(
        &SrState::new(),
        &focus_event_with_ancestors(TraceId::mint(), Pid(1), item, vec![label, list]),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 22, &[20, 21], None),
            vec![
                vec![UtteranceSegment::label("Options")],
                vec![UtteranceSegment::label("Files"), role(Role::List)],
                vec![UtteranceSegment::label("OK"), role(Role::Button)],
            ]
        )
    );
}

#[test]
fn reporting_the_object_speaks_the_role_and_navigating_to_it_does_not() {
    // Reporting the current object is a query and keeps the role; object
    // navigation speaks the new object as NVDA speaks a focus.
    let source = Pid(1);
    let item = node(10, Role::ListItem, Some("alpha.txt"), None, StateSet::new());
    let state = focused(source, item);
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    assert_eq!(
        said(&effects),
        vec![
            UtteranceSegment::label("alpha.txt"),
            UtteranceSegment::new(SegmentContent::Role(Role::ListItem)),
        ]
    );

    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::NextSibling, 0),
    );
    let query = only_fetch(&effects);
    let next = node(11, Role::ListItem, Some("beta.txt"), None, StateSet::new());
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        kind: query.kind,
        result: FetchResult::Node(next),
    };
    let (_, effects) = reduce(&state, &completion);
    assert_eq!(said(&effects), vec![UtteranceSegment::label("beta.txt")]);
}

#[test]
fn navigate_at_a_tree_edge_speaks_the_edge_message_and_stays_put() {
    let source = Pid(1);
    let root = node(10, Role::Window, Some("App"), None, StateSet::new());
    let state = focused(source, root);

    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = only_fetch(&effects);
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        kind: query.kind,
        result: FetchResult::NoNeighbor,
    };
    let (after, effects) = reduce(&state, &completion);
    assert_eq!(
        said(&effects),
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
    assert_eq!(
        said(&effects),
        vec![UtteranceSegment::label("App"), role(Role::Window)]
    );
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
        let query = only_fetch(&effects);
        let completion = Input::FetchCompleted {
            trace_id: TraceId::mint(),
            query_id: query.query_id,
            kind: query.kind,
            result: FetchResult::NoNeighbor,
        };
        let (_, effects) = reduce(&state, &completion);
        assert_eq!(
            said(&effects),
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
    let query = only_fetch(&effects);

    // A focus event for a different node in the same application arrives
    // before the completion does. Review follows focus, so the navigator
    // snaps to it, but this must not discard the user's still-pending
    // navigation.
    let elsewhere = node(20, Role::Button, Some("Cancel"), None, StateSet::new());
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), source, elsewhere));

    let parent = node(11, Role::Group, Some("Buttons"), None, StateSet::new());
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        kind: query.kind,
        result: FetchResult::Node(parent),
    };
    let (_, effects) = reduce(&state, &completion);
    assert_eq!(
        said(&effects),
        vec![UtteranceSegment::label("Buttons"), role(Role::Group)],
        "a navigation completion must still land after an intervening focus event"
    );
}

#[test]
fn a_second_navigation_supersedes_the_first_pending_one() {
    let source = Pid(1);
    let button = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);

    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let first_query = only_fetch(&effects);

    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::NextSibling, 0),
    );
    let second_query = only_fetch(&effects);

    // The first (now stale) completion is dropped.
    let stale_parent = node(11, Role::Group, Some("Buttons"), None, StateSet::new());
    let stale_completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: first_query.query_id,
        kind: first_query.kind,
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
        kind: second_query.kind,
        result: FetchResult::Node(sibling),
    };
    let (_, effects) = reduce(&state, &completion);
    assert_eq!(
        said(&effects),
        vec![UtteranceSegment::label("Cancel"), role(Role::Button)]
    );
}

#[test]
fn to_focus_after_a_navigation_drops_its_late_completion() {
    let source = Pid(1);
    let button = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);

    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = only_fetch(&effects);

    // The user explicitly returns to focus before the navigation's
    // completion arrives; that explicit intent must win.
    let (state, _) = reduce(&state, &command(TraceId::mint(), ReviewCommand::ToFocus, 0));

    let parent = node(11, Role::Group, Some("Buttons"), None, StateSet::new());
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        kind: query.kind,
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
    let query = only_fetch(&effects);

    // The outpost could not re-acquire the navigator's node: distinct from
    // a tree edge, so this must not stay silent. It falls back to
    // announcing whatever is currently focused.
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        kind: query.kind,
        result: FetchResult::Gone,
    };
    let (_, effects) = reduce(&state, &completion);
    assert_eq!(
        said(&effects),
        vec![UtteranceSegment::label("OK"), role(Role::Button)]
    );
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
    assert_eq!(
        effects,
        vec![Effect::Activate {
            node_id: NodeId::in_outpost(outpost_of(source), 10)
        }]
    );
}

#[test]
fn review_cursor_walks_lines_words_and_characters() {
    let source = Pid(1);
    // Static text has no text interface, so its value is walked as flat
    // text.
    let edit = node(
        10,
        Role::StaticText,
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
    assert_eq!(said(&effects), vec![UtteranceSegment::text("first line")]);

    // Next line.
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewNextLine, 0),
    );
    assert_eq!(said(&effects), vec![UtteranceSegment::text("second line")]);

    // Next line at the bottom: says "Bottom", stays put, re-reads.
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewNextLine, 0),
    );
    assert_eq!(
        said(&effects),
        vec![
            UtteranceSegment::new(SegmentContent::Message(verbatim_model::Message::Bottom)),
            UtteranceSegment::text("second line"),
        ]
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
    assert_eq!(said(&effects), vec![UtteranceSegment::text("first")]);
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewNextWord, 0),
    );
    assert_eq!(said(&effects), vec![UtteranceSegment::text("line")]);

    // First character of the current position ("line" -> 'l').
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(said(&effects), vec![UtteranceSegment::text("l")]);
}

#[test]
fn navigator_follows_focus_and_returns_to_focus() {
    let source = Pid(1);
    let first = node(10, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, first);

    // Move the navigator to the parent.
    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = only_fetch(&effects);
    let parent = node(11, Role::Group, Some("Group"), None, StateSet::new());
    let (state, _) = reduce(
        &state,
        &Input::FetchCompleted {
            trace_id: TraceId::mint(),
            query_id: query.query_id,
            kind: query.kind,
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
    let (state, _) = reduce(&state, &focus_event(TraceId::mint(), source, second));
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    // An edit field says its name and role, and its text follows from a
    // read of the selection in place of its value.
    let read_selection = |query_id| {
        Heard::Other(Effect::Text(verbatim_model::TextRequest {
            query_id: QueryId(query_id),
            node_id: NodeId::in_outpost(outpost_of(source), 20),
            op: verbatim_model::TextOp::ReadRange {
                start: verbatim_model::TextPoint::SelectionStart,
                end: verbatim_model::TextPoint::SelectionEnd,
                through: None,
            },
        }))
    };
    assert_eq!(
        heard(&effects),
        vec![
            queued(vec![
                UtteranceSegment::label("Field"),
                role(Role::EditableText),
            ]),
            read_selection(1),
        ],
        "the navigator followed focus to the new control"
    );

    // Explicit "to focus" also reports the focused control after wandering.
    let (state, _) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let (_, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::ToFocus, 0));
    assert_eq!(
        heard(&effects),
        vec![
            queued(vec![
                message(verbatim_model::Message::MoveToFocus),
                UtteranceSegment::label("Field"),
                role(Role::EditableText),
            ]),
            // The parent fetch in between took query 2.
            read_selection(3),
        ],
        "to-focus says \"Move to focus\" and snaps the navigator back"
    );
}

#[test]
fn commands_with_no_navigator_yet_say_so() {
    let state = SrState::new();
    let no_navigator = vec![UtteranceSegment::new(SegmentContent::Message(
        verbatim_model::Message::NoNavigatorObject,
    ))];
    let (_, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    assert_eq!(said(&effects), no_navigator);
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    assert_eq!(said(&effects), no_navigator);
}

// ---- Windows, menus, and name changes ----

#[test]
fn a_foreground_change_into_the_focused_window_is_ignored() {
    let source = Pid(1);
    let edit = node(2, Role::EditableText, Some("Text"), None, StateSet::new());
    let (state, _) = reduce(&SrState::new(), &focus_in(source, window(10), edit, vec![]));

    let window_node = node(1, Role::Window, Some("Notepad"), None, StateSet::new());
    let (state, effects) = reduce(&state, &foreground_in(source, window(10), window_node));

    assert!(effects.is_empty(), "focus is already inside that window");
    assert_eq!(
        state.focused().map(|(_, node)| node.name.clone()),
        Some(Some("Text".to_owned()))
    );
}

#[test]
fn a_foreground_change_to_another_window_announces_the_window_as_the_focus() {
    let source = Pid(1);
    let edit = node(2, Role::EditableText, Some("Text"), None, StateSet::new());
    let (state, _) = reduce(&SrState::new(), &focus_in(source, window(10), edit, vec![]));

    let other = node(3, Role::Window, Some("Find"), None, StateSet::new());
    let (state, effects) = reduce(&state, &foreground_in(source, window(20), other));

    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 3, &[], Some((OutpostId(1), 3)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Find"), role(Role::Window)]),
        ]
    );
    assert_eq!(
        state.focused().map(|(_, node)| node.role),
        Some(Role::Window)
    );
}

#[test]
fn a_window_spoken_as_the_focus_is_not_repeated_when_its_control_takes_focus() {
    let source = Pid(1);
    // A foreground report is read through MSAA.
    let mut notepad = node(1, Role::Window, Some("Notepad"), None, StateSet::new());
    notepad.backend = Backend::Msaa;
    let (state, _) = reduce(&SrState::new(), &foreground_in(source, window(10), notepad));

    // The control's ancestry reaches the same window through another path,
    // so its node id differs; the window's handle is where the two lines
    // meet, and the line keeps its entry.
    let same_window = window_node(5, Backend::Uia, Role::Window, "Notepad", 10);
    let edit = node(2, Role::EditableText, Some("Text"), None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &focus_in(source, foreground_window(10), edit, vec![same_window]),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 2, &[1], Some((OutpostId(1), 1))),
            vec![vec![
                UtteranceSegment::label("Text"),
                role(Role::EditableText)
            ]]
        )
    );
}

#[test]
fn a_list_replaced_by_a_new_one_of_the_same_name_is_entered_anew() {
    // File Explorer opening a subfolder: the items view list is a new UI
    // Automation element with the old one's role and name, in the same
    // window, read by the same outpost.
    let source = Pid(1);
    let window_node = node(1, Role::Window, Some("Folder"), None, StateSet::new());
    let list = |id| node(id, Role::List, Some("Items View"), None, StateSet::new());
    let item = |id, name| node(id, Role::ListItem, Some(name), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(
            source,
            foreground_window(10),
            item(3, "Inner"),
            vec![window_node.clone(), list(2)],
        ),
    );

    let (_, effects) = reduce(
        &state,
        &focus_in(
            source,
            foreground_window(10),
            item(5, "delta"),
            vec![window_node, list(4)],
        ),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 5, &[1, 4], Some((OutpostId(1), 1))),
            vec![
                vec![UtteranceSegment::label("Items View"), role(Role::List)],
                vec![UtteranceSegment::label("delta")],
            ]
        )
    );
}

#[test]
fn the_same_window_reported_by_another_outpost_is_not_reannounced() {
    // A Settings page: the frame window belongs to one process, the page to
    // another, both inside one top-level window.
    let frame_host = Pid(1);
    let settings = Pid(2);
    let frame = node(1, Role::Window, Some("Settings"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &foreground_in(frame_host, window(10), frame),
    );

    let frame_seen_from_page = window_node(1, Backend::Uia, Role::Window, "Settings", 10);
    let group = node(2, Role::Group, Some("Display"), None, StateSet::new());
    let toggle = node(
        3,
        Role::ToggleButton,
        Some("Night light"),
        None,
        StateSet::new(),
    );
    let (_, effects) = reduce(
        &state,
        &focus_in(
            settings,
            foreground_window(10),
            toggle,
            vec![frame_seen_from_page, group],
        ),
    );

    assert_eq!(
        heard(&effects),
        focus_heard(
            FocusNow {
                focus: ObjectKey::of_id(NodeId::in_outpost(OutpostId(2), 3)),
                // The line keeps the frame host's entry for the frame.
                ancestors: vec![
                    ObjectKey::of_id(NodeId::in_outpost(OutpostId(1), 1)),
                    ObjectKey::of_id(NodeId::in_outpost(OutpostId(2), 2)),
                ],
                foreground: Some(ObjectKey::of_id(NodeId::in_outpost(OutpostId(1), 1))),
                menu_item_of_focus: None,
            },
            vec![
                vec![UtteranceSegment::label("Display"), role(Role::Group)],
                vec![
                    UtteranceSegment::label("Night light"),
                    role(Role::ToggleButton),
                    not(State::Pressed),
                ],
            ]
        ),
        "the frame window is not spoken a second time"
    );
}

/// The utterances `inputs` speak, fed in order from a fresh state, each as
/// its segments: what is heard, whatever else the inputs do.
fn spoken_by(inputs: &[Input]) -> Vec<Vec<UtteranceSegment>> {
    let mut state = SrState::new();
    let mut spoken = Vec::new();
    for input in inputs {
        let (next, effects) = reduce(&state, input);
        state = next;
        spoken.extend(effects.into_iter().filter_map(|effect| match effect {
            Effect::Speak(utterance) => Some(utterance.segments),
            _ => None,
        }));
    }
    spoken
}

#[test]
fn a_settings_page_is_announced_before_its_focus_whichever_outpost_reports_first() {
    // The Settings app: the frame window is `ApplicationFrameHost`'s, read
    // through MSAA as its client area, a pane carrying the frame's handle,
    // and the page and its focus are `SystemSettings`'s, whose focus
    // reaches the frame through its ancestry, as NVDA's walk does. Either
    // outpost can reach Core first: the frame's foreground report, or the
    // page's focus.
    let frame_host = Pid(1);
    let settings = Pid(2);
    let frame = |id| {
        let mut frame = window_node(id, Backend::Msaa, Role::Pane, "Settings", 10);
        frame.details.top_level_client = true;
        frame
    };
    let from_frame_host = foreground_in(frame_host, foreground_window(10), frame(1));
    let search = || {
        focus_in(
            settings,
            foreground_window(10),
            node(
                3,
                Role::EditableText,
                Some("Search box"),
                None,
                StateSet::new(),
            ),
            vec![
                frame(4),
                node(2, Role::Window, Some("Settings"), None, StateSet::new()),
            ],
        )
    };
    let expected = vec![
        vec![UtteranceSegment::label("Settings")],
        vec![UtteranceSegment::label("Settings"), role(Role::Window)],
        vec![
            UtteranceSegment::label("Search box"),
            role(Role::EditableText),
        ],
    ];
    assert_eq!(
        spoken_by(&[from_frame_host.clone(), search()]),
        expected,
        "the frame host's report first: the page's focus does not repeat the frame"
    );
    assert_eq!(
        spoken_by(&[search(), from_frame_host]),
        expected,
        "the page's focus first: it announces the frame, whose report then says nothing"
    );
}

/// The node `id` of `role`, named `name`, that is window `handle`.
fn window_node(id: u64, backend: Backend, role: Role, name: &str, handle: u64) -> NodeSnapshot {
    let mut window = node(id, role, Some(name), None, StateSet::new());
    window.backend = backend;
    window.details.window = Some(WindowHandle(handle));
    window
}

/// The Settings app as its readers report it: the page's outpost reads the
/// frame, above its process boundary, as NVDA's MSAA client for it, a pane
/// marked a top-level window's client and named by its title, carrying
/// the frame's handle, which the frame host's own focus, the frame's
/// client area, carries too. The pane is the frame that focus entered, so
/// the page's focus says the content window and the focus, as NVDA says
/// "Settings" and then "Settings window" (`docs/parity/focus-context.md`,
/// "Focus-ancestry context").
#[test]
fn a_frame_read_by_two_outposts_is_entered_once() {
    let mut client = window_node(1, Backend::Msaa, Role::Pane, "Settings", 10);
    client.details.top_level_client = true;
    let mut frame = window_node(4, Backend::Msaa, Role::Pane, "Settings", 10);
    frame.details.top_level_client = true;
    let content = window_node(2, Backend::Uia, Role::Window, "Settings", 11);
    let search = node(
        3,
        Role::EditableText,
        Some("Search box"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        spoken_by(&[
            foreground_in(Pid(1), foreground_window(10), client),
            focus_in(Pid(2), foreground_window(10), search, vec![frame, content]),
        ]),
        vec![
            vec![UtteranceSegment::label("Settings")],
            vec![UtteranceSegment::label("Settings"), role(Role::Window)],
            vec![
                UtteranceSegment::label("Search box"),
                role(Role::EditableText),
            ],
        ]
    );
}

/// The Settings frame's own focus, its MSAA client in window 10 as the
/// frame host's outpost reports it on a return by Alt+Tab, keeps its
/// speech when the page's outpost then reports the content's focus with its
/// own node for that window at the top of its chain: the two nodes are one
/// object by their window, as NVDA's `==` finds its one object for the
/// frame (`docs/parity/focus-events.md`, "When speech is cut off, and
/// cancellation of expired focus speech").
#[test]
fn a_frame_s_focus_speech_survives_the_content_s_focus() {
    let mut client = window_node(1, Backend::Msaa, Role::Pane, "Settings", 10);
    client.details.top_level_client = true;
    let mut frame = window_node(4, Backend::Msaa, Role::Pane, "Settings", 10);
    frame.details.top_level_client = true;
    let content = window_node(2, Backend::Uia, Role::Window, "Settings", 11);
    let search = node(
        3,
        Role::EditableText,
        Some("Search box"),
        None,
        StateSet::new(),
    );
    let (state, said) = reduce(
        &SrState::new(),
        &focus_in(Pid(1), foreground_window(10), client, Vec::new()),
    );
    let settings = only_speech(&said[1..]);
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(2), foreground_window(10), search, vec![frame, content]),
    );
    let Some(Effect::DropExpiredSpeech(now)) = effects.first() else {
        panic!("the speech manager is told where the focus is first: {effects:?}");
    };
    assert_eq!(
        settings
            .iter()
            .map(|utterance| utterance.validity.map(|validity| validity.holds(now)))
            .collect::<Vec<_>>(),
        [Some(true)],
        "the frame's \"Settings\" is still worth hearing"
    );
}

/// A window's focus speech, reported by one outpost, holds while another
/// outpost reports the same window as the focus under a node of its own:
/// the new focus meets the old one by its window, so the old node is not
/// among the focus's ancestors, and only the identity rule keeps the speech
/// (`docs/parity/focus-events.md`, "When speech is cut off, and
/// cancellation of expired focus speech").
#[test]
fn a_window_s_focus_speech_holds_while_another_outpost_reports_it_as_the_focus() {
    let mut client = window_node(1, Backend::Msaa, Role::Pane, "Settings", 10);
    client.details.top_level_client = true;
    let mut frame = window_node(4, Backend::Msaa, Role::Pane, "Settings", 10);
    frame.details.top_level_client = true;
    let (state, said) = reduce(
        &SrState::new(),
        &focus_in(Pid(1), foreground_window(10), client, Vec::new()),
    );
    let settings = only_speech(&said[1..]);
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(2), foreground_window(10), frame, Vec::new()),
    );
    let Some(Effect::DropExpiredSpeech(now)) = effects.first() else {
        panic!("the speech manager is told where the focus is first: {effects:?}");
    };
    assert_eq!(
        settings
            .iter()
            .map(|utterance| utterance.validity.map(|validity| validity.holds(now)))
            .collect::<Vec<_>>(),
        [Some(true)],
        "the first outpost's \"Settings\" is still worth hearing"
    );
}

/// A window above the process boundary with no system menu, a host's
/// child window between its top-level window and the focus's own window,
/// is read as NVDA's MSAA client for it, a pane not marked a top-level
/// window's client, which is structure and never spoken, named or not,
/// while the top-level window's client above it is spoken by its title
/// (`docs/parity/focus-context.md`, "Top-level windows in the ancestry").
#[test]
fn a_child_window_above_the_process_boundary_is_not_spoken() {
    let mut frame = window_node(1, Backend::Msaa, Role::Pane, "Host", 10);
    frame.details.top_level_client = true;
    let between = window_node(2, Backend::Msaa, Role::Pane, "Island site", 12);
    let mut ok = node(3, Role::Button, Some("OK"), None, StateSet::new());
    ok.backend = Backend::Uia;
    assert_eq!(
        spoken_by(&[focus_in(
            Pid(2),
            foreground_window(10),
            ok,
            vec![frame, between]
        )]),
        vec![
            vec![UtteranceSegment::label("Host")],
            vec![UtteranceSegment::label("OK"), role(Role::Button)],
        ]
    );
}

/// Two windows are two containers whatever their role and name: a focus in
/// a second "Remove" dialog, another window in the same top-level window,
/// enters it (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn two_windows_with_one_role_and_name_are_two_containers() {
    let first = window_node(1, Backend::Msaa, Role::Dialog, "Remove", 20);
    let second = window_node(3, Backend::Msaa, Role::Dialog, "Remove", 21);
    let button = |id, name| node(id, Role::Button, Some(name), None, StateSet::new());
    let mut ok = button(2, "OK");
    ok.backend = Backend::Msaa;
    let mut yes = button(4, "Yes");
    yes.backend = Backend::Msaa;
    let dialog = vec![UtteranceSegment::label("Remove"), role(Role::Dialog)];
    assert_eq!(
        spoken_by(&[
            focus_in(Pid(1), foreground_window(10), ok, vec![first]),
            focus_in(Pid(1), foreground_window(10), yes, vec![second]),
        ]),
        vec![
            dialog.clone(),
            vec![UtteranceSegment::label("OK"), role(Role::Button)],
            dialog,
            vec![UtteranceSegment::label("Yes"), role(Role::Button)],
        ]
    );
}

/// The focus Core holds, as it tells the outposts, carries the handle of
/// each node that is a window, so that an outpost judges a dialog newly
/// entered as Core does (`docs/parity/focus-context.md`, "Focus-ancestry
/// context").
#[test]
fn the_focus_core_holds_carries_its_windows_handles() {
    let dialog = window_node(1, Backend::Msaa, Role::Dialog, "Remove", 20);
    let button = node(2, Role::Button, Some("OK"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(Pid(1), foreground_window(7), button, vec![dialog]),
    );
    let held = state.held_focus().expect("Core holds the focus");
    assert_eq!(
        (
            held.ancestors
                .iter()
                .map(|node| node.window)
                .collect::<Vec<_>>(),
            held.focus.window
        ),
        (vec![Some(WindowHandle(20))], None)
    );
}

#[test]
fn a_name_change_on_a_focus_ancestor_is_silent() {
    let source = Pid(1);
    let window_node = node(1, Role::Window, None, None, StateSet::new());
    let edit = node(2, Role::EditableText, Some("Text"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(source, foreground_window(10), edit, vec![window_node]),
    );

    let (_, effects) = reduce(
        &state,
        &event_in(
            source,
            Some(window(10)),
            NormalizedEvent::PropertyChanged {
                node_id: NodeId::new(1),
                change: PropertyChange::Name(Some("Untitled - Notepad".to_owned())),
                child_count: None,
            },
        ),
    );

    assert!(
        effects.is_empty(),
        "a window nameless when focus entered it is not announced later"
    );
}

#[test]
fn entering_menus_is_silent_and_only_the_item_is_announced() {
    let menu_bar = node(1, Role::MenuBar, Some("Application"), None, StateSet::new());
    let menu = node(2, Role::Menu, Some("File"), None, StateSet::new());
    let item = node(3, Role::MenuItem, Some("Open"), None, StateSet::new());

    let (_, effects) = reduce(
        &SrState::new(),
        &focus_event_with_ancestors(TraceId::mint(), Pid(1), item, vec![menu_bar, menu]),
    );

    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 3, &[1, 2], None)),
            Heard::Stop,
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Open")]),
        ],
        "entering each menu cancels speech, and only the item is announced"
    );
}

/// A container entered outside a menu in the same focus change is cut off
/// as the menu is entered, as NVDA cancels in the menu's place among the
/// entered containers: Windows 11 Notepad's File menu queues its popup
/// window and cancels it (`docs/parity/changes-and-selection.md`, "Entered menus").
#[test]
fn a_container_entered_before_a_menu_is_cut_off_by_it() {
    let popup = node(1, Role::Window, Some("Popup"), None, StateSet::new());
    let menu = node(2, Role::Menu, None, None, StateSet::new());
    let item = node(3, Role::MenuItem, Some("New tab"), None, StateSet::new());

    let (_, effects) = reduce(
        &SrState::new(),
        &focus_event_with_ancestors(TraceId::mint(), Pid(1), item, vec![popup, menu]),
    );

    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 3, &[1, 2], None)),
            queued(vec![UtteranceSegment::label("Popup"), role(Role::Window)]),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("New tab")]),
        ]
    );
}

// ---- Attention (decision D14) ----

#[test]
fn a_focus_from_a_window_outside_attention_is_dropped() {
    let state = switch_to(&SrState::new(), Pid(1));
    let button = node(2, Role::Button, Some("OK"), None, StateSet::new());

    let (state, effects) = reduce(&state, &focus_in(Pid(2), window(20), button, vec![]));

    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
    assert_eq!(state.attention(), Some(Pid(1)));
    assert_eq!(state.focused().map(|(pid, _)| pid), Some(Pid(1)));
}

#[test]
fn a_focus_read_after_its_window_lost_the_foreground_is_dropped() {
    // The desktop holds attention; Notepad has become the foreground, but
    // its report has not reached Core yet when a desktop focus, read after
    // the change, arrives. NVDA judges a focus against the real foreground
    // window, so it is dropped although the attention record still names
    // the desktop's window (D14, amended 2026-10-05).
    let source = Pid(1);
    let desktop = window_node(1, Backend::Uia, Role::Pane, "Program Manager", 10);
    let (state, _) = reduce(
        &SrState::new(),
        &foreground_in(source, foreground_window(10), desktop.clone()),
    );
    let item = node(
        2,
        Role::ListItem,
        Some("Recycle Bin"),
        None,
        StateSet::new(),
    );

    let (state, effects) = reduce(
        &state,
        &focus_in(source, window(10), item.clone(), vec![desktop.clone()]),
    );
    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
    assert_eq!(state.attention(), Some(source));

    let (_, effects) = reduce(
        &state,
        &focus_in(source, foreground_window(10), item, vec![desktop]),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 2, &[1], Some((OutpostId(1), 1))),
            vec![vec![UtteranceSegment::label("Recycle Bin")]]
        ),
        "read while its window was the foreground"
    );
}

#[test]
fn topmost_shared_owner_and_active_uwp_windows_are_attended() {
    let attended = [
        WindowFacts {
            topmost: true,
            ..window(20)
        },
        // A window sharing the foreground window's root owner: its outpost
        // reads it as in the foreground.
        WindowFacts {
            root_owner: WindowHandle(1000),
            ..foreground_window(30)
        },
        WindowFacts {
            under_active_window: Some(true),
            ..window(40)
        },
    ];
    for facts in attended {
        let state = switch_to(&SrState::new(), Pid(1));
        let button = node(2, Role::Button, Some("OK"), None, StateSet::new());
        let (_, effects) = reduce(&state, &focus_in(Pid(2), facts.clone(), button, vec![]));
        assert_eq!(
            heard(&effects),
            vec![
                Heard::Expire(focus_now(OutpostId(2), 2, &[], Some((OutpostId(1), 1000)))),
                Heard::Stop,
                queued(vec![UtteranceSegment::label("OK"), role(Role::Button)]),
            ],
            "attended: {facts:?}"
        );
    }
}

#[test]
fn without_window_facts_the_application_decides() {
    let state = switch_to(&SrState::new(), Pid(1));
    let button = node(2, Role::Button, Some("OK"), None, StateSet::new());

    let (_, effects) = reduce(
        &state,
        &focus_event(TraceId::mint(), Pid(2), button.clone()),
    );
    assert!(effects.is_empty(), "another application without facts");

    let (_, effects) = reduce(
        &state,
        &focus_event_with_ancestors(
            TraceId::mint(),
            Pid(1),
            button,
            vec![window_node(1000, Backend::Uia, Role::Window, "App", 1000)],
        ),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 2, &[1000], Some((OutpostId(1), 1000))),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        ),
        "the attention application without facts"
    );
}

#[test]
fn a_foreground_change_is_always_accepted_and_moves_attention() {
    let state = switch_to(&SrState::new(), Pid(1));
    let other = node(2, Role::Window, Some("Calculator"), None, StateSet::new());

    let (state, effects) = reduce(&state, &foreground_in(Pid(2), window(20), other));
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 2, &[], Some((OutpostId(2), 2)))),
            Heard::Stop,
            queued(vec![
                UtteranceSegment::label("Calculator"),
                role(Role::Window)
            ]),
        ]
    );
    assert_eq!(state.attention(), Some(Pid(2)));

    // The previous application is now in the background.
    let button = node(3, Role::Button, Some("OK"), None, StateSet::new());
    let (_, effects) = reduce(&state, &focus_in(Pid(1), window(1000), button, vec![]));
    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
}

fn notification_in(source: Pid, activity_id: Option<&str>) -> Input {
    event_in(
        source,
        None,
        NormalizedEvent::Notification {
            node_id: NodeId::new(1),
            notification: verbatim_model::Notification {
                kind: verbatim_model::NotificationKind::Other,
                processing: verbatim_model::NotificationProcessing::MostRecent,
                display_string: Some("Snapped".to_owned()),
                activity_id: activity_id.map(str::to_owned),
            },
        },
    )
}

#[test]
fn notifications_are_spoken_only_from_the_focus_application() {
    // As in Settings: the frame's application holds attention, and the
    // focus is in another application's content inside that window.
    let state = switch_to(&SrState::new(), Pid(1));
    let toggle = node(2001, Role::Button, Some("Wi-Fi"), None, StateSet::new());
    let (state, _) = reduce(
        &state,
        &focus_in(Pid(2), foreground_window(1000), toggle, vec![]),
    );

    let (_, effects) = reduce(&state, &notification_in(Pid(2), None));
    assert_eq!(
        heard(&effects),
        vec![Heard::Say(
            SpeechPriority::Interrupt,
            vec![UtteranceSegment::text("Snapped")]
        )],
        "the focus's application"
    );

    let (_, effects) = reduce(&state, &notification_in(Pid(1), None));
    assert!(
        effects.is_empty(),
        "the attended application without the focus"
    );
}

#[test]
fn with_no_focus_notifications_are_spoken_from_the_attention_application() {
    let state = switch_to(&SrState::new(), Pid(1));

    let (_, effects) = reduce(&state, &notification_in(Pid(2), None));
    assert!(
        effects.is_empty(),
        "a background application's notification"
    );

    let (_, effects) = reduce(&state, &notification_in(Pid(1), None));
    assert_eq!(
        heard(&effects),
        vec![Heard::Say(
            SpeechPriority::Interrupt,
            vec![UtteranceSegment::text("Snapped")]
        )]
    );
}

#[test]
fn the_snap_results_notification_is_spoken_from_anywhere_queued() {
    let state = switch_to(&SrState::new(), Pid(1));

    let (next, effects) = reduce(
        &state,
        &notification_in(
            Pid(2),
            Some("Windows.Shell.SnapComponent.SnapHotKeyResults"),
        ),
    );

    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::text("Snapped")])]
    );
    assert_eq!(
        next.attention(),
        Some(Pid(1)),
        "background never moves attention"
    );

    // Queued even from the attended application, which asks for the
    // notification to supersede earlier speech.
    let (_, effects) = reduce(
        &state,
        &notification_in(
            Pid(1),
            Some("Windows.Shell.SnapComponent.SnapHotKeyResults"),
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::text("Snapped")])]
    );
}

// ---- Outpost replacement ----

/// Feeds `input` as though it arrived on the pipe of outpost `outpost`.
fn reduce_from(state: &SrState, input: &Input, outpost: OutpostId) -> (SrState, Vec<Effect>) {
    let mut input = input.clone();
    if let Input::Event { event, .. } = &mut input {
        event.assign_outpost(outpost);
    }
    let mut next = state.clone();
    let effects = verbatim_core::reduce(&mut next, &input);
    (next, effects)
}

fn ended(outpost: OutpostId) -> Input {
    Input::OutpostEnded { outpost }
}

#[test]
fn an_ended_outposts_focus_is_dead_and_navigation_does_nothing() {
    let source = Pid(1);
    let button = node(5, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);

    let (state, effects) = reduce(&state, &ended(outpost_of(source)));
    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
    assert!(state.focused().is_none());
    assert!(state.held_nodes().is_empty());

    // The navigator was cleared with the outpost, so navigator commands
    // say so, as NVDA does; returning to a dead focus does nothing.
    for cmd in [
        ReviewCommand::Parent,
        ReviewCommand::ReportObject,
        ReviewCommand::Activate,
    ] {
        let (_, effects) = reduce(&state, &command(TraceId::mint(), cmd, 0));
        assert_eq!(
            said(&effects),
            vec![UtteranceSegment::new(SegmentContent::Message(
                verbatim_model::Message::NoNavigatorObject
            ))],
            "{cmd:?} after the outpost ended"
        );
    }
    let (_, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::ToFocus, 0));
    assert!(effects.is_empty(), "to focus after the outpost ended");
}

#[test]
fn a_pending_navigation_to_an_ended_outpost_is_dropped() {
    let source = Pid(1);
    let button = node(5, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);
    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = only_fetch(&effects);

    let (state, _) = reduce(&state, &ended(outpost_of(source)));
    let completion = Input::FetchCompleted {
        trace_id: TraceId::mint(),
        query_id: query.query_id,
        kind: query.kind,
        result: FetchResult::Node(node(6, Role::Group, Some("Buttons"), None, StateSet::new())),
    };
    let (_, effects) = reduce(&state, &completion);
    assert_eq!(effects, [] as [verbatim_model::Effect; 0]);
}

#[test]
fn a_replacement_outpost_reporting_the_same_focus_is_taken_silently() {
    let source = Pid(1);
    let dialog = node(4, Role::Dialog, Some("Save"), None, StateSet::new());
    let button = node(5, Role::Button, Some("OK"), None, StateSet::new());
    let report = focus_event_with_ancestors(TraceId::mint(), source, button, vec![dialog]);
    let (state, _) = reduce_from(&SrState::new(), &report, OutpostId(1));
    let (state, _) = reduce(&state, &ended(OutpostId(1)));

    // The replacement numbers its nodes afresh.
    let dialog = node(1, Role::Dialog, Some("Save"), None, StateSet::new());
    let button = node(2, Role::Button, Some("OK"), None, StateSet::new());
    let report = observed_at(
        focus_event_with_ancestors(TraceId::mint(), source, button, vec![dialog]),
        3_000,
    );
    let (state, effects) = reduce_from(&state, &report, OutpostId(2));

    assert!(effects.is_empty(), "the user already heard this focus");
    assert_eq!(
        state.focused().map(|(_, node)| node.id),
        Some(NodeId::in_outpost(OutpostId(2), 2))
    );
    assert_eq!(
        state.latest_focus_observed_at(),
        Some(3_000),
        "the re-read is the newest focus taken"
    );
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::Activate, 0),
    );
    assert_eq!(
        effects,
        vec![Effect::Activate {
            node_id: NodeId::in_outpost(OutpostId(2), 2)
        }],
        "the navigator follows the re-read focus"
    );
}

/// A replacement outpost's first focus that is not a silent re-read meets
/// the dead outpost's line at its first window, by its handle: the dialog
/// that is its window's client is not said again, and the windowless
/// group below it is, under the replacement's new ids (Dickson accepted
/// re-announcing; NVDA has no counterpart).
#[test]
fn a_replacement_outpost_reporting_a_different_focus_announces_it() {
    let source = Pid(1);
    let general = |id| node(id, Role::Group, Some("General"), None, StateSet::new());
    let button = node(5, Role::Button, Some("OK"), None, StateSet::new());
    let (state, _) = reduce_from(
        &SrState::new(),
        &focus_in(
            source,
            window(10),
            button,
            vec![
                window_node(2, Backend::Msaa, Role::Dialog, "Options", 10),
                general(3),
            ],
        ),
        OutpostId(1),
    );
    let (state, _) = reduce(&state, &ended(OutpostId(1)));

    let other = node(9, Role::Button, Some("Cancel"), None, StateSet::new());
    let (_, effects) = reduce_from(
        &state,
        &focus_in(
            source,
            window(10),
            other,
            vec![
                window_node(7, Backend::Msaa, Role::Dialog, "Options", 10),
                general(8),
            ],
        ),
        OutpostId(2),
    );
    let key = |outpost, id| ObjectKey::of_id(NodeId::in_outpost(OutpostId(outpost), id));
    assert_eq!(
        heard(&effects),
        focus_heard(
            FocusNow {
                focus: key(2, 9),
                ancestors: vec![key(1, 2), key(2, 8)],
                foreground: None,
                menu_item_of_focus: None,
            },
            vec![
                vec![UtteranceSegment::label("General"), role(Role::Group)],
                vec![UtteranceSegment::label("Cancel"), role(Role::Button)],
            ]
        )
    );
}

#[test]
fn the_same_node_number_from_another_outpost_never_reaches_the_focus() {
    // A successor, or any other outpost, issues the same number for an
    // unrelated element; only the outpost stamp tells them apart.
    let source = Pid(1);
    let slider = node(500, Role::Slider, Some("Rate"), Some("50"), StateSet::new());
    let (state, _) = reduce_from(
        &SrState::new(),
        &focus_event(TraceId::mint(), source, slider),
        OutpostId(1),
    );

    let value_changed = event_in(
        source,
        None,
        NormalizedEvent::ValueChanged {
            node_id: NodeId::new(500),
            value: Some("90".to_owned()),
        },
    );
    let (_, effects) = reduce_from(&state, &value_changed, OutpostId(2));
    assert!(effects.is_empty(), "another outpost's node 500");

    let (_, effects) = reduce_from(&state, &value_changed, OutpostId(1));
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::value("90")])],
        "the focus's own outpost"
    );
}

#[test]
fn a_replacement_numbering_afresh_after_many_ids_is_heard_at_once() {
    // Additional finding 1: a replaced outpost started its counters again
    // while Core remembered the old ones. Node ids name their generation,
    // so a replacement's small numbers are accepted at once, and the same
    // numbers stamped with the old generation are a different node. The
    // app drops the old generation's messages before they reach the
    // reducer (`LiveOutposts` in verbatim-app).
    let source = Pid(1);
    let mut state = SrState::new();
    for number in 1..=10_000 {
        let control = node(number, Role::Button, Some("Old"), None, StateSet::new());
        state = reduce_from(
            &state,
            &focus_in(source, window(10), control, Vec::new()),
            OutpostId(1),
        )
        .0;
    }
    let (state, _) = reduce(&state, &ended(OutpostId(1)));

    let slider = node(1, Role::Slider, Some("Volume"), Some("50"), StateSet::new());
    let (state, effects) = reduce_from(
        &state,
        &focus_in(source, window(10), slider, Vec::new()),
        OutpostId(2),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(2), 1, &[], None),
            vec![vec![
                UtteranceSegment::label("Volume"),
                role(Role::Slider),
                UtteranceSegment::value("50"),
            ]]
        ),
        "the replacement's first event is accepted"
    );

    let value = |number, value: &str| {
        event_in(
            source,
            None,
            NormalizedEvent::ValueChanged {
                node_id: NodeId::new(number),
                value: Some(value.to_owned()),
            },
        )
    };
    let (_, effects) = reduce_from(&state, &value(10_000, "90"), OutpostId(1));
    assert!(
        effects.is_empty(),
        "a late message from the replaced outpost"
    );
    let (_, effects) = reduce_from(&state, &value(1, "60"), OutpostId(1));
    assert!(
        effects.is_empty(),
        "the replaced outpost's own node 1 is not the focus"
    );
    let (_, effects) = reduce_from(&state, &value(1, "60"), OutpostId(2));
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::value("60")])],
        "the replacement's node 1 is the focus"
    );
}

#[test]
fn a_focus_in_the_system_foreground_window_moves_attention_without_a_foreground_fact() {
    // Windows raised the new window's foreground event while refusing it the
    // foreground, so that fact was dropped; when it got the foreground later
    // no second event came. Its focus says it is in the foreground window.
    let state = switch_to(&SrState::new(), Pid(1));
    let item = node(
        2,
        Role::TreeItem,
        Some("System Summary"),
        None,
        StateSet::new(),
    );
    let facts = WindowFacts {
        in_foreground: true,
        ..window(30)
    };
    // The window, unnamed, which both focuses are in.
    let frame = window_node(4, Backend::Uia, Role::Window, "", 30);

    let (state, effects) = reduce(&state, &focus_in(Pid(2), facts, item, vec![frame.clone()]));

    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 2, &[4], Some((OutpostId(2), 4)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("System Summary")]),
        ],
        "the focus is spoken, cutting off speech for the window left"
    );
    assert_eq!(state.attention(), Some(Pid(2)), "attention follows it");
    let button = node(3, Role::Button, Some("OK"), None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(2), foreground_window(30), button, vec![frame]),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(2), 3, &[4], Some((OutpostId(2), 4))),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        ),
        "the window's later events are attended"
    );
}

#[test]
fn a_focus_in_a_new_foreground_window_without_a_foreground_fact_announces_the_window() {
    // The same dropped foreground fact, in a File Explorer folder window:
    // the focus's outermost ancestor is the window's client area, read
    // through MSAA as a pane, which, being a top-level window's client with
    // a system menu, is presented by its title before the containers and
    // the focus, as NVDA presents a window it meets in the focus's ancestry.
    let state = switch_to(&SrState::new(), Pid(1));
    let mut title = node(
        5,
        Role::Pane,
        Some("Folder - File Explorer"),
        None,
        StateSet::new(),
    );
    title.details.top_level_client = true;
    let list = node(6, Role::List, Some("Items View"), None, StateSet::new());
    let item = node(7, Role::ListItem, Some("alpha.txt"), None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(2), foreground_window(40), item, vec![title, list]),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 7, &[5, 6], Some((OutpostId(2), 5)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Folder - File Explorer")]),
            queued(vec![
                UtteranceSegment::label("Items View"),
                role(Role::List)
            ]),
            queued(vec![UtteranceSegment::label("alpha.txt")]),
        ],
        "window, list, item"
    );
}

#[test]
fn a_pane_ancestor_that_is_no_top_level_windows_client_is_not_announced() {
    // The same window entered, but its outermost ancestor is a pane that is
    // not a top-level window's client with a system menu: it is structure,
    // as any other pane, and the window's speech is cut off and the
    // containers and the focus spoken without it.
    let state = switch_to(&SrState::new(), Pid(1));
    let pane = node(
        5,
        Role::Pane,
        Some("Folder - File Explorer"),
        None,
        StateSet::new(),
    );
    let list = node(6, Role::List, Some("Items View"), None, StateSet::new());
    let item = node(7, Role::ListItem, Some("alpha.txt"), None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(2), foreground_window(40), item, vec![pane, list]),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 7, &[5, 6], Some((OutpostId(2), 5)))),
            Heard::Stop,
            queued(vec![
                UtteranceSegment::label("Items View"),
                role(Role::List)
            ]),
            queued(vec![UtteranceSegment::label("alpha.txt")]),
        ],
        "list, item"
    );
}

#[test]
fn a_focus_entering_another_top_level_window_speaks_its_entered_ancestors_outermost_first() {
    // No foreground report came for the Settings window, as none comes for
    // a window read through UI Automation: the focus entering it cuts off
    // the speech of the window left, then speaks its entered ancestors, the
    // window first, then the focus (`docs/design/focus-pipeline.md`,
    // section 16.4).
    let state = switch_to(&SrState::new(), Pid(1));
    let window_node = node(1, Role::Window, Some("Settings"), None, StateSet::new());
    let group = node(2, Role::Group, Some("Display"), None, StateSet::new());
    let button = node(3, Role::Button, Some("Apply"), None, StateSet::new());
    let (state, effects) = reduce(
        &state,
        &focus_in(
            Pid(2),
            foreground_window(30),
            button,
            vec![window_node, group],
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 3, &[1, 2], Some((OutpostId(2), 1)))),
            Heard::Stop,
            queued(vec![
                UtteranceSegment::label("Settings"),
                role(Role::Window)
            ]),
            queued(vec![UtteranceSegment::label("Display"), role(Role::Group)]),
            queued(vec![UtteranceSegment::label("Apply"), role(Role::Button)]),
        ],
        "window, group, button"
    );
    assert_eq!(
        state.attention(),
        Some(Pid(2)),
        "attention follows the focus"
    );
}

#[test]
fn a_window_already_announced_by_its_foreground_fact_is_not_announced_again() {
    let state = switch_to(&SrState::new(), Pid(1));
    let mut title = node(
        5,
        Role::Pane,
        Some("Folder - File Explorer"),
        None,
        StateSet::new(),
    );
    title.details.top_level_client = true;
    let (state, effects) = reduce(
        &state,
        &foreground_in(Pid(2), foreground_window(40), title.clone()),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 5, &[], Some((OutpostId(2), 5)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Folder - File Explorer")]),
        ],
        "the foreground fact"
    );
    let item = node(7, Role::ListItem, Some("alpha.txt"), None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(2), foreground_window(40), item, vec![title]),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(2), 7, &[5], Some((OutpostId(2), 5))),
            vec![vec![UtteranceSegment::label("alpha.txt")]]
        ),
        "only the item"
    );
}

#[test]
fn focus_returning_from_a_topmost_popup_is_still_attended() {
    // A context menu is topmost and takes focus without becoming the
    // foreground window, so it must not take attention with it.
    let source = Pid(1);
    let state = switch_to(&SrState::new(), source);
    let item = node(2, Role::MenuItem, Some("Copy"), None, StateSet::new());
    let popup = WindowFacts {
        topmost: true,
        ..window(77)
    };
    let (state, effects) = reduce(&state, &focus_in(source, popup, item, vec![]));
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 2, &[], Some((OutpostId(1), 1000)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Copy")]),
        ],
        "the topmost menu is attended"
    );

    let edit = node(3, Role::EditableText, Some("Text"), None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &focus_in(source, foreground_window(1000), edit, vec![]),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 3, &[], Some((OutpostId(1), 1000)))),
            Heard::Stop,
            queued(vec![
                UtteranceSegment::label("Text"),
                role(Role::EditableText)
            ]),
        ],
        "focus back in the foreground window"
    );
}

#[test]
fn a_nameless_foreground_window_moves_attention_silently() {
    let state = switch_to(&SrState::new(), Pid(1));
    let nameless = node(5, Role::Window, None, None, StateSet::new());

    let (state, effects) = reduce(&state, &foreground_in(Pid(2), window(20), nameless));
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 5, &[], Some((OutpostId(2), 5)))),
            Heard::Stop
        ],
        "a bare window says nothing, but a new foreground window cancels speech, nameless or not"
    );
    assert_eq!(state.attention(), Some(Pid(2)));

    // The window has its name by the time its control takes focus, but the
    // focus line already holds it, so it is not announced later.
    let named = window_node(6, Backend::Uia, Role::Window, "Calculator", 20);
    let button = node(7, Role::Button, Some("Seven"), None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(2), foreground_window(20), button, vec![named]),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(2), 7, &[5], Some((OutpostId(2), 5))),
            vec![vec![UtteranceSegment::label("Seven"), role(Role::Button)]]
        )
    );
}

#[test]
fn held_nodes_group_focus_ancestors_selection_and_navigator_by_outpost() {
    let source = Pid(3);
    let list = node(1, Role::List, Some("Files"), None, StateSet::new());
    let pane = node(2, Role::Pane, None, None, StateSet::new());
    let item = node(3, Role::ListItem, Some("a.txt"), None, StateSet::new());
    let input = event_in(
        source,
        None,
        NormalizedEvent::FocusChanged {
            node: list,
            foreground: false,
            ancestors: vec![pane],
            selected_child: Some(item),
            text: None,
            earlier: None,
            group: None,
        },
    );
    let (state, _) = reduce(&SrState::new(), &input);

    let outpost = outpost_of(source);
    let held = state.held_nodes();
    assert_eq!(held.len(), 1);
    assert_eq!(
        held[&outpost].iter().copied().collect::<Vec<_>>(),
        vec![
            NodeId::in_outpost(outpost, 1),
            NodeId::in_outpost(outpost, 2),
            NodeId::in_outpost(outpost, 3),
        ]
    );
}

fn toast_in(source: Pid, facts: WindowFacts) -> Input {
    event_in(
        source,
        Some(facts),
        NormalizedEvent::Alert {
            node: node(
                9,
                Role::Window,
                Some("Download complete"),
                None,
                StateSet::new(),
            ),
        },
    )
}

#[test]
fn a_toast_is_spoken_from_anywhere_queued() {
    let state = switch_to(&SrState::new(), Pid(1));

    let (next, effects) = reduce(&state, &toast_in(Pid(2), window(20)));

    assert_eq!(
        heard(&effects),
        vec![queued(vec![
            UtteranceSegment::label("Download complete"),
            role(Role::Window)
        ])]
    );
    assert_eq!(
        next.attention(),
        Some(Pid(1)),
        "a toast never moves attention"
    );
}

// ---- States, values, and descriptions as NVDA speaks them ----

fn states(list: &[State]) -> StateSet {
    list.iter().copied().collect()
}

fn state(state: State) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::State(state))
}

fn not(state: State) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::NegatedState(state))
}

fn role(role: Role) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Role(role))
}

/// The segments spoken when focus lands on `snapshot`, after asserting
/// that the focus change produced nothing but its drop of expired speech
/// and that one queued utterance.
fn focus_segments(snapshot: NodeSnapshot) -> Vec<UtteranceSegment> {
    let focus = snapshot.id.number();
    let (_, effects) = reduce(
        &SrState::new(),
        &focus_event(TraceId::mint(), Pid(1), snapshot),
    );
    match heard(&effects).as_slice() {
        [
            Heard::Expire(expired),
            Heard::Say(SpeechPriority::Queued, segments),
        ] if *expired == plain_focus(focus) => segments.clone(),
        other => panic!("expected the focus's one queued utterance, got {other:?}"),
    }
}

fn value_changed(node_id: u64, value: &str) -> Input {
    Input::Event {
        observed_at_us: 0,
        trace_id: TraceId::mint(),
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::ValueChanged {
            node_id: NodeId::new(node_id),
            value: Some(value.to_string()),
        },
    }
}

#[test]
fn typing_in_an_edit_field_does_not_speak_the_whole_field() {
    let edit = node(
        30,
        Role::EditableText,
        Some("Name"),
        Some("A"),
        StateSet::new(),
    );
    let state = focused(Pid(1), edit);
    let (state, effects) = reduce(&state, &value_changed(30, "An"));
    assert!(effects.is_empty(), "spoke {effects:?}");
    assert_eq!(
        state.focused().map(|(_, n)| n.value.clone()),
        Some(Some("An".to_string())),
        "the value is still recorded"
    );
}

#[test]
fn an_unchanged_value_is_not_spoken_again() {
    let slider = node(
        31,
        Role::Slider,
        Some("Volume"),
        Some("100"),
        StateSet::new(),
    );
    let state = focused(Pid(1), slider);
    let (_, effects) = reduce(&state, &value_changed(31, "100"));
    assert!(effects.is_empty(), "spoke {effects:?}");
}

#[test]
fn a_check_box_or_link_does_not_speak_its_value() {
    let link = node(
        32,
        Role::Link,
        Some("Help"),
        Some("https://example.com/help"),
        StateSet::new(),
    );
    assert_eq!(
        focus_segments(link),
        vec![UtteranceSegment::label("Help"), role(Role::Link)]
    );
}

#[test]
fn a_description_repeating_the_name_is_dropped() {
    let mut back = node(33, Role::Button, Some("Back"), None, StateSet::new());
    back.details.description = Some("Back".to_string());
    assert_eq!(
        focus_segments(back),
        vec![UtteranceSegment::label("Back"), role(Role::Button)]
    );
}

#[test]
fn states_are_spoken_in_nvda_order() {
    let check_box = node(
        34,
        Role::CheckBox,
        Some("Wrap"),
        None,
        states(&[State::Disabled]),
    );
    assert_eq!(
        focus_segments(check_box),
        vec![
            UtteranceSegment::label("Wrap"),
            role(Role::CheckBox),
            state(State::Disabled),
            not(State::Checked),
        ]
    );
}

#[test]
fn read_only_is_spoken_only_for_edit_fields_and_check_boxes() {
    let text = node(
        35,
        Role::StaticText,
        Some("Ready"),
        None,
        states(&[State::ReadOnly]),
    );
    assert_eq!(focus_segments(text), vec![UtteranceSegment::label("Ready")]);
    let edit = node(
        36,
        Role::EditableText,
        Some("Path"),
        None,
        states(&[State::ReadOnly]),
    );
    assert_eq!(
        focus_segments(edit),
        vec![
            UtteranceSegment::label("Path"),
            role(Role::EditableText),
            state(State::ReadOnly),
        ]
    );
}

#[test]
fn multi_line_is_spoken_after_read_only_and_only_for_edit_fields() {
    let edit = node(
        38,
        Role::EditableText,
        Some("Description"),
        None,
        states(&[State::Multiline, State::ReadOnly]),
    );
    assert_eq!(
        focus_segments(edit),
        vec![
            UtteranceSegment::label("Description"),
            role(Role::EditableText),
            state(State::ReadOnly),
            state(State::Multiline),
        ]
    );
    let document = node(
        39,
        Role::Document,
        Some("Text editor"),
        None,
        states(&[State::Multiline]),
    );
    assert_eq!(
        focus_segments(document),
        vec![UtteranceSegment::label("Text editor"), role(Role::Document)]
    );
}

#[test]
fn a_selected_tab_says_selected_and_an_unselected_one_says_nothing() {
    let selectable = [State::Focusable, State::Selectable];
    let selected = node(
        37,
        Role::Tab,
        Some("General"),
        None,
        states(&[State::Focusable, State::Selectable, State::Selected]),
    );
    assert_eq!(
        focus_segments(selected),
        vec![
            UtteranceSegment::label("General"),
            role(Role::Tab),
            state(State::Selected),
        ]
    );
    let unselected = node(38, Role::Tab, Some("Sharing"), None, states(&selectable));
    assert_eq!(
        focus_segments(unselected),
        vec![UtteranceSegment::label("Sharing"), role(Role::Tab)]
    );
}

#[test]
fn not_selected_needs_an_item_that_can_take_the_focus() {
    let item = node(
        39,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        states(&[State::Selectable]),
    );
    assert_eq!(
        focus_segments(item),
        vec![UtteranceSegment::label("alpha.txt")]
    );
}

#[test]
fn a_checkable_list_item_says_not_checked() {
    let item = node(
        40,
        Role::ListItem,
        Some("Bold"),
        None,
        states(&[State::Checkable]),
    );
    assert_eq!(
        focus_segments(item),
        vec![UtteranceSegment::label("Bold"), not(State::Checked)]
    );
}

#[test]
fn a_combo_box_does_not_say_submenu_nor_a_submenu_item_expanded() {
    let combo = node(
        41,
        Role::ComboBox,
        Some("Font"),
        None,
        states(&[State::HasPopup, State::Collapsed]),
    );
    assert_eq!(
        focus_segments(combo),
        vec![
            UtteranceSegment::label("Font"),
            role(Role::ComboBox),
            state(State::Collapsed),
        ]
    );
    let submenu = node(
        42,
        Role::MenuItem,
        Some("Recent"),
        None,
        states(&[State::HasPopup, State::Collapsed]),
    );
    assert_eq!(
        focus_segments(submenu),
        vec![UtteranceSegment::label("Recent"), state(State::HasPopup)]
    );
}

#[test]
fn reporting_the_object_speaks_selected_read_only_and_focused() {
    let item = node(
        43,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        states(&[
            State::Focused,
            State::Focusable,
            State::Selectable,
            State::Selected,
            State::ReadOnly,
        ]),
    );
    let state_after_focus = focused(Pid(1), item);
    let (_, effects) = reduce(
        &state_after_focus,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    assert_eq!(
        said(&effects),
        vec![
            UtteranceSegment::label("alpha.txt"),
            role(Role::ListItem),
            state(State::Focused),
            state(State::Selected),
            state(State::ReadOnly),
        ]
    );
}

#[test]
fn a_selection_gained_by_the_focus_says_selected() {
    let item = node(
        44,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        states(&[State::Focusable, State::Selectable]),
    );
    let state_after_focus = focused(Pid(1), item);
    let (_, effects) = reduce(
        &state_after_focus,
        &states_changed_input(
            TraceId::mint(),
            Pid(1),
            NodeId::new(44),
            states(&[State::Focusable, State::Selectable, State::Selected]),
        ),
    );
    assert_eq!(said(&effects), vec![state(State::Selected)]);
}

#[test]
fn losing_half_checked_says_not_checked() {
    let check_box = node(
        45,
        Role::CheckBox,
        Some("All"),
        None,
        states(&[State::Mixed]),
    );
    let state_after_focus = focused(Pid(1), check_box);
    let (_, effects) = reduce(
        &state_after_focus,
        &states_changed_input(TraceId::mint(), Pid(1), NodeId::new(45), StateSet::new()),
    );
    assert_eq!(said(&effects), vec![not(State::Checked)]);
}

// ---- Selection in a list the focus controls ----

fn controlled_selection(controller: u64, selected: NodeSnapshot) -> Input {
    Input::Event {
        observed_at_us: 0,
        trace_id: TraceId::mint(),
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event: NormalizedEvent::ControlledSelection {
            controller: NodeId::new(controller),
            node: selected,
        },
    }
}

fn search_result() -> NodeSnapshot {
    let mut result = node(
        51,
        Role::ListItem,
        Some("Notepad, App"),
        None,
        states(&[State::Selectable, State::Selected]),
    );
    result.details.position_in_set = Some(1);
    result.details.set_size = Some(4);
    result
}

#[test]
fn a_result_selected_in_the_list_the_focus_controls_is_spoken_as_a_focus() {
    let search_box = node(
        50,
        Role::EditableText,
        Some("Search box"),
        None,
        StateSet::new(),
    );
    let state = focused(Pid(1), search_box);

    let (state, effects) = reduce(&state, &controlled_selection(50, search_result()));
    assert_eq!(
        heard(&effects),
        vec![Heard::Say(
            SpeechPriority::Interrupt,
            vec![
                UtteranceSegment::label("Notepad, App"),
                UtteranceSegment::new(SegmentContent::Position {
                    position: 1,
                    set_size: Some(4),
                }),
            ]
        )]
    );
    assert_eq!(
        state.focused().map(|(_, focus)| focus.name.clone()),
        Some(Some("Search box".to_string())),
        "the focus stays in the search box"
    );
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    assert_eq!(
        said(&effects),
        vec![
            UtteranceSegment::label("Notepad, App"),
            role(Role::ListItem),
            UtteranceSegment::new(SegmentContent::State(State::Selected)),
            UtteranceSegment::new(SegmentContent::Position {
                position: 1,
                set_size: Some(4),
            }),
        ],
        "the navigator moved to the result"
    );
}

#[test]
fn a_controlled_selection_is_silent_once_the_controller_is_not_the_focus() {
    let other = node(60, Role::Button, Some("Close"), None, StateSet::new());
    let state = focused(Pid(1), other);
    let (_, effects) = reduce(&state, &controlled_selection(50, search_result()));
    assert!(effects.is_empty(), "spoke {effects:?}");
}

// ---- Where a new chain meets the previous one ----

/// The Settings frame, reported by its own outpost as its focus and by the
/// page's outpost as a pane above the page, is one entry of the focus
/// line: the page's chain meets the frame host's focus at the frame by its
/// handle, the frame host's entry is kept in the line, as NVDA keeps its
/// old object, only the content window below it is entered, and speech is
/// not cut (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn a_new_chain_meets_the_previous_one_at_a_window_reported_by_two_outposts() {
    let mut client = window_node(1, Backend::Msaa, Role::Pane, "Settings", 10);
    client.details.top_level_client = true;
    let (state, _) = reduce(
        &SrState::new(),
        &foreground_in(Pid(1), foreground_window(10), client),
    );
    let mut frame = window_node(4, Backend::Msaa, Role::Pane, "Settings", 10);
    frame.details.top_level_client = true;
    let content = window_node(2, Backend::Uia, Role::Window, "Settings", 11);
    let search = node(3, Role::EditableText, Some("Search"), None, StateSet::new());
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(2), foreground_window(10), search, vec![frame, content]),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            FocusNow {
                focus: ObjectKey::of_id(NodeId::in_outpost(OutpostId(2), 3)),
                ancestors: vec![
                    ObjectKey::of_id(NodeId::in_outpost(OutpostId(1), 1)),
                    ObjectKey::of_id(NodeId::in_outpost(OutpostId(2), 2)),
                ],
                foreground: Some(ObjectKey::of_id(NodeId::in_outpost(OutpostId(1), 1))),
                menu_item_of_focus: None,
            },
            vec![
                vec![UtteranceSegment::label("Settings"), role(Role::Window)],
                vec![UtteranceSegment::label("Search"), role(Role::EditableText)],
            ]
        )
    );
}

/// A focus whose walk was cut by its bound, its chain completed by plain
/// windows from its nearest window, meets the previous line at that window
/// and enters nothing; the focus line then holds no more than that window,
/// so the next focus, read in full, enters again the dialog the cut walk
/// did not reach, as NVDA's focus line would have lost it.
#[test]
fn a_chain_cut_in_the_same_window_says_again_what_it_did_not_reach() {
    let app = window_node(1, Backend::Uia, Role::Window, "App", 10);
    let options = node(3, Role::Dialog, Some("Options"), None, StateSet::new());
    let button = |id, name| node(id, Role::Button, Some(name), None, StateSet::new());
    let plain = window_node(4, Backend::Msaa, Role::Window, "App", 10);
    assert_eq!(
        spoken_by(&[
            focus_in(
                Pid(1),
                window(10),
                button(2, "OK"),
                vec![app.clone(), options.clone()]
            ),
            focus_in(Pid(1), window(10), button(5, "Cancel"), vec![plain]),
            focus_in(Pid(1), window(10), button(6, "Apply"), vec![app, options]),
        ]),
        vec![
            vec![UtteranceSegment::label("App"), role(Role::Window)],
            vec![UtteranceSegment::label("Options"), role(Role::Dialog)],
            vec![UtteranceSegment::label("OK"), role(Role::Button)],
            vec![UtteranceSegment::label("Cancel"), role(Role::Button)],
            vec![UtteranceSegment::label("Options"), role(Role::Dialog)],
            vec![UtteranceSegment::label("Apply"), role(Role::Button)],
        ]
    );
}

/// Speech is cut when the new focus line shares no entry with the previous
/// one below the desktop, NVDA's focus difference level of one or less,
/// and not when they meet: a popup in a top-level window of its own whose
/// chain goes through the window the focus was in meets the previous line
/// there and is not cut, while a focus in another top-level window, whose
/// chain shares nothing, is (`docs/parity/focus-context.md`,
/// "Focus-ancestry context").
#[test]
fn speech_is_cut_only_when_the_chains_differ_below_the_desktop() {
    let first = window_node(1, Backend::Uia, Role::Window, "First", 10);
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(
            Pid(1),
            window(10),
            node(2, Role::Button, Some("OK"), None, StateSet::new()),
            vec![first.clone()],
        ),
    );
    let popup = window_node(3, Backend::Uia, Role::List, "Choices", 20);
    let (state, effects) = reduce(
        &state,
        &focus_in(
            Pid(1),
            window(20),
            node(4, Role::ListItem, Some("Red"), None, StateSet::new()),
            vec![first, popup],
        ),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 4, &[1, 3], None),
            vec![
                vec![UtteranceSegment::label("Choices"), role(Role::List)],
                vec![UtteranceSegment::label("Red")],
            ]
        ),
        "the popup's chain meets the line at the first window"
    );
    let second = window_node(5, Backend::Uia, Role::Window, "Second", 30);
    let (_, effects) = reduce(
        &state,
        &focus_in(
            Pid(1),
            window(30),
            node(6, Role::Button, Some("Cancel"), None, StateSet::new()),
            vec![second],
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(1), 6, &[5], None)),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Second"), role(Role::Window)]),
            queued(vec![UtteranceSegment::label("Cancel"), role(Role::Button)]),
        ],
        "the chains share nothing below the desktop"
    );
}

/// A reader leaves an unpresentable top-level window, such as an unnamed
/// UI Automation one, out of a chain, which NVDA's line holds: two focuses
/// in one top-level window whose chains share no entry meet at that
/// window, which their window facts name, so speech is not cut and their
/// containers are entered (`docs/parity/focus-context.md`,
/// "Focus-ancestry context").
#[test]
fn a_top_level_window_left_out_of_both_chains_is_where_they_meet() {
    let group = |id, name: &str| node(id, Role::Group, Some(name), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(
            Pid(1),
            window(10),
            node(2, Role::Button, Some("OK"), None, StateSet::new()),
            vec![group(1, "Left")],
        ),
    );
    let (_, effects) = reduce(
        &state,
        &focus_in(
            Pid(1),
            window(10),
            node(4, Role::Button, Some("Apply"), None, StateSet::new()),
            vec![group(3, "Right")],
        ),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 4, &[3], None),
            vec![
                vec![UtteranceSegment::label("Right"), role(Role::Group)],
                vec![UtteranceSegment::label("Apply"), role(Role::Button)],
            ]
        )
    );
}

/// A reader leaves layout ancestors out of a chain, so a focus can come
/// with no ancestors at all: focuses whose window facts name one top-level
/// window meet at that window, empty chains or not, and speech is not cut
/// as the focus moves among them (`docs/parity/focus-context.md`,
/// "Focus-ancestry context").
#[test]
fn focuses_with_empty_chains_in_one_top_level_window_meet_there() {
    let button = |id, name: &str| node(id, Role::Button, Some(name), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(Pid(1), window(10), button(1, "One"), Vec::new()),
    );
    let (state, effects) = reduce(
        &state,
        &focus_in(Pid(1), window(10), button(2, "Two"), Vec::new()),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            plain_focus(2),
            vec![vec![UtteranceSegment::label("Two"), role(Role::Button)]]
        ),
        "the second button"
    );
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(1), window(10), button(3, "Three"), Vec::new()),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            plain_focus(3),
            vec![vec![UtteranceSegment::label("Three"), role(Role::Button)]]
        ),
        "the third button"
    );
}

/// Two chains read through MSAA in one web page, whose document carries
/// its window: the new chain meets the previous line only at the
/// document, by its window handle, and an unnamed list below it with the
/// role and name of the list the previous focus was in is a new container,
/// since the group above it is new, so NVDA's "Search grouping", "list",
/// "b link" is said (`docs/parity/focus-context.md`, "Focus-ancestry
/// context").
#[test]
fn a_container_matched_by_role_and_name_does_not_end_the_walk_to_the_meeting() {
    let msaa = |id, role, name: Option<&str>| {
        let mut node = node(id, role, name, None, StateSet::new());
        node.backend = Backend::Msaa;
        node
    };
    let document = window_node(1, Backend::Msaa, Role::Document, "Page", 20);
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(
            Pid(1),
            window(20),
            msaa(4, Role::Link, Some("a")),
            vec![
                document.clone(),
                msaa(2, Role::List, None),
                msaa(3, Role::ListItem, None),
            ],
        ),
    );
    let (_, effects) = reduce(
        &state,
        &focus_in(
            Pid(1),
            window(20),
            msaa(8, Role::Link, Some("b")),
            vec![
                document,
                msaa(5, Role::Group, Some("Search")),
                msaa(6, Role::List, None),
                msaa(7, Role::ListItem, None),
            ],
        ),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 8, &[1, 5, 6, 7], None),
            vec![
                vec![UtteranceSegment::label("Search"), role(Role::Group)],
                vec![role(Role::List)],
                vec![UtteranceSegment::label("b"), role(Role::Link)],
            ]
        )
    );
}

/// A UI Automation group and an MSAA group of one window, alike in role
/// and name, are two entries, as NVDA's classes never compare equal: the
/// MSAA group is entered and said, while the window, met by its handle, is
/// not said again (`docs/parity/focus-context.md`, "Focus-ancestry
/// context").
#[test]
fn a_uia_group_and_an_msaa_group_of_one_window_are_two_entries() {
    let group = |id, backend| {
        let mut node = node(id, Role::Group, Some("Tools"), None, StateSet::new());
        node.backend = backend;
        node
    };
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(
            Pid(1),
            window(10),
            node(3, Role::Button, Some("Cut"), None, StateSet::new()),
            vec![
                window_node(1, Backend::Uia, Role::Window, "App", 10),
                group(2, Backend::Uia),
            ],
        ),
    );
    let mut paste = node(6, Role::Button, Some("Paste"), None, StateSet::new());
    paste.backend = Backend::Msaa;
    let (_, effects) = reduce(
        &state,
        &focus_in(
            Pid(1),
            window(10),
            paste,
            vec![
                window_node(4, Backend::Msaa, Role::Window, "App", 10),
                group(5, Backend::Msaa),
            ],
        ),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 6, &[1, 5], None),
            vec![
                vec![UtteranceSegment::label("Tools"), role(Role::Group)],
                vec![UtteranceSegment::label("Paste"), role(Role::Button)],
            ]
        )
    );
}

/// A duplicate of the focus does not replace the focus line, as NVDA keeps
/// its line when the focus matches itself: a duplicate whose chain was
/// cut at its bound, holding only the application's window, leaves the
/// dialog in the line, so the next focus in the dialog says only itself
/// (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn a_duplicate_focus_does_not_replace_the_focus_line() {
    let app = window_node(1, Backend::Uia, Role::Window, "App", 10);
    let dialog = node(2, Role::Dialog, Some("Options"), None, StateSet::new());
    let ok = node(3, Role::Button, Some("OK"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(
            Pid(1),
            window(10),
            ok.clone(),
            vec![app.clone(), dialog.clone()],
        ),
    );
    // The duplicate's chain, cut at its bound, completed by the plain
    // window of the application's window.
    let plain = window_node(5, Backend::Msaa, Role::Pane, "App", 10);
    let (state, effects) = reduce(&state, &focus_in(Pid(1), window(10), ok, vec![plain]));
    assert_eq!(heard(&effects), Vec::new(), "the duplicate is silent");
    let (_, effects) = reduce(
        &state,
        &focus_in(
            Pid(1),
            window(10),
            node(4, Role::Button, Some("Apply"), None, StateSet::new()),
            vec![app, dialog],
        ),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 4, &[1, 2], None),
            vec![vec![UtteranceSegment::label("Apply"), role(Role::Button)]]
        )
    );
}

/// An invisible ancestor and a border are not presented as entered
/// containers, as NVDA counts an invisible object unavailable and a border
/// layout, while a visible dialog between them is
/// (`docs/parity/focus-context.md`, "Focus-ancestry context").
#[test]
fn an_invisible_or_border_ancestor_is_not_announced() {
    let msaa = |id, role, name: &str, states| {
        let mut node = node(id, role, Some(name), None, states);
        node.backend = Backend::Msaa;
        node
    };
    let hidden = msaa(
        1,
        Role::Group,
        "Hidden",
        StateSet::new().with(State::Invisible),
    );
    let frame = msaa(2, Role::Border, "Frame", StateSet::new());
    let dialog = msaa(3, Role::Dialog, "Visible", StateSet::new());
    let ok = msaa(4, Role::Button, "OK", StateSet::new());
    assert_eq!(
        spoken_by(&[focus_in(
            Pid(1),
            window(10),
            ok,
            vec![hidden, frame, dialog]
        )]),
        vec![
            vec![UtteranceSegment::label("Visible"), role(Role::Dialog)],
            vec![UtteranceSegment::label("OK"), role(Role::Button)],
        ]
    );
}

/// A UI Automation slider of class `SensitiveSlider`, which gives the focus
/// to its thumb.
fn sensitive_slider(id: u64, value: &str) -> NodeSnapshot {
    let mut slider = node(
        id,
        Role::Slider,
        Some("Volume"),
        Some(value),
        StateSet::new(),
    );
    slider.details.class_name = Some("SensitiveSlider".to_owned());
    slider
}

/// An entered `SensitiveSlider` is spoken as a focus is, its value
/// included, where any other entered container leaves its value out
/// (`docs/parity/focus-context.md`, "A slider that focuses its thumb").
#[test]
fn a_sensitive_slider_ancestor_is_spoken_with_its_value() {
    let thumb = node(2, Role::Thumb, Some("Position"), None, StateSet::new());
    assert_eq!(
        spoken_by(&[focus_in(
            Pid(1),
            window(10),
            thumb,
            vec![sensitive_slider(1, "50")]
        )]),
        vec![
            vec![
                UtteranceSegment::label("Volume"),
                role(Role::Slider),
                UtteranceSegment::value("50"),
            ],
            vec![UtteranceSegment::label("Position"), role(Role::Thumb)],
        ]
    );
}

/// A `SensitiveSlider`'s change of value is spoken while it is the focus's
/// parent, its thumb being the focus, and not when it repeats the value it
/// was last read with (`docs/parity/focus-context.md`, "A slider that
/// focuses its thumb").
#[test]
fn a_sensitive_slider_s_value_change_is_spoken_while_its_thumb_is_the_focus() {
    let thumb = node(2, Role::Thumb, Some("Position"), None, StateSet::new());
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(Pid(1), window(10), thumb, vec![sensitive_slider(1, "50")]),
    );
    let changed = |value: &str| {
        event_in(
            Pid(1),
            Some(window(10)),
            NormalizedEvent::ValueChanged {
                node_id: NodeId::new(1),
                value: Some(value.to_owned()),
            },
        )
    };
    let (state, effects) = reduce(&state, &changed("60"));
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::value("60")])]
    );
    let (_, effects) = reduce(&state, &changed("60"));
    assert_eq!(heard(&effects), vec![], "the value it was last read with");
}

// ---- The navigator stays current, and slow answers leave it put ----

#[test]
fn reporting_the_object_after_a_change_reads_the_object_as_it_is_now() {
    let source = Pid(1);
    let check_box = node(120, Role::CheckBox, Some("Wrap"), None, StateSet::new());
    let state = focused(source, check_box);
    let (state, _) = reduce(
        &state,
        &states_changed_input(
            TraceId::mint(),
            source,
            NodeId::new(120),
            StateSet::new().with(State::Checked),
        ),
    );
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    assert_eq!(
        said(&effects),
        vec![
            UtteranceSegment::label("Wrap"),
            role(Role::CheckBox),
            UtteranceSegment::new(SegmentContent::State(State::Checked)),
        ],
        "report object says checked after the box was checked"
    );
}

#[test]
fn a_navigation_the_application_did_not_answer_leaves_the_navigator_put() {
    let source = Pid(1);
    let button = node(121, Role::Button, Some("OK"), None, StateSet::new());
    let state = focused(source, button);
    let (state, effects) = reduce(&state, &command(TraceId::mint(), ReviewCommand::Parent, 0));
    let query = only_fetch(&effects);
    let (state, effects) = reduce(
        &state,
        &Input::FetchCompleted {
            trace_id: TraceId::mint(),
            query_id: query.query_id,
            kind: query.kind,
            result: FetchResult::Unanswered,
        },
    );
    assert!(effects.is_empty(), "nothing is spoken: {effects:?}");
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
    );
    assert_eq!(
        said(&effects),
        vec![UtteranceSegment::label("OK"), role(Role::Button)],
        "the navigator did not jump"
    );
}

#[test]
fn selecting_the_focused_item_itself_says_selected() {
    let source = Pid(1);
    let item = node(
        122,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        states(&[State::Focusable, State::Selectable]),
    );
    let state = focused(source, item.clone());
    let mut selected = item;
    selected.states.insert(State::Selected);
    let (_, effects) = reduce(&state, &selection_event(TraceId::mint(), source, selected));
    assert_eq!(said(&effects), vec![state_segment_selected()]);
}

fn state_segment_selected() -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::State(State::Selected))
}

#[test]
fn an_entered_container_speaks_its_states_and_position_but_not_its_value() {
    let source = Pid(1);
    let mut tab = node(
        130,
        Role::Tab,
        Some("General"),
        Some("tab value"),
        states(&[State::Selected, State::Selectable]),
    );
    tab.details.position_in_set = Some(1);
    tab.details.set_size = Some(3);
    tab.details.keyboard_shortcut = Some("Alt+G".to_string());
    tab.details.description = Some("General".to_string());
    let field = node(131, Role::EditableText, Some("Name"), None, StateSet::new());
    let (_, effects) = reduce(
        &SrState::new(),
        &focus_event_with_ancestors(TraceId::mint(), source, field, vec![tab]),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 131, &[130], None),
            vec![
                vec![
                    UtteranceSegment::label("General"),
                    role(Role::Tab),
                    state(State::Selected),
                    UtteranceSegment::new(SegmentContent::Position {
                        position: 1,
                        set_size: Some(3),
                    }),
                ],
                vec![UtteranceSegment::label("Name"), role(Role::EditableText)],
            ]
        ),
        "no value, no shortcut, and no description repeating the name"
    );
}

// ---- Review messages and repeated presses, as NVDA's review commands ----

/// Runs review command `cmd` and returns the segments it speaks, after
/// asserting that it produced exactly one queued utterance and no other
/// effect.
fn review(state: &SrState, cmd: ReviewCommand, repeat: u8) -> (SrState, Vec<UtteranceSegment>) {
    let (state, effects) = reduce(state, &command(TraceId::mint(), cmd, repeat));
    (state, said(&effects))
}

/// The segments of the one queued utterance that is all of `effects`.
fn said(effects: &[Effect]) -> Vec<UtteranceSegment> {
    match heard(effects).as_slice() {
        [Heard::Say(SpeechPriority::Queued, segments)] => segments.clone(),
        other => panic!("expected one queued utterance and nothing else, got {other:?}"),
    }
}

fn message(message: verbatim_model::Message) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Message(message))
}

/// A focus whose value is reviewed as flat text: a role with no text
/// interface, so the review cursor walks its value (the M3 flat review).
fn reviewing(value: &str) -> SrState {
    let edit = node(
        140,
        Role::StaticText,
        Some("Body"),
        Some(value),
        StateSet::new(),
    );
    focused(Pid(1), edit)
}

#[test]
fn review_edges_are_named_and_the_unit_is_read_again() {
    use verbatim_model::Message;
    let state = reviewing("ab\ncd");
    let (state, segments) = review(&state, ReviewCommand::ReviewPreviousLine, 0);
    assert_eq!(
        segments,
        vec![message(Message::Top), UtteranceSegment::text("ab")]
    );
    let (state, segments) = review(&state, ReviewCommand::ReviewPreviousCharacter, 0);
    assert_eq!(
        segments,
        vec![message(Message::Left), UtteranceSegment::text("a")]
    );
    let (state, _) = review(&state, ReviewCommand::ReviewNextCharacter, 0);
    // Character moves stop at the end of the line.
    let (_, segments) = review(&state, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        segments,
        vec![message(Message::Right), UtteranceSegment::text("b")]
    );
}

/// Flat review walks grapheme clusters: in "हिन्दी" the consonant ha with
/// its vowel sign is one character, and the conjunct after it, na, virama,
/// da, and the long vowel sign, is the next, starting at byte 6.
#[test]
fn flat_review_moves_by_grapheme_cluster() {
    let state = reviewing("हिन्दी");
    let (state, segments) = review(&state, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        segments,
        vec![UtteranceSegment::new(SegmentContent::Character(
            "न्दी".to_owned()
        ))]
    );
    // The cursor is at byte 6: a start marked there, copied to the end of
    // the line, is the second character exactly.
    let (state, segments) = review(&state, ReviewCommand::SetStartMarker, 0);
    assert_eq!(
        segments,
        vec![message(verbatim_model::Message::StartMarked)]
    );
    let (state, segments) = review(&state, ReviewCommand::ReviewEndOfLine, 0);
    assert_eq!(segments, vec![UtteranceSegment::text("हिन्दी")]);
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::SelectThenCopy, 1),
    );
    assert_eq!(effects, vec![Effect::CopyToClipboard("न्दी".to_owned())]);
}

/// Flat text cannot be selected, so select then copy's first press says
/// so as NVDA does, and the second copies; with the review cursor moved
/// back before the marker, from the cursor through the character at the
/// marker (`docs/parity/review.md`, "Select then copy from the review
/// cursor").
#[test]
fn flat_review_copies_back_from_the_marker_through_the_marker_s_character() {
    let state = reviewing("abcd");
    let (state, _) = review(&state, ReviewCommand::ReviewNextCharacter, 0);
    let (state, _) = review(&state, ReviewCommand::ReviewNextCharacter, 0);
    let (state, _) = review(&state, ReviewCommand::SetStartMarker, 0);
    let (state, _) = review(&state, ReviewCommand::ReviewPreviousCharacter, 0);
    let (state, segments) = review(&state, ReviewCommand::SelectThenCopy, 0);
    assert_eq!(
        segments,
        vec![message(verbatim_model::Message::CannotSelect)]
    );
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::SelectThenCopy, 1),
    );
    assert_eq!(effects, vec![Effect::CopyToClipboard("bc".to_owned())]);
}

/// Flat text's first press is NVDA's attempt to select, so a further first
/// press asks for the second, which copies and clears the start marker
/// (`docs/parity/review.md`, "Select then copy from the review cursor").
#[test]
fn flat_review_asks_for_the_second_press_after_a_first() {
    use verbatim_model::Message;
    let state = reviewing("abcd");
    let (state, _) = review(&state, ReviewCommand::SetStartMarker, 0);
    let (state, _) = review(&state, ReviewCommand::ReviewNextCharacter, 0);
    let (state, segments) = review(&state, ReviewCommand::SelectThenCopy, 0);
    assert_eq!(segments, vec![message(Message::CannotSelect)]);
    let (state, segments) = review(&state, ReviewCommand::SelectThenCopy, 0);
    assert_eq!(segments, vec![message(Message::PressTwiceToCopy)]);
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::SelectThenCopy, 1),
    );
    assert_eq!(effects, vec![Effect::CopyToClipboard("ab".to_owned())]);
    let (_, segments) = review(&state, ReviewCommand::SelectThenCopy, 0);
    assert_eq!(segments, vec![message(Message::NoStartMarker)]);
}

/// The second press copies the range the first press built, not the one
/// the review cursor has moved to since (`docs/parity/review.md`, "Select
/// then copy from the review cursor").
#[test]
fn flat_review_copies_the_range_the_first_press_built() {
    use verbatim_model::Message;
    let state = reviewing("abcd");
    let (state, _) = review(&state, ReviewCommand::SetStartMarker, 0);
    let (state, _) = review(&state, ReviewCommand::ReviewNextCharacter, 0);
    let (state, segments) = review(&state, ReviewCommand::SelectThenCopy, 0);
    assert_eq!(segments, vec![message(Message::CannotSelect)]);
    let (state, _) = review(&state, ReviewCommand::ReviewNextCharacter, 0);
    let (state, _) = review(&state, ReviewCommand::ReviewNextCharacter, 0);
    let (state, segments) = review(&state, ReviewCommand::SelectThenCopy, 0);
    assert_eq!(segments, vec![message(Message::PressTwiceToCopy)]);
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::SelectThenCopy, 1),
    );
    assert_eq!(effects, vec![Effect::CopyToClipboard("ab".to_owned())]);
}

/// A copy of empty flat text copies nothing, says nothing, and still
/// clears the start marker.
#[test]
fn flat_review_copy_of_nothing_clears_the_start_marker() {
    use verbatim_model::Message;
    let state = focused(
        Pid(1),
        node(140, Role::StaticText, None, None, StateSet::new()),
    );
    let (state, _) = review(&state, ReviewCommand::SetStartMarker, 0);
    let (state, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::SelectThenCopy, 1),
    );
    assert_eq!(effects, Vec::new());
    let (_, segments) = review(&state, ReviewCommand::SelectThenCopy, 0);
    assert_eq!(segments, vec![message(Message::NoStartMarker)]);
}

/// An emoji with a skin tone is one character in flat review, so the next
/// character after it is the space.
#[test]
fn flat_review_keeps_an_emoji_sequence_whole() {
    let state = reviewing("👍🏽 ok");
    let (_, segments) = review(&state, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(segments, vec![message(verbatim_model::Message::Space)]);
}

/// Flat review finds the words of Thai, written without spaces, by
/// dictionary: "สวัสดีชาวโลก" is "สวัสดี", "ชาว", and "โลก".
#[test]
fn flat_review_finds_words_written_without_spaces() {
    let state = reviewing("สวัสดีชาวโลก");
    let (_, segments) = review(&state, ReviewCommand::ReviewNextWord, 0);
    assert_eq!(segments, vec![UtteranceSegment::text("ชาว")]);
}

#[test]
fn an_empty_unit_is_blank() {
    let state = reviewing("ab\n\ncd");
    let (state, _) = review(&state, ReviewCommand::ReviewNextLine, 0);
    let (_, segments) = review(&state, ReviewCommand::ReviewCurrentLine, 0);
    assert_eq!(segments, vec![message(verbatim_model::Message::Blank)]);
}

#[test]
fn the_current_line_pressed_twice_is_spelled_and_a_character_thrice_gives_its_code() {
    let state = reviewing("a b");
    let (state, segments) = review(&state, ReviewCommand::ReviewCurrentLine, 1);
    assert_eq!(
        segments,
        vec![
            UtteranceSegment::text("a"),
            message(verbatim_model::Message::Space),
            UtteranceSegment::text("b"),
        ]
    );
    let (_, segments) = review(&state, ReviewCommand::ReviewCurrentCharacter, 2);
    assert_eq!(
        segments,
        vec![
            UtteranceSegment::text("97,"),
            UtteranceSegment::text("0"),
            UtteranceSegment::text("x"),
            UtteranceSegment::text("6"),
            UtteranceSegment::text("1"),
        ]
    );
}

#[test]
fn an_activation_says_its_action_activate_or_no_action() {
    let outcome = |activated, action| {
        let (_, effects) = reduce(
            &SrState::new(),
            &Input::ActivationCompleted {
                trace_id: TraceId::mint(),
                activated,
                action,
            },
        );
        said(&effects)
    };
    assert_eq!(
        outcome(true, None),
        vec![message(verbatim_model::Message::Activate)]
    );
    assert_eq!(
        outcome(true, Some(verbatim_model::ActionName::Invoke)),
        vec![message(verbatim_model::Message::Invoke)]
    );
    assert_eq!(
        outcome(
            true,
            Some(verbatim_model::ActionName::Named("Press".to_owned()))
        ),
        vec![UtteranceSegment::text("Press")]
    );
    assert_eq!(
        outcome(false, None),
        vec![message(verbatim_model::Message::NoAction)]
    );
}

/// When speech is cut off (`docs/nvda/speech.md`, "Cancellation"): a
/// window coming to the front cancels speech and is announced, and its
/// control taking the focus a moment later queues behind the window's
/// title rather than cutting it off, since the title stays valid while its
/// window is in front.
#[test]
fn a_window_title_is_queued_before_its_control_not_cut_off() {
    let state = switch_to(&SrState::new(), Pid(3));
    let notepad = node(10, Role::Window, Some("Notepad"), None, StateSet::new());
    let (state, effects) = reduce(&state, &foreground_in(Pid(4), window(40), notepad.clone()));
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(4), 10, &[], Some((OutpostId(4), 10)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Notepad"), role(Role::Window)]),
        ],
        "a new foreground cancels speech and is announced"
    );
    let title = only_speech(&effects[2..]);
    let validity = title[0]
        .validity
        .expect("focus speech carries its validity");
    assert_eq!(validity.node.number(), 10);
    assert!(validity.had_focus);

    let editor = node(
        11,
        Role::Document,
        Some("Text editor"),
        None,
        StateSet::new(),
    );
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(4), foreground_window(40), editor, vec![notepad]),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(4), 11, &[10], Some((OutpostId(4), 10))),
            vec![vec![
                UtteranceSegment::label("Text editor"),
                role(Role::Document)
            ]]
        ),
        "the same window: nothing is cancelled, and the control queues behind the title"
    );
    let Some(Effect::DropExpiredSpeech(now)) = effects.first() else {
        panic!("the speech manager is told where the focus is first: {effects:?}");
    };
    assert_eq!(now.foreground.map(|key| key.id.number()), Some(10));
    assert_eq!(
        now.ancestors.iter().map(|key| key.id).collect::<Vec<_>>(),
        vec![NodeId::in_outpost(OutpostId(4), 10)]
    );
    assert!(
        title[0]
            .validity
            .is_some_and(|validity| validity.holds(now)),
        "the window's title is still worth hearing"
    );
}

/// Focus speech for a control the user has left no longer holds, but an
/// entered container's does while the focus is inside it, and so does
/// speech for a node that never had the focus.
#[test]
fn focus_speech_holds_while_its_node_is_the_focus_or_contains_it() {
    let now = FocusNow {
        focus: ObjectKey::of_id(NodeId::new(3)),
        ancestors: vec![ObjectKey::of_id(NodeId::new(1))],
        foreground: Some(ObjectKey::of_id(NodeId::new(9))),
        menu_item_of_focus: None,
    };
    let left = FocusValidity {
        node: NodeId::new(2),
        had_focus: true,
        window: None,
    };
    assert!(!left.holds(&now), "a control the user has left");
    for node in [1, 3, 9] {
        let validity = FocusValidity {
            node: NodeId::new(node),
            had_focus: true,
            window: None,
        };
        assert!(validity.holds(&now), "node {node}");
    }
    let never_focused = FocusValidity {
        node: NodeId::new(2),
        had_focus: false,
        window: None,
    };
    assert!(
        never_focused.holds(&now),
        "a dialog announced on entering it"
    );
}

/// An MSAA submenu item's speech stays valid when the application then
/// focuses the submenu that holds it, and only then: not for a menu that
/// does not hold it, nor for UIA (`docs/nvda/speech.md`, "Expired focus
/// speech").
#[test]
fn a_submenu_items_speech_holds_when_its_submenu_takes_the_focus() {
    let source = Pid(1);
    let msaa = |id: u64, role: Role| NodeSnapshot {
        backend: Backend::Msaa,
        ..node(id, role, Some("Node"), None, states(&[State::Focused]))
    };
    let focus = |snapshot: NodeSnapshot, ancestors: Vec<NodeSnapshot>| Input::Event {
        observed_at_us: 0,
        trace_id: TraceId::mint(),
        source,
        backend: snapshot.backend,
        window: None,
        event: NormalizedEvent::FocusChanged {
            foreground: false,
            node: snapshot,
            ancestors,
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
        },
    };
    let menu_item_of_focus = |effects: &[Effect]| match effects.first() {
        Some(Effect::DropExpiredSpeech(now)) => now.menu_item_of_focus.map(NodeId::number),
        other => panic!("the speech manager is told where the focus is first: {other:?}"),
    };
    let item_in_submenu = |state: &SrState, item: NodeSnapshot| {
        reduce(state, &focus(item, vec![msaa(5, Role::Menu)])).0
    };

    let state = item_in_submenu(&SrState::new(), msaa(6, Role::MenuItem));
    let (_, effects) = reduce(&state, &focus(msaa(5, Role::Menu), Vec::new()));
    assert_eq!(menu_item_of_focus(&effects), Some(6), "its submenu");
    let (_, effects) = reduce(&state, &focus(msaa(7, Role::Menu), Vec::new()));
    assert_eq!(menu_item_of_focus(&effects), None, "another menu");

    let uia_item = NodeSnapshot {
        backend: Backend::Uia,
        ..msaa(6, Role::MenuItem)
    };
    let state = item_in_submenu(&SrState::new(), uia_item);
    let (_, effects) = reduce(&state, &focus(msaa(5, Role::Menu), Vec::new()));
    assert_eq!(menu_item_of_focus(&effects), None, "a UIA item");
}

/// Spelling marks each capital so the theme raises its pitch, as NVDA
/// does; other characters stay plain text.
#[test]
fn spelling_marks_capitals_for_a_raised_pitch() {
    let state = reviewing("Hi");
    let (_, effects) = reduce(
        &state,
        &command(TraceId::mint(), ReviewCommand::ReviewCurrentCharacter, 0),
    );
    assert_eq!(
        said(&effects),
        vec![UtteranceSegment::new(SegmentContent::SpelledCapital(
            "H".to_owned()
        ))]
    );
}

#[test]
fn a_theme_with_descriptions_off_stops_their_fetching() {
    use verbatim_model::{Indication, IndicationSetting, Presentation, Theme};
    let state = SrState::new();
    assert!(
        state.fetches().description,
        "everything is fetched at first"
    );

    let mut theme = Theme::new("terse", "Terse");
    theme.indications.insert(
        Indication::Description,
        IndicationSetting {
            report: Presentation::Off,
            ..IndicationSetting::default()
        },
    );
    let (state, effects) = reduce(&state, &Input::Fetches(theme.fetches()));
    assert_eq!(effects, Vec::<Effect>::new());
    let fetches = state.fetches();
    assert!(!fetches.description, "off is not fetched");
    assert!(fetches.shortcut && fetches.position && fetches.spelling_errors);

    // The choice is part of the state a flight-recorder dump restores.
    let restored: SrState =
        serde_json::from_str(&serde_json::to_string(&state).expect("serializes"))
            .expect("deserializes");
    assert_eq!(restored.fetches(), fetches);
}

// ---- A runtime id reused for a new element ----

/// A File Explorer item, `position` of `set_size` in its folder's list.
fn explorer_item(
    id: u64,
    name: &str,
    item_states: &[State],
    position: u32,
    set_size: u32,
) -> NodeSnapshot {
    let mut item = node(id, Role::ListItem, Some(name), None, states(item_states));
    item.details.position_in_set = Some(position);
    item.details.set_size = Some(set_size);
    item
}

/// File Explorer, going back from a subfolder (found 2026-10-07), in the
/// order its events arrived: the focus moves to the parent folder's item
/// "Inner" in a new list, its states read before Explorer selected it, then
/// Explorer selects it, then reports the same focus again. Inner keeps the
/// node of delta.txt, whose runtime id Explorer gave it, and its report says
/// delta.txt, the element the reader read before under it, is gone
/// (`docs/design/focus-pipeline-rework.md`, "One object, one identity"), so
/// it is no duplicate: Inner is announced after its list, a new element, as
/// NVDA says it; the selection that follows says only "selected"; the
/// repeated focus, whose report says Inner still has the focus, is silent.
#[test]
fn a_reused_runtime_id_whose_element_is_gone_is_announced_and_its_selection_follows() {
    let app = Pid(1);
    let list = |id| node(id, Role::List, Some("Items View"), None, StateSet::new());
    // The folder window both lists are in, unnamed here.
    let frame = window_node(23, Backend::Uia, Role::Window, "", 7000);
    let unselected = [
        State::Focused,
        State::Focusable,
        State::Selectable,
        State::Offscreen,
    ];
    let selected = [
        State::Focused,
        State::Focusable,
        State::Selectable,
        State::Selected,
    ];
    let delta = explorer_item(28, "delta.txt", &unselected[..3], 1, 1);
    let (state, effects) = reduce(
        &SrState::new(),
        &focus_in(app, window(7000), delta, vec![frame.clone(), list(24)]),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 28, &[23, 24], None),
            vec![
                vec![UtteranceSegment::label("Items View"), role(Role::List)],
                vec![
                    UtteranceSegment::label("delta.txt"),
                    not(State::Selected),
                    UtteranceSegment::new(SegmentContent::Position {
                        position: 1,
                        set_size: Some(1),
                    }),
                ],
            ]
        ),
        "the first focus Core takes cuts nothing off"
    );

    let inner = explorer_item(28, "Inner", &unselected, 1, 4);
    let (state, effects) = reduce(
        &state,
        &focus_in_after(
            app,
            window(7000),
            inner,
            vec![frame.clone(), list(29)],
            EarlierElement::Gone,
        ),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 28, &[23, 29], None),
            vec![
                vec![UtteranceSegment::label("Items View"), role(Role::List)],
                vec![
                    UtteranceSegment::label("Inner"),
                    not(State::Selected),
                    UtteranceSegment::new(SegmentContent::Position {
                        position: 1,
                        set_size: Some(4),
                    }),
                ],
            ]
        ),
        "a new list with the same name and role is entered anew"
    );

    let inner_selected = explorer_item(28, "Inner", &selected, 1, 4);
    let (state_after, effects) = reduce(
        &state,
        &event_in(
            app,
            Some(window(7000)),
            NormalizedEvent::SelectionChanged {
                node: inner_selected.clone(),
            },
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![queued(vec![UtteranceSegment::new(SegmentContent::State(
            State::Selected
        ))])]
    );

    let (_, effects) = reduce(
        &state_after,
        &focus_in_after(
            app,
            window(7000),
            inner_selected,
            vec![frame.clone(), list(29)],
            EarlierElement::Focused,
        ),
    );
    assert_eq!(heard(&effects), vec![], "the repeated focus is silent");
}

/// The same return to the parent folder with the states the outpost reads
/// when it handles the focus, after Explorer selected Inner: "Items View
/// list", "Inner 1 of 4", as NVDA says it, and the selection and repeated focus that follow are
/// silent, since Inner was announced selected.
#[test]
fn a_focus_read_after_its_item_was_selected_is_announced_without_the_selection() {
    let app = Pid(1);
    let list = |id| node(id, Role::List, Some("Items View"), None, StateSet::new());
    // The folder window both lists are in, unnamed here.
    let frame = window_node(23, Backend::Uia, Role::Window, "", 7000);
    let selected = [
        State::Focused,
        State::Focusable,
        State::Selectable,
        State::Selected,
    ];
    let delta = explorer_item(28, "delta.txt", &selected, 1, 1);
    let (state, _) = reduce(
        &SrState::new(),
        &focus_in(app, window(7000), delta, vec![frame.clone(), list(24)]),
    );

    let inner = explorer_item(30, "Inner", &selected, 1, 4);
    let (state, effects) = reduce(
        &state,
        &focus_in(
            app,
            window(7000),
            inner.clone(),
            vec![frame.clone(), list(29)],
        ),
    );
    assert_eq!(
        heard(&effects),
        focus_heard(
            focus_now(OutpostId(1), 30, &[23, 29], None),
            vec![
                vec![UtteranceSegment::label("Items View"), role(Role::List)],
                vec![
                    UtteranceSegment::label("Inner"),
                    UtteranceSegment::new(SegmentContent::Position {
                        position: 1,
                        set_size: Some(4),
                    }),
                ],
            ]
        )
    );

    let (state, effects) = reduce(
        &state,
        &event_in(
            app,
            Some(window(7000)),
            NormalizedEvent::SelectionChanged {
                node: inner.clone(),
            },
        ),
    );
    assert_eq!(heard(&effects), vec![], "the selection was already heard");

    let (_, effects) = reduce(
        &state,
        &focus_in(app, window(7000), inner, vec![frame.clone(), list(29)]),
    );
    assert_eq!(heard(&effects), vec![], "the repeated focus is silent");
}

/// A focus or a foreground change on one of the shell's staging windows,
/// which File Explorer gives the focus or the foreground to on Alt+Tab,
/// Task View and Windows+X, is taken like any other focus, so it cuts
/// speech and the focus and the navigator move to it, but nothing is said
/// for it, even with a name (`docs/parity/windows-and-processes.md`, "The
/// shell's staging windows"). A window of another class is announced as
/// any is.
#[test]
fn a_focus_or_foreground_on_a_shell_staging_window_cuts_speech_and_says_nothing() {
    let state = switch_to(&SrState::new(), Pid(1));
    let pane = node(3, Role::Pane, Some("Staging"), None, StateSet::new());
    let staging_pane_reported = vec![queued(vec![
        UtteranceSegment::label("Staging"),
        role(Role::Pane),
    ])];
    for class in [
        "ForegroundStaging",
        "LauncherTipWnd",
        "ApplicationManager_DesktopShellWindow",
    ] {
        let facts = WindowFacts {
            class: class.to_owned(),
            ..foreground_window(50)
        };
        // A focus with no ancestry names no foreground object; a foreground
        // report makes the staging window the foreground object.
        for (input, foreground_object, foreground) in [
            (
                focus_in(Pid(2), facts.clone(), pane.clone(), vec![]),
                None,
                "a focus",
            ),
            (
                foreground_in(Pid(2), facts, pane.clone()),
                Some((OutpostId(2), 3)),
                "the foreground",
            ),
        ] {
            let (after, effects) = reduce(&state, &input);
            assert_eq!(
                heard(&effects),
                vec![
                    Heard::Expire(focus_now(OutpostId(2), 3, &[], foreground_object)),
                    Heard::Stop,
                ],
                "{foreground} in {class}"
            );
            assert_eq!(
                after.focused().map(|(source, focus)| (source, focus.id)),
                Some((Pid(2), NodeId::in_outpost(OutpostId(2), 3))),
                "{foreground} in {class}"
            );
            let (_, effects) = reduce(
                &after,
                &command(TraceId::mint(), ReviewCommand::ReportObject, 0),
            );
            assert_eq!(
                heard(&effects),
                staging_pane_reported,
                "the navigator followed {foreground} in {class}"
            );
        }
    }
    let named = node(3, Role::Button, Some("OK"), None, StateSet::new());
    let facts = WindowFacts {
        class: "Button".to_owned(),
        topmost: true,
        ..window(50)
    };
    let (_, effects) = reduce(&state, &focus_in(Pid(2), facts, named, vec![]));
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 3, &[], Some((OutpostId(1), 1000)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("OK"), role(Role::Button)]),
        ]
    );
}

/// The Windows 11 task switcher's `InputSite` window, in File Explorer, as
/// its outpost reads it for a focus.
fn task_switcher_window() -> WindowFacts {
    WindowFacts {
        class: "Windows.UI.Input.InputSite.WindowClass".to_owned(),
        application: "explorer".to_owned(),
        topmost: true,
        ..window(50)
    }
}

/// Node `id`, of `role` and named `name`, whose own UI Automation class is
/// `class`.
fn node_of_class(id: u64, role: Role, name: Option<&str>, class: &str) -> NodeSnapshot {
    let mut snapshot = node(id, role, name, None, StateSet::new());
    snapshot.details.class_name = Some(class.to_owned());
    snapshot
}

/// A UI Automation focus in File Explorer on a pane whose own class is
/// `Windows.UI.Input.InputSite.WindowClass`, the Windows 11 Alt+Tab
/// switcher's pane, is refused whole, as NVDA's File Explorer module
/// refuses it: no speech is cut, nothing is said, and the focus and the
/// navigator stay where they were (`docs/parity/windows-and-processes.md`,
/// "The Alt+Tab switcher's pane"). The switcher's list item in the same
/// window is announced.
#[test]
fn a_uia_focus_on_the_task_switching_pane_is_refused_whole() {
    let state = switch_to(&SrState::new(), Pid(1));
    let facts = task_switcher_window();
    let pane = node_of_class(
        3,
        Role::Pane,
        None,
        "Windows.UI.Input.InputSite.WindowClass",
    );
    let (after, effects) = reduce(&state, &focus_in(Pid(2), facts.clone(), pane, vec![]));
    assert_eq!(heard(&effects), vec![]);
    assert_eq!(after, state);

    let item = node(4, Role::ListItem, Some("Notes"), None, StateSet::new());
    let (_, effects) = reduce(&state, &focus_in(Pid(2), facts, item, vec![]));
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 4, &[], Some((OutpostId(1), 1000)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Notes")]),
        ]
    );
}

/// On Windows 10, the task switcher's frame is a window whose own UI
/// Automation class is `MultitaskingViewFrame`, in File Explorer: its UI
/// Automation focus is refused whole too, by the same rule of NVDA's File
/// Explorer module (`docs/parity/windows-and-processes.md`, "The Alt+Tab
/// switcher's pane").
#[test]
fn a_uia_focus_on_the_windows_10_task_switching_frame_is_refused_whole() {
    let state = switch_to(&SrState::new(), Pid(1));
    let facts = WindowFacts {
        class: "MultitaskingViewFrame".to_owned(),
        application: "explorer".to_owned(),
        topmost: true,
        ..window(50)
    };
    let frame = node_of_class(
        3,
        Role::Window,
        Some("Task Switching"),
        "MultitaskingViewFrame",
    );
    let (after, effects) = reduce(&state, &focus_in(Pid(2), facts, frame, vec![]));
    assert_eq!(heard(&effects), vec![]);
    assert_eq!(after, state);
}

/// The rule matches the focused element's own class and its application,
/// not the class of the window it is in: a pane of another class in File
/// Explorer's `InputSite` window, and a pane of the `InputSite` class in
/// another application's `InputSite` window, as a XAML island in Windows
/// Terminal has, are taken as any focus (`docs/parity/windows-and-processes.md`,
/// "The Alt+Tab switcher's pane").
#[test]
fn a_pane_is_refused_only_by_its_own_class_in_file_explorer() {
    let state = switch_to(&SrState::new(), Pid(1));
    let scroll_viewer = node_of_class(3, Role::Pane, Some("Output"), "ScrollViewer");
    let terminal_window = WindowFacts {
        application: "windowsterminal".to_owned(),
        ..task_switcher_window()
    };
    let terminal_pane = node_of_class(
        3,
        Role::Pane,
        Some("Output"),
        "Windows.UI.Input.InputSite.WindowClass",
    );
    for (facts, pane) in [
        (task_switcher_window(), scroll_viewer),
        (terminal_window, terminal_pane),
    ] {
        let (_, effects) = reduce(&state, &focus_in(Pid(2), facts, pane, vec![]));
        assert_eq!(
            heard(&effects),
            vec![
                Heard::Expire(focus_now(OutpostId(2), 3, &[], Some((OutpostId(1), 1000)))),
                Heard::Stop,
                queued(vec![UtteranceSegment::label("Output")]),
            ]
        );
    }
}

/// The task switcher's item for the window `name`, the second of ten, in
/// the first row and second column of its list, as the focus listener
/// reads it in Windows 11: a list item whose raw-view parent, the list,
/// has the automation id `parent_automation_id`.
fn task_switching_item(id: u64, name: &str, parent_automation_id: Option<&str>) -> NodeSnapshot {
    let mut item = node_of_class(id, Role::ListItem, Some(name), "ListViewItem");
    item.details.position_in_set = Some(2);
    item.details.set_size = Some(10);
    item.details.row = Some(1);
    item.details.column = Some(2);
    item.details.parent_automation_id = parent_automation_id.map(str::to_owned);
    item
}

/// The task switcher's containers as the focus listener reads them for
/// its item: its window, "Task Switching", and its list, "Running
/// applications", outermost first.
fn task_switching_containers() -> Vec<NodeSnapshot> {
    vec![
        node(
            10,
            Role::Window,
            Some("Task Switching"),
            None,
            StateSet::new(),
        ),
        node(
            11,
            Role::List,
            Some("Running applications"),
            None,
            StateSet::new(),
        ),
    ]
}

/// What the task switcher's item `name` says itself: its title, row,
/// column and position.
fn task_switching_item_said(name: &str) -> Heard {
    queued(vec![
        UtteranceSegment::label(name),
        UtteranceSegment::new(SegmentContent::Row(1)),
        UtteranceSegment::new(SegmentContent::Column(2)),
        UtteranceSegment::new(SegmentContent::Position {
            position: 2,
            set_size: Some(10),
        }),
    ])
}

/// What the task switcher's containers say as they are entered.
fn task_switching_containers_said() -> [Heard; 2] {
    [
        queued(vec![
            UtteranceSegment::label("Task Switching"),
            role(Role::Window),
        ]),
        queued(vec![
            UtteranceSegment::label("Running applications"),
            role(Role::List),
        ]),
    ]
}

/// With Alt held in the Windows 11 Alt+Tab switcher, its item is said
/// alone, by its window's title with its row, column and position, as
/// NVDA's capture says it ("Nvda Sw B  row 1  column 2  2 of 10",
/// `target/e2e-evidence/switcher-pane-2026-10-24/NvdaSw/`): NVDA's File
/// Explorer module drops the item's containers while Alt is held
/// (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's pane").
/// None of them is kept either: a later focus with Alt released, as in the
/// switcher Control+Alt+Tab keeps open, says them both.
#[test]
fn a_task_switching_item_reached_with_alt_held_says_only_itself() {
    let state = switch_to(&SrState::new(), Pid(1));
    let alt_held = WindowFacts {
        alt_held: true,
        ..task_switcher_window()
    };
    let item = task_switching_item(4, "Nvda Sw B", Some("SwitchItemListControl"));
    let (after, effects) = reduce(
        &state,
        &focus_in(
            Pid(2),
            alt_held.clone(),
            item.clone(),
            task_switching_containers(),
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 4, &[], Some((OutpostId(1), 1000)))),
            Heard::Stop,
            task_switching_item_said("Nvda Sw B"),
        ]
    );
    // The same focus again, a duplicate whose newer reading is kept, keeps
    // no containers either.
    let (after, effects) = reduce(
        &after,
        &focus_in(Pid(2), alt_held, item, task_switching_containers()),
    );
    assert_eq!(heard(&effects), vec![]);

    let released = task_switching_item(5, "Nvda Sw A", Some("SwitchItemListControl"));
    let (_, effects) = reduce(
        &after,
        &focus_in(
            Pid(2),
            task_switcher_window(),
            released,
            task_switching_containers(),
        ),
    );
    let [window_said, list_said] = task_switching_containers_said();
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(
                OutpostId(2),
                5,
                &[10, 11],
                Some((OutpostId(1), 1000))
            )),
            // The line of the item said alone held only the item, and so
            // shares nothing with this one below the desktop, as NVDA's,
            // whose item had the desktop for its container.
            Heard::Stop,
            window_said,
            list_said,
            task_switching_item_said("Nvda Sw A"),
        ]
    );
}

/// The rule drops an item's containers only for a UI Automation list item
/// in File Explorer, read with Alt held, whose parent is the task
/// switcher's list, or, in Windows 10 before version 1809, whose window is
/// the switcher's `MultitaskingViewFrame` window. A list item in File
/// Explorer under another list, one in another application under a list of
/// that id, one read through MSAA, and an element of another role under the
/// switcher's list say their containers with Alt held
/// (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's pane").
#[test]
fn only_the_task_switchers_items_drop_their_containers() {
    let state = switch_to(&SrState::new(), Pid(1));
    let alt_held = WindowFacts {
        alt_held: true,
        ..task_switcher_window()
    };
    let windows_10 = WindowFacts {
        class: "MultitaskingViewFrame".to_owned(),
        ..alt_held.clone()
    };
    let (_, effects) = reduce(
        &state,
        &focus_in(
            Pid(2),
            windows_10,
            task_switching_item(4, "Nvda Sw B", None),
            task_switching_containers(),
        ),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 4, &[], Some((OutpostId(1), 1000)))),
            Heard::Stop,
            task_switching_item_said("Nvda Sw B"),
        ]
    );

    let terminal = WindowFacts {
        application: "windowsterminal".to_owned(),
        ..alt_held.clone()
    };
    let msaa = NodeSnapshot {
        backend: Backend::Msaa,
        ..task_switching_item(4, "Nvda Sw B", Some("SwitchItemListControl"))
    };
    let cell = NodeSnapshot {
        role: Role::Cell,
        ..task_switching_item(4, "Nvda Sw B", Some("SwitchItemListControl"))
    };
    for (facts, item) in [
        (
            alt_held.clone(),
            task_switching_item(4, "Nvda Sw B", Some("ItemsView")),
        ),
        (
            terminal,
            task_switching_item(4, "Nvda Sw B", Some("SwitchItemListControl")),
        ),
        (alt_held.clone(), msaa),
        (alt_held, cell),
    ] {
        let (_, effects) = reduce(
            &state,
            &focus_in(Pid(2), facts, item, task_switching_containers()),
        );
        let [window_said, list_said] = task_switching_containers_said();
        assert_eq!(
            heard(&effects),
            vec![
                Heard::Expire(focus_now(
                    OutpostId(2),
                    4,
                    &[10, 11],
                    Some((OutpostId(1), 1000))
                )),
                Heard::Stop,
                window_said,
                list_said,
                task_switching_item_said("Nvda Sw B"),
            ]
        );
    }
}

/// The rule matches the class with its role: an element of the Windows 11
/// switcher pane's class in File Explorer that is a window, not a pane, is
/// taken as any focus (`docs/parity/windows-and-processes.md`, "The Alt+Tab
/// switcher's pane").
#[test]
fn a_window_of_the_task_switching_panes_class_is_taken() {
    let state = switch_to(&SrState::new(), Pid(1));
    let window = node_of_class(
        3,
        Role::Window,
        None,
        "Windows.UI.Input.InputSite.WindowClass",
    );
    let (after, _) = reduce(
        &state,
        &focus_in(Pid(2), task_switcher_window(), window, vec![]),
    );
    assert_eq!(
        after.focused().map(|(source, focus)| (source, focus.id)),
        Some((Pid(2), NodeId::in_outpost(OutpostId(2), 3)))
    );
}

/// The rule refuses a UI Automation focus event only: the same pane as a
/// foreground change, which NVDA never refuses so, or read through MSAA,
/// which NVDA's rule does not match, is taken as any focus or foreground
/// change (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's
/// pane").
#[test]
fn the_task_switching_panes_foreground_change_or_msaa_focus_is_taken() {
    let state = switch_to(&SrState::new(), Pid(1));
    let pane = node_of_class(
        3,
        Role::Pane,
        Some("Output"),
        "Windows.UI.Input.InputSite.WindowClass",
    );
    let (after, _) = reduce(
        &state,
        &foreground_in(Pid(2), task_switcher_window(), pane.clone()),
    );
    assert_eq!(
        after.focused().map(|(source, focus)| (source, focus.id)),
        Some((Pid(2), NodeId::in_outpost(OutpostId(2), 3)))
    );

    let msaa_pane = NodeSnapshot {
        backend: Backend::Msaa,
        ..pane
    };
    let (_, effects) = reduce(
        &state,
        &focus_in(Pid(2), task_switcher_window(), msaa_pane, vec![]),
    );
    assert_eq!(
        heard(&effects),
        vec![
            Heard::Expire(focus_now(OutpostId(2), 3, &[], Some((OutpostId(1), 1000)))),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Output")]),
        ]
    );
}
