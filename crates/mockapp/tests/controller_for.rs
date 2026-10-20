//! Selection in a list the focus controls, cross-process: a search box whose
//! UIA `ControllerFor` relation names a results list, as the Settings app's
//! search box names its suggestions ("Selection in a list the focus
//! controls" in `docs/nvda/events.md`).
//!
//! An item selected through mockapp's `select` command raises a real
//! `ElementSelected` event; `verbatim_uia_rops::controlled_selection`, the
//! check the outpost makes on such an event, must find it from the search
//! box, one level down or inside a group of the list, and must find
//! nothing for an item outside the controlled list while the list has
//! nothing selected, for the controlled list itself, or from an element
//! that controls nothing. A selection the list does not hold, as Settings
//! raises for a suggestion it has already replaced, is answered with the
//! list's current selection, and so is an item outside the list while the
//! list has one, which pins a difference from NVDA, whose test finds such
//! an item in none of the controlled elements and reports nothing of it
//! (`docs/parity/changes-and-selection.md`, "Selection in a list the focus
//! controls"). It answers each in one remote operation, and the classic
//! calls behind the same signature answer the same. The focused element is passed in rather than read from
//! the keyboard focus, so the test runs headless.
//!
//! Through a real outpost, a controlled selection is also reported when the
//! element the outpost keeps for the focus came from one of its events,
//! whose cache holds less than a focus read's, with no failed cached read.
//! A selection raised for an item the list has since replaced (mockapp's
//! `replace-items`) is reported as the list's current selection, the new
//! item, with its name and position. A selection of the focus itself, of
//! an ancestor reported with it, or of anything while the focus is a list,
//! is reported as an ordinary selection change, never as the list's.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::mpsc;

use verbatim_model::{
    CallCounts, NodeId, NodeSnapshot, NormalizedEvent, PropertyChange, Role, StateSet,
};
use verbatim_outpost::focus_read::Judged;
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor, UiaSnapshotFact};
use verbatim_outpost::{Heard, OutpostOptions};
use verbatim_uia::{
    Agile, Cached, Fill, ListenerEvent, Live, Node, Part, Registration, Shape, Subscription, Uia,
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
        current_instead: true,
    };
    let _ = verbatim_uia::calls::take();
    let (remote, path) = controlled_selection(uia, &query, true).expect("the remote program");
    let (remote_ids, remote) = (remote.controlled, remote.selected);
    assert!(matches!(path, Path::Remote), "answered by {}", path.name());
    assert_eq!(
        verbatim_uia::calls::take(),
        CallCounts {
            uia: 1,
            msaa: 0,
            window_messages: 0,
            connections: 0,
        },
        "one remote operation"
    );
    let (classic, _) = controlled_selection(uia, &query, false).expect("the classic calls");
    assert_eq!(
        remote_ids, classic.controlled,
        "both ways name the same controlled elements"
    );
    let classic = classic.selected;
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
        "an item outside the controlled list, which has nothing selected, is not a controlled selection"
    );

    // The list's own selection stands for an element it does not hold.
    app.send("select result2");
    let current = selected_id("Sound settings");
    let replaced: Vec<i32> = current.iter().copied().chain([1]).collect();
    assert_eq!(
        controlled(&uia, &search, &replaced),
        Some((Some("Sound settings".to_owned()), current.clone())),
        "a selection the list does not hold is answered with its current selection"
    );
    assert_eq!(
        controlled(&uia, &search, &elsewhere),
        Some((Some("Sound settings".to_owned()), current)),
        "so is an item outside the list while the list has a selection, a difference from NVDA \
         (docs/parity/changes-and-selection.md)"
    );
    assert_eq!(
        controlled(&uia, &search, &verbatim_uia::runtime_id(&results)),
        None,
        "the controlled list itself is not answered with its selection"
    );

    assert!(
        selected.try_recv().is_err(),
        "one selection event for each select"
    );
    app.quit();
}

/// Moves mockapp's focus to its element `id`, named `name`, has the
/// listener read and send the focus, and returns the focus's node and the
/// ancestors reported with it once the outpost has reported it.
fn report_focus(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    (uia, root): (&Uia, &Live),
    (id, name): (&str, &str),
) -> (NodeSnapshot, Vec<NodeSnapshot>) {
    app.send(&format!("set-focus {id}"));
    let listener = uia
        .request_fixed::<ListenerEvent>()
        .expect("the listener's cache request");
    let condition = uia
        .property_condition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(name)))
        .expect("name condition");
    let focused = root
        .find_first(TreeScope_Descendants, &condition, &listener)
        .expect("the search")
        .unwrap_or_else(|| panic!("no element named {name:?}"));
    let (judged, _) = outpost.listener_read(&focused);
    outpost.deliver(judged.into_fact(0));
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged {
                node, ancestors, ..
            },
            ..
        } => (node, ancestors),
        other => panic!("the outpost said {other:?}, not the focus on {name}"),
    }
}

