# The end-to-end suite

How to run the end-to-end suite on this machine: the test-audio mode its runs use, the agent, `cargo xtask park`, the measure and demo sets, and how a run is heard and recorded; the VM harness has a guide of its own.

## Test-audio mode

`VERBATIM_TEST_AUDIO=null`, set in the environment before starting
`verbatim.exe`, is a test-only escape hatch `verbatim-app`'s `run` checks at
startup. It does two things together: registers the capture synthesizer
(`verbatim-synth-capture`, id `capture`) alongside eSpeak NG and OneCore,
and builds the audio mixer over `verbatim_audio::SilentDevice` instead of
the real `WasapiDevice`. It does not change which synthesizer is
selected: that is still whatever `settings.toml` names, eSpeak NG by
default.

This exists because much of the tooling in these guides needs to run
with no sound card — a bare CI runner, a fresh VM image, or just a dev
machine where you don't want Verbatim actually talking while you script a
test. eSpeak NG is built with Verbatim, so it needs no installed voices
either. The capture synth records every `SpeechSequence` with a
timestamp into an in-memory log and produces only a short quiet tone; the
end-to-end suite does not select it; it serves unit tests and
by-hand use under this variable. `SilentDevice` plays at real-time speed
into silence: frames leave its queue at the rate a real device would play
them, so the mixer reports each utterance's audio start and ending at the
moments a listener would have heard them. `LatencyLedger` therefore still
records a complete event-to-audio timeline with nothing audible, the
latency each run records still has something to measure, and every
utterance still takes its real duration, so a test's speech is paced
exactly as it would be on real hardware.

Every scenario `crates/verbatim-e2e` launches sets this variable in
runner-direct mode unless `VERBATIM_E2E_AUDIBLE` is set (see below).
Every run, silent or audible, selects eSpeak NG, so silent and audible
runs differ only in the device: the same synthesizer produces the same
speech, played into silence or through the real device.
`cargo xtask vm deploy` (and `vm test`, and `vm create`'s own bake-in of
the golden checkpoint) stages a `settings.toml` selecting eSpeak NG too,
and every VM run is audible — see "Hearing and recording a run" below
for what that means for a VM run's audio.

A second variable, `VERBATIM_RECORD_AUDIO=<path to a WAV file>`, has
Verbatim record everything its mixer plays, as heard, into that file, with
silence where nothing played so the file runs in step with the clock, and
its start time (Unix milliseconds) in `<path>.start` beside it. It works
with either device, so a silent run records its speech too. The end-to-end
harness sets it for every recorded scenario; see "Hearing and recording a
run" below, and `WavRecorder` in [the verbatim-audio guide](../crates/verbatim-audio.md).

## Running the E2E suite runner-direct

"Runner-direct" means the agent and the tests it drives share one machine —
your dev box, or a plain GitHub-hosted Windows runner — as opposed to
`cargo xtask vm test`, which drives a Hyper-V guest. Every live test in
`crates/verbatim-e2e` is `#[ignore]`d, so `cargo test` and `cargo xtask ci`
list them as ignored, never as passed, and stay green with no agent
anywhere. A run with `--ignored` runs them, and each fails, rather than
passing, when the `VERBATIM_E2E_ENDPOINT` environment variable is unset.

To actually run the suite locally:

```
cargo build -p verbatim-agent
target\debug\verbatim-agent.exe --bind-address 127.0.0.1 --port 44001
```

The suite builds Verbatim itself (see below), so only the agent needs
building first.

Leave that agent running in its own terminal (loopback avoids a firewall
prompt that binding all interfaces would trigger on a dev machine), then in
a second terminal:

```
$env:VERBATIM_E2E_ENDPOINT = '127.0.0.1:44001'
cargo test -p verbatim-e2e -- --ignored --skip demo_ --skip measure_ --test-threads=1
```

