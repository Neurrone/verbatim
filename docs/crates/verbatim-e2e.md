# verbatim-e2e

The end-to-end suite, a scenario registry: it drives a real, running
Verbatim (and target applications such as Notepad) through
`verbatim-agent` and, tunneled through it, Verbatim's own control plane.
Dev-only; a library rather than only a test binary because both
`crates/verbatim-e2e/tests/e2e.rs` and `xtask vm test` drive it. Every
scenario meets `docs/testing.md`. [The end-to-end tooling
guide](../tooling/e2e.md) says how to run it, lists the environment
variables, and says what a run leaves behind and how to read a failure;
this guide describes the code.

## Running, skipping, and failing

- The suite is one test binary, `crates/verbatim-e2e/tests/e2e.rs`, with
  a `#[test]` per scenario named after it, each calling
  `registry::run_named` with that name. One binary rather than one per
  scenario keeps the build small, since each test binary links the whole
  harness. A unit test in `registry.rs`, which `cargo xtask ci` runs,
  reads `tests/e2e.rs` as text and fails, naming each, when a registered
  scenario has no test there or a test there has no registered scenario.
- Every live `#[test]` is `#[ignore]`d with the reason "live", so a
  workspace test run (and `cargo xtask ci`) lists them as ignored rather
  than reporting them as passed.
- `endpoint()` reads `ENDPOINT_ENV` (`VERBATIM_E2E_ENDPOINT`). A live test
  run without it fails: a scenario that cannot reach an agent has not
  passed. `session_info`'s `agent_reports_an_interactive_window_station`,
  the precondition check that the agent runs in an interactive session on
  the `Default` input desktop, is a plain ignored `#[test]` outside the
  registry and fails the same way.
- Local-only scenarios' names, and only theirs, start with one of
  `LOCAL_ONLY_FILTERS` (`notepad_`, `settings_search_suggestions`, and
  `settings_combo_box_expanded`); the tooling guide's "Local-only
  scenarios" says why each is. A unit test checks that CI's `e2e` job
  skips exactly these, and that no other scenario's name contains one. A
  run with `SKIP_LOCAL_ONLY_ENV` (`VERBATIM_E2E_SKIP_LOCAL_ONLY`) set to `1`
  that runs one anyway fails. `skips` validates the variable: unset,
  empty, `0`, or `1`, anything else an error. Nothing detects the machine.
- Demonstrations (the Demo group, `demo_` names) run only through
  `cargo xtask demo`. The measure set (the Measure group, `measure_`
  names) is measurement, not tests: run only by name or with
  `--group measure`, and deselected from every suite run with
  `--skip measure_`.
- A same-process `Mutex` allows one live Verbatim per test process, and
  `--test-threads=1` makes that sufficient; the workspace's run lock,
  `target/e2e-run.lock`, keeps separate test processes apart.

## The starting state

`Scenario::launch_with` brings the desktop to the same state for every
scenario, recorded or not, and recording changes nothing else:

1. In a local run, builds `verbatim-app`, `verbatim-outpost`, `verbatim-listener`,
   `verbatim-synth-host`, and `mockapp` (unless `VERBATIM_E2E_VERBATIM_EXE`
   names a build to stage), copies them, eSpeak NG's data, and the default
   theme's sounds into `target/e2e-stage`, and writes
   `Settings::for_e2e`'s fixed settings, with the scenario's own change
   applied. In a remote run (`REMOTE_ENV`), `cargo xtask vm deploy` has
   staged the guest, and the settings are written through the agent.
2. Sweeps what an earlier run left: Windows 11 Notepad's harness tabs are
   closed as tabs first (a Notepad ended by its handle keeps its tabs for
   its next session, without their documents), then every process the
   agent launched that still runs is ended by its own handle
   (`EndLaunched`), the windows an abandoned scenario left are reported
   and closed by handle (`CloseAbandonedWindows`, under "Cleanup"), other
   windows titled with `DOCUMENT_MARKER` are closed, and harness files and
   folders in the run directory are deleted. Anything that will not go
   fails the launch. The agent then records every top-level window as the
   scenario's starting windows (`RecordWindows`), before the scenario
   opens anything.
3. Opens the scenario's Windows 11 Notepad document, when its definition
   names one (`ScenarioDef::document`, a `Document`), and waits on window
   events until it is in the foreground titled with the document; it
   fails if a Notepad window is already open, so the window and the
   process are the scenario's own. Opened before Verbatim starts, the
   window's first-showing rename from "Notepad" to the document cannot
   race Verbatim's announcement of it.
4. Minimizes every window, as the taskbar's Show Desktop does, and then
   directly each window still restored after it; waits until every window
   that can be minimized is (cloaked windows aside), and brings the
   desktop, Program Manager, to the foreground, all through the agent and
   with no input injected. It then fails, naming the window, if any shown
   window other than the desktop, owned windows such as a message box
   included, is not minimized, since such a window would take the
   foreground whenever the window in front closed (`minimize_failure`). A
   window with `WS_EX_NOACTIVATE`, such as the Widgets board's, never takes
   the foreground, so it passes.
5. Starts the recording, when recording, and waits for ffmpeg to capture
   its first frame; then creates a named event through the agent,
   launches Verbatim with its name in `VERBATIM_READY_EVENT`, and waits for
   Verbatim to set it once it is ready for input; then opens the command
   and speech connections and checks the status reports it ready.

Verbatim is launched with `RUST_LOG` set from `RUST_LOG_ENV`
(`VERBATIM_E2E_RUST_LOG`), or `DEFAULT_RUST_LOG` when it is unset; the
tooling guide lists the filter and what it logs.

