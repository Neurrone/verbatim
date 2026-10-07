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
current (report; repeated presses spell and copy the name and the value
joined by a space, or for an object with a real text interface the name
and its selected text or caret line, as "What an object with text says" in
[Speech](speech.md) explains), parent, next,
previous, firstChild, toFocus (navigator snaps to focus, after saying
"Move to focus"),
moveFocus (focus snaps to navigator; second press moves the caret to
the review position), and dimension/location reports. Each movement
announces the landed-on object via
`speech.speakObject(obj, reason=OutputReason.FOCUS)`-style reporting;
hitting an edge reports "No parent" / "No next" etc. and stays put.

## Reporting the focus

NVDA+Tab, bound for every layout, reports the focus object, wherever the
navigator is, and moves neither the navigator nor the review cursor.

- Pressed once, it speaks the focus exactly as reporting the current
  object speaks the navigator object on its first press: the object
  alone, without the containers above it, as a query, so the role is
  always spoken and every state is (focused, and selected on a list item,
  included), with the value, description, keyboard shortcut and position.
  An object with text says its text in place of its value, as a focus
  does: "selected" and the selected text when there is a selection,
  otherwise the line at the caret, "blank" for an empty one ("What an
  object with text says" in [Speech](speech.md)).
- Pressed twice, it spells the focus object's name, and only the name:
  not its value, and not its selected text or caret line, which
  reporting the current object a second time spells with the name. An
  object with no name spells nothing but says "blank".
- Pressed three or more times, it spells the name with character
  descriptions ("Alpha" for a). Nothing is ever copied to the clipboard,
  which reporting the current object does on its third press.
- With no focus object at all, it says "No focus".

(`script_reportCurrentFocus` in `globalCommands.py`, with
`getObjectSpeech` and `speakSpelling` in `speech/speech.py`.)

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
