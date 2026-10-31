# Second review of the determinism design

The design reviewed is the section "Determinism (a build of its own)" of `docs/design/logic-foundations.md` in the working copy of 2026-10-31, including the third build as widened during the review ("Simpler owners"). It was read against origin/main b5d2588d, checked out in `verbatim-wt-split`. Code paths are given from `crates/`, with line numbers at b5d2588d. NVDA lines are from the pinned submodule (732d1ce2).

Important findings: 6.

## Does the revision reach its goal

Mostly. Every Important finding of the first review has a mechanism that works: the `Locals` answers leave the reducer, every production mint has a source, the scan rejects group-level silencing and pins the manifests' lint tables, third-party crates are held to an allow-list, the clock methods and whole modules are closed, and the lost read has a budget and a fallback that names its five maps. Most of the audit is answered too. The request table, the gesture map, `due_starts`, the shell's liveness records and the floating-point move each remove the impurity claimed. The reducers would be pure after the first build, except for two gaps:

- The new ask for a publication opens a window that the rest of the same transition misreads (finding 1).
- The hung-window test moved to the worker changes what is heard and is under-specified (findings 2 and 3).

The supervisor's single writer leaves the main wedge judgment in the shell (finding 4). The encode split cannot be built as written (finding 5). One of the third build's additions puts a walk on every command of Core's reducer thread and misstates its cost (finding 6).

The NVDA claims check out line by line. `nvda/source/UIAHandler/__init__.py` 768, 882, 981, 1085 and 1172 are each `if utils._shouldSkipEventForHungWindow(sender):`, at the top of the automation, focus, property, notification and active-text-position handlers. `utils.py` 438 to 452 defines the test: it reads the sender's cached window and returns false when there is none (449 and 450).

The ask rule matches an existing mechanism. The shell already answers `Effect::Release` with `Input::Released` inside its effect loop, putting the answer's effects ahead of the rest (`verbatim-outpost/src/outpost/shell.rs` 680 to 696). So "the next input, before another message" can be built, and no deadlock was found: the shell takes the charge's lock for one copy, as today.

## Important findings

### 1. A publication asked at the walk's bound is lost when the same tick abandons the program

- Where: design, "A walk published at its bound" and "The rule for asks, and the sequences". Code: `verbatim-outpost-logic/src/lib.rs` 652 to 668 (`tick`), 972 to 994 (`publish`), 1000 to 1074 (`judge_abandonment`, the walk test at 1028 and the group at 1069 to 1073); `verbatim-outpost/src/outpost/shell.rs` 554 to 562 (the charge looked up by `reducer.generation()`).
- Defect: between the `Publish` ask and its answer, the walk's bound is cleared and `claimed` is not yet set. `judge_abandonment` reads that state as "no walk armed", so a tick that passes both the bound and the deadline abandons the program before the answer comes. The design says the ask adds no state, but this unnamed state is there.
- Counterexample: program R of generation G has a walk armed with its bound at 9.998 s, and its deadline is at 10 s. The tick wakes late, at 10.001 s.
  - `tick` sees the bound passed, emits `Publish { R, then_abandon: none }` and clears `walk`.
  - In the same transition, `judge_abandonment` finds the deadline passed. `current.walk` is `None`, so it emits no `then_abandon`. It takes `current`, makes G + 1 the generation in charge, emits `Abandon` and answers R's query `Abandoned`.
  - Since `claimed` is false, it also queues the group's older entries for the replacement.
  - The shell then carries out `Publish { R }`. If it looks up the charge as `with_locals` does today, by `reducer.generation()`, it gets G + 1's charge, and `Charge::publish(R)` answers nothing. If it uses G's charge, the answer `Published { R, change }` is dropped, because R is no longer current.
  - Today, the same tick commits the published focus, sets `claimed`, and then abandons without trying the group's older entries. So the focus that was read is not spoken, and an older focus of the group is tried in its place.
- Fix: judge the abandonment's reason first in `tick`. If there is a reason and a walk is armed, emit one `Publish` carrying `then_abandon`. Otherwise publish at the bound. State that the shell carries out `Publish` against the charge of `request.generation`, not the generation in charge. Add this tick to the sequences and to the reducer's tests.

