# Terminal reading: the two reviews merged and verified

Checked against main ba89dca7, NVDA pinned at 732d1ce2f (the study's pin). "The study" is `docs/design/terminal-rebuild.md`; "M6.4" is the roadmap's "M6.4 — Terminal reading rebuild and residuals" (`docs/roadmap.md` 1240 on). "A" and "B" are the two reviewers. Every quotation below was compared with the file today; every count was made with `wc -l` or `grep -c`; every commit was checked with `git show --stat`. Nothing here recommends for or against a feature.

## 1. Size

### 1.1 Today's product code, file by file

Counted with `wc -l`, less each file's own `#[cfg(test)]` module or helpers (moved to the test table).

| Part | File | Lines | In the study's 11,970 |
|---|---|---|---|
| Core | `crates/verbatim-core/src/terminal.rs` | 1,177 | yes |
| Core | `crates/verbatim-core/src/terminal/reading.rs` (1,417, less 80 lines of `cfg(test)` helpers and the test module, 1338 to 1417) | 1,337 | yes |
| Core | `crates/verbatim-core/src/terminal/commands.rs` (363, less its test module, 326 on) | 325 | yes |
| Core | `crates/verbatim-core/src/terminal/items.rs` (167, less 126 on) | 125 | yes |
| Core | `crates/verbatim-core/src/terminal_model.rs` (861, less the test-only `pending_kinds_for_tests` and the module line) | about 851 | yes |
| Core | `crates/verbatim-core/src/terminal_model/review.rs` | 1,050 | yes |
| Core | `crates/verbatim-core/src/keys/terminal.rs` | 93 | yes |
| Core subtotal | | about 4,960 | study 4,950; B 4,970 |
| Rules | `crates/verbatim-text/src/terminal_reading.rs` | 2,410 | yes |
| Rules | `crates/verbatim-text/src/terminal_rows.rs` (2,895, less its `cfg(test)` lines) | about 2,885 | yes |
| Rules | `crates/verbatim-text/src/terminal_rows/align.rs` | 337 | yes |
| Rules subtotal | | about 5,630 | study 5,630 |
| Outpost | `crates/verbatim-outpost/src/terminal.rs` | 381 | yes |
| Outpost | `crates/verbatim-outpost/src/outpost/text_reads.rs` 715 to 957 (the terminal read, caret, history and find handlers) | about 243 | **no** |
| Remote operations | `crates/verbatim-uia-rops/src/terminal_snapshot.rs` | 621 | yes |
| Remote operations | `crates/verbatim-uia-rops/src/terminal_find.rs` | 383 | yes |
| Remote operations | `crates/verbatim-uia-rops/src/terminal_colours.rs` (791, less the test module, 741 on) | 740 | **no** |
| Remote operations | `crates/verbatim-uia-rops/src/terminal_rows.rs`, called by no product code (R1) | 198 | **no** |
| Wire types | `crates/verbatim-model/src/terminal.rs` (216, less 196 on) | 195 | **no** |
| Wire types | `crates/verbatim-model/src/text.rs`: `TextOp::TerminalHistory`, `TerminalFind`, `ReadTerminal`, `TerminalRead`, `TerminalReadReply` and the three `TextReply` variants (1022 to 1061, 1064 to 1110, 1183 to 1206) | about 110 | **no** |
| Wire codec | `crates/verbatim-process/src/message/raw.rs` (510, less 509 on): "How an outpost's answers that carry a terminal's text cross the pipe" | 508 | **no**; neither reviewer counted it (the merger's own count) |
| **Total** | | **about 13,970** | study 11,970; A about 13,200; B about 13,430 |

What the study's 11,970 leaves out: the colour read (740), the dead rows operation (198), the outpost's handlers (243), the wire types (305) and the wire codec (508), about 2,000 lines. The study's outpost figure, 1,390, is exactly `verbatim-outpost/src/terminal.rs` 381 plus snapshot 621 plus find 383 (1,385). Counted nowhere: the terminal arms in shared files (`keys/caret_key.rs` `terminal_read` 752 to 788, `entries.rs`, `editing.rs`, `input_desktop.rs`, the outpost's worker), and the settings page `crates/verbatim-gui/src/terminal_panel.rs` (357), which is settings UI rather than the reading.

### 1.2 Today's test code, file by file

| Crate | Files | Tests | Lines |
|---|---|---|---|
| verbatim-core, unit | `src/terminal/tests.rs` 1,681 (44 tests); `src/terminal/reading_tests.rs` 759 (31); `src/terminal_model/tests.rs` 393 (7); test modules in `commands.rs` 38 (1) and `items.rs` 41 (2); `cfg(test)` helpers in `reading.rs` 80 and `terminal_model.rs` about 8 | 85 | about 3,000 |
| verbatim-core, integration | `tests/terminal_reading.rs` 1,578 (56); `tests/terminal_floods.rs` 422 (14); `tests/terminal_cost.rs` 327 (1, ignored); `tests/terminal_support/mod.rs` 1,129 | 71 | 3,456 |
| verbatim-text | `src/terminal_reading/tests.rs` 2,027 (80); `src/terminal_rows/tests.rs` 1,460 (61); `tests/terminal_oracle.rs` 697 (11); `tests/terminal_cost.rs` 498 (1, ignored) | 153 | 4,682 |
| verbatim-term-sim (a test-only crate) | `src/oracle.rs` 2,447; `tests/oracle.rs` 2,122 (62); `src/conformance.rs` 420 and `tests/conformance.rs` 78 (2); `grid.rs` 748, `provider.rs` 495, `vt.rs` 254, `lib.rs` 222, `src/tests.rs` 139 (9) | 73 | 6,925, plus 48 MB in `conformance/expected` (134 files), 1.1 MB of `streams` (67) and 40 recipes |
| verbatim-test-console (recording tool) | `verbatim-stream-test/main.rs` 488, `phase.rs` 1,010, `lib.rs` 362 | - | 1,860 |
| mockapp | `tests/terminal.rs` 772, `tests/terminal_reading.rs` 727, `tests/terminal_snapshot.rs` 421 (custom harness); plus `tests/instruction_limit.rs` 746 to 796 for the dead rows operation | - | 1,920 plus about 51 |
| verbatim-outpost | `src/terminal/tests.rs` (3, one ignored) | 3 | 299 |
| verbatim-process | `src/message/raw/tests.rs` (the terminal answers' codec) | - | 409 (the merger's count) |
| verbatim-uia-rops, verbatim-model | test modules in `terminal_colours.rs` and `terminal.rs` | - | 51 and 21 |
| **Total** | | | **about 22,600**, plus 48 MB of expectations |

The study's "about 17,000" equals Core's unit and integration tests, `verbatim-text`'s `src` tests, the outpost's and the simulator's; it leaves out `verbatim-text/tests` (1,195), mockapp (1,920), the recording tool (1,860) and the codec's tests (409). B's account of what it leaves out is right; A's total (about 22,000) is right.

### 1.3 NVDA's counterpart

- `nvda/source/NVDAObjects/behaviors.py` 372 to 636, 265 lines: `LiveText` 372 to 510 (`_monitor` 468 to 505), `Terminal` 511 to 539, `EnhancedTermTypedCharSupport` 540 to 621, `KeyboardHandlerBasedTypedCharSupport` 622 to 635.
- `nvda/source/diffHandler.py`, 135 lines.
- `nvda/source/NVDAObjects/UIA/winConsoleUIA.py`, 512 lines, of which 39 to 355 (317 lines, `ConsoleUIATextInfo` and `ConsoleUIATextInfoWorkaroundEndInclusive`) serve console API levels below "formatted"; at the formatted level `_get_TextInfo` returns plain `UIATextInfo` (396 to 397).
- Total 912 (B's sum is right); 595 without the older consoles' TextInfo. A's and the roadmap's "about 800" match neither.
- NVDA has no unit test of any of them: `grep -rl "LiveText\|diffHandler\|winConsole" nvda/tests` finds nothing.

### 1.4 Ratio and what the extra lines buy

| | Verbatim | NVDA | Ratio |
|---|---|---|---|
| Product | about 13,970 | 912 (595) | 15.3 (23.5) |
| Tests | about 22,600 and 48 MB | 0 | - |
| The owner's target, 1.3 times NVDA | about 1,190 (775) | | |

The extra lines buy one cross-process hand-off (the outpost's read and handlers, the wire types and codec, Core's request lifecycle: about 1,700 lines on these counts, against A's 1,800 and B's 600, B leaving out the codec and Core's side), and the features NVDA lacks, F1 to F14, each ruled by Dickson (section 2.2), with F3's row model under F3, F7, F8, F11 and F12.

### 1.5 Deletion target

| | Product lines | Test lines |
|---|---|---|
| Remove now (R1 to R4) | about 230 | about 220 (tests about 100; the recording tool's pipe mode about 120) |
| If Dickson rules for a removal or speech change (section 4) | F3 dropped rather than kept at A's 200: about 200 more, with the study's 4,000 test lines of F3's present form going either way; F4 dropped: 50 to 60; groups handed to speech whole (Q13): about 130 | in proportion |
| Planned phase, M6.4, every feature kept | from about 13,970 to about 3,300 (A) or about 4,060 (B's 3,860, which drops F3, plus A's 200 for F3 kept): 9,900 to 10,700 lines | from about 22,600 to 3,000 (A) or 4,000 (B) |

## 2. Findings

| id | Title | Class | Found by | File and line | What goes | Product saved | Test saved | Speech changes |
|---|---|---|---|---|---|---|---|---|
| S1 | The study changes what is spoken in places it does not list | Question for Dickson (Q1 to Q8, section 4) | both (A two, B six) | `docs/design/terminal-rebuild.md` | nothing; the study's inventory and question 9 need them | - | - | yes; needs captures |
| S2 | Study state-table cells that do not hold or are undecided | Question for Dickson (Q4, Q5, Q9 to Q12) | both | the study's watch and pacing tables | nothing now | - | - | yes in Q4, Q5, Q9, Q10 |
| S3 | The study's inventory misses Core's caret record fed by every terminal read | Planned phase M6.4 (study correction) | A only | `terminal_model.rs` 800 to 802 | nothing now | - | - | needs a capture if the feed goes |
| S4 | The study's size section and questions 4, 5, 7, 8 predate its own rulings header | Planned phase M6.4 (study correction) | B only | the study, "Size" and "Questions" | nothing | - | - | no |
| R1 | The `terminal_rows` remote operation has no product caller | Remove now | B only | `crates/verbatim-uia-rops/src/terminal_rows.rs` | the file, its re-export, two mockapp tests, a crate-guide section | about 202 | about 95 | no |
| R2 | `verbatim-stream-test`'s `--pipe` mode has no caller | Remove now | A only | `crates/verbatim-test-console/src/bin/verbatim-stream-test/main.rs` | the pipe mode | - | about 100 to 130 (tool) | no |
| R3 | Dead read numbers on the wire | Remove now | A only | `verbatim-model/src/terminal.rs` 62 to 69; `text.rs` 1193 to 1196, 1203 to 1205; `verbatim-outpost/src/terminal.rs` 33, 272 | three fields, a counter, their codec arms | about 30 | about 10 | no |
| R4 | Two `serde(default)` fallbacks for old dumps | Remove now | both (`release_waiting`); A only (`ended_ms`) | `terminal.rs` 103; `verbatim-model/src/terminal.rs` 58 | two attributes | 2 | - | no |
| P1 | "A release read is out" held three ways; defect T1 is their drift | Planned phase M6.4 (T1 recorded there) | both | `reading.rs` 102, 283 to 285, 521; `terminal.rs` 98 to 104 | `release_waiting`, `deciding` | about 40 | - | fixes T1 |
| P2 | Terminal state in three homes, reset from four places | Planned phase M6.4 | B (A in part) | `terminal.rs` 1114 to 1123; `reading.rs` 315 to 368, 1131 to 1137; `input_desktop.rs` 101 | the separate resets | - | - | no |
| P3 | The flood policy's drop rule written three times | Planned phase M6.4 | both | `terminal.rs` 604 to 648, 704 to 744, 749 to 817; 412 to 420 and 528 to 536 | two of three | 200 (A) to 250 (B) | - | no |
| P4 | Mirror types converted field by field; padding removed in three crates | Planned phase M6.4 | both | `terminal/items.rs` 9 to 98; `terminal_model.rs` 491 to 650 | the mirrors | 150 to 230 | - | no |
| P5 | The setting, the focus's arrival and the key window each held twice | Planned phase M6.4 | both | `terminal.rs` 394; `terminal_model.rs` 126, 150, 287 to 305, 393 to 397, 845 to 853; `terminal_reading.rs` 594 to 599 | the copies | not estimated | - | no |
| P6 | Copies of the event queue's in-flight state | Planned phase M6.4 | both | `reading.rs` 142 to 153; `terminal.rs` 105 to 108, 287, 303 | `entry_read`, `cut_at_ms`, `now_us` | not estimated | - | no |
| P7 | The fresh-read glue written twice | Planned phase M6.4 | both | `terminal/commands.rs` 85 to 112; `keys/caret_key.rs` 752 to 788 | one copy | about 25 | - | no |
| P8 | Caps standing in for an owner's release | Planned phase M6.4 | both | `terminal_model.rs` 64, 69, 340, 441, 473; `review.rs` 44 | four caps | - | - | no |
| P9 | A line's life held as eleven fields; a parallel row array | Planned phase M6.4 | B only | `terminal_reading.rs` 305 to 360, 368, 426 | - | - | - | no |
| P10 | Silent fallbacks | Planned phase M6.4 | B only | `terminal.rs` 807 to 811, 1040; `reading.rs` 233 to 238, 618 to 623 | - | - | - | no |
| P11 | `Step.withheld` read only by the oracle | Planned phase M6.4 | A only | `terminal_reading.rs` 137 to 166, 257 | the field and `Reason` | about 60 | with the oracle | no |
| P12 | Identity by value conventions | Planned phase M6.4 | A only | `terminal_model.rs` 73; `terminal.rs` 562 to 571; `terminal_rows.rs` 2805 | - | - | - | no |
| P13 | A 760-line function | Planned phase M6.4 | B only | `terminal_reading.rs` 907 to 1668 | - | - | - | no |
| P14 | Every terminal remote operation written twice, remote and classic | Planned phase M6.4 (the window read's design; the pattern is the remote-operations crate's) | B only | `terminal_snapshot.rs` 315, 538; `terminal_colours.rs` 580, 666; `terminal_find.rs` 121, 320 | - | about 700 if one path | - | no |
| P15 | The study's citations into shared code have drifted; one NVDA citation is wrong | Planned phase M6.4 (study correction) | A (shared code), B (NVDA) | the study | - | - | - | no |

### 2.1 Remove now

**R1. The `terminal_rows` remote operation (B only; confirmed).** `crates/verbatim-uia-rops/src/terminal_rows.rs` is 198 lines; `lib.rs` 62 (`mod terminal_rows;`) and 106 to 108 re-export `MOST_RUNS, RowsAnswer, RowsQuery, TerminalRowsFn, terminal_rows, terminal_rows_classic, terminal_rows_remote`. `grep -rn "terminal_rows\|RowsQuery\|RowsAnswer\|TerminalRowsFn\|MOST_RUNS"` over `crates`, `xtask` and `uia-facts`, outside `verbatim-text`'s own `terminal_rows` module, finds only the module, its re-export, `crates/mockapp/tests/terminal_snapshot.rs` 23 to 24 and 299 to 337 (`rows_by_their_place_are_read_from_the_kept_start_both_ways`, registered at 413 to 414) and `crates/mockapp/tests/instruction_limit.rs` 26 to 30, 746 to 796 (`terminal_rows_execute_exactly`) and 1187 to 1188. `git log -S "terminal_rows(" -- crates/verbatim-outpost` shows its last product caller removed by b10e5fc1 (2026-10-24, "Leave floods to Core and speak every line the alignment finds new"). Also goes: "Rows by their place" in `books/dev/src/reference/crates/verbatim-uia-rops.md` (381 to 393). About 202 product lines and about 95 test lines.

**R2. The pipe mode of `verbatim-stream-test` (A only; confirmed).** The module documentation (11 to 17) describes `--pipe <name>` with `run`, `paced`, `frame` and `quit`; the code is `Event::Command` (52 to 53), `pipe` (63, 71, 82), the pipe thread and replies in `run` (272 to 292, 307 to 315) and `Session::command` (431 on). All 40 recipes run `"{bin}\\verbatim-stream-test.exe --write-log {write_log} --run <phase>"`; `grep -rn "\-\-pipe\b\|-commands\|-replies"` over `crates`, `xtask` and `uia-facts` finds this pipe only in this file (the listener's and outpost's `--pipe-in` belong to other programs). About 100 to 130 lines of tool code; the exact figure needs the removal, since `paced` and `frame` share `Session` with the `--run` path.

**R3. Dead read numbers (A only; confirmed).** `TerminalSnapshot.read` (`verbatim-model/src/terminal.rs` 62 to 69, "0 when not numbered", `#[serde(default)]`) is set only by `wire.read = READS.fetch_add(1, Ordering::Relaxed) + 1;` (`verbatim-outpost/src/terminal.rs` 272; `static READS` at 33). Core's fingerprint ignores it (`terminal_model.rs` 671, `read: _`). `TextReply::TerminalHistory.read` and `TerminalFound.read` (`text.rs` 1193 to 1196, 1203 to 1205) are always sent as `read: 0` (`text_reads.rs` 883 and 922), and Core matches both with `..` (`terminal/commands.rs` 192 to 196, 261). `grep -rn "snapshot\.read\b\|reply\.read\b"` over `crates` finds no reader; the other hits for the fields are writers, test fixtures, and `verbatim-process/src/message/raw.rs` (75 to 76, 117, 190 to 203, 267, 286, 351 to 366), which carries them across the pipe and loses those arms with them. About 30 product and 10 test lines. The fields sit in wire types the window read replaces at M6.4; deleting them now changes nothing spoken.

**R4. Fallbacks for old dumps (`release_waiting` both, `ended_ms` A only; confirmed).** `terminal.rs` 103, `#[serde(default)]` on `release_waiting`; `verbatim-model/src/terminal.rs` 58, `#[serde(default)]` on `ended_ms`, documented "0 when not known, the start standing in", read through `snapshot.ended_ms.max(snapshot.started_ms)` (`terminal_model.rs` 518). The outpost always sets it (`verbatim-outpost/src/terminal.rs` 120 and 271). 46638337 (2026-11-08, "Keep no fallbacks for dumps older code wrote", 14 files) removed this kind elsewhere.

### 2.2 Features beyond NVDA's loop: today's cost, ruling, and the least each could cost

Names and numbers are the study's. "Today" figures are the reviewers' line-range attributions, good to about 15 percent, since one 760-line function (`TerminalReader::handle`) serves most features (P13). Rulings are quoted from `books/dev/src/explanation/design/terminal-model.md` ("TM", with the rule number). The roadmap's M6.4 entry adds: "Every existing terminal feature is kept unless Dickson authorizes its removal (Dickson, 2026-11-19); among them, by name: keys owning the changes they cause, lists and colours (fzf, tig, Microsoft Edit), since many terminal programs need them; the review cursor over the terminal's text and moving by command; holding reads while speech waits during a flood; an unfinished word held; volatile rows."

| Feature | What it says or does | Today, product (tests) | Ruling | A's least cost and idea | B's least cost and idea | Cheapest form changes speech? |
|---|---|---|---|---|---|---|
| F1 flood groups and skip counts | first 30 lines whole, "skipped N lines", newest 30; "about", "more than"; 10 MB bound, "line cut" | A about 900, B about 610 (tests A about 1,200, B about 1,400) | TM "Core's flood policy": "Reason: NVDA has no flood control (Dickson, 2026-10-06 to 2026-10-17, and 2026-10-25 for "about")" | about 250 with the pacing: the bound written once (P3), each group handed to speech whole with one closing mark | about 120 on top of the study's 380-line pacing: one "drop the oldest past keep, count them" function; no "about" or "more than" unless F3 stays | A's: yes, when "report new output" is turned off mid-group (Q13); B's: loses "about" and "more than" only with F3 dropped |
| F2 holding reads while speech waits | no read while 30 lines wait; one read at the group's last line | A about 250, B about 300 (B: about 500 test lines) | TM rule 1: "a flood once took 2,800 reads (Dickson, 2026-10-07 and 2026-10-14)"; kept by name (roadmap, 2026-11-19) | about 40, the study's derived hold | about 40, the same | yes in a long flood (Q9); needs a capture |
| F3 exact counts and the row model | "skipped 9940 lines" through a full history; anchored starts, history block, alignment, lingering, aside screen | A about 3,500, B about 3,800 (tests: study 4,000; B about 7,000 with the oracle) | TM status line (2026-10-25): "the count in "skipped N lines" need not be exact"; not named as kept; the study proposes dropping it | about 200: the outpost keeps one range at the last read's end and counts rows from it in the same remote operation, "more than" when refused; a question (Q14) | 0, if Dickson authorizes dropping it | A's: needs a capture; B's: "skipped lines" with no count (the study's F3 sequence) |
| F4 memory across focus changes | review position and marks survive Alt+Tab | about 250 (both) | TM rule 15: "the memory lasts until the terminal closes" | about 50, places kept in the navigator or the outpost | about 60, a per-terminal record dropped with the outpost | no, if F11 and F12 keep their places |
| F5 an unfinished word held | "The", "quick brown", "fox" for a streamed reply | about 250 (both) | TM rule 13: "streamed replies (Dickson, 2026-10-23)"; kept by name | about 120: one timer and the last inserted line's held tail | about 100: one `Option<(String, due)>` | no by intent; needs recordings |
| F6 a changed line from its first changed word | vim's ruler "2,1 Top"; today a spinner's glyph is silent | about 70 (both) | TM rule 16: "vim's ruler and tig's count (Dickson, 2026-10-07 and 2026-10-08)"; spinner: "Pi's and Claude Code's spinners (progress recording)" | 70 | 70 | yes: the study's F6 has no spinner rule (Q6) |
| F7 volatile rows | "Working... 1s", then "Working... 34s" | A about 500, B about 450 | TM rule 17 (Dickson, 2026-10-23); kept by name | about 200: compare the visible screen by place, rows counted from the document's end | about 150: a screen-tall array by place with each row's last two change times | needs recordings: place replaces row identity |
| F8 keys owning changes, lists, suggestions, colours | "> banana" in a list; suggestions; emphasis; caret-line answers; deferred keys | A about 2,500 plus the 740-line colour read; B about 2,600 with it | TM rules 18 to 23 (Dickson, 2026-10-23 to 2026-10-26); kept by name, "many terminal programs need it" (the study's rulings header) | about 700: one key window in the watch, the screen before the key compared by place with the first read after it within NVDA's caret wait, colours read only for changed rows | about 1,200: screens before and after compared row by row at the same places (Dickson's direction), one colour read | intended no; needs recordings ("each checked against recordings in the phase") |
| F9 typing echoed as the terminal shows it | a password prompt with a clock speaks no typed character | study 80 (rebuilt form); B about 300 today | TM rule 20 (Dickson, 2026-10-07 to 2026-10-23) | about 80 | 80 | not by itself; but the study also adopts NVDA's drops (Q2, Q8) |
| F11 review over Core's snapshot | review position kept while output arrives | A about 1,400, B about 1,000 | TM rule 24 (Dickson, 2026-10-23 and 2026-10-25); kept by name | about 200: the position as rows from the document's end plus its line's text, found by one remote read at the command | about 200 on `review_text.rs`, the same direction | needs recordings |
| F12 moving by command (marks) | Verbatim+Alt+Up and Down Arrow | A about 400, B about 1,070 (with `terminal_find.rs` 383) | TM "Moving by command": "Reason: Dickson, 2026-10-23 and 2026-10-26" | about 250, on ranges the outpost holds or a live search | about 250, plus a find written once (about 200) | needs recordings |
| F13 Report status bar | Verbatim+End reads the footer | about 100 (both) | TM "Report status bar" (Dickson, 2026-10-23 and 2026-10-26) | 100 | 100 | no |
| F14 a new screen speaks its screen | `less` closed: the shell's screen | A about 20, B about 300 (rules 7 and 8 today) | TM rule 7 (Dickson, 2026-10-24 and 2026-10-25) | 20 | 20 | yes in floods: conflicts with F1 (Q10) |

**The two floors.** A's: about 1,050 for the core (the study's 1,270 with the bound written once and each group handed whole) plus the features above, about 3,300, 3.6 times 912. B's: the study's 1,270 plus 120 + 40 + 60 + 100 + 70 + 150 + 1,200 + 80 + 200 + 450 + 100 + 20 = 2,590, about 3,860, 4.2 times 912 (the arithmetic checks). The assumptions that differ:

- **F3.** A keeps it at about 200, in a form that breaks the study's "keeps nothing between reads"; B counts it at 0, which needs Dickson's authorization. So B's 3,860 is not "every feature kept"; with A's F3 it is about 4,060.
- **F8.** A about 700 (one key window in the watch, colours only for changed rows); B about 1,200 (full before-and-after comparison, one colour read). The largest difference, 500 lines.
- **The core's pacing.** A hands each group to speech whole (a speech change, Q13), saving about 220; B keeps the study's pacing.
- **F12.** B adds a separate find of about 200.
- **F1, F4, F5, F7.** Within 50 to 130 lines of each other.

Both floors miss the owner's 1.3 target (about 1,190) by about three times; F8 alone, in either form, is 60 to 100 percent of that target.

### 2.3 What the study's inventory misses

**S3. Core's caret record (A only; confirmed).** `terminal_model.rs` 800 to 802: `if let Some(caret) = &snapshot.caret { crate::editing::update_caret(state, node, caret.clone()); }`. The outpost fills it on every full read: `text_reads.rs` 811 to 817, "a terminal raises no caret event for every character typed, so the read keeps Core's caret following typing", and a caret key's starting point is minted from the same caret. `editing::update_caret` (`editing.rs` 548) writes `focus.caret`, which `review_text.rs` 138 and `editing.rs` 100 read. The study's window read carries only "the caret's row and column" (study line 70), and `grep -n caret` over the study finds no line for this record. The study must say what feeds the focus's caret in a terminal after the rebuild.

**Other gaps** (each verified, each in section 4): blank lines counted (Q1), short output spoken (Q2), output written during a hang (Q3), a baseline that fails or passes its due time (Q4), the spinner's silence (Q6), rules 9 and 10 for redraws (Q7), NVDA's Tab exemption (Q8), the outpost ending while lines wait (Q11), the desktop switch (Q12). The size gaps are in section 1.1.

**S4. The study's size and questions predate its rulings (B only; confirmed).** The header, "Rulings so far (Dickson, 2026-11-19)", names F2, F5, F7, F8, F11 and F12 as kept; "Size" still says "With the recommended set (F1, F6, F9, F13, F14) it is about 1,720 product lines", and questions 4, 5, 7 and 8 still recommend dropping F2, F8, F5, F7, F11, F12 and F4. The size section describes no target Dickson can approve.

**P15. Citations (A and B; confirmed).** The study cites `entries.rs` "`exempt_from_foreground` 525 to 624, `kept_by` 690 to 700, `merges` 861 to 884" and `reduce.rs` 516 to 534; they are now at 541, 732 and 925, and the constructor `focus_context` at 525 to 543 (its literal at 531). The study cites `winConsoleUIA.py` 408 to 414 for "read the whole document"; those lines are `_get_diffAlgo` (difflib below the formatted level, `super().diffAlgo` at it). The whole-document read is `LiveText._getText`, `behaviors.py` 446 to 453 (`makeTextInfo(textInfos.POSITION_ALL)`). The study's other NVDA citations hold: `behaviors.py` 468 to 505, 519 to 525, 559 to 569, 572 to 592, 595 to 611; `diffHandler.py` 39 to 48; `winConsoleUIA.py` 374 to 376 and 470. `STABILIZE_DELAY = 0` at `behaviors.py` 381; only `window/winConsole.py` 23 sets 0.03.

### 2.4 Planned-phase findings (M6.4)

**P1. "A release read is out" (both; confirmed).** `reading.rs` 102 `Waited { release: bool }`; 283 to 285 `if mark == (Mark::Waited { release: true }) { state.terminal.release_waiting = true; }`; 521 `SpeechEntry.release`; `terminal.rs` 98 to 104, `deciding: bool` and `release_waiting: bool`. `committed` (`reading.rs` 315 to 368) replaces `focused` but never touches `state.terminal`, so `release_waiting` survives a re-take; `pump` and `mark_reached` then set `deciding` (1018 to 1021, 1060 to 1063) and nothing clears it. This is T1 as the roadmap records it ("a terminal silent after its reading is dropped during a flood, until a key").

**P2. Three homes, four resets (B, A in part; confirmed).** `state.terminal`, `state.terminal_focus`, `state.terminals`. Resets: `terminal::focus_moved` (`terminal.rs` 1114 to 1123, called at `reduce.rs` 678), `reading::committed`, `reading::outpost_ended` (1131 to 1137) and `input_desktop.rs` 101. `committed` does not clear `entry_read`; `outpost_ended` (1135) and the entry's end do.

**P3. The drop rule three times (both; confirmed).** `Bounded::add` (604 on, documented "as [`skip_all_but`] would"), `skip_all_but` (704 to 744), `bound` (749 to 817); `Adding::is_line` and `belongs` (528 to 536) copy `is_line` and `belongs` (412 to 420). The "group ended" test is written in `pump` (1013 to 1025) and in `mark_reached` (1057 to 1066). The reviewers' 200 and 250 differ by under a quarter; no recount decides a rank, since all of it is in code the rebuild replaces.

**P4. Mirror types (both; confirmed).** `TerminalUnit`, `TerminalItem`, `TerminalKeyKind`, `Skipped` (`terminal/items.rs` 9, 20, 71, 90) against the rules' `Unit`, `Speech`, `KeyKind`; converted by `rules_key` (491), `speech_of` (613), `colours_of` (594), `read_of` (525) and `snapshot_of` (550). `snapshot_of` is called only from tests (`terminal/tests.rs` 569, `reading_tests.rs` 95, `tests/terminal_reading.rs` 591, `tests/terminal_support/mod.rs` 177; the outpost's and `verbatim-uia`'s `snapshot_of` are other functions), yet exported from `lib.rs` 103. Padding is removed three times: `TerminalText::unpadded` and `padded` (`verbatim-model/src/terminal.rs` 97, 116), `ReadText::unpadded` and `padded` (`terminal_rows.rs` 108, 127), `unpadded_utf16` (`verbatim-outpost/src/terminal.rs` 43).

**P5. Facts held twice (both; confirmed).** "Report new output" judged in Core (`terminal.rs` 394, `!state.settings.report_terminal_output`) and copied into every model by `sync_settings` (`terminal_model.rs` 845 to 853), run after every input while a model exists (`reduce.rs` 78 to 80). The focus's arrival: `TerminalModel.focus_arrived` (150) and `TerminalReader.fresh` and `focused` (`terminal_reading.rs` 596 to 599). The newest key: `TerminalModel.last_key: Option<(TraceId, u64)>` (126) and `TerminalReader.last_key: Option<(u64, Option<u64>)>` (594), the window judged on `KEY_WINDOW_MS` in `colours_owed` and `takes_colours` (287 to 305) and `speech_key` (393 to 397) as well as in the rules.

**P6. Copies of the queue's in-flight state (both; confirmed, one claim narrowed).** `entry_read: bool` (`reading.rs` 142 to 144); `cut_at_ms` (`terminal.rs` 105 to 108) with `observed_at_ms != 0 &&` (287) and `started_ms != 0 &&` (303), 0 standing for "not supplied"; `now_us` (`reading.rs` 147 to 149). A's claim that `deferred` "copies the stroke machinery's hold" is narrowed: `keys/terminal.rs` 50 and 82 read `state.strokes.holds()` at each decision; what is held twice is the list of keys behind the hold.

**P7. The fresh-read glue twice (both; confirmed).** `terminal/commands.rs` `fresh` 85 to 112 and `keys/caret_key.rs` `terminal_read` 752 to 788 carry the same `entry_read`, `cx.ask`, six-arm outcome match, `observed`, `entry_read_ended` and `speak_read`.

**P8. Caps (both; confirmed).** `MODELS = 16` (64; eviction at 441), `PENDING_KEYS = 256` (69; at 473, past it the oldest key goes to the rules unread), `KEPT_PLACES = 64` (`review.rs` 44, 189), `mark_lines` at `2 * MARKS` (340).

**P9. Eleven fields and a parallel array (B only; confirmed, one detail corrected).** `LineState` (`terminal_reading.rs` 308 to 335): `redrawn`, `volatile`, `changed_ms`, `interval_ms`, `changed_read`, `unprompted_ms`, `grew_ms`, `grew_interval_ms`, `hold: Option<Hold>`, `input`, `cut`. `due` (348 to 359) returns `None` for a `Hold::Word` whose `grew_ms` is `None`, so such a hold is never due (B wrote "due at no time"). `Screen`'s trim, "Drops the first `dropped` rows, as the memory trimmed them" (426), and `RowState.text`, "The memory's row's text, shared" (368), keep a second array in step with the memory.

**P10. Silent fallbacks (B only; confirmed, one location corrected).** `terminal.rs` 807 to 811, `(Some(_), None) => { debug_assert!(false, "no count before the line at {index}"); terminal.waiting.insert(index, count); }`; `reading.rs` 618 to 623, `read_reply`'s `_ => None` takes a reply of another kind as refused; `full_read` with no model sends `height: 0` and `width: 0` (`reading.rs` 233 to 238, not `terminal_model.rs` as B wrote); `mark_reached`'s `let Some(trace_id) = terminal.trace else` (`terminal.rs` 1040).

**P11. `withheld` (A only; confirmed).** `Step.withheld` (`terminal_reading.rs` 257), `Withheld` (158), `Reason` (137). `grep -rn "withheld\|Withheld" crates` outside the rules and their tests: `verbatim-term-sim/src/oracle.rs` (44 lines), `verbatim-term-sim/tests/oracle.rs` (8), `verbatim-text/tests/terminal_oracle.rs` (6), `verbatim-text/tests/terminal_cost.rs` (2). No product reader; `speech_of` takes `step.speech`.

**P12. Identity by value (A only; confirmed).** `TERMINAL_ANCHORS: u64 = 1 << 63` (`terminal_model.rs` 73); `Skipped::MoreThan(0)` and `Skipped::About(0)` as markers rather than counts (comment at `terminal.rs` 547 to 550; 562 to 571); rows found again by `hash` of their text (`terminal_rows.rs` 2805).

**P13. One 760-line function (B only; confirmed).** `fn handle` at `terminal_reading.rs` 907; the next function, `caret_step`, at 1669.

**P14. Remote and classic twice (B only; confirmed).** `terminal_snapshot_remote` 315 and `terminal_snapshot_classic` 538; `go_on_remote` 580 and `colours_classic_read` 666; `terminal_find_remote` 121 and `terminal_find_classic` 320. The crate's pattern, outside this area; it bears on how the window read is written.

## 3. Test findings

### 3.1 Survives a rebuild, or bound to today's mechanism

| Crate and file | Tests, lines | States behaviour (survives) | Bound to today's mechanism |
|---|---|---|---|
| core `src/terminal/tests.rs` | 44, 1,681 | about 10 (B) to 20 (A): cuts, the toggle, "line cut", a group's speech, `blank_lines_count_when_skipped` | the rest: `Bounded`, `bound`, marks ahead, `deciding`, F3's counts |
| core `src/terminal/reading_tests.rs` | 31, 759 | 0 | all 31 (Tables 1 to 4: `Follow` resent, `Stalled`, `Holding`, `Behind`) |
| core `src/terminal_model/tests.rs` | 7, 393 | 0 | all 7 |
| core `tests/terminal_reading.rs` | 56, 1,578 | about 12 (A) to 29 (B): status bar, marks, review, echo, the hang, dump and replay, line by line | history and search requests, owed reads, snapshot round trips, colour requests |
| core `tests/terminal_floods.rs` | 14, 422 | stream in, speech out (B's point); but 12 of 14 assert F3's counts ("counted by text", "counted exactly", "says more than", "counts its rows once", "about"), and only `a_full_screen_redraw_goes_through_the_limits_in_the_console_host` and `..._in_windows_terminal` (267, 272) assert F1 alone | the 12 survive only with F3 kept (A's "all F3" is right on content) |
| core `tests/terminal_cost.rs` | 1 ignored, 327 | - | a measurement of the whole-document alignment, backing `performance.md` |
| core `tests/terminal_support/mod.rs` | 1,129 | harness | shrinks with the window read (B: about 500) |
| text `src/terminal_reading/tests.rs` | 80, 2,027 | about 20 to 30 on keys, typing, F5, F6, F7, F9, F14, restatable as diff tests | about 50 to 60 judged on `Step` and `Withheld` reasons |
| text `src/terminal_rows/tests.rs` | 61, 1,460 | 0 | all (alignment, row outcomes, aside screen, lingering) |
| text `tests/terminal_oracle.rs` | 11, 697 | 0 | a generated test over the alignment, with no ordering across a hand-off |
| text `tests/terminal_cost.rs` | 1 ignored, 498 | - | a measurement |
| term-sim | 73, 6,925 | grid, VT parser, provider and the conformance test (2 tests): the stand-in checked against recordings, which CLAUDE.md requires | `oracle.rs` 2,447 and `tests/oracle.rs` 2,122 (62 tests): generated invariants of the rules, no ordering across a hand-off |
| mockapp terminal tests | 1,920 | the read against a real provider, line by line, Backspace, status bar, caret watch, colours | history and find from a kept start; R1's test |
| outpost `src/terminal/tests.rs` | 3, 299 | 2 (`unpadded_utf16`) | 1 ignored measurement |

At M6.4, A removes about 19,000 lines and keeps or rewrites about 3,000; B removes about 14,000, replaces about 2,000 with about 900 of table-cell tests and keeps about 2,600. They differ mainly on the simulator (A keeps about 1,200, B about 1,500), on `tests/terminal_reading.rs` (12 against 29 kept) and on the floods file (above).

**Removable now:** R1's two mockapp tests (about 95 lines) and R2's pipe mode (about 100 to 130 tool lines). The three ignored measurements (1,152 lines) back figures in `books/dev/src/reference/performance.md` and wait for M6.4.

**Allowances:** neither reviewer found an optional utterance or a repeat count in this area's unit and integration tests, and the merger found none in the tests read for this record.

### 3.2 Captured NVDA speech

No unit or integration test in this area reads a capture. `grep -rln -i "captur\|nv-con"` over the area's tests finds only comments that cite a capture for a hand-written expectation (`terminal_reading/tests.rs` 950, 969, 1576; `terminal_rows/tests.rs` 560) and the measurements' `--nocapture`. Expectations from captures live only in the end-to-end scenarios, outside this area. The 67 recorded streams reach the rules only through the oracle's invariants. A proposes one test per end-to-end family with a capture that replays a recorded stream through the simulator into Core and asserts the captured speech (about 40 lines each); B proposes the defect tests below.

### 3.3 The smallest set that fails on this area's recorded defects

Merged by defect (A listed 8, B 10; together 11); every commit checked with `git show --stat`. Each is one Core test fed in the listener's and outpost's order, about 30 to 40 lines, about 400 lines in all.

| Defect | Found by | Commit (date, subject, files) | Today's test | The test to keep or write |
|---|---|---|---|---|
| T1: a terminal silent after its reading is dropped during a flood | both | unfixed; roadmap M6.4 | none | a flood held, the re-take commits while the release read is out; the next output is spoken |
| T2: typing echoed during a read, stamped with the front's fence and dropped | both | unfixed; roadmap M6.4 | none | a caret key's read out under key 8, "z" typed as key 9; "z" is spoken |
| A modifier's cut left later speech under an older fence | both | 6a9e55dc (2026-11-17, "Carry the cutting key's number in a speech cut, so later speech outlives it", 12 files) | `speech_arriving_after_a_modifiers_cut_carries_the_modifiers_number` (`tests/text.rs`), five flood tests adjusted | a cut by a modifier alone, then terminal output; it is spoken |
| A reply type that could not hold "no text" | both | 75b5fb5c (2026-11-17, "Decode every answer a text request can have, "no text" included", 19 files) | `a_review_commands_fresh_read_answered_with_a_failure_decodes` (`terminal/tests.rs` 1330) | the window read answered "no text" takes the failure cell |
| A review command looping on "anchor lost" | both (A: only with F11 kept) | 33b0523c (2026-11-17, "End a review command whose read at the caret finds the caret's line lost", 3 files) | the same test, extended | "review current line" with a failed fresh read ends saying nothing |
| An ended outpost asked for reads | both | b7038a48 (2026-11-03, "Keep the flood policy's hold in the terminal's model, so an ended outpost is asked nothing", 4 files) | `output_read_before_the_outpost_ended_is_spoken_and_nothing_is_asked_of_it` (`terminal/tests.rs` 1616) | the outpost ends while lines wait: nothing asked of it, the lines spoken |
| A typed line's wrap shown over two reads | both | f0902400 (2026-11-09, "Say nothing for a typed line's wrap shown over two reads", 6 files) | `a_typed_lines_wrap_shown_over_two_reads_says_nothing_for_the_empty_row` (`terminal_reading/tests.rs` 1386) | `conhost_typing`'s recorded reads; nothing said |
| A key reaching the program during a read | B only | ffce203b (2026-10-26, "Order a terminal's keys against its reads by when each read ended", 10 files) | `a_key_that_reached_the_program_while_a_read_was_taken_is_given_before_it` | the echo goes out under that key |
| A recalled line and a typed character | B only | f4fb5828 (2026-10-26, 5 files) and 1adc1421 (2026-10-26, "Settle a terminal's colours hold at the next coloured read, and keep typing a recalled line does not show", 7 files) | `a_character_typed_after_up_arrow_and_shown_after_the_recalled_line_is_output` and `..._is_echoed` | Up Arrow, then "t", with "cat a.txt" recalled |
| An output read past its due time | A only | not a commit: today's behaviour (`reading.rs` 863 to 868) that the study's table drops (Q5) | none found | answered `NoLongerAwaited(DueTime)`: one more read is sent |
| Output written during a hang | B only | not a commit: today's behaviour (Q3) | `a_hung_terminal_sounds_then_speaks_its_output_once_the_window_answers` (`tests/terminal_reading.rs` 1563 to 1578) | keep as it is |

## 4. Questions for Dickson

Each changes what is spoken against today, or leaves a cell of the study undecided. "Needs a capture" means the repository holds none. NVDA's behaviour is from the pinned source at the lines given.

**Q1. Blank lines counted in the flood policy (A; confirmed).** The ruling: `terminal.rs` 544 to 546, "Every line counts, blank ones included (Dickson, 2026-10-07, and the coherence review of 2026-10-09): a blank line waits like any other and is counted when skipped", and TM "Core's flood policy", "every line counts, blank lines included". The fact that changed: the study adopts NVDA's diff, which drops blank and white-space lines (`diffHandler.py` 46, `if line and not line.isspace():`), so they never reach the count; NVDA has no flood policy. Sequence, `blank_lines_count_when_skipped` (`crates/verbatim-core/src/terminal/tests.rs` 812 to 823): 30 lines, 10 blank lines, 30 lines, limits 30 and 30. Today the test expects lines 1 to 30, "skipped 10", lines 41 to 70. Under the study's rules the blanks are dropped, 60 lines reach the policy and no skip is said (derived from the study, not captured). A estimates about 10 lines to keep the ruling (blank lines passed to the pacing, counted, not spoken). Not listed in the study.

**Q2. Output of one character (A and B; confirmed).** The ruling: TM rule 20, "output that is not the typing is output however short", and `docs/parity/terminals.md` ("Typing"): "In place of NVDA's rule dropping a change of one or two characters ... A line of one or two characters is output like any other", and "At a password prompt that shows asterisks Verbatim speaks the asterisks the console shows, where NVDA echoes each typed character". NVDA: `behaviors.py` 491 to 495 drops a result of one line of one character; 559 to 569 drops one line shorter than the typed word plus one, or than three characters, unless after Tab (Q8). The study keeps both ("A read's lines pass NVDA's two drops"). Sequence, `with_passwords_spoken_a_character_is_heard_as_typed_and_its_asterisk_as_shown` (`crates/verbatim-core/tests/terminal_reading.rs` 418 to 434): "speak passwords" on, "Password: ", "s" typed, the prompt shows "*". Today: "s", then "*". Under the study, by NVDA's source: "s" only; needs a capture. A's second example, `lines_of_one_and_two_characters_after_enter_are_output_not_echo` (437 to 454), is not changed: its read inserts three lines ("y", "ok", "ready>"), and both drops apply only to a single line. Not listed in the study.

**Q3. Output written while the terminal hangs (B; confirmed).** Today `recovered` (`reading.rs` 1067 to 1127) wants an output read when the window answers, before `crate::focus::retake::recovered` (`entries.rs` 703 to 708) queues the re-take; `a_hung_terminal_sounds_then_speaks_its_output_once_the_window_answers` (`tests/terminal_reading.rs` 1563 to 1578) writes "alpha" and "beta" while hung and asserts `plain(&["alpha", "beta"])` once the window answers. The test cites the build-3 design's sequence 14; no dated ruling of Dickson's is cited. The study files the hang under "goes", "The recovery's re-take commits, which baselines anew", and its baseline is silent, so under the study nothing is spoken. NVDA by its source: `_monitor` calls `_getText` (488) with no timeout of its own and diffs against `oldText` when it returns, so it would speak "alpha" and "beta"; whether UI Automation's own call gives up first on a hung console is not in these lines. Needs a capture. Not listed in the study.

**Q4. A baseline refused or past its due time (B; confirmed, B's speech corrected).** Today `baseline_ended` (`reading.rs` 784) maps `Outcome::Refused(_) | Outcome::NoLongerAwaited(_)` to `None`, which leaves `Baselining { read: None, .. }` (786 to 793); the next tell sends a new, silent baseline (`told`, 397). The study's cell "Baselining clean, C other: Live, old empty, Idle clean (NVDA 473)", column C including `NoLongerAwaited(_)`: the next read is diffed against empty text, so the whole window, up to two screens, is spoken through F1. B wrote "30 lines, skipped N lines, 30 lines"; with two screens of 30 rows about 60 lines reach the policy and no skip follows, so the exact speech needs a capture. NVDA: 473 is the exception path of the first `_getText`; NVDA has no due time, its first read blocking until it returns and setting `oldText` silently. B's option: split column C into "abandoned: baseline again" and "failed: empty".

**Q5. An output read past its due time (A; confirmed).** Today `output_ended` (`reading.rs` 863 to 868): `Outcome::NoLongerAwaited(CancelReason::DueTime) => { // One read sent again with the same mark (column D).` Output reads carry `due_after_us: OTHER_DUE_US` (`reading.rs` 181), 11 seconds (`event_queue/mod.rs` 63). The study's cell "Out clean, C: Idle clean, old kept (NVDA 504)": `dirty` was cleared when the read was sent, so the change is not read until another arrives. Sequence: Windows Terminal, `cargo build` prints "Finished" and stops; the read passes 11 seconds. Today one more read follows and "Finished" is spoken; under the study it is spoken only with the next change. NVDA: 504 is the exception path; NVDA has no due time. A's fix costs no lines: `DueTime` from Out goes to Idle dirty, `NotResponding` and `Hung` stay clean. Exact speech needs a capture.

**Q6. The spinner's silence (B; confirmed, B's framing corrected).** Today `spinner` (`terminal_reading.rs` 2196 to 2205), "Whether exactly one character changed, neither a letter nor a digit", tested by `a_spinner_symbol_replaced_alone_is_silent` (`terminal_reading/tests.rs` 388 to 395); ruled in TM rule 16 ("silent when the one character that changed is neither a letter nor a digit (a spinner)"; "Pi's and Claude Code's spinners (progress recording)") and in `docs/parity/terminals.md` 108. The study's inventory files "a spinner silent (rule 16)" under F6, but F6's definition ("widening each inserted run on a line that had text to the start of its word", about 70 lines) has no spinner rule. NVDA: the glyph alone is one line of one character, dropped (491 to 495); with a new output line in the same read the result has two lines and the glyph is spoken. Sequence: Claude Code streams, and one read shows a new line and the spinner's next glyph. Today: the line only. Under the study's F6 as written: the line and the glyph. Needs a capture.

**Q7. Redraws, rules 9 and 10 (B; confirmed).** TM rule 9: "a read can land inside Ink's erase and redraw (Claude Code; Dickson, 2026-10-24)"; rule 10: "A read whose screen shows no text where the last read's had text changes nothing but the caret". The study: "lingering rows, reads with no text ... the rest go with F3", with no feature code, so they cannot be kept on their own. Sequence: Claude Code's Ink erases and redraws its input box and a read lands between. Today: silent. Under the study's diff the next read inserts the box again and speaks it, as NVDA's diff does by its source. Needs a recording and a capture.

**Q8. NVDA's Tab exemption (B; confirmed, B's example corrected).** NVDA: `_reportNewLines` drops a single short line only `and not self._hasTab` (`behaviors.py` 559 to 564); `event_typedCharacter` sets `_hasTab` on Tab and clears the typed word buffer (573 to 578). The study lists "NVDA's two drops" with no Tab exemption. After Tab the buffer is empty, so the threshold is three characters, and a one-character insert is dropped by 491 to 495 whatever the Tab: the exemption matters only for a one-line completion inserting exactly two characters. B's example, "Get-Ch" completed to "Get-ChildItem", inserts seven and is spoken either way. Today Verbatim speaks every completion (TM rule 20; parity: "so a tab completion's added text is read"). Needs a capture of a two-character completion.

**Q9. F2 derived from the waiting count, against F1's look (B; confirmed; A's contrary claim settled).** Today the hold begins when `lines_hold` (`terminal.rs` 326 to 335) and is released only at the group's last line by `group_last_line` (`reading.rs` 969 to 991), and the look at the backlog waits for that read's speech (`pump` 1013 to 1025 sets `deciding` while `release_waiting`). The study holds while "waiting lines at least 'Lines spoken in full'", and the look "runs at its mark whatever the reading does" (structural rule 3). A wrote that "the hold lifts while the group is still playing, so the release read's lines join the backlog before the backlog is looked at"; by the study's rule that is so only while fewer than 30 lines wait behind the group. In a flood whose reads each bring up to two screens, 30 or more wait when the group's last mark is reached, the hold has not lifted, and the look runs on lines read before the hold: the next group is stale, and the newest output is read only once fewer than 30 wait. B's sequence (`1..10000` in the console host, limits 30 and 30): today "line 1" to "line 30", "skipped 9940 lines", "line 9971" to "line 10000" (the study's own F3 sequence gives the same count); under the study a stale group of 30 lines follows the first skip. The exact lines need a capture or a model run. B's options: the look waits for a read sent at the group's last line (derived from what the queue holds, not a flag), or the stale group is accepted.

**Q10. F14 and F1 on a read that shares nothing (B; confirmed, B's outcome narrowed).** F14: "a read sharing no line with the last speaks only its screen's rows". F1's sequence: "'skipped lines' with no number when a read found nothing of the last". Both fire on the same read: the one after held reads, or after more than two screens scrolled past between reads. The study does not say which wins. If F14 wins, no skip phrase is said and the screen's rows follow the last group as if they came next (B's outcome); if F1 wins, "skipped lines" is said. Needs a decision and a capture.

**Q11. The outpost ending while lines wait (B; confirmed).** Today what was read is still spoken: b7038a48's message, "what Core had already read is still spoken, as NVDA's terminal monitor queues every line it read and an application's end cancels none of it (behaviors.py, LiveText._monitor and _reportNewText; appModuleHandler.cleanup)", and the test at `terminal/tests.rs` 1616. The study's pacing table has columns G to M (turn, marks, cut, toggle, commits) and none for the outpost or reader ending; its column L ("a commit of the same terminal not left: carried over") covers the replacement's re-take that follows, not the end itself.

**Q12. The desktop switch (B; confirmed in part, B's consequence contradicted).** Today `input_desktop.rs` 100 to 101 calls `committed(state, None, Vec::new(), now_us)` and then `state.focus.sleep()`. The study's rule 2 names one constructor and says "every commit ... replaces the watch", but not this path. B's consequence, that "a read still out answers and its lines join the queue while the secure desktop is up", does not follow: `sleep` sets the focus object to `None` (`focus/object.rs` 59 to 61, `self.object = None;`), so a watch held in that record goes with it, and a later answer matches no watch (the study's column D). What the study leaves undecided is whether paced lines waiting at the switch are dropped with the record. Today `committed` does not touch `state.terminal`, so whether waiting output is spoken over the secure desktop today needs a test.

**Q13. Each group handed to speech whole (A; confirmed as stated).** Today `AHEAD = 2` (`terminal.rs` 71), "two queued behind the one playing, so there is no gap between lines and the backlog stays here, where it can be trimmed", each utterance between an opening and a closing mark (1002 to 1005). The group itself is never trimmed (module documentation, 17 to 20: "the first "Lines spoken in full" lines of a burst of output are spoken whole, however fast the rest arrives"). A's proposal hands the group whole with one closing mark, about 130 lines fewer than the study's pacing and F1. The difference A names: with "report new output" turned off from the settings dialog mid-group, the rest of the group (up to 30 lines) is heard instead of at most two. How other speech queued behind a whole group interleaves was examined by neither reviewer; needs a capture.

**Q14. F3 kept as an outpost-held range (A).** A's 200-line F3 has the outpost keep one range at the last read's end between reads, against the study's "It keeps nothing between reads: no start anchor, no scope". F3 is not among the features Dickson named as kept, and the study proposes dropping it; keeping it at the least cost needs this departure. B's floor assumes F3 dropped, which needs his authorization.

## 5. Cost of one behaviour

From `git show --stat` (all checked):

- **A reading rule alone:** f0902400, 6 files, 78 insertions (`terminal_reading.rs` and its tests, the model document, the parity ledger, the roadmap); f4fb5828, 5 files, 65 insertions. Cheap because the rules are one crate, but each grows `handle` (P13).
- **A behaviour that needs a read or a command:** fad163c2 (moving by command, 2026-10-26), 34 files, 1,758 insertions: a new `TextOp::TerminalFind` and reply in `verbatim-model`, the outpost's worker and handler, a new remote operation (`terminal_find.rs`), the rules' memory, Core's model and review answers, mockapp, an end-to-end scenario and its registry entries, seven documents. Its review fixes, c291dbed, 16 files.
- **A cross-cutting change:** f171c7b3, 20 files; 89c36cc8 (the reading as Core's own requests, 2026-11-16), 55 files, 4,001 insertions and 2,424 deletions.

The shared mechanisms the reviewers propose, merged:

1. A terminal read as an ordinary text request decoded by `text_request::decode_text`, and review and marks on the generic review path over live ranges (Dickson's F11 and F12 directions): no `ReadTerminal`, `TerminalHistory` or `TerminalFind`, no codec arms in `raw.rs`, no handler or remote operation per command (both).
2. One type for a read and its text, used by the outpost, Core and `verbatim-text`: no mirror types, `read_of` or `snapshot_of` (both; P4).
3. One owner of the key window and of the setting (both; P5).
4. The watch and pacing in the committed focus's record, so there are no separate resets (A; the study's rule 2).
5. Terminal speech emitted only at its own entry's turn: no `speak_read` or `take_waiting_before` (A; the study's rule 5).
6. Documents that point at the tests instead of restating the rules, so a rule change edits one place instead of the model document, the parity ledger and three crate guides (B).

## 6. Dropped or corrected

No finding was dropped for a quotation not found: every quotation checked matched the code, its line numbers drifted by a few lines at most. Corrected or narrowed:

- A: NVDA's loop "about 800" lines; the files' ranges sum to 912, or 595 without the older consoles' TextInfo.
- A: `lines_of_one_and_two_characters_after_enter_are_output_not_echo` listed as changed by the rebuild; its read inserts three lines, and NVDA's drops apply only to one.
- A: "the pacing table holds with F2 derived from the waiting count"; it holds only while fewer than 30 lines wait behind the group (Q9).
- A: `deferred` "copies the strokes' hold"; it reads `state.strokes.holds()` (P6).
- A: "NVDA cannot reach this case: its `_getText` blocks and does not time out"; the pinned lines show no timeout of NVDA's own, and UI Automation's is not in them (Q3, Q5).
- B: the desktop switch "would leave the watch alive in a sleeping focus"; `sleep` drops the record (Q12).
- B: a failed baseline speaks "30 lines, skipped N lines, 30 lines"; two screens bring about 60 lines and no skip (Q4).
- B: the Tab example "Get-Ch"; it inserts seven characters and is spoken either way (Q8).
- B: the floor of 3,860 "with every feature kept"; it drops F3 (section 2.2).
- B: `full_read` at `terminal_model.rs` 233 to 244; it is `reading.rs` 227 to 243 (P10).
- B: `Hold::Word` without `grew_ms` "due at no time"; it is never due (P9).
- B: "no F code covers" the spinner; the study files it under F6, whose definition omits it (Q6).
- Both: the hand-off's size (A 1,800, B 600); neither counted `verbatim-process/src/message/raw.rs` (508 product, 409 test lines).
