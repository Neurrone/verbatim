# Backends

How the MSAA, UIA, and other accessibility backends are read and their events taken in.

### Dual-stack MSAA+UIA with per-window arbitration

NVDA: the
`isUIAWindow` referee with good/bad class lists
([The UIA client](../nvda/uia.md)). Verbatim: **matched since
2026-10-03**, in NVDA's order: the class name is normalized as NVDA
normalizes it (its class map, and the Windows Forms and `ATL:` wrappers
removed, so a Delphi `TEdit` or a Windows Forms edit counts as an
`Edit`); NVDA's good classes are UIA; so is a Windows 11 shell window,
recognized by its root ancestor's class, except the Start button; NVDA's
bad classes are MSAA; otherwise the window is probed. A window with a
provider is still read through MSAA when NVDA would not use the
provider: a console whose text does not report formatting (older
consoles; the Windows 11 console reports it and stays UIA, checked
live), and a list view outside Windows Forms (a Windows Forms list view
was recognized live). Until 2026-10-03 the class was not normalized and
the shell rule tested the window's own class. **Not yet (M6):** NVDA's
Word, Excel, and Chromium exceptions, which set UIA aside only when NVDA
has injected its in-process helper; without it NVDA uses UIA for them,
as Verbatim does, and they arrive with Verbatim's helper (decision D2).
**Not carried:** the Office 2013 and older ribbon rule (`NetUIHWND`),
for Office versions Microsoft no longer supports, and the rule against
NVDA's own process, which guards against a freeze of UIA inside the
screen reader's process that outposts cannot have. Application modules'
own good and bad windows wait for extensions, except the Explorer shell
rule above, which is core policy (roadmap M3). A probe
that finds a UIA provider is kept for the window's lifetime, one that
finds none for 500 ms, NVDA's cache period. Checked live on
2026-10-02 across about 65 windows of Explorer, Settings, Start, and
the desktop: no window's answer changed from a provider to none
during its life. **Different from NVDA, since 2026-10-03:**
`UiaHasServerSideProvider` reports no provider when the window does
not answer in time (three seconds for Windows 11 Notepad's text control
while Notepad was starting, five for a stalled mockapp window), and
NVDA takes that as the answer, reading the focus through MSAA ("edit")
until it moves. Verbatim counts only the window's own answer: a "no"
slower than a second (real answers took 0 to 89 ms) makes the probe
wait for the window to process messages and ask again. Found as the
cause of `notepad_and_verbatim_menu` failing about one run in five; the
dropped UIA focus had come from Notepad's own provider, and a check 18
ms after the slow "no" answered "yes". The same failure had a second
form: UIA itself gives up on a provider after two seconds by default
(neither NVDA nor Verbatim had changed it), and reading Notepad's
focused element then returned UIA's stand-in for the window, a
nameless edit, instead of the "Text editor" document. Verbatim's UIA
client waits ten seconds, the deadline its watchdog already holds each
read to.

### How an outpost turns events into focus reports

How an outpost turns events into focus reports, from the audit of
2026-10-03 (all **matched since 2026-10-03** unless marked otherwise):
- A UIA focus is built from the event: NVDA builds the focus object
  from the event's sender. The sender's cache holds only NVDA's base
  cache properties (`baseCachePropertyIDs` in its UIA handler: name,
  control type, class, automation id, framework, process,
  `HasKeyboardFocus`, and a few more), so its name and role come from
  the event. Its states, value, description, shortcut, position, level,
  and bounds are not in that cache: the focus object's `event_gainFocus`
  fetches them in one `buildUpdatedCache` (`_focusPrefetchUIAPropertyIDs`
  and `_UIAStatesPropertyIDs`) when NVDA handles the focus, and they are
  spoken from that fetch. Until 2026-10-07 Verbatim took those from the
  event too, so going back to a File Explorer folder said "Inner not
  selected 1 of 4" and then "selected", where NVDA says "Inner 1 of 4":
  Explorer raised the focus event before it selected the item, and
  selected it before the focus was handled. Since then the outpost takes
  the name and role from the event and the rest from the focused element
  it reads when it handles the focus, whose cache that read fills, at no
  extra call (**matched since 2026-10-07**; mockapp's
  `a_focus_reports_the_states_read_when_it_is_handled` and Core's
  `a_focus_read_after_its_item_was_selected_is_announced_without_the_selection`
  check it). A focus reported without its element, which was not found
  in time, keeps the event's values. The focus is accepted only when the element has
  the keyboard focus read live, as NVDA reads `currentHasKeyboardFocus`
  in `shouldAllowUIAFocusEvent` (since NVDA commit `3ca80a5fa`), because
  a stale cached value let intermediate focus events through ([The UIA
  client](../nvda/uia.md)) (**matched since 2026-10-06**). The console
  host's window element never gives the focus, whatever it reports, as
  NVDA's `consoleUIAWindow` refuses its focus events
  (`NVDAObjects/UIA/winConsoleUIA.py` 356 to 358 and 446 to 447;
  **matched since 2026-10-08**; since 2026-10-11 as NVDA does it, one
  rule, before any read: before then the outpost reported the window's
  focus when the focused element read answered the window itself,
  provisional, awaiting Dickson; since stage A step 6 the focus listener
  applies it). Since stage A step 6 (2026-10-11,
  `docs/design/focus-pipeline.md`, section 12; awaiting its first build)
  the focus listener judges each UIA focus as NVDA does: on its thread
  for the application, a few milliseconds after the event, it reads the
  event's own element's `HasKeyboardFocus` live and drops the focus when
  it is false, then reads the element's states, value, and details
  afresh and its ancestors in the same remote operation (**matched**).
  The outpost reports every focus the listener read, whatever has
  happened since, as NVDA queues every focus it accepts, and Core culls
  the speech of one no longer current. Gone with the step: the outpost's
  read of the system's focused element for a focus event, and its rules
  for a focus that had moved on since its event (reported as its event
  said, unless its element was gone, held the element focused now, or
  had lost its selection), which were Verbatim's, not NVDA's; and the
  window-element check of 2026-10-08 (mockapp's
  `a_windowed_focus_is_reported_though_the_focused_element_read_answers_a_stand_in`),
  since the listener reads the event's own element whatever kind it is.
  Until 2026-10-03 Verbatim's outpost read the focused element live,
  took the focus from that read, and dropped the focus when the read
  failed; under a busy application the read blocked
  for more than ten seconds or returned UIA's stand-in for the window,
  so the focus was announced wrongly or not at all. When the listener's
  read fails (its application does not answer within the call timeout),
  the focus is reported from its event with its ancestors unknown, as
  NVDA lets a focus through whose `HasKeyboardFocus` read fails.
