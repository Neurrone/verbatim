# Tooling guide

The practical companion to `docs/architecture.md` and `docs/roadmap.md`: how
to actually drive this project day to day, as of milestone M2. Where the
architecture doc explains why something exists and the roadmap says when it
landed, this doc says which command to type.

Like every doc in this repository, this one avoids ASCII diagrams, box
drawings, arrow chains, and pipe tables, so it reads well with a screen
reader in both rendered and source form.

## Driving a running Verbatim with verbatim-inspect

`verbatim-inspect` is a developer CLI over the control plane
(`docs/architecture.md` section 10). It is not a child of Core and is not in
any job object; it attaches through the named pipe (or, for a VM or CI
agent, a TCP tunnel) like any other control-plane client. Every subcommand
prints plain text, one fact per line.

Build and start a Verbatim to inspect first:

```
cargo build -p verbatim-app
target\debug\verbatim.exe
```

Then, in a second terminal, run `verbatim-inspect` subcommands against it.
Every subcommand accepts a global `--connect <ADDRESS>` option before the
subcommand name:

- Omit `--connect` entirely to reach the well-known local named pipe — the
  ordinary case when both `verbatim-inspect` and the Verbatim you are
  inspecting are on the same machine.
- `--connect tcp:HOST:PORT` connects over TCP instead — this is how the E2E
  suite and `cargo xtask vm test` reach a Verbatim tunneled through the
  in-guest agent, and it also works for a Verbatim you started manually
  with a TCP-reachable control plane.
- Anything else passed to `--connect` is treated as a literal named-pipe
  path, for tests that start a `ControlServer` on a non-default pipe name.

The subcommands:

- `status` prints the process id, version, active synthesizer, and one line
  per known outpost (its target pid, its own outpost pid once spawned, and
  its lifecycle state).
- `watch-events` subscribes to normalized accessibility events and prints
  one line per event: the trace id, the source, the backend (UIA or MSAA),
  and a summary (a focus change's role and name, a property change's new
  value, and so on).
- `watch-speech` subscribes to captured speech and prints one line per
  utterance. A line with no audio-start time was printed at queue time; a
  follow-up line for the same trace, printed once audio actually starts,
  carries the true event-to-audio latency. An interrupted utterance simply
  never gets that follow-up line.
- `watch` subscribes to both on one connection and interleaves them in
  arrival order, each line prefixed `event` or `speech`. Because the lines
  interleave in the order Verbatim actually produced them, an event line
  reads directly above the speech it caused — the fastest way to eyeball
  whether a gesture produced the announcement you expected.
- `send-gesture <identifier>` routes a gesture identifier through the same
  gesture router real key presses use, for example `verbatim-inspect
  send-gesture kb:verbatim+v` to open the Verbatim menu without touching a
  keyboard.
- `send-keys <combo...>` synthesizes real OS keyboard input via `SendInput`,
  reaching whatever currently has focus — for example `verbatim-inspect
  send-keys downarrow enter "shift+tab"`. Because this is real synthetic
  input, not a gesture-router shortcut, it needs an unlocked, focused
  interactive desktop to land anywhere (see Troubleshooting below).
- `latency --last N` prints the N most recent end-to-end latency timelines
  (default 10), newest first: event-observed time, and the millisecond
  deltas to speech-queued and audio-started when each is known.
- `dump-tree` asks the outpost for the foreground application's
  accessibility tree, walked from its top-level window, and prints one node
  per line indented two spaces per depth level, reusing the same
  role-and-name summary the event stream uses. A trailing line notes when
  the outpost's depth or node-count cap cut the walk short.
- `dump-recorder` asks Core to write its flight recorder's current contents
  to disk (next to `verbatim.exe`, under a `dumps` folder, named
  `flight-<UTC timestamp>.jsonl`) and prints the path it wrote to. Useful
  for capturing a live session as a `verbatim-core` replay fixture without
  waiting for a crash.
- `quit` asks Verbatim to exit cleanly through the control plane.

