# verbatim-e2e

The end-to-end suite, a scenario registry since milestone M3 Track B:
drives a real, running Verbatim (and target applications such as Notepad)
through `verbatim-agent` and, tunneled through it, Verbatim's own control
plane. Dev-only; a library rather than only a test binary because both
`crates/verbatim-e2e/tests/e2e.rs` and `xtask vm test` drive it. Every scenario
meets `docs/testing.md`. See `docs/tooling/e2e.md` for how to run it by hand
and how to read a failure.

## Running, skipping, and failing

- The suite is one test binary, `crates/verbatim-e2e/tests/e2e.rs`, with
  a `#[test]` per scenario named after it, so
  `cargo test -p verbatim-e2e --test e2e -- --ignored --exact <name> --test-threads=1`
  runs one scenario. One binary rather than one per scenario keeps the
  build small: each test binary links the whole harness. A unit test in
  `registry.rs`, which `cargo xtask ci` runs, reads `tests/e2e.rs` as text
  and fails, naming each, when a registered scenario has no test there or
  a test there has no registered scenario.
- Every live `#[test]` in it is
  `#[ignore]`d with the reason "live", so a workspace test run (and
  `cargo xtask ci`) lists them as ignored rather than reporting them as
  passed. The end-to-end job runs them with
  `cargo test -p verbatim-e2e -- --ignored --skip demo_ --skip measure_ --skip notepad_ --skip settings_search_suggestions --skip settings_combo_box_expanded --test-threads=1`.
- `endpoint()` reads `ENDPOINT_ENV` (`VERBATIM_E2E_ENDPOINT`). A live test
  run without it fails: a scenario that cannot reach an agent has not
  passed. `session_info`, the precondition check that the agent runs in an
  interactive session on the `Default` input desktop, is a plain ignored
  `#[test]` outside the registry and fails the same way.
- Local-only scenarios test Windows 11 Notepad, which GitHub's Windows
  Server runner lacks, or, for `settings_search_suggestions`, a Settings
  app that selects the first search suggestion as the list opens, which
  the runner's Settings does not, or, for `settings_combo_box_expanded`, a
  Date and time page that opens with its focus on the Region button, where
  the runner's opens on its location settings link. Their names, and only
  theirs, start with one of `LOCAL_ONLY_FILTERS` (`notepad_`,
  `settings_search_suggestions`, and `settings_combo_box_expanded`), so a run off a Windows 11 desktop
  deselects them with a `--skip` for each; a unit test checks that CI's
  `e2e` job skips exactly these, and that no other scenario's name
  contains one. A run with `SKIP_LOCAL_ONLY_ENV`
  (`VERBATIM_E2E_SKIP_LOCAL_ONLY`) set to `1` that runs one anyway fails.
  `skips` validates the variable: unset, empty, `0`, or `1`, anything else
  an error. Nothing detects the machine.
- Demonstrations (the Demo group, `demo_` names) run only through
  `cargo xtask demo`.
- The measure set (the Measure group, `measure_` names) is measurement,
  not tests: ignored, run only by name or with `--group measure`, and
  deselected from every suite run with `--skip measure_`
  (`docs/tooling/e2e.md`, "The measure set").

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
2. Sweeps what an earlier run left: Windows 11 Notepad's harness tabs are
   closed as tabs first (a Notepad ended by its handle keeps its tabs for
   its next session, without their documents), then every process the
   agent launched that still runs is ended by its own handle
   (`EndLaunched`), the windows an abandoned scenario left are reported
   and closed by handle (`CloseAbandonedWindows`, described under
   Cleanup), other windows titled with
   `DOCUMENT_MARKER` are closed, and harness files and folders in the run
   directory are deleted. Anything that will not go fails the launch. The
   agent then records every top-level window there is as the scenario's
   starting windows (`RecordWindows`), before the scenario opens anything,
   its document included; minimizing, which comes later, changes no
   window's identity.
3. Opens the scenario's Windows 11 Notepad document, when its definition
   names one (`ScenarioDef::document`, a `Document`), and waits on window
   events until it is in the foreground titled with the document; it
   fails if a Notepad window is already open, so the window and the
   process are the scenario's own. Opened before Verbatim starts, the
   window's first-showing rename from "Notepad" to the document cannot
   race Verbatim's announcement of it.
4. Minimizes every window, as the taskbar's Show Desktop does, and then
   directly each window still restored after it (once, the document
   opened in step 3 stayed restored; the suspected cause, not confirmed,
   is that the taskbar skips a window it has not taken in yet); waits until
   every window that can be minimized is (cloaked windows, which are not
   shown, aside), and brings the desktop, Program Manager, to the
   foreground, all through the agent and with no input injected. It then
   fails, naming the window, if any shown window other than the desktop,
   owned windows such as a message box included, is not minimized: Show
   Desktop does not minimize such a window, and it would take the
   foreground whenever the window in front closed (`minimize_failure`).
   A window with `WS_EX_NOACTIVATE`, such as the Widgets board's, never
   takes the foreground, so it passes.
5. Starts the recording, when recording, and waits for ffmpeg to capture
   its first frame, then creates a named event through
   the agent, launches Verbatim with its name in `VERBATIM_READY_EVENT`,
   and waits for Verbatim to set it once it is ready for input; then opens the command connection and the speech
   connection and checks the status reports it ready.

Verbatim is launched with `RUST_LOG` set to `DEFAULT_RUST_LOG`
(`info,verbatim_outpost=debug,verbatim_uia::subscribe=debug,verbatim_uia::com=debug,verbatim_ia2::hook=debug`), which its outposts inherit, so a run's
artifacts hold the outposts' debug lines: what each read and when, what it
released, and what it reported, and every move of a UIA subscription with
each call it made and its result. The focus listener runs in the same
binary (`verbatim_outpost::listener`) and is covered by the filter, but
has no log lines of its own yet. `RUST_LOG_ENV` (`VERBATIM_E2E_RUST_LOG`)
overrides the filter; set but empty, Verbatim logs as its own
configuration says.

