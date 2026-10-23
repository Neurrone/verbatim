# verbatim-text

Platform-neutral text segmentation for the text model of milestone M4
(`docs/design/phase6-design.md`, "Internationalization in the text model"). Original
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
- `Segmenter::words_in`: the same with the text's language, whose
  tailoring of Unicode's rules ICU applies where it has one: in Finnish or
  Swedish a colon joins a word to its ending ("EU:n" is one word).
- `WordRules::for_text`: which word rules a text takes, from its language
  tag when the provider gives one, and otherwise from its characters by
  their Unicode script (Han without kana is Chinese, Han outside the Basic
  Multilingual Plane included; Japanese mixes kanji with kana). Japanese
  written only in kanji cannot be told from Chinese without its language,
  and is segmented as Chinese then, as NVDA's automatic choice does.
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

## The terminal line diff

`terminal_diff` is the diff of the terminal reading design
(`docs/design/terminal-reading.md`, "The diff"; its review's sections 5
and 6), written from that design and `docs/parity/terminals.md`, "New
terminal output". It is not wired into the outpost yet. It depends on
`verbatim-model` for `LineChange` and `Skipped`, the shapes the outpost's
`TerminalOutput` carries.

Both entry points take a `Read`: the old screen's lines as last read, the
same lines as last said, the snapshot's lines (padding trimmed, logical
lines, so a wrapped line is one), the screen's height, and the caret's
line in the snapshot. Each returns a `Found` and a `Work`.

- `diff_read` is the normal read, whose snapshot starts at the old
  screen's top. The common start is taken first: the lines above the old
  last line unchanged, then the old last line as it was or grown, which
  is the usual case and linear. Otherwise the common end is taken, and the
  rest is aligned by the longest common subsequence of lines, ties going
  to the alignment that keeps lines nearest their own position. Only the
  snapshot's first lines, the old screen's length plus one screen and one
  line more, are aligned; the extra line keeps a footer the console host
  redraws a row lower inside the window. Old and new lines left between
  the same two kept lines are paired as rewritten in place: one old line
  with the new line sharing the longest start with it, or, replaced by a
  single line, when either is blank; several first to first when at
  least as many new lines replace them; none when fewer do.
- What it says, in speaking order: `above`, lines inserted before the
  last old line kept or rewritten, and what changed of lines rewritten
  there; `changed`, the caret's line rewritten in place, or else the old
  last line; and `appended`, every line after the last old line kept,
  uncut, for the flood policy. A line rewritten speaks through
  `line_change`: what it gained at its end, or the line from the start of
  the word where it first differs, nothing for a spinner or a line that
  only lost text or gained white space. A line's change is taken from
  what it said, with its change since it was read beside it where they
  differ, and `said` lists the lines that only got shorter than what they
  said, which the memory keeps.
- `written` gives the lines that count: a blank line only above the
  caret's line, or, with no caret, every line but the blank ones at the
  end.
- When no line of the old screen that is not blank is found unchanged,
  and its last line is not found grown in place, `old_missing` is set and
  nothing else: the outpost then takes one whole-document read, never
  repeated.
- `diff_document` is that read. It scans the document from its end
  backward for a line that is the old screen's last line that is not
  blank, or that line grown, with every old line above it standing above
  it unchanged; the nearest the end wins. Found, the document is diffed
  from there as `diff_read` does. Not found in a document longer than a
  screen, the history overflowed: the screen's lines are appended, and the
  rest of the document is `Skipped::MoreThan` that many lines, or
  `Skipped::Uncounted` when it is one line repeated, since identical lines
  with a full history cannot be counted. Not found in a document no longer
  than a screen, the screen was cleared or another buffer shown, and the
  document is diffed from its top.
- `Work` counts the window (`aligned_new`), the old lines aligned
  (`aligned_old`), and every comparison of two whole lines (`compared`),
  which the tests assert exactly: a 2,000-line flood after a 30-line
  screen aligns 61 lines against 30 with 30 comparisons, and a 9,001-line
  document without the old screen takes 8,973.

## Units

Every offset is a byte offset into the UTF-8 string given. A provider's
own units (UTF-16 code units, UIA text ranges, MSAA character offsets) are
converted in the outpost, so this crate never sees them.

## Cost

ICU4X's compiled data is built into the binary. The jieba dictionary is
loaded the first time Chinese text is segmented and kept for the life of
the process.
