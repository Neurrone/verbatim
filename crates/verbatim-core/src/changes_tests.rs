//! Changes on objects at their turn, against a committed UI Automation
//! focus: the button "OK", node 2 in window 5, under the frame "Editor",
//! node 1.

use crate::entries::tests::found_at;
use crate::readers::Found;
use verbatim_model::NodeId;
use verbatim_model::batch::{Done, Op, StepOutcome};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{NodeRead, Outcome as WireOutcome, Reply};
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
        found_at(
            &mut state,
            3_000,
            (OUTPOST, 2),
            Found::Read(focus_read(None))
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

/// The answer to an MSAA change's batch: the object is the node at
/// `place` among those named, its properties `values`.
fn read(place: u32, values: PropertyValues) -> Reply {
    Reply::Steps(vec![
        StepOutcome::Done(Done::IsOneOf(Some(place))),
        StepOutcome::Done(Done::Read(NodeRead {
            node: None,
            window: win(5),
            values,
        })),
    ])
}

fn one(id: PropertyId, value: PropertyValue) -> PropertyValues {
    PropertyValues(vec![PropertyAnswer {
        id,
        value: Some(value),
    }])
}

/// The nodes the change read out asks the object to be, the focus first.
fn asked(state: &Held) -> Option<Vec<NodeId>> {
    state
        .queue
        .batches_for_tests()
        .into_iter()
        .find_map(|(_, batch)| {
            batch.steps.iter().find_map(|step| match &step.op {
                Op::IsOneOf {
                    equal, at_address, ..
                } => Some(equal.iter().chain(at_address).copied().collect()),
                _ => None,
            })
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
    assert_eq!(asked(&state), Some(vec![node(2)]));
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(
                OUTPOST,
                4,
                read(
                    0,
                    one(PropertyId::Name, PropertyValue::Text("Save".to_owned()))
                )
            )
        ),
        ["Label(\"Save\")"]
    );
    assert_eq!(
        state
            .versions
            .focus()
            .unwrap()
            .context
            .snapshot
            .name
            .as_deref(),
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
    assert_eq!(asked(&state), Some(vec![node(2), node(1)]));
    let mut states = StateSet::default();
    states.insert(State::Disabled);
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(
                OUTPOST,
                4,
                read(0, one(PropertyId::States, PropertyValue::States(states)))
            )
        ),
        ["State(Disabled)"]
    );
}

/// A state change whose read does not answer the states it asked, a
/// reader's fault, is refused, where it was once taken as every state
/// removed: the focus keeps its states, so the same states read again say
/// nothing (keys M11).
#[test]
fn a_state_change_read_without_its_states_answered_is_refused() {
    let mut state = committed();
    let mut checked = StateSet::default();
    checked.insert(State::Checked);
    for (number, values, said) in [
        (
            4,
            one(PropertyId::States, PropertyValue::States(checked)),
            vec!["State(Checked)"],
        ),
        (5, PropertyValues::default(), Vec::new()),
        (
            6,
            one(PropertyId::States, PropertyValue::States(checked)),
            Vec::new(),
        ),
    ] {
        feed_at(
            &mut state,
            4_000,
            entry(msaa_change(EventKind::StateChange, 5)),
        );
        let answered = answer(OUTPOST, number, read(0, values));
        assert_eq!(feed_at(&mut state, 5_000, answered), said);
    }
}

/// A plain selection asks whether its item is the focus or an ancestor,
/// with no controller-for relation, since an MSAA object controls
/// nothing; an item that is none of them is read no further and says
/// nothing (the request list's design, section 6).
#[test]
fn a_plain_selection_of_an_item_neither_the_focus_nor_an_ancestor_says_nothing() {
    let mut state = committed();
    feed_at(
        &mut state,
        4_000,
        entry(msaa_change(EventKind::Selection, 9)),
    );
    assert_eq!(asked(&state), Some(vec![node(2), node(1)]));
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(
                OUTPOST,
                4,
                Reply::Steps(vec![
                    StepOutcome::Done(Done::IsOneOf(None)),
                    StepOutcome::Skipped,
                ])
            )
        ),
        NOTHING
    );
    assert_eq!(state.versions.focus().unwrap().context.snapshot.id, node(2));
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
            Input::Entry(crate::entries::Entry::of_tell(LISTENER, tell, false).unwrap())
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

