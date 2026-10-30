# The one judge build

Status: design, written on 2026-10-30 against main at e4fecc9b, rebuilt the same day around one rule for what an outpost reads, after reviews showed that every mechanism in which an outpost guessed Core's verdict needed a patch per sequence, and revised after the rule's own review, which judged the goal achievable, with Dickson's decisions of the same day. It applies [the ownership model](ownership.md) to Core's judgement of focuses, the focus listener and the outposts' copy of Core's focus; its brief and starting draft are "The one judge build" in [the ownership builds](ownership-builds.md). The refactor preserves what is heard, except the three changes the model keeps and the two Dickson accepted below.

## Terms

- The back-channel: the `SupervisorToOutpost::CoreFocus` notice and every function that builds or reads it.
- The store: an outpost's two node registries (`crates/verbatim-uia/src/registry.rs`, `crates/verbatim-ia2/src/registry.rs`) with the outbound queue's record of which message carried each node (`crates/verbatim-outpost/src/outpost/outbound.rs`).
- An unreleased node: any node the store holds, minted, queued or sent, that it has not released.
- The release protocol: `SupervisorToOutpost::NodesHeld` (`crates/verbatim-outpost/src/protocol.rs`, line 815), the nodes Core holds in an outpost and the position of the last message Core handled; the outpost releases a node only when it was carried by a message at or before that position, no waiting message carries it, and Core does not hold it (`take_releasable`), and a node minted but never carried is kept.

## The rule

An outpost reads changes for every unreleased node, and Core's targeting decides what is spoken (Dickson, 2026-10-30). Core's focus line is always a set of nodes the outpost carried and Core still holds, so it is always inside the set; while a `NodesHeld` is in flight the outpost has released only what Core released at an acknowledged position, so the set can be larger than Core's, never smaller: a lag costs reads, never a word. `NodesHeld` is a release message about node lifetime and carries no judgement; the outpost judges nothing against Core's focus.

Queued nodes must be in the set: a change of W judged while the report that carries W, a new node, still waits in the outbound queue is read and queued behind that report, so Core holds W by the time it judges the change.

Name, value and description changes are read only for unreleased nodes that were the focus of a report, state changes for every unreleased node, since Core speaks the first kind only on its focus and the second on the focus or an ancestor; the outpost's own exclusions stay (a tree item's logical ancestors, the foreground window's own object, `is_foreground_window`).

### Manageability

What Core holds in an outpost (`SrState::held_nodes`, `state.rs` line 983): its focus and the focus's ancestors while the focus is alive; the last announced selection; the navigator object; a pending navigation's start; the caret's node, a pending caret or text request's node, a held key's read, the start marker's node, say-all's node. All but the focus line are single nodes, usually the focus itself.

