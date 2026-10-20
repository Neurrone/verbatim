//! A UIA combo box without the `Value` pattern and a looping selector, a
//! time picker's list, through the focus listener's reader and a real
//! outpost: each focus is reported with its first selected item's name as
//! its value, as NVDA reads it (`ComboBoxWithoutValuePattern`,
//! `NVDAObjects/UIA/__init__.py` 1380 to 1392 and 2746 to 2767), and an
//! item of the focused combo box selected later is a change of its value,
//! the item's name as it is when found (2786 to 2800), unless the item has
//! the keyboard focus. A selection outside the combo box, and one in a
//! combo box with the `Value` pattern, is no change of its value.
//!
//! Each selection is mockapp's own `ElementSelected` event, raised by its
//! `select` command and caught by a subscription shaped as the focus
//! listener's, its element passed on as the listener's selection fact,
//! except for the item named late: its event's cached name must be empty,
//! which mockapp's event for a named item cannot carry, so that fact is
//! built by hand.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::mpsc;

use verbatim_core::SrState;
use verbatim_model::{
    Backend, Effect, Input, NodeSnapshot, NormalizedEvent, OutpostId, Pid, Role, SegmentContent,
    TraceId,
};
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor, UiaSnapshotFact};
use verbatim_uia::{Cached, ListenerEvent, Part, Registration, Subscription, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    TreeScope_Descendants, UIA_NamePropertyId, UIA_SelectionItem_ElementSelectedEventId,
};
use windows::core::BSTR;

use common::outpost::OutpostUnderTest;

/// The element mockapp reports as having the keyboard focus, built with the
/// listener's cache request, as a focus event's element arrives.
fn focused(hwnd: HWND) -> Cached<ListenerEvent> {
    let uia = Uia::new().expect("a UIA client");
    common::listener_focus(&uia, hwnd.0 as isize)
}

/// The runtime id of mockapp's element named `name`.
fn runtime_id_of(hwnd: HWND, name: &str) -> Vec<i32> {
    let uia = Uia::new().expect("a UIA client");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    let condition = uia
        .property_condition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(name)))
        .expect("a name condition");
    let request = uia
        .request_fixed::<ListenerEvent>()
        .expect("a cache request");
    let found = root
        .find_first(TreeScope_Descendants, &condition, &request)
        .unwrap_or_else(|error| panic!("no element named {name:?}: {error}"))
        .unwrap_or_else(|| panic!("no element named {name:?}"));
    found.runtime_id()
}

/// mockapp's combo box fixture in its UIA backend, with an outpost
/// watching it.
fn start(name: &str) -> (common::MockApp, HWND, OutpostUnderTest) {
    common::init_com();
    let title = common::unique_title(name);
    let app = common::spawn("combo_box.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    (app, hwnd, outpost)
}

/// Moves mockapp's focus to `id`, has the listener's reader read it and
/// the outpost report it, and returns the focus and its selected child,
/// which must be all the outpost said.
fn focus(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    hwnd: HWND,
    id: &str,
) -> (NodeSnapshot, Option<NodeSnapshot>) {
    app.send(&format!("set-focus {id}"));
    outpost.deliver(outpost.listener_fact(&focused(hwnd)));
    let reported = match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    selected_child,
                    ..
                },
            ..
        } => (node, selected_child),
        other => panic!("the outpost said {other:?}, not the focus on {id}"),
    };
    outpost.settled();
    reported
}

/// The combo box's value is its selected item's name, and the item is not
/// reported as its selected child.
fn a_combo_box_without_the_value_pattern_has_its_selected_item_as_its_value() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-value");
    let (node, selected_child) = focus(&mut app, &outpost, hwnd, "hour");
    assert_eq!(
        (node.role, node.name.as_deref(), node.value.as_deref()),
        (Role::ComboBox, Some("Hour"), Some("Nine"))
    );
    assert_eq!(selected_child, None);
    drop(outpost);
    app.quit();
}

/// A list whose class name contains `LoopingSelector` has its selected
/// item as its value too, not as its selected child.
fn a_looping_selector_has_its_selected_item_as_its_value() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-looping");
    let (node, selected_child) = focus(&mut app, &outpost, hwnd, "minute");
    assert_eq!(
        (node.role, node.name.as_deref(), node.value.as_deref()),
        (Role::List, Some("Minute"), Some("Five"))
    );
    assert_eq!(selected_child, None);
    drop(outpost);
    app.quit();
}

