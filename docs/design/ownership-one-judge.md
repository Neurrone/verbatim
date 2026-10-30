# The one judge build

Status: design, written on 2026-10-30 against main at e4fecc9b, rebuilt the same day around one rule for what an outpost reads, after two reviews showed that every mechanism in which an outpost guessed Core's verdict needed a patch per sequence. It applies [the ownership model](ownership.md) to Core's judgement of focuses, the focus listener and the outposts' copy of Core's focus; its brief and starting draft are "The one judge build" in [the ownership builds](ownership-builds.md). The refactor preserves what is heard, except the three changes the model keeps; each place where this design would change speech is named and put to Dickson under "Open questions".

## Terms

- The back-channel: the `SupervisorToOutpost::CoreFocus` notice and every function that builds or reads it.
- An unreleased report: a focus or foreground report an outpost sent to Core whose focus node Core has not released. Its nodes are the report's focus and the ancestors it carried.
- The release protocol: `SupervisorToOutpost::NodesHeld` (`crates/verbatim-outpost/src/protocol.rs`, line 815), which Core's shell sends an outpost after handling its messages whenever the nodes Core holds there or the position it acknowledges change, and by which the outpost releases every node it reported at or before that position that Core does not hold (`outbound.rs`, `take_releasable`; the worker's `release`).

## The rule, and its test

The rule (candidate, approved for study by Dickson, 2026-10-30): an outpost reads changes for every node of its unreleased reports, and Core's targeting decides what is spoken. While a `NodesHeld` is in flight, the outpost has released only what Core released at an acknowledged position, so its set can only be a superset of the focus line Core holds, never a subset: a lag costs extra reads, never a word. `NodesHeld` is a release message about node lifetime, sent today, carrying no judgement; the outpost judges nothing against Core's focus, only against what Core may still hold.

### Manageability

What Core holds in an outpost today (`SrState::held_nodes`, `state.rs` line 983): its focus and the focus's ancestors, while the focus is alive; the focus's last announced selection; the navigator object; the node a pending navigation starts from; the caret's node, a pending caret or text request's node, a held key's read, the start marker's node and say-all's node. Every one but the focus line is one node, and usually the focus itself.

