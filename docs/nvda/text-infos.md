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
  document), readingChunk (say-all's step, which resolves to sentence,
  paragraph, or line by the `speech.sayAllReadingUnit` setting through
  `TextInfo.unit_readingChunk`; UIA always uses line;
  [Speech](speech.md)), controlField,
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
string splitting. `OffsetsTextInfo`'s character unit calls into
`nvdaHelperLocal` (`calculateCharacterOffsets`,
`nvdaHelper/local/textUtils.cpp`), which runs **Uniscribe**
(`ScriptBreak`) over the line and reads `fCharStop` from its logical
attributes, so a "character" is a grapheme cluster: surrogate pairs,
combining marks, and emoji sequences move as one.

The word unit (`OffsetsTextInfo._getWordOffsets`) hands the line to a
`WordSegmenter` (`textUtils/_wordSeg/wordSegmenter.py`), which picks a
strategy (`textUtils/_wordSeg/wordSegStrategy.py`) from the
`documentNavigation.wordSegmentationStandard` feature flag
(`WordNavigationUnitFlag`: Automatic, the default; Chinese; Unicode
(ICU); Legacy (Uniscribe)):

- **Chinese** (`ChineseWordSegmentationStrategy`, the bundled cppjieba
  library): always under the Chinese flag, and under Automatic for
  text containing CJK ideographs and no Japanese kana, when cppjieba
  loaded.
- **ICU** (`IcuWordSegmentationStrategy`, Windows' built-in ICU,
  `textUtils/icu.py` `calculateWordOffsets`): UAX 29 word boundaries
  plus ICU's script-selected dictionary segmentation for scripts such
  as Thai, Lao, Khmer, and CJK. Used for every flag except Legacy, and
  as the fallback when cppjieba is unavailable. To match Uniscribe, a
  word includes its trailing whitespace: the start moves back over
  whitespace-only segments and the end moves forward over them, so an
  offset anywhere in a run of spaces, tabs, or both yields the same
  word (commit `0fd87b9c7`, #20494, which fixed the browse mode caret
  sticking in runs of tabs).
- **Uniscribe** (`UniscribeWordSegmentationStrategy`, `fWordStop` from
  `calculateWordOffsets` in `textUtils.cpp`): the Legacy flag's only
  strategy and the final fallback when ICU is unavailable. Classes
  that must match a Windows control pin it: `EditTextInfo`
  (`NVDAObjects/window/edit.py`) forces `WordSegFlag.UNISCRIBE` to
  match the edit control and Notepad.

A plain whitespace/punctuation fallback (`findStartOfWord` /
`findEndOfWord`) remains for backends that opt out (the deprecated
`useUniscribe = False`) or when the chosen strategy fails. Range-based backends
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
