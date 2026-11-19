//! MSAA events from mockapp's scripted provider, as the focus listener
//! tells Core of them and Core asks a real outpost in this process
//! (`common::core_driver`): what NVDA's base handlers speak for a change
//! on the focus, and that a change on any other object says nothing
//! (`docs/nvda/events.md`, "The focus gate"), a tree view item's logical
//! parent included: NVDA's ancestors are those reached through
//! `accParent`. A state change on such an ancestor with no address of its
//! own is not spoken either, as NVDA does not speak it: nothing NVDA does
//! meets that ancestor again from an event.
//!
//! A foreground event whose window is not yet in front says nothing, as
//! NVDA drops it: the window is announced from the focus's ancestry
//! instead. One whose window is in front is spoken at once, whether or
//! not the window has been shown yet.
//!
//! mockapp's focus moves with `set-focus`, which raises no event; the
//! test tells Core of the focus itself.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::{MsaaIds, Outcome, Reply, Request};
use verbatim_model::{Earcon, QueryKind, Role, SegmentContent, State, WindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;

use common::core_driver::{CoreDriver, Said};
use common::outpost::OutpostUnderTest;

/// The counts fixture's second list item, by its index in mockapp's tree.
const ITEM_TWO: usize = 6;

/// The modal owner fixture's Remove button, by its index in mockapp's
/// tree.
const REMOVE: usize = 1;

/// The form fixture's button, by its index in mockapp's tree.
const FORM_BUTTON: usize = 1;

/// Nothing said.
const NOTHING: [Vec<SegmentContent>; 0] = [];

/// mockapp with `fixture` on its MSAA provider, titled from `prefix`, its
/// window, and Core driving an outpost watching it.
fn started(fixture: &str, prefix: &str) -> (common::MockApp, HWND, CoreDriver) {
    common::init_com();
    let title = common::unique_title(prefix);
    let app = common::spawn(fixture, "msaa", &title);
    let hwnd = common::find_window(&title);
    let core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    (app, hwnd, core)
}

/// [`started`], the user having switched to mockapp from another
/// application ([`CoreDriver::switched_to`]).
fn switched_to(fixture: &str, prefix: &str) -> (common::MockApp, HWND, CoreDriver) {
    common::init_com();
    let title = common::unique_title(prefix);
    let app = common::spawn(fixture, "msaa", &title);
    let hwnd = common::find_window(&title);
    let core = CoreDriver::switched_to(&app, OutpostUnderTest::new(&app));
    (app, hwnd, core)
}

/// mockapp with `fixture` on its MSAA provider, its window topmost and not
/// the foreground window, as a popup menu's is: the foreground window is
/// another top-level window of mockapp's (`thread-window`), as a popup
/// menu's application's main window is. Core drives an outpost watching
/// it.
fn started_in_the_background(fixture: &str, prefix: &str) -> (common::MockApp, HWND, CoreDriver) {
    common::init_com();
    let title = common::unique_title(prefix);
    let mut app = common::spawn(fixture, "msaa", &title);
    let hwnd = common::find_window(&title);
    common::make_topmost(hwnd);
    let front = app.thread_window();
    let outpost = OutpostUnderTest::new(&app);
    outpost.set_foreground(front);
    let core = CoreDriver::new(&app, outpost);
    (app, hwnd, core)
}

/// Core's focus on the modal owner fixture's Remove button, its client
/// area identified by its window.
fn on_remove(app: &mut common::MockApp, hwnd: HWND, core: &mut CoreDriver) {
    app.send("client-identity");
    app.send("set-focus remove");
    let _ = core.msaa_focus(hwnd, REMOVE);
}

/// A description change on the focus speaks the new description; a state
/// change on the group beside the focus's list, neither the focus nor an
/// ancestor, is not even reported, as the description change after it is
/// the next thing the outpost says.
fn a_description_change_on_the_focus_is_spoken() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-change-events");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    app.send("set-focus item2");
    assert_eq!(
        core.msaa_focus(hwnd, ITEM_TWO).spoken(),
        [
            vec![
                SegmentContent::Label("Options".to_owned()),
                SegmentContent::Role(verbatim_model::Role::List)
            ],
            vec![
                SegmentContent::Label("Two".to_owned()),
                SegmentContent::NegatedState(State::Selected)
            ],
        ]
    );

    assert_eq!(
        core.raise(
            &mut app,
            &["set-description item2 Second choice"],
            &[WinEventKind::DescriptionChange],
            1
        )
        .spoken(),
        [vec![SegmentContent::Description(
            "Second choice".to_owned()
        )]]
    );

    assert_eq!(
        core.raise(
            &mut app,
            &[
                "set-states group expanded",
                "set-description item2 Third choice"
            ],
            &[WinEventKind::StateChange, WinEventKind::DescriptionChange],
            2
        )
        .spoken(),
        [vec![SegmentContent::Description("Third choice".to_owned())]],
        "the group's change says nothing"
    );
    drop(core);
    app.quit();
}

