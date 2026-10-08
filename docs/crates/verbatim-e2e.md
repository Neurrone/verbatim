# verbatim-e2e

The end-to-end suite, a scenario registry since milestone M3 Track B:
drives a real, running Verbatim (and target applications such as Notepad)
through `verbatim-agent` and, tunneled through it, Verbatim's own control
plane. Dev-only; a library rather than only test binaries because both
`crates/verbatim-e2e/tests/` and `xtask vm test` drive it. Every scenario
meets `docs/testing.md`. See `docs/tooling.md` for how to run it by hand
and how to read a failure.

## Running, skipping, and failing

- Every live `#[test]` wrapper under `crates/verbatim-e2e/tests/` is
  `#[ignore]`d with the reason "live", so a workspace test run (and
  `cargo xtask ci`) lists them as ignored rather than reporting them as
  passed. The end-to-end job runs them with
  `cargo test -p verbatim-e2e -- --ignored --skip demo_ --skip notepad_ --test-threads=1`.
- `endpoint()` reads `ENDPOINT_ENV` (`VERBATIM_E2E_ENDPOINT`). A live test
  run without it fails: a scenario that cannot reach an agent has not
  passed. `session_info`, the precondition check that the agent runs in an
  interactive session on the `Default` input desktop, is a plain ignored
  `#[test]` outside the registry and fails the same way.
- Local-only scenarios test Windows 11 Notepad, which GitHub's Windows
  Server runner lacks. Their names, and only theirs, start with
  `LOCAL_ONLY_PREFIX` (`notepad_`), so a run without Windows 11 Notepad
  deselects them with `--skip notepad_`. A run with `SKIP_LOCAL_ONLY_ENV`
  (`VERBATIM_E2E_SKIP_LOCAL_ONLY`) set to `1` that runs one anyway fails.
  `skips` validates the variable: unset, empty, `0`, or `1`, anything else
  an error. Nothing detects the machine.
- Demonstrations (the Demo group, `demo_` names) run only through
  `cargo xtask demo`.

## The starting state

`Scenario::launch_with` brings the desktop to the same state for
every scenario, recorded or not, and recording changes nothing else:

1. In runner-direct mode, builds `verbatim-app`, `verbatim-outpost`,
   `verbatim-synth-host`, and `mockapp` (unless `VERBATIM_E2E_VERBATIM_EXE`
   names a build to stage), copies them, eSpeak NG's data, and the default
   theme's sounds into `target/e2e-stage`, and writes
   `Settings::for_e2e`'s fixed settings, with the scenario's own change
   applied. In remote mode (`REMOTE_ENV`), `cargo xtask vm deploy` has
   staged the guest, and the settings are written through the agent.
2. Sweeps what an earlier run left: every process the agent launched that
   still runs is ended by its own handle (`EndLaunched`), Windows 11
   Notepad's harness tabs are closed as tabs, other windows titled with
   `DOCUMENT_MARKER` are closed, and harness files and folders in the run
   directory are deleted. Anything that will not go fails the launch.
3. Opens the scenario's Windows 11 Notepad document, when its definition
   names one (`ScenarioDef::document`, a `Document`), and waits on window
   events until it is in the foreground titled with the document; it
   fails if a Notepad window is already open, so the window and the
   process are the scenario's own. Opened before Verbatim starts, the
   window's first-showing rename from "Notepad" to the document cannot
   race Verbatim's announcement of it.
4. Minimizes every window, as the taskbar's Show Desktop does, waits until
   every window that can be minimized is (cloaked windows, which are not
   shown, aside), and brings the desktop, Program Manager, to the
   foreground, all through the agent and with no input injected.
5. Starts the recording, when recording, creates a named event through
   the agent, launches Verbatim with its name in `VERBATIM_READY_EVENT`,
   and waits for Verbatim to set it once it is ready for input; then opens the command connection and the speech
   connection and checks the status reports it ready.

Verbatim is launched with `RUST_LOG` set to `DEFAULT_RUST_LOG`
(`info,verbatim_outpost=debug`), which its outposts inherit, so a run's
artifacts hold the outposts' debug lines: what each read and when, what it
released, and what it reported. The focus listener runs in the same
binary (`verbatim_outpost::listener`) and is covered by the filter, but
has no log lines of its own yet. `RUST_LOG_ENV` (`VERBATIM_E2E_RUST_LOG`)
overrides the filter; set but empty, Verbatim logs as its own
configuration says.

