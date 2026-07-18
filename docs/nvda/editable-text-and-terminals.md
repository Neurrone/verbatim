# Editable text and terminals

How NVDA reports typing, caret movement, and terminal output — the
behaviors a screen reader's text milestone lives or dies by.

## The EditableText mixin

`source/editableText.py` (`EditableText`, mixed into editable
NVDAObjects) implements caret-key reporting with a design premise
worth stating plainly: *when the user presses an arrow key, NVDA
passes the key to the app and then must find out what happened* — it
does not move any cursor itself.

- The `script_caret_*` family (moveByLine/Character/Word/Paragraph,
  sentence variants, home/end, backspace variants, delete) each:
  save a bookmark of the current caret TextInfo, `gesture.send()` the
  real key, then wait for evidence of change — `_hasCaretMoved`
  polls the caret position against the bookmark on a
  retry-with-timeout loop (10 ms steps, timeout scaled by
  `_caretMovementTimeoutMultiplier`, longer when caret events exist
  but haven't arrived) — then speak the new unit (line after
  up/down, character after left/right, deleted character after
  delete, and so on).
- `_hasCaretMoved` has three short-circuits that matter as much as
  the timeout: a pending `gainFocus` event returns "moved"
  immediately (focus is about to be announced anyway, so the
  caret-line announcement yields to it); `isScriptWaiting()` — a
  newer keystroke already queued — returns "not moved" so the wait
  aborts rather than lag behind fast typing; and for Delete, the
  caret often *does not move*, so the word at the caret is compared
  against the pre-keystroke `origWord` after a minimum wait and a
  word change counts as evidence.
- When the caret genuinely refuses to move, the scripts fire a
  `caretMovementFailed` event (opt-in via
  `shouldFireCaretMovementFailedEvents`, default off) — the hook
  document implementations use to detect and react to document edges
  rather than staying silent.
- Backends with reliable caret events shortcut the polling
  (`caretMovementDetectionUsesEvents` — the `caret` event from
  IA2/UIA arrives and ends the wait); backends without get the
  polling loop against their TextInfo. This hybrid is the
  compatibility story for the whole Win32 world.
- Selection changes: `reportSelectionChange` compares old and new
  selection TextInfos and speaks "selected X" / "unselected X"
  deltas (`speech.speakSelectionChange`); typed text echo is
  separate ([Keyboard input](input.md) — typed characters arrive via the injected
  reports or UIA textEdit events, filtered against the actual
  control's text where possible to suppress phantom echo from
  autocomplete rewriting).

## The per-control text backend ladder

"An edit control" is not one thing; NVDA maintains a ladder of text
backends and picks per window class (overlay selection;
[Object model](object-model.md)):

- **Plain and rich edit via window messages**
  (`source/NVDAObjects/window/edit.py`, `EditTextInfo`): an
  offsets-based TextInfo over the `EM_*` protocol —
  `EM_GETSEL`/`EM_SETSEL` for caret and selection, `EM_LINEINDEX`
  and friends for line structure, `EM_GETTEXTRANGE` plus
  `CharFormat2W` structures for rich-edit text and formatting. Every
  primitive is a cross-process `SendMessage`, routed through
  `watchdog.cancellableSendMessage`
  ([Main loop and watchdog](main-loop-and-watchdog.md)).
- **The Text Object Model (TOM)** (`ITextDocumentTextInfo`, same
  file): rich edit controls expose the `ITextDocument`/`ITextRange`
  COM interfaces (obtained via `EM_GETOLEINTERFACE`), a real
  range-based text API with formatting — preferred over raw `EM_*`
  when present ("unidentified rich edit fields will most likely use
  ITextDocumentTextInfo", per the code's own comment). TOM is a
  documented Windows API worth knowing exists: it is the richest
  no-injection path for classic rich text.
- **Scintilla** (`source/NVDAObjects/window/scintilla.py`): the
  editor component under Notepad++ and many dev tools has its own
  message protocol (`SCI_*`), transcribed as another offsets
  TextInfo.
- **IA2 text**, **UIA text pattern**, and the **Office object
  models** cover the modern and app-specific ends
  ([IA2 usage](ia2.md), [The UIA client](uia.md),
  [Office through COM](office-com.md)).

The behavior layer above (`EditableText`, the caret-key scripts) is
identical across all of these; only the TextInfo differs — the
clean seam that makes the ladder maintainable.

## Terminals

Terminals are documents that rewrite themselves constantly; NVDA has
two eras of support:

- **Legacy consoles** (`source/winConsoleHandler.py`): attach to the
  console with console APIs — `AttachConsole` after a
  connect (`connectConsole`), read the visible screen buffer
  (`getConsoleVisibleLines`) through `wincon` APIs, and use a
  console winevent hook plus polling to produce *diffs*: new text is
  computed by comparing before/after screen snapshots and spoken.
  A control-handler guards against the console dying underneath
  (`isConsoleDead`).
- **UIA consoles/terminals**
  (`source/NVDAObjects/UIA/winConsoleUIA.py`, plus Windows
  Terminal's UIA): the console exposes a UIA Text pattern; caret
  routing and review use UIA text ranges (with substantial per-build
  workarounds for the console's early UIA bugs, visible throughout
  that file), and *new output* arrives as UIA textChange
  events feeding the same diffing layer.

The diffing layer (`source/diffHandler.py`) is shared: given the
before and after text, it chooses per config between Difflib
(line-level, ordered) and DMP (diff-match-patch, character-level)
to extract what to speak — the "read new terminal output" feature
(`speakNewText`). Where the terminal implements UIA notifications
for output, those are preferred (the passive path).

Design note transferable to any implementation: terminal reporting is
explicitly *best-effort lossy* — under fast output NVDA coalesces
(diffing the latest state rather than every intermediate write), on
the theory that speaking every scroll of a compile log is worse than
speaking its current tail.