/// A modal dialog's owner disabled as the dialog opens says nothing: a
/// state change on the foreground window the focus is in, an ancestor at
/// its own address, the window's client area, is not spoken, as NVDA,
/// checked live, never speaks it (`docs/parity/focus-announcement.md`, "A
/// top-level window's state change"). The window is the foreground as
/// the focus enters it, and is disabled while the focus is still on its
/// Remove button; the description change on the button after it is
/// spoken.
fn disabling_the_focus_window_is_not_spoken() {
    let (mut app, hwnd, mut core) = started("modal_owner.json", "mockapp-msaa-modal-owner");
    on_remove(&mut app, hwnd, &mut core);
    assert_eq!(
        core.raise(
            &mut app,
            &["disable-client", "set-description remove Removes the theme"],
            &[WinEventKind::StateChange, WinEventKind::DescriptionChange],
            2
        )
        .spoken(),
        [vec![SegmentContent::Description(
            "Removes the theme".to_owned()
        )]],
        "the window's change says nothing"
    );
    drop(core);
    app.quit();
}

/// The same owner disabled after a foreground event of its window, which
/// says nothing since the focus is already inside: Core, whose foreground
/// object the window is, says nothing of its change either, as NVDA's
/// event on the window resolves to its foreground object, never to the
/// ancestor.
fn disabling_the_focus_window_after_its_refused_foreground_report_is_not_spoken() {
    let (mut app, hwnd, mut core) = started("modal_owner.json", "mockapp-msaa-modal-owner-refused");
    on_remove(&mut app, hwnd, &mut core);
    assert_eq!(
        core.foreground(hwnd).spoken(),
        NOTHING,
        "the focus is already in the window"
    );
    assert_eq!(
        core.raise(
            &mut app,
            &["disable-client"],
            &[WinEventKind::StateChange],
            1
        )
        .spoken(),
        NOTHING
    );
    drop(core);
    app.quit();
}

/// A top-level window that was not the foreground as the focus entered
/// it, as a popup menu's never is, keeps its place among the focus's
/// ancestors, as in NVDA, whose foreground object is another window's: its
/// state change is spoken.
fn disabling_a_window_not_in_the_foreground_is_spoken() {
    let (mut app, hwnd, mut core) =
        started_in_the_background("modal_owner.json", "mockapp-msaa-background-owner");
    on_remove(&mut app, hwnd, &mut core);
    assert_eq!(
        core.raise(
            &mut app,
            &["disable-client"],
            &[WinEventKind::StateChange],
            1
        )
        .spoken(),
        [vec![SegmentContent::State(State::Disabled)]]
    );
    drop(core);
    app.quit();
}

/// The focus on the settings window's Remove button, in a window not the
/// foreground, so that the window's state change is spoken; the window is
/// disabled, and then the focus moves to node `to` (its fixture id and
/// index). Returns what Core does with the two, in order.
fn disable_then_move((to_id, to_index): (&str, usize)) -> Vec<Said> {
    let (mut app, hwnd, mut core) =
        started_in_the_background("modal_owner.json", "mockapp-msaa-change-before-focus");
    on_remove(&mut app, hwnd, &mut core);
    let mut said = core
        .raise(
            &mut app,
            &["disable-client"],
            &[WinEventKind::StateChange],
            1,
        )
        .said;
    app.send(&format!("set-focus {to_id}"));
    said.extend(core.msaa_focus(hwnd, to_index).said);
    drop(core);
    app.quit();
    said
}

