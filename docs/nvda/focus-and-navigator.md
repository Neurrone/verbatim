# Focus, the navigator object, and object navigation

NVDA maintains several cursors at once; this file covers the two
object-level ones — the *focus object* (where the system focus is) and
the *navigator object* (where the user's exploration is) — and object
navigation in both simple-review states. Text-level review positions
are [Review modes](review-modes.md).

## The state, all in api.py

`source/api.py` holds globals with typed accessors:

- Focus: `getFocusObject` / `setFocusObject` (set only by event
  processing; [Event handling](events.md) describes the ancestry computation),
  `getFocusAncestors`, `getFocusDifferenceLevel`.
- Foreground: `getForegroundObject` / `setForegroundObject`.
- Navigator: `getNavigatorObject` / `setNavigatorObject(obj,
  isFocus=False)`. **Review follows navigator**: setting the navigator
  re-seeds the review position to the start of the object's text
  (`setNavigatorObject` resets `reviewPosition`); and **navigator
  follows focus** when config `reviewCursor.followFocus` is on (the
  default): `setFocusObject` re-points the navigator (and review) at
  each focus change. Both setters refuse objects below the lock screen
  while locked (security).
- Mouse and desktop objects round out the set.

The design invariant: focus is *event-authoritative* (NVDA believes
focus events, with the recovery paths in [MSAA and winevent handling](msaa.md)/[The UIA client](uia.md) when apps
lie), while the navigator is *user-authoritative* — commands move it
and nothing else, except the follow-focus coupling.

## Object navigation commands

`source/globalCommands.py`, the `script_navigatorObject_*` family:
current (report; repeated presses spell and copy), parent, next,
previous, firstChild, toFocus (navigator snaps to focus),
moveFocus (focus snaps to navigator; second press moves the caret to
the review position), and dimension/location reports. Each movement
announces the landed-on object via
`speech.speakObject(obj, reason=OutputReason.FOCUS)`-style reporting;
hitting an edge reports "No parent" / "No next" etc. and stays put.

## Simple review off: the raw tree

With `reviewCursor.simpleReviewMode` disabled, the movement commands
use the raw properties `parent`, `next`, `previous`, `firstChild` —
the unfiltered accessibility tree exactly as the backend reports it
(each script checks the flag: e.g. `script_navigatorObject_parent`
uses `curObject.simpleParent if simpleReviewMode else curObject.parent`).

## Simple review on: the filtered tree (the default)

With the flag on (NVDA's out-of-box default), movement uses the
`simple*` properties (`source/NVDAObjects/__init__.py`,
`_get_simpleParent`, `_get_simpleNext`, `_get_simplePrevious`,
`_get_simpleFirstChild`, `_get_simpleLastChild`), which present a
*virtual tree containing only content-classified nodes*
(`presentationType == content`; [Object model](object-model.md) defines the
classification):

- `simpleParent`: walk real parents until one is content.
- `simpleFirstChild` / `simpleLastChild`: the first/last real child,
  *promoted through* layout nodes — if the child is layout, descend
  into it (`_findSimpleNext(useChild=True…)`) to find the first
  content descendant. Layout containers are thus dissolved: their
  content children appear as direct children of the nearest content
  ancestor.
- `simpleNext` / `simplePrevious` (`_findSimpleNext`): try the real
  sibling; a layout sibling is entered (its first content descendant
  is the answer); if siblings run out, climb through layout parents
  and continue from them — so navigation *flows out of* dissolved
  containers seamlessly. Unavailable (invisible) nodes are skipped
  without descending.

Two properties of this algorithm worth knowing: it can make many
property calls per keypress (each candidate's `presentationType`
computes role, states, sometimes text), and because
`presentationType` consults config (table and landmark reporting
toggles), *the simple tree's shape changes with user settings*.

## Related recovery behavior

Because the navigator can point at a node that dies (window closed,
DOM mutated), commands defensively check `isAlive`-ish failure on use;
NVDA's general posture is to report "No object" style messages rather
than auto-relocating the navigator — the navigator only moves when the
user moves it, focus moves it via follow-focus, or an explicit toFocus
runs. Focus recovery after app crashes similarly falls back to
re-querying the real focus (`api.getDesktopObject().objectWithFocus()`
paths) rather than trusting stale state.
