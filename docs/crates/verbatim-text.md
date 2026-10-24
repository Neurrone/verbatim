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
`Outcome`. The outpost reads every focused terminal through both
(`docs/crates/verbatim-outpost.md`, "Terminals").

### Public API

- `Read`: what one read returned, as the read program's snapshot gives it
  (the text from the read's start to the document's end, the screen's
  text, whether the read started where the last said, whether that start
  was refused, whether the read and the screen start at the document's
  start, the document's first row, the caret's row and column and how many
  rows its row and those below hold, and how many rows it holds above the
  start it was asked for), with the text as UTF-8 and the time of the read
  in milliseconds. The start for the next read is an opaque provider range
  the outpost keeps.
- `TerminalMemory::new` and `TerminalMemory::read(&Read) -> Outcome`: one
  memory per terminal, kept while the terminal exists. `rows` gives the
  last read's rows as `Rows`, counted from the read's first row: `len` is
  how many the read held, `first_kept` the first the memory keeps, `kept`
  those rows and `get` one of them (`Row`: its text without padding,
  shared as an `Arc<str>`, a 64-bit hash, whether it wraps onto the next
  row, the hash of its whole line, whether it has been redrawn, and when
  its text last changed). `trim` keeps only the block and the rows below
  it (below), and `first_kept` says before it from which row it will keep.
  `known_history` gives the hashes of the rows known above the rows kept;
  `review` and `set_review` the review position, and `marks` and
  `add_mark` the command marks, each an `Anchor` (a row counted from the
  last read's first row, negative in the history above it, a column in
  characters, and its line's hash), set only on a row the memory keeps;
  `set_review_anchor` puts the review position at any anchor, a history
  row above the rows kept among them, whose line's hash the caller read,
  and `anchor` makes one on a row the memory keeps. `add_place`, `place`
  and `drop_place` keep places, numbered anchors the caller holds in the
  text (the outpost's review answers' rows), which move with their rows as
  the review position does, are kept aside with the memory while the
  alternate screen is up, and are dropped when their row cannot be
  followed: when the history overflowed and their row was not kept, or,
  for a read the memory takes as a first, when their line is not where it
  should be.
  `above` gives how many rows the next read takes above its start, which
  the read program is asked for. `restart(&Read)` takes a read as a first
  read, keeping the command marks it cannot refute and the review position
  on its row when the read shows its line there, for a caller that will
  not take a read the memory asked for.
- `Outcome`: for each row of the read a `NewRow` (`New`; `Kept`, from which
  old row and whether it moved other than with the history; `Rewritten`,
  from which old row and whether it moved; `Returned`, a lingering row taken
  back, and whether elsewhere; `Reprint`; or `Earlier`, history the
  memory never covered); for each old row an `OldRow` (`Kept` or
  `Rewritten` and where, `Gone` and whether it lingers, `Above`, past
  the read's start, or `Left`, out of the buffer past the document's
  start); the read's `Line`s (rows joined while each wraps, with
  their text, a one-row line's shared with its row, and hash, and `grew_from` when a line that did not move only
  gained at its end, so the text after that offset is what it gained);
  the rotation; whether the first row continues a line from above the
  read; the caret's row; the screen's width and height; a `Switch` of
  screens; whether more went by than the history holds (`overflowed`);
  and, each set alone when the memory needs another read and given before
  it changes anything, `whole_document` and `search_above`, a read from
  the same start with that many rows above it.
- `hash`: the 64-bit hash of a row's text (FNV-1a), the same in every
  process.
- `rows_of`: a read's text split into rows as the memory splits a read,
  for a caller that reads rows the memory does not keep, the history above
  them, at the width a read found.

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
known shift, and the old rows are found wherever they are. Rows above
where the old rows began, in a read with a known shift, are history the
memory never read (`Earlier`), since no program writes into the history.

