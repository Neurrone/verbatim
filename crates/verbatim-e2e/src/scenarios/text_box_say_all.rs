//! Say-all in a standard Win32 edit control (milestone M4 item 6): the
//! test of what the `demo_say_all` demonstration's second part shows, with
//! the same window, text, and steps, which this module holds for both.
//!
//! The harness's Windows Forms text box opens ([`super::text_box`]), since
//! classic Notepad's edit control is not on Windows 11. The text box's focus is
//! announced with its name, "Story", and then the caret's line, the first
//! sentence's. Verbatim reads an edit control through its window messages
//! and splits its text into sentences, so say-all (Verbatim+Down Arrow)
//! there reads by sentence, the "Say all reads by" setting's default: each
//! sentence is spoken as an utterance of its own, never with the next one,
//! and say-all reads to the end of the text. The text box shows each
//! paragraph on one line, since a sentence is also cut where a line wraps.
//!
//! The window is announced by its title, then the text box by its name,
//! role, and state, then the caret's line, the whole first paragraph. Each
//! sentence say-all speaks keeps the space after it, as the edit control's
//! sentence unit gives it.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

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

/// Opens the text box holding the story, and waits for its window to take
/// the foreground.
///
/// # Errors
///
/// Returns an error if a file cannot be written, Windows PowerShell cannot
/// be started, or its window does not take the foreground.
pub(crate) fn open_story(scenario: &mut Scenario) -> io::Result<()> {
    let story: Vec<String> = SENTENCES
        .iter()
        .map(|paragraph| paragraph.join(" "))
        .collect();
    super::text_box::open(scenario, NAME, BOX_NAME, &story.join("\r\n"))?;
    Ok(())
}

/// Asserts the window's and the text box's announcements and the caret's
/// line, then say-all from the top, one sentence an utterance, each with
/// the space after it but the last of each paragraph, to the end of the
/// text.
pub(crate) fn read_by_sentence(scenario: &mut Scenario) {
    super::text_box::expect_announced(scenario, NAME, BOX_NAME, &SENTENCES[0].join(" "));

    scenario
        .send_gesture("kb:verbatim+downarrow")
        .expect("sends say all");
    let mut sentences: Vec<String> = Vec::new();
    for paragraph in SENTENCES {
        for (index, sentence) in paragraph.iter().enumerate() {
            if index + 1 == paragraph.len() {
                sentences.push((*sentence).to_owned());
            } else {
                sentences.push(format!("{sentence} "));
            }
        }
    }
    let sentences: Vec<&str> = sentences.iter().map(String::as_str).collect();
    scenario.speech().expect(&sentences);
}

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    open_story(scenario)?;
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    read_by_sentence(scenario);
}
