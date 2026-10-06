//! A flood of terminal output (milestone M4 item 9 and the M4 exit
//! criteria; `phase6-design.md`, "Terminal end-to-end scenarios"), in
//! Windows Terminal, or the console host where Windows Terminal is not
//! installed. The shared setup is described in the `terminal` module.
//!
//! The written script `flood.ps1` prints "flood line 1" to "flood line
//! 10000" as fast as the shell can, which also exceeds Windows Terminal's
//! default scrollback of 9,001 lines, then writes how long that took, in
//! milliseconds by its own stopwatch, to `flood-<run>.ms` in the run's
//! folder: the measurement, and the evidence the flood finished. The
//! scenario runs it four times.
//!
//! 1. The first flood, with no key pressed while it runs. Speech is read
//!    until "ready>" is queued after it, Verbatim's control plane answering
//!    a status request between reads, and then until speech is quiet. The
//!    assertions:
//!    - The flood lines queued are in increasing order, none twice.
//!    - Among the utterances heard in full, at least one is a skipped-lines
//!      utterance (one containing "skipped", its sound marker aside), and
//!      the flood lines and skipped-lines utterances account for every
//!      line exactly: between two flood lines heard, a and b, and before
//!      the first one heard (a being 0), there is no skipped-lines
//!      utterance when b is a + 1; otherwise the skipped-lines utterances
//!      between them say how many lines were skipped and their counts add
//!      up to b - a - 1, unless one of them says "skipped lines" without a
//!      number (the reader lost count when the scrollback overflowed), which
//!      accounts for any gap.
//!    - "flood line 10000" is heard in full, with no skipped-lines
//!      utterance after it, and then "ready>".
//!    - Nothing flood-related (a flood line or a skipped-lines utterance) is
//!      queued after "ready>".
//! 2. Responsiveness: the second flood runs, and while a flood line is
//!    playing, Verbatim+5 is sent. "report new output off" must be queued
//!    within five seconds, with the control plane answering a status
//!    request between reads, so a hang fails the scenario rather than
//!    stalling it, and then be heard in full. Once the flood's file is
//!    written and speech is quiet, nothing flood-related was queued after
//!    the confirmation. (NVDA's report-title command, Verbatim+T, which the
//!    design names here, is not bound in Verbatim; the toggle is a reducer
//!    command answered with known text, so it serves.)
//! 3. The third flood, with output reporting off throughout: once its file
//!    is written and speech is quiet, nothing flood-related and no
//!    "ready>" was queued, only the typed command's echo. Verbatim+5 then
//!    says "report new output on", and `echo back` is answered with
//!    exactly "back" and then "ready>", with nothing flood-related queued
//!    before them.
//! 4. A fourth flood, with output reported and no key pressed, heard out to
//!    "ready>". The wall-time ratio, the M4 exit criterion: its time
//!    divided by the third flood's (output not reported), each measured by
//!    the script's own stopwatch in the same window with the scrollback
//!    already full, is how much reporting the terminal's output slows it
//!    down. The first flood is not the measure, since it starts with an
//!    empty scrollback: in the console host a flood that fills the
//!    scrollback while it is read took about twice as long as one into a
//!    full scrollback, output reported or not, which would be counted
//!    against reporting. The ratio is printed, saved to
//!    `wall-time-ratio.txt` with the scenario's artifacts, and must be
//!    under two. The design describes the ratio as computed from the trace
//!    stages and the run's calibration; the suite has no per-run
//!    calibration yet, so this direct measurement stands in for it.

use std::io;
use std::time::{Duration, Instant};

use super::terminal::{self, PROMPT, Terminal};
use crate::artifacts;
use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Heard;

/// The scenario's name, also its artifacts directory's.
const NAME: &str = "terminal_flood";

/// How many lines a flood prints.
const LINES: u32 = 10_000;

