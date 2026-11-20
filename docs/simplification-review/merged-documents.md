# The documents: merged and verified record (2026-11-20, main ba89dca7)

Area: books/, docs/, CLAUDE.md, REVIEW.md, the root readme files and .claude/skills/. This is Markdown, so "lines" are document lines, no test lines are saved, and no finding changes speech. Every quotation was re-read at main ba89dca7. Every commit was checked with `git show`. Every "does not exist" was checked against `crates/`, `xtask/` and `uia-facts/`, using the scripts in `merge-documents-work/` (idents.py, nc.py, g.py and cites.py). nc.py strips comments before it looks, so a name that survives only in a code comment counts as dead.

## 1. Size

| Body | Verbatim | NVDA (pinned) | Ratio |
|---|---|---|---|
| Developer and agent Markdown, books/ and docs/ (`git ls-files 'books/*.md' 'docs/*.md'`) | 47,899 lines, 615,330 words. books/dev 26,929; books/extensions 1,121; books/user 171; docs/design 5,661; docs/parity 6,137; docs/nvda 5,264 | `nvda/projectDocs/**/*.md`: 5,846 lines, 48,015 words | 8.2 by lines, 12.8 by words |
| Crate guides (`books/dev/src/reference/crates/*.md`) | 14,091 lines, 140,945 words | none: NVDA keeps this in docstrings | n/a |
| Their "Public API" sections (7 guides) | 1,042 lines: outpost 354, model 238, supervisor 174, core 138, listener 89, replay 39, term-sim 10 | none | n/a |
| Working rules: CLAUDE.md, REVIEW.md, and the port skill with its references | 506 lines, 9,441 words | none | n/a |
| User guide | 171 lines (not written) | `nvda/user_docs/en/userGuide.md` 6,638 lines and `changes.md` 6,556 lines | n/a |
| One behaviour's documents: the dialog read once it is shown, `git diff bc99bc40^ 870bda79` | 13 .md files, +222 −11, beside +4,709 −316 of code | `Dialog.getDialogText`, `nvda/source/NVDAObjects/behaviors.py` 93 to 182, no separate document | n/a |

The reviewers did not disagree on a fact. A gave the ratio by words (12.8), B by lines (8.0). B's line count was rounded to 46,900; the true count is 47,899, which gives 8.2.

What the extra lines buy:

- **The parity ledger and `docs/nvda/`** (11,401 lines). CLAUDE.md requires them as the platform-neutral tier's source ("Implement their behavior from the parity ledger under `docs/parity/` and the prose under `docs/nvda/`, never by translating NVDA source"). They earn their place.
- **Most of the rest is copies of one mechanism**: code doc comment, crate guide, explanation, ADR, built design file and performance ledger (section 5).
- **Built design files kept after their build**: 3,175 lines across the RTCA (ADR 0003, "Readers tell, Core asks") build notes and its three build designs, communication, progress-bars, m42-parity and the cross-process study.

Deletion target, in document lines (no test lines in this area):

- **Remove now: about 1 line.** test-coverage.md L88, the stale test name. The other stale statements outside planned phases need rewriting rather than deleting, so they are Defects: about 30 lines rewritten in the glossary, the ledger and the parity files.
- **If Dickson rules for it: up to about 1,450 lines.**
  - The ledger's "Today:" bullets would name their test instead of copying its count. There are 26 bullets, 170 lines in all. Neither A's "about 600" nor B's "about 1,000" was counted, and neither is settled.
  - The crate guides' "Public API" sections would point at rustdoc: 1,042 lines, less about 7 paragraphs.
  - cross-process-calls-study.md: 233 lines.
  - ADR 0001's live re-read limits: 6 bullets.
  - ADR 0003's message counts: 2 lines.
  - test-coverage.md's lists of test names: 142 table rows. Its line count barely changes, but about 90 KB of its 96 KB goes.
- **Planned phase: about 3,050 lines.**
  - The four RTCA design files: 2,371 lines, less the about 250 lines of build 1's 18 state tables (L419 to about 670), which move rather than go.
  - The three dead sections of verbatim-core.md: 168 lines.
  - progress-bars.md (138) and m42-parity.md (279). The misc fixes' progress-bar defect corrects them.
  - communication.md (454), split once M6.3's audio step is built.
  - m5-build-a.md's dead type names, corrected by M5's design corrections.

## 2. Findings

