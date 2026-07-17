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

`speechDictHandler` maintains three layered dictionaries, each a list
of pattern/replacement entries applied in order: **default** (always
active), **voice** (per synthesizer voice), and **temporary** (this
session only, never saved). Entry types (`speechDictHandler/types.py`
`EntryType`): `ANYWHERE` (plain substring), `WORD` (whole-word match)
and `REGEXP` (Python regex with group references), each optionally
case-sensitive. Files live per user config as `*.dic` under
`speechDicts/`, with voice dictionaries keyed by synth and voice name
(`dictFormatUpgrade.py` migrates old layouts). The GUI editor is
under NVDA's Speech menu; entries apply immediately.

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
