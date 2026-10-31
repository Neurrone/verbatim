# Review of the outpost's program bookkeeping design

The design reviewed is `docs/design/logic-foundations.md`, section "The outpost's program bookkeeping (a build of its own)" (lines 280 to 412 of the working copy on 2026-10-31). It was read against `crates/verbatim-outpost/src/outpost/` (reducer, worker, inbox, shell, commit, text_reads), Core's held-key wait (`crates/verbatim-core/src/editing.rs`), the mockapp tests that settle, and `nvda/source/editableText.py`. Design line numbers below are the working copy's; code line numbers are today's.

Important findings: 4.

## Does the design reach its goal

Yes, for the most part. With the recommended poll option, the build removes the poll's second slot, `behind_poll`, the re-dispatch after an abandoned poll, the yield, the worker's held queue, the carry, the `Settles` slot with its `in_charge` cleanup, and `admit`'s verdicts branch. It adds one state: the reducer's set of pending settles. Each request has one answer path in every sequence below, except where a finding says otherwise. The four Important findings concern a behaviour change the design says it does not make, the evidence behind the recommended poll rule, an unanalysed change in what is read first during a poll, and part five's cost, which falls on an unrelated program.

## Important findings

### 1. Part three's rule lets a foreground change inherit "not in front"

- Where: design line 343; today's rule at `crates/verbatim-outpost/src/outpost/worker.rs` 4408 to 4431 (`judged_focus`).
- Defect: `reducer::Focus` carries `in_front: Option<isize>` for both a foreground change and a focus whose top-level window was read as the foreground window, and the reducer's rule is "inherited from the newest report in the same top-level window, or else the evidence". So a foreground change now inherits. Today a foreground change never inherits: `inherited` is `None` when `foreground` is true, and the change sets `Some(top_level)`. The design says the behaviour is unchanged.
- The reducer cannot tell the two cases apart from the report either. A foreground report's `event` is `foreground_event(hwnd)` = `(hwnd, OBJID_CLIENT, CHILDID_SELF)` (worker.rs 609), the same triple as an MSAA focus on the window's client object.
- Counterexample:
  1. Window W of the application is in the background. A focus event in W is reported as F1. W is not the foreground window, so F1's `entered_in_front` is `None`, and F1 is the newest report in W.
  2. The user switches to W. The foreground change is reported as F2 with `in_front = Some(W)`.
  3. Today F2's `entered_in_front` is `Some(W)`. Under the design's rule, F2 inherits `None` from F1.
  4. Every later focus inside W inherits `None` too, so W never counts as entered in front. What is heard changes, which the design's own last question (line 412) treats as a change to be decided separately.
- Fix: carry the foreground flag in the evidence, for example `in_front: InFront { Foreground(isize), Read(Option<isize>) }`, with the rule "a foreground change sets it; otherwise inherit; otherwise the read". Also add the new replay test's counterpart: a foreground change after a background focus in the same window.

### 2. The recommended poll rule rests on a misquoted measurement

- Where: design lines 359, 362 and 409; the evidence is in `docs/design/key-holding.md`, "The console host's caret poll" (Costs), and in `experiments/the-console-host-raises-its-caret-events-on-its-blink-timer/`.
- Defect: the design says "every watch the poll answered was answered within 8 ms", and concludes that the 50 ms wait falls "only after a key whose caret does not move". The evidence does not support either claim.
  - What key-holding.md measured was every watch left open by the first check, answered by "one re-read, or the console update's check that comes as soon". Some were answered by an event's check that ran because the poll yielded, which is the behaviour the recommended rule removes. The 8 ms figure therefore does not describe the proposed rule.
  - The workload (`measure_keys_conhost`, `crates/verbatim-e2e/src/scenarios/measure.rs` 1037 to 1074) presses only keys that move the caret: Left and Right over the typed line's last ten characters, Backspace, Escape and typing. It has no key that moves nothing, so it says nothing about how often the 50 ms is paid.
  - The blink-timer experiment found the cursor moved "at most 54 ms after the key", which is past `POLL_BOUND` (50 ms). So a slow move also pays the whole bound, and then waits for the blink-timer caret event, 257 to 540 ms after the key.
