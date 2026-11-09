//! The review cursor on objects that no longer exist, through a real
//! outpost (`docs/parity/review.md`, "Dead objects"): an element the
//! navigator keeps answers gone once mockapp has disconnected it, for
//! navigation, a live read and a text read, the parent of a dead element is
//! the parent of its window, and a node Core holds as the navigator keeps
//! its element after the focus moves on.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::protocol::{OutpostToCore, Query, QueryOutcome, QueryResult};
use verbatim_model::{
    Backend, CallCounts, NodeId, QueryKind, Role, TextOp, TextPoint, TextRead, TextReply, TextUnit,
};
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;

use common::outpost::OutpostUnderTest;

/// Reads `node_id` live through `outpost`, as a review command does:
/// its name and the calls it made, the reply being the next thing the
/// outpost says.
fn read_node(outpost: &mut OutpostUnderTest, node_id: NodeId) -> (Option<String>, CallCounts) {
    let request_id = outpost.ask(Query::ReadNode { node_id });
    let answer = match outpost.next() {
        OutpostToCore::Reply {
            request_id: answered,
            outcome: QueryOutcome::Done(QueryResult::Node(node)),
            timing,
            ..
        } if answered == request_id => (node.name, timing.calls),
        other => panic!("the outpost said {other:?}, not the node"),
    };
    outpost.settled();
    answer
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
    outpost.uia_focus(&focused).node.id
}

/// The navigator on an element that goes away: next, previous and first
/// child answer gone, as a live read of it does; and parent answers the
/// parent of its window.
fn a_dead_element_answers_gone_and_its_parent_is_its_windows_parent() {
    let title = common::unique_title("mockapp-dead-object");
    let mut app = common::spawn("ancestry.json", "uia", &title);
    let window = common::find_window(&title).0 as isize;
    let mut outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    let notes = focus_notes(&mut app, &outpost, &uia, window);
    let (group, _) = outpost.navigate(notes, QueryKind::Parent);
    assert_eq!(group.name.as_deref(), Some("Inner group"));

    app.send("disconnect group2");
    for kind in [
        QueryKind::NextSibling,
        QueryKind::PreviousSibling,
        QueryKind::FirstChild,
    ] {
        outpost.gone(Query::Navigate {
            node_id: group.id,
            kind,
        });
    }
    outpost.gone(Query::ReadNode { node_id: group.id });

    let (parent, _) = outpost.navigate(group.id, QueryKind::Parent);
    assert_eq!(
        (parent.role, parent.details.desktop_window),
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
    let mut outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    let focused = common::listener_focus(&uia, island.0 as isize);
    let ok = outpost.uia_focus(&focused).node.id;
    let (inner, _) = outpost.navigate(ok, QueryKind::Parent);
    assert_eq!(inner.name.as_deref(), Some("Inner"));

    app.send("set-focus apply");
    let focused = common::listener_focus(&uia, host.0 as isize);
    let _ = outpost.uia_focus(&focused);

    app.send("island disconnect inner");
    let (parent, _) = outpost.navigate(inner.id, QueryKind::Parent);
    assert_eq!(
        (
            parent.backend,
            parent.role,
            parent.name.as_deref(),
            parent.details.desktop_window
        ),
        (
            Backend::Uia,
            Role::Window,
            Some("Mockapp Dead Island"),
            false
        ),
        "the host window's element, not the focus window's parent"
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
    let mut outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    app.send("set-focus apply");
    let focused = common::listener_focus(&uia, host.0 as isize);
    let apply = outpost.uia_focus(&focused).node.id;

    app.send("island set-focus ok");
    let focused = common::listener_focus(&uia, island.0 as isize);
    let _ = outpost.uia_focus(&focused);

    app.send("disconnect apply");
    let (parent, _) = outpost.navigate(apply, QueryKind::Parent);
    assert_eq!(
        (parent.role, parent.details.desktop_window),
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
    let window = common::find_window(&title).0 as isize;
    let mut outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    app.send("set-focus copy");
    let focused = common::listener_focus(&uia, window);
    let copy = outpost.uia_focus(&focused).node.id;
    // The element, found by the focus's route, is kept for the read below.
    let _ = read_node(&mut outpost, copy);

    let (name, calls) = read_node(&mut outpost, copy);
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
    let window = common::find_window(&title).0 as isize;
    let mut outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    app.send("set-focus doc");
    let focused = common::listener_focus(&uia, window);
    let doc = outpost.uia_focus(&focused).node.id;

    app.send("disconnect doc");
    let request = outpost.ask(Query::Text {
        node_id: doc,
        op: TextOp::Read(TextRead {
            at: TextPoint::Start,
            movement: None,
            unit: TextUnit::Line,
        }),
    });
    match outpost.next() {
        OutpostToCore::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Text(TextReply::Gone)),
            ..
        } if request_id == request => {}
        other => panic!("the outpost said {other:?}, not that the text is gone"),
    }
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
    let window = common::find_window(&title).0 as isize;
    let mut outpost = OutpostUnderTest::new(&app);
    let uia = Uia::new().expect("a UIA client");
    let notes = focus_notes(&mut app, &outpost, &uia, window);
    let (group, _) = outpost.navigate(notes, QueryKind::Parent);

    app.send("set-focus apple");
    let focused = common::listener_focus(&uia, window);
    let _ = outpost.uia_focus(&focused);

    let (name, calls) = read_node(&mut outpost, group.id);
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
