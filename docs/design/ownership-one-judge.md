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
- The spliced ancestor: a gap in the store's record, not in the rule; see "The spliced ancestor" below.

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

An outpost tries a focus group's entries newest first, as NVDA's `pumpAll` tries a run of events, stopping at the first processed. Today the outpost dropped a foreground change of the window holding Core's focus before reading it, judged against its copy, and the older focus was tried (`a_foreground_change_of_the_focus_s_window_is_dropped_for_the_focus_beside_it`). Whether that change is processed is Core's verdict, so the stop moves into Core: every report of a group carries the group's number and its rank, and Core takes a report only when it took no newer report of the same group (Opus, 2026-10-30, pending Dickson's acceptance). The outpost reads an older entry after a reported foreground change only when the change's window holds an unreleased node of a report's focus line; otherwise the group ends at the change, as today. A reported focus ends the group, as today. So Core's focus the control in W: the change refused, the older focus taken ("Three"); Core's focus elsewhere: the change taken and W said, the older focus dropped; Core's focus another outpost's node in W: the older focus not read, as today.

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

## The spliced ancestor

The sequence: report F1 carries window node W; Core refuses F1. F2's walk meets F1's line and splices W from it (`read.rs`, `splice`, line 165) as a snapshot, with no registry lookup, so W is not recorded as carried by F2's message. Core's acknowledgement of F1 frees W, since no waiting message is recorded as carrying it. Core then takes F2 and holds W; W's next change resolves to a new node id, which Core does not hold, and drops: a lost word.

- The splice fix, in this build: every spliced entry is recorded as carried by the message it rides in (touched in its registry, as a lookup would), and the walk splices only from entries the store still holds, reading on otherwise. A few lines in `read.rs` and the worker, no call, and it keeps today's identity model: an id lives exactly as long as some message or Core may name it.
- Dickson's alternative, identity outliving the object, so a re-read element resolves to the same id and the target set adds the ids in Core's latest `NodesHeld`. Against the code:
  - UIA ids come from runtime ids (`id_for`, `id_for_element`), which a key-to-id map could outlive the element. But a runtime id is unique only among live elements and File Explorer reuses them (2026-10-07; 5797c075): an id kept past its element maps a new element to the old node, which today happens only for the focus, where the reader's `earlier` evidence corrects it (and which the paused keep-id fix in `verbatim-wt-fix2` refines). For a spliced or walked ancestor there is no such evidence, so a reused runtime id would make a different container the same object to Core's line meeting and leave it unspoken. Nor can the map be pruned by window: a runtime id names no window.
  - MSAA has no stable key for most nodes: an address identifies an object only when the object was acquired at it; an object reached through `accParent` or as a child object has a made-up address that it may share, and is found only as the same COM object, whose identity (`by_object`, the canonical `IUnknown`'s address) means nothing once the object is released. A simple child's id is reused when its list repopulates. So a re-read MSAA ancestor cannot be resolved to its old id without keeping the object, which is what release ends; matching by NVDA's equality facts alone would join different objects with equal facts.
  - Bound and cost: the UIA map would grow for the outpost's life unless capped, and a cap silently ends identity; dropping and rebuilding intermediate nodes when a walk needs them adds the walk's calls back. It is a redesign of identity, which the model places in the outpost's reducer build (the object store and the release rule) and the focus pipeline rework (one node per window).

Recommended: the splice fix in this build. It closes the sequence within today's identity model at no call; the alternative cannot give MSAA ancestors a stable id and would extend runtime-id reuse from the focus, where evidence corrects it, to ancestors, where none does.

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

- Core (`crates/verbatim-core/tests/reduce.rs`): the repeat hole in both arrival orders (A at 1,000 from outpost 1, B at 1,100 from outpost 2, A at 1,200 with `earlier` `Focused`): in order, A spoken as above; with A's repeat first, B silent and Core on A; the refused foreground report at 2,000 making the other application's focus at 1,900 stale, Core still in W; `menu_or_switch_ended` reading the applied stamp only; an MSAA repeat of the alive focus silent with its newer states kept; a change off the focus silent; the group rule's three cases; one test per `ForegroundFact`. `notepads_foreground_report_after_the_desktops_last_focuses_is_spoken` and the two stale tests pass unchanged.
- The outpost: a report stays in `Held` until its focus node is released; a node carried by a waiting message is a candidate; a spliced entry recorded as carried, and no splice from a released entry; name changes only on reports' focuses; a repeat by its own evidence leaves `states_read`; `entered_in_front` set by a foreground change; the per-element group set added at a report and removed at release. Mockapp (`msaa_events.rs`), each spoken through `SrState`: the check box's state change after its window's refused foreground report, "checked"; the window's name change when Core took the window; a change of a still-unreleased predecessor of a refused report, spoken; the group's older entry reported with its rank and "Three" spoken; a change of a released line not read; the dialog text carried after another application's focus. Mockapp (`focus_reports.rs`): a UIA change of an unreleased older focus's element heard and attributed by its runtime id.
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
- This design's reviews: focus groups by Core's group rule; the stale stamp split; an abandoned thread's last read; queued nodes in the set; the spliced ancestor.

## Dickson's decisions (2026-10-30)

- The rule: an outpost reads changes for every unreleased node.
- `NodesHeld` written on its own only when the held set changes or after a focus report.
- The ordering and applied stamps split, with the two speech changes above, subject to the builder's confirmation from NVDA's source.
- UIA's focus-following group per unreleased report, attributed by cached runtime id, its uncounted costs counted in the scenarios.
- The dialog text carried; menu currentness lost only on another top-level window; the menu end's grace as a pure transition; the ignored-window drop in the outpost; every focus report carrying window facts.

## Open questions for Dickson

- The spliced ancestor: the splice fix in this build, or identity outliving the object? Recommended: the splice fix, for the reasons in its section.
- Focus groups: accept moving `pumpAll`'s stop into Core, every report of a group carrying its group and rank? Recommended, yes: whether a foreground change is processed is Core's verdict, so the stop can only be decided where the verdict is, and the outpost reads the older entry only where today's copy would have dropped the change.
