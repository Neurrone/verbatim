# Windows and processes

Windows whose parts live in different processes or behave unlike ordinary controls: a window in two processes, content hosted in another process's window, console windows, a control's own focus, the shell's staging windows, and the Alt+Tab switcher's pane and items.

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
completes the chain with the host or owner window and every window above
it as plain windows, named by their text, with no call into the other
process (`docs/design/listener-without-connections.md`, decision 7 and
section 4.4; `docs/design/focus-pipeline-rework.md`, "Calls that reach
another application"). **different** (decision 7 of that design,
Dickson, 2026-10-18, amended 2026-10-29): such a window is named by its
text with the window role, where NVDA reads it through UIA or MSAA. For
the Settings frame NVDA's object in the chain is a window named by its
title too, which it never speaks; at entry the frame's own foreground
focus is spoken. Reading them through the host window's own outpost by UIA
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

Verbatim: **matched (verified)** for what is said, by NVDA's capture of
Alt+Tab of 2026-10-09 (Dickson, 2026-10-24, that speech is cut there as
NVDA cuts it). A focus or foreground change on such a window
(`ForegroundStaging`, `LauncherTipWnd`,
`ApplicationManager_DesktopShellWindow`) is reported with the window's
class and taken like any other focus (decision D6;
`docs/crates/verbatim-core.md`, its rules for particular applications'
windows): the focus, the navigator and, for a foreground change, the
foreground object move to the staging window, and speech is cut when the
foreground window changed, but nothing is said for it, whatever its
name. As in NVDA, the focus that follows, in the window switched to, is
a new focus and is announced. No scenario reaches a staging window, and
the Windows+X menu's host took neither the focus nor the foreground in
the `quick_link_menu` scenario's runs, so Core's unit test pins the rule
(`a_focus_or_foreground_on_a_shell_staging_window_cuts_speech_and_says_nothing`);
a hand-run Alt+Tab check of 2026-10-24 found every switch completing and
nothing said for the staging window.

### The Alt+Tab switcher's pane

On Alt+Tab in Windows 11, File Explorer gives the focus to the
switcher's `Windows.UI.Input.InputSite.WindowClass` window, whose own
element is a pane of that UI Automation class, with no name. Windows
raises the `WinEvent` focus on that window in every switch; a UI
Automation focus on the pane follows only in some, with Alt held (two of
six switches and one of twelve in the runs below). When none follows
within about a second, Verbatim's outpost reads the focus itself and
reports it as a UI Automation focus, which NVDA does not do
([Focus events](focus-events.md), "A focus UIA does not deliver").

NVDA's File Explorer app module gives a UI Automation object in File
Explorer's process a class that refuses its UI Automation focus events
when the object's own UI Automation class and role are
`MultitaskingViewFrame` and window (Windows 10) or
`Windows.UI.Input.InputSite.WindowClass` and pane (Windows 11)
(`appModules/explorer.py` 35 to 36 and 290 to 302). NVDA's UI Automation
handler drops a focus-changed event, and a menu-opened event, which it
handles as a focus, for such an object as the event arrives, before it
becomes NVDA's focus (`UIAHandler/__init__.py` 261, 856 and 948), so the
focus, the navigator and the foreground stay where they were, speech is
not cut, and nothing is said. A foreground change is not refused so.
NVDA's capture of Alt+Tab between two console windows, four quick
switches and two with Alt held
(`target/e2e-evidence/switcher-pane-2026-10-24/NvdaSw/`), said nothing
for the switcher's pane in any switch. This rule does not explain all of
that silence: an independent observer in the same run saw no UI
Automation focus on the `InputSite` pane, but one on the Task Switching
window's own pane, of class `XamlExplorerHostIslandWindow`, in four
switches: late in three, 0.66 to 0.77 seconds after the switch
completed, and during the fourth, with Alt held, just before its list
item's focus; which the rule does not refuse, and NVDA said nothing for
it either; why is not established.

