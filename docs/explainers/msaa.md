# MSAA: Microsoft Active Accessibility

MSAA is the 1997-era accessibility API: one COM interface, `IAccessible`,
plus WinEvents for notification. It survives because the Win32 common
controls implement it natively and countless legacy apps expose nothing
better. This file describes the API as Windows defines it; NVDA's usage is in
[MSAA and winevent handling](../nvda/msaa.md).

## The object model

An MSAA node is an `IAccessible` pointer *plus a child ID*. The pair is the
unit of identity:

- `CHILDID_SELF` (0) means the object itself.
- A positive integer means the Nth *simple element* — a child that has no
  `IAccessible` of its own (list items and tree items in Win32 controls are
  simple elements). All property getters take the child ID as a `VARIANT`,
  so one interface pointer can represent a whole control and its thousands
  of children.

That design decision has consequences you will keep meeting: a simple
element cannot be QueryInterfaced for anything (it is not an object), and
there is no persistent, comparable node identity — MSAA gives you no
`IsSameObject`; clients compare by (window, role, location, name) heuristics
and get it wrong at the margins.

## Properties and methods

The interface is small; the properties are per-child:

- `get_accName`, `get_accRole` (a `VARIANT`: usually an integer from the
  `ROLE_SYSTEM_*` set of about 60 — `PUSHBUTTON`, `LISTITEM`, `OUTLINE` for
  tree views…), `get_accState` (a 32-bit mask of `STATE_SYSTEM_*`:
  `FOCUSED`, `CHECKED`, `INVISIBLE`, `OFFSCREEN`, `UNAVAILABLE`, …),
  `get_accValue` (a string — sliders, edits), `get_accDescription`,
  `get_accKeyboardShortcut`, `get_accDefaultAction` / `accDoDefaultAction`,
  `accLocation` (screen rectangle), `get_accFocus`, `get_accSelection`.
- Structure: `get_accParent`, `get_accChildCount`, `get_accChild`, and
  `accNavigate(direction, startChild)` for first/last/next/previous
  sibling navigation. Implementations of `accNavigate` are notoriously
  unreliable in real apps — returning nothing, or the wrong node, or
  *the same node* — which is why clients layer fallbacks over it.
- `accHitTest(x, y)` for point-to-node resolution.

There is no text model beyond the single `accValue`/`accName` strings: no
character offsets, no formatting, no caret. (IA2 exists to fix exactly
this; the system *caret* is separately observable via winevents —
`EVENT_OBJECT_LOCATIONCHANGE` on `OBJID_CARET`.)

## Getting an object

- `AccessibleObjectFromWindow(hwnd, objid, iid, out)` — the workhorse.
  `objid` selects which of a window's accessibility surfaces you want:
  `OBJID_CLIENT` (the content — almost always what you want),
  `OBJID_WINDOW` (the frame), `OBJID_SYSMENU`, `OBJID_CARET`, etc. Under
  the hood the system sends the window `WM_GETOBJECT` (a synchronous
  cross-process message — all the caveats of [Windows and messages](windows-and-messages.md))
  and marshals back whatever the app returns; if the app returns nothing,
  `oleacc.dll` fabricates a default implementation from the window class.
- `AccessibleObjectFromEvent(hwnd, objid, childid, …)` — resolve a winevent
  triple to a node.
- `AccessibleObjectFromPoint` — hit testing from screen coordinates.
- `WindowFromAccessibleObject` — back from a node to its `HWND`.

Servers implement MSAA by answering `WM_GETOBJECT`, typically via
`CreateStdAccessibleObject` (wrap the system default) or `LresultFromObject`
(marshal a custom implementation).

## Events

MSAA's notification channel is WinEvents ([Windows and messages](windows-and-messages.md)):
content-free triples that the client resolves and queries after the fact.
The event vocabulary is fixed: `EVENT_OBJECT_FOCUS`, `EVENT_OBJECT_SHOW` /
`HIDE` / `REORDER`, `EVENT_OBJECT_NAMECHANGE` / `VALUECHANGE` /
`STATECHANGE` / `SELECTION*`, `EVENT_SYSTEM_FOREGROUND`, `EVENT_SYSTEM_MENU*`,
`EVENT_SYSTEM_ALERT`, `EVENT_OBJECT_LOCATIONCHANGE`, and friends. Nothing
describes *what* changed — a `NAMECHANGE` does not carry the new name — so
every acted-on event costs synchronous re-query round trips.

## Working characteristics, honestly

- Cheap to obtain, universally present, and the *only* full-fidelity view
  of classic Win32 controls' accessibility.
- Every navigation step and property read is a separate blocking
  cross-process COM call into (usually) the app's UI-thread STA: slow at
  scale and hangs when the app hangs ([COM](com.md)).
- Identity, tree consistency, and `accNavigate` are unreliable in the wild;
  robust clients treat MSAA answers as hints to be cross-checked (window
  hierarchy, control-specific messages) rather than truth.
- The role/state vocabulary cannot express modern UI (no toggle switch, no
  tab-list distinction from list, no document structure); apps overload
  roles and clients special-case by window class.

## References

- [Microsoft Active Accessibility (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/winauto/microsoft-active-accessibility)
- `oleacc.h` — the header defining `ROLE_SYSTEM_*` and `STATE_SYSTEM_*`.
