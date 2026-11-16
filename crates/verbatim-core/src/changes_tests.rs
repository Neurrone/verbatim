//! Changes on objects at their turn, against a committed UI Automation
//! focus: the button "OK", node 2 in window 5, under the frame "Editor",
//! node 1.

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    NodeRead, ObjectRead, Outcome as WireOutcome, RelationFound, RelationQuery, Reply,
    Request as Wire,
};
use verbatim_model::tell::{ElementToken, Event, EventObject, Payload, TellWindow};
use verbatim_model::{Role, State, StateSet, WindowHandle};

use crate::entries::tests::{
    APP, Held, LISTENER, NOTHING, OUTPOST, SPOKEN_FOLLOWED, answer, answer_outcome,
    asked_completion, entry, facts, feed_at, focus_read, node, values, win,
};
use crate::event_queue::Input;

/// The focus committed; the next request is number 4.
fn committed() -> Held {
    let mut state = asked_completion();
    assert_eq!(
        feed_at(
            &mut state,
            3_000,
            answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None))))
        ),
        SPOKEN_FOLLOWED
    );
    state
}

fn msaa_change(kind: EventKind, window: u64) -> Event {
    Event {
        trace_id: verbatim_model::TraceId::from_raw(8),
        kind,
        process: APP,
        application: "app".to_owned(),
        thread: 41,
        at_us: 4_000,
        object: EventObject::Msaa {
            window: WindowHandle(window),
            object: -4,
            child: 0,
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::Found(Box::new(facts("Button"))),
        checked: None,
    }
}

fn read(relation: RelationFound, values: PropertyValues) -> Reply {
    Reply::Object(Box::new(ObjectRead {
        object: NodeRead {
            node: Some(node(7)),
            window: win(5),
            values,
        },
        relation: Some(relation),
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met: None,
        route: None,
    }))
}

fn one(id: PropertyId, value: PropertyValue) -> PropertyValues {
    PropertyValues(vec![PropertyAnswer {
        id,
        value: Some(value),
    }])
}

/// The relation query of the change read out.
fn asked(state: &Held) -> Option<RelationQuery> {
    state
        .queue
        .table_for_tests()
        .into_iter()
        .find_map(|request| match request {
            Wire::ReadObject(read) => read.relation,
            _ => None,
        })
}

#[test]
fn a_name_change_on_the_focus_is_asked_of_its_outpost_and_spoken() {
    let mut state = committed();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            entry(msaa_change(EventKind::NameChange, 5))
        ),
        ["send Outpost(Pid(40)) ReadObject"]
    );
    assert_eq!(asked(&state), Some(RelationQuery::OneOf(vec![node(2)])));
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(
                OUTPOST,
                4,
                read(
                    RelationFound::Is(0),
                    one(PropertyId::Name, PropertyValue::Text("Save".to_owned()))
                )
            )
        ),
        ["Label(\"Save\")"]
    );
    assert_eq!(
        state.focus.object().unwrap().snapshot.name.as_deref(),
        Some("Save")
    );
}

#[test]
fn a_change_outside_the_windows_of_the_focus_and_its_ancestors_is_dropped() {
    let mut state = committed();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            entry(msaa_change(EventKind::NameChange, 9))
        ),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_change_with_nothing_committed_is_dropped() {
    let mut state = crate::entries::tests::ready();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            entry(msaa_change(EventKind::StateChange, 5))
        ),
        NOTHING
    );
}

#[test]
fn an_object_that_is_none_of_the_nodes_says_nothing() {
    let mut state = committed();
    feed_at(
        &mut state,
        4_000,
        entry(msaa_change(EventKind::NameChange, 5)),
    );
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer_outcome(OUTPOST, 4, WireOutcome::NotThere)
        ),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_state_change_names_the_focus_and_its_ancestors_and_speaks_on_an_ancestor() {
    let mut state = committed();
    feed_at(
        &mut state,
        4_000,
        entry(msaa_change(EventKind::SelectionAdd, 5)),
    );
    assert_eq!(
        asked(&state),
        Some(RelationQuery::OneOf(vec![node(2), node(1)]))
    );
    let mut states = StateSet::default();
    states.insert(State::Disabled);
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(
                OUTPOST,
                4,
                read(
                    RelationFound::Is(0),
                    one(PropertyId::States, PropertyValue::States(states))
                )
            )
        ),
        ["State(Disabled)"]
    );
}

