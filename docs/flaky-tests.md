# Known failing and flaky tests

The one list of tests and scenarios known to fail intermittently, or consistently, for a known reason. Check it before debugging a failure, and before rerunning or running a baseline: a failure that matches an entry is already understood. Leave a listed scenario out of a related-scenario batch, as `CLAUDE.md` says.

Each entry gives the exact name, how it fails, how often and where, the cause when known, and when the fix is expected. Failure counts are from the dates given, on the development machine ("locally") or GitHub's runner. When a fix lands, delete the entry; history lives in git. When a new failure is explained, add an entry here and link to the work that owns the fix. Every entry is fixed, names the build of the simplification phase that reaches its cause, or has its test taken out of GitHub's runs with the reason and what would let it run there again (Dickson, 2026-11-20); a local-only scenario gives the same reason in its entry's `.local_only("reason")` in `crates/verbatim-e2e/src/registry.rs`.

## Settings' suggestions list replaced as it opens

- Scenario: `settings_search_suggestions_reentered`, local-only, which holds the steps of `settings_search_suggestions` since build 6 merged it there; `settings_search_suggestions` failed 2 of 13 local runs on 2026-11-09.
- How: "Suggestions available" plays to completion where the scenario expects the first suggestion to cut it off. On GitHub's runner "Suggestions available" played to completion the same way (run 39558690998), and again in 5 of 5 dispatched runs (run 45920535477, 2026-11-27).
- Cause: Settings replaces the list's items as it opens, after raising the selection, so the selected element is in none of the elements the focus controls, and nothing is said, as in NVDA (`docs/parity/changes-and-selection.md`, pinned by mockapp's `a_stale_selection_after_a_replacement_reports_nothing_and_an_ordinary_one_the_new_item`). The failures' logs are not kept, so this is inferred from the code and the parity record. When Settings replaces its list is outside Verbatim's control, and accepting either outcome would break the exact-assertion rule.
- Fix: none in Verbatim. It stays off GitHub until dispatched runs show the runner's Settings keeping its first selection; locally they run in build 6's scheduled run of the local-only scenarios.

## UI Automation focus never delivered on GitHub's runner

- Scenario: `windows_terminal_long_lines` (run 44414430624, 2026-11-19), local-only until 5 of 5 dispatched runs passed (run 45920543950, 2026-11-27), so it runs on GitHub again (Claude, 2026-11-27). The same fault failed `settings_system_page` 8 times (2026-11-08 to 2026-11-20) and `windows_terminal_close_tab` once (run 44465917467); build 6 removed the first, its path covered by `settings_combo_box_expanded` and `settings_search_suggestions_reentered`, and merged the second's tab closing into `windows_terminal_tabs`, which runs on GitHub.
- How: in `settings_system_page`, "Settings" was said for the frame, then nothing for 75 s, and "Settings window" was never queued; in the terminal scenarios nothing is said after the new Windows Terminal window opens.
- Cause: UI Automation never delivered the focus to the listener's desktop-wide handler. In run 44571401780 and run 44465917467 the frame's focus arrived and was spoken; about 1.6 s later the page's `Windows.UI.Core.CoreWindow` (`SystemSettings.exe`) raised MSAA focus events, which the outpost rightly left to UI Automation, as NVDA does, and the listener's UI Automation layer, upstream of every filter of the listener's, logged no focus event and no window-opened event for 75 s. The terminals' listener logs hold one or two UI Automation events, none on the new terminal. This is the fault "A new process's focus events reach the desktop-wide handler late or not at all" ([UIA faults](../books/dev/src/reference/uia-bugs.md)); a focus UI Automation never delivers is lost, as in NVDA ([ADR 0001](../books/dev/src/reference/adr/0001-focus.md), its limits; Dickson, 2026-11-07).
- Fix: none in Verbatim; a recurrence on GitHub is matched to this entry.

## `windows_terminal_control_flood`'s command marks steps

