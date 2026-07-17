# TextInfo: the text abstraction

Everything text-shaped in NVDA — the caret, review, browse mode,
say-all, spelling, formatting reports, braille display of text — is
programmed against one abstraction: `textInfos.TextInfo`
(`source/textInfos/__init__.py`). Understanding its contract is a
prerequisite for reading half of NVDA.

## The contract

A TextInfo is a *range* over some object's text (`obj.makeTextInfo`
mints one), with:

- Construction positions: `POSITION_FIRST`, `LAST`, `CARET`,
  `SELECTION`, `ALL`, or a `Bookmark` (an opaque re-anchorable saved
  position; bookmarks survive while the underlying range mechanism
  allows).
- Range surgery: `collapse(end=False)`, `expand(unit)`,
  `move(unit, direction, endPoint=None)`,
  `setEndPoint(other, which)`, `compareEndPoints(other, which)`,
  `copy()`. Units (`UNIT_*`): character, word, line, sentence,
  paragraph, page, table/row/column/cell, screen, story (the whole
  document), readingChunk (say-all's step), controlField,
  formatField. Not every implementation supports every unit; callers
  degrade (the unit constants double as the vocabulary of the "read
  by X" commands).
- Content: `.text` (plain), and `getTextWithFields(formatConfig)` —
  the important one: a sequence interleaving text strings with
  `FieldCommand` markers opening/closing `ControlField`s (element
  boundaries: role, states, attributes — what "link", "heading level
  2" announcements come from) and `FormatField`s (formatting runs:
  font, color, style — what document formatting reporting reads).
  Speech and braille generation consume this field stream, not bare
  text ([Speech](speech.md)).
- Geometry: `pointAtStart`, `boundingRects`, and construction from
  points (mouse tracking, touch).
- Actions: `updateCaret()`, `updateSelection()` — push the range back
  into the app.

## The two implementation families

**Offsets-based** (`textInfos/offsets.py`, `OffsetsTextInfo`): for
backends whose text is addressable by integer offsets. An implementor
provides primitives — `_getStoryLength`, `_getTextRange(start, end)`,
`_getCaretOffset`/`_setCaretOffset`, `_getSelectionOffsets`,
`_getLineOffsets`/`_getWordOffsets`/`_getCharacterOffsets` (unit
boundaries around an offset), point conversions — and the base class
supplies the whole TextInfo contract as offset arithmetic. Notable
subtlety handled centrally: *encoding-aware offsets*
(`OffsetsTextInfo.encoding`, `source/textUtils.py`) — Win32 controls
speak UTF-16 code units, and NVDA converts between those and Python
characters so surrogate pairs and emoji do not corrupt unit movement
(`UtF16OffsetConverter`). Implementors include edit controls, IA2 text
([IA2 usage](ia2.md)), virtual buffers ([Virtual buffers](virtual-buffers.md)), the display model
([The display model](display-model.md)), and the generic fallback
(`NVDAObjectTextInfo` — name/value as text).

**Range-based**: for backends with native range objects — UIA text
ranges (`UIATextInfo`, `source/UIAHandler/`), Word object model ranges
([Office through COM](office-com.md)), Scintilla/rich edit COM ranges. These map TextInfo
verbs onto the native range API (UIA `Move`/`ExpandToEnclosingUnit`,
Word `Range.Move*`), inheriting the native implementation's unit
semantics — and its bugs; per-backend workarounds live in the
respective TextInfo subclasses.

## Word and character segmentation

For offsets-based backends, unit boundaries are not left to naive
string splitting: `OffsetsTextInfo`'s character and word units call
into `nvdaHelperLocal` (`calculateCharacterOffsets` /
`calculateWordOffsets`, `nvdaHelper/local/textUtils.cpp`), which runs
**Uniscribe** (`ScriptBreak`) over the line and reads its logical
attributes — `fCharStop` for character boundaries (so a "character"
is a grapheme cluster: surrogate pairs, combining marks, and emoji
sequences move as one) and `fWordStop` for word boundaries (giving
linguistically informed segmentation, including for scripts without
spaces). A plain whitespace/punctuation fallback exists for backends
that opt out or when the helper is unavailable. Range-based backends
(UIA, Word) instead inherit the native API's own unit semantics —
one reason "word" does not segment identically across controls.

## Paragraph styles

Paragraph navigation (Ctrl+Up/Down and friends) has a user-facing
*paragraph style* setting (`documentNavigation.paragraphStyle`, a
feature flag; `source/config/featureFlagEnums.py`
`ParagraphNavigationFlag`) with three values: **handled by
application** (delegate to the backend's `UNIT_PARAGRAPH` — the
default), **single line break** (each line is a paragraph), and
**multi line break** (paragraphs separated by blank lines — the
plain-text-file convention). The latter two are implemented
generically in `source/documentNavigation/paragraphHelper.py` by
scanning line by line from the caret (capped at `MAX_LINES` = 250
before giving up), and are disabled on backends where per-line
scanning is too slow (non-UIA Word, the display model — the
`_isAcceptableTextInfo` check). A command cycles the style at
runtime (`nextParagraphStyle`).

## Why this shape matters

The design premise: *write reading features once, against ranges and
unit movement, and let every backend supply its own primitives*. What
it costs: unit semantics are only as consistent as backends make them
("line" in a terminal vs. Word vs. a web page differ subtly; word
segmentation differs per control), and every TextInfo operation may be
one or many cross-process calls, so feature code must treat movement
as expensive. Both properties — the leverage and the inconsistency —
are inherited by any design that adopts a TextInfo-like layer.
