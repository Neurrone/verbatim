# Answers to Dickson's questions on the architecture study

These answers go with `architecture-study.md` in the same folder, which has been revised where an answer changes it. They are read-only findings from 2026-10-30, taken from the code on main at 5797c075, its git history, and the documents named in each answer.

## 1. Do outposts keep only the calls, and where does the screen-reader logic live?

### Where alternative A puts the logic

Under alternative A, Core gains logic and loses almost none:

- Repeats and NVDA's two foreground tests move into Core from the listener and the outposts, which apply them today against `CoreFocus`.
- What moves out of Core is the application rules in `crates/verbatim-core/src/applications.rs` (the switcher's frame refusal, its items' dropped ancestry) and the app's ignored-window rule.

  Decision D6 already says these application rules leave Core when modules arrive in M5: modules' node and event rules apply at the outpost's emission. So A only moves them early. The revised study now keeps them in Core until M5 and states what that costs (question 8).

### Why outposts cannot be left with only the calls

Some decisions must sit between two calls:

- whether to read at all, by NVDA's checks before a read;
- which interface reads the next ancestor, by a window's verdict;
- where a walk stops;
- which kept node an MSAA object equals, which needs live reads of the location and name;
- when a caret key's evidence has come, which bounds the reads of the caret rework.

If Core made these decisions, each one would cost a round trip from the outpost to Core and back. That is 0.8 ms at the median and up to 62.4 ms (text requests, step 1), and a typical focus has three to six such points. Alternatively, Core would have to send the outpost a copy of its state, which is the back-channel the study removes.

### Purity is separable from location

What makes Core easy to test is that it is a pure state machine fed recorded inputs. A judge can be just as pure while running inside the outpost's process.

### Variant V1: a pure judge per application, inside the outpost

This variant is now the study's recommendation.

- Each outpost holds one judge for its application. The judge is a state machine with the shape of Core's reducer: it takes inputs and returns effects. It owns all of the outpost's remembered state: the followed focus, the verdict keeps, the gap record, the batch plan, kept nodes and their identity, and the caret evidence watch.
- Its inputs are:
  - the intake's ordered entries;
  - call results, each tagged with the walk or request it belongs to;
  - timer expiries (the bound, the gap, the 100 ms caret wait);
  - Core's queries.
- Its effects are:
  - calls, given as a call program such as "walk up from this object, stopping at these windows", so that a walk is one effect and not one effect per step;
  - messages to Core;
  - timers.
- The worker executes the call programs. The judge never touches COM and never blocks.
- The judge contains ported NVDA code: the acceptance test, the class lists, the walk rules and MSAA equality. By the provenance rule it therefore lives in a Windows-specific crate, as a module with no Windows types.

  Its platform-neutral pieces stay in `verbatim-core::focus_rules` and are called from it, as `repeats_focus` is called today. So it is pure and testable like Core, but not platform-neutral.
- It is tested the way Core is. Recorded inputs are replayed, and orderings are enumerated: the two routes, a call's late return against the bound, and the gap timer. A recorder dump replays an outpost's decisions exactly as Core's flight recorder does.
- Its threading depends on call cancellation (question 6):
  - If `CoCancelCall` ends UIA and MSAA COM calls, the judge runs on the worker thread. The watchdog cancels a stuck call, and the cancelled call comes back as an input. Everything is single-threaded, so the commit point's lock becomes plain sequential code.
  - Without cancellation, the judge needs a thread of its own. Workers post call results to it. That costs one thread hop per call program, tens of microseconds each in the communication design's count, and one wake-up of the judge per program.

### Variant V2: Core holds a judge per application and the outposts only read

This is alternative C of the study with the per-application order added. Every decision point above becomes a round trip. With 5 UIA calls and 3 to 6 decision points per focus, the median added latency is 3 to 5 ms, and worst cases are hundreds of milliseconds. That is against D15's budget of 10 ms from the event to the utterance queued.

It also adds a Core wake-up for every raw event, and the ported code would have to be rewritten from the parity docs into the platform-neutral Core. It is not recommended.

### Comparison

A and V1 give each fact the same owner. V1 adds that the owner's logic is a pure state machine, which is what makes the residual race classes testable by replay rather than found by review.

