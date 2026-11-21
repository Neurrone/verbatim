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
//! listener's: its fact is captured from the event's element by the
//! listener's own `capture_selection` in the callback, and checked on the test's
//! thread by the listener's reading thread's check (`FocusReader::check_fact`,
//! through the harness), which finds a windowless item's nearest window, as
//! the listener's reader does, from the element handed over in its agile
//! form. The exception is the item named late: its event's cached name must
//! be empty, which mockapp's event for a named item cannot carry, so that
//! fact is built by hand.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::mpsc;

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::tell::Payload;
use verbatim_model::{NodeId, Role, SegmentContent, StateSet};
use verbatim_uia::{
    Agile, Cached, ListenerEvent, ListenerSelection, Part, Registration, Subscription, Uia,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::UIA_SelectionItem_ElementSelectedEventId;

use common::core_driver::{CoreDriver, Step};
use common::outpost::OutpostUnderTest;

/// The element mockapp reports as having the keyboard focus, built with the
/// listener's cache request, as a focus event's element arrives.
fn focused(hwnd: HWND) -> Cached<ListenerEvent> {
    let uia = Uia::new().expect("a UIA client");
    common::listener_focus(&uia, hwnd.0 as isize)
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
/// the outpost complete the read as Core asks, and returns the focus's
/// node, role, name and value.
fn focus(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    hwnd: HWND,
    id: &str,
) -> (NodeId, Role, Option<String>, Option<String>) {
    app.send(&format!("set-focus {id}"));
    let focused = outpost.uia_focus_answer(&focused(hwnd));
    outpost.settled();
    let value = match common::outpost::value(&focused.read.focus.read.values, PropertyId::Value) {
        Some(PropertyValue::Text(value)) => Some(value.clone()),
        _ => None,
    };
    (
        focused.node(),
        focused.role(),
        focused.name().map(str::to_owned),
        value,
    )
}

/// The combo box's value is its selected item's name.
fn a_combo_box_without_the_value_pattern_has_its_selected_item_as_its_value() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-value");
    let (_, role, name, value) = focus(&mut app, &outpost, hwnd, "hour");
    assert_eq!(
        (role, name.as_deref(), value.as_deref()),
        (Role::ComboBox, Some("Hour"), Some("Nine"))
    );
    drop(outpost);
    app.quit();
}

/// A list whose class name contains `LoopingSelector` has its selected
/// item as its value too.
fn a_looping_selector_has_its_selected_item_as_its_value() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-looping");
    let (_, role, name, value) = focus(&mut app, &outpost, hwnd, "minute");
    assert_eq!(
        (role, name.as_deref(), value.as_deref()),
        (Role::List, Some("Minute"), Some("Five"))
    );
    drop(outpost);
    app.quit();
}

/// The current focus, read by the outpost itself as Core asks it at
/// startup, answers whether its value comes from its selection, as a focus
/// event's completion does: a combo box without the `Value` pattern's
/// does, one with it does not.
fn the_current_focus_says_whether_its_value_comes_from_its_selection() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-focus-now");
    let mut answers = Vec::new();
    for id in ["hour", "size"] {
        app.send(&format!("set-focus {id}"));
        outpost.read_focus_as(focused(hwnd).live().element());
        let (outcome, calls) = outpost.current_focus();
        let read = common::outpost::Focused::of(outcome, calls).read;
        let value = match common::outpost::value(&read.focus.read.values, PropertyId::Value) {
            Some(PropertyValue::Text(value)) => Some(value.clone()),
            _ => None,
        };
        answers.push((id, value, read.value_from_selection));
    }
    assert_eq!(
        answers,
        [
            ("hour", Some("Nine".to_owned()), true),
            ("size", Some("Large".to_owned()), false)
        ]
    );
    drop(outpost);
    app.quit();
}

/// No speech.
const NOTHING: [Vec<SegmentContent>; 0] = [];