### 2. The hung-window test at handling no longer covers the listener's focus reads, so what is heard changes

- Where: design, "Whether a window is hung", the choice (a): "the listener's focus reads are no longer judged twice", "Every later entry of that application is dropped by the worker, without a call into it", "What is heard does not change", and "Wake-ups: none added or removed". Code: `verbatim-outpost-logic/src/intake.rs` 1236 to 1245 (`admit` tests every UIA item with a window, `UiaFocusRead` included, at 1180 to 1184); `verbatim-listener/src/focus_read.rs` 1332 (the listener's test, made once, before it reads); `verbatim-outpost/src/outpost/worker.rs` 3648 to 3692 (`uia_focus`: `own_window_verdict`, then `uia_chain_from_read`, which goes on walking up live within the read's bound, `read.rs` 848).
- Defect: today a focus read that the listener made before the application hung is dropped by `admit` if the window is hung when the batch is planned. Under the design, the worker handles it with no hung test. It probes the window's backend, which is a call into the application unless the verdict is cached, and continues the ancestor walk live. So it meets the hang, reaches its walk's bound or its deadline, and is published or abandoned.
- Counterexample: the user tabs to a control, and the application stops responding 5 s later, while the listener's read of that focus waits behind a slow program.
  - Today, `admit` drops the read and nothing is said.
  - Under the design, the read is dispatched, its walk is armed, and at its bound the focus is published and spoken, from the listener's snapshot and whatever ancestors were read.
- Further effects:
  - The read also costs a worker abandoned at the deadline if the probe blocks.
  - Each UIA entry from a hung window that is dropped now costs a dispatch and a `Finished`, which wakes both the worker and the shell, where `admit` dropped it at no cost. That is no cost on the ordinary event path, but the claim of no added wake-ups is wrong.
- Fix: test focus reads at handling too, before `own_window_verdict` and with `read.hwnd`, as `admit` tests them today. That is one more `IsHungAppWindow` call per focus read, at 0.41 µs. The alternative is to state the change in what is heard as a question for Dickson. Correct the claim about wake-ups either way.

### 3. "As it already does for MSAA entries" names a test that would drop every windowless UIA event