#[test]
fn a_plain_selection_of_an_item_the_focus_controls_is_spoken_as_a_focus() {
    let mut state = committed();
    feed_at(
        &mut state,
        4_000,
        entry(msaa_change(EventKind::Selection, 9)),
    );
    assert_eq!(
        asked(&state),
        Some(RelationQuery::Selection {
            focus: node(2),
            nodes: vec![node(2), node(1)],
        })
    );
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(
                OUTPOST,
                4,
                read(RelationFound::Controlled, values("Paris", Role::ListItem))
            )
        ),
        ["Label(\"Paris\")"]
    );
    assert_eq!(state.focus.object().unwrap().snapshot.id, node(2));
}

#[test]
fn a_uia_state_change_on_the_focus_speaks_the_states_its_tell_carries_with_no_read() {
    let mut state = committed();
    let mut states = StateSet::default();
    states.insert(State::Disabled);
    let tell = Event {
        kind: EventKind::UiaPropertyChanged,
        object: EventObject::Uia {
            token: ElementToken(12),
            runtime_id: RuntimeId(vec![42, 1]),
        },
        payload: Payload::PropertyChanged {
            property: PropertyId::States,
            value: Some(PropertyValue::States(states)),
        },
        ..msaa_change(EventKind::UiaPropertyChanged, 5)
    };
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            Input::Entry(crate::entries::Entry::of_tell(LISTENER, tell).unwrap())
        ),
        ["State(Disabled)"]
    );
}

/// A UI Automation property change of `property`, carrying `value`, told
/// by the outpost following the committed focus, whose runtime id is
/// `[42, 1]`.
fn followed_change(property: PropertyId, value: Option<PropertyValue>) -> Event {
    Event {
        kind: EventKind::UiaPropertyChanged,
        object: EventObject::Followed {
            runtime_id: RuntimeId(vec![42, 1]),
        },
        payload: Payload::PropertyChanged { property, value },
        window: TellWindow::NoWindow,
        ..msaa_change(EventKind::UiaPropertyChanged, 5)
    }
}

/// The properties of a node the request out asks, by set.
fn asked_properties(state: &Held) -> Option<Wire> {
    state
        .queue
        .table_for_tests()
        .into_iter()
        .find(|request| matches!(request, Wire::Properties { .. }))
}

#[test]
fn a_followed_name_change_on_the_focus_speaks_the_name_its_tell_carries_with_no_read() {
    let mut state = committed();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            Input::Entry(
                crate::entries::Entry::of_tell(
                    OUTPOST,
                    followed_change(
                        PropertyId::Name,
                        Some(PropertyValue::Text("Saved".to_owned()))
                    )
                )
                .unwrap()
            )
        ),
        ["Label(\"Saved\")"]
    );
}

#[test]
fn a_followed_help_text_change_reads_the_focus_s_description_and_speaks_it() {
    let mut state = committed();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            Input::Entry(
                crate::entries::Entry::of_tell(
                    OUTPOST,
                    followed_change(
                        PropertyId::Description,
                        Some(PropertyValue::Text("carried".to_owned()))
                    )
                )
                .unwrap()
            )
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        asked_properties(&state),
        Some(Wire::Properties {
            node: node(2),
            set: vec![PropertyId::Description],
        })
    );
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(
                OUTPOST,
                4,
                Reply::Properties(one(
                    PropertyId::Description,
                    PropertyValue::Text("Type to search".to_owned())
                ))
            )
        ),
        ["Description(\"Type to search\")"]
    );
}

#[test]
fn a_followed_help_text_change_on_an_ancestor_asks_nothing() {
    let mut state = committed();
    let tell = Event {
        object: EventObject::Followed {
            runtime_id: RuntimeId(vec![42, 9]),
        },
        ..followed_change(PropertyId::Description, None)
    };
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            Input::Entry(crate::entries::Entry::of_tell(OUTPOST, tell).unwrap())
        ),
        NOTHING
    );
    assert_eq!(asked_properties(&state), None);
}