- Size: one focus line plus a few nodes. A Win32 control in a dialog carries two to four nodes (mockapp's MSAA fixture: "Settings", "First"), a Settings page through UIA about ten to fifteen; the builder records `held_nodes`' size per outpost in the scenarios' flight dumps and puts the figures in `docs/performance.md`.
- Worst case: two focus lines for one release round trip, plus a line Core keeps through its navigator or an unfinished text request; the chain's depth bounds each, and nothing grows with time.
- How fast it shrinks: Core's shell writes `NodesHeld` after handling a focus report, or any message that changed the nodes Core holds there (Dickson, 2026-10-30); it reaches the outpost in one pipe hop and is handled as an intake item behind the events queued before it (`Key::NodesHeld`, a newer one replacing a waiting one).
- Call cost: changes of a window unrelated to the foreground are dropped by the acceptance test before any call. An accepted MSAA change is acquired (1 `WM_GETOBJECT`, 1 `get_accChild`, as today) and told from the candidates by its identity with no call; only a candidate's property is read, so the rule adds 1 or 2 MSAA calls and a message per change of an unreleased node outside Core's line while its window is in front.

### The sequences the reviews found

- The shared-window stale case (Settings' control A, Core's focus, inside ApplicationFrameHost's window; Settings' B in another window refused as stale): A is unreleased, so its MSAA changes are read, and its UIA changes are heard through the focus-following groups kept per unreleased element (below), in either order of arrival.
- A refused foreground report of the window holding Core's focus: the window and the control are both unreleased; "checked" on Space, and the window's name change when Core took the window, as today.
- Core's focus in another application: Core releases this outpost's line after one hop, after which none of it is read, as today after the notice.
- The repeat hole and attention: nothing is skipped as a repeat, and nothing read depends on why Core refused a report.
- Nodes put in a message without being recorded as carried: a gap in the store's record, not in the rule; see "Every carried node recorded as carried" below.

## Core's refusal paths

Every path by which Core refuses a focus report, in `crates/verbatim-core/src/reduce.rs`: `focus_dropped_first`'s application rule (line 641), duplicate rule (644) and stale rule (650); `reduce_focus_changed`'s foreground report of the window already holding Core's focus (943 to 948); `reduce_event`'s second foreground test (714 to 721, with `classify` at 300 to 310); and the app shell's ignored-window drop (`crates/verbatim-app/src/main.rs`, line 1390). Under the rule an outpost predicts none of them.

## Core

Runs on the reducer thread of `verbatim.exe`.

### The back-channel removed

- `crates/verbatim-app/src/main.rs`: `tell_core_focus`, `held_focus_owner`, `notice_recipients` and its test, and their call sites.
- `crates/verbatim-core/src/state.rs`: `focuses_queued` with its two increments, `held_focus_owner`, `held_focus`.
- `crates/verbatim-core/src/focus_rules.rs`: `HeldFocus`, `HeldFocus::for_outpost`, `RepeatEvidence::settled`.
- `crates/verbatim-outpost/src/protocol.rs`: `CoreFocus`; `supervisor/mod.rs`'s `core_focus`; `supervisor/owner.rs`'s `OwnerEvent::CoreFocus` and `tell_core_focus`.

`send_views` in the app writes `NodesHeld` on its own only when the held node set changed or the handled message was a focus report; a change of held anchors or a bare move of the acknowledged position rides on the next message written to that outpost (Dickson, 2026-10-30).

### Repeats

`is_duplicate_focus` is unchanged, with the rule for `earlier` exactly as on main. Every repeat a reader used to drop named Core's own alive focus (an MSAA repeat's node is the one minted for its address before, as `a_change_of_a_focus_core_no_longer_holds_is_not_read` asserts), so Core drops it silently and keeps its newer reading. The kept change: when another application's focus B reaches Core between A and A's return, A is spoken, as `the_same_focus_after_another_applications_focus_is_announced` pins (the expired focus speech, a stop, "Text editor", "edit").

### Ordering focuses: two facts

`latest_focus` today is written for every attended focus before `reduce_focus_changed` may refuse a foreground report, and read both by the stale rule and by `menu_or_switch_ended` (`shell.rs` line 179). It becomes two facts of the foreground record (Dickson, 2026-10-30):

- The ordering stamp, which the stale rule reads: written for every attended observation from any outpost, a focus taken, a duplicate that passes `classify`, or a refused foreground report. So the stale and second foreground tests are made before the duplicate rule and the same-window refusal, and a duplicate or refused report that passes them records its observation.
- The applied stamp, which `menu_or_switch_ended` reads: written only for a focus taken.

This replaces the earlier exception keyed by outpost. Speech changes, accepted by Dickson: with Core's check box in W, W's foreground change at 2,000 refused, and another application's focus observed at 1,900 arriving after it, the 1,900 focus is now stale and silent and Core stays in W; with A's repeat (1,200) arriving before another application's B (1,100), B is now stale and silent and Core ends on A, closing the repeat hole in that order too, and `a_duplicate_focus_does_not_make_a_later_focus_stale` is rewritten to assert it. Before building, the builder confirms from the pinned NVDA source that NVDA's in-order queue ends on W and on A respectively (`docs/parity/focus-events.md`, "Stale focus events", and the duplicate and foreground rules it cites), and stops if not.

### Focus groups

