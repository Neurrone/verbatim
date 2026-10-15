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
in [Event handling](../nvda/events.md)). Verbatim: **partial**. Matched
for UIA, with reducer tests, and checked live on the Settings app's
search suggestions ("Display settings 1 of 25" and so on as the user
arrows, with the focus staying in the search box); no scenario pins it
yet. The ControllerFor relation is read live for each selection event,
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
