//! The navigator object's commands as event-queue entries: each resolves
//! the navigator at its turn, makes its reads as the queue's requests on
//! the object's window and holds the queue until its last answer.
use crate::entries::tests::found_at;
use crate::readers::Found;

use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{NodeRead, Outcome as WireOutcome, Press, PressAction, Reply};
use verbatim_model::{
    ActionName, Effect as Said, KeySeq, ReviewCommand, Role, TextPoint, TraceId, WindowHandle,
};

use crate::entries::Entry;
use crate::entries::tests::{
    APP, Held, NOTHING, OUTPOST, answer, answer_outcome, feed_at, focus_read, node, ready,
    ready_at, values, win,
};
use crate::event_queue::{Foreground, Input, Timed};
use crate::keys::entry::PressEntry;
use verbatim_model::Effect;
use verbatim_model::batch::StepRefusal;
use verbatim_model::request::QueueEffect;

/// The application's window 4 in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: Some(APP),
    thread: 41,
    by_class: None,
    ..Foreground::BLANK
};

/// The startup read answered with the button "OK" committed, node 2 in
/// window 5, its `Follow` request 2, so the next request is 3.
fn committed() -> Held {
    let mut state = ready();
    let _ = feed_at(&mut state, 500, Input::Foreground(IN_FRONT));
    let _ = found_at(&mut state, 600, (OUTPOST, 1), Found::Read(focus_read(None)));
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
    answer(OUTPOST, number, crate::entries::tests::related(read))
}

