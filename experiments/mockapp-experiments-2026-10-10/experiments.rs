//! Design experiments for the focus pipeline (scratchpad
//! `focus-pipeline-design.md`, section 11). Not tests: each prints what it
//! measured, as lines starting `RESULT`, and fails only when its setup does.
//! Kept on the experiment branch only.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::io::BufReader;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use verbatim_outpost::Outpost;
use verbatim_outpost::listener::uia_focus_fact;
use verbatim_outpost::protocol::{ListenerFact, OutpostToSupervisor, SupervisorToOutpost, read_message};
use verbatim_uia::{ElementExt, Registration, Scope, Subscription, Uia};
use windows::Win32::UI::Accessibility::{
    ConnectionRecoveryBehaviorOptions_Disabled, ConnectionRecoveryBehaviorOptions_Enabled,
    IUIAutomation2, IUIAutomation6, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationEventHandler, IUIAutomationFocusChangedEventHandler, TreeScope_Descendants,
    TreeScope_Subtree, UIA_AutomationFocusChangedEventId, UIA_NamePropertyId,
    UIA_NativeWindowHandlePropertyId, UIA_ProcessIdPropertyId, UIA_RuntimeIdPropertyId,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, Interface};

const LONG: Duration = Duration::from_secs(40);

fn ms(since: Instant) -> u128 {
    since.elapsed().as_millis()
}

/// A mockapp with the small UIA fixture, and its window.
fn start(prefix: &str) -> (common::MockApp, isize) {
    let title = common::unique_title(prefix);
    let app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;
    (app, hwnd)
}

/// The minimal cache section 2.3 proposes: process id, runtime id, window,
/// and the name, to tell events apart.
fn minimal_cache(uia: &Uia) -> IUIAutomationCacheRequest {
    uia.cache_request(&[
        UIA_ProcessIdPropertyId,
        UIA_RuntimeIdPropertyId,
        UIA_NativeWindowHandlePropertyId,
        UIA_NamePropertyId,
    ])
    .expect("cache request")
}

fn find(uia: &Uia, hwnd: isize, name: &str) -> IUIAutomationElement {
    let cache = uia.base_cache_request().expect("cache");
    let root = uia.element_from_handle(hwnd, &cache).expect("root");
    let value = VARIANT::from(BSTR::from(name));
    let condition = uia
        .property_condition(UIA_NamePropertyId, &value)
        .expect("condition");
    root.find_first_build_cache(TreeScope_Descendants, &condition, &cache)
        .expect("find")
        .expect("found")
}

fn current_name(element: &IUIAutomationElement) -> windows::core::Result<String> {
    // SAFETY: a live element; a cross-process read.
    unsafe { element.CurrentName() }.map(|name| name.to_string())
}

/// Sends `line` and waits for "applied", printing anything else it hears
/// first (a `held` line, for instance).
fn send_seeing(app: &mut common::MockApp, line: &str) -> Vec<String> {
    app.write(line);
    let mut other = Vec::new();
    loop {
        match app.line(LONG) {
            Some(answer) if answer == "applied" => return other,
            Some(answer) => other.push(answer),
            None => panic!("mockapp did not apply {line:?}"),
        }
    }
}

/// Waits for a `held` line.
fn held(app: &common::MockApp, within: Duration) -> Option<String> {
    app.line(within)
}

/// A desktop-wide focus registration on this thread's own client with
/// `cache`, sending (pid, name) for each event.
fn focus_registration(uia: &Uia, cache: &IUIAutomationCacheRequest) -> (IUIAutomationFocusChangedEventHandler, Receiver<(i32, Option<String>, Instant)>) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let handler: IUIAutomationFocusChangedEventHandler = handler::Focus {
        callback: Arc::new(move |element: &IUIAutomationElement| {
            let pid = element.cached_i32(UIA_ProcessIdPropertyId).unwrap_or(0);
            let name = element.cached_string(UIA_NamePropertyId);
            let _ = tx
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send((pid, name, Instant::now()));
        }),
    }
    .into();
    // SAFETY: live client, cache and handler.
    unsafe { uia.client().AddFocusChangedEventHandler(cache, &handler) }.expect("register");
    (handler, rx)
}

/// A focus registration through an event handler group, scoped to the
/// subtrees of `hwnds`, with the minimal cache, sending (pid, name, when).
fn scoped_focus(hwnds: Vec<isize>) -> (Registration, Receiver<(i32, Option<String>, Instant)>) {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let registration = Registration::with_cache(
        vec![Subscription::Event {
            event: UIA_AutomationFocusChangedEventId,
            callback: Arc::new(move |element| {
                let _ = tx.lock().unwrap_or_else(PoisonError::into_inner).send((
                    element.cached_i32(UIA_ProcessIdPropertyId).unwrap_or(0),
                    element.cached_string(UIA_NamePropertyId),
                    Instant::now(),
                ));
            }),
        }],
        Scope::Windows(hwnds),
        &[
            UIA_ProcessIdPropertyId,
            UIA_RuntimeIdPropertyId,
            UIA_NativeWindowHandlePropertyId,
            UIA_NamePropertyId,
        ],
    )
    .expect("scoped focus registration");
    (registration, rx)
}

mod handler {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]
    use std::sync::Mutex;
    use std::sync::mpsc::Sender;
    use std::time::Instant;

    use verbatim_uia::ElementExt;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationEventHandler_Impl, UIA_EVENT_ID, UIA_NamePropertyId,
        UIA_ProcessIdPropertyId,
    };

    #[windows_core::implement(
        windows::Win32::UI::Accessibility::IUIAutomationFocusChangedEventHandler
    )]
    pub struct Focus {
        pub callback: std::sync::Arc<dyn Fn(&IUIAutomationElement) + Send + Sync>,
    }

    impl windows::Win32::UI::Accessibility::IUIAutomationFocusChangedEventHandler_Impl
        for Focus_Impl
    {
        fn HandleFocusChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
        ) -> windows_core::Result<()> {
            if let Some(element) = sender.as_ref() {
                (self.callback)(element);
            }
            Ok(())
        }
    }

    #[windows_core::implement(
        windows::Win32::UI::Accessibility::IUIAutomationPropertyChangedEventHandler
    )]
    pub struct Props {
        pub tx: Mutex<Sender<(i32, Option<String>, Instant)>>,
    }

    impl windows::Win32::UI::Accessibility::IUIAutomationPropertyChangedEventHandler_Impl
        for Props_Impl
    {
        fn HandlePropertyChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            propertyid: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
            _newvalue: &windows::Win32::System::Variant::VARIANT,
        ) -> windows_core::Result<()> {
            if let Some(element) = sender.as_ref() {
                let _ = self
                    .tx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .send((propertyid.0, element.cached_string(UIA_NamePropertyId), Instant::now()));
            }
            Ok(())
        }
    }

    #[windows_core::implement(windows::Win32::UI::Accessibility::IUIAutomationEventHandler)]
    pub struct Events {
        pub tx: Mutex<Sender<(i32, Option<String>, Instant)>>,
    }

    impl IUIAutomationEventHandler_Impl for Events_Impl {
        fn HandleAutomationEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            _eventid: UIA_EVENT_ID,
        ) -> windows_core::Result<()> {
            if let Some(element) = sender.as_ref() {
                let pid = element.cached_i32(UIA_ProcessIdPropertyId).unwrap_or(0);
                let name = element.cached_string(UIA_NamePropertyId);
                let _ = self
                    .tx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .send((pid, name, Instant::now()));
            }
            Ok(())
        }
    }
}

fn drain(rx: &Receiver<(i32, Option<String>, Instant)>, wait: Duration) -> Vec<(i32, Option<String>)> {
    let mut seen = Vec::new();
    while let Ok((pid, name, _)) = rx.recv_timeout(wait) {
        seen.push((pid, name));
    }
    seen
}

/// E-concurrency: one thread's read of A is held; does another thread's
/// read of B complete at once?
fn e_concurrency() {
    common::init_com();
    let (mut a, a_hwnd) = start("exp-conc-a");
    let (_b, b_hwnd) = start("exp-conc-b");
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel::<(u128, String)>();
    let reader = std::thread::spawn(move || {
        let uia = Uia::new().expect("uia");
        let cache = uia.base_cache_request().expect("cache");
        let root = uia.element_from_handle(a_hwnd, &cache).expect("A root");
        ready_tx.send(()).expect("ready");
        go_rx.recv().expect("go");
        let started = Instant::now();
        let name = current_name(&root);
        let _ = done_tx.send((ms(started), format!("{name:?}")));
    });
    ready_rx.recv().expect("reader ready");
    a.hold();
    go_tx.send(()).expect("go");
    let method = held(&a, LONG).expect("A's read is held");
    println!("RESULT e_concurrency: A held in {method}");

    // Separate client on this thread.
    let uia = Uia::new().expect("uia");
    let cache = uia.base_cache_request().expect("cache");
    let started = Instant::now();
    let b_root = uia.element_from_handle(b_hwnd, &cache).expect("B root");
    let connect = ms(started);
    let started = Instant::now();
    let name = current_name(&b_root);
    println!(
        "RESULT e_concurrency: separate client: B ElementFromHandle {connect} ms, CurrentName {} ms, {name:?}",
        ms(started)
    );
    // A third thread reading B while A is still held.
    let other = std::thread::spawn(move || {
        let uia = Uia::new().expect("uia");
        let cache = uia.base_cache_request().expect("cache");
        let started = Instant::now();
        let root = uia.element_from_handle(b_hwnd, &cache).expect("B root");
        let name = current_name(&root);
        (ms(started), format!("{name:?}"))
    });
    let (took, name) = other.join().expect("join");
    println!("RESULT e_concurrency: third thread, new client: B read in {took} ms, {name}");
    assert!(done_rx.try_recv().is_err(), "A's read is still held");
    a.release();
    let (took, name) = done_rx.recv_timeout(LONG).expect("A's read returns");
    println!("RESULT e_concurrency: A's held read returned after {took} ms with {name}");
    reader.join().expect("join");
}

