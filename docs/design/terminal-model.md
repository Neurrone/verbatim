# A model of terminal screens, and how Verbatim reads them

Status: accepted by Dickson, 2026-10-23, after one design review whose findings are folded in; being built. It replaces the earlier terminal design of 2026-10-10 and its review of 2026-10-11, and the rules that design accumulated in its first build, with a model of how terminals change, derives the reading rules from it, and checks every case and every bug found against it. What still held of the earlier design (its requirements, its account of NVDA, its measurements and the rules Verbatim already follows) is here, in the sections it belongs to; the rest is in git history. When the build is complete this document is rewritten to describe the design as built.

## Decisions

Taken by Dickson on 2026-10-23, and recorded in the sections named:

- Track the screen as rows, and let one alignment of old rows to new rows decide what is new, where each old row went, where the review cursor goes, and what a key did (sections 4 and 5).
- Any row a program rewrites in place is a redrawn row, wherever it is; footer and header are names only (section 5.4).
- Changes to redrawn rows are spoken; a row that keeps changing has its first change heard, so the user knows it exists, and is then silent until it settles, when its text is spoken once (section 5.4).
- One rule for every key: the row away from the cursor that gained text or emphasis is the key's answer, spoken at once; other rows the keys changed are spoken once the keys pause (section 5.7). fzf therefore says the current entry after each typed character and the count when typing pauses.
- A key's effects are those found within 300 milliseconds of the key reaching the program; later changes are output (section 5.5).
- Colours are read with the first read after each key, in the same remote operation as the text, and the key's echo waits for that read; never before a key (section 5.7, decided by Dickson, 2026-10-24).
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

Sources: NVDA files are under `nvda/source/` at the pinned commit 732d1ce2f; "measured" means the measurements of 2026-10-06 to 2026-10-24 in sections 2.1, 2.5 and 10; Microsoft's terminal source is microsoft/terminal at the commit each statement names.

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

The decided requirements these rest on, which every rule of section 5 keeps:

- Output is spoken line by line, queued in order, one utterance per line; newer output never cancels older output still waiting (Dickson, 2026-10-06). A key, anything but Shift, cuts speech off and drops the output still waiting, as NVDA does; output read before the key is never spoken after it, and output read after it is (Dickson, 2026-10-07 and 2026-10-10). Shift pauses and resumes speech, and a paused queue does not drain (NVDA's behaviour).
- The flood policy: the first "Lines spoken in full" lines of a burst (default 30) are spoken whole, never cut short by what arrives meanwhile; once that group has been heard, if more lines wait than that, all but the newest "Last lines to speak" (default 30) become "skipped N lines", and the decision repeats after each group (Dickson, 2026-10-06 and 2026-10-07). Every terminal line counts in N, blank lines included (confirmed 2026-10-09). When lines left a full history unread, Verbatim says "skipped more than N lines", N being the history's rows less the lines about to be spoken, and "skipped lines" when no count is possible (2026-10-07). Both limits may be raised to 10,000, above any terminal's history, which degrades to NVDA's reading of everything (2026-10-07 and 2026-10-09). A flood never hangs Verbatim, and its commands are answered during it (M4's exit criteria).
- Blank lines are not spoken; only what was inserted is spoken, so a line only deleted, or a line that only lost text, says nothing (NVDA's rule; Dickson, 2026-10-08).
- A line is spoken whole however long; waiting output is bounded at 10 MB, and a single line larger than that is cut on a grapheme boundary, and Verbatim says so (Dickson, 2026-10-07).
- Lines made only of symbols are spoken as they are until symbol processing (M8) decides otherwise (Dickson, 2026-10-08). Output is spoken in one language for the terminal unless the terminal reports a language per line (Dickson, 2026-10-07).
- Output of one or two characters ("y", "ok") is spoken, not taken for typing echo as NVDA's "probably typed" rules take it; matching typing against what the line gained makes that guess unnecessary.
- Every UIA notification of Windows Terminal's terminal control and of the console host's text area is ignored, whatever its activity id, so nothing is spoken twice (Dickson, 2026-10-06; NVDA's overlays block them all). Output from anything but the focused terminal is not spoken, as NVDA monitors only the focus.
- "Report new output" (Verbatim+5, NVDA's key for "report dynamic content changes") turns output speech off and on, saying "report new output on" or "off" (Dickson, 2026-10-06). While it is off the terminal is still read, since typing echo depends on those reads (Dickson, 2026-10-07).
- No fixed waits, retries or polling: every wait is for evidence (Dickson, 2026-10-07 and 2026-10-08). UIA call sequences run as remote operations, with the classic fallback behind the same entry point (Dickson, 2026-10-07). The legacy console, which has no UIA text, is not supported (Dickson, 2026-10-08).

## 2. Evidence

### 2.1 What the terminals do (measured)

- One `GetText` is atomic in both terminals; separate calls are not atomic with each other, but the calls of one remote operation run together in the terminal's process. The source agrees (microsoft/terminal at 7d06b26f, `src/types/UiaTextRangeBase.cpp`): `GetText` holds the terminal's lock for the whole copy (the console host's global console lock, which every console write holds; Windows Terminal's read-write lock, which its output processing holds), and every other range method takes the lock separately for its own call. A `GetText` is atomic with respect to the terminal's writes (one console API call, one chunk of Windows Terminal's output), not the program's: a line the program writes in two calls, or ConPTY delivers in two chunks, can end a read half written. The terminal's writer waits for the whole copy, so a large read slows the program's output while it lasts.
- The document ends with the lower of the last row with text and the cursor's row, as each provider finds them when asked: rows an insertion moved down stay in it, and a row erased leaves it (conformance, 2026-10-23).
- Rows keep their coordinates until the history is full; then the whole buffer rotates under fixed coordinates, so a kept range reads newer text. The history's length is the user's setting and is never needed: rotation shows in the document's first row.
- Both terminals pad every row to the width, and give a wrapped line whole, its padding filling its last row.
- The console host's ranges end before a row's line break, so an expanded line, the screen and the document end without one; Windows Terminal's end after it, except on the buffer's last row, where the range ends one cell short of the width: the alternate screen's last row, and the main screen's once the history is full. A width taken from the narrowest line is therefore wrong in Windows Terminal unless the screen's last line is left out, as it is (conformance, 2026-10-23).
- The console host keeps 9,001 rows in all, Windows Terminal 9,001 rows of history above its screen, by default.
- The alternate screen is one screen tall, with no history; a range kept on the main screen reads the alternate screen silently when its row is inside the alternate screen's height, and fails beyond it. The console host raises a layout event when the screen switches, and also on some clears; Windows Terminal raises nothing particular.
- The console host's `cls` pushes the screen into the history; Windows Terminal's clear erases the screen and the history, so the document shrinks.
- A scroll region from the screen's top, with a footer below it, scrolls into the history in both terminals: the footer keeps its row on the screen and the new blank row comes in at the region's bottom (conformance, 2026-10-23). The console host was seen writing a line over the footer's row in the footer scenarios, whose scripts draw differently; a read can also land in the middle of a program's own redraw, which lingering rows absorb (section 4.1).
- Reverse video covers only the cells written in it, not the row's padding, so a selected row's colours read as mixed over the whole row; telling a selected row needs the row's colour runs (section 10). A single cell answers its own colour; a stretch answers its first cell's colour when every cell but its last has it, and UIA's "mixed" otherwise, since neither provider compares the last cell; the format unit stops only at rows; Windows Terminal's `FindAttribute` is not implemented (measured 2026-10-24). So a stretch is known to be one colour from its answer and its last cell read alone, and the read program checks each row whole, a row of several colours in 16-cell chunks, and a chunk of several colours cell by cell. On the buffer's last row (the alternate screen's, or the main screen's once the history is full) moving by characters stops a cell short, so that row's last cell is built the other way and the row is read cell by cell.
- Windows Terminal serves UIA calls on its UI thread. Where it renders in software (a machine with no 3D graphics, such as this development VM), a call that arrives while that thread draws the window's own controls (its title or tab, its scrollbar) during a heavy frame waits for the frame, 10 to 36 milliseconds measured, more under load; a read made on its text-changed event, which arrives once the frame is done, never waited (worst 2.6 milliseconds over a 30-frame stream). So reads follow events (section 5.1), and a read not prompted by an event, such as one before a key is passed on, can cost a key one software frame there. Windows Terminal raises no text-changed event for writes made only of blank lines (line feeds alone, or a reset scroll region and a line feed), so such a scroll is seen only through another event or the next read.
- A `FindText` that finds nothing, measured on 2026-10-24 in the pinned Windows Terminal 1.24.12741, returns a non-null range whose `GetText`, `CompareEndpoints` and `MoveEndpointByRange` fail with `E_FAIL` or `E_INVALIDARG` and whose other calls succeed, with no access violation; the console host behaves alike. The model finds rows by their text in its own reads and does not use `FindText`.

### 2.2 What NVDA does

- It reads the whole document on every change, diffs it by character, speaks only inserted text, and has no flood limit.
- It hides typed text with "probably typed" rules that also drop real short output, holds typed characters until any text change, and forgets them on Enter, Tab, Control+C, Control+D and Control+Pause (`NVDAObjects/behaviors.py` 553 to 619).
- Its wait for the caret after a caret key is 100 milliseconds, times 3 in Windows Terminal and 1 to 1.5 in the console host (`NVDAObjects/UIA/winConsoleUIA.py` 374 to 376, 470, 486).
- It speaks nothing for keys it has no script for (Control+B, Alt+F, Control+W, Control+U, Control+K).
- Its review cursor is a live text range, never adjusted when the text changes, and it follows every caret event when following is on.
- It has nothing for full-screen programs, footers, selection lists, completion menus or moving by command.

The rest of NVDA's terminal reading is in `docs/nvda/editable-text-and-terminals.md`, "Terminals" and "Terminal details at the pinned source". What this design also relies on:

- Monitoring starts on focus: the monitor thread takes a baseline read and then reads again on each text change (`NVDAObjects/behaviors.py` 468 to 488), so output written just after the focus arrives is spoken (captured: "terminal blank" and then PowerShell's start-up notice).
- Events are coalesced before NVDA's monitor sees them: UIA's own event coalescing is on (`UIAHandler/__init__.py` 539), NVDA's C++ rate limiter merges events by element and kind (`nvdaHelper/local/UIAEventLimiter/rateLimitedEventHandler.cpp` 33 to 60), and the monitor's text-changed flag collapses any number of events between two reads into one more read (`behaviors.py` 420 to 424).
- For Windows Terminal and the console host at the FORMATTED API level the text read is the whole document, history included, in one `GetText` (`NVDAObjects/UIA/__init__.py` 511 to 512, `diffHandler.py` 33 to 34). Measured on 2026-10-06: after 5,000 lines in a 120-column Windows Terminal the document was 610,244 characters, 86 percent of them padding, and took 6 milliseconds to read before any diff; NVDA pays that on every change.
- When it reports new lines it also forgets the typed characters still queued (`behaviors.py` 567 to 569), so characters typed between a text change and its report are never echoed.
- Its cancel goes on the same queue as its line reports, so lines queued before a key are cut off, but lines from a read that ends after the key are still spoken (`inputCore.py` 595 to 605 against `behaviors.py` 497 to 502).
- After a caret key it polls the caret (three spins, then 10-millisecond sleeps) up to its wait, holding its main thread, and then speaks the unit at the caret whether or not the caret moved (`editableText.py` 70 to 192), so a pager key says ":" at the end of the wait. Backspace is the exception: it speaks the character or word before the caret only if the caret moved within the wait (`editableText.py` 313 to 315); Delete speaks the unit at the caret (329 to 342). A key with a script is swallowed and sent again by the script, and keys without a script queue behind it to keep their order (`inputCore.py` 641 to 645).
- Its known weaknesses, captured with alpha-57645 on 2026-10-09 and replayed with alpha-58022, the pinned build, on 2026-10-12: fragments from the character diff ("eady greater dot altscreen dot ps 1" on closing the alternate screen); typed commands and recalled lines spoken twice, once as echo and again as output; queued characters of a password spoken when anything else on the line changes; no flood control, so a console flood was still being spoken after its window closed. With a full history and every line the same, the document is identical before and after a flood and its diff finds nothing.

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

The events Verbatim reads on are UIA's text-changed event in Windows Terminal and, in the console host, its console WinEvents:

- The console host's four console WinEvents are its text-change evidence: UIA's text changes stopped reaching Verbatim in the middle of 12,000-line writes in 2 of 14 runs, cause unknown, and since 2026-10-13 the WinEvents are the only evidence taken (`docs/design/wake-ups.md`, item 15). Measured on 2026-10-13 on Windows 11 24H2's console host: a region update (`EVENT_CONSOLE_UPDATE_REGION`) comes for every write of a row's cells (each line of output, a prompt, a line the shell redraws, the alternate screen opened and drawn, a VT clear or scroll); a simple update (`EVENT_CONSOLE_UPDATE_SIMPLE`) for one cell written, which is every typed character's echo, and for a whole-buffer clear by the console API (`cls`, `[Console]::Clear()`); a scroll update (`EVENT_CONSOLE_UPDATE_SCROLL`) for text scrolled up by output once the cursor is on the last row, and for a block moved by `ScrollConsoleScreenBuffer`, alone; a layout event (`EVENT_CONSOLE_LAYOUT`) for the view moving, a window resized, the alternate screen opened, and, alone, the alternate screen closed. Each kind is the only evidence of some change, so all four are taken. In a flood each line raises a region, a scroll and a layout; the outpost's intake merges a window's updates while they wait, and drops a held console's updates without waking its worker while its text has changed unread and no caret key's watch is open, as NVDA's console handler skips an update while one is pending. Microsoft warns that registering for console WinEvents costs every console program on the system, and the console host checks whether any hook is installed before raising them.
- Read in the console host's source, not measured on the installed build: it raises its accessibility events for a write only when the cursor's position differs before and after it (`src/host/_stream.cpp` 33 to 79, at microsoft/terminal 7d06b26f; rewritten and debounced in #19344, 2025-09-22). With a full history the buffer rotates under a cursor that stays on the bottom row, so a write of a whole line and its line break may raise no event at all. NVDA has the same exposure. Whether build 26100's console host does this is open.

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

Times compared across processes (a key's press in the hook, a read's start in the outpost) are all taken on one clock, the performance counter with an offset shared by every process of a run, in microseconds (`crates/verbatim-process/src/clock.rs`). Core takes the hook's reports and the outpost's messages in the order they arrive, on one channel; the hook reports a key before passing it on, so the report is queued before the program can receive the key. A message's arrival time is never used to judge which key a read answers, since a read taken before a key can arrive after it: the read's own start time, carried in the message, decides (section 5.5).

### 3.4 What cannot be observed

Which operations happened between two reads. The model's job is to choose the explanation a terminal program could actually have produced, and to say plainly where two cannot be told apart.

## 4. Changes as alignments of rows

Every operation of section 3.2 leaves each old row kept (same text, in place or moved), rewritten (same row, other text) or gone, and makes each new row an old row or new. An alignment of old rows to new rows is therefore a complete description of a change, and the alignment that keeps the most rows, moved in the fewest blocks, is the explanation that assumes the program did the least.

### 4.1 The memory

For each terminal, kept while the terminal exists, across focus changes:

- The rows of the last read: each row's text without padding, a 64-bit hash of that text, whether it continues onto the next row, the hash of the whole line it belongs to, its coordinate, its colour runs when last read, and its history (what it last said, when it changed, its kind; section 5.2).
- Lingering rows: rows that a read found gone (erased or written over, not scrolled away) that were redrawn rows or were at or below the caret's row (where it was before the read or is after it), kept for 2 seconds with their history. A read that finds a new row with a lingering row's text takes that row back, history and all, so a footer the console host wrote over and drew again a row lower, or a region Ink erased and a read caught before the rewrite, is the same row returning, not new, even when the footer's text never changed. Output above the caret never lingers, so output repeated after a clear is new. A read that shows no text at all keeps every gone row lingering (section 4.4).
- The known history: the hash of every row read since the terminal was first read that the buffer still holds above the memory, by coordinate; a rotation drops the oldest, so it is bounded by the terminal's history (about 9,000 hashes, 72 kilobytes). When the first read did not start at the document's start, the number of rows above the known history is unknown until a read from the start tells it, and a rotation drops the oldest hashes first. Rows that went by unread are held as unread, their count known and their text not. It measures the rotation (section 4.3, item 5) and aligns a reprint after a clear (section 4.4).
- Anchors: the review position and the command marks, each a coordinate and the hash of its line.
- While the alternate screen is up, the main screen's memory, kept aside (section 5.8).

### 4.2 What a read covers

The rows from the kept start, which is the top of the screen as last read, to the document's end. The top of the screen is found from the document's end, moved back by the screen's height, never from the view, so a user who scrolls the view back into the history neither moves the kept start nor makes every read as long as the distance scrolled. A read so covers the last screen and whatever was written since. Measured: a 30-row read 0.12 milliseconds; 1,000 rows 1.3 to 3.4; a full history of 9,001 rows 9.4 to 29. The first read is the screen, which need not start at the document's start, so the next read's kept start may lie above it. The whole document is read only when no row with text of the last screen is in the read (more than a screen went by, or the history was cleared) or the kept start is refused (section 4.4; a refused read that, started a block higher, reaches the document's start is already the whole document), to find the old screen and count what went by; since reads wait while Core's speech queue is full (on-demand reading), that is at most once per group of spoken output. With "report new output" off nothing waits, but nothing is spoken or counted either, so the whole document is not read: the memory is simply taken from the read.

### 4.3 The alignment

1. The unit is the row. Lines are split into rows by the width, and rebuilt from rows for speech. Every screen line's padded length is a whole number of rows, but for Windows Terminal's cut last row (section 2.1). The width is the length of the caret's row, which the read gives as one row (one cell more when it is the row Windows Terminal cuts), when that fits every screen line; when two lengths fit, the one that gives the screen the height it had at the last read. Otherwise it is the narrowest of the screen's lines but its last, which is wrong only when every one of those lines wraps.
2. Old rows can be in only two places of the new read: near its top (rows that stayed, or moved up when the history rotated) and near its end (rows that moved down with the end of the text, as a program's bottom rows do when output is written above them). So the old screen's rows are aligned against the new read's first rows (the old screen and a screen more) and its last rows (a screen); every row between is new. No operation of section 3.2 moves a row down by more than the text grew. A reprint after a clear is the one case outside this bound, and section 4.4 handles it.
3. The alignment is a longest common subsequence of row hashes, by dynamic programming over those windows: at most about 60 by 180 cells, each a comparison of two integers, so microseconds per read. Among alignments that keep equally many rows, the one whose kept rows move in the fewest blocks wins (a block is a run of kept rows that moved by the same number of rows), and then the one that moves fewest rows. So a repeated row (a prompt, a blank, a rule line, a list entry) is never paired with a copy of itself elsewhere, a program's bottom rows that stay at their coordinates while the history above them rotates are kept, and a transcript repainted one row higher is kept as one block. A subsequence cannot keep a row that moved past others, as an entry of Claude Code's command list does, so a row whose text is once in the old rows and once in the read is also kept where that text went.
4. Between two kept rows, the old and new rows left over are paired as rewrites by likeness: two rows are alike when they share at least half the characters of the longer in order, and among alike pairings the one that moves rows by the same amount as the kept row below them wins. An old row with no partner is gone (and lingers if it was a redrawn row); a new row with no partner is new. So Pi's status row "⠋ Working..." that moved down a row as a streamed line was written above it pairs with "⠙ Working...", and the streamed line is new.
5. The rotation is the number of rows the history moved up. With a full history no coordinate the providers report changes as the buffer rotates (measured in both terminals), so it is found from text, decided exactly as comparing the whole buffer with the known history would decide it, without reading the whole buffer (the effect decided by Dickson, 2026-10-24). Rows above the screen change only by rotation or a clear, and no program writes into the history, so a rotation of r rows predicts every row of the new history: the row now at a coordinate is the one the known history held r rows lower. Every read takes the document's first row and, by starting a few rows higher than the kept start, a block of history rows (four, extended upward past blank rows to take in two with text, at most a screen). The candidate rotations are those for which the known history's row r has the new first row's hash, usually exactly one. Each candidate predicts where the block now lies: a prediction inside the read is checked against it at no cost, and the rest are checked together in one remote operation that reads just those rows by coordinate. A candidate that holds at both ends, the first row and the block, is the rotation when it is the only one; the old screen lies below the block, and the rows between the old screen's new place and the read's start are the output that went by unread. When several candidates hold, the whole document is read and compared with the known history, which is the whole-buffer comparison itself; when the history is all blank or one repeated text, even that cannot tell the rotation, and the smallest one consistent with the text is taken. When no candidate holds and the known history covers the whole buffer, the history overflowed or was cleared (below). When the known history does not reach the document's start, because the first read was only the screen, or a candidate's row is one that went by unread, the whole document is read once, which completes the known history. Costs: usually no read beyond the block's few rows; a small read of a few rows when the block moved above the read; the whole document, 9 to 29 milliseconds, only when the text is ambiguous or the known history incomplete. Anchors move up by it. When no row of the last screen is in the read, the whole document is read (section 4.2) and the alignment is made against it; if neither the block nor any row of the old screen with unique text is there, the history overflowed: every row still held (a volatile row's latest value, a growing line's unfinished word) is spoken as it was last read, then "skipped more than N lines", so nothing a read saw is lost when a flood pushes it out of the history before the next read (decided by Dickson, 2026-10-24); and each anchor is checked against its line's hash where it should be and dropped if its line is gone, except that the review position stays on its row when that row was kept.
6. The same alignment maps every old row to its new place. Everything that refers to a row (the review position, the caret before a key, what a row last said, its change times, its colours) moves with that mapping. A row's change history (its change times and kind, which make it volatile) moves only with a row kept in place or moved with its neighbours as part of a block; a row paired alone by likeness to a row elsewhere keeps the text it last said but starts its change history afresh, so a status row's volatility never passes to a new row that only resembles it (decided by Dickson, 2026-10-24).

### 4.4 Clears and reprints

When the document shrank or the kept start no longer reads (Windows Terminal's clear, a program clearing the history), the read program starts its read a screen above the document's end, and the whole document is read next (section 4.2). A read that finds the screen blank where the memory had text keeps the old memory as the reference for the next read as well, so a read that lands between a program's clear and its reprint changes nothing.

Programs that clear and then reprint everything (Pi's regular mode on a resize or a change above the screen) write back rows the user has heard, usually in one frame, so no read sees the clear. A reprint is recognised by its shape, not by the clear: the old screen's rows are found again, and the new rows above them repeat, in order, the rows the known history held directly above the old screen. Those new rows are matched against the known history backwards from the old screen with a histogram diff over hashes (each match extended above and below while rows agree, the rarest row nearest the middle anchoring each part, so close to the rows times their logarithm: a 9,000-row reprint takes about 6 milliseconds) and kept; new rows that do not match are new. Output that merely repeats earlier output (`ls`, `cls`, `ls`) is not a reprint, since the old screen is not found below it, and is spoken. A document can also shrink because rows of the screen were erased (Windows Terminal erasing below the cursor) rather than cleared; when the block above the screen is still in place counted from the document's start, the history is kept where it was. Output repeated after a clear that leaves the document exactly as erasing its last rows would is therefore silent: no reader can tell the two apart (section 3.4). A reprint that leaves the document as long as it was refuses no start, so its rows above the screen are not read; they are not shown, and nothing is said of them.

## 5. The reading rules

### 5.1 When to read

A change event marks the terminal changed; the worker reads when it is free, so every change is followed by a read that began after it. Reads wait while Core's speech queue is full (on-demand reading). A caret event in Windows Terminal also reads, since Windows Terminal sometimes raises a caret move with no text change. A held key reads before it is sent on (text and caret only) and after the terminal answers. A timer runs only while something waits for the quiet period (section 5.14).

- Change events coalesce: a terminal has a flag for "changed since the last read began", which a change event sets; when the worker is free and the flag is set it reads, and a change observed after a read began sets it again, so exactly one more read follows. There is no delay and no minimum interval between reads (Dickson, 2026-10-07). When the terminal raises no further event, nothing is read until it does.
- On-demand reading (Dickson, 2026-10-07): while Core's speech queue is full the outpost only notes changes until Core asks, when a group's last line is handed to speech. A cut (a key that cancels speech) while Core's read is unanswered is answered after that read, whose answer is output read before the cut and is used only to echo typing, as the cut's own answer is (fixed 2026-10-14, after a key typed just as reading went live lost its echo). A cut while reading is live, or held with nothing changed, is answered empty without reading (fixed 2026-10-17), since a read then finds only what the key did, and that belongs to the text change that follows; only a cut with changes held, or with a read owed, reads.
- The caret while output is held (Dickson, 2026-10-14): while output is held and the text has changed, a caret event is noted, the outpost keeps one noted caret, and reads it only when nothing else waits, skipping it if a read since has covered it. Core's caret is so never staler than the outpost is busy, and a console flood's separate caret reads fell from about 3,000 to about 20.

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
- A row's change that is still waiting in Core's queue when the row changes again is replaced by the newer change, so a waiting progress line is spoken once, as it last was.
- Changes made as a key's answer are never held (section 5.5); other rows changed while keys are being pressed follow section 5.7.
- The footer and header are names, not rules: the footer is the rows below the caret's row on the main screen, or the redrawn rows at the bottom of the screen, the header the redrawn rows at the top. Pi's command window in the middle of the transcript follows the same rules.

### 5.5 Keys

- A key's window: the reads that begin within 300 milliseconds after the key reached the program can carry its effects. A change found later is output, heard by the rules above. 300 milliseconds is NVDA's wait for the caret in Windows Terminal (section 2.2).
- The caret's row after a key, against the caret's line before it:
  - It gained text the user did not type: that text is spoken (a tab completion, Control+R's match, a paste, a recalled line's change after a line key).
  - It lost text next to the caret: what it lost is spoken, whether the program erased it in one write or over several reads (decided 2026-10-08).
  - The caret moved with no text change: the key's unit for a caret key Verbatim knows (decided 2026-10-08); for any other key, the landing rule: another line, the line; one character, that character; the start or end of the line or a word, the character or word there (decided 2026-10-08). Control+B, Control+F, Alt+B, Alt+F, Control+A and Control+E in bash, PSReadLine and Pi are this rule.
- Typed characters: the rules of `docs/parity/terminals.md` stand unchanged (echoed when the terminal shows them; never spoken again as output; a line changed some other way, such as a password prompt's asterisks, drops the held typing and speaks what the line shows; a clock on a password prompt's line never speaks the password; typing that the terminal cannot show yet is echoed when the caret moves over it; Up Arrow followed at once by a character speaks the recalled line with the character in it once, "echo onex", without the character's own echo). Held typing is forgotten on Enter, Tab, Escape, Control+C, Control+D and Control+Break (decided 2026-10-09), and a character not shown within 300 milliseconds of reaching the program is dropped unspoken. The limit is judged at the next read, with no timer, and counts from when the character reached the program, since key holding can defer a key behind a held one.
- Delete speaks what it leaves at the caret (Dickson, 2026-10-09).
- A caret move the program makes with no key is not spoken (Dickson, 2026-10-08), as NVDA does not use a terminal's caret events.
- The console host's keys are translated with the console host's own keyboard layout, as NVDA does.
- Rows away from the caret that a key changed: section 5.7.

### 5.6 The review cursor

- The review position is an anchor: a row and column of the memory, and the hash of its line. Every read's alignment maps it: a kept row carries it to the row's new place, so it moves with its text when output scrolls, including into the history when the history rotates; a rewritten row keeps it on that row, the column clamped to the row's text, and so does a blank row written over in place; a gone row moves it to the nearest surviving row above, or to the caret when nothing survived.
- A redraw elsewhere (Claude Code's or Pi's footer, a spinner, a timer) never moves it.
- With "review follows caret" on, it follows the caret when a key moved the caret, within the key's window, and not when a program moves the cursor while it draws. With it off, only the user moves it. A focus change moves it to the new focus, as everywhere in Verbatim.
- When the alternate screen opens, it moves to the caret, and the main screen's position is kept with the main memory; when the main screen comes back, that position is restored.
- Review commands never act on a live provider range, which could point at other text from the moment output rotates the history until the next read. A review command first takes a read (the same atomic snapshot as for output, so its alignment maps the anchor onto the text as it is now), then moves and speaks within that snapshot. The anchor is therefore always on its text when the command acts; between reads nothing acts on it.
- Above the memory, in the history, rows are read from the provider by coordinate, in the same remote operation as that read, each checked against the hash of the line the anchor expects there; a row that does not match is found again by its hash near the coordinate the measured rotation gives, and the anchor moves to it.

### 5.7 Rows away from the caret that keys change: lists, menus, colours

One rule for every key, with no knowledge of list markers or highlights:

- A row that gained text or gained emphasis within the key's window is the key's answer and is spoken whole at once. Gained text: compared without spaces, the new text contains the old and adds more than at its end, which is growth (a ">" or "→" before an entry, "[x]" for "[ ]"). A row that only lost text (the old text contains the new) or lost emphasis is silent, so a list moved in two writes is silent until its new marker is drawn (decided 2026-10-09). A row that changed without a key in the 2 seconds before the key (a meter, a progress bar, a streamed line) is not the key's answer, whatever it gained.
- Gained emphasis: a row whose colours changed while its text did not. When two such rows swapped, the one that took the colours the other gave up; when one row changed, that row; when there are no earlier colours to compare with (the first key in a list), the row whose colours differ from both its neighbours'.
- Every other row a key changed (fzf's count, vim's ruler, a status line) is held while keys keep coming, and spoken once, by the changed-word rule, when no key has been pressed for the quiet period.
- When typed characters change rows away from the caret (a list filtered as the user types), entries that survive the filtering are kept rows that moved and are silent; the row that gained the pointer or the highlight is the current entry, spoken after the echo; the count is held until typing pauses. Whether entries the filtering brings into view are told from rows rewritten in place well enough is tried on recorded fzf, Pi and Claude Code output before this is built.
- Colours are read for the whole screen in the first read after every key, in the same remote operation as the text, and compared with the colours kept from the last read that read them. Which rows changed is known only once the read is compared with the memory, so the whole screen is read rather than a choice of rows. They are never read before a key, so a held key reaches the program no later than now. Measured in a debug build: 1 to 3 milliseconds for a plain shell screen, Claude Code or a PSReadLine line, 5 to 8 for Pi's screen (two programs), and 31 to 72 for the worst case, every cell a different colour (six programs, none over 4,200 instructions) (section 10).
- On the caret's row, text after the caret in colours other than the typed text's, which the user did not type, is a suggestion (grey ghost text).
- Colours that change with no key and no text change (Pi's tool state shown by background colour) are not read (decided).

### 5.8 Full-screen programs and the alternate screen

- The rules are the same on both screens: a change on the caret's row is the key's answer; rows that scrolled into view are new and spoken (decided: a pager moving one line speaks only that line); a screen replaced whole is new, by the flood policy; status lines, rulers and meters are redrawn rows.
- Switching to the alternate screen shows as the document becoming one screen tall with nothing above it while the main screen had history, and in the console host as a layout event. Windows Terminal's clear looks the same. Both are handled alike, and no rule depends on telling them apart: the current memory is kept aside and a new one begins; when the document grows back and its rows match the kept memory, the kept memory is restored and only what follows is new (decided). After a clear the kept memory never matches, and is replaced at the next switch. A program erasing the history looks like a switch too, so the kept memory is not restored when the read also shows rows read since the switch, still where they were (a full-screen program's last frame printed on the main screen as it leaves is not such a row) (decided by Dickson, 2026-10-23): those rows prove the screen never changed, and restoring would speak them again. When the main screen had no history, no read shows a switch at all; the oracle therefore counts text shown on either screen as shown (section 9), so a full-screen program's last frame reprinted on the main screen as it leaves is not new (decided by Dickson, 2026-10-23).
- Command marks set while the alternate screen is up belong to its memory and go with it (section 5.13).

### 5.9 Resizing

Resizing shows as a new width (from the padded length of rows) or height (from the screen's rows), and in the console host as a layout event. The memory is taken again from the read; nothing is spoken but the caret's line if it changed; the review position and the marks are found again by their lines' hashes near their old places. Output written in the same read as a resize is not heard (section 6).

### 5.10 Focus

- Taking the focus subscribes to the terminal's changes, waits for the subscription, and then takes the first read, which speaks nothing but the caret's line; output written after that read began is heard. The focus says "terminal" (the console host's English-only name "Text Area" dropped, as NVDA drops it) and the caret's line, "blank" for an empty one, in a new window or tab of either terminal (Dickson, 2026-10-09).
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

- Identical lines that left a full history cannot be counted; this happens only when the history is full, and then whenever more than a screen of them scrolls between two reads, not only when the history overflows. No reader can count them from text, NVDA included. The console host's scroll event cannot count them either: its distance sums buffer and view scrolls over a debounce interval, a VT line feed raises no rotation count at all, and Windows Terminal raises no console events (microsoft/terminal at 7d06b26f, `src/host/AccessibilityNotifier.cpp` 246 to 289, `src/host/outputStream.cpp` 445 to 447).
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
- Its cursor is hidden; Pi draws its own in reverse video and moves the hidden cursor to it after each frame. Both terminals report the caret where the hidden cursor is (conformance, 2026-10-23), so the caret is in Pi's editor.
- Its editing keys are bash's: Control+B and Control+F, Alt+B and Alt+F, Control+A and Control+E, Control+W, Control+U, Control+K and Alt+D, Control+Y.
- A tool's state is shown by background colour; a running command's output in a five-line window, with an "Elapsed" line updated every second.

The general rules cover it: streaming and growing lines (section 5.3), redrawn, volatile and lingering rows (4.1, 5.4), the status row moving with the bottom rows (4.3), the landing rule for its keys (5.5), the autocomplete list (5.7), reprints (4.4), and, in fullscreen mode, the transcript moved as a block (4.3). What is left: a tool's state is heard only through its text; its self-wrapped paragraphs are spoken a row at a time (section 6); and in fullscreen mode the conversation cannot be reviewed beyond the screen except by Pi's scrolling, which is why regular mode is the one to recommend for now.

## 9. Testing

- A simulated terminal (`verbatim-term-sim`, in tests only): an emulator on a VT parser, with its own grid, so that every row carries a hidden identity through scrolls, rotation and clears and every operation is logged; adapters answer as the console host's and Windows Terminal's providers do (the read program's questions: text from a kept start, the screen, the first row, the caret, colours, refused starts), and a virtual clock drives the time limits.
- The simulated terminal checked against the real ones. `cargo xtask regenerate-terminal-conformance-expectations`, run on demand on this machine or in the VM (it opens real terminal windows, so never in continuous integration), writes each stream of the corpus into the console host and the pinned Windows Terminal, chunk by chunk, waits after each chunk for a window title the player sets after it (a terminal changes its title only once it has parsed everything written before, and Windows Terminal parses it itself), reads each terminal with Verbatim's own read program, and saves what it answered as expectation files in the repository. A unit test in continuous integration replays the same streams in the simulated terminal and compares its answers with those files. The command is run when a stream is added to the corpus, when the simulator's behaviour is changed, and when Windows or Windows Terminal updates; the changed expectation files are reviewed in the commit like any other change of expectation. Events are not part of the expectations: they depend on timing, and the reader treats them only as signals to read.
- The oracle: invariants over the simulator's ground truth, checked whatever the stream did, for random streams with random read points (`proptest`), exhaustive small cases (every sequence of a few operations on a tiny terminal), and the recorded streams:
  - Every silence is accounted for, judged by text (decided by Dickson, 2026-10-23). The oracle does not decide what must be heard, which is the speech rules' business, and it judges only what a reader could observe: the text of the main screen's rows, never the hidden identity of the row a program wrote to, which no read reveals (section 3.4). Every line whose text no earlier read showed, on either screen (a reader cannot always tell them apart, section 5.8), while the terminal had the focus and output was reported, is either reported (spoken as a line, its growth or a redrawn row's change, spoken as a held row as last read before an overflow, or counted in a flood's summary, each report saying the line's new text) or withheld with a reason the reader states, and the oracle checks each reason's premise against the ground truth: a volatile row really is a row whose text changed at an earlier read and changes again; a growing line's unfinished word really is the end of a line that grows; a row held until the keys pause, held typing, and a key's answer spoken elsewhere really follow a key; a spinner's change really replaces one symbol that is not a letter or a digit (a counter's tick is heard, section 5.4); a row that only lost text really shows a part of what an earlier read showed on it, as the part of a wrapped line pushed down by a row inserted inside it does (section 5.7; decided by Dickson, 2026-10-24). A held line is then reported, or its text replaced on screen while still unreported, before the reader is asked once more after the stream with time passed, when nothing may still be held. Text a read showed before is not new, so moved, kept, lingering and reprinted rows need no reason; the cost is that output repeating text shown much earlier is not required by the oracle and is left to unit tests and scenarios. A line is reported twice only as a fault when its text stayed on the screen in between. Lines written and erased between two reads, lines that left a full history or a scroll region between two reads, and lines written in the same read as a resize are never shown to a read, so nothing accounts for them. A line whose start left a full history is new only if its text is not the end of a line shown before. When no read showed a switch to the alternate screen, the main screen is judged against what the alternate screen's read showed, which is also the last read a line's growth is measured from.
  - Every spoken line is text the stream printed, or a summary.
  - For every row whose text is unique in both reads and on the screen in both, the alignment maps it where that text went.
  - With no key pressed, following off and no switch of screens, the review position stays on its text while that text is on the screen and unique.
  - Padding is never spoken.
  These are what "by construction" means in section 11. What redrawn rows, keys, lists and colours say is decided speech, not a property of the stream, so it is tested by unit tests that state a stream and the speech it must give, never by an oracle that would repeat the rules (`docs/testing.md`).
- Expectations are guarded: an expectation is changed only from an NVDA capture or a decision recorded here, never to make a failing test pass, and a commit that changes one says which.
- Recorded streams of real programs as fixtures: Pi in both modes (`PI_TUI_WRITE_LOG`), PSReadLine in both modes, bash with readline, an Ink program, vim, less, htop, fzf, docker-like progress.
- End-to-end scenarios to add, with NVDA captures where NVDA's speech is the expectation: the landing rule's keys; Control+W, Control+U and Control+K; tab completion; suggestions; a completion list; menu completion; history search; output while typing; pasting; slow echo; a spinner with a message; several progress lines; resizing; a list by colour; a checkbox list; a refreshing full-screen script; fzf-like filtering; the review cursor kept through a footer redraw and moved with its text through a scroll; "report status bar"; moving by command; output just after the focus, with an interactive shell; the console host's real password prompt; Pi in both modes.

## 10. Cost

Measured on 2026-10-23, 120 columns, 30 rows, both terminals:

- A read of the screen's text alone, as one remote operation: 0.19 milliseconds in the console host, 0.27 in Windows Terminal.
- The same with each row's foreground and background colour: 0.96 and 0.50. Too coarse to tell a selected row (section 2.1).
- The same with every colour run of a colourful screen (283 runs): 3.9 and 1.9. Read with separate calls instead of one remote operation, the runs cost 56 and 118 milliseconds, which is why colours are read only in the remote operation.
- Colours are read only in the first read after a key, for the whole screen, after the key has reached the program, so a held key is not delayed: 1 to 3 milliseconds for most screens, 5 to 8 for Pi's, 31 to 72 in the worst case, added to the time from each key, typed characters included, to its speech. Read with every read, they would cost 6 to 12 percent of a core while Pi or Claude Code streams at up to 30 frames a second.
- The alignment: at most about 60 by 180 integer comparisons per read, microseconds; the histogram diff after a clear is close to the rows times their logarithm.
- The whole document is read at most once per group of spoken output, and never with output reporting off (section 4.2).
- The quiet-period timer: one wake-up for each thing that waits for it, when it ends.
- The terminal's writer waits while a read copies, so large reads are also measured by how much they slow the program's output: its lines per second with reads running against without.

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
4. Documents: done on 2026-10-24. What still held of the earlier design and its review moved here and both were deleted; `docs/parity/terminals.md` took the changed decisions; the roadmap's M4 entries point here.
5. The simulated terminal (`verbatim-term-sim`).
6. Conformance: the read program brought back beside the current one in `verbatim-uia-rops`; the stream player; `cargo xtask regenerate-terminal-conformance-expectations`; the expectation files for the hand-written corpus; the unit test that checks the simulator against them; the simulator fixed until it matches both terminals.
7. Recordings: a ConPTY recorder; streams of Pi in both modes, PSReadLine in both modes, an Ink program, and vim, htop, fzf, less and bash in WSL, with docker-like progress from a script; their conformance expectations.
8. The oracle and random tests, shown to catch deliberately flawed readers. Showing it fail against the reading Verbatim runs today was dropped (Dickson, 2026-10-23): that reading is the outpost's old screen reader, whose adaptation would cost more than it shows, since the core replaces it.
9. The core, in `verbatim-text`: the memory, the alignment, lingering rows, the known history, anchors, and the rules of section 5 on the virtual clock. The line diff becomes the row alignment.
10. The read program's colours and the remote operations audit.
11. The outpost and Core: reading moved onto the core; the review position as an anchor; following the caret only within a key's window; the key's window and the typed-character limit; the quiet-period timer; the subscription and the caret-event read restored.
12. "Report status bar" with NVDA+End; then moving by command with Verbatim+Alt+Up and Verbatim+Alt+Down, with their scenarios in both terminals, in a shell and in Pi's regular mode.
13. End-to-end scenarios: the ones listed in section 9, with NVDA captures where NVDA's speech is the expectation, after checking that the NVDA pin equals the installed build; the existing terminal scenarios brought back to passing. Then streaming and redrawing at a real program's rate, which no NVDA capture can give an expectation for, since NVDA reads these cases poorly (decided by Dickson, 2026-10-24): a test program of Verbatim's own, driven phase by phase through a pipe, writes each way a program streams text, at up to 60 frames a second, beside a redrawn region with a spinner and a timer, with typing at its prompt; the region is a footer, a header (a scroll region that starts below the top, whose scrolled-out lines never reach the history), and a status row in the middle, between the output and the input, as Pi draws one:
    - appending to the last line, a token at a time, split anywhere, including inside a word and inside a character's bytes;
    - rewriting the whole line for each token, after a carriage return or an erase of the line;
    - redrawing a whole region each frame, Ink's way, with the streamed paragraph inside it growing and wrapping;
    - wrapping by the program itself, so a partly written word moves down to the next row;
    - writing into a scroll region above a fixed footer, or inserting lines above it;
    - clearing and repainting the screen each frame, on the main screen and on the alternate screen;
    - earlier lines changed as the text streams, as a Markdown renderer restyles or rewraps them.
  Each is recorded into the conformance corpus, so the oracle checks every read point of it, and the random streams gain the same operations. The scenarios' expectations come from this document's rules: in a phase whose pace the test sets, the exact speech; in a phase at the program's own rate, the exact text of everything spoken for the stream, joined, which must be the streamed text once, in order, with no word split, whatever the reads' timing.
14. Measurements after, against the baseline, recorded in `docs/performance.md`.
15. This document rewritten to describe the design as built, for any reader; the decisions move to `docs/parity/terminals.md`.
16. The whole terminal suite, `cargo xtask ci`, and the push.
