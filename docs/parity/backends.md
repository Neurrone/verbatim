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
Windows Forms (a Windows Forms list view was recognized live). A probe's
verdict, a provider or none, is kept by Core for half a second from the
probe's start, NVDA's cache period; an outpost keeps a verdict only
within one request, so a window a walk crosses that Core is never told
of is probed once per request ([ADR
0004](../../books/dev/src/reference/adr/0004-focus-validity.md)).

Not yet (M7): NVDA's Word, Excel, and Chromium exceptions, which set UIA
aside only when NVDA has injected its in-process helper; without it NVDA
uses UIA for them, as Verbatim does, and they arrive with Verbatim's
helper (decision D2). Not carried: the Office 2013 and older ribbon rule
(`NetUIHWND`), for Office versions Microsoft no longer supports, and the
rule against NVDA's own process, which guards against a freeze of UIA
inside the screen reader's process that outposts cannot have.
Application modules' own good and bad windows wait for extensions,
except the Explorer shell rule above, which is core policy.

### The class lists the per-window verdict reads

The per-window verdict above is judged from the window's class, normalized first. These are NVDA's lists at the pin, written here so that Core and both readers apply them, from one copy in `verbatim-model`, to a window's class.

Normalizing a class name (`Window.normalizeWindowClassName`, `NVDAObjects/window/__init__.py` 338 to 362): a name found in NVDA's class map is replaced by its target; otherwise a Windows Forms name of the form `WindowsForms` followed by any number of digits, none included, a dot, the inner name up to the last `.app.`, and anything after, or an `ATL:` name, is reduced to its inner name, which is then looked up in the map; an empty inner name, or any other name, is kept (25 to 26). The map (444 to 490), by target:

- To `Edit`: `EDIT`, `TTntEdit.UnicodeClass`, `TMaskEdit`, `TTntMemo.UnicodeClass`, `TRichViewEdit`, `TInEdit.UnicodeClass`, `TInEdit`, `TEdit`, `TFilenameEdit`, `TSpinEdit`, `ThunderRT6TextBox`, `TMemo`, `TPasswordEdit`, `THppEdit.UnicodeClass`, `TUnicodeTextEdit.UnicodeClass`, `TTextEdit`, `TPropInspEdit`, `TFilterbarEdit.UnicodeClass`, `EditControl`, `TNavigableTntMemo.UnicodeClass`, `TNavigableTntEdit.UnicodeClass`, `TAltEdit.UnicodeClass`, `TAltEdit`, `TDefEdit`, `TAccessibleEdit`, `TNumEdit`.
- To `RichEdit20`: `TRichEdit`, `WFMAINRE`, `RichEdit20A`, `RichEdit20W`, `TChatRichEdit`, `TskRichEdit.UnicodeClass`, `RichEdit20WPT`, `TChatRichEdit.UnicodeClass`, `TMyRichEdit`, `TExRichEdit`, `RichTextWndClass`, `TSRichEdit`, `TRxRichEdit`, `TAccessibleRichEdit`.
- To `RichEdit`: `RICHEDIT`, `TRichEditViewer`.
- To `RICHEDIT50W`: `RICHEDIT60W`, `RICHEDIT60W_WLXPRIVATE`.
- To `Scintilla`: `ScintillaWindowImpl`.

The verdict, in NVDA's order (`_isUIAWindowHelper`, `UIAHandler/__init__.py` 1206 onward): a window of NVDA's own process is not UI Automation (not carried, see above); a normalized class in the good list is UI Automation; an application module may then call it good; a normalized class in the bad list is not UI Automation; an application module may then call it bad; Office 2013 and older ribbons are not UI Automation (not carried); otherwise the window is probed.

- Good (96 to 107): `RAIL_WINDOW`, `Microsoft.UI.Content.DesktopChildSiteBridge`, `CASCADIA_HOSTING_WINDOW_CLASS`.
- Bad (109 to 132): `Microsoft.IME.CandidateWindow.View`, `SysTreeView32`, `WuDuiListView`, `ComboBox`, `msctls_progress32`, `msctls_trackbar32`, `Edit`, `CommonPlacesWrapperWndClass`, `SysMonthCal32`, `SUPERGRID`, `RichEdit`, `RichEdit20`, `RICHEDIT50W`, `Button`, `FoxitDocWnd`, `MozillaWindowClass`, `MozillaDropShadowWindowClass`, `MozillaDialogClass`, `MozillaContentWindowClass`.
- The Explorer module's good windows on Windows 11 (`appModules/explorer.py` 453 to 480): a window whose root ancestor (`GA_ROOT`) has the class `Shell_TrayWnd`, `Shell_InputSwitchTopLevelWindow`, `XamlExplorerHostIslandWindow` or `TopLevelWindowForOverflowXamlIsland`, except a window of class `Start`; Verbatim applies this as core policy for every process, as above. The module's `ApplicationFrameWindow` rule, for the emoji panel, is not carried, since the probe answers those windows correctly.

