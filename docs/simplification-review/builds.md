# The builds of the turnaround: per-build inventories

Compiled 2026-11-20 from the review's verified records in this folder (REPORT.md and the twenty merged records), against main at ba89dca7 and NVDA pinned at 732d1ce2. Nothing here was re-verified; where two records disagree, the entry says so. The next working session executes it. This session is planning only.

## How to read this plan

Record names are shortened: "queue" is merged-core-queue.md, "focus" merged-core-focus.md, "keys" merged-core-keys.md, "model" merged-model-protocol.md, "supervisor" merged-supervisor-app.md, "listener" merged-listener.md, "outpost" merged-outpost.md, "mockapp" merged-mockapp.md, "apis" merged-access-apis.md, "text" merged-text.md, "calls" merged-reader-call-tests.md, "scenarios" merged-e2e-scenarios.md, "harness" merged-e2e-harness.md, "tools" merged-tools.md, "documents" merged-documents.md, "facts" merged-uia-facts.md, "terminals" merged-terminals.md, "settings" merged-settings-gui.md, "speech" merged-speech-audio.md, and "dialog" merged-dialog-study.md. A finding is named by its record and its id in that record, for example "queue F1". Line counts are the records' own estimates: "product" and "tests" each give lines removed, then lines added.

Terms used throughout:

- The wait: today's dialog mechanism, in which the outpost answers that a dialog is hidden, Core asks the listener to be told of its show, and the queue's front holds until then.
- Postponement: the rebuild Dickson accepted, in which a read meeting a hidden dialog ends its entry, Core keeps one fact, and the listener's tell of the foreground window's show joins the queue as an ordinary event.
- A twin: the call-by-call version of a read that also exists as a remote program.
- The process revision: the revision of CLAUDE.md, REVIEW.md and testing.md that Dickson approved on 2026-11-20 and that commit 4e8fbc69 applied. Findings it left to the builds are listed at the end, with the session's default for each in [plan.md](plan.md), "What still needs Dickson".
- Today's documentation correction: the separate correction made today, which corrects stale statements only.

### Dickson's rulings of 2026-11-20 that the builds rest on

Quoted exactly; nothing else in this plan is attributed to him as a ruling of today.

1. The strategy: keep the process layout and the model of ADR 0003 and ADR 0004, finish ADR 0003's generic crossing, and freeze new behaviour until two proofs are in: the dialog read once shown rebuilt, then one ordinary behaviour built on the generic crossing and counted against NVDA.
2. This session plans only; the plan is executed next session.
3. The Hyper-V harness: "The Hyper-V harness should be removed, since no one uses it. I do want the ability to control an instance of Verbatim running in a separate VM though, if that makes sense and wouldn't be expensive to maintain."
4. Progress bars, Core asking the listener for a bar: "Would core only ask the listener about a progress bar if the background setting was on? If so, I'm fine with that". The answer given to him: a background application's bar is asked for only with the setting on; a bar in the application holding attention is asked for either way, as it is read today.
5. The terminal's skip count: "Exact when it is easy and cheap to give an exact count. Otherwise, provide some sort of estimate when possible, e.g skilled over 1000 lines" (he means "skipped over 1000 lines").
6. The dialog read once shown, rebuilt by postponement as REPORT.md's decision 3 describes: "For the dialog: ok".

### Dickson's rulings of 2026-11-20, asked ahead of the builds

Asked while he was present so the builds can run without him; each build acts on its own.

- The whole phase: a design is built without his approval when it stays inside the plan and his rulings, keeps within its budget, adds no kind of request, reply, text operation, waiting request or queue hook, and clears its review within three rounds; otherwise it waits for him. The session may take the desktop whenever a build needs it (parked, one background script at a time, reporting when it is free). A capture that still fails after about 10 tries is decided from NVDA's pinned source, pinned in a unit or replay test marked as source-derived, and no end-to-end expectation is written without a capture. The cited part of a measurement a decision rests on may be committed under `experiments/` without proposing it first, if the build's report lists it; new files in `docs/design/` are still proposed.
- Build 2, approved by Dickson on 2026-11-20 as differences from NVDA, each recorded in the parity ledger: Tab pressed while a dialog is hidden keeps today's race, in which the new focus can go unheard; a dialog that takes the foreground and is never shown stays silent (no timer); the owner dialog of a message box closed while it is hidden is said at its show. Build 2: today's tested words are kept; in sequences no test states, the build follows NVDA's capture where that adds no state, and otherwise keeps the design's speech and lists the difference in its report.
- Build 3: key auto-repeats count as presses for every command, as NVDA counts them (every key down runs the script and the press count is runs within the multi-press timeout, `keyboardHandler.py` 309-312, `scriptHandler.py` 286-290), the tray-list key included with no exception (Dickson, 2026-11-21: "For auto repeats handling, match NVDA"); the design's 160 test lines are accepted against 150 (Dickson, 2026-11-21).
- Build 3, approved by Dickson on 2026-11-20 as a difference from NVDA, recorded in `docs/parity/focus-events.md` by the build: the check after a menu closes compares the committed node, not NVDA's object identity, since matching needs a commit counter (in the one sequence found, both say the focus once). Build 3: NVDA's window class lists live once, in `verbatim-model`, written from the parity docs. ADR 0001's "Report focus" is rewritten to the 2026-11-15 ruling (the committed focus). Key repeats of Verbatim+Numpad 5 count as presses, as NVDA's, once a capture confirms it.
- Build 4: a progress bar's tell joins the queue and is judged against the foreground at its turn, and the bar is read outside the queue. The dialog-text and activation walks stay bounded: the text read up to the cap is spoken and both caps are raised above any measured application, so security-model U2 and U3 hold.
- Build 5: if the twins go, the generic classic path gains the formatting read and the selection-on-first-line read (about 60 lines). Say-all ends sentences only where NVDA does, after its capture (text B12).
- Build 6: the build acts on each scenario's verdict and reports the table after, never removing the only end-to-end test of a feature or a protected scenario (the quit scenarios, `controlled_list_selections`).
- Build 7: crate guides keep their mechanisms, drop their Public API sections and point at rustdoc. `ownership.md` becomes a present-tense explanation, with CLAUDE.md's pointer reworded.
- Build 8's design approved by Dickson in person on 2026-11-21 (three review rounds; its rules now in the verbatim-uia-rops and verbatim-outpost crate guides and ADR 0003): the 17 operations, subjects, outcomes and conditions as the contract, a cap of 4 steps per batch, programs generated by one builder, the classic reads kept by re-cutting them per operation into the call-by-call executor with the per-step fallback, FocusRead and MoveBySentence as shared operations, builds 8 and 9 pushed together, the matcher type in verbatim-model (extensions.md line 61 corrected), keys M2, model 6, 14 and 15 and focus F5 left by build 3 and apis F4 and text B3, B4 and B5 left by build 5 for build 8, and ADR 0003 line 146 reworded to name operations. Its value is in the code a new feature or change needs, not in lines removed now (an honest net of about 700 fewer).
- Build 8: remote programs are generated from the operations by one program builder (shape A), over serving the operations with today's hand-written programs (shape B), after being shown both sizes and that NVDA has no generator (Dickson, 2026-11-21).
- Build 8: the design of the request list waits for Dickson's approval with him present, whatever the conditions above (2026-11-20: "I want to be around for this"); the session sends it with a summary as soon as its review is settled, and builds nothing on the list until he approves.
- Builds 5 and 8: the classic, call-by-call UI Automation reads (the twins of the remote operations) are not removed without Dickson's explicit approval (2026-11-20), whatever the fallback count shows; the count is reported to him with a recommendation. This replaces the default that the twins go when the count shows no fallback outside the developer setting. Whether the caret read and the terminal read are generated from generic operations or kept as one shared operation is decided by the rule: generated if under 10,000 executed instructions and no slower in the outpost beyond the spread between runs, otherwise the one operation, with the figures in the report. If the second proof passes three times NVDA's lines, builds 9 to 11 continue, anything facing M5's extension surface is held, and the count goes to him.
- Build 9: the six ruled differences are kept (blank lines counted in a skip, "s" then "*" at a password prompt, a lone spinner glyph silent, a read inside Claude Code's redraw silent, every tab completion spoken, today's pacing); today's behaviour is kept for a refused or late first read and for an output read past its due time; Q9 to Q12 are approved as written in the study. The budget may rise to about 4,060 lines only if the recordings show keys owning changes (F8) needs its larger form. Of M6.4's residuals, the build takes those the rebuild makes moot, those its tests reach as defects and those moving speech to NVDA's; the rest wait.
- Build 10: from M6.2 only supervisor F1's owner copy, F10, F12, F13 and F14; from M6.3 speech F4 with the `drop_expired` fix and the queue's pure reducer, plus F11, F17 and F18; from M6 speech F9, F10 and F16; from M6.1 the queue work that grows with the waiting line; from M4.9 its two findings; from M5 the gesture tables merged and the command vocabulary written once; from M4.3 the settings pages built from one table of rows. New function waits for its phase.
- Build 11: an area ruled above the goal counts as within it at its ruled size; an area with no NVDA counterpart is reported with its lines. When the test of done is met, the session reports and waits for him; it starts nothing more.
- The fourteen defaults in his areas listed at the end of this page and under "The session's defaults" are confirmed.
### Dickson's rulings of 2026-11-21

- Terminals: padding past a row's text is never reviewed, and review next character at a row's last character says "Right" and that character, as now; it never moves to the next line.
- Text: previous word stops on a line break and says it, as NVDA does, in UI Automation text too (the 2026-11-03 difference in `docs/parity/review.md` is retired once built).
- Compare mode is exploration only; it stops a run when NVDA exits or is replaced mid-run.
- Build 7's document points take their defaults: crate guides drop their Public API sections and point at rustdoc (CLAUDE.md's wording follows); each decision keeps its latest date when its "Amended" lines go; the deferred simulation entry is judged again at the second pass; the later-milestone notes move to the extension, security and roadmap documents; the terminal design file goes with the terminal rebuild; the 2026-10-30 ruling is recorded as retired by build 3's code; the testing rule moves to `testing.md`.
- Terminals: switching back to a window or tab says its terminal and the caret's line, whatever the line holds, as the console host does in NVDA; NVDA's leaving the line out in Windows Terminal is not copied (extends the 2026-10-09 close-tab ruling; recorded in `docs/parity/terminals.md`, "Windows and tabs").
- The dead text protocol (`ReadFacts`, `Change`, `CaretChanges` and their five fields, text B2) is removed as planned.
- Build 5: `Role::Caret` and `Role::Cursor` ("caret", "cursor", as NVDA's) are added and become extension-matcher role names (apis Q4); review by word through the provider (text B13), the previous-word ruling and the word-edge short-cut move to build 8, which does each as one batch.
- A window's events dropped because it is hung (ghosted or flagged by `IsHungAppWindow`, as NVDA drops them) are reported to Core as that window being hung, by a new "window hung" report that the listener and the outposts both send, each when it first sees a window hung (Dickson, 2026-11-21: the listener keeps the check and informs Core, and the outpost sends the same message), so that Core's existing recovery plays "not responding" while it holds attention and reads its focus again once it answers; NVDA stays silent there, and Verbatim speaks the dialog late (Dickson, 2026-11-21). With it, the end-to-end harness writes a dump of Verbatim when a speech wait times out, and Verbatim logs the time around a native message box's `ShowModal`, so the next GUI-thread stall names its cause. Built after build 5, with a short design.
- Build 8: a failed remote program keeps the approved per-step rerun through the classic calls, so normal use keeps working; each fallback is still logged and fails its end-to-end scenario (Dickson, 2026-11-21).
- Build 8: an act's remote program ends after the act, and when it fails the classic fallback still runs the act, so an act can run twice if the program failed after it; harmless for Select, today's only act; whether a non-repeatable act such as Invoke reruns is decided again when one is added (Dickson, 2026-11-21).
- No end-to-end scenario, local-only or not, is deleted without Dickson's explicit approval; each proposed deletion names what still covers it (2026-11-21).
- Build 11, the final pass, analyses what can still be reduced or simplified along the lines of the phase's work and builds it; where anything is ambiguous it waits for Dickson (2026-11-21).
- Build 8: the generic executor's classic path (`run_calls`) is the one classic path; each old hand-written classic twin goes once the generic path is proven correct for its reads (Dickson, 2026-11-22). Cognitive complexity matters more than line count: the representations a step passes through are cut to what is needed (Dickson, 2026-11-22).
- Build 9: the terminal's needs are met by generic operations that browsers and Word can use (a visible-range read, `FindText`, a padding-trim option) rather than terminal-specific ones, the exact list and batch size shown to Dickson first (Dickson, 2026-11-22). The terminal's classic twins stay (Dickson, 2026-11-22). The skipped-lines count is taken during the read's own turn, and output after a dropped read is compared with what was last spoken, both as defects (Claude, 2026-11-22).
- The personal paths already in the October terminal recordings on origin are left; new recordings are checked before commit (Dickson, 2026-11-22).
- One execution path (Dickson, 2026-11-22: "this is what it should have been in the first place"): every operation in the request list is implemented only inside the generic executor, one per backend, and the outpost's own per-operation functions and the old classic twins are deleted once a both-executors test proves the generic answers equal. The step representations are cut to the model's step and outcome, the resolver's objects and the program. `Caret` returns one `at_text_start` flag on the selection in place of its edges. The terminal uses only generic operations (`Visible`, `Move`, `FindText`; `whole` and `trim` on `ReadText`, `trim` on `ReadRange`; up to 16 steps), with its colour runs read by `ReadText`'s formats and interpreted in Core.
- The three refusals Core no longer logs stay unlogged until the request list closes. The 5-second figure's program stays in git history only.

### The session's defaults (REPORT.md section 9, "unless you object")

Each build names the defaults it rests on. In short:

- Where NVDA has an answer, match it after a capture.
- Where two decisions of record disagree, the later governs, and the earlier document is corrected. So Core's backend cache owns a window's verdict, by ADR 0004.
- Dead code, dead protocol and unreachable fallbacks go.
- The call-by-call twins go only if a count of fallbacks in the GitHub runs' logs shows none outside the developer setting.
- Section 9's table of smaller rulings, each with the recommendation written there.

Several defaults touch an area Dickson owns (thread layout, public contracts, a dated ruling). They are covered by his "unless you object", but each build's report to him names them, so that he can object. They are marked "(his area, by default)" below.

### Standing constraints from CLAUDE.md that shape the order

- One build that adds or changes state is in flight at a time. Builds 1 to 5 and 9 change state and run one after another. Build 6 (tests and harness) and build 7 (documents) change no product state and may run beside another build, each in its own worktree and target directory.
- Each build has one design with its state tables where it adds or changes state, an inventory of what the replaced code does and says, one adversarial review, and its scenarios found from the code.
- Captures come before expectations. Before any capture series, the NVDA pin (`git -C nvda rev-parse HEAD`) is compared with the installed NVDA build the capture header prints, and the series stops if they differ.
- The freeze: no new behaviour until both proofs are in (builds 2 and 8). Defect fixes that move speech towards NVDA's are not new behaviour.
- The process revision was approved and applied on 2026-11-20 (commit 4e8fbc69). A build that would change CLAUDE.md, REVIEW.md or testing.md further brings the change to Dickson.

## Build 1. Defects and tooling, with the Hyper-V harness's removal

### Purpose

Fix every confirmed defect that needs no ruling and no new design, each with the test that would have caught it, and make the tools report failures honestly. Remove the Hyper-V harness as Dickson ruled while keeping the ability to drive a Verbatim in another machine through the agent's address.

The build has three groups that touch separate files and may be committed separately: the tools and harness group, the Hyper-V removal, and the leaf-crate group (speech, synthesizers, settings, GUI and localization), placed here because the defects in those crates open the same files as their dead code. If the leaf group grows past its budget, the session may split it into its own build; it changes no Core state.

### Before the design: measure, capture or record

- Capture: NVDA at the pin starting over a corrupt `nvda.ini`, for the "Configuration File Error" message box and what NVDA says (settings D1).
- Capture: NVDA speaking a window title or document line holding U+0001 followed by `V` and a word, with eSpeak NG (speech F1).
- Record: NVDA's audio across two say-all utterances with eSpeak NG, to time the gap between them, for the trailing-silence decision (speech F7).
- Measure, after the fix: the control plane's speech-history path with no client subscribed (model 11; the record says the fix's cost on the speech path needs a measurement).
- Check: `.github/workflows/ci.yml` 494 to 495 already starts the agent with `--bind-address 127.0.0.1`, so making loopback the default changes nothing for CI.

### Inventory

Tools and harness group:

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| tools T1 | `cargo xtask e2e` passes a misspelled scenario | `xtask/src/local_e2e.rs` 154-181; `crates/verbatim-e2e/tests/e2e.rs` 284, 292 | 0 / 8 | 0 / 10 | no |
| tools T2 | `cargo xtask measure` exits 0 when a scenario failed | `xtask/src/perf.rs` 145-147, 192-246 | 0 / 3 | 0 / 10 | no |
| tools T3 | measure's default list runs 7 of the 15 silent measurements | `xtask/src/perf.rs` 27-37; `books/dev/src/how-to/measuring.md` 36 | 9 / 0 | 0 | no |
| tools T4 | the nightly Explore workflow runs zero tests | `.github/workflows/explore.yml` (133 lines) | 133 YAML | 0 | no |
| tools T5 | dead exception entries for deleted exploration tests | `xtask/src/ownership.rs` 769-771, 784-786 | 6 / 0 | 0 | no |
| tools T6 | `DETERMINISM_ENTRIES` copies `clippy.toml` | `ownership.rs` 629-752, 904-910, 1417-1428 | 136 / 0 | 13 / 0 | no |
| tools T7 | two exception lists repeat ten paths | `ownership.rs` 760, 776 | 10 / 0 | 0 | no |
| tools T8 | `cargo xtask ci` stops at the first failing step | `xtask/src/main.rs` 363-458; `tiers.rs` 517 | about 20 changed | 0 | no |
| tools T9 | CRLF checkouts on GitHub's runners | `.gitattributes`; `listener.rs` 3566-3569; `xtask/src/nvda.rs` 131 | 8 / 1 | 0 | no |
| tools T10 | crate and binary lists Cargo or the tiers hold | `main.rs` 146-161, 462-485; `ci.yml` 404; `scenario.rs` 3742 | 50 / 0 | 9 / 0 | no |
| tools T12 | `demo` starts the agent its own way; `Agent` ignores `CARGO_TARGET_DIR` | `xtask/src/demo.rs` 133-160, 206-232; `agent.rs` 41-69 | 55 / 10 | test moved | no |
| tools T13 | scan helpers written two and three times | `waits.rs` 125, 145; `ownership.rs` 408, 458, 498, 973; `coverage.rs` 493 | 50 / 0 | 0 | no |
| tools T14 | `vm-smoke.yml`, retired by the roadmap, still present | `.github/workflows/vm-smoke.yml` | 125 YAML | 0 | no |
| tools T15 | `source_commit` copied between two build scripts | `verbatim-replay/build.rs` 19; `verbatim-app/build.rs` 85 | 37 / 0 | 0 | no |
| tools T16 | `HOOK_START_FORBIDDEN` restates `HOOK_FORBIDDEN` | `ownership.rs` 263-290, 298-323 | 20 / 0 | 0 | no |
| tools T17 | coverage rebuilds `ci`'s fault arguments | `coverage.rs` 115-139; `main.rs` 492-530 | 20 / 0 | 0 | no |
| tools T18 | the event recorder names its UIA events twice | `verbatim-event-record/src/uia_events.rs` 42-68, 370-401 | 20 / 0 | 0 | no |
| tools T19 | the waits scan prints sleeps nothing acts on | `xtask/src/waits.rs` 85-95 | 10 / 0 | 0 | no |
| tools T20 | cache steps written three times; scans on three hosts | `ci.yml` 119-137, 206-224, 381-399 | about 40 YAML | 0 | no |
| tools T21 | stale descriptions of the checks (books and manifest part) | `books/dev/src/how-to/setup.md` 179-187; `xtask/Cargo.toml` 3 | text | 0 | no |
| tools T22 | the pure rule's forbidden lists overlap `clippy.toml` | `ownership.rs` 160-182, 329-347 | kept: no net saving and the allow-lists cannot move | 0 | no |
| facts M6 | the uia-facts workflow triggers on `verbatim-core` it no longer builds | `.github/workflows/uia-facts.yml` 26; `uia-facts/readme.md` 67 | 1 YAML | 0 | no |
| harness F1 | the logged-defect gate misses two of its 23 messages | `crates/verbatim-e2e/src/artifacts.rs` 467-492, 505-512, 1024-1033 | 30 net | 10 / 10 | no |
| harness F4 | the agent takes a failed focus read for "the client has the focus" | `crates/verbatim-agent/src/focus.rs` 267 | 0 / 5 | 0 / 20 | no |
| harness F5 | a frozen screen capture passes the recording check | `crates/verbatim-agent/src/capture.rs` 229-251 | 0 / 15 | 0 / 20 | no |
| harness F6 | NVDA transcript replies read missing fields as empty | `crates/verbatim-e2e/src/nvda.rs` 126-131, 166-167, 180-181, 312 | about 150 (serde types; the pushed transcript is optional) | 160 net | no |
| harness F7 | two definitions of "the running NVDA is the pinned build" | `xtask/src/nvda.rs` 666-694; `compare.rs` 330-377 | 30 / 0 | 10 / 0 | no |
| harness F18 | an unreadable image is never counted as a foreign Windows Terminal | `crates/verbatim-agent/src/desktop.rs` 538; `terminals.rs` 126 | small | 0 | no |
| supervisor F2 | a stalled reducer ends every outpost before reading queued pongs | `crates/verbatim-app/src/main.rs` 2041-2052 | loop order changed | 30-60 / 40 | only in the stall sequence |
| model 11 | the control plane's startup speech history never closes | `crates/verbatim-control/src/server.rs` 1073-1081, 1186-1191; app `latency.rs` 734-745 | a few / a few | 0 / one test | no |

