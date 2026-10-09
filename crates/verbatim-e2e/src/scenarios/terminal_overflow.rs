//! Floods arriving in one write while a group plays and after it has been
//! heard, within the terminal's history and past it, in Windows Terminal
//! and the console host, each its own code (`docs/testing.md`). These are
//! the acceptance tests of the terminal reading package
//! (`phase6-design.md`, "Terminal decisions" and "Terminal reading by
//! diffing the screen"): when Core reaches the end of a group it asks, and
//! the outpost reads up to the end, so one read gives the count and the
//! newest lines; when the anchor's line has left a full history, Verbatim
//! says "skipped more than N lines". That package has landed, and these
//! scenarios are part of the suite.
//!
//! The written script `phased.ps1` prints "before line 1" to "before line
//! 30", then waits, on its folder's change notifications, for a go file,
//! and then writes its whole second part, "after line 1" onwards, in one
//! write. The prompt follows, and counts as a line like any other: the
//! last 30 lines are 29 after lines and the prompt.
//!
//! The go file is written at one of two points:
//!
//! - After all 30 before lines have been heard in full (`*_history_flood`,
//!   `*_scrollback_overflow`). The burst has ended, so the second part is
//!   a burst of its own, whose first 30 lines are spoken whole.
//! - As soon as the first before line has been heard in full, while the
//!   group still plays (`*_during_group`). The second part joins the
//!   burst, and is skipped down to its last lines.
//!
//! Windows Terminal's `windows_terminal_scrollback_overflow` has the script
//! split its second part: its first 30 lines come in a write of their own,
//! and the rest once the first has been heard, as a second go file lets
//! it, so the burst's first lines are read live and the overflow lands
//! while their group plays. Written whole, 12,000 lines may or may not
//! leave the history before Windows Terminal's first text change reaches
//! the outpost (measured 2026-10-08: the first read found line 11,881 in
//! two runs of three, and line 6 in the third), so what is spoken has no
//! one correct form the scenario could fix: either the burst's first lines
//! and "skipped more than 9001 lines", or that count alone, each followed
//! by the last lines. NVDA, on the same single write, spoke lines 10,758 to
//! 10,772 and 11,902 to 12,000 and the prompt, whatever its diff found
//! current, with nothing for the rest (captured live, 2026-10-08). The
//! whole write landing before any read is tested deterministically by
//! `windows_terminal_scrollback_overflow_during_group`, whose write lands
//! while Core holds the terminal.
//!
//! Within the history, 5,000 lines; past it, 12,000. Past it, the anchor is
//! known to be gone, and Verbatim says "skipped more than N lines", N being
//! the number of lines the terminal's buffer holds, scrollback and visible
//! rows together, less the 30 last lines. Both buffers were measured on
//! 2026-10-08 by opening each terminal as the harness does, writing 12,000
//! numbered lines in one write, and reading its text area's whole document
//! through UI Automation's text pattern:
//!
//! - Windows Terminal (`--size 120,30`, its default history) held 9,031
//!   lines: 9,001 of scrollback and 30 rows. N = 9,001.
//! - The console host (buffer set to 9,001 lines by the start script, as
//!   `[Console]::BufferHeight` then reports) held 9,001 lines in all.
//!   N = 8,971.

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The go file the script waits for before its second part.
const GO_FILE: &str = "phased-go";

/// The longest the flood and the speech it causes may take between one
/// utterance and the next; it only bounds a hang.
const FLOOD_STEP: Duration = Duration::from_secs(120);

/// The second go file, which a script told to split its second part
/// waits for between the two writes.
const SECOND_GO_FILE: &str = "phased-go-2";