Reading `watch`'s output: each `event` line names the trace id that a
following `speech` line will usually share, so scanning down the interleaved
stream shows cause and effect directly — an `event` line for a focus change
immediately followed by a `speech` line announcing that same control's name,
role, and state is what a working focus announcement looks like. A `speech`
line with no matching `event` line above it is Core-originated speech (for
instance the startup announcement), which has no triggering accessibility
event.

## Test-audio mode

`VERBATIM_TEST_AUDIO=null`, set in the environment before starting
`verbatim.exe`, is a test-only escape hatch `verbatim-app`'s `run` checks at
startup. It does two things together: registers the capture synthesizer
(`verbatim-synth-capture`, id `capture`) alongside OneCore, and swaps in
`verbatim_audio::NullSink` for the real `WasapiSink`.

This exists because most of the tooling in this document needs to run with
no sound card and no installed OneCore voices — a bare CI runner, a fresh VM
image, or just a dev machine where you don't want Verbatim actually talking
while you script a test. The capture synth records every `SpeechRequest`
with a timestamp into an in-memory log instead of producing audio, and
exposes just a voice choice and a rate numeric (no toggle — see the
Troubleshooting-adjacent note in the E2E section below for why that matters
to one specific regression test). `NullSink` accepts any PCM format and
discards every sample, but still emits the `audio_started` tracing event on
each utterance's first (discarded) write, at exactly the point `WasapiSink`
would have emitted it on real hardware — so `LatencyLedger` still records a
complete event-to-audio timeline with nothing actually playing, and
`--last N`/`report_latency` assertions still have something to check.

Every scenario `crates/verbatim-e2e` launches sets this variable, and
`cargo xtask vm deploy` stages a `settings.toml` selecting the capture synth
for exactly the same reason (no installed voices required, cross-process
setting-descriptor coverage is still exercised).

## Running mockapp by hand

`mockapp` is a real, separate-process Win32 application that answers
`WM_GETOBJECT` as a genuine out-of-process UIA or MSAA provider over a
JSON-scripted tree — see `docs/overview.md`'s `mockapp` section for the
full internal design. This section is about running it standalone, outside
its own integration tests, to poke at it directly.

Build it and run it against a fixture:

```
cargo build -p mockapp
target\debug\mockapp.exe --fixture crates\mockapp\tests\fixtures\tree.json --backend uia
```

`--backend` is `uia` or `msaa`; `--title` overrides the window title
(default `mockapp`) when you need more than one instance running at once,
for example one of each backend side by side. The process creates one real
top-level window, prints `ready` once the window exists and the provider is
answering, then reads commands from stdin until `quit`.

Fixture format: a JSON file holding one object per node. Each node has an
`id` (a unique string), a `role` (a `verbatim_model::Role` name in snake
case, such as `check_box` or `list_item`), optional `name` and `value`
strings, an optional `states` array (`State` names in snake case, such as
`read_only` or `mixed`), and an optional `children` array of nested node
objects. The root object corresponds to the window itself.
`crates/mockapp/tests/fixtures/tree.json` is a full worked example — a
window containing a button group, a list, a combo box, an editable text
field, a slider, a spin button, a tab control, a link, a toolbar, a status
bar, and a menu — and doubles as the reference for every role and state
name the fixture format accepts.

Stdin commands, one per line, once `ready` has printed:

- `focus <id>` raises the backend's real focus-changed notification for
  that node (`UiaRaiseAutomationEvent` under UIA,
  `NotifyWinEvent(EVENT_OBJECT_FOCUS, ...)` under MSAA).
- `set-name <id> <text>` and `set-value <id> <text>` update the scripted
  tree in place and raise the matching name-changed, value-changed, or
  property-changed notification.
- `quit` exits the process.

A typical by-hand session: start `mockapp` with a fixture, point a real
accessibility client (`verbatim-inspect dump-tree` against a Verbatim whose
outpost is targeting the mockapp window, or any other UIA/MSAA inspection
tool) at the window, then type `focus chk_enable` or `set-value slider1 75`
at the `mockapp` process's own stdin and watch the notification arrive on
the client side. This is exactly what `crates/mockapp/tests/events.rs`
automates, minus the automation.

