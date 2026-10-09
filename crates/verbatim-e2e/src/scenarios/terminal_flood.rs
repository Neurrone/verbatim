//! A flood of terminal output (milestone M4 item 9 and the M4 exit
//! criteria; `phase6-design.md`, "Test design decisions"), as
//! `windows_terminal_flood` in Windows Terminal and `conhost_flood` in the
//! console host. The shared setup is described in the `terminal` module.
//!
//! The flood policy: the first "Lines spoken in full" lines of a burst
//! (30) are spoken whole; only once they have been spoken does Verbatim
//! look at what arrived meanwhile, and when that is more than the limit it
//! says "skipped N lines" for all but the newest "Last lines to speak"
//! (30) and speaks those; it repeats that after each group until the
//! output stops. The written script `flood.ps1` prints "flood line 1" to
//! "flood line 2000" as fast as the shell can, which, all four floods
//! together, stays within the terminal's scrollback of 9,001 lines, so
//! every line is counted, and ends long before thirty lines have been
//! spoken, then writes how long that took, by its own stopwatch, to
//! `flood-<run>.ms` in the run's folder. The prompt that follows is the
//! burst's last line, and counts as a line of its output like any other:
//! the burst is 2001 lines, and its last 30 are flood lines 1972 to 2000
//! and the prompt. So a flood is heard exactly as: flood lines 1 to 30,
//! "skipped 1941 lines" after the skipped-lines sound, then flood lines
//! 1972 to 2000 and the prompt.
//!
//! 1. The first flood, with no key pressed, asserted exactly as above.
//! 2. Responsiveness and output reporting off: the second flood's first
//!    line is playing, and its next two queued behind it (output is handed
//!    to speech two ahead of the line playing), when Verbatim+5 is sent;
//!    all three are cut off, "report new output off" is heard in full, and
//!    nothing more of the flood is said.
//! 3. The third flood, with output reporting off throughout: only the
//!    typed command's echo is spoken. Verbatim+5 then says "report new
//!    output on", and `echo back` is answered with exactly "back" and then
//!    the prompt.
//! 4. A fourth flood, with output reported, asserted as the first. The
//!    ratio of its time to the third's, how much reporting the terminal's
//!    output slows it down, is recorded in the run's artifacts as
//!    `wall-time-ratio.txt`, for trends; it varies by machine, so it is not
//!    asserted.
//!
//! `conhost_wrapped_flood` floods the console host while a line that
//! wrapped onto three rows is on its screen: `long.ps1` prints forty
//! numbered words, heard whole, then the first flood is heard exactly as
//! above. The screen's rows are counted as rows, not as its lines, so no
//! row the screen held is read again as one that went by unread.

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT};
use crate::artifacts;
use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Ending;

pub(crate) use super::no_teardown as teardown;

/// How many lines a flood prints.
const LINES: u32 = 2_000;

/// How many lines a burst speaks whole, and how many it speaks last: the
/// e2e settings' "Lines spoken in full" and "Last lines to speak".
const GROUP: u32 = 30;

/// The flood script; its argument names the run's elapsed-time file, which
/// is written under another name and renamed, so it never exists empty for
/// the scenario to read.
pub(crate) const SCRIPT: &str = "param([string]$Run)\r\n\
$watch = [Diagnostics.Stopwatch]::StartNew()\r\n\
for ($line = 1; $line -le 2000; $line++) { \"flood line $line\" }\r\n\
$watch.Stop()\r\n\
[IO.File]::WriteAllText(\"$PSScriptRoot\\flood-$Run.tmp\", [string]$watch.ElapsedMilliseconds)\r\n\
[IO.File]::Move(\"$PSScriptRoot\\flood-$Run.tmp\", \"$PSScriptRoot\\flood-$Run.ms\")\r\n";

/// The longest a flood and the speech it causes may take between one
/// utterance and the next; it only bounds a hang.
const FLOOD_STEP: Duration = Duration::from_secs(120);

