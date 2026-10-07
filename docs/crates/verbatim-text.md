# verbatim-text

Platform-neutral text segmentation for the text model of milestone M4
(`phase6-design.md`, "Internationalization in the text model"). Original
code over ICU4X's segmenters, jieba, and `unicode-width`; it never touches a
Windows API, and `cargo xtask ci` checks that it does not.

## What it provides

- `graphemes` and `grapheme_at`: characters as grapheme clusters, so an
  emoji sequence, a letter with combining accents, a Hangul syllable, or an
  Indic conjunct is one character, and a surrogate pair is never split.
- `Segmenter::words` and `Segmenter::word_at`: words by Unicode's word rules
  with ICU's dictionaries for scripts written without spaces (Japanese,
  Thai, Lao, Khmer, Burmese), or by jieba's dictionary for Chinese, as NVDA
  uses cppjieba. The segments cover the whole text, punctuation and
  whitespace included, and a run of white space is one segment, so any
  position in it belongs to the same word. White space (`is_space`) is
  Unicode's, together with the zero-width space and the zero-width
  no-break space, which Khmer, Thai, and Burmese text uses to mark word
  breaks.
- `WordRules::for_text`: which word rules a text takes, from its language
  tag when the provider gives one, and otherwise from its characters (Han
  without kana is Chinese; Japanese mixes kanji with kana).
- `Segmenter::sentences`: sentences by Unicode's sentence rules.
- `last_pause`: where say-all's speech without pauses splits a piece of
  text (`docs/nvda/speech.md`, "Say-all speaks without pauses"): just past
  its last sentence end, a sentence-ending mark after a character that is
  neither whitespace nor another such mark, with at most one closing
  character after it and then whitespace or the text's end, together with
  that whitespace. The marks are the full stop, exclamation mark, and
  question mark, their Chinese and Japanese full-width forms (after which
  no space is needed), the Devanagari danda and double danda, the Arabic
  question mark, the Urdu and Armenian full stops, and the Ethiopic full
  stop and question mark; the closing characters are quotation marks,
  guillemets either way round, German quotation marks, corner brackets,
  and parentheses, full-width or not. There is no list of abbreviations,
  so "Dr. " ends a sentence; a decimal point and an ellipsis do not.
- `is_line_break`, `lines`, and `line_at`: lines of text Core holds whole,
  such as an object's value reviewed as flat text. Any line break ends a
  line: a carriage return and line feed together are one break, and a
  carriage return alone (Windows 11 Notepad's), a line feed alone, the
  vertical tab, the form feed, the next-line control, and Unicode's line
  and paragraph separators are each one. An offset in a break belongs to
  the line the break ends.
- `cell_width`: how many terminal cells text takes, two for wide East Asian
  characters and none for combining marks.
- `trim_padding`: a terminal line without its trailing padding, whatever
  whitespace characters it is made of.
- `composed` and `is_capital`: a character in Unicode's composed normal
  form (NFC, by ICU4X's `icu_normalizer`), and whether it is a capital,
  some code point uppercase and none lowercase once composed; the
  character table and the capital pitch use them, so a letter written
  with combining accents is named and raised as its precomposed form is.
- `is_word_grapheme`: whether a grapheme cluster continues a typed word:
  every code point a letter, mark, or number by its general category
  (ICU4X's `icu_properties`), or the zero-width non-joiner or joiner.
- `is_bidi_control` and `strip_bidi_controls`: the bidirectional
  formatting characters (the left-to-right and right-to-left marks, the
  embeddings and overrides, the isolates, and the pops that end them),
  which the outposts remove from every name, value, and description they
  read.

## Units

Every offset is a byte offset into the UTF-8 string given. A provider's
own units (UTF-16 code units, UIA text ranges, MSAA character offsets) are
converted in the outpost, so this crate never sees them.

## Cost

ICU4X's compiled data is built into the binary. The jieba dictionary is
loaded the first time Chinese text is segmented and kept for the life of
the process.
