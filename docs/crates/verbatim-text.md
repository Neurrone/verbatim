# verbatim-text

Platform-neutral text segmentation for the text model, and the reading of
terminals: the memory of a terminal's rows and the rules that decide what
is spoken (`docs/design/terminal-model.md`). Original code over ICU4X's
segmenters, jieba, and `unicode-width`; it never touches a Windows API,
and `cargo xtask ci` checks that it does not.

Every offset is a byte offset into the UTF-8 string given. A provider's
own units (UTF-16 code units, UIA text ranges, MSAA character offsets) are
converted in the outpost, so this crate never sees them. ICU4X's compiled
data is built into the binary; the jieba dictionary is loaded the first
time Chinese text is segmented and kept for the life of the process.

## Segmentation

- `graphemes` and `grapheme_at`: characters as grapheme clusters, so an
  emoji sequence, a letter with combining accents, a Hangul syllable, or
  an Indic conjunct is one character, and a surrogate pair is never split.
- `Segmenter::new`, then `Segmenter::words` and `Segmenter::word_at`: words by Unicode's word
  rules with ICU's dictionaries for scripts written without spaces
  (Japanese, Thai, Lao, Khmer, Burmese), or by jieba's dictionary for
  Chinese, as NVDA uses cppjieba. The segments cover the whole text, and a
  run of white space is one segment. White space (`is_space`) is
  Unicode's, with the zero-width space and zero-width no-break space,
  which Khmer, Thai, and Burmese use to mark word breaks.
- `Segmenter::words_in`: the same with the text's language, whose
  tailoring ICU applies where it has one ("EU:n" is one word in Finnish).
- `WordRules::for_text`: which word rules a text takes, from its language
  tag when the provider gives one, otherwise from its characters' Unicode
  script (Han without kana is Chinese; Japanese mixes kanji with kana).
  Japanese written only in kanji is segmented as Chinese without its
  language, as NVDA's automatic choice does.
- `Segmenter::sentences`: sentences by Unicode's sentence rules.
- `last_pause`: where say-all's speech without pauses splits a piece of
  text (`docs/nvda/speech.md`, "Say-all speaks without pauses"): just past
  its last sentence end, with the whitespace after it. A sentence end is a
  sentence-ending mark (the full stop, exclamation and question marks,
  their full-width Chinese and Japanese forms, the Devanagari danda and
  double danda, the Arabic question mark, the Urdu and Armenian full
  stops, the Ethiopic full stop and question mark) after a character that
  is neither whitespace nor such a mark, with at most one closing quotation
  mark, guillemet, corner bracket, or parenthesis after it, then whitespace
  or the end (none needed after a full-width mark). There is no list of
  abbreviations, so "Dr. " ends a sentence; a decimal point and an
  ellipsis do not.
- `is_line_break`, `lines`, and `line_at`: lines of text Core holds whole,
  such as an object's value reviewed as flat text. A carriage return and
  line feed together are one break; a carriage return alone, a line feed
  alone, the vertical tab, the form feed, the next-line control, and
  Unicode's line and paragraph separators are each one. An offset in a
  break belongs to the line the break ends.
- `cell_width`: how many terminal cells text takes, two for wide East
  Asian characters and none for combining marks.
- `trim_padding`: a terminal line without its trailing padding, whatever
  whitespace it is made of; ASCII spaces are dropped eight bytes at a
  time.
- `composed` and `is_capital`: a character in NFC (ICU4X's
  `icu_normalizer`), and whether it is a capital, some code point
  uppercase and none lowercase once composed, so a letter written with
  combining accents is named and raised as its precomposed form is.
