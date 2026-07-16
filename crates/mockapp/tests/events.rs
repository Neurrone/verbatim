//! Cross-process event delivery (architecture section 13, layer 2).
//!
//! After a `set-name`, `set-value`, `select`, or `notify` stdin command,
//! asserts that the matching real client-side registration observes the
//! change: `verbatim_uia::PropertyRegistration`, `SelectionRegistration`,
//! and `NotificationRegistration` for UIA, `verbatim_ia2::WinEventHook` for
//! MSAA. Property, value, selection, and notification changes are used
//! rather than focus, per the architecture note that these tests must pass
//! headless on CI runners without real keyboard focus or
//! `SetForegroundWindow` succeeding.

mod common;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use verbatim_ia2::{APP_SUBSCRIPTIONS, WinEventHook, WinEventKind};
use verbatim_model::{NotificationKind, NotificationProcessing, State};
use verbatim_uia::{
    NotificationRegistration, PropertyRegistration, SelectionRegistration, Uia,
    map::{notification_kind_from_uia, notification_processing_from_uia},
};
use windows::Win32::UI::Accessibility::{UIA_NamePropertyId, UIA_ValueValuePropertyId};
use windows::Win32::UI::WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageW, TranslateMessage};

/// Serializes UIA registration setup (`PropertyRegistration`,
/// `SelectionRegistration`, `NotificationRegistration`) across this
/// binary's tests. `cargo test` runs `#[test]` functions concurrently on a
/// thread pool by default, and empirically, two registration calls racing
/// from different threads in the same process can make UI Automation's
/// internal event-registration state return a spurious `E_FAIL`
/// ("Unspecified error"). Real outposts never register UIA handlers
/// concurrently from two threads for the same reason `verbatim-uia`
/// gives each registration its own dedicated thread rather than sharing one;
/// this lock reproduces that single-registration-at-a-time discipline for
/// the tests.
fn uia_registration_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn uia_set_name_raises_a_property_changed_event() {
    let _guard = uia_registration_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let title = common::unique_title("mockapp-events-uia-name");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let seen = Arc::new(Mutex::new(Vec::<i32>::new()));
    let seen_cb = seen.clone();
    let _registration = PropertyRegistration::new(
        vec![hwnd.0 as isize],
        Arc::new(move |_element, property_id| {
            seen_cb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(property_id);
        }),
    )
    .expect("PropertyRegistration::new");

    app.send("set-name btn1 Renamed");

    common::wait_until("UIA name-changed property event", || {
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&UIA_NamePropertyId.0)
    });

    // Cross-check that the provider's live data actually changed, not just
    // that some event fired: re-fetch the button and confirm its cached
    // name is the new one.
    let uia = Uia::new().expect("Uia::new");
    let cache = uia.base_cache_request().expect("base cache request");
    let root = uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("element_from_handle");
    // SAFETY: `root` was built with the base cache request; `TreeScope_Children`
    // and the true condition are standard client-side arguments.
    let children = unsafe {
        root.FindAllBuildCache(
            windows::Win32::UI::Accessibility::TreeScope_Children,
            &uia.client()
                .CreateTrueCondition()
                .expect("CreateTrueCondition"),
            &cache,
        )
    }
    .expect("FindAllBuildCache");
    let registry =
        verbatim_uia::NodeIdRegistry::new(Arc::new(std::sync::atomic::AtomicU64::new(1)));
    // SAFETY: `children` is the array just returned above.
    let count = unsafe { children.Length() }.unwrap_or(0);
    let mut found_renamed = false;
    for i in 0..count {
        // SAFETY: `i` is within `[0, count)`.
        let child = unsafe { children.GetElement(i) }.expect("GetElement");
        // SAFETY: `child` was built with the base cache request.
        let snapshot =
            unsafe { verbatim_uia::map::snapshot_from_cached_element(&child, &registry) };
        if snapshot.name.as_deref() == Some("Renamed") {
            found_renamed = true;
        }
    }
    assert!(
        found_renamed,
        "the button's cached name did not actually change to \"Renamed\""
    );

    app.send("quit");
}

#[test]
fn uia_set_value_raises_a_property_changed_event() {
    let _guard = uia_registration_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let title = common::unique_title("mockapp-events-uia-value");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let seen = Arc::new(Mutex::new(Vec::<i32>::new()));
    let seen_cb = seen.clone();
    let _registration = PropertyRegistration::new(
        vec![hwnd.0 as isize],
        Arc::new(move |_element, property_id| {
            seen_cb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(property_id);
        }),
    )
    .expect("PropertyRegistration::new");

    app.send("set-value slider1 77");

    common::wait_until("UIA value-changed property event", || {
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&UIA_ValueValuePropertyId.0)
    });

    app.send("quit");
}

