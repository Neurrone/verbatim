//! Keys pressed while a terminal's output is being spoken, or at once
//! after another key (`docs/parity/terminals.md`, "The backlog" and
//! "Typing"; Dickson, 2026-10-07 and 2026-10-09), each scenario in both
//! terminals as its own code (`docs/testing.md`). The shared setup is
//! described in the `terminal` module.
//!
//! - `*_control_flood`: Control while a flood is being spoken, as in NVDA
//!   (`docs/parity/terminals.md`, "New terminal output"), with both limits
//!   lowered to 10, so the flood's last line is queued sooner; what Control
//!   cuts off does not depend on them. First `echo one` is entered twice,
//!   each heard as "one" and the prompt. Then `gated.ps1` writes "flood
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
//!
//!   Then Shift while the first line of `short.ps1`'s output plays pauses
//!   speech, and Shift again resumes it: each waits for Verbatim to report
//!   speech paused, then resumed, and asserts it. Pausing acts on speech,
//!   not on the output's size, so the script prints only "flood line 1" to
//!   "flood line 10", within the lowered limits, so nothing is skipped.
//!   Nothing is cut off or lost: the first line and the two queued behind
//!   it are heard in full, then the rest and the prompt.
//!
//!   Then the command marks (`docs/design/terminal-model.md`, "Features
//!   and settings"): `echo three` is entered, heard as "three" and the
//!   prompt, and Enter on each typed line has marked it.
//!   Verbatim+Alt+Up Arrow six times moves the review cursor to each
//!   command's line and speaks it, newest first: "echo three", `short.ps1`'s
//!   line, `gated.ps1`'s line, and "echo one" twice; the last three lie
//!   more than a hundred rows above the prompt, above the rows Core keeps,
//!   so each is found by the outpost's search, the match nearest its own
//!   mark. A seventh press says nothing, past the oldest mark, and
//!   Verbatim+Alt+Down Arrow goes back to the newer "echo one", then to
//!   `gated.ps1`'s line. Last, `cls`, heard as the prompt, which in both
//!   terminals erases the history with the marks' lines
//!   (`docs/design/terminal-model.md`, "The terminals"), so
//!   Verbatim+Alt+Up Arrow says nothing. NVDA has no command marks, so each press is Verbatim's own
//!   (`docs/parity/terminals.md`, "Reading a terminal on demand"); the
//!   marks steps come after Control's timing-dependent part, so they are
//!   asserted exactly.
//! - `*_line_key_flood`: Up Arrow while a flood's first line plays, once
//!   the flood has ended and the shell shows its prompt (Core receives the
//!   caret on it). `long.ps1` prints 200 lines of twenty words, so the
//!   first is still playing when the flood ends. Up Arrow cuts the output
//!   off, as any key does, and recalls the command, the caret moving to
//!   its end, so the caret's line is spoken as the key's: "ready>
//!   .\long.ps1". NVDA, captured live on 2026-10-09, says the recalled line
//!   too. Before the flood, `echo one` is run, then Up Arrow, which says the
//!   recalled line, "ready> echo one", and once that is heard "x" typed,
//!   at human speed: the "x" is typing, echoed, not output, and not cut off
//!   by its own key; then Enter runs `echo onex`, answered "onex" and the
//!   prompt (Dickson, 2026-10-26: one fixed outcome at human speed, where
//!   Up Arrow and "x" pressed at once gave "echo onex" or the line and then
//!   the "x" by where a read fell between them).

use std::io;

use super::terminal::{self, PROMPT};
use super::terminal_flood::{FLOOD_STEP, line, wait_for_prompt, watch_for_prompt};
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

/// Ten numbered lines, fewer than "Lines spoken in full", so every one is
/// heard.
const SHORT_SCRIPT: &str = "for ($line = 1; $line -le 10; $line++) { \"flood line $line\" }\r\n";

/// How many lines [`SHORT_SCRIPT`] prints.
const SHORT_LINES: u32 = 10;

/// Two hundred lines of twenty words.
const LONG_SCRIPT: &str = "for ($line = 1; $line -le 200; $line++) { \"flood line $line one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen\" }\r\n";

/// Line `n` of `long.ps1`.
fn long_line(n: u32) -> String {
    format!(
        "flood line {n} one two three four five six seven eight nine ten eleven twelve \
         thirteen fourteen fifteen sixteen seventeen"
    )
}

/// Asserts the announcement as a terminal of this scenario takes the focus
/// with nothing written yet, and the first prompt: Windows Terminal's
/// names its terminal for its tab, the console host's drops the name, and
/// both say "blank" for the empty line.
fn opened(scenario: &mut Scenario, state: &ScenarioState, windows_terminal: bool) {
    if windows_terminal {
        terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    } else {
        let window = format!("{} window", terminal::title(state));
        terminal::expect_prompt_read(scenario, state, &[&window, "terminal", "blank"]);
    }
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
        &[
            ("gated.ps1", &scripts_control()),
            ("short.ps1", SHORT_SCRIPT),
        ],
    )
}