- Where: design, choice (a): "The worker tests the window as it handles each UIA entry, before any call into the application, as it already does for MSAA entries." Code: `verbatim-outpost/src/outpost/worker.rs` 2278 (`readable`, the MSAA test), `verbatim-win32/src/window.rs` 278 to 294 (`event_window_unreadable`), `worker.rs` 2784 to 2805 (`uia_event`'s choice of window).
- Defect: the MSAA test is `event_window_unreadable`, which also drops "no window" (window 0, or one that is no longer a window) and windows a ghost stands in for. NVDA's UIA test does none of that. It passes a sender with no cached window (`utils.py` 449 and 450), and `admit` today skips window 0 (`intake.rs` 1240). The design does not name the function, the window it tests, or where it goes in `uia_event`.
- Counterexample:
  - A builder reuses `readable(event.hwnd)` for UIA entries. Every UIA event from an element with no native window of its own is then dropped as "no window": most property, selection, text and notification events in XAML, Chromium and WinUI applications, since `event.hwnd` is 0 there.
  - Placed after line 2793 instead, the test would use a window found by `nearest_window_handle(element.live()...)`, which is a live call into the application, made before the test.
- Fix: name `verbatim_win32::window::window_is_hung` on the event's own cached window (`event.hwnd`, or `read.hwnd` for a focus read), with 0 passing untested. Place it first in `uia_event` and `uia_focus`, before any window lookup or probe.

### 4. The supervisor's heartbeat still judges `Live` in the shell

- Where: design, "Core's supervisor", "Liveness written by the shell": `hang_checked` "applies `wedge_decision` itself, so the shell no longer copies `Live` out to judge it". Code: `verbatim-supervisor/src/owner.rs` 1951 to 1981 (the heartbeat) and 2100 to 2124 (`on_hang_checked`).
- Defect: the heartbeat reads `live.abandoned`, `live.hang_check` and `live.last_pong_at` through `live()`. It decides there whether to start a hang check, and judges the wedge with `wedge_decision`. That is the main judgment, made on every heartbeat. The design's three transitions do not cover it. Once `Live`'s fields are private, the builder must either add getters, so the shell goes on judging the generation's state and the wedge rule is applied in two places, or invent a heartbeat transition the design does not have.
- Counterexample: after the first build as designed, `hang_checked` judges the wedge in the generation, and the heartbeat judges the same rule in the shell from getters. A change to the rule made in one place, such as counting abandoned workers only while no check runs, leaves the other place deciding differently on the same state. The ownership rule this section exists to enforce is broken by the same section.
- Fix: add `Generation::heartbeat(now_us) -> Heartbeat { Ping, CheckHang, Wedged(reason) }`. It applies `wedge_decision` and the hang-check rule, and sets `hang_check` when it answers `CheckHang`. If the spawn fails, the shell reports it with `hang_check_ended()`. Otherwise, state that getters are kept and why the copy is acceptable.

### 5. The encode split moves its calls to a writer thread that does not exist

- Where: design, "The outbound backlog and the writer queue", choice (b): "the writer thread stamps it, signals and recycles". Code: `verbatim-process/src/transport.rs` 1093 to 1102 (the `Backlog::encode` contract), 1370 to 1414 (`write` and `write_all`, which call `encode` under the sender's lock on whichever thread writes, the sending thread among them); `verbatim-outpost/src/outpost/outbound.rs` 223 to 245; `verbatim-supervisor/src/writer.rs` 418.
- Defect: `encode` is called by the generic transport in `verbatim-process`, not by a writer thread of the outpost or the supervisor. The transport knows nothing of `Flushed`, latency stamps or terminal buffers. Its contract requires the stamp to be taken in `encode` "so a time stamped then is the write's", and a marker to be reached in encode order.
  - So the split needs a change to `verbatim-process`'s `Backlog` trait, used by both backlogs, which the design does not state.
  - The calls could then only move to another method of the same impl, on the same thread, so the "pure type" gains nothing that a test can use.
- Counterexample: a builder following the design finds no writer thread to move `done.send(())` to. Moving it after the write, through a new trait hook, changes when `Outbound::flush` returns (`outbound.rs` 325 to 332): after the write completes rather than as its frame is reached. That is a change in the shutdown's ordering, which the design does not state. The builder must stop or improvise.
- Fix: take (a) and leave both `encode`s as they are, recorded as timing and recycling only, which no decision reads. Otherwise, state the trait change, its new hook, and when the flush's waiter is answered.

### 6. Removing `timed` adds a walk of the request table to every command on Core's reducer thread

- Where: design, third build, "No count of timed entries": "Outstanding requests number a few at a time, so `expire` walks the map instead, as `next_deadline` already does ... The cost is one walk of the outstanding entries per call of `expire`." Code: `verbatim-app/src/requests.rs` 228 to 232 and 252 to 255 (both return at once while `timed` is 0); `verbatim-app/src/main.rs` 2111 to 2120 (the loop calls `next_deadline` before every command, with the comment "a command costs no scan of the requests").
- Defect: `next_deadline` does not already walk the map. It returns at once while no request has a deadline, as `expire` does. The cost of removing `timed` is not one walk per `expire`, since `expire` runs only when a due time passes. It is one walk of the outstanding entries in `next_deadline` per command of the reducer thread, which includes every event, key and speech completion.
- Counterexample: while the user types in an editor, each caret key leaves a passive watch request outstanding, with no deadline, until its evidence or its bound. Every event of every keystroke then walks the table in `next_deadline`, where today `timed == 0` returns at once. That is a cost on the event path that `docs/performance.md` does not state, and that the design calls a cost of `expire`.
- Fix: keep `timed`, with the new `Ends` type making its updates one match each. Otherwise, state the walk per command, measure it on `measure_keys_notepad`, and add it to the ledger.

## The third build's other additions

- **The abandoned count kept by the shell: sound.**
  - No decision of the reducer reads `abandoned` (`lib.rs` 468, 615 and 1045 only build effects).
  - The shell already stores the number in `Context::abandoned` (`shell.rs` 730 and 735), which is read for the pong (`mod.rs` 1523).
  - A worker posts `Returned` only when its charge is dead, after the shell has carried out `Abandon` and dropped its inbox (`worker.rs` 285 to 287, `shell.rs` 727 to 729), so counting up in `Abandon` and down on `Posted::Returned` cannot go below zero.
  - The `pump` and `answer_idle` that `Input::Returned` runs today change nothing, since no other state changed.
- **Num Lock as a parameter of `on_key`: sound.**
  - `on_key` has one production caller, and `set_num_lock` is called immediately before it on every path that reaches it (`verbatim-input-windows/src/hook.rs` 366 to 368). The passed-on return at 357 to 361 calls neither.
  - So no key's decision changes. The 109 calls of `on_key` in `state.rs`'s tests change with it, which the design does not count (a nit).
- **The reasons for no change: they hold.**
  - Core's `SrState` has 27 fields (`verbatim-core/src/state.rs`), and the three related-looking pairs are set by different inputs.
  - The reason for keeping `to_drop` holds: the drop is decided by the reducer when no program is in flight, and attaching it to the next program would wait for an event.

## The simplification goal and the build split

- The first build takes inputs and effects from 15 to 17, and the third takes each to 16. That is one more of each, against a callback interface removed. The goal of smaller interfaces is met only if finding 1's fix adds no state. Its recommended form, judging the reason first and emitting one ask, adds none.
- The walk enum mirrors the worker's `SlotState` (`worker.rs` 123 to 134). It is a typed copy of a fact the charge also holds, and the walk and `claimed` fields are already such a copy. It is worth one sentence in the design.
- No build leaves main with a reducer less pure, or a test meaningless, until the next lands. The enforcement is the last commit of the first build. The request table's deadlines become microseconds in the same commit that moves the table into `verbatim-core`. `verbatim-input` gets its `clippy.toml` in its own commit.

## Nits

- `due_starts(backoffs, now_us)` is described as pure, but `Backoff::ended` clears `due` as it answers (`verbatim-core/src/supervisor/policy.rs` 130 to 135). Say whether `due_starts` takes the backoffs mutably, or whether the shell calls `ended` after it, which would be a second judgment of "ended".
- `RequestTable::dump_tree_request` (`requests.rs` 262 to 271) finds an entry by its asker's ticket. A table generic over an opaque asker needs a predicate argument for it. The design should say so.
- `GestureId` derives `Hash` but not `Ord` (`verbatim-model/src/gesture.rs` 18). A `BTreeSet<GestureId>` needs the derive in `verbatim-model`.
- `verbatim-input` declares `crossbeam-channel` and `tracing` but uses neither. Neither is on the third-party allow-list, so the tier check fails when the crate joins the covered crates unless they are removed. The design also says the covered list is derived from the graph, but `verbatim-input` is on no reducer's path, so it has to be listed by hand. Say so.
- `TraceId::mint`'s own test (`verbatim-model/src/lib.rs` 211 and 212) must call `mint`. It is not among the listed exception files, so it fails Clippy.
- "A reducer test checks that every ask in a generated run is answered once" cannot test the rule, since the test gives the answers itself. The shell test of the rule is the one that proves it.
- Each `MovedOn` and `Published` answer needs a `Now`, which is three clock reads (`shell.rs` 511 to 517). "Calls: as today" should count them. They are only for slow programs.
- `hang_checked` judges even while the supervisor is shutting down, where today the shell returns first (`owner.rs` 2111 to 2113). Say that the shell drops the reason then.
- The third-party verdicts for `serde_json` (`preserve_order`) and `vte` (`ansi`) depend on features that `workspace-hack` could unify in from any workspace crate, and the tier check holds crates, not features. Neither feature would make a covered result nondeterministic, so a sentence in the limitations is enough.

## Pre-existing

- `admit` drops a hung window's `Item::Uia` without returning its parked element (`intake.rs` 1236 to 1245, called from `plan` at 1303). `Intake::push` returns parked objects for replaced entries (486 and 514), and `close` returns them for the entries still waiting (704). An entry that the batch plan drops returns nothing, so its parked element, and an active-text-position event's range, stay in the store until the outpost ends. The design's choice (a) would release them on the worker, as `uia_event` takes the parked element first (`worker.rs` 2785 to 2788). That is worth stating as a benefit, and recording if (a) changes.
