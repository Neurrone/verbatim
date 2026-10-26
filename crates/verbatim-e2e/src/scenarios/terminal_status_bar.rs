//! "Report status bar" in a terminal (`docs/design/terminal-model.md`,
//! "Features and settings"), as `windows_terminal_status_bar` in Windows
//! Terminal and `conhost_status_bar` in the console host. The shared setup
//! is described in the `terminal` module.
//!
//! The footer flood's script, `footer.ps1` (`terminal_footer`), keeps its
//! last row for a footer below the caret's row. Verbatim+End reads it as
//! "status: busy" while the script waits, and as "status: done" after the
//! flood; once the script has cleared the screen and the prompt is back,
//! with nothing below the caret and no row a program rewrote, it reads the
//! prompt's line, the last with text. NVDA has no terminal status bar, so
//! each press is Verbatim's own (`docs/parity/terminals.md`, "Reading a
//! terminal on demand").

use std::io;

use super::terminal::{self, PROMPT};
use super::terminal_footer::{self, SCRIPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "status-bar", &[("footer.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "status-bar", &[("footer.ps1", SCRIPT)])
}

/// Presses "Report status bar" and asserts that exactly `heard` is read.
fn status_bar(scenario: &mut Scenario, heard: &str) {
    scenario
        .send_gesture("kb:verbatim+end")
        .expect("sends report status bar");
    scenario.deviates_from_nvda("docs/parity/terminals.md", "Reading a terminal on demand");
    scenario.speech().expect(&[heard]);
}

/// The steps once the prompt has been read.
fn steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\footer.ps1", terminal::Echo::Shown);
    scenario.speech().expect(&["status: busy"]);
    status_bar(scenario, "status: busy");
    terminal_footer::flood(scenario);
    status_bar(scenario, "status: done");
    scenario.send_keys(&["n"]).expect("presses n");
    scenario.speech().expect(&["footer gone", PROMPT]);
    status_bar(scenario, PROMPT);
}

/// `windows_terminal_status_bar`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    steps(scenario);
}

/// `conhost_status_bar`.
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
