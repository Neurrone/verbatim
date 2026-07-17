# Event handling

NVDA turns raw platform notifications (winevents, UIA callbacks, RPC calls
from injected code) into named *NVDA events* ("gainFocus", "nameChange",
"valueChange", "show", …) executed against NVDAObjects on the main thread.
This file covers the generic pipeline; the per-API front ends are in
[MSAA and winevent handling](msaa.md) and [The UIA client](uia.md).

## Queuing

`eventHandler.queueEvent(eventName, obj, **kwargs)`
(`source/eventHandler.py`) puts an event on the main queue
(`queueHandler.eventQueue`) and requests a core pump. Focus-related events
get bookkeeping at *queue time* (`_trackFocusObject`): the queued focus
object is remembered (`api.setFocusObject` will later confirm it), which
lets other components ask "is there a newer focus already in flight?"
(`isPendingEvents`). gainFocus events queued behind a newer gainFocus are
effectively obsolete, and NVDA's *cancellable speech* mechanism
(`FocusLossCancellableSpeechCommand`, same file) attaches a validity check
to focus-announcement speech so that utterances for a focus the user has
already left are dropped from the speech queue before being spoken
(`speech.manager._shouldCancelExpiredFocusEvents`,
`doPreGainFocus`).

## Acceptance filtering

Because creating an NVDAObject and running an event costs synchronous
cross-process calls, events are filtered *before* object creation
(`eventHandler.shouldAcceptEvent`). The rules, all from that function:

- Anything a component explicitly registered interest in via
  `eventHandler.requestEvents(eventName, processId, windowClassName)` — the
  opt-in table `_acceptEvents` (app modules use this to receive events
  from background processes they care about).
- `hide` events: never accepted (flood control). `show` events: only for
  an allowlist of window classes (notification bars, tooltips, IME
  candidate windows, a Skype alert class).
- `valueChange`: accepted from anywhere only when background progress-bar
  reporting is on.
- Otherwise the event's window must belong to the foreground: descendant
  of the foreground window, sharing the foreground's root owner (Office
  ribbon and Edge downloads cases), descendant of the input thread's
  active window for `Windows.UI.Core` windows (UWP), a topmost window, or
  the desktop window itself (cursor events map there). Menu-lifecycle and
  desktop-switch events are always allowed since they become focus events.

This is NVDA's version of foreground gating: *background applications'
events are mostly dropped unheard*, by design, with explicit opt-ins per
feature that needs otherwise.

## Execution and the handler chain

`eventHandler.executeEvent` runs an event: lock-screen safety check
(objects below the lock screen are ignored while locked), honor the
object's `focusRedirect` (an NVDAObject may substitute another object for
its own focus event), then for gainFocus run `doPreGainFocus` (below),
then dispatch through `_EventExecuter`.

`_EventExecuter.gen` defines the handler chain, in this order: every
running global plugin, then the object's app module, then the object's
tree interceptor (only when the interceptor `isReady`, unless the handler
is marked `ignoreIsReady`), then the NVDAObject itself. Each level's
handler `event_<name>(obj, nextHandler)` decides whether to call
`nextHandler()` — so an app module can swallow or wrap default handling,
and a tree interceptor sees events before the plain object does. The
NVDAObject-level handler is called last with no next.

## What a focus event actually does (doPreGainFocus)

`doPreGainFocus` (`source/eventHandler.py`) is the heart of focus
behavior:

1. `api.setFocusObject(obj)` (`source/api.py`) walks the *ancestor chain*
   of the new focus by repeated `container` traversal, reusing the old
   ancestor list from the convergence point downward (it scans the old
   ancestors for a match to avoid re-walking, and caches the container
   link at the splice). It stores the list and computes the
   *focus difference level* — the index of the first ancestor that
   differs from the previous focus's chain.
2. If the difference level is 0 or 1 (the top changed), NVDA decides the
   foreground changed: it asks the desktop object for the real foreground
   (`objectInForeground`), falls back to ancestor index 1 if that fails,
   sets it (`api.setForegroundObject`), and executes a synthetic
   `foreground` event on it — foreground announcements are therefore an
   *effect of focus processing*, not a separate platform event.
3. It fires `focusEntered` on each ancestor from the difference level
   down — the "you have entered this container" announcements — before
   the gainFocus event itself runs. Which ancestors actually speak is
   decided by the object model's presentation rules
   ([Focus and the navigator](focus-and-navigator.md)).
4. Tree interceptor bookkeeping: if the focus moved into or out of a
   document with a tree interceptor, `event_treeInterceptor_loseFocus` /
   `gainFocus` fire (browse mode entry/exit; [Browse mode](browse-mode.md)).

A note on ordering: nothing guarantees platform events arrive in a sane
order. The MSAA side re-orders and coalesces before queuing (the ordered
winevent limiter, [MSAA and winevent handling](msaa.md)); the queue-time focus tracking plus
cancellable speech handle the remainder (a stale focus announcement is
cancelled rather than spoken). There is no timestamp-based arbitration:
freshness is by queue position and "is a newer focus pending."

## Object presentation settings

A cluster of config options (the Object Presentation panel; config
`[presentation]`) gates what event-driven announcements survive to
speech, checked at the consuming site rather than in the event
pipeline:

- Tooltips and help balloons/toast notifications — whether `show`
  events on tooltip/toast windows are reported (the acceptance
  allowlist above lets them through; the `ToolTip`/`Notification`
  behaviors in [Object model](object-model.md) then honor the
  setting).
- Progress bar output — speak percentages, beep, both, or off, plus
  "report background progress bars" (the explicit background-app
  event opt-in in `shouldAcceptEvent`).
- Reporting of object description, position information, and
  keyboard shortcuts on focus announcements — verbosity trims
  applied during `speakObject` content generation
  ([Speech](speech.md)).
- Dynamic content changes — the master switch for `LiveText`-based
  reporting (terminals and live text controls).
- Focus follows mouse-style options live elsewhere
  ([Mouse and touch](mouse-and-touch.md), [Review modes](review-modes.md)).

The pattern to note: NVDA filters *late* (at announcement time, per
setting) rather than early (at event acceptance), except where the
event volume itself is the problem (background progress bars, the
`show` allowlist).

## Watchdog interaction

Event execution runs on the main thread under the watchdog
([Main loop and watchdog](main-loop-and-watchdog.md)); every property fetched during
announcement generation is a potential stall, and during freeze recovery
`isAttemptingRecovery` short-circuits cancellable calls so the event
storm drains quickly. App modules can also put NVDA to *sleep* per
application (`sleepMode` on the app module — self-voicing apps): events
for sleeping apps are dropped in `executeEvent` after minimal focus
bookkeeping.