const OUTPUT_OFF: &str = "report new output off";
const OUTPUT_ON: &str = "report new output on";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "flood", &[("flood.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "flood", &[("flood.ps1", SCRIPT)])
}

/// Forty numbered words on one line, which wraps onto three rows.
const LONG_SCRIPT: &str = "$words = foreach ($i in 1..40) { \"word$i\" }\r\n\
$words -join ' '\r\n";

pub(crate) fn setup_wrapped_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(
        scenario,
        "wrapped-flood",
        &[("flood.ps1", SCRIPT), ("long.ps1", LONG_SCRIPT)],
    )
}

/// `conhost_wrapped_flood`.
pub(crate) fn body_wrapped_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    terminal::type_with_echo(scenario, r".\long.ps1", terminal::Echo::Shown);
    let long = (1..=40)
        .map(|n| format!("word{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    scenario
        .speech()
        .expect_within(&[long.as_str(), PROMPT], FLOOD_STEP);
    heard_flood(scenario, 1);
}

/// "flood line `line`".
fn line(line: u32) -> String {
    format!("flood line {line}")
}

/// Exactly what a flood says: the first group, the skipped lines, and the
/// last group, the burst's last [`GROUP`] lines, of which the prompt is
/// the last: 29 flood lines and the prompt.
pub(crate) fn flood_speech() -> Vec<String> {
    burst_speech(LINES, line)
}

/// What a burst of `lines` lines named by `name`, and the prompt after
/// them, says.
fn burst_speech(lines: u32, name: fn(u32) -> String) -> Vec<String> {
    let total = lines + 1;
    let mut speech: Vec<String> = (1..=GROUP).map(name).collect();
    speech.push(format!(
        "sound: skipped-lines skipped {} lines",
        total - 2 * GROUP
    ));
    speech.extend((lines - GROUP + 2..=lines).map(name));
    speech.push(PROMPT.to_owned());
    speech
}

/// Waits for the flood `run`'s elapsed-time file and returns its time.
fn elapsed(scenario: &mut Scenario, directory: &str, run: u32) -> Duration {
    let contents = scenario
        .wait_for_agent_file(&format!(r"{directory}\flood-{run}.ms"), FLOOD_STEP)
        .expect("the flood finishes and writes its time");
    let text = String::from_utf8_lossy(&contents);
    let millis: u64 = text
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("flood {run}'s time {text:?} is not a number: {error}"));
    Duration::from_millis(millis)
}

/// Runs flood `run` with output reported and asserts exactly what it says.
pub(crate) fn heard_flood(scenario: &mut Scenario, run: u32) {
    terminal::type_with_echo(
        scenario,
        &format!(r".\flood.ps1 {run}"),
        terminal::Echo::Shown,
    );
    let speech = flood_speech();
    let speech: Vec<&str> = speech.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&speech, FLOOD_STEP);
}

/// Step 2: Verbatim+5 during the second flood cuts its first group off and
/// stops the rest.
/// Starts watching, on a thread of its own, for Core receiving the caret
/// on the prompt after the flood whose command is typed next: the flood's
/// script finishing is no evidence that the terminal has shown all of its
/// output, and output it shows after reporting is turned back on is new
/// output, rightly spoken. The subscription is read as events arrive, since
/// one left unread through a flood falls behind and is disconnected.
fn watch_for_prompt(scenario: &mut Scenario) -> std::thread::JoinHandle<()> {
    let mut events = scenario
        .subscribe_events()
        .expect("subscribes to Verbatim's events");
    std::thread::spawn(move || {
        terminal::wait_for_caret_on(&mut events, PROMPT, FLOOD_STEP)
            .expect("the terminal shows the prompt after the flood");
    })
}

/// Waits for [`watch_for_prompt`]'s evidence.
fn wait_for_prompt(watch: std::thread::JoinHandle<()>) {
    if let Err(panic) = watch.join() {
        std::panic::resume_unwind(panic);
    }
}

