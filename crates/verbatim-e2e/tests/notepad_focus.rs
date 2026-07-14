//! Regression: launching Notepad brings its focus announcement through
//! Verbatim, and Verbatim keeps running after Notepad exits.
//!
//! Windows 11's Notepad is tabbed, so the window-name check is generous (a
//! substring match on "Notepad") rather than an exact title. Deliberately
//! minimal: typed-character echo is a later milestone, so this test proves
//! only that focus tracking reaches a second, real application and that
//! losing it does not take Verbatim down too.

use std::time::Duration;

use verbatim_control::client::ok_or_error;
use verbatim_control::protocol::Request;
use verbatim_e2e::Scenario;

#[test]
fn notepad_focus_reaches_verbatim_and_verbatim_survives_notepad_exit() {
    let Some(_endpoint) = verbatim_e2e::endpoint() else {
        println!("VERBATIM_E2E_ENDPOINT is not set; skipping the live E2E suite");
        return;
    };

    let mut scenario = Scenario::launch().expect("launches Verbatim through the agent");

    let notepad_pid = scenario
        .launch_target("notepad.exe", &[])
        .expect("launches notepad through the agent");

    scenario
        .speech()
        .expect_in_order(&["Notepad"], Duration::from_secs(10));

    scenario
        .kill_target(notepad_pid)
        .expect("kills notepad through the agent");

    let status = ok_or_error(
        scenario
            .control()
            .request(Request::Status)
            .expect("sends Status"),
    )
    .expect("Verbatim still answers Status after Notepad exits");
    println!("status after notepad exit: {status:?}");
}
