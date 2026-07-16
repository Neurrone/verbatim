//! Object navigation through a real Win32 tree view (milestone M3):
//! msinfo32's `SysTreeView32` category tree, read over MSAA. Regression
//! coverage for the flat-exposure defect found in live testing: MSAA
//! presents every visible tree item as a flat sibling list under the tree
//! control, so before the `TVM_*`-based logical navigation in
//! `verbatim_ia2::acquire`, next-sibling moved into a node's own children,
//! previous-sibling moved to its logical parent, parent landed on the
//! unnamed tree control (spoken as just "unknown"), and first-child was
//! always a silent edge. Every assertion below fails against that behavior.
//!
//! The walk: focus starts on the "System Summary" root item; move to its
//! first child ("Hardware Resources"), to the next sibling ("Components"),
//! back to the previous sibling ("Hardware Resources"), up to the parent
//! ("System Summary"), up again onto the tree control itself (role "tree
//! view", never an item), and finally snap the navigator back to focus.
//! Substring matches, tolerant of state wording, like the other scenarios.
//! The tree-control step additionally captures the full utterance and
//! asserts it is not an item announcement, since "tree view" is a
//! substring of "tree view item" and a wrong landing must not pass.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Generous per-step budget: msinfo32 is slow to open and populate on a
/// loaded VM.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let pid = scenario.launch_target("msinfo32.exe", &[])?;
    Ok(ScenarioState::TargetPid(pid))
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // msinfo32 opens with its window ("System Information") and focus on the
    // tree's "System Summary" root item. Asserted as one ordered matcher
    // sequence rather than two separate waits: under load the window title
    // and the focused item can arrive as a single combined announcement, and
    // two waits would let the first consume the line the second needs. One
    // `expect_in_order` advances through all three substrings whether they
    // land on one line or several.
    scenario.speech().expect_in_order(
        &["System Information", "System Summary", "tree view item"],
        STEP_TIMEOUT,
    );

    // First child of the root: "Hardware Resources", one level deeper.
    scenario
        .send_gesture("kb:verbatim+numpad2")
        .expect("sends move-to-first-child");
    scenario
        .speech()
        .expect_in_order(&["Hardware Resources", "tree view item"], STEP_TIMEOUT);

    // Next sibling: "Components" — a real sibling; the flat exposure used
    // to answer the next visible item, descending into children instead.
    scenario
        .send_gesture("kb:verbatim+numpad6")
        .expect("sends move-to-next-sibling");
    scenario
        .speech()
        .expect_in_order(&["Components", "tree view item"], STEP_TIMEOUT);

    // Previous sibling: back to "Hardware Resources".
    scenario
        .send_gesture("kb:verbatim+numpad4")
        .expect("sends move-to-previous-sibling");
    scenario
        .speech()
        .expect_in_order(&["Hardware Resources", "tree view item"], STEP_TIMEOUT);

    // Parent: the logical parent item "System Summary", not the tree
    // control (the flat exposure used to answer the control for every
    // item).
    scenario
        .send_gesture("kb:verbatim+numpad8")
        .expect("sends move-to-parent");
    scenario
        .speech()
        .expect_in_order(&["System Summary", "tree view item"], STEP_TIMEOUT);

    // Parent from the root item: the tree control itself, spoken by its
    // bare role since msinfo32's tree control is unnamed — and never as an
    // item, which the captured text rules out ("tree view" is a substring
    // of "tree view item", so the matcher alone cannot).
    scenario
        .send_gesture("kb:verbatim+numpad8")
        .expect("sends move-to-parent");
    let spoken = scenario
        .speech()
        .expect_in_order_capturing(&["tree view"], STEP_TIMEOUT);
    assert!(
        !spoken.contains("tree view item"),
        "parent of the root item must be the tree control, not an item; heard: {spoken}"
    );

    // Snap the navigator back to focus: the focused "System Summary" item.
    scenario
        .send_gesture("kb:verbatim+numpadminus")
        .expect("sends move-review-to-focus");
    scenario
        .speech()
        .expect_in_order(&["System Summary", "tree view item"], STEP_TIMEOUT);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(scenario: &mut Scenario, state: ScenarioState) {
    if let ScenarioState::TargetPid(pid) = state {
        scenario
            .kill_target(pid)
            .expect("kills msinfo32 through the agent");
    }
}
