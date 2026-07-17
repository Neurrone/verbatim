# Tooling guide

The practical companion to `docs/architecture.md` and `docs/roadmap.md`: how
to actually drive this project day to day, as of milestone M2 plus the M3
Track B restructuring of the end-to-end suite into a scenario registry.
Where the architecture doc explains why something exists and the roadmap
says when it landed, this doc says which command to type.

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

Every scenario `crates/verbatim-e2e` launches sets this variable in
runner-direct mode, which defaults to the capture synth for exactly the
same reason (no installed voices required, cross-process setting-descriptor
coverage is still exercised). `cargo xtask vm deploy` (and `vm test`, and
`vm create`'s own bake-in of the golden checkpoint) is different: it always
stages a `settings.toml` selecting the real `OneCore` synthesizer instead,
since the VM's golden image always has `OneCore` voices installed and a
real VB-CABLE audio device — see "Hearing and recording a run" below for
what that means for a VM run's audio.

## Running mockapp by hand

`mockapp` is a real, separate-process Win32 application that answers
`WM_GETOBJECT` as a genuine out-of-process UIA or MSAA provider over a
JSON-scripted tree — see [the mockapp crate guide](crates/mockapp.md) for the
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

The suite always runs against a fixed, generated configuration, never
whatever `settings.toml` a developer's own manual runs left behind.
Concretely, in this runner-direct mode, `Scenario::launch` copies
`verbatim.exe` and `verbatim-outpost.exe` into `target/e2e-stage` under the
workspace root (skipping a copy when the destination already matches
byte-for-byte) and writes a fresh `settings.toml` there — `Settings::default`
plus exactly the synthesizer choice — before launching that staged copy.
Your own `target/debug/verbatim.exe` and its `settings.toml` are never read
or mutated by a test run. Against the VM, `cargo xtask vm deploy` stages the
guest side the same way, independently (the two crates cannot share code, so
they are kept in lockstep by hand — see `verbatim_config::Settings::for_e2e`'s
doc comment, the shared constructor both staging steps build from).

Two more environment variables matter for less common cases:

- `VERBATIM_E2E_VERBATIM_EXE` overrides the directory a scenario stages its
  binaries *from* — the source build, not where it actually launches from.
  It defaults to `target/debug/verbatim.exe` under the workspace root
  (computed from the crate's own manifest directory, so it does not depend
  on your working directory). Setting it chooses a different source build;
  the fixed, staged configuration regime is unaffected either way.
- `VERBATIM_E2E_REMOTE=1` marks a remote run, where the agent, Verbatim, and
  its configuration live on another machine (the Hyper-V guest) — set by
  `cargo xtask vm test`, not something you normally set by hand. It skips
  the runner-direct staging step entirely (checking the source binaries
  exist, copying them, and writing `settings.toml`), since `cargo xtask vm
  deploy` already staged the guest-side equivalent.
- `VERBATIM_E2E_AUDIBLE=1` requests an audible run — see "Hearing and
  recording a run" below. Set by hand for a runner-direct audible run;
  `cargo xtask vm test` sets it automatically now, always, since a VM run
  is audible by default.
- `VERBATIM_E2E_PACED=1` makes every speech assertion wait for the matched
  utterance's audio to finish (the control plane's per-utterance
  `SpeechFinished` frame) before the next keystroke, so each utterance is
  heard in full rather than cut off — for watching or recording a run, not
  for fast CI. It changes only timing, never what is asserted. `cargo xtask
  vm test --paced` sets it, and `--record` implies it; set it by hand for a
  paced runner-direct run (only meaningful alongside `VERBATIM_E2E_AUDIBLE`,
  since the capture synth produces no audio to wait on).

