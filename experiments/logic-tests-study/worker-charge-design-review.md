# Review of "The shared worker charge" design

Reviewed: the section "The shared worker charge (a build of its own)" of `docs/design/logic-foundations.md` (working tree, lines 1606 to 1784), against main 39dce0b2. Read only; nothing was built. NVDA lines were read at the pinned 732d1ce2.

Important findings: 5.

## Whether the goal is reached

One `Charge<T>` can serve both readers. The real differences stay with the readers: the listener learns a window during the read (`Moved::Unknown`), keeps a charge per application, reuses a returning abandoned thread and keeps the abandoned limit; the outpost keeps `hand_on`, the focus group and the owed work. No fact gets a second owner: `TickRead`'s gathered copy and `WatchedRead`'s claim, the two copies today, both go. The counts were checked against the code: the outpost's reducer goes from 16 fields to 13 and its program in flight from 10 fields with 3 flags to 3 fields with 1; the listener's schedule goes from 10 inputs to 14 and from 8 effects to 10, and `App` from 9 fields to 8. The listener's larger interface is still a simplification of ownership, because each new input replaces a judgement the shell made outside the pure reducer. It is not a simplification of interface size. The findings below are defects in the rules as written, not in the choice of one type.

## Important

### A moved-on answer is dropped once the walk is published, so a stuck reader that has been published is never freed by the moved-on rule

Lines 1706 and 1671. The rule "an answer whose request is not running, or whose walk is published, is dropped" also drops `MovedOn`. But both readers today abandon a published reader that is still stuck when the user moves on:

- the outpost's `judge_abandonment` ignores `claimed` (`verbatim-outpost-logic`, `src/lib.rs` 1003 to 1030);
- the listener's `tick` computes `MOVED_ON` whatever `running.published` says (`schedule.rs` 806 to 813), and `Claim::Publish` keeps the read watched (`focus_read.rs` 1796 to 1803).

Counterexample, in the outpost:

1. File Explorer's folder window W, on thread A, has a focus walk stuck in `GetParentElementBuildCache`.
2. At 2 s the tick asks `Publish(R)`. It is answered `got`, so the walk becomes `Published`.
3. At 2.5 s the user moves to the folder window on thread B, and a foreground fact arrives. `foreground` asks `AskMovedOn(R)`, which is answered `true`.
4. The walk is `Published`, so the answer is dropped.
5. The worker stays in charge until its 10 s deadline. Every later event of explorer.exe, the new window's focus included, waits that long.

Today the worker is abandoned at step 3. The listener's version of this holds the application's events until the 8 s deadline. The determinism section has the same sentence (line 705), so both places need it: a `MovedOn` answer applies whenever its request is running, published or not. A generated-test invariant should check that a published item can still be abandoned for `MovedOn`.

### Taking the progress at `Publish` makes the reading thread panic after every publication at the bound

Line 1681, "`Publish` takes the read's progress ... that replaces `WatchedRead`'s claim". When its walk ends, the reading thread takes the progress with `.expect("the read's progress, armed above and taken only here")` (`focus_read.rs` 1029 to 1031). Today the watchdog only reads the progress, through `prepare_publish` (513 to 518).

Counterexample:

1. A walk is stuck past its bound.
2. The tick's `Publish(R)` takes the progress, and the focus is published.
3. The stuck call returns, and the thread's `take_progress()` is `None`. The thread panics.

A panicking thread sends no `Returned`, so the charge keeps its item until the deadline. An abandoned thread never counts back, so after two such reads the application stays at `ABANDONED_LIMIT`. From then on its events wait and are dropped at `READ_DEADLINE`.

Taking the progress is not needed, because `Walk::Published` already prevents a second publication. The fix is for `Publish` to read the progress, as `prepare_publish` does today. The alternative is for the thread to treat `None` as published, which makes the progress a claim again.

### The grace-end alarm brings back e46d4183's wasted wake for each quick read while another read is under way

Lines 1698 and 1741. The no-timer choice is e46d4183's (2026-10-20): "The watchdog set its timer for every read's grace end, a second wake-up per quick read". 0b5826cd only added the "front already elsewhere" evidence. The alarm is now a timer that a read's return cancels, but only through `settle_alarm` (`schedule.rs` 710 to 721). That cancels only when no application has a read under way and none waits at the limit. A read's start sets the alarm with `AlarmChange::By` (638), which never moves it later.

Counterexample:

1. App X's read is held, for up to 8 s, or X waits at the abandoned limit.
2. During that time the user tabs through app Y. Each of Y's focus reads with a cached window starts `Pending` and sets `By(start + 500 ms)`.
3. Each read returns in 50 ms. The alarm is not settled, because X's read is still under way.
4. The watchdog wakes at each of Y's grace ends and finds nothing to do.

This breaks "a loop never times a wake it will not act on", and the design's claim of "a watchdog wake only for a read still under way at 500 ms". The same holds today for `on_arm`'s `By(bound)`, which is pre-existing.

The fix: after each step that ends or abandons an item, the shell sets the alarm to the minimum of `next_wake` over the charges and the limit ends (`Set`), not `By`. The cost of that, one timer system call per read's end, belongs in the ledger.

### Re-judging only at foreground events misses moves to the taskbar and the desktop, the main same-application case

Lines 1699 to 1701 and 1776. A move to the taskbar (`Shell_TrayWnd`) or the desktop (`Progman`) is a `ShellForeground` event (`verbatim-ia2`, `hook.rs` 119 to 125). The listener returns on that kind before `foreground_changed` (`listener.rs` 1040 to 1047), and no `DeliveredFact::Foreground` reaches the outpost for it either. Both windows are explorer.exe's, on threads other than its folder windows. The parity entry names the taskbar as a reason for the rule.

