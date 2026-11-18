//! What Core says of a focus and of changes on it, through the event
//! queue ([`crate::desk_tests`]): the focus's own words, the containers it
//! enters, and its name, value, state and description changing.

use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::{
    NodeDetails, NodeSnapshot, Role, SegmentContent, SpeechPriority, State, StateSet,
    UtteranceSegment,
};

use crate::desk_tests::{
    App, Desk, FRAME, Heard, ONE, TWO, focus_heard, focus_now, focus_now_in, focus_now_with,
    focus_read_of, follow, queued,
};

fn node(
    app: App,
    id: u64,
    role: Role,
    name: Option<&str>,
    value: Option<&str>,
    states: StateSet,
) -> NodeSnapshot {
    NodeSnapshot {
        id: app.node(id),
        backend: verbatim_model::Backend::Uia,
        role,
        name: name.map(str::to_string),
        value: value.map(str::to_string),
        states,
        details: NodeDetails::default(),
    }
}

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

/// The segments spoken when the focus lands on `snapshot` of [`ONE`],
/// after asserting that the focus had nothing but its drop of expired
/// speech, that one queued utterance and its `Follow`.
fn focus_segments(snapshot: &NodeSnapshot) -> Vec<UtteranceSegment> {
    let focus = snapshot.id.number();
    let mut desk = Desk::new();
    match desk.focus(ONE, snapshot, &[]).as_slice() {
        [
            Heard::Expire(expired),
            Heard::Say(SpeechPriority::Queued, segments),
            sent,
        ] if *expired == focus_now(ONE, focus, &[]) && *sent == follow(ONE) => segments.clone(),
        other => panic!("expected the focus's one queued utterance, got {other:?}"),
    }
}

#[test]
fn focus_menu_item_with_popup_speaks_name_and_submenu() {
    let mut desk = Desk::new();
    let snapshot = node(
        ONE,
        1,
        Role::MenuItem,
        Some("Settings..."),
        None,
        StateSet::new().with(State::HasPopup),
    );
    assert_eq!(
        desk.focus(ONE, &snapshot, &[]),
        focus_heard(
            ONE,
            focus_now(ONE, 1, &[]),
            vec![vec![
                UtteranceSegment::label("Settings..."),
                state(State::HasPopup),
            ]]
        )
    );
}

#[test]
fn focus_slider_then_drag_speaks_value_only_on_change() {
    let mut desk = Desk::new();
    let slider = node(
        ONE,
        2,
        Role::Slider,
        Some("Rate"),
        Some("50"),
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(ONE, &slider, &[]),
        focus_heard(
            ONE,
            focus_now(ONE, 2, &[]),
            vec![vec![
                UtteranceSegment::label("Rate"),
                role(Role::Slider),
                UtteranceSegment::value("50"),
            ]]
        )
    );
    assert_eq!(
        desk.followed_change(
            ONE,
            2,
            PropertyId::Value,
            Some(PropertyValue::Text("55".to_owned()))
        ),
        vec![queued(vec![UtteranceSegment::value("55")])]
    );
    assert_eq!(
        desk.held.focused().map(|(_, n)| n.value.clone()),
        Some(Some("55".to_string()))
    );
}