#[test]
fn a_followed_controller_for_change_reads_whether_suggestions_are_open() {
    for (open, said) in [
        (Some(true), vec!["Earcon(SuggestionsOpened)"]),
        (Some(false), vec!["Earcon(SuggestionsClosed)"]),
        (None, Vec::new()),
    ] {
        let mut state = committed();
        assert_eq!(
            feed_at(
                &mut state,
                4_000,
                Input::Entry(
                    crate::entries::Entry::of_tell(
                        OUTPOST,
                        followed_change(PropertyId::Suggestions, None)
                    )
                    .unwrap()
                )
            ),
            ["send Outpost(Pid(40)) other"]
        );
        assert_eq!(
            asked_properties(&state),
            Some(Wire::Properties {
                node: node(2),
                set: vec![PropertyId::Suggestions],
            })
        );
        let effects = crate::entries::feed(
            &mut state.state,
            &mut state.runtime,
            crate::event_queue::Timed {
                at_us: 5_000,
                input: answer(
                    OUTPOST,
                    4,
                    Reply::Properties(PropertyValues(vec![PropertyAnswer {
                        id: PropertyId::Suggestions,
                        value: open.map(PropertyValue::Flag),
                    }])),
                ),
            },
        )
        .unwrap();
        let earcons: Vec<String> = effects
            .into_iter()
            .filter_map(|effect| match effect {
                crate::event_queue::Effect::Said(verbatim_model::Effect::PlayEarcon(earcon)) => {
                    Some(format!("Earcon({earcon:?})"))
                }
                _ => None,
            })
            .collect();
        assert_eq!(earcons, said, "suggestions read {open:?}");
    }
}

fn uia_states(runtime: i32, states: StateSet) -> Event {
    Event {
        kind: EventKind::UiaPropertyChanged,
        object: EventObject::Uia {
            token: ElementToken(12),
            runtime_id: RuntimeId(vec![42, runtime]),
        },
        payload: Payload::PropertyChanged {
            property: PropertyId::States,
            value: Some(PropertyValue::States(states)),
        },
        ..msaa_change(EventKind::UiaPropertyChanged, 5)
    }
}

#[test]
fn a_focus_under_the_committed_focus_judges_its_waiting_state_tells_first() {
    use verbatim_model::request::{FocusAncestor, FocusNode};
    let mut state = committed();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            entry(crate::entries::tests::uia_tell(3, Vec::new()))
        ),
        ["send Listener ReadObject"]
    );
    let mut expanded = StateSet::default();
    expanded.insert(State::Expanded);
    let name = Event {
        payload: Payload::PropertyChanged {
            property: PropertyId::Name,
            value: Some(PropertyValue::Text("Saved".to_owned())),
        },
        ..uia_states(1, StateSet::default())
    };
    for tell in [uia_states(1, expanded), name] {
        assert_eq!(
            feed_at(
                &mut state,
                4_100,
                Input::Entry(crate::entries::Entry::of_tell(LISTENER, tell).unwrap())
            ),
            NOTHING
        );
    }
    let mut listened = crate::entries::tests::listener_answer();
    if let Some(route) = listened.route.as_mut() {
        route.steps[0].runtime_id = RuntimeId(vec![42, 3]);
    }
    assert_eq!(
        feed_at(
            &mut state,
            4_200,
            answer(LISTENER, 4, Reply::Object(Box::new(listened)))
        ),
        ["send Outpost(Pid(40)) CompleteUia"]
    );
    let mut read = focus_read(None);
    read.focus.read.node = Some(node(3));
    read.focus.read.values = values("Paris", Role::ListItem);
    read.focus.runtime_id = Some(RuntimeId(vec![42, 3]));
    let combo = FocusAncestor {
        node: FocusNode {
            read: NodeRead {
                node: Some(node(2)),
                window: win(5),
                values: values("OK", Role::Button),
            },
            runtime_id: Some(RuntimeId(vec![42, 1])),
        },
        simple_child: false,
        dialog_text: None,
    };
    read.ancestors.insert(0, combo);
    let said = feed_at(
        &mut state,
        4_300,
        answer(OUTPOST, 5, Reply::Focus(Box::new(read))),
    );
    // "expanded" on the committed combo box first, then the focus; the
    // name change is not moved, so it is judged after the commit, on an
    // ancestor, and says nothing (were it moved, "Saved" would be said).
    assert_eq!(
        said,
        [
            "State(Expanded)",
            "Label(\"Paris\")",
            "send Outpost(Pid(40)) Follow"
        ]
    );
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_focus_not_under_the_committed_focus_leaves_its_state_tells_waiting() {
    let mut state = committed();
    feed_at(
        &mut state,
        4_000,
        entry(crate::entries::tests::uia_tell(3, Vec::new())),
    );
    let mut expanded = StateSet::default();
    expanded.insert(State::Expanded);
    feed_at(
        &mut state,
        4_100,
        Input::Entry(crate::entries::Entry::of_tell(LISTENER, uia_states(1, expanded)).unwrap()),
    );
    feed_at(
        &mut state,
        4_200,
        answer(
            LISTENER,
            4,
            Reply::Object(Box::new(crate::entries::tests::listener_answer())),
        ),
    );
    let mut read = focus_read(None);
    read.focus.read.node = Some(node(3));
    read.focus.read.values = values("Paris", Role::ListItem);
    read.focus.runtime_id = Some(RuntimeId(vec![42, 3]));
    // The state change is judged after the commit, on a node that is no
    // longer the focus or its ancestor: nothing is said of it.
    assert_eq!(
        feed_at(
            &mut state,
            4_300,
            answer(OUTPOST, 5, Reply::Focus(Box::new(read)))
        ),
        ["Label(\"Paris\")", "send Outpost(Pid(40)) Follow"]
    );
}

