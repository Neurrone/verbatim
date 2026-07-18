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
  event acceptance exception in [Event handling](events.md)).
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

The design fact that matters: these are *mixins keyed by role and
context, not per-app code* — NVDA's per-app modules mostly just
attach or tune them. A normalized-model equivalent needs a home for
exactly this layer: behavior that is neither backend mapping nor
per-app, but role-shaped presentation logic.
