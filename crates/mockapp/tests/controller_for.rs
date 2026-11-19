//! Selection in a list the focus controls, cross-process: a search box whose
//! UIA `ControllerFor` relation names a results list, as the Settings app's
//! search box names its suggestions ("Selection in a list the focus
//! controls" in `docs/nvda/events.md`).
//!
//! An item selected through mockapp's `select` command raises a real
//! `ElementSelected` event; `verbatim_uia_rops::controlled_selection`, the
//! check the outpost makes on such an event, must find it from the search
//! box, one level down, inside a group of the list, or the controlled
//! element itself, and must find nothing for an item outside the controlled
//! list or from an element that controls nothing. It answers each in one
//! remote operation, and the classic calls behind the same signature answer
//! the same. The focused element is passed in rather than read from the
//! keyboard focus, so the test runs headless.
//!
//! Through a real outpost, a controlled selection is also reported when the
//! element the outpost keeps for the focus came from one of its events,
//! whose cache holds less than a focus read's, with no failed cached read.
//! A selection raised for an item the list has since replaced (mockapp's
//! `replace-items`) is no controlled selection, and an ordinary selection
//! of the new item is one. A selection whose event carried no name, as
//! Settings' first suggestion's does, is said with the name read when Core
//! reads the item. A selection of the focus itself is reported as
//! an ordinary selection change, never as the list's. A
//! search box holding text is reported with the line at its caret in the
//! focus's own message, ahead of a controlled selection that arrived after
//! the focus.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::mpsc;

use verbatim_model::event_kind::EventKind;
use verbatim_model::tell::Payload;
use verbatim_model::{CallCounts, Role, SegmentContent, StateSet};
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::{
    Agile, Cached, Fill, ListenerEvent, ListenerSelection, Live, Node, Part, Registration, Shape,
    Subscription, Uia,
};
use verbatim_uia_rops::{ControlledQuery, Path, controlled_selection};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Descendants, UIA_NamePropertyId,
    UIA_SelectionItem_ElementSelectedEventId,
};
use windows::core::BSTR;

use common::core_driver::CoreDriver;
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
    let results_id = verbatim_uia::runtime_id(&results);
    assert_eq!(
        controlled(&uia, &search, &results_id),
        Some((Some("Suggestions".to_owned()), results_id.clone())),
        "the controlled list itself is the element NVDA's walker answers"
    );

    app.send("select recent1");
    let elsewhere = selected_id("Elsewhere");
    assert_eq!(
        controlled(&uia, &search, &elsewhere),
        None,
        "an item outside the controlled list is not a controlled selection"
    );

    // A selection the list does not hold, however it came to be raised,
    // reports nothing of the list's own selection.
    app.send("select result2");
    let current = selected_id("Sound settings");
    let replaced: Vec<i32> = current.iter().copied().chain([1]).collect();
    assert_eq!(
        controlled(&uia, &search, &replaced),
        None,
        "a selection the list does not hold is not answered with its current selection"
    );

    // The relation switched to a second list: its item is the event's own
    // element, and an item of the first list is in nothing the box controls.
    app.send("set-controller-for search recent");
    assert_eq!(
        controlled(&uia, &search, &elsewhere),
        Some((Some("Elsewhere".to_owned()), elsewhere.clone())),
        "a selection in a second controlled element is the event's own element"
    );
    assert_eq!(
        controlled(&uia, &search, &current),
        None,
        "a selection in a list the box no longer controls"
    );

    // A marker: its event coming next shows one selection event came for
    // each select before it.
    app.send("select result1");
    selected_id("Display settings");
    app.quit();
}

/// No speech.
const NOTHING: [Vec<SegmentContent>; 0] = [];

/// Core driving an outpost watching mockapp's `fixture`, with its search
/// box focused and told to Core as the listener tells its focus: what
/// Core said of the focus.
fn search_box_focused(
    fixture: &str,
    prefix: &str,
    options: OutpostOptions,
) -> (common::MockApp, HWND, CoreDriver, Vec<Vec<SegmentContent>>) {
    let title = common::unique_title(prefix);
    let mut app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::with_options(&app, options));
    app.send("set-focus search");
    let uia = Uia::new().expect("Uia::new");
    let said = core
        .uia_focus(&common::listener_element(
            &uia,
            hwnd.0 as isize,
            "Search box",
        ))
        .spoken();
    (app, hwnd, core, said)
}

/// The states an element selected event's cache holds for mockapp's
/// element `name`, as the listener's own `capture_selection` reads them,
/// with the element as the listener tells it.
fn selected(uia: &Uia, hwnd: HWND, name: &str) -> (Cached<ListenerEvent>, StateSet) {
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    let condition = uia
        .property_condition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(name)))
        .expect("name condition");
    let request = uia
        .request_fixed::<ListenerSelection>()
        .expect("the listener's selection cache request");
    let element = root
        .find_first(TreeScope_Descendants, &condition, &request)
        .expect("the search")
        .unwrap_or_else(|| panic!("no element named {name:?}"));
    let (_, snapshot) = verbatim_listener::listener::capture_selection(&element, &element.read())
        .expect("mockapp's element has its process");
    (element.narrow(), snapshot.states)
}