/// mockapp's combo box fixture in its UIA backend, with Core driving an
/// outpost watching it, and the combo box `id` focused and told to Core,
/// as the listener tells its focus: what Core said of the focus.
fn started_on(name: &str, id: &str) -> (common::MockApp, HWND, CoreDriver, Step) {
    let (mut app, hwnd, outpost) = start(name);
    let mut core = CoreDriver::new(&app, outpost);
    app.send(&format!("set-focus {id}"));
    let said = core.uia_focus(&focused(hwnd));
    (app, hwnd, core, said)
}

/// An item of the focused combo box selected, told by the listener with
/// the states its event's cache holds, is a change of the combo box's
/// value, to the item's name: Core speaks the new value.
fn an_item_selected_in_the_focused_combo_box_changes_its_value() {
    let (mut app, hwnd, mut core, _) = started_on("mockapp-combo-box-selection", "hour");
    let selections = Selections::new(hwnd);
    app.send("select ten");
    let (element, states) = selections.next("Ten");
    assert_eq!(
        core.uia_event(
            EventKind::UiaElementSelected,
            &element,
            Some(states),
            Payload::None
        )
        .spoken(),
        [vec![SegmentContent::Value("Ten".to_owned())]]
    );
    drop(core);
    app.quit();
}

/// mockapp's element selected events under its window, subscribed to with
/// the focus listener's cache request: each event's element as the
/// listener's callback narrows it, with the states its cache holds, as the
/// listener's own `capture_selection` reads them.
struct Selections {
    _registration: Registration,
    events: mpsc::Receiver<(Agile<ListenerSelection>, Option<String>, StateSet)>,
}

impl Selections {
    fn new(hwnd: HWND) -> Self {
        let window = hwnd.0 as isize;
        let (sent, events) = mpsc::channel();
        let registration = Registration::with_parts(
            vec![Part::shaped::<ListenerSelection>(Subscription::Event {
                event: UIA_SelectionItem_ElementSelectedEventId,
                callback: Arc::new(move |element: &Cached<ListenerSelection>| {
                    let (_, snapshot) =
                        verbatim_listener::listener::capture_selection(element, &element.read())
                            .expect("mockapp's element has its process");
                    let agile = element.agile().expect("an agile reference");
                    // The test may have finished.
                    let _ = sent.send((agile, snapshot.name, snapshot.states));
                }),
            })],
            common::window_subtree(window),
        )
        .expect("a subscription to element selected events");
        Self {
            _registration: registration,
            events,
        }
    }

    /// The next selection event, which must be of the item named `name`:
    /// its element as the listener tells it, and its cached states.
    fn next(&self, name: &str) -> (Cached<ListenerEvent>, StateSet) {
        let (agile, cached_name, states) = self
            .events
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|_| panic!("no element selected event for {name}"));
        assert_eq!(cached_name.as_deref(), Some(name), "the selected item");
        let element = agile.resolve().expect("the event's element");
        (element.narrow(), states)
    }
}

/// The item's name is read as it is found, not taken from the event's
/// cache, which can hold none for an item named late: the value is the
/// combo box's, read live, the item's own name.
fn the_new_value_is_the_items_name_as_it_is_now() {
    let (mut app, hwnd, mut core, _) = started_on("mockapp-combo-box-late-name", "hour");
    let selections = Selections::new(hwnd);
    app.send("select ten");
    let (element, states) = selections.next("Ten");
    let mut tell = core.checked_tell(
        EventKind::UiaElementSelected,
        &element,
        Some(states),
        Payload::None,
    );
    // The event's cache holds no name for an item named late.
    tell.cached
        .0
        .retain(|answer| answer.id != verbatim_model::property::PropertyId::Name);
    assert_eq!(
        core.tell(tell).spoken(),
        [vec![SegmentContent::Value("Ten".to_owned())]]
    );
    drop(core);
    app.quit();
}