Every scenario then starts by asserting the startup speech exactly
(`registry::startup_speech`): the start sound heard (Verbatim speaks no
start message, as NVDA speaks none), then the desktop's announcement, as
`Scenario::desktop_speech` gives it: "Program Manager", "Desktop list", and
the focused desktop item, read independently by the agent through UI
Automation, its name, "not selected" when it is not, and its position and
the item count. UI Automation gives no position for a Win32 list view's
items, so the agent counts the item's place among its parent's children.

## Speech assertions

`SpeechCollector` reads a dedicated speech connection. The first speech
subscriber receives the speech Verbatim queued before it subscribed, so
startup speech is asserted like any other. There is one kind of
assertion: the next utterances are exactly this list, each with its exact
text, in order, each ending as expected, and nothing else in between. An
empty utterance counts, and so does a sound played at once for an event,
which reads as `sound:` and its indication (`sound: exit`); a sound inside
an utterance is named in its text.

- `expect(texts)`: each heard in full, queued and ended within
  `STEP_TIMEOUT` (15 seconds, a bound on a hang).
- `expect_sequence(expected)`: the same, with each ending given:
  `heard(text)` or `cut_off(text)`, for a step whose speech is
  interrupted, such as the startup message.
- `expect_within(texts, timeout)`: `expect` with a longer bound, for a
  terminal flood.
- `expect_started(text)`: the next utterance is exactly `text` and its
  audio has started, for a step that then interrupts it;
  `expect_queued(texts)` matches utterances queued behind it without
  waiting for them; `expect_ended(heard, ending)` then asserts how each
  ended, `Ending::Completed` or `Ending::Cancelled`.
- `take_until(until, timeout)` and `ending_of` hand over utterances whose
  text cannot be known before the step, for the caller to assert on.
- `expect_nothing_more(after_input)`: ends every scenario. The collector
  sends `AwaitIdle` on its own connection with the number of the last key
  the scenario injected; Verbatim answers once it has handled that input
  and is idle, so everything it queued before then arrives first, and none
  of it may be unmatched. That is evidence, never a stretch of silence.
- `require_all_asserted(when)`: called before every injected input. It
  reads the frames already on their way, without waiting for anything to
  happen, and fails, as a harness error, if any utterance is waiting that
  no assertion matched. A scenario never moves past speech it has not
  asserted.

A mismatch prints the expected and actual sequences escaped (`{:?}`), the
index of the first utterance that differs and the first character in it
that differs, the utterances matched so far, and then the timeline.
`latency_rows` gives one row per utterance with the time of the event
behind it, which the run saves as `latency.csv`.

## Input

Every key and character the scenario injects goes through the agent
(`Scenario::send_keys`, `type_text`), which numbers it in `dwExtraInfo`
(see the [verbatim-input guide](verbatim-input.md)); Verbatim's keyboard
hook reads the number and Core reports it handled
(`Frame::InputHandled`) once every effect the key caused is queued, the
barrier `expect_nothing_more` waits on. `send_gesture` sends a gesture by
its identifier through the control plane. Nothing taps keys or uses
Alt+Tab to move the foreground; a window that does not take the
foreground fails the step. The agent can let a launched program take the
foreground only while it injected the last input, which it does as every
scenario runs (see `docs/tooling.md`).

## Scenario

`Scenario` owns one live run. Besides `control()`, `speech()`, `status()`,
and the input methods above:

