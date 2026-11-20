//! The M1 exit criteria (`docs/roadmap-done.md`): Verbatim+V opens the menu,
//! every menu item and every control in the Speech settings dialog
//! announces its name, role, value, state, and shortcut on focus, and
//! adjusting the rate slider and the voice combo box speaks each new value.
//!
//! Every run speaks through eSpeak NG with the fixed e2e settings
//! (`Settings::for_e2e`), so the Speech page always has the same controls
//! with the same values: voice English (Great Britain), variant Max, rate
//! 80, pitch 50, inflection 80, volume 100. The Tab order is wx's: the
//! category list, the Change button, the six driver controls, then OK,
//! Cancel, and Apply; the read-only synthesizer field takes no tab stop.
//! On this wx slider the up arrow decreases the value. Escape closes the
//! dialog, and the focus returns to the desktop.
//!
//! Object navigation and the review cursor (milestone M3 reducer item 4)
//! run on the same dialog: once it opens, the walk reports the current
//! object, moves to the category list and back down to its first item,
//! up again, and back to the focus; at the rate slider, whose MSAA object
//! answers next and previous with itself, object navigation reports the
//! edge ("No next", "No previous") rather than landing on the slider
//! again. The gestures use the desktop layout's bindings, addressed by
//! their stable identifiers, so `NumLock` state cannot affect the run.

use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// Sends `gesture` and asserts exactly `heard`.
fn navigate(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut crate::registry::ScenarioState) {
    super::open_verbatim_menu(scenario);
    let steps: [(&[&str], &[&str]); 3] = [
        (&["downarrow"], &["Settings... s"]),
        (&["downarrow"], &["Exit x"]),
        (&["uparrow"], &["Settings... s"]),
    ];
    for (keys, heard) in steps {
        scenario.send_keys(keys).expect("sends the keys");
        scenario.speech().expect(heard);
    }
    scenario.send_keys(&["enter"]).expect("sends enter");
    scenario.speech().expect(&[
        "Verbatim Settings: Speech dialog",
        "Categories: list Alt+c",
        "Speech 1 of 3",
    ]);
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
    // The Change button sits in the "Synthesizer" group box, entered from
    // the category list, which is spoken before it.
    let to_rate: [(&[&str], &[&str]); 6] = [
        (
            &["tab"],
            &["Synthesizer grouping", "Change... button Alt+h"],
        ),
        (
            &["tab"],
            &["Voice combo box English (Great Britain) collapsed Alt+v"],
        ),
        (&["downarrow"], &["English (Scotland)"]),
        (&["uparrow"], &["English (Great Britain)"]),
        (&["tab"], &["Variant combo box Max collapsed Alt+a"]),
        (&["tab"], &["Rate slider 80 Alt+r"]),
    ];
    for (keys, heard) in to_rate {
        scenario.send_keys(keys).expect("sends the keys");
        scenario.speech().expect(heard);
    }
    // The rate slider's MSAA object answers next and previous with itself,
    // since its window is its whole world: object navigation must report
    // the edge rather than land on the slider again.
    navigate(scenario, "kb:verbatim+numpad6", "No next");
    navigate(scenario, "kb:verbatim+numpad4", "No previous");
    let from_rate: [(&[&str], &[&str]); 9] = [
        (&["uparrow"], &["79"]),
        (&["downarrow"], &["80"]),
        (&["downarrow"], &["81"]),
        (&["tab"], &["Pitch slider 50 Alt+p"]),
        (&["tab"], &["Inflection slider 80 Alt+i"]),
        (&["tab"], &["Volume slider 100 Alt+o"]),
        (&["tab"], &["OK button"]),
        (&["tab"], &["Cancel button"]),
        (&["tab"], &["Apply button Alt+a"]),
    ];
    for (keys, heard) in from_rate {
        scenario.send_keys(keys).expect("sends the keys");
        scenario.speech().expect(heard);
    }
    super::close_settings_to_desktop(scenario);
}
