//! Announcements at their turn, against the committed UI Automation
//! focus of the entries' tests: the button "OK", node 2 in window 5,
//! runtime id `[42, 1]`, under the frame "Editor", node 1.

use crate::entries::tests::found_at;
use crate::readers::Found;
use verbatim_model::batch::{Descendant, Descended, Done, StepOutcome};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{NodeRead, Outcome as WireOutcome, Reply};
use verbatim_model::tell::{ElementToken, Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    NotificationKind, NotificationProcessing, Pid, Role, SpeechPriority, State, StateSet,
    WindowHandle,
};

use super::{Announcement, Kind};
use crate::entries::tests::{
    APP, Held, LISTENER, NOTHING, OUTPOST, SPOKEN_FOLLOWED, answer, answer_outcome,
    asked_completion, entry, facts, feed_at, focus_read, node, ready, uia_tell, values, win,
};

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

/// A UI Automation tell of `kind` on the element `[42, runtime]`, kept
/// behind token 12, in window 5 of `class`.
fn uia(kind: EventKind, runtime: i32, class: &str, payload: Payload) -> Event {
    Event {
        kind,
        object: EventObject::Uia {
            token: ElementToken(12),
            runtime_id: RuntimeId(vec![42, runtime]),
        },
        payload,
        at_us: 4_000,
        window: TellWindow::Found(Box::new(facts(class))),
        ..uia_tell(runtime, Vec::new())
    }
}

/// A notification of `process`'s, processed `processing`.
fn notification(process: Pid, processing: NotificationProcessing, activity: &str) -> Event {
    Event {
        process,
        ..uia(
            EventKind::UiaNotification,
            9,
            "AppPane",
            Payload::Notification {
                text: "Saved".to_owned(),
                kind: NotificationKind::ActionCompleted,
                processing,
                activity: activity.to_owned(),
            },
        )
    }
}

/// An MSAA tell of `kind` on object 3 of window 6, of `class`: a window
/// read through MSAA, beside the focus's window 5, which UI Automation
/// reads.
fn msaa(kind: EventKind, class: &str) -> Event {
    let mut facts = facts(class);
    facts.window.handle = WindowHandle(6);
    Event {
        kind,
        object: EventObject::Msaa {
            window: WindowHandle(6),
            object: 3,
            child: 0,
        },
        cached: PropertyValues::default(),
        checked: None,
        window: TellWindow::Found(Box::new(facts)),
        at_us: 4_000,
        ..uia_tell(1, Vec::new())
    }
}

fn with(mut values: PropertyValues, id: PropertyId, value: PropertyValue) -> PropertyValues {
    values.0.push(PropertyAnswer {
        id,
        value: Some(value),
    });
    values
}

/// An object read: node 7 in window 5, with `values`.
fn read_of(values: PropertyValues) -> NodeRead {
    NodeRead {
        node: Some(node(7)),
        window: win(5),
        values,
    }
}

/// A show's answer: its object read with `values`.
fn shown(values: PropertyValues) -> Reply {
    Reply::Steps(vec![StepOutcome::Done(Done::Read(NodeRead {
        node: None,
        ..read_of(values)
    }))])
}

/// Which of the focus's line an alert is, when the line was asked.
#[derive(Clone, Copy)]
enum Line {
    /// Its place, or none of them.
    Is(Option<u32>),
}

/// An alert's answer: which of the focus's line it is, `line`, and the
/// walk of the alert read with `values` and its `descendants`.
fn alert(values: PropertyValues, line: Line, descendants: Vec<NodeRead>) -> Reply {
    let walked = StepOutcome::Done(Done::Descendants(Descended {
        nodes: std::iter::once(read_of(values))
            .chain(descendants)
            .enumerate()
            .map(|(at, node)| Descendant {
                node,
                parent: (at > 0).then_some(0),
                root: 0,
            })
            .collect(),
        complete: true,
    }));
    let Line::Is(place) = line;
    Reply::Steps(vec![StepOutcome::Done(Done::IsOneOf(place)), walked])
}