- `is_word_grapheme`: whether a grapheme cluster continues a typed word:
  every code point a letter, mark, or number (ICU4X's `icu_properties`),
  or the zero-width non-joiner or joiner.
- `is_bidi_control` and `strip_bidi_controls`: the bidirectional
  formatting characters (marks, embeddings, overrides, isolates, and their
  pops), which the outposts remove from every name, value, and description
  they read.

## The terminal row memory

`terminal_rows` is the platform-neutral core of the terminal model's
memory and alignment (`docs/design/terminal-model.md`, "The model" and
"The rules in detail"), written from that design. It classifies and does
not speak: the reading rules (`terminal_reading`, below) are built on its
`Outcome`. Core's terminal model runs both on every read a focused
terminal's outpost sends ([verbatim-core](verbatim-core.md), "Core's
terminal model").

### Public API

- `Read`: what one read returned, as the read program's snapshot gives it:
  the text from the read's start to the document's end, the screen's text,
  whether the read started where the last said and whether that start was
  refused, whether the read and the screen start at the document's start,
  the document's first row, the caret's row and column and how many rows
  its row and those below hold, how many rows it holds above the start it
  was asked for, and the read's time in milliseconds. The text comes
  without its lines' padding (`ReadText`); the screen, the first row and
  the caret's row, a screen at most, come padded. The start for the next
  read is an opaque provider range the outpost keeps.
- `ReadText`: a read's text without the spaces that pad its lines
  (`text`), and each line's padded length in characters as runs of lines
  of one length (`lengths`); it crosses to Core as
  `verbatim_model::TerminalText`. `unpadded` makes one and `padded` gives
  the text back. The memory splits rows from it without copying padding.
- `TerminalMemory::new` and `TerminalMemory::read(&Read) -> Outcome`: one
  memory per terminal.
  - `above`: how many rows the next read takes above its start, a screen
    as the last read found it, none before the first read.
  - `rows`: the last read's rows as `Rows`, counted from its first row:
    `len`, `first_kept`, `kept`, and `get` of a `Row` (its text as a
    `Text`, a 64-bit hash, whether it wraps onto the next row, its line's
    hash, and whether a program rewrote it).
  - `trim`: keep only the screen above the screen and the rows below it
    (see "What the memory keeps").
  - `review`, `set_review`, `set_review_anchor` and `anchor`: the review
    position, an `Anchor` (a row counted from the last read's first row,
    negative in the history above it, a column in characters, and its
    line's hash), set on any anchor or made on a row the memory keeps.
  - `marks`, `add_mark`, `replace_mark` and `newest_mark`: the command
    marks, at most `MARKS` (1,000), oldest dropped first.
  - `add_place`, `place` and `drop_place`: numbered anchors the caller
    holds (Core's review answers' rows), which move with their rows and
    are dropped when their row cannot be followed.
  - `restart(&Read)`: take a read for which the memory asked for the whole
    document, for a caller that does not take it ("report new output"
    off): the memory starts again from it.
- `Outcome`, what a read did:
  - for each row of the read a `NewRow`: `New`; `Kept` (from which old
    row, and whether it moved other than with the history); `Grown` (an
    old row whose text grew where it was); `Rewritten` (from which old
    row, whether it moved, and whether alone); `Returned` (a lingering row
    taken back); or `Shown` (text shown before but carrying no row of the
    last read, history above where the last read's rows are found);
  - for each old row an `OldRow`: `Kept` or `Rewritten` and where, `Gone`
    and whether it lingers, `Above` (past the read's start), or `Left`
    (out of the buffer);
  - the read's `Line`s, rows joined while each wraps, with their text,
    hash, and `grew_from` when a line that did not move only gained at its
    end;
  - the rotation, whether the first row continues a line from above, the
    caret's row, the screen's width and height, a `Switch` of screens,
    `lost` (a `Lost`: `About`, `MoreThan` or `Uncounted`) for a lost read
    read whole, `by_text`, `first`, and `rewritten` (a program may have
    printed its history again);
  - each set alone with the memory unchanged: `whole_document` (read the
    whole document and give that instead), `history_block` (read again
    with no start, counting a screen up from the document's end, with a
    screen above), and `blank` (the screen shows no text where the last
    read's had text).
- `Text`: a row's or line's text, a stretch of a shared string that reads,
  compares and prints as that stretch alone (`as_str`, `len`, `is_empty`,
  and `str`'s methods through `Deref`). The rows of one read share strings
  of about 64 KB or 1,024 rows, so splitting a read is a few allocations;
  the rows the memory keeps are given strings of their own before anything
  takes their text, so nothing kept holds a whole read.
- `hash`: the 64-bit hash of a row's text, the same in every process and
  build: eight bytes at a time as little-endian words, each mixed in by a
  128-bit product with a constant whose halves are combined by exclusive
  or, the length mixed in first and a final product at the end.
- `rows_of`: a padded text split into rows as the memory splits a read,
  for a caller reading history above the rows the memory keeps.
- `shows_text`: whether a read's screen shows anything but padding and
  white space.
- `LINGERING_MS`: how long a gone row lingers, 2 seconds.

### How a read is aligned

- Rows. A read's rows are its text split at line breaks and each line,
  its padding counted as spaces, split into rows of the screen's width;
  padding is left out of a line's last row only. A line is measured cell
  by cell only when it may be longer than a row, since no character takes
  more cells than its bytes and none below U+0800 more than one (a unit
  test checks both of `unicode-width`). The width is the narrowest of the
  screen's lines but its last (Windows Terminal's last buffer row is one
  cell short), or the caret's row's length when every line wraps. A
  changed width or height starts the memory again.
- Reads. Each read starts one screen above the top of the last read's
  screen, at the provider range the last read kept; a refused start (the
  document shrank, as after Windows Terminal's clear) reads from a screen
  above the screen instead. A first read takes only the screen, and is
  read again from a screen above when the document goes on above it, so
  every read the memory holds has its history block.
- Positions are trusted when the rows above the last read's screen read
  exactly as the last read's history block did, at least one with text,
  or when the last read's screen started at the document's start. Then
  the last read's rows are where they were. Otherwise, as once the
  history is full and the buffer moves under fixed places, they are found
  by their text.
- The alignment is a longest common subsequence of the hashes of rows with
  text, by dynamic programming that prefers, among alignments keeping
  equally many rows, the fewest blocks and then the fewest rows moved.
  Found by text, a history-block row is found at its place or higher, or
  lower by the one shift the screen's rows took when they went lower
  (Pi's regular mode reprinting its history longer on Control+O). The
  shift most kept rows share gives the rotation. A row left gone is kept at
  the nearest new row with its text, except in the history block; a row
  whose new text starts with its old is grown.
- Rewritten rows. Between two kept rows, leftover old and new rows are
  paired as rewritten when neither is blank and they share in order at
  least half the characters of the longer (a bit-parallel longest common
  subsequence), keeping order, making as many pairs as it can, then the
  most with the kept row's shift below them. The caret's line now starting
  where the last caret line did is that line rewritten, unless it only
  lost text while the caret moved forward over unchanged text, which is
  another line written there (Windows Terminal's `cls`). Rows above where
  the last read's first row went are `Shown`.
- Lingering. A gone row lingers for 2 seconds when a program had rewritten
  it or it was at or below the caret; a new row with its text is that row
  back (`Returned`).
- Lost reads. A read is lost when it keeps and pairs nothing and is no new
  screen, or when, positions not trusted and the start not refused, it
  keeps no row with text the last read had at or above the caret, or
  keeps them equally well at two places. The memory asks for the whole
  document and aligns that: found at one place, the rows below are new,
  counted as `Lost::About`; found nowhere, `Lost::MoreThan` (or `About`
  after a refused start); at more than one place, only the screen is
  aligned and the count is `Lost::Uncounted`.
- Screens. A read keeping and pairing nothing is a new screen
  (`Switch::New`) when its screen starts at the document's start or the
  aside screen is found in it: its rows are new, the review position goes
  to the caret, and the command marks are set aside with their screen. A
  new screen sets aside the one it replaced as a map from each row's hash
  to its least distance from the document's end; it is found when a row of
  it is no nearer the end than it was, and dropped when a read off the
  document's start keeps something. It only recognises the screen coming
  back and is never spoken.
- A read whose screen shows no text where the last had text changes
  nothing (`blank`).
- Starting again, after a resize or `restart`: each anchor is found by its
  line's hash nearest its old place, at its character position within the
  line, so the review cursor stays on its character however the line
  rewraps; a review position not found goes to the caret, a place not
  found is dropped, a mark not found keeps its place.
- Anchors move with the alignment: a kept or rewritten row carries its
  anchor, the column clamped; a gone row passes the review position to the
  nearest surviving row above, or to the caret; above the read an anchor
  keeps its place relative to the shift, or, above a refused start, is
  found again by its line's hash. A command mark moves with its row and is
  dropped when its row was rewritten or is gone; Core resolves a mark by
  its line's text only when a command looks for it.
  `TerminalReader::caret_line_input` says whether the caret's line holds
  input, for Core to mark it on Enter.

### What the memory keeps

The memory needs of the last read only the screen, the rows written below
it, and the screen above it, its history block; review commands read rows
above that from the provider. Once a read is handled, `trim` drops the
rows above the history block and the reading rules drop their state of
those rows with it; every read first trims what the last left. Nothing
counted from the last read's first row moves, since the memory keeps how
many rows it dropped. A whole document of 9,000 rows therefore leaves the
same memory as an ordinary read, about 60 rows of a 120 by 30 screen, and
the aside screen adds at most 60 hashes. With the reading rules, about
21 KB is held between reads at that size, about 450 bytes per row kept.

### Cost

The memory's and the rules' time together, at 120 by 30 with a full
history, is about 50 microseconds for an ordinary read and under 2.5
milliseconds for the costliest (a lost read's whole document, an overflow
that keeps nothing, a clear and reprint, or the subsequence's worst case);
handling a whole-document read raises the peak by a few megabytes. The
figures are in `docs/performance.md`, "A terminal's rotation" and "A
terminal's reads in Core".

### Tests

`src/terminal_rows/tests.rs` drives the memory with
[verbatim-term-sim](verbatim-term-sim.md), reading as the outpost does,
one case of the design each, asserted whole: output, scrolls and
rotations, footers, redrawn regions, wrapped lines, clears and reprints,
the alternate screen and pagers, floods, lost reads of each count, the
aside screen, the review position, and lingering rows. One more checks the
`unicode-width` property that row splitting relies on.

## The terminal reading rules

`terminal_reading` is the platform-neutral core of the terminal model's
reading rules (`docs/design/terminal-model.md`, "Reading rules" and "The
rules in detail"), written from that design on top of the row memory. It
is a state machine with no clock and no thread: every input carries its
time, and every input answers with what to say, what it withholds and
why, and when the quiet-period timer should next fire. Floods are Core's
to split (that design's "Features and settings", "Core's flood policy"):
every new line goes to Core.

### Public API

- `TerminalReader::new(Settings)`: one reader per terminal, owning its
  `TerminalMemory` (`memory`, and `memory_mut` for the review commands).
  `Settings` holds "report new output" and "review follows caret".
- The inputs, each returning a `Step`:
  - `read(&Read, waited, colours)`: a read, with whether it waited for
    Core's speech queue and, for a read in a key's window, the screen's
    colour runs (`ColourRun`) row by row from the screen's top.
  - `key(Key)`: a key as it reached the program, with when. `KeyKind` is
    `Typed`, a caret key with its `Unit` (character, word or line),
    `Enter`, `Forget` (Tab, Escape, Control+C, Control+D, Control+Break,
    which forget held typing), or `Other`.
  - `focus(at_ms)` and `blur()`.
  - `timer(now_ms)`: the timer the last step asked for came due.
  - `set_settings(Settings)`.
  - `caret_line_input()`, above.
- `Step`: the `Speech` in order, the lines `Withheld` with a `Reason`, the
  `timer` due time, `whole_document` and `history_block` as in the
  `Outcome`, the read's `Outcome`, and `followed`, whether a key moved the
  caret within its window and the review position followed it.
- `Speech`: a new line (`Line`, blank with empty text, which Core counts
  and does not speak; its text a `Text` sharing the read's string, so
  counting it never copies it), what a growing line gained (`Growth`), a
  redrawn row from the word that changed (`Changed`), a held line that
  left the read before it was spoken (`Held`), a lost read (`LostRead`,
  before its lines, with how Core counts those it skips), typing shown
  (`Echo`), a key's answer on the caret's line in a unit (`Caret`), text
  the caret's line gained or lost (`Gained`, `Lost`), a row away from the
  caret that is a key's answer (`Answer`), and a suggestion
  (`Suggestion`). Each names the first row of its line.
- `Reason`, the design's silences: `Volatile`, `UnfinishedWord`,
  `HeldForKeys`, `Spinner`, `LostText`, and `WrittenBack` (text erased and
  written back within the time a gone row lingers, a listed limit).
- The time limits (`docs/design/terminal-model.md`, "Every time limit"):
  `KEY_WINDOW_MS` and `TYPED_LIMIT_MS` (300), `VOLATILE_MS` and
  `UNPROMPTED_MS` (2,000), `QUIET_MS` (1,000), and `quiet_period`, 1
  second or 1.5 times the interval between the last two changes when that
  is longer.

### How the rules are built

Beside the memory's rows the reader keeps each row's colours when last
read and, on a line's first row, the line's history: its text as last
read, what the user was last told of it, whether it is volatile, when it
last changed and the interval before, when it last changed outside a key's
window and last grew, what it waits for, and whether a key changed it
since it appeared. That history moves with the alignment as the memory's
anchors do; a row rewritten alone starts its change history afresh, and a
lingering line taken back brings its history. The state is trimmed with
the memory; a line's text, what was said of it, and its row's text are one
shared string while they are the same, and no history is built for empty
lines above the rows the memory will keep, so a whole-document read does
not build one per row.

Each read, line by line, top to bottom:

- The first read, the first after the focus arrives, a read while the
  focus is away, and a read of a new size speak nothing (a new size speaks
  the caret's line when it changed). A blank read is idle.
- A line with no history whose rows with text are new is new output,
  spoken whole, unless it is the input the shell reprinted. After a lost
  read, `LostRead` comes first.
- A line that grew speaks what it gained since what was said; a last word
  with the caret at its end is held (`UnfinishedWord`) until the line grows
  past it, the caret leaves, or the quiet period passes.
- A line rewritten in place is compared with its last read once (shared
  start and end by whole graphemes, what was removed and inserted, and
  whether it gained text other than at its end or only lost text), and
  every rule reads that record. Within a key's window, away from the
  caret, and unchanged in the 2 seconds before the keys, the keys own it:
  text gained other than at the end is the answer, spoken whole; only lost
  text is silent; anything else waits for the keys to pause
  (`HeldForKeys`). Otherwise only lost text is silent (`LostText`), a
  spinner's symbol replaced alone is silent (`Spinner`), and a row changing
  within 2 seconds of its last spoken change, or at two reads in a row
  when the second waited, is `Volatile` until it has not changed for the
  quiet period. Any other change is spoken from the start of the word
  where the line first differs.
- Kept, grown with nothing to say, and shown lines are silent; one now
  holding less of its old text is `LostText`, and lingering rows taken
  back are `WrittenBack`.
- A held line that left the read is spoken as last read (`Held`), before
  the read's other output.

Keys open a window of 300 milliseconds after the last of them reached the
program. Each read in the window compares the caret's line with the last
read's caret, mapped through the alignment, and is tagged with who owns
its changes (`owner`) and how the caret moved (`Move`: not at all, along
its line, onto another line already there, or onto a new line); the caret
step is the one place the keys' kinds are read. In the window, typing
shown at the caret is echoed and typing not shown within 300 milliseconds
is dropped; a caret moved with no text change says a caret key's unit, or
the landing rule for another key; a caret on another line says that line;
gained and lost text at the caret are said; a new line at the caret is
output. Enter is answered only by output, as NVDA turns off its new-line
announcement in terminals. With "review follows caret" on, a key that
moved the caret, or the focus arriving, moves the review position to it.

Colours are compared in every read in a key's window: among rows whose
text did not change, the one row whose colours changed, or the one row
that differs from both neighbours, is the answer, when exactly one stands
out; text after the caret in other colours than the cell before it is a
suggestion, spoken when it appears and not while typing follows it. A
read in the window without colours holds the text after the caret for the
next read with colours, or speaks it as gained when the window ends
without one (`docs/design/terminal-model.md`, "Lists and colours").

"Report new output" off speaks no output, holds nothing, and takes no
whole-document read, starting the memory again instead
(`TerminalMemory::restart`); keys are still answered.

With the optional `serde` feature, which `verbatim-core` enables, the
reader, its memory and every type they hold serialize, so the flight
recorder dumps and replays Core's state with its terminals whole; a row's
text is written as its text alone and restored as a string of its own,
and the segmenter is made afresh. They are also `Clone`, `PartialEq` and
`Eq`, as Core's state is. Nothing is done between inputs; the only wake-up
the rules ask for is the quiet-period timer, while something waits for it.

### Tests

`src/terminal_reading/tests.rs` drives the rules with `verbatim-term-sim`,
reading as the outpost does at given times with keys as they reach the
program, one rule or one case of the design's "Evidence" each, asserting
the whole step. Further tests check that a read without colours in a key's
window defers the suggestion and emphasis to the next read with colours;
that a reader serialized and restored after each input gives the same
steps; and that, over every stream of the conformance corpus in both
terminals, a read taken again with nothing written between says and
withholds nothing.

`tests/terminal_oracle.rs` checks the rules against the simulator's
oracle, which judges by text, over the conformance corpus in both
terminals, 512 random streams, and every stream of up to four operations
of its tiny alphabet, with nothing pinned: a failure is a consistent
explanation the oracle accepts, a listed limit through its own reason, or
a defect. Each listed limit has its smallest stream as a fixed test.
`tests/terminal_cost.rs` is a measurement, ignored so that no test run
includes it (`docs/performance.md`, "A terminal's rotation").