| id | title | class | found by | file and line | what goes | doc lines saved | test lines saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| D1 | Four built RTCA design files are still the home of rules that 183 code comments cite | Planned phase (RTCA clearing up) | both | docs/design/readers-tell-core-asks{,-build1,-build2,-build3}.md; docs/readme.md:76-85 | the four files, after their cited tables move | about 2,100 net | 0 | no |
| D2 | verbatim-core.md describes three removed mechanisms and names 66 dead identifiers and 5 dead files | Planned phase (RTCA clearing up) | both (counts settled here) | books/dev/src/reference/crates/verbatim-core.md:581-602, 681-746, 998-1077 and 30 other lines | three sections, and dead names elsewhere | 168 + about 30 | 0 | no |
| D3 | The roadmap, the homepage and the docs index say the built phase is unbuilt | Question for Dickson | both | docs/roadmap.md:26, 41, 114-117, 196; books/index.html:39; docs/readme.md:76 | status lines rewritten | about 10 | 0 | no |
| D4 | performance.md copies pinned counts by policy, and two entries are wrong | Defect (the two figures) and Question for Dickson (the policy) | both | books/dev/src/reference/performance.md:27-29, 264-269, 303-305 | the "Today:" counts | up to 170 | 0 | no |
| D5 | The roadmap's confirmed defects name removed code as live | Question for Dickson | both | docs/roadmap.md:265-268, 313-315, 317-339, 474 | stale defect text and pointers | about 25 | 0 | no |
| D6 | Other crate guides name removed code and contradict themselves | Planned phase (RTCA clearing up) | both (counts settled here) | verbatim-app.md (19 names), verbatim-outpost.md:157-158, 215-216, 275, 752, 774, mockapp.md:493-494, verbatim-uia-rops.md:500, verbatim-e2e.md:332 | dead names, the contradictions | about 30 | 0 | no |
| D7 | ADR 0001 states the RTCA phase unbuilt and keeps limits of removed code | Question for Dickson | both | books/dev/src/reference/adr/0001-focus.md:5, 37, 157, 162-168 | L5's sentence; 6 limits | about 8 | 0 | no |
| D8 | The glossary defines four terms by removed mechanisms, against architecture.md | Defect | B only (confirmed) | books/dev/src/reference/glossary.md:16-20, 90-93, 159-163, 233-236 | the mechanism text in 4 entries | about 8 | 0 | no |
| D9 | ownership.md describes removed mechanisms in the present tense | Planned phase (RTCA clearing up), with a question on CLAUDE.md:82 | B only (confirmed) | books/dev/src/explanation/design/ownership.md:46, 57, 82, 92, 97, 133, 138 | stale sentences | about 10 | 0 | no |
| D10 | progress-bars.md and m42-parity.md are built designs that describe the removed bar watch | Planned phase (misc fixes, progress-bar defect) | both | docs/design/progress-bars.md:5, 11; docs/design/m42-parity.md:61, 103 | both files, once split | 417 | 0 | no |
| D11 | communication.md is built but kept, cited by 86 code lines, 82 by section number | Planned phase (M6.3 speech and audio pipeline) | both | docs/design/communication.md:3; docs/readme.md:95-97 | the file once split | about 450 | 0 | no |
| D12 | Documents cite tests that do not exist | Defect (5 names) and Remove now (test-coverage L88) | both | test-coverage.md:88; performance.md:977; parity/focus-events.md:552-553, 1052; parity/input.md:39 | 6 wrong names | 1 (plus 5 corrected) | 0 | no |
| D13 | One behaviour's rule written in 9 places (the dialog read once shown) | Question for Dickson | both | section 5 | copies in guides and ledger | about 60 | 0 | no |
| D14 | Crate guides restate module doc comments | Question for Dickson | both (A proposes, B gives the Public API figure) | e.g. verbatim-listener.md:119-126 against crates/verbatim-listener/src/listener/shown.rs:1-13 | Public API sections, per-function prose | 1,042 + unmeasured | 0 | no |
| D15 | test-coverage.md is a hand-kept second list of test names | Question for Dickson | both | books/dev/src/reference/test-coverage.md; CLAUDE.md:53 | the name lists | about 90 KB | 0 | no |
| D16 | architecture.md and ADR 0001 keep "Amended" lines that the convention forbids | Question for Dickson | B only (confirmed) | architecture.md:29, 38, 43, 47, 60, 70, 83, 99, 128, 144, 156, 161, 179, 270, 610, 1038; adr/0001-focus.md:3, 21, 33, 37 | amended-on clauses | 0 (edits) | 0 | no |
| D17 | ADR 0003 keeps message-kind counts that each behaviour bumps | Question for Dickson | A only (confirmed) | books/dev/src/reference/adr/0003-readers-tell-core-asks.md:144, 148 | the counts | 2 | 0 | no |
| D18 | cross-process-calls-study.md is a study kept in docs/design | Question for Dickson | both | docs/design/cross-process-calls-study.md:3 | the file, once decisions 2 to 5 have a home | 233 | 0 | no |
| D19 | m5-build-a.md designs changes to a type that no longer exists | Planned phase (M5's design corrections) | B only (confirmed) | docs/design/m5-build-a.md:216, 424 | 2 lines corrected | 0 | 0 | no |
| D20 | The control protocol still carries a removed mechanism's count | Question for Dickson (public contract; code area) | B only (confirmed) | crates/verbatim-control/src/protocol.rs:1-2, 31, 309-314; verbatim-app/src/main.rs:2288; verbatim-agent/src/server.rs:1245 | `live_re_reads` | n/a (code) | 0 | no |
| D21 | M4.6's "second measure" has no verdict | Question for Dickson | A only (confirmed) | docs/roadmap.md:518, 530 | a verdict or the entry | 0 | 0 | no |
| D22 | The standing rules: overlaps and contradictions, laid out only | Question for Dickson | both | section 4 | none proposed | 0 | 0 | no |

### D1. Four built RTCA design files are still the home of rules

- **Built.** The three build designs were added by 0cc4d60d (2026-11-16, "Add the approved designs of the three builds of 'Readers tell, Core asks'") and built as follows:
  - build 1: fc5dccbe (2026-11-16, "Add Core's async driver, request table and inbox for the event queue") through 3f9e68ac (2026-11-17, "Remove Core's old focus paths and judge every focus through the event queue");
  - build 2: 6612507a (2026-11-16, "Name each read's details in Core's request, and remove Fetches");
  - build 3: c88df424 and 89c36cc8 (2026-11-16).
- **Their own status lines are now false.** Each line 3 still reads "Nothing in it is built." (`grep -n -o` on all three files). Each also says "ADR 0003 wins and the difference is a defect here", a sentence that exists only because the design and the ADR are two copies.
- **Still edited as living documents.** build1 was last changed by 162ea34a (2026-11-20), and before it by 7431d850, bc61c881 and 6554b510 (2026-11-19). The build notes were last changed by a11ecd70 (2026-11-19).

What cites each file, what covers it now, and how it was built:

| File | Lines | Built by | Code lines citing it (`git grep "design/<name>.md" -- crates xtask uia-facts`) | Of them, naming a numbered table or section | In test files | Explanation page and ADR covering it |
|---|---|---|---|---|---|---|
| readers-tell-core-asks-build1.md | 1,334 (49,513 words, 359 KB) | fc5dccbe..3f9e68ac | 140 in 98 files | 48 (Table 14 ×9, 17 ×5, 5 ×4, 4, 6, 9 and 13 ×3 each); 88 more cite a named section | 33 | focus.md, key-holding.md, focus-validity.md; ADR 0003, ADR 0004. None holds its 18 state tables (L419 to L654); no explanation page names a "Table N" |
| readers-tell-core-asks-build2.md | 85 | 6612507a | 6 in 5 | 0 | 0 | ADR 0003 only |
| readers-tell-core-asks-build3.md | 352 | c88df424, 89c36cc8 | 37 in 26 | 9 (Table 5 ×4, 1 ×2) | 10 | ADR 0003. terminal-model.md was last changed f0902400 (2026-11-09), before the build, and does not describe Core's terminal requests |
| readers-tell-core-asks.md (build notes) | 600 (139 KB) | the phase | 0 | 0 | 0 | ADR 0003. Cited by 11 lines in other .md files (ADR 0003 ×6) |

- **The reviewers' counts.** A's correction (11 citations of the build notes) is right. B's 76 counted ADR 0003's own file name. Both reviewers' 140 for build1 and B's 183 in all are right. B's "24 name a numbered table" undercounts: cites.py, which reads each citation through to its closing parenthesis, finds 57.
- **What the conventions say should have happened.**
  - CLAUDE.md, "Writing conventions": "Once its work is built, a design document becomes two: an explanation of how it works today in `books/dev/src/explanation/design/`, with no alternatives or history, and a numbered decision record in `books/dev/src/reference/adr/` with the problem, the options weighed and why each was taken or rejected, the decision with its date, and its limits; the mechanism's details stay in the crate guide (Dickson, 2026-11-07)."
  - CLAUDE.md:10: "`docs/` — the working documents only agents read, never published: the roadmap, the design records of work not yet built, …"
  - docs/readme.md:23-28: "Once its work is built, a design record under `docs/design/` becomes two published pages: … the record under `docs/design/` is then deleted, because only finished work is published."
  - Against all three, docs/readme.md:76 lists all four files under "Design records of work still being designed or built".
- **The planned phase.** docs/roadmap.md:145-147: "Clearing up: the messages and code nothing uses are deleted, the explanation pages and crate guides describe what is built, and wake-ups and processor time are measured for the phase." The step names the explanation pages and crate guides but not the split and deletion of the four design files. Whether it covers them is the one point for the owner.

### D2. verbatim-core.md: three dead sections and the full list of dead names

**The three sections**, all removed by 3f9e68ac (2026-11-17). `git log --diff-filter=D` shows that commit deleting `key_line.rs`, `live_reread.rs` and `focus_queue.rs` from crates/verbatim-core/src.

- **L581-602, "The focus queue"** (22 lines), opening "- The focus queue (`focus_queue.rs`;".
- **L681-746, "## The ancestry cache"** (66 lines). It describes a cache that roadmap L133-134 says goes: "The marker, the hand-over of an application's first events, the live re-read and the readers' ancestry caches go."
- **L998-1077, "### The key line"** (80 lines), opening "(`books/dev/src/explanation/design/key-holding.md`, "Core"; `src/key_line.rs`.)". The replacement section, "### The caret key on the event queue", follows at L1078.

**The dead names, settled.** A counted 44 and B 47. Both missed the dead names whose parts are common words (`Effect::Fetch`, `Review::Text`) and the names left alive only in code comments. NVDA function names (`_handleUserDesktop`, `handlePossibleDesktopNameChange`, `setFocusObject`, `processFocusNVDAEvent`, `doPreGainFocus`, `initAutoSelectDetection`) are not counted.

First, 44 names with a part that appears nowhere in the code. They match A's 44; B's 47 counted spellings separately.

- L19 `SrState::holds_inputs_back`
- L137 `wedge_decision`
- L544 `LineCut`
- L548 `focus_stamps`
- L552 `Foreground::stale_floor`
- L573 `keep_duplicate`
- L592 `reduce::run_queue`
- L593 `reduce::decide_report`
- L596 `EntryState::Rereading`
- L687 `verbatim_reader_rules::ancestry_cache::title_check`
- L690 and L692 `first_cached_ancestor`
- L695 `plan_focus`
- L698 `focus_rules::needs_live_ancestry`
- L706 `Effect::LiveReread`
- L721 `Input::LiveRereadGone`
- L722 `Input::ListenerReplaced`
- L732 `CoreToListener::LiveReread`
- L733 `CoreToOutpost::LiveReread`
- L757 `CurrentKey::Review`
- L761 `NavigatorSelection`, `NavigatorLine`
- L774 `editing::reported_text`
- L788 `review_text::press_answer`
- L801 `ApplicationNotResponding`
- L804 `window_is_attended`
- L805 `process_is_attended`
- L970 `DesktopNews::NameDue`
- L1007 `FinalReading`
- L1009 `MovingBySentence`
- L1010 `SentenceStep`
- L1019 `TextOp::ReadCaret`, `ReadPurpose::BeforeFieldKey`
- L1020 `ReadPurpose::BeforeTerminalKey`
- L1023 `Effect::KeyNote`
- L1027 `Effect::BoundHeldKey`
- L1028 `Input::HeldKeyDue`, `HELD_READ_BOUND_MS`
- L1029 `KEY_BOUND_MS`
- L1033 `ReadPurpose::Final`
- L1048 `WaitingKey`
- L1051 `later_key_at_us`
- L1276 `editing::selection_changed`
- L1377 `Input::SpeechCancelled` (with `::at_ms` at L1443)
- L1543 `terminal_model::report_status_bar`

Second, 22 more found with comments excluded, each checked by hand.

- L139 `Parked::other`: `enum Parked` has only `Property` and `Text`, at crates/verbatim-outpost/src/outpost/store/mod.rs:225.
- L343 `state_after`: no code holds it.
- L485 `focus_queue::Change`: the module is gone, and survives only in comments at crates/verbatim-core/src/reduce.rs:207, 258 and 413.
- L562 `Foreground::stale`.
- L673 `Input::OutpostEnded`: that name now exists only as `Frame::OutpostEnded` in verbatim-control.
- L718 `SrState::live_re_reads`.
- L759 `announce_navigator`.
- L759 `editing::navigator_text`: the code has `navigator_text_start` and `navigator_text_answered`.
- L762 `Then::Review`: `enum Then` is in verbatim-outpost-logic, with variants `Poll` and `Again`.
- L792 `Query::ReadNode` and `Effect::ReadNode`: no code holds either. Four code comments still cite `Query::ReadNode`: crates/verbatim-ia2/src/acquire.rs:1362, crates/verbatim-model/src/tree.rs:550, crates/verbatim-outpost/src/outpost/mod.rs:387 and crates/verbatim-outpost/src/outpost/read.rs:1754.
- L963 `Input::Desktop`.
- L965 `Effect::ReadFocus`, `FocusPurpose::Again`.
- L1001 `SrState::key_line`: `crate::key_line::Press` survives in a comment at crates/verbatim-core/src/review_text.rs:9.
- L1005 `Watched::Field` and L1006 `Watched::Terminal`: `enum Watched` at crates/verbatim-model/src/text.rs:850 has `Effect`, `Bound`, `BoundUnread` and `EffectUnread`.
- L1022 `Effect::PassHeldKey`.
- L1278 `Busy::heard`.
- L1327 `review_text::run`: the code has `run_next`.
- L1542 `Review::Text`, `Review::StatusBar`.
- L1548 `Review::Mark`.

**Dead files: 5.**
- `focus_queue.rs` (L581);
- `live_reread.rs` (L717);
- `live_reread/tests.rs` (L734);
- `live_reread/generated.rs` (L735);
- `key_line.rs` (L757; `src/key_line.rs` at L1000).

**In total: 66 dead names and 5 dead files.** The code comments listed above are passed on to the code reviewers.

**The rule this breaks.** REVIEW.md:13 names it a failure: "a document that describes a removed mechanism". **The planned phase** is roadmap L145-147, "the explanation pages and crate guides describe what is built".

### D3. The roadmap, the homepage and the docs index say the built phase is unbuilt

- docs/roadmap.md:41 reads "Next (Dickson, 2026-11-13). Nothing here is built."
- The Status list (L26) still puts it first under "Next, in this order": "1. "Readers tell, Core asks", below;".
- L114-117 say "the builds are built in number order and pushed together", in the future tense.
- L196 reads "Next (Dickson, 2026-11-10). Nothing here is built." This is the misc fixes, whose own L247 says "- Built: `crates/verbatim-event-record`".
- books/index.html:39 reads "<li>Next: "Readers tell, Core asks", a simpler arrangement of the readers and Core; then misc fixes …". Roadmap L35-36 says the homepage "carries one line of this status per milestone, and changes with it". The homepage also leaves out the exact replay of a paused entry, which Status item 2 added on 2026-11-19.
- Contradicted by the build commits in D1.
- **Class.** This is a roadmap entry, so it is a Question: is the phase done, or is its clearing up still open?

### D4. performance.md copies the pinned counts, and two entries are wrong

**The two wrong figures:**

| Entry | What the ledger says | What the test pins |
|---|---|---|
| A repeated focus (`performance.md:264-269`) | "read in full, 5 UIA calls (`uia_repeated_focus_costs_exactly`) … They cost mockapp 149 provider calls" | crates/mockapp/tests/call_counts.rs:1830-1842: `calls(3, 0, 0)`, with provider counts 1+14+8+6+5+3 = 37 |
| A focus change, UIA, steady state (`performance.md:303-305`) | "Today: 4 UIA calls, all the listener's … 312 provider calls in all with the outpost's remote walk and 306 with its classic walk" | `uia_focus_changes_cost_exactly_remote` (call_counts.rs:1668): `steady: (connected(7, 0, 0)` at L1687, with provider counts summing to 371. `uia_focus_changes_cost_exactly_classic` (L1737): `connected(11, 0, 0)` at L1757, summing to 353. `connected` adds one connection (L1623-1627) |

- **Who was right.** A gave the steady-state drift and B gave both; both are right. A's line numbers (306-310) have drifted to 303-305, and B's (272-277) to 264-269.
- **When it drifted.** `git log -S'312 provider'` finds 630d6a6e (2026-10-25) as the commit that introduced the figure.
- **How often the two copies change together.** Since 2026-10-20, performance.md has had 59 commits at its books/ path, which it has held only since a7c954e9 (2026-11-04). call_counts.rs has had 89. 17 commits touch both.
- **Dropped counts.** A's "4 of 9 figures appear in no test" and B's "115 figures" are left out. The tests pin per-method counts, not sums, so a literal search cannot settle either.
- **Class.** The two figures are a Defect. The policy that makes them copies is a Question; section 4 sets out every place it is written.

### D5. The roadmap's confirmed defects name removed code as live

- **L265-268, the progress bars.** "the outpost's bar watch, its progress reports and Core's message naming the window in front are still live beside the listener's". These were removed by 34af17a2 (2026-11-16, "Remove the outpost's bar watch, Front and ProgressChanged").
- **L317-339, the code locations.** They are headed "Where each starts in the code, as the audits found it at 04268b78".
  - `BarWatch`, `ProgressChanged`, `CoreToOutpost::Front` and `menu_or_switch_ended` are named and found nowhere in the code.
  - `already_entered` in the outpost's read.rs is named. It is now `already_entered_ancestors` at crates/verbatim-core/src/focus_rules.rs:325.
  - "the query planned behind a marker in `crates/verbatim-outpost-logic/src/intake.rs`" is named, but intake.rs holds no "marker".
  - The menu-opening and caret-key pointers were brought up to date (L330-336), so the section mixes the 04268b78 view with today's.
- **L313-315.** "the wire variant for an MSAA live re-read that never crosses a pipe": no `LiveReread` exists in the code. e039805f (2026-11-17) is "Remove Core's dead focus fields and the live re-read's last uses".
- **L474.** "`verbatim-model`'s comment on `DeliveredFact::key`": `DeliveredFact` exists nowhere.
- **The roadmap's own instruction.** L236-237 says each defect "is checked against it first, since some lie in code it removes". The entries are known to be stale, but the text says "still live".
- **Class.** Question, since it is a roadmap entry.

### D6. Other crate guides name removed code and contradict themselves

- **verbatim-app.md: 19 dead names.** A counted 10 and B 8; both missed the common-word ones.
  - L143 `SrState::holds_inputs_back`
  - L236 `Effect::BoundHeldKey`
  - L239 `QueueInput::HeldKeyDue`
  - L240 `Effect::Withdraw`
  - L248 and L57 `Effect::Activate`
  - L248 `Effect::Fetch`, `Effect::Text`
  - L249-250 `Query::Navigate`, `Query::Activate`, `Query::Text`, `RequestId::reducer`
  - L335 `Input::ApplicationNotResponding`
  - L337 `LiveOutposts::application_of`
  - L340 `QueryOutcome::Abandoned`
  - L464 `Input::OutpostEnded`
  - L472 `requests::reply_input`
  - L500 `reports_for`
  - L505 `FetchResult::Gone`
  - L529 `ShellCommand::CaretKey`

  `verbatim_model::Effect` (crates/verbatim-model/src/event.rs:1229) has no `Fetch`, `Activate` or `Text` variant, and `ShellCommand` (crates/verbatim-app/src/main.rs:877) has no `CaretKey`.
- **verbatim-outpost.md.**
  - L752 `read::held_prefix` and `Store::held_prefix` are dead.
  - L274-275 reads "one being enough behind the intake's marker barrier". The marker was removed by 2886e858 and 15da2c54, and the same words are in a code comment at crates/verbatim-outpost/src/arbitration.rs:55.
  - The guide contradicts itself: L157-158 says "The outpost's walks have no time limit but their program's deadline", L215-216 "the program's deadline bounds it, and a walk still under way at the deadline is abandoned", and L774 "under a deadline". Against them, L368 says "A program has no deadline and the outpost judges no hang", and L490-491 "a program has no deadline". 93b2974c (2026-11-16) is "Remove the outpost's deadlines, moved-on rule and hang record".
  - A's first draft also listed `Judgement::judged_at` and `Verdicts::kept_at`, and A corrected it: both exist, at arbitration.rs:120 and crates/verbatim-reader-rules/src/verdicts.rs:114.
- **The smaller guides.**
  - mockapp.md:493-494 names `listener_forces_uia` and `FocusReader::forces_uia`.
  - verbatim-uia-rops.md:500 names `baseline_moved` and `baseline_selection`.
  - verbatim-e2e.md:332 names `terminal_flood::watch_for_prompt_output`.
  - The other guides name only Windows, wx or NVDA identifiers that the code does not hold, which is correct for them.

### D7. ADR 0001 states the RTCA phase unbuilt and keeps limits of removed code

- **L5:** "The sections amended on 2026-11-13 state decisions of [ADR 0003] (0003-readers-tell-core-asks.md) that are not built yet: until the builds of its phase replace it, the code works as How focus works and the crate guides describe, and the limits accepted below are those of that cod[e]".
- **L157:** "loses its focus at the outpost's deadline". Removed by 93b2974c.
- **L162:** "costs a live re-read".
- **L163:** "waits for its live re-read … bounded by the re-read's moved-on rule and its 13-second read deadline".
- **L166:** "a live re-read of that read is answered gone".
- **L167:** "A report set aside for a live re-read".
- **L168:** "at the cost of a live re-read".
- **L37:** the heading "One route per application, and the marker". The marker is gone.
- The live re-read was removed by 3f9e68ac and e039805f.
- **Class.** Question, since it is an ADR. The fact that has changed: the builds of ADR 0003 are built.

### D8. The glossary defines four terms by removed mechanisms

- **L16-17, "Abandoned worker":** "an outpost worker whose call passed its deadline; the outpost's reducer replaces it". architecture.md:378 says "The readers keep no deadline of their own", and L382-384 says a worker is abandoned only "when Core stops waiting because the foreground moved away from the request's window".
- **L159-160, "Recovery ladder":** "rung 1, per-entry deadlines". architecture.md:373 gives rung 1 as "**Core stops waiting.**"
- **L91-92, "Fact":** "a `ListenerFact` becomes a `DeliveredFact`". Neither type exists.
- **L234-235, "Wedge":** "the heartbeat's `wedge_decision`". It does not exist.
- B only, and confirmed. It is a Defect because each entry needs a rewrite, not a deletion.

### D9. ownership.md describes removed mechanisms in the present tense

- **Present tense, about removed code:**
  - L97: "`Merge` is a trait with a key, a declared barrier and `may_merge`". There is no `trait Merge` and no `may_merge`.
  - L133: "the reducer holds the hook's whole stream from install until `ForwardingEnded`". `ForwardingEnded` does not exist, and roadmap L133 says the hand-over goes.
  - L138: "the hook's stream waits for `ForwardingEnded`".
  - L82: "waits in Core's held-key line". The key line was removed by 3f9e68ac.
- **History, not present tense:** L46, L57 and L92 name `CoreFocus` and `HeldElements`. They narrate the problem in the past.
- **The open question.** CLAUDE.md:82 makes this page history on purpose: "the ownership refactor (`books/dev/src/explanation/design/ownership.md`) records the history behind them". The writing convention says an explanation page has "no alternatives or history". That is a question about CLAUDE.md. The stale present-tense lines fall under the clearing-up step ("the explanation pages … describe what is built").
- B only, and confirmed.

### D10. progress-bars.md and m42-parity.md

- **progress-bars.md.** Built by dd625bda (2026-11-09, "Report every progress bar through the listener") and finished by 34af17a2.
  - L5 reads "Today a progress bar that is not the focus is heard only through the foreground application's outpost", which is no longer true.
  - L11 is history: "**This is a revert of recent work.** It removes the foreground-window subscription built today … `BarWatch`, `CoreToOutpost::Front`".
  - Cited by 13 code lines in 10 files, none naming a numbered section. Its decision is in ADR 0003, "Progress bars (Dickson, 2026-11-14)" (L129). It has no explanation page.
- **m42-parity.md.** Built by 81003396 and 550452aa (2026-11-09); roadmap L21 says M4.2 is built.
  - L103 still designs `enum BarWatch {`, and L61 names `DeliveredFact`.
  - Cited by 3 code lines. It has no explanation page and no ADR.
- **The planned phase.** Roadmap L271-274 already plans the correction: "`docs/design/progress-bars.md`, `docs/design/m42-parity.md`, the M4.2 and M4.3 entries and the performance ledger's progress entries are corrected".

### D11. communication.md

- **Status line (L3):** "Status: built, except the audio step (section 11)". It has carried this status since 1cddafdd (2026-10-28).
- **Cited by** 86 code lines in 42 files. 82 of them name a numbered section: 6.7 ×15, 10.1 ×9, 6.1 ×9, and 4.2, 4.3, 4.5 and 7.3 ×5 each.
- **Covered by** no explanation page and no ADR.
- **Why it is still kept.** docs/readme.md:95-97 gives the reason: "published once its audio step is built". So it is held for M6.3, speech and audio pipeline.
- **The open point.** The convention's split is not done for the parts already built.

### D12. Documents cite tests that do not exist

- **test-coverage.md:88** names `an_unheld_key_says_nothing_asks_nothing_and_forgets_the_typed_word`. 7431d850 renamed the test to `…_keeps_the_typed_word` (crates/verbatim-core/src/keys/caret_key_tests.rs:766) and updated L93 only. Remove now.
- **performance.md:977** names `a_selection_is_read_in_full_only_inside_a_focused_list`. The same name is at build1 L1269.
- **docs/parity/focus-events.md:552-553** names `a_foreground_candidate_not_in_front_is_refused` and `a_foreground_candidate_of_the_last_focuss_window_or_an_ancestors_is_refused`.
- **docs/parity/focus-events.md:1052** names `a_uia_focus_whose_walk_found_no_window_does_not_count`.
- **docs/parity/input.md:39** names `a_review_command_pressed_while_a_caret_key_is_held_runs_after_its_read_pass_and_watch`.
- **The check.** Each name has zero hits in crates/ and xtask/ (`git grep -c -w`).
- **Class.** The five are Defects, because each document cites the test as evidence and the correct test has to be found.
- **Dropped.** B's `tray_list_and_task_bar` is a planned name, marked "([roadmap] (…), M5)" at test-coverage.md:12, so it is not stale.

### D13 to D21

- **D13.** See section 5.
- **D14. Crate guides restate module doc comments.** verbatim-listener.md:119-126 restates crates/verbatim-listener/src/listener/shown.rs:1-13 in nearly the same words: "over a plain channel and a nudge", "with no lock on the hook path". The 7 "Public API" sections are 1,042 lines; B was right. A's "about 4,500 lines" is an estimate and was not counted. A's claim that verbatim-model.md repeats a doc comment "twice" is half right. verbatim-model.md:419-421 matches crates/verbatim-model/src/request.rs:1613-1616 nearly word for word, once, and the rule is paraphrased again at crates/verbatim-outpost-logic/src/lib.rs:126 and crates/verbatim-outpost/src/outpost/worker.rs:208. It is a Question because CLAUDE.md:11 defines the guides' remit: "what each does, its public API, and how the intricate parts work."
- **D15. test-coverage.md is a hand-kept list of test names.** 191 lines, 96 KB, 142 table rows. CLAUDE.md:53 requires it: "When adding or removing a gesture, a user-facing feature or a test, update its row".
- **D16. "Amended" lines.** architecture.md has 16 lines with "Amended" (`grep -c`), for example L128: "Decided 2026-07-16. Amended 2026-10-20 (Dickson), 2026-11-02 …". ADR 0001:3 reads "amended 2026-10-29, 2026-11-02, 2026-11-07, 2026-11-08 and 2026-11-13". The convention says: "A decision of record states the decision as it stands, dated when it was made, with no amended-on line."
- **D17. Message-kind counts in ADR 0003.** L144 reads "Reader to Core (13)" and L148 "That is 27 kinds where there are 42". bc61c881 changed them from (12) and 26.
- **D18. The cross-process study.** Its L3 reads "Status: study of 2026-10-29; … Decisions 1, 4 and 5 are built …, decision 1 recorded in `books/dev/src/reference/adr/0001-focus.md`". Decisions 2 to 5 have no ADR. It is cited by 8 .rs lines and one uia-facts readme. CLAUDE.md:67 reads: "Studies, reviews, design drafts, run logs and measurement records are kept in the session's scratchpad, not the repository."
- **D19. m5-build-a.md.** L216 reads "`DeliveredFact::UiaNotification` gains `accepted`", and L424 repeats it. The roadmap plans "M5's design corrections" at L149 and L238.
- **D20. The control protocol's count.** crates/verbatim-control/src/protocol.rs:314 declares `pub live_re_reads: u64`, which is always filled with `0`. The control protocol is a public contract, one of the areas Dickson owns. This belongs to the code reviewers.
- **D21. M4.6's second measure.** L518 reads "The second measure of the generated tests' judgement, set by Dickson on 2026-11-02, is taken over the next three builds that change a covered owner". L530 gates a later item on it ("Built only if the second measure above is met"). No document records the measure (`git grep -i "second measure"`).

## 3. Test findings

None in this area: it holds no tests. test-coverage.md (D15) is an index of tests, and its upkeep is a question for Dickson. Five documents cite tests that do not exist (D12).

## 4. Questions for Dickson

### The standing rules, laid out (no recommendation)

Rules are numbered by order in CLAUDE.md L47 to L78. "Incident" quotes the clause naming what went wrong. "Reason only" means the rule gives a reason but names no incident, which CLAUDE.md:76 requires of a new rule ("names its incident in one clause").

The reviewers differed only on R20 and R17. B listed R20 as naming no incident; A did not list it. R20's "because leftovers break the next run and fill the disk" is a reason, not an incident. R17's "because restated briefs and long reports cost tokens" is a reason too, and neither reviewer listed it. So the rules naming no incident are R1, R3, R4, R7, R10, R13, R17, R18, R19, R20, R21, R24, R26, R30 and R32.

| Rule | Line | First clause | Incident | Overlaps | Contradictions (each side quoted) |
|---|---|---|---|---|---|
| R1 | 47 | "Fix root causes, never adding retries, sleeps, skipped cleanup, or self-termination to hide a race or failure" | none (reason only) | REVIEW.md:12 "a race hidden by a retry, a sleep or a bound added without evidence that the proper fix is not enough"; port SKILL.md:107 "Fix root causes rather than adding a mechanism for each failure, because the outpost code once became a pile of independent mechanisms" (the skill names an incident, the rule does not) | none |
| R2 | 48 | "Verify every claim about NVDA's behavior from the pinned source under `nvda/` or a live capture" | "because the November rebuild carried three wrong claims about NVDA from its designs into code (Dickson, 2026-11-17)"; "Because 52 fixes in one month came from an application's behaviour learned after building." | R3; R12 (adversarial review); REVIEW.md:9; SKILL.md:93 ("for anything spoken, a live NVDA capture … taken before the expectation is written") | none |
| R3 | 49 | "A decision put to Dickson cites the pinned NVDA source for every claim about NVDA" | none | R2 | none |
| R4 | 50 | "The NVDA submodule pin must equal the installed NVDA build used for captures" | none (reason: "because expectations come from captures and ported code from the pinned source") | none | none |
| R5 | 51 | "The third fix commit in one mechanism after that mechanism's implementation review stops its build" | "because the rule as a judgement did not fire for the gap record, which took seven more fixes (Dickson, 2026-11-10)" | REVIEW.md:44 "The third fix commit in one mechanism after its implementation review ends the loop and goes to Dickson, as `CLAUDE.md` says."; R14 (gaps a generated test finds count as fix rounds) | none |
| R6 | 52 | "Follow `books/dev/src/reference/testing.md`" | "because Core's tests fed the foreground report before the foreground event, … and every one passed while new windows went unspoken (Dickson, 2026-11-17)" | REVIEW.md:10; SKILL.md:91 "Follow `books/dev/src/reference/testing.md` exactly" | none |
| R7 | 53 | "When adding or removing a gesture, a user-facing feature or a test, update its row in `books/dev/src/reference/test-coverage.md`." | none | none | none |
| R8 | 54 | "Run locally only the one to three end-to-end scenarios whose code path a change touches, through `cargo xtask e2e`, never a whole family by prefix or the full suite" | "because a local batch of 42 scenarios on 2026-11-19 took an hour repeating what GitHub runs anyway" | R11, R28 (who runs scenarios, when); R9 (flaky-test matching) | (a) **CLAUDE.md:125, Commands:** "a family by prefix (all `windows_terminal_` and `conhost_` scenarios) only for a change every scenario of it sees. Keep full runs for wide-ranging changes"; **CLAUDE.md:123** gives the full-suite command. (b) R8 says "through `cargo xtask e2e`"; **CLAUDE.md:123** says "run one scenario with `cargo test -p verbatim-e2e --test e2e -- --ignored --exact <name> --test-threads=1`". (c) **SKILL.md:113:** "Do not run the whole end-to-end suite by default, because it takes a long time; follow `CLAUDE.md` and `books/dev/src/how-to/e2e.md` for when a full run is justified" (it allows a justified full run) |
| R9 | 55 | "Before calling an end-to-end failure interference, a flake or environmental, read its artifacts and name the process or code responsible" | "because on 2026-10-30 two failures called interference were a unit test's stray console window and a real defect" | R8's last sentence (match a failure to `docs/flaky-tests.md`) | none |
| R10 | 56 | "Work on main, with no pull requests or feature branches unless asked" | none (reason: "so that every session and subagent builds on the same history") | none | none |
| R11 | 57 | "Push after each finished build, once `cargo xtask ci` and its related scenarios pass locally" | "because on 2026-11-19 five pushes went out over a unit test failing on GitHub that no one read" | R8, R17, R28 | **REVIEW.md:37:** "Builders commit as they go; nothing is pushed before the step's review." R11's condition for a push names ci and scenarios but not the review |
| R12 | 58 | "Confirm a design before building on it, including all edge cases, where each part runs and the exact speech expected" | "because the November rebuild silently lost the formatting reads and two ruled differences (Dickson, 2026-11-17)" | R2 (adversarial review); R13; R22 ("while a design study or decision is open, build nothing that depends on it", against R22's "nothing in that mechanism is built on a provisional answer"); SKILL.md:85 | The review's phase: **REVIEW.md:37:** "A step's design is reviewed before implementation (the `port` skill's phase 5)". Against it, **SKILL.md:28:** "the design before implementation (phase 4)", and SKILL.md:85, in phase 4: "Have a subagent review the design adversarially"; the skill's phase 5 (L87) is "Establish the judge before implementation" |
| R13 | 59 | "A design that adds or changes state includes a table of its states and the rules that read them" | none | R12 (the review checks every cell); R16 and R22 (questions in one batch); R25 ("covered by tests" of the state tables) | **CLAUDE.md:29:** "every other decision is Claude's"; **R16 (L62):** "decide the rest, matching NVDA where NVDA has an answer". Against them, R13: "every cell the ADR does not decide goes to Dickson in one batch" |
| R14 | 60 | "A generated test is written only where a specific ordering across a hand-off is at risk (Dickson, 2026-11-10)" | "because generated tests of whole lifecycles found too little for their size"; "Because models written by their builders were bent to the code." | R5 | none |
| R15 | 61 | "Review each completed step, not each commit, as `REVIEW.md` says" | "because on 2026-10-21 reviewing every commit with no stopping rule produced contradictory findings and sixteen rounds on one area" | REVIEW.md:35-37 (the whole of "The unit of review"); SKILL.md:123 "as one diff against the commit it started from, not slice by slice" | none |
| R16 | 62 | "Plan multi-step work as a few large builds" | "because on 2026-10-30 a cycle per step … took more than twelve hours for half of one design" | R13, R22 (questions in one batch); REVIEW.md:43 ("except a defect in the mechanism the step builds, which is fixed in the step rather than deferred") | (a) R13, above. (b) **SKILL.md:60:** "For a scope with several steps, do the smallest one end to end first, as a shakedown"; R16: "a few large builds … rather than a full cycle per small step" |
| R17 | 63 | "Builders run targeted tests while working and `cargo xtask ci` once at the end" | none (reason: "because restated briefs and long reports cost tokens on every hand-off") | R28 ("Builder subagents end at unit tests and `cargo xtask ci`"); SKILL.md:28 ("Research subagents that only read and summarize can be Sonnet; implementation and review subagents are Opus") | **R26 (L72):** "Every brief … quotes the ADR decisions and limits it must honour"; R17: "keep briefs short by pointing at documents" |
| R18 | 64 | "Use UIA remote operations wherever a sequence of UIA calls would benefit" | none (reason only) | port references/review.md:33-34 | none |
| R19 | 65 | "Judge every change on wake-ups, calls into other processes, locks, measured processor time and work that grows with the application" | none (reason only; dated "(Dickson, 2026-11-04)") | REVIEW.md:11; performance.md:11-17; SKILL.md:83, 95; port references/review.md:28-45 | See "The ledger policy" below: R19's "a command's own calls are pinned by a call-count test, not written in the ledger", against SKILL.md:95 "An operation that makes calls into an application gets its entry in the ledger" |
| R20 | 66 | "Clean up: stop the processes you started, remove merged worktrees, delete large traces once read, and release held keys" | none (reason: "because leftovers break the next run and fill the disk") | none | none |
| R21 | 67 | "Studies, reviews, design drafts, run logs and measurement records are kept in the session's scratchpad, not the repository." | none | Writing conventions ("evidence lives in `experiments/`") | **SKILL.md:85:** "For a nontrivial redesign, update or create the design document before implementation"; against it, R21: "a new file in `experiments/` or `docs/design/` is proposed to Dickson before it is committed" (the skill has no proposal step). **SKILL.md:39** describes `docs/design/` as "detailed Verbatim designs", with no deletion once built (D1) |
| R22 | 68 | "Do not stop while work in scope remains" | "because on 2026-11-09 a build continued on provisional rulings and one question was decided six times (Dickson, 2026-11-10)" | R12, R13, R16, R27; REVIEW.md:54 ("A decision in his areas is never taken pending: its build waits for him") | Within CLAUDE.md, R22 "Do not stop" against R27 "stops and reports at once" and R5 "build nothing more in it until he answers". Each is scoped, but nowhere is the scoping said once |
| R23 | 69 | "A proposal, brief or design that would change a decision of record … quotes that decision and names the fact that has changed since" | "because proposals that ignored recorded decisions and planned work each cost a review and a reversal (Dickson, 2026-11-10)" | R26 | none |
| R24 | 70 | "Keep ported NVDA code in the GPL-tier crates" | none (reason: licensing) | "NVDA provenance" (CLAUDE.md:99-101); SKILL.md:77, 109 | none |
| R25 | 71 | "One build that adds or changes state is in flight at a time" | "because more exhausts the machine's disk and memory, and state changes built side by side were corrected against each other (Dickson, 2026-11-10)" | R13 | none |
| R26 | 72 | "Every brief for a design, model or build quotes the ADR decisions and limits it must honour" | none | R23; SKILL.md:30 ("Write every brief from `references/brief.md`, and point it at the 'Standing rules' section") | R17, above |
| R27 | 73 | "A subagent that finds its brief's design cannot work … stops and reports at once" | "because improvised choices committed as 'pending' each cost a review and often a reversal" | R22 | **REVIEW.md:54:** "A decision outside Dickson's areas that the parent session takes while he is away is recorded as '(Claude, <date>, pending Dickson's acceptance)'"; R27: "the parent never commits such a choice for later acceptance". The label is in use 5 times (docs/flaky-tests.md:60, 73, 161, 169; docs/parity/focus-events.md:829), and adr/readme.md:5 describes it as practice |
| R28 | 74 | "Builder subagents end at unit tests and `cargo xtask ci`; the parent runs the end-to-end scenarios" | "because agents that waited on the desktop with a large context spent hours polling and fought over it" | R17, R8, R11 | none |
| R29 | 75 | "Measure wake-ups and processor time at the end of each milestone, and inside a build only when it adds a thread, a timer or a message on the event or speech path" | "because three builds' measurements showed no change beyond the spread between runs (Dickson, 2026-11-10)" | roadmap L147 (the clearing-up measurement) | **SKILL.md:117:** "For a substantial change or a new feature on the event or speech path, measure before closing: the call counts against the design's costing, and latency, wake-ups, and processor time"; **SKILL.md:95:** "A change on the event or speech path is also measured" |
| R30 | 76 | "A new standing rule replaces or merges an existing one and names its incident in one clause." | none (reason: "because each incident added a rule as each failure added a mechanism") | none | 15 of the 32 rules name no incident (above) |
| R31 | 77 | "Estimate time from measured durations" | "because a guessed hour was twenty minutes" | none | none |
| R32 | 78 | "When adding, removing or changing the length of an end-to-end scenario, run `cargo xtask e2e-durations` once its GitHub run finishes." | none | none | none |

The reviewers also raised two items outside the rules themselves. Both are confirmed.

- **REVIEW.md:27** says of Pre-existing findings: "Recorded as a follow-up in `docs/roadmap.md`". That sits against the owner's memory note, which says roadmap additions need approval.
- **CLAUDE.md:10** describes `docs/` as holding "the design records of work not yet built", against the six built designs kept there (D1, D10, D11).

### The ledger policy (performance.md copying counts the tests pin)

Every place a change to this policy would touch:

- **performance.md:14-15:** "A command's own calls are pinned by a call-count test (`crates/mockapp/tests/call_counts.rs`), not written here."
- **performance.md:27-29:** "An exact count is a ratchet: a change that adds a call fails CI, and a change that removes one lowers the number in the test and here in the same commit." It conflicts with L14-15 for operations that are not commands.
- **CLAUDE.md:65:** "Record a cost in `books/dev/src/reference/performance.md` only when it is on the event or speech path or was measured to be slow; a command's own calls are pinned by a call-count test, not written in the ledger (Dickson, 2026-11-04)."
- **CLAUDE.md:63:** "use Sonnet for mechanical work (failure triage, nits commits, ledger updates, capture runs)".
- **REVIEW.md:11:** "a call into another process, wake-up, lock or timer added on the event or speech path that the ledger (`books/dev/src/reference/performance.md`) does not state; … the ledger records only substantially slow things such as accessibility calls".
- **REVIEW.md:37:** "with its tests, documents and ledger entries".
- **REVIEW.md:50:** "A reviewer reads the evidence before the code: … the ledger's counts."
- **.claude/skills/port/SKILL.md:83:** "Write the expected counts into the design as the ledger entry the implementation must meet."
- **SKILL.md:95:** "An operation that makes calls into an application gets its entry in the ledger (`books/dev/src/reference/performance.md`, "The ledger") and an exact call-count test in `crates/mockapp/tests/call_counts.rs`, in the same commit".
- **.claude/skills/port/references/review.md:28-29:** "And for cost, which the design's costing and the ledger (`books/dev/src/reference/performance.md`) fix in advance:"
- **references/review.md:44-45:** "a ledger entry or call-count test left unchanged by a change that altered the operation's calls."

What this costs today: two entries are wrong (D4).

### The other questions

1. **D3 and D5.** Is "Readers tell, Core asks" done, with only clearing up open? Should the misc fixes' confirmed defects be rewritten against today's code (roadmap L41, L196, L265-268, L313-339, L474; books/index.html:39)?
2. **D1.** Does the clearing-up step ("the explanation pages and crate guides describe what is built", roadmap L145-147) include splitting and deleting the four RTCA design files? It would move build 1's 18 state tables and build 3's tables into module docs or explanation pages, and repoint 183 code comments.
3. **D7.** Rewrite ADR 0001's L5 and its six limits about the live re-read and the outpost's deadline (L157, L162-168)? The fact that has changed: 3f9e68ac, e039805f and 93b2974c removed them.
4. **D4.** The ledger policy, set out above.
5. **D13.** Where the dialog read once shown, and rules like it, are written (section 5).
6. **D14.** Narrow the crate guides' remit (CLAUDE.md:11, "its public API") to how the parts fit, pointing at rustdoc? The Public API sections alone are 1,042 lines.
7. **D15.** Generate test-coverage.md's test names, and keep only the feature rows and gaps (CLAUDE.md:53)?
8. **D16.** Remove the "Amended" lines from architecture.md (16) and ADR 0001, as the writing convention says?
9. **D17.** Drop ADR 0003's message-kind counts (L144, L148)?
10. **D18.** Where do the cross-process study's decisions 2 to 5 live, so the study can go?
11. **D9.** Is ownership.md history (CLAUDE.md:82) or an explanation page (the writing convention)?
12. **D20.** Remove `StatusInfo::live_re_reads` from the control protocol? It is a public contract.
13. **D21.** M4.6's second measure: a verdict, or remove the entry and the item gated on it (roadmap L518, L530)?

## 5. Cost of one behaviour

The three traces, merged and recounted with `git show --numstat` and `git diff --numstat`:

| Behaviour | Commits | .md files | Lines in .md | Beside code |
|---|---|---|---|---|
| Dialog read once shown | bc99bc40..870bda79 (documents in c1ade8dc, 4a35e930, 2b2e0c12) | 13 distinct (24 changes over the three documents commits) | net +222 −11; per commit +237 −28 (A and B, each right by its method) | +4,709 −316 |
| Holding Enter, NVDA's typed-word rule | 7431d850 | 11 | +161 −61 | +1,133 −168 |
| Window destroy said as "window invisible" | bc61c881, 6554b510 | 16 distinct (both reviewers said 15) | +157 −51 | +1,618 −332 |

The window destroy's 16 files:

- the ADR 0003 counts (D17);
- build1's table rows: a built design edited (D1);
- performance.md (+28);
- test-coverage.md;
- two parity files and docs/nvda/speech.md;
- the mockapp how-to;
- eight crate guides.

In each trace, what holds something no other place holds:

- the parity entry, for NVDA's behaviour and the ruled differences;
- the explanation page, for the behaviour;
- the ADR, when a decision changes.

The crate guides, the built design, the ledger and the test-coverage row restate the code or the tests.

### The dialog read once shown: every place its ending rule is written

1. **crates/verbatim-core/src/focus/shown.rs:11-17**, the module doc and the owner's statement: "The wait ends early when the user is elsewhere, a condition on the queue rather than an event: … A key pressed while the dialog is hidden ends the wait too, but does not skip the dialog: the focus is read at once, hidden, and committed before the key is handled".
2. **crates/verbatim-core/src/focus/shown.rs:107-127**, the doc of `ending`: "It is the user being elsewhere when any of these holds: - the foreground record names a window other than `dialog` …, owner and owned windows included; - a menu or popup start waits for a window whose top-level window is not `dialog`, accepted, from the application holding attention; - an MSAA or UI Automation focus, a UI Automation menu opening among them … waits for a window whose top-level window is not `dialog`, that the one definition's rows needing no read accept … Otherwise any key pressed since the front joined, a modifier alone among them, ends it as [`Ending::Key`]".
3. **books/dev/src/explanation/design/focus.md:106**: "The wait ends early when the user is elsewhere, a condition on the queue that holds whenever the input making it true arrived, so it stops a wait from beginning as well as ending one: the foreground record naming any window other than the dialog, owner and owned windows included; a menu or popup start outside the dialog, accepted, from the application holding attention; or a focus outside the dialog, a UI Automation menu opening among them …". **L108** then reads: "A key pressed while the dialog is hidden, any key whose press cuts speech, a modifier alone included, ends the wait too, but does not skip the dialog … When a key and the user being elsewhere both hold, the user being elsewhere wins." **L110** adds the due time: "A dialog never shown, with no key and no other ending, holds the queue until the due time, 13 seconds, then ends unspoken."
4. **books/dev/src/reference/crates/verbatim-core.md:207-219**: "The ending condition (`shown::ending`) is a pure function of the dialog's window, the foreground record and the entries waiting behind the front: the user is elsewhere when the record names another window, a menu or popup start outside the dialog from the application holding attention waits, or a focus outside it, a UI Automation menu opening among them, that the one definition's rows needing no read accept waits; otherwise a key pressed since the front joined ends it as a key, by the speech fence's one rule (`Arrivals::fence` above the front's fence), a modifier alone included".
5. **docs/parity/focus-context.md:166-173**: "- the user elsewhere before the show (the foreground, a menu or a focus outside the dialog) says nothing for the hidden dialog. … - a key pressed before the show reads the dialog at once, without its text, as NVDA would read it then; - a dialog never shown, with no key and no other ending, holds the queue for the 13 s due time and ends unspoken, where NVDA speaks its focus."
6. **books/dev/src/reference/adr/0003-readers-tell-core-asks.md:225**, a limit: "so does a hidden dialog that is never shown, with no key pressed and no foreground, menu or focus elsewhere (Dickson, 2026-11-19)."
7. **books/dev/src/reference/crates/verbatim-model.md:305-306**: "`CancelReason::LeftHiddenDialog` cancels it when the user is elsewhere (Dickson, 2026-11-19)."
8. **crates/verbatim-model/src/request.rs:1152-1154**: "/// A wait for a hidden dialog's show ended because the user is /// elsewhere: the foreground, a menu or a focus outside the dialog."
9. **books/dev/src/reference/test-coverage.md:122**: "the user elsewhere (the foreground, a menu or a focus outside the dialog) ends the wait with nothing said for it, a key reads it at once, hidden, before the key is handled".

A and B counted 5. The count above is 9: 3 in code, 6 in documents. performance.md:592-635 states the mechanism's costs but not the ending rule. verbatim-listener.md:119-126 restates the listener's half (shown.rs in the listener), not the ending.

### The shared mechanisms the reviewers propose, merged

Each one depends on a question in section 4.

- **Delete the built design files once their cited tables live in the types' module docs or the explanation page** (D1; question 2). Every behaviour touches build1 today.
- **Ledger entries name their test and give the bound and its reason, not the count** (D4; the ledger policy). This removes the ledger edit from each behaviour, and the drift.
- **Crate guides keep how the parts fit and point at module docs for mechanisms** (D14; question 6). This removes the 5 to 8 guide edits per behaviour.
- **Generate test-coverage.md's test names** (D15; question 7).
- **One owner per rule.** The explanation page holds the behaviour, the module doc the mechanism, the parity entry NVDA's behaviour and the ruled differences, and the ADR the decision and its limit. The others point (D13; question 5).

With all five, the reviewers agree a behaviour touches 2 or 3 documents (the parity entry, the explanation page, the ADR when a decision changes) in place of 11 to 16.

## 6. Dropped

- **B: docs/flaky-tests.md L66, L145 and L148 as stale.** L66 names the scenario's former name on purpose ("when they were `windows_terminal_command_marks`"). L145 cites retired scenarios as the evidence of an open entry ("Cause: not found. Fix: none yet") and says "since retired".
- **B: `tray_list_and_task_bar` in test-coverage.md as stale.** It is marked planned for M5 at L12.
- **B: ownership-milestone-rules.md as superseded.** docs/readme.md lists it as rules for planned milestones, which `docs/design/` holds by CLAUDE.md:10. It is not stale.
- **A: "4 of the 9 distinct provider-call figures appear in no test", and B: "115 figures of the form N calls".** The tests pin per-method counts, not sums, so a literal search cannot settle either.
- **A: "the crate-guide restatement is about 4,500 lines", and B: "done roadmap entries, about −250".** Estimates that were not counted. Only the 1,042 Public API lines are settled.
- **A: verbatim-outpost.md's `Judgement::judged_at` and `Verdicts::kept_at` as dead.** A corrected this: both exist.
- **A: verbatim-model.md's sentence "twice" in doc comments.** It is once nearly word for word (request.rs:1613-1616), plus two paraphrases. Kept inside D14 with that correction.
- **B: "rule 8 against the roadmap's M4 preamble" and "rule 29 against a memory note".** Outside the comparisons asked for (other rules, Commands, REVIEW.md, the port skill). The memory notes are not repository documents.
