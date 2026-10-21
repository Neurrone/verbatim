# The main loop, the queues, and the watchdog

NVDA runs almost everything on one thread and assumes that thread will
routinely get stuck in synchronous cross-process calls. The architecture
around that assumption — a heartbeat, a watcher thread, and cancellable
call wrappers — is among the most instructive parts of NVDA to a project
that chose a different answer to the same problem.

## The main thread and the core pump

NVDA's main thread runs a wx (wxWidgets) event loop; screen reader work is
scheduled onto it as a "core pump" (`source/core.py`, class `CorePump` in
`main()`). A pump cycle calls, in order: `touchHandler.pump()`,
`JABHandler.pumpAll()`, `IAccessibleHandler.pumpAll()`,
`queueHandler.pumpAll()`, `mouseHandler.pumpAll()`, `braille.pumpAll()`,
`vision.pumpAll()`, `sessionTracking.pumpAll()` (`core.py`, `CorePump`).
Pumps are requested (`core.requestPump`), slightly delayed to coalesce
bursts (with an "immediate" flag for latency-sensitive cases like touch
exploration), and explicitly non-re-entrant: a pump requested during a
pump schedules another rather than nesting (`core.py` comment referencing
issue [#3803](https://github.com/nvaccess/nvda/issues/3803)).

`queueHandler` (`source/queueHandler.py`) provides the general queue:
`queueFunction(eventQueue, …)` enqueues a callable for the next pump;
`pumpAll` also steps registered *generators* — long-running work (say-all
is the classic case) yields between chunks and is advanced once per pump,
cooperative-multitasking style.

So: events arrive from winevent callbacks, UIA handler threads, the
injected helper's RPC, and braille/input drivers; nearly all of them are
marshaled onto the main thread via these queues; and every property fetch
those handlers then make (COM into some app's STA) blocks the entire
screen reader while it runs. That is the design premise the watchdog
exists to make survivable.

## The watchdog: detection

`source/watchdog.py` runs a watcher thread against a heartbeat:

- Each pump cycle calls `watchdog.alive()`, which re-arms a waitable
  timer (`_coreDeadTimer`) `MIN_CORE_ALIVE_TIMEOUT` = 0.5 s in the
  future; `asleep()` (called when the pump goes idle) cancels it. If the
  main thread stops calling `alive()`, the timer fires and the watcher
  wakes (`_watcher`).
- The watcher then decides how patient to be
  (`_shouldRecoverAfterMinTimeout`): normally it waits up to
  `NORMAL_CORE_ALIVE_TIMEOUT` = 10 s before declaring a freeze — but it
  recovers after only 0.5 s if the evidence says the *foreground app* is
  implicated: `GetGUIThreadInfo(0)` reports no focused window ("the
  foreground thread is frozen," the comment says), the foreground window
  changed out from under the focus object, a system menu is open that
  NVDA hasn't caught up with, or focus moved to a different thread.
  A small allowlist of window classes known to run long legitimate calls
  (`safeWindowClassSet`: Internet Explorer_Server, Word `_WwG`, Excel
  `EXCEL7`) always gets the patient timeout.
- After 15 s frozen (`FROZEN_WARNING_TIMEOUT`) it logs stacks for all
  Python threads, repeatedly.

## The watchdog: recovery is call cancellation

Recovery (`waitForFreezeRecovery`, `_recoverAttempt`) is not restart —
it is *unsticking the main thread by cancelling whatever it is blocked
in*, in a loop every 50 ms until the heartbeat resumes:

- COM: at startup NVDA enables COM call cancellation on the main thread
  (`winBindings.ole32.CoEnableCallCancellation` in `initialize()`);
  each recovery attempt calls `CoCancelCall(core.mainThreadId)`,
  making the stuck call raise `RPC_E_CALL_CANCELED` in the caller.
- Window messages: NVDA's helper DLL hooks `SendMessage*` in NVDA's own
  process (`nvdaHelper/local/nvdaHelperLocal.cpp`,
  `fake_SendMessageTimeoutW` etc., installed with Detours, which patches
  the functions themselves, so every caller in NVDA's process is
  affected) so every send to another thread becomes `SendMessageTimeout`
  with `SMTO_ABORTIFHUNG` (except for messages in the application range
  0x8000 to 0xBFFF), its timeout capped at 10 s, sent in 400 ms slices
  (`CANCELSENDMESSAGE_CHECK_INTERVAL`) with a check of a shared
  `cancelCallEvent` before each slice (`cancellableSendMessageTimeoutTemplate`).
  A plain `SendMessage` is given a 60 s timeout, which the cap reduces
  to 10 s. The watchdog sets the event during recovery, aborting *all*
  in-flight and future sends until the core is alive again. A cancelled send is surfaced to Python by an
  exported callback that raises `exceptions.CallCancelled` in the
  calling frame via `sys.setprofile` trickery
  (`watchdog._notifySendMessageCancelled`).
- Known-risky calls are routed through `watchdog.cancellableExecute`:
  the call runs on a sacrificial `CancellableCallThread` while the main
  thread waits on {done, cancel} events; on cancellation the worker
  thread is *abandoned* (marked unusable, replaced next time) since the
  call inside it may never return. Off the main thread, or while the
  watchdog is suspended, `cancellableExecute` simply makes the call,
  so it protects nothing on the UIA event threads. `cancellableSendMessage`
  wraps the helper's cancellable send for explicit use.

`CoCancelCall` is aimed at the main thread only, and only COM calls
that go through COM's own channel can be cancelled; a cancelled call
surfaces as `exceptions.CallCancelled`, which a comtypes patch raises
in place of the `RPC_E_CALL_CANCELED` error
(`monkeyPatches/comtypesMonkeyPatches.py`, `new_WINFUNCTYPE`).
`CallCancelled` derives from `Exception`, not from `COMError`, so the
many property getters that catch only `COMError` let it through. NVDA's
own comment on `UiaHasServerSideProvider` calls it "a blocking
cross-process call that the watchdog cannot otherwise cancel"
(`UIAHandler._isUIAWindowHelper`); that UIA client calls in general
are not cancelled by `CoCancelCall` is a reasonable inference from
it, not something the source states.

## When an application stops responding

NVDA has no special case for a focus that arrives just before its
application stops responding, and no mode that reports a focus "from
the event" without its ancestors. What happens follows from the
ordinary mechanisms above plus one check, Windows' own verdict that an
application is hung (`winUser.isHungAppWindow`, which calls
`IsHungAppWindow`). Windows gives that verdict once the window's thread
has not retrieved messages for about five seconds; that figure comes
from Microsoft's documentation of `IsHungAppWindow`, not from NVDA's
source.

NVDA sets neither UIA timeout: nothing in its source or helper sets
`ConnectionTimeout` or `TransactionTimeout`, so UIA's defaults apply
(two seconds to connect to a provider and 20 seconds for a call, per
Microsoft's documentation).

Once Windows reports the application hung, NVDA stops making new
calls into it:

- `winEventToNVDAEvent` (`IAccessibleHandler/__init__.py`) drops every
  winevent whose window belongs to a hung application, after the
  ghost-window check and before any object is built.
- Every UIA event handler (focus changed, automation event, property
  changed, notification, active text position changed;
  `UIAHandler/__init__.py`) first asks `_shouldSkipEventForHungWindow`
  (`UIAHandler/utils.py`), which reads the sender's cached native window
  handle (cached by the base cache request every registration carries,
  never fetched live) and drops the event when that window is hung. An
  element without a window handle of its own is never treated as hung
  by this check.
- `Window.getPossibleAPIClasses` (`NVDAObjects/window/__init__.py`)
  offers no UIA, MSAA or Java Access Bridge class for a hung window,
  so any object built for it, including a parent reached through
  `GetAncestor`, is a plain `Window`, whose name is read with
  `InternalGetWindowText` without calling into the application.
  `isUsableWindow` treats a hung window as unusable, so the window-level
  sibling and child walks skip it.

Before that verdict, which is the case for a focus event raised just
before the application stopped, the event is processed normally and its
calls block:

- MSAA: the winevent hook is out of context, so the event itself
  reaches NVDA, and the main thread builds the object
  (`AccessibleObjectFromEvent`, then `IAccessible` construction) and
  blocks. The watchdog sees the main thread stop after 0.5 s and
  starts recovery at once when the foreground window no longer contains
  NVDA's current focus or the focused window is on a different thread
  from it (`_shouldRecoverAfterMinTimeout`); when the application took
  the focus from another application this is true, since NVDA's focus
  is still the old one. Otherwise it waits up to 10 s. Recovery cancels
  the call: `accessibleObjectFromEvent` catches any exception and
  returns nothing, so the focus event is dropped; a `CallCancelled`
  raised later, from a getter that catches only `COMError`, ends the
  whole core cycle (`CorePump.Notify` logs "errors in this core pump
  cycle"), and the winevents already taken from the limiter for that
  cycle are lost. Nothing is spoken for the focus. A window message
  that is still in flight when Windows declares the application hung
  returns at its next 400 ms slice, since `SMTO_ABORTIFHUNG` is set on
  every slice.
- UIA: with enhanced event processing on (the default), every UIA
  handler runs on the rate limiter's single flusher thread
  (`nvdaHelper/local/UIAEventLimiter/rateLimitedEventHandler.cpp`,
  `flushEvents`), one event after another. The focus handler makes live
  calls into the application before queuing the event
  (`getNearestWindowHandle`'s `NormalizeElementBuildCache`,
  `UiaHasServerSideProvider` when the 500 ms window verdict has lapsed,
  and `shouldAllowUIAFocusEvent`'s `currentHasKeyboardFocus`, which
  treats an error as "allow"). The watchdog does not watch this thread,
  so each call lasts until the application answers or UIA's timeout
  expires, and every UIA event from every application waits behind it,
  coalesced by element and event type. If the event is queued, the main
  thread then makes more live calls: the parent walk in
  `api.setFocusObject` (`GetParentElementBuildCache`), and in
  `event_gainFocus` the prefetch (`buildUpdatedCache`) and property
  reads. A parent fetch that fails with a `COMError` falls back to the
  window parent, so the ancestors become plain windows once the
  application is flagged hung. The values cached with the event
  (`baseCachePropertyIDs`, including the name; the role comes from the
  cached control type) are used only within the core cycle in which the
  object was created, because `AutoPropertyObject.invalidateCaches` at
  the end of every cycle discards them (`core.py`, `CorePump.Notify`);
  after that, reads are live and a failed name read gives an empty name.

During recovery (`isAttemptingRecovery`), NVDA executes no input
gestures: keys and mouse input pass to the system unhandled
(`inputCore.manager.executeGesture`, `keyboardHandler`,
`winInputHook.mouseHook`). `api.setFocusObject` calls `watchdog.alive()`
inside its ancestor loop, so an ancestor walk that makes progress ends
recovery.

Nothing is replayed when the application recovers. Dropped events are
gone, nothing re-reads the focus, and NVDA's focus stays whatever it
last committed (the previous application's object, if the dropped event
was the one bringing focus here) until the application raises another
focus event or the user moves the focus.

Crash handling is adjacent (`initialize()`): an unhandled-exception
filter writes a minidump and restarts NVDA, with a crash-loop breaker
(recent-crash timestamps; recovery disabled if too many crashes in a
window — `utils._crashHandler`).

A related hazard class is documented in
`source/garbageHandler.py` and NVDA's design note
`nvda/projectDocs/design/unreachableObjects.md`: Python's *cyclic*
garbage collector can release a COM pointer from whatever thread the
collection happens to run on, and releasing an STA object from the
wrong thread can deadlock or crash — the mechanism behind the
uncancellable `IUnknown::Release` freezes of issue
[#11398](https://github.com/nvaccess/nvda/issues/11398).
`garbageHandler` therefore *polices* cycles: it logs every object
that reaches the cyclic collector (`TrackedObject` instances —
event executers, NVDAObjects — should die by refcount, not
collection), treating any such log line as a bug. The transferable
rule: in a COM-holding system, object graphs must be kept acyclic so
destruction happens deterministically on a known thread.

### What a live capture shows

Captured on 2026-10-17 against NVDA alpha-58022 (the pinned commit),
with mockapp as the application: its window is activated, raises a focus
event for a button inside a grouping, and its window thread stops
pumping for 6, 20 or 30 seconds (mockapp's `on-activate` and `stall`
commands); in most runs it raises a second focus event, for a check box
in the same grouping, 8 seconds into the stall (2 seconds into the
6-second one). Each case was run three times with identical results;
the transcripts and NVDA debug-log excerpts are under
`target/e2e-evidence/nvda-hung-app/`.

- UIA, focus arriving from another application, no second focus: the
  button is spoken when the application resumes, 30 to 180 ms after it
  does, with its full ancestry ("Mockapp Tree Fixture window", "Group
  One grouping", "OK button" and its description and shortcut). Nothing
  is spoken during the stall; NVDA's focus stays on the other
  application.
- UIA, stall longer than UIA's call timeout (30 s): the flusher thread's
  first live call for the button (`getNearestWindowHandle`) fails with
  `UIA_E_TIMEOUT` about 22 s after it began; the element is then treated
  as not native and the event ignored. Nothing is spoken, not even when
  the application resumes 8 s later, and NVDA's focus stays on the other
  application.
- UIA, with a second focus during the stall: the button's event, held on
  the flusher thread until the application answers, is then ignored
  because the button no longer has the keyboard focus
  (`shouldAllowUIAFocusEvent`), and the check box's event, which waited
  behind it, is processed: the check box is spoken 30 to 70 ms after the
  application resumes, with the ancestors NVDA's focus did not already
  share (window and grouping when the focus came from another
  application; none when it was already in the grouping). The second
  event is not dropped although Windows judged the window hung when it
  was raised: the check box has no window of its own, so
  `_shouldSkipEventForHungWindow` cannot see the hang, and by the time
  the handler ran the window was no longer hung. The same held for a
  6-second stall and a 30-second one.
- A UIA focus event also reaches NVDA as a winevent, on the main thread.
  With the window's UIA verdict lapsed, `UiaHasServerSideProvider`
  blocks; when the focus came from another application the watchdog
  starts recovery after 0.5 s, the call is cancelled ("treating window
  as non-UIA"), `AccessibleObjectFromEvent` fails and the winevent is
  dropped. When NVDA's focus was already in the application there is no
  recovery: the main thread waits in `AccessibleObjectFromEvent` about
  5.2 s, until Windows judges the window hung, and the call then fails.
- MSAA: the button's focus winevent is lost the same way, after 0.5 s
  (from another application, by the watchdog) or about 5.2 s (within the
  application, by the window message failing once Windows judges the
  window hung; the watchdog does not intervene). The second focus is
  dropped at once ("Hung application. Dropping winEvent"). Nothing is
  spoken during the stall or after it, and NVDA's focus stays where it
  was, on the other application or the previously focused control.
- The 5.2 s is this capture's: NVDA's `AccessibleObjectFromEvent`
  failed with `E_FAIL` 5.20 to 5.24 s into the stall in all three
  `msaa-within` runs, with mockapp's window in front on the user's
  desktop. Verbatim's CoCancelCall experiment saw a different end for
  the same wait
  (`experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/readme.md`,
  experiment 2): from a multithreaded-apartment caller, with mockapp on
  a desktop of its own and stalled for 15 s, `AccessibleObjectFromEvent`
  returned after about 10 s with no error and oleacc's default client
  object for the window. Which difference in setup accounts for this is
  not established.
- About 8 s into a 20- or 30-second stall, Windows replaces the window
  with a ghost window (class `Ghost`, owned by `dwm`), which takes the
  foreground; NVDA drops its foreground, focus and name-change
  winevents as belonging to a hung application, and its UIA
  window-opened event through `_shouldSkipEventForHungWindow`. No ghost
  appeared in the 6-second stall.
- The activation that came just before the stall produced no foreground
  winevent for the application's window in NVDA's log, during the stall
  or after it; when the application resumes, NVDA receives the window's
  show event and its UIA window-opened event, neither of which is
  spoken.

## The evidence that stuck cross-process calls cause the hangs

Everything above encodes a diagnosis: NVDA freezes because its one
working thread makes synchronous cross-process calls that stop
returning, and NVDA lags when event floods serialize behind such
calls. The evidence that this diagnosis is correct, not folklore:

1. **By construction**: the entire subsystem above exists to detect "the
   main thread stopped moving" and to fix it *specifically by cancelling
   COM calls and window messages* — those are the only two recovery
   levers. If freezes had other primary causes, cancellation would not
   recover them; NVDA's logs ("Recovered from freeze after …") show it
   routinely does.
2. **Platform semantics**: Microsoft documents that a cross-thread
   `SendMessage` "is blocked until the receiving thread processes the
   message"
   ([SendMessageW remarks, Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendmessagew)),
   and COM calls into
   a single-threaded apartment are delivered through that same message
   queue with no default timeout ([COM](../explainers/com.md)) — so a hung
   or non-pumping app blocks any caller indefinitely.
3. **The bug record**: nvaccess/nvda issue [#1806](https://github.com/nvaccess/nvda/issues/1806) (convert every
   `sendMessage` to `cancellableSendMessage` "to eliminate freezing when
   sending window messages to windows that stop responding"); [#16749](https://github.com/nvaccess/nvda/issues/16749)
   (app thread sleeps, NVDA hangs silent until it resumes); [#15538](https://github.com/nvaccess/nvda/issues/15538)
   (watchdog triggers on slow-launching apps, initial focus lost —
   showing the recovery path itself costs correctness); [#10276](https://github.com/nvaccess/nvda/issues/10276) / [#10247](https://github.com/nvaccess/nvda/issues/10247)
   (Word object model calls freezing NVDA); [#11398](https://github.com/nvaccess/nvda/issues/11398) (freezes in COM
   releases during garbage collection — a cancellation blind spot,
   since `IUnknown::Release` cannot be cancelled).
4. **The busy-background-app case** is flood plus serialization rather
   than a single stuck call: winevents from *every* process funnel into
   the one pump, and each acted-on event costs synchronous queries into
   the (busy, slowly-responding) source process. NVDA's defenses measure
   the problem: the ordered winevent limiter caps events at 10 per
   thread per pump and 4 focus changes total
   (`source/IAccessibleHandler/orderedWinEventLimiter.py`,
   `MAX_WINEVENTS_PER_THREAD`), and `_shouldRecoverAfterMinTimeout`'s
   fast path exists because waiting the full 10 s with a dead foreground
   was judged unacceptable. Issues [#16703](https://github.com/nvaccess/nvda/issues/16703) (sluggish then multi-minute
   freeze in an event-heavy web app) and [#7197](https://github.com/nvaccess/nvda/issues/7197) (elements list freezing
   on huge pages) show the residual symptom.

In sum: for the foreground-hang case the mechanism, the platform
documentation, and NVDA's own remedy all agree; for the
busy-background case, NVDA's flood-control code and its issue record
show the lag arising from the one pump serializing event handling
behind slow synchronous queries. NVDA's chosen answer is
heartbeat-plus-cancellation on that single thread, and its
`safeWindowClassSet` shows what the one-thread model forces:
case-by-case patience heuristics per window class.