## Running the E2E suite runner-direct

"Runner-direct" means the agent and the tests it drives share one machine —
your dev box, or a plain GitHub-hosted Windows runner — as opposed to
`cargo xtask vm test`, which drives a Hyper-V guest. Every live test in
`crates/verbatim-e2e` checks the `VERBATIM_E2E_ENDPOINT` environment
variable first and prints a one-line skip notice instead of running when it
is unset, so `cargo test` and `cargo xtask ci` stay green with no agent
anywhere.

To actually run the suite locally:

```
cargo build -p verbatim-app -p verbatim-agent
target\debug\verbatim-agent.exe --bind-address 127.0.0.1 --port 44001
```

Leave that agent running in its own terminal (loopback avoids a firewall
prompt that binding all interfaces would trigger on a dev machine), then in
a second terminal:

```
set VERBATIM_E2E_ENDPOINT=127.0.0.1:44001
cargo test -p verbatim-e2e -- --test-threads=1
```

`--test-threads=1` is not optional: every live test in this crate launches
a real `verbatim.exe` on the real desktop and injects real keystrokes, and
`crates/verbatim-e2e/src/scenario.rs` enforces only one such instance at a
time within the test process via a shared lock, but Windows itself has no
notion of "only one verbatim.exe" — two scenarios running concurrently
would each think they own an instance the other just replaced out from
under it (`single_instance::acquire_replacing`'s own algorithm). This is
exactly what `.github/workflows/ci.yml`'s `e2e` job does, on a plain
`windows-latest` runner, with the same two environment variables.

Two more environment variables matter for less common cases:

- `VERBATIM_E2E_VERBATIM_EXE` overrides the `verbatim.exe` path a scenario
  launches. It defaults to `target/debug/verbatim.exe` under the workspace
  root (computed from the crate's own manifest directory, so it does not
  depend on your working directory).
- `VERBATIM_E2E_REMOTE=1` marks a remote run, where the agent, Verbatim, and
  its configuration live on another machine (the Hyper-V guest) — set by
  `cargo xtask vm test`, not something you normally set by hand. It skips
  the two host-filesystem steps that only make sense when the suite and
  Verbatim share a filesystem: checking `verbatim.exe` exists, and writing
  the capture-synth `settings.toml` next to it.

The suite currently has three tests: `session_info` (the agent reports an
interactive session — the precondition everything else depends on),
`notepad_focus` (launching Notepad reaches Verbatim and Verbatim survives
Notepad exiting), and `m1_exit_regression` (the scripted walk of the M1 exit
criteria — see `docs/roadmap.md`'s M2 section for exactly what it asserts
and does not assert).

Reading a speech-assertion failure: `SpeechCollector::expect_in_order`
panics with a message naming which matcher, by position, it was waiting for
(for example "waiting for utterance 3 of 5") along with the literal
substring it expected, followed by a transcript of every utterance actually
heard on that connection so far, one per line, oldest first, prefixed with
its index. Comparing the named matcher against the transcript's tail
usually shows immediately whether Verbatim said something *close* (wrong
wording, a race with unrelated desktop activity) or said nothing relevant
at all (a real regression, or focus never reached the expected control). A
timeout and an outright connection failure produce distinguishable
messages — the second appends "underlying error: ..." — so a suite that
hangs and then fails is not automatically the same bug as one that dies
outright.

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
  (debug profile, matching the CI job), then copies `verbatim.exe` and
  `verbatim-outpost.exe` into `C:\VerbatimLab\verbatim`, a staged
  capture-synth `settings.toml` alongside them, and
  `verbatim-agent.exe` into `C:\VerbatimLab\agent`, then restarts the
  `VerbatimAgent` scheduled task so the freshly deployed agent is the one
  actually running. Use this on its own when you want to push a fresh
  build into an already-running guest without touching checkpoints at all.
- `test` restores `golden`, deploys the current build on top of it,
  discovers the guest's IP address, then runs `crates/verbatim-e2e`'s suite
  on the host with `VERBATIM_E2E_ENDPOINT` pointed at the guest's agent,
  `VERBATIM_E2E_VERBATIM_EXE` pointed at the guest-side path, and
  `VERBATIM_E2E_REMOTE=1` set. This is the one-command loop
  `docs/roadmap.md`'s M2 exit criteria describes.
- `logs [dir]` pulls flight-recorder dumps (from the guest's
  `C:\VerbatimLab\verbatim\dumps`) and the agent's own log
  (`C:\VerbatimLab\agent\agent.log`) out over PowerShell Direct, into
  `artifacts/vm-logs` by default. A missing dumps folder or no agent log
  yet is logged and skipped, not a failure — an ordinary state early in a
  VM's life.
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
   across runs, a best-effort Scream virtual audio driver install, the
   `VerbatimAgent` scheduled task, and its inbound firewall rule). The
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