#[test]
fn a_navigation_moves_the_navigator_and_announces_the_node_reached() {
    let mut state = committed();
    assert_eq!(
        feed_at(&mut state, 1_000, press(1, ReviewCommand::Parent, 0)),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        crate::entries::tests::last_op(&state).as_ref(),
        Some(&verbatim_model::batch::Op::Related {
            subject: verbatim_model::batch::Subject::Node(node(2)),
            relation: verbatim_model::batch::Relation::Parent,
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

/// A navigation reaching a node in a window no process could be read as
/// serving, such as a console whose client runs as another user, has no
/// reader to ask about it: the navigator stays where it was and nothing is
/// said.
#[test]
fn a_navigation_reaching_a_window_with_no_serving_process_stays_put() {
    let mut state = committed();
    let before = state.navigator.clone();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::Parent, 0));
    let mut unserved = reached(7, 6, "Buttons", Role::Group);
    unserved.window.process = None;
    assert_eq!(
        feed_at(&mut state, 2_000, navigated(3, Some(unserved))),
        NOTHING
    );
    assert_eq!(state.navigator, before);
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
        crate::entries::tests::last_op(&state).as_ref(),
        Some(&verbatim_model::batch::Op::Related {
            subject: verbatim_model::batch::Subject::Node(node(7)),
            relation: verbatim_model::batch::Relation::Parent,
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
            Effect::Queue(QueueEffect::Cancel { cancel, .. })
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
    let last = |state: &Held| {
        state
            .queue
            .batches_for_tests()
            .last()
            .map(|(_, batch)| batch.clone())
    };
    assert_eq!(
        last(&state),
        Some(crate::text_batches::range(
            node(8),
            (TextPoint::SelectionStart, TextPoint::SelectionEnd),
            None
        ))
    );
    assert_eq!(
        feed_at(
            &mut state,
            3_000,
            answer(
                OUTPOST,
                4,
                crate::text_batches::one(crate::text_batches::refused(StepRefusal::Unsupported))
            )
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        last(&state),
        Some(crate::text_batches::line(node(8), TextPoint::Caret, None))
    );
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            answer(
                OUTPOST,
                5,
                crate::text_batches::one(crate::text_batches::refused(StepRefusal::NoText))
            )
        ),
        ["Value(\"one\")"]
    );
}

#[test]
fn an_activation_says_its_action_activate_or_no_action() {
    for (outcome, said) in [
        (
            WireOutcome::Result(crate::entries::tests::activated(Some(ActionName::Named(
                "press".to_owned(),
            )))),
            vec!["Text(\"press\")"],
        ),
        (
            WireOutcome::Result(crate::entries::tests::activated(Some(ActionName::Invoke))),
            vec!["Message(Invoke)"],
        ),
        (
            WireOutcome::Result(crate::entries::tests::activated(None)),
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
            crate::entries::tests::last_op(&state).as_ref(),
            Some(&verbatim_model::batch::Op::Activate {
                subject: verbatim_model::batch::Subject::Node(node(2)),
            })
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
            answer(
                OUTPOST,
                3,
                crate::entries::tests::properties(values("Okay", Role::Button))
            )
        ),
        ["Label(\"Okay\") Role(Button)"]
    );
    let _ = feed_at(&mut state, 3_000, press(2, ReviewCommand::ReportObject, 1));
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            answer(
                OUTPOST,
                4,
                crate::entries::tests::properties(values("Ok", Role::Button))
            )
        ),
        ["SpelledCapital(\"O\") Text(\"k\")"]
    );
    let _ = feed_at(&mut state, 5_000, press(3, ReviewCommand::ReportObject, 2));
    let effects = crate::entries::feed(
        &mut state.state,
        &mut state.runtime,
        Timed {
            at_us: 6_000,
            input: answer(
                OUTPOST,
                5,
                crate::entries::tests::properties(values("Ok", Role::Button)),
            ),
        },
    )
    .unwrap();
    assert!(
        effects.contains(&Said::CopyToClipboard("Ok".to_owned())),
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

/// `values` with `id` set to `value`.
fn with(mut values: PropertyValues, id: PropertyId, value: PropertyValue) -> PropertyValues {
    values.0.push(PropertyAnswer {
        id,
        value: Some(value),
    });
    values
}

/// The application's edit field "Body", node 8 in window 5.
fn body_field() -> NodeRead {
    reached(8, 5, "Body", Role::EditableText)
}

/// [`committed`] with the navigator moved to [`body_field`], the
/// selection's read out as request 4.
fn on_body_field() -> Held {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::NextSibling, 0));
    assert_eq!(
        feed_at(&mut state, 2_000, navigated(3, Some(body_field()))),
        [
            "Label(\"Body\") Role(EditableText)",
            "send Outpost(Pid(40)) other"
        ]
    );
    state
}

/// A line read's answer, `text` at the caret.
fn line_read(text: &str) -> Reply {
    crate::text_batches::one(crate::text_batches::line_at(text, 200, 0))
}

/// A selection read's answer, `text` selected.
fn selected(text: &str) -> Reply {
    crate::text_batches::one(crate::text_batches::range_text(text))
}

/// Feeds `input` at `at_us`, every effect.
fn feed_all(state: &mut Held, at_us: u64, input: Input<Entry>) -> Vec<Effect> {
    crate::entries::feed(&mut state.state, &mut state.runtime, Timed { at_us, input }).unwrap()
}

#[test]
fn every_direction_from_an_object_not_there_says_its_edge() {
    for (command, message) in [
        (ReviewCommand::Parent, "Message(NoContainingObject)"),
        (ReviewCommand::NextSibling, "Message(NoNextObject)"),
        (ReviewCommand::PreviousSibling, "Message(NoPreviousObject)"),
        (ReviewCommand::FirstChild, "Message(NoObjectsInside)"),
    ] {
        let mut state = committed();
        let _ = feed_at(&mut state, 1_000, press(1, command, 0));
        assert_eq!(
            feed_at(
                &mut state,
                2_000,
                answer_outcome(OUTPOST, 3, WireOutcome::NotThere)
            ),
            [message]
        );
        assert_eq!(state.navigator.as_ref().unwrap().object.id, node(2));
    }
}

/// A navigation its application did not answer, or refused, says nothing
/// and leaves the navigator where it was.
#[test]
fn a_navigation_not_answered_says_nothing_and_leaves_the_navigator_put() {
    for outcome in [
        WireOutcome::NotResponding,
        WireOutcome::Refused(verbatim_model::request::Refusal::Unsupported),
    ] {
        let mut state = committed();
        let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::Parent, 0));
        assert_eq!(
            feed_at(&mut state, 2_000, answer_outcome(OUTPOST, 3, outcome)),
            Vec::<String>::new()
        );
        assert_eq!(state.navigator.as_ref().unwrap().object.id, node(2));
    }
}

