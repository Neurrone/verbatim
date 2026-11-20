# The accessibility API crates: merged and verified record

Area: `crates/verbatim-uia`, `crates/verbatim-ia2`, `crates/verbatim-win32`, and `crates/verbatim-uia-rops` but its caret, text, watch and terminal files (which enter only the dispatcher table). Checked against main at ba89dca7, NVDA pinned at 732d1ce2. Every quotation below was read in the file at the line given; every count names its grep.

## 1. Size

Recounted with a script that splits each file at its first `#[cfg(test)]` (shape/tests.rs counted as tests). Reviewer B's counts were right; reviewer A undercounted verbatim-uia's tests (1,248 against 2,653) and overcounted its product (9,006 against 7,601).

| Part | Verbatim product | Verbatim tests | NVDA counterpart (pinned, `wc -l`) | NVDA lines | Ratio |
|---|---|---|---|---|---|
| verbatim-uia | 7,601 | 2,653 | NVDAObjects/UIA/__init__.py 2,950; UIAHandler/__init__.py 1,600; UIAHandler/utils.py 475; UIAHandler/remote.py 369; UIAHandler/constants.py 85 | 5,479 | 1.39 |
| verbatim-uia-rops core (builder, instruction, opcode, operation, error, lib) | 3,892 | 1,078 | UIAHandler/_remoteOps/, every file | 4,321 | 0.90 |
| verbatim-uia-rops operations (focus, tree, navigate, path, focus_states, controlled) | 2,671 | 20 (their real tests are in mockapp) | inside the NVDA UIA files above; NVDA has no remote program for these | (counted above) | uia plus these: 10,272 against 5,479, 1.87 |
| verbatim-ia2 | 7,256 | 1,561 | IAccessibleHandler/__init__.py 1,318; internalWinEventHandler.py 273; orderedWinEventLimiter.py 128; IAccessibleHandler/utils.py 74; oleacc.py 373; NVDAObjects/IAccessible/__init__.py 2,917; sysListView32.py 840; sysTreeView32.py 335; NVDAObjects/window/edit.py 1,121 | 7,379 | 0.98 |
| verbatim-win32 | 1,740 | 686 | winUser.py 943; NVDAObjects/window/__init__.py 490 | 1,433 | 1.21 |
| Area | 23,160 | 5,998 | | 18,612 | 1.24 |

The area as a whole is under the 1.3 target. The UI Automation side is 1.87 times NVDA's, and more than that in truth, since NVDA's 5,479 includes its UIA text code, which Verbatim keeps in the rops caret and text files outside this area. What the extra UIA lines buy:
- Every rops operation written as a remote program, a classic twin and its own dispatcher (section 2, the dispatcher table). NVDA writes each read once, classically, and uses remote operations in a few places (UIAHandler/remote.py, 369 lines).
- Two classic ancestor walks and two classic navigation steps where one of each would do (finding F2).
- The shape mechanism (shape/mod.rs 1,241, shapes.rs 1,204), Dickson's decision 1 of 2026-10-13 in `books/dev/src/explanation/design/uia-shapes.md`; not proposed for removal.
- subscribe.rs (1,940 lines): a registering thread, groups and moves, a hand-off NVDA does on its own UIA thread; not proposed for removal.
- 31 builder methods and their instructions that no program emits (finding F1).

Deletion target, product lines and test lines:
- Remove now: about 1,600 product, about 470 test. The largest parts: dead builder methods and instructions 620 and 214; one classic ancestor walk 150; the unused previous-focus read 70; forwarders and duplicate wrappers 100; one dispatcher rule and one error classifier 90; one MSAA read 80; WinEvent kinds 80 (A's estimate); subscription variants 80 (A's estimate); dead hook code 65 and 40; parts builder 60; call counter 50 and 20; refusal table 45 and 9; navigation step 40; the table-restating tests 173 test lines.
- If Dickson rules for the removal: about 100 product and 120 test lines for the presentation rule in the reader (question Q1); about 1,100 product lines of classic twins, of which about 150 are in this area and the rest in caret.rs and text.rs (question Q2); about 150 product lines of sync markers moved behind a feature, not deleted (question Q3).
- Planned phase M6.4 (terminal reading rebuild): `terminal_rows.rs` (198 lines, no product caller) and the three terminal dispatchers' fall-back-on-anything rule.

## 2. Findings