/// The window the focus is in disabled, and the focus moving within it:
/// NVDA handles the state change first, judged against the focus it was
/// observed under, and speaks "unavailable", which the focus change
/// within the window does not cut off, and then the new focus.
fn a_change_before_a_focus_within_the_window_is_heard_first() {
    assert_eq!(
        disable_then_move(("close", 2)),
        [
            Said::Speech(vec![SegmentContent::State(State::Disabled)]),
            Said::Speech(vec![
                SegmentContent::Label("Close".to_owned()),
                SegmentContent::Role(Role::Button),
            ]),
        ]
    );
}

/// The window the focus is in disabled, and the focus moving into a menu:
/// "unavailable" is spoken and then cut off as the menu is entered, as
/// NVDA's entering a menu cancels speech.
fn a_change_before_a_focus_into_a_menu_is_cut_off() {
    assert_eq!(
        disable_then_move(("copy", 4)),
        [
            Said::Speech(vec![SegmentContent::State(State::Disabled)]),
            Said::Stop,
            Said::Speech(vec![SegmentContent::Label("Copy".to_owned())]),
        ]
    );
}

/// A state change on a windowless ancestor of the focus, one reached
/// through `accParent` with no address of its own, is not spoken, as NVDA
/// does not speak it: NVDA meets an event's object again only at an
/// address an earlier object was created for, which such an ancestor never
/// had, and its speech gate asks whether the object is the very ancestor it
/// holds, never whether it is like it (`docs/parity/focus-announcement.md`,
/// "Focus announcement content and property order"). The focused
/// list item's list becomes unavailable and says nothing; the description
/// change on the focus after it is spoken.
fn a_state_change_on_a_windowless_ancestor_is_not_spoken() {
    let (mut app, hwnd, mut core) = started("counts.json", "mockapp-msaa-acc-parent-state");
    app.send("set-focus item2");
    let _ = core.msaa_focus(hwnd, ITEM_TWO);
    assert_eq!(
        core.raise(
            &mut app,
            &[
                "set-states list focusable disabled",
                "set-description item2 Second choice"
            ],
            &[WinEventKind::StateChange, WinEventKind::DescriptionChange],
            2
        )
        .spoken(),
        [vec![SegmentContent::Description(
            "Second choice".to_owned()
        )]],
        "the list's change says nothing"
    );
    drop(core);
    app.quit();
}

/// Collapsing the item the focused tree item is in says nothing of the
/// collapsed item until it has the focus. It is the focused item's logical
/// parent, read through the control's messages, not an ancestor NVDA has:
/// NVDA's ancestors are those reached through `accParent`, and a tree view
/// item's is the control, so the control moving its selection to the
/// collapsed item says nothing. The control's focus on it says its
/// announcement, collapsed; the item's state change, raised after it, is
/// then the focus's own and says nothing new, the collapsed state having
/// been spoken with the focus.
fn collapsing_a_tree_items_parent_says_nothing_until_its_focus() {
    use windows::Win32::UI::Controls::{TVE_COLLAPSE, TVM_EXPAND};
    common::init_com();
    let title = common::unique_title("mockapp-msaa-ancestor-state");
    let app = common::spawn("tree_view.json", "msaa", &title);
    let tree = common::tree_view::tree_view(common::find_window(&title));
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    let disks = common::tree_view::child_id(tree, "Disks");
    let _ = core.msaa_event(EventKind::MsaaFocus, tree, (OBJID_CLIENT.0, disks));
    let hardware = common::tree_view::item(tree, "Hardware");
    // The control's focus on the parent comes first, and the parent's
    // state change last; the focus, in the same top-level window, does not
    // cut speech.
    let collapse = common::core_driver::win_events(
        app.pid(),
        || {
            common::tree_view::send(tree, TVM_EXPAND, TVE_COLLAPSE.0 as usize, hardware);
        },
        &[WinEventKind::Focus, WinEventKind::StateChange],
        2,
    );
    assert_eq!(
        collapse.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [WinEventKind::Focus, WinEventKind::StateChange]
    );
    assert_eq!(
        core.win_event(&collapse[0]).said,
        [Said::Speech(vec![
            SegmentContent::Level(0),
            SegmentContent::Label("Hardware".to_owned()),
            SegmentContent::State(State::Mixed),
            SegmentContent::State(State::Collapsed),
            SegmentContent::Position {
                position: 1,
                set_size: Some(3)
            },
        ])]
    );
    assert_eq!(core.win_event(&collapse[1]).said, []);
    drop(core);
    app.quit();
}