- **Different, because of the outposts:** the event's element is in the
  listener's process and cannot cross to the outpost (experiment
  E-marshal). Since stage A step 6 the listener reads the focus's
  ancestors from it, and the outpost, after reporting the focus, finds
  its own copy by the runtime ids of the chain the listener read, from
  the nearest window, for its subscriptions, text, and navigation. An
  element that path does not reach, such as Windows Terminal's tab
  focus elements, which are in no tree, is reported from the listener's
  read with its own ancestors; its changes are followed from its window
  until its own event brings the element, and review and navigation
  start from the nearest ancestor found (decision 3 of section 15 of the
  design). NVDA has the element from the event and never looks for it.
- A focus's ancestors are read only until they meet the previous
  focus's chain, whose rest is reused, as NVDA's focus ancestry does.
  **Different:** reading the rest is limited to two seconds, after which
  the ancestors are reported unknown rather than the focus being held
  back; NVDA waits.
- An MSAA focus is accepted only when the object or one of its
  ancestors has the focused state (`shouldAllowIAccessibleFocusEvent`).
  Since 2026-10-07 the states are checked first and read live, as NVDA
  reads them, ancestors included; Verbatim had read the focus and its
  ancestors in full first, and had checked ancestors reused from the
  previous focus's chain as last read. A focus event naming the address
  of the focus last reported, a whole object, is dropped before it is
  read, as NVDA's `isDuplicateIAccessibleEvent` drops it; **different**
  only in that a reported foreground change makes the next such event no
  duplicate, since an outpost does not see focus in other applications,
  which is how NVDA knows the focus left. (`msaa_focus_changes_cost_exactly`
  pins both.)
- When the newest focus event of a batch cannot be reported (unreadable,
  destroyed, refused), the next older one is tried, up to three, as
  NVDA's event pump falls back.
- The fake focus after a menu or the Alt+Tab switcher closes is skipped
  only when a focus observed after the close was actually applied; any
  focus fact used to cancel it, even one later dropped or unreadable.
- NVDA's early `WinEvent` filters: object ids at or below `OBJID_ALERT`,
  a focus on a menu bar object itself, foreground events for Program
  Manager and the taskbar, and menu events from the IME candidate
  window are ignored. Selection add, remove, and within events are
  changes of state, not new selections, so a deselected item is no
  longer announced as selected.
- Events are judged by their own application's window, never the
  system's focus window, which can belong to another application; UIA
  notifications are not arbitrated, as NVDA does not arbitrate them.
- A provider probe stays within the outpost's deadline; a window that
  does not answer is read through MSAA for the event at hand and probed
  again next time, as NVDA treats a cancelled probe.

### UIA caching discipline

UIA caching discipline (cache requests on events and walks). NVDA:
`baseCacheRequest` pattern. Verbatim: **matched (unverified)** —
cached elements + scoped search landed in 918e5b8/4563bff after a
desktop-wide re-find bug; audit against NVDA's per-event cache
contents still open.

### UIA event registration

