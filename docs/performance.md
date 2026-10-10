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

The provider's counts are exact only if the client under test is the only
one calling: a provider cannot tell its callers apart (UIA's calls reach it
from UI Automation's own threads in its process), and a screen reader,
another test agent, or another test's client answers mockapp's window at
times of its own. So each ratchet test runs alone in a process on a
desktop of its own (`docs/crates/mockapp.md`), where nothing else sees
mockapp's windows or their events.

## What counts as a call

A call counts when it reaches the application's process. There are three
kinds, counted separately (`verbatim_model::CallKind`):

- A UIA call: a UI Automation client method that the application's
  provider answers. The fetches (`GetFocusedElementBuildCache`,
  `ElementFromHandleBuildCache`, `FindFirstBuildCache`, `BuildUpdatedCache`),
  a tree walker's `*BuildCache` steps and `NormalizeElementBuildCache`,
  `CurrentControllerFor`, `GetCurrentPattern`, and a pattern's methods, such
  as `GetCurrentSelection`, `Invoke`, or `Toggle`. Each `Execute` of a
  remote operations program counts as one (`verbatim-uia-rops`); importing
  its elements and asking which instructions are supported are local.
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
  a list view's or tree view's `LVM_` and `TVM_` messages, and an edit
  control's `EM_` messages and `WM_GETTEXT`.

Text (milestone M4) adds no new kind. A text pattern's methods
(`GetSelection`, `DocumentRange`, `GetCaretRange`) and a text range's
(`Clone`, `CompareEndpoints`, `ExpandToEnclosingUnit`, `Move`,
`MoveEndpointByUnit`, `MoveEndpointByRange`, `GetText`, `Select`,
`GetBoundingRectangles`, `GetAttributeValue`) are UIA calls, one each: a
range is a provider object in the application, so even copying one is a
round trip. A rich edit control's structures are written into memory in
the application's process and read back (`VirtualAllocEx`,
`WriteProcessMemory`, `ReadProcessMemory`); those are calls into the
kernel, which never wait on the application's message loop, so only the
message counts.

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
`verbatim-inspect latency` prints it with the outpost read stage. Every
stage and count of a trace's line is one message's own: the message whose
reduction first asked for speech on the trace. A later message on the same
trace, such as the caret report that speaks a text focus's line, changes
none of them; until 2026-10-08 the ledger kept the last message's outpost
stages and calls, so a text focus's line showed its caret report's. For a
caret key whose watch stayed open, the calls of the checks that found no
evidence are kept with the watch and printed with the caret wait stage;
the reply's total holds them and the answering check's, so the counts
this ledger pins are each check's.

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

One UIA read is made only for a kind of element the fixture does not
have: a menu item that no pattern makes checkable reads its legacy MSAA
state live, one more UIA call, when it is the focus, the focus-now answer,
or a navigation step's neighbor (NVDA 2027.1 reads it the same way, and
only for menu items). Every other focus and step makes no such call, as
the ratchet's unchanged counts confirm. Caching the state for every
element instead cost no call but roughly doubled the provider work of every
focus change, so it is read lazily.

Focus changes and navigation steps, UIA and MSAA alike, run through a
real outpost in the test's process, handed the focus fact or the query as
the listener and Core hand them. The outpost finds a UIA focus's element
by reading the system's keyboard focus (`GetFocusedElementBuildCache`),
which a test must not take from the desktop it runs on, so the test's
outpost reads it from the test instead
(`Outpost::with_focused_element_reader`): the element mockapp reports
focused, found beforehand, counted as the one call the system read is.
The UIA focus counts below are therefore the outpost's whole count; only
the provider calls of that one read are missing from the hits. The hits
are read once the outpost has settled (`Outpost::settle`), so they
include everything it did for the focus, the move of its focus-following
property subscription to the new focus too: 1 `HostRawElementProvider`
and 2 `FragmentRoot` provider calls, made on the subscription's own
thread and counted as no call. A UIA focus is measured both ways the
outpost reads it: with remote operations, the default, and with the
classic walk (`uia.remote_operations = false` in `settings.toml`, or a
window whose elements cannot be imported into a program).

### A focus change, UIA, steady state

