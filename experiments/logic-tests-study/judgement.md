# Mutation baseline for M4.6

- Date: 2026-11-02 to 2026-11-03
- Commit: be5bd5b9
- Tool: cargo-mutants 27.1.0, run in place in a worktree, one mutant at a time (`--jobs 1` is refused together with `--in-place`, and one job is the default), each crate's own tests only. `CARGO_TARGET_DIR` inside the worktree, `VERBATIM_WX_DIR` at the main checkout's `target/wxwidgets`.
- Run time: 55 minutes of wall clock for all six runs (reader-rules 3 min, outpost-logic 31 min, listener-logic 4 min, core 9 + 5 + 4 min), first build included.
- Command lines, run from the worktree root:
  - `cargo mutants --in-place -p verbatim-reader-rules -f crates/verbatim-reader-rules/src/in_flight.rs`
  - `cargo mutants --in-place -p verbatim-outpost-logic -f` each of `lib.rs`, `watch.rs`, `gap_record.rs`, `follow.rs`, `verdicts.rs`, `hold.rs`, `intake.rs` under `crates/verbatim-outpost-logic/src/`
  - `cargo mutants --in-place -p verbatim-listener-logic -f crates/verbatim-listener-logic/src/schedule.rs -f crates/verbatim-listener-logic/src/foreground.rs`
  - `cargo mutants --in-place -p verbatim-core -f crates/verbatim-core/src/queries.rs`
  - `cargo mutants --in-place -p verbatim-core -f crates/verbatim-core/src/reduce.rs -F 'foreground_changed|leaves_focus|observed_before_foreground_change|outpost_ended'`
  - `cargo mutants --in-place -p verbatim-core -f crates/verbatim-core/src/terminal_model.rs -F 'ask_model|model_asks|answered_read|reply|answerable'`

## verbatim-reader-rules

Caught 42, missed 4, unviable 53, timeout 0.

### in_flight.rs: InFlight<T>::deadline

- in_flight.rs:425: replace body with None
- in_flight.rs:425: replace body with Some(0)
- in_flight.rs:425: replace body with Some(1)

### in_flight.rs: InFlight<T>::item_mut

- in_flight.rs:430: replace body with None

## verbatim-outpost-logic

Caught 413, missed 101, unviable 101, timeout 0.

### follow.rs: FocusPath::of

- follow.rs:149: replace body with Default::default()
- follow.rs:157: replace != with ==

### follow.rs: FocusPath::is_cut_at

- follow.rs:167: replace body with false
- follow.rs:167: replace body with true
- follow.rs:168: replace && with ||
- follow.rs:168: replace == with !=
- follow.rs:169: replace && with ||

### follow.rs: FocusPath::route_to

- follow.rs:178: replace == with !=
- follow.rs:178: replace body with None
- follow.rs:183: replace + with *
- follow.rs:183: replace + with -

### follow.rs: FollowState::retarget

- follow.rs:485: replace == with !=
- follow.rs:501: replace += with *=
- follow.rs:501: replace += with -=

### follow.rs: FollowState::follows

- follow.rs:513: replace == with !=

### follow.rs: FollowState::release_groups

- follow.rs:556: replace += with *=
- follow.rs:556: replace += with -=

### follow.rs: apply

- follow.rs:648: replace == with !=
- follow.rs:661: replace && with ||
- follow.rs:661: replace == with !=
- follow.rs:661: replace == with !=

### follow.rs: apply_watch

- follow.rs:696: replace && with ||

### follow.rs: answer_to_judge

- follow.rs:732: replace == with !=
- follow.rs:734: delete field event from struct held::Focus expression
- follow.rs:735: delete field simple_child from struct held::Focus expression

### gap_record.rs: GapRecord::foreground

- gap_record.rs:345: replace && with ||
- gap_record.rs:345: replace match guard open.top_level == window && open.awaits == Awaits::Focus with true

### hold.rs: Hold::held

- hold.rs:91: replace body with ::std::iter::empty()

### hold.rs: forwarded_time

- hold.rs:172: replace match guard forwarded(fact) with true

### hold.rs: forwarded

- hold.rs:179: replace body with true

### intake.rs: (no function named)

- intake.rs:176: delete -

### intake.rs: Marks::drops_on_arrival

- intake.rs:392: delete match arm Item::Msaa{kind:WinKind::Caret, ..}

