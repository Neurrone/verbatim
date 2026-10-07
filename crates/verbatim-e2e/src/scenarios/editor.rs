//! The two editors each editing scenario runs in (`docs/testing.md`, "One
//! target, one code path"): the harness's Windows Forms text box
//! ([`super::text_box`]), a Win32 edit control read through MSAA and its
//! window messages, which runs everywhere; and Windows 11 Notepad, a UIA
//! document, which is local-only. A scenario's steps are the same in both,
//! and so is most of what they say; where the two differ, the scenario
//! says so with [`Editor`].

use std::io;
use std::time::Instant;

use verbatim_control::protocol::Frame;
use verbatim_model::NormalizedEvent;

use crate::registry::ScenarioState;
use crate::scenario::{Document, Scenario, WINDOW_TIMEOUT};

/// The text box's accessible name.
const BOX_NAME: &str = "Text";

/// Which editor a scenario runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Editor {
    /// The harness's Windows Forms text box.
    TextBox,
    /// Windows 11 Notepad.
    Notepad,
}

impl Editor {
    /// Opens `document`, its caret at the start, in the text box, or
    /// brings it forward in Notepad, which opened it before Verbatim
    /// started ([`Document`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the editor cannot be opened or does not take the
    /// foreground.
    pub(crate) fn open(
        self,
        scenario: &mut Scenario,
        document: &Document,
    ) -> io::Result<ScenarioState> {
        match self {
            Self::TextBox => {
                super::text_box::open(scenario, document.name, BOX_NAME, &document.contents)?;
            }
            Self::Notepad => scenario.bring_document_forward(document.name)?,
        }
        Ok(ScenarioState::None)
    }

    /// Asserts what the editor's window says as it comes to the foreground
    /// with its caret on `line`.
    pub(crate) fn expect_in_front(self, scenario: &mut Scenario, name: &str, line: &str) {
        match self {
            Self::TextBox => super::text_box::expect_announced(scenario, name, BOX_NAME, line),
            Self::Notepad => super::expect_notepad_in_front(scenario, name, line),
        }
    }

    /// Pastes the clipboard at the caret with Control+V, which says
    /// nothing, and waits for the evidence that the text changed: Notepad's
    /// title marking unsaved changes, or the text box's change of value as
    /// Core receives it.
    pub(crate) fn paste(self, scenario: &mut Scenario, name: &str) {
        match self {
            Self::Notepad => {
                scenario.send_keys(&["control+v"]).expect("sends control+v");
                scenario
                    .expect_unsaved(name, WINDOW_TIMEOUT)
                    .expect("the paste reaches the document");
            }
            Self::TextBox => {
                let mut events = scenario
                    .subscribe_events()
                    .expect("subscribes to Verbatim's events");
                scenario.send_keys(&["control+v"]).expect("sends control+v");
                let deadline = Instant::now() + WINDOW_TIMEOUT;
                loop {
                    match events.next_frame() {
                        Ok(Frame::Event {
                            event: NormalizedEvent::TextChanged { .. },
                            ..
                        }) => return,
                        Ok(_) => {}
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                            ) => {}
                        Err(error) => panic!("the event subscription failed: {error}"),
                    }
                    assert!(
                        Instant::now() < deadline,
                        "the text box's text did not change within {WINDOW_TIMEOUT:?} of the paste"
                    );
                }
            }
        }
    }

    /// How the character a line ends on, the last of its line break, is
    /// named (`docs/nvda/editable-text-and-terminals.md`, "A line break as
    /// a character"): Windows 11 Notepad breaks lines with a carriage
    /// return alone; an edit control with a carriage return and a line
    /// feed, and the line ends on the line feed. The caret at the end of a
    /// line's text is on the carriage return in both.
    pub(crate) fn line_break(self) -> &'static str {
        match self {
            Self::TextBox => "line feed",
            Self::Notepad => "carriage return",
        }
    }

    /// The character before the last of a line's line break, whose text
    /// ends with `last_letter`: the edit control's carriage return, or in
    /// Notepad the text's own last character.
    pub(crate) fn before_line_break(self, last_letter: &'static str) -> &'static str {
        match self {
            Self::TextBox => "carriage return",
            Self::Notepad => last_letter,
        }
    }

    /// What Shift+End from inside a line's last word says: Windows 11
    /// Notepad's selection takes the line break in, spoken as white space
    /// before "selected"; an edit control's stops before it.
    pub(crate) fn selected_to_line_end(self, text: &str) -> String {
        match self {
            Self::TextBox => format!("{text} selected"),
            Self::Notepad => format!("{text}  selected"),
        }
    }

    /// Saves the document, for Notepad, which would otherwise keep its
    /// unsaved changes for its next session; the text box keeps nothing.
    pub(crate) fn save(self, scenario: &mut Scenario, name: &str) {
        if self == Self::Notepad {
            scenario
                .save_document(name, WINDOW_TIMEOUT)
                .expect("saves the document");
        }
    }
}