Verbatim: **matched (verified)** for the pane's focus by NVDA's capture
and two hand-run checks of 2026-10-24 (`ExactV/` and `ExactV2/` in the
same folder, each four quick switches and two with Alt held): Verbatim
said nothing for the pane in any of the twelve switches, and announced
the window switched to in each; the flight recorder
(`ExactV2/flight.jsonl`) shows the outpost's own reads and a listener
focus, each with class `Windows.UI.Input.InputSite.WindowClass`, role
pane and application `explorer`, producing no effect. Core refuses a
focus that is not a foreground change, whose node was read through UI
Automation, whose application is `explorer` (`WindowFacts::application`,
named as NVDA names an application module), and whose node's own class
(`NodeDetails::class_name`) and role are NVDA's, before anything else
judges it (decision D6; `docs/crates/verbatim-core.md`, its rules for
particular applications' windows), so it moves nothing and cuts nothing;
the outpost's own read is refused by the same rule. The class of the
window the focus is in is not matched: Windows Terminal's terminal
control is in an `InputSite` window too (`ExactV2/flight.jsonl`), so a
pane of another class there, or one of that class in another
application, is taken as any focus. No scenario can use Alt+Tab
(`docs/testing.md`), so Core's unit tests pin the rule
(`a_uia_focus_on_the_task_switching_pane_is_refused_whole`,
`a_uia_focus_on_the_windows_10_task_switching_frame_is_refused_whole`,
`a_pane_is_refused_only_by_its_own_class_in_file_explorer`,
`the_task_switching_panes_foreground_change_or_msaa_focus_is_taken`);
no Windows 10 machine has run it. Different: the outpost's own read of a
focus UI Automation did not deliver is Verbatim's alone (above).

### The Alt+Tab switcher's items

NVDA does not refuse the switcher's list items: with Alt held, NVDA says
the item reached alone, by its window's title, row, column and position
("Nvda Sw B  row 1  column 2  2 of 10"), and then the window switched
to. The same module gives a UI Automation list item whose parent has the
automation id `SwitchItemListControl`, or whose window's class is
`MultitaskingViewFrame`, a container that is the desktop while Alt is
held (`appModules/explorer.py` 39 to 45 and 303 to 311), so the item's
focus ancestry is the desktop alone and no container is spoken
([App modules](../nvda/app-modules-mechanism.md)). A probe of 2026-10-25
read the Windows 11 switcher's item (class `ListViewItem`) with its
raw-view parent the list "Running applications", automation id
`SwitchItemListControl`, and its `GridItem` row 0 and column 1, so this
rule applies on Windows 11 and the row and column are the grid item's
([Focus announcement](focus-announcement.md), "Table cell coordinates";
the probe's script and output are
`target/e2e-evidence/switcher-pane-2026-10-24/probe-item.ps1` and
`Probe/`).

Verbatim: **matched (verified)** for the item by NVDA's capture and a
hand-run check of 2026-10-25 (`ItemV/` in the same folder, four quick
switches and two with Alt held): in both switches with Alt held Verbatim
said the item alone, "ItemVB row 1 column 2 2 of 11" and "ItemVA row 1
column 2 2 of 11", then the window switched to, as NVDA says "Nvda Sw B
row 1  column 2  2 of 10" (eleven windows were open in that run, ten in
NVDA's). A UI Automation focus's snapshot carries its raw-view parent's
automation id (`NodeDetails::parent_automation_id`) and its window facts
whether Alt was held when the outpost read them (`WindowFacts::alt_held`,
`GetAsyncKeyState`); Core takes a UI Automation list item in `explorer`,
read with Alt held, whose parent's automation id is
`SwitchItemListControl` or whose window's class is
`MultitaskingViewFrame`, as having no containers, known to be none, so
none is spoken or kept for the next focus (decision D6). The item says
its name, row, column and position. Core's unit tests pin the rule
(`a_task_switching_item_reached_with_alt_held_says_only_itself`,
`only_the_task_switchers_items_drop_their_containers`), and mockapp's
`a_focus_carries_its_parents_automation_id` the parent's automation id
from the listener's read and the outpost's own read. Cases that differ:
a focus-now answer carries no application name and no Alt, so the rule
never applies to one, where NVDA's applies however the item is reached;
a second Tab with Alt held says "row 1" again, where NVDA leaves an
unchanged row out ([Focus announcement](focus-announcement.md), "Table
cell coordinates").
