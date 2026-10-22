# verbatim-uia

The UIA client stack (architecture section 4): cache requests, dedicated
threads, and the arbitration probe.

Public API:

- `CONNECTION_TIMEOUT` and `TRANSACTION_TIMEOUT` — two seconds, UIA's
  connection timeout (a lookup by handle, the focused element's lookup,
  and the waits of UIA's own focus tracking), and five seconds, its
  transaction timeout (each call on a held element), for the whole
  process. Both are settings of the process, not of
  a client (experiment E1 in `docs/design/focus-pipeline.md`), so UIA's
  first-time setup sets them once, through its own client, and creating a
  client changes neither (measured 2026-10-11: a new client, on the same
  thread or another, reads whatever was set last). Each bounds one call
  into a provider, not an operation: a search makes many calls and waits a
  full timeout for each that is not answered, which is why
  `BoundedClient` exists. Two seconds is UIA's default and NVDA's, which
  bounds how long a hung window named in a focus `WinEvent` holds the
  listener's UIA events (`docs/design/listener-without-connections.md`,
  section 5); five seconds is under the outpost worker's ten-second
  deadline and over the two to three seconds a starting application
  takes to answer a call.
- `BoundedClient` — a client on a thread of its own for an operation made
  of many provider calls that needs a deadline of its own (a `FindFirst`
  search, a dialog's text). `run(deadline, operation)` queues the
  operation, which captures and answers only `Send` values (UIA objects as
  agile references), and waits for its answer; past the deadline it
  answers `Unanswered::DeadlinePassed` and abandons the thread, which
  finishes its call unheard and ends, and the next `run` starts a new
  thread and client. A UIA call cannot be cancelled, so abandonment is the
  bound, as it is for the outpost's worker. The calls an operation made
  and failed cached reads an operation made are added to the waiting
  thread's counts (`calls::add`, `calls::add_failed_reads`) when it
  answers.
  `close()` waits for every thread, abandoned ones included; `abandoned()`
  counts them.
- `Uia` — a per-thread client (one COM apartment, one `IUIAutomation`
  instance; nothing COM crosses threads), each of whose calls waits up to
  `CONNECTION_TIMEOUT` to connect to the application and
  `TRANSACTION_TIMEOUT` for its answer. Every fetch takes a shape's
  `Request` and returns its `Cached` (the shape module, below), or a
  `Live` when nothing of the element is read: `window_element(hwnd,
  request)` (a window's element, `ElementFromHandleBuildCache`, for an
  outpost's own application's window with a provider; refused, by the
  proxy rule, when only `UIAutomationCore.dll`'s own proxies serve the
  element returned, as a lookup of an application busy past the
  connection timeout returns, read from the provider description cached
  with the lookup, an empty one counting as answered; the request with
  the description added is built once per `Request` and kept with it,
  and the count of a window's refusals in a row, kept for the logs, is
  dropped when a lookup of it is answered or, for a window destroyed
  meanwhile, at the next refusal),
  `any_window_element(hwnd, request)` (the same lookup without the proxy
  rule, for a window with no provider, whose element is UIA's proxies by
  nature),
  `request_fixed::<S>()` (the request of a shape with no gates; local),
  `request::<S>(gates)` (the request of a shape for the gates given;
  local), `focused(request)` (the focused element as a shape, under the
  proxy rule as `window_element` is),
  `window_live(hwnd)` (a window's element with no cache, for a caller
  that reads nothing of it, only searches under it: `ElementFromHandle`,
  one call), `answered_window_live(hwnd)` (the same for an outpost's own
  application's window with a provider, under the proxy rule, the lookup
  caching only the provider description: one call), `children(parent, request)` (`parent`'s raw-view children,
  each filled by `request`, in one `BuildUpdatedCache` on a copy of the
  request scoped to the children alone in the raw view, as NVDA fetches
  children, so `parent` itself is filled with nothing; a dialog's own
  text is gathered this way),
  `find_by_runtime_id(root, runtime_id, request)` (a search under `root`,
  the fallback when no element is kept), `find_child_by_runtime_id(parent,
  runtime_id, request)` (a search of `parent`'s children only, which tells
  the outpost whether a selected item is a child of the focus) and `controlled(focused,
  runtime_id, request, current_instead)` (the selected element, when it is inside an
  element the focus names in its ControllerFor relation, else, with
  `current_instead`, the first
  controlled element's first selected element unless `runtime_id` names
  that element; the classic
  implementation behind `verbatim_uia_rops::controlled_selection`, which
  answers with it the runtime ids of the elements the relation named),
  plus
  the M3 node-relative operations `ancestors`, `neighbor`, and `activate`
  described below, `tree(root, request, registry, max_depth, max_nodes)`
  (a tree dump), and the local helpers
  `raw_view_walker`, `control_view_walker`, `root_element`, and
  `property_condition`, and `compare_elements` (UI Automation's
  `CompareElements`, counted as one call, for the focus listener's log of
  whether two focus senders are one element). Every one of
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
  features"). Since 2026-10-10 every client made by `Uia::new` also tells
  UIA's proxies never to raise, from an application's `WinEvent`s, the
  UIA events Verbatim subscribes to: property changes of the
  focus-following properties, an element selected, a menu opened, a
  notification, and a text focus's caret, text, and active text position
  (the `proxy` module, as NVDA's `ignoreWinEventsMap`, NVDA issue 7345).
  Focus changes keep their mapping, as in NVDA. Each proxy entry whose
  mapping changed is put back in the client's proxy factory mapping, and
  a mapping is cleared with a null array, since UIA refuses an empty one.
  Every client in the crate, and the provider probe, first wait for UIA's
  first-time setup, which creates a client and builds a cache request
  from it once, under a lock, on a thread of its own (so a thread that only
  probes never joins COM), with the multithreaded apartment kept for the
  life of the process so the setup is not undone:
  while UIA's first-time setup is still running on one thread, a cache
  request built on another fails with `E_FAIL`, which made a mockapp test
  fail about one run in five (mockapp's
  `tests/uia_bugs/a_cache_request_fails_while_uias_first_time_setup_runs_on_another_thread.rs`
  races six threads in each of four fresh processes).
- `Uia::ancestors((element, element_window), request, registry, max_hops, stops)` — the
  chain of ancestors of an element, each read as the `Ancestor` shape,
  outermost first, as `NodeSnapshot`s: a per-hop `GetParentElementBuildCache` walk
  over the raw view (one cross-process round trip per ancestor, the walk
  NVDA shipped for years), capped by the caller, and stopped by
  `AncestorStops`: at a window read through the other API, at an ancestor
  the caller already knows (reporting `AncestorWalk::MetKnown`), or at a
  caller's out-of-time check (`out_of_time`) or a hop that UIA's
  transaction timeout ended (`AncestorWalk::OutOfTime`). The root has no
  parent, which ends the walk as complete; any other failed hop, or one
  that finds no parent, from an element below the root ends it as
  `AncestorWalk::Unknown`, carrying a `FailedHop` (`Error` with the code,
  or `NoParent`), which callers treat as they treat `OutOfTime`, as NVDA
  reads the window hierarchy when its UIA parent read fails. Telling the
  root is two local calls, made when a hop fails, which includes every
  walk that reaches the root, since the root's step to its parent finds
  none. On the event path they are paid only for an MSAA focus whose
  walk crosses into a UIA window and reaches the root, once per such
  walk. The time check is made between hops, each of which UIA ends
  after `TRANSACTION_TIMEOUT`. The walk never steps across the process boundary:
  before each hop from an element whose own window (`element_window` for
  the first, the cached handle after) has a parent window, or visible
  owner, that another process serves (`parent_across_process_boundary`),
  it stops and answers that window as the one to continue through the
  other API. Ancestors that are not
  presentable focus context — NVDA's `isPresentableFocusAncestor`,
  ported: layout elements (unknown and pane roles, textless static text,
  nameless windows, property pages, and groupings) plus list items, tree
  items, and editable text — are crossed but never reported, matching
  what NVDA speaks as entered containers regardless of its review-mode
  setting; a UIA element must also be both a control and content.
  `Ancestor::is_focus_context(snapshot)` is that whole test, public so the
  outpost applies it to the element of a UIA window that an MSAA walk
  crosses into. `Uia::ancestors_from(parents, registry, stops)` gives the
  same result from ancestors already fetched as `Ancestor`s, nearest
  first: what `verbatim-uia-rops`'s `focus_ancestry` returns after
  reading the same raw-view ancestors in one round trip inside the
  provider process. The outpost uses that for a focus-now query and keeps
  the per-hop walk for windows read the classic way. Each ancestor is
  reported as its node, whose description follows the theme's gate,
  while the layout judgment reads the ancestor's own description, fetched
  whatever the theme, as NVDA's `presentationType` reads it whatever its
  setting (decision 4 of `docs/design/uia-shapes.md`).
  `Uia::ancestor_chain_from_parts(parents, registry, stops)` does the same
  for ancestors another process read and sent as their parts
  (`AncestorParts`: the cached parts, the own window handle, and whether
  UIA counts the element both a control and content), minting each node
  from its runtime id with no element kept: the outpost reports a UIA
  focus's ancestors this way from the focus listener's read, since stage
  A step 6 of phase 6 (`docs/design/focus-pipeline.md`, section 12).
  `element_by_name_and_position`, which found a focus whose sender no
  element of the tree is by its name and position, is gone with that
  step: the listener reads such a focus from its own element.
- `Uia::selected(container, request)` — the first element of a
  selection container's current selection, filled by `request`, or
  `None`. One `BuildUpdatedCache` refreshes the container as the
  `SelectionContainer` shape, which holds both
  `SelectionPattern2`'s `FirstSelectedItem` property (ignoring its
  default) and the `Selection` pattern object, as NVDA uses the newer
  pattern where the provider has it: two calls with the item's rebuild,
  and for a provider without `SelectionPattern2`, whose property reads
  "not supported", three (`GetCurrentSelection` on the cached pattern
  between), the count the `Selection` pattern alone took.
  A missing pattern or an empty selection is `Ok(None)`, and so is a call
  that fails in any other way than two: a call the provider did not
  answer within UIA's transaction timeout (`timed_out`), and one that
  finds the container or its selected item gone (`element_is_gone`), are
  errors, never "nothing selected". `Uia::selected_within(container,
  request, out_of_time)` is the same within the caller's time: it asks
  `out_of_time` before each call after the first and answers `None`, the
  selected child unknown, once the time is up; the classic focus
  ancestry calls it. `Uia::selected_node(container,
  request, registry)` reads the item as the `Node` shape and maps it to
  a snapshot, with the same errors; the outpost logs such an
  error and reports the focus without a selected child, and
  `verbatim-uia-rops`'s classic focus ancestry fails with it, as it does
  for a hop that timed out. Pinned against mockapp with `slow` and
  `stall` (`crates/mockapp/tests/remote_ops.rs`,
  `a_selection_read_that_times_out_or_finds_the_list_gone_fails`).
- `timed_out(error)` — whether a failed call ended at UIA's transaction
  timeout (`UIA_E_TIMEOUT`): a busy or stalled application, which has
  said nothing about what was asked.
- `Uia::neighbor(element, request, registry, kind)` — one raw-view
  tree-walker step (parent, next or previous sibling, first child, named
  by a navigation `QueryKind` from `verbatim-model`) returning the
  neighbor, read as the `Node` shape, as a snapshot, with `Ok(None)` for
  "no such neighbor". A step
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
  before this) — `find_by_runtime_id` remains as the fallback, now
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
  that fails is an error. `Uia::tree(root, request, registry, max_depth,
  max_nodes)` walks a subtree the same way, every node read as a
  `TreeDump`, for the control plane's tree dump.
