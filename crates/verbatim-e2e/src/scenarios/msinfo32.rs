//! An MSAA-only legacy application (milestone M3's "at least one MSAA/IA2
//! legacy app" exit item): the System Information tool, `msinfo32.exe`.
//!
//! `msinfo32` is a classic Win32 application — a tree view of categories on
//! the left, a details list on the right — with no server-side UIA provider,
//! so Verbatim reads it through the MSAA client stack, and its window
//! arbitrates to MSAA rather than the UIA common-control proxies. This
//! scenario proves that path end to end: launching the tool brings its
//! window and the focused tree through Verbatim, read over MSAA.
//!
//! Deliberately minimal and tolerant: `msinfo32` populates its data
//! asynchronously and its exact focus target and wording vary across Windows
//! builds, so the assertions are substring matches on the stable pieces (the
//! window title and the default "System Summary" category), not an exact
//! walk. It runs non-elevated, so no `UIAccess` is needed (that is M8).

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Generous per-step budget: `msinfo32` is slow to open and populate on a
/// loaded VM.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let pid = scenario.launch_target("msinfo32.exe", &[])?;
    Ok(ScenarioState::TargetPid(pid))
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // Launching msinfo32 brings its window to the foreground; Verbatim
    // announces the window and then the focused control through the MSAA
    // stack. The window is titled "System Information".
    scenario
        .speech()
        .expect_in_order(&["System Information"], STEP_TIMEOUT);

    // Focus settles in the category tree on the default "System Summary"
    // node. Assert it as the settled focus, tolerating the intermediate
    // window and pane announcements.
    scenario
        .speech()
        .expect_in_order(&["System Summary"], STEP_TIMEOUT);
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