Every scenario then starts by asserting the startup speech exactly
(`registry::startup_speech`): the start sound (Verbatim speaks no start
message, as NVDA speaks none), then the desktop's announcement, as
`Scenario::desktop_speech` gives it: "Desktop list" and the focused
desktop item, read independently by the agent through MSAA, its name,
"not selected" when it is not, and its position and the item count, from
the list view's own messages. When the agent cannot read the item's
position, `desktop_speech` returns an error rather than an expectation
without the item.

## Speech assertions

`SpeechCollector` reads a dedicated speech connection. The first speech
subscriber receives the speech Verbatim queued before it subscribed, so
startup speech is asserted like any other. Once subscribed, the
connection is split (`verbatim_control::client::Client::split`): a thread
of the collector's own reads every frame as it arrives, records it on the
timeline then, so its time is true even while the scenario waits on the
agent, and passes it on to the assertions, while `AwaitIdle` is sent on
the writing half.

Every utterance arrives as up to three frames: a queue-time `Speech`
frame with its text, a `SpeechStarted` frame when its first frame plays,
and exactly one `SpeechEnded` frame. Assertions match queue-time frames
only, then wait for the matched utterance's ending. A pause or resume of
speech (Shift) comes as a `SpeechPaused` frame, asserted on its own,
apart from the utterances, by `expect_paused` and `expect_resumed`. Every
Shift pressed with another key, such as Shift+Tab, pauses speech and the
key after it resumes it, as in NVDA, so a pause no step asserted is left
behind at the next input.

There is one kind of assertion: the next utterances are exactly this
list, each with its exact text, in order, each ending as expected, and
nothing else in between. An empty utterance counts, and so does a sound
played at once for an event, which reads as `sound:` and its indication
(`sound: exit`); a sound inside an utterance is named in its text.

- `expect(texts)`: each heard in full, queued and ended within
  `STEP_TIMEOUT` (15 seconds, a bound on a hang).
