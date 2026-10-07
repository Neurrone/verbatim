# NVDAObjects: the normalized object model

Every UI node NVDA touches, from any API, is wrapped in an `NVDAObject` —
one Python type exposing one vocabulary of properties (name, role, states,
value, location, …) regardless of whether MSAA, IA2, UIA, or the Java
bridge is underneath. The interesting machinery is *how* the wrapping
class is chosen and composed per node, and the presentation properties
that drive announcement decisions.

## Lazy properties

`NVDAObject` derives from `baseObject.AutoPropertyObject`
(`source/baseObject.py`): any `_get_foo` method is exposed as property
`foo`, computed on first access and *cached for the current core pump
cycle* (the cache is invalidated between pumps). So "read `obj.name`
twice" costs one cross-process call, but nothing is fetched until asked
for. There is no snapshotting: each first access is a live call into the
app at announcement time.

## Dynamic class composition

`NVDAObjects.DynamicNVDAObjectType` (`source/NVDAObjects/__init__.py`) is
the metaclass that builds each instance's class on the fly:

1. The *API class* is chosen by the constructor kwargs (an
   `IAccessible`-based object, a `UIA` object, and so on — each API
   front end constructs its own subtype; `findBestAPIClass` can also
   re-decide, letting one API hand off to another for a given window).
2. The API class's `findOverlayClasses(clsList)` appends *overlay
   classes*: refinements for particular controls, keyed on window class
   name, role, or other cheap probes (for example the SysListView32 or
   Chromium document overlays). The base API class lands at the end.
3. The object's app module's `chooseNVDAObjectOverlayClasses(obj,
   clsList)` and every global plugin's hook may prepend further overlays
   (`appModuleHandler.AppModule.chooseNVDAObjectOverlayClasses`) — this
   is the primary customization point for per-app behavior.
4. If Windows is locked, `LockScreenObject` is forced to the front
   (security: it suppresses interaction with below-lock content).
5. The final class list is deduplicated into a Python MRO and a dynamic
   class named `Dynamic_<Overlay names>` is created and cached
   (`_dynamicClassCache`); `initOverlayClass` runs per overlay.

The effect: behavior for "a list item, in a SysListView32, in Explorer"
is the *stack* of those three classes, each able to override any
property or event handler, resolved by Python method resolution order.
This composition-by-class-list is NVDA's central extension mechanism,
and its subtlety is ordering: whoever is earlier in `clsList` wins.

## The property vocabulary

The normalized surface (all on `NVDAObject`): `name`, `role` (the
`controlTypes.Role` enum) plus `roleText` (author-supplied role
override), `states` (a set of `controlTypes.State`), `value`,
`description`, `keyboardShortcut`, `location` (screen rectangle),
`positionInfo` (index/level/similar-items-in-group), `parent`, `next`,
`previous`, `firstChild`, `children`, `treeInterceptor`, `appModule`,
`windowHandle` and the window* properties, `TextInfo` (the class used
for its text; [TextInfo](text-infos.md)), plus identity: `isDuplicateIRelevant`
comparisons go through `__eq__`, which each API implements from its own
identity primitives (IA2 uniqueID, UIA runtime ID, MSAA heuristics).

Events arrive as `event_<name>` methods on these same objects
([Event handling](events.md)), so overlays override announcement behavior by overriding
event handlers. The base handlers gate property-change announcements
on the object being the focus (states: focus or a focus ancestor) —
the focus-gate rule detailed in [Event handling](events.md).

## Presentation classification

Two derived properties drive most "should this thing be spoken"
decisions:

- `presentationType` (`NVDAObjects/__init__.py`,
  `_get_presentationType`): classifies a node as `content`, `layout`, or
  `unavailable`. The rules: invisible state means unavailable; landmarks
  with landmark reporting off are layout; a nonempty `roleText` forces
  content; static text is content only if its text is nonempty and not
  whitespace; a fixed role set (unknown, pane, text frame, root/layered/
  scroll/split pane, section, paragraph, title bar, label, whitespace,
  border) is always layout; and window/panel/property-page/grouping-ish
  roles are layout *when they have neither name nor description*
  (whitespace-only names are treated as absent — an Office workaround).