#[test]
fn a_notification_from_the_focus_application_is_spoken_with_no_request() {
    let mut state = committed();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            entry(notification(APP, NotificationProcessing::All, ""))
        ),
        ["Text(\"Saved\")"]
    );
}

#[test]
fn a_notification_from_another_application_is_dropped() {
    let mut state = committed();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            entry(notification(Pid(77), NotificationProcessing::All, ""))
        ),
        NOTHING
    );
}

#[test]
fn a_notification_whose_check_found_no_window_is_dropped_but_the_snap_results() {
    let mut state = committed();
    let windowless = Event {
        window: TellWindow::NoWindow,
        ..notification(APP, NotificationProcessing::All, "")
    };
    assert_eq!(feed_at(&mut state, 4_000, entry(windowless)), NOTHING);
    let snap = Event {
        window: TellWindow::NoWindow,
        application: "explorer".to_owned(),
        ..notification(
            Pid(77),
            NotificationProcessing::MostRecent,
            crate::reduce::SNAP_RESULTS_ACTIVITY,
        )
    };
    assert_eq!(feed_at(&mut state, 5_000, entry(snap)), ["Text(\"Saved\")"]);
}

#[test]
fn a_most_recent_notification_cuts_speech_and_another_is_queued() {
    let state = committed();
    let judged = |processing| {
        let tell = notification(APP, processing, "");
        let announcement = Announcement::of_tell(1, tell).unwrap();
        let super::Kind::Notification {
            text,
            processing,
            activity,
            ..
        } = &announcement.kind
        else {
            panic!("a notification: {announcement:?}");
        };
        let notification = (text.as_str(), *processing, activity.as_str());
        match super::notification::judged(&state, &announcement.tell, notification, Some(APP)) {
            Some(verbatim_model::Effect::Speak(utterance, _)) => utterance.priority,
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(
        judged(NotificationProcessing::MostRecent),
        SpeechPriority::Interrupt
    );
    assert_eq!(
        judged(NotificationProcessing::ImportantMostRecent),
        SpeechPriority::Interrupt
    );
    assert_eq!(judged(NotificationProcessing::All), SpeechPriority::Queued);
}

/// What `input` at `at_us` said, each utterance with its priority.
fn said_with_priority(
    state: &mut Held,
    at_us: u64,
    input: crate::event_queue::Input<crate::entries::Entry>,
) -> Vec<(SpeechPriority, String)> {
    crate::entries::feed(
        &mut state.state,
        &mut state.runtime,
        crate::event_queue::Timed { at_us, input },
    )
    .unwrap()
    .into_iter()
    .filter_map(|effect| match effect {
        verbatim_model::Effect::Speak(utterance, _) => Some((
            utterance.priority,
            utterance
                .segments
                .iter()
                .filter(|segment| {
                    !matches!(segment.content, verbatim_model::SegmentContent::Mark(_))
                })
                .map(|segment| format!("{:?}", segment.content))
                .collect::<Vec<_>>()
                .join(" "),
        )),
        _ => None,
    })
    .collect()
}

/// Two notifications of one sender, kind, processing and activity, waiting
/// behind a toast whose read is out, are both spoken in their order, as
/// NVDA handles each: the second does not replace the first.
#[test]
fn two_notifications_of_one_key_waiting_behind_an_ask_are_both_spoken_in_order() {
    let mut state = committed();
    assert_eq!(
        feed_at(&mut state, 4_000, entry(toast())),
        ["send Listener ReadObject"]
    );
    let copying = |text: &str| {
        let mut tell = notification(APP, NotificationProcessing::All, "copy");
        tell.payload = Payload::Notification {
            text: text.to_owned(),
            kind: NotificationKind::ActionCompleted,
            processing: NotificationProcessing::All,
            activity: "copy".to_owned(),
        };
        tell
    };
    assert_eq!(
        feed_at(&mut state, 4_100, entry(copying("Copying 1 of 3"))),
        NOTHING
    );
    assert_eq!(
        feed_at(&mut state, 4_200, entry(copying("Copying 2 of 3"))),
        NOTHING
    );
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer_outcome(LISTENER, 4, WireOutcome::NotThere)
        ),
        ["Text(\"Copying 1 of 3\")", "Text(\"Copying 2 of 3\")"]
    );
}

