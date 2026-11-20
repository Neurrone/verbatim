# Known failing and flaky tests

The one list of tests and scenarios known to fail intermittently, or consistently, for a known reason. Check it before debugging a failure, and before rerunning or running a baseline: a failure that matches an entry is already understood. Leave a listed scenario out of a related-scenario batch, as `CLAUDE.md` says.

Each entry gives the exact name, how it fails, how often and where, the cause when known, and when the fix is expected. Failure counts are from the dates given, on the development machine ("locally") or GitHub's runner. When a fix lands, delete the entry; history lives in git. When a new failure is explained, add an entry here and link to the work that owns the fix. Every entry is fixed, names the build of the simplification phase that reaches its cause, or has its test taken out of GitHub's runs with the reason and what would let it run there again (Dickson, 2026-11-20); a local-only scenario gives the same reason beside its `local_only` flag in `crates/verbatim-e2e/src/registry.rs`.

## Settings' suggestions list replaced as it opens

- Scenarios: `settings_search_suggestions` and `settings_search_suggestions_reentered`, local-only. `settings_search_suggestions` failed 2 of 13 local runs on 2026-11-09.
- How: "Suggestions available" plays to completion where the scenario expects the first suggestion to cut it off. On GitHub's runner "Suggestions available" played to completion the same way (run 39558690998); the log shows only that.
- Cause: Settings replaces the list's items as it opens, after raising the selection, so the selected element is in none of the elements the focus controls, and nothing is said, as in NVDA (`docs/parity/changes-and-selection.md`, pinned by mockapp's `a_stale_selection_after_a_replacement_reports_nothing_and_an_ordinary_one_the_new_item`). The failures' logs are not kept, so this is inferred from the code and the parity record. When Settings replaces its list is outside Verbatim's control, and accepting either outcome would break the exact-assertion rule.
- Fix: none in Verbatim. Both stay off GitHub until dispatched runs show the runner's Settings keeping its first selection; locally they run in build 6's scheduled run of the local-only scenarios.

## UI Automation focus never delivered on GitHub's runner

- Scenarios, local-only for this reason: `settings_system_page` (8 failures, runs 42492633218, 42514816068, 42861051487, 43556583340, 44391277251, 44465917467, 44495098916 and 44571401780, 2026-11-08 to 2026-11-20), `windows_terminal_long_lines` (run 44414430624, 2026-11-19) and `windows_terminal_close_tab` (run 44465917467, 2026-11-19).
- How: in `settings_system_page`, "Settings" is said for the frame, then nothing for 75 s, and "Settings window" is never queued; in the two terminal scenarios nothing is said after the new Windows Terminal window opens.
- Cause: UI Automation never delivered the focus to the listener's desktop-wide handler. In run 44571401780 and run 44465917467 the frame's focus arrived and was spoken; about 1.6 s later the page's `Windows.UI.Core.CoreWindow` (`SystemSettings.exe`) raised MSAA focus events, which the outpost rightly left to UI Automation, as NVDA does, and the listener's UI Automation layer, upstream of every filter of the listener's, logged no focus event and no window-opened event for 75 s. The terminals' listener logs hold one or two UI Automation events, none on the new terminal. This is the fault "A new process's focus events reach the desktop-wide handler late or not at all" ([UIA faults](../books/dev/src/reference/uia-bugs.md)); a focus UI Automation never delivers is lost, as in NVDA ([ADR 0001](../books/dev/src/reference/adr/0001-focus.md), its limits; Dickson, 2026-11-07).
- Fix: none in Verbatim. They run on GitHub again once a runner image or a UI Automation change delivers these focuses, seen in dispatched runs.

## `windows_terminal_control_flood`'s command marks steps

