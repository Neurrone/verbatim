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
//! heard as its first lines, "skipped more than N lines", and its last
//! lines, the prompt the last of them (Dickson, 2026-10-25). All three
//! scenarios lower both limits: `windows_terminal_clear_flood` to 15
//! ([`HALF`]), since its 9,100-line write must land while the cleared
//! lines' group plays, the others to 10 ([`LOWERED`]); the counts and
//! ranges below follow from those limits.
//!
//! `fresh.ps1` writes "fill line 1" to "fill line 9100" in one call of the
//! raw standard output stream's `Write`, which Windows PowerShell 5.1's
//! console stream passes to one `WriteFile`; the console host takes such
//! a write whole before the outpost's first read (`terminal_overflow`). The
//! console host's buffer holds 9,001 rows: the command's row and the 9,100
//! lines need 9,102 rows with the caret's, so the first 101 rows left the
//! buffer, and it holds fill lines 101 to 9,100 and the prompt, 9,001
//! lines: 10 spoken, 10 last, "skipped more than 8981 lines".
//!
//! Windows Terminal may read a write of that size before it is whole
//! (`terminal_overflow`), so `clear.ps1` makes it land while Core holds the
//! terminal. It writes "before line 1" to "before line 10" in one write, a
//! group's worth, so Core holds the outpost; once the first has been heard
//! the scenario writes a go file, and the script clears the screen with
//! `Clear-Host`, which in Windows Terminal erases the screen and the
//! history, and writes "cleared line 1" to "cleared line 10" in one write
//! with no line break after the last, so the screen still starts at the
//! document's start. The read after the group's last line finds the clear,
//! a new screen setting the screen before it aside, and the cleared lines
//! a group of their own, so Core holds again. Once "cleared line 1" has
//! been heard the scenario writes a second go file, and the script writes
//! the 9,100 fill lines in one write while Core holds. Windows Terminal's
//! document then holds 9,031 rows, its history and its screen: fill lines
//! 71 to 9,100 and the prompt, so 10 last and "skipped more than 9021
//! lines" once the cleared lines' group is heard.
//!
//! `windows_terminal_cleared_flood` is Windows Terminal's clear followed by
//! more than a screen of output before the next read, the document then
//! shorter than at that read, as `clear; dir` typed as one line in a
//! terminal with some history: the read's start is refused, it keeps
//! nothing, its screen does not start at the document's start and no
//! screen is aside, so it is a lost read, and the whole document, holding
//! nothing of the last read, is all new, counted as about (Dickson,
//! 2026-10-25). `old.ps1` first writes "old line 1" to "old line 200" in
//! one write, a flood heard as any flood, its count exact: "old line 1" to
//! "old line 10", "skipped 181 lines", "old line 192" to "old line 200" and
//! the prompt. `cleared.ps1` then writes "before line 1" to "before line
//! 10" in one write, so Core holds the terminal, and once the first has
//! been heard the scenario writes a go file, and the script clears with
//! `Clear-Host`, erasing the screen and the history, and writes "cleared
//! line 1" to "cleared line 100" in one write. The read after the group's
//! last line finds a document of 101 rows, the cleared lines and the
//! prompt, against about 213 at the last read: it joins the burst, skipped
//! down to its last 10, "skipped about 91 lines".

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT};
use super::terminal_flood::{HALF, LOWERED};
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

/// A group's worth of lines (`$Group`), the go file, the clear and a
/// group's worth of cleared lines, the second go file, then the fill
/// lines, each in one raw write.
const CLEAR_SCRIPT: &str = "function Wait-Go($name) {\r\n\
\x20   $watcher = New-Object IO.FileSystemWatcher($PSScriptRoot, $name)\r\n\
\x20   if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $name))) { $null = $watcher.WaitForChanged('Created') }\r\n\
\x20   $watcher.Dispose()\r\n\
}\r\n\
function Write-Raw($text) {\r\n\
\x20   $bytes = [Console]::OutputEncoding.GetBytes($text)\r\n\
\x20   [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)\r\n\
}\r\n\
$before = foreach ($line in 1..$Group) { \"before line $line\" }\r\n\
Write-Raw (($before -join \"`r`n\") + \"`r`n\")\r\n\
Wait-Go 'clear-go'\r\n\
Clear-Host\r\n\
$cleared = foreach ($line in 1..$Group) { \"cleared line $line\" }\r\n\
Write-Raw ($cleared -join \"`r`n\")\r\n\
Wait-Go 'clear-go-2'\r\n\
$fill = foreach ($line in 1..9100) { \"fill line $line\" }\r\n\
Write-Raw (\"`r`n\" + ($fill -join \"`r`n\") + \"`r`n\")\r\n";

/// The go file the second clearing script waits for.
const CLEARED_GO_FILE: &str = "cleared-go";

/// The old lines, in one raw write.
const OLD_SCRIPT: &str = "$old = foreach ($line in 1..200) { \"old line $line\" }\r\n\
$bytes = [Console]::OutputEncoding.GetBytes(($old -join \"`r`n\") + \"`r`n\")\r\n\
[Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)\r\n";

