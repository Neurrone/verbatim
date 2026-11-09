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
    CallCounts, NodeId, QueryKind, Role, TextOp, TextPoint, TextRead, TextReply, TextUnit,
};
use verbatim_uia::Uia;

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
        (parent.role, parent.name.as_deref()),
        (Role::Window, Some("Desktop")),
        "the parent of the dead element's window"
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
            "a_text_read_of_a_dead_object_says_gone",
            a_text_read_of_a_dead_object_says_gone,
        ),
        (
            "a_navigator_node_keeps_its_element_after_the_focus_moves_on",
            a_navigator_node_keeps_its_element_after_the_focus_moves_on,
        ),
    ]);
}
