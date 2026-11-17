# Changes and selection

What is spoken when something changes on or near the focus: names, menus, toggles and negated states, state changes, tree item counts, selection, controlled lists, suggestions, and values.

### Name change on the focus

NVDA: when the focused object's name
changes, the new name alone is spoken, queued behind current
speech; a name change on any other object, including an ancestor
of the focus, is silent ([Event handling](../nvda/events.md), "The focus
gate" and "Foreground windows"). Verbatim: the new name alone is
spoken at `Queued` priority, only for the focused node, when it differs
from the name last spoken of it, which a repeated report of the focus
does not move and a spoken report of the focus does, as NVDA remembers
every property it speaks of an object whatever the reason. A state change of an ancestor is compared with the states
last spoken of it, which an ancestor that was the previous focus keeps
from when it was spoken as the focus.
**matched (unverified)**.

### Entered menus

NVDA: when focus newly enters a menu bar, a popup
menu, or a menu item as an ancestor, speech is cancelled and the
ancestor is not announced; the focused item is announced as usual.
NVDA cancels as it reaches the menu among the entered ancestors,
outermost first, so a container entered outside the menu in the same
focus change is cut off too, as Windows 11 Notepad's "Popup window" is
on Alt+F ([Event handling](../nvda/events.md), "The focus gate").
Verbatim: entering one cuts speech (`Effect::StopSpeech`) at its place
among the entered containers, and it is not announced.
**matched (unverified)**.

### Menu popup announcements

NVDA: menu events with fake-focus
fallback ([MSAA and winevent handling](../nvda/msaa.md)). Verbatim:
NVDA's menu rules run in the outpost's worker. Within a batch, focus
events are handled first and the newest menu opening from each
backend last; a menu opening is ignored if a focus in the batch
already put focus on a menu or menu item, and otherwise becomes a
focus on the popup menu, so the reducer receives only focus events
for menus. A menu closing, menu mode ending, or the Alt+Tab
switcher closing anywhere on the desktop is forwarded by the focus
listener to Core 50 milliseconds later, with the time it closed; unless
a focus observed after the close has been applied by then, Core asks
the application in the foreground for its real focus, as NVDA's fake
focus reads the focus from the foreground window when no focus followed
the close. Focus usually returns to another application than the one
that owned the menu, which a check inside the menu's own outpost
could not see. **matched (unverified)**.

### Toggle button role and "pressed"/"not pressed" wording

Toggle button role and "pressed"/"not pressed" wording (UIA Toggle
pattern on a Button; no separate Switch role). NVDA: UIA
detection. Verbatim: **matched (verified)** (a cross-process test).

### Negated states

"not checked", "not pressed", and "not selected", by
the rules in "Which states are spoken, and in what order" in
[Speech](../nvda/speech.md). Verbatim: **matched (unverified)**, checked
against NVDA's source; it needs a live NVDA comparison across roles.

### State changes after the focus has left

NVDA speaks a state change
only while the changed object is its focus, and its focus changes when
it handles the focus event, in the order the events came ("The focus
gate" in [Event handling](../nvda/events.md)): the state change of a
control the focus has already left, raised after the focus event for
the next control, is silent. Verbatim's outposts can deliver that state
change before the focus event, which takes longer to read, so Core
can still hold the old control as the focus when it arrives. Verbatim
judges it by the changed object's own states: a focus that reported
itself focused when it became the focus, and whose new state set no
longer includes focused, has lost the focus, and its change is not
spoken (its states are still kept). A control that never reports the
focused state is unaffected. A focus that has expired is not spoken
for either: the focus listener drops a focus that lost the keyboard
focus before it was read, so it never becomes Core's focus, as the
Control+Tab case in [terminals](terminals.md) records. **matched
(unverified)**.

### The number of items an expanded tree view item holds

NVDA follows
"expanded" on a Win32 tree view item that has just become expanded
while it is the focus with the number of its children, "52 items"
("How many items an expanded tree view item holds" in
[Speech](../nvda/speech.md)). Verbatim: the outpost counts a
`SysTreeView32` item's children with a state change that leaves it
expanded, and Core speaks the count when the change makes the focus
expanded. **matched (verified)** (the `theme_panel` scenario).

### Selection announcements

Selection announcements (a focused list's selected item; changes
while focus stays on container; combo box exclusion). NVDA has no rule
that speaks a newly selected item while focus rests on a list or tab
control; its base `event_selection` speaks only a selection inside an
element the focus controls, and every other selection is a change of
state, spoken only for the focus or its ancestors. Selecting the focused
item itself is such a change of state, and Verbatim speaks it
("selected"), as NVDA does ([verbatim-core](../../books/dev/src/reference/crates/verbatim-core.md)).
Verbatim: selection events take NVDA's rule (Dickson, 2026-11-15),
judged in Core at the event's turn against the committed focus: a UI
Automation selection on the committed focus is its change of state,
judged on the states its event carries; any other, and every plain MSAA
selection, has Core ask for the item with the committed focus's
controller-for relation read live, and an item that is an object the
focus controls, or inside one, is spoken as a focus, while any other is
a change of state of the selected object, spoken only on the focus or an
ancestor (`docs/design/readers-tell-core-asks-build1.md`, "Changes on
objects"). **matched (unverified)**. A focused list's selected item is
not said with the focus, as NVDA says "Fruits list" (Dickson,
2026-11-17), and neither reader reads it: only the selected item that is
a combo box's or a looping selector's value is read with its focus.

### A selection container's selected item through UIA

NVDA reads `SelectionPattern2` where the provider has it, and counts a
provider without it, or a failing selection container, as no selection
without cutting focus speech short ([The UIA client](../nvda/uia.md),
"Element wrapping"). Verbatim: **partial**. Matched for the selected
item that is a focus's value (below, "A combo box's value from its
selected item"): the first selected item from `SelectionPattern2`'s
`FirstSelectedItem`, classically and in the focus's remote program, and
the `Selection` pattern for a provider without `SelectionPattern2`; any
failure is no selected item and the focus is still reported. Verbatim reads the selection from the
container, so the item's own selection container is never read. Not
yet built: NVDA's use of `ItemCount` to leave "selected" unsaid on a
focused selectable item (a cell, a header) that is the only one
selected, which Verbatim's reducer does not apply; Verbatim leaves
"selected" unsaid only for list items, tree items, menu items, rows,
and check boxes, whatever the count.

### Selection in a list the focus controls

Selection in a list the focus controls (search suggestions and
results). NVDA: when an item is selected inside an element the focus
names in its UIA ControllerFor relation, NVDA reports that item as it
reports a focus, without moving focus; this is how it reads the Start
menu's search results and the Settings app's search suggestions as the
user types and arrows ("Selection in a list the focus controls" in
[Event handling](../nvda/events.md)). Its test walks up from the
event's element to an element the relation names (`isDescendantOf`,
`NVDAObjects/UIA/__init__.py` 2444 to 2463). Verbatim: **partial**.
UIA suggestions match, verified for the arrows and the first suggestion
by reducer tests and the `settings_search_suggestions` scenario: "sound"
typed in one injection gives each character's echo, "Suggestions
available" cut off by the first suggestion, and the suggestions as Down
Arrow twice and Up Arrow move through them, as NVDA says them in the
scenario's comparison (NVDA alpha-58022, the pin), apart from the
differences below. The scenario's expectations take the list's size and
its first three names from the agent, read through UI Automation once
the list is open, since Settings' search index differs between
editions. It is local-only, because GitHub's runner's Settings raises
no selection event for a first suggestion as the list opens, so there
"Suggestions available" plays to completion ([the end-to-end
guide](../../books/dev/src/how-to/e2e.md), the local-only scenarios). Start's results
that are in a Chromium document NVDA reads through IA2 are not yet
built; they belong to the phase that implements IA2 (browsers, M7).

Verbatim reads the relation live for each selection event, as NVDA
reads it (Dickson, 2026-10-10): Core asks the focus listener to read the
selected item and its outpost to complete it, and the completion reads
the committed focus's controller-for relation and answers whether the
item is an element it names or inside one, from which Core judges.
NVDA's results captured on mockapp's list fixtures (2026-11-09):
a selection of the controlled list itself says "Suggestions list selected",
as NVDA's `NormalizeElement` answers the element itself
(`NVDAObjects/UIA/__init__.py` 2444 to 2461); a selection in a second
controlled element says "Elsewhere", the event's element being reported,
and a selection in a list the focus no longer controls says nothing. A
selection left stale by a replacement of the list, the first or a second,
says nothing, since its element is in none of what the focus controls; a
selection of the replacement item is an ordinary one, "Sound settings  1
of 2". A mockapp test pins these through a real outpost
(`crates/mockapp/tests/controller_for.rs`, mockapp's `replace-items` and
`set-controller-for`). A
capture of 30 searches for "sound" (2026-11-03) also found, in 5 of
them, a moment of about 90 ms after the first selection when the list
had no children and its current selection had no name and no position,
before all 22 items appeared together with a `ChildrenBulkAdded`
structure change; read then, the item would be said as "list item".
No such reading was seen with Verbatim running: 20 runs of
`settings_search_suggestions` each said "Sound settings 1 of 22", so
nothing in Verbatim waits for the list to fill (Dickson, 2026-11-03).
For each arrow NVDA queues the search box's line ("A caret key that
moves nothing" in [text](text.md)) and cuts it off with the suggestion
as it reports it, about 100 ms after the key, once its script ends.
Verbatim does the same: the selection's event waits behind the key in
Core's event queue, so the key's line is said and then cut off by the
suggestion.

Different: the first suggestion, which Settings selects itself as its
list opens, is spoken by NVDA with the name its event's element carried,
which Settings has not set yet ("list item 1 of 22"), and by Verbatim
with the name read when the selection is handled ("Sound settings 1 of
22"), since NVDA's word tells the user nothing (Dickson, 2026-10-20).

### Suggestions appearing and going away

NVDA: when the ControllerFor
relation of a focused editable field or search field changes, NVDA
plays its suggestions-opened sound if the field is the focus and now
names elements, and its suggestions-closed sound otherwise
([Event handling](../nvda/events.md), "Selection in a list the focus
controls"). Verbatim: the outpost follows ControllerFor on the focus,
reads it live for an element with a text pattern or the automation id
"SearchTextBox" or "TextBox", and Core plays the suggestions-opened or
suggestions-closed indication. NVDA's editable-text condition also asks
for navigable text or a text selection; Verbatim takes the text pattern
alone. Help text changes on the focus are followed too, and spoken as
NVDA speaks a description change. The item status is not followed:
NVDA's own handling of it is in app modules only. Settings' search box
sometimes raises its ControllerFor change twice, about 5 ms apart, as
its suggestions close, and both readers play the suggestions-closed
sound for each change they receive: NVDA plays it whenever the field is
not the focus or names no elements, and its UIA debug log received the
duplicate after one Escape of three
(`target/e2e-evidence/m42-desktop-2-2026-10-22/part3a-nvda-1.log` to
`-3.log`, read from the line each `.startline` file gives); both enable
UIA's event coalescing, so only how often Settings' duplicate arrives
differs. `settings_search_suggestions_reentered` accepts the second
sound as an optional utterance. **matched (unverified)**.

### Value change on focused node speaks bare value

Value change on focused node speaks bare value (slider drag), not
for an edit field or document, not when unchanged, and not for a role
that never speaks its value. Verbatim: **partial**. The bare value is
matched, checked against NVDA's source. A progress bar's value changes,
focused or not, by NVDA's progress bar behavior ("How a progress bar
reports its value" in [Object model](../nvda/object-model.md)), are
matched for MSAA (reducer tests and the `msaa_events` mockapp test).
Verbatim's progress indication, a tone by default (D12), is given once
the percentage moves by a percent from the last one indicated at that
place, and the value is not spoken; an off-screen or invisible progress
bar, or one whose value is no number, is an ordinary value change.
Different: a theme that reports the indication as speech speaks it at
the same one-percent interval, where NVDA's speech mode speaks every
ten percent.

Progress bars that are not the focus: NVDA follows every UIA range value
change desktop-wide and drops all but a progress bar's. Verbatim: the
same, and no outpost reports a progress bar. The listener also takes every bar's change, foreground or background,
as NVDA's one hook does (`docs/design/progress-bars.md`, approved by
Dickson on 2026-11-09): a UIA bar's range value change in its
desktop-wide group, filtered by its cached control type and provider on
the callback thread, and a classic window's `EVENT_OBJECT_VALUECHANGE` on
its global hook, recognised by its normalised class or, through a capped
memo per application, its role, with its percentage read on the
application's reading thread and no outpost started. Core keeps every
change at one place to one percent, and drops a bar outside the foreground
window unless `report_background_progress_bars` is on, false by default
and with no place in the dialog until M4.3. A focused bar's ordinary
value change is silent when its value is a number and it is on screen,
heard only as a bar; off screen or with no number it is an ordinary value
change.

### A combo box's value from its selected item

NVDA: a UIA combo box without the `Value` pattern, and a list whose
class name contains `LoopingSelector` (a time picker's), has as its value
the name of the first item its `Selection` pattern holds; when an item
that does not have the focus changes state, or is selected, while its
parent is such a focus, NVDA raises a value change on the focus, so the
new item is spoken as the focus's value. NVDA reads the combo box's
current selection when it handles the event
(`ComboBoxWithoutValuePattern._get_value`, `NVDAObjects/UIA/__init__.py`
2762 to 2767), and also takes the items' added-to and removed-from
selection events.

Verbatim: **matched (unverified)**: the expectation is from NVDA's
source (`NVDAObjects/UIA/__init__.py`, lines 1380 to 1392, 2746 to 2767
and 2786 to 2800), with no capture of a real such combo box or time
picker. The focus is read with its first selected item's name as its
value, and the listener's selection event for a child of such a focus is
reported as a value change of the focus, which the reducer speaks as it
speaks any value change on the focus, the bare value (mockapp's
`combo_box` tests). The one search that finds the item among the focus's
children reads with it its name and whether it has the keyboard focus,
both as they are then, as NVDA reads them live when it handles the
event: an item with the keyboard focus is no value change and is said by
its focus alone, and the value is the item's name as found, never the
event's cached name, which can be empty for an item an application names
late. The added-to and removed-from selection events are not subscribed
to (`docs/parity/backends.md`, "UIA event registration").

Different: the value is the event's item, so that each item the user
passed is said, as each arrow's answer. When two quick arrows on a
collapsed combo box select A and then B before A's event is handled,
NVDA says B for both events and Verbatim says A, then B.

The Settings app's combo boxes take this path: checked live on Windows
build 26200, every combo box on its Display, Mouse, Date and time,
Language and region, and Colors pages has no `Value` pattern, and
arrowing the collapsed "Show additional calendars in the taskbar" combo
box raises an element selected event on the new item, which Verbatim
speaks as the combo box's value, "Simplified Chinese (Lunar)", and back,
"Don't show additional calendars".

### A combo box with the `Value` pattern whose items give its value

NVDA: the value change above is a rule of `ListItem`, NVDA's class for a
UIA list item control type, and has two more cases.

- A list item without the focus whose parent is the focus, when the
  parent has the `Value` pattern (read from its cache) and its window's
  class begins with `Windows.UI.Core`, for a combo box that does not raise
  value changes of its own (`ListItem.event_stateChange`,
  `NVDAObjects/UIA/__init__.py` 2786 to 2800, NVDA issue 6337). The
  parent need not be a combo box. Like the first rule, it fires on the
  item's selection, which NVDA's `event_selection` ends with a state
  change (`NVDAObjects/__init__.py` 1339).
- A classic .NET Framework Windows Forms combo box
  (`_NetFrameworkWinFormsComboBox`, lines 1387 to 1399 and 2734 to 2744,
  NVDA issue 17454): a combo box from framework `WinForm`, whose window's
  class normalizes to `COMBOBOX`, whose provider description names
  `System.Windows.Forms, Version=4.0.0.0`, with the `Value` pattern and
  without the `Selection` pattern. Its drop-down list is a window of its
  own, class `ComboLBox`, whose items are not its children, and it raises
  no value change when its selection changes. The object asks for the
  element selected events of its process's `ComboLBox` windows whatever
  the foreground (`requestEvents`, the acceptance test's rule 2). When a
  list item is selected (`ListItem.event_UIA_elementSelected`, lines 2770
  to 2784), NVDA first handles the selection as any other, then, when the
  focus is such a combo box in the item's process, the item's window's
  class is `ComboLBox`, and `GetComboBoxInfo` on the focus's window names
  the item's window as its list, raises a value change on the focus.

Verbatim: **matched (unverified)**, from NVDA's source, with no capture
of such a combo box. The item's role must be a list item. For the
`Windows.UI.Core` case, Core asks the committed focus's outpost one read
of the focus's `HasValuePattern`, `WindowClass` (its own or nearest
window's class) and `Value` when such an item's parent is the focus and
its value is not its selection's, and speaks the value as a value change
when NVDA's condition holds; NVDA reads the first two from its cache with
no call. For the Windows Forms case, a selection from a `ComboLBox`
window of the committed focus's process while the focus is a combo box
makes Core ask the focus's `ComboBoxList`, which the outpost answers with
`GetComboBoxInfo`'s list only for such a combo box; with an answer, the
selection is taken whatever the acceptance test says, handled as any
other, and an item in that list gives the focus's value change, its value
read live. Different in form: NVDA takes these events from the time such
a combo box is first made, Verbatim only while the committed focus is a
combo box of their process; while another object is the focus, a
selection NVDA takes says nothing unless the focus controls the list,
which no application is known to do.

### A combo box opened into its list

NVDA, captured on 2026-10-21 (NVDA alpha-58022, the pin), arrowing the
Settings combo box "Show additional calendars in the taskbar" after
Alt+Down Arrow, says "expanded" at Alt+Down Arrow before the first item,
and each item by its focus, "not selected" included
(`target/e2e-evidence/fixes-round-eight-2026-10-21/nvda-expanded-combo-box-capture.txt`).
Verbatim: **matched (verified)**, from that capture (the
`settings_combo_box_expanded` scenario). Expanded, the items take the
keyboard focus as they are arrowed, and each is said once, by its focus
("Simplified Chinese (Lunar) not selected 2 of 3"), with no value change
(`target/e2e-evidence/fixes-round-six-2026-10-21/settings-combo-box-live-check.txt`).
Settings raises the combo box's expand state change before the item's
focus, from 0 to 2 ms before it. The change is told by the outpost
following the combo box and the focus by the focus listener, two
processes with no order between them, so when the item's focus read
completes with the committed combo box among its ancestors, Core first
judges the combo box's waiting UI Automation state changes, then takes
the focus (ADR 0003's narrow ordering rule,
`docs/design/readers-tell-core-asks-build1.md`, "The narrow ordering
rule"), and Verbatim says "expanded" there too. A change that reaches
Core only after the focus has been taken is said after the item, as an
ancestor's change.

### What an accepted MSAA change reads

NVDA: a name, description, value,
or state change on its focus or an ancestor reads only the property its
event names, of the object it already holds, whose class was chosen
when the object was made; a progress bar's value change reads its
states, value, and location. Verbatim: the same, once the object is
known by its identity to be the focus or an ancestor; the rules of a
full read that depend on the object's role (a list view item named by
its columns, an edit field in a labelled combo box nameless, a tree
view item's check state, a multi-line edit control) take the role the
outpost last read it with. A provider that changes two properties under
one event is heard as NVDA hears it, the second change only with its
own event. **matched (unverified)**.

### State-change diff announcements

State-change diff announcements (gained states; lost states spoken
by their absence, including half checked becoming "not checked"), by
"Which states are spoken, and in what order" in
[Speech](../nvda/speech.md). Verbatim: **matched (unverified)**,
checked against NVDA's source.
