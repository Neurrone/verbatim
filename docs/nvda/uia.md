# The UIA client

NVDA's UIA front end is `source/UIAHandler/__init__.py` (class
`UIAHandler`), with C++ assistance for event rate limiting
(`nvdaHelper/local/UIAEventLimiter/`). Its defining features: a dedicated
MTA thread, everything prefetched through one cache request, a
per-window "is UIA actually good here" decision, and aggressive event
filtering.

## Threading

All UIA client work happens on one dedicated thread, `MTAThread`
(`UIAHandler.MTAThreadFunc`): it calls
`CoInitializeEx(COINIT_MULTITHREADED)`, creates the `CUIAutomation8`
client object, registers every handler, and then services a small
internal queue; COM delivers event callbacks on UIA's own MTA worker
threads. Handler methods do fast filtering and then
`eventHandler.queueEvent(...)` to the main thread — no announcement
logic runs on UIA's threads. Where available (Windows 11), NVDA opts
into `CoalesceEvents` (duplicate-event batching in UIA itself) and
`ConnectionRecoveryBehavior` (UIA reconnects to crashed providers)
(`MTAThreadFunc`).

## Caching discipline

At startup the handler builds `baseCacheRequest` with the window
handle, control type, name, and the other properties every event
handler will need, plus the Text pattern (`MTAThreadFunc`); every event
registration passes it, so each event arrives with its element's core
properties prefetched, and focus objects are constructed from cached
values without re-round-tripping. Tree walking and searches elsewhere
in the UIA code build purpose-specific cache requests. This is the
pattern NVDA relies on for latency; property access outside a cache is
treated as a bug to hunt.

## Event registration: global plus focus-local

Events NVDA consumes are split into two groups
(`globalEventHandlerGroupUIAEventIds` /
`localEventHandlerGroupUIAEventIds` and the property equivalents):

- The *global* group — focus changed (registered separately and always,
  `AddFocusChangedEventHandler`), window opened, system alert,
  notification, layout invalidated, selection changes, name/value/state
  property changes, active-text-position changed — registered
  tree-wide on the root element as one `EventHandlerGroup` (one
  cross-process registration; a Python-side `FakeEventHandlerGroup`
  emulates the grouping API pre-Windows 11).
- The *local* group — high-frequency text events (text changed,
  caret/text-selection changed, controller-for changes) — registered
  *only on the focused element*, re-registered on every focus change
  (config "Selective event registration" historically; now the
  default). This is NVDA's answer to the cost of tree-wide
  registrations for chatty events.

## The C++ rate limiter

When enabled (default), all handlers are wrapped by a native
rate-limiting proxy created by
`localLib.rateLimitedUIAEventHandler_create`
(`nvdaHelper/local/UIAEventLimiter/`): UIA calls the C++ handler, which
coalesces/deduplicates bursts (per event type and runtime ID, latest
wins) on a background flushing thread before invoking the Python
handlers. Purpose: survive event storms (terminal output, busy web
apps) without the Python-side cost per event. This is the UIA
counterpart of the MSAA ordered winevent limiter.

## Handler-side filtering

