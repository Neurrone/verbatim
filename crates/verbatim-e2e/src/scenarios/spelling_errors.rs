//! Spelling errors (milestone M4 item 7): the default theme reports a
//! misspelt word as the caret reaches it, as NVDA does
//! (`docs/nvda/document-formatting.md`): a line says "spelling error"
//! before each misspelt word in it; a character or a word says it where an
//! error starts and "out of spelling error" where it ends; and only
//! changes are said, so moving within a misspelt word says nothing more.
//!
//! The text is `mockapp`'s, a scripted UIA text provider whose spelling
//! errors are fixed stretches reported with UIA's spelling error
//! annotation, as Windows 11 Notepad reports its spell checker's marks, and
//! whose caret moves with the keys pressed here. The suite runs the same
//! everywhere, and no real control with spelling errors Verbatim reads is
//! on both Windows 11 and GitHub's Windows Server runners: the runner's
//! Notepad is classic Notepad, an edit control with no spell checker; a
//! `RichEdit` control with its spell checking on is read through MSAA and
//! its window messages, as NVDA reads it, which carry no spelling errors
//! (its character formatting has none to give); and a WPF text box checks
//! its spelling but leaves the errors out of its UIA text. Windows 11
//! Notepad's own spell checker is tested by the local-only
//! [`notepad_spelling_errors`](super::notepad_spelling_errors), which
//! GitHub's runner skips.
//!
//! The errors are there from the start, so every step presses a key and
//! asserts exactly what it says, heard in full, before the next.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

pub(crate) use super::no_teardown as teardown;

/// The window's name in the run ([`harness_marker`]).
const NAME: &str = "spelling";

/// The text: two misspelt words on the first line, none on the second.
const TEXT: &str = r"Ths line has a tset.\nAll fine here.\n";

/// The misspelt words, as UTF-16 offsets into [`TEXT`]: "Ths" and "tset".
const SPELLING_ERRORS: &str = "[[0, 3], [15, 19]]";

/// The text area, by its name and role.
const TEXT_AREA: &str = "Text edit";

/// The first line as the default theme reads it.
const FIRST_LINE: &str = "sound: spelling-error spelling error Ths line has a sound: spelling-error spelling error tset .";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let directory = scenario.run_directory().to_owned();
    let title = harness_marker(NAME);
    // Deleted when the scenario ends, and by the next launch's sweep after
    // an aborted run.
    let fixture = scenario.harness_file(NAME, "json");
    let contents = format!(
        r#"{{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [
    {{
      "id": "text",
      "role": "editable_text",
      "name": "Text",
      "states": ["focusable", "focused"],
      "text": "{TEXT}",
      "spelling_errors": {SPELLING_ERRORS}
    }}
  ]
}}
"#
    );
    scenario.write_agent_file(&fixture, contents.as_bytes())?;
    let args = [
        "--fixture",
        &fixture,
        "--backend",
        "uia",
        "--title",
        &title,
        "--show",
    ]
    .map(str::to_owned);
    scenario.launch_titled(&format!(r"{directory}\mockapp.exe"), &args, &title, true)?;
    Ok(ScenarioState::None)
}

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &str) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(&[heard]);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // The window, then its text area, then the caret's line, the first,
    // with its errors.
    let window = format!("{} window", harness_marker(NAME));
    scenario.speech().expect(&[&window, TEXT_AREA, FIRST_LINE]);

    // The line ended out of the error, after its full stop, so the next
    // character, inside the misspelt "Ths", enters it again; the one after
    // that says only itself. The next word leaves the error, which a word
    // says, and the next misspelt word and the full stop after it enter
    // and leave it again.
    press(
        scenario,
        "rightarrow",
        "sound: spelling-error spelling error h",
    );
    press(scenario, "rightarrow", "s");
    press(scenario, "control+rightarrow", "out of spelling error line");
    press(scenario, "control+rightarrow", "has");
    press(scenario, "control+rightarrow", "a");
    press(
        scenario,
        "control+rightarrow",
        "sound: spelling-error spelling error tset",
    );
    press(scenario, "control+rightarrow", "out of spelling error dot");

    // The second line has no error, and says nothing about formatting;
    // the first, read again, says its errors again.
    press(scenario, "downarrow", "All fine here.");
    press(scenario, "uparrow", FIRST_LINE);
}