fn toast() -> Event {
    uia(
        EventKind::UiaWindowOpened,
        30,
        "Windows.UI.Core.CoreWindow",
        Payload::None,
    )
}

#[test]
fn a_toast_is_read_by_the_listener_and_announced_and_its_repeat_within_a_second_dropped() {
    let mut state = committed();
    assert_eq!(
        feed_at(&mut state, 4_000, entry(toast())),
        ["send Listener ReadObject"]
    );
    let read = shown(values("Download complete", Role::Pane));
    assert_eq!(
        feed_at(&mut state, 5_000, answer(LISTENER, 4, read)),
        ["Label(\"Download complete\")"]
    );
    let repeat = Event {
        at_us: 900_000,
        ..toast()
    };
    assert_eq!(feed_at(&mut state, 900_000, entry(repeat)), NOTHING);
    let later = Event {
        at_us: 1_200_000,
        ..toast()
    };
    assert_eq!(
        feed_at(&mut state, 1_200_000, entry(later)),
        ["send Listener ReadObject"]
    );
}

#[test]
fn a_window_opened_outside_a_core_window_is_no_toast() {
    let mut state = committed();
    let tell = uia(EventKind::UiaWindowOpened, 30, "AppPane", Payload::None);
    assert_eq!(feed_at(&mut state, 4_000, entry(tell)), NOTHING);
}

#[test]
fn a_toast_whose_read_fails_is_dropped_unspoken() {
    let mut state = committed();
    feed_at(&mut state, 4_000, entry(toast()));
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer_outcome(LISTENER, 4, WireOutcome::NotThere)
        ),
        NOTHING
    );
    assert!(state.last_toast.is_none());
}

fn selected(runtime: i32, states: StateSet) -> Event {
    Event {
        cached: PropertyValues(vec![PropertyAnswer {
            id: PropertyId::States,
            value: Some(PropertyValue::States(states)),
        }]),
        ..uia(
            EventKind::UiaElementSelected,
            runtime,
            "AppPane",
            Payload::None,
        )
    }
}

/// The arrival table's cell "the committed focus at its turn, on the line
/// at arrival": spoken as the focus's change.
#[test]
fn a_selection_of_the_committed_focus_is_its_state_change_with_no_request() {
    let mut state = committed();
    let mut states = StateSet::default();
    states.insert(State::Selected);
    assert_eq!(
        feed_at(&mut state, 4_000, entry(selected(1, states))),
        ["State(Selected)"]
    );
}

/// A UI Automation focus on `[42, 3]`, "Paris", under `ancestors`, read by
/// the listener and asked of the outpost: the request out is the
/// completion, number 5.
fn paris_asked(state: &mut Held) {
    assert_eq!(
        feed_at(state, 4_000, entry(uia_tell(3, Vec::new()))),
        ["send Listener ReadObject"]
    );
}

