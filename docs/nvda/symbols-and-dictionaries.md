# Symbols, dictionaries, and character processing

Between "the text to speak" and "what the synthesizer receives" NVDA
runs two rewriting stages — speech dictionaries, then symbol
processing — plus a separate character-description lookup used when
spelling. Together they decide how every punctuation mark, emoji, and
user-corrected word is actually pronounced. Implementation:
`source/characterProcessing.py` and `source/speechDictHandler/`.

## Where the processing sits

`speech.speech.processText(locale, text, symbolLevel)` is applied to
every text item of every speech sequence as it is spoken: first
`speechDictHandler.processText` (dictionaries), then
`characterProcessing.processSpeechSymbols` (symbols) — so
dictionaries see raw text and can create or destroy symbols, and
symbol expansion always has the last word. Braille does *not* run
this pipeline; it renders the unprocessed text through its own
tables ([Braille](braille.md)).

## Speech dictionaries

`speechDictHandler` maintains three user-level layered dictionaries,
each a list of pattern/replacement entries applied in order:
**default**, **voice** (per synthesizer voice), and **temporary** (this
session only, never saved). The default and voice dictionaries are
enabled by the `speech.speechDictionaries` setting, whose default lists
both (`config/configSpec.py`; `speechDictHandler/types.py`, the
`enabled` property); the built-in dictionary below is mandatory and
ignores that setting. Entry types (`speechDictHandler/types.py`
`EntryType`): `ANYWHERE` (plain substring), `WORD` (whole-word match)
and `REGEXP` (Python regex with group references), each optionally
case-sensitive. Files live per user config as `*.dic` under
`speechDicts/`, with voice dictionaries keyed by synth and voice name
(`dictFormatUpgrade.py` migrates old layouts). The GUI editor is
under NVDA's Speech menu; entries apply immediately.

A fourth layer, the **built-in** dictionary, ships with NVDA as
`source/builtin.dic` and is registered in
`speechDictHandler/definitions.py` `_addSpeechDictionaries` as
`DictionaryType.BUILTIN` with `mandatory=True`. A mandatory dictionary
is always enabled (`speechDictHandler/types.py`, the `enabled`
property) and is not user visible (the `userVisible` property), so the
settings panel's dictionary list leaves it out and it cannot be turned
off or edited. It is the same for every language and synthesizer, and
it is registered last, so `speechDictHandler.processText` applies it
after the temporary, voice and default dictionaries. Its three entries
are case-sensitive regular expressions, each replaced by a single
space:

- `(?=[A-Z])(?<=[a-z])`, "Break up words that use a capital letter to
  denote another word": a space before a capital that follows a
  lowercase letter, so "camelCase" becomes "camel Case".
- `(?=[A-Z][a-z])(?<=[A-Z])`, "Break away a word starting with a
  capital from a fully uppercase word": "NVDAObjects" becomes "NVDA
  Objects".
- `(?=\d)(?<=[^\W\d])`, "Break on word-to-digit boundaries": a space
  wherever a digit follows a word character that is not a digit, so
  "issue123" becomes "issue 123", "e2e" becomes "e 2e", and "mv94i5hr"
  becomes "mv 94i 5hr". Only letter-to-digit is split; a letter after
  a digit, and a run of digits, are left alone.

Because the layer runs inside `speech.speech.processText`, it reaches
every text item of every speech sequence: names, messages, say-all,
and caret and review text. Spelling is unaffected in practice, since
it passes one character at a time. The digit rule dates from commit
de7022dd8 (2006-10-03), whose message says it "breaks up words that end
in numbers, at least viavoice does funny things with the pronunciation
if this isn't done". The rules moved into the distributed default
dictionary on 2007-12-15 (2e3005086), became `builtin.dic` on
2008-11-11 (4cb09bf46), and were rewritten for speed on 2026-08-12
(9fc2ccf4e, #20433) with the same output.

Design points worth noting: matching is sequential over *all* entries
of all three dictionaries on every utterance (users with thousands of
entries pay for it), replacements are plain text (a replacement is
re-scanned by *symbol* processing but not by later dictionary
entries), and dictionaries operate on text only — they cannot match
across command boundaries in a speech sequence.

## Symbol processing

`characterProcessing.processSpeechSymbols(locale, text, level)`
expands punctuation and symbols to words according to the user's
*symbol level* — `SymbolLevel`: NONE, SOME, MOST, ALL, CHAR, plus
UNCHANGED for contexts that inherit (`SymbolLevel` enum; the "Punctuation/symbol
level" setting, cyclable with NVDA+P). Per locale, symbol data merges
from layered sources (`_getSpeechSymbolsForLocale`,
`SymbolDictionaryDefinition`):

- `symbols.dic` — the locale's base symbol set, each symbol carrying
  its spoken replacement, the *level* at which it starts being
  spoken, and a preserve rule (whether the literal character is also
  sent to the synth — needed where the symbol affects prosody, like
  sentence-ending periods).
- `cldr.dic` — Unicode CLDR data including emoji names, a separately
  toggleable source ("Unicode Consortium data (including emoji)").
- User symbol dictionaries (the "Punctuation/symbol pronunciation"
  dialog writes `symbols-<locale>.dic` in the user config), layered
  on top; add-ons can register more sources.

`SpeechSymbolProcessor` compiles the merged set into one regex pass;
*complex symbols* (multi-character, context-sensitive patterns like
decimal points between digits, defined in the locale's complex
symbols section) are handled with their own regex rules so "3.14"
does not say "three dot one four" at levels where "." alone would be
silent. Results are cached per locale until config or profile switch
(`handlePostConfigProfileSwitch` clears the cache).

## Character descriptions

`characterProcessing.getCharacterDescription(locale, character)`
reads `characterDescriptions.dic` per locale: the phonetic or
descriptive words used when the user asks for a character description
(review-character command pressed twice — "Alpha", "Bravo"; for CJK
locales, disambiguating descriptions of the character). This is a
separate lookup from symbol processing: symbols answer "how is this
pronounced in flowing text," character descriptions answer "describe
this character so I can identify it."

## Parity-relevant behaviors in one list

- Symbol level applies per utterance, with spelling (CHAR) contexts
  forcing everything spoken.
- The interaction ordering — dictionaries before symbols — is
  user-visible: a dictionary entry rewriting "." changes what symbol
  processing sees.
- Voice dictionaries switch with the voice, temporary dictionaries
  die with the session.
- Symbol data is locale-layered (base, CLDR, user, add-on) with a
  defined precedence, and the user dialog edits only the top layer.
- The preserve rules ("always", "never", "norep" — only when not
  replaced) are part of symbol semantics, not cosmetics; getting them
  wrong audibly changes synthesizer prosody.