Once the history is full the buffer rotates under fixed coordinates, so
the rows at the kept start move up by a number of rows no provider reports
(section 4.3, item 5). Every read therefore starts some rows above the
kept start: the block, the rows directly above the screen's top at the
last read, at least four, extended upward past blank rows to take in two
with text, at most a screen's height, and while the read holds fewer than
that, four more are asked for at the next read. When the read shows the
block where it was, nothing rotated. Otherwise the alignment below is
made with the rotation free, and its topmost kept row tells the rotation
when it moved up, its text is on no other row of the old screen or of the
read, and the rotation agrees with what of the block the read shows.
Otherwise the memory asks for the read again from the same start
(`search_above`), a screen further above, then four times as far each
time, and takes the lowest place it finds the block, the smallest rotation
the text allows; a read that reached the document's start may find the
block cut by the buffer's top. With the rotation known, a row now above
the screen is an old screen row only where that rotation puts it. A read
reaching the document's start without the block means the history
rotated past it: the old screen is searched for anywhere, and the rows
above a row kept from it that is now in the history and whose text is
unique, and that moved up at least past the block, are history; with no
such row, as when only a footer was kept, every row above is output that
went by and the history overflowed. A refused start whose read, moved up
by the block, reached the document's start is taken as a whole-document
read at once.

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
are kept unmoved, and the others kept as moved. A screen with no history
above it, as the alternate screen, never rotates: rows a scroll moved up
are kept as moved, and those it moved off the top are gone. A longest common
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
above the kept ones, by the histogram method (the rarest shared row
first, the one nearest the middle among equally rare, at its place
nearest the old screen, the match extended above and below it while the
rows agree, then the rows before and after that run the same way), when
a row with text of the old screen was kept, or the old screen had none:
in a normal read the matched rows are a reprint (section 4.4), and in a
whole-document read or a restored memory they are history found again.
A whole-document read that lost the old screen matches them too, as
history found again, to tell how much went by. Output that only repeats
the known history, after a clear that took the old screen with it, is
new. A whole-document read after the document shrank, whose block is
still at its place counted from the document's start, kept the history:
only rows of the screen were erased, and the rows above are where they
were.

When the old screen had text and none of it was kept, or the read's start
was refused, and the read does not start at the document's start, the
memory asks for the whole document and changes nothing until it comes;
the old screen is then searched for in all of it, anchored by the block
where it is lowest when the read lost the old screen (no lower than it
was, when the memory's rows began at the document's start, since no row
of the history moves down), and when the old screen is not found in a
document longer than a screen and no row of the known history is found
either, the history overflowed. So did a read from the kept start that
reached the document's start, longer than a screen, when no row of the old
screen was kept but rows that moved down with the end of the text. With
the history overflowed, the review position stays on its row when that
row was kept, and goes to the caret otherwise; a command mark stays only
on a kept row whose line is its line.

A read whose screen starts at the document's start, after one whose screen
did not, is a switch of screens or a clear that looks like one (section
5.8): it is aligned as usual, then the memory before it is kept aside with
its anchors, and the new screen's memory starts with the review position
at the caret. When the screen no longer starts at the document's start,
the memory kept aside is restored if every row with text of its old
screen is kept in the read, unless the read shows, where they were, rows
with text read since the switch: then the screen never changed, as when a
program erased the history, and restoring would speak those rows again.
Otherwise it is dropped, and the rows of the alternate screen's memory are
not taken as rewritten into the main screen's rows.

Anchors move with the alignment: a kept or rewritten row carries its
anchor to its new place, the column clamped to the text, and so does a
blank row written over in place, which the alignment, pairing no blank
row, finds gone; a gone row passes the review position to the nearest
surviving row above, or to the caret; above the read it moves with the
shift and the rotation while the buffer still holds its row, and goes to
the caret once its row left the buffer. A command mark moves to the first
row of its line, and is dropped when its row was rewritten or is gone.

### What the memory keeps

