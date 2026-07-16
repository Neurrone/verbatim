//! Object navigation and the review cursor (milestone M3 reducer item 4),
//! driven against Verbatim's own settings dialog — the same self-voicing
//! target the M1 exit regression uses, so it needs no external application.
//!
//! The walk opens the Speech settings dialog (focus lands in the category
//! list), then exercises the object-navigation gestures through the control
//! plane's `SendGesture`, which routes them exactly as a physical keypress
//! would: report the current object, move to the list's first child (the
//! "Speech" category item), report again to confirm the navigator moved,
//! then snap the navigator back to focus. Assertions are substring matches,
//! tolerant of the platform controls' own wording, matching the M1
//! regression's style.
//!
//! Gestures use the desktop layout's bindings (the default), addressed by
//! their stable identifiers rather than raw numpad keystrokes, so `NumLock`
//! state cannot affect the run.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Per-step speech timeout, matching the M1 regression's generous budget for
/// a loaded VM.
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    // Nothing external: the walk drives Verbatim's own settings dialog.
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // Open the Speech settings dialog. Focus settles, after an intermediate
    // step, on the selected category *item* inside the category list — the
    // dialog announces the list and its "Speech" selected item, and focus
    // then lands on that item. Waiting for the item as the settled focus
    // tolerates the intermediate list announcement and lets the focus
    // sequence finish before the navigator commands below run.
    super::open_verbatim_menu(scenario, STEP_TIMEOUT);
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&["Settings", "menu item"], STEP_TIMEOUT);
    scenario.send_keys(&["enter"]).expect("sends enter");
    scenario
        .speech()
        .expect_in_order(&["Speech", "list item"], STEP_TIMEOUT);

    // Report the current navigator object: the navigator follows focus, so
    // this re-announces the focused list item.
    scenario
        .send_gesture("kb:verbatim+numpad5")
        .expect("sends report-current-object");
    scenario
        .speech()
        .expect_in_order(&["Speech", "list item"], STEP_TIMEOUT);

    // Move the navigator to the item's parent — the category list — proving
    // an object-navigation query round-trips through the outpost and moves
    // the navigator.
    scenario
        .send_gesture("kb:verbatim+numpad8")
        .expect("sends move-to-parent");
    scenario
        .speech()
        .expect_in_order(&["Categories", "list"], STEP_TIMEOUT);

    // Move to the list's first child: back to the "Speech" item.
    scenario
        .send_gesture("kb:verbatim+numpad2")
        .expect("sends move-to-first-child");
    scenario
        .speech()
        .expect_in_order(&["Speech", "list item"], STEP_TIMEOUT);

    // Wander to the parent again, then snap the navigator back to focus with
    // the to-focus command: the focused item.
    scenario
        .send_gesture("kb:verbatim+numpad8")
        .expect("sends move-to-parent");
    scenario
        .speech()
        .expect_in_order(&["Categories", "list"], STEP_TIMEOUT);
    scenario
        .send_gesture("kb:verbatim+numpadminus")
        .expect("sends move-review-to-focus");
    scenario
        .speech()
        .expect_in_order(&["Speech", "list item"], STEP_TIMEOUT);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {
    // Nothing to restore: the dialog closes when Verbatim quits at the end
    // of the run, and no external application was launched.
}
