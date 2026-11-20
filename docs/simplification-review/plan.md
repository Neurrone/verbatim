# The simplification: the plan

Decided by Dickson on 2026-11-20, after a review of the whole workspace at ba89dca7. This page is where a session starts. It gives why the phase exists, his rulings, the rules every build follows, the builds in order and the test of when the phase is done. [builds.md](builds.md) holds each build's inventory, [REPORT.md](REPORT.md) the review's report, and the twenty `merged-<area>.md` files beside them the verified findings each build cites.

This folder is working material. A record is deleted when the last build that cites it is built, and this page and `builds.md` are deleted when the phase ends, with what lasts moved into the decision records, the explanation pages and the crate guides.

## Why

At the cost measured on 2026-11-20 Verbatim cannot be finished. A behaviour costs 20 to 40 times what it costs NVDA: holding Enter took 1,294 lines in 35 files against NVDA's 26 to 31, and the dialog read once shown took 4,931 lines in 67 files. In a month 449,000 lines of Rust went in and 268,000 came out of a codebase of 379,000. The review confirmed 22 defects by reading, in code with 142,000 lines of tests.

The causes, in the order of their weight:

1. Every behaviour that crosses a process boundary builds its own crossing. ADR 0003 decided "A new command adds no kind of request", and that part was not built: 23 request kinds, text operations and replies each serve one behaviour, and the property table's columns for each interface are read by no reader. Code on one side of a boundary is already near NVDA's size; every overrun is at a crossing.
2. Verbatim took on behaviour beyond NVDA before reaching NVDA's: terminal reading at 15 times NVDA's loop, progress bars at 24 times, the dialog wait.
3. The focus path was rebuilt repeatedly, and each rebuild left its remains in the code and the documents.
4. The process added and never subtracted: a rule, a test level and a document for each incident.
5. The project produced more decisions than one person can take.

Removal alone does not fix this. What can go with no ruling is about 10,000 product lines and 11,000 test lines of 237,000 and 142,000.

### Why writing Core's logic as async functions did not bring the cost down

The async functions fixed what ADR 0003 named, a chain of requests split across several functions, and that was the smallest cost. They stay; none of the following comes from them, and the builds below remove each:

- Most of a behaviour's lines are outside Core's function: of the dialog wait's 1,560 product lines, about 390 are its Core file.
- Core's thread never blocks, so what arrives while a function is paused is judged outside it, through hooks the feature supplies. The queue's trait had 4 hooks on 2026-11-16 and has 32, of which 14 serve one feature.
- Every wait has six outcomes, each handled by hand where the wait is written. One rule about failed reads is written at nine such places and has drifted into a defect.
- The reducer the functions replaced was not removed: key presses are converted back into its inputs and run through it.

The one cost that is the functions' own is that a paused function cannot be saved, which is why a flight dump that starts while an entry is paused cannot replay it. The roadmap's phase "Exact replay of a paused entry" settles that after this one.

## Dickson's rulings

All of 2026-11-20. His words are quoted where the wording matters.

