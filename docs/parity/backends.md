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
button; NVDA's bad classes are MSAA; otherwise the window is probed, as
"The UIA provider probe" describes. A window with a provider is still
read through MSAA when NVDA would not use the provider: a console whose
text does not report formatting (older consoles; the Windows 11 console
reports it and stays UIA, checked live), and a list view outside
Windows Forms (a Windows Forms list view was recognized live). A probe
that finds a UIA provider is kept for the window's lifetime, one that
finds none for 500 ms, NVDA's cache period; no window's answer was seen
to change from a provider to none during its life, across about 65
windows of Explorer, Settings, Start, and the desktop.

Not yet (M7): NVDA's Word, Excel, and Chromium exceptions, which set UIA
aside only when NVDA has injected its in-process helper; without it NVDA
uses UIA for them, as Verbatim does, and they arrive with Verbatim's
helper (decision D2). Not carried: the Office 2013 and older ribbon rule
(`NetUIHWND`), for Office versions Microsoft no longer supports, and the
rule against NVDA's own process, which guards against a freeze of UIA
inside the screen reader's process that outposts cannot have.
Application modules' own good and bad windows wait for extensions,
except the Explorer shell rule above, which is core policy.

### The UIA provider probe

NVDA asks `UiaHasServerSideProvider`, which connects to the window, and
times its cached verdict from before the probe, so a "no" that took the
whole wait is out of date when stored ([The UIA client](../nvda/uia.md),
"The per-window UIA decision").