Every scenario then starts by asserting the startup speech exactly
(`registry::startup_speech`): the start sound heard (Verbatim speaks no
start message, as NVDA speaks none), then the desktop's announcement, as
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
connection is split (`verbatim_control::client::Client::split`): a
thread of the collector's own reads every frame as it arrives, records
it on the timeline then, and passes it on to the assertions, while
`AwaitIdle` is sent on the writing half. Read on the scenario's thread,
as frames were before, a frame that arrived while that thread waited on
the agent, such as for a window to be brought forward, was recorded only
once the agent answered, so the timeline put it after the step it
preceded. There is one kind of
assertion: the next utterances are exactly this list, each with its exact
text, in order, each ending as expected, and nothing else in between. An
empty utterance counts, and so does a sound played at once for an event,
which reads as `sound:` and its indication (`sound: exit`); a sound inside
an utterance is named in its text.

- `expect(texts)`: each heard in full, queued and ended within
  `STEP_TIMEOUT` (15 seconds, a bound on a hang).
- `expect_sequence(expected)`: the same, with each ending given:
  `heard(text)` or `cut_off(text)`, for a step whose speech is
  interrupted, such as the startup message. An item may instead be an
  optional utterance (`docs/testing.md`, "Optional utterances"):
  `optional(text)` accepts, in its place, `text` heard in full, cut off,
  or not said at all, and `optional_up_to(text, times)` the same up to
  `times` times in a row. Nothing else is accepted in its place. An
  optional utterance must be followed in the same sequence by an
  utterance of other text said once, whose arrival shows that it was not
  said; otherwise the assertion fails as a harness error. Where the same
  text is expected once right after it, the run of that text is split
  with the expected ones last, since a repeat cuts off the one before
  it: `optional(t), heard(t), heard(u)` accepts `t u` and `t t u`, the
  first `t` of two heard in full or cut off. What became of each
  optional utterance, an `OptionalOutcome` (its text, the input its step
  answers, and each time's ending, none when absent), is noted on the
  timeline and kept in `optional_outcomes()`, which the registry reports.
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
  sends `AwaitIdle` on its own connection with the number of the last key
  the scenario injected; Verbatim answers once it has handled that input
  and is idle, so everything it queued before then arrives first, and none
  of it may be unmatched. That is evidence, never a stretch of silence.
- `require_all_asserted(when)`: called before every injected input. It
  reads the frames already on their way, without waiting for anything to
  happen, and fails, as a harness error, if any utterance is waiting that
  no assertion matched. A scenario never moves past speech it has not
  asserted. `require_asserted_before(when, expected)` is the same check
  before the input of a step whose speech is `expected`: the waiting
  utterances may be exactly that step's leading optional utterances, in
  order and each up to its declared times, which are then left for the
  step's assertion to match; anything else fails as before.
  `Scenario::send_keys_expecting(keys, expected)` makes that check, sends
  the keys, and asserts `expected` with `expect_sequence`, for a step whose
  leading optional utterance may arrive before or after its keys.
- `collect_only()` and `take_all_queued(after_input)`, for the measure set
  only, which asserts no speech: the registry sets collect-only mode for a
  Measure scenario once the startup speech and the desktop's announcement
  are asserted. From then on every utterance is still read and kept on
  the timeline, but `require_all_asserted` and `expect_nothing_more`
  collect unmatched utterances instead of failing, and `expect_nothing_more`
  no longer waits for each utterance to end. `take_all_queued` returns
  everything collected or unmatched once Verbatim is idle after the given
  input, together with every frame that has reached the harness since,
  each counted as matched without its content being asserted.
  `take_collected()` returns only what collect-only mode has already
  collected, waiting for nothing, so a measure scenario that waits with
  `take_until` after a key can keep the speech that came before the key
  first, in arrival order.

A failed assertion unwinds with a `SpeechFailure`, without the panic
hook, and the run reports it once the logs are collected
(`SpeechFailure::report`): the expected and actual sequences escaped
(`{:?}`), the index of the first utterance that differs and the first
character in it that differs; the failing step's trace, every line
carrying its trace ID in Verbatim's log, Core's flight recorder and the
outposts', listener's and synthesizer host's logs, merged in time order
(`artifacts::trace_lines`); and the timeline, each earlier step on one
line (its input and what it said) and the failing step in full. The
failing step's trace ID is the one the speech frame of the utterance the
assertion failed on carries; when it failed on none, an utterance never
queued, it is the newest heard in the step.
`latency_rows` gives one row per utterance with the time of the event
behind it, which the run saves as `latency.csv`.

## Compare mode

Compare mode runs a scenario's own setup and body against NVDA instead of
Verbatim, asserts nothing, and writes a report setting what the scenario
expects Verbatim to say beside what NVDA said, step by step
(`docs/nvda-transcript.md`, "Comparing a scenario", says how to run it).
It is a research instrument, not a test, and never runs in CI.
`compare::ENV`, `VERBATIM_E2E_COMPARE`, set to a scenario's name, selects
it for that scenario only: `registry::run_named` asks `compare::selects`,
and a scenario run while the variable names another fails at once as a
harness error naming both, so a suite run with it left set cannot pass.
`compare::QUIET_ENV`, `VERBATIM_E2E_QUIET_MS`, sets the quiet period
(`nvda::DEFAULT_QUIET`, one second, by default), raised to
`compare::QUIET_FLOOR`, 500 ms, NVDA's multi-press timeout, when it is
shorter (`compare::QuietPeriod`, which the report's header notes), so the
inputs of two steps are never a repeated press for NVDA.

- The NVDA source and sink. `nvda::Transcript` is the add-on's client,
  shared with `cargo xtask nvda capture`: `session_port`, `connect` with
  its `Hello`, and `entries_after`, giving `TranscriptEntry`s. For
  compare mode, `nvda::speech_source` makes the frame source and request
  sink the collector reads a speech connection through, so the collector
  and the scenarios need nothing of NVDA's own. A reader thread polls the
  transcript every `POLL_INTERVAL` (50 ms). Each speech entry becomes
  `Frame::Speech` with an utterance id of its own, the flattened text, a
  trace ID minted for it, which nothing reads, and its time on the
  harness's clock from the entry's milliseconds since the `Hello`,
  followed at once by `Frame::SpeechEnded`, completed, since the add-on
  reports only what NVDA queued. A cancel, or any other event, goes on
  the timeline as `nvda cancel` and is no frame. The sink answers
  `SubscribeSpeech` at once and refuses every other request but one.
- The barrier. The sink answers `AwaitIdle` once NVDA has queued nothing
  for the quiet period since the request or since its last entry,
  whichever is later (`nvda::QuietWait`), or at the request's timeout,
  with a note on the timeline that NVDA's speech did not settle: the
  capture's settling rule applied as the harness's barrier. A second
  `AwaitIdle` sent while one waits is an error.
- The collector (`SpeechCollector::compare_mode`). `expect`,
  `expect_sequence` and `expect_within` record their texts against the
  step under way, send `AwaitIdle` and read everything until its answer,
  so the scenario's next action, input or not, does not run ahead of
  NVDA, and mark how many entries the timeline held then.
  `expect_started` and `expect_queued` record their texts and return
  placeholders without waiting for the quiet period, only until NVDA has
  queued speech in the step (a note says when it queued none within the
  step timeout), since they assert speech the next input interrupts,
  which follows at once, as the second press of a double press does; the assertions on endings, pauses and resumes return at
  once, and `ending_of` answers `None`. `require_all_asserted`, before
  every input and every action that changes what is in front, and
  `expect_nothing_more` close the step: they wait for `AwaitIdle` in the
  same way unless the timeline holds no entry since that mark, which
  rules out an input or an action since, each putting an entry on the
  timeline as it starts its step, so an input right after an assertion
  does not wait twice; and record a `compare::CompareStep` from the
  timeline's entries since the step before (`Timeline::step_since`) and
  the texts expected. A step that recorded speech the next input
  interrupts is closed without waiting or recording, with a note: it
  stays open, and its texts and what NVDA queued are recorded with the
  next step's, as one unit whose input names both inputs, since NVDA's
  speech for a rapid double press cannot be told apart press by press.
  `take_until`, `take_until_idle` and `take_all_queued` cannot keep their
  promises without Verbatim and fail as a harness error. An optional
  utterance is left out of the texts recorded, so NVDA's speech is
  compared with what is always expected.
  `take_comparison` hands the steps to the registry.
- The scenario. `Scenario::compare_with` does what `Scenario::launch_with`
  does up to opening the document, building and staging included, since
  scenarios start `mockapp` and the portable Windows Terminal from the
  stage, then connects to the transcript in the agent's session, before
  minimizing, so NVDA's announcement of the desktop is heard; it launches
  no Verbatim and records no video, and refuses a remote run.
  `is_comparing` says which mode a run is in. Each action that changes
  what is in front (`launch_titled`, `launch_titled_once`,
  `launch_owning_window`, `launch_console`, `bring_window_forward`,
  `bring_document_forward`, `open_folder`, `open_settings_page`,
  `run_handing_off`, `close_windows_titled`, `focus_by_automation_id`,
  and `terminal::expect_prompt_read`'s writing of the file that lets the
  shell show its first prompt) calls `Scenario::begin_action`, which in
  compare mode closes the step under way and starts one with the action
  as its input (`Timeline::push_action`), as a keystroke does; a normal
  run is unchanged by it. `send_gesture` presses every keyboard gesture
  as keys, the Verbatim modifier as Insert (`kb:verbatim+v` as
  `insert+v`, `kb:numpad8` as `numpad8`), and whatever NVDA does with
  them is what the step records. What needs Verbatim itself (`status`,
  `subscribe_events`, `verbatim_children`, `end_verbatim_process`,
  `send_keys_from_verbatim`, and the checks after the body) is an error.
  A wait on one of Verbatim's events has no NVDA equivalent, so
  `terminal::expect_prompt_read`, `text_box::paste`,
  `terminal_flood::watch_for_prompt` and
  `terminal_flood::watch_for_prompt_output` skip theirs in compare mode,
  and the quiet period the assertion after it waits for stands in.
- Declared differences. `Scenario::deviates_from_nvda(file, entry)`,
  called after a step's input, records on the timeline that the step
  deliberately differs from NVDA, as the parity entry headed `entry` in
  `file` records, such as `"docs/parity/review.md"` and
  `"Review cursor columns"`. In a normal run it is a timeline note only. A
  unit test (`every_declared_deviation_names_an_entry_of_the_parity_ledger`)
  reads every call in every source under `src/scenarios`, its subfolders
  included, in the method form and in the function-call form
  (`Scenario::deviates_from_nvda(scenario, file, entry)`), and checks the
  entry is a `###` heading of that file. It fails on a call whose file or
  entry is not a string literal, and does not see a call made through
  another name, such as a wrapper.
- `ScenarioDef::comparable`. `Comparable::Yes`, or `Comparable::No` with
  why: the scenarios that end Verbatim's processes, read its status, take
  speech until a line or until idle, or drive Verbatim's own menu and
  settings dialog, which do not exist without Verbatim, and the demo and
  measure groups. Compare mode refuses them with the reason.
- The run (`registry::run_compare`). It hears the desktop as an ordinary
  step, runs setup, the body and the end of the speech, the teardown and
  the cleanup, and writes the timeline, the report, the foreground record
  and the summary, and archives the run; none of the checks on Verbatim
  after the body runs. It passes unless the harness itself fails, such as
  a window not in front, a method that needs Verbatim, or the transcript
  lost.
- The report (`compare::render_report`, `compare.txt` in the run's
  artifacts, also printed). The header names the scenario, the date,
  NVDA's version from the `Hello`, the `nvda` submodule's pin
  (`compare::submodule_pin`), whether the pin is the installed build,
  told from the commit an alpha's version ends with or a release's tag,
  and the quiet period, with a note when it was raised to the floor. Each
  step gives its input, "Verbatim:" and the texts expected one per line,
  an utterance that is nothing but a sound marked "not compared",
  "NVDA:" and what it said one per line, "cancel" where it cancelled, any
  declared deviation and note, on a step that differs the two strings
  compared ("Verbatim compared:" and "NVDA compared:"), and one verdict
  (`CompareStep::verdict`). A step with an input is always reported,
  even with nothing said on either side (`CompareStep::new` gives `None`
  only for a step with no input and nothing in it). A step is judged on
  `CompareStep::compared`: on the expected side every sound, `sound:` and
  its name, is taken out wherever it is in an utterance and the rest
  kept, NVDA's cancels and empty entries are dropped, each side is joined
  with single spaces, and every run of white space made one space, since
  the add-on records NVDA's speech sequence parts rather than utterance
  boundaries, so boundaries and spacing are not evidence of a difference.
  The verdict is "same" when the two strings are equal; "differs";
  "declared deviation: <entry>"; "declared, but NVDA agrees: <entry>",
  worth a look; or, for the run's start (`CompareStep::start`, its first
  step when that has no input), "not compared: NVDA says nothing about a
  desktop already in front", given before anything is judged. A summary
  counts each verdict.

## Input

Every key and character the scenario injects goes through the agent
(`Scenario::send_keys`, `type_text`), which numbers it in `dwExtraInfo`
(see the [verbatim-input guide](verbatim-input.md)); Verbatim's keyboard
hook reads the number and Core reports it handled
(`Frame::InputHandled`) once every effect the key caused is queued, the
barrier `expect_nothing_more` waits on. `hold_key` and `release_key` hold
one key, such as Control, down across the keys sent between them, through
the agent's `PressKey` and `ReleaseKey`; a key still held when the body
ends, passed or failed, is released then and fails the run
(`release_held_keys`, run again by the cleanup). `send_gesture` sends a gesture by
its identifier through the control plane. For the measure set,
`send_keys_paced(keys, interval_ms)` and `type_text_paced(text,
interval_ms)` inject one stroke or character every `interval_ms` through
the agent's `SendKeysPaced` and `TypeTextPaced`, numbered like any other
input, and return the first and last numbers; `repeat_key(key, delay_ms,
interval_ms, repeats)` holds a key down with auto-repeat through the
agent's `RepeatKey` and returns the press's and the release's numbers;
`take_all_queued_within(bound)` is `take_all_queued` with a longer bound
on Verbatim becoming idle, for a measurement whose keys Verbatim keeps
waiting; `send_keys_from_verbatim`
injects keys through Verbatim's own control plane (`Request::SendKeys`),
which carry no harness number; and `take_all_queued` and
`close_windows_titled` (close every window whose title contains the text,
failing unless all closed) serve their steps. The matching
`AgentClient` methods are `send_keys_paced`, `type_text_paced` and
`repeat_key`. Nothing taps keys or uses
Alt+Tab to move the foreground, not even in a scenario about switching
windows: the window is brought forward through the agent, as its
taskbar button does; a window that does not take the foreground fails
the step. The agent can let a launched program take the
foreground only while it injected the last input, which it does as every
scenario runs (see `docs/tooling/troubleshooting.md`).

## Scenario

`Scenario` owns one live run. Besides `speech()`, `status()`,
and the input methods above:

- `launch_titled(command, args, title, owner_exits)` starts a program
  whose window carries `title`, its first window minimized and inactive,
  then restores that window and sets it as the foreground, as its taskbar
  button does, and waits for it to take the foreground, so the launch does
  not depend on the agent having injected the last input (`docs/tooling/troubleshooting.md`,
  "Windows' foreground lock keeps launched applications behind"); a
  window that ignores the minimized show state, as msinfo32's does, is
  minimized by the agent before it is restored;
  `launch_titled_once(command, args, title, owner_exits, ready)` does the
  same, bringing the window forward only once the agent sees the file
  `ready`, the program's evidence that it has written what it writes as it
  starts, which is then on screen before the window is announced;
  `launch_console(command, args, title, ready)` starts a console program with its
  console window titled from its first frame (the launch's console title),
  which the console host scenarios use, opened minimized and inactive,
  without the agent's foreground right, and brought forward as
  `launch_titled_once` does; `require_absent(title)` fails
  when a window so titled is already open, for a program whose title is
  not the run's own, such as msinfo32; `launched_children` lists what the
  last launched program started.
- `bring_window_forward(marker)` brings a window the scenario opened,
  found by the marker in its title, to the foreground, as its taskbar
  button does and as NVDA's system tests do with `SetForegroundWindow`,
  and waits on window events until it is in front; the suite switches
  windows only this way (Dickson, 2026-10-10).
  `bring_document_forward(name)` does the same for the Notepad document
  opened before launch; `save_document` and `expect_unsaved` wait on its title.
- `open_folder` opens a File Explorer window on a harness folder,
  launched without the agent's foreground right and brought forward like
  every other launch;
  `open_settings_page` opens a page of the Settings app.
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
  first child reports and the names of the first children), `name_by_automation_id`
  (the name of the foreground window's element with that UI Automation
  identifier, read without focusing it), and `misspelt_words`
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
exit. A File Explorer window (`open_folder`) is closed by its handle
instead, recorded as it opened, since File Explorer retitles it as it
navigates; it is closed, and waited for until it has gone, before the
harness folders are deleted, so it is never left to navigate to the
parent folder. Windows 11 Notepad is closed only by closing its harness tabs, never
by closing its window, since Notepad keeps the tabs of a closed window for
its next session; a Notepad process still running afterwards fails the run
with the windows it still has. A program with no window of its own is
ended by its process id. Nothing is ever ended by its image name. The
`Drop` impl quits or ends Verbatim and runs the cleanup when a panic
skipped it.

Closing by title is how a scenario closes its own windows, but it cannot
prove that nothing was left: a window can lose the title it was found
by, as a File Explorer window whose folder is deleted is retitled with
the parent folder. So the cleanup's last step asks the agent to wait, on
window events and up to the close timeout, for every shown window that
was not among the scenario's starting windows to be gone
(`WaitForNewWindowsGone`). Each window still open fails the run, named by
its title, class, process id, and program, and is left open: the cleanup
closes none of them itself. Windows are identified by handle, owning
process, and class, never by title; a window counts as gone once it is
closed, hidden, or cloaked, as the Settings app keeps its closed window.

The agent keeps the starting windows, which outlive the test process, with
the connection that recorded them. When that connection ends before the
check has found nothing left, because the test process was killed or the
check failed, the scenario is abandoned, and the agent notes the windows
it had open then. The next scenario's start prints each of them that is
still open and closes it by handle, and fails only if one will not close.
That is safe because only windows absent from the abandoned scenario's
starting windows and open when its connection ended are closed: the
owner's own windows, including the Windows Terminal running the suite,
were open before it started, or were opened after its test process ended.

The title sweep at the start (`DOCUMENT_MARKER`) is kept: it still closes
a harness window left by a run whose starting windows the agent no longer
holds, because the agent was restarted in between.

### Crashes

Any of Verbatim's own processes (Verbatim, an outpost, the focus
listener, a synthesizer host) that exits during the scenario without the
scenario having ended it on purpose fails the run, read from the exits
the agent records for Verbatim's job (`JobExits`). Verbatim must quit with
exit code 0, and must leave none of its processes behind: once it has
exited, the harness waits for its job to empty (`WaitForJobEmpty`), and
every outpost and the focus listener (`verbatim-outpost.exe`) but one the
scenario ended on purpose must have shut down cleanly, exiting with code 0
before Verbatim did; one that Verbatim's supervisor had to kill exits with
`verbatim_process::KILLED_EXIT_CODE` and fails the run
(`Scenario::outposts_shut_down_cleanly`; `docs/crates/verbatim-outpost.md`,
"Shutting down"). When crash dumps are configured
(`vm/scripts/Enable-VerbatimCrashDumps.ps1`), the dumps that appeared in
`CRASH_DUMP_FOLDER` during the run are copied into its artifacts, in
chunks, since an outpost's dump is a full-memory dump of a few hundred
megabytes.

