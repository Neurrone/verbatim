# MSAA and winevent handling

NVDA's MSAA front end (`source/IAccessibleHandler/`) is where the fire
hose of winevents is filtered, coalesced, re-ordered, and turned into NVDA
events. It embodies more accumulated real-world lore than any other part
of NVDA; the filtering rules below are each a scar with an issue number.

## Registration and the callback

`internalWinEventHandler.initialize` registers one *out-of-context*
winevent hook per event ID in `winEventIDsToNVDAEventNames` — the map
that defines which winevents NVDA consumes at all: focus and foreground,
show/hide/destroy (but not reorder), name/value/
state/description changes, selection events, menu start/end, alert,
scrolling start, switch end (Alt+Tab), desktop switch, caret location
changes, live region change, plus the IA2 extension events (caret moved,
document load complete, attribute changed, page changed). Everything
else never reaches NVDA.

The callback (`winEventCallback`, same file) runs on the thread that
pumps NVDA's message loop and does cheap triage only:

- Drops object IDs at or below `OBJID_ALERT` (sound, native OM, etc.),
  and all `locationChange` events except the caret's.
- Routes `EVENT_OBJECT_DESTROY` immediately to focus/desktop
  bookkeeping (dead-object cleanup) rather than the queue.
- Normalizes `OBJID_WINDOW` child 0 to `OBJID_CLIENT` ("better
  reporting"), and substitutes the desktop window for null/invalid
  windows on menu-end/switch events (they often arrive windowless).
- Per-window-class hacks: Excel's `EXCEL7` child events are dropped
  (UIA-proxied duplicates; querying them froze Excel 2016 for seconds),
  IME candidate-host menu events are dropped, `MSNHiddenWindowClass` is
  ignored entirely (touching it killed Messenger, [#677](https://github.com/nvaccess/nvda/issues/677)), foreground
  events from `Progman`/`Shell_TrayWnd` are dropped.
- `EVENT_SYSTEM_FOREGROUND` sets a *defer*: NVDA waits until
  `GetForegroundWindow()` actually reports the new window (up to a few
  pump cycles, `_shouldGetEvents`, [#3831](https://github.com/nvaccess/nvda/issues/3831)) before processing any events,
  because the winevent routinely precedes the state it announces.
- Whatever survives goes into the limiter, and a core pump is requested
  (immediate for focus events).

## The ordered winevent limiter

`orderedWinEventLimiter.OrderedWinEventLimiter` (own file) is the flood
control. Per flush (one core pump): focus events are capped at
`maxFocusItems` = 4 (oldest evicted); *all other* events are deduplicated
by (event, window, object, child) with a duplicate re-timestamped to now
(latest position wins); menu events keep only the single most recent
(`_lastMenuEvent`); and each source thread is allowed at most
`MAX_WINEVENTS_PER_THREAD` = 10 generic events per flush (the rest are
dropped, with the count logged) — the direct defense against one busy
application starving the pump. Events flush in timestamp order. The
current focus object's own events bypass all filtering
(`alwaysAllowedObjects` in `pumpAll`).

## Pump-time processing

`IAccessibleHandler.pumpAll` drains the limiter each core cycle:

- `shouldAcceptEvent` (foreground gating; [Event handling](events.md)) is applied here,
  *not* in the callback (doing it early broke app-launch focus, [#4001](https://github.com/nvaccess/nvda/issues/4001)).
  Show/hide events on the caret bypass it and become caret events.
- Focus and foreground winevents are batched, and only the *most recent
  processable one* is turned into an NVDA gainFocus (walk backward until
  one succeeds — `processFocusWinEvent` / `processForegroundWinEvent`).
- Menu start/end and switch-end events are held as a *fake focus* of
  last resort: if no real focus event validated this cycle, NVDA
  fabricates focus on the active menu item / switcher
  (`processMenuStartWinEvent`, `processFakeFocusWinEvent`) — menus
  frequently fire no real focus event.
- The rest go through `processGenericWinEvent`, which resolves the
  triple to an `IAccessible` (`accessibleObjectFromEvent`), wraps it
  (`NVDAObjects.IAccessible`), and queues the mapped NVDA event; caret
  show/hide/location become `caret` events.

Focus winevents get further validity checks in `processFocusWinEvent`
(same file): events on windows that are not really focused per
`GetGUIThreadInfo`, `Shell DocObject View` chrome, or console windows
handled by the console module are rejected; a `SysListView32` child
focus while the parent claims focus is accepted only with state checks;
and `processFocusNVDAEvent` refuses objects whose `shouldAllowIAccessibleFocusEvent`
is false (an overlay hook — objects can veto focus claims, used heavily
where controls lie about focus).

## Object wrapping and identity

`NVDAObjects.IAccessible.IAccessible` wraps a live `IAccessible` pointer
plus child ID; `getNVDAObjectFromEvent` caches per-(window, objectID,
childID) within a pump. Identity/equality uses IA2 `uniqueID` when
available, else (window, role, childID, location) heuristics. Navigation
via `accNavigate` is wrapped with fallbacks: NVDA cross-checks against
the *window hierarchy* for windowed children (a control's HWND children
are real even when its MSAA says nothing), and per-control overlays
(SysListView32, SysTreeView32, toolbar, …) replace navigation with
control-specific window messages where MSAA lies. The IA2 layer on top
of these objects is [IA2 usage](ia2.md).

## Interaction points to remember

- All the property fetches triggered by pump-time processing are
  synchronous COM calls governed by the watchdog
  ([Main loop and watchdog](main-loop-and-watchdog.md)).
- The limiter trades completeness for liveness *by design*: NVDA
  knowingly drops generic events under load and keeps at most 4
  candidate focus changes — behaviors downstream must tolerate missing
  intermediate states.
- The `event_objectID`/`event_windowHandle`/`event_childID` attributes
  stamped on each object at event time are what later code uses to
  re-resolve or compare event sources.
