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
  but haven't arrived; Windows Terminal's overlays in
  `NVDAObjects/UIA/winConsoleUIA.py` set the multiplier to 3.0 because
  its UIA caret can update after the default timeout, especially over
  SSH, commit `fdbb6c851`, #20398) — then speak the new unit (line after
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
  selection TextInfos and speaks the deltas (`speech.speakSelectionChange`)
  with the text first: "hello selected" and "hello unselected", a single
  character by its name ("comma unselected"), and 512 characters or more
  as their number ("600 characters selected"). Only text an object
  already has selected when it is announced puts the word first,
  "selected hello", as a warning that typing would replace it. Controls
  that report their selection through caret events (UIA text and the
  standard edit controls, `EditableTextWithAutoSelectDetection` in
  `NVDAObjects/behaviors.py`) have every selection change on the focus
  reported this way after the caret event, whatever caused it, so a plain
  arrow key that collapses a selection is spoken as its unit, then the
  text it unselected: Right Arrow after Control+A in Notepad says
  "blank", then "alpha beta gamma delta epsilon unselected". When the
  object's content changed since the selection it compares with was
  taken, the change is spoken in general terms, since the old selection's
  bounds may no longer hold the text that was selected
  (`speakSelectionChange`'s `generalize`, `speech/speech.py` 1340 to
  1417): if anything was selected, the whole new selection followed by
  "selected"; if anything was unselected, the whole new selection followed
  by "selected instead", or "selection removed" when nothing is selected
  now; a selection replaced by one apart from it gets both. The flag,
  `hasContentChangedSinceLastSelection`, is read and cleared by every
  comparison (`detectPossibleSelectionChange`, `editableText.py` 429 to
  451), which always takes the new selection as the one to compare with
  next and never forgets it on a text change. Who sets it differs by
  backend. A classic edit control sets it when its value change is
  handled (`NVDAObjects/IAccessible/__init__.py` 2019 to 2023) and at its
  caret event when a value change for it is still pending
  (`NVDAObjects/window/edit.py` 1068 to 1084), so either order of the two
  events gives the general words: in the Windows Forms text box, "x"
  typed over the selected "hello " says "x" then "selection removed", and
  Control+X on a selected line "selection removed". A UIA text object
  sets it only as a text change is handled, and NVDA handles UIA's text
  changed event as one only for Word, the console host and Windows
  Terminal (`UIAHandler/__init__.py` 145 to 151 and 778 to 799), so in
  Windows 11 Notepad the specific text is always compared, the old
  selection's range read at comparison time after it followed the edit:
  "x" typed over the selected "hello " says "x" then "x unselected", and
  Control+X on a selected line says nothing, the old range being empty
  (captures in `target/e2e-evidence/m42-desktop-2-2026-10-22/`,
  `part3b-*.txt`). Typed text echo is
  separate ([Keyboard input](input.md) — typed characters arrive via the injected
  reports or UIA textEdit events, filtered against the actual
  control's text where possible to suppress phantom echo from
  autocomplete rewriting).

### A line break as a character

NVDA's text units include each line's line break: a line's text runs up
to and including the break that ends it, and the character unit is one
UTF-16 code unit, so a carriage return and line feed are two characters
(`_getCharacterOffsets` in `textInfos/offsets.py`; the standard edit
control's lines in `NVDAObjects/window/edit.py` extend to the next line's
start). A line break is therefore something a character unit can land
on, and NVDA names it like any other character spoken on its own.

- How it is named. A unit of one character is spelled
  (`getTextInfoSpeech` in `speech/speech.py` spells a character or word
  unit whose text is one character, keeping whitespace as it is), and
  spelling names a character by the symbol table: in
  `locale/en/symbols.dic`, a carriage return is "carriage return" and a
  line feed "line feed". A unit of more than one character that is only
  spaces and line breaks is "blank" instead (`isBlank`), as is a unit
  with no text at all, such as the end of the text.