### intake.rs: Intake::push

- intake.rs:521: replace < with <=
- intake.rs:521: replace < with ==
- intake.rs:521: replace < with >
- intake.rs:523: replace -= with +=
- intake.rs:523: replace -= with /=

### intake.rs: Intake::next

- intake.rs:600: replace > with >=
- intake.rs:605: replace += with *=

### intake.rs: Intake::cancel

- intake.rs:649: delete !

### intake.rs: Intake::batch_number

- intake.rs:711: replace body with 0
- intake.rs:711: replace body with 1

### intake.rs: Intake::waiting_carets

- intake.rs:718: replace body with vec![]
- intake.rs:722: replace / with %
- intake.rs:722: replace / with *
- intake.rs:722: replace >= with <

### intake.rs: old_focus_events_first

- intake.rs:786: replace + with *

### intake.rs: remove_waiting

- intake.rs:873: delete !
- intake.rs:875: replace + with *
- intake.rs:875: replace + with -

### intake.rs: caret_after_update

- intake.rs:975: replace && with ||
- intake.rs:983: replace + with *
- intake.rs:988: replace && with ||

### intake.rs: admit

- intake.rs:1128: replace += with *=

### lib.rs: Reducer::input

- lib.rs:728: delete !

### lib.rs: Reducer::tick

- lib.rs:748: replace && with ||

### lib.rs: Reducer::gap_bound_tested

- lib.rs:834: replace && with ||
- lib.rs:834: replace match guard reads && self.intake.gap().is_open() with true

### lib.rs: Reducer::send

- lib.rs:945: replace == with !=

### lib.rs: Reducer::answer_watch

- lib.rs:1080: delete field awaited_at_us from struct EventTiming expression
- lib.rs:1081: delete field awaited_calls from struct EventTiming expression
- lib.rs:1082: delete field calls from struct EventTiming expression
- lib.rs:1083: delete field published_at_us from struct EventTiming expression

### lib.rs: Reducer::released

- lib.rs:1139: delete !

### lib.rs: Reducer::finished

- lib.rs:1276: replace && with ||
- lib.rs:1300: replace || with &&

### lib.rs: Reducer::pump

- lib.rs:1430: replace += with *=

### lib.rs: Reducer::intake_batch

- lib.rs:1468: replace body with 0
- lib.rs:1468: replace body with 1

### lib.rs: Reducer::run

- lib.rs:1484: delete field item from struct Entry expression
- lib.rs:1538: replace match guard now.wall_us >= walk_until_us with false
- lib.rs:1545: replace match guard now.wall_us >= walk_until_us with true

### lib.rs: Reducer::live

- lib.rs:1560: replace body with vec![]

### lib.rs: Reducer::dispatch_finish

- lib.rs:1665: replace == with !=

### lib.rs: Reducer::waiting

- lib.rs:1681: replace body with vec![]
- lib.rs:1682: delete match arm Work::Entry{entry:Entry{item:Item::Query{query:Query::Text{op:TextOp::AwaitCaret(watch), ..}, ..}, ..}, ..}

### lib.rs: waiting_caret

- lib.rs:1733: replace body with None
- lib.rs:1734: delete match arm Item::Uia(UiaEvent{kind:UiaKind::TextSelection, parts, ..})
- lib.rs:1739: delete match arm Item::Msaa{kind:WinKind::ConsoleCaret, hwnd, ..}
- lib.rs:1744: delete match arm Item::Msaa{kind:WinKind::Caret | WinKind::TextSelectionChange, hwnd, ..}

### lib.rs: clears_absorb

- lib.rs:1772: replace body with true

### lib.rs: owed_clears_absorb

- lib.rs:1790: replace body with false
- lib.rs:1790: replace body with true

### lib.rs: budget

- lib.rs:1806: delete match arm Query::FocusNow
- lib.rs:1807: delete match arm Query::Ancestors{..} | Query::DumpTree{..}
- lib.rs:1812: delete match arm Query::Text{..}

### lib.rs: watch_ended

- lib.rs:1850: delete field calls from struct EventTiming expression
- lib.rs:1851: delete field published_at_us from struct EventTiming expression

### lib.rs: gap_report

- lib.rs:1873: delete match arm OutpostToCore::Reply{outcome:QueryOutcome::Done(QueryResult::Focus(now)), ..}

