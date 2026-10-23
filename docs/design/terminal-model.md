# A model of terminal screens, and how Verbatim reads them

Status: accepted by Dickson, 2026-10-23, after one design review whose findings are folded in; being built. It replaces the rules the terminal redesign accumulated in its first build (`docs/design/terminal-reading.md`, "As built (2026-10-23)", never committed) with a model of how terminals change, derives the reading rules from it, and checks every case and every bug found against it. The parts of `docs/design/terminal-reading.md` that still hold (its requirements, its account of NVDA, and its measurements) move here, and that document is deleted, in the documents step of section 12. When the build is complete this document is rewritten to describe the design as built.

## Decisions

Taken by Dickson on 2026-10-23, and recorded in the sections named:

- Track the screen as rows, and let one alignment of old rows to new rows decide what is new, where each old row went, where the review cursor goes, and what a key did (sections 4 and 5).
- Any row a program rewrites in place is a redrawn row, wherever it is; footer and header are names only (section 5.4).
- Changes to redrawn rows are spoken; a row that keeps changing has its first change heard, so the user knows it exists, and is then silent until it settles, when its text is spoken once (section 5.4).
- One rule for every key: the row away from the cursor that gained text or emphasis is the key's answer, spoken at once; other rows the keys changed are spoken once the keys pause (section 5.7). fzf therefore says the current entry after each typed character and the count when typing pauses.
- A key's effects are those found within 300 milliseconds of the key reaching the program; later changes are output (section 5.5).
- Colours are read with the first read after each key, in the same remote operation as the text, never before a key (section 5.7).
- A typed character not shown within 300 milliseconds of reaching the program is dropped unspoken (section 5.5).
- With "review follows caret" on, the review cursor follows the caret only when a key moved it or the focus arrived; with it off, only the user moves it (section 5.6).
- An input line the shell reprints at the cursor is not spoken (section 5.3).
- A growing line holds back its unfinished last word until the line grows past it, the cursor leaves it, or the quiet period passes (section 5.3).
- "Report status bar" with NVDA's gestures, which in a terminal reads the footer (section 5.11).
- Moving by command with Verbatim+Alt+Up Arrow and Verbatim+Alt+Down Arrow, from Verbatim's own marks; no prompt guessing for commands entered before Verbatim watched the terminal (section 5.13).
- The terminals' events are signals to read, not content (section 2.5).
- Panes drawn side by side by one program are not handled; colours that change with no key and no text change are not heard (section 6).
- Pi: both modes supported, its regular mode recommended in the user guide while fullscreen mode reads less well (section 8).
- Testing: a simulated terminal, checked against the real ones by `cargo xtask regenerate-terminal-conformance-expectations`, and an oracle over the simulator's ground truth (section 9); costs measured before and after (section 10).

## How to read this document

Terms used throughout:

- A row is one line of the terminal's grid, as many cells wide as the screen, with a coordinate in the terminal's buffer.
- A line is what the terminal's text gives between two line breaks: a line too long for one row wraps onto several rows, and the text gives it whole (measured in both terminals).
- Padding is the spaces a terminal adds to fill a row to the width; it is never spoken and never reviewed (section 5.12).
- The screen is the bottom rows of the buffer that the program writes to. The history is the rows above it.
- The cursor is the terminal's cursor, where the program's next character goes. UI Automation reports it as the caret. A program can hide it.
- A read is one snapshot of the text, taken by one `GetText`, with the hints and colours read in the same remote operation (section 3.3).
- A redrawn row is a row a program rewrites in place, as opposed to output written once (section 5.4). The footer is the redrawn rows at the bottom of the screen; the header, those at the top.
- A volatile row is a redrawn row that keeps changing (section 5.4).
- The quiet period is how long a row or the keys must be still before what was held is spoken (section 5.14).
- An anchor is a remembered place in the history: the review position or a command mark (section 4).

Sources: NVDA files are under `nvda/source/` at the pinned commit 732d1ce2f; "measured" means the measurements of 2026-10-23, in `docs/design/terminal-reading.md` and in sections 2.5 and 10 here.

## 1. What a user needs from a terminal

From the decisions already made (`docs/parity/terminals.md`, `docs/design/phase6-design.md`), from screen-reader users' accounts (Sampath, Merrick and Macvean, "Accessibility of Command Line Interfaces", CHI 2021; Pi's issues 1188, 4593, 4687 and 8831; "The text mode lie", xogium.me, 2026-01-05) and from Dickson:

- New output is heard as it arrives, in order, once, and a flood is summarised rather than read for minutes (the flood policy, decided 2026-10-06 and 2026-10-07).
- What the user types is echoed once, never again as output, and a password the terminal does not show is never spoken.
- Editing keys say what they did, whether or not Verbatim knows the key.
- Things that animate (spinners, timers, counters, progress) do not flood speech; their existence and their outcome are heard.
- Decoration does not drown content; what counts as decoration in speech waits for symbol processing (M8).
- The review cursor stays where the user put it while the program redraws other parts of the screen.
- Full-screen programs read like an editor.
- Selection lists and menus say the selected item.
- The user can review the whole history, move by command, and read the status lines on demand.

## 2. Evidence

### 2.1 What the terminals do (measured)

- One `GetText` is atomic in both terminals; separate calls are not atomic with each other, but the calls of one remote operation run together in the terminal's process.
- The document ends one row below the lower of the last written row and the cursor's row.
- Rows keep their coordinates until the history is full; then the whole buffer rotates under fixed coordinates, so a kept range reads newer text. The history's length is the user's setting and is never needed: rotation shows in the document's first row.
- Both terminals pad every row to the width, and give a wrapped line whole, its padding filling its last row.
- The alternate screen is one screen tall, with no history; a range kept on the main screen reads the alternate screen silently when its row is inside the alternate screen's height, and fails beyond it. The console host raises a layout event when the screen switches, and also on some clears; Windows Terminal raises nothing particular.
- The console host's `cls` pushes the screen into the history; Windows Terminal's clear erases the screen and the history, so the document shrinks.
- The console host moves its view down a row for each line written into a scroll region above a footer, and writes the next line over the footer's row until the program draws the footer again.
- Reverse video covers only the cells written in it, not the row's padding, so a selected row's colours read as mixed over the whole row; telling a selected row needs the row's colour runs (section 10).