Hyper-V removal:

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| tools T24 | the Hyper-V harness, removed as ruled | `xtask/src/vm/` (connect, create, deploy 622, dotenv, host 553, lifecycle, logs, mod, packer_build, test 474) | 2,478 Rust / 0 | 268 / 0 | no |
| tools T24 | VM-only PowerShell | `vm/scripts/Build-VerbatimWindows11Image.ps1` 347, `Initialize-VerbatimHarness.ps1` 289, `Initialize-VerbatimBaseImage.ps1` 60 | 696 PowerShell | 0 | no |
| tools T24 | Packer and answer files | `verbatim-windows11.pkr.hcl` 97, `variables.pkr.hcl` 95, `example.pkrvars.hcl` 14, answer file 138 | 344 | 0 | no |
| tools T24 | guides | `vm/README.md` 134; `books/dev/src/how-to/vm.md` 284, its entries in `books/dev/src/SUMMARY.md` 19 and `books/dev/src/how-to/readme.md` 57 | 418 document lines | 0 | no |
| tools T24 | the vendored `vm/vendor/ffmpeg/ffmpeg.exe` used by `deploy.rs`, with its `.gitattributes` LFS line | as named | binary | 0 | no |
| scenarios 20 | the topical groups, read only by `cargo xtask vm test --group` | `crates/verbatim-e2e/src/registry.rs` 250-300; `xtask/src/vm/test.rs` 183 | about 120 | 0 | no |
| (added) | an endpoint option on `cargo xtask e2e`: use an agent already running at an address, start none | `xtask/src/local_e2e.rs` 175 (where `VERBATIM_E2E_ENDPOINT` is set today) | 0 / about 20 | 0 / about 10 | no |
| (added) | loopback as the agent's default bind address; an explicit address for a VM, used only on an isolated virtual switch | `crates/verbatim-agent/src/main.rs` 31-37 (`#[arg(long, default_value = "0.0.0.0")] bind_address`, with a doc comment justifying it by the Hyper-V guest) | about 6 changed | 0 | no |
| (added) | a short guide section on copying a build into a VM and starting the agent there | replaces `vm.md`; see the architecture amendment below | text | 0 | no |

What stays of `vm/scripts`, 300 lines that serve other commands: `Enable-VerbatimCrashDumps.ps1` (101, cited by `crates/verbatim-e2e/src/scenario.rs` 139), `Register-VerbatimParkTask.ps1` (88, `cargo xtask park`) and `Register-VerbatimTraceTask.ps1` (111, `cargo xtask trace`). The deploy list in vm/deploy.rs 518-536 that tools T10 names goes with the harness. The run summary's only reader outside its module is `xtask/src/vm/test.rs` 312 and 498 (harness F8), so after this build the summary format has no outside reader; build 6 replaces it.

D3 as it stands (`books/dev/src/explanation/architecture.md`, decisions list):

> **D3, the end-to-end suite runs wherever the harness reaches; CI is hosted runners only.** The suite talks to the in-guest agent at an address, on a hosted runner, a developer's machine or VM, or a local Hyper-V VM (section 14). CI runs it silently on every change, recorded, with no self-hosted runners, no nested virtualization and never NVDA; a snapshot restore is always an explicit request. Why: a suite needing no VM runs in CI and on the development VM alike. Decided 2026-07-13. Amended 2026-10-01.

Section 14 as it stands begins:

> ## 14. VM harness (the interactive loop, per D3)
>
> `cargo xtask vm <cmd>` drives a Windows guest behind a `Host` trait that covers hypervisor lifecycle only: exists, start, stop, snapshot, restore, delete, and the guest's address. `HyperVHost` is the only implementation, kept for contributors who test in a local VM; the maintainer runs the suite runner-direct on a development VM instead (D3).

It continues with the transport (PowerShell Direct and the agent over TCP), the guest's provisioning, the verbs `create`, `start`, `stop`, `restart`, `restore`, `deploy`, `test`, `logs`, `connect` and `delete`, a paragraph on audio, and the interactive-session rule.

The amendment both need, for the design to settle in wording: D3 drops "or a local Hyper-V VM (section 14)" and "a snapshot restore is always an explicit request", says the suite talks to the agent at an address, loopback by default, or the address of another machine or VM where the developer started an agent, on an isolated network, and records Dickson's ruling of 2026-11-20 with its one sentence of why ("since no one uses it"). The writing convention says a decision of record carries no amended-on line, so the existing "Amended 2026-10-01" is not followed by another (documents D16 flags the existing ones; build 7 removes them). Section 14 keeps its number, so that citations of later sections hold, and becomes the guide to driving a Verbatim on another machine: copy a build in, start the agent with an explicit bind address, run `cargo xtask e2e` with the endpoint option. Its audio paragraph and the interactive-session rule stay, since they hold for any machine. CLAUDE.md's `cargo xtask vm` paragraph and its pointer to `vm.md` go to the process revision.

