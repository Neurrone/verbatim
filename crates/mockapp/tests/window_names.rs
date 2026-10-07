//! How a real outpost names a top-level window's client area read through
//! MSAA: by the window's text, even when the client area's accessible name
//! says otherwise, which differs from NVDA (`docs/parity.md`, "A top-level
//! window's name"); and a real rename of the window, its text changing, is
//! reported as the new name, which Core speaks as NVDA speaks a name change
//! on the focus. The foreground report of such a window names it the same
//! way, through the same function, but needs the window in the foreground,
//! which these tests never rely on (see `events.rs`), so the name changes
//! carry the assertions.
//!
//! The outpost runs in this process (`common::outpost`). mockapp's
//! `client-name` changes the client area's accessible name alone, as
//! Windows 11 Notepad does for a moment when its window is activated, and
//! `set-title` changes the window's text; both raise a name change on the
//! client area.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::{NormalizedEvent, PropertyChange};
use verbatim_outpost::protocol::OutpostToSupervisor;

use common::outpost::OutpostUnderTest;

/// The name the outpost's next message, a name change, reports.
fn next_name(outpost: &OutpostUnderTest) -> Option<String> {
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::PropertyChanged {
                    change: PropertyChange::Name(name),
                    child_count: None,
                    ..
                },
            ..
        } => name,
        other => panic!("the outpost said {other:?}, not a name change"),
    }
}

#[test]
fn a_top_level_windows_client_area_is_named_by_the_window_text() {
    common::init_com();
    let title = common::unique_title("mockapp-window-names");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());

    // The client area renamed "Notepad", the window's text unchanged: the
    // name reported is still the window's text, the title, and Core, which
    // already has that name, says nothing.
    common::apply(&mut app, hwnd, "client-name Notepad");
    assert_eq!(next_name(&outpost), Some(title.clone()));
    outpost.settled();

    // The window renamed: its new text is its new name, which Core speaks.
    common::apply(&mut app, hwnd, "set-title Renamed window");
    assert_eq!(next_name(&outpost), Some("Renamed window".to_owned()));
    outpost.settled();
    app.quit();
}
