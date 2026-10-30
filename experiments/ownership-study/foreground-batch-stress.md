# Stress test: a foreground change is never dropped by a newer focus in its batch

Read-only study, 2026-10-30. Nothing was built or run. "Run" below means one consecutive run of MSAA focus events and foreground changes in one outpost batch, as `plan` in `crates/verbatim-outpost/src/outpost/intake.rs` forms it. F(W) is a foreground change for top-level window W; f(x in W) an MSAA focus event on object x whose top-level window (GetAncestor GA_ROOT) is W.

## Decisions needed

- Adopt the exact rule below (rule P) in place of the rule as worded. The worded rule is right for the sequence it came from, but as worded it says a title that the next focus cuts off (a popup or owned window taking the focus), lets a focus older than the foreground change be tried in a window the user has left, and loses the title again when the newest foreground change fails its tests and an older one would pass.
- Accept one deliberate difference from NVDA. When a focus in W was raised before F(W) in the same run, NVDA says the window only and the focus is lost; P says the title and then the focus.
- Ask for a capture series before calling P's speech "matched". NVDA's own result for "F then f within about 10 ms" depends on timing (detailed below), and P always gives NVDA's result for the slow timing. The NVDA captures needed: Escape from NVDA's menu back to the Windows Forms window (the `text_box_editing` step), and a Windows Forms dialog opening and closing, about 10 to 20 runs each. Today no capture shows a Windows Forms dialog's speech (`docs/parity/focus-context.md` says so).

## What NVDA does (pinned source)

- `winEventCallback` (`IAccessibleHandler/internalWinEventHandler.py` around 163-193). A foreground event asks for a delayed pump (`PUMP_MAX_DELAY`, 10 ms, `core.py` 55). A focus event asks for an immediate one. So when the focus arrives within about 10 ms of the foreground event, both usually land in one `pumpAll`. They also share a pump when NVDA's main thread is busy as they arrive.
- `pumpAll` (`IAccessibleHandler/__init__.py` 1019-1075) collects focus and foreground events. When another accepted event arrives, or at the end of the pump, it tries the collected ones newest first and stops at the first that is processed.
- `processForegroundWinEvent` (816-908) drops the event in three cases:
  - its window is not `GetForegroundWindow()`;
  - the last queued focus is in that window (`isDescendantWindow`);
  - the last queued focus has the same event parameters.

  Otherwise it queues `gainFocus` on the window. The foreground event maps to "gainFocus" (`internalWinEventHandler.py` 36), so the window is spoken as the focus. `winEventToNVDAEvent` drops it for a window read through UIA.
- The limiter (`orderedWinEventLimiter.py`) keeps the last 4 focus or foreground events. A foreground event is also stored as a generic event, so it can appear twice in one flush. That changes nothing above.
- A window's title is also spoken when the focus enters it as an ancestor, but only if the window object is presentable (`doPreGainFocus`, `eventHandler.py` 378-419; `_get_isPresentableFocusAncestor`). A Windows Forms form's client object, reached through `accParent`, has no identity string. So NVDA does not say the form's title from the ancestry (`docs/parity/focus-context.md`, "Top-level windows in the ancestry"). For such a window the title comes only from a foreground event that gets processed.
- Result: for "F(W) then f in W" NVDA says the title only when the two land in different pumps. The `text_box_editing` capture says the title, so NVDA got two pumps there. How often NVDA gets one pump in that step has not been measured.

## Sequences

Each line gives three outcomes:
- **NVDA**: the pinned source with one pump. "2 pumps" means the source when the events are split.
- **Today**: current Verbatim, newest first within the run.
- **Worded**: the rule as Dickson worded it, taken literally: every F into a top-level window other than the held focus's is handled before the run's focus events, which are then tried newest first.

An outpost hooks one process, so a run only ever holds one application's events. Events from different applications meet only in Core, where they are ordered by observation time.

1. **Escape from Verbatim's menu to the Windows Forms window: F(W), f(edit in W).**
   - NVDA: edit only. With 2 pumps: title, then edit (as captured).
   - Today: edit only. The title is lost (the defect).
   - Worded: title, edit. Correct.
