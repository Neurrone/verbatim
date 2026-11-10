//! The event recorder (`books/dev/src/reference/crates/verbatim-event-record.md`)
//! records mockapp while a steps file presses Tab, beside a running
//! Verbatim: the file holds the window's registration and the focus the
//! key moved after that registration, and the recorder ends by itself after
//! its last step. A focus event arrives as the handlers are added, before
//! any step runs, naming the taskbar or the window's focus, so only what
//! comes after the registration is the key's.

use serde_json::Value;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, WINDOW_TIMEOUT, harness_marker};

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

const NAME: &str = "event-recorder";

/// Two buttons, the first focused: Tab moves the focus to the second.
const FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "dialog_keys": true,
  "children": [
    { "id": "first", "role": "button", "name": "First", "states": ["focusable", "focused"] },
    { "id": "second", "role": "button", "name": "Second", "states": ["focusable"] }
  ]
}
"#;

/// The lines of a recording, each parsed.
fn parse_lines(contents: &[u8]) -> Vec<Value> {
    std::str::from_utf8(contents)
        .expect("a recording is UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a recording's line is JSON"))
        .collect()
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let directory = scenario.run_directory().to_owned();
    let title = harness_marker(NAME);
    let fixture = scenario.harness_file(NAME, "json");
    scenario
        .write_agent_file(&fixture, FIXTURE.replace("{title}", &title).as_bytes())
        .expect("writes mockapp's fixture");
    let args = [
        "--fixture",
        &fixture,
        "--backend",
        "uia",
        "--title",
        &title,
        "--show",
    ]
    .map(str::to_owned);
    let window = scenario
        .launch_titled(&format!(r"{directory}\mockapp.exe"), &args, &title, true)
        .expect("opens mockapp");
    scenario
        .speech()
        .expect(&[&format!("{title} window"), "First button"]);

    let steps = scenario.harness_file(&format!("{NAME}-steps"), "txt");
    scenario
        .write_agent_file(&steps, b"press tab\n")
        .expect("writes the steps file");
    let output = scenario.harness_file(&format!("{NAME}-recording"), "jsonl");
    let handle = format!("0x{:X}", window.window);
    let recorder_args =
        ["--window", &handle, "--steps", &steps, "--output", &output].map(str::to_owned);
    scenario
        .run_to_success(
            &format!(r"{directory}\verbatim-event-record.exe"),
            &recorder_args,
        )
        .expect("the recording ends by itself with code 0");
    scenario.speech().expect(&["Second button"]);

    let contents = scenario
        .wait_for_agent_file(&output, WINDOW_TIMEOUT)
        .expect("the recording");
    let recording = String::from_utf8_lossy(&contents);
    let lines = parse_lines(&contents);
    assert_eq!(lines[0]["format_version"], Value::from(1));
    assert_eq!(lines[0]["target"]["window"], Value::from(handle.clone()));
    let registered: Vec<&Value> = lines
        .iter()
        .filter(|line| line["kind"] == "registered")
        .map(|line| &line["window"])
        .collect();
    assert_eq!(registered, [&Value::from(handle)], "{recording}");
    let after_registration = lines
        .iter()
        .position(|line| line["kind"] == "registered")
        .expect("the registered line");
    let focuses: Vec<&Value> = lines[after_registration..]
        .iter()
        .filter(|line| line["kind"] == "uia_focus" && line["in_target"] == true)
        .map(|line| &line["name"])
        .collect();
    assert_eq!(focuses, [&Value::from("Second")], "{recording}");
}
