# Changes and selection

What is spoken when something changes on or near the focus: names, menus, toggles and negated states, state changes, tree item counts, selection, controlled lists, suggestions, and values.

### Name change on the focus

NVDA: when the focused object's name
changes, the new name alone is spoken, queued behind current
speech; a name change on any other object, including an ancestor
of the focus, is silent ([Event handling](../nvda/events.md), "The focus
gate" and "Foreground windows"). Verbatim: the new name alone is
spoken at `Queued` priority, only for the focused node.
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
focused state is unaffected. **matched (unverified)**.

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

Selection announcements (focused list's selected child; changes
while focus stays on container; combo box exclusion). NVDA has no rule
that speaks a newly selected item while focus rests on a list or tab
control; its base `event_selection` speaks only a selection inside an
element the focus controls, and every other selection is a change of
state, spoken only for the focus or its ancestors. Selecting the focused
item itself is such a change of state, and Verbatim speaks it
("selected"), as NVDA does ([verbatim-core](../crates/verbatim-core.md)).
Verbatim: **different** (Dickson, 2026-10-08; the roadmap's M3 item
"announce a focused list's selected item"): it speaks MSAA selection
changes and a focused list's selected item ("Fruits list Apple" where
NVDA says "Fruits list"). The outpost applies these rules before it
reads an MSAA selection: one on the focus or an ancestor has its states
read alone and is reported as a change of state, one inside a focused
list or tab control is read in full, and any other is dropped after its
acquisition, never read. Core's rule asks whether its own focus is a
container, the outpost's whether the focus it last reported is one, so
a selection under an older focus that is a container, whose outpost's
newer focus Core did not take, is not spoken.

### A selection container's selected item through UIA

NVDA reads `SelectionPattern2` where the provider has it, and counts a
provider without it, or a failing selection container, as no selection
without cutting focus speech short ([The UIA client](../nvda/uia.md),
"Element wrapping"). Verbatim: **partial**. Matched for the selected
item it reports with a focused list or tab control: the first selected
item from `SelectionPattern2`'s `FirstSelectedItem`, classically and in
the focus's remote program, and the `Selection` pattern for a provider
without `SelectionPattern2`; any failure is no selected item and the
focus is still reported. Verbatim reads the selection from the
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
menu's search results and the Settings app's search suggestions as
the user types and arrows ("Selection in a list the focus controls"
in [Event handling](../nvda/events.md)). Verbatim: **partial**. For
UIA suggestions, **matched (verified)** for the arrows and the first
suggestion, by a different mechanism when Settings replaces the list's
items as it opens it (the replacing state below), with reducer tests and
the `settings_search_suggestions` scenario (10 of 10 runs on 2026-10-21, one of them in the replacing state). The
scenario checks: "sound" typed in one injection
gives each character's echo, "Suggestions available" cut off by the
first suggestion, and the suggestions as Down Arrow twice and Up Arrow
move through them, what NVDA says in the scenario's comparison of
2026-10-20 (NVDA alpha-58022, the pin), apart from two recorded
differences below. The scenario is local-only because Settings'
search index on GitHub's runner is untested, and because whether
Settings is in the replacing state, which decides how its first
suggestion is found, is Settings' own ([the end-to-end
guide](../tooling/e2e.md), the local-only scenarios). The scenario's expectations take the list's size, as
its first item's set size (`SizeOfSet`, the property Verbatim's "of N"
comes from), and its first three names from the agent, read through UI
Automation once the list is open, since Settings' search index differs between editions.
An earlier comparison that typed one character a step, with compare
mode's second of quiet after each, had Settings open its suggestions
after every character, NVDA saying them each time; that came from the
pacing, and typing in one injection, before Settings' search delay,
opens them once.

Different, each a deliberate difference:

- The first suggestion, which the Settings app selects itself as its
  list opens, is spoken by NVDA with the name its event's element
  carried, which Settings has not set yet ("list item 1 of 22"), and by
  Verbatim with the name read when the selection is handled ("Sound
  settings 1 of 22"), since NVDA's word tells the user nothing (Dickson,
  2026-10-20).
- For each arrow NVDA also says the search box's line, which Verbatim
  leaves unsaid for a caret key that moves nothing ([text](text.md),
  "TextInfo-equivalent layer").

