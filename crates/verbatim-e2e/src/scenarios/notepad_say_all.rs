//! Say-all in Windows 11 Notepad (milestone M4 item 6): Verbatim+Down
//! Arrow, the desktop layout's say all from the caret (NVDA+A on laptops),
//! reads the text piece by piece, moving the caret as each piece starts to
//! play; a key interrupts it and leaves the caret where speech stopped.
//! Local-only: GitHub's Windows Server runner has classic Notepad.
//!
//! Notepad's text is UIA, which has no sentence unit, so say-all reads by
//! line, as Notepad lays the lines out (the long second line wraps, and is
//! read as its two lines), and speaks by sentence: the second line's one
//! sentence is one utterance, with a mark where its wrapped second part
//! starts. Say-all hands speech two utterances ahead of the one playing
//! (`docs/performance.md`, "Say-all"), so once the second line has started,
//! the third is queued. Control then cuts both off, as any key does, and
//! say-all stops: the third line is never heard. The caret was left at
//! the start of the second line, so Right Arrow speaks that line's second
//! character, and numpad 8 reads the line there, since the review cursor
//! follows the caret.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario};
use crate::speech::Ending;

pub(crate) use super::no_teardown as teardown;

/// The harness document's name.
const NAME: &str = "say-all";

/// The first line, read first.
const FIRST: &str = "Reading starts on this line.";

/// The second line, long enough to be playing when the key is pressed.
const SECOND: &str = "Mostly this second line is long enough to be playing when a key interrupts it, since it goes on for quite a while with nothing much to say.";

/// The second line's first part, as Notepad wraps it, where the caret
/// stays when speech stops before the second part starts.
const SECOND_FIRST_PART: &str = "Mostly this second line is long enough to be playing when a key interrupts it, since it goes on for quite a while ";

/// The third line, queued and never heard.
const THIRD: &str = "Nobody hears this third line.";

/// The scenario's document, which Notepad opens before Verbatim starts.
pub(crate) fn document() -> Document {
    Document {
        name: NAME,
        contents: format!("{FIRST}\r\n{SECOND}\r\n{THIRD}\r\n"),
    }
}

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.bring_document_forward(NAME)?;
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::expect_notepad_in_front(scenario, NAME, FIRST);

    scenario
        .send_gesture("kb:verbatim+downarrow")
        .expect("sends say all");
    scenario.speech().expect(&[FIRST]);
    let playing = scenario.speech().expect_started(SECOND);
    let queued = scenario.speech().expect_queued(&[THIRD]);
    scenario.send_keys(&["control"]).expect("sends control");
    scenario.speech().expect_ended(&playing, Ending::Cancelled);
    for heard in &queued {
        scenario.speech().expect_ended(heard, Ending::Cancelled);
    }

    // The caret is at the start of the line speech stopped in, and the
    // review cursor follows it there.
    scenario
        .send_keys(&["rightarrow"])
        .expect("sends rightarrow");
    scenario.speech().expect(&["o"]);
    scenario.send_gesture("kb:numpad8").expect("sends numpad 8");
    scenario.speech().expect(&[SECOND_FIRST_PART]);
}
