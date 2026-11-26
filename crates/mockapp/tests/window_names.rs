//! A window renamed by its accessible name after it took the foreground,
//! as Windows 11 Notepad renames its window as it is first activated: the
//! window is "Notepad" for a moment, then names the document. What
//! Verbatim says is what NVDA says: the window was announced by the name it
//! had when it took the foreground, and its rename, off the focus, is not
//! spoken. NVDA speaks a name change only for the focus, so Core, judging
//! the listener's tell against the committed focus, says nothing of it
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects", its changes).
//!
//! Core drives a real outpost in this process (`common::core_driver`).
//! mockapp's `client-name` renames the client area alone, raising a name
//! change on it. The focus, a slider, is told to Core as the listener
//! would; a change of its value raised after the renames is the evidence
//! that their tells were judged, since it comes from the same hook: it is
//! the only thing Core says. Whether the window is in the foreground plays
//! no part, and these tests never rely on the foreground (see `events.rs`).

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::SegmentContent;

use common::core_driver::CoreDriver;
use common::outpost::OutpostUnderTest;

fn a_window_renamed_after_activation_is_not_reported() {
    common::init_com();
    let title = common::unique_title("mockapp-window-names");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    // The slider is node 2, after the root and the button.
    common::apply(&mut app, hwnd, "set-focus slider1");
    assert_eq!(
        core.msaa_focus(hwnd, 2).spoken(),
        [vec![
            SegmentContent::Label("Level".to_owned()),
            SegmentContent::Role(verbatim_model::Role::Slider),
            SegmentContent::Value("1".to_owned())
        ]]
    );
    assert_eq!(
        core.raise(
            &mut app,
            &[
                "client-name Notepad",
                "client-name notes.txt - Notepad",
                "set-value slider1 77"
            ],
            &[WinEventKind::NameChange, WinEventKind::ValueChange],
            3
        )
        .spoken(),
        [vec![SegmentContent::Value("77".to_owned())]]
    );
    drop(core);
    app.quit();
}

fn main() {
    harness::run_isolated(&[(
        "a_window_renamed_after_activation_is_not_reported",
        a_window_renamed_after_activation_is_not_reported,
    )]);
}
