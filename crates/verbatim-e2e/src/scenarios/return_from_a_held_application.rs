//! The return from an application whose reads are held
//! (`docs/design/focus-pipeline-rework-remaining.md`, "One foreground
//! change"): the user leaves the Windows Forms text box
//! ([`super::text_box`]) for a `mockapp` UIA window whose window thread
//! holds the first call any client makes into it (mockapp's `hold`), as a
//! hung application's does, and comes back. Nothing is said for the held
//! window while it does not answer, as NVDA says nothing, and the return
//! to the text box says what the same switch says without a stall.
//!
//! The mockapp window is read once first, so the outpost's verdict that
//! it is a UIA window is kept and the entry into it makes no probe. The
//! scenario waits on mockapp's own evidence: the
//! named events mockapp sets as it prints `applied` for the `hold` and
//! `held` as the outpost's call begins (`--line-events`).

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, WINDOW_TIMEOUT, harness_marker};

pub(crate) use super::no_teardown as teardown;

/// The text box's window's name in the run, as in
/// `second_application_and_verbatim_menu`.
const NAME: &str = "second-application";

/// The text box's name.
const BOX_NAME: &str = "Note";

/// The text box's one line.
const LINE: &str = "Hello from the second application.";

/// The held mockapp window's name in the run.
const HELD: &str = "held-application";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::TargetPid(super::text_box::open(
        scenario, NAME, BOX_NAME, LINE,
    )?))
}

/// The prefix of the named events the held mockapp window sets as it
/// prints its lines, unique to this run of the test binary.
fn line_events() -> String {
    format!(
        r"Local\verbatim-e2e-held-application-{}-",
        std::process::id()
    )
}

/// Opens the held mockapp window, titled with [`harness_marker`] of
/// [`HELD`], a UIA window holding one focused button, its standard input a
/// pipe and its lines setting the events of `events`, and waits for it to
/// take the foreground. Returns its process id.
fn open_held_application(scenario: &mut Scenario, events: &str) -> io::Result<u32> {
    let directory = scenario.run_directory().to_owned();
    let title = harness_marker(HELD);
    let fixture = scenario.harness_file(HELD, "json");
    let output = scenario.harness_file(HELD, "log");
    let contents = format!(
        r#"{{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [
    {{ "id": "ok", "role": "button", "name": "OK", "states": ["focusable", "focused"] }}
  ]
}}
"#
    );
    scenario.write_agent_file(&fixture, contents.as_bytes())?;
    let args = [
        "--fixture",
        &fixture,
        "--backend",
        "uia",
        "--title",
        &title,
        "--show",
        "--line-events",
        events,
    ]
    .map(str::to_owned);
    let window = scenario.launch_titled_with_stdin(
        &format!(r"{directory}\mockapp.exe"),
        &args,
        &title,
        &output,
    )?;
    Ok(window.pid)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::text_box::expect_announced(scenario, NAME, BOX_NAME, LINE);

    // The mockapp window, read once, so its verdict is kept.
    let events = line_events();
    let held = open_held_application(scenario, &events).expect("opens the mockapp window");
    let held_window = format!("{} window", harness_marker(HELD));
    scenario.speech().expect(&[&held_window, "OK button"]);

    scenario
        .bring_window_forward(&harness_marker(NAME))
        .expect("brings the text box forward");
    super::text_box::expect_announced(scenario, NAME, BOX_NAME, LINE);

    // mockapp holds the next call into it, once it has taken the command.
    let applied = format!("{events}applied");
    scenario
        .create_event(&applied)
        .expect("creates the event for hold's acknowledgement");
    scenario.write_stdin(held, "hold").expect("writes hold");
    scenario
        .wait_for_event(&applied, held, WINDOW_TIMEOUT)
        .expect("mockapp takes hold");

    // Entered while held: nothing is said, so the next utterance is the
    // return's.
    let held_call = format!("{events}held");
    scenario
        .create_event(&held_call)
        .expect("creates the event for the held call");
    scenario
        .bring_window_forward(&harness_marker(HELD))
        .expect("brings the mockapp window forward");
    scenario
        .wait_for_event(&held_call, held, WINDOW_TIMEOUT)
        .expect("a call into mockapp is held");

    // The return, said as the same switch without a stall.
    scenario
        .bring_window_forward(&harness_marker(NAME))
        .expect("brings the text box forward");
    super::text_box::expect_announced(scenario, NAME, BOX_NAME, LINE);

    scenario
        .write_stdin(held, "release")
        .expect("writes release");
}
