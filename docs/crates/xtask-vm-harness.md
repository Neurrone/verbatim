# xtask VM harness

`xtask/src/vm/` implements `cargo xtask vm <verb>` (`docs/architecture.md`
section 14, decision D3): building and importing the golden Hyper-V VM,
deploying builds into it, and running `verbatim-e2e`'s registered scenarios
against it, one at a time (milestone M3 Track B). See `docs/tooling.md` for
the full verb reference and the steps to rebuild the golden image from
scratch; this section is the code-level map.

Public structure (all `pub(crate)`; this is a binary target's internal
module tree, not a library):

- `host` — the `Host` trait: every Hyper-V (or, later, other hypervisor)
  operation a verb needs, abstract enough that a non-Hyper-V implementation
  is plausible (`vm_exists`, `import_vm`, `rename_vm`,
  `ensure_guest_file_transfer`, `start_vm`/`stop_vm`/`restart_vm`,
  `checkpoint_vm`, `restore_checkpoint`, `delete_vm`, `guest_ip`,
  `copy_file_to_guest`, `run_in_guest`, `read_guest_file`,
  `list_guest_dir`). `HyperVHost` is the only implementation, over Hyper-V's
  PowerShell module; every method drives `powershell.exe -File` against a
  temp script (never `-Command`, so arguments only ever cross one layer of
  parsing) and, where a value must come back, wraps it in unique text
  markers rather than trusting a cmdlet's own stdout is clean (some Hyper-V
  cmdlets write incidental text to the success stream). `wait_for_agent`
  is free-standing rather than a trait method: it is pure orchestration
  over other `Host` calls (poll `guest_ip`, then raw-TCP-probe the agent's
  port), so a future non-Hyper-V host gets it for free.
- `create`, `deploy`, `test`, `lifecycle` (`start`/`stop`/`restart`
  /`restore`/`delete`), `logs`, `connect` — one module per verb or verb
  family, each orchestrating `Host` calls; `mod.rs` dispatches
  `cargo xtask vm <verb>` to them. `deploy::stage_and_copy` always stages a
  `settings.toml` selecting the real `OneCore` synthesizer now — there is
  no more capture-synth choice or `--audible` flag on the VM path, since
  `test` is audible by default; the capture synth remains the runner-direct
  default, independently, in `verbatim_e2e::scenario`. `deploy::build`
  probes for `libclang.dll` before its `cargo build` the same way
  `xtask`'s own `ci` command does (reusing `find_libclang`), since building
  `verbatim-app` pulls in `verbatim-gui`'s wxDragon dependency.
- `test` (milestone M3 Track B: per-scenario selection and boundaries,
  replacing "the whole suite runs as one blob with one recording"). `xtask`
  now depends on `verbatim-e2e` directly, reading `registry::SCENARIOS` and
  `registry::select` rather than duplicating the scenario catalog.
  `--list` prints the registry (name and group) and exits, touching neither
  build nor VM. Otherwise `test::test` resolves `--scenario`/`--group` via
  `registry::select` (erroring out before any build or restore on an
  unrecognized name), builds, restores (unless `--no-restore`), deploys,
  then runs `session_info`'s own test as a precondition — once, unrecorded,
  regardless of selection — before entering `run_one_scenario`'s per-scenario
  loop: `--record` (when set) brackets exactly that scenario's own `cargo
  test -p verbatim-e2e <name> -- --exact` subprocess with
  `recording::start_recording_with_fallback`/`stop_recording`/
  `pull_recording`, so the recording's boundary is exactly one scenario's
  `Scenario::launch`, setup, body, and teardown — all of which run inside
  that one subprocess — never spilling into a neighboring scenario's
  recording; this is the module's own answer to "how does `xtask` control
  scenario boundaries" (a dedicated multi-scenario runner mode inside
  `verbatim-e2e` was the other option considered — see the module's doc
  comment for why a fresh, exactly-filtered subprocess per scenario was
  chosen instead: it gets a process-lifetime boundary for free, no new IPC).
  After each subprocess exits, `run_one_scenario` reads back the
  `verbatim_e2e::artifacts::ScenarioSummary` that scenario's own run wrote,
  rather than parsing the subprocess's stdout; `print_run_summary` prints
  the final one-line-per-scenario pass/fail-plus-latency report. No retry of
  any kind exists at this level either: one scenario's failure is
  accumulated into the run's error list and the loop continues to the next
  scenario, never re-running the one that failed.
- `recording` — `test`'s `--record` flag, now started and stopped around
  one scenario at a time (see `test` above) rather than once for the whole
  run: a small client speaking `verbatim_agent::protocol` directly (`Hello`,
  `LaunchProcess`, `ProcessStatus`, `KillProcess`; not `Host`, and not
  `verbatim-e2e`'s own fuller `AgentClient` — see the module's doc comment
  for why) to pin VB-CABLE as the guest's default render device, launch
  ffmpeg inside the guest's interactive session, confirm it is still running
  a moment later, terminate it once that scenario's subprocess finishes, and
  pull the fragmented-MP4 result back to `artifacts/vm-recordings` on the
  host via `Host::read_guest_file` (the same PowerShell Direct mechanism
  `logs` uses, since `Copy-VMFile` only copies host-to-guest).
  `pull_recording` now takes the scenario's name and names the file after
  it (`<scenario_name>-<unix-seconds>[-no-audio].mp4`), one recording per
  scenario instead of one per run. Recording audio and a connected RDP
  session are mutually exclusive (`docs/tooling.md` has the full constraint
  and why); `test`'s own `start_recording_with_fallback` treats a failure to
  pin the render device, or ffmpeg exiting immediately after an
  audio-capturing launch, as the expected fallout of a connected session —
  not fatal — and retries `recording::start_recording` with
  `with_audio: false` instead, so the scenario still runs and a video-only
  recording is still pulled. `recording::pull_recording`'s own
  ffprobe-based check, not which launch path was taken, is what decides the
  pulled file's `-no-audio` filename tag.
- `dotenv` — a minimal hand-rolled `.env` reader (`KEY=VALUE` lines,
  comments, quoting) for `VERBATIM_VM_USERNAME`/`VERBATIM_VM_PASSWORD` from
  the repository-root `.env`, deliberately not a crate dependency for a
  format this small.
- `packer_build` — wraps `vm/scripts/Build-VerbatimWindows11Image.ps1` and
  locates the `.vmcx` it exports (reading `output_directory` out of
  `vm/local.pkrvars.hcl` with a minimal line-oriented HCL reader, the same
  approach `dotenv` takes) so `create` can hand it to `Import-VM`.

Constants worth knowing when reading any of the above: `VM_NAME` is always
`verbatim`; `CHECKPOINT_NAME` is `golden`; `AGENT_PORT` is 44001, duplicated
from `verbatim_agent::protocol::DEFAULT_PORT` rather than depending on that
crate for one constant; `VERBATIM_DIR` (`C:\VerbatimLab\verbatim`) and
`AGENT_DIR` (`C:\VerbatimLab\agent`) are the guest install paths `deploy`
writes into and `logs` reads out of, matching
`vm/scripts/Initialize-VerbatimHarness.ps1`'s own paths.
