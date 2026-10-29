# verbatim-uia

The UIA client stack (`docs/architecture.md`, section 4, "UIA"): clients
and their timeouts, typed cache reads (shapes), event registrations, the
arbitration probe, and the window checks the outposts and the focus
listener make. Its remote operations are in `verbatim-uia-rops`
(`docs/crates/verbatim-uia-rops.md`).

## Clients and timeouts

- `CONNECTION_TIMEOUT` and `TRANSACTION_TIMEOUT`: two seconds, UIA's
  connection timeout (a lookup by handle, the focused element's lookup,
  and the waits of UIA's own focus tracking), and five seconds, its
  transaction timeout (each call on a held element). Both are settings of
  the process, not of a client (experiment E1 in
  `docs/design/focus-pipeline.md`), so UIA's first-time setup sets them
  once. Each bounds one call into a provider, not an operation, which is
  why `BoundedClient` exists. Why these values is in
  `docs/design/listener-without-connections.md`, section 5; five seconds
  is under the outpost worker's ten-second deadline and over the time a
  starting application takes to answer a call.
- `BoundedClient`: a client on a thread of its own for an operation made
  of many provider calls that needs a deadline of its own (a `FindFirst`
  search, a dialog's text). `run(deadline, operation)` queues the
  operation, which captures and answers only `Send` values (UIA objects as
  agile references), and waits for its answer; past the deadline it
  answers `Unanswered::DeadlinePassed` and abandons the thread, which
  finishes its call unheard and ends, and the next `run` starts a new
  thread and client. A UIA call cannot be cancelled, so abandonment is the
  bound. The operation's calls and failed cached reads are added to the
  waiting thread's counts (`calls::add`, `calls::add_failed_reads`).
  `close()` waits for every thread, abandoned ones included; `abandoned()`
  counts them.
- `Uia`: a per-thread client (one COM apartment, one `IUIAutomation`;
  nothing COM crosses threads). The coclass is `CUIAutomation8`, as NVDA
  creates, because only its objects implement the newer client
  interfaces such as `IUIAutomation5`. Every client turns on
  `IUIAutomation6`'s event coalescing and connection recovery, as NVDA
  does (`docs/performance.md`, "Newer UIA features"), and tells UIA's
  proxies never to raise, from an application's `WinEvent`s, the UIA
  events Verbatim subscribes to other than focus changes (the `proxy`
  module, as NVDA's `ignoreWinEventsMap`); a mapping is cleared with a
  null array, since UIA refuses an empty one.
- UIA's first-time setup: every client in the crate, and the provider
  probe, first wait for it. It creates a client and builds a cache request
  once, under a lock, on a thread of its own (so a thread that only probes
  never joins COM), and pins the multithreaded apartment
  (`CoIncrementMTAUsage`) so the setup is not undone, because a cache
  request built on one thread while the setup runs on another fails with
  `E_FAIL` (mockapp's
  `tests/uia_bugs/a_cache_request_fails_while_uias_first_time_setup_runs_on_another_thread.rs`).
  `release_mta_usage()` gives that hold back, which an outpost and the
  focus listener do at the end of their shutdown; it also ends the
  registering thread (below).
- `init_mta()` joins the multithreaded apartment, failing on a thread
  already in a single-threaded one; `leave_mta()` leaves it again, once,
  for a thread done with UIA for good. `release_thread_state()` drops the
  per-thread client and walker behind `console_reports_formatting` and
  `nearest_window_handle`; a thread that used them calls it before it
  exits, since a thread-local destructor runs under the loader lock, where
  COM work must not run.

## Fetching elements

Every fetch takes a shape's `Request` and returns its `Cached` (the
shapes, below), or a `Live` when nothing of the element is read. Each
call waits up to `CONNECTION_TIMEOUT` to connect and
`TRANSACTION_TIMEOUT` for its answer, and every one is safe to call.

- `window_element(hwnd, request)`: a window's element
  (`ElementFromHandleBuildCache`), for an outpost's own application's
  window with a provider. It is refused, by the proxy rule, when only
  `UIAutomationCore.dll`'s own proxies serve the element returned, as a
  lookup of an application busy past the connection timeout returns; the
  rule reads the provider description cached with the lookup, an empty
  one counting as answered, and the count of a window's refusals in a row
  is kept for the logs. The description counts as served only by those
  proxies when every provider it names, whatever its role (main,
  override, nested), is from `UIAutomationCore.dll`, so File Explorer's
  child windows whose main provider is the MSAA proxy under an override
  from `DUI70.dll` are answered. A refusal is logged at debug, and from a
  window's fourth refusal in a row at info with the count
  (`REFUSALS_BEFORE_INFO`), so a window that never answers shows in the
  logs; a refusal is never retried, the next event or query that needs
  the window looking it up again. `focused(request)` is the focused element under
  the same rule. `any_window_element(hwnd, request)` is the lookup without
  the rule, for a window with no provider.
- `window_live(hwnd)`: a window's element with no cache, for a caller
  that only searches under it; one call.
- `request_fixed::<S>()` and `request::<S>(gates)`: a shape's request,
  without gates or for the gates given; local.
- `children(parent, request)`: `parent`'s raw-view children, each filled
  by `request`, in one `BuildUpdatedCache` on a copy of the request scoped
  to the children, as NVDA fetches children; a dialog's own text and the
  tree dump are gathered this way.
- `find_child_by_runtime_id(parent, runtime_id, request)`: a search of
  `parent`'s children only. No search of a whole subtree by runtime id is
  offered but the controlled-element search's, since one enters any child
  window another process serves
  (`docs/design/cross-process-calls-study.md`, instance 3). The runtime id
  condition's `VARIANT` is built by `InitVariantFromInt32Array`, so no raw
  array is handled; `windows`'s `VARIANT` clears its array on drop. A
  search that finds nothing is `Ok(None)`, and so is one that passed over
  a child window whose process did not answer within the connection
  timeout; one that fails otherwise is an error.
- `controlled(focused, runtime_id, request, current_instead)`: the
  selected element when it is inside an element the focus names in its
  `ControllerFor` relation, else, with `current_instead`, the first
  controlled element's first selected element unless `runtime_id` names
  it; the classic implementation behind
  `verbatim_uia_rops::controlled_selection`.
- `selected(container, request)`: the first element of a selection
  container's current selection, or `None`. One `BuildUpdatedCache`
  refreshes the container as the `SelectionContainer` shape, which holds
  `SelectionPattern2`'s `FirstSelectedItem` and the `Selection` pattern,
  so NVDA's preference for the newer pattern costs two calls, and three
  for a provider without it. A missing pattern or an empty selection is
  `Ok(None)`, as is any failure but two: a call that timed out
  (`timed_out`) or found the container or item gone (`element_is_gone`)
  is an error, never "nothing selected". `selected_within(container,
  request, out_of_time)` checks the caller's time before each call after
  the first; `selected_node(container, request, registry)` reads the item
  as a `Node` snapshot.
- `neighbor(element, request, registry, kind)`: one raw-view tree-walker
  step (parent, next or previous sibling, first child, named by a
  navigation `QueryKind` from `verbatim-model`), the neighbor as a `Node`
  snapshot, `Ok(None)` for none. A step that fails because the element is
  gone is an error; any other failure reads as no neighbor, as NVDA's
  tree-walker failures do. The tree is the full, unfiltered one, matching
  NVDA with simple review mode off.
- `activate`: NVDA's activation ladder, `Invoke`, then `Toggle`, then
  `SelectionItem`'s select, answering `ActionName::Invoke` for an Invoke
  and no name otherwise; each pattern fetched live, since activation is
  infrequent.
- `tree(root, request, registry, max_depth, max_nodes)`: a tree dump; the
  control plane's is `verbatim_uia_rops::tree_dump`.
- `compare_elements`: UIA's `CompareElements`, counted as one call, for
  the focus listener's log of whether two focus senders are one element.
- `raw_view_walker`, `control_view_walker`, `root_element`, and
  `property_condition`: local helpers.
- `nearest_window_handle(element)`: NVDA's `getNearestWindowHandle`, the
  native window handle of `element` or of its nearest ancestor that has
  one, in one round trip (`NormalizeElementBuildCache` against a walker
  whose condition excludes every element without one). It can block on a
  hung application, and is callable from UIA event threads because each
  outpost watches a single application (decision D9), as NVDA runs the
  same walk on its UIA event thread. Its client, walker and request are
  kept per thread. `nearest_window_lookup(element)` is the same lookup
  with a call that timed out (`timed_out`) returned as an error rather
  than as no window, so the focus listener can drop such a focus for the
  application not answering.

## Ancestors

`Uia::ancestors((element, element_window, nearest), request, registry,
(max_hops, parent_automation_id), stops)` gives the chain of an element's
ancestors, each read as the `Ancestor` shape, outermost first, as
`NodeSnapshot`s, with the automation id of the raw-view parent put in
`parent_automation_id`. It is a per-hop `GetParentElementBuildCache` walk
over the raw view, capped by the caller and stopped by `AncestorStops`:
at a window read through the other API, at an ancestor the caller
already knows (`AncestorWalk::MetKnown`), or at the caller's
`out_of_time` check, a hop the transaction timeout ended, or a parent
found past `max_hops` ancestors (`AncestorWalk::Cut`, with the outermost element's nearest window, from
which the caller completes the chain by plain windows). The root has no
parent, which ends the walk as complete; any other failed hop, or one that
finds no parent, below the root ends it as `AncestorWalk::Failed`, carrying
a `FailedHop` (`Error` with the code, or `NoParent`) and the window the
chain goes on from, the parent window of the outermost element's nearest
window, as NVDA reads the window hierarchy when its UIA parent read fails.
`nearest` is the element's nearest window, 0 when not known; the outermost
element's is followed by `outermost_nearest_window(nearest, hwnds)`, local
calls: an element that is its window's own has that window, one with none
above one that had its own is hosted by that window's parent window, and
one with none above one with none has the same window as it
(`parent_window` is `GetAncestor` with `GA_PARENT`). Telling the root is
two local calls, made only when a hop fails.

The walk never steps across the process boundary
(`docs/design/listener-without-connections.md`, section 3): before each
hop from an element whose own window has a parent window, or visible
owner, that another process serves, it stops and answers that window as
the one to continue through the other API.

Ancestors that are not presentable focus context, by NVDA's
`isPresentableFocusAncestor` ported (layout elements, that is unknown and
pane roles, textless static text, nameless windows, property pages, and
groupings, plus list items, tree items, and editable text), are crossed
but never reported; a UIA element must also be both a control and
content. `Ancestor::is_focus_context(snapshot)` is that whole test,
public so the outpost applies it to the element of a UIA window an MSAA
walk crosses into. The layout judgment reads the ancestor's own
description whatever the theme, as NVDA's `presentationType` does
(decision 4 of `docs/design/uia-shapes.md`).

- `Uia::ancestors_from(parents, registry, stops)` gives the same result
  from ancestors already fetched as `Ancestor`s, nearest first, such as
  those `verbatim_uia_rops::focus_ancestry` returns; a fetch cut short or
  ended by a failed step is reported as complete up to there, and the
  caller, which knows how the fetch ended, completes the chain.
- `Uia::ancestor_chain_from_parts(parents, registry, stops)` does the
  same for ancestors another process read and sent as their parts
  (`AncestorParts`: the cached parts, the own window handle, and whether
  UIA counts the element both a control and content), minting each node
  from its runtime id with no element kept: the outpost reports a UIA
  focus's ancestors this way from the focus listener's read
  (`docs/design/focus-pipeline.md`, section 12).

## Mapping and the `map` module

A node is read from the `Node` shape (below): name, control type, value,
native window handle, enabled, focus states, toggle, expand-collapse, and
selection-item state with their pattern-availability flags; the details,
each fetched only when the theme reports it (`FullDescription` and
`HelpText`, `AccessKey` and `AcceleratorKey`, spoken joined by two
spaces as NVDA joins them, `PositionInSet`, `SizeOfSet`, and `Level`);
for an element read alone, `BoundingRectangle` and `IsOffscreen`, and,
when the theme reports cell coordinates, the grid item's row and column;
and always `ClassName`, `IsDialog` (a dialog by NVDA's rule),
`IsPassword`, `IsRequiredForForm`, `IsDataValidForForm`, and
`ValueIsReadOnly` (the protected, required, invalid entry, and read-only
states), and `RangeValueValue`, the rounded value of a control with no
`Value` pattern. The last two are read only when `IsValuePatternAvailable`
and `IsRangeValuePatternAvailable` say the pattern is there, because a
remotely filled cache stores their defaults where a local one stores "not
supported". An ancestor adds `IsContentElement` and `IsControlElement`.
`runtime_id(element)` reads an element's runtime id.

UIA returns default values for properties an element does not support,
so every mapping treats the default as "not reported": the toggle,
expand-collapse, and selection-item states are interpreted only when
their pattern-availability flag says the pattern is there (a plain pane's
`ToggleState` reads as indeterminate), selection-item availability maps
to `Selectable` as MSAA's selectable bit does, and the details' empty
string and zero map to `None`. Every snapshot's name, value, and
description lose their bidirectional formatting characters as they are
read (`verbatim_text::strip_bidi_controls`), as NVDA strips the direction
marks from File Explorer's dates and the clock.

`map` holds the role and state mapping, the total
`notification_kind_from_uia` and `notification_processing_from_uia`
tables, and these rules:

- `map::value_from_selection(role, class_name, value_available)`, and
  `Node::value_from_selection`: a node's value is its first selected
  item's name, as NVDA reads it, for a combo box without the `Value`
  pattern, or a list whose class name contains `LoopingSelector` (a time
  picker's) without it.
- `map::with_legacy_checked_state(element, node)`: a menu item that no
  pattern makes checkable (`map::wants_legacy_checked_state`) is
  checkable and checked when its legacy MSAA state has the checked bit, as
  NVDA reads Windows Forms menu items. The state is read live, one call,
  only for such a menu item, as NVDA reads it lazily, because caching it
  for every element made UIA ask every provider for the
  `LegacyIAccessible` pattern. `map::add_legacy_checked_state` is the
  rule alone, for a state read already. A selected radio button is
  checked rather than selected, and a toggleable element other than a
  check box or toggle button is checkable.
- `map::ignores_notifications(class, automation_id)`: a terminal
  control's and the console host's text area's (automation id "Text
  Area") notifications are all ignored, as NVDA's terminal overlays
  ignore them.
- `map::is_terminal_class(class)`: Windows Terminal's text control
  (`TermControl`) and the one embedded in .NET applications
  (`WPFTermControl`) are terminals, by their UIA class as NVDA recognizes
  them; the mapping gives them `Role::Terminal`. The console host's text
  area is recognized by its window, in the outpost.

## Safe wrappers

The `windows` crate marks every COM method `unsafe` only because its
bindings are generated; an interface value is a counted reference to a
live object, and a gone provider or unsupported property comes back as an
error. So each wrapper method holds one documented `unsafe` call and is
safe to call.

- `ElementExt`, on `IUIAutomationElement`: the live calls
  (`current_control_type`, `current_i32_ignoring_default`, `current_bool`,
  `has_keyboard_focus`, `build_updated_cache`, `current_pattern`,
  `controller_for`, `find_first`, `find_first_build_cache`), each counted
  as one UIA call; no cached read, since the shapes make every one.
- `WalkerExt`, on `IUIAutomationTreeWalker`: `parent`, `next_sibling`,
  `previous_sibling`, `first_child`, and `normalize`, each built with a
  cache and counted.
- `elements_of` reads an element array, locally.
- The `VARIANT` readers `variant_string`, `variant_i32`,
  `variant_i32_array` (a range's annotation types, one integer or an
  array), `variant_f64`, `variant_optional_bool`, and `variant_element`
  give `None` for a sentinel or another type; `is_not_supported` and
  `is_mixed` tell UIA's two sentinels apart. `element_is_gone(error)` is
  UIA's element not available or a disconnected provider;
  `timed_out(error)` is a call that ended at the transaction timeout
  (`UIA_E_TIMEOUT`), which says nothing about what was asked.

The pattern methods the crate calls are crate-private wrappers of the
same kind. `unsafe` remains only in the wrapper module (`element.rs`),
the `VARIANT` and `SAFEARRAY` helpers (`com.rs`), the event handler
registrations, the provider probe's window message, client creation, and
apartment setup. The event handlers are COM objects generated by
`windows-core`'s `#[implement]` macro, in a module that scopes lint
allowances to the generated code.

## The `text` module

Safe wrappers over the text pattern, which the outpost's text protocol
reads with. Every method is a cross-process call, counted once: a text
range is a provider object in the application, so even copying one is a
round trip.

- `text_pattern(element)` fetches a node's `TextPattern2` where the
  provider has it (the caret's own range) and else its `TextPattern`, one
  call or two; `TextPatternExt` has `selection` and `document_range`;
  `caret_range(pattern2)` is `GetCaretRange`.
- `TextRangeExt` on a range: `clone_range`, `compare_endpoints`,
  `expand`, `move_by`, `move_endpoint_to` (by another range's end),
  `move_endpoint_by_unit` (both moves' counts negative for a backward
  move whatever sign the provider gave, `signed_move`, as NVDA corrects
  them), `text` (UTF-16, up to a limit), `select`, `find_text` (case
  sensitive, forward or backward, `None` when the provider returns no
  range), `bounding_rectangles`, `culture` (the `Culture` attribute as a
  BCP 47 tag, `None` when the range mixes languages or the provider does
  not say), `language` (the same read as a `Language`: one tag, mixed, or
  none, so a caller reading several units asks once), `attribute` (any
  text attribute's raw `VARIANT`, sentinels included), and `attributes`,
  several attributes in one `IUIAutomationTextRange3::GetAttributeValues`
  call (NVDA's bulk fetch), falling back to one `attribute` call each, a
  failed attribute read as "not supported" as NVDA reads it.
- `locale_name` makes a BCP 47 tag from a locale id; `Endpoint` names a
  range's start or end; `uia_text_unit` maps a model `TextUnit` to UIA's,
  `None` for the sentence, which UIA does not have.

## Event registrations

The design is `docs/design/focus-pipeline.md`, section 17.

- `Registration::with_parts(parts, scope)`, `retarget(scope)`, and
  `retarget_to(scope, group)`: one registration type for every event. A
  `Subscription` is `Properties` (property ids, such as
  `FOCUS_PROPERTIES`: name, value, range value, toggle state, enabled,
  expand/collapse, help text, and `ControllerFor`), `Event` (an
  automation event id), `Events` (several event ids through one handler
  whose callback receives the id), `Notifications` (the raising element
  with kind, processing, display string, and activity id),
  `ActiveTextPosition` (`IUIAutomation6`'s event, with the range now
  active), or `Focus` (focus changes anywhere on the desktop). The
  callback types are `PropertyCallback`, `ElementCallback`,
  `EventCallback`, `NotificationCallback`, `ActiveTextPositionCallback`,
  and `FocusCallback`.
- The subscriptions are registered as an event handler group
  (`IUIAutomationEventHandlerGroup`), as NVDA registers its handlers: each
  handler is added to the local group, which is registered on each
  element of the scope with one `AddEventHandlerGroup` call. Each
  subscription is a `Part` whose events' elements arrive with its shape's
  properties and patterns prefetched. `Part::shaped::<S>(subscription)`,
  for a shape with no gates, is the only way to make one; its callbacks
  receive each element as a `Cached<S>` (the subscription is
  `Subscription<Cached<S>>`, wrapped inside into one that receives the
  raw element, so a registration holds parts of several shapes), and
  their reads are counted in `Registration::failed_reads()`.
  `.extended()` marks a part held only by the extended groups, and
  `.text_events()` one that delivers text or caret changes.
- A `Group` is NVDA's `Base` (every part not marked extended) or
  `Extended` (every part), or `ExtendedWithoutTextEvents`, every part but
  the text events, for an element whose text and caret changes reach the
  caller another way. Each is built once per registration, on first need.
- A focus subscription is not part of the groups: its handler is
  registered on the registering thread's client itself
  (`AddFocusChangedEventHandler`) before any group, as NVDA registers its
  focus handler on the client that holds its global group, because UIA
  delivers the focus events Windows Terminal raises from a second
  identity of its terminal to a client with only a focus handler, but not
  to one that also has a group on the desktop's root
  (`docs/design/focus-pipeline.md`, section 16.8). The handler is
  desktop-wide, so only the focus listener registers it. The order is
  NVDA's too: the desktop's root element is fetched before the focus
  handler is registered, the group is registered on that root after it,
  and parts asking for the same properties share one cache request;
  registered in another order, the second identity reaches the client
  now and then.
- Every handler names the UI Automation thread it runs on
  `verbatim-uia-callback` at its first event, unless the thread has a
  name already (once per thread; later callbacks cost a thread-local
  read), so a trace of the processor's scheduling tells the threads
  running Verbatim's handlers from UI Automation's own, which stay
  unnamed.
- A `Scope` is nothing yet, the subtrees of given elements (`Subtrees`),
  the whole desktop, or exactly given elements. The caller looks a
  window's element up itself, so the registering thread makes no
  connection by handle.
- A move (`retarget` gives the base group, `retarget_to` the group named)
  is handed to the registering thread, so the caller never waits on UIA's
  removal, which waits for running callbacks; so no callback may wait on
  any registration's moves. The thread adds the group to each element of
  the new scope that does not already have it, then removes it from each
  element no longer in the scope, as NVDA moves its local group. An
  element that already has the same group, by `CompareElements`, keeps it
  with no call; a move to the same element with the other group adds the
  new one, then removes the old. Elements that fail to resolve or to
  register (an element that has gone) are skipped. A move queued behind a
  newer move of the same registration is skipped, unless a settle or close
  comes between. Every move and each call it or a close makes is logged at
  debug level with its result and elapsed milliseconds, at warn level when
  the call took longer than `CONNECTION_TIMEOUT` or was a failed removal,
  naming the element by runtime id. `settle()` waits until every move
  asked for before it has been made, for tests.
- `close()` ends a registration for good and returns once done: every
  move already asked for is made, then the registering thread removes
  each group (`RemoveEventHandlerGroup`) and the focus handler
  (`RemoveFocusChangedEventHandler`), each waiting for a callback in
  progress. Moving or settling a closed registration does nothing, and
  dropping one closes it. Each removal is tried once: a group a move
  failed to remove is tried again at the close (`docs/uia-bugs.md`, "A
  registration that times out reports success"), and one whose removal
  fails at the close stays live, calling the closed registration's
  callbacks, until the thread ends and `RemoveAllEventHandlers` removes
  it. A `close()` made on the registering thread itself is queued and
  does not wait.
- Every registration in a process is made by one registering thread with
  one client, created on the process's first registration, in the order
  asked, because Microsoft's guidance for UIA clients is that a client
  should not add or remove event handlers from several threads at once
  ([Understanding Threading Issues](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading)).
  The thread waits on its commands and wakes for nothing else. No call is
  given a deadline: UIA ends a registration on an application that does
  not answer at the connection timeout, and what is asked after it waits
  for it. `release_mta_usage` ends the thread, calling
  `RemoveAllEventHandlers`, and every registration still open then hears
  nothing more; a later registration starts the thread again.

The outpost's and the focus listener's registrations, their parts and
shapes, are described in `docs/crates/verbatim-outpost.md`.

## Provider checks and the process boundary

- `raw_probe(hwnd) -> RawProbe`: the arbitration probe, which the
  outposts make on their own application's windows and the focus
  listener on the windows of the focuses it reads
  (`docs/design/listener-without-connections.md`, decision 2): one
  `SendMessageTimeoutW` of `WM_GETOBJECT` with wParam 0 and
  `UiaRootObjectId` sign-extended, with
  `SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT` and
  `RAW_PROBE_TIMEOUT_MS` (3,000 ms), counted as one window message, so it
  is called only from deadline-guarded threads. It asks once, as NVDA
  does, and its answer is kept for 500 ms (`docs/design/focus-pipeline.md`,
  section 8). Unlike `UiaHasServerSideProvider`, which NVDA calls, it
  sends only that one message and makes no UIA connection, so it answers
  correctly where a connection would meet another process's queued remote
  operation, and where `UiaHasServerSideProvider` gives a new console
  window a false "no" (`docs/uia-bugs.md`). It runs UIA's first-time
  setup first. `raw_probe_verdict(sent, answer, last_error)` reads the
  outcome, a pure function: exactly `0x10000` is `RawProbe::Provider`;
  any other answer is `Answered` (no provider); a failure is `TimedOut`
  (or judged hung), `AccessDenied` (a window of higher integrity),
  `Gone`, or `Failed` with its error. `RawProbe::has_provider` is true
  only for `Provider`. The probe's memory cost in the application is
  risk 7 of `docs/design/listener-without-connections.md`.
- `provider_process(hwnd)` and `console_host_of(client)`: the process that
  serves a window's UIA, by local calls: the window's owner, except a
  console window, whose provider is its console host, read from the
  client with `NtQueryInformationProcess`'s `ProcessConsoleHostProcess`
  (`docs/design/listener-without-connections.md`, section 4.3). A console
  window whose host cannot be read has no provider process.
  `served_by(hwnd, pid)` is whether `pid` serves the window, for such a
  console window by whether it is one of `pid`'s threads' windows, so a
  console host's outpost still recognizes its own console window.
- `is_top_level_window(hwnd)`: whether `hwnd` is its own root
  (`GetAncestor` with `GA_ROOT`); local.
- `parent_across_process_boundary(hwnd)`: the window above `hwnd` in
  UIA's raw view when another process serves it: its parent, or, for a
  top-level window, its owner while the owner is visible; `None` for a
  top-level window with no visible owner. Local calls only; the boundary
  is defined in `docs/design/listener-without-connections.md`.
  `raw_view_parent_window(hwnd)` is that window whatever process serves
  it.
- `console_reports_formatting(hwnd)`: the check NVDA makes on a console
  window with a provider before using it, whether its text area reports
  one visible range with its font. `held_console_reports_formatting(uia,
  sender, automation_id, out_of_time)` is the same check from an event's
  sender the caller holds, with no connection to the window, the focus
  listener's: the sender is the console's text area (automation id
  `CONSOLE_TEXT_AREA`), whose Text pattern is read from its cache
  (`TextAreaPattern`), or the console window's element, under which the
  text area is found. Each answers `None` when the application did not
  answer in time, and the held check also when its sender is gone or the
  caller's time ran out, so the caller asks again.
  The tests' console comes from `verbatim-test-console`.

## Counting calls

`calls` counts the cross-process calls this crate makes, per thread:
`calls::count(kind)` counts one, `calls::take()` returns this thread's
`CallCounts` since the last take and resets them, and `calls::peek()`
returns them without resetting, so a step can log its own calls by the
difference. Every call that reaches the application is counted where it
is made: the `*BuildCache` fetches and tree-walker steps,
`BuildUpdatedCache`, `FindFirst`, `CurrentControllerFor`, pattern fetches
and methods, `NormalizeElementBuildCache`, and the provider checks' text
reads as UIA calls, and the raw probe as one window message. The calls
that connect to a window by its handle (`ElementFromHandle`,
`ElementFromHandleBuildCache`, `GetFocusedElementBuildCache`) also count
as connections (`CallCounts::connections`), which the focus listener
keeps at zero (`docs/design/listener-without-connections.md`, section 2).
Cached reads, `GetRuntimeId`, and creating clients, conditions, walkers,
and cache requests are local and not counted, nor are event
subscriptions (`docs/performance.md`, "What counts as a call").

Each thread also counts its failed cached reads, a cached read UIA
answers with a failure for a property or pattern the element's request
did not hold: `calls::take_failed_reads()` and
`calls::peek_failed_reads()`. Their owners gather them, including
`Registration::failed_reads()` for the callbacks on UIA's threads
(`docs/design/uia-shapes.md`, section 3.6). The shape module's unit tests
read every shape, for every combination of its gates, through the
crate-private cached-read seam against a recording source that answers
every read, and assert that the properties and patterns read, with the
prefetched patterns, equal the request for those gates exactly, so a
missing read and an over-fetch both fail.

## Shapes

The shape module is `docs/design/uia-shapes.md`, section 3. A shape is a
struct whose fields are what one purpose reads of an element; its cache
request and its read are generated from one field list by a macro
private to the crate, so a shape's read names exactly the properties and
patterns its request holds.

- `Shape`, `Ungated` and `Covers<T>`, sealed traits. `Shape` carries the
  shape's `PROPERTIES` and `PATTERNS`, `UNGATED` (the properties every
  request of it holds), `all_on()`, and its gates type, `()` for a shape
  with no gates, which `Ungated` marks. `Covers<T>` says a shape's ungated
  request holds every property and pattern of `T`'s, checked at compile
  time.
- Gates: a shape declared with gates has a gates struct of its own
  (`NodeGates`, `AncestorGates`), with a `bool` for each gated property
  and an `Option` of the group's gates for each gated group, and each
  gated field reads as an `Option`, `None` without touching the cache
  when its gate was off. The outpost maps the theme to the gates.
- The groups: `RoleCore` (control type, class name, `IsDialog`,
  `IsTogglePatternAvailable`, native window handle; `role()`, and
  `window()`, the handle an element that is its window's own carries in
  `NodeDetails::window` from every shape's `parts`, `None` for an element
  inside a window),
  `ValueSet` (the Value pattern's value and the range value with its
  availability; `value(class_name)`), `States` (every state property but
  `IsOffscreen`; `states(role, offscreen)`), `Description`, `Shortcut`,
  `Position`, `Place` (`BoundingRectangle` and `IsOffscreen`), and `Cell`
  (a grid item's availability, row and column; `coordinates()`,
  one-based). A read of many elements never holds `Place`, as NVDA's base
  cache holds neither, because for some elements, a top-level window's
  title bar among them, UIA answers either only by sending the
  application's UI thread `WM_GETTITLEBARINFOEX` (decision 9 of
  `docs/design/uia-shapes.md`). A rectangle that does not fit reads as
  `None`.
- `Kept`: the native window handle and class name, all any reader of a
  kept element reads, covered by every shape whose elements are kept.
  `FocusInput`: control type and native window handle, what the remote
  focus query reads of its input.
- The event shapes: `ListenerEvent` (NVDA's thirteen base properties,
  `IsDialog` and `IsTogglePatternAvailable`, with the Text pattern;
  `parts(runtime_id)`; `lacked_keyboard_focus()`, true unless its cached
  `HasKeyboardFocus` is true, since a provider that answers nothing and
  one whose answer fails leave the same default), `ListenerSelection`
  (the listener's element selected: a `ListenerEvent` with what Core
  speaks of an item selected in a focused list, `ValueSet`, `States`,
  `Description`, `Shortcut`, `Position`, `Cell` and the level, 40
  properties, a `Node`'s without its `Place`; `parts(runtime_id)`),
  `PropertyEvent`
  (`RoleCore`, name, `ValueSet`, `States` and the process id;
  `parts(runtime_id)`), and `TextEvent` (process id, native window handle
  and class name). All four cover `Kept`, `ListenerEvent` covers
  `FocusInput` and `TextAreaPattern`, and `ListenerSelection` covers
  `ListenerEvent` and both of those.
- The fetch shapes: `Node` (`RoleCore`, `ValueSet`, `States`, name, four
  gated details, the `Description`, `Shortcut` and `Position` groups and
  the level, and the gated `Place` and `Cell`: 22 to 34 properties;
  `parts(runtime_id)`, and on `Cached<Node>` `snapshot(registry)`, which
  keeps the element), covering `Kept` and `FocusInput`; `Ancestor` (a
  `Node`, its own ungated `Description`, the content and control flags,
  and its automation id: 27 to 37; `is_control_and_content()`,
  `layout_description()`, and `snapshot` on `Cached<Ancestor>`), covering
  `Kept`; `DialogText` (role, value, states, name and description), a
  dialog's child; `SelectionContainer` (`SelectionPattern2`'s first
  selected item, read as an `ElementValue`, and the `Selection` pattern);
  `SelectedItem` (name and keyboard focus; `name()`), an item found among
  a focused combo box's children; `HelpTextRefresh` (`Description`;
  `description()`, the full description, else the help text) and
  `SuggestionsRefresh` (Text pattern availability, automation id, and
  whether `ControllerFor` names any element; `reports_suggestions()`,
  `controls_any`), what a help text and a `ControllerFor` change of the
  focus read; `WindowLookup` (native window handle); `TextAreaPattern`
  (the Text pattern of an event's sender), the held console check's; and
  `ShellButton` (native window handle, control type, name, and whether it
  is off the screen), what File Explorer's outpost reads of each of the
  shell's buttons.
- `Request<S>`: a shape's cache request, keeping its gates (`gates()`);
  `with_process()` is a copy that also caches the element's process, for
  the outpost's read of the focused element, which checks it before
  anything else is read.
- `Cached<S>`: an element whose cache `S`'s request filled, made only by
  this crate: by a fill from a `Request<S>`, a registration's part,
  `Agile::resolve`, `narrow`, or `Fill::claim`. `read()` reads the shape
  locally; `live()` gives the `Live` element; `gates()`; `runtime_id()`;
  `agile()` makes the `Agile<S>`; `narrow::<T>()` reads the same element
  as a shape it covers; `cached_process()` reads the process a
  `with_process()` request cached (`ProcessOrigin`, a crate-internal
  shape), `None` when it holds none; `observe_cached(property,
  ignore_default)` gives the raw cached value, for tests only.
- `Agile<S>`: a `Cached<S>` for another thread, `Send` and `Sync` with no
  `unsafe`; `resolve()` gives the `Cached<S>` back, and `reference()` the
  untyped agile reference a `Scope::Elements` takes.
- `Live`: an element for calls into its application, with no cached
  read: `new`, `element()`, `runtime_id()`, `refresh(request)`
  (`BuildUpdatedCache` into a shape), `find_first(scope, condition,
  request)`, `walk(walker, direction, request)` (one step, `Direction`
  naming the neighbour, into a shape), `step(walker, direction)` (the same
  step with no cache, for a walk that compares runtime ids alone) and
  `normalize(walker, request)`.
- `Fill<S>`: the token `verbatim-uia-rops` fills a remote program's
  caches with: `properties()` and `patterns()`, `request()` for its
  classic fallback, and `claim(element, absent)`, hidden from the
  documentation, which wraps an element the program returned as a
  `Cached<S>`. Neither `Clone` nor `Copy`. That a program fills its caches
  from the token's list is reviewed code, not something the compiler
  proves.
- `Absent` and `LEFT_OUT_WHEN_UNSUPPORTED`: the record a remote program
  returns of the properties (`IsDataValidForForm`) it left out of an
  element's cache because the element does not support them, so a read of
  one answers "not supported" without asking UIA (decision 3 of
  `docs/design/uia-shapes.md`). A local fill's record is empty.

The shape module's reader is the only cached read in the workspace.
`cargo xtask ci` rejects a raw cached read anywhere but the shape module
and `Uia::children`'s read of cached children, a raw cache refill outside
this crate, and `Fill::claim` outside `verbatim-uia-rops`
(`xtask/src/shapes.rs`; `docs/design/uia-shapes.md`, section 3.5).

## The node registry

`NodeIdRegistry` maps UIA runtime ids to stable `NodeId`s and keeps the
live element behind each as an `Agile<Kept>` (decision 2 of
`docs/design/uia-shapes.md`). `new(counter)` takes a shared counter so
the UIA and MSAA registries in one outpost never hand out the same id.
`id_for(runtime_id)` and `id_for_element(runtime_id, element)` mint or
look up a node, the second keeping a `Cached<Kept>`, which a keeper of
any shape covering `Kept` makes with `narrow`; `existing_id`, `ids`, and
`runtime_id_of` look up; `element_of(node)` gives the `Agile<Kept>`
back, which a reader refreshes into a declared shape when it needs more;
`evict_element` forgets a node's element. Nodes stay until the outpost
releases them with `retain`, which returns them as `Released`, to be
dropped once no lock is held, since releasing an element can call into
its process. They are not dropped when their window is destroyed, since a
runtime id has no documented structure naming its window, so a node Core
still holds after its window closed is the node for any new element
given the same runtime id; NVDA has the same exposure. `take_touched`
reports the nodes issued or looked up since the last call. A runtime id is
unique only among live elements (File Explorer gives a dead element's id
to a new one), so `reissue` forgets the node a runtime id names, and the
outpost calls it before reporting a focus whose runtime id names a node
whose element no longer has the keyboard focus or cannot be read.
