# verbatim-text

Platform-neutral text segmentation for the text model of milestone M4
(`docs/design/phase6-design.md`, "Internationalization in the text model"),
and the reading of terminals: the memory of a terminal's rows and the
rules that decide what is spoken (`docs/design/terminal-model.md`).
Original
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
  whitespace characters it is made of; ASCII spaces are dropped eight
  bytes at a time, and only a non-ASCII character has its property looked
  up.
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
memory and alignment (`docs/design/terminal-model.md`, "The model", "The
read", "The alignment", "Lingering", "A read with no text", "Screens" and
"Starting again"), written from that design. It classifies; it does not
speak: the reading rules (`terminal_reading`, below) are built on its
`Outcome`. Core's terminal model runs both on every read a focused
terminal's outpost sends (`docs/crates/verbatim-core.md`, "Core's terminal
model").

### Public API

- `Read`: what one read returned, as the read program's snapshot gives it
  (the text from the read's start to the document's end, the screen's
  text, whether the read started where the last said, whether that start
  was refused, whether the read and the screen start at the document's
  start, the document's first row, the caret's row and column and how many
  rows its row and those below hold, and how many rows it holds above the
  start it was asked for), with the text as UTF-8 and the time of the read
  in milliseconds. The start for the next read is an opaque provider range
  the outpost keeps. The text from the read's start comes without its
  lines' padding (`ReadText`); the screen, the first row and the caret's
  row, a screen at most, come padded.
- `ReadText`: a read's text without the spaces that pad its lines
  (`text`), and each line's length with them in characters, as runs of
  lines of one length (`lengths`), the form in which a read crosses to
  Core (`verbatim_model::TerminalText`). `unpadded` makes one from padded
  text, and `padded` gives that text back. The memory splits a read into
  rows from it, each line's padding counted from its length and never
  copied, so a full history of short lines is split from about a tenth of
  its padded size.
