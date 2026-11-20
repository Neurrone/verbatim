# Mockapp and the tests that drive readers against it: merged record

Merged from reviewer A's and reviewer B's notes and checked against main at ba89dca7 (2026-11-20). Everything below was re-read in the code or rerun as a grep or git command during the merge. "A stand-in" means a mockapp module or fixture that imitates a real application's behaviour.

## 1. Size

NVDA has no scripted provider process, so no NVDA code does mockapp's work: its system tests drive real Notepad, Chrome and VS Code. The nearest counterpart is NVDA's test harness: `nvda/tests/system/libraries/*.py` 1,483 lines, `nvda/tests/system/libraries/SystemTestSpy/` 1,175, `nvda/tests/unit/__init__.py` 155 and `nvda/tests/unit/objectProvider.py` 40, about 2,850 lines. All of `nvda/tests` is 33,406 lines of Python. Counts are from `wc -l`. B gave the system libraries as 1,523 lines; the recount is 1,483. The difference is under a tenth.

| Part | Verbatim lines | NVDA counterpart | Ratio |
|---|---|---|---|
| mockapp/src (the stand-in), including 591 lines of its own unit tests (stdin 269, tree 156, fixture 135, late_dialog 31) | 11,367 | none | n/a |
| tests/common (mod, harness, outpost, core_driver 1,474, tree_view) | 4,663 | harness about 2,850 | 1.6x |
| The 19 test files of this area (arbitration, child_cleanup, combo_box, controller_for, dead_objects, events, failure_paths, focus_reports, idle_barrier, listener_replays, msaa_events, native_controls, object_equality, shutdown, slow_application, time_limits, watch_checks, window_messages, window_names) | 11,377 | no cross-process tests | n/a |
| terminal_reading.rs and terminal_snapshot.rs (A counted them; B left them to the terminal pair; M6.4 rewrites them) | 1,148 | n/a | n/a |
| Fixtures (80 JSON files) | 3,156 | none | n/a |
| Area without the terminal files | about 30,560 | about 2,850 of harness | about 10.7x the harness; 0.9x all of NVDA's tests |
| The whole mockapp crate, Rust only | 43,847 | all of nvda/tests, 33,406 | 1.3x |

The reviewers agree on the counts. The two totals differ only in the terminal files: A's 12,525 test lines are B's 11,377 plus 727 and 421.

What the extra lines buy:
- A real provider in another process, so the real client stacks (verbatim-uia, verbatim-ia2) and a real outpost run against cross-process COM on GitHub's runners. This is a decision of record. `docs/roadmap.md` line 539 says of the planned simulation: "It does not replace mockapp, which tests the real client stack, COM and the cross-process calls at real timing".
- Things they need not buy: a second copy of the listener and of the shell in the test support (M1, M14), dead stand-ins (M2), a test list written twice (M3), and per-backend command tables (M6, M7).

Deletion target, in lines:

| | Product (mockapp/src, verbatim-listener) | Tests and test support | Fixtures, documents and scenarios |
|---|---|---|---|
| Remove now | about 920 (M2 580, M6 150, M7 150, M10 40); the export of the listener's check adds about 30 to verbatim-listener | about 1,450 (M3 650, M1 175 plus up to 170, M8 230, M9 100, M11 75, M15 20, M16 25) | 97 lines of fixtures (M2); about 500 document lines (M13); 73 inline-fixture lines in e2e (M5) |
| If Dickson rules for the removal | 130 (late_dialog's three unused variants) or 359 (the whole module) | 42 (its self-test) | 4 lines of the crate guide |
| Planned phase | none found in mockapp/src. terminal_reading.rs and terminal_snapshot.rs (1,148 lines) and the terminal faults in uia/text.rs belong to M6.4 and are not judged here | 0 | 0 |

## 2. Findings

| id | title | class | found by | file and line | what goes | product lines saved | test lines saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| M1 | The test support copies the listener's check, its bar routing, its limiter and its MSAA tell, and the check copy has already drifted | Remove now | both | crates/mockapp/tests/common/core_driver.rs 1190-1349, 1381-1414, 484-557, 579-645, 647-674 | the copies, replaced by calls to verbatim-listener's own functions | -30 (exports added) | 175, up to 345 | no |
| M2 | Dead stand-ins, commands and fixtures | Remove now | both | src/menu.rs, src/popup.rs, write-on-read, hold-acc-parent, freeze; 4 fixtures | the modules, their command variants and arms, and the fixtures | about 580 | 6 (one parse test), 97 fixture lines | no |
| M3 | Each test is named twice in a hand-written registration list | Remove now | both | every `fn main()`, such as focus_reports.rs 3843-4178 | the lists, replaced by a `stringify!` macro | 0 | about 650 | no |
| M4 | late_dialog: three of its four behaviours have no user, the fourth's only test tests the stand-in, and its recording check is claimed but not made | Question for Dickson | both | src/late_dialog.rs; msaa_events.rs 820-870 | the unused variants, or the module and its test | 130 or 359 | 0 or 42 | no |
| M5 | Scenarios carry inline copies of fixtures, and the twin-items copy differs from the file its recording test checks | Defect | B only | crates/verbatim-e2e/src/scenarios/twin_item_at_a_deleted_items_ids.rs 28-45 against tests/fixtures/list_view_twins.json | 5 inline copies, replaced by `include_str!` | 0 | 73 e2e lines | needs a capture (twin scenario only) |
| M6 | One command enum for every backend; each backend rejects the other's commands and silently accepts unreachable ones | Remove now | both (the catch-all B only) | src/stdin.rs 92-503, uia.rs 165-440, msaa.rs 208-463 | the cross-listed arms and catch-alls | about 150 | 0 | no |
| M7 | The hit counters' method list is written three times, kept equal by a compile-time assertion | Remove now | A only | src/hits.rs 199, 366, 451, 540 | `ALL`, `name` and the assertion, generated by one macro table | about 150 | 0 | no |
| M8 | events.rs delivery tests subsumed by later tests, and a test of the mock alone | Remove now | both (A 7 tests, B 4) | tests/events.rs 121, 229, 264, 720, 744, 802, 838 | 7 tests | 0 | about 230 | no |
| M9 | Dead test helpers hidden by `#![allow(dead_code)]` | Remove now | both | tests/common/mod.rs 11; outpost.rs 478, 702, 742, 766, 792, 840, 1306, 1322, 1428 | 9 helpers and the wiring only they use | 0 | about 100 | no |
| M10 | UIA and MSAA each have their own node mutators, and their `select_node` already differ | Defect (stand-in) | B only | src/msaa.rs 468-526; src/uia.rs 519-590 | one set of mutators on `Tree` | about 40 | 0 | no |
| M11 | One incident guarded twice: hand-ordered focus_reports tests beside the replays of the same runs | Remove now (after M1) | both (they differ on which to keep) | focus_reports.rs 2686, 2713; listener_replays.rs 262, 297 | the two focus_reports tests | 0 | 75 | no |
| M12 | Most stand-ins were never checked against a recording, and the three checks that exist copy the recording by hand | Test finding (add) | both | focus_reports.rs 540, 722, 845; the stand-in table below | nothing; recordings and comparisons are added | 0 | adds about 60-90 per stand-in | no |
| M13 | The documents restate the commands and the tests, and have drifted | Remove now | both | books/dev/src/how-to/mockapp.md 60-345; books/dev/src/reference/crates/mockapp.md 548-789 | the restatements, replaced by pointers to the rustdoc and the module comments | 0 | 0 (about 500 document lines) | no |
| M14 | The test support keeps its own copy of the shell's effect dispatch | Remove now (touches verbatim-app) | B only | core_driver.rs 965-1083 | the copy, replaced by the shell's dispatch as a library function | unknown, net near 0 | about 120 | no |
| M15 | arbitration.rs repeats an outpost unit test's arbitrator check | Remove now | B only | tests/arbitration.rs 25-51, 201-231 | the arbitrator halves, keeping the `raw_probe` assertions | 0 | about 20 | no |
| M16 | A menu-opening test repeats the first half of another | Remove now | B only | focus_reports.rs 2211, 2241 | 2241, its call-count assertion folded into 2211 | 0 | about 25 | no |
| M17 | Weak assertions | Remove now (replace) | A only | focus_reports.rs 682, 693, 695, 698, 821, 3261 | `assert!(matches!` and `is_some()` replaced by whole-value `assert_eq!` | 0 | 0 | no |

### M1. The test support copies the listener (both reviewers)

`crates/mockapp/tests/common/core_driver.rs` 1190 `pub fn uia_tell(` holds the listener's check (`crates/verbatim-listener/src/listener/checking.rs` 647 `fn check(`), the classification from `CheckEvent::new` (checking.rs 106-143), and the tell assembly from `Checks::checked` (checking.rs 517-557). The two files were compared function by function:

- `cached_of`: core_driver.rs 1381-1414 and checking.rs 148-182 are the same code, except for one comment (`// No kind the listener hears caches another.`).
- The call loop has the same eight arms in both. They differ in these ways:
  - Sender. The listener resolves the event's kept agile reference: `let sender = event.element.resolve().ok();`. If that fails, every sender call is answered `Read::Failed`, as in `sender.as_ref().map_or(Read::Failed, ...)`. The copy calls the live element directly (`verbatim_uia::nearest_window_lookup(element.live().element())`), so the resolve-failure path never runs.
  - `Call::KeptFocus`. The listener resolves the kept element: `.and_then(|(_, element)| element.resolve().ok()).map_or_else(failed, |element| keyboard_focus(&element))`. The copy uses the `Cached` directly: `.map_or(Read::Failed, |(_, kept)| keyboard_focus(kept))`.
  - Parent request. The listener tolerates its absence (`(Some(sender), Some(walker), Some(request))` else `Read::Failed`). The copy panics: `.expect("the parent's cache request")`.
  - Clock. The listener uses `now_us()`. The copy uses the test clock, `clock.now()`.
  - Window reads. The listener uses `SystemReads`. The copy uses `TestReads`, whose foreground and active window are the test's.
- The window of the tell, which is where the copy has drifted:
  - Listener, checking.rs 748-749: `(Some(window), _, None) => { PendingWindow::Found(WindowPart::read(&SystemReads, window, None)) }`. The foreground's part is added later, by the writer as it places the tell (tells.rs 89: `PendingWindow::Found(part) => TellWindow::Found(Box::new(part.with_foreground(reads)))`).
  - Copy, core_driver.rs 1326-1328: `(Some(window), _, None) => TellWindow::Found(Box::new( WindowPart::read(&reads, window, None).with_foreground(&reads), ))`, completed at once.
- Classification. The listener writes `placeholder_candidate: fields.role.class_name.as_deref() == Some(NET_UI_MENU_ITEM_CLASS)` (checking.rs 138, the constant `"NetUITWMenuItem"` at line 57). The copy writes `let placeholder = fields.role.class_name.as_deref() == Some("NetUITWMenuItem")` (core_driver.rs 1201), so the class string is written twice.
- Tell assembly:
  - Ownership. The listener tells the event only `if own`, that is, while its thread's record is its own (`state.threads.checked(pid, generation)`). Otherwise it releases the element and sends no tell. The copy always keeps and tells.
  - Time. The listener's `at_us` is the event's arrival time (`at_us: event.at_us`). The copy's is the time after the check (`at_us: self.now()`).
  - Application name. The listener's comes from its registry (`self.applications...name(event.process)`). The copy writes `"mockapp".to_owned()`.
  - Parent's automation id. The listener uses `with_answer(event.cached, told.parent)`. The copy writes the same merge out inline (core_driver.rs 1306-1313).
- Other copies in the same file:
  - `bar_change` (484-557) routes a classic value change with its own `self.bars` and `self.bar_windows` vectors (`let read = if read == ClassicBarRead::Unknown && !self.bar_windows.contains(&event.hwnd)`). The listener (listener.rs 1647) decides with `knowledge.judge(&ValueChange { pid, object, owner: owner_thread, at_us })`, and before that drops a hung window's change (`if windows.readability_now(hwnd).is_hung()`) and ignored processes. The copy has neither.
  - `raise_at_once` (579-645) re-implements the limiter's one waiting event per kind, object and thread with `waiting.retain(...)`.
  - `msaa_tell` (647-674) builds the MSAA tell with the foreground read at once and no foreground hold. The listener holds MSAA tells behind a pending foreground (tells.rs 204-206: "the hold holds MSAA events only, Dickson, 2026-11-17").

Which tests go through the copies. The count follows every test function, and the local helpers it calls, to `CoreDriver`, and separately to `.uia_focus(`, `.uia_event(` or `.uia_tell(`, the path through the check copy. The script is at the scratchpad's mergetools/reach.py.

| Test file | Tests through CoreDriver (all copies) | Tests through the check copy (`uia_tell`) |
|---|---|---|
| focus_reports.rs (83 tests) | 27 | 24 |
| msaa_events.rs (24) | 22 | 0 (MSAA path: `msaa_tell`, `bar_change`, `raise_at_once`) |
| native_controls.rs (12) | 12 | 3 |
| object_equality.rs (8) | 8 | 0 |
| combo_box.rs (8) | 5 | 5 |
| controller_for.rs (6) | 5 | 5 |
| window_names.rs (1) | 1 | 0 |
| Area total | 80 | 37 |
| Same crate, outside the area: caret_watch.rs (27), call_counts.rs (65), text.rs (15) | 26, 14, 1 | 17, 7, 1 |
| Crate total | 121 | 62 |

`crates/verbatim-listener/src/listener/checking.rs` has no unit test (`grep '#\[test\]'` finds 0), so `check`, `CheckEvent::new` and `Checks::checked` run only in end-to-end scenarios. Here is a defect in the real check that every one of the 62 tests would pass over. In `Checks::checked` (checking.rs 544), replace `cached: with_answer(event.cached, told.parent)` with `cached: event.cached`. Every real UIA tell then loses the parent's automation id, which is what lets Core recognise a task switcher item. `a_focus_carries_its_parents_automation_id` (focus_reports) still passes, because core_driver.rs 1306-1313 does its own merge. A second example: change `.reading_parent(event.switcher_item)` (checking.rs 666) to `.reading_parent(false)`. The real listener never reads the switcher item's parent, and the copy, which computes `switcher_item` itself at core_driver.rs 1205-1208, still does. Reviewer A's example also holds: read the sender's keyboard focus in the `Call::KeptFocus` arm, and `a_repeated_focus_on_the_still_focused_element_is_a_duplicate` (focus_reports.rs 2113) still passes. Reviewer B's sequence also holds: a writer that stopped adding the foreground (tells.rs 89) fails no mockapp test, since the copy adds it itself.

The testing standard (`books/dev/src/reference/testing.md` line 81): "A test measures or exercises the production code, never a copy of its logic written in the test."

Proposal: export `check`, `CheckEvent::new`, `cached_of` and `Checks::checked`'s assembly from verbatim-listener, with the window reads and the clock as parameters (`SystemReads` and `now_us` in the listener, `TestReads` and the test clock in the tests). Then expose the bar routing (`knowledge.judge`) and the limiter's key the same way. The check copy is 160 + 34 lines less a call site of about 20, so about 175 lines go; `bar_change`, `raise_at_once` and `msaa_tell` are 169 more. M6.1 will change checking.rs's queue to coalesce tells (`docs/roadmap.md` 1049-1058). That work changes the queue, not `check`, so exporting `check` first means M6.1 is tested through the real code. No speech changes.

Reviewer disagreement: A counted 16 msaa_events tests through the check copy. The recount is 22 through CoreDriver and none through `uia_tell`. B's 190 lines saved is the check copy alone; A's 350 includes the bar, limiter and MSAA copies. Both are right about their own scope.

### M2. Dead stand-ins and commands (both reviewers)

The grep was `grep -rn -- "<command>" crates/ xtask/ uia-facts/ vm/ --include=*.rs --include=*.json --include=*.py --include=*.ps1`, excluding crates/mockapp/src. The last use comes from `git log -S"<string>" -- crates/mockapp/tests crates/verbatim-e2e xtask`, checked with `git show`.

| Command or file | Code it keeps | Senders today | Last commit that used it |
|---|---|---|---|
| `popup-menu`, `menu-select`, `menu-close` | src/menu.rs (248); the stdin variants; the window.rs arm | 0, 0, 0 in crates and xtask. uia-facts/focus-for-a-newly-opened-or-shown-window-is-not-held/src/main.rs 80 and 91 send `menu-close` to uia-facts' own provider (uia-facts/provider/src/main.rs 488), not to mockapp | d17e3dba (2026-11-16) removed the last test, `let opened = app.send_answered("popup-menu");` with `menu-select` and `menu-close`. B's "before 5d75e0e5" is wrong: 5d75e0e5 removed only a doc comment |
| `popup <id>` and `tests/fixtures/popup.json` (24 lines) | src/popup.rs (132), uia.rs 411-423 `Command::Popup(id) => {`, the popup host paths, tree.rs 124 `pub(crate) popup: Option<isize>` | 0 (`grep -rnE '"popup( \|")'` over mockapp/tests, verbatim-e2e and xtask; 0 for `popup.json`) | adb0a7e2 (2026-11-07), for both the command and the fixture |
| `write-on-read` | stdin.rs 172, 646-651, 751; uia.rs 277-282; msaa.rs 366-369; edit.rs 218-221 `pub(crate) fn write_on_read` and its thread-local; uia/text.rs 195 `fn write_pending`; tree.rs 75. The edit control's watching procedure stays, since `stale_focus_on_left` uses it | 0 | c5fec096 (2026-11-16), `app.send("write-on-read term x")` and `"write-on-read doc x"` |
| `hold-acc-parent` | msaa.rs 415-422 and 865; tree.rs 105 `acc_parent_held`; uia.rs 370; stdin.rs 307 and 626; parse test stdin.rs 1062-1065 | 0 | b94fa1d8 (2026-11-07), `app.send("hold-acc-parent group1")`. B's "before b94fa1d8" is wrong: b94fa1d8 removed it |
| `freeze` | stdin.rs 473-477 and 666, window.rs 669, msaa.rs 458, uia.rs 435 (19 lines added by f8e01286) | 0 in crates, xtask, experiments and docs | Never sent by a test. Added in f8e01286 (2026-11-06), "Add mockapp's freeze command, for hung-window captures". No lasting document cites it |
| `on-activate` (the command, not the fixture's `focus_on_activate` flag) | window.rs 55-66 and the armed commands | 0 in crates and xtask. It is used by the hand-driven capture recorded in experiments/focus-pipeline-rework-measurements/captures/uia-parent-read-cancelled.txt, which docs/nvda/main-loop-and-watchdog.md 231 cites ("mockapp's `on-activate` and `stall`") | Never sent by a test. Added in 128957b7 (2026-10-17) with only a parse test. B's "last test removed in 42541256" is wrong: 42541256 added `focus_on_activate_raises_the_focused_nodes_focus_event`, whose window title `"mockapp-focus-on-activate"` matched the search. Kept: a lasting document cites it |
| `open-dialog-shown-late ... owned-box`, `... hide-close`, `tray-menu-over-hidden-dialog` | late_dialog.rs 46-52, 60-72, 94-98, 146-230 | 0, 0, 0. The only sender of the command is msaa_events.rs 849, `"open-dialog-shown-late 50"`, with no variant | Never sent by any test: `git log -S` over tests, e2e and xtask finds no commit. Added in bc99bc40 (2026-11-19). See M4 |
| `reuse_filtered.json` (39 lines) | none | 0 | d17e3dba (2026-11-16) |
| `terminal_reading_button.json` (16), `console_terminal_button.json` (18) | none | 0 | c5fec096 (2026-11-16) |

The search over all 80 fixtures (`grep` of each base name over crates, xtask, uia-facts and vm, outside the fixtures folder) finds `popup.json` and these three, and no other unused fixture. The crate guide still documents `console_terminal_button.json` (books/dev/src/reference/crates/mockapp.md 126), `popup.json` (246) and the `menu` module (266). Proposal: delete the commands marked unused, except `on-activate`, together with their fixtures and documentation. This removes about 580 product lines (menu 248, popup 132 plus about 30 lines of arms, write-on-read about 70, hold-acc-parent about 25, freeze 19, the stdin variants and parsing about 50), 97 fixture lines, one parse test and about 60 document lines. No speech changes, and no remaining test changes.

### M3. Registration lists (both reviewers)

Each `harness = false` binary lists its tests in `main()` as four-line `("name", name),` entries, for example focus_reports.rs 3843-4178, 336 lines for 83 tests. Summed over the 18 files of the area that have a `main` (listener_replays uses `#[test]`), the lists come to 881 lines; B said about 860 and is right. A said about 1,000, which included the terminal files. A `tests![a, b]` macro in common/harness.rs, producing `(stringify!(a), a as fn())`, saves about 650 lines in the area and about 1,400 in the crate, and a test can no longer be registered under the wrong name. No speech changes.

### M4. late_dialog (both reviewers; a question, see section 4)

src/late_dialog.rs is 359 lines. Its `BeforeShow::OwnedBox`, `BeforeShow::HideClose` and `tray-menu-over-hidden-dialog` have no sender (M2). Its one user, msaa_events.rs 849, asserts the class `"#32770"` and that the show was heard. Its comment says: "The foreground and the field's focus it raises while hidden are not asserted here". The same comment, the crate guide (mockapp.md 256-258: "Checked against the recording of Import a Theme (2026-11-19): the foreground, then the focus, while hidden, then the show, as Import") and commit 8c3978c7 ("a test checks the stand-in's order (foreground, the dialog's and the Edit's focus while hidden, then the show) against Import's recording") all claim a check that no committed test makes. The only Import artefact in the repository is a flight-recorder dump (crates/verbatim-core/tests/fixtures/dialog_shown_import.jsonl, header `"format_version":9`), not a verbatim-event-record recording.

The stand-ins exist by a ruling: late_dialog.rs 4-6 says "the stand-ins for the hidden dialog's cases (Dickson, 2026-11-19; the recordings of Import, Save As, Run and Properties showed every dialog hidden at its foreground shown 14 to 221 ms later)". That is why this is a question rather than a removal.

The module also polls. `spawn` loops on `made.try_recv()` with a 1 ms `wait_pumping` (late_dialog.rs 111-124), and `pump_for` (282-301) calls `thread::sleep(Duration::from_millis(1))` on every empty peek. Line numbers corrected: A gave 263-281 and B gave 279-300.

test-coverage.md row 122 lists "mockapp's `late_dialog` template test, msaa_events `the_dialog_shown_late_answers_and_shows_its_hidden_dialog`" as coverage. The feature's real coverage is verbatim-core's replay_validity of the Import dump and the `quit_inside_a_file_dialog` scenario. Both reviewers proposed removal and both claims check; whether the stand-ins go is Dickson's decision.

### M5. Inline fixtures in scenarios (B only; confirmed)

twin_item_at_a_deleted_items_ids.rs 28-45:
```
const FIXTURE: &str = r#"{
  "id": "root",
  "role": "window",
  "name": "{title}",
  "children": [ ... "clipboard" list_view, "first" and "second" "Copy" ... ]
```
tests/fixtures/list_view_twins.json, the fixture `the_twin_deletion_stand_in_matches_the_desktop_lists_recording` (focus_reports.rs 540, spawned at 547) checks:
```
  "id": "root",
  "role": "pane",
  "name": "Mockapp Twin Items List View Fixture",
  "states": ["focused"],
```
A JSON comparison finds the children identical. The root differs in `role` ("window" against "pane") and `states` (absent against `["focused"]`). Both files date from 9aba90be and 7ee82b09 (2026-11-18), so the drift dates from creation.

Every scenario with an inline mockapp fixture, compared with every file in crates/mockapp/tests/fixtures after parsing both as JSON:

| Scenario | Inline fixture | File of the same content |
|---|---|---|
| read_only_field_typing.rs 28 | `FIXTURE` | read_only_field.json, identical except the root's name |
| stale_focus_behind_a_caret_key.rs 33 | `FIXTURE` | stale_tree.json, identical except the root's name |
| owned_dialog_across_processes.rs 61 | `OWNER_FIXTURE` | owner_window.json, identical except the root's name |
| owned_dialog_across_processes.rs 74 | `DIALOG_FIXTURE` | owned_dialog.json, identical except the root's name |
| twin_item_at_a_deleted_items_ids.rs 28 | `FIXTURE` | list_view_twins.json differs (root role and states) |
| background_progress_bar.rs 24 and 45 | `FIXTURE`, `UIA_FIXTURE` | none |
| controlled_list_selections.rs 17 | `FIXTURE` | none (nearest is controller.json, which differs) |
| event_recorder.rs 19 | `FIXTURE` | none |
| foreground_progress_bar.rs 18 | `FIXTURE` | none |
| measure.rs 332 | `RETURN_FIXTURE` | none |
| msaa_flood_then_focus.rs 24 | `FIXTURE` | none |
| review_dead_object.rs 18 | `FIXTURE` | none |
| table_cells.rs 15 | `FIXTURE` | none |
| menu_end_over_a_hung_window.rs 59, return_from_a_held_application.rs 61, object_navigation_over_uia.rs 33, spelling_errors.rs 55, mod.rs 211 | `format!` templates | none, even with every name and title ignored |

The defect: `twin_item_at_a_deleted_items_ids` runs a stand-in other than the one checked against the recording. The fix is to have each of the five scenarios load its file with `include_str!` and fill in the title, which removes 73 e2e lines and the drift. Whether the twin scenario's expected speech ("{title} window", then "list", "Copy 1 of 2", "Copy 1 of 1") changes with a "pane" root that has the focused state needs a run or a capture; the other four do not change.

### M6. Commands by backend (both reviewers; the catch-all is B's alone)

`Command` (src/stdin.rs 92-503) holds every backend's commands, and `uia.rs:165 apply_command` and `msaa.rs:208 apply_command` each match all of them. msaa.rs has 14 arms that say "is not supported on the msaa backend" and uia.rs has 11 that say "on the uia backend", for example uia.rs 298 `return Err("client-name is not supported on the uia backend".into());`. Both end with an arm that accepts what the window thread already took: uia.rs 424-440 and msaa.rs 448-463 `| Command::Quit | Command::Unrecognized(_) => {}`. `Unrecognized` is rejected first by window.rs 719 (`Command::Unrecognized(line) => Err(format!("unrecognized command: {line}"))`), so those arms are reachable only by a logic bug and answer `Ok(())` silently. CLAUDE.md says: "Code reaches no fallback that could only be a logic bug". The lookup `.index_of(&id)` appears 11 times in uia.rs. Proposal: parse each line once into a window command or a command for one backend, so a misrouted command cannot be represented, and add one `node(id)` helper. This saves about 150 lines and drops the other backend's rejection arm from every new command: d1ffdb65, 0341332c and 9aba90be each show `crates/mockapp/src/uia.rs | 3 +` for that arm alone. No speech changes.

### M7. The hit counters' table (A only; confirmed, lines recounted)

src/hits.rs 199 `pub(crate) enum Method {` (77 variants), 366 `pub(crate) const ALL: [Method; 77] = [` (to 444), 451 `pub(crate) fn name(self) -> &'static str {` (to 531), and 540-547 `const _: () = { ... assert!(Method::ALL[index] as usize == index); ...}`. One `macro_rules!` table of variants and names would produce all three. It saves about 150 lines (ALL 79, name 81, assertion 8, less the macro), not A's 230, since the documented variants stay. A gave line 361 for `ALL`; it is 366. No speech changes.

### M8. events.rs tests the mock (both; A's broader list confirmed)

These tests check that mockapp's events reach `Registration` or `WinEventHook`:
- uia_set_name_raises_a_property_changed_event (121, 44 lines)
- uia_set_value_raises_a_property_changed_event (229, 30)
- uia_select_raises_a_selection_event (264, 49)
- msaa_set_name_raises_a_name_change_win_event (720, 24)
- msaa_set_value_raises_a_value_change_win_event (744, 27)
- msaa_select_raises_a_selection_win_event (802, 36)

Other tests already raise the same events through the same paths and assert more:
- UIA `set-name`: focus_reports.rs 1159 `app.send(&format!("set-name {id} Renamed"))`, heard by the outpost's own subscription.
- UIA `set-value`: native_controls.rs 645 `app.send("set-value slider 60")`.
- UIA `select`: combo_box.rs 158-160, through a `Registration` of `UIA_SelectionItem_ElementSelectedEventId`.
- MSAA: msaa_events.rs 380 (`set-value`) and 404 (`select item1`), call_counts.rs 611 (`select item2`) and window_names.rs 51 (`set-value slider1 77`), all through `core.raise`, which installs a `WinEventHook`.

`a_command_for_an_unknown_node_is_rejected` (838, 21 lines) tests the mock alone. Together the seven tests are about 230 lines. A keeps the `dialog_keys` and `focus_on_activate` tests (859, 907), because scenarios depend on those fixture features, and that is confirmed: owned_dialog_across_processes and event_recorder use `dialog_keys`, and eight scenarios use `focus_on_activate`. B listed four of the seven; A's other three also check out.

### M9. Dead helpers (both; B's `fixture_path` is wrong)

tests/common/mod.rs 11 `#![allow(dead_code)]`. `grep -rnw` over crates/mockapp/tests finds no caller for these helpers in outpost.rs:
- `with_listener` (478)
- `focus_moves_during_the_read` (702)
- `listener_judges_readability` (742)
- `idle_round` (766)
- `wait_for_foreground_reads` (792)
- `follow_the_system_clock` (840)
- `release_to_core_line` (1306)
- `release_all` (1322)
- `now_ms` (1428)

The only other matches are the definitions themselves and doc links. B's `fixture_path` (mod.rs 99) is called at mod.rs 424, so it stays. Together the nine helpers are 47 code lines and 30 doc lines, plus the `moves_to`, `system_time` and readability wiring they alone use: about 100 lines in all. A said 70 and B 130. The doc of `with_listener` names `Self::forwarding_ended` (outpost.rs 475), which does not exist, and the crate guide (mockapp.md 490) still describes `idle_round`.

### M10. Two sets of node mutators (B only; confirmed)

msaa.rs 468-526 and uia.rs 519-590 each define `focus_node`, `with_node`, `set_name`, `set_value` and `select_node`. UIA's `select_node` clears the state from the siblings too: `for sibling in guard.nodes[parent].children.clone() { guard.nodes[sibling].states.remove(verbatim_model::State::Selected); }`. MSAA's clears it only from `guard.selected.replace(index)`. `Tree.selected` starts as `None` (tree.rs 224) whatever the fixture says. Sequence: counts.json on the MSAA backend has item1 `"states": ["focusable", "selectable", "selected"]`; `select item2` (call_counts.rs 611) leaves item1 selected as well as item2, which a single-selection list never shows. One set on `Tree` saves about 40 lines. It is a stand-in defect; Verbatim's speech is unaffected.

### M11. One incident guarded twice (both; they differ on which copy to keep)

These pairs guard the same runs:
- focus_reports.rs 2686 `a_second_focus_checked_before_the_first_is_handled_is_said_once` (comment: "GitHub run 44344991044") and listener_replays.rs 262 `the_consoles_repeated_focus_says_terminal_once` (replays/conhost_editing_44344991044.jsonl).
- focus_reports.rs 2713 `refused_focuses_are_let_go_so_each_check_reads_its_own_focus_alone` and listener_replays.rs 297 `refused_text_box_focuses_are_let_go_so_each_check_reads_its_own_focus` (replays/measure_focus_text_box_uia_focuses.jsonl).

A keeps the replays, since they carry the recorded order the standing rule asks for, and deletes the focus_reports pair (75 lines) once M1 has them run the real check. B offered either. Resolution: keep the replays, and delete the focus_reports pair after M1. The replay's own limit stands in its module comment (listener_replays.rs 16-21: "their keyboard-focus answers are made up ... The console replay's 'terminal said once' rests on that made-up answer"). A's proposal to move listener_replays.rs into verbatim-listener-logic, where it would run on the Linux job, does not hold: it uses `verbatim_process::held_slot::HeldSlot` (line 48), and verbatim-process is in the Windows tier (xtask/src/tiers.rs 100). It also depends on verbatim-core. It stays in mockapp's tests unless `HeldSlot` moves.

The duplicates across crates, named by A, are real. focus_reports.rs 2113 corresponds to verbatim-core validity_tests.rs 360 and focus/duplicate.rs 198. msaa_events.rs 99 and verbatim-core focus_speech_tests.rs 547 share a name. They are not removals: the mockapp tests cross the process boundary.

### M12. Stand-ins and recordings (both; test finding)

The three tests that compare a stand-in with a recording (focus_reports.rs 540, 722, 845) all write the recording's facts out by hand: `grep -n "deldesk\|np_open"` in focus_reports.rs finds nothing. Two of the recordings are in the repository: crates/verbatim-core/tests/fixtures/recordings/deldesk_run1.jsonl, whose source line reads "verbatim-event-record, deldesk (2026-11-18), run 1", and np_open_run1.jsonl. The third, "recording R4 of the keys-and-text study" (focus_reports.rs 836), is not. The obligation comes from CLAUDE.md: "A stand-in for an application (mockapp, a scenario's fixture) is checked against a recording of the application it imitates". The stand-in table follows.

| Stand-in | Real application it imitates | Recording checked against | Tests that use it | Scenarios that use it |
|---|---|---|---|---|
| uia.rs and msaa.rs scripted trees | generic UIA and MSAA providers | none | nearly every test file | every mockapp scenario |
| list_view.rs (`list_view*.json`, bidi_marks.json) | comctl32 SysListView32; the deletion imitates the desktop's list (File Explorer) | crates/verbatim-core/tests/fixtures/recordings/deldesk_run1.jsonl, compared by focus_reports.rs 540 from hand-written facts | native_controls, call_counts, window_messages, focus_reports | twin_item_at_a_deleted_items_ids (drifted copy, M5) |
| tree_view.rs (tree_view.json, stale_tree.json) | SysTreeView32, Windows Forms' wrapped tree, msconfig's state images; the stale focus imitates Notepad's Open dialog folder tree | stale focus only: recordings/np_open_run1.jsonl, compared by focus_reports.rs 722 from hand-written facts | native_controls, msaa_events, call_counts, focus_reports | stale_focus_behind_a_caret_key |
| edit.rs (MSAA fixtures with `text`; read_only_field.json; stale_tree.json) | the Win32 EDIT control; the read-only Location field of a file's Properties | "recording R4 of the keys-and-text study", not in the repository (focus_reports.rs 845) | focus_reports, window_messages, call_counts, text and others | read_only_field_typing, stale_focus_behind_a_caret_key |
| buttons.rs (group_box.json, boundary_uia_*host.json) | a Win32 dialog's group box and push buttons | none | native_controls, msaa_events | none |
| uia_window.rs (uia_island*.json, dead_island.json, boundary_win32_host.json) | a XAML island in a Win32 window | none | focus_reports, native_controls, dead_objects | none |
| tip_window.rs (tree_view.json's tooltips_class32 nodes) | a help balloon and a tooltip | none | msaa_events, native_controls, call_counts | none |
| late_dialog.rs | Import a Theme (a hidden common file dialog) | the flight-recorder dump crates/verbatim-core/tests/fixtures/dialog_shown_import.jsonl; no test compares | msaa_events (1 test) | none |
| popup.rs | a combo box's drop-down list | none | none (M2) | none |
| menu.rs | a Win32 #32768 popup menu | none | none (M2) | none |
| held_message.rs | none (a test control that holds a message) | n/a | window_messages | none |
| window.rs `dialog_keys` and `focus_on_activate` (owned_dialog.json, owner_window.json) | an owned dialog across processes | none (the scenario's expectations are NVDA's capture of the same fixture) | events, uia_tree | owned_dialog_across_processes, event_recorder; `focus_on_activate` in eight scenarios |
| uia/text.rs | the UIA text pattern of a document or terminal | none | text, caret_watch, terminal* | spelling_errors, terminal scenarios |
| reuse.json | File Explorer's runtime-id reuse between a folder and its subfolder (focus_reports.rs 132, 345) | none | focus_reports | none |
| controller.json, replaced_suggestions.json, typed_search.json | Settings' search box with suggestions | none | controller_for, focus_reports, instruction_limit | controlled_list_selections (its own inline fixture, which differs) |
| combo_box.json | a time picker's list (combo_box.rs 2) | none | combo_box, call_counts, remote_ops | none |
| taskbar.json | Shell_TrayWnd | none | slow_application | none |
| console_window.json | ConsoleWindowClass | none | focus_reports | none |
| toast_host.json, uia_help_balloon_client.json | a toast and a UIA help balloon | none | msaa_events | none |

object_equality.rs 11 says its expectations are read from NVDA's source and that the "capture session against this fixture is still to be made". That capture is already a roadmap item (`docs/roadmap.md` 444-445). Add first a recording for each stand-in a scenario depends on: the owned dialog and the Settings search box. Each comparison adds about 60-90 test lines. The three existing comparisons should read their recording files rather than hand-written facts.

### M13. Documents (both)

books/dev/src/how-to/mockapp.md, which is 352 lines (A's "193-464" cannot be right), lists every command again at lines 60-345, beside stdin.rs's rustdoc. It lacks `await-advised`, `list-view-focus`, `list-view-delete`, `open-dialog-shown-late`, `set-title` and `stale-focus-on-left` (`grep -c` gives 0 for each), and still lists dead ones (`write-on-read`, `hold-acc-parent`, `freeze`, `popup <id>` at line 237). books/dev/src/reference/crates/mockapp.md 548-789 restates each test file's module comment. It omits failure_paths, dead_objects, listener_replays and watch_checks (0 matches) and describes `idle_round`, `popup.json`, `console_terminal_button.json` and the `menu` module. Proposal: the guides point at the rustdoc and the module comments, which saves about 500 document lines.

### M14. The shell's effect dispatch, copied (B only; confirmed)

core_driver.rs 965-1083 dispatches Core's effects again, ending in `_ => {}` (line 1000). The `QueueEffect` arms also drop `Answered`, `SendStrokes`, `ReleaseAtExit`, `AnswerDropped`, `EndOutpost` and `StaleFrame` (`=> {}`, lines 1066-1071). The same file has a third stand-in for the shell's view sending and the listener's kept elements (core_driver.rs 1084-1188), and listener_replays.rs has its own `struct Listener` (60 onward). Proposal: the shell's dispatch becomes a library function behind a trait, and verbatim-app and the tests both call it. This changes verbatim-app, outside this area. No speech changes. No roadmap phase rewrites it.

### M15. arbitration.rs (B only; confirmed)

arbitration.rs 25-51 and 201-231 assert `raw_probe(hwnd)` against a real mockapp window, which is worth keeping. Their arbitrator halves (`arbitrator.record_probe(&mut verdicts, hwnd, true)`, then `verdict`) repeat crates/verbatim-outpost/src/arbitration.rs 675 `a_probed_verdict_lasts_until_the_window_is_destroyed` with a hand-fed verdict. Keep the probe assertions and remove about 20 lines.

### M16. A duplicated menu-opening test (B only; partly confirmed)

focus_reports.rs 2241 `a_menu_opening_on_the_current_focus_is_judged_as_that_focus` has the same input and speech as the first half of 2211 `a_menu_opening_is_judged_as_a_focus_on_the_menu`. It also asserts `CallCounts::default()`, which 2211 does not. Fold that assertion into 2211 and remove 2241, about 25 lines.

### M17. Weak assertions (A only; counts corrected)

focus_reports.rs has four `assert!(matches!(` sites (682, 698, 821, 3261) and `assert!(facts_only.read.is_none(), ...)` and `assert!(read.read.is_some(), ...)` at 693 and 695. msaa_events.rs and failure_paths.rs have none (`grep -c "matches!("` gives 0), so A's counts of 6, 2 and 3 are wrong. Replace each with a whole-value `assert_eq!`.

## 3. Test findings (crate mockapp)

| Action | Tests | Lines | Names |
|---|---|---|---|
| Remove | 7 | about 230 | events.rs `uia_set_name_raises_a_property_changed_event`, `uia_set_value_raises_a_property_changed_event`, `uia_select_raises_a_selection_event`, `msaa_set_name_raises_a_name_change_win_event`, `msaa_set_value_raises_a_value_change_win_event`, `msaa_select_raises_a_selection_win_event`, `a_command_for_an_unknown_node_is_rejected` (M8) |
| Remove after M1 | 2 | 75 | focus_reports `a_second_focus_checked_before_the_first_is_handled_is_said_once`, `refused_focuses_are_let_go_so_each_check_reads_its_own_focus_alone` (M11) |
| Remove | 1 | about 25 | focus_reports `a_menu_opening_on_the_current_focus_is_judged_as_that_focus`, its call-count assertion moved into 2211 (M16) |
| Remove | 1 (parse test) | about 6 | stdin.rs's hold-acc-parent assertions in `parses_the_slow_and_failing_parent_steps` (M2) |
| Remove dead helpers | 0 | about 100 | M9 |
| Remove if ruled | 1 | 42 | msaa_events `the_dialog_shown_late_answers_and_shows_its_hidden_dialog` (M4) |
| Replace | 0 tests; 121 tests then run the real listener code | 175 to 345 | core_driver's copies by verbatim-listener's own (M1) |
| Replace | 0 | about 650 | registration lists by a macro (M3) |
| Replace | 2 | about 20 | arbitration `uia_backend_has_a_server_side_provider_and_arbitrates_to_uia`, `msaa_backend_has_no_server_side_provider_and_arbitrates_to_msaa`, reduced to their probe assertions (M15) |
| Replace | 0 | about 120 | core_driver's effect dispatch by the shell's (M14) |
| Replace | 5 scenarios | 73 e2e lines | inline fixtures by `include_str!` of the checked file (M5) |
| Add | about 2 first, up to 8 | about 60-90 each | a recording comparison per stand-in a scenario depends on: the owned dialog and the Settings search box first (M12) |

Removable in all: 11 tests and about 1,450 lines of tests and test support, plus 42 lines if Dickson rules. No remaining test loses a defect it can catch. All of this is in the mockapp crate, apart from the e2e lines of M5.

## 4. Questions for Dickson

**Q1. Do late_dialog's unused stand-ins stay?** The ruling, late_dialog.rs 4-6: "the stand-ins for the hidden dialog's cases (Dickson, 2026-11-19; the recordings of Import, Save As, Run and Properties showed every dialog hidden at its foreground shown 14 to 221 ms later)". The fact that has changed since: the feature shipped (bc99bc40, 8c3978c7) with its coverage in verbatim-core's replay of the real Import dump and the `quit_inside_a_file_dialog` scenario, and in the day since, nothing has sent `owned-box`, `hide-close` or `tray-menu-over-hidden-dialog`. No roadmap entry names a scenario on them. There are three options:
- Delete the three unused variants (about 130 lines) and bring them back with a scenario.
- Delete the module and its self-test (359 and 42 lines) until a scenario needs it.
- Keep them and write their scenarios.

Under any option the crate guide's claim, "Checked against the recording of Import a Theme", and the test's matching comment should go: no test compares the stand-in with a recording, and the repository holds only a flight-recorder dump of Import. No speech changes under any option, since nothing that speaks uses the stand-in.

No other finding needs a ruling. `freeze`, a hand-capture tool with no user and no citing document, is put under M2 as a removal. B left it to Dickson; if he wants hand-capture tools kept regardless, it moves here.

## 5. Cost of one behaviour

Four recent commits show what adding one behaviour costs here.
- 0341332c (no echo in a read-only field) touched 23 files, +489 lines: mockapp's edit.rs, fixture.rs, main.rs, msaa.rs, stdin.rs, tree.rs, uia.rs (the 3-line rejection arm), window.rs and a fixture; a 96-line focus_reports test; Core and its tests; a scenario registered in three files (registry.rs, scenarios/mod.rs, tests/e2e.rs) with an inline copy of the fixture; and five documents.
- d1ffdb65 (the stale tree focus) touched 12 files, +303: edit.rs, msaa.rs, stdin.rs, tree_view.rs, uia.rs (the rejection arm), a fixture, a 96-line test, the scenario with an inline copy and its three registrations.
- 9aba90be (the list view's twins) also added 36 lines to common/outpost.rs and 310 to call_counts.rs.
- 7ee82b09 then touched 6 files to make one stand-in follow its recording.

A new stand-in command costs a variant, its doc and a parse arm in stdin.rs, often a parse test, an arm in each backend (one only saying "not supported"), often `NodeData` fields and threading through main.rs and window.rs, the command written up again in two documents, a fixture copied into the scenario, the test named twice in its registration list, and any listener rule mirrored in core_driver.rs.

The shared mechanisms the reviewers propose, merged:
- Commands parsed by owner (M6), which removes the other backend's arm.
- One command reference in rustdoc (M13), which removes two document edits.
- Scenarios that load the mockapp fixture file (M5), which removes the copy and its drift.
- A registration macro (M3), which removes the second listing.
- The listener's own check and routing called from the tests (M1), so a listener rule never needs a matching edit in core_driver.rs.
- The fixture root parsed once into one window-options struct (A), which removes the tuple threading through main.rs 132-140 and window.rs.

Together these remove about half of the 10 to 12 touches per behaviour.

## 6. Dropped

- A: test-coverage.md says "the scenarios on `open-dialog-shown-late` and `tray-menu-over-hidden-dialog` wait for their recordings". That quotation is not in the file today (`grep` over books and docs finds neither command name outside the crate guide).
- A: moving listener_replays.rs to verbatim-listener-logic so it runs on Linux. Contradicted: it imports `verbatim_process::held_slot::HeldSlot`, a Windows-tier crate.
- A: weak-assertion counts of 6 in focus_reports, 2 in msaa_events and 3 in failure_paths. Grep gives 4, 0 and 0; corrected in M17.
- A: 16 msaa_events tests through the check copy. Recounted as 22 through CoreDriver and 0 through `uia_tell` (M1).
- A: hits.rs saves 230 lines. Recounted as about 150 (M7).
- A: the how-to's command list at lines 193-464. The file has 352 lines; the list is at 60-345.
- B: `on-activate` "last test removed in 42541256". Contradicted: no test ever sent it; 42541256 matched on a window title. It is kept as cited by docs/nvda/main-loop-and-watchdog.md 231.
- B: `fixture_path` is dead. Contradicted: it is called at tests/common/mod.rs 424.
- B: menu's last use "before 5d75e0e5" and hold-acc-parent's "before b94fa1d8". The last uses are d17e3dba and b94fa1d8 itself.
- B: write-on-read's code is edit.rs 218-260. That range includes the watching procedure that `stale_focus_on_left` still uses; only about 20 lines of edit.rs go.
- B: slow_application.rs's four tests are removable because "each guards a deadline that no longer exists". The tests assert today's behaviour, an answer after a stall, as NVDA waits. The claim that "the place in the code no longer exists" was not verified, so it is dropped.
- B: msaa_events `a_help_balloon_shown_is_spoken` is circular and duplicates verbatim-core announcements/tests.rs 1039. Not circular: mockapp maps the role with its own table (msaa.rs 639 `fn role_to_msaa`), and the test has the outpost read a real `tooltips_class32` window across the process boundary, which the Core test does not cover.
- B: child_cleanup.rs is removable because it tests the harness. Dropped as a removal: it guards the containment whose failure leaves `cargo test` hanging on mockapp's inherited standard error, a real failure mode of the suite.
- B: stdin.rs's 19 parser tests are removable because a mistyped command fails as "unrecognized". There are 18 tests. A misparsed argument (a number, a nested `stall` or `on-activate`) is not rejected as unrecognized, so only the dead commands' parse assertions go (M2).
- B: a 0.9x ratio against "all of NVDA's tests" for the whole area. It is kept in the size table only as context, since NVDA's tests do different work.