/// E-delivery: with A holding the provider call behind its focus event,
/// does B's focus event reach a desktop-wide handler in the same process,
/// and does a property handler on B's window still get B's events?
fn e_delivery() {
    common::init_com();
    let (mut a, a_hwnd) = start("exp-deliv-a");
    let (mut b, b_hwnd) = start("exp-deliv-b");
    let (_focus_registration, focus) = scoped_focus(vec![a_hwnd, b_hwnd]);
    let (prop_tx, props) = mpsc::channel();
    let prop_tx = Mutex::new(prop_tx);
    let _props = Registration::new(
        vec![Subscription::Properties {
            properties: vec![UIA_NamePropertyId],
            callback: Arc::new(move |element, _| {
                let _ = prop_tx
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send((element.cached_string(UIA_NamePropertyId), Instant::now()));
            }),
        }],
        Scope::Windows(vec![b_hwnd]),
    )
    .expect("property registration");

    // Baseline: both deliver.
    send_seeing(&mut b, "focus btn1");
    let base = drain(&focus, Duration::from_millis(1500));
    println!("RESULT e_delivery: baseline focus events {base:?}");

    a.hold();
    let a_pid = i32::try_from(a.pid()).unwrap_or(0);
    let raised = Instant::now();
    a.write("focus item1");
    // Either the raise or UIA's fill is held; whichever, A is now stuck.
    let first = a.line(LONG);
    let second = if first.as_deref() == Some("applied") {
        held(&a, Duration::from_secs(5))
    } else {
        None
    };
    println!(
        "RESULT e_delivery: A after focus: {first:?} then {second:?}, {} ms after raising",
        ms(raised)
    );
    let started = Instant::now();
    send_seeing(&mut b, "focus item2");
    let mut b_focus = None;
    let mut a_focus = None;
    while let Ok((pid, name, at)) = focus.recv_timeout(Duration::from_secs(25)) {
        if pid == a_pid {
            a_focus = Some((name, at.duration_since(started).as_millis()));
        } else {
            b_focus = Some((name, at.duration_since(started).as_millis()));
            break;
        }
    }
    println!("RESULT e_delivery: with A held, B's focus event {b_focus:?} (ms after B raised); A's {a_focus:?}");
    let started = Instant::now();
    send_seeing(&mut b, "set-name btn1 Renamed");
    let prop = props.recv_timeout(Duration::from_secs(25)).ok();
    println!(
        "RESULT e_delivery: with A held, B's name change {:?} after {} ms",
        prop.as_ref().map(|(name, _)| name.clone()),
        prop.map_or(u128::MAX, |(_, at)| at.duration_since(started).as_millis())
    );
    a.release();
    let late = drain(&focus, Duration::from_secs(3));
    println!("RESULT e_delivery: after release {late:?}");
}

/// E-scope: a focus-changed event registered on A's window subtree, through
/// `AddAutomationEventHandler` and through an event handler group.
fn e_scope() {
    common::init_com();
    let (mut a, a_hwnd) = start("exp-scope-a");
    let (mut b, _b_hwnd) = start("exp-scope-b");
    let uia = Uia::new().expect("uia");
    let cache = minimal_cache(&uia);
    let root = uia.element_from_handle(a_hwnd, &cache).expect("A root");

    let (raw_tx, raw) = mpsc::channel();
    let raw_handler: IUIAutomationEventHandler = handler::Events {
        tx: Mutex::new(raw_tx),
    }
    .into();
    let started = Instant::now();
    // SAFETY: live client, element, cache and handler.
    let added = unsafe {
        uia.client().AddAutomationEventHandler(
            UIA_AutomationFocusChangedEventId,
            &root,
            TreeScope_Subtree,
            &cache,
            &raw_handler,
        )
    };
    println!(
        "RESULT e_scope: AddAutomationEventHandler(focus, A subtree) = {added:?} in {} ms",
        ms(started)
    );

    let (group_tx, group) = mpsc::channel();
    let group_tx = Mutex::new(group_tx);
    let started = Instant::now();
    let _group = Registration::with_cache(
        vec![Subscription::Event {
            event: UIA_AutomationFocusChangedEventId,
            callback: Arc::new(move |element| {
                let _ = group_tx.lock().unwrap_or_else(PoisonError::into_inner).send((
                    element.cached_i32(UIA_ProcessIdPropertyId).unwrap_or(0),
                    element.cached_string(UIA_NamePropertyId),
                    Instant::now(),
                ));
            }),
        }],
        Scope::Windows(vec![a_hwnd]),
        &[UIA_ProcessIdPropertyId, UIA_NamePropertyId],
    )
    .expect("group registration");
    println!("RESULT e_scope: group registration on A's window took {} ms (with its thread)", ms(started));

    let (root_tx, root_group) = mpsc::channel();
    let root_tx = Mutex::new(root_tx);
    let _root_group = Registration::with_cache(
        vec![Subscription::Event {
            event: UIA_AutomationFocusChangedEventId,
            callback: Arc::new(move |element| {
                let _ = root_tx.lock().unwrap_or_else(PoisonError::into_inner).send((
                    element.cached_i32(UIA_ProcessIdPropertyId).unwrap_or(0),
                    element.cached_string(UIA_NamePropertyId),
                    Instant::now(),
                ));
            }),
        }],
        Scope::Desktop,
        &[UIA_ProcessIdPropertyId, UIA_NamePropertyId],
    )
    .expect("root group registration");
    let reference_cache = minimal_cache(&uia);
    let (reference_handler, reference) = focus_registration(&uia, &reference_cache);
    send_seeing(&mut a, "focus btn1");
    send_seeing(&mut b, "focus btn1");
    send_seeing(&mut a, "focus item1");
    let reference_seen = drain(&reference, Duration::from_millis(1500));
    println!("RESULT e_scope: desktop-wide reference saw {reference_seen:?}");
    println!(
        "RESULT e_scope: group on the desktop root saw {:?}",
        drain(&root_group, Duration::from_millis(300))
    );
    println!(
        "RESULT e_scope: A pid {} B pid {}; raw scoped saw {:?}; group scoped saw {:?}",
        a.pid(),
        b.pid(),
        drain(&raw, Duration::from_millis(300)),
        drain(&group, Duration::from_millis(300))
    );
    // SAFETY: removing this thread's handlers.
    unsafe {
        let _ = uia.client().RemoveFocusChangedEventHandler(&reference_handler);
        let _ = uia.client().RemoveAllEventHandlers();
    }
}

/// E-cost: provider calls for one focus event with 0, 1 and 3 extra
/// desktop-wide minimal-cache focus registrations, and their timing.
fn e_cost() {
    common::init_com();
    let (mut a, a_hwnd) = start("exp-cost");
    let hwnd = windows::Win32::Foundation::HWND(a_hwnd as *mut _);
    let mut clients: Vec<(Registration, Receiver<(i32, Option<String>, Instant)>)> = Vec::new();
    let targets = ["btn1", "item1", "item2", "slider1"];
    let mut turn = 0;
    for count in [0usize, 1, 3] {
        while clients.len() < count {
            let started = Instant::now();
            let (registration, rx) = scoped_focus(vec![a_hwnd]);
            println!("RESULT e_cost: a scoped focus registration took {} ms", ms(started));
            clients.push((registration, rx));
        }
        common::reset_hits(hwnd);
        let target = targets[turn % targets.len()];
        turn += 1;
        let raised = Instant::now();
        send_seeing(&mut a, &format!("focus {target}"));
        let mut delays = Vec::new();
        for (_, rx) in &clients {
            let (_, _, at) = rx.recv_timeout(LONG).expect("the callback");
            delays.push(at.duration_since(raised).as_micros());
        }
        // Each callback has run; the fills are done.
        println!(
            "RESULT e_cost: {count} registrations: callbacks after {delays:?} us; provider calls {:?}",
            common::read_hits(hwnd)
        );
    }
    // A scoped group on the window, for the race's width.
    let started = Instant::now();
    let _scoped = Registration::new(
        vec![Subscription::Event {
            event: windows::Win32::UI::Accessibility::UIA_MenuOpenedEventId,
            callback: Arc::new(|_| {}),
        }],
        Scope::Windows(vec![a_hwnd]),
    )
    .expect("scoped");
    println!("RESULT e_cost: a scoped group registration on a window took {} ms", ms(started));
}

/// E1: whether the connection and transaction timeouts are per client or
/// per process.
fn e1_timeout_scope() {
    common::init_com();
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let (read_tx, read_rx) = mpsc::channel::<Sender<(u32, u32)>>();
    let other = std::thread::spawn(move || {
        let uia = Uia::new().expect("uia");
        let client: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
        ready_tx.send(()).expect("ready");
        while let Ok(reply) = read_rx.recv() {
            // SAFETY: plain reads.
            let values = unsafe { (client.ConnectionTimeout().unwrap_or(0), client.TransactionTimeout().unwrap_or(0)) };
            let _ = reply.send(values);
        }
    });
    ready_rx.recv().expect("ready");
    let read = || {
        let (tx, rx) = mpsc::channel();
        read_tx.send(tx).expect("ask");
        rx.recv().expect("answer")
    };
    println!("RESULT e1: other client before: {:?}", read());
    let uia = Uia::new().expect("uia");
    println!("RESULT e1: other client after this thread's Uia::new: {:?}", read());
    let client: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
    // SAFETY: plain sets.
    unsafe {
        client.SetConnectionTimeout(1000).expect("set");
        client.SetTransactionTimeout(1000).expect("set");
    }
    // SAFETY: plain reads.
    let own = unsafe { (client.ConnectionTimeout().unwrap_or(0), client.TransactionTimeout().unwrap_or(0)) };
    println!("RESULT e1: this client set to 1000/1000 reads {own:?}; other client reads {:?}", read());
    drop(read_tx);
    other.join().expect("join");
}

/// What `GetFocusedElement` returns on a test's own desktop, for E2, E3
/// and E5.
fn focused_description(uia: &Uia) -> String {
    let cache = uia.base_cache_request().expect("cache");
    match uia.focused_element(&cache) {
        Ok(element) => format!(
            "pid {:?} name {:?}",
            element.cached_i32(UIA_ProcessIdPropertyId),
            element.cached_string(UIA_NamePropertyId)
        ),
        Err(error) => format!("error {error}"),
    }
}

/// E2: a stalled window against `within(1 s)`, for the focused element and
/// for `ElementFromHandle`.
fn e2_stall_within() {
    common::init_com();
    let (mut a, a_hwnd) = start("exp-e2");
    let uia = Uia::new().expect("uia");
    println!("RESULT e2: focused element on this desktop: {}", focused_description(&uia));
    let stall = Duration::from_millis(4000);
    a.stall(stall);
    let cache = uia.base_cache_request().expect("cache");
    let started = Instant::now();
    let answer = uia
        .within(Duration::from_secs(1), |uia| uia.focused_element(&cache))
        .expect("within");
    let took = ms(started);
    let ended = a.stall_ended(stall);
    let returned = common::now_us();
    println!(
        "RESULT e2: within(1 s, focused_element) took {took} ms: {:?} (returned {} us vs stall end)",
        answer.as_ref().map(|element| element.cached_string(UIA_NamePropertyId)),
        i128::from(returned) - i128::from(ended)
    );
    a.stall(stall);
    let fresh = Uia::new().expect("uia");
    let started = Instant::now();
    let answer = fresh
        .within(Duration::from_secs(1), |uia| uia.element_from_handle(a_hwnd, &cache))
        .expect("within");
    let took = ms(started);
    let _ = a.stall_ended(stall);
    println!(
        "RESULT e2: fresh client within(1 s, ElementFromHandle) took {took} ms: {:?}",
        answer.as_ref().map(|element| element.cached_string(UIA_NamePropertyId))
    );
}

