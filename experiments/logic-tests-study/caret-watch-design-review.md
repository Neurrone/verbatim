# Adversarial review: the caret watch design

Reviewed: "The caret watch (a build of its own)" in `docs/design/logic-foundations.md` (working copy), against the code at 691ef87e. Read-only; nothing was built or run.

Important findings: 1. Nits: 10. Pre-existing: 1.

## Does the build reach its goal

Yes, with one gap in how it is guarded. With the watch as one state in the reducer, nothing but the reducer writes its times, and a `Checked` is taken only from the program in flight while the watch is Checking. Under that model the stale-copy defects (the lost tick, the wake left after a take, the abandoned read matching by node, the spin) cannot be written. Every transition that leaves a state answers once, and every state reaches None. I tried each sequence the brief lists against the code (below) and found none that gives a query no answer or two. I found no sequence where what is heard changes.

The gap: the design rests one rule on a test that cannot check it. The rule is that the program's copy equals the reducer's watch at every commit the program makes. The test is the generated test, and it drives the watch module alone. That is the Important finding.

## Important

### The generated test covers the watch module alone, so it cannot check the rules the past defects broke

Where: the design's "Tests" (the generated test) and its "Limitations" (second item). The integration points it leaves out are in `crates/verbatim-outpost-logic/src/lib.rs`: `next_timer` 538 to 558, `accepts` 526, `judge_abandonment` 1000, `finished` 1079, `answer_idle` 1159, `dispatch` 1423 and `publish` 972.

The defect: the generated test runs sequences of the watch module's own inputs against a reference model with the same four states. Most of the rules the past defects broke, and the rule the limitation hands to this test, live in the reducer around the module, so this test never runs them.

Which rules live where:

- **The spin (17767cbf).** The watch's due time is timed only while no program is in flight. That condition is in `next_timer`.
- **The abandoned read touching a newer watch (463f2ef9).** A late result is refused because `accepts` refuses its request and `publish` claims it. Neither is in the module.
- **The poll read the idle answer did not wait for (b5d2588d).** The idle answer waits for `Current::deferred`.
- **The limitation's rule.** The reducer changes the watch outside the program's own commits only at a dispatch before the copy is taken, or with no program in flight. Whether a program is in flight, and when the copy is taken, are facts `dispatch` and `Current` hold. The module never sees them.
- **The reference model.** A four-state model of a four-state machine is not the much simpler model `CLAUDE.md` asks for. It repeats the production logic, which is what `REVIEW.md` names as a test failure.

Why it matters: when the copy and the reducer disagree, a key's speech changes.

- If the reducer takes the watch into Checking while the program's copy finds none, the program returns without `Checked`. Its end answers `WatchEnded`, so the key's evidence, arriving later, is never spoken.
- If the copy takes the watch while the reducer does not, the program checks and commits `Checked`, and the reducer drops it. Evidence the program read is lost, which is the 6b0bf0d4 kind of defect.

By inspection the two agree today. A publication changes the watch while its program is in flight (finding "The no-change rule leaves out the publication" below), but it claims that program, so the program's later commits are refused. A later change, such as a report or a release that ends the watch while a program is in flight, would break the rule silently. Under this design nothing mechanical would notice.

Counterexample: a later edit makes `Input::Released` end a watch whose node was released (the pre-existing follow-up below). The sequence:

1. A text-change program P is dispatched with W1 Waiting in its copy.
2. A release of W1's node is applied while P is in flight. The reducer moves W1 to None and answers it `WatchEnded`.
3. P commits `Take(node)`. Its copy takes W1, so P checks the application and finds the evidence.
4. P commits `Checked::Answered`. The reducer finds no watch in Checking and drops it.

Every module-level sequence still passes, because the module is never told that P is in flight.

Fix: make the generated test drive `Reducer::input` and `Reducer::tick`. Its inputs are a caret key's entry, the program's commits (`Open`, `Take`, both kinds of `Checked`, a report ending the watch), `Finished` (normal, panicked, with a cut), a deadline tick, a walk's publication, `Released` and `Close`. After every step it checks four invariants:

- every `AwaitCaret` request id has at most one `Reply` in the `Send` effects, and exactly one once the sequence closes;
- `next_timer` gives no time for the watch while `busy()` or releasing;
- the watch in the last dispatched `Program::facts`, with that program's own commits applied, equals the reducer's watch at each accepted commit;
- no idle answer is sent while the watch is not None or the program in flight is deferred.

The reference model can then be simple: "the open query, and whether a program holds it". The module-level enumeration can stay as a unit test.

## Nits

- **The last poll read moves past `until`.** Design, "Transitions", `Checked`'s `Watching` after a poll read, gives `next` as "5 ms after now". `Open` and today's `Poll::after_read` (`crates/verbatim-outpost-logic/src/follow.rs` 194 to 199) take the earlier of that and `until`. As written, a read at 47 ms sets the next for 52 ms rather than 50, so the last read can land up to 5 ms after the documented bound (`docs/design/key-holding.md`, "The console host's caret poll"). State the minimum.
- **The program's copy after a poll read's `Watching` is unspecified.** The copy cannot compute `next` without the reducer's clock. Say that `apply_local` leaves the copy in Checking, or sets it to None. Today no poll read does anything after that commit, so nothing observes the difference.
- **The step-two argument quotes the wrong deadline.** An event's program runs under `HANDLING_DEADLINE`, 10 s, not the 2-second text deadline (`budget`, `lib.rs` 1598 to 1605), or until the moved-on rule abandons it. The argument holds with the right figure.
- **The first limitation's "at most 2 s" is wrong.** A before read that is still queued waits behind the program in flight, which can run up to 10 s (handling) or 5 s (`WALK_DEADLINE`, `crates/verbatim-model/src/protocol/mod.rs` 947), plus its own 2 s. Core's idle round queues behind the same program in the outpost, so the harness waits about as long today. Correct the figure rather than the mechanism.
- **The shell still judges a request by its kind.** The held key's one bound says it no longer does, but the `AwaitCaret` branch to `begin_passive` stays (`crates/verbatim-app/src/main.rs` 1828). Say "no longer judges the before read by its kind".
- **The no-change rule leaves out the publication.** The rule that the reducer changes the watch outside the program's own commits only at a dispatch or with no program in flight has one exception. `publish` (`lib.rs` 972, from `tick` 658 and `judge_abandonment` 1029) applies a report that can end a Waiting or Polling watch while its program is in flight. This is harmless because the program is claimed and its commits refused. State the exception.
- **The due time's place relative to `when_idle` is unstated.** Today `Item::Wake` comes after the idle entry, the noted caret (`crates/verbatim-outpost-logic/src/intake.rs` 664 to 673). "Nothing ready in the intake" should say the due time is taken only once `Intake::next` returns `None`, so a noted caret still goes first.
- **Frozen and passed test clocks.** The due times are on the observation clock, which mockapp's frozen clock does not advance, and which `pass_time` moves without waking the shell. The gap record has a test timer for exactly this (`Input::GapTimerGiven`; `crates/mockapp/tests/common/outpost.rs` 386 onwards). On a frozen clock a watch's bound and poll would never be reached, and the shell would time wakes it does not act on. Today `Poll::new` compares `pressed_at_ms` with the shared clock, which `pass_time` does not move. No frozen-clock mockapp test opens a watch today (checked: `call_counts.rs` and `remote_ops.rs` call the check directly). Say so, or give the watch the gap record's test timer.
- **The panicked or claimed `WatchEnded` now reaches the latency ledger.** It carries the watch's timing, whose `dequeued_at_us` is not 0, so the app records it (`crates/verbatim-app/src/main.rs` 1340). The `Failed` it replaces carried the default timing and was not recorded. Nothing heard changes, but "What is heard" should say that measurement sees it.
- **The scenario list and the removal list.** `text_box_editing` does not reach the reducer's 10-second bound. Right Arrow follows Control+Home at once (`crates/verbatim-e2e/src/scenarios/editing.rs` 83 to 86), and `send_keys` waits for nothing, so the old watch ends at Right Arrow's dispatch. The scenario does exercise the key-ahead deadline. The bound's own end is covered only by unit tests, and the design should say so. Separately, "Removed" omits `Change::aside` and its branch (`follow.rs` 435, `lib.rs` 808 to 811). Only the watch uses them, and they go dead once the reducer builds the reply.