An outpost tries a focus group's entries newest first, as NVDA's `pumpAll` tries a run of events, stopping at the first processed. Today the outpost dropped a foreground change of the window holding Core's focus before reading it, judged against its copy, and the older focus was tried (`a_foreground_change_of_the_focus_s_window_is_dropped_for_the_focus_beside_it`). Whether that change is processed is Core's verdict, so the stop moves into Core: every report of a group carries the group's number and its rank, and Core takes a report only when it took no newer report of the same group (Dickson, 2026-10-30). The outpost reads an older entry after a reported foreground change only when the change's window holds an unreleased node of a report's focus line; otherwise the group ends at the change, as today. A reported focus ends the group, as today. So Core's focus the control in W: the change refused, the older focus taken ("Three"); Core's focus elsewhere: the change taken and W said, the older focus dropped; Core's focus another outpost's node in W: the older focus not read, as today.

### One foreground record

A new module, `crates/verbatim-core/src/foreground.rs` (planned): one type `Foreground`, private fields, one `apply(&mut self, ForegroundFact)`, read-only accessors, replacing `attention`, `foreground` (the object), `foreground_changed_at`, `foreground_window` and `latest_focus`. One variant per writer: `Reported`, `Unreported`, `Object`, `LineCut`, `Window`, `Observed` (the ordering stamp) and `Applied` (the applied stamp). `classify` takes `&Foreground`, keeping its second test. `SrState` holds the record with `#[serde(flatten)]`; `latest_focus` keeps its name as the ordering stamp and the applied stamp is a new field defaulting to it, so `tests/fixtures/focus_value_states.jsonl` and earlier dumps still load.

## The listener

Runs in the listener process: callbacks queue, one reading thread per application judges and reads, the scheduler's state is under the readers' lock, its watchdog abandons and publishes.

- `HeldElements`, `Accepted` and `Judgement` go; `judge_before_reading`'s step 3 goes, so a focus passing the other checks is read in full, and the comparison is made for menu openings only.
- A menu opening is compared with the reading thread's own last read while that read is current (Dickson, 2026-10-30): no other application's UIA focus or menu opening queued, no other process's MSAA focus forwarded by the listener's WinEvent thread, and no foreground event of another top-level window than the last read's, or than the read under way once it knows its window (one during a read whose window is unknown counts as moving away), since the read's event was queued. The scheduler keeps per application `left_at`, the queue place of the newest such event; each queued event records it; `LastFocus` records its event's place and top-level window. A failed comparison drops the opening, as today. This is the third kept change.
- A reading thread clears its `LastFocus` when the scheduler answers that its read was abandoned, so an abandoned thread going on at the abandoned limit (`run`, lines 2639 to 2655) neither compares with nor reuses a focus Core never got.
- The scheduler moves into `crates/verbatim-outpost/src/read_schedule.rs` (planned) as `Schedule<J>`, `step(&mut self, input, now_us) -> Vec<ScheduleEffect<J>>`: inputs `Push`, `Next`, `Returned`, `Tick` (moved-on answers, each tagged with the window it was judged for; an answer for a read whose window changed since judges nothing and sets the alarm to now), `Foreground`, `MsaaFocus`, `OutpostEnded`, `WhenIdle`, `Stop`; effects `Outcome`, `Spawn`, `Wake`, `Claim`, `ForgetLast`, `Alarm`, `RunIdle`, `Job`. The shell keeps the lock, threads, condition variables, alarm, waiters' closures and the reads' progress, and applies each step's effects in order under the lock. The menu end's grace moves into the same module as `GraceState::step` (Dickson, 2026-10-30). The marker's responder waits for the messages build; the transition emits exactly one `Outcome` per marker.
- The first ownership check, `xtask/src/ownership.rs` (planned), a `cargo xtask ci` step beside `waits.rs` and `shapes.rs`: for the scheduler's module it fails on a `use` outside its allow-list (`std::collections`, `std::time::Duration`, `verbatim_model`, the listener's data types) and on any occurrence, imported or as a full path, of `thread`, `Mutex`, `Condvar`, `Atomic`, `Arc`, `mpsc`, `Instant::now`, `SystemTime`, `std::fs`, `std::io`, `std::env`, `std::process`, `std::net`, `windows::` or `unsafe`. It reads text, so a review rule covers the rest; the replay tests are the real check.