- `expect_sequence(expected)`: the same, with each ending given:
  `heard(text)` or `cut_off(text)`, for a step whose speech is
  interrupted. An item may instead be an optional utterance
  (`docs/testing.md`, "Optional utterances"): `optional(text)` accepts, in
  its place, `text` heard in full, cut off, or not said at all, and
  `optional_up_to(text, times)` the same up to `times` times in a row.
  Nothing else is accepted in its place. An optional utterance must be
  followed in the same sequence by an utterance of other text said once,
  whose arrival shows that it was not said; otherwise the assertion fails
  as a harness error. The matcher weighs every reading of the sequence,
  each optional utterance said a given number of times, against the
  speech as it arrives, and takes the first whose texts and endings all
  match, the fewest utterances first and, among as many, the earlier
  optional utterances said more times; a reading still short of speech
  waits for the next utterance, within the step's timeout. So
  `optional(t), heard(t), heard(u)` accepts `t u` and `t t u`, the first
  `t` of two heard in full or cut off, and
  `optional(t), optional(u), heard(t), heard(u)` accepts `t u` with both
  optional utterances absent, and `t u t u` when the first `t u` is not
  heard in full. What became of each optional utterance, an
  `OptionalOutcome` (its text, the input its step answers, and each
  time's ending, none when absent), is noted on the timeline and kept in
  `optional_outcomes()`, which the registry reports.
- `expect_within(texts, timeout)`: `expect` with a longer bound, for a
  terminal flood.
- `expect_started(text)`: the next utterance is exactly `text` and its
  audio has started, for a step that then interrupts it;
  `expect_queued(texts)` matches utterances queued behind it without
  waiting for them; `expect_ended(heard, ending)` then asserts how each
  ended, `Ending::Completed` or `Ending::Cancelled`. Where the step's
  action takes time of its own, such as bringing a window forward,
  `require_not_completed(heard, when)` first requires, once the action
  has completed, that the utterance had not been heard in full: one that
  had was not cut off because the action came too late, a harness error
  naming `when`, not Verbatim's failure.
- `take_until(until, timeout)` and `ending_of` hand over utterances whose
  text cannot be known before the step, for the caller to assert on.
- `expect_nothing_more(after_input)`: ends every scenario. The collector
  sends `AwaitIdle` with the number of the last key the scenario
  injected; Verbatim answers once it has handled that input and is idle,
  so everything it queued before then arrives first, and none of it may
  be unmatched. That is evidence, never a stretch of silence.
- `require_all_asserted(when)`: called before every injected input. It
  reads the frames already on their way, without waiting, and fails, as a
  harness error, if any utterance is waiting that no assertion matched.
  `require_asserted_before(when, expected)` is the same check before the
  input of a step whose speech is `expected`: the waiting utterances may
  be exactly that step's leading optional utterances, in order and each up
  to its declared times, which are then left for the step's assertion.
  `Scenario::send_keys_expecting(keys, expected)` makes that check, sends
  the keys, and asserts `expected` with `expect_sequence`, for a step
  whose leading optional utterance may arrive before or after its keys.
- `collect_only()` and `take_all_queued(after_input)`, for the measure set
  only: the registry sets collect-only mode for a Measure scenario once
  the startup speech and the desktop's announcement are asserted. From
  then on every utterance is still read and kept on the timeline, but
  `require_all_asserted` and `expect_nothing_more` collect unmatched
  utterances instead of failing, and `expect_nothing_more` no longer
  waits for each utterance to end. `take_all_queued` returns everything
  collected or unmatched once Verbatim is idle after the given input,
  with every frame that has reached the harness since, each counted as
  matched without its content being asserted. `take_collected()` returns
  only what collect-only mode has already collected, waiting for nothing.

A failed assertion unwinds with a `SpeechFailure`, without the panic
hook, and the run reports it once the logs are collected
(`SpeechFailure::report`): the expected and actual sequences, the failing
step's trace (`artifacts::trace_lines`), and the timeline. The failing
step's trace ID is the one the speech frame of the utterance the
assertion failed on carries; when it failed on none, an utterance never
queued, it is the newest heard in the step. `latency_rows` gives one row
per utterance with the time of the event behind it, which the run saves
as `latency.csv`; no latency budget is enforced (`docs/testing.md`).

## Compare mode

Compare mode runs a scenario's own setup and body against NVDA instead of
Verbatim, asserts nothing, and writes a report setting what the scenario
expects Verbatim to say beside what NVDA said, step by step. It is a
research instrument, not a test, and never runs in CI;
[the NVDA guide](../tooling/nvda.md), "Comparing a scenario", says how to
run it, how steps are formed, the report's contents and verdicts, and
which scenarios cannot be compared. `compare::ENV`
(`VERBATIM_E2E_COMPARE`), set to a scenario's name, selects it for that
scenario only: `registry::run_named` asks `compare::selects`, and a
scenario run while the variable names another fails at once as a harness
error naming both. `compare::QUIET_ENV` (`VERBATIM_E2E_QUIET_MS`) sets the
quiet period (`nvda::DEFAULT_QUIET`, one second, by default), raised to
`compare::QUIET_FLOOR`, 500 ms, NVDA's multi-press timeout, when it is
shorter (`compare::QuietPeriod`).

- The NVDA source and sink. `nvda::Transcript` is the add-on's client,
  shared with `cargo xtask nvda capture`: `session_port`, `connect` with
  its `Hello`, and `entries_after`, giving `TranscriptEntry`s.
  `nvda::speech_source` makes the frame source and request sink the
  collector reads a speech connection through, so the collector and the
  scenarios need nothing of NVDA's own. A reader thread polls the
  transcript every `POLL_INTERVAL` (50 ms). Each speech entry becomes
  `Frame::Speech` with an utterance id of its own, the flattened text, a
  trace ID minted for it, and its time on the harness's clock, followed
  at once by `Frame::SpeechEnded`, completed, since the add-on reports
  only what NVDA queued. A cancel, or any other event, goes on the
  timeline as `nvda cancel` and is no frame. The sink answers
  `SubscribeSpeech` at once and refuses every other request but
  `AwaitIdle`.
- The barrier. The sink answers `AwaitIdle` once NVDA has queued nothing
  for the quiet period since the request or its last entry, whichever is
  later (`nvda::QuietWait`), or at the request's timeout, with a note that
  NVDA's speech did not settle. A second `AwaitIdle` sent while one waits
  is an error.
- The collector (`SpeechCollector::compare_mode`). `expect`,
  `expect_sequence` and `expect_within` record their texts against the
  step under way, send `AwaitIdle` and read everything until its answer,
  so the scenario's next action does not run ahead of NVDA, and mark how
  many entries the timeline held then. `expect_started` and
  `expect_queued` record their texts and wait only until NVDA has queued
  speech in the step, since the next input interrupts it at once; the
  assertions on endings, pauses and resumes return at once, and
  `ending_of` answers `None`. `require_all_asserted` and
  `expect_nothing_more` close the step: they wait for `AwaitIdle` the same
  way unless the timeline holds no entry since that mark, and record a
  `compare::CompareStep` from the timeline's entries since the step before
  (`Timeline::step_since`) and the texts expected. A step that recorded
  speech the next input interrupts stays open, and is recorded with the
  next step's as one unit naming both inputs. `take_until`,
  `take_until_idle` and `take_all_queued` cannot keep their promises
  without Verbatim and fail as a harness error. An optional utterance is
  left out of the texts recorded. `take_comparison` hands the steps to
  the registry.
- The scenario. `Scenario::compare_with` does what `Scenario::launch_with`
  does up to opening the document, building and staging included, then
  connects to the transcript in the agent's session, before minimizing,
  so NVDA's announcement of the desktop is heard; it launches no Verbatim,
  records no video, and refuses a remote run. `is_comparing` says which
  mode a run is in. Each action that changes what is in front
  (`launch_titled`, `launch_titled_once`, `launch_owning_window`,
  `launch_console`, `bring_window_forward`, `bring_document_forward`,
  `open_folder`, `open_settings_page`, `run_handing_off`,
  `close_windows_titled`, `focus_by_automation_id`, and
  `terminal::expect_prompt_read`'s writing of the file that lets the shell
  show its first prompt) calls `Scenario::begin_action`, which in compare
  mode closes the step under way and starts one with the action as its
  input (`Timeline::push_action`); a normal run is unchanged by it.
  `send_gesture` presses a gesture's keys as in a normal run. What needs Verbatim itself (`status`,
  `subscribe_events`, `verbatim_children`, `end_verbatim_process`,
  `send_keys_from_verbatim`, and the checks after the body) is an error.
  A wait on one of Verbatim's events has no NVDA equivalent, so
  `terminal::expect_prompt_read`, `text_box::paste`,
  `terminal_flood::watch_for_prompt` and
  `terminal_flood::watch_for_prompt_output` skip theirs in compare mode,
  and the quiet period stands in.
- Declared differences. `Scenario::deviates_from_nvda(file, entry)`,
  called after a step's input, records that the step deliberately differs
  from NVDA, as the parity entry headed `entry` in `file` records, such as
  `"docs/parity/review.md"` and `"Review cursor columns"`. In a normal run
  it is a timeline note only. A unit test
  (`every_declared_deviation_names_an_entry_of_the_parity_ledger`) reads
  every call under `src/scenarios`, in the method and the function-call
  form, and checks the entry is a `###` heading of that file; it fails on
  a call whose file or entry is not a string literal, and does not see a
  call made through a wrapper.
- `ScenarioDef::comparable`: `Comparable::Yes`, or `Comparable::No` with
  why. Compare mode refuses the latter with the reason.
