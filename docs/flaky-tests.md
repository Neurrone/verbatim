# Known failing and flaky tests

The one list of tests and scenarios known to fail intermittently, or consistently, for a known reason. Check it before debugging a failure, and before rerunning or running a baseline: a failure that matches an entry is already understood. Leave a listed scenario out of a related-scenario batch, as `CLAUDE.md` says.

Each entry gives the exact name, how it fails, how often and where, the cause when known, and when the fix is expected. Failure counts are from the dates given, on the development machine ("locally") or GitHub's runner. When a fix lands, delete the entry; history lives in git. When a new failure is explained, add an entry here and link to the work that owns the fix. Every entry is fixed, names the build of the simplification phase that reaches its cause, or has its test taken out of GitHub's runs with the reason and what would let it run there again (Dickson, 2026-11-20); a local-only scenario gives the same reason in its entry's `.local_only("reason")` in `crates/verbatim-e2e/src/registry.rs`.

## Settings' suggestions list replaced as it opens

- Scenario: `settings_search_suggestions_reentered`, local-only, which holds the steps of `settings_search_suggestions` since build 6 merged it there; `settings_search_suggestions` failed 2 of 13 local runs on 2026-11-09.
- How: "Suggestions available" plays to completion where the scenario expects the first suggestion to cut it off. On GitHub's runner "Suggestions available" played to completion the same way (run 39558690998); the log shows only that.
- Cause: Settings replaces the list's items as it opens, after raising the selection, so the selected element is in none of the elements the focus controls, and nothing is said, as in NVDA (`docs/parity/changes-and-selection.md`, pinned by mockapp's `a_stale_selection_after_a_replacement_reports_nothing_and_an_ordinary_one_the_new_item`). The failures' logs are not kept, so this is inferred from the code and the parity record. When Settings replaces its list is outside Verbatim's control, and accepting either outcome would break the exact-assertion rule.
- Fix: none in Verbatim. It stays off GitHub until dispatched runs show the runner's Settings keeping its first selection; locally they run in build 6's scheduled run of the local-only scenarios.

## UI Automation focus never delivered on GitHub's runner

- Scenario, local-only for this reason: `windows_terminal_long_lines` (run 44414430624, 2026-11-19). The same fault failed `settings_system_page` 8 times (2026-11-08 to 2026-11-20) and `windows_terminal_close_tab` once (run 44465917467); build 6 removed the first, its path covered by `settings_combo_box_expanded` and `settings_search_suggestions_reentered`, and merged the second's tab closing into `windows_terminal_tabs`, which runs on GitHub.
- How: in `settings_system_page`, "Settings" was said for the frame, then nothing for 75 s, and "Settings window" was never queued; in the terminal scenarios nothing is said after the new Windows Terminal window opens.
- Cause: UI Automation never delivered the focus to the listener's desktop-wide handler. In run 44571401780 and run 44465917467 the frame's focus arrived and was spoken; about 1.6 s later the page's `Windows.UI.Core.CoreWindow` (`SystemSettings.exe`) raised MSAA focus events, which the outpost rightly left to UI Automation, as NVDA does, and the listener's UI Automation layer, upstream of every filter of the listener's, logged no focus event and no window-opened event for 75 s. The terminals' listener logs hold one or two UI Automation events, none on the new terminal. This is the fault "A new process's focus events reach the desktop-wide handler late or not at all" ([UIA faults](../books/dev/src/reference/uia-bugs.md)); a focus UI Automation never delivers is lost, as in NVDA ([ADR 0001](../books/dev/src/reference/adr/0001-focus.md), its limits; Dickson, 2026-11-07).
- Fix: none in Verbatim. It runs on GitHub again once a runner image or a UI Automation change delivers these focuses, seen in dispatched runs.

## `windows_terminal_control_flood`'s command marks steps

