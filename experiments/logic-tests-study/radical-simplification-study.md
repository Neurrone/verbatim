# Radical simplification of the pure owners: a study

Studied on 2026-10-31, read only, against origin/main b5d2588d (the worktree `verbatim-wt-split`), beside the determinism design in `docs/design/logic-foundations.md` ("Determinism", including "Simpler owners") and the purity audit (`purity-audit.md`). Paths are relative to `crates/`. Line counts are non-test lines and approximate; every size of work assumes one Opus builder.

The question from Dickson: beyond the incremental changes already planned, are there refactors that would make the reducers much simpler, by rewriting a mechanism or by splitting a reducer?

## The answer in brief

Yes for three mechanisms, and no for splitting the large reducers. The reducers are already divided into modules by concern. What makes them hard to reason about is mostly not their size. Three mechanisms are each written twice, or decided partly outside their owner:

- the caret key's watch, whose timing is decided by the worker on a copy;
- the worker charge (generation, deadline, moved-on grace, publication at the bound, abandonment), written once in the outpost's reducer and again in the listener's scheduler;
- Core's outstanding queries, tracked twice, once in the request table under one numbering and once in eight separate reducer slots under another.

Ranked by benefit for cost:

- The caret watch decided wholly by the outpost's reducer. Medium size, with the most past defects.
- One pure worker charge shared by the outpost's reducer and the listener's scheduler. Medium to large.
- Core's outstanding queries in one table, owned by Core's reducer. Large.
- The supervisor's three kinds of child record made one, done as M6.2's pure owner. Large; mostly a re-planning of work already on the roadmap.
- The hook's hold decision moved into the keyboard decision machine. Small to medium.
- Three small tidies inside the outpost's reducer: one copy of the marks, the gap timer derived from the record, and the focus group's remaining tries kept in the intake. Small.

## What was looked at, owner by owner

### The outpost's reducer

It has seven concerns, each mostly in its own module:

- the intake: arrival order, keyed replacement, markers, barriers, batches, NVDA's limiter, focus runs and menus (`intake.rs`, about 1,440 lines);
- the hand-over's hold (`hold.rs`, about 190), already a three-state enum;
- the gap record (`gap_record.rs`, about 310);
- the reports Core may hold (`held.rs`, about 270);
- the text facts (`text.rs`, about 460);
- the verdicts (`verdicts.rs`, about 190);
- the follow state with the caret watch (`follow.rs`, about 920).

The program lifecycle in `lib.rs` (about 1,640 lines) ties them together.

The state they share is small. The intake reads the marks, and the lifecycle reads the intake's waiting value changes and caret events. Commits write the gap record, the follow state, the held reports, the text facts and the marks. The idle answer reads the hold, the watch, the gap record and the program in flight.

A split into separate sub-reducers would therefore be mostly a move of code, since the modules already are that split. The gains are inside three mechanisms: the watch, the charge, and the intake's side queues (the candidates below).

### Core's reducer

Its 27 fields fall into about twelve concerns: focus judging, focus change and foreground, property changes, caret and editing, held keys, focus and navigator text, review and navigation, say-all, terminal speech, the terminal model, release at an outpost's end, and settings.

Three fields are written by five to seven concerns each:

- `focus`;
- `navigator`, whose `text` alone has five writers;
- `caret`, with its history.

Every text concern is gated on `focus`. A split into sub-reducers would make these three the crossing facts of every part, and each part would still need the other parts' latest values in the same transition. So a split would add plumbing and remove nothing; it is not recommended.

The terminal speech and terminal model are already modules with their own state. What does repeat is the request lifecycle (candidate three).

### The listener's scheduler

The scheduler (`verbatim-listener-logic/src/schedule.rs`, about 1,130 lines) is a per-application queue with NVDA's per-element limiter, plus a worker charge with these parts:

- a live generation per application;
- the running read, with its start and `published` flag;
- the deadline;
- the moved-on grace, armed by a foreground count or the front elsewhere;
- publication at the bound;
- abandonment, and the abandoned limit with its wait.

The shell gathers the moved-on answer and the read at the bound before each tick (`TickRead`), a two-phase hand-off. `GraceState` is small and explicit.

### The supervisor's policy and generations

The pure part (`verbatim-core/src/supervisor/`, about 990 lines) is sound, and `Generation` is a real typestate machine.

Most decisions are in the shell (`verbatim-supervisor/src/owner.rs`, 2,298 lines, about 1,430 of them decisions). It keeps three record types:

- `Record`, for an application's outposts;
- `ListenerRecord`;
- `SpareChild`.

Because of that, the same facts and steps are repeated:

- Liveness is held three ways, and `wedge_decision` is called from four places.
- The end, forget-writer and retire sequence is written out at six to ten sites.
- There are two backoffs.
- "Launching" is expressed three ways.
- A spare's identity is held in three places, and its fault in two.
- About 15 `shutting_down` guards are spread over the methods.

### Core's request table

`verbatim-app/src/requests.rs`, about 500 lines, implements its lifecycle once; only its delivery is per kind.

Its entries are numbered by `RequestId`. The reducer's own requests (fetch, activation, text) carry a separate `QueryId`, and the reducer tracks each in a slot of its own. So every reducer query has two records under two numberings.

### The keyboard decision machine

`verbatim-input/src/state.rs` is about 690 lines. Its lone-modifier lifecycle is already an explicit enum.

The hook's shell (`verbatim-input-windows/src/hook.rs`, 246 to 462) overrides the machine's decision for holding keys. It also keeps `held_downs`, a second "swallow the release" set beside the machine's `swallowed_downs`, and duplicates the modifier and lock-key tables.

## The candidates, ranked

### First: the caret watch decided wholly by the outpost's reducer

**Today.** The open watch lives in the reducer's follow state. Each program applies its own commits to its copy of that state with the same pure function (`follow::apply_local`), and the reducer applies them again.

Decisions about the watch are made by the worker, on that copy and with the worker's clock:

- `Worker::wake` (`verbatim-outpost/src/outpost/worker.rs` 1069) decides whether the bound has passed. A non-polling watch's bound is therefore a whole program dispatched to the worker only to compare two times.
- `read_poll` (1092) decides whether a poll read is due yet. A wake taken early dispatches a program that only commits the wake again.

`WatchChange` has eight variants: `Open`, `PutBack`, `Take`, `End`, `Read`, `Answered`, `Ended` and `Wake`. The reducer special-cases the watch in four places:

- the commit's `running` and `deferred` changes (`lib.rs` 812 to 819 and 838 to 843);
- abandonment (1052 to 1067);
- a program that ends unanswered (1098 to 1112);
- the wake item (1344 to 1374).

The watch, its wake and the idle answer have had six fix commits since 26 October: 463f2ef9, 6b0bf0d4, 17767cbf in part, 60dc079c, b5d2588d (two defects) and d7ec6b04 on Core's side. That is past the three-round rule in `CLAUDE.md`.

**Proposed, in two steps.**

- **Timing and bound in the reducer.** The reducer holds the watch as an explicit state with `Open`, `Polling { next, until }` and `Checking` states. It decides the bound and each poll read's due time from its `Now`, and ends a watch at its bound itself, with `WatchEnded` sent, and no program.
  - Programs report only what a check found: answered with a reply, still watching with its calls, or cut.
  - Removed:
    - `WatchChange::Wake`;
    - the early-wake and bound branches of `Worker::wake` and `read_poll`;
    - the dispatch of a program for a bound;
    - the worker's clock reads for the watch.
  - Changed: `Read`, `Answered` and `Ended` become one `Checked` result.
  - This is also a determinism gain the design does not list: the worker's `context.now()` decides the watch's state today, and a replay cannot reproduce that.
- **No watch taken out mid-program.** The reducer lends the open watch to a program at dispatch, as it already sets `deferred` and `running` at dispatch for a gap read and a poll read. The program gives it back in its end.
  - This removes:
    - `Take` and `PutBack`;
    - `Applied::taken`;
    - the commit-time `running` and `deferred` changes;
    - the second application of watch changes to the worker's copy.
  - The open design question is which programs get the watch. Today `check_watch_now` decides by node after resolving the event through the store, which the reducer cannot see. Lending it to every program whose entry may check it (caret, text, selection, console and value-change events, and terminal reads, while a watch is open) means a program abandoned before its check would answer the key's query "abandoned" where today the watch stays open.
  - Whether that changes anything heard must be settled from Core's held-key handling before this step is built. If it does, keep `Take` as the only mid-program change and still take the first step.

**Size.** The first step: about 150 lines removed from the worker and the reducer, and an explicit watch state of about 120 lines added; one to two days. The second step: about 150 more lines removed; one day after the design question is answered.

**Risk.** The defects above, especially a lost or stale wake (463f2ef9, b5d2588d) and an abandoned check touching a newer watch (463f2ef9).

**Tests.**
- The guards are the replay tests in `verbatim-outpost-logic/src/tests.rs` on the watch, poll and idle answer, and the mockapp `caret_watch.rs` suite.
- For M4.6 the watch becomes one small state machine with a reference model of four states, where today a model would have to follow a watch moving between two copies.
- What is heard does not change with the first step.

**Order.** Before "Simpler owners", which touches the same `Current` fields, and before M4.6. No determinism work is wasted.

### Second: one worker charge for the outpost and the listener