fn output_off_during_flood(scenario: &mut Scenario, directory: &str) {
    let watch = watch_for_prompt(scenario);
    terminal::type_with_echo(scenario, r".\flood.ps1 2", terminal::Echo::Shown);
    let first = scenario.speech().expect_started(&line(1));
    let queued = scenario.speech().expect_queued(&[&line(2), &line(3)]);
    scenario
        .send_gesture("kb:verbatim+5")
        .expect("sends Verbatim+5");
    scenario.speech().expect_ended(&first, Ending::Cancelled);
    for heard in &queued {
        scenario.speech().expect_ended(heard, Ending::Cancelled);
    }
    scenario.speech().expect(&[OUTPUT_OFF]);
    let _ = elapsed(scenario, directory, 2);
    wait_for_prompt(watch);
    scenario.expect_nothing_more();
}

/// Step 3: the third flood, silent, then output reporting back on.
fn silent_flood(scenario: &mut Scenario, directory: &str) -> Duration {
    let watch = watch_for_prompt(scenario);
    terminal::type_with_echo(scenario, r".\flood.ps1 3", terminal::Echo::Shown);
    let unreported = elapsed(scenario, directory, 3);
    wait_for_prompt(watch);
    scenario.expect_nothing_more();
    scenario
        .send_gesture("kb:verbatim+5")
        .expect("sends Verbatim+5");
    scenario.speech().expect(&[OUTPUT_ON]);
    terminal::type_with_echo(scenario, "echo back", terminal::Echo::Shown);
    scenario.speech().expect(&["back", PROMPT]);
    unreported
}

/// `windows_terminal_flood`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the flood's setup opens a terminal window");
    };
    let directory = directory.clone();
    let title = terminal::title(state).to_owned();
    terminal::expect_prompt_read(
        scenario,
        state,
        &[
            &format!("{title} window"),
            &format!("{title} terminal"),
            "blank",
        ],
    );
    heard_flood(scenario, 1);
    let _ = elapsed(scenario, &directory, 1);
    output_off_during_flood(scenario, &directory);
    let unreported = silent_flood(scenario, &directory);
    heard_flood(scenario, 4);
    let reported = elapsed(scenario, &directory, 4);
    record_ratio("windows_terminal_flood", reported, unreported);
}

/// `conhost_flood`.
pub(crate) fn body_console_host(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the flood's setup opens a terminal window");
    };
    let directory = directory.clone();
    let title = terminal::title(state).to_owned();
    // The console host's text area has no name.
    terminal::expect_prompt_read(
        scenario,
        state,
        &[&format!("{title} window"), "terminal", "blank"],
    );
    heard_flood(scenario, 1);
    let _ = elapsed(scenario, &directory, 1);
    output_off_during_flood(scenario, &directory);
    let unreported = silent_flood(scenario, &directory);
    heard_flood(scenario, 4);
    let reported = elapsed(scenario, &directory, 4);
    record_ratio("conhost_flood", reported, unreported);
}

/// Records, in the artifacts of the scenario `name`, how much reporting
/// the terminal's output slowed the flood down.
fn record_ratio(name: &str, reported: Duration, unreported: Duration) {
    let ratio = reported.as_secs_f64() / unreported.as_secs_f64().max(0.001);
    let report = format!(
        "flood with output reported: {} ms\nflood with output not reported: {} ms\nwall-time ratio: {ratio:.2}\n",
        reported.as_millis(),
        unreported.as_millis()
    );
    print!("{report}");
    let dir = artifacts::scenario_dir(&artifacts::artifacts_root(), name);
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join("wall-time-ratio.txt"), &report))
        .expect("saves the wall-time ratio with the run's artifacts");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flood_speaks_its_first_and_last_groups_around_one_skip() {
        let speech = flood_speech();
        assert_eq!(speech.len(), 2 * GROUP as usize + 1);
        assert_eq!(speech[0], "flood line 1");
        assert_eq!(speech[29], "flood line 30");
        assert_eq!(speech[30], "sound: skipped-lines skipped 1941 lines");
        assert_eq!(speech[31], "flood line 1972");
        assert_eq!(speech[59], "flood line 2000");
        assert_eq!(speech[60], "ready>");
    }

    #[test]
    fn the_script_prints_every_line() {
        assert!(SCRIPT.contains(&format!("-le {LINES};")));
        assert!(SCRIPT.contains(r#""flood line $line""#));
    }
}