/// Runs `read` on a thread and returns how long it took and what it said,
/// or `None` if it had not returned within `limit`.
fn timed<T: Send + 'static>(
    limit: Duration,
    read: impl FnOnce() -> T + Send + 'static,
) -> (Receiver<(u128, T)>, Instant) {
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        common::init_com();
        let begun = Instant::now();
        let value = read();
        let _ = tx.send((ms(begun), value));
    });
    let _ = limit;
    (rx, started)
}

/// E3: a held provider and `within(1 s)`, never released: with the default
/// transaction timeout, then with 1 second.
/// A call on an element fetched before the hold, so the held call is a
/// transaction, not the connection: the thread fetches `a_hwnd`'s root,
/// sets the transaction timeout when given, says it is ready, waits to be
/// told to go, and then runs `call` inside `within(1 s)`.
fn held_transaction(
    app: &mut common::MockApp,
    a_hwnd: isize,
    transaction_ms: Option<u32>,
    call: fn(&Uia, &IUIAutomationElement) -> String,
) -> (Option<String>, Result<(u128, String), mpsc::RecvTimeoutError>, u128) {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        common::init_com();
        let uia = Uia::new().expect("uia");
        if let Some(timeout) = transaction_ms {
            let client: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
            // SAFETY: a plain set.
            unsafe { client.SetTransactionTimeout(timeout) }.expect("set");
        }
        let cache = uia.cache_request(&[UIA_ProcessIdPropertyId]).expect("cache");
        let root = uia.element_from_handle(a_hwnd, &cache).expect("root");
        ready_tx.send(()).expect("ready");
        go_rx.recv().expect("go");
        let begun = Instant::now();
        let answer = uia
            .within(Duration::from_secs(1), |uia| call(uia, &root))
            .expect("within");
        let _ = done_tx.send((ms(begun), answer));
    });
    ready_rx.recv().expect("ready");
    app.hold();
    go_tx.send(()).expect("go");
    let method = held(app, Duration::from_secs(10));
    let started = Instant::now();
    let answer = done_rx.recv_timeout(LONG);
    (method, answer, ms(started))
}

fn e3_hold_never_released() {
    common::init_com();
    for transaction_ms in [None, Some(1000u32)] {
        let (mut a, a_hwnd) = start("exp-e3");
        let (method, answer, after) = held_transaction(&mut a, a_hwnd, transaction_ms, |_, root| {
            format!("{:?}", root.has_keyboard_focus())
        });
        println!(
            "RESULT e3: transaction timeout {transaction_ms:?}: held {method:?}; returned {answer:?} ({after} ms after held)"
        );
        a.kill();
    }
}

#[allow(dead_code)]
fn e3_old() {
    common::init_com();
    for transaction_ms in [None, Some(1000u32)] {
        let (mut a, a_hwnd) = start("exp-e3");
        let (rx, _) = {
            a.hold();
            timed(LONG, move || {
                let uia = Uia::new().expect("uia");
                if let Some(timeout) = transaction_ms {
                    let client: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
                    // SAFETY: a plain set.
                    unsafe { client.SetTransactionTimeout(timeout) }.expect("set");
                }
                let cache = uia.base_cache_request().expect("cache");
                let focused = uia.within(Duration::from_secs(1), |uia| {
                    uia.element_from_handle(a_hwnd, &cache).and_then(|root| {
                        root.has_keyboard_focus()
                    })
                });
                format!("{focused:?}")
            })
        };
        let method = held(&a, LONG);
        let started = Instant::now();
        let answer = rx.recv_timeout(LONG);
        println!(
            "RESULT e3: transaction timeout {transaction_ms:?}: held {method:?}; returned {:?} ({} ms after held)",
            answer,
            ms(started)
        );
        a.kill();
    }
}

/// E4: a held call, then mockapp killed: when does the call return?
fn e4_hold_then_kill() {
    common::init_com();
    for (connection_ms, recovery) in [(1000u32, true), (10_000, true), (1000, false), (10_000, false)] {
        let (mut a, a_hwnd) = start("exp-e4");
        a.hold();
        let (rx, _) = timed(LONG, move || {
            let uia = Uia::new().expect("uia");
            let client2: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
            let client6: IUIAutomation6 = uia.client().cast().expect("IUIAutomation6");
            // SAFETY: plain sets.
            unsafe {
                client2.SetConnectionTimeout(connection_ms).expect("set");
                client6
                    .SetConnectionRecoveryBehavior(if recovery {
                        ConnectionRecoveryBehaviorOptions_Enabled
                    } else {
                        ConnectionRecoveryBehaviorOptions_Disabled
                    })
                    .expect("set");
            }
            let cache = uia.base_cache_request().expect("cache");
            let answer = uia
                .element_from_handle(a_hwnd, &cache)
                .and_then(|root| current_name(&root));
            format!("{answer:?}")
        });
        let method = held(&a, LONG);
        a.kill();
        let killed = Instant::now();
        let answer = rx.recv_timeout(LONG);
        println!(
            "RESULT e4: connection {connection_ms} ms, recovery {recovery}: held {method:?}; returned {answer:?}, {} ms after the kill",
            ms(killed)
        );
    }
}

/// E5: an outpost for A is given A's UIA focus while its read of the
/// focused element lands in B, which is held. The read is injected to read
/// B, since nothing on a test's own desktop has the system focus.
fn e5_focus_read_in_held_application() {
    common::init_com();
    let (mut a, a_hwnd) = start("exp-e5-a");
    let (mut b, b_hwnd) = start("exp-e5-b");
    let uia = Uia::new().expect("uia");
    send_seeing(&mut a, "focus btn1");
    let element = find(&uia, a_hwnd, "Original Name");
    let ListenerFact { fact, .. } = uia_focus_fact(&element).expect("fact");

    let (pipe_in, pipe_out) = std::io::pipe().expect("pipe");
    let reader: verbatim_outpost::FocusedElementReader =
        Arc::new(move |uia: &Uia, cache| uia.element_from_handle(b_hwnd, cache));
    let outpost = Outpost::with_focused_element_reader(
        Box::new(pipe_out),
        a.pid(),
        verbatim_outpost::OutpostOptions::default(),
        reader,
    );
    outpost.set_foreground_reader(Arc::new(move || a_hwnd));
    let (tx, messages) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(pipe_in);
        while let Ok(Some(message)) = read_message::<_, OutpostToSupervisor>(&mut reader) {
            if tx.send(message).is_err() {
                return;
            }
        }
    });
    let _ready = messages.recv_timeout(LONG).expect("ready");
    b.hold();
    let started = Instant::now();
    outpost.handle_command(&SupervisorToOutpost::DeliverFact {
        trace_id: verbatim_model::TraceId::mint(),
        observed_at_ms: 0,
        timing: verbatim_outpost::protocol::EventTiming::default(),
        fact,
    });
    let method = held(&b, LONG);
    println!("RESULT e5: B held {method:?} {} ms after the fact", ms(started));
    let said = messages.recv_timeout(LONG);
    println!(
        "RESULT e5: the outpost said {:?} after {} ms",
        said.map(|message| format!("{message:?}").chars().take(160).collect::<String>()),
        ms(started)
    );
    b.release();
}

/// E-TT: with a 1-second transaction timeout, a held provider's
/// `GetFocusedElement`, `CurrentName`, `CurrentHasKeyboardFocus` and an
/// event handler group's registration.
fn e_tt() {
    common::init_com();
    let (mut a, a_hwnd) = start("exp-tt");
    let uia = Uia::new().expect("uia");
    let client: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
    // SAFETY: a plain set; process-wide or not, E1 says.
    unsafe { client.SetTransactionTimeout(1000) }.expect("set");
    println!("RESULT e_tt: focused element here: {}", focused_description(&uia));
    let calls: [(&str, Box<dyn FnOnce() -> String + Send>); 4] = [
        (
            "GetFocusedElement",
            Box::new(|| {
                let uia = Uia::new().expect("uia");
                let cache = uia.base_cache_request().expect("cache");
                format!("{:?}", uia.focused_element(&cache).map(|_| ()))
            }),
        ),
        (
            "CurrentName",
            Box::new(move || {
                let uia = Uia::new().expect("uia");
                let cache = uia.cache_request(&[UIA_ProcessIdPropertyId]).expect("cache");
                let root = uia.element_from_handle(a_hwnd, &cache);
                format!("{:?}", root.and_then(|root| current_name(&root)))
            }),
        ),
        (
            "CurrentHasKeyboardFocus",
            Box::new(move || {
                let uia = Uia::new().expect("uia");
                let cache = uia.cache_request(&[UIA_ProcessIdPropertyId]).expect("cache");
                let root = uia.element_from_handle(a_hwnd, &cache);
                format!("{:?}", root.and_then(|root| root.has_keyboard_focus()))
            }),
        ),
        (
            "AddEventHandlerGroup",
            Box::new(move || {
                let registration = Registration::new(
                    vec![Subscription::Event {
                        event: windows::Win32::UI::Accessibility::UIA_MenuOpenedEventId,
                        callback: Arc::new(|_| {}),
                    }],
                    Scope::Windows(vec![a_hwnd]),
                );
                format!("{:?}", registration.map(|_| ()))
            }),
        ),
    ];
    for (label, call) in calls {
        a.hold();
        let (rx, _) = timed(LONG, call);
        let method = held(&a, Duration::from_secs(10));
        let started = Instant::now();
        let answer = rx.recv_timeout(LONG);
        println!(
            "RESULT e_tt: {label}: held {method:?}; returned {answer:?} {} ms after held",
            ms(started)
        );
        a.release();
        // Whatever UIA queued behind the held call is answered now.
        let _ = a.line(Duration::from_millis(500));
    }
}