- `isPresentableFocusAncestor` (`_get_isPresentableFocusAncestor`):
  whether the node is announced when entered as part of the focus
  ancestry ([Event handling](events.md) fires `focusEntered` per new ancestor; this
  filters which of those speak). Layout and unavailable presentation
  types are excluded, and so are tree view items, list items, progress
  bars, and editable text — exclusion-based, so any *other* role that
  survives `presentationType` is presented, named or not.

These two functions are the precise reference for focus-context
announcement parity: they define which containers NVDA speaks when focus
dives into a dialog, and in what sense "simple" filtering happens even
outside simple review ([Focus and the navigator](focus-and-navigator.md) covers the separate
simple-review tree filter built on `presentationType`).

## Creation entry points

Objects are minted by: each API handler's event resolution (winevent
triple or UIA element to NVDAObject; [MSAA and winevent handling](msaa.md), [The UIA client](uia.md)),
`api.getDesktopObject()` and tree walking from it, hit testing
(`NVDAObjects.NVDAObject.objectFromPoint` via each API's point
resolution), and `objectWithFocus` (the API-specific "who has focus now"
query used at startup and for recovery, distinct from event-driven focus
tracking). Object construction can fail (`InvalidNVDAObject`) and
callers treat that as "node vanished" — the routine churn of dying
windows.

## The shared behavior mixins

`source/NVDAObjects/behaviors.py` is where a large share of NVDA's
*recognizable announcements* actually live: API-agnostic overlay
mixins that API classes and app modules attach to matching nodes.
The inventory, because parity work will meet every one:

- `ProgressBar` — progress reporting: speak percentages, beep with
  pitch encoding progress, or both, per the output mode config;
  covers *background* progress bars when that option is on (the
  event acceptance exception in [Event handling](events.md)). The
  details are in "How a progress bar reports its value" below.
- `Dialog` — dialog text harvesting: on focus entry, collects and
  speaks the dialog's static text children (the reason NVDA reads a
  message box's message, which is not the focused button's name).
  `WebDialog` adapts the same for web content.
- `LiveText` — the diff-and-speak engine: buffers text change
  notifications, diffs old against new (`diffHandler`;
  [Editable text and terminals](editable-text-and-terminals.md)),
  and speaks additions. `Terminal` composes it with editable text;
  the `EnhancedTermTypedCharSupport` variants suppress double-echo
  of the user's own typing in terminals.
- `EditableText` variants — the caret-key machinery
  ([Editable text and terminals](editable-text-and-terminals.md))
  plus *auto-select detection* (`EditableTextWithAutoSelectDetection`):
  detecting selection changes by comparison for controls that fire
  no selection events.
- `InputFieldWithSuggestions` — the suggestion sounds: earcons when
  a search box's suggestion list appears and disappears, plus
  suggestion-count reporting.
- `CandidateItem` — IME candidate presentation
  ([Keyboard input](input.md)).
- `RowWithFakeNavigation`, `RowWithoutCellObjects`, `_FakeTableCell`
  — table-cell navigation (Ctrl+Alt+arrows) synthesized for list
  rows whose cells are not real accessibility objects (SysListView32
  in report view, and friends).
- `ToolTip` and `Notification` — tooltip and toast/balloon
  reporting, gated by the object presentation settings
  ([Event handling](events.md)).
- `FocusableUnfocusableContainer` — the workaround mixin for
  containers that take focus but shouldn't present it.

### How a progress bar reports its value

Every progress bar NVDA reads through MSAA, UIA, or the Java Access
Bridge has the `ProgressBar` behavior, which decides what its value
changes say. The focus gate does not apply: a progress bar's value
change is reported whether or not it has the focus.

