//! Announcements at their turn (Table 12), against the committed UI
//! Automation focus of the entries' tests: the button "OK", node 2 in
//! window 5, runtime id `[42, 1]`, under the frame "Editor", node 1.

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{
    PropertyAnswer, PropertyId, PropertyValue, PropertyValues, RuntimeId,
};
use verbatim_model::request::{
    NodeRead, ObjectRead, Outcome as WireOutcome, RelationFound, Reply, Route, RouteEnd, RouteStep,
};
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
        feed_at(
            &mut state,
            3_000,
            answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None))))
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

/// An object read: node 7 in window 5, with `values`, `relation` and
/// `descendants`.
fn object(
    values: PropertyValues,
    relation: Option<RelationFound>,
    descendants: Vec<NodeRead>,
) -> Reply {
    Reply::Object(Box::new(ObjectRead {
        object: NodeRead {
            node: Some(node(7)),
            window: win(5),
            values,
        },
        relation,
        descendants,
        ancestors: Vec::new(),
        met: None,
        route: None,
    }))
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
        match super::notification::judged(&state, &announcement, Some(APP)) {
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

#[test]
fn notifications_merge_by_sender_kind_processing_and_activity() {
    let of = |tell| Announcement::of_tell(1, tell).unwrap();
    let waiting = of(notification(APP, NotificationProcessing::All, "a"));
    assert!(waiting.merged_by(&of(notification(APP, NotificationProcessing::All, "a"))));
    assert!(!waiting.merged_by(&of(notification(APP, NotificationProcessing::All, "b"))));
    assert!(!waiting.merged_by(&of(notification(
        APP,
        NotificationProcessing::MostRecent,
        "a"
    ))));
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
    let read = ObjectRead {
        object: NodeRead {
            node: None,
            window: win(5),
            values: values("Download complete", Role::Pane),
        },
        relation: None,
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met: None,
        route: None,
    };
    assert_eq!(
        feed_at(
            &mut state,
            5_000,
            answer(LISTENER, 4, Reply::Object(Box::new(read)))
        ),
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

/// The listener's read of the item `[42, 8]`, its walk meeting the
/// committed focus, the first ancestor Core knows.
fn item_read() -> Reply {
    Reply::Object(Box::new(ObjectRead {
        object: NodeRead {
            node: None,
            window: win(5),
            values: values("Paris", Role::ListItem),
        },
        relation: None,
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met: None,
        route: Some(Route {
            window: WindowHandle(5),
            steps: vec![RouteStep {
                runtime_id: RuntimeId(vec![42, 8]),
                window: None,
                content: true,
            }],
            end: RouteEnd::Known(verbatim_model::request::KnownAncestor::Uia(RuntimeId(
                vec![42, 1],
            ))),
        }),
    }))
}

/// The item completed: node 7, met at `met` of what Core knows.
fn item(relation: RelationFound, met: Option<u32>) -> Reply {
    Reply::Object(Box::new(ObjectRead {
        object: NodeRead {
            node: Some(node(7)),
            window: win(5),
            values: values("Paris", Role::ListItem),
        },
        relation: Some(relation),
        descendants: Vec::new(),
        ancestors: Vec::new(),
        met,
        route: None,
    }))
}

#[test]
fn a_selected_item_the_focus_controls_is_read_completed_and_spoken_as_a_focus() {
    let mut state = committed();
    assert_eq!(
        feed_at(&mut state, 4_000, entry(selected(8, StateSet::default()))),
        ["send Listener ReadObject"]
    );
    assert_eq!(
        feed_at(&mut state, 5_000, answer(LISTENER, 4, item_read())),
        ["send Outpost(Pid(40)) CompleteUia"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            6_000,
            answer(OUTPOST, 5, item(RelationFound::Controlled, None))
        ),
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

/// The committed focus's `HasValuePattern`, `WindowClass` and `Value`,
/// as its outpost answers them.
fn value_facts(has_value: bool, class: &str, value: &str) -> Reply {
    Reply::Properties(PropertyValues(vec![
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
fn last_asked(state: &Held) -> Option<verbatim_model::request::Request> {
    state.queue.table_for_tests().last().cloned()
}

#[test]
fn a_selected_list_item_whose_parent_is_the_focus_asks_its_value_facts_and_says_nothing_outside_a_core_window()
 {
    let mut state = committed();
    feed_at(&mut state, 4_000, entry(selected(8, StateSet::default())));
    feed_at(&mut state, 5_000, answer(LISTENER, 4, item_read()));
    assert_eq!(
        feed_at(
            &mut state,
            6_000,
            answer(OUTPOST, 5, item(RelationFound::Unrelated, Some(1)))
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        last_asked(&state),
        Some(verbatim_model::request::Request::Properties {
            node: node(2),
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
    feed_at(&mut state, 5_000, answer(LISTENER, 4, item_read()));
    feed_at(
        &mut state,
        6_000,
        answer(OUTPOST, 5, item(RelationFound::Unrelated, Some(1))),
    );
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
    feed_at(&mut state, 5_000, answer(LISTENER, 4, item_read()));
    feed_at(
        &mut state,
        6_000,
        answer(OUTPOST, 5, item(RelationFound::Unrelated, Some(1))),
    );
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
    feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(read))),
    );
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
    Reply::Properties(PropertyValues(vec![PropertyAnswer {
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
        Some(verbatim_model::request::Request::Properties {
            node: node(2),
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
        feed_at(&mut state, 6_000, answer(LISTENER, 5, item_read())),
        ["send Outpost(Pid(40)) CompleteUia"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            7_000,
            answer(OUTPOST, 6, item(RelationFound::Unrelated, None))
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        last_asked(&state),
        Some(verbatim_model::request::Request::Properties {
            node: node(2),
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
                Reply::Properties(PropertyValues(vec![PropertyAnswer {
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
    feed_at(&mut state, 6_000, answer(LISTENER, 5, item_read()));
    assert_eq!(
        feed_at(
            &mut state,
            7_000,
            answer(OUTPOST, 6, item(RelationFound::Unrelated, None))
        ),
        NOTHING
    );
    assert!(state.queue.front_for_tests().is_none());
}

#[test]
fn an_item_of_the_focused_combo_box_without_the_value_pattern_reads_and_speaks_its_value() {
    let mut state = asked_completion();
    let mut read = focus_read(None);
    read.value_from_selection = true;
    read.focus.read.values = values("Hour", Role::ComboBox);
    feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(read))),
    );
    feed_at(&mut state, 4_000, entry(selected(8, StateSet::default())));
    feed_at(&mut state, 5_000, answer(LISTENER, 4, item_read()));
    // Met at place 1 of what Core sends the outpost: the focus's own
    // runtime id, after its node.
    assert_eq!(
        feed_at(
            &mut state,
            6_000,
            answer(OUTPOST, 5, item(RelationFound::Unrelated, Some(1)))
        ),
        ["send Outpost(Pid(40)) other"]
    );
    assert_eq!(
        feed_at(
            &mut state,
            7_000,
            answer(
                OUTPOST,
                6,
                Reply::Properties(PropertyValues(vec![PropertyAnswer {
                    id: PropertyId::Value,
                    value: Some(PropertyValue::Text("Ten".to_owned())),
                }]))
            )
        ),
        ["Value(\"Ten\")"]
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
                object(
                    alert_values(Role::Alert),
                    Some(RelationFound::Unrelated),
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
                object(
                    alert_values(Role::Alert),
                    Some(RelationFound::Is(1)),
                    Vec::new()
                )
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
                object(
                    alert_values(Role::Pane),
                    Some(RelationFound::Unrelated),
                    Vec::new()
                )
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
        feed_at(
            &mut state,
            2_000,
            answer(
                LISTENER,
                1,
                Reply::Object(Box::new(crate::entries::tests::listener_answer()))
            )
        ),
        ["send Outpost(Pid(40)) CompleteUia"]
    );
    // The first focus done, the alert reaches the front and is moved
    // behind the second focus, which is asked at once.
    let said = feed_at(
        &mut state,
        3_000,
        answer(OUTPOST, 2, Reply::Focus(Box::new(focus_read(None)))),
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
            answer(
                OUTPOST,
                4,
                object(values("Updates", Role::HelpBalloon), None, Vec::new())
            )
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
            answer(
                OUTPOST,
                5,
                object(values("Close", Role::ToolTip), None, Vec::new())
            )
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