#[test]
fn unchecked_checkbox_announces_negated_checked() {
    let checkbox = node(
        ONE,
        3,
        Role::CheckBox,
        Some("Remember me"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        focus_segments(&checkbox),
        vec![
            UtteranceSegment::label("Remember me"),
            role(Role::CheckBox),
            not(State::Checked),
        ]
    );
}

#[test]
fn checked_checkbox_announces_checked() {
    let checkbox = node(
        ONE,
        4,
        Role::CheckBox,
        Some("Remember me"),
        None,
        StateSet::new().with(State::Checked),
    );
    assert_eq!(
        focus_segments(&checkbox),
        vec![
            UtteranceSegment::label("Remember me"),
            role(Role::CheckBox),
            state(State::Checked),
        ]
    );
}

#[test]
fn mixed_checkbox_does_not_announce_negated_checked() {
    let checkbox = node(
        ONE,
        5,
        Role::CheckBox,
        Some("Some of these"),
        None,
        StateSet::new().with(State::Mixed),
    );
    assert_eq!(
        focus_segments(&checkbox),
        vec![
            UtteranceSegment::label("Some of these"),
            role(Role::CheckBox),
            state(State::Mixed),
        ]
    );
}

#[test]
fn unpressed_toggle_button_announces_negated_pressed() {
    let toggle_button = node(
        ONE,
        900,
        Role::ToggleButton,
        Some("Bold"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        focus_segments(&toggle_button),
        vec![
            UtteranceSegment::label("Bold"),
            role(Role::ToggleButton),
            not(State::Pressed),
        ]
    );
}

#[test]
fn pressed_toggle_button_announces_pressed() {
    let toggle_button = node(
        ONE,
        901,
        Role::ToggleButton,
        Some("Bold"),
        None,
        StateSet::new().with(State::Pressed),
    );
    assert_eq!(
        focus_segments(&toggle_button),
        vec![
            UtteranceSegment::label("Bold"),
            role(Role::ToggleButton),
            state(State::Pressed),
        ]
    );
}

#[test]
fn disabled_button_announces_unavailable_state() {
    let button = node(
        ONE,
        6,
        Role::Button,
        Some("OK"),
        None,
        StateSet::new().with(State::Disabled),
    );
    assert_eq!(
        focus_segments(&button),
        vec![
            UtteranceSegment::label("OK"),
            role(Role::Button),
            state(State::Disabled),
        ]
    );
}

#[test]
fn focus_related_states_are_never_announced_but_unselected_is() {
    let item_states = states(&[
        State::Focused,
        State::Focusable,
        State::Selectable,
        State::Offscreen,
    ]);
    let item = node(ONE, 7, Role::ListItem, Some("Row"), None, item_states);
    assert_eq!(
        focus_segments(&item),
        vec![
            UtteranceSegment::label("Row"),
            // NVDA's rule: a selectable item that is not selected announces
            // exactly that; focused/focusable/offscreen stay silent.
            not(State::Selected),
        ]
    );
}

#[test]
fn selected_items_do_not_announce_positive_selected_on_focus() {
    let item = node(
        ONE,
        7,
        Role::ListItem,
        Some("Row"),
        None,
        states(&[State::Selectable, State::Selected]),
    );
    assert_eq!(
        focus_segments(&item),
        vec![UtteranceSegment::label("Row")],
        "a focused item being selected is the expected default and stays silent"
    );
}

#[test]
fn value_changed_for_non_focused_node_produces_no_effects() {
    let mut desk = Desk::new();
    let focused = node(
        ONE,
        8,
        Role::EditableText,
        Some("Name"),
        Some("a"),
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &focused, &[]);
    assert_eq!(
        desk.uia_change(
            ONE,
            9,
            PropertyId::Value,
            Some(PropertyValue::Text("changed".to_owned()))
        ),
        []
    );
    assert_eq!(
        desk.held.focused().map(|(_, n)| n.value.clone()),
        Some(Some("a".to_string()))
    );
}

#[test]
fn a_name_change_on_the_focus_speaks_the_new_name_alone_queued() {
    let mut desk = Desk::new();
    let focused = node(
        ONE,
        10,
        Role::EditableText,
        Some("Old name"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &focused, &[]);
    assert_eq!(
        desk.followed_change(
            ONE,
            10,
            PropertyId::Name,
            Some(PropertyValue::Text("New name".to_owned()))
        ),
        vec![queued(vec![UtteranceSegment::label("New name")])],
        "the new name alone, as NVDA speaks it"
    );
    assert_eq!(
        desk.held.focused().map(|(_, n)| n.name.clone()),
        Some(Some("New name".to_string()))
    );
}

#[test]
fn entering_a_dialog_announces_it_before_the_control() {
    let mut desk = Desk::new();
    let window = node(
        ONE,
        100,
        Role::Window,
        Some("Settings - App"),
        None,
        StateSet::new(),
    );
    let dialog = node(
        ONE,
        101,
        Role::Dialog,
        Some("Save changes"),
        None,
        StateSet::new(),
    );
    let button = node(ONE, 102, Role::Button, Some("Save"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &button, &[window, dialog]),
        focus_heard(
            ONE,
            focus_now_with(ONE, 102, &[100, 101], Some((ONE, 100))),
            vec![
                // The named window and then the dialog introduce themselves,
                // outermost first, each in an utterance of its own.
                vec![
                    UtteranceSegment::label("Settings - App"),
                    role(Role::Window)
                ],
                vec![UtteranceSegment::label("Save changes"), role(Role::Dialog)],
                vec![UtteranceSegment::label("Save"), role(Role::Button)],
            ]
        )
    );
}

fn states_value(states: StateSet) -> PropertyValue {
    PropertyValue::States(states)
}

/// A state change on an ancestor of the focus is spoken, as NVDA's base
/// state change handler speaks one (a focused button that changes the
/// state of the container above it), diffed against the ancestor's states
/// as the focus was reported with them; one on a node that is neither the
/// focus nor an ancestor stays silent.
#[test]
fn a_state_change_on_an_ancestor_of_the_focus_is_spoken() {
    let mut desk = Desk::new();
    let header = node(
        ONE,
        31,
        Role::ColumnHeader,
        Some("Name"),
        None,
        states(&[State::Collapsed]),
    );
    let button = node(
        ONE,
        32,
        Role::Button,
        Some("Sort"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    let _ = desk.focus(ONE, &button, &[header]);
    let expanded = Some(states_value(states(&[State::Expanded])));
    assert_eq!(
        desk.uia_change(ONE, 31, PropertyId::States, expanded.clone()),
        vec![queued(vec![state(State::Expanded)])]
    );
    assert_eq!(
        desk.uia_change(ONE, 31, PropertyId::States, expanded),
        vec![],
        "the same states again say nothing"
    );
    assert_eq!(
        desk.uia_change(
            ONE,
            99,
            PropertyId::States,
            Some(states_value(states(&[State::Checked])))
        ),
        vec![],
        "another node's change is silent"
    );
}

/// Visited is said only of a link, and linked only when it changes, as
/// NVDA says them; a busy indicator does not say its value.
#[test]
fn visited_is_said_of_a_link_and_linked_only_as_a_change() {
    let link = node(
        ONE,
        61,
        Role::Link,
        Some("Home"),
        None,
        states(&[State::Visited, State::Linked]),
    );
    assert_eq!(
        focus_segments(&link),
        vec![
            UtteranceSegment::label("Home"),
            role(Role::Link),
            state(State::Visited)
        ]
    );
    let button = node(
        ONE,
        62,
        Role::Button,
        Some("Home"),
        None,
        states(&[State::Visited, State::Focused]),
    );
    assert_eq!(
        focus_segments(&button),
        vec![UtteranceSegment::label("Home"), role(Role::Button)]
    );
    let busy = node(
        ONE,
        63,
        Role::BusyIndicator,
        Some("Loading"),
        Some("50"),
        StateSet::new(),
    );
    assert_eq!(
        focus_segments(&busy),
        vec![
            UtteranceSegment::label("Loading"),
            role(Role::BusyIndicator)
        ]
    );

    let mut desk = Desk::new();
    let item = node(
        ONE,
        64,
        Role::ListItem,
        Some("Part"),
        None,
        states(&[State::Focused]),
    );
    let _ = desk.focus(ONE, &item, &[]);
    assert_eq!(
        desk.followed_change(
            ONE,
            64,
            PropertyId::States,
            Some(states_value(states(&[State::Focused, State::Linked])))
        ),
        vec![queued(vec![state(State::Linked)])]
    );
}

/// The one request a change sent: a read of the node's properties.
fn properties_asked() -> Vec<Heard> {
    vec![Heard::Sent(
        crate::event_queue::Reader::Outpost(ONE.pid),
        "Properties",
    )]
}

/// The answer to a read of one property.
fn property(id: PropertyId, value: PropertyValue) -> verbatim_model::request::Reply {
    verbatim_model::request::Reply::Properties(verbatim_model::property::PropertyValues(vec![
        verbatim_model::property::PropertyAnswer {
            id,
            value: Some(value),
        },
    ]))
}

/// A description change on the focus speaks the new description alone, as
/// NVDA's base handler does, read from the focus when its change is told,
/// even one that repeats the name, NVDA's change speech having no name to
/// compare it with; the same description again, or another node's change,
/// says nothing.
#[test]
fn a_description_change_on_the_focus_is_spoken() {
    let mut desk = Desk::new();
    let field = node(
        ONE,
        41,
        Role::EditableText,
        Some("Password"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    let _ = desk.focus(ONE, &field, &[]);
    let described = |desk: &mut Desk, description: &str| {
        assert_eq!(
            desk.followed_change(ONE, 41, PropertyId::Description, None),
            properties_asked()
        );
        desk.reply(property(
            PropertyId::Description,
            PropertyValue::Text(description.to_owned()),
        ))
    };
    assert_eq!(
        described(&mut desk, "Too short"),
        vec![queued(vec![UtteranceSegment::new(
            SegmentContent::Description("Too short".to_owned())
        )])]
    );
    assert_eq!(described(&mut desk, "Too short"), vec![], "unchanged");
    assert_eq!(
        described(&mut desk, "Password"),
        vec![queued(vec![UtteranceSegment::new(
            SegmentContent::Description("Password".to_owned())
        )])],
        "the name again"
    );
    assert_eq!(
        desk.uia_change(ONE, 42, PropertyId::Description, None),
        vec![],
        "not the focus"
    );
}

/// A field's suggestions appearing play the suggestions-opened indication
/// only while the field is the focus; their going away, or a change of
/// another node, plays the suggestions-closed one
/// (`docs/parity/changes-and-selection.md`, "Suggestions appearing and going away").
#[test]
fn suggestions_opening_on_the_focus_and_closing_play_their_indications() {
    use verbatim_model::Earcon;
    let mut desk = Desk::new();
    let field = node(
        ONE,
        41,
        Role::EditableText,
        Some("Search"),
        None,
        states(&[State::Focusable, State::Focused]),
    );
    let _ = desk.focus(ONE, &field, &[]);
    let suggestions = |desk: &mut Desk, open: bool| {
        assert_eq!(
            desk.followed_change(ONE, 41, PropertyId::Suggestions, None),
            properties_asked()
        );
        desk.reply(property(PropertyId::Suggestions, PropertyValue::Flag(open)))
    };
    assert_eq!(
        suggestions(&mut desk, true),
        vec![Heard::Earcon(Earcon::SuggestionsOpened)]
    );
    assert_eq!(
        suggestions(&mut desk, false),
        vec![Heard::Earcon(Earcon::SuggestionsClosed)]
    );
    // Off the focus, a change plays the suggestions-closed indication only
    // for a suggestions field, by the facts its tell carries: a text
    // pattern, or a search field's automation id.
    let off_focus = |desk: &mut Desk, cached: Vec<PropertyAnswer>| {
        let tell = verbatim_model::tell::Event {
            kind: verbatim_model::event_kind::EventKind::UiaPropertyChanged,
            cached: verbatim_model::property::PropertyValues(cached),
            payload: verbatim_model::tell::Payload::PropertyChanged {
                property: PropertyId::Suggestions,
                value: None,
            },
            checked: None,
            ..crate::desk_tests::focus_tell(ONE, 42, desk.at_us + 1_000)
        };
        desk.step(crate::event_queue::Input::Entry(
            crate::entries::Entry::of_tell(ONE.instance(), tell).unwrap(),
        ))
        .0
    };
    assert_eq!(
        off_focus(&mut desk, Vec::new()),
        vec![],
        "no suggestions field"
    );
    assert_eq!(
        off_focus(
            &mut desk,
            vec![PropertyAnswer {
                id: PropertyId::HasTextPattern,
                value: Some(PropertyValue::Flag(true)),
            }]
        ),
        vec![Heard::Earcon(Earcon::SuggestionsClosed)],
        "editable text"
    );
    assert_eq!(
        off_focus(
            &mut desk,
            vec![
                PropertyAnswer {
                    id: PropertyId::HasTextPattern,
                    value: Some(PropertyValue::Flag(false)),
                },
                PropertyAnswer {
                    id: PropertyId::AutomationId,
                    value: Some(PropertyValue::Text("SearchTextBox".to_owned())),
                },
            ]
        ),
        vec![Heard::Earcon(Earcon::SuggestionsClosed)],
        "a search field"
    );
}

#[test]
fn expanding_without_a_count_says_only_expanded() {
    let mut desk = Desk::new();
    let collapsed = StateSet::new().with(State::Focused).with(State::Collapsed);
    let item = node(ONE, 22, Role::TreeItem, Some("Folder"), None, collapsed);
    let _ = desk.focus(ONE, &item, &[]);
    let expanded = StateSet::new().with(State::Focused).with(State::Expanded);
    assert_eq!(
        desk.followed_change(ONE, 22, PropertyId::States, Some(states_value(expanded))),
        vec![queued(vec![state(State::Expanded)])],
        "a tree item read through UIA says no count"
    );
}

#[test]
fn states_changed_checkbox_toggle_on_announces_checked() {
    let mut desk = Desk::new();
    let checkbox = node(
        ONE,
        20,
        Role::CheckBox,
        Some("Agree"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &checkbox, &[]);
    assert_eq!(
        desk.followed_change(
            ONE,
            20,
            PropertyId::States,
            Some(states_value(StateSet::new().with(State::Checked)))
        ),
        vec![queued(vec![state(State::Checked)])]
    );
    assert_eq!(
        desk.held.focused().map(|(_, n)| n.states),
        Some(StateSet::new().with(State::Checked))
    );
}

#[test]
fn states_changed_checkbox_toggle_off_announces_negated_checked() {
    let mut desk = Desk::new();
    let checkbox = node(
        ONE,
        21,
        Role::CheckBox,
        Some("Agree"),
        None,
        StateSet::new().with(State::Focused).with(State::Checked),
    );
    let _ = desk.focus(ONE, &checkbox, &[]);
    assert_eq!(
        desk.followed_change(
            ONE,
            21,
            PropertyId::States,
            Some(states_value(StateSet::new().with(State::Focused)))
        ),
        vec![queued(vec![not(State::Checked)])]
    );
    assert_eq!(
        desk.held.focused().map(|(_, n)| n.states),
        Some(StateSet::new().with(State::Focused))
    );
}

#[test]
fn states_changed_as_the_focus_leaves_an_item_is_silent() {
    // The list item the focus is leaving loses its selection and the
    // focus before the focus event for the next item arrives: no longer
    // focused, its "not selected" is not spoken.
    let mut desk = Desk::new();
    let selectable = StateSet::new()
        .with(State::Focusable)
        .with(State::Selectable);
    let item = node(
        ONE,
        26,
        Role::ListItem,
        Some("Speech"),
        None,
        selectable.with(State::Focused).with(State::Selected),
    );
    let _ = desk.focus(ONE, &item, &[]);
    assert_eq!(
        desk.followed_change(ONE, 26, PropertyId::States, Some(states_value(selectable))),
        vec![]
    );
}

#[test]
fn states_changed_unselecting_the_focused_item_announces_not_selected() {
    let mut desk = Desk::new();
    let focused = StateSet::new()
        .with(State::Focused)
        .with(State::Focusable)
        .with(State::Selectable);
    let item = node(
        ONE,
        27,
        Role::ListItem,
        Some("Speech"),
        None,
        focused.with(State::Selected),
    );
    let _ = desk.focus(ONE, &item, &[]);
    assert_eq!(
        desk.followed_change(ONE, 27, PropertyId::States, Some(states_value(focused))),
        vec![queued(vec![not(State::Selected)])]
    );
}

#[test]
fn states_changed_disabled_appearing_announces_unavailable() {
    let mut desk = Desk::new();
    let button = node(ONE, 22, Role::Button, Some("Submit"), None, StateSet::new());
    let _ = desk.focus(ONE, &button, &[]);
    assert_eq!(
        desk.followed_change(
            ONE,
            22,
            PropertyId::States,
            Some(states_value(StateSet::new().with(State::Disabled)))
        ),
        vec![queued(vec![state(State::Disabled)])]
    );
}

#[test]
fn a_button_disabled_after_the_focus_left_it_is_silent() {
    // The Reset button hands the focus to the tree, then is disabled; its
    // state change arrives before the tree's focus event. It no longer
    // reports itself focused, so "unavailable" is not spoken, nor is any
    // further change before the focus event.
    let mut desk = Desk::new();
    let focusable = StateSet::new().with(State::Focusable);
    let button = node(
        ONE,
        28,
        Role::Button,
        Some("Reset"),
        None,
        focusable.with(State::Focused),
    );
    let _ = desk.focus(ONE, &button, &[]);
    let disabled = StateSet::new().with(State::Disabled);
    assert_eq!(
        desk.followed_change(ONE, 28, PropertyId::States, Some(states_value(disabled))),
        vec![]
    );
    assert_eq!(desk.held.focused().map(|(_, n)| n.states), Some(disabled));
    assert_eq!(
        desk.followed_change(ONE, 28, PropertyId::States, Some(states_value(focusable))),
        vec![]
    );
}

#[test]
fn a_button_disabled_while_still_focused_says_unavailable() {
    let mut desk = Desk::new();
    let focused = StateSet::new().with(State::Focusable).with(State::Focused);
    let button = node(ONE, 29, Role::Button, Some("Reset"), None, focused);
    let _ = desk.focus(ONE, &button, &[]);
    assert_eq!(
        desk.followed_change(
            ONE,
            29,
            PropertyId::States,
            Some(states_value(focused.with(State::Disabled)))
        ),
        vec![queued(vec![state(State::Disabled)])]
    );
}

#[test]
fn states_changed_identical_set_is_silent() {
    let mut desk = Desk::new();
    let checked = StateSet::new().with(State::Checked);
    let checkbox = node(ONE, 23, Role::CheckBox, Some("Agree"), None, checked);
    let _ = desk.focus(ONE, &checkbox, &[]);
    assert_eq!(
        desk.followed_change(ONE, 23, PropertyId::States, Some(states_value(checked))),
        vec![]
    );
}

#[test]
fn states_changed_for_non_focused_node_is_ignored() {
    let mut desk = Desk::new();
    let focused = node(
        ONE,
        24,
        Role::CheckBox,
        Some("Agree"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &focused, &[]);
    assert_eq!(
        desk.uia_change(
            ONE,
            25,
            PropertyId::States,
            Some(states_value(StateSet::new().with(State::Checked)))
        ),
        vec![]
    );
    assert_eq!(
        desk.held.focused().map(|(_, n)| n.states),
        Some(StateSet::new())
    );
}

#[test]
fn moving_within_the_same_dialog_does_not_reannounce_it() {
    let mut desk = Desk::new();
    let dialog = node(
        ONE,
        101,
        Role::Dialog,
        Some("Save changes"),
        None,
        StateSet::new(),
    );
    let save = node(ONE, 102, Role::Button, Some("Save"), None, StateSet::new());
    let cancel = node(
        ONE,
        103,
        Role::Button,
        Some("Cancel"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &save, std::slice::from_ref(&dialog));
    assert_eq!(
        desk.focus(ONE, &cancel, &[dialog]),
        focus_heard(
            ONE,
            focus_now_in(ONE, 103, &[101]),
            vec![vec![UtteranceSegment::label("Cancel"), role(Role::Button)]]
        )
    );
}

/// The window node of `app`'s top-level window, [`FRAME`], named `name`.
fn frame(app: App, name: &str) -> NodeSnapshot {
    let mut frame = node(app, FRAME, Role::Window, Some(name), None, StateSet::new());
    frame.details.window = Some(app.top_level());
    frame
}

/// `app`'s top-level window came to the foreground, as the focus listener
/// reports it: Core asks `app` for its focus, answered as `read`, or, with
/// no `read`, asks nothing. What the answer said.
fn bring_forward(
    desk: &mut Desk,
    app: App,
    read: Option<verbatim_model::request::FocusRead>,
) -> Vec<Heard> {
    desk.app(app);
    let (heard, _) = desk.step(crate::event_queue::Input::Foreground(
        crate::event_queue::Foreground {
            window: app.top_level(),
            root_owner: app.top_level(),
            process: app.pid,
            thread: 41,
            by_class: None,
            shell: false,
        },
    ));
    let Some(read) = read else {
        assert_eq!(heard, []);
        return heard;
    };
    assert_eq!(
        heard,
        [Heard::Sent(
            crate::event_queue::Reader::Outpost(app.pid),
            "CurrentFocus"
        )]
    );
    desk.reply(verbatim_model::request::Reply::Candidate(
        verbatim_model::request::CandidateAnswer::focused(read),
    ))
}

#[test]
fn focus_from_another_application_treats_the_chain_as_new() {
    let mut desk = Desk::new();
    let dialog = node(ONE, 101, Role::Dialog, Some("Find"), None, StateSet::new());
    let edit_a = node(
        ONE,
        102,
        Role::EditableText,
        Some("Find what"),
        None,
        StateSet::new(),
    );
    let edit_b = node(
        TWO,
        202,
        Role::EditableText,
        Some("Search"),
        None,
        StateSet::new(),
    );
    let dialog_b = node(TWO, 201, Role::Dialog, Some("Open"), None, StateSet::new());
    let _ = desk.focus(ONE, &edit_a, &[dialog]);
    let window = frame(TWO, "App");
    let _ = bring_forward(&mut desk, TWO, Some(focus_read_of(TWO, &window, &[])));
    assert_eq!(
        desk.focus(TWO, &edit_b, &[window, dialog_b]),
        focus_heard(
            TWO,
            focus_now_in(TWO, 202, &[FRAME, 201]),
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
    let mut desk = Desk::new();
    let nameless = node(ONE, 300, Role::Group, None, None, StateSet::new());
    let named = node(
        ONE,
        301,
        Role::Group,
        Some("Margins"),
        None,
        StateSet::new(),
    );
    let field = node(
        ONE,
        302,
        Role::SpinButton,
        Some("Top"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(ONE, &field, &[nameless, named]),
        focus_heard(
            ONE,
            focus_now(ONE, 302, &[300, 301]),
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
    // focus enters it: NVDA presents a named list ancestor.
    let mut desk = Desk::new();
    let list = node(
        ONE,
        600,
        Role::List,
        Some("Categories"),
        None,
        StateSet::new(),
    );
    let item = node(
        ONE,
        601,
        Role::ListItem,
        Some("Speech"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(ONE, &item, &[list]),
        focus_heard(
            ONE,
            focus_now(ONE, 601, &[600]),
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
    let mut desk = Desk::new();
    let tree = node(ONE, 610, Role::Tree, None, None, StateSet::new());
    let item = node(
        ONE,
        611,
        Role::TreeItem,
        Some("Home"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(ONE, &item, &[tree]),
        focus_heard(
            ONE,
            focus_now(ONE, 611, &[610]),
            vec![
                vec![role(Role::Tree)],
                vec![UtteranceSegment::label("Home")]
            ]
        )
    );
}

fn tree_item(id: u64, name: &str, level: u32) -> NodeSnapshot {
    let mut item = node(ONE, id, Role::TreeItem, Some(name), None, StateSet::new());
    item.details.level = Some(level);
    item
}

#[test]
fn a_tree_view_taking_the_focus_on_its_item_is_spoken_as_the_items_ancestor() {
    // A Win32 tree view taking the focus is reported by its focused item,
    // with the tree as its ancestor (`docs/parity/windows-and-processes.md`, "A control's own
    // focus with a focused child"): NVDA says the tree's name and role,
    // without its shortcut, and then the item.
    let mut desk = Desk::new();
    let mut tree = node(
        ONE,
        630,
        Role::Tree,
        Some("Categories"),
        None,
        StateSet::new(),
    );
    tree.details.keyboard_shortcut = Some("Alt+i".to_owned());
    let mut item = tree_item(631, "General", 0);
    item.details.position_in_set = Some(1);
    item.details.set_size = Some(5);
    assert_eq!(
        desk.focus(ONE, &item, &[tree]),
        focus_heard(
            ONE,
            focus_now(ONE, 631, &[630]),
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
    let mut desk = Desk::new();
    let group = node(ONE, 620, Role::Group, None, None, StateSet::new());
    let button = node(ONE, 621, Role::Button, Some("OK"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &button, &[group]),
        focus_heard(
            ONE,
            focus_now(ONE, 621, &[620]),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        ),
        "a nameless group adds nothing and is not announced"
    );
}

#[test]
fn a_named_window_ancestor_is_announced_as_entered_context() {
    // NVDA presents a named window entered as an ancestor like any other
    // container; an unnamed one is layout.
    let mut desk = Desk::new();
    let window = node(
        ONE,
        630,
        Role::Window,
        Some("App - Window"),
        None,
        StateSet::new(),
    );
    let button = node(ONE, 631, Role::Button, Some("OK"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &button, &[window]),
        focus_heard(
            ONE,
            focus_now_with(ONE, 631, &[630], Some((ONE, 630))),
            vec![
                vec![UtteranceSegment::label("App - Window"), role(Role::Window)],
                vec![UtteranceSegment::label("OK"), role(Role::Button)],
            ]
        )
    );
}

#[test]
fn an_unnamed_window_ancestor_is_not_announced() {
    let mut desk = Desk::new();
    let window_node = node(ONE, 632, Role::Window, Some("  "), None, StateSet::new());
    let button = node(ONE, 633, Role::Button, Some("OK"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &button, &[window_node]),
        focus_heard(
            ONE,
            focus_now_with(ONE, 633, &[632], Some((ONE, 632))),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        )
    );
}

#[test]
fn a_border_or_white_space_ancestor_is_not_announced() {
    // NVDA counts both roles as layout, named or not.
    let mut desk = Desk::new();
    let border = node(ONE, 634, Role::Border, Some("Frame"), None, StateSet::new());
    let space = node(
        ONE,
        635,
        Role::Whitespace,
        Some("Gap"),
        None,
        StateSet::new(),
    );
    let button = node(ONE, 636, Role::Button, Some("OK"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &button, &[border, space]),
        focus_heard(
            ONE,
            focus_now(ONE, 636, &[634, 635]),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        )
    );
}

#[test]
fn list_item_and_editable_text_ancestors_are_dropped() {
    // NVDA's focus-ancestry exclusions: item and editable-text roles never
    // announce as entered context, even when named.
    let mut desk = Desk::new();
    let list_item = node(ONE, 640, Role::ListItem, Some("Row"), None, StateSet::new());
    let edit = node(
        ONE,
        641,
        Role::EditableText,
        Some("Field"),
        None,
        StateSet::new(),
    );
    let button = node(ONE, 642, Role::Button, Some("Go"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &button, &[list_item, edit]),
        focus_heard(
            ONE,
            focus_now(ONE, 642, &[640, 641]),
            vec![vec![UtteranceSegment::label("Go"), role(Role::Button)]]
        )
    );
}

#[test]
fn details_speak_in_nvda_property_order() {
    let mut item = node(
        ONE,
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
        cell: Some(verbatim_model::CellPosition {
            row: 3,
            column: 1,
            table: None,
        }),
        ..NodeDetails::default()
    };
    // The first level spoken goes first ("Where the level goes" in
    // `docs/nvda/speech.md`); everything else keeps NVDA's order, a table
    // cell's row and column after the shortcut and before the position
    // (`docs/parity/focus-announcement.md`, "Table cell coordinates").
    assert_eq!(
        focus_segments(&item),
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
    let mut cell = node(ONE, 400, Role::Cell, None, None, StateSet::new());
    cell.details.cell = Some(verbatim_model::CellPosition {
        row: 4,
        column: 2,
        table: None,
    });
    assert_eq!(
        focus_segments(&cell),
        vec![
            UtteranceSegment::new(SegmentContent::Row(4)),
            UtteranceSegment::new(SegmentContent::Column(2)),
        ]
    );
}

/// A list item named `name` at the table's `row` and `column`, in `table`
/// if it has one, as the focus listener reads a grid's item.
fn cell_in(id: u64, name: &str, (row, column): (u32, u32), table: Option<u64>) -> NodeSnapshot {
    let mut cell = node(ONE, id, Role::ListItem, Some(name), None, StateSet::new());
    cell.details.cell = Some(verbatim_model::CellPosition {
        row,
        column,
        table: table.map(verbatim_model::TableId),
    });
    cell
}

/// Reports `cells` focused in order, each after the last has been spoken,
/// and answers what each said of its row and column.
fn rows_and_columns_said(cells: &[NodeSnapshot]) -> Vec<Vec<UtteranceSegment>> {
    let mut desk = Desk::new();
    cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let heard = desk.focus(ONE, cell, &[]);
            // Each focus in the one window says one utterance.
            let segments = match heard.as_slice() {
                [
                    Heard::Expire(_),
                    Heard::Say(SpeechPriority::Queued, segments),
                    sent,
                ] if *sent == follow(ONE) => segments.clone(),
                other => panic!("expected focus {index}'s one queued utterance, got {other:?}"),
            };
            segments
                .into_iter()
                .filter(|segment| {
                    matches!(
                        segment.content,
                        SegmentContent::Row(_) | SegmentContent::Column(_)
                    )
                })
                .collect()
        })
        .collect()
}

fn row_said(row: u32) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Row(row))
}

fn column_said(column: u32) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Column(column))
}

/// Within one table, a cell says only what changed, as NVDA was captured
/// saying on the Alt+Tab switcher (`docs/parity/focus-announcement.md`,
/// "Table cell coordinates").
#[test]
fn a_cell_in_the_remembered_table_says_only_the_row_or_column_that_changed() {
    let said = rows_and_columns_said(&[
        cell_in(1, "One", (1, 2), Some(7)),
        cell_in(2, "Two", (1, 3), Some(7)),
        cell_in(3, "Three", (2, 3), Some(7)),
        cell_in(4, "Four", (2, 3), Some(7)),
    ]);
    assert_eq!(
        said,
        vec![
            vec![row_said(1), column_said(2)],
            vec![column_said(3)],
            vec![row_said(2)],
            vec![],
        ]
    );
}

/// A cell of another table says both, even at the row and column last
/// spoken, and that table becomes the one remembered.
#[test]
fn a_cell_in_another_table_says_both_and_is_remembered() {
    let said = rows_and_columns_said(&[
        cell_in(1, "One", (2, 3), Some(7)),
        cell_in(2, "Other", (2, 3), Some(8)),
        cell_in(3, "Beside", (2, 4), Some(8)),
        cell_in(1, "One", (2, 4), Some(7)),
    ]);
    assert_eq!(
        said,
        vec![
            vec![row_said(2), column_said(3)],
            vec![row_said(2), column_said(3)],
            vec![column_said(4)],
            vec![row_said(2), column_said(4)],
        ]
    );
}

/// A cell whose table is not known always says both and leaves the table
/// remembered as it was, so the next cell of that table is still in it.
#[test]
fn a_cell_with_no_table_says_both_and_keeps_the_table_remembered() {
    let said = rows_and_columns_said(&[
        cell_in(1, "One", (1, 2), Some(7)),
        cell_in(2, "Unknown", (1, 2), None),
        cell_in(3, "Two", (1, 3), Some(7)),
    ]);
    assert_eq!(
        said,
        vec![
            vec![row_said(1), column_said(2)],
            vec![row_said(1), column_said(2)],
            vec![column_said(3)],
        ]
    );
}

#[test]
fn a_tree_items_level_goes_first_only_when_it_changes() {
    let level = |level| UtteranceSegment::new(SegmentContent::Level(level));
    let mut desk = Desk::new();
    let mut window = node(ONE, 499, Role::Window, Some(""), None, StateSet::new());
    window.details.window = Some(verbatim_model::WindowHandle(499));
    let steps: Vec<Vec<Heard>> = [
        (500, "Hardware Resources", 1),
        (501, "Components", 1),
        (502, "System Summary", 0),
        (503, "Software Environment", 1),
    ]
    .into_iter()
    .map(|(id, name, depth)| desk.focus(ONE, &tree_item(id, name, depth), &[window.clone()]))
    .collect();
    assert_eq!(
        steps,
        vec![
            focus_heard(
                ONE,
                focus_now(ONE, 500, &[499]),
                vec![vec![
                    level(1),
                    UtteranceSegment::label("Hardware Resources")
                ]]
            ),
            focus_heard(
                ONE,
                focus_now_in(ONE, 501, &[499]),
                vec![vec![UtteranceSegment::label("Components"), level(1)]]
            ),
            focus_heard(
                ONE,
                focus_now_in(ONE, 502, &[499]),
                vec![vec![level(0), UtteranceSegment::label("System Summary")]]
            ),
            focus_heard(
                ONE,
                focus_now_in(ONE, 503, &[499]),
                vec![vec![
                    level(1),
                    UtteranceSegment::label("Software Environment")
                ]]
            ),
        ]
    );
}

#[test]
fn identical_back_to_back_focus_is_suppressed() {
    let mut desk = Desk::new();
    let button = node(ONE, 700, Role::Button, Some("OK"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &button, &[]),
        focus_heard(
            ONE,
            focus_now(ONE, 700, &[]),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        ),
        "first focus is announced"
    );
    // The exact same element focusing again from the same app: suppressed.
    assert_eq!(
        desk.duplicate(ONE, 700),
        [],
        "a redundant identical focus event is not re-announced"
    );
}

#[test]
fn returning_to_a_window_after_visiting_another_is_announced() {
    let mut desk = Desk::new();
    let a = node(ONE, 700, Role::Button, Some("OK"), None, StateSet::new());
    let b = node(
        TWO,
        800,
        Role::Button,
        Some("Cancel"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &a, &[]);
    // Visit another control (different app), then come back to the first.
    let _ = bring_forward(
        &mut desk,
        TWO,
        Some(focus_read_of(TWO, &frame(TWO, "App"), &[])),
    );
    let _ = desk.focus(TWO, &b, &[frame(TWO, "App")]);
    // The return: the window's own focus, as the listener tells it.
    let _ = bring_forward(&mut desk, ONE, None);
    let _ = desk.focus(ONE, &frame(ONE, "App"), &[]);
    // Its chain meets the line at its window, which the return brought
    // forward.
    assert_eq!(
        desk.focus(ONE, &a, &[frame(ONE, "App")]),
        focus_heard(
            ONE,
            focus_now_in(ONE, 700, &[FRAME]),
            vec![vec![UtteranceSegment::label("OK"), role(Role::Button)]]
        ),
        "focus that differs from the last announced one is announced, even if seen earlier"
    );
}

#[test]
fn a_named_item_leaves_its_role_unspoken_on_focus_but_an_unnamed_one_speaks_it() {
    // NVDA's rule ("When the role is spoken" in docs/nvda/speech.md): on
    // focus, a list item with a name says its name alone; with nothing else
    // to hear, it still says "list item".
    let named = node(
        ONE,
        7,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        focus_segments(&named),
        vec![UtteranceSegment::label("alpha.txt")]
    );
    let unnamed = node(ONE, 8, Role::ListItem, None, None, StateSet::new());
    assert_eq!(focus_segments(&unnamed), vec![role(Role::ListItem)]);
}

#[test]
fn a_name_change_after_a_repeat_is_spoken() {
    // A repeat of the focus reads nothing; the name change's own event is
    // compared with the name last spoken.
    let mut desk = Desk::new();
    let first = node(
        ONE,
        42,
        Role::Button,
        Some("Connect"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &first, &[]);
    assert_eq!(desk.duplicate(ONE, 42), vec![], "already the focus");
    assert_eq!(
        desk.followed_change(
            ONE,
            42,
            PropertyId::Name,
            Some(PropertyValue::Text("Disconnect".to_owned()))
        ),
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
    let mut desk = Desk::new();
    let collapsed = node(
        ONE,
        50,
        Role::ComboBox,
        Some("Theme"),
        None,
        states(&[State::Collapsed]),
    );
    let _ = desk.focus(ONE, &collapsed, &[]);
    let expanded = node(
        ONE,
        50,
        Role::ComboBox,
        Some("Theme"),
        None,
        states(&[State::Expanded]),
    );
    let item = node(ONE, 51, Role::ListItem, Some("Dark"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &item, &[expanded]),
        focus_heard(
            ONE,
            focus_now_in(ONE, 51, &[50]),
            vec![vec![UtteranceSegment::label("Dark")]]
        ),
        "the combo box is not entered anew"
    );
    assert_eq!(
        desk.uia_change(
            ONE,
            50,
            PropertyId::States,
            Some(states_value(states(&[State::Expanded])))
        ),
        vec![queued(vec![state(State::Expanded)])]
    );
}

/// A focused progress bar with a number, on screen, is heard only from the
/// listener: its ordinary value change is silent, its value kept. Off
/// screen, or with no number, it is an ordinary value change.
#[test]
fn a_focused_numeric_bar_is_silent_as_a_value_change_and_spoken_otherwise() {
    let bar = |extra: &[State]| {
        node(
            ONE,
            51,
            Role::ProgressBar,
            Some("Copying"),
            Some("0"),
            states(extra),
        )
    };
    let value = |desk: &mut Desk, value: &str| {
        desk.followed_change(
            ONE,
            51,
            PropertyId::Value,
            Some(PropertyValue::Text(value.to_owned())),
        )
    };
    let mut desk = Desk::new();
    let _ = desk.focus(ONE, &bar(&[State::Focusable, State::Focused]), &[]);
    assert_eq!(value(&mut desk, "41"), vec![], "on screen, with a number");
    assert_eq!(
        desk.held.focused().map(|(_, n)| n.value.clone()),
        Some(Some("41".to_owned()))
    );
    let mut desk = Desk::new();
    let _ = desk.focus(ONE, &bar(&[State::Focusable, State::Focused]), &[]);
    assert_eq!(
        value(&mut desk, "indeterminate"),
        vec![queued(vec![UtteranceSegment::value("indeterminate")])],
        "no number"
    );
    let mut desk = Desk::new();
    let _ = desk.focus(
        ONE,
        &bar(&[State::Focusable, State::Focused, State::Offscreen]),
        &[],
    );
    assert_eq!(
        value(&mut desk, "41"),
        vec![queued(vec![UtteranceSegment::value("41")])],
        "off screen"
    );
}

/// A state change of the focus's top-level window, the foreground object,
/// is not spoken as the focus's ancestor, as NVDA's event on the window
/// resolves to its foreground object (`docs/parity/focus-announcement.md`,
/// "A top-level window's state change").
#[test]
fn a_state_change_of_the_foreground_window_is_silent() {
    let mut desk = Desk::new();
    let client = frame(ONE, "Options");
    let check_box = node(ONE, 2, Role::CheckBox, Some("Wrap"), None, StateSet::new());
    let _ = desk.focus(ONE, &check_box, &[client]);
    assert_eq!(
        desk.uia_change(
            ONE,
            FRAME,
            PropertyId::States,
            Some(states_value(states(&[State::Disabled])))
        ),
        vec![]
    );
}

/// A name change of a node that is no longer Core's focus is silent.
#[test]
fn a_change_off_the_focus_is_silent() {
    let mut desk = Desk::new();
    let editor = node(
        ONE,
        1,
        Role::EditableText,
        Some("Text editor"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &editor, &[]);
    let button = node(ONE, 2, Role::Button, Some("OK"), None, StateSet::new());
    let _ = desk.focus(ONE, &button, &[]);
    assert_eq!(
        desk.uia_change(
            ONE,
            1,
            PropertyId::Name,
            Some(PropertyValue::Text("Notes".to_owned()))
        ),
        vec![]
    );
}

/// The duplicate rule compares with the reader's last focus: once another
/// application's focus came between, a focus on the earlier element is no
/// duplicate and is announced.
#[test]
fn the_same_focus_after_another_applications_focus_is_announced() {
    let mut desk = Desk::new();
    let edit = node(
        ONE,
        1,
        Role::EditableText,
        Some("Text editor"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &edit, &[]);
    let button = node(TWO, 1, Role::Button, Some("OK"), None, StateSet::new());
    let _ = desk.focus(TWO, &button, &[]);
    assert_eq!(
        desk.focus(ONE, &edit, &[]),
        vec![
            Heard::Expire(focus_now(ONE, 1, &[])),
            Heard::Stop,
            queued(vec![
                UtteranceSegment::label("Text editor"),
                role(Role::EditableText)
            ]),
            follow(TWO),
            follow(ONE),
        ]
    );
}

#[test]
fn an_entered_container_is_spoken_as_a_focus_is() {
    // A named static text entered as context says its name alone; a list,
    // which is not a silent role, still says "list".
    let mut desk = Desk::new();
    let label = node(
        ONE,
        20,
        Role::StaticText,
        Some("Options"),
        None,
        StateSet::new(),
    );
    let list = node(ONE, 21, Role::List, Some("Files"), None, StateSet::new());
    let item = node(ONE, 22, Role::Button, Some("OK"), None, StateSet::new());
    assert_eq!(
        desk.focus(ONE, &item, &[label, list]),
        focus_heard(
            ONE,
            focus_now(ONE, 22, &[20, 21]),
            vec![
                vec![UtteranceSegment::label("Options")],
                vec![UtteranceSegment::label("Files"), role(Role::List)],
                vec![UtteranceSegment::label("OK"), role(Role::Button)],
            ]
        )
    );
}

// Flat review over a focus with no text: its value, read live at each
// press.

/// A desk whose focus, and so the navigator, is the static text "Body"
/// with `value`, which has no text interface, so the review cursor walks
/// its value as flat text.
fn reviewing(value: &str) -> (Desk, NodeSnapshot) {
    let body = node(
        ONE,
        140,
        Role::StaticText,
        Some("Body"),
        Some(value),
        StateSet::new(),
    );
    let mut desk = Desk::new();
    let _ = desk.focus(ONE, &body, &[]);
    (desk, body)
}

/// Review command `command` pressed on `desk`, its navigator `body`: the
/// segments of the one queued utterance that is all it said.
fn review(
    desk: &mut Desk,
    body: &NodeSnapshot,
    command: verbatim_model::ReviewCommand,
    repeat: u8,
) -> Vec<UtteranceSegment> {
    match desk.review(command, repeat, body).as_slice() {
        [Heard::Say(SpeechPriority::Queued, segments)] => segments.clone(),
        other => panic!("expected one queued utterance and nothing else, got {other:?}"),
    }
}

fn message(message: verbatim_model::Message) -> UtteranceSegment {
    UtteranceSegment::new(SegmentContent::Message(message))
}

/// A character as a flat review press speaks it.
fn spelled_character(character: &str) -> Vec<UtteranceSegment> {
    vec![UtteranceSegment::text(character.to_owned())]
}

/// Flat review's current line leaves the position where it was
/// (`docs/parity/review.md`, "Dead objects"; `globalCommands.py` 1975 to
/// 1990).
#[test]
fn flat_reviews_current_line_leaves_the_position_where_it_was() {
    use verbatim_model::ReviewCommand;
    let (mut desk, body) = reviewing("hello\nworld");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0),
        spelled_character("e")
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentLine, 0),
        vec![UtteranceSegment::text("hello")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0),
        spelled_character("l")
    );
}

#[test]
fn review_edges_are_named_and_the_unit_is_read_again() {
    use verbatim_model::{Message, ReviewCommand};
    let (mut desk, body) = reviewing("ab\ncd");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewPreviousLine, 0),
        vec![message(Message::Top), UtteranceSegment::text("ab")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewPreviousCharacter, 0),
        vec![message(Message::Left), UtteranceSegment::text("a")]
    );
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    // Character moves stop at the end of the line.
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0),
        vec![message(Message::Right), UtteranceSegment::text("b")]
    );
}

/// Flat review's bottom moves to the last line's start and reads the line.
#[test]
fn flat_review_bottom_reads_the_last_line_from_its_start() {
    use verbatim_model::ReviewCommand;
    let (mut desk, body) = reviewing("ab\ncd\nef");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewBottom, 0),
        vec![UtteranceSegment::text("ef")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentCharacter, 0),
        vec![UtteranceSegment::text("e")]
    );
}

/// Flat review's start of line moves to the line's start but still speaks
/// the line, not the character there (`docs/parity/review.md`, "Review
/// cursor line/word/character over object text").
#[test]
fn flat_review_start_of_line_moves_to_the_lines_start_and_speaks_the_line() {
    use verbatim_model::ReviewCommand;
    let (mut desk, body) = reviewing("ab\ncd");
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewStartOfLine, 0),
        vec![UtteranceSegment::text("cd")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentCharacter, 0),
        vec![UtteranceSegment::text("c")]
    );
}

/// Flat review's previous word crosses a line start, and at the first word
/// says "Top" and the word again.
#[test]
fn flat_review_previous_word_crosses_a_line_start_and_stops_at_the_top() {
    use verbatim_model::{Message, ReviewCommand};
    let (mut desk, body) = reviewing("ab cd\nef");
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewPreviousWord, 0),
        vec![UtteranceSegment::text("cd")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewPreviousWord, 0),
        vec![UtteranceSegment::text("ab")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewPreviousWord, 0),
        vec![message(Message::Top), UtteranceSegment::text("ab")]
    );
}

/// Flat review walks grapheme clusters: in "हिन्दी" the consonant ha with
/// its vowel sign is one character, and the conjunct after it, na, virama,
/// da, and the long vowel sign, is the next, starting at byte 6.
#[test]
fn flat_review_moves_by_grapheme_cluster() {
    use verbatim_model::ReviewCommand;
    let (mut desk, body) = reviewing("हिन्दी");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0),
        vec![UtteranceSegment::new(SegmentContent::Character(
            "न्दी".to_owned()
        ))]
    );
    // The cursor is at byte 6: a start marked there, copied to the end of
    // the line, is the second character exactly.
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SetStartMarker, 0),
        vec![message(verbatim_model::Message::StartMarked)]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewEndOfLine, 0),
        vec![UtteranceSegment::text("हिन्दी")]
    );
    assert_eq!(
        desk.review(ReviewCommand::SelectThenCopy, 1, &body),
        vec![Heard::Copied("न्दी".to_owned())]
    );
}

/// Flat text cannot be selected, so select then copy's first press says
/// so as NVDA does, and the second copies; with the review cursor moved
/// back before the marker, from the cursor through the character at the
/// marker (`docs/parity/review.md`, "Select then copy from the review
/// cursor").
#[test]
fn flat_review_copies_back_from_the_marker_through_the_marker_s_character() {
    use verbatim_model::ReviewCommand;
    let (mut desk, body) = reviewing("abcd");
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    let _ = review(&mut desk, &body, ReviewCommand::SetStartMarker, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewPreviousCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        vec![message(verbatim_model::Message::CannotSelect)]
    );
    assert_eq!(
        desk.review(ReviewCommand::SelectThenCopy, 1, &body),
        vec![Heard::Copied("bc".to_owned())]
    );
}

/// Flat text's first press is NVDA's attempt to select, so a further first
/// press asks for the second, which copies and clears the start marker
/// (`docs/parity/review.md`, "Select then copy from the review cursor").
#[test]
fn flat_review_asks_for_the_second_press_after_a_first() {
    use verbatim_model::{Message, ReviewCommand};
    let (mut desk, body) = reviewing("abcd");
    let _ = review(&mut desk, &body, ReviewCommand::SetStartMarker, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        vec![message(Message::CannotSelect)]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        vec![message(Message::PressTwiceToCopy)]
    );
    assert_eq!(
        desk.review(ReviewCommand::SelectThenCopy, 1, &body),
        vec![Heard::Copied("ab".to_owned())]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        vec![message(Message::NoStartMarker)]
    );
}

/// The second press copies the range the first press built, not the one
/// the review cursor has moved to since (`docs/parity/review.md`, "Select
/// then copy from the review cursor").
#[test]
fn flat_review_copies_the_range_the_first_press_built() {
    use verbatim_model::{Message, ReviewCommand};
    let (mut desk, body) = reviewing("abcd");
    let _ = review(&mut desk, &body, ReviewCommand::SetStartMarker, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        vec![message(Message::CannotSelect)]
    );
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextCharacter, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::SelectThenCopy, 0),
        vec![message(Message::PressTwiceToCopy)]
    );
    assert_eq!(
        desk.review(ReviewCommand::SelectThenCopy, 1, &body),
        vec![Heard::Copied("ab".to_owned())]
    );
}

/// A copy of empty flat text copies nothing, says nothing, and still
/// clears the start marker.
#[test]
fn flat_review_copy_of_nothing_clears_the_start_marker() {
    use verbatim_model::{Message, ReviewCommand};
    let empty = node(ONE, 140, Role::StaticText, None, None, StateSet::new());
    let mut desk = Desk::new();
    let _ = desk.focus(ONE, &empty, &[]);
    let _ = review(&mut desk, &empty, ReviewCommand::SetStartMarker, 0);
    assert_eq!(
        desk.review(ReviewCommand::SelectThenCopy, 1, &empty),
        vec![]
    );
    assert_eq!(
        review(&mut desk, &empty, ReviewCommand::SelectThenCopy, 0),
        vec![message(Message::NoStartMarker)]
    );
}

/// An emoji with a skin tone is one character in flat review, so the next
/// character after it is the space.
#[test]
fn flat_review_keeps_an_emoji_sequence_whole() {
    let (mut desk, body) = reviewing("👍🏽 ok");
    assert_eq!(
        review(
            &mut desk,
            &body,
            verbatim_model::ReviewCommand::ReviewNextCharacter,
            0
        ),
        vec![message(verbatim_model::Message::Space)]
    );
}

/// Flat review finds the words of Thai, written without spaces, by
/// dictionary: "สวัสดีชาวโลก" is "สวัสดี", "ชาว", and "โลก".
#[test]
fn flat_review_finds_words_written_without_spaces() {
    let (mut desk, body) = reviewing("สวัสดีชาวโลก");
    assert_eq!(
        review(
            &mut desk,
            &body,
            verbatim_model::ReviewCommand::ReviewNextWord,
            0
        ),
        vec![UtteranceSegment::text("ชาว")]
    );
}

#[test]
fn an_empty_unit_is_blank() {
    use verbatim_model::ReviewCommand;
    let (mut desk, body) = reviewing("ab\n\ncd");
    let _ = review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentLine, 0),
        vec![message(verbatim_model::Message::Blank)]
    );
}