/// Move to focus pressed while a navigation waits runs after it: the
/// navigation's node, then the focus.
#[test]
fn moving_to_the_focus_behind_a_navigation_speaks_after_it() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::Parent, 0));
    assert_eq!(
        feed_at(&mut state, 1_100, press(2, ReviewCommand::ToFocus, 0)),
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
            "Message(MoveToFocus) Label(\"OK\") Role(Button)"
        ]
    );
    assert_eq!(state.navigator.as_ref().unwrap().object.id, node(2));
}

#[test]
fn with_no_navigator_each_navigator_command_says_so() {
    for command in [
        ReviewCommand::Parent,
        ReviewCommand::ReportObject,
        ReviewCommand::Activate,
    ] {
        let mut state = ready();
        assert_eq!(
            feed_at(&mut state, 1_000, press(1, command, 0)),
            ["Message(NoNavigatorObject)"],
            "{command:?}"
        );
    }
}

/// The outpost that replaces [`OUTPOST`].
const REPLACEMENT: crate::event_queue::Instance = crate::event_queue::Instance {
    reader: crate::event_queue::Reader::Outpost(APP),
    generation: crate::entries::tests::GENERATION + 1,
};

/// [`committed`], then its outpost ends and the re-take's read finds
/// nothing, a navigator command pressed before the read answers.
fn ended_with(command: ReviewCommand) -> Vec<String> {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, Input::ReaderEnded(OUTPOST));
    let _ = feed_at(&mut state, 1_100, press(1, command, 0));
    assert_eq!(
        ready_at(&mut state, 1_200, REPLACEMENT),
        ["send Outpost(Pid(40)) CurrentFocus"]
    );
    feed_at(
        &mut state,
        1_300,
        answer_outcome(REPLACEMENT, 3, WireOutcome::NotThere),
    )
}

/// The navigator goes with its outpost's nodes, so its commands say there
/// is none, and moving to the ended focus says nothing.
#[test]
fn after_the_navigators_outpost_ends_its_commands_say_there_is_none() {
    assert_eq!(
        ended_with(ReviewCommand::Parent),
        ["Message(NoNavigatorObject)"]
    );
    assert_eq!(
        ended_with(ReviewCommand::ReportObject),
        ["Message(NoNavigatorObject)"]
    );
    assert_eq!(
        ended_with(ReviewCommand::Activate),
        ["Message(NoNavigatorObject)"]
    );
    assert_eq!(ended_with(ReviewCommand::ToFocus), Vec::<String>::new());
}

/// A replacement outpost reporting the same focus takes it silently, and
/// the navigator follows it to the replacement's node.
#[test]
fn the_navigator_follows_a_focus_retaken_silently_from_a_replacement_outpost() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, Input::ReaderEnded(OUTPOST));
    let _ = ready_at(&mut state, 1_100, REPLACEMENT);
    let anew = |id: verbatim_model::NodeId| {
        verbatim_model::NodeId::in_outpost(
            verbatim_model::OutpostId(crate::entries::tests::GENERATION + 1),
            id.number(),
        )
    };
    let mut read = focus_read(None);
    read.focus.read.node = read.focus.read.node.map(anew);
    for ancestor in &mut read.ancestors {
        ancestor.node.read.node = ancestor.node.read.node.map(anew);
    }
    assert_eq!(
        found_at(&mut state, 1_200, (REPLACEMENT, 3), Found::Read(read)),
        ["send Outpost(Pid(40)) Follow"]
    );
    assert_eq!(
        feed_at(&mut state, 1_300, press(1, ReviewCommand::Activate, 0)),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        crate::entries::tests::last_op(&state).as_ref(),
        Some(&verbatim_model::batch::Op::Activate {
            subject: verbatim_model::batch::Subject::Node(anew(node(2))),
        })
    );
}

