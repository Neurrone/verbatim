# Review of the determinism design

The design reviewed is `docs/design/logic-foundations.md`, section "Determinism (a build of its own)" (lines 526 to 643 of the working copy on 2026-10-31). It was read against the crate split's worktree (`verbatim-wt-split`, branch `split`, head dcb9be0b; the design names fa1ce8e5, and the one commit between them changes only the tier check). Design line numbers are the working copy's; code paths are given from `crates/` in that worktree. Two Clippy behaviours were checked on a scratch crate with Clippy 0.1.99, the version the design used.

Important findings: 6.

## Does the design reach its goal

Not yet. Most of the audit holds. The hash map list is complete for the six crates' own code: every `HashMap` and `HashSet` in them is listed, and the order claims check out. `verbatim-speech` really is off the path: no covered crate depends on it, and its four maps are only looked up, never iterated. `jieba-rs` 0.11 uses `rustc-hash`'s fixed hasher and keeps per-thread scratch buffers that do not change its output. But the goal, "a reducer's effects identical for identical inputs", fails in three places. The outpost's reducer reads live system state and another thread's progress through an injected trait that the audit does not mention (finding 1). Three production trace id mints are missing from the audit (finding 2). And the enforcement can be silenced, or bypassed for seeded maps and clock reads, in ways the scan does not see (findings 3 to 5). The performance plan's worst case is too small, and the threshold that would trigger its fallback is not defined (finding 6).

No sequence was found in which moving `verbatim-supervisor` from `Instant` to shared-clock microseconds changes a value any decision uses. `now_us` reads the same performance counter as `Instant`, and a wait computed from it never wakes before the due time. The only effects are on a logged figure and on the number of clock reads (see the nits).

## Important findings

### 1. The outpost's reducer reads live state through `Locals`, which the audit leaves out

- Where: design line 581 ("No production code in the six crates reads a clock, ... The hidden inputs are these"). Code: `verbatim-outpost-logic/src/lib.rs` 105 to 117 (`trait Locals`), 1008 (`locals.publication`), 1052 (`locals.moved_on`), 1325 (`locals.window_is_hung` passed to the intake), `intake.rs` 1231. The shell's implementation is `verbatim-outpost/src/outpost/shell.rs` 348 to 375.
- Defect: three answers that are not inputs reach a transition's effects, and the design neither names them as inputs nor brings them under the rule:
  - `window_is_hung` is `IsHungAppWindow`;
  - `moved_on` is a live foreground query;
  - `publication` reads a worker thread's walk in progress (`Charge::publish`) and builds a `Change` from it.
- Counterexample: the same input sequence is run twice, with a walk armed for request R. `Input::Tick` reaches `judge_abandonment`, and the program's deadline passes, so `publish_at_bound` calls `locals.publication(R, ..)`.
  - In run one, the worker has read the focus's name, so the commit carries it and Core is told the named focus.
  - In run two, the worker thread was descheduled a moment longer, so the progress has no name and the effects differ.
  - Likewise, `admit` drops a batch's UIA events when `IsHungAppWindow` answers true at that instant, and keeps them otherwise.
  - M4.6's "run each case twice" uses the tests' `Recorded` locals, so it cannot catch this. The recorder of reducer inputs for replay (logic-tests.md, "Recorded inputs replayed against NVDA") replays different effects unless each answer is recorded.
- Fix: state in the design that each `Locals` answer is an input of the transition, list the three, and require the input recorder to record each answer in call order. `publication`'s thread read goes in the limitations, as the one cross-thread read left inside a transition.

### 2. Three production trace id mints in the outpost's reducer, and model's own mints, are missing from the audit