The suite is a scenario registry (`crates/verbatim-e2e/src/registry.rs`,
milestone M3 Track B): every scenario is a named, grouped setup/body/teardown
definition, and `crates/verbatim-e2e/tests/` holds one thin `#[test]`
wrapper per scenario calling `registry::run_named("that scenario's name")`,
plus `session_info` (the agent reports an interactive session — the
precondition everything else depends on, not itself a scenario). The
scenarios today: `notepad_focus` (launching Notepad reaches Verbatim and
Verbatim survives Notepad exiting), `multi_outpost_switch` (switching
foreground between Notepad and Verbatim's own menu keeps both outposts
alive and re-announces correctly), `m1_exit_regression` (the scripted
walk of the M1 exit criteria — see `docs/roadmap.md`'s M2 section for
exactly what it asserts and does not assert), `object_navigation` (the M3
object-navigation and review commands against Verbatim's own settings
dialog), `msinfo32` (an MSAA-only legacy application reaches Verbatim
through the MSAA stack), and `tree_navigation` (logical object navigation
through msinfo32's real Win32 tree view — the regression scenario for the
flat MSAA tree-view exposure), and `start_menu` (pressing the Windows key
opens the Start/Search surface and Verbatim announces its search box).
A real Explorer folder-window scenario is deliberately not among them — see
the "Explorer" note in `docs/roadmap.md`'s M3 section for why it is verified
by hand for now, and the same section's toggle-controls and Start-menu notes
for the other by-hand cases (the Settings app's toggles, and navigating the
Start menu's search *results*, both deferred for the same reason). A
scenario's name is also its
`#[test]` function name, so `cargo test -p verbatim-e2e <name> -- --exact
--test-threads=1` runs exactly that one scenario runner-direct, the same
selection mechanism `cargo xtask vm test --scenario <name>` uses against the
VM (see "cargo xtask vm verbs" below).

Every scenario run, pass or fail, writes a one-line-per-fact summary (name,
pass or fail, latency counts) to `target/e2e-artifacts/<scenario name>/
summary.txt` under the workspace root (`VERBATIM_E2E_ARTIFACTS_DIR`
overrides the root), whether run runner-direct or through `cargo xtask vm
test`; `xtask vm test` reads this back to build its own run summary rather
than parsing test output. Every scenario run, pass or fail, also writes into
the same directory the interleaved timeline
(`timeline.txt` — the same account an `expect_*` panic already prints),
Verbatim's captured stderr log (`stderr.log`), the per-process outpost and
listener logs the supervisor redirected each spawned process's stderr into
(`outpost-core.log`, `listener.log`, and one `outpost-<pid>.log` per launched
target — fetched by name from the guest's `logs` directory, since the agent
reads single files and cannot list a directory), and a reducer flight-recorder
dump (`flight-recorder.jsonl`, fetched via the control plane's `DumpRecorder`
request and read back through the agent) — the timeline, stderr, and outpost
logs by `Scenario::collect_run_artifacts` and the flight recorder by
`Scenario::collect_flight_recorder` (taken before the clean quit, so a passing
run captures it too), both from inside the scenario's own process, where the
live control and agent connections they need still exist. A passing run leaves
these behind so its announcement timings and reducer inputs can be read, not
only a failing one. None of this is a retry mechanism: a failed scenario is
reported failed exactly once, with these artifacts left for root-causing,
never re-run automatically by anything in this crate or by `xtask`.

Reading a speech-assertion failure: `SpeechCollector::expect_in_order`
panics with a message naming which matcher, by position, it was waiting for
(for example "waiting for utterance 3 of 5") along with the literal
substring it expected, followed by the run's timeline so far — every
gesture and key the scenario injected and every utterance Verbatim spoke,
interleaved in time order, one entry per line, each prefixed with the
milliseconds elapsed since the first entry. Because the commands the test
sent appear alongside the speech they did or did not provoke, the last line
before speech stopped is usually the whole diagnosis: you can see the exact
gesture or keystroke that went out, and whether Verbatim said something
*close* (wrong wording, a race with unrelated desktop activity) or said
nothing relevant at all (a real regression, or focus never reached the
expected control). A timeout and an outright connection failure produce
distinguishable
messages — the second appends "underlying error: ..." — so a suite that
hangs and then fails is not automatically the same bug as one that dies
outright.

Every utterance reaches the speech stream twice — a queue-time frame, then
an audio-start follow-up whenever the synthesizer actually begins playing
it. The collector matches assertions against queue-time frames only:
under a loaded real synthesizer the audio follow-ups arrive seconds late,
interleaved with fresh queue-time frames, and matching them satisfied
assertions with stale text (the off-by-one that broke the M1 tab walk on
a cold guest until this was fixed). The rendered timeline still records
both, each audio follow-up as its own `audio`-tagged line, so a failure
readout shows real audio timing without the stale text ever counting as
an utterance.

### Hearing and recording a run: the three-mode story

There used to be a silent capture-synth default for the VM, an `--audible`
flag, and machinery trying to bridge listening and recording into a single
session. All of that is gone. A VM run now picks one of two things to do
with its audio, per invocation, and the two never overlap:

- `cargo xtask vm test`, with no flags, is audible by default: it deploys
  and runs the real `OneCore` synthesizer and real `WasapiSink`, always —
  there is no more capture-synth default and no `--audible` flag to opt
  into real audio, since a silent, unrecorded headless VM run produces
  nothing observable and has no purpose.
- `cargo xtask vm test --record` is also audible, and additionally records
  the run as a video with audio, for headless CI and developer review
  after the fact.
- `--no-restore` stays, orthogonal to both, exactly as before: it skips the
  checkpoint restore and its post-restore agent wait for fast local
  iteration, and must never be used for an acceptance run.

**The key constraint: recording audio and a connected RDP session are
mutually exclusive. You cannot do both at once.** The moment an RDP
session — `cargo xtask vm connect`, plain `mstsc.exe`, or `vmconnect.exe`'s
Enhanced Session — is connected to the guest, Windows replaces that
session's audio with a "Remote Audio" endpoint, and the VB-CABLE capture
device `--record` needs becomes invisible within that session. This was
proven live, twice, with two different tools: ffmpeg and, independently,
SoX, both failed identically to open the VB-CABLE capture device while an
RDP session was connected. It is ordinary Windows/RDP session-audio
behavior, not a bug in this harness, and there is no way around it from
inside the guest — two approaches were tried and abandoned:

- Configuring "Listen to this device" on the VB-CABLE capture endpoint,
  to mirror its audio out to whatever Remote Audio endpoint RDP created,
  via the registry. The audio-endpoint property-store keys involved are
  protected — writes are denied even running as SYSTEM — and re-enumerating
  the device (needed to make any change to it take effect) wipes them
  again regardless.
- A SoX-based forwarder relaying the cable's audio to Remote Audio. SoX
  cannot open the cable in the RDP session for the exact same reason
  ffmpeg cannot: the device simply is not visible there.

Neither is worth retrying. The practical rule that follows: to *hear* a run
live, connect first and then run `cargo xtask vm test` without `--record`;
to *record* a run, make sure nothing is connected to the guest first.

**To hear a run live:** `cargo xtask vm connect`, which starts the VM if
needed, enables Remote Desktop in the guest the first time only (idempotent
— a second run prints only "already" lines), stores the guest's test
credentials in this host's own Windows Credential Manager keyed to the
guest's address, then opens `mstsc.exe` against the guest with audio
redirected to this computer and no sign-in prompt. Because it signs in as
the same user the guest's autologon session already runs as, the RDP
session takes over that session rather than creating a second one — the
same session the agent and tests run in, so a following test run drives
exactly what you are watching and listening to. Then, in a second terminal,
`cargo xtask vm test --no-restore` (audible by default, no flag needed for
that anymore). Closing the RDP window disconnects rather than logging off,
leaving the guest running for the test to reuse. Run
`cargo xtask vm connect --forget` afterward to remove the stored
credentials again, for example on a shared host; as with any RDP
disconnect, see Troubleshooting's note below on a locked desktop silently
swallowing `SendInput` if gestures or keys stop landing after a session.
The older path still works too, with no host-side credential storage: open
`vmconnect.exe localhost verbatim` and turn Enhanced Session on (the
toolbar or View menu) to get audio redirection.

Every speech assertion in this suite, including `m1_exit_regression`'s
voice-combo section, expects a fixed pair of voice names chosen by mode: the
capture synth's two fixed names ("Capture A", "Capture B") for a
non-audible runner-direct run, or the VM's golden image's always-installed
`OneCore` voice order ("Microsoft David" default, "Microsoft Zira" listed
next) for an audible run, confirmed live — see that test's own module doc
and its `expected_voices` helper. `m1_exit_regression`'s own trailing
latency check does not yet pass under an audible run, though: it requires
at least one traced utterance to have actually reached audio, and every
timeline in a live audible run is reported interrupted before audio
regardless of a connected session or a generous settle pause — a gap
somewhere in the real `OneCore`/`WasapiSink` audio path, unrelated to and
unresolved independently of the speech assertions above.

**To record a run:** make sure no RDP session is connected to the guest,
then `cargo xtask vm test --record` (combinable with `--no-restore`, in
either order). Before starting ffmpeg, this re-asserts VB-CABLE as the
guest's default render device (`vm/scripts/Set-DefaultAudioRenderDevice.ps1`,
already staged to `C:\VerbatimLab\tools` by the golden image, run again at
record time to undo any stale pin a prior, now-disconnected `connect`
session left behind), then launches ffmpeg inside the guest's own
interactive session through the in-guest agent before the suite starts.
ffmpeg has to go through the agent rather than PowerShell Direct for the
same session-isolation reason the agent exists at all (see Troubleshooting
below): `gdigrab`, ffmpeg's Windows desktop-capture input, needs a real
interactive desktop, which a PowerShell Direct or WinRM session never has.
Recording itself needs no Enhanced Session and no `cargo xtask vm connect`:
the guest's VB-CABLE virtual audio driver
(`vm/scripts/Initialize-VerbatimHarness.ps1`'s `Install-VbCableAudioDriver`)
gives it a real WASAPI render endpoint ("Speakers (VB-Audio Virtual
Cable)") with a matching loopback capture endpoint ("CABLE Output
(VB-Audio Virtual Cable)") that ffmpeg's `dshow` input records from
directly, headless, with no host RDP session involved at all — which is
exactly why one must not be connected when `--record` runs. This driver is a
required part of the golden image for a recorded run to work — unlike the
Scream driver it replaces (which failed to root-enumerate a device node
under Secure Boot), a missing one now fails the image build rather than
silently leaving recording unavailable. The ffmpeg and ffprobe binaries the
recording path drives are not in the image at all: `cargo xtask vm deploy`
copies them into `C:\VerbatimLab\tools` over PowerShell Direct from the
LFS-vendored `vm/vendor/ffmpeg` copy, hash-skipping after the first deploy
just like Verbatim's own binaries (see the image-rebuild section below and
`vm/vendor/ffmpeg/README.md`).

