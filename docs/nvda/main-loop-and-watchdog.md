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
  `fake_SendMessageTimeoutW` etc. installed via Detours-style import
  hooking) so every send becomes `SendMessageTimeout` with
  `SMTO_ABORTIFHUNG` plus a 60 s cap, checked against a shared
  `cancelCallEvent` in 10 ms slices — the watchdog sets that event
  during recovery, aborting *all* in-flight and future sends until the
  core is alive again. A cancelled send is surfaced to Python by an
  exported callback that raises `exceptions.CallCancelled` in the
  calling frame via `sys.setprofile` trickery
  (`watchdog._notifySendMessageCancelled`).
- Known-risky calls are routed through `watchdog.cancellableExecute`:
  the call runs on a sacrificial `CancellableCallThread` while the main
  thread waits on {done, cancel} events; on cancellation the worker
  thread is *abandoned* (marked unusable, replaced next time) since the
  call inside it may never return. `cancellableSendMessage` wraps the
  helper's cancellable send for explicit use.

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