- `windows_terminal_control_flood` on GitHub's runner (run 41697089785, 2026-11-03, when the steps were `windows_terminal_command_marks`, and run 42487419395, 2026-11-08)
  - How: after Verbatim+Alt+Up (the previous command mark), Verbatim played its "application not responding" sound where the mark's line "ready> .\long.ps1" was expected.
  - Cause: the mark lay above the rows Core keeps, so Core asked the outpost to search the terminal (`TerminalFind`, `crates/verbatim-uia-rops/src/terminal_find.rs`, bounded at 100 calls). In run 42487419395 the search took 6.4 s in Windows Terminal (`elapsed_us=6404203`), Windows judged the window hung, and the sound played as the rule says.
  - Fix: build 9, the terminal rebuild, whose finding that `terminal_find.rs` is written twice (terminals P14) and whose marks with a find written once (F12) reach the search; its first measurement is one `TerminalFind` in Windows Terminal on the runner against this 6.4 s. The test is not loosened.

## The caret watch's baseline anchor released while it waits

- `text_box_caret_keys` (run 41848692720, 2026-11-04, and run 44514942878 on GitHub's runner, 2026-11-19)
  - How: in the first run, at Control+V on the start of "alpha beta gamma", nothing was said within 15 s where "mangoalpha beta gamma" was expected. In run 44514942878, at the Down Arrow after Control+V, "delta epsilon" was never queued.
  - Cause: in run 44514942878's flight recorder, the paste's start read (answer 150) came back after 332 ms, and Core's answers 151 and 152 for the Down Arrow were not there (`Outcome::NotThere`, from `TextReply::Gone` in `crates/verbatim-outpost/src/outpost/asked.rs`). The outpost's release of the nodes and anchors no live request can name (`crates/verbatim-outpost/src/outpost/store/mod.rs`) ran four times in 1 s, freeing anchors 275 and 276 of answer 150. It frees an anchor once its report position is acknowledged and Core's held set lacks it, and Core's held set (`held_anchors`, `crates/verbatim-core/src/state.rs`) is built only from committed state, so an anchor Core has received but not yet committed can be freed. Which anchor answers 151 and 152 named is not shown.
  - Fix: build 8, the request list closed, whose study asks how anchors cross and whose first conversions include the caret key; this failure is one of that study's cases. If the study cannot reach it, the scenario is made local-only with this reason.

## Console host scenarios

- `conhost_leave_flood`, on the development VM only; it passes on GitHub's runner and stays there.
  - How: fails at "brings the first window back": on 2026-10-31 the harness could not bring the first console window back to the foreground, because its window did not process the restore for 5 s after its flood. 10 of 10 locally.
  - Cause: the VM renders the desktop in software (WARP), so the Desktop Window Manager works through a flooded console's repaints for about 40 seconds after the flood, and a minimize waits for it in `win32kfull!DwmSyncNotifyMinimizing` while the console's window thread is idle. It happens with no Verbatim running ([troubleshooting](../books/dev/src/how-to/troubleshooting.md), "Minimizing a console after a flood takes 30 to 45 seconds").
  - Fix: none planned for the manager's backlog (Dickson decided to leave it unaddressed); the harness does not wait for the manager (Dickson, 2026-10-26).
- `outpost::read::tests::only_the_console_host_reads_the_focus_in_its_console_window` under `cargo xtask ci` on GitHub's runner (run 44414430624, 2026-11-19)
  - How: the test console panicked with "the console's window took its client's title" (`crates/verbatim-test-console/src/lib.rs`) after its 10 s wait for the console window's name change, before the test read anything.
  - Cause: not yet named. The panic now names every name the console host's windows took during the wait and each process in the console's job with its image and whether it is running, so the next failure says whether the shell started and what title the window had.
  - Fix: a measurement of 20 runs of the test alone, then the next failure's message read. The wait is not lengthened.

## Terminal scenarios, single failures on GitHub's runner

- `conhost_long_lines` (run 41333390347, 2026-11-01)
  - How: after Enter, "r" is heard where "part1 part2 ... part10" is expected. The step sends Enter and then the typed "n" 1 ms later; Core spoke "r" 5 ms after the key, from a snapshot showing "ready> .\grow.ps1" and an empty caret row.
  - Cause: not yet named. Which rule produced "r" is not shown; the typed-echo rules and the key line's handling were candidates, and that code has since been replaced (dbd39d56, 89c36cc8). It has failed once in about 120 failed-run logs from 2026-10-30 to 2026-11-20.
  - Fix: build 9, the terminal rebuild, which rewrites the terminal reading rules and the typed-echo rules (terminals P5, P9 and P13) and adds a test of a key arriving 1 ms after an Enter that said nothing, with this run's snapshot.

## Windows Terminal's two windows, the elevated title at launch

- Scenario: `windows_terminal_two_windows`, local-only for this reason. It failed in 14 GitHub runs between 2026-11-04 and 2026-11-19, most recently run 44562491188.
- How: the first step hears "Administrator: verbatim-e2e-two-a-... window" where "verbatim-e2e-two-a-... window" is expected, then the terminal and "blank" as expected.
- Cause: a harness ordering gap, not wrong speech: Verbatim reads the window's current name. The harness waits for the shell's "settling" title and then its own, then restores the minimized window; the recording of run 44562491188 shows the tab titled "Administrator: verbatim-e2e-..." for about three frames right after the restore, then the plain title, and the listener logged four name changes in that span. The prefix is the elevated console host's; which process sends the prefixed title, and why on the restore, is not established.
- Fix: it runs on GitHub again after one dispatched run with the agent logging the window's title at each name change names the sender and its trigger, and a harness wait on that evidence is designed. Launching the shell with the desktop shell's token failed every Windows Terminal scenario on the runner (run 44229970650) and is not tried again.

## File Explorer's folder window not responding on GitHub's runner

- Scenario: `explorer_folder_window`, local-only for this reason (GitHub runs 42654703245, 44123231466 and 44344991044; locally on 2026-10-31).
- How: nothing is spoken after the desktop's "Recycle Bin 1 of 13"; the folder window, opened in a new `explorer.exe` process, stops answering and Verbatim plays the application-not-responding sound.
- Cause: not yet named. In run 44344991044, `not-responding-wait-chains.txt` shows both `dwm.exe`'s and the new `explorer.exe`'s window threads blocked, with no cycle; the wait chain does not show what Explorer waits on. A link to the Desktop Window Manager rests on two runs and one wait chain. The harness does not wait for the manager (Dickson, 2026-10-26).
- Fix: it runs on GitHub again after a local reproduction (on 2026-11-09 it failed 6 of 6 runs straight after a batch of other scenarios) whose kernel wait or wait chain names what Explorer's thread waits on, followed by a fix in Verbatim if the wait is Verbatim's, or after Dickson rules to keep it there.

## Scenarios local-only for the runner's image

These are `local_only` in the registry, each with its reason beside the flag; the `e2e` job leaves them out of its groups (`cargo xtask e2e-plan`). The suggestions pair is under "Settings' suggestions list replaced as it opens" above.

- The `notepad_` scenarios: the runner is said to have classic Notepad, a Win32 edit control, not Windows 11 Notepad; the claim rests on commit messages, not on an artifact. A dispatched run reads the runner's Notepad version, and the reason is corrected or confirmed from it. Editing is also covered in the harness's Windows Forms text box.
- `settings_combo_box_expanded`: the runner's Settings opens its Date and time page with the focus on "Location Settings link", where this machine's opens it on "Region button", which the scenario's opening speech asserts. It runs on GitHub again when the runner's Settings opens on "Region button"; choosing the expectation from what the application focused is ruled out (`books/dev/src/reference/testing.md`, "One target, one code path").
- `toast`: no reason was recorded when it was made local-only (81003396), and it has never run on GitHub. A dispatched run, `gh workflow run ci.yml -f scenario=toast -f repeat=3`, gives the reason from its artifacts, or, if it passes, lets it run there.

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
