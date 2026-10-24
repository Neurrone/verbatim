# Windows and processes

Windows whose parts live in different processes or behave unlike ordinary controls: a window in two processes, content hosted in another process's window, console windows, a control's own focus, the shell's staging windows, and the Alt+Tab switcher's pane.

### A window and its content in two processes

The Settings app's frame,
`ApplicationFrameHost`'s, holds `SystemSettings`'s page. NVDA handles
the frame's foreground event and the content's focus event in one
queue, in the order they were raised, so it announces the window and
then the focus ([Event handling](../nvda/events.md), "Queuing").
NVDA's focus also reaches the frame through its ancestry: its raw-view
walk goes from the page up through the frame window to the desktop.
Verbatim's two outposts, one for each process, report them with
nothing ordering them, and the reducer says nothing for a window
reported after a focus inside it. Verbatim follows NVDA's ancestry: the
focus listener's read goes on past the top-level element of the focus's
own process, where a remote operation's walk ends, the classic way,
until the top-level window, and the outpost reads the frame through its
own API, so the frame is an entered ancestor of the page's focus and is
announced with it whichever outpost is quicker; a foreground report for
the frame that comes before the focus is announced, and the page's
focus does not announce the frame again, since it is the same top-level
window with the same role and name. **matched (unverified)** (a reducer
test feeding both orders). A console window is one application, as
"Console windows" says.

### Content hosted in another process's window

A focus in content one process serves inside a window of another, such
as WebView2 content (`msedgewebview2.exe`) in a WinUI or Win32 host's
window, or in a top-level dialog owned by another process's window,
which UIA's raw view puts under its owner while the owner is visible.
NVDA's raw-view walk goes on up through the host's windows, or from the
dialog to its owner, reading each window through UIA when it has a
provider and through MSAA otherwise ([The UIA client](../nvda/uia.md),
"The per-window UIA decision"). Verbatim: the focus listener stops at
the process boundary, and the outpost of the focus's application
continues the chain from the host or owner window through MSAA
(`docs/design/listener-without-connections.md`, decision 7 and section
4.4). **different** (decision 7 of that design, Dickson, 2026-10-18):
the ancestors above the host window are read through MSAA, where NVDA
reads those with a provider through UIA, so such a window, a WinUI
host's "title window" among them, may be named and roled differently;
for a host without a provider, such as Quick Assist's, both read the
same object. Reading them through the host window's own outpost by UIA
is reconsidered if an NVDA capture of entering such a host differs from
Verbatim's announcement (Dickson, 2026-10-20). An owned dialog seen on
2026-10-19: the Windows Security credential dialog
(`CredentialUIBroker.exe`) opened with File Explorer's folder window
as its parent window is owned by it. NVDA (alpha-58022) said as it opened: "unavailable", then "Windows Security
window", then "User name edit"; the folder window, the previous focus's
own top-level window, is not spoken again either way, so the capture
does not show whether NVDA's chain reaches the owner. The dialog's
window reports UIA's Win32 framework, which suggests it has no provider
of its own; if so, Verbatim reads it through MSAA, as NVDA does, whose
parent is the desktop, so the owner is not read; unverified.

### Console windows

Windows names a console window's owner, by
`GetWindowThreadProcessId`, as the console's client (such as
`powershell.exe`), while the window, its UIA provider and its text
area are the console host's. NVDA gives the window and its text area
the client's app module and learns the console host's real thread from
the console's WinEvents ([App modules, global plugins, and add-ons: the
mechanism](../nvda/app-modules-mechanism.md), "App modules").
Verbatim: the focus listener sends every `WinEvent` for a console
window to the console host's outpost, the process of the event's own
thread (`GetProcessIdOfThread`), where its UIA events already went, and
that outpost reports its events in a console window with the client as
the application, so one outpost reports the window, once, with one
name. Before any event from a console, the console host's outpost
answers for it (at start-up, when the console is in front), Core
finding the host from the client with a local call. A console window's
foreground event comes before the window is in front, and none follows,
and the window is read through UI Automation; NVDA drops the event on
both counts and announces the window as the focus enters it, from the
console window element, named and with the window role, which is the
text area's parent (`eventHandler.doPreGainFocus`). Verbatim does the
same (`docs/parity/focus-context.md`, "Window announcement on switching
applications"). **matched (unverified)**, pending live checks.
Different: NVDA gives a console window's events to the client's app
module whatever the event's thread, its object for the window taking
the window's process, the client, rather than UIA's
(`NVDAObjects/UIA/__init__.py` 2336 to 2346), and takes the window's
thread from each event without opening it
(`IAccessibleHandler/internalWinEventHandler.py` 151 to 153 and 266 to
273); Verbatim routes them to the console host's outpost and drops them
when the host cannot be read from the event's thread, so that no outpost
but the console host's reads its window (Dickson, 2026-10-19;
`docs/design/listener-without-connections.md`, section 10, step 3).

