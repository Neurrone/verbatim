//! Cross-process event delivery (architecture section 13, layer 2).
//!
//! After a `set-name`, `set-value`, `select`, `notify`, or
//! `active-text-position` stdin command, asserts that the matching real
//! client-side registration observes exactly the change: one event, from
//! the element the command changed, carrying the changed data, read through
//! `verbatim_uia::Registration` for UIA and `verbatim_ia2::WinEventHook` for
//! MSAA. Property, value, selection, and notification changes are used
//! rather than focus, per the architecture note that these tests must pass
//! headless on CI runners without real keyboard focus or
//! `SetForegroundWindow` succeeding.
//!
//! Each test ends with a marker: one more command whose own event is the
//! last one expected. A provider's events reach a client in the order they
//! were raised, so a duplicate of the event under test would come before
//! the marker's, and the test sees every event up to it.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use verbatim_ia2::{APP_SUBSCRIPTIONS, WinEventHook, WinEventKind};
use verbatim_model::{NotificationKind, NotificationProcessing, State, StateSet};
use verbatim_uia::{
    ElementExt, FOCUS_PROPERTIES, Registration, Scope, Subscription, Uia,
    map::{notification_kind_from_uia, notification_processing_from_uia},
};
use windows::Win32::Foundation::{HWND, WAIT_TIMEOUT};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_NamePropertyId, UIA_SelectionItem_ElementSelectedEventId,
    UIA_ValueValuePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage,
};
use windows::core::AgileReference;

/// Receives exactly `expected`, in order, each within the shared wait, and
/// then finds nothing more waiting.
fn expect_events<T: Debug + PartialEq>(events: &Receiver<T>, expected: &[T]) {
    let mut seen = Vec::with_capacity(expected.len());
    for _ in expected {
        match events.recv_timeout(common::WAIT_TIMEOUT) {
            Ok(event) => seen.push(event),
            Err(error) => panic!("only {seen:?} arrived of {expected:?}: {error}"),
        }
    }
    assert_eq!(seen, expected);
    match events.try_recv() {
        Err(TryRecvError::Empty) => {}
        Ok(extra) => panic!("an event after the marker: {extra:?}"),
        Err(TryRecvError::Disconnected) => panic!("the registration ended"),
    }
}

/// A property change as a registration hears it: the sender's cached name
/// and value, and the property.
#[derive(Debug, PartialEq)]
struct PropertyChange {
    name: Option<String>,
    value: Option<String>,
    property: i32,
}

/// A registration on mockapp's window for the focus-following properties,
/// sending each change it hears.
fn property_changes(hwnd: HWND) -> (Registration, Receiver<PropertyChange>) {
    let (sender, changes) = mpsc::channel();
    let registration = Registration::new(
        vec![Subscription::Properties {
            properties: FOCUS_PROPERTIES.to_vec(),
            callback: Arc::new(move |element, property| {
                // The test may have finished.
                let _ = sender.send(PropertyChange {
                    name: element.cached_string(UIA_NamePropertyId),
                    value: element.cached_string(UIA_ValueValuePropertyId),
                    property,
                });
            }),
        }],
        Scope::Windows(vec![hwnd.0 as isize]),
    )
    .expect("Registration::new");
    (registration, changes)
}

/// The names of the root's children, read afresh through a new client.
fn children_names(hwnd: HWND) -> Vec<(verbatim_model::Role, Option<String>)> {
    let uia = Uia::new().expect("Uia::new");
    let cache = uia.base_cache_request().expect("base cache request");
    let root = uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("element_from_handle");
    // SAFETY: a live client; the condition takes no arguments.
    let condition = unsafe { uia.client().CreateTrueCondition() }.expect("CreateTrueCondition");
    // SAFETY: `root` was built with the base cache request; `TreeScope_Children`
    // and the true condition are standard client-side arguments.
    let children = unsafe {
        root.FindAllBuildCache(
            windows::Win32::UI::Accessibility::TreeScope_Children,
            &condition,
            &cache,
        )
    }
    .expect("FindAllBuildCache");
    let registry = verbatim_uia::NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
    verbatim_uia::elements_of(&children)
        .iter()
        .map(|child| {
            let snapshot = verbatim_uia::map::snapshot_from_cached_element(child, &registry);
            (snapshot.role, snapshot.name)
        })
        .collect()
}