## Troubleshooting

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
the same check live — when its own window station is not interactive. If
`session_info` (the E2E test, or `AgentClient::session_info` by hand) ever
reports `interactive_window_station: false`, something restarted the agent
outside that scheduled task; check the task's last run result in the guest
before debugging anything downstream.

**A port band around 47xxx silently refuses binds on some machines.** On at
least one real development machine, every port tried in a band around
47600 (47600, 47601, 47650, 47712, 47800, 47900) failed to bind with
"address already in use", while nothing owned any of them according to
`Get-NetTCPConnection`, and none appeared in
`netsh interface ipv4/ipv6 show excludedportrange` — an invisible
reservation, almost certainly security software, that no standard tool
could see or explain. This is exactly why `verbatim_agent::protocol
::DEFAULT_PORT` is 44001 and not something in that band. If 44001 itself
turns out to be blocked on a given machine, override it everywhere
consistently: `verbatim-agent.exe --port <N>`, and the matching endpoint
value wherever the suite or `xtask vm` expects one (`xtask/src/vm/mod.rs`'s
`AGENT_PORT` constant, for the VM path).

**A locked desktop makes `SendInput` inject nothing.** `send-keys`,
`Scenario::send_keys`, and anything else that goes through real synthetic
input needs an unlocked, interactive desktop with a foreground window that
can actually receive it — a locked screen or an active screensaver silently
swallows the injection with no error anywhere in the chain (the control
plane still replies `Ok`, since sending the input is fire-and-forget from
its point of view). `Initialize-VerbatimHarness.ps1` disables the
screensaver and workstation locking by machine-wide policy for exactly this
reason, but a manual Win+L during a debugging session, or a plain (not
Enhanced Session) RDP disconnect that locks the workstation behind it,
reintroduces the problem. If gestures and status queries succeed but
nothing is ever heard or focus never seems to move, check the VM's console
is actually sitting at an unlocked desktop before looking anywhere else.

**Stray processes survive a failed run.** `Scenario`'s `Drop` impl always
tries a clean `Quit` through the control plane, then unconditionally kills
`verbatim.exe` (and anything launched via `launch_target`, such as
Notepad) through the agent — this runs even if the test panicked partway
through. It cannot run at all, though, if the test process itself is
killed outright (Ctrl+C, a CI job cancellation, or the whole `cargo test`
process being terminated). Runner-direct stray processes are usually
self-healing on the *next* run regardless:
`single_instance::acquire_replacing` means a fresh `verbatim.exe` replaces
whatever old instance it finds. A stray guest-side process is not
self-healing the same way and is simplest to clear by restoring a
checkpoint (`cargo xtask vm restore`), which reverts the whole VM's process
state along with everything else.

**A gesture sent immediately after launch can silently do nothing.** The
control server starts (and so a tunnel connection succeeds) before
`verbatim-app`'s GUI thread finishes wiring its gesture handle; a
`SendGesture` that arrives in that narrow window is logged and dropped by
the router, not queued, even though the control plane still answers `Ok`.
`Scenario::launch` covers this with a fixed one-second settle delay after
its own connection succeeds; a hand-rolled script driving a freshly
launched Verbatim through `verbatim-inspect` should pause briefly after
launch for the same reason, especially on a slow CI runner.