- `windows_terminal_control_flood` on GitHub's runner (run 41697089785, 2026-11-03, when the steps were `windows_terminal_command_marks`, and run 42487419395, 2026-11-08)
  - How: after Verbatim+Alt+Up (the previous command mark), Verbatim played its "application not responding" sound where the mark's line "ready> .\long.ps1" was expected.
  - Cause: the mark lay above the rows Core keeps, so Core asked the outpost to search the terminal (`TerminalFind`, `crates/verbatim-uia-rops/src/terminal_find.rs`, bounded at 100 calls). In run 42487419395 the search took 6.4 s in Windows Terminal (`elapsed_us=6404203`), Windows judged the window hung, and the sound played as the rule says.
  - Fix: build 9, the terminal rebuild, whose finding that `terminal_find.rs` is written twice (terminals P14) and whose marks with a find written once (F12) reach the search; its first measurement is one `TerminalFind` in Windows Terminal on the runner against this 6.4 s. The test is not loosened.

## The caret watch's baseline anchor released while it waits

- `text_box_caret_keys` (run 41848692720, 2026-11-04, and runs 44514942878 and 44754313469 on GitHub's runner, 2026-11-19 and 2026-11-20)
  - How: in the first run, at Control+V on the start of "alpha beta gamma", nothing was said within 15 s where "mangoalpha beta gamma" was expected. In run 44514942878, at the Down Arrow after Control+V, "delta epsilon" was never queued.
  - Cause: in run 44514942878's flight recorder, the paste's start read (answer 150) came back after 332 ms, and Core's answers 151 and 152 for the Down Arrow were not there (`Outcome::NotThere`, from `TextReply::Gone` in `crates/verbatim-outpost/src/outpost/asked.rs`). The outpost's release of the nodes and anchors no live request can name (`crates/verbatim-outpost/src/outpost/store/mod.rs`) ran four times in 1 s, freeing anchors 275 and 276 of answer 150. It frees an anchor once its report position is acknowledged and Core's held set lacks it, and Core's held set (`held_anchors`, `crates/verbatim-core/src/state.rs`) is built only from committed state, so an anchor Core has received but not yet committed can be freed. Which anchor answers 151 and 152 named is not shown.
  - Fix: build 8, the request list closed, which redesigns how anchors cross; this failure is one of its cases. Until then the scenario is local-only, taken off GitHub after it failed again there in run 44754313469 (2026-11-20, answers 141, 145, 151 and 152 not there during the paste), and it runs on GitHub again once build 8 fixes the release.

## Verbatim's GUI thread stalls as the Remove Theme confirmation opens

- `theme_panel` on GitHub's runner (run 44883440325, 2026-11-21, at 5ce7fcb2; it passed in the 8 earlier runs whose artifacts were read, runs 44562491188 to 44858971330)
  - How: after Enter on "Remove button", "Remove Theme dialog Remove the theme Default copy? This cannot be undone." was never queued within 75 s, and nothing else was said.
  - Cause: Verbatim's own GUI thread (`verbatim.exe`, thread 672) stopped handling messages for about 5.2 s inside the confirmation's `ShowModal` (`crates/verbatim-gui/cpp/gui.cpp`, the Remove handler). Its last event was the hidden frame's state change at 08:20:25.507, and the next was the Theme dialog's at 08:20:30.675 (`listener.log`). In passing runs the dialog is said 100 to 420 ms after the key. Windows ghosted the hung windows at 08:20:30.69 and kept the ghost until 08:20:31.507. The task dialog's foreground and focus events came at 08:20:31.486 to .489, while its window still read as hung, so the listener's hung-window drop (`hang_tested`, `crates/verbatim-listener-logic/src/tell.rs`, from 5b4d1ad8, before build 4) took every one of them. The UI Automation focuses on the dialog and its No button reached Core, which read them as nothing: Core's verdict for the `#32770` dialog is MSAA. Replaying the run's flight recorder (`cargo xtask replay`) shows each input from line 464 to 476 producing only a timer, with no request sent. Core and both readers were idle through the stall: the Verbatim outpost's log shows no request from 08:20:25.11 until shutdown at 08:21:40. So no reader or Core caused the stall, and build 4 changed no GUI code. What the GUI thread waited on inside the task dialog's creation is not shown.
  - Fix: the silence is fixed by the report of a hung window (ADR 0003, "A frozen window"): the listener reports the dropped dialog's window, so Core watches it and reads the dialog once it answers, about half a second after the stall ends. If the stall recurs, the run now fails at the Enter step on "sound: app-not-responding", played as the Theme dialog joins the recovery set at about 30.68, and the harness records the not-responding windows' wait chains at once. The stall's own cause is still open. Verbatim's log now times the confirmation's `ShowModal`, which `theme_panel`'s teardown requires, so a dispatched run, `gh workflow run ci.yml -f scenario=theme_panel -f repeat=20`, measures how often the stall happens and how long it lasts. A stall in Verbatim's own code then goes to the build that owns that code; a stall in Windows' task dialog goes to Dickson.

## Console host scenarios

- `conhost_leave_flood`, on the development VM only; it passes on GitHub's runner and stays there.
  - How: fails at "brings the first window back": on 2026-10-31 the harness could not bring the first console window back to the foreground, because its window did not process the restore for 5 s after its flood. 10 of 10 locally.
  - Cause: the VM renders the desktop in software (WARP), so the Desktop Window Manager works through a flooded console's repaints for about 40 seconds after the flood, and a minimize waits for it in `win32kfull!DwmSyncNotifyMinimizing` while the console's window thread is idle. It happens with no Verbatim running ([troubleshooting](../books/dev/src/how-to/troubleshooting.md), "Minimizing a console after a flood takes 30 to 45 seconds").
  - Fix: none planned for the manager's backlog (Dickson decided to leave it unaddressed); the harness does not wait for the manager (Dickson, 2026-10-26).

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

These are `.local_only` in the registry, each with its reason; the `e2e` job leaves them out of its groups (`cargo xtask e2e-plan`). The suggestions pair is under "Settings' suggestions list replaced as it opens" above.

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

## A second Windows Terminal window's focus raised more often than the allowance

- `windows_terminal_leave_flood` on GitHub's runner (run 44858971330, 2026-11-21; it passed in run 44850817950 on the same scenario code)
  - How: "verbatim-e2e-leave-b-… terminal" was said 3 times more than once as the second window opened, where the scenario accepts at most 2.
  - Cause: Windows Terminal raises a second window's terminal focus under more than one runtime id, and both readers announce each repeat (`docs/parity/terminals.md`, "Windows and tabs"); this run raised one repeat more than the allowance, which counts the repeats seen in earlier runs rather than a rule of the application's.
  - Fix: build 9, the terminal rebuild, which reaches how a terminal's focus is announced; the allowance is not raised.

## The outpost's hidden-dialog check of a field

- `verbatim-outpost`'s unit test `outpost::hidden_root_tests::a_field_in_a_hidden_dialog_matches`, locally, twice under `cargo test -p verbatim-outpost --lib` (2026-11-21 and 2026-11-22), each time passing on reruns and in `cargo xtask ci`; 20 of 20 runs of the crate's unit tests passed on 2026-11-22.
  - How: the `assert_eq!` at `crates/verbatim-outpost/src/outpost/hidden_root_tests.rs` line 119 failed; which of the check's result or its two reads differed was not kept.
  - Cause: not yet named. The test reads only windows its own thread made: the dialog's visibility (`IsWindowVisible`) and its client object's role through oleacc's own proxy, in this process, with no clock, no wait and no state shared with the crate's other tests, so the code shows no ordering or timing it depends on.
  - Fix: build 6, the tests and the harness. The next failure's left and right values are kept, and they name which read differed before anything is changed.
