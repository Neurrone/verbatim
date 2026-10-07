//! Object navigation and the review cursor (milestone M3 reducer item 4),
//! driven against Verbatim's own settings dialog — the same self-voicing
//! target the M1 exit regression uses, so it needs no external application.
//!
//! The walk opens the Speech settings dialog (focus lands in the category
//! list), then exercises the object-navigation gestures through the control
//! plane's `SendGesture`, which routes them exactly as a physical keypress
//! would: report the current object, move to the list's first child (the
//! "Speech" category item), report again to confirm the navigator moved,
//! then snap the navigator back to focus. Last, it tabs to the rate slider,
//! whose MSAA object answers next and previous with itself, and checks that
//! object navigation reports the edge ("No next", "No previous") rather than
//! landing on the slider again. Assertions are substring matches,
//! tolerant of the platform controls' own wording, matching the M1
//! regression's style.
//!
//! Gestures use the desktop layout's bindings (the default), addressed by
//! their stable identifiers rather than raw numpad keystrokes, so `NumLock`
//! state cannot affect the run.

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// Sends `gesture` and asserts exactly `heard`.
fn navigate(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_speech_settings(scenario);

    // Report the current navigator object: the navigator follows focus, so
    // this reports the focused list item, with its role and states, as a
    // report does. Then to its parent, the category list, and back to its
    // first child, the item, which object navigation speaks as a focus
    // speaks it, without the role. To the parent again, then back to the
    // focus.
    navigate(
        scenario,
        "kb:verbatim+numpad5",
        "Speech list item focused selected 1 of 3",
    );
    navigate(scenario, "kb:verbatim+numpad8", "Categories: list Alt+c");
    navigate(scenario, "kb:verbatim+numpad2", "Speech 1 of 3");
    navigate(scenario, "kb:verbatim+numpad8", "Categories: list Alt+c");
    navigate(
        scenario,
        "kb:verbatim+numpadminus",
        "Move to focus Speech 1 of 3",
    );

    // The rate slider's MSAA object answers next and previous with itself,
    // since its window is its whole world: object navigation must report
    // the edge rather than land on the slider again. Tab reaches it after
    // the Change button and the voice and variant boxes.
    for heard in [
        "Change... button Alt+h",
        "Voice combo box English (Great Britain) collapsed Alt+v",
        "Variant combo box Max collapsed Alt+a",
        "Rate slider 80 Alt+r",
    ] {
        scenario.send_keys(&["tab"]).expect("sends tab");
        scenario.speech().expect(&[heard]);
    }
    navigate(scenario, "kb:verbatim+numpad6", "No next");
    navigate(scenario, "kb:verbatim+numpad4", "No previous");
    super::close_settings_to_desktop(scenario);
}
