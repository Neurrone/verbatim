# The VM harness

How to drive the Hyper-V end-to-end harness: every `cargo xtask vm`
verb, and rebuilding the golden image from scratch. Split out of
[the tooling guide](tooling.md), which covers verbatim-inspect,
mockapp, and running the E2E suite runner-direct; the troubleshooting
section there covers VM failures too.

## cargo xtask vm verbs

`cargo xtask vm <verb>` wraps Hyper-V PowerShell behind a `Host` trait
(`xtask/src/vm/host.rs`); every verb module talks to `&dyn Host` only, which
is what lets a future non-Hyper-V implementation (QEMU/KVM on Linux
runners, per `docs/architecture.md` section 14's deferred CI story) be
dropped in later without rewriting verb logic. Run `cargo xtask vm` with no
arguments for the full verb list printed from the source of truth.

- `create` builds the base image with Packer (see the next section — this
  is the slow one), imports the exported VM, renames it to `verbatim`,
  enables the Guest Service Interface (Hyper-V's file-copy integration
  service, off by default and required by `deploy`), starts it, deploys a
  debug build so the checkpoint below already has a running agent, waits
  for the agent to answer, then checkpoints the VM as `golden`. Fails
  outright if a VM named `verbatim` already exists — run `delete` first for
  a clean rebuild.
- `start` powers the VM on and waits (up to five minutes, polling every two
  seconds) for the in-guest agent to accept a TCP connection on its port.
- `stop` powers the VM off (`Stop-VM -Force`).
- `restart` restarts the VM and waits for the agent the same way `start`
  does.
- `restore [checkpoint]` restores a named checkpoint (default `golden`) and
  waits for the agent — the fast way back to a known-clean state between
  runs, instead of a full `create`.
- `deploy` builds `verbatim-app`, `verbatim-agent`, and `verbatim-outpost`
  (debug profile, matching the CI job), then compares a SHA-256 hash of
  each of the four artifacts it would place in the guest (`verbatim.exe`
  and `verbatim-outpost.exe` in `C:\VerbatimLab\verbatim`, a staged
  capture-synth `settings.toml` alongside them, and `verbatim-agent.exe` in
  `C:\VerbatimLab\agent`) against the guest's existing copy, fetching all
  four guest-side hashes in a single PowerShell Direct call. Only artifacts
  whose hash differs are copied; each is reported as either "unchanged;
  skipping" or "changed; will copy". The guest's `VerbatimAgent` scheduled
  task and any running Verbatim are stopped first, but only when at least
  one executable (never `settings.toml` alone) actually needs copying, and
  the task is restarted afterward only if it was stopped or
  `verbatim-agent.exe` itself was among the copied artifacts. When every
  hash already matches, the guest is left completely untouched — no stop,
  no copy, no restart — which is the common case in a tight edit-test loop
  where nothing changed since the last deploy. Use this on its own when you
  want to push a fresh build into an already-running guest without touching
  checkpoints at all.
- `test` builds the current source first, before touching the VM at all
  (needs `LIBCLANG_PATH`, set automatically when found — see "Hearing and
  recording a run" above), then restores `golden`, stages and copies that
  build onto it (always with the real `OneCore` synthesizer — no more
  capture-synth choice on the VM path), discovers the guest's IP address,
  runs `session_info`'s own test once as a precondition (not itself a
  scenario, not recorded, and not affected by `--scenario`/`--group` — a
  failure here aborts the whole run, since nothing downstream can work from
  a non-interactive agent session), and then runs the selected scenarios
  from `crates/verbatim-e2e/src/registry.rs`, one at a time. Each scenario
  runs as its own `cargo test -p verbatim-e2e <name> -- --exact` subprocess
  on the host with `VERBATIM_E2E_ENDPOINT` pointed at the guest's agent,
  `VERBATIM_E2E_VERBATIM_EXE` pointed at the guest-side path,
  `VERBATIM_E2E_REMOTE=1`, and `VERBATIM_E2E_AUDIBLE=1` set — this is what
  gives `--record` a clean, one-scenario-at-a-time recording boundary (see
  "Hearing and recording a run" above) with no new machinery inside
  `verbatim-e2e` itself. Building first, ahead of the restore, is
  deliberate: a compile failure is then caught with zero VM state changes,
  rather than after a restore that then has to be paid for again on the
  next attempt. `--no-restore` does not change this — the build still runs
  first either way. This is the one-command loop `docs/roadmap.md`'s M2
  exit criteria describes.
  - `--scenario <name>` (repeatable) and `--group <name>` (repeatable, one
    of `speech`, `shell`, `legacy`, `navigation`) select which scenarios
    run; with neither given, every registered scenario runs, the same as
    before milestone M3 Track B's restructuring. An unrecognized name is
    reported and the run aborts before anything touches the VM.
  - `--list` prints the scenario registry (name and group, one per line)
    and exits immediately — no build, no restore, no deploy, nothing
    touches the VM at all.
  - `cargo xtask vm test --no-restore` skips the checkpoint restore (and
    its post-restore agent wait) entirely, deploying straight onto whatever
    the guest is currently running, and prints a prominent line stating the
    guest was not restored and its state may be dirty. Combined with
    `deploy`'s hash-skipping, this makes a rerun after a small code change
    fast — restore plus its agent wait is most of an ordinary run's
    wall-clock cost. Never use `--no-restore` for an acceptance run: only a
    run that actually restored `golden` first demonstrates the harness's
    real exit criteria.
  - `--record` additionally captures each scenario as its own video with
    audio into `artifacts/vm-recordings`, one file per scenario named after
    it; see "Hearing and recording a run" above for the key constraint that
    recording audio and a connected RDP session are mutually exclusive, and
    how `--record` degrades to a video-only, `-no-audio`-tagged recording
    with a warning rather than aborting when a session is connected anyway.
  - Every flag may be given together, in any order (aside from `--list`,
    which short-circuits before any of the others matter).
  - At the end of a run selecting more than one scenario, `cargo xtask vm
    test` prints a run summary: one line per scenario, pass or fail, and how
    many of that scenario's latency timelines reached audio — read back from
    the `verbatim_e2e::artifacts::ScenarioSummary` each scenario's own
    subprocess wrote (see the note on failure artifacts above), not scraped
    from subprocess output. There is no retry of any kind anywhere in this
    path: a scenario that fails is reported failed, once, and the run moves
    on to the next selected scenario.
- `logs [dir]` pulls flight-recorder dumps (from the guest's
  `C:\VerbatimLab\verbatim\dumps`), the agent's own log
  (`C:\VerbatimLab\agent\agent.log`), and a launched Verbatim's captured
  stdout and stderr (`C:\VerbatimLab\verbatim\stderr-e2e.log` — see the next
  paragraph) out over PowerShell Direct, into `artifacts/vm-logs` by
  default. A missing dumps folder, no agent log yet, or no stderr log yet
  is logged and skipped, not a failure — an ordinary state early in a VM's
  life. Every guest file this verb reads is opened with read/write sharing
  on the guest side, so a log the agent or Verbatim still has open for
  writing is still readable rather than failing with a sharing violation.

Every scenario `crates/verbatim-e2e` launches asks the agent to capture the
launched Verbatim's combined stdout and stderr into a file, truncated fresh
on each launch: `C:\VerbatimLab\verbatim\stderr-e2e.log` in a VM run (see
`Scenario::launch` and `verbatim_agent::protocol::Request::LaunchProcess`'s
`stderr_to` field), or a file named `stderr-e2e.log` next to `verbatim.exe`
in a runner-direct run. Verbatim intermittently crashes at launch inside the
guest with no other trace of why — the flight recorder proves a panic
happened but never captures its message, since flight-recorder dumps are
written by Verbatim's own graceful teardown path, which a panic does not
reach. This capture file is exactly the corner that closes: pull it with
`cargo xtask vm logs` and its tail is normally the Rust panic message and
backtrace that would otherwise be lost with the process.
- `connect` starts the VM if needed, enables Remote Desktop in the guest
  the first time only, stores the guest's test credentials in this host's
  own Windows Credential Manager keyed to the guest's address, then opens
  `mstsc.exe` against the guest with audio redirected to this computer and
  no sign-in prompt — see "Hearing and recording a run" above for the full
  recipe and why this exists instead of `vmconnect.exe`. `--forget` removes
  the stored credentials (`cmdkey /delete:TERMSRV/<guest-ip>`) and exits
  without connecting.
- `delete` stops the VM, removes every checkpoint, removes the VM
  registration, and deletes its virtual hard disks, clearing the way for a
  clean `create`.

Every verb that reaches the guest reads `VERBATIM_VM_USERNAME` and
`VERBATIM_VM_PASSWORD` from a repository-root `.env` file (gitignored,
never committed) — the guest's local administrator credentials, the same
ones autologon and the `VerbatimAgent` scheduled task use.

## Rebuilding the golden image from scratch

Host prerequisites:

- A Windows host with Hyper-V enabled, and a Hyper-V virtual switch
  (usually the built-in `Default Switch`).
- Packer 1.15 or newer, with the HashiCorp Hyper-V plugin installed via
  `packer init vm` from the repository root.
- `oscdimg.exe` from the Windows ADK Deployment Tools — Packer needs an ISO
  creation tool for the answer-file CD it attaches during setup. The build
  wrapper searches `PATH` and the standard ADK install locations
  automatically; pass `-OscdimgPath` explicitly if it lives somewhere else.
- A local Windows 11 business-editions x64 ISO, and its SHA-256 checksum.
- A repository-root `.env` with `VERBATIM_VM_USERNAME` and
  `VERBATIM_VM_PASSWORD` — see the previous section.

Steps:

1. Copy `vm/example.pkrvars.hcl` to `vm/local.pkrvars.hcl` (gitignored,
   never committed) and edit it: the local ISO path and its SHA-256, the
   Hyper-V switch name, a `temp_path` on a volume with room for the
   temporary build VHDX, and a local administrator password matching
   `.env`. Neither `temp_path` nor the output directory may be
   NTFS-compressed — Hyper-V cannot import compressed VHDX files, and a
   child directory can silently inherit compression from its parent, which
   is why the build wrapper clears compression explicitly on both before
   every build rather than trusting the developer remembered.
2. Validate the template: `packer init vm` then `packer fmt -check vm` and
   `packer validate -var-file vm/local.pkrvars.hcl vm`.
3. Run `cargo xtask vm create` from the repository root. Internally this
   runs `vm/scripts/Build-VerbatimWindows11Image.ps1 -Force`, which creates
   the temp and output directories, clears NTFS compression, runs
   `packer validate`, then `packer build`. During the build Packer boots
   the ISO unattended from `vm/answer-files/Autounattend.xml.pkrtpl.hcl`
   and runs two PowerShell provisioners in order:
   `vm/scripts/Initialize-VerbatimBaseImage.ps1` (WinRM automation
   prerequisites Packer itself needs, plus a `C:\VerbatimLab\image.json`
   metadata stamp), then `vm/scripts/Initialize-VerbatimHarness.ps1` (the
   M2 harness proper: persistent autologon, every unattended-session
   setting listed in that script's own header comment, a pinned
   1920x1080 display resolution so control positions stay deterministic
   across runs, the VB-CABLE virtual audio driver (the guest's WASAPI
   render endpoint every VM run now uses, audible by default, required
   rather than best-effort), the `VerbatimAgent` scheduled task
   (restart-on-failure, so an agent killed by an RDP disconnect tearing
   down its session comes back on its own), and its inbound firewall rule).
   ffmpeg and ffprobe are deliberately not installed by the image build;
   `cargo xtask vm deploy` stages them from `vm/vendor/ffmpeg` at deploy
   time (see the note under "Hearing and recording a run" above and
   `vm/vendor/ffmpeg/README.md`), which is why an in-guest ffmpeg download
   is not among the provisioner steps. The
   wrapper then clears compression on the exported files and runs
   `Compare-VM` to catch Hyper-V import incompatibilities before they
   become a mystery later. `cargo xtask vm create` then locates the
   exported `.vmcx` (under `artifacts/packer/windows11/Virtual Machines`
   by default, or `output_directory` from `local.pkrvars.hcl`), imports
   it, renames it to `verbatim`, enables the Guest Service Interface,
   starts it, deploys a debug build, waits for the agent, and checkpoints
   `golden`.
4. From there, `cargo xtask vm test` is the everyday loop; run
   `Build-VerbatimWindows11Image.ps1` alone (via `-SkipBuild` for a
   preflight-only dry run, or without it for a real rebuild) only when the
   base image itself needs to change.

The generated image lives under `artifacts/packer/windows11` by default
(gitignored) and must never be committed; neither may `vm/local.pkrvars.hcl`
or `.env`.

## The app-layers idea

M2 ships exactly one Hyper-V checkpoint, `golden`: base Windows 11 plus the
harness (autologon, the agent, the display and session settings). Nothing
in the mechanism is specific to that being the *only* layer, though. Both
Packer provisioners that build it (`Initialize-VerbatimBaseImage.ps1`,
`Initialize-VerbatimHarness.ps1`) are deliberately idempotent and log one
fact per line for exactly this reason — a safe re-run is also a safe *later*
run against an already-provisioned machine. `xtask vm`'s `restore` verb
already takes an optional checkpoint name rather than being hardwired to
`golden`, and `checkpoint_vm` on the `Host` trait takes an arbitrary name
too.

The idea this sets up for later milestones: install scripts for target
applications the E2E corpus needs (a specific Notepad++ build for the
porting track, a browser for M6, an Office install for the M8-era Tier D
work) can each be written the same idempotent, one-fact-per-line way, run
once against a running `verbatim` VM, and checkpointed under their own name
layered on top of `golden` (or on top of each other) — so a given E2E
scenario's `xtask vm test` run restores whichever layer it actually needs
instead of every scenario re-provisioning a bare machine from scratch.
Nothing beyond `golden` exists yet; this section documents the intended
shape so the day an app-specific layer is needed, it is an addition to the
existing pattern rather than a redesign.
