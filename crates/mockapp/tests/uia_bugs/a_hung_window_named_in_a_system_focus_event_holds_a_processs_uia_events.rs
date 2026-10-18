//! `docs/uia-bugs.md`, "A hung window named in a system focus event holds a
//! process's UIA events" (`h1` to `h3` and `e_hung_cause` in its
//! experiments folder): in a process holding a UIA focus registration, a
//! focus `WinEvent` naming a window that UIA reads through its proxy and
//! that answers no message holds all of the process's UIA events while
//! UIA's focus tracking sends that window two messages in turn, each
//! waiting the connection timeout.
//!
//! The hung window is mockapp's comctl32 tree view, on the MSAA backend,
//! its window thread stalled until the test releases it
//! (`stall-until-release`), which names each client's message sent to it:
//! the evidence of UIA's two sends, `WM_GETOBJECT` for its root object and
//! then its registered `UiaServerConnection_HookMessage`. The held event is
//! a name change in a second mockapp, on the UIA backend.

use std::time::{Duration, Instant};

use verbatim_ia2::WinEventKind;
use verbatim_model::{NormalizedEvent, PropertyChange};
use verbatim_outpost::Heard;
use verbatim_outpost::protocol::OutpostToSupervisor;
use windows::Win32::UI::Accessibility::{NotifyWinEvent, UIA_NamePropertyId};
use windows::Win32::UI::WindowsAndMessaging::{CHILDID_SELF, EVENT_OBJECT_FOCUS, OBJID_CLIENT};

use crate::common;
use crate::common::outpost::OutpostUnderTest;
use crate::support::{self, Handler};

/// The connection timeout these tests set, short so that the hold is too.
const CONNECTION_TIMEOUT: Duration = Duration::from_millis(500);

/// The class of mockapp's tree view, as Windows Forms names it.
const TREE_VIEW: &str = "WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1";

/// A mockapp whose tree view is the hung window, and the tree view.
fn tree_view() -> (common::MockApp, windows::Win32::Foundation::HWND) {
    let title = common::unique_title("hung-tree-view");
    let app = common::spawn("tree_view.json", "msaa", &title);
    let tree = common::tree_view::tree_view(common::find_window(&title));
    (app, tree)
}

/// Stalls `tree_view`'s window thread until it is released, and raises a
/// focus `WinEvent` naming the tree view; returns when it was raised.
fn hang_and_name_in_a_focus_event(
    tree_view: &mut common::MockApp,
    tree: windows::Win32::Foundation::HWND,
) -> Instant {
    tree_view.write("stall-until-release");
    tree_view.expect_line("stall started", String::new);
    let child = i32::try_from(CHILDID_SELF).expect("CHILDID_SELF is 0");
    let raised = Instant::now();
    // SAFETY: a WinEvent for a live window, raised from this process.
    unsafe { NotifyWinEvent(EVENT_OBJECT_FOCUS, tree, OBJID_CLIENT.0, child) };
    raised
}

/// With a desktop-wide focus handler in this process and the connection
/// timeout at 500 ms, another application's name change, raised once UIA
/// has sent the hung tree view its first message, is not heard when UIA
/// sends the second, and arrives after that, more than one connection
/// timeout after the focus `WinEvent`, while the tree view still hangs:
/// UIA's two sends, each ended by the timeout, not the window, hold it.
pub fn a_name_change_waits_for_both_sends_to_a_hung_focus() {
    let (mut hung, tree) = tree_view();
    let (mut b, b_hwnd) = support::mockapp("hung-other", "uia");
    let uia = support::client();
    support::set_connection_timeout(uia.client(), CONNECTION_TIMEOUT);
    let focus = Handler::new();
    // SAFETY: live client, cache request and handler.
    unsafe {
        uia.client()
            .AddFocusChangedEventHandler(&support::cache(uia.client()), &focus.focus)
    }
    .expect("the desktop-wide focus handler");
    let names = Handler::new();
    support::names_on(
        uia.client(),
        &support::element(uia.client(), b_hwnd),
        &names,
    );

    let raised = hang_and_name_in_a_focus_event(&mut hung, tree);
    hung.expect_line(&support::root_object_request(TREE_VIEW), String::new);
    b.send("set-name btn1 Renamed");
    hung.expect_line(&support::hook_message(TREE_VIEW), String::new);
    names.heard_nothing_more();
    let (heard, at) = names.next();
    assert_eq!(heard, support::from(&b, "Renamed"));
    let held = at.duration_since(raised);
    assert!(
        held > CONNECTION_TIMEOUT,
        "the name change arrived {held:?} after the focus WinEvent"
    );
    hung.release();
    names.heard_nothing_more();
    hung.quit();
    b.quit();
}

