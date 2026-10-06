//! Demonstration: a session in Windows Terminal (milestone M4 item 9),
//! recorded by `cargo xtask demo` for `videos/demos`. The terminal
//! scenarios test the same behavior; this one shows it as a session, at a
//! viewer's pace. It needs Windows Terminal and fails, saying so, when
//! `wt.exe` cannot be started, rather than using the console host as the
//! test scenarios do. The window, the shell, and the prompt are set up as
//! for every terminal scenario (the `terminal` module).
//!
//! Every command is typed one character at a time, each character's echo
//! heard in full before the next, and then Enter is pressed. The walk:
//!
//! 1. The prompt is spoken as it appears and read with the review cursor
//!    (numpad 8): "ready>".
//! 2. `Get-ChildItem -Name files` lists a folder the scenario wrote: its
//!    three file names are spoken, then the prompt.
//! 3. `.\table.ps1` prints a table of planets and their moons; every row is
//!    spoken. The review cursor then reads up from the prompt to the
//!    header row (numpad 7), moves to the Moons column (Shift+numpad 1,
//!    then numpad 6), and goes down the column: numpad 9 reads each row and
//!    numpad 5 the number of moons in it, the column kept on every row.
//! 4. `echo helo` is typed with a typo; Backspace deletes the last "o",
//!    which is spoken, `lo` is typed, and Enter gives "hello" and the
//!    prompt.
//! 5. `.\password.ps1` asks for a password with `Read-Host
//!    -AsSecureString`; "secret" is typed, and with "speak passwords" off,
//!    the default, none of its characters is spoken. The script then says
//!    "done", and the prompt follows.
//! 6. `.\flood.ps1` prints a thousand lines at once. Verbatim speaks the
//!    first lines, "skipped" and how many lines it skipped, and the last
//!    lines, then the prompt; every line is accounted for, as the
//!    `terminal_flood` scenario checks.
//! 7. Verbatim+5 turns output reporting off ("report new output off").
//!    `.\quiet.ps1` prints two lines and writes a file once done: the
//!    typing is still echoed, but neither line nor the prompt is spoken.
//!    Verbatim+5 turns reporting back on ("report new output on"), and
//!    `echo back` gives "back" and the prompt, with nothing from while
//!    reporting was off.

use std::io;
use std::time::Duration;

use super::terminal::{self, PROMPT, STEP_TIMEOUT};
use super::terminal_flood::{Spoken, account, classify};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Heard;

/// The files `Get-ChildItem` lists, in the order it lists them.
const FILES: [&str; 3] = ["alpha.txt", "beta.txt", "gamma.txt"];

/// The table's rows, each with its word in the Moons column, column 9.
const TABLE: [(&str, &str); 6] = [
    ("Planet   Moons  Rings", "Moons"),
    ("Mercury  0      no", "0"),
    ("Earth    1      no", "1"),
    ("Mars     2      no", "2"),
    ("Jupiter  95     yes", "95"),
    ("Saturn   146    yes", "146"),
];

/// How many lines the flood prints.
const FLOOD_LINES: u32 = 1000;

/// The longest the flood and its speech may take; it only bounds a
/// failure.
const FLOOD_TIMEOUT: Duration = Duration::from_secs(300);

/// What `quiet.ps1` prints while output reporting is off.
const QUIET_LINES: [&str; 2] = ["This output is not spoken.", "Nor is this line."];

/// The file `quiet.ps1` writes once it has printed its lines.
const QUIET_DONE: &str = "quiet-done";

const OUTPUT_OFF: &str = "report new output off";
const OUTPUT_ON: &str = "report new output on";

