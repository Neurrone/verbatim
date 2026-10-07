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
  described below, `children_with(element, properties)` (an element's
  raw-view children with `properties` cached, in one `BuildUpdatedCache`
  whose scope takes in the children, for a dialog's own text), and the
  local helpers `cache_request(properties)`,
  `raw_view_walker`, `control_view_walker`, `root_element`, and
  `property_condition`. Every one of
  them is safe to call. The coclass is `CUIAutomation8`, not the
  older `CUIAutomation`: only the former's objects implement the newer
  client interfaces, and querying `IUIAutomation5` (the notification-event
  registration) on a plain `CUIAutomation` object fails with
  `E_NOINTERFACE`, observed live. NVDA likewise creates `CUIAutomation8`.
  Every client, including the one made for UIA's first-time setup, also
  turns on `IUIAutomation6`'s event coalescing (UIA drops an event that
  duplicates one still waiting for this client) and connection recovery
  (UIA adjusts its waits for a provider that stopped answering), as NVDA
  does whenever Windows has them; Verbatim's minimum, Windows 11 24H2,
  always does, so there is no check. Neither changed a call count or a
  wait measured against mockapp (`docs/performance.md`, "Newer UIA
  features").
  Every client in the crate, and the provider probe, first wait for UIA's
  first-time setup, which creates a client and builds a cache request
  from it once, under a lock, on a thread of its own (so a thread that only
  probes never joins COM), with the multithreaded apartment kept for the
  life of the process so the setup is not undone:
  while UIA's first-time setup is still running on one thread, a cache
  request built on another fails with `E_FAIL`, which made a mockapp test
  fail about one run in five (`tests/first_use.rs` races six threads in a
  fresh process).
- `Uia::ancestor_chain` — the chain of ancestors of an element, outermost
  first, as `NodeSnapshot`s: a per-hop `GetParentElementBuildCache` walk
  over the raw view (one cross-process round trip per ancestor, the walk
  NVDA shipped for years), capped by the caller, and stopped by
  `AncestorStops`: at a window read through the other API, at an ancestor
  the caller already knows (reporting `AncestorWalk::MetKnown`), or at a
  deadline or a hop that UIA's transaction timeout ended
  (`AncestorWalk::OutOfTime`); any other failed hop is the root. `Uia::within(wait, read)` runs a
  read with a shorter connection timeout, for reads that are only extras;
  verified with `verbatim-uia-rops`'s stall test, the connection timeout
  does not bound a call on an element already fetched, which UIA's
  process-wide transaction timeout (20 seconds by default) does, so it
  limits only reads that connect to a provider anew. Ancestors that are not
  presentable focus context — NVDA's `isPresentableFocusAncestor`,
  ported: layout elements (unknown and pane roles, textless static text,
  nameless windows, property pages, and groupings) plus list items, tree
  items, and editable text — are crossed but never reported, matching
  what NVDA speaks as entered containers regardless of its review-mode
  setting. `Uia::ancestor_chain_from(parents, registry, stops)` gives the
  same result from ancestors already fetched, nearest first, with their
  caches filled: what `verbatim-uia-rops`'s `focus_ancestry` returns after
  reading the same raw-view ancestors in one round trip inside the
  provider process. The outpost uses that for a UIA focus and keeps the
  per-hop walk for windows read the classic way.
- `Uia::selected_element(element, cache)` — the first element of a
  selection container's current selection, rebuilt with `cache`, or
  `None`. One `BuildUpdatedCache` on the container caches both
  `SelectionPattern2`'s `FirstSelectedItem` property (ignoring its
  default) and the `Selection` pattern object, as NVDA uses the newer
  pattern where the provider has it: two calls with the item's rebuild,
  and for a provider without `SelectionPattern2`, whose property reads
  "not supported", three (`GetCurrentSelection` on the cached pattern
  between), the count the `Selection` pattern alone took.
  Any failure, a missing pattern, or an empty selection is `None`, so the
  focus is still reported, without a selected child;
  `Uia::selected_child` maps it to a snapshot, and `verbatim-uia-rops`'s
  classic focus ancestry uses it as it is.
- `Uia::navigate` — one raw-view tree-walker step (parent, next or
  previous sibling, first child, named by a navigation `QueryKind` from
  `verbatim-model`) returning
  the neighbor's snapshot, with `Ok(None)` for "no such neighbor". A step
  that fails because the element is gone (`element_is_gone`: UIA's element
  not available, or a disconnected provider) is an error, which the outpost
  answers as "gone"; any other failure reads as no neighbor, as NVDA's
  tree-walker failures do. Deliberately the full,
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
  crash-respawn loop once M3 made runtime-id lookup per-focus-event. The
  variant is now built by `InitVariantFromInt32Array`, so no raw array is
  ever handled, and reading a runtime id goes through one helper that owns
  the returned array. A search that finds nothing is `Ok(None)`; a search
  that fails is an error.
