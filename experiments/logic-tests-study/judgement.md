# Mutation baseline for M4.6

Baseline commit be5bd5b9, cargo-mutants 27.1.0, run in place in a worktree, each crate's own tests only. Result: the generated tests kill 49 of the 71 target mutants (69%), against a threshold of 22 and at least one in each crate.

Command lines, run from the worktree root (the covered files are the ones named; the nightly workflow `.github/workflows/explore.yml` lists them as its groups):

- `cargo mutants --in-place -p <crate> -f <covered file>`, once per crate and file, with `-F` limiting `crates/verbatim-core/src/reduce.rs` to `foreground_changed|leaves_focus|observed_before_foreground_change|outpost_ended` and `crates/verbatim-core/src/terminal_model.rs` to `ask_model|model_asks|answered_read|reply|answerable`.
- `cargo mutants --in-place -p verbatim-outpost-logic --test-package verbatim-outpost-logic --test-package verbatim-outpost --re <alternation of the missed mutants>`
- `cargo mutants --in-place -p verbatim-listener-logic --test-package verbatim-listener-logic --test-package verbatim-listener --re <alternation of the missed mutants>`

The last two add the role crates' tests, which also drive the real reducer. They killed 10 of the outpost's missed mutants (all excluded) and 11 of the listener's (all targets), marked below.

## Classes

Each missed mutant classed on 2026-11-02: equivalent (cannot change any effect or later behaviour), excluded (in what the owner's reference model leaves out by design, as the owner's crate guide states), or target (a gap the generated tests should kill). "NL" marks a target that no check planned at the time covered; checks were added for them. The checks quoted below are the planned invariants and postconditions of each owner's generated test.

Two exclusions beyond the design's own list: `FocusPath` in `follow.rs` (11), worker code with no caller in the reducer, tested from `verbatim-outpost`; and the watch answer's timing in `lib.rs` (6), measurement contents no decision reads.

Counts, after the role crates' tests were run on the missed mutants (below, "Killed by the role crates' tests"):

- verbatim-reader-rules (4): equivalent 0, excluded 0, target 4.
- verbatim-outpost-logic (101): equivalent 3, excluded 61, target 37.
- verbatim-listener-logic (22): equivalent 2, excluded 0, target 9, and 11 killed by `verbatim-listener`'s tests.
- verbatim-core (24): equivalent 1, excluded 2, target 21.
- All (151): equivalent 6, excluded 63, killed by the role crates' tests 11, target 71. The threshold, 30% of the targets, is 22 killed, and at least one in each crate.

The outpost's 10 mutants killed by `verbatim-outpost`'s tests were all excluded, so its counts stand. The role crates' runs cover their unit and integration tests, not `crates/mockapp/tests`, whose tests exercise the subscription and the verdicts against a real application and are too slow to run once per mutant; an excluded mutant left here may still be caught there.


## Classes of the missed mutants

