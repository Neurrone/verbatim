# Performance: the operation ledger

How fast Verbatim answers depends mostly on how many times it has to wait
for another process. A call into an application's process costs a round
trip through that application's message loop, and a busy application makes
every one of them slow, so the number of such calls an operation makes is
the cost that matters and the one Verbatim controls. Times vary from run to
run and machine to machine; the number of calls does not. So what CI
enforces is the counts, exactly, and times are judged against a floor
measured in the same run (see "The floor and the ratio").

This document defines what counts as a call, the words the ledger uses
(cold, steady state, a cancelled trace), and the floor; then the ledger
itself: for each operation and backend, the fewest calls the operation can
be done in, the count today, and the target.

The counts today are pinned by `crates/mockapp/tests/call_counts.rs`, which
asserts every one of them exactly, along with the calls mockapp's providers
answered. An exact count is a ratchet: a change that adds a call fails CI,
and a change that removes one must lower the number in the test and here,
in the same commit.

## What counts as a call

A call counts when it reaches the application's process. There are three
kinds, counted separately (`verbatim_model::CallKind`):

- A UIA call: a UI Automation client method that the application's
  provider answers. The fetches (`GetFocusedElementBuildCache`,
  `ElementFromHandleBuildCache`, `FindFirstBuildCache`, `BuildUpdatedCache`),
  a tree walker's `*BuildCache` steps and `NormalizeElementBuildCache`,
  `CurrentControllerFor`, `GetCurrentPattern`, and a pattern's methods, such
  as `GetCurrentSelection`, `Invoke`, or `Toggle`. Once remote operations
  exist, each `Execute` of a program counts as one.
- An MSAA call: an `IAccessible` method (`accName`, `accValue`, `accRole`,
  `accState`, `accDescription`, `accKeyboardShortcut`, `accLocation`,
  `accParent`, `accChildCount`, `accNavigate`, `accFocus`, `accSelection`,
  `accDefaultAction`, `accDoDefaultAction`), `IAccIdentity`'s identity
  string, the acquisitions `AccessibleObjectFromEvent`,
  `AccessibleObjectFromWindow`, `AccessibleChildren`, and
  `WindowFromAccessibleObject`, and a `QueryInterface` for any interface
  but `IUnknown` on an object from the application.
- A window message sent to one of the application's windows and answered
  by its window procedure: the arbitration probe (`UiaHasServerSideProvider`
  is one `WM_GETOBJECT`, and the probe's `WM_NULL` wait for a busy window),
  and a list view's or tree view's `LVM_` and `TVM_` messages.

The rules that decide the edge cases:

- One API call counts once, however many round trips it makes inside. A
  UIA fetch makes dozens of provider calls; `WindowFromAccessibleObject`
  walks `accParent` until it finds a window; `AccessibleChildren` asks for
  the count and then each child. The client's count is what Verbatim
  decides; mockapp's hit counters (`docs/crates/mockapp.md`) show what each
  call cost the application, and the ratchet pins both.
- `QueryInterface` is counted every time it asks an object from the
  application for an interface other than `IUnknown`: COM answers it from
  its proxy when the proxy already holds that interface, and otherwise with
  a round trip, and Verbatim cannot tell which. Asking for `IUnknown`, the
  object's identity, is always answered locally and is not counted.
- Local work is not counted: UIA's cached reads (`Cached*` and every
  snapshot read from an element built with a cache request), `GetRuntimeId`
  (UIA keeps an element's runtime id with it), creating a client, a cache
  request, a condition, or a tree walker, setting a timeout, reading an
  element array, taking or resolving an agile reference, and the window
  functions that read the window manager's own state rather than the
  application's message loop (`IsWindow`, `GetClassName`, `GetAncestor`,
  `GetWindow`, `GetWindowThreadProcessId`, `IsWindowVisible`, `IsChild`,
  `GetGUIThreadInfo`, `InternalGetWindowText`).
- Reference counting is not counted: COM may add a reference when an
  object is kept and release it later, in batches Verbatim does not
  control.
- Event subscriptions are not counted. They run on their own threads,
  belong to no one event, and the remote operations design keeps them out
  of programs for the same reason.

Where the counting happens: `verbatim-uia` and `verbatim-ia2` each count
the calls they make, per thread, at the call (their `calls` modules), and
the outpost counts the one read it makes itself. Each outpost has one
worker thread, the only thread that calls into the application, so the
worker's count is exactly the calls its current entry has made. The worker
takes the count when it publishes an event or a query reply and sends it
in the message's timing; Core's latency ledger keeps it with the trace, and
`verbatim-inspect latency` prints it with the outpost read stage.

## Cold and steady state

- Cold: the first focus in a window after its outpost started. The outpost
  has no verdict on the window's backend yet, so it probes the window for
  a UIA provider, and no previous focus, so the ancestor walk goes all the
  way to the root.
