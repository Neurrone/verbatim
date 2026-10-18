//! `docs/uia-bugs.md`, "Focus events cannot be limited to one application"
//! (E-scope and P1 in its experiments folder): the documented call that
//! would limit focus events to one window is refused; an event handler
//! group on one window, which is accepted, hears that application's own
//! focus events and no other application's, but none of a window UIA reads
//! through its proxy, whose focus a desktop-wide handler hears.
//!
//! On a test's own desktop no window has the keyboard focus, so a focus a
//! provider raises reaches only a registration scoped to its window, and a
//! desktop-wide handler hears only the focus UIA makes from a focus
//! `WinEvent`, which the MSAA backend raises.

use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::UI::Accessibility::{TreeScope_Subtree, UIA_AutomationFocusChangedEventId};

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
