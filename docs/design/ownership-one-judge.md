# The one judge build

Status: design, written on 2026-10-30 against main at e4fecc9b, for adversarial review before it is built. It applies [the ownership model](ownership.md) to Core's judgement of focuses, the focus listener and the outposts' copy of Core's focus; its brief and starting draft are "The one judge build" in [the ownership builds](ownership-builds.md). The refactor preserves what is heard, except the three changes the model keeps; each place where this design would change speech is named and either answered or put to Dickson under "Open questions".

## Terms

- The back-channel: the `SupervisorToOutpost::CoreFocus` notice, by which Core's focus reaches the listener and the outposts, and every function that builds or reads it.
- The followed focus: what an outpost judges its application's events against before it reads them, built from its own reports only.
- A repeat by its own evidence: a report whose reader's own evidence says it names the element its previous report named, still focused: for UIA, `earlier` is `Focused`; for MSAA, the event address of the followed focus, which was not acquired as a simple child.

## Core's refusal paths

The first step of the brief. Every path by which Core refuses a focus report of an application, in `crates/verbatim-core/src/reduce.rs`:

- `focus_dropped_first`, line 641: an application rule, `applications::refuses_focus` (File Explorer's task view frame and the Alt+Tab switcher's input site pane, read through UIA, not a foreground report). The application rules.
- `focus_dropped_first`, line 644: `is_duplicate_focus`, whose newer reading is kept unspoken. The duplicate rule.
- `focus_dropped_first`, line 650: `is_stale_focus`. The stale rule.
- `reduce_focus_changed`, lines 943 to 948: a foreground report whose top-level window is that of Core's focus. A foreground report of the window already holding Core's focus.

All four are the design's four. Two more refusals lie outside those two functions and outside the four, and the followed focus must answer both:

- `reduce_event`, lines 714 to 721, with `classify`'s arm at lines 300 to 310: a focus whose own window facts are unrelated to the foreground (`ForegroundRelation::of_facts(..).related()`), or, with no window facts, whose application does not hold attention. This is Core's second foreground test, which Dickson kept in the refactor (audit difference 12).
- `crates/verbatim-app/src/main.rs`, line 1390, `in_ignored_window`: the app shell drops, before Core, a focus whose top-level window belongs to a process Verbatim ignores (`IGNORE_PIDS_ENV`, set only by the end-to-end harness).

The reducer's other early returns in `reduce_focus_changed` are not refusals: the silent re-read (lines 969 to 982), the nameless foreground window and the shell's staging windows (lines 1022 to 1026) all take the report as Core's focus.

## Core

Runs on the reducer thread of `verbatim.exe`.

### The back-channel removed

