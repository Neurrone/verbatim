# verbatim-text

Platform-neutral text segmentation for the text model of milestone M4
(`docs/design/phase6-design.md`, "Internationalization in the text model"),
and the reading of terminals: the memory of a terminal's rows and the
rules that decide what is spoken (`docs/design/terminal-model.md`,
sections 4 and 5). Original
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
rules of section 5 (`terminal_reading`, below) are built on its
`Outcome`. Neither is wired into the outpost yet.

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
are kept unmoved, and the others kept as moved. A longest common
subsequence keeps rows in order, so a row moved past others, as a list
entry drawn again elsewhere in the list, is kept afterwards where its
text went when that text is on no other row of the old rows or of the
read, so every such row is mapped where it went.

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
than paired with its copy, a row moved past others kept where its text
went, a wrapped line split across the read's top, Windows Terminal's cut
last row, a clear and a reprint, output repeated after `cls`, the
alternate screen opened and closed, and the review position through a
scroll, a rotation and a redraw elsewhere.

The memory is also checked by the oracle, through the reading rules built
on it (below).

## The terminal reading rules

`terminal_reading` is the platform-neutral core of the terminal model's
section 5, "The reading rules", written from that design on top of the row
memory. It is a state machine with no clock and no thread of its own:
every input carries its time, and every input answers with what to say,
what it withholds and why, and when the quiet-period timer should next
fire. Reading on demand and moving by command (sections 5.11 and 5.13)
are not built here; they act on the memory, which the reader lends out.

### Public API

- `TerminalReader::new(Settings)`: one reader per terminal, owning its
  `TerminalMemory` (`memory`, and `memory_mut` for the review commands).
  `Settings` holds "report new output", "review follows caret", and the
  flood policy's "lines spoken in full" and "last lines to speak".
- The inputs, each returning a `Step`:
  - `read(&Read, waited, colours)`: a read of the terminal, with whether
    it waited for Core's speech queue and, for the first read after a key,
    the screen's colour runs (`ColourRun`), row by row from the screen's
    top to the document's end.
  - `key(Key)`: a key as it reached the program, with when. `KeyKind` is a
    typed character (`Typed`), a caret key Verbatim knows with its `Unit`
    (character, word or line), a key that forgets held typing (`Forget`:
    Enter, Tab, Escape, Control+C, Control+D, Control+Break), or any other
    key (`Other`).
  - `focus(at_ms)` and `blur()`: the focus arriving and leaving.
  - `timer(now_ms)`: the timer the last step asked for came due.
  - `set_settings(Settings)`.
- `Step`: the `Speech` in order, the lines `Withheld` with a `Reason`, the
  `timer` due time, `whole_document` when the read is to be taken again
  over the whole document, and the read's `Outcome`. `Speech` is a new
  line (`Line`), what a growing line gained (`Growth`), a redrawn row from
  the word that changed (`Changed`), a flood's count (`Skipped`, with the
  first rows of its lines, how many, and whether the history overflowed),
  typing shown (`Echo`), a key's answer on the caret's line in a unit
  (`Caret`), text the caret's line gained or lost (`Gained`, `Lost`), a row
  away from the caret that is a key's answer (`Answer`), and a suggestion
  after the caret (`Suggestion`). Each names the first row of its line in
  the memory's last read. A `Reason` is one of the design's silences:
  `Volatile`, `UnfinishedWord`, `HeldForKeys`, `Spinner`, `LostText`,
  `HeldTyping` and `KeyAnswer`.