### 2.2 What NVDA does

- It reads the whole document on every change, diffs it by character, speaks only inserted text, and has no flood limit.
- It hides typed text with "probably typed" rules that also drop real short output, holds typed characters until any text change, and forgets them on Enter, Tab, Control+C, Control+D and Control+Pause (`NVDAObjects/behaviors.py` 553 to 619).
- Its wait for the caret after a caret key is 100 milliseconds, times 3 in Windows Terminal and 1 to 1.5 in the console host (`NVDAObjects/UIA/winConsoleUIA.py` 374 to 376, 470, 486).
- It speaks nothing for keys it has no script for (Control+B, Alt+F, Control+W, Control+U, Control+K).
- Its review cursor is a live text range, never adjusted when the text changes, and it follows every caret event when following is on.
- It has nothing for full-screen programs, footers, selection lists, completion menus or moving by command.

### 2.3 What other screen readers do

- Readers that see the write stream rather than the screen (TDSR, Speakup, Emacspeak's eterm) classify each write by how the cursor moved: one cell gives a character, a jump along the row a word, anything else the line. Fenrir diffs the screen but uses the cursor's movement to tell typing from output.
- Users silence regions by hand: Speakup and Emacspeak have user-defined windows; Terminal Access for NVDA has per-program profiles with fixed rows chosen by window title. Fixed rows break when the layout changes; this model finds redrawn rows from behaviour instead.
- Speakup's highlight tracking is a cursor-tracking mode the user chooses by cycling a key: it follows text written in a highlight colour, so a menu shown only by colour is read as the highlight moves. Section 5.7 does the same automatically, for what a key changed, so it needs no setting.
- Emacspeak's review pointer stays put when output arrives.
- Moving by command: VS Code's terminal moves between commands in its accessible view from its own shell integration; Terminal Access finds prompts by fixed patterns of text (`user@host:path$`, `PS C:\...>`), which miss cmd's prompt and others; Emacs's shell mode moves between prompts.

### 2.4 What programs do

- Shells: readline and PSReadLine edit the input line in place, move the cursor without writing, print completion lists below the prompt and reprint the prompt and input below them, reprint the input below output that arrives while the user types, and show history search in the prompt line. PSReadLine without its screen-reader mode rewrites the input line on every key to recolour it. zsh's and fish's suggestions and PSReadLine's inline prediction write grey text after the cursor.
- Ink programs (Claude Code, Gemini CLI, Copilot CLI): finished output goes above a dynamic region that Ink erases and rewrites on every frame, up to 30 a second.
- Pi (section 8): fullscreen mode on the alternate screen, repainting its transcript to scroll; regular mode on the main screen, rewriting its bottom rows one row lower for each streamed line, and clearing and reprinting everything on a resize or a change above the screen.
- Full-screen programs (vim, less, htop, fzf, tmux) put the cursor anywhere, keep status lines at the top or bottom, and scroll by rewriting rows or by scroll regions.
- Progress: one line rewritten after a carriage return, or several lines rewritten in place above the cursor (docker pull, npm, cargo).

### 2.5 What the terminals' events carry (measured and read)

The best Linux screen readers see the write stream. Windows offers no supported way to see another process's writes; the only route is code injected into the terminal process that patches its internals, which breaks with their updates and which decision D2 keeps off the correctness path. What the terminals do offer was measured with a probe that logged every event while a script wrote known patterns, and checked against Windows Terminal's source (microsoft/terminal at a4bb4756):

- Windows Terminal's notification ("TerminalTextOutput") carries each run of printed characters of a render frame, in order, with no positions. Cursor moves, erases and colour changes add nothing; identical rewrites are sent again in full; a frame's text is cut every 1,000 characters at arbitrary places. Its filter for typed characters fails for shifted symbols, other layouts and redrawn input lines. It is raised only while the control has the focus.
- The console host on this machine reported every write exactly, in order. Its current source batches the events and reports only the span the cursor travelled, so a footer drawn with save and restore reports nothing.

So neither event tells new output from a redraw, and the console host's will lose most of its detail. Each is a reliable signal that something changed, which is all the reading rules use it for. A read can land in the middle of a program's redraw (a footer overwritten and not yet drawn again, a region erased and not yet rewritten); section 4 keeps such rows for a moment so that their return is not new.

## 3. The model of the terminal

### 3.1 State

- A buffer of rows of one width, with coordinates. The main buffer has a history; once full, adding a row discards the oldest and every row's text moves up one coordinate (rotation). The alternate buffer is one screen tall.
- The screen: the bottom rows of the active buffer.
- The cursor: a row and column on the screen, visible or hidden.
- Per cell, colours and other formatting, exposed as text attributes.
- Optional scroll regions: bands of screen rows that scroll among themselves.

### 3.2 What programs do to it

Every change is a sequence of these operations:

- Write text at the cursor; past the last column it wraps onto the next row (one line over two rows).
- A line feed: the cursor down a row, or, on the last row of the screen or of a scroll region, the rows scroll up (the history grows, the buffer rotates, or the region's top row goes).
- A carriage return; cursor moves (relative, absolute, save and restore), which change nothing on the screen.
- Erase part of a row, a row, the rest of the screen, the screen, or the history.
- Insert or delete rows or characters.
- Switch between the main and alternate buffers.
- Resize: the width changes and rows rewrap, or the height changes.
- Change colours without changing text.
- Signals with nothing visible: bell, title, progress, marks.

### 3.3 What Verbatim can observe

- A change event: something changed.
- A read, as one remote operation: the text from a kept start to the document's end (lines whole, padding kept), the screen's text (its height and width), the document's first row, the caret's position and line, where the next read should start, and, after a key, colours (section 5.7).
- Keys: every key, with the time it was pressed and the time it reached the program, and, for held keys, the screen's text and caret before the key.

### 3.4 What cannot be observed

Which operations happened between two reads. The model's job is to choose the explanation a terminal program could actually have produced, and to say plainly where two cannot be told apart.

## 4. Changes as alignments of rows

Every operation of section 3.2 leaves each old row kept (same text, in place or moved), rewritten (same row, other text) or gone, and makes each new row an old row or new. An alignment of old rows to new rows is therefore a complete description of a change, and the alignment that keeps the most rows, moved in the fewest blocks, is the explanation that assumes the program did the least.

### 4.1 The memory

For each terminal, kept while the terminal exists, across focus changes:

- The rows of the last read: each row's text without padding, a 64-bit hash of that text, whether it continues onto the next row, the hash of the whole line it belongs to, its coordinate, its colour runs when last read, and its history (what it last said, when it changed, its kind; section 5.2).
- Lingering rows: rows that a read found gone (erased or written over, not scrolled away) that were redrawn rows or were below the caret's row, kept for 2 seconds with their history. A read that finds a new row with a lingering row's text takes that row back, history and all, so a footer the console host wrote over and drew again a row lower, or a region Ink erased and a read caught before the rewrite, is the same row returning, not new, even when the footer's text never changed. Output above the caret never lingers, so output repeated after a clear is new.
- The known history: the hash of every row read since the terminal was first read, by coordinate, shifted when the history rotates. It is used only to align a reprint after a clear (section 4.4).
- Anchors: the review position and the command marks, each a coordinate and the hash of its line.
- While the alternate screen is up, the main screen's memory, kept aside (section 5.8).

### 4.2 What a read covers

The rows from the kept start, which is the top of the screen as last read, to the document's end: the last screen and whatever was written since. Measured: a 30-row read 0.12 milliseconds; 1,000 rows 1.3 to 3.4; a full history of 9,001 rows 9.4 to 29. The whole document is read only when no row of the last screen is in the read (more than a screen went by, or the history was cleared), to find the old screen and count what went by; since reads wait while Core's speech queue is full (on-demand reading), that is at most once per group of spoken output. With "report new output" off nothing waits, but nothing is spoken or counted either, so the whole document is not read: the memory is simply taken from the read.

### 4.3 The alignment

1. The unit is the row. Lines are split into rows by the width (every line's padded length is a whole number of rows), and rebuilt from rows for speech.
2. Old rows can be in only two places of the new read: near its top (rows that stayed, or moved up when the history rotated) and near its end (rows that moved down with the end of the text, as a program's bottom rows do when output is written above them). So the old screen's rows are aligned against the new read's first rows (the old screen and a screen more) and its last rows (a screen); every row between is new. No operation of section 3.2 moves a row down by more than the text grew. A reprint after a clear is the one case outside this bound, and section 4.4 handles it.
3. The alignment is a longest common subsequence of row hashes, by dynamic programming over those windows: at most about 60 by 180 cells, each a comparison of two integers, so microseconds per read. Among alignments that keep equally many rows, the one whose kept rows move in the fewest blocks wins (a block is a run of kept rows that moved by the same number of rows), and then the one that moves fewest rows. So a repeated row (a prompt, a blank, a rule line, a list entry) is never paired with a copy of itself elsewhere, a program's bottom rows that stay at their coordinates while the history above them rotates are kept, and a transcript repainted one row higher is kept as one block.
4. Between two kept rows, the old and new rows left over are paired as rewrites by likeness: two rows are alike when they share at least half the characters of the longer in order, and among alike pairings the one that moves rows by the same amount as the kept row below them wins. An old row with no partner is gone (and lingers if it was a redrawn row); a new row with no partner is new. So Pi's status row "⠋ Working..." that moved down a row as a streamed line was written above it pairs with "⠙ Working...", and the streamed line is new.
5. The rotation is the number of rows the topmost kept row moved up. Anchors move up by it. When no row of the last screen is in the read, the whole document is read (section 4.2) and the alignment is made against it; if the old screen is not there either, the history overflowed ("skipped more than N lines"), and each anchor is checked against its line's hash where it should be and dropped if its line is gone.
6. The same alignment maps every old row to its new place. Everything that refers to a row (the review position, the caret before a key, what a row last said, its change times, its colours) moves with that mapping.

### 4.4 Clears and reprints

When the document shrank or the kept start no longer reads (Windows Terminal's clear, a program clearing the history), the read starts at the document's start. A read that finds the screen blank where the memory had text keeps the old memory as the reference for the next read as well, so a read that lands between a program's clear and its reprint changes nothing.

Programs that clear and then reprint everything (Pi's regular mode on a resize or a change above the screen) write back rows the user has heard, usually in one frame, so no read sees the clear. A reprint is recognised by its shape, not by the clear: the old screen's rows are found again, and the new rows above them repeat, in order, the rows the known history held directly above the old screen. Those new rows are matched against the known history backwards from the old screen with a histogram diff over hashes (linear in the rows) and kept; new rows that do not match are new. Output that merely repeats earlier output (`ls`, `cls`, `ls`) is not a reprint, since the old screen is not found below it, and is spoken.

## 5. The reading rules

### 5.1 When to read

A change event marks the terminal changed; the worker reads when it is free, so every change is followed by a read that began after it. Reads wait while Core's speech queue is full (on-demand reading). A caret event in Windows Terminal also reads, since Windows Terminal sometimes raises a caret move with no text change. A held key reads before it is sent on (text and caret only) and after the terminal answers. A timer runs only while something waits for the quiet period (section 5.14).

### 5.2 What each row is

After the alignment, each new row is new, kept or rewritten. Each row also carries what it is over time:

- Output: written once and unchanged since. The default.
- Growing: a line whose text only gained at its end since the last read, rows wrapped onto it included. Growth is output still being written, never a redraw.
- Redrawn: a row rewritten in place other than by growth, or a lingering row taken back, at least once.
- Volatile: a redrawn row that changed twice within 2 seconds, other than as a key's answer; or at two reads in a row when the second read had waited for Core's speech queue, so the time between them says nothing about how often the row changes.

None of these depends on where the row is or on the caret. A row's kind is learned from what the program did to it, and is forgotten when the row is gone and no longer lingering.

### 5.3 New output

- New rows are output: their lines (rows joined where they wrap) are spoken in order, by the flood policy.
- A line seen for the first time is spoken whole at once, so a prompt that ends without a space is heard without waiting.
- A growing line speaks what it gained. Once a line has grown, its last word is held while it is unfinished (the cursor at its end with no space after it), and spoken when the line grows past it, when the cursor leaves the line, or after the quiet period. A word split across the first two reads of a line is the one case left (section 6).
- A new blank row is a line once a row with text is written below it (the cursor passed it). Blank rows the cursor rests after, with nothing below, are not lines yet. A line feed makes a line; a cursor move does not.
- The input reprinted: when after a read the caret is on a different row whose line equals the caret's line as last read, with any characters typed since, and that line held input (a key changed it since it appeared) and new rows lie between the two, that row is the shell reprinting the input below a completion list or below output that arrived while the user typed, and it is not spoken; characters typed since are echoed as usual. A fresh prompt after Enter on an empty line is not this, and is spoken.
- Kept rows are silent, wherever they moved.

### 5.4 Redrawn rows

- A redrawn row rewritten with other text speaks what changed, by the changed-word rule and what the row last said (decided 2026-10-07 and 2026-10-08), unless it is volatile. A spinner symbol replaced alone is silent (decided). So the first change of a timer or counter is heard ("Working... 1s"), and the user knows the row exists.
- A volatile row is not spoken while it changes. When it has been unchanged for the quiet period, its text is spoken once, by the changed-word rule against what it last said, and it stops being volatile. "Working... 34s" is heard when the work ends; a clock that never stops is heard once, at its first tick.
- Counting changes in time, and at reads only when a read waited for speech, means a row is judged the same when reads are close together and when they wait behind a long spoken flood, and a bar that steps every few seconds is heard at each step.
- A redrawn row that moved unchanged, or lingered and came back, is silent.
- Changes made as a key's answer are never held (section 5.5); other rows changed while keys are being pressed follow section 5.7.
- The footer and header are names, not rules: the footer is the rows below the caret's row on the main screen, or the redrawn rows at the bottom of the screen, the header the redrawn rows at the top. Pi's command window in the middle of the transcript follows the same rules.

### 5.5 Keys

- A key's window: the reads that begin within 300 milliseconds after the key reached the program can carry its effects. A change found later is output, heard by the rules above. 300 milliseconds is NVDA's wait for the caret in Windows Terminal (section 2.2).
- The caret's row after a key, against the caret's line before it:
  - It gained text the user did not type: that text is spoken (a tab completion, Control+R's match, a paste, a recalled line's change after a line key).
  - It lost text next to the caret: what it lost is spoken, whether the program erased it in one write or over several reads (decided 2026-10-08).
  - The caret moved with no text change: the key's unit for a caret key Verbatim knows (decided 2026-10-08); for any other key, the landing rule: another line, the line; one character, that character; the start or end of the line or a word, the character or word there (decided 2026-10-08). Control+B, Control+F, Alt+B, Alt+F, Control+A and Control+E in bash, PSReadLine and Pi are this rule.
- Typed characters: the rules of `docs/parity/terminals.md` stand unchanged (echoed when the terminal shows them; never spoken again as output; a line changed some other way, such as a password prompt's asterisks, drops the held typing and speaks what the line shows; a clock on a password prompt's line never speaks the password; typing that the terminal cannot show yet is echoed when the caret moves over it; Up Arrow followed at once by a character speaks the recalled line with the character in it once, "echo onex", without the character's own echo). Held typing is forgotten on Enter, Tab, Escape, Control+C, Control+D and Control+Break (decided 2026-10-09), and a character not shown within 300 milliseconds of reaching the program is dropped unspoken. The limit is judged at the next read, with no timer, and counts from when the character reached the program, since key holding can defer a key behind a held one.
- Rows away from the caret that a key changed: section 5.7.

### 5.6 The review cursor

- The review position is an anchor: a row and column of the memory, and the hash of its line. Every read's alignment maps it: a kept row carries it to the row's new place, so it moves with its text when output scrolls, including into the history when the history rotates; a rewritten row keeps it on that row, the column clamped to the row's text; a gone row moves it to the nearest surviving row above, or to the caret when nothing survived.
- A redraw elsewhere (Claude Code's or Pi's footer, a spinner, a timer) never moves it.
- With "review follows caret" on, it follows the caret when a key moved the caret, within the key's window, and not when a program moves the cursor while it draws. With it off, only the user moves it. A focus change moves it to the new focus, as everywhere in Verbatim.
- When the alternate screen opens, it moves to the caret, and the main screen's position is kept with the main memory; when the main screen comes back, that position is restored.
- Above the memory, in the history, it reads the provider's rows directly, its coordinate moved by each rotation.

### 5.7 Rows away from the caret that keys change: lists, menus, colours

One rule for every key, with no knowledge of list markers or highlights:

- A row that gained text or gained emphasis within the key's window is the key's answer and is spoken whole at once. Gained text: compared without spaces, the new text contains the old and adds more than at its end, which is growth (a ">" or "→" before an entry, "[x]" for "[ ]"). A row that only lost text (the old text contains the new) or lost emphasis is silent, so a list moved in two writes is silent until its new marker is drawn (decided 2026-10-09). A row that changed without a key in the 2 seconds before the key (a meter, a progress bar, a streamed line) is not the key's answer, whatever it gained.
- Gained emphasis: a row whose colours changed while its text did not. When two such rows swapped, the one that took the colours the other gave up; when one row changed, that row; when there are no earlier colours to compare with (the first key in a list), the row whose colours differ from both its neighbours'.
- Every other row a key changed (fzf's count, vim's ruler, a status line) is held while keys keep coming, and spoken once, by the changed-word rule, when no key has been pressed for the quiet period.
- When typed characters change rows away from the caret (a list filtered as the user types), entries that survive the filtering are kept rows that moved and are silent; the row that gained the pointer or the highlight is the current entry, spoken after the echo; the count is held until typing pauses. Whether entries the filtering brings into view are told from rows rewritten in place well enough is tried on recorded fzf, Pi and Claude Code output before this is built.
- Colours are read for the whole screen in the first read after every key, in the same remote operation as the text, and compared with the colours kept from the last read that read them. Which rows changed is known only once the read is compared with the memory, so the whole screen is read rather than a choice of rows. They are never read before a key, so a held key reaches the program no later than now. Measured: 2 to 4 milliseconds for a colourful screen of 283 runs; a plain shell screen has a few dozen (section 10).
- On the caret's row, text after the caret in colours other than the typed text's, which the user did not type, is a suggestion (grey ghost text).
- Colours that change with no key and no text change (Pi's tool state shown by background colour) are not read (decided).

### 5.8 Full-screen programs and the alternate screen

- The rules are the same on both screens: a change on the caret's row is the key's answer; rows that scrolled into view are new and spoken (decided: a pager moving one line speaks only that line); a screen replaced whole is new, by the flood policy; status lines, rulers and meters are redrawn rows.
- Switching to the alternate screen shows as the document becoming one screen tall with nothing above it while the main screen had history, and in the console host as a layout event. Windows Terminal's clear looks the same. Both are handled alike, and no rule depends on telling them apart: the current memory is kept aside and a new one begins; when the document grows back and its rows match the kept memory, the kept memory is restored and only what follows is new (decided). After a clear the kept memory never matches, and is replaced at the next switch.
- Command marks set while the alternate screen is up belong to its memory and go with it (section 5.13).

### 5.9 Resizing

Resizing shows as a new width (from the padded length of rows) or height (from the screen's rows), and in the console host as a layout event. The memory is taken again from the read; nothing is spoken but the caret's line if it changed; the review position and the marks are found again by their lines' hashes near their old places. Output written in the same read as a resize is not heard (section 6).

### 5.10 Focus

- Taking the focus subscribes to the terminal's changes, waits for the subscription, and then takes the first read, which speaks nothing but the caret's line; output written after that read began is heard.
- Output written while the terminal was in the background is not heard on return (decided).
- The memory, the lingering rows, the known history and the anchors are kept when the focus leaves and dropped only when the terminal closes. The first read on return aligns with the kept memory, so moving by command still works and nothing written meanwhile is spoken.

### 5.11 Reading on demand

- "Report status bar", with NVDA's gestures (NVDA+End in the desktop layout, NVDA+Shift+End in the laptop layout, where Verbatim+End reviews the line's end), reads the status bar of any application, and in a terminal reads the footer as it is now, top to bottom; with no footer, the last row with text. This is the way to hear a row that never stops changing.
- The review cursor reaches every row, including volatile ones and the header.

### 5.12 Padding

Padding is never spoken and never part of a row's text in the memory: a row's text ends at its last character. Terminals pad only after a line's last character, so a wrapped line's earlier rows have no padding; spaces at the end of one of those rows are spaces the program wrote where the line wrapped, usually one between two words, and are kept. Spaces a program writes inside a line (Pi aligning its model name to the right) are its text too. The review cursor's line ends at its last character (decided 2026-10-06), and the alignment compares rows without padding.

### 5.13 Moving by command

Verbatim+Alt+Up Arrow and Verbatim+Alt+Down Arrow move the review cursor to the previous and next command the user entered, and speak the command's line; its output is the rows below it, read with the review cursor as usual. NVDA uses NVDA+Alt+Up and Down to pan its magnifier, which Verbatim does not have.

- What exists. Windows Terminal records marks for prompts, commands and output (from shell-integration sequences, OSC 133, which no Windows shell writes without setup, and since 1.22 from Enter pressed on the main screen, `autoMarkPrompts`), with actions that scroll to them (`scrollToMark`, which moves neither the cursor nor a selection) or select them (`selectCommand`, bound to no key by default). None of it is exposed through UI Automation, and the console host that ships with Windows ignores the sequences.
- Verbatim's own marks. When Enter is pressed in a terminal, Verbatim notes the caret's line, if any key changed that line since it appeared (so a bare Enter sets no mark and a recalled command does). The mark goes on the row that holds that line once the terminal has answered: the same row in a shell; in Pi's or Claude Code's regular mode, where the input is cleared and the text printed into the transcript, the new row whose line equals it, looked for until the quiet period passes. This is the signal Windows Terminal's own `autoMarkPrompts` uses, so it works in both terminals and every shell with no setup.
- A mark is an anchor: the coordinate of its line's first row and the hash of the whole line, so a command typed over two rows is one mark. Marks move with the history by the rotation the alignment measures; when that cannot be measured, each is checked against its line where it should be and dropped if gone; after a resize each is found again by its line near its old place; a clear that erases the history leaves them with the memory kept aside.
- Commands entered before Verbatim watched the terminal have no mark, and moving past the oldest mark says nothing more (decided). Marks set on the alternate screen (inside vim) belong to its memory and go with it.
- The exit status is not known, so there is no success or failure sound as in VS Code.

### 5.14 Every time limit, in one place

New with this model:

- A key's window: 300 milliseconds after the key reached the program (section 5.5).
- A typed character not shown within 300 milliseconds of reaching the program is dropped unspoken; judged at the next read, with no timer.
- Volatile: two changes within 2 seconds, or at two reads in a row; judged at each read, with no timer.
- Lingering: a gone redrawn row is kept for 2 seconds; judged at each read, with no timer.
- The quiet period: 1 second, or 1.5 times the interval between the row's last two changes if that is longer. It is the one timer the model adds, armed only while something waits for it: a volatile row to settle, a growing line's unfinished word, rows a key changed waiting for the keys to pause, a command mark waiting for its line to be printed. One wake-up each, when it ends; it is re-armed by reads that already happen.

Kept from key holding (`docs/design/key-holding.md`), unchanged: a held caret key waits at most 150 milliseconds for the read before it; a key pressed behind a held key waits at most 100 milliseconds after that key was passed on; a held key's watch ends with its evidence, at the next key or focus change, or after 10 seconds. Verbatim does not poll for the caret, so it has no wait like NVDA's; the key's window bounds what a key is credited with.

## 6. What diffing cannot do

- Identical lines that left a full history cannot be counted; this happens only when the history is full.
- A blank row a line feed wrote and one the cursor jumped over look the same; section 5.3 judges by where the cursor rests.
- Output written and erased between two reads is never seen; accepted, since the final state is what matters.
- A word split across the first two reads of a streamed line is spoken in two pieces.
- Output written in the same read as a resize is not heard.
- Lines that scrolled out of a scroll region between reads are seen only if the terminal kept them in its history.
- Programs that wrap their own text with line breaks (Pi) give rows that are separate lines, so a sentence broken across rows is spoken in pieces.
- Colours that change with no key and no text change are not heard (decided).
- A hidden cursor parked away from the user's attention gives the caret nothing useful to say.
- Panes drawn side by side by one program share rows, so a change is not attributed to its pane (decided: not handled).

## 7. Every case, against the model

For each: what the program does, what the model makes of it, and whether a scenario covers it (section 9 lists those to add).

### 7.1 Shell line editing

- Typing at a prompt: echoed when shown; nothing else. Scenarios: `*_commands`, `*_typing`.
- PSReadLine rewriting the input row on each key to recolour it: the caret's row gained the typed character, echoed; the rest is unchanged text, silent. Scenario: none; add with PSReadLine in its default and screen-reader modes.
- Inline suggestions (PSReadLine, zsh, fish): text after the caret in other colours, not typed (section 5.7). Spoken when it first appears and when it becomes something other than the previous suggestion less the typed character, so typing along it does not repeat it (refines the 2026-10-07 decision that suggestions are spoken). Accepting it (Right Arrow, End) speaks the key's unit. Scenario: none; add.
- Tab completion: the caret's row gained text the user did not type; spoken (section 5.5). Scenario: none; add.
- A completion list printed below the prompt, the prompt and input reprinted below it: the list's rows are new, spoken by the flood policy; the reprinted input at the caret is silent (section 5.3). Scenario: none; add.
- Menu completion (PSReadLine's MenuComplete): the entry that gained emphasis is spoken (section 5.7); the completed text on the caret's row is the key's answer. Scenario: none; add.
- History search (Control+R): the typed text echoed; the match the row gained is spoken. Scenario: none; add.
- History recall (Up Arrow): the caret's line is the key's answer; Up followed at once by a character, "echo onex" (decided). Scenarios: `*_up_typing`, `*_line_key_flood`.
- Keys Verbatim does not know (Control+A, Control+E, Control+B, Control+F, Alt+B, Alt+F): the landing rule. Scenario: none; add one per rule in both terminals.
- Keys that remove text (Backspace, Control+W, Control+U, Control+K, Alt+D, Escape): what the line lost. Scenario: `*_typing` covers Backspace and Escape; add the others.
- Multi-line input (Shift+Enter in Pi and Claude Code): line keys answer with the caret's row. Scenario: none; add.
- Pasting: the caret's row gained the pasted text, spoken once. Scenario: none; add.
- Slow echo over SSH: a character not shown within 300 milliseconds is dropped unspoken; an echo after that is text the caret's row gained, heard as output. Scenario: none; a scripted delay stands in.
- Passwords: never shown, never spoken; asterisks spoken as decided. Scenarios: `*_commands`, `*_spoken_password`, `*_typing`.
- Output while typing: the output rows are new and spoken; the input reprinted below them is silent. Scenario: none; add.

### 7.2 Command output

- Output after a command: new rows, spoken. Scenario: `*_commands`.
- A flood within the history, and past it: the flood policy; past it, "skipped more than N lines". Scenarios: `*_flood`, `*_history_flood`, `*_scrollback_overflow`.
- Identical lines: counted while the history has room. Scenario: `*_same_flood`.
- A long wrapped line: one line over several rows, spoken whole; streamed, it grows, its unfinished word held. Scenarios: `*_long_lines`, `conhost_wrapped_flood`.
- Progress on one line: a redrawn row; its first change spoken; volatile once it changes twice within 2 seconds, and its final value spoken when it settles. A bar that steps every few seconds speaks each step. Scenario: `*_progress`.
- Several progress lines (docker pull, npm, cargo): each a redrawn row by the same rules; each heard when it settles ("Pull complete"). Scenario: none; add.
- A spinner with a message: the symbol alone is silent; a changing message is volatile after its first change. Scenario: none; add.
- Clearing: section 4.4 and 5.8; `cls` says the prompt. Scenario: `*_typing`. Resizing: section 5.9; scenario none; add.
- Output written while in the background: not heard on return. Scenario: `*_leave_flood`.

### 7.3 Programs that redraw their bottom rows (Ink, Claude Code, Pi in regular mode)

- Streaming output above the bottom rows: new rows and growing lines are spoken; the bottom rows are kept as they move down, and a status row that changed as it moved pairs with its old self by likeness (section 4.3). With the history full the bottom rows stay at their coordinates while the transcript rotates, and are kept. Scenario: none; add with a recorded Pi stream.
- The bottom region erased and rewritten every frame: rows that come back with the same text are kept, or taken back while lingering if a read caught the region erased.
- The spinner with "Working..." and a time: its first change heard, then volatile; heard when the work ends.
- A footer of several lines (Pi's directory, tokens, cost and model; Claude Code's status lines): redrawn rows; a cost that changes once a turn is spoken; counts that tick while streaming are heard at their first change and when they settle. The whole footer is read with "report status bar".
- Pi's editor between rule lines, with "↑ 3 more" written into a rule: the rule's change is spoken.
- An autocomplete list (Pi's "→" before the selected entry; Claude Code's slash commands): Up and Down move the marker, and the row that gained it is spoken; typing filters it as in section 5.7.
- The program parking its hidden cursor in the input after each frame: not a key's move, so not followed and not spoken.
- A full clear and reprint (Pi regular mode on a resize or a change above the screen): aligned against the known history (section 4.4); only what changed is new.
- A dialog drawn over the bottom rows (a permission prompt): new and rewritten rows, spoken; a list in it by section 5.7.
- A turn ending: the status row's last change settles and is spoken, or the row goes; the footer's cost change is spoken.

### 7.4 Full-screen programs

- vim: the key's answer is the caret's line, or the landing rule; scrolling speaks the rows that came into view; messages on the command line are spoken; the ruler, if shown, is spoken once the keys pause (Dickson turns it off). Scenario: `*_full_screen` imitates it; add one with vim itself.
- less: a row scrolled in is spoken; a page by the flood policy. Scenario: `*_pager`.
- htop: meters and the process list are redrawn rows, heard at their first change and then volatile; the selection bar, shown by colour, is spoken as it moves (section 5.7). Scenario: none; add a script that refreshes rows on a timer.
- fzf: after each typed character, its echo and then the current entry ("m", "a", "i", "src/main.rs", cut short by the next key), and the count once typing pauses ("12/340"); the arrows say the current entry (section 5.7, decided).
- tmux and other programs that draw panes side by side: not handled (decided). Panes stacked one above the other are rows like any other. Windows Terminal's own panes are separate controls and need nothing.
- Pi in fullscreen mode: the transcript repainted one row higher on each streamed line, kept as one block (section 4.3); the rows that came in are spoken; the bottom rows as in section 7.3; Pi's own scrolling shifts the transcript the other way and the rows that came into view are spoken. Scenario: none; add with a recorded stream.

### 7.5 Lists and prompts drawn by programs

- A marker list whose caret follows the marker; one redrawn whole with the caret below; one redrawn in two writes: section 5.7. Scenarios: `*_marker_list`, `*_redrawn_list`, `*_split_list`.
- A list shown by colour only: section 5.7. Scenario: none; add.
- A checkbox list: the row that gained "[x]" is spoken. Scenario: none; add.
- A yes-or-no question: new output; the answer echoed. Scenario: `*_short_output`.

### 7.6 The terminal itself

- Focus, new windows and tabs: section 5.10. Scenarios: `*_two_windows`, `windows_terminal_tabs`, `windows_terminal_close_tab`.
- Output just after the focus: section 5.10. Scenario: none; to restore.
- The user scrolling the view back while output arrives: the read does not follow the view; the review cursor stays on its text. Scenario: none; add.
- Reviewing the whole history, and moving by command: sections 5.6 and 5.13. Scenarios: `*_review_output`, `*_review_grid`, `*_editing`; add moving by command.
- The bell, the title, progress and marks: not visible through UI Automation; out of scope.
- Wide characters: two cells each; the width and columns are cell counts (decided 2026-10-06); the alignment compares text and is unaffected.

## 8. Pi

Pi (`github.com/earendil-works/pi`, version 1.4.0 read):

- Fullscreen mode, the default: the alternate screen, no terminal history, the transcript scrolled by repainting rows. Earlier turns are reviewed only through Pi's own scrolling.
- Regular mode (`--tui-mode regular`): the main screen with history; its bottom rows rewritten one row lower for each streamed line; a clear and full reprint on a resize or a change above the screen. The history holds the whole conversation, so the review cursor and moving by command reach all of it.
- Its rows: the transcript (header, messages, tool output); then queued messages, a status line with an 80-millisecond spinner ("⠋ Working..."), extension widgets, the editor between rule lines, an autocomplete list ("→" before the selected entry), and a footer of two or three lines.
- Its cursor is hidden; Pi draws its own in reverse video and moves the hidden cursor to it after each frame. Whether Windows Terminal's caret follows a hidden cursor is measured in the recordings step.
- Its editing keys are bash's: Control+B and Control+F, Alt+B and Alt+F, Control+A and Control+E, Control+W, Control+U, Control+K and Alt+D, Control+Y.
- A tool's state is shown by background colour; a running command's output in a five-line window, with an "Elapsed" line updated every second.

The general rules cover it: streaming and growing lines (section 5.3), redrawn, volatile and lingering rows (4.1, 5.4), the status row moving with the bottom rows (4.3), the landing rule for its keys (5.5), the autocomplete list (5.7), reprints (4.4), and, in fullscreen mode, the transcript moved as a block (4.3). What is left: a tool's state is heard only through its text; its self-wrapped paragraphs are spoken a row at a time (section 6); and in fullscreen mode the conversation cannot be reviewed beyond the screen except by Pi's scrolling, which is why regular mode is the one to recommend for now.

## 9. Testing

- A simulated terminal (`verbatim-term-sim`, in tests only): an emulator on a VT parser, with its own grid, so that every row carries a hidden identity through scrolls, rotation and clears and every operation is logged; adapters answer as the console host's and Windows Terminal's providers do (the read program's questions: text from a kept start, the screen, the first row, the caret, colours, refused starts), and a virtual clock drives the time limits.
- The simulated terminal checked against the real ones. `cargo xtask regenerate-terminal-conformance-expectations`, run on demand on this machine or in the VM (it opens real terminal windows, so never in continuous integration), writes each stream of the corpus into the console host and the pinned Windows Terminal, chunk by chunk, waits after each chunk for the terminal's answer to a cursor-position query (proof the chunk is handled), reads each terminal with Verbatim's own read program, and saves what it answered as expectation files in the repository. A unit test in continuous integration replays the same streams in the simulated terminal and compares its answers with those files. The command is run when a stream is added to the corpus, when the simulator's behaviour is changed, and when Windows or Windows Terminal updates; the changed expectation files are reviewed in the commit like any other change of expectation. Events are not part of the expectations: they depend on timing, and the reader treats them only as signals to read.
- The oracle: invariants over the simulator's ground truth, checked whatever the stream did, for random streams with random read points (`proptest`), exhaustive small cases (every sequence of a few operations on a tiny terminal), and the recorded streams:
  - Every line the stream wrote as output (printed once, a line feed after it, never changed afterwards, on the main screen, while the terminal had the focus and output was reported) is spoken exactly once or counted in a flood's summary, and in the order written relative to other such lines; except lines written and erased between two reads, lines that left a full history, and lines written in the same read as a resize.
  - Every spoken line is text the stream printed, or a summary.
  - For every row whose text is unique in both reads and unchanged, the alignment maps it where the row really went.
  - With no key pressed, following off and no switch of screens, the review position stays on the same row (by identity) while that row exists.
  - Padding is never spoken.
  These are what "by construction" means in section 11. What redrawn rows, keys, lists and colours say is decided speech, not a property of the stream, so it is tested by unit tests that state a stream and the speech it must give, never by an oracle that would repeat the rules (`docs/testing.md`).
- Expectations are guarded: an expectation is changed only from an NVDA capture or a decision recorded here, never to make a failing test pass, and a commit that changes one says which.
- Recorded streams of real programs as fixtures: Pi in both modes (`PI_TUI_WRITE_LOG`), PSReadLine in both modes, bash with readline, an Ink program, vim, less, htop, fzf, docker-like progress.
- End-to-end scenarios to add, with NVDA captures where NVDA's speech is the expectation: the landing rule's keys; Control+W, Control+U and Control+K; tab completion; suggestions; a completion list; menu completion; history search; output while typing; pasting; slow echo; a spinner with a message; several progress lines; resizing; a list by colour; a checkbox list; a refreshing full-screen script; fzf-like filtering; the review cursor kept through a footer redraw and moved with its text through a scroll; "report status bar"; moving by command; output just after the focus; Pi in both modes.

## 10. Cost

Measured on 2026-10-23, 120 columns, 30 rows, both terminals:

- A read of the screen's text alone, as one remote operation: 0.19 milliseconds in the console host, 0.27 in Windows Terminal.
- The same with each row's foreground and background colour: 0.96 and 0.50. Too coarse to tell a selected row (section 2.1).
- The same with every colour run of a colourful screen (283 runs): 3.9 and 1.9. Read with separate calls instead of one remote operation, the runs cost 56 and 118 milliseconds, which is why colours are read only in the remote operation.
- Colours are read only in the first read after a key, for the whole screen, after the key has reached the program, so a held key is not delayed: 2 to 4 milliseconds for a colourful screen, less for a plain one, added to the time from each key, typed characters included, to its speech. Read with every read, they would cost 6 to 12 percent of a core while Pi or Claude Code streams at up to 30 frames a second.
- The alignment: at most about 60 by 180 integer comparisons per read, microseconds; the histogram diff after a clear is linear in the rows.
- The whole document is read at most once per group of spoken output, and never with output reporting off (section 4.2).
- The quiet-period timer: one wake-up for each thing that waits for it, when it ends.

To measure before and after the redesign, with the `measure_` scenarios and the outposts' counters (`docs/performance.md`), against the build before the redesign and against NVDA where NVDA can be measured: the outpost's processor time and wake-ups per event and per second, and the time from a change to its speech, for typing, a 20,000-line flood, a Pi stream replayed at its recorded pace, a clock ticking once a second, and a full-screen program refreshing rows. The budget: on the typing path, no rise but the colours of the read after each key, measured; and no rise in wake-ups for output beyond the quiet-period timer.

## 11. The bugs found in the first build, against the model

"By construction" means the model has no state in which the bug can happen, checked by the invariants of section 9.

- A wrapped line's tail spoken as a line of its own: by construction, since a row continuing a line is growth of that line, and a row at the top of a read is kept or gone like any other.
- Windows Terminal's footer flood losing lines 1 to 71, after the old screen was not found and the read judged it "redrawn in place": by construction, since the alignment never decides whether the old screen was found as a block; rows that match are kept, the rest are new.
- A kept range reading newer text after the history rotated: by construction, since the rotation is measured from the kept rows and moves the anchors.
- A prompt paired with the wrong copy of itself, and identical lines miscounted: by construction, by the tie-break of fewest blocks.
- The footer moved and written over by the console host; "status: done" spoken instead of "footer gone" after a clear; "status: ready" repeated in the full-screen scenario; "  elderberry" spoken before "chose banana" in the marker list: by construction, since each of those rows is unchanged, kept wherever it went or taken back while lingering, and silent.
- A progress line read half rewritten: by construction, since a row that only lost text is silent and what it last said is part of its memory.
- A cut whose rows were erased over several reads: by construction, since the loss is measured against the line before the key.
- "back" spoken twice and the prompt not at all, after output reporting was turned back on: not diagnosed. The invariants rule out a reading cause; if it recurs, it lies elsewhere, such as in how reporting is switched back on.
- The blank-row rule: not by construction; the one judgement of section 5.3, with its limit in section 6.
- The width: not a judgement; rows are padded to the width, measured.
- Windows Terminal's caret move with no text change, and output just after the focus: kept from the first build; the caret event reads, and the subscription is in place before the first read.
- Windows Terminal's second focus identity, and the recorder failures: outside reading terminals; recorder failures are fixed at their cause if they recur.

## 12. Order of work

Each part is one step in the sense of `REVIEW.md`: built with its tests and documents, its related scenarios run, then reviewed once in full by a separate read-only subagent. Nothing is pushed until the last part.

1. Design review: done on 2026-10-23, with one re-review; its findings are folded into this document, those that changed behaviour decided by Dickson.
2. The working tree settled: the shared clock (899ca87c) and the console host's say-all fix (83cde1c5) committed; the line-based reading saved as a patch in the scratchpad and removed. The read program, the reads stamped with their start, the subscription in place before the first read and the caret event reading in Windows Terminal could not be separated from it and come back with the steps below.
3. Baseline measurements, on the build before any reading changes: processor time, wake-ups and change-to-speech time for the workloads of section 10.
4. Documents: what still holds in `docs/design/terminal-reading.md` moves here and that document is deleted; `docs/parity/terminals.md` takes the changed decisions; the roadmap's M4 entries point here.
5. The simulated terminal (`verbatim-term-sim`).
6. Conformance: the read program brought back beside the current one in `verbatim-uia-rops`; the stream player; `cargo xtask regenerate-terminal-conformance-expectations`; the expectation files for the hand-written corpus; the unit test that checks the simulator against them; the simulator fixed until it matches both terminals.
7. Recordings: a ConPTY recorder; streams of Pi in both modes, PSReadLine in both modes, an Ink program, and vim, htop, fzf, less and bash in WSL, with docker-like progress from a script; their conformance expectations; whether Windows Terminal's caret follows a hidden cursor.
8. The oracle and random tests, first shown to fail against the current reading.
9. The core, in `verbatim-text`: the memory, the alignment, lingering rows, the known history, anchors, and the rules of section 5 on the virtual clock. The line diff becomes the row alignment.
10. The read program's colours and the remote operations audit.
11. The outpost and Core: reading moved onto the core; the review position as an anchor; following the caret only within a key's window; the key's window and the typed-character limit; the quiet-period timer; the subscription and the caret-event read restored.
12. "Report status bar" with NVDA+End; then moving by command with Verbatim+Alt+Up and Verbatim+Alt+Down, with their scenarios in both terminals, in a shell and in Pi's regular mode.
13. End-to-end scenarios: the ones listed in section 9, with NVDA captures where NVDA's speech is the expectation, after checking that the NVDA pin equals the installed build; the existing terminal scenarios brought back to passing.
14. Measurements after, against the baseline, recorded in `docs/performance.md`.
15. This document rewritten to describe the design as built, for any reader; the decisions move to `docs/parity/terminals.md`.
16. The whole terminal suite, `cargo xtask ci`, and the push.