Core keeps every decision that spans applications, and every decision about words: speech, presentation, attention, repeats, the foreground, chains, text and terminals. Measured against Dickson's hope, the screen-reader logic in the sense of what is said stays in Core. The outposts keep only the logic that decides which calls to make.

## 2. Every judgement, and what happens to it

A judgement here means a decision that changes what is read, what is spoken, what is followed, or what is the focus. A pure transformation of data, such as building a snapshot or parsing a message, is not one.

For each judgement:

- "Stays" means the same process and owner.
- "Moves" names where to.
- "Changes" says how.
- V1 adds whether it becomes part of the pure judge.

Placements are the same under A and V1 unless the entry says otherwise.

### The listener (`crates/verbatim-outpost/src/focus_read.rs`, `listener.rs`)

- NVDA's checks before a read, in `FocusReader::judge_before_reading`: the hung window, the console window's own element, the MSAA class-list skip (`skipped_by_class`), the acceptance test against the real foreground window, and the sender's keyboard focus (`focused`).
  - Under A: stay, since they are observations.
  - Under V1: they become the listener's pure check function. The listener is not an application's judge, and its reading threads stay as they are.
- The repeat drop against `CoreFocus` (`HeldElements`, `held`, `accept`, `core_focus`, `release`).
  - Changes: no longer judged against Core's state. It is either a read in full, or a full fact built from the listener's own cache (question 7).
- `gap_answer` and `nearest_timeout_reason`.
  - Stay as observations. Each drop carries what the listener saw: whether the sender had no window, and whether the listener gave up.
- The watchdog's publication at the bound and its abandonment (`watchdog`, `publish_at_bound`, `abandon_reason`, the moved-on rule).
  - Stay, or become a cancel plus the reader's own completion if M6.1 succeeds.
- The menu or switcher end and its grace (`menu_or_switch_ended`, `grace_ended`).
  - Stay. They are observations.
- Forwarding until a hook is in (`hook_installed`, `hook_removed`, `forward_msaa_event`, `routed_process`).
  - Stays, with the hold.

### The outpost worker and intake (`crates/verbatim-outpost/src/outpost/`)

- `accepts`, `in_front` and `accepts_event` (acceptance against the real foreground window).
  - Stay. They become pure functions of the window facts.
- `readable`, and the hung retest in `intake.rs` `admit`.
  - Stay. The hung retest of a listener read goes, as the third build already plans.
- The batch planning in `intake.rs` (`plan`, `try_newest_first`, `ends_a_run`, `old_focus_events_first`, `remove_client_focus`) and the limiter.
  - Stay, with Dickson's 2026-10-30 rule that a foreground change into another top-level window is never dropped by a focus in its batch.
  - Under V1: become part of the judge. They already sit close to pure in the intake.
- The gap record (`gap_copy_dropped`, `gap_accepted`, `gap_bound_reached`, `apply_gap_outcome`, `read_answer`; `worker.rs` `gap_bound`, `gap_read`, `copy_dropped_for_uia`).
  - Stays in the outpost, closed by the outpost's own reports.
  - Under V1: part of the judge, its timer an effect.
- The verdict (`arbitration.rs` `decide`, `verdict_at`, `record_probe`, `uses_uia_keeping`; `read.rs` `own_window_uses_uia`, `corrected_backend`).
  - Stays. The outpost is its one owner. The listener's class skip is the same table.
  - Under V1: the keeps and the decision become judge state, and the probe is an effect.
  - Defect B (the frame window judged MSAA) is fixed here as a single-owner parity rule.
- The MSAA duplicate test and the foreground repeat (`msaa_focus_repeats`, `foreground_repeats_focus`, `foreground_repeats`).
  - Change. They no longer drop: the fact is sent in full, from the followed focus's kept record (question 7), and Core judges.
- NVDA's focused-state test and child redirect for an MSAA focus (`msaa_focus`, `report_msaa_focus`).
  - Stay. They are observations by calls.
- `is_focus`, `spoken_node`, `is_logical_ancestor`, `logical_ancestors`, `selection_states_spoken`, `controlled_selection`, `selection_as_value`, `value_change`, `expanded_child_count` (change and selection targeting, the 2026-10-29 exception).
  - Stay, judged against the followed focus, which is the outpost's newest report.
  - Under V1: part of the judge.
