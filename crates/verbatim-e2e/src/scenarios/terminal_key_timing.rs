//! Keys pressed while a terminal's output is being spoken, or at once
//! after another key (`phase6-design.md`, "Test design decisions", the
//! floods; the coherence review of 2026-10-09), each scenario in both
//! terminals as its own code (`docs/testing.md`). The shared setup is
//! described in the `terminal` module.
//!
//! - `*_control_flood`: Control while a flood's first line plays.
//!   `gated.ps1` writes "flood line 1" to "flood line 100", then writes the
//!   file `half` and waits for the file `more` before it writes lines 101
//!   to 2000, so Control comes while the shell is waiting and everything it
//!   wrote is on screen. Control cuts the playing line and the two queued
//!   behind it off, and drops everything waiting; nothing more is said.
//!   Once `more` is written, lines 101 to 2000 and the prompt are a burst
//!   of their own, heard as any flood: lines 101 to 130, "skipped 1841
//!   lines", lines 1972 to 2000 and the prompt.
//! - `*_shift_flood`: Shift while a flood's first line plays pauses
//!   speech, and Shift again resumes it. Nothing is cut off or lost: the
//!   first line and the two queued behind it are heard in full, and the
//!   flood then as usual. Speech gives no event for a pause, so the
//!   scenario asserts only that pausing and resuming lose nothing.
//! - `*_line_key_flood`: Up Arrow while a flood's first line plays, once
//!   the flood has ended and the shell shows its prompt (Core receives the
//!   caret on it). `long.ps1` prints 200 lines of twenty words, so the
//!   first is still playing when the flood ends. Up Arrow cuts the output
//!   off, as any key does, and recalls the command, the caret moving to
//!   its end, so the caret's line is spoken as the key's: "ready>
//!   .\long.ps1". NVDA, captured live on 2026-10-09, says the recalled line
//!   too.
//! - `*_up_typing`: `echo one` run, then Up Arrow and "x" pressed in one
//!   batch, the second before the first's line is read. The line's change
//!   is read once, with both in it, and spoken as the change: "echo onex";
//!   then Enter runs it, answered "onex" and the prompt. NVDA, captured
//!   live, says "x" as it is typed and then the line, "echo onex"; the
//!   character's own echo is the difference (`docs/parity.md`).

use std::io;

use super::terminal::{self, PROMPT};
use super::terminal_flood::{
    FLOOD_STEP, GROUP, flood_speech, line, wait_for_prompt, watch_for_prompt,
};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Ending;

pub(crate) use super::no_teardown as teardown;

/// PowerShell functions a script uses to wait for a file the scenario
/// writes into its folder, on the folder's change notifications, never
/// polling, and to write one the scenario waits for.
pub(crate) const FILE_SIGNALS: &str = "function Wait-For([string]$Name) {\r\n\
\x20   $watcher = New-Object IO.FileSystemWatcher($PSScriptRoot, $Name)\r\n\
\x20   if (-not (Test-Path -LiteralPath \"$PSScriptRoot\\$Name\")) { $null = $watcher.WaitForChanged('Created') }\r\n\
\x20   $watcher.Dispose()\r\n\
}\r\n\
function Mark([string]$Name) { [IO.File]::WriteAllText(\"$PSScriptRoot\\$Name\", '') }\r\n";

/// The flood that waits halfway, for the Control scenarios.
const GATED_SCRIPT: &str = "for ($line = 1; $line -le 100; $line++) { \"flood line $line\" }\r\n\
Mark half\r\n\
Wait-For more\r\n\
for ($line = 101; $line -le 2000; $line++) { \"flood line $line\" }\r\n";

/// Two hundred lines of twenty words.
const LONG_SCRIPT: &str = "for ($line = 1; $line -le 200; $line++) { \"flood line $line one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen\" }\r\n";