## Pre-existing (follow-up, not this build)

- **A watch on a released node stays open.** A Waiting watch whose node the store releases (`Input::Released`, `lib.rs` 620 to 635) stays open until its bound or the next key. Core forgets that node, so its answer is dropped. The cost is only the idle answer's wait, up to 10 s. Record in the roadmap if wanted; ending it on release would be a reducer change while a program may be in flight, which the Important finding's test must then cover.

## The nine past defects, under the new model

Every sequence was checked against the code. None recurs.

- **A console caret event stopping the poll (463f2ef9).** A take keeps `resume` as the poll stood (`next`, `until`), and `Watching` restores it. Only `Answered`, a due time, an abandonment, an end or a report leaves Polling. Does not recur.
- **The lost tick (463f2ef9).** The due time is read from the one state each time `next_timer` is called. No program writes a time, since the worker's five deciding clock reads go, and the copy is never read back. Does not recur.
- **An abandoned read matching by node (463f2ef9).** Its late `Checked` is refused by `accepts` (dead generation, `lib.rs` 526 and 591) and names nothing to match. Does not recur.
- **The evidence a read already saw (6b0bf0d4).** `Program::waiting` is still computed at dispatch (`lib.rs` 1490). For `PollRead` it must read the Checking state's watch, which the design says. Unchanged.
- **A held key waiting out the 10-second bound (d7ec6b04).** The sequence in the design was checked against `next_held` and `held_wait_ended` (`crates/verbatim-core/src/editing.rs` 272 to 435) and `reduce_text_completed` (`crates/verbatim-core/src/reduce.rs` 560). W1's `WatchEnded` at K2's dispatch goes to `superseded_reply` and is silent, and `awaiting_watch` is K2's by then. Correct.
- **The spinning reducer (17767cbf).** Nothing is timed while a program is in flight or a release is pending. When the program ends, `pump` runs in the same input and dispatches a ready entry or takes the due time. Does not recur.
- **A watch kept for a replaced node (60dc079c).** The report rule ends the watch in any state, and the program's copy applies the same rule. Does not recur.
- **The poll read the idle answer did not wait for (b5d2588d).** The poll read is dispatched deferred and stays deferred after `Answered`, as today. Does not recur.
- **The wake left after a take (b5d2588d).** Checking has no due time. Does not recur.

## Exactly one answer: the sequences tried

Each of these gives one answer.