fn uia_set_name_raises_a_property_changed_event() {
    let title = common::unique_title("mockapp-events-uia-name");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let (_registration, changes) = property_changes(hwnd);

    app.send("set-name btn1 Renamed");
    app.send("set-name slider1 Marker");
    expect_events(
        &changes,
        &[
            PropertyChange {
                name: Some("Renamed".to_owned()),
                value: None,
                property: UIA_NamePropertyId.0,
            },
            PropertyChange {
                name: Some("Marker".to_owned()),
                value: Some("1".to_owned()),
                property: UIA_NamePropertyId.0,
            },
        ],
    );

    // The provider's live data changed, not just its event: the button, and
    // only the button, has the new name. The title bar comes first, from
    // UIA's own proxy for the window's frame.
    assert_eq!(
        children_names(hwnd),
        [
            (verbatim_model::Role::TitleBar, None),
            (verbatim_model::Role::Button, Some("Renamed".to_owned())),
            (verbatim_model::Role::Slider, Some("Marker".to_owned())),
            (verbatim_model::Role::List, Some("Options".to_owned()))
        ]
    );

    app.quit();
}

/// The focus-following scope, selective registration: the focus's changes
/// arrive, and neither an ancestor's nor a sibling's do.
fn uia_only_the_focus_is_followed() {
    let title = common::unique_title("mockapp-events-uia-ancestors");
    let mut app = common::spawn("counts.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let uia = Uia::new().expect("Uia::new");
    let cache = uia.base_cache_request().expect("base cache request");
    let registry = verbatim_uia::NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
    let root = uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("element_from_handle");
    let (tree, _) = uia
        .walk_tree(&root, &cache, &registry, 8, 64)
        .expect("mockapp's tree");
    let first = tree
        .children
        .iter()
        .flat_map(|group| &group.children)
        .find(|node| node.snapshot.name.as_deref() == Some("First"))
        .and_then(|node| registry.element_of(node.snapshot.id))
        .expect("the button First");

    let (sender, names) = mpsc::channel();
    let _registration = Registration::new(
        vec![Subscription::Properties {
            properties: FOCUS_PROPERTIES.to_vec(),
            callback: Arc::new(move |element, property| {
                // The sender arrives with the base cache request's name.
                let _ = sender.send((element.cached_string(UIA_NamePropertyId), property));
            }),
        }],
        Scope::Elements(vec![first]),
    )
    .expect("Registration::new");

    app.send("set-name second Sibling");
    app.send("set-name group Ancestor");
    app.send("set-name first Focus");
    app.send("set-name first Marker");
    expect_events(
        &names,
        &[
            (Some("Focus".to_owned()), UIA_NamePropertyId.0),
            (Some("Marker".to_owned()), UIA_NamePropertyId.0),
        ],
    );

    app.quit();
}

fn uia_set_value_raises_a_property_changed_event() {
    let title = common::unique_title("mockapp-events-uia-value");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let (_registration, changes) = property_changes(hwnd);

    app.send("set-value slider1 77");
    app.send("set-name btn1 Marker");
    expect_events(
        &changes,
        &[
            PropertyChange {
                name: Some("Level".to_owned()),
                value: Some("77".to_owned()),
                property: UIA_ValueValuePropertyId.0,
            },
            PropertyChange {
                name: Some("Marker".to_owned()),
                value: None,
                property: UIA_NamePropertyId.0,
            },
        ],
    );

    app.quit();
}

/// A selected element as a registration hears it, mapped exactly as an
/// outpost maps it: its name and its whole state set.
fn selected(element: &IUIAutomationElement) -> (Option<String>, StateSet) {
    let registry = verbatim_uia::NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
    let snapshot = verbatim_uia::map::snapshot_from_cached_element(element, &registry);
    (snapshot.name, snapshot.states)
}

fn uia_select_raises_a_selection_event() {
    let title = common::unique_title("mockapp-events-uia-select");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);

    // The marker is a notification, heard by the same group: another
    // selection would take the state off the first item before UIA reads
    // the first event's cached properties, which it does as it delivers it.
    let (sender, selections) = mpsc::channel();
    let marker = sender.clone();
    let _registration = Registration::new(
        vec![
            Subscription::Event {
                event: UIA_SelectionItem_ElementSelectedEventId,
                callback: Arc::new(move |element| {
                    let _ = sender.send(Some(selected(element)));
                }),
            },
            Subscription::Notifications {
                callback: Arc::new(move |_, _, _, _, _| {
                    let _ = marker.send(None);
                }),
            },
        ],
        Scope::Windows(vec![hwnd.0 as isize]),
    )
    .expect("Registration::new");

    app.send("select item1");
    app.send("notify Marker");
    let first = (
        Some("First".to_owned()),
        [State::Focusable, State::Selectable, State::Selected]
            .into_iter()
            .collect::<StateSet>(),
    );
    expect_events(&selections, &[Some(first), None]);

    app.quit();
}

