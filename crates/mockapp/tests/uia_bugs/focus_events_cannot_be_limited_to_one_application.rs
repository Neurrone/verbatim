//! `docs/uia-bugs.md`, "Focus events cannot be limited to one application"
//! (E-scope and P1 in its experiments folder): the documented call that
//! would limit focus events to one window is refused; an event handler
//! group on one window, which is accepted, hears that application's own
//! focus events and no other application's, but none of a window UIA reads
//! through its proxy, whose focus a desktop-wide handler hears; and the
//! core API's registration on one window's node (P5) hears that
//! application's focus alone too.
//!
//! On a test's own desktop no window has the keyboard focus, so a focus a
//! provider raises reaches only a registration scoped to its window, and a
//! desktop-wide handler hears only the focus UIA makes from a focus
//! `WinEvent`, which the MSAA backend raises.

use std::sync::mpsc;
use std::sync::{Mutex, PoisonError};

use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::SafeArrayGetElement;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    AutomationElementMode_Full, ConditionType_True, HUIAEVENT, HUIANODE, TreeScope_Element,
    TreeScope_Subtree, UIA_AutomationFocusChangedEventId, UIA_ProcessIdPropertyId, UiaAddEvent,
    UiaCacheRequest, UiaCondition, UiaEventArgs, UiaNodeFromHandle,
};

use crate::common;
use crate::support::{self, Handler};

/// `IUIAutomation::AddAutomationEventHandler` for the focus event on one
/// window's subtree fails with `E_INVALIDARG` and UIA's message.
pub fn a_focus_handler_on_one_window_is_refused() {
    let (app, hwnd) = support::mockapp("scope-refused", "uia");
    let uia = support::client();
    let window = support::element(uia.client(), hwnd);
    let handler = Handler::new();
    // SAFETY: live client, element, cache request and handler.
    let added = unsafe {
        uia.client().AddAutomationEventHandler(
            UIA_AutomationFocusChangedEventId,
            &window,
            TreeScope_Subtree,
            &support::cache(uia.client()),
            &handler.events,
        )
    };
    let refused = added.expect_err("UIA refuses the registration");
    assert_eq!(
        (refused.code(), refused.message()),
        (
            E_INVALIDARG,
            "Focus events must be registered using IUIAutomation::AddFocusChangedEventHandler"
                .to_owned()
        )
    );
    app.quit();
}

/// A group with the focus event on A's window hears A's focus events and
/// not B's, while a group on B's window hears B's. B's events reach both
/// groups' process together, as one raise, so the group on A's window,
/// which hears A's event after B's first, would have heard B's first
/// before it.
pub fn a_focus_event_in_a_group_on_one_window_hears_that_applications_focus_alone() {
    let (mut a, a_hwnd) = support::mockapp("scope-a", "uia");
    let (mut b, b_hwnd) = support::mockapp("scope-b", "uia");
    let uia = support::client();
    let on_a = Handler::new();
    let on_b = Handler::new();
    let _a_group = support::group_on(
        uia.client(),
        &support::element(uia.client(), a_hwnd),
        &on_a,
        false,
    );
    let _b_group = support::group_on(
        uia.client(),
        &support::element(uia.client(), b_hwnd),
        &on_b,
        false,
    );

    b.send("focus btn1");
    assert_eq!(on_b.next().0, support::from(&b, "Original Name"));
    a.send("focus item1");
    assert_eq!(on_a.next().0, support::from(&a, "First"));
    b.send("focus item2");
    assert_eq!(on_b.next().0, support::from(&b, "Second"));
    on_a.heard_nothing_more();
    on_b.heard_nothing_more();
    a.quit();
    b.quit();
}

/// A group with the focus event and name changes on the window of an MSAA
/// mockapp, which UIA reads through its proxy, hears none of its focus
/// events, which a desktop-wide focus handler hears, but hears the name
/// change raised after the focus: UIA's proxy delivers the group that
/// window's events, in the order they were raised, and the focus is not
/// among them.
///
/// The client is a plain one, since `verbatim_uia`'s tells its proxies not
/// to raise name changes from `WinEvent`s.
pub fn a_focus_event_in_a_group_on_a_proxied_window_hears_none_of_its_focus() {
    let (mut m, m_hwnd) = support::mockapp("scope-proxied", "msaa");
    let _setup = support::client();
    // SAFETY: CUIAutomation8 is a registered in-process COM server whose
    // objects implement IUIAutomation; this thread is in COM's
    // multithreaded apartment, which the client above joined.
    let client: windows::Win32::UI::Accessibility::IUIAutomation = unsafe {
        windows::Win32::System::Com::CoCreateInstance(
            &windows::Win32::UI::Accessibility::CUIAutomation8,
            None,
            windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
        )
    }
    .expect("a plain UIA client");
    let desktop = Handler::new();
    // SAFETY: live client, cache request and handler.
    unsafe { client.AddFocusChangedEventHandler(&support::cache(&client), &desktop.focus) }
        .expect("the desktop-wide focus handler");
    let on_m = Handler::new();
    let _group = support::group_on(&client, &support::element(&client, m_hwnd), &on_m, true);

    m.send("focus btn1");
    assert_eq!(desktop.next().0, support::from(&m, "Original Name"));
    m.send("set-name btn1 Renamed");
    assert_eq!(on_m.next().0, support::from(&m, "Renamed"));
    on_m.heard_nothing_more();
    desktop.heard_nothing_more();
    m.quit();
}