Leaf-crate group:

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| speech F1 | application text can issue commands to eSpeak NG (U+0001) | `crates/verbatim-synth-espeak/src/lib.rs` 717-741, 869-873 | 10 net | 0 / 15 | yes, only for text holding U+0001, towards NVDA; after its capture |
| speech F15 | two SSML writers that drift; one escape for both drivers is F1's fix | espeak `lib.rs` 838-873; onecore `lib.rs` 484-538 | 30 / 0 | 0 | only through F1 |
| speech F5 | an utterance whose post is refused never ends | `crates/verbatim-speech/src/manager.rs` 478-481, 487-490, 897 | 0 / 8 | 0 / 30 | no; a stalled say-all ends |
| speech F3 | the speech observer wrapped twice behind an always-`Some` option | `manager.rs` 348-351, 729-837; app `speech_events.rs` 37-121 | 150 / 0 | 0 | no |
| speech F6 | dead and test-only speech API | `manager.rs` 584-590, 528-539; `driver.rs` 168-183; onecore 682-690; `registry.rs` 91-95 | 105 / 0 | 95 / 0 | no |
| speech F8 | the presenter seam serves only tests | `manager.rs` 133-137, 628-635 | 30 / 0 | 45 replaced | no |
| speech F13 | the settings mirror's active synthesizer with a made-up default | `crates/verbatim-speech/src/host.rs` 178-185 | 10 / 0 | 0 | no |
| speech F14 and settings R3 | `#[non_exhaustive]` inside one workspace hides unspoken variants (one item: the same attributes) | speech `theme.rs` 577; model `speech.rs` 63, 141, 213, 298; `event.rs` 1201; `theme.rs` 94, 775; `tree.rs` 24; i18n `lib.rs` 765, 910, 1096, 1261, 1327, 1398, 1458 | 20 / a `Role::Unknown` arm | 0 | no |
| speech F2 | the capability fallbacks serve no registered speaking driver | `manager.rs` 1464-1527, 1578; `driver.rs` 148-212, 401-415; `hosting.rs` 52-66 | kept: Dickson, 2026-11-20, "This could be needed" | 0 | no |
| speech F7 | trailing and inner silence trimmed where NVDA trims only leading silence | `crates/verbatim-speech/src/trim.rs` 1-9, 97-109 | 90 / 0 | 35 / 0 | gaps only, towards NVDA; after its recording |
| settings D1 | a settings file that fails to load is overwritten with defaults; the session's themes and sounds looked up under the temporary folder | `crates/verbatim-app/src/main.rs` 125-126, 268-269, 565-584; `saver.rs` 46-49, 133-158 | 5 / 20 | 0 / 25 | yes: NVDA's message box at start; its words after the capture |
| settings D2 | the hidden frame's title is localized; a second launch looks for a fixed title | `crates/verbatim-gui/src/lib.rs` 211-213; `single_instance.rs` 49-52 | 2 / 0 | 0 / 5 | no |
| settings D3 | theme file errors shown in hard-coded English | config `themes.rs` 133-151; `theme_panel.rs` 481 and six more | 0 / 30 | 0 | no |
| settings D4 | a runtime lookup shows Fluent's "No localization" text | i18n `lib.rs` 241-243 | 0 / 10 | 0 / 15 | no |
| settings R1 | one function per message in i18n | i18n `lib.rs` 248-720 | 250 / 0 | 0 | no |
| settings R2 | `ControlPlan` is a third copy of each Speech setting | `crates/verbatim-gui/src/plan.rs` 12-135 | 100 / 0 | 60 net | no |
| settings R4 | a sound's gain computed in two crates; the earcon mapping both ways | `theme_panel.rs` 627-632, 927-941; speech `theme.rs` 197-203 | 20 / 0 | 0 | no |
| settings R5 | modifier keys and layout declared twice | config `lib.rs` 98-152; `verbatim-input/src/state.rs` 49-80; `scripts.rs` 22-38 | 40 / 0 | 0 | no |
| settings R6 | `ConfigValue` mirrors `SettingValue`; an out-of-range number dropped silently | config `lib.rs` 214-223; app `speech_settings.rs` 34-38, 59-61 | 25 / 0 | 0 | no |
| settings R7 | GUI lifecycle facts held twice | `lifecycle.rs` 64-80; gui `lib.rs` 242-262, 464-479 | moved to build 10: folding the page objects into the pure `Lifecycle` is not the 15 lines recorded, and build 10 rebuilds those pages from one table of rows | 0 | no |
| settings R8 | bounded settings clamped at every reader | model `settings.rs` 87-126 and five readers | about 0 net (newtypes) | 20 / 0 | no |
| settings R9 | the settings dialog's key enums tested twice across the bridge | gui `lib.rs` 802-838 | 0 | 41 / 0 | no |
| settings R10 | three atomic writes; `persist` returns an error it never has | config `lib.rs` 571-578; `themes.rs` 652-659; `package.rs` 98-110 | 20 / 0 | 0 | no |
| settings R11 | dead and string-typed i18n wrappers | i18n `lib.rs` 232, 287-299, 323 | 20 / 0 | 8 / 0 | no |
| settings R12 | the settings preview example nothing runs | `crates/verbatim-gui/examples/settings_preview.rs` | 0 | 259 / 0 | no |
| settings R13 | stale comments and an unused default | model `settings.rs` 23, 62; `gui.cpp` 920-922; app `main.rs` 571-572; config `lib.rs` 281-282; `verbatim-ext/src/lib.rs` 3-5 | 3 / 0 | 0 | no |
| settings P2 | profile layering with no profiles (section 9: delete) | config `lib.rs` 255-403, 445, 475, 526-527, 553 | 120 / 0 | 103 / 0 | no |
| settings Q1 | the Terminal page applies only what changed (section 9: write all four on OK, as NVDA's panels do) | `terminal_panel.rs` 11-16, 35-106, 121, 175-183; app `main.rs` 1428-1433 | 45 / 0 | 40 / 0 | nothing spoken at OK; the setting's later effect changes in one sequence |
| settings Q3 | the `AttachThreadInput` fallback with no recorded case | `crates/verbatim-gui/src/foreground.rs` 42, 120-133 | kept: removal needs a measurement no one has asked for | 0 | no |

Also in this build: the unused `proptest` dependency in `crates/verbatim-core/Cargo.toml` 11, a side note of tools T4.

### Tests to add

- `cargo xtask e2e` and the test binary: a run with filters that selects no trial fails (tools T1); `measure` exits with failure naming failed scenarios (tools T2).
- One marker for logged defects: a `tracing` target such as `verbatim::defect`, the gate matching the marker and the panic text, and a 10-line test of the marker in place of `every_defect_message_is_found` (harness F1).
- The agent: an `accFocus` failure is an error; a lost duplication or a size change fails the capture (harness F4, F5), about 40 lines.
- The NVDA client: the add-on's reply shape parses, or fails on a missing field (harness F6).
- The reducer loop: pongs queued behind a past due time end no outpost, through the real loop or a function taken out of it (supervisor F2), about 40 lines, replacing `a_reducer_held_past_three_ping_intervals_ends_no_outpost` and its `PINGS_ENV` plumbing.
- The control server: `has_speech_subscribers` is false once the history has closed with no subscriber (model 11).
- eSpeak NG: U+0001 followed by `V` in the driver's test binary (speech F1).
- Speech: a refused post ends the utterance as failed, through the `PortRefusesPost` fault point (speech F5).
- Settings: a bad `settings.toml` is kept as `settings.toml.corrupted.bak` and the saver's first write does not touch it (settings D1); the frame title equals the single-instance constant (settings D2); every registered synthesizer's label keys resolve (settings D4).
- The leaf group's test removals and replacements listed in the settings and speech records' test sections: config's tautology and layout tests, i18n's three restating tests, the speech tests named under F6, `a_cancel_ends_current_and_queued_speech`, the order-sorting allowance in `utterances_with_marks_cut_off_are_reported_with_their_marks` asserted in its fixed order, `hosted_espeak_ng_places_a_mark_within_one_synthesis`, and `text_segment_defaults_to_utterance_language`.

### Rulings and defaults it rests on

- Dickson's Hyper-V ruling, quoted above.
- Section 3 of REPORT.md, "Tooling follows the evidence": `cargo xtask ci` reports every failure in one run; source files check out with one line ending (`* text=auto eol=lf`; the index is all LF, so nothing is renormalized); the dead nightly workflow is deleted.
- Section 9's smaller rulings: unused profile code and the settings example deleted (settings P2, R12); the Terminal page writes all four values on OK (settings Q1); the "Configuration File Error" message matches NVDA after a capture (settings D1); the manager's fallbacks for drivers without index marks or inline pitch kept, Dickson having objected to their removal (speech F2, 2026-11-20: "This could be needed"); trailing and inner silence matched to NVDA after a capture (speech F7).
- Dead code and unreachable fallbacks go.
- tools T22 and settings Q3 are kept: the first because no line is saved and the allow-lists are a recorded enforcement (`ownership.md` 24), the second for want of a measurement.

### What it must not do

- Keep the 300 lines of `vm/scripts` that serve park, trace and crash dumps, and keep the `VERBATIM_E2E_ENDPOINT` path, which GitHub's runs use on every push.
- Keep the `Group` values the planner reads (`Measure`, `Demo`); only the topical groups go.
- tools T1: the guard belongs in the test binary's `main` (filters given, no trial left), after which CI's list check can go.
- tools T12: `Agent` must take a port and honour `CARGO_TARGET_DIR` before `demo` moves onto it, and `the_agent_comes_from_cargo_target_dir_when_set` moves to `agent.rs` rather than going; with `CARGO_TARGET_DIR` set, `cargo xtask e2e`, `measure` and `nvda` start a stale or missing agent today.
- tools T8: the determinism step needs the tier step's covered set; return it alongside the tier offences rather than dropping the dependency.
- facts M6: remove only the `crates/verbatim-core/**` trigger; model, text and reader-rules are built through `verbatim-uia` and `verbatim-win32`.
- supervisor F2: swapping `expire` and `on_command` inside one pass does not fix it; every command already waiting is handled before `Due` is queued. Do not make the owner thread judge outposts' liveness, which would change the 2026-11-15 ruling at the build 1 design's line 364.
- settings D1: NVDA resets only a file it cannot parse; keeping the other values of a file with one bad value is M8.3's work and is not built now.
- settings D2: a second instance does not run beside the first; it exits after the 2,000 ms mutex wait.
- settings R1: a `t!` macro keeps the compile-time id check only if each calling crate gets its own `i18n.toml`; the i18n tests of formatting and mapping logic, and `cycling_wraps_in_both_directions`, stay.
- settings R9: do not move cxx's open enums into the router; only the duplicate bridge test goes.
- settings R10: `lib.rs` 533-537 handles real write failures and stays.
- speech F2 is not built: the fallbacks stay (Dickson, 2026-11-20).
- speech F1: NVDA's other replacement, `[` to ` [`, was not verified by the record; do not add it without checking.
- harness F6: the agent client's skipping of a reply with another request's id is deliberate (the dropped claim in the harness record) and stays.
- harness F16: the five-minute sweep of finished launches is not shown removable; only the merge of the two maps is (and that is build 6's).

### Size budget

About 4,250 product lines of Rust removed (the VM harness 2,478, tools 415, settings 665, speech 540, the topical groups 120 and smaller items) and about 150 added; about 1,650 test lines removed and about 250 added. Beside them about 1,000 lines of PowerShell, Packer and answer files, 300 of YAML and about 420 of guides go.

### Done when

- `git grep -n "xtask vm\|HyperVHost\|vm-smoke"` over `crates`, `xtask`, `.github` and `books` finds nothing but the new section 14's history-free text; `cargo xtask docs` passes with `vm.md` gone.
- `cargo xtask e2e conhost_typng` fails; `cargo xtask measure` with a failing scenario exits non-zero and runs all 15 silent measurements by default.
- `cargo xtask ci` with a deliberate rustfmt and Clippy failure reports both in one run.
- The agent with no `--bind-address` listens on 127.0.0.1 only; one scenario runs through `cargo xtask e2e` with the endpoint option against an agent started by hand.
- Every test listed above exists and fails on the defect it names when the fix is reverted.
- The captures named above are made at the pin and the two speech changes match them; the GitHub run of the push is read in full.

## Build 2. The dialog read once shown, by postponement, with the queue's stop hooks made one judge

### Purpose

The first proof: rebuild the dialog read once shown at about 400 product and 340 test lines, against today's 1,560 to 1,590 product lines (and 394 in mockapp) and 2,190 test lines with a 425-line fixture. It keeps today's words in every sequence a test states, holds no key, and in the same code takes the queue's remaining stop hooks down to one judge.

### Before the design: measure, capture or record

Captures at the pin, since spoken expectations come only from captures (REPORT question 1 and dialog section 5):

- a typed character pressed while a dialog is hidden;
- a caret key pressed while a dialog is hidden;
- a modifier alone pressed while a dialog is hidden;
- Tab pressed while a dialog is hidden;
- a tray menu opened over a hidden dialog-based main window;
- a message box raised while its owner dialog is hidden;
- a dialog never shown.

Recordings with `verbatim-event-record`: Import a Theme again under the new protocol, so that `crates/verbatim-core/tests/fixtures/dialog_shown_import.jsonl` can be re-recorded (the present fixture cannot replay under any rebuild); and the real applications behind the message-box and tray cases, so that the reducer tests feed recorded orders.

One reducer test, at no cost, before anything else: whether speech made inside a key's entry is fenced by that key's cut (dialog section 5). Its answer decides how the key's read is stamped.

Measurement named by the dialog study for the listener's tell: the rate of `EVENT_OBJECT_SHOW` on (`OBJID_WINDOW`, `CHILDID_SELF`) across the desktop in ordinary use, how many name the foreground window, and the listener's processor time per told show. With the foreground-only tell the cost is one `GetForegroundWindow` per show and a tell only for a window shown while in front, so this is a check on the chosen shape, not a choice between shapes.

### What each process does

- The listener: its hook already hears every window object's show (`crates/verbatim-ia2/src/hook.rs` 198, 1035-1042). It tells Core of a window object's show only when that window is the foreground window, as an ordinary tell. It keeps no waiter, token or record of requests. This adds a local drop that is neither one of NVDA's local checks nor a drop on an event's own cached class and automation id, which ADR 0003 must state.
- The outpost: a focus read that meets a hidden dialog answers so (the `HiddenDialog` reply is the one part of today's protocol kept). About 75 lines.
- Core: keeps one fact, the postponed dialog's window. A read that meets a hidden dialog ends its entry and sets the fact. At the show tell's turn for that window, Core reads the current focus, as the hung-window recovery already re-takes. A key at its turn reads the dialog hidden first, and that read's speech carries the postponed entry's older fence, so a typed key is heard alone as today.

### The one fact and its state table

The fact: `postponed: Option<WindowHandle>`, `None` or the hidden dialog's window. The rows below are the cells the design's table must hold; each says what the rule does in each state. Speech is today's where a test states it, derived otherwise.

| Rule (input at its turn) | With `None` | With `Some(dialog)` |
| --- | --- | --- |
| A foreground or focus read answers "hidden dialog" | becomes `Some(dialog)`; the entry ends, nothing said | stays; the entry ends (the field's own read postpones too) |
| The listener's show tell for `dialog` | nothing to do | re-take reading the current focus with the hidden check; speaks `DIALOG_SPOKEN`; becomes `None` at the commit |
| A show tell for another window | nothing | nothing |
| A key that cuts speech (typed character, caret key, Tab) | ordinary | reads the dialog hidden first and commits it under the postponed entry's older fence, then handles the key; heard: the key alone, as today; becomes `None` |
| A modifier alone (joins no queue entry) | ordinary | stays; at the show the re-take speaks the dialog with its text (accepted change: silent today) |
| A focus commit in another application, or a menu start committed | ordinary | becomes `None`; nothing said for the dialog (option C kept) |
| The foreground moves to another application | ordinary | stays, the gate closed while another window is in front; the other application's commit clears it |
| A message box owned by the hidden dialog | ordinary | the box is read and spoken at its turn; its commit clears; after OK the dialog's foreground read finds it shown or postpones again |
| A focus elsewhere in the same application, refused by its read | ordinary | stays; the show's re-take speaks `DIALOG_SPOKEN` |
| A focus elsewhere in the same application that executes | ordinary | becomes `None`; "Rename... button" only, as today |
| A background application's entry that speaks | ordinary | spoken at its turn, before the dialog (accepted change: after it today) |
| `dialog` destroyed | nothing | becomes `None`; nothing said |
| The application hangs while hidden | ordinary | no sound until something reads the window (accepted change) |
| The outpost generation ends | ordinary | stays; the re-take postpones again; the show's re-take speaks |
| The listener generation ends | ordinary | a re-take when the new listener is ready, so a show missed meanwhile is not lost |
| A dialog never shown | ordinary | stays; nothing held, nothing said; a later key reads it hidden |
| Events on the old focus (a name or state change of the owner's button) | ordinary | judged against the old focus before the dialog; the study names this as unaddressed, so the design states it and its review checks it |
| A UI Automation focus in the hidden dialog | ordinary | the show's re-take reads the current focus; expected the same object, unproved until the re-recorded fixture shows it |

The three accepted differences, as REPORT decision 3 lists them and Dickson accepted: a modifier pressed alone while the dialog is hidden (nothing today; the dialog with its text at the show); a background application's speaking entry before the dialog (after it today); a hang while hidden plays no "not responding" sound until something reads the window. It also stops holding keys for 13 seconds behind a dialog never shown.

### Inventory

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| dialog decision 1 | the shape: postponement (ruled) | as below | 1,560-1,590 and 394 mockapp / 380-400 | 2,190 and the 425-line fixture / about 340 | three accepted differences |
| dialog decision 2 | the bound of a parked read | none: postponement sets no bound | 0 | 0 | no |
| dialog decision 3 | the speech differences | the table above | 0 | 0 | as accepted |
| dialog decision 4 | the listener tells only the foreground window's show | `crates/verbatim-ia2/src/hook.rs`; listener | 0 / about 10 to 40 | in the 340 | no |
| dialog, study A's error | the key's read stamped with the postponed entry's older fence | Core `keys/entry.rs` 136-143; `entries.rs` 616-617 | in the 400 | about 40 with the cross-process order | keeps today's |
| dialog, replay | the replay test finds its place by substring | `crates/verbatim-core/tests/replay_validity.rs` 1119 (`label.contains("Import a Theme")`) | 0 | replaced by an exact place | no |
| dialog, fixture | the Import fixture re-recorded under the new protocol | `crates/verbatim-core/tests/fixtures/dialog_shown_import.jsonl` | 0 | re-recorded | no |
| queue F1 | four hooks judge whether the front's request stops | `event_queue/mod.rs` 228-275, 392-402; `inbox.rs` 386-450, 452-472, 607-619; `driver.rs` 148-167 | about 100 net | about 60 | no |
| queue F11 | `entries::unsettled` re-derives the settle judgement | `entries.rs` 1317-1330; `reduce.rs` 94-97 | 15 / 0 | 0 | no |
| focus Q2 | the wait's carry-over through merges and the limiter's cap | `shown.rs` 228-282, 373-390; `msaa_run.rs` 353-358; `uia_focus.rs` 98-100 | 110 / 0 | 135 / 0 | the corner's speech follows the show's re-take |
| listener L7 | the pipe thread mirrors the show waiters | `crates/verbatim-listener/src/listener/shown.rs` 175-255 | 45 / 0 | 55 / 0 | no |
| mockapp M4 (Q1) | `late_dialog`: three variants never sent, its recording check claimed but not made (section 9: goes with build 2) | `crates/mockapp/src/late_dialog.rs` (359); `msaa_events.rs` 820-870 | 359 / 0 | 42 / 0 | no |
| documents D13 | the ending rule written in nine places | `shown.rs` 11-17, 107-127; `focus.md` 106, 108, 110; `verbatim-core.md` 207-219; `docs/parity/focus-context.md` 166-173; ADR 0003 225; `verbatim-model.md` 305-306; `request.rs` 1152-1154; `test-coverage.md` 122 | each rewritten once, the guides pointing at the explanation page | 0 | no |

What goes, by file, from today's build (`git diff --numstat bc99bc40^ 870bda79`):

- Model: `Request::AwaitShown`, `Reply::Shown`, `CancelReason::LeftHiddenDialog` and their arms in `unstamp`, `named` and `visit_nodes` (`request.rs` +52). Whether `HiddenRoot` and the `hidden` field on the three focus reads stay is the design's to settle; the dialog study keeps only `HiddenDialog`.
- Core: `focus/shown.rs` (394) rewritten at about 90; `focus/shown_tests.rs` (1,247) replaced; the after-wait re-take; `ended_wait` on two entry kinds; `stopped_at_once` in the checkpoint; the queue's diffs (driver +76, inbox +34, limiter +47, mod +31, outposts +19, request +14, table +44): the hooks `stops`, `own_foreground_rule`, `merged`, `stops_as_it_begins` and `Table::answer_cancelling`; the 13-second due time of the wait.
- Listener: `listener/shown.rs` (255), listener-logic `shown.rs` (239, 136 of them product), the wait's part of `listener.rs` (+343), `event_thread.rs` (+142) and `hook.rs` (+57, less the foreground-only tell).
- Outpost: what `asked.rs` (+82) added beyond the `HiddenDialog` answer; `hidden_root_tests.rs` cases that test removed paths.
- Mockapp: `late_dialog.rs` and its test; the crate guide's "Checked against the recording of Import a Theme" claim and the test comment matching it.
- The test-coverage row 122, which lists the stand-in's test as coverage.

### The sentences of ADR 0003 and the focus design that change, quoted

From ADR 0003 (`books/dev/src/reference/adr/0003-readers-tell-core-asks.md`):

- "The listener has one kind of request waiting on an event, which Core can cancel: 'tell me when this window is shown', kept on its WinEvent thread from the request to its one answer (Dickson, 2026-11-19)". It goes, replaced by a tell of the foreground window's show.
- Under "Limits accepted": "so does a hidden dialog that is never shown, with no key pressed and no foreground, menu or focus elsewhere (Dickson, 2026-11-19)". It goes: nothing is held.
- "for a hidden dialog, the wait for its show reads the focus once, at the show" becomes the re-take at its show.
- "the listener's requests to be told when a window is shown (Dickson, 2026-11-19)", under "Readers tell, Core asks", goes.
- "A reader drops an event only by NVDA's local checks that cannot hang ... and by the listener's drops on an event's own cached class and automation id" gains the foreground-only show drop.
- "What Verbatim keeps beyond NVDA" gains the show tell and the postponement.
- The reason given at line 46, "only the listener's desktop-wide hook hears that show, since an outpost holds no WinEvent hook", still holds and explains why the tell is the listener's.

From the focus design (`books/dev/src/explanation/design/focus.md`, "A dialog read once it is shown"):

- "Core asks the listener to tell it when the window is shown ..."
- "The wait ends early when the user is elsewhere, a condition on the queue that holds whenever the input making it true arrived ..." (line 106)
- "A key pressed while the dialog is hidden, any key whose press cuts speech, a modifier alone included, ends the wait too, but does not skip the dialog ... When a key and the user being elsewhere both hold, the user being elsewhere wins." (line 108)
- "A dialog never shown, with no key and no other ending, holds the queue until the due time, 13 seconds, then ends unspoken." (line 110)
- "A focus that ended a wait carries the dialog with it, through a merge or the limiter's cap too; should its run then execute nothing while the dialog is still in front, Core reads the current focus as a re-take, which waits for the show in turn." (focus Q2)

The parity ledger's ruled list under "A dialog's text" (`docs/parity/focus-context.md` 166-173) changes with them. Each new sentence records "(Dickson, 2026-11-20: "For the dialog: ok")".

### The four stop hooks made one judge

`withdraws`, `stops`, `own_foreground_rule` and `unneeded` (queue F1) each judge at their own point. Postponement removes `stops`, `own_foreground_rule` and `merged` outright, since nothing at the front waits for anything but an answer. What remains becomes one function, `ends(facts, front, request, view, cause)`, with the causes `Begins`, `Arrived(entry)`, `Foreground` and `Settled`; `stopped_at_once` merges with `at_once`; `entries::unsettled`'s copy of `unneeded` (queue F11) goes. A new ending then is a match arm, not a hook and a call site.

### Tests kept and added, within about 340 lines

The dialog study's list for postponement (about 300) and about 40 for the fence and the cross-process order:

- the plain show; a dialog shown at its foreground (asks nothing);
- a typed key while hidden (heard: the key alone), a caret key, Tab (judged from the field), a modifier alone (the dialog with its text at the show);
- the foreground leaving for another application; a message box owned by the hidden dialog, in both orders; a dialog closed by hiding; a tray menu over a hidden dialog window;
- a focus elsewhere refused by its read; a focus elsewhere that executes; a background entry that speaks, before the dialog;
- a dialog never shown (nothing held); destroyed while hidden; the application exiting; a hang while hidden (no sound);
- the outpost ending and the listener ending while postponed; a UI Automation focus in a hidden dialog;
- the cross-process order: the show tell before and after the hidden answer, fed as the listener and the outpost send them, and the re-recorded Import fixture replayed at an exact place;
- the listener's foreground-only show tell; the outpost's hidden answer;
- one table test of `ends` over its four causes, replacing the toy hook overrides (about 60 lines) the queue record names.

### Rulings and defaults it rests on

- Dickson's "For the dialog: ok" on REPORT decision 3, and the strategy ruling that makes this the first proof.
- Section 9: the late-dialog stand-in's variants go with this build (mockapp Q1).

### What it must not do

- Do not build study A as written: its key's read is stamped with the key's own number, so the dialog without its text and its field become audible before the echo. Stamp it with the postponed entry's older fence (dialog section 1).
- Do not give the show's re-take `Fence::default()` on the key path; the modifier-alone sequence is the only one meant to become audible.
- Do not adopt the parked read or the inline wait: they break the ruled option C in two sequences, silence caret keys held past 150 ms and tie Import's speech to the machine's speed against a 300 ms bound close to Import's 221 ms.
- In the one judge, keep the begin-time judgement: dropping it would send a request and then cancel it where today it is never sent (queue F1 correction). `withdraws` must still see the arriving entry before the limiter merges it.
- Keep `HiddenDialog`; nothing else of today's protocol needs to survive.
- Do not count the change as a fix round of today's mechanism: it is a rebuild with its own design and review.

### Size budget

About 400 product lines for the behaviour (380 to 400, the dialog study's estimate with the fence correction and the foreground-only tell), and the one judge about 100 net lines removed. About 340 test lines. Neither reaches 1.3 times NVDA, which has no code that waits; about 400 lines is the floor for today's speech.

### Done when

- `git grep -n "AwaitShown\|LeftHiddenDialog\|own_foreground_rule\|answer_cancelling\|late_dialog"` over `crates` finds nothing.
- The product and test lines of the behaviour, counted by `git diff --numstat` against ba89dca7, are within the budget; the count is written into the build's report beside today's 1,560 and 2,190.
- The seven captures exist at the pin and the tests that state those sequences expect their speech; the three accepted differences are stated in ADR 0003 and the focus design with Dickson's ruling.
- `quit_inside_a_file_dialog`, the one scenario the records name on this path, passes 3 runs locally, with the other scenarios on the path found from the code; the GitHub run is read.

## Build 3. Core: one owner per fact, dead paths out

### Purpose

Put each fact Core holds twice into one record and one rule, which fixes two confirmed defects (a control unspoken after an outpost is replaced, a mouse return silenced), and remove the bridges, copies, dead protocol and unreachable fallbacks left by the rebuilds of the focus path. It also removes the app shell's copies of Core's generation record and readiness.

### Before the design: measure, capture or record

- Capture: Notepad's Find dialog, "Find what" focused, Tab to "Find Next" (for focus F1's spoken words after the fix).
- Capture: a click speaking an MSAA list item, a click into an application read through UI Automation, a click back on the same item (focus F2; the mockapp replay says "Alpha", "1 of 3"; NVDA's words need the capture).
- Capture: a popup menu from an edit closed with no focus event while the focus moved (focus F12's corner).
- Capture: holding NVDA+Numpad 5 past the auto-repeat delay (keys M16).
- Capture: a background progress bar's value change waiting while "report background progress bars" is turned off, then a foreground change (keys M10's question Q5).
- Capture: a split MSAA pair with a UI Automation focus between them refused as read through MSAA (focus Q3).
- No measurement: the build adds no thread, timer or message on the event or speech path; it removes the listener's pong wake (supervisor F7).

### Inventory

One record of each fact:

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| focus F1 | the committed focus in two records joined by different rules (defect) | `focus/execute.rs` 178-230, 481-495; `focus/object.rs` 18-22; `versions.rs` 131-172, 222-240; `reduce.rs` 1842-1910 | 100-150 / 0 | 0 / 40 | yes: the silenced focus is spoken; words after the capture |
| focus F2 | "a failed try ends the fence" at nine sites with three rules (defect) | `msaa_run.rs` 327-347; `menus.rs` 134-246; `retake.rs` 288-297; `uia_focus.rs` 142-148, 194-210, 274-303 | 45 / 0 | 230 / 50 | yes: the mouse return speaks the item |
| focus Q3 | a reader's refusal on the UIA path clears the fence (section 9: leave it) | `uia_focus.rs` 295-303 | in F2 | a row of F2's table | the split pair's item said once; after the capture |
| focus F12 | the menu check compares a cloned record by value (defect) | `menus.rs` 100-104, 188 | a few | 0 | one corner, after the capture |
| queue F4 | one foreground report enters Core twice, with the own-window rule ADR 0003 removed | app `main.rs` 1163, 1200; `foreground.rs` 41-62; `reduce.rs` 213-216 | 80 / 0 | 25 / 0 | no |
| supervisor F6 | one foreground report rebuilt as three types; progress re-wrapped | `owner.rs` 722-751; supervisor `lib.rs` 215-237; app `main.rs` 1154-1200 | 60 / 0 | 0 | no |
| queue F3 | Core's state holds an empty queue while a step runs (defect, no speech shown) | `state.rs` 613-617, 1082; `entries.rs` 1075 | about 0 | 0 / one test | no |
| queue F6 | a reader's readiness kept in three places | `driver.rs` 228-230, 321-334; `inbox.rs` 207-209; `record.rs` 116-118 | 30 / 0 | 0 | no |
| supervisor F1 | one generation in four records; the app's live map and status map (the owner's copy waits for M6.2) | `live.rs` 52-57, 112-120; app `main.rs` 987-994, 1027, 1045, 1237-1244 | 70 / 0 | 20 / 0 | no |
| supervisor F3 | readiness computed twice; Core's copy read only by a test | `state.rs` 626-633, 792-803; `harness.rs` 57-66, 105-118 | 50 / 0 | a few | no |
| queue F7 and supervisor F5, F9 | backoff due time and liveness constants written twice; `Timings` configures nothing (one item) | `policy.rs` 13-27, 77-139; `record.rs` 22-56, 152-163; supervisor `lib.rs` 83-122 | 95 / 0 | 10 / 0 | no |
| queue F12 | two rules for sending a held view | `held_tokens.rs` 94-142; app `live.rs` 32-47, `main.rs` 1531-1551 | 40 / 0 | 0 | no |
| queue F17 | the shell's copy of the listener generation | `held_tokens.rs` 60, 106-151 | 30 / 0 | 20 / 0 | no |
| queue F8 and keys M7 | the speech fence and arrival fact on every entry kind (one item: the queue slot holds them, set once at the join) | `entries.rs` 385-431 and 17 constructors; `changes.rs` 90-97 | 40-70 / 0 | 0 | no |
| queue F16 | `kept_by` restates `told_token` | `entries.rs` 732-744; `announcements/mod.rs` 283-292 | 25 / 0 | 0 | no |
| queue F20 | `Live.probing` writable on a waiting request | `table.rs` 50-76 | 5 / 0 | 0 | no |
| queue F2 | the queue generic over `Logic` with mirrored inputs and effects (section 9: keep it generic, emit `QueueEffect` directly) | `inbox.rs` 125-182; `entries.rs` 1359-1390 | 80 / 0 | 0 | no |
| queue F9 | a run is `Vec<E>` and drops non-candidates silently | `limiter.rs` 109-113; `entries.rs` 510-512, 960-970 | 40 / 0, only if an associated type is cheap once build 2 has removed three hooks | 0 | no |
| queue F15 | Core's requests begin by four paths; the `Follow` mapping twice | `driver.rs` 1095-1103; `inbox.rs` 89, 264; `entries.rs` 801-807, 1299-1305 | 40 / 0 | 0 | no |
| queue F19 | the lock key's 30 ms read is a wake outside the queue's (the terminal timer stays for M6.4) | app `main.rs` 1882-1919; `state.rs` 522, 706-719 | 30 / 0 | 0 | no |
| focus F4 | NVDA's class lists copied into Core, already drifted from NVDA's pattern | `focus/backend.rs` 33-171; `verbatim-reader-rules/src/class.rs` | 140 / 0 | 96 / 0 | no in practice |
| outpost 16 | the same copy, noted from the outpost side | as focus F4 | in focus F4 | 0 | no |

The key-press bridge and the request types:

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| keys M1 | presses converted back into the old inputs and run through `reduce.rs` | `keys/entry.rs` 187-210, 266-325; `reduce.rs` 104-108, 156-200, 248-296; `keys/mod.rs` 20-121; model `event.rs` 596-766 | 370 / about 10 | 64 / 0; terminal tests rewritten to presses | no, once the two follow-ups carry their key number |
| queue F5 | the same bridge seen from the queue; dead `Input::Tick` and `Input::MarkReached` | `keys/entry.rs` 266-324; `reduce.rs` 247-281, 104-108; `event.rs` 661-664, 767 | in keys M1 | replaceable | no |
| model 9 | two shapes of a key press and of a focus answer, translated for "today's" code | `request.rs` 1467-1540; `event.rs` 596-770; `execute.rs` 79-120 | in keys M1 and focus F1 | not counted | no |
| keys M2 and model 16 | twelve `Request` implementations of one shape; `Properties` wrapped four times (one item) | `caret_key.rs` 128-205; `caret_tell.rs` 118-148; `changes.rs` 237-310; `selection.rs` 58-121; `report_focus.rs` 55-86; `navigator.rs` 68-166 | 200 / 0 | 0 | no |
| keys M11 | a missing property answer taken as "no states" (defect, reader fault only) | `changes.rs` 207-210, 460-461 | 5 / 0 | 0 / 20 | only under a reader fault |
| focus F5 | read depth written as marker types under two traits | `msaa_run.rs` 31-166, 256-309; `retake.rs` 57-87 | 100 / 0 | 0 | no |
| focus F9 | `judge` and `judge_read` are one match twice | `validity.rs` 316-346 | 25 / 0 | 0 | no |
| focus F10 | the MSAA execute tail twice | `msaa_run.rs` 430-451; `menus.rs` 167-181 | 20 / 0 | 0 | no |
| focus F11 | smaller copies (`Why`, `Outcome` arms three times, the foreground relation, old-dump defaults) | `retake.rs` 33-42; `driver.rs` 861-870; `execute.rs` 148-150; `versions.rs` 162, 170 | 60 / 0 (shown.rs's part goes in build 2) | 0 | no |
| focus F8 | "the run before executed a focus" as a flag in shared state | `state.rs` 591-593; `entries.rs` 501 | 10 / 0 | 0 | no |
| keys M10 | acceptance re-judged at eleven sites; the exempt list and `passes` twice; judging once at the join (keys Q5) | `announcements/mod.rs` 219, 255-326; `changes.rs` 324-335; `entries.rs` 436-457 | 30 / 0 | 0 | the copies no; judging once after its capture |
| keys M12 | the navigator's window patched after the fact | `state.rs` 152-156; `navigator.rs` 10-13; `changes.rs` 517 | 25 / 0 | 0 | no |
| keys M13 | the hook's `Decision` as eight fields with invariants in comments | `verbatim-input/src/state.rs` 234-273 | about 0 | 0 | no |
| keys M14 | smaller copies in details, toasts, kinds re-matched, nine generation lookups | `details.rs` 26-73; `toast.rs` 28 | 60 / 0 | 0 | no |
| keys M8 | duplicate types in the input crates; a press kind never sent | `toggle.rs` 105; input-windows `lib.rs` 143-155; app `strokes.rs` 33-40; `request.rs` 1521; `scripts.rs` 553 | 70 / 0 | 35 / 0 | no |
| keys M9 | modifier state kept three times on the hook thread | input `state.rs` 542, 637, 1010-1018; `hook.rs` 58, 317-330; `typed.rs` 296-306 | 40 / 0 | 40 / 0 | no |
| keys M3 (Q1) | report focus over the sleeping focus (section 9: replace the 2026-11-15 ruling with "says nothing") | `report_focus.rs` 115, 126 | 0 | 0 / about 20 | no; pinned as today's |
| keys M16 (Q2) | auto-repeat not counted as a multi-press (section 9: match NVDA; capture) | input `state.rs` 104-108, 517-520; `docs/parity/input.md` 27-29 | small | small | towards NVDA, after its capture |
| keys M17 (Q3) | the MSAA alert path no known application reaches (section 9: keep; NVDA has it) | `announcements/alert.rs` | kept | kept | no |
| keys M18 and supervisor Q1 | the retry of posts only an out-of-memory system refuses (section 9: keep the hook's, drop the other three and log) | relay `Outbox` `owner.rs` 283-341; `relay.rs` 87-140; `port_queue.rs` 120-150; supervisor `lib.rs` 136-178 | about 65 / 0 (the hook's 95 stay) | about 330 / 0 (`fault_paths.rs`, `relay.rs` 472-521) | no |

Dead code and dead protocol:

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| focus F3 | `focus_rules.rs` mostly dead behind a stale module doc | `focus_rules.rs` 1-14, 24-116, 163-243, 253-264, 316-330; `lib.rs` 98 | 185 / 0 | 130 / 0 | no |
| focus F6 | dead code behind a blanket `expect(dead_code)` | `focus/mod.rs` 5-11 | 50 / 0 | 14 / 0 | no |
| focus F7 and model 18 | `ObjectClass` has one variant | model `request.rs` 731-741; `duplicate.rs` 32, 60, 107 | 30 / 0, or kept | small | no |
| queue F13 | stale module docs, `expect(dead_code)` and six old-dump `serde(default)` | `event_queue/mod.rs` 16-25; `entries.rs` 15-24; `reduce.rs` 207, 258, 413, 550, 665; `shell.rs` 1-12 | 50 / 0 | 0 | no |
| terminals R4 | two old-dump `serde(default)` in the terminal records | Core `terminal.rs` 103; model `terminal.rs` 58 | 2 / 0 | 0 | no |
| terminals R3 | dead read numbers on the wire | model `terminal.rs` 62-69; `text.rs` 1193-1205; outpost `terminal.rs` 33, 272; `raw.rs` arms | 30 / 0 | 10 / 0 | no |
| keys M6 | dead model events, a copied toast class and a hashed toast identity | model `event.rs` 367-402, 463-470, 484-519; `toast.rs` 21, 69, 90-102 | 90 / 0 | 5 / 0 | no |
| model 4 | `NormalizedEvent`'s never-sent variants and constant fields (section 9: delete with a protocol version) | model `event.rs` 278-520 | the variants, `FocusGroup`, `AlertOrigin` / 0 | a few | no |
| model 3 and supervisor Q2 | 12 of 16 `EventTiming` fields never written; 19 of 30 latency stages never produced (section 9: delete with a protocol version) | model `protocol/mod.rs` 25-90; app `latency.rs` 140-330; control `protocol.rs` 378-550 | 45 model, 200 app, 15 outpost, 110 control / 0 | 250-450 / 0 | no |
| supervisor Q3, documents D20 | `StatusInfo::live_re_reads` always zero (section 9: delete with the protocol version) | control `protocol.rs` 1-7, 27-33, 309-314; app `main.rs` 2286-2288; agent `server.rs` 1245 | a few / 0 | a few | no |
| model 5 | dead message parts: `NotResponding` (removed by ADR 0003), `Kind`, `parked_on_hung`, two always-zero listener fields | `protocol/mod.rs` 232-895 (as listed) | 110 / 0 | 180 / 0 | no |
| model 6 | four walks over a message's node ids; one clones every answer | `request.rs` 926-1122; `protocol/mod.rs` 721-781 | 80 / 0 | 0 | no |
| model 8 | three classifications of the outpost variants | `protocol/mod.rs` 620, 888-935; process `message.rs` 204-233 | 50 / 0 | 60 / 0 | no |
| model 10 | protocol fields defaulted for an older peer that cannot exist (the control protocol keeps its defaults) | `protocol/mod.rs` 24 and ten more; `protocol/tests.rs` 353-358 | 15 / 0 | 6 / 0 | no |
| model 12 | element-fact types no message carries | `protocol/mod.rs` 110-167 | 40 / 0 | 0 | no |
| model 13 | dead transport and control-plane code | process `transport.rs` 441-470, 1211; `message.rs` 33; control `protocol.rs` 577-585 | 80 / 0 | 40 / 0 | no |
| model 14 | `outside_the_queue` ends in a wildcard | `request.rs` 1003 | 0 | 0 | no |
| model 15 | `Input::MarkReached` never built; one shared focus-read spec | `event.rs` 767; `request.rs` 273-292, 514-534, 627-645 | 10 / 0 | 0 | no |
| supervisor F4 | `OutpostMessage::Running`, reachable only by a logic bug | `owner.rs` 929-949; app `main.rs` 1000-1015 | 40 / 0 | 0 | no |
| supervisor F7 | the listener's pong wakes the reducer every 3 s and Core drops it | `owner.rs` 822-831; app `main.rs` 1093-1106 | 10 / 0 | 0 | no; one wake fewer every 3 s |
| supervisor F10 | dead and stale pieces (the `pending` item waits for M6.2) | `owner.rs` 271-276; supervisor `lib.rs` 321-329; `writer.rs` 103-115; app `main.rs` 526-532, 1697-1703 | 60 / 0 | 20 / 0 | no |
| supervisor F11 | a wildcard over `Effect` hides any new effect | app `main.rs` 1622; model `event.rs` 1228 | 2 / 0 | 0 | no |
| speech F12 | `FocusValidity { had_focus: false }` behaves as `None` | model `speech.rs` 476, 486-502; `reduce.rs` 711-715, 748-752 | 15 / 0 | 5 / 0 | no |

### Tests to add

- A re-take that keeps an old window entry, then a Tab inside it (focus F1), about 40 lines in `retake_tests.rs`.
- One table of every outcome against the fence, about 50 lines, replacing about 230 lines in seven tests and covering the completion's `Ended` (focus F2 and Q3).
- `held_anchors` called from inside an entry while a terminal read naming an anchor is out, unless the state type change removes the possibility (queue F3).
- A `Properties` reply missing an asked property is refused as undecodable (keys M11).
- Report focus over Core's own sleeping focus says nothing (keys M3).
- The removals tied to each finding as the records list them: `foreground.rs`'s own-window tests, `ListenerView`'s mismatch tests, `focus_rules.rs`'s four tests, `backend.rs`'s eight, the two `Kind` tests and their chains, the older-peer test, `live.rs`'s impossible-sequence test, `fault_paths.rs` and the relay's refused-wake test, and the latency stages' tests.

### Rulings and defaults it rests on

- Two decisions of record disagree on nothing in this build except the verdict, which is build 4's.
- Section 9's smaller rulings: report focus over the sleeping focus "says nothing" (replaces a ruling dated 2026-11-15; his area, by default); auto-repeat matched to NVDA after a capture; an outpost's refusal as read through MSAA leaves the fence after a capture; the MSAA alert path kept; the retry kept on the hook only; the latency ledger's dead stages and the control protocol's never-produced vocabulary deleted with a protocol version (a public contract; his area, by default); the queue kept generic with the model's effects emitted directly.
- keys M10's judging once is not in section 9; the default "match NVDA after a capture" applies, since NVDA stops nothing at a foreground change (keys Q5).
- Dead code, dead protocol and unreachable fallbacks go.

### What it must not do

- The key-press bridge: the table tying speech to a key still serves two announcements. `KeyOrigins` stamps the key number on Report time's clock speech (`Effect::ReadClock`, `Input::ClockRead`) and on lock-key speech (`ToggleKeyPressed`, `LockKeyToggled`). Make those two follow-ups carry the key number first; removing `KeyOrigins` before that changes speech.
- Do not queue lock keys as presses: a lock key never waits behind a held key, as NVDA starts the announcement's timer at the press (`shell.rs` 110-115).
- The key variants in the model: the records disagree on when they can go. Queue F5 says removing them waits for M6.4's terminal tests; keys M1 says those tests are rewritten to presses now. The design chooses one; if the terminal tests' rewrite is not cheap, the model's eight variants stay until build 9 and only Core's bridge goes now.
- Do not merge the readers' last focus into the committed record: ADR 0003 line 100 keeps them as separate facts, and the review dropped that proposal.
- Keep per-candidate fences when the fence moves to the queue slot (`validity.rs` 380, `msaa_run.rs` 442).
- Keep the owner thread's `Ready` stage until M6.2: it starts the spares and its end's log level decides an error sound.
- focus F4: the records disagree. The focus record proposes one home for the class lists (in `verbatim-model`, used by both, or the listener's tell carrying the class verdict); the outpost record (finding 16) says the tier rule forces Core's copy. The design settles where NVDA's class data may live under "NVDA provenance" before moving it.
- focus F7 and model 18: the records disagree. The focus record says remove `ObjectClass` now; the model record says its doc reserves it for Word, Outlook and Internet Explorer and deleting it would be reversed. Under the default, dead code goes, and the milestone that reads those classes adds its own type.
- Do not replace `port_queue` with a standard or crossbeam channel (rejected by `ownership.md` 75 and 80); `stamp_sent` and `ToastId` are live; `UiaSnapshotFact` stays in the model, since the outpost's logic crate is Windows-free.
- model 7 is build 5's; keep `TextReply` as Core's full set until then.
- Keep `ignored_process.rs` and `listener_destroyed.rs` as trials when build 6 folds them; they are not removed here.

### Size budget

About 3,000 to 3,500 product lines removed and about 2,500 test lines removed (REPORT's estimate, about 3,000 and 2,500), with about 150 product and 150 test lines added.

### Done when

- One record of the committed focus: `chain`'s agreement checks, `FocusState`'s second copy and the copied `role` and `own_window` are gone, and the re-take test passes.
- One `tried` rule: `grep -n "tried_and_refused\|refused_or_not_there"` shows one place per answer; the outcome table passes.
- One foreground record and one input; `ForegroundWindow.own` is gone.
- `git grep -n "fn today\|waited_input\|KeyOrigins\|focus_rules::\|DETERMINISM\|NotResponding {"` finds no product use (the last outside `Outcome::`).
- The control protocol's version is raised and verbatim-inspect builds against it.
- The captures named above are made and the three speech changes match them; `cargo xtask ci` passes; the related scenarios found from the code pass 3 runs; the GitHub run is read.

## Build 4. The readers

### Purpose

Give the listener and the outpost one state per fact and one owner per rule: the listener's pending foreground held once (a defect), the delivery-time check as plain code that its tests and mockapp's support both run, Core's cache as the only lasting keeper of a window's verdict, and progress bars read when Core asks, under Dickson's condition. Remove the readers' dead tags, placeholder snapshots, the caret element sweep and mockapp's dead stand-ins.

### Before the design: measure, capture or record

Progress bars asked outside the event queue are an unverified variant: the review's merger did not examine it. Before the design is approved, check and measure:

1. Whether Core's path for requests outside the queue carries a read to the listener's reading thread. Today it serves terminal output reads to outposts: `Logic::wanted` and `facts_answered` (`crates/verbatim-core/src/event_queue/mod.rs` 285-303), used by `terminal/reading.rs` 173-184, with the endings of the waiting-states table's rows 6, 7 and 11. Check that a request owned by Core's facts can name the listener as its reader, and how its due time, hang check and reader-ended endings apply.
2. Where a value-change tell is judged. ADR 0003 gives as a reason that "progress facts are handled at once in Core, outside the event queue". The design must say whether the tell still joins the queue for its ordering against focus changes, or is judged at arrival, and what that means for a bar in an application that has just lost attention.
3. Dickson's condition: a background application's bar is asked for only with "report background progress bars" on; a bar in the application holding attention is asked for either way. Check that Core can tell those apart at the moment it asks, from its foreground record and attention.
4. What the listener must read when asked: its MSAA read (`read_msaa`, `listener/asked.rs` 403-414) reads only the name today, so a bar read needs role, value and place (the listener record estimates about 250 lines added in Core and the listener).
5. Measure, on a real bar and on `background_progress_bar`: the rate of accepted value changes, Core's wake-ups and processor time per accepted change, the listener's round trip (the design records 0.65 ms for a reading thread's read), and the delay of the tone against today. Measure a flood of bar changes against today's listener limits.
6. Check a slow application: no key waits, but the tone waits up to the request's due time (11 s) or its hang check.
7. Capture: a bar in an old console with an incomplete provider (the listener's judge records it as a limit; under Core's verdict its speech needs the capture).

If any measured cost passes a budget of record, the design goes back to Dickson before the build; otherwise it is built.

Other items before the design:

- Capture: a window with no provider probed at 0 s, a provider appearing at 0.55 s, a request at 0.6 s (outpost 1 and Q1; today read through MSAA until 1.1 s, NVDA reads through UI Automation from 0.6 s).
- Capture: 65 caret tells while Core's first caret entry waits (outpost 4).
- Capture: a dialog of 513 objects (outpost 17); activation 9 parents above the navigator (outpost 18); a controller-for read that times out during a suggestion list's selection (outpost 19).
- Capture: a foreground event for a window not yet in front, with no later event within 20 ms (listener L2; what the refused tell loses today).
- Capture or recording: a bar whose window's class is on NVDA's bad list with a provider that is not a proxy (listener L14).
- Recording with `verbatim-event-record`: an Alt+Tab order, to replace the three hand-written foreground ordering tests (`listener.rs` 3236-3505); none exists in the repository today.
- Measure: the listener's processor time on a value-change flood before and after removing the window-read memo (listener L6 raises local calls from 18 back towards 32 per value change; no figure of their time exists).
- Measure wake-ups and processor time: the build moves a timer onto the WinEvent thread and changes messages on the event path.

### Inventory

The listener:

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| listener L2 | the pending foreground held twice; the timer sends the tell before the report (defect) | logic `foreground.rs` 79-82; logic `tell.rs` 160-169; listener `tells.rs` 304-308; `listener.rs` 303-432 | 60 + 130 (the threadpool timer, `PoolTimer`, its raw pointer and four `unsafe impl`s) / 0 | 0 / 40 | towards NVDA; after its capture |
| listener L1 | progress bars as Core's asks, under Dickson's condition | `listener.rs` 1470-1871; `value_change.rs`; `listener/configuration.rs`; logic `focus.rs`; `schedule.rs` 1044-1079 | about 1,300 / 250 (about 1,050 net) | about 1,700 / 300 (about 1,400 net) | no for the two scenarios; the old console's bar after its capture |
| listener L14 | three judges of a window's native UIA in the listener | `listener.rs` 1557-1563, 1827-1851; logic `check.rs` 326-345 | 55 / 0 (the two bar judges go with L1) | 0 | a double report; after its capture |
| listener L15 | bar caps standing in for an application's end | `value_change.rs` 33, 36; logic `focus.rs` 41, 97; `asked.rs` 94 | 30 / 0 | 50 / 0 | no |
| listener L17 | the bar reads' abandon count kept by process id (defect) | logic `schedule.rs` 409-412, 493, 889 | goes with L1's bar deadline, or keyed by `AppKey` | 0 / 20 if kept | towards NVDA (tones restored) |
| listener L5 | `Check` is a step function over optional fields | logic `check.rs` 203-226, 266 onward | 150-200 / 0 | 505 / about 250 | no |
| mockapp M1 | the test support copies the listener's check, bar routing, limiter and MSAA tell | `crates/mockapp/tests/common/core_driver.rs` 484-674, 1190-1414 | -30 (exports from the listener) | 175-345 / 0 | no |
| mockapp M11 | one incident guarded twice; the hand-ordered pair goes once M1 lands | `focus_reports.rs` 2686, 2713 | 0 | 75 / 0 | no |
| listener L6 | the per-event window-read memo and its source-text test | `listener/event_windows.rs` 1-155; `listener.rs` 3556-3611 | 150 / 0 | 55 / 0 (the count test stays with new counts) | no |
| listener L4 | the writer's numbering written once per reader | logic `kept.rs` 128-222; outpost `outbound.rs` 66-139 | 50 / 0 (one copy in `verbatim-reader-rules`) | 40 net | no |
| listener L8 | one release, two paths | `listener/checking.rs` 462-491 | 40 / 0 | 25 / 0 | no |
| listener L9 | the foreground watch's exhaustive enumeration (a generated test of no hand-off) | logic `foreground.rs` 357-555 | 0 | 199 / 0 | no |
| listener L10 | six copies of the window-handle conversion | logic `check.rs` 418 and five more | 20 / 0 | 0 | no |
| listener L11 | two executable-name records holding process handles | `listener.rs` 761; `checking.rs` 347 | 10 / 0 | 0 | no |
| listener L12 | the console's process routed by two copies that differ | `listener.rs` 1973-1979; `tells.rs` 438-443 | 8 / 0 | 0 | no |
| listener L13 | fallbacks only a logic bug reaches; the nudge's hand-made union (its show part goes in build 2) | logic `kept.rs` 328-331; `asked.rs` 177-180, 307-309; `listener.rs` 788-799 | 25 / 0 | 0 | no |

The outpost:

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| outpost 1 | "probe first" answered with a probe the outpost did not make (defect); the verdict's two keepers | outpost `asked.rs` 295-309; `read.rs` 159-168; `arbitration.rs` 342-360 | 210 / small | 285 / 20 | after its capture |
| outpost 5 | verdicts judged at an entry's observation that no handler judges | `arbitration.rs` 44-62, 114-120, 352; `worker.rs` 250-255, 336-341 | 40 / 0 | 55 / 0 | no |
| outpost 4 | the caret-tell elements swept at 64 entries or 60 s (defect) | `store/mod.rs` 203-218, 359-372; `worker.rs` 598 | 15 net | 0 / one test | no in recorded runs |
| outpost 2 | the release's "live" list is always empty, so the tags decide nothing | logic `lib.rs` 659-700; `store/mod.rs` 487-502 | 70 / 0 | 35 / 0, one test rewritten | no |
| outpost 3 | walks given placeholder snapshots of known ancestors | `read.rs` 69-94, 445; `asked.rs` 1007-1023 | 65 / 0 | 25 / 10 | no; fewer calls |
| outpost 6 | dead test seams and fields (mockapp's `WorkerGate` and clock go with them) | outpost `mod.rs` 234-236, 278-279, 860, 1084-1088, 1161-1163; `arbitration.rs` 136-205, 296-307 | 90 / 0 | 15 + 110 mockapp / 0 | no |
| outpost 7 | the program in flight recorded three times | logic `lib.rs` 275-278, 576-597; `waiting.rs` 76; `watch.rs` 102, 120 | 65 / 0 | 20 / 0 | no |
| outpost 8 | `is_state_property` in the logic crate with an unreachable arm | logic `entry.rs` 26-53; `worker.rs` 392-421 | 30 / 0 | 0 | no |
| outpost 9 | the active text position registered, heard and dropped | outpost `mod.rs` 1485-1501; `docs/parity/backends.md` 336-343 | 20 / 0 | 0 | no |
| outpost 10 | a hook-stamp helper with one caller | outpost `mod.rs` 882-888, 1222-1285 | 20 / 0 | 42 / 0 | no |
| outpost 11 | copies of the nearest window, the hidden-root dispatch, the completion's parts and the depth-first walk | `asked.rs`, `read.rs`, `completion.rs` as listed | 80 / 0 | 0 | no |
| outpost 12 | "which object NVDA makes of a window" in five functions | `read.rs` 530, 588, 626, 1628, 1924 | 40 / 0 | 0 | no |
| outpost 13 | the test settle's numbering and its agreement check | `shell.rs` 48-528; `intake.rs` 131-141 | 40 / 0 | 0 | no |
| outpost 14 | unread fields | logic `entry.rs` 115-126; `worker.rs` 356-366; logic `text.rs` 340-342 | 20 / 0 | 0 | no |
| outpost 15 | statements of removed mechanisms in the code | about 20 comments in both crates | 0 net | 0 | no |
| outpost 17 (Q4) | a dialog of more than 512 objects gives no text (section 9: remove the cap; capture) | `dialog_text.rs` 20-23 | 5 / 0 | 0 | yes, towards NVDA; after its capture |
| outpost 18 (Q5) | activation stops 8 parents up (section 9: remove; capture) | `read.rs` 2179-2205 | 3 / 0 | 0 | yes, towards NVDA; after its capture |
| outpost 19 (Q6) | a failed controller-for read answered "unrelated" (section 9: answer a failure; capture) | `asked.rs` 854-893 | 5 / 0 | 0 | after its capture |
| outpost 20 (Q2) | text facts the reducer holds but no rule reads (section 9: move them to the store) | logic `text.rs`; `store/mod.rs` 178-190; logic `lib.rs` 550-563 | 250-400 / 0 | 200 / 0 | no |
| mockapp M2 | dead stand-ins, commands and fixtures | `src/menu.rs`, `src/popup.rs`, `write-on-read`, `hold-acc-parent`, `freeze`; four fixtures | 580 / 0 | 6 / 0; 97 fixture lines | no |

### Tests to add

- The foreground hold released by its timer with the window in front: the report goes before the tell, and a second foreground event loses nothing (listener L2), about 40 lines.
- The delivery-time check as a straight-line function over a calls trait, tested directly over recorded call answers (listener L5), about 250 lines replacing 505; mockapp's 121 tests then run the listener's own `check`, `CheckEvent::new`, `cached_of` and the tell assembly, with the window reads and the clock as parameters (mockapp M1).
- Progress bars: Core asks for a background bar only with the setting on, and for the attention application's bar either way; no key waits behind a bar read; the two scenarios' sounds unchanged.
- A "probe first" request whose window has a kept verdict reports only a probe it made (outpost 1).
- A caret-tell token Core holds is kept past 64 tells (outpost 4).
- A replay of the recorded Alt+Tab order through `on_win_event`, replacing the three hand-written ordering tests once the recording exists.
- The removals the records list: `reading/tests.rs`'s five duplicates of the pure schedule (keep the wiring test and the two cancels of a running request), the foreground enumeration, the release test, the scan test, the numbering tests moved to reader-rules, `waiting.rs`'s five duplicates of reducer-level tests, the tag and join tests, `hook_stamp_tests`, the observation-time tests, the verdict-keep tests once Core is the only keeper, and the class-list tests of `classify_with`, which move to `verbatim-reader-rules` rather than going.

### Rulings and defaults it rests on

- Dickson's progress-bar ruling and its condition, quoted at the top. It changes these decisions of record, which the design quotes and rewrites:
  - ADR 0003, "Progress bars (Dickson, 2026-11-14)": "The approved progress-bar design stands as the one place a reader reads unasked: the listener reads a bar's role, percentage and place and reports facts, and Core judges them. Core's requests go ahead of bar reads on a reading thread."
  - ADR 0003, "Acceptance judged in Core": "The listener judges nothing with these facts, except, while background progress bars are not reported, whether to read a value change's classic bar, and a UI Automation bar change after its nearest window, with Core's own function on facts it reads for that purpose (Dickson, 2026-11-19)."
  - ADR 0003's rejected option (line 216), "Progress bars as ordinary requests, Core asking the listener to read a bar when told of its change", whose stated reason, the foreground filter, was not built.
  - ADR 0003, "What Verbatim keeps beyond NVDA": "the progress path's own backend verdict".
  - `books/dev/src/explanation/design/ownership.md` 29, on the background-bars setting: "Core sends it at launch and on every change as configuration, and the reader judges under a value at most one message old (Dickson, 2026-11-19)", with `architecture.md` 680-685 saying the same. Under Dickson's condition the listener needs no setting, so the configuration channel and the launch argument built for it (ccdb2a7a) go.
  - `docs/design/progress-bars.md`, "The listener owns every progress bar ...", which build 7 then deletes.
- The default that the later decision governs: ADR 0004, "Core's backend cache is the one owner of a window's verdict; the check thread's probe record only spares a repeated probe and never ends a check (delegated)", governs ADR 0001's "Each window has one backend verdict, its outpost's (class lists, then one raw probe, kept as NVDA keeps it)". The outpost keeps a verdict only within one program; ADR 0001's sentence is amended and `architecture.md` 902-905's lifetime keep corrected. The listener's three judges are taken into the same ruling: the two bar paths go with L1, the check thread's probe record only spares a repeated probe.
- Section 9: the foreground hold's timer moved onto the WinEvent thread (which work runs on which thread is his area, by default); the dialog-text cap and the activation cap removed and the controller-for failure answered as a failure, each after its capture; the text facts moved to the store, which changes `ownership.md` 130 ("The reducer holds every remembered fact of the application with no Windows type"; his ruling of 2026-10-30, by default).
- mockapp M2: `freeze` goes as a removal; if Dickson wants hand-capture tools kept, it stays.

### What it must not do

- Do not remove the outpost's keep outright: every window a walk crosses would then be probed on every request, where NVDA caches each for half a second. Keep a verdict within one program; windows Core is never told of cost one probe per crossed window per request.
- Do not take the review's narrower bar variant (keep the listener's ownership, drop the memo and deadline): Dickson ruled for Core asking.
- listener L2: the watch's `sent` unhooks the console events with `hook_consoles(false)`, which must run on the WinEvent thread.
- listener L3, the merge of waiting tells, stays: Dickson reaffirmed it the day it was measured, and M6.1 rewrites that queue.
- Keep `on-activate` in mockapp: a lasting document cites it (`docs/nvda/main-loop-and-watchdog.md` 231).
- Do not move `listener_replays.rs` to the listener's logic crate: it uses `verbatim_process::held_slot::HeldSlot`, a Windows-tier crate.
- Keep the seven hand-written foreground tests (logic `foreground.rs` 215-355), `child_cleanup.rs`, `slow_application.rs`'s tests and `a_help_balloon_shown_is_spoken`.
- Keep `shutdown_drops_the_queued_requests_unanswered` (different inputs from its supposed duplicate), and rewrite `a_text_write_for_a_released_node_is_not_made` rather than deleting it.
- `Arbitrator::new(extra_good)` stays for M5.
- The straight-line check's size (about 220 lines from 420) is the review's estimate of code not yet written.

### Size budget

About 2,400 product lines removed in the readers (the upper end of REPORT's 1,200 to 2,400, with progress bars) and mockapp's 580, with about 300 added; about 2,600 test lines removed and about 650 added.

### Done when

- `ForegroundWatch::pending`, `PoolTimer` and `event_windows.rs` are gone; the timer-path test passes.
- `git grep -n "uia_tell\|fn bar_change\|raise_at_once\|fn msaa_tell" crates/mockapp/tests/common` finds nothing; the two concrete mutations the mockapp record names (`with_answer` dropped, `reading_parent(false)`) now fail a test.
- `CoreToListener::Configuration`, the `--background-bars` argument and `listener_configuration.rs` are gone; the progress-bar measurements are recorded in the build's report.
- The outpost's verdict keep, `Input::Verdict`, `KeptFromAnyGeneration` and the store's tags are gone; ADR 0001, ADR 0003 and `architecture.md` say one thing.
- The captures are made and the changed speech matches them; wake-ups and processor time are measured; `background_progress_bar`, `background_progress_bar_off` and the related scenarios found from the code pass 3 runs; the GitHub run is read.

## Build 5. Remote operations and text

### Purpose

Give the remote-operation crate one dispatcher with one rule, remove its duplicate walks and the comparison that never matches, keep the builder methods no program emits yet (Dickson, 2026-11-20), and decide the call-by-call twins from a counted fallback rate. Fix flat review and Backspace at a line's start towards NVDA after their captures, and remove the dead text protocol, reply variants, per-method provider counts and the dead focus query path.

### Before the design: measure, capture or record

- Count, over the GitHub runs' logs of the last month, every `Path::Fallback` warning (outpost `read.rs` 2108, `completion.rs` 449) and every import failure that marks a window for classic reading (`read_classically`), split by whether `uia.remote_operations` was off. This one count decides the twins (apis Q2, text B1).
- Capture: navigator on a list item with no text pattern ("Inbox"), review start of line, end of line and current line (text B9).
- Capture: Windows 11 Notepad with "alpha", carriage return, "beta", focus landing on "beta" before any line break is seen, Home, Backspace (text B10, the unseen-break case; the seen-break cases are captured already in `editing.rs` 121 and 208).
- Capture: say-all by line over "你好。再见" then "谢谢" (text B12).
- Capture: a Windows Forms password box of stars, Control+Right Arrow, then review current word (text B13).
- Capture: in File Explorer, Tab from the address bar into the items list with an unnamed UI Automation pane between (apis F5).
- Capture: an MSAA dialog whose static text holds U+200E between digits (apis F8).
- Capture: a console window whose host cannot be read (apis F18); an element whose runtime id read fails (apis F20); an object of role caret or cursor (apis Q4).
- Measure after the change: the focus-ancestry worst case in `instruction_limit.rs` (3,078 today includes the dead `previous` read, calls F3).

### Inventory

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| apis F4 and calls F4 | thirteen dispatchers with five rules; ten hand copies of `choose`, three without the timeout guard (one item; the terminal three wait for build 9) | rops `text.rs` 299-313; `caret.rs` 583, 596; `controlled.rs` 74; `navigate.rs` 72; `path.rs` 59; `watch.rs` 124; `focus.rs` 285, 325; `focus_states.rs` 68, 86; `tree.rs` 73, 129, 137 | 90 / 0 | 0 / 40 | no |
| apis F1 | 31 builder methods no program emits yet, with their instructions: kept, not removed (Dickson, 2026-11-20: "future work like browser UIA support or Word support would need them") | rops `builder.rs` 411-1299; `instruction.rs` 88-1016 | 0 | 0 | no |
| apis F2 | two classic UIA ancestor walks and two classic navigation steps | uia `client.rs` 676, 790, 822, 945; rops `focus.rs` 996; `navigate.rs` 152 | 190 / 0 | mockapp tests of the deleted walk | no |
| apis F3 and calls F3 | the previous focus's keyboard-focus read and the program's `refresh` path, reached only from tests (one item) | rops `focus.rs` 48, 296-317, 504-536, 560, 766-778, 1006-1033 | 100 / 0 | 60 / 0 | no |
| apis F6 | agile-reference equality is reference identity: the comparison never matches (defect) | uia `shape/mod.rs` 521; outpost `completion.rs` 358-363 | 8 / 0 | 0 | no |
| apis F7 | the per-thread call counter written twice | ia2 `calls.rs` 35-66; uia `calls.rs` 95 | 50 / 0 | 20 / 0 | no |
| apis F8 | MSAA name, value, description and state rules in four copies; the dialog copy keeps bidirectional marks | ia2 `acquire.rs` 313, 342, 366, 2146, 2614; `dialog.rs` 168-220 | 80 / 0 | 0 | the dialog's bidi marks only; after its capture |
| apis F9 | dead WinEvent hook code and stale per-application-outpost prose | ia2 `hook.rs` 223-239, 526-572 | 65 / 0 | about 40 repointed | no |
| apis F10 | a global map, lock and sweep kept to choose a log level | uia `answered.rs` 31-39, 82-120 | 45 / 0 | 9 / 0 | no |
| apis F11 | six hand-written parts builders | uia `shape/shapes.rs` 347, 435, 498, 766, 900, 970 | 60 / 0 | 0 | no |
| apis F12 | `WinEventKind` folds raw ids that `tell_kind` splits again | ia2 `hook.rs` 61, 188, 193, 851-884 | 80 / 0 | 0 | no |
| apis F13 | the value-from-selection and menu-item rules twice; a stale program doc | uia `map.rs` 170; rops `focus.rs` 338, 391, 472 | 30 / 0 | 0 | no |
| apis F14 | forwarders and duplicate wrappers | win32 `window.rs` 456; uia `lib.rs` 106-109; `element.rs` 442-470; `checks.rs` 28, 58 | 100 / 0 | 0 | no |
| apis F15 | invalid states writable in the MSAA registry | ia2 `registry.rs` 54-66, 238, 297, 314; `equality.rs` 192 | 20 net | 0 | no |
| apis F16 | subscription shapes that can be written meaningless | uia `subscribe.rs` 210, 225-265, 364-369 | 100 / 0 | 0 | no |
| apis F17 | the proxy's ignore list and the subscriptions drifted (defect) | uia `proxy.rs` 27-45; listener `listener.rs` 1438 | 10 net | 0 | no |
| apis F18 | a console window's process decided twice, the second defaulting to 0 (defect) | win32 `provider.rs` 88-94; `window_facts.rs` 52, 102-106 | 5 / 0 | 0 | after its capture |
| apis F19 | a malformed ancestor dropped silently (defect, a program bug only) | rops `focus.rs` 722-731 | 0 | 0 | no |
| apis F20 | a failed runtime id read as an empty id (defect) | uia `com.rs` 292-300; `registry.rs` 121-176 | 0 | 0 | after its capture |
| apis F21 | `role_from_msaa`'s callers each convert an option by hand | ia2 `acquire.rs` 303 and seven more | 10 / 0 | 0 | no |
| apis F5 (Q1) | NVDA's presentable-ancestor rule in the UIA reader and again in Core (section 9: Core alone; capture in File Explorer first) | uia `client.rs` 1111, 1329, 1356, 1369; Core `reduce.rs` 1973 | 100 / 0 | 178 / 120 | after its capture |
| apis Q3 | sync markers installed in every product process (section 9: behind the faults feature) | ia2 `event_thread.rs` 255, 359 | about 150 moved | 0 | no |
| apis Q4 | NVDA's caret and cursor roles read as unknown (default: match NVDA after a capture) | model `Role`; ia2 role map | small / small | small | after its capture |
| apis F23 and terminals R1 | the `terminal_rows` remote operation has no product caller (one item) | rops `terminal_rows.rs` (198); `lib.rs` 62, 106-108; the guide's "Rows by their place" | 202 / 0 | 95 / 0 | no |
| apis Q2 and text B1 | the call-by-call twins (default: they go only if the count shows no fallback outside the developer setting) | rops `caret.rs` 531-572, 1658-2115; `text.rs` 1102-1509; `watch.rs` 457-575; and the non-text twins in the apis record's table | about 1,030 text and caret, about 1,100 in all / small | about 700 / 0 | a read that falls back today would fail instead; formatting and the first-line edge on the generic path need two added reads |
| text B2 | dead text protocol: `ReadFacts`, `Change`, `CaretChanges` and five fields | model `text.rs` 388-403, 424-434, 480-516; `event.rs` 419-436 | 115 / 0 | 30 / 0 | no |
| text B3 | `TextReply::Moved` and `Selected` never reach Core; Core converts back | outpost `text_reads.rs` 619-624; `held.rs` 446-475; Core `review_text.rs` 217-220, 956 | 60 / 0 | 20 moved | no |
| text B4 and model 7 | "gone" has two encodings; text failures restated as `TextFailure`; `OutsideOutcome` (one item; the terminal half waits for build 9) | outpost `asked.rs` 121, 227, 243-248; Core `review_text.rs` 222; `say_all.rs` 278; model `text.rs` 1229-1297; `request.rs` 894-905, 1562-1578 | 50 + 90 / 0 | 0 | no, checked caller by caller |
| text B5 | two copies of "which anchors a request names", already different | Core `text_facts.rs` 142-193; outpost `text/mod.rs` 752-777 | 35 / 0 | 2 tests replaced by 1 | no |
| text B6 | stamps and a generation the event queue made unnecessary (the terminal caller waits for build 9) | Core `editing.rs` 548-566; `state.rs` 524-529, 232-234; outpost `held.rs` 451, 481-493 | 60 / 0 | 35 / 0 | no |
| text B7 | say-all's copied trace, a read that returns nothing, a branch with equal arms | Core `say_all.rs` 316-325, 342, 449, 566-575 | 25 / 0 | 0 | no |
| text B8 | the formatting memory written by two pairs of functions | Core `editing.rs` 262-291, 350-373 | 22 / 0 | 0 | no |
| text B9 | flat review is a second review implementation (defect) | Core `review.rs`; `reduce.rs` 1250-1393; `review_text.rs` 1119-1206; `state.rs` 149, 326-346 | 420 / 40 | 150 rewritten | yes, towards NVDA; after its capture |
| text B10 | Backspace at a line's start names a remembered break (defect) | Core `editing.rs` 122-153, 177-180, 559-562, 584-586; `state.rs` 226-231 | 50 / 0 | 0 / one test | yes, towards NVDA; after its capture |
| text B11 | twelve smaller duplicates and dead items | as listed in the text record | 145 / 0 | 20 / 0 | no |
| text B12 | say-all's sentence ends in other scripts (section 9: match NVDA; capture) | `verbatim-text/src/lib.rs` 401-450 | 60 / 0 | 45 / 0 | yes, towards NVDA; after its capture |
| text B13 | review word commands by Core's ICU words (section 9: match NVDA; one round trip per press) | Core `text.rs` 185-208; `review_text.rs` 424, 507, 1064, 1075 | 250 / small | not estimated | yes, towards NVDA; after its capture |
| calls F1 and calls Q2 | provider hits pinned exactly; 33 figure moves and no catch in 92 commits (the session's decision, REPORT section 10) | `crates/mockapp/tests/call_counts.rs`, 624 hit tuples | 0 | 700-760 / about 10 targeted checks | no |
| calls F6 | `uia_event_caches_cost_exactly` adds only UI Automation's fixed overhead | `call_counts.rs` 3312-3420 | 0 | 110 / 0 | no |

### Tests to add

- One unit test of the shared dispatch rule over fake remote and classic functions (apis F4), about 40 lines.
- If the twins stay where a design requires them: one hand-written test comparing `element_by_path` and `controlled_selection` remote against classic (calls), about 40 lines.
- Backspace at a line's start with no break seen, after its capture (text B10).
- One exhaustive test of `TextOp::anchors()` replacing `text_facts_tests.rs`'s two (text B5).
- Flat review's tests rewritten as tests of the one review path (text B9); the 16 flat-review tests in `focus_speech_tests.rs` 1827-2150 move here from the focus file.
- About ten targeted provider-side checks kept where the hit tuples go (the boundary's `WM_GETOBJECT`s, the title bar, the probe).
- The removals tied to findings: the five and six table-restating tests in `verbatim-uia` and `verbatim-ia2`; `answered.rs`'s refusal test; the `previous` and `refresh` parts of four tests; the fixtures spelling out B2's dead fields and B3's `Selected` (`tests/text.rs` 1737-1744 feeds a reply the outpost never sends); `a_copy_keeps_a_start_marker_set_while_its_read_was_in_flight` (text T5); the agreement halves of the twins' tests if the twins go.

### Rulings and defaults it rests on

- The twins default: they go only if the count shows no fallback outside the developer setting; the listener's focus walk is call-by-call by decision and stays, and the size-limit paths keep their classic reads.
- Section 9: the presentable-ancestor rule in Core alone after the File Explorer capture; say-all's sentence ends and review word moves matched to NVDA after captures (the second costs one round trip per press, a performance budget; his area, by default); sync markers behind the faults feature.
- Matching NVDA's captured speech needs no decision, so text B9 and B10 need only their captures; B9's current-line part is already ordered by the M4.2 item of 2026-10-08, whose status line marks M4.2 built though the difference remains.
- REPORT section 10: dropping the per-method provider counts is the session's; client calls and failed reads stay exact.

### What it must not do

- apis F6: do not compare by runtime id: the kept element is found by that same runtime id, so the comparison would always match and disable the stale check. Delete the comparison, or compare with `CompareElements` if a measurement shows the call matters.
- apis F4: keep `focus_states`' fallback on a gone element (one gone dialog must not lose the focus's states), `tree_dump`'s per-run loop and `text_range`'s completion; correct the guide's "every entry point" sentence instead.
- The three terminal dispatchers and their missing timeout guard are build 9's; only `terminal_rows`, which has no caller, goes here. The records disagree on it: the terminals record says remove now; the apis and calls records leave it to M6.4. Removing a file nothing calls changes no speech, so the default applies.
- The twins: if any go, the generic path on windows read classically reads no formatting and answers `(false, false)` for the edges; add the two reads or record the loss. The agreement tests stay while their twins stay. Calls Q3 records that "either way, the classic walks stay" for the listener, the 2026-10-07 fallback ruling and the diagnostic setting; the default removes only twins the count shows unused.
- calls F1 and calls Q2 exclude each other: the review's table shape (every figure kept in a regenerated table) is not taken, since REPORT section 10 chose dropping the hits.
- text B10: the A reviewer's sequence (a failed read silences the key) was wrong and is not the defect.
- Keep the `msaa_tree` walk's raw `IAccessible` reads (an independent check of the stand-in) and `msaa_slow_acc_parent_answers_the_parent_late` (a cited capture uses it).
- The notification-processing default and the `unwrap_or(i32::MAX)` conversions are not defects.

### Size budget

About 2,500 to 3,700 product lines removed (REPORT's estimate, the upper end with the twins) and about 100 added; about 1,300 to 2,000 test lines removed and about 200 added.

### Done when

- One dispatcher: `grep -n "if !remote {"` in `crates/verbatim-uia-rops/src` shows `choose` and the named exceptions only.
- `Uia::neighbor` is gone and one chain loop remains (`Uia::ancestors` is kept: the twin lacks four of its outputs and would add calls to two callers that are not focuses; Claude, 2026-11-21); `FocusQuery` has no `previous`.
- The fallback count is written into the build's report, with the twins' outcome that follows from it.
- `review_offset` and `StartMarker::Flat` are gone and flat review goes through the text path; Backspace says the provider's unit at every offset; both match their captures.
- `call_counts.rs` holds no hit tuple and `performance.md` names no hit figure (the ledger wording itself waits for the process revision).
- The related scenarios found from the code (the text-box editing, caret, selection and review families on GitHub) pass; the GitHub run is read.

## Build 6. The tests and the harness

Added by Dickson's ruling of 2026-11-20, and done first, beside build 1: every entry of `docs/flaky-tests.md` is triaged once to a named cause and then either fixed (here, or by the build of this plan that reaches its cause, which the entry then names) or taken out of GitHub's runs with its reason recorded. The inventory below removes the registry's empty list of disabled scenarios (scenarios, the `DISABLED` finding); under this ruling that list, or the reason a local-only scenario must give, is what records a scenario taken out of GitHub's runs, so the design decides which of the two stays and removes the other.

Also added on 2026-11-20, after Dickson asked whether every end-to-end scenario is needed: the records answer that only in part. The scenarios record names the duplicated Theme page walk, three demonstration aliases, the fourteen Notepad copies and the echo scenarios for each terminal, and it dropped two proposals to merge the quit scenarios because they would undo scenarios added on 2026-11-17 and 2026-11-19; it did not judge each of the 105 scenarios. So this build judges each one once by the rule in [plan.md](plan.md), build 6, and reports the table. One caution from the mockapp record: `controlled_list_selections` looked like a duplicate of a mockapp test and is in fact the only test that runs the real listener for its path, because the mockapp test runs a copy; until build 4 replaces that copy, a scenario is not removed on the strength of a mockapp test.

### Purpose

Make the end-to-end suite and the test support keep each list once and run the real code, and give every scenario expectation a capture. It changes no product state and may run beside another build, in its own worktree and target directory.

### Before the design: measure, capture or record

- Captures for the scenarios whose expectations have no capture: `toast` (speech declared the design's), `twin_item_at_a_deleted_items_ids` (speech "to be captured from NVDA on this stand-in"), and the end-of-row steps of the two `review_grid` scenarios (the design's rule).
- Re-captures for the 18 scenarios whose only cited capture files are gone: `settings_combo_box_expanded`, `settings_search_suggestions_reentered`, `text_box_editing`, `notepad_editing`, the eight `text_box_` and `notepad_` `document_keys`, `page_keys`, `paragraph_keys` and `select_all_count`, `text_box_password`, `wpf_password`, `text_box_typed_words`, `notepad_typed_words`, `windows_terminal_two_windows` and `windows_terminal_leave_flood`.
- Re-captures for the three commits that changed an expectation to Verbatim's own output with no capture named: e48e06ab (the hung mockapp's opening: window only), 50b1d527 (location as a percentage; the follow-caret toggle's wording) and 472833ff (terminal progress: the prompt then "100%").
- Captures for the six Core tests that say "Needs capture: written to the design's derived speech" (keys test findings): four in `caret_key_tests.rs` and two in `caret_tell_tests.rs`.
- Recordings with `verbatim-event-record` for the stand-ins scenarios depend on: the owned dialog across processes and Settings' search box with suggestions (mockapp M12); the three existing comparisons then read their recording files.
- Read the artifacts of `settings_system_page`'s eight GitHub failures and name the process or code responsible (scenarios 9).
- One load run of `call_counts` (the flaky-test entry waiting since b94fa1d8), after which the entry and the serial filter go or the failure is recorded (calls F12).

### Inventory

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| scenarios 1 and harness F2 | the scenario list kept three times, held in step by text-parsing checks (one item) | `tests/e2e.rs` 38-191; `registry.rs` 2845-2884; `xtask/src/plan.rs` 236 (`test_names`) | 0 | 225 / 0 | no |
| tools T11 | the planner parses Rust source as text | `xtask/src/plan.rs` 127-288; `group.rs` 88-110 | 35 (`test_names`) / 0; the registry read stays | 45 / 0 | no |
| scenarios 2 | a registry entry is eleven lines of mostly defaults; local-only has no reason field | `registry.rs` 397-1834 | 0 | 900 / a constructor with `local_only("reason")` | no |
| scenarios 4 | expectations with no capture; cited captures gone | as listed above | 0 | 0; expectations re-set from captures | only where a capture differs |
| scenarios 6 | `quit_inside_the_remove_theme_confirmation` replays the whole Theme page walk | `quit_inside_the_remove_theme_confirmation.rs` 20; `theme_panel.rs` 113-271 | 0 | about 0; about 60 s of every GitHub run | no |
| scenarios 7 | mockapp launched by eight hand copies; nine live scenarios import the measure set's launcher | `measure.rs` 393-430 and seven more | 0 | 150 / 0 | no |
| scenarios 9 | `settings_system_page` failed eight times on GitHub with no fix | `docs/flaky-tests.md` 44-56 | 0 | triage only | no |
| scenarios 11 | an empty disabled-scenario mechanism; a constructor nothing calls | `registry.rs` 234-248, 1866-1929, 2886-2899; `scenario.rs` 370-376 | 0 | 50 / 0 | no |
| scenarios 12 | `run` and `run_compare` are near copies | `registry.rs` 2013, 2151 | 0 | 80 / 0 | no |
| scenarios 13 | `press` written 14 times; six launch variants over one function | 14 files; `scenario.rs` 1299-1640 | 0 | 175 / 0 | no |
| scenarios 14 | `Scenario` holds "Verbatim or NVDA" and its lifecycle as options and flags | `scenario.rs` 212-215, 291-357 | 0 | 50 / 0 | no |
| scenarios 15 | scenarios that repeat another's inputs (not the quit scenarios) | `second_application_and_verbatim_menu`; `settings_search_suggestions_reentered`; three demonstration aliases | 0 | 300 / 0 | no |
| scenarios 19 | the measure set, 1,201 lines and 16 registry entries | `measure.rs`; `registry.rs` | kept; its launcher moves to the shared mockapp module (scenarios 7) | 0 | no |
| harness F3 | one hand-written client method per agent request, matched by name at run time | `crates/verbatim-e2e/src/agent_client.rs` (55 `unexpected` arms; 7 launch methods) | 900 / a typed request with its reply type and one generic call | 60 / 0 | no |
| harness F8 | the run summary as a hand-made text format and parser | `artifacts.rs` 184-230, 584-690 | 120 / 0 | 80 / about 20 | no |
| harness F9 | protocol compatibility defaults the handshake makes dead; a changelog in a doc comment | agent `protocol.rs` 20-95, 177-182; `server.rs` 149-152 | 80 / 0 | 20 / 0 | no |
| harness F10 | one utterance's facts in seven collections; collector modes as two fields | `speech.rs` 457-487, 632-642 | 60 / 0 | 0 | no |
| harness F11 | speech frames turned into text three times | `speech.rs` 590-657, 1934; `xtask/src/nvda.rs` 503 | 60 / 0 | 0 | no |
| harness F12 | the overlapped pipe written twice | agent `tunnel.rs` 160-357; control `server.rs` 394-619 | under 150 (not recounted) / 0 | 0 | no |
| harness F13 | dead code | `speech.rs` 1474; `timeline.rs` 304 | 25 / 0 | 40 / 0 | no |
| harness F15 | agent dispatch matches each group twice, with fallbacks only a logic bug reaches | agent `server.rs` 415, 437, 593, 685, 913, 951 | 40 / 0 | 0 | no |
| harness F16 | a launch recorded in two pid-keyed maps (the merge only) | agent `process.rs` 592-595; `jobs.rs` 54, 299 | 30 / 0 | 0 | no |
| harness F17 | per-connection held keys in a global | agent `server.rs` 55-70 | 20 / 0 | 0 | no |
| harness F19 | trivial path tests | `artifacts.rs` 864-888 | 0 | 25 / 0 | no |
| mockapp M3 | each test named twice in a hand-written registration list | every `fn main()` in mockapp's test binaries | 0 | 650 in the area, about 1,400 in the crate / a `tests!` macro | no |
| mockapp M5 | five scenarios carry inline copies of fixtures; the twin-items copy differs from its file (defect) | `twin_item_at_a_deleted_items_ids.rs` 28-45 against `list_view_twins.json`; four more | 0 | 73 / 0 | the twin scenario, after its capture |
| mockapp M6 | one command enum for every backend; catch-alls a logic bug reaches | mockapp `stdin.rs` 92-503; `uia.rs` 165-440; `msaa.rs` 208-463 | 150 / 0 | 0 | no |
| mockapp M7 | the hit counters' method list written three times | mockapp `hits.rs` 199, 366, 451, 540 | 150 / 0 | 0 | no |
| mockapp M8 | `events.rs` delivery tests subsumed by later tests | `tests/events.rs` 121-838 (7 tests) | 0 | 230 / 0 | no |
| mockapp M9 | dead helpers hidden by `#![allow(dead_code)]` | `tests/common/mod.rs` 11; `outpost.rs` (9 helpers) | 0 | 100 / 0 | no |
| mockapp M10 | UIA and MSAA each have their own node mutators; `select_node` differs (stand-in defect) | mockapp `msaa.rs` 468-526; `uia.rs` 519-590 | 40 / 0 | 0 | no |
| mockapp M12 | most stand-ins never checked against a recording; three checks copy the recording by hand | `focus_reports.rs` 540, 722, 845 | 0 | 0 / 60-90 per stand-in, two first | no |
| mockapp M14 | the test support keeps its own copy of the shell's effect dispatch | `core_driver.rs` 965-1083; `verbatim-app` | net near 0 (a library function) | 120 / 0 | no |
| mockapp M15 | `arbitration.rs` repeats an outpost unit test's arbitrator check | `tests/arbitration.rs` 25-51, 201-231 | 0 | 20 / 0 | no |
| mockapp M16 | a menu-opening test repeats the first half of another | `focus_reports.rs` 2211, 2241 | 0 | 25 / 0 | no |
| mockapp M17 | weak assertions | `focus_reports.rs` 682, 693, 695, 698, 821, 3261 | 0 | replaced | no |
| calls F5 | exact instruction counts at typical sizes (the session's half: keep exact worst cases, drop the typical runs) | `instruction_limit.rs` 330-334 and others | 0 | part of about 400 / 0 | no |
| calls F7 | `the_listener_makes_no_connection` duplicates the focus ratchet | `call_counts.rs` 1848-1890 | 0 | 43 / 0 | no |
| calls F8 | figures and history in test comments | `call_counts.rs` 1640-1657, 1806-1816; `instruction_limit.rs` 514, 553-556 | 0 | 100 / 0 | no |
| calls F9 | `identity_of` rebuilds Core's identity with silent defaults | `call_counts.rs` 887-915 | 0 | 40 / 0 | no |
| calls F10 | two tree-dump tests are one test written twice | `uia_tree.rs` 565-640 | 0 | 30 / 0 | no |
| calls F11 | the description change's speech pinned twice | `call_counts.rs` 581-590; `msaa_events.rs` 121-133 | 0 | 15 / 0 | no |
| calls F12 | smaller items: the serial run owed a recheck, a uia-facts claim checked twice, registration pins, a misplaced comment | `docs/flaky-tests.md` 182-185; `xtask/src/main.rs` 551; uia-facts `a-registration-that-times-out-reports-success/src/main.rs` 320-324; `remote_ops.rs` 1469-1470 | 0 | about 10 | no |
| terminals R2 | `verbatim-stream-test`'s `--pipe` mode has no caller | `crates/verbatim-test-console/src/bin/verbatim-stream-test/main.rs` | 0 | 100-130 tool lines | no |

Test findings without an id, placed here because they change no product code:

- One Core test harness: `src/desk_tests.rs` (681) shares 343 of 482 distinct lines with `tests/desk/mod.rs` (748). The queue record counts a saving of about 600; the focus record says it is not verified until the internal tests are shown to drive the public reducer. Show that first.
- The queue record's duplicates: four `entries_tests.rs` tests (about 90 lines), seven of the eight outside-the-queue tests in `event_queue/tests.rs` (122), four of the five property tests in `tests/merge.rs` (45), `replay_is_deterministic` (10), the eight one-output `tests/shell.rs` tests as unit tests (140, low value), about ten `contains` and `any` assertions; and `flight_recorder_dump_replays_to_the_same_effects_as_live_reduction` fixed to keep one runtime across the loop (it cannot fail on a rebuild defect today).
- The focus record's: trim the two duplicate row tests in `validity_tests.rs` (about 30); run each replay session in its test and compare its dump with the committed fixture (`replay_validity.rs` 337-758); assert the destroy's effects in place of JSON paths (887-949).
- The keys record's: `navigator_tests.rs`' two subset tests (about 25); `scripts.rs`' six table-restating tests (about 220); `hook.rs`' unbounded-burst test (about 50).
- The model record's: five serde round-trip tests replaced by one round trip per message direction (about 155 out, 20 in); control's eleven send-keys parse tests by two table tests (90 fewer); one each of the round-trip triples in control and process (110 to 150).
- The supervisor record's: one stand-in in `tests/common` and one binary of trials (about 600), with `ignored_process.rs` and `listener_destroyed.rs` folded into it as trials (about 350).
- The text record's: T1, eight say-all tests in `tests/text.rs` that repeat `say_all_tests.rs` (258 lines); T2, about five `review_text_tests.rs` tests covered at the same level (180).

### Tests to add

- A recording comparison for the owned dialog and for Settings' search box, about 60 to 90 lines each; the three existing comparisons read their recording files.
- The outpost-kept assertion and the menu's Down Arrow moved into `text_box_editing` when `second_application_and_verbatim_menu` merges into it.
- A short walk of its own for the quit inside the remove-theme confirmation.

### Rulings and defaults it rests on

- REPORT section 10: dropping per-method provider counts is the session's (done in build 5); keeping exact worst cases and dropping typical-size runs needs no change to "Counts are exact numbers, not bounds".
- The standing rule that spoken expectations come only from captures, which the re-captures carry out.
- Where the captures are kept, and whether compare mode stays, are with the process revision; this build makes the captures and cites them as that revision decides.

### What it must not do

- The planner cannot read the test binary's `--list`: the `plan` job runs on Linux and builds neither the registry crate nor the test binary. Keep the planner's reading of the registry and make it follow the constructor's new shape.
- Do not merge `quit_inside_a_modal_dialog` or `quit_inside_the_verbatim_menu` into other scenarios: they test separate shutdown paths added on 2026-11-17 and 2026-11-19.
- `controlled_list_selections` is the only test of the real listener's controlled-selection tell across the process boundary; it is not a duplicate.
- `settings_system_page`: read its artifacts and name the cause before anything else; if no Verbatim cause is found, its handling (keep, local-only, retire) goes to the process revision.
- The desk harness merge is not done until its feasibility is shown.
- `on-activate`, `fixture_path` and `child_cleanup.rs` stay.

### Size budget

About 1,900 scenario and registry lines, about 1,700 harness product lines and 560 harness test lines, about 1,450 mockapp test-support lines and 300 of mockapp product, about 400 call-count and instruction lines, and about 1,500 Core and crate test lines removed; about 300 added beside the captures.

### Done when

- `tests/e2e.rs` holds no scenario name and iterates `registry::SCENARIOS`; `the_test_binary_has_a_test_for_every_scenario_and_no_other` and `test_names` are gone.
- Every live scenario names a capture at the pin, or is one of Verbatim's own pages, or carries a declared deviation; the three named commits' expectations match their re-captures.
- mockapp's binaries register tests through the macro; `agent_client.rs` has no `unexpected` arm.
- `cargo xtask ci` passes; GitHub's suite passes and its run is read; `cargo xtask e2e-durations` is rerun after the Theme walk's change.

## Build 7. The documents

### Purpose

Turn the four built "Readers tell, Core asks" design files into explanation pages and decision records, as the writing convention requires, and delete them once the 183 code comments that cite them point at the new homes. Clear the crate guides of dead sections, delete the uncited experiments files and the fact programs nothing rests on, and finish what today's documentation correction leaves. It changes no product state and may run beside another build; its parts that describe a mechanism wait for that mechanism's build.

### Before the design: measure, capture or record

- Diff today's documentation correction against this inventory, so that build 7 does only what it left.
- Nothing to capture.

### Inventory

| Record and id | Title | Files and lines | Document lines | Code touched | Speech changes |
| --- | --- | --- | --- | --- | --- |
| documents D1 | four built design files still the home of rules | `docs/design/readers-tell-core-asks.md` (600), `-build1.md` (1,334), `-build2.md` (85), `-build3.md` (352) | about 2,100 net; build 1's 18 state tables (lines 419 to about 670) and build 3's tables move to explanation pages or module docs | comments repointed: build1 140 code lines in 98 files (48 naming a numbered table, 88 a named section, 33 in test files); build3 37 in 26 files (9 naming a table); build2 6 in 5; the build notes 0 code lines and 11 lines in other documents (ADR 0003 six) | no |
| documents D2 and focus F13 | `verbatim-core.md`: three dead sections, 66 dead names and 5 dead files | `books/dev/src/reference/crates/verbatim-core.md` 581-602, 681-746, 998-1077 and 30 other lines; four code comments citing `Query::ReadNode`, one citing `key_line::Press`, three citing `focus_queue` | 168 + about 30 | comments | no |
| documents D6 | other crate guides name removed code and contradict themselves | `verbatim-app.md` (19 names); `verbatim-outpost.md` 157-158, 215-216, 275, 752, 774; `mockapp.md` 493-494; `verbatim-uia-rops.md` 500; `verbatim-e2e.md` 332 | about 30 | `arbitration.rs` 55 comment | no |
| documents D7 | ADR 0001 states the phase unbuilt and keeps limits of removed code | `adr/0001-focus.md` 5, 37, 157, 162-168 | about 8 | 0 | no |
| documents D8 | the glossary defines four terms by removed mechanisms | `glossary.md` 16-20, 90-93, 159-163, 233-236 | about 8 rewritten | 0 | no |
| documents D9 | `ownership.md` describes removed mechanisms in the present tense | `books/dev/src/explanation/design/ownership.md` 46, 57, 82, 92, 97, 133, 138 | about 10 | 0 | no |
| documents D10 | `progress-bars.md` and `m42-parity.md` are built designs describing the removed bar watch (after build 4) | `docs/design/progress-bars.md` (138; cited by 13 code lines in 10 files); `docs/design/m42-parity.md` (279; 3 code lines) | 417 | comments repointed | no |
| documents D12 | documents cite tests that do not exist | `test-coverage.md` 88; `performance.md` 977; `docs/parity/focus-events.md` 552-553, 1052; `docs/parity/input.md` 39 | 1 removed, 5 corrected | 0 | no |
| documents D16 | "Amended" lines the convention forbids | `architecture.md` 29, 38, 43, 47, 60, 70, 83, 99, 128, 144, 156, 161, 179, 270, 610, 1038; ADR 0001 3, 21, 33, 37 | edits | 0 | no |
| documents D17 | ADR 0003 keeps message-kind counts each behaviour bumps | ADR 0003 144, 148 | 2 | 0 | no |
| documents D18 | the cross-process study kept in `docs/design` | `docs/design/cross-process-calls-study.md` (233; cited by 8 code lines and one uia-facts readme) | 233, once decisions 2 to 5 have an ADR home | comments repointed | no |
| keys M15 | a parity entry contradicts the code and cites a missing test; stale crate guide and comments | `docs/parity/input.md` 30-50; `verbatim-input.md` 84-96, 142-144; input-windows `lib.rs` 235; `typed.rs` 1 | edits | comments | no |
| model 20 | stale figures and headings in the model's guide | `verbatim-model.md` 250-252, 474-475, 733-734 | edits | 0 | no |
| listener L16 | stale and contradictory documents | `newest_per_thread.rs` 1-4; two `Cargo.toml` descriptions; `check_threads.rs` 3-4; `listener-without-connections.md` 132, 170, 181, 184, 199 | edits | comments | no |
| mockapp M13 | the mockapp documents restate the commands and tests, and have drifted | `books/dev/src/how-to/mockapp.md` 60-345; `books/dev/src/reference/crates/mockapp.md` 548-789 | about 500 | 0 | no |
| apis F22 | stale documents and figures written twice | `verbatim-uia.md` 135; uia `registry.rs` 249-265; uia `lib.rs` 15-16; `uia-bugs.md` 387, 573; `focus-context.md` 110-113; `performance.md` 2177 | edits | comments | no |
| calls Q3 | R3, remote operations on ARM64: the `ci-arm64` job runs every mockapp remote-operation test on every push | `architecture.md` 751, 1201-1205 | the risk entry updated with that fact; real applications on ARM64 remain unverified | 0 | no |
| scenarios 8 | documents restating the scenario list and allowances have drifted | `e2e.md` 370-374; `flaky-tests.md` 7-14, 23-28, 44-69, 176-189, 206-213; `registry.rs` 41-150; `verbatim-e2e.md`; `test-coverage.md` 181 | about 240 | registry module doc | no |
| harness F14 | the add-on's docstring cites a document that does not exist (the testing.md half is the process revision's) | `nvda-addon/src/globalPlugins/verbatimTranscript.py` 7 | 1 | 0 | no |
| facts M1 | experiments files nothing cites | listed below | 1,140,377 bytes; 4,144 lines of code, 926 of them Rust | 0 | no |
| facts Q1 | fact programs whose own READMEs say nothing rests on them (section 9: delete, by the folder's own rule) | the five programs, three guides and, after facts M3, the program below | 1,647 Rust lines and 864,773 recorded bytes | the uia-facts workspace | no |
| facts M2 | dead support and provider code | `uia-facts/support/src/apps.rs` 43-149, 219-243; `uia-facts/provider/src/main.rs` 68, 77, 444-459 | about 135 Rust | uia-facts | no |
| facts M3 | a README describes a recovery decision 11 removed | `uia-facts/a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all/readme.md` 9 | 1 sentence; the program then joins facts Q1 | 0 | no |
| facts M4 | `uia-bugs.md` cites folders and sections that do not hold what it says | `uia-bugs.md` 5, 227 and twelve section numbers | edits | 0 | no |
| facts M5 | other stale statements in uia-facts and experiments | `uia-facts.yml` 2; `uia-facts/readme.md` 11, 67; `check-copies/src/main.rs` 2; `support/src/lib.rs` 1; two experiments readmes; the provider's module doc | edits | 0 | no |
| facts M7 | a folder that holds only a pointer | `uia-facts/a-remote-operation-stops-after-exactly-10000-instructions/` and its index row | 28 | 0 | no |
| facts M9 | READMEs name client settings where the setting changed no outcome | `a-focus-change-.../readme.md` 24; `a-hung-window-.../readme.md` 62 | a few clauses | 0 | no |
| facts Q2 | make the facts members of the main workspace | `uia-facts/Cargo.toml`, `check-copies`, `build.ps1` | not proposed: neither reviewer names a changed fact against the recorded decision (`uia-facts/readme.md` 65) | 0 | no |

The uncited experiments files (facts M1), each folder's citing files kept:

- `experiments/focus-pipeline-rework-measurements/`: 532,755 bytes go, namely `readme.md` (29,200), `debug.patch` (38,378), `tally.py`, `batch.ps1`, `programs.rs`, `probes-per-new-window.txt`, `captures/run/**` (about 307 KB, including `listener.log` 108,321 and `outpost-explorer-3332.log` 135,453), the 12 other captures, `driver.py`, `lib.sh`, `keys.ps1`, `winevent-hook.ps1`, `desktop-accname.ps1` and `nameless-window.json`. Kept: the five cited captures (`two-same-named-dialogs.txt`, `classic-context-menu.txt`, `keyboard-opened-context-menu.txt`, `combo-box-drop-down.txt`, `provider-description.txt`; 20,921 bytes) and the six scripts they name (`two-same-named-dialogs.ps1`, `window-ancestry.ps1`, `forms-list.ps1`, `dropdown-chain.ps1`, `context-menu.ps1`, `menu-hook.ps1`; 17,123 bytes).
- `experiments/the-settings-frame-comes-in-front-after-its-foreground-event/`: 95,834 bytes go (`main.rs`, `readme.md`, `Cargo.toml.txt`, `runs/`). Kept: `nvda-capture.txt` (5,982 bytes).
- `experiments/the-listeners-read-below-another-processs-window/`: 511,788 bytes go (`main.txt`, `cto2.txt`, `fix.txt`, `fix-outpost.txt`, `boundary-collision.patch`, `stop-below-the-window.patch`), and `summary.py` and `fix_summary.py` (6,367 bytes). Kept: `readme.md`, `main-summary.txt`, `cto2-summary.txt`.
- `experiments/readme.md` 38 and 40 are trimmed to the files kept. If a citation of a folder is read as covering every file in it, M1 shrinks to 628,589 bytes.

The fact programs facts Q1 deletes: `another-clients-stuck-call-holds-an-events-cache-fetch` (147), `desktop-wide-focus-handlers-in-a-second-process-deliver-late` (297), `focus-for-a-newly-opened-or-shown-window-is-not-held` (126), `parallel-client-threads-gain-at-most-twice-on-one-application` (253), `uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler` (661), the guides `requests-per-client-operation`, `the-uia-channel-and-the-provider-io-thread` and `uias-client-threads-and-focus-tracking-calls`, and, after facts M3, `a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all` (163). Every claim stays in `uia.md` and `uia-bugs.md` with its evidence in git.

What today's documentation correction leaves, since it corrects stale statements only:

- The structural work: splitting the four RTCA files, `progress-bars.md`, `m42-parity.md` and the cross-process study into explanation pages and decision records, moving the state tables, repointing the code comments, and deleting the files.
- The deletions: the experiments files, the uia-facts programs and folder, the dead uia-facts code, the guides' restating sections.
- Decision records whose limits describe removed code (ADR 0001's limits) and the "Amended" lines, which are edits to decisions of record rather than stale statements.
- The documents each later build changes (ADR 0003 and the focus design in build 2, ADR 0001 and ADR 0003 in build 4, and the crate guides after builds 3 to 5). Each build edits its own; build 7 checks the result.

### Tests to add

None; `cargo xtask docs` checks the links and paths after each move.

### Rulings and defaults it rests on

- The writing convention in CLAUDE.md: once built, a design becomes an explanation page and a numbered decision record, and the design file is deleted; a decision of record carries no amended-on line.
- Section 9: the five fact programs and three guides deleted, by the folder's own rule.
- The roadmap's clearing-up step of "Readers tell, Core asks" ("the explanation pages and crate guides describe what is built") covers the RTCA files, the dead guide sections and `ownership.md`'s stale lines; doing them here is that step's own work. Whether that step includes splitting and deleting the four design files is not written; the convention requires it, so this plan treats it as included.

### What it must not do

- Keep `communication.md` until M6.3's audio step is built; its split is that phase's (documents D11).
- Keep the cited experiments files and scripts named above; keep `parallel-uia-calls` whole (its tools run three modes); keep `ownership-study/architecture-study.md` (cited by `docs/design/ownership-milestone-rules.md`, which holds rules for planned milestones).
- `docs/flaky-tests.md` 66 and 145 and `test-coverage.md`'s `tray_list_and_task_bar` are not stale.
- Whether `ownership.md` is history or an explanation page, how far crate guides restate the code, whether `test-coverage.md` lists test names, and the ledger's copying of pinned counts are with the process revision (documents D9's question, D14, D15, D4); fix only stale statements in them here.
- The roadmap's status lines and confirmed-defect list (documents D3, D5) wait for Dickson's answer below.

### Size budget

About 3,050 document lines with the clearing-up step and up to about 1,450 more as the process revision decides (REPORT's figures); 1.1 MB of experiments; about 1,780 lines of uia-facts Rust.

### Done when

- `docs/design/readers-tell-core-asks*.md`, `progress-bars.md`, `m42-parity.md` and `cross-process-calls-study.md` are gone; `git grep -n "design/readers-tell-core-asks\|design/progress-bars\|design/m42-parity\|cross-process-calls-study"` over `crates`, `xtask` and `uia-facts` finds nothing; `cargo xtask docs` passes.
- `verbatim-core.md` names no identifier that the documents record's identifier check finds absent from the code.
- The uncited experiments files and the deleted fact programs are gone and `uia-facts.yml` builds the rest.

## Build 8. The request list closed: the design study, then the second proof

Built (2026-11-22): 8f2093e3, 6f56d812, 78a9641f, 62c6b288, 2fdc3d5f, 4735d977, 68b74ef5, a284654e, 3ec949c2, 6cb318f3, 9619b41c, 015e80dc, 145fa267, 8a824534, e786784b, 653cb334, 703139cb, 527affab, 9b651d75, 84fe2dc8, 9ec01c14, 2a58bc01, 16660469, 8d41b4e8, 89d2f298 and the documents in 57f0bb7b.

### Purpose

The centre of the strategy: build what ADR 0003 decided, one request list of generic operations shared by Core's features and the extension surface, so that a new behaviour adds no kind of request and the twins stop being written by hand. It starts as a design study during builds 3 to 5, measures its premise first, and ends with the second proof: one ordinary behaviour rebuilt on the generic crossing and counted against NVDA.

### What ADR 0003 decided, against what is built (model record)

The decision, ADR 0003, "Properties, event kinds and requests (Dickson, 2026-11-13)", line 52: "Properties and event kinds are each declared once. A property's entry gives its name, its type and how each interface reads it. A read names a set of properties fixed at compile time; the cached request and the remote program are built from that set, so every field a read needs is cached and costs nothing extra, and the compiler checks that a rule uses only what its read asked for. Extensions name entries of the same table at run time." Line 53: "One request list is shared by Core's own features and the extension surface: properties of an object, navigation, find a descendant by property, and text reads. A new command adds no kind of request."

What is built: one `PropertyId` enum used by every reader and Core; the `detail` column, which drives the settings' filtering of a read's set; the `id` list; the property's `name`, ready for M5. What is not: no reader reads the `uia`, `msaa`, `ia2` or `cost` columns; no read names a compile-time set (every set travels as `Vec<PropertyId>` chosen at run time); the UI Automation cache requests are built from `verbatim-uia`'s own shapes; no rule reads a typed set. Eight hand-written mappers do the work (outpost `asked.rs` `values`, `read_apart` and an MSAA relation match; listener `asked.rs` `values` and `checking.rs` `cached_of`; outpost `worker.rs` `told_property`; Core's plain reader; and three mappers back to `NodeSnapshot` with silent defaults). Of the event-kind table only `cached` is read.

The settled counts:

| Kind | Count | Serving exactly one behaviour |
| --- | --- | --- |
| `Request` variants | 15 | 6: `Navigate`, `Activate`, `DumpTree`, `ShellItems`, `ShellItemPosition`, `AwaitShown`; `Find` and `Children` serve none yet |
| `TextOp` variants | 13 | 9, serving 7 behaviours: `StartingRead`, `Watch`, `MoveBySentence`, `ReadSelection`, `Select`, `MoveCaret`, `ReadAhead`, `TerminalHistory`, `TerminalFind` |
| `Reply` variants | 16 | 8: `CandidateSame`, `Navigated`, `Activated`, `Tree`, `ShellItems`, `ShellItemPosition`, `HiddenDialog`, `Shown`; `Found` and `Children` none |
| `TextReply` variants | 22 | 16 own and 6 shared failures; `Moved` and `Selected` never cross the pipe |
| Message kinds by direction | 29 (35 variants) | 13 to Core (with the dead `NotResponding`), 8 to the listener, 8 to an outpost |

The shapes of request that would have needed no change, for each behaviour the records trace:

- The dialog read once shown (bc99bc40, 51 files, 3,718 lines; 67 files and 4,931 lines with its follow-ups): facts, not a mode, with every focus read reporting its top-level window's visibility and Core deciding; a generic wait, either "tell me when this kind of event happens on this object" or a registration that tells, as `Follow` already is; a cancel carrying only whether to abandon the call. Build 2 already takes the tell.
- Control+Backspace says the whole word (e380a0ce, 28 files, 633 lines added, 159 removed): `StartingRead` carries a list of the generic `TextRead { at, movement, unit }` (model `text.rs` 533-540), made in its one program, with `for_motion` in Core; Backspace's word is then `TextRead { at: Caret, movement: Some(TextMovement { unit: Character, count: -1 }), unit: Word }` and `StartingUnit` goes. The uia-rops program must then run generic reads under the 10,000-instruction limit.
- A tree view item's child count (8ec23de0, 4 files, 56 lines): with the table cut to what is read, the model change is the variant alone; with `read_apart` keyed by the property, the outpost's three places are one.
- A selection's item name read afresh (7eb9035a, 7 files, 78 lines): every read answers the fresh name, which came in the same cache request, and Core, which holds the tell's cached name, chooses; `Purpose::Selection` goes and the reader judges nothing.

### Before the design: the measurement its premise needs

- The cost of one round trip: Core to an outpost and back with an empty operation, and Core to the listener's reading thread and back, median and worst, on x64 and on GitHub's ARM64 runner, with the wake-ups on each side. `crates/verbatim-process/examples/crossing.rs`, the measurement tool the build notes cite, is the starting point; the build notes record 0.65 ms for a reading thread's read and 0.45 ms for a reading thread's start.
- The cost of a request that is a list of operations: Control+Backspace's starting read as a list of generic `TextRead`s run as one remote program, against today's purpose-built program: executed instructions (each program's worst case kept under 10,000, the limit the call-count record attributes to its owner), processor time in the outpost, and calls; the same list run as ordinary calls by the classic executor, counting calls.
- The number of added round trips each present one-behaviour kind would cost if decomposed, against one list.
- The fallback count from build 5, which decides whether the executor of ordinary calls replaces every twin.

### The design study's questions

1. The closed list of operations on objects and on text: properties of an object by a set of `PropertyId`s, navigation, find a descendant by property, children, the generic text reads, select, move the caret, activate. Is a wait (the caret watch, a show) an operation, a registration that tells, or the outpost's mechanical work as ADR 0003 already lists ("read, compare a caret with a starting point, ... move the caret")?
2. A request as a list: how each result is addressed in the reply, whether a failed operation ends the list or the rest continue, how node ids minted inside a list are stamped (the model record's four walks over node ids collapse into one), and how anchors cross.
3. One program builder that turns a list into a remote operation, and one executor that runs it as ordinary calls, so the twins stop being written by hand; which size-limit paths keep a classic read (a controlled list over about 280 items, a terminal row over about 1,300, dialog children beyond 60 per run).
4. Properties declared once: does the table drive the readers' matches, the `read_apart` dispatch and ia2's subscriptions (option B of question 2 in REPORT), with Core parsing answers once at the boundary; how this fits `uia-shapes.md`'s decision 1 of 2026-10-13, which keeps UI Automation's shapes as the cache declaration.
5. What each present one-behaviour kind becomes (below).
6. One object abstraction per backend, so a walk ported from NVDA, such as a dialog's text, is written once. The records show dialog text read per backend today: ia2's `DialogObject` with its own copy of the MSAA rules (apis F8), UIA's `DialogText::parts`, and the outpost's `dialog_text.rs`. The shape of "an object trait with two adapters" is REPORT's move 3, not a finding of the records; the study tests it on dialog text.
7. The MSAA name, value, description and state rules in four copies (apis F8, removed in build 5) become the one MSAA read the abstraction's adapter uses.
8. The plain reader: today `plain.rs`'s `subject` classes all 15 wire kinds, so each new kind must be classed there (focus Q1); with a list of generic operations it answers operations, not kinds, and its size falls.
9. The control protocol and the extension surface: the list is shared with M5's build B, a public contract, so the study's request list is put to Dickson before M5's build B is designed.
10. The listener's focus walk stays call-by-call by decision; the study says which readers run which operations.

What each present one-behaviour kind would become, for the study to confirm or correct:

| Kind today | What it would become |
| --- | --- |
| `Navigate`, `Find`, `Children` | operations in the list (ADR 0003's generic kinds); `Find` and `Children` are kept for M5 (model 19) |
| `Activate` | an operation in the list |
| `DumpTree` | the control plane's dump as a list of children and property reads, or kept as a tool request outside the list |
| `ShellItems`, `ShellItemPosition` | converted in this build to `Find` on a window named by its class path (Dickson, 2026-11-20) |
| `AwaitShown` | gone in build 2 |
| `StartingRead`, `MoveBySentence`, `ReadSelection`, `ReadAhead` | lists of generic text reads, with the rule for which key reads where in Core |
| `Select`, `MoveCaret` | operations in the list |
| `Watch` | the open question 1 |
| `TerminalHistory`, `TerminalFind`, `ReadTerminal` | the terminal rebuild's window read as an ordinary text request (terminals' shared mechanism 1), in build 9 |
| Modes on a request: `HiddenRoot`, `Purpose::Selection`, `StartingUnit`, `Depth::FullUnlessSame` | facts the read reports, with Core deciding |
| Replies `CandidateSame`, `Navigated`, `Activated`, `Tree`, `ShellItems`, `ShellItemPosition`, `HiddenDialog`, `Shown` | results of list operations, or gone with their kinds |

### The second proof

Which behaviour: Control+Backspace saying the whole word deleted (e380a0ce), recommended over holding Enter (7431d850). Control+Backspace crosses the boundary through exactly the request the model record traces to a generic shape; it touches no behaviour ruled beyond NVDA; its scenarios exist (the password and editing scenarios). Holding Enter is bound up with the hook's holding of keys, a ruled Verbatim-only behaviour ("no key overtakes a held key"), which would muddy the count.

How its lines are counted against NVDA:

- NVDA's lines: `editableText.py` 293-319, 27 lines (the text record; REPORT gives 20 to 30). For holding Enter, `script_caret_newLine`, `editableText.py` 208-239, 32 lines (the keys record).
- Verbatim's lines: the product lines that exist only for the behaviour once rebuilt, measured by deleting the behaviour on a scratch branch and taking `git diff --numstat` of product files, tests and documents left out. Generic operations the behaviour needed but others also use are not counted, and are listed beside the count so the review can see them.
- The stopping rule from REPORT move 5: if the rebuilt behaviour is not within about three times NVDA's lines (about 80 for Control+Backspace), stop and look again before M5, and bring the count to Dickson.

### Rulings and defaults it rests on

- Dickson's rulings of 2026-11-20 on the list's draft ([build8-operations.md](build8-operations.md)): an operation may carry a test on an earlier result (matches a matcher, was found, or the negation of either; no loops, branches or variables), dropped later only if the round-trip measurement shows guarded reads are rare; M5's extension surface imports the reads the Extensions guide's imports page lists, each one call Core turns into a batch, with batches not exposed to modules and one matcher grammar for Find and the tests; ShellItems and ShellItemPosition are converted in this build to Find on a window named by its class path.

- Dickson's strategy ruling: finish ADR 0003's generic crossing; the second proof is one ordinary behaviour built on it and counted against NVDA.
- ADR 0003's decision quoted above, which the study carries out rather than changes. Option A (accept a request kind per behaviour) is not taken.
- The twins' default, through build 5's count.
- The plain reader is kept as ruled; its size falls with the list (section 9).
- `Find` and `Children` stay for M5 (model 19; no ruling asked to remove them).

### What it must not do

- Do not reopen the process layout (D9, D13) or move judging out of Core: three designs that moved key and focus judging to the readers failed review.
- Build nothing on the list before the measurement is in and the study's design is approved; the freeze holds until the second proof's count.
- Do not put the extension surface's shape into M5 without Dickson: it is a public contract.

### Size budget

The study itself writes no product code. The list's machinery is sized in the study; the second proof's own lines are budgeted at about three times NVDA's (about 80 product lines for Control+Backspace), with its tests at the cheapest level that runs the real code.

### Done when

- The round-trip and list measurements are recorded and cited by the study.
- The study's design is approved after one adversarial review and the request list is put to Dickson as M5's shared surface.
- Control+Backspace is rebuilt on the list, its deletion count is written beside NVDA's 27 lines, and the next step follows the stopping rule.

## Build 9. The terminal rebuild (roadmap M6.4), from a corrected study

### Purpose

Rebuild terminal reading from `docs/design/terminal-rebuild.md` once the study is corrected against the code: every feature Dickson ruled kept, the skip count in the form his ruling of today allows, and each place where the study would change what is spoken settled before the design. It runs after the second proof, as roadmap M6.4.

### Corrections the study needs (terminals S3, S4, P15 and section 1)

- Today's size is about 13,970 product lines, not 11,970: the study leaves out the colour read (740), the dead rows operation (198), the outpost's handlers (243), the wire types (305) and the 508-line codec in `verbatim-process/src/message/raw.rs`.
- Today's tests are about 22,600 lines and 48 MB of expectations, not about 17,000.
- NVDA's counterpart is 912 lines (595 without the older consoles' TextInfo), not about 800.
- Its inventory misses Core's caret record, which every full terminal read feeds (`terminal_model.rs` 800-802, `editing::update_caret`); the study must say what feeds the focus's caret in a terminal after the rebuild (S3).
- Its "Size" section and questions 4, 5, 7 and 8 predate its own rulings header, "Rulings so far (Dickson, 2026-11-19)", which names F2, F5, F7, F8, F11 and F12 as kept (S4).
- Its citations into shared code have drifted (`entries.rs` and `reduce.rs` line numbers), and one NVDA citation is wrong: `winConsoleUIA.py` 408-414 is `_get_diffAlgo`; the whole-document read is `LiveText._getText`, `behaviors.py` 446-453 (P15).
- It does not list as questions the eight speech differences or the four state-table cells below (S1, S2).

### The feature table, with each feature's cheapest form (terminals section 2.2)

| Feature | What it says or does | Today, product | Cheapest form (the reviewers' estimates) | Cheapest form changes speech? |
| --- | --- | --- | --- | --- |
| F1 flood groups and skip counts | first 30 lines, "skipped N lines", newest 30 | about 610-900 | about 120-250: the bound written once, one "drop the oldest past keep, count them" function | only as Q13 below |
| F2 holding reads while speech waits | no read while 30 lines wait | about 250-300 | about 40, the study's derived hold | in a long flood (Q9) |
| F3 exact counts and the row model | "skipped 9940 lines" through a full history | about 3,500-3,800 | about 200: the outpost keeps one range at the last read's end and counts rows from it in the same remote operation, "more than" when refused | per Dickson's ruling, below |
| F4 memory across focus changes | review position and marks survive Alt+Tab | about 250 | about 50-60 | no, if F11 and F12 keep their places |
| F5 an unfinished word held | streamed replies | about 250 | about 100-120 | no by intent; needs recordings |
| F6 a changed line from its first changed word | vim's ruler; the spinner silent today | about 70 | 70, plus the spinner rule (Q6) | yes unless the spinner rule is kept |
| F7 volatile rows | "Working... 1s" | about 450-500 | about 150-200, compared by place | needs recordings |
| F8 keys owning changes, lists, suggestions, colours | "> banana" in a list; emphasis | about 2,500-2,600 with the colour read | about 700 (one key window in the watch) to 1,200 (screens before and after compared by place) | intended no; needs recordings |
| F9 typing echoed as the terminal shows it | a password prompt with a clock | about 80-300 | about 80 | not by itself (Q2, Q8) |
| F11 review over Core's snapshot | review position kept while output arrives | about 1,000-1,400 | about 200: rows from the document's end plus the line's text | needs recordings |
| F12 moving by command | Verbatim+Alt+Up and Down Arrow | about 400-1,070 | about 250, plus a find written once (about 200) | needs recordings |
| F13 report status bar | Verbatim+End | about 100 | 100 | no |
| F14 a new screen speaks its screen | `less` closed | about 20-300 | 20 | yes in floods (Q10) |

The record's table has no F10.

### Dickson's ruling on the skip count, and the form that satisfies it

The ruling: "Exact when it is easy and cheap to give an exact count. Otherwise, provide some sort of estimate when possible, e.g skilled over 1000 lines" ("skipped over 1000 lines").

The form: the outpost keeps one range at the last read's end between reads and counts the rows from it to the new screen in the same remote operation, about 200 product lines (the A reviewer's form, terminals Q14). When the range holds, the count is exact at no added round trip. When it is refused or lost, the read says an estimate from what it saw ("skipped more than N lines", where "more than" and "about" are already ruled phrasings since 2026-10-25), and when nothing can be estimated, "skipped lines". This departs from the study's "It keeps nothing between reads: no start anchor, no scope", which the corrected study must change. Today's row model (about 3,500 lines) and its 4,000 test lines go either way. Cases where today's exact count needs the row model's alignment (a full history, an aside screen) become estimates.

### The eight places where the study's speech differs from today's (S1), with the default to keep today's ruled speech

| Question | Today | The study | Default |
| --- | --- | --- | --- |
| Q1 blank lines in the flood count | counted when skipped (ruled 2026-10-07; `blank_lines_count_when_skipped`) | dropped by NVDA's diff before the count | keep: pass blank lines to the pacing, counted, not spoken (about 10 lines) |
| Q2 output of one character | "s" then "*" at a password prompt (TM rule 20; parity "Typing") | NVDA's two drops: "s" only | keep TM rule 20 |
| Q3 output written during a hang | "alpha", "beta" once the window answers | the recovery's re-take baselines silently | keep; no dated ruling, and NVDA by its source also speaks it; capture |
| Q4 a baseline refused or past its due time | a new, silent baseline at the next tell | the whole window spoken as new output | keep: split the cell into "abandoned: baseline again" and "failed: empty" |
| Q5 an output read past its due time | one read sent again, "Finished" spoken | not read until another change | keep: `DueTime` from Out goes to Idle dirty, `NotResponding` and `Hung` stay clean (no lines) |
| Q6 the spinner | silent (TM rule 16) | the glyph spoken with a new line in the same read | keep: add the spinner rule to F6 |
| Q7 redraws, rules 9 and 10 | silent when a read lands inside Ink's erase and redraw | the box spoken again | keep, in a form that does not need F3's row model; needs a recording |
| Q8 NVDA's Tab exemption | every completion spoken (TM rule 20) | NVDA's drops with no Tab exemption | keep TM rule 20 |

### The four state-table cells to settle (S2)

- Q9: F2 derived from the waiting count against F1's look. In a flood whose reads bring up to two screens, the look runs on lines read before the hold and the next group is stale. Settle by making the look wait for a read sent at the group's last line, derived from what the queue holds, which keeps today's speech ("skipped 9940 lines" then the newest 30).
- Q10: F14 and F1 on a read that shares nothing with the last. The study does not say which wins; settle by the corrected study's capture, keeping today's speech where a test states it.
- Q11: the outpost ending while lines wait. Add the column; keep today's (what was read is still spoken, as NVDA queues every line it read; `terminal/tests.rs` 1616).
- Q12: the desktop switch. `sleep` drops the focus record and the watch with it; what the study leaves open is whether paced lines waiting at the switch are dropped. Write a test of today's behaviour first, then keep it.

Two more questions of the record: Q13, each group handed to speech whole, about 130 lines fewer, changes what is heard when "report new output" is turned off mid-group; the default keeps the study's pacing. Q14 is settled by Dickson's ruling above.

### Recordings and captures needed

- Recordings of terminal programs for the features kept "each checked against recordings in the phase": a streamed reply (F5), volatile rows such as "Working... 34s" (F7), keys in fzf, tig and Microsoft Edit with their colours (F8), review while output arrives (F11), marks (F12), Claude Code's Ink erase and redraw (Q7), and a spinner with a new line in the same read (Q6).
- Captures at the pin: a password prompt with "speak passwords" on (Q2); a console written to while hung (Q3); a spinner glyph with a new line (Q6); Ink's redraw (Q7); a two-character tab completion (Q8); a read sharing nothing with the last (Q10); speech queued behind a whole group (Q13); and a flood of 30, 10 blank and 30 lines to confirm NVDA's diff drops the blanks (Q1).
- `conhost_typing`'s recorded reads for the wrap defect's test.
- Claude Code: the committed term-sim recordings (`claude-code`, `claude-code-reply`) were made with Dickson's Claude Code in screen reader mode (`"axScreenReader": true`; each screen starts "[Screen Reader Mode: on via settings]"), so they are not taken as covering the spinner or Ink's redraw. Recordings and captures are made in both modes, the mode turned off for the capture's process alone with `claude --settings '{"axScreenReader":false}'`, and checked by its first screen. Dickson allowed one short, content-free prompt per capture (2026-11-20).

### Inventory

| Record and id | Title | Files and lines | Product | Tests | Speech changes |
| --- | --- | --- | --- | --- | --- |
| terminals S1, S2, S3, S4 | the study's speech differences, cells, missing caret record, stale size and questions | `docs/design/terminal-rebuild.md` | 0 | 0 | as settled above |
| terminals P1 | "a release read is out" held three ways; defect T1 is their drift | `reading.rs` 102, 283-285, 521; `terminal.rs` 98-104 | 40 / 0 | 0 / one T1 test | fixes T1 |
| terminals P2 | terminal state in three homes, reset from four places | `terminal.rs` 1114-1123; `reading.rs` 315-368, 1131-1137; `input_desktop.rs` 101 | in the rebuild | 0 | no |
| terminals P3 | the flood policy's drop rule written three times | `terminal.rs` 604-817; 412-420, 528-536 | 200-250 / 0 | 0 | no |
| terminals P4 | mirror types converted field by field; padding removed in three crates | `terminal/items.rs` 9-98; `terminal_model.rs` 491-650 | 150-230 / 0 | 0 | no |
| terminals P5 | the setting, the focus's arrival and the key window each held twice | `terminal.rs` 394; `terminal_model.rs` 126-853; `terminal_reading.rs` 594-599 | in the rebuild | 0 | no |
| terminals P6 | copies of the event queue's in-flight state | `reading.rs` 142-153; `terminal.rs` 105-108, 287, 303 | in the rebuild | 0 | no |
| terminals P7 | the fresh-read glue twice | `terminal/commands.rs` 85-112; `keys/caret_key.rs` 752-788 | 25 / 0 | 0 | no |
| terminals P8 | caps standing in for an owner's release | `terminal_model.rs` 64, 69, 340, 441, 473; `review.rs` 44 | in the rebuild | 0 | no |
| terminals P9 | a line's life as eleven fields; a parallel row array | `terminal_reading.rs` 305-426 | in the rebuild | 0 | no |
| terminals P10 | silent fallbacks | `terminal.rs` 807-811, 1040; `reading.rs` 233-238, 618-623 | in the rebuild | 0 | no |
| terminals P11 | `Step.withheld` read only by the oracle | `terminal_reading.rs` 137-166, 257 | 60 / 0 | with the oracle | no |
| terminals P12 | identity by value conventions | `terminal_model.rs` 73; `terminal.rs` 562-571; `terminal_rows.rs` 2805 | in the rebuild | 0 | no |
| terminals P13 | a 760-line function | `terminal_reading.rs` 907-1668 | in the rebuild | 0 | no |
| terminals P14 | every terminal remote operation written twice | `terminal_snapshot.rs` 315, 538; `terminal_colours.rs` 580, 666; `terminal_find.rs` 121, 320 | about 700 if one path | 0 | no |
| terminals P15 | the study's citations drifted; one NVDA citation wrong | the study | 0 | 0 | no |
| terminals Q13, Q14 | groups handed whole; F3 as an outpost-held range | as above | as above | as above | as settled |
| focus F14 | `observed_at_us`, 0 for Core's own reads, passed to a callee that ignores it | `execute.rs` 611; `verbatim-text/src/terminal_reading.rs` 669-670 | 3 / 0 | 0 | no |
| keys M5 | the edit and terminal caret paths repeat the stage table and loop | `caret_key.rs` 268-375, 605-640, 716-744 | 90 / 0 | 40 / 0 | no |
| keys M19 | the terminal's copy of the deferred keys; a console caret tell dropped twice | `keys/terminal.rs` 49-87; `caret_tell.rs` 207-218 | 50 / 0 | 0 | no |
| model 17 | `raw.rs` mirrors `Answer`, `Reply`, `TextReply` and `TerminalSnapshot` | `verbatim-process/src/message/raw.rs` 40-110 | 100 / 0 | 0 | no |
| outpost 22 | the terminal's second UTF-16 converter and its agreement tests | outpost `terminal.rs` 37-43; `terminal/tests.rs` 38, 69, 213-214 | 60 / 0 | 290 / 0 | no |
| text B14 | say-all's grid fact derived twice, differently | Core `say_all.rs` 126-138, 346-350 | 10 / 0 | 0 / one test | terminal only |
| scenarios 18 | three terminal scenarios per terminal each launch to run an echo | `terminal_commands.rs` 78; `terminal_editing.rs` 100; `terminal_typing.rs` 93 | 0 | merged | no |
| facts M10 | raw `.tsv` runs kept beside their summaries | `experiments/the-console-host-raises-its-caret-events-on-its-blink-timer/runs/*.tsv`; `experiments/a-terminal-read-ends-at-the-document-end-it-started-with/runs/*.tsv` | 1,426,434 bytes, once the citing pages are rewritten | 0 | no |

Carried here from other builds, in code this rebuild rewrites: the three terminal dispatchers' any-error fallback (apis F4, calls F4), the terminal half of "gone" in two encodings (text B4), `update_caret`'s terminal caller (text B6), the terminal timer's wake outside the queue (queue F19) and, if build 3 left them, the model's eight old key variants (queue F5, keys M1).

### The smallest test set

One Core test per recorded defect, each fed in the listener's and outpost's order, about 30 to 40 lines each and about 400 in all (terminals 3.3):

- T1: a flood held, the re-take commits while the release read is out; the next output is spoken.
- T2: a caret key's read out under key 8, "z" typed as key 9; "z" is spoken.
- A cut by a modifier alone, then terminal output; it is spoken (6a9e55dc).
- The window read answered "no text" takes the failure cell (75b5fb5c).
- "Review current line" with a failed fresh read ends saying nothing (33b0523c).
- The outpost ends while lines wait: nothing asked of it, the lines spoken (b7038a48).
- `conhost_typing`'s recorded reads of a typed line's wrap: nothing said for the empty row (f0902400).
- A key reaching the program during a read: its echo goes out under that key (ffce203b).
- Up Arrow then "t" with "cat a.txt" recalled (f4fb5828, 1adc1421).
- An output read past its due time sends one more read (Q5).
- A hung terminal's output spoken once the window answers (Q3; the existing test kept).

Beyond these, one test per end-to-end family with a capture replaying a recorded stream through the simulator into Core and asserting the captured speech (about 40 lines each), and the simulator's conformance test against recordings, which CLAUDE.md requires of a stand-in.

### Rulings and defaults it rests on

- Dickson's ruling on the skip count, quoted above.
- The roadmap's M6.4 entry: "Every existing terminal feature is kept unless Dickson authorizes its removal (Dickson, 2026-11-19); among them, by name: keys owning the changes they cause, lists and colours (fzf, tig, Microsoft Edit), since many terminal programs need them; the review cursor over the terminal's text and moving by command; holding reads while speech waits during a flood; an unfinished word held; volatile rows."
- The terminal model's dated rules (TM rules 7, 9, 10, 13, 15 to 24, the flood policy) quoted per question above; the default keeps today's ruled speech in each of the eight places.

### What it must not do

- The B reviewer's floor of about 3,860 lines drops F3, which needs Dickson's authorization; with F3 at about 200 it is about 4,060.
- The study's pacing table holds with F2 derived from the waiting count only while fewer than 30 lines wait behind the group (Q9); do not build it as written.
- At the desktop switch, `sleep` drops the focus record, so a watch held in it goes with it; do not design for a watch alive in a sleeping focus.
- `lines_of_one_and_two_characters_after_enter_are_output_not_echo` is not changed by NVDA's drops (its read inserts three lines).
- NVDA's `_getText` has no timeout of its own in the pinned lines; whether UI Automation's call gives up first on a hung console is unknown.
- The skip count's design must not keep today's row model under another name: the ruling allows an estimate where exactness is not cheap.

### Size budget

About 3,300 product lines (every feature kept, F3 as the outpost-held range, F8 in its smaller form) to about 4,060 (F8 in its larger form), 3.6 to 4.5 times NVDA's 912, against about 13,970 today; tests about 3,000 to 4,000 against about 22,600. The owner's 1.3 target (about 1,190) is not reachable with every feature kept; F8 alone is 60 to 100 percent of it. Whether this size is acceptable is a question below.

### Done when

- The corrected study is approved after its adversarial review, with every cell of its tables decided and the eight speech differences settled.
- The recordings and captures exist and the features match them.
- The defect tests above pass and each fails with its fix reverted.
- The product lines are within the budget by `git diff --numstat`; the terminal families pass on GitHub and their runs are read; wake-ups and processor time are measured, since the rebuild changes timers on the event path.

## Findings that belong to an already planned roadmap phase

Roadmap M6.4 is build 9, so its findings are listed there. Under the freeze the other phases do not run, so the restructuring among the findings below is done by build 10 of [plan.md](plan.md), whose first step splits each roadmap entry into restructuring, which it builds, and new function, which waits. By phase:

- M4.3 (settings): settings P1, the Terminal page's per-control plumbing; one table of setting rows served by the generic page code would save roughly 100 product lines per setting across M4.3's about 24 controls.
- M4.9 (lints and unsafe): queue F14, `proven.rs`' silent default; speech F19, a second unsafe kernel-event wrapper.
- M5 (extensions): keys M4 and supervisor F8, the gesture meaning in two tables with a silent fallback (the fallback becomes an error in build 3, two lines; the tables merge in M5; the records disagree on the class, the supervisor record saying remove now); outpost 23, `Arbitrator::new(extra_good)`; scenarios 10, the Windows Terminal launch allowance standing in for an unbuilt rule; harness F21, the optional-utterance matcher sized for uses M5 removes; documents D19, `m5-build-a.md`'s dead type; settings P3, the tray list's details (with M7 for `KeepOpen`).
- M6 (the synthesizer host): speech F9, F10 and F16.
- M6.1 (bounded queues): queue F10, work proportional to the waiting line (with an allocation test growing the line to 1,000 entries, about 40 lines, after a measurement under a flood); listener L3, the listener's merge of waiting tells.
- M6.2 (the supervisor's owner thread): supervisor F12, F13 and F14, and the owner's copy of the generation stages from supervisor F1 and `ListenerRecord::pending` from supervisor F10.
- M6.3 (speech and audio pipeline): speech F4, F11, F17 and F18; documents D11, `communication.md`'s split once the audio step is built.
- Exact replay of a paused entry: queue F18, every answer formatted as text and kept twice (and the checkpoint's clone of the state under queue F10).
- Misc fixes: listener L18, the failed-read counters only log lines read.
- M15 (positional sound): speech F20, `Utterance::source`, whose comment names M11 where the roadmap's positional sound is M15.

## Questions that still need Dickson

These are few. Each says what is asked and the session's recommendation. Dickson answered questions 1, 2, 4 and 5 on 2026-11-20 "per your defaults", so each recommendation there is his ruling; question 3 was settled the same day, and question 6 waits for the latency budgets.

1. The terminal rebuild's size. The skip count is ruled; REPORT decision 2 also asked whether about 3,300 lines (3.6 times NVDA) with every feature kept is acceptable, and that is not answered. Recommendation: accept about 3,300 as build 9's target, with about 4,060 as its ceiling, judged again after the second proof's count.
2. The event recorder's drain passes (tools T23). `verbatim-event-record` numbers each WinEvent's drain pass for "the focus-batch design", which no document or roadmap entry records. Is that design planned? Recommendation: if not, remove `passes.rs`, the pass numbering and file format 2's `pass` field (about 100 product and 51 test lines).
3. Settled on 2026-11-20: the roadmap says that the three builds of "Readers tell, Core asks" are built, and the simplification takes in that phase's clearing up. The misc fixes' list of confirmed defects (documents D5) still names removed code in places; each defect is checked against today's code by the build that reaches it.
4. M4.6's second measure and its mutation follow-up (documents D21, facts Q4). The second measure of 2026-11-02 has no verdict, and the follow-up names five deleted files and the nightly workflow build 1 deletes. Recommendation: retire both entries and the `logic-tests-study` evidence they cite.
5. Evidence kept for a call Verbatim no longer makes (facts Q3, Q5). The kept runs and exposure logs that `uia-facts/readme.md` records as excluded by you on 2026-11-04 (601,559 bytes), and the CI test of `UiaAddEvent`, which no Verbatim code calls. Recommendation: replace both with their commits and fold `design-raw-probe.md` into the listener's explanation and the `verbatim-uia` guide.
6. The latency budgets' source (harness F20), not needed now. When the budgets of `testing.md` 64 are built, should they read the collector's per-utterance rows, which nothing reads, or the `verbatim::latency` log lines `cargo xtask measure` reads? Recommendation: the log lines, deciding when the budgets are built.

Left to the builds by the process revision, each with the session's default in [plan.md](plan.md), "What still needs Dickson", or settled by the revision itself (the captures committed with a check, the ledger naming its tests, the standing rules' contradictions, the Commands paragraph): the NVDA captures kept as committed files with a check against their scenario, and compare mode's future (harness, its two questions); the performance ledger copying pinned counts (documents D4, calls F2; the two drifted figures are stale statements for today's correction); exact instruction counts against a bound (the bound half of calls F5); the generated caret-watch test (outpost 21); the key-holding walk's model half (keys Q6); the 18 local-only scenarios and the UIA document path's routine run (scenarios 5); `settings_system_page`'s handling if build 6's triage finds no Verbatim cause; the testing standard's two stale allowance counts (scenarios 3, harness F14's half); the paced step whose outcome is the machine's (scenarios 16); File Explorer's placeholder as a real application or a recorded order (scenarios 17); the crate guides' remit, `test-coverage.md`'s lists and `ownership.md`'s status (documents D14, D15, D9's question); the standing rules' contradictions (documents D22); and CLAUDE.md's Commands paragraph and its `cargo xtask vm` lines (tools T21's half, and build 1).

Defaults in his areas that the builds act on unless he objects, listed so that each build's report names them: the foreground hold's timer moved onto the WinEvent thread (build 4, thread layout); the control protocol's never-produced vocabulary and `live_re_reads` removed at a new protocol version (build 3, a public contract); report focus over the sleeping focus replaced with "says nothing" (build 3, a dated ruling); the outpost's text facts moved to the store (build 4, `ownership.md` 130); review word moves by the provider's word at one round trip per press (build 5, a performance budget).

## Placement counts

A finding is a row of a record's findings table, plus each question or decision a record numbers without a row of its own (dialog's four decisions and three corrections; model's question 2; supervisor Q1 to Q3; core-keys Q6; apis Q2 to Q4; calls Q2 and Q3; terminals Q13 and Q14; harness's two process questions; facts Q1 to Q5). Test findings without an id are placed in the builds' test lists and not counted. Each finding is placed once; where two records describe one item, both ids are listed on its row and each is counted.

| Destination | Findings |
| --- | --- |
| Build 1, defects, tooling and the Hyper-V removal | 62 |
| Build 2, the dialog read once shown | 13 |
| Build 3, Core | 73 |
| Build 4, the readers | 36 |
| Build 5, remote operations and text | 45 |
| Build 6, tests and harness | 45 |
| Build 7, documents | 29 |
| Build 8, the request list closed | 5 |
| Build 9, the terminal rebuild (M6.4) | 29 |
| Planned phases other than M6.4 | 26 |
| Questions for Dickson | 8, in six questions |
| The process revision | 13 |
| Total | 384, the sum of the twenty records' findings |

By record: dialog 7, queue 20, focus 17, keys 20, model 21, supervisor 17, listener 18, outpost 23, mockapp 17, apis 26, text 14, calls 14, scenarios 20, harness 23, tools 24, documents 22, facts 14, terminals 25, settings 22, speech 20.

Nothing was left unplaced. The records disagree in eight places, each named where it falls: the class lists in Core (focus F4 against outpost 16), `ObjectClass` (focus F7 against model 18), the gesture tables' class (keys M4 against supervisor F8), `terminal_rows` (terminals R1 against apis F23 and calls F4), when the model's old key variants can go (queue F5, keys M1 and model 9), whether the classic walks stay (calls Q3 against the twins default), the desk harness's saving (queue against focus), and how many performance-ledger figures have drifted (four in calls F2, two in documents D4).