- `windows_terminal_control_flood` on GitHub's runner (run 41697089785, 2026-11-03, when the steps were `windows_terminal_command_marks`, and run 42487419395, 2026-11-08)
  - How: after Verbatim+Alt+Up (the previous command mark), Verbatim played its "application not responding" sound where the mark's line "ready> .\long.ps1" was expected.
  - Cause: the mark lay above the rows Core keeps, so Core asked the outpost to search the terminal, then by a search program of its own bounded at 100 `FindText` calls for both ways together. In run 42487419395 the search took 6.4 s in Windows Terminal (`elapsed_us=6404203`), Windows judged the window hung, and the sound played as the rule says.
  - Fix: Core's search is now its terminal reader's batch, a `FindText` each way with 100 calls each (`verbatim_core::terminal::reader::search_batch`; Dickson, 2026-11-26), and this scenario's marks steps on the runner are its measurement against this 6.4 s, read from the artifacts whether the scenario passes or not. The test is not loosened.

## Verbatim's GUI thread stalls as the Remove Theme confirmation opens

- `theme_panel` on GitHub's runner (run 44883440325, 2026-11-21, at 5ce7fcb2; it passed in the 8 earlier runs whose artifacts were read, runs 44562491188 to 44858971330)
  - How: after Enter on "Remove button", "Remove Theme dialog Remove the theme Default copy? This cannot be undone." was never queued within 75 s, and nothing else was said.
  - Cause: Verbatim's own GUI thread (`verbatim.exe`, thread 672) stopped handling messages for about 5.2 s inside the confirmation's `ShowModal` (`crates/verbatim-gui/cpp/gui.cpp`, the Remove handler). Its last event was the hidden frame's state change at 08:20:25.507, and the next was the Theme dialog's at 08:20:30.675 (`listener.log`). In passing runs the dialog is said 100 to 420 ms after the key. Windows ghosted the hung windows at 08:20:30.69 and kept the ghost until 08:20:31.507. The task dialog's foreground and focus events came at 08:20:31.486 to .489, while its window still read as hung, so the listener's hung-window drop (`hang_tested`, `crates/verbatim-listener-logic/src/tell.rs`, from 5b4d1ad8, before build 4) took every one of them. The UI Automation focuses on the dialog and its No button reached Core, which read them as nothing: Core's verdict for the `#32770` dialog is MSAA. Replaying the run's flight recorder (`cargo xtask replay`) shows each input from line 464 to 476 producing only a timer, with no request sent. Core and both readers were idle through the stall: the Verbatim outpost's log shows no request from 08:20:25.11 until shutdown at 08:21:40. So no reader or Core caused the stall, and build 4 changed no GUI code. What the GUI thread waited on inside the task dialog's creation is not shown.
  - Fix: the silence is fixed by the report of a hung window (ADR 0003, "A frozen window"): the listener reports the dropped dialog's window, so Core watches it and reads the dialog once it answers, about half a second after the stall ends. If the stall recurs, the run now fails at the Enter step on "sound: app-not-responding", played as the Theme dialog joins the recovery set at about 30.68, and the harness records the not-responding windows' wait chains at once. The stall's own cause is still open. Verbatim's log now times the confirmation's `ShowModal`, which `theme_panel`'s teardown requires, so a dispatched run, `gh workflow run ci.yml -f scenario=theme_panel -f repeat=20`, measures how often the stall happens and how long it lasts. A stall in Verbatim's own code then goes to the build that owns that code; a stall in Windows' task dialog goes to Dickson.

- `quit_inside_a_file_dialog` on GitHub's runner (run 45772404137, 2026-11-26, at 5809c5b5; it passed at 18debaf6)
  - How: after Enter on "Import...", only "sound: app-not-responding" was heard where the Import a Theme dialog's four utterances were expected.
  - Cause: the same stall in a second native dialog. Verbatim's GUI thread handled no message for about 4.5 s inside the file dialog's `ShowModal` (`gui.cpp`, `AskFile`); the listener reported the dialog hung, Core played the sound, and the dialog answered about 6.2 s after the key, after the step had failed.
  - Fix: owned by build 6 with the entry above. The file dialog's `ShowModal` is timed in Verbatim's log like the message boxes.