If the guest's default render device cannot be pinned to VB-CABLE, or
ffmpeg exits immediately after being launched with an audio input, that is
treated as the expected fallout of a connected RDP session having hidden
the capture device — not aborted on. `cargo xtask vm test --record` prints
a clear warning and retries with a video-only ffmpeg launch instead, so the
scenario still runs and a video (with no audio track) is still pulled once
it finishes; the output filename gets a `-no-audio` suffix in that case,
decided by probing the pulled file with ffprobe rather than trusted from
which launch path was taken, so any other way a recording ends up without
real audio is caught the same way.

The recording is written inside the guest as fragmented MP4
(`+frag_keyframe+empty_moov+default_base_moof`), so terminating ffmpeg the
same blunt way `Scenario`'s own cleanup terminates everything else (no
graceful stdin `q`, just the agent's existing `KillProcess`) still leaves a
playable file: each completed video/audio fragment stands on its own, so
the worst a kill mid-fragment costs is a couple of seconds off the tail,
never the whole recording. Milestone M3 Track B moved the recording
boundary from the whole run to one scenario at a time: `--record` starts
ffmpeg immediately before that scenario's own `cargo test -p verbatim-e2e
<name> -- --exact` subprocess (covering its `Scenario::launch`, setup,
body, and teardown, deliberately, since setup and teardown are exactly
where a target application appears or a target application's window closes
— useful context for debugging, not noise to trim) and stops it as soon as
that subprocess exits, whether it passed or failed (a failing scenario's
video is exactly what is useful to look at). `cargo xtask vm test` pulls
each scenario's result out of the guest over the same PowerShell Direct
file-read `cargo xtask vm logs` already uses for flight-recorder dumps
(`Copy-VMFile` only copies host-to-guest, never the other direction), and
writes it to `artifacts/vm-recordings` on the host as one file per
scenario: `<scenario name>-<unix-seconds>.mp4` (or
`<scenario name>-<unix-seconds>-no-audio.mp4`), printing each path as it
lands. A multi-scenario `--record` run therefore produces one recording per
scenario, never one recording covering the whole run — the point of the
restructuring: a recording that only ever needs to show one scenario's
behavior is far easier to review than one long recording someone has to
scrub through. This per-scenario pull sequencing was verified live during
M3: a `--record` run of the whole suite produced one correctly named,
audio-carrying mp4 per scenario. The other Track B caveat, reading a
flight-recorder dump back off the guest through the agent, is exercised
continuously — every scenario's `collect_flight_recorder` pulls
`flight-recorder.jsonl` this way, and those dumps were read repeatedly
while root-causing M3's navigation and cold-start work.

`cargo xtask vm test` needs `LIBCLANG_PATH` for wxDragon's bindgen, exactly
as `cargo xtask ci` does (see this repository's `CLAUDE.md`) — building
`verbatim-app` pulls in `verbatim-gui`. Unlike a plain `cargo build`, `vm
test` (through `xtask::vm::deploy::build`) now probes for it itself, reusing
`cargo xtask ci`'s own candidate list, and sets it on the build's
environment automatically when found; it prints which directory it used, or
a note that it found none and is relying on `LIBCLANG_PATH` or `PATH`
already being set. Nothing needs to be exported by hand on a machine where
Visual Studio's bundled LLVM or a standalone LLVM install lives in one of
the probed locations.

## The VM harness

The `cargo xtask vm` verbs and the golden-image rebuild live in
[the VM harness guide](vm.md).

## Troubleshooting

**The agent is unreachable after a checkpoint restore, but perfectly
healthy inside the guest.** Restoring a running checkpoint resumes the
guest with the DHCP lease it held when the checkpoint was taken, and
Hyper-V's Default Switch regenerates its NAT subnet on every host reboot —
so a golden checkpoint restored after the host rebooted leaves the guest
on an unroutable address: the agent listens, PowerShell Direct works, but
no host-side TCP probe can connect (diagnosed live: guest on a 172.18.x
lease while the switch had moved to 172.21.x). Every restore path
(`vm restore`, `vm test`) now renews the guest's lease over PowerShell
Direct before waiting for the agent, so this fixes itself; if an agent
wait ever times out anyway, compare the guest's IP
(`Get-VMNetworkAdapter`) against the host's `vEthernet (Default Switch)`
subnet first.

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
state along with everything else — though a target application such as
Notepad no longer strictly needs that: `launch_target`-launched processes
are killed by pid and then swept by image name (Windows 11 Notepad hands
launches off to a differently pid'd process, confirmed live even for a
single, solo launch, so a pid-only kill can silently miss the real window),
and `Scenario::launch` sweeps those same image names before doing anything
else, so a scenario starts clean even after a prior run's cleanup was
skipped entirely.

**A gesture sent immediately after launch can silently do nothing.** The
control server starts (and so a tunnel connection succeeds) before
`verbatim-app`'s GUI thread finishes wiring its gesture handle; a
`SendGesture` that arrives in that narrow window is logged and dropped by
the router, not queued, even though the control plane still answers `Ok`.
`Scenario::launch` covers this with a fixed one-second settle delay after
its own connection succeeds; a hand-rolled script driving a freshly
launched Verbatim through `verbatim-inspect` should pause briefly after
launch for the same reason, especially on a slow CI runner.
