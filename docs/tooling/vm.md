# The VM harness

How to drive the Hyper-V end-to-end harness: every `cargo xtask vm`
verb, rebuilding the golden image from scratch, and where each verb lives
in the code. The rest of [the tooling guide](readme.md) covers
verbatim-inspect, mockapp, and running the end-to-end suite against this
machine; [its troubleshooting guide](troubleshooting.md) covers VM
failures too. The harness implements `docs/architecture.md` section 14,
decision D3.

## cargo xtask vm verbs

`cargo xtask vm <verb>` wraps Hyper-V PowerShell behind a `Host` trait
(`xtask/src/vm/host.rs`); every verb module talks to `&dyn Host` only, so
a non-Hyper-V implementation (QEMU or KVM on Linux runners, the deferred
CI story of `docs/architecture.md` section 14) could be added without
rewriting verb logic. Run `cargo xtask vm` with no arguments for the full
verb list printed from the source of truth.

- `create` builds the base image with Packer (see the next section; this
  is the slow one), imports the exported VM, renames it to `verbatim`,
  enables the Guest Service Interface (Hyper-V's file-copy integration
  service, off by default and required by `deploy`), starts it, deploys a
  debug build so the checkpoint below already has a running agent, waits
  for the agent to answer, then checkpoints the VM as `golden`. It fails
  outright if a VM named `verbatim` already exists; run `delete` first for
  a clean rebuild.
- `start` powers the VM on and waits (up to five minutes, polling every two
  seconds) for the in-guest agent to accept a TCP connection on its port.
- `stop` powers the VM off (`Stop-VM -Force`).
- `restart` restarts the VM and waits for the agent the same way `start`
  does.
- `restore [checkpoint]` restores a named checkpoint (default `golden`) and
  waits for the agent: the fast way back to a known-clean state, instead
  of a full `create`.
- `deploy` builds `verbatim-app`, `verbatim-agent`, `verbatim-outpost`,
  `verbatim-listener`,
  and `verbatim-synth-host` (debug profile, matching the CI job), then
  compares a SHA-256 hash of each artifact it would place in the guest
  against the guest's existing copy, fetching all the guest-side hashes in
  a single PowerShell Direct call. The artifacts are:
  - `verbatim.exe`, `verbatim-outpost.exe`, `verbatim-listener.exe`, and
    `verbatim-synth-host.exe`
    in `C:\VerbatimLab\verbatim`;
  - eSpeak NG's `espeak-ng-data` directory, which the build puts next to
    the executables, archived with Windows' own `tar` into one
    `espeak-ng-data.tar`, copied beside them and unpacked there when it
    changed;
  - the default theme's `sounds` directory, shipped the same way as
    `sounds.tar`;
  - the terminal scenarios' pinned portable Windows Terminal, downloaded
    and checked on the host into
    `target/xtask-vm-staging/windows-terminal-<version>` and shipped the
    same way as `windows-terminal.tar`, unpacked beside the executables
    where the scenarios start it ([the end-to-end guide](e2e.md));
  - a staged `settings.toml` beside the executables selecting eSpeak NG,
    the default synthesizer;
  - `verbatim-agent.exe` in `C:\VerbatimLab\agent`;
  - the vendored `ffmpeg.exe` in `C:\VerbatimLab\tools`, which each
    scenario's recording runs.

  Only artifacts whose hash differs are copied; each is reported as either
  "unchanged; skipping" or "changed; will copy". The guest's
  `VerbatimAgent` scheduled task and any running Verbatim are stopped
  first, but only when at least one executable or eSpeak NG data file
  (which a running synthesizer host may hold open; never `settings.toml`
  alone) needs copying, and the task is restarted afterward only if it was
  stopped or `verbatim-agent.exe` itself was copied. When every hash
  already matches, the guest is left untouched: no stop, no copy, no
  restart, the common case in an edit-test loop. Use this on its own to
  push a fresh build into a running guest without touching checkpoints.