/// Focuses mockapp's search box, as the listener reads and sends the
/// focus, and returns its node and the ancestors reported with it once the
/// outpost has reported it, and that it has no text, and has nothing more
/// to say.
fn focus_search_box(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    uia: &Uia,
    root: &Live,
) -> (NodeId, Vec<NodeSnapshot>) {
    let (search, ancestors) = report_focus(app, outpost, (uia, root), ("search", "Search box"));
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
    (search.id, ancestors)
}

/// The listener's selection fact for the element `runtime_id` names, of
/// role `role` and named `name`, in mockapp's window `hwnd`.
fn selection_fact(hwnd: isize, runtime_id: Vec<i32>, role: Role, name: &str) -> DeliveredFact {
    DeliveredFact::UiaSelection {
        hwnd,
        focus_window: hwnd,
        snapshot: UiaSnapshotFact {
            runtime_id,
            role,
            name: Some(name.to_owned()),
            value: None,
            states: StateSet::new(),
            details: verbatim_model::NodeDetails::default(),
        },
    }
}

/// mockapp's element selected events under its window `window`, each with
/// its fact as the focus listener's callback captures it
/// (`verbatim_outpost::listener::capture`) and its element in agile form,
/// as the listener hands both to its reading thread. The subscription
/// lasts as long as the registration returned.
fn selections(
    window: isize,
) -> (
    Registration,
    mpsc::Receiver<(Agile<ListenerEvent>, DeliveredFact)>,
) {
    let (sent, events) = mpsc::channel();
    let registration = Registration::with_parts(
        vec![Part::shaped::<ListenerEvent>(Subscription::Event {
            event: UIA_SelectionItem_ElementSelectedEventId,
            callback: Arc::new(move |element: &Cached<ListenerEvent>| {
                let (hwnd, snapshot) =
                    verbatim_outpost::listener::capture(element, &element.read())
                        .expect("mockapp's element has its process");
                let fact = DeliveredFact::UiaSelection {
                    hwnd,
                    focus_window: 0,
                    snapshot,
                };
                let agile = element.agile().expect("an agile reference");
                // The test may have finished.
                let _ = sent.send((agile, fact));
            }),
        })],
        common::window_subtree(window),
    )
    .expect("a subscription to element selected events");
    (registration, events)
}