/// A progress bar that is not the focus indicates each change of its
/// percentage by a tone at once, as NVDA's progress bar behavior beeps,
/// when it moves by at least one percent; a value that is not a number is
/// an ordinary value change, silent off the focus.
fn a_progress_bar_off_the_focus_indicates_its_percentage() {
    /// mockapp's scripted Inbox item, by its index in its tree.
    const INBOX: usize = 2;
    let (mut app, hwnd, mut core) = started("tree_view.json", "mockapp-msaa-progress");
    let _ = core.msaa_focus(hwnd, INBOX);
    let mut indicated = |value: &str| {
        core.raise(
            &mut app,
            &[&format!("set-value copying {value}")],
            &[WinEventKind::ValueChange],
            1,
        )
        .said
    };
    assert_eq!(indicated("40"), [Said::Earcon(Earcon::Progress(40))]);
    assert_eq!(indicated("40.5"), [], "less than a percent from the last");
    assert_eq!(indicated("55%"), [Said::Earcon(Earcon::Progress(55))]);
    assert_eq!(indicated("Paused"), [], "not a number, and not the focus");
    drop(core);
    app.quit();
}

/// An item selected in a list beside the focus, a button, is neither the
/// focus nor an ancestor, nor inside an object the focus controls, and
/// says nothing.
fn a_selection_beside_the_focus_says_nothing() {
    /// The counts fixture's First button, by its index in mockapp's tree.
    const FIRST: usize = 2;
    let (mut app, hwnd, mut core) = started("counts.json", "mockapp-msaa-selection-read");
    app.send("set-focus first");
    let _ = core.msaa_focus(hwnd, FIRST);
    assert_eq!(
        core.raise(&mut app, &["select item1"], &[WinEventKind::Selection], 1)
            .spoken(),
        NOTHING
    );
    drop(core);
    app.quit();
}

/// mockapp's window of the tooltip class titled `name`, which the fixture
/// hosts a node in (`"window_class": "tooltips_class32"`).
fn tip_window(app: &common::MockApp, name: &str) -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let title: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut after = None;
    loop {
        // SAFETY: a local search of the desktop's top-level windows; the
        // strings outlive the call.
        let found = unsafe {
            FindWindowExW(
                None,
                after,
                windows::Win32::UI::Controls::TOOLTIPS_CLASSW,
                windows::core::PCWSTR(title.as_ptr()),
            )
        }
        .expect("mockapp made the tooltip window");
        if verbatim_win32::window::window_owner(found.0 as isize).1 == app.pid() {
            return found;
        }
        after = Some(found);
    }
}

/// A tooltip window's show event is spoken as NVDA's notification
/// behavior speaks a help balloon, which NVDA reports by default; an
/// ordinary tooltip is not, as NVDA does not report tooltips by default.
/// Both are windows of the real tooltip class, `tooltips_class32`, which
/// mockapp makes from comctl32's own class, checked here.
fn a_help_balloon_shown_is_spoken() {
    let (app, _, mut core) = started("tree_view.json", "mockapp-msaa-help-balloon");
    let balloon = tip_window(&app, "Updates are ready");
    let tip = tip_window(&app, "Close");
    assert_eq!(
        (
            verbatim_win32::window::window_class_name(balloon.0 as isize),
            verbatim_win32::window::window_class_name(tip.0 as isize)
        ),
        ("tooltips_class32".to_owned(), "tooltips_class32".to_owned())
    );
    assert_eq!(
        core.msaa_event(EventKind::Show, tip, (OBJID_CLIENT.0, 0))
            .spoken(),
        NOTHING
    );
    assert_eq!(
        core.msaa_event(EventKind::Show, balloon, (OBJID_CLIENT.0, 0))
            .spoken(),
        [vec![
            SegmentContent::Label("Updates are ready".to_owned()),
            SegmentContent::Role(Role::HelpBalloon)
        ]]
    );
    drop(core);
    app.quit();
}

