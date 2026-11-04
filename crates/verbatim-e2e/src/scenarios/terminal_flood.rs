//! A flood of terminal output (milestone M4 item 9 and the M4 exit
//! criteria; `docs/parity/terminals.md`, "The backlog"), as
//! `windows_terminal_flood` in Windows Terminal and `conhost_flood` in the
//! console host. The shared setup is described in the `terminal` module.
//! These two keep the shipped limits, so the suite tests the defaults once;
//! the other flood scenarios lower them ([`LOWERED`], [`HALF`]) to shorten
//! what is heard.
//!
//! The flood policy: the first "Lines spoken in full" lines of a burst
//! (30) are spoken whole; only once they have been spoken does Verbatim
//! look at what arrived meanwhile, and when that is more than the limit it
//! says "skipped N lines" for all but the newest "Last lines to speak"
//! (30) and speaks those; it repeats that after each group until the
//! output stops. The written script `flood.ps1` prints "flood line 1" to
//! "flood line 2000" as fast as the shell can, which, all three floods
//! together, stays within the terminal's scrollback of 9,001 lines, so
//! every line is counted, and ends long before thirty lines have been
//! spoken, then writes how long that took, by its own stopwatch, to
//! `flood-<run>.ms` in the run's folder, the evidence that the script has
//! finished. The prompt that follows is the burst's last line, and counts
//! as a line of its output like any other: the burst is 2001 lines, and
//! its last 30 are flood lines 1972 to 2000 and the prompt. So a flood is
//! heard exactly as: flood lines 1 to 30, "skipped 1941 lines" after the
//! skipped-lines sound, then flood lines 1972 to 2000 and the prompt.
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

use std::io;
use std::time::Duration;

use verbatim_config::Settings;

use super::terminal::{self, PROMPT};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Ending;

pub(crate) use super::no_teardown as teardown;

/// How many lines a flood prints.
const LINES: u32 = 2_000;

/// How many lines a burst speaks whole, and how many it speaks last: the
/// shipped "Lines spoken in full" and "Last lines to speak", which the e2e
/// settings keep.
pub(crate) const GROUP: u32 = 30;

/// Both limits lowered, for a flood scenario whose subject is not the
/// limits' value: ten lines each side of a skip are heard in about a third
/// of the time thirty take, and a flood of 2,000 or 5,000 lines is written
/// long before ten lines have been spoken.
pub(crate) const LOWERED: u16 = 10;

/// Both limits at half their default, for a scenario whose output must land
/// while a group plays, such as a 12,000-line write, which takes Windows
/// Terminal about two seconds (`books/dev/src/reference/performance.md`, "A terminal flood"),
/// or that must hear a 30-row screen whole: 30 lines are not more than 15
/// waiting behind a group of 15, so none is skipped.
pub(crate) const HALF: u16 = 15;

/// The settings of a scenario run at [`LOWERED`]: both limits 10.
pub(crate) fn lowered_limits(settings: &mut Settings) {
    settings.reader.terminal_full_lines = LOWERED;
    settings.reader.terminal_last_lines = LOWERED;
}

/// The settings of a scenario run at [`HALF`]: both limits 15.
pub(crate) fn half_limits(settings: &mut Settings) {
    settings.reader.terminal_full_lines = HALF;
    settings.reader.terminal_last_lines = HALF;
}

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
pub(crate) const FLOOD_STEP: Duration = Duration::from_secs(120);

const OUTPUT_OFF: &str = "report new output off";
const OUTPUT_ON: &str = "report new output on";

pub(crate) fn setup_windows_terminal(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_windows_terminal(scenario, "flood", &[("flood.ps1", SCRIPT)])
}

pub(crate) fn setup_console_host(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    terminal::open_console_host(scenario, "flood", &[("flood.ps1", SCRIPT)])
}

/// "flood line `line`".
pub(crate) fn line(line: u32) -> String {
    format!("flood line {line}")
}

/// Exactly what a flood says with both limits at `group`: the first
/// group, the skipped lines, and the last group, the burst's last `group`
/// lines, of which the prompt is the last.
pub(crate) fn flood_speech(group: u32) -> Vec<String> {
    burst_speech(LINES, group, line)
}

/// What a burst of `lines` lines named by `name`, and the prompt after
/// them, says with both limits at `group`.
pub(crate) fn burst_speech(lines: u32, group: u32, name: fn(u32) -> String) -> Vec<String> {
    let total = lines + 1;
    let mut speech: Vec<String> = (1..=group).map(name).collect();
    speech.push(format!(
        "sound: skipped-lines skipped {} lines",
        total - 2 * group
    ));
    speech.extend((lines - group + 2..=lines).map(name));
    speech.push(PROMPT.to_owned());
    speech
}

/// Waits for the flood `run`'s elapsed-time file, the evidence that its
/// script has finished, and returns its time.
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