NVDA: selective registration
(`UIA.eventRegistration`, automatic by default, which is selective from
Windows 11 22H2), with a global and a local event handler group
([The UIA client](../nvda/uia.md)). Verbatim: **matched, narrower, since
2026-10-07**, with no setting, since Verbatim's minimum is Windows 11
24H2. Every registration is an event handler group, registered with one
`AddEventHandlerGroup` call per element; a group that cannot be
registered on an element (most often one that has gone) is logged and
skipped, and removing handlers from an element that died is left to
UIA, as NVDA does.
- The global group, desktop-wide in the focus listener: an element
  selected, a menu opened, and notifications. NVDA's also takes live
  regions, tool tips, windows opened, system alerts, layout
  invalidation, drag and drop, and range values anywhere (progress
  bars); Verbatim subscribes to none of these until it handles them.
- The local group, on the focus: name, value, range value, toggle,
  enabled, expand and collapse, help text, and ControllerFor changes
  (the last two since 2026-10-12). NVDA registers its local group
  (those but the range value, which it follows desktop-wide, the item
  status, which only its app modules act on, and caret events) on the
  focused element with
  `TreeScope_Element | TreeScope_Ancestors`, when it handles a UIA
  focus. Verbatim registers on the focus with `TreeScope_Element`
  alone, and the outcome is the same, because UIA delivers no
  ancestor's event to an ancestors-scoped registration. Checked on
  2026-10-07 (Windows 11 build 26200):
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
  ancestor state rule is listed above as a difference. Until 2026-10-07
  Verbatim registered on the focus and on each reported ancestor with
  `TreeScope_Element`, which delivered those changes only for the
  reducer to drop them.
- Caret and text changes: on a focus that has text, alone. NVDA takes
  text changes only from Word, the console host, and Windows Terminal
  (when its notifications are off); Verbatim takes them from any text
  focus, for typed text and terminal output.
- NVDA checks that the element is still focused before registering on
  it (comparing it with `GetFocusedElement`, a call). Verbatim does
  not: a registration's thread registers only the newest focus it was
  handed, and a newer focus replaces it.

### The active text position

The active text position (UIA's active text position changed event,
raised when an application scrolls to a place in a document without
moving the caret, such as an in-page link's target). NVDA: registered
desktop-wide in its global group; the event is handed to the object,
and only a UIA browse mode document acts on it, moving its caret to the
range's line and speaking the line when the caret was not already on
it; an event whose element is gone, or from an application that is not
responding, is dropped, and outside browse mode nothing happens.
Verbatim: **matched as far as it goes, since 2026-10-07**: the outpost
registers for it on a text focus, in the same event handler group as
its caret and text changes (the range belongs to the application and
cannot travel to another process, so the focus listener cannot carry
it), keeps the range's start as a position in the focus's text, and
sends it to Core (`ActiveTextPositionChanged`), dropping an event whose
element or range cannot be had, or whose window is hung. Core does
nothing with it. **Not yet (M6):** browse mode, which moves its caret
there and speaks the line, as NVDA's does; it will need the
registration on the document a focus is inside, not only a text focus.

### Text attributes through UIA

NVDA: a range's formatting is fetched in
one `IUIAutomationTextRange3::GetAttributeValues` call where the range
has it, else attribute by attribute, and an attribute whose read fails
is "not supported" (its issue 7124); a mixed value is handled apart.
Verbatim: **matched since 2026-10-07** on the classic path, the same
bulk read with the same fallback and the same reading of a failed
attribute; the remote program reads each attribute inside the
provider, and a failed one comes back not supported there too. A
stretch with a mixed value is read again by words and then characters,
as NVDA reads it, on both paths since 2026-10-07 (it was no value
before).

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
**matched since 2026-10-11** (`docs/design/focus-pipeline.md` section 5;
[verbatim-outpost](../crates/verbatim-outpost.md), the queue under
`Outpost`), with these differences: a foreground change keeps its own
place in a run rather than being tried with its focus events, as
before; and events from a window the system reports hung are dropped
before any read. Every MSAA event is counted by the thread it carries,
whichever hook took it in; until stage A step 5 (`docs/roadmap.md`) the
listener's focus, foreground, alert and show facts carried no thread,
and their window's thread stood in. Before 2026-10-11 the limits
counted UIA events too, by the thread of the event's
window, and the newest three focus events of each backend were kept
across the whole batch. Before an outpost hears them, the focus
listener coalesces its facts by the same one-per-object-and-kind rule,
and the supervisor holds the facts that arrive while an outpost starts
by that rule too, and lets a newer fact replace a waiting one when an
outpost's queue is full.

### Event acceptance filtering

Event acceptance filtering (foreground gating, show/hide rules).
NVDA: `shouldAcceptEvent`. Verbatim: the same test, in each outpost
before any call into the application; see "Event acceptance in the
outpost". Background progress bars are not offered (Dickson,
2026-10-10), and no application support asks for opt-ins yet.

### IA2 upgrade of MSAA objects

NVDA: `normalizeIAccessible`
([IA2 usage](../nvda/ia2.md)). Verbatim: **matched (unverified)** —
verbatim-ia2 QueryService path exists ([verbatim-ia2](../crates/verbatim-ia2.md));
IA2 text/hypertext consumption **not yet (M4/M6)**.

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
