# Document formatting reporting

How NVDA decides which formatting facts to speak while reading text —
the layer between the FormatFields a TextInfo yields
([TextInfo](text-infos.md)) and the words the user hears. All in
`source/speech/speech.py` with the option set in
`source/config/configSpec.py` (the `[documentFormatting]` section).

## The option vocabulary

The Document Formatting settings panel is a direct projection of the
config section; the major groups (each key `report*` unless noted):

- Font: name, size, attributes (`fontAttributeReporting` — off,
  speech, braille, both), superscripts/subscripts, color, emphasis,
  highlighted (marked) text, style.
- Document information: comments, bookmarks, revisions (tracked
  changes), spelling errors (`reportSpellingErrors2`, a bitmask of
  speech/sound/braille), grammar errors.
- Pages and spacing: page numbers, line numbers, line indentation
  (`reportLineIndentation`: off, speech, tones, both), paragraph
  indentation, line spacing, alignment.
- Elements: headings, links, lists, block quotes, groupings,
  landmarks, articles, frames, figures, clickable state.
- Tables: `reportTables`, row/column headers, cell coordinates, cell
  borders.

Two cross-cutting knobs: `detectFormatAfterCursor` (scan the whole
line for formatting changes rather than just the caret position — the
expensive thorough mode), and `extraDetail` (forced on when reading
by character/word units for review commands, so review reveals more
than flowing reading).

## The diffing model

Formatting is announced *as change*, not per run:
`getTextInfoSpeech` walks the field stream keeping an
`attrsCache` of the formatting last spoken;
`getFormatFieldSpeech(attrs, attrsCache, formatConfig,
initialFormat=…)` emits words only for attributes that are enabled
*and* differ from the cache — "bold" when bolding starts, "no bold"
(negated wording per attribute) when it ends. The `initialFormat`
call at the start of each spoken unit resolves what to announce when
the reading position jumps (the cache carries across successive
utterances of a say-all, but a jump re-baselines). This
cache-and-diff is why reading a fully bold paragraph says "bold"
once, and why any implementation that re-announces per line sounds
broken to NVDA users.

### The cache, attribute by attribute

What follows was read from `getTextInfoSpeech` and
`getFormatFieldSpeech` in `speech/speech.py`, since the summary above
does not settle what a caret movement says.

- Where the cache lives. The formatting last spoken is kept per object
  (on the NVDA object, through `SpeakTextInfoState`), not globally. A
  focus change brings a new object with an empty cache, so the first line
  read in a newly focused edit field reports whatever formatting is
  enabled and present at its start, as a change from nothing.
- What is compared. Each time a unit is spoken (a caret movement, a focus
  line, a review command), the formatting at the unit's start (the
  initial format) is compared with the cache, and each later change of
  formatting inside the unit's text is compared with the cache as it
  stands at that point. After each comparison the cache becomes that
  formatting, whether or not anything was spoken, so the cache ends as
  the formatting of the unit's last stretch of text, trailing white space
  included.
- Where changes are spoken. The initial format's changes come before the
  unit's text; a change inside the text is spoken at the point in the
  text where it happens, so a line with a misspelt word in the middle
  says "hello spelling error wrold there". A unit of one character (a
  caret movement by character, or a word that is one character) speaks
  only its initial format and then the character.
- Which unit's text. White space is part of the text the changes are
  placed in, so a word read with the space after it ends with whatever
  the space's formatting changes, even though white space itself says
  nothing. When everything in the unit is white space, only the initial
  format's changes are spoken, followed by "blank".
- Spelling and grammar errors. Entering an error says "spelling error"
  or "grammar error" (as words, as the error sound, or both, by the
  setting). Leaving one says "out of spelling error" or "out of grammar
  error" only for character and word units (the extra detail mode: caret
  movement by character or word, and the review cursor's character and
  word commands); a line or a say-all chunk leaving an error says
  nothing. Caret movement by paragraph does not report spelling errors,
  for speed.
- Font name, size, and color. Spoken, as the value alone ("Calibri",
  "11.0 pt", "dark red"), when present and different from the cache; an
  attribute that becomes absent says nothing.
- Bold, italic, and underline (the font attributes setting). "bold" when
  it starts, "no bold" when it ends after having been reported as on;
  likewise "italic" and "no italic", "underlined" and "not underlined".
  A value that was never known (absent in the cache) and is off says
  nothing.
- The order within one change follows the setting groups: font name,
  font size, color, then bold, italic, underline, then spelling error
  and grammar error.
- UIA providers. NVDA walks a range by UIA's format unit and reads each
  stretch's attributes. A spelling error is the spelling error
  annotation type in the range's `AnnotationTypes` attribute (grammar
  error likewise); bold is a font weight of 700 or more; underline is
  any underline style but none; the color is the foreground color,
  named by its nearest hue, saturation, and brightness ("dark red",
  "light pale blue", "grey"); the size is in points, "11.0 pt".

Some attributes are not spoken as words at every level:
line indentation can be *tones* (pitch encodes depth), spelling
errors can be a sound rather than the word "spelling error"
(`reportSpellingErrors2` bit flags), and in braille many of the same
facts render as format indicators through the braille table instead
([Braille](braille.md)).

## Control fields versus format fields

Element-level announcements ("link", "heading level 2", "list with 5
items", "out of table") come from *ControlFields*, spoken by
`getControlFieldSpeech` with their own enable flags (headings, links,
tables…) and role-dependent enter/exit wording; character-level facts
(font, color) come from *FormatFields* via the diffing above. The two
interleave in one field stream, and reasons matter: `OutputReason`
(focus, caret movement, say-all, quick nav) selects wording variants
and which categories speak at all — the table NVDA users internalize
as "browse mode says 'link' before, focus mode says it after."

## Where the fields come from

The reporting layer is backend-agnostic; fidelity depends on what
each TextInfo yields: IA2 text attributes
([IA2 usage](ia2.md)), UIA TextPattern attribute ranges
([The UIA client](uia.md)), Word object model properties
([Office through COM](office-com.md)), buffer-rendered attributes for
browse mode ([Virtual buffers](virtual-buffers.md)), and the display
model's draw-time facts ([The display model](display-model.md)).
Attribute names inside FormatFields are a de-facto NVDA-internal
vocabulary ("font-name", "bold", "text-position", "invalid-spelling",
…) that every backend normalizes into — undocumented upstream, so the
authoritative list is what `getFormatFieldSpeech` consumes.
