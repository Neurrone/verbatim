//! The review cursor on objects that no longer exist, through a real
//! outpost (`docs/parity/review.md`, "Dead objects"): an element the
//! navigator keeps answers gone once mockapp has disconnected it, for
//! navigation, a live read and a text read, the parent of a dead element is
//! the parent of its window, and a node Core holds as the navigator keeps
//! its element after the focus moves on.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::property::PropertyId;
use verbatim_model::request::{Outcome, Reply, Request};
use verbatim_model::{CallCounts, NodeId, QueryKind, Role, TextPoint, TextUnit};
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;

use common::outpost::{OutpostUnderTest, focus_set, name_in, role_in, value};

/// Reads `node_id`, in `window`, live through `outpost`, as a review
/// command does: its name and the calls it made, the answer being the next
/// thing the outpost says.
fn read_node(
    outpost: &OutpostUnderTest,
    window: HWND,
    node_id: NodeId,
) -> (Option<String>, CallCounts) {
    let (outcome, calls) = outpost.asked(
        window,
        Request::Properties {
            node: node_id,
            set: focus_set(),
        },
    );
    let Outcome::Result(Reply::Properties(values)) = outcome else {
        panic!("the outpost answered {outcome:?}, not the node");
    };
    outpost.settled();
    (name_in(&values).map(str::to_owned), calls)
}

/// Asks `request` about a node of `window`, which must answer that it is
/// not there.
fn not_there(outpost: &OutpostUnderTest, window: HWND, request: Request) {
    let (outcome, _) = outpost.asked(window, request);
    assert_eq!(outcome, Outcome::NotThere);
    outpost.settled();
}

/// Focuses mockapp's text box "Notes" and reports it to `outpost`; its
/// node.
fn focus_notes(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    uia: &Uia,
    window: isize,
) -> NodeId {
    app.send("set-focus notes");
    let focused = common::listener_focus(uia, window);
    outpost.uia_focus_answer(&focused).node()
}

/// The navigator on an element that goes away: next, previous and first
/// child answer gone, as a live read of it does; and parent answers the
/// parent of its window.
fn a_dead_element_answers_gone_and_its_parent_is_its_windows_parent() {
    let title = common::unique_title("mockapp-dead-object");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let window = hwnd.0 as isize;
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    let notes = focus_notes(&mut app, &outpost, &uia, window);
    let (group, _) = outpost.navigate(hwnd, notes, QueryKind::Parent);
    assert_eq!(name_in(&group.values), Some("Inner group"));
    let group = group.node.expect("the outpost mints the node");
    outpost.settled();

    app.send("disconnect group2");
    for direction in [
        QueryKind::NextSibling,
        QueryKind::PreviousSibling,
        QueryKind::FirstChild,
    ] {
        not_there(
            &outpost,
            hwnd,
            Request::Navigate {
                node: group,
                direction,
                set: focus_set(),
            },
        );
    }
    not_there(
        &outpost,
        hwnd,
        Request::Properties {
            node: group,
            set: focus_set(),
        },
    );

    let (parent, _) = outpost.navigate(hwnd, group, QueryKind::Parent);
    assert_eq!(
        (role_in(&parent.values), parent.window.desktop),
        (Role::Window, true),
        "the parent of the dead element's window, the desktop for Core to name"
    );

    drop(outpost);
    app.quit();
}

/// mockapp's window of `dead_island.json`, titled after `label`, and its UIA
/// child window, holding the windowless "Inner" and its focused button "OK",
/// the host holding the windowless button "Apply".
fn dead_island(label: &str) -> (common::MockApp, HWND, HWND) {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    let title = common::unique_title(label);
    let app = common::spawn("dead_island.json", "uia", &title);
    let host = common::find_window(&title);
    // SAFETY: a local search of the host window's children.
    let island = unsafe {
        FindWindowExW(
            Some(host),
            None,
            windows::core::w!("VerbatimMockAppUiaWindow"),
            windows::core::PCWSTR::null(),
        )
    }
    .expect("mockapp made the UIA window");
    (app, host, island)
}

/// A windowless element's Parent, once it is dead, answers the parent of the
/// window it was found in, whatever the focus has done since: "Inner",
/// reached by navigation inside the child window, gives its host window's
/// own UIA object with its own role and name, the focus having moved to the
/// host.
fn a_dead_element_s_parent_is_its_own_window_s_parent_with_its_own_role() {
    let (mut app, host, island) = dead_island("mockapp-dead-island-navigated");
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    let focused = common::listener_focus(&uia, island.0 as isize);
    let ok = outpost.uia_focus_answer(&focused).node();
    let (inner, _) = outpost.navigate(island, ok, QueryKind::Parent);
    assert_eq!(name_in(&inner.values), Some("Inner"));
    let inner = inner.node.expect("the outpost mints the node");

    app.send("set-focus apply");
    let focused = common::listener_focus(&uia, host.0 as isize);
    let _ = outpost.uia_focus_answer(&focused);

    app.send("island disconnect inner");
    let (parent, _) = outpost.navigate(island, inner, QueryKind::Parent);
    assert_eq!(
        (
            value(&parent.values, PropertyId::RuntimeId).is_some(),
            role_in(&parent.values),
            name_in(&parent.values),
            parent.window.desktop
        ),
        (true, Role::Window, Some("Mockapp Dead Island"), false),
        "the host window's UI Automation element, not the focus window's parent"
    );

    drop(outpost);
    app.quit();
}

