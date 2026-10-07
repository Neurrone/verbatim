//! Say-all in a standard Win32 edit control (milestone M4 item 6): the
//! test of what the `demo_say_all` demonstration's second part shows, with
//! the same window, text, and steps, which this module holds for both.
//!
//! A Windows Forms window opens, its whole client area one multi-line text
//! box, which is a standard Win32 edit control. Windows PowerShell shows
//! it, with a script the scenario writes, since classic Notepad's edit
//! control is not on Windows 11, and every Windows 11 and Windows Server
//! has Windows PowerShell and Windows Forms. The text box's focus is
//! announced with its name, "Story", and then the caret's line, the first
//! sentence's. Verbatim reads an edit control through its window messages
//! and splits its text into sentences, so say-all (Verbatim+Down Arrow)
//! there reads by sentence, the "Say all reads by" setting's default: each
//! sentence is spoken as an utterance of its own, never with the next one,
//! and say-all reads to the end of the text. The text box does not wrap,
//! and is wide enough to show each paragraph on one line, since a sentence
//! is also cut where a line wraps.
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

/// The window's name in the run ([`harness_marker`]), and its folder's.
const NAME: &str = "edit-control";

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

/// Writes the story and the script, opens the edit control's window, and
/// brings it to the foreground; returns the launch's pid, which closes the
/// window when killed.
///
/// # Errors
///
/// Returns an error if a file cannot be written, Windows PowerShell cannot
/// be started, or its window does not take the foreground.
pub(crate) fn open_story(scenario: &mut Scenario) -> io::Result<u32> {
    let title = harness_marker(NAME);
    let directory = scenario.harness_folder(NAME)?;
    let story_path = format!(r"{directory}\story.txt");
    let script_path = format!(r"{directory}\edit.ps1");
    let story: Vec<String> = SENTENCES
        .iter()
        .map(|paragraph| paragraph.join(" "))
        .collect();
    scenario.write_agent_file(&story_path, story.join("\r\n").as_bytes())?;
    scenario.write_agent_file(&script_path, edit_script(&title, &story_path).as_bytes())?;
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
    let pid = scenario.launch_titled("powershell.exe", &args, &title, true)?;
    scenario.bring_titled_window_forward(&title, WINDOW_TIMEOUT)?;
    Ok(pid)
}

/// Hears the text box's focus and the caret's line, then say-all from the
/// top, one sentence an utterance, to the end of the text.
pub(crate) fn read_by_sentence(scenario: &mut Scenario) {
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
}

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::TargetPid(open_story(scenario)?))
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    read_by_sentence(scenario);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    if let ScenarioState::TargetPid(pid) = state {
        scenario
            .kill_target(pid)
            .expect("closes the edit control's window");
    }
}