- `quit_inside_a_file_dialog` on GitHub's runner again (run 45805762032, 2026-11-26): the third stall, about one in 30 runs on GitHub and none in 40 dispatched runs locally.
  - How: as above, after Enter on "Import...".
  - Cause: the file dialog's `ShowModal` took 8,122 ms (`stderr.log`). The wait chain read while it was stalled, `not-responding-wait-chains.txt`, says only "window 0x10292 (pid 5380), thread 3860: thread 3860 of verbatim.exe pid 5380 (blocked); no cycle": the GUI thread is blocked on something Wait Chain Traversal does not follow, so no other thread or process is named.
  - Fix: the cause is still open, and the next stall names it. When the sound follows a window of Verbatim's own, the harness now dumps Verbatim straight after the wait chains, as `verbatim-<pid>.dmp` in the scenario's artifacts with every thread's stack, and the job uploads its `verbatim.exe` and `verbatim.pdb` as `e2e-symbols-<group>`; `books/dev/src/how-to/e2e.md`, "Reading Verbatim's minidump", says how to read the GUI thread's stack from them. A stall in Verbatim's own code then goes to the build that owns that code; a stall inside Windows' dialog goes to Dickson.

- `quit_inside_a_file_dialog` on GitHub's runner (run 45845095452, 2026-11-26, at 520ca5f9)
  - How: "sound: app-not-responding" where the file dialog's opening was expected; the dialog's `ShowModal` took 54,762 ms.
  - Cause: the same stall, and again desktop-wide: the wait chains list a blocked thread of `dwm.exe` beside Verbatim's GUI thread, and the harness's own dump of Verbatim failed because the agent could not read the process within 10 s (`verbatim-696.txt`). So the stall is the runner's, not the dialog's; what blocks is still not known, since the dump is what would have named it.
  - Fix: none yet; the next stall names more. Every failed body now records the machine's load after the wait chains, as `runner-load.txt` in the scenario's artifacts: processor, memory and disk-queue use, the 12 processes that spent the most processor time over 1 s, and each `dwm.exe`'s processor time and threads, read with no window call so a frozen desktop cannot hold it up. The dump of Verbatim now waits up to 60 s, longer than any stall seen, and `verbatim-<pid>.txt` says how long it took. `books/dev/src/how-to/e2e.md`, "The runner's load", says how to read them.