- `launch_titled(command, args, title, owner_exits)` starts a program
  whose window carries `title` and waits for it to take the foreground;
  `launch_console(command, args, title)` starts a console program with its
  console window titled from its first frame (the launch's console title),
  which the console host scenarios use; `launch_target` refuses to run when
  a window so titled is already open; `launched_children` lists what the
  last launched program started.
- `bring_document_forward(name)` brings the Notepad document opened
  before launch to the foreground, as its taskbar button does;
  `save_document` and `expect_unsaved` wait on its title.
- `open_folder` opens a File Explorer window on a harness folder;
  `open_settings_page` opens a page of the Settings app.
- `subscribe_events` opens a connection subscribed to the normalized
  events Core receives, for evidence no speech shows: a caret report, a
  focus, the end of an outpost (`Frame::OutpostEnded`).
- `wait_for_agent_file` waits, on folder change notifications, for a file
  a script writes; `wait_for_window_in_front` and
  `wait_for_window_to_close` wait on window events.
- State read independently of Verbatim, before the scenario fixes what it
  expects: `desktop_speech`, `key_toggled` (a lock key),
  `focus_by_automation_id` (focuses an element with UI Automation's
  `SetFocus` and returns how many children it has), and `misspelt_words`
  (the words of the focused text marked with UI Automation's
  spelling-error annotation).
- `verbatim_children`, `end_verbatim_process(pid)` (ending one of
  Verbatim's own processes on purpose, which is then expected to exit),
  and `unexpected_exits`.
- `expect_exit_at_cleanup(title, pid)` adds a process, such as a
  terminal's shell, that must exit once the window closes.

### Cleanup

`clean_up` closes what the scenario opened, last first, and returns every
problem as a failure of the run. A window is closed by its title and,
when the scenario started its program, the process that owned it must
exit. Windows 11 Notepad is closed only by closing its harness tabs, never
by closing its window, since Notepad keeps the tabs of a closed window for
its next session; a Notepad process still running afterwards fails the run
with the windows it still has. A program with no window of its own is
ended by its process id. Nothing is ever ended by its image name. The
`Drop` impl quits or ends Verbatim and runs the cleanup when a panic
skipped it.

### Crashes

Any of Verbatim's own processes (Verbatim, an outpost, the focus
listener, a synthesizer host) that exits during the scenario without the
scenario having ended it on purpose fails the run, read from the exits
the agent records for Verbatim's job (`JobExits`). Verbatim must quit with
exit code 0. When crash dumps are configured
(`vm/scripts/Enable-VerbatimCrashDumps.ps1`), the dumps that appeared in
`CRASH_DUMP_FOLDER` during the run are copied into its artifacts.

## The run

`registry::run_named` is what every `#[test]` wrapper calls; its `run`
does, in order: launch; assert the startup speech and the desktop; run
setup and the body, which ends with `expect_nothing_more`; save the
latency timelines, Core's focus (`collect_focus`, `focus.txt`: the focus,
its ancestors, and the navigator, from the control plane's `DumpFocus`),
and the flight recorder while Verbatim is up; check for unexpected exits;
quit Verbatim; run the teardown; clean up; collect the artifacts; finish
the recording; write `foreground.txt`, the summary, and archive the run.
Every step after the body runs whatever happened before it, and every
problem is a failure reported together with the body's own panic.

## Artifacts

Kept for every run, pass or fail, in `artifacts::scenario_dir`
(`target/e2e-artifacts/<name>`, or under `VERBATIM_E2E_ARTIFACTS_DIR`):
`timeline.txt`, `latency.csv` (each utterance's step, text, event to
queue, and event to audio in milliseconds), `focus.txt`,
`flight-recorder.jsonl`, `foreground.txt`, `stderr.log`, every file of the
launch's log directory (the outposts' logs, the listener's, the
synthesizer host's), `verbatim-audio.wav` (everything Verbatim played),
new crash dumps, the video `<name>.mp4` when recording, and `summary.txt`.
Losing any of them fails the run. `archive_run` copies each run into
`history/<scenario>/<UTC time>-<pass or fail>`, keeping the newest 100,
without the video.

## Recording

A video of each scenario with Verbatim's speech (decision D16), unless
`RECORD_ENV` (`VERBATIM_E2E_RECORD`) is `0` or `false`. ffmpeg, named by
`FFMPEG_ENV`, captures the desktop on the agent's machine into fragmented
MP4; Verbatim always writes everything it plays into a WAV file
(`VERBATIM_RECORD_AUDIO`), recording or not, so recording changes nothing
about Verbatim's run; `finish` muxes the two and copies the result back.
A recording that cannot start or be finished is a failure.

## The registry

`ScenarioDef` is one named, grouped scenario: `name` (its `#[test]`
function, `--scenario` selector, artifacts directory, and video name),
`group`, `settings` (a change to the fixed settings), `local_only`, and
`setup`/`body`/`teardown`. `ScenarioState` is what setup hands the body
and teardown: nothing, a target's pid, a window title, names, or a
`Window` (the launch's pid, the window's title, how Verbatim announces
the terminal's text area, `text_area`, and the folder of its files).
`SCENARIOS` lists them in order; `find` and `select` look them up.

The groups:

- Speech: `menu_and_settings_dialog`, `rapid_tabbing_in_settings`,
  `switch_to_onecore` (to Microsoft David on OneCore, asserting the voice
  and the active synthesizer, and back), `synth_host_crash_recovery`
  (Verbatim's one synthesizer host ended by its process id, a new one
  after), `lock_key_announcements` (Scroll Lock's state read through the
  agent first), `settings_dialog_keys`, `theme_panel`, and
  `terminal_settings_page`.
- Shell: `second_application_and_verbatim_menu` (the Windows Forms text
  box as a second application, Verbatim's menu, and back, asserting the
  text box's outpost is the same process), `outpost_crash_recovery` (an
  application's outpost ended by its process id; once Core reports its end
  and is idle, nothing more was said, and the review cursor reads the text
  through a new outpost), `explorer_folder_window`, and
  `settings_system_page` (the list focused by its UI Automation
  identifier through the agent).
- Navigation: `object_navigation_in_settings` (over MSAA, in Verbatim's
  own dialog), `object_navigation_over_uia` (over UIA, in `mockapp`'s
  provider), and `system_information_tree`.
- Text: the editing scenarios, each in the Windows Forms text box and in
  Windows 11 Notepad, each its own code (`text_box_editing` and
  `notepad_editing`, `_review_cursor`, `_review_words`, `_typed_words`,
  `_word_selection`, `_say_all`), `spelling_errors` (`mockapp`'s scripted
  text), `notepad_spelling_errors`, and the terminal scenarios, each in
  Windows Terminal and in the console host. The Notepad scenarios are
  local-only.
- Demo: `demo_notepad_editing`, `demo_review_cursor`, `demo_say_all`,
  `demo_terminal_session`, `demo_settings_dialog_keys`. Everything a
  demonstration shows is covered by a test scenario.

### Shared setups

- `scenarios/text_box.rs`: the harness's Windows Forms text box, a window
  whose whole client area is one multi-line Win32 edit control, shown by
  Windows PowerShell from a script the scenario writes, the same on every
  machine. It does not wrap, its caret starts at the start of the text,
  and it is announced by the window's title, the box's name with "edit
  multi line", and the caret's line. The `text_box_` scenarios and
  `second_application_and_verbatim_menu` use it.
- Notepad: each scenario's document is opened before Verbatim starts
  (`ScenarioDef::document`) and brought forward by the scenario, which
  sets the caret where it starts with keys it asserts and saves what it
  edited. Typing is one
  character at a time, each echo heard in full before the next, since a
  character typed while an echo plays cuts it off. `notepad_spelling_errors`
  reads, through the agent, the words Notepad's spell checker marked once
  the document's opening has been announced, and fails unless they are
  exactly the two misspelt words: Notepad raises no event when it marks
  them.
- `scenarios/terminal.rs`: each terminal scenario opens its terminal,
  `open_windows_terminal` or `open_console_host`, and gets that one or
  fails, with no fallback. Windows Terminal opens with
  `wt.exe -w new --size 120,30 new-tab --title <title>
  --suppressApplicationTitle`, and its window must belong to
  `WindowsTerminal.exe`. The console host opens with `launch_console`, its
  window titled from its first frame; Windows reports a console window as
  its first client's, the shell, so the scenario asserts the window's
  class is `ConsoleWindowClass` and the shell is the launched console
  host's child. The shell writes its process id, which must exit at
  cleanup. Every body starts with `expect_prompt_read`, given the exact
  announcement: Windows Terminal names its text area with the tab's title
  ("<title> terminal"), and the console host's has no name ("terminal"). `type_hearing(text, echo)` types one
  character at a time: with `Echo::Shown`, the echo is what the terminal
  shows, and a space at the end of a line cannot be told from padding
  until something follows it, so a space is typed with the next character
  and both echoes heard together; with `Echo::Typed` ("speak passwords"
  on), each key is echoed as it is typed.

The flood scenarios assert the flood policy exactly within the
scrollback: 2,000 lines are heard as lines 1 to 30, "skipped 1941 lines"
after the skipped-lines sound, and lines 1,972 to 2,000 with the prompt.
The ratio of a reported flood's time to an unreported one's is saved as
`wall-time-ratio.txt` for trends, never asserted. Outpost logs are collected in chunks, since a flood's
debug log can be larger than one read of the agent's.

## Other modules

- `AgentClient`: a typed client for `verbatim_agent::protocol` (see the
  [verbatim-agent guide](verbatim-agent.md)). Every request that waits on
  the agent's side is read with a timeout ten seconds longer than its
  wait, set on the socket handle replies are read from, so a failure is a
  named timeout. `open_control_tunnel` relays Verbatim's control-plane
  pipe over the connection and hands back a ready
  `verbatim_control::client::Client`.
- `timeline`: injected gestures, keys, typed text, and speech (queued,
  audio started, ended), printed in time order on failure and saved.
- `artifacts`: `artifacts_root`, `scenario_dir`, `ScenarioSummary`, and
  `archive_run`, shared with `xtask vm test` without argument passing.
- `latency`: `fetch` and `report` read the control plane's latency
  timelines; budgets are reported, not enforced.
- `recording`: described above.

Implementation notes: a same-process `Mutex` enforces one live Verbatim
per test process, and `--test-threads=1` makes that sufficient.
`menu_and_settings_dialog` is the scripted walk of the M1 exit criteria.