/// The read of a node's properties the request out asks, by set.
fn asked_properties(state: &Held) -> Option<Op> {
    state
        .queue
        .batches_for_tests()
        .into_iter()
        .find_map(|(_, batch)| {
            batch
                .steps
                .iter()
                .find(|step| matches!(step.op, Op::Read { .. }))
                .map(|step| step.op.clone())
        })
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
                    ),
                    false,
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
                    ),
                    false,
                )
                .unwrap()
            )
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        asked_properties(&state),
        Some(Op::Read {
            subject: verbatim_model::batch::Subject::Node(node(2)),
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
                crate::entries::tests::properties(one(
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
            Input::Entry(crate::entries::Entry::of_tell(OUTPOST, tell, false).unwrap())
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
                        followed_change(PropertyId::Suggestions, None),
                        false,
                    )
                    .unwrap()
                )
            ),
            ["send Outpost(Pid(40)) other"]
        );
        assert_eq!(
            asked_properties(&state),
            Some(Op::Read {
                subject: verbatim_model::batch::Subject::Node(node(2)),
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
                    crate::entries::tests::properties(PropertyValues(vec![PropertyAnswer {
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
                verbatim_model::Effect::PlayEarcon(earcon) => Some(format!("Earcon({earcon:?})")),
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
                Input::Entry(crate::entries::Entry::of_tell(LISTENER, tell, false).unwrap())
            ),
            NOTHING
        );
    }
    let listened = crate::entries::tests::listener_answer_of(vec![42, 3]);
    assert_eq!(
        found_at(&mut state, 4_200, (LISTENER, 4), listened),
        ["send Outpost(Pid(40)) Completion"]
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
        uia_layout: false,
        described: false,
    };
    read.ancestors.insert(0, combo);
    let said = found_at(&mut state, 4_300, (OUTPOST, 5), Found::Read(read));
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
        Input::Entry(
            crate::entries::Entry::of_tell(LISTENER, uia_states(1, expanded), false).unwrap(),
        ),
    );
    found_at(
        &mut state,
        4_200,
        (LISTENER, 4),
        crate::entries::tests::listener_answer(),
    );
    let mut read = focus_read(None);
    read.focus.read.node = Some(node(3));
    read.focus.read.values = values("Paris", Role::ListItem);
    read.focus.runtime_id = Some(RuntimeId(vec![42, 3]));
    // The state change is judged after the commit, on a node that is no
    // longer the focus or its ancestor: nothing is said of it.
    assert_eq!(
        found_at(&mut state, 4_300, (OUTPOST, 5), Found::Read(read)),
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
                0,
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
            answer(
                OUTPOST,
                4,
                crate::entries::tests::properties(values("Save", Role::Button))
            )
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
            answer(
                OUTPOST,
                4,
                crate::entries::tests::properties(values("Save", Role::Button))
            )
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
            crate::entries::tests::related(Some(NodeRead {
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
                crate::entries::tests::related(Some(NodeRead {
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
            answer(
                OUTPOST,
                6,
                crate::entries::tests::properties(values("Save", Role::Button))
            )
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
            answer(OUTPOST, 4, crate::entries::tests::properties(check_box))
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
                read(0, one(PropertyId::States, PropertyValue::States(checked)))
            )
        ),
        NOTHING
    );
}

/// The facts of a window outside the foreground window, which acceptance
/// refuses: not under it, nor sharing its root owner, nor topmost.
fn background_facts() -> verbatim_model::tell::AcceptanceFacts {
    let mut facts = crate::entries::tests::facts("Button");
    facts.window_to_foreground = verbatim_model::tell::Relation::Outside;
    facts.root_owner_to_foreground = verbatim_model::tell::Relation::Outside;
    facts.root_owner_is_foregrounds = false;
    facts
}

#[test]
fn a_uia_property_change_of_a_window_acceptance_refuses_says_nothing() {
    for followed in [true, false] {
        let mut state = committed();
        let mut tell = followed_change(
            PropertyId::Name,
            Some(PropertyValue::Text("Saved".to_owned())),
        );
        tell.window = TellWindow::Found(Box::new(background_facts()));
        if !followed {
            tell.object = EventObject::Uia {
                token: ElementToken(5),
                runtime_id: RuntimeId(vec![42, 1]),
            };
        }
        let from = if followed {
            OUTPOST
        } else {
            crate::entries::tests::LISTENER
        };
        assert_eq!(
            feed_at(
                &mut state,
                4_000,
                Input::Entry(crate::entries::Entry::of_tell(from, tell, false).unwrap())
            ),
            NOTHING,
            "followed: {followed}"
        );
    }
}

/// The outpost's tell of a state change of `[42, runtime]` to `states`.
fn followed_states(runtime: i32, states: StateSet) -> Input<crate::entries::Entry> {
    let tell = Event {
        object: EventObject::Followed {
            runtime_id: RuntimeId(vec![42, runtime]),
        },
        ..followed_change(PropertyId::States, Some(PropertyValue::States(states)))
    };
    Input::Entry(crate::entries::Entry::of_tell(OUTPOST, tell, false).unwrap())
}

/// The focus on `[42, 3]`, node 3, told and its listener read out
/// (request 4).
fn focus_on_three(state: &mut Held) {
    assert_eq!(
        feed_at(
            state,
            4_000,
            entry(crate::entries::tests::uia_tell(3, Vec::new()))
        ),
        ["send Listener ReadObject"]
    );
}

/// The listener's read of the focus [`focus_on_three`] asked, answered at
/// `at_us`: the completion asked (request 5).
fn three_routed(state: &mut Held, at_us: u64) {
    let listened = crate::entries::tests::listener_answer_of(vec![42, 3]);
    assert_eq!(
        found_at(state, at_us, (LISTENER, 4), listened),
        ["send Outpost(Pid(40)) Completion"]
    );
}

/// The completion of the focus [`focus_on_three`] asked: node 3, named
/// `name`, of `role`, with `states`, under the frame and, when given, the
/// list "Cities", node 6, runtime id `[42, 6]`.
fn three_read(name: &str, role: Role, states: StateSet, in_cities: bool) -> Found {
    use verbatim_model::request::{FocusAncestor, FocusNode};
    let mut read = focus_read(None);
    read.focus.read.node = Some(node(3));
    read.focus.read.values = values(name, role);
    read.focus.read.values.0.push(PropertyAnswer {
        id: PropertyId::States,
        value: Some(PropertyValue::States(states)),
    });
    read.focus.runtime_id = Some(RuntimeId(vec![42, 3]));
    if in_cities {
        read.ancestors.insert(
            0,
            FocusAncestor {
                node: FocusNode {
                    read: NodeRead {
                        node: Some(node(6)),
                        window: win(5),
                        values: values("Cities", Role::List),
                    },
                    runtime_id: Some(RuntimeId(vec![42, 6])),
                },
                simple_child: false,
                uia_layout: false,
                described: false,
            },
        );
    }
    Found::Read(read)
}

/// The arrival table for an outpost's state change, at its turn on the
/// committed focus: told while the focus on node 3 is read by the
/// listener, before its completion is out, node 3 is on no line and the
/// change says nothing once node 3 is committed.
#[test]
fn a_state_change_of_a_focus_told_before_its_completion_is_silent() {
    let mut state = committed();
    focus_on_three(&mut state);
    let expanded = StateSet::default().with(State::Expanded);
    assert_eq!(
        feed_at(&mut state, 4_100, followed_states(3, expanded)),
        NOTHING
    );
    three_routed(&mut state, 4_200);
    assert_eq!(
        found_at(
            &mut state,
            4_300,
            (OUTPOST, 5),
            three_read("Paris", Role::ListItem, StateSet::default(), false)
        ),
        ["Label(\"Paris\")", "send Outpost(Pid(40)) Follow"]
    );
    assert!(state.queue.front_for_tests().is_none());
}

/// The arrival table's "a kept ancestor at its turn": the list "Cities",
/// an ancestor of the committed focus, expanding is spoken as the
/// ancestor's change when it was on the line at its arrival, and not when
/// it arrived while the focus inside it was read, before any of it was
/// committed.
#[test]
fn an_ancestors_state_change_is_spoken_only_when_on_the_line_at_its_arrival() {
    let expanded = StateSet::default().with(State::Expanded);
    // On the line at arrival: the focus inside "Cities" committed first.
    let mut state = committed();
    focus_on_three(&mut state);
    three_routed(&mut state, 4_200);
    found_at(
        &mut state,
        4_300,
        (OUTPOST, 5),
        three_read("Paris", Role::ListItem, StateSet::default(), true),
    );
    assert_eq!(
        feed_at(&mut state, 4_400, followed_states(6, expanded)),
        ["State(Expanded)"]
    );
    // Off the line at arrival: told while the focus inside it is read.
    let mut state = committed();
    focus_on_three(&mut state);
    assert_eq!(
        feed_at(&mut state, 4_100, followed_states(6, expanded)),
        NOTHING
    );
    three_routed(&mut state, 4_200);
    assert_eq!(
        found_at(
            &mut state,
            4_300,
            (OUTPOST, 5),
            three_read("Paris", Role::ListItem, StateSet::default(), true)
        ),
        [
            "Label(\"Cities\") Role(List)",
            "Label(\"Paris\")",
            "send Outpost(Pid(40)) Follow"
        ]
    );
    assert!(state.queue.front_for_tests().is_none());
}

/// The menu button "File" expanding as soon as it is focused: the outpost
/// registers it as the completion reaches it, reads it collapsed, then
/// tells its expand change before its answer. The focus in flight is on
/// the line, so the change is spoken once, after the focus: "expanded".
#[test]
fn a_state_change_told_while_the_focus_completion_is_out_is_spoken_after_the_focus() {
    let mut state = committed();
    focus_on_three(&mut state);
    three_routed(&mut state, 4_200);
    let focused = StateSet::default()
        .with(State::Focusable)
        .with(State::Focused);
    assert_eq!(
        feed_at(
            &mut state,
            4_250,
            followed_states(3, focused.with(State::Expanded))
        ),
        NOTHING
    );
    assert_eq!(
        found_at(
            &mut state,
            4_300,
            (OUTPOST, 5),
            three_read(
                "File",
                Role::MenuButton,
                focused.with(State::Collapsed),
                false
            )
        ),
        [
            "Label(\"File\") Role(MenuButton) State(Collapsed)",
            "send Outpost(Pid(40)) Follow",
            "State(Expanded)"
        ]
    );
}

/// A followed value change on the focus, of a window acceptance refuses,
/// is a value change to acceptance, which takes it from any window while
/// background progress bars are reported, as NVDA maps a UI Automation
/// value change to its value change event: spoken with the setting on, as
/// when the taskbar comes to the front and the focus stays in a media
/// player's window, and silent with it off; a name change of the same
/// window stays silent with the setting on, the setting being for value
/// changes alone.
#[test]
fn a_followed_value_change_of_a_refused_window_is_spoken_only_with_background_bars_reported() {
    let said = |property, value: &str, background: bool| {
        let mut state = committed();
        state.settings.report_background_progress_bars = background;
        let mut tell = followed_change(property, Some(PropertyValue::Text(value.to_owned())));
        tell.window = TellWindow::Found(Box::new(background_facts()));
        feed_at(
            &mut state,
            4_000,
            Input::Entry(crate::entries::Entry::of_tell(OUTPOST, tell, background).unwrap()),
        )
    };
    assert_eq!(
        said(PropertyId::Value, "1:23", true),
        ["Value(\"1:23\")"],
        "a value change, the setting on"
    );
    assert_eq!(
        said(PropertyId::Value, "1:23", false),
        NOTHING,
        "a value change, the setting off"
    );
    assert_eq!(
        said(PropertyId::Name, "Saved", true),
        NOTHING,
        "a name change, the setting on"
    );
}

/// An MSAA value change in a window unrelated to the foreground, accepted
/// only because background progress bars were reported at its arrival, is
/// accepted apart from the foreground: with the setting on then, a
/// foreground change in another window neither stops its request out nor
/// ends it waiting, as NVDA stops nothing at a foreground change; with the
/// setting off then it is stopped and ended, as one accepted by the
/// foreground relation is.
#[test]
fn a_value_change_accepted_by_background_bars_reported_goes_on_past_a_foreground_change() {
    use crate::event_queue::{Foreground, Logic};
    let mut tell = msaa_change(EventKind::ValueChange, 5);
    tell.window = TellWindow::Found(Box::new(background_facts()));
    let elsewhere = Foreground {
        window: WindowHandle(70),
        root_owner: WindowHandle(70),
        process: Some(verbatim_model::Pid(41)),
        thread: 0,
        by_class: None,
        ..Foreground::BLANK
    };
    let judged = |background: bool| {
        let entry = crate::entries::Entry::of_tell(LISTENER, tell.clone(), background).unwrap();
        let crate::entries::Entry::MsaaChange(change) = &entry else {
            panic!("an MSAA change: {entry:?}");
        };
        let state = crate::state::SrState::new();
        (
            crate::entries::exempt_from_foreground(&entry, &change.window),
            crate::entries::Entries::ends_at_foreground(&state, &entry, &elsewhere),
        )
    };
    assert_eq!(judged(true), (true, false), "the setting on: goes on");
    assert_eq!(judged(false), (false, true), "the setting off: stopped");
}
