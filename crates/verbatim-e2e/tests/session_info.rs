//! The M2 agent reports an interactive session on the default desktop: the
//! precondition every live test in this crate depends on. A screen reader
//! driven from a non-interactive session (session 0, `WinRM`, PowerShell
//! Direct) can never work, and a locked machine's input desktop is the
//! secure desktop, where the suite does nothing useful, so this is the
//! first thing worth checking when the suite behaves strangely. Like every
//! live test, it is ignored by a plain `cargo test` and run with
//! `--ignored`, and it fails when no agent is named.

use verbatim_e2e::AgentClient;

#[test]
#[ignore = "live: needs a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn agent_reports_an_interactive_window_station() {
    let endpoint = verbatim_e2e::endpoint().unwrap_or_else(|| {
        panic!(
            "{} is not set: a live test needs a running agent",
            verbatim_e2e::ENDPOINT_ENV
        )
    });
    let mut agent =
        AgentClient::connect(&endpoint).expect("connects to the agent and completes Hello");
    let info = agent.session_info().expect("agent answers SessionInfo");
    assert!(
        info.interactive_window_station,
        "agent's window station is not interactive (session {}); the E2E suite cannot drive a screen reader from here",
        info.session_id
    );
    assert_eq!(
        info.input_desktop_name.as_deref(),
        Some("Default"),
        "the input desktop is not the default desktop: the machine is locked, or the secure desktop is up"
    );
}
