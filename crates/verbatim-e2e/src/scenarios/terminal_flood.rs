//! A flood of terminal output (milestone M4 item 9 and the M4 exit
//! criteria; `phase6-design.md`, "Test design decisions"), in Windows
//! Terminal. The shared setup is described in the `terminal` module.
//!
//! The flood policy: the first "Lines spoken in full" lines of a burst
//! (30) are spoken whole; only once they have been spoken does Verbatim
//! look at what arrived meanwhile, and when that is more than the limit it
//! says "skipped N lines" for all but the newest "Last lines to speak"
//! (30) and speaks those; it repeats that after each group until the
//! output stops. The written script `flood.ps1` prints "flood line 1" to
//! "flood line 3000" as fast as the shell can, which is within the
//! terminal's scrollback and ends long before thirty lines have been
//! spoken, then writes how long that took, by its own stopwatch, to
//! `flood-<run>.ms` in the run's folder. The prompt that follows is the
//! burst's last line, and counts as a line of its output like any other:
//! the burst is 3001 lines, and its last 30 are flood lines 2972 to 3000
//! and the prompt. So a flood is heard exactly as: flood lines 1 to 30,
//! "skipped 2941 lines" after the skipped-lines sound, then flood lines
//! 2972 to 3000 and the prompt.
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

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT, Terminal};
use crate::artifacts;
use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Ending;

pub(crate) use super::no_teardown as teardown;

/// The scenario's name, also its artifacts directory's.
const NAME: &str = "terminal_flood";

/// How many lines a flood prints.
const LINES: u32 = 3_000;

/// How many lines a burst speaks whole, and how many it speaks last: the
/// e2e settings' "Lines spoken in full" and "Last lines to speak".
const GROUP: u32 = 30;

/// The flood script; its argument names the run's elapsed-time file.
pub(crate) const SCRIPT: &str = "param([string]$Run)\r\n\
$watch = [Diagnostics.Stopwatch]::StartNew()\r\n\
for ($line = 1; $line -le 3000; $line++) { \"flood line $line\" }\r\n\
$watch.Stop()\r\n\
[IO.File]::WriteAllText(\"$PSScriptRoot\\flood-$Run.ms\", [string]$watch.ElapsedMilliseconds)\r\n";

/// The longest a flood and the speech it causes may take between one
/// utterance and the next; it only bounds a hang.
const FLOOD_STEP: Duration = Duration::from_secs(120);

const OUTPUT_OFF: &str = "report new output off";
const OUTPUT_ON: &str = "report new output on";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open(
        scenario,
        "flood",
        Terminal::WindowsTerminal,
        &[("flood.ps1", SCRIPT)],
    )
}

/// "flood line `line`".
fn line(line: u32) -> String {
    format!("flood line {line}")
}

/// Exactly what a flood says: the first group, the skipped lines, and the
/// last group, the burst's last [`GROUP`] lines, of which the prompt is
/// the last: 29 flood lines and the prompt.
pub(crate) fn flood_speech() -> Vec<String> {
    let total = LINES + 1;
    let mut speech: Vec<String> = (1..=GROUP).map(line).collect();
    speech.push(format!(
        "sound: skipped-lines skipped {} lines",
        total - 2 * GROUP
    ));
    speech.extend((LINES - GROUP + 2..=LINES).map(line));
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
fn output_off_during_flood(scenario: &mut Scenario, directory: &str) {
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
    scenario.expect_nothing_more();
}

/// Step 3: the third flood, silent, then output reporting back on.
fn silent_flood(scenario: &mut Scenario, directory: &str) -> Duration {
    terminal::type_with_echo(scenario, r".\flood.ps1 3", terminal::Echo::Shown);
    let unreported = elapsed(scenario, directory, 3);
    scenario.expect_nothing_more();
    scenario
        .send_gesture("kb:verbatim+5")
        .expect("sends Verbatim+5");
    scenario.speech().expect(&[OUTPUT_ON]);
    terminal::type_with_echo(scenario, "echo back", terminal::Echo::Shown);
    scenario.speech().expect(&["back", PROMPT]);
    unreported
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the flood's setup opens a terminal window");
    };
    let directory = directory.clone();
    terminal::expect_prompt_read(scenario, state);

    heard_flood(scenario, 1);
    let _ = elapsed(scenario, &directory, 1);
    output_off_during_flood(scenario, &directory);
    let unreported = silent_flood(scenario, &directory);
    heard_flood(scenario, 4);
    let reported = elapsed(scenario, &directory, 4);

    let ratio = reported.as_secs_f64() / unreported.as_secs_f64().max(0.001);
    let report = format!(
        "flood with output reported: {} ms\nflood with output not reported: {} ms\nwall-time ratio: {ratio:.2}\n",
        reported.as_millis(),
        unreported.as_millis()
    );
    print!("{report}");
    let dir = artifacts::scenario_dir(&artifacts::artifacts_root(), NAME);
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
        assert_eq!(speech[30], "sound: skipped-lines skipped 2941 lines");
        assert_eq!(speech[31], "flood line 2972");
        assert_eq!(speech[59], "flood line 3000");
        assert_eq!(speech[60], "ready>");
    }

    #[test]
    fn the_script_prints_every_line() {
        assert!(SCRIPT.contains(&format!("-le {LINES};")));
        assert!(SCRIPT.contains(r#""flood line $line""#));
    }
}