- Minimum: 2 UIA calls. The outpost must have a live element, confirm that
  it still has the keyboard focus (NVDA's live `HasKeyboardFocus` check),
  and read its ancestors up to the first one it already knows, with
  Verbatim's cached property set, and its nearest window. One remote
  operations program does all of that after the element is in hand: it
  reads `HasKeyboardFocus` first and returns early if it is false (Option
  B, agreed on 2026-10-06), then walks the raw-view parents until it meets
  a known runtime id, and reads the nearest window handle on the way. But
  the element itself costs a call: the focus event's sender is held by the
  focus listener, a separate process, and cannot cross to the outpost, so
  the outpost reads the focused element (`GetFocusedElementBuildCache`).
  The design's earlier minimum of 1 assumed the outpost imported the
  sender, which it does not have.
- Today, with remote operations: 2 UIA calls, the focused element and one
  `Execute`, which also confirms the focus, finds the nearest window, and
  meets the group the previous focus was in. The `Execute` cost mockapp 88
  provider calls, 91 with the subscription's move.
- Today, classic: 3 UIA calls: the focused element, its nearest window
  (`NormalizeElementBuildCache`, since the focus fact carries no window of
  its own for an element that is not a window), and one ancestor hop. The
  last two cost mockapp 95 provider calls, 98 with the subscription's move.
- Target: 2, met with remote operations: phase 6 step 2's exit criterion,
  asserted exactly.

### A focus behind other objects' events

Not a count of calls but of what a focus waits for. A failed
`explorer_folder_window` run on 2026-10-07 had the first focus in a new
File Explorer window wait 2,589.6 ms in the outpost's queue ("outpost
queue 2589.6, outpost read 20.8 (84 calls)"). Its flight recorder shows
what the worker was doing: the MSAA events Explorer raised while it built
the window, observed in the half second before the focus, state changes
of tree items (one expanded, its 23 children counted) and a value change,
reported, and ten more, whose trace ids are missing from the record,
handled without being reported. Each was read as a full snapshot of an
object the focus was not on, while Explorer answered slowly. NVDA reads such an event with one call and judges it against the
focus when it runs. Since then a focus change goes before the events of
other objects queued ahead of it (`docs/crates/verbatim-outpost.md`, the
queue), and the events of the focus and of the object focus moves to keep
their place.

mockapp's `a_focus_is_handled_before_slow_reads_queued_ahead_of_it`
reproduces it: every provider call answered 20 ms late (`slow 20`), ten
selections in a list queued behind a query, then a focus. Before the
change the focus was taken up 611.7 and 610.6 ms after the query was
answered (two runs), after the ten selections; after it, 35 and 115 µs,
before them. The test asserts the order exactly and that the focus waits
less than one of the slow application's calls after the query.

### A focus change, UIA, cold

- Minimum: 1 window message and 2 UIA calls. The window's provider must be
  probed once in its lifetime for arbitration, the focused element read,
  and one program reads the whole chain up to the top-level window.
- Today, with remote operations: 2 UIA calls and 1 window message: the
  focused element, the probe, and one `Execute`, which walks up to the
  process's top-level window and stops there, never asking for the
  desktop's root. 149 provider calls besides the focused-element read, the
  subscription's move included.
- Today, classic: 6 UIA calls and 1 window message: the focused element,
  its nearest window, the probe, and four ancestor hops (the group, the
  window, the desktop's root element, and a hop that finds no parent above
  the desktop and ends the walk). 163 provider calls besides the
  focused-element read, the subscription's move included.
- Target: 2 UIA calls and 1 window message, met. The classic walk could
  still stop at the desktop's root element, which is known locally,
  instead of asking for its parent.

### A focus change into a list, UIA

The focus lands on the list itself, and its selected item is read with it.

- Minimum: 2 UIA calls: the focused element, and the same program also
  reads the list's selection and the first selected item's cached
  properties.
- Today, with remote operations: 2 UIA calls, the focused element and one
  `Execute`. 129 provider calls besides the focused-element read, the
  subscription's move included.
- Today, classic: 5 UIA calls: the focused element, its nearest window,
  one ancestor hop (the window, known from the previous focus), and the
  selected item in two calls (`SelectionPattern2`'s `FirstSelectedItem`
  and `BuildUpdatedCache` on it). 139 provider calls besides the
  focused-element read, the subscription's move included. Through the `Selection` pattern, before, 6 and
  143 ("A container's selected item" below).
- Target: 2, met.

### Arrowing through a list, UIA

The focus moves from one list item to the next.

- Minimum: 2 UIA calls, as for any steady-state focus change.
- Today, with remote operations: 2 UIA calls, the same as a steady-state
  focus change, 91 provider calls besides the focused-element read; the
  program stops at the list, which the previous chain holds.
- Today, classic: 3 UIA calls: the focused element, its nearest window,
  and one ancestor hop, which meets the list. 98 provider calls besides
  the focused-element read.
- Target: 2, met.

### An object navigation step, UIA

Next sibling and parent, from a node the outpost holds.

- Minimum: 1 UIA call: one program takes the step, fills the neighbor's
  cache with every property it is announced with, and finds the node's
  nearest window, for correcting the neighbor's backend
  (`verbatim_uia_rops::navigation_step`).
- Today, with remote operations: 1 UIA call each, from the element the
  registry holds without refreshing it first: the program fails at once
  when the element is gone, and the outpost then searches for it. 91
  provider calls for the next sibling and 92 for the parent. A step from
  a top-level window is taken classically, since a program's walk ends
  there.
- Today, classic: 3 UIA calls each, as before remote operations:
  refreshing the held element's cache, which also proves it still answers
  (`BuildUpdatedCache`), its nearest window, and the step. A neighbor with
  no window of its own needs no further call for the correction. They
  cost mockapp 141 provider calls for the next sibling and 142 for the
  parent.
- Target: 1, met.

### A focus change, MSAA, steady state

MSAA has no cache requests and no remote operations, so every property is
its own call: reading one object is 7 calls (name, value, role, state,
description, keyboard shortcut, and location), and reaching a parent is 3
more (`accParent`, the `QueryInterface` for its `IAccessible`, and its
window, `WindowFromAccessibleObject`).

- Minimum: 19 MSAA calls. Acquiring the focused object from the event's
  address (1), asking its `accFocus` whether a focused child of it should
  be reported instead (1, `docs/parity.md`, "A control's own focus with a
  focused child"), and reading it (7), then one ancestor hop (10) that
  reads the first ancestor and recognizes it as the previous focus's
  container.
- Before the MSAA work package (2026-10-07): 30 MSAA calls, the focus's
  role read twice, once for NVDA's check whether a focus event names a
  list and once with the rest of its properties.
- Today: 30 MSAA calls. The focus costs 9 (its role and state, read
  first for NVDA's checks, are kept for the read, and its `accFocus` is
  asked once), and the walk goes to the root: two ancestors at
  10 each and an `accParent` that finds none. The walk does not stop at the
  group, because oleacc hands the outpost a new wrapper for every object
  it reaches through `accParent`, whose COM identity therefore matches no
  kept node, and such an object has no address of its own. mockapp answered 38 provider calls, 14 of them `accParent`,
  most from `WindowFromAccessibleObject`'s own walk.
- Target: 19. Recognizing an ancestor by its address and identity string,
  as the registry already does for objects acquired at an address.

### A focus change, MSAA, cold

- Minimum: 1 window message and 30 MSAA calls: the probe, the focus (9),
  each of the two ancestors (10 each), and the `accParent` that ends the
  walk at the root.
- Today: 30 MSAA calls and 1 window message (30 before the MSAA work
  package, the role read twice, and 29 after it, before the `accFocus`
  probe): the steady-state count
  (nothing is recognized either way) plus the probe of mockapp's window.
  `WindowFromAccessibleObject` answers no window for mockapp's ancestors,
  whose root object has no parent; until 2026-10-07 their addresses carried
  window 0 and the walk probed it as a second window, which it no longer
  does, since no window is now read as none and the walk keeps the window
  it is in. mockapp answered 39 provider calls.
- Target: 30 MSAA calls and 1 window message, met.

### A focus change into a list, MSAA

- Minimum: 29 MSAA calls: the focus (8), `accFocus` on the list (1, NVDA's
  check whether a list's own focus event should move to its focused item),
  the first ancestor, recognized (10), and the selected item: `accSelection`,
  its `IAccessible`, its window, and its seven properties (10).
- Today: 30 MSAA calls (31 before the MSAA work package, the role read
  twice): the walk asks the window for its parent because the window is
  not recognized. mockapp answered 32 provider calls.
- Target: 29.

### Arrowing through a list, MSAA

- Minimum: 19 MSAA calls, as for any steady-state focus change.
- Today: 30 MSAA calls, the same as a steady-state focus change and for
  the same reasons; mockapp answered 38 provider calls.
- Target: 19.

### A focus on a tree view item, MSAA

A focus on a nested item of a real comctl32 tree view registered under a
Windows Forms class name (mockapp's `tests/fixtures/tree_view.json`, the
`msaa_tree_view_costs_exactly` ratchet), after a focus on another root
item of the same tree: the item, its logical parent found through the
control's messages, and the tree view, where the walk meets the previous
focus's container. mockapp's scripted root is not read. Only the client's
calls are pinned: the provider is comctl32's, and the hits mockapp counts
for its root come from other clients answering the control's creation
events, at times of their own.

- Before the MSAA work package (2026-10-07), once the Windows Forms class
  was recognized at all: 28 MSAA calls and 14 window messages. The window
  messages are the item's position (mapping its child id to its item, then
  counting its siblings both ways, 4), its parent (3), and the parent's
  own position (5) and parent (2).
- With the check state read from each item's state image, as NVDA reads
  it: 28 MSAA calls and 16 window messages, one `TVM_GETITEMSTATE` for the
  item and one for its parent.
- Today: 35 MSAA calls and 12 window messages. The parent is read as an
  ancestor, which is not spoken in full, so its siblings are no longer
  counted (4 window messages fewer). The focused-state check comes first,
  as in NVDA, reading the states live: the item has the focused state only
  while the control has the keyboard focus, which the test does not give
  it, so the check reads the tree view's, its window's, and mockapp's root
  state, finding it on the root (7 MSAA calls more). A real tree view's
  focused item has the state itself, and the check then costs nothing.

### A focus on a report view item, MSAA

A focus on an item of a real list view in the report view (mockapp's
`tests/fixtures/list_view.json`, the `msaa_list_view_costs_exactly`
ratchet), after a focus on another item, named by its four columns as NVDA
names it.

- Before the MSAA work package (2026-10-07) the item kept MSAA's name, and
  no column was read.
- Today: 29 MSAA calls and 14 window messages: the view, the header and
  its item count, the column order, each column's rectangle (4), the text
  of each column shown (3), the header of each but the first (2), and the
  item count for the position. The structures the messages take are
  written into and read from the list view's process with the kernel's
  memory calls, which are not counted.

### A tree view item expanded or collapsed, MSAA

The focused tree view item's own state change, raised by the control.

- Before the MSAA work package (2026-10-07): every state change that left
  any tree item expanded, the focus or not, counted the item's children.
- Before reading only the property that changed (2026-10-10): 13 MSAA
  calls for either change, the item read in full.
- Today: 2 MSAA calls, the acquisition and the item's state, and 7
  window messages for the change that expands the focus: its child count
  (mapping its child id, then its first child and each next sibling, 5)
  and its check state and item (2); 2 window messages for a collapse,
  which counts nothing. A change on an object that is not the focus is
  not read at all (below).

### An event on an object that is not the focus, MSAA

A value change on an object that is not the focus, followed by the
focus's own: NVDA speaks a name, value, description, or state change only
for the focus, so the outpost tells the object from the focus by its
identity, the address it was reported at or the same COM object, before
reading any of its properties.

- Before the MSAA work package (2026-10-07): every such event read the
  object in full, 7 MSAA calls and its acquisition.
- Before reading only the property that changed (2026-10-10): the
  focus's change was read in full, 11 MSAA calls, and a progress bar off
  the focus too.
- Today: its acquisition and its role only, the role read to know
  whether it is a progress bar, whose value changes are reported off the
  focus too (mockapp answers `get_accChild`, `accParent`, and
  `get_accRole` once each for it), and the focus's change 2 MSAA calls,
  its acquisition and its value. A progress bar off the focus then has
  its states, value, and location read, what NVDA's progress bar behavior
  reads, and nothing else. mockapp answers a `WM_GETOBJECT` for each
  event's acquisition, 2.

### A name, description, or state change on the focus, MSAA

NVDA reads only the property its event names, of the focus object it
already holds. So does the outpost, once the object is known to be the
focus or one of its ancestors, with the rules of a full read that depend
on the object's role, taken from the role it was last read with: a list
view item named by its columns, an edit field in a labelled combo box
with no name of its own, a tree view item's check state, an edit control
that edits more than one line (`msaa_state_change_costs_exactly`).

- Before reading only the property that changed (2026-10-10): the object
  read in full, 11 MSAA calls with its acquisition.
- Today: 2 MSAA calls each, the acquisition and the one property;
  mockapp answers its `WM_GETOBJECT`, `accParent` and `get_accChild` for
  the acquisition and the one property's method, nothing else.

The window's verdict of no UIA provider is renewed by time, every half
second, as NVDA renews it: nothing tells a client that a window has begun
to answer the probe, so no event can drive it. The ratchet's outpost reads
that time from the test, which holds it still and moves it on only where
it pins a renewal, so no count depends on how long the test took. A
steady-state focus whose window's verdict has run out costs the probe
again, 1 window message and 1 `WM_GETOBJECT` more than the next focus,
which finds the verdict renewed. (Both are arrows between list items whose
previous focus was itself reached by an arrow, 33 MSAA calls each, the
`accFocus` probe included: one `accRole` more than the arrow above, whose previous focus was reached from
the list. Why is not yet known.)

### A selection, MSAA

A list item selected while the focus is a button beside the list, and an
item selected while the focus is the list
(`a_selection_is_read_in_full_only_inside_a_focused_list`), and the focus
itself selected (`msaa_state_change_costs_exactly`). Core speaks a
selection only inside a focused list or tab control, and a selection of
the focus or an ancestor as a change of its state.

- Before reading a selection only as far as it is spoken (2026-10-10):
  every selection was read in full for speaking, its position counted,
  whatever the focus.
- Today: outside a focused container, the acquisition alone, and mockapp
  answers a `WM_GETOBJECT` and a `get_accChild`; inside one, a full read,
  as before; the focus selected, 2 MSAA calls, the acquisition and its
  states.

### A selection inside a list the focus controls, UIA

A search box whose ControllerFor relation names a list, and an item of
the list selected (`crates/mockapp/tests/controller_for.rs`).

- Before the controlled selection's remote operation (2026-10-10): the
  relation read (1 UIA call), the controlled element's runtime id read
  (a call the ledger does not count), and a `FindFirstBuildCache` under
  it (1 UIA call): three round trips for one controlled list, more for
  each further controlled element.
- Today: 1 UIA call, one remote operation, whatever the number of
  controlled elements, and the same answer as the classic calls.

### An event from a window not related to the foreground window, MSAA

A state change on the focus while another application's window is in
front, and the same change with the focus's window in front
(`msaa_state_change_costs_exactly`). The outpost tests each event by
NVDA's acceptance test against the foreground window before anything else
(`docs/parity.md`, "Event acceptance in the outpost").

- Before the event filtering work (2026-10-10): the change was acquired,
  its window probed when its verdict had run out, and read in full, then
  dropped by Core.
- Today: dropped before any call; mockapp answers nothing, not even a
  `WM_GETOBJECT` for a probe of its window, whose verdict has run out. With
  the focus's window in front, the change costs the probe, the
  acquisition, and the focus's states, 2 MSAA calls and 1 window message,
  and mockapp answers 2 `WM_GETOBJECT` and 3 provider calls.

### A repeated or unfocused focus event, MSAA

- A focus event naming the address of the focus the outpost last
  reported, a whole object rather than a child by id, with no foreground
  change since: NVDA drops it as a duplicate. Before the MSAA work
  package (2026-10-07) it was read in full with its ancestors and the
  reducer found it the focus already, 29 MSAA calls; today only its role
  is read, for NVDA's list redirect, and its `accFocus` asked, for a
  focused child to report instead, and mockapp answers 4 provider calls
  and the acquisition's `WM_GETOBJECT`.
- A focus event on an object that neither has the focused state nor is
  inside one that has: NVDA reads the states first. Before, it was read
  in full with its ancestors, then dropped; today its role, its
  `accFocus`, and its state and each ancestor's state are read, and
  mockapp answers 12 provider calls and the acquisition's `WM_GETOBJECT`,
  for an object two levels below its root.

### Entering a dialog, MSAA

A focus moves into a message box, a dialog holding its question as static
text and two buttons, from outside it, so the dialog's own text is gathered
and reported as its description (`docs/nvda/object-model.md`, "A dialog's
own text"). mockapp's `tests/fixtures/dialog.json` is that message box.

- Minimum: a cold focus's 30 MSAA calls and 1 window message, and 14 for
  the text: the dialog's child count and children (2), each of the three
  children's `IAccessible` and role (6) and states (3), and the question's
  name, value, and description (3). The buttons give no text, so nothing
  else of them is read, and the question's neighbor is a button, which is
  never labelled, so its name is not read either.
- Today: 44 MSAA calls and 1 window message, the cold focus's 30 and the
  text's 14. mockapp answered 56 provider calls. A focus moving within the
  dialog does not read it again, since the dialog is then in the previous
  focus's chain when the dialog has its own window; mockapp's dialog has
  none, and oleacc hands out a new wrapper for every `accParent`, so the
  dialog it reaches from the next button is a new node to the outpost.
- Target: the minimum, met for the text.

### A dialog's text, UIA

The same message box through UIA, gathered from the dialog's element the
outpost holds.

- Minimum: 1 UIA call per container read: the dialog's children, with
  every property the rules look at cached, in one `BuildUpdatedCache` whose
  scope takes in the children.
- Today: 1 UIA call. mockapp answered 208 provider calls.
- Target: 1, met.

### An object navigation step, MSAA

- Minimum: 10 MSAA calls for a next sibling (`accNavigate`, the neighbor's
  `IAccessible`, its window, and its seven properties) and 10 for a parent
  (`accParent` in place of `accNavigate`).
- Today: 10 each. mockapp answered 9 provider calls for the next sibling
  and 15 for the parent, 8 of them `accParent` from
  `WindowFromAccessibleObject`'s walk. With a theme reporting descriptions
  and shortcuts as off, a next sibling is 8 calls: neither property is
  read (milestone M4, themes).
- Target: 10 each.

### An interrupt

Stopping speech when a key is pressed.

- Minimum: none: the keyboard hook and the speech pipeline are Verbatim's
  own, and nothing about the application is read.
- Today: none. The ratchet does not measure it, since it makes no call to
  count.
- Target: none.

### A caret move, UIA

A caret key the application has already handled (Right Arrow, the caret
one character on), answered with the caret's line, its offset in it, and
the character there with its formatting, from the text pattern the
outpost keeps for the node, under the default theme, which reports
spelling and grammar errors. Measured against mockapp's text provider
(`tests/fixtures/text.json`), whose "beta" is a spelling error, by
`crates/mockapp/tests/call_counts.rs`, both ways.

- Minimum: 1 UIA call, one remote operations program
  (`verbatim_uia_rops::caret_read`). Classically, 12 calls one at a time:
  the selection (`GetSelection`), whether it is empty (`CompareEndpoints`
  of its two ends), the evidence (`CompareEndpoints` with the caret Core
  knew), the line (a copy of the caret, `ExpandToEnclosingUnit`,
  `GetText`), the caret's offset in it (a copy of the line,
  `MoveEndpointByRange` to the caret, `GetText`), and the character's
  annotation types and link (a copy of the caret, `ExpandToEnclosingUnit`,
  and one `GetAttributeValues` call, which the provider answers with two
  `GetAttributeValue` reads since the default theme reports links, one
  before). The character is cut from the line's text, at no cost.
- Today: 1 remotely, 9 calls before remote operations (2026-10-06, with no
  formatting). The program makes inside the provider the classic reads'
  provider calls and one copy more (a `Clone` and a `MoveEndpointByRange`
  collapsing the caret), plus the import of the element and its text
  pattern (`GetPatternProvider` and four more provider calls UIA makes).
  Classically, with remote operations off or a provider that cannot run
  programs: 12, the 9 of before and 3 for the character's spelling error.
  A theme with the font, its attributes, or the color on adds one
  attribute read each classically, and nothing remotely.
- Target: 1.

### A caret key's watch, UIA

A caret key that could have moved the caret but has not yet (Right Arrow
one character into the text, which the application has not handled when
the request arrives): the watch's first check reads the caret once,
finds no evidence, and leaves the watch open, and the worker goes on with
its queue (`docs/crates/verbatim-outpost.md`, the caret key's watch).
When the application then moves the caret and reports it, the check that
caret event prompts reads the caret once more and answers the key. A key
that moves nothing makes the first check alone, and is silent. Measured
by `crates/mockapp/tests/call_counts.rs` in mockapp's text, both ways,
and in its edit control.

- Minimum: one round trip for each check; nothing for waiting.
- Today: 1 remotely for each check, the whole caret read above, so the
  check that finds the evidence is the answer; classically 12 for each,
  the caret move's reads; in the edit control 5 window messages for a
  check that finds nothing and 6 for one that answers (the selection read
  again, as for a caret move).
  Before 2026-10-08 the outpost waited for the evidence on its worker,
  reading the caret every 10 milliseconds until its 100 ran out (300 in a
  terminal): a key that moved nothing cost 11 round trips remotely, 134
  calls classically (12 for each of eleven reads and 2 comparing the
  caret with an end of the document, which a wait's first read made for
  the keys whose destination it could tell), and 55 window messages in
  the edit control; and Control+Home at the start of the text, the one
  such key that read the line's formatting, 396 calls classically, before
  that comparison let it answer at once.
- Target: one round trip for each check.
- Wall clock: what matters is what waits behind the key. Before, every
  other entry of the application's outpost waited out the key's wait: in
  mockapp, a focus change delivered just after a Right Arrow the
  application ignored was handled 100.7 milliseconds later, once the
  wait's 100 had run out (`crates/mockapp/tests/caret_watch.rs`,
  `a_focus_change_during_a_caret_watch_is_handled_at_once`, failing on
  the code before the change). Archived end-to-end runs showed about one
  caret key in ten taking about 110 milliseconds, every one a key that
  could not move the caret. Now the focus is taken from the queue as soon
  as the key's one check is done: 0.74 to 0.85 milliseconds after the
  key in five runs of the same test, and the key's watch ends with it,
  saying nothing.

### A caret report, UIA

The caret reported without a key: a caret event's report (`CaretMoved`),
which follows every typed character, and which nothing speaks.

- Minimum: 1 UIA call remotely; classically 8, a caret move's without the
  comparison or the formatting.
- Today: 1 remotely, 8 before remote operations and classically.
- Target: 1.

### The caret report after a focus, UIA

The report after a text focus, whose line Core speaks, with the line's
formatting: mockapp's first line has four stretches (bold "alpha", a
space, the misspelt "beta", the line feed).

- Minimum: 1 UIA call remotely. Classically, a caret report's 8 and the
  walk by UIA's format unit: a collapsed copy of the line (2), and for each
  stretch a copy, `MoveEndpointByUnit`, a comparison with the line's end,
  its text, and its attributes (one call for all of them,
  `GetAttributeValues`), 5 each, moving the walk on after each but the
  last (3), and a `MoveEndpointByRange` to cut a stretch that runs past
  the line's end, here the last: 34 for four, however many attributes the
  theme reads; and one call asking the line for its annotation types
  and the attributes whose support is not yet known ("Generic text
  attributes" below): 35.
- Today: 1 remotely, 35 classically (8 before formatting was read; 34
  before the line was asked first), with the default theme and with every
  formatting indication on (63 before the attributes were read in one
  call, "Several text attributes in one call" below; 39 before the text
  range audit of 2026-10-07, which dropped a second comparison per
  stretch, of where the next one starts with the line's end: the first
  comparison already says whether the stretch reached the end, and the
  walk is no longer moved on after the last stretch). The remote
  program's provider calls fell the same way, from 9 comparisons to 5
  and from 8 endpoint moves to 7. Against mockapp, debug build, 200
  reports, three runs: classically 3.67 to 3.84 ms at the median before
  and 3.16 to 3.46 after, with the default theme or every indication on;
  remotely 0.40 ms (0.50 with every indication) either way. The provider
  reads, pinned, with the default theme, which reads the annotation
  types and links: the line's annotation types and its link, then two
  per stretch, 10, the first time; once mockapp's text is known not to
  support links, the line's annotation types and one per stretch, 5. With
  every indication on, eleven attributes: 11 asked of the line and 44 of
  the stretches the first time, then 1 and 28 once the four mockapp does
  not support (strikethrough, background color, bullet style, link) are
  known.
- Target: 1.

### A caret key that selects, UIA

Shift with Right Arrow, answered with the caret's line and character and
the text newly selected: mockapp's caret goes from collapsed at the start
to a selection of "alpha".

- Minimum: 1 UIA call: the caret read's program also compares the
  selection's ends with the old ones and, when they moved, reads the text
  of each change.
- Today: 1 remotely (8 before: the caret read and then the selection's
  changes worked out call by call, two comparisons for whether the two
  selections are apart, one for each side, and three for the text of the
  change). Classically 23 (30 before): the caret read's calls, and the
  same comparisons and text read, now only when the selection moved.
- Target: 1, met.

### A review cursor line, UIA

The review cursor's next line (Verbatim's numpad 9), from a position the
outpost reported: a `Read` moving by a line from the start of the line
holding the position, then reading that line and its language.

- Minimum: 1 UIA call, one program (`verbatim_uia_rops::text_units`).
- Today: 1 remotely; classically 8 (a copy of the position, expanded to
  its line, collapsed, moved; a copy expanded to the line reached and its
  text; its `Culture` attribute). 9 before the text range audit of
  2026-10-07, which dropped a second collapse after the move: UIA keeps a
  collapsed range collapsed when it moves. The first review command and report current object on an
  edit field read the line at the caret the same way, in one program
  (classically 9: the caret's two reads, the line's three, the caret's
  offset in it three, and its language).
- Target: 1, met.

### A review cursor word inside a line, UIA

The word at a position Core found inside a line it holds (the review
cursor moved by characters within the line, then current word): the
position is the line's start and the text before it, found by moving a
copy by that many characters and comparing the text passed.

- Minimum: 1 UIA call: the program moves and compares inside the
  provider, trying each character count a provider may use.
- Today: 1 remotely; classically 12, as before (five to find the
  position, three for the word, three for the position's offset in it,
  and the language).
- Target: 1, met.

### Say-all, UIA

Reading mockapp's three lines (two lines and the empty one after the last
line break) from the caret, and moving the caret as each is reached.