| id | title | class | found by | file and line | what goes | product lines saved | test lines saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| F1 | 31 builder methods no program emits, with their instructions | Remove now | both (both said 33; 2 are emitted) | crates/verbatim-uia-rops/src/builder.rs 411 to 1299; instruction.rs | 31 methods, 31 Instruction variants, their opcode() and encode arms, 31 table rows | about 620 | about 214 | no |
| F2 | Two classic UIA ancestor walks and two classic navigation steps | Remove now | both | crates/verbatim-uia/src/client.rs 676, 790, 822, 945; crates/verbatim-uia-rops/src/focus.rs 996; navigate.rs 152 | Uia::ancestors and its stops, Uia::neighbor, one of two identical chain loops | about 190 | mockapp tests of the deleted walk (not counted) | no |
| F3 | The previous focus's keyboard-focus read is dead in product | Remove now | merger, settling A's ancestry divergence | crates/verbatim-uia-rops/src/focus.rs 48, 296 to 316, 504 to 511, 766, 1006 to 1013 | FocusQuery::previous, Ancestry::previous_focused, Progress::PreviousFocused, the retry in focus_ancestry | about 70 | about 10 | no |
| F4 | Thirteen remote-or-classic dispatchers with five rules and five error classifiers; the guide contradicts three rules | Remove now (non-terminal); Planned phase M6.4 (terminal) | both | rops caret.rs 583, focus.rs 285, focus_states.rs 68, tree.rs 73, terminal_snapshot.rs 208 and the rest in the table below | five private classifiers, the identical dispatcher bodies folded into text.rs's `choose` | about 90 | 0 (one test to add) | no |
| F5 | NVDA's presentable-ancestor rule applied in the UIA reader and again in Core | Question for Dickson | B only (with a correction) | crates/verbatim-uia/src/client.rs 1111, 1329, 1356, 1369; crates/verbatim-core/src/reduce.rs 1973 | is_layout, is_presentable_focus_ancestor, the filter in take_reported, is_focus_context | about 100 | about 120 | needs a capture |
| F6 | Agile-reference equality is reference identity, so the completion's shortcut is never taken | Defect | B only | crates/verbatim-uia/src/shape/mod.rs 521; crates/verbatim-outpost/src/outpost/completion.rs 358 to 363 | the dead comparison | about 8 | 0 | no |
| F7 | The per-thread call counter written twice | Remove now | both | crates/verbatim-uia/src/calls.rs 95; crates/verbatim-ia2/src/calls.rs 35 to 66 | ia2's counter | about 50 | about 20 | no |
| F8 | MSAA name, value, description and state rules in four copies; the dialog copy keeps bidirectional marks | Remove now, with one Defect | both (the bidi defect A only) | crates/verbatim-ia2/src/acquire.rs 313, 342, 366, 2146, 2614; dialog.rs 168 to 220 | one MSAA read used by snapshot, event object, live name and dialog | about 80 | 0 | dialog bidi marks only: needs a capture |
| F9 | Dead WinEvent hook code and stale per-application-outpost prose | Remove now | B only | crates/verbatim-ia2/src/hook.rs 223 to 239, 526 to 572 | APP_SUBSCRIPTIONS, WindowCache | about 65 | about 40 repointed | no |
| F10 | A global map, lock and sweep kept to choose a log level | Remove now | both | crates/verbatim-uia/src/answered.rs 31 to 39, 82 to 120 | REFUSALS, refused, window_exists | about 45 | 9 | no |
| F11 | Six hand-written parts builders | Remove now | both | crates/verbatim-uia/src/shape/shapes.rs 347, 435, 498, 766, 900, 970 | one constructor | about 60 | 0 | no |
| F12 | WinEventKind folds raw ids that tell_kind splits again | Remove now | A only | crates/verbatim-ia2/src/hook.rs 61, 188, 193, 851 to 884 | the second representation | about 80 (A's estimate) | 0 | no |
| F13 | The value-from-selection and menu-item rules written twice; stale program doc | Remove now | A only | crates/verbatim-uia/src/map.rs 170; rops focus.rs 338, 391, 472 | the control-type copy | about 30 | 0 | no |
| F14 | Forwarders and duplicate wrappers | Remove now | A and B, different instances | win32 window.rs 456; uia lib.rs 106 to 109; uia element.rs 442 to 470; checks.rs 28, 58 | the aliases and copies | about 100 | 0 | no |
| F15 | Invalid states writable in the MSAA registry | Remove now | B only | crates/verbatim-ia2/src/registry.rs 54 to 66, 238, 297, 314; equality.rs 192 | Option pairs and Option of Option become enums | about 20 net | 0 | no |
| F16 | Subscription shapes that can be written meaningless | Remove now | A and B, different instances | crates/verbatim-uia/src/subscribe.rs 210, 225 to 265, 364 to 369 | Event beside Events, Properties beside PropertyValues; three membership fields | about 100 (estimates) | 0 | no |
| F17 | The proxy's ignore list and the subscriptions drifted | Defect | B only | crates/verbatim-uia/src/proxy.rs 27 to 45; crates/verbatim-listener/src/listener.rs 1438 | a hand list replaced by one table | about 10 net | 0 | no |
| F18 | verbatim-win32 decides a console window's process twice, the second defaulting to process 0 | Defect | B only | crates/verbatim-win32/src/provider.rs 88 to 94; window_facts.rs 52, 102 to 106 | one constant, unread host carried as None | about 5 | 0 | needs a capture |
| F19 | A remote focus program's malformed ancestor is dropped silently | Defect | A only | crates/verbatim-uia-rops/src/focus.rs 722 to 731 | the `_ => None` arm becomes Error::ResultType | 0 | 0 | no |
| F20 | A failed GetRuntimeId reads as an empty id, minting a new node per event | Defect | B only | crates/verbatim-uia/src/com.rs 292 to 300; registry.rs 121 to 176 | the default | 0 | 0 | needs a capture |
| F21 | role_from_msaa's callers each convert an Option by hand | Remove now | B only | crates/verbatim-ia2/src/acquire.rs 303, 675, 874, 2159, 2379, 2550; dialog.rs 154, 164 | eight copies of one conversion | about 10 | 0 | no |
| F22 | Stale documents, and figures written twice | Remove now | both, plus two found in verification | listed in its paragraph | prose | 0 | 0 | no |
| F23 | terminal_rows has no product caller | Planned phase M6.4 | merger, from the dispatcher table | crates/verbatim-uia-rops/src/terminal_rows.rs | the operation | 198 | the comparison test | no |

### The dispatcher table (the first point asked)

Each named operation has a remote entry, a classic twin and a dispatcher. Lines are the entry function's own, from its `fn` line to its closing brace, without doc comments or helpers. "Classic runs when" quotes the dispatcher's match arm or its first branch. Every dispatcher also runs the classic twin alone when its `remote` argument is false (`if !remote { return ..._classic(...) }`), except tree_dump, which then reads classically by the same `Dump`.

| Operation (file) | Remote entry, lines | Classic twin, lines | Dispatcher, lines | Classic runs when, besides `remote` false (quoted) | Product callers that reach the classic path | Tests that run both and compare |
|---|---|---|---|---|---|---|
| caret_read (caret.rs) | caret_read_remote 1353 to 1441, 89 | caret_read_classic 1885 to 1988, 104 | 583 to 592, 10 | `Err(error) if gone_or_timed_out(&error) => Err(error), Err(error) => caret_read_classic(query)` | verbatim-outpost text/uia.rs 652, with `self.remote` set by `.remote(bool)` | mockapp text.rs caret_both (used by remote_and_classic_caret_reads_agree and others), a_failing_attribute_is_not_supported, every_attribute_is_read_both_ways, unsupported_attributes_are_found_both_ways |
| controlled_selection (controlled.rs) | controlled_selection_remote 106 to 221, 116 | controlled_selection_classic 238 to 251, 14 | 69 to 84, 16 | as caret_read | verbatim-outpost outpost/asked.rs 880, `remote = context.tries_remote(window)` (879) | none found |
| focus_ancestry (focus.rs) | focus_ancestry_remote 495 to 787, 293 | focus_ancestry_classic 996 to 1179, 184 | 285 to 322, 38 | after a rerun without `previous` on `is_element_gone`, `if gone_or_timed_out(&error) { return Err(error); } focus_ancestry_classic(uia, query)` | outpost read.rs 920 (`remote = !window.is_some_and(... plain_window_above ...)` at 916, reached only when `context.tries_remote(window)` at 881); the listener calls focus_ancestry_classic directly (listener/asked.rs 814) | mockapp remote_ops.rs both_agree (used by a_deep_chain_reads_the_same_both_ways and five more), a_cells_table_is_read_alike_by_the_program_and_the_classic_way, a_provider_that_has_exited_fails_at_once |
| focus_states (focus_states.rs) | focus_states_remote 105 to 199, 95 | focus_states_classic 210 to 223, 14 | 68 to 83, 16 | `Err(error) if gone_or_timed_out(&error) && !is_gone(&error) => Err(error)`, else classic: a gone element falls back | outpost completion.rs 442, `context.tries_remote(Some(window))` | outpost read.rs a_console_window_reads_enabled_remotely_and_classically (asserts the two differ: remote `(None, false)`, classic `(Some(true), false)`) |
| navigation_step (navigate.rs) | navigation_step_remote 93 to 140, 48 | navigation_step_classic 152 to 181, 30 | 67 to 80, 14 | as caret_read | outpost read.rs 2095 always passes `true`, so the twin runs only as a fallback; the outpost's own classic path is Uia::neighbor (read.rs 1979) | mockapp remote_ops.rs a_navigation_step_reads_the_same_both_ways, a_cells_table_is_read_alike_by_the_program_and_the_classic_way |
| element_by_path (path.rs) | element_by_path_remote 86 to 135, 50 | element_by_path_classic 153 to 185, 33 | 54 to 69, 16 | as caret_read | outpost focus_path.rs 93, `remote` from completion.rs 179 `context.tries_remote(...)` | none found |
| tree_dump (tree.rs) | tree_dump_remote 102 to 108, 7 | tree_dump_classic 120 to 126, 7 (both drive one `Dump`) | 73 to 92, 20 | per run: `Err(Stop::Run(error)) if !is_timeout(&error) => path = Path::Fallback(error)`: a gone element and any other failure continue classically from where the run stopped | outpost read.rs 2268, `remote = context.tries_remote(Some(hwnd))` | none comparing; time_limits.rs uses the classic one alone |
| text_units (text.rs) | text_units_remote 637 to 814, 178 | text_units_classic 1248 to 1345, 98 | `choose` 299 to 313, 15, shared | `Err(error) if gone_or_timed_out(&error) => Err(error), Err(error) => classic_fn(query)` | outpost text/uia.rs 773 | mockapp text.rs remote_and_classic_text_reads_agree, a_batch_in_several_languages_gives_each_line_its_own, a_backward_move_is_counted_backward_both_ways, reading_ahead_ends_where_lines_clamp_back_both_ways |
| text_range (text.rs) | text_range_remote 821 to 915, 95 | text_range_classic 1385 to 1442, 58 | 341 to 364, 24 | as caret_read, and a program that selected but left `unread` is completed by classic_changes_since, not by the twin | outpost text/uia.rs 824 | text.rs remote_and_classic_text_reads_agree |
| text_location (text.rs) | text_location_remote 1080 to 1100, 21 | text_location_classic 1499 to 1509, 11 | `choose`, shared | as text_units | outpost text/uia.rs 886 | text.rs remote_and_classic_text_reads_agree |
| watch_check (watch.rs) | watch_check_remote 418 to 455, 38 | watch_check_classic 484 to 575, 92 | 123 to 132, 10 | as caret_read | outpost text/uia.rs 724 | mockapp watch_checks.rs the_program_and_its_classic_reads_agree |
| terminal_snapshot (terminal_snapshot.rs) | terminal_snapshot_remote 315 to 346, 32 | terminal_snapshot_classic 538 to 621, 84 | 208 to 222, 15 | `Err(error) => terminal_snapshot_classic(uia, query)`: any error, the timeout included | outpost terminal.rs 245 and 328 | mockapp terminal_snapshot.rs `both` (used by four tests), colours_are_read_as_exact_runs_at_any_width_hint_both_ways |
| terminal_find (terminal_find.rs) | terminal_find_remote 121 to 135, 15 | terminal_find_classic 320 to 383, 64 | 97 to 109, 13 | `Err(error) => terminal_find_classic(uia, query)`: any error | outpost terminal.rs 369 | mockapp terminal.rs find_on_path runs one path per test, by its `remote` argument |
| terminal_rows (terminal_rows.rs) | terminal_rows_remote 103 to 126, 24 | terminal_rows_classic 176 to 198, 23 | 65 to 79, 15 | `Err(error) => terminal_rows_classic(uia, query)`: any error | none: no product crate calls any of the three (grep `terminal_rows` over crates/, only lib.rs exports and mockapp tests) | mockapp terminal_snapshot.rs rows_by_their_place_are_read_from_the_kept_start_both_ways |

Tests that only compare: none found. Every comparison test read (both_agree's callers, a_navigation_step_reads_the_same_both_ways, caret_both's callers, terminal_snapshot.rs `both`'s callers, the console focus_states test) also asserts expected values. The comparison grep: `grep -rnE "(caret_read|...|watch_check)_(classic|remote)\b"` over crates/, xtask/ and uia-facts/ outside rops: product hits only listener/asked.rs 37 and 814, and outpost read.rs 2353 to 2382, which is inside read.rs's `#[cfg(test)]` module (it starts at 2296).

Rules, settled between the reviewers: 13 dispatcher bodies serve 14 entry points (B's thirteen was right; A's six counted only its own files). There are five rules, not four: gone or timeout returned (caret, controlled, navigation, path, watch, text); that plus a rerun without `previous` (focus_ancestry); only a timeout returned, gone falls back (focus_states); per-run, only a timeout returned (tree_dump); any error falls back, timeout included (the three terminal operations). The crate guide's "Fallback rules" says: "A gone provider and one that times out (`UIA_E_ELEMENTNOTAVAILABLE`, `UIA_E_TIMEOUT`) are returned by every entry point without the classic calls" (books/dev/src/reference/crates/verbatim-uia-rops.md 719 to 721); focus_states, tree_dump and the three terminal operations contradict it. Five private classifiers exist: gone_or_timed_out (caret.rs 596), is_element_gone (focus.rs 325), is_gone (focus_states.rs 86), is_timeout and is_gone (tree.rs 129, 137), each comparing one or two HRESULTs, while `verbatim_uia::element_is_gone` (com.rs 40 to 52) counts eight, RPC_E_SERVER_DIED among them. So a program whose provider process died is retried classically, one more failed call; navigation_step_classic itself already uses the eight-code test (navigate.rs 172).

focus_states' exception has a reason the code shows but no document states: its classic twin answers each dialog's children separately (`uia.children(dialog, ...).unwrap_or_default()`, focus_states.rs 216 to 219), so one gone dialog does not lose the focus's states, where a remote run fails whole on a gone import. git log -L of the dispatcher shows it was written so in 87e2dc30 (checked with git show; the commit message does not give the reason).

Why the classic twins exist, from the documents:
- The listener makes no remote operation: "The classic walk is the focus listener's read, which makes no remote operation (`books/dev/src/explanation/design/listener-without-connections.md`, section 2), and the outposts' fallback" (verbatim-uia-rops.md 276 to 278). Only focus_ancestry_classic serves this.
- A client-side proxy cannot run a program: "An element served by a client-side proxy (UIA's MSAA proxy inside Verbatim's own process) fails with `E_UNEXPECTED`, since there is no provider process to run in" (verbatim-uia-rops.md 121 to 123), and "An import that fails (`Error::Import`, a client-side proxy) marks the window for its lifetime, and the outpost then reads it classically without trying" (707 to 708). The mark is `read_classically` (outpost/mod.rs 584) read by `tries_remote` (576).
- Any other failed run, an unsupported instruction or the instruction limit among them: "A run that fails otherwise is answered classically for that call (`Path::Fallback`)" (716 to 717), and "A run that exceeded the limit would be answered classically for that call" (751).
- Size limits answered classically by design: a controlled list of "more than about 280 is answered classically" (696), a terminal row "wider than about 1,300 is answered classically" (423), dialog children beyond DIALOG_CHILDREN_PER_RUN (60) "for the caller to read classically" (focus_states.rs 22 to 27).
- A developer setting: "`uia.remote_operations` in `settings.toml`, on by default) forces the classic walk; the supervisor passes it to each outpost as `--classic-uia`" (723 to 725).
- A window whose focus has a plain window above it: "The program cannot judge a window between its steps, so a focus with a ghost's or a hung application's window above its own is read by the classic walk" (outpost read.rs 913 to 916).
- As the reference the comparison tests check programs against (tree_dump_classic's doc: "the fallback and the reference").
- ARM64: architecture.md R3 says only "verify on ARM64 before relying on remote operations there" (architecture.md 1201 to 1205); no document says a classic twin serves ARM64.
- uia-bugs.md names no window or provider on which a program cannot run; its two remote-operation entries (383 and 417) concern differing answers, not failures. No log, recording or test in the repository counts how often Path::Fallback or an import failure happens on a real application.

### F1: 31 builder methods no program emits

Both reviewers listed 33. Grep run for each name over crates/, xtask/ and uia-facts/, outside builder.rs, for `.name(` or `name(`: 0 hits for all 33. Inside builder.rs, get_operation_status and set_operation_status are called by `try_catch` (builder.rs 1409 and 1411: `let status = self.get_operation_status(); ... self.set_operation_status(zero);`), and try_catch has 13 callers (caret.rs 516, focus.rs 616 and 804, focus_states.rs 135 and others). So those two are emitted by every program with a try block and stay; 31 are dead: and_assign, array_remove_at, call_extension, divide, divide_assign, get_metadata_value, is_extension_supported, lookup_guid, lookup_id, map_remove, map_size, multiply, multiply_assign, new_char, new_double, new_guid, new_point, new_rect, point_property, rect_property, string_get_at, substring, text_pattern2_get_caret_range, text_range_add_to_selection, text_range_compare, text_range_find_attribute, text_range_get_children, text_range_get_enclosing_element, text_range_remove_from_selection, text_range_scroll_into_view, text_range_show_context_menu. For example builder.rs 604 to 612 `pub fn divide`.

What goes with them, counted by script over instruction.rs: 31 Instruction variants, 145 lines of the enum (88 to 493); 31 one-line arms of `opcode()` (499 onward); 31 patterns in `encode` (603 to 1016), 164 lines, most of them or-patterns sharing a body, so no body goes; 31 rows of the byte table in the test module (1045 onward), 214 lines. Builder methods with their doc comments: 282 lines. Each variant's name has no other hit in crates/ but instruction.rs and opcode.rs (grep -rnw per variant). The opcode constants in opcode.rs stay: they are NVDA's table (`Opcode` "is the table NVDA uses and five more", verbatim-uia-rops.md). Total about 620 product and 214 test lines; reviewer A's 590 and 250 was closer than B's 300 and 250.

### F2: two classic ancestor walks and two classic navigation steps

Both found it; A's client.rs line numbers (307 to 411) were wrong, B's are right. `Uia::ancestors` is client.rs 676 (one GetParentElementBuildCache per hop), called only at outpost read.rs 335 inside `uia_ancestors`, whose callers are read.rs 289, 705 and 1739; the outpost uses it when `!context.tries_remote(window)` (read.rs 881 returns None to it). The rops classic walk is focus.rs 996 to 1179, used by the listener (asked.rs 814) and as the outposts' fallback. `Uia::ancestors_from` (client.rs 790 to 811) and `Uia::ancestor_chain_from_parts` (822 to 843) are the same loop, character for character but for `take_shaped_ancestor` against `take_ancestor_parts`. Navigation: `Uia::neighbor` (client.rs 945 to 969) is run by the outpost's uia_step (read.rs 1966 to 1987) with its own `nearest_window_handle`, whenever uia_remote_step answers None; `navigation_step_classic` (navigate.rs 152 to 181) makes the same walk, the same gone rule (`Err(error) if verbatim_uia::element_is_gone(&error) => return Err(...)`, `Err(_) => None`) and the same window read. Both fill a neighbor's table (Uia::neighbor through `snapshot_in_table`, shapes.rs 854 to 858; the twin through `Cached::table`), so the outputs match. Proposal: the outpost calls `navigation_step(.., false)` and `focus_ancestry(.., false)` where it now calls the older pair; one generic chain loop. About 150 product lines for the walk and its stops, 40 for the step. Speech: no change, the two make the same calls. M6.2 (outpost retirement) does not name these reads.

### F3: the previous focus's keyboard-focus read is dead in product

A reported that the program and the classic walk answer a failed read of `previous` differently: the classic walk maps any error to Gone (focus.rs 1013 `Some(Err(_)) => Some(EarlierElement::Gone)`), the program maps a non-boolean to NotFocused (766 to 771), and the model's doc says Gone is "Its keyboard focus could not be read, other than by time" (verbatim-model event.rs 264). All three quotations are right. But both product FocusQuery literals pass `previous: None` (outpost read.rs 898, listener asked.rs 800; grep `FocusQuery {` outside mockapp gives these two and the dispatcher's own rerun), and Core now derives Gone from the outpost's `runtime_id_reused` (verbatim-core focus/execute.rs 399), as `docs/design/readers-tell-core-asks-build1.md` 1069 records: "a focus answer says only whether its runtime id named another element (`runtime_id_reused`)". The listener ignores the progress arm (`Progress::PreviousFocused(_) => {}`, asked.rs 561). So the divergence never runs; the field, the program's import and read, the classic read, the dispatcher's rerun on a gone import (focus.rs 296 to 316), the Ancestry field and the Progress arm go, about 70 lines; mockapp instruction_limit.rs 274 and 357 and remote_ops.rs 287 set or compare it. With the rerun gone, focus_ancestry's dispatcher is the common rule (F4). uia-bugs.md 573 still says the listener reads this "in the remote operation that reads the focus's ancestry": stale.

### F4: one dispatcher rule and one error classifier

Both found it; see the dispatcher table for the quotations. Proposal: the seven dispatchers that apply the common rule use text.rs's existing `choose` (299 to 313), made public to the crate; the five private classifiers give way to `verbatim_uia::element_is_gone` and `timed_out`; focus_states' and tree_dump's exceptions and text_range's unread completion stay, each with its reason in the guide; the guide's "every entry point" sentence is corrected. About 90 product lines (A's estimate; B's 180 assumed all thirteen collapse, which the exceptions prevent). Speech: none; a dead provider answers gone either way. The terminal dispatchers' any-error rule, which retries a timed-out terminal read classically after a second wait, is in code M6.4 rewrites (`docs/design/terminal-rebuild.md` 35 counts "the `verbatim-uia-rops` snapshot and find" in the rebuild). Test to add: one unit test of the shared rule over fake remote and classic functions, about 40 lines.

### F5: the presentable-ancestor rule in the UIA reader and in Core

B only; confirmed, with one correction. NVDA's rule (nvda/source/NVDAObjects/__init__.py 1164 to 1177): `if self.presentationType in (self.presType_layout, self.presType_unavailable): return False` then `if self.role in (controlTypes.Role.TREEVIEWITEM, controlTypes.Role.LISTITEM, controlTypes.Role.PROGRESSBAR, controlTypes.Role.EDITABLETEXT): return False` and `return True`. presentationType is 910 to 975 (invisible is unavailable; UNKNOWN, PANE, TITLEBAR, WHITESPACE, BORDER and others are layout; static text without text is layout; WINDOW, PANEL, PROPERTYPAGE, GROUPING and others without name and description are layout), and for UIA NVDAObjects/UIA/__init__.py 2121 to 2128 adds: "UIA NVDAObjects can only be considered content if UI Automation considers them both a control and content." NVDA judges it when speaking, in `event_focusEntered` (NVDAObjects/__init__.py 1358 to 1363), over `api.getFocusAncestors()[api.getFocusDifferenceLevel() :]` (eventHandler.py 418), and api.py 104 to 147 builds the ancestors and the difference level from every parent, unfiltered.

Verbatim's copies:
- UIA reader, client.rs: `is_layout` (1329 to 1342: `Role::Unknown | Role::Pane | Role::TitleBar => true`, blank StaticText, and `Role::Window | Role::PropertyPage | Role::Group` blank in name and description), `is_presentable_focus_ancestor` (1369 to 1378: not layout and not `Role::ListItem | Role::TreeItem | Role::ProgressBar | Role::EditableText`), applied in `take_reported` (1111 to 1141, `if presentable && content { ... chain.push(snapshot)`) and in `Ancestor::is_focus_context` (1356 to 1360), which the outpost calls at read.rs 691.
- Core, reduce.rs 1973 to 2003 `is_presentable_container`: Invisible; TreeItem, ListItem, EditableText, ProgressBar, BusyIndicator, TitleBar, Unknown, Border, Whitespace, MenuBar, Menu, MenuItem false; Pane only when `top_level_client`; Window, Group, PropertyPage named or described; StaticText named.
- What differs: the reader has the control-and-content test, which Core cannot apply since no such fact reaches it; Core has invisibility, BusyIndicator, Border, Whitespace, the menu roles (NVDA handles menus in event_focusEntered by cancelling speech, not in this rule) and the top-level client pane exception, which the reader lacks. Every entry the reader drops but the control-and-content ones Core would also judge not presentable.
- The MSAA path, correcting B: verbatim-ia2's `ancestor_chain_until` (acquire.rs 997 onward) pushes every ancestor, but the outpost then drops every MSAA window object, `.filter(|ancestor| ancestor.role != Role::Window)` (outpost read.rs 1130), citing NVDA's GenericWindow (`isPresentableFocusAncestor = False`, NVDAObjects/IAccessible/__init__.py 2301 to 2303). So the two readers send Core differently filtered chains: UIA by the full rule, MSAA by window objects only. `docs/parity/focus-context.md` 110 to 113 says "The readers leave out of a chain the entries that are not presentable ... an MSAA window object and every layout container"; for MSAA layout containers that is not so.
- Decision of record it touches: ADR 0003, "Core holds every rule (Dickson, 2026-11-12)": "Core holds every rule that decides what is spoken", and D14 as amended there: "a reader judges nothing but NVDA's local drops and the listener's drops on an event's own cached class and automation id". The reader's filter is a judgement outside both, so the change would carry those decisions out, not change them; it is a question only because the chain Core compares changes.

Speech: needs a capture (see Q1).

### F6: agile-reference equality never holds for a fresh reference

B only; confirmed. windows-core 0.62.2 (the version in Cargo.lock) declares `#[derive(Clone, PartialEq, Eq)] pub struct AgileReference<T>(imp::IAgileReference, PhantomData<T>);` (agile_reference.rs 5 to 7), and `new` calls `imp::RoGetAgileReference` (11 to 23). IAgileReference is declared by `define_interface!` (imp/com_bindings.rs 48 to 52), which derives PartialEq over its IUnknown (imp/mod.rs 63 to 64), whose `eq` compares the raw pointers or the IUnknown pointers QueryInterface returns (unknown.rs 49 to 62). So equality is the COM identity of the agile-reference object, not of the element. `Cached::agile` makes a new one each call (shape/mod.rs 445 to 454, `AgileReference::new(&self.live.element)?`), and the crate's own doc of `same_as` says so: "Whether `other` is this same agile reference, as a clone of it is: an element kept again gets another" (shape/mod.rs 518 to 519). In `replaced_stale` (completion.rs 345 onward), `let Ok(found) = found.agile() else { return false; }; if kept.reference() == found.reference() { return false; }` (358 to 363) compares the kept reference with one made on the line before; it can be equal only if RoGetAgileReference returned the very object kept earlier, which nothing in the code arranges. So the shortcut is not taken and every call with a found element resolves the kept element and refreshes it, one cross-process call. Grep `same_as(` and `reference() ==` over crates/: the registry's `evict_element_if` (registry.rs 288) is correct, since it compares clones of one reference. B's proposed fix, comparing runtime ids, would be wrong: the kept element is found by that same runtime id (`context.nodes().existing(runtime_id)`, completion.rs 351), so the two always match and the stale check, which exists for a reused runtime id, would never run. Fix: delete the comparison (the cost it meant to save is already paid today), or compare the elements by UIA's CompareElements if a measurement shows the call matters. Speech: no change.

### F7: the call counter written twice

Both found it. verbatim-ia2/src/calls.rs 35 to 66 holds `thread_local! { static COUNTS: Cell<CallCounts> ... }` and a `count` identical to verbatim-uia's (calls.rs 95 to 106: `verbatim_win32::held_locks::check("a counted call into an application"); verbatim_win32::calls::check()?; COUNTS.with(...)`), and the outpost adds them: `verbatim_uia::calls::take() + verbatim_ia2::calls::take()` (worker.rs 91). One counter in verbatim_win32::calls beside the gate both consult. About 50 product, 20 test lines.

### F8: the MSAA read rules in four copies

Both found the dialog's copy; A found the rest. Confirmed: `EventObject::name` (acquire.rs 313 to 335) re-implements `live_name` (2614 to 2640) from `labelled_name` (2591) and `columned_name` (2599); `EventObject::description` (342 to 354) and `value` (366 to 380) repeat the list-view and tree-view rules of read_snapshot_with (2146) and value_for (2356). `DialogObject::seen` (dialog.rs 168 to 186) copies the multi-line rule of role_and_states (acquire.rs 2323 to 2329) without `adjust_role_and_states` (map.rs 157 to 163, which only turns a mixed progress bar into a busy indicator, a role dialog text never keeps, behaviors.py 135 to 145), and dialog.rs 30 to 36 keeps `invisible` beside the states "which the model has no state for", while map.rs 149 maps `set(STATE_SYSTEM_INVISIBLE, State::Invisible);` (so map.rs 46's comment is stale too). The one behaviour difference: DialogObject's name, value and description (dialog.rs 202 to 220) never strip bidirectional marks, which every other MSAA read and UIA's `DialogText::parts` (shapes.rs 975, `strip_bidi_controls([&mut name, &mut value, &mut description])`) do; grep `strip_bidi` over the outpost, Core and reader-rules finds no later strip. Sequence: an MSAA dialog whose static text holds U+200E between digits is gathered with the mark; the same text read through UIA without it. Whether a synthesizer speaks the mark needs a capture. About 80 product lines.

### F9: dead hook code

B only; confirmed. Grep `APP_SUBSCRIPTIONS` over crates/: hook.rs's own tests (1134, 1211, 1220, 1230, 1286, 1302, 1495, 1523, 1530), lib.rs's export and doc, and mockapp tests/events.rs 28 and 643; no product crate. Its doc of CONSOLE_SUBSCRIPTIONS says "No other outpost hooks a `WinEvent`" (hook.rs 250 to 251). `WindowCache<T>` (hook.rs 526 to 572) is exported (lib.rs 72) and named nowhere else (grep `WindowCache` over crates/). Prose still describing per-application outpost hooks: hook.rs 587 ("The per-application outpost passes [`APP_SUBSCRIPTIONS`]") and lib.rs 15. About 65 product lines; the hook and mockapp tests that use the set are repointed at CONSOLE_SUBSCRIPTIONS or LISTENER_SUBSCRIPTIONS (about 40 lines).

### F10: the refusal table

Both found it. answered.rs 38: `static REFUSALS: LazyLock<Mutex<HashMap<isize, u32>>>`; `checked` takes its lock on every lookup, answered or refused (82, `refusals.remove(&window)` on an answer), and `refused` (110 to 119) sweeps it with IsWindow per entry; its only use is choosing `tracing::info!` over `tracing::debug!` past REFUSALS_BEFORE_INFO (31). Log each refusal at debug with its window. About 45 product lines and the 9-line test a_destroyed_windows_refusals_are_dropped_at_the_next_refusal.

### F11: six parts builders

Both found it. shapes.rs: ListenerEvent::parts 347, ListenerSelection::parts 435, PropertyEvent::parts 498, Node::parts 766, Ancestor::parts 900, DialogText::parts 970, each calling `strip_bidi_controls` on its own fields (349, 449, 502, 792, 920, 975); Cached<Ancestor>::snapshot (929 to 936) copies the class name again. One constructor. About 60 lines.

### F12: WinEventKind folded and split again

A only; confirmed. SUBSCRIPTIONS maps `(EVENT_OBJECT_SELECTIONADD, WinEventKind::StateChange)` (hook.rs 188) and `(EVENT_SYSTEM_MENUPOPUPEND, WinEventKind::MenuEnd)` (193); `tell_kind` splits them again from the raw id: `WinEventKind::StateChange => match event.event { EVENT_OBJECT_SELECTIONADD => EventKind::SelectionAdd, ...` and `WinEventKind::MenuEnd if event.event == EVENT_SYSTEM_MENUPOPUPEND => EventKind::PopupEnd` (858 to 869). The saving (A's 80 lines) was not recounted.

### F13: the value-from-selection and menu-item rules twice

A only; confirmed. map.rs 170 to 175 `value_from_selection`: `!value_available && (role == Role::ComboBox || (role == Role::List && class_name.is_some_and(|class| class.contains("LoopingSelector"))))`; rops focus.rs 338 to 345 `wants_selected_child` states the same by control type id, and `reads_selected_child` (354 to 372) and the program (612 to 625) apply the Value clause. Menu items: map.rs 159 `role == Role::MenuItem && !states.contains(State::Checkable)` and focus.rs 391 to 393 `element.control_type == Some(UIA_MenuItemControlTypeId.0)`. The program's doc at focus.rs 472 to 473 still says it "reads a list's or tab control's selected child", which the code (and the 2026-11-17 ruling quoted at 334 to 336) no longer does. About 30 lines.

### F14: forwarders and duplicate wrappers

A's and B's instances, each confirmed: win32 window.rs 456 to 458 `served_by` only calls `crate::provider::served_by`; uia lib.rs 106 to 109 re-exports nine verbatim_win32::provider functions; uia element.rs `visible_ranges` (442), `ranges_of` (452) and `attribute_value` (463) repeat text.rs 128, 78 and 359; checks.rs 28 redeclares `UIA_E_TIMEOUT` and its `answer` (58 to 61) repeats com.rs `timed_out`. A's other count, nine Interning methods forwarded in ia2 registry.rs, was not recounted. About 100 lines together.

### F15: invalid states in the MSAA registry

B only; confirmed, B's line numbers drifted. registry.rs 54 to 66: `Node { key, held: Option<Held>, role: Option<Role>, at_address, facts: Option<Facts> }`; `id_for` inserts `(None, None)` (238), `insert` `(Some(role), Some(facts))` (297), so `(Some, None)` is writable and `facts_of` must `node.facts.clone()?` (314); equality.rs 192 then defaults `kept.role.unwrap_or(Role::Unknown)`. `Facts.identity` (registry.rs 80 to 84) and acquire.rs `Reading.identity` (145 to 152) are `Option<Option<..>>` under `#[expect(clippy::option_option, ...)]`. Two enums. About 20 lines net.

### F16: subscription shapes

A's Event and Events, Properties and PropertyValues (subscribe.rs 225 to 265, each with its own handler type, PropertyHandler 1673 and PropertyValueHandler 1694), and B's three membership fields (`extended_only`, `text_events`, `alone`, 364 to 369, read together by `Group::holds` at 210). Confirmed at their declarations; the line estimates (80 and 25) are the reviewers'.

### F17: the proxy's ignore list

B only; confirmed. proxy.rs 27 to 45 lists FOCUS_PROPERTIES and six automation events; the listener also subscribes `event: UIA_Window_WindowOpenedEventId` (listener.rs 1438), not on the list. NVDA builds its ignore map from its one event table: `for id in UIAEventIdsToNVDAEventNames: ignoreWinEventsMap[id] = [0]` (UIAHandler/__init__.py 291 to 295), WindowOpened among them (268). Speech: no change, since a window opened is a toast only with `id.contains("ToastView")` (map.rs 416 to 419); the cost is extra events in the listener. One table read by both.

### F18: a console window's process decided twice

B only; confirmed. provider.rs 88 to 94 answers `Provider::UnreadConsole` when the host cannot be read; window_facts.rs 52 declares its own `const CONSOLE_CLASS: &str = "ConsoleWindowClass";` and `facet` answers `reads.console_host(window).unwrap_or(0)` (104), so an unread host becomes `Pid(0)` for acceptance. What acceptance then says needs a capture. One constant, the unread host carried as None.

### F19: a malformed ancestor dropped silently

A only; confirmed. focus.rs 722 to 731: `.filter_map(|(value, absent)| match (value, absent) { (Value::Element(element), Value::Int(absent)) => Some(...), _ => None })`; a wrong result shortens the chain with no error, where Error::ResultType exists for it. Reachable only by a program bug; speech unchanged in a correct program.

### F20: a failed runtime id read as empty

B only; confirmed. com.rs 292 to 300 ends `.unwrap_or_default()`; registry.rs 121 to 123 says "Empty runtime IDs (unavailable elements) still get an identity so events are never silently dropped, but they are never deduplicated", so a failed read and an element without an id are one case, and each event from it is a new node to Core. What that does to duplicate-focus suppression needs a capture.

### F21: role_from_msaa's callers

B only; confirmed: grep `role_from_msaa(` with `map_or` over crates/ gives eight, acquire.rs 303, 675, 874, 2159, 2379, 2550 and dialog.rs 154, 164, each `map_or(Role::Unknown, |r| role_from_msaa(r.cast_unsigned()))`. role_from_msaa takes the Option.

### F22: stale documents and figures written twice

- books/dev/src/reference/crates/verbatim-uia.md 135 documents "`tree(root, request, registry, max_depth, max_nodes)`: a tree dump"; grep `pub fn tree(` in verbatim-uia finds none (A).
- crates/verbatim-uia/src/registry.rs 249 to 252 ("fall back to a search") and 263 to 265 ("goes straight to the search fallback") against the module doc 18 to 22 ("A node whose element is not kept is not searched for by runtime id") (B).
- crates/verbatim-uia/src/lib.rs 15 to 16: "M4's remote-ops work replaces the per-hop walk" (B).
- rops focus.rs 472 to 473 (F13), ia2 map.rs 46 (F8), hook.rs 587 and ia2 lib.rs 15 (F9).
- The crate guide's "Fallback rules" (F4).
- books/dev/src/reference/uia-bugs.md 387 says "the listener reads the sender's `HasKeyboardFocus` inside its remote operation"; the listener makes none now (`focus_ancestry_classic`, asked.rs 814) (A). uia-bugs.md 573 describes the dead previous-focus read (F3, found in verification).
- docs/parity/focus-context.md 110 to 113 claims every layout container is left out of MSAA chains (F5, found in verification).
- map.rs 408 repeats the literal `"Text Area"` that checks.rs 67 names `CONSOLE_TEXT_AREA`; map.rs 77 `(UIA_AppBarControlTypeId, Role::Unknown)` repeats the default (A).
- books/dev/src/reference/performance.md 2177 says "Pinned exactly in `crates/mockapp/tests/instruction_limit.rs`" and repeats the figures; git show --stat 1b43f866 shows performance.md (28 lines) and instruction_limit.rs (26) edited together. The document should point at the test (both; CLAUDE.md's rule on figures pinned by a test).

### F23: terminal_rows has no product caller

Found in building the dispatcher table. grep `terminal_rows\b` over crates/ outside the rops file: the lib.rs export, mockapp instruction_limit.rs and terminal_snapshot.rs, and unrelated `verbatim_text::terminal_rows` module paths. The rebuild in M6.4 owns the terminal remote operations, so it is left to that phase.

## 3. Test findings

verbatim-uia (in-crate 2,653 lines):
- Removable: 5 tests, 115 lines, that restate tables: map.rs known_control_types_map_to_expected_roles (32), unmapped_control_type_is_unknown (5), roles_nvda_maps_are_not_unknown (23), notification_kinds_map_one_to_one (27), notification_processing_maps_every_documented_value (28). answered.rs a_destroyed_windows_refusals_are_dropped_at_the_next_refusal (9) goes with F10.
- Replaceable if Q1 is ruled for Core: client.rs presentation_tests (1381 to 1558, 178 lines) by Core's tests of is_presentable_container, about 120 lines; the ancestor_chain_from_parts tests stay with the chain code.
- B's proposal to shorten seven state tests by dropping spelled-out RawUiaStates fields (about 150 lines) is a rewrite, not a removal; not recounted.
- Keep the shape recording test (decision 1 of uia-shapes.md names it).

verbatim-ia2 (in-crate 1,561 lines):
- Removable: 6 tests, 58 lines, restating the role and state table: map.rs known_roles_map_as_specified (11), roles_nvda_maps_are_not_unknown (14), roles_nvda_maps_with_no_other_counterpart_have_their_own (13), unmapped_role_is_unknown (5), focused_focusable_states_combine (8), traversed_and_linked_are_visited_and_linked (7). Keep the mixed-state and busy-indicator rule tests and hook.rs's filter tests.
- Replaceable: hook.rs tests that run APP_SUBSCRIPTIONS (1134 onward, about 40 lines), and mockapp tests/events.rs 643, repointed at a set a product process installs (F9).

verbatim-win32 (686 lines): nothing removable.

verbatim-uia-rops, this area's files (1,098 lines):
- Removable: the 31 byte-table rows of dead instructions, 214 lines, the test kept (F1).
- To add: one unit test of the shared dispatch rule, about 40 lines (F4).
- mockapp (outside the area): the comparison tests of section 2 stay while the twins stay; the tests of the deleted Uia::ancestors walk go with F2; instruction_limit.rs 274 and 357 and remote_ops.rs 287 lose their `previous` with F3.

No generated tests in the area. Totals: removable about 470 lines (12 tests and 31 rows, plus the repointed hook tests); replaceable 120 lines (Q1); to add 1 test, about 40 lines.

## 4. Questions for Dickson

### Q1: the presentable-ancestor rule in Core alone

Should the UI Automation reader report every ancestor, with its control-and-content fact, and Core alone apply NVDA's rule (F5)? Decisions it carries out rather than changes: ADR 0003 "Core holds every rule that decides what is spoken" and D14 "a reader judges nothing but NVDA's local drops ...". It changes the chain Core compares, so NVDA's focus difference level could fall at a different entry: the difference shows only when two chains differ only in an entry the reader drops today. Sequence (B's): in File Explorer, Tab from the address bar into the items list, with an unnamed UIA pane between the window and the list; today Core receives the window and the list, NVDA's line holds the window, the pane and the list. Speech today and under the change: needs a capture (no capture of this sequence was found). Saves about 100 product and 120 test lines, and the MSAA and UIA chains would be filtered by one rule. Note on NVDA: NVDA's line holds every parent (api.py 104 to 147), and the rule is applied at speech time (eventHandler.py 418, NVDAObjects/__init__.py 1362).

### Q2: which operations keep a classic twin

Every rops operation keeps a classic twin for the reasons in section 2: the listener (focus_ancestry only), client-side proxies, failed runs and the instruction limit, size limits answered classically, the developer setting, and plain windows above a focus. Removing a twin removes reading for that case, so it is a feature question. Should the twins be kept only where a design requires one (the listener's focus ancestry, and the size-limit paths), with a failed program answered as a failed read? About 1,100 product lines, mostly in caret.rs and text.rs outside this area, and the comparison tests with them. Speech: a read that falls back today would fail instead; needs the fallback rate first, which nothing in the repository records (Path::Fallback is logged at warn, outpost read.rs 2108 and completion.rs 449).

### Q3: sync markers in product processes

Every product WinEvent hook set installs a sync-marker hook and a hidden window (event_thread.rs 359, install_with_sync_marker), used only by `Nudger::sync` (event_thread.rs 255), whose callers are `Outpost::settle` (outpost/mod.rs 968), which only tests call, and the listener's tests (listener.rs 4504 onward, inside its `#[cfg(test)]` module from 2058). Move it behind the faults feature? About 150 product lines; no speech change.

### Q4: NVDA's caret and cursor roles

NVDA maps `oleacc.ROLE_SYSTEM_CARET: controlTypes.Role.CARET` and `oleacc.ROLE_SYSTEM_CURSOR: controlTypes.Role.CURSOR` (IAccessibleHandler/__init__.py 122 and 125); verbatim-model's Role has neither, so ia2 reads both as Unknown. Adding them changes what such an object is called: needs a capture. Rarely met.

## 5. Cost of one behaviour

From git show --stat, each checked:
- 1b43f866, a missing IsEnabled read as enabled: 12 files, 200 insertions and 71 deletions: shape/mod.rs, shapes.rs, shape/tests.rs (82 lines), outpost read.rs, mockapp call_counts.rs and instruction_limit.rs, performance.md (28 lines of the same figures), three crate guides' worth of documents (verbatim-uia.md, verbatim-uia-rops.md, uia-shapes.md), test-coverage.md, flaky-tests.md.
- f2ff96d8, a suggestions change's automation id: 8 files, 57 and 18: shapes.rs (3 lines), the outpost's worker.rs, mod.rs and outpost-logic entry.rs, call_counts.rs, performance.md, two crate guides.
- 87e2dc30, a focus's states and its dialogs' children in one program: 11 files, 491 and 70: a 216-line rops module (program, classic twin, dispatcher), lib.rs, outpost read.rs, completion.rs, dialog_text.rs, asked.rs, worker.rs, instruction_limit.rs, slow_application.rs, performance.md, the rops guide.
- b85340df, a menu item's legacy checked state: 7 files, 173 and 25: map.rs, the outpost's worker.rs, mockapp src/uia.rs (91 lines of stand-in), focus_reports.rs, three documents.

So a new UIA read touches a shape and its recording test, a call-count figure written in a test and again in performance.md, and the guides; a new multi-call read adds a program, a classic twin and a dispatcher with its own fallback rule and error classifier, and an instruction-limit figure written twice. The shared mechanisms the reviewers propose, merged: one dispatch rule and one classifier (F4), so a new operation writes no dispatcher; one classic implementation per read (F2), so the outpost has no second classic path; one parts constructor (F11), so a new shape field reaches every reader once; one MSAA read (F8), so an MSAA rule is written once; performance.md pointing at instruction_limit.rs and call_counts.rs rather than repeating their figures (F22); and, if Dickson rules, twins kept only where a design needs one (Q2), which halves a new rops module.

## 6. Dropped

- The 33 dead builder methods: get_operation_status and set_operation_status are emitted by try_catch in every program with a try block (builder.rs 1409 and 1411; 13 callers of try_catch); 31 remain (F1).
- "The MSAA reader does not filter" (B): the outpost drops every MSAA window object at read.rs 1130; corrected in F5, not dropped whole.
- B's fix for F6, comparing by runtime id: the kept element is looked up by that runtime id, so the comparison would always succeed and disable the stale check.
- The notification-processing default (both: map.rs 472 to 486 maps an undocumented value to ImportantAll): Core treats only MostRecent and ImportantMostRecent specially (announcements/notification.rs 75), and NVDA's handler cancels only for ImportantMostRecent and MostRecent (NVDAObjects/UIA/__init__.py 2546 to 2549), so an unknown value is spoken as NVDA speaks it; no defect.
- A's `unwrap_or(i32::MAX)` conversions (rops focus.rs 637, 669, 931): conversions of small counts and masks with no reachable effect; not worth a change.
- A's divergence between the remote and classic previous-focus answers: the quotations are right, but no product query sets `previous`; replaced by F3.
- A's size table (verbatim-uia 9,006 product and 1,248 test lines): recounted as 7,601 and 2,653.
- A's client.rs line numbers for the ancestor walks (307 to 411, 421 to 474, 576 to 600, 679 to 737): the code is at 676, 790, 822, 945 and 1048 to 1106.
- B's "four fallback policies": five, the terminal operations' any-error rule being the fifth.
