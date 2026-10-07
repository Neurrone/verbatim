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
`verbatim-inspect latency` prints it with the outpost read stage. For a
caret key, the calls its wait for evidence made are taken when the wait
ends and printed with the caret wait stage instead; the message's total
still holds them, so the counts this ledger pins are unchanged.

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

The UIA operations are measured on the test's own thread, making the same
`verbatim-uia` and `verbatim-uia-rops` calls in the same order as the
outpost's worker once it has the focused element: the outpost finds that
element by reading the system's keyboard focus
(`GetFocusedElementBuildCache`), which a test must not take from the
desktop it runs on. The test counts that one read where the outpost makes
it, so the UIA focus counts below are the outpost's whole count; only the
provider calls of that read are missing from the hits. A UIA focus is
measured both ways the outpost reads it: with remote operations, the
default, and with the classic walk (`uia.remote_operations = false` in
`settings.toml`, or a window whose elements cannot be imported into a
program). The MSAA operations run through a real outpost.

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
  provider calls.
- Today, classic: 3 UIA calls: the focused element, its nearest window
  (`NormalizeElementBuildCache`, since the focus fact carries no window of
  its own for an element that is not a window), and one ancestor hop. The
  last two cost mockapp 95 provider calls.
- Target: 2, met with remote operations: phase 6 step 2's exit criterion,
  asserted exactly.

### A focus change, UIA, cold

- Minimum: 1 window message and 2 UIA calls. The window's provider must be
  probed once in its lifetime for arbitration, the focused element read,
  and one program reads the whole chain up to the top-level window.
- Today, with remote operations: 2 UIA calls and 1 window message: the
  focused element, the probe, and one `Execute`, which walks up to the
  process's top-level window and stops there, never asking for the
  desktop's root. 146 provider calls besides the focused-element read.
- Today, classic: 6 UIA calls and 1 window message: the focused element,
  its nearest window, the probe, and four ancestor hops (the group, the
  window, the desktop's root element, and a hop that finds no parent above
  the desktop and ends the walk). 160 provider calls besides the
  focused-element read.
- Target: 2 UIA calls and 1 window message, met. The classic walk could
  still stop at the desktop's root element, which is known locally,
  instead of asking for its parent.

### A focus change into a list, UIA

The focus lands on the list itself, and its selected item is read with it.

- Minimum: 2 UIA calls: the focused element, and the same program also
  reads the list's selection and the first selected item's cached
  properties.
- Today, with remote operations: 2 UIA calls, the focused element and one
  `Execute`. 126 provider calls besides the focused-element read.
- Today, classic: 5 UIA calls: the focused element, its nearest window,
  one ancestor hop (the window, known from the previous focus), and the
  selected item in two calls (`SelectionPattern2`'s `FirstSelectedItem`
  and `BuildUpdatedCache` on it). 136 provider calls besides the
  focused-element read. Through the `Selection` pattern, before, 6 and
  143 ("A container's selected item" below).
- Target: 2, met.

### Arrowing through a list, UIA

The focus moves from one list item to the next.

- Minimum: 2 UIA calls, as for any steady-state focus change.
- Today, with remote operations: 2 UIA calls, the same as a steady-state
  focus change, 88 provider calls besides the focused-element read; the
  program stops at the list, which the previous chain holds.
- Today, classic: 3 UIA calls: the focused element, its nearest window,
  and one ancestor hop, which meets the list. 95 provider calls besides
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
- Today: 30 MSAA calls and 1 window message: the steady-state count
  (nothing is recognized either way) plus the probe of mockapp's window.
  `WindowFromAccessibleObject` answers no window for mockapp's ancestors,
  whose root object has no parent; until 2026-10-07 their addresses carried
  window 0 and the walk probed it as a second window, which it no longer
  does, since no window is now read as none and the walk keeps the window
  it is in. mockapp answered 39 provider calls.
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

### Entering a dialog, MSAA