/// E-TT on transactions: each call on an element fetched before the hold,
/// with a 1-second transaction timeout.
fn e_tt_transactions() {
    common::init_com();
    let calls: [(&str, fn(&Uia, &IUIAutomationElement) -> String); 4] = [
        ("CurrentName", |_, root| format!("{:?}", current_name(root))),
        ("CurrentHasKeyboardFocus", |_, root| {
            format!("{:?}", root.has_keyboard_focus())
        }),
        ("FindFirst", |uia, root| {
            let value = VARIANT::from(BSTR::from("First"));
            let condition = uia
                .property_condition(UIA_NamePropertyId, &value)
                .expect("condition");
            let cache = uia.cache_request(&[UIA_NamePropertyId]).expect("cache");
            format!(
                "{:?}",
                root.find_first_build_cache(TreeScope_Descendants, &condition, &cache)
                    .map(|found| found.is_some())
            )
        }),
        ("AddEventHandlerGroup", |uia, root| {
            let client: IUIAutomation6 = uia.client().cast().expect("IUIAutomation6");
            let cache = uia.cache_request(&[UIA_NamePropertyId]).expect("cache");
            let (tx, _rx) = mpsc::channel();
            let events: IUIAutomationEventHandler = handler::Events { tx: Mutex::new(tx) }.into();
            // SAFETY: live client, element, cache and handler.
            let added = unsafe {
                client.CreateEventHandlerGroup().and_then(|group| {
                    group.AddAutomationEventHandler(
                        windows::Win32::UI::Accessibility::UIA_MenuOpenedEventId,
                        TreeScope_Subtree,
                        &cache,
                        &events,
                    )?;
                    client.AddEventHandlerGroup(root, &group)
                })
            };
            format!("{added:?}")
        }),
    ];
    for (label, call) in calls {
        let (mut a, a_hwnd) = start("exp-tt2");
        let (method, answer, after) = held_transaction(&mut a, a_hwnd, Some(1000), call);
        println!("RESULT e_tt2: {label}: held {method:?}; returned {answer:?} {after} ms after held");
        a.kill();
    }
}

/// E4 on a transaction: held, then mockapp killed.
fn e4_transaction_then_kill() {
    common::init_com();
    let (mut a, a_hwnd) = start("exp-e4b");
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        common::init_com();
        let uia = Uia::new().expect("uia");
        let cache = uia.cache_request(&[UIA_ProcessIdPropertyId]).expect("cache");
        let root = uia.element_from_handle(a_hwnd, &cache).expect("root");
        ready_tx.send(()).expect("ready");
        go_rx.recv().expect("go");
        let answer = current_name(&root);
        let _ = done_tx.send((Instant::now(), format!("{answer:?}")));
    });
    ready_rx.recv().expect("ready");
    a.hold();
    go_tx.send(()).expect("go");
    let method = held(&a, Duration::from_secs(10));
    a.kill();
    let killed = Instant::now();
    let answer = done_rx.recv_timeout(LONG);
    println!(
        "RESULT e4b: held {method:?}; killed; returned {:?} {} ms after the kill",
        answer.as_ref().map(|(_, text)| text.clone()),
        answer.as_ref().map_or(u128::MAX, |(at, _)| at.duration_since(killed).as_millis())
    );
}

mod mapping {
    //! The proxy factory mapping: what each entry maps, clearing focus's
    //! mapping, and how far a change reaches.

    use windows::Win32::System::Com::SAFEARRAY;
    use windows::Win32::System::Ole::{SafeArrayDestroy, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound};
    use windows::Win32::UI::Accessibility::{
        IUIAutomation, UIA_AsyncContentLoadedEventId, UIA_AutomationFocusChangedEventId,
        UIA_AutomationPropertyChangedEventId, UIA_EVENT_ID, UIA_MenuClosedEventId,
        UIA_MenuModeEndEventId, UIA_MenuModeStartEventId, UIA_MenuOpenedEventId,
        UIA_NamePropertyId, UIA_PROPERTY_ID, UIA_SelectionItem_ElementAddedToSelectionEventId,
        UIA_SelectionItem_ElementSelectedEventId, UIA_StructureChangedEventId,
        UIA_SystemAlertEventId, UIA_ToolTipClosedEventId, UIA_ToolTipOpenedEventId,
        UIA_Window_WindowClosedEventId, UIA_Window_WindowOpenedEventId,
    };

    pub const FOCUS: (UIA_EVENT_ID, UIA_PROPERTY_ID) =
        (UIA_AutomationFocusChangedEventId, UIA_PROPERTY_ID(0));

    /// The events whose mappings are dumped.
    pub fn events() -> Vec<(&'static str, UIA_EVENT_ID, UIA_PROPERTY_ID)> {
        let none = UIA_PROPERTY_ID(0);
        vec![
            ("focus", UIA_AutomationFocusChangedEventId, none),
            ("menu opened", UIA_MenuOpenedEventId, none),
            ("menu closed", UIA_MenuClosedEventId, none),
            ("menu mode start", UIA_MenuModeStartEventId, none),
            ("menu mode end", UIA_MenuModeEndEventId, none),
            ("tooltip opened", UIA_ToolTipOpenedEventId, none),
            ("tooltip closed", UIA_ToolTipClosedEventId, none),
            ("window opened", UIA_Window_WindowOpenedEventId, none),
            ("window closed", UIA_Window_WindowClosedEventId, none),
            ("element selected", UIA_SelectionItem_ElementSelectedEventId, none),
            ("added to selection", UIA_SelectionItem_ElementAddedToSelectionEventId, none),
            ("structure changed", UIA_StructureChangedEventId, none),
            ("async content loaded", UIA_AsyncContentLoadedEventId, none),
            ("system alert", UIA_SystemAlertEventId, none),
            ("name property", UIA_AutomationPropertyChangedEventId, UIA_NamePropertyId),
        ]
    }

    /// Reads and destroys an array of i32.
    fn take(array: *mut SAFEARRAY) -> Vec<i32> {
        if array.is_null() {
            return Vec::new();
        }
        let mut values = Vec::new();
        // SAFETY: a one-dimensional array UIA returned to this caller.
        let bounds = unsafe { (SafeArrayGetLBound(array, 1), SafeArrayGetUBound(array, 1)) };
        if let (Ok(low), Ok(high)) = bounds {
            for index in low..=high {
                let mut value = 0i32;
                // SAFETY: an index within the bounds.
                if unsafe { SafeArrayGetElement(array, &raw const index, (&raw mut value).cast()) }.is_ok() {
                    values.push(value);
                }
            }
        }
        // SAFETY: owned here.
        let _ = unsafe { SafeArrayDestroy(array) };
        values
    }

    /// Every entry's class and the WinEvents it maps to each event, the
    /// non-empty ones.
    pub fn dump(client: &IUIAutomation) -> Vec<String> {
        // SAFETY: local calls on a live client.
        let mapping = unsafe { client.ProxyFactoryMapping() }.expect("mapping");
        // SAFETY: as above.
        let count = unsafe { mapping.Count() }.expect("count");
        let mut lines = Vec::new();
        for index in 0..count {
            // SAFETY: an index below the count.
            let entry = unsafe { mapping.GetEntry(index) }.expect("entry");
            // SAFETY: local reads.
            let class = unsafe { entry.ClassName() }.map(|name| name.to_string()).unwrap_or_default();
            let mut mapped = Vec::new();
            for (label, event, property) in events() {
                // SAFETY: a local read.
                let winevents = unsafe { entry.GetWinEventsForAutomationEvent(event, property) }
                    .map(take)
                    .unwrap_or_default();
                if !winevents.is_empty() {
                    mapped.push(format!("{label}: {winevents:x?}"));
                }
            }
            if !mapped.is_empty() {
                lines.push(format!("entry {index} class {class:?}: {}", mapped.join("; ")));
            }
        }
        lines
    }

    /// Whether any entry maps a WinEvent to focus changes.
    pub fn maps_focus(client: &IUIAutomation) -> bool {
        // SAFETY: local calls on a live client.
        let mapping = unsafe { client.ProxyFactoryMapping() }.expect("mapping");
        // SAFETY: as above.
        let count = unsafe { mapping.Count() }.expect("count");
        (0..count).any(|index| {
            // SAFETY: an index below the count.
            let entry = unsafe { mapping.GetEntry(index) }.expect("entry");
            // SAFETY: a local read.
            !unsafe { entry.GetWinEventsForAutomationEvent(FOCUS.0, FOCUS.1) }
                .map(take)
                .unwrap_or_default()
                .is_empty()
        })
    }

    /// Clears focus's mapping in every entry, as `verbatim_uia::proxy`
    /// clears the other events'.
    pub fn clear_focus(client: &IUIAutomation) {
        // SAFETY: local calls on a live client.
        let mapping = unsafe { client.ProxyFactoryMapping() }.expect("mapping");
        // SAFETY: as above.
        let count = unsafe { mapping.Count() }.expect("count");
        for index in 0..count {
            // SAFETY: an index below the count.
            let entry = unsafe { mapping.GetEntry(index) }.expect("entry");
            // SAFETY: a local read.
            let mapped = unsafe { entry.GetWinEventsForAutomationEvent(FOCUS.0, FOCUS.1) }
                .map(take)
                .unwrap_or_default();
            if mapped.is_empty() {
                continue;
            }
            // SAFETY: a null array maps no WinEvent.
            unsafe { entry.SetWinEventsForAutomationEvent(FOCUS.0, FOCUS.1, std::ptr::null()) }
                .expect("clear");
            // SAFETY: put back at its place, as changes need.
            unsafe { mapping.RemoveEntry(index) }.expect("remove");
            // SAFETY: as above.
            unsafe { mapping.InsertEntry(index, &entry) }.expect("insert");
        }
    }
}

/// E-mapping: what the proxy factory mapping maps, and how far clearing
/// focus's mapping reaches: another client on another thread, a client
/// created afterwards, and another process.
fn e_mapping() {
    common::init_com();
    let uia = Uia::new().expect("uia");
    let before = mapping::dump(uia.client());
    println!("RESULT e_mapping: a new client's mapping (after Verbatim's own clearing):");
    for line in &before {
        println!("RESULT e_mapping:   {line}");
    }
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let (ask_tx, ask_rx) = mpsc::channel::<Sender<bool>>();
    let other = std::thread::spawn(move || {
        common::init_com();
        let other = Uia::new().expect("uia");
        ready_tx.send(()).expect("ready");
        while let Ok(reply) = ask_rx.recv() {
            let _ = reply.send(mapping::maps_focus(other.client()));
        }
    });
    ready_rx.recv().expect("ready");
    let ask = || {
        let (tx, rx) = mpsc::channel();
        ask_tx.send(tx).expect("ask");
        rx.recv().expect("answer")
    };
    println!("RESULT e_mapping: before clearing: this client maps focus {}, other thread's client {}", mapping::maps_focus(uia.client()), ask());
    mapping::clear_focus(uia.client());
    let after = mapping::dump(uia.client());
    println!(
        "RESULT e_mapping: after clearing focus on this client: this client maps focus {}, other thread's client {}, a client created now {}",
        mapping::maps_focus(uia.client()),
        ask(),
        mapping::maps_focus(Uia::new().expect("uia").client())
    );
    println!("RESULT e_mapping: this client's mapping after clearing focus:");
    for line in &after {
        println!("RESULT e_mapping:   {line}");
    }
    let output = std::process::Command::new(std::env::current_exe().expect("exe"))
        .env("VERBATIM_EXP_MAPPING", "1")
        .output()
        .expect("another process");
    println!(
        "RESULT e_mapping: another process, started after the clearing: {}",
        String::from_utf8_lossy(&output.stdout).trim()
    );
    drop(ask_tx);
    other.join().expect("join");
}