- Minimum: 1 UIA call for every batch of lines read ahead
  (`TextOp::ReadAhead`, twenty units), and 1 for each caret move.
- Today: 1 UIA call for all three lines remotely, its end found in the
  same program; 24 classically (29 before the text range audit, below).
  Before, say-all asked for one line per
  request, each a round trip remotely or not: 9 calls for the first line,
  10 for each later one, and 10 more for a last request that found the
  end, 39 calls in four requests. Each caret move is 1 call remotely and 4
  classically (4 both ways before).
- Target: 1 per batch and 1 per caret move, met.

When the next batch is read (decided with Dickson on 2026-10-07): say-all
reads 20 lines or sentences at a time, keeps their pieces in a buffer,
hands them to speech two ahead of playback, and reads the next 20 once
fewer than 10 pieces are left to speak, handed out and buffered together
(`docs/crates/verbatim-core.md`, "Say-all"). It replaced a low-water mark
in time (three seconds at a pace of speech measured from the marks): a
count needs no estimate, and ten pieces, even of a few words each, last
far longer than a batch's read, as measured against Windows 11 Notepad:

Measured on 2026-10-07 on this machine (x64), debug build, otherwise
quiet, reading a 300-line document of prose (lines of 40 to 80 characters
taken from `docs/architecture.md`) from its top to its end, with eSpeak NG
at the end-to-end rate (376 words a minute) in Verbatim and in NVDA. Each
batch's read time and calls are the outpost read stage of Verbatim's
latency log; the gap between two pieces is the silence in the system's
loopback audio where the second piece starts, found from each piece's
reported start (Verbatim's `SpeechStarted` frames, NVDA's index marks),
and zero where the pieces ran together. Fifteen batches of 20 lines each;
the read that then finds the end is left out.