A focus moves into a message box, a dialog holding its question as static
text and two buttons, from outside it, so the dialog's own text is gathered
and reported as its description (`docs/nvda/object-model.md`, "A dialog's
own text"). mockapp's `tests/fixtures/dialog.json` is that message box.

- Minimum: a cold focus's 29 MSAA calls and 1 window message, and 14 for
  the text: the dialog's child count and children (2), each of the three
  children's `IAccessible` and role (6) and states (3), and the question's
  name, value, and description (3). The buttons give no text, so nothing
  else of them is read, and the question's neighbor is a button, which is
  never labelled, so its name is not read either.
- Today: 44 MSAA calls and 1 window message, the cold focus's 30 and the
  text's 14. mockapp answered 56 provider calls. A focus moving within the
  dialog does not read it again, since the dialog is then in the previous
  focus's chain; mockapp cannot show that, as every `accParent` it answers
  is a new COM object, so the dialog it reaches from the next button is a
  new node to the outpost.
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
  annotation types (a copy of the caret, `ExpandToEnclosingUnit`,
  `GetAttributeValue`). The character is cut from the line's text, at no
  cost.
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

### A caret wait that finds nothing, UIA

A caret key that changed nothing (Left Arrow at the start of the text):
the wait reads the caret every 10 milliseconds until its 100 run out,
then answers.

- Minimum: one round trip per read: 11 for a wait of 100 milliseconds.
- Today: 11 remotely, each read the whole caret read above, so the read
  that would find the evidence is the answer. Classically 132, 12 per
  read; before remote operations each read was 9 calls (the caret, the
  comparisons, and the line, whose characters at the caret are evidence
  too).
- Target: 11.

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
  stretch a copy, `MoveEndpointByUnit`, two comparisons (with the line's
  end, and of where the next starts), its text, its attributes (one call
  for all of them, `GetAttributeValues`), and moving the walk on, 7 each,
  and a `MoveEndpointByRange` to cut a stretch that runs past the line's
  end, here the last: 39 for four, however many attributes the theme
  reads.
- Today: 1 remotely, 39 classically (8 before formatting was read), with
  the default theme and with every formatting indication on (63 before
  the attributes were read in one call; "Several text attributes in one
  call" below).
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
- Today: 1 remotely; classically 9, as before remote operations (a copy
  of the position, expanded to its line, collapsed, moved, collapsed; a
  copy expanded to the line reached and its text; its `Culture`
  attribute). The first review command and report current object on an
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
  same program; 29 classically. Before, say-all asked for one line per
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
- Today: 5.
- Target: 5.

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

A focused terminal's text changed, and the outpost finds what is new
(`docs/crates/verbatim-outpost.md`, "Terminals"): the anchor's line and
the line before it checked against what they held, the lines to the end
counted, and only the lines spoken read. Measured against mockapp's text
provider (`tests/fixtures/terminal.json`, `tests/terminal.rs`), whose
`set-text` command rewrites the text as a terminal's buffer changes.

- Minimum: 1 UIA call, one remote operations program
  (`verbatim_uia_rops::terminal_tail`), whatever the size of the
  scrollback and however many lines it reads.
- Today: 1 UIA call for a read that finds new output, a prompt that grew
  or a command's output line. A read that finds nothing new after the
  anchor reads the screen afresh to compare it line by line, 1 more, a
  second program that gets the document range itself (2 before, the
  document range read first); so is a terminal's first read when it gains
  the focus, the baseline (1 call, 2 before). Classically, with
  `uia.remote_operations` off or a provider that cannot run programs, one
  call per provider method: 34 for the baseline of a six-line text, 30 for
  a grown prompt, 43 for an output line and a new prompt. The provider's
  own work is the same either way, and pinned too: 13 clones, 6 line
  expansions, 6 reads, 5 moves, and 13 other range calls for the output
  line. The lines spoken are read in one call, however many there are, so
  the cost does not grow with them. Of these calls, the line above where
  the read started, read again at the end, and the last line and the one
  before it, compared with the end of that one read, tell whether the
  text moved while it was read (a full scrollback scrolling beneath the
  ranges during a flood), when the read is set aside for the next.
- Target: 1.

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
  classically 63 UIA calls before and 39 after, 5.26 ms at the median
  before (6.11 at the 95th percentile) and 3.69 ms after (4.29), 200
  runs each; remotely 1 call, 0.36 ms before and 0.39 after. With the
  default theme, which reads only the annotation types, one attribute per
  stretch, the count is the same as before, 39 classically.

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