- Nothing is reported, and the change goes to the ordinary value change
  handling (speech for the focus only), when progress bar output is off,
  or the progress bar is invisible or off screen, or its value is not a
  number. The value is read as a number after any percent sign and null
  characters at either end are removed, and held between 0 and 100.
- A progress bar in a background window says nothing unless "report
  background progress bars" is on, which it is not by default; such an
  event is let through acceptance only for that option.
- Otherwise the percentage is reported, and the ordinary value handling
  is skipped, so a focused progress bar does not also speak its value.
  With the default output mode, beep, a 40 millisecond tone sounds whose
  pitch rises with the percentage (from 110 Hz, doubling every 25
  percent), but only when the percentage differs by at least the beep
  interval, 1 percent by default, from the last one beeped. With the
  speak mode, "N percent" is spoken when it differs by at least the
  speech interval, 10 percent by default. With both, each happens on its
  own interval.
- What was last reported is remembered per progress bar by the centre of
  its location on the screen, not by the object, so a progress bar that
  is replaced by another in the same place carries on from where the
  last one was.

### A dialog's own text

A dialog's description, in NVDA, is the dialog's own text, unless the
dialog supplies a description of its own that is not blank. Because it is
the description, it is spoken wherever a description is: after the
dialog's name, role, and states, whether the dialog is announced as the
focus, as the foreground window, or as a container the focus entered, and
only while object descriptions are reported. A message box therefore says
its title, "dialog", and its question, and then its focused button. (`Dialog`
in `NVDAObjects/behaviors.py`, its `_get_description` and `getDialogText`.)

What counts as a dialog: through MSAA, any object whose role is dialog,
alert, or property page, and the client area of a Windows Installer dialog
(window class `MsiDialogCloseClass`); through UI Automation, an element that
says it is a dialog (`IsDialog`), or a window element whose class is one of
NVDA's dialog classes (`#32770`, `NUIDialog`, the UAC dialog's host, and the
shell's dialog and flyout classes). Web dialogs have a variant of their own,
not covered here.

The text is gathered from the dialog's children, in order:

- A child that is invisible or unavailable is skipped, with everything
  inside it.
- A child that is a pane, panel, property page, option pane, window,
  grouping, paragraph, section, text frame, or of unknown role is a
  container: the gathering goes on inside it, and what it finds there is
  added as one piece. A container that is itself a dialog, such as a
  property page, gives no text when a child of it that would otherwise be
  considered has the focus, and the dialog it sits in then gives no text
  either. This keeps a property page's text from being read twice, by the
  dialog and by the page, while the focus is inside the page.
- Of the other children, only static texts, labels, links, and read-only
  edit fields that are not multi-line give text.
- A text right after a grouping, or right after a graphic that is right
  after a grouping, is skipped: it is taken to be the grouping's
  description.
- A named text whose next sibling has the same name is skipped as that
  sibling's label, unless the sibling is a graphic, static text, separator,
  window, pane, or button.
- A static text, label, or link gives its name, value, and description,
  joined by spaces, leaving out any that are empty or blank. A read-only
  edit field gives its name and then its text; when its text is empty or
  blank, it gives its name, value, and description, as a static text does.

The pieces are joined by line breaks, which speech turns into spaces
([Speech](speech.md), "Line breaks in spoken text"). Through MSAA, a child
that is a window object stands for that window's client area, read through
whichever API NVDA uses for the window, so a dialog's buttons and texts in
windows of their own are seen with their own roles rather than as windows.
The text is gathered each time the description is asked for, which is each
time the dialog is announced.

The design fact that matters: these are *mixins keyed by role and
context, not per-app code* — NVDA's per-app modules mostly just
attach or tune them. A normalized-model equivalent needs a home for
exactly this layer: behavior that is neither backend mapping nor
per-app, but role-shaped presentation logic.
