# Performance: the operation ledger

How fast Verbatim answers depends mostly on how many times it waits for
another process. A call into an application's process is a round trip
through that application's message loop, and a busy application makes
every one slow, so the number of such calls is the cost that matters and
the one Verbatim controls. Call counts do not vary between runs, so CI
enforces them exactly; times vary, so they are judged against a floor
measured in the same run.

- `crates/mockapp/tests/call_counts.rs` pins every operation's calls by
  kind, the provider calls mockapp answered by method, and its failed
  cached reads (a cached read of a property or pattern the cache request
  did not hold, which costs no call but makes UI Automation raise an
  error inside itself, `docs/design/uia-shapes.md`). No pinned operation
  makes a failed cached read.
- `crates/mockapp/tests/instruction_limit.rs` pins remote programs'
  instruction counts.
- An exact count is a ratchet: a change that adds a call fails CI, and a
  change that removes one lowers the number in the test and here in the
  same commit. A provider cannot tell its callers apart, so each ratchet
  test runs alone in a process on a desktop of its own
  (`docs/crates/mockapp.md`).

Every other figure is a measurement and gives its build and conditions,
and its date where that matters to reading it. "This machine" is the
development machine: x64, a QEMU virtual machine with 12 virtual
processors of an AMD Ryzen 7 5700G and 16 GB of memory, running Windows
11 Pro build 26200.

## What counts as a call

A call counts when it reaches the application's process. There are three
kinds, counted separately (`verbatim_model::CallKind`):

- A UIA call: a UI Automation client method the application's provider
  answers: the fetches (`GetFocusedElementBuildCache`,
  `ElementFromHandleBuildCache`, `FindFirstBuildCache`,
  `BuildUpdatedCache`), a tree walker's `*BuildCache` steps,
  `NormalizeElementBuildCache`, `CurrentControllerFor`,
  `GetCurrentPattern`, and a pattern's methods. A text pattern's and a
  text range's methods are one each, since a range is a provider object
  in the application, so even copying one is a round trip. Each `Execute`
  of a remote operations program is one; importing its elements and
  asking which instructions are supported are local.
- An MSAA call: an `IAccessible` method, `IAccIdentity`'s identity
  string, the acquisitions `AccessibleObjectFromEvent`,
  `AccessibleObjectFromWindow`, `AccessibleChildren` and
  `WindowFromAccessibleObject`, and a `QueryInterface` for any interface
  but `IUnknown` on an object from the application.
- A window message answered by the application's window procedure: the
  arbitration probe (one `WM_GETOBJECT` with wParam 0, the outposts' and
  the listener's alike, where `UiaHasServerSideProvider`, which Verbatim
  does not call, sends two and connects), a list view's or tree view's
  `LVM_` and `TVM_` messages, and an edit control's `EM_` messages and
  `WM_GETTEXT`. Writing a rich edit control's structures into the
  application's memory (`VirtualAllocEx`, `WriteProcessMemory`,
  `ReadProcessMemory`) is a call into the kernel and is not counted.

UIA calls that connect to a window by its handle (`ElementFromHandle`,
`ElementFromHandleBuildCache` and `GetFocusedElementBuildCache`) are also
counted apart as connections (`CallCounts::connections`), each still
once under its kind. The probe is not one: an outpost's probe that finds
a provider costs mockapp one `WM_GETOBJECT`, two `ProviderOptions`, one
`HostRawElementProvider` and one `Navigate`, and no connection follows
(`an_outposts_probe_that_finds_a_provider_makes_no_connection`). The
focus listener makes no connection
(`docs/design/listener-without-connections.md`, section 2;
`the_listener_makes_no_connection`).

The rules for the edge cases, each with its reason:

- One API call counts once, however many round trips it makes inside,
  because the count is what Verbatim decides; mockapp's hit counters show
  what each call cost the application, and the ratchet pins both.
- `QueryInterface` for anything but `IUnknown` counts every time, because
  COM answers it from its proxy or with a round trip and Verbatim cannot
  tell which; `IUnknown` is always local.
- Local work is not counted: cached reads, `GetRuntimeId`, creating a
  client, cache request, condition or tree walker, setting a timeout,
  reading an element array, agile references, and the window functions
  that read the window manager's state rather than the application's
  message loop (`IsWindow`, `GetClassName`, `GetAncestor`, `GetWindow`,
  `GetWindowThreadProcessId`, `IsWindowVisible`, `IsChild`,
  `GetGUIThreadInfo`, `InternalGetWindowText`).
- Reference counting is not counted, because COM batches it outside
  Verbatim's control.
- Event subscriptions are not counted, because they run on their own
  threads and belong to no one event.

`verbatim-uia` and `verbatim-ia2` count calls per thread at the call. An
outpost's worker is the only thread that calls into the application, so
its count is exactly its current entry's; it travels in the published
event's or reply's timing, and `verbatim-inspect latency` prints it with
the outpost read stage. A caret key's watch keeps the calls of checks
that found nothing and prints them with the caret wait stage. A UIA
dialog's text runs on the outpost's `verbatim_uia::BoundedClient` for a
deadline of its own, its calls added back to the worker's count
(`verbatim_uia::calls::add`) for one hand-over of tens of microseconds.

UIA's connection timeout is 2 seconds and its transaction timeout 5 in
every Verbatim process (`verbatim_uia::CONNECTION_TIMEOUT` and
`TRANSACTION_TIMEOUT`); only an unanswered call is affected.

## Cold and steady state

- Cold: the first focus in a window after its outpost started. The
  window's backend has no verdict yet, so it is probed, and there is no
  previous focus, so the ancestor walk goes to the root.
- Steady state, also called warm: a focus in a window already
  arbitrated, after a focus whose ancestors are kept; the walk stops at
  the first ancestor already held. Moving within one group or one list is
  the steady state.
- For object navigation, warm means the node is held with its live
  element. Every navigation in the ledger is warm.

## Cancelled traces

A trace is cancelled when the outpost handles its entry and publishes
nothing: a focus whose element no longer has the keyboard focus, a focus
in a window the other backend owns, an MSAA focus where nothing has the
focused state, an older focus superseded in its batch, or an entry the
watchdog abandoned at its deadline. The ledger counts completed traces
only.

- A cancelled trace's calls, and calls made after an entry's last
  publish, are logged at debug level on the worker's "handled" line (the
  `unpublished` field) with the trace id, never shown in the latency
  ledger and never added to the next entry's count.
- An abandoned entry's calls are never sent: its worker is replaced, and
  the `Abandoned` reply carries no counts. A query withdrawn before it
  started made none.
- In a batch of several focus events, the newest is handled first and
  older ones only if it is dropped; each counts its own calls.

## The floor and the ratio

- An operation's floor is its minimum call count from this ledger times
  the cost of one cross-process call, calibrated against the same
  application in the same run.
- The ratio is the measured outpost read time divided by the floor. Near
  one, the time went to unavoidable round trips; a large ratio means
  extra calls, calls slower than a round trip (a fetch that builds a large
  cache), or work between calls.
- The end-to-end suite calibrates against each scenario's target and
  reports each stage as a ratio, saved with its artifacts. It is a report,
  not a gate.
- `verbatim-inspect latency` prints each stage's time and the outpost
  read's calls by kind; it has no calibrated per-call cost, so it says the
  floor needs calibration instead of printing a ratio.

## verbatim.exe's relay

Every fact the listener sees and every request Core makes passes through
verbatim.exe, so its share is a stage of its own in the latency line
(`docs/design/communication.md`, section 10.1; `docs/tooling/inspect.md`
lists the stages). Budget: verbatim.exe's own stage at most 1 to 2 ms,
and its added hop no worse than the platform's floor for crossing
processes, every exception attributed.

- Its own time is measured on its monotonic clock alone, from taking a
  fact in or queuing a request to its relay thread beginning the write,
  and travels as a duration (`EventTiming::core_relay_us`), so it does not
  depend on clocks agreeing across processes. The spans either side use
  the clock every process shares (`verbatim_process::clock`).
- On the way back the relay stamps when it read each outpost frame, and
  the reducer parses it, separating the pipe from the wait for the
  reducer.
- The reducer parses every outpost frame on its own thread, whatever its
  size, with no wake-up of its own: a few microseconds for the common
  messages. A terminal's text and colours travel in the frame's raw
  section (`docs/design/communication.md`, section 4.2), so a
  whole-document read of 9,031 rows takes 0.08 to 0.12 ms for 0.29 MB of
  Latin text and 0.09 to 0.18 ms for 0.36 MB of Cyrillic, and an ordinary
  read with its screen's 30 rows of 120 colour runs 0.03 to 0.04 ms
  (release, parsing alone, mean of 50, six runs of the ignored
  `measure_conversion`, measured 2026-10-28); keys and other outposts'
  frames posted after it wait for it.
- A fact held while its outpost starts is reported as "waiting for the
  outpost to start". An application's outpost is usually a spare started
  ahead of need (`docs/design/focus-pipeline.md`, section 19), so the wait
  is the assignment and the hook, a P50 of 1.4 to 3.6 ms in a debug build;
  starting an outpost from nothing takes 36 to 118 ms, about 40 ms typical,
  in a debug build before it takes its first event.
- Nothing on the relay's path logs: stamps travel in the messages.

## The ledger

Each entry gives the fewest calls the operation can be done in (the
minimum), the count today as the ratchet measures it, the target, and
what bounds the count. The fixture is mockapp's
`tests/fixtures/counts.json` unless named: a window with a named group of
two buttons and a list of three items, the first selected. Counts are UIA
calls, MSAA calls and window messages; a kind not mentioned is zero.
Provider calls are mockapp's hits, pinned beside the counts.

How a UIA focus is measured: the listener reads it from the event's own
element the classic way, with no remote operation or connection
(`docs/design/listener-without-connections.md`, sections 2 and 3): after
NVDA's checks, the cache refreshed, a container's selected item, and one
`GetParentElementBuildCache` per ancestor up to a known one, the
desktop's root or the process boundary. The test runs the listener's
reader and a real outpost in its process, so the figures are both
together. After reporting a focus without text the outpost finds its own
copy of the element, one `ElementFromHandle` and one remote operation or
classic walk, outside the worker's count but inside the hits, as are the
subscriptions' moves. Each is measured with remote operations, the
default, and with the classic walk (`uia.remote_operations = false`).

Rules that hold for every UIA focus:

