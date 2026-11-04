//! A flood past the terminal's history arriving in one write while a group
//! plays, as `windows_terminal_scrollback_overflow_during_group` in Windows
//! Terminal and `conhost_scrollback_overflow_during_group` in the console
//! host, each its own code (`books/dev/src/reference/testing.md`). These are the acceptance
//! tests of the terminal reading package (`docs/parity/terminals.md`, "The
//! backlog" and "Reading a terminal on demand"): when Core reaches the end
//! of a group it asks, and the outpost reads up to the end; the anchor's
//! line has left a full history, so the read is lost, Core reads the whole
//! document, about 1.1 million characters, through each terminal's own UI
//! Automation text provider, and Verbatim says "skipped more than N lines".
//! The counts and the lost read's logic are covered at the same sizes by
//! `crates/verbatim-core/tests/terminal_floods.rs`; these scenarios keep the
//! real providers' whole-document read and a real write landing against a
//! real hold.
//!
//! Both limits are 15 ([`HALF`]), half their default, not the 10 of the
//! other flood scenarios: a write of 12,000 lines lands while a group
//! plays, and Core asks for what came meanwhile as it hands the group's
//! last line to speech, so the group must last longer than the write.
//!
//! The written script `phased.ps1` prints "before line 1" to "before line
//! 15", a group's worth, in one write, so that one read takes all 15 and
//! Core, with a group's worth of lines waiting, tells the outpost to hold
//! before the first of them is heard. That write is one call of the raw
//! standard output stream's `Write` with the text encoded in the console's
//! output encoding, which Windows PowerShell 5.1's console stream passes to
//! one `WriteFile`, unbuffered; `[Console]::Out` would not do, since it is
//! a `StreamWriter` with a 256-character buffer and automatic flushing, and
//! so writes the lines in pieces. The script then waits, on its folder's
//! change notifications, for a go file, which the scenario writes as soon
//! as the first before line has been heard in full, and then writes its
//! second part, "after line 1" to "after line 12000", in chunks of whole
//! lines of at most 256 characters, one `WriteFile` each, so that no read
//! finds a line whose digits are not yet written. The second part joins the
//! burst, and is skipped down to its last lines. The prompt follows, and
//! counts as a line like any other: the last 15 lines are 14 after lines
//! and the prompt. The outpost reads nothing until Core asks as the group's
//! last line is handed to speech, 13 lines after the go file, and Windows
//! Terminal takes a write of 10,000 lines in 1.5 to 1.7 seconds
//! (`books/dev/src/reference/performance.md`, "A terminal flood"), long before those lines are
//! heard.
//!
//! Verbatim says "skipped more than N lines", N being the number of lines
//! the terminal's buffer holds, scrollback and visible rows together, less
//! the last lines spoken: 9,031 less 15 in Windows Terminal and 9,001 less
//! 15 in the console host. Both buffers were measured on 2026-10-08 by
//! opening each terminal as the harness does, writing 12,000 numbered lines
//! in one write, and reading its text area's whole document through UI
//! Automation's text pattern:
//!
//! - Windows Terminal (`--size 120,30`, its default history) held 9,031
//!   lines: 9,001 of scrollback and 30 rows ([`WINDOWS_TERMINAL_HOLDS`]).
//! - The console host (buffer set to 9,001 lines by the start script, as
//!   `[Console]::BufferHeight` then reports) held 9,001 lines in all
//!   ([`CONSOLE_HOST_HOLDS`]).

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT};
use super::terminal_flood::HALF;
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The go file the script waits for before its second part.
const GO_FILE: &str = "phased-go";

/// The longest the flood and the speech it causes may take between one
/// utterance and the next; it only bounds a hang.
const FLOOD_STEP: Duration = Duration::from_secs(120);

/// How many lines Windows Terminal's buffer holds, scrollback and rows.
const WINDOWS_TERMINAL_HOLDS: u32 = 9_031;

/// How many lines the console host's buffer holds.
const CONSOLE_HOST_HOLDS: u32 = 9_001;

/// The script's parameter: its second part's length.
const SCRIPT_PARAMETERS: &str = "param([int]$After)\r\n";