/// The flood script; its argument names the run's elapsed-time file.
const SCRIPT: &str = "param([string]$Run)\r\n\
$watch = [Diagnostics.Stopwatch]::StartNew()\r\n\
for ($line = 1; $line -le 10000; $line++) { \"flood line $line\" }\r\n\
$watch.Stop()\r\n\
[IO.File]::WriteAllText(\"$PSScriptRoot\\flood-$Run.ms\", [string]$watch.ElapsedMilliseconds)\r\n";

/// The longest a flood and the speech it causes may take; it only bounds
/// a failure.
const FLOOD_TIMEOUT: Duration = Duration::from_secs(600);

/// How long a flood line is given to start playing.
const PLAYING_TIMEOUT: Duration = Duration::from_secs(60);

/// How soon Verbatim must answer a command during a flood.
const RESPONSE_BOUND: Duration = Duration::from_secs(5);

/// How long speech is given to go quiet.
const QUIET_TIMEOUT: Duration = Duration::from_secs(30);

/// How much slower the terminal may run while its output is reported.
const RATIO_LIMIT: f64 = 2.0;

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

/// What one utterance says about the flood.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spoken {
    /// A flood line, by number.
    Line(u32),
    /// A skipped-lines utterance, with its count when it gives one.
    Skipped(Option<u64>),
}

/// What `text` says about the flood, or `None` when it is not about it.
/// A sound named in the text (`sound: skipped-lines`) is set aside first,
/// so a sound alone is not a skipped-lines utterance.
fn classify(text: &str) -> Option<Spoken> {
    if let Some(number) = text.strip_prefix("flood line ") {
        return number.parse().ok().map(Spoken::Line);
    }
    let mut words = Vec::new();
    let mut tokens = text.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == "sound:" {
            let _ = tokens.next();
        } else {
            words.push(token);
        }
    }
    let words = words.join(" ");
    if !words.contains("skipped") {
        return None;
    }
    let digits: String = words
        .chars()
        .skip_while(|character| !character.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    Some(Spoken::Skipped(digits.parse().ok()))
}

/// Checks that `spoken`, the flood utterances heard in full in order,
/// account for every line from 1 to `last` (this module's doc comment
/// gives the rules), and that at least one says lines were skipped.
fn account(spoken: &[Spoken], last: u32) -> Result<(), String> {
    let mut previous = 0u32;
    let mut skipped = 0u64;
    let mut uncounted = false;
    let mut any_skipped = false;
    for item in spoken {
        match *item {
            Spoken::Skipped(count) => {
                any_skipped = true;
                match count {
                    Some(count) => skipped += count,
                    None => uncounted = true,
                }
            }
            Spoken::Line(line) => {
                if line <= previous {
                    return Err(format!(
                        "flood line {line} was heard after flood line {previous}"
                    ));
                }
                let gap = u64::from(line - previous - 1);
                if gap == 0 && (skipped > 0 || uncounted) {
                    return Err(format!(
                        "lines were said to be skipped between flood lines {previous} and {line}"
                    ));
                }
                if gap > 0 && !uncounted && skipped != gap {
                    return Err(format!(
                        "{gap} lines between flood lines {previous} and {line} were not spoken, but the skipped-lines utterances between them count {skipped}"
                    ));
                }
                previous = line;
                skipped = 0;
                uncounted = false;
            }
        }
    }
    if previous != last {
        return Err(format!(
            "the last flood line heard in full was {previous}, not {last}"
        ));
    }
    if skipped > 0 || uncounted {
        return Err(format!(
            "lines were said to be skipped after flood line {last}"
        ));
    }
    if !any_skipped {
        return Err("no skipped-lines utterance was heard".to_owned());
    }
    Ok(())
}

/// The texts of `heard` that are about the flood.
fn flood_related(heard: &[Heard]) -> Vec<&str> {
    heard
        .iter()
        .map(|heard| heard.text.as_str())
        .filter(|text| classify(text).is_some())
        .collect()
}

/// Waits for the flood `run`'s elapsed-time file and returns its time.
fn elapsed(scenario: &mut Scenario, directory: &str, run: u32) -> Duration {
    let contents = scenario
        .wait_for_agent_file(&format!(r"{directory}\flood-{run}.ms"), FLOOD_TIMEOUT)
        .expect("the flood finishes and writes its time");
    let text = String::from_utf8_lossy(&contents);
    let millis: u64 = text
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("flood {run}'s time {text:?} is not a number: {error}"));
    Duration::from_millis(millis)
}