/// Line `n` of `long.ps1`.
fn long_line(n: u32) -> String {
    format!(
        "flood line {n} one two three four five six seven eight nine ten eleven twelve \
         thirteen fourteen fifteen sixteen seventeen"
    )
}

/// What a burst of the flood lines `first` to `last`, and the prompt after
/// them, says: its first group, the skipped lines, and its last group.
fn burst_from(first: u32, last: u32) -> Vec<String> {
    let total = last - first + 2;
    let mut speech: Vec<String> = (first..first + GROUP).map(line).collect();
    speech.push(format!(
        "sound: skipped-lines skipped {} lines",
        total - 2 * GROUP
    ));
    speech.extend((last - GROUP + 2..=last).map(line));
    speech.push(PROMPT.to_owned());
    speech
}

/// The announcement as a terminal of this scenario takes the focus with
/// nothing written yet: Windows Terminal's, named for its tab, says no
/// line; the console host's, whose name is dropped, says "blank".
fn opening(title: &str, windows_terminal: bool) -> Vec<String> {
    if windows_terminal {
        vec![
            format!("{title} window"),
            format!("{title} terminal"),
            "blank".to_owned(),
        ]
    } else {
        vec![
            format!("{title} window"),
            "terminal".to_owned(),
            "blank".to_owned(),
        ]
    }
}

/// Asserts the opening announcement and the first prompt.
fn opened(scenario: &mut Scenario, state: &ScenarioState, windows_terminal: bool) {
    let title = terminal::title(state).to_owned();
    let opening = opening(&title, windows_terminal);
    let opening: Vec<&str> = opening.iter().map(String::as_str).collect();
    terminal::expect_prompt_read(scenario, state, &opening);
}

/// The folder of the scenario's shell.
fn directory(state: &ScenarioState) -> String {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("a terminal scenario's setup opens a terminal window");
    };
    directory.clone()
}

fn scripts_control() -> String {
    format!("{FILE_SIGNALS}{GATED_SCRIPT}")
}

pub(crate) fn setup_control_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(
        scenario,
        "control-flood",
        &[("gated.ps1", &scripts_control())],
    )
}

pub(crate) fn setup_control_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(
        scenario,
        "control-flood",
        &[("gated.ps1", &scripts_control())],
    )
}

/// Control while the gated flood's first line plays, then the rest.
fn control_steps(scenario: &mut Scenario, directory: &str) {
    terminal::type_with_echo(scenario, r".\gated.ps1", terminal::Echo::Shown);
    scenario
        .wait_for_agent_file(&format!(r"{directory}\half"), FLOOD_STEP)
        .expect("the flood writes its first hundred lines");
    let first = scenario.speech().expect_started(&line(1));
    let queued = scenario.speech().expect_queued(&[&line(2), &line(3)]);
    scenario.send_keys(&["control"]).expect("presses Control");
    scenario.speech().expect_ended(&first, Ending::Cancelled);
    for heard in &queued {
        scenario.speech().expect_ended(heard, Ending::Cancelled);
    }
    scenario.expect_nothing_more();
    scenario
        .write_agent_file(&format!(r"{directory}\more"), b"")
        .expect("lets the flood go on");
    let rest = burst_from(101, 2000);
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&rest, FLOOD_STEP);
}

/// `windows_terminal_control_flood`.
pub(crate) fn body_control_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, true);
    control_steps(scenario, &directory(state));
}

/// `conhost_control_flood`.
pub(crate) fn body_control_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, false);
    control_steps(scenario, &directory(state));
}

pub(crate) fn setup_shift_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(
        scenario,
        "shift-flood",
        &[("flood.ps1", super::terminal_flood::SCRIPT)],
    )
}

pub(crate) fn setup_shift_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(
        scenario,
        "shift-flood",
        &[("flood.ps1", super::terminal_flood::SCRIPT)],
    )
}

