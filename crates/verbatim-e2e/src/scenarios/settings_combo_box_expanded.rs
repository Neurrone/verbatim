//! A Settings combo box opened with Alt+Down Arrow: its expansion is
//! announced before its first item, and Escape returns to it, as NVDA
//! reads it (`docs/parity/changes-and-selection.md`, "A combo box opened
//! into its list"; the capture is
//! `target/e2e-evidence/fixes-round-eight-2026-10-21/nvda-expanded-combo-box-capture.txt`).
//!
//! Read-only: Escape closes the list without choosing an item, so it
//! changes no setting. `ms-settings:dateandtime` opens the Date and time
//! page with the keyboard focus on its Region button, as its opening
//! speech asserts. The controls between it and the combo box differ
//! between machines (the time zone, whether location is on), so the agent
//! focuses the "Show additional calendars in the taskbar" combo box
//! directly, by its UI Automation identifier, as a script would, injecting
//! no input; it is announced inside its Additional settings group, as the
//! capture announces it when Shift+Tab reaches it from outside the group.
//!
//! Local-only: GitHub's runner's Settings opens the page with its focus on
//! "Location Settings link" instead of the Region button (run 39686251059
//! heard "Settings", "Settings window", "Location Settings link"), a
//! difference in the runner's Settings, not in Verbatim. The opening is
//! not read from the agent, as `settings_search_suggestions` reads its
//! suggestions' names, because the agent's reads of the focus give its
//! name and no role, and composing the spoken role word here would copy
//! Verbatim's role vocabulary into the scenario.
//!
//! Settings raises the combo box's expand state change before its item's
//! focus, but the change reaches the outpost through its own subscription
//! and the focus through the focus listener, so which the outpost takes in
//! first varies from press to press; the outpost restores the provider's
//! order (`docs/design/focus-pipeline.md`, section 12.4). Since the race is
//! intermittent, a pass alone proves little: the judge of that order is
//! `mockapp`'s
//! `a_change_of_the_old_focus_queued_behind_a_focus_inside_it_is_handled_first`,
//! which fixes the order the outpost takes the two in.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The UI Automation identifier of the "Show additional calendars in the
/// taskbar" combo box.
const COMBO_BOX_AUTOMATION_ID: &str = "SystemSettings_DateTime_LunarCalendarLanguage_ComboBox";

/// The combo box's announcement, closed.
const COMBO_BOX: &str =
    "Show additional calendars in the taskbar combo box Don't show additional calendars collapsed";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_settings_page("ms-settings:dateandtime")?;
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    scenario
        .speech()
        .expect(&["Settings", "Settings window", "Region button"]);
    scenario
        .focus_by_automation_id(COMBO_BOX_AUTOMATION_ID)
        .expect("the agent focuses the combo box");
    scenario
        .speech()
        .expect(&["Additional settings grouping", COMBO_BOX]);

    // Alt+Down Arrow opens the list: its expansion, then its first item
    // with its position.
    scenario
        .send_keys(&["alt+downarrow"])
        .expect("sends alt+downarrow");
    scenario
        .speech()
        .expect(&["expanded", "Don't show additional calendars 1 of 3"]);

    // Escape closes the list and returns to the combo box.
    scenario.send_keys(&["escape"]).expect("sends escape");
    scenario.speech().expect(&[COMBO_BOX]);
}