/// The combo box "Name" with the value "Ann".
fn combo() -> Reply {
    crate::entries::tests::properties(with(
        values("Name", Role::ComboBox),
        PropertyId::Value,
        PropertyValue::Text("Ann".to_owned()),
    ))
}

/// Reporting an object with a value: announced, then its name and value
/// spelled, then copied, as NVDA does.
#[test]
fn reporting_an_object_with_a_value_spells_then_copies_its_name_and_value() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::ReportObject, 0));
    assert_eq!(
        feed_at(&mut state, 2_000, answer(OUTPOST, 3, combo())),
        ["Label(\"Name\") Role(ComboBox) Value(\"Ann\")"]
    );
    let _ = feed_at(&mut state, 3_000, press(2, ReviewCommand::ReportObject, 1));
    assert_eq!(
        feed_at(&mut state, 4_000, answer(OUTPOST, 4, combo())),
        [
            "SpelledCapital(\"N\") Text(\"a\") Text(\"m\") Text(\"e\") Message(Space) \
             SpelledCapital(\"A\") Text(\"n\") Text(\"n\")"
        ]
    );
    let _ = feed_at(&mut state, 5_000, press(3, ReviewCommand::ReportObject, 2));
    let effects = feed_all(&mut state, 6_000, answer(OUTPOST, 5, combo()));
    assert!(
        effects.contains(&Said::CopyToClipboard("Name Ann".to_owned())),
        "{effects:?}"
    );
}

#[test]
fn reporting_the_object_speaks_its_focused_selected_and_read_only_states() {
    let mut states = verbatim_model::StateSet::new();
    for state in [
        verbatim_model::State::Focused,
        verbatim_model::State::Focusable,
        verbatim_model::State::Selectable,
        verbatim_model::State::Selected,
        verbatim_model::State::ReadOnly,
    ] {
        states.insert(state);
    }
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::ReportObject, 0));
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer(
                OUTPOST,
                3,
                crate::entries::tests::properties(with(
                    values("alpha.txt", Role::ListItem),
                    PropertyId::States,
                    PropertyValue::States(states)
                ))
            )
        ),
        ["Label(\"alpha.txt\") Role(ListItem) State(Focused) State(Selected) State(ReadOnly)"]
    );
}

/// Reporting the current object is a query and keeps a list item's role;
/// navigating to one speaks it as NVDA speaks a focus, without it.
#[test]
fn reporting_the_object_speaks_its_role_and_navigating_to_it_does_not() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::ReportObject, 0));
    assert_eq!(
        feed_at(
            &mut state,
            2_000,
            answer(
                OUTPOST,
                3,
                crate::entries::tests::properties(values("alpha.txt", Role::ListItem))
            )
        ),
        ["Label(\"alpha.txt\") Role(ListItem)"]
    );
    let _ = feed_at(&mut state, 3_000, press(2, ReviewCommand::NextSibling, 0));
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            navigated(4, Some(reached(9, 5, "beta.txt", Role::ListItem)))
        ),
        ["Label(\"beta.txt\")"]
    );
}

/// A cell `number` of table 7 in row 1, `column`.
fn cell(number: u64, column: u32) -> NodeRead {
    let mut read = reached(number, 5, "Cell", Role::ListItem);
    read.values = with(
        read.values,
        PropertyId::Cell,
        PropertyValue::Cell(verbatim_model::CellPosition {
            row: 1,
            column,
            table: Some(verbatim_model::TableId(7)),
        }),
    );
    read
}