/// One observed notification: kind, processing, display string, activity id.
type SeenNotification = (
    NotificationKind,
    NotificationProcessing,
    Option<String>,
    Option<String>,
);

fn uia_notify_raises_a_notification_event() {
    let title = common::unique_title("mockapp-events-uia-notify");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let (sender, notifications) = mpsc::channel::<SeenNotification>();
    let _registration = Registration::new(
        vec![Subscription::Notifications {
            callback: Arc::new(move |_element, kind, processing, display, activity| {
                let _ = sender.send((
                    notification_kind_from_uia(kind),
                    notification_processing_from_uia(processing),
                    display,
                    activity,
                ));
            }),
        }],
        Scope::Windows(vec![hwnd.0 as isize]),
    )
    .expect("Registration::new");

    app.send("notify Window snapped to the left");
    app.send("notify Marker");
    let notification = |display: &str| {
        (
            NotificationKind::Other,
            NotificationProcessing::All,
            Some(display.to_owned()),
            Some("mockapp-notify".to_owned()),
        )
    };
    expect_events(
        &notifications,
        &[
            notification("Window snapped to the left"),
            notification("Marker"),
        ],
    );

    app.quit();
}

/// The active text position changed event arrives with the element that
/// raised it and the range now active, whose text the client reads.
fn uia_active_text_position_arrives_with_its_range() {
    use verbatim_uia::text::TextRangeExt;

    let title = common::unique_title("mockapp-events-uia-active-position");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let (sender, positions) = mpsc::channel();
    let _registration = Registration::new(
        vec![Subscription::ActiveTextPosition {
            callback: Arc::new(move |element, range| {
                let name = element.cached_string(UIA_NamePropertyId);
                let range = range.and_then(|range| AgileReference::new(range).ok());
                let _ = sender.send((name, range));
            }),
        }],
        Scope::Windows(vec![hwnd.0 as isize]),
    )
    .expect("Registration::new");

    app.send("active-text-position doc 6 10");
    app.send("active-text-position doc 0 5");
    let mut seen = Vec::new();
    for _ in 0..2 {
        let (name, range) = positions
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|_| panic!("only {seen:?} arrived"));
        let range = range
            .expect("the event's range")
            .resolve()
            .expect("the range");
        let text = range.text(-1).expect("the range's text");
        seen.push((name, String::from_utf16_lossy(&text)));
    }
    assert_eq!(
        seen,
        [
            (Some("Notes".to_owned()), "beta".to_owned()),
            (Some("Notes".to_owned()), "alpha".to_owned())
        ]
    );
    assert!(
        matches!(positions.try_recv(), Err(TryRecvError::Empty)),
        "an event after the marker"
    );

    app.quit();
}

