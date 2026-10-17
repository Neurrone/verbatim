//! Selection in a list the focus controls, cross-process: a search box whose
//! UIA `ControllerFor` relation names a results list, as the Settings app's
//! search box names its suggestions ("Selection in a list the focus
//! controls" in `docs/nvda/events.md`).
//!
//! An item selected through mockapp's `select` command raises a real
//! `ElementSelected` event; `verbatim_uia_rops::controlled_selection`, the
//! check the outpost makes on such an event, must find it from the search
//! box, one level down or inside a group of the list, and must find
//! nothing for an item outside the controlled list, for the controlled list
//! itself, or from an element that controls nothing. It answers each in one
//! remote operation, and the classic calls behind the same signature
//! answer the same. The focused element is passed in rather than read from
//! the keyboard focus, so the test runs headless.
//!
//! Through a real outpost, a controlled selection is also reported when the
//! element the outpost keeps for the focus came from one of its events,
//! whose cache holds less than a focus read's, with no failed cached read.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::mpsc;

use verbatim_model::{CallCounts, NormalizedEvent, PropertyChange, StateSet};
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor, UiaSnapshotFact};
use verbatim_outpost::{Heard, OutpostOptions};
use verbatim_uia::{
    Cached, Fill, ListenerEvent, Live, Node, Part, Registration, Shape, Subscription, Uia,
};
use verbatim_uia_rops::{ControlledQuery, Path, controlled_selection};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Descendants, UIA_HelpTextPropertyId, UIA_NamePropertyId,
    UIA_SelectionItem_ElementSelectedEventId,
};
use windows::core::BSTR;

use common::outpost::OutpostUnderTest;

/// The element named `name` under `root`, for the remote program to import
/// and its runtime id: nothing of its cache is read.
fn find(uia: &Uia, root: &Live, name: &str) -> IUIAutomationElement {
    let condition = uia
        .property_condition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(name)))
        .expect("name condition");
    let nodes = uia
        .request::<Node>(Node::all_on())
        .expect("a cache request");
    root.find_first(TreeScope_Descendants, &condition, &nodes)
        .unwrap_or_else(|error| panic!("no element named {name:?}: {error}"))
        .unwrap_or_else(|| panic!("no element named {name:?}"))
        .live()
        .element()
        .clone()
}

/// The element `selected` names when it is inside what `focused` controls,
/// asked as the outpost asks, by the remote program and by the classic
/// calls: both must answer the same, the program in one call. The answer
/// as the selected element's name and runtime id, its cache read.
fn controlled(
    uia: &Uia,
    focused: &IUIAutomationElement,
    selected: &[i32],
) -> Option<(Option<String>, Vec<i32>)> {
    let fill = Fill::new(
        &uia.request::<Node>(Node::all_on())
            .expect("a cache request"),
    );
    let query = ControlledQuery {
        focused,
        selected,
        fill: &fill,
    };
    let _ = verbatim_uia::calls::take();
    let (remote, path) = controlled_selection(uia, &query, true).expect("the remote program");
    assert!(matches!(path, Path::Remote), "answered by {}", path.name());
    assert_eq!(
        verbatim_uia::calls::take(),
        CallCounts {
            uia: 1,
            msaa: 0,
            window_messages: 0
        },
        "one remote operation"
    );
    let (classic, _) = controlled_selection(uia, &query, false).expect("the classic calls");
    let answer = |found: Option<Cached<Node>>| {
        found.map(|element| (element.read().name, element.runtime_id()))
    };
    let remote = answer(remote);
    assert_eq!(remote, answer(classic), "both ways answer the same");
    remote
}