- Remote operations on: a batch read in 3.3 ms at the median, 9.3 ms at
  the 95th percentile and at worst, in 1 call each; the gaps between
  pieces 1.5 ms at the median, 14.5 ms at the 95th percentile, and 23.3
  ms at worst, over 297 boundaries.
- Remote operations off: a batch read in 21.1 ms at the median, 30.8 ms
  at the 95th percentile and at worst, in 166 calls each (165 for the
  first); the gaps 1.5, 13.9, and 25.3 ms.
- NVDA, which reads a piece at a time, a few lines per call to the
  synthesizer: the gaps 1.2, 30.2, and 62.5 ms, over 299 boundaries.

Each batch's request also waited 5 ms at the median (10.8 at worst)
behind the application's other events in the outpost's queue. With either
path, a batch arrives with ten pieces, several seconds of speech, still
to speak, and the gaps are the same with remote operations on and off:
the pieces already with speech hide the read entirely.

Since 2026-10-07 say-all speaks without pauses, as NVDA does
(`docs/nvda/speech.md`, "Say-all speaks without pauses"): a piece is an
utterance ending at a sentence end, which may hold the rest of one line
and the start of the next, and the counts above (20 units a read, the next
read below 10 pieces) count units read and utterances left respectively.
The measurements above were taken before that change, with a piece a
line.