- The followed focus and its follow state (`commit.rs` `Watch::commit`, `after_follow`, `emit_focus`, `attempt_owed`, `release`, `slider_target`).
  - Stay. The followed focus is the newest report, with no refusal notice.
  - Under V1: the commit point becomes the judge's transition.
- MSAA equality and node minting (`read.rs` `known_uia_nodes`, the equality steps, `uia_node`; `text/mod.rs` `mint`).
  - Stay. The candidates become the outpost's own reported line, not Core's line.
  - Under V1: the judge decides from facts. The location and name reads are call effects.
- Window description: `plain_windows_from`, `window_from_local_state`, `client_pane`, the boundary and hung stops.
  - Stay.
  - Defect A is fixed here: one node per window handle in an outpost.
- `dialog_text_of` and its bound.
  - Stays.
- Caret and text: `caret_key`, `check_watch_now`, `watched_edit`, `end_watch`, `poll_caret`, `text/mod.rs` `evidence`, `check_caret`, `read_watched`, `caret_report_checking`, `same_caret`, `selection_changes`.
  - Stay. D14 names the caret evidence as the one read-time decision outside Core.
  - Under V1: the watch and its bounds become judge state, and the reads are effects.
- Terminals: `outpost/terminal.rs` `read`, `sends`, `takes_colours`; `worker.rs` `terminal_event`, `read_terminal`, `terminal_read_now`.
  - Stay as reads. The judgement is already Core's (ec7c88c8).
- The outbound merge (`outbound.rs` `merged`).
  - Stays, made lossless as the third build designs.
- `focused_control_in`, the gap and focus-now reads with their served-by checks.
  - Stay.

### The app and the supervisor (`crates/verbatim-app/src/main.rs`, `crates/verbatim-outpost/src/supervisor/`)

- `in_ignored_window`.
  - Under A as first written: moved to the outpost. The revised default leaves it in the app, then Core, until M5. It covers only the harness's own Windows Terminal.
- `fake_focus`, `want_focus_now`, `focus_now_server` (which application is asked after a menu ends, or at start).
  - Stay. They read Windows' own state and make a query with one answer.
- `tell_core_focus`, `held_focus_owner` (`main.rs` 1709 to 1760; `owner.rs` 771).
  - Go with `CoreFocus`.
- The idle barrier (`advance_idle_barrier`, `idle_conditions_clear`, `idle.rs`).
  - Stays, with read requests no longer counted.
- Supervision: `outpost_failed`, the backoff in `policy.rs`, `check_hang`, `heartbeat`, `retire_child`, `assign`, `keep_spares`, `route_fact`, `check_served_by`.
  - Stay. This is lifecycle, single-writer.
- The hold of facts for a starting outpost (`HeldFacts`).
  - Stays, as the second build designs.

### Core (`crates/verbatim-core/src/`)

- `reduce.rs`: `focus_dropped_first`, `is_duplicate_focus`, `is_stale_focus`, `classify`, `observed_before_foreground_change`, `reduce_focus_changed`, `focus_line`, `line_top`, `is_presentable_container`, `focus_speech`, `follow_an_unreported_foreground`, `reads_the_same`, `outpost_ended`, every `reduce_*` for changes, selections, value, progress, states, names, alerts and notifications, and navigation.
  - Stay.
  - Changes: `is_duplicate_focus` takes the full fact's evidence and the MSAA address mark. `classify`'s second foreground test goes. One foreground record replaces the four notions of the foreground.
- `focus_rules.rs`: `same_object`, `meeting`, `already_entered`, `repeats_focus`, `change_target`, `change_candidates`, `selection_target`, `is_sensitive_slider`.
  - Stay as shared definitions. `HeldFocus::for_outpost` goes with `CoreFocus`.
- `applications.rs`: `refuses_focus`, `drops_ancestry`, `is_shell_staging_window`.
  - Stay in Core until M5, by D6 (question 8).
- `shell.rs` `menu_or_switch_ended` (whether to ask for a fake focus).
  - Stays.
- `editing.rs`: the caret, key and selection words (`follow_caret`, `selects`, `removal`, `later_key`, `landing_segments` and the rest).
  - Stay. The caret rework keeps every word in Core.
