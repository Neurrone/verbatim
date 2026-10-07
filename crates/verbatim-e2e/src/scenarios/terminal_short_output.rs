//! Very short lines of output in a terminal, as
//! `windows_terminal_short_output` in Windows Terminal and
//! `conhost_short_output` in the console host. The shared setup is
//! described in the `terminal` module.
//!
//! The written script `short.ps1` prints "y" and then "ok", each on a line
//! of its own. Both are spoken, exactly and in order, and then the prompt:
//! a line of one or two characters is output like any other, never taken
//! for the echo of typing (NVDA drops such lines; Verbatim does not,
//! `docs/parity.md`).

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The script printing the two short lines.
const SCRIPT: &str = "'y'\r\n'ok'\r\n";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "short-output", &[("short.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "short-output", &[("short.ps1", SCRIPT)])
}

/// `windows_terminal_short_output`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[
            &format!("{title} window"),
            &format!("{title} terminal"),
            "blank",
        ],
    );
    terminal::type_with_echo(scenario, r".\short.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&["y", "ok", PROMPT]);
}

/// `conhost_short_output`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    terminal::type_with_echo(scenario, r".\short.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&["y", "ok", PROMPT]);
}