/// A group's worth of lines (`$Group`), the go file, the clear, then the
/// cleared lines, each in one raw write.
const CLEARED_SCRIPT: &str = "function Wait-Go($name) {\r\n\
\x20   $watcher = New-Object IO.FileSystemWatcher($PSScriptRoot, $name)\r\n\
\x20   if (-not (Test-Path -LiteralPath (Join-Path $PSScriptRoot $name))) { $null = $watcher.WaitForChanged('Created') }\r\n\
\x20   $watcher.Dispose()\r\n\
}\r\n\
function Write-Raw($text) {\r\n\
\x20   $bytes = [Console]::OutputEncoding.GetBytes($text)\r\n\
\x20   [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)\r\n\
}\r\n\
$before = foreach ($line in 1..$Group) { \"before line $line\" }\r\n\
Write-Raw (($before -join \"`r`n\") + \"`r`n\")\r\n\
Wait-Go 'cleared-go'\r\n\
Clear-Host\r\n\
$cleared = foreach ($line in 1..100) { \"cleared line $line\" }\r\n\
Write-Raw (($cleared -join \"`r`n\") + \"`r`n\")\r\n";

/// `script` with `$Group` set to a group's worth of lines, `group`.
fn with_group(script: &str, group: u16) -> String {
    format!("$Group = {group}\r\n{script}")
}

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
    terminal::open_windows_terminal(
        scenario,
        "clear-flood",
        &[("clear.ps1", &with_group(CLEAR_SCRIPT, HALF))],
    )
}

pub(crate) fn setup_cleared_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(
        scenario,
        "cleared-flood",
        &[
            ("old.ps1", OLD_SCRIPT),
            ("cleared.ps1", &with_group(CLEARED_SCRIPT, LOWERED)),
        ],
    )
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "fresh-flood", &[("fresh.ps1", FRESH_SCRIPT)])
}

/// The rest of the cleared lines' group, the skip past Windows Terminal's
/// 9,031 lines, and the last fill lines and the prompt.
fn clear_rest() -> Vec<String> {
    let group = u32::from(HALF);
    let mut rest = numbered("cleared line", 2, group);
    rest.push(format!(
        "sound: skipped-lines skipped more than {} lines",
        9_031 - group
    ));
    rest.extend(numbered("fill line", 9_100 - group + 2, 9_100));
    rest.push(PROMPT.to_owned());
    rest
}

/// `windows_terminal_clear_flood`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    terminal::type_with_echo(scenario, r".\clear.ps1", terminal::Echo::Shown);
    expect(scenario, &numbered("before line", 1, 1));
    go(scenario, state, GO_FILE);
    let mut cleared = numbered("before line", 2, u32::from(HALF));
    cleared.extend(numbered("cleared line", 1, 1));
    expect(scenario, &cleared);
    go(scenario, state, SECOND_GO_FILE);
    expect(scenario, &clear_rest());
}

/// The old lines' flood: 200 lines and the prompt, counted exactly.
fn old_flood() -> Vec<String> {
    let group = u32::from(LOWERED);
    let mut old = numbered("old line", 1, group);
    old.push(format!(
        "sound: skipped-lines skipped {} lines",
        201 - 2 * group
    ));
    old.extend(numbered("old line", 200 - group + 2, 200));
    old.push(PROMPT.to_owned());
    old
}

/// The rest of the before lines' group, then the cleared document of 100
/// lines and the prompt, all new, counted as about.
fn cleared_rest() -> Vec<String> {
    let group = u32::from(LOWERED);
    let mut rest = numbered("before line", 2, group);
    rest.push(format!(
        "sound: skipped-lines skipped about {} lines",
        101 - group
    ));
    rest.extend(numbered("cleared line", 100 - group + 2, 100));
    rest.push(PROMPT.to_owned());
    rest
}

/// `windows_terminal_cleared_flood`: the old lines' flood, then the cleared
/// lines all new after the clear, counted as about.
pub(crate) fn body_cleared_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    terminal::type_with_echo(scenario, r".\old.ps1", terminal::Echo::Shown);
    expect(scenario, &old_flood());
    terminal::type_with_echo(scenario, r".\cleared.ps1", terminal::Echo::Shown);
    expect(scenario, &numbered("before line", 1, 1));
    go(scenario, state, CLEARED_GO_FILE);
    expect(scenario, &cleared_rest());
}

/// The fresh console host's fill: the first lines its 9,001-line buffer
/// kept, the skip, and the last lines and the prompt.
fn fresh_fill() -> Vec<String> {
    let group = u32::from(LOWERED);
    let mut heard = numbered("fill line", 101, 100 + group);
    heard.push(format!(
        "sound: skipped-lines skipped more than {} lines",
        9_001 - 2 * group
    ));
    heard.extend(numbered("fill line", 9_100 - group + 2, 9_100));
    heard.push(PROMPT.to_owned());
    heard
}

/// `conhost_fresh_flood`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    terminal::type_with_echo(scenario, r".\fresh.ps1", terminal::Echo::Shown);
    expect(scenario, &fresh_fill());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counts_follow_from_the_lowered_limits() {
        assert_eq!(
            clear_rest()[14],
            "sound: skipped-lines skipped more than 9016 lines"
        );
        assert_eq!(clear_rest()[15], "fill line 9087");
        assert_eq!(old_flood()[10], "sound: skipped-lines skipped 181 lines");
        assert_eq!(old_flood()[11], "old line 192");
        assert_eq!(
            cleared_rest()[9],
            "sound: skipped-lines skipped about 91 lines"
        );
        assert_eq!(cleared_rest()[10], "cleared line 92");
        let fill = fresh_fill();
        assert_eq!(fill.len(), 21);
        assert_eq!(fill[9], "fill line 110");
        assert_eq!(
            fill[10],
            "sound: skipped-lines skipped more than 8981 lines"
        );
        assert_eq!(fill[11], "fill line 9092");
    }

    #[test]
    fn the_scripts_write_a_group_s_worth_of_lines() {
        assert!(with_group(CLEARED_SCRIPT, LOWERED).starts_with("$Group = 10\r\n"));
    }
}