## The outposts

The event thread queues, the worker judges and reads, the watchdog publishes at the bound.

### The reports, from the store

`outpost/held.rs` keeps its place and changes its meaning: `Held` holds the outpost's reports whose focus node is unreleased, each with today's record (top-level window, `entered_in_front`, MSAA event address, `simple_child`), added at the worker's commit (`Report::Reported`, `Report::Answered` with `answer_to_judge`) and removed by the worker's `release` under the same locks, plus `states_read` and the reports' dialog texts. `core_focus`, `core`, `unjudged` and `newest_unjudged` go; `focuses()` iterates these reports, so `spoken_node`, `selection_states_spoken`, `known_role`, `logical_ancestors`, `msaa_selection`'s chain, `msaa_candidates` and `entered_before` keep their code; `expected()` is the newest of them. The candidates for a change are every unreleased node of these reports' lines and every unreleased node a waiting message carries. `msaa_focus_repeats` and `foreground_repeats` go: every MSAA repeat and every foreground change in front is read in full and judged in Core.

What else the outpost decides, each from its own facts:

- `Tracking` (`commit.rs`): the caret, text and terminal output follow the newest report of a control, as today.
- `states_read`: its own; a repeat by its own evidence (a report of the newest report's node whose evidence says the same element still has the focus: UIA `earlier` `Focused` or absent, MSAA the same address not acquired as a simple child) does not update it, and moves no focus-following group.
- `entered_in_front`: inherited from the newest report in the same top-level window, and set to the window by every foreground change of it the outpost handles.
- The walk's bound (`walk_left`): today's rule against `expected()`; it differs from today only while a line Core no longer holds as its focus is unreleased, and then only in timing.
- Dialog text: a dialog counts as entered against every report's line, a superset, so each report carries the text gathered when its line first entered the dialog, with no call (Dickson, 2026-10-30); Core speaks it only when its own line enters the dialog.
- A selection read against the focus (`expected_uia_focus`, `selection_as_value`): against every report's focus.
- The gap record's restamp (`gap_accepted`, `worker.rs` line 3790): reached by every foreground change the outpost reads; where today's copy dropped the change, the restamp now also answers a record open in another of the application's windows, removing a silent duplicate read.

### UIA: one focus-following group per unreleased report

UIA property, text and caret changes reach an outpost only through its focus-following group (`docs/design/focus-pipeline.md`, section 17). So the group stays on the element of every report whose focus node is unreleased and is removed when the store releases that node, not when the next focus is followed (Dickson, 2026-10-30): `Tracking`'s `retarget` becomes a per-element set, adding as today and removing at release, which also avoids re-adding a group to an element the focus returns to within a round trip, the churn section 17 records. An event is attributed to its node by its cached runtime id (`existing_id`), with no `CompareElements`; the comment at `worker.rs` line 3290 that the subscription's caret and text events are the focus's is replaced by that attribution, and a caret or text event of an element that is not the newest report's is read for that node, which Core targets.

Uncounted costs, which the builder counts in `text_box_editing` and `settings_system_page` and records in `docs/performance.md`: `nearest_window_handle`'s one normalize call for an event of a XAML element with no cached window that is not the newest focus; `caret_of`'s text calls for a `TextSelection` event of an old element; the refresh made for a `HelpText` or `ControllerFor` change of an old element; and the second group for one round trip.

### Reports

- The ignored-window drop moves into the outpost, from the ignore list it inherits through `IGNORE_PIDS_ENV` (Dickson, 2026-10-30); the app's check stays.
- Every focus report carries window facts (Dickson, 2026-10-30). Two carry none today: a listener read whose event had no window handle and whose read found no nearest window, which only tests make (`worker.rs` lines 4343 to 4347), and a focus-now answer whose control's window was not found. Both take the application's keyboard focus window (`focus_window_of`), else, for a focus-now answer, the foreground window it read; a report with neither is not sent; a debug assertion in `event_message` holds it.

## Every carried node recorded as carried

The store releases a node only when no waiting message is recorded as carrying it (`take_releasable`), and a message records only the nodes its registries issued or looked up while it was built (`take_touched`). Two paths put a node in a message without that record, so an older acknowledgement can free a node a newer message still carries; Core then holds it, and its next change resolves to a new id that Core drops, a lost word:

- The walk's splice: report F1 carries window node W and Core refuses it; F2's walk meets F1's line and splices W from it (`read.rs`, `splice`, line 165) as a snapshot; Core's acknowledgement of F1 frees W; Core takes F2 and holds W.
- `existing_id` lookups, which return a node "without issuing a node or recording it as reported" (`crates/verbatim-uia/src/registry.rs`, lines 143 to 153): a caret event (`worker.rs` line 2916), a text change (3297, 3359) and an active text position (3388) each send a message naming a node found so, which an acknowledgement of the node's last recorded carrier frees while that message waits.

The fix (Dickson, 2026-10-30): every node a message carries is recorded as carried by it, spliced entries and `existing_id` results included, and a walk splices only from entries the store still holds, reading on otherwise. A few lines in `read.rs`, the worker and the registries, no call; an id lives exactly as long as some message or Core may name it.

The alternative weighed, identity outliving the object, so a re-read element resolves to its old id: UIA runtime ids are reused after their element dies (5797c075), which today reaches only the focus, where the reader's `earlier` evidence corrects it, and would reach ancestors, where nothing does; most MSAA nodes have no stable key, since an object reached through `accParent` is found only as the same COM object, whose identity ends with its release, and a simple child's id is reused when its list repopulates; and a key-to-id map would grow for the outpost's life. It is a redesign of identity, left to the outpost's reducer build and the focus pipeline rework.

## Text anchors released by message position

Today an outpost replaces its held anchor set when Core's list arrives (`outpost/mod.rs`, lines 1510 to 1516) and forgets any other anchor of a node once 64 newer exist for it (`text/mod.rs`, lines 740 to 768, `KEPT_ANCHORS`). An anchor Core took after its last list was sent is in neither: with follow-caret off, typing more than 64 caret reports before Core's next list forgets the review cursor's anchor, so the next review command restarts from the caret and speaks the wrong line (`review_text.rs`, lines 756 to 766), and say-all stops silently (`say_all.rs`, line 180).

Anchors are released as nodes are (Dickson, 2026-10-30): an anchor is forgotten only when a `NodesHeld` acknowledges a position at or after the last message that carried it and does not list it, and an anchor minted but never carried is kept. The 64-anchor slack goes. A change of Core's anchor list does not write `NodesHeld` on its own, since the caret's anchor changes with every key and a write per key costs a pipe write and the outpost's reader wake on the typing path; it rides on the next message to that outpost or the relay's 3-second ping, which bounds how long an outpost keeps an anchor Core has dropped (Dickson, 2026-10-30).

## At a generation change

An outpost's generation is the `OutpostId` stamped on every node id it reports (`protocol.rs`, `assign_outpost`, lines 1296 to 1300); a replacement outpost for the same application is a new generation, whose counters start again.

- Focus groups are keyed by the generation of the report's node with the group's number, since group numbers restart with each generation.
- The applied stamp is also written by the silent re-read's early return (`reduce.rs`, lines 968 to 980), where a replacement generation's report of the focus already heard is taken silently.
- The stale rule compares generations, not applications (`reduce.rs`, line 369): a replacement's report is judged against its predecessor's newest focus as another outpost's. That is safe because its first report, the focus-now answer, carries the time its read began (`requests.rs`, line 470), after the predecessor's last observation.

## Readers that decide what to read

Every remaining place where a reader decides what to read or watch, checked for a guess of Core's verdict:

- MSAA and UIA caret events, text changes and active text positions (`worker.rs` `caret_event`, `caret_of`, the text events): read for any node the registries hold, a superset; sound, once their lookups are recorded as carried (above).
- A caret key's watch and the console caret poll: started by Core's request, which names its node; sound.
- The text read with a focus report (`read_focus_text`): the report's own node; sound.
- A terminal's new output, on its caret and text events and its console's update events (`focused_terminal`, `focused_console_terminal`, `held_with_changes`): read only for the newest report's terminal (the intake's focused object and `Tracking`), a guess when Core holds an older terminal focus that the store has not released. Fixed in this build: output is read for every unreleased terminal node with a follow entry, whose entries are already released with their nodes; Core speaks output only for its focus (`terminal_model::snapshot_event`, `focus_matches`), so the extra reads cost calls during one round trip and add no word.
- `applications::refuses_focus`: used only by Core; no outpost applies it.
- The supervisor's `HeldFacts`: facts held while an application's outpost starts, released in arrival order with NVDA's one-per-object rule; no judgement of Core's state; sound.
- The supervisor's view of attention and of the outposts Core holds nodes in (`OwnerEvent::Views`): retirement only, the lagging copy for lifecycle the model allows; sound.

