# verbatim-uia

The UIA client stack (architecture section 4): cache requests, dedicated
threads, and the arbitration probe.

Public API:

- `Uia` — a per-thread client (one COM apartment, one `IUIAutomation`
  instance; nothing COM crosses threads): `focused_element`,
  `element_from_handle`, `element_by_runtime_id`, `base_cache_request`,
  plus the M3 node-relative operations `ancestor_chain`, `navigate`, and
  `activate` described below. The coclass is `CUIAutomation8`, not the
  older `CUIAutomation`: only the former's objects implement the newer
  client interfaces, and querying `IUIAutomation5` (the notification-event
  registration) on a plain `CUIAutomation` object fails with
  `E_NOINTERFACE`, observed live. NVDA likewise creates `CUIAutomation8`.
- `Uia::ancestor_chain` — the chain of ancestors of an element, outermost
  first, as `NodeSnapshot`s: a per-hop `GetParentElementBuildCache` walk
  over the raw view (one cross-process round trip per ancestor, the walk
  NVDA shipped for years), capped by the caller. Ancestors that are not
  presentable focus context — NVDA's `isPresentableFocusAncestor`,
  ported: layout elements (unknown and pane roles, textless static text,
  nameless windows, property pages, and groupings) plus list items, tree
  items, and editable text — are crossed but never reported, matching
  what NVDA speaks as entered containers regardless of its review-mode
  setting. Deliberately the simplest correct implementation behind this
  method as a seam: M4's remote-operations work replaces the per-hop walk
  with a single batched round trip inside the provider process, so
  callers must depend only on the resulting list.
- `Uia::navigate` — one raw-view tree-walker step (parent, next or
  previous sibling, first child; the `NavigateDirection` enum) returning
  the neighbor's snapshot, with `Ok(None)` as the first-class "no such
  neighbor" outcome distinct from an error. Deliberately the full,
  unfiltered tree: a recorded decision matching NVDA with its simple
  review mode off, the user's baseline (an intermediate revision
  projected NVDA's simple-review filtering here and was reverted). The
  registry caches the live element behind every node as an agile
  reference, so navigation resolves nodes directly instead of re-finding
  them by runtime id (an unscoped desktop-wide `FindFirst` per step,
  before this) — `element_by_runtime_id` remains as the fallback, now
  scoped to a caller-supplied root. Its runtime-id `VARIANT` carries a
  hard-won ownership lesson (commit c1d2d45): `windows`'s `VARIANT` has
  a `Drop` that calls `VariantClear`, which for a `VT_ARRAY` variant
  destroys the wrapped `SAFEARRAY` — so a manually built array variant
  must *not* also be freed with `SafeArrayDestroy`. The double free was
  latent heap corruption that surfaced as a continuous outpost
  crash-respawn loop once M3 made runtime-id lookup per-focus-event.
- `Uia::activate` — NVDA's activation ladder: `Invoke`, then `Toggle`,
  then the legacy `DoDefaultAction` pattern, each fetched live since
  activation is an infrequent user action, not something the cache
  prefetches.
- `base_cache_request(client)` — the property set prefetched with every
  event and fetch: name, control type, value, process id, native window
  handle, enabled, focus states, toggle, expand-collapse, and
  selection-item state with their three pattern-availability flags (see
  the implementation note below), and the `NodeDetails` properties —
  `FullDescription` and `HelpText`, `AccessKey` and `AcceleratorKey`,
  `PositionInSet`, `SizeOfSet`, `Level`, and `BoundingRectangle`.
- `FocusRegistration::new(callback)` — the self-contained, desktop-global
  UIA focus registration; drop unregisters and tears down its own thread.
  UIA's focus registration is desktop-global and unscopeable, so exactly one
  exists per process. Under decision D13 the one process that holds it is the
  focus listener, which watches every application at once; the per-application
  pid filter this module once carried is gone with that move (the sealed
  module made the relocation a change of caller, not a rewrite).
- `PropertyRegistration::new(hwnds, callback)` — name, value, toggle-state,
  enabled, and expand-collapse property changes, scoped to the target's
  top-level windows with subtree scope.
- `SelectionRegistration::new(hwnds, callback)` and
  `NotificationRegistration::new(hwnds, callback)` — the M3 event
  additions, following `PropertyRegistration`'s pattern exactly (own
  thread, apartment, client, and handler; unregister on drop; windows that
  fail to resolve are skipped rather than failing the rest).
  `SelectionRegistration` subscribes `SelectionItem_ElementSelected`;
  `NotificationRegistration` subscribes `AutomationNotification` via
  `IUIAutomation5::AddNotificationEventHandler`, delivering the raising
  element plus kind, processing, display string, and activity id.
- `has_server_side_provider(hwnd)` — the arbitration probe. Sends
  `WM_GETOBJECT` and can block on a hung application, so it is documented
  as callable only from deadline-guarded query threads.
- `nearest_window_handle(element)` — NVDA's `getNearestWindowHandle`:
  resolves the native window handle of `element` itself, or of its nearest
  ancestor that has one, in one cross-process round trip
  (`NormalizeElementBuildCache` against a tree walker whose condition
  excludes every element without a native window handle). If `element`
  already has a window handle the call still works, since `NormalizeElement`
  degenerates to returning the starting element unchanged. Like the probe
  above, this sends a cross-process COM call and can block on a hung
  application; unlike the probe, it is documented as callable from UIA
  event-callback threads specifically because each outpost watches a single
  application (decision D9), so a hang here stalls only that application's
  own outpost, which the recovery ladder already covers — the same trade
  NVDA makes running this same walk on its own UIA event-handler thread.
  Backed by a thread-local `IUIAutomation` instance, walker, and cache
  request, lazily built the first time a given thread calls it and reused
  after that, matching `Uia`'s one-client-per-thread rule so nothing COM
  here crosses a thread boundary.
- `NodeIdRegistry` — maps UIA runtime IDs to stable `NodeId`s; takes an
  injected shared counter so the UIA and MSAA registries in one outpost
  never hand out the same id. `init_mta()`, role and state mapping in
  `map`, plus `map`'s total `notification_kind_from_uia` and
  `notification_processing_from_uia` tables for the notification payload.

Implementation note on default values: UIA returns default values for
properties on elements that do not support them, rather than an error, and
every mapping here has to treat the default as "not reported". For pattern
state properties — `ToggleState` reads as Indeterminate on a plain pane,
which briefly made everything announce "half checked" — the state rebuild
gates on the cached `IsTogglePatternAvailable`,
`IsExpandCollapsePatternAvailable`, and `IsSelectionItemPatternAvailable`
flags and only interprets those properties when the pattern is genuinely
there (selection-item availability itself maps to `Selectable`, mirroring
MSAA's selectable bit). For the `NodeDetails` properties the defaults are
the empty string and zero, both mapped to `None` — including a genuine
`0` answered for `PositionInSet`/`SizeOfSet`/`Level` by an hwnd-hosted
root element through its host provider, where a pure fixture element just
leaves the variant empty. The event handlers are COM objects generated by
`windows-core`'s `#[implement]` macro, with the macro's generated glue
wrapped in a small module that scopes lint allowances to generated code
only.
