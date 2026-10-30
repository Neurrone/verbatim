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
exception, in the next entry; a slider that focuses its thumb is
another, in "A slider that focuses its thumb".

The containers entered are found as NVDA builds its focus line when the
focus moves (`api.setFocusObject`): the new focus's chain is walked from
the focus outward, and at each entry the previous line, the previous
focus and its ancestors, from its focus outward; the first entry that is
the same object as one of the previous line is where the two lines meet.
The new line is the previous one up to that entry, its old objects kept,
then the new chain's entries below it, and those entries are the
containers entered. NVDA judges a UI Automation element the same by its
identity alone (`CompareElements`), so a container replaced by a new
element with the same role and name is entered anew: File Explorer's
items view list after Enter opens a folder or Backspace goes up, where
NVDA says "Items View list" before the item (10 of 10 captures,
alpha-58022); and two MSAA objects are never the same when their windows
differ. Verbatim chooses the meeting point by one identity rule: two entries
are the same object exactly when they are the same node, or both are
windows (a window read from the system's own data, a UI Automation
element that is its window's own, or an MSAA window or client object)
and the same window, whichever outposts read them. An outpost gives one
UI Automation element one node, and one outpost's MSAA objects are the
same exactly when NVDA's equality holds, in its order: the same child
id; the same event window, event object id and event child id, each
where both are known; the same window; the same identity string; the
same role; the same location, the old object's read live; and the same
name, the old object's read live when it differs. So a container moved
or renamed between two focuses is the same, and one destroyed and
recreated in its place is not. Nodes from two outposts are the same
only as windows ("A window and its content in two processes" in
[Windows and processes](windows-and-processes.md)). Nothing is matched
by role and name: in a web page whose document carries its window, a
link in a "Search" group's unnamed list, after a link in another unnamed
list of the same document, says "Search grouping", "list", "b link", as
NVDA does.
The Settings frame its own process reports as its focus and the
page's process as a pane above the page is entered once, so NVDA's
"Settings", then "Settings window" for the page, is said without the
frame again; and two windows of one role and name are two containers, as
two Windows Forms panels whose client objects are dialogs named "Remove",
each said "Remove dialog" as the focus moves into it
(`experiments/focus-pipeline-rework-measurements/captures/two-same-named-dialogs.txt`).
A focus reported again, the same as the focus, does not replace the
line: NVDA keeps its line when the focus matches itself, so a repeat
whose chain was cut at its bound leaves the containers the focus is in,
and the next focus in them says only itself; the repeat's newer reading
of the focus and of the line's nodes it holds is kept (Fable, 2026-10-30, pending Dickson's acceptance).
A child window's client object reached from its window object is its
window's, whether or not it has an identity string, so it carries its
window. Each report's chain is complete to the desktop, windows a walk
did not reach being plain windows, so a chain cut short in the same
window meets the previous line at that window, enters nothing, and the
next focus read in full enters again what the cut chain did not reach.
A window matched by its handle keeps, in the line, the name it was
entered with.

Speech is cut when the two lines differ just below the desktop, sharing
no entry, NVDA's focus difference level of one or less, which runs its
foreground event and cancels speech (`eventHandler.doPreGainFocus`); not
when they meet below it. The readers leave out of a chain the entries
that are not presentable, which NVDA's line holds, among them an unnamed
UI Automation top-level window, an MSAA window object and every layout
container, so a chain can be empty, and the focus's top-level window,
which its window facts name, stands for the entry just below the
desktop: two chains in one top-level window that share no entry meet
there, empty chains or not, and speech is not cut, except for a focus whose
containers a rule drops, whose container is the desktop, as NVDA's task
switcher item's is with Alt held. The first focus Verbatim takes cuts nothing, as
NVDA, starting, makes the focus its focus object before it handles it. A
combo box's drop-down opened by Alt+Down says "expanded" and cuts
nothing: the focus stays on the combo box, so no chain is compared.