Outpost crashes are also counted, so how often they happen is known.
Once the artifacts are collected, `Scenario::outpost_crashes` reads the
job's exits again and returns an `artifacts::OutpostCrash` for each
outpost or focus listener the scenario did not end on purpose whose exit
Windows called abnormal or whose exit code is an exception code (an
`NTSTATUS` of `0xC0000000` or above, such as `0xC0000005` for an access
violation or `0xC0000409` for a fail-fast): its pid, its exit code, and
the path of its dump in the artifacts (`verbatim-outpost.exe.<pid>.dmp`),
when one was written. The run records them in its summary as the field
`ScenarioSummary::outpost_crashes` whether or not its steps passed, and
each is also an unexpected exit, so the scenario fails as well. After every
scenario the run writes one line to the stderr handle itself, which the
test harness does not capture, so it shows for a passing scenario too:
that scenario's crashes, and the running total of crashes and scenarios
in this test process. The suite runs every scenario in one test process,
so the last scenario's line holds the suite's total.

Optional utterances are reported the same way, so a change in how often
an application causes one stays visible: after every scenario the run
writes one line to the stderr handle for each optional utterance the
scenario asserted, such as `scenario "explorer_folder_window": optional
utterance "Working on it..." after keys enter: cut off`, and records the
same text in its summary as the field
`ScenarioSummary::optional_utterances` (`optional_utterance:` lines),
which `cargo xtask vm test` prints in its run summary.