- Counterexample: a console host that handles a Left Arrow 54 ms after it is passed on (the experiment's maximum). The poll reads until 50 ms and finds nothing, and its watch ends at the bound. A typed character's console update that arrived at 10 ms waited about 40 ms behind the poll, although the key did move the caret.
- Fix: before the question goes to Dickson, measure the proposed rule on a workload that includes keys that move nothing (Left at the start of the line, Home at the start, Up with no history), and state how long a focus or echo waits behind a poll (P50, P99, maximum), in release builds and test audio mode. Then reword lines 359, 362 and 409 to match the measurement.

### 3. The recommended rule changes which read sees a console update first, and no scheduled scenario would notice

- Where: design lines 359 and 400 (scenarios `conhost_up_typing`, `conhost_editing`, `conhost_commands`) and line 405 (`conhost_typing` left out).
- Defect: today, an event arriving during the poll makes the poll yield before its next re-read (`look`, worker.rs 335; `Worker::poll` 1181). The event's program then checks the watch and reads the terminal under the event's own trace. Under "the poll waits for nothing", the poll's re-read answers the watch first. After that answer it reads the terminal under the key's trace (`terminal_output(node_id, open.trace, 0)`, worker.rs 1235 to 1237), and that read takes in the text the console update brought. The design states no expected speech for this order, which CLAUDE.md asks of a design. `crates/verbatim-e2e/src/scenarios/terminal_key_timing.rs` (lines 33 to 39) records that where a read falls between Up Arrow and a typed "x" changed what was heard ("echo onex", or the line and then the "x").
- Counterexample:
  1. In the console host, Left Arrow is pressed at the start of the line. Its first check finds nothing, the watch opens with a poll, and P is dispatched.
  2. At about 10 ms, "a" is typed. A `WaitingKey::Later` key is not held for the watch (`editing.rs` 290). The console inserts "a", the caret moves one place right, and console update U arrives.
  3. Today: U is dispatched behind P. P yields at its next look, and U's program checks the watch (the caret moved) and reads the terminal as U.
  4. Proposed: U waits. P's next re-read finds the move and answers the Left Arrow with the caret after "a". P then reads the terminal under the Left Arrow's trace, so the inserted "a" is first read as the key's result. U's read then finds nothing new.
  5. Whether Core speaks "a" as typed echo, as the key's character, or both is not established. `conhost_typing`, the one scenario that types into the console host, is left out as flaky. `conhost_up_typing` types only after the recalled line is heard, so it never types during a poll.
- Fix: state the expected speech for a character typed during an unanswered poll, check it against Core's terminal reading rules, and add or name a scenario that types within 50 ms after a key that moves nothing. Otherwise end the poll's re-reads on a console update for its node, which the design would then have to cost.

### 4. Part five moves a program's drops under the next program's deadline, and lets a settle return before them

- Where: design line 382 ("`Finished` is posted before the context's drops, which then run under the next program's deadline"); today's order is at worker.rs 438 to 467, where failed reads are counted, the context is dropped, and only then `Finished` is posted.
- Defect: once `Finished` comes first, the reducer counts the worker as idle while the worker is still releasing objects, and the design itself says those releases "can call into the application". If a release hangs, it is the next program that is abandoned: its query is answered `Abandoned`, and its entry is never handled. With part one, a settle, and Core's idle round, are answered with no program in flight, so they return while the previous program is still calling into the application. Today they cannot, because the drop comes before `Finished`.
- Counterexample:
  1. Program A handles a focus and returns. `Finished(A)` is posted, and A's context drop starts releasing an element. The application is busy, so the release blocks.
  2. The reducer dispatches B, a console update or a held key's before read, which waits in the inbox behind the drop.
  3. B passes its deadline, or the moved-on grace measured from B's dispatch (`judge_abandonment`, reducer/mod.rs 987 to 994) runs out. B is abandoned and its query is answered `Abandoned`, although B never ran. Today A, whose work was already done, would have been abandoned, at no cost to any entry.
  4. In a mockapp test, `outpost.settled()` after A returns once the barrier is reached, while A's release is still under way on the worker.
- Fix: if part five is built, post `Finished` after the drops and carry the last commit with it, which still saves the separate commit wake the measurement looks for. Otherwise state the cost, and make the barrier wait for the worker's drop.

## Nits

- Line 359: the worst case "plus one read" assumes a short read. A poll's read can last until `HANDLING_DEADLINE` (10 s, reducer/mod.rs 68), and the poll's `window` is 0, so the moved-on grace never ends it (reducer/mod.rs 990). This is pre-existing (today a program behind the poll waits the same way), but the worst case should say so.
- Line 359: the poll returns "when its inbox closes", but `Inbox::wait_until` (inbox.rs 106) reports nothing. The poll needs a `try_take` after each wait. Keep `the_poll_ends_when_its_inbox_is_closed`, as the design does.
- Part two: `Posted::Verdict` must be taken after the mailbox closes, as `Commit` is (`Mailbox::post`, shell.rs 137 to 156). The design does not say.
- Line 345: "about a microsecond" for `GetForegroundWindow` is an estimate, not a measurement. The read also runs on the shell thread, for the publication at a walk's bound (worker.rs 4655).
- Line 360: three or four wakes per re-read over ten re-reads is 30 to 40 more, not "up to 30".
- Part one removes the settle's loop over noted carets. Today a settle takes a noted caret each time it is reached (reducer/mod.rs 1362 to 1365). The barrier waits for one noted caret (`behind_noted`). Whether a noted caret's program can note another caret was not checked.
- Line 401's scenarios (`notepad_caret_keys`, `text_box_caret_keys`) exercise only the round's path. The settle path runs only in the mockapp suites that settle (`caret_watch.rs`, `call_counts.rs`, `terminal_reading.rs` and the others), which the commit should name.

## Sequences tried that hold

- Settle one answer. The answer path was traced for a settle pushed during a program, during that program's abandonment, with a noted caret pending, and at `Close` (the mailbox drops `Posted::Settle` after close, shell.rs 139 to 152, so the waiter is dropped and `Outpost::settle` returns). Each has one answer.
- Moves made before the barrier are ordered ahead of the caller-thread waits. Subscription moves are made by the shell from effects (`retarget_with`, shell.rs 915), messages are queued by the shell before `Settled` is applied, and the event thread's sync already runs on the caller's thread (outpost/mod.rs 1314 to 1324).
- The caller-thread waits do not deadlock. `Registration::settle` returns at once once the registration is closed, `Outbound::flush` is never called on the writer thread, and none of these waits runs on a thread that answers them.
- The caret watch tests keep their meaning. In `caret_watch.rs` (lines 180 to 187 and 248 to 260), a settle after a query sees the watch opened and open. The key's query is ahead of the barrier in the one mailbox, and the barrier is answered without the round's wait for the watch.
- No commit's acceptance changes under part two. `accepts` is unchanged, `record_verdicts` already judges by generation (reducer/mod.rs 815 to 845), and a refused commit gave the reducer nothing but verdicts. No dead generation's commit is now accepted, and no live one is refused.
- No fact is read stale without the carry, under the recommended option. Nothing is dispatched while the poll is in flight. Commits and `commit_text` are posted before `Finished` (worker.rs 435 and 462), and the store's text is written by the shell at commit.
- The rules for an abandoned or panicked poll hold. An ordinary program that took the watch has `running` set to the watch's query, but the watch is already out of the reducer's facts, so the moved rules do not fire for it, and it is answered `Abandoned` or `Failed` once. A late `Answered(q)` from an abandoned poll is refused because its slot is empty. Since `until` is at most 50 ms past the key and the deadline 10 s past dispatch, an abandoned poll's next step always ends at `after_read`, with no further call.
- A key arriving during the poll waits no longer than it does today. Core holds a held key, and a review command or script, until the key ahead's watch is answered or 100 ms pass (`editing.rs` 255 to 266). The NVDA comparison at line 354 matches `_hasCaretMoved`: it returns early for a queued script or a pending focus, and for caret or text change events only where `caretMovementDetectionUsesEvents` holds, which is false in consoles.
- Focus, echo and release wait behind at most one poll. A poll is dispatched only when nothing else is ready, so anything waiting when it ends goes first.

## Facts with an owner in two places

- Pending settles: the reducer's set, the shell's waiter map, and the barrier items in the intake. Under the recommended option, the set holds exactly the intake's `Asker::Settle` items, so it can be removed: the shell's check can read the intake's barrier items instead.
- `entered_in_front`: judged twice, by the program's local apply and by the reducer at commit, from the same evidence. The design argues that the two copies are equal at each commit, and the argument holds. The design still calls it "decided only by the reducer's pure function", which is not true of the program's copy.
- The console poll's progress (`poll`, `calls`): the reducer's watch keeps the dispatch-time value while the program holds the live one. Nothing reads the reducer's value while the poll is in flight.
- Verdicts: the program's copy takes a verdict at once, and the reducer records it only by generation, both as today. Only an abandoned worker's copy can differ, and that worker makes no further call.

## Lifecycle states added

- The reducer's set of pending settles. It can be removed, as above.
- Under "today's yield" only: a settle reached and held in that set until the poll's `Finished`. This state goes away if that option is not chosen.
- Part five's `last: Option<Effects>` on `Finished`. It adds no state, but it moves the drops after the end, which finding 4 shows has a cost.
- `Input::Verdict` and the `in_front` evidence are a message kind and a field, not states.
