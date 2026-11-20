# Verbatim simplification review: assessment, turnaround, findings and proposed builds

Written 2026-11-20, against main at ba89dca7 and NVDA pinned at 732d1ce2. Nothing in the repository was changed.
This report is as it was sent to Dickson on 2026-11-20, before his answers. His rulings, and what was done after them, are in [plan.md](plan.md).

How it was made. The workspace was divided into 19 areas. Two reviewers read each area independently. A third agent, here called the merger, then matched the two sets of notes and checked every finding against the code, the pinned NVDA source, the documents and git, dropping what it could not confirm. Everything below comes from the mergers' records. The mergers dropped or corrected about 180 reviewer claims, among them a quotation that is not in its file, a test that does not exist, and several proposals that contradicted a recorded decision. I checked the timeline's quotations, the key-press bridge, the two `cargo xtask` claims and the dead nightly workflow myself.

Three classes are used throughout:

- **Remove now**: no change to speech, to a decision of record or to a feature, and not in code a planned phase rewrites.
- **If you rule**: it changes speech, a feature or a decision of record, so it is a question.
- **Planned phase**: in code a named roadmap phase rewrites; recorded for that phase.

The records are in the folder beside this file, one `merged-<area>.md` per area. Each holds its size table with NVDA's files and line ranges, every finding with file, line and quoted code, the per-crate test counts, the questions in full with their sequences, and the list of dropped claims.

## 1. My honest assessment

**At today's cost Verbatim will not be finished.** That is not a matter of pace. The cost of each behaviour is too high, and nothing in how the project is built pushes it down.

The evidence:

- **A behaviour costs 20 to 40 times what it costs NVDA.** Holding Enter took 1,294 lines in 35 files against NVDA's 26 to 31. Control+Backspace saying the whole word took 633 lines in 28 files against 20 to 30. One listener setting took 1,867 lines in 52 files. The dialog read once shown took 4,931 lines in 67 files.
- **The code is being rewritten faster than it is being extended.** Since 2026-10-20, 449,000 lines of Rust were inserted and 268,000 deleted, in a codebase of 379,000. ADR 0003 itself counts 519 defect fixes in the month before it.
- **The safety net is large and does not hold.** There are 142,000 lines of tests, yet this review found 22 defects by reading the code. 121 tests go through test support that carries its own copy of the listener's logic, 62 of them through a copy of its delivery-time check, so none of them can fail on the real one. Of 105 scenarios, 4 cite an NVDA capture that still exists. The exact call counts changed in 92 commits in a month and caught nothing. No source scan is credited with catching a defect in that time.
- **Most of NVDA is still ahead.** Browse mode, the application modules, braille and the rest of the roadmap are where most of NVDA's 188,000 lines are. Verbatim already has 237,000 lines for a fraction of it.

**Why it is so.** Five causes, in the order I would weigh them:

