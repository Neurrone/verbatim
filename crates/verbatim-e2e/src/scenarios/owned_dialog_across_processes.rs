//! A dialog owned across processes (`docs/parity/focus-context.md`,
//! "Focus-ancestry context"): a `mockapp` UIA window in one process owns a
//! UIA dialog that a
//! second `mockapp` process opens over it (`--owner-title`), as a dialog
//! another process opens is owned by the window it opens over. The
//! dialog's outpost describes the owner, above its process boundary, as a
//! stand-in from Windows' state, which is the owner's own UIA element by
//! NVDA's equality, so the dialog is entered with no owner title said; Tab
//! and Shift+Tab move between its two buttons, and Space presses one,
//! which closes the dialog and ends its process, and the owner, restoring
//! its focus as it is activated (`focus_on_activate`), says its focus
//! alone. Every expectation is NVDA's capture of the same fixture at the
//! pin (alpha-58022, 3 runs, identical).

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// The owner window's name in the run.
const OWNER: &str = "Mockapp Owner";

/// The owned dialog's name in the run.
const DIALOG: &str = "Mockapp Owned Dialog";

/// Opens a mockapp UIA window titled with [`harness_marker`] of `name`,
/// from the fixture `root`'s JSON with `{title}` replaced by that title,
/// with `extra` arguments, and waits for it to take the foreground.
fn open(scenario: &mut Scenario, name: &str, root: &str, extra: &[String]) -> io::Result<()> {
    let directory = scenario.run_directory().to_owned();
    let title = harness_marker(name);
    let fixture = scenario.harness_file(name, "json");
    let output = scenario.harness_file(name, "log");
    scenario.write_agent_file(&fixture, root.replace("{title}", &title).as_bytes())?;
    let mut args = [
        "--fixture",
        &fixture,
        "--backend",
        "uia",
        "--title",
        &title,
        "--show",
    ]
    .map(str::to_owned)
    .to_vec();
    args.extend_from_slice(extra);
    scenario.launch_titled_with_stdin(
        &format!(r"{directory}\mockapp.exe"),
        &args,
        &title,
        &output,
    )?;
    Ok(())
}

/// The owner: a UIA window whose focused button "Open" has its UIA focus
/// event raised each time the window is activated, as an application
/// restores its focus.
const OWNER_FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "focus_on_activate": true,
  "children": [
    { "id": "open", "role": "button", "name": "Open", "states": ["focusable", "focused"] }
  ]
}
"#;

/// The owned dialog: a UIA dialog with the buttons "OK", focused, and
/// "Cancel", taking a dialog's keys (`dialog_keys`).
const DIALOG_FIXTURE: &str = r#"{
  "id": "root",
  "role": "dialog",
  "name": "{title}",
  "dialog_keys": true,
  "children": [
    { "id": "ok", "role": "button", "name": "OK", "states": ["focusable", "focused"] },
    { "id": "cancel", "role": "button", "name": "Cancel", "states": ["focusable"] }
  ]
}
"#;

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    open(scenario, OWNER, OWNER_FIXTURE, &[]).expect("opens the owner window");
    let owner_window = format!("{} window", harness_marker(OWNER));
    scenario.speech().expect(&[&owner_window, "Open button"]);

    let owner_title = ["--owner-title".to_owned(), harness_marker(OWNER)];
    open(scenario, DIALOG, DIALOG_FIXTURE, &owner_title).expect("opens the owned dialog");
    // The dialog is entered with no owner title said: the owner's stand-in
    // is the owner's own object.
    let dialog_window = format!("{} window", harness_marker(DIALOG));
    scenario.speech().expect(&[&dialog_window, "OK button"]);

    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario.speech().expect(&["Cancel button"]);
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario.speech().expect(&["OK button"]);
    scenario.send_keys(&["shift+tab"]).expect("sends shift+tab");
    scenario.speech().expect(&["Cancel button"]);

    // Space presses "Cancel", which closes the dialog; the return to the
    // owner says its focus alone.
    scenario.send_keys(&["space"]).expect("sends space");
    scenario.speech().expect(&["Open button"]);
}