#[test]
fn the_current_line_pressed_twice_is_spelled_and_a_character_thrice_gives_its_code() {
    use verbatim_model::ReviewCommand;
    let (mut desk, body) = reviewing("a b");
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentLine, 1),
        vec![
            UtteranceSegment::text("a"),
            message(verbatim_model::Message::Space),
            UtteranceSegment::text("b"),
        ]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentCharacter, 2),
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
fn review_cursor_walks_lines_words_and_characters() {
    use verbatim_model::ReviewCommand;
    // Static text has no text interface, so its value is walked as flat
    // text.
    let (mut desk, body) = reviewing("first line\nsecond line");
    // Current line at the start.
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentLine, 0),
        vec![UtteranceSegment::text("first line")]
    );
    // Next line.
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0),
        vec![UtteranceSegment::text("second line")]
    );
    // Next line at the bottom: says "Bottom", stays put, re-reads.
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextLine, 0),
        vec![
            message(verbatim_model::Message::Bottom),
            UtteranceSegment::text("second line"),
        ]
    );
    // Top, then first word, then next word.
    let _ = review(&mut desk, &body, ReviewCommand::ReviewTop, 0);
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentWord, 0),
        vec![UtteranceSegment::text("first")]
    );
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewNextWord, 0),
        vec![UtteranceSegment::text("line")]
    );
    // First character of the current position ("line" -> 'l').
    assert_eq!(
        review(&mut desk, &body, ReviewCommand::ReviewCurrentCharacter, 0),
        vec![UtteranceSegment::text("l")]
    );
}

