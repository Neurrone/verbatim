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
in the same commit. Each operation's failed cached reads are pinned the
same way: a cached read of a property or pattern the element's cache
request did not hold, which costs no call but makes UI Automation raise
and report an error inside itself (`docs/design/uia-shapes.md`,
section 3.6). On 2026-10-13 a UIA focus read with remote operations made
2 or 3 and a remote navigation step 1, reading what the remote program
leaves out of an element's cache; every other operation made none. Since
step 3 of that design, when the program began to return a record of what
it leaves out, every operation pinned makes none, a focus's ancestors read
with descriptions off among them.

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
  by its window procedure: the arbitration probe, the outposts' and the
  focus listener's alike, the raw probe, one `WM_GETOBJECT` with wParam 0
  and nothing more, where `UiaHasServerSideProvider`, which Verbatim does
  not call, sends two, first with a nonzero wParam, and then connects
  (experiment `e39`, `docs/design/focus-pipeline.md` section 20). Also
  a list view's or tree view's `LVM_` and `TVM_` messages, and an edit
  control's `EM_` messages and `WM_GETTEXT`.

Of the UIA calls, those that connect to a window by its handle are also
counted apart (`CallCounts::connections`): an `ElementFromHandle` or
`ElementFromHandleBuildCache`, and a `GetFocusedElementBuildCache`, which
reaches the focused window the same way; the probe is not one, and no
connection follows it (`call_counts.rs`,
`an_outposts_probe_that_finds_a_provider_makes_no_connection`: the probe
costs mockapp one `WM_GETOBJECT`, two `ProviderOptions`, one
`HostRawElementProvider` and one `Navigate`, and nothing more;
`docs/design/listener-without-connections.md`, risk 11). Each still counts once
under its own kind, so a call's total is unchanged. The focus listener
makes none (`docs/design/listener-without-connections.md`, section 2), and
the ratchet asserts its count is zero (`call_counts.rs`,
`the_listener_makes_no_connection`).

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

One operation of the worker runs on another thread, so that it can
have a deadline of its own: a UIA dialog's own text. It runs on the outpost's `verbatim_uia::BoundedClient`, whose
thread counts its calls and hands them back with the answer; they are
added to the worker's count (`verbatim_uia::calls::add`), so the ledger's
counts are unchanged. An operation given up at its deadline has no answer,
and its calls are not counted: they belong to an abandoned thread, as an
abandoned worker's do. A dialog's text now pays one hand-over to that
thread and back, tens of microseconds, and the first one in an outpost
also creates the thread and its UIA client.

UIA's connection timeout is two seconds, UIA's default and NVDA's, and
its transaction timeout five, in every Verbatim process
(`verbatim_uia::CONNECTION_TIMEOUT` and `TRANSACTION_TIMEOUT`), set once
by UIA's first-time setup, where the transaction timeout was UIA's
default 20 seconds. A call that the application answers costs the same;
only a call it does not answer ends sooner.

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

## verbatim.exe's relay

Every fact the focus listener sees passes through verbatim.exe on its way
to the application's outpost, and every request Core makes goes the same
way, so verbatim.exe's share of the time is a stage of its own in the
latency line and in `verbatim-inspect latency` (`docs/archive/relay.md`,
sections 4.5 and 7; `docs/tooling/inspect.md` lists the stages). Its budget is
that verbatim.exe adds at most 1 to 2 ms, read as section 9 of the design
defines it: verbatim.exe's own stage at most 1 to 2 ms, and its added hop
no worse than the platform's floor, every exception attributed.

- verbatim.exe's own time is measured inside verbatim.exe on its
  monotonic clock alone, from taking a fact in (its relay thread taking
  the listener read's completion) or queuing a request, to its relay
  thread beginning the write to the outpost, and travels to the outpost
  as a duration (`EventTiming::core_relay_us`). It does not depend on
  clocks agreeing across processes.
- The spans either side of it, from the listener to verbatim.exe and from
  verbatim.exe to the outpost, are on the clock every process of the run
  shares (`now_us`, `verbatim_process::clock`), as the other
  cross-process stages are.
- A fact held while its outpost started is reported as "waiting for the
  outpost to start", never pooled with verbatim.exe's own time. Since
  2026-10-16 an application's outpost is usually a spare, started before
  it was needed (`docs/design/focus-pipeline.md`, section 19), and the
  wait from the first held fact to `Ready`, which the supervisor logs, is
  the assignment and the hook: a P50 of 1.4 to 3.6 ms in a debug build,
  where starting an outpost took 36 to 118 ms (section 19.9 has the
  figures).
- On the way back, verbatim.exe's reader stamps when it read and parsed
  each outpost message, which divides the old "to Core" stage into the
  pipe and the wait for the reducer, which is the reducer busy with
  earlier input, not the relay.
- Nothing on the relay's path logs: stamps travel in the messages, and
  the platform floor's benchmark (`cargo xtask relay-floor`,
  `docs/tooling/measuring.md`) keeps its stamps in memory until it exits.

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
state (NVDA 2027.1 reads it the same way, and only for menu items). The
focus listener's classic read, the focus-now answer, and a navigation
step's neighbor read it live, one more UIA call (`focus_reports.rs`,
`a_menu_items_legacy_state_is_read_with_its_focus`: 7 calls for a menu
item two steps below the focus before's window). The outposts' remote
program reads it in its one round trip, for an element whose cached
control type is a menu item, at no extra call, a Windows Forms menu item
among them (`remote_ops.rs`,
`a_windows_forms_menu_item_is_checked_from_the_remote_read`). Every other focus and step makes no such read, as the
ratchet's unchanged counts confirm. Caching the state for every element
instead cost no call but roughly doubled the provider work of every focus
change, so it is read only for menu items.

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
and 2 `FragmentRoot` provider calls, made on the process's registering
thread and counted as no call. A UIA focus is measured both ways the
outpost reads it: with remote operations, the default, and with the
classic walk (`uia.remote_operations = false` in `settings.toml`, or a
window whose elements cannot be imported into a program).

The focus listener reads a UIA focus from the event's own element
(`docs/design/focus-pipeline.md`, section 12), so the outpost does not
read the focused element. The test stands in for the listener with the
listener's own reader, counted on the test's thread, and adds its calls
to the outpost's, so the "Today" figures in the four UIA focus sections
below are the listener's and the outpost's together. The listener reads
every focus the classic way, with no remote operation and no connection
(`docs/design/listener-without-connections.md`, sections 2 and 3, step
8): after its checks, the focus's own cache refreshed
(`BuildUpdatedCache`), a list's or tab control's selected item, and one
`GetParentElementBuildCache` per ancestor until a known ancestor, the
desktop's root, or the process boundary. It does not read the focus's
keyboard focus or nearest window again, having read both in its checks.
The outpost makes no UIA call before it reports the focus; after the
report it finds its own copy of the element, one `ElementFromHandle` and
one remote operation or a classic walk, which the worker's count does not
reach. The classic walk compares each child it passes by its runtime id
alone and fills only the element found, as the program does; when it
filled every child it passed, a focus cost mockapp 75 to 164 provider
calls more, and the fill of the window's title bar had UIA ask the
window's thread for the title bar's location (`WM_GETTITLEBARINFOEX`),
which Windows answers for one window at a time across the desktop
(2026-10-24). The provider hits include the listener's read, the
outpost's walk to its own element, and both moves of the focus-following
subscriptions.
The `WM_GETOBJECT` count, 10 in the steady state, comes from the
outpost's `ElementFromHandle` and the listener's and the outpost's
separate clients each connecting; this is recorded as a follow-up for
step 11's measurements rather than changed here. The outposts' refreshed
remote read executes 2,190 instructions at mockapp's worst (2,180 without
the refresh), and the outpost's walk down mockapp's deepest path,
sixty-two levels, 1,139 (`instruction_limit.rs`,
`reads_and_paths_execute_exactly`).

With the listener judging a repeat and testing acceptance before it reads
a focus, in NVDA's order (a revision of the listener judging and reading
each UIA focus from the event's own element, 2026-10-11;
`docs/roadmap-done.md`, "M4, completed so far";
`docs/design/focus-pipeline.md`, sections 16.2 and 16.3), every UIA focus
costs one call more before its read, the nearest window of a windowless
element (`NormalizeElementBuildCache`), and every focus after the first
one more again, the comparison with the last focus accepted
(`CompareElements`, which reaches no provider). The nearest window costs
mockapp 31 provider calls for a button or the list and 22 for a list item.
A repeated focus on the element that still has the keyboard focus is
dropped after the comparison and that element's keyboard focus read: 2
calls and 3 provider calls, where it was read in full, 1 call and 146
provider calls, and then dropped by Core (`call_counts.rs`,
`uia_repeated_focus_costs_exactly`).

With NVDA's last check before it takes a focus, the sender's own keyboard
focus read with a plain property read (`shouldAllowUIAFocusEvent`;
2026-10-12, `docs/design/focus-pipeline.md`, section 16.8), every UIA
focus that passes the acceptance test costs one call more before its
read (`CurrentHasKeyboardFocus`), 3 provider calls in mockapp: the
property, the host provider, and the fragment root. The read does not
check it again: a focus that moves on while it is read is followed by
the focus event of where it went (`call_counts.rs`,
`uia_focus_changes_cost_exactly_remote` and
`uia_focus_changes_cost_exactly_classic`).

With the listener's judgement ported from NVDA's focus handler in its order
(2026-10-12, `docs/design/focus-pipeline.md`, section 18), NVDA's native
check comes before the duplicate rule, so a repeated focus on a windowless
element costs one call more, its nearest window
(`NormalizeElementBuildCache`), 3 calls in all
(`uia_repeated_focus_costs_exactly`); the window's verdict is kept, so it
asks the window nothing. A focus on a console window's own element is
dropped after the console's provider check instead of before any call.
That check, made once per console window, is made by the listener from
the held sender, with no connection to the window (2026-10-19,
`docs/design/listener-without-connections.md`, section 3, step 5): four
calls from the console window's element (the text area's search, its
text pattern, its visible ranges and the range's font) and two from the
text area, whose text pattern the event's cache holds (its visible ranges
and the range's font), where the check by the window's handle made five
from either (`focus_reports.rs`, `a_console_windows_own_focus_is_never_reported`
and `a_console_text_areas_focus_is_checked_from_the_text_area`).
Every other focus costs what it did: the outpost takes the listener's
verdict for the focus's window rather than asking the window again.