/// The script: 30 lines, the go file, then `$After` lines in one write; or,
/// given `$First`, the first `$First` of them in one write, the second go
/// file, and the rest in another.
const SCRIPT: &str = "param([int]$After, [int]$First = 0)\r\n\
function Wait-Go($name) {\r\n\
\x20   $watcher = New-Object IO.FileSystemWatcher($PSScriptRoot, $name)\r\n\
\x20   if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $name))) { $null = $watcher.WaitForChanged('Created') }\r\n\
\x20   $watcher.Dispose()\r\n\
}\r\n\
function Write-After($from, $to) {\r\n\
\x20   $lines = foreach ($line in $from..$to) { \"after line $line\" }\r\n\
\x20   [Console]::Out.Write(($lines -join \"`r`n\") + \"`r`n\")\r\n\
}\r\n\
foreach ($line in 1..30) { \"before line $line\" }\r\n\
Wait-Go 'phased-go'\r\n\
if ($First -gt 0) {\r\n\
\x20   Write-After 1 $First\r\n\
\x20   Wait-Go 'phased-go-2'\r\n\
\x20   Write-After ($First + 1) $After\r\n\
} else {\r\n\
\x20   Write-After 1 $After\r\n\
}\r\n";

/// "before line `first`" to "before line `last`".
fn before(first: u32, last: u32) -> Vec<String> {
    (first..=last)
        .map(|line| format!("before line {line}"))
        .collect()
}

/// "after line `first`" to "after line `last`".
fn after(first: u32, last: u32) -> Vec<String> {
    (first..=last)
        .map(|line| format!("after line {line}"))
        .collect()
}

/// Types the command running the script with `count` lines in its second
/// part, and returns the folder the go file goes in.
fn start_script(scenario: &mut Scenario, state: &ScenarioState, count: u32) -> String {
    start_script_with(scenario, state, &format!("{count}"))
}

/// Types the command running the script with `arguments`, and returns the
/// folder the go files go in.
fn start_script_with(scenario: &mut Scenario, state: &ScenarioState, arguments: &str) -> String {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the scenario's setup opens a terminal window");
    };
    let directory = directory.clone();
    terminal::type_with_echo(
        scenario,
        &format!(r".\phased.ps1 {arguments}"),
        terminal::Echo::Shown,
    );
    directory
}

/// Writes the go file, letting the script write its second part.
fn go(scenario: &mut Scenario, directory: &str) {
    scenario
        .write_agent_file(&format!(r"{directory}\{GO_FILE}"), b"")
        .expect("lets the script write its second part");
}

/// Writes the second go file, letting a script that split its second part
/// write the rest.
fn second_go(scenario: &mut Scenario, directory: &str) {
    scenario
        .write_agent_file(&format!(r"{directory}\{SECOND_GO_FILE}"), b"")
        .expect("lets the script write the rest of its second part");
}

/// Asserts exactly `speech`, in order, each heard in full.
fn expect(scenario: &mut Scenario, speech: &[String]) {
    let speech: Vec<&str> = speech.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&speech, FLOOD_STEP);
}

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "phased-flood", &[("phased.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "phased-flood", &[("phased.ps1", SCRIPT)])
}

/// Asserts Windows Terminal's announcement and the prompt read.
fn windows_terminal_ready(scenario: &mut Scenario, state: &ScenarioState) {
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), &format!("{title} terminal")],
    );
}

/// Asserts the console host's announcement, whose text area has no name,
/// and the prompt read.
fn console_host_ready(scenario: &mut Scenario, state: &ScenarioState) {
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
}

/// `windows_terminal_history_flood`.
pub(crate) fn body_history_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    windows_terminal_ready(scenario, state);
    let directory = start_script(scenario, state, 5_000);
    expect(scenario, &before(1, 30));
    go(scenario, &directory);
    let mut rest = after(1, 30);
    rest.push("sound: skipped-lines skipped 4941 lines".to_owned());
    rest.extend(after(4_972, 5_000));
    rest.push(PROMPT.to_owned());
    expect(scenario, &rest);
}

/// `conhost_history_flood`.
pub(crate) fn body_history_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    console_host_ready(scenario, state);
    let directory = start_script(scenario, state, 5_000);
    expect(scenario, &before(1, 30));
    go(scenario, &directory);
    let mut rest = after(1, 30);
    rest.push("sound: skipped-lines skipped 4941 lines".to_owned());
    rest.extend(after(4_972, 5_000));
    rest.push(PROMPT.to_owned());
    expect(scenario, &rest);
}