- `test` builds the current source first, before touching the VM, so a
  compile failure changes no VM state. It then starts the guest if it is
  not running and waits for its agent, stages and copies that build onto
  it (always selecting eSpeak NG, the same synthesizer every local run
  uses), discovers the guest's IP address, and runs `session_info`'s own
  test once as a precondition (not a scenario, not recorded, and not
  affected by `--scenario` or `--group`; a failure here aborts the whole
  run, since nothing downstream can work from a non-interactive agent
  session). Then it runs the selected scenarios from
  `crates/verbatim-e2e/src/registry.rs`, one at a time, each as its own
  `cargo test -p verbatim-e2e --test e2e <name> -- --exact` subprocess on
  the host with `VERBATIM_E2E_ENDPOINT` pointed at the guest's agent,
  `VERBATIM_E2E_VERBATIM_EXE` at the guest-side path,
  `VERBATIM_E2E_REMOTE=1`, `VERBATIM_E2E_AUDIBLE=1`, and
  `VERBATIM_E2E_FFMPEG` naming the guest's `C:\VerbatimLab\tools\ffmpeg.exe`
  ([the end-to-end guide](e2e.md) lists every variable). Every scenario
  is recorded, as every end-to-end run is: its video, with Verbatim's own
  audio, lands in `target/e2e-artifacts/<scenario>/<scenario>.mp4` on the
  host, pass or fail, and `VERBATIM_E2E_RECORD=0` turns that off.
  Recording does not depend on any audio device in the guest, so a run
  can be watched and heard live over `cargo xtask vm connect` while it is
  recorded ("Hearing and recording a run" in the end-to-end guide).
  - `--scenario <name>` (repeatable) and `--group <name>` (repeatable, one
    of `speech`, `shell`, `navigation`, `text`) select which scenarios
    run; with neither given, every registered scenario runs except the
    demonstrations (the `demo` group), which are recorded on a
    development machine with `cargo xtask demo` and refused here, by name
    or by group. An unrecognized name is reported and the run aborts before
    anything touches the VM.
  - `--list` prints the scenario registry (name and group, one per line)
    and exits at once: no build, no restore, no deploy.
  - `--restore` restores the `golden` checkpoint first (then starts the
    guest, renews its DHCP lease, and waits for the agent) before
    deploying. Restoring is never automatic: deploy only copies files,
    skipping unchanged ones, and nothing is installed into the guest, so a
    restore buys nothing on an ordinary run while it and its agent wait
    are most of a run's wall-clock cost. An acceptance run passes
    `--restore`, since only a run that restored `golden` first
    demonstrates the harness from a clean state.
  - The flags combine in any order; `--list` ignores the others.
  - At the end of a run selecting more than one scenario, `test` prints a
    run summary: one line per scenario, pass or fail, and how many of
    that scenario's utterances with an event time reached audio, of how
    many, read back from the `verbatim_e2e::artifacts::ScenarioSummary`
    each scenario's subprocess wrote, not scraped from its output. There
    is no retry anywhere in this path: a scenario that fails is reported
    failed, once, and the run moves on to the next.
- `logs [dir]` pulls flight-recorder dumps (from the guest's
  `C:\VerbatimLab\verbatim\dumps`), the agent's own log
  (`C:\VerbatimLab\agent\agent.log`), and a launched Verbatim's captured
  stdout and stderr (`C:\VerbatimLab\verbatim\stderr-e2e.log`, below) out
  over PowerShell Direct, into `artifacts/vm-logs` by default. A missing
  dumps folder, agent log, or stderr log is logged and skipped, not a
  failure. Every guest file this verb reads is opened with read and write
  sharing on the guest side, so a log the agent or Verbatim still has open
  is still readable.
- `connect` starts the VM if needed, enables Remote Desktop in the guest
  the first time only, stores the guest's test credentials in this host's
  Windows Credential Manager keyed to the guest's address, then opens
  `mstsc.exe` against the guest with audio redirected to this computer and
  no sign-in prompt ("Hearing and recording a run" in
  [the end-to-end guide](e2e.md) has the recipe and why this exists
  instead of `vmconnect.exe`). `--forget` removes the stored credentials
  (`cmdkey /delete:TERMSRV/<guest-ip>`) and exits without connecting.
- `delete` stops the VM, removes every checkpoint, removes the VM
  registration, and deletes its virtual hard disks, clearing the way for a
  clean `create`.

