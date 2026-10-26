//! Moving the review cursor between the commands entered in a terminal
//! (`docs/design/terminal-model.md`, "Features and settings"), as
//! `windows_terminal_command_marks` in Windows Terminal and
//! `conhost_command_marks` in the console host. The shared setup is
//! described in the `terminal` module.
//!
//! Three commands are entered: `echo one`, the written script `long.ps1`,
//! which prints "long line 1" to "long line 100", more than three screens,
//! heard through the flood policy, and `echo three`. Enter on each typed
//! line marks it. Verbatim+Alt+Up Arrow three times then moves the review
//! cursor to each command's line and speaks it, newest first: "echo
//! three", the script's line, and "echo one", which output has carried
//! above the rows Core keeps, so it is found by the outpost's search. A
//! fourth press says nothing, past the oldest mark, and Verbatim+Alt+Down
//! Arrow goes back to the script's line. In Windows Terminal, `cls` then
//! erases the history, and Verbatim+Alt+Up Arrow says nothing. NVDA has no
//! such command, so each press is Verbatim's own
//! (`docs/parity/terminals.md`, "Reading a terminal on demand").

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The longest the long command's speech may take between one utterance
/// and the next; it only bounds a hang.
const FLOOD_STEP: Duration = Duration::from_secs(120);

/// More than three screens of output.
const SCRIPT: &str = "1..100 | ForEach-Object { \"long line $_\" }\r\n";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "command-marks", &[("long.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "command-marks", &[("long.ps1", SCRIPT)])
}

/// Presses `gesture`, a move between command marks, and asserts that
/// exactly `heard` is read, or nothing when `heard` is `None`.
fn mark(scenario: &mut Scenario, gesture: &str, heard: Option<&str>) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.deviates_from_nvda("docs/parity/terminals.md", "Reading a terminal on demand");
    match heard {
        Some(heard) => scenario.speech().expect(&[heard]),
        None => scenario.expect_nothing_more(),
    }
}

const UP: &str = "kb:verbatim+alt+uparrow";
const DOWN: &str = "kb:verbatim+alt+downarrow";

/// The steps once the prompt has been read.
fn steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, "echo one", terminal::Echo::Shown);
    scenario.speech().expect(&["one", PROMPT]);
    terminal::type_with_echo(scenario, r".\long.ps1", terminal::Echo::Shown);
    // The flood policy, which NVDA does not have: the 100 lines and the
    // prompt, 30 whole, 41 skipped, and the last 30.
    scenario.deviates_from_nvda("docs/parity/terminals.md", "New terminal output");
    let mut heard: Vec<String> = (1..=30).map(|n| format!("long line {n}")).collect();
    heard.push("sound: skipped-lines skipped 41 lines".to_owned());
    heard.extend((72..=100).map(|n| format!("long line {n}")));
    heard.push(PROMPT.to_owned());
    let heard: Vec<&str> = heard.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&heard, FLOOD_STEP);
    terminal::type_with_echo(scenario, "echo three", terminal::Echo::Shown);
    scenario.speech().expect(&["three", PROMPT]);
    mark(scenario, UP, Some("ready> echo three"));
    mark(scenario, UP, Some(r"ready> .\long.ps1"));
    mark(scenario, UP, Some("ready> echo one"));
    mark(scenario, UP, None);
    mark(scenario, DOWN, Some(r"ready> .\long.ps1"));
}

/// `windows_terminal_command_marks`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    steps(scenario);
    // Windows Terminal's clear erases the history with the marks' lines.
    terminal::type_with_echo(scenario, "cls", terminal::Echo::Shown);
    scenario.speech().expect(&[PROMPT]);
    mark(scenario, UP, None);
}

/// `conhost_command_marks`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    steps(scenario);
}