- **The strategy.** Keep the process layout (D9, D13) and the model of ADR 0003 and ADR 0004; finish the generic crossing ADR 0003 decided; no rewrite.
- **The freeze.** "This cannot continue and I agree on a complete freeze on new functionality till this is resolved." No new behaviour and no new feature until the test at the end of this page is met. A defect fix that moves speech to NVDA's is not new behaviour.
- **No old code left to copy.** "Isn't it pointless if there is old code, because future sessions is just going to copy it, perpetuating the problem." So a build that introduces a better form converts everything built the old way and deletes the old form in the same build.
- **The goal.** A behaviour on one side of a process boundary within 1.3 times NVDA's lines, and one that crosses a boundary within three times, as the starting goal ("Lets start with this goal").
- **The dialog read once shown** is rebuilt by postponement, as build 2 describes ("For the dialog: ok").
- **Progress bars.** Core asks the listener for a bar; a background application's bar is asked for only when the background setting is on ("Would core only ask the listener about a progress bar if the background setting was on? If so, I'm fine with that").
- **The terminal's skip count.** "Exact when it is easy and cheap to give an exact count. Otherwise, provide some sort of estimate when possible", for example "skipped over 1000 lines".
- **The Hyper-V harness** is removed, "since no one uses it"; driving a Verbatim in a separate machine stays, through the agent's network address.
- **The remote-operation builder methods that no program emits yet stay.** "The 'dead code' in remote operations shouldn't be deleted because future work like browser UIA support or Word support would need them." So the review's finding of 31 such methods (about 620 product and 214 test lines) removes nothing, and build 5's budget is larger by that much.
- **Batches before specialized requests.** "There could be an argument for a specialized request if it saves many round trips. Though the argument for this is that if multiple requests could be sent as one batch, that could achieve the same goal if replies could also be delivered in one batch." So build 8's study starts from a batch of generic requests sent together and answered together, and a specialized request is kept or added only where a measurement shows that a batch cannot save its round trips, with his sign-off.
- **No time goes on known flaky tests.** "The other big waste of time is triaging flaky tests. So for the ones that we can fix now before any feature work, those get done as part of the plan as well. If not, the other flaky ones are disabled in GitHub." So every entry of `docs/flaky-tests.md`, 23 on 2026-11-20, ends one of two ways in this phase: its cause is fixed, or its test is not run on GitHub and the entry says why and what would let it run again.
- **Matching NVDA where practical.** "We want to do that where practical but not if it causes the product to bloat and become unmaintainable." So a build matches NVDA, after a capture, where that needs no new state, rule or mechanism and keeps the build within its budget; otherwise the difference, with NVDA's capture, the sequence and what matching it would cost, goes to Dickson for his approval when it arises ("I want to be alerted when this happens, i.e, I need to approve it"), the build stopping on that behaviour alone until he answers, and once approved it is recorded in the parity ledger. Matching NVDA one sequence at a time in an architecture that is not NVDA's was found to be the root cause of fixes that add mechanisms (2026-11-20).
- **Budgets report, they do not stop.** "I'll be away soon, so the budget should not stop a build. I believe the real value of this is in reducing the code needed for new features or changes" (2026-11-21), for every build in the phase. A build's lines against its budget, and the second proof's count, go in its report; for this phase this replaces rule 6's stop and the CLAUDE.md standing rule's stop at a design's budget. The other stops stay: a new kind of request, reply, text operation, waiting request or queue hook; must-fixes after three review rounds; a difference from NVDA that matching would need new state, a rule or a mechanism for; and any question in his areas.
- **The process revision** was approved and is applied (commit 4e8fbc69): the request list is closed, every design carries NVDA's lines and a budget that stops the build, a review asks first what a change adds to shared machinery, each behaviour is tested once on the real code, captures are committed beside what cites them, and a figure a test pins is named by that test.
- **This plan** is the outcome of the session of 2026-11-20, to be executed from the next session on. The records it cites stay under `docs/` until no longer needed.

## Rules every build follows

These add to `CLAUDE.md`, which holds the standing rules.

1. A function in Core waits only for calls into applications, never for an event. Something that must wait for an event ends its entry, and the event arrives as its own entry, judged at its turn. The check after a menu closes, the return from another desktop and say-all's next read already work this way; the dialog wait was the one exception.
2. One place decides what a wait that returned no result does. A function does not handle the six outcomes at each wait.
3. A behaviour adds no hook to the event queue. If it seems to need one, rule 1 or rule 2 has been missed.
4. One owner and one record for each fact. The review found nine facts kept in several places, four of them with a defect between the copies; `REPORT.md`, "One fact, several owners", lists them.
5. A build that introduces a better form converts every user of the old form and deletes the old form before it is pushed. Nothing is left for later, and nothing is deprecated in place.
6. Each design states NVDA's lines for the behaviour and its budget in product and test lines, and the build stops for a look at its shape when it passes the budget.
7. Speech that no test or capture states today is captured from NVDA before the build that touches it. Each record says "needs a capture" where that applies.

## The builds, in order

One build that changes state is in flight at a time. The tests and harness build and the documents build change no product state and may run beside another. [builds.md](builds.md) gives, for each, what is measured or captured first, the inventory by finding, the tests, what it must not do, its budget and how it is known to be done.