- The caret. End lands on the line's line break, so NVDA says "carriage
  return", both in Windows 11 Notepad, whose UIA text breaks lines with a
  carriage return alone, and in a standard edit control, whose lines end
  with a carriage return and line feed (End lands on the carriage
  return). Right Arrow onto the end of a line says the same. On the
  text's last line, which has no break, End lands at the end of the text
  and says "blank".
- The review cursor. End of line (Shift+Numpad 3) moves to the line's
  last character, its line break included: "carriage return" in Notepad,
  "line feed" in a standard edit control. Next character moves onto the
  break from the line's last letter, and at the line's last character
  says "Right" and that character again, since review movement by
  character stays within the line (`script_review_nextCharacter` and
  `script_review_endOfLine` in `globalCommands.py`).
- Backspace. NVDA reads the character before the caret before it sends
  the key, so at a line's start it reads the line break it is about to
  delete. It turns a carriage return and line feed into a line feed
  before speaking (`_backspaceScriptHelper` in `editableText.py`), so
  Backspace over a line break says "carriage return" in Notepad and "line
  feed" in a standard edit control.

NVDA transcripts on 2026-10-07 confirmed each of these in Windows 11
Notepad and a standard edit control.

### Protected (password) fields

NVDA reads a protected field's text through the same object
announcement, caret scripts and review commands as any field's; nothing
in its speech code tests the protected state when it reads text
(`getObjectSpeech`, `nvda/source/speech/speech.py` 865 to 923). What it
reads is masked below that, by the text backend:

- A classic edit or rich edit control with the protected state
  (`STATE_SYSTEM_PROTECTED`, which an `ES_PASSWORD` edit control has)
  has its text replaced by one "*" per character in NVDA's own
  `EditTextInfo`: the whole text (`_getStoryText`,
  `NVDAObjects/window/edit.py` 469 to 471), a rich edit's range, since
  some rich edit controls do not hide their password characters (#4095,
  579 to 582), and the Text Object Model's range (853 to 855).
  MSHTML's text does the same (`NVDAObjects/IAccessible/MSHTML.py` 398
  to 400).
- A UIA element whose `IsPassword` is true has the protected state
  (`NVDAObjects/UIA/__init__.py` 1997 to 1999), and its text pattern's
  text is read as the provider gives it, with no masking of NVDA's own
  (`UIATextInfo._get_text`, 1043 to 1044). A WPF `PasswordBox` gives one
  black circle (U+25CF) per character through its text pattern, and an
  empty value.

The words then go through symbol processing like any text. In English
"*" is "star" from the "some" level, NVDA's default, and a run of it is
collapsed to its count ("6 star"); "●" is "circle" only from the "most"
level, so a line of circles at the default level is dropped and says
nothing, while one character spoken on its own, as a caret moving by
character or a Backspace says it, uses the character level and says
"circle" (`nvda/source/locale/en/symbols.dic` lines 63 and 113;
[Symbols, dictionaries, and character processing](symbols-and-dictionaries.md)).
That is why, captured at the pinned build
(`target/e2e-evidence/m42-selection-keys-2026-10-22/`), Tab into a
Windows Forms password box holding six characters says "Password edit
protected selected 6 star", and Left Arrow and Backspace say "star",
while Tab into a WPF `PasswordBox` says only "Password edit protected",
and Left Arrow and Backspace say "circle".

Typed characters are the one place NVDA tests the state itself: when
the focus is protected (`isProtected`, the protected state or the
password edit role, kept for the object's life once seen,
`NVDAObjects/__init__.py` 1121 to 1136; `api.isTypingProtected`,
`nvda/source/api.py` 357), each typed character is echoed as "*",
`PROTECTED_CHAR`, and no typed word is spoken (`speakTypedCharacters`,
`speech/speech.py` 1431 and 1449 to 1469).

