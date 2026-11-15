//! The navigator object's commands as event-queue entries: each resolves
//! the navigator at its turn, makes its reads as the queue's requests on
//! the object's window and holds the queue until its last answer.

use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::request::{
    NodeRead, Outcome as WireOutcome, Press, PressAction, Reply, Request as Wire,
};
use verbatim_model::{
    ActionName, Effect as Said, KeySeq, QueryKind, ReviewCommand, Role, TextOp, TextPoint,
    TextReply, TraceId, WindowHandle,
};

use crate::entries::Entry;
use crate::entries::tests::{
    APP, Held, OUTPOST, answer, answer_outcome, feed_at, focus_read, node, ready, values, win,
};
use crate::event_queue::{Effect, Foreground, Input, Timed};
use crate::keys::entry::PressEntry;

/// The application's window 4 in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: APP,
    thread: 41,
    by_class: None,
};

/// The startup read answered with the button "OK" committed, node 2 in
/// window 5, its `Follow` request 2, so the next request is 3.
fn committed() -> Held {
    let mut state = ready();
    let _ = feed_at(&mut state, 500, Input::Foreground(IN_FRONT));
    let _ = feed_at(
        &mut state,
        600,
        answer(OUTPOST, 1, Reply::Focus(Box::new(focus_read(None)))),
    );
    state
}

/// `command` pressed as key `key`, `repeat` extra times.
fn press(key: u64, command: ReviewCommand, repeat: u8) -> Input<Entry> {
    Input::Entry(Entry::Press(Box::new(PressEntry::new(
        TraceId::from_raw(70 + key),
        Press {
            key_seq: KeySeq::new(key),
            pressed_at_us: 0,
            action: PressAction::Command { command, repeat },
        },
    ))))
}

/// The node `number`, `name` of `role`, read in window `window`.
fn reached(number: u64, window: u64, name: &str, role: Role) -> NodeRead {
    NodeRead {
        node: Some(node(number)),
        window: win(window),
        values: values(name, role),
    }
}

/// The answer to request `number`, a navigation reaching `read`.
fn navigated(number: u64, read: Option<NodeRead>) -> Input<Entry> {
    answer(OUTPOST, number, Reply::Navigated(read))
}

#[test]
fn a_navigation_moves_the_navigator_and_announces_the_node_reached() {
    let mut state = committed();
    assert_eq!(
        feed_at(&mut state, 1_000, press(1, ReviewCommand::Parent, 0)),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        state.queue.table_for_tests().last(),
        Some(&Wire::Navigate {
            node: node(2),
            direction: QueryKind::Parent,
            set: crate::report_focus::ANNOUNCED_SET.to_vec(),
        })
    );
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            navigated(3, Some(reached(7, 6, "Buttons", Role::Group)))
        ),
        ["Label(\"Buttons\") Role(Group)"]
    );
    let navigator = state.navigator.as_ref().unwrap();
    assert_eq!(navigator.object.id, node(7));
    assert_eq!(navigator.window.unwrap().handle, WindowHandle(6));
}

#[test]
fn every_navigation_direction_speaks_its_own_edge_message() {
    for (command, message) in [
        (ReviewCommand::Parent, "Message(NoContainingObject)"),
        (ReviewCommand::NextSibling, "Message(NoNextObject)"),
        (ReviewCommand::PreviousSibling, "Message(NoPreviousObject)"),
        (ReviewCommand::FirstChild, "Message(NoObjectsInside)"),
    ] {
        let mut state = committed();
        let _ = feed_at(&mut state, 1_000, press(1, command, 0));
        assert_eq!(feed_at(&mut state, 2_000, navigated(3, None)), [message]);
        assert_eq!(state.navigator.as_ref().unwrap().object.id, node(2));
    }
}

#[test]
fn a_gone_object_says_the_edge_and_an_ended_reader_nothing() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::NextSibling, 0));
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer_outcome(OUTPOST, 3, WireOutcome::NotThere)
        ),
        ["Message(NoNextObject)"]
    );
    let _ = feed_at(&mut state, 3_000, press(2, ReviewCommand::NextSibling, 0));
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            answer_outcome(OUTPOST, 4, WireOutcome::Ended)
        ),
        Vec::<String>::new()
    );
    assert_eq!(state.navigator.as_ref().unwrap().object.id, node(2));
}

/// A second press waits behind the first and runs from where the first
/// moved the navigator, naming the window the first's answer read: NVDA
/// runs its scripts in order.
#[test]
fn a_second_navigation_runs_from_where_the_first_moved_the_navigator() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::Parent, 0));
    assert_eq!(
        feed_at(&mut state, 1_100, press(2, ReviewCommand::Parent, 0)),
        Vec::<String>::new()
    );
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            navigated(3, Some(reached(7, 6, "Buttons", Role::Group)))
        ),
        [
            "Label(\"Buttons\") Role(Group)",
            "send Outpost(Pid(40)) other"
        ]
    );
    assert_eq!(
        state.queue.table_for_tests().last(),
        Some(&Wire::Navigate {
            node: node(7),
            direction: QueryKind::Parent,
            set: crate::report_focus::ANNOUNCED_SET.to_vec(),
        })
    );
}