#[test]
fn uia_select_raises_a_selection_event() {
    let _guard = uia_registration_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let title = common::unique_title("mockapp-events-uia-select");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);

    // Capture full snapshots (mapped exactly as an outpost would map them)
    // rather than a bare "an event fired" flag, so the assertion also
    // proves the selected element arrived with its cached properties.
    let seen = Arc::new(Mutex::new(Vec::<verbatim_model::NodeSnapshot>::new()));
    let seen_cb = seen.clone();
    let registry =
        verbatim_uia::NodeIdRegistry::new(Arc::new(std::sync::atomic::AtomicU64::new(1)));
    let _registration = SelectionRegistration::new(
        vec![hwnd.0 as isize],
        Arc::new(move |element| {
            // SAFETY: the element was delivered with the registration's own
            // base cache request, so the mapping reads only cached values.
            let snapshot =
                unsafe { verbatim_uia::map::snapshot_from_cached_element(element, &registry) };
            seen_cb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(snapshot);
        }),
    )
    .expect("SelectionRegistration::new");

    app.send("select item1");

    common::wait_until("UIA element-selected event for item1", || {
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|snapshot| {
                snapshot.name.as_deref() == Some("First")
                    && snapshot.states.contains(State::Selected)
            })
    });

    app.send("quit");
}

/// One observed notification: kind, processing, display string, activity id.
type SeenNotification = (
    NotificationKind,
    NotificationProcessing,
    Option<String>,
    Option<String>,
);

#[test]
fn uia_notify_raises_a_notification_event() {
    let _guard = uia_registration_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let title = common::unique_title("mockapp-events-uia-notify");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let seen = Arc::new(Mutex::new(Vec::<SeenNotification>::new()));
    let seen_cb = seen.clone();
    let _registration = NotificationRegistration::new(
        vec![hwnd.0 as isize],
        Arc::new(move |_element, kind, processing, display, activity| {
            seen_cb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    notification_kind_from_uia(kind),
                    notification_processing_from_uia(processing),
                    display,
                    activity,
                ));
        }),
    )
    .expect("NotificationRegistration::new");

    app.send("notify Window snapped to the left");

    common::wait_until("UIA notification event with the sent payload", || {
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|(kind, processing, display, activity)| {
                *kind == NotificationKind::Other
                    && *processing == NotificationProcessing::All
                    && display.as_deref() == Some("Window snapped to the left")
                    && activity.as_deref() == Some("mockapp-notify")
            })
    });

    app.send("quit");
}

/// Pumps this thread's message queue while waiting for `condition`, since
/// out-of-context `WinEvent`s (`WinEventHook`) deliver via the installing
/// thread's message loop — a plain sleep loop would never see them.
fn pump_wait_until(message: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + common::WAIT_TIMEOUT;
    let mut msg = MSG::default();
    loop {
        if condition() {
            return;
        }
        // SAFETY: standard non-blocking message pump; `msg` is written by
        // `PeekMessageW` when it returns a message.
        unsafe {
            if PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&raw const msg);
                windows::Win32::UI::WindowsAndMessaging::DispatchMessageW(&raw const msg);
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {:?} waiting for: {message}",
            common::WAIT_TIMEOUT
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn msaa_set_name_raises_a_name_change_win_event() {
    let title = common::unique_title("mockapp-events-msaa-name");
    let mut app = common::spawn("small.json", "msaa", &title);
    let _hwnd = common::find_window(&title);
    let pid = app.pid();

    let seen = Arc::new(Mutex::new(Vec::<WinEventKind>::new()));
    let seen_cb = seen.clone();
    let _hook = WinEventHook::install(
        pid,
        APP_SUBSCRIPTIONS,
        Box::new(move |kind, _hwnd, _id_object, _id_child| {
            seen_cb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(kind);
        }),
    )
    .expect("WinEventHook::install");

    app.send("set-name btn1 Renamed");

    pump_wait_until("MSAA name-change WinEvent", || {
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&WinEventKind::NameChange)
    });

    app.send("quit");
}

#[test]
fn msaa_set_value_raises_a_value_change_win_event() {
    let title = common::unique_title("mockapp-events-msaa-value");
    let mut app = common::spawn("small.json", "msaa", &title);
    let _hwnd = common::find_window(&title);
    let pid = app.pid();

    let seen = Arc::new(Mutex::new(Vec::<WinEventKind>::new()));
    let seen_cb = seen.clone();
    let _hook = WinEventHook::install(
        pid,
        APP_SUBSCRIPTIONS,
        Box::new(move |kind, _hwnd, _id_object, _id_child| {
            seen_cb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(kind);
        }),
    )
    .expect("WinEventHook::install");

    app.send("set-value slider1 77");

    pump_wait_until("MSAA value-change WinEvent", || {
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&WinEventKind::ValueChange)
    });

    app.send("quit");
}

#[test]
fn msaa_select_raises_a_selection_win_event() {
    let title = common::unique_title("mockapp-events-msaa-select");
    let mut app = common::spawn("small.json", "msaa", &title);
    let _hwnd = common::find_window(&title);
    let pid = app.pid();

    let seen = Arc::new(Mutex::new(Vec::<WinEventKind>::new()));
    let seen_cb = seen.clone();
    let _hook = WinEventHook::install(
        pid,
        APP_SUBSCRIPTIONS,
        Box::new(move |kind, _hwnd, _id_object, _id_child| {
            seen_cb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(kind);
        }),
    )
    .expect("WinEventHook::install");

    app.send("select item1");

    pump_wait_until("MSAA selection WinEvent", || {
        seen.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&WinEventKind::Selection)
    });

    app.send("quit");
}