/// E-hung-proxy (design section 12.11): does a hung Win32 window, which
/// UIA proxies in the client, hold up the client's UIA delivery of another
/// application's events?
fn e_hung_proxy() {
    use windows::Win32::UI::Accessibility::NotifyWinEvent;
    use windows::Win32::UI::WindowsAndMessaging::{
        CHILDID_SELF, EVENT_OBJECT_FOCUS, EVENT_OBJECT_NAMECHANGE, OBJID_CLIENT,
    };
    common::init_com();
    for (timeouts_ms, clear_focus) in [(None::<u32>, false), (None, true)] {
        let title = common::unique_title("exp-hung-p1");
        let mut p1 = common::spawn("tree_view.json", "msaa", &title);
        let host = common::find_window(&title);
        let tree = common::tree_view::tree_view(host);
        let (mut p2, p2_hwnd) = start("exp-hung-p2");
        let p1_pid = i32::try_from(p1.pid()).unwrap_or(0);

        let uia = Uia::new().expect("uia");
        if let Some(ms) = timeouts_ms {
            let client: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
            // SAFETY: plain sets, process-wide (E1).
            unsafe {
                client.SetTransactionTimeout(ms).expect("set");
                client.SetConnectionTimeout(ms).expect("set");
            }
        }
        if clear_focus {
            mapping::clear_focus(uia.client());
        }
        println!(
            "RESULT e_hung_proxy: focus mapping cleared {clear_focus}; this client maps focus {}",
            mapping::maps_focus(uia.client())
        );
        let cache = minimal_cache(&uia);
        let (desktop_handler, desktop_focus) = focus_registration(&uia, &cache);
        let (name_tx, names) = mpsc::channel();
        let name_tx = Mutex::new(name_tx);
        let _desktop_names = Registration::with_cache(
            vec![Subscription::Properties {
                properties: vec![UIA_NamePropertyId],
                callback: Arc::new(move |element, _| {
                    let _ = name_tx.lock().unwrap_or_else(PoisonError::into_inner).send((
                        element.cached_i32(UIA_ProcessIdPropertyId).unwrap_or(0),
                        element.cached_string(UIA_NamePropertyId),
                        Instant::now(),
                    ));
                }),
            }],
            Scope::Desktop,
            &[UIA_ProcessIdPropertyId, UIA_NamePropertyId],
        )
        .expect("desktop name registration");
        let (_p2_focus_registration, p2_focus) = scoped_focus(vec![p2_hwnd]);

        let raise = |event| {
            // SAFETY: a WinEvent for a live window, raised from this process.
            unsafe { NotifyWinEvent(event, windows::Win32::Foundation::HWND(tree.0), OBJID_CLIENT.0, CHILDID_SELF as i32) };
        };
        let label = format!("timeouts {timeouts_ms:?} focus mapping cleared {clear_focus}");

        // 1. Baseline, P1 answering.
        let raised = Instant::now();
        raise(EVENT_OBJECT_NAMECHANGE);
        raise(EVENT_OBJECT_FOCUS);
        let seen_focus = desktop_focus.recv_timeout(Duration::from_secs(3)).ok();
        let seen_name = names.recv_timeout(Duration::from_secs(1)).ok();
        println!(
            "RESULT e_hung_proxy {label}: baseline, tree view WinEvents: desktop focus {:?}, desktop name {:?}",
            seen_focus.map(|(pid, name, at)| (pid == p1_pid, name, at.duration_since(raised).as_millis())),
            seen_name.map(|(pid, name, at)| (pid == p1_pid, name, at.duration_since(raised).as_millis()))
        );
        while desktop_focus.try_recv().is_ok() {}
        while names.try_recv().is_ok() {}

        // Controls: P2's focus while P1 answers, and while P1 is hung but
        // no WinEvent names its window.
        let control = |p2: &mut common::MockApp, target: &str| {
            let raised = Instant::now();
            p2.send(&format!("focus {target}"));
            p2_focus
                .recv_timeout(Duration::from_secs(25))
                .ok()
                .map(|(_, name, at)| (name, at.duration_since(raised).as_millis()))
        };
        println!(
            "RESULT e_hung_proxy {label}: control, P1 answering: P2 focus {:?}",
            control(&mut p2, "item1")
        );
        let stall = Duration::from_secs(15);
        p1.stall(stall);
        let stalled = Instant::now();
        println!(
            "RESULT e_hung_proxy {label}: control, P1 hung, no WinEvent for it: P2 focus {:?}",
            control(&mut p2, "item2")
        );
        // 2. WinEvents for P1's tree view while it is hung.
        raise(EVENT_OBJECT_NAMECHANGE);
        raise(EVENT_OBJECT_FOCUS);
        // 3. P2's events while the proxy may be inside a call to P1.
        let _ = names.recv_timeout(Duration::from_millis(200));
        let p2_raised = Instant::now();
        p2.send("focus btn1");
        p2.send("set-name slider1 Renamed");
        let p2_focus_at = p2_focus.recv_timeout(Duration::from_secs(25)).ok();
        let mut p2_name_at = None;
        while let Ok((pid, name, at)) = names.recv_timeout(Duration::from_secs(25)) {
            if pid != p1_pid {
                p2_name_at = Some((name, at.duration_since(p2_raised).as_millis()));
                break;
            }
        }
        // 4. A read of P2 on another thread meanwhile.
        let read = std::thread::spawn(move || {
            common::init_com();
            let uia = Uia::new().expect("uia");
            let cache = uia.base_cache_request().expect("cache");
            let started = Instant::now();
            let name = uia
                .element_from_handle(p2_hwnd, &cache)
                .and_then(|root| current_name(&root));
            (started.elapsed().as_millis(), format!("{name:?}"))
        })
        .join()
        .expect("join");
        println!(
            "RESULT e_hung_proxy {label}: with P1 hung ({} ms into the stall): P2 focus {:?} ms after raised; P2 name change {:?}; other-thread read of P2 {:?}",
            stalled.elapsed().as_millis(),
            p2_focus_at.map(|(_, _, at)| at.duration_since(p2_raised).as_millis()),
            p2_name_at,
            read
        );
        let ended = p1.stall_ended(stall);
        let late_focus: Vec<_> = std::iter::from_fn(|| desktop_focus.recv_timeout(Duration::from_secs(2)).ok())
            .map(|(pid, name, _)| (pid == p1_pid, name))
            .collect();
        println!(
            "RESULT e_hung_proxy {label}: after the stall ended (at {ended} us): desktop focus events {late_focus:?}"
        );
        // SAFETY: removing this thread's handler.
        let _ = unsafe { uia.client().RemoveFocusChangedEventHandler(&desktop_handler) };
        p1.kill();
        p2.kill();
    }
}

/// E-hung-cause: which registration and which WinEvent make a hung,
/// provider-less window hold up another application's events: each case
/// on a fresh pair of mockapps, P2's focus timed through a group scoped to
/// its window.
fn e_hung_cause() {
    use windows::Win32::UI::Accessibility::NotifyWinEvent;
    use windows::Win32::UI::WindowsAndMessaging::{
        CHILDID_SELF, EVENT_OBJECT_FOCUS, EVENT_OBJECT_NAMECHANGE, OBJID_CLIENT,
    };
    common::init_com();
    // (label, desktop focus handler, desktop name registration, raise focus, raise name)
    let cases = [
        ("desktop focus + desktop name, focus WinEvent only", true, true, true, false),
        ("desktop focus + desktop name, name WinEvent only", true, true, false, true),
        ("desktop name only, both WinEvents", false, true, true, true),
        ("desktop focus only, both WinEvents", true, false, true, true),
        ("desktop focus only, cleared focus mapping, focus WinEvent", true, false, true, false),
        ("no desktop registration, both WinEvents", false, false, true, true),
    ];
    for (label, desktop_focus, desktop_name, raise_focus, raise_name) in cases {
        let title = common::unique_title("exp-cause-p1");
        let mut p1 = common::spawn("tree_view.json", "msaa", &title);
        let host = common::find_window(&title);
        let tree = common::tree_view::tree_view(host);
        let (mut p2, p2_hwnd) = start("exp-cause-p2");
        let uia = Uia::new().expect("uia");
        if label.contains("cleared") {
            mapping::clear_focus(uia.client());
        }
        let cache = minimal_cache(&uia);
        let focus_handler = desktop_focus.then(|| focus_registration(&uia, &cache));
        let _names = desktop_name.then(|| {
            Registration::with_cache(
                vec![Subscription::Properties {
                    properties: vec![UIA_NamePropertyId],
                    callback: Arc::new(|_, _| {}),
                }],
                Scope::Desktop,
                &[UIA_ProcessIdPropertyId],
            )
            .expect("desktop name registration")
        });
        let (_p2_focus_registration, p2_focus) = scoped_focus(vec![p2_hwnd]);
        let stall = Duration::from_secs(8);
        p1.stall(stall);
        if raise_name {
            // SAFETY: a WinEvent for a live window, raised from this process.
            unsafe { NotifyWinEvent(EVENT_OBJECT_NAMECHANGE, windows::Win32::Foundation::HWND(tree.0), OBJID_CLIENT.0, CHILDID_SELF as i32) };
        }
        if raise_focus {
            // SAFETY: as above.
            unsafe { NotifyWinEvent(EVENT_OBJECT_FOCUS, windows::Win32::Foundation::HWND(tree.0), OBJID_CLIENT.0, CHILDID_SELF as i32) };
        }
        let _ = p2_focus.recv_timeout(Duration::from_millis(200));
        let raised = Instant::now();
        p2.send("focus btn1");
        let arrived = p2_focus
            .recv_timeout(Duration::from_secs(20))
            .ok()
            .map(|(_, _, at)| at.duration_since(raised).as_millis());
        println!("RESULT e_hung_cause: {label}: P2 focus after {arrived:?} ms (P1 stalled {stall:?})");
        let _ = p1.stall_ended(stall);
        if let Some((handler, _)) = focus_handler {
            // SAFETY: removing this thread's handler.
            let _ = unsafe { uia.client().RemoveFocusChangedEventHandler(&handler) };
        }
        p1.kill();
        p2.kill();
    }
    // No focus registration anywhere in this process: P2 timed by a name
    // change through a property registration scoped to its window.
    for focus_registered in [false, true] {
        let title = common::unique_title("exp-cause-p1");
        let mut p1 = common::spawn("tree_view.json", "msaa", &title);
        let host = common::find_window(&title);
        let tree = common::tree_view::tree_view(host);
        let (mut p2, p2_hwnd) = start("exp-cause-p2");
        let uia = Uia::new().expect("uia");
        let cache = minimal_cache(&uia);
        let focus_handler = focus_registered.then(|| focus_registration(&uia, &cache));
        let (tx, names) = mpsc::channel();
        let tx = Mutex::new(tx);
        let _p2_names = Registration::with_cache(
            vec![Subscription::Properties {
                properties: vec![UIA_NamePropertyId],
                callback: Arc::new(move |_, _| {
                    let _ = tx.lock().unwrap_or_else(PoisonError::into_inner).send(Instant::now());
                }),
            }],
            Scope::Windows(vec![p2_hwnd]),
            &[UIA_ProcessIdPropertyId],
        )
        .expect("P2 name registration");
        let stall = Duration::from_secs(8);
        p1.stall(stall);
        // SAFETY: a WinEvent for a live window, raised from this process.
        unsafe { NotifyWinEvent(EVENT_OBJECT_FOCUS, windows::Win32::Foundation::HWND(tree.0), OBJID_CLIENT.0, CHILDID_SELF as i32) };
        let _ = names.recv_timeout(Duration::from_millis(200));
        let raised = Instant::now();
        p2.send("set-name btn1 Renamed");
        let arrived = names
            .recv_timeout(Duration::from_secs(20))
            .ok()
            .map(|at| at.duration_since(raised).as_millis());
        println!(
            "RESULT e_hung_cause: no focus registration but {}: focus WinEvent for the hung window; P2's name change after {arrived:?} ms",
            if focus_registered { "a desktop focus handler" } else { "none" }
        );
        let _ = p1.stall_ended(stall);
        if let Some((handler, _)) = focus_handler {
            // SAFETY: removing this thread's handler.
            let _ = unsafe { uia.client().RemoveFocusChangedEventHandler(&handler) };
        }
        p1.kill();
        p2.kill();
    }
}