/// An element selected event: its element, its cached name and states.
type Selected = (Agile<ListenerSelection>, Option<String>, StateSet);

/// mockapp's element selected events under its window `window`, each with
/// its element as the listener's callback narrows it and the states its
/// cache holds (`verbatim_listener::listener::capture_selection`). The
/// subscription lasts as long as the registration returned.
fn selections(window: isize) -> (Registration, mpsc::Receiver<Selected>) {
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
    (registration, events)
}

/// The next selection event, which must be of the item named `name`, as
/// the listener tells it.
fn next_selection(
    events: &mpsc::Receiver<Selected>,
    name: &str,
) -> (Cached<ListenerEvent>, StateSet) {
    let (agile, cached_name, states) = events
        .recv_timeout(common::WAIT_TIMEOUT)
        .unwrap_or_else(|_| panic!("no element selected event for {name}"));
    assert_eq!(cached_name.as_deref(), Some(name), "the selected item");
    let element = agile.resolve().expect("the event's element");
    (element.narrow(), states)
}

/// What Core says of the controlled selection of "Sound settings": the
/// item, as a focus, with its position in its fixture's list of size,
/// when its fixture gives one.
fn sound_settings(size: Option<u32>) -> Vec<Vec<SegmentContent>> {
    let mut said = vec![SegmentContent::Label("Sound settings".to_owned())];
    if let Some(size) = size {
        said.push(SegmentContent::Position {
            position: 1,
            set_size: Some(size),
        });
    }
    vec![said]
}

/// Settings' replacing state, through a real outpost: the list's first
/// item is selected, the list then replaces its items under new runtime
/// ids, and the selection of the old item reaches Core after that. Nothing
/// is said of it: Core's read of the item finds it gone. The listener's
/// check of the old item runs before the replacement, as the real
/// listener's check precedes Core's read, and as Settings' old element
/// still answers then (the route's live firings,
/// `docs/parity/changes-and-selection.md`). A selection of the new item
/// that follows is a controlled selection, spoken with its name and
/// position.
fn a_stale_selection_after_a_replacement_reports_nothing_and_an_ordinary_one_the_new_item() {
    let (mut app, hwnd, mut core, _) = search_box_focused(
        "replaced_suggestions.json",
        "mockapp-controller-replaced",
        OutpostOptions::default(),
    );
    let uia = Uia::new().expect("Uia::new");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    let (_registration, events) = selections(hwnd.0 as isize);

    app.send("select sound");
    let (element, states) = next_selection(&events, "Sound settings");
    let stale = core.uia_tell(
        EventKind::UiaElementSelected,
        &element,
        Some(states),
        Payload::None,
    );
    app.send("replace-items suggestions");
    let current = verbatim_uia::runtime_id(&find(&uia, &root, "Sound settings"));
    assert_ne!(
        Some(current.as_slice()),
        match &stale.object {
            verbatim_model::tell::EventObject::Uia { runtime_id, .. } =>
                Some(runtime_id.0.as_slice()),
            verbatim_model::tell::EventObject::Msaa { .. }
            | verbatim_model::tell::EventObject::Followed { .. }
            | verbatim_model::tell::EventObject::Terminal { .. } => None,
        },
        "the new item has a runtime id of its own"
    );
    assert_eq!(core.tell(stale).spoken(), NOTHING);

    app.send("select sound");
    let (element, states) = next_selection(&events, "Sound settings");
    assert_eq!(
        core.uia_event(
            EventKind::UiaElementSelected,
            &element,
            Some(states),
            Payload::None
        )
        .spoken(),
        sound_settings(Some(2))
    );
    drop(core);
    app.quit();
}

/// The first suggestion Settings selects as its list opens, in the order
/// the flight recorders of `settings_search_suggestions` and
/// `settings_search_suggestions_reentered` (2026-11-19) show: the item's
/// element selected event carries no name in its cache, since Settings
/// has not set it yet, the listener's check runs then, and Settings has
/// set the name by the time Core's read of the item runs. The item is
/// spoken with the name read then, "Sound settings 1 of 2", not the
/// event's (Dickson, 2026-10-20, `docs/parity/changes-and-selection.md`,
/// "Selection in a list the focus controls").
fn a_selection_whose_event_carried_no_name_is_said_with_the_name_read() {
    let (mut app, hwnd, mut core, _) = search_box_focused(
        "replaced_suggestions.json",
        "mockapp-controller-unnamed",
        OutpostOptions::default(),
    );
    let (_registration, events) = selections(hwnd.0 as isize);

    app.send("set-name sound");
    app.send("select sound");
    let (agile, cached_name, states) = events
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("an element selected event for the unnamed item");
    assert_eq!(cached_name, None, "the event's cache carries no name");
    let element: Cached<ListenerEvent> = agile.resolve().expect("the event's element").narrow();
    let selection = core.uia_tell(
        EventKind::UiaElementSelected,
        &element,
        Some(states),
        Payload::None,
    );
    app.send("set-name sound Sound settings");
    assert_eq!(core.tell(selection).spoken(), sound_settings(Some(2)));
    drop(core);
    app.quit();
}