/// Runs flood `run` with output reported and asserts exactly what it says
/// at the shipped limits.
pub(crate) fn heard_flood(scenario: &mut Scenario, run: u32) {
    terminal::type_with_echo(
        scenario,
        &format!(r".\flood.ps1 {run}"),
        terminal::Echo::Shown,
    );
    let speech = flood_speech(GROUP);
    let speech: Vec<&str> = speech.iter().map(String::as_str).collect();
    scenario.speech().expect_within(&speech, FLOOD_STEP);
}

/// Step 2: Verbatim+5 during the second flood cuts its first group off and
/// stops the rest.
/// Starts watching, on a thread of its own, for Core receiving the caret
/// on the prompt after the flood whose command has just been typed, Enter
/// not yet pressed (watched any earlier, the caret Core may report on the
/// prompt as typing starts would be taken for it): the flood's
/// script finishing is no evidence that the terminal has shown all of its
/// output. The caret is the evidence, not the prompt's output, because
/// while Core holds the terminal's output the prompt is read in an answer
/// to Core's request, which the subscription does not carry; and with
/// output reporting off, the reading rules say nothing of the prompt, which
/// is not spoken once reporting is back on either, since it is no longer
/// new. The subscription is read as events arrive, since
/// one left unread through a flood falls behind and is disconnected. In
/// compare mode NVDA offers no events, so nothing is watched, and NVDA's
/// quiet period, waited for before the next input, is the only evidence
/// there is.
pub(crate) fn watch_for_prompt(scenario: &mut Scenario) -> Option<std::thread::JoinHandle<()>> {
    if scenario.is_comparing() {
        return None;
    }
    let mut events = scenario
        .subscribe_events()
        .expect("subscribes to Verbatim's events");
    Some(std::thread::spawn(move || {
        terminal::wait_for_caret_on(&mut events, PROMPT, FLOOD_STEP)
            .expect("the terminal shows the prompt after the flood");
    }))
}

/// Waits for the evidence of [`watch_for_prompt`], when it watches.
pub(crate) fn wait_for_prompt(watch: Option<std::thread::JoinHandle<()>>) {
    if let Some(watch) = watch
        && let Err(panic) = watch.join()
    {
        std::panic::resume_unwind(panic);
    }
}

fn output_off_during_flood(scenario: &mut Scenario, directory: &str) {
    terminal::type_hearing(scenario, r".\flood.ps1 2", terminal::Echo::Shown);
    let watch = watch_for_prompt(scenario);
    scenario.send_keys(&["enter"]).expect("presses enter");
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
fn silent_flood(scenario: &mut Scenario, directory: &str) {
    terminal::type_hearing(scenario, r".\flood.ps1 3", terminal::Echo::Shown);
    let watch = watch_for_prompt(scenario);
    scenario.send_keys(&["enter"]).expect("presses enter");
    let _ = elapsed(scenario, directory, 3);
    wait_for_prompt(watch);
    scenario.expect_nothing_more();
    scenario
        .send_gesture("kb:verbatim+5")
        .expect("sends Verbatim+5");
    scenario.speech().expect(&[OUTPUT_ON]);
    terminal::type_with_echo(scenario, "echo back", terminal::Echo::Shown);
    scenario.speech().expect(&["back", PROMPT]);
}

/// `windows_terminal_flood`.
pub(crate) fn body_windows_terminal(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the flood's setup opens a terminal window");
    };
    let directory = directory.clone();
    terminal::expect_windows_terminal_prompt_read(scenario, state, terminal::LAUNCH_REPEATS);
    heard_flood(scenario, 1);
    let _ = elapsed(scenario, &directory, 1);
    output_off_during_flood(scenario, &directory);
    silent_flood(scenario, &directory);
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
    silent_flood(scenario, &directory);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flood_speaks_its_first_and_last_groups_around_one_skip() {
        let speech = flood_speech(GROUP);
        assert_eq!(speech.len(), 2 * GROUP as usize + 1);
        assert_eq!(speech[0], "flood line 1");
        assert_eq!(speech[29], "flood line 30");
        assert_eq!(speech[30], "sound: skipped-lines skipped 1941 lines");
        assert_eq!(speech[31], "flood line 1972");
        assert_eq!(speech[59], "flood line 2000");
        assert_eq!(speech[60], "ready>");
    }

    #[test]
    fn a_flood_at_the_lowered_limits_keeps_ten_lines_each_side() {
        let speech = flood_speech(u32::from(LOWERED));
        assert_eq!(speech.len(), 21);
        assert_eq!(speech[9], "flood line 10");
        assert_eq!(speech[10], "sound: skipped-lines skipped 1981 lines");
        assert_eq!(speech[11], "flood line 1992");
        assert_eq!(speech[20], "ready>");
    }

    #[test]
    fn the_script_prints_every_line() {
        assert!(SCRIPT.contains(&format!("-le {LINES};")));
        assert!(SCRIPT.contains(r#""flood line $line""#));
    }
}