/// An item that has the keyboard focus as it is selected is no change of
/// the combo box's value: its selection is not spoken, and the item is
/// said once, by its focus.
fn an_item_with_the_keyboard_focus_is_said_once_as_the_focus() {
    let (mut app, hwnd, mut core, combo_box) = started_on("mockapp-combo-box-focused-item", "hour");
    let mut said = core.startup.clone();
    said.extend(combo_box.spoken());
    assert_eq!(
        said,
        [
            vec![
                SegmentContent::Label("Mockapp Combo Box Fixture".to_owned()),
                SegmentContent::Role(Role::Window)
            ],
            vec![
                SegmentContent::Label("Hour".to_owned()),
                SegmentContent::Role(Role::ComboBox),
                SegmentContent::Value("Nine".to_owned()),
                SegmentContent::State(verbatim_model::State::Collapsed)
            ]
        ]
    );
    let selections = Selections::new(hwnd);
    app.send("select ten");
    // The event's cache is filled before the item takes the focus, as the
    // item is selected on its way to the focus.
    let (element, states) = selections.next("Ten");
    app.send("set-focus ten");
    assert_eq!(
        core.uia_event(
            EventKind::UiaElementSelected,
            &element,
            Some(states),
            Payload::None
        )
        .spoken(),
        NOTHING
    );
    assert_eq!(
        core.uia_focus(&focused(hwnd)).spoken(),
        [vec![SegmentContent::Label("Ten".to_owned())]]
    );
    drop(core);
    app.quit();
}

/// A selection of an item that is not one of the focused combo box's
/// children is no change of its value, and says nothing.
fn a_selection_outside_the_combo_box_is_no_value_change() {
    let (mut app, hwnd, mut core, _) = started_on("mockapp-combo-box-elsewhere", "hour");
    let selections = Selections::new(hwnd);
    app.send("select fifteen");
    let (element, states) = selections.next("Fifteen");
    assert_eq!(
        core.uia_event(
            EventKind::UiaElementSelected,
            &element,
            Some(states),
            Payload::None
        )
        .spoken(),
        NOTHING
    );
    drop(core);
    app.quit();
}

/// A combo box with the `Value` pattern has its own value, and a selection
/// of its item is no change of it outside a `Windows.UI.Core` window:
/// nothing is said. The outpost reads the facts Core asks for it: the
/// `Value` pattern, its window's class, and no Windows Forms list
/// (`docs/parity/changes-and-selection.md`, "A combo box with the `Value`
/// pattern whose items give its value").
fn a_combo_box_with_the_value_pattern_has_no_value_change_from_a_selection() {
    let (mut app, hwnd, outpost) = start("mockapp-combo-box-value-pattern");
    let (node, _, _, value) = focus(&mut app, &outpost, hwnd, "size");
    assert_eq!(value.as_deref(), Some("Large"));
    let answer = outpost.requested(
        hwnd,
        common::outpost::read(
            node,
            vec![
                PropertyId::HasValuePattern,
                PropertyId::WindowClass,
                PropertyId::ComboBoxList,
            ],
        ),
    );
    assert_eq!(
        common::outpost::values_read(answer.outcome),
        verbatim_model::property::PropertyValues(vec![
            verbatim_model::property::PropertyAnswer {
                id: PropertyId::HasValuePattern,
                value: Some(PropertyValue::Flag(true)),
            },
            verbatim_model::property::PropertyAnswer {
                id: PropertyId::WindowClass,
                value: Some(PropertyValue::Text("VerbatimMockAppWindow".to_owned())),
            },
            verbatim_model::property::PropertyAnswer {
                id: PropertyId::ComboBoxList,
                value: None,
            },
        ])
    );
    let mut core = CoreDriver::new(&app, outpost);
    let _ = core.uia_focus(&focused(hwnd));
    let selections = Selections::new(hwnd);
    app.send("select large");
    let (element, states) = selections.next("Large");
    assert_eq!(
        core.uia_event(
            EventKind::UiaElementSelected,
            &element,
            Some(states),
            Payload::None
        )
        .spoken(),
        NOTHING
    );
    drop(core);
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
            "the_current_focus_says_whether_its_value_comes_from_its_selection",
            the_current_focus_says_whether_its_value_comes_from_its_selection,
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