/// Reporting a table says its counts and forgets the table, so the next
/// cell says its row again.
#[test]
fn reporting_a_table_says_its_counts_and_forgets_the_table() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::FirstChild, 0));
    assert_eq!(
        feed_at(&mut state, 1_100, navigated(3, Some(cell(10, 1)))),
        ["Label(\"Cell\") Row(1) Column(1)"]
    );
    let _ = feed_at(&mut state, 1_200, press(2, ReviewCommand::NextSibling, 0));
    assert_eq!(
        feed_at(&mut state, 1_300, navigated(4, Some(cell(11, 2)))),
        ["Label(\"Cell\") Column(2)"]
    );
    let _ = feed_at(&mut state, 1_400, press(3, ReviewCommand::ReportObject, 0));
    assert_eq!(
        feed_at(
            &mut state,
            1_500,
            answer(
                OUTPOST,
                5,
                crate::entries::tests::properties(with(
                    values("Grid", Role::List),
                    PropertyId::TableCounts,
                    PropertyValue::TableCounts(verbatim_model::TableCounts {
                        rows: 3,
                        columns: 2
                    })
                ))
            )
        ),
        ["Label(\"Grid\") Role(List) Phrase(TableCounts { rows: 3, columns: 2 })"]
    );
    let _ = feed_at(&mut state, 1_600, press(4, ReviewCommand::NextSibling, 0));
    assert_eq!(
        feed_at(&mut state, 1_700, navigated(6, Some(cell(12, 2)))),
        ["Label(\"Cell\") Row(1) Column(2)"]
    );
}

/// The desktop window is named by Core, as NVDA names its desktop object,
/// and reporting it keeps that name.
#[test]
fn the_desktop_window_is_named_by_core_and_keeps_its_name_when_reported() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::Parent, 0));
    let mut desktop = NodeRead {
        node: Some(node(11)),
        window: win(65_552),
        values: PropertyValues(vec![PropertyAnswer {
            id: PropertyId::Role,
            value: Some(PropertyValue::Role(Role::Window)),
        }]),
    };
    desktop.window.top_level = WindowHandle(65_552);
    desktop.window.root_owner = WindowHandle(65_552);
    desktop.window.desktop = true;
    assert_eq!(
        feed_at(&mut state, 2_000, navigated(3, Some(desktop))),
        ["Message(Desktop) Role(Window)"]
    );
    let _ = feed_at(&mut state, 3_000, press(2, ReviewCommand::ReportObject, 0));
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            answer(
                OUTPOST,
                4,
                crate::entries::tests::properties(values("Program Manager", Role::Pane))
            )
        ),
        ["Message(Desktop) Role(Window)"]
    );
}

#[test]
fn a_navigation_to_an_edit_field_with_a_selection_says_it_is_selected() {
    let mut state = on_body_field();
    assert_eq!(
        feed_at(&mut state, 3_000, answer(OUTPOST, 4, selected("second"))),
        ["Phrase(Preselected(Text(\"second\")))"]
    );
}

#[test]
fn a_navigation_to_a_protected_edit_field_reads_its_masked_selection() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::NextSibling, 0));
    let mut field = body_field();
    let mut protected = verbatim_model::StateSet::new();
    protected.insert(verbatim_model::State::Protected);
    field.values = with(
        field.values,
        PropertyId::States,
        PropertyValue::States(protected),
    );
    let _ = feed_at(&mut state, 2_000, navigated(3, Some(field)));
    assert_eq!(
        state
            .queue
            .batches_for_tests()
            .last()
            .map(|(_, batch)| batch.clone()),
        Some(crate::text_batches::range(
            node(8),
            (TextPoint::SelectionStart, TextPoint::SelectionEnd),
            None
        ))
    );
    assert_eq!(
        feed_at(&mut state, 3_000, answer(OUTPOST, 4, selected("******"))),
        ["Phrase(Preselected(Text(\"******\")))"]
    );
}

/// Nothing selected: the line at the caret is read and said in place of
/// the value.
#[test]
fn a_navigation_to_an_edit_field_with_nothing_selected_says_its_caret_line() {
    let mut state = on_body_field();
    assert_eq!(
        feed_at(&mut state, 3_000, answer(OUTPOST, 4, selected(""))),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            answer(OUTPOST, 5, line_read("second line\r\n"))
        ),
        ["Text(\"second line\")"]
    );
}

