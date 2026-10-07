//! Demonstration: say-all (milestone M4 item 6), recorded by
//! `cargo xtask demo` for `videos/demos`. The `notepad_say_all` scenario
//! tests the same command, and `edit_control_say_all` the second part, with
//! the same window and text; this one shows them at a viewer's pace, and
//! shows the "Say all reads by" setting's two cases.
//!
//! The walk:
//!
//! 1. Notepad opens on four paragraphs of prose. The caret moves to the
//!    top, and Verbatim+Down Arrow, the desktop layout's say all, reads from
//!    there. Notepad's text is UIA, which has no sentence unit, so it reads
//!    line by line. The first two paragraphs are heard in full; once the
//!    third starts playing, Control interrupts it. Home then speaks the
//!    first character of the caret's line, "S", and numpad 8 reads that
//!    line, which starts the third paragraph: the caret is where speech
//!    stopped.
//! 2. A Windows Forms window opens, its whole client area one multi-line
//!    text box, which is a standard Win32 edit control. Windows PowerShell
//!    shows it, with a script the scenario writes, since classic Notepad's
//!    edit control is not on Windows 11 and every Windows 11 has Windows
//!    PowerShell and Windows Forms. Verbatim reads an edit control through
//!    its window messages and splits its text into sentences, so say-all
//!    there reads by sentence, the setting's default: each sentence is
//!    spoken, and heard in full, as an utterance of its own, never with
//!    the next one. Say-all reads to the end of the text, and the window is
//!    closed. The text box does not wrap, and is wide enough to show each
//!    paragraph on one line, since a sentence is also cut where a line
//!    wraps.
//!
//! Every step waits for evidence, with a deadline that only bounds a
//! failure: speech heard in full, or a window taking the foreground.

use std::io;
use std::time::Duration;

use super::edit_control_say_all;
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// The harness document's name.
const NAME: &str = "demo-say-all";

/// The prose Notepad reads, a paragraph a line.
const PARAGRAPHS: [&str; 4] = [
    "The lighthouse stood on a ledge of black rock, a mile from the nearest village. Every evening the keeper climbed its spiral stair to light the lamp, and every morning he climbed it again to put the lamp out.",
    "In winter the storms came in from the west. Waves broke over the gallery rail, and salt crusted the windows so thickly that the keeper scraped them clean with a knife before the light could shine through.",
    "Ships passing in the night never saw him. They saw only the beam, sweeping across the water once every ten seconds, and they knew from its rhythm exactly where they were.",
    "When the light was finally automated, the keeper rowed ashore for the last time and never went back.",
];

/// How each paragraph starts: few enough words to fit the first line
/// Notepad shows of it, however narrow its window.
const STARTS: [&str; 3] = [
    "The lighthouse stood",
    "In winter the storms",
    "Ships passing in the night",
];

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let document: String = PARAGRAPHS
        .iter()
        .flat_map(|paragraph| [*paragraph, "\r\n"])
        .collect();
    let pid = scenario.open_document_with("notepad.exe", NAME, &document)?;
    Ok(ScenarioState::TargetPid(pid))
}

/// Part 1: say-all in Notepad, by line, interrupted in the third paragraph.
fn notepad_by_line(scenario: &mut Scenario) {
    let _ = super::expect_notepad_text(scenario, STEP_TIMEOUT);
    scenario
        .send_keys(&["control+home"])
        .expect("sends control+home");
    scenario
        .speech()
        .expect_in_order(&[STARTS[0]], STEP_TIMEOUT);

    scenario
        .send_gesture("kb:verbatim+downarrow")
        .expect("sends say all");
    // The first line of each of the first two paragraphs, heard in full;
    // their other lines, if Notepad wraps them, are heard too, since
    // nothing interrupts until the third paragraph starts.
    scenario
        .speech()
        .expect_in_order(&[STARTS[0]], STEP_TIMEOUT);
    scenario
        .speech()
        .expect_in_order(&[STARTS[1]], STEP_TIMEOUT);
    scenario.speech().expect_playing(STARTS[2], STEP_TIMEOUT);
    scenario.send_keys(&["control"]).expect("sends control");

    // The caret is on the line speech stopped in.
    scenario.send_keys(&["home"]).expect("sends home");
    scenario.speech().expect_exactly(&["S"], STEP_TIMEOUT);
    scenario.send_gesture("kb:numpad8").expect("sends numpad 8");
    scenario
        .speech()
        .expect_in_order(&[STARTS[2]], STEP_TIMEOUT);
    scenario.speech().wait_until_quiet(STEP_TIMEOUT);
}

/// Part 2: say-all in a Win32 edit control, by sentence, as the
/// `edit_control_say_all` scenario tests it.
fn edit_control_by_sentence(scenario: &mut Scenario) {
    let pid = edit_control_say_all::open_story(scenario)
        .expect("the edit control's window takes the foreground");
    edit_control_say_all::read_by_sentence(scenario);
    scenario
        .kill_target(pid)
        .expect("closes the edit control's window");
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    notepad_by_line(scenario);
    edit_control_by_sentence(scenario);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    if let ScenarioState::TargetPid(pid) = state {
        scenario
            .kill_target(pid)
            .expect("kills notepad through the agent");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_start_begins_its_paragraph() {
        for (start, paragraph) in STARTS.iter().zip(PARAGRAPHS) {
            assert!(paragraph.starts_with(start), "{start:?}");
        }
        assert!(PARAGRAPHS[2].starts_with('S'));
    }
}
