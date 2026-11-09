# Verbatim Roadmap

Milestones are scoped by _risk retired_, not calendar time. Each has explicit
exit criteria so progress is testable. Ongoing tracks (localization,
observability, latency budgets) start early and run through every milestone
rather than being milestones themselves.

## Status

M0 (foundations), M1 (self-voicing prototype), M2 (test harness and
VM), and M3 (desktop usability core) are complete, and what each
delivered is in [Completed milestones](roadmap-done.md). M4 is in
progress; its completed parts, M4.1 (stability) among them, are listed
there too, and M4.1's open follow-ups are under M4, "Follow-ups from
M4.1". M4.5 (logic foundations) and M4.6 (generated tests) are complete. The process and thread
layout was weighed again and kept: an outpost process per application
and a separate focus listener (D9, D13; Dickson, 2026-11-02). The focus
pipeline rework's second build is built (2026-11-02). The owner endings build is
built (2026-11-03). The UIA facts build is built (2026-11-06). The slow
applications build is built (2026-11-07). Next, in this order (Dickson,
2026-11-05):

1. the focus pipeline rework's third build, with Core's focus queue
   redesign;
2. the state models below, "State models" (Dickson, 2026-11-08);
3. the caret and selection rework, which ends M4;
4. M4.2;
5. M5;
6. M6;
7. M4.3;
8. M4.8 (Guardian) and M4.7 (Containment);
9. M4.9 (Lints and unsafe);
10. M6.2, with the rest after.

The published site's homepage, `books/index.html`, carries one line of
this status per milestone, and changes with it.

## M4 — Text, editing, and terminals

- TextPattern support in the model; caret tracking, typed-character echo,
  word/line/character navigation; say-all with index-mark continuation.
- Text runs carry formatting attributes as utterance spans (spelling and
  grammar markers, and font/color where exposed), so M11's
  formatting-change sounds have data to act on.
- A minimal built-in character-description table (punctuation and symbol
  names): character navigation must say "comma" on a comma even though the
  configurable dictionary system waits until M8.2.
- Remote-ops integration (ancestor fetch on focus; terminal text ranges).
  The ARM64 remote-ops verification (R3) is not part of M4: it was
  deferred on 2026-10-06, and M4 is built and verified on x64 only.
- Windows Terminal: diff-based output announcement with flood policy; the
  "cat a huge file" scenario is an E2E latency test.

Exit: Notepad editing is solid, and the focus pipeline and caret and
selection reworks are built.

M4 ends when the two groups below land: the focus pipeline rework, then
the caret and selection rework, each in a session of its own (Dickson,
2026-10-25). Within a group the items are in the order they are done
(Dickson, 2026-10-10 and 2026-10-13). The rest of the open work found
during M4 is scheduled in the milestones that follow, and a Deferred list
closes the section with items that are not M4's. Each item is one
change, checked with `cargo xtask ci` and its related scenarios ten
times, with the full suite where every scenario is affected. A change in
what is heard goes to Dickson first; while he is away, it follows NVDA
where NVDA is clear, recorded as provisional.

### Follow-ups from the focus and ownership builds

Follow-ups from the one judge build:

- Entering a top-level window without a system menu through an ancestor
  walk does not say the window, where NVDA's objectInForeground does.
- Log the published message's calls on the outpost worker's `handled`
  line, so the uncounted UIA costs in the performance ledger can be
  measured.

Follow-ups from the focus pipeline rework:

- A focus group's older entries are lost with an abandoned worker.
- The text anchors' locks are held across text calls.
- NVDA's speech for the edge cases of MSAA object equality waits for one
  capture session against mockapp's `object_equality.json` fixture.

More follow-ups:

- The end-to-end harness's `set_foreground` minimizes a folder window left
  behind (`crates/verbatim-agent/src/desktop.rs`, lines 262 to 300), which
  leaves Explorer's list without focus; open folder windows without
  minimizing them.
- Explorer's DirectUIHWND window element was once spoken as "pane" after
  its item, a late UIA focus reported as a new focus. Ten runs of
  `explorer_folder_window` with the listener's and the outpost's debug
  logging on 2026-11-08 spoke the expected words every time, and no focus
  on a pane reached Core; every pane in the reports was an ancestor of a
  list item. If it is heard again, check with an NVDA capture first.
- A new console host's window can stay hidden and not responding while
  the outpost's first call into it passes its deadline (3 of 20 measure
  runs on 2026-10-31; [the flaky tests](flaky-tests.md), "A new console
  host not responding at its outpost's first call"). The kept artifacts
  show the console host hung on its own. The harness now records the wait
  chain of each not-responding window's thread and of the launched
  process's threads when its window wait fails (`wait-chains.txt`), and
  the outpost's abandoned-worker warning names the entry its worker had in
  hand; read the next failure's chains to find what holds the window's
  thread.
- A new application's window sometimes does not appear within the
  harness's 30 seconds: Notepad five times locally and a mockapp Windows
  Forms window once on GitHub's runner, on 2026-11-01 ([the flaky
  tests](flaky-tests.md), "A new application's window not shown in
  time"). Find, from a failed run's kept artifacts and its
  `wait-chains.txt`, whether an outpost's first call holds the new window,
  as the console host entry above may.
- `verbatim-model`'s comment on `DeliveredFact::key` cites NVDA source
  files, which the provenance rule forbids in a platform-neutral crate;
  rewrite it to cite the parity docs.
- After a voice change, NVDA refreshes the other settings controls from the
  driver (`gui/settingsDialogs.py`, lines 1601 to 1619); Verbatim does not.

### State models

Facts kept in several places or as combinations of flags, found by a
study of Core, the outposts and the listener on 2026-11-08, each with a
history of repeated fixes. Each is rebuilt as one explicit state with one
writer, in this order, now that Core's focus queue is built (Dickson,
2026-11-08). Symbols are named rather than lines, which move.

- Core's held-key line. Today: `HeldKeys` in `crates/verbatim-core/src/state.rs` (`waiting`, `passed_at_ms`, `awaiting_watch`, `bounded`), with the key being read found by searching the query table for `Then::HeldRead`; the stage is written at about nine places in `editing.rs` and `queries.rs`, `awaiting_watch` is found by scanning the effects just made for an `AwaitCaret`, and `bounded` is set apart from the stage its deadline belongs to. Fix history: 8658fc10 then 8893fd97 (a deadline set after the read was asked left a timer), 0bd0d8ed then c635a70f (keys held after exit), b748ab99 and 71b7ab52 (a trace lost on resuming). NVDA has no line: a caret script runs alone and waits in `_hasCaretMoved`, later scripts queued behind it. Proposed: one stage, idle, reading a key's caret (its query, key and due time) or awaiting a watch (its query, when passed on, due time), beside `waiting`, with `caret_key` returning the watch's query instead of a scan. Keys now also wait behind focus reports in Core's focus queue (ADR 0001), which the stage must keep. Its generated test is not fit (study of 2026-11-08): `crates/verbatim-core/src/queries/generated.rs` leaves the line out and checks only its awaited watch, skips its state comparison exactly when an answer is to the held keys' watch, generates one key with one shared trace, never a deadline, `BoundHeldKey`, `WithdrawText` or exit, and its resume check was loosened in 71b7ab52 to let the code pass; none of the listed defects was within its reach. The rebuild gives the stage an independent model and a world with distinct traces, several keys, deadlines and their effects, exit, and keys behind focus reports, and removes the skipped comparison. Of that file's three loose focus checks, the focus-taken and left-mark checks go, since `live_reread/generated.rs` models both; the foreground check's assertion that a foreground change changes nothing outside the foreground record stays, as no other generated test makes it.
- The listener's record of an application. Today: `App` in `crates/verbatim-listener-logic/src/schedule.rs` (`threaded`, `flight`, `abandoned`, `limit_reached`, `ending`, `left_at`, `last_top`), "spent" derived from four of them; `ending` copies the supervisor's fact and any later event clears it; records are keyed by process id alone, so a reused id inherits an ended process's abandoned count and `limit_reached` (found by reading, not reproduced); the last top-level window is kept three times (`last_top`, the running read's `top_level` in `crates/verbatim-listener/src/focus_read.rs`, `FocusReader.last`). Fix history: e282e7ea, 8fa9c917, f67f1dc9, 6c9a1a93, 1c5e1a6d. Proposed: records keyed by application generation, one lifecycle value (live with its reader idle, in charge or starved since a time, or ending) holding the abandoned count. Its generated test, `crates/verbatim-listener-logic/src/schedule/generated.rs`, is partly fit: it found e282e7ea's and 6c9a1a93's defects and would catch 1c5e1a6d's, but it follows the outcomes the schedule emits, clears `ending` on any event as the code does, and keys records by a fixed process id with no exit or reuse, so the inherited count cannot arise. The rebuild models records by generation with one lifecycle value from the recorded decisions, and adds process exit, process id reuse and the supervisor's end to the world.
- The outpost's record of hung windows. Today: the reducer's `hung` set and the program in flight's `destroyed` set and `hang_mark` (`crates/verbatim-outpost-logic/src/lib.rs`), the `abandoned` and `abandoned_on_hung` counters read by two separate loads (`crates/verbatim-outpost/src/outpost/mod.rs`), and the supervisor's `Parked` (`crates/verbatim-core/src/supervisor/policy.rs`); `NotResponding` is sent only as the set goes from empty to holding a window, and a window leaves it only when named answered, found readable or destroyed, so a background window that recovers unseen silences a later hang in another window (found by reading). Fix history: 91589a28, d7b2962d, c39da5ff, a2e2508d, 3e8c5725, b94fa1d8. NVDA keeps no record: it asks `isHungAppWindow` where it needs it. Proposed: one hang being told per application, its window judged live at each half-second check and ended when that window answers or is destroyed, and each parked worker counted once in the pong. A later hang in another window of the same application plays the "not responding" sound again (Dickson, 2026-11-08). Its test, `Run::hangs` in `crates/verbatim-outpost-logic/src/tests/reducer.rs`, is not fit: its model copies the code's sets, encodes the second-window rule Dickson reversed, was changed in the same commits as the code (a2e2508d, d7b2962d), has no step where a background window recovers unseen, and leaves out the counters and `Parked`. The rebuild needs an independent model of one hang per application judged at each check, with steps for a window answering unseen and a second hang.
- The outpost's followed focus. Today: `Tracking` (nine optional fields, `crates/verbatim-outpost-logic/src/follow.rs`), `Marks` (`intake.rs`), `FollowState` (`followed`, `followed_element`, `followed_slider`, `kept_groups`, `focus_path`, `owed`) and the newest held report, rewritten together at adoption but `focus_path`, the foreground role and batch, and element retargets written by separate parts of a commit, a commit being about fifteen independent optional fields. Fix history: 2bd69a85, 62ee398b, f6b84c89. Proposed: one followed record replaced whole when a focus is adopted, with only the foreground role and batch outside it; check first which facts the outpost still needs now that Core's focus is a queue. No generated test covers it: `reducer.rs` builds the followed change with the test's own helper and never compares `Tracking`, `focus_path`, `kept_groups`, the `followed_` fields or `entered_in_front`, and checks the marks only for placement. The rebuild settles first which facts survive the focus queue, then models one followed record replaced whole at adoption, with a world of separate commit parts.

### The ownership refactor

Every fact has one owner, logic is a pure reducer with calls as its
effects, and no process holds a copy of another's state to judge against
([ownership](../books/dev/src/explanation/design/ownership.md), Dickson, 2026-10-30). It preserves what
is heard, except the three changes the design names. Built in a session of
its own, a few large builds, each designed against the code and reviewed
just before it. All five builds are built; the rules for the planned milestones are in [the milestone rules](design/ownership-milestone-rules.md).

### The caret and selection rework

Follow-up from the reducers' simplification study: Core's
`caret_history` makes the same caret-before-the-key judgement as the
outpost, a fact judged in two places; the rework gives it one owner.

The decisions, the design and the order of work are in
[the caret and selection rework](design/caret-selection-rework.md), consolidated for building, the one
source for them; its first step is NVDA's captures of the keys not yet
captured. Agreed with Dickson on 2026-10-25 and 2026-10-29 after a study found the design's shape
right (a held key's caret read before it, the outpost watching for
evidence and comparing positions in the provider, Core choosing every
word) but a second layer, added before key holding closed the race it
answered, never removed: the caret and selection were kept in nine
places, and two known defects come from copies disagreeing (Control+A
followed by a repeated focus says nothing; a superseded key's answer
leaves Core's caret stale).

The decisions, in brief:

- Pin today's defects with tests first.
- One last read per text node in the outpost as the only baseline, one
  request per key that reads the caret before it and opens the watch, and
  Core taking every report and answer as the newest caret by one stamp,
  the read's start; the baseline's life follows the focus Core holds.
- Remove the second layer: Core's eight caret reports and its Backspace
  and Delete prediction, the outpost's eight reads (so that until
  uiAccess, M8.7, a caret key in an elevated window is judged from the
  last read), the time-zero record, the line-limited "since" check,
  report numbers, the selection baseline's three modes, the separate
  watch request, and the single superseded slot.
