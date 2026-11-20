# Merged review: the end-to-end scenarios

Area: `crates/verbatim-e2e/src/scenarios/` (75 files including `mod.rs`, `terminal.rs` and `text_box.rs`), `src/scenario.rs`, `src/registry.rs`, `tests/e2e.rs`, with `docs/flaky-tests.md` and `books/dev/src/reference/test-coverage.md`. Checked at ba89dca7 (2026-11-20). Every quotation below was compared with the file today; every count was rerun with the command named. The area holds no product code: all of it is test harness and scenarios, so every "lines saved" figure is test lines.

## 1. Size

Recounted with `wc -l` (reviewer A's 12,412 for the scenarios was one line short; B's figures were right).

| Part | Verbatim | NVDA counterpart (pinned) | Ratio | What the extra lines buy |
|---|---|---|---|---|
| Scenario bodies, 105 live scenarios (scenarios folder 12,413 less `measure.rs` 1,201 and the five `demo_` files 104) | 11,108, about 106 a scenario | `nvda/tests/system/robot/*.py` and `*.robot`: 5,478 for 102 test cases (counted from the `*** Test Cases ***` sections), about 54 a case | 2.0x | A second copy of every editing scenario (text box and Notepad) and terminal scenario (Windows Terminal and console host), required by `testing.md` "One target, one scenario"; the 36 terminal twins alone are 4,079 lines (`cat scenarios/terminal*.rs`) |
| Harness in the area: `scenario.rs` 5,097, `registry.rs` 2,900, `tests/e2e.rs` 301 | 8,298 | `nvda/tests/system/libraries/`: 2,658 (NvdaLib 476, SystemTestSpy speechSpyGlobalPlugin 599, ChromeLib 298, NotepadLib 187, WindowsLib 176 and others) | 3.1x | The remote agent and staging, compare mode against NVDA, video and audio recording, failure artifacts and wait chains, the run lock, the local-only skip, the measure set; and a scenario table kept in three copies (finding 1) and written out field by field (finding 2) |
| The area | 20,711 | 8,152 (all of `nvda/tests/system`) | 2.5x | as above |
| Whole `verbatim-e2e` crate | 30,175 | 8,152 | 3.7x | as above |

Deletion target, all test and harness lines (product lines: 0 in every part):

| Part | Test and harness lines | Also |
|---|---|---|
| Remove now | about 1,900 (findings 1, 2, 7, 11, 12, 13, 14, 15) | about 240 lines of stale lists in documents (finding 8); about 60 s of every GitHub run (finding 6) |
| If Dickson rules for the removal | about 1,120: the 14 Notepad copies about 1,000 (finding 5), `settings_system_page` about 60 (finding 9), File Explorer's loading steps about 60 (finding 17); and, if he rules for it, the measure set's 1,201 lines and 16 registry entries (finding 19) | |
| Planned phase | about 90 in `terminal.rs` with M5 (finding 10); the three echo scenarios per terminal merged with M6.4 (finding 18) | |

## 2. Findings