2. **Dialog opens: F(D), f(ctrl in D).**
   - NVDA: ctrl. For a Win32 #32770 dialog, the dialog name comes from the ancestry. For a Windows Forms dialog there is no title.
   - Worded: title (foreground read), then ctrl. The ctrl's line should meet the foreground node at D, by the window-handle identity rule.
   - Risk: if `focus_line` does not join the foreground's window node and the ancestry's client for D, the title is said twice. This path already runs whenever the worker is on time. The rule makes it the usual path, so it needs a mockapp test for both a Win32 and a Windows Forms dialog.
   - For a Windows Forms dialog this is extra speech compared with NVDA's one-pump result. No capture exists.
3. **Dialog closes: F(main), f(ctrl in main).** The same as 2. For a Win32 main window with a system menu, NVDA says the title from the ancestry and the words match. For a Windows Forms main window, Verbatim adds the title, and no capture exists.
4. **Focus in window A, then foreground to B, then focus in B (one application): f(a in A), F(B), f(b in B).**
   - NVDA: b (B's title from the ancestry if it is presentable).
   - Worded: B's title, then b. Correct while b is valid.
   - Defect: if b fails (window gone, or the object not focused when read), the worded rule goes on to try f(a) in A, a window the user has left. NVDA never reaches it, because the foreground event it processed stops the run. The outpost's live focused-state check usually drops it, but nothing guarantees that. An object whose focused state is wrong would say a stale "a" and cut B's title.
5. **The focus lands in another top-level window (an owned popup, a floating palette, a combo drop-down, a menu window): F(W), f(p in P).**
   - NVDA, one pump: p. With 2 pumps: W's title, which `doPreGainFocus` then cancels (foreground difference, `cancelSpeech`).
   - Worded: reads W, queues its title, and then p's line, which differs just below the desktop, cuts it. Whether a fragment of the title is heard depends on timing. It also costs a wasted read. Wrong.
6. **Two foreground changes in one batch (Alt+Tab between two windows of one application, or a dialog that opens and closes at once): F(A), f(a), F(B), f(b).**
   - NVDA: b.
   - Worded: handles both F(A) and F(B) before the focus events. F(A) fails "in front" (good). If both were in front in turn, both titles would be spoken: doubled. The wording does not say how many are taken.
   - Needed: take only the newest foreground change that passes its tests.
7. **The newest foreground change fails but an older one passes: F(A), f(a), F(B), where B closed at once and the foreground returned to A with no new event** (NVDA notes that foreground events come even when nothing changed).
   - Today and the worded rule (if F(B) is the only one counted): f(a) newest first, so A's title is lost again.
   - Needed: try the foreground changes newest first among themselves.
8. **A foreground change whose window has already gone.** It fails "in front" and `readable` with local calls only, so nothing is said. The fallback is the run newest first, as today. Correct.
9. **A foreground change whose window is hung.**
   - IsHungAppWindow is caught by `readable`, so the event is dropped. Correct.
   - A window busy but not yet "hung" blocks the title read, and the focus read waits behind it. The focus's own ancestry walk reaches W's client through W's thread anyway, so the exposure is about the same. A watchdog abandonment loses the rest of the group, as it would lose a focus read in W.
10. **Splash screen: F(splash), F(main), f(ctrl in main).**
    - Newest foreground change: main's title, then ctrl. The splash is dropped. Correct.
    - If the splash is still in front at handling: under the worded rule, the splash title is read and then cut by ctrl's line, as in sequence 5.
11. **UAC and the lock screen.**
    - The secure desktop raises nothing on the user desktop. The lock screen (LockApp) is read through UIA, so its F is dropped.
    - On return, F(W) and f in W can share a batch, and the rule says W's title and then the focus.
    - NVDA runs `processDesktopSwitchWinEvent`, then 200 ms later queues `gainFocus` on the system's focus, normally the focus alone.
    - Extra title compared with NVDA. No capture exists, and the secure desktop is unsupported until M8.1.
12. **A focus older than the foreground change in the same window: f(x in W), F(W).** This is rare. Activation raises the foreground event before WM_SETFOCUS. It does happen when the application sets focus before it is activated, and with hand-over copies sorted by raise time.
    - NVDA, one pump: F is tried first and passes against the old focus, so the window is said, the focus is lost, and the window becomes the focus.
    - Split across batches: f, then F dropped as a repeat (the focus is already in W).
    - Worded: title, then x. No outcome is the same in all three timings. Worded is the most useful outcome and is the difference recorded above.
13. **Split across two batches.**
    - [F(W)] then [f]: unchanged, title then f.
    - [f] then [F(W)]: f, then F dropped as a repeat (NVDA does the same).
    - No rule inside a batch can change either. The title is lost in the second, as in NVDA: a stated limit.
14. **Split between the listener and the outpost** (hand-over, before the hook is in).
    - Copies are sorted by raise time (`in_raised_order`), so "raised after F" is right.
    - A copy with no raise time falls back to arrival order. A focus raised before F in another window could then count as "after" it. The live focused-state check covers that.
    - After `ForwardingEnded` the hook is the one source, so the order is exact.
15. **Both backends.**
    - A window read through UIA gets no MSAA foreground change (it fails the UIA-owned test), so the rule never applies and the title comes from the ancestry, as in NVDA.
    - A UIA focus never joins or ends a run. In [F(W), UIA focus in W, f], the UIA focus is handled first. F is then a repeat (the focus is held in W, including unjudged reports) and is dropped, as NVDA's test against its last queued focus drops it. No change.
    - Foreground changes and focus events from different processes (the Settings frame and its content) are in different outposts. Core's top-level exemption in `is_stale_focus` and its "foreground of the window the focus is already in" rule handle them, and the rule does not apply.
16. **Alt+Tab burst between applications.** Each application's run holds at most its own F. Ones not in front at handling are dropped. The task switcher's window is UIA and dropped. Only the final application's batch matters (sequence 1 or 3). Correct.
17. **Menus and their owners.** Win32 menus (#32768), context menus, and the menu bar raise no foreground change, and closing them returns focus with no F. The tray area's and the desktop's foreground events are dropped by class in the hook (`is_shell_foreground_class`, `crates/verbatim-ia2/src/hook.rs`). Unaffected.

## The exact rule that survives (rule P)

For each run in a batch:

1. **Which foreground changes count.** A foreground change F(W) counts when it passes the existing foreground tests as it is tried:
   - not Core's hidden frame;
   - accepted;
   - in front now;
   - not a repeat of the held focus by NVDA's two tests;
   - readable;
   - not read through UIA.

   It must also meet one new test: when the run holds focus events raised after F, the newest of them has top-level window W, judged with GetAncestor GA_ROOT, a local call. This test drops sequences 5 and 10, where the next focus would cancel the title.
2. **Order.** Before any focus event of the run, try the run's foreground changes newest first. The first that counts is reported. At most one is reported per run, and the others are dropped (sequences 6 and 7).
3. **What is tried after it.** Then try newest first the focus events raised after the reported change, together with those raised before it in W (sequence 12). Drop every other entry of the run, such as older focus events in other windows (sequence 4).
4. **When none counts.** If no foreground change counts, try the run's focus events newest first, as today. A foreground change that failed is not retried. Its tests only get harder once a focus in W is held, so this keeps NVDA's result.

A run with no foreground change is unchanged.

Limits that remain:
- Sequence 13's second split.
- Sequence 5's variant where the newest later focus is in W but fails and an older one in P passes: the title is cut.
- Speech that differs from NVDA's one-pump result for Windows Forms windows (sequences 2, 3 and 11), pending captures.

## Cost

- **A batch with no foreground change:** nothing beyond today's scan in `plan`.
- **A run with a foreground change:**
  - Per foreground change tried: local calls only (class, GetForegroundWindow, IsWindow or IsHungAppWindow, GA_ROOT of the newest later focus, the held-focus lock). These are microseconds, and the in-front test fails first for a stale change.
  - The reported change costs one window read: the MSAA client snapshot (AccessibleObjectFromWindow and its property calls, about 5 to 10 cross-process calls, a few milliseconds for a responsive Win32 application, plus WM_GETTEXT when the name is empty) and the window's UIA probe, which the focus in the same window also makes.
  - It also costs one more fact to Core, one more reduction, and one more utterance.
  - There is no timer and no extra wake-up.
- **Latency:** the focus's read and speech start after the title's read. The title is spoken first and the first speech comes at about the same time, but the focus's words come later by the title's read and its spoken length. This is exactly the cost already paid whenever the worker is on time. The rule makes it the cost in every timing rather than adding a new one.

## Where it runs

Rule P depends on whether a tried entry was reported, which is known only after the local tests and the read on the worker. It also needs GetForegroundWindow at handling time. So it belongs where the tries happen, which today is the worker (`Planned::handle` and `try_newest_first`, and `foreground` in `worker.rs`).

If an architecture study moves the decision to Core, Core needs the outcome of the foreground try before it can order the focus events. That is a round trip per run with a foreground change, unless the outpost still makes this ordering and Core only judges the facts.

The raise-order facts (`in_raised_order`) and the GA_ROOT of each event's window are outpost-local, so the plan step can stay in the intake.

## Related findings (not fixed, for follow-up)

- A reordered foreground report carries `confirmed_at_us = now`, which is later than the observation time of the focus that follows it from the same outpost. Core's `latest_focus` therefore moves back in time between the two reports. Another outpost's focus observed in that gap and arriving between them is judged stale (`is_stale_focus`), although NVDA would handle it after the focus. The same gap exists today on the separate-batch path, and the rule makes that path more common.
- `docs/parity/focus-context.md`, "The desktop's announcement", still says the Program Manager and taskbar class rule for foreground events is not ported. `is_shell_foreground_class` in `crates/verbatim-ia2/src/hook.rs` ports it.
- NVDA defers all events until a foreground event's window is actually in front (`_deferUntilForegroundWindow`, `MAX_FOREGROUND_DEFERS`). Verbatim drops a foreground change that is not yet in front at handling, so the title is lost in that timing whatever rule this study picks. The foreground-wait study covers it.

## Redoing the mechanism (follow-up for Dickson)

The question: is there a mechanism that removes the two cases rule P left, even if the mechanism is redone entirely?
- **L1**, sequence 13's second split: a focus in W is handled, and W's foreground change arrives in a later batch, where it is a repeat and is dropped.
- **L2**: the newest focus in W fails, and an older focus in a popup P is reported, so the title is cut.

### New evidence that changes the premise

NVDA does not say the same thing for the same state. Two of the pinned-build captures show a Windows Forms form brought to the front with no title:
- `experiments/focus-pipeline-rework-measurements/captures/two-same-named-dialogs.txt` ("At entry NVDA did not say the form's title").
- `classic-context-menu.txt`, in the same folder ("Notes edit selected hello" on front, and the same on each Escape from the context menu).

The `text_box_editing` capture, Escape from NVDA's own menu back to a Windows Forms form, does say the title. By the source, the only difference that matters is timing:
- A foreground event asks for a pump 10 ms later (`core.py` 55, `internalWinEventHandler.py` 193). A focus within those 10 ms shares the pump, and newest first drops the foreground event.
- NVDA returns from its own menu through `postPopup` (`gui/__init__.py` 190-200), which shows and hides its hidden main frame to hand the foreground back. That plausibly separates the two events by more than 10 ms.

So any mechanism that always says the title on "foreground then focus" contradicts two captures. One that never says it contradicts one capture. This includes Dickson's rule, rule P, and options A to C below. Only a mechanism keyed on the events' timing, the way NVDA's pump is, matches all three.

### Option A: end-of-batch state, as Chromium recomputes focus

At the end of each batch, read the real foreground window (GetForegroundWindow, local) and the real focus. The focus read is GetGUIThreadInfo (local), then AccessibleObjectFromWindow and accFocus descent, or UIA GetFocusedElement: 2 to 4 cross-process calls. Speak the title when the foreground window differs from the last announced top-level window, then the focus when it differs from the last spoken.

Against the sequences:
- Sequences 1 to 3: the title is always said. This contradicts both front captures for Windows Forms.
- Sequences 4 to 8 and 10: the final state, correct.
- Sequence 9: the live read blocks like the event read.
- Sequence 11: a title where NVDA has none.
- Sequence 12: the title, then the focus.
- Sequences 14 to 17: correct, but sequence 15 needs both backends' live focus, which costs double.
- L2: removed.
- L1: remains. The focus is spoken before W is in front, so the state at that batch's end has no title. When the foreground arrives later, W is already the line's top-level window. The only ways to say the title are after the focus (out of order) or by holding every focus until the foreground is known.

Problems:
- It speaks the live focus, not the event's object, which NVDA never does except as a fake focus. Known cases differ: Windows Terminal's second runtime id, lists firing on child 0, Office.
- It adds 2 to 4 cross-process calls to every batch with a focus event. That raises cost on the event path.
- Fit with a pure per-application state machine is poor, because the machine's input becomes a read made inside the decision.
- It would run in the outpost, with the compare in Core.

Rejected.

### Option B: the foreground change as a fact that persists until spoken or superseded

A foreground change that passes its tests records "pending W" in the application's state. The next focus this application reports whose top-level window is W carries the title, taken from its chain's top entry by window text: a local read, no extra cross-process call. A focus whose top-level window is elsewhere clears the pending window silently. A newer foreground change replaces it. At batch end, a pending window with no focus is reported alone, as today.

Against the sequences:
- Sequences 1 to 12 and 14 to 17: as rule P, with no title read.
- L2: removed, because the title is decided after the focus outcome and never queued and then cut.
- L1: remains, for the reason given under option A.
- Sequences 1 to 3: still always the title, so the two front captures are still contradicted.

Cost: below P's. No extra cross-process call, no timer, no wake-up.

Fit: good with the per-application state machine, whose inputs are "foreground passed", "focus reported" and "batch end".

### Option C: Core judges from the final state

Core sees reports, not batches. It can know the final state only in two ways:
- Each superseded focus event is read and sent, which turns 1 read per run into N.
- Unread facts are sent and Core asks for the read, which adds one pipe round trip (about 0.3 to 1 ms) and one Core and one outpost wake-up per batch.

The outcomes are A's or B's, depending on the criterion, so it removes nothing more. It moves judgement within one application out of that application's machine, against the study's direction.

Rejected.

### Option D (recommended): cut runs where NVDA's pump would, by the events' raise stamps

Keep NVDA's newest-first try exactly as today, and drop the foreground-first rule. Change only where a run ends:

**A foreground change ends its run unless a focus event of the same application was raised within 10 ms after it, by the events' raise stamps (`WinEventStamp`).**

When the worker is on time, the intake holds a foreground change until its stamp plus 10 ms. The hold ends at once when a focus arrives. The result is NVDA's own result for the same event timing, independent of how late the worker wakes.

Against the sequences (gap means the time between the foreground change and the focus):
- Sequence 1: the title exactly when the gap is over 10 ms, as NVDA.
- Sequences 2 and 3: NVDA's. A quick Windows Forms dialog has no title, which matches both front captures (which Verbatim misses today when its worker is on time).
- Sequence 4: within 10 ms, b. Over 10 ms, B's title and then b. f(a) is dropped by the processed foreground change, as NVDA.
- Sequence 5: within 10 ms, p, with no title and no cut. Over 10 ms, the title and then the cut, which is NVDA's two-pump result.
- Sequences 6 to 11, 14, 16 and 17: NVDA's for the same timing.
- Sequence 15: unchanged, since UIA events never join an MSAA pump in NVDA either.
- Sequence 12: NVDA's: the window is said and the focus is lost.
- L2: removed. Within 10 ms, newest first reaches p before the foreground change, so no title is queued.
- L1: remains, as NVDA's.

Cost:
- No cross-process call.
- One timer wake-up per foreground change, and only when the worker is idle. None when it is behind.
- The title comes up to 10 ms later, which NVDA's delay matches. Focus speech is never delayed.

Fit: the per-application machine takes stamped events and a timer expiry, and outputs runs to try. It is pure and can be tested with stamps, no clocks. It runs in the outpost intake, where `plan` is today. Core is unchanged.

### What remains and why

Two cases remain under every option, and both are NVDA's own result in every timing:
- **L1.** The focus is reported before its window's foreground change arrives. The title could only come after the focus, or by holding every focus until the foreground is known, which costs latency on every focus.
- **Sequence 12.** A focus raised before its own window's foreground change, within 10 ms: NVDA says the window and loses the focus. It can be removed at no cost by one deliberate difference: within a run, a foreground change of W is tried after the run's focus events in W.

There is also one non-deterministic residue that no model removes: NVDA's main thread being late joins pumps that the 10 ms model keeps apart.

### Before building D

Check the `text_box_editing` trace for the gap between the Windows Forms foreground change and the focus. Then measure NVDA's gap on the same step, with a hook script like the context-menu capture's `menu-hook.ps1`.
- If Verbatim's gap is under 10 ms and NVDA's is over, D drops the title on that step. The difference then lies in how Verbatim's hidden frame hands the foreground back, compared with NVDA's `postPopup` show and hide. It is fixed there, not by an event rule.
- If both gaps are over 10 ms, D alone restores the title.
