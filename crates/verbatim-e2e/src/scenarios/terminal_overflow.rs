//! Floods arriving in one write while a group plays and after it has been
//! heard, within the terminal's history and past it, in Windows Terminal
//! and the console host, each its own code (`docs/testing.md`). These are
//! the acceptance tests of the terminal reading package
//! (`docs/parity/terminals.md`, "The backlog" and "Reading a terminal on
//! demand"): when Core reaches the end of a group it asks, and
//! the outpost reads up to the end, so one read gives the count and the
//! newest lines; when the anchor's line has left a full history, Verbatim
//! says "skipped more than N lines". That package has landed, and these
//! scenarios are part of the suite.
//!
//! Both limits are 15 ([`HALF`]), half their default, not the 10 of the
//! other flood scenarios: each of these has a write of 5,000 or 12,000
//! lines land while a group plays, and Core asks for what came meanwhile
//! as it hands the group's last line to speech, so the group must last
//! longer than the write. The counts and ranges below are at 15.
//!
//! The written script `phased.ps1` prints "before line 1" to "before line
//! 15", a group's worth, in one write, so that one read takes all 15 and
//! Core, with a group's worth of lines waiting, tells the outpost to hold
//! before the first of them is heard; written in pieces, a read could take
//! some of them, speech start, and the rest arrive with fewer than a
//! group's worth waiting, so that the outpost read live through the
//! second part's write and a read landing in the middle of it changed what was said. That
//! write is one call of the raw standard output stream's `Write` with the
//! text encoded in the console's output encoding, which Windows PowerShell
//! 5.1's console stream passes to one `WriteFile`, unbuffered;
//! `[Console]::Out` would not do, since it is a `StreamWriter` with a
//! 256-character buffer and automatic flushing, and so writes the lines
//! in pieces. The script then waits, on its folder's change notifications,
//! for a go file, and then writes its second part, "after line 1" onwards,
//! in chunks of whole lines of at most 256 characters, one `WriteFile`
//! each, so that the first read after the go file takes the second part's
//! first lines, and no read finds a line whose digits are not yet written,
//! as one `[Console]::Out.Write` cutting its 256-character pieces in the
//! middle of a line let a read find "after line" alone. Written with one
//! `WriteFile`, the
//! console host took all 12,000 lines before the outpost's first read, and
//! the first line spoken was "after line 3001" (2026-10-25). The prompt
//! follows, and counts as a line like any other: the last 15 lines are 14
//! after lines and the prompt.
//!
//! The go file is written at one of two points:
//!
//! - After all the before lines have been heard in full
//!   (`*_scrollback_overflow`). The burst has ended, so the second part is
//!   a burst of its own, whose first 15 lines are spoken whole.
//! - As soon as the first before line has been heard in full, while the
//!   group still plays (`*_during_group`). The second part joins the
//!   burst, and is skipped down to its last lines.
//!
//! Windows Terminal's `windows_terminal_scrollback_overflow` has the script
//! split its second part: its first 15 lines come in a write of their own,
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
//! whole write landing before any read is tested by
//! `windows_terminal_scrollback_overflow_during_group`, whose write lands
//! while Core holds the terminal: the outpost reads nothing until Core asks
//! as the group's last line is handed to speech, 13 lines after the go
//! file, and Windows Terminal takes a write of 10,000 lines in 1.5 to 1.7
//! seconds (`docs/performance.md`, "A terminal flood"), long before those
//! lines are heard.
//!
//! Within the history, 5,000 lines; past it, 12,000. Past it, the anchor is
//! known to be gone, and Verbatim says "skipped more than N lines", N being
//! the number of lines the terminal's buffer holds, scrollback and visible
//! rows together, less the last lines spoken: 9,031 less 15 in Windows
//! Terminal and 9,001 less 15 in the console host. Both buffers were
//! measured on 2026-10-08 by opening each terminal as the harness does, writing 12,000
//! numbered lines in one write, and reading its text area's whole document
//! through UI Automation's text pattern:
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

/// The second go file, which a script told to split its second part
/// waits for between the two writes.
const SECOND_GO_FILE: &str = "phased-go-2";

/// How many lines Windows Terminal's buffer holds, scrollback and rows.
const WINDOWS_TERMINAL_HOLDS: u32 = 9_031;

