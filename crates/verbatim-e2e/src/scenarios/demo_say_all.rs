//! Demonstration: say-all (milestone M4 item 6), recorded by
//! `cargo xtask demo` for `videos/demos`. The `notepad_say_all` scenario
//! tests the same command; this one shows it at a viewer's pace, and shows
//! the "Say all reads by" setting's two cases.
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

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long the edit control's window is given to appear.
const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);

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

/// The text box's accessible name, spoken when it takes the focus.
const BOX_NAME: &str = "Story";

/// The edit control's text: two paragraphs of sentences, in reading order.
const SENTENCES: [&[&str]; 2] = [
    &[
        "A letter arrived on Tuesday.",
        "It had no stamp and no return address.",
        "Inside was a single brass key.",
    ],
    &[
        "Nobody in the house knew what it opened.",
        "Grandmother said it was older than the house itself.",
    ],
];

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let document: String = PARAGRAPHS
        .iter()
        .flat_map(|paragraph| [*paragraph, "\r\n"])
        .collect();
    let pid = scenario.open_document_with("notepad.exe", NAME, &document)?;
    Ok(ScenarioState::TargetPid(pid))
}

/// The script showing the edit control's window, titled `title`, its text
/// read from `story`.
fn edit_script(title: &str, story: &str) -> String {
    let title = title.replace('\'', "''");
    let story = story.replace('\'', "''");
    format!(
        "Add-Type -AssemblyName System.Windows.Forms\r\n\
         Add-Type -AssemblyName System.Drawing\r\n\
         [System.Windows.Forms.Application]::EnableVisualStyles()\r\n\
         $form = New-Object System.Windows.Forms.Form\r\n\
         $form.Text = '{title}'\r\n\
         $form.ClientSize = New-Object System.Drawing.Size(1100, 400)\r\n\
         $form.StartPosition = 'CenterScreen'\r\n\
         $box = New-Object System.Windows.Forms.TextBox\r\n\
         $box.Multiline = $true\r\n\
         $box.ScrollBars = 'Both'\r\n\
         $box.WordWrap = $false\r\n\
         $box.Dock = 'Fill'\r\n\
         $box.Font = New-Object System.Drawing.Font('Segoe UI', 16)\r\n\
         $box.AccessibleName = '{BOX_NAME}'\r\n\
         $box.Text = [IO.File]::ReadAllText('{story}')\r\n\
         $box.Select(0, 0)\r\n\
         $form.Controls.Add($box)\r\n\
         [void]$form.ShowDialog()\r\n"
    )
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

/// Part 2: say-all in a Win32 edit control, by sentence.
fn edit_control_by_sentence(scenario: &mut Scenario) {
    let title = harness_marker("demo-edit-control");
    let directory = scenario
        .harness_folder("demo-edit-control")
        .expect("the run has a directory for harness files");
    let story_path = format!(r"{directory}\story.txt");
    let script_path = format!(r"{directory}\edit.ps1");
    let story: Vec<String> = SENTENCES
        .iter()
        .map(|paragraph| paragraph.join(" "))
        .collect();
    scenario
        .write_agent_file(&story_path, story.join("\r\n").as_bytes())
        .expect("writes the story");
    scenario
        .write_agent_file(&script_path, edit_script(&title, &story_path).as_bytes())
        .expect("writes the script");
    let args: Vec<String> = [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-WindowStyle",
        "Hidden",
        "-File",
        &script_path,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let pid = scenario
        .launch_titled("powershell.exe", &args, &title, true)
        .expect("starts Windows PowerShell");
    scenario
        .bring_titled_window_forward(&title, WINDOW_TIMEOUT)
        .expect("the edit control's window takes the foreground");

    // The text box's focus, then the caret's line, at the start of the
    // text.
    let first = SENTENCES[0][0];
    scenario
        .speech()
        .expect_in_order(&[BOX_NAME, first], STEP_TIMEOUT);

    scenario
        .send_gesture("kb:verbatim+downarrow")
        .expect("sends say all");
    let sentences: Vec<&str> = SENTENCES
        .iter()
        .flat_map(|paragraph| paragraph.iter().copied())
        .collect();
    for (index, sentence) in sentences.iter().enumerate() {
        let heard = scenario
            .speech()
            .expect_in_order_capturing(&[*sentence], STEP_TIMEOUT);
        if let Some(next) = sentences.get(index + 1) {
            assert!(
                !heard.contains(next),
                "say-all read {heard:?} as one piece, not sentence by sentence"
            );
        }
    }
    scenario.speech().wait_until_quiet(STEP_TIMEOUT);
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