**Today.** The outpost's `Current`, `generation`, `abandoned`, `publish` and `judge_abandonment` (`lib.rs`, about 300 lines) and the listener's `Running`, `live`, `abandoned`, `limit_reached`, `Claim`, `tick`, `abandon` and `publish` (`schedule.rs`, about 350 lines) are the same mechanism. Each has a generation in charge, one read or program at a time with a deadline, the moved-on rule after `MOVED_ON_GRACE`, publication of a walk at its bound, and abandonment with a replacement.

The rule is ported once (`verbatim-reader-rules`) but its state machine is written twice, with different judging points:

- the outpost judges moved-on once at the grace's end and again at each foreground fact;
- the listener judges at every tick after the grace while a foreground change has armed it.

Its local reads reach it two ways: the outpost's `Locals` calls today, which the determinism build turns into asks, and the listener's two-phase `TickRead`.

**Proposed.** One pure `Charge` sub-reducer in `verbatim-reader-rules`, which both readers already depend on. Each reader embeds its own instance, so each fact keeps one owner.

- State: the generation, and the running item with its deadline, its window, its start, and its bound as `Unarmed`, `Armed(at)` or `Published`.
- Inputs: started, armed, returned, tick, the moved-on answer, and the published answer.
- Effects: ask moved-on, ask publish, and abandon with a reason.
- Composition: the reader passes its own item and deadline in. The charge returns whether the item's commits are still accepted and when it next wants a tick. Nothing else crosses.

What it removes:
- one of the two implementations, about 250 lines net;
- `claimed` and `walk` as separate fields, which "Simpler owners" would turn into an enum, now done once;
- the listener's two-phase tick gather, since the listener asks as the outpost will.

What remains per reader: the outpost's focus-group continuation, and the listener's abandoned limit and per-application queue, which stay in their owners.

**Risk.** This is the area of the publication at the bound, four rounds (c47d57ae, 6dbcca25, 2bd69a85, 8a087fe7), and the moved-on rule's window-0 defect (e46d4183). Unifying the judging points changes when the moved-on rule is judged in one reader. That must be checked against NVDA's watchdog in the pinned source before building, and may change what is heard in rare slow-application sequences.

**Tests.**
- The guards are both crates' replay tests.
- For M4.6 there is one generated test and one reference model of the charge, in place of two models with different rules, and the two readers' models shrink to their queues.

**Size.** Three to four days, including the NVDA check and an adversarial review.

**Order.** After the determinism build's first build. Its asks (`AskMovedOn`, `Publish` with `then_abandon`) become the charge's effects, so that design is reused, not wasted. It replaces the "walk as one state" and "abandoned count" items of "Simpler owners". It should precede M4.6.

### Third: Core's outstanding queries in one table owned by Core's reducer

**Today.** Each reducer query is recorded twice:

- in the request table, under a `RequestId`, for its ending: the answer, "gone" when its outpost ends, or its deadline;
- in one of eight reducer slots, under a `QueryId`, for what to do with the answer: `latest_navigation`, `pending_text`, `pending_caret`, `superseded_caret`, `held_keys.reading`, `say_all.pending`, the terminal's `request`, and the terminal model's asks.

`text_completed` (`verbatim-core/src/reduce.rs` 573 to 628) tries the slots in a fixed order to route each answer, and each module has its own `is_pending`. `held_nodes` and `held_anchors` (`state.rs` 927 and 975) gather from all the slots. Two request helpers are byte-identical (`review_text::request` and `editing::navigator_request`), and two command paths repeat the same inline request.

**Proposed.** One `outstanding: BTreeMap<QueryId, Awaiting>` in `SrState`.
- `Awaiting` is an enum naming what the answer continues, with its own supersession rule as a field.
- The reducer takes an outpost's end and a deadline tick as inputs, so the request table's endings become reducer transitions.
- The table's non-reducer askers, the GUI's and the control plane's, stay in a small shell table, as the determinism design already makes the table generic over its asker.

What it removes:
- the second numbering and record;
- the eight-way routing;
- the per-module `is_pending` functions;
- the duplicated request helpers;
- probably about 300 to 400 lines of Core and `verbatim-app`.

**Risk.** Supersession differs by kind: navigation drops a superseded answer, the caret key keeps `superseded_caret` and `later_key_at_ms`, and text overwrites. That rule must move intact onto each `Awaiting` kind. Held-key ordering defects (d7ec6b04, e180f14a) and the say-all and terminal reply rules are the regressions to guard.

**Tests.**
- Core's replay tests under `verbatim-core/tests/` are the guards, as are the request table's tests of each ending.
- For M4.6 it gives one invariant for all of Core: every query has one entry and exactly one answer path.