#[test]
fn a_focus_on_another_window_announces_the_window_as_the_focus() {
    let mut desk = Desk::new();
    let edit = node(
        ONE,
        2,
        Role::EditableText,
        Some("Text"),
        None,
        StateSet::new(),
    );
    let _ = desk.focus(ONE, &edit, &[]);
    let find = ONE.in_window(4_100);
    let mut other = node(find, 3, Role::Window, Some("Find"), None, StateSet::new());
    other.details.window = Some(find.top_level());
    assert_eq!(
        desk.focus(find, &other, &[]),
        vec![
            Heard::Expire(focus_now(ONE, 3, &[])),
            Heard::Stop,
            queued(vec![UtteranceSegment::label("Find"), role(Role::Window)]),
            follow(ONE),
        ]
    );
    assert_eq!(
        desk.held.focused().map(|(_, node)| node.role),
        Some(Role::Window)
    );
}

#[test]
fn a_window_spoken_as_the_focus_is_not_repeated_when_its_control_takes_focus() {
    let mut desk = Desk::new();
    let mut notepad = node(ONE, 1, Role::Window, Some("Notepad"), None, StateSet::new());
    notepad.details.window = Some(ONE.top_level());
    let _ = desk.focus(ONE, &notepad, &[]);
    // The control's ancestry reaches the same window through another path,
    // so its node id differs; the window's handle and backend are where the
    // two lines meet, and the line keeps its entry.
    let mut same_window = node(ONE, 5, Role::Window, Some("Notepad"), None, StateSet::new());
    same_window.details.window = Some(ONE.top_level());
    let edit = node(
        ONE,
        2,
        Role::EditableText,
        Some("Text"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(ONE, &edit, &[same_window]),
        focus_heard(
            ONE,
            focus_now_with(ONE, 2, &[1], Some((ONE, FRAME))),
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
    let mut desk = Desk::new();
    let window_node = frame(ONE, "Folder");
    let list = |id| {
        node(
            ONE,
            id,
            Role::List,
            Some("Items View"),
            None,
            StateSet::new(),
        )
    };
    let item = |id, name| node(ONE, id, Role::ListItem, Some(name), None, StateSet::new());
    let _ = desk.focus(ONE, &item(3, "Inner"), &[window_node.clone(), list(2)]);
    assert_eq!(
        desk.focus(ONE, &item(5, "delta"), &[window_node, list(4)]),
        focus_heard(
            ONE,
            focus_now_in(ONE, 5, &[FRAME, 4]),
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
    let mut desk = Desk::new();
    let frame_host = frame(ONE, "Settings");
    let _ = desk.focus(ONE, &frame_host, &[]);
    let settings = TWO.in_window(ONE.top);
    let mut frame_seen_from_page = node(
        settings,
        1,
        Role::Window,
        Some("Settings"),
        None,
        StateSet::new(),
    );
    frame_seen_from_page.details.window = Some(ONE.top_level());
    let group = node(
        settings,
        2,
        Role::Group,
        Some("Display"),
        None,
        StateSet::new(),
    );
    let toggle = node(
        settings,
        3,
        Role::ToggleButton,
        Some("Night light"),
        None,
        StateSet::new(),
    );
    assert_eq!(
        desk.focus(settings, &toggle, &[frame_seen_from_page, group]),
        vec![
            Heard::Expire(verbatim_model::FocusNow {
                focus: verbatim_model::ObjectKey::of_id(TWO.node(3)),
                // The line keeps the frame host's entry for the frame.
                ancestors: vec![
                    verbatim_model::ObjectKey::of_id(ONE.node(FRAME)),
                    verbatim_model::ObjectKey::of_id(TWO.node(2)),
                ],
                foreground: Some(verbatim_model::ObjectKey::of_id(ONE.node(FRAME))),
                menu_item_of_focus: None,
            }),
            queued(vec![UtteranceSegment::label("Display"), role(Role::Group)]),
            queued(vec![
                UtteranceSegment::label("Night light"),
                role(Role::ToggleButton),
                not(State::Pressed),
            ]),
            follow(ONE),
            follow(TWO),
        ],
        "the frame window is not spoken a second time"
    );
}

/// A desk focused on `snapshot` of [`ONE`].
fn focused(snapshot: &NodeSnapshot) -> Desk {
    let mut desk = Desk::new();
    let _ = desk.focus(ONE, snapshot, &[]);
    desk
}

fn value_text(value: &str) -> PropertyValue {
    PropertyValue::Text(value.to_owned())
}

#[test]
fn typing_in_an_edit_field_does_not_speak_the_whole_field() {
    let mut desk = focused(&node(
        ONE,
        30,
        Role::EditableText,
        Some("Name"),
        Some("A"),
        StateSet::new(),
    ));
    assert_eq!(
        desk.followed_change(ONE, 30, PropertyId::Value, Some(value_text("An"))),
        vec![]
    );
    assert_eq!(
        desk.held.focused().map(|(_, n)| n.value.clone()),
        Some(Some("An".to_string())),
        "the value is still recorded"
    );
}

#[test]
fn an_unchanged_value_is_not_spoken_again() {
    let mut desk = focused(&node(
        ONE,
        31,
        Role::Slider,
        Some("Volume"),
        Some("100"),
        StateSet::new(),
    ));
    assert_eq!(
        desk.followed_change(ONE, 31, PropertyId::Value, Some(value_text("100"))),
        vec![]
    );
}

#[test]
fn a_check_box_or_link_does_not_speak_its_value() {
    let link = node(
        ONE,
        32,
        Role::Link,
        Some("Help"),
        Some("https://example.com/help"),
        StateSet::new(),
    );
    assert_eq!(
        focus_segments(&link),
        vec![UtteranceSegment::label("Help"), role(Role::Link)]
    );
}

#[test]
fn a_description_repeating_the_name_is_dropped() {
    let mut back = node(ONE, 33, Role::Button, Some("Back"), None, StateSet::new());
    back.details.description = Some("Back".to_string());
    assert_eq!(
        focus_segments(&back),
        vec![UtteranceSegment::label("Back"), role(Role::Button)]
    );
}

#[test]
fn states_are_spoken_in_nvda_order() {
    let check_box = node(
        ONE,
        34,
        Role::CheckBox,
        Some("Wrap"),
        None,
        states(&[State::Disabled]),
    );
    assert_eq!(
        focus_segments(&check_box),
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
        ONE,
        35,
        Role::StaticText,
        Some("Ready"),
        None,
        states(&[State::ReadOnly]),
    );
    assert_eq!(
        focus_segments(&text),
        vec![UtteranceSegment::label("Ready")]
    );
    let edit = node(
        ONE,
        36,
        Role::EditableText,
        Some("Path"),
        None,
        states(&[State::ReadOnly]),
    );
    assert_eq!(
        focus_segments(&edit),
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
        ONE,
        38,
        Role::EditableText,
        Some("Description"),
        None,
        states(&[State::Multiline, State::ReadOnly]),
    );
    assert_eq!(
        focus_segments(&edit),
        vec![
            UtteranceSegment::label("Description"),
            role(Role::EditableText),
            state(State::ReadOnly),
            state(State::Multiline),
        ]
    );
    let document = node(
        ONE,
        39,
        Role::Document,
        Some("Text editor"),
        None,
        states(&[State::Multiline]),
    );
    assert_eq!(
        focus_segments(&document),
        vec![UtteranceSegment::label("Text editor"), role(Role::Document)]
    );
}

#[test]
fn a_selected_tab_says_selected_and_an_unselected_one_says_nothing() {
    let selected = node(
        ONE,
        37,
        Role::Tab,
        Some("General"),
        None,
        states(&[State::Focusable, State::Selectable, State::Selected]),
    );
    assert_eq!(
        focus_segments(&selected),
        vec![
            UtteranceSegment::label("General"),
            role(Role::Tab),
            state(State::Selected),
        ]
    );
    let unselected = node(
        ONE,
        38,
        Role::Tab,
        Some("Sharing"),
        None,
        states(&[State::Focusable, State::Selectable]),
    );
    assert_eq!(
        focus_segments(&unselected),
        vec![UtteranceSegment::label("Sharing"), role(Role::Tab)]
    );
}

#[test]
fn not_selected_needs_an_item_that_can_take_the_focus() {
    let item = node(
        ONE,
        39,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        states(&[State::Selectable]),
    );
    assert_eq!(
        focus_segments(&item),
        vec![UtteranceSegment::label("alpha.txt")]
    );
}

#[test]
fn a_checkable_list_item_says_not_checked() {
    let item = node(
        ONE,
        40,
        Role::ListItem,
        Some("Bold"),
        None,
        states(&[State::Checkable]),
    );
    assert_eq!(
        focus_segments(&item),
        vec![UtteranceSegment::label("Bold"), not(State::Checked)]
    );
}

#[test]
fn a_combo_box_does_not_say_submenu_nor_a_submenu_item_expanded() {
    let combo = node(
        ONE,
        41,
        Role::ComboBox,
        Some("Font"),
        None,
        states(&[State::HasPopup, State::Collapsed]),
    );
    assert_eq!(
        focus_segments(&combo),
        vec![
            UtteranceSegment::label("Font"),
            role(Role::ComboBox),
            state(State::Collapsed),
        ]
    );
    let submenu = node(
        ONE,
        42,
        Role::MenuItem,
        Some("Recent"),
        None,
        states(&[State::HasPopup, State::Collapsed]),
    );
    assert_eq!(
        focus_segments(&submenu),
        vec![UtteranceSegment::label("Recent"), state(State::HasPopup)]
    );
}

#[test]
fn a_selection_gained_by_the_focus_says_selected() {
    let mut desk = focused(&node(
        ONE,
        44,
        Role::ListItem,
        Some("alpha.txt"),
        None,
        states(&[State::Focusable, State::Selectable]),
    ));
    assert_eq!(
        desk.followed_change(
            ONE,
            44,
            PropertyId::States,
            Some(states_value(states(&[
                State::Focusable,
                State::Selectable,
                State::Selected
            ])))
        ),
        vec![queued(vec![state(State::Selected)])]
    );
}

#[test]
fn losing_half_checked_says_not_checked() {
    let mut desk = focused(&node(
        ONE,
        45,
        Role::CheckBox,
        Some("All"),
        None,
        states(&[State::Mixed]),
    ));
    assert_eq!(
        desk.followed_change(
            ONE,
            45,
            PropertyId::States,
            Some(states_value(StateSet::new()))
        ),
        vec![queued(vec![not(State::Checked)])]
    );
}

/// File Explorer's Down Arrow onto the next item, in the order the
/// readers sent it (GitHub run 44087451603, `explorer_folder_window`):
/// the listener tells the item's focus while the window's probe from the
/// last focus is in force, half a second passes before Core asks the
/// listener to read the element, and only then does Core ask the outpost
/// to complete the read. The listener never probes, so its answer, which
/// brings no probe, leaves the lapsed window with no verdict: the
/// outpost's completion carries "probe first", and the item is read and
/// spoken, as NVDA probes a window again once its half-second verdict has
/// lapsed and never takes a missing probe for "no provider"
/// (`docs/parity/backends.md`, "The UIA provider probe").
#[test]
fn a_focus_whose_verdict_lapses_before_the_listener_read_probes_again_at_the_completion() {
    use verbatim_model::request::{Answer, Outcome, Probe, Reply, Request as Wire, RequestNumber};
    use verbatim_model::request::{Reader, WindowVerdict};

    use crate::desk_tests::{focus_tell, listener_read};
    use crate::entries::Entry;
    use crate::entries::tests::{LISTENER, answer};
    use crate::event_queue::Input;

    let mut desk = Desk::new();
    let mut alpha = node(ONE, 1, Role::ListItem, Some("alpha"), None, StateSet::new());
    alpha.details.position_in_set = Some(2);
    alpha.details.set_size = Some(4);
    // The listener's tell carries its probe, begun as it told the focus.
    let probe_began = desk.at_us + 1_000;
    assert_eq!(
        desk.focus(ONE, &alpha, &[]),
        focus_heard(
            ONE,
            focus_now(ONE, 1, &[]),
            vec![vec![
                UtteranceSegment::label("alpha"),
                UtteranceSegment::new(SegmentContent::Position {
                    position: 2,
                    set_size: Some(4),
                }),
            ]]
        )
    );

    // The next item's focus, observed just before the probe's half second
    // ends, so the listener's check thread did not probe again, and taken
    // at its turn just after.
    let mut tell = focus_tell(ONE, 2, probe_began + 499_000);
    if let Some(checked) = tell.checked.as_mut() {
        checked.probe = None;
    }
    desk.at_us = probe_began + 500_000;
    let (_, sent) = desk.step(Input::Entry(Entry::of_tell(LISTENER, tell).unwrap()));
    let [(object, Reader::Listener, Wire::ReadObject(_))] = sent[..] else {
        panic!("one listener read asked: {sent:?}");
    };
    let (_, sent) = desk.step(answer(
        LISTENER,
        object,
        Reply::Object(Box::new(listener_read(ONE, 2))),
    ));
    let [(completion, Reader::Outpost(_), Wire::CompleteUia(_))] = sent[..] else {
        panic!("one completion asked: {sent:?}");
    };
    assert_eq!(
        desk.verdicts,
        [(completion, WindowVerdict::ProbeFirst)],
        "the listener's answer brought no probe, so no verdict is in force"
    );

    let mut beta = node(ONE, 2, Role::ListItem, Some("beta"), None, StateSet::new());
    beta.details.position_in_set = Some(3);
    beta.details.set_size = Some(4);
    let read = Reply::Focus(Box::new(focus_read_of(ONE, &beta, &[])));
    let probed = Input::Answer {
        from: ONE.instance(),
        answer: Answer {
            number: RequestNumber(completion),
            outcome: Outcome::Result(read),
            probes: vec![Probe {
                window: ONE.window(),
                thread: 41,
                provider: true,
                began_us: desk.at_us + 500,
            }],
            detail: String::new(),
        },
    };
    assert_eq!(
        desk.step(probed).0,
        focus_heard(
            ONE,
            focus_now_in(ONE, 2, &[]),
            vec![vec![
                UtteranceSegment::label("beta"),
                UtteranceSegment::new(SegmentContent::Position {
                    position: 3,
                    set_size: Some(4),
                }),
            ]]
        )
    );
}