/// Step 1: the first flood, with output reported and no key pressed.
fn first_flood(scenario: &mut Scenario) {
    terminal::run_command_after_echo(scenario, r".\flood.ps1 1");
    let heard = terminal::listen_until(scenario, PROMPT, FLOOD_TIMEOUT, true);
    let after = scenario.speech().take_until_quiet(QUIET_TIMEOUT);

    let queued: Vec<u32> = heard
        .iter()
        .filter_map(|heard| match classify(&heard.text) {
            Some(Spoken::Line(line)) => Some(line),
            _ => None,
        })
        .collect();
    assert!(
        queued.windows(2).all(|pair| pair[0] < pair[1]),
        "the flood lines were not queued in increasing order, each once: {queued:?}"
    );

    let speech = scenario.speech();
    let spoken: Vec<Spoken> = heard
        .iter()
        .filter(|heard| speech.ending_of(heard) == Some(verbatim_model::UtteranceEnding::Completed))
        .filter_map(|heard| classify(&heard.text))
        .collect();
    if let Err(failure) = account(&spoken, LINES) {
        panic!(
            "the first flood's speech does not account for every line: {failure}; heard in full: {spoken:?}"
        );
    }
    let late = flood_related(&after);
    assert!(
        late.is_empty(),
        "flood speech was queued after the prompt: {late:?}"
    );
}

/// Step 2: Verbatim+5 answered during the second flood.
fn responsive_during_flood(scenario: &mut Scenario, directory: &str) {
    terminal::run_command_after_echo(scenario, r".\flood.ps1 2");
    scenario
        .speech()
        .expect_playing("flood line", PLAYING_TIMEOUT);
    let asked = Instant::now();
    scenario
        .send_gesture("kb:verbatim+5")
        .expect("sends Verbatim+5");
    let _ = terminal::listen_until(scenario, OUTPUT_OFF, RESPONSE_BOUND, true);
    println!(
        "Verbatim+5 was answered {} ms into the flood's speech",
        asked.elapsed().as_millis()
    );
    let _ = elapsed(scenario, directory, 2);
    let after = scenario.speech().take_until_quiet(QUIET_TIMEOUT);
    let late = flood_related(&after);
    assert!(
        late.is_empty(),
        "flood speech was queued after output reporting was turned off: {late:?}"
    );
}

/// Step 3: the third flood, silent, then output reporting back on.
fn silent_flood(scenario: &mut Scenario, directory: &str) {
    terminal::run_command(scenario, r".\flood.ps1 3");
    let _ = elapsed(scenario, directory, 3);
    let heard = scenario.speech().take_until_quiet(QUIET_TIMEOUT);
    let spoken: Vec<&str> = heard
        .iter()
        .map(|heard| heard.text.as_str())
        .filter(|text| classify(text).is_some() || *text == PROMPT)
        .collect();
    assert!(
        spoken.is_empty(),
        "terminal output was spoken with output reporting off: {spoken:?}"
    );

    scenario
        .send_gesture("kb:verbatim+5")
        .expect("sends Verbatim+5");
    scenario
        .speech()
        .expect_exactly(&[OUTPUT_ON], terminal::STEP_TIMEOUT);
    terminal::run_command_after_echo(scenario, "echo back");
    let heard = terminal::listen_until(scenario, "back", terminal::STEP_TIMEOUT, false);
    let stale = flood_related(&heard);
    assert!(
        stale.is_empty(),
        "output from while reporting was off was spoken once it was on: {stale:?}"
    );
    scenario
        .speech()
        .expect_exactly(&[PROMPT], terminal::STEP_TIMEOUT);
}