### verdicts.rs: Probed::lasts

- verdicts.rs:42: replace body with false
- verdicts.rs:42: replace body with true

### verdicts.rs: Kept::judges

- verdicts.rs:98: replace < with >
- verdicts.rs:102: replace match guard at_us >= until with false

### verdicts.rs: Verdicts::lasts

- verdicts.rs:146: replace body with false
- verdicts.rs:148: replace && with ||
- verdicts.rs:148: replace == with !=

### verdicts.rs: Verdicts::apply

- verdicts.rs:171: replace == with !=
- verdicts.rs:171: replace match guard window.current.owner == kept.owner with false
- verdicts.rs:171: replace match guard window.current.owner == kept.owner with true
- verdicts.rs:172: replace >= with <

### watch.rs: Watch::take

- watch.rs:237: replace match guard open.node_id == node with true

## verbatim-listener-logic

Caught 105, missed 22, unviable 9, timeout 0.

### schedule.rs: Schedule<J>::stopping

- schedule.rs:422: replace body with false
- schedule.rs:422: replace body with true

### schedule.rs: Schedule<J>::focus_pending

- schedule.rs:465: replace == with !=

### schedule.rs: Schedule<J>::push

- schedule.rs:618: replace && with ||
- schedule.rs:633: replace != with ==
- schedule.rs:658: replace + with *

### schedule.rs: Schedule<J>::next

- schedule.rs:734: replace == with !=

### schedule.rs: Schedule<J>::returned

- schedule.rs:753: replace && with ||
- schedule.rs:775: replace < with <=
- schedule.rs:775: replace < with ==
- schedule.rs:775: replace < with >

### schedule.rs: Schedule<J>::abandon

- schedule.rs:883: replace && with ||

### schedule.rs: Schedule<J>::published

- schedule.rs:941: replace && with ||

### schedule.rs: Schedule<J>::drop_waiting

- schedule.rs:972: replace || with &&

### schedule.rs: Schedule<J>::foreground

- schedule.rs:1011: replace == with !=

### schedule.rs: Schedule<J>::outpost_ended

- schedule.rs:1023: replace body with ()
- schedule.rs:1026: delete !
- schedule.rs:1026: replace && with ||
- schedule.rs:1026: replace && with ||
- schedule.rs:1026: replace == with !=

### schedule.rs: Schedule<J>::spawn_failed

- schedule.rs:1035: replace body with ()
- schedule.rs:1038: delete !

## verbatim-core

Caught 76, missed 24, unviable 26, timeout 0.

### queries.rs: Queries::find_mut

- queries.rs:230: replace && with ||

### queries.rs: Queries::next

- queries.rs:302: replace body with 0
- queries.rs:302: replace body with 1

### queries.rs: Queries::set_traces

- queries.rs:310: replace == with !=
- queries.rs:310: replace body with ()

### queries.rs: Queries::set_trace

- queries.rs:319: replace body with ()
- queries.rs:320: delete match arm Effect::ForKey(key, inner)
- queries.rs:326: replace >= with <

### queries.rs: outpost_gone

- queries.rs:435: replace && with ||

### reduce.rs: observed_before_foreground_change

- reduce.rs:519: replace < with <=

### reduce.rs: leaves_focus

- reduce.rs:604: replace < with <=

### reduce.rs: outpost_ended

- reduce.rs:1418: replace == with !=
- reduce.rs:1425: replace == with !=
- reduce.rs:1430: delete match arm Then::Caret(PendingCaret{node, ..}) | Then::Text(PendingText{node, ..})
- reduce.rs:1431: replace == with !=
- reduce.rs:1438: replace == with !=

### terminal_model.rs: answered_read

- terminal_model.rs:905: replace body with None
- terminal_model.rs:905: replace body with Some((Default::default(), None, vec![]))
- terminal_model.rs:912: delete match arm TextReply::TerminalSnapshot(snapshot)
- terminal_model.rs:916: delete match arm TextReply::TerminalUnchanged

### terminal_model.rs: reply

- terminal_model.rs:973: delete !
- terminal_model.rs:974: delete !
- terminal_model.rs:974: replace && with ||

### terminal_model.rs: answerable

- terminal_model.rs:1098: delete match arm Effect::ForKey(_, effect)

## Classes