- The calls before the read are NVDA's focus handler's, in its order
  (`docs/design/focus-pipeline.md`, section 18): the sender's nearest
  window (`NormalizeElementBuildCache`, answered from the event's cache
  for a sender that is a window), costing mockapp 31 provider calls for a
  button or the list and 22 for a list item; the comparison with the
  focus Core holds (`CompareElements`, NVDA's duplicate rule, which
  reaches no provider but is a client call); and the sender's keyboard
  focus read plainly (`CurrentHasKeyboardFocus`, NVDA's last check), 3
  provider calls. It is read plainly because inside a remote program
  Windows Terminal's second focus identity answers that it has the focus
  where NVDA's plain read answers that it has not (section 16.8).
- A repeated focus on the element that still has the keyboard focus is
  dropped after those three calls (`uia_repeated_focus_costs_exactly`);
  the window's verdict is kept, so the window is asked nothing.
- A focus on a console window's own element is dropped after the
  console's provider check, made once per console window by the listener
  from the held sender: four calls from the console window's element and
  two from the text area, whose text pattern the event's cache holds
  (`a_console_windows_own_focus_is_never_reported`,
  `a_console_text_areas_focus_is_checked_from_the_text_area`).
- A menu item that no pattern makes checkable has its legacy MSAA state
  read, as NVDA 2027.1 reads it for menu items only: one UIA call more in
  the listener's read, the focus-now answer and a navigation step
  (`a_menu_items_legacy_state_is_read_with_its_focus`: 7 calls for a menu
  item two steps below the previous focus's window), and none in the
  outpost's remote program
  (`a_windows_forms_menu_item_is_checked_from_the_remote_read`). It is
  not cached for every element, because that doubled every focus's
  provider work.
- The classic walk compares each child it passes by runtime id alone and
  fills only the element found, because filling a window's title bar has
  UIA send its thread `WM_GETTITLEBARINFOEX`, which Windows answers for
  one window at a time across the desktop.
- The steady state's 10 `WM_GETOBJECT`s come from the outpost's
  `ElementFromHandle` and the listener's and outpost's separate clients
  each connecting; recorded as a follow-up, not changed.

### A focus change, UIA, steady state

- Minimum: 5 UIA calls for a sender without a window of its own, as
  mockapp's are, and 4 for a sender that is a window: the nearest window,
  the comparison, the keyboard focus, the sender's cache refreshed
  (`BuildUpdatedCache`), and one step up to the previous focus's group,
  whose own ancestors are reused. The listener makes no remote operation,
  because only the application's outpost may send one into it
  (`docs/design/listener-without-connections.md`, section 2).
- Today: 5 UIA calls, all the listener's, either way the outpost walks;
  the minimum, met. 312 provider calls in all with the outpost's remote
  walk and 306 with its classic walk, of which 9 `GetPatternProvider`
  calls are the grid item's row and column (3 for each of the three
  elements read alone, one of them the window element of the outpost's
  walk), 2 `GetPropertyValue` calls the ancestors' automation ids, and
  one `GetPropertyValue` the provider description that the outpost's
  lookup of the route's window caches for the proxy rule
  (`docs/design/listener-without-connections.md`, section 5).
- Beside the calls, local work only and no wake-up added: the reading
  threads' watchdog judges once whether the user moved on from the read's
  window (`GetGUIThreadInfo`, `GetWindowThreadProcessId`, class and
  process reads), on the wake-up the read's start makes; and when the new
  focus is inside the old, the intake moves the old focus's queued state
  changes ahead of the read (`docs/design/focus-pipeline.md`, section
  12.4), delaying the report by their handling, 0.2 to 1.8 ms per property
  change in Settings' Date and time page (debug).
- Bound: one step per ancestor until a known one, so the count grows only
  with how far the focus moved up the tree.

### A WinEvent focus in a UIA window, and the read when no UIA focus follows

- Per focus: nothing. The worker drops a `WinEvent` focus copy in a UIA
  window and records a gap, with no lock or call; the listener's UIA focus
  closes it. The gap's bound rides on the worker's wait for its next entry
  (`Intake::next_with_gap`), so it sets no timer. A listener focus message
  carries about 13 bytes more, and one local `GetAncestor` is made per copy
  and per focus read.
- Per gap that comes to a read, once a second passes with no UIA focus or
  at once when it was dropped for having no window above it
  (`docs/design/listener-without-connections.md`, decision 11): one worker
  wake-up and the focus-now query's read (`read::focused_control_in`),
  local checks first so a hung application is not called, then one
  `GetFocusedElementBuildCache`, whose request caches one property more,
  the element's process, checked locally against the application with no
  extra call (an element of another process is read no further), the
  enrichment, the MSAA continuation
  above the process boundary, and one `ElementFromHandleBuildCache` more
  for a window's own element read without its handle, as the console
  host's is (`docs/uia-bugs.md`). The late UIA focus then costs one
  connection more, its element found by path (`find_focus_element`).
- Per event the worker judges a window for, an MSAA event, a foreground
  change, a menu opening, a UIA selection, a listener's read or drop: the
  check that its application serves the window (`served_by`), local calls
  and no call into any application; an event naming another
  application's window is dropped there, before any probe
  (`a_focus_event_naming_another_applications_window_is_not_read`).
- Per listener drop on its verdict ("not UIA" or "unknown") in a window
  the outpost judges UIA, a disagreement of the two processes' verdicts:
  the same focus-now read, at once, in the drop's place, which answers the
  gap, so the gap does not read again
  (`a_focus_the_listener_dropped_in_a_window_the_outpost_judges_uia_is_read`).
  In a window the outpost judges MSAA too, its verdict alone: kept, from
  the class lists, or its own probe when it has none.
- Reads made in vain are accepted: in `measure_focus_explorer` a gap can
  come to a read after the focus moved to another process's window, whose
  foreground event comes about 200 ms early; the read ends at its local
  check, about 110 µs.
- The harness's idle barrier waits while a gap is open, at most about 12.2
  seconds for one focus marker (the bound and the listener's 11-second
  `READ_DEADLINE`), within its 15-second idle timeout.
- Target: none per focus and at most one read per gap, met.

### A focus behind other objects' events

The outpost handles events in the order they arrive, as NVDA does, so an
MSAA focus waits behind the events queued before it, as NVDA's does; a
UIA focus is judged by the listener as it arrives
(`docs/design/focus-pipeline.md`, sections 5 and 9). How long the wait
can be: in mockapp with every provider call answered 20 ms late, a focus
behind ten selections was taken up about 610 ms after the query before it
was answered (measured 2026-10-07).

### A focus change, UIA, cold

- Minimum: 1 window message and 4 UIA calls for a windowless sender, 3
  for a sender that is a window, plus one step per ancestor up to the
  desktop's root: the nearest window, the window's provider probed once in
  its lifetime (the listener's raw probe), the keyboard focus, the cache
  refreshed, and the steps; no comparison, since no focus was taken
  before.
- Today: 6 UIA calls and 2 window messages: the listener's nearest
  window, keyboard focus, cache, three steps (to the group, the window and
  the desktop's root), and raw probe, and the outpost's own raw probe of
  the window, which it judges with its own verdict before it reports the
  listener's read (`docs/design/focus-pipeline-rework.md`, "Who owns each
  duplicated fact", the verdicts), once in the window's lifetime. 359
  provider calls with the outpost's remote walk and 350 with its classic
  walk, 12 of them the grid item's row and column and the ancestors'
  automation ids, and 5 the outpost's probe.
- Probes per new window, each process's (step 1's baseline,
  `experiments/focus-pipeline-rework-measurements/probes-per-new-window.txt`):
  in a UIA window, the listener's 1 at the first focus and the outpost's
  1, where the outpost made none before by copying the listener's
  verdict; in an MSAA window not on the class lists, unchanged, the
  outpost's 1 at its first MSAA focus and the listener's 1 for UIA's proxy
  focus, then none while each process's "no" lasts, 500 ms.
- Target: the minimum. The walk could stop at the desktop's root, which
  is known locally, instead of asking for its parent.
- Bound: the depth of the focus below the desktop.

### A focus below the process boundary, UIA

The focus is in a window whose parent belongs to another process, as a
Settings page is inside `ApplicationFrameHost`'s frame. The ratchet makes
it a cold focus with two mockapps, one's UIA window a child of the
other's MSAA window (`uia_focus_chains_stop_below_the_process_boundary`).

- Minimum: the listener's calls of a cold focus, none a step into the
  parent's process, and the outpost's MSAA reads of the parent's client
  area and the chain above it.
- Today: the listener's 5 UIA calls and 1 window message, its two steps
  ending at the child's window. The outpost makes 8 MSAA calls, the client
  area and the chain above it, and no probe of the parent
  (`docs/design/listener-without-connections.md`, section 4.3); above a
  parent with a UIA provider, 21 MSAA calls and no UIA call
  (`uia_focus_chains_continue_through_msaa_above_a_parent_with_a_provider`).
  Either way it probes the child's window, its own application's, once:
  1 window message.
  UIA's client side still connects to that parent when a read returns the
  child window's element, once remotely, twice classically and once in a
  focus-now query, each connection a `UiaRootObjectId` `WM_GETOBJECT`
  with a nonzero wParam, one with a zero wParam, 6 `ProviderOptions`, 2
  `HostRawElementProvider` and 2 `Navigate`; a parent without a provider
  receives the nonzero message alone, an owner nothing. The listener's
  read sends the parent's process 2 such `WM_GETOBJECT`s.
- Target: no message to the parent, which UIA's client side does not
  allow while the child window's element is held; accepted as a known
  limit (`docs/design/listener-without-connections.md`, decision 13).
- Bound: the chain above the boundary, read once.

### A focus change into a list, UIA

The focus lands on the list, and its selected item is read with it.

- Minimum and today: 7 UIA calls, all the listener's: a steady-state
  focus's five, and the selected item's two (`SelectionPattern2`'s
  `FirstSelectedItem`, fetched with the `Selection` pattern, and
  `BuildUpdatedCache` on it); met. 352 provider calls with the outpost's
  remote walk and 347 with its classic walk, 15 of them the grid items'
  row and column, the selected item's among them, and the ancestors'
  automation ids.
- Bound: one selected item per focus.

### A focus change onto a combo box without the Value pattern, UIA

The focus lands on a combo box whose value is its first selected item's
name (`verbatim_uia::map::value_from_selection`,
`crates/mockapp/tests/combo_box.rs`); then another item is selected.
Pinned by `uia_combo_box_costs_exactly`, both ways.

- Minimum: 7 UIA calls for the focus, as for a list; 1 for the
  selection, the search that finds the item among the combo box's
  children with its name and keyboard focus.
- Today: the focus 8, all the listener's: a steady-state focus's five and
  the selected item's three through the `Selection` pattern, since
  mockapp's combo box has no `SelectionPattern2`. A combo box with the
  `Value` pattern reads no selected item (`FocusQuery::value_available`);
  a looping selector is read as a list, 7. The selection 2, in the
  outpost: the controlled-list test every selection is given first, as
  NVDA's `event_selection` reads `ControllerFor` first, and one
  `FindFirstBuildCache` over the children.
- Target: the minimum for the focus, met by a provider with
  `SelectionPattern2`; for the selection, the test and the search in one
  remote operation, 1.
- Bound: one search per selection, over the combo box's children.

### Arrowing through a list, UIA

- Minimum and today: 5 UIA calls, a steady-state focus on a windowless
  element; the walk stops at the list; met. 327 provider calls with the
  outpost's remote walk and 324 with its classic walk, 11 of them the
  grid item's row and column and the ancestors' automation ids.

### An object navigation step, UIA

Next sibling and parent, from a node the outpost holds.

- Minimum: 1 UIA call: one program takes the step, fills the neighbor's
  cache and finds the node's nearest window, for correcting the
  neighbor's backend (`verbatim_uia_rops::navigation_step`).
- Today, remotely: 1 each, met, from the held element without refreshing
  it: the program fails at once when the element is gone, and the outpost
  then searches for it. 91 provider calls for the next sibling and 92 for
  the parent. A step from a top-level window is taken classically, since
  a program's walk ends there.
- Today, classically: 3 each: the held element's cache refreshed, which
  proves it still answers, its nearest window, and the step; 141 provider
  calls for the next sibling and 142 for the parent.
- Bound: one step per command.

### A focus change, MSAA, steady state

MSAA has no cache requests or remote operations, so each property is a
call: reading an object is 7 calls (name, value, role, state,
description, keyboard shortcut, location), 6 for an ancestor, whose
location is not read (`docs/design/uia-shapes.md`, decision 9), and
reaching a parent 3 more (`accParent`, the `QueryInterface` for its
`IAccessible`, and `WindowFromAccessibleObject`).

- Minimum: 18 MSAA calls: the acquisition from the event's address (1),
  `accFocus` for a focused child to report instead (1,
  `docs/parity/windows-and-processes.md`, "A control's own focus with a
  focused child"), the read (7), and one ancestor hop (9) that recognizes
  the previous focus's container.
- Today: 28: the focus 9 (its role and state, read first for NVDA's
  checks, kept for the read) and a walk to the root, two ancestors at 9
  each and an `accParent` that finds none. The walk does not stop at the
  group, because oleacc hands out a new wrapper for every object reached
  through `accParent`, whose identity matches no kept node. mockapp
  answered 32 provider calls, 10 of them `accParent`, most from
  `WindowFromAccessibleObject`'s own walk.
- Target: 18, by recognizing an ancestor by its address and identity
  string, as the registry already does for objects acquired at an
  address.
- Bound: the depth of the focus below its window's root.

### A focus change, MSAA, cold

- Minimum and today: 28 MSAA calls and 1 window message, the steady
  state's count, since nothing is recognized either way, plus the probe;
  met. `WindowFromAccessibleObject` answers no window for mockapp's
  ancestors, and the walk keeps the window it is in. mockapp answered 33
  provider calls.

### A focus change into a list, MSAA

- Minimum: 28 MSAA calls: the focus (8), `accFocus` on the list (1,
  NVDA's check whether a list's focus moves to its focused item), the
  first ancestor recognized (9), and the selected item: `accSelection`,
  its `IAccessible`, its window and its seven properties (10).
- Today: 29: the walk asks the window for its parent because the window
  is not recognized. mockapp answered 30 provider calls.
- Target: 28. Bound: one selected item per focus.

### Arrowing through a list, MSAA

- Minimum: 18 MSAA calls. Today: 28, as for a steady-state focus and for
  the same reason; mockapp answered 32 provider calls. Target: 18.

### A focus on a tree view item, MSAA

A nested item of a real comctl32 tree view under a Windows Forms class
name (`tests/fixtures/tree_view.json`, `msaa_tree_view_costs_exactly`),
after a focus on another root item. Only the client's calls are pinned,
since the provider is comctl32's.

- Today: 33 MSAA calls and 12 window messages: the item's position
  (mapping its child id to its item and counting its siblings both ways),
  its check state from its state image (`TVM_GETITEMSTATE`, as NVDA reads
  it), and its logical parent through the control's messages, read as an
  ancestor, so its siblings and location are not. The focused-state check
  comes first, as in NVDA; the test gives the control no keyboard focus,
  so the check reads the tree view's, its window's and mockapp's root
  state, 7 MSAA calls a real focused item does not need.
- Bound: the item's depth in the tree.

### A focus on a report view item, MSAA

An item of a real list view in report view
(`tests/fixtures/list_view.json`, `msaa_list_view_costs_exactly`), after
another item, named by its four columns as NVDA names it.

- Today: 28 MSAA calls and 14 window messages: the view, the header and
  its item count, the column order, each column's rectangle (4), each
  shown column's text (3), the header of each but the first (2), and the
  item count for the position.
- Bound: the number of columns.

### A tree view item expanded or collapsed, MSAA

- Today: 2 MSAA calls, the acquisition and the item's state, and 7 window
  messages for an expand of the focus (its child count, 5, and its check
  state and item, 2); 2 window messages for a collapse. A change on an
  object that is not the focus is not read.
- Bound: the item's children.

### An event on an object that is not the focus, MSAA

NVDA speaks a name, value, description or state change only for the
focus, so the outpost tells the object from the focus by identity before
reading anything.

- Today: the acquisition and its role only, the role to know whether it
  is a progress bar, whose value changes are reported off the focus
  (mockapp answers `get_accChild`, `accParent` and `get_accRole` once
  each); a progress bar then has its states, value and location read,
  what NVDA's progress bar behavior reads. The focus's own change costs 2
  MSAA calls, the acquisition and its value. mockapp answers a
  `WM_GETOBJECT` for each acquisition, 2.
- Bound: one property per event.

### A name, description, or state change on the focus, MSAA

NVDA reads only the property its event names. So does the outpost once
the object is the focus or an ancestor, applying the role-dependent rules
of a full read from the role it was last read with
(`msaa_state_change_costs_exactly`).

- Today: 2 MSAA calls each, the acquisition and the one property; mockapp
  answers the acquisition's `WM_GETOBJECT`, `accParent` and
  `get_accChild` and the property's method.
- Bound: one property per event.

A window's verdict of no UIA provider is renewed every half second, as
NVDA renews it, because nothing tells a client that a window began to
answer the probe. A focus whose window's verdict has run out costs the
probe again, 1 window message and 1 `WM_GETOBJECT` more. The ratchet's
outpost reads time from the test, so no count depends on the test's
speed. The two focuses it pins this way are arrows between list items
whose previous focus was itself reached by an arrow, 31 MSAA calls each:
one `accRole` more than an arrow whose previous focus was reached from
the list, for a reason not yet known. A probe that times out after 3
seconds is kept as "no" by an outpost for the rest of its entry and for
500 ms from the probe's end, so an entry sends the window one probe at
most; the listener keeps nothing of it, and drops the focus as unknown.

### A selection, MSAA

Core speaks a selection only inside a focused list or tab control, and a
selection of the focus or an ancestor as a change of its state
(`a_selection_is_read_in_full_only_inside_a_focused_list`,
`msaa_state_change_costs_exactly`).

- Today: outside a focused container, the acquisition alone (mockapp
  answers a `WM_GETOBJECT` and a `get_accChild`); inside one, a full
  read; the focus selected, 2 MSAA calls, the acquisition and its states.
- Bound: one item per selection event.

### A selection inside a list the focus controls, UIA

A search box whose ControllerFor relation names a list, and an item of
the list selected (`crates/mockapp/tests/controller_for.rs`).

- Minimum and today: 1 UIA call, one remote operation, whatever the
  number of controlled elements; met. An item found in none is answered
  with the first controlled element's current selection, read in the
  same operation, where the classic calls take three more. A selection
  naming the focus itself reads no current selection: one comparison of
  runtime ids per UIA selection while the focus is a UIA element, and no
  lock.
- Bound: the controlled list's elements, walked inside the provider (see
  "The instruction limit").

### An event from a window not related to the foreground window, MSAA

The outpost applies NVDA's acceptance test against the foreground window
before anything else (`docs/parity/focus-events.md`, "Event acceptance";
`msaa_state_change_costs_exactly`).

- Today: an event from an unrelated window is dropped before any call;
  mockapp answers nothing. With the focus's window in front, a state
  change costs the probe, the acquisition and the focus's states, 2 MSAA
  calls and 1 window message, and mockapp answers 2 `WM_GETOBJECT` and 3
  provider calls.

### A repeated or unfocused focus event, MSAA

- A focus event at the address of the focus last reported, a whole
  object, with no foreground change and no other application's focus
  queued since, is NVDA's duplicate: only its role (for NVDA's list
  redirect) and `accFocus` are read, and mockapp answers 4 provider calls
  and the acquisition's `WM_GETOBJECT`.
- A focus event on an object neither focused nor inside a focused one:
  its role, `accFocus`, and its and each ancestor's state are read, as
  NVDA reads the states first; mockapp answers 12 provider calls and the
  `WM_GETOBJECT` for an object two levels below its root.

### Entering a dialog, MSAA

A focus moves into a message box from outside it, so the dialog's own
text is gathered as its description (`docs/nvda/object-model.md`, "A
dialog's own text"; `tests/fixtures/dialog.json`).

- Minimum and today: 42 MSAA calls and 1 window message, met: a cold
  focus's 28 MSAA calls and 1 window message, and 14 for the text: the
  child count and children (2), each of three children's `IAccessible`
  and role (6) and states (3), and the question's name, value and
  description (3). mockapp answered 50 provider calls. A focus moving
  within a dialog with its own window does not read it again; mockapp's
  dialog has none, so it is a new node each time.
- Bound: the dialog's children.

### A dialog's text, UIA

The same message box through UIA, from a dialog element the outpost holds
as the `Kept` shape, of which nothing is read
(`docs/design/uia-shapes.md`, section 6).

- Minimum and today: 1 UIA call per container, met: its children with
  every property the rules read cached, in one `BuildUpdatedCache` scoped
  to the children alone, as NVDA's is. mockapp answered 135 provider
  calls: 26 `ProviderOptions`, 33 `GetPatternProvider`, 43
  `GetPropertyValue`, 11 `HostRawElementProvider`, 9 `Navigate`, 6
  `GetRuntimeId` and 7 `FragmentRoot`.
- A top-level message box's first child is the title bar UIA adds, read
  without whether it is off the screen, because that read has UIA send
  `WM_GETTITLEBARINFOEX`, about 6 ms alone and seconds under load
  (`uia_dialog_window_text_reads_nothing_of_its_title_bar`).
- Bound: one call per container read.

### A tree dump, UIA

The control plane's tree dump (`verbatim-inspect dump-tree`), through
`verbatim_uia_rops::tree_dump` (`crates/mockapp/tests/uia_tree.rs`).

- Minimum: one call per listed element above the deepest level, its
  children read in one scoped `BuildUpdatedCache`; fewer remotely, each
  program reading up to 60 children.
- Today: classically one call per listed element, 3 for mockapp's list of
  two items; remotely 1 for that list and for any tree of up to 60
  children read; met. Locations and off-screen states are read only when
  the dump asks for locations, so a dump sends no `WM_GETTITLEBARINFOEX`.

### The shell's tray and taskbar list, UIA

File Explorer's outpost walks the notification area or the taskbar for
Core's list dialog (`outpost/shell_items.rs`): one call for the window's
element and one per walk step, each element filled with the `ShellButton`
shape, which holds whether it is off the screen but not its rectangle
(`docs/architecture.md`, "Bulk reads carry only cheap properties"). A
click walks again and reads `Place` for each button with the item's
runtime id or name, normally one. Measured on 2026-10-25 against this
machine's taskbar, from a separate process running the outpost's walk,
100 runs each:

- A taskbar list: 23 calls, 19 elements filled, 14 buttons listed; 20.1
  ms at the median and 22.5 at the 95th percentile.
- A click: 24 calls with one candidate, 25 with two; the `Place` read 0.4
  ms at the median (0.5 at the 95th percentile).
- A tray list: 2 calls listing nothing, since this build shows its icons
  under the taskbar's element.
- Bound: the surface's elements.

### An object navigation step, MSAA

- Minimum and today: 10 MSAA calls for a next sibling (`accNavigate`, the
  neighbor's `IAccessible`, its window and its seven properties) and 10
  for a parent (`accParent` in place of `accNavigate`); met. mockapp
  answered 9 provider calls for the next sibling and 15 for the parent, 8
  of them `accParent` from `WindowFromAccessibleObject`'s walk. With a
  theme reporting descriptions and shortcuts off, a next sibling is 8.

### A key through the keyboard hook

- Minimum and today: no call into an application. A key that would be
  held back or deferred behind a held one reads `GetForegroundWindow` and
  one lock-free load of Core's `ForegroundView`; every press that reaches
  the application and is not Verbatim's loads the view once more, to
  learn which thread's keyboard layout a console window's keys take.
  Neither waits or allocates. The hook sends each gesture on an unbounded
  channel (`docs/crates/verbatim-input-windows.md`), waking the router
  once; the channel allocates a block of 31 slots about once every 31
  sends, and a queued gesture holds about 70 bytes.

### An interrupt

Stopping speech on a key press makes no call into an application, so the
ratchet does not measure it.

### A notification from an application with no outpost

Core speaks a UIA notification only from the focus's application, the
shell's window-snap results excepted, so the supervisor starts no outpost
for one (`DeliveredFact::may_start_outpost`); the snap results still
start one.

### Speech only the reducer makes

Every utterance and earcon comes from the reducer (`docs/architecture.md`,
section 2), and the speech pipeline reports an utterance with index marks
that ends unheard.

- No call into an application. An utterance with index marks costs the
  speech queue thread a list of its marks and a table insert and remove;
  each one cut off or failed costs one post to the reducer and one
  recorded step, two or three per key that cuts say-all or terminal output.
  Lock keys, the time, the clipboard's result, a shell item gone, the
  theme page's previews and the start, exit, error and "not responding"
  sounds take one thread hop to the reducer and one step; a menu or the
  task switcher ending is one step.
- A synthesizer host's audio: each chunk is one frame, a small JSON body
  and its samples as the frame's raw section
  (`docs/design/communication.md`, section 4.2). The synth thread decodes
  a chunk of 1,323 samples (60 ms at 22,050 Hz) in 0.2 microseconds and
  one of 4,410 in 0.28, and the host encodes them in 0.6 and 2.0 (release,
  best of 30, measured 2026-10-28).

### A caret move, UIA

A caret key the application has already handled (Right Arrow), answered
with the caret's line, its offset in it and the character with its
formatting under the default theme, which reports spelling and grammar
errors and links, against mockapp's `tests/fixtures/text.json`, whose
"beta" is a spelling error.

- Minimum and today: 1 UIA call, one program
  (`verbatim_uia_rops::caret_read`); met. The program makes inside the
  provider the classic reads' provider calls, one copy more, and the
  import of the element and its text pattern.
- Classically 15: the selection, whether it is empty, the evidence (a
  comparison with the caret Core knew), the line (a copy,
  `ExpandToEnclosingUnit`, `GetText`), the offset (a copy,
  `MoveEndpointByRange`, `GetText`), the character's annotation types and
  link (a copy, `ExpandToEnclosingUnit`, one `GetAttributeValues`), and
  the caret read again with its comparison, because the application can
  move the caret between classic calls (the console host writes a typed
  character between them). A theme with font, its attributes or colour on
  adds one attribute read each classically and nothing remotely.
- Bound: one line per key.

### A caret key's watch, UIA

A caret key the application has not handled when the request arrives:
the watch's first check reads the caret, finds no evidence and leaves the
watch open, and the worker goes on with its queue
(`docs/crates/verbatim-outpost.md`); the check the next caret event
prompts reads the caret again and answers. A key that moves nothing makes
the first check alone, silently.

- Minimum and today: one round trip per check, nothing for waiting; met.
  1 remotely per check, the whole caret read; classically 15; in the edit
  control 5 window messages for a check that finds nothing and 6 for one
  that answers.
- With something selected, when a caret report after the key replaced the
  kept selection, up to two `CompareEndpoints` round trips more (none in an
  edit control). A possible reduction: record in the caret history whether
  a report changed the selection.
- Over a selection, any key, Backspace and Delete included, compares with
  the kept selection in the same round trip, reading the old selection's
  text as its ends now hold it, up to 513 UTF-16 code units
  (`verbatim_uia_rops::MAX_COMPARED_TEXT`, one more than the 512 characters
  from which a selection is spoken as a count): inside the program through
  UIA, one call classically, one `WinEvent` sync in an edit control.
- In the console host an open watch re-reads the caret every 5 ms for up
  to 50 ms, a documented workaround (`docs/design/key-holding.md`, "The
  console host's caret poll"): at most 10 round trips and 10 worker
  wake-ups a key. In `measure_keys_conhost` (release, 2026-10-26) every
  open watch was answered within 5 ms, caret reads by watches were 1.34 a
  key, and caret keys reached speech at P95 9.0, P99 9.4 and maximum 9.4
  ms.
- Nothing waits behind the key: a focus delivered just after a Right Arrow
  the application ignored is taken 0.74 to 0.85 ms after the key
  (`a_focus_change_during_a_caret_watch_is_handled_at_once`).

### A caret report, UIA

The caret reported without a key (`CaretMoved`), after every typed
character, compared with the selection of the outpost's last report for
the node, for a selection no key made. Measured with the caret moved one
character with nothing selected, then "al" selected from the caret.

- Minimum: 1 UIA call remotely; classically 8, a caret move's without the
  comparison or the formatting, when nothing is selected before or after;
  with a selection, the comparison and the change's text.
- Today: 1 remotely in either case, met: the comparison, the change's
  text, and the new selection's text up to 513 units, read only when the
  selection's ends moved, are in the program.
- Classically 8 with nothing selected and 21 for "al": the selection's
  caret (`GetCaretRange`), two comparisons of the ends with the old
  caret's (a selection from nothing is one whole change, as NVDA speaks
  it, with no comparison placing it), the change's text (a copy, its two
  ends moved, its text), the selection's text, where the selection
  starts, since `TextPattern2`'s caret can be its other end (4, and 3 more
  for a start on an earlier line), and, at its line's start, whether it
  is the text's start (2). Inside a line it costs 19. When the last
  selection held something, what its ends now hold is read too, up to 513
  units, so an edit is said in general terms (`docs/parity/text.md`,
  "TextInfo-equivalent layer"); a longer text is compared by its first
  513 units and its length. A possible reduction classically: a selection
  on the caret's line starts at the caret's offset less its length, two
  calls fewer.
- Beside the calls: a registry lookup per report and a copy of the
  selection's two ends (for UIA two references to ranges already held). A
  terminal's caret report does neither.

### The caret read before a held key

A caret key pressed while the focus has text is held by the hook until
the outpost has read the caret (`TextOp::ReadCaret`,
`docs/design/key-holding.md`); the read is a caret report's that compares
no selection (`caret_moves_cost_exactly`).

- Minimum and today: 1 UIA call remotely, 8 classically, 5 window
  messages in a Win32 edit control; met. A possible reduction: skip the
  watch's first check when the read was a few milliseconds before.
- Wake-ups: two more of the hook's thread per held key, and the read's
  round trip: roughly 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time
  per held key in the terminals (debug; `docs/design/key-holding.md`,
  "Costs"). A key deferred behind a held key wakes the reducer once more
  (`ShellCommand::DeferredKey`) and is sent again with one `SendInput`.
- Each foreground change costs the listener the integrity check's local
  calls and one message to Core, which publishes the hook's view with one
  lock-free store; the hook's thread is not woken.
- A held key waiting for the key ahead's watch costs the reducer one
  wake-up at the 100 ms bound (`Effect::BoundHeldKeyWait`), one of its
  existing deadlines; the last of 15 auto-repeats of a key that moves
  nothing reached the text box 1.05 to 1.06 s after the release
  (`measure_held_repeat_text_box`).

### The shell's staging windows

On Alt+Tab, Task View and the Windows+X menu, File Explorer gives the
focus or the foreground to windows that only stage the switch
(`docs/parity/windows-and-processes.md`, "The shell's staging windows").
Core decides what is said for them (decision D6), so they are read like
any window.

- Minimum: NVDA reads each as its focus object and says nothing.
- Today: an MSAA focus there costs 13 or 14 MSAA calls and one window
  message, a foreground change's read 8 MSAA calls and one window
  message, and the listener judges and reads a UIA focus there. Only a
  focus change's window facts carry the window's class
  (`WindowFacts::class`).
- Target: the minimum. The staging reads take 1.5 to 14 ms, and up to
  about 540 on a slow File Explorer (debug, 2026-10-24); such a read can
  delay the switcher's own focus past the target's, so the switcher is
  announced after it closed. Switching between two console host windows,
  the target was in front 450 to 890 ms after Alt's release for a quick
  switch and 18 to 33 ms with Alt held, against 525 to 811 and 22 to 28
  with no Verbatim.

### Ordering focuses across processes

Core drops a focus from another outpost observed before the focus it has
applied (decision D14; `docs/parity/focus-events.md`, "Stale focus
events"). Every process stamps an observation with one read of the
performance counter, in microseconds, for every event kind.

- Minimum and today: one clock read per observation and no wait; every
  message carries one stamp. With no millisecond field, an event message
  is 31 bytes smaller for a focus and 16 for any other event, a listener
  fact and its relay to the outpost and a focus-now answer 31 bytes
  smaller, Core's `CoreFocus` notice 10, and a menu's end 3 bytes larger.
  No thread, timer, message or wake-up.
- What it does not catch: a switcher focus Windows raises after the
  console's (11 ms after its foreground event and 15 ms after its focus)
  happened later, so no ordering drops it; Core's refusal of the
  switcher's pane covers it ("The task switcher's frame").

### The focus Core holds, told to the judges before a read

Decision D14, "Where the rules run": the listener and the outposts judge
events against the focus Core holds (`SupervisorToOutpost::CoreFocus`),
never a copy of the focus they last reported.

- Minimum: one notice per change of Core's focus to each process that
  judges against it, none on the event path's threads.
- Today: after every focus the reducer judges, taken or refused, one
  `OwnerEvent` to the supervisor's owner thread, which writes one
  `CoreFocus` to the listener, one to the judged event's outpost, and on a
  change of application one to the outpost left; the outpost's pipe thread
  takes it without waking the worker. Local calls added: the acceptance
  test's foreground relation reads all three facts, and the listener one
  window property (`GetPropW`) per UIA focus, to drop a focus in Core's
  hidden frame before its read. While a focus awaits judgement, a repeat
  is read in full and Core's duplicate rule drops it.
- Over the three `measure_focus_` scenarios a key to its speech queued at
  a median of 0.6 ms and a 99th percentile of 6.1 (debug, 2026-10-25).
- Target: the minimum, but for the reporting outpost's notice of a refused
  focus, which keeps a refused focus from standing as the one it judges
  against.

### The task switcher's frame

Core refuses the UI Automation focus of the task switcher's frame in File
Explorer (`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's
pane"), by the element's class and the focus's application.

- Minimum and today: no cross-process call, thread, timer, message or
  wake-up. The class comes from the element's cache (the listener's
  `ListenerEvent` shape and the outpost's `Node` shape). Every snapshot is
  24 bytes larger for the class and every window's facts for the
  application's name. The name is read once per application and console
  client with `QueryFullProcessImageNameW` and kept with its process held
  open, so it never outlives the process; per focus change, one map lookup
  under a lock, `GetExitCodeProcess` and a zero-wait
  `WaitForSingleObject`, and one allocation.

### The task switcher's items, and a grid item's row and column

Core drops the containers of a switcher item reached with Alt held, and
speaks a grid item's row and column (`docs/parity/focus-announcement.md`,
"Table cell coordinates").

- Minimum: the parent's automation id from its cache, Alt with one
  `GetAsyncKeyState`, and the row and column as two properties.
- Today: no cross-process call added; met. Every ancestor's request holds
  its automation id (`verbatim_uia::Ancestor`). The requests of an element
  read alone (the focus, its selected child, a navigation step's
  neighbour) hold the `GridItem` pattern's availability, row and column
  (`verbatim_uia::Cell`), which costs mockapp 3 `GetPatternProvider` calls
  per such element; no read of many elements asks for them. Every snapshot
  is 40 bytes larger. One `GetAsyncKeyState` per reported focus change.

### The system caret's events

The system caret's location change or show (`OBJID_CARET`), handled as a
caret event on the focus (`docs/parity/text.md`), measured from the
outposts' logs over three runs each of `notepad_editing` and
`conhost_editing` (2026-10-20).

- Minimum: no call for an event about a focus that reports its own moves;
  otherwise the caret report's, 1 UIA call remotely.
- Today: each event wakes the outpost's event thread, which drops it while
  the focus has caret events of its own (a UIA focus once it raised one, a
  focused console host terminal); any other wakes the worker, which makes
  NVDA's acceptance test and the readability check with local calls only,
  then reads the caret unless a read since covers it. In `notepad_editing`
  the hook delivered 47 to 52 a run and 3 woke the worker, so 16 to 18
  caret reads a run against 16 with each move read once; in
  `conhost_editing` it delivered 7 and none woke the worker.
- Target: the minimum, met but for the first move after a focus in an
  application that raises both, whose system caret's event comes first.

### The focus-now answer on a text field

The focus-now query's answer for a text field, asked for at Core's start,
after a replaced outpost, and after a menu closes or Alt+Tab, carries the
caret read (`FocusedControl::text`), so the field's line is spoken.
Pinned in `text_field_focuses_cost_exactly`, mockapp's UIA document with
nothing selected. The MSAA edit control's answer is not pinned, because
the MSAA read takes the system's keyboard focus, which no test on its own
desktop can give mockapp.

- Minimum and today: the query's read, 2 UIA calls and 1 window message,
  plus the text pattern's fetch and one program remotely (2), or the
  fetch and 35 calls classically (36): 4 UIA calls and 1 window message
  remotely, 40 and 1 classically; met. A selection adds what it adds to a
  reported focus. A terminal's follow-up read is outside the answer's
  count.

### The caret report after a focus, UIA

The caret read of a text focus with its line's formatting: mockapp's
first line has four stretches (bold "alpha", a space, the misspelt "beta",
the line feed). It is counted in the focus's message ("A focus on a text
field"); this entry is the read alone.

- Minimum and today: 1 UIA call remotely, met; 35 classically, with the
  default theme or every indication on: a caret report's 8, a collapsed
  copy of the line (2), for each stretch a copy, `MoveEndpointByUnit`, a
  comparison with the line's end, its text and its attributes in one
  `GetAttributeValues` (5 each), the walk moved on after each but the
  last (3), a `MoveEndpointByRange` to cut the last stretch at the line's
  end, and one call asking the line for its annotation types and the
  attributes whose support is not yet known ("Text attributes").
- The provider's attribute reads with the default theme: the line's
  annotation types and link, then two per stretch, 10, the first time;
  once mockapp's text is known not to support links, 5. With every
  indication on, eleven attributes: 11 asked of the line and 44 of the
  stretches the first time, then 1 and 28 once the four unsupported ones
  are known.
- Time against mockapp, debug build, 200 reports, three runs: classically
  3.16 to 3.46 ms at the median, remotely 0.40 ms (0.50 with every
  indication).
- Bound: the line's stretches, at most 64 a span.

### A focus on a text field

The whole cost of a focus on a text field, its caret read carried in the
focus's one message (`FocusChanged`'s `text`), each the first focus in its
window (`text_field_focuses_cost_exactly`). It is paid for every text
focus the outpost handles, superseded or not, as NVDA reads an object's
text for every focus it queues. A UIA focus's parts:

- the listener's read: 5 UIA calls and 1 window message;
- the outpost's search for its element, made before the report for a focus
  that may have text: 2 UIA calls and 1 connection remotely, 4 calls and
  the connection classically;
- the enrichment: 1 UIA call;
- the caret read: 1 remotely; classically 35 for mockapp's document, 43
  with a selection, and 17 for its terminal;
- the selected text, only when something is selected, a protected field's
  too: 1 remotely, 4 classically.

Counts, with the outpost's verdict on the window made before the focus,
as it is once any event of the window has been handled; at a cold focus
its probe adds 1 window message (above):

- Minimum: each part's own: 9 UIA calls, 1 window message and 1
  connection remotely with nothing selected, 10 with a selection.
- Today: a UIA document (`text.json`) 9 remotely and 10 with a selection,
  protected or not, each with 1 window message and 1 connection, so the
  minimum is met remotely; classically 45, and 57 with a selection. A UIA
  terminal (`terminal_reading.json`) 9 remotely and 27 classically, each
  with 1 window message and 1 connection. A classic edit control through
  MSAA 37 MSAA calls and 6 window messages. A focus without text is
  reported before its search.
- Measured on 2026-10-22 with `cargo xtask measure` (debug, info level,
  five runs; minimum, P50, P95, P99 and maximum in ms). mockapp's field as
  the first focus in a new application (`measure_focus_new_application`):
  first word event to queued 28.0, 34.2, 44.1, 46.6, 46.6; outpost read
  18.9, 20.7, 25.8, 28.2, 28.2 with 4 calls; the line follows by 0 to 1.
  Of the read, the search for the outpost's element, its first connection
  to a new application, takes 19.0 to 24.3 ms and the caret read 0.8 to
  1.0: a defect against the costing. `measure_focus_text_box` (Windows
  Forms edit controls through MSAA, 8 or 9 calls): first word 14.1, 20.5,
  51.7, 55.9, 56.8, its tail the wait for the reducer.

### A caret key that selects, UIA

Shift with Right Arrow, from a collapsed caret at the start to "alpha"
selected, answered with the line, the character and the text newly
selected.

- Minimum and today: 1 UIA call, met: the caret read's program compares
  the selection's ends with the old ones and reads each change's text
  when they moved. Classically 30: the caret read's calls and the same
  comparisons and text reads, only when the selection moved. A selection
  from nothing or to nothing is one whole change, as NVDA speaks it.

### A review cursor line, UIA

The review cursor's next line (numpad 9), from a position the outpost
reported.

- Minimum and today: 1 UIA call (`verbatim_uia_rops::text_units`), met;
  classically 8 (a copy of the position, expanded to its line, collapsed,
  moved; a copy expanded to the line reached and its text; its `Culture`).
  The first review command and report current object on an edit field
  read the line at the caret in one program, classically 9.

### A review cursor word inside a line, UIA

The word at a position inside a line Core holds, found by moving a copy of
the line's start by the text before the position.

- Minimum and today: 1 UIA call, the program trying each character count
  a provider may use, met; classically 12.

### Say-all, UIA

Reading mockapp's three lines from the caret and moving the caret as each
is reached.

- Minimum and today: 1 UIA call for every batch read ahead
  (`TextOp::ReadAhead`, twenty units), its end found in the same program,
  and 1 per caret move; met. Classically 24 for the three lines, and 4 per
  caret move.
- A batch of twenty lines classically, against mockapp's text replaced by
  forty-five short lines (`uia_say_all_batches_cost_exactly`): 109 for the
  first batch and 108 for each later one. Each line after the first costs
  5 (a copy of the line before, collapsed, moved, expanded, its text), since
  UIA keeps a collapsed range collapsed when it moves and expanding
  normalizes from the start alone. The language is read once a batch over
  its whole range (3 calls), and line by line only when that read answers
  "mixed": 33 for `languages.json`'s four lines in three languages.
- Time, debug build against mockapp in its own process, 200 runs, three
  runs: classically 10.0 to 10.6 ms at the median (11.7 to 13.0 at the
  95th percentile), remotely 1.0 to 1.6.

Say-all reads the next batch once fewer than 10 pieces are left to speak
(Dickson, 2026-10-07; `docs/crates/verbatim-core.md`, "Say-all"), since
ten pieces last far longer than a batch's read. Against Windows 11
Notepad (debug, a 300-line document, eSpeak NG at 376 words a minute, a
piece a line) a batch read in 3.3 ms at the median remotely and 21.1
classically, and the gaps between pieces were 1.5 ms at the median and 14
at the 95th percentile either way, against NVDA's 1.2 and 30.2.

### The caret's location, UIA

Report caret location.

- Minimum and today: 1 UIA call, met; classically 5 (the caret's two
  reads, a copy expanded to the character, its bounding rectangles).

### The selected text, UIA

The selected text read for a copy, or with a focus's or the navigator's
announcement (`ReadRange`).

- Minimum and today: 1 UIA call, met; classically 7, of which the range's
  read is 4; at a focus, whose caret read already fetched the selection,
  those 4 alone.

### The review copy's selection, UIA

The review copy's first press (Verbatim+F10, `TextOp::Select`), answered
with how the selection changed: mockapp's collapsed caret, then "alpha"
selected from the marker through the character at the review cursor. Core
sends both ends as points past their characters (`TextThrough`), since
only the outpost can tell which comes first.

- Minimum and today: 1 UIA call, met: one program reads the selection,
  selects (`TextRangeSelect`), reads it again, compares and reads each
  change's text and the selection's text up to 513 units. It executes 163
  instructions (`range_selections_execute_exactly`). Classically 29.
- When the program fails after its select, the selection is made: the
  program still returns the selection before, and only the after-read is
  made classically, never the select again; by the code's count for
  mockapp's case, 11 classic calls more, and 4 for each further change.
- In a Win32 edit control 6 window messages, also the minimum: `EM_GETSEL`,
  `EM_SETSEL` and `EM_SCROLLCARET`, `EM_GETSEL` again, and
  `WM_GETTEXTLENGTH` and `WM_GETTEXT` (`uia_text_requests_cost_exactly`,
  "UIA review copy select"; `caret_moves_cost_exactly`, "Edit control
  review copy select").

### Wall-clock gain

What remote operations save, measured on 2026-10-07 on this machine,
release build, against mockapp in its own process, 200 runs of each
operation as the worker makes it, with other builds loading the machine.
Medians held within about 0.2 ms across three runs; the worst cases vary
with load, and the worst remote navigation step is the process's first
program, which creates the remote operations machinery. mockapp's
providers run on one window thread, so a real application gains more.
Median, 95th percentile and worst in ms, remote against classic:

- Navigation, next sibling: 0.93, 8.8 and 109 against 1.62, 7.7 and 19.6.
- Review cursor's next line: 0.40, 0.49 and 8.4 against 0.77, 4.7 and
  7.3.
- Review cursor's word inside a line: 0.38, 0.43 and 4.9 against 0.60, 4.6
  and 5.2.
- Line at the caret: 0.43, 4.0 and 10.1 against 0.76, 5.4 and 7.5.
- Say-all's caret move: 0.29, 0.35 and 4.4 against 0.40, 4.9 and 9.2.
- The caret's location: 0.36, 0.40 and 0.44 against 0.54, 8.8 and 24.4.
- The selected text: 0.39, 0.46 and 4.3 against 0.68, 6.2 and 13.9.
- A caret key that selects: 0.45, 0.55 and 1.1 against 2.5, 21.5 and 56.8.
- A caret move: 0.41, 0.45 and 0.96 against 1.34, 19.4 and 34.8.

Say-all's batch is timed under "Say-all, UIA".

### A caret move, Win32 edit control

- Minimum: 5 window messages: `EM_GETSEL`, `EM_LINEFROMCHAR`, two
  `EM_LINEINDEX`, and `EM_GETLINE`. A rich edit control from version 2.0
  takes the same with `EM_EXGETSEL`, `EM_EXLINEFROMCHAR` and
  `EM_GETTEXTRANGE`.
- Today: 6: the selection is read again before answering, because read by
  parts the caret can be read before the control handles the key and its
  line after (a Backspace said nothing); when the two differ, the
  control's caret event answers instead.
- Target: 5, which needs a message reading the caret and its line together;
  none exists.

A word in a rich edit control from version 2.0 is found with two
`EM_FINDWORDBREAK` messages, three when the first word found ends at or
before the offset, and the text's length (`EM_GETTEXTLENGTHEX`) once per
request, to find no word at the end, as NVDA does.

An answer that compares selections waits once for the `WinEvent` thread to
take every event raised before it (`Nudger::sync`) and scans the intake
once for the control's value change, so an edit is said in general terms;
no call to the application. The sync posts one thread message and raises
one marker with `NotifyWinEvent`, because a posted message alone can
overtake an event raised before it: about 25 to 36 µs at the median
(release, 20,000 syncs of an idle thread), and no `WM_GETOBJECT`. A sync is
made only here, in the caret report below, and in `Outpost::settle`.

### A caret report, Win32 edit control

- Minimum and today: 5 window messages with nothing selected before or
  after, the comparison of offsets being local; 7 for a selection no key
  made, its text read with `WM_GETTEXTLENGTH` and `WM_GETTEXT`; met. A
  comparison in general terms reads the new selection's text with the
  same two messages instead.
- Beside the calls: the two offsets kept and, for a report that compares a
  selection, one sync and one intake scan. Each value change of the focused
  control locks the edit controls' anchors once.

### Typed-character echo

The echo itself makes no call: the hook translates the key locally
(`ToUnicodeEx`). The caret event the typing causes is one caret report.

### A terminal output line

A focused terminal's text changed: the outpost reads it and sends the read
to Core, whose rules judge it (`docs/design/terminal-model.md`, "The read"
and "Where it runs"): one snapshot (`verbatim_uia_rops::terminal_snapshot`)
of the text from a screen above the last read's screen to the document's
end in one `GetText`, with the screen, where the next read starts, the
document's first row, and the caret and its line. Measured against
mockapp's `tests/fixtures/terminal.json` (`tests/terminal.rs`).

- Minimum and today: 1 UIA call for every read, the first after the focus
  included (`new_output_is_said_line_by_line_remote`). Classically, one
  call per provider method of the snapshot's classic implementation.
- Bound: one call whatever the history and however many lines changed; the
  text read grows only with the output since the last read.
- A caret key is held for a read before it is sent on, which in a terminal
  is a snapshot. A terminal's text change forgets no selection and takes no
  lock on the anchors.

### A terminal's caret

A terminal raises no caret event for every character echoed, so every
read takes the caret and its line in the same program
(`verbatim_uia_rops::SnapshotQuery::caret`), and Core takes it as its
caret. A caret event of either terminal reads the terminal itself. No call
more.

### A terminal flood

What a flood costs the terminal, measured on 2026-10-07 on this machine,
debug build, ten thousand lines written as fast as PowerShell writes them
into a full scrollback of 9,001 lines, each time one flood's stopwatch:

- The console host takes 1.6 to 1.9 seconds with no screen reader. It
  raises about 13,600 text change and caret events per flood at about 40
  µs each to a listening client, so merely subscribing makes it 2.2 to 2.6
  seconds, a cost every subscriber pays. About 0.15 seconds of that is the
  subscription's base cache request; a one-property request stalled the
  console host for minutes, so it is not used.
- Windows Terminal takes 1.4 to 1.6 seconds with no screen reader and 1.5
  to 1.7 with Verbatim; it raises about 60 text events per flood.

The outpost reads whenever the text changed, coalesced with no fixed delay
(`docs/design/terminal-model.md`, "Reading rules", rule 1), and logs each
read's time, calls and size at debug level; reads and bytes per scenario
are under "A terminal's reads, live". Core asks for new output as it hands
a group's last line to speech, so "skipped" starts within 1 ms of that
line ending in both terminals. The flood scenarios' wall-time ratio
compares output reported against output off, and the outpost reads either
way, so it does not measure the reads' cost; a starved machine distorts
it (twelve busy threads made the flood 75 to 99 seconds in Windows
Terminal with no screen reader). Core's flood policy keeps a lost read's
kind on each waiting line, so handing a line on walks nothing; a lost
read counting none walks the queue once.

### A terminal's rotation

How far a full history rotated between two reads is settled by the read
itself (`docs/design/terminal-model.md`, "The read" and "The rules in
detail"): every read starts a screen above the last read's screen, so that
screen stays inside it for a rotation of up to two screens less a row. A
rotation past that keeps no row of the last read, and the whole document
is read once.

Measured on 2026-10-24 on this machine, release build, 120 by 30, a full
history of 9,001 rows: in the console host the read program's snapshot
takes 0.45 to 1.06 ms for a rotation of 0 to 1,000 rows, 7,318 UTF-16
units; a rotation of 61 rows or more adds the whole document's snapshot,
25 to 29 ms and 1.1 million units. A terminal's first read takes only the
screen; when the document goes on above it, it is read again at once with
a screen of rows above, one such snapshot each time the outpost begins
reading a terminal.

The memory's and rules' time from a read's text to its speech, by
`crates/verbatim-text/tests/terminal_cost.rs`, an ignored test (`cargo
test --release -p verbatim-text --test terminal_cost -- --ignored
--nocapture`): a 120 by 30 terminal of each kind with a full history,
every read timed 31 times with a fresh reader. Measured on 2026-10-26 on
this machine, medians over both terminals and two runs, moving by up to
0.3 ms between runs and 0.6 for cases marked bimodal (the heap handing
back pages or not):

- A rotation of 58 rows, settled in one read: 37 to 42 µs.
- A rotation of 61 rows, and of 1,000 rows, each a lost read and the whole
  document of 1.1 MB: 1.2 to 1.4 ms.
- The same 1,000 rows of lines one cell short of the width: 1.3 to 1.9,
  bimodal; of Cyrillic text, 1.9 MB: 1.7 to 1.8.
- An overflow of 10,000 lines keeping nothing of the last read, about
  9,000 lines spoken: numbered lines 1.0 to 1.1 ms; lines one cell short
  1.4 to 1.5; Cyrillic 1.8 to 2.0; lines of two rows 1.2.
- A clear and a reprint of 8,905 rows, read whole: 1.0 to 1.1 ms.
- 1,000 lines of one repeated line, an ordinary read: 46 to 49 µs.
- 1,000 lines walking through the last screen's texts, so only the screen
  is aligned: 1.0 ms.
- The subsequence's worst case, 8,000 lines drawn from the last screen's
  texts then 60 new lines: 1.7 to 1.9 ms.
- Freeing the steps afterwards, the caller's: 16 to 60 µs for a whole
  document.

The bound is what the work must touch: hashing and copying every row's
text once (about 0.1 ms a megabyte each), the vectors kept per row, and in
the subsequence each of the last read's rows compared with each row
sharing its text. No case allocates a block of a megabyte or more, because
such a block takes pages of its own at about 120 µs a megabyte to fill and
free; a read's rows share strings of about 64 KB or 1,024 rows
(`docs/crates/verbatim-text.md`, "`Text`").

### A full-screen program's exit

Measured on 2026-10-25 on this machine by a probe outside Verbatim (a
release build making the read program's calls one at a time), 120 by 30
with a full history of numbered lines.

- The read after a full-screen program closes starts at the range kept at
  the alternate screen's top, which lies above the next read's start, so
  it is the whole document: 1,098,120 UTF-16 units in the console host and
  1,101,779 in Windows Terminal, its `GetText` 28.6 to 30.5 ms and 11.3 to
  11.6 ms while the terminal's writer waits, against 0.44 to 0.60 and 0.77
  to 1.29 for two screens. It is paid once per close over a full history,
  and stays (Dickson, 2026-10-25), because a start no higher than two
  screens above the end would miscount a fresh or cleared screen's output.
- The console host's scroll update counts rows the text moved up in its
  window, which equals the rotation only with a full buffer outside a
  scroll region (8,950 events while 9,000 lines filled the buffer with the
  first row unchanged), so Verbatim does not use it (Dickson, 2026-10-25).

The aside screen (`docs/design/terminal-model.md`, "Reading rules", rule
8) adds no call, wake-up or timer: at most 60 hashes and distances while
one is held, and one hash lookup per row of a read that keeps nothing of
the last read.

### A terminal's reads in Core

Each read crosses the pipe as a snapshot and is aligned on the reducer
(`docs/design/terminal-model.md`, "Where it runs"). Measured through
`reduce` by `crates/verbatim-core/tests/terminal_cost.rs`, an ignored test
(`cargo test --release -p verbatim-core --test terminal_cost -- --ignored
--nocapture`): a 120 by 30 terminal of each kind with 9,001 numbered rows,
each input carrying a read timed 31 times from a clone of the state
before. The reducer's time is the whole step; the parse, of the input's
JSON, runs on the reducer's thread before the step and is timed apart;
sizes are the unpadded snapshot's JSON. Measured on 2026-10-26 on this
machine, three runs, medians in µs over both terminals:

- An ordinary read of one new line: 1,109 to 1,135 bytes, of which the
  screen 264 to 272; reducer 56 to 68; parse 4 to 9.
- A rotation of 61 rows: the read, 34 to 42, keeps nothing and asks for
  the whole document, 71.8 to 72.0 KB; reducer 1,109 to 1,272; parse 123 to
  141.
- A lost read after 10,000 lines: the read 37 to 45, then the whole
  document, 81.6 to 81.9 KB, every line handed to the flood policy; reducer
  1,664 to 1,845; parse 125 to 146.
- A full-screen program's exit, its read the whole document: 71.7 to 72.0
  KB; reducer 1,082 to 1,282; parse 125 to 131.
- A clear and a reprint of 8,905 rows: the read 35 to 40, then the whole
  document, 160.1 KB; reducer 1,437 to 1,888; parse 140 to 210.
- `clear; dir`, a clear and 100 lines: the read 1,823 to 1,863 bytes, 22
  to 23 µs, and the whole document of about a hundred rows, 2,421 to 2,461
  bytes, 40 to 41.

Every case is within the reducer's budget of 2 ms a step
(`docs/design/communication.md`, decision 9) at the median, so the
alignment of a whole document stays on the reducer (Dickson, 2026-10-26,
who would rather optimize than move it off as an effect). The slowest of
the 31 runs of the lost read and of the clear and reprint reach 1.9 to 2.7
ms, the runs in which the heap takes back pages it had returned.

Where a whole document's time goes (median µs, three runs a terminal):
making the rules' read 2 to 37, the text taken unpadded and copied once;
the split into rows 265 to 350; the alignment 321 to 460; the rules' pass
with the speech of 9,000 lines 469 to 689, each line's speech sharing the
read's text (`Speech::Line` holds a `Text`); handing speech to the flood
policy nothing, every text moved; freeing the step 30 to 270; the flood
policy 123 to 178, a line past the backlog's bound never made a string.
Page faults: 330 to 400 for the lost read, 16 to 390 for the clear and
reprint. What remains is the memory's and rules' work on every row ("A
terminal's rotation") and the page faults of the heap taking pages back;
Core's own part is about 0.2 to 0.5 ms.

The outpost's conversion of a read into its snapshot, in one pass into
kept buffers, and its writing in the event sent to Core
(`terminal::unpadded_utf16`; the ignored `measure_conversion` in
`crates/verbatim-outpost/src/terminal/tests.rs`, release, 2026-10-28): a
whole document of 9,031 rows of 120 cells 1,557 to 1,795 µs with at most
one page fault, in Latin or Cyrillic text; an ordinary read of two screens
16 to 20 µs; 132,096 bytes of buffers kept after a whole document.

### A terminal's reads, live

Measured on 2026-10-26 on this machine, release build, every read sent to
Core, one run of each scenario, before a terminal's text moved to the
frame's raw section.

- Messages: one per read, about 1.6 to 3.3 KB for an ordinary read; a
  flood in the console host sent 1,911 messages and 6.01 MB, in Windows
  Terminal 252 and 0.69 MB; a full-history flood 3,906 and 12.33 MB, and
  154 and 0.81 MB; the key scenarios 2,420 and 5.92 MB, and 2,991 and
  6.30 MB.
- The reducer's time per input carrying a read: a live read 101 to 116 µs
  at the median and 182 to 406 at the 99th percentile, an answer to
  Core's request 53 to 83. Over the 2 ms budget: the whole document after
  a lost read, 2.0 to 2.3 ms, and 3 of the 1,822 reads of `conhost_flood`,
  the largest 4.7 ms; no key scenario's step (the largest 1.35).
- Wake-ups: sending every read to Core costs 2.6 percent more wake-ups of
  all processes over the console host's flood and 2.5 percent over Windows
  Terminal's than keeping reads in the outpost (Dickson, 2026-10-26). A
  clock rewriting one line once a second for 30 seconds: the console host
  read 117 times, the second identical read of each tick not sent; Windows
  Terminal 58; the reducer woke 94 and 101 times. Pi and a streaming
  Claude Code reply were not measured.

Colours: in `measure_keys_conhost` 439 of 911 keys found a read ended
after the key without colours and read again at once, about one remote
operation and message more for every other key; 39 of 911 in Windows
Terminal. A key costs as many coloured reads as its window holds: 1.0 to
1.33 in the console host and 0.90 to 2.09 in Windows Terminal.

From the last key the hook saw to the speech queued, announcements of one
character, a space or "blank", minimum, P50, P95, P99 and maximum in ms:

- The typing echo: `measure_keys_conhost` (752) 1.3, 4.1, 6.4, 10.0, 17.8;
  `measure_keys_windows_terminal` (623) 0.8, 21.4, 54.3, 58.2, 59.3.
  Windows Terminal's median of about 21 ms is one frame.
- Windows Terminal's caret keys and Backspace (74): 3.7, 5.5, 44.9, 262.2,
  288.0. Its tail is Backspace at 33 ms a key: each held key waits for the
  watch of the key ahead (`docs/design/key-holding.md`, "Keys behind a
  held key"), answered only when it draws 20 to 80 ms later, so the keys
  queue; the tail varies from run to run. The console host's caret keys
  are under "A caret key's watch, UIA".

### A terminal's upward search

Moving to a command mark above the rows Core keeps searches for the mark's
line's first row, unpadded (`verbatim_uia_rops::terminal_find`; Dickson,
2026-10-26), outward from the mark's place, alternately below and above,
in one remote operation of at most 20 matches, each checked by its line's
hash in Core, because the console host's `FindText` also matches a longer
row with the same start. A failing call ends that direction, including
the range Windows Terminal 1.24.12741 returns for nothing found, whose
reads fail. A hit within a row is passed over uncounted, so the search is
bounded at 100 `FindText` calls (`verbatim_uia_rops::FIND_CALLS`),
because a one-letter needle would otherwise walk the whole history.

- Instructions: at the bound with 19 matches taken, its worst case, 3,065
  against mockapp, under a third of the limit.
- Time: one `FindText` costs 0.1 to 0.2 ms classically wherever the text
  is, and about 3 inside a program, against either terminal with a full
  scrollback of 9,001 lines. The search is not measured in a real
  terminal; it is estimated at about 60 ms for 20 calls and about 300 at
  the bound of 100. Against mockapp, debug, a search at the bound took 1.2
  ms remotely and 61 classically.

The reading itself never searches: its read program keeps a range where
the next read starts.

### A terminal review command above the last read

Figures in this entry and the next follow from the code and were not
measured. Nothing in either is on the event or speech path.

- A review command whose place lies above the rows a terminal's memory
  keeps makes at most one history read, one remote operation
  (`review::History`, `verbatim_uia_rops::terminal_snapshot`) from two
  screens above the place to the document's end (Dickson, 2026-10-25).
- Finding the place's line by its hash adds up to a screen of 64-bit hash
  comparisons below the place, a few microseconds; the second screen above
  lets up to a screen of output scroll while the read is out without a
  second read, at about 0.12 ms.

### A terminal's command marks

- Setting a mark costs nothing on a read beyond comparing each read's new
  rows' hashes, a few microseconds, while a command's line waits to be
  marked; numbering a read is one counter in the outpost (Dickson,
  2026-10-26).
- A move between marks costs one fresh read per press and a walk over the
  marks and rows in the reducer; a mark above the rows Core keeps adds one
  search per mark tried ("A terminal's upward search"), an estimated 60 ms
  when its line is the nearest of 20 rows starting with its text and at
  most about 300. A press makes at most three searches
  (`review::MARK_SEARCHES`), so it waits at most about 900 ms, and never
  asks for the same answer twice.

### A terminal's status bar

"Report status bar" costs one fresh read per press and a walk up the
memory's last screen in the reducer; the third press's copy is the
shell's clipboard write. Nothing is added to any other read (Dickson,
2026-10-26).

### A terminal's colours

Every read of a focused terminal that begins in a key's window, the 300
ms after it, reads the screen's colours too, each row's runs of one
foreground and background (`docs/design/terminal-model.md`, "Lists and
colours"; `docs/crates/verbatim-uia-rops.md`, "The colours").

Each row is checked whole, a row of several colours in chunks of 16
cells (`verbatim_uia_rops::COLOUR_CHUNK`, the fastest of 8, 16, 30 and
120 on Pi's screen), and a chunk of several cell by cell, so a plain row
costs one check (`docs/crates/verbatim-uia-rops.md`, "The colours", gives
why). Reading every cell separately would cost about 18,000 instructions
and 55 to 70 ms in the console host for any screen.

Measured on 2026-10-24 on this machine, debug build, 120 by 30, in the
console host and the pinned Windows Terminal 1.24.12741, medians of 20
reads against the same read without colours:

- A plain shell screen (bash's, 30 rows, 30 runs): 1,349 instructions in
  the snapshot's own program; 2.6 ms more in the console host (2.9 against
  0.35) and 1.6 in Windows Terminal (2.1 against 0.48).
- PSReadLine's coloured command line (2 rows, 4 runs): 555 instructions;
  1.9 ms more in the console host and 1.2 in Windows Terminal.
- Claude Code's screen (8 rows, 8 runs): 425 instructions; 1.0 and 0.7 ms
  more.
- Pi's colourful screen (30 rows and 39 runs in the console host, 23 rows
  and 32 runs in Windows Terminal): two programs, 3,451 and 457
  instructions, and 3,451 and 163; 8.0 ms more in the console host (8.3
  against 0.35) and 4.9 in Windows Terminal (5.4 against 0.46).
- The worst case, every cell's colours differing from its neighbours',
  reverse video on every other cell and true colour throughout (3,600
  runs): six programs, the snapshot's of 4,151 instructions and five of
  4,099 or fewer (24,644 in all), 72 ms in the console host and 31 in
  Windows Terminal.

So colours cost 1 to 3 ms on an ordinary screen and up to 8 on a
colourful one, on each read in a key's window ("A terminal's reads, live"
gives the reads a key), after the key reached the program. Each check or
cell is a provider call inside the program, about 2 to 6 µs; no wake-up
is added.

## Newer UIA features

The UIA features newer than Verbatim's first UIA code, measured on
2026-10-07 on this machine, release build, against mockapp in its own
process; the counts are pinned by `call_counts.rs`.

### Event coalescing and connection recovery

Every UIA client Verbatim creates turns on `IUIAutomation6`'s
`CoalesceEvents` and `ConnectionRecoveryBehavior`, as NVDA does
(`docs/crates/verbatim-uia.md`). Both are local settings, so they add no
call and change no mockapp wait: a burst of 100 name changes is delivered
whole, a stalled window's reads time out, and creating a client takes
0.036 ms at the median.

### Event handler groups

Every `verbatim_uia::Registration` registers its subscriptions as one
`IUIAutomationEventHandlerGroup`, one `AddEventHandlerGroup` per element of
its scope, as NVDA does, so the focus listener's three desktop-wide
subscriptions (an element selected, a menu opened, notifications) are one
registration with one thread and one client. A registration runs on the
process's registering thread and makes no counted call; the provider calls
UIA makes are pinned (`uia_event_registrations_cost_exactly`), the same
with remote operations on or off.

- The listener's registration costs mockapp no provider call; registering
  it takes 9.9 ms at the median (11.9 at the 95th percentile, 30 runs).
- The outpost's focus-following property subscription is registered on the
  focus alone (`docs/parity/backends.md`, "UIA event registration"),
  because the reducer acts only on the focus's own changes, and UIA
  delivered no ancestor's event under NVDA's scope of the focus with its
  ancestors either. Moving it to a focus inside a group costs 1
  `HostRawElementProvider` and 2 `FragmentRoot` provider calls, 0.75 ms at
  the median (1.14 at the 95th percentile, 30 runs).
- A text focus's caret, text and active text position changes are one
  group on the focus (`uia_active_text_position_costs_exactly`):
  registering it on mockapp's text costs 1 `HostRawElementProvider` and 2
  `FragmentRoot`, 0.89 ms at the median. An active text position change is
  kept as a position without reading anything: no call, 0.002 ms.

### Event caches

UIA fills an event registration's cache request inside the application
for every event, so each request holds only what is read from it.

- The listener's desktop-wide registrations carry NVDA's base cache
  request, thirteen properties and the Text pattern, as the
  `verbatim_uia::ListenerEvent` shape with two more; the rest is read from
  the sender, or by the outpost from its focus's subtree in one search.
- The outpost's text registration asks for 3 properties (`TextEvent`),
  since the worker reads the text itself, and its property registration
  for 23 (`PropertyEvent`), without a node's details or its off-screen
  state (`docs/design/uia-shapes.md`, decision 9).
- Pinned on demand (`uia_event_caches_cost_exactly`), because the calls
  UIA makes to route an event vary with load: the text events' request 3
  `GetPropertyValue`; the property changes' 12 `GetPropertyValue` and 11
  `GetPatternProvider`; the listener's 15 `GetPropertyValue` and 3
  `GetPatternProvider`; each also 5 `ProviderOptions`, 3
  `HostRawElementProvider`, 3 `FragmentRoot` and 2 `GetRuntimeId`.
- A help text change and a `ControllerFor` change on the focus each read
  what they need when handled, one call, as NVDA does
  (`a_focus_s_help_text_and_suggestions_are_followed`); the first after
  Enter in Settings' search box waits 138 to 164 ms in that call while
  Settings lays out the page.

### Text attributes

The classic reads fetch a stretch's attributes in one
`IUIAutomationTextRange3::GetAttributeValues` call; the provider's work is
one `GetAttributeValue` per attribute either way. The caret's read reads
the attributes the theme asks for among those the control supports
(`docs/parity/speech-audio.md`, "Which text attributes are fetched"):
while an attribute's support is unknown the line is asked for it once,
and "not supported" stops it being asked again for that control. The
line is asked for its annotation types first, and its stretches only when
it has some, so a line with nothing to read is not walked. The counts are
pinned in `call_counts.rs` ("The caret report after a focus, UIA") and
the learning in `crates/mockapp/tests/text.rs`.

- A line of 14 stretches without spelling errors, as a terminal's line
  is, with the default theme: 9 calls classically and no walk either way.
  In Windows Terminal, which supports neither annotations nor links, the
  caret's read costs 0.43 ms remotely on a line of one stretch
  (`experiments/the-terminals-report-no-language-links-or-annotations/readme.md`,
  "What reading them costs").
- Medians, release, against mockapp, 200 reports after 20 to warm up,
  three runs: `text.json`'s line 0.35 to 0.55 ms remotely and 3.4 to 4.6
  classically (every indication on 0.43 to 0.70 and 3.4 to 4.0); the
  14-stretch line 0.30 to 0.37 and 0.82 to 1.3 (every indication on 0.47
  to 0.61 and 9.2 to 10.6, 94 calls).

### A container's selected item

A focused list's or tab control's selected item is read through
`SelectionPattern2`'s `FirstSelectedItem` where the provider has it, and
through the `Selection` pattern where it does not, classically and in the
focus's program (`docs/crates/verbatim-uia.md`). Pinned over
`tests/fixtures/ancestry.json`, whose list has `SelectionPattern2` and
whose tab control does not (`uia_selected_children_cost_exactly`).
Classically one `BuildUpdatedCache` on the container caches both the
property and the pattern, so a provider without `SelectionPattern2` costs
no extra call.

- A list's selected item classically: 2 UIA calls, 74 provider calls,
  0.41 ms at the median (0.48 at the 95th percentile, 200 runs).
- A tab control's classically: 3 calls (the cache, `GetCurrentSelection`
  on the cached pattern, the item's cache), 0.54 ms.
- Inside the focus's program: 1 call either way, 1.10 ms at the median for
  the list and 1.07 for the tab control.

## Text range conformance

Verbatim's text range code follows Microsoft's guidance ("Using
IUIAutomationTextRange", "Understanding Performance Issues When Using the
Text and TextRange Control Patterns", and the reference pages of
`ITextRangeProvider` and `IUIAutomationTextRange`); each rule is pinned
by `call_counts.rs` on both paths.

- A stretch whose attribute is "mixed" is read again by its words, and a
  mixed word by its characters, as NVDA does, up to 64 stretches a span,
  because a format unit that does not end where italics change, as both
  terminals' do, would lose them. `italic.json`, every indication on: 82
  calls classically; 1 remotely, twelve stretches and 84 attribute reads.
- A range is expanded without collapsing it first, and not collapsed after
  a move, because `ExpandToEnclosingUnit` normalizes from the start alone
  and a collapsed range stays collapsed when it moves.
- `GetText` is given a length except in a terminal's reads, bounded by
  lines (Dickson); a failed attribute read is "not supported", told apart
  from "mixed"; a backward move answered with a positive count is
  corrected, as NVDA does (`docs/parity/speech-audio.md`, "Text range
  moves").
- Left for a decision: a provider without a unit silently uses the next
  larger one, which no call detects; only the terminals' paragraph and page
  are refused.
- Not used yet: `FindAttribute`, `GetChildren`, `RangeFromChild`,
  `GetVisibleRanges`, annotation objects, `IsHidden` and
  `ShowContextMenu`.

## The instruction limit

UIA stops a remote operation that executes too many instructions, with
`InstructionLimitExceeded` (2). The limit is not published; measured on
this machine against mockapp, it is exactly 10,000: a loop of 2,498
passes runs, and one more instruction stops it
(`the_instruction_limit_is_measured`). A run that exceeds it is answered
classically for that call (`Path::Fallback`), at hundreds of calls;
programs are not made resumable (Dickson, 2026-10-07).

A count is taken by running the program's counting form
(`verbatim_uia_rops::counting`), which executes twice as many
instructions, so only programs under half the limit can be counted.
Pinned exactly in `crates/mockapp/tests/instruction_limit.rs`, worst case
first, the share of the limit in brackets:

- The focus ancestry: 2,446 (24 percent) for a list sixty groups deep,
  every ancestor returned under the depth limit of 64, with the selected
  item, whose read in a try block costs 3; 1,859 at a depth limit of 30;
  1,124 for a list whose group is known. A deeper tree adds about a dozen
  a level beyond the depth limit.
- The listener's refreshed read 2,459 at the worst case; the outpost's
  path walk 1,143 down sixty-two levels, 157 down one, and 150 to an
  element not found.
- The navigation step: 879 (9 percent) sixty-two levels below its window,
  148 right under it, about 12 a level.
- The tree dump: 4,097 (41 percent) for a run reading its sixty children
  (`CHILDREN_PER_RUN`) down `deep.json`'s sixty-two nested groups, then
  746; `wide.json`'s hundred items 2,545, then 654, 588 for each sixteen
  items, and 356.
- The controlled selection: 183 inside a group of the controlled list, 215
  for its second child, 238 for an item outside it, about 35 an element
  passed, plus 11 to read the current selection of an element not found
  and 4 for each further controlled element. About 280 elements fit under
  the limit; the controlled list is the one bound the application sets
  (Settings' search suggestions were 25).
- The caret read: 4,462 (45 percent) for a line of 64 stretches reached by
  walking a mixed stretch by words and characters, eleven attributes, and
  every selection text up to 513 units; 97 for the report after a focus on
  plain text with the default theme. Where every stretch is mixed
  (`mixed.json`), each adds 249: 2,158 for 17 stretches and 4,150 for 33,
  so 64 come to about 8,000 (80 percent), under the limit but uncountable.
  It is the one program that can come within a factor of two of the limit,
  bounded by the 64 stretches.
- Say-all's batch of twenty lines: 683 (7 percent) in three languages from
  a held position; 704 for a first batch from the caret. It does not grow
  with the lines' length.
- The review copy's selection: 163.
- A terminal's snapshot: 50 for a first read, 90 with a start and the caret
  and its line; no loop, so it does not grow with the history.
- A terminal's rows by their place: 172 (2 percent) at its largest input,
  16 runs, and 22 for one run; no loop.
- A terminal's colours on a 120 by 30 screen: 1,349 for a plain screen; in
  the worst case six programs of 4,151 (42 percent), 4,099, 4,099, 4,099,
  4,099 and 4,097. A program reads rows while a worst-case row (887 at 120
  cells) fits its budget of 4,600, so a row wider than about 300 cells is
  read alone, and one wider than about 1,300 is answered classically.
- A terminal's upward search: 3,065 at its bound, measured, not pinned.

## Memory use

Measured on 2026-10-24 on this machine, release build, default settings,
eSpeak NG, before Core kept any terminal memory: every process sampled
four times a second with `Get-Process`. Private bytes (the commit) is the
figure to compare, since outposts share most of their working set's
pages. Workloads: idle at Verbatim's Settings dialog; the console host and
Windows Terminal, 120 by 30 with 9,001-row buffers, with a small then a
full history; a 10,000-line flood in each with output reported; both
closed. Private bytes and working set, steady, in MB:

- `verbatim.exe`: 7.5 and 33.4 idle, 8.2 and 32.5 at the end; no terminal
  workload moved it by more than 0.3.
- The focus listener: 3.6 and 19.8 idle, 4.0 to 4.1 private with Windows
  Terminal.
- The outpost for Verbatim's own window: 2.2 and 13.1.
- The console host's outpost: 3.4 to 3.6 and 18.9 to 19.5; Windows
  Terminal's 3.0 to 3.2 and 17.6 to 18.0.
- Spare outposts, two at a time: 1.9 to 2.1 and 12.3 to 12.9 each.
- The synthesizer host: 2.5 and 9.3 to 9.4, floods included.

In total about 23 MB private idle, 28 MB with both terminals open, and 24
MB after; neither a full history nor a flood moved a terminal's outpost by
more than 0.3 MB.

### The terminal memory's cost, estimated

Core keeps a terminal memory (`TerminalMemory`,
`crates/verbatim-text/src/terminal_rows.rs`) and the reading rules'
per-row state (`TerminalReader`, `terminal_reading.rs`) for every terminal
it has read, each Windows Terminal tab its own, at most 16, the least
recently read forgotten first (`SrState::terminals`;
`docs/design/terminal-model.md`, "Where it runs").

Sizes on x64 for a row of 120 columns: a kept row's text is a 136-byte
heap string, shared by the memory's row, its line and the rules' line
state through 32-byte `Text`s. The memory's `Row` is 56 bytes, about 190
with its text; the reader's `RowState` 72, a line's `LineState` 176 and a
row's line start 8: about 450 bytes a row in all. Colours add 24 bytes a
run, for the screen's rows after a key. For a 120 by 30 screen with a full
history: the last read's 60 rows, about 27 KB; lingering rows, about 410
bytes each for 2 seconds, under 13 KB for a screen's worth; anchors 24
bytes each.

Once a read is handled only the screen, the rows written below it and the
screen above are kept (`TerminalMemory::trim`), so the memory does not
grow. Measured on 2026-10-25 with a counting allocator, release, 120 by 30
in the simulated console host with a full history: 21 KB after 100,000
numbered lines read 20 at a time, 20 KB after a flood of 10,000 numbered
lines read whole, 27 KB after a flood of lines one cell short of the
width. A whole-document read raises the rules' peak by 3.6 MB for numbered
lines and 5.6 MB for lines one cell short, an ordinary read by 27 and 36
KB. Core also holds the snapshot while aligning it, 72 to 160 KB; the
outpost holds the 2.2 MB UTF-16 string UI Automation returned until the
read is written, keeping each spare buffer and the pipe's write buffer to
64 KiB. The flight recorder's 8 MiB byte bound holds fewer than its 1,024
inputs only after several whole reads (an ordinary read about 1.1 KB).

The vote on the shift the last read's screen rows took runs at most once a
read, only when a history block row's text appears lower in the read, as
with rustc's `  |` rows or a repeated prompt; a stream of distinct lines
takes none
(`a_stream_through_a_full_history_takes_no_vote_on_the_screens_shift`). It
costs within an ordinary read's noise: 58 to 69 µs a read at the median
against 51 to 69 with every row distinct (release, 31 runs). The caret
line's check makes six strings of a line's length, only when the caret
moved forward on a line starting where it did.