- The run (`registry::run_compare`). It hears the desktop as an ordinary
  step, runs setup, the body and the end of the speech, the teardown and
  the cleanup, writes the timeline, the report, the foreground record and
  the summary, and archives the run; none of the checks on Verbatim after
  the body runs. It passes unless the harness itself fails.
- The report (`compare::render_report`, `compare.txt` in the run's
  artifacts, also printed). The header's pin check is
  `compare::submodule_pin`, comparing with the commit an alpha's version
  ends with or a release's tag. A step's verdict is
  `CompareStep::verdict`, judged on `CompareStep::compared`;
  `CompareStep::new` gives `None` only for a step with no input and
  nothing in it, and `CompareStep::start` marks the run's start, which is
  not judged.

## Input

Every key and character the scenario injects goes through the agent
(`Scenario::send_keys`, `type_text`), which numbers it in `dwExtraInfo`
([verbatim-input](verbatim-input.md)); Verbatim's keyboard hook reads the
number and Core reports it handled (`Frame::InputHandled`) once every
effect the key caused is queued, the barrier `expect_nothing_more` waits
on. `hold_key` and `release_key` hold one key, such as Control, down
across the keys sent between them, through the agent's `PressKey` and
`ReleaseKey`; a key still held when the body ends is released then and
fails the run (`release_held_keys`, run again by the cleanup).
`send_gesture` presses a keyboard gesture's keys through the agent, as
`send_keys` does, the Verbatim modifier as Insert (`kb:verbatim+v` as
`insert+v`, `verbatim_control::send_keys::gesture_keys`), so the
keyboard hook decides and numbers them as a person's keys and the waits
after it are ordered by its number. Pressed again within the
multi-press timeout (500 ms) with no other key between, the same gesture
counts as a second press, as a person's does; a step meant as a first
press follows a different gesture or key, as `review_cursor` does before
Verbatim+F10. For the measure set, `send_keys_paced(keys, interval_ms)` and
`type_text_paced(text, interval_ms)` inject one stroke or character every
`interval_ms` through the agent, numbered like any other input, and
return the first and last numbers; `repeat_key(key, delay_ms,
interval_ms, repeats)` holds a key down with auto-repeat and returns the
press's and the release's numbers; `take_all_queued_within(bound)` is
`take_all_queued` with a longer bound on Verbatim becoming idle; and
`send_keys_from_verbatim` injects keys through Verbatim's own control
plane (`Request::SendKeys`), which carry no harness number.

Nothing taps keys or uses Alt+Tab to move the foreground, not even in a
scenario about switching windows: the window is brought forward through
the agent, as its taskbar button does, and a window that does not take the
foreground fails the step. The agent can let a launched program take the
foreground only while it injected the last input, which it does as every
scenario runs (`docs/tooling/troubleshooting.md`).

## Scenario

`Scenario` owns one live run. Besides `speech()`, `status()`, and the
input methods above:

- `launch_titled(command, args, title, owner_exits)` starts a program
  whose window carries `title`, its first window minimized and inactive,
  then restores that window and sets it as the foreground, as its taskbar
  button does, and waits for it to take the foreground, so the launch does
  not depend on the agent having injected the last input
  (`docs/tooling/troubleshooting.md`, "Windows' foreground lock keeps
  launched applications behind"); a window that ignores the minimized show
  state, as msinfo32's does, is minimized by the agent before it is
  restored. `launch_titled_once(command, args, title, owner_exits, ready)`
  does the same, bringing the window forward only once the agent sees the
  file `ready`, the program's evidence that it has written what it writes
  as it starts. `launch_console(command, args, title, ready)` starts a
  console program with its console window titled from its first frame,
  opened minimized and inactive, without the agent's foreground right, and
  brought forward as `launch_titled_once` does. `launch_owning_window`
  does the same for a program that must own the window it opens.
  `require_absent(title)` fails when a window so titled is already open,
  for a program whose title is not the run's own, such as msinfo32;
  `launched_children` lists what the last launched program started.
- `bring_window_forward(marker)` brings a window the scenario opened,
  found by the marker in its title, to the foreground, as its taskbar
  button does and as NVDA's system tests do with `SetForegroundWindow`,
  and waits on window events until it is in front; the suite switches
  windows only this way. `bring_document_forward(name)` does the same for
  the Notepad document opened before launch; `save_document` and
  `expect_unsaved` wait on its title.
- `open_folder` opens a File Explorer window on a harness folder,
  launched without the agent's foreground right and brought forward like
  every other launch; `open_settings_page` opens a page of the Settings
  app.
- `subscribe_events` opens a connection subscribed to the normalized
  events Core receives, for evidence no speech shows: a caret report, a
  focus, the end of an outpost (`Frame::OutpostEnded`).
- `wait_for_agent_file` waits, on folder change notifications, for a file
  a script writes; `wait_for_window_in_front` waits on window events.
- State read independently of Verbatim, before the scenario fixes what it
  expects: `desktop_speech`, `key_toggled` (a lock key),
  `focus_by_automation_id` (focuses an element with UI Automation's
  `SetFocus` and returns how many children it has), `controlled_items`
  (of the first element the focused element controls, the set size its
  first child reports and the names of the first children),
  `name_by_automation_id` (the name of the foreground window's element
  with that identifier, read without focusing it), and `misspelt_words`
  (the words of the focused text marked with UI Automation's
  spelling-error annotation).
- `verbatim_children`, `end_verbatim_process(pid)` (ending one of
  Verbatim's own processes on purpose, which is then expected to exit),
  and `unexpected_exits`.
- `expect_exit_at_cleanup(title, pid)` adds a process, such as a
  terminal's shell, that must exit once the window closes.
- `close_windows_titled` closes every window whose title contains the
  text, failing unless all closed.

### Cleanup

`clean_up` closes what the scenario opened, last first, and returns every
problem as a failure of the run. A window is closed by its title and,
when the scenario started its program, the process that owned it must
exit. A File Explorer window (`open_folder`) is closed by its handle
instead, recorded as it opened, since File Explorer retitles it as it
navigates; it is closed, and waited for until it has gone, before the
harness folders are deleted. Windows 11 Notepad is closed only by closing
its harness tabs, never its window, since Notepad keeps the tabs of a
closed window for its next session; a Notepad process still running
afterwards fails the run. A program with no window of its own is ended by
its process id. Nothing is ever ended by its image name. The `Drop` impl
quits or ends Verbatim and runs the cleanup when a panic skipped it.

Closing by title cannot prove that nothing was left, since a window can
lose the title it was found by, so the cleanup's last step asks the agent
to wait, on window events and up to the close timeout, for every shown
window that was not among the scenario's starting windows to be gone
(`WaitForNewWindowsGone`). Each window still open fails the run, named by
its title, class, process id, and program, and is left open. Windows are
identified by handle, owning process, and class, never by title; a window
counts as gone once it is closed, hidden, or cloaked, as the Settings app
keeps its closed window.

The agent keeps the starting windows with the connection that recorded
them. When that connection ends before the check has found nothing left,
because the test process was killed or the check failed, the scenario is
abandoned, and the agent notes the windows it had open then. The next
scenario's start prints each of them still open and closes it by handle,
failing only if one will not close. Only windows absent from the
abandoned scenario's starting windows and open when its connection ended
are closed, so the owner's own windows are safe. The title sweep at the
start (`DOCUMENT_MARKER`) still closes a harness window left by a run
whose starting windows the agent no longer holds, because the agent was
restarted.

### Crashes

Any of Verbatim's own processes (Verbatim, an outpost, the focus
listener, a synthesizer host) that exits during the scenario without the
scenario having ended it on purpose fails the run, read from the exits
the agent records for Verbatim's job (`JobExits`). Verbatim must quit with
exit code 0 and leave none of its processes behind: once it has exited,
the harness waits for its job to empty (`WaitForJobEmpty`), and every
outpost and the focus listener but one the scenario ended on purpose must
have exited with code 0 before Verbatim did; one the supervisor had to
kill exits with `verbatim_process::KILLED_EXIT_CODE` and fails the run
(`Scenario::outposts_shut_down_cleanly`; [verbatim-outpost](verbatim-outpost.md),
"Shutting down"). When crash dumps are configured
(`vm/scripts/Enable-VerbatimCrashDumps.ps1`), the dumps that appeared in
`CRASH_DUMP_FOLDER` during the run are copied into its artifacts, in
chunks, since an outpost's dump is a full-memory dump of a few hundred
megabytes.

Once the artifacts are collected, `Scenario::outpost_crashes` reads the
job's exits again and returns an `artifacts::OutpostCrash` for each
outpost or focus listener the scenario did not end on purpose whose exit
Windows called abnormal or whose exit code is an exception code (an
`NTSTATUS` of `0xC0000000` or above): its pid, its exit code, and the path
of its dump in the artifacts, when one was written. The run records them
in `ScenarioSummary::outpost_crashes` whether or not its steps passed.
After every scenario the run writes one line to the stderr handle itself,
which the test harness does not capture: that scenario's crashes, and the
running total of crashes and scenarios in this test process, so the last
scenario's line holds the suite's total. Optional utterances are reported
the same way, one line each, such as `scenario "explorer_folder_window":
optional utterance "Working on it..." after keys enter: cut off`, and
recorded in `ScenarioSummary::optional_utterances`, which
`cargo xtask vm test` prints in its run summary.

### The user's Windows Terminal

Nothing a run does may reach the owner's own Windows Terminal; the
tooling guide's "Keeping your Windows Terminal safe" gives the rules and
why. In code: every console program the harness runs itself (cargo,
`curl.exe`, `tar.exe`) is started with `CREATE_NO_WINDOW`; the agent
watches for Windows Terminal windows from the start of each run
(`TakeForeignTerminalWindows`), and any window of a Windows Terminal
process the agent did not launch fails the run
(`Scenario::foreign_terminal_windows`); Verbatim is launched with
`AgentClient::launch_verbatim`, which has the agent name every such
Windows Terminal, and the console hosts it runs, for Verbatim to ignore
(`VERBATIM_IGNORE_PIDS`); and the run fails if Verbatim's log folder holds
an outpost log named for one of them (`Scenario::collect_run_artifacts`).

## The run

`registry::run_named` is what every `#[test]` wrapper calls; its `run`
does, in order: launch; assert the startup speech and the desktop; run
setup and the body, which ends with `expect_nothing_more`; save Core's
focus (`collect_focus`, `focus.txt`, from the control plane's `DumpFocus`)
and the flight recorder (`collect_flight_recorder`) while Verbatim is up;
check for unexpected exits; quit Verbatim and check that its outposts and
listener shut down cleanly; take the speech Verbatim sent: after a clean
quit, every frame until the speech connection ends (`take_until_the_end`,
bounded by the step timeout, its end not coming a problem of the run),
since the agent shuts the connection down when Verbatim's pipe closes;
after a failed quit, only what has arrived (`take_remaining`); run the
teardown; clean up; collect the artifacts (`collect_run_artifacts`);
finish the recording; check that no Windows Terminal window the run did
not open appeared; write `foreground.txt` and the summary, and archive
the run. The summary's latency lines (`ScenarioSummary::new`) are built
from the collector's `latency_rows`. Every step after the body runs
whatever happened before it, and every problem is a failure reported
together with the body's own panic.

## Artifacts

`artifacts` holds `artifacts_root`, `scenario_dir`
(`target/e2e-artifacts/<name>`, or under `VERBATIM_E2E_ARTIFACTS_DIR`),
`ScenarioSummary` (with its `OutpostCrash` list, and `read` for
`xtask vm test`), `archive_run`, which copies each run into
`history/<scenario>/<UTC time>-<pass or fail>`, keeping the newest 100,
without the video, and `trace_lines`, every line carrying a trace ID in a
run's collected logs and flight recorder, in time order. `xtask vm test`
calls the same functions to find and read what a scenario subprocess
left, rather than parsing its output. The files a run keeps are listed in
the tooling guide's "What a run leaves behind"; losing any of them fails
the run.

## Recording

`recording` records a video of each scenario with Verbatim's speech
(decision D16), unless `RECORD_ENV` (`VERBATIM_E2E_RECORD`) is `0` or
`false`; `FFMPEG_ENV` names ffmpeg and `QUALITY_ENV` selects the demo
quality. The tooling guide's "Hearing and recording a run" describes the
capture, its stop, the mux and the check. In code: `start` launches the
agent's desktop capture through the agent (the command
`verbatim_agent::protocol::AGENT_SELF` with `capture-desktop`,
[verbatim-agent](verbatim-agent.md)), which feeds ffmpeg with
`capture_args`, and returns once ffmpeg has created its output file,
within `FIRST_FRAME_TIMEOUT` (30 seconds). `stop` writes `q` to the
capture's standard input (`WriteStdin`) and waits for it to exit with
code 0 within `STOP_TIMEOUT` (15 seconds); the capture is killed only when
that bound passes, and that is a failure. A scenario that panicked is
stopped the same way, by `Scenario`'s `Drop`. `finish` takes the
wall-clock time the scenario's body ended, which `registry::run` records,
stops the capture, muxes it with the audio, copies the result back with
ffmpeg's logs, and checks it (`verify`). `audio_path` and `audio_env` name
the WAV file Verbatim records its audio into (`VERBATIM_RECORD_AUDIO`) on
every run, recording or not, so recording changes nothing about
Verbatim's run.

## The registry

`ScenarioDef` is one named, grouped scenario: `name` (its `#[test]`
function, `--scenario` selector, artifacts directory, and video name),
`group`, `settings` (a change to the fixed settings), `local_only`,
`comparable`, `document`, and `setup`, `body` and `teardown`.
`ScenarioState` is what setup hands the body and teardown: nothing, a
target's pid, a window title, names, or a `Window` (the launch's pid, the
window's title, how Verbatim announces the terminal's text area,
`text_area`, and the folder of its files). `SCENARIOS` lists them in
order; `find` and `select` look them up. Groups are a coarse selector for
`cargo xtask vm test --group`, not a strict taxonomy. Each scenario module
under `src/scenarios/` documents exactly what it asserts.

