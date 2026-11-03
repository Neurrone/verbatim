# Known failing and flaky tests

The one list of tests and scenarios known to fail intermittently, or consistently, for a known reason. Check it before debugging a failure, and before rerunning or running a baseline: a failure that matches an entry is already understood. Leave a listed scenario out of a related-scenario batch, as `CLAUDE.md` says.

Each entry gives the exact name, how it fails, how often and where, the cause when known, and when the fix is expected. Failure counts are from the dates given, on the development machine ("locally") or GitHub's runner. When a fix lands, delete the entry; history lives in git. When a new failure is explained, add an entry here and link to the work that owns the fix.

## Windows Terminal's first focus, ancestor walk past its bound

Scenarios: the `windows_terminal_` scenarios generally, for example `windows_terminal_scrollback_overflow` and `windows_terminal_clear_flood`, and `windows_terminal_spoken_password`.

- How: a new Windows Terminal window's first focus speaks the title without "window". For `windows_terminal_spoken_password` on GitHub's runner (run 41163500778, 2026-10-31) the expected "ready>" was heard as "verbatim-e2e-spoken-password-mvvtyykb terminal".
- How often: `windows_terminal_progress` and one other scenario failed this way in one full run on GitHub's runner, after the crate split (run 41211072805, 2026-10-31). `windows_terminal_redraw_limit`, `windows_terminal_tabs` and one other scenario failed this way after the caret watch build (run 41225371284, 2026-10-31). `windows_terminal_review_padding` and `windows_terminal_same_flood` failed this way after the first determinism build (run 41233417983, 2026-10-31). On 2026-10-31, with the reducer build, `windows_terminal_scrollback_overflow` failed 10 of 10 locally, and `windows_terminal_clear_flood` 4 of 9, and 4 of 9 before the reducer build too.
- Cause: the listener's ancestor walk for the focus passes its 500 ms bound, so the window's entry is missing from the chain.
- Fix: out of scope until [the focus pipeline rework's remaining design](design/focus-pipeline-rework-remaining.md), which holds it.

## Windows Terminal launch speech, not this cause

- Scenarios: the `*_up_typing`, `*_typing` and `*_commands` Windows Terminal scenarios.
- How: the launch focus sometimes speaks "... terminal" without "... window", and the desktop's announcement is sometimes cancelled.
- Cause: not found; possibly the first entry's.
- Fix: [M6.4 terminal residuals](roadmap.md#m64--terminal-residuals).

## Windows Terminal raises no TextChanged for a typed character

- Scenarios: the Windows Terminal typing scenarios.
- How: the typed character is not echoed, only caret moves are raised. About 5 percent of runs.
- Cause: not known whether Windows Terminal or UIA loses the event; settling it takes about 60 runs and a second, independent UIA client.
- Fix: [M6.4 terminal residuals](roadmap.md#m64--terminal-residuals), with the outpost and Core step.

## Console host scenarios

- `conhost_leave_flood`
  - How: fails at "brings the first window back": on 2026-10-31 the harness could not bring the first console window back to the foreground, because its window did not process the restore for 5 s after its flood. 10 of 10 locally. Earlier, it once heard no "blank" for an empty new console window, its first read giving nothing within 15 seconds.
  - Cause: on the development VM, which renders the desktop in software (WARP), the Desktop Window Manager works through a flooded console's repaints for about 40 seconds after the flood, using up to two cores, and a minimize waits for it in `win32kfull!DwmSyncNotifyMinimizing` while the console's window thread is idle. It happens with no Verbatim running and a restart does not clear it, so it is not Verbatim's to fix. Whether this covers the 2026-10-31 failures on the development machine, and the cause of the empty-console read, are not found.
  - Fix: no fix planned for the manager's backlog (Dickson decided to leave it unaddressed); the unsettled read is under [M6.4 terminal residuals](roadmap.md#m64--terminal-residuals).
- `conhost_typing`
  - How: missed a typed "o" in 2 of 6 runs on the base build.
  - Cause: not found.
  - Fix: [M6.4 terminal residuals](roadmap.md#m64--terminal-residuals).

## File Explorer

- `explorer_folder_window` on GitHub's runner (runs 41157849900 and 41163500778)
  - How: "Items View list" is not spoken after "Working on it..."; the first utterance heard is "delta not selected 1 of 1". Passes locally.
  - Cause: the placeholder's UIA read fails on the runner (0x80040201), so its chain is filled in from window handles, and `ObjectKey::is_same_object` treats two nodes on one window handle as one object whatever their backend, so nothing is entered. NVDA differs on both points.
  - Fix: [the M4.1 follow-ups](roadmap.md#follow-ups-from-m41), the entry on this failure.
  - Also seen (run 41501468361): File Explorer answered the outpost's MSAA calls slowly, a name change taking 3.3 s and a focus 4.5 s, so the gap record's bound passed first; the window's title was spoken, then "Inner" alone from the gap read, its chain filled in from window handles, so "Items View list" and the position were not spoken.
- `explorer_folder_window` locally, Explorer's folder window not responding (2026-10-31)
  - How: nothing is spoken after the desktop's "Recycle Bin 1 of 13"; the folder window, opened in a new `explorer.exe` process, never takes the foreground, its UI thread stops answering about 1.5 s after it opens (an arbitration probe of its `ShellTabWindowClass` window times out at 3 s, an MSAA call waits 13 s), and it does not close within 15 s, so the next scenarios fail at launch on the window left open. Once in 13 runs, with the keep-id fix; the next 10 passed.
  - Cause: not found. Verbatim's outpost was the one waiting on Explorer; no Verbatim call was found that Explorer waited on.
  - Fix: none yet.
- `measure_focus_explorer`
  - How: times out. On 2026-10-29 it failed in 22 of 23 runs in a row on the development machine, before and after that day's changes, while `explorer_folder_window` passed.
  - Cause: File Explorer can focus its "Working on it..." placeholder and raise no later focus; what it raises then, and what makes it constant, are not found.
  - Fix: [the M4.1 follow-ups](roadmap.md#follow-ups-from-m41), the entry on this failure.

## A new console host not responding at its outpost's first call

Scenarios: `measure_keys_conhost` and `measure_keys_conhost_unmoved` (release build, 2026-10-31, on main's code with temporary log lines), and `conhost_full_history_flood` (debug build, 2026-11-01, 1 of 3 with the first determinism build; its outpost was killed at shutdown, and the next scenario, `notepad_editing`, found no Notepad window within 30 s, then passed 10 of 10).

- How: setup fails with "no window titled ... opened within 30s"; the PowerShell console's window is hidden and not responding, or not shown at all. The outpost given the new `conhost.exe` starts watching it, and its worker's first call passes its 10 s deadline about 10 s later and finishes only at shutdown, 2 to 35 s after. 3 of 20 runs; in one, a crate split build's unit tests were running on the same desktop.
- Cause: not found. In two `conhost_full_history_flood` failures on 2026-11-01 whose artifacts were kept, Verbatim waited on the console host and not the reverse: the window never answered the outpost's first message (the arbitration's `WM_GETOBJECT` probe timed out at 3 s, where a healthy console answers in about 0.4 s), the outpost's worker then blocked in an MSAA read until the outpost was killed, and the console host stayed hung after every Verbatim process had exited. Both runs also had Explorer's notification area not answering (`Shell_NotifyIcon` timed out at Verbatim's start) and the Widgets process not responding, which no passing run showed. What the console host's window thread waited on is not in the logs.
- Fix: none yet. The harness now records, when this wait fails, the wait chain of each not-responding window's thread and of the launched process's threads, in the failure message and in `wait-chains.txt` ([the end-to-end guide](tooling/e2e.md)), and the outpost's abandoned-worker warning names the entry its worker had in hand; the next failure's artifacts are to be read for what the thread waits on. A roadmap follow-up under M4.

## A new application's window not shown in time

Scenarios and tests: `notepad_editing`, `notepad_selection_keys` and `notepad_typed_words` locally, and mockapp's `remote_ops::a_windows_forms_menu_item_is_checked_from_the_remote_read` under `cargo xtask ci` on GitHub's runner.

- How: setup fails with "no window titled ... opened within 30s" and no Notepad window among the visible ones, before any key is sent; the mockapp test fails with "the window is shown: Timeout". Locally five times on 2026-11-01, in bursts of up to three runs in a row, then passing; on the runner once (run 41254405706). Twelve runs in a row, six idle and six under a release build's load, all passed.
- Cause: not found. Load from a concurrent build did not reproduce it. The console host entry above, whose kept artifacts show the new window hung on its own with Explorer's notification area also not answering, may be the same; no Notepad failure's artifacts have been kept yet.
- Fix: none yet. The next harness failure carries the wait chains of the not-responding windows' threads and of the launched Notepad's threads, in the failure message and in `wait-chains.txt`, which say what a hung window's thread waits on; the mockapp test's failure does not. A roadmap follow-up under M4.

## Mockapp tests under `cargo xtask ci`

- `call_counts::uia_focus_chains_stop_below_the_process_boundary` and `shutdown::a_remote_operation_in_progress_finishes_before_the_outpost_shuts_down`
  - How: fail intermittently under `cargo xtask ci`. The first's counts hold with a 1.5 s pause between its steps, so the lapse of UI Automation's copy of an element's runtime id (`docs/uia-bugs.md`) is not its cause.
  - Cause: not found.
  - Fix: [the M4.1 follow-ups](roadmap.md#follow-ups-from-m41), the entry on these tests.

## Synthesizer host tests on GitHub's runner

- `verbatim-synth-host`'s `hosting` tests `a_hosted_synthesizer_streams_audio_and_marks_in_order`, `a_cancelled_utterance_ends_without_the_hosts_answer_and_the_host_carries_on` and `an_utterance_cancelled_before_any_audio_ends_without_any_and_the_host_carries_on`
  - How: all three failed together under `cargo xtask ci` with "the synthesizer host sent nothing for 10s" (run 41267826098, 2026-11-01, a documents-only commit); the runs before and after it passed.
  - Cause: not found. The speech platform's first activation in a fresh runner session can take more than ten seconds, which is why the unit-test jobs run `cargo xtask warm-speech` first; whether that warm-up did not hold here is not known.
  - Fix: none yet.

## The mixer's allocation test under load

- `verbatim-audio`'s `mixer` test `the_audio_thread_allocates_and_frees_nothing_while_it_plays`
  - How: times out waiting for a playback event, "a playback event arrives: Timeout", in its warm-up's first utterance, whose end never comes after the harness plays its frames (`exercise` in `crates/verbatim-audio/tests/mixer.rs`).
  - How often: 1 of 640 runs with eight copies running at once on the 12-core development machine, both before and after f3284cf9 fixed `settle` (2026-11-02); never seen alone.
  - Cause: not found. The tests run the real audio and delivery threads and judge from outside when the mixer is idle, so it is not known whether this is the harness's guess or a lost wake in the mixer.
  - Fix: [M6.3](roadmap.md#m63--speech-and-audio-pipeline), the mixer's decision tests driven pass by pass on the test thread, where it is reproduced or ruled out.

## Failed group removal tests reading the count too early

- `mockapp`'s `failure_paths` test `a_group_whose_removals_all_failed_is_removed_when_the_registering_thread_ends`
  - How: its first assertion, "the old element's group outlives the close", found the two applications' advised groups at (1, 1) where (1, 0) was expected (`crates/mockapp/tests/failure_paths.rs`, line 233): right after `close`, the new element's group was still advised.
  - How often: once, in the `ci-arm64` job of run 41333390347 (2026-11-01, a commit adding only documents and `experiments/`); the same code passed `ci-arm64` in run 41331655970 just before it, and passed `cargo xtask ci` locally.
- `mockapp`'s `failure_paths` test `a_group_a_move_failed_to_remove_is_removed_at_close`
  - How: its assertion "the close removed both groups" found the advised groups at (1, 0) where (0, 0) was expected (line 209): right after `close`, the old window still counted one registration.
  - How often: once in six runs of `cargo xtask faults` on the development machine (2026-11-03); five runs in a row passed after it.
- Cause, for both, likely but not shown: `Registration::close` (`crates/verbatim-uia/src/subscribe.rs`) returns when `RemoveEventHandlerGroup` returns, while mockapp's count falls only when UI Automation calls `AdviseEventRemoved` on its provider (`crates/mockapp/src/uia.rs`), and the tests read the count once, with nothing waiting for that call.
- Fix: mockapp signals each change of its count and the tests wait for the count they expect, as its `RELEASED` wait does, so a count that never arrives is a defect in the close's wait (a follow-up of the owner endings build).

## Scenarios that fail on GitHub's runner, skipped there

These are `local_only` in the registry. The `e2e` job deselects them (`--skip notepad_ --skip settings_search_suggestions --skip settings_combo_box_expanded`), and they pass locally. No fix planned for any of them: the runner's image is not ours.

- The `notepad_` scenarios: the runner has classic Notepad, a Win32 edit control, not Windows 11 Notepad. Editing is also covered in the harness's Windows Forms text box.
- `settings_search_suggestions`: the runner's Settings raises no selection event for a first suggestion as the list opens, so "Suggestions available" plays to completion there, while the scenario expects the first suggestion, which this machine's Settings selects as the list opens, to cut it off.
- `settings_combo_box_expanded`: the runner's Settings opens its Date and time page with the focus on "Location Settings link", where this machine's opens it on "Region button", which the scenario's opening speech asserts.