- Where: design lines 584 and 585. Code: `verbatim-outpost-logic/src/lib.rs` 584 (`Input::Cancel` replies `NotStarted` with `TraceId::mint()`), 688 (`gap_bound_entry`, `Item::GapBound`), 973 (`Item::GapRead` for a dropped gap copy); `verbatim-model/src/protocol/samples.rs` 22 and 48 (a public, non-test module); `verbatim-listener-logic/src/schedule.rs` 1156 (a test).
- Defect: the design names only the wake's mint (`intake.rs` 670) as production, and counts "47 test sites in `verbatim-core` and `verbatim-outpost-logic`". The 47 are in fact 43 test sites and 4 production sites. The listener-logic test site and verbatim-model's 10 call sites are not counted.
- Counterexample: Core cancels request 9 twice, in two runs whose earlier history differs only in how many events the outpost's process observed. The `NotStarted` reply's trace id differs between the two runs, so the effect differs. When the enforcement lands, Clippy denies all of these sites. The disallowed path `verbatim_model::TraceId::mint` resolves inside `verbatim-model` itself: a scratch crate disallowing its own `dt::T::mint` was reported at its own call. So `samples.rs` and model's tests fail `cargo xtask ci`.
- None of these sites has a designed source for its trace id:
  - `Input::Cancel(request_id)` carries none;
  - the gap bound is a timer;
  - a `GapNote::CopyDropped` carries none.
- So a builder must either stop or improvise.
- Fix: give each a source in the design. For example, the cancel's trace id comes with `Input::Cancel` from Core's message, the gap bound's comes from the entry that opened the gap record, and the gap read's comes from the copy's own fact. Say what `samples.rs` uses (the new constructor), and count every site.

### 3. A group-level `allow` silences both lints without naming them

- Where: design line 602 ("No covered crate's code names `disallowed_types` or `disallowed_methods`").
- Defect: both lints belong to Clippy's `style` group, which `clippy::all` contains. So `#[allow(clippy::style)]`, `#![allow(clippy::all)]`, or the same in a crate's own `[lints.clippy]` table in place of `lints.workspace = true`, lifts them. None of these names either lint.
- Counterexample: checked on a scratch crate with the design's `clippy.toml` entries. With `#[allow(clippy::style)]` on a function holding `std::collections::HashMap::new()`, Clippy reports nothing. The same body without the attribute is reported twice.
- Fix: the scan rejects any `allow` or `expect` naming `clippy::style`, `clippy::all`, `disallowed_types` or `disallowed_methods` in a covered crate. It also checks that each covered crate's manifest has `lints.workspace = true` and no `[lints]` table of its own, and that the workspace table does not lower these lints.

### 4. Third-party maps with a per-process seed pass the enforcement

- Where: design lines 593, 603 and 605.
- Defect: Clippy reports a disallowed type only where the source names it. The design's own check shows that a third-party alias over `std::collections::HashMap` (`rustc_hash::FxHashMap`) is not reported. So a seeded alias or map from a dependency passes too:
  - `foldhash::HashMap`;
  - `hashbrown::HashMap`, whose default hasher is foldhash's per-process `RandomState`;
  - any type whose `S = RandomState` default is never written.
- The scan narrows `std::collections` paths only for the outpost's reducer and the read scheduler. It puts no limit on the third-party dependencies of `verbatim-core`, `verbatim-text`, `verbatim-model` or `verbatim-reader-rules`. The design itself rejects `foldhash` because "a crate could take the random one by its default name", but nothing stops a covered crate from doing so.
- Counterexample: a later change adds `hashbrown` to `verbatim-text` and holds a terminal's rows in a `hashbrown::HashMap<u64, usize>` that it iterates to build an effect. Clippy and the scan both pass, and the order changes with each process.
- Fix: the scan holds each covered crate's normal third-party dependencies to an allow-list: today's set plus `rustc-hash` without its `rand` feature. A new dependency is then a reviewed change to that list.

### 5. Clock and other input reads that the method list misses

- Where: design line 596.
- Defect: the list bans `Instant::now` and `SystemTime::now` and the two types by name. A clock read that names neither passes, and so do reads of other hidden inputs:
  - `std::time::UNIX_EPOCH.elapsed()`, where `UNIX_EPOCH` is a constant and `elapsed` is not listed. Checked on the scratch crate: not reported.
  - `Instant::elapsed` and `SystemTime::elapsed` on a value from elsewhere.
  - `std::fs` reads, `std::env::current_exe`, `std::thread::current`, `std::thread::available_parallelism`, and `std::process::Command`.