/// One registration of several subscriptions, as the focus listener makes,
/// is one event handler group, and each of its handlers hears its own
/// events, once each.
fn uia_one_group_delivers_each_of_its_subscriptions() {
    let title = common::unique_title("mockapp-events-uia-group");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let (sender, seen) = mpsc::channel::<&'static str>();
    let note = |what: &'static str| {
        let sender = sender.clone();
        move || {
            let _ = sender.send(what);
        }
    };
    let (property, selection, notification) =
        (note("property"), note("selection"), note("notification"));
    let _registration = Registration::new(
        vec![
            Subscription::Properties {
                properties: FOCUS_PROPERTIES.to_vec(),
                callback: Arc::new(move |_, _| property()),
            },
            Subscription::Event {
                event: UIA_SelectionItem_ElementSelectedEventId,
                callback: Arc::new(move |_| selection()),
            },
            Subscription::Notifications {
                callback: Arc::new(move |_, _, _, _, _| notification()),
            },
        ],
        Scope::Windows(vec![hwnd.0 as isize]),
    )
    .expect("Registration::new");

    app.send("set-name btn1 Renamed");
    app.send("select item1");
    app.send("notify Done");
    // The marker.
    app.send("set-name btn1 Marker");
    expect_events(
        &seen,
        &["property", "selection", "notification", "property"],
    );

    app.quit();
}

/// A `WinEvent` as the hook reports it: kind, window, object, and child.
type SeenWinEvent = (WinEventKind, isize, i32, i32);

/// A hook on mockapp's process for the events an outpost hooks, sending
/// each event it hears. The events come through this thread's message
/// queue, which [`pump_until`] pumps.
fn hook(pid: u32) -> (WinEventHook, Receiver<SeenWinEvent>) {
    let (sender, events) = mpsc::channel();
    let hook = WinEventHook::install(
        pid,
        APP_SUBSCRIPTIONS,
        Box::new(move |kind, hwnd, id_object, id_child, _| {
            let _ = sender.send((kind, hwnd, id_object, id_child));
        }),
    )
    .expect("WinEventHook::install");
    (hook, events)
}