- `terminal.rs` and `terminal_model.rs`: the reading rules, the flood policy, review, marks and the status bar.
  - Stay, all in Core since ec7c88c8.
- `say_all.rs`, `review.rs`, `review_text.rs`, `text.rs`.
  - Stay.

### The speech manager (`crates/verbatim-speech/src/manager.rs`)

- `drop_expired` judges each utterance's `FocusValidity` against the `FocusNow` the reducer publishes.
  - Stays. This is a copy of Core's focus on another thread, kept one-way and judged by the `ObjectKey` rule. It is the same class within one process, and it is confined because the reducer is its only writer.

### Input (`crates/verbatim-input/src/state.rs`, `crates/verbatim-input-windows/src/lib.rs`, `crates/verbatim-app/src/held_keys.rs`)

- Gesture building, the key's role, whether to swallow a key or pass it on (`on_press`, `build_gesture`, `hands_press_to_os`), the hook's holding of caret keys (`may_hold`, `hold`, `decide`), and the deferral behind a held key.
  - Stay. They are in Core's process and use no other process's state.
  - The caret rework adds the editing chords' reports here.

### The caret and selection rework

- Its placement already fits both A and V1:
  - the outpost holds the last read per node and judges the evidence (stays; part of the judge under V1);
  - Core decides every word (stays).
- Under A, its dependence on the unread repeat goes. "A repeat sent unread is not read and leaves the last read as it was" becomes: a repeat sent from the reader's own cache makes no read and leaves the last read as it was.
- The rule that a node's last read lives while Core holds the node (`NodesHeld`) stays.

### Extensions in M5

- Rules compiled into tables, consulted by the listener and the outposts: stay.
- Modules' node and event rules at the outpost's emission, and hooks on the worker: stay.
  - Under V1: a hook is a call-making effect of the judge, run on the worker under the existing deadline.
- Core resolves commands and runs the modules' output effects: stays.
- See question 3 for what changes.

## 3. Is the extensions implementation affected?

Little, and in its favour:

- `docs/design/extensions.md` already places modules where A places within-application judgement. Modules run in the outposts and in Core, never in the listener. Rules apply where the outpost emits a snapshot, and hooks run on the worker before the report. A module's decision is therefore made before anything reaches Core, by the module, its one owner. The outpost never follows a focus its module refused, so no refusal notice is needed.
- What changes:
  - The `focus()` import means the outpost's followed focus, its newest report, rather than "Core's focus" via `CoreFocus`. Section 4.4 of the extensions design should say so.
  - The Windows Terminal twin rule (`adopt`) compares `node.window()` with `focus().window()` against that followed focus. The rework had already noted it must restate the rule against the followed focus ("Room for extensions").
  - Under V1, a hook is an effect the judge emits and the worker runs. The judge takes the hook's answer (`pass`, `drop`, `replace`, `adopt`) as an input, so a module's decision is replayable with the judge's other inputs, given the recorded answers.
  - The 2026-10-24 plan to move the application rules from `applications.rs` into modules (D6) stays as it is. The rules move once, at M5.
- Hot reload. M5 drops an outpost's kept verdicts when its rule tables change. Under V1 that is one input to the judge, "the tables changed".
- Unchanged: Core-side hooks never read; Core resolves commands; a module's speech is an ordinary effect.

## 4. Adopt the model first, then the rest of the rework, or keep the current order?

### What is already built

Steps 2 to 8 and 17 are built on main, along with the commit point, the identity rule, MSAA equality and the reused runtime id's keep. Every one of them fits the model, so nothing built is undone.

### What the model changes

The model changes only the unbuilt steps:

- Step 9, read requests and the foreground: read requests are not built. The foreground becomes a full fact judged by Core.
- Step 10, repeats: repeats become full facts, from a cache or a read.
- Step 11, the followed focus and `CoreFocus` removed: no refusal notice, and the targets come from the followed focus.
- Step 15's splice defect: replaced by the release rule.

So adopting the model first means redesigning the second build to the model before building it. The second build was going to be designed and reviewed anyway.

### Why this is simpler