Section 4.1 needs of the last read only the screen, the rows written below
it, and the block above it; the rows above the block matter only as hashes
in the known history, and the review commands read history rows from the
provider (section 5.6). So once a read is handled, `trim` drops the rows
above the block, putting their hashes in the known history, and the
reading rules drop their state of those rows with it. The reader trims
after every read, and every read first trims what the last left, so the
alignment always works on the trimmed memory. Nothing counted from the
last read's first row moves: the memory keeps how many rows it dropped,
and anchors, lingering rows, the caret and what the next `Outcome` says
of old rows are still counted from that row. The first row kept carries
whether the row above it wrapped onto it.

The next read finds a dropped row only by its hash. A read that reaches
it, the whole document or a read that restores a memory, keeps it where
the shift puts it when the read's row there has its hash, and its
history starts anew; otherwise it went above the read, left the buffer,
or is gone, as a row above the old screen does. The known history keeps
a dropped row's hash only when the row went above the read, so it holds
what it held when the memory kept every row of the last read, and only
while the buffer holds that row (section 4.1): a rotation takes the
oldest hashes with the rows that left, after any rows above the known
history that a read from the document's start counted, and a read from
the document's start, the whole document's among them, holds no row
above it, so the known history becomes that read's rows above the block.
It is so bounded by the terminal's history: 8,997 hashes, measured after
100,000 lines through a 120 by 30 screen with 9,001 rows of history. A
limit of 32,768 hashes bounds only the hashes a rotation measured short,
the smallest one consistent with a history of repeated text, leaves until
a read reaches the document's start. A whole document of 9,000 rows
therefore leaves the same memory as an ordinary read: about 34 rows of a
120 by 30 screen and its block.

### Cost

In a release build on Dickson's x64 machine, with 120 by 30 screens of
ASCII text: a read of the last screen and one line more takes about 24
microseconds, most of it splitting the text into rows and lines, the
dynamic programming over 30 by 61 cells a small part; the block adds its
four rows to every read, which measured beside the memory without it
(9,000 rows of history, 2,000 reads each way) cost nothing measurable,
29.5 against 29.8 microseconds a read, and about 2.6 microseconds, 30.8
against 28.2, to a footer over five streamed lines whose rotation the
kept rows tell. A footer over more lines than the screen holds, which
kept rows cannot tell, takes the read twice more, a screen and then four
screens further above, about 350 microseconds of the memory's time in all
against 225 for one read that missed the rotation, beside the provider's
0.2 to 0.4 milliseconds a search read (section 4.3, item 5); a screen rewritten
whole about 117 microseconds, the likeness pairing of 30 rows by 30; a
read of 1,000 new rows after a screen 1.2 milliseconds; a whole-document
read of 9,030 rows 12 milliseconds, against the 9.4 to 29 milliseconds
the read itself takes. A reprint as Pi makes one, the whole transcript of
9,029 rows printed again after a clear with one row above the screen
taken out, read whole against a known history of 8,996 rows, takes 5.6
milliseconds, measured on 2026-10-24; with one row in ten changed as well,
11 milliseconds. The read program's added instructions for the rows
above the start are five (`docs/performance.md`).

With the reading rules on top, measured on 2026-10-24 with a counting
allocator in a release build, 120 by 30 with a full history of 9,001 rows:
an ordinary read of one line more takes about 35 microseconds and a
whole-document read about 9 milliseconds. Held between reads, a
terminal's memory and rules take 122 to 127 KB after either kind of read,
measured on 2026-10-24 after 100,000 lines, most of it the known
history's 8,997 hashes (72 KB) and its queue's spare capacity; the rows
kept take about 400 bytes each with the rules' state.
Handling a whole-document read raises the peak by 4.5 MB: the read's rows
and lines, the outcome, and a small state for each row, with no line state
for the lines above the rows the memory will keep.

### Tests

