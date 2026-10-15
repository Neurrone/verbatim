# Focus announcement

What a focus announcement contains and in what order: names, roles, states, values, multi-line fields, and the reading of an edit field on focus.

### Focus announcement content and property order

Focus announcement content and property order (name, role, value,
states in fixed order, description, shortcut, position, level).
NVDA: `speakObject` ordering ([Speech](../nvda/speech.md)). Verbatim:
**matched (verified)** for the M3 surface — E2E scenarios assert
wording; per-property order transcribed in `verbatim-core`
([verbatim-core](../crates/verbatim-core.md)). **Different**, found on
2026-10-02 by running NVDA with its speech log through the same keys
in Explorer, Settings, and Start:
- NVDA speaks no role on focus for a set of roles, among them list
  item, menu item, tree view item, pane, static text, and unknown,
  when the object has a name or value; Verbatim spoke them ("alpha.txt
  list item" where NVDA says "alpha.txt"). **Matched since
  2026-10-02** ("When the role is spoken" in
  [Speech](../nvda/speech.md)), including object navigation, which NVDA
  speaks with the focus reason, entered containers, which it speaks
  like a focus, and reporting the current object, which keeps the
  role.
- Verbatim announced an MSAA window object above a control as an
  entered container ("Categories: window", "vbtest - File Explorer
  window vbtest - File Explorer"); NVDA never presents a window object
  it reaches through a control's parents, and reads a foreground
  window and window-level events through the window's client area.
  **Matched since 2026-10-02.**
- Explorer's file items exposed their name again as their value, so
  Verbatim spoke it twice; NVDA's `UIItem` class reports no value.
  **Matched since 2026-10-02.**
- NVDA speaks a tree item's level first ("level 1, System, 2 of
  12"); Verbatim spoke it last. An NVDA transcript in msinfo32 on
  2026-10-07 showed NVDA puts the level first only when it differs from
  the last one spoken ("level 1 Hardware Resources collapsed 1 of 3",
  then "Components collapsed 2 of 3 level 1"). **Matched since
  2026-10-07** ("Where the level goes" in [Speech](../nvda/speech.md)): a
  tree or list item's level goes first when it differs from the last
  level put first, and last otherwise, for a focus, object navigation, a
  selection, and reporting the current object alike (the
  `system_information_tree` scenario). The same transcript showed a standard
  tree view's root item is "level 0", the raw `accValue`, in NVDA;
  Verbatim had added one, and matches since 2026-10-07 (the
  `system_information_tree` scenario).

A parity audit of the UIA and MSAA handling on 2026-10-02, checked
against NVDA's source, found these differences in generic behavior.
**Matched since 2026-10-02**, with reducer, mapping, and mockapp
tests:
- States: spoken in NVDA's fixed order (unavailable before checked,
  so "check box unavailable not checked"); "read only" only for edit
  fields and check boxes outside a query; "default" never, as NVDA
  has no such state; "submenu" dropped on a combo box and
  expanded/collapsed on a submenu item; "not selected" only for a
  focusable list item, tree view item, row, cell, header, or check
  box, and only on focus or a change on the focus; "not checked" also
  for any checkable item; positive "selected" kept on a tab and in a
  query; "focused" and "off screen" spoken in a query. The rules are
  "Which states are spoken, and in what order" in
  [Speech](../nvda/speech.md).
- "multi line" for a standard or rich edit control with the
  `ES_MULTILINE` style, after "read only" (NVDA's edit control class;
  UIA's multi-line state is dropped, as NVDA drops it): **matched since
  2026-10-07**, found by comparing transcripts with NVDA in a Windows
  Forms text box and Verbatim's own read-only description field.
- Found by NVDA transcripts of Verbatim's settings dialog on
  2026-10-07, tabbing through the Speech, Theme, and Terminal pages:
  NVDA announces the group box a control sits in when the focus enters
  it ("Synthesizer grouping" before the Change button), which Verbatim
  did not, since a Win32 group box is the control's sibling rather than
  its parent and NVDA finds it by position. **Matched since 2026-10-07**:
  a control's window object has as its container the first visible
  group box before it in z-order whose rectangle holds it, as NVDA's
  `findGroupboxObject` finds it (`native_controls` mockapp test with
  real buttons; the end-to-end check is the settings dialog's Speech
  page). **Not yet:** the group box's description, which NVDA takes from
  a static text right after it; and NVDA says "Verbatim", the menu's owner window,
  before "Context menu". Control+Tab to another category made Verbatim
  announce the dialog again under its new title, because the dialog
  was read as a different node after the title changed, where NVDA,
  seeing the same object, does not, and says nothing about the new
  title either: **matched since 2026-10-07**, by matching a window's
  own MSAA object by the address its identity string names (Held
  objects, below; the `settings_dialog_keys` scenario). NVDA's
  "Verbatim": NVDA names the foreground window a menu opens from, and
  Verbatim's hidden frame, titled "Verbatim", is shown and brought
  forward for the menu, since without NVDA's `uiAccess` privilege a
  hidden window cannot take the foreground. (NVDA's own menu has no such
  frame: NVDA keeps its frame hidden and says only "NVDA menu", a
  transcript of NVDA+N on 2026-10-07.) **Matched since 2026-10-07, by
  decision**: Verbatim+V says "Verbatim", then "Context menu", as NVDA
  does. The frame's own focus and foreground events stay suppressed,
  since it transits focus (the hidden-frame suppression in
  `docs/crates/verbatim-outpost.md`); the menu's opening is reported in
  the frame's window with the frame as its ancestor, and the reducer
  names a new foreground window from the top of the focus's ancestry, as
  NVDA does when no foreground event named it (every scenario that opens
  the menu asserts both). The Terminal page's "Lines spoken in full"
  and "Last lines to speak" sliders read 29 for the setting's 30 in
  both screen readers: a standard trackbar's MSAA value is its position
  as a percentage of its range, which was 1 to 100 there then. **Fixed since
  2026-10-07**: each slider gives its position as its value, so both
  screen readers read 30 (the `terminal_settings_page` scenario). Every other
  control on the three pages reads the same in both, apart from the
  states and tree levels above.
- Found by NVDA transcripts in Windows 11 Notepad on 2026-10-07 and
  **not yet** matched: opening a file in a Notepad already running,
  NVDA announces the window again as its title changes to the file's
  ("editing dot txt - Notepad"), where Verbatim announces only the text
  area; and closing a tab makes NVDA announce the tab that takes its
  place ("... tab selected 131 of 131") before the text area, where
  Verbatim announces only the text area.
- Values and descriptions: an edit field or document no longer
  speaks the whole field after every keystroke; an unchanged value is
  not repeated; a check box, radio button, link, menu item, or
  application never speaks its value; a description equal to the
  name is dropped ("When values and descriptions are spoken" in
  [Speech](../nvda/speech.md)).
- UIA: a selected radio button is checked, not selected; a
  toggleable list or menu item is checkable; a menu item that no
  pattern makes checkable is checkable and checked when its legacy
  MSAA state (`LegacyIAccessibleState`) has the checked bit, as NVDA
  2027.1 reads Windows Forms menu items (**matched since
  2026-10-06**); password, required,
  invalid entry, and read-only states are read (`ValueIsReadOnly`
  and `IsDataValidForForm` ignoring their defaults, as NVDA reads
  them); a control with only a `RangeValue` pattern has that value,
  rounded, and its changes are followed; the access key and the
  accelerator key are both spoken; dialogs are recognized by
  `IsDialog` and NVDA's dialog class names; and an ancestor counts as
  context only when UIA calls it both a control and content.
- MSAA: protected is read; whitespace-only names and values count as
  absent; the edit field of a labelled combo box has no label of its
  own; list view and tree view items report their position ("1 of
  3").
- Roles: both backends now map every role NVDA's tables give a
  counterpart for in Verbatim's vocabulary, including split button,
  graphic, progress bar, scroll bar, table, row, cell, headers, data
  grid, document, title bar, tool tip, and separator; a UIA document
  is a "document", not an "edit" (Notepad's text area is "Text editor
  document"). Progress bars and title bars are never focus context.
  The MSAA roles NVDA maps that had no counterpart in Verbatim's
  vocabulary were unknown, so spoken as nothing: IP address, animation,
  clock, drop list, dial, grip, indicator, border, white space, math
  (an equation), chart, diagram, column, character, sound, tree view
  button, drop down button grid, and help balloon. **Matched since
  2026-10-07**, each with NVDA's English name, and so are the visited
  state (MSAA traversed), said only of a link, and the linked state,
  said only when it changes; a half-checked progress bar is a "busy
  indicator" with no half-checked state and no value spoken, as NVDA
  adjusts it (map unit tests and reducer tests).

**Different**, still, from the same audit:
- On focus NVDA reads an edit field's selection or the line at the
  caret through its text interface, in place of its value ("What an
  object with text says" in [Speech](../nvda/speech.md)). **Matched since
  2026-10-06** for focus, through the text protocol: the name, role, and
  states are spoken at once, and the line, or "selected" with the
  selected text, follows as soon as the outpost's caret report arrives;
  an edit field, document, or terminal with no text interface speaks its
  value once the outpost says so (`NoText`), after the rest of the
  announcement rather than between the role and the states.
  **Different:** a protected field's text is never read on focus, where
  NVDA reads whatever masked text the control gives; and an empty field
  has no placeholder to speak. Object navigation, to-focus, and
  reporting the current object read the text the same way, **matched
  since 2026-10-07**, whether or not the object has the focus: the
  selected text, else the line at the caret, else the first line of a
  control that reports no caret, and the value when it has no text
  interface. **Not yet:** reporting the current object a second and
  third time still spells and copies the name and value, where NVDA
  spells and copies the name and the same text.
- A multi-column list view item (a report view, such as msinfo32's
  right pane) is named by NVDA from its column texts, "content; Header:
  content", with no value or description; Verbatim kept MSAA's name and
  description. **Matched since 2026-10-07**: an item of a
  `SysListView32` in the report view or tiles is named by its columns
  in the order shown, each but the first after its header, leaving out
  a column of zero width or no text, through the control's messages with
  their structures in the list view's process, and no list view item has
  a value or description (`native_controls` mockapp test, a real list
  view; the end-to-end scenario is still to be written). **Not yet:** an
  owner-drawn list view's display text, which needs a display model
  (M14).
- UIA read-only state from a text pattern's document range, which
  NVDA falls back to when `ValueIsReadOnly` is unsupported.
- Events NVDA handles that Verbatim does not subscribe to: UIA
  description changes (`HelpText`), UIA live region changes and system
  alerts, and UIA elements added to or removed from a selection. MSAA
  description changes (`EVENT_OBJECT_DESCRIPTIONCHANGE`) on the focus,
  and state changes, a selection included, on the focus's ancestors:
  **matched since 2026-10-07** (reducer tests; the `msaa_events`
  mockapp test pins that a real tree view item's logical parent is not
  such an ancestor). An ancestor is recognized by the address it was
  reported at, as NVDA recognizes it: NVDA meets an event's object
  again only through its table of live objects, keyed by the address
  an object was created for or its identity string names, and its
  state-change gate asks whether the event's object is the very
  ancestor it holds, by Python identity, not whether it is equal by
  window, role, name, and location (that comparison redirects only an
  event on the focus). A windowless ancestor reached through
  `accParent`, with no identity string and no `IAccessible2` unique id,
  has no such address, so neither NVDA nor Verbatim speaks its state
  change (the `msaa_events` mockapp test pins it, since 2026-10-08).
  Its COM identity cannot serve either: oleacc hands every object to
  another process in a new wrapper of its own, so two sightings of one
  object never share an `IUnknown`, as measured against mockapp, whose
  provider keeps one object per node (`docs/crates/verbatim-ia2.md`,
  "Identity"). Verbatim does not read `IAccessible2` unique ids yet, by
  which NVDA keys an `IAccessible2` object's address. UIA delivers no ancestor's events to the
  focus-following registration ("UIA event registration"), so a UIA
  ancestor's state change is still not spoken. The ancestors are NVDA's, those
  reached through `accParent`: a tree view item's logical parents,
  which Verbatim reads among the focus's ancestors through the
  control's own messages, are not NVDA's ancestors, whose tree view
  items are the control's children, so their state changes and
  selections are not spoken (since 2026-10-08, after Left Arrow to a
  tree item's parent said "selected" before the parent; the
  `native_controls` mockapp test, on a real tree view).

### An ancestor's state change observed just before a focus change

NVDA
  handles events in the order they came (`orderedWinEventLimiter`'s
  `flushEvents` keeps one heap ordered by arrival), so it judges the
  change against the focus it was observed under and speaks it
  (`event_stateChange`, reason CHANGE). The focus change does not cancel
  that speech within a window: only speech spoken for the focus carries
  the `FocusLossCancellableSpeechCommand` that `doPreGainFocus`'s
  `removeCancelledSpeechCommands` culls
  (`eventHandler._getFocusLossCancellableSpeechCommand` returns none for
  any other reason), and `event_gainFocus` cancels nothing. A focus
  change into another top-level window does: `doPreGainFocus` runs the
  foreground event, whose `event_foreground` calls `cancelSpeech`, and
  entering a menu does too (`event_focusEntered`). So NVDA says
  "unavailable" and then the new focus within a window, and effectively
  nothing of it when the focus leaves the window or enters a menu.
  Verbatim: **matched since 2026-10-08**, by arrival order since
  2026-10-11: the outpost handles events in the order they reach it, so
  such a change is handled before a focus change that reached it later,
  and Core's cancelling, NVDA's, does the rest (the `msaa_events` mockapp
  tests, each in both batchings, a move within the window and one into a
  menu, and a reducer test for a move into another window). From
  2026-10-08 to 2026-10-11 the intake moved the events of objects other
  than the focus behind a focus change for latency, with an exception
  for such a change observed before it. Until 2026-10-11 (stage A step
  5 in `docs/roadmap.md`) one gap remained: the focus reached the outpost
  from the listener in another process and the change from the
  outpost's own hook, so a change raised just before the focus could
  reach the outpost after it and be judged against the new focus, which
  NVDA, with one hook and one queue, never does. Now the outpost's own
  hook takes the MSAA focus too, so both reach it in the order Windows
  raised them, as in NVDA; the listener's copy of a focus still reaches
  an outpost whose hook went in only moments before, and is matched with
  the hook's copy by counting.

### A top-level window's state change

Opening a modal dialog disables
  its owner, raising a state change on the owner's client area while
  the focus is still inside it, on an ancestor NVDA holds. NVDA says
  nothing: live captures on 2026-10-08 of Verbatim's Remove Theme
  confirmation opened eleven times, from the Remove button reached by
  Tab and after returning from the confirmation, never said
  "unavailable". The rule that silences it is in how NVDA meets the
  event's object again. Its base `event_stateChange` speaks for an
  ancestor only when the event's object is the very ancestor instance
  (`any(self is obj for obj in api.getFocusAncestors())`), and the
  event finds an existing instance only through `liveNVDAObjectTable`,
  keyed by its address (`IAccessibleHandler.winEventToNVDAEvent`), in
  which each new instance with that address replaces the last one
  (`IAccessible.__init__`, at the end). When the focus enters a
  top-level window, `doPreGainFocus` first sets the focus and its
  ancestors (`api.setFocusObject`, whose walk creates the window's
  ancestor instance), then, the focus difference level being 1 or less,
  creates the foreground object (`objectInForeground`, an object for
  the foreground window's `OBJID_CLIENT`), which takes the table's
  entry for that address and is held as the foreground. So the event
  on the window resolves to the foreground object, which is not among
  the ancestors, and is silent. A top-level window that was not the
  foreground as the focus entered it, as a popup menu's never is, keeps
  its ancestor instance, and its state change is spoken. Verbatim:
  **matched since 2026-10-08**: the outpost records, as the focus enters
  a top-level window, whether that window is the foreground, and while it
  is, does not count the window's own object, its client area or window
  object, among the ancestors whose state changes are spoken (the
  `msaa_events` mockapp tests, one each way, and the `theme_panel`
  scenario). Before, whether Verbatim said "unavailable" depended on
  timing: the intake moves the events of objects that are neither the
  focus nor the object a focus change moves to after that focus change
  when both are in one batch, so the owner's change was silent when it
  shared a batch with the confirmation's focus and spoken when it came
  in a batch of its own.

### Dialog text, which NVDA reads on entering a dialog

Dialog text, which NVDA reads on entering a dialog: a message box's
  question, read after the dialog's title and role and before its
  focused button. **Matched since 2026-10-07** ("A dialog's own text" in
  [Object model](../nvda/object-model.md)): the outpost gathers the text by
  NVDA's rules for a dialog, alert, or property page the focus newly
  enters, or one reported as the foreground window, and reports it as
  the dialog's description, as NVDA's dialog class does, so it is spoken
  where the description is and only while descriptions are (the
  `theme_panel` scenario's Remove confirmation). **Different:** NVDA
  reads a child window's client area through whichever API it uses for
  that window, where Verbatim reads a dialog's children in the dialog's
  own API; an owner-drawn static text with no name, which NVDA reads
  from the screen, gives nothing; the Windows Installer's dialog class,
  and web dialogs, are not recognized; and a dialog with more than 512
  objects says nothing.