A caret script's wait for evidence takes a pending caret or text change
event within 60 ms of the key as evidence for a control whose caret
events it relies on, which is every control but a terminal, before it
compares the caret's bookmark with the one from before the key
(`_hasCaretMoved`, `nvda/source/editableText.py` 57, 100 to 138 and 194
to 206). Which of these finds Backspace's effect in a `PasswordBox`,
whose characters either side of the caret are the same circle before
and after, was not traced; the capture shows that one does.

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

### Rich edit text

A rich edit control (edit API version 2 or later) gives a range of text
through `EM_GETTEXTRANGE` into a buffer NVDA allocates in the control's
process, twice the range's length plus a terminator, so it holds either
character size. NVDA reads the whole buffer back. From a Unicode window
it decodes UTF-16. From any other window it does not trust the window's
flag: when more than one character was copied and any byte past the
count the message answered is not zero, the buffer holds UTF-16 and is
decoded so; otherwise it is ANSI text in the user's code page
(`textUtils.getTextFromRawBytes`, called with no encoding). One copied
character cannot be told apart and is read as ANSI.

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
  events feeding the same diffing layer. Windows Terminal's UIA
  provider reports the paragraph, page, and document units as spanning
  the whole buffer, so its TextInfo (`_WinTerminalUIATextInfo`) maps
  the mouse chunk from paragraph to line.

The diffing layer (`source/diffHandler.py`) is shared: given the
before and after text, it chooses per config between Difflib
(line-level, ordered) and DMP (diff-match-patch, character-level)
to extract what to speak — the "read new terminal output" feature.
`LiveText._getText` (`NVDAObjects/behaviors.py`) reads the object's
`POSITION_ALL` text, which for current (FORMATTED API level) consoles
and for Windows Terminal is the whole document, not the visible screen;
only older consoles bound it to the visible range
(`ConsoleUIATextInfo`), and those prefer Difflib
(`WinConsoleUIA._get_diffAlgo`). Everything else uses DMP by default
(`LiveText._get_diffAlgo` returns `prefer_dmp()`; the
`terminals.diffAlgo` setting can override either choice).

UIA notifications for terminal output are not the default path:

- The console host's notification events are blocked to avoid
  double reporting (`WinConsoleUIA.event_UIA_notification` only logs
  them).
- Windows Terminal's are used only under an opt-in feature flag,
  `terminals.wtStrategy` (default: diffing), which selects
  `_NotificationsBasedWinTerminalUIA` over `_DiffBasedWinTerminalUIA`;
  the diffing overlay blocks notifications too. The user guide warns
  that notification mode reports typed characters that are not shown
  on screen, such as passwords, and may not report contiguous spans of
  output over 1,000 characters accurately.