Verbatim: **different** (`docs/design/listener-without-connections.md`,
decision 2; the outposts' probe decided by Dickson on 2026-10-20): the
focus listener and the outposts ask the same question with one raw
`WM_GETOBJECT` for `UiaRootObjectId`, counting only the answer `0x10000`
as a provider and waiting at most 3 seconds, rather than with
`UiaHasServerSideProvider`, which answered "no" falsely beside another
process's queued remote operation (`e34`), under `e38`'s trigger, and
for the outposts on new console and File Explorer windows. No connection
to the window follows the probe, where NVDA's probe connects as part of
it, so a new File Explorer window gets no UIA connection at Verbatim's
first sight of it. The window is asked once. A window that did not answer
the probe within its 3 seconds has an unknown verdict: the listener keeps
nothing of it and drops the focus, naming the window; an outpost reads the
window through MSAA for the rest of the entry that probed it, as NVDA
takes a cancelled probe as "no provider", and keeps that "no" for 500 ms
from the probe's end, where NVDA times its cache from before the probe
(`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated fact",
the verdicts). A provider probe so stays within the outpost's deadline.
The 3 seconds stay, though `UiaHasServerSideProvider` waits about 5
seconds on a window that does not pump, since the listener's read
deadline is derived from them
(`experiments/focus-pipeline-rework-measurements/server-side-provider-under-stall.txt`).

### UIA timeouts

NVDA leaves UIA's connection timeout at its default of 2 seconds and its
transaction timeout at its default of 20. Verbatim: **different**
(`docs/design/listener-without-connections.md`, section 5): the
connection timeout is UIA's default of 2 seconds, as NVDA's is, but the
transaction timeout is 5 seconds.

### How an outpost turns events into focus reports

NVDA: [The UIA client](../nvda/uia.md), "Caching discipline", and
[MSAA and winevent handling](../nvda/msaa.md). NVDA builds the focus
object from the event's sender. The sender's cache holds only NVDA's
base cache properties (`baseCachePropertyIDs` in its UIA handler: name,
control type, class, automation id, framework, process,
`HasKeyboardFocus`, and a few more), so its name and role come from the
event. Its states, value, description, shortcut, position, level, and
bounds are not in that cache: the focus object's `event_gainFocus`
fetches them in one `buildUpdatedCache` (`_focusPrefetchUIAPropertyIDs`
and `_UIAStatesPropertyIDs`) when NVDA handles the focus, and they are
spoken from that fetch. So going back to a File Explorer folder, NVDA
says "Inner 1 of 4": Explorer raises the focus event before it selects
the item, and selects it before the focus is handled. NVDA has the
element from the event and never looks for it.

Verbatim: **matched (unverified)**, case by case:

- The focus listener judges each UIA focus as NVDA does: on its thread
  for the application, a few milliseconds after the event, it reads the
  event's own element's `HasKeyboardFocus` live and drops the focus when
  it is false, as NVDA reads `currentHasKeyboardFocus` in
  `shouldAllowUIAFocusEvent`, then reads the element's states, value,
  and details afresh with one cache-building call, and its ancestors one
  step up at a time, every call on the element it holds and none a
  remote operation (`docs/design/focus-pipeline.md`, section 12;
  `docs/design/listener-without-connections.md`, sections 2 and 3).
  Pinned by mockapp's `a_focus_reports_the_states_read_when_it_is_handled`
  and Core's
  `a_focus_read_after_its_item_was_selected_is_announced_without_the_selection`.
  A focus reported without its element, which was not found in time,
  keeps the event's values. The outpost reports every focus the listener
  read, whatever has happened since, as NVDA queues every focus it
  accepts, and Core culls the speech of one no longer current.
- When the listener's read fails (its application does not answer within
  the call timeout), the focus is reported from its event with its
  ancestors unknown, as NVDA lets a focus through whose
  `HasKeyboardFocus` read fails, whatever the event's cached
  `HasKeyboardFocus` says, always as the focus, with the focused state
  (mockapp's
  `a_focus_whose_read_fails_is_reported_whatever_its_event_says_of_its_focus`).
  When the read's time budget has passed before the keyboard focus is
  read, the event's cache stands in, as `docs/parity/focus-events.md`, "A
  repeat when a read's time is up", records with its tests.
- The console host's window element never gives the focus, whatever it
  reports, as NVDA's `consoleUIAWindow` refuses its focus events
  (`NVDAObjects/UIA/winConsoleUIA.py` 356 to 358 and 446 to 447): one
  rule, applied by the focus listener before any read.
- Different, because of the outposts (D9, D13): the event's element is
  in the listener's process and cannot cross to the outpost (experiment
  E-marshal). The outpost, after reporting the focus, finds its own copy
  by the runtime ids of the chain the listener read, from the nearest
  window, for its subscriptions, text, and navigation. An element that
  path does not reach, such as Windows Terminal's tab focus elements,
  which are in no tree, is reported from the listener's read with its own
  ancestors; its changes are followed from its window until its own event
  brings the element, and review and navigation start from the nearest
  ancestor found (`docs/design/focus-pipeline.md`, decision 3 of section
  15).
- A focus's ancestors are read only until they meet the previous focus's
  chain, whose rest is reused, as NVDA's focus ancestry does. Different:
  reading the rest is limited to two seconds, after which the ancestors
  are reported unknown rather than the focus being held back; NVDA waits.
- An MSAA focus is accepted only when the object or one of its ancestors
  has the focused state (`shouldAllowIAccessibleFocusEvent`); the states
  are checked first and read live, as NVDA reads them, ancestors
  included. A repeated MSAA focus is dropped before it is read, as NVDA's
  `isDuplicateIAccessibleEvent` drops it (`docs/parity/focus-events.md`,
  "Duplicate focus suppression"). `msaa_focus_changes_cost_exactly` pins
  both.
- Each consecutive run of a batch's MSAA focus events and foreground
  changes is tried newest first until one is reported, as NVDA's event
  pump tries a run of focus and foreground events: when the newest cannot
  be reported (unreadable, destroyed, refused, or a foreground change
  whose window is not the foreground window, or that NVDA's tests against
  the last focus queued drop), the next older one is tried ("MSAA winevent
  flood control").
- The fake focus after a menu or the Alt+Tab switcher closes is skipped
  only when a focus observed after the close was actually applied; a
  focus fact that is dropped or unreadable does not skip it.
- NVDA's early `WinEvent` filters: object ids at or below `OBJID_ALERT`,
  a focus on a menu bar object itself, foreground events for Program
  Manager and the taskbar, and menu events from the IME candidate
  window are ignored. Selection add, remove, and within events are
  changes of state, not new selections, so a deselected item is not
  announced as selected.
- Events are judged by their own application's window, never the
  system's focus window, which can belong to another application; UIA
  notifications are not arbitrated, as NVDA does not arbitrate them.

### UIA caching discipline

NVDA: the `baseCacheRequest` pattern ([The UIA client](../nvda/uia.md),
"Caching discipline"). Verbatim: **matched (unverified)**: cached
elements and scoped search. No audit against NVDA's per-event cache
contents has been made.

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
  ancestor's event to an ancestors-scoped registration, checked on
  Windows 11 build 26200 against the taskbar's clock button and against
  mockapp, whose group raises the change correctly through
  `UiaRaiseAutomationPropertyChangedEvent` on its own provider
  (`docs/uia-bugs.md`, "Ancestors cannot be cached or listened to").
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
only the newest menu event, outside both limits. A foreground event pops
a focus event waiting on its window's client object from the same thread
(`orderedWinEventLimiter.py`, `addEvent`, 65 to 67). `pumpAll`
(`IAccessibleHandler/__init__.py`) then walks the flushed events in
order, collecting each consecutive run of focus and foreground events
and processing it newest first, stopping at the first valid one, when
the next other event comes; the menu event is tried last.

Verbatim: **matched (unverified)**, in each outpost's intake
(`docs/design/focus-pipeline.md`, sections 5.1 and 5.2, which own the
ordering; [verbatim-outpost](../crates/verbatim-outpost.md), the queue
under `Outpost`; `outpost/handover.rs` for the hand-over's ordering and
its one limit). Every MSAA event is counted by the thread it carries,
whichever hook took it in. Every foreground change of a batch is tried
with the focus events of the run it is in (Dickson, 2026-10-20), and a
newer change of the same window from the same thread replaces one
waiting, as NVDA's limiter keeps one per window, object and thread. A
run whose events all carry the time Windows raised them is tried in that
order, the order NVDA's one hook receives them in, since an outpost can
receive the focus listener's copy of an event after its own hook's copy
of a later one during a hand-over. A foreground change removes a focus
event waiting on its window's client object from the same thread when
the focus was not raised after it, as NVDA's limiter does, compared by
raise time; the hook takes an event on a window object as the client
object, as NVDA's callback does. The limiter so keeps, in one run, the
newest 4 focus and foreground events and every older foreground change
of another window, or of the same window from another thread, that its
thread's limit of 10 keeps.

Before an outpost hears them, the focus listener coalesces its facts one
per object and kind, and for an MSAA event other than a menu opening,
the thread that raised it, and the supervisor holds the facts that
arrive while an outpost starts by that rule too, and lets a newer fact
replace a waiting one when an outpost's queue is full.

### MSAA foreground changes judged against the last focus

NVDA's `processForegroundWinEvent` (lines 842 to 867) makes two tests
against the last focus queued, and a change they drop lets the run's
next older event be tried. Verbatim: **matched (unverified)**: the
outpost's foreground handling makes them after its in-front test and
before any read: the last focus or foreground change the outpost
reported, while no focus Core queued from another application has
replaced it, is in the change's window or a child window of it
(`isDescendantWindow`, which follows parents only), or came from the
change's own event address. The fact the hook forwards carries the
window alone, so the change's object and child ids are taken as the
system raises them, on the window object, which the hook, as NVDA's
event callback, takes as the client object (`OBJID_CLIENT`,
`CHILDID_SELF`). A last focus Core took from another outpost inside the
window, which this outpost cannot see, is left to the reducer, which
ignores a foreground change of the window the focus is already in.
Mockapp's `msaa_events` tests pin the first test and the replacement by
another application's focus, and the worker's unit tests both tests.

### What ends an MSAA event run

NVDA's pump ends a run at any other event it receives, including a menu
end and a switch end, and drops MSAA change events from a window read
through UIA only after its pump's acceptance test
(`IAccessibleHandler/__init__.py` 577, after 1056). It hooks events the
outposts never subscribe to (`internalWinEventHandler.py` 34 to 63).

Verbatim: **different** (Dickson, 2026-10-20). Entries NVDA's pump never
receives neither join a run nor end it, and are handled at their own
places: UIA events and UIA focus reads, Core's queries and its list of
held nodes, the outpost's settle request, an object destroyed, the
console host's events, and a text control's selection change. None of
the sequences this decides is captured from NVDA. A focus's caret is
read within the focus's own handling, as NVDA reads an object's text
within its announcement, so rapid tabbing through an edit field N, with
a newer focus B already waiting, reads N's caret before B is reported,
as NVDA reads N's line with N's focus. The differences:

- Events from a window the system reports hung are dropped before any
  read.
- An MSAA event the outpost's acceptance test will drop ends a run,
  where NVDA's pump filters it out before its run, since the test is
  made against the foreground window when the event is handled.
- A menu end and a switch end do not end a run in an outpost, since the
  focus listener takes them.
- Events NVDA's pump receives that never reach the intake end no run
  there: MSAA change events from a window known to be read through UIA,
  which the outpost drops at its hook or does not hook
  (`Outpost::take_in_win_event`), and the events the outpost never
  subscribes to, which NVDA hooks: the IA2 caret, document-load,
  attribute and page events, live region changes, scrolling start,
  desktop switch, caret show and hide, and the shows NVDA accepts besides
  tooltips. For an application with an MSAA tree beside a UIA item view,
  [focus A in the tree, a name change in the item view, focus B in the
  tree] is heard "A, B" in NVDA, whose pump the name change ends a run
  in, and "B" in Verbatim.

### MSAA menu events in one flush

NVDA's limiter keeps its newest menu event in one slot for all four of
its menu events, menu start and end and popup start and end
(`MENU_EVENTIDS`, `orderedWinEventLimiter.py` 12 to 17; `_lastMenuEvent`,
76 to 78), and its one limiter covers every application. Verbatim:
**different** (Dickson, 2026-10-21): an outpost's intake keeps a batch's
newest popup start, per application. So a popup start followed in one
flush by a popup end, a menu end or a menu start of any application, or
by another application's popup start, is announced by Verbatim and not
by NVDA, whose slot keeps only the later event
(`IAccessibleHandler/__init__.py` 1093 to 1103). Each needs two menu
events within one flush, so they are rare, and none is changed.

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