/// Step 4: the fourth flood, with output reported and no key pressed, in a
/// scrollback as full as the third's was, for the wall-time ratio; its
/// speech is heard out to the prompt.
fn measured_flood(scenario: &mut Scenario) {
    terminal::run_command_after_echo(scenario, r".\flood.ps1 4");
    let _ = terminal::listen_until(scenario, PROMPT, FLOOD_TIMEOUT, true);
    scenario.speech().wait_until_quiet(QUIET_TIMEOUT);
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the flood's setup opens a terminal window");
    };
    let directory = directory.clone();
    terminal::expect_prompt_read(scenario, state);

    first_flood(scenario);
    responsive_during_flood(scenario, &directory);
    silent_flood(scenario, &directory);
    let unreported = elapsed(scenario, &directory, 3);
    measured_flood(scenario);
    let reported = elapsed(scenario, &directory, 4);

    let ratio = reported.as_secs_f64() / unreported.as_secs_f64().max(0.001);
    let report = format!(
        "flood with output reported: {} ms\nflood with output not reported: {} ms\nwall-time ratio: {ratio:.2}\n",
        reported.as_millis(),
        unreported.as_millis()
    );
    print!("{report}");
    let dir = artifacts::scenario_dir(&artifacts::artifacts_root(), NAME);
    if let Err(error) = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join("wall-time-ratio.txt"), &report))
    {
        eprintln!("could not save the wall-time ratio: {error}");
    }
    assert!(
        ratio < RATIO_LIMIT,
        "reporting the flood's output slowed the terminal by {ratio:.2} times, not under {RATIO_LIMIT}"
    );
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    terminal::close(scenario, &state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utterances_are_classified_by_what_they_say_about_the_flood() {
        assert_eq!(classify("flood line 42"), Some(Spoken::Line(42)));
        assert_eq!(classify("skipped 1 line"), Some(Spoken::Skipped(Some(1))));
        assert_eq!(
            classify("sound: skipped-lines skipped \u{2068}25\u{2069} lines"),
            Some(Spoken::Skipped(Some(25)))
        );
        assert_eq!(classify("skipped lines"), Some(Spoken::Skipped(None)));
        assert_eq!(classify("sound: skipped-lines"), None);
        assert_eq!(classify("ready>"), None);
        assert_eq!(classify("report new output off"), None);
    }

    #[test]
    fn the_script_prints_every_line() {
        assert!(SCRIPT.contains(&format!("-le {LINES};")));
        assert!(SCRIPT.contains(r#""flood line $line""#));
    }

    #[test]
    fn gaps_must_be_counted_exactly() {
        use Spoken::{Line, Skipped};
        assert_eq!(
            account(&[Line(1), Line(2), Skipped(Some(7)), Line(10)], 10),
            Ok(())
        );
        assert_eq!(
            account(
                &[
                    Skipped(Some(4)),
                    Skipped(Some(3)),
                    Line(8),
                    Line(9),
                    Line(10)
                ],
                10
            ),
            Ok(())
        );
        assert!(account(&[Line(1), Skipped(Some(6)), Line(10)], 10).is_err());
        assert!(account(&[Line(1), Line(5), Skipped(Some(5)), Line(10)], 10).is_err());
    }

    #[test]
    fn an_uncounted_skip_accounts_for_any_gap() {
        use Spoken::{Line, Skipped};
        assert_eq!(
            account(
                &[
                    Line(1),
                    Skipped(None),
                    Line(9000),
                    Skipped(Some(999)),
                    Line(10_000)
                ],
                10_000
            ),
            Ok(())
        );
    }

    #[test]
    fn order_end_and_a_skip_are_required() {
        use Spoken::{Line, Skipped};
        assert!(account(&[Skipped(Some(1)), Line(3), Line(2)], 3).is_err());
        assert!(account(&[Skipped(Some(1)), Line(2), Line(2)], 2).is_err());
        assert!(account(&[Skipped(Some(8)), Line(9)], 10).is_err());
        assert!(account(&[Skipped(Some(9)), Line(10), Skipped(Some(1))], 10).is_err());
        assert!(account(&[Line(1), Line(2)], 2).is_err());
        assert!(account(&[Line(1), Skipped(Some(1)), Line(2)], 2).is_err());
    }
}
