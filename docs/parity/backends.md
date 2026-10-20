# Backends

How the MSAA, UIA, and other accessibility backends are read and their events taken in.

### Dual-stack MSAA+UIA with per-window arbitration

NVDA: the `isUIAWindow` referee with good/bad class lists, its verdict
cached for 500 ms, and a probe that times out taken as "no provider"
([The UIA client](../nvda/uia.md), "The per-window UIA decision").
Verbatim: **partial**. Matched, in NVDA's order: the class name is
normalized as NVDA normalizes it (its class map, and the Windows Forms
and `ATL:` wrappers removed, so a Delphi `TEdit` or a Windows Forms edit
counts as an `Edit`); NVDA's good classes are UIA; so is a Windows 11
shell window, recognized by its root ancestor's class, except the Start
button; NVDA's bad classes are MSAA; otherwise the window is probed. A
window with a provider is still read through MSAA when NVDA would not
use the provider: a console whose text does not report formatting
(older consoles; the Windows 11 console reports it and stays UIA,
checked live), and a list view outside Windows Forms (a Windows Forms
list view was recognized live). A probe that finds a UIA provider is
kept for the window's lifetime, one that finds none for 500 ms, NVDA's
cache period; no window's answer was seen to change from a provider to
none during its life, across about 65 windows of Explorer, Settings,
Start, and the desktop. The window is asked once, and a "no" given because
the window did not answer the probe within its 3 seconds is kept like any
other, as NVDA keeps the "no" it is given (`docs/design/focus-pipeline.md`, section 8).

Different: the focus listener and the outposts ask the same question
with one raw `WM_GETOBJECT` for `UiaRootObjectId`, counting only the
answer `0x10000` as a provider and waiting at most 3 seconds, rather than
with `UiaHasServerSideProvider`, which connects to the window and
answered "no" falsely beside another process's queued remote operation
(`e34`), under `e38`'s trigger, and for the outposts on new console and
File Explorer windows
(`docs/design/listener-without-connections.md`, decision 2; the
outposts' probe decided by Dickson on 2026-10-20). An outpost whose probe
finds a provider then connects to the window once, which NVDA's probe
does as part of it, so a new File Explorer window gets the same kind of
call at its first sight as NVDA makes; whether it comes as early as
NVDA's is not measured. A list view whose provider the Windows Forms
check excludes, from its class name alone, is not connected to.

Not yet (M7): NVDA's Word, Excel, and Chromium exceptions, which set UIA
aside only when NVDA has injected its in-process helper; without it NVDA
uses UIA for them, as Verbatim does, and they arrive with Verbatim's
helper (decision D2). Not carried: the Office 2013 and older ribbon rule
(`NetUIHWND`), for Office versions Microsoft no longer supports, and the
rule against NVDA's own process, which guards against a freeze of UIA
inside the screen reader's process that outposts cannot have.
Application modules' own good and bad windows wait for extensions,
except the Explorer shell rule above, which is core policy.

Different (`docs/design/listener-without-connections.md`, section 5):
Verbatim's UIA connection timeout is UIA's default of 2 seconds, as
NVDA's is, but its transaction timeout is 5 seconds, where NVDA leaves
UIA's default of 20.

### How an outpost turns events into focus reports

NVDA: [The UIA client](../nvda/uia.md), "Caching discipline", and
[MSAA and winevent handling](../nvda/msaa.md). Verbatim: **matched
(unverified)**, case by case:

- A UIA focus is built from the event: NVDA builds the focus object
  from the event's sender. The sender's cache holds only NVDA's base
  cache properties (`baseCachePropertyIDs` in its UIA handler: name,
  control type, class, automation id, framework, process,
  `HasKeyboardFocus`, and a few more), so its name and role come from
  the event. Its states, value, description, shortcut, position, level,
  and bounds are not in that cache: the focus object's `event_gainFocus`
  fetches them in one `buildUpdatedCache` (`_focusPrefetchUIAPropertyIDs`
  and `_UIAStatesPropertyIDs`) when NVDA handles the focus, and they are
  spoken from that fetch. So going back to a File Explorer folder, NVDA
  says "Inner 1 of 4": Explorer raises the focus event before it selects
  the item, and selects it before the focus is handled. The focus
  listener judges each UIA focus as NVDA does: on its thread for the
  application, a few milliseconds after the event, it reads the event's
  own element's `HasKeyboardFocus` live and drops the focus when it is
  false, as NVDA reads `currentHasKeyboardFocus` in
  `shouldAllowUIAFocusEvent`, then reads the element's states, value,
  and details afresh with one cache-building call, and its ancestors one
  step up at a time, every call on the element it holds and none a
  remote operation (`docs/design/focus-pipeline.md`, section 12;
  `docs/design/listener-without-connections.md`, sections 2 and 3).
  Pinned by mockapp's
  `a_focus_reports_the_states_read_when_it_is_handled` and Core's
  `a_focus_read_after_its_item_was_selected_is_announced_without_the_selection`.
  A focus reported without its element, which was not found in time,
  keeps the event's values. The outpost reports every focus the listener
  read, whatever has happened since, as NVDA queues every focus it
  accepts, and Core culls the speech of one no longer current. When the
  listener's read fails (its application does not answer within the
  call timeout), the focus is reported from its event with its ancestors
  unknown, as NVDA lets a focus through whose `HasKeyboardFocus` read
  fails, whatever the event's cached `HasKeyboardFocus` says: the
  keyboard focus check before the read has already judged the focus, and
  a sender whose live read said it had the focus is spoken by NVDA
  whatever its cache said (mockapp's
  `a_focus_whose_read_fails_is_reported_whatever_its_event_says_of_its_focus`),
  always as the focus, with the focused state, as every focus the
  listener reads is reported.
  When the read's time budget has passed before the keyboard focus
  is read, the event's cached `HasKeyboardFocus` stands in for NVDA's
  live read, and a focus whose event does not say it had the focus is
  dropped (mockapp's
  `a_focus_out_of_time_whose_event_says_it_lacked_the_keyboard_focus_is_dropped`);
  a live read that fails, by UIA's transaction timeout or otherwise, lets
  the focus through, as NVDA's does (Dickson, 2026-10-20; mockapp's
  `a_focus_whose_keyboard_focus_read_times_out_is_reported`). A provider that answers the property empty, or whose answer
  fails, leaves UIA's default, `false`, in the event's cache, the same
  for both (mockapp's events test
  `uia_a_focus_events_cache_holds_a_failed_keyboard_focus_as_an_empty_one`),
  so such a focus is dropped from the cache as NVDA drops it when its live
  read of such a provider gets the same default `false`
  (`verbatim-uia`'s
  `a_focus_event_lacks_the_keyboard_focus_unless_its_cache_holds_true`).