A batch of twenty lines, classically, after the text range audit of
2026-10-07 (`phase6-design.md`, item 14 of the work scheduled that day),
pinned against mockapp's text replaced by forty-five short lines
(`uia_say_all_batches_cost_exactly`):

- Before: 165 calls for the first batch and 166 for each later one, the
  same counts as against Windows 11 Notepad above. Each line after the
  first cost 8: a copy of the line before, collapsed, moved by a line,
  collapsed again, another copy, expanded, its text, and its `Culture`
  attribute; and the move that finds whether a line follows the twentieth
  cost 4.
- After: 109 for the first batch and 108 for each later one. Each line
  after the first costs 5: a copy of the line before, collapsed, moved,
  expanded in place, and its text, since UIA keeps a collapsed range
  collapsed when it moves (`ITextRangeProvider::Move`) and expanding
  normalizes a range from its start alone (`ExpandToEnclosingUnit`), so
  neither the second collapse nor the second copy did anything. The
  language is read once for the batch, over a range from its first line's
  start to its last line's end (3 calls), and line by line only when that
  one read answers UIA's "mixed": 33 calls classically for mockapp's
  `languages.json`, four lines in English, French, and German, where the
  three-line batch above costs 24.
- Remotely still 1 call each; the program makes the same reads inside the
  provider, so its provider calls fell from 42 copies, 42 or 43 endpoint
  moves, and 20 `Culture` reads per batch to 24, 23, and 1.
- Wall clock, debug build against mockapp in its own process, 200 runs of
  each batch through the outpost's text module, three runs: classically
  21.6 to 24.0 ms at the median (29 to 42 at the 95th percentile) before
  and 10.0 to 10.6 (11.7 to 13.0) after; remotely 1.7 to 2.1 ms before and
  1.0 to 1.6 after.

### The caret's location, UIA

Report caret location: the screen position of the character at the
caret.

- Minimum: 1 UIA call.
- Today: 1 remotely; classically 5, as before (the caret's two reads, a
  copy expanded to the character, and its bounding rectangles).
- Target: 1, met.

### The selected text, UIA

The selected text read for a copy, or for the selection a focus or the
navigator object is announced with (`ReadRange` from the selection's
start to its end).

- Minimum: 1 UIA call.
- Today: 1 remotely; classically 7 (10 before: the caret was read once for
  each end).
- Target: 1, met.

### Wall-clock gain

Measured on 2026-10-07 on this machine (x64), release build, against
mockapp in its own process, 200 runs of each operation through the
outpost's text module or the navigation step as the worker makes them,
with the machine loaded by two other engineers' builds. The medians held
within about 0.2 ms across three runs; the 95th percentiles and the worst
runs vary with that load, and the worst remote navigation step is the
first program a process runs, which pays for creating the remote
operations machinery. mockapp's providers run on one window thread, so
each provider call inside a program is itself marshaled there; a real
application's provider answers inside the program without that hop, so
the gain is larger there. Median, 95th percentile, and worst, in
milliseconds, remote against classic:

- Navigation, next sibling: 0.93, 8.8, and 109 against 1.62, 7.7, and 19.6.
- Review cursor's next line: 0.40, 0.49, and 8.4 against 0.77, 4.7, and
  7.3.
- Review cursor's word inside a line: 0.38, 0.43, and 4.9 against 0.60,
  4.6, and 5.2.
- Line at the caret (first review command, report current object): 0.43,
  4.0, and 10.1 against 0.76, 5.4, and 7.5.
- Say-all's read ahead of sixteen lines: 0.90, 1.08, and 6.4 against
  22.3, 83.3, and 149. The same sixteen lines read a request each, as
  say-all read before: 6.5, 12.7, and 37.5 remotely and 29.8, 112, and 190
  classically, before counting Core's round trip per request.
- Say-all's caret move: 0.29, 0.35, and 4.4 against 0.40, 4.9, and 9.2.
- The caret's location: 0.36, 0.40, and 0.44 against 0.54, 8.8, and 24.4.
- The selected text: 0.39, 0.46, and 4.3 against 0.68, 6.2, and 13.9.
- A caret key that selects: 0.45, 0.55, and 1.1 against 2.5, 21.5, and
  56.8.
- A caret move (for comparison, converted earlier): 0.41, 0.45, and 0.96
  against 1.34, 19.4, and 34.8.

### A caret move, Win32 edit control

The same caret key in a standard edit control, read through its messages.

- Minimum: 5 window messages: the selection (`EM_GETSEL`), the caret's
  line (`EM_LINEFROMCHAR`), where it and the next line start (two
  `EM_LINEINDEX`), and its text (`EM_GETLINE`). A rich edit control from
  version 2.0 takes the same number, with `EM_EXGETSEL`,
  `EM_EXLINEFROMCHAR`, and `EM_GETTEXTRANGE`.