Settings is in the replacing state in some launches, of a frequency
not known: 2 of about 36 scenario runs on 2026-10-20, and, of 17
launches by `cargo xtask nvda capture --verbatim --launch explorer.exe
--arg ms-settings:system` on 2026-10-21, 10 confirmed in it by the
outpost's log, 1 by its symptom alone, 4 whose state is unknown because
the outpost took no focus in Settings and made no search, and 2 not in
it. No runtime id of the search box marks these launches. In them Settings replaces the
list's item elements within about 60 ms of raising the first item's
element selected event, and raises no event for the replacement. Every
call into Settings then waits until it has rebuilt the list, 60 to 80 ms
after the event; by then the relation names a list whose children carry
new runtime ids, so Verbatim's search down from it finds nothing. NVDA's test walks up instead, from the
event's own element to an element the relation names (`isDescendantOf`,
`NVDAObjects/UIA/__init__.py` 2444 to 2463), and in that state it
reaches the list: the replaced item still names the list as its parent,
one step, which NVDA's walk took 199 to 260 microseconds to find once
the relation's read had waited for Settings, in the two launches
confirmed in the replacing state, and in the two whose state is unknown
(`target/e2e-evidence/fixes-round-six-2026-10-21/finding1-walk.txt`).
NVDA said "list item 1 of 22" in every capture taken in the same mode.
Verbatim, finding the event's element in none of the elements the
relation names, reports the first one's current selection instead, read
through its `Selection` property in the same remote operation, with no
further call: in the replacing state that is the new first item, "Sound
settings", 1 of 22, which Verbatim says, as it says it without
replacement (in 4 by-hand launches confirmed in the state, logged at
debug as "the first one's current selection is reported",
`target/e2e-evidence/fixes-round-eight-2026-10-21/route-logs`). A
temporary diagnostic in 4 earlier launches confirmed in the state read
the list's selection there, and the replaced item live from the event's
element on the listener's reading thread after its nearest-window call;
both answered "Sound settings", 1 of 22 (`diag-logs` beside it). This is
a difference in mechanism from NVDA's test, with the same item spoken.
A mockapp test pins the replacing state through a real outpost: an item
selected, its selection event captured and checked as the listener
does while the item still answers, the list's items replaced under new
runtime ids, and the old item's selection then delivered is reported as
the new item, with its name and position (`crates/mockapp/tests/controller_for.rs`, mockapp's
`replace-items`).
The two differ for a selection of an element outside every element the
relation names while the first of them has a selection: NVDA says
nothing of it as a suggestion, and Verbatim reports the list's current
selection (pinned as this difference in
`crates/mockapp/tests/controller_for.rs`). Excepted is a selection of
the focus itself, of one of the ancestors last reported with it, and any
selection while the focus is a list or tab control, which the reducer
reads as one in the focus: Verbatim reads it as the focus's own, from
runtime ids and roles it already holds, with no call, reads no current
selection, and handles it as other selections, as NVDA's base selection
handler falls through to its state change handler
(`NVDAObjects/__init__.py` 1314 to 1339); each of the three cases is
pinned in `crates/mockapp/tests/controller_for.rs`. The last case does not ask
whether the element is one of the focus's items, since that needs a
call, so a focused list whose relation names another list that replaces
its items loses the route: the stale selection of the replaced item goes
to the other selection handling, which reads a dead element, where
NVDA's walk up from the item reaches the controlled list and reports it
as a suggestion. No application is known to do this. By-hand
captures of the two Settings sequences that could take that route by
mistake, on 2026-10-21 (NVDA alpha-58022, the pin, twice each, and
Verbatim three times each, `target/e2e-evidence/fixes-round-ten-2026-10-21/`,
the `nvda-*-capture-*.txt` and `verbatim-*-capture-*.txt` files, with
Verbatim's logs beside them), found no such report. Enter on a
suggestion from another category ("mouse" typed on the System page,
"Mouse settings") makes Settings raise three selection events while the
search box is still the focus, and by then the box's relation names
nothing, so neither the search nor the route runs; both screen readers
then say "Mouse" and "Primary mouse button combo box Left collapsed".
Shift+Tab out of the open list ("sound" typed) gives "list" and
"Display 1 of 19" from both. In both sequences the only difference in
speech is the first suggestion's name, the deliberate difference above. Porting NVDA's walk instead needs the event's
element, which only the focus listener holds, and a live read of the
replaced item's name, position and set size: about 8 more wake-ups in
the listener and 2 in the application for each selection event. When
the list has nothing selected, or the selection is the focus's own, the
event is no controlled selection, as before, logged at debug as "a
selection in none of the elements the focus controls, and no current
selection read from the first" when the relation names something.

Three more differences are known from the source and not captured:

- NVDA tests the event's element against every element the relation
  names, and Verbatim reads only the first one's current selection, so a
  selection in a second controlled element that the search cannot find
  is reported as the first controlled element's current selection when
  it has one, and is silent otherwise, where NVDA's walk from the
  event's element reaches the second element and reports the event's
  element. Settings' search box names one.
- A selection event left stale by a second replacement of the list, as a
  second burst of typing while the list is open could cause, is reported
  by Verbatim as the current list's selection, and when the new list's
  own selection event follows, Verbatim says the same item again, the
  first report cut off. NVDA's walk from the stale event's element
  reaches the list, as after the first replacement, and NVDA reports the
  event's element.
- NVDA's test (`isDescendantOf`, `NVDAObjects/UIA/__init__.py` 2444 to
  2463) is a tree walker's `NormalizeElement` from the event's element
  with a condition on the controlled element's runtime id, which by
  Microsoft's documentation answers the element itself when it meets the
  condition, so NVDA reports a selection event raised on the controlled
  list itself as a controlled selection. Verbatim does not count the
  list as its own descendant and reports no controlled selection for it
  (`crates/mockapp/tests/controller_for.rs`). No Settings sequence has
  been seen to raise one.

