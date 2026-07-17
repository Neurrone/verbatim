# verbatim-e2e

The end-to-end suite, restructured in milestone M3 Track B into a scenario
registry: drives a real, running Verbatim (and target applications such as
Notepad) through `verbatim-agent` and, tunneled through it, Verbatim's own
control plane. Dev-only; a library rather than only test binaries because
both `crates/verbatim-e2e/tests/` and `xtask vm test` drive it. See
`docs/tooling.md` for how to run it by hand and how to read a failure.

Public API:

- `endpoint()` — reads `ENDPOINT_ENV` (`VERBATIM_E2E_ENDPOINT`), the
  live-suite skip guard every scenario in this crate checks first; `None`
  means no agent is reachable, and callers print a one-line skip notice and
  return rather than failing. This is what keeps `cargo test` and
  `cargo xtask ci` green with no agent anywhere.
- `AgentClient` — a typed host-side client for `verbatim_agent::protocol`:
  connects over TCP, completes the agent's `Hello` handshake, and exposes
  `launch_process`, `kill_process`, `process_status`, `session_info`,
  `read_file`, and `open_control_tunnel` as plain methods.
  `open_control_tunnel` is the seam into Verbatim's own control plane: it
  asks the agent to stop speaking its own protocol on the connection and
  relay Verbatim's control-plane pipe instead, then completes the control
  protocol's own `Hello` on the same socket and hands back a ready
  `verbatim_control::client::Client`.
- `Scenario` — the lifecycle owner for one live, agent-driven Verbatim run:
  a guard struct, not a manual-cleanup checklist. `Scenario::launch` writes
  a `settings.toml` selecting the capture synthesizer next to
  `verbatim.exe` (audio-free, no installed voices needed — deliberately
  not `OneCore`, whose `new` fails outright with none installed), launches
  Verbatim through the agent with `VERBATIM_TEST_AUDIO=null`, waits for its
  control plane to answer over the agent's tunnel, opens a *second*,
  dedicated tunnel connection for speech collection, and pauses briefly
  (`GUI_SETTLE_DELAY`) for the GUI thread's gesture handle to exist before
  returning — see `docs/tooling.md`'s troubleshooting section for what
  happens to a gesture sent before that pause. `control()` and `speech()`
  expose the two connections; `send_gesture`, `send_keys`, `launch_target`,
  `kill_target`, `process_status`, `quit_verbatim`, and `report_latency`
  drive the running instance; `latency_snapshot` is the non-asserting,
  non-printing fetch the registry's run summary uses (see `registry`
  below), and `collect_run_artifacts` (timeline, stderr, and the per-process
  outpost and listener logs fetched from the guest's `logs` directory — Core's
  own outpost, the listener, and each launched target's outpost by pid) plus
  `collect_flight_recorder` (the reducer flight recorder, dumped before the
  clean quit) are what every run calls to save its diagnostics, pass or fail
  (see `artifacts` below). Its `Drop` impl kills every
  process it launched, unconditionally, even after a panic — because every
  live scenario here launches a real `verbatim.exe` on the real desktop. A
  same-process `Mutex` (`live_instance_lock`) enforces one live instance
  per test *process*, not per machine; `--test-threads=1` (mandatory,
  documented on the type) is what makes that sufficient, since Windows has
  no notion of "only one verbatim.exe" and `single_instance
  ::acquire_replacing` inside `verbatim.exe` *replaces* a running instance
  rather than refusing to start. The hardcoded `SWEPT_TARGET_IMAGE_NAMES`
  constant this pre-launch sweep once read from is gone: it now reads
  `registry::swept_target_image_names()`, derived from every registered
  scenario's own declared target images instead of a name maintained by
  hand.
- `registry` — the scenario registry itself. `ScenarioDef` is one named,
  grouped scenario: `name` (also its `#[test]` function name, its
  `cargo xtask vm test --scenario` selector, its artifacts directory name,
  and its recording file name prefix — one identifier, everywhere),
  `group` (a `Group`: `Speech`, `Shell`, `Legacy`, or `Navigation`, a coarse
  `--group` selector, not a strict taxonomy — see the module's own doc
  comment for what each currently holds), `target_images` (image names its
  `setup`/`teardown` may launch or kill, unioned by
  `swept_target_image_names`), and `setup`/`body`/`teardown` function
  pointers. `SCENARIOS` is the fixed, ordered list of every registered
  scenario — today `m1_exit_regression`, `notepad_focus`, and
  `multi_outpost_switch`, each implemented in `crates/verbatim-e2e/src/
  scenarios/`. `find` looks one up by name; `select` resolves
  `--scenario`/`--group` filters (both repeatable, unioned, deduplicated,
  registry order preserved, empty means "every scenario") into a list,
  erroring on any unrecognized name; `run_named` is the thin entry point
  every `#[test]` wrapper under `crates/verbatim-e2e/tests/` calls.
  `run_named`'s internal `run` launches, runs `setup` then `body` then
  `teardown` — `body` and `teardown` each in their own
  `std::panic::catch_unwind`, so a panicking `body` still lets `teardown`
  run with whatever `setup` produced (borrowed, not moved, so the panic
  leaves it intact) rather than skipping cleanup — asserts a clean
  `quit_verbatim` only when both succeeded (an already-failed scenario's
  Verbatim is in an unknown state, and `Scenario::drop` kills it regardless,
  so nothing more is proved by also demanding a graceful quit), collects
  the run artifacts (timeline, stderr, and the reducer flight recorder — the
  last dumped before the quit while Verbatim is still up) for every run pass
  or fail, always writes a `ScenarioSummary`, then re-raises whatever panic
  occurred so `cargo test` still reports the original failure. None of this weakens `Scenario`'s own guard-struct
  discipline; `setup`/`body`/`teardown` are structure on top of it for
  scenario-specific state `Scenario` itself does not track, not a
  replacement for it. `crates/verbatim-e2e/src/scenarios/` holds the actual
  setup/body/teardown logic per scenario — the scripted walks themselves are
  otherwise unchanged from before the restructuring, just moved out of
  `#[test]` functions into free functions the registry wires together.
- `artifacts` — the host-side seam both a scenario subprocess and
  `cargo xtask vm test` read through without any argument passing between
  them, since both independently compute the same paths.
  `artifacts_root()` is `VERBATIM_E2E_ARTIFACTS_DIR` when set, otherwise
  `target/e2e-artifacts` under the workspace root; `scenario_dir(root,
  name)` joins in the scenario's name. `ScenarioSummary` (`name`, `passed`,
  `latency_records`, `latency_reached_audio`) is written by
  `ScenarioSummary::write` at the end of every scenario run, pass or fail,
  as plain `key: value` lines, and read back by `ScenarioSummary::read` —
  `xtask vm test`'s own run summary is built from this file, never by
  parsing a subprocess's stdout.