- Today: 6: the selection is read again before the key is answered
  (since 2026-10-09). Read by parts, the caret can be read before the
  control handles the key and its line after (a Backspace's character
  message comes after the key's own), which paired a caret from before a
  deletion with the text after it, and the Backspace said nothing; when
  the second read differs, the control's caret event answers instead.
- Target: 5, with the caret and its line read in one message, which no
  edit control message offers.

### A caret report, Win32 edit control

- Minimum: 5 window messages, as for a caret move, whose comparison is
  local for offsets.
- Today: 5.
- Target: 5.

### Typed-character echo

- Minimum: none for the echo itself: the keyboard hook translates the key
  locally (`ToUnicodeEx`, `docs/crates/verbatim-input-windows.md`), and
  nothing about the application is read. The caret event the typing causes
  is reported like any other, a caret report above.
- Today: none, and one caret report for the caret event.
- Target: none.

### A terminal output line

A focused terminal's text changed, and the outpost reads its screen and
diffs it with the screen as last read (`docs/crates/verbatim-outpost.md`,
"Terminals"): the screen's text in one call, its top two rows and the
text's first row, the anchor found at its range or by its text, the rows
from it to the screen counted by moving, and the first unread rows when
any went by. Measured against mockapp's text provider
(`tests/fixtures/terminal.json` with a four-row screen,
`tests/terminal.rs`), whose `set-text` command rewrites the text as a
terminal's buffer changes and whose `screen` command puts only its last
lines on screen.

- Minimum: 1 UIA call, one remote operations program
  (`verbatim_uia_rops::terminal_screen`), whatever the size of the
  history and however many lines changed.
- Today: 1 UIA call for every read, the baseline when the terminal gains
  the focus included. Classically, with `uia.remote_operations` off or a
  provider that cannot run programs, one call per provider method: 25 for
  the baseline, 43 for a read that finds the anchor at its range (a grown
  prompt, an output line, a redraw), 50 for one that also reads the first
  rows that went by unread, 43 for one whose anchor has left the history
  (searched by its text, then the text's rows counted), and 38 for a
  cleared screen. Every one of these is pinned, both ways, and so is the
  provider's own work, which is the same either way but for the remote
  program's import of the element and its text pattern. The screen is
  read in one call however many lines it holds, so the cost does not grow
  with them. Of these calls, the top row read again at the end, the
  walk from the screen's top to the text's end made twice, and the visible
  range read again at the end tell whether the terminal wrote to the
  screen, or moved its view, while it was read, when the read is not
  trusted (the visible range since 2026-10-09: the console host moves its
  view down a row for each line written into a scroll region above a
  footer, and a read between the two had the rows above the view).
- Target: 1.

The read it replaced (an anchor at the last line read, its two-line
fingerprint, and a read afresh when it did not match) cost the same one
call remotely and 27 to 61 classically.

### A terminal's caret

A terminal raises no caret event for every character typed: the console
host raises its caret events on a schedule of its own, so a character
echoed between two of them leaves Core's copy of the caret behind, and a
Backspace then says the character the caret was last heard after. So each
read of a focused terminal's changed text reads the caret and its line
too (`verbatim_uia_rops::ScreenQuery::caret`), and the outpost reports
it with the output (`docs/crates/verbatim-outpost.md`, "Terminals"),
stamped as read when the read began.

- Minimum: no call more, since it goes in the program the text's read
  already runs.
- Today: no call more remotely; from mockapp, the text pattern got from
  the element again, the selection, and the caret's line read.
  Classically, 8 calls more: 51 for a grown prompt where the read without
  the caret is 43. Both are pinned (`tests/terminal.rs`,
  `a_read_with_the_caret_costs_exactly`).
- Target: the minimum, met.

### A terminal flood

What Verbatim costs a terminal while ten thousand lines are written as
fast as PowerShell can write them (the flood scenarios' script) into a full
scrollback of 9,001 lines, measured on 2026-10-07 on a 12-thread x64
desktop with a debug build. Each time is one flood's own stopwatch; the
probes that split the cost apart registered and read exactly as the
outpost does, without the rest of Verbatim.

The console host:

- With no screen reader, a flood takes 1.6 to 1.9 seconds (1.7 typical).
- Subscribed to the text change and caret events alone, reading nothing,
  2.2 to 2.6 seconds. The console host raises about 13,600 of these events
  per flood, 1.4 for each line, and raising each one to a listening client
  costs it about 40 microseconds. This is the largest part of the cost,
  and it is the console host's own: every client that subscribes pays it.
  About 0.15 seconds of it is the base cache request the subscription
  attaches, whose 34 properties the provider computes for every event;
  a one-property cache request saves that in isolation, but in the full
  outpost it made the console host stall for three to four minutes in
  each of five runs, with a thread of UIA's own in the outpost spinning,
  so it was not adopted.
- Reading the tail back to back without events, 1.8 to 2.0 seconds: each
  read holds the provider for 0.4 to 0.9 milliseconds, and the provider
  answers a read at a time.
- Verbatim, which does both, 2.9 to 3.7 seconds: about twice the time
  with no screen reader. The outpost reads continuously, since every text
  change that arrives during a read causes one more (`phase6-design.md`:
  coalesced, with no fixed delay): about 2,800 tail reads per flood,
  median 0.6 milliseconds, busy for 40 to 60 percent of the flood. In the
  first flood, which fills an empty scrollback, the caret also moves on
  every line, and about 2,000 caret reads (0.5 milliseconds each) are
  added; once the scrollback is full the caret stays on the last row and
  hardly any are made.
- Before the fix of 2026-10-07, a read the text scrolled under that found
  no lines took a fingerprint of two empty lines, and from then until the
  flood ended every read searched all 256 lines above the anchor without
  finding it: about 760 reads per flood at 3 milliseconds median, 2.5
  seconds of the 3.7-second flood, each saying only "skipped lines". After
  it, no read of the same floods fails to find its fingerprint, and the
  output is counted. The wall time did not change: cheaper reads are more
  numerous, since the outpost reads whenever the text changed.

Windows Terminal:

- With no screen reader, 1.4 to 1.6 seconds; with Verbatim, 1.5 to 1.7.
- It raises about 60 text events per flood (it batches its output by
  frame), so the outpost reads about 50 times per flood, at 1 to 5
  milliseconds each (the fingerprint is found about 200 lines up, searched
  a line at a time), busy for 6 to 10 percent of the flood.
- Read back to back with no events, it would take 1.6 to 1.8 seconds.

The wall-time ratio the flood scenarios record compares a flood with output
reported against one with it turned off, and the outpost reads the
terminal either way (turning reporting off is Core's), so the ratio is
about 1 by construction and does not measure the reads' cost; the
comparison with no screen reader above does. The one ratio of 5.68 on
record (2026-10-06, Windows Terminal) came from a run during which
another engineer's tests started seven mockapp processes and Verbatim's
audio ran dry thirteen times in the fourth flood alone: the machine was
starved. Twelve busy threads at normal priority make the same flood take
75 to 99 seconds in Windows Terminal with no screen reader at all.

With the screen read and on-demand reading (2026-10-08, the flood
scenarios' 2,000 lines, debug build, one run each after ten in a row
passed in Windows Terminal):

- The console host: 1,054 reads of the screen, each a whole remote
  program, 0.7 to 4.5 milliseconds (median 1.8, 90th percentile 2.6); the
  flood took 1.87 seconds with output reported and 1.97 with it off, a
  ratio of 0.95.
- Windows Terminal: 84 reads, 0.8 to 2.3 milliseconds (median 1.3); 0.32
  seconds reported against 0.38 off.
- The gap before "skipped 1941 lines": Core asks for new output when it
  hands a group's last line to speech, so the answer is there before that
  line ends. In both terminals the skipped-lines announcement started
  within 1 millisecond of the 30th line finishing (the speech request
  came 0 to 1 milliseconds before).

### A terminal's upward search

A read finds the screen's top row as last read without any search while
the terminal has discarded nothing since (its text's first row reads as
it did, or the old screen had no history above it): a range kept at that
row stays on it, and the rows from it to the screen are counted by
moving, so the read costs the same wherever the row now is. Only once a
full history discards its oldest rows does a range keep its row while the
text moves beneath it, and the row is then sought by its text
(`FindText`), at most 20 matches, nearest the screen first (Dickson,
2026-10-07), with the row padded in the console host, whose `FindText`
matches a row's padding, so a longer row starting with the same text is
not a match (a flood of numbered lines made "flood line 14" match "flood
line 1499" down to "flood line 140" before reaching itself, exhausting
the 20 matches, on 2026-10-08). Windows Terminal's `FindText` throws on
padding, so there the row is sought without it. Neither terminal can
search across a line break: the console host's `FindText` returns a range
for such a needle whose text then cannot be read, and Windows Terminal's
matches none (measured 2026-10-08).

Measured on 2026-10-07 for the tail read this replaced, against Windows
Terminal and the console host each with a full scrollback of 9,001 lines:
one `FindText` costs 0.1 to 0.2 milliseconds classically wherever the
text is, and about 3 inside a program on a range the program made, so a
search costs about 3 milliseconds a match remotely; a walk up line by
line, the earlier search, cost 0.01 milliseconds a line remotely and
could not look further than its bound. In a console host whose
scrollback is not yet full, lines are slow to walk one at a time (about
1.6 milliseconds a line in a 9,001-line buffer holding 600 lines), which
the screen read never does: it moves by many rows in one call.

## Newer UIA features

What adopting the UIA features newer than Verbatim's first UIA code
changed (`phase6-design.md`, item 13 of the work scheduled on
2026-10-07). Each was measured on 2026-10-07 on this machine (x64),
release build, against mockapp in its own process, before and after the
change, with another engineer's build running; the counts are pinned by
`crates/mockapp/tests/call_counts.rs` as the ledger above is.

### Event coalescing and connection recovery

Every UIA client Verbatim creates turns on `IUIAutomation6`'s
`CoalesceEvents` and `ConnectionRecoveryBehavior`, as NVDA does on its
one client (`docs/crates/verbatim-uia.md`). Both are local settings of
the client, so no operation's call count changed.

- A burst of 100 name changes from one mockapp element, through a
  property subscription: all 100 delivered before and after, the last
  arriving 91 ms after the burst began at the median before and 88 ms
  after; with a handler that takes 5 ms per event, still 100. mockapp's
  provider raises each change at once, and UIA delivered every one of
  them either way. The same burst from mockapp's MSAA backend, through
  UIA's MSAA proxy, arrived as one event both before and after: the
  system merges those `WinEvent`s before UIA sees them.
- Reads while mockapp's window thread is stalled: a fetch on a new
  connection waited out a 3 second stall and answered, and failed with
  UIA's timeout after 10.0 seconds of a 12 second stall, before and
  after; a read of an element already fetched waited out the stall
  either way. Connection recovery therefore leaves Verbatim's long wait
  for a starting application's answer as it was.
- Creating a client: 0.038 ms at the median before, 0.036 after.

### Event handler groups

Every `verbatim_uia::Registration` now registers its subscriptions as one
`IUIAutomationEventHandlerGroup`, with one `AddEventHandlerGroup` call per
element of its scope, as NVDA registers its handlers. The focus listener's
three desktop-wide subscriptions (an element selected, a menu opened, and
notifications) became one registration with one thread and one client,
where they were three of each. A registration runs on its own thread, so
it makes no counted call on the worker's; what it costs the application
is the provider calls UIA makes while registering, now pinned by the
ratchet (`uia_event_registrations_cost_exactly`), the same with remote
operations on or off.

- The listener's registration: none of mockapp's provider calls, before
  and after. Registering it took 18.5 ms at the median (24.9 at the 95th
  percentile) as three registrations and 9.9 ms (11.9) as one group, 30
  runs each.
- An outpost's focus-following property subscription moved to a focus
  inside a group (the focus, the group, and the window): 2
  `HostRawElementProvider` and 5 `FragmentRoot` provider calls, and 1.19
  ms at the median before, 1.15 after, as one group per element.

### Selective registration

The outpost's focus-following property subscription is registered on the
focus alone, where it was registered on the focus and on each ancestor
the focus reported (`docs/parity.md`, "UIA event registration"). The
reducer acted only on the focus's own changes, so the ancestors' events
were read and sent for nothing. NVDA's scope, the focus with its
ancestors, was measured too: UIA delivers no ancestor's event to it,
against mockapp and against Windows 11's taskbar, and it cost mockapp
the same provider calls as the focus alone.

- Moving it to a focus inside a group: 2 `HostRawElementProvider` and 5
  `FragmentRoot` provider calls before, on the focus, the group, and the
  window; 1 and 2 after. 1.15 ms at the median before (1.33 at the 95th
  percentile) and 0.75 ms after (1.14), 30 runs each.
- A change on an ancestor (a window's title, a group's name) no longer
  reaches the outpost, which read it, mapped it, and sent it to Core,
  which dropped it.

### Several text attributes in one call

The classic reads of a stretch's formatting fetch all its attributes in
one `IUIAutomationTextRange3::GetAttributeValues` call, where each was a
call (`docs/crates/verbatim-uia-rops.md`). The remote program is
unchanged: it reads the attributes inside the provider already. The
provider's own work is the same either way, one `GetAttributeValue` per
attribute.

- The caret report after a focus, mockapp's first line (four stretches),
  with every formatting indication on (seven attributes per stretch):
  classically 63 UIA calls before and 39 after (34 since the text range
  audit, "The caret report after a focus, UIA" above), 5.26 ms at the median
  before (6.11 at the 95th percentile) and 3.69 ms after (4.29), 200
  runs each; remotely 1 call, 0.36 ms before and 0.39 after. With the
  default theme, which reads only the annotation types, one attribute per
  stretch, the count is the same as before, 39 classically.

### Generic text attributes

The caret's read reads the attributes the theme's indications ask for,
among those the control supports, learned from the control
(`docs/text-attributes.md`, "What Verbatim fetches now"): while an
attribute's support is not known, the line is asked for it once, in the
same program or classic call, and its "not supported" answer stops it
being asked again for that control. The line is also asked for its
annotation types first, and its stretches only when it has some; a line
with neither annotations nor anything else to read is not walked.
Before, every attribute an indication asked for was read for every
stretch of every read. The counts are pinned in
`crates/mockapp/tests/call_counts.rs` ("The caret report after a focus,
UIA" above, and the mixed stretch's) and the learning in
`crates/mockapp/tests/text.rs`.

- mockapp's `text.json`, whose line has four stretches and a spelling
  error, with the default theme: classically 34 calls before and 35
  after; the provider's attribute reads 4 before, 10 on the first read
  after (links being learned), and 5 on every later read.
- The same line with every indication on: 34 calls classically before,
  35 after; the provider's attribute reads 28 for seven attributes
  before, 55 for eleven on the first read after, and 29 once the four
  attributes mockapp does not support are known.
- A line of 14 stretches without spelling errors, as a terminal's line
  is, with the default theme: 93 calls classically before, 9 after (the
  caret's read, and one call asking the line for its annotation types,
  which it has none of; no walk). Remotely the program no longer walks
  the stretches either.

Wall-clock, measured on 2026-10-07 on this machine (x64), release build,
against mockapp, 200 caret reports after a focus each, after 20 to warm
up (so the learning is done), the build before and after run alternately
twice more after a first pair, with another engineer's builds loading
the machine; medians in milliseconds, before against after, and the
range across runs:

- `text.json`'s line, default theme: remotely 0.31 to 0.59 against 0.35
  to 0.55; classically 3.1 to 4.9 against 3.4 to 4.6. The same within
  the load's noise: one call more classically, and inside the provider
  one read of the line more against one per stretch fewer.
- The same, every indication on: remotely 0.41 to 0.67 against 0.43 to
  0.70; classically 3.3 to 4.2 against 3.4 to 4.0.
- The 14-stretch line without errors, default theme: remotely 0.37 to
  0.51 against 0.30 to 0.37; classically 8.7 to 11.5 against 0.82 to
  1.3, a tenth.
- The same, every indication on: remotely 0.55 to 0.65 against 0.47 to
  0.61; classically 8.7 to 9.5 against 9.2 to 10.6 (93 calls against
  94).

Against the survey's numbers (`docs/text-attributes.md`, "What reading
them costs"), with the default theme: in Windows Terminal and the console
host, which support neither annotations nor links, the caret's read now
costs about what it costs with no attributes (Windows Terminal 0.43
milliseconds remotely on a line of one stretch, against 0.54 to 0.71 with
one group read stretch by stretch; classically 9 calls in place of 17 to
23, and in place of about 88 for a line of 13 stretches, at 0.1 to 0.2
milliseconds a call in Windows Terminal and 0.04 to 0.1 in the console
host). In Notepad a line without spelling errors costs 9 calls
classically in place of 46 to 76 for 5 stretches, and a line with one
costs one read more than before. With every indication on, Windows
Terminal is no longer asked for FontSize, BulletStyle, or Link once
learned, each about 0.05 milliseconds per stretch inside a remote
operation, about 2 milliseconds for a line of 13 stretches.

### A container's selected item

A focused list's or tab control's selected item is read through
`SelectionPattern2`'s `FirstSelectedItem` where the provider has it, and
through the `Selection` pattern where it does not, classically and in the
focus's remote program (`docs/crates/verbatim-uia.md`). Pinned over
mockapp's `tests/fixtures/ancestry.json`, whose list has
`SelectionPattern2` and whose tab control does not
(`uia_selected_children_cost_exactly`).

- Classically, one `BuildUpdatedCache` on the container caches both its
  `FirstSelectedItem` property, ignoring its default, and its `Selection`
  pattern object, so a provider without `SelectionPattern2` costs no
  extra call.
- A list's selected item, classically: 2 UIA calls (that one and the
  item's cache) where it was 3 (the pattern, `GetCurrentSelection`, and
  the cache); 74 provider calls where it was 63; 0.41 ms at the median
  where it was 0.46 (0.48 and 0.55 at the 95th percentile), 200 runs
  each.
- A tab control's, classically: 3 calls, as before (that one, which
  finds `FirstSelectedItem` not supported, `GetCurrentSelection` on the
  cached pattern, and the item's cache); 0.54 ms at the median where it
  was 0.46, the cache request with a pattern costing the provider more
  than fetching the pattern alone. A first version asked for
  `FirstSelectedItem` live before fetching the pattern, 4 calls and 0.58
  ms.
- Inside the focus's remote program: 1 call either way, with the same
  provider calls but for the selection read itself (`FirstSelectedItem`
  in place of `GetSelection`, and for the tab control one more
  `GetPatternProvider`); 1.10 ms at the median before and after for the
  list, 1.07 for the tab control.

### The active text position

A text focus's caret and text changes and its active text position
changes are registered as one event handler group on the focus, and an
active text position change is kept as a position in the focus's text
without reading anything (`docs/crates/verbatim-outpost.md`). Pinned by
the ratchet (`uia_active_text_position_costs_exactly`), the same with
remote operations on or off.

- Registering the group on mockapp's text: 1 `HostRawElementProvider` and
  2 `FragmentRoot` provider calls, as for the caret and text changes
  alone; 0.89 ms at the median with the new handler and 0.72 without (30
  runs each).
- Handling a change: no call and no provider call, 0.002 ms at the
  median. A change reached a registration 0.13 ms at the median after
  mockapp was told to raise it (100 runs).

## The text range audit

What auditing Verbatim's text range code against Microsoft's guidance
changed (`phase6-design.md`, item 14 of the work scheduled on 2026-10-07):
"Using IUIAutomationTextRange", "Understanding Performance Issues When
Using the Text and TextRange Control Patterns", and the reference pages of
`ITextRangeProvider` and `IUIAutomationTextRange`. Each change is pinned by
`crates/mockapp/tests/call_counts.rs` on both paths; say-all's batches and
the caret report after a focus are given above, with the counts before.

### A stretch whose attribute is mixed

`GetAttributeValue` answers UIA's "mixed" sentinel when an attribute
varies across the range. Verbatim read it, on both paths, as no value: a
provider whose format unit does not end where italics change (Windows
Terminal's and the console host's, `docs/text-attributes.md`) had its
italics lost. A stretch with a mixed attribute is now read again by its
words, and a mixed word by its characters, as NVDA reads one, up to the
64 stretches a span may have. Ordinary text costs nothing more, since the
mixed test is made on values already read.

- mockapp's `italic.json`, "plain italic text" as one format stretch with
  "italic" in italics, every formatting indication on (the caret report
  after a focus): 82 calls classically, where a stretch read as one cost
  34 for mockapp's four-stretch line; 1 remotely, its program walking the
  twelve stretches (the line's, four words, seven characters) inside the
  provider with 84 attribute reads. Before, the line was read as one
  stretch whose italics were none.

### Expanding without collapsing first

`ExpandToEnclosingUnit` normalizes a range from its start alone: a range
that starts inside a unit becomes that unit, however far its end reaches.
A copy collapsed before it was expanded made one call more for nothing.
The terminal's reads of a line's text (`line_text`, both ways) now expand
a plain copy: a terminal read costs 3 to 6 calls fewer classically ("A
terminal output line" above) and as many endpoint moves fewer inside the
provider remotely. The text source's own reads of a unit, a move, and a
location from a held position (`UiaText`'s `unit_at`, `move_by`, and
`location`), and the classic location read, do the same, and a move no
longer collapses the range after it moves, since UIA keeps a collapsed
range collapsed when it moves; the ledger's operations above do not reach
those reads, so their counts are unchanged.

### What the audit found conforming, or left for a decision

- `GetText` is given a length everywhere but a terminal's reads, which
  are bounded by lines (judged fine by Dickson).
- A failed attribute read is "not supported" on both paths, and "mixed"
  is now told apart from it (above).
- A terminal's search uses `FindText` both ways ("A terminal's upward
  search" above), with its text trimmed of the padding and
  line breaks Windows Terminal cannot match.
- Not detected: a provider without a unit silently uses the next larger
  one (`ITextRangeProvider::Move`), so a word move in such a provider
  moves by lines. No call tells it apart; only the terminals' paragraph
  and page, known to be the whole buffer, are refused; left for a
  decision. Some providers also return a positive count for a backward
  move, which NVDA corrects; Verbatim corrects it too since 2026-10-07,
  on both paths (`docs/parity.md`, "Text range moves"), at no call and,
  in a remote program, two instructions where the count's sign is used.
- Not used yet, each a feature of its own rather than a fault in what
  Verbatim reads: `FindAttribute` (finding the next spelling error in one
  call), `GetChildren` and `RangeFromChild` for embedded objects,
  `GetVisibleRanges`, annotation objects and `RangeFromAnnotation`, the
  `IsHidden` attribute, and `ShowContextMenu`.

## The instruction limit

UIA stops a remote operation that executes too many instructions, with
the status `InstructionLimitExceeded` (2). The limit is not published;
NVDA's local emulator of remote operations assumes 10,000
(`phase6-design.md`, item 11 of the work scheduled on 2026-10-07). Measured
on 2026-10-07 on this machine (Windows 11, build 26200, x64) against
mockapp (`crates/mockapp/tests/instruction_limit.rs`), it is exactly
10,000: a loop of 2,498 passes, which executes 10,000 instructions, runs,
and one more instruction stops it. The test finds it by halving the
number of passes and then adding single instructions, and pins it.

Each program's count is taken by running it as Verbatim runs it with
`verbatim_uia_rops::counting` on, which runs the program's counting form
(an `Add` to a counter before each instruction, every jump adjusted) and
records how many of the program's own instructions the run executed. The
counting form executes twice as many, so it can count only programs under
half the limit. Pinned exactly, worst case against mockapp first, the
share of the limit in brackets:

- The focus ancestry: 2,180 (22 percent) for a list sixty groups deep,
  every ancestor up to the window returned under the outpost's depth limit
  of 64, against 64 known ancestors none of which it meets, with the
  list's selected item and a held element's focus read; 1,686 for the same
  stopped at a depth limit of 30, walking on to the window without
  returning the rest; 1,038 for the list whose group is known, its window
  still sixty-one levels up. A deeper tree adds a level of the window walk
  for each level beyond the depth limit, a dozen or so instructions (the
  navigation step's walk, much the same, takes 12): about two hundred more
  levels before half the limit. mockapp's fixtures cannot nest
  deeper than about sixty levels (its JSON parser's recursion limit).
- The navigation step: 875 (9 percent) from an item sixty-two levels
  below its window, 144 from right under it, about 12 for each level
  walked.
- The controlled selection: 168 for a result inside a group of the
  controlled list, 200 for the list's second child, and 215 for an item
  outside the list, all three of the list's elements walked; about 35 for
  each element passed, so about 140 elements fit under half the limit and
  about 280 under the limit, past which the call is answered classically.
  The walk is bounded by the controlled list, not by the query, the one
  program whose worst case is the application's to set; the Settings
  app's search suggestions, checked live on 2026-10-03, were 25.
- The caret read: 4,425 (44 percent) for a line of 64 stretches reached by
  walking one mixed format stretch by words and a mixed word by
  characters, every attribute read (eleven since 2026-10-07, ten of them
  being learned), with a word, the evidence, and a selection's change;
  84 for the report after a focus on a line of plain text with the
  default theme's two attributes, the annotation types and the link,
  whose support is known. It was 4,884 and 85 with seven attributes and
  one: the program now returns each value as the provider gave it, and
  the caller reads it by its type, where the program tested each value's
  type and the annotation types' array itself, which more than paid for
  the four attributes added and the line's own reads. Where every format
  stretch is mixed (mockapp's `mixed.json`: stretches of two characters,
  one of them italic), each is walked by words and its word by
  characters, and each stretch adds 249 instructions (239 with seven
  attributes): 2,148 for 17 stretches and 4,140 for 33, so the 64
  stretches a span may have come to about 8,000 (80 percent), which runs
  under the limit but cannot be counted. This is the one
  program that can come within a factor of two of the limit. A run that
  exceeded it would be answered classically for that call
  (`Path::Fallback`), at a cost of hundreds of calls; programs are not
  made resumable (Dickson, 2026-10-07), and none is needed while the 64
  stretches bound the walk.
- Say-all's batch of twenty lines: 626 (6 percent) a line on from a held
  position with its lines in three languages, each line's then read; 647
  for a first batch from the caret in one language. The count does not
  grow with the lines' length.
- A terminal's screen, with the caret and its line read in the same
  program as the outpost reads a focused terminal: 442 (4 percent) when
  its anchor is nowhere and the search checks its 20 matches
  (`SEARCH_MATCHES`), about 15 each, before the text's rows are counted;
  167 for an anchor found by its text one row up under new output (the
  tail read it replaced took 1,164 and 137, its search checking 64
  matches). Nine of each are the visible range read again at the end, to
  find a view the terminal moved while it was read (2026-10-09). The count
  does not grow with the history or the lines read, except when output
  grows the text while the program walks it: the shift is then found by
  halving the rows it may lie within, about 17 instructions a step, 14
  steps for a growth of 9,000 rows (2026-10-09).