- The time limits of section 5.14 as constants: `KEY_WINDOW_MS` and
  `TYPED_LIMIT_MS` (300), `VOLATILE_MS` and `UNPROMPTED_MS` (2,000),
  `LINGERING_MS` (2,000, the memory's), `QUIET_MS` (1,000), and
  `quiet_period`, 1 second or 1.5 times the interval between the last two
  changes when that is longer.

### How the rules are built

Beside the memory's rows the reader keeps, for each row, its colours when
last read and, on a line's first row, the line's history: its text as last
read, what the user was last told of it, whether it was redrawn and is
volatile, when it last changed and the interval before, when it last
changed outside a key's window, when it last grew, what it waits for, and
whether a key changed it since it appeared. That history moves with the
alignment as the memory's does: from the old line a kept or rewritten
first row started, or that the read's first row continues; a lingering
line taken back brings its history with it; a row split off a line starts
a line of its own, with the old line's text kept only to tell whether it
lost text. A line the read's first row continues keeps the part above the
read in its text, and speech quotes only the part the read shows. The
history kept aside when the alternate screen opens is restored with the
memory.

Each read, line by line, top to bottom:

- The first read, the first after the focus arrives, a read while the
  focus is away, and a read of a new size speak nothing; each line's text
  becomes what the user knows of it. A new size speaks the caret's line
  when it changed, and finds the review position again by its line's hash
  nearest its old row.
- A line with no history whose rows with text are new is new output,
  spoken whole, unless it is the input the shell reprinted: the caret's
  new line, equal to the caret's line at the last read with any typing
  since, that line holding input and still above with new rows between.
  New lines go through the flood policy: with more than "lines spoken in
  full" and "last lines to speak" together, the first and last are spoken
  and the lines between counted in one `Skipped`, blank lines counted but
  not those after the last line with text; a whole-document read that did
  not find the last screen counts them as more than.
- A line that grew, and whose last read was all heard or held back only
  by its unfinished word, speaks what it gained since what was said. When
  the caret is at its end after a character that is not a space, the last
  word is held (`UnfinishedWord`) and spoken when the line grows past it,
  when the caret leaves the line, or after the quiet period. Spaces alone
  wait with the word after them.
- A line rewritten in place is a redrawn row. Within a key's window, away
  from the caret, and with no change of its own in the 2 seconds before
  the key, it is the key's: one that gained text other than at its end
  (compared without spaces, the old text in order within the new) is the
  answer, spoken whole; one that only lost text is silent; any other is
  held until the keys pause (`HeldForKeys`). Otherwise a row that only
  lost text is silent (`LostText`), a spinner's symbol replaced alone is
  silent (`Spinner`), and a row that changed within 2 seconds of its last
  spoken change, or at two reads in a row when the second waited for
  speech, is volatile: withheld (`Volatile`) and spoken once it has not
  changed for the quiet period. Any other change is spoken by the
  changed-word rule: from the start of the word, by Unicode's word rules,
  where the line first differs, by whole graphemes, from what was last
  said or from its last read, whichever is earlier, to its end.
- Kept, returned, reprinted and earlier lines are silent; such a line that
  now holds less of its old line's text is withheld as `LostText`, and one
  whose rows were joined into a line no read held is spoken as a change.
- A held line that has scrolled above the screen, where no program redraws
  it and the next read does not reach, is spoken at once, as when its time
  has come.

Keys open a window of 300 milliseconds after the last of them reached the
program; a key arriving while a window is open joins it. The caret's line
before the window's first key, without any suggestion, is what its first
read compares with, and each read in the window leaves the line as it
found it for the next, so an erase over several reads is spoken as it
goes. At a read in the window, the caret's line
is the keys' answer and is not output: typing shown at the caret, inserted
or over text that showed it already, is echoed, and typing not shown
within 300 milliseconds of reaching the program is dropped unspoken; a
caret moved with no text change says a caret key's unit, or for any other
key the landing rule (one character, that character; the line's start or
end, the character there; a word's start or end, the word); a line key
says the line; a line that gained text says the gain, one that lost text
next to the caret says the loss, and one changed otherwise drops held
typing and speaks from the changed word. A new line at the caret, as after
Enter, is output. With "review follows caret" on, a key that moved the
caret moves the review position to it; the focus arriving does so too.

Colours are compared only in the first read after a key: among rows whose
text did not change, one whose colours changed is the answer; of two, the
one whose colours now differ from both its neighbours'; with no colours
read before, the one row that differs from both its neighbours. Text
after the caret in other colours than the cell before the caret is a
suggestion, spoken when it appears and not again while typing follows it.

"Report new output" off speaks no output and holds nothing, and the
memory is taken from a read that asks for the whole document rather than
reading it; keys are still answered.

### Cost

In a release build on Dickson's x64 machine, with 120 by 30 screens of
ASCII text, the rules add to each read on top of the alignment: about 7
microseconds to a read of the last screen and one line more (the
alignment about 40); about 210 microseconds when every row of the screen
is rewritten, the changed-word rule's word segmentation about 7
microseconds a row; and about 170 microseconds to a read of 1,000 new
rows. Nothing is done between inputs, and the only wake-up the rules ask
for is the quiet-period timer, while something waits for it.

### Tests

`src/terminal_reading/tests.rs` drives the rules with `verbatim-term-sim`,
reading as the outpost does at given times, with keys as they reach the
program, one rule of section 5 or one case of sections 7 and 8 each,
asserting the whole step: new output line by line, a line seen first
spoken whole, growth with its unfinished word held and spoken when the
line grows past it or after the quiet period, kept rows silent, a flood's
first and last lines with the rest counted, and counted as more than past
the history, the input reprinted below a completion list, padding; a
redrawn row's changed word, a spinner, a volatile row heard at its first
change and once settled, a row volatile at two reads after waiting, a row
that only lost text, a held row scrolled above the screen; typing echoed,
shown late, never shown and changed otherwise, the landing rule at a word,
a character and the line's start, a known caret key, text gained and lost
at the caret, Up Arrow and a character at once, a change after the
window; the review cursor following a key, not following with the setting
off, and moved by the focus; a marker list, a meter that is not the key's
answer, a ruler held until the keys pause, a filtered list with its echo,
current entry and count, a highlight moved by colour, a suggestion; the
alternate screen; a resize; output in the background; "report new output"
off; Pi streaming above its status row, Ink's region drawn again with a
changed cost, and several progress lines settling.

`tests/terminal_oracle.rs` checks the rules against the simulator's
oracle, which judges by text, over the conformance corpus in both
terminals, the oracle's 512 random streams and every stream of up to four
operations of its tiny alphabet, reading at the streams' times. Every
case passes but those the file pins, each with its violations and one of
four open design questions: the alternate screen opened with no history,
which no read shows (Pi's fullscreen mode in both terminals, four tiny
streams, six random ones); the rotation measured from kept rows when
the last screen was blank, its only kept rows moved down with the end of
the text, or its rows repeat text found elsewhere (the corpus's footer
over a rotating history in both terminals, sixteen random streams);
erasing the history taken for the alternate screen opening (one random
stream); and a line holding its unfinished word pushed out of a full
history between two reads, so the word is never spoken (146 tiny streams,
whose history is one row, and six random streams with floods).

## Units

Every offset is a byte offset into the UTF-8 string given. A provider's
own units (UTF-16 code units, UIA text ranges, MSAA character offsets) are
converted in the outpost, so this crate never sees them.

## Cost

ICU4X's compiled data is built into the binary. The jieba dictionary is
loaded the first time Chinese text is segmented and kept for the life of
the process.