/// Shift twice while the flood's first line plays: paused and resumed,
/// nothing lost.
fn shift_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\flood.ps1 1", terminal::Echo::Shown);
    let first = scenario.speech().expect_started(&line(1));
    let queued = scenario.speech().expect_queued(&[&line(2), &line(3)]);
    scenario
        .send_keys(&["shift"])
        .expect("presses Shift to pause");
    scenario
        .send_keys(&["shift"])
        .expect("presses Shift to resume");
    scenario.speech().expect_ended(&first, Ending::Completed);
    for heard in &queued {
        scenario.speech().expect_ended(heard, Ending::Completed);
    }
    let speech = flood_speech();
    let rest: Vec<&str> = speech[3..].iter().map(String::as_str).collect();
    scenario.speech().expect_within(&rest, FLOOD_STEP);
}

/// `windows_terminal_shift_flood`.
pub(crate) fn body_shift_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, true);
    shift_steps(scenario);
}

/// `conhost_shift_flood`.
pub(crate) fn body_shift_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, false);
    shift_steps(scenario);
}

pub(crate) fn setup_line_key_windows_terminal(
    scenario: &mut Scenario,
) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "line-key-flood", &[("long.ps1", LONG_SCRIPT)])
}

pub(crate) fn setup_line_key_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "line-key-flood", &[("long.ps1", LONG_SCRIPT)])
}

/// Up Arrow while the long flood's first line plays, once the prompt is
/// shown.
fn line_key_steps(scenario: &mut Scenario) {
    let watch = watch_for_prompt(scenario);
    terminal::type_with_echo(scenario, r".\long.ps1", terminal::Echo::Shown);
    let first = scenario.speech().expect_started(&long_line(1));
    let queued = scenario
        .speech()
        .expect_queued(&[&long_line(2), &long_line(3)]);
    wait_for_prompt(watch);
    scenario.send_keys(&["uparrow"]).expect("presses Up");
    scenario.speech().expect_ended(&first, Ending::Cancelled);
    for heard in &queued {
        scenario.speech().expect_ended(heard, Ending::Cancelled);
    }
    scenario.speech().expect(&[r"ready> .\long.ps1"]);
}

/// `windows_terminal_line_key_flood`.
pub(crate) fn body_line_key_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, true);
    line_key_steps(scenario);
}

/// `conhost_line_key_flood`.
pub(crate) fn body_line_key_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, false);
    line_key_steps(scenario);
}

pub(crate) fn setup_up_typing_windows_terminal(
    scenario: &mut Scenario,
) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "up-typing", &[])
}

pub(crate) fn setup_up_typing_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "up-typing", &[])
}

/// A command run, recalled with Up Arrow and typed after at once, and run.
fn up_typing_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, "echo one", terminal::Echo::Shown);
    scenario.speech().expect(&["one", PROMPT]);
    scenario
        .send_keys(&["uparrow", "x"])
        .expect("presses Up and types x at once");
    scenario.speech().expect(&["echo onex"]);
    scenario.send_keys(&["enter"]).expect("presses Enter");
    scenario.speech().expect(&["onex", PROMPT]);
}

/// `windows_terminal_up_typing`.
pub(crate) fn body_up_typing_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, true);
    up_typing_steps(scenario);
}

/// `conhost_up_typing`.
pub(crate) fn body_up_typing_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, false);
    up_typing_steps(scenario);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_burst_after_control_is_counted_from_line_101() {
        let speech = burst_from(101, 2000);
        assert_eq!(speech.len(), 2 * GROUP as usize + 1);
        assert_eq!(speech[0], "flood line 101");
        assert_eq!(speech[29], "flood line 130");
        assert_eq!(speech[30], "sound: skipped-lines skipped 1841 lines");
        assert_eq!(speech[31], "flood line 1972");
        assert_eq!(speech[59], "flood line 2000");
        assert_eq!(speech[60], PROMPT);
    }

    #[test]
    fn a_long_line_is_what_the_script_prints() {
        assert!(LONG_SCRIPT.contains(&long_line(1).replace("line 1 ", "line $line ")));
    }
}