/// A show event from a native UIA window says nothing, as NVDA drops every
/// `WinEvent` of a window it reads through UIA, though the window's client
/// object answers MSAA as a help balloon, which is spoken from a window
/// read through MSAA ([`a_help_balloon_shown_is_spoken`]).
fn a_show_from_a_native_uia_window_says_nothing() {
    common::init_com();
    let title = common::unique_title("mockapp-uia-show");
    let app = common::spawn("uia_help_balloon_client.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    assert_eq!(
        core.msaa_event(EventKind::Show, hwnd, (OBJID_CLIENT.0, 0))
            .spoken(),
        NOTHING
    );
    drop(core);
    app.quit();
}

/// The child window of a `ToastChildWindowClass` parent, as mockapp makes
/// it (`child-window`).
fn toast_child(app: &mut common::MockApp, title: &str) -> HWND {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let hwnd = common::find_window(title);
    app.send("child-window");
    // SAFETY: a local search of the window's children.
    unsafe { FindWindowExW(Some(hwnd), None, None, None) }.expect("mockapp made the child window")
}

/// An MSAA alert from a child of a `ToastChildWindowClass` parent is
/// spoken, from any application, as a toast's alert is, when its window is
/// read through MSAA. NVDA speaks an MSAA alert only when its object's role
/// is alert (`NVDAObjects/IAccessible/__init__.py` 2025 to 2066), so
/// `toast_host.json`'s object has the alert role, and speaks it as a focus,
/// role included (`speech.speakObject` with reason focus); whether a real
/// toast's object has the alert role awaits a recording of one.
fn an_alert_from_a_toast_window_is_reported() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-toast");
    let mut app = common::spawn("toast_host.json", "msaa", &title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    let child = toast_child(&mut app, &title);
    assert_eq!(
        core.msaa_event(EventKind::Alert, child, (OBJID_CLIENT.0, 0))
            .spoken(),
        [vec![
            SegmentContent::Label("Download complete".to_owned()),
            SegmentContent::Role(Role::Alert)
        ]]
    );
    drop(core);
    app.quit();
}

/// The same alert from a native UIA window, whose client object answers
/// MSAA as the one above does, says nothing.
fn an_alert_from_a_native_uia_toast_window_says_nothing() {
    common::init_com();
    let title = common::unique_title("mockapp-uia-toast");
    let mut app = common::spawn("toast_host.json", "uia", &title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    let child = toast_child(&mut app, &title);
    assert_eq!(
        core.msaa_event(EventKind::Alert, child, (OBJID_CLIENT.0, 0))
            .spoken(),
        NOTHING
    );
    drop(core);
    app.quit();
}

/// A window of mockapp's destroyed, its destroy told as the listener sends
/// it: Core passes it to mockapp's outpost, whose store then holds no MSAA
/// node of the window, the focus Core still holds among them, so a reused
/// handle inherits none (Table 14). The focus is "Save" in a child window
/// of the "Save As" dialog (`child-window`), which mockapp then destroys
/// (`destroy-child-window`).
fn a_destroyed_windows_msaa_nodes_leave_the_outposts_store() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-destroyed-window");
    let mut app = common::spawn("save_as.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    app.send("child-window");
    // SAFETY: a local search of the dialog's children.
    let child = unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowExW(Some(hwnd), None, None, None)
    }
    .expect("mockapp made the child window");
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    let _ = core.msaa_focus(child, 1);
    let (_, focus) = core.state().focused().expect("Core committed the focus");
    let node = focus.id;
    assert_eq!(focus.name.as_deref(), Some("Save"));
    assert!(
        core.outpost.outpost.holds(node),
        "the outpost holds the focus"
    );
    app.send("destroy-child-window");
    assert_eq!(core.destroyed(child).spoken(), NOTHING);
    assert!(
        !core.outpost.outpost.holds(node),
        "the outpost forgot the destroyed window's node"
    );
    drop(core);
    app.quit();
}