### Whether a UIA element is native

NVDA handles a UIA focus or automation event only from an element it
judges native (`isNativeUIAElement`, `UIAHandler/__init__.py` 1516 to
1575; called by the focus handler at 887 and the automation event
handler at 819 to 824; the notification handler, from 1063, does not
call it). In order:

- Not native when the element's cached process id cannot be read (1522
  to 1527), or is NVDA's own process (1528 to 1533).
- Otherwise its nearest window is found (`getNearestWindowHandle`, 1383
  to 1460): a cached walk up to the nearest element with a native window
  handle, its answer kept on the element; an element of Windows Defender
  Application Guard takes the active window instead when that is the
  Application Guard window of its process, and no window otherwise
  (1407 to 1429). With no window, or when the walk fails, the element is
  not native.
- Native when that window is a UIA window (`isUIAWindow`, 1368 to 1381:
  the per-window decision, "Dual-stack MSAA+UIA with per-window
  arbitration" above, its verdict kept 0.5 s).
- Otherwise native when the element is inside a NetUI container
  (an ancestor of class `NetUIHWNDElement` with the same window)
  embedded in a Word document window, such as Word's modern comments
  pane, which only UIA exposes (1550 to 1555; `_isNetUIEmbeddedInWordDoc`,
  1462 to 1489).
- Otherwise native when it is in Internet Explorer 9's downloads list:
  its window's class is `DirectUIHWND`, its provider description names
  `IEFRAME.dll`, and its own class is `DownloadBox`, `accessiblebutton`,
  `DUIToolbarButton` or `PushButton` (1556 to 1572).
- Otherwise not native (1573 to 1575).

Verbatim: **partial**. The focus listener drops a UIA event whose
nearest window's lookup fails or finds none, and one whose window the
arbitration judges MSAA (`docs/parity/focus-events.md`, "A focus UIA
does not deliver"). The Word NetUI and Internet Explorer 9 exceptions
are not built.

### The UIA provider probe

NVDA asks `UiaHasServerSideProvider`, which connects to the window, and
times its cached verdict from before the probe, so a "no" that took the
whole wait is out of date when stored ([The UIA client](../nvda/uia.md),
"The per-window UIA decision").

Verbatim: **different** (`books/dev/src/explanation/design/listener-without-connections.md`,
decision 2; the outposts' probe decided by Dickson on 2026-10-20; the one
verdict per window decided by Dickson on 2026-10-30,
`books/dev/src/reference/adr/0001-focus.md`, "One judge per fact"): each outpost alone probes its application's windows, with
one raw `WM_GETOBJECT` for `UiaRootObjectId`, counting only the answer
`0x10000` as a provider and waiting at most 3 seconds, rather than with
`UiaHasServerSideProvider`, which answered "no" falsely beside another
process's queued remote operation (`e34`), under `e38`'s trigger, and on
new console and File Explorer windows. Its verdict is the one verdict per
window, as NVDA's one cache is, judging both routes' events by the verdict
that stood at each event's observation; the focus listener judges none, its
check thread making every UIA focus's keyboard-focus reads whatever its
window's verdict, and carrying the probe it made, for Core to judge. No
connection to the window follows the probe, where NVDA's probe connects as
part of it, so a new File Explorer window gets no UIA connection at
Verbatim's first sight of it. The window is asked once. A window that did
not answer the probe within its 3 seconds has an unknown verdict: the
outpost reads the window through MSAA, as NVDA takes a cancelled probe as
"no provider", and keeps that "no" for the events observed until 500 ms
after the probe's end, where NVDA times its cache from before the probe. A
provider probe so stays within the outpost's deadline. The 3 seconds stay,
though `UiaHasServerSideProvider` waits about 5 seconds on a window that
does not pump (Dickson, 2026-10-30; the `stall` case of
[the program that measured it](https://github.com/Neurrone/verbatim/blob/e29c8f3a/uia-facts/uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/readme.md),
in git at commit e29c8f3a).

### UIA timeouts

NVDA leaves UIA's connection timeout at its default of 2 seconds and its
transaction timeout at its default of 20. Verbatim: **different**
(`books/dev/src/explanation/design/listener-without-connections.md`, section 5): the
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
  remote operation (`books/dev/src/explanation/design/focus.md`, "One UI
  Automation focus, end to end";
  `books/dev/src/explanation/design/listener-without-connections.md`, sections 2 and 3).
  Pinned by mockapp's `a_focus_reports_the_states_read_when_it_is_handled`
  and Core's
  `a_focus_read_after_its_item_was_selected_is_announced_without_the_selection`.
  A focus reported without its element, which was not found in time,
  keeps the event's values. The outpost reports every focus the listener
  read, whatever has happened since, as NVDA queues every focus it
  accepts, and Core culls the speech of one no longer current.
- When the listener's read fails (its application does not answer within
  the call timeout), the focus is reported from its event, its chain
  going on from the parent window live, as NVDA lets a focus through whose
  `HasKeyboardFocus` read fails, whatever the event's cached
  `HasKeyboardFocus` says, always as the focus, with the focused state
  (mockapp's
  `a_focus_whose_read_fails_is_answered_whatever_its_event_says_of_its_focus`).
  A keyboard focus read that fails lets the focus through, as
  `docs/parity/focus-events.md`, "A focus whose keyboard focus read
  fails", records with its tests.
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
  ancestor found (`books/dev/src/reference/adr/0001-focus.md`, "The outpost's
  own element").
- A focus's ancestors are read only until they meet the previous focus's
  chain, whose rest is reused, as NVDA's focus ancestry does. Different:
  reading the rest is limited to two seconds, after which the ancestors
  are reported unknown rather than the focus being held back; NVDA waits.
- An MSAA focus is accepted only when the object or one of its ancestors
  has the focused state (`shouldAllowIAccessibleFocusEvent`); the states
  are checked first and read live, as NVDA reads them, ancestors
  included. Past an `accParent` that fails or names nothing the test goes
  on from the parent window's own object, as NVDA's `Window._get_parent`
  builds it: its MSAA client or its UIA element, by the outpost's verdict;
  a plain window, a ghost's, a hung one or one above the process boundary,
  is never focused, as NVDA's `Window` has no focused state (Fable,
  2026-10-30;
  `the_focused_state_test_continues_past_a_failed_acc_parent` pins the
  drop). A repeated MSAA focus is dropped before it is read, as NVDA's
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
  alone, and the outcome is the same, because a registration scoped
  to the ancestors alone receives no ancestor's event and adding the
  ancestors to the element's scope hears nothing beyond the element's
  scope alone, checked on Windows 11 build 26200: in a provider's tree
  view, a registration on the focused item with NVDA's scope,
  Verbatim's or the ancestors alone heard none of the tree view's name
  changes, raised through `UiaRaiseAutomationPropertyChangedEvent`; on
  the taskbar, a registration on the clock button's child heard the
  button's name change with NVDA's scope and with Verbatim's in the
  same minutes, and never with the ancestors alone
  (`books/dev/src/reference/uia-bugs.md`, "Ancestors cannot be cached or listened to").
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
- Caret and text changes: the caret event is in the local group of
  every focus; text changes only on the focuses the next section names.
- NVDA checks that the element is still focused before registering on
  it (comparing it with `GetFocusedElement`, a call). Verbatim does
  not: the registering thread registers only the newest focus a
  registration was handed, and a newer focus replaces it.

### Which focus takes text-change events

NVDA, under selective registration, gives the local group with text
changes only to a focus of UI Automation class `_WwG` (Word's document),
of automation id "Text Area" (the console host's text area), or of class
`TermControl` or `WPFTermControl` (Windows Terminal) unless Windows
Terminal is set to notifications, which it is not by default; every
other focus gets the local group whose text event is the caret event
alone (`UIAHandler/__init__.py`, `textChangeUIAClassNames`,
`textChangeUIAAutomationIDs` and `windowsTerminalUIAClassNames`). Under
global registration it takes text changes desktop-wide but drops those
from other classes, so the outcome is the same. Its wait for a caret
key's effect therefore counts a text change only in those controls
(`editableText.py`, `_hasCaretMoved`).

Verbatim: **matched**. Core alone names the group in `Follow`: a
terminal its terminal group (Windows Terminal's text area with text
changes; the console host's text area without them, its console's own
events telling its output), Word's document, by its class `_WwG`, the
extended group, and every other focus, edit fields and documents
included, the base group, with the caret event. A focus's completion in
the outpost decides nothing: it gives the new node the base group, and
the node `Follow` names keeps the group Core named. Not covered: an
element with automation id "Text Area" outside a console host, which
Verbatim does not read as a terminal. Limit: between a Word document's
completion and Core's `Follow`, one commit, it has the base group, so a
caret key there hears its caret event but no text change; NVDA registers
when it handles the focus.

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
happens. Verbatim: **partial**. Matched as far as it goes: outside
browse mode nothing happens, and no reader registers for the event,
since nothing would act on it. Not yet (M7): browse mode, which moves
its caret there and speaks the line, as NVDA's does; it will need the
registration in the outpost (the range belongs to the application and
cannot travel to another process, so the focus listener cannot carry
it), on the document a focus is inside.

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
only the newest menu event, outside both limits. A flush is one pass of
the MSAA pump, once per core cycle (`core.py` 1026) and again whenever
code waiting on the main thread processes pending events
(`api.processPendingEvents`, `api.py` 381 to 396, as a caret key's wait
does), so the limits count the events that arrived since the last pass,
not a span of time. A foreground event is
stored in both caches, each copy with its own arrival counter
(`addEvent`, 65 to 67 and then 79, with no return between), so it
counts toward its thread's 10 and a flush returns it twice, the generic
copy after the focus-cache one. The object whose events are always kept
is the last focus NVDA queued (`eventHandler.lastQueuedFocusObject`),
not the focus it has committed, and only when that is an MSAA object
with an event object id; its window, object id and child id are matched
(`IAccessibleHandler/__init__.py` 1025 to 1031). A foreground event pops
a focus event waiting on its window's client object from the same thread
(`orderedWinEventLimiter.py`, `addEvent`, 65 to 67). `pumpAll`
(`IAccessibleHandler/__init__.py`) then walks the flushed events in
order, collecting each consecutive run of focus and foreground events
and processing it newest first, stopping at the first valid one, when
the next other event comes; the menu event is tried last.

Verbatim: **matched (unverified)**, in each outpost's intake
(`books/dev/src/explanation/design/focus.md`, "One MSAA focus", which owns the
ordering; [verbatim-outpost](../../books/dev/src/reference/crates/verbatim-outpost.md), the queue
under `Outpost`; `outpost/reducer/hold.rs` for the hand-over's hold). Every MSAA event is counted by the thread it carries,
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
thread's limit of 10 keeps. Core, which has no pump, applies both limits
each time an MSAA entry joins its queue, counting only the waiting MSAA
entries whose tells' arrival times lie within 10 ms of the newest one's,
the longest NVDA's core pump waits once requested (`core.py` 55,
`PUMP_MAX_DELAY`), so entries spread over several pump cycles are never
capped against each other, as in NVDA
(`verbatim-core/src/event_queue/limiter.rs`, `PUMP_WINDOW_US`).

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

### MSAA mouse pointer shape changes

When the mouse pointer changes shape, Windows raises a name change on
the desktop window with the object `OBJID_CURSOR`, whose name is the
shape ("Normal", "Working in background"). NVDA hands such a name
change to its mouse-shape reporting before any other handling, so it
never reaches the desktop's virtual-desktop-name check
(`IAccessibleHandler/__init__.py` 673 to 676), and that reporting
speaks only with "Report mouse shape changes" on, off by default
(`config/configSpec.py` 165). Verbatim: **matched (verified)** with that
reporting off, which Verbatim does not offer: the listener drops a name
change of `OBJID_CURSOR` before telling Core (`dropped_by_class`,
`verbatim-listener-logic`), so the pointer's shape is never announced as
a virtual desktop's name, and a name change of the desktop's own object
after it still is (the listener's test
`a_pointer_shape_change_is_not_told_and_the_desktops_name_change_after_it_is`).

### IA2 upgrade of MSAA objects

NVDA: `normalizeIAccessible`
([IA2 usage](../nvda/ia2.md)). Verbatim: **partial**: the verbatim-ia2
QueryService path exists ([verbatim-ia2](../../books/dev/src/reference/crates/verbatim-ia2.md)),
unverified; IA2 text and hypertext consumption is not yet (M7).

### JAB

Verbatim: **not yet (M16)**.

### Remoted UIA trees

Remoted UIA trees (Application Guard-style: valid UIA, locally
invalid window handles and process identity). NVDA: the WDAG
pathway ([The UIA client](../nvda/uia.md)). Verbatim: **not planned**
— MDAG is deprecated and removed in Windows 11 24H2; noted because
Verbatim's window-handle assumptions (arbitration, hierarchy
navigation) would need a treat-as-remote pathway if a successor
technology with the same shape appears.
