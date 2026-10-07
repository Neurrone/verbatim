//! The M1 exit criteria (`docs/roadmap.md`): Verbatim+V opens the menu,
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

use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

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
    let walk: [(&[&str], &str); 15] = [
        (&["tab"], "Change... button Alt+h"),
        (
            &["tab"],
            "Voice combo box English (Great Britain) collapsed Alt+v",
        ),
        (&["downarrow"], "English (Scotland)"),
        (&["uparrow"], "English (Great Britain)"),
        (&["tab"], "Variant combo box Max collapsed Alt+a"),
        (&["tab"], "Rate slider 80 Alt+r"),
        (&["uparrow"], "79"),
        (&["downarrow"], "80"),
        (&["downarrow"], "81"),
        (&["tab"], "Pitch slider 50 Alt+p"),
        (&["tab"], "Inflection slider 80 Alt+i"),
        (&["tab"], "Volume slider 100 Alt+o"),
        (&["tab"], "OK button"),
        (&["tab"], "Cancel button"),
        (&["tab"], "Apply button Alt+a"),
    ];
    for (keys, heard) in walk {
        scenario.send_keys(keys).expect("sends the keys");
        scenario.speech().expect(&[heard]);
    }
    super::close_settings_to_desktop(scenario);
}