/// Pumps this thread's message queue until `count` events have arrived,
/// waiting for each message rather than polling, since out-of-context
/// `WinEvent`s are delivered through the installing thread's queue; then
/// pumps what is already queued and asserts nothing more arrived.
fn pump_until(events: &Receiver<SeenWinEvent>, count: usize) -> Vec<SeenWinEvent> {
    let deadline = Instant::now() + common::WAIT_TIMEOUT;
    let mut seen = Vec::with_capacity(count);
    loop {
        pump_queued();
        seen.extend(events.try_iter());
        if seen.len() >= count {
            break;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        let left = u32::try_from(left.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: no handles; wakes when a message arrives or the time is up.
        let woke =
            unsafe { MsgWaitForMultipleObjectsEx(None, left, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
        assert_ne!(woke, WAIT_TIMEOUT, "only {seen:?} arrived");
    }
    pump_queued();
    seen.extend(events.try_iter());
    seen
}

/// Dispatches every message already in this thread's queue.
fn pump_queued() {
    let mut msg = MSG::default();
    // SAFETY: `msg` is a local the call writes when it returns a message.
    while unsafe { PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
        // SAFETY: `msg` is the message just retrieved.
        let _ = unsafe { TranslateMessage(&raw const msg) };
        // SAFETY: as above.
        unsafe { DispatchMessageW(&raw const msg) };
    }
}

/// mockapp's MSAA address for the small fixture's node at `index`: its
/// window, the object id `index + 1`, and the object itself.
fn address(hwnd: HWND, index: i32) -> (isize, i32, i32) {
    (hwnd.0 as isize, index + 1, verbatim_ia2::CHILDID_SELF)
}

/// The small fixture's nodes, by their index in mockapp's tree.
const BUTTON: i32 = 1;
const SLIDER: i32 = 2;
const FIRST_ITEM: i32 = 4;
const SECOND_ITEM: i32 = 5;

/// The node an MSAA event names, read through it as an outpost would.
fn read_object((hwnd, id_object, id_child): (isize, i32, i32)) -> verbatim_model::NodeSnapshot {
    let registry = verbatim_ia2::NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
    verbatim_ia2::acquire::snapshot_from_event(
        hwnd,
        id_object,
        id_child,
        &registry,
        verbatim_ia2::acquire::Purpose::Announce,
    )
    .expect("the event's object")
}

/// The event `kind` on the node at `index`.
fn win_event(kind: WinEventKind, hwnd: HWND, index: i32) -> SeenWinEvent {
    let (hwnd, id_object, id_child) = address(hwnd, index);
    (kind, hwnd, id_object, id_child)
}

fn msaa_set_name_raises_a_name_change_win_event() {
    common::init_com();
    let title = common::unique_title("mockapp-events-msaa-name");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let (_hook, events) = hook(app.pid());

    app.send("set-name btn1 Renamed");
    app.send("set-value slider1 77");
    assert_eq!(
        pump_until(&events, 2),
        [
            win_event(WinEventKind::NameChange, hwnd, BUTTON),
            win_event(WinEventKind::ValueChange, hwnd, SLIDER),
        ]
    );
    assert_eq!(
        read_object(address(hwnd, BUTTON)).name.as_deref(),
        Some("Renamed")
    );

    app.quit();
}

fn msaa_set_value_raises_a_value_change_win_event() {
    common::init_com();
    let title = common::unique_title("mockapp-events-msaa-value");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let (_hook, events) = hook(app.pid());

    app.send("set-value slider1 77");
    app.send("set-name btn1 Marker");
    assert_eq!(
        pump_until(&events, 2),
        [
            win_event(WinEventKind::ValueChange, hwnd, SLIDER),
            win_event(WinEventKind::NameChange, hwnd, BUTTON),
        ]
    );
    assert_eq!(
        read_object(address(hwnd, SLIDER)).value.as_deref(),
        Some("77")
    );

    app.quit();
}

fn msaa_select_raises_a_selection_win_event() {
    common::init_com();
    let title = common::unique_title("mockapp-events-msaa-select");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let (_hook, events) = hook(app.pid());

    app.send("select item1");
    app.send("set-name btn1 Marker");
    assert_eq!(
        pump_until(&events, 2),
        [
            win_event(WinEventKind::Selection, hwnd, FIRST_ITEM),
            win_event(WinEventKind::NameChange, hwnd, BUTTON),
        ]
    );
    let item = read_object(address(hwnd, FIRST_ITEM));
    assert_eq!(item.name.as_deref(), Some("First"));
    assert_eq!(
        item.states,
        [State::Focusable, State::Selectable, State::Selected]
            .into_iter()
            .collect::<StateSet>()
    );
    assert!(
        !read_object(address(hwnd, SECOND_ITEM))
            .states
            .contains(State::Selected),
        "only the item selected is"
    );

    app.quit();
}

/// A command naming a node the fixture lacks is rejected, not applied, so
/// a typo in a test fails at once.
fn a_command_for_an_unknown_node_is_rejected() {
    let title = common::unique_title("mockapp-events-unknown");
    let mut app = common::spawn("small.json", "uia", &title);
    assert_eq!(
        app.send_answered("set-name nosuch Renamed"),
        "rejected: no node has the id nosuch"
    );
    assert_eq!(
        app.send_answered("frobnicate btn1"),
        "rejected: unrecognized command: frobnicate btn1"
    );
    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "uia_set_name_raises_a_property_changed_event",
            uia_set_name_raises_a_property_changed_event,
        ),
        (
            "uia_set_value_raises_a_property_changed_event",
            uia_set_value_raises_a_property_changed_event,
        ),
        (
            "uia_select_raises_a_selection_event",
            uia_select_raises_a_selection_event,
        ),
        (
            "uia_notify_raises_a_notification_event",
            uia_notify_raises_a_notification_event,
        ),
        (
            "uia_one_group_delivers_each_of_its_subscriptions",
            uia_one_group_delivers_each_of_its_subscriptions,
        ),
        (
            "uia_active_text_position_arrives_with_its_range",
            uia_active_text_position_arrives_with_its_range,
        ),
        (
            "uia_only_the_focus_is_followed",
            uia_only_the_focus_is_followed,
        ),
        (
            "msaa_set_name_raises_a_name_change_win_event",
            msaa_set_name_raises_a_name_change_win_event,
        ),
        (
            "msaa_set_value_raises_a_value_change_win_event",
            msaa_set_value_raises_a_value_change_win_event,
        ),
        (
            "msaa_select_raises_a_selection_win_event",
            msaa_select_raises_a_selection_win_event,
        ),
        (
            "a_command_for_an_unknown_node_is_rejected",
            a_command_for_an_unknown_node_is_rejected,
        ),
    ]);
}