/// A foreground event whose window is not the foreground window says
/// nothing and is not kept for a focus inside the window: once the window
/// is in front, its focus is spoken alone, as NVDA, having dropped the
/// event, meets the window only in the focus's ancestry.
fn a_foreground_event_before_its_window_is_in_front_is_dropped() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-early-foreground");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    outpost.set_foreground(common::another_applications_window());
    let mut core = CoreDriver::new(&app, outpost);
    assert_eq!(core.foreground(hwnd).spoken(), NOTHING);

    core.outpost.set_foreground(hwnd);
    let _ = core.foreground_report();
    app.send("set-focus item2");
    assert_eq!(
        core.msaa_focus(hwnd, ITEM_TWO).spoken(),
        [
            vec![
                SegmentContent::Label("Options".to_owned()),
                SegmentContent::Role(Role::List)
            ],
            vec![
                SegmentContent::Label("Two".to_owned()),
                SegmentContent::NegatedState(State::Selected)
            ],
        ]
    );
    drop(core);
    app.quit();
}

/// A foreground event whose window is in front is spoken at once, as a
/// focus on the window, though the window has not been shown: NVDA tests
/// only that the window is the foreground window. mockapp's window is never
/// shown in these tests.
fn a_foreground_event_for_a_window_in_front_not_yet_shown_is_reported() {
    let (app, hwnd, mut core) = switched_to("counts.json", "mockapp-msaa-hidden-foreground");
    // SAFETY: IsWindowVisible tolerates any handle.
    let shown = unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) };
    assert!(!shown.as_bool(), "mockapp's window is hidden");
    assert_eq!(
        core.foreground(hwnd).spoken(),
        [vec![
            SegmentContent::Label("Mockapp Counts Fixture".to_owned()),
            SegmentContent::Role(Role::Window),
        ]]
    );
    drop(core);
    app.quit();
}

/// A second foreground event of a window whose foreground Core has just
/// spoken says nothing, Core's focus being that window.
fn a_second_foreground_change_of_a_reported_window_is_reported_and_silent() {
    let (app, hwnd, mut core) = switched_to("counts.json", "mockapp-msaa-second-foreground");
    assert_eq!(
        core.foreground(hwnd).spoken(),
        [vec![
            SegmentContent::Label("Mockapp Counts Fixture".to_owned()),
            SegmentContent::Role(Role::Window),
        ]]
    );
    assert_eq!(core.foreground(hwnd).spoken(), NOTHING);
    drop(core);
    app.quit();
}

/// With Core's focus a list item, its window's foreground event says
/// nothing, Core keeping its focus, and the item's state change is spoken,
/// "checked".
fn a_change_of_the_focus_after_its_window_s_refused_foreground_report_is_spoken() {
    let (mut app, hwnd, mut core) = started("counts.json", "mockapp-msaa-change-after-refused");
    app.send("set-focus item2");
    let _ = core.msaa_focus(hwnd, ITEM_TWO);
    assert_eq!(
        core.foreground(hwnd).spoken(),
        NOTHING,
        "the focus is already in the window"
    );
    assert_eq!(
        core.raise(
            &mut app,
            &["set-states item2 focusable focused checked"],
            &[WinEventKind::StateChange],
            1
        )
        .spoken(),
        [vec![SegmentContent::State(State::Checked)]]
    );
    drop(core);
    app.quit();
}

