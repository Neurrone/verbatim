# The one judge build

Status: design, written on 2026-10-30 against main at e4fecc9b and revised the same day after its adversarial review, before it is built. It applies [the ownership model](ownership.md) to Core's judgement of focuses, the focus listener and the outposts' copy of Core's focus; its brief and starting draft are "The one judge build" in [the ownership builds](ownership-builds.md). The refactor preserves what is heard, except the three changes the model keeps; each place where this design would change speech is named and either answered or put to Dickson under "Open questions".

## Terms

- The back-channel: the `SupervisorToOutpost::CoreFocus` notice, by which Core's focus reaches the listener and the outposts, and every function that builds or reads it.
- The followed focus: what an outpost judges its application's events against before it reads them, built from its own reports and observations only.
- A repeat by its own evidence: a report naming the followed node whose reader's evidence says the same element still has the focus: for UIA, `earlier` is `Focused`, or `earlier` is absent (Core then judges by the node being alive, and so does the outpost); for MSAA, the followed focus's event address, not acquired as a simple child.
- A shared window: a top-level window owned by another process than the outpost's application, holding a control of that application, as ApplicationFrameHost's frame holds a Settings page.

## Can an outpost choose change targets without Core's verdict?

Not by predicting the verdict: one sequence defeats every rule made from the outpost's own observations. Outpost P's control A, in a shared window W, is Core's focus. The user moves to P's other top-level window W2 (focus B, observed at 1,100) and back to W, whose foreground change ApplicationFrameHost's outpost Q observes at 1,200 and Core refuses, since its focus is already in W; A raises no focus event on the return. If Q's report reaches Core first, B is stale and refused, and Core holds A; if B's arrives first, Core takes B. P observes exactly the same events in both orders, so following A loses B's changes in one order and following B loses A's in the other.

The rule that holds in every sequence is a superset: the outpost reads the changes of every node that can still be Core's focus, and Core's targeting decides what is said. That is correct by construction, since words are judged only in Core, and its cost is bounded by three facts of the code:

- Every refusal but the stale rule is decided on facts the outpost has (its own window facts, Core's application rules, the ignore list, its own record of the window its focus is in), so a report those refuse is simply not followed.
- The stale rule can refuse an outpost's report while Core's focus is still that outpost's only when the newest focus Core applied is a refused foreground report of another outpost for the window holding that focus, a shared window (see "A refused foreground report" below). So the superset needs only one extra node: the last followed focus in a shared window.
- Changes of a window unrelated to the foreground are dropped by the outpost's acceptance test before any call, so an extra target costs calls and messages only while its window is in front, which is when Core may need it.

The alternative, a typed message telling the outpost Core's verdict on each report, is the forbidden back-channel: it carries Core's state for a child to judge against, and it arrives one message late, so between a report and its verdict the outpost would still need the superset, which is today's `Held` with its unjudged reports. Reading every change of every candidate in the foreground window and letting Core target would also be correct, but costs calls on every event of every control in front; it is not proposed.

A third option, weighed at Dickson's request: give the outpost a view of Core's state (attention, Core's focus, its verdicts), each change carrying Core's version, every outpost judgement naming the version it used, and Core recognising a judgement made under a version it has moved past. Across processes the view is always at least one message old, so "never diverges" can only mean "every divergence is detected and costs no word". The version rule detects; it does not correct by itself:

- A judgement that read too much (a change of a node Core no longer holds) costs nothing: Core drops it.
- A judgement that skipped a read under an old version leaves Core nothing to correct unless the outpost reports the skip, and Core then needs the node read again, a round trip on the speech path for exactly the racy cases. In the repeat hole, the outpost's view says Core holds A, so A's return is skipped under version v while Core, at v+1, has taken B; only a skip report followed by Core's request to read A again gets A spoken, later than today's design speaks it. In the shared-window stale case, the view says Core holds A until Core's verdict on B arrives, so before it the outpost must still read for both A and B.
- So a lag is harmless in every sequence only if the outpost, while any of its reports or its view is unconfirmed, reads for every node Core may hold under any version not yet confirmed: the superset again, plus the view. That is today's `Held` (Core's focus and the unjudged reports) with a version added. `CoreFocus` lagged the same way but without a version, so a skip under an old copy went undetected, which is the repeat hole.

Its cost over the superset alone: one view message per change of Core's focus to the listener and each concerned outpost (one to three), each a pipe write, a supervisor owner-thread wake and a reader wake-up, as `CoreFocus` costs today, plus a skip report per skipped event, or a request and its read per detected stale skip. It is also what the model forbids: a message carrying Core's state for a reader to judge against. The superset without the view needs neither message, and is the design below.

Decisions an outpost makes before a read that are not change targets (the walk's bound, the gap record's restamp, the window entered in front) depend today on whether Core's focus is elsewhere. Each is answered below from the outpost's own observations where one exists, and otherwise put to Dickson with the residual difference.

## Core's refusal paths

Every path by which Core refuses a focus report, in `crates/verbatim-core/src/reduce.rs`:

- `focus_dropped_first`, line 641: an application rule, `applications::refuses_focus`.
- `focus_dropped_first`, line 644: `is_duplicate_focus`.
- `focus_dropped_first`, line 650: `is_stale_focus`.
- `reduce_focus_changed`, lines 943 to 948: a foreground report of the window already holding Core's focus.

Outside these functions and the draft's four:

- `reduce_event`, lines 714 to 721, with `classify`'s arm at lines 300 to 310: Core's second foreground test, a non-foreground focus whose own window facts are unrelated to the foreground, or, with none, whose application does not hold attention. Before any attention, everything is accepted (lines 280 to 282), and a foreground report always is (lines 287 to 290).
- `crates/verbatim-app/src/main.rs`, line 1390, `in_ignored_window`: a focus in a top-level window of an ignored process.

This build adds one: a focus report tried only because the foreground report before it might be refused (see "Focus groups"), which Core drops when it took that foreground report.

## Core

Runs on the reducer thread of `verbatim.exe`.

### The back-channel removed

- `crates/verbatim-app/src/main.rs`: `tell_core_focus`, `held_focus_owner`, `notice_recipients` and its test, and their call sites.
- `crates/verbatim-core/src/state.rs`: `focuses_queued` with its two increments, `held_focus_owner`, `held_focus`.
- `crates/verbatim-core/src/focus_rules.rs`: `HeldFocus`, `HeldFocus::for_outpost`, `RepeatEvidence::settled`. `HeldNode` stays as the outpost's record of a node.
- `crates/verbatim-outpost/src/protocol.rs`: `CoreFocus` and its stamping arms; `supervisor/mod.rs`'s `core_focus`; `supervisor/owner.rs`'s `OwnerEvent::CoreFocus` and `tell_core_focus`; the mention in `supervisor/writer.rs`.

### Repeats judged only in Core

`is_duplicate_focus` is unchanged, with the rule for `earlier` exactly as on main. A UIA repeat carries the listener's `same_as_last` as `earlier`; an MSAA repeat carries none and names the node minted for its address before (`a_change_of_a_focus_core_no_longer_holds_is_not_read` asserts the same id), so Core finds its own alive focus and drops it, keeping the newer reading. Every repeat a reader used to drop named Core's alive focus, so no MSAA address or simple-child mark is needed in Core to preserve speech; a simple child's repeat reaches Core today already.

The kept change, the repeat hole, in arrival order: Core holds A (Notepad's edit), another application's B reaches Core, then A's repeat. Before, the reader dropped A against its copy of Core's focus; now Core says what `the_same_focus_after_another_applications_focus_is_announced` pins: the expired focus speech, a stop, then "Text editor", "edit", queued.

Its limit: when A's repeat (observed at 1,200) reaches Core before B (observed at 1,100, a slow read), the repeat is dropped as a duplicate without moving the stale rule's stamp, so B is taken and spoken and Core ends on B while A has the focus. Today ends the same way, and `a_duplicate_focus_does_not_make_a_later_focus_stale` pins it. See the open questions.

### A refused foreground report

`reduce_event` records the newest applied focus (`latest_focus`, lines 726 to 732) before `reduce_focus_changed` refuses a foreground report of the window holding Core's focus. Such reports used to be dropped by the outpost whose focus Core held, so with them reported, a refused one would make another outpost's earlier focus in another window stale (Core's check box in W from outpost 1; W's foreground at 2,000 refused; outpost 2's focus at 1,900 elsewhere would turn silent), and would make `menu_or_switch_ended` (`shell.rs` line 179) skip its read of the foreground focus.

So `reduce_event` does not record a foreground report of the window holding Core's focus when Core's focus is the reporting outpost's: exactly the reports the outpost dropped before. One from another outpost (the shared-window case) is recorded, as today. The one transient difference: today such a report reached Core if the outpost had a report outside W still awaiting Core's judgement.

### Focus groups

An outpost tries a focus group's entries newest first and stops at the first reported (`intake::try_newest_first`). Today a foreground change of the window holding Core's focus, when that focus is the outpost's, is dropped before it is read (`worker.rs` lines 3776 to 3781) and so lets the next older entry be tried; `a_foreground_change_of_the_focus_s_window_is_dropped_for_the_focus_beside_it` pins it. A dropped MSAA repeat ends the group today (`self.reported = true`) and still does, being reported.

Now a foreground change of the top-level window holding the followed focus is read and reported, and the group goes on: the next older entry is tried and its report, if it reports a focus, carries `if_refused`, the trace of that foreground report, a new optional field of `NormalizedEvent::FocusChanged`. Core keeps, in the foreground record, the trace of the last foreground report it judged and whether it took it, and drops an `if_refused` report, before anything else judges it, unless that report was refused while Core's focus was the reporting outpost's. So:

- Core's focus is the followed control in W: the foreground report is refused, the older focus taken and spoken, as today ("Three" in the test above).
- Core's focus is elsewhere: the foreground report is taken and W said; the older focus is dropped, as today, where the group ended at the foreground change.
- Core's focus is another outpost's node in W: today the foreground report reached Core, was refused and ended the group; the older focus is dropped, as today. NVDA would try the older focus here; that divergence is recorded for the focus pipeline rework.

A foreground change of any other window ends the group when reported, as today.

### One foreground record

A new module, `crates/verbatim-core/src/foreground.rs` (planned), with one type `Foreground`, private fields, one `apply(&mut self, ForegroundFact)` and read-only accessors, replacing `attention`, `foreground` (the object), `foreground_changed_at`, `foreground_window` and `latest_focus` in `SrState`, plus the last foreground report's trace and outcome above. `ForegroundFact` has one variant per writer: `Reported`, `Unreported`, `Object`, `LineCut`, `Window`, `ForegroundJudged`, and `FocusOrdered` (the newest focus by observation, written for every attended focus except the refused foreground reports above). `classify` takes `&Foreground`, so its second test stays as a function of the record. `SrState` holds the record with `#[serde(flatten)]` and today's field names, so `tests/fixtures/focus_value_states.jsonl` and earlier dumps still load. Each value is written where it is today, with the two exceptions this section names: refused foreground reports of the focus's own outpost no longer order focuses, and the last foreground report's outcome is new.

## The listener

Runs in the listener process: callbacks queue, one reading thread per application judges and reads, the scheduler's state is under the readers' lock, its watchdog abandons and publishes.

### `HeldElements` removed

`focus_read.rs` loses `HeldElements`, `Accepted`, `Judgement` and their calls; `listener.rs` its field and `CoreFocus` arm. `judge_before_reading`'s step 3 goes: a focus passing the other checks is read in full. The comparison is made for menu openings only.

### The menu opening's comparison

A menu opening is compared with the reading thread's own last read (`LastFocus`) while that read is current (Dickson, 2026-10-30): no focus or menu opening of another application, and no foreground event of another top-level window than the read's, has come since the read's event was queued. The scheduler keeps per application `left_at`, the queue place of the newest event that moved focus away from it: another application's UIA focus or menu opening as it is queued, another process's MSAA focus as the listener's WinEvent thread forwards it, and a foreground event of another top-level window than the application's last read's, or than its read under way once that read knows its window; a foreground event during a read whose window is not yet known counts as moving away. Each queued event records `left_at`; `LastFocus` records its event's place and top-level window. A menu opening compares only when the last read's place is greater than its recorded `left_at`; a failed comparison drops it, as today. This is the third kept change.

A reading thread clears its `LastFocus` when its read was abandoned, as the scheduler answers its return, so an abandoned thread that goes on reading at the abandoned limit (`run`, lines 2639 to 2655) neither compares with nor reuses ancestors of a focus Core never got.

### The scheduler as a pure transition

`ReadersState` and its decisions move into `crates/verbatim-outpost/src/read_schedule.rs` (planned): `Schedule<J>` with `step(&mut self, input, now_us) -> Vec<ScheduleEffect<J>>`, no clock, thread, lock, channel, atomic, file, process or binding.

- Inputs: `Push`, `Next`, `Returned` (generation, outcome kind, top-level window), `Tick` (the watchdog's wake, with moved-on answers each tagged with the window it was judged for), `Foreground` (top-level), `MsaaFocus` (pid), `OutpostEnded`, `WhenIdle`, `Stop`.
- Effects: `Outcome`, `Spawn`, `Wake`, `Claim` (written by the shell into the read's `WatchedRead`), `ForgetLast` (to a thread whose read was abandoned), `Alarm`, `RunIdle`, `Job`.
- A `Tick` answer for a read whose window changed after the answer was gathered judges nothing for it and sets the alarm to now, so the read is judged again on its known window.
- The shell, `Readers`, keeps the lock, threads, condition variables, alarm, waiters' closures by number and the reads' progress, and applies each step's effects in order under the lock, as outcomes are sent under it today.
- The menu end's grace moves into the same module as `GraceState::step(now_us, input)`, its timer staying in the shell (Dickson, 2026-10-30).

The marker's responder waits for the messages build; the transition emits exactly one `Outcome` per pushed marker, which its tests assert.

### The first ownership check

A new `cargo xtask ci` step, `xtask/src/ownership.rs` (planned), beside `waits.rs` and `shapes.rs`: for each listed module, today the scheduler's alone, it fails on a `use` outside the module's allow-list (`std::collections`, `std::time::Duration`, `verbatim_model`, the listener's data types) and on any occurrence, imported or written as a full path, of `thread`, `Mutex`, `Condvar`, `Atomic`, `Arc`, `mpsc`, `Instant::now`, `SystemTime`, `std::fs`, `std::io`, `std::env`, `std::process`, `std::net`, `windows::` or `unsafe`. It reads text, so a review rule covers what it cannot see; the replay tests are the real check. One unit test of the checker.

## The outposts

The event thread queues, the worker judges and reads, the watchdog publishes at the bound; the followed focus is a monitor whose lock is never held across a call.

### `Held` becomes the followed focus

`outpost/held.rs` is replaced by `followed.rs`, `Followed`, holding:

- The followed focus: the newest report's node and chain with what `held::Focus` records today (top-level window, `entered_in_front`, MSAA event address, `simple_child`), from the worker's commit (`Report::Reported`, `Report::Answered` with `answer_to_judge`).
- `beside`: the window of a foreground report that did not replace the followed focus, a change target until the next report (Dickson, 2026-10-30).
- `shared`: the last followed focus in a shared window, a change target until the outpost reports a focus in that window again or its node is released (Opus, 2026-10-30, pending Dickson's acceptance). This answers the stale refusal (Dickson, 2026-10-30: fixed, not accepted as a limit).
- `states_read`, the outpost's own.
- The dialog texts of the followed line (Dickson, 2026-10-30).

A report replaces the followed focus, except:

- A report an application rule refuses, through `applications::refuses_focus` made `pub`.
- A report in a window of an ignored process, from the ignore list the outposts inherit through `IGNORE_PIDS_ENV` (Dickson, 2026-10-30).
- If Dickson accepts the mirror: a non-foreground report whose own window facts are unrelated to the foreground, by `ForegroundRelation::of_facts(&facts).related()`, made only while the outpost follows a focus, since Core accepts everything before any attention and an outpost with nothing followed cannot be holding Core's focus.
- A foreground report of the top-level window holding the followed focus, which becomes `beside`.

Every focus report carries window facts, so Core never judges an outpost's report by attention and no exception depends on Core's attention (Dickson, 2026-10-30: the gap closed). Two reports carry none today: a listener read whose event had no window handle and whose read found no nearest window, which only tests make (`worker.rs` lines 4343 to 4347); and a focus-now answer whose control's window was not found. Both now carry the facts of the application's keyboard focus window (`focus_window_of`, a local call `uia_focus_window` already makes), else, for a focus-now answer, of the foreground window it read; a report with neither is not sent, which only the test path reaches. A debug assertion in `event_message` holds every focus report to it. Speech changes only where Core judged such a report by attention and its window's relation to the foreground differs; the builder logs these reports for a run of the scenarios to confirm none reaches production.

A repeat by its own evidence updates the followed reading but leaves `states_read` and moves no subscription (the `adopt: false` path), with the same evidence Core uses, so the two judgements agree, including a UIA report with no `earlier` after an abandonment.

`Tracking` (`commit.rs`) stays separate: it follows controls only, for the caret, text and subscription.

### Judgements moved onto it

`spoken_node`, `selection_states_spoken`, `known_role`, `logical_ancestors`, `msaa_selection`'s chain, `expected_uia_focus`, `walk_left`, `entered_before` and `read::msaa_candidates` judge against the followed focus, `beside` and `shared` (candidates from all three; the walk's bound and equality from the followed focus). `msaa_focus_repeats` and `foreground_repeats` go.

Where today's answer depended on Core's focus being elsewhere (`Held::expected()` returning `None`):

- The window entered in front (`judged_focus`, `entered_in_front`): the followed record refreshes it to the window on every foreground change of its top-level window the outpost handles. On a return from another application the window's foreground change comes, so the value equals today's recomputation; across a popup of another process that took no foreground, the window stayed in front, and the inherited value is today's. So the owner window's own state change on a dialog opening stays unspoken.
- The walk's bound (`walk_left`): `MOVED_ON_GRACE` when the outpost has handled a foreground change of its window since its last report, as on a return by Alt+Tab, else by the followed focus's top-level. The residual: a return from another process's popup with no foreground change gets `ENRICHMENT_BUDGET`, 2 seconds, where today it got 500 ms; it differs in speech only for a walk slower than 500 ms. See the open questions.
- The gap record's restamp (`gap_accepted`, `worker.rs` line 3790): reached today only when the foreground change was not dropped. See the open questions.
- `expected_uia_focus` and `selection_as_value`, and change reads of the followed focus: read for an application whose focus Core no longer holds while its window is related to the foreground (another process's menu or the switcher over it); calls and messages, which Core drops.

### Dialog text

The followed record keeps each dialog's text as gathered when the line entered it, and every later report whose chain holds that dialog carries it, with no call (Dickson, 2026-10-30), so a return by Alt+Tab or from another process's menu to a message box keeps its text. The carried text can be older than a fresh read if the dialog's text changed while focus stayed in it.

### Edge cases and speech

- Space on a check box after its window's foreground report: refused by Core, `beside` for the outpost; "checked", as today.
- The same with Core's focus elsewhere: Core takes W and says it, as today; W's name, value and description changes are read through `beside` and spoken, as today; the box's changes are sent and dropped.
- The defeating sequence above: A stays a target through `shared`, so A's state change is spoken when Core holds A, as today, and B's when Core took B.
- Across processes in one top-level window: each outpost follows its own; Core's targeting decides.

## Cost

Calls are counted by kind, as `docs/performance.md` does; the builder pins each figure in `crates/mockapp/tests/call_counts.rs` and the document in the same commit.

- A UIA repeat: 3 UIA calls before, about 5 after, plus any text read for a text control. One message up to Core, one reducer input, and one `NodesHeld` down (it is sent whenever the outpost's acknowledged position moves, `live.rs` line 48), each with its reader's wake-up.
- An MSAA repeat: the role and `accFocus` before, the steady-state focus read after, 20 MSAA calls; the same messages.
- A foreground change into the followed focus's window: local calls before, 5 to 10 MSAA calls after; the same messages; and in a focus group, the older entry is read too, conditionally reported, where today it was read only when Core's focus was in that window.
- `shared` and `beside`: no call while their windows are unrelated to the foreground; otherwise, per change, what any change read costs (2 MSAA calls and 1 window message for a state change) and a message.
- Removed on every focus Core judges: a supervisor owner-thread wake (`supervisor/mod.rs` line 611), one `CoreFocus` to the listener and one to each of one to three outposts, each a pipe write and a reader wake-up, and Core's copy of its focus line.
- Latency on the speech path and the reducer's work per input do not rise.

## Tests

### Added

- Core (`crates/verbatim-core/tests/reduce.rs`): the repeat hole in arrival order (A at 1,000 from outpost 1, B at 1,100 from outpost 2, A at 1,200 with `earlier` `Focused`), heard as the existing test above; an MSAA repeat of the alive focus is silent and keeps its newer states; a change off the focus is silent; a refused foreground report of the focus's own outpost makes no later focus stale (the 1,900 and 2,000 sequence, outpost 2's focus spoken); the three `if_refused` cases above, each exact; one test per `ForegroundFact`. `notepads_foreground_report_after_the_desktops_last_focuses_is_spoken` and the two stale tests pass unchanged.
- The outpost (`followed.rs`): each exception; `shared` kept until a report in its window; a repeat by its own evidence, `earlier` absent included, leaves `states_read`; `entered_in_front` refreshed by a foreground change. Mockapp (`msaa_events.rs`): the followed check box kept beside its window's foreground report, and its state change spoken through `SrState`, "checked"; the focus group's older entry reported with `if_refused` after the foreground report, and "Three" spoken through `SrState`; the dialog text carried after another application's focus.
- The listener (`read_schedule.rs` replay tests): an event at the abandoned limit read by a returning thread; an abandoned read's late return gives no outcome and a `ForgetLast`; no grace-end alarm without a foreground event; an event waiting at the limit dropped with its reason at the deadline and not a pending focus elsewhere; an idle waiter run with later events queued; a grace callback before its end re-arms and sends nothing; one outcome per marker; menu currentness for each kind of moving away; a stale `Tick` answer re-arms. A thread-level test with the test `Reading`: an abandoned thread that goes on at the limit has no last read.
- Mockapp (`focus_reports.rs`): a repeated UIA focus and an MSAA repeat each read in full with their calls; a menu opening not current judged by its own window with no comparison.

### Removed, each with its cheaper replacement

- `held.rs`'s tests and `worker.rs`'s `foreground_repeats` tests: by `followed.rs`'s tests, `a_foreground_change_into_the_focused_window_is_ignored`, and the `if_refused` tests.
- `focus_read.rs`'s `HeldElements` tests: by the replay and thread-level tests of an abandoned return.
- `notice_recipients`' test and `the_focus_core_holds_carries_its_windows_handles`: the behaviour is removed; window handles stay covered by `read.rs`'s `entered_before` tests.
- `a_focus_waiting_for_core_s_judgement_is_no_repeat`, `a_focus_is_no_repeat_once_core_holds_another`, `a_uia_focus_is_no_repeat_after_another_application_s_msaa_focus`, `a_repeated_msaa_focus_is_reported_after_another_application_s_uia_focus`: by `the_same_focus_after_another_applications_focus_is_announced`, `a_duplicate_focus_does_not_make_a_later_focus_stale`, the new Core repeat tests and the mockapp full-read tests.
- `a_repeated_focus_out_of_time_is_reported`: by `a_focus_out_of_time_is_dropped_only_by_its_windows_class`.
- `a_change_of_a_focus_core_no_longer_holds_is_not_read`: by the Core test of a change off the focus.
- The harness's `core_focus_elsewhere`, `listener_core_focus_elsewhere`, `listener_core_refuses_last_read`, `tell_core_focus`; the notice's parts of `idle_round.rs`, `spares.rs` and `tests/common`'s `held_focus`.

Rewritten: `a_repeated_focus_on_the_still_focused_element_is_dropped_before_any_read` and `a_repeated_msaa_focus_is_a_duplicate_only_of_a_full_object` become the full-read tests; `uia_repeated_focus_costs_exactly` pins the full read; `a_menu_opening_on_another_element_is_judged_by_its_own_window` makes its second read not current; `a_foreground_change_of_the_focus_s_window_is_dropped_for_the_focus_beside_it` asserts the foreground report, then the focus with `if_refused`, with their calls; `a_second_foreground_change_of_a_reported_window_is_dropped` asserts the second change reported and silent through `SrState`; `a_foreground_change_after_another_application_s_focus_is_reported` asserts the change reported, with no `core_focus_elsewhere`, its window said through `SrState` holding another application's focus; `a_change_of_a_focus_core_has_not_judged_yet_is_reported` keeps its sequence.

### Scenarios

The full suite once, then `text_box_editing`, `second_application_and_verbatim_menu`, `settings_system_page`, `settings_combo_box_expanded`, `explorer_folder_window`, `menu_and_settings_dialog`, `windows_terminal_two_windows`.

## The known traps, against the code

- `states_read` (review 3): the outpost's own; untouched by a repeat by its own evidence.
- A report refusable by Core's definition (review 3): not followed.
- Core's refusal of a foreground report within one application (review 1, finding 2): the foreground exception, `beside`, and the group's `if_refused`.
- The menu opening's lost subject (review 1, finding 3): the thread's own current last read.
- The cost of MSAA repeats and the menu's pending focus (review 1, nits): stated; `Readers::focus_pending` stays a query of the scheduler.
- Across processes in one top-level window (review 2): messages added, never words; the stale case answered by `shared`.
- This design's review: focus groups by `if_refused`; refused foreground reports kept out of the stale stamp; the repeat hole's out-of-order limit stated; Core's focus elsewhere answered per decision above; an abandoned thread's last read cleared.

## Dickson's decisions (2026-10-30)

- The dialog text carried; `beside`; menu currentness lost only on another top-level window; the menu end's grace as a pure transition.
- The outpost mirrors the ignored-window drop from the inherited ignore list.
- The no-window-facts gap is closed, not left.
- The stale refusal while Core's focus is still the outpost's is fixed, not accepted as a limit.

## Open questions for Dickson

- Mirroring `classify`'s second test on the outpost's own window facts (still being decided): recommended, yes, only for non-foreground reports and only while the outpost follows a focus; without it, a report Core refuses on those facts becomes the followed focus and Core's focus's changes go unread.
- The stale fix by `shared`, the last followed focus in a shared window kept as a change target until a report in that window: accept it? Recommended, yes: the argument above shows no rule from the outpost's observations can choose one target there, and `shared` costs no call while its window is not related to the foreground.
- The repeat hole when the repeat arrives before the other application's earlier focus: keep it as a limit, or let a duplicate advance the stale rule's stamp? Advancing makes Core end on A, but B is then never spoken in that order, and `a_duplicate_focus_does_not_make_a_later_focus_stale` changes from B spoken to silence. Recommended: keep the limit, the rule as pinned today, and leave it to the focus pipeline rework.
- The gap record's restamp by a foreground change of the followed focus's window: today it happens only when Core's focus is not the outpost's in that window. Restamp on every such change, or on none? Recommended: on every one, since a return is what the restamp exists for; it differs from today only when Core's focus is the outpost's control in that window, where Core's same-window exemption makes the later stamp harmless to the stale rule, but the restamp also answers a record open in another of the application's windows, which the builder must confirm reads nothing differently.
- The walk's bound on a return from another process's popup with no foreground change: 2 seconds where today 500 ms. Accept, or have the listener forward to the outpost the observation that another application's focus came? Recommended: accept; a forwarded observation of another application is no observation of the outpost's own, and the difference reaches speech only for a walk slower than 500 ms.