/// The script after its parameter and `$Before`: `$Before` lines in one
/// `WriteFile`, the go file, then `$After` lines in chunks of whole lines.
const SCRIPT: &str = "function Wait-Go($name) {\r\n\
\x20   $watcher = New-Object IO.FileSystemWatcher($PSScriptRoot, $name)\r\n\
\x20   if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $name))) { $null = $watcher.WaitForChanged('Created') }\r\n\
\x20   $watcher.Dispose()\r\n\
}\r\n\
function Write-After($from, $to) {\r\n\
\x20   $chunks = New-Object Collections.Generic.List[string]\r\n\
\x20   $chunk = ''\r\n\
\x20   foreach ($line in $from..$to) {\r\n\
\x20       $text = \"after line $line`r`n\"\r\n\
\x20       if ($chunk.Length + $text.Length -gt 256) { $chunks.Add($chunk); $chunk = '' }\r\n\
\x20       $chunk += $text\r\n\
\x20   }\r\n\
\x20   $chunks.Add($chunk)\r\n\
\x20   $out = [Console]::OpenStandardOutput()\r\n\
\x20   foreach ($chunk in $chunks) {\r\n\
\x20       $bytes = [Console]::OutputEncoding.GetBytes($chunk)\r\n\
\x20       $out.Write($bytes, 0, $bytes.Length)\r\n\
\x20   }\r\n\
}\r\n\
$before = foreach ($line in 1..$Before) { \"before line $line\" }\r\n\
$bytes = [Console]::OutputEncoding.GetBytes(($before -join \"`r`n\") + \"`r`n\")\r\n\
[Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)\r\n\
Wait-Go 'phased-go'\r\n\
Write-After 1 $After\r\n";

/// The script, its first part a group's worth of lines at [`HALF`].
fn script() -> String {
    format!("{SCRIPT_PARAMETERS}$Before = {HALF}\r\n{SCRIPT}")
}

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
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the scenario's setup opens a terminal window");
    };
    let directory = directory.clone();
    terminal::type_with_echo(
        scenario,
        &format!(r".\phased.ps1 {count}"),
        terminal::Echo::Shown,
    );
    directory
}

/// Writes the go file, letting the script write its second part: in
/// compare mode a step of its own, apart from the command's.
fn go(scenario: &mut Scenario, directory: &str) {
    scenario.begin_action("let the script write its second part");
    scenario
        .write_agent_file(&format!(r"{directory}\{GO_FILE}"), b"")
        .expect("lets the script write its second part");
}

/// Asserts exactly `speech`, in order, each heard in full.
fn expect(scenario: &mut Scenario, speech: &[String]) {
    let speech: Vec<&str> = speech.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&speech, FLOOD_STEP);
}

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "phased-flood", &[("phased.ps1", &script())])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "phased-flood", &[("phased.ps1", &script())])
}

/// Asserts Windows Terminal's announcement and the prompt read.
fn windows_terminal_ready(scenario: &mut Scenario, state: &ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
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

/// The skip past a terminal whose buffer `holds` lines, then the last lines
/// of 12,000 and the prompt.
fn past_history(holds: u32) -> Vec<String> {
    let group = u32::from(HALF);
    let mut rest = vec![format!(
        "sound: skipped-lines skipped more than {} lines",
        holds - group
    )];
    rest.extend(after(12_000 - group + 2, 12_000));
    rest.push(PROMPT.to_owned());
    rest
}

/// `windows_terminal_scrollback_overflow_during_group`.
pub(crate) fn body_overflow_during_group_windows_terminal(
    scenario: &mut Scenario,
    state: &mut ScenarioState,
) {
    windows_terminal_ready(scenario, state);
    let directory = start_script(scenario, state, 12_000);
    expect(scenario, &before(1, 1));
    go(scenario, &directory);
    let mut rest = before(2, u32::from(HALF));
    rest.extend(past_history(WINDOWS_TERMINAL_HOLDS));
    expect(scenario, &rest);
}

/// `conhost_scrollback_overflow_during_group`.
pub(crate) fn body_overflow_during_group_console_host(
    scenario: &mut Scenario,
    state: &mut ScenarioState,
) {
    console_host_ready(scenario, state);
    let directory = start_script(scenario, state, 12_000);
    expect(scenario, &before(1, 1));
    go(scenario, &directory);
    let mut rest = before(2, u32::from(HALF));
    rest.extend(past_history(CONSOLE_HOST_HOLDS));
    expect(scenario, &rest);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_past_the_history_skips_all_it_holds_but_the_last_lines() {
        assert_eq!(
            past_history(WINDOWS_TERMINAL_HOLDS)[0],
            "sound: skipped-lines skipped more than 9016 lines"
        );
        let speech = past_history(CONSOLE_HOST_HOLDS);
        assert_eq!(
            speech[0],
            "sound: skipped-lines skipped more than 8986 lines"
        );
        assert_eq!(speech[1], "after line 11987");
        assert_eq!(speech.len(), 16);
        assert_eq!(speech[15], PROMPT);
    }

    #[test]
    fn the_script_writes_a_group_s_worth_of_before_lines() {
        assert!(script().starts_with("param("));
        assert!(script().contains("$Before = 15\r\n"));
    }
}