/// `command` pressed as key `key`.
fn press(key: u64, command: verbatim_model::ReviewCommand) -> Input<crate::entries::Entry> {
    use verbatim_model::request::{Press, PressAction};
    Input::Entry(crate::entries::Entry::Press(Box::new(
        crate::keys::entry::PressEntry::new(
            verbatim_model::TraceId::from_raw(70 + key),
            Press {
                key_seq: verbatim_model::KeySeq::new(key),
                pressed_at_us: 0,
                action: PressAction::Command { command, repeat: 0 },
            },
        ),
    )))
}

/// The focus's name read as `name` by the name change's request `number`.
fn named(state: &mut Held, number: u64, name: &str) -> Vec<String> {
    assert_eq!(
        feed_at(state, 8_000, entry(msaa_change(EventKind::NameChange, 5))),
        ["send Outpost(Pid(40)) ReadObject"]
    );
    feed_at(
        state,
        9_000,
        answer(
            OUTPOST,
            number,
            read(
                RelationFound::Is(0),
                one(PropertyId::Name, PropertyValue::Text(name.to_owned())),
            ),
        ),
    )
}

/// Reporting the current object on the focus speaks its name as read now,
/// so the name change's event finds it already spoken, as NVDA's
/// property cache of the one object does.
#[test]
fn a_name_change_the_report_of_the_object_on_the_focus_spoke_is_silent() {
    let mut state = committed();
    let _ = feed_at(
        &mut state,
        4_000,
        press(1, verbatim_model::ReviewCommand::ReportObject),
    );
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(OUTPOST, 4, Reply::Properties(values("Save", Role::Button)))
        ),
        ["Label(\"Save\") Role(Button)"]
    );
    assert_eq!(named(&mut state, 5, "Save"), NOTHING);
}

/// Report focus speaks the focus's name as read now, so the name change's
/// event finds it already spoken.
#[test]
fn a_name_change_the_report_of_the_focus_spoke_is_silent() {
    let mut state = committed();
    let _ = feed_at(
        &mut state,
        4_000,
        press(1, verbatim_model::ReviewCommand::ReportFocus),
    );
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(OUTPOST, 4, Reply::Properties(values("Save", Role::Button)))
        ),
        ["Label(\"Save\") Role(Button)"]
    );
    assert_eq!(named(&mut state, 5, "Save"), NOTHING);
}

/// Moving the navigator to the focus announces the focus, which is then
/// what was last spoken of it, so a name change to that name is silent.
#[test]
fn a_name_change_the_navigators_return_to_the_focus_spoke_is_silent() {
    let mut state = committed();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            press(1, verbatim_model::ReviewCommand::ToFocus)
        ),
        ["Message(MoveToFocus) Label(\"OK\") Role(Button)"]
    );
    assert_eq!(named(&mut state, 4, "OK"), NOTHING);
}

