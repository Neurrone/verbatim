# verbatim-text

Platform-neutral text segmentation for the text model of milestone M4
(`docs/design/phase6-design.md`, "Internationalization in the text model"),
and the memory of a terminal's rows that reading terminals is built on
(`docs/design/terminal-model.md`, section 4). Original
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

## The terminal row memory

`terminal_rows` is the platform-neutral core of the terminal model's
section 4, "Changes as alignments of rows" (`docs/design/terminal-model.md`),
written from that design. It classifies; it does not speak: the reading
rules of section 5 (growing lines, redrawn and volatile rows, keys,
colours) are built on its `Outcome`. It is not wired into the outpost yet.

### Public API

- `Read`: what one read returned, as the read program's snapshot gives it
  (the text from the read's start to the document's end, the screen's
  text, whether the read started where the last said, whether that start
  was refused, whether the read and the screen start at the document's
  start, the document's first row, the caret's row and column and how many
  rows its row and those below hold), with the text as UTF-8 and the time
  of the read in milliseconds. The start for the next read is an opaque
  provider range the outpost keeps.
- `TerminalMemory::new` and `TerminalMemory::read(&Read) -> Outcome`: one
  memory per terminal, kept while the terminal exists. `rows` gives the
  last read's rows (`Row`: its text without padding, a 64-bit hash, whether
  it wraps onto the next row, the hash of its whole line, whether it has
  been redrawn, and when its text last changed); `known_history` the hashes
  of the rows known above the last read; `review` and `set_review` the
  review position, and `marks` and `add_mark` the command marks, each an
  `Anchor` (a row counted from the last read's first row, negative in the
  history above it, a column in characters, and its line's hash).
- `Outcome`: for each row of the read a `NewRow` (`New`; `Kept`, from which
  old row and whether it moved other than with the history; `Rewritten`,
  from which old row and whether it moved; `Returned`, a lingering row taken
  back, and whether elsewhere; `Reprint`; or `Earlier`, history the
  memory never covered); for each old row an `OldRow` (`Kept` or
  `Rewritten` and where, `Gone` and whether it lingers, or `Above`, past
  the read's start); the read's `Line`s (rows joined while each wraps, with
  their text and hash, and `grew_from` when a line that did not move only
  gained at its end, so the text after that offset is what it gained);
  the rotation; whether the first row continues a line from above the
  read; the caret's row; the screen's width and height; a `Switch` of
  screens; whether a whole-document read did not find the old screen
  (`overflowed`); and `whole_document`, set alone when the memory needs
  the whole document read and given before it changes anything.
- `hash`: the 64-bit hash of a row's text (FNV-1a), the same in every
  process.

### How a read is aligned

The read gives text, not coordinates, so a row's place is its index in
the read that found it. A read's rows are its text split at line breaks
and each line split into rows of the screen's width; padding is trimmed
from a line's last row only, so a wrapped line's earlier rows keep the
spaces where it wrapped. The width is the narrowest of the screen's lines
but its last, every line being a whole number of rows except Windows
Terminal's last row of the buffer, which is one cell short. When every
line but the last wraps, that narrowest line is several rows, so the
caret's row, which the read gives as one row, is used instead when its
length, or that length and the cut cell, fits every screen line; when
both fit, the one that gives the height of the last read wins.

Each read starts where the last said, the screen's top as last read, so
a row that did not move is at a known shift: its old index less the rows
the last read held above that start. A read the outpost started anywhere
else (the start was refused, the whole document, a restored memory) has no
known shift, and the old rows are found wherever they are.

The old screen (the last read's rows from that start) is aligned with the
new read's first rows, as many as the old screen and a screen more, and
its last screen of rows; rows between those windows are new. The
alignment is a longest common subsequence of the hashes of the rows with
text, by dynamic programming over every cell of the old screen by the
windows: each cell keeps two running maxima, the best alignment ending on
the same shift (which continues a block) and the best anywhere above and
to the left (which starts one), so among alignments keeping equally many
rows the one with the fewest blocks wins, and then the one with the fewest
rows off the known shift. A blank row is kept only in place, where the
kept row above it, or else below it, puts it. The rotation is how far the
topmost kept row moved up from its known place; rows that moved with it
are kept unmoved, and the others kept as moved.

Between two kept rows, the old and new rows left over are paired as
rewritten by likeness: alike when neither is blank and one begins the
other, or they share in order at least half the characters of the longer,
counted by the bit-parallel longest common subsequence of their
characters. The pairing keeps their order and makes as many pairs as it
can, and then the most whose shift is that of the kept row below them, so
Pi's status row drawn a row lower with its spinner changed pairs with its
old self.

An old row left over is gone, and lingers for 2 seconds when it was a
redrawn row, was at or below the caret's row of the last read, would be at
or below the caret's row now, or the read shows no text at all, where a
program cleared the screen before drawing it again. A new row with a
lingering row's text takes that row back. New rows above the topmost kept
row are matched, in order, against the known history and the old rows
above the kept ones, by the histogram method (the rarest shared row first,
nearest the old screen, then the rows before and after it the same way):
in a normal read the matched rows are a reprint (section 4.4), and in a
whole-document read or a restored memory they are history found again.

When the old screen had text and none of it was kept, or the read's start
was refused, and the read does not start at the document's start, the
memory asks for the whole document and changes nothing until it comes;
the old screen is then searched for in all of it, and when it is not
found in a document longer than a screen and no row of the known history
is found either, the history overflowed.

A read whose screen starts at the document's start, after one whose screen
did not, is a switch of screens or a clear that looks like one (section
5.8): it is aligned as usual, then the memory before it is kept aside with
its anchors, and the new screen's memory starts with the review position
at the caret. When the screen no longer starts at the document's start,
the memory kept aside is restored if every row with text of its old
screen is kept in the read, and dropped otherwise.

Anchors move with the alignment: a kept or rewritten row carries its
anchor to its new place, the column clamped to the text; a row written
over keeps it in place; a gone row with no place in the read passes it to
the nearest surviving row above, or to the caret; above the read it moves
with the shift and the rotation. A command mark moves to the first row of
its line, and is dropped when its row was rewritten or is gone. The known
history takes each old row that went above the read, up to 32,768 hashes.

### Cost

In a release build on Dickson's x64 machine, with 120 by 30 screens of
ASCII text: a read of the last screen and one line more takes about 24
microseconds, most of it splitting the text into rows and lines, the
dynamic programming over 30 by 61 cells a small part; a screen rewritten
whole about 117 microseconds, the likeness pairing of 30 rows by 30; a
read of 1,000 new rows after a screen 1.2 milliseconds; a whole-document
read of 9,030 rows 12 milliseconds, against the 9.4 to 29 milliseconds
the read itself takes.

### Tests

`src/terminal_rows/tests.rs` drives the memory with `verbatim-term-sim`,
reading as the outpost does, one case of the design each, asserted whole:
plain output, a scroll, rotation, a footer kept at its coordinate while
the history rotates, a footer redrawn in place, Pi's status row drawn a row
lower with its spinner changed, Ink's region erased and drawn again in one
write and caught erased between two, a repeated row kept in place rather
than paired with its copy, a wrapped line split across the read's top, Windows Terminal's cut
last row, a clear and a reprint, output repeated after `cls`, the
alternate screen opened and closed, and the review position through a
scroll, a rotation and a redraw elsewhere.

`tests/terminal_oracle.rs` checks a reader built on the memory against
the simulator's oracle, which judges by text, over the conformance corpus
in both terminals, the oracle's 512 random streams and every stream of up
to four operations of its tiny alphabet. The reader speaks a line with a
new row as a line, growth as growth and a line with a rewritten row as
redrawn; it is silent on a line whose rows were all kept, taken back,
reprinted or found in the history, but withholds such a line as having
lost text when the line its first row was in had more, and speaks it as
redrawn when its rows were joined into a line no read held; and it
reports where each row went and its review position. Every case passes
but those the file pins, each with its violations and one of three open
design questions: the alternate screen opened with no history, which no
read shows (Pi's fullscreen mode in both terminals, four tiny streams,
seven random ones); the rotation measured from kept rows when the last
screen was blank, its only kept rows moved down with the end of the text,
or its rows repeat text found elsewhere (seventeen random streams); and
erasing the history taken for the alternate screen opening (one random
stream). Rows moved past others, which a longest common subsequence keeps
in order and so does not map where their text went, are pinned too
(Claude Code's stream and two tiny streams in both terminals, two random
streams, and counted in three random streams pinned for the rotation).

## Units

Every offset is a byte offset into the UTF-8 string given. A provider's
own units (UTF-16 code units, UIA text ranges, MSAA character offsets) are
converted in the outpost, so this crate never sees them.

## Cost

ICU4X's compiled data is built into the binary. The jieba dictionary is
loaded the first time Chinese text is segmented and kept for the life of
the process.
