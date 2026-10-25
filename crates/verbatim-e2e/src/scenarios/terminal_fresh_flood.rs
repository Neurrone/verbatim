//! A write of more lines than the history holds onto a screen that starts
//! at the document's start, with no history above it, as
//! `windows_terminal_clear_flood` after Windows Terminal's clear and
//! `conhost_fresh_flood` on a fresh console host (`docs/design/terminal-model.md`,
//! "A lost read" and "The aside screen"). Each its own code
//! (`docs/testing.md`); the shared setup is described in the `terminal`
//! module.
//!
//! The read after such a write keeps nothing of the screen before it, and
//! the screen it replaced is not aside or no longer in the buffer, so the
//! read is lost and the whole document is read: the last read's rows are
//! nowhere in it, so more went by than the buffer holds, and the burst is
//! heard as its first 30 lines, "skipped more than N lines", and its last
//! 30, the prompt the last of them (Dickson, 2026-10-25).
//!
//! `fresh.ps1` writes "fill line 1" to "fill line 9100" in one call of the
//! raw standard output stream's `Write`, which Windows PowerShell 5.1's
//! console stream passes to one `WriteFile`; the console host takes such
//! a write whole before the outpost's first read (`terminal_overflow`). The
//! console host's buffer holds 9,001 rows: the command's row and the 9,100
//! lines need 9,102 rows with the caret's, so the first 101 rows left the
//! buffer, and it holds fill lines 101 to 9,100 and the prompt, 9,001
//! lines: 30 spoken, 30 last, "skipped more than 8941 lines".
//!
//! Windows Terminal may read a write of that size before it is whole
//! (`terminal_overflow`), so `clear.ps1` makes it land while Core holds the
//! terminal. It writes "before line 1" to "before line 30" in one write, a
//! group's worth, so Core holds the outpost; once the first has been heard
//! the scenario writes a go file, and the script clears the screen with
//! `Clear-Host`, which in Windows Terminal erases the screen and the
//! history, and writes "cleared line 1" to "cleared line 30" in one write
//! with no line break after the last, so the screen still starts at the
//! document's start. The read after the group's last line finds the clear,
//! a new screen setting the screen before it aside, and the cleared lines
//! a group of their own, so Core holds again. Once "cleared line 1" has
//! been heard the scenario writes a second go file, and the script writes
//! the 9,100 fill lines in one write while Core holds. Windows Terminal's
//! document then holds 9,031 rows, its history and its screen: fill lines
//! 71 to 9,100 and the prompt, so 30 last and "skipped more than 9001
//! lines" once the cleared lines' group is heard.

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The longest the flood and the speech it causes may take between one
/// utterance and the next; it only bounds a hang.
const FLOOD_STEP: Duration = Duration::from_secs(120);

/// The go files the clearing script waits for.
const GO_FILE: &str = "clear-go";
const SECOND_GO_FILE: &str = "clear-go-2";

/// The fill lines, in one raw write.
const FRESH_SCRIPT: &str = "$fill = foreach ($line in 1..9100) { \"fill line $line\" }\r\n\
$bytes = [Console]::OutputEncoding.GetBytes(($fill -join \"`r`n\") + \"`r`n\")\r\n\
[Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)\r\n";

/// A group's worth of lines, the go file, the clear and the cleared lines,
/// the second go file, then the fill lines, each in one raw write.
const CLEAR_SCRIPT: &str = "function Wait-Go($name) {\r\n\
\x20   $watcher = New-Object IO.FileSystemWatcher($PSScriptRoot, $name)\r\n\
\x20   if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $name))) { $null = $watcher.WaitForChanged('Created') }\r\n\
\x20   $watcher.Dispose()\r\n\
}\r\n\
function Write-Raw($text) {\r\n\
\x20   $bytes = [Console]::OutputEncoding.GetBytes($text)\r\n\
\x20   [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)\r\n\
}\r\n\
$before = foreach ($line in 1..30) { \"before line $line\" }\r\n\
Write-Raw (($before -join \"`r`n\") + \"`r`n\")\r\n\
Wait-Go 'clear-go'\r\n\
Clear-Host\r\n\
$cleared = foreach ($line in 1..30) { \"cleared line $line\" }\r\n\
Write-Raw ($cleared -join \"`r`n\")\r\n\
Wait-Go 'clear-go-2'\r\n\
$fill = foreach ($line in 1..9100) { \"fill line $line\" }\r\n\
Write-Raw (\"`r`n\" + ($fill -join \"`r`n\") + \"`r`n\")\r\n";

/// `word` lines `first` to `last`.
fn numbered(word: &str, first: u32, last: u32) -> Vec<String> {
    (first..=last)
        .map(|line| format!("{word} {line}"))
        .collect()
}

/// Asserts exactly `speech`, in order, each heard in full.
fn expect(scenario: &mut Scenario, speech: &[String]) {
    let speech: Vec<&str> = speech.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&speech, FLOOD_STEP);
}

/// Writes the go file `name` into the window's folder: in compare mode a
/// step of its own.
fn go(scenario: &mut Scenario, state: &ScenarioState, name: &str) {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the scenario's setup opens a terminal window");
    };
    let directory = directory.clone();
    scenario.begin_action("let the script write its next part");
    scenario
        .write_agent_file(&format!(r"{directory}\{name}"), b"")
        .expect("lets the script write its next part");
}

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "clear-flood", &[("clear.ps1", CLEAR_SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "fresh-flood", &[("fresh.ps1", FRESH_SCRIPT)])
}

/// `windows_terminal_clear_flood`: N = 9,001.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    terminal::type_with_echo(scenario, r".\clear.ps1", terminal::Echo::Shown);
    expect(scenario, &numbered("before line", 1, 1));
    go(scenario, state, GO_FILE);
    let mut cleared = numbered("before line", 2, 30);
    cleared.extend(numbered("cleared line", 1, 1));
    expect(scenario, &cleared);
    go(scenario, state, SECOND_GO_FILE);
    let mut rest = numbered("cleared line", 2, 30);
    rest.push("sound: skipped-lines skipped more than 9001 lines".to_owned());
    rest.extend(numbered("fill line", 9_072, 9_100));
    rest.push(PROMPT.to_owned());
    expect(scenario, &rest);
}

/// `conhost_fresh_flood`: N = 8,941.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    terminal::type_with_echo(scenario, r".\fresh.ps1", terminal::Echo::Shown);
    let mut heard = numbered("fill line", 101, 130);
    heard.push("sound: skipped-lines skipped more than 8941 lines".to_owned());
    heard.extend(numbered("fill line", 9_072, 9_100));
    heard.push(PROMPT.to_owned());
    expect(scenario, &heard);
}
