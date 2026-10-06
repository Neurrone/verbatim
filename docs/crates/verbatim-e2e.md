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
  and `verbatim-synth-host` (once per test binary, skipped when
  `VERBATIM_E2E_VERBATIM_EXE` names a build to stage instead), then
  copies `verbatim.exe`, `verbatim-outpost.exe`,
  `verbatim-synth-host.exe`, and eSpeak NG's `espeak-ng-data` directory
  into `target/e2e-stage` (staging fails if the data directory is
  missing, which building `verbatim-synth-host` creates) and writes
  `Settings::for_e2e`'s fixed `settings.toml` there; in remote mode
  `cargo xtask vm deploy` has already staged the guest side. Every run
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
  running instance. `open_document` opens an application such as Notepad
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
  `last_heard` gives the last utterance queued so far.
- `timeline` — the scenario's shared log of injected gestures and keys
  and of speech: each utterance at queue time, its audio start, and its
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
  and `setup`/`body`/`teardown`
  function pointers. `SCENARIOS` is the fixed, ordered list of every
  registered scenario. The Speech group holds
  `menu_and_settings_dialog`, `rapid_tabbing_in_settings`,
  `switch_to_onecore`, and `synth_host_crash_recovery`; the Shell group
  holds `notepad_and_verbatim_menu` and `start_menu_search`; the
  Navigation group holds `object_navigation_in_settings` and
  `system_information_tree`; the Text group (milestone M4) holds
  `notepad_editing`, `notepad_review_cursor`, and `notepad_say_all`. Each
  is implemented in
  `crates/verbatim-e2e/src/scenarios/`. `find` looks one up by name;
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
set by `xtask vm test`, not normally by hand — and skips the two ordinary
host-filesystem steps (`verbatim.exe` existence check, writing
`settings.toml`) that `xtask vm deploy` has already done inside the guest
instead. `crates/verbatim-e2e/tests/` holds one thin `#[test]` wrapper per
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
`synth_host_crash_recovery` opens the Verbatim menu, kills
`verbatim-synth-host.exe` with `kill_processes_by_name` (expecting
exactly one), and expects the next menu item to be heard in full from
the replacement host. `switch_to_onecore` chooses Windows OneCore voices
through the Speech page's Select Synthesizer dialog, expects the rebuilt
page's voice to be one of Microsoft's, switches back to eSpeak NG, and
cancels the settings dialog so the configuration is left as it was; it
needs OneCore voices installed, which Windows 11 and GitHub's Windows
runners have.
