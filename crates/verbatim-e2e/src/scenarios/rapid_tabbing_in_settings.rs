//! Rapid focus churn (outpost redesign, step 7): many focus changes in quick
//! succession inside one application must leave Verbatim's focus and
//! navigator on the control that really has focus, with the outpost still
//! answering. A stress scenario of this kind was written in July and never
//! committed; this recreates it against Verbatim's own settings dialog, which
//! every run has, so it needs no external application.
//!
//! The churn is a single burst of real keystrokes: Tab several times, then
//! Shift+Tab the same number of times, sent in one request so they arrive
//! faster than the outpost reads each focus. Tab order is a fixed cycle, so
//! the burst ends where it started, on the selected category item. The
//! outpost's limiter keeps only the newest focus events of each batch, and
//! the reducer accepts a focus only from the attention window; both are
//! exercised here.
//!
//! The intermediate announcements are not asserted, since which of them are
//! spoken depends on timing. After the burst the scenario reads and discards
//! speech until the desktop has been quiet for a while, then asks for the
//! current navigator object: it must be the category item, which proves the
//! final focus won, and the reply proves the outpost still answers queries.

use std::io;
use std::time::{Duration, Instant};

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// Per-step speech timeout, matching the other live scenarios.
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest the burst's focus may take to settle back on the category
/// item; it only bounds a failure.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How many Tab presses the burst makes before coming back.
const TABS: usize = 6;

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    // Nothing external: the churn drives Verbatim's own settings dialog.
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_speech_settings(scenario, STEP_TIMEOUT);

    // The burst: away and back again in one request.
    let burst: Vec<&str> = std::iter::repeat_n("tab", TABS)
        .chain(std::iter::repeat_n("shift+tab", TABS))
        .collect();
    scenario.send_keys(&burst).expect("sends the burst");

    // The navigator follows focus, so reporting it names the control that
    // has focus. The burst's focus events may still be arriving when the
    // report is answered, and when the burst ends where it began nothing
    // more is spoken, so there is nothing to wait for first. Instead the
    // report is repeated after each later announcement until it names the
    // category item the burst returned to; a final focus that is wrong
    // never does, and fails at the deadline.
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        scenario
            .send_gesture("kb:verbatim+numpad5")
            .expect("sends report-current-object");
        let report = scenario.speech().expect_change_capturing("", STEP_TIMEOUT);
        if report.contains("Speech") && report.contains("list item") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the focus never settled on the Speech category item; last report {report:?}"
        );
        scenario
            .speech()
            .expect_change_capturing(&report, STEP_TIMEOUT);
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {
    // Nothing to restore: the dialog closes when Verbatim quits at the end
    // of the run, and no external application was launched.
}