The first suggestion comes before the suggestions-opened sound only
because the selection's place in the outpost's queue arrives before
Settings' second change of the search box's relation, by about 50 ms in
the runs of 2026-10-20; anything that delays the listener's handling of
the selection by that much puts the sound first, as temporary
diagnostics in the listener's callback did in two runs that day
(`target/e2e-evidence/fixes-item-11-2026-10-20/finding1-diagnosis.txt`).
The ControllerFor relation is read live for each selection event,
as NVDA reads it (Dickson, 2026-10-10), in one remote operation with
the search for the selected element. Not yet built: Start's results
that are in a Chromium document NVDA reads through IA2, which belong to
the phase that implements IA2 (browsers, M7).

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
NVDA's own handling of it is in app modules only.
**matched (unverified)**.

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
ten percent. A UIA progress bar is reported only while it is the focus,
since UIA's property events are followed on the focus alone
(`docs/parity/backends.md`, "UIA event registration"); a foreground UIA
progress bar that is not the focus is not yet reported (M4.2). Background progress bar reporting, an
NVDA option off by default, is not yet built (M4.3): background
progress bars stay silent for now, with no setting (Dickson, 2026-10-10,
confirmed as planned for later on 2026-10-15).

### A combo box's value from its selected item

NVDA: a UIA combo box without the `Value` pattern, and a list whose
class name contains `LoopingSelector` (a time picker's), has as its value
the name of the first item its `Selection` pattern holds; when an item
that does not have the focus changes state, or is selected, while its
parent is such a focus, NVDA raises a value change on the focus, so the
new item is spoken as the focus's value. Verbatim: the focus is read
with its first selected item's name as its value, and the listener's
selection event for a child of such a focus is reported as a value
change of the focus, which the reducer speaks as it speaks any value
change on the focus, the bare value (mockapp's `combo_box` tests). The
one search that finds the item among the focus's children reads with it
its name and whether it has the keyboard focus, both as they are then,
as NVDA reads the item's focus and the combo box's value live when it
handles the event: an item with the keyboard focus is no value change and
is said by its focus alone, and the value is the item's name as found,
never the event's cached name, which can be empty for an item an
application names late. Different: the value is the event's item, where
NVDA reads the combo box's current selection when it handles the event
(`ComboBoxWithoutValuePattern._get_value`, `NVDAObjects/UIA/__init__.py`
2762 to 2767). The two differ when the selection moves again before the
event is handled: when two quick arrows on a collapsed combo box select
A and then B before A's event is handled, NVDA says B for both events
and Verbatim says A, then B. Verbatim keeps the event's item, so that each item the
user passed is said, as each arrow's answer. NVDA also takes the items' added-to and removed-from
selection events, which Verbatim does not subscribe to
(`docs/parity/backends.md`, "UIA event registration").

The Settings app's combo boxes take this path: checked live on
2026-10-20 on Windows build 26200, every combo box on its Display,
Mouse, Date and time, Language and region, and Colors pages has no
`Value` pattern, and arrowing the collapsed "Show additional calendars
in the taskbar" combo box raises an element selected event on the new
item, which Verbatim speaks as the combo box's value, "Simplified
Chinese (Lunar)", and back, "Don't show additional calendars". Expanded
with Alt+Down Arrow on 2026-10-21, its items take the keyboard focus as
they are arrowed, and each is said once, by its focus ("Simplified
Chinese (Lunar) not selected 2 of 3"), with no value change
(`target/e2e-evidence/fixes-round-six-2026-10-21/settings-combo-box-live-check.txt`).
NVDA, captured with the same keys on 2026-10-21 (NVDA alpha-58022, the
pin), says each item in the same words, "not selected" included, and
says "expanded" at Alt+Down Arrow before the first item, which Verbatim
does not say, a difference on the roadmap (M4.2)
(`target/e2e-evidence/fixes-round-eight-2026-10-21/nvda-expanded-combo-box-capture.txt`,
beside Verbatim's `target/e2e-evidence/fixes-round-six-2026-10-21/expanded-combo-box-capture.txt`).
So NVDA's second rule, the same value change for a `Windows.UI.Core`
combo box that has the `Value` pattern (lines 2795 to 2798, NVDA issue
6337), is not needed for Settings and is not built: no such combo box
was found to try it on. Not built either: the classic .NET Framework
Windows Forms combo box (the roadmap's audit item 11, deferred).
**matched (unverified)**: the
expectation is from NVDA's source (`NVDAObjects/UIA/__init__.py`, lines
1380 to 1392, 2746 to 2767 and 2786 to 2800), with no capture of a real
such combo box or time picker yet.

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