/// The scripts the session runs, and the files it lists.
fn scripts() -> Vec<(String, String)> {
    let mut scripts: Vec<(String, String)> = FILES
        .iter()
        .map(|file| (format!(r"files\{file}"), String::new()))
        .collect();
    let table: String = TABLE
        .iter()
        .flat_map(|(row, _)| ["'", *row, "'\r\n"])
        .collect();
    let quiet: String = QUIET_LINES
        .iter()
        .map(|line| format!("'{line}'\r\n"))
        .chain(std::iter::once(format!(
            "[IO.File]::WriteAllText(\"$PSScriptRoot\\{QUIET_DONE}\", '')\r\n"
        )))
        .collect();
    scripts.extend([
        ("table.ps1".to_owned(), table),
        (
            "password.ps1".to_owned(),
            "$secure = Read-Host -AsSecureString 'Password'\r\n'done'\r\n".to_owned(),
        ),
        (
            "flood.ps1".to_owned(),
            format!(
                "for ($line = 1; $line -le {FLOOD_LINES}; $line++) {{ \"flood line $line\" }}\r\n"
            ),
        ),
        ("quiet.ps1".to_owned(), quiet),
    ]);
    scripts
}

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let scripts = scripts();
    let scripts: Vec<(&str, &str)> = scripts
        .iter()
        .map(|(file, contents)| (file.as_str(), contents.as_str()))
        .collect();
    terminal::open_windows_terminal_only(scenario, "demo-session", &scripts)
}

/// Types `text` a character at a time, each echo heard in full before the
/// next is typed. Verbatim holds a character typed into a terminal until
/// the terminal shows it, and a space at the end of the line shows as
/// nothing, so a space is typed together with the character after it, and
/// the two echoes are heard in order.
fn type_in_terminal(scenario: &mut Scenario, text: &str) {
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == ' '
            && let Some(next) = characters.next()
        {
            scenario
                .type_text(&format!(" {next}"))
                .expect("types the space and the character after it");
            let next = super::character_name(next);
            for heard in ["space", next.as_str()] {
                scenario.speech().expect_exactly(&[heard], STEP_TIMEOUT);
            }
        } else {
            let name = super::character_name(character);
            super::type_and_hear(scenario, character, &[&name], STEP_TIMEOUT);
        }
    }
}

/// Types `command` a character at a time, each echo heard in full, and
/// presses Enter.
fn enter_command(scenario: &mut Scenario, command: &str) {
    type_in_terminal(scenario, command);
    scenario.send_keys(&["enter"]).expect("presses enter");
}

/// Sends the review gesture `gesture` and waits for `text`, trailing white
/// space aside.
fn review_text(scenario: &mut Scenario, gesture: &str, text: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    let heard = scenario
        .speech()
        .expect_in_order_capturing(&[text], STEP_TIMEOUT);
    assert_eq!(
        heard.trim_end(),
        text,
        "{gesture} read {heard:?}, not {text:?}"
    );
}

/// Step 3: the table printed, then read down its Moons column.
fn table(scenario: &mut Scenario) {
    enter_command(scenario, r".\table.ps1");
    let mut printed: Vec<&str> = TABLE.iter().map(|(row, _)| *row).collect();
    printed.push(PROMPT);
    scenario.speech().expect_exactly(&printed, STEP_TIMEOUT);

    for (row, _) in TABLE.iter().rev() {
        review_text(scenario, "kb:numpad7", row);
    }
    scenario
        .send_gesture("kb:shift+numpad1")
        .expect("sends the gesture");
    scenario.speech().expect_exactly(&["P"], STEP_TIMEOUT);
    review_text(scenario, "kb:numpad6", TABLE[0].1);
    for (row, moons) in &TABLE[1..] {
        review_text(scenario, "kb:numpad9", row);
        review_text(scenario, "kb:numpad5", moons);
    }
}

/// Step 4: a typo corrected with Backspace.
fn corrected_command(scenario: &mut Scenario) {
    type_in_terminal(scenario, "echo helo");
    scenario.send_keys(&["backspace"]).expect("sends backspace");
    scenario.speech().expect_exactly(&["o"], STEP_TIMEOUT);
    enter_command(scenario, "lo");
    scenario
        .speech()
        .expect_exactly(&["hello", PROMPT], STEP_TIMEOUT);
}