- Control+Z, Control+Y, Control+X, Control+V and Escape in a text field
  match NVDA: not held and silent, replacing the decision of 2026-10-08
  that spoke them; the `caret_keys` scenarios' expectations change.
- A key that moves nothing speaks as NVDA's script does, so that Up
  Arrow on a one-line field reads the line, replacing the decision of
  2026-10-08 that made it silent: at once at the edge of the text when
  nothing is selected, otherwise 100 ms after the key; a move that
  arrives later, within the watch's new 1-second bound, cuts speech and
  is spoken. Alt+Up and Alt+Down Arrow move by sentence where the
  application does not, as NVDA does. Terminals keep their reading
  rules for all of this (Dickson, 2026-10-29).
- One time stamp per caret report; the outbound queue's clause against
  merging a caret report across a reply is removed, its stated reason
  predating 505e3ad8.
- The judgement of a key's evidence stays in the outpost, named in D14 as
  the one read-time decision outside Core, since it bounds reads on text
  changes.
- M4.6's generated tests of the outpost's reducer extended to the text
  facts this rework keeps, among them one baseline per text node, once it
  has removed the second layer, so no test is written for a removed state.

### Deferred

- Review keys straight to the reducer
  ([communication](design/communication.md), section 11) is reconsidered
  when the command set is settled (Dickson, 2026-10-14).

### Follow-ups from M4.6