## Cost

Calls by kind, as `docs/performance.md` counts them; the builder pins each in `crates/mockapp/tests/call_counts.rs` and the document.

- A UIA repeat: 3 UIA calls before, about 5 after, plus a text control's text read; one message to Core, one reducer input, and the `NodesHeld` written after a focus report, each with its reader's wake-up.
- An MSAA repeat: the role and `accFocus` before, the steady-state focus read after (20 MSAA calls); the same messages.
- A foreground change into the window of a report: local calls before, 5 to 10 MSAA calls after; the same messages; in a focus group, the older entry's read too.
- The rule: 1 or 2 MSAA calls and a message per change of an unreleased node outside Core's line while its window is in front; the UIA costs listed above.
- `NodesHeld` after each focus report costs roughly what today's `CoreFocus` to the focus's outpost costs: one pipe write and the outpost's reader wake. The saving is the `CoreFocus` notices to the listener and to the other outposts concerned, and the supervisor owner-thread wake each caused (`supervisor/mod.rs` line 611).
- Latency on the speech path and the reducer's work per input do not rise.

## Tests

### Added

- Core (`crates/verbatim-core/tests/reduce.rs`): the silent re-read writing the applied stamp; the repeat hole in both arrival orders (A at 1,000 from outpost 1, B at 1,100 from outpost 2, A at 1,200 with `earlier` `Focused`): in order, A spoken as above; with A's repeat first, B silent and Core on A; the refused foreground report at 2,000 making the other application's focus at 1,900 stale, Core still in W; `menu_or_switch_ended` reading the applied stamp only; an MSAA repeat of the alive focus silent with its newer states kept; a change off the focus silent; the group rule's three cases; one test per `ForegroundFact`. `notepads_foreground_report_after_the_desktops_last_focuses_is_spoken` and the two stale tests pass unchanged.
- The outpost: a report stays in `Held` until its focus node is released; a node carried by a waiting message is a candidate; a spliced entry and an `existing_id` result recorded as carried, and no splice from a released entry; an anchor kept until an acknowledgement past its carrier omits it; a focus group keyed by its generation; output read for an unreleased older terminal; name changes only on reports' focuses; a repeat by its own evidence leaves `states_read`; `entered_in_front` set by a foreground change; the per-element group set added at a report and removed at release. Mockapp (`msaa_events.rs`), each spoken through `SrState`: the check box's state change after its window's refused foreground report, "checked"; the window's name change when Core took the window; a change of a still-unreleased predecessor of a refused report, spoken; the group's older entry reported with its rank and "Three" spoken; a change of a released line not read; the dialog text carried after another application's focus. Mockapp (`focus_reports.rs`): a UIA change of an unreleased older focus's element heard and attributed by its runtime id.
- The listener (`read_schedule.rs` replay tests): an event at the abandoned limit read by a returning thread; an abandoned read's late return with no outcome and a `ForgetLast`; no grace-end alarm without a foreground event; an event waiting at the limit dropped at the deadline and no pending focus elsewhere; an idle waiter run with later events queued; a grace callback before its end re-arming; one outcome per marker; menu currentness for each kind of moving away; a stale `Tick` answer re-arming. A thread-level test: an abandoned thread going on at the limit has no last read.
- Mockapp (`focus_reports.rs`): a repeated UIA focus and an MSAA repeat each read in full, with their calls; a menu opening not current judged by its own window.