/// Where the core API's focus registration sends the process id of each
/// sender it hears: its callback is given no context of its own.
static CORE_HEARD: Mutex<Option<mpsc::Sender<u32>>> = Mutex::new(None);

/// The core API's event callback: sends the sender's process id, the one
/// property the registration's cache request asks for, which UIA delivers
/// in a two-dimensional array, one row per element: the sender's node, and
/// then the property.
unsafe extern "system" fn core_heard(
    _arguments: *mut UiaEventArgs,
    data: *mut SAFEARRAY,
    _tree: windows_core::Ref<windows_core::BSTR>,
) {
    let mut value = VARIANT::default();
    // SAFETY: the array UIA passes the callback, live for the call, and an
    // index pair within its bounds, one row by two columns; the element is
    // copied into `value`.
    unsafe { SafeArrayGetElement(data, [0, 1].as_ptr(), (&raw mut value).cast()) }
        .expect("the sender's cached process id");
    let pid = verbatim_uia::variant_i32(&value)
        .and_then(|pid| u32::try_from(pid).ok())
        .expect("a process id");
    if let Some(sender) = CORE_HEARD
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
    {
        let _ = sender.send(pid);
    }
}

/// The core API's route (P5): `UiaAddEvent` for the focus event on A's
/// window's node, with its subtree, hears A's provider-raised focus events
/// and not B's. B's events would reach the registration's process with
/// A's, in the order raised, so each of A's is heard after any of B's
/// raised before it.
pub fn a_core_api_focus_registration_on_one_window_hears_that_applications_focus_alone() {
    let (mut a, a_hwnd) = support::mockapp("scope-core-a", "uia");
    let (mut b, _) = support::mockapp("scope-core-b", "uia");
    let _setup = support::client();
    let (sender, heard) = mpsc::channel();
    *CORE_HEARD.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
    let mut node = HUIANODE::default();
    // SAFETY: a window handle, which UIA checks, and an out-pointer for the
    // node.
    unsafe { UiaNodeFromHandle(a_hwnd, &raw mut node) }.expect("A's window's node");
    let mut condition = UiaCondition {
        ConditionType: ConditionType_True,
    };
    let mut properties = [UIA_ProcessIdPropertyId.0];
    let mut request = UiaCacheRequest {
        pViewCondition: (&raw mut condition).cast(),
        Scope: TreeScope_Element,
        pProperties: properties.as_mut_ptr().cast(),
        cProperties: 1,
        pPatterns: std::ptr::null_mut(),
        cPatterns: 0,
        automationElementMode: AutomationElementMode_Full,
    };
    let callback: unsafe extern "system" fn(_, _, _) = core_heard;
    let mut event = HUIAEVENT::default();
    // SAFETY: a live node; the callback itself, which the C API takes where
    // the binding names a pointer to it; the request and the properties it
    // points to, which outlive the call; and an out-pointer for the event.
    unsafe {
        UiaAddEvent(
            node,
            UIA_AutomationFocusChangedEventId.0,
            callback as usize as *mut _,
            TreeScope_Subtree,
            std::ptr::null_mut(),
            0,
            &raw mut request,
            &raw mut event,
        )
    }
    .expect("the core API's registration");

    b.send("focus btn1");
    a.send("focus item1");
    let next = || {
        heard
            .recv_timeout(common::WAIT_TIMEOUT)
            .expect("a focus event heard")
    };
    assert_eq!(next(), a.pid());
    b.send("focus item2");
    a.send("focus btn1");
    assert_eq!(next(), a.pid());
    assert_eq!(heard.try_recv(), Err(mpsc::TryRecvError::Empty));
    a.quit();
    b.quit();
}