1. **Every behaviour that crosses a process boundary builds its own crossing.** ADR 0003 decided that it would not: one request list, properties declared once, "a new command adds no kind of request". The builds did not do it. 23 request kinds, text operations and replies serve one behaviour each, and the property table's per-interface columns are read by no reader. Where code sits on one side of a boundary it is already near NVDA's size: the accessibility API crates are 1.24 times NVDA and the settings frame 1.2; by the reviewers' counts say-all is 1.1 to 1.2 and the focus validity rows 2.0. The overruns are all at the crossings: keys 3.1, focus 3.9, the listener 5.0, the queue 5.9, the reader lifecycle 19.
2. **Verbatim is trying to be better than NVDA before it is as complete as NVDA.** The worst ratios are things NVDA does not do: terminal reading at 15 times with fourteen features beyond NVDA's loop; progress bars at 24 times; the dialog wait; the plain reader for hung windows; thirteen ways for a request to end. Each was a reasonable ruling. Together they are where most of the code, the defects and the review rounds went, and each has no NVDA answer to copy, so each generates decisions for you.
3. **The focus path has been rebuilt repeatedly since mid-October** (the focus pipeline rework's three builds, the ownership refactor, "Readers tell, Core asks", then ADR 0004), and each rebuild left its remains: a bridge that converts key presses back into the older inputs, a protocol message the ADR removed, 66 identifiers in one guide that no longer exist, four built design files still serving as the rule store. Another re-architecture would most likely do the same.
4. **The process adds and never subtracts, and I am part of that.** Agents produce code, tests, documents and rules cheaply. Each incident got a rule, a test level and a document, and nothing was taken away. You named the pattern yourself on 2026-11-10. One behaviour now pays for 11 to 15 documents, a dozen runs of the checks and three rounds of review, and the reviews find edge cases that are then handled by adding state.
5. **The project produces more decisions than one person can take.** This review alone raised about seventy questions. That is a finding, not a deliverable.

**What is sound.** The layers below the crossings are healthy. The model in ADR 0003 and ADR 0004 is coherent: most of what this review found is code that departs from it, not a flaw in it. The defects are local and fixable. Where the code followed ADR 0004's shape (one definition, readers report facts, one owner decides) it came out at twice NVDA's size, not five times.

**So: I think it can be turned round, but not by removal and not by another rewrite.** Removal with no ruling needed is about 10,000 product lines and 11,000 test lines, around 4 and 8 percent. With every question answered towards removal and the terminal rebuild done, it is about 31,000 and 44,000, 13 and 31 percent. That makes the codebase more honest. It does not make the next behaviour cheap.

**What I cannot promise.** 1.3 times NVDA for the whole is not reachable while readers are separate processes: the plumbing they need has no counterpart in NVDA. I think 1.3 times is reachable for a behaviour that stays on one side of a boundary, and two to three times for one that crosses, once the crossing is generic. That is an estimate. Section 2 proposes measuring it on two behaviours before anything else is built on it. If it does not hold, the question that remains is the process layout itself (D9 and D13). I am not raising that now: three attempts to move judging out of Core failed review, and the evidence here points at how behaviours cross the boundary, not at the boundary.

## 2. How to turn it round

Five moves, in order. The builds in section 8 carry them out.

1. **Stop adding until the cost is measured down.** No new behaviour and no new feature beyond NVDA until moves 2 to 5 are done. Where NVDA has an answer, take it after a capture, with no question to you; that is already your rule. Where Verbatim would go beyond NVDA, the default becomes "not yet".
2. **Fix what is broken and delete what is dead.** The 22 defects in section 5, and the removals that need no ruling. This is low risk and makes every later step smaller.
3. **Make crossing a boundary generic, as ADR 0003 decided.** A closed list of operations on objects and text. A request is a list of them. One program builder turns such a list into a remote operation and one executor runs it as ordinary calls, so the call-by-call twins stop being written by hand. One object abstraction per backend, so a ported walk such as a dialog's text is written once and not once per backend. Readers report facts; Core judges. A behaviour then becomes a function in Core. This is a design study first, and its premise, the cost of the round trips, is measured before anything is built.
4. **Make the safety net real and small.** Each behaviour is tested once, at the cheapest level that runs the real code: for most, a recording replayed through Core against captured NVDA speech. Captures are kept in git beside what cites them. Figures live in tests and documents point at them. Each rule is written in one document. The standing rules are cut to the ones that have prevented a defect.
5. **Prove it on two behaviours before resuming the roadmap.** The dialog read once shown, rebuilt at about 400 product and 340 test lines against today's 1,560 and 2,190. Then one ordinary behaviour that crosses a boundary, built again on the generic crossing, with its lines counted against NVDA's. If the second is not within about three times NVDA, stop and look again before M5.

After that, a budget becomes a standing check: a behaviour whose size passes a stated multiple of NVDA's stops for a look at its shape before it is finished, as the third review round of the dialog wait should have.

## 3. What I need from you

Four decisions. Everything else I will decide by the defaults at the end of this section and report afterwards.

**Decision 1. The strategy.** Keep the process layout and the model of ADR 0003 and ADR 0004; finish the generic crossing that ADR 0003 decided; freeze new behaviour until the two proofs of move 5 are in. Yes or no.

**Decision 2. Parity before improvement.** Behaviour beyond NVDA is deferred until parity, except what you name. I would keep the frozen-window handling, since it is the reason for the layout, and I would put these three to you now:

- **Progress bars** (1,370 lines against NVDA's 58): Core asks for a bar's value outside the event queue, as terminal output reads already are, and the listener's own judging goes. About 1,050 product and 1,400 test lines. Same sounds in the two scenarios that exist.
- **The terminal rebuild.** With every feature you ruled kept on 2026-11-19 it comes to about 3,300 lines, 3.6 times NVDA, not the study's 1,270. The largest single item is the exact count in "skipped N lines" with the row model behind it: about 3,500 lines today, about 200 if the outpost holds one range, nothing if the count is dropped. Tell me whether the exact count stays, and whether 3,300 is acceptable.
- **The plain reader for hung windows** (522 product and 1,456 test lines): I would keep it as ruled.

**Decision 3. The dialog read once shown.** Rebuild it by postponement: a focus read that meets a hidden dialog ends its entry; Core keeps one fact; the listener tells the foreground window's show as an ordinary event; Core reads the focus at that event's turn. It keeps today's words in every sequence a test states. It changes three things, none stated by any test or capture today:

- a modifier pressed alone while the dialog is hidden: today nothing is said for the dialog; it would be said with its text when shown;
- speech from a background application's event would come before the dialog, where today it follows;
- a hang while hidden plays no "not responding" sound until something reads the window.

It also stops holding keys for 13 seconds behind a dialog that is never shown. It changes two sentences of ADR 0003 that you ruled on 2026-11-19; they are quoted in section 9. The alternative, a short wait inside the outpost, is smaller but breaks your ruled option C in two sequences and ties the Import dialog's speech to the machine's speed, so I do not recommend it.

**Decision 4. The working process.** Let me bring you one proposed revision of `CLAUDE.md`, `REVIEW.md` and `testing.md` to approve or reject as a whole: the rules that contradict each other resolved, the ones that name no defect proposed for removal as your rule of 2026-11-10 asks, captures kept in git, the performance ledger pointing at its tests, and one level of test per behaviour.

**What I will decide myself unless you object.** Section 9 lists them all; the defaults are:

- Where NVDA has an answer, match it after a capture. That settles about fifteen of the questions.
- Where two decisions of record disagree, the later one governs, and I correct the earlier document. That settles the backend verdict's owner: ADR 0004 of 2026-11-18 says Core's cache, so the outpost's keep goes.
- Dead code, dead protocol and unreachable fallbacks go. The call-by-call twins go if a count of the fallbacks in the GitHub runs' logs shows that none occurs outside the developer setting.
- Tooling follows the evidence: `cargo xtask ci` reports every failure in one run; source files check out with one line ending; the dead nightly workflow is deleted.

**The Hyper-V harness.** It goes, as you said: `xtask/src/vm` (2,478 lines and 268 of tests), the Packer and answer files, about 700 lines of PowerShell, `vm-smoke.yml`, and the VM guide. D3 and section 14 of the architecture document are amended to say so, dated today as your decision.

Driving a Verbatim in another machine survives that at almost no cost, because it never depended on the harness. The agent already listens on a network address, and the suite already takes the agent's address from `VERBATIM_E2E_ENDPOINT`; GitHub's runs use the same path over loopback on every push, so it cannot rot unnoticed. What I would add is small: an endpoint option on `cargo xtask e2e` so that it uses an agent already running elsewhere and starts none of its own, and a short guide section on copying a build into a VM and starting the agent there. What is lost is only the automation that built, restored and deployed to a Hyper-V image.

One thing I found while checking this: the agent binds every network interface by default and asks for no credential, and it can start programs and send keys. That default existed for the Hyper-V guest. With the harness gone I will make loopback the default and require an explicit address for a VM, to be used only on an isolated virtual switch.

## 4. Your three lead questions

### The event queue's head-of-line hold

Holding the queue behind a read is not the cost. NVDA's main thread blocks inside each call, so it holds everything behind a read too. The verified table of the queue's 21 waiting states (`merged-core-queue.md`, its own section 4) shows where the cost is:

- NVDA has a full counterpart for 8 of the 21, a partial one for 3 and none for 10.
- A request that is out has 13 ways to end, costing about 600 lines and about 47 tests. NVDA has three of them in part. The rest are your "frozen window" rulings and the consequences of separate processes.
- The dialog-shown wait is the only state in which something at the queue's front waits for an event that is not an answer. Everything else that waits for an event or a time already ends its entry and joins the queue again later: the check after a menu closes, the return from another desktop, the lock keys, say-all's next read.

So the rule is already the queue's own, with one exception: an entry waits only for calls into applications, and an event arrives as its own entry. Making the dialog follow it removes the endings, the carried mark and three hooks.

The queue's trait began with 4 hooks on 2026-11-16 and has 32 now. Fourteen serve exactly one feature. Four separately judge whether the front's request still stands; they become one judge with no ruling needed.

### One fact spread over three processes

The shared mechanism already exists: the tell. The listener's hook hears every window's show. Telling Core of the foreground window's show, as an ordinary event judged at its turn, needs no request kind, waiter table, cancel path or answer token. The comparison of the two rebuilds (`merged-dialog-study.md`) checked this against the code; the hung-window recovery already works this way.

The wider answer is the protocol. ADR 0003 decided "one request list ... A new command adds no kind of request". As built, that holds only for review and navigation. `merged-model-protocol.md` traces three behaviours and lays out, for each, the shape of request that would have needed no change to the protocol: a focus read that reports the window's visibility as a fact; a starting read that takes a list of the generic text reads; reads that return fresh values with Core choosing between them and the event's.

### Tests sized to state tables

Core has 46,000 lines of tests for 31,000 of product code. What the mergers found is less that there are too many tests of each rule than that the same thing is tested at several levels, and that some levels do not run the real code:

- **Tests that run a copy.** The mockapp test support holds its own copy of the listener's delivery-time check, bar routing, limiter and tell construction. 121 tests in that crate go through it, 62 through the copy of the check. The listener's real check has no unit test, and two concrete mutations of it would pass them all.
- **Duplicated harnesses and lists.** Two Core test harnesses share 343 of 482 distinct lines. The scenario list is written three times and held in step by checks. Each mockapp test is named twice.
- **Figures pinned exactly that catch nothing.** `call_counts.rs` changed in 92 commits in a month: 33 moved figures for an intended change, 24 added pins, 32 moved no figure, 3 fixed flaky tests, and none caught an unintended call.
- **Generated tests whose model reads the code under test**: the key-holding walk and the listener's foreground watch. The terminal simulator's oracle (about 4,600 lines) covers no ordering across a hand-off.
- **Expectations without evidence.** Of 105 live scenarios, 4 cite a capture file that exists. Three commits last month changed an expectation to Verbatim's own output with no capture named (e48e06ab, 50b1d527, 472833ff).

## 5. Defects confirmed

Each was traced step by step by a merger. None is fixed. "No speech change" means the fix changes no words in any known sequence.

| Defect | Where | Effect |
| --- | --- | --- |
| Application text can issue commands to eSpeak NG | `verbatim-synth-espeak`, both text paths | U+0001 followed by a command letter changes the voice or punctuation setting; NVDA strips it (`espeak.py` 104 to 113) |
| A settings file that fails to load is overwritten with defaults | `verbatim-app/src/main.rs` 268 and 578, the saver | One typo, then any saved change, erases every setting with no backup; NVDA keeps `.corrupted.bak` and shows a message |
| A stalled reducer ends every outpost | `verbatim-app/src/main.rs` 2041 to 2052 | After a stall of 9 seconds Core ends outposts before reading the pongs queued behind; the test named for it cannot fail on it |
| A control goes unspoken after an outpost is replaced | the committed focus kept in two records, `focus/object.rs` and `versions.rs` | Notepad's outpost is replaced, the focus re-taken; the next Tab's "Find Next" is not spoken, and nothing is logged |
| A mouse return to a list item is silenced | the "failed try ends the fence" rule at nine sites | The UI Automation read paths leave the fence where ADR 0004 says it ends |
| A foreground window's tell arrives out of step with its report | the listener's hold and watch keep the same window | Whether the window is spoken depends on queue timing; a second foreground event can lose the report |
| The outpost reports a probe it did not make | `asked.rs` 295 to 309; the verdict has two keepers | A provider appearing 0.55 s after a "no" is read through MSAA until 1.1 s; NVDA uses it from 0.6 s |
| The outpost's caret elements are swept at 64 entries or 60 s | the sweep your 2026-11-19 ruling replaced for the listener | A burst of 65 caret tells loses the token Core's caret read needs |
| Flat review differs from NVDA at start of line, end of line and current line | a second review implementation | Recorded in the parity ledger; the M4.2 item ordering the fix is marked built |
| Backspace at a line's start names a remembered line break | Core's cached break against the provider's read | Two match arms can never run; needs a capture |
| A "same element" check never matches | `completion.rs` 358 to 362 | One wasted call into the application per check; no speech change |
| An utterance whose post is refused never ends | `manager.rs` 478 and 487 | A say-all whose last utterance is lost never finishes; needs an out-of-memory system |
| The control plane's startup speech history never closes without a client | `verbatim-control` server 1073 to 1081 | Every utterance copies its text and takes two locks; no speech change |
| Core's state holds an empty queue while a step runs | `state.rs` 617, `entries.rs` 1075 | Terminal review commands miss the positions of live requests; no spoken difference shown yet |
| Five scenarios carry inline copies of fixture files, one drifted | the twin-item scenario against `list_view_twins.json` | The stand-in checked against a recording is not the one the scenario runs |
| `cargo xtask e2e` passes a misspelled scenario name | `tests/e2e.rs` 284, `local_e2e.rs` | Zero tests run and the command reports a pass |
| `cargo xtask measure` exits successfully when a scenario failed, and runs 7 of 15 measurements | `xtask/src/perf.rs` | Found by the merger while checking the item above |
| The check that fails a run on a logged defect misses two defects | `artifacts.rs` 468 to 492 | Two of its 23 copied messages were reworded, one 23 minutes after the check went in |
| A failed focus read is taken as "the window has the focus" | `verbatim-agent` `focus.rs` 267 | Every scenario's startup check rests on this read |
| A frozen screen capture passes the recording check | `capture.rs` 229 to 251 | The video shows a stale screen with no log |
| NVDA transcript replies read missing fields as empty | `verbatim-e2e` `nvda.rs` | A capture reads as "NVDA said nothing", or doubles NVDA's speech |
| The nightly Explore workflow has run zero tests since 2026-11-17 | `.github/workflows/explore.yml` 64 | It passes every night; its mutation groups name five deleted files |

A hidden window's title is localized while a second launch looks for a fixed title; it cannot be reached today because only the English strings ship.

## 6. Sizes against NVDA, and deletion targets

Product and test lines are Verbatim's. Ratios are product code against the pinned NVDA files each record names. Targets are product lines and test lines, written "product / tests".

| Area | Product | Tests | Ratio | Remove now | If you rule | Planned phase |
| --- | --- | --- | --- | --- | --- | --- |
| Core's event queue | 4,956 | 5,108 | 5.9 | 680 / 1,000 | 340 / 260 | 180 / 0 |
| Core's focus rules | 5,743 | 10,825 | 3.9 | 620 / 270 | 110 / 135 | none |
| Keys, announcements, input | 9,104 | 12,278 | 3.1 | 970 / 425 | 225 / 510 | 390 / 160 |
| Text reading and editing | 15,090 | 10,900 | 2.1 | 500 / 500 | 1,310 / 700 | 40 / 0 |
| Terminal reading | 13,970 | 22,600 | 15.3 | 230 / 220 | see section 9 | 9,900 to 10,700 / about 19,000 |
| The listener | 9,445 | 6,413 | 5.0 | 520 / 765 | 1,180 / 1,400 | 120 / 80 |
| The outposts | 14,161 | 6,977 | 4.1 on rows NVDA has | 700 / 430 | 450 to 600 / 2,600 | 70 / 300 |
| The accessibility API crates | 23,160 | 5,998 | 1.24 | 1,600 / 470 | 100 / 120 | 200 / 0 |
| Model, protocol, transport | 19,712 | 4,593 | 1.8 for the vocabulary | 800 / 630 | 1,380 / 265 | 600 / 0 |
| Supervisor and app shell | 12,080 | 7,453 | 19 for the lifecycle | 430 / 700 | 410 / 900 | 250 / 100 |
| Speech, audio, synthesizers | 12,463 | 6,951 | 1.80 | 300 / 190 | 240 / 335 | 500 / 1,500 |
| Settings, GUI, localization | 10,130 | 3,010 | 1.6 | 500 / 560 | 60 / 40 | 290 / 215 |
| Mockapp and its tests | 11,367 | 16,040 | no counterpart | 920 / 1,450 | up to 359 / 42 | none |
| Call-count and remote-operation tests | none | 11,428 | no counterpart | 165 / 1,160 | 0 / 400 | 30 / 0 |
| End-to-end harness and agent | 17,082 | 4,532 | 6.4 against NVDA's test libraries | 1,700 / 560 | 1,500 / 650 | 120 / 250 |
| End-to-end scenarios | none | 20,711 | 2.5 against NVDA's system tests | 0 / 1,900 | 0 / 1,120 | 0 / 90 |
| xtask and tools | 17,422 | 3,241 | 1.5 for checks and CI | 415 / 80 | 245 / 140, or 2,720 / 410 with the VM harness | none |

Notes on the table:

- The dialog-shown rebuild is separate: about 1,160 product lines, 394 of mockapp and 1,850 of tests go under the recommended option.
- Some removals are counted in two areas (the key-press bridge in the queue and in keys; one test harness in the queue and in focus). The totals in section 1 allow for that.
- The documents are in section 7. Their target is in document lines: about 3,050 with the phase's clearing-up step and up to 1,450 more if you rule. `uia-facts` and `experiments` are there too: about 135 lines of dead Rust and 1.1 MB of uncited files now, and 1,647 lines of fact programs if you rule.
- The lifecycle's ratio of 19 is 5,200 lines against NVDA's 268; 1,170 of them are the process boundary itself.
- The terminal rebuild's floor with every kept feature is 3,300 to 4,060 product lines, 3.6 to 4.5 times NVDA, not the design study's 1,270.

## 7. Findings by theme

Each area's record has the full list. These are the ones that recur across areas or are worth most.

### One fact, several owners

| Fact | Copies | Consequence |
| --- | --- | --- |
| The committed focus | the focus context and the versions' kept record, joined by different rules | a confirmed defect; three checks exist only to make them agree |
| The window in front | two inputs from one report, two records in Core; four types on the way | the two records disagree for a ghost window; the extra input carries a rule ADR 0003 forbids |
| A window's backend verdict | Core's cache, the outpost's keep, and three judges inside the listener | a confirmed defect; two ADRs disagree on the owner |
| NVDA's class lists | Core's `backend.rs` and `verbatim-reader-rules` | already drifted: Core's Windows Forms parsing differs from NVDA's pattern |
| One outpost generation's stage | Core's record, the owner thread, the app's live map, the status map | your 2026-11-15 ruling makes Core the only judge; one of the app's drops can never fire |
| The listener's waiting foreground | the hold and the watch, on two threads | a confirmed defect |
| A gesture's meaning | the hook's map and the app's tables | a silent default on a miss |
| The scenario list | the registry, the test binary, the CI planner reading source as text | three agreement checks |
| NVDA's presentable-ancestor rule | half in the UI Automation reader, half in Core | a question; needs a capture |

### Written twice or more

- **Remote programs and their call-by-call twins.** A twin, here, is the same read written as ordinary calls beside the remote program. There are 14 entry points behind 13 dispatchers with five different fallback rules; the crate guide's rule is contradicted by five of them. Text reads have a third copy in the outpost. No ADR or roadmap entry decides the twins, no document names an application whose remote program failed, and no test drives that fallback. The listener's focus walk is call-by-call by decision and stays.
- **Thirty-one remote-operation builder methods no program emits**, with their instructions, encode arms and table rows: about 620 product and 214 test lines.
- **The key-press bridge.** Presses are converted back into the reducer's older inputs: about 370 lines, with arms no code reaches and five comments naming a module that does not exist. The table tying speech to a key is not dead: it still stamps the key number on Report time's clock speech and lock-key speech.
- **Twelve request types of one shape in Core** (about 200 lines saved), and 55 hand-written agent client methods (about 900).
- **Dead protocol.** A "not responding" message ADR 0003 removed; caret change types only ever filled with defaults; 12 of 16 latency timing fields written only in tests, so 19 of 30 latency stages can never be produced.
- **Dead code behind blanket allowances.** Three modules carry a module-wide dead-code expectation whose stated reason is stale. `focus_rules.rs` is mostly unused (about 185 lines).

### Readers judging what ADR 0003 gives to Core

- **Progress bars** (about 1,370 lines against NVDA's 58): the listener judges acceptance, the backend, whether an object is a bar, a flood limit that guesses the focus, and its own deadline. Two of the five are exceptions ADR 0003 records.
- **The listener's merge of waiting tells** has a different reach from Core's limiter. You reaffirmed it the day it was measured, so no change is proposed.
- **The outpost's verdict keep**, above.

### The terminal rebuild's study

`merged-terminals.md` checks `docs/design/terminal-rebuild.md` against the code:

- Today's size is 13,970 product lines, not 11,970: the study leaves out the colour read, a dead operation, the outpost's handlers, the wire types and a 508-line codec.
- The study would change what is spoken in eight places it does not list as questions: blank lines counted in the flood (your ruling of 2026-10-07); one-character output; output written during a hang; a failed baseline; the spinner; redraws; NVDA's Tab exemption; a read past its due time.
- Four cells of its state tables are wrong or undecided. A read that times out loses the change it was sent for.
- Its inventory misses Core's caret record, which every terminal read feeds.
- No terminal test compares against captured NVDA speech. The smallest set that fails on this area's recorded defects is eight to ten Core tests, 320 to 400 lines.

### Tests

Per-crate counts of tests and lines removable, replaceable and to add are in section 3 of each record. The totals that need no ruling:

| Crate or group | Removable or replaceable test lines | Largest items |
| --- | --- | --- |
| verbatim-core | about 2,200 | one of two test harnesses (about 600); the merge tests that compare the code with itself; duplicated say-all and review tests |
| mockapp | about 2,600 | the listener copy; registration lists (about 650); dead helpers; per-method provider counts (about 624) |
| verbatim-listener and its logic | about 765 | the foreground watch's generated test; a source-scanning test; the check's step tests |
| verbatim-outpost and its logic | about 430 | tests of tags that decide nothing; duplicates of reducer-level tests |
| verbatim-supervisor | about 700 | nine near-identical stand-in children; duplicate binaries |
| verbatim-e2e and verbatim-agent | about 2,460 | registry entries of defaults (about 900); the second scenario list; serde round trips |
| verbatim-model, control, process | about 630 | tests of dead tables; round trips of derived code |
| the rest | about 1,400 | restated tables in the API crates; settings and localization tests of Fluent text |

Tests to add are few, about 25 in all, nearly all for the defects in section 5.

### Process

From yesterday's transcript (`dialog-session-timeline.md`) and the tools record:

- The dialog work took 3 hours 25 minutes from your choice to the last GitHub run being read. About 50 to 70 minutes of that waited on your answers.
- The builder ran `cargo xtask ci` 12 times, 7 failing, each failure exposing the next check. The command stops at its first failing step; only one step depends on another.
- Three design review rounds each found new must-fixes, every one about a way the wait could end. That was the design telling us its shape was wrong.
- Captures are remade because none is kept: scenarios cite 13 folders under a path git ignores, and two exist.
- Five pushes failed on line endings last week because GitHub's runners check out CRLF and local checkouts use LF; `.gitattributes` sets nothing for source files.
- In the month, no commit credits the tier, waits, shapes, ownership or documentation scans with catching a defect. Sixteen commits were made only to satisfy a check, 12 of them Clippy.

### Documents

From `merged-documents.md`:

- 47,899 lines of Markdown, 8.2 times NVDA's developer documents by lines and 12.8 times by words. The crate guides are 14,091 of them; NVDA keeps the same matter in docstrings.
- The four "Readers tell, Core asks" design files are built but still the home of live rules: 183 code comments cite them, 140 the first, and 57 name a numbered table. No explanation page holds the first file's 18 state tables. Line 3 of each still says "Nothing in it is built."
- The roadmap, the published homepage and the documents index say the phase is unbuilt, and the roadmap's list of confirmed defects names removed code as still live.
- `verbatim-core.md` names 66 identifiers and 5 files that no longer exist.
- The dialog wait's ending rule is written in 9 places, 3 in code and 6 in documents.
- The performance ledger copies pinned counts by its own policy, and two entries are wrong against their tests. Changing the policy touches eleven sentences across the ledger, `CLAUDE.md`, `REVIEW.md` and the port skill.
- The standing rules: 15 of 32 name no incident, and five pairs contradict each other, among them the rule against local runs of a family or the full suite and the Commands section that allows both.

### `uia-facts` and `experiments`

From `merged-uia-facts.md`:

- `uia-facts` is 18,068 lines of Rust in 32 fact programs with their support and provider. No fact program runs routinely; the workflow builds and lints them on 242 of the 628 commits since 2026-11-07, 106 of those only because `verbatim-core` is in its path filter, though that crate has not been in the build since 2026-11-17.
- About 135 lines of dead support and provider code.
- In `experiments/`, 1.1 MB and 4,144 lines of code that nothing cites, most of it measurements from the focus rework that ADR 0003 replaced.
- Five fact programs and three guides whose own READMEs say nothing in Verbatim rests on them (1,647 lines). Four of the five run their cases by default, which the folder's own rule forbids.
- One README contradicts your decision 11 of 2026-11-07, and `uia-bugs.md` cites 12 numbered sections that do not exist.

## 8. Proposed builds, in order

Each build gets one design with its state tables and an inventory of what the replaced code does and says, one adversarial review, and its scenarios found from the code. One state-changing build is in flight at a time. The harness and document builds change no product state and can run beside the others.

1. **Defects and tooling that need no ruling.** The eSpeak NG escape; the settings file's backup; the reducer's loop order; the refused post; the control plane's history; the comparison that never matches; `cargo xtask e2e`'s empty-selection guard and `measure`'s exit code; `cargo xtask ci` reporting every failure; one marker for logged defects; the agent's failed focus read, frozen capture and transcript fields. Each with the test that would have caught it. Also the Hyper-V harness's removal as you ruled, with an endpoint option on `cargo xtask e2e`, loopback as the agent's default address, and D3 and section 14 amended; the line-endings setting; and the dead nightly workflow's deletion.
2. **The dialog read once shown, rebuilt.** The test of this review. Scope and removal in question 1. Its design starts with the captures listed there. It also takes the four stop hooks down to one judge, since that is the same code.
3. **Core: one owner per fact, dead paths out.** One committed focus record and one fence rule (two defects); one foreground record; the key-press bridge; `focus_rules.rs`; the class lists moved to one place; the twelve request types; dead protocol in the model; the app's copies of the generation record. About 3,000 product and 2,500 test lines. The verdict's owner (question 3) decides part of it.
4. **The readers.** The listener's one foreground state (a defect; question on the timer's thread); the check as a straight-line function, tested directly and used by mockapp's support in place of the copy; the outpost's dead tags and placeholder snapshots; the caret element sweep; the false probe report; progress bars as you rule. About 1,200 to 2,400 product lines.
5. **Remote operations and text.** One dispatcher with one rule; the dead builder methods; the duplicate walks; the twins as you rule; flat review through the text path and the Backspace fix, each after its capture; the dead text protocol. About 2,500 to 3,700 product lines.
6. **The tests and the harness** (beside the others). One scenario list and a registry constructor; typed agent requests; the registration macro; one Core harness; captures kept as you rule; the allowances and the testing standard brought back in step; the per-method provider counts dropped and the ledger pointing at its tests as you rule.
7. **The documents** (beside the others). The built designs split into explanation pages and decision records and deleted, with code comments repointed; crate guides pointing at module documentation; stale statements corrected; the roadmap's status as you approve.
8. **The request list closed.** A design study first, during builds 3 to 5: the generic operations, a request as a list of them, what each present one-behaviour kind becomes, and the measured cost of the added round trips. Built before M5's build B, which shares the list with extensions.
9. **The terminal rebuild (M6.4)**, from a corrected study, at the size you choose in question 6.

Planned phases keep their findings: M6.2 (the owner thread's generation record), M6.3 (the speech queue's three records of an utterance), M4.3 (a table of setting rows), M5 (the gesture tables).

## 9. The questions in full

You asked for fewer questions, so only the four decisions of section 3 need you. This section keeps every question the review raised, so that nothing is decided out of your sight: question 1 is decision 3, question 2 is decision 1, questions 4 and 6 are decision 2, and question 7 becomes the one revision of decision 4; questions 3, 5 and 8 I will settle by the defaults of section 3 unless you object. The full form of each, with its sequence, the speech under each option where a test or capture states it, and NVDA's source lines, is in the record named. Where a line says "needs a capture", no speech is stated anywhere and I have written none.

### Question 1. The dialog read once shown (`merged-dialog-study.md`)

The decisions it changes, ADR 0003: "The listener has one kind of request waiting on an event, which Core can cancel: 'tell me when this window is shown', kept on its WinEvent thread from the request to its one answer (Dickson, 2026-11-19)"; and under "Limits accepted", "so does a hidden dialog that is never shown, with no key pressed and no foreground, menu or focus elsewhere (Dickson, 2026-11-19)". The fact that has changed: the built size, about 1,560 product lines and 2,190 test lines, of which the behaviour itself needs 100 to 165.

| Option | Product lines | Test lines | What it costs |
| --- | --- | --- | --- |
| Today | 1,560 to 1,590, and 394 in mockapp | 2,190, and a 425-line fixture | keys held up to 13 s behind a dialog never shown |
| Postponement | 380 to 400 | about 340 | three speech differences, below |
| A bounded wait parked in the outpost | 280 to 360 | about 330 | breaks your option C twice; silences caret keys held past 150 ms; ties Import's speech to machine speed |
| The same with the worker waiting inline | 120 to 170 | about 250 | the above, and blocks the application's one worker |

Postponement means: a focus read that meets a hidden dialog ends its entry; Core keeps one fact, the postponed dialog's window; the listener tells the foreground window's show as an ordinary event; at that tell's turn Core reads the current focus. A key at its turn reads the dialog hidden first, its speech carrying the postponed entry's older fence, so a typed key is heard alone as today.

It keeps today's words in every sequence a test states. It differs in three, none stated by any test or capture:

- A modifier pressed alone while hidden: today nothing is said for the dialog; under postponement it is said with its text at the show.
- An entry from a background application that speaks is said before the dialog, where today it follows it.
- A hang while hidden plays no "not responding" sound until something reads the window.

**Recommendation:** postponement, with the listener telling only the foreground window's show. Before the build: NVDA captures of a typed key, a caret key, a modifier alone and Tab while a dialog is hidden, a tray menu over a hidden window, a message box from a hidden dialog, and a dialog never shown; and one reducer test of whether speech made inside a key's entry is fenced by that key's cut. Neither rebuild reaches 1.3 times NVDA, which has no code that waits; about 400 lines is the floor for today's speech.

### Question 2. The request list (`merged-model-protocol.md`)

The decision, ADR 0003, "Properties, event kinds and requests": properties and event kinds "are each declared once ... the cached request and the remote program are built from that set ... and the compiler checks that a rule uses only what its read asked for"; and "One request list is shared by Core's own features and the extension surface ... A new command adds no kind of request."

What is built: the table's per-interface columns and its property sets are read by no product code; eight hand-written mappers do the work; six requests, nine text operations and eight replies each serve one behaviour.

- **Option A:** cut the tables to what is read (about 1,050 lines) and accept a request kind per behaviour.
- **Option B:** build the decision. Readers derive from the table; a request is a list of generic operations; a behaviour is composed in Core and adds no kind.

**Recommendation:** B, as the study in build 8, with the unread columns cut now whichever way you rule. The fact to measure first is the cost of the added round trips, since your instruction of 2026-11-13 was to minimise Core's traffic; a request that is a list keeps it to one.

### Question 3. A window's backend verdict (`merged-outpost.md`, Q1)

ADR 0001: "Each window has one backend verdict, its outpost's". ADR 0004: "Core's backend cache is the one owner of a window's verdict". The architecture document sides with ADR 0004 in two places and states the outpost's lifetime keep in a third.

- Keep the outpost's keep and fix only the false report.
- Core's cache as the only lasting keep, the outpost keeping a verdict only within one request: about 210 product and 285 test lines go, at one probe per crossed window per request for windows Core is never told of.
- Keep the outpost's keep by NVDA's rule, half a second for yes and no alike.

**Recommendation:** the second, with ADR 0001's sentence amended and the listener's three judges taken into the same ruling. The speech in the sequence needs a capture.

### Question 4. Progress bars (`merged-listener.md`, 1)

ADR 0003, "Progress bars (Dickson, 2026-11-14)", stands as the one place a reader reads unasked. New since: the acceptance gate and configuration channel of 2026-11-19 (ccdb2a7a, 1,867 lines). The path is about 1,370 product and 1,650 test lines against NVDA's 58.

- Keep.
- Core asks the listener for a bar at its value change's turn: about 1,050 product and 1,400 test lines go; one listener round trip at the front of Core's queue per accepted bar change, ahead of the keys behind it.
- Keep the listener's ownership but drop the role memo and the bar's own deadline: about 400 lines.

Speech is the same under each for the two scenarios that exist; an old console's bar needs a capture.

**Recommendation:** Core asks, but outside the event queue, as terminal output reads already are, so that no key waits behind a bar. That variant was not examined by the merger; I would settle it in build 4's design and bring you the measured cost before building.

### Question 5. The call-by-call twins (`merged-access-apis.md` Q2, `merged-text.md` 1)

Keep a twin only where a design requires one (the listener's focus walk, the size-limit paths), with a failed program answered as a failed read? About 1,100 product lines go, and the outpost's own text path reads windows where a program fails; formatting there needs two added reads or is lost.

**Recommendation:** measure first. Nothing records how often a program falls back; the fallback is already logged at warning level, so one count over the GitHub runs' logs answers it. If the count is zero outside the developer setting, remove the twins and keep the setting on the outpost's path.

### Question 6. The terminal rebuild's size (`merged-terminals.md`, sections 2.2 and 4)

With every feature you ruled kept on 2026-11-19, the floor is 3,300 to 4,060 product lines against a target of about 1,190. The three features that decide it:

- **Exact skip counts and the row model:** about 3,500 lines today; about 200 as a range the outpost holds, which breaks the study's "the outpost keeps nothing between reads"; or dropped, where "skipped lines" is said with no count.
- **Keys owning the changes they cause:** about 2,500 lines today with the colour read; 700 to 1,200 rebuilt.
- **The review cursor over the terminal's text:** about 1,400 today; about 200 as a position counted from the document's end.

**Recommendation:** accept a rebuilt size of about 3,300 with every feature kept, and take the fourteen speech and state-table questions in that record as the first step of M6.4's design, on a corrected study, not now. The one thing worth deciding now is whether the exact count stays, since it is the largest single item.

### Question 7. Testing and process

These are yours under "The working process".

| Question | Options | Recommendation |
| --- | --- | --- |
| NVDA captures | keep each cited capture as a committed file beside its scenario, with a check that the expectation matches it; or cite compare reports only | commit them; re-capture the 18 scenarios whose files are gone and the three commits named in section 4 |
| Compare mode (about 1,500 product lines; run by nothing) | keep; schedule; retire for `cargo xtask nvda capture`. Retiring changes M4.2's exit criterion | retire once captures are committed |
| The performance ledger copies figures its tests pin (required by the ledger's own policy, `REVIEW.md` and the port skill) | keep; or the ledger names the test | the ledger names the test; four entries have already drifted |
| Instruction counts | exact figures at typical sizes; or each program's worst case kept exact and the typical runs dropped | the second, which needs no change to "Counts are exact numbers, not bounds" |
| The generated caret-watch test (2,716 lines; covers a real hand-off ordering; nine of its sixteen commits only patched its seam) | keep; narrow to about 600 lines; replace with replays of recorded Notepad orders | narrow |
| The key-holding walk's model reads the code under test | give the model its own gesture table; or delete the model half (about 250 lines) | delete the model half |
| The Explore workflow and `testing.md` line 124 | delete; or rescope | delete |
| `cargo xtask ci` runs every step and fails at the end; scans run on one job; line endings set to LF for source files | yes or no | yes to each |
| 18 local-only scenarios run nowhere routinely | a scheduled run; or a stand-in checked against a Notepad recording with the 14 copies retired | a scheduled run first, since two features have no other end-to-end coverage |
| `settings_system_page` failed 8 times on GitHub with "Fix: none" | keep; local-only; retire | triage it properly before any of the three; the rule against calling a failure environmental applies |
| The Hyper-V harness (2,478 Rust lines and its scripts) | decided: you ruled on 2026-11-20 that it goes | removed in build 1; the agent's network address stays as the way to drive a Verbatim in another machine |
| Standing rules that contradict each other (`merged-documents.md` has one table of all 32: five contradictions, 15 rules naming no incident) | resolve each; propose removals | part of the one revision of decision 4 |

### Question 8. Smaller rulings

Matching NVDA's captured speech needs no decision, so for most of these the question is only whether you want something other than NVDA. "Capture" means the speech is unknown until captured.

| Question | Record | Recommendation |
| --- | --- | --- |
| Report focus over the sleeping focus: the 2026-11-15 ruling cannot be built, since putting the focus to sleep empties Core's record and NVDA runs no script then | core-keys Q1 | replace the ruling with "says nothing" |
| Auto-repeat is not counted as a multi-press; called deliberate, never ruled | core-keys Q2 | match NVDA; capture |
| An outpost refusing a UI Automation focus as read through MSAA clears the fence; the MSAA counterpart leaves it | core-focus Q3 | leave it, so a split pair's item is said once; capture |
| A dialog of more than 512 objects gives no text; activation stops 8 parents up; neither has a ruling | outpost Q4, Q5 | remove both caps; capture |
| A failed controller-for read is answered as "unrelated" | outpost Q6 | answer a failure; capture |
| The text facts the outpost's reducer holds but no rule reads (`ownership.md` line 130) | outpost Q2 | move them to the store |
| The presentable-ancestor rule applied in Core alone | access-apis Q1 | yes; capture in File Explorer first |
| Say-all ends sentences on marks from other scripts, with no ruling | text 2 | match NVDA; capture |
| Review word commands use Core's word rules, not the application's | text 3 | match NVDA; one round trip per press |
| Trailing and inner silence is trimmed; NVDA trims only leading silence; no ruling | speech-audio | match NVDA; capture of the gaps |
| The manager's fallbacks for drivers without index marks or inline pitch; the parity ledger's "for every synthesizer, as NVDA does" contradicts the pinned source | speech-audio | remove them and correct the ledger |
| The MSAA alert path no known application reaches (140 product, 141 test lines) | core-keys Q3 | keep; NVDA has it |
| The retry of posts only an out-of-memory system refuses: four copies, three fault points, about 450 test lines | supervisor-app, core-keys Q4 | keep the hook's, which serves your 2026-10-21 ruling; drop the other three and log |
| The latency ledger's dead stages, and never-produced vocabulary in the control protocol | supervisor-app, model-protocol | delete both, with a protocol version; add the request-to-answer stage when the budgets are built |
| The plain reader: 522 product and 1,456 test lines here, as ruled 2026-11-19 | core-focus Q1 | keep; its size falls with question 2, since it must class every request kind |
| The queue generic over its trait | core-queue | keep it generic, emit the model's effects directly (about 80 lines); revisit after build 2 removes three hooks |
| The foreground hold's timer moved onto the WinEvent thread | listener 2 | yes; it removes a hand-written timer with a raw pointer |
| The Terminal page writes all four values on OK, as NVDA's panels do | settings-gui | yes |
| The "Configuration File Error" message at start | settings-gui | match NVDA; capture |
| Unused profile code; the 259-line settings example nothing runs | settings-gui | delete both |
| Sync markers installed in every product process, used only by tests | access-apis Q3 | move behind the faults feature |
| The late-dialog stand-in's three variants never sent | mockapp Q1 | goes with build 2 |
| The event recorder's passes for a "focus-batch design" no document records | tools Q5 | tell me whether that design is planned |
| Five fact programs and three guides whose own READMEs say nothing rests on them (1,647 lines) | uia-facts Q1 | delete, by the folder's own rule |

## 10. Decisions I took that you should know of

- I classed dropping the per-method provider counts in `call_counts.rs` as mine: client call counts and failed reads stay exact, so your rule that a command's calls are pinned by a test holds.
- I stopped one duplicate merger that I had launched by mistake; its finished twin's record is the one kept.
- I asked every merger to recommend nothing on questions in your areas and to lay out facts; the recommendations above are mine.
- I have changed nothing in the repository and built nothing. No capture or recording was made, so no speech in this report comes from anywhere but the repository's tests and scenarios.