/// With Core's focus mockapp's window, taken from its foreground event,
/// the window's client area renamed is that focus: Core says the new name.
fn a_name_change_of_the_window_core_took_is_spoken() {
    let (mut app, hwnd, mut core) = switched_to("modal_owner.json", "mockapp-msaa-window-renamed");
    assert_eq!(
        core.foreground(hwnd).spoken(),
        [vec![
            SegmentContent::Label("Settings".to_owned()),
            SegmentContent::Role(Role::Dialog),
        ]]
    );
    assert_eq!(
        core.raise(
            &mut app,
            &["client-name Options"],
            &[WinEventKind::NameChange],
            1
        )
        .spoken(),
        [vec![SegmentContent::Label("Options".to_owned())]]
    );
    drop(core);
    app.quit();
}

/// A name change of a client area whose role is "window", the focus, is
/// spoken: NVDA directs a change event to the focus object it equals
/// (`IAccessible._isEqual`, `processGenericWinEvent`). mockapp's client
/// area (`window_role_client.json`), focused at `OBJID_CLIENT`, is renamed
/// (`client-name`), and Core says the new name.
fn a_name_change_of_a_client_with_the_window_role_is_spoken() {
    let (mut app, hwnd, mut core) = started(
        "window_role_client.json",
        "mockapp-msaa-window-role-renamed",
    );
    let _ = core.msaa_event(EventKind::MsaaFocus, hwnd, (OBJID_CLIENT.0, 0));
    assert_eq!(
        core.raise(
            &mut app,
            &["client-name Options"],
            &[WinEventKind::NameChange],
            1
        )
        .spoken(),
        [vec![SegmentContent::Label("Options".to_owned())]]
    );
    drop(core);
    app.quit();
}

/// Object navigation still reaches a window's own window object above its
/// client area and goes on past it, the purpose of the IA2 reader's key
/// for a window object (`OBJID_WINDOW`), which a role-window client shares:
/// from the client area of mockapp's real push button "Apply"
/// (`group_box.json`), the parent is the button's window object, another
/// node of the window role, and its parent the host window's client area,
/// rather than the button's client area once more.
fn a_parent_window_object_is_still_reached_by_navigation() {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-parent-window-object");
    let app = common::spawn("group_box.json", "msaa", &title);
    let host = common::find_window(&title);
    // SAFETY: a local search of the host window's children.
    let apply = unsafe {
        FindWindowExW(
            Some(host),
            None,
            windows::core::w!("Button"),
            windows::core::w!("Apply"),
        )
    }
    .expect("mockapp made the button");
    let outpost = OutpostUnderTest::new(&app);
    let button = outpost
        .msaa_focus_answer(MsaaIds {
            window: WindowHandle(apply.0 as u64),
            object: OBJID_CLIENT.0,
            child: 0,
        })
        .node();
    let parent = |node| {
        let answer = outpost.requested(
            apply,
            Request::Navigate {
                node,
                direction: QueryKind::Parent,
                set: common::outpost::focus_set(),
            },
        );
        match answer.outcome {
            Outcome::Result(Reply::Navigated(Some(read))) => read,
            other => panic!("the step answered {other:?}"),
        }
    };
    let window = parent(button);
    let window_node = window.node.expect("the window object's node");
    let above = parent(window_node);
    let above_node = above.node.expect("the client area's node");
    assert_eq!(
        (
            common::outpost::value(&window.values, PropertyId::Role),
            window_node != button,
            common::outpost::value(&above.values, PropertyId::Name),
            above_node != window_node && above_node != button,
        ),
        (
            Some(&PropertyValue::Role(Role::Window)),
            true,
            Some(&PropertyValue::Text("Mockapp Group Box Fixture".to_owned())),
            true,
        ),
        "the button's window object and the host's client area above it"
    );
    drop(outpost);
    app.quit();
}

/// A top-level window's client area reached by `accParent` from a
/// windowless object inside it, with no address of its own, is acquired at
/// its window's client address and read there, as NVDA reads the
/// foreground object (`objectInForeground`) when the focus enters a new
/// top-level window, so its title is said: the "OK" button of mockapp's
/// form (`form.json`), whose client area has the client role and names no
/// address, entered with no foreground event, says "<title>" once before
/// the focus.
fn a_top_level_client_reached_from_a_windowless_object_is_said_by_its_title() {
    let (app, hwnd, mut core) = switched_to("form.json", "mockapp-msaa-reached-client");
    assert_eq!(
        core.msaa_focus(hwnd, FORM_BUTTON).spoken(),
        [
            vec![SegmentContent::Label("Mockapp Form Fixture".to_owned())],
            vec![
                SegmentContent::Label("OK".to_owned()),
                SegmentContent::Role(Role::Button),
            ],
        ]
    );
    drop(core);
    app.quit();
}