- `Uia::activate` — NVDA's activation ladder: `Invoke`, then `Toggle`,
  then `SelectionItem`'s select, answering `ActionName::Invoke` for an
  Invoke and no name otherwise, as NVDA names them; each fetched live since
  activation is an infrequent user action, not something the cache
  prefetches.
- What a node is read from (the `Node` shape and its groups, below):
  name, control type, value, native window handle, enabled, focus states,
  toggle, expand-collapse, and selection-item state with their three
  pattern-availability flags (see the implementation note below), and
  the `NodeDetails` properties, each fetched only when the active theme
  reports it: `FullDescription` and `HelpText`, `AccessKey` and
  `AcceleratorKey` (both spoken, joined by two spaces, as NVDA joins
  them), `PositionInSet`, `SizeOfSet`, and `Level`; and always
  `BoundingRectangle`, `ClassName` and `IsDialog` (a dialog by NVDA's
  rule), `IsPassword`, `IsRequiredForForm`, `IsDataValidForForm`, and
  `ValueIsReadOnly` (the protected, required, invalid entry, and
  read-only states; the last two read ignoring their defaults),
  `RangeValueValue` (the rounded value of a control with no `Value`
  pattern, likewise ignoring its default), `IsValuePatternAvailable` and
  `IsRangeValuePatternAvailable` (which gate `ValueIsReadOnly` and
  `RangeValueValue`, because a cache filled by a remote operation stores
  their defaults, read-only and zero, where a local cache stores "not
  supported"). An ancestor adds `IsContentElement` and
  `IsControlElement` (it is focus context only when both hold).
  `runtime_id(element)` reads an element's runtime id.
- `map::value_from_selection(role, class_name, value_available)`, and
  `Node::value_from_selection` from a node's cache: whether a node's
  value is its first selected item's name, as NVDA reads it, for a combo
  box without the `Value` pattern, or a list whose class name contains
  `LoopingSelector` (a time picker's) without it.
- `map::with_legacy_checked_state(element, node)` — a menu item that no
  pattern makes checkable (`map::wants_legacy_checked_state`) is checkable
  and checked when its legacy MSAA state (`LegacyIAccessibleState`, read
  ignoring its default) has the checked bit, as NVDA 2027.1 reads Windows
  Forms menu items. The state is read live, one counted UIA call, and only
  for such a menu item, as NVDA reads it lazily for its menu item class:
  caching it for every element made UIA ask every provider for the
  `LegacyIAccessible` pattern, roughly doubling a focus change's provider
  work. The outpost applies it to the focus-now answer and a navigation
  step's neighbor; the focus listener applies `map::add_legacy_checked_state`,
  the rule alone, to the state its remote program read with the focus, or
  reads it live when the focus was read the classic way.
  A selected radio button is checked rather than selected, and a
  toggleable element other than a check box or toggle button is
  checkable.
- `ElementExt`, `WalkerExt`, and `elements_of` — the safe wrappers. The
  `windows` crate marks every COM method `unsafe` only because its
  bindings are generated; an interface value is a counted reference to a
  live object, and a gone provider or unsupported property comes back as
  an error. So each wrapper method holds one documented `unsafe` call and
  is safe to call. `ElementExt`, on `IUIAutomationElement`, has the
  live calls (`current_control_type`, `current_i32_ignoring_default`,
  `current_bool`, `has_keyboard_focus`,
  `build_updated_cache`, `current_pattern`, `controller_for`,
  `find_first`, `find_first_build_cache`), each of which counts one UIA
  call; it has no cached read, since the shape module makes every one.
  `WalkerExt`, on `IUIAutomationTreeWalker`, has `parent`,
  `next_sibling`, `previous_sibling`, `first_child`, and `normalize`, each
  built with a cache and counted. `elements_of` reads an element array,
  locally. The pattern methods the crate calls (a selection's current
  selection, invoke, toggle, select, a text pattern's visible ranges, and
  a range's attribute) are crate-private wrappers of the same kind. The
  snapshot mapping, the walks, navigation, activation, and the provider
  checks are written against these, so they are safe code; `unsafe`
  remains only in the wrapper module (`element.rs`), the `VARIANT` and
  `SAFEARRAY` helpers (`com.rs`), the event handler registrations, the
  provider probe's window message, client creation, and apartment setup.
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
- `HelpTextRefresh` (its `Description` group's `description()`: the full
  description, else the help text) and `SuggestionsRefresh`
  (`reports_suggestions()`: a text pattern, or the automation id
  `SearchTextBox` or `TextBox`; `controls_any`: whether the
  `ControllerFor` relation names any element): what a help text or
  `ControllerFor` change of the focus reads when the outpost handles it,
  in one cache refresh.
- `map::ignores_notifications(class, automation_id)` — a terminal control's and
  the console host's text area's (automation id "Text Area")
  notifications are all ignored, as NVDA's terminal overlays ignore them.
- `map::is_terminal_class(class)` — Windows Terminal's text control
  (`TermControl`) and the one embedded in .NET applications
  (`WPFTermControl`) are terminals, by their UIA class as NVDA recognizes
  them, never by a window's title; the snapshot mapping gives them
  `Role::Terminal`. The console host's text area is recognized by its
  window, in the outpost.
- NVDA's base cache request (thirteen properties and the Text pattern,
  with UIA's default scope and filter) is the `ListenerEvent` shape's
  request with `IsDialog` and `IsTogglePatternAvailable` added, which the
  focus listener's parts carry.
- `Registration::with_parts(parts, scope)`, `retarget(scope)`, and
  `retarget_to(scope, group)` — one
  subscription type for every event: `Subscription::Properties`
  (a list of property ids, such as `FOCUS_PROPERTIES`: name, value,
  range value, toggle state, enabled, expand/collapse, help text, and
  `ControllerFor`), `Subscription::Event` (an
  automation event id, such as `SelectionItem_ElementSelected` or
  `MenuOpened`), `Subscription::Events` (several automation event ids
  through one handler whose callback receives the event id, such as a text
  control's `Text_TextSelectionChanged` and `Text_TextChanged`), or
  `Subscription::Notifications` (delivering the raising element plus
  kind, processing, display string, and activity id), or
  `Subscription::ActiveTextPosition` (`IUIAutomation6`'s active text
  position changed event, delivering the raising element and the range now
  active, when the event carries one), or `Subscription::Focus` (focus
  changes anywhere on the desktop, delivering the focused element with its
  part's cache request). A focus subscription is not part of the groups:
  its handler is registered on the registering thread's client itself
  (`AddFocusChangedEventHandler`) before any group, as NVDA registers its
  focus handler on the client that holds its global group. The two must
  share a client: UIA delivers the focus events Windows Terminal's XAML
  raises from a second identity of its terminal to a client with only a
  focus handler, but not to one that also has a group on the desktop's root
  (`docs/design/focus-pipeline.md`, section 16.8). UIA's focus handler is
  desktop-wide, so the focus listener, which watches every application at
  once (decision D13), is the one process that registers it. `close()` ends a registration for
  good and returns once it is done: every move already asked for is made,
  then the registering thread removes each group the registration
  registered (`RemoveEventHandlerGroup`) and its focus handler
  (`RemoveFocusChangedEventHandler`), each waiting for a callback in
  progress, and releases its objects; moving or settling a closed
  registration does nothing, and dropping one closes it. Each removal is
  tried once: a group a move failed to remove is tried again at the
  close, with an outcome no test shows (`docs/uia-bugs.md`, "A
  registration that times out reports success"), and a group or focus
  handler whose removal fails at the close, or when a registration's
  setting up fails after its focus handler was added, stays live on the
  registering thread's client, calling the closed
  registration's callbacks, until the thread ends and
  `RemoveAllEventHandlers` removes it. A `close()` made on the
  registering thread itself, as when a registration's owner is dropped
  there with another registration's callbacks, is queued and does not
  wait. An outpost closes its registration when it shuts down. No test
  shows a focus handler removed by `close()`: the shutdown tests' counts
  cover groups only, and on mockapp's isolated desktop a desktop-wide
  focus handler receives no focus event and the provider is told of no
  focus advise, so no fixed target can show it. The
  `Scope` is nothing yet, the subtrees of given elements (`Subtrees`), the
  whole desktop (the subtree of the root element), or exactly given
  elements. The caller looks a window's element up itself, on a thread
  that serves the window's application, so the registering thread makes
  no connection by handle.
- A registration registers its subscriptions as an event handler group
  (`IUIAutomationEventHandlerGroup`, from `IUIAutomation6`), as NVDA
  registers its handlers: each handler is added to the group, which is
  local, and the group is registered on each element of the scope with one
  `AddEventHandlerGroup` call. Each subscription is a `Part`, whose
  events' elements arrive with its shape's properties and patterns
  prefetched, so one group can carry a different cache request for each
  handler. A part's fields are private:
  `Part::shaped::<S>(subscription)`, for a shape with no gates, is the
  only way to make one, a part whose request is `S`'s, properties and
  patterns both, and whose
  callbacks receive each event's element as a `Cached<S>`: `Subscription`
  and its callback types (`PropertyCallback`, `ElementCallback`,
  `EventCallback`, `NotificationCallback`, `ActiveTextPositionCallback`,
  `FocusCallback`) are generic over the element type they deliver, the raw
  element by default, and a shaped part's subscription is
  `Subscription<Cached<S>>`, wrapped inside into one that receives the raw
  element, so a registration still holds parts of several shapes. The
  wrapper runs inside the failed-read counting, so a shaped callback's
  reads are counted in `Registration::failed_reads()`. `.extended()` marks
  a part as held only by the extended groups, and `.text_events()` as
  one that delivers text or caret changes. A registration has NVDA's two
  local groups (`UIAHandler/__init__.py`, `_createLocalEventHandlerGroup`):
  `Group::Base`, which holds every part not marked `.extended()`, and
  `Group::Extended`, which holds every part; and a third,
  `Group::ExtendedWithoutTextEvents`, every part but those marked
  `.text_events()`, for an element whose text and caret changes
  reach the caller another way, so UIA is not asked to deliver them. Each
  is built once per registration, on first need, every part with its
  own shape's request, so the provider is asked for no more with each
  event than the part's callbacks read. The
  desktop's root is fetched with the first part's cache request.
- The outpost's one focus-following registration has four parts: the
  focus's property changes shaped as `PropertyEvent` (the base set
  without the details and focus context properties, since a change of
  states carries the whole state set) and its caret event shaped as
  `TextEvent` (process id, native window handle, and class
  name: nothing is spoken from it, and the class name of an element one
  brings, which may become the node's kept element, says whether its text
  is a terminal's), in every group but the one without text events; and
  its text changes and active text position changes, shaped as
  `TextEvent`, in the extended groups alone. The caret event and the text
  changes are marked `text_events`. The focus listener's
  desktop-wide registration has four parts shaped as `ListenerEvent`.
- A move (`retarget` gives the base group, `retarget_to` the group named)
  is handed to the registering thread, so the caller never waits on
  UIA's removal, which waits for running callbacks; so no callback may
  wait on any registration's moves. The thread adds the
  group to each element of the new scope that does not already have it,
  then removes from each element no longer in the scope the group it added
  there (`RemoveEventHandlerGroup`), keeping one whose removal failed for
  the close to try again, as NVDA adds its local group to a new focus and removes it from
  the old one (`UIAHandler/__init__.py` 642 to 756). An element that
  already has the same group, the same element by `CompareElements`, keeps
  it, and no call is made for it, so a move to the element already
  followed costs the application nothing. A move to the same element with
  the other group adds the new group, then removes the old one. Elements
  that fail to resolve, or on which the group cannot be registered (an
  element that has gone, which NVDA also logs and passes over), are
  skipped. Every move is logged at debug level, a move skipped for a
  later one of the same registration with that later move's scope, and a
  move made with the elements it moves from,
  by runtime id and group, the scope and elements it moves to, and each
  call it or a close makes (`AddEventHandlerGroup`,
  `RemoveEventHandlerGroup`, `AddFocusChangedEventHandler`,
  `RemoveFocusChangedEventHandler`) with its result and its elapsed
  milliseconds, at warn level when the call took longer than
  `CONNECTION_TIMEOUT` or was a removal that failed, naming the element by runtime id, which is read
  only when the line is written; the first
  registration `with_parts` makes is logged the same way. The element's
  window is not named: it would take a cached read the caller's element
  is not known to hold, or a call into the application that has just
  failed to answer. `settle()` waits until every move asked
  for before it has been made, for a test that measures what the moves
  cost an application.
- Every registration in a process is made by one thread, the registering
  thread, with one client: the thread, its client, and its place in COM's
  multithreaded apartment are created on the process's first
  registration, and every registration's first registration, moves,
  settles, and closes are made there in the order asked, as Microsoft's
  guidance for UIA clients asks: a client should not add or remove event
  handlers from several threads at once
  ([Understanding Threading Issues](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading)).
  What a registration registers, its parts, groups, and the elements they
  are on, stays its own; a move of one registration queued behind a newer
  move of the same registration is skipped, unless a settle or close of
  that registration comes between them. The thread waits on its commands
  and wakes for nothing else. No call is given a deadline or abandoned:
  UIA ends a registration on an application that does not answer at the
  connection timeout (`docs/uia-bugs.md`, "A registration that times out
  reports success"), and what is asked after it waits for it.
  `with_parts` waits for the first registration however long it takes;
  a desktop-wide registration that advises several such applications
  waits a timeout for each. `RemoveAllEventHandlers` is called only as the
  thread ends, when the process has finished with UIA:
  `release_mta_usage` ends the thread first, and every registration still
  open in the process then hears nothing more, whoever holds it. A later
  registration starts the thread again, once the ending one has ended: a
  registration asked for meanwhile waits for it.
  The focus listener holds one registration, the
  desktop-wide selection, menu-opened, and notification subscriptions as
  one group, where it held three registrations before; each outpost holds
  one focus-following registration, where it held two before
  (`docs/design/focus-pipeline.md`, section 17).
  Registering the listener's group took 9.9 ms at
  the median against mockapp where its three registrations took 18.5 ms
  (`docs/performance.md`, "Event handler groups").
- `raw_probe(hwnd) -> RawProbe` — the arbitration probe, which the
  outposts make on their own application's windows and the focus
  listener on the windows of the focuses it reads
  (`docs/design/listener-without-connections.md`, decision 2): one
  `SendMessageTimeoutW` of `WM_GETOBJECT` with wParam 0 and
  `UiaRootObjectId` sign-extended, with
  `SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT` and
  `RAW_PROBE_TIMEOUT_MS` (3,000 ms), counted as one window message. It can
  block for that long on a busy application, so it is called only from
  deadline-guarded threads. It asks once, as NVDA does, and its answer is
  the answer: a window that does not answer in time, like any answer but
  exactly `0x10000`, is no provider, kept for 500 ms like any other "no"
  (`docs/design/focus-pipeline.md`, section 8). `UiaHasServerSideProvider`,
  which NVDA calls and Verbatim does not, sends two `WM_GETOBJECT`s, first
  with a nonzero wParam and then with wParam 0, and then connects
  (experiment `e39`, `docs/design/focus-pipeline.md` section 20); the raw
  probe sends only the second, with wParam 0, and nothing more. It is not
  a UIA connection, and it answers correctly where a new connection would
  meet another process's queued remote operation (section 3, step 4 of
  that design) and where `UiaHasServerSideProvider` gives a new console
  window a false "no" (`docs/uia-bugs.md`). It runs UIA's first-time
  setup in the process (`ensure_ready`) first, as Verbatim's UIA calls
  do.
  `raw_probe_verdict(sent, answer, last_error)` reads the outcome, a pure
  function: exactly `0x10000` is `RawProbe::Provider`; any other answer is
  `Answered`, 0 being the default window procedure's and other values,
  such as File Explorer's child windows' `0xffffffff80040201`, windows UIA
  also says have none; a failure is `TimedOut` (or judged hung),
  `AccessDenied` (a window of higher integrity), `Gone`, or `Failed` with
  its error. `RawProbe::has_provider` is true only for `Provider`. Nothing
  is done with the answer but compare it. A one-off count of mockapp's
  handles and private bytes over 12,000 probes found no growth without
  bound, but about 2.3 KB kept in the application for each of the first
  1,000 or so probes of a window (`docs/design/listener-without-connections.md`,
  risk 7).
- `provider_process(hwnd)` and `console_host_of(client)` — the process
  that serves a window's UIA, read with local calls: the window's owner by
  `GetWindowThreadProcessId`, except a console window, whose owner by that
  call is the console's client and whose provider is its console host,
  read from the client with `NtQueryInformationProcess`'s
  `ProcessConsoleHostProcess`. An outpost's window filters, its refusal to
  read another application's focused element, and Core's routing of a
  console client's focus-now query decide by it
  (`docs/design/listener-without-connections.md`, section 4.3). A console
  window whose host cannot be read, such as one whose client runs as
  another user, has no provider process. `served_by(hwnd, pid)` is
  whether `pid` serves the window: its provider process, or for such a
  console window whether it is one of `pid`'s threads' windows by
  `EnumThreadWindows`, which goes by the thread that created it, so a
  console host's outpost still recognizes its own console window. The
  tests' console comes from the dev-only crate `verbatim-test-console`.
- `is_top_level_window(hwnd)` — whether `hwnd` is a top-level window,
  its own root (`GetAncestor` with `GA_ROOT`); a local call. The remote
  focus program's walk is complete only when it ends at such a window's
  own element, or at that of a window below the process boundary.
- `parent_across_process_boundary(hwnd)` — the window above `hwnd` in
  UIA's raw view when another process serves it, by
  `provider_process` (a console window being its console host's): its
  parent (`GetAncestor` with `GA_PARENT`), or, for a top-level window,
  its owner (`GetWindow` with `GW_OWNER`) while the owner is visible,
  since UIA's raw view puts an owned window under a visible owner and
  under the desktop otherwise; `None` for a top-level window with no
  visible owner: where the process boundary lies, at which the
  focus listener's walks up stop and the outpost continues the chain
  through MSAA (`docs/design/listener-without-connections.md`, section
  3). Local calls only.
- `console_reports_formatting(hwnd)` — the check NVDA makes on a console
  window with a provider before using it: whether its text area reports
  one visible range with its font (the complete console provider of
  current Windows); the other such check, whether a list view comes from
  Windows Forms, is the arbitration's, from the class name.
  `held_console_reports_formatting(uia, sender, automation_id,
  out_of_time)` is the console check from an event's sender the caller
  holds, with no connection to the window: the focus listener's. The
  sender is the console's text area (its automation id
  `CONSOLE_TEXT_AREA`), whose Text pattern is read from its cache
  (`TextAreaPattern`), or the console window's element, under which the
  text area is found; `out_of_time`, the caller's time, is asked before
  each call. Each answers `None` when the application did not answer in
  time, and the held check also when its sender is gone
  (`UIA_E_ELEMENTNOTAVAILABLE`) or the caller's time ran out, so the
  caller asks again rather than keep the verdict.
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
  (a terminal's screen read) can log its own calls by the difference. Every
  call that reaches the application is counted where it is made, inside
  the client method or wrapper that makes it: the
  `*BuildCache` fetches and tree-walker steps, `BuildUpdatedCache`,
  `FindFirst`, `CurrentControllerFor`, pattern fetches and methods,
  `NormalizeElementBuildCache`, and the provider checks' text reads, as UIA
  calls; the raw probe's `WM_GETOBJECT` as one window message. The calls
  that connect to a window by its handle, `ElementFromHandle`,
  `ElementFromHandleBuildCache` and `GetFocusedElementBuildCache`, are
  also counted as connections
  (`CallCounts::connections`), which the focus listener keeps at zero
  (`docs/design/listener-without-connections.md`, section 2). Cached reads, `GetRuntimeId`, and
  creating clients, conditions, walkers, and cache requests are local and
  not counted, nor are event subscriptions (`docs/performance.md`, "What
  counts as a call"). The outpost's worker takes the count around each
  entry it handles. Beside the calls, each thread counts its failed cached
  reads, apart from `CallCounts` since a failed read is not a call: a
  cached read UIA answers with a failure, for a property or pattern the
  element's cache request did not hold, counted in `ElementExt`'s cached
  reads and in reading cached children (a pattern the element lacks,
  answered with no failure code, is not a failed read).
  `calls::take_failed_reads()` and `calls::peek_failed_reads()` read it as
  `take` and `peek` read the calls. It is gathered per thread into
  counters their owners keep: `Registration::failed_reads()` for the
  callbacks on UIA's threads, the listener's reading threads, and the
  outpost's worker (`docs/design/uia-shapes.md`, section 3.6).
- The shape module (`docs/design/uia-shapes.md`, section 3). A shape is a
  struct whose fields are what one purpose reads of an element; its cache
  request and its read are generated from one field list by a macro
  private to the crate, so a shape's read names exactly the properties and
  patterns its request holds. Its public parts:
  - `Shape`, `Ungated` and `Covers<T>`, sealed traits no other crate can
    implement. `Shape` carries the shape's `PROPERTIES` and `PATTERNS`,
    computed at compile time without repeats, `UNGATED` (the properties
    every request of it holds), `all_on()` (every gate on), and its gates
    type, `()` for a shape with no gates, which is what `Ungated` marks.
    `Covers<T>` says a shape's ungated request holds every property and
    pattern of `T`'s, checked by a compile-time assertion.
  - Gates: a shape declared with gates has a gates struct of its own,
    with a `bool` for each gated property and an `Option` of the group's
    gates for each gated group (`None`: not fetched), and each gated field
    reads as an `Option`, `None` without touching the cache when its gate
    was off. An ungated group's gates nest as they are, so
    `AncestorGates` holds `NodeGates`. A request's properties are the
    union of what its gates turn on, so a property both gated and ungated,
    such as `Ancestor`'s description, is fetched whenever the ungated field
    asks for it. The macro takes gated fields in sections of their own,
    `gated_groups` and `gated_properties`, since a `macro_rules!` macro
    cannot tell a `#[gated]` marker from a field's documentation. The
    outpost maps the theme to the gates (`docs/crates/verbatim-outpost.md`).
  - The shapes, each a plain struct of public fields. The groups:
    `RoleCore` (control type, class name, `IsDialog`,
    `IsTogglePatternAvailable`, native window handle; `role()` maps them as
    every read maps a role, refined for toggle buttons, terminals and
    dialogs), `ValueSet` (the Value pattern's value and the range value
    with its availability; `value(class_name)`), and `States` (the fifteen
    state properties; `states(role)`). `Kept`: the native window handle
    and class name, all any reader of a kept element reads, covered by
    every shape whose elements are kept. The event shapes:
    `ListenerEvent` (NVDA's thirteen base properties, `IsDialog` and
    `IsTogglePatternAvailable`, with the Text pattern prefetched;
    `parts(runtime_id)` gives its role, name and focused state, nothing
    else; `lacked_keyboard_focus()` is true unless its cached
    `HasKeyboardFocus` is `true`, for the focus listener's judgement once a
    read's time is up or its live keyboard focus read has timed out: a
    provider that answers nothing for the property and one whose answer
    fails leave the same UIA default, `false`, in the cache, which mockapp's
    events test `uia_a_focus_events_cache_holds_a_failed_keyboard_focus_as_an_empty_one`
    shows), `PropertyEvent` (`RoleCore`, name, `ValueSet`, `States` and the
    process id, 24 properties; `parts(runtime_id)` gives everything but
    details), and `TextEvent` (process id, native window handle and class
    name). All three cover `Kept`, and `ListenerEvent` covers
    `FocusInput` (control type and native window handle, what the remote
    focus query reads of its input). The fetch shapes: `Node` (`RoleCore`,
    `ValueSet`, `States`, name and bounding rectangle, and four gated
    details, the `Description`, `Shortcut` and `Position` groups and the
    level: 24 to 31 properties; `parts(runtime_id)`, and on
    `Cached<Node>` `snapshot(registry)`, which keeps the element), covering
    `Kept` and `FocusInput`; `TreeDump`, a tree dump's node, which is
    `Node`; `Ancestor` (a `Node`, its own ungated `Description`, and the
    content and control flags: 28 to 33; `is_control_and_content()`,
    `layout_description()`, and `snapshot` on `Cached<Ancestor>`),
    covering `Kept`; `DialogText` (role, value, states, name and an
    ungated description: 25; `parts(runtime_id)`), a dialog's child;
    `SelectionContainer` (`SelectionPattern2`'s first selected item, read
    as an `ElementValue`, and the `Selection` pattern, read as its
    interface); `SelectedItem` (name and keyboard focus, covering
    `Kept`; `name()` without bidirectional controls), an item found among
    a focused combo box's children as it is selected, whose name is the
    combo box's new value; `HelpTextRefresh` (`Description`) and `SuggestionsRefresh`
    (Text pattern availability, automation id, and whether the
    `ControllerFor` relation names any element, read as a count of the
    cached element array; `reports_suggestions()`), what a help text and
    a `ControllerFor` change read; `WindowLookup` (native window handle), one property;
    `TextAreaPattern` (the Text pattern of an event's sender, which
    `ListenerEvent` covers), the held console check's;
    and `ShellButton`, what `verbatim-gui`'s list of the shell's buttons
    reads. A rectangle that does not fit reads as `None` rather than
    overflowing.
  - `Request<S>`, a shape's cache request, keeping the gates it was built
    with (`gates()`).
  - `Cached<S>`, an element whose cache `S`'s request filled, made only by
    this crate: by a fill from a `Request<S>`, by a registration's part for
    `S`, by `Agile::resolve`, by `narrow`, and by `Fill::claim`. `read()`
    reads the shape from the cache, locally; `live()` gives the `Live`
    element; `gates()`; `runtime_id()`; `agile()` makes the `Agile<S>`
    that crosses threads; `narrow::<T>()` reads the same element as a
    shape it covers; `observe_cached(property, ignore_default)` gives the
    raw cached value as UIA answers it, for the tests that observe what UIA
    stores for a provider's answer, which a shape's read kinds would hide,
    and is not used by Verbatim's own code.
  - `Agile<S>`, a `Cached<S>` for another thread of this process, `Send`
    and `Sync` with no `unsafe`; `resolve()` gives the `Cached<S>` back,
    the same element and cache, and `reference()` the untyped agile
    reference a registration's `Scope::Elements` takes, which reads nothing
    of the element's cache.
  - `Live`, an element for calls into its application, with no cached
    read: `new`, `element()` for the raw element, `runtime_id()`,
    `refresh(request)` (`BuildUpdatedCache` into a shape),
    `find_first(scope, condition, request)`, `walk(walker, direction,
    request)` (one tree walker step, `Direction` naming the neighbour,
    into a shape) and `normalize(walker, request)`.
  - `Fill<S>`, the token `verbatim-uia-rops` fills a remote program's
    caches with: `properties()` and `patterns()` for the program's cache
    requests, `request()` for its classic fallback, and `claim(element,
    absent)`, hidden from the documentation, which wraps an element the
    program returned as a `Cached<S>`. Neither `Clone` nor `Copy`. That a
    program fills its caches from the token's list is that crate's
    reviewed code, not something the compiler proves.
  - `Absent`, the record a remote program returns for each element of
    which `LEFT_OUT_WHEN_UNSUPPORTED` properties (`IsDataValidForForm`) it
    left out of the element's cache because the element does not support
    them: a remotely filled cache stores a property's default where a
    local one stores "not supported", and `IsDataValidForForm`'s default,
    false, would read as an invalid entry. A read of a property the record
    holds answers "not supported" without asking UIA, so it makes no
    failed read (decision 3 of `docs/design/uia-shapes.md`). A local
    fill's record is empty.

  The shape module's reader is the only cached read in the workspace:
  `ElementExt` has none, and no property list or list-taking fetch is
  public. `cargo xtask ci` rejects a raw cached read (`GetCached` or a
  `.Cached` getter) anywhere but the shape module and `Uia::children`'s
  read of cached children, a raw cache refill (`build_updated_cache`)
  outside this crate, and `Fill::claim` outside `verbatim-uia-rops`
  (`xtask/src/shapes.rs`; `docs/design/uia-shapes.md`, section 3.5,
  item 7).
- `NodeIdRegistry` — maps UIA runtime IDs to stable `NodeId`s and keeps the
  live element behind each, as an `Agile<Kept>`: `id_for_element(runtime_id,
  element)` takes a `Cached<Kept>`, which a keeper of any shape that covers
  `Kept` makes with `narrow`, and `element_of(node)` gives the
  `Agile<Kept>` back, so a reader reads the window handle and class name
  whichever request filled the element, and refreshes it into a declared
  shape (`Live::refresh`) when it needs more (decision 2 of
  `docs/design/uia-shapes.md`); takes an injected shared counter so the UIA and
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
  failing on a thread already in a single-threaded one; `leave_mta()`
  leaves it again, once, for a thread done with UIA for good, after it
  has released its objects. A thread that joined more than once stays in
  the apartment, which costs nothing. The apartment itself is pinned by
  UIA's first-time setup (`CoIncrementMTAUsage`) until
  `release_mta_usage()` gives that hold back, which an outpost and the
  focus listener do at the end of their shutdown, so COM ends the
  apartment once every thread has left it. Role and state mapping in
  `map`, plus `map`'s total `notification_kind_from_uia` and
  `notification_processing_from_uia` tables for the notification payload.
  Every snapshot's name, value, and description lose their bidirectional
  formatting characters as they are read
  (the shapes' `parts`, through `verbatim_text::strip_bidi_controls`), as NVDA strips the direction marks
  from File Explorer's dates and the clock.

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
