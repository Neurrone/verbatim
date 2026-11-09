# Known failing and flaky tests

The one list of tests and scenarios known to fail intermittently, or consistently, for a known reason. Check it before debugging a failure, and before rerunning or running a baseline: a failure that matches an entry is already understood. Leave a listed scenario out of a related-scenario batch, as `CLAUDE.md` says.

Each entry gives the exact name, how it fails, how often and where, the cause when known, and when the fix is expected. Failure counts are from the dates given, on the development machine ("locally") or GitHub's runner. When a fix lands, delete the entry; history lives in git. When a new failure is explained, add an entry here and link to the work that owns the fix.

## Windows Terminal's first focus, ancestor walk past its bound

Scenarios: the `windows_terminal_` scenarios generally, for example `windows_terminal_progress` and `windows_terminal_tabs`.

- How: a new Windows Terminal window's first focus speaks the title without "window". On GitHub's runner (run 41163500778, 2026-10-31) the expected "ready>" was heard as "verbatim-e2e-spoken-password-mvvtyykb terminal".
- How often: `windows_terminal_progress` and one other scenario failed this way in one full run on GitHub's runner, after the crate split (run 41211072805, 2026-10-31). `windows_terminal_tabs` and two other scenarios failed this way after the caret watch build (run 41225371284, 2026-10-31), and two scenarios after the first determinism build (run 41233417983, 2026-10-31). On 2026-10-31, with the reducer build, two Windows Terminal flood scenarios since retired failed 10 of 10 and 4 of 9 locally.
- Cause: the listener's ancestor walk for the focus passes its 500 ms bound, so the window's entry is missing from the chain.
- Fix: built with the focus pipeline rework's third build (115d825f, 2026-11-08), which removed the walk's bound, so a read waits for the application; the entry is deleted once the Windows Terminal scenarios pass on GitHub's runner.

## Windows Terminal launch speech, not this cause