- The 22 target mutants the generated tests did not kill, listed in `experiments/logic-tests-study/judgement.md`, "Targets still missed", with every missed mutant's class and reason above it, are the next checks to add, when a build changes their owner.
- The second measure of the generated tests' judgement, set by Dickson on 2026-11-02, is taken over the next three builds that change a covered owner (the focus pipeline rework's third build, the caret and selection rework, and the next). Each build records every ordering, lifecycle or hand-off defect found in a covered owner, by anything, with what found it first, and the time spent on generators, models and their upkeep. A generated test's failure counts as a defect found only when its fix changes production code, and is logged when found; a failure fixed in the model or the generator is upkeep. Reason: a failure fixed before its commit leaves no trace, and counting every failure would count the model's own errors. Threshold: the generated tests find first at least two such defects across the three builds, and their upkeep stays under 15% of each build's time. Reason: October's study puts the defects within reach of these tests at roughly 8 to 12 a month in code that survives the rework, so two in three builds is a low bar, and a bar the tests cannot clear says they are not worth growing. If it is not met, the generated tests stop growing, are kept while they pass, and the simulation below is not built. The record so far, from M4.6's own build, whose defects do not count toward the three builds; each was fixed with its shrunk case kept as a replay test and a fixed case:
  - A closed outpost still tested the gap record's bound, so a title could reach Core (found by the outpost's generated test in the gate).
  - A verdict that left the record unchanged was reported as a change, so the record was sent again and the hook narrowed again (found by exploration).
  - A gap record closed by its own tick's publication still had its title committed (found by exploration).
  - A push that replaced the event an idle waiter waited on ran no waiters (found by reading while the design was surveyed; the generated test fails without the fix).
  - The listener kept an ended application for good once its abandoned read returned, one per such process (found by the scheduler's model).
  - A reading thread's failed start was stepped before the other effects of its step, so an idle answer went ahead of another application's drop (found by exploration).
  - The build's upkeep: building the tests took 5.1 hours, the mutation runs 3.7 hours of machine time, and the design's surveys and reviews about 0.6 hours.
  - Counted toward the threshold (Dickson, 2026-11-03), though found outside the three builds since no one knew of it: a gap read queued for a bound that a foreground change then replaced ran before the new bound's relatedness test was answered, so a window answered unrelated was read (found by the scheduled exploration, seed 1, Explore run 41623400047). Fixed by the gap record's removal, the third build's first commit, with its replay test.

### The simulation of the reducers, deferred

Built only if the second measure above is met. A simulation joining the reducers through pretend pipes, with their concurrency, as Dropbox's Trinity did beside its CanopyCheck tests of single pure owners. It needs two prerequisites: the transport's merge, priority and overflow rules as pure code it can call, since the real pipes merge by key, put urgent messages first and drop the oldest past 64 (`docs/design/communication.md`, section 4.4), and a recorder of the worker's commits. It is hand-written, since the simulation frameworks assume async code, and it has a fault model for application answers (slow, failing, reordered, the application exiting), counters for the states reached, a separate random stream per component, and the end-to-end suite kept, since that catches what no simulation models.

A simulated application would follow it, once the simulation has shown value by the same kind of judgement. In order:

- The worker's reads behind one trait, as the listener's reading threads already are (`Reading` in the listener's focus reading), so a program runs unchanged against it.
- A tree of elements with UI Automation's and MSAA's answers, and its events, in memory.
- A fault model taken from `books/dev/src/reference/uia-bugs.md`: a hung window, a call past its timeout, an element gone between two calls, a focus event late or repeated, a reused runtime id.
- Conformance checked as the simulated terminal's is: mockapp is the reference for UI Automation's and MSAA's own behaviour, and recorded end-to-end sessions for real applications'. Every difference is a defect in the model or a new entry in `books/dev/src/reference/uia-bugs.md`.

It does not replace mockapp, which tests the real client stack, COM and the cross-process calls at real timing; the simulated application tests the logic above the calls, in memory, under a seeded scheduler. Code outside every simulation should then be only the calls themselves: COM and UI Automation, the hooks, the pipes' I/O, the audio device and the engines.

### Follow-ups from M4.1

M4.1's follow-ups still open or deferred, each with its next action.
Unless an entry names a scenario or an `experiments/` folder, no evidence
is kept in the repository.

Tests and tooling:

- `call_counts::uia_focus_chains_stop_below_the_process_boundary` was recorded as failing intermittently under `cargo xtask ci`, a defect by `books/dev/src/reference/testing.md`, but did not fail in 456 runs on 2026-11-03, alone, with its binary and under load ([the known failures](flaky-tests.md)). Its next failure's message is to be kept and its cause found from the count that moved.
- Audit every end-to-end scenario for timing races, each fixed by waiting for evidence, not a longer bound; find why the agent's bring-forward always takes its slowest path (up to 1.4 s).
- Root-cause, from the GitHub run logs, the ARM64 unit test `the_codeview_record_read_is_the_one_dbghelp_reports` (`verbatim-trace-waits`), which failed on main before 2026-10-15 and does not reproduce here.
- The trace tool's symbol lookup for ARM64X images, such as ntdll, needs a fix testable only on the ARM64 runner (deferred, Dickson, 2026-10-21).
- Compare mode presses Verbatim+V (`kb:verbatim+v`) as Insert+V, which under NVDA types "v" into Notepad and corrupts the later steps of `notepad_editing`'s comparison.
- `cargo xtask trace scenario` and `cargo xtask measure` look for the build under `<worktree>\target` and ignore `CARGO_TARGET_DIR`, so a worktree building elsewhere measures a stale or missing build.
- A double-click is checked only by hand: the agent has no mouse request, so no scenario pins "beta selected" for a double-click inside a word.
- Key holding has no test of a waiting key following the newest watch (`crates/verbatim-core/src/editing.rs`); its other gaps are in M6.5.

Crashes and stalls:

- Verbatim's own outpost once left a fact in its intake for 6.37 seconds; reproduce it with the intake and menu logs.

Focus and File Explorer:

- Core drops a changed description that equals the name
  (`reduce_description_changed`), where NVDA speaks a changed description
  even then (`docs/nvda/speech.md`); found 2026-10-30.
- A scenario's final check that nothing more was said sees only events an
  application raised before Verbatim answered the idle barrier, so a
  focus raised just after, such as File Explorer's focus on the Start
  button after Escape closes the Windows+X menu, let `quick_link_menu`
  pass in some runs while expecting silence where NVDA and Verbatim both
  speak (found 2026-10-30); a scenario expecting silence after a key needs
  evidence that the key's effects have all been raised.
- `cargo xtask nvda compare quick_link_menu` flattens NVDA's Windows+X
  item to "Installed apps Alt, p1 of 17" where the scenario has "p 1 of
  17", so the step is judged "differs"; find whether the compare tool
  joins NVDA's speech parts without a space or NVDA runs them together.
- In one runner run the `delta` item inside Inner still carried the parent folder's window title; check whether File Explorer's title lags or an ancestor name is stale (`explorer_folder_window`).
- The listener's read in File Explorer scenarios once came 285 ms late, locally mostly the first nearest-window lookup waiting on a busy File Explorer; confirm on GitHub's runner (deferred, Dickson, 2026-10-21).
- On a fresh GitHub runner the first Verbatim start can say "Start toggle button not pressed" instead of "Desktop list", because the taskbar rehosts the Start button in a new PopupHost window and raises a UIA focus NVDA would accept too; the choice between a throwaway warm-up start and an experiment to find the trigger is deferred (Dickson, 2026-10-21).
- A focus-now answer carries no application name and no Alt state, so an application rule such as the Alt+Tab switcher's cannot apply to a focus read on request.

Events and controls:

- The classic .NET Framework Windows Forms combo box rule, element-selected events from the `ComboLBox` window reported as the combo box's value change (`NVDAObjects/UIA/__init__.py`, lines 2734 to 2744 and 2779 to 2790), is deferred until Verbatim reads such a combo box in an application (Dickson, 2026-10-20).
- The rule for a `Windows.UI.Core` combo box with the `Value` pattern (lines 2795 to 2798) is not built, since every Settings combo box checked lacks the pattern (`docs/parity/changes-and-selection.md`, "A combo box's value from its selected item").
- The events NVDA registers desktop-wide that Verbatim lacks are deferred until each is supported (Dickson, 2026-10-20; `docs/parity/backends.md`, "UIA event registration"); a foreground UIA progress bar that is not the focus is in M4.2, background progress bars in M4.3.
- Table cells: header texts, spans, a cell's own coordinates text, or MSAA and IAccessible2 cells.
- On this Windows 11 build `TrayNotifyWnd`'s UIA subtree is empty, so the tray list (`crates/verbatim-outpost/src/outpost/shell_items.rs`) is empty unless the overflow flyout is open; and a double press of Verbatim+F11 opens the tray list rather than the taskbar list.
- The shell items' `refind` (`crates/verbatim-gui/src/shell_items.rs`) takes a candidate by its runtime id alone, without its name, so a runtime id the shell has reused for another button clicks that button.

Text and selection:

- `text_box_selection_keys` fails at Control+Delete over the selection "alpha " in some runs, saying "alpha" where "beta", "selection removed" is expected: the key's watch answers from its first check, before the edit control has handled the key. The caret and selection rework removes the first check and runs the scenario ten times; if it still fails, the likely cause, unconfirmed, is Core comparing "alpha" while the outpost's word unit reads "alpha " with its white space.
- `text_box_caret_keys` fails at Control+V in some runs, saying nothing where the pasted line is expected (run 41848692720): the watch's baseline is the newest caret the outpost reported before the key, whose anchor no request names, so the outpost released it while the watch waited for the caret event after a read the paste moved, and the watch then answered with no `same_line` ([the flaky-test entry](flaky-tests.md#the-caret-watchs-baseline-anchor-released-while-it-waits)). The rework's one baseline per text node, kept with the focus Core holds, removes it (Dickson, 2026-11-04).
- The parity ledger's statement that Alt+Up and Alt+Down Arrow speak the line when the application moves the caret has no capture behind it; the caret and selection rework's captures settle it.

## M4.2 — Parity small items

Small differences from NVDA found during M4, each closed against a live
NVDA capture.

- A foreground UIA progress bar that is not the focus is reported, as
  NVDA does (Dickson, 2026-10-10, as a separate fix); background progress
  bars are not yet reported (M4.3).
- Toasts ([parity](parity/alerts.md), "Toasts"): Verbatim reads them only
  through an MSAA alert, NVDA through UIA's window-opened event. Capture a
  Windows 11 toast live with NVDA and with Verbatim; if Verbatim is silent,
  follow NVDA (a `Windows.UI.Core.CoreWindow` whose automation id contains
  `ToastView`, from a desktop-wide window-opened subscription in the
  listener), then adopt NVDA's rule dropping MSAA alert and show events
  from native UIA windows. Its end-to-end scenario, `toast`, raises a
  toast of its own: it turns Windows' Do Not Disturb off for the run and
  restores what it found at the end, so a machine with it on keeps it on
  (Dickson, 2026-11-03).
- Selection inside a list the focus controls: the
  `settings_search_suggestions` scenario covers it, compared with NVDA
  on 2026-10-20. Left to check against a live NVDA capture are the
  differences the parity entry records from the source alone ([changes
  and selection](parity/changes-and-selection.md), "Selection in a list
  the focus controls"): a stale selection after a second replacement of
  the list, a selection in a second controlled element, and a selection
  raised on the controlled list itself.
- The review cursor on objects that no longer exist (Dickson,
  2026-10-08): most review commands answer from Core's cached line or the
  node's snapshot, and a Gone answer is dropped with the navigator left on
  the dead node, so a closed tab's or dialog's old text can be read where
  NVDA, reading live on every command, gives silence or "blank". Review
  commands read live from the application rather than from Core's cache;
  a Gone answer for the navigator's node re-seeds it from the focus, or
  says "No navigator object" when the dead node is the focus; the outpost
  reports held MSAA nodes it forgets with their window; and an end-to-end
  test in both terminals that a reviewed line stays on its text as new
  output is written below it without filling the history. The review
  position is the provider's, with no bookkeeping of Verbatim's own: once
  the history is full, staying on the same row is acceptable.
  Flat review's current-line command (`crates/verbatim-core/src/reduce.rs`,
  the flat `ReviewCurrentLine` arm) moves the review offset to the line's
  start, where NVDA's current-line command does not move the review
  position; it is fixed with this item ([review parity](parity/review.md)).
  Object navigation from a node whose element is gone: Verbatim moves the
  navigator to the focus and announces it, where NVDA says "No next" or
  "No objects inside", or takes the window for a parent step
  (`NVDAObjects/UIA/__init__.py` 2136 to 2196); settled here against a
  capture. Such answers are more frequent since the outpost's runtime-id
  search was removed (719b4ed4, 2026-10-29), so a node with no kept
  element and no focus route answers gone.
- Table cells: an unchanged row or column within one table left out, as
  NVDA leaves it out, which needs the containing grid's identity read
  with each cell (Dickson, 2026-11-01;
  [focus announcement](parity/focus-announcement.md), "Table cell
  coordinates").

The remaining work is two sessions (Dickson, 2026-10-21): one takes
toasts, the foreground UIA progress bar, and the differences in a list
the focus controls; the other takes the review cursor on objects that no
longer exist. The table cells go with the first session.

Exit: each entry is matched (verified) by a scenario, with a declared
deviation where Dickson chose one, checked with compare mode.

## M4.3 — Settings and indications

These are decided designs that daily use needs.

- The settings package (Dickson, 2026-10-08, from an inventory of NVDA's
  242 user-visible settings against Verbatim's): the
  missing settings for behaviour Verbatim already has (capital pitch
  change, OneCore's pause after punctuation, the audio output device,
  sounds' volume following the voice,
  handling keys from other applications, the multiple key press timeout,
  the review cursor following focus, reporting tooltips, reporting
  notifications, the word segmentation standard, cancelling speech for
  expired focus events, trimming leading silence); dialog controls for
  the nine settings that exist only in `settings.toml`; and three small
  features (say "cap" before capitals, delayed character descriptions,
  the spelling-error sound while typing). The review cursor follows focus
  and the caret by default, each with NVDA's setting to turn it off.
  Spoken progress comes every 10 percent, as NVDA's does. Each of NVDA's
  settings for a feature not yet built lands with that feature's
  milestone.
- Status cues (Dickson, 2026-10-08): every indication in
  the theme's Events group (start and exit, error, application not
  responding, browse and focus mode, suggestions opened and closed,
  progress) plays through key presses; indications in every other group
  are content and are cut off. This is built-in behaviour defined by the
  group, not a setting.
- Sound-only announcements (Dickson, 2026-10-08): sounds without speech
  are normal, since a theme can set any indication to sound alone, so an
  announcement a theme reports by sound alone is queued, timed, measured
  and cancelled exactly like a spoken one; the start-up sound becomes
  one.
- Background progress bar reporting with its setting, an NVDA option off
  by default (Dickson, 2026-10-10, confirmed as planned for later on
  2026-10-15; [changes and selection parity](parity/changes-and-selection.md),
  "Value change on focused node speaks bare value").

Exit: the settings dialog covers the settings package's inventory, and
every indication plays or speaks as its cue says.

## M4.7 — Containment

Built with M4.8, after M4.3 (Dickson, 2026-11-05). The design is
[containment](design/containment.md): every thread and callback edge
contained, a panic in a child ending only what it must, a native-exception
handler in every child that writes Verbatim's own dump so Windows Error
Reporting never runs (the security model's L5), a GUI that cannot start
making Verbatim fail to start, the GUI thread's rules, the hook passing
keys while the reducer is stuck, the raw-input hook watcher with its
reinstall, liveness judged from the working threads, the relay contained,
and the containment scan in `cargo xtask ci`.

Exit: the containment scan passes with every edge guarded and every
thread from the one spawner; a unit test panics or throws at each kind of
edge and asserts its stated outcome; `keyboard_hook_reinstalled_after_removal`
passes; a forced native crash in an outpost, the listener and the
synthesizer host each leaves a dump in Verbatim's folder and no Windows
Error Reporting entry; and wake-ups and processor time are measured
before and after.

## M4.8 — Guardian

Built with M4.7, after M4.3 (Dickson, 2026-11-05). The design is
[the guardian](design/guardian.md): `verbatim.exe` as a guardian that
starts and restarts `verbatim-core.exe`, Core's one panic path with its
held keys released in order, the single instance and replacement through
a private namespace (the security model's C6 and C2's startup names,
moved here from M6.6), the launch rules, the end of the session, hang detection from the
Core page's stamps, old children exiting before their replacements, the
restart breaker with its last restart without extensions and then
shutdown, dumps of Core written from outside, Windows Error Reporting's
dialog suppressed for a fast-fail, a memory cap on Core (A2), the
1-second silence budget, and the timeouts of its section 12, with MSAA
calls cancelled through COM (moved here from M6.1).

Exit: the `InjectFault` scenarios pass for a reducer panic, a relay panic,
another Core thread's panic, a Core crash and a Core hang, each speaking
the focus again with its time to speech recorded; the breaker shuts
Verbatim down on the third crash within 120 seconds; a Low-integrity
process cannot stop or replace Verbatim; the generated tests of the
breaker and the owner's holds pass; and wake-ups and processor time are
measured before and after.

## M4.9 — Lints and unsafe

After M4.7 and M4.8, and before M6.2 (Dickson, 2026-11-05). The design is
[lints and unsafe](design/lints-and-unsafe.md): the panic-free lints in
the Windows crates and then workspace-wide (the twelve crates that build
on Linux are done), every fallback that could only be a logic bug removed
by construction, the three latent bugs fixed, `unsafe_code` denied outside
the wrapper crates with safe wrappers, bounds on sizes applications
control (the security model's U2 to U4, moved here from M6.6), terminal
alignment matched by row hash first with a cap and a scroll-shift
fallback (U5), and generated tests and fuzzing at the parsers (X12).

Exit: the lints are in `[workspace.lints.clippy]` with no crate-level
copies and no exemption but the item-level ones the design names;
`debug_assert!(false, ...)` and the `proven` modules are gone;
`unsafe_code` is allowed only in the wrapper crates; the hostile
`mockapp` mode and the wide-row terminal test pass within their stated
bounds; each parser has its generated tests and a fuzz target in CI; and
wake-ups and processor time are measured before and after.

## M5 — Extensions v1

The design is [extensions](design/extensions.md); the order of work, with the
tests that prove each step, is [the M5 work plan](design/m5-extensions-work.md);
the surface as extension authors see it is the Extensions guide's reference
(`books/extensions/src/reference/readme.md`), settled with Dickson on
2026-11-09. This entry keeps the scope and the exit criteria. M4.4, which gave the
messages the extension surface exposes their final home, is done.

Before step 1, the design is restated against the outpost's reducer,
which postdates it, and reviewed adversarially (the work plan's step 0b;
Dickson, 2026-10-31). Each step adds its invariants and model checks to
M4.6's generated tests, and the rules, the derived capabilities and the
package reader get generated tests of their own.

- Modules as folders: a module is a `module.toml` beside a Wasm component;
  a built-in is a module folder under `extensions/builtin/` (planned), never listed
  and never disabled by the user; an extension is a `.vext` bundle of
  modules, a zip its author makes by hand, as NVDA's add-ons are, with no
  packaging tool; installed by a request to the running Verbatim through a
  small helper executable and the control plane (NVDA's `nvda_slave.exe`
  pattern), never by a flag on `verbatim.exe`, whose single-instance
  takeover would kill the running instance, or from the Extensions
  dialog's Install from file button. At most one installed extension
  claims an application, above the built-in, and no two modules of one
  extension claim the same one; a second is refused at install, so
  installed extensions need no order among themselves (Dickson,
  2026-11-09).
- Rules as data: a matcher and an action with a required id and an explicit
  backend, scoped to the sender's application identity, applied by the
  outpost before any hook runs, refused at load when two rules of one module
  contradict.
- The component host on wasmtime with the component model (D6, decided
  2026-10-22), in extension host processes only: every built-in runs as
  one internal extension in a host process of its own, started with
  Verbatim and always running, so no Wasm runtime is in Core, the
  listener or an outpost (Dickson, 2026-11-09). A host runs one thread
  per lane: each outpost it serves, each global module; its reads are
  requests straight to the claiming outpost, its speech and every action
  that depends on attention go through Core's reducer. Each hook's
  deadline is judged by its own store, since the epoch is the engine's;
  the outpost's reducer calls each hook and holds its event intake
  while it waits, its worker answering the host's reads as ordinary
  programs, and a module's effects travel back with the hook's verdict
  inside the event's report. Speech is enforced by attention. The hooks are `on-focus`,
  `on-event`, `on-node` (one call per emitted batch), `status-bar`,
  `on-command(command, trigger)` and `on-list-dialog`, and the WIT files
  are the single source of the host's and the guests' bindings and of
  the API reference pages, which `cargo xtask ci` checks are current
  (Dickson, 2026-11-09). The `ci-arm64` job ratifies Cranelift on ARM64 and
  measures host-call overhead against the hook deadlines; Pulley is
  switched on only if that ratification or a Cranelift fault requires it.
  A hook's time is a stage in the latency timeline and two rows in the
  performance ledger.
- Capabilities derived from the component's imports, never declared; the
  install and Extensions dialogs are laid out as lists to tab through
  (applications, commands with their conflicts, and warnings only when
  there are any), worded as the Extensions guide's capabilities page says
  (Dickson, 2026-11-09).
- The breaker: three failures to respond within a minute disable an
  installed extension for the session, spoken with its name, and a
  built-in module alone, since built-ins have no switch of their own
  (Dickson, 2026-11-09) ([security model](design/security-model.md),
  A9); resource figures per extension in developer mode (A6).
- Binding by application identity (executable, console client, hosted app,
  WebView2 parent) resolved by the supervisor, one host process per
  extension, of every kind, the developer folder working as one extension
  as the built-ins do, in one host of its own (Dickson, 2026-11-09;
  [security model](design/security-model.md), X4); a developer-folder
  module replaces an installed extension's module for the application it
  claims while it is there, as NVDA's scratchpad does (Dickson,
  2026-11-09). Loading, enabling and hot reload done on a loader thread
  of Core's, which reads, validates and compiles, while the reducer owns
  each extension's active version and publishes the rule tables and the
  hook registrations to the listener and the outposts, and the gesture
  map to the hook thread, at a version (Dickson, 2026-11-09); hot reload
  of developer and built-in module folders in developer mode by
  validate-then-swap, the folder watch starting from the `notify` crate
  with `notify-debouncer-full`, adopted only once its source and its idle
  wake-ups are checked.
- Gestures that change while Verbatim runs (Dickson, 2026-11-09): the
  keyboard hook thread's gesture map published at a version, so module
  commands follow install, enable and reload; a binding names a number
  of presses, every press fires at once, the hook thread alone judges
  from its own map whether the same keys are bound to a higher count
  and stamps the press when they are, and a list dialog the command
  opens is held until that press's multi-press window has closed and
  every later press of the same keys has answered, so a double press
  never shows the first dialog. Gesture conflicts are listed in the install dialog rather than
  refused, replacing the reserved gestures.
- Global modules (Dickson, 2026-10-31, reversing the decision of
  2026-10-22 that left them out): the tray list, the taskbar list and
  reading the current time move from core code to global modules, which
  run in their extension's host on a lane of their own, read only the
  applications their manifest's `reads` names, and may be shipped by
  installed extensions too (Dickson, 2026-11-09); the surface gains a
  clock, the Windows version, a list dialog shown by Core with its answer
  hook, and a click on a node the module holds.
- The minimal log ring, the log viewer window and the Verbatim+F1 focus
  report, pulled forward from M9 because module failures and rule
  provenance need somewhere to be read.
- Built for the installed copy that M8.6 adds, though M5 ships only the
  portable one: every path to a folder Verbatim writes (settings, installed
  extensions with their staging folders, the developer folder, compiled
  code, logs) comes from one resolver of the configuration root, which
  answers the folder beside the executable in portable mode, and never
  from the executable's own path; built-ins are found from the program
  folder, never the configuration root, since they are the same for every
  user; compiled code is named by its content, the runtime's version and
  the architecture, in a cache that can always be rebuilt; the `.vext`
  helper finds Verbatim only through the control plane, never by a path,
  and shows a message box that Verbatim must be running when nothing
  answers, as NVDA's `nvda_slave.exe` does; and the Extensions dialog has
  an Install from file button, which is how a portable copy installs, with
  no file association registered (Dickson, 2026-11-09).
- The guest workspace under `extensions/` (planned) with a permissively licensed SDK;
  every built-in written in Rust, Calculator and Notepad first.
- Every Windows inbox application as a built-in: Explorer (starting with the
  cosmetic fixes deferred from M3), Windows Terminal, Notepad, Calculator,
  Task Manager (proven by `mockapp` until M8.7 lets it be read end to
  end), Settings, Start and search, Quick Settings and Open With; sleep mode
  as a core per-application list rather than five modules.
- Windows Terminal's second terminal identity as a focus veto rule of the Terminal built-in, never core code
  and with no hook (Dickson, 2026-11-09): the listener drops every focus
  event from a `TermControl` whose UIA runtime id starts 14, so only the
  usual identity starting 42 is announced and followed. In every one of
  the 156 focus events from a 14 identity measured, a 42 from the same
  process arrived within 1 second, in either order; where the 14 comes
  first, "terminal" is said when the 42 arrives instead, which is the
  rule's one cost, measured by rerunning the experiment's judge with the
  order and the gap recorded. Node matchers gain the UIA runtime id, with
  exact and prefix forms, and the listener's cache carries it. Tests: the
  terminal is announced once and followed afterwards for each order of
  the two identities, and with two panes in one window; the
  `windows_terminal_` scenarios without the optional repeated "terminal".
  Evidence:
  `experiments/windows-terminal-raises-focus-from-a-second-runtime-identity/`.

- Configuration versioned, by the ownership model's version rule
  ([ownership](../books/dev/src/explanation/design/ownership.md); Dickson, 2026-10-30): the rule
  tables and the fetch configuration carry a version published by Core
  alone, every judgement the listener or an outpost makes under them
  names that version, and a receiver recognises a judgement made under
  a version it has moved past and acts on it by its kind, as
  [extensions](design/extensions.md), section 2.2, says.
- Focus vetoes and redirects with no read NVDA would not make (Dickson,
  2026-10-29): a veto declared as a rule judged by the listener
  from the event's cache, and by the outpost on every focus it reads
  itself; a hook's code veto or redirect run in the
  outpost after the listener's read, as NVDA runs `event_gainFocus` after
  its walk, so it makes no read NVDA would not; node and event rules
  applied in the outpost to every kind of focus report, its own recovery
  reads included; kept verdicts dropped when rule tables are reloaded; and
  File Explorer's search band refusal (the focus pipeline
  audit's difference 24) as a rule of the File Explorer built-in, needed
  on Windows 11 24H2 and not on 25H2, with no `StartButton` rule, since
  the rework's first step found no keyboard route to the MSAA focus on
  File Explorer's `Start` window.
- The system tray and taskbar lists (Verbatim+F11, pressed once and
  twice) as a global module (Dickson, 2026-11-03). Today they read the
  notification area's UIA tree, which is empty on Windows 11 build 26200
  unless the overflow flyout is open. NVDA's `systrayList` add-on reads
  the MSAA children of the taskbar's
  `Windows.UI.Input.InputSite.WindowClass` window instead, the taskbar's
  buttons under its first child, and clicks an item at its location
  (`ruifontes/systrayList`, read on 2026-11-09). An
  end-to-end scenario, `tray_list_and_task_bar`, lists Verbatim's own
  tray icon and opens its menu with Enter, then lists the taskbar on a
  second press.

Out of M5 by decision (Dickson, 2026-10-22, amended 2026-11-09): module
settings and storage, the settings view and the timer and asynchronous
imports (M13), `on-load` and `on-unload`, the utterance, settings-changed
and response hooks other than the list dialog's, updating installed
extensions,
scripted modules and the interpreter console (M10), and the
`verbatim:synth` world with its tone synthesizer (M6, where the host work
is).

Exit: an app module can be edited and hot-reloaded without restarting
Verbatim; a host function a module did not import is unreachable from it,
a surface item outside the design is refused at load, and both are tested;
the inbox built-ins' scenarios are green. This milestone opens the
app-module porting track (see the section after M18), which then runs
continuously alongside every later milestone.

## M6 — Native synth host + Eloquence

This milestone comes right after M5 and before browse mode so that
Eloquence can be delivered through the extension mechanism and
Verbatim can become Dickson's daily screen reader sooner (Dickson,
2026-10-15). The design is section 12 of [extensions](design/extensions.md)
and D18; the order of work, with its tests, is
[the M6 work plan](design/m6-native-synth-work.md).

Its first build is the synthesizer host's recovery (Dickson, 2026-11-05):
a backoff between host starts; a start that can be cancelled, so a host
that never becomes ready does not hold each utterance for its ten
seconds; after repeated failures, a fallback to eSpeak NG, or to OneCore
when eSpeak NG is the one failing, never to silence, saying why (the
security model's A3); and a memory cap on the host.

- The `verbatim_synth_v1` C ABI, the `SynthDriver` trait spelled as C
  exports, and the adapter crate that generates them; the `verbatim:synth`
  world with a tone synthesizer component, hosted by the same
  `verbatim-synth-host`, so Wasm and native engines share one host from
  the start.
- The host loads adapters matched by architecture, read from each DLL's
  file header rather than declared in the manifest, which only lists the
  DLLs; Core picks the best build the machine can run, its own
  architecture, then x64, then x86, and an extension with none is refused
  at install (Dickson, 2026-11-09). A reduced x86 host carrying only the
  adapter loader and the protocol, cross-built for `i686-pc-windows-msvc`
  by `cargo xtask`, serves 32-bit engines on x64 and ARM64. ARM64
  installations are to run x64 engines too, through a full x64 host under
  emulation, not built in M6 (Dickson, 2026-11-09).
- The ordinary AppContainer for every host (Dickson, 2026-11-09), the
  container's access entries set by the contained launch itself, and a
  probe child asserting every denial.
- The Eloquence adapter, written from IBM's documented SDK as
  `eloquence-x86.dll`, with the engine files supplied by the user; the
  `eloquence_` scenarios run by Dickson by hand with the engine files,
  never in CI.
- A shared-memory PCM ring only if the pipe path misses the budget for
  Eloquence; pipes until numbers prove otherwise (D18). A ring, if built,
  is a well-tested crate or has a `loom` model before it is used, since it
  would be a lock-free structure shared between processes.
- The host's command handling as one pure module both host builds share
  (commands in; speaking, stopping, answering an utterance cancelled
  before it began and applying a setting out), and `HostedSynth`'s view
  of the host as a pure state: what the host owes, what drains, and the
  hang rule, a host hung once it has owed a message for the timeout with
  nothing sent (2bdfffb7). Generated tests against a model of the pair
  check that every `Speak` ends once, nothing of a cancelled utterance
  plays, and a host that stops answering is judged hung within the
  timeout. The adapters' mark tables get generated tests: every mark
  reported once and in order, and the PCM length exact (Dickson,
  2026-10-31).

Exit: Eloquence speaks through Verbatim under the sandbox; the latency
scenario is green in Dickson's run of the `eloquence_` scenarios, recorded
here.

## M6.1 — Hung and flooding applications

Scheduled after M6 and before M7, whose IAccessible2 work adds many COM
calls into browsers (Dickson, 2026-10-29). Separate from the focus
pipeline rework, since cancelling a stuck call frees a thread that is now
abandoned but changes nothing spoken.

`CoCancelCall` ends no UIA client call, because UIA waits on its own
channel rather than in a COM call
(`uia-facts/nothing-cancels-a-uia-call/`, 2026-10-30),
so the UIA readers keep abandonment and `FOCUS_READ_DEADLINE`.
Cancelling the outpost worker's stuck MSAA calls through COM is built in
M4.8 ([the guardian](design/guardian.md), section 12).

Found while weighing the process layout (2026-11-01;
`books/dev/src/reference/uia-bugs.md`, "A hung window named in a system focus event holds
a process's UIA events"):

- The ghost window: when a window stops answering, Windows shows its
  ghost, "(Not Responding)", which takes the foreground, and Verbatim
  speaks nothing, neither then nor when the window answers again.
  NVDA says nothing either: captured on 2026-11-07 with NVDA's pinned
  build, the ghost (class `Ghost`, owned by `dwm.exe`, titled "...
  (Not Responding)") took the foreground about 5 s after a freeze, in
  front, after Alt+Tab back and after a taskbar click, with no speech
  then or on release. Injected keys do not reach a foreground ghost, so
  keys pressed while it is in front were not captured. Speech that
  differs from NVDA's is Dickson's.
- An event flood wedges the process receiving it: at about 10,000 UIA
  events a second from one application, UIA's worker thread in the
  receiving process stops delivering any event and keeps one core busy
  for at least 90 seconds. A flood of name, value or text changes wedges
  that application's outpost, and a flood of the events the listener
  registers desktop-wide (focus, selection, menu opened, notifications)
  would wedge the listener and with it every application's UIA focus.
  Nothing detects either, since no call of Verbatim's is stuck. First
  measured: the rates real applications reach, a terminal's output and
  bursts of notifications.
- Queues an application can fill have no count limit: the listener's and
  the outposts' queues merge by key only, and the outpost mailbox and
  Core's port queue are unbounded.
- The listener was held 7.8 seconds by a focus event naming the
  long-running test window `hung-msaa`, twice the 3.8 seconds against a
  new one; the cause is not known.

## M6.2 — Outpost retirement

At most one outpost serves an application at any time (Dickson,
2026-10-29): a failed or retired outpost is ended, killed if it does not
exit, before its replacement starts, because a failed outpost is retired
on a thread of its own while its replacement is started after the backoff
(`crates/verbatim-supervisor/src/owner.rs`, the failure path),
so for up to `SHUTDOWN_LIMIT` a stuck outpost and its replacement can
both be in the application, where two Verbatim processes' calls collide
inside UIA (`books/dev/src/explanation/design/listener-without-connections.md`).

- The supervisor checks an application's exit twice, from the outpost's
  report and by polling the process; M6.2's single child record keeps
  one (the reducers' simplification study).
- A mockapp test that fails an outpost during a held remote operation and
  checks what its replacement reads.
- The replacement started only once the old process has exited, the old
  one killed 1 second after its shutdown message if it has not exited
  (Dickson, 2026-11-06).
- `SHUTDOWN_LIMIT`, 21 seconds today, becomes 1 second (Dickson,
  2026-11-06), the bound the order above works with.
- The supervisor's owner thread as a pure state machine, with the clock
  and I/O completions as inputs (Dickson, 2026-10-31, revisiting the
  ownership refactor's default of 2026-10-30;
  `experiments/ownership-study/audit-structure.md`, item 7), in
  `verbatim-core` beside the decisions M4.5 moved, built with the order
  above, since this milestone rewrites it; and readiness as one of its
  decisions, announced only once every hook is installed and Core knows
  the focus (ff25f3dd, 46b993fa). A generated test against a model checks
  that at most one outpost serves an application at any time.
- The decisions in `crates/verbatim-app/src/main.rs` moved into pure,
  unit-tested code beside the owner thread's, in the same design (Dickson,
  2026-11-04): about 350 of its lines decide (the idle barrier, query
  deadlines and routing, the exit), and today only the end-to-end suite
  runs them.
- Retirement only on evidence that the fault is Verbatim's (Dickson,
  2026-11-05): an outpost that looks stuck first checks itself with a
  fresh worker making one cheap call; only if that also sticks does a
  one-shot check process, the outpost binary in a check mode, make the
  same call. The check answering while the outpost is stuck means
  retire; the check stuck too means the application is at fault, so the
  outpost's threads are capped and it waits.
- Backoff that does not reset for a repeated failure of the same reason,
  and a restart shown not to help stopping after a number of same-reason
  failures, until the application comes to the foreground (Dickson,
  2026-11-05).
- Caps on the threads an outpost and the bounded UIA client can abandon
  while an application is hung.
- The listener restarted with a backoff, and a listener whose WinEvent
  hook or UIA registration failed detected as blind rather than reported
  ready.
- A bound on the outbox of messages refused while a child is busy.

## M6.3 — Speech and audio pipeline

The speech and audio pipeline looked at as a whole, after M6 adds a
second real engine (Dickson, 2026-10-29).

Its first build is the audio device's containment (Dickson, 2026-11-05):
every error contained at the device edge; no busy loop reopening the
device after a write fails; and a liveness check that notices speech
requested with no frames reaching the device, never falling back to
silence.

- An audit of the speech and audio pipeline against NVDA, as the focus
  pipeline's was made: for each step, the rule, the order, what happens
  on a slow or failed engine, and what is heard, across the speech
  manager's priorities and cancellation, the culling of expired focus
  speech, index marks and say-all's continuation, the synthesizer host
  and the mixer; every difference no decision covers is fixed here.
- Long utterances (Dickson, 2026-11-09): OneCore synthesizes a whole
  utterance before returning any audio (`crates/verbatim-synth-onecore`,
  `SynthesizeSsmlToStreamAsync` on the whole SSML), so a very long text
  is heard only after all of it is synthesized and its audio is held in
  memory, the likely cause of NVDA's freeze on speaking a long copied
  text, which NVDA avoided rather than fixed (nvaccess/nvda#11843, fixed
  by speaking a character count above 1,024 characters in
  nvaccess/nvda#11852). First measure, with OneCore and eSpeak NG, the
  time to first audio, the synthesizer host's memory and that cancelling
  stops it, for a text of 100,000 characters; then, if that confirms the
  cause, have the speech pipeline hand a synthesizer long utterances in
  pieces split at sentence or clause boundaries, each synthesized while
  the one before plays, with every piece keeping its marks so say-all and
  cancelling work across them. The size of a piece is a latency budget
  Dickson sets. No cap on the length of extensions' speech is needed
  once this is done.
- When anything handed to the synthesizer or the mixer has expired,
  everything handed on is stopped and the still-valid utterances are
  handed on again, as NVDA's speech manager does; today they are lost
  (`crates/verbatim-speech/src/manager.rs`, `drop_expired`), a critical
  bug (Dickson, 2026-10-29).
- The menu-item clause of the culling of expired focus speech judged for
  every waiting utterance, as NVDA does, where Verbatim judges it only
  for the focus just left (the focus pipeline audit).
- The audio step of the communication design
  ([communication](design/communication.md), section 11; Dickson,
  2026-10-08): the audio stream stays awake after speech, writing silence,
  for a configurable period, 30 seconds by default as in NVDA, because
  Bluetooth, USB, HDMI and virtual-machine endpoints otherwise clip or
  delay the next speech; with it, the device keeps running, writing
  silence, while an utterance is unfinished, and a lane push wakes the
  audio thread only for an utterance's first record, a cancel or a pause,
  the device's own period waking it for the rest, which brings back the
  wake-ups the output build added by waking on every synth chunk, with
  generated invariant tests for the mixer driven pass by pass on the fake
  device (every utterance ends once, a cancel ends only older ones, nothing
  plays after its cancel, marks in order, a waiting producer always woken)
  (`books/dev/src/reference/performance.md`, "Threads after the output build"), with no state
  shared between threads; before them, a test driver that runs the
  mixer's pass (`AudioLoop::pass`) on the test thread with events
  delivered in the same step, onto which the mixer tests that check its
  decisions (what is written, when an utterance starts, reaches a mark or
  ends, cancels and format changes, about 23 of 27) move, so that holding
  is not stepping, settling is stepping until a pass waits with nothing
  done, and nothing waits on time; the tests of the threading itself (a
  producer woken for room and at shutdown, a write waiting while too far
  ahead, the allocation test) stay on the real threads, each waiting only
  on evidence; today's tests judge from outside when the mixer is idle,
  which caused both races fixed on 2026-11-02 and leaves the allocation
  test's timeout unexplained (`docs/flaky-tests.md`, "The mixer's
  allocation test under load"), which is reproduced in the stepped form or
  ruled out (Dickson, 2026-11-02); the silence written keeps waking the audio
  thread every device period while the stream stays awake, a cost the
  period setting weighs; first measure key press to first audio heard for
  NVDA and Verbatim on one machine, then try a larger device buffer, which
  gives more margin against running dry and should add no latency while a
  cancel still discards the queue at once (NVDA's WASAPI player asks for
  400 ms, `nvda/nvdaHelper/local/wasapi.cpp`, against Verbatim's 40 ms),
  and the 10 ms start threshold, which NVDA does not have; the synth
  thread writing to the device itself, as NVDA's does, is not pursued,
  because with the synthesizer in its own process (decision D18) and the
  mixer the one place for mixing, recording and recovery (decision D17)
  it would make the synth thread the mixer and put its blocking work on
  the real-time path; cancelling keeps Stop and Reset, as NVDA does,
  since shared mode has no other way to discard queued audio; the engine
  period trial; and the earcon re-mix, where speech that starts while an
  earcon plays is mixed into audio not yet played, re-mixing what was
  already handed to the device, so an indication's sound and its speech
  start together and cancelling an utterance cancels both. Speech now
  waits behind the roughly 40 ms of earcon already queued, which delays
  every start-up's first announcement. The latency ledger stops counting
  the tail of earlier speech as "mixer and device", which made
  back-to-back utterances look slow there. Each is measured with the
  device stages of the communication design's section 10.1.
- Passing runs log 1 to 10 warnings "the device ran dry in the middle of
  an utterance: an audible gap" from `verbatim::audio` (debug build); look
  at them with the audio step.
- The audio device awake time setting, with the awake period above.
- A notification that interrupts say-all is spoken at once and say-all then resumes where it was, as NVDA does (Dickson, 2026-11-03): NVDA speaks a MostRecent or ImportantMostRecent UIA notification at its NOW priority while say-all runs (`nvda/source/NVDAObjects/UIA/__init__.py`, lines 2546 to 2557), and NOW interrupts lower-priority speech, which resumes after it (`nvda/source/speech/priorities.py`, lines 22 to 26; `nvda/source/speech/manager.py`, lines 257 to 265 and 403 to 412). Verbatim has no interrupting priority that resumes: today Core does not stop say-all on an interrupting notification, so a read-ahead answered before the cancellation reaches Core queues more text after the notification, and say-all then stops (reproduced by a replay test of Core, 2026-11-03). Needs an interrupt-and-resume priority and say-all resuming from the point reached; designed with this milestone's speech work.
- What Verbatim does for a user whose first OneCore activation after logon takes longer than the synthesizer host's ten-second hang bound is open (Dickson, 2026-10-21).
- The speech queue thread's decisions as a pure reducer, built with the
  `drop_expired` fix, since both rewrite the queue (Dickson, 2026-10-31), with a generated test
  against a model of the queue: every utterance ends exactly once, a
  cancel ends only older utterances, a cull keeps every utterance still
  valid.
- Generated tests for OneCore's WAV and mark parsing (any bytes: no panic
  and no read past the end) and eSpeak NG's mark splitting (every sample
  delivered once, marks in order).
- The check that a speech lane's producer goes on only with room for what
  it needs (`crates/verbatim-audio/tests/mixer.rs`, around line 590) passes
  with a producer that asks for room for one frame, so the size it waits
  for is made exact with the mixer's tests driven pass by pass (Dickson,
  2026-11-03).

## M6.4 — Terminal residuals

What remains of the terminal redesign (Dickson, 2026-10-29).

Built from [the terminal model](../books/dev/src/explanation/design/terminal-model.md) (accepted by
Dickson on 2026-10-25), which states what is read, the rules and the
limits; each part is one step in the sense of `REVIEW.md`, with nothing
pushed until the last. The model in `verbatim-text`, its move into Core's
reducer, "Report status bar" and moving by command are built. What
remains, in order:

- Generated invariant tests for the terminal's line comparison, if it is pure: the comparison of an old and a new screen must give back the new one exactly.
- Pi's regular mode in the moving-by-command and "Report status bar"
  scenarios, once Pi is installed on the test machine.
- Recordings for the conformance corpus, after checking that the NVDA pin
  equals the installed build: Pi's regular mode resized, set aside until
  the corpus can record a resize (Dickson, 2026-10-25); fzf, once Dickson
  installs it. htop is dropped (Dickson, 2026-10-25).
- The terminal simulator's line feed in the console host (Dickson,
  2026-11-04): the console host's default output mode turns a bare line
  feed into a carriage return and line feed, but `verbatim-term-sim` moves
  down a row without returning, so a script writing LF replays wrongly;
  `a_flood_above_a_footer_past_the_console_hosts_history_says_more_than_it_holds`
  (`crates/verbatim-core/tests/terminal_floods.rs`) writes CRLF to work
  around it. Confirm the console host's behaviour with a conformance
  recording before changing the simulator.
- A program silent on a key that then prints output and redraws a status
  row within 300 ms has the redrawn row spoken at once as the key's
  answer, ahead of the queued output: "Rows the keys own" counts every
  read in the key window as the keys' (`conhost_footer_overflow`, since
  retired, 1 of 3 local runs on 2026-11-03). A known
  limit until here (Dickson, 2026-11-03): replay the recorded reads
  ([the evidence](../experiments/a-redraw-in-a-keys-window-is-spoken-as-its-answer/readme.md))
  through the rules to confirm the cause, then ask whether a key's
  ownership of output should rest on firmer evidence than its window.
  The window fails the other way too: in `windows_terminal_clear_flood`,
  since retired (run 41693694156), Windows Terminal showed a typed "." 472 ms after the
  key, under the runner's load, so it was read as output and spoken "."
  by the timer rather than echoed "dot" (Claude, 2026-11-04, pending
  Dickson's acceptance).
- `conhost_long_lines` once heard "r" after Enter where its output line
  was expected (run 41333390347, [the known failures](flaky-tests.md));
  find the cause, the typed-character rules suspected (Dickson,
  2026-11-03).
- A typed line's wrap shown over two reads: an empty row that appears
  below the typed line, where the caret already is, is not new output.
  The console host showed the row in a read 13 ms after the one showing
  the typed character, and Core spoke it "blank" as the key's answer
  (`conhost_typing`, run 41859263092; Dickson, 2026-11-04).
- Whether a key such as Control+Shift+Tab, which Windows Terminal takes
  for its own tab switcher, opens a caret watch in the terminal: the
  watch's read waits on Windows Terminal opening the switcher, up to the
  11 s handling deadline (`windows_terminal_tabs`, run 41859263092;
  Dickson, 2026-11-04).
- End-to-end scenarios: the existing terminal scenarios brought back to
  passing; then cargo's build bar below its output; the landing rule's
  keys; Control+W, Control+U and Control+K; tab completion; suggestions; a
  completion list; menu completion; history search; output while typing;
  pasting; slow echo; a spinner with a message; several progress lines;
  resizing; a list shown by colour; a checkbox list; a refreshing
  full-screen script; fzf-like filtering; the review cursor kept through a
  footer redraw and moved with its text through a scroll; output just
  after the focus with an interactive shell; the console host's real
  password prompt; and Pi in both modes. Then, as decided on 2026-10-24,
  Verbatim's own streaming program (`verbatim-stream-test`) at up to 60
  frames a second in each way programs stream, with a footer, a header and
  a middle status row, expected to speak the streamed text once, in order,
  with no word split, whatever the reads' timing.
- The measurements after, in [performance](../books/dev/src/reference/performance.md), for typing, a
  20,000-line flood, a Pi stream at its recorded pace, a clock ticking
  once a second, and a full-screen program refreshing rows, against the
  baseline of processor time, wake-ups and change-to-speech time taken on
  2026-10-23 on the build before the reading changes (runs under
  `target/perf/terminal-baseline-*`; the memory baseline in
  [performance](../books/dev/src/reference/performance.md), "Memory use").
- [The terminals parity entry](parity/terminals.md), the crates'
  documents and these entries brought in line with the model.
- The whole terminal suite, `cargo xtask ci`, and the push.

Related items, each to be settled within the redesign or after it:

- Provisional (2026-10-11): the console host raises no
  text change for a write that leaves the cursor where it was, as a full
  history scrolling whole lines may (the model's "The terminals"); if a
  measurement on the installed build confirms it, follow NVDA, which has
  the same gap.
- An unsettled terminal read with nothing queued behind it is never
  retried; `conhost_leave_flood` once heard no "blank" for an empty new
  console window ([the known failures](flaky-tests.md)).
- Windows Terminal sometimes raises no TextChanged event for a typed
  character ([the known failures](flaky-tests.md)); settle
  whether Windows Terminal or UIA loses the event, with about 60 runs and
  a second, independent UIA client listening. The model's reading on
  caret events (its "Reading rules", rule 1) comes back with the outpost
  and Core step (Dickson, 2026-10-17).
- `windows_terminal_two_windows` may say "<title> terminal" two or three
  times, from Windows Terminal's second runtime identity (Dickson,
  2026-10-19; [UIA faults](../books/dev/src/reference/uia-bugs.md), "Windows Terminal raises focus
  from a second runtime identity"); the rule is M5's.
- The console host's say-all repeats one row: its text pattern clamps an
  expansion below the last row with text back onto that row, and nothing
  checks that the expanded line contains its point. Proposed fix: take a
  unit that does not contain its point as the end of the text, one
  comparison inside the existing remote program, judged by a fake-source
  unit test, a mockapp provider whose expansion clamps, and a console host
  scenario step from a fresh NVDA capture.
- A lost read was seen to ask twice for the whole document, where one
  whole-document read should answer it.
- "hellook1" is heard in the recorded hand streams.
- Windows Terminal's launch focus sometimes speaks "... terminal" without
  "... window", and `conhost_typing` misses a typed "o" sometimes
  ([the known failures](flaky-tests.md)).
- The reader's fix for a caret ahead of the typing covers only a line
  showing none of the pending typing: with "a" and "b" typed, a line
  showing "a" with the caret one cell ahead gives Gained "a" and then
  Changed "ab".
- Enter that erases a grey suggestion speaks the input line again.
- Up Arrow and then "e" typed before the recalled line is read gives Echo
  "e" and then Gained "cho one".
- Backspace erasing "o", Control+Backspace erasing "tw", and tab
  completion have no exact reader tests.
- A trimmed caret line taller than the kept screen could miscount the
  caret's column along its line.
- The reader ignores the colours of a colours re-read that comes late.
- The outpost takes a read's `ended_ms` after converting its text from
  UTF-16, so the conversion's time counts as part of the read.
- A review command's history read is placed with gaps in its checks: its
  answer carries no `from_start`, so a start the outpost refused during
  the read is placed as if counted from the read it names; an
  `Unanswered` history read is placed as no rows above the start; and a
  whole-document history read is placed by the end of the memory's rows
  rather than by a start, so more than a screen of unshown output printed
  during the round trip shifts every placed row (ReviewTop then
  ReviewNextLine read the wrong line). The outpost could count a whole
  read's rows from its newest start too, so Core places it by start.

Follow-ups:

- Root-cause, from the GitHub run logs, the hosted end-to-end `windows_terminal_two_windows` and `windows_terminal_leave_flood` failures on main before 2026-10-15; neither reproduces here.
- Repeat `line_key_flood` on GitHub's runner, where it once read a half-written line (deferred, Dickson, 2026-10-21).
- The cleanup of a stuck flood console, which once outlived its scenario until the agent was force-killed, is not tested.
- An outpost crash inside UIA's saved-event delivery (`ProtobufHelper::GetCacheResponse` reading a null element reference on a UIA worker thread): with the focus pipeline's section 17 built, `conhost_leave_flood`'s console host outpost failed fast again; read its full dump, kept outside the repository, and make section 17's fifty-run check.
- A console window read through MSAA is "pane" where NVDA gives its client object the legacy console overlay with the terminal role (`nvda/source/NVDAObjects/IAccessible/__init__.py`, lines 731 to 733, and `IAccessible/winConsole.py`; deferred, Dickson, 2026-10-21).
- A console whose provider's formatting check gets no answer is probed and checked twice in one arbitration entry, through the `None` branch of `decide` in `crates/verbatim-outpost/src/arbitration.rs`.

Exit: Terminal sessions are solid; terminal flood E2E shows bounded
latency and no hang.

## M6.5 — Text residuals

The text work left after M4's caret and selection rework, gathered in
one milestone (Dickson, 2026-10-29).

Formatting and the rich edit control:

- A rich edit control read through its own object model
  (`ITextDocument`), as NVDA reads a rich edit control whenever it can
  get that model (`ITextDocumentTextInfo`,
  `nvda/source/NVDAObjects/window/edit.py` 1041 to 1045): its own
  paragraphs, so that Control+Up and Control+Down Arrow speak the
  paragraph rather than the wrapped line, its own sentences for Alt+Up
  and Alt+Down and say-all, its embedded objects named, its paragraph
  alignment and its links. Verbatim reads a rich edit control through
  window messages, its paragraph unit being its line as shown
  (`docs/parity/text.md`, "Rich edit paragraphs").
- Formatting in classic edit controls, plain and rich, as NVDA reads it;
  they read none today.
- Formatting in review commands and say-all, which read none today
  (`books/dev/src/reference/crates/verbatim-core.md`, "Formatting").

Text and selection:

- Settle where NVDA's say-all starts from NVDA's speech log before any say-all expectation changes: in every capture of 2026-10-23 NVDA's recorded speech began at the line after the caret's, in Notepad and in the text box, which is either this NVDA build or the capture add-on missing the first chunk.
- Notepad's Find box differs from NVDA's capture: Control+F says Notepad's notification "Press F6 to go to new notification" first and "Find edit" and "selected beta" as two utterances, where NVDA says "Popup window", then "Find edit selected beta" as one, then the notification; settle it with a capture that does not depend on the text Notepad remembers in the box.
- The order of a selection change's parts: NVDA speaks every text selected, then every text unselected (`speakSelectionChange`, `nvda/source/speech/speech.py` 1367 to 1417), where Verbatim says them in text order, so a replaced selection and one crossing its anchor are spoken the other way round; no capture has the replaced selection yet.
- NVDA's classic edit control drops a caret event whose selection offsets equal the last one's (`NVDAObjects/window/edit.py` 1068 to 1075); Verbatim reads each caret event, and a caret event a read already covered clears the content-changed note whether or not the offsets changed (`docs/parity/text.md`, "Not yet built").
- The remote caret program could leave out its two comparisons of the old and new selections' ends when the old selection is known at build time to hold nothing (`previous_collapsed`, `crates/verbatim-uia-rops/src/caret.rs`), since that case is a whole change already.
- The caret read made with a focus chooses remote or classic reading by the previous focus's window, since the outpost's window tracking is updated after the focus is published; only the read's cost differs.

Typing and keys:

- Verbatim echoes a typed character from its keyboard hook, before the application handles the key, where NVDA echoes from the application's typed-character event; a letter typed within about 40 ms of Tab is not echoed (Dickson, 2026-10-10, for a later milestone).
- Key holding's gaps: no test or scenario of a word read in a rich edit control; `Modifiers::after` keeps one flag for left and right Shift, Control and Alt; a key whose release is never seen stays in the decision machine's record of the keys down; and `call_counts.rs` pins the read before a held key by copying the production code's arguments rather than calling it.

Exit: each entry is matched (verified) by a scenario or a test, with a
declared deviation where Dickson chose one.

## M6.6 — Security fixes

After M6 (Dickson, 2026-10-31). Developer mode, and the fixes from [the security model](design/security-model.md) that need no milestone of their own; the model's mitigation ids are named in brackets, and each item names the test that proves it. Built as a few large builds with one design and review each.

- Developer mode: said at startup, never reaching secure screens; the control protocol's two endpoints, the developer one only in developer mode (C1, C1a); the extension developer folder and debug-level logging only in developer mode (C7, L3). Test: with developer mode off, every developer method is refused and the developer folder is not watched.
- The test agent bound to loopback with a deploy-time token, and the VM's firewall rule scoped to the host's switch (C4); it now listens on every interface with no authentication. Test: a connection from another machine, and one without the token, are refused.
- The control pipe named per session and user, a pipe failure that never stops startup, and clients that check the server they reach (C2's pipe half; its startup mutex and replacement event are M4.8's). Test: Verbatim starts and speaks while another session holds the old pipe name and while a Low-integrity process holds every pipe name it can create, and a client refuses a server that is not Verbatim.
- Size limits on the control pipe's and configuration's readers, and no fallback to `%TEMP%\verbatim-defaults` or the current directory (C3). Test: an oversized line and an oversized or malformed settings file each give the stated outcome.
- Keys injected from a lower integrity level never trigger gestures and are never held or re-sent (C5). Test: a gesture injected from Low integrity does nothing, and one injected at the user's level runs.
- Theme import's name check refusing reserved device names and names ending in a dot or space, and writing relative to the staging folder's handle (X13). Test: a corpus of packages with `..`, drives, reserved names, trailing dots, alternate streams and a planted junction writes nothing outside the theme's folder.
- Characters typed into protected fields recorded as stars (L2). Test: after a scenario types known text into a password field and a forced native crash, no log, dump or crash file holds the text.
- COM unmarshalling hardened with `EOAC_NO_CUSTOM_MARSHAL`, and the write-restricted token for outposts, which must also stop an outpost writing into the user's other processes, tested together against UIA, MSAA and a remote operation (X7, section 6.9). Test: `mockapp` and Notepad scenarios pass under both, and an object naming a custom unmarshaler is refused.
- An application named by a handle rather than a process id, a bound on outposts, and the background flood cap (U7 to U9). Test: the hostile `mockapp` mode of M4.9 gains a notification flood and a provider that hangs or crashes mid-call, each asserting the exact speech, that another application keeps speaking within a stated time, and that memory stays under its cap.
- The per-process mitigations of [the security model](design/security-model.md), section 6.9, applied at each child's start: control-flow guard, strict handle checks, the image load policy, the extension hosts' job limits, and control-flow guard and shadow stacks at link time; never the Microsoft-only image policy for synthesizer hosts or outposts, and no extension-point policy for Core while the GUI is in Core, since NVDA's helper must read Verbatim's GUI for parity captures. Test: each process's mitigations, read back from it, equal its stated list on x64 and ARM64, and the `mockapp` and Notepad scenarios pass under them.
- Pinned DLL search (X2), moved in from M8.6: each executable calls `SetDefaultDllDirectories` with the system and application folders before anything else loads. Test: a DLL planted beside a portable copy is not loaded.
- Synthesizer escapes covering control and NUL characters (J1). Test: text with SSML, NULs and control characters is spoken as text by both built-in drivers.
- `cargo deny` in `cargo xtask ci` (X10).

Exit: every item above is built with its test passing, and wake-ups and processor time are measured before and after each build.

Security work other milestones carry, each with its test:

- M5: the extension surface's rules ([extensions](design/extensions.md), section 4), each with a test, every navigation import at every boundary included; installed extensions in hosts of their own, with the round trip measured (X4), and a probe inside the host whose every attempt to write outside its data folder, follow a planted junction or hard link out of it, open a socket without importing a network interface, start a process or read the clipboard fails; the breaker (A9); compiled code mapped only from where ordinary processes cannot write (X5); starting with every extension off (A8); the user endpoint taking requests and never commands, one install dialog at a time (C1, C9), tested by a second request refused while the dialog is open; extension installation that writes only inside its own folder (X13), tested by the malicious package corpus.
- M6: the synthesizer container, confirmed before any native code loads, read-only, limited at creation, with the host's messages checked (X6), tested by a probe child asserting every denial on x64 and ARM64 and a host started with the wrong token loading nothing; the synthesizer fallback after repeated crashes and the idle-spin check (A3, A7), tested by an engine that crashes on a string and one that spins; x86 emulation in the container measured.
- M6.3: speech moved to another audio device while the default refuses Verbatim (A5), tested with the default held in exclusive mode; a cut string known to speech (U4).
- M6.5: typed characters judged against the focus they were typed in (L9), tested by a password field that closes on its last character.
- M7: the helper loaded narrowly, handed its channel, version-checked and treated as hostile (X8), tested with a mismatched helper and malformed messages; link destinations (S3).
- M8.6, with the installer (the screen curtain with M8.4, and linear-time dictionaries, direction overrides and clipboard history with M8.2): a machine-wide, signed, administrator-owned installation (X1); signed updates from a process of their own (X9), tested with an unsigned and a wrongly signed download; never running elevated (B2); diagnostics' home and limits (L4); the screen curtain built for a user who cannot see it (L8), tested across a display change, unlocking and a restart; linear-time dictionaries (J2); direction overrides spoken (S2); clipboard history (L7).
- M4.7 and M4.8: native crashes dumped by Verbatim, never by Windows Error Reporting (L5); Core restarted by the guardian, with a breaker whose last restart runs without extensions (A1); Core's memory cap (A2); the single instance in a private namespace and the `WM_QUIT` filter removed (C2's startup names, C6).
- M4.9: walks, lists and strings bounded (U2 to U4), terminal alignment below quadratic time (U5), and fuzzing at the parsers (X12).
- M8.7: the UIAccess measurement first (B1), then the lock and secure-screen rules (B3 to B8) with the secure instance's menu and dialogs as an allowlist, no file choice, and the toolkit's dialogs routed to the log (G1, G2, G4), tested by a list of every window and command the secure instance can reach equalling the allowlist, and by each of NVDA's secure-screen advisories as a scenario; the administrator's copy for secure screens following no links (X13), tested with a planted junction; developer mode in UIAccess builds turned on only in Verbatim's own window (C1); the secure instance's children started without SYSTEM (B10), tested by reading their tokens; "Secure Desktop" announced on the switch (B11); and the secure instance's own curtain.
- M9: no content in logs at default levels (L3), tested by scanning a full suite run's logs for the planted text.
- M12: Remote's end-to-end encryption, announcements and lock behaviour (C8, B9), tested with a relay that tries to read and alter traffic and a session that locks.
- M18: braille drivers in their extensions' hosts, given only the devices Core opens for them (section 6.11), tested with a device sending malformed reports.

## M7 — Browse mode, browsers, and the injection helper

- Document projection / incremental virtual buffer over the normalized tree;
  quick-nav keys, elements list (presented through the M3 list dialog),
  switching between focus and browse modes. Where the buffer lives is
  decided here: in Core, or in the outpost or the injection helper with
  Core querying it, as NVDA keeps its buffers inside the browser's
  process. Either way Core's state stays cheap to clone (agreed
  2026-10-06): a buffer held in Core keeps its text in a rope and its
  nodes in a persistent map, both sharing memory when cloned, so the
  flight recorder's checkpoints stay affordable. Starting points: `imbl`
  for the map, whose MPL-2.0 licence is checked against the
  platform-neutral tier first, and `ropey` or `crop` for the rope (Dickson,
  2026-10-31).
- IA2 lands here, deferred from M3 because browsers are what motivate it:
  interface acquisition (`IServiceProvider::QueryService` from the
  WinEvent's `IAccessible` to `IAccessible2` and the IA2 text, hypertext,
  and relation interfaces), the proxy/stub marshaling story from Rust, and
  IA2 roles and states preferred over MSAA in the normalized mapping.
  `mockapp`'s MSAA mode grows an IA2 answering path so the client stack is
  testable cross-process without a browser.
- Firefox and Chromium (Edge/Chrome) via IA2 as the primary source, with a
  UIA comparison where relevant.
- The D2 injection helper lands here, staged inside the milestone:
  out-of-process IA2 first, proving correctness, then the in-process helper
  for performance — IA2 call batching and virtual-buffer acceleration —
  including the x64/ARM64EC/x86 helper matrix and antivirus/signing
  considerations. NVDA has already proven that in-process access is what
  makes browsing fast enough, so this is scheduled work rather than a
  measure-first gate; the fixed-page-corpus measurement against NVDA
  remains as this milestone's exit verification.
- Screen review as a spatial projection of the normalized tree (D11):
  visible nodes ordered by bounding rectangle, grouped into visual lines,
  walked by the review cursor by line, word, and character. Extent-backed
  wherever a text interface exists — UIA TextPattern bounding rectangles
  and IA2 character extents give exact per-character geometry, batched
  through the new helper — with rectangle interpolation as the fallback for
  name-plus-rectangle elements. OCR joins as a second text source in M8.5 and
  the display model as a third in M17; the review commands and cursor stay
  the same throughout, only the source improves underneath.
- Typed-character echo of composed text from the D2 injection helper, as
  NVDA echoes what the application receives: Windows' text-service
  keyboards outside Chinese, Japanese and Korean (Vietnamese Telex and
  VNI, the Indic Phonetic keyboards) compose characters the keyboard hook
  cannot see. Until then Verbatim stays silent while such a keyboard is
  active rather than echoing raw keys (decided 2026-10-08).
- IAccessible2 unique ids as identities: NVDA uses them to recognize an
  IA2 object again when it is named by an event, so a state change on an
  IA2 focus ancestor can be spoken. Plain MSAA objects named by events are
  oleacc wrappers whose COM identity never matches an object reached
  through `accParent` (found 2026-10-08, `books/dev/src/reference/crates/verbatim-ia2.md`,
  "Identity"), so without such ids neither NVDA nor Verbatim speaks those.
- Interaction-before-full-render E2E on a very large page.
- Scan-mode generalization: the same projection over an ordinary app.
- Start-menu search results: the results are a Chromium (WebView2)
  document inside the search host, which NVDA reads through IA2
  (`appModules/searchui.py`, `StartChromiumObj`); once IA2 lands, a
  Start-menu search scenario types a query
  and arrows through the results (NVDA's reading, captured on 2026-10-06: "Notepad, App, Press
  right to view options 1 of 5" after the first letter, each result with
  its position as the arrows move).
- From NVDA's changes up to 663e4b679 (reviewed 2026-10-06): Alt+Up and
  Alt+Down collapse or expand the control at the caret, or move by
  sentence, depending on the control (`browseMode.py`); focusing a
  `role=application` element announces the labels of the regions and
  groupings it enters; a native selection mode setting; quick navigation
  to clickable elements; a history in the Find dialog; virtual buffer
  attributes whose names are not valid XML are dropped.

Exit: real browsing works day-to-day; large-page E2E passes; corpus
performance/parity report vs NVDA written, at (or consciously accepted
near) parity; screen review reads a modern app's screen correctly.

## M8.1 — Input handling

No exit criterion of its own: the milestone is done when all of its items
are.

- Layered commands (Dickson, 2026-11-02): a command layer in the input
  crate's decision machine, active after its prefix key until one of its
  commands runs or Escape is pressed, named by input help; extensions
  declare layers as data from M13. 24 NVDA add-ons use one; the gesture
  map published at a version arrives in M5.
- Input help mode; the gesture-remapping configuration GUI, where
  changing a binding keeps the binding a module declared, so its
  `trigger` still names it, and the install dialog's list of an
  extension's conflicting gestures offers to rebind each (Dickson,
  2026-11-09).
- Modifiers left held after the secure desktop, since their releases
  never reach the hook (NVDA clears its modifier record when the secure
  desktop opens; the signal of a desktop switch is designed in the focus
  pipeline rework's third build). From the NVDA input audit of
  2026-11-03.
- A beep for a lowercase letter typed with Caps Lock on, as NVDA does by
  default, settled with an NVDA capture (input audit, 2026-11-03).
- Sticky Keys (input audit, 2026-11-03).
- Key names Verbatim lacks: F13 to F24, media keys, numpad digits with
  Num Lock on, and punctuation other than period and plus (input audit,
  2026-11-03).
- Bound keys are still swallowed while Core's reducer stalls, where
  NVDA's watchdog lets keys through (input audit, 2026-11-03).

## M8.2 — Speech configurability

No exit criterion of its own: the milestone is done when all of its items
are.

- Automatic language switching, as NVDA does it: the language of each
  run of text (UIA's Culture per format run, IA2 text attributes) carried
  through the text reads, Core and speech, and each run spoken in its
  language's voice by every synthesizer, with a setting to turn it off.
  Today the language is read but used only for word segmentation, and
  OneCore's SSML always uses the current voice's language (language audit,
  2026-10-08). Terminal output switches language only where the terminal
  reports a language per line (decided 2026-10-07).
- Full pronunciation/symbol dictionaries and their configuration UI (the
  data-driven infrastructure exists from M3/M4). The built-in
  dictionary's letter-to-digit rule, a space wherever a digit follows a
  letter, is built with them, after checking whether eSpeak NG and
  OneCore still need it ([speech and audio parity](parity/speech-audio.md),
  "Letter-to-digit splitting"; Dickson, 2026-10-15). With punctuation and
  symbol levels in place, decide how terminal output's symbol-only lines
  (vim's "~" rows, box-drawing rules) are spoken: M4 speaks them as they
  are, and they should then follow the user's symbol level, as NVDA's
  are silent at its default level (Dickson, 2026-10-08). The compare
  check once symbol levels exist: at
  `windows_terminal_commands`'s first prompt NVDA says "ready greater"
  at its default symbol level ("some"), where Verbatim leaves the ">"
  out (Dickson, 2026-10-21).
- Speech history (Dickson, 2026-11-02): reviewing recent utterances,
  moving back and forward through them and copying one, beside copying
  the last spoken text below; extensions read it from M13.
- From NVDA's changes up to 663e4b679 (reviewed 2026-10-06): copying the
  last spoken text to the clipboard (NVDA+Control+X) and a controller
  call reporting whether speech is in progress; dictionary rules written
  as zero-width lookarounds, which avoid catastrophic backtracking (NVDA's
  rewrite of its camel-case and word-to-digit rules in `builtin.dic`).

## M8.3 — Profiles and localization

No exit criterion of its own: the milestone is done when all of its items
are.

- Configuration profiles (manual and triggered); localized UI shipped in at
  least two languages as proof.
- Generated tests for loading and applying settings, over arbitrary files
  and values: no panic, and every value used, clamped or refused with a
  warning (Dickson, 2026-11-05), since the saved settings are read on every
  start, the guardian's last restart included.

## M8.4 — Overlays

No exit criterion of its own: the milestone is done when all of its items
are.

- Focus highlight (DirectComposition overlay) and screen curtain (R6 check);
  overlay component designed to host a future magnifier.
- From NVDA's changes up to 663e4b679 (reviewed 2026-10-06): menu item
  locations in 32-bit applications converted from the 96-DPI space the
  OLEACC menu proxy reports when the client's bitness differs (needed
  once locations are used, by the focus highlight or the mouse).

## M8.5 — OCR

No exit criterion of its own: the milestone is done when all of its items
are.

- OCR capability (Windows.Media.Ocr) exposed to extensions; synthetic-subtree
  review of an image/window; OCR becomes screen review's second text source,
  for windows whose pixels contain text that no API exposes.
- From NVDA's changes up to 663e4b679 (reviewed 2026-10-06): OCR captured
  through Windows Graphics Capture.

## M8.6 — Installer and updater

No exit criterion of its own: the milestone is done when all of its items
are.

- Installer/updater skeleton.
- The installed copy (Dickson, 2026-11-09), by the layout M5 left ready
  for it: the configuration root resolved to the user's profile for an
  installed copy ([security model](design/security-model.md), section
  4.1), so settings, profiles, gestures, installed extensions and the
  developer folder are per user, and compiled code, logs and dumps go to
  the user's local application data (L4); built-ins and their compiled
  code stay in the administrator-owned installation, the same for every
  user (X5). NVDA keeps its user configuration in roaming application
  data, or local with an installer option, and a portable copy's beside
  it (`config/__init__.py`, `getInstalledUserConfigPath` and
  `getUserDefaultConfigPath`); which of roaming and local Verbatim uses is
  decided here.
- The `.vext` file type registered machine-wide to the helper, as NVDA's
  installer registers `.nvda-addon` to `nvda_slave.exe`
  (`installer.py`, `registerAddonFileAssociation`).

## M8.7 — UIAccess: elevated windows, UAC and sign-in

Reading elevated console windows, UAC prompts and the lock screen or
sign-in needs the UIAccess permission; this milestone builds it and reads
each of them end to end.

- Secure-desktop instance (`--secure`), AT registration, UIAccess/test-signing
  story in the VM (R5).
- Remove the keyboard hook's guard that holds no caret key while a
  window of higher integrity is in front
  (Core's `SrState::hook_foreground`, from the integrity level the focus
  listener reports for the window in front, published in the hook's
  `ForegroundView`): with UIAccess, keys Verbatim
  sends on reach elevated windows, so they are held there too
  ([holding caret keys](../books/dev/src/explanation/design/key-holding.md), "Elevated windows";
  Dickson, 2026-10-22).
- In the keyboard decision machine, a release whose key's press still
  waits in the line waits behind it whatever the permit, so a key
  released as a window Verbatim cannot hold keys in comes to the front
  is not left down; with UIAccess no send Verbatim makes is dropped, so
  the rule holds in every window ([verbatim-input](../books/dev/src/reference/crates/verbatim-input.md);
  Dickson, 2026-11-01).
- The secure desktop and the lock screen wait for UIAccess support,
  without which Verbatim cannot read the secure desktop (Dickson,
  2026-10-12).
- An elevated console window is named "Administrator: <title>" or just
  "<title>" depending on when it is first read (seen in a console host
  flood scenario on GitHub's elevated runner). Capture what NVDA says for
  an elevated console window, which needs an elevation prompt Dickson
  approves, then make Verbatim read one name consistently.

Exit: a UAC prompt, an elevated console, and the sign-in screen are read
in the VM.

## M9 — Logging and log viewer

Proper user-facing observability, distinct from (and built on) the
developer-facing flight recorder and tracing spans. M5 brings forward the
minimal pieces its modules need: the in-memory ring, a log viewer window
and the Verbatim+F1 focus report; this milestone finishes the story.

- User-facing log levels and categories; logging to file on top of the
  ring; the existing tracing spans, collected stderr, and panic dumps
  absorbed into one coherent story.
- The log viewer window grows from M5's minimal viewer into one readable
  with Verbatim at every level and category — the viewer is its own
  dogfooding test.
- The commands NVDA users expect around it: open the log viewer, report the
  most recent error, cycle the log level at runtime.

Exit: a user can reproduce a bug, open the log viewer, and read what
happened, without touching developer tooling.

## M10 — Extension console and scripted modules

The equivalent of NVDA's Python console for extension development, and
the scripted module kind, both deferred from M5 (Dickson, 2026-10-22)
because a script engine compiled to Wasm costs every outpost that hosts
it. Until then M5's call console in `verbatim-inspect`, which calls a
module's hooks and tests rules against a live Verbatim, is the smoke test.

Python is an obvious fit for NVDA; with Wasm extensions the console is
instead an interactive interpreter that is itself an extension: an
interpreter compiled to Wasm, granted broad capabilities, evaluating
against exactly the `verbatim:ext` WIT API every extension uses. No
privileged side-channel API exists — if the console can do it, an
extension can — so the console doubles as a standing test that the API is
ergonomic enough for exploratory work. Convenience aliases in the NVDA
console style (the focused object, the navigator object) are pre-bound
bindings over the same calls, never separate host functions. A scripted
module is the same interpreter loaded with a module's script, with
TypeScript by type erasure, and sits in a module folder exactly as a
component does.

Design questions settled inside this milestone: which interpreter (QuickJS,
RustPython, or similar, compiled to a component), how epoch preemption
interacts with long-running evaluations, and whether the console UI lives
in the Verbatim GUI, in `verbatim-inspect`, or both.

Exit: from the console, inspect the focused object, walk its tree, speak,
and bind a gesture — against a live Verbatim, without restarting anything.

## M11 — Audio formatting: earcons and voice styling

The payoff for D12's structured utterances: presentation themes that map
semantic spans to sound, in the tradition of Emacspeak's audio formatting
and the audio-themes family of NVDA add-ons.

- A theme maps span semantics to presentation: a role can become an earcon
  plus shorter speech (a slider sound and "pitch 50" instead of "Pitch
  rate slider 50"); formatting attributes on text runs can become sounds
  (a spelling or syntax error under the cursor plays a sound rather than
  being spoken); capitals, quotes, and emphasis can become pitch or voice
  changes.
- The default theme, built in M4, matches NVDA's defaults: everything is
  spoken as NVDA speaks it, and sounds play where NVDA plays them by
  default, plus Verbatim's own cues and the error sound for a spelling
  error (decided 2026-10-07). A theme without sounds is added when
  braille arrives. Switching themes is a runtime configuration change, no
  restart.
- Themes are data, and eventually extension-provided packages — giving the
  porting track a consumer in the audio-themes add-on family.

Exit: an earcon theme ships alongside the default theme; a scripted E2E
hears the slider earcon and the spelling-error sound; the default
theme's output is unchanged from before M11.

## M12 — Remote support

- Pairing/auth UX on the control plane; speech mirroring out, input in;
  secure-desktop and permission rules applied to remote sessions.
- The TLS WebSocket transport starting from the synchronous `tungstenite`
  and `rustls` on a thread of their own, handing messages to the rest of
  Verbatim as the control plane does, with no async runtime
  (Dickson, 2026-10-31).
- Remote Desktop, Citrix and similar products (Dickson, 2026-11-02): a
  channel plug-in, built into Verbatim, that the product's client loads and
  that carries the remote protocol over the product's own virtual channel,
  so a Verbatim inside a remote session speaks through the local Verbatim
  with no relay or pairing, as NVDA's rdAccess add-on does. The plug-in
  runs inside the other vendor's client process, a trust boundary its
  design states, and registering it for Citrix needs administrator rights.

Exit: control a second Verbatim instance (between the VM and the host)
end-to-end.

## M13 — Extensions v2

The extension surface grown from a survey of NVDA's add-ons, once M5 has
proven the surface on the built-in app modules, which is M5's goal
(Dickson, 2026-11-02). Every item has add-ons that need it, as the
extension API growth rule asks.

- Acting in the module's own application, bounded by the application
  boundary of the extensions design (section 4) and allowed only while
  the application holds attention: node actions (invoke, focus, select,
  expand or collapse, toggle, scroll, and named accessible actions);
  sending keys and inserting text, typed by Core so no extension holds an
  input-injection handle; prompts shown by Core; commands scoped to a
  node; silencing the focus moves a module's own command causes; synthetic
  nodes over the module's own window; the module's own top-level windows;
  toast notification actions performed by Core.
- Text and events: text ranges by unit with format fields, caret and
  selection; a text provider hook; caret-key declarations; typed-character
  and caret events to `on-event`; a controller speech hook for the
  module's application; declared read-only window messages to the
  module's own application.
- Output: packaged sounds, tone sequences with pan, a capped PCM lane, and
  saying all of a node's subtree (speaking a node arrives in M5).
- Module settings shown by Core, a secret store outside the settings file,
  shipped with the network imports, and the data folder each extension
  reads and writes, with its size cap
  ([security model](design/security-model.md), X14).
- The deferred imports: a timer with a minimum interval that fires only
  while the module's application holds attention; the network imports
  (HTTP with every method and streamed bodies, and WebSockets) to named
  hosts or a host the user enters, in the extension's own host, with
  loopback reachability from the container measured (X15), and re-consent
  on an update that adds a flow (a new exit, host or device, or a new
  source in an extension that has an exit); clipboard reads and a
  clipboard-changed event from Windows' clipboard
  listener, never polled; the mouse beyond M5's click; global modules
  reading every application; reading speech history, shown at install as receiving all the
  text Verbatim speaks.
- The speech-text stage module kind (Dickson, 2026-11-02): an ordered chain
  in Core's speech queue, each stage a module in its extension's host, on a lane of its own,
  that takes an utterance's text segments and language tags and returns new
  ones, for translation, untagged-language detection and text rewrites.
  The original text is never spoken: a stage that fails or misses its
  deadline gives the failure indication it declares, "Translation failed"
  for a translator. A stage reads its own folder, and the package limit
  admits language models (Dialingo's is 16.5 MiB). The deadline per stage
  and the latency budget are set in its design.
- The recogniser provider module kind (Dickson, 2026-11-02): Core captures
  the navigator object, a window or the screen through Windows Graphics
  Capture, which works under the screen curtain, and hands only the image
  to the module, which returns text, word boxes or a description. Its
  design settles how a recogniser that sends images to a service is
  consented to, and an inference import with which Verbatim runs a local
  model on the GPU while the module stays Wasm, its runtime chosen as a
  major dependency.

Out by decision (Dickson, 2026-11-02): a channel from components inside
applications to extensions. Much later: licence activation for engines
that need it, under the extension host's mechanisms.

Exit: every item above is used by at least one ported add-on or built-in,
each boundary rule has its test, and the call ledger shows no cost for a
module that imports none of them.

## M14 — Maths

MathML and LaTeX reading and navigation built into Verbatim, after M13
(Dickson, 2026-11-02). MathCAT, a Rust library NVDA uses, is the candidate
engine; adopting it is a major-dependency decision taken with this
milestone's design. Wording in other languages, such as Greek, comes as
rule sets.

Exit: MathML in a browser document is spoken and navigable by
subexpression.

## M15 — Positional sound

Sounds placed in space (Dickson, 2026-11-02), after maths (M14):
the mixer plays a sound at a position, so an earcon can say where an
object is on the screen or where it sits in a list, as NVDA's Unspoken and
audio-theme add-ons do. A theme names a position as an attribute of an
indication's sound, and extensions get positioned playback through M13's
packaged sounds.

Exit: an earcon theme with positions ships, and a scripted scenario hears
an object's sound panned by its place on the screen.

## M16 — Java Access Bridge

JAB is a committed backend, deliberately last among the accessibility APIs
(D1): far fewer apps need it than UIA and MSAA/IA2.

- `verbatim-jab` client stack: the WindowsAccessBridge-64 C API and its
  event callbacks, running on outpost threads like the other backends,
  mapped into the normalized model behind the same arbitration.
- A small Java Swing fixture app joins `mockapp` duty for provider-level
  tests; E2E against a current Java IDE or LibreOffice-adjacent Java app.
- Unblocks porting-track Tier E (javaw, Eclipse).

Exit: a mainstream Swing application is readable and navigable end-to-end.

## M17 — Display model (gated)

A GDI display model in the NVDA tradition, deliberately last among the
text sources (D11), and opened by an explicit re-triage gate: test the
then-current PuTTY, SecureCRT, and Tera Term (and any other app that has
motivated this milestone by then) against stock Verbatim plus OCR, and
build only if the display model still earns its maintenance cost. By this
point everything it needs already exists, which is what keeps the
milestone cheap: the M7 injection helper is the delivery vehicle (the
display model is its second in-process client), synthetic nodes make its
output first-class model content, and the M4 diff announcer and flood
policy handle live terminal output.

- GDI text-output hooks (`ExtTextOutW` and family) in the injection helper;
  a per-window text model (chunk rectangles, baseline ordering,
  invalidation on redraw).
- A live-text diff source over that model — the `DisplayModelLiveText`
  equivalent — feeding the same announcement pipeline as Windows Terminal.
- Screen review gains the display model as its third, pixel-faithful text
  source for GDI apps; text-under-mouse works in apps with no text API.
- Unblocks the porting track's legacy terminal clients (PuTTY, SecureCRT,
  Tera Term).

Exit: the re-triage decision is recorded; if built, a PuTTY session is
readable with live output announcement and the existing screen-review
commands, with no fidelity regression anywhere else.

## M18 — Braille (deliberately last, D7)

- liblouis integration; `BrailleDisplay` trait implementations for common
  displays; braille viewer (on-screen virtual display) so development and
  E2E tests need no hardware; routing keys, cursor tethering, speech-braille
  sync via existing index marks.
- Drivers are extensions in their own hosts, and Core opens the devices
  their manifests name or the port the user picks
  ([security model](design/security-model.md), section 6.11). Vendor
  libraries that open the device themselves, Bluetooth LE and BRLTTY's
  pipe are made to work under those mechanisms where they can.
- A braille display's gestures are user inputs that are not keys, so
  they need key numbers ordered against the keyboard's: the keyboard
  hook's thread is the one minter and takes only keys
  (`books/dev/src/explanation/design/ownership.md`), so M18's design
  decides how a braille gesture is numbered, as NVDA queues its speech
  cancel behind what is already queued.

Exit: braille viewer E2E scenarios pass; at least one hardware display
verified when hardware is available.

## The app-module porting track (runs from M5 onward)

Goal: eventually port all of NVDA's built-in app modules
(`nvda/source/appModules`, roughly 80) except those for software that no
longer exists or is unmaintained — Skype, Lync, MSN Messenger, Outlook
Express, Windows Live Mail, Winamp, Instantbird, Miranda, Lotus
Notes/Symphony, the legacy EdgeHTML modules, and similar; each skip is a
recorded case-by-case call.

Method: triage before porting. Many NVDA modules exist to patch quirks of
NVDA's pipeline or of APIs at the time they were written; for each module,
first test the app against Verbatim's stock UIA/IA2 handling and port only
the behavior that is still needed. Every port doubles as a test of the
extension API (per the API-growth rule: no host-API additions without a
consumer).

Tiers, gated by the capabilities each module needs:

- **Tier A — immediately after M5** (needs only M5's surface: rules,
  event and node hooks, speech, commands): the utility modules beyond the
  Windows inbox applications, which are M5's own built-ins — Notepad++,
  Poedit, Spotify, foobar2000, Audacity, 1Password, basic Zoom and Teams
  behavior. Notepad++'s support, the
  first Scintilla editor, takes with it NVDA's per-class rule that a
  Scintilla editor's caret shows and hides are no caret event
  (`shouldAcceptShowHideCaretEvent`, `NVDAObjects/IAccessible/scintilla.py`,
  line 15, tested in `IAccessibleHandler/__init__.py`, lines 1039 to
  1055), since its caret blinks by them; until then a focused Scintilla
  editor tries to read its caret at each blink.
- **Lock screen and logon UI — after M8.7**: reading the sign-in screen
  needs UIAccess, which M8.7 builds.
- **Tier B — the terminal clients, in two stages**: Windows Terminal
  workflows are core M4 behavior, and mintty is triaged on its own after M4
  (its ConPTY integration may make it readable stock). PuTTY, SecureCRT,
  and Tera Term draw their screens through GDI with no accessibility API at
  all, so they are gated on the M17 display model — with ssh from Windows
  Terminal as the recommended interim answer.
- **Tier C — after M7 browse mode** (web-content-hosting apps): WebView2
  hosts, VS Code, WhatsApp, deeper Teams support, Thunderbird, Kindle and
  other readers.
- **Tier D — after the object-model bridge capability lands (planned around
  M8.5)**: Word, Excel, PowerPoint, Outlook, LibreOffice, Visual Studio.
  NVDA's Office modules rely heavily on COM object-model automation, which
  Wasm extensions cannot reach directly; this tier is gated on designing a
  capability-gated app object-model bridge in the WIT host API — the
  largest single API-growth item in the plan, and worth its own design pass
  for both surface and security. Word's support takes with it NVDA's
  rule that the system caret's events are no caret event on Word's UIA
  document, which Chinese input methods flood with them
  (`IAccessibleHandler/__init__.py`, lines 641 to 657); Verbatim handles
  them as a caret event on every UIA focus until then.
- **Tier E — after the JAB backend lands (M16)**: javaw and Eclipse.

## Ongoing tracks (every milestone)

- **Latency**: the end-to-end suite records each step's response time
  and asserts none. Enforcing the response-time budgets is decided but
  not yet built (Dickson, 2026-10-08, reaffirmed 2026-10-09): enforced by
  default in local runs, recorded but not enforced on GitHub's runners,
  and with them a failure when verbatim.exe's relay takes over 2 ms.
- Memory growth (Dickson, 2026-10-08): a scenario of
  about 25 seconds in every suite, ten iterations of a Windows Forms text
  box window opened, edited, reviewed and closed, and the settings dialog
  Tabbed through. After two warm-up iterations, the counts Verbatim
  reports through the control plane return to exactly the same values
  after every iteration, and each process's private memory grows from
  iteration 3 to 10 by no more than a small bound, enforced in local runs
  and recorded on GitHub's runners.
- A behaviour-based MSAA audit (Dickson, 2026-10-08):
  live NVDA captures against a matrix of standard controls, compared
  exactly with Verbatim.
- Medium integrity on GitHub's runner (Dickson, 2026-10-10): the e2e job
  tries running Verbatim and the test applications at medium integrity, as
  users run them, after measuring that it behaves with the runner's UAC
  disabled. The runner is elevated today (`runneradmin`), which is what
  puts "Administrator: " in console titles.
- **Localization**: no hardcoded user-visible strings, ever; pseudo-locale
  test from M1.
- **Flight recorder into regression tests**: every reproduced field/VM bug
  lands as a replay test.
- **Extension API growth**: only via porting real app modules/add-ons;
  each addition needs a consumer.
- **Continuous integration** (D3, amended 2026-09-02): GitHub-hosted
  Windows runners run the end-to-end suite runner-direct, silently, with
  recordings from the audio tee (D16); that is the whole of CI. The VM
  harness is the interactive loop. `.github/workflows/vm-smoke.yml`, which
  probed nested virtualization on larger runners, is to be retired.
- **Housekeeping**: Cargo warns that `proc-macro-error2` 2.0.1, which
  `i18n-embed-fl` depends on, will be rejected by a future Rust
  (`cargo report future-incompatibilities`: it re-exports the private
  `proc_macro` crate, E0365); move off it before the pinned toolchain
  reaches a Rust that rejects it.