Every scenario `crates/verbatim-e2e` launches asks the agent to capture the
launched Verbatim's combined stdout and stderr into a file, truncated fresh
on each launch: `C:\VerbatimLab\verbatim\stderr-e2e.log` in a VM run (see
`Scenario::launch` and the `stderr_to` field of
`verbatim_agent::protocol::Request::LaunchProcess`), or `stderr-e2e.log`
next to `verbatim.exe` in a local run. A panic's message never reaches the
flight recorder, whose dumps are written by Verbatim's graceful teardown,
which a panic does not reach; the tail of this file, pulled with
`cargo xtask vm logs`, is normally the panic message and backtrace.

Every verb that reaches the guest reads `VERBATIM_VM_USERNAME` and
`VERBATIM_VM_PASSWORD` from a repository-root `.env` file (gitignored,
never committed): the guest's local administrator credentials, the same
ones autologon and the `VerbatimAgent` scheduled task use.

## Rebuilding the golden image from scratch

Host prerequisites:

- A Windows host with Hyper-V enabled, and a Hyper-V virtual switch
  (usually the built-in `Default Switch`).
- Packer 1.15 or newer, with the HashiCorp Hyper-V plugin installed via
  `packer init vm` from the repository root.
- `oscdimg.exe` from the Windows ADK Deployment Tools: Packer needs an ISO
  creation tool for the answer-file CD it attaches during setup. The build
  wrapper searches `PATH` and the standard ADK install locations; pass
  `-OscdimgPath` explicitly if it lives somewhere else.
- A local Windows 11 business-editions x64 ISO, and its SHA-256 checksum.
- A repository-root `.env` with `VERBATIM_VM_USERNAME` and
  `VERBATIM_VM_PASSWORD` (see the previous section).

Steps:

1. Copy `vm/example.pkrvars.hcl` to a file named local.pkrvars.hcl in the
   vm folder (gitignored, never committed) and edit it: the local ISO path
   and its SHA-256, the Hyper-V switch name, a `temp_path` on a volume with
   room for the temporary build VHDX, and a local administrator password
   matching `.env`. Neither `temp_path` nor the output directory may be
   NTFS-compressed, because Hyper-V cannot import compressed VHDX files
   and a child directory can inherit compression from its parent; the
   build wrapper clears compression on both before every build.
2. Validate the template: `packer init vm`, then `packer fmt -check vm`
   and `packer validate -var-file vm/local.pkrvars.hcl vm`.
3. Run `cargo xtask vm create` from the repository root. Internally this
   runs `vm/scripts/Build-VerbatimWindows11Image.ps1 -Force`, which creates
   the temp and output directories, clears NTFS compression, runs
   `packer validate`, then `packer build`. During the build Packer boots
   the ISO unattended from `vm/answer-files/Autounattend.xml.pkrtpl.hcl`
   and runs two PowerShell provisioners in order:
   `vm/scripts/Initialize-VerbatimBaseImage.ps1` (WinRM automation
   prerequisites Packer itself needs, plus a `C:\VerbatimLab\image.json`
   metadata stamp), then `vm/scripts/Initialize-VerbatimHarness.ps1` (the
   harness proper: persistent autologon, every unattended-session setting
   listed in that script's header comment, a pinned 1920x1080 display
   resolution so control positions stay the same across runs, the
   `VerbatimAgent` scheduled task (restart-on-failure, so an agent killed
   by an RDP disconnect tearing down its session comes back on its own),
   and its inbound firewall rule). No audio driver is installed: Verbatim
   records its own audio for the videos, and with no audio device it plays
   silently in real time, while a connected `cargo xtask vm connect`
   session gives it that session's Remote Audio to speak through. ffmpeg
   is not installed by the image build; `cargo xtask vm deploy` stages it
   from `vm/vendor/ffmpeg` (see `vm/vendor/ffmpeg/README.md`). The wrapper
   then clears compression on the exported files and runs `Compare-VM` to
   catch Hyper-V import incompatibilities early. `cargo xtask vm create`
   then locates the exported `.vmcx` (under
   `artifacts/packer/windows11/Virtual Machines` by default, or the
   `output_directory` the local variables file sets), imports it, renames
   it to `verbatim`, enables the Guest Service Interface, starts it,
   deploys a debug build, waits for the agent, and checkpoints `golden`.
4. From there, `cargo xtask vm test` is the everyday loop; run
   `Build-VerbatimWindows11Image.ps1` alone (with `-SkipBuild` for a
   preflight-only dry run, or without it for a real rebuild) only when the
   base image itself needs to change.