Each missed mutant classed on 2026-11-02 against `docs/design/logic-tests.md`, "The judgement's thresholds" and "The owners": equivalent (cannot change any effect or later behaviour), excluded (in what the owner's model leaves out by design), or target (a gap the generated tests should kill). "NL" marks a target no check of the design's listed at the time; the builders add the checks named below.

Two exclusions beyond the design's own list: `FocusPath` in `follow.rs` (11), worker code with no caller in the reducer, tested from `verbatim-outpost`; and the watch answer's timing in `lib.rs` (6), measurement contents no decision reads.

Counts, after the role crates' tests were run on the missed mutants (below, "Killed by the role crates' tests"):

- verbatim-reader-rules (4): equivalent 0, excluded 0, target 4.
- verbatim-outpost-logic (101): equivalent 3, excluded 61, target 37.
- verbatim-listener-logic (22): equivalent 2, excluded 0, target 9, and 11 killed by `verbatim-listener`'s tests.
- verbatim-core (24): equivalent 1, excluded 2, target 21.
- All (151): equivalent 6, excluded 63, killed by the role crates' tests 11, target 71. The threshold, 30% of the targets, is 22 killed, and at least one in each crate.

The outpost's 10 mutants killed by `verbatim-outpost`'s tests were all excluded, so its counts stand. The role crates' runs cover their unit and integration tests, not `crates/mockapp/tests`, whose tests exercise the subscription and the verdicts against a real application and are too slow to run once per mutant; an excluded mutant left here may still be caught there.

Checks added for the targets no listed check covered: the work in flight's `running()`, `deadline()` and `item_mut()` against the model after every step; the hold's release on forwarding ended and reads ended; client-area events generated with the literal value -4; in the intake's test, batch numbers, a keyed entry's replacement, and the limiter's counts; in the listener's, `stopping()`, `focus_pending`, each job's `after_elsewhere` and the applications held after an outpost's end; in Core's, an open entry's trace unchanged after the step that made it.

## Classes of the missed mutants

Source: `experiments/logic-tests-study/judgement.md`, code at be5bd5b9 (`git diff be5bd5b9 -- crates/` is empty, so main's lines are the same). Classes against `docs/design/logic-tests.md`, "The judgement's thresholds" and "The owners". "NL" means target with no listed check ("none listed").

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


## Killed by the role crates' tests

Date: 2026-11-03. Same commit and tool as above. Each crate's missed mutants were run again with the role crate's tests added, which also drive the real reducer: `--test-package` names both the logic crate and its role crate. The mutants were chosen with one `--re` alternation per crate that matches exactly the file, line and mutation listed under that crate above (a few extra mutants on neighbouring lines also ran and are ignored here). The unmutated baseline passed for both runs, with mockapp built first.

- `cargo mutants --in-place -p verbatim-outpost-logic --test-package verbatim-outpost-logic --test-package verbatim-outpost --re <alternation>`: 112 mutants, 41 minutes.
- `cargo mutants --in-place -p verbatim-listener-logic --test-package verbatim-listener-logic --test-package verbatim-listener --re <alternation>`: 24 mutants, 3 minutes 20 seconds.

Killed, 10 of the 101 missed outpost-logic mutants, by `verbatim-outpost`'s tests, all excluded:

- follow.rs:149, body Default (excluded).
- follow.rs:178, == to != (excluded).
- follow.rs:178, body None (excluded).
- follow.rs:183, + to * (excluded).
- follow.rs:183, + to - (excluded).
- verdicts.rs:98, < to > (excluded).
- verdicts.rs:102, guard to false (excluded).
- verdicts.rs:171, == to != (excluded).
- verdicts.rs:171, guard to false (excluded).
- verdicts.rs:172, >= to < (excluded).

Killed, 11 of the 22 missed listener-logic mutants, by `verbatim-listener`'s tests, all target (the first was killed by timeout, the rest by a failing test):

- schedule.rs:422, stopping to false (target, NL; timeout).
- schedule.rs:422, stopping to true (target, NL).
- schedule.rs:465, == to != (target, NL).
- schedule.rs:618, && to || (target).
- schedule.rs:658, + to * (target).
- schedule.rs:1023, outpost_ended body () (target, NL).
- schedule.rs:1026, delete ! (target).
- schedule.rs:1026, && to || first (target).
- schedule.rs:1026, && to || second (target).
- schedule.rs:1035, spawn_failed body () (target).
- schedule.rs:1038, delete ! (target).

## Second run, at 5cfa65e9

- Date: 2026-11-03. Commit: 5cfa65e9, detached, in a worktree. Tool, settings and the six runs are the baseline's, with `-o` naming each run's output directory outside the repository. No source file was changed.
- Command lines: the baseline's six, listed at the top of this document, each with the crate's own tests only. Then, for the missed mutants of verbatim-outpost-logic and verbatim-listener-logic only, the role crates' tests were added as in "Killed by the role crates' tests", with `cargo build -p mockapp` first and `--re` an alternation of the exact names of the missed mutants of the first run, each as `^name$`:
  - `cargo mutants --in-place -p verbatim-outpost-logic --test-package verbatim-outpost-logic --test-package verbatim-outpost --re <alternation>`
  - `cargo mutants --in-place -p verbatim-listener-logic --test-package verbatim-listener-logic --test-package verbatim-listener --re <alternation>`
- Run time: the six runs took 84 minutes (reader-rules 2 min 38 s, outpost-logic 54 min 15 s, listener-logic 6 min 4 s, core 9 min 42 s + 5 min 50 s + 5 min 25 s), first build included. The two role reruns took 25 min 14 s and 40 s. The outpost-logic run took 54 minutes against the baseline's 31, with 619 mutants against 514.

### Counts

Own tests only (baseline in brackets):

- verbatim-reader-rules: caught 46 [42], missed 0 [4], unviable 53 [53], timeout 0 [0].
- verbatim-outpost-logic: caught 450 [413], missed 66 [101], unviable 103 [101], timeout 0 [0].
- verbatim-listener-logic: caught 126 [105], missed 3 [22], unviable 9 [9], timeout 0 [0].
- verbatim-core: caught 86 [76], missed 14 [24], unviable 26 [26], timeout 0 [0], summed over the three runs (queries.rs 50, 2, 15; reduce.rs 27, 4, 4; terminal_model.rs 9, 8, 7).

With the role crates' tests added to the missed ones:

- verbatim-outpost-logic: 79 mutants ran (the 66 and 13 on neighbouring lines, which the name match also took). 8 of the 66 were killed by `verbatim-outpost`'s tests, all of them in the baseline's classes "excluded" (`FocusPath::of` with `Default::default()`, `route_to` with `None`, `route_to` `==` to `!=`, both `+` mutations on line 183, `Kept::judges` `<` to `>`, its guard `at_us >= until` to false, and `Verdicts::apply` `>=` to `<`). 58 stay missed. None of the target mutants is among the 8.
- verbatim-listener-logic: 5 mutants ran. The 3 missed (`returned` `<` to `<=`, `published` `&&` to `||`, `drop_waiting` `||` to `&&`) stay missed under `verbatim-listener`'s tests.

One mutant differs between runs: follow.rs:735 (`answer_to_judge`, delete field `simple_child`), baseline class excluded, was unviable in the outpost-logic run and missed in the rerun, whose build includes the other test crates.

### Mutants that did not exist at the baseline

None missed. Every missed mutant of the six runs was matched by file, function and mutation text to a mutant missed at the baseline, using the line mapping of `git diff -U0 be5bd5b9 5cfa65e9` for the files that changed. The 35 outpost-logic and 19 listener-logic mutants missed at the baseline and not now are therefore the difference.

### Targets

Each of the 71 target mutants (the 82 target-class mutants less the 11 killed by `verbatim-listener`'s tests at the baseline, as the baseline counts), by its baseline file and line. "Caught by" names the tests that failed under the mutant; "now line" is given where the code moved. The baseline's mutations are matched by file, function and mutation text.

Two target mutants no longer exist as written: schedule.rs:734 (`Schedule::next`) and schedule.rs:1026 (`Schedule::outpost_ended`). Both conditions on `abandoned == 0` moved into the new `App::spent` (schedule.rs:372), whose `==` to `!=` mutant is caught by `schedule::generated::fixed_cases_keep_the_schedules_rules`, `schedule::generated::generated_cases_keep_the_schedules_rules` and two hand-written regression tests, and which has no missed mutant.

### verbatim-reader-rules


in_flight.rs: InFlight<T>::deadline

- in_flight.rs:425, replace body with None: caught by `in_flight::tests::generated_cases_match_the_reference_model`
- in_flight.rs:425, replace body with Some(0): caught by `in_flight::tests::generated_cases_match_the_reference_model`
- in_flight.rs:425, replace body with Some(1): caught by `in_flight::tests::generated_cases_match_the_reference_model`

in_flight.rs: InFlight<T>::item_mut

- in_flight.rs:430, replace body with None: caught by `in_flight::tests::generated_cases_match_the_reference_model`

### verbatim-outpost-logic


follow.rs: apply

- follow.rs:661, replace && with ||: caught by `tests::reducer::generated_cases_keep_the_reducers_rules`
- follow.rs:661, replace == with !=: caught by `tests::reducer::generated_cases_keep_the_reducers_rules`
- follow.rs:661, replace == with !=: caught by `tests::reducer::generated_cases_keep_the_reducers_rules`

follow.rs: apply_watch

- follow.rs:696, replace && with ||: still missed (now line 696)

gap_record.rs: GapRecord::foreground

- gap_record.rs:345, replace && with ||: caught (now line 352) by `tests::reducer::generated_cases_keep_the_reducers_rules`
- gap_record.rs:345, replace match guard open.top_level == window && open.awaits == Awaits::Focus with true: caught (now line 352) by `tests::reducer::generated_cases_keep_the_reducers_rules`

hold.rs: forwarded_time

- hold.rs:172, replace match guard forwarded(fact) with true: still missed (now line 172)

hold.rs: forwarded

- hold.rs:179, replace body with true: still missed (now line 179)

intake.rs: (no function named)

- intake.rs:176, delete -: caught by `tests::intake_order::generated_cases_keep_the_intakes_order`

intake.rs: Intake::push

- intake.rs:521, replace < with <=: still missed (now line 521)
- intake.rs:521, replace < with ==: caught by `tests::intake_order::generated_cases_keep_the_intakes_order`
- intake.rs:521, replace < with >: caught by `tests::intake_order::generated_cases_keep_the_intakes_order`
- intake.rs:523, replace -= with +=: caught by `tests::intake_order::generated_cases_keep_the_intakes_order`, `tests::reducer::generated_cases_keep_the_reducers_rules`
- intake.rs:523, replace -= with /=: caught by `tests::intake_order::generated_cases_keep_the_intakes_order`

intake.rs: Intake::next

- intake.rs:600, replace > with >=: caught by `tests::intake_order::generated_cases_keep_the_intakes_order`, `tests::reducer::generated_cases_keep_the_reducers_rules`
- intake.rs:605, replace += with *=: caught by `tests::intake_order::generated_cases_keep_the_intakes_order`

intake.rs: Intake::cancel

- intake.rs:649, delete !: caught by `tests::intake_order::generated_cases_keep_the_intakes_order`, `tests::reducer::generated_cases_keep_the_reducers_rules`

intake.rs: Intake::batch_number

- intake.rs:711, replace body with 0: caught (now line 729) by `tests::intake_order::generated_cases_keep_the_intakes_order`
- intake.rs:711, replace body with 1: caught (now line 729) by `tests::intake_order::generated_cases_keep_the_intakes_order`

intake.rs: remove_waiting

- intake.rs:873, delete !: caught (now line 891) by `tests::intake_order::generated_cases_keep_the_intakes_order`
- intake.rs:875, replace + with *: caught (now line 893) by `tests::intake_order::generated_cases_keep_the_intakes_order`
- intake.rs:875, replace + with -: caught (now line 893) by `tests::intake_order::generated_cases_keep_the_intakes_order`

intake.rs: caret_after_update

- intake.rs:975, replace && with ||: still missed (now line 993)
- intake.rs:988, replace && with ||: still missed (now line 1006)

intake.rs: admit

- intake.rs:1128, replace += with *=: caught (now line 1146) by `tests::intake_order::generated_cases_keep_the_intakes_order`

lib.rs: Reducer::input

- lib.rs:728, delete !: caught by `tests::a_gap_record_opened_after_the_close_is_never_tested`

lib.rs: Reducer::tick

- lib.rs:748, replace && with ||: caught by `tests::reducer::fixed_cases_keep_the_reducers_rules`, `tests::reducer::generated_cases_keep_the_reducers_rules`

lib.rs: Reducer::gap_bound_tested

- lib.rs:834, replace && with ||: caught (now line 852) by `tests::reducer::generated_cases_keep_the_reducers_rules`
- lib.rs:834, replace match guard reads && self.intake.gap().is_open() with true: caught (now line 852) by `tests::reducer::generated_cases_keep_the_reducers_rules`

lib.rs: Reducer::send

- lib.rs:945, replace == with !=: caught (now line 963) by `tests::reducer::generated_cases_keep_the_reducers_rules`

lib.rs: Reducer::finished

- lib.rs:1276, replace && with ||: caught (now line 1294) by `tests::reducer::generated_cases_keep_the_reducers_rules`
- lib.rs:1300, replace || with &&: caught (now line 1318) by `tests::reducer::generated_cases_keep_the_reducers_rules`

lib.rs: Reducer::pump

- lib.rs:1430, replace += with *=: caught (now line 1448) by `tests::reducer::generated_cases_keep_the_reducers_rules`

lib.rs: Reducer::run

- lib.rs:1538, replace match guard now.wall_us >= walk_until_us with false: caught (now line 1556) by `tests::reducer::generated_cases_keep_the_reducers_rules`
- lib.rs:1545, replace match guard now.wall_us >= walk_until_us with true: still missed (now line 1563)

lib.rs: gap_report

- lib.rs:1873, delete match arm OutpostToCore::Reply{outcome:QueryOutcome::Done(QueryResult::Focus(now)), ..}: still missed (now line 1891)

watch.rs: Watch::take

- watch.rs:237, replace match guard open.node_id == node with true: caught by `tests::reducer::fixed_cases_keep_the_reducers_rules`

### verbatim-listener-logic


schedule.rs: Schedule<J>::push

- schedule.rs:633, replace != with ==: caught (now line 649) by `schedule::generated::fixed_cases_keep_the_schedules_rules`, `schedule::generated::generated_cases_keep_the_schedules_rules`

schedule.rs: Schedule<J>::next

- schedule.rs:734, replace == with !=: no longer exists

schedule.rs: Schedule<J>::returned

- schedule.rs:753, replace && with ||: caught (now line 769) by `schedule::generated::generated_cases_keep_the_schedules_rules`
- schedule.rs:775, replace < with ==: caught (now line 791) by `schedule::generated::generated_cases_keep_the_schedules_rules`
- schedule.rs:775, replace < with >: caught (now line 791) by `schedule::generated::generated_cases_keep_the_schedules_rules`

schedule.rs: Schedule<J>::abandon

- schedule.rs:883, replace && with ||: caught (now line 903) by `schedule::generated::fixed_cases_keep_the_schedules_rules`, `schedule::generated::generated_cases_keep_the_schedules_rules`

schedule.rs: Schedule<J>::published

- schedule.rs:941, replace && with ||: still missed (now line 961)

schedule.rs: Schedule<J>::foreground

- schedule.rs:1011, replace == with !=: caught (now line 1031) by `schedule::generated::generated_cases_keep_the_schedules_rules`

schedule.rs: Schedule<J>::outpost_ended

- schedule.rs:1026, replace == with !=: no longer exists

### verbatim-core


queries.rs: Queries::next

- queries.rs:302, replace body with 0: still missed (now line 302)
- queries.rs:302, replace body with 1: caught by `queries::generated::fixed_cases_keep_the_reducers_rules`, `queries::generated::generated_cases_keep_the_reducers_rules`

queries.rs: Queries::set_traces

- queries.rs:310, replace == with !=: caught by `queries::generated::fixed_cases_keep_the_reducers_rules`, `queries::generated::generated_cases_keep_the_reducers_rules`
- queries.rs:310, replace body with (): caught by `queries::generated::fixed_cases_keep_the_reducers_rules`, `queries::generated::generated_cases_keep_the_reducers_rules`

queries.rs: Queries::set_trace

- queries.rs:319, replace body with (): caught by `queries::generated::fixed_cases_keep_the_reducers_rules`, `queries::generated::generated_cases_keep_the_reducers_rules`
- queries.rs:320, delete match arm Effect::ForKey(key, inner): caught by `queries::generated::generated_cases_keep_the_reducers_rules`
- queries.rs:326, replace >= with <: caught by `queries::generated::fixed_cases_keep_the_reducers_rules`, `queries::generated::generated_cases_keep_the_reducers_rules`

queries.rs: outpost_gone

- queries.rs:435, replace && with ||: caught by `queries::generated::generated_cases_keep_the_reducers_rules`

reduce.rs: leaves_focus

- reduce.rs:604, replace < with <=: caught by `queries::generated::fixed_cases_keep_the_reducers_rules`, `queries::generated::generated_cases_keep_the_reducers_rules`

reduce.rs: outpost_ended

- reduce.rs:1418, replace == with !=: caught by `queries::generated::generated_cases_keep_the_reducers_rules`
- reduce.rs:1425, replace == with !=: caught by `queries::generated::generated_cases_keep_the_reducers_rules`
- reduce.rs:1430, delete match arm Then::Caret(PendingCaret{node, ..}) | Then::Text(PendingText{node, ..}): still missed (now line 1430)
- reduce.rs:1431, replace == with !=: still missed (now line 1431)
- reduce.rs:1438, replace == with !=: still missed (now line 1438)

terminal_model.rs: answered_read

- terminal_model.rs:905, replace body with None: still missed (now line 905)
- terminal_model.rs:905, replace body with Some((Default::default(), None, vec![])): still missed (now line 905)
- terminal_model.rs:912, delete match arm TextReply::TerminalSnapshot(snapshot): still missed (now line 912)

terminal_model.rs: reply

- terminal_model.rs:973, delete !: still missed (now line 973)
- terminal_model.rs:974, delete !: still missed (now line 974)
- terminal_model.rs:974, replace && with ||: still missed (now line 974)

terminal_model.rs: answerable

- terminal_model.rs:1098, delete match arm Effect::ForKey(_, effect): still missed (now line 1098)

### Targets caught by the generated tests

Caught now, missed at the baseline, and not killed by the role crates' tests (the missed ones that the role crates' tests were added to stayed missed, so no target is killed by them), out of 71:

- verbatim-reader-rules: 4 of 4.
- verbatim-outpost-logic: 29 of 37 (8 still missed).
- verbatim-listener-logic: 6 of 9 (1 still missed, 2 no longer exist as written).
- verbatim-core: 10 of 21 (11 still missed).
- All: 49 of 71.

All but one of the 49 fail under the generated tests (`generated_cases_...` or `fixed_cases_...`, the fixed cases of the same files). The exception is lib.rs:728, `Reducer::input` delete `!`, which only `tests::a_gap_record_opened_after_the_close_is_never_tested` catches, a hand-written regression test that the outpost's generated test found (it is not in the baseline's tests.rs). Without it the count is 48.

## The first measure

Met (2026-11-03): the generated tests kill 49 of the 71 targets, 69%, against a threshold of 22, and at least one in each crate (reader-rules 4 of 4, outpost-logic 29 of 37, listener-logic 6 of 9, core 10 of 21). One of the 49, `lib.rs:728`, is killed only by a replay test the outpost's generated test found; without it, 48. The second measure, ordering defects found first and upkeep, is recorded below for each of the next three builds that change a covered owner.

## Build record

M4.6's own build, for reference; the second measure counts from the next build. Production defects found while it was built, each fixed with its shrunk case kept as a replay test and a fixed case:

- `574072e1`: a closed outpost still tested the gap record's bound, so a title could reach Core. Found by the outpost's generated test in the gate.
- `4d2c7221`: a verdict that left the record unchanged was reported as a change, so the record was sent again and the hook narrowed again. Found by exploration.
- `e7362127`: a gap record closed by its own tick's publication still had its title committed. Found by exploration.
- `6c9a1a93`: a push that replaced the event an idle waiter waited on ran no waiters. Found by reading while the design was surveyed; the generated test fails without the fix.
- `e282e7ea`: the listener kept an ended application for good once its abandoned read returned, one per such process. Found by the scheduler's model.
- `d12f5da1`: a reading thread's failed start was stepped before the other effects of its step, so an idle answer went ahead of another application's drop. Found by exploration.
- Not a defect: a gap title ended a caret watch the program in flight held, against the watch's stated rule. Found by exploration; the rule was amended rather than the code (Claude, 2026-11-03, accepted by Dickson the same day), after a deferral that added a lifecycle state was reverted.

Time, by the agents' run times: building the tests 5.1 hours (the outpost's part 3.0, the listener's and Core's 1.5, the review's fixes 0.6); the mutation runs 3.7 hours of machine time; the design's surveys and reviews about 0.6 hours.