mod cross {
    //! E-cross-process and E-bridge: whether a stalled listener stand-in
    //! holds up a separate process with no focus registration, whether its
    //! own WinEvent hooks keep delivering, and what MSAA sees of a UIA
    //! provider's focus.

    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};
    use std::time::{Duration, Instant};

    use verbatim_ia2::{WinEventHook, WinEventKind};
    use verbatim_uia::{ElementExt, Registration, Scope, Subscription, Uia};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        AccessibleObjectFromEvent, IAccessible, IUIAutomationElement,
        IUIAutomationFocusChangedEventHandler, NotifyWinEvent, SetWinEventHook, HWINEVENTHOOK,
        UIA_NamePropertyId, UIA_ProcessIdPropertyId, UIA_Text_TextChangedEventId,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CHILDID_SELF, DispatchMessageW, EVENT_OBJECT_FOCUS, EVENT_OBJECT_NAMECHANGE,
        EVENT_SYSTEM_FOREGROUND, GetMessageW, MSG, OBJID_CLIENT, WINEVENT_OUTOFCONTEXT,
    };

    fn now_us() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_micros())
    }

    static OUT: OnceLock<Mutex<()>> = OnceLock::new();

    fn say(line: &str) {
        let _guard = OUT
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        println!("L {} {line}", now_us());
        let _ = std::io::stdout().flush();
    }

    unsafe extern "system" fn listener_winevent(
        _hook: HWINEVENTHOOK,
        event: u32,
        hwnd: HWND,
        _object: i32,
        _child: i32,
        _thread: u32,
        _time: u32,
    ) {
        say(&format!("winevent {event:#x} hwnd={}", hwnd.0 as isize));
    }

    /// The listener stand-in: a desktop focus registration, a name
    /// registration on B's window (to see its own UIA stall), and
    /// out-of-context focus and foreground hooks for every process on a
    /// thread of their own. Reports on stdout until stdin closes.
    pub fn listener(b_hwnd: isize) -> ! {
        crate::common::init_com();
        std::thread::spawn(|| {
            // SAFETY: out-of-context hooks for every process, pumped below.
            let hooks = unsafe {
                [
                    SetWinEventHook(EVENT_OBJECT_FOCUS, EVENT_OBJECT_FOCUS, None, Some(listener_winevent), 0, 0, WINEVENT_OUTOFCONTEXT),
                    SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(listener_winevent), 0, 0, WINEVENT_OUTOFCONTEXT),
                ]
            };
            say(&format!("hooks {}", hooks.iter().all(|hook| !hook.is_invalid())));
            let mut message = MSG::default();
            // SAFETY: this thread's message loop.
            while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.as_bool() {
                // SAFETY: the message just taken.
                unsafe { DispatchMessageW(&raw const message) };
            }
        });
        let uia = Uia::new().expect("uia");
        let cache = uia.cache_request(&[UIA_ProcessIdPropertyId, UIA_NamePropertyId]).expect("cache");
        let handler: IUIAutomationFocusChangedEventHandler = super::handler::Focus {
            callback: Arc::new(|element: &IUIAutomationElement| {
                say(&format!("uia-focus {:?}", element.cached_string(UIA_NamePropertyId)));
            }),
        }
        .into();
        // SAFETY: live client, cache and handler.
        unsafe { uia.client().AddFocusChangedEventHandler(&cache, &handler) }.expect("focus");
        let _names = Registration::with_cache(
            vec![Subscription::Properties {
                properties: vec![UIA_NamePropertyId],
                callback: Arc::new(|element, _| {
                    say(&format!("uia-name {:?}", element.cached_string(UIA_NamePropertyId)));
                }),
            }],
            Scope::Windows(vec![b_hwnd]),
            &[UIA_NamePropertyId],
        )
        .expect("names");
        say("ready");
        for _ in std::io::stdin().lock().lines() {}
        std::process::exit(0);
    }

    /// A process-scoped hook on `pid`, on a thread of its own, sending
    /// what it hears.
    fn hook(pid: u32, kinds: &'static [WinEventKind]) -> Receiver<(WinEventKind, isize, i32, i32, Instant)> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            crate::common::init_com();
            let tx = Mutex::new(tx);
            let _hook = WinEventHook::install(
                pid,
                kinds,
                Box::new(move |kind, hwnd, object, child, _| {
                    let at = Instant::now();
                    if kind == WinEventKind::Focus {
                        // Acquired at once, in the callback, as an
                        // in-context client would.
                        println!(
                            "RESULT e_bridge: in the callback, ({hwnd}, {object}, {child}) gives {}",
                            msaa_object(hwnd, object, child)
                        );
                    }
                    let _ = tx
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .send((kind, hwnd, object, child, at));
                }),
            )
            .expect("hook");
            let mut message = MSG::default();
            // SAFETY: this thread's message loop.
            while unsafe { GetMessageW(&raw mut message, None, 0, 0) }.as_bool() {
                // SAFETY: the message just taken.
                unsafe { DispatchMessageW(&raw const message) };
            }
        });
        rx
    }

    fn raise(event: u32, hwnd: isize) {
        // SAFETY: a WinEvent for a live window, from this process.
        unsafe { NotifyWinEvent(event, HWND(hwnd as *mut _), OBJID_CLIENT.0, CHILDID_SELF as i32) };
    }

    /// What MSAA gives for a WinEvent's address: role and name.
    fn msaa_object(hwnd: isize, object: i32, child: i32) -> String {
        let mut accessible: Option<IAccessible> = None;
        let mut variant = VARIANT::default();
        // SAFETY: out-pointers for the object and its child id.
        let acquired = unsafe {
            AccessibleObjectFromEvent(
                HWND(hwnd as *mut _),
                object as u32,
                child as u32,
                &raw mut accessible,
                &raw mut variant,
            )
        };
        match (acquired, accessible) {
            (Ok(()), Some(accessible)) => {
                // SAFETY: cross-process reads on a live object.
                let name = unsafe { accessible.get_accName(&variant) }.map(|name| name.to_string());
                // SAFETY: as above.
                let role = unsafe { accessible.get_accRole(&variant) }
                    .ok()
                    .and_then(|role| i32::try_from(&role).ok());
                format!("role {role:?} name {name:?}")
            }
            (result, _) => format!("not acquired: {result:?}"),
        }
    }

    /// Lines from the listener stand-in, with when this process read them.
    fn lines(child: &mut std::process::Child) -> Receiver<(String, Instant)> {
        let out = child.stdout.take().expect("stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { return };
                if tx.send((line, Instant::now())).is_err() {
                    return;
                }
            }
        });
        rx
    }

    /// The first listener line containing `what` within `wait`, as ms
    /// after `since`.
    fn listener_saw(rx: &Receiver<(String, Instant)>, what: &str, since: Instant, wait: Duration) -> Option<u128> {
        let deadline = Instant::now() + wait;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let Ok((line, at)) = rx.recv_timeout(left) else { return None };
            if line.contains(what) {
                return Some(at.saturating_duration_since(since).as_millis());
            }
        }
        None
    }

    fn after<T>(rx: &Receiver<T>, since: Instant, wait: Duration, at: impl Fn(&T) -> Instant) -> Option<u128> {
        rx.recv_timeout(wait).ok().map(|value| at(&value).saturating_duration_since(since).as_millis())
    }

    pub fn experiment() {
        crate::common::init_com();
        let title = crate::common::unique_title("exp-cross-p1");
        let mut p1 = crate::common::spawn("tree_view.json", "msaa", &title);
        let tree = crate::common::tree_view::tree_view(crate::common::find_window(&title)).0 as isize;
        let b_title = crate::common::unique_title("exp-cross-b");
        let mut b = crate::common::spawn("text.json", "uia", &b_title);
        let b_hwnd = crate::common::find_window(&b_title).0 as isize;
        let bm_title = crate::common::unique_title("exp-cross-bm");
        let mut bm = crate::common::spawn("small.json", "msaa", &bm_title);
        let bm_hwnd = crate::common::find_window(&bm_title).0 as isize;
        let b2_title = crate::common::unique_title("exp-cross-b2");
        let mut b2 = crate::common::spawn("small.json", "uia", &b2_title);

        // The outpost stand-in: this process, no focus registration.
        let (name_tx, names) = mpsc::channel();
        let name_tx = Mutex::new(name_tx);
        let (text_tx, texts) = mpsc::channel();
        let text_tx = Mutex::new(text_tx);
        let _b_events = Registration::with_cache(
            vec![
                Subscription::Properties {
                    properties: vec![UIA_NamePropertyId],
                    callback: Arc::new(move |_, _| {
                        let _ = name_tx.lock().unwrap_or_else(PoisonError::into_inner).send(Instant::now());
                    }),
                },
                Subscription::Events {
                    events: vec![UIA_Text_TextChangedEventId],
                    callback: Arc::new(move |_, _| {
                        let _ = text_tx.lock().unwrap_or_else(PoisonError::into_inner).send(Instant::now());
                    }),
                },
            ],
            Scope::Windows(vec![b_hwnd]),
            &[UIA_NamePropertyId],
        )
        .expect("B's events");
        let bm_hook = hook(bm.pid(), &[WinEventKind::NameChange]);
        let b_hook = hook(b.pid(), &[WinEventKind::Focus, WinEventKind::NameChange]);
        let b2_hook = hook(b2.pid(), &[WinEventKind::Focus]);

        let mut listener = Command::new(std::env::current_exe().expect("exe"))
            .env("VERBATIM_EXP_LISTENER", b_hwnd.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("the listener stand-in");
        let from_listener = lines(&mut listener);
        assert!(listener_saw(&from_listener, "ready", Instant::now(), Duration::from_secs(15)).is_some(), "the listener stand-in is ready");

        let read_b = move || {
            std::thread::spawn(move || {
                crate::common::init_com();
                let uia = Uia::new().expect("uia");
                let cache = uia.base_cache_request().expect("cache");
                let started = Instant::now();
                let name = uia.element_from_handle(b_hwnd, &cache).map(|root| root.cached_string(UIA_NamePropertyId));
                (started.elapsed().as_millis(), format!("{name:?}"))
            })
            .join()
            .expect("join")
        };

        for (round, stalled) in [("baseline", false), ("P1 hung, focus WinEvent raised for it", true)] {
            if stalled {
                p1.stall(Duration::from_secs(10));
                raise(EVENT_OBJECT_FOCUS, tree);
                let _ = names.recv_timeout(Duration::from_millis(300));
            }
            let start = Instant::now();
            b.send(&format!("set-name doc Name {round}"));
            let outpost_name = after(&names, start, Duration::from_secs(20), |at| *at);
            let listener_name = listener_saw(&from_listener, "uia-name", start, Duration::from_secs(20));
            let start = Instant::now();
            b.send("text-changed doc");
            let outpost_text = after(&texts, start, Duration::from_secs(20), |at| *at);
            let read = read_b();
            let start = Instant::now();
            bm.send(&format!("set-name btn1 Name {round}"));
            let outpost_hook = after(&bm_hook, start, Duration::from_secs(20), |(_, _, _, _, at)| *at);
            let start = Instant::now();
            raise(EVENT_OBJECT_FOCUS, bm_hwnd);
            let listener_focus = listener_saw(&from_listener, &format!("winevent 0x8005 hwnd={bm_hwnd}"), start, Duration::from_secs(20));
            let start = Instant::now();
            raise(EVENT_SYSTEM_FOREGROUND, bm_hwnd);
            let listener_foreground = listener_saw(&from_listener, &format!("winevent 0x3 hwnd={bm_hwnd}"), start, Duration::from_secs(20));
            println!(
                "RESULT e_cross {round}: outpost stand-in: B name event {outpost_name:?} ms, B text event {outpost_text:?} ms, explicit read {read:?}, hook on Bm {outpost_hook:?} ms; listener stand-in: its UIA name event for B {listener_name:?} ms, its focus WinEvent {listener_focus:?} ms, foreground WinEvent {listener_foreground:?} ms"
            );

            // E-bridge: a UIA provider's focus, as MSAA sees it.
            while b_hook.try_recv().is_ok() {}
            while b2_hook.try_recv().is_ok() {}
            for (which, target) in [(1, "doc"), (2, "btn1"), (2, "item1")] {
                let (app, hook) = if which == 1 { (&mut b, &b_hook) } else { (&mut b2, &b2_hook) };
                let start = Instant::now();
                app.send(&format!("focus {target}"));
                match hook.recv_timeout(Duration::from_secs(3)) {
                    Ok((kind, hwnd, object, child, at)) => println!(
                        "RESULT e_bridge {round}: UIA focus on {target}: {kind:?} after {} ms, address ({hwnd}, {object}, {child}), MSAA gives {}",
                        at.saturating_duration_since(start).as_millis(),
                        msaa_object(hwnd, object, child)
                    ),
                    Err(_) => println!("RESULT e_bridge {round}: UIA focus on {target}: no WinEvent within 3 s"),
                }
                while hook.try_recv().is_ok() {}
            }
            if stalled {
                let _ = p1.stall_ended(Duration::from_secs(10));
            }
        }
        drop(listener.stdin.take());
        let _ = listener.kill();
        let _ = listener.wait();
        let _ = EVENT_OBJECT_NAMECHANGE;
        p1.kill();
        b.kill();
        bm.kill();
        b2.kill();
    }
}