/// With no focus registration in this process, another application's name
/// change, raised after the focus `WinEvent`, arrives while the tree view
/// still hangs, with the connection timeout longer than the test's wait for
/// it, so that a held one would not: UIA's focus tracking runs only in a
/// process that holds a focus registration.
pub fn a_name_change_is_not_held_in_a_process_with_no_focus_registration() {
    let (mut hung, tree) = tree_view();
    let (mut b, b_hwnd) = support::mockapp("hung-other", "uia");
    let uia = support::client();
    support::set_connection_timeout(uia.client(), 2 * common::WAIT_TIMEOUT);
    let names = Handler::new();
    support::names_on(
        uia.client(),
        &support::element(uia.client(), b_hwnd),
        &names,
    );

    hang_and_name_in_a_focus_event(&mut hung, tree);
    b.send("set-name btn1 Renamed");
    assert_eq!(names.next().0, support::from(&b, "Renamed"));
    hung.release();
    names.heard_nothing_more();
    hung.quit();
    b.quit();
}

/// Verbatim's workaround, that an outpost holds no UIA focus registration
/// (`docs/uia-bugs.md`, "Workarounds that worked"): a real outpost in this
/// process, following the focus in the second mockapp, hears its name
/// change by UIA's own event, and reports it, while the tree view still
/// hangs, named in a focus `WinEvent`, with the connection timeout longer
/// than the test's wait for the event, so that a held one would not be
/// heard.
pub fn an_outposts_uia_events_are_not_held_behind_a_hung_focus() {
    let (mut hung, tree) = tree_view();
    let (mut b, b_hwnd) = support::mockapp("hung-outpost", "uia");
    let uia = support::client();
    support::set_connection_timeout(uia.client(), 2 * common::WAIT_TIMEOUT);
    let outpost = OutpostUnderTest::new(&b);
    b.send("set-focus btn1");
    let button = common::listener_focus(&uia, b_hwnd.0 as isize);
    let focus = outpost.uia_focus(&button).node;

    hang_and_name_in_a_focus_event(&mut hung, tree);
    b.send("set-name btn1 Renamed");
    outpost.heard(&[
        Heard::UiaProperty(UIA_NamePropertyId.0),
        Heard::Msaa(WinEventKind::NameChange),
    ]);
    hung.release();
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::PropertyChanged {
                    node_id,
                    change: PropertyChange::Name(name),
                    child_count: None,
                },
            ..
        } => assert_eq!((node_id, name.as_deref()), (focus.id, Some("Renamed"))),
        other => panic!("the outpost said {other:?}, not the focus's new name"),
    }
    outpost.settled();
    drop(outpost);
    hung.quit();
    b.quit();
}

/// A server-side provider held while raising its own focus event holds
/// only its own events (E-delivery, in "Not affected"): with mockapp A's
/// provider held in the call UIA makes into it as A raises its focus,
/// B's focus event reaches this process, and A's arrives once A is
/// released.
pub fn a_provider_held_raising_its_event_holds_only_its_own_events() {
    let (mut a, a_hwnd) = support::mockapp("held-raise-a", "uia");
    let (mut b, b_hwnd) = support::mockapp("held-raise-b", "uia");
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

    a.hold();
    a.write("focus btn1");
    assert_eq!(a.held(), "ProviderOptions");
    b.send("focus btn1");
    assert_eq!(on_b.next().0, support::from(&b, "Original Name"));
    on_a.heard_nothing_more();
    a.release();
    a.expect_line("applied", String::new);
    assert_eq!(on_a.next().0, support::from(&a, "Original Name"));
    on_a.heard_nothing_more();
    on_b.heard_nothing_more();
    a.quit();
    b.quit();
}

/// The cost of a short connection timeout (`h6`): with it at 500 ms, an
/// MSAA mockapp's focus raised from inside a 1.5-second stall of its window
/// thread is never delivered to a desktop-wide focus handler, which hears a
/// second MSAA mockapp's focus, raised after the stall, first. UIA's focus
/// tracking handles the two focus `WinEvent`s in turn, so a late delivery
/// of the first would come before it; the second is another application's,
/// so UIA cannot take the first for it.
pub fn a_focus_raised_while_its_window_is_busy_past_the_connection_timeout_is_dropped() {
    let (mut busy, _) = support::mockapp("hung-dropped", "msaa");
    let (mut other, _) = support::mockapp("hung-dropped-other", "msaa");
    let uia = support::client();
    support::set_connection_timeout(uia.client(), CONNECTION_TIMEOUT);
    let focus = Handler::new();
    // SAFETY: live client, cache request and handler.
    unsafe {
        uia.client()
            .AddFocusChangedEventHandler(&support::cache(uia.client()), &focus.focus)
    }
    .expect("the desktop-wide focus handler");

    let stall = Duration::from_millis(1_500);
    busy.write(&format!("stall {} 0 focus btn1", stall.as_millis()));
    busy.expect_line("stall started", String::new);
    busy.expect_line("applied", String::new);
    let _ = busy.stall_ended(stall);
    other.send("focus btn1");
    assert_eq!(focus.next().0, support::from(&other, "Original Name"));
    focus.heard_nothing_more();
    busy.quit();
    other.quit();
}
