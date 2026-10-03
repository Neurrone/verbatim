# verbatim-uia

The UIA client stack (architecture section 4): cache requests, dedicated
threads, and the arbitration probe.

Public API:

- `Uia` — a per-thread client (one COM apartment, one `IUIAutomation`
  instance; nothing COM crosses threads), whose connection timeout is ten
  seconds rather than UIA's default two, so a busy application's read
  waits for its own answer instead of failing or returning UIA's stand-in
  for the window: `focused_element`,
  `element_from_handle`, `element_by_runtime_id`, `base_cache_request`,
  `controlled_descendant` (the selected element, when it is inside an
  element the focus names in its ControllerFor relation), plus the M3
  node-relative operations `ancestor_chain`, `navigate`, and `activate`
  described below. The coclass is `CUIAutomation8`, not the
  older `CUIAutomation`: only the former's objects implement the newer
  client interfaces, and querying `IUIAutomation5` (the notification-event
  registration) on a plain `CUIAutomation` object fails with
  `E_NOINTERFACE`, observed live. NVDA likewise creates `CUIAutomation8`.
- `Uia::ancestor_chain` — the chain of ancestors of an element, outermost
  first, as `NodeSnapshot`s: a per-hop `GetParentElementBuildCache` walk
  over the raw view (one cross-process round trip per ancestor, the walk
  NVDA shipped for years), capped by the caller, and stopped by
  `AncestorStops`: at a window read through the other API, at an ancestor
  the caller already knows (reporting `AncestorWalk::MetKnown`), or at a
  deadline (`AncestorWalk::OutOfTime`). `Uia::within(wait, read)` runs a
  read with a shorter connection timeout, for reads that are only extras. Ancestors that are not
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
  previous sibling, first child, named by a navigation `QueryKind` from
  `verbatim-model`) returning
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
  then `SelectionItem`'s select, answering `ActionName::Invoke` for an
  Invoke and no name otherwise, as NVDA names them; each fetched live since
  activation is an infrequent user action, not something the cache
  prefetches.
- `base_cache_request(client)` — the property set prefetched with every
  event and fetch: name, control type, value, process id, native window
  handle, enabled, focus states, toggle, expand-collapse, and
  selection-item state with their three pattern-availability flags (see
  the implementation note below), and the `NodeDetails` properties —
  `FullDescription` and `HelpText`, `AccessKey` and `AcceleratorKey`
  (both spoken, joined by two spaces, as NVDA joins them),
  `PositionInSet`, `SizeOfSet`, `Level`, and `BoundingRectangle`; plus
  `ClassName` and `IsDialog` (a dialog by NVDA's rule), `IsPassword`,
  `IsRequiredForForm`, `IsDataValidForForm`, and `ValueIsReadOnly` (the
  protected, required, invalid entry, and read-only states; the last two
  read with `GetCachedPropertyValueEx` ignoring their default of true),
  `RangeValueValue` (the rounded value of a control with no `Value`
  pattern, likewise ignoring its default), and `IsContentElement` and
  `IsControlElement` (an ancestor is focus context only when both hold).
  A selected radio button is checked rather than selected, and a
  toggleable element other than a check box or toggle button is
  checkable.
- `FocusRegistration::new(callback)` — the self-contained, desktop-global
  UIA focus registration; drop unregisters and tears down its own thread.
  UIA's focus registration is desktop-global and unscopeable, so exactly one
  exists per process. Under decision D13 the one process that holds it is the
  focus listener, which watches every application at once; the per-application
  pid filter this module once carried is gone with that move (the sealed
  module made the relocation a change of caller, not a rewrite).
- `Registration::new(subscription, scope)` and `retarget(scope)` — one
  subscription type for everything but focus: `Subscription::Properties`
  (a list of property ids, such as `FOCUS_PROPERTIES`: name, value,
  toggle state, enabled, and expand/collapse), `Subscription::Event` (an
  automation event id, such as `SelectionItem_ElementSelected` or
  `MenuOpened`), or `Subscription::Notifications`
  (`IUIAutomation5::AddNotificationEventHandler`, delivering the raising
  element plus kind, processing, display string, and activity id). The
  `Scope` is nothing yet, the subtree of given top-level windows, the whole
  desktop (the subtree of the root element), or exactly given elements.
  Each registration owns its thread, apartment, client, and handler and
  registers with the base cache request; `retarget` hands the new scope to
  that thread, which removes everything its client registered and registers
  again, so the caller never waits on UIA's removal (which waits for
  running callbacks). Elements that fail to resolve are skipped. Dropping a
  registration unregisters and ends its thread. The focus listener holds the
  desktop-wide selection, menu-opened, and notification subscriptions; each
  outpost holds one focus-following property subscription.
- `has_server_side_provider(hwnd)` — the arbitration probe. Sends
  `WM_GETOBJECT` and can block on a hung application, so it is documented
  as callable only from deadline-guarded query threads. Only the window's
  own answer counts: `UiaHasServerSideProvider` reports no provider when
  a busy window does not answer in time, so a "no" slower than a second
  is followed by waiting for the window to process a `WM_NULL` and asking
  once more, all within eight seconds. `probe_server_side_provider`
  answers `None` for a window that never answered, which the outpost
  reads through MSAA for the event at hand without keeping that as the
  window's answer, as NVDA treats a cancelled probe.
- `console_reports_formatting(hwnd)` and `is_windows_forms(hwnd)` — the
  checks NVDA makes on a window with a provider before using it: whether a
  console's text area reports one visible range with its font (the
  complete console provider of current Windows), and whether a window's
  UIA framework is `WinForm`. Each answers `None` when the application did
  not answer in time, so the outpost asks again rather than keep the
  verdict.
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
- `NodeIdRegistry` — maps UIA runtime IDs to stable `NodeId`s and keeps the
  live element behind each; takes an injected shared counter so the UIA and
  MSAA registries in one outpost never hand out the same id. Nodes stay
  until the outpost releases them with `retain` (the clear-at-2048 element
  cache is gone), and `take_touched` reports the nodes issued or looked up
  since the last call. `init_mta()`, role and state mapping in
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