1. **Defects and tooling, with the Hyper-V harness's removal.** The confirmed defects that need no capture, each with the test that would have caught it; `cargo xtask ci` reporting every failure in one run; `cargo xtask e2e` refusing an empty selection; one line ending for source files; the dead nightly workflow deleted; the harness removed, with an endpoint option on `cargo xtask e2e` and loopback as the agent's default address.
2. **The dialog read once shown, by postponement**, with the queue's four stop hooks made one judge. The first proof: about 400 product and 340 test lines against today's 1,560 and 2,190.
3. **Core: one owner per fact, dead paths out.** One committed focus record, one fence rule, one foreground record; the key-press bridge and the old reducer's inputs; the dead rules and protocol; the gesture's meaning in one table.
4. **The readers.** The listener's one pending foreground; its delivery-time check as a straight-line function, tested directly and used by mockapp's tests in place of their copy, which is deleted; Core as the only lasting keeper of a window's verdict; progress bars as Core's asks; the outpost's dead tags and its caret element sweep.
5. **Remote operations and text.** One dispatcher with one rule; the duplicate walks; the call-by-call twins as the fallback count decides; flat review through the text path and the Backspace fix, each after its capture.
6. **The tests and the harness.** First, and beside build 1, the flaky list: each of its entries is triaged once to a named cause, fixed where the fix lies in the tests, the harness or a build of this plan (the entry then names that build), and otherwise taken out of GitHub's runs with its reason recorded, so that no later build reads a known failure in its GitHub run. Then every end-to-end scenario is judged once, since the review looked only for duplicates it could prove and did not ask of each scenario whether it is needed: a scenario stays when it is the cheapest test that runs the real processes along its path against captured NVDA speech; one whose path another scenario already runs is merged into it or removed; one that asserts only what a recording replayed through Core would assert becomes that replay; and the table of all 105, with each one's path, duration, capture and verdict, goes to Dickson with the build's report. Then one scenario list; typed agent requests; one Core test harness; fixtures read from their files; captures committed and the scenarios without one captured again.
7. **The documents.** The built design files split into explanation pages and decision records and deleted, with the code comments that cite them repointed; crate guides that point at module documentation.
8. **The request list closed.** A design study first, then the build. Its measurements run beside build 2, and its design is reviewed and brought to Dickson, who approves it in person, once build 2 is pushed; the build on the list follows build 5, because builds 3 to 5 delete or reshape kinds it converts (Dickson, 2026-11-20). The build: a closed list of operations on objects and text, a request as a batch of them answered in one batch, a later operation able to name an earlier one's result so that dependent steps need no second round trip, one builder that turns a list into a remote operation and one executor that runs it as ordinary calls, one object abstraction for each backend. Every one of the 23 kinds that serve one behaviour is converted and its old form deleted, or kept with Dickson's sign-off and its reason. The first behaviours converted are the ones sessions copy most, a focus read, a caret key and a text command, and each becomes the example a brief points at. The second proof: one ordinary behaviour that crosses a boundary, built on the list and counted against NVDA.
9. **The terminal rebuild** (roadmap M6.4), from a corrected study, with every feature Dickson ruled kept and the skip count as he ruled it.
10. **The structural parts of later milestones.** The freeze means M6.2, M6.3, M5, M4.3, M6 and M6.1 do not run, so the repairs the review left for them are done here: the outpost generation in one record and the owner thread's copy of it; the speech queue's one record of an utterance and one report of its ending; the command vocabulary written once; the settings pages built from one table of rows; the host protocol's mirrored message type. Its first step is to read each of those roadmap entries and split restructuring, which joins this build, from new function, which waits. `builds.md`, "Findings that belong to an already planned roadmap phase", lists the findings.
11. **The second pass.** Every area's lines are counted again against NVDA's, by the method of the review's size tables, which reads the code and NVDA's pinned source and runs nothing. Wake-ups and processor time are measured once for the phase, as the standing rules ask at a milestone's end. An area above the goal gets its own look, and what it finds is built, before the freeze lifts.

## One session works through the whole phase

Dickson plans one session for all of it (2026-11-20). Its context will be summarized many times, so the state of the phase lives here and not in the session's memory:

- At the start, and again after every summary of its context, the session reads this page and the progress table below, then the section of `builds.md` for the build in hand.
- Dickson is present at the start and absent after it, so what needs him comes first, before any build: the session asks for the desktop for the captures and recordings of builds 1, 2, 5 and 9; it makes build 8's design study and puts the list of generic operations to him, since that list is a public contract; and it corrects the terminal study and puts to him anything in it that the defaults do not settle. The builds then run without him, in their order, and stop only as the next point says.
- A build's steps (design, adversarial review, build, implementation review, fixes) are ordinary subagents, one after another, with the session checking each hand-off against the code before the next step starts and judging between builds: it runs the scenarios, pushes, reads the GitHub run and fills in the progress table, because a result nobody checked is how the present state came about. A workflow of subagents is for the steps that fan out over many independent items with nothing to judge between them: the triage of the flaky list's entries, the judgement of the 105 scenarios, and the second pass's count of every area's lines against NVDA's, each of which reads code and logs and needs no desktop. Captures, recordings, scenario runs and measurements of wake-ups, processor time and latency are not such a step: there is one desktop and one machine, so the session runs them itself, one at a time from one background script.
- Each build runs the same way: what must be captured or measured first; one design with its state tables, NVDA's lines and its budget; one adversarial review; the build by a builder subagent; the scenarios whose path it touches; the push, and its GitHub run read. Then the row below is filled in, the records that no later build cites are deleted, and Dickson gets a report of at most a page: what changed, the lines against the budget, and each default it acted on in his areas.
- The session stops and goes to Dickson, building nothing more in that mechanism, when a build passes its budget, when its design still has must-fixes after three review rounds, when it would add a kind of request, reply, text operation, waiting request or queue hook, or when a question falls in his areas. It carries on with a build the question cannot change.
- Captures and recordings need the desktop. Build 2 needs seven NVDA captures and a recording of the Import dialog; build 9 needs recordings of the terminal programs. The session asks before it takes the desktop.
- The tests and harness build and the documents build may run beside a state-changing build, each in its own worktree.

### Progress

