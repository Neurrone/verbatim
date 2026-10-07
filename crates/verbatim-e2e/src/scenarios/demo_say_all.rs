//! Demonstration: say-all (milestone M4 item 6), recorded by `cargo xtask
//! demo` for `videos/demos`. It shows the "Say all reads by" setting's two
//! cases, each exactly as a test tests it, so it shows nothing no test
//! covers:
//!
//! 1. In Windows 11 Notepad, say-all reads by line, and a key interrupts
//!    it, leaving the caret where speech stopped: the `notepad_say_all`
//!    scenario's document and steps.
//! 2. In a Win32 edit control, a Windows Forms text box, say-all reads by
//!    sentence to the end of the text: the `edit_control_say_all`
//!    scenario's window and steps.

use std::io;

use super::{edit_control_say_all, notepad_say_all};
use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    notepad_say_all::setup(scenario)
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    notepad_say_all::body(scenario, state);
    edit_control_say_all::open_story(scenario)
        .expect("the edit control's window takes the foreground");
    edit_control_say_all::read_by_sentence(scenario);
}
