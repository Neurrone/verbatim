# Focus announcement

What a focus announcement contains and in what order: names, roles, states, values, multi-line fields, and the reading of an edit field on focus.

### Focus announcement content and property order

Focus announcement content and property order (name, role, value,
states in fixed order, description, shortcut, position, level).
NVDA: `speakObject` ordering ([Speech](../nvda/speech.md)). Verbatim:
**matched (verified)**: the per-property order is transcribed in
`verbatim-core` ([verbatim-core](../crates/verbatim-core.md)), E2E
scenarios assert the wording, and each case below is pinned by the tests
it names, or by reducer, mapping, and mockapp tests. The cases not yet
built are in "Focus announcement cases not yet built". Different: an
empty field has no placeholder to speak.

- Roles spoken: NVDA speaks no role on focus for a set of roles, among
  them list item, menu item, tree view item, pane, static text, and
  unknown, when the object has a name or value ("alpha.txt", not
  "alpha.txt list item"); Verbatim matches, including object navigation,
  which NVDA speaks with the focus reason, entered containers, which it
  speaks like a focus, and reporting the current object, which keeps the
  role ("When the role is spoken" in [Speech](../nvda/speech.md)).
- Window objects: NVDA never presents an MSAA window object it reaches
  through a control's parents, and reads a foreground window and
  window-level events through the window's client area; Verbatim
  matches.
- Explorer's file items: NVDA's `UIItem` class reports no value, so the
  name is not spoken twice; Verbatim matches.
- Levels: a tree or list item's level goes first when it differs from
  the last level put first, and last otherwise ("level 1 Hardware
  Resources collapsed 1 of 3", then "Components collapsed 2 of 3 level
  1"), for a focus, object navigation, a selection, and reporting the
  current object alike; a standard tree view's root item is "level 0",
  the raw `accValue` ("Where the level goes" in
  [Speech](../nvda/speech.md); the `system_information_tree` scenario).
- States: spoken in NVDA's fixed order (unavailable before checked, so
  "check box unavailable not checked"); "read only" only for edit fields
  and check boxes outside a query; "default" never, as NVDA has no such
  state; "submenu" dropped on a combo box and expanded/collapsed on a
  submenu item; "not selected" only for a focusable list item, tree view
  item, row, cell, header, or check box, and only on focus or a change
  on the focus; "not checked" also for any checkable item; positive
  "selected" kept on a tab and in a query; "focused" and "off screen"
  spoken in a query ("Which states are spoken, and in what order" in
  [Speech](../nvda/speech.md)).
- "multi line" for a standard or rich edit control with the
  `ES_MULTILINE` style, after "read only", as NVDA's edit control class
  says it; UIA's multi-line state is dropped, as NVDA drops it.
- A window's own MSAA object is matched by the address its identity
  string names, so a dialog whose title changes (Control+Tab to another
  settings category) is the same node and is not announced again, and
  the new title is not spoken, as in NVDA ("Held objects" in
  [focus-events.md](focus-events.md); the `settings_dialog_keys`
  scenario).
- Verbatim's menu: NVDA names the foreground window a menu opens from.
  Verbatim's hidden frame, titled "Verbatim", is shown and brought
  forward for the menu, since without NVDA's `uiAccess` privilege a
  hidden window cannot take the foreground, and Verbatim+V says
  "Verbatim", then "Context menu", as NVDA does for a menu opened from a
  foreground window, by decision (NVDA's own menu keeps its frame hidden
  and says only "NVDA menu"). The frame's own focus and foreground events stay
  suppressed, since it transits focus (the hidden-frame suppression in
  `docs/crates/verbatim-outpost.md`); the menu's opening is reported in
  the frame's window with the frame as its ancestor, and the reducer
  names a new foreground window from the top of the focus's ancestry, as
  NVDA does when no foreground event named it (every scenario that opens
  the menu asserts both).
