# Purity audit of the reducers and pure owners

Audited on 2026-10-31, read only, against branch `bookkeeping` (cec81531) in the worktree `verbatim-wt-split`. The question for each owner: does its output, the change to its state and the effects it returns, depend on anything but its state and its input? Each impurity gives its file and line, what it reads, which outputs it reaches, a one-line sketch of how it could become an input, and whether the determinism design (main's `docs/design/logic-foundations.md`, "Determinism") covers it.

Paths are relative to `crates/`. Line numbers are those on the branch.

## Summary of verdicts

- Core's reducer (`verbatim-core`, `reduce`): pure in its own code. The impurities it reaches are in `verbatim-text` and one hash set of its own, all covered.
- The outpost's reducer (`verbatim-outpost-logic`): not pure. It has the three `Locals` calls, four trace id mints and its hash maps, all covered. The shell's `publication` also does writes inside a transition, which the design covers only in part.
- The listener's read scheduler and grace (`verbatim-listener-logic`): pure apart from its two hash maps and its logging, all covered. `GraceState` and `ForegroundWatch` are pure.
- `verbatim-reader-rules`: pure functions. The acceptance test takes live window queries through `WindowQueries`, but it is called only from the Windows workers (covered).
- The supervisor's policy and generations (`verbatim-core`, `supervisor`): not pure. Their `Instant` values and the clock closure are covered. Two things are missed: the shell writes the generation's liveness fields directly, outside any transition, and keeps the listener's and the spares' liveness itself.
- Core's request table (`verbatim-app`, `requests::RequestTable`): not pure, and none of its impurities are covered, since its crate is not in the design's scope.
- Other owners described as pure or as state machines:
  - The keyboard decision machine (`verbatim-input`, `DecisionMachine`): not pure, and none of its impurities are covered.
  - The idle barrier (`verbatim-app`, `IdleBarrier`): pure.
  - The GUI lifecycle (`verbatim-gui`, `lifecycle`): pure.
  - The outpost's terminal reading (`verbatim-outpost`, `terminal/reading.rs`): pure.
  - The outpost's outbound `Pending` and the supervisor writer's `Queue`: the merging decisions are pure, but `encode` is not. Not covered.
  - The supervisor shell's own decisions: one reaches a hash map's order. Not covered.

## Core's reducer

Verdict: pure in its own code. `reduce` (`verbatim-core/src/reduce.rs` 49) takes `&mut SrState` and `&Input`. The app changes `SrState` only through `reduce` (`verbatim-app/src/main.rs` 1593). The checks found:

- No clock, randomness, thread, environment or file read, and no production `TraceId::mint`. The mints in `dump.rs`, `recorder.rs`, `terminal/tests.rs` and `terminal_model/tests.rs` are all in test modules.
- No callback that reaches outside the state:
  - Core mints the terminal anchors through a closure over `state.next_terminal_anchor` (`terminal_model.rs` 1198 to 1248, `terminal_model/review.rs` 199).
  - The `focus_rules` closures read the state.
  - `EnteredKey` is implemented on data.
- `Arc` is used only for copy-on-write sharing of the reducer's own values (`state.rs` 47, 109 and 197, `terminal_model.rs` 185 with `make_mut`). No `Mutex`, atomic or other interior mutability.
- Floating point: `progress_percentage` (`reduce.rs` 2262) parses an `f64`, clamps it, multiplies it by 100 and rounds it (`reduce.rs` 2303). These are basic IEEE operations, the same on x64 and ARM64. No transcendental function.

Impurities it reaches:

- `terminal_model.rs` 1315, `held_anchors` builds a std `HashSet` (random seed), passed as `held` through `terminal_model/review.rs` 91, 177, 200 and 688.
  - Reads: nothing hidden, but the hash is seeded randomly.
  - Reaches: nothing, since it is only used for `contains` (`review.rs` 184).
  - Sketch: a `BTreeSet`, or pass the `HeldAnchors` it is copied from.
  - Covered.
- `verbatim-text`, which Core calls for terminal reading and text segmentation:
  - Std `HashMap` and `HashSet` with a random seed at `terminal_reading.rs` 1072 and `terminal_rows/align.rs` 121, and the `Spread` fixed-hasher maps and sets at `terminal_rows.rs` 39 and 41, used at 769, 1519, 1572, 1577, 1671, 1708 and 1819.
  - `commonest_shift` (1671) iterates its map, but the tie-break on first place is unique, so the result does not depend on order.
  - `Aside::rows` (769) is serialized into dumps in iteration order. That changes the dump's text, not a decision.
  - Sketch: `BTreeMap` and `BTreeSet`.
  - Covered.
- `verbatim-text/src/lib.rs` 434, the `OnceLock<Jieba>` segmenter, built once from embedded data. It reaches no decision. Covered.
- `verbatim-text/src/terminal_rows.rs` 266, `Text::through` compares with `Arc::ptr_eq`. So its result depends on whether two rows share one allocation, which a state restored from a dump does not keep. Its caller `lines` (2405 to 2413) then falls back to `line_text`, which gives the same string. So the value of the output does not change, only the allocation. Benign, and not in the design. No change is needed.
- `verbatim-text/src/terminal_rows.rs` 2754, the `thread_local!` `VOTES` counter. It is compiled only under `#[cfg(test)]`, so it is not reached in production.

## The outpost's reducer

Verdict: not pure. `Reducer::input` and `Reducer::tick` (`verbatim-outpost-logic/src/lib.rs` 561 and 652) take `Now`, which is three clocks given as input, and `&mut dyn Locals`. The shell reads `Now` in one place (`verbatim-outpost/src/outpost/shell.rs` 511 to 517).

### Calls through `Locals` (`lib.rs` 104 to 114)

`Locals` has three methods. The audit found no other trait object or callback reaching outside the state.

- `window_is_hung(hwnd)`.
  - Called from `lib.rs` 1234 through `intake.rs` 625 and 1228, and asked once per window per batch (`intake.rs` 1244).
  - Reads the system's `IsHungAppWindow` (`shell.rs` 310 to 312).
  - Reaches: the batch plan, so which entries run, in what order, and which are parked.
  - Sketch: covered as an answer to record.
  - Covered.
- `moved_on(window)`.
  - Called from `lib.rs` 1018.
  - Reads the foreground window and its thread (`shell.rs` 314 to 316).
  - Reaches: abandonment, so `Effect::Abandon` and the replies of the abandoned request.
  - Covered.
- `publication(request, facts)`.
  - Called from `lib.rs` 986.
  - Reads another thread's walk in progress (`Charge::publish`) and the object store (`shell.rs` 318 to 334).
  - Reaches: the `Change` committed, so the effects sent to Core and the facts kept.
  - Covered as an answer.
  - Not covered: the call also makes writes inside the transition. It pushes into the store's garbage (`shell.rs` 323), calls `write`, which writes the store and hands out element tokens (`shell.rs` 329 to 335), and logs (`shell.rs` 324).
    - These are effects carried out before the reducer returns, not effects it returns. A replay with recorded answers stays exact for the reducer. But the rule that a reducer's calls are its returned effects does not hold here.
    - The design says only "read and marked published".
    - Sketch: return `Effect::Publish(request)`, and have the shell answer with an `Input::Published(change)`. The design weighed this and rejected it, but its record should name the store writes.

### Process-wide trace id mints

These are all covered by the design's four sources.

- `lib.rs` 582, the "not started" reply to `Input::Cancel`. Reaches the trace id in `Effect::Send`. Sketch: the withdrawn entry's trace id.
- `lib.rs` 683, `gap_bound_entry`, used from `Input::GapBound` and from `tick`. Reaches the gap bound entry's trace, and so the trace of whatever it sends. Sketch: the gap record keeps the trace id of its newest copy.
- `lib.rs` 951, the `GapRead` entry for `GapNote::CopyDropped`. Reaches the read's trace. Sketch: the note carries the trace id of the copy's event.
- `intake.rs` 670, the `Item::Wake` entry. Reaches the wake's trace. Sketch: the trace id of the watch's deadline.

The mints in `follow.rs` 923, `hold.rs` 206 and 222, `intake.rs` from 1467 on, and `tests.rs` are in test modules.

### Hash maps and sets with a random seed

These are all covered.

- `gap_record.rs` 140, `answered`. Lookups only.
- `intake.rs` 1230 and 1233, `hung` and `per_thread`, and 1313, `runs`. `runs.values_mut()` (1329) sorts each run in place, so the order is not used.
- `text.rs` 363 and 364, `uia` and `edit`.
  - `release` (436 to 453) iterates them.
  - The order of the dropped ranges reaches `Effect::ForgetRanges` (`lib.rs` 627 to 630).
  - This is the one place where a hash order reaches an effect today.
- `verdicts.rs` 119, `windows`. Lookups only.
- `lib.rs` 389 and 391, `Input::Released`'s `nodes` and `anchors`. Only `contains` and `retain` (`held.rs` 164, `follow.rs` 694, `text.rs` 433).

### Shared state

- `Facts` holds `Arc<Verdicts>` and `Arc<TextFacts>`, shared with the dispatched programs and copied on write with `Arc::make_mut` (`lib.rs` 627 and 870). A worker never changes them, so the values do not depend on sharing.
- `Effect::Verdicts(Arc<Verdicts>)` hands the reducer's own value out.
- No `Mutex`, atomic, `static` or `thread_local!` in production code. No logging.

## The listener's read scheduler and grace

Verdict: pure apart from its two hash maps and its logging, which are covered.

- `Schedule::step` (`verbatim-listener-logic/src/schedule.rs` 453) takes the time `now` as a `u64` input.
- `TickRead` carries the moved-on answer the shell read, so the scheduler makes no call.
- `GraceState::step` (1090) and `ForegroundWatch` (`foreground.rs` 52) are pure.
- `nearest_timeout_reason` (`focus.rs` 106) takes the elapsed time as an argument.

Impurities:

- `schedule.rs` 338, `apps: HashMap<Pid, App<J>>`, with a random seed.
  - `tick` sorts its keys first (761 and 762).
  - `markers` sorts its result (388 to 403).
  - `moved_away_from_all_but` (589), the foreground walk (980) and `focus_pending` (439) do not depend on order.
  - Covered.
- `schedule.rs` 753, `tick`'s `reads`, a `HashMap`. Lookup and remove only. Covered.
- `tracing` events at `schedule.rs` 573, 678, 700, 875, 882, 896, 919, 948, 954 and 1020. Write only. No `enabled!` is read, and no field expression has a side effect. Covered.

## `verbatim-reader-rules`

Verdict: its functions are pure given their arguments. `accepts_event` and `foreground_relation` take live window queries through `WindowQueries` (`acceptance.rs` 87), but they are called only from the listener's and the outpost's workers in the Windows crates, never during a reducer transition.

- `acceptance.rs` 77, `RequestedEvents::requested`, a `HashSet` with a random seed. Lookup only. Covered.
- `acceptance.rs` 87, `WindowQueries`, live calls. Covered: not in a reducer.
- `class.rs`, `class_verdict` and `classify_with`, are pure over static tables.

## The supervisor's policy and generations

Verdict: not pure. They take time as `Instant`, which cannot be recorded or replayed.

Covered by the design:

- `policy.rs`:
  - 43 and 44, `wedge_decision`;
  - 65 and 66, `retirement_decision`;
  - 91 to 131, `Backoff`'s `due` and `failed`/`ended`;
  - 174 to 194, `HeldFacts`'s `first_held_at`;
  - 232 to 244, `Spares`' `wanted` and `due`.
- `policy.rs` 182, `HeldFacts::hold(fact, now: impl FnOnce() -> Instant)`. The shell passes `Instant::now` (`verbatim-supervisor/src/owner.rs` 955, 1675 and 1911), so the clock is read inside the logic's call. It reaches `first_held_at`, and so the logged hold time.
- `generation.rs` 51 and 59, `Live::last_pong_at`, set in the transitions at 369, 425 and 451.
- `mod.rs` 48, `Received::at`.
- Sketch for all of these: microseconds on the shared clock, as the design says.

Missed by the design:

- The shell writes the generation's liveness outside any transition.
  - `owner.rs` 801 and 802 set `live.last_pong_at = Instant::now()` and `live.abandoned`, through `Generation::live_mut` (`generation.rs` 310). Its fields are `pub` (`generation.rs` 49 to 56).
  - The policy's later output (`wedge_decision` at `owner.rs` 1968, 2000, 2033 and 2115) depends on state that no input of the generation's put there.
  - Sketch: a `Generation::ponged(at_us, abandoned)` transition, with the fields private.
  - The design converts the field's type but leaves this second writer.
- The shell keeps the listener's and the spares' liveness itself.
  - `owner.rs` 206 and 339 hold `last_pong_at: Instant` in the shell's own records. They are written at 633, 809, 1088, 1297, 1454 and 1574 with `Instant::now()`, and judged by `wedge_decision`.
  - Part of the policy's state therefore lives in the shell, outside the pure module, and the design's list of `Instant` uses does not name it.
  - Sketch: move these records into `supervisor` beside `Live`, with microsecond inputs.
- A supervisor shell decision depends on hash order.
  - `owner.rs` 291, 297 and 325 are `HashMap`s with a random seed (`records`, `apps`, `spare_children`).
  - `start_due` (`owner.rs` 1122 to 1133) collects the applications whose backoff has ended, in hash order, and calls `incarnate` for each. `Spares::take` gives the oldest spare first.
  - So when two backoffs end on the same wake, which application gets the oldest spare, and the order of their starts, depends on the hasher.
  - This is in the shell, not in the pure module, but it is a decision.
  - Sketch: `BTreeMap`, or sort the due pids, or move the choice into `Spares`.

## Core's request table

Verdict: not pure. `RequestTable` (`verbatim-app/src/requests.rs` 122) is the single owner of each query's one outcome, kept on the reducer thread. Its crate is outside the design's covered crates, so none of these impurities are covered.

- Hash order reaches reducer input order.
  - `requests.rs` 124 holds `entries: HashMap<RequestId, Entry>` with a random seed.
  - `outpost_ended` (211 to 222) and `expire` (228 to 247) collect the ids to end by iterating the map. They return the reducer `Input`s (`FetchCompleted`, `TextCompleted`, `ActivationCompleted`, focus-now events) in that order, and the reducer applies them in it (`main.rs` 1982 and 2011).
  - When one outpost ends with several requests outstanding, or several deadlines pass on one wake, the reducer's input order, and so the order of what it speaks, differs from process to process.
  - Sketch: `BTreeMap<RequestId, Entry>`. The ids are already allocated in increasing order (`insert`, 160 to 182), so this would also give the order the requests were made in.
- A deadline is an `Instant`.
  - `Entry::deadline` (117), `begin_until` (139, called with `Instant::now() + HELD_KEY_BOUND` at `main.rs` 1835 and with a deadline at 1927), `expire(now: Instant)` (228) and `next_deadline` (252).
  - Reaches which requests are ended as failed, and when.
  - Sketch: microseconds on the shared clock, as for the supervisor.
- Effects carried out inside the owner.
  - `deliver` (344 to 461) answers a `Responder` (310, 330), drops one so it answers (317, 337), sends on a channel (`reply.try_send`, 399), and logs.
  - These are calls made inside the owner's operation, not effects it returns. That is outside the definition of an output, but it breaks "calls as its effects".
  - Sketch: return an enum of deliveries (an input for the reducer, or a reply to an asker), and let the reducer thread carry them out.
- `DumpTicket::mint` (32 to 36), a process-wide `AtomicU64`. It is called by the control-plane caller, not by the table, so the table only stores the ticket. Not an impurity of the table.

## Other owners described as pure or as state machines

### The keyboard decision machine

`verbatim-input/src/state.rs` 319, `DecisionMachine`, is described as "The pure keyboard decision state machine". Verdict: not pure. `verbatim-input` is outside the design's covered crates by its own statement, so none of these impurities are covered.

- Shared state read on every key.
  - `map: Arc<ArcSwap<GestureMap>>` (374) is loaded at 534 and 573.
  - Another thread rebinds the gestures with an atomic store.
  - Reaches: whether a key is swallowed and a gesture emitted, or the key passed on and observed.
  - Sketch: a `set_map(GestureMap)` input, like `set_num_lock` (421), given by the hook thread when the binding changes.
- Process-wide trace ids.
  - `TraceId::mint()` at 171 (`pass_observed`) and 539 (a bound gesture).
  - Reaches: the trace id of every `EmittedGesture`.
  - Sketch: the hook passes the trace id in with the `KeyEvent`.
- A clock as an `Instant`.
  - `on_key(event, now: Instant)` (431). The hook passes `Instant::now()` (`verbatim-input-windows/src/hook.rs` 368).
  - It is stored in `last_gesture` (387) and `LoneModifier` (263), and reaches the multi-press `repeat` count and the double-tap pass-through.
  - Sketch: microseconds on the shared clock, or the key event's own `time` stamp.
- `HashSet`s with a random seed at 377 and 381 (`held_modifiers`, `swallowed_downs`).
  - `build_gesture` iterates `held_modifiers` (646 and 647), but `GestureId::parse` sorts the parts.
  - `is_typed_character` uses `all` (484).
  - So the order reaches no output. Benign, and a `BTreeSet` would make that structural.

### The idle barrier

`verbatim-app/src/idle.rs`, `IdleBarrier`. Verdict: pure.

- `unanswered: HashSet<OutpostId>` (50) is used only for membership.
- `standing` sorts before reporting (218 and 219).
- `W` is opaque and is never called.

### The GUI lifecycle and the outpost's terminal reading

`verbatim-gui/src/lifecycle.rs` and `verbatim-outpost/src/terminal/reading.rs`. Verdict: pure. No marker of any kind.

### The outpost's outbound backlog

`verbatim-outpost/src/outpost/outbound.rs` 91, `Pending`, is described as "Pure, so the merging and the numbering are tested on their own". Verdict: its merging and numbering are pure, but its `Backlog::encode` is not, and one of its reads reaches a decision. Not covered.

- `encode` stamps the message with `now_us()` (239). The stamp is for timing only.
- `encode` signals a flush's waiter on a channel (`done.send`, 232).
- `encode` hands the read's buffers to a process-wide `Mutex` pool (243, `verbatim-outpost/src/terminal.rs` 267).
- Hash order reaches a decision through `waiting_nodes`.
  - `reported: HashMap<Carried, Report>` (102) has a random seed.
  - `waiting_nodes` (183 to 192) returns nodes in hash order.
  - The worker appends them to the candidates for a state change (`worker.rs` 2702 to 2706), and `which_of` (`verbatim-ia2/src/acquire.rs` 223 to 227) takes the first candidate whose key matches.
  - The order would decide only if two candidates had the same key. It is in a worker, not a reducer.
  - Sketch: `BTreeMap`.

### The supervisor's writer queue

`verbatim-supervisor/src/writer.rs` 302, `Queue`, is described as "Pure, so the overload policy is tested on its own". Verdict: `push` and `pop`, the overload policy, are pure. `encode` stamps the clock through `relayed` (228 and 229) for the latency stamps, which reach no decision. Not covered. A timing stamp only.

## Gaps in the design's rule itself

- `verbatim-model` is a covered crate, and `theme.rs` 425 calls `f64::exp2` in `progress_frequency`, which on Windows goes to the platform's C library.
  - The design checks third-party floating point (icu's LSTM, through `libm`), but not the standard library's transcendental functions in the covered crates' own code. Clippy's configuration does not name them.
  - `progress_frequency` is called only by `verbatim-speech` today, so no reducer output reaches it.
  - Sketch: a `disallowed-methods` entry for `f64::exp2`, `powf`, `ln`, `exp` and the like, or a `libm` call.
- The design's scope is the crates that reducers depend on. Three owners that decide reducer inputs or keys sit outside it: `RequestTable` and `IdleBarrier` in `verbatim-app`, and `DecisionMachine` in `verbatim-input`. The supervisor shell's own choices (`owner.rs`) are outside it too. Of these, the request table and the decision machine are not pure.
