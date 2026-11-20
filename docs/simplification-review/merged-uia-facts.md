# Merged review: uia-facts and experiments

Merged from reviewers A and B. I checked every claim below again at ba89dca7 (2026-11-20) with file reads, `git grep`, `git log` and `wc`. Nothing in this area speaks, so every "speech changes" is no. Sizes are file bytes (`git ls-files | xargs cat | wc -c`). The reviewers' larger figures (4.3 MB, 3.8 MB) were disk usage in 4 KB blocks.

## 1. Size

NVDA has no counterpart: it keeps no reproductions of UIA's behaviour, so there is no ratio. The area is judged on what cites each part, on duplication and on dead code.

| Part | Lines | Bytes | Run by |
|---|---|---|---|
| `uia-facts/support` | 3,980 Rust | | the uia-facts workspace only |
| `uia-facts/provider` | 2,131 Rust | | the same |
| `uia-facts/check-copies` | 185 Rust (+16 manifest) | | `uia-facts/build.ps1` |
| 32 fact programs (B is right; A's 30 is wrong: `uia-facts/Cargo.toml` lists 35 members, three of them support, provider and check-copies) | 11,772 Rust | | built and linted by `.github/workflows/uia-facts.yml`; run only by hand |
| 36 fact folders (32 programs and 4 that hold only a README), the READMEs plus the index | 1,745 Markdown | | |
| `uia-facts/*/recorded/` | 286 files | 3,121,222 | cited by the folder READMEs |
| `experiments/` | 24 folders, 246 files; 6,680 lines of Rust, 11,175 lines of `.rs/.py/.ps1/.psm1/.sh/.patch` | 3,850,549 | never built; `cargo xtask docs` skips it (`xtask/src/docs.rs` 95-101, where `EXCLUDED` holds `"experiments"`) |
| `crates/mockapp/tests/uia_bugs/` | 15 tests, 1,391 lines | | `cargo xtask ci` |

History: 62 commits touch `uia-facts/`. Eight came on 2026-11-05 and 52 on 2026-11-06. The other two are df0700c2 (2026-11-08) and fa592419 (2026-11-17), both documentation changes. No recorded run is later than 2026-11-06.

Deletion target:

| | Tool (product-side) lines | Test lines | Bytes of evidence |
|---|---|---|---|
| Remove now | about 135 Rust (M2), 28 Markdown (M7), 1 workflow line (M6); in experiments, 4,144 lines of code, scripts and patches, 926 of them Rust (M1) | 0 | 1,140,377 (M1) |
| If Dickson rules | Q1: 1,484 Rust in five programs, plus 163 after M3, plus 213 and 149 README lines; Q2: about 310 lines of copy-keeping, plus about 350 net lines of manifests | Q5: about 112 (one test) | Q1: 293,877, plus 570,896 in the guides; Q3: 601,559; Q4: 22,872 |
| Planned phase (M6.4) | 0 | 0 | 1,426,434 (M10) |

## 2. Findings

| id | title | class | found by | file and line | what goes | product lines | test lines | speech |
|---|---|---|---|---|---|---|---|---|
| M1 | Experiments files that nothing cites | Remove now | both (A found all three folders, B the first two) | `experiments/focus-pipeline-rework-measurements/`, `the-settings-frame-comes-in-front-after-its-foreground-event/`, `the-listeners-read-below-another-processs-window/` | 1,140,377 bytes; 4,144 lines of code | 4,144 (never built) | 0 | no |
| Q1 | Fact programs whose own READMEs say nothing rests on them | Question | both (B counted five, A eight) | see the program table below | 5 programs, 3 guides, and M3's program | 1,647 | 0 | no |
| M2 | Dead support and provider code | Remove now | both (B missed `image_of` and `hung`) | `uia-facts/support/src/apps.rs` 43-149, 219-243; `uia-facts/provider/src/main.rs` 68, 77, 444-459 | 5 functions, 2 constants, 2 provider requests | about 135 | 0 | no |
| M6 | The workflow is triggered by a crate it no longer builds | Remove now | A (corrected) | `.github/workflows/uia-facts.yml` 26; `uia-facts/readme.md` 67 | the `verbatim-core` path; the readme's "eight" path crates become seven | 1 | 0 | no |
| M3 | A README describes a recovery that decision 11 removed | Remove now (edit) | B only, confirmed | `uia-facts/a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all/readme.md` 9 | one sentence | 0 | 0 | no |
| M4 | `uia-bugs.md` cites folders and sections that do not hold what it says | Remove now (edits) | A (line 5), B (line 227 and the section numbers) | `books/dev/src/reference/uia-bugs.md` 5, 227, and 12 lines with section numbers | wrong citations | 0 | 0 | no |
| M5 | Other stale statements | Remove now (edits) | A and B, each item marked | see the paragraph | wrong text | 0 | 0 | no |
| M7 | A folder that holds only a pointer, and nothing links to it | Remove now | both | `uia-facts/a-remote-operation-stops-after-exactly-10000-instructions/` | the folder and its row in the index | 28 Markdown | 0 | no |
| Q2 | A second Cargo workspace and its copy checks | Question | both | `uia-facts/Cargo.toml` 58-136, `uia-facts/check-copies/`, `uia-facts/build.ps1` | the copy-keeping | about 310, plus about 350 net in manifests | 0 | no |
| Q3 | Kept end-to-end runs, a built design and exposure logs kept as evidence | Question | A only, facts confirmed | experiments/uiahasserversideprovider-.../kept-runs/ and `design-raw-probe.md`; uia-facts/a-new-connection-fails-.../recorded/ | 601,559 bytes | 0 | 0 | no |
| Q4 | A roadmap follow-up and a nightly workflow name deleted files | Question | B only, confirmed | `docs/roadmap.md` 517; `.github/workflows/explore.yml` 37-42 | `experiments/logic-tests-study/` and 5 dead paths | 5 workflow lines | 0 | no |
| Q5 | A test of an API Verbatim never calls | Question | B only, confirmed | `crates/mockapp/tests/uia_bugs/focus_events_cannot_be_limited_to_one_application.rs` 145-252 | 1 test | 0 | about 112 | no |
| M9 | READMEs name client settings where the setting changed no outcome | Remove now (edits) | both; two of their four rows confirmed | `a-focus-change-.../readme.md` 24; `a-hung-window-.../readme.md` 62 | a few clauses | 0 | 0 | no |
| M10 | Raw `.tsv` runs kept beside their summaries | Planned phase: M6.4 | both (B named the phase) | `experiments/the-console-host-raises-its-caret-events-on-its-blink-timer/runs/*.tsv`, `experiments/a-terminal-read-ends-at-the-document-end-it-started-with/runs/*.tsv` | 1,426,434 bytes | 0 | 0 | no |

### The experiments folders, settled

I found the citations with `git grep -n -o -E "<folder>[^)\` '\"]*" -- books docs crates xtask uia-facts .github CLAUDE.md REVIEW.md`, and with a grep of each cited file name. A citing document counts as in force when it describes the present, is a pending roadmap entry, or holds the rules for planned milestones.

| Folder | Bytes | Cited by (file:line, what) | In force | Stays | Nothing cites |
|---|---|---|---|---|---|
| a-process-scoped-hook-takes-events-by-the-windows-owner | 5,166 | `docs/design/cross-process-calls-study.md`:168, 225 (folder) | yes: the study's decisions are built, and architecture.md and two crate guides still cite it | all | none |
| a-redraw-in-a-keys-window-is-spoken-as-its-answer | 97,681 | `docs/roadmap.md`:1326 (readme.md) | yes, a known limit until M6.4 | all (the readme names its other two files) | none |
| a-replacement-outposts-lookup-beside-its-predecessors-operation | 4,969 | study:140, 228 | yes | all | none |
| a-terminal-read-ends-at-the-document-end-it-started-with | 343,694 | `books/dev/src/explanation/design/terminal-model.md`:126 (folder; its figures, 186 of 200 and 130 of 200, are in readme.md and `runs/*-region-summary.txt`) | yes; M6.4 rewrites the citing page | readme, summaries, main.rs, analyse.py | 5 `runs/*.tsv`, 307,764 bytes (M10) |
| a-walk-up-whose-window-changes-after-its-check | 9,597 | `crates/mockapp/tests/call_counts.rs`:2610; study:156, 226 | yes | all | none |
| accfocus-names-no-other-processs-child-window | 7,133 | study:176, 227 | yes | all | none |
| cocancelcall-ends-an-msaa-com-call-but-not-a-window-message | 103,861 | `docs/nvda/main-loop-and-watchdog.md`:283 and `docs/roadmap-done.md`:99 (readme.md); study:62, 233 (folder) | yes | all (the citation names the folder, and neither reviewer proposed a cut) | none by name |
| cross-process-calls-study-program | 50,909 | study:232; uia-facts/a-new-connection-fails-.../readme.md:39 | yes | all | none |
| focus-pipeline-rework-measurements | 570,799, 70 files | `crates/mockapp/tests/focus_reports.rs`:3162 and `docs/parity/focus-context.md`:70 (`captures/two-same-named-dialogs.txt`); focus-context.md:337 (`classic-context-menu.txt`) and :354 (`keyboard-opened-context-menu.txt`); `docs/design/readers-tell-core-asks-build1.md`:776 and 819 (`combo-box-drop-down.txt`, `classic-context-menu.txt`); `experiments/getfocusedelement-after-the-focus-moves/readme.md`:35 (`provider-description.txt`) | the parity doc and the build 1 design are in force; the readme describes the outpost's one-second gap, which decision 11 removed | the 5 cited files (20,921 bytes), and the 6 scripts the cited captures name in their own text (`two-same-named-dialogs.ps1`, `window-ancestry.ps1`, `forms-list.ps1`, `dropdown-chain.ps1`, `context-menu.ps1`, `menu-hook.ps1`; 17,123 bytes) | 532,755 bytes: readme.md (29,200), debug.patch (38,378), tally.py, batch.ps1, programs.rs, probes-per-new-window.txt, `captures/run/**` (about 307 K, including `listener.log` at 108,321 and `outpost-explorer-3332.log` at 135,453), the 12 other captures, driver.py, lib.sh, keys.ps1, winevent-hook.ps1, desktop-accname.ps1 and nameless-window.json |
| getfocusedelement-after-the-focus-moves | 49,345 | `listener-without-connections.md`:95; `books/dev/src/reference/crates/verbatim-outpost.md`:78; `crates/verbatim-outpost/src/outpost/read.rs`:1314; focus_reports.rs:1989; study:233 | yes | all | none |
| logic-tests-study | 22,872 | `docs/roadmap.md`:517, `docs/roadmap-done.md`:125, `.github/workflows/explore.yml`:33 (judgement.md) | the follow-up is pending, but it names deleted files (Q4) | judgement.md | none |
| ownership-study | 91,007 | `books/dev/src/explanation/design/ownership.md`:3 (folder) and 23, 27, 28, 33 (prior-art.md); `docs/roadmap.md`:1095 (audit-structure.md); `docs/design/ownership-milestone-rules.md`:66 (architecture-study.md, "section 1") | yes: ownership-milestone-rules.md holds "the rules each planned milestone follows" | all | none. A's claim that only a finished work plan cites architecture-study.md is wrong |
| parallel-uia-calls | 117,360 | `books/dev/src/how-to/debugging.md`:5, 156 (stacks.ps1), 161 (`puia threads`), 180 (blocked-summary.py), 191 (dbg.psm1) | yes, as tools | all. stacks.ps1 runs `puia hung` and dbg.psm1 runs `puia probe`, so the program is used for more than `threads` | not settled (see Dropped) |
| security-model-survey | 36,602 | `docs/design/security-model.md`:3 (readme.md) | yes (M6.6) | all | none |
| the-console-host-raises-its-caret-events-on-its-blink-timer | 1,155,616 | `books/dev/src/explanation/design/key-holding.md`:72 (folder; its figures, 530 ms, 257 to 278 ms and 540 ms, are in readme.md and `runs/*-summary.txt`) | yes | readme, summaries, main.rs, analyse.py | 4 `runs/*.tsv`, 1,118,670 bytes (M10) |
| the-listeners-read-below-another-processs-window | 560,193 | `listener-without-connections.md`:3 and 182 (folder; risk 14's figures, 2,325 ms, 317 ms, 40 of 40 and 12 of 12, are in readme.md and the two summaries); study:38, 78, 130 ("The candidate fix", a section of the readme) and 233 | yes | readme.md, main-summary.txt, cto2-summary.txt | 511,788 bytes: main.txt (249,150), cto2.txt (103,402), fix.txt (79,481), fix-outpost.txt (28,676), boundary-collision.patch (21,280), stop-below-the-window.patch (29,799); also summary.py and fix_summary.py (6,367 bytes, not in the 511,788) |
| the-listeners-registration-waits-on-the-foreground-application | 6,873 | study:94, 229 | yes | all | none |
| the-msaa-continuation-waits-on-the-parent-windows-application | 4,030 | call_counts.rs:2422; study:78, 230 | yes | all | none |
| the-runtime-id-search-enters-another-processs-child-window | 43,105 | study:109, 224 | yes | all | none |
| the-settings-frame-comes-in-front-after-its-foreground-event | 101,816 | `docs/parity/windows-and-processes.md`:56 (nvda-capture.txt) | the capture is in force; `experiments/readme.md`:40 says the measurement was "for a proposed foreground wait that was not built" | nvda-capture.txt (5,982 bytes) | 95,834 bytes: main.rs (654 lines), readme.md, Cargo.toml.txt, `runs/` |
| the-terminals-report-no-language-links-or-annotations | 11,343 | `books/dev/src/reference/performance.md`:2111 (readme.md) | yes | all | none |
| uiahasserversideprovider-says-no-... (experiments) | 418,089, 57 files | `listener-without-connections.md`:11 and `crates/verbatim-uia/src/probe.rs`:19, 105, 186 (design-raw-probe.md); `uia-bugs.md`:89 (`kept-runs/`), 102 and 118 (design-raw-probe.md), 122 (folder); `uia-facts/readme.md`:139 ("excluded by Dickson, 2026-11-04") | an inference about a call Verbatim no longer makes (Q3) | all, until Q3 is answered | none by name |
| windows-terminal-raises-focus-from-a-second-runtime-identity | 29,429 | `uia-bugs.md`:415, 603, 625; `books/dev/src/reference/crates/verbatim-uia.md`:396; `docs/roadmap.md`:898 | yes | all | none |
| readme.md (the index) | 9,060 | `uia-bugs.md`:5 | its line 25 cites a deleted file (M5) | yes | |

So 1,140,377 bytes are cited by nothing and need no ruling (M1). Another 1,426,434 bytes of raw runs are for M6.4 to decide. A's "about 1.9 MB" included part of the `.tsv` files. B's "about 0.86 MB" left out the raw outputs of the-listeners-read-below. If Dickson reads a citation of a folder as covering every file in it, M1 shrinks to 628,589 bytes.

### The fact programs, settled

I read the "What Verbatim relies on" section of every program's README (`awk` over each `readme.md`). Citations come from `git grep -n -F <folder> -- books docs crates xtask uia-facts experiments .github`, leaving out the program's own folder. Five programs and three guides say outright that nothing rests on them. A's three extra programs each name a decision or a code comment, so they are not in that set.

| Folder | Rust | recorded bytes | README sentence | Cited by |
|---|---|---|---|---|
| another-clients-stuck-call-holds-an-events-cache-fetch | 147 | 0 | "Nothing recorded: the explainer's model of the worker." | `books/dev/src/explanation/windows/uia.md`:244; `uia-facts/readme.md`:112 |
| desktop-wide-focus-handlers-in-a-second-process-deliver-late | 297 | 70,989 | "Nothing: Verbatim holds one desktop-wide focus handler, in the listener." | uia.md:142; `uia-bugs.md`:299, 303; readme:114 |
| focus-for-a-newly-opened-or-shown-window-is-not-held | 126 | 0 | "Nothing is built on it: it explains why a menu's focus is announced during a hold while focus within an existing window is not, as in NVDA" | uia-bugs.md:182; readme:109; the hung-window folder's readme:19 and main.rs:11; the uias-client-threads guide:58 |
| parallel-client-threads-gain-at-most-twice-on-one-application | 253 | 153,045 | "No figure; D9 gives up parallel queries within one application" | uia.md:238; readme:102; the uia-channel guide:37 |
| uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler | 661 | 69,843 | "Nothing in its code: the raw probe replaced the call (`verbatim_uia::raw_probe`), and Verbatim's workaround is right whether or not the fault remains." | `listener-without-connections.md`:3; uia-bugs.md:81, 122; `docs/parity/backends.md`:121; readme:123 |
| requests-per-client-operation (guide) | 0 | 83,363 | "No code. The provider calls of Verbatim's own operations are pinned by `crates/mockapp/tests/call_counts.rs`." | uia.md:235; four uia-facts READMEs and one main.rs |
| the-uia-channel-and-the-provider-io-thread (guide) | 0 | 29,989 | "No code. The observable outcomes ... are checked by [a provider process serves one UIA request at a time]..." | uia.md:233; three uia-facts READMEs |
| uias-client-threads-and-focus-tracking-calls (guide) | 0 | 457,544 | "No code; the observable outcomes are checked by the hung-window and flood folders." | `listener-without-connections.md`:3; uia.md:242; uia-bugs.md:186; four uia-facts READMEs |
| After M3: a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all | 163 | 0 | today it describes a recovery that no longer exists (M3) | uia-bugs.md:333; readme:115 |
| A only: a-provider-continues-when-a-client-is-killed-in-the-middle-of-its-call | 340 | 0 | names "The clean shutdown of outposts and the listener, which gets 1 second", which rests on a suspicion that the program shows "is not a general property of UIA" | `books/dev/src/explanation/architecture.md`:268; readme:127 |
| A only: a-timeout-bounds-each-request-and-window-message-not-an-operation | 241 | 102,737 | "Only the explanation in `crates/verbatim-supervisor/src/retire.rs` and `crates/verbatim-model/src/limits.rs`" | uia.md:237, 255; uia-bugs.md:551; two uia-facts READMEs |
| A only: real-applications-serve-uia-calls-one-at-a-time | 97 | 690,764 | names D9, then says "That claim is checked against the provider program in [a provider process serves one UIA request at a time]"; its Method says "No case starts a real application" | uia.md:240; readme:126 |

Only one folder is cited by nothing outside itself except the index: `a-remote-operation-stops-after-exactly-10000-instructions` (uia-facts/readme.md:105), M7.

### M1. Experiments files that nothing cites

The rule (CLAUDE.md): something enters or stays in `experiments/` "only when a lasting document cites it as the evidence for a decision still in force, and only the part it cites". The folder table above names every file. The focus-pipeline readme opens "These are the measurements and captures of the focus pipeline rework's first step". It defines "A firing is the outpost's gap reaching its one-second bound", and decision 11 removed that mechanism on 2026-11-07. The settings-frame study served "a proposed foreground wait that was not built" (`experiments/readme.md`:40). The removal also trims `experiments/readme.md`:38 and :40 to the files kept. Check: the grep above, and `wc -c` of every file.

### M2. Dead support and provider code

`uia-facts/support/src/apps.rs`:134 reads `pub fn wait_window(class: &str, exe: &str, before: &[isize], limit: Duration) -> Option<isize> {`. It polls with `std::thread::sleep(Duration::from_millis(50));`, against the readme's rule "Waits are for evidence, a provider's signal, an event or a reply, never a sleep" (`uia-facts/readme.md`:37). The greps:

- `git grep -n -E "image_of|process_ids|top_windows|wait_window|alt_f\b" -- uia-facts crates xtask`: every hit is inside apps.rs. Only `wait_window` calls `top_windows` and `image_of`.
- `git grep -n -o -E "apps::[a-zA-Z_]+" -- uia-facts` finds only `alt_tab`, `class_of`, `foreground_window`, `kill`, `system_program` and `window_pid`.
- No file imports apps' items another way: `git grep -E "apps(::\{|;)"` finds only `pub mod apps;`.
- `escape()` and the constants `ESCAPE` and `F` have no caller. `git grep -n -E "\bescape\(\)"` finds only the definition, and the four other `ESCAPE` hits are verbatim-input's own constant.

The dead lines are 43-69, 71-100, 113-129, 131-149, 219-222 and 229-243, about 116 in all.

In the provider, `uia-facts/provider/src/main.rs`:444 reads `"post-null" | "hung" => {`, documented at lines 68 and 77. I listed every request string sent through `Provider::request`: `busy, focus, foreground, hide, hold, log, menu, menu-close, parent, peak, raise, raise-rate, release, rename, show, slow, winevent`. Neither verb is among them. The one `"hung"` in a program is a return value (`a-provider-continues-.../src/main.rs`:165). That is about 19 lines. Clippy cannot see either kind of dead code: it never flags a library's `pub fn` or a verb read from standard input.

### M3. A README describes a recovery that decision 11 removed

`uia-facts/a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all/readme.md`:9 says: "The recovery, not a workaround: the application's outpost waits a second from the `WinEvent` focus copy for its UIA focus and otherwise reads the focus itself (`books/dev/src/explanation/design/listener-without-connections.md`, decision 11)."

Decision 11 (`listener-without-connections.md`:26) says: "a `WinEvent` focus in a UIA window with no UIA focus following it is dropped, as NVDA drops it, and nothing is read in its place (Dickson, 2026-11-07; ...)". `uia-bugs.md`:333 says "No recovery is kept". The README last changed in 11cdfa63 (2026-11-06), the day before the decision.

Fix: the section should say that nothing rests on the fact, since decision 11 drops such a focus. That puts the program in Q1's set.

### M4. uia-bugs.md cites folders and sections that do not hold what it says

- Line 5 (A): "The experiments live in [`experiments/`] (...), one folder per entry that has them, named after its heading". `experiments/readme.md`:7 says "Every experiment of the two UIA documents has moved there", meaning `uia-facts/`, and one UIA-fault folder remains in experiments. The line should point at `uia-facts/readme.md`.
- Line 227 (B), under the flood entry, cites the flood folder for "the five reruns of P2b's script (`p2b_rerun1.out` to `p2b_rerun5.out`)". `git ls-files | grep p2b_rerun` puts all five in `uia-facts/desktop-wide-focus-handlers-in-a-second-process-deliver-late/recorded/`. Remove the clause.
- Section numbers (B, widened in checking): the document has no numbered headings, yet these lines cite numbered sections:
  - line 159, "E-delivery, section 11";
  - line 163, "the focus pipeline design, section 12.3";
  - line 239, "E1, section 11";
  - line 257, "focus pipeline section 6.1";
  - line 347, "E-TT, section 11";
  - line 363, "section 11, E-TT" and "section 13.2";
  - line 407, "focus pipeline section 16.9";
  - line 535, "E-scope, section 11";
  - line 551, "E-TT, section 11";
  - lines 599, 613 and 617, "section 16.8".

  These numbers name sections of the October experiments readme or of a focus pipeline design, and neither exists in the tree any longer (`git ls-files | grep -i focus-pipeline` finds only the experiments folder). Line 363's "`listener-without-connections.md`, section 10, step 8" is valid, because that document has a "## 10." heading. Replace each number with the uia-facts folder that now holds the experiment, or drop it.

### M5. Other stale statements

- `.github/workflows/uia-facts.yml`:2 (A) says "It runs no fact binary: each one takes over the desktop and judges the machine it runs on". `uia-facts/readme.md`:5 says the opposite: "Each program takes over no desktop unless its folder says so".
- `uia-facts/readme.md`:11 (A) says "On the first build the derived lock held 196 of the main lock's 276 entries: the 93 this workspace does not use were dropped, the 13 uia-facts members added". The workspace has 35 members today. This is a first build's history, in a document that must describe the present.
- `uia-facts/readme.md`:67 (found while checking M6) says "The eight path crates (`verbatim-model`, `verbatim-text`, `verbatim-core`, ...) build once more for uia-facts". Commit 5b5d6094 (2026-11-17, "Drop the dependencies the removal left unused in the logic crates") left `crates/verbatim-reader-rules/Cargo.toml` depending only on `workspace-hack`. Since then no crate that uia-facts builds depends on `verbatim-core`: uia depends on model, text and win32; win32 on model and reader-rules; uia-rops on model and uia. The count is now seven.
- `uia-facts/check-copies/src/main.rs`:2 (B) cites the section "Reusing the main build" of `uia-facts/readme.md`, and `uia-facts/support/src/lib.rs`:1 cites "Shared support". Neither section exists. The readme's headings (`grep -n '^## '`) are Building and running, Writing a fact program, The UI Automation the records were taken with, The workspace, The provider program, Output and verdicts, The facts, and Cited, not reproduced.
- `experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/readme.md`:3 and `experiments/readme.md`:25 (A) cite docs/design/phase6-design.md, which does not exist. `experiments/ownership-study/readme.md`:3 (B) names docs/design/ownership.md, which is now `books/dev/src/explanation/design/ownership.md`. Nothing catches these, because `cargo xtask docs` excludes `experiments`.
- The module documentation of `uia-facts/provider/src/main.rs` (B) lists `--windows`, `--threading`, `--nodes` and `--bridge` (lines 11-18), but not `--tree-view`. The parser accepts it at :146, and `ancestors-cannot-be-cached-or-listened-to/src/main.rs`:211 uses it.

### M6. The uia-facts workflow, and a trigger on a crate it no longer builds

What it runs: two jobs, `ci` on `windows-latest` and `ci-arm64` on `windows-11-arm`. Each runs `./uia-facts/build.ps1`, `cargo fmt --all --check` and `cargo clippy --locked --all-targets -- -D warnings`, and reads the main CI's cache without writing to it (`save-if: false`). It runs no fact program.

Triggers (lines 17-36): `workflow_dispatch`, and any push to main that touches one of these paths:

- `uia-facts/**`
- `crates/verbatim-uia/**`, `-uia-rops`, `-model`, `-text`, `-core`, `-reader-rules`, `-win32` and `workspace-hack`
- `Cargo.toml`, `Cargo.lock`, `.config/hakari.toml`, `rust-toolchain.toml` and `.cargo/config.toml`
- `.github/workflows/ci.yml` and the workflow file itself

Commits since 2026-11-07 (`git rev-list --count --since=2026-11-07 HEAD -- <paths>`): 242 of 628 touch those paths. 106 of them match only through `crates/verbatim-core`, and 2 touch `uia-facts/` itself. Since 5b5d6094, 78 of 202 commits match, 38 of them only through `verbatim-core`, a crate no longer in uia-facts' build graph (see the dependency check under M5).

The fix is to remove the `crates/verbatim-core/**` line. A's wider proposal, dropping model, text and reader-rules too, is wrong: all three are built through `verbatim-uia` and `verbatim-win32`. B's reason ("`verbatim-core`, which enters only because `verbatim-win32` depends on it") is also wrong: `crates/verbatim-win32/Cargo.toml` has no `verbatim-core`. This is a CI choice within Claude's remit, but Dickson should be told.

### M7. A folder that holds only a pointer

`uia-facts/a-remote-operation-stops-after-exactly-10000-instructions/readme.md` has 27 lines and no program. It points at `cargo test -p mockapp --test instruction_limit -- --exact the_instruction_limit_is_measured`. Its only citation is the index row `uia-facts/readme.md`:105, while `books/dev/src/explanation/windows/uia.md`:111 and `books/dev/src/reference/performance.md`:2170 name the test directly. Remove the folder and the row.

### M9. READMEs name client settings where the setting changed no outcome

The rule (`uia-facts/readme.md`:39): "A README mentions a setting only where it changes an outcome, in one short statement with the command that shows it."

Confirmed on two rows:
- `a-focus-change-.../readme.md`:24 reads "once with `--coalescing` 101.5 ms at 1 s", a control 1.5 ms past its bound.
- `a-hung-window-.../readme.md`:62 records Dickson's three runs by default and three with `--coalescing`: "one wait in 7 of 12 runs and two in 5, whatever the setting". Line 61 of the same README is the run that line 62 judges.

Not confirmed: `a-process-that-used-uia-.../readme.md`:32 records "1 of 150, a crash, with `--recovery` and with `--coalescing`" against 0 by default. That outcome differs, so the mention stays.

The fix is a few clauses.

### M10. Raw runs kept beside their summaries (M6.4)

The `.tsv` files (1,118,670 bytes for the blink timer, 307,764 for the terminal read) feed the `*-summary.txt` files. The summaries and the readmes hold every figure the citing pages quote, which I checked by grepping each quoted figure. Both citing pages belong to M6.4: `docs/roadmap.md`:1294 says M6.4 is built "from the terminal model", and key-holding.md is the caret-holding design that M6.4 touches. Decide this with M6.4.

## 3. Test findings

- `crates/mockapp/tests/uia_bugs` (15 tests, 1,391 lines, run by `cargo xtask ci`): one test of about 112 lines can be removed, as a question (Q5). Nothing to replace, nothing to add. Both reviewers found no test asserting the same quantity as a program. I spot-checked the three folders that have both (focus-events, connection-timeout, elements), and each matches its README's split.
- `uia-facts` programs: no routine run executes any of them (the workflow only builds and lints), so none counts as a test. Removable as questions: Q1. Nothing to add.
- `experiments/`: never built. M1 removes 4,144 lines of code, scripts and patches that nothing builds.

## 4. Questions for Dickson

None of these changes speech, and none involves a capture.

- **Q1. Delete the programs nothing rests on?** The proposal: delete the five programs and three guides whose READMEs say nothing rests on them, and, after M3, `a-new-processs-focus-events-...`. Every claim stays in `uia.md` and `uia-bugs.md`, with its evidence in git.
  - Decision touched: `uia-facts/readme.md`:5, "They are run by hand on Dickson's machine, before and after a Windows update, to learn whether a fact Verbatim relies on still holds", and :35 (Dickson, 2026-11-05 and 2026-11-06, per :27), "A case Verbatim does not rely on may run only when named; its claims stay in the documents, and the folder's README says it is run only when named."
  - Facts changed since: decision 11 (2026-11-07) removed what `a-new-processs-...` was relied on for, and the raw probe replaced the call that `uiahasserversideprovider-...` reproduces.
  - Found in checking: four of the five run cases by default (`args.wants`) even though nothing relies on them, which :35 forbids. They are `another-clients-...` (`holds`, `timeout`), `desktop-wide-...` (all five cases), `parallel-...` (`gains`, `shared`) and `uiahasserversideprovider-...` (`focus-handler`). So either the programs go, or their cases become named-only and the rule holds.
  - Cost today: 1,647 Rust lines and 864,773 recorded bytes, rebuilt and linted on two architectures by 242 of 628 commits since 2026-11-07.
  - A's three further programs name a decision or a code comment. They are not part of this question unless Dickson widens it.
- **Q2. Make the facts members of the main workspace?** They would be kept out of `default-members` and out of `cargo xtask ci`'s Clippy and test lists.
  - This removes `check-copies` (201 lines with its manifest), `build.ps1` (29) and the copied tables in `uia-facts/Cargo.toml` 58-136 (79). If the programs become `[[bin]]` targets of one package, it also removes about 350 net lines of manifests.
  - Decision touched: `uia-facts/readme.md`:65, "`uia-facts/` is a Cargo workspace of its own, which the root `Cargo.toml` excludes, so it never joins the main build or `cargo xtask ci`", and the root `Cargo.toml`'s `exclude = ["uia-facts"]`.
  - Neither reviewer names a changed fact beyond the measured upkeep.
  - Cost of the change: classing about 35 members in `xtask/src/tiers.rs`, and a wider `workspace-hack`.
- **Q3. Replace kept runs and logs by their commits?** The proposal: replace the kept end-to-end runs (experiments/uiahasserversideprovider-.../kept-runs/, 404,284 bytes) and the exposure logs (uia-facts/a-new-connection-fails-.../recorded/notepad/, `wt-launch-slow/` and `runs-wt-section20-traced.txt`, 185,036 bytes) by their commits. Also fold `design-raw-probe.md` (12,239 bytes) into the listener explanation and the verbatim-uia crate guide; it is cited by `crates/verbatim-uia/src/probe.rs`:19, 105, 186 and `listener-without-connections.md`:11.
  - Decisions touched: `uia-facts/readme.md`:139, "Its kept runs in experiments/... (excluded by Dickson, 2026-11-04)", and :144, "History, linked to their commits and logs from each entry (Dickson, 2026-11-04)".
  - Changed fact: after 2026-11-04 the raw probe replaced `UiaHasServerSideProvider` (decision 2), so the inference in `uia-bugs.md`:89 is about a call Verbatim no longer makes.
- **Q4. Retire the M4.6 mutation follow-up, or take a new baseline?** `docs/roadmap.md`:517 says "The 22 target mutants the generated tests did not kill, listed in `experiments/logic-tests-study/judgement.md`, 'Targets still missed' ... are the next checks to add". `.github/workflows/explore.yml` `GROUPS` (lines 37-42) still names five files that no longer exist: crates/verbatim-reader-rules/src/in_flight.rs, `crates/verbatim-outpost-logic/src/{gap_record,verdicts,hold}.rs` and crates/verbatim-core/src/queries.rs (the last removed by 3f9e68ac, 2026-11-17).
  - The choice: retire the follow-up, the study and the dead groups, or take a new baseline.
  - Changed fact: ADR 0003's builds deleted the five files.
- **Q5. Delete the test of `UiaAddEvent`?** The test is `a_core_api_focus_registration_on_one_window_hears_that_applications_focus_alone`: `focus_events_cannot_be_limited_to_one_application.rs` 145-252, with `CORE_HEARD`, `core_heard` and `core_focus_registration`, and its entry in `main.rs`.
  - It exercises `UiaAddEvent`, which no Verbatim code calls (`git grep -n UiaAddEvent -- crates xtask uia-facts` finds only this file).
  - It is the CI reproduction of a claim in `uia-bugs.md`:535: "the core API's `UiaAddEvent` on a window's node ... delivered that window's provider-raised focus events and no other application's". That is one of the two routes the limitation names. The claim would stay, with P5's evidence in git.
  - Asked rather than removed for the same reason as Q1: it removes the reproduction of a recorded fact.

## 5. Cost of one behaviour (one new fact)

The cost comes from fdfb63ae (2026-11-06, "Add the UIA facts' connections, probe and applications programs", 23 files, 2,643 insertions) and from the readme's rules. That commit adds eight programs, not six as A said or seven as B said. One new fact touches:

- a folder with a `Cargo.toml` of 14-15 lines, `src/main.rs`, and a README of fixed sections with a results table;
- a line in the `members` list of `uia-facts/Cargo.toml`;
- a row in the facts table of `uia-facts/readme.md`;
- often a new provider request and a support helper;
- `recorded/` outputs;
- the citing entry in `uia-bugs.md` or `uia.md` (a full GitHub link, plus a link definition in `uia.md`);
- a first run under the four client-setting combinations (`uia-facts/readme.md`:39).

52 of the 62 commits came on one day, most of them review fixes.

The reviewers propose two shared changes, merged here:

- One package with a `[[bin]]` per fact, in the main workspace (Q2). This removes the per-fact manifest, the members edit and the copy check.
- A program only for a fact something relies on (Q1). Such a fact needs none of the README and verdict apparatus.

## 6. Dropped

- A's Q3, which proposed moving `puia threads` to xtask and trimming `experiments/parallel-uia-calls/src` to `threads` ("about 1,800 of 2,381 lines"): contradicted. The cited `stacks.ps1` (debugging.md:156) starts `puia hung`, and `dbg.psm1` (debugging.md:191) starts `puia probe`. So the cited tools use at least three modes. I did not recount the trim size.
- A's claim that `architecture-study.md` in ownership-study is cited "only through a work plan whose builds are all built": contradicted. `docs/design/ownership-milestone-rules.md`:66 cites it, and that document holds the rules for planned milestones.
- A's narrower trigger in F8 (drop model, text and reader-rules): contradicted by the dependency graph. Only the `verbatim-core` line is kept, as M6.
- A's "each of the last eight pushes ran it, about 2 min 15 s a job": not checked, since a read-only merge makes no `gh` call. The commit count replaces it.
- A's "one index of the folders instead of two (uia.md 233-256)": those lines are link definitions for inline citations, not a second index.
- A's estimate of about 1,200 lines of named-only cases inside programs something relies on: not recounted, so it is left out of the target.
- A's settings row at `a-process-that-used-uia-.../readme.md`:32: the settings did change that outcome.
- B's "`verbatim-core`, which enters only because `verbatim-win32` depends on it": contradicted by `crates/verbatim-win32/Cargo.toml`.
- A's count of 30 programs, and both reviewers' program counts for fdfb63ae: corrected above, to 32 programs and eight in that commit.
