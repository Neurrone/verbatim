//! A window renamed by its accessible name after it took the foreground,
//! as Windows 11 Notepad renames its window as it is first activated: the
//! window is "Notepad" for a moment, then names the document. What
//! Verbatim says is what NVDA says: the window was announced by the name it
//! had when it took the foreground, and its rename, off the focus, is not
//! spoken. NVDA speaks a name change only for the focus, so the outpost
//! tells the renamed object from the focus by its identity and does not
//! read or report it (`docs/crates/verbatim-outpost.md`, "MSAA name,
//! value, and state changes").
//!
//! The outpost runs in this process (`common::outpost`). mockapp's
//! `client-name` renames the client area alone, raising a name change on
//! it. The focus, a slider, is handed to the outpost as the listener
//! would; a change of its value raised after the renames is the evidence
//! that their events were handled, since it comes from the same hook: it
//! is the first thing the outpost says. Whether the window
//! is in the foreground plays no part in the outpost's report, and these
//! tests never rely on the foreground (see `events.rs`).

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::NormalizedEvent;
use verbatim_outpost::protocol::OutpostToSupervisor;

use common::outpost::OutpostUnderTest;

fn a_window_renamed_after_activation_is_not_reported() {
    common::init_com();
    let title = common::unique_title("mockapp-window-names");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());

    // The slider is node 2, after the root and the button.
    common::apply(&mut app, hwnd, "set-focus slider1");
    let focus = outpost.msaa_focus(hwnd, 2);
    assert_eq!(focus.node.name.as_deref(), Some("Level"));
    common::apply(&mut app, hwnd, "client-name Notepad");
    common::apply(&mut app, hwnd, "client-name notes.txt - Notepad");
    common::apply(&mut app, hwnd, "set-value slider1 77");
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::ValueChanged { value, .. },
            ..
        } => assert_eq!(value.as_deref(), Some("77")),
        other => panic!("the outpost said {other:?} before the focus's value change"),
    }
    outpost.settled();
    app.quit();
}

fn main() {
    harness::run(&[(
        "a_window_renamed_after_activation_is_not_reported",
        a_window_renamed_after_activation_is_not_reported,
    )]);
}