- The console host's window element never gives the focus, whatever it
  reports, as NVDA's `consoleUIAWindow` refuses its focus events
  (`NVDAObjects/UIA/winConsoleUIA.py` 356 to 358 and 446 to 447): one
  rule, applied by the focus listener before any read.
- Different, because of the outposts (D9, D13): the event's element is
  in the listener's process and cannot cross to the outpost (experiment
  E-marshal). The listener reads the focus's ancestors from it, and the
  outpost, after reporting the focus, finds its own copy by the runtime
  ids of the chain the listener read, from the nearest window, for its
  subscriptions, text, and navigation. An element that path does not
  reach, such as Windows Terminal's tab focus elements, which are in no
  tree, is reported from the listener's read with its own ancestors; its
  changes are followed from its window until its own event brings the
  element, and review and navigation start from the nearest ancestor
  found (`docs/design/focus-pipeline.md`, decision 3 of section 15). NVDA
  has the element from the event and never looks for it.
- A focus's ancestors are read only until they meet the previous
  focus's chain, whose rest is reused, as NVDA's focus ancestry does.
  Different: reading the rest is limited to two seconds, after which
  the ancestors are reported unknown rather than the focus being held
  back; NVDA waits.
- An MSAA focus is accepted only when the object or one of its
  ancestors has the focused state (`shouldAllowIAccessibleFocusEvent`);
  the states are checked first and read live, as NVDA reads them,
  ancestors included. A focus event naming the address of the focus
  last reported, a whole object, is dropped before it is read, as NVDA's
  `isDuplicateIAccessibleEvent` drops it, unless a foreground change the
  outpost reported, or a focus Core queued from another application,
  came after it, as NVDA compares with the last focus queued from any
  application (`docs/parity/focus-events.md`, "Duplicate focus
  suppression"). `msaa_focus_changes_cost_exactly` pins both.
- Each consecutive run of a batch's MSAA focus events and foreground
  changes is tried newest first until one is reported, as NVDA's event
  pump tries a run of focus and foreground events: when the newest cannot
  be reported (unreadable, destroyed, refused, or a foreground change
  whose window is not the foreground window, or that NVDA's tests against
  the last focus queued drop), the next older one is tried, through every
  event the limiter kept: its four focus events, and a foreground change
  kept as one of its thread's other events besides. A run is ended only
  by an MSAA event NVDA's pump receives; UIA events and focus reads,
  Core's queries and list of held nodes, the worker's caret follow-up
  and the outpost's settle request keep their own places without ending
  it, with the differences "MSAA winevent flood control" records.
- The fake focus after a menu or the Alt+Tab switcher closes is skipped
  only when a focus observed after the close was actually applied; a focus fact that
  is dropped or unreadable does not skip it.
- NVDA's early `WinEvent` filters: object ids at or below `OBJID_ALERT`,
  a focus on a menu bar object itself, foreground events for Program
  Manager and the taskbar, and menu events from the IME candidate
  window are ignored. Selection add, remove, and within events are
  changes of state, not new selections, so a deselected item is not
  announced as selected.
- Events are judged by their own application's window, never the
  system's focus window, which can belong to another application; UIA
  notifications are not arbitrated, as NVDA does not arbitrate them.
- A provider probe stays within the outpost's deadline: the raw probe
  gives up on a window that does not answer within 3 seconds and reports
  no provider, which is kept for 500 ms, as NVDA keeps UIA's "no" after
  its own wait.

### UIA caching discipline

NVDA: the `baseCacheRequest` pattern ([The UIA client](../nvda/uia.md),
"Caching discipline"). Verbatim: **matched (unverified)**: cached
elements and scoped search. An audit against NVDA's per-event cache
contents is still open.

### UIA event registration

NVDA: selective registration (`UIA.eventRegistration`, automatic by
default, which is selective from Windows 11 22H2), with a global and a
local event handler group ([The UIA client](../nvda/uia.md)). Verbatim:
**partial**: it registers a narrower set, with no setting, since
Verbatim's minimum is Windows 11 24H2. Every registration is an event
handler group, registered with one `AddEventHandlerGroup` call per
element; a group that cannot be registered on an element (most often
one that has gone) is logged and skipped, and removing handlers from an
element that died is left to UIA, as NVDA does. Handlers are added and
removed from one thread per process, as Microsoft's guidance for UIA
clients asks
([Understanding Threading Issues](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading)).
- The global group, desktop-wide in the focus listener: an element
  selected, a menu opened, and notifications. NVDA's also takes live
  regions, tool tips, windows opened, system alerts, layout
  invalidation, drag and drop, and range values anywhere (progress
  bars); not yet: Verbatim subscribes to none of these until it handles
  them.
- The local group, on the focus: name, value, range value, toggle,
  enabled, expand and collapse, help text, and ControllerFor changes.
  NVDA registers its local group (those but the range value, which it
  follows desktop-wide, the item status, which only its app modules act
  on, and caret events) on the focused element with
  `TreeScope_Element | TreeScope_Ancestors`, when it handles a UIA
  focus. Verbatim registers on the focus with `TreeScope_Element`
  alone, and the outcome is the same, because UIA delivers no
  ancestor's event to an ancestors-scoped registration. Checked on
  Windows 11 build 26200:
  - Against Windows 11's taskbar, a real provider in Explorer: a
    registration on the clock button's child, scoped to its ancestors
    (with or without the element itself) or to its parent, did not
    hear the clock button's name change each minute; one on the clock
    button itself, and one on the taskbar with its subtree in scope,
    did.
  - Against mockapp: the same, for a group's name change and a
    registration on a button inside it, made both as an event handler
    group and directly. mockapp raises the change
    correctly, through `UiaRaiseAutomationPropertyChangedEvent` on the
    group's own provider (raising without first asking
    `UiaClientsAreListening` is allowed; that call only saves work): a
    registration on the window with its subtree in scope hears it. As
    UIA raised the event, it asked the group for its parent and runtime
    id and went no further, so nothing related the group to the
    registered button.
  NVDA's `event_stateChange` speaks a state change on a focus ancestor
  (its issue 10890), but under selective registration UIA never hands
  it one, so for UIA NVDA too speaks property changes of the focus
  alone. Verbatim's reducer acts only on the focus's own changes
  (`reduce.rs`: a name change, a value change, and a state change each
  return early unless the node is the live focus, and the navigator's
  copy is updated only while it rests on the focus); a change on a
  focus ancestor, such as a window's title changing when a file is
  saved, reaches the reducer only from MSAA, and is dropped there. The
  ancestor state rule is a difference recorded in
  `docs/parity/focus-announcement.md`, "Focus announcement content and
  property order".