Captures made before the builds (2026-11-20), kept in `target/captures-2026-11-20/` with the scripts that made them until a build commits the ones it cites; NVDA ran at the pin from a copy of Dickson's configuration with eSpeak NG. Made: build 1's corrupt `nvda.ini` (D1), U+0001 in a line and a title with audio (F1), the say-all gap's audio (F7); build 2's recording of Import a Theme; build 5's B10, B12, B13 and F5; build 9's Q1, Q2, Q8, Q10, Q13, Q15, Q16 and Claude Code in screen reader mode. Still to make, by the build named: B9 and F8 (build 5; the focus did not reach File Explorer's item, so find how the end-to-end harness gives it the focus); the four keys pressed while a dialog is hidden (build 2; needs `cargo xtask nvda capture` to accept a hidden window), and the tray, message-box and never-shown cases (build 2; a real application must be found first); Claude Code with screen reader mode off, in the classic renderer (build 9); recordings of fzf, tig and Microsoft Edit (build 9). Not captured: F18, since Verbatim does not yet support elevated windows (Dickson, 2026-11-20: "Don't do an elevated capture since that's not supported yet"); F20 and Q4 (no reachable element known), Q3 (no way known to hang a console that keeps writing); their builds decide from NVDA's source.

The session fills in a row when its build is pushed and its GitHub run is read. Lines added by review fixes are tallied in the row, so that creep shows as it happens: build 1, about 150 product lines (the configuration box blocking settings and exit), the whole of its overrun; build 2, the design's round-two fix for a lost Tab announcement added a rule that broke another sequence and was reverted, so none.

| Build | State | Commits | Product lines against budget | Test lines against budget | Records deleted |
| --- | --- | --- | --- | --- | --- |
| 1. Defects and tooling | pushed 2026-11-20 (a734622e), GitHub run read: passed | 75f7f106, d628370f, f68618f5, dd6bae83, 2faff85c, c60ae1e3 to a734622e | about 4,700 removed against a floor of 3,950; about 535 new added against 450, over by the review's modal-box fix | about 530 new added against 500 | none: every record build 1 cites is cited by a later build |
| 2. The dialog read once shown | pushed 2026-11-20 (6e4ac482), GitHub run read: failed only on text_box_caret_keys, its known anchor cause, which is now local-only (c7cfd6fc) | 8141d0fa, bdec81ea, 31248cb2, 6e4ac482 | 387 new added against 400 | 501 new added, accepted by Dickson over 500 | none: every record build 2 cites is cited by a later build |
| 3. Core: one owner per fact | building | | | | |
| 4. The readers | not started | | | | |
| 5. Remote operations and text | not started | | | | |
| 6. The tests and the harness | flaky list triaged (23 entries) and its fixes pushed (5713c64d and after); the measurements closed four more entries; the test console's cause named and being fixed | | | | |
| 7. The documents | not started | | | | |
| 8. The request list closed | not started | | | | |
| 9. The terminal rebuild | not started | | | | |
| 10. The structural parts of later milestones | not started | | | | |
| 11. The second pass | not started | | | | |

## The test of done

The freeze lifts when all of these hold, each checked and reported to Dickson:

- every defect in `REPORT.md`, "Defects confirmed", is fixed, with a test that fails without the fix;
- each fact in `REPORT.md`, "One fact, several owners", has one owner and one record;
- no request kind, reply kind or text operation serves one behaviour without Dickson's sign-off recorded with its reason;
- no event-queue hook serves one feature;
- no test runs a copy or stand-in of Verbatim's own code;
- every end-to-end scenario cites a capture that is in git, and a check compares the two;
- `docs/flaky-tests.md` holds no entry that is neither fixed nor taken out of GitHub's runs with its reason;
- no built design file remains under `docs/design/`, and no code comment cites one;
- both proofs are measured and reported with their lines against NVDA's;
- the second pass is done, and each area is within the goal or has Dickson's ruling on why it is not.

## What Dickson has settled, and the defaults open to his objection

Settled on 2026-11-20, each "per your defaults":

1. About 3,300 product lines, 3.6 times NVDA's loop, is accepted for the terminal rebuild with every feature kept, judged again after the second proof.
2. The "focus-batch design" that the event recorder numbers its passes for is not planned, and the passes go (build 1).
3. M4.6's second measure and its mutation follow-up are retired (build 7).
4. The evidence kept for a call Verbatim no longer makes is replaced by its commits (build 7).

Nothing now waits for him before build 1.

The session's other defaults in his areas are listed at the end of `builds.md`, and each build's report names the ones it acted on, so that he can object: match NVDA after a capture; the later of two decisions of record governs; dead code, dead protocol and unreachable fallbacks go.

Decided by the session and open to his objection, since the process revision left them to the builds: compare mode is retired once captures are committed (build 6); each remote program's worst-case instruction count stays exact and the typical-size runs go (build 5); the generated caret-watch test is narrowed to the key, the application's events, the checks and a pending focus (build 4); the key-holding walk keeps its checks and loses the model that reads the code under test (build 3); the 18 local-only scenarios get a scheduled run (build 6); `settings_system_page` is triaged to a named cause before anything else is done with it (build 6).

## What is not yet verified

- Progress bars asked outside the event queue, so that no key waits behind a bar, is the session's variant of the ruling; build 4's design checks it against the code and measures its cost before building.
- The goal of three times NVDA for a behaviour that crosses a boundary is an estimate. The second proof measures it.
- One description of a read serving both as a remote program and as ordinary calls is an idea, not a finding: nothing in the review shows that one builder can turn a list of generic operations into a program as small and as fast as the ones written by hand, within the limit of 10,000 executed instructions, or that every present read can be said in such a list. Build 8's study settles it before anything is built on it, by converting one read that is hard (the caret read with its formatting, or the focus ancestry) and comparing its instruction count, its calls and its time with today's. If it does not hold, the twins stay written by hand and the study says what else removes their drift.
- No speech in the review comes from a capture; the records quote the repository's own tests and scenarios, and say "needs a capture" elsewhere.
- The review confirmed its findings, not the absence of others: each reviewer covered 10,000 to 30,000 lines in minutes.
