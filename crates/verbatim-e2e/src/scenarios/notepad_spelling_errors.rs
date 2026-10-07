//! Spelling errors in Windows 11 Notepad (milestone M4 item 7): the default
//! theme reports a misspelt word as the caret reaches it, as NVDA does
//! (`docs/nvda/document-formatting.md`): a line says "spelling error"
//! before each misspelt word in it; a character or a word says it where an
//! error starts and "out of spelling error" where it ends; and only
//! changes are said, so moving within a misspelt word says nothing more.
//!
//! Notepad's spell checker marks its errors with UIA's spelling error
//! annotation, which Verbatim reads with the caret in one remote
//! operation. The checker marks the document a moment after it opens,
//! raising no event when it does, and well before Notepad's opening has
//! been announced; so once it has, the agent reads which words are marked
//! through UI Automation, independently of Verbatim, and the scenario
//! fails unless they are exactly the two misspelt words. Every step waits
//! for its speech to be heard in full before the next key, as a listening
//! user would, and there is no other wait.
//!
//! This scenario is local-only ([`ScenarioDef::local_only`]): every local
//! and VM run includes it, and GitHub's `e2e` job skips it, since its
//! Windows Server runner has classic Notepad, a Win32 edit control with no
//! spell checker. [`spelling_errors`](super::spelling_errors) hears the
//! same speech from `mockapp`'s scripted text, which holds everywhere; this
//! checks the real spell checker's marks, read from a real provider.
//!
//! [`ScenarioDef::local_only`]: crate::registry::ScenarioDef::local_only

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The harness document's name.
const NAME: &str = "spelling";

/// The document: two misspelt words on the first line, none on the second.
const DOCUMENT: &str = "Ths line has a tset.\r\nAll fine here.\r\n";

/// The first line as the default theme reads it once the errors are
/// marked.
const FIRST_LINE: &str = "sound: spelling-error spelling error Ths line has a sound: spelling-error spelling error tset .";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_document_with(NAME, DOCUMENT)?;
    Ok(ScenarioState::None)
}

/// Presses `keys` and asserts exactly `heard`.
fn press(scenario: &mut Scenario, keys: &str, heard: &str) {
    scenario.send_keys(&[keys]).expect("sends the key");
    scenario.speech().expect(&[heard]);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_opened(scenario, NAME, "Ths line has a tset.");
    assert_eq!(
        scenario
            .misspelt_words()
            .expect("the agent reads the marked words"),
        ["Ths", "tset"],
        "the words Notepad's spell checker marked once its opening was announced"
    );
    press(scenario, "control+home", FIRST_LINE);

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