/// A selection of the focus itself, while the list it controls has a
/// selection, is the focus's own state change, judged on the states its
/// event's cache holds, which are those already spoken: nothing is said,
/// and it is never answered with the list's current selection.
fn a_selection_of_the_focus_itself_is_reported_as_its_own() {
    let (mut app, hwnd, mut core, _) = search_box_focused(
        "replaced_suggestions.json",
        "mockapp-controller-own",
        OutpostOptions::default(),
    );
    app.send("select sound");
    let uia = Uia::new().expect("Uia::new");
    let (search_box, states) = selected(&uia, hwnd, "Search box");
    assert_eq!(
        core.uia_event(
            EventKind::UiaElementSelected,
            &search_box,
            Some(states),
            Payload::None
        )
        .spoken(),
        NOTHING
    );
    drop(core);
    app.quit();
}

/// A controlled selection read the classic way, after a change of the
/// focus's description the outpost tells, reads nothing absent
/// from a cache: the completion reads the focus's controller relation from
/// the element it holds for the focus.
fn a_controlled_selection_from_a_kept_event_element_reads_nothing_uncached() {
    let (mut app, hwnd, mut core, _) = search_box_focused(
        "controller.json",
        "mockapp-controller-kept",
        OutpostOptions {
            remote_operations: false,
        },
    );
    let uia = Uia::new().expect("Uia::new");
    app.send("set-description search Type to search");
    assert_eq!(
        core.told(1).spoken(),
        [vec![SegmentContent::Description(
            "Type to search".to_owned()
        )]],
        "the focus's new description"
    );
    let _ = core.outpost.take_failed_reads();

    app.send("select result2");
    let (result, states) = selected(&uia, hwnd, "Sound settings");
    assert_eq!(
        core.uia_event(
            EventKind::UiaElementSelected,
            &result,
            Some(states),
            Payload::None
        )
        .spoken(),
        sound_settings(None)
    );
    assert_eq!(core.outpost.take_failed_reads(), 0, "no failed cached read");
    drop(core);
    app.quit();
}

/// A focus on a search box holding text, followed by a selection in the
/// list it controls, as Settings selects its first suggestion on a Shift+Tab
/// into its search box: the focus is spoken with the line at its caret,
/// which is part of the focus's announcement (`docs/nvda/speech.md`, "What
/// an object with text says"), then the controlled selection.
fn a_text_focus_carries_its_line_ahead_of_a_selection_that_followed_it() {
    let title = common::unique_title("mockapp-controller-typed");
    let mut app = common::spawn("typed_search.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    let uia = Uia::new().expect("Uia::new");

    app.send("set-focus search");
    app.send("select sound");
    let focus = common::listener_element(&uia, hwnd.0 as isize, "Search box");
    let (result, states) = selected(&uia, hwnd, "Sound settings");
    let focus = core.uia_tell(EventKind::UiaFocus, &focus, None, Payload::None);
    let selection = core.uia_tell(
        EventKind::UiaElementSelected,
        &result,
        Some(states),
        Payload::None,
    );
    let mut said = core.tell(focus).spoken();
    said.extend(core.tell(selection).spoken());
    assert_eq!(
        said,
        [
            vec![
                SegmentContent::Label("Mockapp Typed Search Fixture".to_owned()),
                SegmentContent::Role(Role::Window),
            ],
            vec![
                SegmentContent::Label("Search box".to_owned()),
                SegmentContent::Role(Role::EditableText),
            ],
            vec![
                SegmentContent::Format(verbatim_model::TextFormat::FontName("Consolas".to_owned())),
                SegmentContent::Format(verbatim_model::TextFormat::FontSize("11.0 pt".to_owned())),
                SegmentContent::Format(verbatim_model::TextFormat::Color("black".to_owned())),
                SegmentContent::Text("sound".to_owned()),
            ],
            sound_settings(Some(1))[0].clone(),
        ]
    );
    drop(core);
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
            "a_stale_selection_after_a_replacement_reports_nothing_and_an_ordinary_one_the_new_item",
            a_stale_selection_after_a_replacement_reports_nothing_and_an_ordinary_one_the_new_item,
        ),
        (
            "a_selection_whose_event_carried_no_name_is_said_with_the_name_read",
            a_selection_whose_event_carried_no_name_is_said_with_the_name_read,
        ),
        (
            "a_selection_of_the_focus_itself_is_reported_as_its_own",
            a_selection_of_the_focus_itself_is_reported_as_its_own,
        ),
        (
            "a_text_focus_carries_its_line_ahead_of_a_selection_that_followed_it",
            a_text_focus_carries_its_line_ahead_of_a_selection_that_followed_it,
        ),
    ]);
}