### A control's own focus with a focused child

A Win32 control taking
the focus, such as a tree view (`SysTreeView32`) or a tab control
(`SysTabControl32`), raises a focus event on itself (child id 0) and
then one on its focused child, both within the application's one
`SetFocus` call. NVDA redirects a container's own focus to its
`accFocus` child only for lists, but its event pump runs after both
events are delivered and tries focus events newest first, so the child
wins and the control is spoken only as a new ancestor, without its
shortcut ("Indications: tree view", then the item, on Verbatim's Theme
settings page). Verbatim's outpost could read the control before the
child's event reached it. **matched (verified)** (`mockapp`'s
`focus_reports` test and the `theme_panel` scenario), by a different
route: before reporting an MSAA focus on an object's child id 0, the
outpost asks the object's `accFocus` once, and when that names a child
by id that has the focused state, reports the child instead, with the
object as its ancestor.

### The shell's staging windows

On Alt+Tab, Task View, and the Windows+X
menu, File Explorer gives the focus or the foreground to windows that
only stage the switch, with no name. NVDA's File Explorer app module
leaves the focus on them unsaid, after NVDA has made the window its
focus and run its foreground event, which cuts speech ([App modules,
global plugins, and add-ons: the
mechanism](../nvda/app-modules-mechanism.md), "App modules").
Verbatim: the outposts and the listener report a focus or foreground
change on such a window (`ForegroundStaging`, `LauncherTipWnd`,
`ApplicationManager_DesktopShellWindow`) like any other, with the
window's class, and Core takes it like any other focus (decision D6):
the focus, the navigator and, for a foreground change, the foreground
object move to the staging window, and speech is cut when the
foreground window changed, as for any focus and as in NVDA, but nothing
is said for it, whatever its name. As in
NVDA, the focus that follows, in the window switched to, is a new focus
and is announced. **matched (verified)** for what is said, by NVDA's
capture of Alt+Tab of 2026-10-09; no scenario reaches a staging window
since the two-window scenarios stopped switching with Alt+Tab, and the
Windows+X menu's host took neither the focus nor the foreground in the
`quick_link_menu` scenario's runs, so Core's unit test pins the rule
(`a_focus_or_foreground_on_a_shell_staging_window_cuts_speech_and_says_nothing`),
and a hand-run Alt+Tab check of 2026-10-24 found every switch completing
and nothing said for the staging window.
Dickson decided on 2026-10-24 that speech is cut there as NVDA cuts it,
replacing the earlier rule that dropped the change whole.

### The Alt+Tab switcher's pane

On Alt+Tab in Windows 11, File Explorer gives the UI Automation focus to
the switcher's pane, the element of its
`Windows.UI.Input.InputSite.WindowClass` window, with no name, before
its list items, and in some switches 11 to 15 ms after the window
switched to has taken the focus. NVDA's File Explorer app module gives
every UI Automation object in File Explorer's process whose UI
Automation class is `Windows.UI.Input.InputSite.WindowClass` and whose
role is pane (on Windows 10, the `MultitaskingViewFrame` window) a class
that refuses its UI Automation focus events: the event is dropped as it
arrives, before it becomes NVDA's focus, so the focus, the navigator and
the foreground stay where they were, speech is not cut, and nothing is
said ([App modules, global plugins, and add-ons: the
mechanism](../nvda/app-modules-mechanism.md), "App modules"). The
switcher's list items are not refused: with Alt held, NVDA says the item
reached, by its window's title, row, column and position ("row 1 column
2 2 of 10"), and then the window switched to. NVDA's capture of Alt+Tab between two console windows,
three quick switches and two with Alt held, said nothing for the
switcher's pane in any switch.
Verbatim: Core refuses a UI Automation focus, not a foreground change,
whose node is a pane and whose window facts carry the class
`Windows.UI.Input.InputSite.WindowClass`, before anything else judges it
(decision D6), so it moves nothing and cuts nothing. Core matches the
class of the focus's window and does not know the focus's application,
where NVDA matches the element's own class in File Explorer's process;
for the switcher's pane, its window's own element, the two agree.
**matched (verified)** for what is said, by NVDA's capture and a
hand-run Alt+Tab check of 2026-10-24 with Verbatim before and after
(`target/e2e-evidence/switcher-pane-2026-10-24/`): before, Verbatim
queued "Task Switching window, pane" in five of six switches; after, it
queued nothing for the pane in any. No scenario can use Alt+Tab
(`docs/testing.md`), so Core's unit test pins the rule
(`a_uia_focus_on_the_task_switching_pane_is_refused_whole`).
A case that differs: with Alt held, Verbatim says the switcher's window
and list ("Task Switching window", "Running applications list") before
the item, and the item without its row and column, where NVDA says the
item alone with them. The same app module's rule that drops a task
switching list item's ancestry while Alt is held is not ported; whether
it is what silences the ancestry on Windows 11 is not established.