### A focus change, UIA, steady state

- Minimum: 5 UIA calls for a sender without a window of its own, as
  mockapp's are, and 4 for a sender that is a window: the calls NVDA's
  focus handler makes, in its order (`docs/design/focus-pipeline.md`,
  section 18; Dickson, 2026-10-16), and the read, made with calls on held
  elements only, since the listener makes no remote operation
  (`docs/design/listener-without-connections.md`, section 2, decision 12).
  The element itself costs no call: the listener holds the event's sender
  and reads from it. The calls are:
  - the sender's nearest window (`NormalizeElementBuildCache`), for NVDA's
    native check and acceptance test; a sender that is a window answers
    it from the event's cache without a call. NVDA makes it before its
    duplicate rule, so it stays there;
  - the comparison with the last focus accepted (`CompareElements`),
    NVDA's duplicate rule, made while that focus is still the last focus
    queued. It reaches no provider, but it is a UIA client call and is
    counted as one, as NVDA makes it;
  - the sender's keyboard focus, read plainly (`CurrentHasKeyboardFocus`),
    NVDA's last check: read inside a remote program, the element is the
    provider's own, which for Windows Terminal's second focus identity
    answers that it has the focus where the plain read, NVDA's, answers
    that it has not (section 16.8);
  - the sender's cache filled afresh (`BuildUpdatedCache`);
  - one step up (`GetParentElementBuildCache`) to the group the previous
    focus was in, a known ancestor, whose own ancestors are reused.

  A remote program would read the cache and the walk in one call, but
  only the application's outpost may send one into it (section 2 of the
  same design).
- Today: 5 UIA calls, all the listener's, whichever way the outpost walks
  to its own copy of the element. 303 provider calls in all with the
  outpost's remote walk, 297 with its classic walk, the subscriptions'
  moves included; one of them is the `GetPropertyValue` that the
  provider description cached with the outpost's lookup of the route's
  window costs, for the proxy rule (`docs/design/listener-without-connections.md`,
  section 5), in the same call. The listener's one remote operation made 4 calls and
  331 provider calls; its classic read with the keyboard focus and the
  nearest window asked again made 7 (2026-10-19, section 10, step 6).
- Beside the read, the reading threads' watchdog judges once whether the
  user has already moved on from the read's window when that becomes
  known (0b5826cd): for a sender that is a window, as the read starts,
  one run of the moved-on rule's local calls (`GetGUIThreadInfo` and two
  `GetWindowThreadProcessId`, then, when the two windows are on different
  threads, each window's provider process: `GetClassNameW` and
  `GetWindowThreadProcessId` for each, and for a console window its
  client's `OpenProcess`, `NtQueryInformationProcess` and `CloseHandle`,
  kernel calls that wake no other thread, as the window filters pay) by
  the watchdog under the readers' lock, on
  the wake-up the read's start already makes; for a windowless sender,
  the same calls on the reading thread once its nearest window is found,
  and only then one boxed closure for the read, made as it starts, which
  wakes the watchdog only when the front is another thread of its
  application's. The closure
  is made only for a focus or a menu opening whose sender is windowless,
  never for a selection or a notification, whose reads are not judged. No call
  into the application, and no wake-up beyond the start's.
- In the outpost's intake, as the read takes its marker's place, a
  comparison of the focus before's runtime id with each of the read's
  ancestors' runtime ids, which the read already carries (about six in
  Settings' Date and time page), under the lock the push takes anyway;
  only when the new focus is inside the old does the intake clone the old
  focus's runtime id once, scan the entries behind the marker up to the
  next marker or idle query, and move the old focus's own UIA state
  changes ahead of the read (`docs/design/focus-pipeline.md`,
  section 12.4). No call and no wake-up. The focus report is delayed only
  when events are moved, by their handling time, 0.2 to 1.8 ms for each
  property change in Settings' Date and time page traces of 2026-10-21;
  the call counts above are unchanged.
- In the outpost, after its remote focus program's walk ends at a null
  parent at a window's own element, one local `GetAncestor` to tell
  whether that window is top-level, and for one that is not, the process
  boundary check's local calls (`parent_across_process_boundary`), and,
  when neither finds the top and the last element's cached window handle
  is another, the same calls for that handle, one `GetAncestor` and, for
  a window that is not top-level, the boundary check's, so a walk is
  complete only at the top (since 2026-10-22). No call into the
  application, no wake-up, and no instruction more in the program.
- Target: the minimum, which the listener meets.

### A WinEvent focus in a UIA window, and the read when no UIA focus follows

- Per focus, steady state: nothing added. The outpost's worker drops a
  `WinEvent` focus copy whose window uses UIA and records it
  as a gap in its own state, with no lock and no call; the UIA focus read
  from the listener closes it. The gap's time is given to the wait the
  worker already makes for its next entry (`Intake::next_with_gap`),
  which takes the intake's lock anyway; the UIA focus's marker wakes the
  worker, so a gap answered within the bound sets no
  timer of its own and costs no wake-up. The news of the focus markers
  the intake resolved without a read comes with each entry the worker
  takes, two optional times copied under the lock `next_with_gap` takes
  anyway.
  Each of the listener's drops carries the JSON field `"evidence"` with
  its class's name and each focus read `"focus":true`, about 13 bytes more
  on every listener focus message; the listener, the supervisor and the
  outposts build from one `protocol.rs`, so neither needs a version.
  Nothing is read in steady state on any drop: a drop that ends the wait
  closes the gap, and a drop that could not establish the focus, which
  ordinary fast navigation makes, leaves the gap to its bound with no
  read and no wake-up, the next UIA focus closing it; only a drop for a
  sender with no window above it, which comes in a new window's first
  burst, reads at once. A read answers a copy only in the copy's
  top-level window: one local `GetAncestor` for each copy in a UIA window
  and one for each focus read from the listener, no call into any
  application and no wake-up. The existing debug lines gain fields
  (`opened_gap`, `answered_gap_ms`), and one debug line, written only
  with debug logging on, is added when a drop or the end of the
  listener's reads closes a gap. Not traced; by construction.
- The idle barrier's answer waits while a gap is open, a cost to the
  harness alone: until the bound; while a marker waits for its
  resolution then, until it is resolved, up to the listener's read
  deadline of 11 seconds (`READ_DEADLINE`), and that again for a marker
  queued behind its application's abandoned reads; then the read, if the
  gap comes to one. For one focus marker on an application that answers,
  that is about 12.2 seconds at most, within the harness's 15-second idle
  timeout; a slower application fails the step at that timeout, which is
  the right outcome.
- Per gap that comes to a read, once a second has passed with no UIA
  focus, or at once when the UIA focus was dropped because its sender had
  no window above it (`docs/design/listener-without-connections.md`,
  decision 11): one wake-up of the worker at the bound, unless another
  entry wakes it first, and the focus-now query's read
  (`read::focused_control_in`): its local checks of the focus window,
  that the application serves it and that it is a window, has no ghost
  window standing in for it and is not hung, so a hung application is
  not called; then, when the window uses UIA, one
  `GetFocusedElementBuildCache`, the enrichment by one remote operation
  or the classic walk, and the MSAA continuation above the process
  boundary, as a focus-now query costs (`call_counts.rs`), and one
  `ElementFromHandleBuildCache` more for an ancestor that is a window's
  own element read inside the provider without its handle, which the
  console host's window is (`docs/uia-bugs.md`), read again through the
  window's proxy; and one report to Core. At most one read a gap. When the late UIA focus of the element
  read then comes, the outpost finds its own copy of the element by the
  path again (`find_focus_element`), since the listener holds no element
  for the read: one connection more per recovered focus. A gap whose copy
  the worker took up after a hold can be due at once, since the bound
  counts from the copy's observation.
- Reads made in vain, accepted as the design's cost: in
  `measure_focus_explorer` a gap can come to a read in the first folder
  window as it takes the focus back, after the focus has moved to a new
  window of another process, whose foreground `WinEvent` comes about
  200 ms before it takes the foreground. The read ends at its local check,
  about 110 µs with no call into the application; the outpost's own hook
  sees only its application's events, so nothing it has says the other
  window is coming.
- Target: no cost per focus; one read per gap.

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
focus when it runs. From then until 2026-10-11 a focus change went before
the events of other objects queued ahead of it, and the events of the
focus and of the object the focus moved to kept their place.

mockapp's `a_focus_is_handled_before_slow_reads_queued_ahead_of_it`
reproduces it: every provider call answered 20 ms late (`slow 20`), ten
selections in a list queued behind a query, then a focus. Before the
change the focus was taken up 611.7 and 610.6 ms after the query was
answered (two runs), after the ten selections; after it, 35 and 115 µs,
before them. The test asserted the order exactly and that the focus waited
less than one of the slow application's calls after the query.

Removed on 2026-10-11 with the test (`docs/roadmap-done.md`, "M4, completed so far";
`docs/design/focus-pipeline.md`, sections 5 and 9): the outpost handles
events in the order they arrive, as NVDA does, so this wait can return
until a UIA focus is judged as it arrives (the listener judging and
reading each UIA focus from the event's own element, `docs/roadmap-done.md`,
"M4, completed so far"), and an MSAA
focus waits behind the events queued before it as NVDA's does.

### A focus change, UIA, cold

