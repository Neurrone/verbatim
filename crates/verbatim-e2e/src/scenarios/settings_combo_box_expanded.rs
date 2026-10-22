//! A Settings combo box opened with Alt+Down Arrow: its expansion is
//! announced before its first item, and Escape returns to it, as NVDA
//! reads it (`docs/parity/changes-and-selection.md`, "A combo box opened
//! into its list"; the capture is
//! `target/e2e-evidence/fixes-round-eight-2026-10-21/nvda-expanded-combo-box-capture.txt`).
//!
//! Read-only: Escape closes the list without choosing an item, so it
//! changes no setting. `ms-settings:dateandtime` opens the Date and time
//! page with the keyboard focus on a control that differs between
//! machines: the Region button on the development machine, the Location Settings link on
//! GitHub's runner (run 39686251059). Once the page's opening is heard, the
//! agent reads the focused control's name and UI Automation control type,
//! and the scenario expects the name followed by the role word for that
//! type ([`role_word`]), so one exact expectation covers both machines'
//! first control. The controls between it and the combo box differ
//! between machines (the time zone, whether location is on), so the agent
//! focuses the "Show additional calendars in the taskbar" combo box
//! directly, by its UI Automation identifier, as a script would, injecting
//! no input; it is announced inside its Additional settings group, as the
//! capture announces it when Shift+Tab reaches it from outside the group.
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

/// UI Automation's button control type (`UIA_ButtonControlTypeId`).
const BUTTON_CONTROL_TYPE: i32 = 50000;

/// UI Automation's hyperlink control type (`UIA_HyperlinkControlTypeId`).
const HYPERLINK_CONTROL_TYPE: i32 = 50005;

/// The role word spoken after the name of a control of UI Automation
/// control type `control_type`, for the two types the page has been seen
/// to focus first. The words are NVDA's role names for a button and a
/// link (the display strings of `Role.BUTTON` and `Role.LINK` in
/// `nvda/source/controlTypes/role.py`), which Verbatim speaks on purpose
/// (`docs/parity/focus-announcement.md`, "Focus announcement content and
/// property order").
///
/// # Panics
///
/// Panics, naming the control type, for any other type, so an unknown
/// first control fails the scenario rather than being accepted.
fn role_word(control_type: i32) -> &'static str {
    match control_type {
        BUTTON_CONTROL_TYPE => "button",
        HYPERLINK_CONTROL_TYPE => "link",
        other => panic!(
            "the page's first focused control has UI Automation control type {other}, neither a button ({BUTTON_CONTROL_TYPE}) nor a hyperlink ({HYPERLINK_CONTROL_TYPE})"
        ),
    }
}

/// The combo box's announcement, closed.
const COMBO_BOX: &str =
    "Show additional calendars in the taskbar combo box Don't show additional calendars collapsed";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_settings_page("ms-settings:dateandtime")?;
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // The focus announcement's context comes before the control, both
    // spoken for the one focus the page opens with, so the focus is on the
    // control once its context is heard.
    scenario.speech().expect(&["Settings", "Settings window"]);
    let (name, control_type) = scenario
        .focused_control()
        .expect("the agent reads the focused control");
    let first = format!("{name} {}", role_word(control_type));
    scenario.speech().expect(&[&first]);
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