- Counterexample: `let since = std::time::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_micros());` in `verbatim-core` passes `cargo xtask ci` under the design's configuration, and every effect computed from it differs between runs.
- Fix: add `std::time::SystemTime::elapsed`, `std::time::Instant::elapsed`, `std::time::SystemTime::duration_since` and the listed `std::fs`, `std::env`, `std::thread` and `std::process` entries. Each is shown to fire once, as the design already plans for the existing entries.

### 6. The alignment's worst case is too small, and its fallback has no trigger and a wrong scope

- Where: design lines 617 and 621.
- Defect: the measured worst case is "about 9,100 rows" because both terminals keep 9,001 rows by default (`docs/design/terminal-model.md` line 124). That is a default, not a bound. An application in the console host can grow its own screen buffer to 32,766 rows (`SetConsoleScreenBufferSize`, or `mode con lines=32766`), and a lost read then reads all of it. The fallback is taken "if its time grows past the read's budget", but no budget is stated, so the measurement cannot decide.
- The fallback also names "the six transient row-hash sets", but the six maps keyed by row hash (line 568) include `Aside::rows`. That map is not transient: it is kept in Core's state, serialized in dumps, and looked up on reads while a full-screen program runs. The transient ones are five, and the sixth `Spread` map, `shifts`, is classed as minted.
- Counterexample: a program sets a 32,766-row buffer, fills it, and clears it. This is the lost read of `conhost_full_history_flood` at 3.6 times the rows. Each of the six maps becomes a B-tree about 5 levels deep of about 32,000 keys, against one probe of the `Spread` table today, over several passes. The design gives no figure that this cost is checked against.
- Fix: measure at 32,766 rows as well as 9,100, and state the budget, such as the lost read's time today plus a stated margin. Name exactly which maps the fallback converts, and say what `Aside::rows` becomes (a sorted vector serializes as a sequence, which changes the dump's format).

## Nits

- `verbatim-text/src/terminal_rows/align.rs` 121 and 158: `Likeness::other` becomes a `BTreeMap<char, u16>` looked up for every non-ASCII character of every row pair compared in a gap. It is per read and unmeasured, since the scratch test's flood is ASCII. Add a CJK gap to the scratch test.
- `verbatim-supervisor/src/owner.rs` 949 to 955: if `hold` is given the `now` read at 949, `first_held_at` moves earlier by `check_served_by`'s provider lookup, which shifts the logged `first_fact_held_us` (focus-pipeline 19.6). If `now_us()` is read at each `hold` call (lines 955, 1675 and 1911), there is one clock read per held fact where today there is one per application. State which, and read at 955's point.
- `wedge_decision` and `retirement_decision` (`verbatim-core/src/supervisor/policy.rs` 54 and 73) use `saturating_duration_since`. The design should say that the microsecond forms use `saturating_sub`, and that `a_now_before_the_last_pong_never_panics` keeps that case with constants. A plain `-` panics in the debug builds the end-to-end suite runs.
- `verbatim-text/src/terminal_rows.rs` 1671 (`commonest_shift`'s `shifts`): the shifts come from which rows of the application's text matched where, so the application influences them. They are bounded by the rows read, so collision flooding is not possible, but "Verbatim mints the key" is the wrong reason. Say "bounded integers Verbatim computes".
- `verbatim-term-sim` (a dev-dependency of `verbatim-core` and `verbatim-text`) keeps standard `HashMap`s in `src/oracle.rs` 620 and 739 to 765. M4.6 builds on it, but the rule "the crates' tests follow the same rules" does not reach it, because the derived list follows normal dependencies only.
- Test sites that build the `Input::Released` sets (`follow.rs` 1120, `held.rs` 260, `text.rs` 498 and 547, `tests.rs` 628 and 629) change with the type. They are not among the "2 in tests".
- The design names fa1ce8e5; the branch is now at dcb9be0b.
