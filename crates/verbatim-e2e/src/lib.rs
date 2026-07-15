//! Milestone M2 end-to-end suite, restructured in milestone M3 Track B into
//! a scenario registry.
//!
//! This crate is dev-only: it drives a real, running Verbatim (and target
//! applications such as Notepad) through the M2 in-guest agent
//! (`verbatim-agent`, `verbatim_agent::protocol`) and, once tunneled
//! through it, Verbatim's own control plane (`verbatim_control`). It is a
//! library so the thin test binaries under `tests/` and `xtask vm test` can
//! both drive it.
//!
//! [`registry`] is the entry point for the restructuring: every live
//! scenario is a named, grouped [`registry::ScenarioDef`] — setup, body,
//! teardown — instead of logic living only inside a `#[test]` function.
//! [`registry::run_named`] is what each thin `#[test]` wrapper under
//! `crates/verbatim-e2e/tests/` calls, and what `cargo xtask vm test`
//! selects by name or group ([`registry::select`]) and invokes once per
//! scenario (one `cargo test` subprocess per scenario, so a `--record`
//! recording's boundary is exactly one scenario's setup, body, and
//! teardown — see `docs/tooling.md`). [`artifacts`] is the host-side
//! artifacts (a summary, and on failure the timeline, Verbatim's stderr
//! log, and a flight-recorder dump) both sides of that process boundary
//! agree on without any argument passing between them.
//!
//! Every live scenario needs an already-running agent to connect to. None
//! of them start one: `cargo test` and `cargo xtask ci` must stay green
//! with no agent and no Verbatim anywhere, so [`registry::run_named`]
//! checks [`endpoint`] first and prints a one-line skip notice instead of
//! running when it is unset. `session_info`
//! (`crates/verbatim-e2e/tests/session_info.rs`) is not a scenario — a
//! precondition check every scenario depends on, not a setup/body/teardown
//! walk — so it stays a plain `#[test]` outside the registry, checking
//! [`endpoint`] itself the same way.
//!
//! Local verification: build `verbatim-app` and `verbatim-agent` (debug),
//! start the agent with `--bind-address 127.0.0.1` (loopback avoids a
//! firewall prompt on a dev machine; the default port,
//! `verbatim_agent::protocol::DEFAULT_PORT`, is deliberately not in the
//! 47000s — see that constant's doc comment for why), export
//! `VERBATIM_E2E_ENDPOINT=127.0.0.1:44001`, and run
//! `cargo test -p verbatim-e2e -- --test-threads=1`. Tests run strictly
//! serially by design (documented on [`scenario::Scenario`]): this suite
//! launches a real Verbatim on the developer's live desktop, and
//! `--test-threads=1` is how both a local run and CI honor that.

pub mod agent_client;
pub mod artifacts;
pub mod latency;
pub mod registry;
pub mod scenario;
mod scenarios;
pub mod speech;
pub mod timeline;

pub use agent_client::AgentClient;
pub use scenario::Scenario;
pub use speech::SpeechCollector;
pub use timeline::Timeline;

/// Environment variable naming the endpoint (`host:port`) of an already
/// running `verbatim-agent`. Every live test in this crate checks
/// [`endpoint`] first and skips instead of failing when it is unset.
pub const ENDPOINT_ENV: &str = "VERBATIM_E2E_ENDPOINT";

/// Reads [`ENDPOINT_ENV`], the live-suite skip guard every test in this
/// crate checks first. `None` means "no agent is reachable"; callers print
/// a one-line skip notice and return rather than failing.
#[must_use]
pub fn endpoint() -> Option<String> {
    std::env::var(ENDPOINT_ENV)
        .ok()
        .filter(|value| !value.is_empty())
}
