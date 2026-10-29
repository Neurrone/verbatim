# Focus context

The ancestry spoken with a focus: entered containers, top-level windows, the window announced on switching applications, a File Explorer folder window, and the desktop.

### Focus-ancestry context

Announce newly entered presentable containers before the control. NVDA:
`focusEntered` + `isPresentableFocusAncestor`
([Event handling](../nvda/events.md),
[Object model](../nvda/object-model.md)). Verbatim: the filter follows
`_get_isPresentableFocusAncestor` (exclusion-based, same role
exclusions, the border and white space roles among the layout ones,
named or not), and an entered container is spoken as NVDA speaks focus
entered: as a focus, states and position included, without its value,
level, or (except a list) keyboard shortcut. Top-level windows are the
exception, in the next entry. A container is already entered when it is
the same object as one in the previous focus's chain, which NVDA judges
for a UI Automation element by its identity alone (`CompareElements`),
so a container replaced by a new element with the same role and name is
entered anew: File Explorer's items view list after Enter opens a folder
or Backspace goes up, where NVDA says "Items View list" before the item
(10 of 10 captures, alpha-58022). Verbatim compares two UI Automation
nodes from one outpost by node id alone, the outpost giving one element
one id; it treats nodes from two outposts, or read through MSAA, in the
same top-level window as the same when their role and name match, since
their ids cannot be compared ("A window and its content in two
processes" in [Windows and processes](windows-and-processes.md)). Two
nodes that are both windows (a window read from the system's own data,
a UI Automation element that is its window's own, or an MSAA window or
client object) are the same exactly when they are the same window,
whichever outposts read them: the Settings frame its own process reports
as its focus and the page's process as a plain window above the page is
entered once, so NVDA's "Settings", then "Settings window" for the page,
is said without the frame again; and two windows of one role and name are
two containers.
An invisible MSAA ancestor is still announced, where NVDA counts it
unavailable, since the model has no invisible state and the outposts do
not report it. **partial**: reducer tests, and `explorer_folder_window`
asserts the Enter and Backspace cases.

### Top-level windows in the ancestry

NVDA presents an entered window like any other container when it has a
name or a description, and treats an unnamed window as layout; the
client area of an MSAA window with a system menu is presented although
it is a pane, and spoken by its title alone; a UI Automation window is
presented only when UI Automation counts it both a control and content
([Event handling](../nvda/events.md), "Foreground windows"). Verbatim:
the window is one of the roles presentable only when named or described,
as in NVDA; a UI Automation ancestor is reported only when it is a
control and content; and the outposts mark the MSAA client area of a
window with a system menu (`NodeDetails::top_level_client`), which the
reducer presents although it is a pane, so it is spoken by its title.
A window above the process boundary, which the outpost reads with no
call into its process, is described as NVDA's MSAA client for it: a
pane, marked the same way when its style has a system menu, so a host's
top-level window is spoken by its title and a child window between it
and the focus's own window is structure; a ghost's or a hung
application's window there stays a plain window, role window, as NVDA's
object for it is (`docs/design/focus-pipeline-rework.md`, "The
foreground wait"). There is no separate foreground announcement.
**matched (unverified)**: reducer tests for the marked pane and for a
pane without the mark, and mockapp's
`a_window_above_the_process_boundary_is_a_plain_window_with_no_call`
for the windows above the boundary.

### Window announcement on switching applications

NVDA: a foreground event is a focus on the window; it is ignored when
the latest focus is already in that window, and the window is otherwise
announced as the focus. A window that is an ancestor of the new focus is
spoken as an entered container. Nothing speaks a window just because it
became the foreground. When the top of the focus's ancestry changes,
NVDA cancels speech and speaks the entered presentable ancestors,
outermost first, then the focus, whether or not a foreground event came
(`eventHandler.doPreGainFocus`). A foreground event whose window is not
the foreground window when it is handled is dropped, and every MSAA
event for a window NVDA reads through UI Automation is dropped, so such
windows are announced only from the focus's ancestry
([Event handling](../nvda/events.md), "Foreground windows").

Verbatim: a foreground fact arrives as a focus on the window, and the
reducer ignores it when its window handle equals the current focus's
top-level window handle. Window handles are compared rather than node
ids because one window has different node ids in different outposts (a
Settings page whose frame belongs to `ApplicationFrameHost.exe`), and an
ancestor walk that timed out leaves no ancestors to compare. A window
that has no name when focus enters it is not announced later, and a
foreground change to a nameless window moves attention without speaking
(a bare "window" says nothing), though it cancels speech, as NVDA's
foreground event does. The outposts drop a foreground fact whose window
is not the foreground window when it is handled, and one for a window
read through UI Automation; the reducer, when the focus's top-level
window changes, cuts speech off and speaks the entered presentable
ancestors outermost first, the top-level window among them by the rule
in "Top-level windows in the ancestry", then the focus
(`docs/design/focus-pipeline.md`, section 16.4). **matched
(unverified)**: reducer tests, and `mockapp` tests that an early
foreground event and a UI Automation window's foreground event are
dropped; the Start menu, window-switch, `conhost_` and
`explorer_folder_window` scenarios do not yet verify it live, and what
NVDA says when a console window comes in front some 400 ms after its
text area's focus is not captured.

### A File Explorer folder window opening

A File Explorer folder window opening, as `explorer_folder_window`
opens it (launched without the foreground right, then brought forward).
NVDA says the window's title, "Items View list", and the first item in
most openings (27 of 29 captures with alpha-58022), and "pane" before
"Items View list" in the rest. Its "pane" comes from MSAA focus events
on the items view's window that File Explorer raises while the window is
still behind: NVDA queues them and, when it processes them only after
the window has come in front, they pass its acceptance test. It is
NVDA's own delay racing the window's activation, not a rule. NVDA also
says the folder's tab, "<folder> tab selected 1 of 1", after the item in
some openings (16 of the 29), by a rule of File Explorer's app module
([Event handling](../nvda/events.md), "Foreground windows").

Verbatim judges an MSAA focus as it is handled, when the window is still
behind, drops those events, and says the title, "Items View list", and
the first item. **partial**: the usual case matches, and
`explorer_folder_window` asserts it; NVDA's rare "pane" is deliberately
not said (Dickson, 2026-10-12), the scenario asserting Verbatim's
deterministic result; the tab rule is not yet built, belonging to the
app-module porting track. Verbatim says "pane" in place of "Items View
list" in a few openings (2 of 29): its focus listener takes a UIA focus event on the
items view's own window element, raised just before the first item's,
and reads it before its content is ready, as an unnamed pane; the item's
focus then has that element as its previous focus, not an entered
container. NVDA received no such event in 30 openings. This is not yet
investigated, so `explorer_folder_window` cannot yet be counted on.

### The desktop's announcement

NVDA, starting with the desktop focused, and after Escape from
Verbatim's settings dialog back to the desktop, says "Desktop list" and
the focused icon, such as "NVDA 3 of 12", and never "Program Manager",
the desktop window's title: starting, it announces the system's focus
with its newly entered presentable ancestors, and Program Manager's
client area is not one, and it drops every foreground event for the
Program Manager and the taskbar windows by their window classes
([Event handling](../nvda/events.md), "Foreground windows"). Verbatim:
a focus-now answer with a focused control is that control's focus
alone, the foreground window announced only as an entered ancestor when
it is presentable, so starting with the desktop says "Desktop list" and
the icon. **partial**: starting matches; returning to the desktop does
not, since a foreground fact for the desktop window that reaches
Core before the list item's focus is still spoken as the focus,
"Program Manager", as Verbatim has not ported NVDA's class rule for
foreground events; the harness expects NVDA's speech there
(`close_settings_to_desktop`).

When the desktop takes focus, as when windows are minimized, Verbatim
announces the desktop's nameless window of class `WorkerW` as "pane";
NVDA's File Explorer app module ignores that window's focus events.
That is not yet ported (unscheduled), with the app-module porting
track's Tier A.