/// `windows_terminal_history_flood_during_group`.
pub(crate) fn body_history_during_group_windows_terminal(
    scenario: &mut Scenario,
    state: &mut ScenarioState,
) {
    windows_terminal_ready(scenario, state);
    let directory = start_script(scenario, state, 5_000);
    expect(scenario, &before(1, 1));
    go(scenario, &directory);
    let mut rest = before(2, 30);
    rest.push("sound: skipped-lines skipped 4971 lines".to_owned());
    rest.extend(after(4_972, 5_000));
    rest.push(PROMPT.to_owned());
    expect(scenario, &rest);
}

/// `conhost_history_flood_during_group`.
pub(crate) fn body_history_during_group_console_host(
    scenario: &mut Scenario,
    state: &mut ScenarioState,
) {
    console_host_ready(scenario, state);
    let directory = start_script(scenario, state, 5_000);
    expect(scenario, &before(1, 1));
    go(scenario, &directory);
    let mut rest = before(2, 30);
    rest.push("sound: skipped-lines skipped 4971 lines".to_owned());
    rest.extend(after(4_972, 5_000));
    rest.push(PROMPT.to_owned());
    expect(scenario, &rest);
}

/// `windows_terminal_scrollback_overflow`: N = 9,001. Windows Terminal takes
/// a write of 12,000 lines whole before a client hears that its text
/// changed (measured 2026-10-08: the first read after the write found line
/// 11,881 in two runs of three), so its burst's first lines may have left
/// the history before any read; the script writes them first, 30 lines in
/// a write of their own, and the rest once the first has been heard,
/// while the group plays.
pub(crate) fn body_overflow_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    windows_terminal_ready(scenario, state);
    let directory = start_script_with(scenario, state, "12000 30");
    expect(scenario, &before(1, 30));
    go(scenario, &directory);
    expect(scenario, &after(1, 1));
    second_go(scenario, &directory);
    let mut rest = after(2, 30);
    rest.push("sound: skipped-lines skipped more than 9001 lines".to_owned());
    rest.extend(after(11_972, 12_000));
    rest.push(PROMPT.to_owned());
    expect(scenario, &rest);
}

/// `conhost_scrollback_overflow`: N = 8,971.
pub(crate) fn body_overflow_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    console_host_ready(scenario, state);
    let directory = start_script(scenario, state, 12_000);
    expect(scenario, &before(1, 30));
    go(scenario, &directory);
    let mut rest = after(1, 30);
    rest.push("sound: skipped-lines skipped more than 8971 lines".to_owned());
    rest.extend(after(11_972, 12_000));
    rest.push(PROMPT.to_owned());
    expect(scenario, &rest);
}

/// `windows_terminal_scrollback_overflow_during_group`: N = 9,001.
pub(crate) fn body_overflow_during_group_windows_terminal(
    scenario: &mut Scenario,
    state: &mut ScenarioState,
) {
    windows_terminal_ready(scenario, state);
    let directory = start_script(scenario, state, 12_000);
    expect(scenario, &before(1, 1));
    go(scenario, &directory);
    let mut rest = before(2, 30);
    rest.push("sound: skipped-lines skipped more than 9001 lines".to_owned());
    rest.extend(after(11_972, 12_000));
    rest.push(PROMPT.to_owned());
    expect(scenario, &rest);
}

/// `conhost_scrollback_overflow_during_group`: N = 8,971.
pub(crate) fn body_overflow_during_group_console_host(
    scenario: &mut Scenario,
    state: &mut ScenarioState,
) {
    console_host_ready(scenario, state);
    let directory = start_script(scenario, state, 12_000);
    expect(scenario, &before(1, 1));
    go(scenario, &directory);
    let mut rest = before(2, 30);
    rest.push("sound: skipped-lines skipped more than 8971 lines".to_owned());
    rest.extend(after(11_972, 12_000));
    rest.push(PROMPT.to_owned());
    expect(scenario, &rest);
}