/// An item of the focused combo box selected, as the listener passes on
/// its selection event, is a change of the combo box's value, to the
/// item's name.
fn an_item_selected_in_the_focused_combo_box_changes_its_value() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-selection");
    let (combo, _) = focus(&mut app, &outpost, hwnd, "hour");
    let selections = Selections::new(hwnd);
    app.send("select ten");
    outpost.deliver(selections.next("Ten"));
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::ValueChanged { node_id, value },
            ..
        } => assert_eq!((node_id, value.as_deref()), (combo.id, Some("Ten"))),
        other => panic!("the outpost said {other:?}, not the combo box's new value"),
    }
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// mockapp's element selected events under its window, subscribed to with
/// the focus listener's cache request, each passed on as the listener's
/// selection fact: the element's identity, role, name and keyboard focus
/// from the event's cache, and nothing read live.
struct Selections {
    _registration: Registration,
    facts: mpsc::Receiver<DeliveredFact>,
}

impl Selections {
    fn new(hwnd: HWND) -> Self {
        let window = hwnd.0 as isize;
        let (sent, facts) = mpsc::channel();
        let registration = Registration::with_parts(
            vec![Part::shaped::<ListenerEvent>(Subscription::Event {
                event: UIA_SelectionItem_ElementSelectedEventId,
                callback: Arc::new(move |element: &Cached<ListenerEvent>| {
                    let parts = element.read().parts(element.runtime_id());
                    // The test may have finished.
                    let _ = sent.send(DeliveredFact::UiaSelection {
                        hwnd: window,
                        focus_window: window,
                        snapshot: UiaSnapshotFact {
                            runtime_id: parts.runtime_id,
                            role: parts.role,
                            name: parts.name,
                            value: parts.value,
                            states: parts.states,
                            details: parts.details,
                        },
                    });
                }),
            })],
            common::window_subtree(window),
        )
        .expect("a subscription to element selected events");
        Self {
            _registration: registration,
            facts,
        }
    }

    /// The fact for the next selection event, which must be of the item
    /// named `name`.
    fn next(&self, name: &str) -> DeliveredFact {
        let fact = self
            .facts
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|_| panic!("no element selected event for {name}"));
        match &fact {
            DeliveredFact::UiaSelection { snapshot, .. } => {
                assert_eq!(snapshot.name.as_deref(), Some(name), "the selected item");
            }
            other => panic!("{other:?} is no selection"),
        }
        fact
    }
}

/// The listener's selection fact for mockapp's item `name`, its cached
/// name `cached_name`, as the listener sends a selection's identity, role
/// and name alone.
fn selection_of(hwnd: HWND, name: &str, cached_name: Option<&str>) -> DeliveredFact {
    DeliveredFact::UiaSelection {
        hwnd: hwnd.0 as isize,
        focus_window: hwnd.0 as isize,
        snapshot: UiaSnapshotFact {
            runtime_id: runtime_id_of(hwnd, name),
            role: Role::ListItem,
            name: cached_name.map(str::to_owned),
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
    }
}

/// The name of the item the outpost's next message reports selected,
/// which must be a selection, not a change of the focus's value.
fn selected_name(outpost: &OutpostUnderTest) -> Option<String> {
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::SelectionChanged { node },
            ..
        } => node.name,
        other => panic!("the outpost said {other:?}, not a selection"),
    }
}

/// The item's name is read as it is found, not taken from the event's
/// cache, which can hold none for an item named late: the value is the
/// item's own name.
fn the_new_value_is_the_items_name_as_it_is_now() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-late-name");
    let (combo, _) = focus(&mut app, &outpost, hwnd, "hour");
    app.send("select ten");
    outpost.deliver(selection_of(hwnd, "Ten", None));
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::ValueChanged { node_id, value },
            ..
        } => assert_eq!((node_id, value.as_deref()), (combo.id, Some("Ten"))),
        other => panic!("the outpost said {other:?}, not the combo box's new value"),
    }
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// What Core does for an event here: speak an utterance, given by its
/// segments' content, or, for a focus, drop the speech it makes expire.
#[derive(Debug, PartialEq)]
enum Said {
    Speech(Vec<SegmentContent>),
    DropExpiredSpeech,
}

/// Gives Core `event`, its node ids stamped with outpost 1, and returns
/// every effect, in order; any effect but speech and a focus's drop of
/// expired speech fails the test.
fn spoken(state: &mut SrState, mut event: NormalizedEvent) -> Vec<Said> {
    event.assign_outpost(OutpostId(1));
    verbatim_core::reduce(
        state,
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_ms: 0,
            source: Pid(1),
            backend: Backend::Uia,
            window: None,
            event,
        },
    )
    .into_iter()
    .map(|effect| match effect {
        Effect::Speak(utterance) => Said::Speech(
            utterance
                .segments
                .into_iter()
                .map(|segment| segment.content)
                .collect(),
        ),
        Effect::DropExpiredSpeech(_) => Said::DropExpiredSpeech,
        other => panic!("an event makes no {other:?}"),
    })
    .collect()
}