The generated image lives under `artifacts/packer/windows11` by default
(gitignored) and must never be committed; neither may the local variables
file or `.env`.

## Checkpoint layers

The harness has one Hyper-V checkpoint, `golden`: base Windows 11 plus the
harness (autologon, the agent, the display and session settings). Nothing
in the mechanism limits it to one layer. Both Packer provisioners are
idempotent and log one fact per line, so a re-run against an
already-provisioned machine is safe; `restore` takes a checkpoint name,
and `checkpoint_vm` on the `Host` trait takes any name. An install script
for a target application the end-to-end corpus needs can be written the
same way, run once against a running `verbatim` VM, and checkpointed under
its own name on top of `golden`, so a scenario restores the layer it needs
instead of provisioning a bare machine. No layer beyond `golden` exists.

## The code

`xtask/src/vm/` is a binary's internal module tree; everything is
`pub(crate)`.

- `host`: the `Host` trait, every hypervisor operation a verb needs
  (`vm_exists`, `import_vm`, `rename_vm`, `ensure_guest_file_transfer`,
  `start_vm`, `stop_vm`, `restart_vm`, `checkpoint_vm`,
  `restore_checkpoint`, `delete_vm`, `guest_ip`, `copy_file_to_guest`,
  `run_in_guest`, `read_guest_file`, `list_guest_dir`). `HyperVHost` is
  the only implementation, over Hyper-V's PowerShell module; every method
  runs `powershell.exe -File` against a temporary script (never
  `-Command`, so arguments cross one layer of parsing) and, where a value
  must come back, wraps it in unique text markers, since some Hyper-V
  cmdlets write incidental text to the success stream. `wait_for_agent`
  is a free function over other `Host` calls (poll `guest_ip`, then probe
  the agent's port over TCP), so any host gets it.
- `create`, `deploy`, `test`, `lifecycle` (`start`, `stop`, `restart`,
  `restore`, `delete`), `logs`, `connect`: one module per verb or verb
  family, each orchestrating `Host` calls; `mod.rs` dispatches to them.
  `deploy::stage_and_copy` stages `settings.toml` from
  `Settings::for_e2e("espeak")`, the synthesizer `verbatim_e2e::scenario`
  selects for local runs, and checks the vendored `ffmpeg.exe` is the real
  binary and not a Git LFS pointer before copying it. When an executable
  must be copied, `deploy` stops the guest's `VerbatimAgent` task first and
  restarts it afterwards even if a copy fails (`copy_then_restart`), so a
  failed deploy never leaves the agent down.
- `test` reads `verbatim_e2e::registry::SCENARIOS` and `registry::select`
  rather than duplicating the catalog. Each scenario runs in a fresh,
  exactly filtered subprocess (`run_scenario_subprocess`), which gives
  every scenario a process boundary with no new IPC; the module's doc
  comment says why this was chosen over a multi-scenario runner inside
  `verbatim-e2e`. The scenario's launch, setup, body, teardown and
  recording all happen inside that subprocess
  ([the verbatim-e2e guide](../crates/verbatim-e2e.md)). Before each
  subprocess, `run_one_scenario` clears that scenario's artifacts
  directory, so a subprocess that never reached the scenario runner
  leaves no summary and is reported as a failure, never as the previous
  run's result. `print_run_summary` prints the final report.
- `dotenv`: a minimal `.env` reader (`KEY=VALUE` lines, comments,
  quoting), deliberately not a dependency for a format this small.
- `packer_build`: wraps `Build-VerbatimWindows11Image.ps1` and locates the
  `.vmcx` it exports, reading `output_directory` out of the local
  variables file with a minimal line-oriented HCL reader.

Constants: `VM_NAME` is `verbatim`; `CHECKPOINT_NAME` is `golden`;
`AGENT_PORT` is 44001, duplicated from
`verbatim_agent::protocol::DEFAULT_PORT` rather than depending on that
crate for one constant; `VERBATIM_DIR` (`C:\VerbatimLab\verbatim`) and
`AGENT_DIR` (`C:\VerbatimLab\agent`) are the guest install paths, matching
`vm/scripts/Initialize-VerbatimHarness.ps1`; and `FFMPEG_GUEST_PATH`
(`C:\VerbatimLab\tools\ffmpeg.exe`) is where `deploy` puts ffmpeg and what
`test` names in `VERBATIM_E2E_FFMPEG`.
