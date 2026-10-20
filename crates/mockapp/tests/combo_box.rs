//! A UIA combo box without the `Value` pattern and a looping selector, a
//! time picker's list, through the focus listener's reader and a real
//! outpost: each focus is reported with its first selected item's name as
//! its value, as NVDA reads it (`ComboBoxWithoutValuePattern`,
//! `NVDAObjects/UIA/__init__.py` 1380 to 1392 and 2746 to 2767), and an
//! item of the focused combo box selected later is a change of its value,
//! the item's name (2786 to 2800).
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::{NodeSnapshot, NormalizedEvent, Role};
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor, UiaSnapshotFact};
use verbatim_uia::{Cached, ListenerEvent, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_NamePropertyId};
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
    app.send("select ten");
    outpost.deliver(DeliveredFact::UiaSelection {
        hwnd: hwnd.0 as isize,
        focus_window: hwnd.0 as isize,
        snapshot: UiaSnapshotFact {
            runtime_id: runtime_id_of(hwnd, "Ten"),
            role: Role::ListItem,
            name: Some("Ten".to_owned()),
            value: None,
            states: verbatim_model::StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
    });
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
    ]);
}