/// Settings' replacing state, through a real outpost: the list's first
/// item is selected, the list then replaces its items under new runtime
/// ids, the new first item selected, and the selection of the old item
/// is delivered after that. The outpost reports the list's current
/// selection, the new item, as a controlled selection of the search box,
/// with the new item's name and position, and nothing more. The old
/// item's fact is the listener's own: captured in the event's callback and
/// checked by the listener's reading thread's check
/// (`OutpostUnderTest::listener_check`), which finds the windowless item's
/// nearest window from its element. The check runs before the
/// replacement, as the real listener's check precedes the outpost's
/// handling, and as Settings' old element still answers then (the route's
/// live firings, `docs/parity/changes-and-selection.md`); a check made
/// after mockapp's item died would drop the event, its nearest-window call
/// failing on the dead element.
fn a_selection_of_an_item_the_list_has_replaced_is_reported_as_its_current_selection() {
    let title = common::unique_title("mockapp-controller-replaced");
    let mut app = common::spawn("replaced_suggestions.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("Uia::new");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    let (search, _) = focus_search_box(&mut app, &outpost, &uia, &root);
    let window = hwnd.0 as isize;
    let (_registration, events) = selections(window);

    app.send("select sound");
    let (agile, fact) = events
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("an element selected event for Sound settings");
    let element = agile.resolve().expect("the event's element");
    let fact = match outpost.listener_check(&element, fact) {
        Judged::Passed(fact) => *fact,
        other => panic!("the listener's check made {other:?} of the selection"),
    };
    let replaced = match &fact {
        DeliveredFact::UiaSelection {
            hwnd,
            focus_window,
            snapshot,
        } => {
            assert_eq!(
                (*hwnd, *focus_window, snapshot.name.as_deref()),
                (0, window, Some("Sound settings")),
                "the selected item's fact"
            );
            snapshot.runtime_id.clone()
        }
        other => panic!("{other:?} is no selection"),
    };
    app.send("replace-items suggestions");
    let current = verbatim_uia::runtime_id(&find(&uia, &root, "Sound settings"));
    assert_ne!(
        current, replaced,
        "the new item has a runtime id of its own"
    );
    outpost.deliver(fact);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::ControlledSelection { controller, node },
            ..
        } => assert_eq!(
            (
                controller,
                node.name.as_deref(),
                node.details.position_in_set,
                node.details.set_size
            ),
            (search, Some("Sound settings"), Some(1), Some(2))
        ),
        other => panic!("the outpost said {other:?}, not the controlled selection"),
    }
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A selection of the focus itself, while the list it controls has a
/// selection, is the focus's own selection change, reported as one, and
/// nothing more, as NVDA's base selection handler, finding it in none of
/// the controlled elements, falls through to its state change handler; it
/// is never answered with the list's current selection.
fn a_selection_of_the_focus_itself_is_reported_as_its_own() {
    let title = common::unique_title("mockapp-controller-own");
    let mut app = common::spawn("replaced_suggestions.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("Uia::new");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    let (search, _) = focus_search_box(&mut app, &outpost, &uia, &root);

    app.send("select sound");
    let search_box = verbatim_uia::runtime_id(&find(&uia, &root, "Search box"));
    outpost.deliver(selection_fact(
        hwnd.0 as isize,
        search_box,
        Role::EditableText,
        "Search box",
    ));
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::SelectionChanged { node },
            ..
        } => assert_eq!(node.id, search, "the search box's own selection"),
        other => panic!("the outpost said {other:?}, not the search box's selection"),
    }
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A selection of the search box's parent, mockapp's window, the nearest
/// of the ancestors reported with the focus, while the list the box
/// controls has a selection, is the window's selection change, reported
/// as one, and nothing more, as NVDA's base selection handler, finding it
/// in none of the controlled elements, falls through to its state change
/// handler; it is never answered with the list's current selection.
fn a_selection_of_an_ancestor_of_the_focus_is_reported_as_its_own() {
    let title = common::unique_title("mockapp-controller-ancestor");
    let mut app = common::spawn("replaced_suggestions.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("Uia::new");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    let (_, ancestors) = focus_search_box(&mut app, &outpost, &uia, &root);
    let parent = ancestors.last().expect("the search box's parent").id;

    app.send("select sound");
    outpost.deliver(selection_fact(
        hwnd.0 as isize,
        verbatim_uia::runtime_id(root.element()),
        Role::Window,
        &title,
    ));
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::SelectionChanged { node },
            ..
        } => assert_eq!(node.id, parent, "the window's own selection"),
        other => panic!("the outpost said {other:?}, not the window's selection"),
    }
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// A focused list whose relation names the suggestions list, while that
/// list has a selection: a selection of the focused list's item is the
/// item's selection change, reported as one, and nothing more, as the
/// reducer reads any selection while the focus is a list or tab control as
/// one in the focus; it is never answered with the controlled list's
/// current selection.
fn a_selection_while_the_focus_is_a_list_is_reported_as_its_own() {
    let title = common::unique_title("mockapp-controller-list");
    let mut app = common::spawn("replaced_suggestions.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("Uia::new");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    report_focus(
        &mut app,
        &outpost,
        (&uia, &root),
        ("categories", "Categories"),
    );
    outpost.settled();

    app.send("select sound");
    let general = verbatim_uia::runtime_id(&find(&uia, &root, "General"));
    outpost.deliver(selection_fact(
        hwnd.0 as isize,
        general,
        Role::ListItem,
        "General",
    ));
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::SelectionChanged { node },
            ..
        } => assert_eq!(
            (node.role, node.name.as_deref()),
            (Role::ListItem, Some("General")),
            "the focused list's item's selection"
        ),
        other => panic!("the outpost said {other:?}, not the item's selection"),
    }
    outpost.settled();

    drop(outpost);
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
    let (search, _) = focus_search_box(&mut app, &outpost, &uia, &root);

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
    outpost.deliver(selection_fact(
        hwnd.0 as isize,
        verbatim_uia::runtime_id(&result),
        Role::ListItem,
        "Sound settings",
    ));
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
        (
            "a_selection_of_an_item_the_list_has_replaced_is_reported_as_its_current_selection",
            a_selection_of_an_item_the_list_has_replaced_is_reported_as_its_current_selection,
        ),
        (
            "a_selection_of_the_focus_itself_is_reported_as_its_own",
            a_selection_of_the_focus_itself_is_reported_as_its_own,
        ),
        (
            "a_selection_of_an_ancestor_of_the_focus_is_reported_as_its_own",
            a_selection_of_an_ancestor_of_the_focus_is_reported_as_its_own,
        ),
        (
            "a_selection_while_the_focus_is_a_list_is_reported_as_its_own",
            a_selection_while_the_focus_is_a_list_is_reported_as_its_own,
        ),
    ]);
}