- `crates/verbatim-app/src/main.rs`: `tell_core_focus`, `held_focus_owner`, `notice_recipients` and its test, and the two call sites (the ignored-window drop and `apply`'s before-and-after comparison).
- `crates/verbatim-core/src/state.rs`: `focuses_queued` (field and accessor; its two increments in `reduce_focus_changed` go), `held_focus_owner`, `held_focus`.
- `crates/verbatim-core/src/focus_rules.rs`: `HeldFocus` and `HeldFocus::for_outpost`; `RepeatEvidence::settled`, which only the listener and the outposts set false. `HeldNode` stays, as the outpost's own record of a node (`EnteredKey`).
- `crates/verbatim-outpost/src/protocol.rs`: the `CoreFocus` variant and its stamping arms; `supervisor/mod.rs`'s `core_focus`; `supervisor/owner.rs`'s `OwnerEvent::CoreFocus` and `tell_core_focus`; the mention in `supervisor/writer.rs`.

Nothing replaces them: no message tells a reader what Core holds, and no API exports Core's focus, so none can be added back without a new function in `state.rs`.

### Repeats judged only in Core

`is_duplicate_focus` stays the one repeat judge, unchanged, with the rule for `earlier` exactly as on main, so the paused keep-id fix rebases cleanly. Every repeat a reader used to drop now reaches it:

- A UIA repeat carries the listener's `same_as_last` as `earlier`: `Focused` when the reading thread's own last element under that runtime id still has the keyboard focus, so Core drops it, keeping its newer reading. Where the thread has no such element (a new thread after an abandonment), `earlier` is `None` and Core judges by the node still alive, as today for such reports.
- An MSAA repeat carries no `earlier`, as every MSAA report does today; its node id is the one the outpost minted for that address before (`a_change_of_a_focus_core_no_longer_holds_is_not_read` already asserts this), so Core finds the same node, alive, and drops it.

So preserving today's speech needs no MSAA address or simple-child mark in Core: every repeat an outpost dropped named the node of Core's own focus, alive, which Core drops by the existing rule, and a simple child's repeat already reaches Core today. The draft's "MSAA event address, simple-child mark" are not added in this build.

Speech: a repeat says nothing, as before. The kept change, the repeat hole: a focus A (Notepad's edit, "Text editor") held by Core, another application's focus B reported and not yet judged, then A again. Before, the reader dropped A's return against its copy saying Core held A, and Core then took B, so A was never spoken. Now A is read and reported, Core's focus is B, and Core says exactly what `the_same_focus_after_another_applications_focus_is_announced` pins: the expired focus speech, a stop, then "Text editor", "edit", queued.

### One foreground record

A new module, `crates/verbatim-core/src/foreground.rs` (planned), with one type `Foreground` whose fields are private to the module, one `apply(&mut self, ForegroundFact)`, and read-only accessors. It replaces five notions in `SrState`:

- `attention` (written at reduce.rs 939 and 1085), read by `classify`, `window_is_attended`, `follow_an_unreported_foreground`, the shell's attention view and `SrState::attention`.
- `foreground`, the foreground object (written at 1004 and 1089), read by `FocusNow`.
- `foreground_changed_at` (written at 1001), read by `observed_before_foreground_change`.
- `foreground_window`, the listener's window in front (written in `shell.rs` line 51), read by `hook_foreground`.
- `latest_focus`, whose top-level window the stale rule's same-window exception reads (written at 728), also read by `menu_or_switch_ended`. It moves whole, its outpost and observation time with its window, so the stale rule reads one owner.

`ForegroundFact` has one variant per writer above, each doing exactly today's assignment at today's place: `Reported` (a foreground report taken: attention), `Unreported` (a focus in front unrelated to attention: attention and the object), `Object` (a foreground report becoming the focus), `LineCut` (the stamp), `FocusApplied` (the newest applied focus), and `Window` (the listener's `ForegroundWindow`). `classify` takes `&Foreground` in place of `Option<&Attention>`, so its second test stays, as a function of the record; its removal belongs to the focus pipeline rework. `SrState` holds the record with `#[serde(flatten)]` and today's field names, so `tests/fixtures/focus_value_states.jsonl` and every earlier flight dump still load.

Behaviour is unchanged by construction: the same values are written at the same points and read by the same rules. Evidence for the one writer: b1917eed, audit difference 12, and the Settings chain (5f1c8e81, 3578a437, e60a7a58, 38cab859).

## The listener

Runs in the listener process: the UIA callbacks queue events; one reading thread per application judges and reads; the scheduler's state is guarded by the readers' lock; its watchdog thread abandons and publishes.

### `HeldElements` removed

`crates/verbatim-outpost/src/focus_read.rs`: `HeldElements`, `Accepted`, `Judgement` and every `accept`, `core_focus`, `abandoned` and `release` call; `FocusReader::new` loses its argument; `listener.rs` drops the field and its `CoreFocus` arm. Each reader's `LastFocus` already holds its element and releases it on its own thread when the reader ends.

In `judge_before_reading`, step 3 (the repeat rule) goes: a focus passing steps 1, 2, 4, 5 and 6 is read in full. The comparison (`compare`) is made for menu openings only.

### The menu opening's comparison

NVDA compares a menu opening's sender with its focus object. The listener compares it with the reading thread's own last read (`LastFocus`) while that read is current, and judges it with that read's window when they are the same element. The read is current when no focus or menu opening of another application has been queued, and no foreground event of a top-level window other than the read's own has been delivered, since the read's event was queued.

How it is known, with no shared state beyond the scheduler's: the scheduler keeps, for each application, `left_at`, the place in the queue order of the newest event that moved NVDA's focus away from it (a focus or menu opening of another application, or a foreground event of another top-level window than its last read's); each queued event records `left_at` as it was queued; `LastFocus` records its event's place and its top-level window (already learned in `Watched::top_level`, now returned with the outcome). A menu opening compares only when the last read's place is greater than its recorded `left_at`. A failed comparison drops the opening, as today; the opening is read either way it is judged. This is the third kept change: before, it was compared with the element of Core's focus, from whichever reader.

Speech: the same as today whenever Core's focus is the last focus that application's thread read, which is the usual case (a context menu or a split button opened on the focus). It differs only where the two differ: the menu then is judged by its own nearest window and NVDA's native check, and Core speaks it as the focus on the menu it reports.

### The scheduler as a pure transition

`ReadersState` and its decisions move into a new module, `crates/verbatim-outpost/src/read_schedule.rs` (planned): `Schedule<J>`, with `step(&mut self, input, now_us) -> Vec<ScheduleEffect<J>>`, no clock, thread, lock, channel, atomic or binding. Time is the run's shared microseconds, passed in.

- Inputs: `Push` (pid, marker, kind, key, window), `Next` (a thread asks for its next job), `Returned` (pid, generation, the outcome's kind and top-level window), `Tick` (the watchdog's wake, with the moved-on answers it gathered with local calls), `Foreground` (top-level window), `OutpostEnded`, `WhenIdle` (a waiter's number), `Stop`.
- Effects: `Outcome` (send this marker's result or drop), `Spawn` (pid, generation), `Wake` (pid), `Claim` (mark a read abandoned or published, which the shell writes into the read's `WatchedRead` for its thread and its progress publication), `Alarm` (set to an instant, or cancel), `RunIdle` (a waiter's number), and `Job` (handed to the asking thread).
- The shell, `Readers`, keeps the lock, the threads, the condition variables, the alarm, the idle waiters' closures by number, and the `WatchedRead` progress; it applies each step's effects in order under the lock, as outcomes are sent under it today, so no ordering of sends changes.
- A late `Returned` from an abandoned or published generation yields no `Outcome` and no last read, the case e46d4183 fixed in `HeldElements::accept`.
- The menu end's grace (`MenuEnd`'s `Grace` in `listener.rs`) moves into the same module as a second small transition, `GraceState::step(now_us, input)`, its threadpool timer staying in the shell, because 69c06239's ordering is in it.

The marker's responder, whose `Drop` sends a drop (0b5826cd), waits for the messages build; the transition already emits exactly one `Outcome` per pushed marker, which its replay tests assert.

### The first ownership check

A new `cargo xtask ci` step, `xtask/src/ownership.rs` (planned), beside `waits.rs` and `shapes.rs`: for each module on its list, today the scheduler's module alone, it reads the source and fails on any `use` outside the module's allow-list (`std::collections`, `std::time::Duration`, `verbatim_model`, the listener's own data types) and on any of the names `thread`, `Mutex`, `Condvar`, `Atomic`, `Arc`, `mpsc`, `Instant::now`, `SystemTime`, `windows::`, `unsafe`. It reads text, so a review rule covers what it cannot see; the replay tests are the real check. One unit test of the checker: a source using `std::thread::spawn` fails, and the scheduler's passes.

## The outposts

Runs in each outpost: the event thread queues, the worker judges and reads, the watchdog publishes at the bound; the followed focus is guarded by its own lock (today `Context::held`), never held across a call.

### `Held` becomes the followed focus

`crates/verbatim-outpost/src/outpost/held.rs` is replaced by `followed.rs`, `Followed`, a monitor: one module, private fields, its lock never held across a call. Its state:

- The followed focus: the node and chain of the newest report, with what `held::Focus` records today (top-level window, `entered_in_front`, the MSAA event address, `simple_child`), taken from the outpost's own report as the worker commits it (`commit.rs`, `Report::Reported` and `Report::Answered`, `answer_to_judge` kept).
- `beside`: the window node of a foreground report that did not replace the followed focus, kept as a second change target until the next report.
- `states_read`, the outpost's own, unchanged in meaning.
- The dialog texts gathered for the dialogs in the followed line (see "Dialog text" below).

A report replaces the followed focus, except:

- A report an application rule refuses, judged by the outpost through Core's one definition (`applications::refuses_focus`, made `pub` in `verbatim-core`, which the outpost already depends on): the Alt+Tab switcher's pane read through UIA.
- A report with window facts unrelated to the foreground, by `ForegroundRelation::of_facts(&facts).related()` on the facts it reports, which is Core's second test on the same facts (Opus, 2026-10-30, pending Dickson's acceptance). A report with no window facts is followed, since Core judges it by attention, which the outpost cannot see; the builder lists which reports carry none.
- A report in a window of an ignored process, judged from the ignore list the outposts inherit through `IGNORE_PIDS_ENV` (Opus, 2026-10-30, pending Dickson's acceptance).
- A foreground report of the top-level window holding the followed focus: Core ignores it while its focus is in that window. It becomes `beside` instead.

A repeat by its own evidence replaces the followed focus's reading but leaves `states_read` as it is and moves no subscription (the `adopt: false` path a same-node focus-now answer takes today), so a repeat read before its state change cannot take the "newly expanded" judgement or the child count from the change.

`Tracking` (`commit.rs`) stays separate: it follows controls only, never a foreground report, for the caret, text and the focus-following subscription.

### Judgements moved onto it

Each use of `Held` judges against the followed focus and `beside` instead of Core's focus and the unjudged reports: `spoken_node` and `selection_states_spoken` (change targeting), `known_role`, `logical_ancestors`, `expected_uia_focus`, `msaa_selection`'s chain, `walk_left`'s bound, `entered_before`, and `read::msaa_candidates` (MSAA equality's candidates from the followed line). `msaa_focus_repeats` and `foreground_repeats` go: every MSAA repeat and every foreground change in front is read in full and judged in Core.

### Dialog text

`describe_dialogs` gathers a dialog's text only when the new line enters it, judged today against Core's line. Against the followed line, a dialog would count as already entered after another application's focus, so the text would be lost in two sequences: Alt+Tab away from a message box and back (the foreground report of the box's window, which Core takes since its focus is elsewhere), and a topmost menu of another process closing over the box (the button's report, which Core takes as entering the box). The followed record keeps each dialog's text as gathered when the line entered it, and every later report whose chain holds that dialog carries it, with no call (Opus, 2026-10-30, pending Dickson's acceptance). Core speaks a dialog's description only when its own line enters it, so words are Core's; the text can be older than a fresh read if the dialog's text changed while focus stayed in it.

### Edge cases and speech

- Space on a check box after its window's foreground report: Core's focus is the box, in window W; the foreground report of W is refused by Core and kept `beside` by the outpost; the state change is a change target and Core says "checked", as today.
- The same, with Core's focus elsewhere (a foreground return from another process's popup that took no focus): Core takes W as its focus and says W as today; the outpost follows the box and keeps W `beside`, so W's name change is read and spoken as today, and the box's changes are sent and dropped by Core's targeting. Messages are added, never words. The draft's "only messages added" missed W's own name, value and description changes, which `beside` answers.
- Across processes in one top-level window (ApplicationFrameHost's frame around Settings' page): each outpost follows its own; Core's targeting decides. Messages are added, never words.
- A change of the followed focus after the focus moved to another application whose window is topmost or related to this one's (another process's menu, the switcher): read and sent, and dropped by Core, where the back-channel had the outpost skip the read. Events of windows unrelated to the foreground are still dropped by the acceptance test before any call.
- The stale rule while Core's focus is still this outpost's: possible only after another process's foreground report of the window holding Core's focus was refused, then a stale report of this outpost from another of its top-level windows. The outpost follows the stale report, so the old focus's changes are no longer read. See the open questions.

## Cost

Calls are counted by kind, as `docs/performance.md` does; the builder pins every figure in `crates/mockapp/tests/call_counts.rs` and updates the document in the same commit.

- A UIA repeat: 3 UIA calls before (nearest window, comparison, keyboard focus), about 5 after (the model's figure: nearest window, keyboard focus, and the read's earlier-element check, cache and one step up), plus the outpost's report and any text read of a text control, as for any focus. One more message to Core and one reducer input per repeat.
- An MSAA repeat: the role and `accFocus` before (mockapp: 4 provider calls and 1 `WM_GETOBJECT`), the steady-state focus's read after, 20 MSAA calls by `msaa_focus_changes_cost_exactly`. One more message and reducer input.
- A foreground change into the window holding the outpost's focus: local calls only before, 5 to 10 MSAA calls after (the model's estimate), and one message and reducer input.
- Removed on every focus Core judges: one `CoreFocus` to the listener and one to each concerned outpost, one to three, each a pipe write and a reader wake-up, and Core's copy built for each (`held_focus`, a clone of the line).
- Neither latency on the speech path nor the reducer's work per input rises: a repeat costs the reducer the duplicate check it already makes for repeats that reached it.

## Tests

### Added

- Core (`crates/verbatim-core/tests/reduce.rs`): the repeat hole's sequence with its observation stamps (A at 1,000 from outpost 1, B at 1,100 from outpost 2, A at 1,200 with `earlier` `Focused`), heard exactly as the existing test above; an MSAA repeat of Core's alive focus with no `earlier` is silent and keeps its newer states; a description change of a node that is not Core's focus is silent. The one record: a unit test per `ForegroundFact` naming the accessor it moves, and the existing `notepads_foreground_report_after_the_desktops_last_focuses_is_spoken`, `a_focus_observed_before_the_latest_from_another_outpost_is_stale` and `a_topmost_focus_observed_before_the_newest_is_stale` pass unchanged.
- The outpost (`followed.rs` unit tests): each exception keeps the followed focus; a repeat by its own evidence leaves `states_read`; `beside` is a change target until the next report. Mockapp (`msaa_events.rs`): the followed check box kept when Core ignores its window's foreground report, and its state change then spoken, through `SrState`, "checked"; the dialog text carried after another application's focus.
- The listener (`read_schedule.rs` replay tests): an event at the abandoned limit waits and is read by a returning thread; an abandoned read's late return gives no outcome and no last read; no grace-end alarm without a foreground event (e46d4183); an event waiting at the limit is dropped with its reason at the deadline and is not a pending focus for another application (0b5826cd); an idle waiter runs with later events still queued (073d63b0); a grace callback before the grace is over sends nothing and sets the timer for the rest (69c06239); every pushed marker gets exactly one outcome; a menu opening is current only while no other application's focus or other window's foreground came since the last read.
- Mockapp (`focus_reports.rs`): a repeated UIA focus read in full, and an MSAA repeat read in full, each with its calls; a menu opening not current is judged by its own window, with no comparison.

### Removed, each with its cheaper replacement

- `held.rs`'s two tests, and `worker.rs`'s `foreground_repeats` tests: by `followed.rs`'s tests and `a_foreground_change_into_the_focused_window_is_ignored`.
- `focus_read.rs`'s `HeldElements` tests: by the replay test of the abandoned late return.
- `main.rs`'s `core_s_focus_is_told_to_the_judged_outpost_and_those_it_moved_between`, and `reduce.rs`'s `the_focus_core_holds_carries_its_windows_handles`: the behaviour is removed; the window handles' use is covered by `read.rs`'s `entered_before` tests on the followed line.
- `a_focus_waiting_for_core_s_judgement_is_no_repeat`, `a_focus_is_no_repeat_once_core_holds_another`, `a_uia_focus_is_no_repeat_after_another_application_s_msaa_focus`, `a_repeated_msaa_focus_is_reported_after_another_application_s_uia_focus`: by `the_same_focus_after_another_applications_focus_is_announced`, `a_duplicate_focus_does_not_make_a_later_focus_stale`, the new Core repeat tests, and the mockapp full-read tests.
- `a_repeated_focus_out_of_time_is_reported`: a repeat is no longer special out of time; covered by `a_focus_out_of_time_is_dropped_only_by_its_windows_class`.
- `msaa_events.rs`'s `a_change_of_a_focus_core_no_longer_holds_is_not_read`: by the Core test of a change off the focus.
- The mockapp harness's `core_focus_elsewhere`, `listener_core_focus_elsewhere`, `listener_core_refuses_last_read` and `tell_core_focus`; the notice's parts of `verbatim-outpost/tests/idle_round.rs` and `spares.rs`, and `tests/common`'s `held_focus`.

Rewritten, not removed: `a_repeated_focus_on_the_still_focused_element_is_dropped_before_any_read` and `a_repeated_msaa_focus_is_a_duplicate_only_of_a_full_object` become the full-read tests; `uia_repeated_focus_costs_exactly` pins the full read; `a_menu_opening_on_another_element_is_judged_by_its_own_window` makes its second read not current instead of telling the listener Core's focus moved; `a_change_of_a_focus_core_has_not_judged_yet_is_reported` keeps its sequence under the followed focus.

### Scenarios

The full suite once, then `text_box_editing`, `second_application_and_verbatim_menu`, `settings_system_page`, `settings_combo_box_expanded`, `explorer_folder_window`, `menu_and_settings_dialog`, `windows_terminal_two_windows`.

## The known traps, against the code

- `states_read` (review 3): kept as the outpost's own; a repeat by its own evidence does not update it, so the expansion's child count survives a repeat read before the state change.
- A report the outpost can refuse by Core's definition (review 3): not followed; `refuses_focus` is applied by the outpost itself. The cost of every foreground change read in full is stated above.
- Core's refusal of a foreground report within one application (review 1, finding 2): answered by the foreground exception, with `beside` added for the window's own changes when Core's focus is elsewhere.
- The menu opening's lost subject (review 1, finding 3): compared with the thread's own current last read, currentness kept by the scheduler.
- The cost of MSAA repeats, and the menu opening's pending focus (review 1, nits): the cost is stated; `Readers::focus_pending` stays in the listener, a query of the scheduler's state.
- Across processes in one top-level window (review 2): messages added, never words, as above.
- Found in this design: the two refusals outside the four, answered by the outpost's own checks; dialog text, answered by the carried text; the stale refusal with Core's focus still here, put to Dickson.

## Open questions for Dickson

- The second foreground test and the ignored-window drop, refusals outside the draft's four: should the outpost mirror them, `classify`'s test on its own window facts and the ignore list it inherits? Recommendation: yes, both, since each is Core's own definition applied to facts the outpost already has; with no mirror, a refused report becomes the followed focus and Core's focus's changes go unread.
- The stale refusal while Core's focus is still this outpost's, in a window two processes share: accept that the outpost follows the stale report, or keep the previous followed focus as a second change target? Recommendation: accept it as a recorded limitation, the choice matching NVDA: in NVDA's single queue the stale focus is handled first and the foreground change then moves the focus to the window, so NVDA speaks neither the old control's changes nor the stale one's; keeping the previous focus as a target would read the old control's focus-loss change on nearly every Tab.
- Dialog text after another application's focus: carry the text gathered when the line entered the dialog on every later report in it, or gather it again on every foreground report? Recommendation: carry it, with no calls; NVDA reads it again on entering, so the carried text is older only when the dialog's text changed while focus stayed inside it.
- The window of a foreground report that did not replace the followed focus, kept as a second change target until the next report: accept it? Recommendation: yes; without it a title, value or description change of a window Core took as its focus is lost, which the draft overlooked.
- When the listener's last read stops being current for a menu opening: a foreground event of another top-level window, as drafted, or any foreground event? Recommendation: another top-level window, as NVDA's focus object leaves the read's window only then.
- The menu end's grace as a pure transition beside the scheduler, so 69c06239's ordering is a replay test: accept it into this build? Recommendation: yes; it is small and the brief's tests need it.