- **Abandonment.** Before `Open` (the key's `running`). While Waiting (the watch is untouched). With the program's `Take` still in the mailbox when a foreground fact abandons it (the `Take` is refused and the watch stays Waiting, as today). While Checking (`Abandoned`).
- **A panic.** After `Watching` (the watch is back in Waiting, and the end answers nothing). After `Answered` (the watch is None).
- **A generation's death.**
- **A new key while a check is in flight.** A poll read or a take is in flight when K2 arrives. K2 waits, and its dispatch ends the watch only if the watch is still open.
- **A publication.** It ends a Checking watch by its report, and then the end finds None. Or it claims the program, and the end answers `WatchEnded`.
- **Close.** Waiting: the outpost's end answers `Gone`, as today. Checking: the end of the program in flight answers it.
- **A check committed after the bound.** It goes back with a past due time, which is taken at the program's end once nothing is ready. A Polling watch gets one more read and then `WatchEnded`, as today.
- **Two caret keys in one batch.** The second key's dispatch ends the first key's watch.
- **A key whose node Core forgot.** A report or a replaced node ends the watch. A release does not, which is the pre-existing follow-up above.
- **A cut.** The finish program inherits Checking and always answers, since `finish_watch` returns a reply.

## The wire differences and the tightening

- **The old watch's end at the next key's dispatch.** In Core, `superseded_reply` (`editing.rs` 855) or the replaced `pending_caret` makes it silent, and the key line resumes on any reply. Today, a key program abandoned before its `End` commit left the old watch open; now the dispatch ends it. Core drops that answer in both cases. Nothing heard changes.
- **`WatchEnded` in place of `Failed`.** `deliver` maps `Failed` to `TextReply::Unanswered` (`crates/verbatim-app/src/requests.rs` 411 to 419). Both reach `caret_reply`'s branch for a reply that is not a caret (`editing.rs` 667), or `superseded_reply`'s early return. Nothing heard changes. The e2e harness does not read reply kinds or these log lines. The ledger does (Nits).
- **The tightening (a report ends a held watch).** Only the program holding the watch, or a publication that claims it, can commit a report while the watch is Checking. No such program takes the watch before a report today: the takes are at `worker.rs` 1522, 1725, 2305, 2360, 2540 to 2563 and 2914, none followed by a report commit. Nothing heard changes.

## The clock

- **The outpost.** Every decision about the watch's lifecycle moves to the reducer on the observation clock. That clock is the run's shared clock in production, since `set_observation_clock` is called only by tests.
- **The worker's clock.** It still supplies `awaited_at_us` and the read stamps (`read_at_ms`), which are timing, not lifecycle decisions.
- **Core's clock.** Core still decides the held key's bounds from key times on the outposts' clock, which the design keeps on purpose.
- **Early wakes.** `next_timer` converts the due time through two clock reads made a fraction of a microsecond apart (`Now`, `crates/verbatim-outpost/src/outpost/shell.rs` 511 to 517). A timer that fired exactly on time could wake that fraction early and be timed again once. The wake-up latency far exceeds that gap, and the gap record has the same pattern. Not a finding.

## The held key's one bound

- **What was checked.** `next_held`, `start_held`, `held_read`, `held_wait_ended` and `text_completed`'s routing (`reduce.rs` 579 to 627). A late before read finds no `reading` with its query id and falls through every branch to nothing, as the table dropped it before. That holds for a terminal snapshot too.
- **No speech changes.** Passing a key on at Core's own deadline uses `held.pressed_at_ms`, as the table's `Failed` did.
- **No key is released later.** The read's deadline starts when the shell applies the effect, as `begin_until` did. The one slot never needs to hold two deadlines: the wait is bounded only while `reading` is None, and a read starts only once the wait ended.
- **The idle effect.** It is in the Nits.

## Counts and wake-ups

The counts were checked against the code and are correct:

- the four special-case blocks are 76 lines (`lib.rs` 812 to 819, 838 to 843, 1052 to 1067, 1098 to 1112 and 1344 to 1374);
- `Item` goes from 10 kinds to 9, and `Work` from 4 to 5;
- `OpenWatch` goes from 8 fields to 6;
- the five clock reads in the worker that decide the watch are at 867, 873, 1075, 1096 and 1150, and the two used for logs at 1017 and 1117.

The wake-up claims follow from the programs that go:

- the `End` commit on every caret key;
- the bound's program;
- the shell's `held_wait` firing after an early answer.

The poll read is unchanged. No new source is added, provided the frozen-clock case in the Nits is kept to tests.

## Step two

The judgement is right on the code. `check_watch_now` and `check_open_watch` find the node only after the worker resolves the event through its store, and `watched_edit` (`worker.rs` 1045) decides from the copy. So the reducer cannot know at dispatch which programs will check. The `Abandoned` and stuck-poll cases follow as the design says. Only the deadline figure in the argument is wrong (Nits).