- Typical size: the focus line, one chain from the focus to the desktop, plus at most a few single nodes. A Win32 control in a dialog carries two to four nodes (mockapp's MSAA fixture: "Settings", "First"); a Settings page read through UIA, about ten to fifteen. The builder records `held_nodes`' size per outpost in the scenarios' flight dumps and puts the figures in `docs/performance.md`; no measurement exists yet.
- Worst case: two focus lines during one release round trip (the old line until Core's `NodesHeld` acknowledges past the new report), plus a line Core keeps only through its navigator or an unfinished text request. The chain's depth bounds each line; nothing grows with time, since every node Core stops holding is released at the next acknowledgement.
- How fast it shrinks: Core's shell sends `NodesHeld` after handling the outpost's message, one pipe hop; the outpost handles it as an intake item in order with its events (`Key::NodesHeld`, a newer one replacing a waiting one). So the old line leaves after one hop and whatever events were queued before it, the reply half of a round trip that costs 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time in all (`docs/performance.md`, held keys, debug).
- Call cost: changes of a window unrelated to the foreground are still dropped by the acceptance test before any call. An MSAA change in front is acquired (1 `WM_GETOBJECT` and 1 `get_accChild`, today's cost for every accepted change) and told from the candidates by its identity with no call; only a candidate's property is then read. So the rule adds a property read (1 or 2 MSAA calls) and a message only for a change of a node that is unreleased but outside the line Core holds: the previous line for one round trip, a report Core refused, or a line Core keeps through its navigator. Name, value and description changes are read only for the nodes sent as a report's focus, state changes for every node of an unreleased report, since Core speaks the first kind only on its focus and the second on the focus or its ancestors, and Core's focus is always a report's focus.

Verdict: manageable. The set is one focus line, two for one round trip, and its extra reads come only from changes of those few nodes while their window is in front.

### The sequences the reviews found

- The shared-window stale case (Settings' control A, Core's focus, inside ApplicationFrameHost's window; Settings' B in another window refused as stale): A stays unreleased, so A's MSAA changes are read whichever order Core judged the two in. It fails for UIA property changes, which reach an outpost only through its one focus-following group, on the newest report's element (`docs/design/focus-pipeline.md`, section 17): once B is reported, A's UIA changes are not heard, as today, where `Tracking` also follows B. See the open questions.
- A refused foreground report of the window holding Core's focus: the window and the control are both unreleased nodes, so the control's state change is read and Core says "checked", as today; when Core took the window instead, the window's name change is read as a report's focus and spoken, as today.
- Focus groups: the rule decides what is read for changes, not where a group stops; the group's stop is moved into Core (see "Focus groups").
- Core's focus in another application: Core releases this outpost's line one hop after it takes the other focus, after which nothing of it is read, as today after the notice. Before that, the line's changes are read and dropped by Core.
- The repeat hole: nothing is skipped as a repeat; Core judges every repeat (see "Repeats").
- Attention, and every refusal the outpost cannot see: none matters to the rule, since what is read depends on what Core still holds, not on why it refused a report. The mirrors of `classify`, the application rules and the stale rule, `beside`, `shared` and `if_refused` of the earlier revision are all unnecessary and gone.

### What else an outpost's view of the focus decides, and from what

- `Tracking` (`commit.rs`): the caret, text, terminal output and the focus-following group follow the newest report of a control, the outpost's own fact, as today; it never followed Core's verdict.
- The walk's stop at a known line and MSAA equality's candidates (`read::msaa_candidates`): every unreleased report's line, today's Core line and unjudged reports with the notice replaced by the release.
- `states_read`: the outpost's own record of the states it last read for a focus; a repeat by its own evidence (below) does not update it, so a state change read after the repeat still finds "newly expanded" and reads the child count.
- `entered_in_front`: inherited from the newest unreleased report in the same top-level window, as from today's expected focus, and set to the window by every foreground change of it the outpost handles, its own observation that the window came to the front; so a dialog disabling its owner stays unspoken after a return.
- The walk's bound (`walk_left`): `ENRICHMENT_BUDGET` when the newest unreleased report is in the same top-level window, else `MOVED_ON_GRACE`, which is today's rule with the expected focus taken from the release set. It differs only while a line is unreleased that Core no longer holds as its focus, for one round trip or through the navigator, and then only in timing.
- Dialog text (`describe_dialogs`): a dialog counts as already entered against every unreleased report's line, a superset, which would gather the text less often than Core enters the dialog; so each report carries the text gathered when its line first entered the dialog, with no call (Dickson, 2026-10-30), and Core speaks it only when its own line enters the dialog.
- A selection read against the focus (`expected_uia_focus`, `selection_as_value`): against every unreleased report's focus.

A repeat by its own evidence: a report naming the newest report's node whose reader's evidence says the same element still has the focus (UIA: `earlier` is `Focused`, or absent, when Core judges by the node being alive and so does the outpost; MSAA: the same event address, not acquired as a simple child). It leaves `states_read` and the focus-following group as they are (the `adopt: false` path).

## Core's refusal paths

Every path by which Core refuses a focus report, in `crates/verbatim-core/src/reduce.rs`: `focus_dropped_first`'s application rule (line 641), duplicate rule (644) and stale rule (650); `reduce_focus_changed`'s foreground report of the window already holding Core's focus (943 to 948); `reduce_event`'s second foreground test (714 to 721, with `classify` at 300 to 310); and the app shell's ignored-window drop (`crates/verbatim-app/src/main.rs`, line 1390). Under the rule none of them needs to be predicted by an outpost.

## Core

Runs on the reducer thread of `verbatim.exe`.

### The back-channel removed

- `crates/verbatim-app/src/main.rs`: `tell_core_focus`, `held_focus_owner`, `notice_recipients` and its test, and their call sites.
- `crates/verbatim-core/src/state.rs`: `focuses_queued` with its two increments, `held_focus_owner`, `held_focus`.
- `crates/verbatim-core/src/focus_rules.rs`: `HeldFocus`, `HeldFocus::for_outpost`, `RepeatEvidence::settled`.
- `crates/verbatim-outpost/src/protocol.rs`: `CoreFocus`; `supervisor/mod.rs`'s `core_focus`; `supervisor/owner.rs`'s `OwnerEvent::CoreFocus` and `tell_core_focus`.

### Repeats

`is_duplicate_focus` is unchanged, with the rule for `earlier` exactly as on main. Every repeat a reader used to drop named Core's own alive focus (an MSAA repeat's node is the one minted for its address before, which `a_change_of_a_focus_core_no_longer_holds_is_not_read` asserts), so Core drops it silently and keeps its newer reading. The kept change: when another application's focus B reaches Core between A and A's return, A is spoken, as `the_same_focus_after_another_applications_focus_is_announced` pins (the expired focus speech, a stop, "Text editor", "edit"). When A's repeat arrives before B, observed earlier, B is taken and spoken and Core ends on B, as today; see the open questions.

### Focus groups

An outpost tries a focus group's entries newest first, as NVDA's `pumpAll` tries a run of events, stopping at the first processed. Today the outpost dropped a foreground change of the window holding Core's focus before reading it, judged against its copy, and the older focus was tried (`a_foreground_change_of_the_focus_s_window_is_dropped_for_the_focus_beside_it`). Whether that change is processed is Core's verdict, so the stop moves into Core: every report of a group carries the group's number and its rank (newest first), and Core takes a report only when it took no newer report of the same group (Opus, 2026-10-30, pending Dickson's acceptance). The outpost reads an older entry after a reported foreground change only when the change's window holds a node of an unreleased report, the case today's drop covered; otherwise the group ends at the change, as today. A reported focus ends the group, as today.

So: Core's focus the control in W, the change refused and the older focus taken and spoken ("Three"), as today; Core's focus elsewhere, the change taken and W said, the older focus dropped, as today; Core's focus another outpost's node in W (a shared window), the change refused, and the older focus not read, since no node of this outpost's unreleased reports is in W, as today.

### A refused foreground report and the stale stamp

`reduce_event` records the newest applied focus (`latest_focus`, lines 726 to 732) before `reduce_focus_changed` refuses a foreground report of the window holding Core's focus. Such reports from the outpost whose focus Core holds used to be dropped in that outpost; reported now, they would make another outpost's earlier focus in another window stale (Core's check box in W from outpost 1; W's change at 2,000 refused; outpost 2's focus at 1,900 elsewhere silent), and would make `menu_or_switch_ended` skip its read. So a refused foreground report from the outpost whose node is Core's focus does not move the stamp, which keeps today's speech; one from another outpost moves it, as today. See the open questions.

### One foreground record

A new module, `crates/verbatim-core/src/foreground.rs` (planned): one type `Foreground`, private fields, one `apply(&mut self, ForegroundFact)`, read-only accessors, replacing `attention`, `foreground` (the object), `foreground_changed_at`, `foreground_window` and `latest_focus` in `SrState`. One variant per writer: `Reported`, `Unreported`, `Object`, `LineCut`, `Window`, and `FocusOrdered` (every attended focus except the refused foreground reports above). `classify` takes `&Foreground`, keeping its second test. `SrState` holds the record with `#[serde(flatten)]` and today's field names, so `tests/fixtures/focus_value_states.jsonl` and earlier dumps still load. Each value is written where it is today, except that refused foreground reports of the focus's own outpost do not order focuses.

## The listener

Runs in the listener process: callbacks queue, one reading thread per application judges and reads, the scheduler's state is under the readers' lock, its watchdog abandons and publishes.

- `HeldElements`, `Accepted` and `Judgement` go, with every call; `judge_before_reading`'s step 3 goes, so a focus passing the other checks is read in full, and the comparison is made for menu openings only.
- A menu opening is compared with the reading thread's own last read while that read is current (Dickson, 2026-10-30): no other application's UIA focus or menu opening queued, no other process's MSAA focus forwarded by the listener's WinEvent thread, and no foreground event of another top-level window than the last read's, or than the read under way once it knows its window (one during a read whose window is unknown counts as moving away), since the read's event was queued. The scheduler keeps per application `left_at`, the queue place of the newest such event; each queued event records it; `LastFocus` records its event's place and top-level window. A failed comparison drops the opening, as today. This is the third kept change.
- A reading thread clears its `LastFocus` when the scheduler answers that its read was abandoned, so an abandoned thread that goes on at the abandoned limit (`run`, lines 2639 to 2655) neither compares with nor reuses a focus Core never got.
- The scheduler moves into `crates/verbatim-outpost/src/read_schedule.rs` (planned) as `Schedule<J>`, `step(&mut self, input, now_us) -> Vec<ScheduleEffect<J>>`: inputs `Push`, `Next`, `Returned`, `Tick` (moved-on answers, each tagged with the window it was judged for; an answer for a read whose window changed since judges nothing and sets the alarm to now), `Foreground`, `MsaaFocus`, `OutpostEnded`, `WhenIdle`, `Stop`; effects `Outcome`, `Spawn`, `Wake`, `Claim`, `ForgetLast`, `Alarm`, `RunIdle`, `Job`. The shell keeps the lock, threads, condition variables, alarm, waiters' closures and the reads' progress, and applies each step's effects in order under the lock. The menu end's grace moves into the same module as `GraceState::step` (Dickson, 2026-10-30). The marker's responder waits for the messages build; the transition emits exactly one `Outcome` per marker.
- The first ownership check, `xtask/src/ownership.rs` (planned), a `cargo xtask ci` step beside `waits.rs` and `shapes.rs`: for the scheduler's module it fails on a `use` outside its allow-list (`std::collections`, `std::time::Duration`, `verbatim_model`, the listener's data types) and on any occurrence, imported or as a full path, of `thread`, `Mutex`, `Condvar`, `Atomic`, `Arc`, `mpsc`, `Instant::now`, `SystemTime`, `std::fs`, `std::io`, `std::env`, `std::process`, `std::net`, `windows::` or `unsafe`. It reads text, so a review rule covers the rest; the replay tests are the real check.

## The outposts

The event thread queues, the worker judges and reads, the watchdog publishes at the bound.

- `outpost/held.rs` keeps its place and changes its meaning: `Held` becomes the outpost's unreleased reports, each with today's record (top-level window, `entered_in_front`, MSAA event address, `simple_child`), added at the worker's commit (`Report::Reported`, `Report::Answered` with `answer_to_judge`) and removed when the worker's `release` releases its focus node, under the same locks; plus `states_read` and the reports' dialog texts. `core_focus`, `core`, `unjudged` and `newest_unjudged` go; `focuses()` iterates the unreleased reports, so `spoken_node`, `selection_states_spoken`, `known_role`, `logical_ancestors`, `msaa_selection`'s chain, `msaa_candidates` and `entered_before` keep their code; `expected()` is the newest unreleased report.
- `msaa_focus_repeats` and `foreground_repeats` go: every MSAA repeat and every foreground change in front is read in full and judged in Core.
- The ignored-window drop moves into the outpost, from the ignore list it inherits through `IGNORE_PIDS_ENV` (Dickson, 2026-10-30); the app's check stays.
- Every focus report carries window facts (Dickson, 2026-10-30). Two carry none today: a listener read whose event had no window handle and whose read found no nearest window, which only tests make (`worker.rs` lines 4343 to 4347), and a focus-now answer whose control's window was not found. Both take the application's keyboard focus window (`focus_window_of`), else, for a focus-now answer, the foreground window it read; a report with neither is not sent, which only the test path reaches; a debug assertion in `event_message` holds it.
- The gap record's restamp (`gap_accepted`, `worker.rs` line 3790) is now reached by every foreground change the outpost reads; see the open questions.

## Cost

Calls by kind, as `docs/performance.md` counts them; the builder pins each in `crates/mockapp/tests/call_counts.rs` and the document.

- A UIA repeat: 3 UIA calls before, about 5 after, plus a text control's text read. One message to Core, one reducer input and one `NodesHeld` back (sent whenever the acknowledged position moves, `live.rs` line 48), each with its reader's wake-up.
- An MSAA repeat: the role and `accFocus` before, the steady-state focus read after (20 MSAA calls); the same messages.
- A foreground change into the window of an unreleased report: local calls before, 5 to 10 MSAA calls after; the same messages; in a focus group, the older entry's read too.
- The rule: a property read (1 or 2 MSAA calls) and a message per change of an unreleased node outside Core's line while its window is in front; none otherwise.
- Removed on every focus Core judges: a supervisor owner-thread wake (`supervisor/mod.rs` line 611), one `CoreFocus` to the listener and to each of one to three outposts, each a pipe write and a reader wake-up, and Core's copy of its focus line.
- Latency on the speech path and the reducer's work per input do not rise.

## Tests

### Added

- Core (`crates/verbatim-core/tests/reduce.rs`): the repeat hole in arrival order (A at 1,000 from outpost 1, B at 1,100 from outpost 2, A at 1,200 with `earlier` `Focused`), heard as above; an MSAA repeat of the alive focus silent with its newer states kept; a change off the focus silent; the refused own-outpost foreground report leaving outpost 2's focus at 1,900 spoken; the group rule's three cases, each exact; one test per `ForegroundFact`. `notepads_foreground_report_after_the_desktops_last_focuses_is_spoken` and the two stale tests pass unchanged.
- The outpost (`held.rs`): a report stays until its focus node is released and its nodes are candidates meanwhile; name changes only on reports' focuses; a repeat by its own evidence, `earlier` absent included, leaves `states_read`; `entered_in_front` set by a foreground change. Mockapp (`msaa_events.rs`), each spoken through `SrState`: the check box's state change after its window's refused foreground report, "checked"; the window's name change when Core took the window; a change of a refused report's predecessor still held, spoken; the group's older entry reported with its rank and "Three" spoken; a change of a released line not read; the dialog text carried after another application's focus.
- The listener (`read_schedule.rs` replay tests): an event at the abandoned limit read by a returning thread; an abandoned read's late return with no outcome and a `ForgetLast`; no grace-end alarm without a foreground event; an event waiting at the limit dropped at the deadline and no pending focus elsewhere; an idle waiter run with later events queued; a grace callback before its end re-arming; one outcome per marker; menu currentness for each kind of moving away; a stale `Tick` answer re-arming. A thread-level test: an abandoned thread going on at the limit has no last read.
- Mockapp (`focus_reports.rs`): a repeated UIA focus and an MSAA repeat each read in full, with their calls; a menu opening not current judged by its own window.

### Removed, each with its cheaper replacement

- `held.rs`'s two tests and `worker.rs`'s `foreground_repeats` tests: by the new `held.rs` tests, `a_foreground_change_into_the_focused_window_is_ignored` and the group rule's tests.
- `focus_read.rs`'s `HeldElements` tests: by the replay and thread-level tests of an abandoned return.
- `notice_recipients`' test and `the_focus_core_holds_carries_its_windows_handles`: the behaviour is removed; window handles stay covered by `read.rs`'s `entered_before` tests.
- `a_focus_waiting_for_core_s_judgement_is_no_repeat`, `a_focus_is_no_repeat_once_core_holds_another`, `a_uia_focus_is_no_repeat_after_another_application_s_msaa_focus`, `a_repeated_msaa_focus_is_reported_after_another_application_s_uia_focus`: by `the_same_focus_after_another_applications_focus_is_announced`, `a_duplicate_focus_does_not_make_a_later_focus_stale`, the new Core repeat tests and the mockapp full-read tests.
- `a_repeated_focus_out_of_time_is_reported`: by `a_focus_out_of_time_is_dropped_only_by_its_windows_class`.
- The harness's `core_focus_elsewhere`, `listener_core_focus_elsewhere`, `listener_core_refuses_last_read`, `tell_core_focus`; the notice's parts of `idle_round.rs`, `spares.rs` and `tests/common`'s `held_focus`.

Rewritten: `a_repeated_focus_on_the_still_focused_element_is_dropped_before_any_read` and `a_repeated_msaa_focus_is_a_duplicate_only_of_a_full_object` become the full-read tests; `uia_repeated_focus_costs_exactly` pins the full read; `a_menu_opening_on_another_element_is_judged_by_its_own_window` makes its second read not current; `a_change_of_a_focus_core_no_longer_holds_is_not_read` delivers the release in place of the notice and asserts the same hits; `a_foreground_change_of_the_focus_s_window_is_dropped_for_the_focus_beside_it` asserts the change and the ranked focus reported, with their calls; `a_second_foreground_change_of_a_reported_window_is_dropped` asserts the second change reported and silent through `SrState`; `a_foreground_change_after_another_application_s_focus_is_reported` delivers the release of the outpost's line in place of `core_focus_elsewhere`; `a_change_of_a_focus_core_has_not_judged_yet_is_reported` keeps its sequence.

### Scenarios

The full suite once, then `text_box_editing`, `second_application_and_verbatim_menu`, `settings_system_page`, `settings_combo_box_expanded`, `explorer_folder_window`, `menu_and_settings_dialog`, `windows_terminal_two_windows`.

## The known traps, against the code

- `states_read` (review 3): the outpost's own, untouched by a repeat by its own evidence.
- A report Core can refuse (reviews 1 and 3, and this design's two reviews): what is read no longer depends on Core's verdict, only on what Core has not released.
- The menu opening's lost subject (review 1): the thread's own current last read.
- The cost of MSAA repeats and the menu's pending focus (review 1): stated; `Readers::focus_pending` stays a query of the scheduler.
- Across processes in one top-level window (review 2): each outpost reads for its unreleased nodes and Core targets; UIA property changes remain on the newest report's group, as today.
- Focus groups, the refused report's stamp, Core's focus elsewhere, an abandoned thread's last read (this design's first review): answered above.

## Dickson's decisions (2026-10-30)

- The rule above approved for study; the dialog text carried; menu currentness lost only on another top-level window; the menu end's grace as a pure transition.
- The ignored-window drop in the outpost, from the inherited ignore list; every focus report carrying window facts.
- The earlier `beside`, `shared`, `if_refused` and mirrored refusals judged workarounds; they are gone.

## Open questions for Dickson

- Focus groups: accept moving `pumpAll`'s stop into Core, every report of a group carrying its group and rank and Core taking one only when it took no newer one of its group? Recommended, yes: whether a foreground change is processed is Core's verdict, so the stop can only be decided where the verdict is, and the outpost reads the older entry only where today's copy would have dropped the change.
- UIA property changes in a shared window: remove the focus-following group from an element when Core releases its report, rather than when the next focus is followed, so a stale-refused report does not silence Core's focus's UIA changes? Recommended, yes, as the rule's UIA half: it also avoids re-adding a group to an element the focus returns to within a round trip, the churn section 17 records; the cost is a second group for one round trip and the builder's change to attribute caret and text events by their element rather than assuming they are the newest focus's. Otherwise this case stays as today.
- The refused foreground report's stamp: keep the build's rule (only the reporting outpost whose node is Core's focus leaves the stamp), or have no refused report order focuses? Recommended: keep it in this build, as it keeps today's speech; no refused report ordering focuses matches NVDA, which drops such a change before queueing it, but changes the shared-window case and belongs to the focus pipeline rework.
- The repeat hole when the repeat arrives before the other application's earlier focus: keep it as a limit, or let a duplicate advance the stale stamp? Advancing makes Core end on A, but B is then never spoken in that order and `a_duplicate_focus_does_not_make_a_later_focus_stale` changes from B spoken to silence. Recommended: keep the limit and leave the rule to the focus pipeline rework.
- The gap record's restamp by every foreground change the outpost reads, where today a change the copy dropped did not restamp: accept? Recommended, yes, since a return is what the restamp exists for, once the builder confirms that the restamp's answer to a record open in another of the application's windows reads nothing differently.