- `Uia::activate` — NVDA's activation ladder: `Invoke`, then `Toggle`,
  then `SelectionItem`'s select, answering `ActionName::Invoke` for an
  Invoke and no name otherwise, as NVDA names them; each fetched live since
  activation is an infrequent user action, not something the cache
  prefetches.
- `cached_properties(fetches)` and `cache_request_for(client, fetches)`
  (also `Uia::cache_request_for`) — the base set without the properties of
  the details the active theme reports as off (`FullDescription` and
  `HelpText`, `AccessKey` and `AcceleratorKey`, `PositionInSet` and
  `SizeOfSet`, and `Level`), in the same order; with every detail wanted
  it is exactly `CACHED_PROPERTIES`.
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
  read with `GetCachedPropertyValueEx` ignoring their defaults),
  `RangeValueValue` (the rounded value of a control with no `Value`
  pattern, likewise ignoring its default), `IsValuePatternAvailable` and
  `IsRangeValuePatternAvailable` (which gate `ValueIsReadOnly` and
  `RangeValueValue`, because a cache filled by a remote operation stores
  their defaults, read-only and zero, where a local cache stores "not
  supported"), and `IsContentElement` and `IsControlElement` (an ancestor
  is focus context only when both hold). The list is public as
  `CACHED_PROPERTIES`, so `verbatim-uia-rops` caches the same set, and
  `runtime_id(element)` reads an element's runtime id.
- `map::with_legacy_checked_state(element, node)` — a menu item that no
  pattern makes checkable (`map::wants_legacy_checked_state`) is checkable
  and checked when its legacy MSAA state (`LegacyIAccessibleState`, read
  ignoring its default) has the checked bit, as NVDA 2027.1 reads Windows
  Forms menu items. The state is read live, one counted UIA call, and only
  for such a menu item, as NVDA reads it lazily for its menu item class:
  caching it for every element made UIA ask every provider for the
  `LegacyIAccessible` pattern, roughly doubling a focus change's provider
  work. The outpost applies it to the focus it reports, the focus-now
  answer, and a navigation step's neighbor; `map::add_legacy_checked_state`
  is the rule alone, for tests.
  A selected radio button is checked rather than selected, and a
  toggleable element other than a check box or toggle button is
  checkable.