An invisible ancestor, MSAA's `STATE_SYSTEM_INVISIBLE` on an object read
through MSAA, is not presented, as NVDA counts an invisible object
unavailable (`_get_presentationType`); the state is never spoken. A
window read from Windows' own state carries no invisible state from its
style (Fable, 2026-10-30, pending Dickson's acceptance): a Windows Terminal window not yet shown when
its focus is published at its bound would lose its title for good, where
NVDA reads it through UI Automation and says it. UI Automation's
`IsOffscreen` is NVDA's offscreen state, not invisible, and does not
hide an ancestor. **partial**: reducer tests,
mockapp's `an_msaa_ancestor_reached_with_no_identity_string_is_its_windows_client`
and `a_uia_window_in_a_win32_window_has_the_windows_client_as_context`
(a hidden window's client area, invisible), and `explorer_folder_window`
asserts the Enter and Backspace cases. The scenarios do not yet show the
Windows Forms dialogs' speech.

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
A window the outpost reads with no call into its process, above the
process boundary or past a walk's bound, is described as NVDA's MSAA
client for it: a pane, marked the same way when its style has a system
menu, so a top-level window is spoken by its title and a child window
between it and the focus's own window is structure; a ghost's or a hung
application's window stays a plain window, role window, as NVDA's object
for it is (`docs/design/focus-pipeline-rework.md`, "The foreground
wait"). Past a walk's bound this is (Fable, 2026-10-30, pending Dickson's acceptance): File Explorer's
folder window published at its bound said its title with "window",
another "window" and "ShellView window", where the walk read in full
says the title once. A window whose element the cut walk had already
read is not added again: plain windows start from its parent window. A client object reached through `accParent` with no
identity string, as a Windows Forms form's, is not marked as it is
reached: NVDA knows a client's object id only from its event or its
identity string (`NVDAObjects/IAccessible/__init__.py` 803 to 808), and
presents it as a focus ancestor by its title only then. NVDA still says
the form's title on entering its window: when the focus line changes
just below the desktop, `doPreGainFocus` speaks the foreground object,
the window's client read at its own address, its object id known
(`eventHandler.py` 402 to 413, `NVDAObjects/IAccessible/__init__.py` 564
to 578), whether or not a foreground event was read. So an MSAA walk that reaches a top-level window with a system menu,
holding its client area read as a pane at an address it made up, reads
that client area again at its own address, marked; the outpost does not
judge whether the window is new, and Core speaks it only as an entered
container, so a focus that stays in the window says nothing more
(Dickson's decision, 2026-10-30, after `text_box_editing` lost the form's
title when its focus was read before its foreground change). The cost is one acquisition and six reads for a
walk that reaches the top; a walk that meets the previous line below it
pays nothing. There is no separate foreground announcement.
**matched (unverified)**: reducer tests for the marked pane and for a
pane without the mark, mockapp's
`a_window_above_the_process_boundary_is_a_plain_window_with_no_call`
for the windows above the boundary, and
`a_window_entered_by_a_focus_beside_its_unread_foreground_change_is_said`
for a form's client reached with no identity string.

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
Settings page whose frame belongs to `ApplicationFrameHost.exe`). A window
that has no name when focus enters it is not announced later, and a
foreground change to a nameless window moves attention without speaking
(a bare "window" says nothing), though it cancels speech, as NVDA's
foreground event does. The outposts drop a foreground fact whose window
is not the foreground window when it is handled, and one for a window
read through UI Automation; the reducer, when the focus line shares
nothing with the previous one below the desktop, cuts speech off and
speaks the entered presentable ancestors outermost first, the top-level
window among them by the rule
in "Top-level windows in the ancestry", then the focus
(`docs/design/focus-pipeline.md`, section 16.4). **matched
(unverified)**: reducer tests, and `mockapp` tests that an early
foreground event and a UI Automation window's foreground event are
dropped; the Start menu, window-switch, `conhost_` and
`explorer_folder_window` scenarios do not yet verify it live, and what
NVDA says when a console window comes in front some 400 ms after its
text area's focus is not captured.

### A slider that focuses its thumb

NVDA gives a UI Automation element of class `SensitiveSlider`, a slider
that tends to give the focus to its thumb, its own behaviour: entered as
an ancestor of the focus, it is spoken as a focus is, its value
included, whatever its presentability, rather than as an entered
container; and a change of its value is spoken, the value alone, while
it is the focus's parent, compared with what it last spoke of it
(`NVDAObjects/UIA/__init__.py` 1371 and 2708 to 2719). Verbatim: each UI
Automation ancestor carries its class name, from the walk's cache with no
further call, and Core speaks an entered `SensitiveSlider` as a focus,
and a value change of one that is the focus's innermost ancestor when
its value differs from the one it was entered with. The outpost follows
such a parent for its value changes alone while the focus is its child,
finding its element by the focus's path when it keeps none (Fable,
2026-10-30, pending Dickson's acceptance). **done**: reducer tests and
mockapp's `a_uia_ancestors_class_makes_a_sensitive_slider_speak_its_value`
and
`a_sensitive_slider_s_value_change_reaches_core_while_its_thumb_is_the_focus`.

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