fn a_selected_result_is_found_only_inside_the_list_the_search_box_controls() {
    let title = common::unique_title("mockapp-controller-for");
    let mut app = common::spawn("controller.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let (selected_tx, selected) = mpsc::channel();
    let _registration = Registration::with_parts(
        vec![Part::shaped::<ListenerEvent>(Subscription::Event {
            event: UIA_SelectionItem_ElementSelectedEventId,
            callback: Arc::new(move |element: &Cached<ListenerEvent>| {
                // The test may have finished.
                let _ = selected_tx.send((element.read().name, element.runtime_id()));
            }),
        })],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("Registration::with_parts");

    let uia = Uia::new().expect("Uia::new");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    let search = find(&uia, &root, "Search box");
    let other = find(&uia, &root, "Other box");
    let results = find(&uia, &root, "Suggestions");

    // The next selection event, which must be the one for `name`.
    let selected_id = |name: &str| -> Vec<i32> {
        let (selected_name, id) = selected
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|_| panic!("no element-selected event for {name}"));
        assert_eq!(selected_name.as_deref(), Some(name), "the selected element");
        id
    };

    app.send("select result2");
    let result = selected_id("Sound settings");
    assert_eq!(
        controlled(&uia, &search, &result),
        Some((Some("Sound settings".to_owned()), result.clone())),
        "the selected result is inside the list the search box controls"
    );
    app.send("select result1");
    let grouped = selected_id("Display settings");
    assert_eq!(
        controlled(&uia, &search, &grouped),
        Some((Some("Display settings".to_owned()), grouped)),
        "a result inside a group of the list is found too"
    );

    assert_eq!(
        controlled(&uia, &other, &result),
        None,
        "a box that controls nothing finds nothing"
    );
    assert_eq!(
        controlled(&uia, &search, &verbatim_uia::runtime_id(&results)),
        None,
        "the controlled list is not its own descendant"
    );

    app.send("select recent1");
    let elsewhere = selected_id("Elsewhere");
    assert_eq!(
        controlled(&uia, &search, &elsewhere),
        None,
        "an item outside the controlled list is not a controlled selection"
    );

    assert!(
        selected.try_recv().is_err(),
        "one selection event for each select"
    );
    app.quit();
}

/// The outpost keeps the element of a property change on the focus, whose
/// cache holds the focus's property events' properties and none of its
/// details. A selection in the list the focus controls is then checked
/// from that element: the controller's runtime id is UIA's own, kept with
/// the element, so nothing absent from its cache is read. Read the classic
/// way, so every cached read is the outpost's own.
fn a_controlled_selection_from_a_kept_event_element_reads_nothing_uncached() {
    let title = common::unique_title("mockapp-controller-kept");
    let mut app = common::spawn("controller.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::with_options(
        &app,
        OutpostOptions {
            remote_operations: false,
        },
    );
    let uia = Uia::new().expect("Uia::new");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");

    app.send("set-focus search");
    let listener = uia
        .request_fixed::<ListenerEvent>()
        .expect("the listener's cache request");
    let condition = uia
        .property_condition(UIA_NamePropertyId, &VARIANT::from(BSTR::from("Search box")))
        .expect("name condition");
    let search_box = root
        .find_first(TreeScope_Descendants, &condition, &listener)
        .expect("the search")
        .expect("the search box");
    let (judged, _) = outpost.listener_read(&search_box);
    outpost.deliver(judged.into_fact(0));
    let search = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => node.id,
        other => panic!("the outpost said {other:?}, not the search box's focus"),
    };
    // The search box has no text pattern in mockapp.
    assert!(
        matches!(
            outpost.next(),
            OutpostToSupervisor::Event {
                event: NormalizedEvent::NoText { .. },
                ..
            }
        ),
        "the search box has no text"
    );
    outpost.settled();

    app.send("set-description search Type to search");
    outpost.heard(&[Heard::UiaProperty(UIA_HelpTextPropertyId.0)]);
    assert!(
        matches!(
            outpost.next(),
            OutpostToSupervisor::Event {
                event: NormalizedEvent::PropertyChanged {
                    change: PropertyChange::Description(_),
                    ..
                },
                ..
            }
        ),
        "the focus's new description"
    );
    let _ = outpost.take_failed_reads();

    app.send("select result2");
    let result = find(&uia, &root, "Sound settings");
    outpost.deliver(DeliveredFact::UiaSelection {
        hwnd: hwnd.0 as isize,
        focus_window: hwnd.0 as isize,
        snapshot: UiaSnapshotFact {
            runtime_id: verbatim_uia::runtime_id(&result),
            role: verbatim_model::Role::ListItem,
            name: Some("Sound settings".to_owned()),
            value: None,
            states: StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
    });
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::ControlledSelection { controller, node },
            ..
        } => assert_eq!(
            (controller, node.name.as_deref()),
            (search, Some("Sound settings"))
        ),
        other => panic!("the outpost said {other:?}, not the controlled selection"),
    }
    assert_eq!(outpost.take_failed_reads(), 0, "no failed cached read");

    drop(outpost);
    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_selected_result_is_found_only_inside_the_list_the_search_box_controls",
            a_selected_result_is_found_only_inside_the_list_the_search_box_controls,
        ),
        (
            "a_controlled_selection_from_a_kept_event_element_reads_nothing_uncached",
            a_controlled_selection_from_a_kept_event_element_reads_nothing_uncached,
        ),
    ]);
}
