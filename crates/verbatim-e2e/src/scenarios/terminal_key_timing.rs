//! Keys pressed while a terminal's output is being spoken, or at once
//! after another key (`phase6-design.md`, "Test design decisions", the
//! floods; the coherence review of 2026-10-09), each scenario in both
//! terminals as its own code (`docs/testing.md`). The shared setup is
//! described in the `terminal` module.
//!
//! - `*_control_flood`: Control while a flood is being spoken, as in NVDA
//!   (`docs/parity.md`, "New terminal output"). `gated.ps1` writes "flood
//!   line 1" to "flood line 100" and waits for the file `more` before it
//!   writes lines 101 to 103. Which bursts Verbatim reads the hundred
//!   lines in depends on timing, so their speech is not asserted line by
//!   line (`docs/testing.md`, "Exact assertions", the owner's exceptions):
//!   the scenario asserts that "flood line 1" starts to play and waits for
//!   "flood line 100" to be queued, the evidence that the whole flood was
//!   read, then presses Control. Everything still queued is cut off: the
//!   utterances end as some, possibly none, heard in full before Control
//!   and then the rest, "flood line 100" always among them, cut off; and
//!   nothing more is said. Once `more` is written, the output read after
//!   Control is spoken: lines 101 to 103 and the prompt, exactly.
//! - `*_shift_flood`: Shift while a flood's first line plays pauses
//!   speech, and Shift again resumes it: each waits for Verbatim to report
//!   speech paused, then resumed, and asserts it. Nothing is cut off or
//!   lost: the first line and the two queued behind it are heard in full,
//!   and the flood then as usual.
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
use super::terminal_flood::{FLOOD_STEP, flood_speech, line, wait_for_prompt, watch_for_prompt};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Ending;
use verbatim_model::UtteranceEnding;

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

/// The flood that waits for Control, then writes a few lines more, for
/// the Control scenarios.
const GATED_SCRIPT: &str = "for ($line = 1; $line -le 100; $line++) { \"flood line $line\" }\r\n\
Wait-For more\r\n\
for ($line = 101; $line -le 103; $line++) { \"flood line $line\" }\r\n";

/// Two hundred lines of twenty words.
const LONG_SCRIPT: &str = "for ($line = 1; $line -le 200; $line++) { \"flood line $line one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen\" }\r\n";

/// Line `n` of `long.ps1`.
fn long_line(n: u32) -> String {
    format!(
        "flood line {n} one two three four five six seven eight nine ten eleven twelve \
         thirteen fourteen fifteen sixteen seventeen"
    )
}

/// The announcement as a terminal of this scenario takes the focus with
/// nothing written yet: Windows Terminal's names its terminal for its tab,
/// the console host's drops the name, and both say "blank" for the empty
/// line.
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

/// Control once the whole gated flood is queued, then the lines written
/// after it.
fn control_steps(scenario: &mut Scenario, directory: &str) {
    terminal::type_with_echo(scenario, r".\gated.ps1", terminal::Echo::Shown);
    let first = scenario.speech().expect_started(&line(1));
    // Everything queued for the flood up to its last line, in whichever
    // bursts it was read: the evidence that all of it was read.
    let mut flood = vec![first];
    flood.extend(scenario.speech().take_until(&line(100), FLOOD_STEP));
    scenario.send_keys(&["control"]).expect("presses Control");
    // Speech plays in order, so the utterances heard in full before
    // Control come first; from the first one cut off, every one is cut
    // off, and the flood's last line always is.
    let mut heard_in_full = true;
    for heard in &flood {
        if heard_in_full
            && matches!(
                scenario.speech().ending_of(heard, FLOOD_STEP),
                Some(UtteranceEnding::Completed)
            )
        {
            continue;
        }
        heard_in_full = false;
        scenario.speech().expect_ended(heard, Ending::Cancelled);
    }
    assert!(
        !heard_in_full,
        "Control cut nothing off: all {} utterances of the flood were heard in full",
        flood.len()
    );
    scenario.expect_nothing_more();
    scenario
        .write_agent_file(&format!(r"{directory}\more"), b"")
        .expect("lets the script write more");
    scenario
        .speech()
        .expect(&[&line(101), &line(102), &line(103), PROMPT]);
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
    scenario.speech().expect_paused();
    scenario
        .send_keys(&["shift"])
        .expect("presses Shift to resume");
    scenario.speech().expect_resumed();
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
    terminal::type_hearing(scenario, r".\long.ps1", terminal::Echo::Shown);
    // Watched from here, so the prompt shown before the command, which
    // Core may report again, is not taken for the one after the flood.
    let watch = watch_for_prompt(scenario);
    scenario.send_keys(&["enter"]).expect("presses enter");
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
    fn a_long_line_is_what_the_script_prints() {
        assert!(LONG_SCRIPT.contains(&long_line(1).replace("line 1 ", "line $line ")));
    }
}
