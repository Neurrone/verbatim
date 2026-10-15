# Windows and processes

Windows whose parts live in different processes or behave unlike ordinary controls: a window in two processes, console windows, a control's own focus, and the shell's staging windows.

### A window and its content in two processes

The Settings app's frame,
`ApplicationFrameHost`'s, holds `SystemSettings`'s page. NVDA handles
the frame's foreground event and the content's focus event in one
queue, in the order they were raised, so it announces the window and
then the focus. Verbatim's two outposts, one for each process, report
them with nothing ordering them, and the reducer says nothing for a
window reported after a focus inside it, so the window went unannounced
whenever the content's outpost was quicker. NVDA's focus also reaches
the frame through its ancestry: its raw-view walk goes from the page up
through the frame window to the desktop. **Matched since 2026-10-11, as
NVDA does it**: the focus listener's read goes on past the top-level
element of the focus's own process, where a remote operation's walk
ends, the classic way, until the top-level window, and the outpost reads
the frame through its own API, so the frame is an entered ancestor of
the page's focus and is announced with it whichever outpost is quicker;
a foreground report for the frame that comes before the focus is
announced, and the page's focus does not announce the frame again,
since it is the same top-level window with the same role and name (a
reducer test feeding both orders). From 2026-10-08 until then an
outpost reporting a focus whose top-level window belonged to another
process reported that window first, as a foreground report, a step
NVDA does not have. A console window was handled the same way until
2026-10-11; it is now one application, below.

### Console windows

Windows names a console window's owner, by
`GetWindowThreadProcessId`, as the console's client (such as
`powershell.exe`), while the window, its UIA provider and its text
area are the console host's. NVDA gives the window and its text area
the client's app module (`UIA._get_processID`,
`NVDAObjects/UIA/__init__.py` 2336 to 2346;
`appModuleHandler.getAppModuleForNVDAObject`, `appModuleHandler.py` 227
to 238) and learns the console host's real thread from the console's
`WinEvents` (`consoleWindowsToThreadIDs`,
`IAccessibleHandler/internalWinEventHandler.py` 266 to 273). **Matched
since 2026-10-11, pending live checks**: the focus listener sends every
`WinEvent` for a console window to the console host's outpost, the
process of the event's own thread (`GetProcessIdOfThread`), where its
UIA events already went, and that outpost reports its events in a
console window with the client as the application, so one outpost
reports the window, once, with one name. Before, the client's outpost
reported the window and the console host's outpost reported it again
before the text area's focus, so the window could be read twice, and
an elevated console's two reads gave two names. When the event's
thread cannot be opened, the event goes to the client's outpost, as
NVDA falls back to the window's own thread; and before any event from
a console, the client's outpost answers for it (at start-up, when the
console is in front). A console window's foreground event comes before
the window is in front, and none follows, and the window is read
through UI Automation; NVDA drops the event on both counts and
announces the window as the focus enters it, from the console window
element, named and with the window role, which is the text area's
parent (`eventHandler.doPreGainFocus`). Verbatim does the same since
2026-10-11 ("Window announcement on switching applications"); from the
same day's stage A step 3 until then, it kept such an early foreground
event for the next focus inside its window.

### A control's own focus with a focused child

A Win32 control taking
the focus, such as a tree view (`SysTreeView32`) or a tab control
(`SysTabControl32`), raises a focus event on itself (child id 0) and
then one on its focused child, both within the application's one
`SetFocus` call. NVDA redirects a container's own focus to its
`accFocus` child only for lists, but its event pump runs after both
events are delivered and tries focus events newest first, so the child
wins and the control is spoken only as a new ancestor, without its
shortcut: five NVDA transcripts on 2026-10-07, tabbing onto the tree on
Verbatim's Theme settings page, all had "Indications: tree view", then
the item. Verbatim's outpost could read the control before the child's
event reached it, and announced the control itself first. **Matched
since 2026-10-07, by a different route**: before reporting an MSAA focus
on an object's child id 0, the outpost asks the object's `accFocus`
once, and when that names a child by id that has the focused state,
reports the child instead, with the object as its ancestor (`mockapp`'s
`focus_reports` test and the `theme_panel` scenario).

### The shell's staging windows

On Alt+Tab, Task View, and the Windows+X
menu, File Explorer gives the focus or the foreground to windows that
only stage the switch (`ForegroundStaging`, `LauncherTipWnd`,
`ApplicationManager_DesktopShellWindow`), with no name. NVDA's File
Explorer app module leaves the focus on them unsaid
(`appModules/explorer.py` lines 443 to 449), after NVDA has made the
window its focus and run its foreground event, which cuts speech.
Verbatim: **matched since 2026-10-09** for what is said: the outpost
drops a focus or foreground fact on such a window; before, Alt+Tab
between two terminal windows was sometimes said as "pane" before the
window switched to (the two-window scenarios). **Different:** since
the focus is dropped, speech is not cut at the staging window, only at
the window switched to (roadmap, the audits' items).