`--test-threads=1` is not optional: every live test in this crate launches
a real `verbatim.exe` on the real desktop and injects real keystrokes, and
`crates/verbatim-e2e/src/scenario.rs` enforces only one such instance at a
time within the test process via a shared lock, but Windows itself has no
notion of "only one verbatim.exe" — two scenarios running concurrently
would each think they own an instance the other just replaced out from
under it (`single_instance::acquire_replacing`'s own algorithm).
Separate test processes are kept apart by the workspace's run lock,
`target/e2e-run.lock`, which each run holds from before it clears its
artifacts until it has archived them: a scenario started while another
`cargo test` in the same workspace is still running fails at once, saying
so, instead of deleting the other run's artifacts and stage files under it.
Wait for one run's process to exit before starting the next; a script
that stops waiting on a timeout and starts another run is the usual way
two overlap. Worktrees have their own locks, so two worktrees must not
run scenarios against the same desktop at once.
`.github/workflows/ci.yml`'s `e2e` job runs, on a plain `windows-latest`
runner with the same environment variable,
`cargo test -p verbatim-e2e --no-fail-fast -- --ignored --skip demo_ --skip measure_ --skip notepad_ --skip settings_search_suggestions --skip settings_combo_box_expanded --test-threads=1`,
with `VERBATIM_E2E_SKIP_LOCAL_ONLY=1` as well (see the local-only scenarios
below). Started by hand, as `gh workflow run ci.yml -f scenario=<name> -f
repeat=<n>`, the workflow runs only its `e2e` job, which runs that one
scenario n times, each in its own test process, and uploads every run's
copy from the history (below) as the `verbatim-e2e-history` artifact, so
an intermittent failure on GitHub's runner can be measured without a push
for each run. The job fails before any run when n is below 1 or the name
does not list exactly one test, since a misspelt name runs nothing and
passes. Before the suite, the job closes a message box the runner
image shows at logon: image 20261007.264.1 shows "System Properties"
(`SystemPropertiesPerformance.exe`, "Windows created a temporary paging
file..."), which stays all run and takes the foreground when Verbatim's
settings dialog closes. Closing it prepares the environment, as installing
ffmpeg does; it is the image's dialog, not the harness's.

Every scenario starts from the same desktop, recorded or not: the harness
closes what an earlier run left (processes the agent launched, by their
own handles; harness windows; Notepad's harness tabs, as tabs), minimizes
every window as the taskbar's Show Desktop does, and brings the desktop
forward, all without injecting input. Every shown window other than the
desktop, owned windows such as dialogs included, must then be minimized:
a scenario fails at setup, naming the window, when one is not, because a
window Show Desktop cannot minimize, such as a message box, takes the
foreground whenever the window in front closes, and the scenario would
fail later on what it announced instead. A window with the
`WS_EX_NOACTIVATE` style is the exception, since Windows never brings it to
the foreground: Windows 11 shows its Widgets board's window ("Widgets", of
`msedgewebview2.exe`) on its own, with no input from the harness, and it
stays shown for many minutes. It then asserts Verbatim's startup
speech and the desktop's announcement, which it reads independently
through the agent. The run takes over the desktop while it lasts.

On a development VM reached over Remote Desktop, a run needs the session
unlocked with a real foreground window, and disconnecting the RDP client
locks it (see [Troubleshooting](troubleshooting.md)). `cargo xtask park` moves the session onto
the machine's console instead, still signed in and unlocked, so a run
keeps working with no client connected; reconnecting over RDP takes the
session back. It runs `tscon <session> /dest:console` through a scheduled
task that runs as SYSTEM, which needs no password and also brings back a
session already disconnected and locked. Register the task once, from an
elevated PowerShell, with `vm\scripts\Register-VerbatimParkTask.ps1`; after
that `cargo xtask park` needs no elevation. It checks that the session
reached the console, that no lock screen is showing, and that an uncloaked
window holds the foreground, and fails saying which check did not pass.
The console must not lock on its own either: turn off the screensaver,
inactivity locking, and sleep, as `Initialize-VerbatimHarness.ps1` does
for the Hyper-V guest. While parked, the session is unlocked for anyone
who opens the VM's console.

The suite always runs against a fixed, generated configuration, never
whatever `settings.toml` a developer's own manual runs left behind.
Concretely, in this runner-direct mode, `Scenario::launch` first runs
`cargo build -p verbatim-app -p verbatim-outpost -p verbatim-synth-host -p mockapp`,
once per test binary, so
a run can never stage an executable older than the source under test
(`cargo test -p verbatim-e2e` alone builds only this crate and its
libraries, not Verbatim's executables). That build is a no-op when
nothing changed. It then copies `verbatim.exe`, `verbatim-outpost.exe`,
`verbatim-synth-host.exe`, `mockapp.exe` (the scripted application the
spelling errors scenario reads), and the `espeak-ng-data` and `sounds`
directories into
`target/e2e-stage` under the workspace root (skipping a copy when the
destination already matches byte-for-byte, file by file for the
directories) and writes a fresh `settings.toml` there — `Settings::for_e2e`
selecting eSpeak NG — before launching that staged copy.
Your own `target/debug/verbatim.exe` and its `settings.toml` are never read
or mutated by a test run. Against the VM, `cargo xtask vm deploy` stages the
guest side the same way, independently (the two crates cannot share code, so
they are kept in lockstep by hand — see `verbatim_config::Settings::for_e2e`'s
doc comment, the shared constructor both staging steps build from).

More environment variables matter for less common cases:

- `VERBATIM_E2E_VERBATIM_EXE` overrides the directory a scenario stages its
  binaries *from* — the source build, not where it actually launches from.
  It defaults to `target/debug/verbatim.exe` under the workspace root
  (computed from the crate's own manifest directory, so it does not depend
  on your working directory). Setting it chooses a different source build
  and skips the automatic build: the named binaries are staged as they
  are. The fixed, staged configuration regime is unaffected either way.
- `VERBATIM_E2E_REMOTE=1` marks a remote run, where the agent, Verbatim, and
  its configuration live on another machine (the Hyper-V guest) — set by
  `cargo xtask vm test`, not something you normally set by hand. It skips
  the runner-direct staging step (checking the source binaries exist and
  copying them), since `cargo xtask vm deploy` already staged the
  guest-side equivalent; the same fixed `settings.toml` is still written
  afresh for every scenario, through the agent, next to the guest's
  Verbatim, so a scenario that changes a setting
  (`windows_terminal_spoken_password` turns "speak passwords" on) never leaves it changed for the next.
- `VERBATIM_E2E_AUDIBLE=1` requests an audible run — see "Hearing and
  recording a run" below. Set by hand for a runner-direct audible run;
  `cargo xtask vm test` sets it automatically now, always, since a VM run
  is audible by default.
- `VERBATIM_E2E_RECORD=0` (or `false`) turns off the per-scenario video
  recording, which is otherwise on — see "Hearing and recording a run"
  below.
- `VERBATIM_E2E_FFMPEG` names the ffmpeg executable on the agent's machine,
  defaulting to `ffmpeg` found on that machine's `PATH`. `cargo xtask vm
  test` sets it to the guest's vendored copy,
  `C:\VerbatimLab\tools\ffmpeg.exe`.
- `VERBATIM_E2E_RUST_LOG` is passed to the Verbatim a scenario launches
  as `RUST_LOG`, and its outposts inherit it. Unset, every scenario's
  Verbatim logs with `info,verbatim_outpost=debug,verbatim_uia::subscribe=debug,verbatim_uia::com=debug,verbatim_ia2::hook=debug`
  (`DEFAULT_RUST_LOG`), so a failed run's artifacts hold the outposts'
  debug lines: what each read and how long it took, what it released, and
  what it reported; and, in every process, each UIA event's arrival ("UIA
  event received") and each `WinEvent`'s with how long ago it was raised
  ("WinEvent received"), so a delay before an event reached Verbatim
  shows, whatever became of the event. That line also gives the raw
  event id (`event`, in hexadecimal), which tells apart the events one
  kind stands for: the console updates are 0x4002 for a region, 0x4003
  for a single cell, 0x4004 for a scroll, and 0x4005 for a layout, and
  their ids are logged as the console host raised them. The focus listener runs from the
  outpost binary, so the filter covers it too. The
  debug lines cost nothing measurable: over six runs each of
  `menu_and_settings_dialog` and `settings_dialog_keys` (2026-10-07),
  event to queued speech had a median of 7 ms and a 90th percentile of
  39 ms both with this filter and with `info` alone, and event to audio
  a median of 8 ms and a 90th percentile of 40 ms with it, 42 ms
  without. Set but
  empty, Verbatim logs as its own configuration says. For a terminal's
  reads during a flood, set it to `info,verbatim_outpost::terminal=debug,verbatim_outpost::outpost::worker=debug`:
  each snapshot then logs `terminal snapshot timing` (the path, its time
  in microseconds, its calls, whether it read the whole document, the
  rows above its start, its text's length, and its flags), each read
  `terminal read timing` (its time, how long its event waited in the
  queue, and what the reading rules said), and each
  caret read `caret read timing`. The console
  host's outpost log then grows past the agent's 8 MB file limit, so
  read it from `target\e2e-stage\logs\<Verbatim's pid>` rather than from
  the artifacts.
- `VERBATIM_E2E_SKIP_LOCAL_ONLY=1` says the run is not on a Windows 11
  desktop: a local-only scenario run with it set fails, since such a run
  deselects them with `--skip notepad_ --skip settings_search_suggestions --skip settings_combo_box_expanded`; unset, empty, or `0` runs them,
  and any other value fails each test saying so. CI's `e2e` job sets it; see the
  local-only scenarios below.

Speech is asserted exactly (`docs/testing.md`): every utterance, in
order, with its exact text, ending as expected, nothing in between, and
each heard in full before the scenario injects its next input, since every
injected key, like every real one, cancels speech. Injecting input while
an utterance no assertion has matched is waiting fails the run as a
harness error. Every scenario ends by asserting that nothing more was
said: the harness numbers every key it injects, and Verbatim answers once
it has handled the last one and is idle, so anything it queued must
already have been matched. There is no separate paced mode.

The suite is a scenario registry (`crates/verbatim-e2e/src/registry.rs`,
milestone M3 Track B): every scenario is a named, grouped setup/body/teardown
definition, and `crates/verbatim-e2e/tests/e2e.rs`, the suite's one test
binary, holds one `#[test]` per scenario, named after it and calling
`registry::run_named` with that name, plus `session_info`'s
`agent_reports_an_interactive_window_station` (the agent reports an interactive session — the
precondition everything else depends on, not itself a scenario). The
M3 scenarios: `menu_and_settings_dialog` (the scripted walk of
the M1 exit criteria through Verbatim's menu and Settings dialog, now
asserting eSpeak NG's voices and its Max variant on the Speech page),
`synth_host_crash_recovery` (the synthesizer host is killed from outside
and the next announcement is still heard in full, from the host Verbatim
starts in its place), `switch_to_onecore` (switching to Windows OneCore
voices through the Select Synthesizer dialog and back to eSpeak NG,
which needs OneCore voices installed, as Windows 11 and GitHub's
runners have), `second_application_and_verbatim_menu` (switching the
foreground between the harness's Windows Forms text box and Verbatim's own
menu keeps the text box's outpost, the same process, and re-announces it),
`outpost_crash_recovery` (an application's outpost ended as a crash would
end it is replaced, saying nothing more, and the text is read through the
new one), `return_after_retirement` (an application brought forward again
once its outpost was retired after two minutes without attention is
announced as before, through a new outpost; it takes about three
minutes), `explorer_folder_window` (a File Explorer window on a harness
folder, where File Explorer's "Working on it..." placeholder is an
optional utterance on the window's opening and after Enter and after
Backspace, before the speech its item's focus brings or between that
speech and the item),
`settings_system_page` (the Settings app's System page),
`settings_combo_box_expanded` (a combo box on its Date and time page,
"expanded" spoken before its first item as Alt+Down Arrow opens it;
local-only),
`settings_search_suggestions` (the Settings app's search suggestions,
read as the selection moves through them while the focus stays in the
search box, their number and names read by the agent before they are
expected), `settings_search_suggestions_reentered` (the search box
entered again by Shift+Tab while it holds text, its line spoken with it
before the suggestion Settings selects; local-only by the same filter),
`lock_key_announcements` (Scroll Lock pressed twice is announced on,
then off, or the reverse),
`rapid_tabbing_in_settings` (a burst of Tab and Shift+Tab presses in
Verbatim's settings dialog must leave focus and the navigator on the
control that really has focus), `object_navigation_in_settings` (the M3
object-navigation and review commands against Verbatim's own settings
dialog), `object_navigation_over_uia` (the same walk over UIA, in
`mockapp`'s provider), and `system_information_tree` (an MSAA-only legacy application, msinfo32,
reaches Verbatim, and logical object navigation works through its real
Win32 tree view — the regression scenario for the flat MSAA tree-view
exposure).
Milestone M4's text scenarios add editing, word selection, typed word
echo, the review cursor, and say-all, each in the harness's Windows Forms
text box (`text_box_`) and in Windows 11 Notepad (`notepad_`), spelling errors in `mockapp`'s scripted text (`spelling_errors`)
and in Windows 11 Notepad (`notepad_spelling_errors`, local-only), and
the terminal scenarios, each in Windows Terminal (`windows_terminal_`) and in the console host
(`conhost_`), each its own code: commands (commands, typed echo, and a
password prompt), spoken_password (its typing spoken with "speak
passwords" on), flood (two thousand lines of output heard as their first
30 lines, one exact "skipped 1941 lines", and the last 30; Verbatim+5;
and the wall-time ratio, saved as `wall-time-ratio.txt` in its artifacts
directory for trends, never asserted), history_flood and
scrollback_overflow (thirty lines heard in full, then 5,000 or 12,000
written in one go, within the terminal's history or past it: the next
30, one exact "skipped 4941 lines" or "skipped more than N lines", N the
history's rows less the last 30, and the last 30), their
`_during_group` twins (the second part written while the first 30 still
play, so it joins their burst), editing (a command corrected
with Backspace, punctuation echoed by name, and the review cursor's
current word down a column), review_grid (the review cursor down a
column of a text table), progress (a line rewritten in place, each step
written once the last was heard, spoken whole the first time and then
by the word that changed), short_output (one- and two-character
lines of output, spoken like any other), and two_windows (a second
window, switched to and from as clicking its taskbar button does, and
typed in), with `windows_terminal_tabs` (a second tab, switched to and
from with Control+Tab and Control+Shift+Tab) in Windows Terminal alone.
Each terminal scenario opens a window of its own titled
`verbatim-e2e-<name>-<token>` and closes it by that title, so your own
terminals are left alone, and runs its shell in a folder of the same name
in `target/e2e-stage`, deleted once the window has closed. Each names its
terminal, Windows Terminal or the console host, and fails when it cannot
open it, with no fallback; it asserts the program that owns the window.

**The Windows Terminal scenarios never use your own Windows Terminal.**
Windows Terminal runs every window of one installation in one process,
so a scenario that opened its window through `wt.exe` ran inside the
same process as your own terminals, and on 2026-10-08 that process
crashed in UI Automation, ending every session in it. The
scenarios instead drive a copy of their own: the official portable
release, pinned to the version this machine runs (1.24.12741.0, in
`crates/verbatim-e2e/src/windows_terminal.rs`), downloaded from GitHub
with Windows' `curl.exe`, checked against its pinned SHA-256, for x64 or
ARM64 as the machine is, and unpacked into
`target/e2e-stage/windows-terminal-1.24.12741.0` with a `.portable` file
next to its executable. Portable mode keeps its settings in that folder,
under `settings`, which each scenario deletes first, so every run starts
from the release's defaults. An unpackaged Windows Terminal names the
window class and mutex it finds a running instance by with a hash of its
own executable's path, so the copy never hands its command line to your
installed Windows Terminal, nor yours to it. A runner-direct run
downloads it the first time anything is staged; `cargo xtask
windows-terminal` does the same on its own, and `cargo xtask vm deploy`
ships it to the guest next to Verbatim.

Each Windows Terminal scenario starts that copy's `WindowsTerminal.exe`
directly, never `wt.exe` from `PATH`, with `-w new --size 120,30 new-tab
--title <title> --suppressApplicationTitle` and the shell, Windows
PowerShell started with `-NonInteractive` so that it prints no notice
that it leaves PSReadLine out, which Windows Terminal could show after
its window is announced (`docs/crates/verbatim-e2e.md`); the console
host's shell is started the same way, since its window takes the
foreground as it is created, before the notice is printed. Before it
drives the window, it asserts that the process the agent launched owns
the window, and that no other Windows Terminal process opened a window
meanwhile. At cleanup it closes that window by its title, and the copy's
process, which had no other window, must exit; nothing else is closed.

Nothing else a run starts can reach your Windows Terminal either. Windows
gives a console program started from a process without a console a new
console, and hands that console to the default terminal application,
which opens it in your Windows Terminal. So every program the agent
starts without a console title, Verbatim, mockapp, ffmpeg, and the
Windows PowerShell that shows the Windows Forms text box among them, and
every console program the harness itself runs, is started with no console
window (`CREATE_NO_WINDOW`); the console host scenarios start
`conhost.exe` explicitly. Every scenario fails if a window of a Windows
Terminal process the agent did not launch appears while it runs.

**The Verbatim under test never reads your Windows Terminal.** A stray
UIA focus fact from the owner's Windows Terminal used to start an outpost
for it, which then called into it, and that Windows Terminal crashed once
inside UI Automation while it was read (`docs/design/phase6-design.md`, "Windows
Terminal crash of 2026-10-08"). So the agent launches Verbatim with
`ignore_foreign_terminals` (`AgentClient::launch_verbatim`): it lists
every `WindowsTerminal.exe` process outside the agent's own jobs, and the
`OpenConsole.exe` processes they host, holds each open for as long as it
keeps the launch, and names their pids to Verbatim in
`VERBATIM_IGNORE_PIDS`, separated by commas. Verbatim holds them open
too, and ignores them entirely: its focus listener drops every fact from
them before routing it, the supervisor never starts an outpost for one,
and Core never asks one for its focus or takes a focus in one of their
windows. The portable copy a terminal scenario launches is in the agent's
jobs, so it is read as usual. Every scenario fails if Verbatim's log
folder holds an outpost log named for one of those processes. Nothing
else sets the variable, so a Verbatim you start yourself ignores
nothing. What still reaches your Windows Terminal is UI Automation's own
work for any desktop-wide client: see `docs/design/phase6-design.md`, "Ignoring the
owner's Windows Terminal".
CI's `e2e` job runs `cargo xtask windows-terminal` as a step of its own
before the suite, so GitHub's runner tests the same release, not the
Windows Terminal its image ships.

The terminal scenarios type through the agent's `TypeText`,
which maps each character with the foreground window's keyboard layout, so
any layout that can type the commands works. `crates/verbatim-e2e/src/
scenarios/` documents exactly what each asserts, at the top of its module.
A scenario's name is also its
`#[test]` function name, so `cargo test -p verbatim-e2e --test e2e --
--ignored --exact <name> --test-threads=1` runs exactly that one scenario
runner-direct, the same
selection mechanism `cargo xtask vm test --scenario <name>` uses against the
VM (see "cargo xtask vm verbs" in [The VM harness](../vm.md)).

Some scenarios are local-only, marked `local_only` in the registry. Most
test Windows 11 Notepad, which GitHub's Windows Server runner lacks (it
has classic Notepad, a Win32 edit control), and their names start with
`notepad_`; editing behaviour is also covered in the harness's Windows
Forms text box, which runs everywhere. `settings_search_suggestions` is
local-only because the runner's Settings raises no selection event for a
first suggestion as the suggestions list opens, so "Suggestions
available" plays to completion there, while the scenario expects the
first suggestion, which this machine's Settings selects as the list
opens, to cut it off (Dickson, 2026-10-21). `settings_combo_box_expanded`
is local-only because the runner's Settings opens its Date and time page
with the focus on "Location Settings link", where this machine's opens
it on "Region button", which the scenario's opening speech asserts: GitHub
run 39686251059 heard "Settings", "Settings window", "Location Settings
link", a difference in the runner's Settings rather than in Verbatim. The
opening is not read from the agent, as the suggestions' names are,
because the agent reads no role, and composing the spoken role word in
the scenario would copy Verbatim's role vocabulary into it. The
registry's `LOCAL_ONLY_FILTERS` lists the filters, and only local-only
scenarios' names start with them. A local-only
scenario is part of the suite: every local run and every
`cargo xtask vm test` run includes it. CI's `e2e` job deselects them with
`--skip notepad_ --skip settings_search_suggestions --skip settings_combo_box_expanded` and sets
`VERBATIM_E2E_SKIP_LOCAL_ONLY=1`, so one that
ran there anyway would fail rather than report a pass. The skip comes from
that setting alone, never from detecting the machine, so a local run
cannot silently lose a scenario. Mark a new scenario local-only only when
it tests Windows 11 Notepad, giving it the `notepad_` prefix.

The registry also holds demonstrations, its `demo` group
(`demo_notepad_editing`, `demo_review_cursor`, `demo_say_all`,
`demo_terminal_session`, and `demo_settings_dialog_keys`): scenarios
written to be recorded as videos for `videos/demos`, showing a feature at
a viewer's pace, so that each announcement is heard in full before the
next action, typing included. They are never part of the suite. Their
`#[test]` wrappers are `#[ignore]`d like every live test, and the suite's
runs leave them out with `--skip demo_`; `registry::select` leaves them out when
no `--scenario` or `--group` is given; and `cargo xtask vm test` refuses
them by name or group. They follow the suite's rules otherwise: the same
fixed settings and speech rate, no fixed waits, and assertions on what
they show, so a broken feature fails the recording rather than producing a
misleading video. `demo_terminal_session` runs in the harness's portable
Windows Terminal and fails saying so when it cannot be started. `cargo xtask demo <scenario>
[--name <name>]` records one scenario on this machine: it builds and
starts an agent of its own, from `CARGO_TARGET_DIR` when that is set, runs the scenario with `--include-ignored` and
the recording's demo quality, and copies the video to `videos/demos` for
a demonstration or `videos/tests` for a test scenario (`videos/readme.md`
describes every video). Like any local run, it takes over the desktop.

Every scenario run, pass or fail, keeps its artifacts, and losing any of
them fails the run. It writes a one-line-per-fact summary (name,
pass or fail, latency counts, and the outpost crashes it saw) to `target/e2e-artifacts/<scenario name>/
summary.txt` under the workspace root (`VERBATIM_E2E_ARTIFACTS_DIR`
overrides the root), whether run runner-direct or through `cargo xtask vm
test`; `xtask vm test` reads this back to build its own run summary rather
than parsing test output.

The summary's latency counts and maxima
cover every utterance that carries its event's time (the startup speech
and the desktop's announcement carry none), the rows `latency.csv` holds
(`latency_records` utterances, `latency_reached_audio` of them played,
and the worst event to queue and event to audio among them, in
milliseconds). Both are taken once Verbatim has quit, from every speech
frame Verbatim sent, read until the speech connection ends (the agent
shuts it down when Verbatim's pipe closes, and the harness's reader
passes that end on after every frame it read; an end that does not come
within the step timeout fails the run), or, when the quit failed, from
the frames that have arrived, so they include the run's last
utterance, also in the measure set, which does not wait for its speech
to end, and a failed run's speech after its failing step. They count
utterances, not traces, and an utterance's event to audio includes the
time it waited while the speech queued before it played. Until c5be1bd2
(2026-10-21) the counts and maxima were per trace, from Verbatim's own
latency record, so the audio maximum is not comparable across that commit,
and the "audio max" `cargo xtask vm test` prints shows a step there with
no regression behind it: `explorer_folder_window`'s went from 294 ms to
2,574 ms, its folder's first item waiting behind the window's title and
list.

Every scenario run, pass or fail, also writes into
the same directory the interleaved timeline
(`timeline.txt`, whole, where a failure prints its earlier steps shortened),
what held the foreground before setup and after teardown (`foreground.txt`,
so a failure can be read against the desktop it started from; after
teardown, a `SystemSettings.exe` whose every window is cloaked is named
as the Settings app suspended, rather than listed among the visible
windows: it is Windows' prelaunch or a Settings left suspended by this or
an earlier scenario, no window a scenario left, since Windows starts that
app ahead of use and keeps it suspended with its window cloaked, and keeps
a Settings a scenario opened so once its window closes; which it is
cannot be told, since a scenario opens Settings through `explorer.exe`
and records the frame host's process, not `SystemSettings.exe`. The
cloaked "Settings" frame of `ApplicationFrameHost.exe`
(`ApplicationFrameWindow`) that hosts it stays listed among the visible
windows, as cloaked, since the agent's window list does not say which
application a frame hosts; Dickson, 2026-10-21),
Verbatim's captured stderr log (`stderr.log`), the per-process outpost,
listener, and synthesizer host logs that each spawned process's stderr was
redirected into (`listener.log`, one `outpost-<image>-<pid>.log` per
application, Core's own as `outpost-verbatim-<pid>.log`, and
`synth-espeak.log`, plus `synth-onecore.log` when OneCore was started —
every file in the launch's own log directory, `logs\<Verbatim's pid>` next to Verbatim's executable,
listed and read through the agent), and a reducer flight-recorder
dump (`flight-recorder.jsonl`, fetched via the control plane's `DumpRecorder`
request and read back through the agent) — the timeline, stderr, and outpost
logs by `Scenario::collect_run_artifacts` and the flight recorder by
`Scenario::collect_flight_recorder` (taken before the clean quit, so a passing
run captures it too), both from inside the scenario's own process, where the
live control and agent connections they need still exist. It also holds
`latency.csv` (each utterance's step, text, and event-to-queue and
event-to-audio times), `focus.txt` (Core's focus, its ancestors, and the
navigator, from the control plane's `DumpFocus`), `verbatim-audio.wav`
(everything Verbatim played), and any crash dump written during the run
(see "Crash dumps" in [Troubleshooting](troubleshooting.md)). When the run was
recorded, the directory also holds its video, `<scenario name>.mp4` (see
"Hearing and recording a run" below). The scenario's
directory holds only its latest run; each run is also copied to
`target/e2e-artifacts/history/<scenario name>/<UTC time>-<pass or fail>`,
keeping the newest 100 runs of each scenario, so an intermittent failure
survives the runs after it. The history leaves out the video, so there is
at most one video per scenario on disk. A passing run leaves
these behind so its announcement timings and reducer inputs can be read, not
only a failing one. None of this is a retry mechanism: a failed scenario is
reported failed exactly once, with these artifacts left for root-causing,
never re-run automatically by anything in this crate or by `xtask`.

Reading a speech-assertion failure: once the run's logs are collected,
the failure prints three things. First, the expected and actual
sequences, escaped (`{:?}`), with the index of the first utterance that
differs and the first character in it that differs. Second, the failing
step's trace: every line carrying its trace ID in Verbatim's log, Core's
flight recorder, and the outposts', listener's and synthesizer host's
logs, merged in time order, each prefixed with its file's name. The
trace ID is the one the utterance the assertion failed on carries, or,
when the expected utterance never came, the newest one the step heard.
Third, the run's timeline up to the failure: each earlier step on one
line (when it started, the gesture, keys or text injected, and what was
said), then the failing step in full, one entry per line, each prefixed
with the milliseconds elapsed since the first entry; `timeline.txt` has
every step in full. Because the commands the test
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

Every utterance reaches the speech stream as up to three frames: a
queue-time `Speech` frame with its text, a `SpeechStarted` frame when its
first frame plays, and exactly one `SpeechEnded` frame. The collector
matches assertions against queue-time frames only, then waits for the
matched utterance's ending. The rendered timeline records all three, the
start as an `audio` line and the ending as a `completed`, `cancelled`, or
`failed` line (with the reason), so a failure readout shows real audio
timing and how each utterance ended. Each frame is recorded when it
reaches the harness, by a thread that reads the speech connection
throughout, so its time is true even while the scenario waits on the
agent; before that thread, frames that arrived during a slow request,
such as a window brought forward, were recorded only when the request
was answered. An assertion that fails because its
utterance was cancelled usually means something cut it off: a key press
(injected keys included), a new foreground window, entering a menu, a
focus change that left the focus it announced, or interrupting speech;
the lines just above the `cancelled` line show what. A pause or resume of
speech (Shift) comes as a `SpeechPaused` frame, recorded as a `speech
paused` or `speech resumed` line; a scenario about pausing asserts it on
its own, apart from the utterances, after the key that caused it. Every
Shift pressed with another key, such as Shift+Tab, pauses speech and the
key after it resumes it, as in NVDA, so one no step asserted is left
behind at the next input.

### The measure set

The registry's `measure` group holds the measure set, the measurement
workloads that `docs/design/communication.md` section 11 runs at every
migration step: its first step records the baseline, and later steps
compare their figures with it. They are measurement, not tests
(`docs/testing.md`): they assert Verbatim's startup speech and the
desktop's, then collect every utterance without asserting it, and pass by
completing. The figures come from Verbatim's latency ledger, the
`verbatim::latency` lines at info in Verbatim's log (described in [inspect.md](inspect.md)),
which every run keeps in its artifacts as `stderr.log`. Each scenario also
writes `measure-truth.jsonl` to its artifacts directory: one JSON line per
step, with what was typed or what each key removed, the harness numbers of
its keys, and the speech collected around it.

The scenarios, all named `measure_`:

- `measure_focus_windows_terminal`: two Windows Terminal windows, brought
  forward 20 times by turns as a taskbar button does.
- `measure_focus_windows_terminal_tabs`: two tabs of one Windows Terminal
  window, 20 switches with Control+Tab and Control+Shift+Tab.
- `measure_focus_explorer`: 10 new File Explorer windows, each holding
  two files named for it, each opened and closed once its first item is
  announced (the first focus in a new window), with a wait after each
  close for the folder window's item as it comes back to the front, so
  every new window opens on the same desktop;
  then 20 rounds of arrows, Enter into a subfolder and Backspace back,
  each key followed by a wait for the item it moves to. None of these
  announcements is part of an input Verbatim reports handled, and each
  can come after Verbatim is idle, so the scenario waits for the
  announcement itself, as `explorer_folder_window` does. It writes
  `measure-summary.txt` to its artifacts: its samples by kind, each with
  its count and figures in milliseconds, and no figure over all of them,
  since the new windows' first focus, which includes a new outpost's
  start, sets any overall percentile alone. The kinds are keys (each
  key's item; their audio figures are taken from the arrow keys' rows
  alone, by their step, `keys downarrow` or `keys uparrow`, so they
  cover 80 of the 120 items in every run: the items after Enter, and
  after every Backspace but the last, never play, the next key's speech
  ending them, and the last Backspace's item waits behind the folder
  window's title and list, so its time to audio is not a key's), new
  windows (each window's title, once per window, though its list and
  item share its latency) and returns (the folder window's title heard
  between each close and the folder window's item, measuring the title's
  event to the title being queued; the list and the item come from a
  later focus event, raised 49 to 304 ms after the title's in round
  fifteen's `mfe-3` (`target/e2e-evidence/fixes-round-fifteen-2026-10-21/mfe-3`)
  and queued within 35 ms of the title). Each percentile is
  the sample at the rounded index p/100 times (n-1), index
  `(p(n-1)+50)/100` of the sorted samples, so a kind's P95 is its maximum with 11
  samples or fewer, and its P99 with 51 or fewer, so the new windows'
  and returns' 10 samples give both as the maximum, as the file says.
  Runs before 3de02d37 (2026-10-21) closed each new window without
  waiting for its announcement, so they carried few or no new-window
  samples, none waited for: their P50 and P95 compare with the keys'
  figures, while their P99 and maximum can hold a new window.
- `measure_focus_text_box`: two Windows Forms text boxes, brought forward
  20 times by turns.
- `measure_focus_new_application`: 12 newly started applications, each
  with a fresh outpost, the Windows Forms text box (MSAA) and `mockapp`'s
  text box (UIA) by turns, each closed before the next.
- `measure_keys_text_box`: 20 Pause keys injected by Verbatim itself
  through its control plane's `SendKeys` (the held-key re-injection
  workload; Pause is not announced, so the ledger logs no line for them),
  then three rounds of typing at 150 ms and 60 ms a character and
  Backspace at 33 ms, in the Windows Forms text box, over 30 numbered
  lines; then three rounds of the caret keys from the top (Right, Left,
  Down and Up Arrow, ten each at 150 ms, then ten Down Arrows 30 ms
  apart, the flight recorder dumped after each burst to `burst-<round>`),
  and ten rounds of select then copy's first press over the top line's
  first word followed at once by Right Arrow (the recorder dumped after
  them to `select-then-right`).
- `measure_keys_notepad`: the same typing, caret keys and select then
  Right in Windows 11 Notepad, over the same lines. It needs Windows 11
  Notepad, so it is not in the set `cargo xtask measure` runs by default
  and is named with `--scenario`; it is not marked local-only, since a
  local-only scenario's name must start with one of the skip filters.
- `measure_keys_conhost` and `measure_keys_windows_terminal`: in each
  terminal, three rounds of typing at 150 ms, ten Left and ten Right
  Arrows at 150 ms, then Escape, and typing at 60 ms then Backspace at
  33 ms; typing while a script prints a line every 50 ms; and five rounds
  of typing a line that wraps onto a second row, then Escape.
- `measure_stale_caret_text_box` and `measure_stale_caret_notepad`: ten
  rounds of Control+Home, then Shift+Right Arrow and Right Arrow in one
  batch, each round's flight recorder dumped to
  `shift-right-then-right-<round>`; and `measure_stale_caret_conhost`: a
  command that wraps onto a second row, then ten rounds of Home, say-all
  from the caret heard through its first utterance, and Right Arrow, each
  round's recorder dumped to `say-all-then-right-<round>`. They measure
  the stale caret cases of [holding caret keys](../design/key-holding.md),
  are not in the default set, and are named with `--scenario`.
- `measure_held_repeat_text_box`: three times, Control+End in the
  Windows Forms text box, then Right Arrow, which moves nothing there,
  held about a second with auto-repeat through the agent's `RepeatKey`
  (the press, 15 repeats from 500 ms, 33 ms apart, and the release at
  995 ms), the release's time recorded in `measure-truth.jsonl` in
  microseconds since the Unix epoch to compare with the times of
  Verbatim's log; Verbatim may take up to 300 seconds to become idle
  after it. It measures the wait of a held key for the key ahead of it
  ([holding caret keys](../design/key-holding.md), "A key held with
  auto-repeat"), is not in the default set, and is named with
  `--scenario`.
- `measure_speech_real` (`docs/design/communication.md`, section 10.3):
  the speech path through the real synthesizer host and audio device, so
  the ledger's host and device clock stages have figures: two Windows
  Forms text boxes brought forward 20 times by turns, three rounds of
  typing at 150 ms, caret keys and Backspace, and 20 rounds of the review
  key that reads the current line, the same key on a long line followed
  300 ms later by a caret key pressed while it is spoken, and Home. It
  needs an audible run, `$env:VERBATIM_E2E_AUDIBLE = '1'`, and fails at
  once without it, so a silent run of the whole set passes
  `--skip measure_speech_real`. It runs where a real audio device exists:
  this machine and bare metal, not GitHub's runners.

The agent paces the typing and Backspace with a high-resolution waitable
timer (`SendKeysPaced` and `TypeTextPaced`); the pace is the workload, not
a wait. Like the demonstrations, the measure set is never part of the
suite: its `#[test]` wrappers are ignored and named `measure_`, so the
suite's runs deselect them with `--skip measure_`, and `registry::select`
leaves them out when no `--scenario` or `--group` is given.

To run one locally, start the agent as for any local run, then, in a
second terminal:

```
$env:VERBATIM_E2E_ENDPOINT = '127.0.0.1:44001'
$env:VERBATIM_E2E_RECORD = '0'
cargo test -p verbatim-e2e --test e2e -- --ignored --exact measure_focus_explorer --test-threads=1
```

`VERBATIM_E2E_RECORD = '0'` leaves out the video, which a measurement does
not need and whose encoding adds load. Run the whole set silently with
`cargo test -p verbatim-e2e --test e2e -- --ignored measure_ --skip measure_speech_real --test-threads=1`,
and `measure_speech_real` on its own in an audible run.
Against the VM, `cargo xtask vm test --group measure` runs the set, with
`measure_speech_real` failing at once in a silent run, and
`cargo xtask vm test --scenario measure_keys_conhost` one of it, in debug.

Section 11 compares debug and release, so measure both. Debug is what a
run stages by default: it builds the debug binaries and stages them from
`target\debug`. For release, build every binary the run stages, then name
the release `verbatim.exe` with `VERBATIM_E2E_VERBATIM_EXE`: the run then
stages `verbatim.exe`, `verbatim-outpost.exe`, `verbatim-synth-host.exe`,
`mockapp.exe`, `espeak-ng-data` and `sounds` from that executable's folder
and skips its own debug build. The agent can stay a debug build.

```
cargo build --release -p verbatim-app -p verbatim-outpost -p verbatim-synth-host -p mockapp
$env:VERBATIM_E2E_VERBATIM_EXE = "$PWD\target\release\verbatim.exe"
cargo test -p verbatim-e2e --test e2e -- --ignored --exact measure_focus_explorer --test-threads=1
```

Remove the variable (`Remove-Item Env:VERBATIM_E2E_VERBATIM_EXE`) to go
back to debug, and before `cargo xtask trace scenario` or `cargo xtask
measure`, which refuse to start while it is set and take another build as
`--verbatim-exe <path>`
([measuring.md](measuring.md)). The stage is replaced file by file, so the two never mix.
Both profiles write the same scenario folders under
`target\e2e-artifacts`, so copy the artifacts away between a debug and a
release run; the history under `target\e2e-artifacts\history` keeps each
run too.

### Hearing and recording a run

How a run sounds, and whether it is recorded, are independent:

- `cargo xtask vm test` is always audible: it deploys and runs eSpeak NG,
  the default synthesizer, through the real `WasapiDevice`. There is no
  capture-synth default and no `--audible` flag, since a silent VM run
  has no purpose. A runner-direct run is silent unless
  `VERBATIM_E2E_AUDIBLE=1` is set (see "Test-audio mode" above).
- Every scenario is recorded, in every mode (runner-direct on a
  development machine, the CI `e2e` job, and `cargo xtask vm test`),
  whenever ffmpeg can be started on the agent's machine. Set
  `VERBATIM_E2E_RECORD=0` to turn recording off. The video, with
  Verbatim's own audio, lands in
  `target/e2e-artifacts/<scenario name>/<scenario name>.mp4` on the
  machine running the tests, pass or fail.
- `--restore`, orthogonal to both, restores the `golden` checkpoint before
  deploying. Restoring is never automatic: an ordinary run deploys onto the
  guest as it is, since deploy only copies files and nothing is installed
  into the guest. An acceptance run, which must start from a known-clean
  guest, passes `--restore`.

Listening live and recording work together, over `cargo xtask vm connect`
or on a local machine, because the recording's audio does not come from
any audio device: Verbatim records it itself (decision D16 in
`docs/architecture.md`).

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
`cargo xtask vm test` (audible by default, no flag needed for
that anymore). Closing the RDP window disconnects rather than logging off,
leaving the guest running for the test to reuse. Run
`cargo xtask vm connect --forget` afterward to remove the stored
credentials again, for example on a shared host; as with any RDP
disconnect, see [Troubleshooting](troubleshooting.md)'s note on a locked desktop silently
swallowing `SendInput` if gestures or keys stop landing after a session.
The older path still works too, with no host-side credential storage: open
`vmconnect.exe localhost verbatim` and turn Enhanced Session on (the
toolbar or View menu) to get audio redirection.

Every run speaks through eSpeak NG, audible or not, so the speech
assertions are the same in every mode. `menu_and_settings_dialog`'s
voice-combo section expects eSpeak NG's default voice, "English (Great
Britain)", and the voice listed after it, "English (Scotland)", both
fixed by eSpeak NG's data — see that test's own module doc. Only
`switch_to_onecore` depends on the machine: it needs some Microsoft
OneCore voice installed, whichever one is the default.

**How a scenario is recorded** (`crates/verbatim-e2e/src/recording.rs`).
`Scenario::launch` starts the agent's desktop capture through the agent
(`verbatim-agent capture-desktop`, launched with the command
`AGENT_SELF`) before it launches Verbatim, and launches Verbatim only
once ffmpeg has its first frame (the evidence is ffmpeg's output file,
which it creates only after logging that frame's time), so the video
shows Verbatim start. The capture has to go through the agent for the
same session-isolation reason the agent exists at all (see "The
interactive-session rule" in [the tooling guide](readme.md)): it needs a
real interactive desktop, which a PowerShell Direct or WinRM session
never has. It duplicates the first output with DXGI Desktop Duplication
and writes a raw frame to an ffmpeg child every frame interval, the
newest the compositor presented or the last again when nothing changed,
which ffmpeg encodes as fragmented MP4 into Verbatim's launch directory on
the agent's machine (`target/e2e-stage` runner-direct,
`C:\VerbatimLab\verbatim` in the guest), a fragment starting at each
keyframe, one a second. It does not use ffmpeg's `gdigrab`, whose GDI blit
of the screen waits while the compositor renders: on a machine without 3D
graphics that blit waited 1.7 to 3.3 seconds while the console host drew a
200-line flood, and the console host's flood scenarios failed their
recordings; duplication had no gap between frames over 81 milliseconds
through the same floods (measured 2026-10-24). ffmpeg's own duplication
sources need a Direct3D 11 device with video support, which the Microsoft
Basic Render Driver lacks. The mouse pointer is not drawn. The capture is
stopped with `q` on its standard input, which the agent gives it as a
pipe (`stdin_piped` and `WriteStdin`): it closes ffmpeg's input, ffmpeg
encodes every frame it was given and exits, and the harness waits for the
capture to exit with code 0. Killing ffmpeg loses more than the fragment
being written: the muxer holds every frame since the last keyframe in
memory, and up to 32 KB of the fragment before that waits in a file
buffer that is never flushed, so a killed capture loses its last one to
two seconds and ends in a cut keyframe. The capture is killed with
`KillProcess`, ffmpeg with it, only when it has not exited 15 seconds
after being told to stop, and the scenario then fails. The launch also
sets `VERBATIM_RECORD_AUDIO`, so Verbatim writes everything its mixer
plays into a WAV file in the same directory, wherever that audio went: to
speakers, to an RDP session's Remote Audio, or into silence on a machine
with no audio device (where Verbatim plays silently in real time).

At the end of every scenario, pass or fail, `registry::run` calls
`Scenario::finish_recording`, which stops the capture, lines the audio up
with the video from their two start times (the WAV's `.start` file, and
the wall-clock time ffmpeg logs for the first frame), muxes them on the
agent's machine into one MP4 with AAC audio, and copies it through the
agent's `ReadFileChunk` request to the scenario's artifacts directory,
with ffmpeg's logs (`recording-video.log`, `recording-mux.log`, and
`recording-check.log`). It then checks the video: decoding it with
`ffmpeg -v error -f null` must log nothing, the mux log must have no error
and no corrupt packet (that ffmpeg exits 0 regardless), the frame count
on the final stats line of `recording-video.log` must equal the number of
frames in the video and its `time=`, the end of the last encoded frame,
must match the end of the video's last frame within one frame, which
catches a killed capture that lost fragments it had encoded; and the
wall-clock time of the first frame plus the last frame's time must be at
or after the moment `registry::run` recorded when the scenario's body
ended. ffmpeg stamps each frame with the wall-clock time it reads it
(`-use_wallclock_as_timestamps`), in a time base of the frame interval,
and keeps every frame at its time (`-fps_mode passthrough -enc_time_base
1:1000`), so the video keeps real time when a loaded machine makes the
capture fall behind. A recording that fails the check fails the scenario,
and its video is still copied, to be watched. The recording covers the
scenario's launch, setup, body, and teardown, since setup and teardown are
where a target application appears or closes. A recording that cannot
start (no ffmpeg, or no desktop to duplicate) or cannot finish fails the
scenario. Verbatim writes its audio file in every run, recording or not,
so recording changes nothing about the run it records.

A local runner-direct run therefore needs ffmpeg on `PATH` to record
(for example `winget install ffmpeg`, then a new terminal), or
`VERBATIM_E2E_FFMPEG` naming it, or `VERBATIM_E2E_RECORD=0`. The CI `e2e` job installs ffmpeg and
uploads the videos with the rest of each scenario's artifacts. For the VM,
`cargo xtask vm deploy` copies ffmpeg into `C:\VerbatimLab\tools` over
PowerShell Direct from the LFS-vendored `vm/vendor/ffmpeg` copy,
hash-skipping after the first deploy just like Verbatim's own binaries
(see `vm/vendor/ffmpeg/README.md`), and `cargo xtask vm test` points
`VERBATIM_E2E_FFMPEG` at it.

The video needs a desktop that is being drawn. An RDP session that is
connected, or a guest console session no RDP client has touched, both
are; a session left behind by a plain RDP disconnect is locked, which
breaks the run itself before it matters for the video (see
[Troubleshooting](troubleshooting.md)'s note on a locked desktop).

### Comparing a scenario with NVDA

A scenario can also run against NVDA instead of Verbatim, asserting
nothing, to see what it expects beside what NVDA says, step by step:
`cargo xtask nvda compare <scenario>`, or `VERBATIM_E2E_COMPARE` set to
the scenario's name with the usual `cargo test` command. Left set, the
variable fails every other scenario at once as a harness error, so unset
it before any other run. [The NVDA transcript
guide](../nvda-transcript.md), "Comparing a scenario", says how, and
which scenarios cannot be compared.

## The VM harness

The `cargo xtask vm` verbs and the golden-image rebuild live in
[the VM harness guide](../vm.md).