- Steady state, also called warm: a focus change in a window the outpost
  has already arbitrated, after a focus whose chain of ancestors it keeps.
  The walk stops at the first ancestor that chain already holds, and the
  rest is reused. Moving between two controls in one group, or arrowing
  between items of one list, is the steady state.
- For object navigation, warm means the node is one the outpost holds with
  its live element, so it is not searched for again. Every navigation in
  the ledger is warm.

## Cancelled traces

A trace is cancelled when the outpost handles its entry and publishes
nothing for it: a focus whose element no longer has the keyboard focus, a
focus in a window the other backend owns, an MSAA focus where nothing has
the focused state, an older focus superseded in its batch, or an entry the
watchdog abandoned because a call passed its deadline.

- A cancelled trace's calls are taken when its entry ends and logged at
  debug level on the worker's "handled" line (the `unpublished` field),
  with the trace id. They belong to no published trace, so the latency
  ledger never shows them and they never leak into the next entry's count.
- An abandoned entry's calls are never sent: its worker is replaced, and
  the `Abandoned` reply to an abandoned query carries no counts. A query
  withdrawn before it started made no calls.
- When one batch holds several focus events, the newest is handled first
  and the older ones only if it is dropped; each counts only its own calls.
- Calls an entry makes after its last publish are logged the same way.

So the ledger's counts are of completed traces only, and a cancelled
trace's cost is visible in the debug log, not in the ledger.

## The floor and the ratio

- The floor of an operation is the least time it could take: its minimum
  call count from this ledger times the cost of one cross-process call,
  calibrated against the same application in the same run.
- The ratio is the operation's measured outpost read time divided by its
  floor. A ratio near one says the time went to the round trips the
  operation cannot avoid; a large one says it went to extra calls, to
  calls that are each slower than one round trip (a fetch that builds a
  large cache), or to work between calls.
- Calibration is the end-to-end suite's job: each scenario measures the
  cost of one call against its own target application in the same run and
  reports each stage's time as a ratio to that floor, saved with its
  artifacts. It is a report, not a gate.
- `verbatim-inspect latency` prints each stage's time and the outpost
  read's call count, by kind. It has no calibrated per-call cost of its
  own, so it says the floor needs calibration instead of printing a ratio.

## The ledger

Each operation gives the minimum call count and the reasoning for it, the
count today as the ratchet measures it against mockapp's
`tests/fixtures/counts.json` (a window holding a named group of two
buttons and a list of three items, the first selected), and the target.
Counts are given as UIA calls, MSAA calls, and window messages; a kind not
mentioned is zero. The provider calls each operation cost mockapp are in
the ratchet beside them.

The UIA operations are measured on the test's own thread, making the same
`verbatim-uia` calls in the same order as the outpost's worker once it has
the focused element: the outpost finds that element by reading the
system's keyboard focus (`GetFocusedElementBuildCache`), which a test must
not take from the desktop it runs on. So each UIA focus count below is
given twice, as measured and with the outpost's one focused-element read
added. The MSAA operations run through a real outpost.

### A focus change, UIA, steady state