### Removed, each with its cheaper replacement

- `held.rs`'s two tests and `worker.rs`'s `foreground_repeats` tests: by the new `held.rs` tests, `a_foreground_change_into_the_focused_window_is_ignored` and the group rule's tests.
- `focus_read.rs`'s `HeldElements` tests: by the replay and thread-level tests of an abandoned return.
- `notice_recipients`' test and `the_focus_core_holds_carries_its_windows_handles`: the behaviour is removed; window handles stay covered by `read.rs`'s `entered_before` tests.
- `a_focus_waiting_for_core_s_judgement_is_no_repeat`, `a_focus_is_no_repeat_once_core_holds_another`, `a_uia_focus_is_no_repeat_after_another_application_s_msaa_focus`, `a_repeated_msaa_focus_is_reported_after_another_application_s_uia_focus`: by `the_same_focus_after_another_applications_focus_is_announced`, the new Core repeat tests and the mockapp full-read tests.
- `a_repeated_focus_out_of_time_is_reported`: by `a_focus_out_of_time_is_dropped_only_by_its_windows_class`.
- The harness's `core_focus_elsewhere`, `listener_core_focus_elsewhere`, `listener_core_refuses_last_read`, `tell_core_focus`; the notice's parts of `idle_round.rs`, `spares.rs` and `tests/common`'s `held_focus`.