- Speech: `menu_and_settings_dialog` (the walk through Verbatim's menu
  and Speech settings dialog, every item and control announcing its name,
  role, value, state and shortcut, eSpeak NG's voices and its Max
  variant), `rapid_tabbing_in_settings` (a burst of Tab and Shift+Tab
  presses must leave focus and the navigator on the control that has
  focus), `switch_to_onecore` (to Microsoft David on OneCore through the
  Select Synthesizer dialog, asserting the voice and the active
  synthesizer, and back), `synth_host_crash_recovery` (the synthesizer
  host ended by its process id, and the next announcement heard in full
  from the host Verbatim starts in its place), `lock_key_announcements`
  (Scroll Lock's state read through the agent first, then announced on
  and off), `settings_dialog_keys`, `theme_panel` (a role changed to a
  sound is then heard as that sound), and `terminal_settings_page` (its
  applied change found in Core with Verbatim+5).
- Shell: `second_application_and_verbatim_menu` (the Windows Forms text
  box as a second application, Verbatim's menu, and back, the text box's
  outpost the same process), `outpost_crash_recovery` (an application's
  outpost ended by its process id; once Core reports its end and is idle,
  nothing more was said, and the review cursor reads the text through a
  new outpost), `return_after_retirement` (a text box brought forward
  again once Core reports its outpost retired after two minutes without
  attention, announced as before through a new outpost; about three
  minutes), `quick_link_menu` (the Windows+X menu opened from the desktop
  and closed with Escape), `explorer_folder_window` (a File Explorer
  window on a harness folder, with File Explorer's "Working on it..." an
  optional utterance on the window's opening and after Enter and
  Backspace), `settings_system_page` (the list focused by its UI
  Automation identifier through the agent), `settings_combo_box_expanded`
  (a Date and time combo box opened with Alt+Down Arrow, "expanded"
  before its first item, and closed with Escape), and
  `settings_search_suggestions` and
  `settings_search_suggestions_reentered` (the suggestions read as the
  selection moves through them while the focus stays in the search box,
  their number and names read by the agent first; and the box entered
  again by Shift+Tab while it holds text).
- Navigation: `object_navigation_in_settings` (over MSAA, in Verbatim's
  own dialog), `object_navigation_over_uia` (over UIA, in `mockapp`'s
  provider), and `system_information_tree` (msinfo32's real Win32 tree
  view over MSAA, logical object navigation through it).
