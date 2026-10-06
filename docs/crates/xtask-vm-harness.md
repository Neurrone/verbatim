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
  `settings.toml` selecting eSpeak NG (`Settings::for_e2e("espeak")`),
  the same synthesizer `verbatim_e2e::scenario` selects for runner-direct
  runs, ships the `sounds` directory next to the built `verbatim.exe` the
  same way as the eSpeak NG data (one archive, unpacked in the guest), and
  copies every file of the `espeak-ng-data` directory found
  next to the built `verbatim-synth-host.exe` into the guest's Verbatim
  folder, one artifact per file so each hash-skips on its own; the data
  files count as executables for stopping the guest, since a running
  synthesizer host may hold them open. It also copies the vendored
  `ffmpeg.exe` to `FFMPEG_GUEST_PATH`, after checking it is the real
  binary and not a Git LFS pointer. There is no `--audible` flag on
  the VM path, since `test` is audible by default. When an
  executable must be copied, `deploy` stops the guest's `VerbatimAgent`
  task first and restarts it afterwards even if a copy fails
  (`copy_then_restart`), so a failed deploy never leaves the agent down.
- `test` (milestone M3 Track B: per-scenario selection and boundaries,
  replacing "the whole suite runs as one blob with one recording"). `xtask`
  now depends on `verbatim-e2e` directly, reading `registry::SCENARIOS` and
  `registry::select` rather than duplicating the scenario catalog.
  `--list` prints the registry (name and group) and exits, touching neither
  build nor VM. Otherwise `test::test` resolves `--scenario`/`--group` via
  `registry::select` (erroring out before any build or restore on an
  unrecognized name, or on a demonstration, a scenario of the `demo`
  group, which only `cargo xtask demo` runs), builds, readies the guest (starting it if needed, or
  restoring `golden` first when `--restore` is given), deploys,
  then runs `session_info`'s own test as a precondition — once, unrecorded,
  regardless of selection — before entering `run_one_scenario`'s per-scenario
  loop: each scenario runs as its own `cargo test -p verbatim-e2e <name> --
  --exact` subprocess (`run_scenario_subprocess`), with
  `VERBATIM_E2E_FFMPEG` set to `FFMPEG_GUEST_PATH`, so the scenario's
  `Scenario::launch`, setup, body, teardown, and its recording (started
  by `Scenario::launch` and finished by `verbatim_e2e::registry::run`;
  see the [verbatim-e2e guide](verbatim-e2e.md)) all happen inside that
  one subprocess; this is the module's own answer to "how does `xtask`
  control scenario boundaries" (a dedicated multi-scenario runner mode inside
  `verbatim-e2e` was the other option considered — see the module's doc
  comment for why a fresh, exactly-filtered subprocess per scenario was
  chosen instead: it gets a process-lifetime boundary for free, no new IPC).
  Before each subprocess starts, `run_one_scenario` clears that scenario's
  artifacts directory; after it exits, it reads back the
  `verbatim_e2e::artifacts::ScenarioSummary` that scenario's own run wrote,
  rather than parsing the subprocess's stdout. Because of the clearing, a
  subprocess that never reached the scenario runner leaves no summary and
  is reported as a failure, never as the previous run's result; `print_run_summary` prints
  the final one-line-per-scenario pass/fail-plus-latency report. No retry of
  any kind exists at this level either: one scenario's failure is
  accumulated into the run's error list and the loop continues to the next
  scenario, never re-running the one that failed.
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
`vm/scripts/Initialize-VerbatimHarness.ps1`'s own paths; and
`FFMPEG_GUEST_PATH` (`C:\VerbatimLab\tools\ffmpeg.exe`) is where `deploy`
puts ffmpeg and what `test` names in `VERBATIM_E2E_FFMPEG`.
