//! A progress line rewritten in place in a terminal, as
//! `windows_terminal_progress` in Windows Terminal and `conhost_progress`
//! in the console host. The shared setup is described in the `terminal`
//! module.
//!
//! The written script `progress.ps1` writes "Downloading 10%" and then
//! rewrites the same line, after a carriage return, as "Downloading 20%"
//! and so on to "Downloading 100%", and ends the line once it is done. Each
//! step is written only once the scenario has heard the step before in
//! full: the script waits, on its folder's change notifications, for a
//! file named for the step (`progress-20` and so on), which the scenario
//! writes. So every rewrite is new output of its own, never part of a
//! burst. Verbatim speaks the line the first time, and after that only the
//! word that changed: "20%", "30%", and so on. The prompt follows the last
//! step.

use std::io;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The steps, in percent.
const STEPS: [u32; 10] = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100];

/// The script: each step waits for its file, then rewrites the line.
const SCRIPT: &str = "foreach ($percent in 10, 20, 30, 40, 50, 60, 70, 80, 90, 100) {\r\n\
\x20   $step = \"progress-$percent\"\r\n\
\x20   $watcher = New-Object IO.FileSystemWatcher($PSScriptRoot, $step)\r\n\
\x20   if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $step))) { $null = $watcher.WaitForChanged('Created') }\r\n\
\x20   $watcher.Dispose()\r\n\
\x20   [Console]::Write(\"`rDownloading $percent%\")\r\n\
}\r\n\
[Console]::WriteLine()\r\n";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "progress", &[("progress.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "progress", &[("progress.ps1", SCRIPT)])
}

/// `windows_terminal_progress`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the progress scenario's setup opens a terminal window");
    };
    let directory = directory.clone();
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), &format!("{title} terminal")],
    );
    terminal::type_with_echo(scenario, r".\progress.ps1", terminal::Echo::Shown);
    for percent in STEPS {
        scenario
            .write_agent_file(&format!(r"{directory}\progress-{percent}"), b"")
            .expect("lets the script write the step");
        if percent == STEPS[0] {
            scenario
                .speech()
                .expect(&[&format!("Downloading {percent}%")]);
        } else {
            scenario.speech().expect(&[&format!("{percent}%")]);
        }
    }
    scenario.speech().expect(&[PROMPT]);
}

/// `conhost_progress`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the progress scenario's setup opens a terminal window");
    };
    let directory = directory.clone();
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    terminal::type_with_echo(scenario, r".\progress.ps1", terminal::Echo::Shown);
    for percent in STEPS {
        scenario
            .write_agent_file(&format!(r"{directory}\progress-{percent}"), b"")
            .expect("lets the script write the step");
        if percent == STEPS[0] {
            scenario
                .speech()
                .expect(&[&format!("Downloading {percent}%")]);
        } else {
            scenario.speech().expect(&[&format!("{percent}%")]);
        }
    }
    scenario.speech().expect(&[PROMPT]);
}