The core-flow design's read requests, unread facts, refusal notices and their idle accounting are about half of that design. Five of the eight Important findings of its review are in them (findings 1, 2, 3, 5 and 7's interplay). None of them would be built.

### Recommended order

1. The model decision.
2. The redesigned second build.
3. The third build.

V1's pure judge is not a prerequisite. Question 9 explains why it is best done after the second build, with one piece pulled into it.

## 5. Was this tried before?

Yes, in both directions. The history supports keeping decisions about words in Core, and judging only from facts the reader itself observes.

### Outposts deciding, before the four rules

Before 2026-10-01 the outposts made presentation decisions with mechanisms each added for one failure. The four rules of 2026-10-01 (`docs/architecture.md`, section 1) replaced them:

- snapshot versions;
- observation-time checks;
- spawn and announce generations;
- newest-wins slots;
- a foreground pid gate.

The four rules said "a decision about presentation never lives in an outpost, because only Core sees every application", and kept identical-focus suppression in the reducer "because an outpost doing it alone would suppress a return to its application after a visit elsewhere".

### Repeats moved out of Core

- On 2026-10-11 the UIA repeat test moved to the listener, judged against its own last accepted focus, as NVDA's handler does (a46d8b53, section 16.3).
- On 2026-10-17, 43de3932 found the defect the four rules had predicted. An outpost compared an MSAA focus with the last focus it had reported itself. When another application's focus came between, with no foreground change, the repeat was dropped where NVDA speaks it.
- The fix sent a `FocusQueued` notice to the outpost that last reported the focus. This was the first back-channel.
- On 2026-10-24, d893e07f restated D14 to judge against Core's focus.
- On 2026-10-25, c0bbf19b generalised that to `CoreFocus` sent to every judge, with `Held` and `HeldElements`.
- Between 2026-10-20 and 2026-10-25, the rework study counted six mechanisms patched three to seven times each (the rework design at 5b1e3d5a, "About ninety mechanisms"). Every chain but two tuned a disagreement between two processes' copies of one fact:
  - the hand-over's run ordering, seven commits;
  - the listener's out-of-time judging, six;
  - the gap's drop classes, four, one reversing another;
  - the verdict cache, three;
  - `CoreFocus`, three;
  - the stamps, two.

### The terminal's rules moved into Core

- On 2026-10-24, 353c17ca put the terminal reading rules in the outpost.
- Two days later, ec7c88c8 and 49fcca0e moved them into Core: "the outpost reads and sends, Core judges", "so there is one caret and one record of keys, Core's". The hook had been reporting each key to both the terminal's outpost and Core.
- No defect chain followed the move.

### Lessons

Three times, a judgement made outside Core against a copy of a fact grew a reconciliation:

- the repeat;
- the terminal's keys;
- the gap, which moved between the listener and the outpost four times.

Each time, the cure was one owner fed one-way facts. In the terminal's case the owner was Core.

The one approach that failed was a reader judging repeats on its own partial observation (43de3932). Question 7 shows how a reader can keep the call saving without judging.

## 6. Bringing call cancellation forward

### Evidence today

- MSAA, from `experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/` (2026-10-21):
  - `CoCancelCall` with timeout 0 ended a blocked `IAccessible` COM call with `RPC_E_CALL_CANCELED` 0.1 to 0.3 ms after the cancel, 10 of 10.
  - The proxy worked afterwards, 30 of 30. The cancelled call still ran in the application later.
  - A new proxy's first call needs two cancels. Cancelling every 50 ms, as NVDA does, ended it within 51 to 62 ms.
  - A proxy's final `Release` in a stall was cancellable.
  - Not cancellable: the window-message half of acquisition (`AccessibleObjectFromEvent` and `AccessibleObjectFromWindow` waiting on `WM_GETOBJECT`, which oleacc abandons by itself after about 10 seconds) and oleacc's in-process proxies for standard controls, which use window messages.
  - Cost: about 12 µs per COM call on a thread with cancellation enabled (debug build). Enabling it costs 2 to 11 µs once.
  - Dickson decided on 2026-10-21 not to build it (`docs/roadmap-done.md` line 99).