- Minimum: 1 window message and 4 UIA calls for a sender without a window
  of its own, as mockapp's are, and 1 window message and 3 UIA calls for
  a sender that is a window, plus one step for each ancestor up to the
  desktop's root: the sender's nearest window, for a windowless sender;
  the window's provider, probed once in its lifetime for NVDA's native
  check (the listener's raw probe, one `WM_GETOBJECT`); the sender's
  keyboard focus, read plainly; the sender's cache, refreshed; and the
  steps. There is no comparison, since no focus was accepted before.
- Today: 6 UIA calls and 1 window message, all the listener's: the
  sender's nearest window, its plain keyboard focus read, its refreshed
  cache, and three steps, to the group, to the window, and to the
  desktop's root, where the walk stops; and the raw probe. 346 provider
  calls in all with the outpost's remote walk, 337 with its classic walk.
  The raw probe costs mockapp one `WM_GETOBJECT`, with wParam 0, where
  `UiaHasServerSideProvider` cost it two, first with a nonzero wParam and
  then with wParam 0, and seven more provider calls as UIA connected (five
  `ProviderOptions`, one `HostRawElementProvider` and one `Navigate`).
  The listener's one remote operation made 3 calls and the window
  message, 367 provider calls.
- Target: the minimum. The walk could still stop at the desktop's root
  element, which is known locally, instead of asking for its parent.

### A focus below the process boundary, UIA

The focus is in a window whose parent window belongs to another process,
as the Settings page's content is inside `ApplicationFrameHost`'s frame
(`docs/design/listener-without-connections.md`, section 3, steps 8 and
11). The ratchet makes it with two mockapps, one's UIA window made the
child of the other's MSAA window (`call_counts.rs`,
`uia_focus_chains_stop_below_the_process_boundary`), as a cold focus.

- Minimum: the listener's calls of a cold focus, none of them a step into
  the parent's process; the outpost's probe of the parent window and the
  MSAA calls that read its client area and the chain above it.
- Today: the listener's 5 UIA calls and 1 window message, as for a cold
  focus but for the step to the desktop's root: the walk's two steps end
  at the child's window, which costs only local window calls. The outpost
  then makes 9 MSAA calls, its client area and the chain above it, and no
  probe of the parent, the other process's window
  (`docs/design/listener-without-connections.md`, section 4.3); with the
  probe it made 1 window message more. Above a parent with
  a UIA provider it makes 23 MSAA calls, which MSAA's standard proxy
  answers in mockapp, and no UIA call of its own
  (`uia_focus_chains_continue_through_msaa_above_a_parent_with_a_provider`);
  UIA's client side still connects to that parent as the outpost's read
  returns the child window's element (decision 13): once by the remote
  program and twice classically, each connection one `UiaRootObjectId`
  `WM_GETOBJECT` with a nonzero wParam, one with a zero wParam, and 6
  `ProviderOptions`, 2 `HostRawElementProvider` and 2 `Navigate` calls,
  and once in a focus-now query by either path. A parent without a
  provider receives the nonzero message alone as often, an owner
  nothing. While the
  listener reads, the parent's process receives 2 `WM_GETOBJECT`s, each
  with `UiaRootObjectId` and a nonzero wParam, which UIA's client side
  sends itself when a call on a held element returns the child window's
  element (the nearest window and the read); no other call. Before the
  walks stopped at the boundary, their step up into the parent cost it 6
  `WM_GETOBJECT`s and 17 MSAA calls.
- Target: no message to the parent from the listener or the outpost,
  which UIA's client side does not allow while either holds the child
  window's element; accepted as a known limit (decision 13).

### A focus change into a list, UIA

The focus lands on the list itself, and its selected item is read with it.

- Minimum: 7 UIA calls: the five of a steady-state focus change on a
  windowless element, and the selected item's two (`SelectionPattern2`'s
  `FirstSelectedItem`, fetched with the `Selection` pattern, and
  `BuildUpdatedCache` on it).
- Today: 7 UIA calls, all the listener's. 339 provider calls in all with
  the outpost's remote walk, 334 with its classic walk. The listener's
  one remote operation made 4 calls, 343 provider calls.
- Target: the minimum.

### A focus change onto a combo box without the Value pattern, UIA

The focus lands on a combo box whose value is its first selected item's
name (`verbatim_uia::map::value_from_selection`), and the item is read
with it (`crates/mockapp/tests/combo_box.rs`); then another of its items
is selected. Pinned by `crates/mockapp/tests/call_counts.rs`,
`uia_combo_box_costs_exactly`, with the outpost's remote program and its
classic walk.

- Minimum: for the focus, 7 UIA calls, a steady-state focus change's
  five and the selected item's two, as for a list (`SelectionPattern2`'s
  first selected item and `BuildUpdatedCache` on it); for the selection,
  1 UIA call, the search that finds the item among the combo box's
  children with its name and keyboard focus.