- `quit_inside_a_file_dialog` on GitHub's runner (run 45886935399, 2026-11-27, at 81ecf771), the fifth stall
  - How: as above; the file dialog's `ShowModal` took 22,895 ms.
  - Cause: this time the dump was written (`verbatim-904.dmp`, read with cdbX64 and the job's uploaded symbols). Verbatim's GUI thread waited on no lock: it was in `comdlg32!AutoComplete`, then `combase!CoCreateInstance`, then at the entry of `shell32!CACLIShellFolder_CreateInstance`, the file name box's autocomplete creating its shell folder object. Every other thread of Verbatim's was idle in its usual wait. The runner had been up 11 minutes and Verbatim 39 s, so the thread was running the shell's first-use code, most likely held up by page-ins on a cold, stalled runner, not by Verbatim.
  - Fix: none in Verbatim's code. The machine's load the harness now records on a failure (`runner-load.txt`) is what can confirm disk or processor pressure the next time.

## A stand-in's window that does not answer after it shows

- `text_box_page_keys` on GitHub's runner (run 45821984446, 2026-11-26, at 1fc0c0f6)
  - How: "sound: app-not-responding" where the window's title and text box were expected, at the scenario's first step.
  - Cause: the runner, as far as the artifacts show. The stand-in's form showed and took focus, then did not answer its first MSAA call for about 8 s (0.14 s in the passing run 45805762032); the outpost's probe timed out at 3 s and the sound is correct. The request made no UIA call and nothing fell back. `not-responding-wait-chains.txt` lists two blocked threads with no cycle: the form's UI thread and one of `dwm.exe`, which draws every window, so the desktop as a whole may have stalled. What they waited on is not known.
  - Fix: none yet. The next failure records the machine's load: `runner-load.txt` for a failed body, and `runner-load-launched-process-<pid>.txt` beside `wait-chains.txt` when a launched window did not appear in time, as for the entry above ("The runner's load" in `books/dev/src/how-to/e2e.md`). When it names a process busy across the runner, or shows the runner idle while `dwm.exe` and the window wait, the stalls of Verbatim's own dialogs above are read again in that light.

- `wpf_password` on GitHub's runner (run 45884584255, 2026-11-27, at 2b3959a5)
  - How: "… window" where "… dialog", "User name edit", "dickson" was expected.
  - Cause: the same runner stall. The compiled WPF host took about 33 s to show its window; Verbatim's first focus read probed its window, timed out after 3 s, fell back to MSAA and read a bare window. The next read found the host answering, but the scenario had ended. This is the mechanism of the entry 39091874 removed when the hosts were compiled, so compiling them was not enough on a stalled runner.
  - Fix: as for the entries above.

- `second_application_and_verbatim_menu` on GitHub's runner (run 45922422116, 2026-11-27, at fa61bdaa)
  - How: setup failed before any speech: the stand-in's window (`text_box_form.exe`) did not handle its restore within 5 s and was listed as not responding.
  - Cause: the same runner stall, the first with the machine's load recorded: the disks' queue averaged 2.89 and stood at 3.00, while one `svchost.exe` (pid 4496) spent 22.2% of the processors and the processor was 33.5% busy; the stand-in had spent 140 ms of processor time. So the runner's disk was busy, most likely for that service host; which services it runs was not recorded.
  - Fix: none in Verbatim. `runner-load.txt` names the services each listed `svchost.exe` runs (agent protocol 37).

- `theme_panel` on GitHub's runner (run 45944007285, 2026-11-27, at 3810e1ec)
  - How: after Enter on "Remove button", "sound: app-not-responding" where the Remove Theme confirmation was expected, as in the entry above on the same dialog.
  - Cause: the same runner stall, the first with the service named: the busiest process was the `svchost.exe` running `StorSvc`, Windows' Storage Service, at 27.7% of the processors, with the processor 42.5% busy and the disks' queue 0.81 on average. So in the two stalls whose load was recorded, a Windows service host was the busiest process; why the Storage Service runs then is not known.
  - Fix: none in Verbatim. When the next stall's `runner-load.txt` names a service again, whether the runner image's services can be held off during the suite goes to Dickson, since it changes the test environment rather than Verbatim.

- `mockapp::remote_ops::a_windows_forms_menu_item_is_checked_from_the_remote_read` in `ci-arm64` (run 45884584255, the `wpf_password` run's)
  - How: UIA_E_TIMEOUT from `window_live` on the compiled Forms program it had just started, after 14.7 s; it took 2.7 to 7.8 s in the 11 runs before.
  - Cause: the Forms program not answering UI Automation's first connection, the same family on the ARM64 runner; the log carries only the panic.
  - Fix: as for the entries above.

## A typed character's echo read after the typing limit

- `conhost_review_grid` on GitHub's runner (run 45886935399, 2026-11-27, at 81ecf771; the code was the same as the passing run at 2b3959a5)
  - How: "\\" was heard where "backslash" was expected while typing `.\grid.ps1`.
  - Cause: the console host's change event for the typed `\` reached the listener about 250 ms late, and the output read was answered about 322 ms after the key, past the typing limit (`TYPED_LIMIT_MS`, 300 ms, in `crates/verbatim-text/src/terminal_reading.rs`). So the echo was taken as plain output and spoken with no symbol name. Every other key in the run was read within 6 ms.
  - Fix: none in Verbatim. The speech is the ruled behaviour (rule 20 of `books/dev/src/explanation/design/terminal-model.md`; Dickson kept the limit on 2026-11-27), so the failure is the runner's stall delaying the echo past the limit, a cause outside Verbatim, matched to the runner-stall entries above. The scenario is not loosened.

## A focus followed before the outpost stores its element

- `windows_terminal_marker_list` locally (run 3 of 3, 2026-11-27, at 085384bb)
  - How: as the terminal opened, before any key, "<title> terminal" was said for the second focus but its line, "blank", never was.
  - Cause: Windows Terminal's second focus is a new element (node 4:6). The focus read reported it, and Core's `Follow` of it was answered not there after no UI Automation call, as was the terminal's first read: the outpost's store had no element for the node yet (`crates/verbatim-outpost/src/outpost/followed.rs` 80, `element_of`), which it held only about 2 ms later. So a node a focus read mints can be followed before the outpost keeps its element. The code before the terminal series behaves the same.
  - Fix: the complete audit's root-cause stage, with its entry 13 (a focus minted with no element answering its reads gone), which share the cause; the scenario is not loosened.

## A key pressed as a terminal flood ends

- `windows_terminal_line_key_flood` on GitHub's runner (run 45787264327, 2026-11-26, at 5b79a4e6; it passed at 4327da97 and 71cf8f3b)
  - How: after Up Arrow as the flood ends, "flood line 93 …" was said where "ready> .\long.ps1" was expected first.
  - Cause: the result depends on whether Core's last output read lands before or after the key, a window of about 25 ms. Landing after, it is the key's own read and the recalled line is said at once; landing before, the unread flood rows are spoken as output and the recalled line queues behind them. Only the old terminal reads ran, with no find, history read or fallback.
  - Fix: the terminal work's step on how a key takes ownership of reads (`docs/simplification-review/builds.md`, the 2026-11-26 terminal entry), after the adapter. The scenario is not loosened.

## Console host scenarios

- `conhost_leave_flood`, on the development VM only; it passes on GitHub's runner and stays there.
  - How: fails at "brings the first window back": on 2026-10-31 the harness could not bring the first console window back to the foreground, because its window did not process the restore for 5 s after its flood. 10 of 10 locally.
  - Cause: the VM renders the desktop in software (WARP), so the Desktop Window Manager works through a flooded console's repaints for about 40 seconds after the flood, and a minimize waits for it in `win32kfull!DwmSyncNotifyMinimizing` while the console's window thread is idle. It happens with no Verbatim running ([troubleshooting](../books/dev/src/how-to/troubleshooting.md), "Minimizing a console after a flood takes 30 to 45 seconds").
  - Fix: none planned for the manager's backlog (Dickson decided to leave it unaddressed); the harness does not wait for the manager (Dickson, 2026-10-26).

## A terminal's window not opening after a flood

- `windows_terminal_flood` and `conhost_flood`, locally: `windows_terminal_flood` failed its setup in runs 2 and 3 of a batch on 2026-11-23, each run straight after `conhost_flood`'s. `windows_terminal_status_bar` failed its setup the same way locally on 2026-11-27, in the first run straight after a `cargo xtask ci`, with no flood before it; `dwm.exe` was the busiest process, at 13% of the processors (`runner-load-launched-process-<pid>.txt`), and runs 2 and 3 passed.
  - How: the setup fails with `no window titled "verbatim-e2e-flood-..." opened within 30s`, before any step; no speech is asserted. Report it apart from a speech failure.
  - Cause: the Desktop Window Manager's backlog after a flood. The new terminal's window thread waits on the manager's API port (`\Sessions\<n>\Windows\DwmApiPort`) before its first `ShowWindow` for more than 30 seconds, so its window is not shown in time ([troubleshooting](../books/dev/src/how-to/troubleshooting.md), "A scenario run soon after a flood fails its setup"). Any `conhost_` or `windows_terminal_` scenario run soon after a flood can meet it; the floods meet it most, since each runs after another's in a batch.
  - Fix: build 6, the tests and the harness, which owns the scenarios' setup. Until then, before a flood scenario's setup, wait until `\Process(dwm)\% Processor Time` reads under 130 percent, as troubleshooting says; the harness does not wait for the manager (Dickson, 2026-10-26).

## Terminal scenarios, single failures on GitHub's runner

- `conhost_long_lines` (run 41333390347, 2026-11-01)
  - How: after Enter, "r" is heard where "part1 part2 ... part10" is expected. The step sends Enter and then the typed "n" 1 ms later; Core spoke "r" 5 ms after the key, from a snapshot showing "ready> .\grow.ps1" and an empty caret row.
  - Cause: not yet named. Which rule produced "r" is not shown; the typed-echo rules and the key line's handling were candidates, and that code has since been replaced (dbd39d56, 89c36cc8). It has failed once in about 120 failed-run logs from 2026-10-30 to 2026-11-20.
  - Fix: build 9, the terminal rebuild, which rewrites the terminal reading rules and the typed-echo rules (terminals P5, P9 and P13) and adds a test of a key arriving 1 ms after an Enter that said nothing, with this run's snapshot.

## Windows Terminal's two windows, the elevated title at launch

- Scenario: `windows_terminal_two_windows`, local-only for this reason. It failed in 14 GitHub runs between 2026-11-04 and 2026-11-19, and in 1 of 5 dispatched runs on 2026-11-27 (run 45920547307).
- How: the first step hears "Administrator: verbatim-e2e-two-a-... window" where "verbatim-e2e-two-a-... window" is expected, then the terminal and "blank" as expected.
- Cause: a harness ordering gap, not wrong speech: Verbatim reads the window's current name. The harness waits for the shell's "settling" title and then its own, then restores the minimized window; the recording of run 44562491188 shows the tab titled "Administrator: verbatim-e2e-..." for about three frames right after the restore, then the plain title, and the listener logged four name changes in that span. The prefix is the elevated console host's; which process sends the prefixed title, and why on the restore, is not established.
- Fix: it runs on GitHub again after one dispatched run with the agent logging the window's title at each name change names the sender and its trigger, and a harness wait on that evidence is designed. Launching the shell with the desktop shell's token failed every Windows Terminal scenario on the runner (run 44229970650) and is not tried again.

## File Explorer's folder window not responding on GitHub's runner

- Scenario: `explorer_folder_window` (GitHub runs 42654703245, 44123231466 and 44344991044; locally on 2026-10-31). It was local-only for this reason until 5 of 5 dispatched runs passed (run 45920550584, 2026-11-27); it runs on GitHub again (Claude, 2026-11-27), because a recurrence there now records Verbatim's dump and the runner's load, the evidence this entry lacks.
- How: nothing is spoken after the desktop's "Recycle Bin 1 of 13"; the folder window, opened in a new `explorer.exe` process, stops answering and Verbatim plays the application-not-responding sound.
- Cause: not yet named. In run 44344991044, `not-responding-wait-chains.txt` shows both `dwm.exe`'s and the new `explorer.exe`'s window threads blocked, with no cycle; the wait chain does not show what Explorer waits on. A link to the Desktop Window Manager rests on two runs and one wait chain. The harness does not wait for the manager (Dickson, 2026-10-26).
- Fix: none yet. The next failure's `runner-load.txt` and wait chains are read for what Explorer's thread waits on; a wait of Verbatim's is fixed, and one of the runner's goes to Dickson.

## Scenarios local-only for the runner's image

These are `.local_only` in the registry, each with its reason; the `e2e` job leaves them out of its groups (`cargo xtask e2e-plan`). The suggestions pair is under "Settings' suggestions list replaced as it opens" above.

- The `notepad_` scenarios: the runner has classic Notepad, a Win32 edit control, not Windows 11 Notepad. A dispatched `notepad_editing` (run 45921800468, 2026-11-27) heard "Text Editor edit multi line" from window class `Notepad` where Windows 11 Notepad's "Text editor document" is expected. Editing is also covered in the harness's Windows Forms text box.
- `settings_combo_box_expanded`: the runner's Settings opens its Date and time page with the focus on "Location Settings link", where this machine's opens it on "Region button", which the scenario's opening speech asserts; confirmed in 5 of 5 dispatched runs (run 45920539316, 2026-11-27). It runs on GitHub again when the runner's Settings opens on "Region button"; choosing the expectation from what the application focused is ruled out (`books/dev/src/reference/testing.md`, "One target, one code path").
- `toast`: the runner's Windows Server 2025 shell shows no toast. In 5 of 5 dispatched runs (run 45920531476, 2026-11-27) PowerShell's call to show the toast succeeded, but no toast window opened: the listener logged no event for one, and the run's video shows none on screen. It runs on GitHub again once a runner image shows toasts.

## `alt_tab_switcher` on a parked local desktop

- `alt_tab_switcher` locally (10 of 10 runs failed at 6e6c1efa and at bce10932, 2026-11-09); it passes on GitHub's runner (run 44700246402) and stays there.
  - How: the agent's `SwitcherItems` found no item with the focus within 30 s.
  - Cause: not yet named. UI Automation's focus stayed on the switcher host's empty `InputSite` pane (class `XamlExplorerHostIslandWindow`) and never reached an element whose parent is `SwitchItemListControl` (`switcher_items`, `crates/verbatim-agent/src/focus.rs`). Verbatim is not involved, since the check is the agent's own read. The failure now lists the switcher window's raw-view elements.
  - Fix: one run parked and one with Remote Desktop connected, then, from the raw view, either the agent reads the items from the list found by its automation id, or the scenario is recorded as unable to run on a parked desktop.

## `table_cells`'s first focus

- `table_cells` locally (one failure in 10 runs, 2026-11-09, after the fix below; 2 of 16 before it)
  - How: the window is spoken, then nothing; Core received a UI Automation focus on the window and none on the first cell.
  - Cause: a focus event raised before a UI Automation client has reached the window's provider goes nowhere, shown by the listener's log of `msaa_flood_then_focus`, where the window's was the only focus event received. mockapp now raises its first focus once a client has asked for its root provider (`crates/mockapp/src/window.rs`, `client_contacted`), which took that scenario and `foreground_progress_bar` to 10 of 10. The remaining failure is not explained.
  - Fix: none yet for the remainder.

## A second Windows Terminal window's focus raised more often than the allowance

- `windows_terminal_leave_flood` on GitHub's runner (run 44858971330, 2026-11-21; it passed in run 44850817950 on the same scenario code)
  - How: "verbatim-e2e-leave-b-… terminal" was said 3 times more than once as the second window opened, where the scenario accepts at most 2.
  - Cause: Windows Terminal raises a second window's terminal focus under more than one runtime id, and both readers announce each repeat (`docs/parity/terminals.md`, "Windows and tabs"); this run raised one repeat more than the allowance, which counts the repeats seen in earlier runs rather than a rule of the application's.
  - Fix: build 9, the terminal rebuild, which reaches how a terminal's focus is announced; the allowance is not raised.

## The outpost's hidden-dialog check of a field

- `verbatim-ia2`'s unit test `batch::hidden_root_tests::a_field_in_a_hidden_dialog_matches`, locally, twice under `cargo test -p verbatim-outpost --lib` before it moved to `verbatim-ia2` (2026-11-21 and 2026-11-22), each time passing on reruns and in `cargo xtask ci`; 20 of 20 runs of the crate's unit tests passed on 2026-11-22.
  - How: the `assert_eq!` at `crates/verbatim-ia2/src/hidden_root_tests.rs` line 119 failed; which of the check's result or its two reads differed was not kept.
  - Cause: not yet named. The test reads only windows its own thread made: the dialog's visibility (`IsWindowVisible`) and its client object's role through oleacc's own proxy, in this process, with no clock, no wait and no state shared with the crate's other tests, so the code shows no ordering or timing it depends on.
  - Fix: build 6, the tests and the harness. The next failure's left and right values are kept, and they name which read differed before anything is changed.
