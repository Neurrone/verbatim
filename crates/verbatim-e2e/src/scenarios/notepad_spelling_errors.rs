//! Spelling errors in Windows 11 Notepad (milestone M4 item 7): the default
//! theme reports a misspelt word as the caret reaches it, as NVDA does
//! (`docs/nvda/document-formatting.md`): a line says "spelling error"
//! before each misspelt word in it; a character or a word says it where an
//! error starts and "out of spelling error" where it ends; and only
//! changes are said, so moving within a misspelt word says nothing more.
//!
//! Notepad's spell checker marks its errors with UIA's spelling error
//! annotation, which Verbatim reads with the caret in one remote
//! operation. The checker marks the document a moment after it opens, so
//! the first step reads the first line until the marks are there; every
//! other step waits for its speech to be heard in full before the next key,
//! as a listening user would, and there is no other wait.
//!
//! This is a demonstration (`registry::DEMONSTRATIONS`), run only through
//! `cargo xtask demo notepad_spelling_errors`, not part of the suite:
//! GitHub's Windows Server runners have classic Notepad, a Win32 edit
//! control with no spell checker. The suite's
//! [`spelling_errors`](super::spelling_errors) hears the same speech from
//! `mockapp`'s scripted text, which holds everywhere; this keeps the real
//! spell checker's marks, read from a real provider, demonstrable.

use std::io;
use std::time::{Duration, Instant};

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// How long Notepad's spell checker is given to mark the document.
const SPELL_CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// The harness document's name.
const NAME: &str = "spelling";

/// The document: two misspelt words on the first line, none on the second.
const DOCUMENT: &str = "Ths line has a tset.\r\nAll fine here.\r\n";

/// The first line as the default theme reads it once the errors are
/// marked.
const FIRST_LINE: &str = "sound: spelling-error spelling error Ths line has a sound: spelling-error spelling error tset .";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let pid = scenario.open_document_with("notepad.exe", NAME, DOCUMENT)?;
    Ok(ScenarioState::TargetPid(pid))
}

/// Presses `keys` and waits for exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &str) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect_exactly(&[heard], STEP_TIMEOUT);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    let _ = super::expect_notepad_text(scenario, STEP_TIMEOUT);

    // Control+Home reads the first line; until the spell checker has marked
    // it, the line comes without its errors, and the key is pressed again
    // once that reading has been heard.
    let deadline = Instant::now() + SPELL_CHECK_TIMEOUT;
    loop {
        scenario
            .send_keys(&["control+home"])
            .expect("sends the key");
        let line = scenario
            .speech()
            .expect_in_order_capturing(&["Ths line has a"], STEP_TIMEOUT);
        if line == FIRST_LINE {
            break;
        }
        assert_eq!(
            line, "Ths line has a tset.",
            "the first line was read as neither unmarked nor marked"
        );
        assert!(
            Instant::now() < deadline,
            "Notepad's spell checker did not mark the document within {SPELL_CHECK_TIMEOUT:?}"
        );
    }

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
