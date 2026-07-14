//! Smoke test: the M2 agent reports an interactive session — the
//! precondition every other live test in this crate depends on. A screen
//! reader driven from a non-interactive session (session 0, `WinRM`,
//! PowerShell Direct) can never work, so this is the first thing worth
//! checking when the suite behaves strangely.

use verbatim_e2e::AgentClient;

#[test]
fn agent_reports_an_interactive_window_station() {
    let Some(endpoint) = verbatim_e2e::endpoint() else {
        println!("VERBATIM_E2E_ENDPOINT is not set; skipping the live E2E suite");
        return;
    };

    let mut agent =
        AgentClient::connect(&endpoint).expect("connects to the agent and completes Hello");
    let info = agent.session_info().expect("agent answers SessionInfo");

    assert!(
        info.interactive_window_station,
        "agent's window station is not interactive (session {}); the E2E suite cannot drive a screen reader from here",
        info.session_id
    );
    assert!(
        info.input_desktop_name.is_some(),
        "agent reports an interactive window station but no input desktop"
    );
    println!(
        "agent session {} interactive, input desktop {}",
        info.session_id,
        info.input_desktop_name.as_deref().unwrap_or("(none)")
    );
}