/// The navigated node's window takes the normal foreground test: a
/// window outside the foreground's tree stops the request.
#[test]
fn a_request_on_a_navigated_node_outside_the_foreground_is_stopped() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::Parent, 0));
    let mut elsewhere = reached(7, 9, "Other", Role::Group);
    elsewhere.window.top_level = WindowHandle(9);
    elsewhere.window.root_owner = WindowHandle(9);
    let _ = feed_at(&mut state, 2_000, navigated(3, Some(elsewhere)));
    let _ = feed_at(&mut state, 3_000, press(2, ReviewCommand::Activate, 0));
    let effects = crate::entries::feed(
        &mut state.state,
        &mut state.runtime,
        Timed {
            at_us: 4_000,
            input: Input::Foreground(IN_FRONT),
        },
    )
    .unwrap();
    assert!(
        effects.iter().any(|effect| matches!(
            effect,
            Effect::Cancel { cancel, .. }
                if cancel.reason == verbatim_model::request::CancelReason::Foreground
        )),
        "{effects:?}"
    );
}

/// An object that may have text is announced without its value; the
/// selection is read, then the line, and with no text its value is said.
#[test]
fn a_navigation_to_an_edit_field_reads_its_text_in_place_of_its_value() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::NextSibling, 0));
    let mut field = reached(8, 5, "Body", Role::EditableText);
    field.values.0.push(PropertyAnswer {
        id: PropertyId::Value,
        value: Some(PropertyValue::Text("one".to_owned())),
    });
    assert_eq!(
        feed_at(&mut state, 2_000, navigated(3, Some(field))),
        [
            "Label(\"Body\") Role(EditableText)",
            "send Outpost(Pid(40)) other"
        ]
    );
    assert_eq!(
        state.queue.table_for_tests().last(),
        Some(&Wire::Text {
            node: node(8),
            op: TextOp::ReadRange {
                start: TextPoint::SelectionStart,
                end: TextPoint::SelectionEnd,
                through: None,
            },
        })
    );
    assert_eq!(
        feed_at(
            &mut state,
            3_000,
            answer(OUTPOST, 4, Reply::Text(TextReply::Unsupported))
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        state.queue.table_for_tests().last(),
        Some(&Wire::Text {
            node: node(8),
            op: crate::editing::navigator_line_op(),
        })
    );
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            answer(OUTPOST, 5, Reply::Text(TextReply::NoText))
        ),
        ["Value(\"one\")"]
    );
}

#[test]
fn an_activation_says_its_action_activate_or_no_action() {
    for (outcome, said) in [
        (
            WireOutcome::Result(Reply::Activated(Some(ActionName::Named(
                "press".to_owned(),
            )))),
            vec!["Text(\"press\")"],
        ),
        (
            WireOutcome::Result(Reply::Activated(Some(ActionName::Invoke))),
            vec!["Message(Invoke)"],
        ),
        (
            WireOutcome::Result(Reply::Activated(None)),
            vec!["Message(Activate)"],
        ),
        (WireOutcome::NotThere, vec!["Message(NoAction)"]),
        (WireOutcome::NotResponding, vec![]),
    ] {
        let mut state = committed();
        assert_eq!(
            feed_at(&mut state, 1_000, press(1, ReviewCommand::Activate, 0)),
            ["send Outpost(Pid(40)) other"]
        );
        assert_eq!(
            state.queue.table_for_tests().last(),
            Some(&Wire::Activate { node: node(2) })
        );
        assert_eq!(
            feed_at(&mut state, 2_000, answer_outcome(OUTPOST, 3, outcome)),
            said
        );
    }
}

#[test]
fn reporting_the_object_reads_it_live_then_announces_spells_and_copies() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::ReportObject, 0));
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer(OUTPOST, 3, Reply::Properties(values("Okay", Role::Button)))
        ),
        ["Label(\"Okay\") Role(Button)"]
    );
    let _ = feed_at(&mut state, 3_000, press(2, ReviewCommand::ReportObject, 1));
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            answer(OUTPOST, 4, Reply::Properties(values("Ok", Role::Button)))
        ),
        ["SpelledCapital(\"O\") Text(\"k\")"]
    );
    let _ = feed_at(&mut state, 5_000, press(3, ReviewCommand::ReportObject, 2));
    let effects = crate::entries::feed(
        &mut state.state,
        &mut state.runtime,
        Timed {
            at_us: 6_000,
            input: answer(OUTPOST, 5, Reply::Properties(values("Ok", Role::Button))),
        },
    )
    .unwrap();
    assert!(
        effects.contains(&Effect::Said(Said::CopyToClipboard("Ok".to_owned()))),
        "{effects:?}"
    );
}

#[test]
fn a_report_of_an_object_not_there_says_nothing() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::ReportObject, 0));
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer_outcome(OUTPOST, 3, WireOutcome::NotThere)
        ),
        Vec::<String>::new()
    );
}

#[test]
fn moving_to_the_focus_says_so_then_the_focus() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::Parent, 0));
    let _ = feed_at(
        &mut state,
        2_000,
        navigated(3, Some(reached(7, 5, "Buttons", Role::Group))),
    );
    assert_eq!(
        feed_at(&mut state, 3_000, press(2, ReviewCommand::ToFocus, 0)),
        ["Message(MoveToFocus) Label(\"OK\") Role(Button)"]
    );
    assert_eq!(state.navigator.as_ref().unwrap().object.id, node(2));
}

#[test]
fn with_no_navigator_a_command_says_so() {
    let mut state = ready();
    assert_eq!(
        feed_at(&mut state, 1_000, press(1, ReviewCommand::Activate, 0)),
        ["Message(NoNavigatorObject)"]
    );
}