- `latency::fetch` — fetches the most recent `last_n` latency timelines with
  no printing and no assertion, the raw building block `report` (below) and
  `Scenario::latency_snapshot` both use.
- `latency::report` — `fetch`, then prints one fact per line and asserts at
  least one timeline reached audio — but only outside audible mode, since a
  real synthesizer is legitimately interrupted before playback at this
  suite's pace (a capture-synth invariant, not a real-synth one).

Implementation notes: `REMOTE_ENV` (`VERBATIM_E2E_REMOTE`) marks a run where
Verbatim lives in a guest rather than sharing this process's filesystem —
set by `xtask vm test`, not normally by hand — and skips the two ordinary
host-filesystem steps (`verbatim.exe` existence check, writing
`settings.toml`) that `xtask vm deploy` has already done inside the guest
instead. `crates/verbatim-e2e/tests/` holds one thin `#[test]` wrapper per
registered scenario (`m1_exit_regression`, `notepad_focus`,
`multi_outpost_switch`, each just calling `registry::run_named` with its own
name) plus `session_info` (the agent reports an interactive session — a
precondition every scenario depends on, not itself a scenario, so it stays a
plain `#[test]` outside the registry). The thin wrappers are what keep
runner-direct CI (`.github/workflows/ci.yml`'s `e2e` job) and plain libtest
filtering (`cargo test -p verbatim-e2e <name> -- --exact`) working
unchanged: `cargo test -p verbatim-e2e -- --test-threads=1` still discovers
and runs every one of them exactly as before the restructuring.
`m1_exit_regression` is the scripted walk of the M1 exit criteria that
`docs/roadmap.md`'s M2 section describes, including exactly what it does and
does not assert about the capture synth's Speech page; `notepad_focus` and
`multi_outpost_switch` are described in `registry`'s own `Group` doc comment
above.