**Size.** Four to six days, in two steps: first the table moved into the reducer with one numbering, then the slots replaced.

**Order.** After the determinism build's second build, whose move of the table into `verbatim-core` is its first step, so nothing is wasted. The "Simpler owners" changes to the request table, `Ends` and the removal of `timed`, are cheap and are subsumed. It may follow M4.6's first generated tests of the outpost, but should precede M4.6's model of Core's reducer.

### Fourth: one child record in a pure supervisor, as M6.2

**Proposed.** M6.2 already makes the supervisor's owner thread a pure state machine. Build it around one `Child { kind, run, liveness, backoff }` for outposts, the listener and spares. That removes:

- two of the three records;
- three of the four `wedge_decision` call sites;
- the duplicated launching states;
- the second record of a spare's identity and fault;
- the repeated end, forget-writer and retire sequences.

A `stopping` state replaces the scattered `shutting_down` guards.

**Risk.** Spare hand-over and idle-round defects (df4c15d9), and the listener's lack of backoff.

**Size.** Large, about a week, but it is M6.2's own work.

**Order.** The determinism build's first build adds a `Liveness` type for the listener and the spares, and `due_starts`. Under this plan `Liveness` would be rewritten by M6.2. Keep the first build to the defect fix (`Live`'s fields private behind its transitions, the `BTreeMap`s and the clock as microseconds) and move `Liveness` into M6.2. That saves a small amount of soon-wasted work. Nothing heard changes.

### Fifth: the hold decision inside the keyboard decision machine

**Proposed.** `DecisionMachine::on_key` returns one verdict per key: pass, swallow, hold, or pass after the outstanding keys. It owns the one swallow set. That removes:

- the hook's `held_downs`;
- the override in `hold()`;
- the duplicated modifier and lock-key tables.

The hook keeps only its calls and its sequence tag.

**Risk.** The held-key route (ba7064e5, e180f14a, d7ec6b04) and the hook's timeout.

**Tests.** The machine's unit tests and `notepad_typed_words`. It gives M4.6 one machine to model in place of a machine plus an override.

**Size.** Two days.

**Order.** With the determinism build's second build, which already moves the machine into the covered crates and makes Num Lock an input of each key. Nothing heard changes.

### Sixth: three small tidies of the outpost's reducer

- **One copy of the marks.** `Intake::marks` copies `Facts::marks` within the same owner (`sync_marks`, `note_focus` and `absorb_console_updates`). The intake can read the facts' marks as an argument. That removes the copy, its two setters and `Applied::adopted`, about 50 lines.
- **The gap timer derived from the record.** `Reducer::gap_due` repeats the open record's `due_us`, and `TimerAction` threads the record's changes out to it. `next_timer` can read the record's due time, with a flag for a bound entry already queued. That removes `TimerAction`, `Pushed::timer` and `gap_due`, about 70 lines.
  - The test seam (`GapTimerGiven`, `GapBound`, `Effect::GapTimer`, `gap_timer_given`) goes once the mockapp gap tests become replay tests, a follow-up the design already names.
- **The focus group's remaining tries in the intake.** `Reducer::after` is a second queue ahead of the intake's batch. `finished` juggles a gap read in front of a group's tries (`lib.rs` 1122 to 1142). Keeping the remaining tries at the front of the intake's batch, with a gap read inserted there, removes `after` and that juggling, about 40 lines.

Risk: low, guarded by the replay tests. What is heard does not change. One day together. They belong in the "Simpler owners" build.

## Considered and not recommended

- **Splitting Core's reducer, or the outpost's, into separate sub-reducers.** As above, the crossing facts would be most of the state.
- **Programs that return only observations, with every decision in the reducer.** For the decisions made between two calls, this needs a hop to the reducer and back per decision. That adds wake-ups on the event path, which the design principles forbid without that purpose. It is worth doing only where the decision follows the program's last call, which is what the first candidate does for the watch.
- **A table-driven intake.** Its rules are NVDA's limiter, ported rule by rule, and the code is about as long as the rules.

## Found while studying, for follow-up

- Shell items wait under two deadlines in Core, `Wanted`'s and then a fresh `SHELL_ITEMS_DEADLINE` in the table (`verbatim-app/src/main.rs` 1898 to 1917), so the wait can reach about twice the constant.
- A held caret key's read has two bounds, the table's `HELD_KEY_BOUND` and `held_wait`.
- Core's `caret_history` makes the same caret-before-the-key judgement the outpost makes on its own timed carets (`verbatim-core/src/state.rs` 285, `editing.rs` 491). That is one judgement in two processes. It belongs to the caret rework.
- The supervisor checks an application's exit twice, by the outpost's `TargetExited` report and by polling the process.
