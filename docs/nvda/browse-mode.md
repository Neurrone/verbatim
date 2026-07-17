# Browse mode and tree interceptors

Browse mode is the user-facing document model: a document becomes a
navigable text surface with single-letter quick navigation, and keys
either operate on that surface ("browse mode") or pass to the
application ("focus mode" / pass-through). The machinery is the *tree
interceptor* (`source/treeInterceptorHandler.py`,
`source/browseMode.py`); virtual buffers ([Virtual buffers](virtual-buffers.md)) are one
implementation, UIA documents another.

## Tree interceptors

A `TreeInterceptor` claims an NVDAObject subtree: while alive, events
and scripts for any object within the root's subtree are offered to
the interceptor before the object ([Event handling](events.md), [Keyboard input](input.md)).
`treeInterceptorHandler.update(obj)` creates one when an object's
`treeInterceptorClass` wants it (documents advertise this; the class
factory is how Gecko documents get a `VirtualBuffer`, UIA documents a
UIA `BrowseModeDocumentTreeInterceptor`, Word documents theirs), keeps
a runningTable, and kills interceptors whose roots die
(`_get_isAlive`, `killTreeInterceptor`).

`BrowseModeTreeInterceptor` (`browseMode.py`) adds the mode logic;
`BrowseModeDocumentTreeInterceptor` adds a caret over a document
(`CursorManager` mixed over the interceptor's TextInfo;
[Review modes](review-modes.md) and [TextInfo](text-infos.md)), selection, and say-all
integration. `VirtualBuffer` and the UIA browse mode
(`source/UIAHandler/browseMode.py`) both derive from it.

## Pass-through: the two-mode dance

`passThrough` is the boolean; the rules around it are the feature:

- Gestures: NVDA+space toggles (`script_passThrough`); Escape returns
  to browse mode (`script_disablePassThrough`); activation keys
  (Enter variants, Applications, Shift+F10) are bound to pass through
  always (`__gestures` in `BrowseModeTreeInterceptor`). In browse
  mode, plain character keys are *trapped* (`script_trapNonCommandGesture`)
  so typing does not leak into the app.
- Automatic switching: on focus changes and caret moves,
  `shouldPassThrough(obj, reason)` decides per object: focusable
  editable fields, combo boxes, applications-role subtrees and the
  like flip to focus mode when *focused* (config
  `autoFocusFocusableElements` and friends modulate when quick nav or
  caret movement causes focus); read-only controls do not flip.
  `disableAutoPassThrough` (set by explicit user toggling) suppresses
  the automation until the user re-enables it.
- Every flip is announced by sound (browseMode.wav / focusMode.wav)
  or speech (`reportPassThrough`).

The subtle contract Verbatim should note: in browse mode the
*application still has real focus somewhere*; NVDA maintains a
separation between the browse caret and the system focus/caret,
syncing them in both directions (caret follows browse position when
`autoFocusFocusableElements` allows; browse position follows real
focus events) — most browse-mode bugs in any screen reader are
failures of this bidirectional sync.

## Quick navigation

Single letters (h heading, k link, b button, f form field, t table, …
shifted for backward) are scripts on the interceptor mapping to
`_iterNodesByType(itemType, direction, position)` — implemented by
each document type: attribute searches in the vbuf storage
([Virtual buffers](virtual-buffers.md)), UIA property/custom searches for UIA
documents. Results are `QuickNavItem`s (`browseMode.py`) that report
themselves and move the caret. The elements list (NVDA+F7,
`script_elementsList`) is the dialog over the same iteration. Table
navigation (Ctrl+Alt+arrows) similarly delegates to per-document cell
resolution.

## Documents without buffers

The UIA browse-mode implementation
(`source/UIAHandler/browseMode.py`) reuses all of the above with
TextInfo primitives on live UIA text ranges instead of a cached
buffer: no injection, no staleness, but every movement is a live
cross-process call (mitigated by remote ops for bulk reads;
[UIA remote operations](uia-remote-ops.md)). Word's classic browse mode similarly rides the
object model. The mode logic, quick nav vocabulary, and pass-through
behavior are identical across implementations — the differences are
purely in the TextInfo/node-iteration substrate.