Rewritten: `a_duplicate_focus_does_not_make_a_later_focus_stale` asserts B silent and Core on A; `a_repeated_focus_on_the_still_focused_element_is_dropped_before_any_read` and `a_repeated_msaa_focus_is_a_duplicate_only_of_a_full_object` become the full-read tests; `uia_repeated_focus_costs_exactly` pins the full read; `a_menu_opening_on_another_element_is_judged_by_its_own_window` makes its second read not current; `a_change_of_a_focus_core_no_longer_holds_is_not_read` delivers the release in place of the notice and asserts the same hits; `a_foreground_change_of_the_focus_s_window_is_dropped_for_the_focus_beside_it` asserts the change and the ranked focus reported, with their calls; `a_second_foreground_change_of_a_reported_window_is_dropped` asserts the second change reported and silent through `SrState`; `a_foreground_change_after_another_application_s_focus_is_reported` delivers the release of the outpost's line in place of `core_focus_elsewhere`; `a_change_of_a_focus_core_has_not_judged_yet_is_reported` keeps its sequence.

### Scenarios

The full suite once, then `text_box_editing`, `second_application_and_verbatim_menu`, `settings_system_page`, `settings_combo_box_expanded`, `explorer_folder_window`, `menu_and_settings_dialog`, `windows_terminal_two_windows`.

## The known traps, against the code

- `states_read` (review 3): the outpost's own, untouched by a repeat by its own evidence.
- A report Core can refuse (every review): what is read depends only on what the store has not released.
- The menu opening's lost subject (review 1): the thread's own current last read.
- The cost of MSAA repeats and the menu's pending focus (review 1): stated; `Readers::focus_pending` stays a query of the scheduler.
- Across processes in one top-level window (review 2): each outpost reads for its unreleased nodes, UIA through its per-element groups, and Core targets.
- This design's reviews: focus groups by Core's group rule; the stale stamp split; an abandoned thread's last read; queued nodes in the set; every carried node recorded; anchors released by position; generation changes; terminal output for every unreleased terminal.

## Dickson's decisions (2026-10-30)

- The rule: an outpost reads changes for every unreleased node.
- `NodesHeld` written on its own only when the held set changes or after a focus report.
- The ordering and applied stamps split, with the two speech changes above, subject to the builder's confirmation from NVDA's source.
- UIA's focus-following group per unreleased report, attributed by cached runtime id, its uncounted costs counted in the scenarios.
- Focus groups' stop in Core; every carried node recorded as carried, the splice included; text anchors released by message position.
- The dialog text carried; menu currentness lost only on another top-level window; the menu end's grace as a pure transition; the ignored-window drop in the outpost; every focus report carrying window facts.
