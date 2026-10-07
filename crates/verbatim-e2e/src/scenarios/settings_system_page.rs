//! Reading the Settings app (roadmap M3's Settings-app scenario): its
//! System page's list of settings, each item with its position.
//!
//! Read-only, so it changes no setting. `ms-settings:system` opens the
//! System page. The controls before the list (an account button, the
//! navigation, links) differ between machines and editions, so the
//! scenario does not Tab to the list: the agent focuses it directly, by
//! its UI Automation identifier, which every edition shares, as a script
//! would, injecting no input. The list's first item is Display; how many
//! items it has differs between editions, so the agent reads the count
//! through UI Automation, independently of Verbatim, before the scenario
//! fixes what it expects.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The UI Automation identifier of the System page's list of settings.
const LIST_AUTOMATION_ID: &str = "settingPagesList";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_settings_page("ms-settings:system")?;
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    scenario.speech().expect(&[
        "Settings",
        "Settings window",
        "Search box, Find a setting edit",
        "blank",
    ]);
    let size = scenario
        .focus_by_automation_id(LIST_AUTOMATION_ID)
        .expect("the agent focuses the System page's list");
    let first = format!("Display 1 of {size}");
    scenario.speech().expect(&["list", &first]);

    // Down moves to the second item, Sound, announced with its position;
    // Up returns to Display.
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario.speech().expect(&[&format!("Sound 2 of {size}")]);
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario.speech().expect(&[&first]);
}
