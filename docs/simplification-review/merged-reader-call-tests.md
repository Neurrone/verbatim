# Merged review: the call-count, remote-operation and tree tests

Checked against main at ba89dca7 on 2026-11-20. Reviewer A's notes are "A", reviewer B's "B". Every quotation and figure below was checked in the repository today, by the command named. Terms:

- A "pin" is an exact expected figure in a test.
- "Provider hits" are mockapp's own counts of the calls its provider answered, by method (`crates/mockapp/src/hits.rs`), such as `("GetPropertyValue", 117)`. "Client calls" are `CallCounts`, the calls Verbatim's own process made across the process boundary, such as `connected(7, 0, 0)`.
- A "dispatcher" is a `verbatim-uia-rops` entry point that runs the remote program and falls back to the classic calls.

## 1. Size

| Part | Verbatim lines | NVDA counterpart | NVDA lines | Ratio |
|---|---|---|---|---|
| `crates/verbatim-uia-rops/src`, product | 12,179 (13,853 with 1,674 lines of inline unit tests, counted from each file's `#[cfg(test)]`) | `nvda/source/UIAHandler/_remoteOps/` (4,321) and `nvda/source/UIAHandler/remote.py` (369) | 4,690 | 2.6x |
| Focus, navigation, path and states programs, remote and classic (`focus.rs` 1,200, `focus_states.rs` 223, `navigate.rs` 181, `path.rs` 185) | 1,789 | `nvda/source/api.py` 80 to 196 (`setFocusObject`) and `nvda/source/NVDAObjects/UIA/__init__.py` 2136 to 2210 (parent, previous, next, first and last child), all classic | about 192 | about 9x |
| Tests: `call_counts.rs` 5,750, `remote_ops.rs` 1,719, `instruction_limit.rs` 1,202, `uia_tree.rs` 767, `msaa_tree.rs` 599, `uia_bugs/` 1,391 | 11,428 | `nvda/tests/unit/test_UIARemoteOps/`, run on NVDA's local emulator (`_remoteOps/localExecute.py` line 68, `maxInstructions: int = 10000`) | 1,002 | 11.4x, not like for like |
| Shared harness: `tests/common/` 4,663, `src/hits.rs` 763 | 5,426 | none | 0 | |

The reviewers' sizes agree with `wc -l`. A's 3.0x counted the inline tests as product; B's 1.3x compared only the framework files. The settled product ratio is 2.6x.

What the extra lines buy:

- NVDA runs remote operations only for Microsoft Word. `UIARemote` is used only in `nvda/source/NVDAObjects/UIA/wordDocument.py` (624 to 660, 873 to 920, each behind `UIARemote.isSupported()`), with `UIAHandler/__init__.py` 570 and 590 starting and stopping it. Verbatim runs programs for the focus, navigation, the tree dump, text, the caret and terminals. CLAUDE.md asks for this: "Use UIA remote operations wherever a sequence of UIA calls would benefit".
- Each operation is written twice, as a program and as a classic walk. The classic walk has three users: a failed run (`Path::Fallback`; performance.md 2170 to 2172, "programs are not made resumable (Dickson, 2026-10-07)"), the listener, which always reads classically (`crates/verbatim-listener/src/listener/asked.rs` 814), and the developer setting `uia.remote_operations` (see Q3).
- NVDA has no call-count test, no instruction-count test and no test that the remote and classic reads agree. The call-count tests serve CLAUDE.md's "a command's own calls are pinned by a call-count test, not written in the ledger (Dickson, 2026-11-04)". The instruction tests serve the memory of the owner's instruction: "Dickson does not want any program to exceed 10,000".
- No ruling asks for the provider-hit pins, the exact instruction figures at typical sizes, or the figures copied into prose. Claude added the hits in 555f5ec5 and fc680516 (2026-10-06), with no ruling named.

Deletion target:

| Part | Product lines | Test lines | Document lines |
|---|---|---|---|
| Remove now | about 165: dispatcher copies 50, the "gone" checks 15, `FocusQuery::previous` and the program's `refresh` path 100 | about 1,160: provider-hit tuples and their wrappers 700 to 760, `uia_event_caches_cost_exactly` 110, `the_listener_makes_no_connection` 43, figure and history comments about 100, the `previous`/`refresh` test parts 60, `identity_of` about 40, the `uia_tree` twin 30, the duplicate description step 15, a misplaced comment 2 | about 200 of performance.md; one uia-facts claim, about 6 |
| If Dickson rules (Q1) | 0 | instruction tests asserted as bounds: A estimates about 400; not recounted | 0 |
| Planned phase (M6.4) | about 30: the three terminal dispatchers | 0 | 0 |

B's table shape (Q2) is the other way to the same lines, and is the session's choice. It keeps every pin and cuts about 1,000 lines of checks. It is not additive with dropping the hits.

## 2. Findings

| id | title | class | found by | file and line | what goes | product saved | test saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| F1 | Provider hits pinned exactly, 33 figure moves and no catch in 92 commits | Remove now (the session's decision) | both (remedies differ) | `crates/mockapp/tests/call_counts.rs`, 624 one-line hit tuples | the per-method hit lists; keep client calls, failed reads, about 10 targeted provider-side checks | 0 | 700 to 760 | no |
| F2 | performance.md copies the pinned figures; four have drifted | Remove now | both | `books/dev/src/reference/performance.md` 264 to 270, 303 to 312, 502 to 509, 728 to 730, 2164 to 2226 | "Today:" figures and the instruction list, replaced by the test's name | 0 | 0 (about 200 document lines) | no |
| F3 | `FocusQuery::previous` and the program's `refresh` path are reached only from tests | Remove now | B only | `crates/verbatim-uia-rops/src/focus.rs` 302 to 317, 504 to 512, 525 to 536, 560, 766 to 778, 1006 to 1013, 1031 to 1033 | `previous`, `Progress::PreviousFocused`, `Ancestry::previous_focused`, the remote refresh branch | about 100 | about 60 | no |
| F4 | Ten hand copies of the remote-or-classic rule; three omit the timeout guard | Remove now (seven); Planned phase M6.4 (three terminal) | both (A ten, B six) | `text.rs` 299 to 313 (`choose`); copies listed below | one dispatcher; one "gone" check | about 65 now, about 30 in M6.4 | 0 | no |
| F5 | Exact instruction counts at typical sizes, where the rule is a bound | Question for Dickson (Q1) | A only | `crates/mockapp/tests/instruction_limit.rs` 330 to 334 and others | typical-size runs; exact worst-case figures if ruled | 0 | about 400 (A's estimate) | no |
| F6 | `uia_event_caches_cost_exactly` adds only UI Automation's fixed overhead to the shape test | Remove now | A only | `call_counts.rs` 3312 to 3420 | the whole test | 0 | 110 | no |
| F7 | `the_listener_makes_no_connection` duplicates the focus ratchet | Remove now | B only | `call_counts.rs` 1848 to 1890 | the whole test | 0 | 43 | no |
| F8 | Figures and history in `call_counts.rs` and `instruction_limit.rs` comments | Remove now | both | `call_counts.rs` 1640 to 1645, 1806 to 1816, 1657; `instruction_limit.rs` 514, 553 to 556 | figures and "before" sentences | 0 | about 100 | no |
| F9 | `identity_of` rebuilds Core's identity, with silent defaults | Remove now | A only | `call_counts.rs` 887 to 915 | the helper; drive through `CoreDriver` | 0 | about 40 | no |
| F10 | Two tree-dump tests are one test written twice | Remove now | B only | `crates/mockapp/tests/uia_tree.rs` 565 to 640 | one copy, a shared helper | 0 | about 30 | no |
| F11 | Description-change speech pinned twice | Remove now | A only | `call_counts.rs` 581 to 590; `crates/mockapp/tests/msaa_events.rs` 121 to 133 | the first step of the `msaa_events` test | 0 | about 15 | no |
| F12 | Smaller items | Remove now | as marked | below | | 0 | about 10 | no |

### F1. Provider hits pinned exactly

Both reviewers found the same fact. A proposes dropping the hits; B proposes moving every figure into a table (Q2).

- Verified: `call_counts.rs` has 624 one-line `("Method", n),` lines (`grep -cE '^\s*\("[A-Za-z_:]+", *[0-9]+\),?\s*$'`), 132 literal client-call pins (`calls(`, `connected(`, `with_connection(`, `first_focus(`), and 80 check sites. A example, `a_cells_table_costs_exactly`: `("ProviderOptions", 29), ("GetPatternProvider", 22), ("GetPropertyValue", 50), ("HostRawElementProvider", 11)`.

The churn, recounted:

- `git log --since="2026-10-20 00:00:00" -- crates/mockapp/tests/call_counts.rs` gives **92 commits**, +4,908 and -3,231 lines, not 89. Both reviewers ran `--since=2026-10-20`. Git fills a bare date with the current time of day, so it left out the three earliest commits of 2026-10-20: e98cfd90, 8c235918 and 6a6b7c5b.
- Of the changed lines, 1,199 are hit tuples and 320 are client-call pins. 53 of the 92 commits also edited performance.md.
- For `instruction_limit.rs`: 42 commits, +701 and -216 lines, 30 of them also editing performance.md.

Every commit, classified from its message and its `call_counts.rs` diff (`git show -U0`):

- **Figures moved by an intended change, 33:** 1b43f866 f2ff96d8 e380a0ce aa751cda 34b5bbbf 9a8bf32e 6612507a 36863d2e 7976fb10 eceffc1f ce3a42c7 de45dfa0 1588a0b6 694e225d 7a6c340b 92fa12ab 07544041 fef26576 715313ff 8826d457 c7c7c6dc 22256896 094a8dc8 630d6a6e 7015ae08 ddc70eee 83cde1c5 d45da1f9 0c678ebf 505e3ad8 5223d5f7 d2d2a24e 2e0945b9. ce3a42c7 and 07544041 are follow-up commits that pin a change made in an earlier commit. eceffc1f and de45dfa0 are ports onto Core that moved figures with the new drive.
- **New pins, 24:** f3218d20 ccdb2a7a 9aba90be a22fa6a5 f4d44217 3f2d2d25 bc6c193c 8597cba5 50b1d527 1b45eb3e 51616d28 38cab859 4af7fe37 719b4ed4 f9ab3fd5 c04584db e6e42ca1 6a644058 88541265 cff2a739 c60bacb1 14a1e716 8c235918 e98cfd90.
- **Flaky-test fixes, 3:** 437d7381 (warm the runtime id before measuring), 93fb5831 (combo box count moved with load), and f2f8209a (freeze the tests' clock, with a product race fix in the same commit).
- **Unintended call caught: none.** No commit message says a pin found a call nobody meant to add. I searched the bodies of all 92 for caught, regress, unintended, unexpected, extra call, stray and spurious.
- **The rest, 32, change no figure:** harness changes, ports, renames and comments. These are bc99bc40 55d472b5 2ec50695 a50b9ace 6a3aa0e4 8b1944d0 550452aa 9fd731a4 243c3344 a9c7bf56 b7932f07 115d825f b94fa1d8 c39da5ff d13eaef2 a7c954e9 d6acaaf9 010f290a 8efb3da1 6b53961b a4117213 e94c3a09 3abda293 a9393c04 1efdbc93 d73dd00b 1ad45f16 8416923b 2b13487b dd363ecc f05cdc6b 6a6b7c5b. 2ec50695 changed a speech expectation only, and b94fa1d8 removed a test with its mechanism.

The nearest thing to a catch:

- 92fa12ab raised the classic caret-key read from 15 to 17 and 18 calls. 694e225d ("drop two calls"), 7a6c340b and 1588a0b6 brought it back to 15. All four were committed within one minute on 2026-11-09, as one build. The pins made the extra calls visible, but no message says the pins found them.
- 8416923b and 8c235918 record mutation checks of new pins ("fails with the branch removed").
- e98cfd90's new counters corrected a wrong design claim from 946e2149. That was a wrong claim, not an added call.

What else catches the defects the hits could catch:

- The property count per shape is pinned in-process by `a_shapes_properties_are_its_fields_union_without_repeats` (`crates/verbatim-uia/src/shape/tests.rs` 143 to 183).
- A failed cached read has its own pinned count.
- The boundary, title-bar and probe tests assert specific provider-side counts and stay.

The change: drop the per-method lists from `Ratchet::check`, and keep the client calls, the failed reads and the targeted checks. No ruling names the hits, so this is the session's decision, reported to Dickson. performance.md 19 to 22, which says the test pins "the provider calls mockapp answered by method", changes with it.

### F2. performance.md copies the pinned figures, and they have drifted

The rule, CLAUDE.md: "Record a cost in `books/dev/src/reference/performance.md` only when it is on the event or speech path or was measured to be slow; a command's own calls are pinned by a call-count test, not written in the ledger (Dickson, 2026-11-04)".

The ledger states the rule and then breaks it. Line 15 says "(`crates/mockapp/tests/call_counts.rs`), not written here", while lines 28 to 29 say "a change that removes one lowers the number in the test and here in the same commit". The test's failure message (`call_counts.rs` 187 to 190) says the same: "an operation's cost moved; when that is deliberate, update this test and books/dev/src/reference/performance.md together."

The drifted figures, ledger beside test. The hit sums were added from the test's tuples.

| Ledger line | Ledger says | Test asserts |
|---|---|---|
| 264 to 270, a repeated focus | "read in full, 5 UIA calls (`uia_repeated_focus_costs_exactly`) ... They cost mockapp 149 provider calls" | `call_counts.rs` 1832: `calls(3, 0, 0)`, hits 1+14+8+6+5+3 = 37. The test's comment, 1814 to 1816: "Before Core judged the repeat from the check's answers, the listener read the repeat in full, two calls more" |
| 303 to 305, a UIA focus in the steady state | "Today: 4 UIA calls, all the listener's, either way the outpost walks; the minimum, met. 312 provider calls in all with the outpost's remote walk and 306 with its classic walk" | 1688: `connected(7, 0, 0)`, 371 hits remotely; 1757: `connected(11, 0, 0)`, 353 hits classically |
| 502 to 507, a UIA focus cold | "Today: the minimum, 6 UIA calls and 1 window message ... 354 provider calls with the outpost's remote walk and 345 with its classic walk" | 1673: `connected(9, 0, 1)`, 413 hits; 1742: `connected(12, 0, 1)`, 392 hits |
| 728 to 730, arrowing through a list | "Minimum and today: 4 UIA calls ... 327 provider calls with the outpost's remote walk and 324 with its classic walk" | `next_item`, 1718: `connected(7, 0, 0)`, 387 hits; 1787: `connected(12, 0, 0)`, 372 hits |
| 2209, the caret read | "so 64 come to about 8,000 (80 percent)" | Agrees with the test's figures: (4,144 - 2,152) / 16 runs, carried to 64 runs, is about 8,000. But `uia-facts/a-remote-operation-stops-after-exactly-10000-instructions/readme.md` line 9 says "the caret read reaching about 7,700", and also "every program is sized under half the limit", which the caret read is not. |

A found the first two; B found the first three and the uia-facts copy; I found the fourth. For the steady and cold entries, the ledger's 4 and 6 are still the listener's share (the test's comment, 1625 to 1666). The ledger leaves out the outpost's completion of three calls and one connection, which Core now asks for. The entry above them (698 to 701, into a list) already names the test and is current: aa751cda did it that way.

How many lines copy a pinned figure (`awk` over the file):

- 23 "Today:" figures, in 22 bullets of 152 lines, 65 of which hold a figure.
- 38 lines with figures in "The instruction limit" (2164 to 2226), which restate every `instruction_limit.rs` pin: 3,078, 2,243, 1,278, 967, 235; 3,109, 1,219, 233, 218; 255, 287, 291; 4,453, 91; 2,152, 4,144; 683, 704; 159; 50, 90; 172, 22; 1,349; 4,151 and 4,099; 4,635, 876, 3,083, 719, 645, 413.
- 10 "N provider calls" lines, partly inside the bullets above.
- So about 103 lines carry a pinned figure, inside about 200 lines that would shrink to a test's name with each entry's minimum, target, bound and reasoning kept.

The change: replace each "Today:" figure and the instruction list with the pinning test's name, as 698 to 701 does. Drop "and here" (line 28) and "and books/dev/src/reference/performance.md" from the failure message. Correct the uia-facts readme's line 9 to point at the test. Speech does not change.

### F3. `FocusQuery::previous` and the program's `refresh` path are reached only from tests (B only, confirmed)

- `grep -rn "FocusQuery {" crates` outside tests finds two product constructions: the listener (`crates/verbatim-listener/src/listener/asked.rs` 797) and the outpost (`crates/verbatim-outpost/src/outpost/read.rs` 895). Both set `previous: None`.
- `previous: Some` appears only in `instruction_limit.rs` 274 and 357.
- The listener sets `refresh: true` and always calls `focus_ancestry_classic(uia, &query)` (814). The outpost sets `refresh: false`. So the program's refresh branch (`focus.rs` 525 `let reads_alone = query.refresh || wants_selected_child(&input);`, 532, 560, 774) runs only in `instruction_limit.rs` 362 and `remote_ops.rs` 346.
- The classic refresh (1036) is live and stays.
- The listener ignores the progress the code reports: `Progress::PreviousFocused(_) => {}` (`asked.rs` 561).
- Dead with `previous`: the dispatcher's retry (`focus.rs` 302 to 317, "if query.previous.is_some() && is_element_gone(&error) {"), the program's read (504 to 512) and decode (766 to 772), and the classic read (1006 to 1013, 1031 to 1033).
- The pinned focus-ancestry worst case, 3,078 (`instruction_limit.rs` 274), includes the `previous` read. It measures a query no product code sends, so it must be measured again after the change.

B also proposes two query types, one per caller, because a remote run accepts and ignores `plain`, `before_call` and `progress`. That is the session's design choice. Speech does not change, and no planned phase rewrites this code.

### F4. Ten copies of the dispatcher, three without the timeout guard

The rule is written once, privately, as `choose` in `crates/verbatim-uia-rops/src/text.rs` 299 to 313 (`Err(error) if gone_or_timed_out(&error) => Err(error),`). `grep -n "if !remote {"` finds 12 dispatchers.

- Two call `choose` (`text.rs` 324 and 376).
- Five are identical hand copies: `caret.rs` 584, `controlled.rs` 74, `navigate.rs` 72, `path.rs` 59 and `watch.rs` 124.
- Three are deliberate exceptions: `focus.rs` 290 (the retry, gone with F3), `focus_states.rs` 73 (falls back on a gone element, documented at 58 to 61), and `text.rs` 342 (`text_range`, the read after a selection).
- Three have no guard: `terminal_rows.rs` 70 to 79, `terminal_snapshot.rs` 213 to 222 and `terminal_find.rs` 102 to 109, each `Err(error) => terminal_rows_classic(uia, query).map(...)`.
- `tree.rs` 73 to 92 has its own per-run loop, with its own `is_timeout` (129).

A's count of ten copies is right; B's six were the copies in its scope.

The crate guide states the rule for every entry point (`books/dev/src/reference/crates/verbatim-uia-rops.md` 719 to 722): "A gone provider and one that times out (`UIA_E_ELEMENTNOTAVAILABLE`, `UIA_E_TIMEOUT`) are returned by every entry point without the classic calls, which would fail the same way after a second wait". The three terminal dispatchers break it.

The sequence: the console host's window thread is stalled; Core asks for the rows; `terminal_rows_remote` returns `UIA_E_TIMEOUT` after one timeout; `terminal_rows_classic` then calls the same thread, which waits again; the answer comes at least twice the timeout late.

"Gone" is written four times in this crate: `focus.rs` 325, `focus_states.rs` 86, `tree.rs` 137, and inside `gone_or_timed_out` (`caret.rs` 596). `verbatim_uia::element_is_gone` (`crates/verbatim-uia/src/com.rs` 40) and `verbatim_uia::timed_out` (58) exist too, and `focus.rs` 381 and `navigate.rs` 172 use them. The crate's check is narrower: `element_is_gone` also counts `RPC_E_DISCONNECTED` and `RPC_E_SERVER_DIED`.

The change: make `choose` the crate's one dispatcher, taking `&Uia`, and keep the named exceptions at their call sites. M6.4, the terminal reading rebuild, rewrites the terminal reads (`docs/design/terminal-rebuild.md` 35 and 70), so the three terminal copies and their missing guard belong to M6.4's design. Speech does not change; only an answer after a terminal timeout comes sooner.

### F5. Exact instruction counts at typical sizes (A only; a question, Q1)

- `instruction_limit.rs` 330 to 334: `[worst, limited, typical, far, near_step], [3078, 2243, 1278, 967, 235]`, then `assert!(worst < LIMIT / 2 && far < LIMIT / 2);`.
- Its doc comment, lines 11 to 13: "every worst case but the caret read's must stay under half the limit, the margin the design asks for".
- In 1b43f866, a change of about ten instructions per element moved 26 lines of this file (`git show --stat`).
- `call_counts.rs` pins two more instruction counts in `caret_tell_reads_cost_exactly`.

Bounds in place of exact figures break testing.md line 40, "Counts are exact numbers, not bounds", so this goes to Dickson. Dropping the typical-size runs and keeping exact worst-case figures changes no rule, and is the session's.

### F6. `uia_event_caches_cost_exactly` (A only, confirmed with a qualification)

- `call_counts.rs` 3312 to 3420 refreshes four event cache requests, each `calls(1, 0, 0)`.
- The property figures equal the in-process shape pins (`shape/tests.rs` 145 to 148: `TextEvent` 3, `PropertyEvent` 25 = 13 `GetPropertyValue` + 12 `GetPatternProvider`, `ListenerEvent` 15).
- The rest is 5 `ProviderOptions`, 3 `HostRawElementProvider`, 2 `GetRuntimeId` and 3 `FragmentRoot`, the same in all four.
- What the test adds is only that UI Automation asks the provider once per property, which is UI Automation's behaviour, not Verbatim's.
- With F1 it pins only `calls(1, 0, 0)`, one `BuildUpdatedCache` by construction. Delete it with F1.

### F7. `the_listener_makes_no_connection` (B only, confirmed)

- `call_counts.rs` 1848 to 1890 focuses "First", "Second", "Options" and "One" on `counts.json`, and asserts `listener_calls.connections, 0`.
- The listener's read goes through `verbatim_listener::read_object` (`tests/common/outpost.rs` 1005 to 1022).
- `uia_focus_changes` drives the same fixture and focuses through `CoreDriver`, which calls the same `read_object` and adds its calls into the step (`tests/common/core_driver.rs` 1160 to 1171).
- Its pins are `connected(..)`, one connection (the outpost's lookup). A listener connection makes it two, so `uia_focus_changes_cost_exactly_remote` and `_classic` fail on the same defect.
- The test with its doc comment is 43 lines (1848 to 1890), not 75. Keep the design citation in the focus test's comment.

### F8. Figures and history in the test files' comments (both)

- `call_counts.rs` has 633 doc-comment and 325 line-comment lines (both counts agree).
- Prose figures: for example 1640 to 1645, "1 `WM_GETOBJECT`, 14 `ProviderOptions`, 7 `GetPropertyValue`, 5 `HostRawElementProvider`, 5 `Navigate` and 2 `FragmentRoot`".
- History: 1814 "Before Core judged the repeat", 1657 "Before Core asked for the completion", 1857 "as the listener's `mark_window` once did".
- `instruction_limit.rs` 514: "4425 and 84 before the caret read began reading the selection's texts", and 553 to 556.
- These break CLAUDE.md's "Documents describe the present". A's 300 lines counts whole comments that would be rewritten. Lines that hold only a figure or a "before" are nearer B's 100, which is the figure used here.

### F9. `identity_of` (A only, confirmed)

`call_counts.rs` 887 to 915 is "built from its focus node's values as Core builds it from its snapshot", with `_ => StateSet::default()` and `_ => Role::Unknown`. Core builds the identity from its snapshot (`crates/verbatim-core/src/focus/validity.rs` 395 to 404). Two tests use the helper (945, 1143). If Core's identity changes, these tests keep passing on the copy. Drive them through `CoreDriver` instead.

### F10. The two tree-dump tests (B only, confirmed)

`diff` of `uia_tree.rs` 565 to 602 against 603 to 640 differs only in the title, `tree_dump(&uia, &query, false|true)`, and the expected `"classic", 3` against `"remote", 1`. One helper taking the path and the expected count, as `dump_with_an_element_gone_during_it(remote)` in the same file already does, saves about 30 lines.

### F11. The description change pinned twice (A only, confirmed)

`call_counts.rs` 581 to 590 and `msaa_events.rs` 121 to 133 both raise `set-description item2 Second choice` with `WinEventKind::DescriptionChange` on `counts.json` under MSAA through `CoreDriver`, and assert `[vec![SegmentContent::Description("Second choice".to_owned())]]`. The `msaa_events` test's next step (135 to 146, the group's state change unspoken) does not depend on the first. Delete that first step, about 15 lines, not 30.

### F12. Smaller items

- **Serial run owed a recheck (both).** `docs/flaky-tests.md` 182 to 185: "the entry is deleted once the tests pass under load. Until then `cargo xtask ci` runs `call_counts` alone." The fix it waited for is b94fa1d8 (2026-11-08). No later commit records that run. One load run settles it: delete the entry and the serial filter (`xtask/src/main.rs` 551), or record the failure.
- **One claim checked twice (A only, confirmed).** `uia-facts/a-registration-that-times-out-reports-success/src/main.rs` 320 to 324, "E-TT's registration, its provider held in FragmentRoot, reports success". The `ett` case registers "a focus handler group" (87), the quantity `uia_bugs`' `a_group_registered_while_its_provider_is_held_reports_success` asserts. `uia-facts/readme.md` 34 says the test and the program "never check the same quantity". Drop the program's success claim and keep its timing claim. B saw the same overlap and kept it.
- **Registration pins that cannot fail (B only, confirmed).** `uia_event_registrations_cost_exactly` pins `calls(0, 0, 0)` for registrations. `crates/verbatim-uia/src/calls.rs` 18 to 19 says event subscriptions are not counted. Only the hits pin anything there, so F1 decides this test's worth.
- **Call counts outside `call_counts.rs` (B only, confirmed).** `focus_reports.rs` (21 matches for `CallCounts`, `calls(` or `connections`), `terminal.rs` 294, `terminal_reading.rs` 439 and `uia_tree.rs`' dump tests. performance.md 19 says `call_counts.rs` "pins every operation's calls"; correct it with F2.
- **Misplaced comment (A only, confirmed).** `remote_ops.rs` 1469 to 1470: `main`'s doc comment sits above `focus_now_with_a_slow_grandparent` (1477), and `main` (1535 to 1536) has none. 2 lines.

## 3. Test findings, per crate

**mockapp**

- Removable, whole tests:
  - `uia_event_caches_cost_exactly` (110 lines);
  - `the_listener_makes_no_connection` (43);
  - one of `a_classic_dump_reads_each_elements_children_in_one_call` and `a_remote_dump_reads_a_small_tree_in_one_call` (about 30).
- Removable, parts of tests:
  - the first step of `a_description_change_on_the_focus_is_spoken` (about 15);
  - the hit tuples across `call_counts.rs` (700 to 760);
  - figure and history comments (about 100);
  - `previous` and `refresh` in `focus_and_navigation_execute_exactly`, `reads_and_paths_execute_exactly`, `a_remotely_read_element_without_validity_reads_valid_with_no_failed_read` and `both_agree` (about 60);
  - the misplaced comment (2).
- Replaceable:
  - `identity_of` and the two MSAA identity-check tests' use of it, by `CoreDriver` (about 40 saved).
  - If Q1 rules for it: typical-size instruction runs, by worst-case bounds (about 400, A's estimate).
  - Alternatively Q2's table, which keeps every figure and replaces about 1,240 lines of checks with about 205.
- To add, if F3 lands and the classic walks stay: one hand-written test comparing `element_by_path` and `controlled_selection` remote against classic. No test compares them directly (B; `grep` finds no direct classic call in tests). About 40 lines.
- Owed: one load run of `call_counts`.
- Keep: every failure-path test in `remote_ops.rs` (stall, timeout, gone provider, failed parent step), `the_instruction_limit_is_measured`, the boundary, probe and title-bar tests, `uia_bugs/`, and `msaa_slow_acc_parent_answers_the_parent_late` (see Dropped).

**uia-facts**: one claim (about 6 lines).

**verbatim-uia-rops and verbatim-uia**: nothing removable. F4 needs no new test.

## 4. Questions for Dickson

**Q1. Instruction counts: exact figures or a bound.**

- The rules that bear on it:
  - the memory of the owner's instruction (worst-case-performance): "For UIA remote operations, run the worst case and count the instructions the remote program executed; Dickson does not want any program to exceed 10,000";
  - testing.md line 40: "Counts are exact numbers, not bounds";
  - `instruction_limit.rs` 11 to 13: "every worst case but the caret read's must stay under half the limit".
- The options:
  - **Exact, as today.** It fails on any change of any program at any size. 1b43f866 moved 26 lines of figures for a fix to one property's default.
  - **Exact worst cases only (the session can do this alone).** It drops the typical-size runs, fails on any change to a worst case, and cuts some churn.
  - **A bound at each worst case (A).** `< LIMIT / 2`, and `< LIMIT` for the caret read. It fails only when a program crosses the bound. It would have accepted 1b43f866's rise of the focus worst case from 2,446 to 3,078, 26 percent. It breaks testing.md's line 40, so it needs the owner's ruling.
- Speech does not change under any option.

**Q2. The ratchet's shape. B's table is the session's to choose; dropping the hits (F1) is the session's too; reported here because the two exclude each other.**

The rules that bear on it:

- CLAUDE.md: "a command's own calls are pinned by a call-count test, not written in the ledger (Dickson, 2026-11-04)".
- CLAUDE.md: "Use UIA remote operations wherever a sequence of UIA calls would benefit, and audit this whenever UIA code changes".
- testing.md 40: "Counts are exact numbers, not bounds".
- testing.md 41: "A test's expectation is fixed before it runs".

The two shapes:

- **B's shape.** Keep the drivers (about 3,400 lines). Hold each measured cost as one line of a checked-in table, `name | uia msaa messages connections | failed reads | method=n ...`, compared whole, with an environment variable that rewrites the table for a deliberate change.
  - About 4,300 lines would be left in `call_counts.rs`.
  - It can fail on everything today's tests fail on: any client call, failed read, provider hit or instruction count.
  - It cannot fail on anything more.
  - It changes neither the number of figures a change moves nor the review of them. It makes each move a regenerated table and a diff.
  - Every count stays exact, and in a normal run the expectation is fixed before the run, so it keeps both testing.md rules. The rewrite mode must never run inside `cargo xtask ci`.
- **A's shape.** Keep the client calls and failed reads exact, drop the 624 hit lines for about ten targeted provider-side checks, and assert each program's worst case under the bound.
  - About 2,400 lines would be left across `call_counts.rs` and `instruction_limit.rs`.
  - It can fail on any added or removed client call, any failed cached read, a targeted provider-side count (the boundary's `WM_GETOBJECT`s, the title bar, the probe), and a program crossing its bound.
  - It cannot fail on more provider work behind the same client calls, which the shape tests partly cover in-process; nor on a program growing under its bound; nor on a typical-size change.
  - Its hit half is the session's. Its bound half is Q1.

The fact that decides whether exact provider hits earn their cost: in 92 commits, none records a hit or a client-call pin catching an unintended call. 33 moved figures for intended changes, and 1,199 of the changed lines were hit tuples.

**Q3. The classic walks, the agreement tests and R3.**

Nothing here is proposed for removal. These are the facts asked for.

- **The setting.** `crates/verbatim-config/src/lib.rs` 161 to 166: "Whether outposts read a UIA focus's ancestors with a remote operation ... On by default; off forces the classic walk, one round trip per ancestor, for diagnosing a provider and for before-and-after measurements."
  - `books/dev/src/reference/crates/verbatim-config.md` 68 to 73 calls it "developer settings for the UIA client".
  - The rops guide 723 to 725 says "A developer setting (`uia.remote_operations` in `settings.toml`, on by default) forces the classic walk".
  - `books/dev/src/how-to/inspect.md` 96 to 101 uses it to compare reads.
  - The user guide does not mention it (`grep -rn remote_operations books/user` finds nothing).
  - So it is a developer setting, not a user setting as B called it.
  - Its documents say it governs the focus's ancestors. In code it gates every UIA program the outposts run, through `tries_remote` (`crates/verbatim-outpost/src/outpost/mod.rs` 576, 13 uses: focus, completion, states, navigation, text, caret and terminal reads). That is a small documentation drift.
- **The fallback.** performance.md 2170 to 2172: "A run that exceeds it is answered classically for that call (`Path::Fallback`), at hundreds of calls; programs are not made resumable (Dickson, 2026-10-07)."
  - A cited architecture.md 744 to 752 for the fallback. That passage says the crate is "Wrapped ... and used for" the reads and does not mention a fallback, so A's citation is corrected to performance.md.
- **R3.** `books/dev/src/explanation/architecture.md` 1201 to 1205: "R3 — Remote-ops on ARM64. API limits or behavior differences. Mitigation: verify on ARM64 before relying on remote operations there. The verification was planned alongside the first terminal work (M4) and was deferred on 2026-10-06: M4 is built and verified on x64 only, and the ARM64 verification is not yet scheduled to a milestone." Line 751 repeats it.
- **What ARM64 already runs.**
  - The `ci-arm64` job (`.github/workflows/ci.yml`, `runs-on: windows-11-arm`) runs `cargo xtask ci`, which includes mockapp's test binaries.
  - In run 44565656341 (2026-11-19), job 135706650297 passed all 15 `instruction_limit` tests on ARM64 with the same exact figures, `the_instruction_limit_is_measured` among them. It also passed 65 `remote_ops` and `call_counts` tests with no failure, among them the agreement tests `a_deep_chain_reads_the_same_both_ways` and `a_cells_table_is_read_alike_by_the_program_and_the_classic_way`.
  - The uia-facts readme already records the limit "x64 and ARM64" (line 19).
  - So against mockapp, remote operations are verified on ARM64 on every push. Against real applications on ARM64, nothing records a verification.
- **The question.** Do the ARM64 CI runs against mockapp count as R3's verification, so that R3's entry is updated? It is a decision of record (dated 2026-10-06), and the fact that changed since is the ARM64 job's mockapp runs.
  - Either way, the classic walks stay: the listener's classic read is live, the fallback is the 2026-10-07 ruling, and the setting is a diagnostic.
  - The agreement tests stay while the classic walks do. Both reviewers agree on that.

## 5. Cost of one behaviour, and what cuts it

- **1b43f866** (one property's default, 12 files, +200/-71): the behaviour was about 45 product lines in `read.rs`. The bookkeeping was `call_counts.rs` 28, `instruction_limit.rs` 26 and `performance.md` 28 lines, plus the crate guides.
- **f2ff96d8** (one property in a cache, 8 files, +57/-18): `call_counts.rs` 8 and `performance.md` 9 lines.
- **aa751cda** (stop reading a focused list's selected item, 28 files, +235/-378): `call_counts.rs` 98, `remote_ops.rs` 73, `performance.md` 63, `msaa_tree.rs` 33 and `instruction_limit.rs` 15 lines, against `focus.rs` 56 and `read.rs` 76.
- **f4d44217**: 62 test lines for 24 product lines.

A change to one program's reads touches all of these:

- the program and its classic twin;
- the dispatcher, if it is a new operation;
- every call-count test that runs it, remote and classic;
- its instruction figures;
- the agreement test;
- the same figures in performance.md and the crate guide.

What cuts it, merged:

- performance.md names the test instead of copying it (F2; both).
- No hit tuples (F1, A), or hits held in a regenerated table (B). Either way a property added to a cache moves no hand-edited figure.
- No figures in comments (F8; both).
- One dispatcher (F4; both).
- One query type per caller, so one caller's field adds no branch to the other's program (F3; B).
- If Q1 rules for it, a worst-case bound, so a small change moves no instruction figure.

## 6. Dropped or corrected

- **Both: 89 commits since 2026-10-20.** Corrected to 92: git's bare `--since` date takes the current time of day. 52 with performance.md is 53, and 41 for `instruction_limit.rs` is 42.
- **B: "79 commits changed product code".** The split this record uses is the four-way classification in F1. 10 commits touched no product or mockapp source, which agrees with B's 10.
- **A 2.9: the classic fallback cited to architecture.md 744 to 752.** That passage does not say it. Corrected to performance.md 2170 to 2172.
- **B: "the user setting `uia.remote_operations`".** Its documents call it a developer setting for diagnosis and measurement. Corrected.
- **A 2.10: the `msaa_tree` walk read through the product walk.** Dropped. It reads mockapp's MSAA server through raw `IAccessible` calls (`msaa_tree.rs` 197 to 227), as an independent check of the stand-in against its fixture, which a product walk would not be. The `uia_tree` walk (192 to 229) already uses the product's `children` and snapshot. Its saving over `tree_dump` was not shown, so that half is dropped too.
- **B F6: `msaa_slow_acc_parent_answers_the_parent_late` "should stay only while that experiment's driver needs the command".** Dropped as a finding. No test or scenario besides it uses `slow-acc-parent` (`grep` in `crates/`), but the capture that uses it, `experiments/focus-pipeline-rework-measurements/`, is cited by `docs/parity/focus-context.md` and `docs/design/readers-tell-core-asks-build1.md`, so the test stays.
- **A: `the_listener_makes_no_connection`-sized and description-duplicate savings.** Recounted: 43, not 75, and 15, not 30.
- **Line drift corrected.** `instruction_limit.rs` 340 to 344 is now 330 to 334. A's `text.rs` 295 to 312 is 299 to 313. The repeated-focus test's comment is 1814 to 1816.