- Scenarios: the `*_up_typing` (since merged into `*_line_key_flood`), `*_typing` and `*_commands` Windows Terminal scenarios.
- How: the launch focus sometimes speaks "... terminal" without "... window", and the desktop's announcement is sometimes cancelled.
- Cause: not found; possibly the first entry's.
- Fix: [M6.4 terminal residuals](roadmap.md#m64--terminal-residuals).

## Windows Terminal's second terminal identity repeats the terminal at launch

- Scenarios: `windows_terminal_editing` (run 42429967492, 2026-11-08).
- How: the opening's terminal focus came under one identity, the second, then the first again, so it was said three times where the opening accepts one repeat.
- Cause: Windows Terminal raises focus from a second runtime identity ([UIA faults](../books/dev/src/reference/uia-bugs.md), "Windows Terminal raises focus from a second runtime identity"); Verbatim's rule dropping those repeats is not built, and the scenario accepts one repeat of the terminal.
- Fix: [M5](roadmap.md#m5--extensions-v1), the Terminal built-in's rule for the second terminal identity.

## Settings' suggestions list replaced as it opens

- Scenarios: `settings_search_suggestions` (local only), 2 of 13 runs on 2026-11-09.
- How: "Suggestions available" plays to completion where the scenario expects the first suggestion to cut it off.
- Cause: Settings replaces the list's items as it opens, after raising the selection, so the selected element is in none of the elements the focus controls, and nothing is said, as `docs/parity/changes-and-selection.md` records.
- Fix: none in Verbatim.

## Settings' page focus never delivered by UI Automation

- `settings_system_page` on GitHub's runner (run 42492633218, 2026-11-08)
- `settings_system_page` on GitHub's runner (run 42514816068, 2026-11-09), the same failure
  - How: "Settings" was said for the frame, then nothing for 75 s; "Settings window" was never queued.
  - Cause: the page's `Windows.UI.Core.CoreWindow` (hwnd 328450, `SystemSettings.exe`) raised two MSAA focus events, which the outpost dropped because UI Automation owns the window, as NVDA drops them; UI Automation delivered no focus event for the page to the listener, whose handler had delivered the frame's focus two seconds before. A focus UI Automation never delivers is lost, as in NVDA ([ADR 0001](../books/dev/src/reference/adr/0001-focus.md), its limits).
  - Fix: none in Verbatim; passed on GitHub's runner in the two runs before it.

## Single failures in the first grouped runs (runs 41693694156 and 41697089785, 2026-11-03)

Recorded by Claude, 2026-11-04, pending Dickson's acceptance.

- `text_box_review_cursor`
  - How: Verbatim+Alt+Home (review the selection's start) was answered "unanswered" by the outpost's text read, so nothing was said within 15 s.
  - Cause: the harness's Windows Forms text box (a Windows PowerShell window) did not answer `EM_GETSEL` within the outpost's fixed 500 ms message limit (`MESSAGE_TIMEOUT_MS`, `crates/verbatim-ia2/src/edit.rs`); its interface thread had stalled twice at the scenario's start too. The menu guard changed nothing in the text box. NVDA waits for the same message until its watchdog judges the application hung, so a slow but live application is still read.
  - Fix: built with the slow applications build (d13eaef2 to d7b2962d, 2026-11-07): a window message has no time limit of its own and is waited for until the application answers or Windows judges its window hung, as NVDA does. Ten local runs passed, none slow; the entry is deleted once a run's log shows the slow read waited for and the speech heard.
- `windows_terminal_control_flood`'s command marks steps (run 41697089785, when they were `windows_terminal_command_marks`)
  - How: 2.0 s after the second Verbatim+Alt+Up (the previous command mark), Verbatim played its "application not responding" sound where the mark's line "ready> .\long.ps1" was expected.
  - Cause: the mark lay above the rows Core keeps, so Core asked the outpost to search the terminal (`TerminalFind`); the search, a remote operation in Windows Terminal (`crates/verbatim-uia-rops/src/terminal_find.rs`), took 3.2 s, past the outpost's 2 s text deadline (`TEXT_DEADLINE`), so the worker was abandoned, and `finish` (`crates/verbatim-app/src/main.rs`) plays the sound for any abandoned query, without asking whether Windows judges the window hung. Windows Terminal was answering throughout. NVDA has no such sound.
  - Fix: built with the slow applications build (2026-11-07): the 2 s text deadline is gone, a query waits up to the 11 s handling deadline, and the sound plays only when Windows judges the window hung and its application holds attention (Dickson, 2026-11-06). Ten local runs passed, none slow; the entry is deleted once a run's log shows a search past 2 s answered and spoken. Seen again in run 42487419395 (2026-11-08): after the step's Verbatim+Alt+Up, outpost 4 reported Windows Terminal (pid 6224) not responding as Windows judged its window hung, so the sound played as the rule says, where the mark's line was expected; the slow search remains the cause. The search's cost in a real Windows Terminal, never measured, is to be measured and fixed with [M6.4 terminal residuals](roadmap.md#m64--terminal-residuals)' measurements.

## Windows Terminal busy opening its tab switcher

Recorded by Claude, 2026-11-04, pending Dickson's acceptance.

- `windows_terminal_tabs` (run 41859263092)
  - How: at Control+Shift+Tab, "sound: app-not-responding" was heard where "Tab switcher grouping", "list" and the first tab were expected.
  - Cause: the key, a caret key in the terminal, opened a caret watch; Windows Terminal did not answer the watch's read for 2 s while it opened its tab switcher, so the outpost abandoned its worker ("a call passed its deadline", 15:43:34.285), and Core plays the sound for any abandoned query, without asking whether Windows judges the window hung.
  - Fix: whether Control+Shift+Tab opens a caret watch in a terminal at all is settled with [M6.4 terminal residuals](roadmap.md#m64--terminal-residuals) (Dickson, 2026-11-04); the watch now waits the 11 s handling deadline and the sound plays only for a window Windows judges hung, built with the slow applications build (2026-11-07). Ten local runs passed, none slow; the entry is deleted once a run's log shows the slow watch answered.

## The caret watch's baseline anchor released while it waits

Recorded by Claude, 2026-11-04.

- `text_box_caret_keys` (run 41848692720)
  - How: at Control+V on the start of "alpha beta gamma", nothing was said within 15 s where "mangoalpha beta gamma" was expected; the outpost's answer to the key's watch had no `same_line`, so Core, judging a key by where the caret landed, spoke nothing.
  - Cause: the watch's baseline is the newest caret the outpost reported before the key (`baseline`, `crates/verbatim-outpost/src/text/mod.rs`), here a caret event's anchor that no request names. The paste moved the caret while the watch read it, so the watch waited for the caret event, and the shell released the anchors no live request names, that one among them; when the event came the baseline no longer resolved. A run whose first read is not overtaken answers before the release and passes.
  - Fix: [the caret and selection rework](roadmap.md#the-caret-and-selection-rework), whose one baseline per text node lives as long as the focus Core holds (Dickson, 2026-11-04).

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

- `explorer_folder_window`, Explorer's folder window not responding (locally 2026-10-31; GitHub run 42654703245, where the window took the foreground and no focus event followed)
  - How: nothing is spoken after the desktop's "Recycle Bin 1 of 13"; the folder window, opened in a new `explorer.exe` process, never takes the foreground, its UI thread stops answering about 1.5 s after it opens (an arbitration probe of its `ShellTabWindowClass` window times out at 3 s, an MSAA call waits 13 s), and it does not close within 15 s, so the next scenarios fail at launch on the window left open. Once in 13 runs, with the keep-id fix; the next 10 passed.
  - Cause: not found. Verbatim's outpost was the one waiting on Explorer; no Verbatim call was found that Explorer waited on.
  - Fix: none yet.

- `explorer_folder_window` opening its folder window (locally, 2026-11-09, at f51b89bb 1 of 3 runs and at 9e4fc9ea 0 of 3 run alone and 6 of 6 run straight after a batch of other scenarios; GitHub run 42602389212)
  - How: the new window's first focus is sometimes on its `ShellTabWindowClass` child, named without " - File Explorer", rather than on `CabinetWClass`, or the speech that follows differs; the scenario then fails on its opening utterances.
  - Cause: not found. The order of Explorer's first focus events changes with what ran before; no Verbatim code on the path differs between those runs.
  - Fix: none yet.

## Terminal scenarios, single failures on GitHub's runner

- `conhost_long_lines` (run 41333390347)
  - How: after Enter, "r" is heard where "part1 part2 ... part10" is expected; the snapshot then has the caret on an empty line and "ready>" on the screen's first row.
  - Cause: not found; the typed-character rules (`crates/verbatim-core/src/terminal_model.rs`) are suspected, not shown. Once in 32 runs.
  - Fix: [M6.4 terminal residuals](roadmap.md#m64--terminal-residuals).

## A new console host not responding at its outpost's first call

Scenarios: `measure_keys_conhost` and `measure_keys_conhost_unmoved` (release build, 2026-10-31, on main's code with temporary log lines), and `conhost_full_history_flood`, since retired (debug build, 2026-11-01, 1 of 3 with the first determinism build; its outpost was killed at shutdown, and the next scenario, `notepad_editing`, found no Notepad window within 30 s, then passed 10 of 10).

- How: setup fails with "no window titled ... opened within 30s"; the PowerShell console's window is hidden and not responding, or not shown at all. The outpost given the new `conhost.exe` starts watching it, and its worker's first call passes its 10 s deadline about 10 s later and finishes only at shutdown, 2 to 35 s after. 3 of 20 runs; in one, a crate split build's unit tests were running on the same desktop.
- Cause: not found. In two `conhost_full_history_flood` failures on 2026-11-01 whose artifacts were kept, Verbatim waited on the console host and not the reverse: the window never answered the outpost's first message (the arbitration's `WM_GETOBJECT` probe timed out at 3 s, where a healthy console answers in about 0.4 s), the outpost's worker then blocked in an MSAA read until the outpost was killed, and the console host stayed hung after every Verbatim process had exited. Both runs also had Explorer's notification area not answering (`Shell_NotifyIcon` timed out at Verbatim's start) and the Widgets process not responding, which no passing run showed. What the console host's window thread waited on is not in the logs.
- Fix: none yet. The harness now records, when this wait fails, the wait chain of each not-responding window's thread and of the launched process's threads, in the failure message and in `wait-chains.txt` ([the end-to-end guide](../books/dev/src/how-to/e2e.md)), and the outpost's abandoned-worker warning names the entry its worker had in hand; the next failure's artifacts are to be read for what the thread waits on. A roadmap follow-up under M4.

## A new application's window not shown in time

Scenarios and tests: `notepad_editing`, `notepad_selection_keys` and `notepad_typed_words` locally; mockapp's `remote_ops::a_windows_forms_menu_item_is_checked_from_the_remote_read` under `cargo xtask ci` on GitHub's runner; and `return_from_a_held_application` and `text_box_page_keys` in one group of run 41693694156 (2026-11-03), where the harness's PowerShell-hosted Windows Forms window never showed (pid 3612) or stayed minimized and not responding (pid 3032), the foreground on Program Manager. A setup that fails this way leaves its host running until the next scenario's start ends it (Claude, 2026-11-04, pending Dickson's acceptance). Also `text_box_password` and `text_box_review_cursor` in group 3 of run 42262303061 (2026-11-06), the PowerShell-hosted Windows Forms window never shown (pid 7088) or minimized and not answering its restore (pid 1584), with no Verbatim log naming either process; and `wpf_password` in group 1 of run 42016960834 (2026-11-05): the harness's PowerShell-hosted WPF window never showed, the foreground on Program Manager. Its wait chain shows the one PowerShell thread blocked with no cycle; the process had loaded WPF's PresentationFramework, then `d3d9.dll` and `dxcore.dll` last, after 437 ms of processor time; and no Verbatim log names the process, so no outpost or the listener had called into it.

- How: setup fails with "no window titled ... opened within 30s" and no Notepad window among the visible ones, before any key is sent; the mockapp test fails with "the window is shown: Timeout". Locally five times on 2026-11-01, in bursts of up to three runs in a row, then passing; on the runner once (run 41254405706). Twelve runs in a row, six idle and six under a release build's load, all passed.
- Cause: not found. Load from a concurrent build did not reproduce it. The console host entry above, whose kept artifacts show the new window hung on its own with Explorer's notification area also not answering, may be the same; no Notepad failure's artifacts have been kept yet.
- Fix: none yet. The next harness failure carries the wait chains of the not-responding windows' threads and of the launched Notepad's threads, in the failure message and in `wait-chains.txt`, which say what a hung window's thread waits on; the mockapp test's failure does not. A roadmap follow-up under M4.

## Scrollback scenarios after a long flood, locally

Recorded by Claude, 2026-11-04, pending Dickson's acceptance.

- Scenarios: `windows_terminal_scrollback_overflow_during_group` and `conhost_scrollback_overflow_during_group`, locally, when they run after `conhost_flood`.
- How: Windows Terminal's window never appears ("no window titled ... opened within 30s"), and the console host's first focus is said without "window". 3 of 3 times each in a round of the terminal scenarios, and once more with `conhost_flood` run alone just before, by a separate command with a fresh agent; both pass run alone.
- Cause: this machine's display adapter, a VirtIO display-only driver with no 3D acceleration, leaves the Desktop Window Manager to compose the screen on the processor. During `conhost_flood` `dwm.exe` used two whole processors of the twelve (200%), and went on at 190 to 200% for about 60 seconds after the scenario and its recording had ended, working off the flood's redraws, before falling to its idle 12 to 15% (sampled every 2 seconds, 2026-11-04). A scenario started in that minute waits on DWM: the launch evidence (`launched-process-<pid>.txt`, `wait-chains.txt`) shows Windows Terminal's main thread waiting 30 seconds on an ALPC call to `dwm.exe`, having used 155 ms of processor time. GitHub's runner, which passes these scenarios, was not measured.
- Fix: none yet. A graphics adapter with 3D acceleration for the VM would remove the cause; a harness wait for DWM to be idle before a launch would only hide it.

## Mockapp tests under `cargo xtask ci`

- `call_counts::uia_focus_chains_stop_below_the_process_boundary`
  - How: recorded as failing intermittently under `cargo xtask ci`, with no failure message kept; the only failures on record came while the test was being rewritten (2026-10-29 and 2026-10-30). On 2026-11-03, at 29237e17, it passed 456 of 456 runs: 100 alone, 50 with its binary, 6 runs of mockapp's whole suite, 120 as twelve copies at once, 20 binaries as four copies at once, and 160 under twelve busy processes. Its counts also hold with a 1.5 s pause between its steps, so the lapse of UI Automation's copy of an element's runtime id (`books/dev/src/reference/uia-bugs.md`) is not its cause.
  - Cause: not found, since it does not reproduce.
  - Fix: none until it fails again; its failure message is to be kept, since which count moved names the call to look at.
- `call_counts::msaa_form_focus_costs_exactly`, `call_counts::msaa_tree_view_costs_exactly` and `call_counts::uia_combo_box_costs_exactly`, under load
  - How: `msaa_form_focus_costs_exactly` failed 7 of 20 runs with four copies of it running at once (2026-11-03): an extra window-title entry at the top of its chain; under twelve busy processes the other two failed as well. Alone they pass.
  - Cause, likely but not shown: the MSAA walk passed its bound under load, and the title was doubled when it did.
  - Fix: the focus pipeline rework's third build removed the outpost's walk bound (b94fa1d8, 2026-11-08); the entry is deleted once the tests pass under load. Until then `cargo xtask ci` runs `call_counts` alone.
- `focus_reports::an_msaa_client_area_reached_by_a_walk_carries_its_window_and_its_content_none` and `focus_reports::an_msaa_ancestor_reached_with_no_identity_string_is_its_windows_client`, under load
  - How: both failed in 1 of 4 runs of the whole unit-test suite under cargo-nextest, many tests at once while another build compiled (2026-11-03): the window's title, and in the second the window's client area, above the dialog at the top of the chain. Alone, the binary passed 3 of 3 under nextest and 3 of 3 under its own runner.
  - Cause, likely but not shown: as for the `call_counts` tests above.
  - Fix: as for the `call_counts` tests above. Until then `cargo xtask ci` runs `focus_reports` with nothing else beside it (`.config/nextest.toml`).

## Synthesizer host tests on GitHub's runner

- `verbatim-synth-host`'s `hosting` tests `a_hosted_synthesizer_streams_audio_and_marks_in_order`, `a_cancelled_utterance_ends_without_the_hosts_answer_and_the_host_carries_on`, `an_utterance_cancelled_before_any_audio_ends_without_any_and_the_host_carries_on` and `onecore_speaks_a_raised_capital_within_one_utterance`
  - How: the first three failed together under `cargo xtask ci` with "the synthesizer host sent nothing for 10s" (run 41267826098, 2026-11-01, a documents-only commit), the runs before and after it passing; the second and fourth failed the same way in the x64 job of run 41853488071 (2026-11-04), whose ARM64 job passed them.
  - Cause: not found. The speech platform's first activation in a fresh runner session can take more than ten seconds, which is why the unit-test jobs run `cargo xtask warm-speech` first; whether that warm-up did not hold here is not known.
  - Fix: none yet.

## The mixer's allocation test under load

- `verbatim-audio`'s `mixer` test `the_audio_thread_allocates_and_frees_nothing_while_it_plays`
  - How: times out waiting for a playback event, "a playback event arrives: Timeout", in its warm-up's first utterance, whose end never comes after the harness plays its frames (`exercise` in `crates/verbatim-audio/tests/mixer.rs`).
  - How often: 1 of 640 runs with eight copies running at once on the 12-core development machine, both before and after f3284cf9 fixed `settle` (2026-11-02); never seen alone.
  - Cause: not found. The tests run the real audio and delivery threads and judge from outside when the mixer is idle, so it is not known whether this is the harness's guess or a lost wake in the mixer.
  - Fix: [M6.3](roadmap.md#m63--speech-and-audio-pipeline), the mixer's decision tests driven pass by pass on the test thread, where it is reproduced or ruled out.

## Scenarios that fail on GitHub's runner, skipped there

These are `local_only` in the registry. The `e2e` job leaves them out of its groups (`cargo xtask e2e-plan`), and they pass locally. No fix is planned for them: the runner's image is not ours.

- The `notepad_` scenarios: the runner has classic Notepad, a Win32 edit control, not Windows 11 Notepad. Editing is also covered in the harness's Windows Forms text box.
- `settings_search_suggestions`: the runner's Settings raises no selection event for a first suggestion as the list opens, so "Suggestions available" plays to completion there, while the scenario expects the first suggestion, which this machine's Settings selects as the list opens, to cut it off.
- `settings_combo_box_expanded`: the runner's Settings opens its Date and time page with the focus on "Location Settings link", where this machine's opens it on "Region button", which the scenario's opening speech asserts.

## `alt_tab_switcher` on a parked local desktop

- `alt_tab_switcher` locally (10 of 10 runs failed at 6e6c1efa and at bce10932, 2026-11-09)
  - How: the agent's `SwitcherItems` found no item with the focus within 30 s.
  - Cause: on this Windows build the switcher, class `XamlExplorerHostIslandWindow`, is open with the selection drawn on its first item, while UI Automation focus stays on the host's empty `InputSite` pane; `switcher_items` (`crates/verbatim-agent/src/focus.rs`) accepts only focus on an element whose parent is `SwitchItemListControl`, and a search of the one switcher window's descendants, in the control view and the raw view, finds no such list for 30 s.
  - Fix: none yet. Why UI Automation exposes no list here is not established. Verbatim is not involved, since the check is the agent's own read.

## `table_cells`'s first focus

- `table_cells` locally (one failure in 10 runs, 2026-11-09, after the fix below; 2 of 16 before it)
  - How: the window is spoken, then nothing; Core received a UI Automation focus on the window and none on the first cell.
  - Cause: a focus event raised before a UI Automation client has reached the window's provider goes nowhere, shown by the listener's log of `msaa_flood_then_focus`, where the window's was the only focus event received. mockapp now raises its first focus once a client has asked for its root provider (`crates/mockapp/src/window.rs`, `client_contacted`), which took that scenario and `foreground_progress_bar` to 10 of 10. The remaining failure is not explained.
  - Fix: none yet for the remainder.
