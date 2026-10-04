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

Three details of how the filter is applied matter for parity:

- It applies to focus events too. MSAA focus and foreground events,
  and UIA focus-changed events, pass through `shouldAcceptEvent`
  under the name "gainFocus", so a focus event from a window outside
  the foreground (and not topmost, not sharing the root owner) is
  dropped.
- "The foreground" is the operating system's foreground window at the
  moment the main thread processes the event batch, not NVDA's own
  record of it.
- UIA notification events do not pass through `shouldAcceptEvent`.
  They are filtered by application instead: the base
  `event_UIA_notification` on UIA objects returns without speaking
  when the notifying element's application differs from the focus's
  application. Exceptions are per-application opt-ins: the File
  Explorer app module speaks the shell's window-snap results
  (activity id `Windows.Shell.SnapComponent.SnapHotKeyResults`) from
  anywhere, and the Voice Access app module accepts notifications
  from elements without a window. Toast notifications reach the user
  through the `alert` event instead, which the filter accepts when
  the window's parent has the class `ToastChildWindowClass`.

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
2. With the new focus and its ancestors in place, the speech manager
   culls expired focus speech (`removeCancelledSpeechCommands`): of the
   utterances already handed to the synthesizer, the newest whose
   validity check fails is removed with everything queued before it,
   and the synthesizer is cancelled. Speech for an object stays valid
   while the object is the focus, an ancestor of the focus, or the
   foreground object, or if it never had the focus, and for a menu item
   when the focus has moved to a popup menu; the exact rules are under
   "Expired focus speech" in [Speech](speech.md). This happens on every
   focus change, before anything about the new focus is spoken.
3. If the difference level is 0 or 1 (the top changed), NVDA decides the
   foreground changed: it asks the desktop object for the real foreground
   (`objectInForeground`), falls back to ancestor index 1 if that fails,
   sets it (`api.setForegroundObject`), and executes a synthetic
   `foreground` event on it — foreground announcements are therefore an
   *effect of focus processing*, not a separate platform event.
4. It fires `focusEntered` on each ancestor from the difference level
   down — the "you have entered this container" announcements — before
   the gainFocus event itself runs. Which ancestors actually speak is
   decided by the object model's presentation rules
   ([Focus and the navigator](focus-and-navigator.md)).
5. Tree interceptor bookkeeping: if the focus moved into or out of a
   document with a tree interceptor, `event_treeInterceptor_loseFocus` /
   `gainFocus` fire (browse mode entry/exit; [Browse mode](browse-mode.md)).

## Foreground windows

NVDA never announces a window because it became the foreground. A
window is spoken only through the two ordinary focus paths.

- A foreground event is a focus on the window. The MSAA handler's
  `processForegroundWinEvent` drops a foreground event when its window
  is no longer the system's foreground window, when the most recently
  queued focus is in that window or a window inside it, and when it
  names exactly the object that is already the focus. These checks run
  in NVDA's main-thread pump, not in the event callback: a starting
  application's window raises its foreground event before it actually
  becomes the foreground window, and filtering in the callback caused
  focus problems when starting applications (NVDA issue 4001). Before
  that check, NVDA holds back the handling of every event after a
  foreground event, for up to two further passes of its event loop,
  until the system's foreground window is the event's window, because
  Windows can report the new foreground window a little late (NVDA
  issue 3831). Otherwise it is
  queued as a `gainFocus` on the window object, and the window is
  announced the way any focused object is. When a control inside the
  window takes focus afterwards, the window is already an ancestor of
  the focus, so it is not spoken again.
- A window that is an ancestor of a new focus is spoken by
  `focusEntered` like any other entered container. A window is
  presentable in the focus ancestry only when it has a name or a
  description: an unnamed or whitespace-named `WINDOW` has the layout
  presentation type (`_get_presentationType` in
  `source/NVDAObjects/__init__.py`), and layout objects are not
  presentable focus ancestors.

`event_foreground`, run from `doPreGainFocus` whenever the top of the
ancestry changes (a focus difference level of 0 or 1), only cancels
speech; its own documentation says it must not speak the object, because
`focusEntered` or `gainFocus` will. It runs whether or not the new
window has a name, so moving to another application always cuts off
speech from the one left behind, before the new window and focus are
announced.

A window that has no name when focus enters it is never announced
later. `event_nameChange` speaks only when the changed object is the
focus itself, so a name arriving on a window that is merely an
ancestor of the focus is silent. When the window is itself the focus,
the name change speaks the new name alone, through
`speakObjectProperties(name=True)`, queued behind current speech
rather than interrupting it, as all NVDA speech is unless something
cancels it.

A note on ordering: nothing guarantees platform events arrive in a sane
order. The MSAA side re-orders and coalesces before queuing (the ordered
winevent limiter, [MSAA and winevent handling](msaa.md)); the queue-time focus tracking plus
cancellable speech handle the remainder (a stale focus announcement is
cancelled rather than spoken). There is no timestamp-based arbitration:
freshness is by queue position and "is a newer focus pending."

## The focus gate: what the default handlers actually speak

The pipeline delivers far more than NVDA announces. The base
`NVDAObject` handlers (`source/NVDAObjects/__init__.py`) apply a final,
easily missed filter: property changes speak only when the changed
object *is the focus*. `event_valueChange`, `event_nameChange`, and
`event_descriptionChange` all skip speech unless
`self is api.getFocusObject()` (braille and vision output still
update). `event_stateChange` widens the gate to focus *ancestors* —
pressing a focused button may flip a state on a container above it,
the [#10890](https://github.com/nvaccess/nvda/issues/10890) case of a
sort button inside a column header. `event_caret` acts only for the
focus object and only when no gainFocus is pending.

The consequence to internalize: a background window's name/value/state
churn that survives `shouldAcceptEvent` (a topmost window, an
explicitly opted-in process) is *still silent by default* — acceptance
filtering bounds the cost of events; the focus gate decides the
speech. Features that need otherwise (progress bars, live text) get it
by overriding these handlers in their behavior mixins
([Object model](object-model.md)), not by loosening the gate.

Two default handlers also *cancel* in-flight speech:
`event_focusEntered` cancels speech and returns without announcing
when the entered container is a menu bar, popup menu, or menu item,
and `event_foreground` cancels speech before the new window is
announced (both in `source/NVDAObjects/__init__.py`) — a large part of
why window switches and menu openings cut stale speech off crisply.

### Selection in a list the focus controls

A search box often keeps the keyboard focus while the user arrows
through suggestions or results in a separate list: the Start menu's
search, and the Settings app's search box. UI Automation links the two
through the focus's ControllerFor relation, which names the elements
the focused control drives. When an element is selected (UIA's
`ElementSelected` event), NVDA looks at the current focus's
ControllerFor elements; if the selected element is inside one of them,
NVDA cancels speech, moves the navigator object to the selected
element, and speaks it exactly as it speaks a focus: name, role (left
out for the roles silent on focus), states, and position, with no
ancestors. The focus itself stays where it was, in the search box, so
typing goes on there. A selection that is not inside a controlled
element is handled as any other selection. (`event_selection` on the
base `NVDAObject`, with the ControllerFor read in the UIA object.)

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