### The user's Windows Terminal

The suite runs on the owner's desktop, whose own Windows Terminal hosts
their work, so nothing a run does may reach it. Every program the agent
starts without a console title, and every console program the harness
runs itself (cargo, `curl.exe`, `tar.exe`, with `CREATE_NO_WINDOW`), has
no console window, so Windows never hands a console to the default
terminal application; the console host scenarios start `conhost.exe`
explicitly, and the Windows Terminal scenarios start the harness's
portable copy. The agent watches for Windows Terminal windows from the
start of each run (`TakeForeignTerminalWindows`), and any window of a
Windows Terminal process the agent did not launch that appears during the
run fails it (`Scenario::foreign_terminal_windows`). Verbatim is launched
with `AgentClient::launch_verbatim`, which has the agent name every such
Windows Terminal, and the console hosts it runs, for Verbatim to ignore
entirely (`VERBATIM_IGNORE_PIDS`, `docs/tooling/e2e.md`); the run fails if
Verbatim's log folder holds an outpost log named for one of them
(`Scenario::collect_run_artifacts`).

## The run

`registry::run_named` is what every `#[test]` wrapper calls; its `run`
does, in order: launch; assert the startup speech and the desktop; run
setup and the body, which ends with `expect_nothing_more`; save Core's
focus (`collect_focus`, `focus.txt`: the focus,
its ancestors, and the navigator, from the control plane's `DumpFocus`),
and the flight recorder while Verbatim is up; check for unexpected exits;
quit Verbatim and check that its outposts and listener shut down cleanly;
take the speech Verbatim sent: after a clean quit, every frame until the
speech connection ends (`take_until_the_end`, a `recv_timeout` loop
bounded by the step timeout, its end not coming a problem of the run),
since the agent shuts the connection down when Verbatim's pipe closes
and the reader thread passes that end on after every frame it read,
recording each frame on the timeline and then passing it on; after a
failed quit, only what has arrived (`take_remaining`, a `try_recv` loop
that waits for nothing, as compare mode's report takes it); run the
teardown; clean up; collect the artifacts; finish the recording;
check that no Windows Terminal window the run did not open appeared;
write `foreground.txt`, the summary, and archive the run. The summary's
latency lines (`ScenarioSummary::new`) are built from the speech
collector's `latency_rows`, the rows `latency.csv` holds: every utterance
that carries its event's time (the startup speech and the desktop's
announcement carry none). Taking the speech up to the connection's end
first makes them include the run's last utterance in collect-only mode, which does not
wait for speech to end, and a failed run's speech after its failing
step. They count utterances, not traces, and an utterance's event to
audio includes the time it waited while the speech queued before it
played, so the audio maximum is not comparable with runs before
c5be1bd2, whose figures were per trace (`docs/tooling/e2e.md`).
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
new crash dumps, the video `<name>.mp4` and ffmpeg's logs
(`recording-video.log` from the capture, `recording-mux.log`, and
`recording-check.log`) when recording, and `summary.txt`.
Losing any of them fails the run. `archive_run` copies each run into
`history/<scenario>/<UTC time>-<pass or fail>`, keeping the newest 100,
without the video; crash dumps are archived with their run.

## Recording

A video of each scenario with Verbatim's speech (decision D16), unless
`RECORD_ENV` (`VERBATIM_E2E_RECORD`) is `0` or `false`. ffmpeg, named by
`FFMPEG_ENV`, captures the desktop on the agent's machine into fragmented
MP4; `start` returns once ffmpeg has created its output file, which it
does only after capturing its first frame and logging that frame's time,
within `FIRST_FRAME_TIMEOUT` (30 seconds). Verbatim always writes
everything it plays into a WAV file (`VERBATIM_RECORD_AUDIO`), recording
or not, so recording changes nothing about Verbatim's run.

`stop` ends the capture as a user ends ffmpeg: the capture is launched
with a stdin pipe (the agent's `stdin_piped`), and `stop` writes `q` to it
(`WriteStdin`) and waits for ffmpeg to exit with code 0 within
`STOP_TIMEOUT` (15 seconds). ffmpeg reads its stdin only once per stats
period, and on `q` its threaded scheduler stops without draining: the
frames still queued between the `gdigrab` input and the scale and encode
stages are dropped, so the video loses a random tail of a few frames, up
to about a second, while every frame ffmpeg encoded reaches the file and
it exits with code 0. The capture is stopped only after Verbatim has quit
and the teardown and artifact collection have run, so that tail is never
the scenario's. Killing ffmpeg instead loses frames it encoded, its last
one to two seconds, not just the fragment being written: the muxer holds every frame
since the last keyframe in memory, and up to 32 KB of the fragment before
that waits in a file buffer that is never flushed, so the video ends in a
cut keyframe. ffmpeg is killed only when the bound passes, and that is a
failure. A scenario that panicked is stopped the same way, by the
scenario's `Drop` when `finish` was never reached.

`finish` takes the wall-clock time the scenario's body ended, which
`registry::run` records, stops the capture, muxes it with the audio,
copies the result back with ffmpeg's logs, and checks it (`verify`). The
check decodes the recording with `ffmpeg -v error -f null`, and fails
when that logs anything; when the mux log has an error or a corrupt
packet (the mux exits 0 on both); when the frame count on the final
stats line of the capture's log differs from the number of frames a
`framecrc` list of the video gives, or that line's `time=` is more than
one frame from the last frame's time, either of which means frames
ffmpeg encoded are missing; or when the first frame's wall-clock time
plus the last frame's time is before the body ended. Every frame keeps the time
`gdigrab` captured it at, to the millisecond (`-fps_mode passthrough`),
so the video keeps real time when a loaded machine makes the capture
miss frames; ffmpeg's default rounds each time to the frame interval and
drops a late frame that rounds onto the next one's time. A recording that cannot start or be finished, or fails the check,
is a failure.

## The registry

`ScenarioDef` is one named, grouped scenario: `name` (its `#[test]`
function, `--scenario` selector, artifacts directory, and video name),
`group`, `settings` (a change to the fixed settings), `local_only`,
`comparable` (whether compare mode can run it, and why not; see "Compare
mode"), and `setup`/`body`/`teardown`. `ScenarioState` is what setup hands the body
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
  through a new outpost), `return_after_retirement` (a Windows Forms text
  box brought forward again once Core reports its outpost retired after
  two minutes without attention, announced as before through a new
  outpost), `explorer_folder_window`, and
  `settings_system_page` (the list focused by its UI Automation
  identifier through the agent), and `settings_combo_box_expanded` (a
  Date and time combo box focused the same way, opened with Alt+Down
  Arrow, "expanded" before its first item, and closed with Escape).
- Navigation: `object_navigation_in_settings` (over MSAA, in Verbatim's
  own dialog), `object_navigation_over_uia` (over UIA, in `mockapp`'s
  provider), and `system_information_tree`.
- Text: the editing scenarios, each in the Windows Forms text box and in
  Windows 11 Notepad, each its own code (`text_box_editing` and
  `notepad_editing`, `_selection_keys`, `_review_cursor`,
  `_review_words`, `_typed_words`, `_word_selection`, `_say_all`),
  the protected (password) fields `text_box_password` (a Windows Forms
  password text box) and `wpf_password` (a WPF `PasswordBox`, read
  through UI Automation), each its own code in `scenarios/password.rs`,
  `spelling_errors` (`mockapp`'s scripted
  text), `notepad_spelling_errors`, and the terminal scenarios, each in
  Windows Terminal and in the console host. The Notepad scenarios are
  local-only.
- Demo: `demo_notepad_editing`, `demo_review_cursor`, `demo_say_all`,
  `demo_terminal_session`, `demo_settings_dialog_keys`. Everything a
  demonstration shows is covered by a test scenario.
- Measure: the measure set in `scenarios/measure.rs`,
  `measure_focus_windows_terminal`, `measure_focus_windows_terminal_tabs`,
  `measure_focus_explorer`, `measure_focus_text_box`,
  `measure_focus_new_application`, `measure_keys_text_box`,
  `measure_keys_conhost`, `measure_keys_windows_terminal` and
  `measure_speech_real`, the last only in an audible run: workloads
  for Verbatim's latency ledger, which assert no speech after the
  desktop's and write each step's ground truth to `measure-truth.jsonl`
  in their artifacts. `measure_focus_explorer` also waits for exact
  announcements (each new window's first item, the folder window's item
  after each close, and each key's item), and fails with a named timeout
  when one is not queued; it writes its samples by kind to
  `measure-summary.txt`. `docs/tooling/e2e.md`, "The measure set", describes
  each and how to run them in debug and release.

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
  fails, with no fallback. Windows Terminal is the harness's portable
  copy (`windows_terminal`, below): its settings folder is deleted, and
  its `WindowsTerminal.exe` is started directly, never `wt.exe`, with
  `-w new --size 120,30 new-tab --title <title>
  --suppressApplicationTitle`, through `Scenario::launch_owning_window`,
  which opens it minimized and inactive and brings it forward as
  `launch_titled_once` does, and fails unless the launched process owns the
  window. Each window is brought forward only once its shell has written
  its process id. With Verbatim running, Windows' screen reader flag is
  set, and Windows PowerShell's interactive host prints, as it starts, a
  notice that it leaves `PSReadLine` out, before the start script runs.
  Windows Terminal takes it from its own console host some time later,
  and shows nothing while minimized that tells when (on GitHub's runner
  its window was announced before the notice, or with half of it); the
  console window takes the foreground as it is created, still minimized,
  before the notice, so Verbatim reads the terminal before it, and the
  start script's `mode con` then clears it, so it is not on screen when
  the process id is written either (diagnosed on 2026-10-11). So every
  shell, in both terminals, runs with `-NonInteractive`, which leaves the
  notice out. The password prompt
  the commands scenarios use is drawn by their script, since `Read-Host`
  fails in a shell that is not interactive. The scenario
  also fails if any other Windows Terminal process opened a window
  meanwhile (`Scenario::top_level_windows`, before and after), and the
  copy's process must exit once its window closes at cleanup. The
  console host opens with `launch_console`, its
  window titled from its first frame; Windows reports a console window as
  its first client's, the shell, so the scenario asserts the window's
  class is `ConsoleWindowClass` and the shell is the launched console
  host's child. The shell writes its process id, which must exit at
  cleanup. Every body starts with `expect_prompt_read`, given the exact
  announcement: Windows Terminal names its text area with the tab's title
  ("<title> terminal"), and the console host's has no name ("terminal").
  A Windows Terminal window's body uses
  `expect_windows_terminal_prompt_read(state, repeats)` instead, which
  builds that announcement with "<title> terminal" also an optional
  utterance up to `repeats` times before the one heard in full
  (`LAUNCH_REPEATS`, 1, as a window opens). A second window opened in
  the running Windows Terminal uses
  `expect_second_windows_terminal_prompt_read(state)`, which also accepts
  the repeats after the one heard in full, until the first prompt, each
  with or without its line, "blank", up to `SECOND_WINDOW_REPEATS`, 2,
  in all. `type_hearing(text, echo)` types one
  character at a time: with `Echo::Shown`, the echo is what the terminal
  shows, and a space at the end of a line cannot be told from padding
  until something follows it, so a space is typed with the next character
  and both echoes heard together; with `Echo::Typed` ("speak passwords"
  on), each key is echoed as it is typed.

The typing scenarios (`scenarios/terminal_typing.rs`) type a password
into a script that never shows it and rewrites its prompt's line after
each key, hearing only the count that changed, never a character typed;
type a character in the middle of a command and hear it echoed; and press
Escape on a typed command, hearing the text it removed.

The screen scenarios (`scenarios/terminal_screens.rs`) print a line of
forty words, far wider than the terminal, heard whole; grow one line by
ten words a key, hearing each key's ten words; and run a full-screen
program on the alternate screen: its thirty rows heard in full, a row near
the top and one near the bottom changed ("changed"), the rows scrolled down
and back up ("row 0", "row 30"), and the alternate screen closed, after
which only "closed" and the prompt are new.

The selection-list scenarios (`scenarios/terminal_lists.rs`) run a
list whose ">" marker moves with the caret, each move one write, and one
redrawn whole in one write on each move with the caret left below it:
Down, Down and Up say "> banana", "> cherry" and "> banana", once each,
the first as the caret's line, as NVDA says it, and the second as output,
where NVDA says the caret's line, "blank", once its wait for the caret
times out. The pager scenarios (`scenarios/terminal_pager.rs`) open a
pager on the alternate screen, its twenty-nine lines and prompt heard in
full, and move it down a line and back up: each move says only the line
it brought onto the screen, "page line 30" and then "page line 1".

Keys during output (`scenarios/terminal_key_timing.rs`), each in both
terminals: Control, pressed once the flood's last line is queued (the
evidence that the whole flood was read), cuts off everything still
queued and nothing more is said, and the lines the script writes after
Control, let go by a file the scenario writes, are spoken, as in NVDA
(`*_control_flood`); Shift twice pauses
and resumes with nothing lost, each change waited for and asserted as
Verbatim reports it (`SpeechCollector::expect_paused` and
`expect_resumed`) (`*_shift_flood`); Up Arrow once a flood
has ended but is still being spoken cuts it and says the recalled line
(`*_line_key_flood`); and Up Arrow with a character typed at once says
the line's change, the recalled line with the character, once
(`*_up_typing`). Scripts wait for
the scenario's files through `FILE_SIGNALS`, on change notifications.
Other floods (`scenarios/terminal_flood_kinds.rs`): 2000 identical lines
counted as any flood (`*_same_flood`), every line of a hundred heard with
both limits raised to 200 (`*_raised_flood`), and a full-screen redraw
of 30 rows with both limits lowered to 10 (`*_redraw_limit`). In
`scenarios/terminal_windows.rs`, `*_leave_flood` leaves a terminal for a
second window during a flood, which is finished while away and not
spoken, and returns to its caret's line. The flood's first line is
sixteen numbered words, about six seconds of speech, so it is still
playing when the switch lands, which takes a few hundred milliseconds;
the scenario requires it had not been heard in full once the second
window is in front, so a slow switch fails as a harness error.
`windows_terminal_close_tab`
closes the second tab and hears the first's terminal and prompt.

The review-output scenarios (`scenarios/terminal_review_output.rs`)
review a line above the caret, let a script print two lines, and hear the
review cursor follow the caret to the prompt. The footer-flood scenarios
(`scenarios/terminal_footer.rs`) write a hundred lines into a scroll
region above a footer that is redrawn during the flood and changed after
it: the flood policy's thirty lines, "skipped 41 lines", the last
twenty-nine, and the footer's change, "done".

The window scenarios (`scenarios/terminal_windows.rs`) open a second
window of each terminal, switch between the two as clicking a taskbar
button does, and type in each; and open a second tab in a Windows
Terminal window from the command line (`-w 0 new-tab`), switch tabs with
Control+Shift+Tab and Control+Tab, and type in each. A second Windows
Terminal window or tab is handed to the harness's running Windows
Terminal, which must own it. The tabs scenario's Windows Terminal has
`confirmCloseAllTabs` turned off, so its window closes at cleanup without
asking.

The flood scenarios assert the flood policy exactly within the
scrollback: 2,000 lines are heard as lines 1 to 30, "skipped 1941 lines"
after the skipped-lines sound, and lines 1,972 to 2,000 with the prompt.
The ratio of a reported flood's time to an unreported one's is saved as
`wall-time-ratio.txt` for trends, never asserted. The acceptance floods
(`scenarios/terminal_overflow.rs`, each terminal's its own code) run a
script that prints 30 lines, waits for a file the scenario writes, and
then writes its second part in one write: 5,000 lines, within the
history, or 12,000, past it. Written once the 30 have been heard, the
second part is a burst of its own (its first 30 lines, then "skipped 4941
lines" or "skipped more than N lines", and its last 30 with the prompt);
written while they still play (`_during_group`), it joins their burst,
and is skipped down to its last 30. N is the terminal's history less the
30 spoken: 9,001 in Windows Terminal, whose text holds 9,031 rows, and
8,971 in the console host, whose holds 9,001, both measured on
2026-10-08. Outpost logs are collected in chunks, since a flood's
debug log can be larger than one read of the agent's.

## Other modules

- `AgentClient`: a typed client for `verbatim_agent::protocol` (see the
  [verbatim-agent guide](verbatim-agent.md)). Every request that waits on
  the agent's side is read with a timeout ten seconds longer than its
  wait, set on the socket handle replies are read from, so a failure is a
  named timeout. `open_control_tunnel` relays Verbatim's control-plane
  pipe over the connection and hands back a ready
  `verbatim_control::client::Client`, reading with `CONTROL_REPLY_TIMEOUT`
  (30 seconds, the same hang bound as the agent's own replies), so a
  slow reply to a request such as `DumpRecorder` is not a failure. A
  tunnel subscribed to events, once its subscribe is answered, reads with
  `CONTROL_POLL_INTERVAL` (2 seconds) instead, so its reader can check
  its own deadline; a read that times out there is not a failure. The
  speech tunnel has no read timeout once subscribed: its reader thread's
  read ends when a frame arrives, or when dropping the collector shuts
  the connection down (`Sender::shutdown`), and the drop joins the
  thread.
- `timeline`: injected gestures, keys, typed text, and speech (queued,
  audio started, ended), saved whole in time order, and printed on
  failure with each earlier step shortened to one line. It also holds
  the declared differences from NVDA (`push_deviation`) and, in compare
  mode, NVDA's other events, the harness's notes, and the steps of
  Verbatim's own.
- `nvda` and `compare`: compare mode, described above.
- `artifacts::trace_lines`: every line carrying a trace ID in a run's
  collected logs and flight recorder, in time order.
- `artifacts`: `artifacts_root`, `scenario_dir`, `ScenarioSummary` (with
  its `OutpostCrash` list), and `archive_run`, shared with `xtask vm test`
  without argument passing.
- Latency: each run records every utterance's event-to-queue and
  event-to-audio times (`latency_rows`, `latency.csv`, the summary); no
  budget is enforced. Enforcing them by default in local runs, and only
  recording them on GitHub's runners, is decided but not yet built
  (`docs/testing.md`).
- `recording`: described above.
- `windows_terminal`: the Windows Terminal the terminal scenarios drive,
  kept apart from the machine's own, which runs every window in one
  process. `prepare(stage_dir)` makes sure the official portable release,
  pinned to `VERSION` (1.24.12741.0), is unpacked in `FOLDER` under the
  stage with a `.portable` file next to `EXECUTABLE`, downloading it with
  Windows' `curl.exe` from GitHub for the host's architecture (x64 or
  ARM64), checking its pinned SHA-256, and unpacking it with Windows'
  `tar.exe` only when the folder's stamp does not name that hash. Portable
  mode keeps settings in `SETTINGS_FOLDER` inside it, and an unpackaged
  Windows Terminal finds a running instance by a window class and mutex
  named with a hash of its executable's path, so the copy never joins the
  installed one. Runner-direct staging (`scenario::stage_directory`, the
  stage) calls it, as do `cargo xtask windows-terminal`, which CI runs
  before the suite, and `cargo xtask vm deploy`, which ships the folder
  to the guest.

Implementation notes: a same-process `Mutex` enforces one live Verbatim
per test process, and `--test-threads=1` makes that sufficient.
`menu_and_settings_dialog` is the scripted walk of the M1 exit criteria.