/// The outpost's next message, which must be an event, as its event.
fn next_event(outpost: &OutpostUnderTest) -> NormalizedEvent {
    match outpost.next() {
        OutpostToSupervisor::Event { event, .. } => event,
        other => panic!("the outpost said {other:?}, not an event"),
    }
}

/// An item that has the keyboard focus as it is selected is no change of
/// the combo box's value: its selection, reported as one, is not spoken
/// under a combo box, and the item is said once, by its focus.
fn an_item_with_the_keyboard_focus_is_said_once_as_the_focus() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-focused-item");
    let mut state = SrState::new();
    app.send("set-focus hour");
    outpost.deliver(outpost.listener_fact(&focused(hwnd)));
    let combo_box = next_event(&outpost);
    outpost.settled();
    assert_eq!(
        spoken(&mut state, combo_box),
        [
            Said::DropExpiredSpeech,
            Said::Speech(vec![
                SegmentContent::Label("Mockapp Combo Box Fixture".to_owned()),
                SegmentContent::Role(Role::Window)
            ]),
            Said::Speech(vec![
                SegmentContent::Label("Hour".to_owned()),
                SegmentContent::Role(Role::ComboBox),
                SegmentContent::Value("Nine".to_owned()),
                SegmentContent::State(verbatim_model::State::Collapsed)
            ])
        ]
    );
    let selections = Selections::new(hwnd);
    app.send("select ten");
    // The event's cache is filled before the item takes the focus, as
    // the item is selected on its way to the focus.
    let selected = selections.next("Ten");
    app.send("set-focus ten");
    outpost.deliver(selected);
    let selection = next_event(&outpost);
    match &selection {
        NormalizedEvent::SelectionChanged { node } => {
            assert_eq!(node.name.as_deref(), Some("Ten"));
        }
        other => panic!("the outpost reported {other:?}, not the item's selection"),
    }
    outpost.settled();
    outpost.deliver(outpost.listener_fact(&focused(hwnd)));
    let item = next_event(&outpost);
    outpost.settled();
    assert_eq!(spoken(&mut state, selection), []);
    assert_eq!(
        spoken(&mut state, item),
        [
            Said::DropExpiredSpeech,
            Said::Speech(vec![SegmentContent::Label("Ten".to_owned())])
        ]
    );
    drop(outpost);
    app.quit();
}

/// A selection of an item that is not one of the focused combo box's
/// children is no change of its value.
fn a_selection_outside_the_combo_box_is_no_value_change() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-elsewhere");
    focus(&mut app, &outpost, hwnd, "hour");
    let selections = Selections::new(hwnd);
    app.send("select fifteen");
    outpost.deliver(selections.next("Fifteen"));
    assert_eq!(selected_name(&outpost).as_deref(), Some("Fifteen"));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A combo box with the `Value` pattern has its own value, and a
/// selection of its item is no change of it here.
fn a_combo_box_with_the_value_pattern_has_no_value_change_from_a_selection() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-value-pattern");
    let (node, _) = focus(&mut app, &outpost, hwnd, "size");
    assert_eq!(node.value.as_deref(), Some("Large"));
    let selections = Selections::new(hwnd);
    app.send("select large");
    outpost.deliver(selections.next("Large"));
    assert_eq!(selected_name(&outpost).as_deref(), Some("Large"));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_combo_box_without_the_value_pattern_has_its_selected_item_as_its_value",
            a_combo_box_without_the_value_pattern_has_its_selected_item_as_its_value,
        ),
        (
            "a_looping_selector_has_its_selected_item_as_its_value",
            a_looping_selector_has_its_selected_item_as_its_value,
        ),
        (
            "an_item_selected_in_the_focused_combo_box_changes_its_value",
            an_item_selected_in_the_focused_combo_box_changes_its_value,
        ),
        (
            "the_new_value_is_the_items_name_as_it_is_now",
            the_new_value_is_the_items_name_as_it_is_now,
        ),
        (
            "an_item_with_the_keyboard_focus_is_said_once_as_the_focus",
            an_item_with_the_keyboard_focus_is_said_once_as_the_focus,
        ),
        (
            "a_selection_outside_the_combo_box_is_no_value_change",
            a_selection_outside_the_combo_box_is_no_value_change,
        ),
        (
            "a_combo_box_with_the_value_pattern_has_no_value_change_from_a_selection",
            a_combo_box_with_the_value_pattern_has_no_value_change_from_a_selection,
        ),
    ]);
}