mod marshal {
    //! E-marshal: whether a UIA element can be handed to another process.

    use std::ffi::c_void;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use verbatim_uia::{CACHED_PROPERTIES, ElementExt, Uia};
    use verbatim_uia_rops::{FocusQuery, focus_ancestry_remote};
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
    use windows::Win32::System::Com::{
        IStream, MSHCTX_LOCAL, MSHLFLAGS_NORMAL, STREAM_SEEK_END, STREAM_SEEK_SET,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationPropertyChangedEventHandler, TreeScope_Descendants,
        TreeScope_Subtree, UIA_NamePropertyId,
    };
    use windows::core::{BSTR, GUID, HRESULT, Interface};

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoMarshalInterface(
            stream: *mut c_void,
            riid: *const GUID,
            unknown: *mut c_void,
            context: u32,
            reserved: *const c_void,
            flags: u32,
        ) -> HRESULT;
        fn CoUnmarshalInterface(stream: *mut c_void, riid: *const GUID, out: *mut *mut c_void)
        -> HRESULT;
        fn CoMarshalInterThreadInterfaceInStream(
            riid: *const GUID,
            unknown: *mut c_void,
            out: *mut *mut c_void,
        ) -> HRESULT;
        fn CoUninitialize();
    }

    fn stream_bytes(stream: &IStream) -> Vec<u8> {
        let mut size = 0u64;
        // SAFETY: a live stream; plain out-pointers.
        unsafe { stream.Seek(0, STREAM_SEEK_END, Some(&raw mut size)) }.expect("seek");
        // SAFETY: as above.
        unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }.expect("seek");
        let mut bytes = vec![0u8; usize::try_from(size).expect("size")];
        let mut read = 0u32;
        // SAFETY: the buffer is as long as the count given.
        let _ = unsafe {
            stream.Read(
                bytes.as_mut_ptr().cast(),
                u32::try_from(bytes.len()).expect("len"),
                Some(&raw mut read),
            )
        };
        bytes.truncate(read as usize);
        bytes
    }

    /// The OBJREF's kind: 1 standard, 2 handler, 4 custom (and the custom
    /// unmarshal class).
    fn describe(bytes: &[u8]) -> String {
        if bytes.len() < 24 || &bytes[0..4] != b"MEOW" {
            return format!("{} bytes, not an OBJREF", bytes.len());
        }
        let flags = u32::from_le_bytes(bytes[4..8].try_into().expect("4"));
        let kind = match flags {
            1 => "standard".to_owned(),
            2 => "handler".to_owned(),
            4 => {
                let field = &bytes[24..40];
                let clsid = GUID::from_values(
                    u32::from_le_bytes(field[0..4].try_into().expect("4")),
                    u16::from_le_bytes(field[4..6].try_into().expect("2")),
                    u16::from_le_bytes(field[6..8].try_into().expect("2")),
                    field[8..16].try_into().expect("8"),
                );
                format!("custom, unmarshal class {clsid:?}")
            }
            8 => "extended".to_owned(),
            other => format!("flags {other}"),
        };
        format!("{} bytes, OBJREF {kind}", bytes.len())
    }

    fn unmarshal(bytes: &[u8]) -> windows::core::Result<IUIAutomationElement> {
        // SAFETY: a new stream that owns its memory.
        let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }?;
        // SAFETY: the buffer is as long as the count given.
        unsafe {
            stream.Write(
                bytes.as_ptr().cast(),
                u32::try_from(bytes.len()).expect("len"),
                None,
            )
        }
        .ok()?;
        // SAFETY: as above.
        unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }?;
        let mut out = std::ptr::null_mut();
        // SAFETY: a live stream and an out-pointer for the interface.
        unsafe { CoUnmarshalInterface(stream.as_raw(), &IUIAutomationElement::IID, &raw mut out) }
            .ok()?;
        // SAFETY: an owned reference CoUnmarshalInterface returned.
        Ok(unsafe { IUIAutomationElement::from_raw(out) })
    }

    /// The owning process: reads mockapp's window element, marshals it two
    /// ways, writes both, and then obeys `uninit` and `exit` on stdin.
    pub fn owner(hwnd: isize, path: &str) -> ! {
        crate::common::init_com();
        let uia = Uia::new().expect("uia");
        let cache = uia.base_cache_request().expect("cache");
        let element = uia.element_from_handle(hwnd, &cache).expect("root");
        // SAFETY: a new stream that owns its memory.
        let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }.expect("stream");
        let started = Instant::now();
        // SAFETY: live stream and element.
        let local = unsafe {
            CoMarshalInterface(
                stream.as_raw(),
                &IUIAutomationElement::IID,
                element.as_raw(),
                MSHCTX_LOCAL.0 as u32,
                std::ptr::null(),
                MSHLFLAGS_NORMAL.0 as u32,
            )
        };
        let local_us = started.elapsed().as_micros();
        let local_bytes = stream_bytes(&stream);
        let mut out = std::ptr::null_mut();
        // SAFETY: live element; an out-pointer for the stream.
        let inter = unsafe {
            CoMarshalInterThreadInterfaceInStream(&IUIAutomationElement::IID, element.as_raw(), &raw mut out)
        };
        let inter_bytes = if inter.is_ok() {
            // SAFETY: an owned stream reference.
            stream_bytes(&unsafe { IStream::from_raw(out) })
        } else {
            Vec::new()
        };
        // Within this process, to another apartment: a single-threaded one.
        let in_process = {
            // SAFETY: a new stream that owns its memory.
            let stream2 = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }.expect("stream");
            // SAFETY: live stream and element.
            let marshaled = unsafe {
                CoMarshalInterface(
                    stream2.as_raw(),
                    &IUIAutomationElement::IID,
                    element.as_raw(),
                    MSHCTX_LOCAL.0 as u32,
                    std::ptr::null(),
                    MSHLFLAGS_NORMAL.0 as u32,
                )
            };
            let bytes = stream_bytes(&stream2);
            let answer = std::thread::spawn(move || {
                // SAFETY: joins a single-threaded apartment on this new thread.
                let _ = unsafe {
                    windows::Win32::System::Com::CoInitializeEx(
                        None,
                        windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
                    )
                };
                match unmarshal(&bytes) {
                    Ok(proxy) => format!("unmarshaled; CurrentName {:?}", name(&proxy)),
                    Err(error) => format!("unmarshal failed {error:?}"),
                }
            })
            .join()
            .expect("join");
            format!("marshal {marshaled:?}, {answer}")
        };
        println!("owner-sta {in_process}");
        // A control: an event handler object of our own, an interface from
        // the same type library, marshaled the same way.
        let (control_tx, _control_rx) = std::sync::mpsc::channel();
        let control: windows::Win32::UI::Accessibility::IUIAutomationEventHandler =
            super::handler::Events {
                tx: std::sync::Mutex::new(control_tx),
            }
            .into();
        // SAFETY: a new stream that owns its memory.
        let control_stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }.expect("stream");
        // SAFETY: live stream and object.
        let control_marshaled = unsafe {
            CoMarshalInterface(
                control_stream.as_raw(),
                &windows::Win32::UI::Accessibility::IUIAutomationEventHandler::IID,
                control.as_raw(),
                MSHCTX_LOCAL.0 as u32,
                std::ptr::null(),
                MSHLFLAGS_NORMAL.0 as u32,
            )
        };
        let control_bytes = stream_bytes(&control_stream);
        println!("owner-control {control_marshaled:?} {}", describe(&control_bytes));
        std::fs::write(format!("{path}.c"), &control_bytes).expect("write");
        std::fs::write(format!("{path}.a"), &local_bytes).expect("write");
        std::fs::write(format!("{path}.b"), &inter_bytes).expect("write");
        println!(
            "ready CoMarshalInterface {local:?} in {local_us} us: {} | CoMarshalInterThreadInterfaceInStream {inter:?}: {} | owner pid {}",
            describe(&local_bytes),
            describe(&inter_bytes),
            std::process::id()
        );
        let _ = std::io::stdout().flush();
        for line in std::io::stdin().lock().lines() {
            match line.as_deref() {
                Ok("uninit") => {
                    drop(stream.clone());
                    // SAFETY: leaves this thread's apartment while the stub
                    // and the element are still held.
                    unsafe { CoUninitialize() };
                    println!("uninitialized");
                    let _ = std::io::stdout().flush();
                }
                _ => break,
            }
        }
        std::process::exit(0);
    }

    fn timed_avg<T>(count: u32, mut call: impl FnMut() -> T) -> (u128, T) {
        let started = Instant::now();
        let mut last = call();
        for _ in 1..count {
            last = call();
        }
        (started.elapsed().as_micros() / u128::from(count), last)
    }

    fn name(element: &IUIAutomationElement) -> windows::core::Result<String> {
        // SAFETY: a live element.
        unsafe { element.CurrentName() }.map(|name| name.to_string())
    }

    fn calls(label: &str, element: &IUIAutomationElement, uia: &Uia) {
        let (us, answer) = timed_avg(20, || name(element));
        println!("RESULT e_marshal: {label}: CurrentName {answer:?}, {us} us per call");
        let (us, answer) = timed_avg(20, || element.has_keyboard_focus());
        println!("RESULT e_marshal: {label}: CurrentHasKeyboardFocus {answer:?}, {us} us per call");
        let (us, answer) = timed_avg(20, || element.cached_string(UIA_NamePropertyId));
        println!("RESULT e_marshal: {label}: cached name {answer:?}, {us} us per read");
        let value = VARIANT::from(BSTR::from("First"));
        let condition = uia.property_condition(UIA_NamePropertyId, &value).expect("condition");
        let cache = uia.base_cache_request().expect("cache");
        let (us, answer) = timed_avg(5, || {
            element
                .find_first_build_cache(TreeScope_Descendants, &condition, &cache)
                .map(|found| found.map(|found| found.cached_string(UIA_NamePropertyId)))
        });
        println!("RESULT e_marshal: {label}: FindFirst {answer:?}, {us} us per call");
        let started = Instant::now();
        let remote = focus_ancestry_remote(
            uia,
            &FocusQuery {
                element,
                known: &[],
                previous: None,
                depth_limit: 50,
                properties: CACHED_PROPERTIES,
                deadline: None,
                require_focus: false,
            },
        );
        println!(
            "RESULT e_marshal: {label}: remote operation {}, {} us",
            match &remote {
                Ok(_) => "answered".to_owned(),
                Err(error) => format!("failed: {error:?}"),
            },
            started.elapsed().as_micros()
        );
    }

    pub fn experiment() {
        crate::common::init_com();
        let (mut app, hwnd) = crate::start("exp-marshal");
        let path = std::env::temp_dir()
            .join(format!("verbatim-marshal-{}", std::process::id()))
            .display()
            .to_string();
        let mut owner = Command::new(std::env::current_exe().expect("exe"))
            .env("VERBATIM_EXP_OWNER", format!("{hwnd};{path}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("owner");
        let mut owner_in = owner.stdin.take().expect("stdin");
        let mut owner_out = BufReader::new(owner.stdout.take().expect("stdout")).lines();
        let sta = owner_out.next().expect("sta").expect("line");
        println!("RESULT e_marshal: owner, to its own STA thread: {sta}");
        let control = owner_out.next().expect("control").expect("line");
        println!("RESULT e_marshal: owner's control object: {control}");
        let ready = owner_out.next().expect("ready").expect("line");
        println!("RESULT e_marshal: owner says: {ready}");
        {
            let bytes = std::fs::read(format!("{path}.c")).expect("read");
            // SAFETY: a new stream that owns its memory.
            let stream = unsafe { CreateStreamOnHGlobal(HGLOBAL::default(), true) }.expect("stream");
            // SAFETY: the buffer is as long as the count given.
            let _ = unsafe {
                stream.Write(bytes.as_ptr().cast(), u32::try_from(bytes.len()).expect("len"), None)
            };
            // SAFETY: as above.
            unsafe { stream.Seek(0, STREAM_SEEK_SET, None) }.expect("seek");
            let mut out = std::ptr::null_mut();
            let iid = windows::Win32::UI::Accessibility::IUIAutomationEventHandler::IID;
            // SAFETY: a live stream and an out-pointer.
            let result = unsafe { CoUnmarshalInterface(stream.as_raw(), &iid, &raw mut out) };
            let called = if result.is_ok() {
                // SAFETY: an owned reference.
                let proxy = unsafe {
                    windows::Win32::UI::Accessibility::IUIAutomationEventHandler::from_raw(out)
                };
                let uia = Uia::new().expect("uia");
                let cache = uia.base_cache_request().expect("cache");
                let direct = uia.element_from_handle(hwnd, &cache).expect("direct");
                // SAFETY: a call through the proxy with a live element.
                let with_element = format!("{:?}", unsafe {
                    proxy.HandleAutomationEvent(&direct, windows::Win32::UI::Accessibility::UIA_MenuOpenedEventId)
                });
                // SAFETY: a call through the proxy with no element.
                let without = unsafe {
                    proxy.HandleAutomationEvent(
                        None::<&IUIAutomationElement>,
                        windows::Win32::UI::Accessibility::UIA_MenuOpenedEventId,
                    )
                };
                format!("with an element argument {with_element}; with none {without:?}")
            } else {
                String::new()
            };
            println!("RESULT e_marshal: control object unmarshaled here: {result:?}; a call through it: {called}");
        }

        let uia = Uia::new().expect("uia");
        let cache = uia.base_cache_request().expect("cache");
        let direct = uia.element_from_handle(hwnd, &cache).expect("direct");
        calls("direct element", &direct, &uia);

        let local = std::fs::read(format!("{path}.a")).expect("read");
        let started = Instant::now();
        let proxy = unmarshal(&local);
        println!(
            "RESULT e_marshal: CoUnmarshalInterface (MSHCTX_LOCAL) = {:?} in {} us",
            proxy.as_ref().map(|_| ()),
            started.elapsed().as_micros()
        );
        let inter = std::fs::read(format!("{path}.b")).expect("read");
        let other = unmarshal(&inter);
        println!(
            "RESULT e_marshal: CoUnmarshalInterface (inter-thread stream) = {:?}",
            other.as_ref().map(|_| ())
        );
        let Ok(proxy) = proxy else {
            let _ = writeln!(owner_in, "exit");
            let _ = owner.wait();
            app.quit();
            return;
        };
        calls("unmarshaled element", &proxy, &uia);

        // Events registered on the unmarshaled element through this
        // process's own client.
        let (tx, rx) = std::sync::mpsc::channel();
        let handler: IUIAutomationPropertyChangedEventHandler = super::handler::Props {
            tx: std::sync::Mutex::new(tx),
        }
        .into();
        // SAFETY: live client, element, cache and handler; one property.
        let added = unsafe {
            uia.client().AddPropertyChangedEventHandlerNativeArray(
                &proxy,
                TreeScope_Subtree,
                &cache,
                &handler,
                &[UIA_NamePropertyId],
            )
        };
        println!("RESULT e_marshal: AddPropertyChangedEventHandler on the unmarshaled element = {added:?}");
        app.send("set-name btn1 Renamed");
        println!(
            "RESULT e_marshal: its name change event: {:?}",
            rx.recv_timeout(Duration::from_secs(3)).ok()
        );

        let _ = writeln!(owner_in, "uninit");
        println!("RESULT e_marshal: owner after uninit: {:?}", owner_out.next());
        let started = Instant::now();
        println!(
            "RESULT e_marshal: after the owner left its apartment: CurrentName {:?} in {} us",
            name(&proxy),
            started.elapsed().as_micros()
        );
        let _ = writeln!(owner_in, "exit");
        let _ = owner.wait();
        let started = Instant::now();
        println!(
            "RESULT e_marshal: after the owner exited: CurrentName {:?} in {} us; HasKeyboardFocus {:?}",
            name(&proxy),
            started.elapsed().as_micros(),
            proxy.has_keyboard_focus()
        );
        let _ = std::fs::remove_file(format!("{path}.a"));
        let _ = std::fs::remove_file(format!("{path}.b"));
        app.quit();
    }
}

fn main() {
    if let Ok(b_hwnd) = std::env::var("VERBATIM_EXP_LISTENER") {
        cross::listener(b_hwnd.parse().expect("hwnd"));
    }
    if std::env::var("VERBATIM_EXP_MAPPING").is_ok() {
        common::init_com();
        let uia = Uia::new().expect("uia");
        println!("maps focus {}", mapping::maps_focus(uia.client()));
        std::process::exit(0);
    }
    if let Ok(owner) = std::env::var("VERBATIM_EXP_OWNER") {
        let (hwnd, path) = owner.split_once(';').expect("hwnd;path");
        marshal::owner(hwnd.parse().expect("hwnd"), path);
    }
    harness::run_isolated(&[
        ("e_marshal", marshal::experiment),
        ("e_hung_proxy", e_hung_proxy),
        ("e_mapping", e_mapping),
        ("e_hung_cause", e_hung_cause),
        ("e_cross", cross::experiment),
        ("e_concurrency", e_concurrency),
        ("e_delivery", e_delivery),
        ("e_scope", e_scope),
        ("e_cost", e_cost),
        ("e1_timeout_scope", e1_timeout_scope),
        ("e2_stall_within", e2_stall_within),
        ("e3_hold_never_released", e3_hold_never_released),
        ("e4_hold_then_kill", e4_hold_then_kill),
        ("e5_focus_read_in_held_application", e5_focus_read_in_held_application),
        ("e_tt", e_tt),
        ("e_tt_transactions", e_tt_transactions),
        ("e4_transaction_then_kill", e4_transaction_then_kill),
    ]);
}