- UIA: not measured. NVDA cancels its main thread's calls (`nvda/source/watchdog.py` 237). Whether that reaches a UIA client call is listed as unknown in the audit (`docs/design/focus-pipeline-audit.md` lines 55 and 56). UIA's client side makes its own cross-process connection, and its calls may wait on UIA's own threads rather than in a COM call on the caller's thread. Today UIA's transaction timeout (5 s, Verbatim's setting) and connection timeout (2 s) bound every UIA call anyway.

### What cancellation would remove, if UIA calls are cancellable too

- Publication at the bound: the watchdog would cancel at the bound, and the reader would complete its own report with plain windows through the one function it already has. With no second publisher, the races of c47d57ae, 6dbcca25 and 2bd69a85 cannot happen.
- Owed work: there is no hand-over between workers.
- Abandonment, `READ_DEADLINE`, `ABANDONED_LIMIT`, and the late-result discard (`WatchedRead`'s claim). Abandonment remains only for window-message calls: MSAA acquisition, oleacc's own proxies, and UIA's connection messages.
- The commit point, in the sense that the worker would be the only writer; its lock order and its exceptions go.
- The moved-on rule becomes a cancel.

Kill-and-replace stays for window-message stalls. M6.2's rule against two outposts in one application stays.

### What it would not remove

Cancellation does not remove the cross-process classes: the two routes, the order across applications, node lifetime and lifecycle. It removes the intra-outpost thread class, and makes V1's judge single-threaded.

### Cost of the experiment

A scratch test like the MSAA one:

- mockapp's UIA provider made to stall or hold `slow-parent` (already built), and its keyboard-focus and value reads;
- a caller with `CoEnableCallCancellation` cancelling from another thread;
- recording the return code, the time to return, whether the client and element are usable afterwards, whether UIA's later calls or exit show a fault (the `UIAutomationCore.dll` exit fault is on record), and the per-call cost.

This is one subagent session of about half a day to a day. It needs no desktop and no NVDA capture. The MSAA one ran in one session.

## 7. Is reading a repeat in full unavoidable?

No. There are four options.

### What NVDA does

NVDA drops a UIA focus before queueing it when `CompareElements` equates its sender with `lastQueuedFocusObject` and that element still has the keyboard focus (`UIAHandler/__init__.py` 902 to 914). It drops an IAccessible focus whose event parameters match `lastQueuedFocusObject`'s (`IAccessibleHandler/__init__.py` 772 to 776; `isDuplicateIAccessibleEvent`).

`lastQueuedFocusObject` is the last focus queued from any application or backend. It is set at queueing, before an application module can refuse the focus. So NVDA's test needs global knowledge: whether any other focus was queued since.

### Options

1. A full read, judged in Core.
   - Correct by construction.
   - A UIA repeat costs 5 calls where the drop costs 3. An MSAA repeat costs about 20 calls where NVDA makes none.
   - Step 1 counted 63 UIA and 6 MSAA repeats in 35 runs.
2. A full fact from the reader's own cache, judged in Core. This is now the study's default.
   - The reader makes NVDA's test on its own kept read and, if it holds, makes no read. It sends a complete focus fact built from what it already has:
     - The outpost sends its followed focus's record. That record is kept current by the outpost's own change events, since its hook delivers every name, state and value change of its application.
     - The listener sends its kept read's snapshot, with the event's cached name and keyboard focus. Unlike the outpost, it receives no change events, so states read since could be stale.
   - Core judges it like any report. A repeat of the focus Core holds is dropped. Otherwise it is taken as the focus with the cached reading, with no round trip.
   - Calls: NVDA's two for UIA, none for MSAA.
   - Correct by construction for what is spoken and followed. The one residual is a cached property that changed with no event the reader saw: for the listener, a state or value changed since its read. Only the rare case speaks it, where Core does not hold the node.
3. The reader judges alone and sends nothing, as on 2026-10-11 to 17.
   - It fails wherever the reader cannot see that the focus went elsewhere. That was 43de3932's case: another application's focus with no foreground change.
   - The shared window shows why. Settings' content is served by SystemSettings and its frame by ApplicationFrameHost, in one top-level window. A move from the content to the frame's back button and back raises no foreground change. So SystemSettings' outpost, and a listener whose currency counts only changes of top-level window, would take the return as a repeat and say nothing, where NVDA, whose last queued focus was the frame's, speaks it.
   - The listener could count any focus event in another process, since it sees every UIA focus and every focus `WinEvent` system-wide. But NVDA's last queued focus also moves on focuses no event announces: a focus-now answer after an outpost's replacement, the gap's read, the desktop's return. Not every event the listener sees would have been queued either: a copy dropped in a UIA window, or a focus dropped by a verdict. So its coverage cannot be made exact.
4. Unread facts with read requests: the second build's design, the back-channel the study removes.

The revised study takes option 2.

## 8. The within-application refusals

1. Ignored processes.
   - Today:
     - The supervisor's `IgnoredProcesses` (from `VERBATIM_IGNORE_PIDS`) means no outpost starts and no fact is routed.
     - The listener, given `--ignore-pids`, drops their events before queueing.
     - The app's `in_ignored_window` (`main.rs` 1286, applied at 1390) drops a focus that another application's outpost reports in an ignored process's top-level window.
     - `fake_focus` and `want_focus_now` skip ignored processes.
   - Why: the end-to-end harness's own Windows Terminal must never be the focus.
   - It is static data, and `in_ignored_window` is the app judging a focus, which the core-flow design wanted moved into Core.
   - Kept in the app or Core under A or V1, a refused report leaves its outpost following a node Core refused. That can only be a window around a harness terminal, and the harness's own window is never read for speech. Cost: nothing heard.
2. Core's hidden frame.
   - Already outside Core, by a window property (`HIDDEN_FRAME_WINDOW_PROP`): the listener drops a focus in it before reading (`focus_read.rs` 1882 to 1887), and the outpost refuses it (`read.rs` 1476, 1726 to 1743, 1859).
   - Stays where it is under every model.
3. The switcher's frame, `applications::refuses_focus` (`applications.rs` 46; `reduce.rs` 641).
   - It drops a UIA focus in File Explorer whose class and role are `MultitaskingViewFrame` and window, or `Windows.UI.Input.InputSite.WindowClass` and pane, before it moves anything.
   - Kept in Core under A or V1, File Explorer's outpost follows the frame until its next focus, which comes within the switcher's own items. While it follows the frame, it targets no changes of the previous File Explorer focus. Cost: such a change, if any, is not spoken during an Alt+Tab.
   - It moves to File Explorer's built-in module in M5, by D6.
4. The switcher's items' ancestry, `drops_ancestry` (`reduce.rs` 661, 2581).
   - Not a refusal: the focus is taken without its containers. The outpost follows the item. No cost under any model.
5. The shell's staging windows, `is_shell_staging_window` (`reduce.rs` 1023).
   - Taken but not spoken. Not a refusal, so no cost.
6. Others that look similar but are not within-application:
   - The stale rule: across applications, stays in Core.
   - Core's duplicate rule: same node, no follow mismatch.
   - Core's second foreground test: goes.
   - The menu opening's pending focus: judged in Core in the core-flow design. Under A it can stay in Core, because dropping a menu opening leaves the outpost following the menu, which closes or is replaced at once.

### What keeping them in Core costs

- Under A and V1: one stated residual, the switcher's frame during Alt+Tab, until M5 moves it.
- Under V2 and C: nothing, since Core owns everything there, but those carry the round-trip costs of question 1.

The study's revised default is to keep them where they are and let M5 move them.

## 9. The pure judge and the compile-time rules first?

### Size

The outpost is about 46,000 lines. The judgement sits in:

- `worker.rs` (5,937 lines);
- `intake.rs` (3,425);
- `focus_read.rs` (4,467);
- `read.rs` (2,419);
- `commit.rs` (693);
- `arbitration.rs` (1,297);
- `text/mod.rs` (2,766).

### Estimate, in the project's unit of a large build (design, adversarial review, build, review, scenarios)

- The compile-time rules: one build, mostly mechanical.
  - A responder type that answers "gone" when dropped, for every query and request.
  - Node snapshots for the wire constructible only by the registry, which records the message sequence.
  - The Core-to-process enum restricted to queries and `NodesHeld`, with an `xtask` listing.
  - Follow state mutable only through the commit.
- V1's judge for the focus path: two builds. One for the intake, gap, verdict and batch plan, which are already near-pure behind `GapTimer` and injected clocks. One for the walk, the publication and the followed focus as judge transitions with call programs as effects.
- The caret watch and the listener's checks: one more build, later.
- The full suite runs once per build that touches the outpost's focus path, which is most of them.

### What it makes testable

Every ordering of an application's two routes:

- the marker against the copy;
- the copy against the gap's timer;
- the foreground change against a focus in one batch;
- a call returning before or after its bound or its abandonment;
- a commit against a publication.

These become replayed input sequences, enumerable in unit tests, the way Core's stale rule is tested. The four rounds on the publication (c47d57ae, 6dbcca25, 2bd69a85, 8a087fe7) and the gap's timer (f2f8209a) are each such orderings, and each was found by review rather than by a test.

### Should it go first?

Not wholly. It refactors exactly the code the second build changes, so doing it first would mean building that code twice. The recommended order:

1. Pull the compile-time rules into the second build. They are small and prevent review findings 3 and 1's kind directly.
2. Run the cancellation experiment early, since its answer decides V1's threading.
3. Make V1's judge the step after the third build, when the code it encloses has stopped changing.

If Dickson wants the pure judge to shape the second build itself, the second build becomes the judge's first build. That adds about one build to the rework and removes the commit point's lock-order rules.

## 10. The gap record, plainly

### What it is

When an application's focus moves, Windows raises an MSAA focus event, a `WinEvent`. If the window is served through UI Automation, UIA also raises its own focus event, which the listener reads. NVDA, like Verbatim, ignores the MSAA copy in a UIA window and speaks the UIA event.

The gap record is the outpost's note: "I dropped this copy because UIA should deliver this focus". If within one second no UIA focus outcome for that window arrives, the outpost reads its application's focus itself through UIA (`GetFocusedElementBuildCache`, checked by process id) and reports it.

### Why it exists

UI Automation delivers a newly started process's focus events late or never, and sometimes after their senders are gone (`docs/uia-bugs.md`, "A new process's focus events reach the desktop-wide handler late"). A new process's UIA focus was measured arriving 222 to 2,694 ms late. NVDA says nothing for such a focus. Dickson decided to recover it (decision 11 of the listener design, 2026-10-18 and 2026-10-21).

The record also covers:

- the time at Verbatim's start before the listener's UIA registration returns, when UIA events are dropped;
- the listener's drops that say nothing about the copy's focus: no keyboard focus, replaced by a newer event, or a nearest-window lookup that failed fast.

### e02c7854's case

In run 5 of `settings_combo_box_expanded`, Settings' nearest-window lookup failed with `UIA_E_TIMEOUT` after 1.1 ms. That cannot be a real wait on a busy application, since UIA's connection timeout is 2 seconds. The listener dropped the focus as timed out, as NVDA drops it, and that drop closed the record. So neither route read the focus, and Settings was silent.

The fix gives a failure under one second a reason of its own, which leaves the record open. The gap's read at one second then reads the focus, and "Settings", "Settings window", "Region button" is heard about a second late.

### What it costs to keep

In step 1's gap runs:

- 427 copies;
- 139 records opened;
- 2 firings, each a read about a second late.

The `focused-read` batch had 4 more firings for late delivery to a running application and 4 whose senders' windows were gone. An answered record costs two local threadpool calls and no wake-up.

### Narrowing it to the time before an application's first delivered UIA focus would lose

- e02c7854's case, in a running application: silent again, as NVDA is;
- late delivery to a running application: the "Inner" firing in `measure_focus_explorer`, and 4 in the `focused-read` batch, all silent, as in NVDA;
- the gone-sender bursts after an application's first focus.

### What narrowing would keep

- A new process's first focus.
- Verbatim's start, since each application's first focus after it is before its first delivered UIA focus.

### What narrowing would remove

- The restamps.
- The per-window answers, and the closing rules that review findings 4 and 6 of `core-flow-review.md` were about.
- `NEAREST_FAILED_FAST`.

The record would then open only until an application's first UIA focus arrives, and close for good.

### The trade

Narrowing trades a few recovered focuses per hundred runs in File Explorer and Settings for the gap's most review-prone half. It is Dickson's choice, since it amends decision 11.