- Text: editing, selection keys, review cursor, review words, typed word
  echo, word selection and say-all, each in the Windows Forms text box
  (`text_box_`) and in Windows 11 Notepad (`notepad_`), each its own code;
  the protected fields `text_box_password` (a Windows Forms password text
  box) and `wpf_password` (a WPF `PasswordBox`, through UI Automation), in
  `scenarios/password.rs`; `spelling_errors` (`mockapp`'s scripted text)
  and `notepad_spelling_errors`; and the terminal scenarios, each in
  Windows Terminal (`windows_terminal_`) and in the console host
  (`conhost_`), each its own code (below). The Notepad scenarios are
  local-only.
- Demo: `demo_notepad_editing`, `demo_review_cursor`, `demo_say_all`,
  `demo_terminal_session`, `demo_settings_dialog_keys`. Everything a
  demonstration shows is covered by a test scenario.
- Measure: the measure set in `scenarios/measure.rs`, workloads for
  Verbatim's latency ledger that assert no speech after the desktop's and
  write each step's ground truth to `measure-truth.jsonl`; the tooling
  guide's "The measure set" describes each and how to run them.
  `measure_focus_explorer` also waits for exact announcements and fails
  with a named timeout when one is not queued.

### Shared setups

- `scenarios/text_box.rs`: the harness's Windows Forms text box, a window
  whose whole client area is one multi-line Win32 edit control, shown by
  Windows PowerShell from a script the scenario writes. It does not wrap,
  its caret starts at the start of the text, and it is announced by the
  window's title, the box's name with "edit multi line", and the caret's
  line.
- Notepad: each scenario's document is opened before Verbatim starts and
  brought forward by the scenario, which sets the caret with keys it
  asserts and saves what it edited. Typing is one character at a time,
  each echo heard in full before the next, since a character typed while
  an echo plays cuts it off. `notepad_spelling_errors` reads, through the
  agent, the words Notepad's spell checker marked once the document's
  opening has been announced, and fails unless they are exactly the two
  misspelt words, since Notepad raises no event when it marks them.
- `scenarios/terminal.rs`: each terminal scenario opens its terminal,
  `open_windows_terminal` or `open_console_host`, and gets that one or
  fails, with no fallback. Windows Terminal is the harness's portable copy
  (`windows_terminal`, below), its settings folder deleted, its
  `WindowsTerminal.exe` started directly, never `wt.exe`, with
  `-w new --size 120,30 new-tab --title <title> --suppressApplicationTitle`,
  through `Scenario::launch_owning_window`, which fails unless the
  launched process owns the window. Each window is brought forward only
  once its shell has written its process id. Every shell, in both
  terminals, runs with `-NonInteractive`: with Verbatim running, Windows'
  screen reader flag is set, and Windows PowerShell's interactive host
  then prints a notice that it leaves `PSReadLine` out, which Windows
  Terminal can show after its window is announced. The password prompt the
  commands scenarios use is drawn by their script, since `Read-Host` fails
  in a shell that is not interactive. The scenario also fails if any other
  Windows Terminal process opened a window meanwhile
  (`Scenario::top_level_windows`, before and after), and the copy's
  process must exit once its window closes. The console host opens with
  `launch_console`; Windows reports a console window as its first
  client's, the shell, so the scenario asserts the window's class is
  `ConsoleWindowClass` and the shell is the launched console host's
  child. The shell writes its process id, which must exit at cleanup.
  Every body starts with `expect_prompt_read`, given the exact
  announcement: Windows Terminal names its text area with the tab's title
  ("<title> terminal"), and the console host's has no name ("terminal").
  A Windows Terminal window's body uses
  `expect_windows_terminal_prompt_read(state, repeats)`, with
  "<title> terminal" also an optional utterance up to `repeats` times
  before the one heard in full (`LAUNCH_REPEATS`, 1); a second window in
  the running Windows Terminal uses
  `expect_second_windows_terminal_prompt_read(state)`, which also accepts
  the repeats after it, each with or without its line, up to
  `SECOND_WINDOW_REPEATS`, 2. `type_hearing(text, echo)` types one
  character at a time: with `Echo::Shown`, the echo is what the terminal
  shows, and a space at the end of a line cannot be told from padding
  until something follows it, so a space is typed with the next character
  and both echoes heard together; with `Echo::Typed` ("speak passwords"
  on), each key is echoed as it is typed. Scripts wait for the scenario's
  files through `FILE_SIGNALS`, on change notifications.

### The terminal scenarios

Each exists in both terminals unless named for one.

- `_commands`: commands, typed echo, and a password prompt;
  `_spoken_password`: its typing spoken with "speak passwords" on;
  `_editing`: a command corrected with Backspace, punctuation echoed by
  name, and the review cursor's current word down a column;
  `_review_grid`: the review cursor down a column of a text table;
  `_progress`: a line rewritten in place, spoken whole the first time and
  then by the word that changed; `_short_output`: one- and two-character
  lines spoken like any other.
- Typing (`scenarios/terminal_typing.rs`): a password typed into a script
  that never shows it and rewrites its prompt's line after each key,
  hearing only the count that changed; a character typed in the middle of
  a command, echoed; Escape on a typed command, hearing the text it
  removed.
- Screens (`scenarios/terminal_screens.rs`): a line of forty words heard
  whole; one line grown by ten words a key, hearing each key's words; and
  a full-screen program on the alternate screen, its rows heard, changes
  near the top and bottom, scrolling, and the screen closed.
- Lists and pagers (`scenarios/terminal_lists.rs`,
  `scenarios/terminal_pager.rs`): a selection list whose marker moves with
  the caret, or is redrawn whole on each move, each move said once; a
  pager on the alternate screen moved down a line and back, each move
  saying only the line it brought on.
- Keys during output (`scenarios/terminal_key_timing.rs`): Control cuts
  off everything still queued once the flood's last line is queued, and
  lines written after it are spoken (`*_control_flood`); Shift twice
  pauses and resumes with nothing lost (`*_shift_flood`); Up Arrow during
  a flood's speech cuts it and says the recalled line (`*_line_key_flood`);
  Up Arrow with a character typed at once says the recalled line with the
  character, once (`*_up_typing`).
- Floods: `_flood`, 2,000 lines heard as lines 1 to 30, "skipped 1941
  lines" after the skipped-lines sound, and lines 1,972 to 2,000 with the
  prompt, with the ratio of its time to an unreported flood's saved as
  `wall-time-ratio.txt`, never asserted; `scenarios/terminal_flood_kinds.rs`
  for identical lines (`*_same_flood`), raised limits (`*_raised_flood`)
  and a full-screen redraw past lowered limits (`*_redraw_limit`). The
  acceptance floods (`scenarios/terminal_overflow.rs`, `_history_flood`
  and `_scrollback_overflow`) print 30 lines, wait for a file, then write
  5,000 or 12,000 lines in one write, within the history or past it,
  heard as their first 30, one exact "skipped 4941 lines" or "skipped
  more than N lines", and the last 30, N being the terminal's history less
  the 30 spoken (9,001 in Windows Terminal, 8,971 in the console host);
  their `_during_group` twins write the second part while the first 30
  still play, so it joins their burst. Further flood variants
  (`*_full_history_flood`, `conhost_wrapped_flood`, `conhost_fresh_flood`,
  `windows_terminal_clear_flood`, `windows_terminal_cleared_flood`) are
  documented in their modules. Outpost logs are collected in
  chunks, since a flood's debug log can be larger than one read of the
  agent's.
- Windows and tabs (`scenarios/terminal_windows.rs`): `_two_windows`
  switches between two windows as a taskbar button does and types in each;
  `windows_terminal_tabs` opens a second tab from the command line
  (`-w 0 new-tab`), switches with Control+Shift+Tab and Control+Tab, and
  types in each; `windows_terminal_close_tab` closes the second tab;
  `*_leave_flood` leaves for a second window during a flood whose first
  line is still playing when the switch lands (a slow switch fails as a
  harness error), the rest finished while away and not spoken. A second
  Windows Terminal window or tab is handed to the harness's running copy,
  which must own it; the tabs scenario turns `confirmCloseAllTabs` off.
- Review (`scenarios/terminal_review_output.rs`,
  `scenarios/terminal_review_padding.rs`): the review cursor follows the
  caret to the prompt after output; it stops at a short row's last
  character, padding never being reviewed.
- Footers (`scenarios/terminal_footer.rs`,
  `scenarios/terminal_status_bar.rs`): a flood longer than the console
  host's history in a scroll region above a footer; three lines written
  above a footer, read with Verbatim+End before and after it changes. No
  scenario floods above a footer that a read must find while output
  arrives, because such a read can leave the footer out
  (`docs/design/terminal-model.md`, "Limits").

## Other modules

- `agent_client`: `AgentClient`, a typed client for
  `verbatim_agent::protocol` ([verbatim-agent](verbatim-agent.md)), past
  its `Hello` handshake (`connect`). Its methods map one to one onto the
  agent's requests:
  - Processes: `launch_process`, `launch_with_stdin` and `write_stdin`,
    `launch_verbatim` (with the foreign terminals named for Verbatim to
    ignore), `launch_minimized`, `launch_without_foreground_right`,
    `launch_console`, each returning a `Launched` (the pid, and whether
    Windows let the agent allow it the foreground); `kill_process`,
    `end_launched`, `child_processes`, `job_exits`, `wait_for_exit`,
    `wait_for_job_empty`.
  - Windows: `set_foreground`, `wait_for_window`, `minimize_all`,
    `foreground_info`, `close_windows`, `close_window`, `record_windows`,
    `wait_for_new_windows_gone`, `close_abandoned_windows`,
    `take_foreign_terminal_windows`.
  - Accessibility reads: `focused_element`, `focus_by_automation_id`,
    `misspelt_words`, `name_by_automation_id`, `controlled_items`,
    `key_toggled`.
  - Input: `send_keys`, `press_key`, `release_key`, `type_text`,
    `send_keys_paced`, `repeat_key`, `type_text_paced`, each returning the
    harness number of its last input.
  - Files and events: `write_file`, `delete_file`, `read_file`,
    `copy_file` (in chunks), `list_files`, `list_folders`, `delete_folder`,
    `wait_for_file`, `create_event`, `wait_for_event`.
  - `session_info`, and `open_control_tunnel`, which relays Verbatim's
    control-plane pipe over the connection and hands back a ready
    `verbatim_control::client::Client`.

  Every request that waits on the agent's side is read with a timeout
  ten seconds longer than its wait, set on the socket handle replies are
  read from, so a failure is a named timeout. A tunnel reads with
  `CONTROL_REPLY_TIMEOUT` (30 seconds, the agent's own hang bound), so a
  slow reply to a request such as `DumpRecorder` is not a failure; a
  tunnel subscribed to events reads with `CONTROL_POLL_INTERVAL` (2
  seconds) once its subscribe is answered, so its reader can check its own
  deadline. The speech tunnel has no read timeout once subscribed: its
  reader thread's read ends when a frame arrives, or when dropping the
  collector shuts the connection down (`Sender::shutdown`), and the drop
  joins the thread.
- `timeline`: gestures and keys pressed, typed text, and speech (queued,
  audio started, ended), saved whole in time order, and printed on
  failure with each earlier step shortened to one line. It also holds the
  declared differences from NVDA (`push_deviation`) and, in compare mode,
  NVDA's other events, the harness's notes, and the steps.
- `nvda` and `compare`: compare mode, above.
- `windows_terminal`: the Windows Terminal the terminal scenarios drive,
  kept apart from the machine's own. `prepare(stage_dir)` makes sure the
  official portable release, pinned to `VERSION`, is unpacked in `FOLDER`
  under the stage with a `.portable` file next to `EXECUTABLE`,
  downloading it with Windows' `curl.exe` from GitHub for the host's
  architecture (x64 or ARM64), checking its pinned SHA-256, and unpacking
  it with Windows' `tar.exe` only when the folder's stamp does not name
  that hash. Portable mode keeps settings in `SETTINGS_FOLDER` inside it.
  Local staging (`scenario::stage_directory`) calls it, as do
  `cargo xtask windows-terminal`, which CI runs before the suite, and
  `cargo xtask vm deploy`, which ships the folder to the guest.