/// Step 5: a password typed at a prompt, never spoken.
fn password(scenario: &mut Scenario) {
    enter_command(scenario, r".\password.ps1");
    scenario
        .speech()
        .expect_in_order(&["Password:"], STEP_TIMEOUT);
    terminal::run_command(scenario, "secret");
    let heard = terminal::listen_until(scenario, "done", STEP_TIMEOUT, false);
    let typed = ["s", "e", "c", "r", "t"];
    let spoken: Vec<&str> = heard
        .iter()
        .map(|heard| heard.text.as_str())
        .filter(|text| typed.contains(text) || text.contains("secret"))
        .collect();
    assert!(
        spoken.is_empty(),
        "the password's characters were spoken: {spoken:?}"
    );
    scenario.speech().expect_exactly(&[PROMPT], STEP_TIMEOUT);
}

/// The texts of `heard` that are about the flood.
fn flood_related(heard: &[Heard]) -> Vec<&str> {
    heard
        .iter()
        .map(|heard| heard.text.as_str())
        .filter(|text| classify(text).is_some())
        .collect()
}

/// Step 6: a flood of output, summarized.
fn flood(scenario: &mut Scenario) {
    enter_command(scenario, r".\flood.ps1");
    let heard = terminal::listen_until(scenario, PROMPT, FLOOD_TIMEOUT, true);
    let after = scenario.speech().take_until_quiet(STEP_TIMEOUT);
    let speech = scenario.speech();
    let spoken: Vec<Spoken> = heard
        .iter()
        .filter(|heard| speech.ending_of(heard) == Some(verbatim_model::UtteranceEnding::Completed))
        .filter_map(|heard| classify(&heard.text))
        .collect();
    if let Err(failure) = account(&spoken, FLOOD_LINES) {
        panic!("the flood's speech does not account for every line: {failure}; heard: {spoken:?}");
    }
    let late = flood_related(&after);
    assert!(
        late.is_empty(),
        "flood speech was queued after the prompt: {late:?}"
    );
}

/// Step 7: output reporting off for a command, then on again.
fn quiet_command(scenario: &mut Scenario, directory: &str) {
    scenario
        .send_gesture("kb:verbatim+5")
        .expect("sends Verbatim+5");
    scenario
        .speech()
        .expect_exactly(&[OUTPUT_OFF], STEP_TIMEOUT);
    enter_command(scenario, r".\quiet.ps1");
    scenario
        .wait_for_agent_file(&format!(r"{directory}\{QUIET_DONE}"), STEP_TIMEOUT)
        .expect("quiet.ps1 prints its lines");
    let heard = scenario.speech().take_until_quiet(STEP_TIMEOUT);
    let spoken: Vec<&str> = heard
        .iter()
        .map(|heard| heard.text.as_str())
        .filter(|text| QUIET_LINES.contains(text) || *text == PROMPT)
        .collect();
    assert!(
        spoken.is_empty(),
        "output was spoken with output reporting off: {spoken:?}"
    );

    scenario
        .send_gesture("kb:verbatim+5")
        .expect("sends Verbatim+5");
    scenario.speech().expect_exactly(&[OUTPUT_ON], STEP_TIMEOUT);
    enter_command(scenario, "echo back");
    let heard = terminal::listen_until(scenario, "back", STEP_TIMEOUT, false);
    let stale: Vec<&str> = heard
        .iter()
        .map(|heard| heard.text.as_str())
        .filter(|text| QUIET_LINES.contains(text))
        .collect();
    assert!(
        stale.is_empty(),
        "output from while reporting was off was spoken once it was on: {stale:?}"
    );
    scenario.speech().expect_exactly(&[PROMPT], STEP_TIMEOUT);
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Window { directory, .. } = state else {
        panic!("the session's setup opens a terminal window");
    };
    let directory = directory.clone();
    terminal::expect_prompt_read(scenario, state);

    enter_command(scenario, "Get-ChildItem -Name files");
    let mut listed: Vec<&str> = FILES.to_vec();
    listed.push(PROMPT);
    scenario.speech().expect_exactly(&listed, STEP_TIMEOUT);

    table(scenario);
    corrected_command(scenario);
    password(scenario);
    flood(scenario);
    quiet_command(scenario, &directory);
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
    fn each_rows_moons_are_the_word_at_its_column_9() {
        for (row, moons) in TABLE {
            let word = row[9..].split_whitespace().next();
            assert_eq!(word, Some(moons), "row {row:?}");
        }
    }
}