- Values and descriptions: an edit field or document does not speak the
  whole field after every keystroke; an unchanged value is not repeated;
  a check box, radio button, link, menu item, or application never
  speaks its value; a description equal to the name is dropped ("When
  values and descriptions are spoken" in [Speech](../nvda/speech.md)).
- UIA: a selected radio button is checked, not selected; a toggleable
  list or menu item is checkable; a menu item that no pattern makes
  checkable is checkable and checked when its legacy MSAA state
  (`LegacyIAccessibleState`) has the checked bit, as NVDA 2027.1 reads
  Windows Forms menu items; password, required, invalid entry, and
  read-only states are read (`ValueIsReadOnly` and `IsDataValidForForm`
  ignoring their defaults, as NVDA reads them); a control with only a
  `RangeValue` pattern has that value, rounded, and its changes are
  followed; the access key and the accelerator key are both spoken;
  dialogs are recognized by `IsDialog` and NVDA's dialog class names;
  and an ancestor counts as context only when UIA calls it both a
  control and content.
- MSAA: protected is read; whitespace-only names and values count as
  absent; the edit field of a labelled combo box has no label of its
  own; list view and tree view items report their position ("1 of 3").
- Roles: both backends map every role NVDA's tables give a counterpart
  for in Verbatim's vocabulary, including split button, graphic,
  progress bar, scroll bar, table, row, cell, headers, data grid,
  document, title bar, tool tip, and separator; a UIA document is a
  "document", not an "edit" (Notepad's text area is "Text editor
  document"). Progress bars and title bars are never focus context. The
  MSAA roles IP address, animation, clock, drop list, dial, grip,
  indicator, border, white space, math (an equation), chart, diagram,
  column, character, sound, tree view button, drop down button grid, and
  help balloon have NVDA's English names, and so do the visited state
  (MSAA traversed), said only of a link, and the linked state, said only
  when it changes; a half-checked progress bar is a "busy indicator"
  with no half-checked state and no value spoken, as NVDA adjusts it
  (map unit tests and reducer tests).
- MSAA description changes (`EVENT_OBJECT_DESCRIPTIONCHANGE`) on the
  focus, and state changes, a selection included, on the focus's
  ancestors (reducer tests). An ancestor is recognized by the address it
  was reported at, as NVDA recognizes it
  ([MSAA and winevent handling](../nvda/msaa.md), "Object wrapping and
  identity"), so a windowless ancestor reached through `accParent`, with
  no identity string and no `IAccessible2` unique id, has no such
  address and its state change is not spoken (the `msaa_events` mockapp
  test). Its COM identity cannot serve either: oleacc hands every object
  to another process in a new wrapper of its own, so two sightings of
  one object never share an `IUnknown`, as measured against mockapp,
  whose provider keeps one object per node
  (`docs/crates/verbatim-ia2.md`, "Identity"). The ancestors are NVDA's,
  those reached through `accParent`: a tree view item's logical parents,
  which Verbatim reads among the focus's ancestors through the control's
  own messages, are not NVDA's ancestors, whose tree view items are the
  control's children, so their state changes and selections are not
  spoken (the `native_controls` mockapp test, on a real tree view, and
  the `msaa_events` mockapp test pins that a real tree view item's
  logical parent is not such an ancestor).

### Group boxes

NVDA announces the group box a control sits in when the focus enters it
("Synthesizer grouping" before the Change button), finding it by
position since a Win32 group box is the control's sibling
(`findGroupboxObject`), and takes the group box's description from a
static text right after it. Verbatim: **partial**: a control's window
object is given, as its container, the first visible group box before it
in z-order whose rectangle holds it (`native_controls` mockapp test with
real buttons; the settings dialog's Speech page end to end); the group
box's description is not yet read.

### Sliders

A standard trackbar's MSAA value is its position as a percentage of its
range. Verbatim: **matched (verified)**: each of Verbatim's settings
sliders gives its position as its value, so both screen readers read the
setting itself (the `terminal_settings_page` scenario). Every other
control on the Speech, Theme, and Terminal pages reads the same in both.

### Text on focus

NVDA reads an edit field's selection or the line at the caret through
its text interface, in place of its value ("What an object with text
says" in [Speech](../nvda/speech.md)); when its read of the selection
fails, it reads the line at the text's first position instead
(`nvda/source/speech/speech.py`, lines 905 to 923). A protected
(password) field's text is read as any field's, masked as its text
interface gives it ("Protected (password) fields" in
[Editable text and terminals](../nvda/editable-text-and-terminals.md)).

Verbatim: **matched (verified)**, through the text protocol. The name,
role, and states are spoken at once, and the line, or "selected" with
the selected text, from the focus's message; an edit field, document,
or terminal with no text interface speaks its value when the outpost
says so (`NoText`), after the rest of the announcement rather than
between the role and the states. Object navigation, to-focus, and
reporting the current object read the text the same way, whether or not
the object has the focus: the selected text, else the line at the caret,
else the first line of a control that reports no caret, and the value
when it has no text interface. The focus's line comes in the focus's own
message, read by the outpost while it handled the focus, so nothing is
spoken between the focus and its line, as NVDA says the line with the
focus.

- On Shift+Tab into Settings' search box holding "sound", NVDA says one
  utterance, "Search box, Find a setting edit sound", and Verbatim two,
  the box and then "sound", which compare mode judges the same on their
  joined text; both are cut off by the suggestion "Sound settings 1 of
  22" (`rapid-tab-nvda-1.txt` in
  `target/e2e-evidence/fixes-round-six-2026-10-21`; the scenario
  `settings_search_suggestions_reentered`).
- When the outpost cannot read the caret (`NotRead`), Verbatim speaks
  neither the line nor the value. Windows Terminal's second focus
  identity, whose text NVDA cannot read either, says "terminal" alone in
  both (`part2c-*` in `target/e2e-evidence/m42-desktop-2-2026-10-22`);
  for any other text whose caret cannot be read, NVDA's first line is a
  difference.
- A protected field is read, masked, on focus, by caret movement, and
  for Backspace, Delete, Control+Backspace and Control+Delete, as NVDA
  reads it: stars in a classic edit control, whose text Verbatim masks as
  NVDA's edit control text does, and the provider's circles in a WPF
  `PasswordBox`; a typed character is still echoed as "star"
  ([input.md](input.md)). Without symbol processing (M8) the masked text
  is spoken as its characters, where NVDA says a run of stars by its
  count and drops a line of circles: Tab into a Windows Forms password
  box holding six characters says "Password edit protected" and
  "selected ******" where NVDA says "selected 6 star", and Tab into the
  `PasswordBox` says the six circles where NVDA says no text
  ("Symbol/punctuation processing" in [speech-audio.md](speech-audio.md)).
  The Windows Forms box's Control+Backspace deletes nothing and inserts
  a DEL character, masked as a star, and is silent, where NVDA says the
  word it expected to go, as in any classic edit control
  ([text.md](text.md), "What Backspace and Delete say"). Pinned by
  `text_box_password` and `wpf_password`, from
  `winforms-password-focus.txt` and `wpf-uia-password-focus.txt` in
  `target/e2e-evidence/m42-selection-keys-2026-10-22/` and
  `winforms-password-deleting-keys.txt`,
  `wpf-password-deleting-keys.txt` and
  `wpf-password-deleting-keys-without-home.txt` in
  `target/e2e-evidence/m42-protected-fixes-2026-10-22/`.
- Reading a masked field with the review cursor or object navigation is
  matched (unverified): it takes the same reads as any field's, and
  Core's unit test
  `navigating_to_a_protected_edit_field_reads_its_masked_selection`
  covers object navigation's request, but no capture or scenario has
  either in a password field.

### Multi-column list view items

NVDA names an item of a report view (such as msinfo32's right pane) from
its column texts, "content; Header: content", with no value or
description. Verbatim: **matched (verified)**: an item of a
`SysListView32` in the report view or tiles is named by its columns in
the order shown, each but the first after its header, leaving out a
column of zero width or no text, through the control's messages with
their structures in the list view's process, and no list view item has a
value or description (`native_controls` mockapp test, a real list view;
no end-to-end scenario covers it).

### Focus announcement cases not yet built

Verbatim: **not yet (unscheduled)**, but for the display model's case,
which is M14.

- In Windows 11 Notepad, opening a file in a Notepad already running:
  NVDA announces the window again as its title changes to the file's
  ("editing dot txt - Notepad"), where Verbatim announces only the text
  area; and closing a tab makes NVDA announce the tab that takes its
  place ("... tab selected 131 of 131") before the text area, where
  Verbatim announces only the text area.
- Reporting the current object a second and third time spells and
  copies the name and value, where NVDA spells and copies the name and
  the same text it read.
- An owner-drawn list view's display text, which needs a display model
  (M14).
- UIA read-only state from a text pattern's document range, which NVDA
  falls back to when `ValueIsReadOnly` is unsupported.
- Events NVDA handles that Verbatim does not subscribe to: UIA
  description changes (`HelpText`), UIA live region changes and system
  alerts, and UIA elements added to or removed from a selection.
  Verbatim does not read `IAccessible2` unique ids, by which NVDA keys
  an `IAccessible2` object's address. UIA delivers no ancestor's events
  to the focus-following registration ("UIA event registration" in
  [backends.md](backends.md)), so a UIA ancestor's state change is not
  spoken.

### Table cell coordinates

NVDA speaks a table cell's row and column, "row 1", "column 2", after
its keyboard shortcut and before its position, from a UI Automation
object's `GridItem` pattern, and leaves out a row or column the same as
the last it spoke in the same table ([Speech](../nvda/speech.md), "Table
cell coordinates"). Verbatim: the outpost reads the `GridItem` pattern's
availability, row and column for an element read alone, a focus, its
selected child and a navigation step's neighbour, never among many
(`verbatim_uia::Cell`), and only while the theme reports the cell
coordinates indication (`cell-coordinates`, on by default, as NVDA's
"Cell coordinates" option is); the snapshot carries them one-based
(`NodeDetails::row` and `NodeDetails::column`), and Core speaks them in
NVDA's place, where they also stand in for a name in deciding whether a
role is spoken. Pinned by Core's unit tests
(`details_speak_in_nvda_property_order`,
`a_nameless_cell_says_its_row_and_column_without_its_role`,
`a_task_switching_item_reached_with_alt_held_says_only_itself`) and the
theme's (`a_cells_row_and_column_render_their_localized_phrases`).
**partial**: matched for the first item the Alt+Tab switcher opens on
(`docs/parity/windows-and-processes.md`, "The Alt+Tab switcher's pane").
Cases that differ: Verbatim speaks both numbers on every focus, where
NVDA leaves out an unchanged row or column within one table, which needs
the containing grid's identity, not read, so a second Tab with Alt held
says "row 1" again where NVDA says only the column (a deliberate
divergence for now; Dickson, 2026-10-25);
NVDA speaks a cell's coordinates text in place of its numbers when the
cell gives one, and its row and column header texts before its row and
column (from UI Automation's `TableItem` pattern, by default) and "through"
a spanned cell's last row or column, neither of which Verbatim reads; a
container the focus enters speaks none, since ancestors are read among
many; with the indication off, a nameless cell speaks its role, where
NVDA, still reading the row for its headers, does not; MSAA and
IAccessible2 cells report none yet.

### An ancestor's state change observed just before a focus change

NVDA handles events in the order they came, so it judges such a change
against the focus it was observed under and speaks it; a focus change
within a window does not cancel that speech, while one into another
top-level window or into a menu does. So NVDA says "unavailable" and
then the new focus within a window, and effectively nothing of it when
the focus leaves the window or enters a menu
([Event handling](../nvda/events.md), "The focus gate: what the default
handlers actually speak"). Verbatim: the outpost handles events in the
order they reach it, and its own hook takes the MSAA focus too, so a
change and a focus reach it in the order Windows raised them, as in
NVDA; such a change is handled before a focus change that reached it
later, and Core's cancelling, NVDA's, does the rest. The listener's copy
of a focus still reaches an outpost whose hook went in only moments
before, and is matched with the hook's copy by counting. **matched
(verified)**: the `msaa_events` mockapp tests, each in both batchings, a
move within the window and one into a menu, and a reducer test for a
move into another window.

### A top-level window's state change

Opening a modal dialog disables its owner, raising a state change on the
owner's client area while the focus is still inside it, on an ancestor
NVDA holds. NVDA says nothing (live captures of Verbatim's Remove Theme
confirmation, opened eleven times, never said "unavailable"), because
the event on a top-level window that was the foreground as the focus
entered it resolves to NVDA's foreground object, not the ancestor
instance; a top-level window that was not the foreground then, as a
popup menu's never is, keeps its ancestor instance, and its state change
is spoken ([MSAA and winevent handling](../nvda/msaa.md), "Object
wrapping and identity"). Verbatim: the outpost records, as the focus
enters a top-level window, whether that window is the foreground, and
while it is, does not count the window's own object, its client area or
window object, among the ancestors whose state changes are spoken.
**matched (verified)**: the `msaa_events` mockapp tests, one each way,
and the `theme_panel` scenario.

### Dialog text, which NVDA reads on entering a dialog

Dialog text, which NVDA reads on entering a dialog: a message box's
question, read after the dialog's title and role and before its focused
button ("A dialog's own text" in [Object model](../nvda/object-model.md)).
Verbatim: the outpost gathers the text by NVDA's rules for a dialog,
alert, or property page the focus newly enters, or one reported as the
foreground window, and reports it as the dialog's description, as NVDA's
dialog class does, so it is spoken where the description is and only
while descriptions are (the `theme_panel` scenario's Remove
confirmation). **partial**: these cases differ. NVDA reads a child
window's client area through whichever API it uses for that window,
where Verbatim reads a dialog's children in the dialog's own API; an
owner-drawn static text with no name, which NVDA reads from the screen,
gives nothing; the Windows Installer's dialog class, and web dialogs,
are not recognized; and a dialog with more than 512 objects says
nothing.
