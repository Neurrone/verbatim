# Review modes and the review cursor

The review cursor is NVDA's text-level exploration position — a
TextInfo ([TextInfo](text-infos.md)) the user moves by line/word/character
without touching the app's caret. *Review modes* choose which text
surface that TextInfo ranges over. Implementation:
`source/review.py`, state in `source/api.py`, commands in
`source/globalCommands.py`.

## The three modes

`review.modes` defines them, in order (NVDA+PageUp/PageDown cycles;
`nextMode`):

1. **Object review** (`getObjectPosition`): the review TextInfo is
   the *navigator object's own text* — `makeTextInfo(POSITION_CARET)`,
   falling back to `POSITION_FIRST`, falling back to the generic
   `NVDAObjectTextInfo` (whose text is roughly the object's name and
   value). Scope: one object.
2. **Document review** (`getDocumentPosition`): available when the
   navigator object is inside a `DocumentTreeInterceptor` (browse
   mode document; [Browse mode](browse-mode.md)); the TextInfo is minted from the
   *whole document* positioned at the navigator object
   (`treeInterceptor.makeTextInfo(obj)`). Scope: the flattened
   document.
3. **Screen review** (`getScreenPosition`): the TextInfo is a
   `DisplayModelTextInfo` over the focus's *top-level window*
   (`GA_ROOT` ancestor), positioned at the navigator object's
   coordinates — the text as physically drawn on screen, from the
   display model ([The display model](display-model.md)). Scope: the visible window,
   laid out spatially.

Mode fallback is automatic and downward: `getPositionForCurrentMode`
tries the current mode's factory and falls back mode-by-mode toward
object review when the current mode is impossible for the navigator
object (no tree interceptor, no display model text). Switching modes
re-anchors the review position at the navigator object
(`setCurrentMode` calls `api.setReviewPosition`).

## Coupling rules

The full cursor-following contract (accessors in `api.py`):

- Setting the *navigator object* resets the review position into the
  (mode-appropriate) text at that object.
- Moving the *review cursor by text units* (the numpad 7/8/9,
  4/5/6, 1/2/3 family in `globalCommands.py`) moves only the review
  position — except that in document review within browse mode the
  review position and the browse caret are the same surface, so
  moving one is felt in the other.
- With `reviewCursor.followFocus` on, focus changes re-seed navigator
  and review; with `reviewCursor.followCaret` on, *caret* moves
  re-seed the review position to the caret line
  (`review.handleCaretMove`, called from caret events); mouse
  following (`followMouse`) likewise.
- The reverse coupling is explicit only: routing commands push review
  to caret / focus (`review-modes` scripts and
  `script_navigatorObject_moveFocus`; [Focus and the navigator](focus-and-navigator.md)).

## Reading commands built on review

Line/word/character (and their repeat behaviors: twice spells, three
times character-description or copies), top/bottom of the surface,
"review current line" variants, plus *say-all from review*, and the
"speak review to clipboard" copy commands — all in
`globalCommands.py`, all pure TextInfo manipulation (expand to unit,
move, speak with `speech.speakTextInfo`), which is what makes them
uniform across the three surfaces. Braille tethering to review
([Braille](braille.md)) reuses the same position.

What the reading commands say, precisely:

- A motion that cannot move says so and then reads the current unit:
  "Top" for previous line or word at the start, "Bottom" for next line or
  word at the end, "Left" and "Right" for previous and next character at
  the ends of the line. Character motions do not leave the line.
- A unit with nothing to read is spoken as "blank".
- The current line or word pressed twice is spelled; three times,
  spelled with character descriptions ("alpha" for a). The current
  character pressed twice gives its description; three times, its code
  in decimal, then in hexadecimal, spelled ("97," and "0 x 6 1").
- Spelling names a space as "space", and marks capitals by raising the
  pitch (by default).
- Every review and navigator command with no navigator object says "No
  navigator object".
- Activate performs the review position's or the navigator object's
  action, walking up its parents until one has an action, and says the
  action's name ("Activate" by default); with nothing to activate it says
  "No action".
- A copy confirms "Copied to clipboard:" with the text, or the number of
  characters from 1024 on, after reading the clipboard back; a failure is
  "Unable to copy".