/// [`on_body_field`], its line read, then report object pressed as key 2,
/// `repeat` extra times, and its live read answered as request 6.
fn reporting_body(repeat: u8) -> Held {
    let mut state = on_body_field();
    let _ = feed_at(&mut state, 3_000, answer(OUTPOST, 4, selected("")));
    let _ = feed_at(&mut state, 4_000, answer(OUTPOST, 5, line_read("x\r\n")));
    let _ = feed_at(
        &mut state,
        5_000,
        press(2, ReviewCommand::ReportObject, repeat),
    );
    assert_eq!(
        feed_at(
            &mut state,
            5_100,
            answer(
                OUTPOST,
                6,
                crate::entries::tests::properties(values("Body", Role::EditableText))
            )
        )
        .last()
        .map(String::as_str),
        Some("send Outpost(Pid(40)) other")
    );
    state
}

#[test]
fn reporting_an_edit_field_says_its_name_and_role_then_its_caret_line() {
    let mut state = reporting_body(0);
    let _ = feed_at(&mut state, 5_200, answer(OUTPOST, 7, selected("")));
    assert_eq!(
        feed_at(
            &mut state,
            5_300,
            answer(OUTPOST, 8, line_read("one two\r\n"))
        ),
        ["Text(\"one two\")"]
    );
}

#[test]
fn reporting_an_edit_field_again_spells_its_name_and_caret_line() {
    let mut state = reporting_body(1);
    let _ = feed_at(&mut state, 5_200, answer(OUTPOST, 7, selected("")));
    assert_eq!(
        feed_at(
            &mut state,
            5_300,
            answer(OUTPOST, 8, line_read("ab cd\r\n"))
        ),
        [
            "SpelledCapital(\"B\") Text(\"o\") Text(\"d\") Text(\"y\") Message(Space) \
             Text(\"a\") Text(\"b\") Message(Space) Text(\"c\") Text(\"d\")"
        ]
    );
}

#[test]
fn reporting_an_edit_field_a_third_time_copies_its_name_and_caret_line() {
    let mut state = reporting_body(2);
    let _ = feed_at(&mut state, 5_200, answer(OUTPOST, 7, selected("")));
    let effects = feed_all(
        &mut state,
        5_300,
        answer(OUTPOST, 8, line_read("ab cd\r\n")),
    );
    assert!(
        effects.contains(&Said::CopyToClipboard("Body ab cd".to_owned())),
        "{effects:?}"
    );
}

#[test]
fn reporting_an_edit_field_with_a_selection_a_third_time_copies_the_selection() {
    let mut state = reporting_body(2);
    let effects = feed_all(&mut state, 5_200, answer(OUTPOST, 7, selected("chosen")));
    assert!(
        effects.contains(&Said::CopyToClipboard("Body chosen".to_owned())),
        "{effects:?}"
    );
}

/// The listener's tell of a UI Automation focus on runtime id `[42, 2]`.
fn cancel_told() -> Input<Entry> {
    crate::entries::tests::entry(crate::entries::tests::uia_tell(2, Vec::new()))
}

/// The listener's answer to request `number`, the element `[42, 2]`.
fn cancel_checked(held: &Held, number: u64) -> Input<Entry> {
    let read = crate::entries::tests::listener_answer_of(vec![42, 2]);
    crate::entries::tests::found(held, crate::entries::tests::LISTENER, number, read)
}

/// The outpost's completion of request `number`: the button "Cancel",
/// node 3, in the frame "Editor".
fn cancel_completed(held: &Held, number: u64) -> Input<Entry> {
    let mut read = focus_read(None);
    read.focus.read.node = Some(node(3));
    read.focus.read.values = values("Cancel", Role::Button);
    read.focus.runtime_id = Some(verbatim_model::property::RuntimeId(vec![42, 2]));
    crate::entries::tests::found(held, OUTPOST, number, Found::Read(read))
}