/// How many lines the console host's buffer holds.
const CONSOLE_HOST_HOLDS: u32 = 9_001;

/// The script's parameters: its second part's length and, to split it,
/// the length of its first write.
const SCRIPT_PARAMETERS: &str = "param([int]$After, [int]$First = 0)\r\n";

/// The script after its parameters and `$Before`: `$Before` lines in one
/// `WriteFile`, the go file, then `$After` lines in chunks of whole lines;
/// or, given `$First`, the first `$First` of them so, the second go file,
/// and the rest so.
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
if ($First -gt 0) {\r\n\
\x20   Write-After 1 $First\r\n\
\x20   Wait-Go 'phased-go-2'\r\n\
\x20   Write-After ($First + 1) $After\r\n\
} else {\r\n\
\x20   Write-After 1 $After\r\n\
}\r\n";

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

/// Writes the go file, letting the script write its second part: in
/// compare mode a step of its own, apart from the command's.
fn go(scenario: &mut Scenario, directory: &str) {
    scenario.begin_action("let the script write its second part");
    scenario
        .write_agent_file(&format!(r"{directory}\{GO_FILE}"), b"")
        .expect("lets the script write its second part");
}

/// Writes the second go file, letting a script that split its second part
/// write the rest: in compare mode a step of its own.
fn second_go(scenario: &mut Scenario, directory: &str) {
    scenario.begin_action("let the script write the rest of its second part");
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

/// What a burst within the history says once the go file is written as
/// its first before line is heard: the rest of the group, the skip, and
/// the last lines of `after` lines and the prompt.
fn within_history_during_group(after_lines: u32) -> Vec<String> {
    let group = u32::from(HALF);
    let mut rest = before(2, group);
    rest.push(format!(
        "sound: skipped-lines skipped {} lines",
        after_lines + 1 - group
    ));
    rest.extend(after(after_lines - group + 2, after_lines));
    rest.push(PROMPT.to_owned());
    rest
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

/// `windows_terminal_history_flood_during_group`.
pub(crate) fn body_history_during_group_windows_terminal(
    scenario: &mut Scenario,
    state: &mut ScenarioState,
) {
    windows_terminal_ready(scenario, state);
    let directory = start_script(scenario, state, 5_000);
    expect(scenario, &before(1, 1));
    go(scenario, &directory);
    expect(scenario, &within_history_during_group(5_000));
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
    expect(scenario, &within_history_during_group(5_000));
}

/// `windows_terminal_scrollback_overflow`. Windows Terminal takes a write
/// of 12,000 lines whole before a client hears that its text changed
/// (measured 2026-10-08: the first read after the write found line 11,881
/// in two runs of three), so its burst's first lines may have left the
/// history before any read; the script writes them first, a group's worth
/// in a write of their own, and the rest once the first has been heard,
/// while the group plays.
pub(crate) fn body_overflow_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    windows_terminal_ready(scenario, state);
    let group = u32::from(HALF);
    let directory = start_script_with(scenario, state, &format!("12000 {group}"));
    expect(scenario, &before(1, group));
    go(scenario, &directory);
    expect(scenario, &after(1, 1));
    second_go(scenario, &directory);
    let mut rest = after(2, group);
    rest.extend(past_history(WINDOWS_TERMINAL_HOLDS));
    expect(scenario, &rest);
}

/// `conhost_scrollback_overflow`.
pub(crate) fn body_overflow_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    console_host_ready(scenario, state);
    let group = u32::from(HALF);
    let directory = start_script(scenario, state, 12_000);
    expect(scenario, &before(1, group));
    go(scenario, &directory);
    let mut rest = after(1, group);
    rest.extend(past_history(CONSOLE_HOST_HOLDS));
    expect(scenario, &rest);
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
    fn a_burst_within_the_history_keeps_fifteen_lines_each_side() {
        let speech = within_history_during_group(5_000);
        assert_eq!(speech.len(), 30);
        assert_eq!(speech[0], "before line 2");
        assert_eq!(speech[13], "before line 15");
        assert_eq!(speech[14], "sound: skipped-lines skipped 4986 lines");
        assert_eq!(speech[15], "after line 4987");
        assert_eq!(speech[29], PROMPT);
    }

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