- Today: the focus, 8 UIA calls, all the listener's, which reads every
  focus the classic way and has no remote path: the five of a
  steady-state focus change on a windowless element and the selected
  item's three through the `Selection` pattern, as for a tab control,
  since mockapp's combo box has no `SelectionPattern2`. The outpost's
  walk to its own copy after the report adds none the worker counts,
  remote or classic. A combo box with the `Value` pattern reads no
  selected item and asks nothing more for it: the listener asks its fresh
  copy of the element, the outpost's remote program asks the element
  inside the provider, and the outpost's classic fallback takes it from
  the node's cache (`FocusQuery::value_available`). A looping selector
  is read as a list is, 7 calls. The selection, 2 UIA calls in the
  outpost: the test every selection is given first, for a selection in a
  list the focus controls, as NVDA's `event_selection` reads its focus's
  `ControllerFor` first (one remote operation, or classically the
  relation's read, which names nothing here), and one
  `FindFirstBuildCache` over the combo box's children, whose cache holds
  the item's name and keyboard focus.
- Target: the minimum for the focus, met by a provider with
  `SelectionPattern2`; for the selection, the controlled-list test and
  the child search made in one remote operation, 1 call.

### Arrowing through a list, UIA

The focus moves from one list item to the next.

- Minimum: 5 UIA calls, as for any steady-state focus change on a
  windowless element.
- Today: 5 UIA calls, the same as a steady-state focus change; the walk
  stops at the list, which the previous chain holds. 318 provider calls
  in all with the outpost's remote walk, 315 with its classic walk. The
  listener's one remote operation made 4 calls, 346 provider calls.
- Target: the minimum.

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
  be reported instead (1, `docs/parity/windows-and-processes.md`, "A control's own focus with a
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
  controlled elements, and the same answer as the classic calls. An item
  found in none is answered with the first controlled element's current
  selection, read in the same operation; the classic calls take three
  more for it (the Selection pattern, its current selection, and the
  selected item's cache). A selection that names the focus itself reads
  no current selection, by either path; telling it costs one comparison
  of runtime ids per UIA selection while the focus is a UIA element, and
  no lock.

### An event from a window not related to the foreground window, MSAA

A state change on the focus while another application's window is in
front, and the same change with the focus's window in front
(`msaa_state_change_costs_exactly`). The outpost tests each event by
NVDA's acceptance test against the foreground window before anything else
(`docs/parity/focus-events.md`, "Event acceptance").

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
  change it reported and no focus Core queued from another application
  since: NVDA drops it as a duplicate. Before the MSAA work
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
outpost holds, here one kept from a property event and resolved on the
bounded client's thread as the `Kept` shape, of which nothing is read, so
no cached read fails (`docs/design/uia-shapes.md`, step 4).

- Minimum: 1 UIA call per container read: the dialog's children, with
  every property the rules look at cached, in one `BuildUpdatedCache` whose
  scope is the children alone, as NVDA's is.
- Today: 1 UIA call. mockapp answered 135 provider calls: 26
  `ProviderOptions`, 33 `GetPatternProvider`, 43 `GetPropertyValue`, 11
  `HostRawElementProvider`, 9 `Navigate`, 6 `GetRuntimeId`, and 7
  `FragmentRoot`. It answered 172 while the scope took in the dialog's
  own element too, filled with the children's properties though nothing
  read them, and 208 before each child was read as the `DialogText`
  shape, which leaves out the properties the rules never read, the
  bounding rectangle and whether it is off the screen among them
  (`docs/design/uia-shapes.md`, step 3).
- A message box that is its own top-level window has the title bar UIA
  adds for the frame as its first child, which gives no text. Reading
  whether it is off the screen had UIA ask the window's thread for the
  title bar's information (`WM_GETTITLEBARINFOEX`), which Windows answers
  for one window at a time across the desktop, about 6 ms alone and
  seconds under load, so the child is read without it, and mockapp's
  window takes no such request (`call_counts.rs`,
  `uia_dialog_window_text_reads_nothing_of_its_title_bar`, 2026-10-24).
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

### A key through the keyboard hook

Passing a gesture from the keyboard hook to the gesture router.

- Minimum: none: the hook decides the key from its own state, and nothing
  about the application is read.
- Today: none. The hook sends each gesture and harness mark on an
  unbounded channel (`docs/crates/verbatim-input-windows.md`), so a key's
  wake-ups are unchanged: the send wakes the router once, as before. The
  channel stores its messages in blocks of 31 slots, so the hook thread
  allocates one block whenever the last block fills, about once every 31
  sends, and the router frees it once it has read the block. While the
  router is backed up, each queued gesture holds about 70 bytes, its
  48-byte slot and its gesture name.
- Target: none.

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
  (`verbatim_uia_rops::caret_read`). Classically, 15 calls one at a time:
  the selection (`GetSelection`), whether it is empty (`CompareEndpoints`
  of its two ends), the evidence (`CompareEndpoints` with the caret Core
  knew), the line (a copy of the caret, `ExpandToEnclosingUnit`,
  `GetText`), the caret's offset in it (a copy of the line,
  `MoveEndpointByRange` to the caret, `GetText`), and the character's
  annotation types and link (a copy of the caret, `ExpandToEnclosingUnit`,
  and one `GetAttributeValues` call, which the provider answers with two
  `GetAttributeValue` reads since the default theme reports links, one
  before), and the caret read again with its comparison (`GetSelection`,
  whether it is empty, and `CompareEndpoints` with the caret first read),
  since a provider takes each call on its own and the application can
  move the caret between them (2026-10-14, the console host writing a
  typed character between the caret's read and its line's). The character
  is cut from the line's text, at no cost.
- Today: 1 remotely, 9 calls before remote operations (2026-10-06, with no
  formatting). The program makes inside the provider the classic reads'
  provider calls and one copy more (a `Clone` and a `MoveEndpointByRange`
  collapsing the caret), plus the import of the element and its text
  pattern (`GetPatternProvider` and four more provider calls UIA makes).
  Classically, with remote operations off or a provider that cannot run
  programs: 15, the 9 of before, 3 for the character's spelling error,
  and 3 for the caret read again (12 before 2026-10-14).
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
  check that finds the evidence is the answer; classically 15 for each,
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
- A watch whose read finds no evidence, with something selected in the
  report its caret was known from and now, compares that report's
  selection's ends with the selection now when a caret report read after
  the key replaced the kept selection the read compared with: up to two
  comparisons of positions, `CompareEndpoints` calls through UIA (one
  round trip each) and none in an edit control, whose positions are
  offsets (since 2026-10-22). Otherwise, the usual case, the read's own
  comparison covers it and nothing more is made. A possible reduction:
  the caret history could record whether a report changed the selection,
  so the watch's two `CompareEndpoints` round trips go away.
- Over a selection, any key compares with the selection the outpost kept,
  in the same round trip, as a caret report does (below); no key sends a
  selection of its own since 2026-10-22 (a selecting key's comparison
  costs what it did), so Backspace,
  Control+Backspace, Delete and Control+Delete over a selection now pay
  what a movement leaving a selection paid already: through UIA, the
  comparison and the texts read in the program, no round trip more; in an
  edit control, one sync of the `WinEvent` thread for the value change
  check ("A caret move, Win32 edit control"). Through UIA the program
  also reads the old selection's text as its ends now hold it, when that
  selection held something, to compare with its text as it was reported:
  a `GetText` inside the provider, and one call classically, each read
  up to 513 UTF-16 code units (`verbatim_uia_rops::MAX_COMPARED_TEXT`,
  one more than the 512 characters from which a selection is spoken as a
  count). A key that leaves no selection and found none pays
  nothing more: the program's comparison is skipped when neither holds
  anything.
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
which follows every typed character, compared with the selection of the
outpost's last report for the node, for a selection no key made, which
Core speaks. Measured in mockapp's text by
`crates/mockapp/tests/call_counts.rs`, both ways: the caret moved one
character with nothing selected before or after, and then "al" selected
from the caret.

- Minimum: 1 UIA call remotely; classically 8, a caret move's without the
  comparison or the formatting, when nothing is selected before or after,
  since NVDA's comparison has nothing to say then; with a selection, the
  comparison and the change's text.
- Today: 1 remotely, in either case: the comparison with the last
  selection, the change's text, and the new selection's text up to 513
  UTF-16 code units, which is kept with it for the next comparison (a
  `GetText` inside the provider, since 2026-10-22, made only when the
  selection's ends moved: when they did not, what the old ends hold now
  is its text and is kept), are part of the caret read's program,
  which compares nothing when the last selection and the new one both hold
  nothing. Classically 8 with nothing selected (8 before remote
  operations), and 21 for the selection of "al" (25 before 2026-10-23,
  18 before 2026-10-22, 19 before the selection's start was read): the
  read of a caret with text selected, which asks for the selection's
  caret (`GetCaretRange`), two comparisons, the selection's ends with the
  old caret's (the old selection holding nothing, the change is the
  whole new selection, as NVDA speaks it, with none of the four
  comparisons that place a change, since 2026-10-23), the change's text (a copy,
  its two ends moved, its text), the selection's text, and where the
  selection starts, since `TextPattern2`'s caret can be its other end
  (its start compared with the line's, a copy of the line or of the
  earlier line it starts in, an end moved, its text: 4, and 3 more, the
  line expanded and its text, for an earlier line), and, the start being
  at its line's start, as "al" at the text's start is, whether it is the
  text's (the document range and a comparison: 2). Inside a line it costs
  19. Through UIA all of this is in the program. When the last selection held something, what
  its ends now hold is read too, up to the same 513 units, to compare
  with its text as it was reported; through UIA a mismatch has the change
  said in general terms (`docs/parity/text.md`, "TextInfo-equivalent
  layer"), and costs one comparison of two strings of at most 513 units.
  A longer text is compared by its first 513 units and by being longer,
  so an edit that leaves both as they were is compared in specific terms,
  as NVDA compares it. With unchanged ends nothing is read of the new
  selection: a report whose ends equal the kept selection's and whose
  old range still holds its text reads no selection text at all.
  A possible reduction classically: when the selection and the caret are
  on the same line, the selection's start is the caret's offset minus the
  selection's length, two calls fewer.
- Target: 1.

Local work beside the calls, which by "What counts as a call" counts as
none: each report looks the node up in the registries (the UIA registry,
then the MSAA registry), and, a UIA text
change being no reason to change the comparison, scans no intake for one;
and each report, a caret key's answer included, keeps its selection's
ends for the next report to compare with, a copy of two text positions,
which for UIA adds two references to ranges the outpost already holds
(reference counting is not counted). A terminal's caret event's report
does neither, since none of its reports compares a selection; a key's answer
in a terminal still keeps the ends, its watch not knowing the node is a
terminal.

### The caret read before a held key

A caret key pressed while the focus has text is held back by the hook
until the focus's outpost has read the caret (`TextOp::ReadCaret`,
[holding caret keys](design/key-holding.md)); the key's watch then runs
as for any caret key. The read is a caret event's report's read that
compares no selection. Measured by `crates/mockapp/tests/call_counts.rs`
(`caret_moves_cost_exactly`): the caret moved on with nothing selected,
after a report of a selection, through UIA both ways and in the Win32
edit control.

- Minimum: 1 UIA call remotely, 8 classically, a caret report's; 5
  window messages in a Win32 edit control.
- Today: the minimum: 1 UIA call remotely, with the caret report's
  provider calls (the element's import and its text pattern, the
  selection, two copies, a comparison, the line expanded, its text twice,
  an endpoint moved, and the program's own copy of the caret); 8
  classically; 5 window messages in the edit control. Every one is
  pinned.
- Target: the minimum. A possible reduction: the watch's first check,
  which follows the read by a few milliseconds and finds nothing when the
  application has not yet handled the key, could be skipped when the
  read is that recent.

Beside the calls, a held key costs the hook's thread two wake-ups more,
for the key sent again and its release, and the reducer thread and the
outpost's worker the read's round trip. In the terminals, traced with
`cargo xtask trace scenario` over typing and arrows in a debug build, this
came to roughly 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time per
held key (6 and 7 percent more wake-ups in the console host's and Windows
Terminal's windows, [holding caret keys](design/key-holding.md), "Costs").

The other costs of holding, none of them a call into an application:

- Each key transition deferred behind a held key (a typed character, a
  modifier, a release, Enter) is swallowed by the hook, wakes the reducer
  thread once more (`ShellCommand::DeferredKey`), is sent again with
  `SendInput` once the keys ahead of it have gone (one `SendInput` for
  everything that may go at once), and passes through the hook a second
  time, where its tag passes it straight on.
- Each change of the foreground window costs the hook's thread
  `GetWindowThreadProcessId`, `OpenProcess`, `OpenProcessToken`, two
  `GetTokenInformation` and two `CloseHandle`, to learn whether the new
  window's process has higher integrity than Verbatim's (the guard for
  elevated windows, `verbatim-input-windows`'s `integrity` module); a key
  costs none of them.
- The hook's thread now also wakes for foreground changes among
  Verbatim's own windows, since its foreground event hook no longer skips
  Verbatim's process (`WINEVENT_SKIPOWNPROCESS` dropped in `typed.rs`), so
  that the guard sees Verbatim's own window come to the front.
- A held key that waits for the watch of the key ahead of it costs the
  reducer thread one wake-up more, at the wait's bound, 100 ms after the
  key ahead was passed on (`Effect::BoundHeldKeyWait`), whether or not
  that watch was answered first: the shell keeps the bound until it is
  due, so a watch answered early still wakes the thread then, and the
  wait's end does nothing (`Input::HeldKeyWaitEnded`); the bound is one of the
  reducer thread's existing deadlines, so it adds no thread, timer or
  wake-up source. Holding a key that moves nothing with auto-repeat makes
  every repeat wait for it (`measure_held_repeat_text_box`):
  the last of 15 repeats reached the text box 1.05 to 1.06 s after the
  release (about 110 ms per repeat), against 149.2 s before the bound
  (`target/e2e-evidence/m42-key-holding-fixes-2026-10-22/held-repeat-analysis.txt`).

### The system caret's events

The system caret's location change or show (`OBJID_CARET`), which NVDA
handles as a caret event on its focus (`docs/parity/text.md`), measured
from the outposts' logs on 2026-10-20 over three runs each of
`notepad_editing` and `conhost_editing`
(`target/e2e-evidence/fixes-item-11-2026-10-20/commit1-caret-counts.txt`).

- Minimum: no call into the application for an event that tells of no
  move the focus does not report itself; for one that does, the caret
  report's calls, 1 UIA call remotely.
- Today: each event the hook delivers wakes the outpost's event thread,
  which drops it, waking nobody, while the focus has caret events of its
  own (a UIA focus once it has raised a UIA caret event, and a focused
  console host terminal). Every other one wakes the worker, which makes
  NVDA's acceptance test as a location change, local calls only (the
  window's class name and process, its root owner and the foreground's,
  `IsChild`, its extended style, and `GetGUIThreadInfo` for a
  `Windows.UI.Core` window), then the window's readability check, local
  calls too (`IsWindow`, `GhostWindowFromHungWindow` where user32 has
  it, and `IsHungAppWindow`), and then reads the caret as a caret report
  does, unless a read since the event covers it. In `notepad_editing`
  the hook delivered 47 to 52 per run, 34 to 39 of them shows, and 3
  woke the worker, each a caret read: those before the document's first
  UIA caret event after each of its two focuses, the first move after a
  focus being read twice, so 16 to 18 caret reads per run against 16
  with each move read once. In `conhost_editing` it delivered 7 and none
  woke the worker: 8 caret reads, one for each console caret event and
  one after the focus. A classic edit control's caret show comes with
  each repaint, each a caret read, as in NVDA.
- Target: the minimum. It is met but for the first move after a focus
  in an application that raises both, whose system caret's event comes
  first, before anything tells that the focus reports its own moves.

### The focus-now answer on a text field

The focus-now query's answer for a text field, which Core asks for at
its start, after a replaced outpost, and after a menu closes or Alt+Tab:
the focused control's caret read with it, by the reported focus's own
read, and carried in the answer (`FocusedControl::text`), so the field's
line is spoken and Core has its caret. Pinned in
`crates/mockapp/tests/call_counts.rs`
(`text_field_focuses_cost_exactly`), mockapp's UIA document with
nothing selected. The MSAA edit control's answer is not pinned: the
MSAA focus-now read takes the system's keyboard focus
(`verbatim_ia2::acquire::focused_snapshot`), which no test on a desktop
of its own can give mockapp's edit control.

- Minimum: the query's read, 2 UIA calls and 1 window message, and what
  the caret read adds to a reported text focus: the text pattern's fetch
  and one program remotely, 2, and the fetch and 35 calls classically
  ("The caret report after a focus, UIA" below), 36. No round trip is
  added beyond what a reported text focus pays for its text.
- Today: 4 UIA calls and 1 window message remotely, 40 and 1
  classically (2 and 1 before 2026-10-22, when the answer carried no
  text). A selection adds what it adds to a reported focus ("A focus on
  a text field" below). Once the answer is sent the control becomes the
  outpost's focus, which makes no call; a terminal's follow-up then
  makes the baseline read of its text a reported terminal focus's
  follow-up makes, outside the answer's count.
- Target: the minimum, met.

### The caret report after a focus, UIA

The caret read of a text focus, whose line Core speaks, with the line's
formatting: mockapp's first line has four stretches (bold "alpha", a
space, the misspelt "beta", the line feed). It is made within the focus's
handling and counted in the focus's one message ("A focus on a text
field" below); this entry is the read alone, as `caret_report` makes it.

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

### A focus on a text field

The whole cost of a focus on a text field, its caret read while the
outpost handles the focus and carried in the focus's one message
(`FocusChanged`'s `text`), each the first focus in its window, pinned in
`crates/mockapp/tests/call_counts.rs`
(`text_field_focuses_cost_exactly`). The cost is paid for every text
focus the outpost handles, whether or not a newer focus supersedes it
before it is spoken, as NVDA reads an object's text for every focus it
queues. The parts of a UIA focus's count are:

- the listener's read: 5 UIA calls and 1 window message;
- the outpost's search for its own element: 2 UIA calls and 1
  connection remotely, 4 calls and the connection classically;
- the focus's enrichment: 1 UIA call;
- the caret read: 1 UIA call remotely; classically 35 for mockapp's
  document ("The caret report after a focus, UIA" above), 43 when
  something is selected (the selection's caret, its text up to 513
  units, kept for the next comparison since 2026-10-22, and where it
  starts, here at the text's start, 6), and 17 for its terminal;
- the selected text, read only when something is selected, a
  protected field's too, whose masked text Core says as any field's: 1
  UIA call remotely and 4 classically, the read of the range alone,
  since the caret read has already fetched the selection ("The selected
  text, UIA" below counts 7 for a read that fetches it too).

- Minimum: each part's own minimum: the listener's read, whose minimum
  "A focus change, UIA, steady state" gives, here 5; the search's
  `ElementFromHandle` and one remote operation; and one call each for
  the enrichment, the caret and the selected text. That is 9 UIA calls, 1 window message and 1
  connection remotely for a document with nothing selected, and 10 with
  a selection. Classically, the walk's 4 calls replace the remote
  operation's 2, and the caret read costs its classic minimum.
- Today: a UIA document (mockapp's `text.json`) read remotely costs 9 UIA
  calls, 1 window message and 1 connection; 10 with a selection,
  protected or not. Classically it costs 45; 57 with a selection,
  protected or not (50 before the selection's text was read to keep,
  and 51 before where it starts was read, 2026-10-22). A UIA terminal
  (`terminal_reading.json`) costs 9 remotely and 27 classically, each
  with 1 window message and 1 connection. A classic edit control through
  MSAA costs 39 MSAA calls and 6 window messages. With nothing
  selected, these are the calls the focus and its caret report made when
  the caret report was a message of its own after the focus. The search, then made after the
  focus's message and counted in neither, now comes before the focus is
  reported for a focus that may have text, so the one message counts
  it, and the first word waits for the search and the caret read. A
  focus without text is reported before its search, as it was.
- Target: the minimum, which the remote counts meet; classically,
  today's counts.

Measured on 2026-10-22 as the measuring guide gives
(`docs/tooling/measuring.md`): debug builds at info level, five runs
each of `measure_focus_text_box` and `measure_focus_new_application`
with `cargo xtask measure`, before at dd363ecc (named with
`--verbatim-exe`) and after at 6a644058, which also holds the 21 commits
after the change. Figures are in milliseconds as minimum, P50, P95, P99
and maximum. The latency line times an announcement's first word; the
field's line is timed from the same focus event with `latency.csv` and
`timeline.txt`, in whole milliseconds. Every stage's figures and the
script that derives them are in
`target/e2e-evidence/m42-measure-2026-10-22` (`figures.txt`,
`analyse.py`).

The only UIA text field in the measure set is `mockapp`'s, the first
focus in a newly started application in `measure_focus_new_application`
(30 focuses each):

- First word, event to queued: before 8.9, 12.2, 38.4, 40.3, 40.3;
  after 28.0, 34.2, 44.1, 46.6, 46.6.
- First word, event to audio: before 9.9, 21.5, 53.8, 62.7, 62.7; after
  29.5, 35.6, 67.7, 72.0, 72.0.
- Outpost read: before 0.1, 0.2, 0.3, 0.3, 0.3, with no calls; after
  18.9, 20.7, 25.8, 28.2, 28.2, with 4 calls.
- The field's line, event to queued: before 27, 34, 63, 71, 71; after
  28, 34, 44, 46, 46. It followed the first word by 19, 21, 24, 33, 33
  before, and by 0, 0, 1, 1, 1 after.

The line ends as it did, and its tail is shorter, but the first word
waits about 22 ms longer, not the caret read's millisecond, and that is
a defect against the costing, not this cost's expected figure: the
outpost's debug log (`debuglog-after-new-application`) shows the search
for its own element taking 19.0 to 24.3 ms in each new application, its
first connection to it, and the caret read 0.8 to 1.0 ms. No `measure_`
scenario times a UIA text focus in an application already connected.

`measure_focus_text_box`'s focuses are Windows Forms edit controls read
through MSAA, which the change leaves at 8 or 9 calls; its 20 switches a
run (97 announcements before, 99 after, 100 lines each):

- First word, event to queued: before 13.3, 20.4, 40.6, 49.1, 53.0;
  after 14.1, 20.5, 51.7, 55.9, 56.8. The tail is the wait for the
  reducer, about 20 ms in both builds, and the runs' own P95s overlap,
  24 to 44 before and 37 to 54 after.
- The field's line, event to queued: before 23, 28, 39, 43, 44; after
  21, 29, 42, 44, 46.

Wake-ups and processor time, from `cargo xtask trace scenario
measure_focus_new_application --after 8 --seconds 5`, three runs each,
harness threads left out (`measure_focus_text_box` has no room for a
window): the outposts were woken 10,245 to 11,367 times over 729 to 867
ms on a processor before and 9,845 to 11,675 times over 712 to 860 ms
after, and verbatim.exe 1,344 to 1,510 times over 371 to 420 ms before
and 1,325 to 1,591 times over 369 to 455 ms after. The window is mostly
outposts starting, so it shows no difference beyond the runs' own
spread.

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
  change). Classically 30 (34 before 2026-10-23, when the comparisons
  that place a change were made from a selection that held nothing; 30
  before that; 27 before the new selection's text was read to keep, and
  28 before where it starts was read, 2026-10-22): the caret read's
  calls, and the same comparisons and text read, now only when the
  selection moved. A selection that held nothing before, or holds
  nothing after, is one whole change, as NVDA speaks it, with no
  comparison to place it (two fewer inside the provider remotely). The key compares with the selection the
  outpost kept, as every key does; Core sends no selection of its own
  since 2026-10-22, which changed no count.
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
2026-10-07 (`docs/design/phase6-design.md`, item 14 of the work scheduled that day),
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
  each end), of which the range's read is 4 and the selection's fetch
  the rest; at a focus, whose caret read has already fetched the
  selection, the read costs those 4 alone ("A focus on a text field"
  above).
- Target: 1, met.

### The review copy's selection, UIA

The first press of the review copy (Verbatim+F10, `TextOp::Select`),
answered with how the selection changed so that Core speaks it as a
selecting key's: mockapp's caret collapsed at the start, "alpha" selected
from the marker at the start of the line through the character at the
review cursor, four characters on. Core sends both ends with the points
just past their characters (`TextThrough`), since only the outpost can
tell which end comes first, and the range runs from the earlier through
the character at the later; the program finds the three points after the
start by the text before each.

- Minimum: 1 UIA call: one program reads the selection, selects
  (`ITextRangeProvider::Select` is an instruction of the remote operations
  bytecode, `TextRangeSelect`), reads the selection again, and compares
  the two and reads the text of each change inside the provider, and the
  selection's text up to 513 units, which the outpost keeps with the
  selection for the next comparison, as a caret report's. It executes 163
  instructions (`instruction_limit.rs`, `range_selections_execute_exactly`;
  165 before a selection from a collapsed caret was one whole change,
  2026-10-23, and 160 before the selection's text was read, 2026-10-22).
- Today: 1 remotely (4 before 2026-10-15: the selection read before by
  two classic calls, the selection as a program of its own, and the read
  after as a caret read's program); classically 29 (33 before
  2026-10-23, when a selection from a collapsed caret was placed by four
  comparisons; 32 before the selection's text was read to keep,
  2026-10-22; 22 with the range from
  the marker to the point past the review cursor's character, 30 before
  that).
- When the program fails after its select, the selection is made: the
  program, whose reads after the select are in a try block, still
  returns the selection before, and the after-read alone is made the
  classic way and compared with it, never the select again. That costs
  the program and then, by the code's count for mockapp's case (not
  measured, since mockapp cannot fail a call after the select), 11
  classic calls: the selection, a comparison and `TextPattern2`'s caret
  (3), four comparisons of the ends, and the change's copy, its two ends
  moved and its text (8); each further change costs 4 more.
- Target: 1, met.

In a Win32 edit control the same request is 6 window messages, as before:
the selection (`EM_GETSEL`), the select (`EM_SETSEL` and
`EM_SCROLLCARET`), the selection again, and the change's text
(`WM_GETTEXTLENGTH` and `WM_GETTEXT`); each is a message of its own, so 6
is also the minimum. Pinned by `crates/mockapp/tests/call_counts.rs`
(`uia_text_requests_cost_exactly`, "UIA review copy select", both ways;
`caret_moves_cost_exactly`, "Edit control review copy select").

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

A word read in a rich edit control from version 2.0 finds the word with
the control's own breaks: two `EM_FINDWORDBREAK` messages, or three when the
first word found ends at or before the offset. Before them it reads the
text's length (`EM_GETTEXTLENGTHEX`, whose structure is written into the
application's memory with `VirtualAllocEx` and `WriteProcessMemory` and
freed with `VirtualFreeEx`) to find no word at the end of the text, as
NVDA does: one message more per word read when the length was not
already read in that request, since the length is kept for the request.

An answer that compares selections, a selecting key's or any key's made
with something selected (Backspace and Delete over a selection included,
since 2026-10-22), also waits once for the `WinEvent` thread to take
every event raised before it (`Nudger::sync`), and then scans the intake
once under its lock for the control's value change, as a caret report
does, so the answer is said in general terms after an edit; it makes no
call to the application. The sync posts one thread message and raises
one sync marker with `NotifyWinEvent`, a `WinEvent` of the outpost's own
that the thread's hook set hears, since a posted message alone can be
handled before an event raised before it has arrived; the thread wakes
once or twice for the two, so the marker costs one `NotifyWinEvent` and
up to one more wake per sync. Measured on the development machine (x64,
release build, 20,000 syncs of an idle event thread), one sync's median
went from about 15 microseconds with the message alone to about 25 to 36
with the marker as well (mean about 16 to about 27 to 33). No hook seen
here asks for a marker's object, so a marker costs the outpost no
`WM_GETOBJECT`: 500 markers raised on a message-only window drew none
with NVDA and Verbatim running, where a hook of the marker's id that asks
for each event's object drew two each. A sync is
made only here, in the caret report below, and in `Outpost::settle`,
which tests call, never once per event. With nothing selected before or
after, it checks nothing. Every event thread's hook set, the focus
listener's included, has the marker hook and its message-only window
from when its hooks are in; the listener never syncs, so its marker
hook stays idle and costs no wake.

### A caret report, Win32 edit control

- Minimum: 5 window messages, as for a caret move, whose comparison is
  local for offsets, and the text of a selection change when there is one.
- Today: 5 with nothing selected before or after; 7 for a selection no
  key made (mockapp's "al" from the caret), the change's text read with
  `WM_GETTEXTLENGTH` and `WM_GETTEXT`, its comparisons of offsets making
  no call. A comparison in general terms, after the control's text
  changed, reads no change's text, only the whole new selection's when
  something is selected, with the same two messages a change's text
  takes, so it adds no message to the comparison it replaces.
- Target: the minimum.

The local work beside the calls: the selection's two offsets kept, and,
for a report that compares a selection, a kept one or a new one that
holds text, the registry looked up, one sync of the `WinEvent` thread
waited for (above) so that it takes the events raised before it first,
and the intake scanned once under its lock for the control's value
change, which
the control may raise while it answers the read; this makes no call to
the application. A report with nothing selected before or after checks
nothing. Each value change of the focused
control locks the edit controls' anchors once to note that its text
changed, and nothing else; a UIA text change takes no lock on the
anchors.

### Typed-character echo

- Minimum: none for the echo itself: the keyboard hook translates the key
  locally (`ToUnicodeEx`, `docs/crates/verbatim-input-windows.md`), and
  nothing about the application is read. The caret event the typing causes
  is reported like any other, a caret report above.
- Today: none, and one caret report for the caret event.
- Target: none.

### A terminal output line

A focused terminal's text changed, and the outpost reads it and gives the
read to its reading rules (`docs/crates/verbatim-outpost.md`,
"Terminals"): one snapshot (`verbatim_uia_rops::terminal_snapshot`), the
text from the screen's top as last read, a few history rows above it, to
the document's end in one `GetText`, with the screen, where the next read
starts, the document's first row, the caret's distance to the end, and the
caret and its line, all in one program. A read the rules ask for again, of
the whole document or from further above, is a snapshot more, and rows
they ask for by their place, to check how far a full history rotated, a
program more ("A terminal's rotation" below). Measured against mockapp's text provider
(`tests/fixtures/terminal.json` with a three-row screen,
`tests/terminal.rs`), whose `set-text` command rewrites the text as a
terminal's buffer changes and whose `screen` command puts only its last
lines on screen.

- Minimum: 1 UIA call, one remote operations program, whatever the size
  of the history and however many lines changed.
- Today: 1 UIA call for every read, the first after the focus included,
  pinned remotely (`new_output_is_said_line_by_line_remote`). Classically,
  with `uia.remote_operations` off or a provider that cannot run
  programs, one call per provider method of the snapshot's classic
  implementation. The screen is read in one call however many lines it
  holds, and a read holds what was written since the last, so the cost
  grows only with the output.
- Target: 1.

A caret key in a terminal is held for a read before it is sent on ("The
caret read before a held key", above), which in a terminal is a snapshot,
so the reading rules compare with the screen just before the key.

A terminal's text change forgets no selection, so it takes no lock on the
anchors for it, and a terminal's caret reports, the one after its focus
aside, keep no selection.

### A terminal's caret

A terminal raises no caret event for every character typed: the console
host raises its caret events on a schedule of its own, so a character
echoed between two of them leaves Core's copy of the caret behind. So each
read of a focused terminal reads the caret and its line too
(`verbatim_uia_rops::SnapshotQuery::caret`), in the same program, and the
outpost reports it with what the read says, stamped as read when the
read began; Windows Terminal's caret events read the terminal itself.

- Minimum: no call more, since it goes in the program the text's read
  already runs.
- Today: no call more remotely.
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
  change that arrives during a read causes one more (`docs/design/phase6-design.md`:
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

### A terminal's rotation

How far a full history rotated between two reads is decided as comparing
the whole buffer with the known history would decide it
(`docs/design/terminal-model.md`, section 4.3, item 5): each candidate
rotation is checked against the history rows the read holds, and one
whose block lies above the read by reading the rows it predicts there by
their place (`verbatim_uia_rops::terminal_rows`), or, when the old
screen's top went above the read too, by reading again from the block's
place; several candidates, or a known history that does not reach the
document's start, read the whole document.

Measured on 2026-10-24 on this machine (x64, release build), 120 by 30,
a full history of 9,001 rows, numbered lines, each rotation written
between two reads:

- Live in the console host, through the outpost's `terminal::read`, the
  time of the whole read and its UIA calls: a first read 0.5 to 1.0
  milliseconds, 1 call; the second read, which completes the known
  history by reading the whole document once, 31 to 38 milliseconds, 2
  calls (the whole document's snapshot alone 25 to 26 milliseconds, 1.1
  million UTF-16 units); nothing new, 0.9 to 1.1 milliseconds, 1 call; a
  rotation of 1 or 3 rows, 1.0 to 1.3 milliseconds, 2 calls (the rows by
  their place 0.19 to 0.23 milliseconds of it); 10 to 100 rows, 1.4 to
  2.0 milliseconds, 2 calls, the read taken again from the block's place;
  1,000 rows, 5.1 to 5.7 milliseconds, 2 calls, the read again holding
  1,004 rows above its start (3.5 milliseconds).
- The same in the simulated terminal, the memory's own time and its
  reads, in both terminals alike: nothing new 0.4 milliseconds, 1
  snapshot; a rotation of 1 or 3 rows 0.09 to 0.14 milliseconds, 1
  snapshot and 1 read of rows by their place; 10 or 100 rows 0.08 to
  0.12 milliseconds and 1,000 rows 0.46 to 0.61, 2 snapshots each.
- The ambiguous case, a full history of one repeated line: every read
  is a whole-document read, nothing new included, since every row of the
  history is a candidate that holds. Live in the console host 31 to 51
  milliseconds a read, 2 calls; in the simulated terminal the memory's
  own time 4.1 to 4.7 milliseconds a read, the whole known history
  compared for the smallest candidate, which holds. The same comes of a
  history whose first row is blank and that holds more than 16 other
  blank rows: each such read, until a rotation moves another row to the
  top, reads the whole document, 33 milliseconds live, since more
  candidates than one read of rows by their place takes need checking.
- A clear and a reprint of 9,000 rows with the first 100 left out, read
  whole after the document shrank: 4.3 to 4.8 milliseconds of the
  memory's time, every row found again by the alignment.

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
the 20 matches, on 2026-10-08). In Windows Terminal the row is sought
without its padding, because its `FindText` was measured to throw on
padding on 2026-10-08. That did not reproduce on 2026-10-24, in the
pinned Windows Terminal 1.24.12741: a row sought with its padding was
found in both terminals. A `FindText` that finds nothing returned a range
rather than null, whose `GetText`, `CompareEndpoints` and
`MoveEndpointByRange` fail, with no access violation. Neither terminal
can search across a line break: the console host's `FindText` returns a
range for such a needle whose text then cannot be read, and Windows
Terminal's matches none (measured 2026-10-08). The redesigned reading
(`docs/design/terminal-model.md`) does not use `FindText`: its read
program keeps a range where the next read starts and never searches.

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

### A terminal's colours

The first read of a focused terminal after each key reads the screen's
colours too, each row's runs of one foreground and background
(`docs/design/terminal-model.md`, section 5.7;
`docs/crates/verbatim-uia-rops.md`, "The colours"). Measured on
2026-10-24 on this machine (x64, debug build), 120 by 30 screens, in the
console host and the pinned Windows Terminal 1.24.12741, each figure the
median of 20 reads with the snapshot's own fields, against the same read
without colours; instructions counted with `verbatim_uia_rops::counting`.

What the providers answer decides the method. A cell's colour attribute
is its own; a stretch answers its first cell's colour when every cell but
its last has it, and "mixed" otherwise (neither provider compares the
last cell); the format unit stops only at rows; and Windows Terminal does
not implement `FindAttribute`. So a check of a stretch reads it and its
last cell, four attribute reads.

The candidates, on the worst case, every cell's colours differing from
its neighbours', with reverse video on every other cell and true colour
throughout (3,600 runs):

- Every cell read separately in a program (a cell a `Move` and two
  attribute reads, five instructions): 18,000 or so instructions in all,
  four programs of under 4,900, 55 to 70 milliseconds in the console host
  and 23 to 36 in Windows Terminal, the same for any screen, a plain one
  included. Classically, cell by cell as the conformance runner reads:
  0.9 seconds and 1.9.
- Chunks of 8, 16, 30 or 120 cells each checked and read cell by cell
  when of several colours, without a check of the row first: 19,000 to
  27,000 instructions in the worst case, which costs about what every cell
  read separately does; on Pi's colourful screen 7.6 to 17 milliseconds in
  the console host and 12 to 15 in Windows Terminal, the 16- and 30-cell
  chunks fastest, every plain row paying a check a chunk.
- Halving a stretch of several colours down to its cells: in the worst
  case about twice as many checks as cells, each four attribute reads, so
  more than every cell read separately; rejected without building it.
- What is built: each row checked whole, a row of several colours in
  chunks of 16 cells, and a chunk of several cell by cell. A plain row
  costs one check, and a row with a few colours a check a chunk and a
  walk of the chunks its colours change in.

What is built costs, beyond the read without colours, on each screen as
a stream of the corpus leaves it:

- A plain shell screen (bash's, 30 rows, 30 runs): 1,349 instructions in
  the snapshot's own program; 2.6 milliseconds more in the console host
  (2.9 against 0.35) and 1.6 in Windows Terminal (2.1 against 0.48).
- PSReadLine's coloured command line (2 rows, 4 runs): 555 instructions;
  1.9 milliseconds more in the console host and 1.2 in Windows Terminal.
- Claude Code's screen (8 rows, 8 runs): 425 instructions; 1.0
  milliseconds more in the console host and 0.7 in Windows Terminal.
- Pi's colourful screen (30 rows and 39 runs in the console host, 23 rows
  and 32 runs in Windows Terminal): two programs, 3,451 and 457
  instructions, and 3,451 and 163; 8.0 milliseconds more in the console
  host (8.3 against 0.35) and 4.9 in Windows Terminal (5.4 against 0.46).
- The worst case above: six programs, the snapshot's of 4,151
  instructions and five of 4,099 or fewer (24,644 in all), 72
  milliseconds in the console host and 31 in Windows Terminal, about what
  reading every cell separately costs.

So the colours cost a key 1 to 3 milliseconds on an ordinary screen and up
to 8 on a colourful one, on the first read after it only, read after the
key has reached the program; section 10 of the design estimated 2 to 4
for a colourful screen. Each check and cell is a provider call inside the
program, about 2 to 6 microseconds each, the console host's the slower;
no wake-up is added, the reads being in the read the key already causes,
and a screen of many colours adds only round trips, one a program.

## Newer UIA features

What adopting the UIA features newer than Verbatim's first UIA code
changed (`docs/design/phase6-design.md`, item 13 of the work scheduled on
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
where they were three of each. A registration runs on the process's
registering thread, so it makes no counted call on the worker's; what it costs the application
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
the focus reported (`docs/parity/backends.md`, "UIA event registration"). The
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

### A notification from an application with no outpost

Core speaks a UIA notification only from the focus's application, the
shell's window-snap results excepted, and an application with no outpost
cannot hold the focus.

- Before (2026-10-10): the notification started an outpost, a process
  with its hooks, its UIA client and subscriptions, about 40 ms in a debug
  build before its first event, and Core then dropped the notification.
- Today: the supervisor starts none for it (`DeliveredFact::may_start_outpost`);
  the snap results still start one.

### NVDA's cache for the focus listener's events

Since 2026-10-12 the focus listener's desktop-wide focus registration and
its event handler group (an element selected, a menu opened, a
notification) carry NVDA's base cache request, thirteen properties and
the Text pattern (now the `verbatim_uia::ListenerEvent` shape's request,
with two properties more), where they
carried the base cache request's 34: UIA fills the request inside each
application for every focus, selection, menu, and notification event on
the desktop. The reading thread reads what it needs beyond it from the
sender, as it already did, and an outpost reads a selected item's states,
value, and details from its focus's subtree, one search, when the
listener's selection reaches it. No pinned count in `call_counts.rs`
changed: its test client then stood in for the listener's registration
with the base cache request. Since the UIA shapes work (2026-10-13) the
test client fills a focus's element with the listener's own request,
`ListenerEvent`, as the registration does.

### Smaller caches for the focus's events

The outpost's two focus-following registrations asked the provider for
the base cache request's 34 properties with every event, a text focus's
caret moves and text changes included, from which nothing is spoken: the
worker reads the text itself. The text registration now asks for 3
(the `TextEvent` shape) and the property registration for 24
(`PropertyEvent`), leaving out only what feeds a node's details
or its standing as focus context, since a change of states carries the
whole state set. This saves work in the application on every keystroke
and caret move, not round trips in Verbatim. Measured on demand, as UIA
fills each request (`uia_event_caches_cost_exactly`, which measured the
base request too until the request was removed in step 5 of
`docs/design/uia-shapes.md`): the base request cost mockapp 22 `GetPropertyValue`, 11 `GetPatternProvider`, and 1
`BoundingRectangle`; the text events' 3 `GetPropertyValue`, and the
property changes' 13 `GetPropertyValue` and the 11 `GetPatternProvider`;
and the listener's desktop-wide events (`ListenerEvent`), measured in
the same test, 15 `GetPropertyValue` and 3 `GetPatternProvider`;
each also costs the same 5 `ProviderOptions`, 3
`HostRawElementProvider`, 3 `FragmentRoot`, and 2 `GetRuntimeId`. On
demand because the calls UIA makes to route an event to its registration
vary from one delivery to another with how busy the machine is, while a
request's do not. A property change delivered to the outpost cost 9
`GetPropertyValue` and 1 `BoundingRectangle` fewer, measured while a
focus without its element was followed in its whole window, a
subscription removed on 2026-10-11 (`docs/design/focus-pipeline.md`,
section 17).

The two rarer property changes of the focus read what they need when
the outpost handles them, as NVDA reads them, not in every property
change's request: a help text change refreshes the element's full
description and help text, one call, and a `ControllerFor` change its
text pattern availability, automation id and the relation itself, which
the refresh reads live, one call
(`a_focus_s_help_text_and_suggestions_are_followed`). The first
`ControllerFor` change after Enter in Settings' search box waits 138 to
164 ms in that call, which Settings answers only once it has laid out
the page Enter opened
(`target/e2e-evidence/controllerfor-2026-10-21/`).

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
changed (`docs/design/phase6-design.md`, item 14 of the work scheduled on 2026-10-07):
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
  on both paths (`docs/parity/speech-audio.md`, "Text range moves"), at no call and,
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
(`docs/design/phase6-design.md`, item 11 of the work scheduled on 2026-10-07). Measured
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

- The focus ancestry: 2,366 (24 percent) for a list sixty groups deep,
  every ancestor up to the window returned under the outpost's depth limit
  of 64, against 64 known ancestors none of which it meets, with the
  list's selected item and a held element's focus read; 1,779 for the same
  stopped at a depth limit of 30, walking on to the window without
  returning the rest; 1,044 for the list whose group is known, its window
  still sixty-one levels up. It was 2,180, 1,686 and 1,038 before each
  ancestor's record of the properties left out of its cache was returned
  with it, three instructions an ancestor (`docs/design/uia-shapes.md`,
  step 3). The focus listener's read, which also fills the focus's own
  cache afresh, executes 2,379 at the worst case. A deeper tree adds a level of the window walk
  for each level beyond the depth limit, a dozen or so instructions (the
  navigation step's walk, much the same, takes 12): about two hundred more
  levels before half the limit. mockapp's fixtures cannot nest
  deeper than about sixty levels (its JSON parser's recursion limit).
- The navigation step: 870 (9 percent) from an item sixty-two levels
  below its window, 139 from right under it, about 12 for each level
  walked.
- The controlled selection: 174 for a result inside a group of the
  controlled list, 206 for the list's second child, and 229 for an item
  outside the list, all three of the list's elements walked and then the
  list's selection read, which is empty; about 35 for each element
  passed. Of these, 6 test whether the search found the element. An
  element not found takes 11 more when the first controlled element has
  nothing selected, to read its runtime id and current selection, plus
  the fill of its first selected element's cache when it has one, which
  the 229 does not include; a selection the outpost reads as the focus's
  own reads no current selection, and takes none of the 11. Besides, 5 return the runtime ids of the
  elements the relation names, for the outpost's log of a selection
  found in none: 1 for the string that collects them and 4 for each
  controlled element (its runtime id read, written as a string, joined
  on, and stored), so each further controlled element adds 4. About 140
  elements passed fit under half the limit and about 280 under the
  limit, past which the call is answered classically.
  The walk is bounded by the controlled list, not by the query, the one
  program whose worst case is the application's to set; the Settings
  app's search suggestions, checked live on 2026-10-03, were 25.
- The caret read: 4,462 (45 percent) for a line of 64 stretches reached by
  walking one mixed format stretch by words and a mixed word by
  characters, every attribute read (eleven since 2026-10-07, ten of them
  being learned), with a word, the evidence, a selection's change, the
  selection's text, read when its ends moved, and what the old
  selection's ends hold now, each up to 513 units, and where the
  selection starts;
  97 for the report after a focus on a line of plain text with the
  default theme's two attributes, the annotation types and the link,
  whose support is known, which asks for the selection's text. They were
  4,425 and 84 before the selection's texts were read, 4,434 before the
  selection's text was read only when its ends moved, and 4,435 and 87
  before where the selection starts was read, whose conditions every
  read with `TextPattern2` runs (2026-10-22), and 4,456 before a
  selection made or removed around a collapsed caret was read as one
  whole change (2026-10-23). It was 4,884 and 85 with seven attributes and
  one: the program now returns each value as the provider gave it, and
  the caller reads it by its type, where the program tested each value's
  type and the annotation types' array itself, which more than paid for
  the four attributes added and the line's own reads. Where every format
  stretch is mixed (mockapp's `mixed.json`: stretches of two characters,
  one of them italic), each is walked by words and its word by
  characters, and each stretch adds 249 instructions (239 with seven
  attributes): 2,158 for 17 stretches and 4,150 for 33 (2,148 and 4,140
  before where a selection starts was read, 2026-10-22), so the 64
  stretches a span may have come to about 8,000 (80 percent), which runs
  under the limit but cannot be counted. This is the one
  program that can come within a factor of two of the limit. A run that
  exceeded it would be answered classically for that call
  (`Path::Fallback`), at a cost of hundreds of calls; programs are not
  made resumable (Dickson, 2026-10-07), and none is needed while the 64
  stretches bound the walk.
- A terminal's snapshot (`terminal_snapshot`): 50 for a first read, and 90
  with a start and the caret and its line read in the same program, each
  taking four rows above its start (the history's block); without rows
  above the start, 5 fewer. Without colours the program has no loop, so
  the count does not grow with the history, the rows above or the lines
  read.
- A terminal's rows by their place (`terminal_rows`, "A terminal's
  rotation" below): 172 (2 percent) at its largest input, 16 runs each
  reaching its rows, and 22 for one run: ten a run and twelve besides. The
  program has no loop, so the count does not grow with how far up a run
  reads or how many rows it holds.
- A terminal's colours, read with its snapshot on a 120 by 30 screen
  ("A terminal's colours" above): 1,349 for a plain screen, in the
  snapshot's own program; in the worst case, every cell's colours
  differing from its neighbours', six programs of 4,151 (42 percent), the
  snapshot's, and 4,099, 4,099, 4,099, 4,099 and 4,097, five rows each.
  Each program reads rows while a worst-case row (887 instructions at 120
  cells, about 7.4 a cell) still fits its budget of 4,600, so at widths up
  to about 600 cells none comes near half the limit, whatever the screen
  holds; a row wider than about 300 cells is read alone, and one wider
  than about 1,300 cells exceeds the limit and is answered classically.
- Say-all's batch of twenty lines: 683 (7 percent) a line on from a held
  position with its lines in three languages, each line's then read; 704
  for a first batch from the caret in one language. The count does not
  grow with the lines' length.
- A terminal's screen, with the caret and its line read in the same
  program as the outpost reads a focused terminal: 441 (4 percent) when
  its anchor is nowhere and the search checks its 20 matches
  (`SEARCH_MATCHES`), about 15 each, before the text's rows are counted;
  166 for an anchor found by its text one row up under new output (the
  tail read it replaced took 1,164 and 137, its search checking 64
  matches). Nine of each are the visible range read again at the end, to
  find a view the terminal moved while it was read (2026-10-09). The count
  does not grow with the history or the lines read, except when output
  grows the text while the program walks it: the shift is then found by
  halving the rows it may lie within, about 17 instructions a step, 14
  steps for a growth of 9,000 rows (2026-10-09). Both were one more until
  2026-10-13, when the caret's read stopped getting the text pattern from
  the element again and uses the one the program already has.

## Memory use

Measured on 2026-10-24 on the development machine, a QEMU virtual
machine with 12 virtual processors of an AMD Ryzen 7 5700G and 16 GB of
memory, running Windows 11 Pro build 26200, with a release build of
commit 975fb269 and its default settings, speaking through eSpeak NG in
its synthesizer host. It is the baseline for the terminal memory of
`docs/design/terminal-model.md`, section 4.1, which the terminal outposts
do not keep yet. A script sampled every process run from the build's
folder four times a second with `Get-Process`: private bytes (the
process's commit), working set, and the peak working set and peak commit
Windows keeps for the process's lifetime. Steady is a workload's median,
and peak its highest sample, or the lifetime peak where that is higher.
Figures are in megabytes (MB, 2 to the power 20 bytes).

Private bytes is the figure to compare. Every outpost maps the same
images, so most of an outpost's working set is shared pages that cost
the machine once; a first run while another build's tests ran raised
every outpost's working set by 6 to 8 MB and left its private bytes
unchanged.

The workloads, in the order run:

- Verbatim idle at its own Settings dialog, for a minute.
- The console host focused, 120 columns by 30 rows with a 9,001-row
  buffer, with a small history of 20 lines, then after 9,100 lines of
  120 columns filled its history.
- Windows Terminal (the end-to-end suite's portable copy, with its default
  history of 9,001 rows) opened beside it, 120 by 30, focused, with the
  same small history and then a full one.
- With both terminals open: 10,000 lines of 120 columns printed in
  Windows Terminal (1.7 seconds), then 30 seconds after; the console host
  focused again, then the same flood printed in it (3.3 seconds), then 30
  seconds after. "Report new output" was on, so each flood was spoken.
- Both terminals closed, and 90 seconds with the focus on another Windows
  Terminal window.

The processes and their figures:

- `verbatim.exe`, the core process: 7.5 MB private and a 33.4 MB working
  set idle at its own dialog, growing slowly to 8.2 MB and 32.5 MB after
  the terminals were left; its peak commit was 8.3 MB and its peak
  working set 33.5 MB, reached at startup and at the dialog. No terminal
  workload moved it by more than 0.3 MB.
- The focus listener (`verbatim-outpost.exe --listener`): 3.6 MB private
  and a 19.8 MB working set idle, 3.8 MB with the console host, and 4.0 to
  4.1 MB from Windows Terminal on; peaks 4.1 MB and 20.7 MB.
- The outpost for Verbatim's own window: 2.2 MB private and 13.1 MB
  working set throughout; peaks 2.4 MB commit and 13.3 MB.
- The console host's outpost: 3.4 MB private and 18.9 MB working set with
  the small history, 3.5 to 3.6 MB and 19.0 MB with the full history, and
  the same 3.6 MB and 19.3 to 19.5 MB in the background while Windows
  Terminal was used and during and after its own flood; peaks 3.7 MB
  commit and 19.5 MB working set. It ended with its terminal.
- Windows Terminal's outpost: 3.0 MB private and 17.6 MB working set with
  the small history, 3.0 to 3.2 MB and 17.7 MB with the full history, and
  3.0 to 3.1 MB and 17.8 to 18.0 MB during and after its flood and in the
  background; peaks 3.3 MB commit and 18.2 MB working set. It ended with
  its terminal.
- The outpost for the other Windows Terminal window, focused after
  leaving: 3.2 MB private and 18.1 MB working set; peaks 3.4 MB and
  18.3 MB. An outpost for that window was also running at the start, at
  3.3 MB and 19.3 MB, and was retired while the test terminals were used.
- Spare outposts, started ahead of need, two at a time: 1.9 to 2.1 MB
  private and 12.3 to 12.9 MB working set each, with no peak above
  2.1 MB and 12.9 MB. Three of the five became the outposts above.
- The synthesizer host (`verbatim-synth-host.exe`, eSpeak NG): 2.5 MB
  private and 9.3 to 9.4 MB working set in every workload, both floods'
  speech included; peaks 2.6 MB and 9.4 MB.

In total, Verbatim's processes held about 23 MB of private bytes idle at
its own dialog, about 28 MB with both terminals open, and about 24 MB
after leaving them. The terminals cost little: neither a full history nor
a flood moved a terminal's outpost by more than 0.3 MB, since today's
outpost reads only the screen and the rows written since its last read,
keeps nothing of the history, and frees each read once it is reported.

### The terminal memory's cost, estimated

The terminal memory of section 4.1 (`TerminalMemory` in
`crates/verbatim-text/src/terminal_rows.rs`) and the reading rules'
per-row state beside it (`TerminalReader` in `terminal_reading.rs`) will
be kept by the outpost of the terminal's application, where the reads are
made (`docs/design/terminal-model.md`, section 12), one of each for every
terminal. Windows Terminal's tabs share one process, so its outpost will
hold one memory for each tab, under the outpost's memory cap of 200 MB.

The sizes, on x64, for a row of 120 columns: a row's text is one shared
string of 136 bytes of heap (the 120 bytes and two counts), which the
memory's row, the line it makes up when the line is one row, and the
reading rules' state of that line all point to. The memory's `Row` is 48
bytes, so with its text it takes about 185 bytes, and one more byte
records whether the row was read since a switch of screens. The reader's
`RowState` is 56 bytes, the `LineState` of the line the row begins 144
bytes more, and each row's line start 8 bytes; what was last said of a
line is the same string as its text until the two differ. That is about
210 bytes of the reader's for a row that begins a line, and about 400
bytes a row in all. Colours add 24 bytes a run, only for the screen's rows
and only after a key.

For a 120 by 30 screen with a full history of 9,001 rows:

- The last read's rows, with their text and history: the screen and the
  rotation block above it (4 to 30 rows), so 34 to 60 rows, 14 to 27 KB.
- Lingering rows: at most the rows a read found gone, for 2 seconds,
  each with its row (64 bytes and its text) and the reader's gone line
  (24 bytes and its 144-byte state), about 370 bytes a row: under 12 KB
  for a screen's worth.
- The known history: one 8-byte hash for every row the buffer holds
  above the rows kept, since a rotation takes the hashes of the rows that
  left and a read from the document's start replaces them (section 4.1):
  at most about 9,000 hashes, 72 KB, up to as much again with the queue's
  unused capacity. Measured after 100,000 lines, 8,997 hashes.
- Anchors: the review position and the command marks, 24 bytes each.

Once a read is handled, the memory and the reading rules keep of it only
the screen, the rows written below it and the block above it, the rows
above the block as hashes in the known history (`TerminalMemory::trim`),
so a read of the whole document (section 4.2: more than a screen went by
between reads, as in every flood and in filling the history) leaves the
same memory as an ordinary read. Measured on 2026-10-24 with a counting
allocator in a release build, 120 by 30 with a full history of 9,001
rows, after 100,000 lines read 20 at a time and then a flood read whole,
a terminal's memory and reading rules hold 125 KB after the ordinary
reads, 122 KB after the whole-document read and 127 KB after one more
ordinary read, so a terminal's memory in its steady state is about
0.1 MB in the terminal's outpost, a few percent of today's outpost,
however long it ran and however it was flooded. While a whole-document read is handled, the
reading rules' peak rises by 4.5 MB: the read's rows (9,001 of about 185
bytes, 1.7 MB), its lines (56 bytes each, a one-row line sharing its
row's text), the outcome, and a 56-byte state for each row, with no line
state for the lines above the rows that will be kept. The outpost also
holds the read's text (1.1 MB as UTF-8, and the 2.2 MB UTF-16 string UI
Automation returned), so its peak rises by about 8 MB for that read, and
falls back once the read is handled. The reading rules take about 35
microseconds for an ordinary read of one line more and about 9
milliseconds for a whole-document read.
