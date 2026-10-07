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
  whitespace included, and a run of spaces and tabs is one segment, so any
  position in it belongs to the same word.
- `WordRules::for_text`: which word rules a text takes, from its language
  tag when the provider gives one, and otherwise from its characters (Han
  without kana is Chinese; Japanese mixes kanji with kana).
- `Segmenter::sentences`: sentences by Unicode's sentence rules.
- `last_pause`: where say-all's speech without pauses splits a piece of
  text (`docs/nvda/speech.md`, "Say-all speaks without pauses"): just past
  its last sentence end, a full stop, exclamation mark, or question mark
  after a character that is neither whitespace nor another such mark, with
  at most one closing quotation mark or parenthesis after it and then
  whitespace or the text's end, together with that whitespace. There is no
  list of abbreviations, so "Dr. " ends a sentence; a decimal point and an
  ellipsis do not.
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