/// The window a focused windowless element was found in is the one its
/// Parent answers from, after it dies and the focus has moved into a child
/// window: "Apply", directly under the host, gives the desktop, the parent
/// of its own window, and not the host, the parent of the child window that
/// now has the focus.
fn a_dead_focus_keeps_the_window_it_was_found_in() {
    let (mut app, host, island) = dead_island("mockapp-dead-island-focused");
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    app.send("set-focus apply");
    let focused = common::listener_focus(&uia, host.0 as isize);
    let apply = outpost.uia_focus_answer(&focused).node();

    app.send("island set-focus ok");
    let focused = common::listener_focus(&uia, island.0 as isize);
    let _ = outpost.uia_focus_answer(&focused);

    app.send("disconnect apply");
    let (parent, _) = outpost.navigate(host, apply, QueryKind::Parent);
    assert_eq!(
        (role_in(&parent.values), parent.window.desktop),
        (Role::Window, true)
    );

    drop(outpost);
    app.quit();
}

/// Report Current Object's live read of a menu item reads its legacy state,
/// as its focus's and a navigation's do
/// (`verbatim_uia::map::wants_legacy_checked_state`): the read and the
/// legacy state, two calls, where a button's is one.
fn a_live_read_of_a_menu_item_reads_its_legacy_state() {
    let title = common::unique_title("mockapp-live-read-menu-item");
    let mut app = common::spawn("modal_owner.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    app.send("set-focus copy");
    let focused = common::listener_focus(&uia, hwnd.0 as isize);
    let copy = outpost.uia_focus_answer(&focused).node();
    // The element, found by the focus's route, is kept for the read below.
    let _ = read_node(&outpost, hwnd, copy);

    let (name, calls) = read_node(&outpost, hwnd, copy);
    assert_eq!(
        (name.as_deref(), calls.uia, calls.msaa),
        (Some("Copy"), 2, 0),
        "the read and the legacy state"
    );

    drop(outpost);
    app.quit();
}

/// A text read of a text object that has gone away says gone.
fn a_text_read_of_a_dead_object_says_gone() {
    let title = common::unique_title("mockapp-dead-text");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    app.send("set-focus doc");
    let focused = common::listener_focus(&uia, hwnd.0 as isize);
    let doc = outpost.uia_focus_answer(&focused).node();

    app.send("disconnect doc");
    let read = common::one_step(verbatim_model::batch::Op::ReadText {
        subject: verbatim_model::batch::Subject::Node(doc),
        at: verbatim_model::batch::Point::At(TextPoint::Start),
        movement: None,
        unit: TextUnit::Line,
        count: 1,
        details: verbatim_model::TextDetails::NONE,
    });
    let number = outpost.request_with(hwnd, verbatim_model::request::WindowVerdict::Uia, read);
    assert_eq!(
        outpost.answer(number).outcome,
        Outcome::Result(Reply::Steps(vec![
            verbatim_model::batch::StepOutcome::NotThere(verbatim_model::batch::Missing::Node(doc))
        ]))
    );
    outpost.settled();

    drop(outpost);
    app.quit();
}

/// The element of a node Core holds as the navigator stays kept after the
/// focus moves on, so the node still reads live, one call, instead of
/// answering gone with no route to it.
fn a_navigator_node_keeps_its_element_after_the_focus_moves_on() {
    let title = common::unique_title("mockapp-navigator-kept");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let window = hwnd.0 as isize;
    let outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    let notes = focus_notes(&mut app, &outpost, &uia, window);
    let (group, _) = outpost.navigate(hwnd, notes, QueryKind::Parent);
    let group = group.node.expect("the outpost mints the node");
    outpost.settled();

    app.send("set-focus apple");
    let focused = common::listener_focus(&uia, window);
    let _ = outpost.uia_focus_answer(&focused);

    let (name, calls) = read_node(&outpost, hwnd, group);
    assert_eq!(
        (name.as_deref(), calls),
        (
            Some("Inner group"),
            CallCounts {
                uia: 1,
                msaa: 0,
                window_messages: 0,
                connections: 0,
            }
        )
    );

    drop(outpost);
    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_dead_element_answers_gone_and_its_parent_is_its_windows_parent",
            a_dead_element_answers_gone_and_its_parent_is_its_windows_parent,
        ),
        (
            "a_dead_element_s_parent_is_its_own_window_s_parent_with_its_own_role",
            a_dead_element_s_parent_is_its_own_window_s_parent_with_its_own_role,
        ),
        (
            "a_dead_focus_keeps_the_window_it_was_found_in",
            a_dead_focus_keeps_the_window_it_was_found_in,
        ),
        (
            "a_live_read_of_a_menu_item_reads_its_legacy_state",
            a_live_read_of_a_menu_item_reads_its_legacy_state,
        ),
        (
            "a_text_read_of_a_dead_object_says_gone",
            a_text_read_of_a_dead_object_says_gone,
        ),
        (
            "a_navigator_node_keeps_its_element_after_the_focus_moves_on",
            a_navigator_node_keeps_its_element_after_the_focus_moves_on,
        ),
    ]);
}
