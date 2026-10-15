# Focus context

The ancestry spoken with a focus: entered containers, top-level windows, the window announced on switching applications, a File Explorer folder window, and the desktop.

### Focus-ancestry context

announce newly entered presentable
containers before the control. NVDA: `focusEntered` +
`isPresentableFocusAncestor` ([Event handling](../nvda/events.md),
[Object model](../nvda/object-model.md)). Verbatim: **matched (unverified)** —
filter claimed at parity with `_get_isPresentableFocusAncestor`
(exclusion-based, same role exclusions). Since 2026-10-03 an entered
container is spoken as NVDA speaks focus entered: as a focus, states and
position included, without its value, level, or (except a list) keyboard
shortcut; it had been spoken with only its name, role, and description.
Except:

### Top-level windows in the ancestry

NVDA presents an entered window
like any other container when it has a name or a description, and
treats an unnamed window as layout ([Event handling](../nvda/events.md),
"Foreground windows"). A window read through UI Automation is content,
and so presented, only when UI Automation counts its element both a
control and content, as every UI Automation ancestor must be; it is
spoken as its title and "window". A window read through MSAA is
reached as its client area, which reads as a pane, a role that is
otherwise layout: NVDA presents the client area of a window with a
system menu (the `WS_SYSMENU` style) all the same, whatever its name,
and since a pane's role is silent on focus it is spoken by its title
alone. Verbatim: the window is one of the roles presentable only when
named or described, as in NVDA; a UI Automation ancestor is reported
only when it is a control and content; and since 2026-10-11 the
outposts mark the MSAA client area of a window with a system menu
(`NodeDetails::top_level_client`), which the reducer presents although
it is a pane, so it is spoken by its title. There is no separate
foreground announcement. **matched (unverified)**: reducer tests for
the marked pane and for a pane without the mark.

### Window announcement on switching applications

NVDA: a foreground
event is a focus on the window; it is ignored when the latest focus
is already in that window, and the window is otherwise announced as
the focus. A window that is an ancestor of the new focus is spoken
as an entered container. Nothing speaks a window just because it
became the foreground ([Event handling](../nvda/events.md), "Foreground
windows"). Verbatim: a foreground fact arrives as a focus on the
window, and the reducer ignores it when its window handle equals the
current focus's top-level window handle. Window handles are compared
rather than node ids because
one window has different node ids in different outposts (a Settings
page whose frame belongs to `ApplicationFrameHost.exe`), and an
ancestor walk that timed out leaves no ancestors to compare. A
window that has no name when focus enters it is not announced
later, and a foreground change to a nameless window moves
attention without speaking (a bare "window" says nothing), though it
cancels speech, as NVDA's foreground event does. NVDA has no rule of
its own for announcing a window otherwise: a window is spoken either
as the object a foreground event focuses, or as a newly entered
ancestor of a focus. When the top of the focus's ancestry changes,
NVDA cancels speech and speaks the entered presentable ancestors,
outermost first, then the focus, whether or not a foreground event
came (`eventHandler.doPreGainFocus`). A foreground event whose window
is not the foreground window when it is handled is dropped, never
kept, and every MSAA event for a window NVDA reads through UI
Automation is dropped, a foreground event included, so such windows,
the console host's and Windows Terminal's among them, are announced
only from the focus's ancestry. Verbatim, since 2026-10-11 (approved
by Dickson; `docs/design/focus-pipeline.md`, section 16.4): the
outposts drop a foreground fact whose window is not the foreground
window when it is handled, and one for a window read through UI
Automation; the reducer, when the focus's top-level window changes,
cuts speech off and speaks the entered presentable ancestors outermost
first, the top-level window among them by the rule in "Top-level
windows in the ancestry" above, then the focus. It replaces a special
case that spoke the outermost ancestor, whatever its role, for a focus
in a new foreground window that no foreground fact had announced
(found live on 2026-10-06 in a File Explorer folder window, whose
foreground event Windows raised while still refusing it the
foreground and did not raise again; the window's client area, a pane,
is now presented by the rule above). **matched (unverified)**: reducer
tests, and `mockapp` tests that an early foreground event and a UI
Automation window's foreground event are dropped; the Start menu,
window-switch, `conhost_` and `explorer_folder_window` scenarios are to
verify it live, and what NVDA says when a console window comes in
front some 400 ms after its text area's focus is still to be captured.

### A File Explorer folder window opening

A File Explorer folder window opening, as `explorer_folder_window`
opens it (launched without the foreground right, then brought forward).
NVDA alpha-58022, captured 29 times so (2026-10-12), says the window's
title, "Items View list", and the first item in 27, and "pane" before
"Items View list" in 2. Its "pane" comes from MSAA focus events on the
items view's window that File Explorer raises while the window is
still behind: NVDA queues them and, when it processes them only after
the window has come in front, they pass its acceptance test. It is
NVDA's own delay racing the window's activation, not a rule; the older
alpha-57645 said "pane" in nine of ten, since its event processing
was slower. Verbatim judges an MSAA focus as it is handled, when the
window is still behind, drops those events, and says the title, "Items
View list", and the first item. **matched** in what is said in the
usual case; **different** in NVDA's rare "pane" (Dickson, 2026-10-12,
keeps Verbatim's deterministic result, and the scenario asserts it).
NVDA also says the folder's tab, "<folder> tab selected 1 of 1", after
the item in 16 of the 29: File Explorer's app module speaks a tab of
the tab list (automation id "TabListView") when its element-selected
event comes with no second one pending (`appModules/explorer.py`,
`event_UIA_elementSelected`, NVDA #14388), which only some openings
meet. **Not yet:** that rule belongs to the app-module porting track.
Verbatim said "pane" in place of "Items View list", in 2 of 29
openings: its focus listener took a UIA focus event on the items
view's own window element, raised just before the first item's, and
read it before its content was ready, as an unnamed pane; the item's
focus then had that element as its previous focus, not an entered
container. NVDA received no such event in 30 openings. To be
investigated before `explorer_folder_window` can be counted on.

### The desktop's announcement

NVDA, starting with the desktop focused,
and after Escape from Verbatim's settings dialog back to the desktop,
says "Desktop list" and the focused icon, such as "NVDA 3 of 12", and
never "Program Manager", the desktop window's title (captures of
2026-10-11, NVDA alpha-57645, on this project's development machine,
and again in every capture of 2026-10-12 with alpha-58022, the build
the pinned source is; Dickson confirms it never says it). Starting, NVDA announces the
system's focus with its newly entered presentable ancestors, and
Program Manager's client area, a pane with no system menu, is not one;
NVDA also drops every foreground event for the Program Manager and the
taskbar windows, by their window classes (`Progman` and
`Shell_TrayWnd`, `IAccessibleHandler/internalWinEventHandler.py` 165 to
166). Verbatim: since 2026-10-11 a focus-now answer with a focused
control is that control's focus alone, the foreground window announced
only as an entered ancestor when it is presentable, so starting with
the desktop says "Desktop list" and the icon; before, the answer's
window was reported first as a foreground change and spoken, "Program
Manager". **Different** on returning to the desktop: a foreground fact
for the desktop window that reaches Core before the list item's focus
is still spoken as the focus, "Program Manager", as Verbatim has not
ported NVDA's class rule for foreground events; the harness expects
NVDA's speech there (`close_settings_to_desktop`).