Design note transferable to any implementation: terminal reporting
coalesces — under fast output the monitor thread (`LiveText._monitor`)
diffs the latest state rather than every intermediate write, and drops
a diff that is a single character, on the assumption that it is a
typed character. It does not cap what it speaks: at this pin
`LiveText._reportNewLines` speaks every new line the diff produced.
A cap existed briefly: #20177 truncated a batch to its last
`MAX_LINES` (100) lines, beeping for the skipped ones, and spoke the
rest from a generator cancelled when speech was cancelled; #20649
made it configurable. Commit `0e48954c8` (#20898) reverted both,
because users could not read the skipped output.

### Terminal details at the pinned source

- The console host's text is read on UIA's text changes alone
  (`UIAHandler/__init__.py` 145 to 147 and 674 to 679).
- The diff speaks only what was inserted (`diffHandler.py` 36 to 48:
  `DiffMatchPatch.diff` keeps only the `+` pieces and drops white-space
  ones), so lines only deleted, and a line that only lost text or
  gained white space, say nothing.
- Windows Terminal's notifications and the console host's are blocked by
  the overlays (`winConsoleUIA.py` 431 to 440 and 474 to 476).
- Every new line is queued (`behaviors.py` 455 to 466), and the monitor
  thread diffs on every change (`behaviors.py` 468 to 505).
- Two probably-typed drops apply. The monitor thread deletes a diff
  that is one character after stripping white space (`behaviors.py` 491
  to 495). The terminal overlay's `_reportNewLines` (559 to 566) then
  drops a single new line whose stripped length is below the larger of
  3 and one more than the typed word so far, unless Tab was pressed,
  since completions can be short. So a short status-line change, such
  as vim's ruler, or a counter of one or two digits, is not spoken, and
  more is dropped while a word is being typed.
- Typed characters are queued and dispatched on the terminal's next text
  change of any kind (`behaviors.py` 591 to 593 and 615 to 619), so a
  character typed at a prompt that shows nothing is spoken when anything
  else on the line changes, such as a ticking clock or Escape clearing
  the line. The queue is forgotten on Enter, Tab, Control+C, Control+D,
  and Control+Pause (`behaviors.py` 595 to 613).
- The console host's keys are translated with the console host's own
  keyboard layout, its thread learned from the console window's
  `WinEvents` (`IAccessibleHandler/internalWinEventHandler.py` 151 to 153
  and 266 to 273, `NVDAObjects/UIA/winConsoleUIA.py` 378 to 388, issue
  10113).
- A terminal's caret events are not used (`NVDAObjects/behaviors.py`
  527 to 530): after a caret key the caret's line is read at the first
  change of the caret's position, or at the end of the wait
  (`editableText.py` 70 to 163 and 181 to 192, the line spoken by 165 to
  178). A key outside the caret keys says nothing.
- A focused terminal's caret line is read on focus, "blank" for an empty
  one (`speech/speech.py` 898 to 917 and 1929 to 1933).

### What NVDA says in terminals, captured live

Captured on 2026-10-09 with alpha-57645 in the console host and Windows
Terminal, and replayed on 2026-10-12 with alpha-58022, the build the
pinned source is, which says the same apart from timing:

- Every character typed is echoed, including a space at a line's end
  and characters past the right margin, and each key a script reads
  without showing it ("space"); a password typed at a prompt that
  shows asterisks is echoed character by character.
- At a prompt that shows nothing typed and rewrites its line on each
  key (a ticking clock), the typed characters are spoken ("a", "b",
  "c") and the line's count is not; Escape after characters typed at a
  prompt that showed nothing speaks them before the line's new text.
- Escape after `echo hi`, or after inline prediction showed `git
  status`, is silent.
- The prompt `ready>` is "ready greater"; Left Arrow says "o" and then
  "Oscar", the delayed character description.
- Opening the alternate screen speaks all thirty rows in Windows
  Terminal but only the last six in the console host; closing it speaks
  the whole main screen again.
- Switching to another terminal window says the window's title and
  "window", then the terminal and the caret's line as one utterance. In
  Windows Terminal a new window or tab with nothing written yet said no
  line in 17 of 19 captures, NVDA not recognising the terminal's focus;
  in the console host it said "blank" in all 14. Returning to a tab or
  window said the prompt in some captures and nothing in others.
- Up Arrow and a character typed at once: the character as typed, then
  the recalled line with it ("x", "echo onex").
- Control+Tab or Control+Shift+Tab in Windows Terminal queues "list",
  the tab (on Control+Tab the tab left first), then "<title> terminal".
  A single Control+Shift+Tab with Control released at once gives speech
  that varies with timing. With Control held through the tab switcher:
  "Tab switcher grouping", "list", the tab moved to ("<title> 1 of 2"),
  and on releasing Control the tab's terminal and its line.
- A list redrawn by a program with the caret left below it: "blank" for
  each key, and the marker's move dropped as probably typed.
- A pager's line move: the prompt, ":", at the end of the wait, then the
  new line.
- A flood of 10,000 numbered lines (2026-10-07, an earlier build): in
  Windows Terminal 690 lines queued in 53 runs from line 323 onward,
  all held until a key; in the console host the reading slowed the
  output itself, and the flood was still being spoken after the window
  closed. alpha-57645 left lines out of floods; alpha-58022 speaks every
  new line.