/// Move to focus pressed while a focus is being read runs after that
/// focus is executed, on it, as NVDA queues gestures with events.
#[test]
fn moving_to_the_focus_while_a_focus_is_read_runs_on_that_focus() {
    let mut state = committed();
    assert_eq!(
        feed_at(&mut state, 1_000, cancel_told()),
        ["send Listener ReadObject"]
    );
    assert_eq!(
        feed_at(&mut state, 1_100, press(1, ReviewCommand::ToFocus, 0)),
        Vec::<String>::new()
    );
    assert_eq!(
        {
            let input = cancel_checked(&state, 3);
            feed_at(&mut state, 1_200, input)
        },
        ["send Outpost(Pid(40)) Completion"]
    );
    assert_eq!(
        {
            let input = cancel_completed(&state, 4);
            feed_at(&mut state, 1_300, input)
        },
        [
            "Label(\"Cancel\") Role(Button)",
            "send Outpost(Pid(40)) Follow",
            "Message(MoveToFocus) Label(\"Cancel\") Role(Button)"
        ]
    );
}

/// The focus read finds its object gone: move to focus runs on the focus
/// committed before it.
#[test]
fn moving_to_the_focus_behind_a_focus_not_there_runs_on_the_focus_before() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, cancel_told());
    let _ = feed_at(&mut state, 1_100, press(1, ReviewCommand::ToFocus, 0));
    let _ = {
        let input = cancel_checked(&state, 3);
        feed_at(&mut state, 1_200, input)
    };
    assert_eq!(
        feed_at(
            &mut state,
            1_300,
            answer_outcome(OUTPOST, 4, WireOutcome::NotThere)
        ),
        [
            "send Outpost(Pid(40)) Follow",
            "Message(MoveToFocus) Label(\"OK\") Role(Button)"
        ]
    );
}

/// The listener ends while its read of a focus is out: move to focus runs
/// on the focus committed before it, then the re-take reads the focus.
#[test]
fn moving_to_the_focus_behind_a_focus_whose_reader_ended_runs_on_the_focus_before() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, cancel_told());
    let _ = feed_at(&mut state, 1_100, press(1, ReviewCommand::ToFocus, 0));
    assert_eq!(
        feed_at(
            &mut state,
            1_200,
            Input::ReaderEnded(crate::entries::tests::LISTENER)
        ),
        [
            "Message(MoveToFocus) Label(\"OK\") Role(Button)",
            "send Outpost(Pid(40)) CurrentFocus"
        ]
    );
}

/// A focus arriving behind a navigation press and a report waits: the
/// navigation speaks, the report reads the navigator it left, then the
/// focus is executed and the navigator follows it.
#[test]
fn a_focus_behind_a_navigation_and_a_report_is_executed_after_them() {
    let mut state = committed();
    let _ = feed_at(&mut state, 1_000, press(1, ReviewCommand::Parent, 0));
    let _ = feed_at(&mut state, 1_100, press(2, ReviewCommand::ReportObject, 0));
    assert_eq!(
        feed_at(&mut state, 1_200, cancel_told()),
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
        crate::entries::tests::last_op(&state).as_ref(),
        Some(&verbatim_model::batch::Op::Read {
            subject: verbatim_model::batch::Subject::Node(node(7)),
            set: crate::report_focus::ANNOUNCED_SET.to_vec(),
        })
    );
    assert_eq!(
        feed_at(
            &mut state,
            3_000,
            answer(
                OUTPOST,
                4,
                crate::entries::tests::properties(values("Buttons", Role::Group))
            )
        ),
        ["Label(\"Buttons\") Role(Group)", "send Listener ReadObject"]
    );
    let _ = {
        let input = cancel_checked(&state, 5);
        feed_at(&mut state, 4_000, input)
    };
    assert_eq!(
        {
            let input = cancel_completed(&state, 6);
            feed_at(&mut state, 5_000, input)
        },
        [
            "Label(\"Cancel\") Role(Button)",
            "send Outpost(Pid(40)) Follow"
        ]
    );
    assert_eq!(state.navigator.as_ref().unwrap().object.id, node(3));
}