/// Navigating away and back reads the focus's node afresh, a separate
/// object: reporting it leaves what was spoken of the focus alone, so the
/// name change's own event is still spoken.
#[test]
fn a_name_change_a_report_of_a_fetched_copy_of_the_focus_spoke_is_spoken() {
    let mut state = committed();
    let _ = feed_at(
        &mut state,
        4_000,
        press(1, verbatim_model::ReviewCommand::Parent),
    );
    let _ = feed_at(
        &mut state,
        4_100,
        answer(
            OUTPOST,
            4,
            Reply::Navigated(Some(NodeRead {
                node: Some(node(1)),
                window: win(4),
                values: values("Editor", Role::Window),
            })),
        ),
    );
    let _ = feed_at(
        &mut state,
        4_200,
        press(2, verbatim_model::ReviewCommand::FirstChild),
    );
    assert_eq!(
        feed_at(
            &mut state,
            4_300,
            answer(
                OUTPOST,
                5,
                Reply::Navigated(Some(NodeRead {
                    node: Some(node(2)),
                    window: win(5),
                    values: values("Save", Role::Button),
                })),
            ),
        ),
        ["Label(\"Save\") Role(Button)"]
    );
    let _ = feed_at(
        &mut state,
        4_400,
        press(3, verbatim_model::ReviewCommand::ReportObject),
    );
    assert_eq!(
        feed_at(
            &mut state,
            4_500,
            answer(OUTPOST, 6, Reply::Properties(values("Save", Role::Button)))
        ),
        ["Label(\"Save\") Role(Button)"]
    );
    assert_eq!(named(&mut state, 7, "Save"), ["Label(\"Save\")"]);
}

/// Reporting the current object on the focus speaks its states as read
/// now, so the state change's event finds them already spoken.
#[test]
fn a_state_change_the_report_of_the_object_on_the_focus_spoke_is_silent() {
    let mut checked = StateSet::default();
    checked.insert(State::Checked);
    let mut check_box = values("OK", Role::CheckBox);
    check_box.0.push(PropertyAnswer {
        id: PropertyId::States,
        value: Some(PropertyValue::States(checked)),
    });
    let mut state = committed();
    let _ = feed_at(
        &mut state,
        4_000,
        press(1, verbatim_model::ReviewCommand::ReportObject),
    );
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(OUTPOST, 4, Reply::Properties(check_box))
        ),
        ["Label(\"OK\") Role(CheckBox) State(Checked)"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            8_000,
            entry(msaa_change(EventKind::StateChange, 5))
        ),
        ["send Outpost(Pid(40)) ReadObject"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            9_000,
            answer(
                OUTPOST,
                5,
                read(
                    RelationFound::Is(0),
                    one(PropertyId::States, PropertyValue::States(checked))
                )
            )
        ),
        NOTHING
    );
}

/// A result selected in the list the focus controls is spoken as a focus
/// and moves the navigator to it; the focus stays.
#[test]
fn a_controlled_selection_moves_the_navigator_to_the_item() {
    let mut state = committed();
    feed_at(
        &mut state,
        4_000,
        entry(msaa_change(EventKind::Selection, 9)),
    );
    let mut item = values("Paris", Role::ListItem);
    let mut selected = StateSet::default();
    selected.insert(State::Selected);
    item.0.push(PropertyAnswer {
        id: PropertyId::States,
        value: Some(PropertyValue::States(selected)),
    });
    feed_at(
        &mut state,
        5_000,
        answer(OUTPOST, 4, read(RelationFound::Controlled, item.clone())),
    );
    let _ = feed_at(
        &mut state,
        6_000,
        press(1, verbatim_model::ReviewCommand::ReportObject),
    );
    assert_eq!(
        state.queue.table_for_tests().last(),
        Some(&Wire::Properties {
            node: node(7),
            set: crate::report_focus::ANNOUNCED_SET.to_vec(),
        })
    );
    assert_eq!(
        feed_at(
            &mut state,
            7_000,
            answer(OUTPOST, 5, Reply::Properties(item))
        ),
        ["Label(\"Paris\") Role(ListItem) State(Selected)"]
    );
    assert_eq!(state.focus.object().unwrap().snapshot.id, node(2));
}
