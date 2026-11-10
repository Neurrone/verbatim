//! Cross-process event delivery (architecture section 13, layer 2).
//!
//! After a `set-name`, `flood`, `set-value`, `select`, `notify`, or
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

use verbatim_ia2::{APP_SUBSCRIPTIONS, WinEvent, WinEventHook, WinEventKind};
use verbatim_model::{NotificationKind, NotificationProcessing, State, StateSet};
use verbatim_uia::{
    Cached, ElementExt as _, FOCUS_PROPERTIES, ListenerEvent, ListenerSelection, Part,
    PropertyEvent, RecordedEvent, Registration, Scope, Subscription, Uia,
    map::{ignores_notifications, notification_kind_from_uia, notification_processing_from_uia},
};
use windows::Win32::Foundation::{HWND, WAIT_TIMEOUT};
use windows::Win32::UI::Accessibility::{
    UIA_AutomationFocusChangedEventId, UIA_ButtonControlTypeId, UIA_HasKeyboardFocusPropertyId,
    UIA_NamePropertyId, UIA_SelectionItem_ElementSelectedEventId, UIA_ValueValuePropertyId,
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

/// A property change as a registration hears it: the sender's name and
/// value, read as the `PropertyEvent` shape the outpost's registration
/// delivers, and the property.
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
    let registration = Registration::with_parts(
        vec![Part::shaped::<PropertyEvent>(Subscription::Properties {
            properties: FOCUS_PROPERTIES.to_vec(),
            callback: Arc::new(move |element: &Cached<PropertyEvent>, property| {
                let read = element.read();
                // The test may have finished.
                let _ = sender.send(PropertyChange {
                    name: read.name,
                    value: read.value.value,
                    property,
                });
            }),
        })],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("Registration::with_parts");
    (registration, changes)
}

/// The roles and names of the root's children, read afresh through a new
/// client, each read as a focus event's element is
/// ([`ListenerEvent`]): what neither the title bar's location nor its
/// states are among, which UIA asks of the window's thread
/// (`WM_GETTITLEBARINFOEX`) and Windows answers for one window at a time
/// across the desktop, seconds on a busy machine.
fn children_names(hwnd: HWND) -> Vec<(verbatim_model::Role, Option<String>)> {
    let uia = Uia::new().expect("Uia::new");
    let request = uia
        .request_fixed::<ListenerEvent>()
        .expect("the focus event's cache request");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    uia.children(&root, &request)
        .expect("the window's children")
        .iter()
        .map(|child| {
            let read = child.read();
            (read.role.role(), read.name)
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

/// `flood` raises each of its name changes from the root. Raised back to
/// back, the window thread answers no call until the last is raised, and
/// UIA fills each event's cache with calls into the window, so every one
/// carries the last name.
fn uia_flood_raises_each_name_change() {
    let title = common::unique_title("mockapp-events-uia-flood");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let (_registration, changes) = property_changes(hwnd);

    app.send("flood 3 0");
    app.send("set-name btn1 Marker");
    let renamed = |name: &str| PropertyChange {
        name: Some(name.to_owned()),
        value: None,
        property: UIA_NamePropertyId.0,
    };
    expect_events(
        &changes,
        &[
            renamed("Flood 3"),
            renamed("Flood 3"),
            renamed("Flood 3"),
            renamed("Marker"),
        ],
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
    let first = AgileReference::new(&common::named_element(&uia, hwnd, "First"))
        .expect("the button First, agile");

    let (sender, names) = mpsc::channel();
    let _registration = Registration::with_parts(
        vec![Part::shaped::<PropertyEvent>(Subscription::Properties {
            properties: FOCUS_PROPERTIES.to_vec(),
            callback: Arc::new(move |element: &Cached<PropertyEvent>, property| {
                let _ = sender.send((element.read().name, property));
            }),
        })],
        Scope::Elements(vec![first]),
    )
    .expect("Registration::with_parts");

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

/// A selected element as a registration hears it, read as a
/// `PropertyEvent` and mapped exactly as an outpost maps one: its name and
/// its whole state set.
fn selected(element: &Cached<PropertyEvent>) -> (Option<String>, StateSet) {
    let parts = element.read().parts(element.runtime_id());
    (parts.name, parts.states)
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
    let _registration = Registration::with_parts(
        vec![
            Part::shaped::<PropertyEvent>(Subscription::Event {
                event: UIA_SelectionItem_ElementSelectedEventId,
                callback: Arc::new(move |element| {
                    let _ = sender.send(Some(selected(element)));
                }),
            }),
            Part::shaped::<ListenerEvent>(Subscription::Notifications {
                callback: Arc::new(move |_, _, _, _, _| {
                    let _ = marker.send(None);
                }),
            }),
        ],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("Registration::with_parts");

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
    let _registration = Registration::with_parts(
        vec![Part::shaped::<ListenerEvent>(Subscription::Notifications {
            callback: Arc::new(move |_element, kind, processing, display, activity| {
                let _ = sender.send((
                    notification_kind_from_uia(kind),
                    notification_processing_from_uia(processing),
                    display,
                    activity,
                ));
            }),
        })],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("Registration::with_parts");

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

/// What the focus listener reads of a selection's or notification's
/// sender: the event, the sender's role and name from its parts, and, for
/// a notification, whether its sender's notifications are ignored.
type ListenerRead = (
    &'static str,
    verbatim_model::Role,
    Option<String>,
    Option<bool>,
);

/// A focus event's cache, filled by the focus listener's request
/// (`ListenerEvent`), holds the same for `HasKeyboardFocus` when the
/// provider's `GetPropertyValue` for it fails as when it answers an empty
/// value: UIA's default, which a read that ignores defaults answers as
/// "not supported" and a plain read as `false`; and a live read
/// (`CurrentHasKeyboardFocus`, NVDA's) answers `false` for both, with no
/// error. So neither the cache nor a live read tells a provider that failed
/// from one that answered nothing, and a provider that answers reads as it
/// answered.
fn uia_a_focus_events_cache_holds_a_failed_keyboard_focus_as_an_empty_one() {
    let title = common::unique_title("mockapp-events-uia-failed-focus");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let (sender, observed) = mpsc::channel::<(Option<String>, (String, String, String))>();
    let registration = Registration::with_parts(
        vec![Part::shaped::<ListenerEvent>(Subscription::Event {
            event: UIA_AutomationFocusChangedEventId,
            callback: Arc::new(move |element: &Cached<ListenerEvent>| {
                let observe = |ignore_default| {
                    describe(element.observe_cached(UIA_HasKeyboardFocusPropertyId, ignore_default))
                };
                let live = match element.live().element().has_keyboard_focus() {
                    Ok(focus) => format!("{focus}"),
                    Err(error) => format!("failed {:#010x}", error.code().0),
                };
                let _ = sender.send((element.read().name, (observe(true), observe(false), live)));
            }),
        })],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("Registration::with_parts");

    // Each event's cache is filled after its command is acknowledged, so
    // each answer stays set until its event has arrived.
    let property = UIA_HasKeyboardFocusPropertyId.0;
    // The next focus event, which must be `item`'s, named `name`.
    let mut observe = |answer: &str, item: &str, name: &str| {
        app.send(&format!("answer-property {property} {answer}"));
        app.send(&format!("focus {item}"));
        let (sender, answers) = observed
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|error| panic!("no focus event from {item}: {error}"));
        assert_eq!(sender.as_deref(), Some(name), "the focus event's sender");
        answers
    };
    let pair = |ignored: &str, plain: &str, live: &str| {
        (ignored.to_owned(), plain.to_owned(), live.to_owned())
    };
    assert_eq!(
        [
            observe("fail", "item1", "First"),
            observe("empty", "item2", "Second"),
            observe("usual", "item1", "First"),
            // A marker: its event coming next shows no other came first.
            observe("usual", "item2", "Second"),
        ],
        [
            pair("not supported", "false", "false"),
            pair("not supported", "false", "false"),
            pair("true", "true", "true"),
            pair("true", "true", "true"),
        ]
    );
    registration.close();
    app.quit();
}

/// A cached read's answer, described: its failure's code, UIA's "not
/// supported" sentinel, a boolean, an empty value, or another type.
fn describe(answer: windows::core::Result<windows::Win32::System::Variant::VARIANT>) -> String {
    match answer {
        Err(error) => format!("failed {:#010x}", error.code().0),
        Ok(value) if verbatim_uia::is_not_supported(&value) => "not supported".to_owned(),
        Ok(value) => match value.vt() {
            windows::Win32::System::Variant::VT_BOOL => {
                format!("{}", bool::try_from(&value).unwrap_or(false))
            }
            windows::Win32::System::Variant::VT_EMPTY => "empty".to_owned(),
            other => format!("type {}", other.0),
        },
    }
}

/// A registration of the focus listener's shapes, `ListenerSelection` for
/// selections and `ListenerEvent` for notifications, on mockapp's window
/// reads each event's sender as the listener does, its read and parts and,
/// for a notification, `ignores_notifications` from its class name and
/// automation id, with no failed cached read.
fn uia_listener_reads_its_facts_with_no_failed_read() {
    let title = common::unique_title("mockapp-events-uia-listener-facts");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let (sender, reads) = mpsc::channel::<ListenerRead>();
    let notified = sender.clone();
    let registration = Registration::with_parts(
        vec![
            Part::shaped::<ListenerSelection>(Subscription::Event {
                event: UIA_SelectionItem_ElementSelectedEventId,
                callback: Arc::new(move |element: &Cached<ListenerSelection>| {
                    let parts = element.read().parts(element.runtime_id());
                    let _ = sender.send(("selection", parts.role, parts.name, None));
                }),
            }),
            Part::shaped::<ListenerEvent>(Subscription::Notifications {
                callback: Arc::new(move |element: &Cached<ListenerEvent>, _, _, _, _| {
                    let fields = element.read();
                    let ignored = ignores_notifications(
                        fields.role.class_name.as_deref(),
                        fields.automation_id.as_deref(),
                    );
                    let parts = fields.parts(element.runtime_id());
                    let _ = notified.send(("notification", parts.role, parts.name, Some(ignored)));
                }),
            }),
        ],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("Registration::with_parts");

    app.send("select item1");
    app.send("notify Window snapped to the left");
    // The marker.
    app.send("select item2");
    expect_events(
        &reads,
        &[
            (
                "selection",
                verbatim_model::Role::ListItem,
                Some("First".to_owned()),
                None,
            ),
            (
                "notification",
                verbatim_model::Role::Window,
                Some("Mockapp Events Fixture".to_owned()),
                Some(false),
            ),
            (
                "selection",
                verbatim_model::Role::ListItem,
                Some("Second".to_owned()),
                None,
            ),
        ],
    );
    // Closing waits for every callback in progress, so each has counted
    // its reads.
    registration.close();
    assert_eq!(registration.failed_reads(), 0);

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
    // Read as a `PropertyEvent`, whose request holds the sender's name.
    let _registration = Registration::with_parts(
        vec![Part::shaped::<PropertyEvent>(
            Subscription::ActiveTextPosition {
                callback: Arc::new(move |element: &Cached<PropertyEvent>, range| {
                    let name = element.read().name;
                    let range = range.and_then(|range| AgileReference::new(range).ok());
                    let _ = sender.send((name, range));
                }),
            },
        )],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("Registration::with_parts");

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
    // A marker: its event coming next shows no other came after the two.
    app.send("active-text-position doc 1 4");
    let (_, range) = positions
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the marker's event");
    let range = range
        .expect("the marker's range")
        .resolve()
        .expect("the range");
    assert_eq!(
        String::from_utf16_lossy(&range.text(-1).expect("the range's text")),
        "lph",
        "the next event is the marker's"
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
    let _registration = Registration::with_parts(
        vec![
            Part::shaped::<PropertyEvent>(Subscription::Properties {
                properties: FOCUS_PROPERTIES.to_vec(),
                callback: Arc::new(move |_, _| property()),
            }),
            Part::shaped::<ListenerEvent>(Subscription::Event {
                event: UIA_SelectionItem_ElementSelectedEventId,
                callback: Arc::new(move |_| selection()),
            }),
            Part::shaped::<ListenerEvent>(Subscription::Notifications {
                callback: Arc::new(move |_, _, _, _, _| notification()),
            }),
        ],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("Registration::with_parts");

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
        Box::new(move |event: WinEvent| {
            let _ = sender.send((event.kind, event.hwnd, event.id_object, event.id_child));
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

/// The MSAA flood raises a value change on a new object each time, then
/// on the fixture's own objects, then a show on a new object each time,
/// from a tooltip window whose handle the test cannot name.
fn msaa_flood_raises_each_kind_on_its_objects() {
    common::init_com();
    let title = common::unique_title("mockapp-events-msaa-flood");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let (_hook, events) = hook(app.pid());

    app.send("flood 2 0 new-values");
    app.send("flood 1 0 repeated-values");
    app.send("flood 2 0 new-shows");
    let events = pump_until(&events, 5);
    let of_kind = |&(kind, _, object, child): &SeenWinEvent| (kind, object, child);
    assert_eq!(
        events.iter().map(of_kind).collect::<Vec<_>>(),
        [
            (WinEventKind::ValueChange, 1000, 0),
            (WinEventKind::ValueChange, 1001, 0),
            (WinEventKind::ValueChange, 1, 0),
            (WinEventKind::Show, 1000, 0),
            (WinEventKind::Show, 1001, 0),
        ]
    );
    assert_eq!(
        events[..3].iter().map(|event| event.1).collect::<Vec<_>>(),
        [hwnd.0 as isize; 3],
        "the value changes are the window's own"
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
/// A window whose fixture asks for a dialog's keys (`owned_dialog.json`)
/// moves the focus among its buttons by Tab and Shift+Tab, in tree order and
/// around, and closes when Enter presses the focused button: the keys are
/// sent to the hidden window as its messages, so no key reaches the
/// desktop.
fn dialog_keys_move_between_buttons_and_close_the_window() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_RETURN, VK_TAB};
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SendMessageW, WM_KEYDOWN};
    common::init_com();
    let title = common::unique_title("mockapp-dialog-keys");
    let app = common::spawn("owned_dialog.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let focused = |name: &str| {
        let element = common::named_element(&uia, hwnd, name);
        // SAFETY: a live element; the read returns a plain boolean.
        unsafe { element.CurrentHasKeyboardFocus() }
            .expect("the button's keyboard focus")
            .as_bool()
    };
    let key = |key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| {
        // SAFETY: a message to mockapp's live window, answered by its
        // window procedure before the call returns.
        let _ = unsafe {
            SendMessageW(
                hwnd,
                WM_KEYDOWN,
                Some(windows::Win32::Foundation::WPARAM(usize::from(key.0))),
                Some(windows::Win32::Foundation::LPARAM(0)),
            )
        };
    };
    let before = (focused("OK"), focused("Cancel"));
    key(VK_TAB);
    let after_tab = (focused("OK"), focused("Cancel"));
    key(VK_TAB);
    let around = (focused("OK"), focused("Cancel"));
    key(VK_RETURN);
    // SAFETY: a local check of a window handle.
    let open = unsafe { IsWindow(Some(hwnd)) }.as_bool();
    assert_eq!(
        (before, after_tab, around, open),
        ((true, false), (false, true), (true, false), false),
        "the focus before and after each Tab, and the window after Enter"
    );
    drop(app);
}

/// A window whose fixture asks for it (`owner_window.json`,
/// `focus_on_activate`) raises UIA's focus event for its focused node,
/// "Open", when it is activated, as an application restores its focus on
/// reactivation: the activation is sent to the hidden window as its
/// `WM_ACTIVATE` message, so nothing on the desktop is activated.
fn focus_on_activate_raises_the_focused_nodes_focus_event() {
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WA_ACTIVE, WM_ACTIVATE};
    let title = common::unique_title("mockapp-focus-on-activate");
    let app = common::spawn("owner_window.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let (sender, observed) = mpsc::channel::<Option<String>>();
    let registration = Registration::with_parts(
        vec![Part::shaped::<ListenerEvent>(Subscription::Event {
            event: UIA_AutomationFocusChangedEventId,
            callback: Arc::new(move |element: &Cached<ListenerEvent>| {
                let _ = sender.send(element.read().name);
            }),
        })],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("Registration::with_parts");
    // SAFETY: a message to mockapp's live window, answered by its window
    // procedure before the call returns.
    let _ = unsafe {
        SendMessageW(
            hwnd,
            WM_ACTIVATE,
            Some(windows::Win32::Foundation::WPARAM(WA_ACTIVE as usize)),
            Some(windows::Win32::Foundation::LPARAM(0)),
        )
    };
    let name = observed
        .recv_timeout(common::WAIT_TIMEOUT)
        .unwrap_or_else(|error| panic!("no focus event after the activation: {error}"));
    assert_eq!(name.as_deref(), Some("Open"), "the focus event's sender");
    registration.close();
    app.quit();
}

/// The listener's UIA subscription for progress bars, on a real provider's
/// cache: a progress bar's change, with no range, is its value, and a
/// slider's range change is dropped by its control type in the callback.
fn the_listeners_callback_keeps_a_progress_bar_and_drops_a_slider() {
    common::init_com();
    let title = common::unique_title("mockapp-listener-bar");
    let mut app = common::spawn("progress.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let (sender, bars) = mpsc::channel();
    let _registration = Registration::with_parts(
        vec![Part::shaped::<verbatim_uia::ListenerProgress>(
            Subscription::Properties {
                properties: vec![windows::Win32::UI::Accessibility::UIA_RangeValueValuePropertyId],
                callback: Arc::new(move |element: &Cached<verbatim_uia::ListenerProgress>, _| {
                    let _ = sender.send(verbatim_listener::listener::bar_percent(&element.read()));
                }),
            },
        )],
        common::window_subtree(hwnd.0 as isize),
    )
    .expect("Registration::with_parts");
    app.send("set-range level 2");
    app.send("set-range bar 40");
    assert_eq!(
        (
            bars.recv_timeout(common::WAIT_TIMEOUT),
            bars.recv_timeout(common::WAIT_TIMEOUT)
        ),
        (Ok(None), Ok(verbatim_model::Percent::parse("40")))
    );
    app.quit();
}

/// What `read_classic_bar` reports and learns for `object`, read
/// synchronously here as the listener's reading thread reads it.
fn classic_read(
    object: verbatim_listener_logic::focus::BarObject,
    read: verbatim_listener_logic::focus::ClassicBarRead,
) -> (
    Option<verbatim_model::Percent>,
    Option<verbatim_listener_logic::focus::BarVerdict>,
    Option<verbatim_listener_logic::focus::WindowAnswer>,
) {
    use verbatim_model::protocol::Responder;
    let (reports, changed) = mpsc::channel();
    let (verdicts, learned) = mpsc::channel();
    let (windows, answered) = mpsc::channel();
    verbatim_listener::listener::read_classic_bar(
        object,
        read,
        Responder::new(move |change: Option<verbatim_model::BarChange>| {
            if let Some(change) = change {
                let _ = reports.send(change.percent);
            }
        }),
        Responder::new(
            move |verdict: Option<verbatim_listener_logic::focus::BarVerdict>| {
                if let Some(verdict) = verdict {
                    let _ = verdicts.send(verdict);
                }
            },
        ),
        Responder::new(
            move |answer: Option<verbatim_listener_logic::focus::WindowAnswer>| {
                if let Some(answer) = answer {
                    let _ = windows.send(answer);
                }
            },
        ),
    );
    (
        changed.try_recv().ok(),
        learned.try_recv().ok(),
        answered.try_recv().ok(),
    )
}

/// A classic bar known only by its role, in a window with no outpost, is
/// found to be a bar by the role read and reported with its percentage.
fn a_classic_bar_is_read_by_its_role_with_no_outpost() {
    common::init_com();
    let title = common::unique_title("mockapp-classic-bar");
    let mut app = common::spawn("progress.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    app.send("set-value bar 40");
    assert_eq!(
        classic_read(
            verbatim_listener_logic::focus::BarObject {
                window: hwnd.0 as isize,
                object: windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
                child: 1,
            },
            verbatim_listener_logic::focus::ClassicBarRead::Probe,
        ),
        (
            verbatim_model::Percent::parse("40"),
            Some(verbatim_listener_logic::focus::BarVerdict::Bar),
            Some(verbatim_listener_logic::focus::WindowAnswer::NotUia)
        )
    );
    app.quit();
}

/// A native UIA window's classic value change is no bar's: the raw probe
/// answers before any MSAA call, and the window's answer says so.
fn a_native_uia_windows_value_change_is_remembered_as_no_bar() {
    common::init_com();
    let title = common::unique_title("mockapp-classic-native");
    let app = common::spawn("progress.json", "uia", &title);
    let hwnd = common::find_window(&title);
    assert_eq!(
        classic_read(
            verbatim_listener_logic::focus::BarObject {
                window: hwnd.0 as isize,
                object: windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
                child: 0,
            },
            verbatim_listener_logic::focus::ClassicBarRead::Probe,
        ),
        (
            None,
            None,
            Some(verbatim_listener_logic::focus::WindowAnswer::Uia)
        )
    );
    app.quit();
}
/// A `RecordedEvent` delivered with a property change reads its whole
/// line, the runtime id included, from its cache: mockapp's providers answer
/// no call between the delivery and the end of the read. mockapp raises the
/// event with an empty new value, which the callback receives as such.
fn a_recorded_event_is_read_with_no_call_after_its_delivery() {
    let title = common::unique_title("mockapp-events-recorded");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let window = hwnd.0 as isize;
    let (sender, reads) = mpsc::channel();
    let _registration =
        Registration::with_parts(
            vec![Part::shaped::<RecordedEvent>(Subscription::PropertyValues {
            properties: vec![UIA_NamePropertyId],
            callback: Arc::new(
                move |element: &Cached<RecordedEvent>,
                      property,
                      value: &windows::Win32::System::Variant::VARIANT| {
                    let hwnd = HWND(window as _);
                    common::reset_hits(hwnd);
                    let read = element.read();
                    let hits = common::read_hits(hwnd);
                    let _ = sender.send((
                        read.name,
                        read.control_type,
                        read.runtime_id.is_some_and(|id| !id.is_empty()),
                        verbatim_uia::variant_string(value),
                        property,
                        hits,
                    ));
                },
            ),
        })],
            common::window_subtree(window),
        )
        .expect("Registration::with_parts");

    app.send("set-name btn1 Renamed");
    expect_events(
        &reads,
        &[(
            Some("Renamed".to_owned()),
            Some(UIA_ButtonControlTypeId.0),
            true,
            None,
            UIA_NamePropertyId.0,
            Vec::new(),
        )],
    );
    app.quit();
}

fn main() {
    harness::run_isolated(&[
        (
            "a_recorded_event_is_read_with_no_call_after_its_delivery",
            a_recorded_event_is_read_with_no_call_after_its_delivery,
        ),
        (
            "the_listeners_callback_keeps_a_progress_bar_and_drops_a_slider",
            the_listeners_callback_keeps_a_progress_bar_and_drops_a_slider,
        ),
        (
            "a_native_uia_windows_value_change_is_remembered_as_no_bar",
            a_native_uia_windows_value_change_is_remembered_as_no_bar,
        ),
        (
            "a_classic_bar_is_read_by_its_role_with_no_outpost",
            a_classic_bar_is_read_by_its_role_with_no_outpost,
        ),
        (
            "focus_on_activate_raises_the_focused_nodes_focus_event",
            focus_on_activate_raises_the_focused_nodes_focus_event,
        ),
        (
            "dialog_keys_move_between_buttons_and_close_the_window",
            dialog_keys_move_between_buttons_and_close_the_window,
        ),
        (
            "uia_set_name_raises_a_property_changed_event",
            uia_set_name_raises_a_property_changed_event,
        ),
        (
            "uia_flood_raises_each_name_change",
            uia_flood_raises_each_name_change,
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
            "uia_listener_reads_its_facts_with_no_failed_read",
            uia_listener_reads_its_facts_with_no_failed_read,
        ),
        (
            "uia_a_focus_events_cache_holds_a_failed_keyboard_focus_as_an_empty_one",
            uia_a_focus_events_cache_holds_a_failed_keyboard_focus_as_an_empty_one,
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
            "msaa_flood_raises_each_kind_on_its_objects",
            msaa_flood_raises_each_kind_on_its_objects,
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