- `TerminalMemory::new` and `TerminalMemory::read(&Read) -> Outcome`: one
  memory per terminal, kept while the terminal exists. `above` gives how
  many rows the next read takes above its start: a screen, as the last
  read found it, and none before the first read, which is read again with
  a screen above its screen when the document goes on above it
  (`history_block`). `rows` gives the last
  read's rows as `Rows`, counted from the read's first row: `len` is how
  many the read held, `first_kept` the first the memory keeps, `kept`
  those rows and `get` one of them (`Row`: its text without padding, a
  `Text`, a 64-bit hash, whether it wraps onto the next row, the hash of
  its whole line, and whether a program has rewritten it). `trim` keeps only the screen above the screen and the rows below
  it, and `first_kept` says before it from which row it will keep.
  `review` and `set_review` give the review position,
  and `marks` and `add_mark` the command marks, each an `Anchor` (a row
  counted from the last read's first row, negative in the history above
  it, a column in characters, and its line's hash), set only on a row the
  memory keeps; `set_review_anchor` puts the review position at any
  anchor, a history row above the rows kept among them, whose line's hash
  the caller read, and `anchor` makes one on a row the memory keeps.
  `add_place`, `place` and `drop_place` keep places, numbered anchors the
  caller holds in the text (Core's review answers' rows), which
  move with their rows as the review position does, and are dropped when
  their row cannot be followed. `restart(&Read)` takes a read for which the memory
  asked for the whole document without that read, for a caller that does
  not take it ("report new output" off): the memory starts again from it.
- `Outcome`: for each row of the read a `NewRow` (`New`; `Kept`, from
  which old row and whether it moved other than with the history;
  `Grown`, an old row whose text grew where it was, which counts as kept;
  `Rewritten`, from which old row, whether it moved, and whether it moved
  alone, with no neighbour kept in the same move; `Returned`, a
  lingering row taken back, and whether elsewhere; or `Shown`, a row whose
  text was shown before but that carries no row of the last read: history
  above the rows the last read's are found at); for each old row an
  `OldRow` (`Kept` or `Rewritten` and where, `Gone` and whether it
  lingers, `Above`, past the read's start, or `Left`, out of the buffer
  past the document's start); the read's `Line`s (rows joined while each
  wraps, with their text, a one-row line's its row's, and hash,
  and `grew_from` when a line that did not move only gained at its end, so
  the text after that offset is what it gained); the rotation, how many
  rows the last read's rows moved up as the alignment found them; whether
  the first row continues a line from above the read; the caret's row; the
  screen's width and height; a `Switch` of screens (`New`, a new screen);
  for a lost read, read whole, how what went by is counted (`lost`, a
  `Lost`: `About`, `MoreThan` or `Uncounted`); whether the last read's
  rows were found by their text (`by_text`); `first`, the memory's first
  read; and, each set alone with the
  memory unchanged, `whole_document`, when the caller is to read the whole
  document and give that read instead, `history_block`, when a first read
  holds no rows above its screen though the document goes on above it and
  the caller is to read again with no start, counting the screen up from
  the document's end by the outcome's height and reading that many rows
  above it, and give that read instead, and `blank`, when the read's
  screen shows no text where the last read's had text.
- `Text`: a row's or a line's text, a stretch of a shared string that
  reads, compares, prints and dereferences as that stretch alone (`as_str`,
  `len`, `is_empty`, and `str`'s methods through `Deref`). The rows of one
  read share strings of about 64 KB or 1,024 rows of their texts,
  whichever comes first, so splitting a read is a few allocations however
  many rows it holds, the rows waiting for their string stay a few tens of
  kilobytes, and a wrapped line's text is the stretch its rows make up
  together. The rows the memory keeps
  after a read are given strings of their own before anything takes their
  text, so that nothing the memory or the reading rules keep holds a
  whole read: what they keep is the size of the rows they keep.
- `hash`: the 64-bit hash of a row's text, the same in every process and
  build: the text taken eight bytes at a time as little-endian words, each
  mixed in by a 128-bit product of the hash and a constant whose two
  halves are combined by exclusive or, with the length mixed in first and
  a last such product at the end. It reads a word where FNV-1a, the hash it
  replaced, read a byte, about eight times fewer dependent
  multiplications.
- `rows_of`: a read's text, padded, split into rows as the memory splits a
  read, for a caller that reads rows the memory does not keep, the history
  above them, at the width a read found.
- `shows_text`: whether a read's screen shows text, anything but padding
  and white space, the one test of "A read with no text", for the memory
  and for the caller that keeps where the next read starts.
- `LINGERING_MS`: how long a gone row lingers, 2 seconds.

### How a read is aligned

The read gives text, not coordinates, so a row's place is its index in
the read that found it. A read's rows are its text split at line breaks
and each line, its padding counted as spaces after its text, split into
rows of the screen's width; padding is left out of a line's last row
only, so a wrapped line's earlier rows keep the spaces where it wrapped,
and a line whose padding fills whole rows has rows with no text, as the
padded text had. A line is measured cell by cell only when it may be
longer than a row: one of no more bytes than the width with its padding,
as a padded line of ASCII is, or of no more characters than the width
all below U+0800, is one row, since no character takes more cells than
its bytes and none below U+0800 more than one (a unit test checks both of
`unicode-width`). A one-row line's hash is its row's, and a wrapped line's
is computed once, as its rows are split. The width is the narrowest of the screen's lines
but its last, every line being a whole number of rows except Windows
Terminal's last row of the buffer, which is one cell short. When every
line but the last wraps, that narrowest line is several rows, so the
caret's row, which the read gives as one row, is used instead when its
length, or that length and the cut cell, fits every screen line; when
both fit, the one that gives the height of the last read wins. A changed
width or height starts the memory again.

Each read starts one screen above the top of the last read's screen, at
the provider range the last read kept; a refused start (the document
shrank below it, as after Windows Terminal's clear) reads from a screen
above the screen instead. A first read, which knows no screen's height,
takes only the screen; when the document goes on above it, it is read
again at once from a screen above the screen, so that every read the
memory holds has its history block. The memory holds only the last read.

Positions are trusted when the read's rows above the last read's screen
read exactly as the last read's history block did in the same places and
at least one of them has text, or when no row can be compared, which
happens only when the last read's screen started at the document's start,
so that it held no rows above it. Rows only one of the two reads holds
are not compared. The
last read's rows are then where they were: the history block is kept in
place and every row below the end of the last read is new. Otherwise, as
once the history is full and the buffer moves under fixed places, the last
read's rows are found by their text.

Either way, the last read's rows are aligned with the read's by a longest
common subsequence of the hashes of the rows with text, by dynamic
programming in which each cell keeps the best alignment ending on the same
shift (which continues a block) and the best anywhere above and to the
left (which starts one), so among alignments keeping equally many rows the
one with the fewest blocks wins, and then the one moving the fewest rows.
Found by text, a row of the history block is found at its place or
higher, or lower by the one shift the last read's screen rows with text
took when they went lower, the commonest by a vote over their hashes,
taken only when a row of the history block has its text lower than its
place:
the same output printed twice in a full terminal puts the history's text
lower while the screen goes up with the buffer, and a program printing
its history again longer (Pi's regular mode on Control+O) moves both
down. Its place is where the read's start puts it, or, with the start
refused, where the document's start puts it when both reads began there;
a screen row is not limited, since a footer moves down. The shift most kept rows share is the one that did not move,
and gives the rotation. A row the subsequence leaves gone is kept at the
nearest new row with its text, whatever else moved and whether or not the
text is unique, except a row of the history block, and no other row grows
into it; a row whose text grew where it was, its new text
starting with its old, is kept as grown beside kept rows. A blank row is
kept only where a kept row beside it puts it.

Between two kept rows, or a kept row and the read's edge, the old and new
rows left over are paired as rewritten when neither is blank and they
share in order at least half the characters of the longer, counted by the
bit-parallel longest common subsequence of their characters. The pairing
keeps their order and makes as many pairs as it can, and then the most
whose shift is that of the kept row below them, so Pi's status row drawn
a row lower with its spinner changed pairs with its old self. A read with
no kept row pairs nothing. The caret's line that now starts where the last
read's caret line did, otherwise new, is that line rewritten, with
positions trusted, unless it only lost text, compared whole without
spaces, while the caret moved forward along it over text both reads show
unchanged in the same columns, counted in characters along the line from
the caret column the memory keeps with the caret: then another line was
written there, as Windows Terminal's `cls` writing the prompt where it
homed the caret. In every read from the kept start, the rows above
where the last read's first row went are `Shown`: the shift most kept
rows share places it with positions trusted or the start refused, and,
found by text from an accepted start, the shift of the history block's
kept rows, with nothing `Shown` when none was kept. `Outcome::rewritten`
says a program may have printed its history again: the start was refused
with something kept, or, found by text, the last read's rows went lower
than their places; what it printed above the last read's reach is not
heard, a listed limit.

An old row left over is gone, and lingers for 2 seconds when a program
had rewritten it or it was at or below the caret. A new row with a
lingering row's text is that row back (`Returned`), wherever it comes back.

A read is lost when it keeps and pairs nothing of the last read and is no
new screen, positions trusted or not, or when positions are not trusted,
its start was not refused, and it keeps no row with text that the last
read had at or above the caret, or keeps them equally well at more than
one place (two shifts tie for the most rows with their text). The memory
then asks for the whole document and changes nothing until it comes. When
the aside screen (below) is found in the whole document, the read is a new
screen; otherwise the whole document is aligned with the last read's rows
the same way: found at one place, the rows above
the topmost kept row are `Shown` and the rows below the last read's are
new, counted as about (`Lost::About`); found nowhere at or above the old
caret, every row is new, counted as more than (`Lost::MoreThan`), or as
about when the read that asked for the whole document had its start
refused, the document having shrunk under a clear (`clear; dir`); fitting at more than one place, only the visible
screen is aligned, the rows above it `Shown`, and what went by has no
count (`Lost::Uncounted`).

A read that keeps and pairs nothing of the last read, rows below the caret
included, is a new screen (`Switch::New`) when its own screen starts at
the document's start or when the aside screen is found in it: its visible
rows are new, the rows above them `Shown`, the review position goes to
the caret, and the command marks wait for their lines. A refused start
with neither is a lost read.

A new screen sets aside the screen it replaced, the last read's screen and
history block, as a map from each row's hash to the least distance from
the document's end a row with it had, for rows with text only, in place of
any earlier aside screen. It is found in a read when a row with text of it
is there no nearer the document's end than it was, its place or higher. A
read that keeps or pairs something of the last read while its own screen
does not start at the document's start drops it, so a full-screen
program's own reads, on a screen at the document's start, keep the shell's
screen aside until the program closes. It is used only to recognise that
screen coming back, never spoken.

A read whose screen shows no text where the last read's had text, as
between a program's clear and its reprint, changes nothing (`blank`), and
the caller keeps where the next read starts as the last read placed it.

A changed width or height starts the memory again from the read, nothing
aligned: each anchor is found again by its line's hash nearest its old
place, from the read's end, at its character position within its line, so
a resize keeps the review cursor on its character however the line
rewraps; the review position not found goes to the caret, and a mark or a
place not found is dropped. A lost read with "report new output" off
starts again the same way (`restart`).

Anchors move with the alignment: a kept or rewritten row carries its
anchor to its new place, the column clamped to the text; a gone row passes
the review position to the nearest surviving row above, or to the caret;
above the read it keeps its place relative to the alignment's shift,
clamped to the document's start when the read began there. With the
read's start refused, the rows above the read are gone, the document
having shrunk beneath them: an anchor there is found again by its line's
hash nearest its old place, from the read's end, or the review position
goes to the caret, and a gone row's search for a surviving row above
stops at the read's first row.
A command mark moves to the first row of its line, and is dropped when its
row was rewritten or is gone.

### What the memory keeps

The memory needs of the last read only the screen, the rows written below
it, and the screen above it, its history block; the review commands read
rows above that from the provider. So once a read is handled, `trim` drops
the rows above the screen above the screen, and the reading rules drop
their state of those rows with it. Every read first trims what the last
left, so the alignment always works on the trimmed memory. Nothing counted
from the last read's first row moves: the memory keeps how many rows it
dropped, and anchors, lingering rows, the caret and what the next
`Outcome` says of old rows are still counted from that row. A whole
document of 9,000 rows therefore leaves the same memory as an ordinary
read: about 60 rows of a 120 by 30 screen. Beside it the memory holds
the aside screen while one is set aside, at most 60 hashes and distances.

### Cost

Measured on 2026-10-25 on Dickson's x64 machine in a release build, 120 by
30 with a full history of 9,001 rows, the memory's and the rules' time
together, in either terminal (`tests/terminal_cost.rs`, an ignored test;
`docs/performance.md`, "A terminal's rotation"): an ordinary read about 50
microseconds, and 32 to 42 for a rotation of 58 rows, settled in one
read; a rotation past that, which keeps no row of the last read, is
lost and reads the whole document, 1.4 to 1.8 milliseconds of the
memory's and rules' time beside the read's own; an overflow that keeps
nothing, every line spoken, 1.3 to 1.7, and 2.1 to 2.4 for lines of
Cyrillic text one cell short of the width; a clear and a reprint of 8,905
rows 1.3 to 1.4; and the worst case for the subsequence, 8,000 lines drawn
from the last screen's texts, 1.8 to 2.1. Live in the console host, the
read program's snapshot takes 0.45 to 1.06 milliseconds with the screen
above, against 0.37 to 0.75 without it, 7,318 UTF-16 units against 3,658;
a whole document 25 to 29 milliseconds, 1.1 million units. The read
program is unchanged by the screen above: its rows above the start cost
the same five instructions however many there are.

With the reading rules on top, measured on 2026-10-25 with a counting
allocator in a release build, 120 by 30 with a full history: held between
reads, about 21 KB, the rows kept taking about 450 bytes each with the
rules' state, and nothing else that grows with the history. Handling a
whole-document read raises the peak by 3.6 MB for numbered lines and 5.6
MB for full rows: the read's rows and their texts in shared strings, the
lines, the outcome, the speech, and a small state for each row, with no
line state for the lines above the rows the memory will keep.

### Tests

`src/terminal_rows/tests.rs` drives the memory with `verbatim-term-sim`,
reading as the outpost does, one read and the whole document when the
memory asks for it, one case of the design each, asserted whole: plain
output, a scroll, rotation within one and two screens, blank rows above
the screen that read the same leaving the rows found by text, a lost read
counted as about and one that fits twice left uncounted, a footer kept at
its coordinate while the history rotates, a footer redrawn in place, Pi's
status row drawn a row lower with its spinner changed, Ink's region erased
and drawn again in one write and caught erased between two, a repeated row
kept in place rather than paired with its copy, a row moved past others
kept where its text went, a wrapped line split across the read's top,
Windows Terminal's cut last row, a clear and a reprint, output repeated
after `cls` and after a clear that took the screen with it, the alternate
screen opened and closed, each a new screen, a pager's pages, a scroll on
the alternate screen, Windows Terminal's clear, a flood larger than the
buffer from a fresh screen and from Windows Terminal's cleared one, each a
lost read counted as more than, a pager and a full-screen program closing
over a long history and over a full one, each found by the aside screen,
the aside screen found in a lost read's whole document and dropped once a
read on a screen with history keeps something, a page that keeps no row
setting the page before it aside (a listed limit), the review position
through a scroll, a rotation and a redraw elsewhere, and on a row gone, a
footer over more output than the history holds counted as more than, a
history erase keeping its rows, blank screens between a clear and a
reprint, and a lingering row taken back. One more checks that no
character takes more cells than its bytes and none below U+0800 more than
one, which splitting a read into rows relies on.

The memory is also checked by the oracle, through the reading rules built
on it (below). `tests/terminal_cost.rs` is a measurement, not a test,
ignored so that no test run includes it: the memory's and the rules' time
for the costliest reads, run in a release build (`docs/performance.md`, "A
terminal's rotation").

## The terminal reading rules

`terminal_reading` is the platform-neutral core of the terminal model's
reading rules (`docs/design/terminal-model.md`, "Reading rules" and "The
rules in detail"), written from that design on top of the row memory. It
is a state machine with no clock and no thread of its own: every input
carries its time, and every input answers with what to say, what it
withholds and why, and when the quiet-period timer should next fire.
Reading on demand and moving by command are not built here; they act on
the memory, which the reader lends out. Floods are Core's to split
("Core's flood policy"): every new line goes to Core.

### Public API

- `TerminalReader::new(Settings)`: one reader per terminal, owning its
  `TerminalMemory` (`memory`, and `memory_mut` for the review commands).
  `Settings` holds "report new output" and "review follows caret".
- The inputs, each returning a `Step`:
  - `read(&Read, waited, colours)`: a read of the terminal, with whether
    it waited for Core's speech queue and, for a read in a key's window,
    the screen's colour runs (`ColourRun`), row by row from the screen's
    top to the document's end.
  - `key(Key)`: a key as it reached the program, with when. `KeyKind` is a
    typed character (`Typed`), a caret key Verbatim knows with its `Unit`
    (character, word or line), Enter (`Enter`), which forgets held typing
    and is answered only by output, a key that forgets held typing
    (`Forget`: Tab, Escape, Control+C, Control+D, Control+Break), or any
    other key (`Other`).
  - `focus(at_ms)` and `blur()`: the focus arriving and leaving.
  - `timer(now_ms)`: the timer the last step asked for came due.
  - `set_settings(Settings)`.
- `Step`: the `Speech` in order, the lines `Withheld` with a `Reason`, the
  `timer` due time, `whole_document` when the read is to be taken again
  over the whole document, `history_block` when a first read is to be
  taken again with a screen above its screen, the read's `Outcome`, and
  `followed`, whether a
  key moved the caret within its window and the review position followed
  it. `Speech` is a new line (`Line`, a blank one with empty text, which
  Core counts and does not speak; its text a `Text` sharing the read's
  string, so a caller that only counts the line, as Core's flood policy
  counts the lines it skips, never copies it), what a growing line gained (`Growth`), a
  redrawn row from the word that changed (`Changed`), a held line that
  left the read before it was spoken, as it was last read (`Held`, naming
  no row), a lost read (`LostRead`, before its lines, with how Core counts
  those it skips), typing shown (`Echo`), a key's answer on the caret's line in a
  unit (`Caret`), text the caret's line gained or lost (`Gained`, `Lost`),
  a row away from the caret that is a key's answer (`Answer`), and a
  suggestion after the caret (`Suggestion`). Each names the first row of
  its line in the memory's last read. A `Reason` is one of the design's
  silences: `Volatile`, `UnfinishedWord`, `HeldForKeys`, `Spinner`,
  `LostText`, and `WrittenBack`, a listed limit: text erased and written
  back within the time a gone row lingers.
- The time limits ("Every time limit") as constants: `KEY_WINDOW_MS` and
  `TYPED_LIMIT_MS` (300), `VOLATILE_MS` and `UNPROMPTED_MS` (2,000),
  `QUIET_MS` (1,000), and `quiet_period`, 1 second or 1.5 times the
  interval between the last two changes when that is longer.

### How the rules are built

Beside the memory's rows the reader keeps, for each row, its colours when
last read and, on a line's first row, the line's history: its text as last
read, what the user was last told of it, whether it was redrawn and is
volatile, when it last changed and the interval before, when it last
changed outside a key's window, when it last grew, what it waits for, and
whether a key changed it since it appeared. That history moves with the
alignment as the memory's does: from the old line a kept, grown or
rewritten first row started, or that the read's first row continues; a
row rewritten alone keeps what it last said but starts its change history
afresh; a lingering line taken back brings its history with it. A line the read's
first row continues keeps the part above the read in its text, and speech
quotes only the part the read shows.

The reader keeps state for the rows the memory keeps, and trims it with
the memory once a read is handled. A line's text, what was said of it, and
its row's text are one shared string while they are the same. While a read
is handled the reader reads the memory's rows in place, and keeps no
history for a line above the rows the memory will keep that holds nothing,
so a whole-document read does not build a line state for each of its
thousands of rows.

Each read, line by line, top to bottom:

- The first read, the first after the focus arrives, a read while the
  focus is away, and a read of a new size speak nothing; each line's text
  becomes what the user knows of it. A new size speaks the caret's line
  when it changed. A blank read is idle: nothing is said or withheld.
- A line with no history whose rows with text are new is new output,
  spoken whole, unless it is the input the shell reprinted: the caret's
  new line, equal to the caret's line at the last read with any typing
  since, that line holding input and still above with new rows between.
  After a lost read `LostRead` comes first.
- A line that grew, and whose last read was all heard or held back only
  by its unfinished word, speaks what it gained since what was said. When
  the caret is at its end after a character that is not a space, the last
  word is held (`UnfinishedWord`) and spoken when the line grows past it,
  when the caret leaves the line, or after the quiet period.
- A line rewritten in place is a redrawn row. Within a key's window, away
  from the caret, and with no change of its own in the 2 seconds before
  the key, it is the key's: one that gained text other than at its end is
  the answer, spoken whole; one that only lost text is silent; any other is
  held until the keys pause (`HeldForKeys`). Otherwise a row that only
  lost text is silent (`LostText`), a spinner's symbol replaced alone is
  silent (`Spinner`), and a row that changed within 2 seconds of its last
  spoken change, or at two reads in a row when the second waited for
  speech, is volatile: withheld (`Volatile`) and spoken once it has not
  changed for the quiet period. Any other change is spoken by the
  changed-word rule: from the start of the word where the line first
  differs from what was last said or from its last read, whichever is
  earlier, to its end.
- Kept, grown with nothing to say, and shown lines are silent; such a line
  that now holds less of its old line's text is withheld as `LostText`. A
  line whose rows are lingering rows taken back is withheld as
  `WrittenBack`.
- A held line that left the read with no line of the read carrying it,
  above the read's start, out of the buffer, or gone after a lost read, is
  spoken as it was last read (`Held`), before the read's
  other output.

Keys open a window of 300 milliseconds after the last of them reached the
program; a key arriving while a window is open joins it. The caret's line
before the window's first key, without any suggestion, is what its first
read compares with. At a read in the window, the caret's line is the keys'
answer and is not output: typing shown at the caret is echoed, and typing
not shown within 300 milliseconds of reaching the program is dropped
unspoken; a caret moved with no text change says a caret key's unit, or
for any other key the landing rule; a caret that ended on another line
says that line, whatever changed on it; a line that gained text says the
gain, one that lost text next to the caret says the loss, and one changed
otherwise speaks from the changed word. A new line at the caret is
output. Enter is answered only by output, never by the caret's line, as
NVDA turns off its new-line announcement in terminals: when Enter is the
newest key, the caret's line is not answered, and what changed is read as
output. With "review follows caret" on, a key that moved the
caret moves the review position to it; the focus arriving does so too.

Colours are compared in every read with colours in a key's window,
which the caller takes as every read that begins in it. A read in the
window without colours, taken before the caller asked for them, answers
the caret's line as any other does, the echo included, with the last
suggestion carried where it still shows after the caret, less what was
typed along it; text after the caret on its row that was not typed, when
the line with it blanked is the typing or a move, is held (a hold of its
own beside the volatile row's, the unfinished word's and the keys'
pause) for the next read with colours in the window, which speaks it as
a suggestion or as text gained, and is spoken as gained when the window
ends without one. Among rows whose
text did not change, the one row whose colours changed, or with no colours
read before the one row that differs from both its neighbours, is the
answer, when exactly one stands out. Text after the caret in other colours
than the cell before the caret is a suggestion, spoken when it appears and
not again while typing follows it.

"Report new output" off speaks no output and holds nothing, and takes no
whole-document read: a read that asks for one starts the memory again
from itself (`TerminalMemory::restart`), each anchor found again by its
line. Keys are still answered.

### Serialization

With the optional `serde` feature, which `verbatim-core` enables, the
reader, its memory and every type they hold serialize and deserialize
(Dickson, 2026-10-26), so Core's state, which
holds a terminal's reader, is dumped and replayed by the flight recorder
whole. A row's text is written as its text alone, and restored as a
string of its own; the segmenter holds no state and is made afresh. The
reader, the memory and their parts are also `Clone`, `PartialEq` and
`Eq`, as Core's state is.

### Cost

The rules' time is in the memory's figures above. Nothing is done between
inputs, and the only wake-up the rules ask for is the quiet-period timer,
while something waits for it.

### Tests

`src/terminal_reading/tests.rs` drives the rules with `verbatim-term-sim`,
reading as the outpost does at given times, with keys as they reach the
program, one rule or one case of the design's "Evidence" each, asserting
the whole step: new output line by line, blank lines given to Core, a line
seen first spoken whole, growth with its unfinished word held and spoken
when the line grows past it or after the quiet period, kept rows silent, a
whole-document read after a lost read, the input reprinted below a
completion list, padding; a redrawn row's changed word, a spinner, a
volatile row heard at its first change and once settled, a row volatile at
two reads after waiting, a row that only lost text, a held row scrolled
above the screen; typing echoed, shown late, never shown and changed
otherwise, the landing rule, a known caret key, text gained and lost at
the caret, Up Arrow and a character at once, a change after the window,
the caret moved by a key to another line that also gained text, and a read
inside a program's redraw; the review cursor following a key, not
following with the setting off, and moved by the focus; a marker list, a
meter that is not the key's answer, a ruler held until the keys pause, a
filtered list, a highlight moved by colour, a suggestion; the alternate
screen; Windows Terminal's clear; a resize; output in the background;
"report new output" off, and its marks and review position through a
flood; Pi streaming above its status row, Ink's region drawn again with a
changed cost, and several progress lines settling.

The harness reads as the outpost does, every read that begins in a key's
window with the screen's colours. Tests take a read without colours in a
key's window and show the echo spoken at once, the suggestion and the
emphasis left to the next read with colours, text after the caret spoken
as gained when the window ends without one, and a suggestion typed along
carried through it unspoken. A reader serialized after each input of a session and restored
gives the same steps as the reader never serialized. And every stream of
the conformance corpus, in both terminals, with each read followed by
another taken with nothing written between, at the time of the read after
it, says exactly what it says with each read once, the quiet-period timer
run when it comes due, and the read taken again says and withholds
nothing: a read identical to the last but for its time changes nothing
heard.

`tests/terminal_oracle.rs` checks the rules against the simulator's
oracle, which judges by text, over the conformance corpus in both
terminals, the oracle's 512 random streams and every stream of up to four
operations of its tiny alphabet, reading at the streams' times, with
nothing pinned: a failure is a consistent explanation the oracle accepts,
a listed limit through its own reason, or a defect. Each listed limit the
rules state has its smallest stream as a fixed test.

## Units

Every offset is a byte offset into the UTF-8 string given. A provider's
own units (UTF-16 code units, UIA text ranges, MSAA character offsets) are
converted in the outpost, so this crate never sees them.

## Cost

ICU4X's compiled data is built into the binary. The jieba dictionary is
loaded the first time Chinese text is segmented and kept for the life of
the process.