fn main() {
    harness::run_isolated(&[
        (
            "a_description_change_on_the_focus_is_spoken",
            a_description_change_on_the_focus_is_spoken,
        ),
        (
            "disabling_the_focus_window_is_not_spoken",
            disabling_the_focus_window_is_not_spoken,
        ),
        (
            "disabling_the_focus_window_after_its_refused_foreground_report_is_not_spoken",
            disabling_the_focus_window_after_its_refused_foreground_report_is_not_spoken,
        ),
        (
            "disabling_a_window_not_in_the_foreground_is_spoken",
            disabling_a_window_not_in_the_foreground_is_spoken,
        ),
        (
            "a_change_before_a_focus_within_the_window_is_heard_first",
            a_change_before_a_focus_within_the_window_is_heard_first,
        ),
        (
            "a_change_before_a_focus_into_a_menu_is_cut_off",
            a_change_before_a_focus_into_a_menu_is_cut_off,
        ),
        (
            "a_state_change_on_a_windowless_ancestor_is_not_spoken",
            a_state_change_on_a_windowless_ancestor_is_not_spoken,
        ),
        (
            "collapsing_a_tree_items_parent_says_nothing_until_its_focus",
            collapsing_a_tree_items_parent_says_nothing_until_its_focus,
        ),
        (
            "a_progress_bar_off_the_focus_indicates_its_percentage",
            a_progress_bar_off_the_focus_indicates_its_percentage,
        ),
        (
            "a_selection_beside_the_focus_says_nothing",
            a_selection_beside_the_focus_says_nothing,
        ),
        (
            "a_help_balloon_shown_is_spoken",
            a_help_balloon_shown_is_spoken,
        ),
        (
            "a_show_from_a_native_uia_window_says_nothing",
            a_show_from_a_native_uia_window_says_nothing,
        ),
        (
            "an_alert_from_a_toast_window_is_reported",
            an_alert_from_a_toast_window_is_reported,
        ),
        (
            "an_alert_from_a_native_uia_toast_window_says_nothing",
            an_alert_from_a_native_uia_toast_window_says_nothing,
        ),
        (
            "a_foreground_event_before_its_window_is_in_front_is_dropped",
            a_foreground_event_before_its_window_is_in_front_is_dropped,
        ),
        (
            "a_foreground_event_for_a_window_in_front_not_yet_shown_is_reported",
            a_foreground_event_for_a_window_in_front_not_yet_shown_is_reported,
        ),
        (
            "a_second_foreground_change_of_a_reported_window_is_reported_and_silent",
            a_second_foreground_change_of_a_reported_window_is_reported_and_silent,
        ),
        (
            "a_change_of_the_focus_after_its_window_s_refused_foreground_report_is_spoken",
            a_change_of_the_focus_after_its_window_s_refused_foreground_report_is_spoken,
        ),
        (
            "a_name_change_of_the_window_core_took_is_spoken",
            a_name_change_of_the_window_core_took_is_spoken,
        ),
        (
            "a_name_change_of_a_client_with_the_window_role_is_spoken",
            a_name_change_of_a_client_with_the_window_role_is_spoken,
        ),
        (
            "a_parent_window_object_is_still_reached_by_navigation",
            a_parent_window_object_is_still_reached_by_navigation,
        ),
        (
            "a_top_level_client_reached_from_a_windowless_object_is_said_by_its_title",
            a_top_level_client_reached_from_a_windowless_object_is_said_by_its_title,
        ),
        (
            "a_destroyed_windows_msaa_nodes_leave_the_outposts_store",
            a_destroyed_windows_msaa_nodes_leave_the_outposts_store,
        ),
    ]);
}
