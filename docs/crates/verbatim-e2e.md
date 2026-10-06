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
  `send_keys` (real key strokes through the agent's `SendKeys`),
  `type_text` (a string typed as real key presses through the agent's
  `TypeText`, each character mapped to its key and shift state in the
  foreground window's keyboard layout),
  `read_file`, `copy_file` (a file of any size on the agent's machine,
  read in chunks with the agent's `ReadFileChunk` request and written to a
  path on this machine), and `open_control_tunnel` as plain methods.
  `open_control_tunnel` is the seam into Verbatim's own control plane: it
  asks the agent to stop speaking its own protocol on the connection and
  relay Verbatim's control-plane pipe instead, then completes the control
  protocol's own `Hello` on the same socket and hands back a ready
  `verbatim_control::client::Client`.
- `Scenario` — the lifecycle owner for one live, agent-driven Verbatim run:
  a guard struct, not a manual-cleanup checklist. In runner-direct mode
  `Scenario::launch` first builds `verbatim-app`, `verbatim-outpost`,
  `verbatim-synth-host`, and `mockapp` (once per test binary, skipped when
  `VERBATIM_E2E_VERBATIM_EXE` names a build to stage instead), then
  copies `verbatim.exe`, `verbatim-outpost.exe`,
  `verbatim-synth-host.exe`, `mockapp.exe`, eSpeak NG's `espeak-ng-data` directory, and
  the default theme's `sounds` directory
  into `target/e2e-stage` (staging fails if either directory is
  missing; building `verbatim-synth-host` creates the first and building
  `verbatim-app` the second) and writes
  `Settings::for_e2e`'s fixed `settings.toml` there; in remote mode
  `cargo xtask vm deploy` has already staged the guest side, and the same
  `settings.toml` is written afresh next to the guest's Verbatim through
  the agent. `launch_with_settings` is `launch` with a scenario's own
  change applied to those fixed settings before they are written, such as
  "speak passwords" turned on; since every launch writes the settings
  afresh, in both modes, one scenario's settings never reach the next.
  Every run
  selects eSpeak NG, the default synthesizer, which is built with
  Verbatim and so needs nothing installed on the machine. A silent run,
  the default, sets `VERBATIM_TEST_AUDIO=null`, so Verbatim plays through
  the silent real-time device; an audible run (`AUDIBLE_ENV`, which
  `cargo xtask vm test` always sets) omits that variable, so Verbatim
  speaks through the real `WasapiDevice`. The two differ only in the
  device: the same synthesizer speaks the same audio, and every utterance
  takes its real duration either way. Before launching Verbatim it starts
  the scenario's recording (see `recording` below) when recording is
  enabled, and passes the recording's `VERBATIM_RECORD_AUDIO` to
  Verbatim's launch; ffmpeg failing to start is a warning, and the
  scenario runs unrecorded. `finish_recording(to)` ends and saves that
  recording (a warning, never a failure, when it cannot); the `Drop` impl
  stops a capture still running. `kill_processes_by_name` ends every
  process with a given image name through the agent, which is how a
  scenario kills the synthesizer host. It then
  launches Verbatim through the agent, waits for its
  control plane to answer over the agent's tunnel, opens a *second*,
  dedicated tunnel connection for speech collection, and waits until
  Verbatim's status reports it ready (`StatusInfo::ready`: the GUI can act
  on gestures, the focus listener is running, and the outpost reading
  Verbatim's own windows is ready) before returning. No scenario waits a
  fixed time: every wait is for a condition, with a deadline that only
  bounds a failure. `control()` and `speech()`
  expose the two connections; `send_gesture`, `send_keys`, `launch_target`,
  `open_document`, `open_folder`, `open_settings_page`, `kill_target`,
  `process_status`, `quit_verbatim`, and `report_latency` drive the
  running instance. `type_text` types through the agent's `TypeText`,
  recorded in the timeline as typed text. For a window of the scenario's
  own, such as a terminal's: `harness_marker(name)` is a title unique to
  the run (the same marker harness documents are named with);
  `run_directory` is the agent-side folder harness files go in, next to
  Verbatim's executable; `write_agent_file` writes a file there, and
  `wait_for_agent_file` waits for a file to appear and returns it, the
  evidence a script reached the point that writes it; `subscribe_events`
  opens one more control-plane connection subscribed to the normalized
  events Core receives, for evidence no speech shows (a caret report);
  `launch_titled`
  starts a program whose window carries that title and tracks it to be
  closed by the title at cleanup, terminated if it will not close only
  when the launch asked for that (never for `wt.exe`, whose window may
  belong to a Windows Terminal process the user's own windows share); and
  `bring_titled_window_forward` waits for the window, finds which program
  owns it from the agent's window list, and brings it to the foreground. `open_document` opens an application such as Notepad
  on an empty file whose name holds `DOCUMENT_MARKER`, brings the window
  with that title forward, and closes it by title at cleanup, so the
  user's own Notepad windows are never touched; `open_folder` does the
  same for a File Explorer window on a folder of empty files it writes,
  never sweeping `explorer.exe`, which is also the shell, and returns the
  window's title for the body (`ScenarioState::Title`);
  `open_document_with` does the same on a file holding given contents,
  for a scenario that reads or edits text; `save_document` saves such a
  document when its window is in front with unsaved changes and waits
  until its title no longer marks them, since Windows 11 Notepad restores
  an edited document left unsaved and then asks whether to keep the
  changes when the harness writes the file afresh; and `expect_unsaved`
  waits until the document's title marks unsaved changes, the evidence
  that an edit with nothing to hear, such as a paste, has reached it;
  `open_settings_page` opens a page of the Settings app by its
  `ms-settings:` URI, and the scenario lists `SystemSettings.exe` among
  its target images; `establish_baseline` makes sure an uncloaked window holds the
  foreground before every scenario's setup, and `foreground_report`
  describes the foreground for failure messages and the run's
  `foreground.txt`. A launched window that does not take the foreground
  fails setup with that report (`launch_target` also asks the agent to bring
  the launched application's window to the foreground, as a user's launch
  would, since Windows' foreground lock otherwise keeps it behind the window
  earlier scenarios typed into); `latency_snapshot` is the non-asserting,
  non-printing fetch the registry's run summary uses (see `registry`
  below), and `collect_run_artifacts` (timeline, stderr, and the per-process
  outpost, listener, and synthesizer host logs: every file in the launch's own log directory,
  `logs\<Verbatim's pid>`, listed through the agent, so an application that
  a launch handed off to, as Notepad does, is still collected) plus
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
- `SpeechCollector` — the speech assertions, reading the dedicated
  speech connection and never sending a request on it after subscribing,
  so no frame is discarded. Every utterance Verbatim queues arrives as a
  `Speech` frame and later ends with exactly one `SpeechEnded` frame
  (decision D17). Sounds are matched like words: a sound in the speech
  stream is named in its utterance's text (`sound: spelling-error`), and
  a sound played at once for an event arrives as a `Sound` frame, matched
  as an utterance of its own (`sound: exit`) that counts as heard at once. The `expect_*` assertions (`expect_in_order`,
  `expect_in_order_capturing`, `expect_change_capturing`,
  `expect_captured`) match queued text, then wait up to 30 seconds for
  the matched utterance's ending and fail unless it completed, so a
  passing assertion means the speech was heard in full and the next input
  cannot cut it off. Utterances queued while an assertion waits are kept
  for the next assertion. `expect_exactly` is `expect_in_order` with each
  utterance equal to its text rather than containing it, for speech as
  short as one character. `expect_playing` waits for an utterance
  containing a text to start playing and returns without waiting for its
  end, for a key pressed while it plays, and `has_played` says whether an
  utterance exactly equal to a text started playing, for showing that one
  never did. `wait_until_quiet(timeout)` waits until every
  utterance queued so far has ended, never for a stretch of silence, and
  panics with the timeline if that does not happen within `timeout`.
  `last_heard` gives the last utterance queued so far. For speech read as
  a sequence rather than matched, such as a terminal flood's:
  `take_heard(until, slice)` hands over the utterances queued during a
  short slice (those an assertion held back first) as `Heard` values,
  stopping just after one whose text is exactly `until`, so a scenario can
  check something else, such as the control plane's status, between
  reads; `take_until_quiet` is `wait_until_quiet` returning what it would
  have discarded; `ending_of` says how a `Heard` utterance ended, and
  `expect_completed` waits for one to be heard in full.
- `timeline` — the scenario's shared log of injected gestures, keys, and
  typed text and of speech: each utterance at queue time, its audio start, and its
  ending, rendered as `completed`, `cancelled`, or `failed` with the
  reason. Failure messages print it in time order.
- `registry` — the scenario registry itself. `ScenarioDef` is one named,
  grouped scenario: `name` (also its `#[test]` function name, its
  `cargo xtask vm test --scenario` selector, its artifacts directory name,
  and its video's file name, `<name>.mp4` — one identifier, everywhere),
  `group` (a `Group`: `Speech`, `Shell`, `Navigation`, or `Text`, a
  coarse `--group` selector, not a strict taxonomy — see
  the module's own doc comment for what each currently holds),
  `target_images` (image names its `setup` may launch with
  `launch_target`, unioned by `swept_target_image_names`; an application
  opened with `open_document` is closed by title instead and not listed),
  `settings` (an optional change to the fixed settings its Verbatim is
  launched with, through `Scenario::launch_with_settings`), and
  `setup`/`body`/`teardown`
  function pointers. `ScenarioState` is what `setup` hands the body and
  teardown: nothing, a target's pid, a window title, or a `Window` (the
  launch's pid, the window's title, and the folder of the files it uses),
  as the terminal scenarios open. `SCENARIOS` is the fixed, ordered list of every
  registered scenario, the suite. `DEMONSTRATIONS` lists the scenarios of
  something only some machines have, run only through `cargo xtask demo`:
  their `#[test]` wrappers are ignored, and `cargo xtask vm test` selects
  only from `SCENARIOS`, so the suite is the same everywhere. It holds
  `notepad_spelling_errors`. The Speech group holds
  `menu_and_settings_dialog`, `rapid_tabbing_in_settings`,
  `switch_to_onecore`, and `synth_host_crash_recovery`; the Shell group
  holds `notepad_and_verbatim_menu` and `start_menu_search`; the
  Navigation group holds `object_navigation_in_settings` and
  `system_information_tree`; the Text group (milestone M4) holds
  `notepad_editing`, `notepad_review_cursor`, `notepad_say_all`,
  `spelling_errors`, `windows_terminal_commands`, `conhost_commands`,
  `terminal_spoken_password`, `terminal_flood`, and
  `terminal_review_grid`. Each
  is implemented in
  `crates/verbatim-e2e/src/scenarios/`. `find` looks one up by name,
  among the scenarios and the demonstrations;
  `select` resolves `--scenario`/`--group` filters (both repeatable,
  unioned, deduplicated, registry order preserved; no filters means every
  scenario) into a list, erroring
  on any unrecognized name; `run_named` is the thin entry point
  every `#[test]` wrapper under `crates/verbatim-e2e/tests/` calls.
  `run_named`'s internal `run` launches, runs `setup` then `body` then
  `teardown`, waiting after `body` until speech is quiet
  (`wait_until_quiet` with a 30-second limit) so the last thing asserted is
  heard in full and nothing is still playing when teardown closes the
  scenario's applications — `body` and `teardown` each in their own
  `std::panic::catch_unwind`, so a panicking `body` still lets `teardown`
  run with whatever `setup` produced (borrowed, not moved, so the panic
  leaves it intact) rather than skipping cleanup — asserts a clean
  `quit_verbatim` only when both succeeded (an already-failed scenario's
  Verbatim is in an unknown state, and `Scenario::drop` kills it regardless,
  so nothing more is proved by also demanding a graceful quit), collects
  the run artifacts (timeline, stderr, and the reducer flight recorder — the
  last dumped before the quit while Verbatim is still up) for every run pass
  or fail, finishes the recording into `<name>.mp4` in the scenario's
  artifacts directory with `Scenario::finish_recording`, always writes a `ScenarioSummary`, then re-raises whatever panic
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
  parsing a subprocess's stdout. `archive_run` copies each finished run's
  directory into `history/<scenario>/<UTC time>-<pass or fail>` under the
  root and keeps the newest 100 runs of each scenario, since the scenario's
  own directory holds only the latest run. It leaves out `.mp4` files, so
  there is at most one video per scenario on disk.
- `recording` — a video of each scenario with Verbatim's speech (decision
  D16). `enabled()` is true unless `RECORD_ENV` (`VERBATIM_E2E_RECORD`) is
  `0` or `false`; `FFMPEG_ENV` (`VERBATIM_E2E_FFMPEG`) names ffmpeg on the
  agent's machine, `ffmpeg` on its `PATH` by default. `Recording::start`
  launches ffmpeg through the agent, since only a process in the
  interactive session can capture the desktop, recording it with
  `gdigrab` into fragmented MP4 in Verbatim's launch directory on the
  agent's machine, a fragment starting at each keyframe, one a
  second, with zero-latency encoding, so the file stays playable however
  ffmpeg is ended and loses at most the last second, after the scenario
  has ended. `audio_env` is the
  `VERBATIM_RECORD_AUDIO` variable naming the WAV file, in the same
  directory, that Verbatim's own `WavRecorder` writes everything it plays
  into, with its start time beside it (see the
  [verbatim-audio guide](verbatim-audio.md)). `finish` ends the capture
  with `TerminateProcess`, reads the wall-clock time of the first video
  frame from ffmpeg's log and the audio's start time, muxes the two on the
  agent's machine into one MP4 with AAC audio, the audio delayed or
  trimmed to line up with the video (video alone when the audio's start
  time is missing), and copies the result to this machine with
  `AgentClient::copy_file`. `stop` only ends the capture.
- `latency::fetch` — fetches the most recent `last_n` latency timelines with
  no printing and no assertion, the raw building block `report` (below) and
  `Scenario::latency_snapshot` both use.
- `latency::report` — `fetch`, then prints one fact per line and asserts at
  least one timeline reached audio, audible or not: every speech assertion
  waits for its utterance to be heard in full, so a scenario that asserted
  any speech has timelines that reached audio.

Implementation notes: `REMOTE_ENV` (`VERBATIM_E2E_REMOTE`) marks a run where
Verbatim lives in a guest rather than sharing this process's filesystem —
set by `xtask vm test`, not normally by hand — and skips the ordinary
host-filesystem steps (`verbatim.exe` existence check, staging) that
`xtask vm deploy` has already done inside the guest instead; the
`settings.toml` is written through the agent instead of to this machine's
disk. `crates/verbatim-e2e/tests/` holds one thin `#[test]` wrapper per
registered scenario (each just calling `registry::run_named` with its own
name) plus `session_info` (the agent reports an interactive session — a
precondition every scenario depends on, not itself a scenario, so it stays a
plain `#[test]` outside the registry). The thin wrappers are what keep
runner-direct CI (`.github/workflows/ci.yml`'s `e2e` job) and plain libtest
filtering (`cargo test -p verbatim-e2e <name> -- --exact`) working
unchanged: `cargo test -p verbatim-e2e -- --test-threads=1` still discovers
and runs every one of them exactly as before the restructuring.
`menu_and_settings_dialog` is the scripted walk of the M1 exit criteria that
`docs/roadmap-done.md`'s M2 section describes; it now asserts eSpeak NG's
Speech page, the default voice English (Great Britain), English
(Scotland) listed after it, and the variant Max. The others are
described in `registry`'s own `Group` doc comment above.
The Text group's three run in Notepad on a document of their own, start by
taking the caret to the top (Notepad can restore a caret position from an
earlier session), and save what they edited, in the teardown too.
`notepad_editing` moves the caret by character, word, and line, selects
and unselects with Shift, types with character echo, and deletes with
Backspace and Delete, each step's speech asserted exactly.
`notepad_review_cursor` reads by line, word, and character with the numpad
review keys, keeps column 8 down a text table through a shorter row,
reaches a line's ends and the text's top, and copies a range with
Verbatim+F9 and Verbatim+F10 pressed twice, checked by pasting it.
`notepad_say_all` reads with Verbatim+Down Arrow, presses Control while
the second line plays, and checks that the caret was left on that line and
that the third was never heard.
`spelling_errors` (milestone M4 item 7) opens `mockapp`'s scripted text,
a line with two misspelt words, marked with UIA's spelling error
annotation, and checks what the default theme reports as the caret moves,
each utterance exactly, the error sound's marker included: the line with
"spelling error" before each misspelt word, a character entering an error
and the next one inside it saying only itself, a word leaving an error
with "out of spelling error", and a line without errors saying nothing
about them. `mockapp` is staged and launched from the run's directory with
a fixture the scenario writes there, and moves its caret with the keys the
scenario presses. It stands in for a real control because none with
spelling errors Verbatim reads is on both Windows 11 and GitHub's Windows
Server runners, whose Notepad is classic Notepad, with no spell checker;
the scenario's doc comment gives the controls considered. The
`notepad_spelling_errors` demonstration walks the same steps in Windows 11
Notepad, whose spell checker marks a document a moment after it opens, so
its first line is read again with Control+Home, each reading heard in
full, until its errors are marked, within 30 seconds.

The terminal scenarios, also in the Text group, share a setup
(`scenarios/terminal.rs`). Each opens a window of its own titled with
`harness_marker`, and finds, brings forward, and closes it by that title,
never by class or program, so the user's own terminals are never touched,
and `WindowsTerminal.exe` and `conhost.exe` are never ended by name.
Windows Terminal opens with `wt.exe -w new --size 120,30 new-tab --title
<title> --suppressApplicationTitle`; the console host with `conhost.exe`
and the shell's command line (the agent starts it with no standard
handles, without which the console host opens a window of its own
whatever the default terminal setting), the shell setting its window to
120 by 30 with `mode con cols=120 lines=30` and its scrollback to 9,001
lines, Windows Terminal's default, since `mode con` leaves it at 30. A
scenario
that prefers Windows Terminal asks the agent to start `wt.exe` and, when
that fails because Windows Terminal is not installed, prints so and uses
the console host; nothing depends on the machine's name, so the same
scenarios hold on Windows 11 and on a Windows Server runner with only the
console host. The shell is Windows PowerShell, started with `-NoProfile
-NoLogo -NoExit -ExecutionPolicy Bypass -File start.ps1`; the start script
removes `PSReadLine`, moves to the scenario's folder, sets the title, and
sets the prompt `ready> `, which writes a file the first time it runs, the
evidence the shell waits for input; then it waits for a go file before the
shell shows its first prompt. The scripts a scenario runs are written into
that folder before the window opens. Every body starts with
`expect_prompt_read`: it hears the terminal's focus announcement out
(by then the outpost has read where the terminal's text ends), writes the
go file, hears the prompt "ready>" in full as new output, waits for Core
to receive the caret on the prompt (through an event subscription of its
own, `Scenario::subscribe_events`, since the console host reports its
caret some time after its text and the review cursor follows the caret),
and then reads the prompt line with numpad 8, hearing exactly "ready>".
Commands are typed with `type_text` and Enter pressed with `send_keys`;
`run_command_after_echo` presses Enter only once the echo of the
command's last word has been heard, so the terminal has shown the command
line and running it adds only the command's output (typing not yet shown
when Enter is pressed is dropped, and the line is then read as output).

`windows_terminal_commands` (in the console host, saying so, where Windows
Terminal is not installed) and `conhost_commands` type `echo hello`,
hearing each character exactly (the space as "space"), then exactly
"hello" and "ready>"; run a script calling `Read-Host -AsSecureString
"Password"`, hearing "Password:"; type `secret` and Enter, and assert that
nothing from the prompt up to "done" is exactly one of the typed
characters or contains "secret"; and hear exactly "done" and "ready>".
`terminal_spoken_password` does the same with "speak passwords" on
(`ScenarioDef::settings`), asserting instead that the six characters are
each spoken, exactly and in order.

`terminal_review_grid` prints a six-row table whose second column starts at
column 10, two rows being shorter than that, hears each row and the
prompt, reads the rows from the prompt line up to the first with numpad 7,
goes to the first row's column 10 with Shift+numpad 1 and numpad 6, and
then reads each next row with numpad 9 and its column 10 with numpad 2:
the cell's character, or "blank" on the shorter rows.

`terminal_flood` runs a script printing "flood line 1" to "flood line
10000", which times itself and writes the time to a file. The first flood,
with no key pressed, must speak its lines in increasing order, none twice;
the lines and the skipped-lines utterances heard in full must account for
every line exactly (between two lines heard, the skipped counts add up to
the gap, unless a skipped-lines utterance without a number says the count
was lost), with at least one skip; "flood line 10000" and then "ready>"
must be heard, and nothing about the flood after it; and the control plane
must answer a status request between reads throughout. During the second
flood, Verbatim+5 sent while a flood line plays must be answered with
"report new output off" within five seconds, the control plane answering
throughout, and nothing about the flood follows it. The third flood, with
output reporting off, must speak nothing but its command's echo; Verbatim+5
then says "report new output on", and `echo back` is answered with "back"
and "ready>". A fourth flood, with output reported and no key pressed, is
heard out to "ready>". The wall-time ratio, the fourth flood's time over
the third's by the script's own stopwatch, both into a full scrollback, is
how much reporting the output slows the terminal down; it is printed,
saved as `wall-time-ratio.txt` with the scenario's artifacts, and must be
under two, the M4 exit criterion. The first flood is not the measure: it
starts with an empty scrollback, and in the console host a flood that
fills the scrollback while it is read took about twice as long as one
into a full scrollback, output reported or not.
NVDA's report-title command, which the design names for the responsiveness
check, is not bound in Verbatim, so the check uses the Verbatim+5 toggle,
a reducer command with known text.
`synth_host_crash_recovery` opens the Verbatim menu, kills
`verbatim-synth-host.exe` with `kill_processes_by_name` (expecting
exactly one), and expects the next menu item to be heard in full from
the replacement host. `switch_to_onecore` chooses Windows OneCore voices
through the Speech page's Select Synthesizer dialog, expects the rebuilt
page's voice to be one of Microsoft's, switches back to eSpeak NG, and
cancels the settings dialog so the configuration is left as it was; it
needs OneCore voices installed, which Windows 11 and GitHub's Windows
runners have.