pub(crate) fn setup_control_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(
        scenario,
        "control-flood",
        &[
            ("gated.ps1", &scripts_control()),
            ("short.ps1", SHORT_SCRIPT),
        ],
    )
}

/// `echo one` entered twice, before the flood, for the marks steps.
fn echo_one_twice(scenario: &mut Scenario) {
    for _ in 0..2 {
        terminal::type_with_echo(scenario, "echo one", terminal::Echo::Shown);
        scenario.speech().expect(&["one", PROMPT]);
    }
}

/// Presses `gesture`, a move between command marks, and asserts that
/// exactly `heard` is read, or nothing when `heard` is `None`.
fn mark(scenario: &mut Scenario, gesture: &str, heard: Option<&str>) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.deviates_from_nvda("docs/parity/terminals.md", "Reading a terminal on demand");
    match heard {
        Some(heard) => scenario.speech().expect(&[heard]),
        None => scenario.expect_nothing_more(),
    }
}

const UP: &str = "kb:verbatim+alt+uparrow";
const DOWN: &str = "kb:verbatim+alt+downarrow";

/// `echo three` entered, then the moves between the commands' marks.
fn marks_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, "echo three", terminal::Echo::Shown);
    scenario.speech().expect(&["three", PROMPT]);
    mark(scenario, UP, Some("ready> echo three"));
    mark(scenario, UP, Some(r"ready> .\short.ps1"));
    mark(scenario, UP, Some(r"ready> .\gated.ps1"));
    mark(scenario, UP, Some("ready> echo one"));
    mark(scenario, UP, Some("ready> echo one"));
    mark(scenario, UP, None);
    mark(scenario, DOWN, Some("ready> echo one"));
    mark(scenario, DOWN, Some(r"ready> .\gated.ps1"));
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

/// Shift twice while the output's first line plays: paused and resumed,
/// nothing lost.
fn shift_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, r".\short.ps1", terminal::Echo::Shown);
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
    let rest: Vec<String> = (4..=SHORT_LINES)
        .map(line)
        .chain(std::iter::once(PROMPT.to_owned()))
        .collect();
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&rest, FLOOD_STEP);
}

/// `windows_terminal_control_flood`.
pub(crate) fn body_control_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, true);
    echo_one_twice(scenario);
    control_steps(scenario, &directory(state));
    shift_steps(scenario);
    marks_steps(scenario);
    // Windows Terminal's clear erases the history with the marks' lines.
    terminal::type_with_echo(scenario, "cls", terminal::Echo::Shown);
    scenario.speech().expect(&[PROMPT]);
    mark(scenario, UP, None);
}

/// `conhost_control_flood`.
pub(crate) fn body_control_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, false);
    echo_one_twice(scenario);
    control_steps(scenario, &directory(state));
    shift_steps(scenario);
    marks_steps(scenario);
    // The console host's `cls` erases the history too, with the marks'
    // lines.
    terminal::type_with_echo(scenario, "cls", terminal::Echo::Shown);
    scenario.speech().expect(&[PROMPT]);
    mark(scenario, UP, None);
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

/// A command run, recalled with Up Arrow, typed after once the recalled
/// line is heard, and run.
fn up_typing_steps(scenario: &mut Scenario) {
    terminal::type_with_echo(scenario, "echo one", terminal::Echo::Shown);
    scenario.speech().expect(&["one", PROMPT]);
    scenario.send_keys(&["uparrow"]).expect("presses Up");
    scenario.speech().expect(&["ready> echo one"]);
    scenario.type_text("x").expect("types x");
    // Dickson, 2026-10-26: the "x" is echoed as typing.
    scenario.speech().expect(&["x"]);
    scenario.send_keys(&["enter"]).expect("presses Enter");
    scenario.speech().expect(&["onex", PROMPT]);
}

/// `windows_terminal_line_key_flood`.
pub(crate) fn body_line_key_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, true);
    up_typing_steps(scenario);
    line_key_steps(scenario);
}

/// `conhost_line_key_flood`.
pub(crate) fn body_line_key_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    opened(scenario, state, false);
    up_typing_steps(scenario);
    line_key_steps(scenario);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_short_script_prints_its_lines() {
        assert!(SHORT_SCRIPT.contains(&format!("-le {SHORT_LINES};")));
    }

    #[test]
    fn a_long_line_is_what_the_script_prints() {
        assert!(LONG_SCRIPT.contains(&long_line(1).replace("line 1 ", "line $line ")));
    }
}
