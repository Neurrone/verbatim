//! Cross-process event delivery (architecture section 13, layer 2).
//!
//! After a `set-name` or `set-value` stdin command, asserts that the
//! matching real client-side registration observes the change:
//! `verbatim_uia::PropertyRegistration` for UIA, `verbatim_ia2::WinEventHook`
//! for MSAA. Property and value changes are used rather than focus, per the
//! architecture note that these tests must pass headless on CI runners
//! without real keyboard focus or `SetForegroundWindow` succeeding.

mod common;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use verbatim_ia2::{WinEventHook, WinEventKind};
use verbatim_uia::{PropertyRegistration, Uia};
use windows::Win32::UI::Accessibility::{UIA_NamePropertyId, UIA_ValueValuePropertyId};
use windows::Win32::UI::WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageW, TranslateMessage};

/// Serializes UIA `PropertyRegistration` setup across this binary's tests.
/// `cargo test` runs `#[test]` functions concurrently on a thread pool by
/// default, and empirically, two `PropertyRegistration::new` calls racing
/// from different threads in the same process can make UI Automation's
/// internal event-registration state return a spurious `E_FAIL`
/// ("Unspecified error"). Real outposts never register UIA property
/// handlers concurrently from two threads for the same reason `verbatim-uia`
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