Counterexample:

1. A folder window's read is stuck.
2. At 0.5 s the grace end asks, and the answer is false.
3. At 1 s the user clicks the taskbar.
4. Neither reader judges again until the bound (a 2 s publication) or the 8 s or 10 s deadline. Until then the taskbar's own events wait behind the stuck read.

NVDA re-judges every 0.5 s (`watchdog.py` 133 to 145, `_shouldRecoverAfterMinTimeout` 206 to 232) and recovers by 1.5 s. The design names only "a child window of another thread", which understates what is heard. It also drops today's listener re-judgement at later ticks while evidence holds (`moved_on_wanted`, `schedule.rs` 422 to 432), which caught such a move at the bound's tick.

The fix: either state this case among what is heard and in the limitations, or step the charges on `ShellForeground` too. The second costs one ask per shell foreground change during a read, with no wake.

### Answering asks inside the step loop, in effect order, trips the shell's owed-marker check

Line 1681. `step` checks at its end that the owed markers equal `schedule.markers()` (`focus_read.rs` 1840 to 1843). An ask answered inside the outer loop runs a nested `step`, and the nested step checks this while the outer step's later `Outcome` effects are not yet applied. A listener `Tick` or `Foreground` steps several charges, so "one ask per transition" holds for each charge, not for the schedule.

Counterexample:

1. App P1's read reaches its grace end in the same tick in which app P2's waiting events pass their bound at the limit.
2. The tick's effects are `[AskMovedOn(P1), Outcome(P2, WAITED_TOO_LONG)...]`.
3. The nested `MovedOn(P1)` step ends with P2's markers still owed, although the schedule no longer holds them.
4. The `debug_assert_eq!` fails. That is a panic in the debug builds that `cargo xtask ci` and the end-to-end suite use.

The fix: the design should say that the shell answers asks after every other effect of the step, or checks only at the outermost step, and that a schedule step may carry one ask per charge.

## Questions the brief asked

- **What is heard, against NVDA.** The cited lines check out: `watchdog.py` 47, 133 to 145, 151 to 163, 186, 206 to 232 and 235 to 240, and `NVDAObjects/UIA/__init__.py` 2136 to 2145.
  - Publish first is nearer NVDA. NVDA cancels the main thread's call and falls back to the window's parent, so it speaks the focus, never drops it. Publication applies only after the read is armed, which is after NVDA's checks, so it matches only NVDA's main-thread phase, as it should.
  - The grace end judged without evidence matches NVDA's first judgement.
  - "No judgment at later wakes" is further from NVDA than the listener today, by the taskbar case above.
  - The "closer to NVDA" claim holds for the first two changes, not the third.
- **0b5826cd.** Its own changes are kept: judging only once the window is known, and no seed from the keyboard focus window (`Moved::Unknown` keeps both). The defect the reversal can bring back is e46d4183's wasted grace-end wake for quick reads, in the case described in the third finding. The design attributes the no-timer choice to 0b5826cd; it was e46d4183's.
- **The `WinEvent` thread.**
  - It makes only local calls: `GetGUIThreadInfo`, the owner reads, a console's `OpenProcess` and `NtQueryInformationProcess`, and `plain_windows_start`'s `GetAncestor`.
  - It may also start a thread (tens of microseconds) and send outcomes and idle answers on channels.
  - It already takes the readers' lock for every foreground event, so it waits on no new holder.
  - Out-of-context `WinEvent`s are queued, not dropped, so a slower callback delays later events and loses none. `forward_msaa_event` runs first, so the foreground fact's stamp is not delayed.
  - Nothing it drops holds a COM object: the listener's item is `Meta` and places, and `ReadProgress` and the dropped `WatchedRead` hold plain data.
  - No finding here.
- **The lock and the removed atomics.**
  - `claim`, `front_judged` and `front_elsewhere` go. Their job moves to refusal by request under the readers' lock, which removes `window_after`'s staleness case.
  - `Learned` and `Armed` are stepped from the reading thread with no other lock held. `arm` drops the progress guard before `on_arm` locks, so no inversion is added.
  - No race is reintroduced. The progress-take defect above is the only fault on this path.
- **The counts.** The outpost's and the listener's counts are right. The line figures do not add up: the outpost loses about 120, the schedule about 150 and the shell about 70 or more, and the charge adds about 250. That is about 90 to 150 lines fewer, not 250.

## Nits

- Line 1668: `walk_returned` should disarm the walk when it is not past, as `WalkReturned` takes the bound today (`lib.rs` 600 to 607). Otherwise `next_wake` times a bound for a walk that has returned, and the bound's `Publish` answers `got = false`.
- Line 1681: `Publish { pid, request }` should find its read by request, as the outpost's publication finds it by generation (line 670). It should not find it by `pid`'s current `watched` entry.
- Line 1684: per-application charges restart their generation when `App` is removed and remade (`schedule.rs` 655 and 1006). That is safe only because removal needs `abandoned == 0` and runs under the same lock as the ending thread's `in_charge` check. Either state that, or keep generations monotonic across removal.
- Line 1708: after a `Published(R, false)` in the race where the thread had already taken its progress, the thread may restore it for the legacy MSAA call (`focus_read.rs` 1256). The walk is then `Unarmed` and is not published at the bound. The same happens today, so this is pre-existing.
- `docs/design/listener-without-connections.md` 120 and 124 give `READ_DEADLINE` as 11 s. The code makes it 8 s (`focus_read.rs` 203 to 205). Pre-existing.