- Caret and text changes: on a focus that has text, alone. NVDA takes
  text changes only from Word, the console host, and Windows Terminal
  (when its notifications are off); Verbatim takes them from any text
  focus, for typed text and terminal output.
- NVDA checks that the element is still focused before registering on
  it (comparing it with `GetFocusedElement`, a call). Verbatim does
  not: the registering thread registers only the newest focus a
  registration was handed, and a newer focus replaces it.

### The active text position

The active text position (UIA's active text position changed event,
raised when an application scrolls to a place in a document without
moving the caret, such as an in-page link's target). NVDA: registered
desktop-wide in its global group ([The UIA client](../nvda/uia.md),
"Event registration: global plus focus-local"); the event is handed to
the object, and only a UIA browse mode document acts on it, moving its
caret to the range's line and speaking the line when the caret was not
already on it; an event whose element is gone, or from an application
that is not responding, is dropped, and outside browse mode nothing
happens. Verbatim: **partial**. Matched as far as it goes: the outpost
registers for it on a text focus, in the same event handler group as
its caret and text changes (the range belongs to the application and
cannot travel to another process, so the focus listener cannot carry
it), keeps the range's start as a position in the focus's text, and
sends it to Core (`ActiveTextPositionChanged`), dropping an event whose
element or range cannot be had, or whose window is hung. Core does
nothing with it. Not yet (M7): browse mode, which moves its caret there
and speaks the line, as NVDA's does; it will need the registration on
the document a focus is inside, not only a text focus.

### Text attributes through UIA

NVDA: a range's formatting is fetched in one
`IUIAutomationTextRange3::GetAttributeValues` call where the range has
it, else attribute by attribute, and an attribute whose read fails is
"not supported" (its issue 7124); a mixed value is handled apart
([Document formatting reporting](../nvda/document-formatting.md)).
Verbatim: **matched (unverified)**: on the classic path, the same bulk
read with the same fallback and the same reading of a failed attribute;
the remote program reads each attribute inside the provider, and a
failed one comes back not supported there too. A stretch with a mixed
value is read again by words and then characters, as NVDA reads it, on
both paths.

### MSAA winevent flood control

MSAA winevent flood control (per-thread caps, focus coalescing,
latest-menu-only). NVDA: `OrderedWinEventLimiter`
([MSAA and winevent handling](../nvda/msaa.md)); NVDA's limits of 4 focus
events and 10 events per thread apply to WinEvents only
(`IAccessibleHandler/orderedWinEventLimiter.py`), and its UIA events
are only coalesced, one per element and kind per flush, with no count
limit (`nvdaHelper/local/UIAEventLimiter/rateLimitedEventHandler.cpp`).
Its limiter counts each thread by the thread id the WinEvent carries,
keeps the newest 4 focus and foreground events of a flush, keeps a
foreground event among both the focus events and its thread's other
events, counts the focus's own events but always keeps them, and keeps
only the newest menu event, outside both limits. `pumpAll`
(`IAccessibleHandler/__init__.py`) then walks the flushed events in
order, collecting each consecutive run of focus and foreground events
and processing it newest first, stopping at the first valid one, when
the next other event comes; the menu event is tried last. Verbatim:
**matched (unverified)** (`docs/design/focus-pipeline.md` section 5;
[verbatim-outpost](../crates/verbatim-outpost.md), the queue under
`Outpost`), in each outpost's intake. Every MSAA event is counted by the
thread it carries, whichever hook took it in; every foreground change
of a batch is tried with the focus events of the run it is in, and of
two changes of the same window from the same thread the later raised
is kept, whichever arrived first: a change arriving during a hand-over
after a later-raised change of its window and thread, waiting, planned
or handed out, is dropped against the intake's record of the newest
change per window and thread, written as each change arrives until the
listener's "forwarding ended" reaches the outpost, as NVDA's limiter,
adding events in raised order, keeps the newest per window, object and
thread (Dickson, 2026-10-20); a run whose events all carry the
time Windows raised them is tried in that order, the order NVDA's one
hook receives them in, since an outpost can receive the focus listener's
copy of an event after its own hook's copy of a later one during a
hand-over. Events of one tick of that clock, about 15.6 ms, are ordered
by what the hand-over knows: a listener copy known to have been raised
before the outpost's hook went in, and every listener copy of the batch
that arrived before it, go ahead of the rest, which keep their arrival
order. A copy is known so when it was raised before the clock reading
taken just before the hook went in, or when it has no twin while the
hook has an unmatched copy of an event raised in the same tick, whose
own listener copy would otherwise have arrived first. The two sources
can lose different copies of one event, and the order can then be
wrong for events of that tick. The listener loses its copy, leaving the
hook's unmatched so that a later listener copy of the tick is taken as
raised before the hook, when its backlog replaces a waiting fact with a
newer one of the same element and kind, and of the same raising thread
for an MSAA event other than a menu opening, while a write is in flight, or
when it finds no process to route the event to. A window destroyed
between the two callbacks' reads of its owner loses a copy in either
direction: when the outpost's hook reads first, the listener drops its
copy and the hook keeps its own, with the same effect; when the
listener reads first, it forwards its copy and the hook drops its own,
so the listener copy has no twin, and while the hook holds an unmatched
copy of a later event of the same tick, as it usually does since hook
copies arrive first, it is taken as raised before the hook and goes
ahead of the hook copies of the tick raised before it, with every
listener copy that arrived before it in that tick. The outpost's hook drops an event whose window has no owner, as the
listener does and as NVDA's callback drops an invalid window, so a
window already gone when both callbacks run loses no copy. Each copy is
queued under the lock it is judged under, so the intake receives the
copies in the order they were judged. NVDA's limiter removes a
waiting focus event on a window's client object from the same thread
when it adds the window's foreground event (`orderedWinEventLimiter.py`,
`addEvent`, 65 to 67), and the outpost matches that exactly: a
foreground change removes such a focus waiting from the same thread
raised no later than the change, comparing raise times in either
arrival order. Such a focus that arrives after a foreground change of
its window and thread raised after it, an order NVDA's one hook never
sees, is dropped as it arrives against the same record, whether the
change waits, is planned or was handed out; the one case left is a
listener copy the listener's hook receives only after it read the tick
that message names; when the later change of its window and thread
still waits, such a late copy of an older change replaces it by key and
takes its place at the back, so the older change is handed out instead
and removes waiting client focuses by its own, older raise time. The
window is the same, but a focus raised between the two that waits after
the later change is then the newest of their run and is reported in
place of the window; this needs two late listener copies, the focus's
and the older change's, so it is rare. And the foreground report can
come after the speech of an event that ends a run and arrived after the
later change. NVDA pumps a focus event at once
(`IAccessibleHandler/internalWinEventHandler.py` 193; `core.py`,
`requestPump` and `processRequest`) and then says exactly one of the
two, never both. NVDA says the foreground change only when its pump
filters or refuses the focus, and the drop matches that case:
`eventHandler.shouldAcceptEvent` (`eventHandler.py` 481 to 599, from
`IAccessibleHandler/__init__.py` 1056) accepts the focus, among other
cases, when its window is the foreground window or descends from it, has
the foreground window's root owner, or is topmost or has a topmost root
(544 to 566); the other cases are an event an app module registered
through `requestEvents` (497 to 498), the desktop window (529 to 531), a
`Windows.UI.Core` window that descends from the active window (535 to
538), and a Chromium document under the focused Chrome window (569 to
598). An accepted focus can still be refused, as by
`winEventToNVDAEvent` failing (`IAccessibleHandler/__init__.py` 737 to
739), by the focused-state test (`processFocusNVDAEvent`'s
`shouldAllowIAccessibleFocusEvent`, whose base is at
`NVDAObjects/IAccessible/__init__.py` 874 to 892 and which app modules
override), or by the SDM test (`processFocusWinEvent`,
`IAccessibleHandler/__init__.py` 710 to 720). Otherwise NVDA says the
focus only, as `processForegroundWinEvent`'s descendant and address
tests then drop the change (`IAccessibleHandler/__init__.py` 845 to
867), and here the drop differs, since NVDA ends on the focus's object
and Verbatim on the window's. A dialog the window in front owns, as an
application's own dialog is, falls in this second case, since the
root-owner test accepts its focus before it comes to the front;
`docs/roadmap.md`, M4.2, records the capture to take. The drop also
loses a focused child, only for a window read through MSAA and only in
the two cases `verbatim-ia2`'s `redirect_focus_to_child` reaches: the
window's client is a list or a `SysListView32` window's client, or its
`accFocus` answers a child id whose child has the focused state. A
client focus that would have been sent on to that child is lost with
it, where the foreground report is the window's node only. Within one tick the copy that
arrived first counts as raised first, except that a hook copy was
raised after a listener copy known to have been raised before the hook
went in. The hook takes an event on a window object as the client object, as NVDA's
callback does, so that is the address NVDA's foreground event has.
The limiter so keeps, in one run, the newest 4 focus and foreground
events and every older foreground change of another window, or of the
same window from another thread, that its thread's limit of 10 keeps. The outpost's foreground handling
makes NVDA's two tests against the last focus queued, after its
in-front test and before any read, and a change they drop lets the
run's next older event be tried (`processForegroundWinEvent`, lines 842
to 867): the last focus or foreground change the outpost reported, while
no focus Core queued from another application has replaced it, is in
the change's window or a child window of it (`isDescendantWindow`, which
follows parents only), or came from the change's own event address. The
fact the hook forwards carries the window alone, so the change's object
and child ids are taken as the system raises them, on the window object,
which the hook, as NVDA's event callback, takes as the client object
(`OBJID_CLIENT`, `CHILDID_SELF`). A last focus Core took from another outpost inside the
window, which this outpost cannot see, is left to the reducer, which
ignores a foreground change of the window the focus is already in.
Mockapp's `msaa_events` tests pin the first test and the replacement by
another application's focus, and the worker's unit tests both tests.
Entries NVDA's pump never receives neither join a run nor end it, and
are handled at their own places: UIA events and UIA focus reads, Core's
queries and its list of held nodes, the worker's own caret follow-up
after a focus it reported, the outpost's settle request, an object
destroyed, the console host's events, and a text control's selection
change (Dickson, 2026-10-20). What is heard changes from the time a run
was ended by any entry, in three shapes, none yet captured from NVDA:
a focus fact and a foreground change of its window with Core's list of
held nodes between them, after a focus in another application, now
announce the window, as NVDA does, where the focus was reported first;
two focuses of one window with a UIA event or Core's entry between them
now report only the newer, as NVDA's one pump does; and when the focus
moves twice quickly, as [focus A, the caret follow-up for the focus N
reported before, focus B], N's caret is now read and its line or value
spoken before B alone is reported, where A was reported and the
follow-up found N gone. Each is closer to NVDA's one pump; the third
puts a read of N's caret on the way to B's report, and awaits a capture
of rapid tabbing out of an edit field.
Different: events from a window the system reports hung are dropped
before any read; an MSAA event the outpost's acceptance test will drop
ends a run, where NVDA's pump filters it out before its run, since the
test is made against the foreground window when the event is handled;
a menu end and a switch end, which end NVDA's run, do not end one in
an outpost, since the focus listener takes them; and events NVDA's pump
receives that never reach the intake end no run there: MSAA change
events from a window known to be read through UIA, which the outpost
drops at its hook or does not hook (`Outpost::take_in_win_event`), where
NVDA drops them only after its pump's acceptance test
(`IAccessibleHandler/__init__.py` 577, after 1056), and the events the
outpost never subscribes to, which NVDA hooks
(`internalWinEventHandler.py` 34 to 63): the IA2 caret, document-load,
attribute and page events, live region changes, scrolling start, desktop
switch, caret show and hide, and the shows NVDA accepts besides
tooltips. For an application with an MSAA tree beside a UIA item view,
[focus A in the tree, a name change in the item view, focus B in the
tree] is heard "A, B" in NVDA, whose pump the name change ends a run
in, and "B" in Verbatim. Before an
outpost hears them, the focus
listener coalesces its facts by the same one-per-object-and-kind rule,
and the supervisor holds the facts that arrive while an outpost starts
by that rule too, and lets a newer fact replace a waiting one when an
outpost's queue is full.

### IA2 upgrade of MSAA objects

NVDA: `normalizeIAccessible`
([IA2 usage](../nvda/ia2.md)). Verbatim: **partial**: the verbatim-ia2
QueryService path exists ([verbatim-ia2](../crates/verbatim-ia2.md)),
unverified; IA2 text and hypertext consumption is not yet (M7).

### JAB

Verbatim: **not yet (M13)**.

### Remoted UIA trees

Remoted UIA trees (Application Guard-style: valid UIA, locally
invalid window handles and process identity). NVDA: the WDAG
pathway ([The UIA client](../nvda/uia.md)). Verbatim: **not planned**
— MDAG is deprecated and removed in Windows 11 24H2; noted because
Verbatim's window-handle assumptions (arbitration, hierarchy
navigation) would need a treat-as-remote pathway if a successor
technology with the same shape appears.