/// The completion of the focus [`paris_asked`] asked: "Paris", node 3,
/// runtime id `[42, 3]`, under the frame and `ancestor`, its node 6 and
/// runtime id `[42, 6]`.
fn paris_read(ancestor: Option<(&str, Role)>) -> Found {
    use verbatim_model::request::{FocusAncestor, FocusNode};
    let mut read = focus_read(None);
    read.focus.read.node = Some(node(3));
    read.focus.read.values = values("Paris", Role::ListItem);
    read.focus.runtime_id = Some(RuntimeId(vec![42, 3]));
    if let Some((name, role)) = ancestor {
        read.ancestors.insert(
            0,
            FocusAncestor {
                node: FocusNode {
                    read: NodeRead {
                        node: Some(node(6)),
                        window: win(5),
                        values: values(name, role),
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

/// The listener's read of the focus [`paris_asked`] asked, its walk going
/// on from the application's window.
fn paris_route() -> Found {
    let element = crate::entries::tests::walked_element("OK", Role::Button, vec![42, 3]);
    Found::Walked(
        element,
        Vec::new(),
        verbatim_model::batch::AncestorsEnd::ContinueFrom(WindowHandle(5)),
    )
}

/// The arrival table's cell "the committed focus at its turn, not on the
/// line at arrival": a selection of "Paris" arriving while its own focus is
/// read, before it is committed, is silent at its turn, as NVDA, given the
/// event before its focus was "Paris", makes a new object that is never its
/// focus.
#[test]
fn a_selection_of_a_focus_not_yet_committed_at_its_arrival_is_silent() {
    let mut state = committed();
    paris_asked(&mut state);
    let mut states = StateSet::default();
    states.insert(State::Selected);
    assert_eq!(
        feed_at(&mut state, 4_100, entry(selected(3, states))),
        NOTHING
    );
    assert_eq!(
        found_at(&mut state, 4_200, (LISTENER, 4), paris_route()),
        ["send Outpost(Pid(40)) Completion"]
    );
    assert_eq!(
        found_at(&mut state, 4_300, (OUTPOST, 5), paris_read(None)),
        ["Label(\"Paris\")", "send Outpost(Pid(40)) Follow"]
    );
    assert!(state.queue.front_for_tests().is_none());
}

/// The arrival table's cell "a kept ancestor at its turn, not on the line
/// at arrival": a selection of the list "Cities", arriving while the focus
/// inside it is read and before any of it is committed, is silent once
/// "Cities" is an ancestor of the committed focus.
#[test]
fn a_selection_of_an_ancestor_off_the_line_at_its_arrival_is_silent() {
    let mut state = committed();
    paris_asked(&mut state);
    let mut states = StateSet::default();
    states.insert(State::Selected);
    assert_eq!(
        feed_at(&mut state, 4_100, entry(selected(6, states))),
        NOTHING
    );
    found_at(&mut state, 4_200, (LISTENER, 4), paris_route());
    assert_eq!(
        found_at(
            &mut state,
            4_300,
            (OUTPOST, 5),
            paris_read(Some(("Cities", Role::List)))
        ),
        [
            "Label(\"Cities\") Role(List)",
            "Label(\"Paris\")",
            "send Outpost(Pid(40)) Follow"
        ]
    );
    assert!(state.queue.front_for_tests().is_none());
}

/// The arrival table's cell "neither at its turn, on the line at arrival":
/// a selection of "OK", the committed focus as it arrives behind the focus
/// on "Paris", is no state change once "Paris" is committed, "OK" being
/// neither it nor an ancestor: the item is read, as any other selection,
/// and gone, it says nothing.
#[test]
fn a_selection_of_the_focus_left_before_its_turn_is_no_state_change() {
    let mut state = committed();
    paris_asked(&mut state);
    let mut states = StateSet::default();
    states.insert(State::Selected);
    assert_eq!(
        feed_at(&mut state, 4_100, entry(selected(1, states))),
        NOTHING
    );
    found_at(&mut state, 4_200, (LISTENER, 4), paris_route());
    assert_eq!(
        found_at(&mut state, 4_300, (OUTPOST, 5), paris_read(None)),
        [
            "Label(\"Paris\")",
            "send Outpost(Pid(40)) Follow",
            "send Listener ReadObject"
        ]
    );
    assert_eq!(
        feed_at(
            &mut state,
            4_400,
            answer_outcome(LISTENER, 7, WireOutcome::NotThere)
        ),
        NOTHING
    );
}

/// The listener's answer to its read `number` of the item `[42, 8]`, its
/// walk meeting the committed focus, the first ancestor Core knows, when
/// `in_focus`, else going on from the window.
fn item_read(
    held: &Held,
    number: u64,
    in_focus: bool,
) -> crate::event_queue::Input<crate::entries::Entry> {
    let element = crate::entries::tests::walked_element("Paris", Role::ListItem, vec![42, 8]);
    let focus = verbatim_model::request::KnownAncestor::Uia(RuntimeId(vec![42, 1]));
    crate::entries::tests::walk_answer(held, number, element, in_focus.then_some(&focus))
}

/// [`item_read`] fed at `at_us`: what it sent and said.
fn listened_item(state: &mut Held, at_us: u64, number: u64, in_focus: bool) -> Vec<String> {
    let input = item_read(state, number, in_focus);
    feed_at(state, at_us, input)
}

/// [`item`] answered to request `number` at `at_us`: what it sent and said.
fn answered_item(state: &mut Held, at_us: u64, number: u64, controlled: bool) -> Vec<String> {
    found_at(state, at_us, (OUTPOST, number), item(controlled))
}

/// The item read by its outpost: node 7, and whether the committed focus
/// controls it.
fn item(controlled: bool) -> Found {
    Found::Item(
        NodeRead {
            node: Some(node(7)),
            window: win(5),
            values: values("Paris", Role::ListItem),
        },
        Some(controlled),
    )
}

/// The item was never on the focus line: the arrival fact does not gate a
/// selection inside an object the focus controls.
#[test]
fn a_selected_item_the_focus_controls_is_read_completed_and_spoken_as_a_focus() {
    let mut state = committed();
    assert_eq!(
        feed_at(&mut state, 4_000, entry(selected(8, StateSet::default()))),
        ["send Listener ReadObject"]
    );
    assert_eq!(
        listened_item(&mut state, 5_000, 4, true),
        ["send Outpost(Pid(40)) Completion"]
    );
    assert_eq!(
        answered_item(&mut state, 6_000, 5, true),
        ["Label(\"Paris\")"]
    );
    assert_eq!(
        state
            .navigator
            .as_ref()
            .map(|navigator| navigator.object.id),
        Some(node(7))
    );
}

/// A selected item whose controller-for read fails is answered not there,
/// and Core says nothing for that selection, as NVDA's COM error from the
/// focus's `controllerFor` ends the whole selection event. Source-derived,
/// approved by Dickson on 2026-11-21 (`docs/parity/changes-and-selection.md`).
#[test]
fn a_selected_item_whose_relation_cannot_be_read_says_nothing() {
    let mut state = committed();
    feed_at(&mut state, 4_000, entry(selected(8, StateSet::default())));
    listened_item(&mut state, 5_000, 4, true);
    let Found::Item(read, _) = item(true) else {
        unreachable!("an item read");
    };
    // The controller-for step failed.
    assert_eq!(
        found_at(&mut state, 6_000, (OUTPOST, 5), Found::Item(read, None)),
        NOTHING
    );
    assert_ne!(
        state
            .navigator
            .as_ref()
            .map(|navigator| navigator.object.id),
        Some(node(7))
    );
}

/// The committed focus's `HasValuePattern`, `WindowClass` and `Value`,
/// as its outpost answers them.
fn value_facts(has_value: bool, class: &str, value: &str) -> Reply {
    crate::entries::tests::properties(PropertyValues(vec![
        PropertyAnswer {
            id: PropertyId::HasValuePattern,
            value: Some(PropertyValue::Flag(has_value)),
        },
        PropertyAnswer {
            id: PropertyId::WindowClass,
            value: Some(PropertyValue::Text(class.to_owned())),
        },
        PropertyAnswer {
            id: PropertyId::Value,
            value: Some(PropertyValue::Text(value.to_owned())),
        },
    ]))
}

/// The request out last, by its wire form.
fn last_asked(state: &Held) -> Option<verbatim_model::batch::Op> {
    crate::entries::tests::last_op(state)
}

#[test]
fn a_selected_list_item_whose_parent_is_the_focus_asks_its_value_facts_and_says_nothing_outside_a_core_window()
 {
    let mut state = committed();
    feed_at(&mut state, 4_000, entry(selected(8, StateSet::default())));
    listened_item(&mut state, 5_000, 4, true);
    assert_eq!(
        answered_item(&mut state, 6_000, 5, false),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        last_asked(&state),
        Some(verbatim_model::batch::Op::Read {
            subject: verbatim_model::batch::Subject::Node(node(2)),
            set: vec![
                PropertyId::HasValuePattern,
                PropertyId::WindowClass,
                PropertyId::Value
            ],
        })
    );
    assert_eq!(
        feed_at(
            &mut state,
            7_000,
            answer(OUTPOST, 6, value_facts(true, "AppPane", "Ten"))
        ),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_selected_list_item_of_a_focus_with_the_value_pattern_in_a_core_window_speaks_its_value() {
    let mut state = committed();
    feed_at(&mut state, 4_000, entry(selected(8, StateSet::default())));
    listened_item(&mut state, 5_000, 4, true);
    answered_item(&mut state, 6_000, 5, false);
    assert_eq!(
        feed_at(
            &mut state,
            7_000,
            answer(
                OUTPOST,
                6,
                value_facts(true, "Windows.UI.Core.CoreWindow", "Ten")
            )
        ),
        ["Value(\"Ten\")"]
    );
}

#[test]
fn a_selected_list_item_of_a_core_window_focus_without_the_value_pattern_says_nothing() {
    let mut state = committed();
    feed_at(&mut state, 4_000, entry(selected(8, StateSet::default())));
    listened_item(&mut state, 5_000, 4, true);
    answered_item(&mut state, 6_000, 5, false);
    assert_eq!(
        feed_at(
            &mut state,
            7_000,
            answer(
                OUTPOST,
                6,
                value_facts(false, "Windows.UI.Core.CoreWindow", "Ten")
            )
        ),
        NOTHING
    );
}

/// The committed focus a combo box, "Hour", node 2 in window 5.
fn combo_box_committed() -> Held {
    let mut state = asked_completion();
    let mut read = focus_read(None);
    read.focus.read.values = values("Hour", Role::ComboBox);
    found_at(&mut state, 3_000, (OUTPOST, 2), Found::Read(read));
    state
}

/// A selection of list item `[42, 8]` in window 9, a `ComboLBox` of the
/// focus's process outside the foreground and not topmost, which the
/// acceptance test refuses.
fn combo_list_selected() -> Event {
    let mut facts = facts("ComboLBox");
    facts.window.handle = WindowHandle(9);
    facts.root = crate::entries::tests::facet(9, "ComboLBox");
    facts.root_owner = crate::entries::tests::facet(9, "ComboLBox");
    facts.window_to_foreground = verbatim_model::tell::Relation::Outside;
    facts.root_owner_to_foreground = verbatim_model::tell::Relation::Outside;
    facts.root_owner_is_foregrounds = false;
    facts.window_to_active = verbatim_model::tell::Relation::Outside;
    let mut cached = values("Ten", Role::ListItem);
    cached.0.push(PropertyAnswer {
        id: PropertyId::States,
        value: Some(PropertyValue::States(StateSet::default())),
    });
    Event {
        cached,
        window: TellWindow::Found(Box::new(facts)),
        ..selected(8, StateSet::default())
    }
}

fn combo_box_list(list: Option<u64>) -> Reply {
    crate::entries::tests::properties(PropertyValues(vec![PropertyAnswer {
        id: PropertyId::ComboBoxList,
        value: list.map(|list| PropertyValue::Window(WindowHandle(list))),
    }]))
}

#[test]
fn a_list_item_selected_in_a_windows_forms_combo_boxs_list_speaks_the_combo_boxs_value() {
    let mut state = combo_box_committed();
    assert_eq!(
        feed_at(&mut state, 4_000, entry(combo_list_selected())),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        last_asked(&state),
        Some(verbatim_model::batch::Op::Read {
            subject: verbatim_model::batch::Subject::Node(node(2)),
            set: vec![PropertyId::ComboBoxList],
        })
    );
    // The focus is NVDA's Windows Forms combo box, so the selection is
    // taken outside the foreground and handled as any other first.
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(OUTPOST, 4, combo_box_list(Some(9)))
        ),
        ["send Listener ReadObject"]
    );
    assert_eq!(
        listened_item(&mut state, 6_000, 5, false),
        ["send Outpost(Pid(40)) Completion"]
    );
    assert_eq!(
        answered_item(&mut state, 7_000, 6, false),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        last_asked(&state),
        Some(verbatim_model::batch::Op::Read {
            subject: verbatim_model::batch::Subject::Node(node(2)),
            set: vec![PropertyId::Value],
        })
    );
    assert_eq!(
        feed_at(
            &mut state,
            8_000,
            answer(
                OUTPOST,
                7,
                crate::entries::tests::properties(PropertyValues(vec![PropertyAnswer {
                    id: PropertyId::Value,
                    value: Some(PropertyValue::Text("Ten".to_owned())),
                }]))
            )
        ),
        ["Value(\"Ten\")"]
    );
}

#[test]
fn a_combo_lbox_selection_for_a_combo_box_that_is_not_windows_forms_is_refused() {
    let mut state = combo_box_committed();
    feed_at(&mut state, 4_000, entry(combo_list_selected()));
    assert_eq!(
        feed_at(&mut state, 5_000, answer(OUTPOST, 4, combo_box_list(None))),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn a_selection_in_another_list_of_a_windows_forms_combo_boxs_process_gives_no_value() {
    let mut state = combo_box_committed();
    feed_at(&mut state, 4_000, entry(combo_list_selected()));
    feed_at(
        &mut state,
        5_000,
        answer(OUTPOST, 4, combo_box_list(Some(10))),
    );
    listened_item(&mut state, 6_000, 5, false);
    assert_eq!(answered_item(&mut state, 7_000, 6, false), NOTHING);
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn an_item_of_the_focused_combo_box_without_the_value_pattern_reads_and_speaks_its_value() {
    let mut state = asked_completion();
    let mut read = focus_read(None);
    read.value_from_selection = true;
    read.focus.read.values = values("Hour", Role::ComboBox);
    found_at(&mut state, 3_000, (OUTPOST, 2), Found::Read(read));
    feed_at(&mut state, 4_000, entry(selected(8, StateSet::default())));
    listened_item(&mut state, 5_000, 4, true);
    // Met at place 1 of what Core sends the outpost: the focus's own
    // runtime id, after its node.
    assert_eq!(
        answered_item(&mut state, 6_000, 5, false),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            7_000,
            answer(
                OUTPOST,
                6,
                crate::entries::tests::properties(PropertyValues(vec![PropertyAnswer {
                    id: PropertyId::Value,
                    value: Some(PropertyValue::Text("Ten".to_owned())),
                }]))
            )
        ),
        ["Value(\"Ten\")"]
    );
}

/// Two selections of one item the focus controls, the second arriving
/// while the first is read, are both handled: the item is spoken, then
/// spoken again cutting the first, as NVDA cancels speech and reports the
/// item for each.
#[test]
fn two_selections_of_one_controlled_item_are_each_spoken_cutting_the_last() {
    let mut state = committed();
    assert_eq!(
        feed_at(&mut state, 4_000, entry(selected(8, StateSet::default()))),
        ["send Listener ReadObject"]
    );
    assert_eq!(
        feed_at(&mut state, 4_100, entry(selected(8, StateSet::default()))),
        NOTHING
    );
    assert_eq!(
        listened_item(&mut state, 5_000, 4, true),
        ["send Outpost(Pid(40)) Completion"]
    );
    assert_eq!(
        {
            let input = crate::entries::tests::found(&state, OUTPOST, 5, item(true));
            said_with_priority(&mut state, 6_000, input)
        },
        [(SpeechPriority::Interrupt, "Label(\"Paris\")".to_owned())]
    );
    assert_eq!(
        listened_item(&mut state, 7_000, 6, true),
        ["send Outpost(Pid(40)) Completion"]
    );
    assert_eq!(
        {
            let input = crate::entries::tests::found(&state, OUTPOST, 7, item(true));
            said_with_priority(&mut state, 8_000, input)
        },
        [(SpeechPriority::Interrupt, "Label(\"Paris\")".to_owned())]
    );
}

fn alert_values(role: Role) -> PropertyValues {
    values("Disk full", role)
}

#[test]
fn an_alert_is_read_with_its_descendants_and_spoken_with_those_nvda_speaks() {
    let mut state = committed();
    assert_eq!(
        feed_at(&mut state, 4_000, entry(msaa(EventKind::Alert, "Alert"))),
        ["send Outpost(Pid(40)) ReadObject"]
    );
    let button = NodeRead {
        node: Some(node(8)),
        window: win(5),
        values: with(
            values("Retry", Role::Button),
            PropertyId::States,
            PropertyValue::States({
                let mut states = StateSet::default();
                states.insert(State::Focusable);
                states
            }),
        ),
    };
    let text = NodeRead {
        node: Some(node(9)),
        window: win(5),
        values: values("Free some space", Role::StaticText),
    };
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(
                OUTPOST,
                4,
                alert(
                    alert_values(Role::Alert),
                    Line::Is(None),
                    vec![button, text]
                )
            )
        ),
        [
            "Label(\"Disk full\") Role(Alert)",
            "Label(\"Retry\") Role(Button)"
        ]
    );
}

#[test]
fn an_alert_among_the_focus_ancestors_or_of_another_role_is_dropped() {
    let mut state = committed();
    feed_at(&mut state, 4_000, entry(msaa(EventKind::Alert, "Alert")));
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(
                OUTPOST,
                4,
                alert(alert_values(Role::Alert), Line::Is(Some(1)), Vec::new())
            )
        ),
        NOTHING
    );
    feed_at(&mut state, 6_000, entry(msaa(EventKind::Alert, "Alert")));
    assert_eq!(
        feed_at(
            &mut state,
            7_000,
            answer(
                OUTPOST,
                5,
                alert(alert_values(Role::Pane), Line::Is(None), Vec::new())
            )
        ),
        NOTHING
    );
}

#[test]
fn an_alert_with_a_focus_pending_behind_it_is_moved_once_behind_it() {
    let mut state = ready();
    // A focus at the front, asking.
    assert_eq!(
        feed_at(&mut state, 1_000, entry(uia_tell(1, Vec::new()))),
        ["send Listener ReadObject"]
    );
    assert_eq!(
        feed_at(&mut state, 1_100, entry(msaa(EventKind::Alert, "Alert"))),
        NOTHING
    );
    // A second focus, pending behind the alert.
    assert_eq!(
        feed_at(&mut state, 1_200, entry(uia_tell(3, Vec::new()))),
        NOTHING
    );
    assert_eq!(
        found_at(
            &mut state,
            2_000,
            (LISTENER, 1),
            crate::entries::tests::listener_answer()
        ),
        ["send Outpost(Pid(40)) Completion"]
    );
    // The first focus done, the alert reaches the front and is moved
    // behind the second focus, which is asked at once.
    let said = found_at(
        &mut state,
        3_000,
        (OUTPOST, 2),
        Found::Read(focus_read(None)),
    );
    assert_eq!(
        said.last().map(String::as_str),
        Some("send Listener ReadObject")
    );
    assert!(
        !said
            .iter()
            .any(|said| said == "send Outpost(Pid(40)) ReadObject")
    );
    assert!(matches!(
        state.queue.entries().last(),
        Some(crate::entries::Entry::Announcement(announcement))
            if matches!(announcement.kind, Kind::Alert { moved: true, .. })
    ));
}

#[test]
fn a_help_balloon_shown_is_spoken_and_a_tooltip_is_not() {
    let mut state = committed();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            entry(msaa(EventKind::Show, "tooltips_class32"))
        ),
        ["send Outpost(Pid(40)) ReadObject"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(OUTPOST, 4, shown(values("Updates", Role::HelpBalloon)))
        ),
        ["Label(\"Updates\") Role(HelpBalloon)"]
    );
    feed_at(
        &mut state,
        6_000,
        entry(msaa(EventKind::Show, "tooltips_class32")),
    );
    assert_eq!(
        feed_at(
            &mut state,
            7_000,
            answer(OUTPOST, 5, shown(values("Close", Role::ToolTip)))
        ),
        NOTHING
    );
}

#[test]
fn a_show_of_the_other_accepted_classes_asks_nothing() {
    let mut state = committed();
    assert_eq!(
        feed_at(
            &mut state,
            4_000,
            entry(msaa(EventKind::Show, "TTrayAlert"))
        ),
        NOTHING
    );
}

#[test]
fn a_toast_and_a_show_are_exempt_from_the_foreground_stop_and_a_selection_is_not() {
    let of = |tell| Announcement::of_tell(1, tell).unwrap();
    assert!(of(toast()).exempt_from_foreground());
    assert!(of(msaa(EventKind::Show, "tooltips_class32")).exempt_from_foreground());
    assert!(!of(selected(8, StateSet::default())).exempt_from_foreground());
}
