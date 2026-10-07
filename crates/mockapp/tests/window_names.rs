//! A window renamed by its accessible name after it took the foreground:
//! the outpost reports the client area's new accessible name, as NVDA
//! reads a window's name (its `accName`). Windows 11 Notepad does this as
//! it is first activated: its window is "Notepad" for a moment, then names
//! the document. What Verbatim says is then what NVDA says: the window was
//! announced by the name it had when it took the foreground, and its
//! rename, on an ancestor of the focus, is silent (Core's
//! `a_name_change_on_a_focus_ancestor_is_silent`).
//!
//! The outpost runs in this process (`common::outpost`). mockapp's
//! `client-name` renames the client area alone, raising a name change on
//! it. Whether the window is in the foreground plays no part in the
//! outpost's report, and these tests never rely on the foreground (see
//! `events.rs`).

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::{NodeId, NormalizedEvent, PropertyChange};
use verbatim_outpost::protocol::OutpostToSupervisor;

use common::outpost::OutpostUnderTest;

/// The node and the name the outpost's next message, a name change,
/// reports.
fn next_name(outpost: &OutpostUnderTest) -> (NodeId, Option<String>) {
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::PropertyChanged {
                    node_id,
                    change: PropertyChange::Name(name),
                    child_count: None,
                },
            ..
        } => (node_id, name),
        other => panic!("the outpost said {other:?}, not a name change"),
    }
}

fn a_window_renamed_after_activation_is_reported_by_its_accessible_name() {
    common::init_com();
    let title = common::unique_title("mockapp-window-names");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());

    // "Notepad" for a moment, then the document: each name change reports
    // the client area's accessible name, whatever the window's text says.
    common::apply(&mut app, hwnd, "client-name Notepad");
    let (client, name) = next_name(&outpost);
    assert_eq!(name.as_deref(), Some("Notepad"));
    outpost.settled();
    common::apply(&mut app, hwnd, "client-name notes.txt - Notepad");
    assert_eq!(
        next_name(&outpost),
        (client, Some("notes.txt - Notepad".to_owned()))
    );
    outpost.settled();
    app.quit();
}

fn main() {
    harness::run(&[(
        "a_window_renamed_after_activation_is_reported_by_its_accessible_name",
        a_window_renamed_after_activation_is_reported_by_its_accessible_name,
    )]);
}