| Id | Title | Class | Found by | File and line | What goes | Product lines saved | Test lines saved | Speech changes |
|---|---|---|---|---|---|---|---|---|
| 1 | The scenario list is kept three times, held in step by text-parsing checks | Remove now | both | `tests/e2e.rs` 38 to 191; `registry.rs` 2845 to 2884; `xtask/src/plan.rs` 176 to 260 | the name lists and macro, the agreement test, `test_names` | 0 | about 225 | no |
| 2 | A registry entry is 11 lines of mostly defaults; local-only has no reason field | Remove now | both | `registry.rs` 397 to 1834 | about 1,000 lines of fields | 0 | about 900 | no |
| 3 | The testing standard's two allowance counts disagree with the code | Defect (record) | both | `testing.md` 52 and 53; `terminal.rs` 717; `explorer_folder_window.rs` 115 and 123 | two stale counts | 0 | 0 | no |
| 4 | Expectations with no capture, and cited captures that no longer exist | Defect (test) | both | `toast.rs` 7 to 9; `twin_item_at_a_deleted_items_ids.rs` 17; `terminal_review_grid.rs` 50 to 52; 18 scenarios citing gone files | nothing; captures to make | 0 | 0 | needs a capture |
| 5 | 18 local-only scenarios run in no routine run; two features have only local-only end-to-end coverage | Question for Dickson | both | `registry.rs` local_only entries; `test-coverage.md` 76 and 129 | up to about 1,000 lines if the Notepad copies are retired | 0 | 0 to about 1,000 | no |
| 6 | `quit_inside_the_remove_theme_confirmation` replays the whole Theme page walk | Remove now (test replaced) | both | `quit_inside_the_remove_theme_confirmation.rs` 20; `theme_panel.rs` 113 to 271 | about 60 s of every GitHub run | 0 | about 0 (a short walk replaces the call) | no |
| 7 | mockapp launched by eight hand-written copies; nine live scenarios import the measure set's launcher | Remove now | both | `measure.rs` 393 to 430 and seven others | the copies | 0 | about 150 | no |
| 8 | Documents restating the scenario list and allowances have drifted | Remove now (documents) | both | `e2e.md` 370 to 374; `flaky-tests.md` 23 to 28, 44 to 56, 206 to 213; `registry.rs` 41 to 150; crate guide; `test-coverage.md` 181 | the copies | 0 | about 240 document lines | no |
| 9 | `settings_system_page` failed eight times on GitHub with no fix in Verbatim | Question for Dickson | both | `flaky-tests.md` 44 to 56 | 48 lines and its entry, or one local-only flag | 0 | 0 or about 60 | no |
| 10 | The Windows Terminal launch allowance stands in for a Verbatim rule not built | Planned phase (M5) | A, B noted | `terminal.rs` 704 to 770; `flaky-tests.md` 23 to 28 | the allowance and its pair checks | 0 | about 90 | no |
| 11 | An empty disabled-scenario mechanism, and a constructor nothing calls | Remove now | B (A for `DISABLED`) | `registry.rs` 234 to 248, 1866, 1880, 1888, 1929, 2886 to 2899; `scenario.rs` 370 to 376 | both | 0 | about 50 | no |
| 12 | `run` and `run_compare` are near copies | Remove now | B only | `registry.rs` 2013 and 2151 | one copy | 0 | about 80 | no |
| 13 | `press` written 14 times; six launch variants over one function | Remove now | B only | 14 files; `scenario.rs` 1299 to 1640 | the copies and wrappers | 0 | about 175 | no |
| 14 | `Scenario` holds "Verbatim or NVDA" and Verbatim's lifecycle as options and flags; a redundant in-process lock | Remove now | A only | `scenario.rs` 212 to 215, 291 to 357 | flags and the mutex | 0 | about 50 | no |
| 15 | Scenarios that repeat another's inputs | Remove now (tests merged) | A (B for one) | see paragraph | up to five scenarios | 0 | about 300 | no |
| 16 | A 30 ms paced step's expectation depends on the machine | Question for Dickson | A only | `selection_keys.rs` 153 to 162, 186 to 194 | nothing yet | 0 | 0 | yes |
| 17 | File Explorer's placeholder orders could move to a CoreDriver test | Question for Dickson | B only | `explorer_folder_window.rs` 100 to 127 | about 60 | 0 | about 60 | no |
| 18 | Three terminal scenarios per terminal each launch to run an echo | Planned phase (M6.4) | B only | `terminal_commands.rs` 78; `terminal_editing.rs` 100; `terminal_typing.rs` 93 | two launches per terminal | 0 | not estimated | no |
| 19 | The measure set: 1,201 lines and 16 registry entries | Question for Dickson | A only | `measure.rs`; `registry.rs` | the set or part of it | 0 | up to about 1,390 | no |
| 20 | The topical groups are read only by `cargo xtask vm test --group` | Question for Dickson | A only | `registry.rs` 250 to 300 (`pub enum Group` at 253); `xtask/src/vm/test.rs` 183 | the groups | 0 | about 120 (A's estimate) | no |

**1. The scenario list kept three times (both).** `registry.rs` 397 `pub const SCENARIOS: &[ScenarioDef] = &[` is the list. `tests/e2e.rs` 48, 160 and 173 copy every name (`const LIVE: &[&str] = scenario_tests! {`, `const DEMONSTRATIONS`, `const MEASURE_SET`, ending at 191; reviewer A gave 48 to 170, B 48 to 191, B was right), though the binary already builds its trials at run time (`let mut trials: Vec<Trial> = [LIVE, DEMONSTRATIONS, MEASURE_SET]` at 267). The macro's reason literal (`$reason:literal`) is matched and dropped. Checks that exist only because of the copies: `registry.rs` `names_in_the_test_binary` (`let source = include_str!("../tests/e2e.rs");`) and `the_test_binary_has_a_test_for_every_scenario_and_no_other`; `xtask/src/plan.rs` `registry_entries` (`.find("pub const SCENARIOS: &[ScenarioDef] = &[")`, `line.strip_prefix("group: Group::")`) and `test_names` (reading `scenario_tests! {` blocks); `xtask/src/group.rs` 163 `the_planners_reading_of_the_registry_is_the_registry` and `compare(&suite, &planned)` at 40. Change: `tests/e2e.rs` iterates `registry::SCENARIOS`; the planner reads only the registry. Reviewer A's alternative, the planner asking the binary's `--list`, does not work: `plan.rs` 4 to 5, "the `plan` job runs it on Linux, where xtask builds neither the registry crate nor the test binary". Saves about 150 lines of `e2e.rs`, 40 of the registry test and 35 of `test_names`. Checked with `grep -n` of each name.

**2. Eleven lines a registry entry (both).** 126 `ScenarioDef {` blocks in 397 to 1834 (1,438 lines). Inside the table `settings: None` 119 times and `document: None` 107 times (A said 122 and 110, B 120 and 107; counted with `grep -c` over those lines). `Comparable::No` repeats "it is a measurement workload of Verbatim's latency" 12 times and "it is a demonstration, recorded by" 5 times. `local_only: bool` is documented "with the reason beside its entry" (352 to 357), yet `toast` (541 to 550) has none, and the 14 Notepad entries have none beside them (the reason is in the module doc, 152 to 155). Change: a constructor with defaults and `local_only("reason")`, so a local-only entry without its reason cannot be written; the planner's text reading (finding 1) must follow the new shape. About 900 lines saved (126 entries at about 3 lines, plus the constructor). Speech unchanged.

**3. The standard's counts disagree with the code (both).** `testing.md` 52, File Explorer's placeholder: "and at most once in all (Dickson, 2026-10-21)"; the code, `explorer_folder_window.rs` 115 `expected.extend([optional_up_to(placeholder, 2), heard(part)]);` and 123 `said <= 2,`, citing "Dickson, 2026-11-19" at 109 to 110. `testing.md` 53, Windows Terminal: "up to once for a window opened on its own"; the code, `terminal.rs` 717 `pub(crate) const LAUNCH_REPEATS: usize = 2;`, citing "(Dickson accepted the return, 2026-11-19)". The standard says "Each use is approved by the owner and listed below", so the code accepts more than the list approves. Neither 5313bd45 nor 88ec29b9 touched `testing.md` (`git show --stat`); its last change is f5a4c4df (2026-11-17). The 2026-11-19 acceptances are recorded only in the two code comments and those commit messages. Fix: the standard states each use once and points at the constant; no speech change.

**4. Expectations not from a capture, and captures gone (both).** Quoted: `toast.rs` 7 to 9 "The expected speech is the design's (`docs/design/m42-parity.md`, "Toasts"), with this scenario's own title and body, to be confirmed against NVDA in compare mode."; `twin_item_at_a_deleted_items_ids.rs` 17 "The speech is to be captured from NVDA on this stand-in." (runs on every push, 7.3 s); `terminal_review_grid.rs` 50 to 52 "These expectations are the design's rule, not an NVDA capture: none has been made of NVDA's review cursor at the end of a terminal row." CLAUDE.md: "take spoken expectations only from captures". The capture files: 11 folders under `target/e2e-evidence/` are cited from scenarios; only `rtca-caret-keys-2026-11-17` and `rtca-selection-keys-2026-11-17` exist (`ls`), `target` is ignored (`.gitignore:1:/target`), and `git log --all -- '*<folder>*'` finds none of the other nine ever committed. The caret-keys captures were committed in 66a15d85 and removed in c341f768 ("Keep the caret-key captures out of the repository, as target is"). Table and counts in section 3.

**5. The local-only set (both).** Section 3, "Scenarios nothing runs routinely", has the list. A question: section 4.

**6. The Theme walk run twice (both).** `quit_inside_the_remove_theme_confirmation.rs` 19 to 21: `super::theme_panel::to_the_remove_confirmation(scenario);`, the function at `theme_panel.rs` 113 to 271, which `theme_panel` also runs (94). Durations: 78.1 s and 82.5 s, the suite's two longest. The quit needs only a removable theme's confirmation open; the page offers "New theme based on this... button Alt+n" (`theme_panel.rs` 247). A short walk of its own keeps the quit's subject (exit code 0 inside the task dialog) and saves about 60 s a run. The quit scenarios were added 2026-11-17 to 2026-11-19 (48971031, e4802f20, e99067c3): this shortens recent work, it does not undo it.

**7. mockapp's launch (both).** `"--fixture",` appears at `measure.rs` 409, `event_recorder.rs` 48, `menu_end_over_a_hung_window.rs` 71, `mod.rs` 229, `object_navigation_over_uia.rs` 54, `owned_dialog_across_processes.rs` 38, `return_from_a_held_application.rs` 73, `spelling_errors.rs` 74 (eight sites; A said seven besides `measure.rs`, B eight, both right). `fn line_events` at `measure.rs` 369, `menu_end_over_a_hung_window.rs` 44, `return_from_a_held_application.rs` 44. Nine live scenarios import from `super::measure` (`grep -l 'use super::measure::'`). One `scenarios/mockapp.rs` saves about 150 lines, and an edit to the measure set stops reaching live scenarios.

**8. Stale copies in documents (both).** `e2e.md` 370 to 371 lists `explorer_folder_window` as local-only (it is `local_only: false`) and omits `toast` and `settings_search_suggestions_reentered`; 374 says "The registry's `local_only` flag is the one place this is recorded." `flaky-tests.md` 206 to 213 repeats the list without `toast` or `settings_search_suggestions_reentered`. `flaky-tests.md` 23 to 28 still says "the scenario accepts one repeat" after 5313bd45 raised it to two. `flaky-tests.md` 44 to 56 is eight dated bullets for one cause. Entries whose fixes are built and that wait on evidence or acceptance: 7 to 14 ("the entry is deleted once the Windows Terminal scenarios pass on GitHub's runner"), 58 to 69 ("Recorded by Claude, 2026-11-04, pending Dickson's acceptance", both fixes "built with the slow applications build"), 176 to 189; the file's rule (line 5) is "When a fix lands, delete the entry". `test-coverage.md` 181 cites "build 1, step 9", defined in no other document (`grep -rn 'step 9' books docs`). The registry's module doc (41 to 150) and the crate guide `books/dev/src/reference/crates/verbatim-e2e.md` both lack all of `toast`, `msaa_flood_then_focus`, `stale_focus_behind_a_caret_key`, `read_only_field_typing`, `twin_item_at_a_deleted_items_ids`, `foreground_progress_bar`, `background_progress_bar`, `controlled_list_selections`, `review_dead_object`, `table_cells`, `event_recorder_records_mockapp` (0 matches each). Change: the documents point at the registry and the constants. About 240 document lines.

**9. `settings_system_page` (both).** `flaky-tests.md` 44 to 56: eight runs from 42492633218 (2026-11-08) to 44571401780 (2026-11-20); "Fix: none in Verbatim". Question in section 4.

**10. The launch allowance and a missing rule (A; B notes it).** `flaky-tests.md` 27: "Verbatim's rule dropping those repeats is not built, and the scenario accepts one repeat"; fix "M5, the Terminal built-in's rule for the second terminal identity". `testing.md`: "An optional utterance is only for speech whose presence or repetition the application's non-deterministic behaviour causes, never for a timing of Verbatim's own." The allowance is used at 19 call sites (`grep -c 'expect_windows_terminal_prompt_read('`, 18 live and one in `measure.rs`; A said 23). The NVDA claim at `terminal.rs` 712 to 714 ("each move a new focus by NVDA's duplicate test, so NVDA says the terminal again too") agrees with `nvda/source/UIAHandler/__init__.py` 902 to 912, which compares the sender only with `eventHandler.lastQueuedFocusObject`; no capture of NVDA saying it is cited. When M5's rule lands, the allowance, `require_launch_blank_after_repeat` and the pair checks (704 to 770, 785 to 828) shrink to exact expectations.

**11. Dead mechanism (B; A for `DISABLED`).** `registry.rs` 239 `pub const DISABLED: &[(&str, &str)] = &[];`, read by `disabled_reason` (243) and in `select` and `run_named` (1866, 1880, 1888, 1929), tested by `a_disabled_scenario_is_registered_never_selected_and_refused_by_name`, whose loop runs over nothing and cannot fail. It was used once, for a day: 0a6bde80 and ca551366, both 2026-10-12. No document names it (`grep -rn DISABLED books docs`: 0). `scenario.rs` 374 `pub fn launch() -> io::Result<Self> {`: `grep -rn 'Scenario::launch()\|launch()'` over `crates`, `xtask`, `uia-facts` finds no caller. About 50 lines.

**12. `run` and `run_compare` (B only, confirmed).** `registry.rs` 2013 `fn run_compare` and 2151 `fn run` both open with `hold_run_lock` and the same clearing of the artifacts folder; `diff` of their first 21 lines differs only in the `Scenario::compare_with` or `Scenario::launch_with` call and its panic text. One `run(def, mode)`: about 80 lines (B's estimate).

**13. Copied helpers (B only, confirmed).** `fn press(` is defined in 14 files, not 15 (`grep -rn '^fn press('`): `caret_keys`, `document_keys`, `editing`, `notepad_spelling_errors`, `password`, `quit_inside_a_file_dialog`, `review_dead_object`, `selection_keys`, `settings_dialog_keys`, `spelling_errors`, `switch_to_onecore`, `terminal_settings_page`, `theme_panel`, `word_selection`; nine have the identical body `scenario.send_keys(&[keys]).expect("sends the key"); scenario.speech().expect(heard);`. `testing.md` permits a shared one ("pressing a key and asserting the utterances the caller gives"). `Scenario::send_keys_expecting` has one caller. Launch variants: `launch_titled` 1299, `launch_titled_once` 1318, `launch_answering_uia` 1345, `launch_titled_with_stdin` 1373, `launch_owning_window` 1584, `launch_console` 1628, over `bring_forward_once` 1448. About 55 lines from `press` and about 120 (B's estimate) from one `launch` with options.

**14. `Scenario`'s shape (A only, confirmed).** `scenario.rs` 296 `verbatim_pid: Option<u32>`, 302 `control: Option<ControlClient>`, 338 `nvda: Option<NvdaRun>`, 349 `launch_exe: String`, and the flags `cleaned_up` (326), `quit` (328), `exited` (331): a Verbatim run with `nvda: Some` can be written. The in-process `live_instance_lock` (212 to 215) is taken only in `launch_with` and `compare_with` (444), whose only callers (`registry.rs` 2032, 2164) hold `hold_run_lock` first (2020, 2152). About 50 lines.

**15. Scenarios repeating another's inputs (A; B for the Settings pair).** `second_application_and_verbatim_menu` (56 lines, 12.5 s): opens the text box, Verbatim+V, Down Arrow ("Settings... s"), Escape, the box again; `text_box_editing` (`editing.rs` 93 to 95) opens the menu and returns the same way. Its extras are the Down Arrow and the outpost-kept assertion; moving both into `text_box_editing` removes about 70 lines. `settings_search_suggestions_reentered` repeats `settings_search_suggestions`' setup (`open_settings_page("ms-settings:system")`) and typing (`type_text("sound")`); both local-only; one scenario saves about 80 lines and a Settings launch. Three demonstrations are aliases (`demo_notepad_editing.rs`: `pub(crate) use super::editing::{document, notepad_body as body, notepad_setup as setup, teardown};`; likewise `demo_review_cursor.rs` and `demo_settings_dialog_keys.rs`); `xtask/src/demo.rs` 1 to 5 already records any test scenario into `videos/tests`, so with a destination option the aliases go (about 75 lines; `videos/demos` keeps its five files). A's merges of `quit_inside_a_modal_dialog` into `switch_to_onecore` and of `quit_inside_the_verbatim_menu` would undo scenarios added 2026-11-17 and 2026-11-19 for separate shutdown paths (48971031: "the case b71147a4 fixed, which no test covered"); kept only as a note, not in the target.

**16. A step whose expectation depends on the machine (A only).** `selection_keys.rs` 189 to 192: `.send_keys_paced(&["shift+rightarrow", "rightarrow"], 30)` with one fixed expectation; 182 to 184 "Whether Shift+Right's selection is said depends on whether its caret event is handled before Right". `testing.md` 15: "Nothing about what it runs against, or what it expects, depends on the machine." The flaky entry 70917fe7 removed said "whether the Windows Forms text box's caret event for Shift+Right reaches the screen reader before Right depends on the application's timing", with "Fix: a question for Dickson on how a test pins a race between two keys 30 ms apart"; 70917fe7 answered it as "a deliberate difference of timing (Dickson, 2026-11-17)". Question in section 4.

**17. File Explorer's placeholder (B only).** The allowance's code changed in 8 commits since 2026-10-20 (section 3); B's "eleven" counted three of 2026-10-17. Question in section 4.

**18. Echo scenarios (B only, confirmed).** `terminal_commands.rs` 78 `type_with_echo(scenario, "echo hello", Echo::Shown)`, `terminal_editing.rs` 100 `type_hearing(scenario, "echo helo")`, `terminal_typing.rs` 93 the same, each in its own launch, per terminal. M6.4 rewrites terminal reading (`docs/design/terminal-rebuild.md`); merge then.

**19. The measure set (A only).** `measure.rs` is 1,201 lines with 16 registry entries, run only by name; it changed with live scenarios because nine of them import its launcher (finding 7). Performance budgets are Dickson's; question in section 4.

**20. Topical groups (A only, confirmed).** `Group::Speech`, `Shell`, `Navigation`, `Text` are read by `registry::select` from `cargo xtask vm test --group` (`xtask/src/vm/test.rs` 183); GitHub's job groups by duration. Dropping them removes a command-line option, so a question.

## 3. Test findings (crate verbatim-e2e; xtask in brackets)

Counts: removable 2 unit tests (`the_test_binary_has_a_test_for_every_scenario_and_no_other`, `a_disabled_scenario_is_registered_never_selected_and_refused_by_name`) [and `test_names` in xtask], about 1,900 harness lines, and 3 demonstration aliases plus 2 merged scenarios (`second_application_and_verbatim_menu`, `settings_search_suggestions_reentered`). Replaceable: `quit_inside_the_remove_theme_confirmation`'s walk by a short one; the Windows Terminal launch allowance by exact expectations with M5. To add: captures for `toast`, `twin_item_at_a_deleted_items_ids` and the review grid's end-of-row steps; captures again for the 18 scenarios whose only cited files are gone; the outpost-kept assertion and the menu's Down Arrow moved into `text_box_editing`.

### Where each scenario's expected speech comes from

Classes: **exists** a cited capture file is on disk (under ignored `target/`); **gone** every cited capture file is absent from the tree and from git history; **described** a capture is named by date, build or `cargo xtask nvda compare` run, with no file; **parity** a parity, NVDA-prose or design document only; **own** the scenario says its speech is the design's, Verbatim's own, or still to be captured; **none** nothing named.

| Scenario | Application | Capture or recording named (quoted) | Class |
|---|---|---|---|
| menu_and_settings_dialog, rapid_tabbing_in_settings, object_navigation_in_settings, switch_to_onecore, quit_inside_a_modal_dialog, quit_inside_the_verbatim_menu, quit_inside_the_remove_theme_confirmation | Verbatim's own dialogs and menu | none | none |
| second_application_and_verbatim_menu | text box | none | none |
| crash_recovery | Verbatim's processes, mockapp | none | none |
| verbatim_lifecycle | Verbatim | `docs/parity/system.md` | parity |
| quit_inside_a_file_dialog | Verbatim's file dialog | "The speech is NVDA's, captured at the pin on 2026-11-19." | described |
| speak_time_and_date, lock_key_announcements | Verbatim, the agent's own reads | none | none |
| toast (local) | PowerShell toast | "The expected speech is the design's (`docs/design/m42-parity.md`, "Toasts"), ... to be confirmed against NVDA in compare mode." | own |
| msaa_flood_then_focus | mockapp MSAA | none | none |
| stale_focus_behind_a_caret_key | mockapp MSAA | "The speech is NVDA's, captured on this scenario's steps" (14) | described |
| read_only_field_typing | mockapp MSAA | "NVDA's speech, captured on the Location field itself (5 of 5 runs, the pinned build, 2026-11-19)" | described |
| twin_item_at_a_deleted_items_ids | mockapp list view | "The speech is to be captured from NVDA on this stand-in." | own |
| foreground_progress_bar | mockapp | `docs/parity/changes-and-selection.md` | parity |
| background_progress_bar, background_progress_bar_off | mockapp | `docs/design/progress-bars.md` | parity |
| controlled_list_selections | mockapp | "NVDA's captures of 2026-11-09" (6) | described |
| review_dead_object | mockapp | "NVDA's captures of 2026-11-09" (4 to 5) | described |
| table_cells | mockapp | `docs/parity/focus-announcement.md` | parity |
| alt_tab_switcher | Alt+Tab switcher | `docs/parity/windows-and-processes.md` | parity |
| return_from_a_held_application | text box, mockapp | none | none |
| menu_end_over_a_hung_window | mockapp | "captures" (7, 14, 18), no file | described |
| owned_dialog_across_processes | two mockapps | "`cargo xtask nvda compare owned_dialog_across_processes`" (94) | described |
| quick_link_menu | Windows+X menu | "captured with `cargo xtask nvda compare" (4) | described |
| explorer_folder_window | File Explorer | "NVDA's captures at the pin say exactly these in each of 10 runs" (20) | described |
| settings_dialog_keys, terminal_settings_page | Verbatim's own dialog | `docs/parity/system.md` | parity |
| settings_system_page | Settings | none | none |
| settings_combo_box_expanded (local) | Settings | "`target/e2e-evidence/fixes-round-eight-2026-10-21/nvda-expanded-combo-box-capture.txt`" | gone |
| settings_search_suggestions (local) | Settings | "as NVDA's capture does (commit 10ddf7b8)"; 10ddf7b8 commits no capture file | described |
| settings_search_suggestions_reentered (local) | Settings | "`target/e2e-evidence/fixes-round-six-2026-10-21/rapid-tab-nvda-1.txt`", "`target/e2e-evidence/m42-desktop-2-2026-10-22/part3a-nvda-2.log`" | gone |
| text_box_editing, notepad_editing (local) | text box, Notepad | "`target/e2e-evidence/m42-desktop-2-2026-10-22/part3b-textbox-1-type-over.txt`", "`m42-word-at-end-2026-10-22/text-box-editing-compare.txt`", "`part3b-notepad-1.txt`", "`m42-desktop-4-2026-10-22/notepad-cut-after-typing-over.txt`" | gone |
| text_box_selection_keys, notepad_selection_keys (local) | text box, Notepad | "`target/e2e-evidence/rtca-selection-keys-2026-11-17/` (`cmp-text_box_selection_keys.txt`, `cmp-notepad_selection_keys.txt`)", present; older `m42-selection-keys-2026-10-22`, `m42-selection-fixes-2026-10-22`, `m42-text-captures-2026-10-23` gone | exists |
| notepad_selection_then_control (local) | Notepad | "`cargo xtask nvda compare notepad_selection_then_control`" (2026-11-17), no file in the folder | described |
| text_box_caret_keys, notepad_caret_keys (local) | text box, Notepad | "`target/e2e-evidence/rtca-caret-keys-2026-11-17/` as `cmp-notepad_caret_keys.txt` and `cmp-text_box_caret_keys.txt`", present (committed 66a15d85, removed c341f768) | exists |
| text_box_ and notepad_ document_keys, page_keys, paragraph_keys, select_all_count (8; Notepad local) | text box, Notepad | "`target/e2e-evidence/m42-text-captures-2026-10-23/`" | gone |
| text_box_password, wpf_password | Windows Forms, WPF | "`m42-selection-keys-2026-10-22/`", "`m42-protected-fixes-2026-10-22/`", "`m42-text-captures-2026-10-23/20a-forms-password-type-a-run1.txt`" | gone |
| text_box_review_cursor, notepad_review_cursor (local) | text box, Notepad | "(captured 2026-10-22, NVDA alpha-58022,732d1ce2)" at 236 and 396, no file | described |
| text_box_typed_words, notepad_typed_words (local) | text box, Notepad | "`target/e2e-evidence/m42-text-captures-2026-10-23/`, `05b-*`" | gone |
| text_box_word_selection, notepad_word_selection (local), text_box_say_all, notepad_say_all (local) | text box, Notepad | none | none |
| text_box_review_words, notepad_review_words (local) | text box, Notepad | `docs/parity/review.md` | parity |
| spelling_errors, notepad_spelling_errors (local) | mockapp, Notepad | `docs/nvda/document-formatting.md` | parity |
| theme_panel | Verbatim's own dialog | "(captured 2026-11-19)" at 159, one step | described |
| system_information_tree | msinfo32 | `docs/nvda/speech.md` | parity |
| event_recorder_records_mockapp, object_navigation_over_uia | mockapp | none | none |
| windows_terminal_ and conhost_ commands, flood, scrollback_overflow_during_group, review_stays_on_line (8) | both terminals | `docs/parity/terminals.md` or `review.md` | parity |
| windows_terminal_ and conhost_ editing, progress (4) | both terminals | none | none |
| windows_terminal_ and conhost_ status_bar (2) | both terminals | "NVDA has no terminal status bar, so each press is Verbatim's own" | own |
| control_flood and line_key_flood, both terminals (4) | both terminals | "NVDA, captured live on 2026-10-09"; the marks steps: "NVDA has no command marks, so each press is Verbatim's own" | described |
| windows_terminal_two_windows, windows_terminal_leave_flood | Windows Terminal | "`target/e2e-evidence/m42-desktop-2-2026-10-22/`, `part2c-windows_terminal_two_windows-run1.txt` and `part2c-windows_terminal_leave_flood-run1.txt`" | gone |
| conhost_two_windows, conhost_leave_flood, windows_terminal_close_tab, windows_terminal_tabs | both terminals | "captured first on each (2026-10-09 ...)", "captured live on 2026-10-12", "captured at the pin (alpha-58022, 732d1ce2" | described |
| review_grid (2) | both terminals | "captured live on 2026-10-09"; end-of-row steps: "These expectations are the design's rule, not an NVDA capture" | described |
| typing, long_lines, full_screen, marker_list, pager_history, both terminals (10) | both terminals | "captured live on 2026-10-09" | described |

Counts over the 105 live scenarios: a cited capture file that exists, 4 (the two caret-key and two selection-key scenarios); a cited capture that is gone, 18; a capture described with no file, 34; a parity or design document only, 21; speech declared the design's, Verbatim's own or still to be captured, 4 (`toast`, `twin_item_at_a_deleted_items_ids`, both status-bar scenarios), plus own steps inside the four key-timing and two review-grid scenarios; none named, 24. So 22 name a capture file (4 present, 18 gone) and 83 name none.

### Allowances

| Where | Code | Scenarios | Permitting sentence of `testing.md` | Agreement |
|---|---|---|---|---|
| `terminal.rs` 747 to 748, `LAUNCH_REPEATS` 717 | `optional_up_to(&terminal, repeats), optional("blank"),` then `require_launch_blank_after_repeat` (764 to 770) | every Windows Terminal opening, 19 call sites | 53: "A Windows Terminal window's "\<title> terminal", repeated before the one heard in full, as the window opens in every Windows Terminal scenario, up to once for a window opened on its own" | disagrees: code allows two |
| `terminal.rs` 792 to 828, `SECOND_WINDOW_REPEATS` 725 | two pairs of `optional_up_to(&terminal, SECOND_WINDOW_REPEATS), optional("blank")`, at most two in all (815) | `windows_terminal_two_windows`, `windows_terminal_leave_flood` (`terminal_windows.rs` 318, 411), `measure.rs` 227 | 55: "A second window opened in the running Windows Terminal ... up to twice in all" | agrees |
| `explorer_folder_window.rs` 115 and 120 to 125 | `optional_up_to(placeholder, 2)` before each part, `said <= 2` | `explorer_folder_window`, three loads | 52: "and at most once in all (Dickson, 2026-10-21)" | disagrees: code allows twice in all and twice at one place |
| `settings_search_suggestions_reentered.rs` 107 | `optional("sound: suggestions-closed")` | that scenario | 54: "Settings' "sound: suggestions-closed" a second time after Escape ..." | agrees |
| `rapid_tabbing_in_settings.rs` 87 | `scenario.take_until_idle()` | that scenario | 36: "One exception, decided by the owner: `rapid_tabbing_in_settings`" | agrees |
| `terminal_key_timing.rs` 206 | `scenario.speech().take_until(&line(100), FLOOD_STEP)` | the two `_control_flood` | 37: "A second exception, decided by the owner (2026-10-15): the `*_control_flood` scenarios" | agrees |
| `verbatim_lifecycle.rs` 46 | `.send_keys_exiting(&["enter"], "sound: exit")` | that scenario | 38: "A third exception, decided by the owner (2026-11-03)" | agrees |
| `registry.rs` 2181 | `scenario.speech().collect_only()` for `Group::Measure` | 16 measure workloads | none (measurement, not tests) | outside the standard |
| `selection_keys.rs` 191 and 360 | `send_keys_paced(..., 30)` with one fixed expectation | `text_box_selection_keys`, `notepad_selection_keys`, `notepad_selection_then_control` | none; 15: "Nothing about what it runs against, or what it expects, depends on the machine." | disagrees (finding 16); pace ruled 2026-11-16, difference 2026-11-17 |

Commits since 2026-10-20 that changed the Windows Terminal launch allowance, with the reason each gives (`git show` of `terminal.rs`):
- 2ddb697d (2026-10-26): added the optional "blank": "Windows Terminal raises a new terminal's focus twice, about 40 ms apart, on two runtime ids, and Verbatim announces both ... (Dickson, 2026-10-26). The rule for the duplicate focus comes with Windows Terminal's app module in M5."
- cd797959 (2026-10-26): reverted it: "about thirty Windows Terminal scenarios failed in CI run 40334092780".
- 544c58f1 (2026-10-26): rewrote the matcher and restored it: "The speech matcher took an optional utterance whenever its text came next".
- 13f1da87 (2026-10-29): extended it to `measure_focus_windows_terminal`: "it failed 2 of 10 runs on them".
- 5313bd45 (2026-11-19): `LAUNCH_REPEATS` 1 to 2: "Windows Terminal may move its focus to another element and back as it opens ... accepted by Dickson on 2026-11-19."
- The second-window variant changed in 1e224c8b (2026-10-22, "as NVDA announces each repeat too") and 7e8fdefe (2026-11-09, "A second Windows Terminal window's repeated terminal can bring its "blank" before the terminal heard in full").

Commits since 2026-10-20 that changed File Explorer's placeholder allowance in code:
- e580e975 (2026-10-21): after Backspace too: "GitHub's runner failed explorer_folder_window there (runs 39558690998 and 39583748471)".
- 26ff6c0d (2026-10-21): before or after the list, "at most once in all ... (Dickson, 2026-10-21)".
- eff11a35 (2026-10-21): on the opening too: "GitHub run 39603654902 heard the placeholder first on the opening".
- 115d825f (2026-11-08): removed it, on the premise that the listener's live read made it impossible.
- 7e4cd800 (2026-11-08): restored it: "GitHub's runner (run 42426734150) showed File Explorer focusing its "Working on it..." static text ... 115d825f had made the placeholder never expected on that premise".
- 7e8fdefe (2026-11-09): before every part: "File Explorer's placeholder can be reported with the window above it but no list".
- 5313bd45 (2026-11-19): twice in all: "File Explorer may raise two focus events on its "Working on it..." placeholder a moment apart".
- 88ec29b9 (2026-11-19): twice at one place: "two in a row before "Items View list" (run 44523277003) failed".
- 43a5ea5b and 51c06a13 (2026-10-21) changed only its comments and a message.
Every widening followed a GitHub failure; none changed `testing.md` after 7e8fdefe.

### Scenarios nothing runs routinely

`ci.yml` sets `VERBATIM_E2E_SKIP_LOCAL_ONLY: "1"` for the e2e job; no workflow runs the local-only set; none is in `xtask/e2e-durations.txt`. The 18 `local_only: true` entries (`grep` of `registry.rs`):

| Scenario | Reason recorded | Other coverage of the same feature |
|---|---|---|
| toast | none anywhere (added local-only in 81003396) | verbatim-core `a_toast_is_spoken_from_anywhere_queued` and others; mockapp msaa_events `an_alert_from_a_toast_window_is_reported` |
| settings_combo_box_expanded | `registry.rs` 776 to 777: "The runner's Date and time page opens with its focus on its location settings link, not the Region button." | verbatim-core `focus_speech_tests` `expanding_without_a_count_says_only_expanded`, `an_expand_of_the_focus_left_reported_after_the_new_focus_is_spoken`; mockapp `combo_box.rs` (selection and value, not the expansion) |
| settings_search_suggestions | 789 to 790: "The runner's Settings raises no selection for a first suggestion as the list opens." | `controlled_list_selections` (GitHub); mockapp `controller_for.rs` |
| settings_search_suggestions_reentered | 802 to 803, the same sentence | as above |
| the 14 `notepad_` scenarios (editing, selection_keys, selection_then_control, caret_keys, document_keys, page_keys, paragraph_keys, select_all_count, review_cursor, say_all, word_selection, typed_words, review_words, spelling_errors) | no reason beside the entries; `registry.rs` 152 to 153 "needs what GitHub's Windows Server runner lacks: Windows 11 Notepad"; `testing.md` 18 | each `text_box_` twin on GitHub covers the Win32 edit path, not the UIA document path; mockapp `tests/text.rs` (37 tests) drives the outpost's text module on mockapp's UIA text provider; `spelling_errors` (mockapp, GitHub); `notepad_selection_then_control`'s ordering by verbatim-core `a_selection_said_after_a_modifiers_cut_carries_the_selection_keys_number` |

Features in `test-coverage.md` whose only end-to-end coverage is a local-only scenario: "Report focus (Verbatim+Tab)" (line 76, `notepad_editing`; `verbatim+tab` is sent only in `editing.rs` 162, inside `notepad_body`), and "Combo box opened" (line 129, `settings_combo_box_expanded`). "Toasts" (line 13) lists `toast` as its end-to-end test with "none" missing, though `toast` never runs routinely and its speech is uncaptured. On 2026-11-17 alone, 8 commits changed `selection_keys.rs` or `caret_keys.rs` (B said nine since then).

### Expectation churn

Population: the 123 commits since 2026-10-20 that touch `crates/verbatim-e2e/src/scenarios` and add or remove a non-comment line holding a string literal with `heard(`, `cut_off(`, `optional`, `expect`, `press(scenario` or `review(scenario` (191 commits touch the folder). Sample: every 4.1th commit of the list, newest first, 30 commits, each classified from its diff and message.

| Commit | Change | Class |
|---|---|---|
| eb135dff | Down Arrow in Settings' suggestions | capture (10ddf7b8) |
| c9d2e705 | hung window's ghost steps dropped | harness or scope |
| e48e06ab | hung mockapp's opening: window only | no capture: matched to the stand-in's output |
| 0341332c | read-only field echo, new scenario | capture (2026-11-19) with behaviour change |
| d1ffdb65 | stale focus scenario added | capture |
| bb06f56b | Shift+Right then Control cut | capture with behaviour change |
| e35951e7 | owned dialog stand-in fixed, steps removed pending capture | harness or scope |
| 70917fe7 | Shift+Right then Right at 30 ms, text box | ruled difference from the capture (Dickson, 2026-11-17) |
| 084ba55e | 30 ms pacing, NVDA's words | capture (`rtca-selection-keys-2026-11-17/paced`) |
| 0ff2864e | event recorder scenario added | no capture: new scenario, mockapp fixture's speech |
| 595f0542 | controlled list selections added | capture (described, 2026-11-09) with behaviour change |
| be0b3117 | editing chords silent; password Control+Backspace "blank" | capture (`15a-...`, `15c-...`) with behaviour change; password step a declared deviation |
| 0fb3280d | Alt+Tab switcher wait | harness or scope |
| 2781573f | terminal scenarios merged | harness or scope |
| 50b1d527 | location as a percentage; follow-caret toggle wording | no capture: parity ledger cited |
| 59c086c9 | lifecycle, time, Alt+Tab, crash recovery added | no capture: new scenarios |
| 26a26a02 | flood limits lowered | harness or scope |
| 048814d1 | held application scenario added | no capture: new scenario |
| 0ec0782b | footer row left out, "done" | ruled limit (Dickson, 2026-10-26) |
| c291dbed | command marks | no capture: Verbatim-only feature |
| bf6d3261 | clear and fresh floods added | no capture: the design's rule (Dickson, 2026-10-25) |
| 472833ff | progress: prompt then "100%" | no capture: "The expectation is changed from the design" |
| 12fc0d1f | caret, document, page, paragraph keys added | capture (`m42-text-captures-2026-10-23`) |
| fd1b92a5 | no word at the text's end, "blank" | capture (`m42-word-at-end-2026-10-22`) with behaviour change |
| 1ad45f16 | password keys pinned | capture |
| 11d08480 | Control+Backspace over a selection | capture (`textbox-word-control-backspace.txt`) |
| b563c80c | spelling marks read once | harness or scope |
| 182c416d | second suggestions-closed sound | allowance (ruled 2026-10-26) |
| 05fe1594 | combo box expanded, new scenario | capture (`fixes-round-eight-2026-10-21`) with behaviour change |
| 26ff6c0d | Explorer placeholder before or after the list | allowance (ruled 2026-10-21) |

Tally: capture 12 (6 with a behaviour change); no capture named 8; harness or scope 6; allowance or ruled timing 4. Both reviewers' samples are superseded: A's 2 capture against this 12 came from a different filter (139 commits, every seventh); B's 6, 5, 4, 3, 2 sorted the same kinds differently. Commits that changed an existing expectation to Verbatim's output with no capture named: e48e06ab, 50b1d527, 472833ff. Commits that set new expectations from Verbatim's own output or the design, with no capture: 0ff2864e, 59c086c9, 048814d1, bf6d3261, c291dbed (the last two Verbatim-only features with no NVDA counterpart). Ruled differences from a named capture: 70917fe7, 0ec0782b.

### The scenario list's copies, and the durations

The copies: `registry.rs` `SCENARIOS` (397 to 1834, the list); `tests/e2e.rs` 48 to 191 (`LIVE`, `DEMONSTRATIONS`, `MEASURE_SET`); `xtask/src/plan.rs` reads both files as text (`registry_entries` 176, `test_names`); the registry's module doc (41 to 150) and the crate guide name scenarios by group. The checks: `registry.rs` test `the_test_binary_has_a_test_for_every_scenario_and_no_other`, xtask `group.rs` test `the_planners_reading_of_the_registry_is_the_registry`, and the runtime `compare(&suite, &planned)` in `cargo xtask e2e-group` (`group.rs` 40). All confirmed (finding 1).

`xtask/e2e-durations.txt` (refreshed 3b0ffe91, 2026-11-20; medians over 19 GitHub runs): 87 scenarios, 1,950.7 s in all, run in parallel groups of at most 9 minutes each. Over 60 s: `theme_panel` 82.5, `quit_inside_the_remove_theme_confirmation` 78.1, `conhost_flood` 73.1, `windows_terminal_flood` 68.8, `windows_terminal_pager_history` 68.2, `conhost_pager_history` 65.5. The 36 terminal twins take 1,022.6 s (52 percent). Duplicated walks: the Theme page walk (`theme_panel.rs` 113 to 271) in both of the two longest, 160.6 s together (finding 6); the Verbatim menu round trip of `second_application_and_verbatim_menu` inside `text_box_editing`; the Settings search typing in both suggestion scenarios (local); the echo command in three scenarios per terminal (finding 18); three demonstrations that alias test bodies.

## 4. Questions for Dickson

**Q1. The UIA document path has no routine end-to-end run.** `testing.md` 18: "UIA documents are tested in Windows 11 Notepad. Those scenarios are local-only (`VERBATIM_E2E_SKIP_LOCAL_ONLY`), since GitHub's Windows Server runner has classic Notepad instead." The fact since: no routine run executes the 14 Notepad scenarios, "Report focus (Verbatim+Tab)" is covered end to end only in `notepad_editing`, and 8 commits on 2026-11-17 changed the Notepad selection and caret scenarios' expectations. Options: a scheduled run of the local-only set on the VM; or a mockapp UIA-text stand-in checked against a recording of Notepad, run on GitHub, with the Notepad copies retired (about 1,000 lines). Speech unchanged either way.

**Q2. `settings_system_page` fails on GitHub with no fix in Verbatim.** Eight runs, 2026-11-08 to 2026-11-20 (`flaky-tests.md` 44 to 56), each triaged under the push rule. `testing.md`: "A test known to fail is listed with its cause and fix ...; it is not skipped or loosened to pass", and `e2e.md` 381: "Mark a new scenario local-only only when it needs what the runner's Windows Server lacks". Options: keep it on GitHub; make it local-only (a change to that rule); or retire it, since list items with positions are asserted on GitHub by `explorer_folder_window` and `quick_link_menu`. Speech unchanged.

**Q3. A paced step whose outcome is the machine's.** Sequence: in the text box, Down Arrow, Home ("a"), then Shift+Right Arrow and Right Arrow 30 ms apart. NVDA's capture (`rtca-selection-keys-2026-11-17`, text box, step 31, quoted at `selection_keys.rs` 153 to 154): "l". The scenario expects `cut_off("a selected"), heard("l"), heard("a unselected")`, ruled a deliberate difference on 2026-11-17. The removed flaky entry said the order "depends on the application's timing", against `testing.md` 15. Options: keep; or drive the order deterministically below the end-to-end level and keep the scenario to unpaced keys. Speech is not changed by either; what the test can prove is.

**Q4. File Explorer's placeholder: real application or recorded order.** Eight code changes since 2026-10-20, each after a GitHub failure. Option: keep the real Explorer steps; or keep its arrow steps and test the placeholder orders in a CoreDriver test fed from a `verbatim-event-record` recording (about 60 lines go). Speech unchanged.

**Q5. The measure set.** 1,201 lines and 16 registry entries, cited by `books/dev/src/reference/performance.md`; performance budgets are Dickson's. Keep, trim, or move out of the e2e crate. Speech unchanged.

**Q6. `cargo xtask vm test --group speech|shell|navigation|text`.** Read only there; GitHub groups by duration. Keep the option or drop the topical groups (about 120 lines). Speech unchanged.

## 5. Cost of one behaviour

From `git show --stat`: a 26-line scenario touched 6 files (48971031: the scenario, `scenarios/mod.rs`, `registry.rs` +33/-11, `tests/e2e.rs`, the crate guide, `test-coverage.md`); one with a mockapp stand-in touched 12 files and 303 lines (d1ffdb65). After GitHub's run, `xtask/e2e-durations.txt` is regenerated and committed, and where the speech was not captured, a later capture commit follows. Shared mechanisms that cut it, merged from both reviewers: one table read by the test binary and the planner (finding 1) and built with defaults and a required local-only reason (finding 2), which removes the `tests/e2e.rs` line, the `use` line and nine of the eleven registry lines; the documents point at the registry instead of listing scenarios (finding 8); one mockapp module (finding 7), one `launch` with options and one `press` (finding 13), which remove the copied launch and helper from each new scenario. What remains per scenario: its module, one table line, one coverage row, and its capture.

## 6. Dropped

- B, `controlled_list_selections` duplicates mockapp's `controller_for.rs`: dropped; the mockapp tests feed listener elements the test builds (`controller_for.rs` 239, `common::listener_element`), so the scenario is the only test of the real listener's controlled-selection tell across the process boundary.
- A, the planner should read the test binary's `--list`: contradicted by `plan.rs` 4 to 5 (the plan job runs on Linux and builds no test binary); B's alternative kept.
- B, "Windows Terminal busy opening its tab switcher" has a built fix: contradicted; `flaky-tests.md` 78 leaves the question to M6.4.
- B, "41 public methods have exactly one call site": not rechecked; left out.
- A, `quit_inside_the_verbatim_menu` "is covered by any scenario ending with the menu open": no such scenario named or found; left out, with the quit merges noted under finding 15 as undoing recent work.
- A, "the registry's module doc lists 38 scenario modules, the crate guide names 39": not rechecked; replaced by the verified list of 11 scenarios missing from both.
- Corrected rather than dropped: A's 23 call sites of the launch allowance (19), A's `e2e.rs` range 48 to 170 (48 to 191), A's and B's `settings: None` counts (119), B's 15 copies of `press` (14), B's eleven Explorer allowance rewrites in a month (8 in code since 2026-10-20), B's nine commits since 2026-11-17 (8, all on that day), both reviewers' churn tallies (superseded by the sample of thirty).