`src/terminal_rows/tests.rs` drives the memory with `verbatim-term-sim`,
reading as the outpost does, one case of the design each, asserted whole:
plain output, a scroll, rotation, a footer kept at its coordinate while
the history rotates, a footer redrawn in place, Pi's status row drawn a row
lower with its spinner changed, Ink's region erased and drawn again in one
write and caught erased between two, a repeated row kept in place rather
than paired with its copy, a row moved past others kept where its text
went, a wrapped line split across the read's top, Windows Terminal's cut
last row, a clear and a reprint, output repeated after `cls`, output
repeated after a clear that took the old screen, the alternate screen
opened and closed, a scroll on the alternate screen, the review position
through a scroll, a rotation and a redraw elsewhere, and on a row gone, a
rotation the kept rows cannot tell found by reading again further above, a
footer over more output than the history holds, the known history moving
up with the rotation and taken from a whole-document read, and a history
erase not restored once rows read since it are shown.

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
  over the whole document, `search_above` when it is to be taken again
  from the same start with that many rows above it, the read's
  `Outcome`, and `followed`, whether a key moved the caret within its
  window and the review position followed it. `Speech` is a new
  line (`Line`), what a growing line gained (`Growth`), a redrawn row from
  the word that changed (`Changed`), a held line that left the read before
  it was spoken, as it was last read (`Held`, naming no row), a flood's
  count (`Skipped`, with the
  first rows of its lines and how many, `Counted`: exactly, more than when
  the history overflowed, or not known when it overflowed and every line
  it holds is spoken),
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

The reader keeps state for the rows the memory keeps, and trims it with
the memory once a read is handled: a line that starts above the first row
kept moves its history to that row, the text above it counted as above
the read. A line's text, what was said of it, and its row's text are one
shared string while they are the same, so a row read whole costs one copy
of its text. While a read is handled the reader reads the memory's rows in
place, and keeps no history for a line above the rows the memory will
keep that holds nothing, so a whole-document read does not build a line
state for each of its thousands of rows.

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
  not find the last screen counts them as more than, or, when it speaks
  every line it holds, says lines went by without a count, after the last
  line spoken.
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
- A held line that left the read with no line of the read carrying it,
  above the read's start, out of the buffer, or gone when the history
  overflowed, is spoken as it was last read (`Held`: the unfinished word,
  or a volatile row from the word that changed), before the read's output
  and its count of lines skipped (section 4.3, item 5).

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
memory is taken from a read that asks for the whole document or a search
further above, rather than taking that read (`TerminalMemory::restart`):
the command marks and the review position are kept, each moved as the
read's start moved, or kept when it is above the read, where it cannot be
checked; a mark whose line is not where it should be is dropped, and the
review position then goes to the caret. When where the memory's rows are in
the read is not known, as when Windows Terminal refused the read's start,
the marks are dropped and the review position goes to the caret. Keys are
still answered.

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
first and last lines with the rest counted, counted as more than past
the history, and not counted when every line the history holds is
spoken, the input reprinted below a completion list, padding; a
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
off, and its marks and review position through a flood; an unfinished
word and a volatile row
pushed out of a full history, spoken as last read before the flood's
count; Pi streaming above its status row, Ink's region drawn again with a
changed cost, and several progress lines settling.

`tests/terminal_oracle.rs` checks the rules against the simulator's
oracle, which judges by text, over the conformance corpus in both
terminals, the oracle's 512 random streams and every stream of up to four
operations of its tiny alphabet, reading at the streams' times. Every
case passes but four random streams the file pins, each with its
violations and one of three open design questions: output repeating
earlier output rotated a full history past the smallest rotation the
block shows, so lines above the read are missed (one stream); the last
read had no history, so no block, and a flood filled and rotated the
buffer, a row whose text was written again in its place taken as kept or
rewritten (two streams); and a row rewritten and rotated into the
history, held as volatile by the reader, which follows the rotation,
while the oracle's premise compares lines at one place counted from the
screen's top (one stream). The corpus and every tiny stream pass.

## Units

Every offset is a byte offset into the UTF-8 string given. A provider's
own units (UTF-16 code units, UIA text ranges, MSAA character offsets) are
converted in the outpost, so this crate never sees them.

## Cost

ICU4X's compiled data is built into the binary. The jieba dictionary is
loaded the first time Chinese text is segmented and kept for the life of
the process.