Code at be5bd5b9 (`git diff be5bd5b9 -- crates/` is empty, so main's lines are the same). "NL" means target with no planned check.

### verbatim-reader-rules

#### in_flight.rs: InFlight::deadline
- in_flight.rs:425, body None: target, NL. No listed invariant compares the `deadline()` query with the model (the fate scan and the wake check use `next_wake`).
- in_flight.rs:425, body Some(0): target, NL. Same reason.
- in_flight.rs:425, body Some(1): target, NL. Same reason.

#### in_flight.rs: InFlight::item_mut
- in_flight.rs:430, body None: target, NL. No listed check reads `item_mut()`; its only callers are in the outpost reducer.

### verbatim-outpost-logic

#### follow.rs: FocusPath::of, is_cut_at, route_to
The reducer never calls these. They are worker-side helpers, called from `verbatim-outpost` (worker.rs, mod.rs). That crate's `focus_path.rs` tests cover `of` and `route_to`, but the run used only outpost-logic's own tests, and no test anywhere calls `is_cut_at`.
- follow.rs:149, body Default: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`; killed by verbatim-outpost's tests.
- follow.rs:157, != to ==: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`.
- follow.rs:167, body false: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`.
- follow.rs:167, body true: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`.
- follow.rs:168, && to ||: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`.
- follow.rs:168, == to !=: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`.
- follow.rs:169, && to ||: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`.
- follow.rs:178, == to !=: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`; killed by verbatim-outpost's tests.
- follow.rs:178, body None: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`; killed by verbatim-outpost's tests.
- follow.rs:183, + to *: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`; killed by verbatim-outpost's tests.
- follow.rs:183, + to -: excluded, not the reducer's: `FocusPath` is worker code the reducer never calls, tested from `verbatim-outpost`; killed by verbatim-outpost's tests.

#### follow.rs: FollowState::retarget, follows, release_groups
- follow.rs:485, == to !=: excluded, the subscription (kept groups).
- follow.rs:501, += to *=: excluded, the subscription.
- follow.rs:501, += to -=: excluded, the subscription.
- follow.rs:513, == to !=: excluded, the subscription.
- follow.rs:556, += to *=: excluded, the subscription.
- follow.rs:556, += to -=: excluded, the subscription.

#### follow.rs: apply
- follow.rs:648, == to !=: excluded, the subscription (retarget against unfollow of an element change).
- follow.rs:661, && to ||: target. Owed work done is cleared for the wrong node or generation. Killed by the model's pending owed work and invariant 12 ("Owed work is tried at most once by each generation...").
- follow.rs:661, == to != (node): target. Same check.
- follow.rs:661, == to != (generation): target. Same check.

#### follow.rs: apply_watch
- follow.rs:696, && to ||: target. `resumed` clears `deferred` after a program answered the watch. Killed by the model's "whether deferred" and invariant 8 ("none ... while a deferred program is in flight").

#### follow.rs: answer_to_judge
- follow.rs:732, == to !=: excluded, held reports.
- follow.rs:734, delete field event: excluded, held reports.
- follow.rs:735, delete field simple_child: excluded, held reports.

#### gap_record.rs: GapRecord::foreground
- gap_record.rs:345, && to ||: target. Whether a record awaits a focus or a title is wrong. Killed by the model's gap record ("awaiting a focus or a title").
- gap_record.rs:345, guard to true: target. Same check.

### hold.rs
- hold.rs:91, Hold::held to empty: excluded, `waiting`. Its only use is `dispatch_finish`'s waiting flag, the program's input about entries waiting.
- hold.rs:172, forwarded_time guard to true: target, NL. It changes which held copies the release drops and the newest forwarded time. The model tracks only "holding", and no check names the release's drop rule.
- hold.rs:179, forwarded to true: target, NL. Same.

### intake.rs
- intake.rs:176, delete - (OBJID_CLIENT 4): target, NL. Every test and the generator use the constant by name, so none can see its value.
- intake.rs:392, Marks::drops_on_arrival delete Caret arm: excluded, the contents of the marks. It is called by the shell, never by the reducer or the intake's push.
- intake.rs:521, < to <=: target. The resolving marker's place is wrong after a removal, so `self.waiting[place]` overwrites another entry. Killed by the intake's "each entry pushed is planned once or dropped with its reason".
- intake.rs:521, < to ==: target. Same check.
- intake.rs:521, < to >: target. Same check.
- intake.rs:523, -= to +=: target. Same check.
- intake.rs:523, -= to /=: target. Same check.
- intake.rs:600, > to >=: target, NL. An empty plan raises the batch number. Only programs see batch numbers, and no check names them.
- intake.rs:605, += to *=: target, NL. Every batch is numbered 0.
- intake.rs:649, Intake::cancel delete !: target. The planned batch keeps only the cancelled query. Killed by "planned once or dropped with its reason" and the outpost's cancel postcondition.
- intake.rs:711, batch_number to 0: target, NL. Batch numbering is unchecked.
- intake.rs:711, batch_number to 1: target, NL. Same.
- intake.rs:718, waiting_carets to vec![]: excluded, `waiting`. It exists only to fill `Reducer::waiting`.
- intake.rs:722, / to %: excluded, `waiting`.
- intake.rs:722, / to *: excluded, `waiting`.
- intake.rs:722, >= to <: excluded, `waiting`.
- intake.rs:786, old_focus_events_first place+1 to place*1: equivalent. The only extra index visited is `place`, the focus marker itself, a `Fact`, which is never an old focus's UIA state change (at place 0 the range is unchanged).
- intake.rs:873, remove_waiting delete !: target, NL. The backward search stops at once, so a keyed entry before `at` is not replaced (and nothing is replaced when `at` is the end). The intake's checks are safety only, and none requires a replacement.
- intake.rs:875, + to *: target. The wrong entry is removed as replaced. Killed by "planned once or dropped with its reason".
- intake.rs:875, + to -: target. Same check.
- intake.rs:975, caret_after_update && to ||: target. Any keyed entry is moved as the caret. Killed by "nothing is planned ahead of an earlier arrival except under those documented exceptions".
- intake.rs:983, skip(at+1) to skip(at*1): equivalent. The extra index is the caret entry itself. Its key is a caret key, never a text change or a console update, and it is an MSAA or UIA entry, so it neither breaks the loop nor counts.
- intake.rs:988, && to ||: target. The caret is moved behind any keyed event. Killed by the same order check.
- intake.rs:1128, admit += to *=: target, NL. The per-thread count of a foreground event stays 0, so the limiter keeps more. None of the intake's three checks requires a drop.

#### lib.rs: Reducer
- lib.rs:728, input delete !: target. Close sends Unpark of nothing, and none when there are parked objects. Killed by the close postcondition (exact effects).
- lib.rs:748, tick && to ||: target. The gap bound is tested before it is due. Killed by the tick postcondition ("then the gap bound") and the model's bound waiting or passed.
- lib.rs:834, gap_bound_tested && to ||: target. A record awaiting a title queues a bound read. Killed by the model's gap record.
- lib.rs:834, guard to true: target. Same.
- lib.rs:945, send == to !=: target. Another reply clears the program's query, so its own query gets no reply. Killed by invariant 2 ("each query has at most one reply, and exactly one by the end of the case").
- lib.rs:1080, delete awaited_at_us: excluded, measurement contents: the watch answer's timing, which no decision reads.
- lib.rs:1081, delete awaited_calls: excluded, measurement contents. Same.
- lib.rs:1082, delete calls: excluded, measurement contents. Same.
- lib.rs:1083, delete published_at_us: excluded, measurement contents. Same.
- lib.rs:1139, released delete !: excluded, text facts (ForgetRanges).
- lib.rs:1276, finished && to ||: target. A panicked or published program's cut still dispatches a finish. Killed by the "a program finished (current, stale, cut, panicked)" postcondition.
- lib.rs:1300, finished || to &&: target. A reported try continues its group. Killed by the program-finished postcondition and the model's "a group's remaining tries".
- lib.rs:1430, pump += to *=: target. Every focus group is numbered 0. Killed by the model's program in flight, "its focus group".
- lib.rs:1468, intake_batch to 0: excluded, the intake's plan inside the outpost's test.
- lib.rs:1468, intake_batch to 1: excluded, the same.
- lib.rs:1484, run delete field item: excluded, the intake's plan. With the field deleted the barrier is not marked as already behind a noted caret, so it waits behind a later one too, which is the plan's documented "barrier behind a noted caret".
- lib.rs:1538, run guard to false: target. A shell query past its walk time is dispatched. Killed by the postcondition "a shell query past its walk time (an empty reply, no dispatch)".
- lib.rs:1545, run guard to true: target. Same postcondition.
- lib.rs:1560, live to vec![]: equivalent. `run` is reached only from `pump` while `!busy()`, so `flight.running()` is None and `live()` is already empty.
- lib.rs:1665, dispatch_finish == to !=: excluded, `waiting` (the finish program's waiting input).
- lib.rs:1681, waiting to vec![]: excluded, `waiting`.
- lib.rs:1682, waiting delete arm: excluded, `waiting`.
- lib.rs:1733, waiting_caret to None: excluded, `waiting`.
- lib.rs:1734, delete TextSelection arm: excluded, `waiting`.
- lib.rs:1739, delete ConsoleCaret arm: excluded, `waiting`.
- lib.rs:1744, delete Caret arm: excluded, `waiting`.
- lib.rs:1772, clears_absorb to true: excluded, `clears_absorb`.
- lib.rs:1790, owed_clears_absorb to false: excluded, `clears_absorb`.
- lib.rs:1790, owed_clears_absorb to true: excluded, `clears_absorb`.
- lib.rs:1806, budget delete FocusNow arm: excluded, `budget`.
- lib.rs:1807, budget delete Ancestors arm: excluded, `budget`.
- lib.rs:1812, budget delete Text arm: excluded, `budget`.
- lib.rs:1850, watch_ended delete calls: excluded, measurement contents: the watch answer's timing, which no decision reads.
- lib.rs:1851, watch_ended delete published_at_us: excluded, measurement contents. Same.
- lib.rs:1873, gap_report delete the focus-now arm: target. A focus-now answer no longer closes the gap record. Killed by the model's gap record.

### verdicts.rs
- verdicts.rs:42, Probed::lasts to false: excluded, verdict contents.
- verdicts.rs:42, to true: excluded, verdict contents.
- verdicts.rs:98, Kept::judges < to >: excluded, verdict contents; killed by verbatim-outpost's tests.
- verdicts.rs:102, guard to false: excluded, verdict contents; killed by verbatim-outpost's tests.
- verdicts.rs:146, Verdicts::lasts to false: excluded, verdict contents. Invariant 13 checks sends against the record's actual change, and the model does not predict it.
- verdicts.rs:148, && to ||: excluded, verdict contents.
- verdicts.rs:148, == to !=: excluded, verdict contents.
- verdicts.rs:171, == to !=: excluded, verdict contents; killed by verbatim-outpost's tests.
- verdicts.rs:171, guard to false: excluded, verdict contents; killed by verbatim-outpost's tests.
- verdicts.rs:171, guard to true: excluded, verdict contents.
- verdicts.rs:172, >= to <: excluded, verdict contents; killed by verbatim-outpost's tests.

#### watch.rs: Watch::take
- watch.rs:237, guard to true: target. A program takes a waiting watch on another node. Killed by invariant 7 and the model's watch "held by the program".

### verbatim-listener-logic

### schedule.rs
- schedule.rs:422, stopping to false: target, NL. No listed check compares the `stopping()` query, which the shell reads; killed by verbatim-listener's tests.
- schedule.rs:422, stopping to true: target, NL. Same; killed by verbatim-listener's tests.
- schedule.rs:465, focus_pending == to !=: target, NL. No check compares `focus_pending`, which the shell's menu handling reads; killed by verbatim-listener's tests.
- schedule.rs:618, push && to ||: target. The wrong events are replaced. Killed by "every marker gets exactly one outcome and the open markers equal the model's" and "jobs in push order, less those replaced or dropped"; killed by verbatim-listener's tests.
- schedule.rs:633, != to ==: target, NL. A job's `after_elsewhere` is inverted. The model tracks the last focus's application, but no invariant compares the job's `after_elsewhere`.
- schedule.rs:658, + to *: target. Killed by "no event waits past its application's bound at the limit once a tick or push at or past it is handled"; killed by verbatim-listener's tests.
- schedule.rs:734, next == to !=: target. An application with abandoned threads is removed, so their return sends no forget-last. Killed by "one forget-last per return from an abandoned thread".
- schedule.rs:753, returned && to ||: target. `last_top` is overwritten by a dropped read. Killed by "each job's moved-away place as the model computes it".
- schedule.rs:775, < to <=: equivalent. `abandoned` never exceeds ABANDONED_LIMIT (threads start only below it), so after the decrement it is below the limit either way.
- schedule.rs:775, < to ==: target. The limit time is never cleared. Killed by the model's "when the limit was reached" and "every marker gets exactly one outcome" (later events dropped wrongly).
- schedule.rs:775, < to >: target. Same.
- schedule.rs:883, abandon && to ||: target. The limit time is set before the limit. Same checks.
- schedule.rs:941, published && to ||: target. A publication answer of another request publishes the current read. Killed by "a published read gets no second outcome" and markers exactly once.
- schedule.rs:972, drop_waiting || to &&: equivalent. Both callers (push and tick through `waiting_end`) already ensure an unthreaded application with a non-empty queue, so neither guard returns.
- schedule.rs:1011, foreground == to !=: target. Another application's read's top is used. Killed by "each job's moved-away place as the model computes it".
- schedule.rs:1023, outpost_ended body (): target, NL. The application is neither removed nor woken to end. No listed check compares the applications kept or the end's Wake; killed by verbatim-listener's tests.
- schedule.rs:1026, delete !: target. A threaded application is removed, and a new life reuses generation numbers. Killed by "each spawn's generation above every earlier one within the application's life"; killed by verbatim-listener's tests.
- schedule.rs:1026, && to || (first): target. An application with abandoned threads or a queue is removed. Killed by forget-last and markers exactly once; killed by verbatim-listener's tests.
- schedule.rs:1026, && to || (second): target. A threaded idle application is removed. Killed by the spawn-generation invariant; killed by verbatim-listener's tests.
- schedule.rs:1026, == to !=: target. An application with abandoned threads is removed. Killed by forget-last.
- schedule.rs:1035, spawn_failed body (): target. The queue gets no outcome. Killed by "every marker gets exactly one outcome"; killed by verbatim-listener's tests.
- schedule.rs:1038, delete !: target. Same; killed by verbatim-listener's tests.

### verbatim-core

### queries.rs
- queries.rs:230, find_mut && to ||: excluded, carets and words. Its callers are editing's caret pending, and the change shows only as caret speech or the heard list.
- queries.rs:302, next to 0: target, NL. `set_traces` re-traces older queries an effect names. "each new entry's trace" checks only new entries. (It would be equivalent if no effect ever names an older query; that was not proved.)
- queries.rs:302, next to 1: target, NL. Same.
- queries.rs:310, == to !=: target. Traces are skipped exactly when queries were made. Killed by "each new entry's trace is its input's, or the key's inside a key's effects".
- queries.rs:310, set_traces body (): target. Same.
- queries.rs:319, set_trace body (): target. Same.
- queries.rs:320, delete ForKey arm: target. Same (the key's trace).
- queries.rs:326, >= to <: target. Same.
- queries.rs:435, outpost_gone && to ||: target. Other outposts' entries end. Killed by the per-outpost-end postcondition ("its entries removed in number order").

### reduce.rs
- reduce.rs:519, observed_before_foreground_change < to <=: excluded, attention for events that are not the focus (background speech only).
- reduce.rs:604, leaves_focus < to <=: target. With equal stamps the focus is left. Killed by the model's focus "left" and the per-foreground-change postcondition.
- reduce.rs:1418, outpost_ended == to !=: target. Say-all in the ended outpost goes on. Killed by "after an outpost ends ... no ... say-all".
- reduce.rs:1425, == to !=: target. Killed by "no ... caret ... in it".
- reduce.rs:1430, delete Caret or Text arm of the forget: target. The dead outpost's caret and text continuations then run on Gone. Killed by the per-outpost-end postcondition (exact effects, "nothing asked of it").
- reduce.rs:1431, == to !=: target. Same.
- reduce.rs:1438, == to !=: target. Killed by "no ... start marker in it".

### terminal_model.rs
- terminal_model.rs:905, answered_read to None: target. No snapshot is taken, so no model is made or asked. Killed by the model's "live terminal models as node and generation in least-recently-used order".
- terminal_model.rs:905, to Some(default): target. Same.
- terminal_model.rs:912, delete TerminalSnapshot arm: target. Same.
- terminal_model.rs:916, delete TerminalUnchanged arm: equivalent. The wildcard's None is unwrapped by the one caller (terminal.rs:458, `unwrap_or_default`) to the same empty triple.
- terminal_model.rs:973, reply delete !: target. Waiting reviews (memory entries) are answered too early or never. Killed by the model's open memory entries and the per-answer postcondition.
- terminal_model.rs:974, delete !: target. Same.
- terminal_model.rs:974, && to ||: target. Same.
- terminal_model.rs:1098, answerable delete ForKey arm: target. A key's request goes to the outpost, not to memory. Killed by the model's open queries' waited kind (memory).

## Targets still missed

After the generated tests, 22 of the 71 targets are not killed: 20 still missed and 2 that no longer exist as written. The next checks to add, when a build changes their owner.

- verbatim-outpost-logic (8): follow.rs:696 (`apply_watch`, && to ||), hold.rs:172 (`forwarded_time`, guard to true), hold.rs:179 (`forwarded`, body true), intake.rs:521 (< to <=), intake.rs:975 and intake.rs:988 (`caret_after_update`, && to ||), lib.rs:1545 (`run`, guard to true), lib.rs:1873 (`gap_report`, delete the focus-now arm).
- verbatim-listener-logic (1 and 2): schedule.rs:941 (`published`, && to ||) is still missed; schedule.rs:734 and schedule.rs:1026 moved into `App::spent`, which has no missed mutant.
- verbatim-core (11): queries.rs:302 (`Queries::next`, body 0), reduce.rs:1430, 1431 and 1438 (`outpost_ended`), terminal_model.rs:905 (two mutants) and 912 (`answered_read`), 973 and 974 (`reply`, three mutants), and 1098 (`answerable`).
