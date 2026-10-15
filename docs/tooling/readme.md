# Tooling guide

The practical companion to `docs/architecture.md` and `docs/roadmap.md`: how
to actually drive this project day to day. Where the architecture doc
explains why something exists and the roadmap says when it lands, these
guides say which command to type. The tools are `cargo xtask` (the
standard check, measurement, traces, NVDA captures, and the VM harness),
`verbatim-inspect` against a running Verbatim, `mockapp`, and the
end-to-end suite with its in-guest agent. Read the rule below before
running anything that launches Verbatim or injects input.

Like every doc in this repository, these avoid ASCII diagrams, box
drawings, arrow chains, and pipe tables, so they read well with a screen
reader in both rendered and source form.

## The interactive-session rule

**Nothing launched over WinRM or PowerShell Direct can drive or be read by
a screen reader.** This is the interactive-session rule, and it is the
reason `verbatim-agent` exists at all instead of the harness just using
WinRM or PowerShell Direct directly: both put the launched process in a
non-interactive session ("session 0"), which has no input desktop a screen
reader could speak through or inject input into. The agent must run in the
guest's real autologon session, which is why
`Initialize-VerbatimHarness.ps1`'s `Register-VerbatimAgentTask` uses
`LogonType Interactive` with a real `UserId` (not SYSTEM, not "run whether
user is logged on or not") triggered `AtLogOn`, and why `verbatim-agent`
itself refuses to even bind a socket at startup — and `SessionInfo` reports
the same check live — when its own window station is not interactive.
`verbatim.exe` makes the same check and exits with a diagnosis, so a
Verbatim launched from WinRM or PowerShell Direct says why it cannot run. If
`session_info` (the E2E test, or `AgentClient::session_info` by hand) ever
reports `interactive_window_station: false`, something restarted the agent
outside that scheduled task; check the task's last run result in the guest
before debugging anything downstream.

## The guides

- [Setup and builds](setup.md): the submodules, CMake and wxWidgets,
  cargo-hakari and the workspace-hack crate, what `cargo xtask ci` scans,
  and the build's disk use.
- [Driving a running Verbatim with verbatim-inspect](inspect.md): the
  control plane's subcommands and the latency line.
- [Running mockapp by hand](mockapp.md): `mockapp` against a fixture, and
  which tests use the interactive desktop.
- [Measuring](measuring.md): "Measuring performance", with `cargo xtask
  measure`, `cargo xtask trace`, and what every measurement reports, and
  "Measuring the platform floor".
- [The end-to-end suite](e2e.md): test-audio mode, running the suite on
  this machine, `cargo xtask park`, the scenarios, the measure set, and
  hearing and recording a run.
- [The VM harness](../vm.md): every `cargo xtask vm` verb and rebuilding
  the golden image.
- [Recording what NVDA says](nvda.md): `cargo xtask nvda capture` and the
  rules it follows.
- [Troubleshooting](troubleshooting.md): the traps, each with its rule and
  its reason.
