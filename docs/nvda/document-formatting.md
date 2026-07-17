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