Each handler (`IUIAutomationFocusChangedEventHandler_...`,
`IUIAutomationEventHandler_HandleAutomationEvent`,
`IUIAutomationPropertyChangedEventHandler_...`) applies, in order:
startup/shutdown guards; `isNativeUIAElement` — the element must come
from a window NVDA has *decided* is UIA (below), dropping events from
proxied-MSAA elements (those are MSAA's job) and from NVDA's own
process; and per-event special cases (for example, redundant
window-opened suppression, focus events on the desktop dropped). Events
surviving become NVDA events on `NVDAObjects.UIA` objects via
`eventHandler.queueEvent`.

## The per-window UIA decision

`isUIAWindow` / `_isUIAWindowHelper` is the heart of NVDA's mixed-stack
strategy — the function that decides, per window class, whether NVDA
treats a window through UIA or leaves it to MSAA/IA2:

1. Never for NVDA's own process.
2. Always for `goodUIAWindowClassNames`; app modules can force either
   way (`isGoodUIAWindow` / `isBadUIAWindow`).
3. Never for `badUIAWindowClassNames` — the documented scar tissue:
   Win32 common controls whose UIA proxies are worse than their MSAA
   (`SysTreeView32`, `ComboBox`, `Edit`, rich edit variants, progress
   bars, `Button`, …), the IME candidate window (UIA events fight the
   MSAA composition support), Foxit ([#8944](https://github.com/nvaccess/nvda/issues/8944)), and all the Mozilla window
   classes — "IA2 is still better for web content in screen readers
   for now."
4. Office `NetUIHWND` (ribbon) windows: non-UIA for Office ≤ 2013
   (event bugs), UIA for 2016+.
5. Otherwise, ask Windows: `UiaHasServerSideProvider(hwnd)` — a
   blocking cross-process call, run through
   `watchdog.cancellableExecute` because it freezes on hung apps.
6. Even with a native provider: Word documents use UIA only per
   `shouldUseUIAInMSWord` (config three-way: always / when suitable /
   only when injection is impossible — older Office builds had
   unworkable UIA, and the object model is still richer;
   [Office through COM](office-com.md)), and Excel `EXCEL7` prefers the object model
   whenever in-process injection succeeded.

The net effect to remember: *NVDA runs MSAA/IA2 and UIA simultaneously,
per window*, with this function as the referee, and its defaults encode
years of per-control fidelity comparisons.

## Remoted UIA trees: Application Guard

Windows Defender Application Guard (WDAG/MDAG) ran Edge (and Office
documents) inside a lightweight Hyper-V container, with the UI
projected to the host as an RDP RemoteApp window. Accessibility
crossed the VM boundary one way only: **Windows forwarded the guest's
entire UIA tree through the host-side projection process** — nothing
else (no MSAA, no injection, no window hierarchy) crosses. The
forwarding rewrote each element's `processID` to the local host
process (`hvsirdpclient`), but left `nativeWindowHandle` as *guest*
window handles, invalid on the host, and the remote tree was not
parented into the local desktop tree.

NVDA's accommodations (added in nvaccess/nvda PR
[#7600](https://github.com/nvaccess/nvda/pull/7600), still in
`UIAHandler/__init__.py` at the pinned commit): the constants
`WDAG_PROCESS_NAME = "hvsirdpclient"` and
`WDAG_WINDOW_CLASS_NAME = "RAIL_WINDOW"`; the RAIL window is forced
UIA-native ("always native UIA, even if it doesn't report as such" —
the projection window exposes no server-side provider to the probe);
and `getNearestWindowHandle` special-cases WDAG elements — "treated
as being from a remote machine" per its comment — substituting the
*active local RAIL window* for the useless guest handles, so
everything downstream keyed on window handles keeps working. Office
inside Application Guard additionally loses the COM object model
(the cross-VM channel is UIA only), so the Word code falls back to
UIA throughout (`NVDAObjects/window/winword.py` comments;
[Office through COM](office-com.md)).

Status and why this stays documented: MDAG is deprecated and removed
from Windows 11 24H2, so the feature itself is fading — but the
*shape* recurs: any remoted or containerized UI (cloud PC app
streaming, RemoteApp, future isolation tech) presents exactly this
profile — a full-fidelity UIA tree whose process and window
identities are locally meaningless. A client that hard-assumes
window handles are dereferenceable (class-name arbitration, window
hierarchy navigation, per-window verdict caches) needs a
treat-as-remote pathway like NVDA's to survive it.

## Element wrapping

`NVDAObjects.UIA.UIA` wraps an element plus its cache; overlay
selection keys off control type, class name, and UIA properties
(automation ID, framework ID). Identity is the runtime ID array.
Navigation uses tree walkers with the control view by default. UIA
`notification` events map to `event_UIA_notification` with the
activity ID, notification kind, and display string — the modern app
announcement channel. Text is served by `UIATextInfo`
(`source/UIAHandler/utils.py` and `NVDAObjects/UIA`), a *range-based*
(not offsets-based) TextInfo built on the Text pattern
([TextInfo](text-infos.md)).
