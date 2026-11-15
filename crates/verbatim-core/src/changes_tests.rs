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
    asked_completion, entry, facts, feed_at, focus_read, node, values,
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
            window: WindowHandle(5),
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