- Minimum: 1 UIA call. The outpost must have a live element, confirm that
  it still has the keyboard focus (NVDA's live `HasKeyboardFocus` check),
  and read its ancestors up to the first one it already knows, with
  Verbatim's cached property set. One remote operations program does all
  of it: it reads `HasKeyboardFocus` first and returns early if it is false
  (Option B, agreed on 2026-10-06), then walks the raw-view parents until
  it meets a known runtime id.
- Today: 2 UIA calls measured, 3 with the focused-element read: the
  focused element, its nearest window (`NormalizeElementBuildCache`, since
  the focus fact carries no window of its own for an element that is not a
  window), and one ancestor hop, which meets the group the previous focus
  was in. The two measured calls cost mockapp 95 provider calls.
- Target: 1, with remote operations. Phase 6 step 2's exit criterion is
  at most 2, asserted exactly.

### A focus change, UIA, cold

- Minimum: 1 window message and 1 UIA call. The window's provider must be
  probed once in its lifetime for arbitration, and one program reads the
  whole chain to the root.
- Today: 5 UIA calls and 1 window message measured, 6 UIA calls with the
  focused-element read: the focused element, its nearest window, the
  probe, and four ancestor hops (the group, the window, the desktop's root
  element, and a hop that finds no parent above the desktop and ends the
  walk). The measured calls cost mockapp 160 provider calls.
- Target: 1 UIA call and 1 window message. The walk could also stop at the
  desktop's root element, which is known locally, instead of asking for its
  parent; that would save one call today, before remote operations.

### A focus change into a list, UIA

The focus lands on the list itself, and its selected item is read with it.

- Minimum: 1 UIA call: the same program also reads the list's selection
  and the first selected item's cached properties.
- Today: 5 UIA calls measured, 6 with the focused-element read: the
  focused element, its nearest window, one ancestor hop (the window, known
  from the previous focus), and the selected item in three calls (the
  `Selection` pattern, `GetCurrentSelection`, and `BuildUpdatedCache` on the
  first item). The measured calls cost mockapp 143 provider calls.
- Target: 1.

### Arrowing through a list, UIA

The focus moves from one list item to the next.

- Minimum: 1 UIA call, as for any steady-state focus change.
- Today: 2 UIA calls measured, 3 with the focused-element read: the
  focused element, its nearest window, and one ancestor hop, which meets
  the list. The same as a steady-state focus change, 95 provider calls.
- Target: 1.

### An object navigation step, UIA

Next sibling and parent, from a node the outpost holds.

- Minimum: 1 UIA call: one tree-walker step built with the cache request
  returns the neighbor with every property it is announced with.
- Today: 3 UIA calls each: refreshing the held element's cache, which also
  proves it still answers (`BuildUpdatedCache`), its nearest window, for
  correcting the neighbor's backend, and the step. A neighbor with no
  window of its own needs no further call for the correction. They cost
  mockapp 141 provider calls for the next sibling and 142 for the parent.
- Target: 1. The refresh is not needed to take the step, which itself
  fails when the element is gone, and the nearest window can be read in
  the same program as the step.

### A focus change, MSAA, steady state

MSAA has no cache requests and no remote operations, so every property is
its own call: reading one object is 7 calls (name, value, role, state,
description, keyboard shortcut, and location), and reaching a parent is 3
more (`accParent`, the `QueryInterface` for its `IAccessible`, and its
window, `WindowFromAccessibleObject`).

- Minimum: 18 MSAA calls. Acquiring the focused object from the event's
  address (1) and reading it (7), then one ancestor hop (10) that reads the
  first ancestor and recognizes it as the previous focus's container.
- Today: 30 MSAA calls. The focus costs 9 (its role is read twice, once
  for NVDA's check whether a focus event names a list, and once with the
  rest of its properties), and the walk goes to the root: two ancestors at
  10 each and an `accParent` that finds none. The walk does not stop at the
  group, because mockapp answers every `accParent` with a new COM object
  and an object reached through `accParent` is recognized only by its COM
  identity. mockapp answered 38 provider calls, 14 of them `accParent`,
  most from `WindowFromAccessibleObject`'s own walk.
- Target: 18. Recognizing an ancestor by its address and identity string,
  as the registry already does for objects acquired at an address, and
  reading the focus's role once.

### A focus change, MSAA, cold

- Minimum: 1 window message and 29 MSAA calls: the probe, the focus (8),
  each of the two ancestors (10 each), and the `accParent` that ends the
  walk at the root.
- Today: 30 MSAA calls and 2 window messages: the steady-state count
  (nothing is recognized either way) plus the probe of mockapp's window,
  and a second probe, of no window at all. `WindowFromAccessibleObject`
  answers no window for mockapp's ancestors, whose root object has no
  parent, so their addresses carry window 0 and the walk asks arbitration
  about it. mockapp answered 39 provider calls.
- Target: 29 MSAA calls and 1 window message.

### A focus change into a list, MSAA

- Minimum: 29 MSAA calls: the focus (8), `accFocus` on the list (1, NVDA's
  check whether a list's own focus event should move to its focused item),
  the first ancestor, recognized (10), and the selected item: `accSelection`,
  its `IAccessible`, its window, and its seven properties (10).
- Today: 31 MSAA calls: the role read twice, and the walk asks the window
  for its parent because the window is not recognized. mockapp answered 33
  provider calls.
- Target: 29.

### Arrowing through a list, MSAA

- Minimum: 18 MSAA calls, as for any steady-state focus change.
- Today: 30 MSAA calls, the same as a steady-state focus change and for
  the same reasons; mockapp answered 38 provider calls.
- Target: 18.

### An object navigation step, MSAA

- Minimum: 10 MSAA calls for a next sibling (`accNavigate`, the neighbor's
  `IAccessible`, its window, and its seven properties) and 10 for a parent
  (`accParent` in place of `accNavigate`).
- Today: 10 each. mockapp answered 9 provider calls for the next sibling
  and 15 for the parent, 8 of them `accParent` from
  `WindowFromAccessibleObject`'s walk.
- Target: 10 each.

### An interrupt

Stopping speech when a key is pressed.

- Minimum: none: the keyboard hook and the speech pipeline are Verbatim's
  own, and nothing about the application is read.
- Today: none. The ratchet does not measure it, since it makes no call to
  count.
- Target: none.

### Typed-character echo and a terminal output line

Milestone M4 adds both. Their minimums, counts, and targets join the
ledger, and their measurements the ratchet, when M4 builds them.
