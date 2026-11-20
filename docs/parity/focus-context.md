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
containers entered. The walk's first entry, the focus itself, is removed
from the line, so the focus is never among its own ancestors (`api.py`
148 to 149). NVDA judges a UI Automation element the same by its
identity alone (`CompareElements`), so a container replaced by a new
element with the same role and name is entered anew: File Explorer's
items view list after Enter opens a folder or Backspace goes up, where
NVDA says "Items View list" before the item (10 of 10 captures,
alpha-58022); and two MSAA objects are never the same when their windows
differ. Verbatim chooses the meeting point by one identity rule: two entries
are the same object exactly when they are the same node, or both are the
same window's object (a UI Automation element that is its window's own,
or an MSAA window or client object) read through one backend, whichever
outposts read them, as NVDA's `NVDAObject.__eq__` never equals objects
of two types (`NVDAObjects/__init__.py` 397 to 403; Dickson,
2026-11-07). A window above the process boundary, described from
Windows' state with no call into the process that serves it, is a
stand-in, the same object as any entry with its handle: NVDA reads that
window through its own API, so both of its ancestries hold the same
object there and meet, and the stand-in lets Verbatim's meet there too
(Core's `a_uia_owner_window_above_another_process_meets_its_stand_in`;
`verbatim-model`'s
`two_windows_of_one_handle_and_two_backends_are_two_objects` and
`a_stand_in_is_the_same_object_as_its_window_through_either_backend`;
the scenario `owned_dialog_across_processes`, from NVDA's capture). An outpost gives one
UI Automation element one node, and one outpost's MSAA objects are the
same exactly when NVDA's equality holds, in its order: the same child
id; the same event window, event object id and event child id, each
where both are known; the same window; the same identity string; the
same role; the same location, the old object's read live; and the same
name, the old object's read live when it differs. So a container moved
or renamed between two focuses is the same, and one destroyed and
recreated in its place is not. Nodes from two outposts are the same
only as windows of one backend or a stand-in ("A window and its content in two processes" in
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
leaves the containers the focus is in, and the next focus in them says
only itself. A repeat that NVDA's duplicate test catches against the
readers' last focus ends with no read, and nothing is kept or said
(`UIAHandler/__init__.py` 917; `IAccessibleHandler/__init__.py` 777). A
child window's client object reached from its window
object is its window's, whether or not it has an identity string, so it
carries its window. Each report's chain reaches an ancestor Core already
knows or the desktop: a
failed walk step goes on from the parent window by its own object, read
live, and only a chain cut at the depth limit is completed by plain
windows ("A focus whose element is gone before its read" in [Focus
events](focus-events.md)). A window matched by its handle keeps, in the
line, the name it was entered with.

The ancestors Core already knows (Dickson, 2026-11-13; [ADR 0003:
Readers tell, Core
asks](../../books/dev/src/reference/adr/0003-readers-tell-core-asks.md),
"The ancestors Core already knows and leaving a window"): a focus
reader, the listener's reading thread for an application or the
application's outpost, walks up from the focus until it reaches one of
the ancestors Core already knows, the committed focus first and then its
ancestors, or else the process boundary or the top. Core sends them with
the request only while the readers' last focus is set, and only the
nodes of the answering outpost generation. No earlier ancestry is kept
per application, so nothing above the meeting point is reused and no
stale window title can be spoken; a return from another application walks
to the top live. NVDA walks the same way, from the focus until it meets
the ancestors of its own focus (`api.setFocusObject`, `api.py` 98 to 101
and 111 to 147). `crates/verbatim-core/src/entries_tests.rs` and
`crates/verbatim-core/src/focus/versions_tests.rs`, mockapp's
`the_walk_stops_at_a_known_ancestor_or_the_depth_limit`, and the
scenarios `windows_terminal_two_windows` (a window retitled while the
user is away, from NVDA's capture) and `measure_focus_return`.

Speech is cut when the two lines differ just below the desktop, sharing
no entry, NVDA's focus difference level of one or less, which runs its
foreground event and cancels speech (`eventHandler.doPreGainFocus`); not
when they meet below it. The readers leave out of a chain entries
that NVDA's line holds: the UI Automation reader every entry that is not
presentable, among them an unnamed top-level window and every layout
container, and the MSAA reader its window objects only, so a chain can
be empty, and the focus's top-level window,
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
unavailable (`_get_presentationType`). The state is spoken for a query
alone, Insert+Tab and report navigator object, on every object that
carries it, and for no other reason, as NVDA's `processAndLabelStates`
keeps it only for a query (`controlTypes/processAndLabelStates.py` 62 to
68; Dickson, 2026-11-19). A window that exists, read from Windows' own
state, carries no invisible state from its style (Fable, 2026-10-30), so a
window not yet shown keeps its title, as NVDA, reading the window through
its own API, says it. The plain window of a ghost destroyed while it is
the focus does carry it, as NVDA's plain `Window` of the dead handle, its
style 0, does (`NVDAObjects/window/__init__.py` 299 to 306), so Insert+Tab
then says "window invisible" (Dickson, 2026-11-19). UI Automation's
`IsOffscreen` is NVDA's offscreen state, not invisible, and does not
hide an ancestor. **partial**: reducer tests,
mockapp's `an_msaa_ancestor_reached_with_no_identity_string_is_its_windows_client`
and `a_uia_window_in_a_win32_window_has_the_windows_client_as_context`
(a hidden window's client area, invisible), and `explorer_folder_window`
asserts the Enter and Backspace cases. The scenarios do not yet show the
Windows Forms dialogs' speech.

### A dialog's text

A dialog entered as the focus's ancestor is said with its text, NVDA's
`Dialog.getDialogText` (`NVDAObjects/behaviors.py` 93 to 194), gathered
from its visible static children. NVDA never waits for a dialog to be
shown: it stopped ignoring focus events on invisible windows (b80983743,
2009: "a window gets focus before it is made visible", the NVDA exit
dialog and the Run dialog) and foreground events on an invisible window
(e01bee3c5, 2008). It reads a dialog focused while hidden when its pass
reaches the events, and its text depends on whether that read comes
after the show: for Verbatim's Import a Theme dialog, NVDA's busy pass
read it about 1 s after Enter and said "Import a Theme  dialog  File
name:", "File name:  combo box  collapsed" (5 of 5 captures at the pin).

Verbatim: a dialog's text is read only once it is shown (Dickson,
2026-11-19). A focus read whose top-level window is a hidden dialog is
answered with that window, Core waits for its show and reads the focus
again, so the dialog is said with its text, then its field, as NVDA's
capture says them. No focus or foreground event is ignored, so both NVDA
commits above hold; only the read is postponed, by the time left until
the show. Covered by the 2026-11-19 ruling, as differences from NVDA:

- the user elsewhere before the show (the foreground, a menu or a focus
  outside the dialog) says nothing for the hidden dialog. Derived from
  the source, not captured: NVDA reads the newest valid focus or
  foreground of each pass (`IAccessibleHandler/__init__.py` 1061 to
  1092), so it says the dialog without its text when its pass reads the
  dialog before the ending event arrives, and nothing for it when both
  fall in one pass;
- a key pressed before the show reads the dialog at once, without its
  text, as NVDA would read it then;
- a dialog never shown, with no key and no other ending, holds the queue
  for the 13 s due time and ends unspoken, where NVDA speaks its focus.

Save As and Run are not yet captured with NVDA, so whether NVDA says
their text is not known; their expectations wait for those captures.

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
process boundary or past the depth limit, is described as NVDA's MSAA
client for it: a pane, marked the same way when its style has a system
menu, so a top-level window is spoken by its title and a child window
between it and the focus's own window is structure; a ghost's or a hung
application's window stays a plain window, role window, as NVDA's object
for it is. Different: NVDA reads a window above the process boundary
through its own API, so a host it reads through UI Automation is said
"<title> window" where Verbatim's stand-in says the title alone
([ADR 0001: Focus](../../books/dev/src/reference/adr/0001-focus.md),
"Object identity"). A window whose element a walk cut at the depth limit had already
read is not added again: plain windows start from its parent window. A client object reached through `accParent` with no
identity string, as a Windows Forms form's, is not marked as it is
reached: NVDA knows a client's object id only from its event or its
identity string (`NVDAObjects/IAccessible/__init__.py` 803 to 808), and
presents it as a focus ancestor by its title only then. NVDA still says
the form's title on entering its window: when the focus line changes
just below the desktop, `doPreGainFocus` speaks the foreground object,
the window's client read at its own address, its object id known
(`eventHandler.py` 402 to 413, `NVDAObjects/IAccessible/__init__.py` 564
to 578), whether or not a foreground event was read. So an MSAA walk
that reaches a top-level window's client area from a child window's
window object, which by MSAA's structure is that window's client, acquires
it at its own address and reads it there, as NVDA reads its foreground
object: it is then the same node as the window's foreground report gives,
and is marked by the rule above. The outpost does not judge whether the
window is new, and Core speaks it only as an entered container, so a focus
that stays in the window says nothing more (Dickson's decision,
2026-10-30, after `text_box_editing` lost the form's title when its focus
was read before its foreground change). The mark keeps NVDA's
presentability rule, the system menu and the client role, which is what
says a top-level window by its title in the ancestry; NVDA's foreground
speech has neither condition, so a top-level window without a system menu
entered this way is not said, where NVDA says it. The cost is one
acquisition. A client area reached through `accParent` from an object
inside the window rather than from a child window's window object is not
known to be the client, and is read as reached. There is no separate foreground announcement.
**matched (unverified)**, but for the stand-in's words: reducer tests for the marked pane and for a
pane without the mark, mockapp's
`a_window_above_the_process_boundary_is_a_plain_window_with_no_call`
for the windows above the boundary, and
`an_msaa_ancestor_reached_with_no_identity_string_is_its_windows_client`
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

Verbatim: the listener tells each foreground change to Core, where it is
tried as a whole-window focus in its run of MSAA focus and foreground
entries. Core refuses it with no read when its window is not the
foreground record's, when it is the window of the readers' last focus or
of one of that focus's ancestors, or when its ids equal that focus's
(design of build 1 of ADR 0003, "Focus and foreground"). Window handles
are compared rather than node ids because one window has different node
ids in different outposts (a Settings page whose frame belongs to
`ApplicationFrameHost.exe`). A window that has no name when focus enters
it is not announced later, and a whole-window focus of a nameless window
is taken silently (a bare "window" says nothing), though it cancels
speech, as NVDA's foreground event does. A candidate whose window is
read through UI Automation is skipped when its outpost reads the run, and
the next older candidate tried, as NVDA's `processFocusWinEvent` returns
false for it (`IAccessibleHandler/__init__.py` 577 to 582, 1088 to
1092). Core, when the focus
line shares nothing with the previous one below the desktop, cuts speech
off and speaks the entered presentable ancestors outermost first, the
top-level window among them by the rule in "Top-level windows in the
ancestry", then the focus ([How focus
works](../../books/dev/src/explanation/design/focus.md), "One UI
Automation focus, end to end").

Leaving a window (Dickson, 2026-11-13; [ADR 0003: Readers tell, Core
asks](../../books/dev/src/reference/adr/0003-readers-tell-core-asks.md),
"The ancestors Core already knows and leaving a window"): when the
foreground leaves for a top-level window unrelated to the committed
focus, neither that focus's top-level window nor a window with its root
owner, the readers' last focus is emptied, in arrival order; the
committed focus stays for commands and typed echo. With no readers' last
focus, the duplicate tests and the refusal of a foreground change behave
as with no earlier focus, and no ancestors are sent as known, so the
return to an application that committed no focus while the user was away
meets nothing of the line it left and is spoken as the switch is, the
window's title and then the focus. Different, deliberately (Dickson,
2026-11-08): NVDA keeps its last queued focus across a foreground change,
so when no other focus was queued while the user was away it judges the
return's focus against the focus it left. A window owned by
the focus's window is related, so the readers' last focus stays and Core
compares the two lines as they are, as NVDA compares its focus line with
the one before; and the title rule
below adds no title when the two lines meet at an object they share, as
NVDA speaks the window in front only when its focus difference level is
at most 1, the two lines differing just below the desktop
(`eventHandler.doPreGainFocus`, `eventHandler.py` 402 to 413). So a
dialog that another process opens over a window it owns is entered with
no owner title, its outpost's stand-in for the owner meeting the owner's
entry, and the return to the owner says only its focus, as NVDA's
capture says (the scenario `owned_dialog_across_processes`; Core's
`a_window_owned_across_processes_meets_its_owner_both_ways`). NVDA's old
focus line keeps its objects whatever becomes of their applications
(`api.setFocusObject`, `api.py` 98 to 147), so when the dialog's
application exits as the focus returns, its line's ancestors that another
application's outpost read, the owner's window among them, stay known,
and only what the ended outpost read is dropped (Core's
`a_return_to_the_owner_as_the_dialogs_application_exits_says_its_focus`).
A walk that met an ancestor the line no longer holds, as after a desktop
switch, is not completed against it: the application's focus is read
afresh.

The title rule (Dickson, 2026-10-30): the window's title is always said
when the focus enters a top-level window it was not in, decided in Core
from the focus's chain. A focus Core takes whose own top-level window is
the system's foreground window when it was read
(`WindowFacts::foreground_window`), that window not the entered window
or the readers' last focus empty, says the window's own entry in its
chain, the outermost with that window's handle, with the words of a
whole-window focus of the window, after the cut and before the other
entered containers, unless its line already speaks it, it has no name,
or the two lines meet at an object they share; a whole-window focus
taken first enters its window, so the focus after it says nothing more. So the title does not depend on whether a
foreground change and its focus were tried in one run, which NVDA's
own result does by pump timing. Program Manager and the taskbar are
entered with no title (`WindowFacts::foreground_is_shell`), as NVDA
takes no foreground event for them (`internalWinEventHandler.py`), so
the desktop says "Desktop list" alone. A foreground change into one of
Verbatim's own windows, its menu or a dialog, is like any other: the
window is unrelated to the application's focus, so the readers' last
focus is emptied and the return from it says the title. Core's
`tests/foreground.rs`.

- Different, deliberately: a Windows Forms form or dialog brought to the
  front with its focus within NVDA's 10 ms pump says its title, where
  NVDA says none (`two-same-named-dialogs.txt` and
  `classic-context-menu.txt` in
  `experiments/focus-pipeline-rework-measurements/captures/`); so does a
  Windows Forms main window when a dialog closes.
- Different, deliberately: a focus reported before its window's
  foreground change arrives, its window already in front, says the title
  with the focus, where NVDA drops the later foreground change as a
  repeat.
- Matched (needs a capture; untestable until M8.7): the return to the
  user's desktop after the secure desktop says the window's title and
  the focus. NVDA's sleeping focus object has no parent
  (`winAPI/secureDesktop.py` 56 to 70), so the focus it queues 200 ms
  after the return meets nothing of it and enters its window anew
  (`IAccessibleHandler/__init__.py` 809 to 813; `api.py` 98 to 150;
  `eventHandler.py` 402 to 419). Verbatim's sleeping focus has no
  ancestors either, and the read after the return is sent none as known.
- Different, known: on the first Escape from Windows 11 Notepad's WinUI
  context menu, NVDA says "Untitled - Notepad"
  (`keyboard-opened-context-menu.txt` in the same folder). The menu is a
  popup in Notepad's foreground, so the title rule adds nothing; Verbatim
  says the title only when Notepad's whole-window focus is tried in its
  run.
- A focus raised before its window's foreground change within one pump
  is no departure: the foreground change, newest, is tried first, and its
  read ends the run, so Verbatim, like NVDA, says the window
  and not that focus.
- Matched (Dickson, 2026-11-07): an application that does not answer
  says nothing until its focus comes, as NVDA
  ([Focus events](focus-events.md), "A focus UIA does not deliver").

**matched (unverified)** for the rest: reducer tests, and `mockapp`
tests that an early foreground event and a UI Automation window's
foreground event are dropped; the Start menu, window-switch, `conhost_`
and `explorer_folder_window` scenarios do not yet verify it live, and
what NVDA says when a console window comes in front some 400 ms after
its text area's focus is not captured.

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
2026-10-30). **done**: reducer tests and
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
a focus query's answer with a focused control is that control's focus
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