- `ElementExt`, `WalkerExt`, and `elements_of` — the safe wrappers. The
  `windows` crate marks every COM method `unsafe` only because its
  bindings are generated; an interface value is a counted reference to a
  live object, and a gone provider or unsupported property comes back as
  an error. So each wrapper method holds one documented `unsafe` call and
  is safe to call. `ElementExt`, on `IUIAutomationElement`, has the cached
  reads (`cached_value`, `cached_value_ignoring_default`, `cached_i32`,
  `cached_bool`, `cached_optional_bool`,
  `cached_f64`, `cached_string`, `cached_bounding_rectangle`,
  `cached_control_type`, `cached_framework_id`), which are local, and the
  live calls (`current_control_type`, `current_i32_ignoring_default`,
  `has_keyboard_focus`,
  `build_updated_cache`, `current_pattern`, `controller_for`,
  `find_first`, `find_first_build_cache`), each of which counts one UIA
  call. `WalkerExt`, on `IUIAutomationTreeWalker`, has `parent`,
  `next_sibling`, `previous_sibling`, `first_child`, and `normalize`, each
  built with a cache and counted. `elements_of` reads an element array,
  locally. The pattern methods the crate calls (a selection's current
  selection, invoke, toggle, select, a text pattern's visible ranges, and
  a range's attribute) are crate-private wrappers of the same kind. The
  snapshot mapping, the walks, navigation, activation, and the provider
  checks are written against these, so they are safe code; `unsafe`
  remains only in the wrapper module (`element.rs`), the `VARIANT` and
  `SAFEARRAY` helpers (`com.rs`), the event handler registrations, the
  provider probe's window messages, client creation, and apartment setup.
- `text` (milestone M4) — the same kind of safe wrappers over the text
  pattern, which the outpost's text protocol reads with:
  `text_pattern(element)` fetches a node's `TextPattern2` where the
  provider has it (the caret's own range) and else its `TextPattern`, one
  UIA call or two; `TextPatternExt` on a pattern has `selection` and
  `document_range`; `caret_range(pattern2)` is `GetCaretRange`; and
  `TextRangeExt` on a range has `clone_range`, `compare_endpoints`,
  `expand`, `move_by`, `move_endpoint_to` (by another range's end),
  `move_endpoint_by_unit` (both moves' counts negative for a backward
  move whatever sign the provider gave, `signed_move`, as NVDA corrects
  them), `text` (UTF-16, up to a limit), `select`,
  `find_text` (`FindText`, case sensitive, forward or backward, `None`
  when the provider returns no range),
  `bounding_rectangles`, and `culture` (the `Culture` attribute as a BCP 47
  tag through `LCIDToLocaleName`, `None` when the range mixes languages or
  the provider does not say; `locale_name` makes the tag from a locale
  id, which `verbatim-uia-rops` uses for the `Culture` its programs
  read), `language` (the same read as a `Language`: one tag, UIA's
  "mixed" answer told apart with `is_mixed`, or no language, so a caller
  reading several units asks once and unit by unit only when they
  differ), and `attribute` (any text attribute's raw
  `VARIANT`, UIA's "not supported" or "mixed" sentinel included, for
  milestone M4's formatting), and `attributes`, several attributes' values
  in the order asked in one call (`IUIAutomationTextRange3`'s
  `GetAttributeValues`, NVDA's bulk attribute fetch), falling back to one
  `attribute` call each where the range has no `IUIAutomationTextRange3`
  or that call fails, with a failed attribute read as "not supported" (an
  empty `VARIANT`), as NVDA reads it; a gone provider is still an error.
  The `variant_*` readers (`variant_string`,
  `variant_i32`, `variant_i32_array` for a range's annotation types, a
  single integer or an array of them, `variant_f64`, and
  `variant_optional_bool`) read a value and give `None` for a sentinel or
  another type; `is_not_supported` and `is_mixed` tell the two sentinels
  apart, comparing with UIA's own objects in this process. `Endpoint` names a range's start or end, and
  `uia_text_unit` maps a model `TextUnit` to UIA's, `None` for the
  sentence, which UIA does not have. Every method is a cross-process call,
  counted once: a text range is a provider object in the application, so
  even copying one is a round trip.
- `map::is_terminal_class(class)` — Windows Terminal's text control
  (`TermControl`) and the one embedded in .NET applications
  (`WPFTermControl`) are terminals, by their UIA class as NVDA recognizes
  them, never by a window's title; the snapshot mapping gives them
  `Role::Terminal`. The console host's text area is recognized by its
  window, in the outpost.
- `FocusRegistration::new(callback)` — the self-contained, desktop-global
  UIA focus registration; drop unregisters and tears down its own thread.
  UIA's focus registration is desktop-global and unscopeable, so exactly one
  exists per process. Under decision D13 the one process that holds it is the
  focus listener, which watches every application at once; the per-application
  pid filter this module once carried is gone with that move (the sealed
  module made the relocation a change of caller, not a rewrite).
- `Registration::new(subscriptions, scope)` and `retarget(scope)` — one
  subscription type for everything but focus: `Subscription::Properties`
  (a list of property ids, such as `FOCUS_PROPERTIES`: name, value,
  toggle state, enabled, and expand/collapse), `Subscription::Event` (an
  automation event id, such as `SelectionItem_ElementSelected` or
  `MenuOpened`), `Subscription::Events` (several automation event ids
  through one handler whose callback receives the event id, such as a text
  control's `Text_TextSelectionChanged` and `Text_TextChanged`), or
  `Subscription::Notifications` (delivering the raising element plus
  kind, processing, display string, and activity id), or
  `Subscription::ActiveTextPosition` (`IUIAutomation6`'s active text
  position changed event, delivering the raising element and the range now
  active, when the event carries one). The
  `Scope` is nothing yet, the subtree of given top-level windows, the whole
  desktop (the subtree of the root element), or exactly given elements.
  A registration takes any number of subscriptions and registers them as
  one event handler group (`IUIAutomationEventHandlerGroup`, from
  `IUIAutomation6`), as NVDA registers its handlers: each handler is added
  to the group, which is local, and the group is registered on each
  element of the scope with one `AddEventHandlerGroup` call. Each
  registration owns its thread, apartment, client, and handlers and
  registers with the base cache request; `retarget` hands the new scope to
  that thread, which removes everything its client registered and registers
  the group again, so the caller never waits on UIA's removal (which waits
  for running callbacks). Elements that fail to resolve, or on which the
  group cannot be registered (an element that has gone, which NVDA also
  logs and passes over), are skipped. `settle()` waits until every move
  asked for before it has been made, for a test that measures what the
  moves cost an application. Dropping a registration unregisters
  and ends its thread. The focus listener holds one registration, the
  desktop-wide selection, menu-opened, and notification subscriptions as
  one group, where it held three registrations, each with its own thread
  and client, before; each outpost holds one focus-following property
  subscription and one focus-following subscription to a text focus's
  caret and text changes and active text position changes, one group.
  Registering the listener's group took 9.9 ms at
  the median against mockapp where its three registrations took 18.5 ms
  (`docs/performance.md`, "Event handler groups").
- `has_server_side_provider(hwnd)` — the arbitration probe. Sends
  `WM_GETOBJECT` and can block on a hung application, so it is documented
  as callable only from deadline-guarded query threads. Only the window's
  own answer counts: `UiaHasServerSideProvider` reports no provider when
  a busy window does not answer in time, so a "no" slower than a second
  is followed by waiting for the window to process a `WM_NULL` and asking
  once more, all within eight seconds. `probe_server_side_provider`
  answers `None` for a window that never answered, which the outpost
  reads through MSAA for the event at hand without keeping that as the
  window's answer, as NVDA treats a cancelled probe. `probe(hwnd)` gives
  the same answer as a `Probe`, with how many times the window was asked
  (1, or 2 when UIA gave up on a busy window first), so a test can show
  which way the answer came; `PROBE_BUDGET` is the eight seconds. The
  probe waits for UIA's first-time setup in the process (`ensure_ready`)
  before it asks: one racing that setup answered "no provider" at once for
  a window that has one, about one run in five of two probes started
  together in a fresh process.
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
  degenerates to returning the starting element unchanged. Its walker, and
  the client behind the console and Windows Forms checks above, are kept
  per thread; `release_thread_state()` drops them, and a thread that used
  them calls it before it exits, since a thread-local destructor runs under
  the loader lock, where COM work must not run. The outpost's worker does.
  Like the probe
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
- `calls` — the count of the cross-process calls this crate makes, kept
  per thread: `calls::count(kind)` counts one, and `calls::take()` returns
  this thread's `CallCounts` since the last take and resets them;
  `calls::peek()` returns them without resetting, so a step of an entry
  (a terminal's tail read) can log its own calls by the difference. Every
  call that reaches the application is counted where it is made, inside
  the client method or wrapper that makes it: the
  `*BuildCache` fetches and tree-walker steps, `BuildUpdatedCache`,
  `FindFirst`, `CurrentControllerFor`, pattern fetches and methods,
  `NormalizeElementBuildCache`, and the provider checks' text reads, as UIA
  calls; the arbitration probe's `UiaHasServerSideProvider` and its
  `WM_NULL` wait, as window messages. Cached reads, `GetRuntimeId`, and
  creating clients, conditions, walkers, and cache requests are local and
  not counted, nor are event subscriptions (`docs/performance.md`, "What
  counts as a call"). The outpost's worker takes the count around each
  entry it handles.
- `NodeIdRegistry` — maps UIA runtime IDs to stable `NodeId`s and keeps the
  live element behind each; takes an injected shared counter so the UIA and
  MSAA registries in one outpost never hand out the same id. Nodes stay
  until the outpost releases them with `retain` (the clear-at-2048 element
  cache is gone). They are not dropped when their window is destroyed: a
  runtime id has no documented structure naming its window. A node Core
  still holds after its window closed (the navigator left there) is
  therefore the node for any new element given the same runtime id, which
  needs Windows to reuse the same window handle value meanwhile; NVDA, which
  compares elements by runtime id and keeps its navigator indefinitely, has
  the same exposure, and `take_touched` reports the nodes issued or looked up
  since the last call. A runtime id is unique only among live elements: an
  application can give a dead element's id to a new one (File Explorer
  did, going back from a subfolder, found 2026-10-07). `reissue` forgets
  the node a runtime id names, its element and reverse entry included, so
  the next lookup mints a new node and a query for the old one answers
  gone; the outpost calls it before reporting a focus whose runtime id
  names a node whose element no longer has the keyboard focus, or cannot
  be read. `init_mta()` joins the multithreaded apartment,
  failing on a thread already in a single-threaded one; each call adds an
  initialization that is never undone, since the MTA is pinned for the
  process's life. Role and state mapping in
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
