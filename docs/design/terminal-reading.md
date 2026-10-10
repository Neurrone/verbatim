# Terminal reading in Verbatim: a design from first principles

Written 2026-10-10 for Dickson. Read-only research: nothing in the repository was edited, built or run. The design covers Windows Terminal and the console host (conhost), both read through UI Automation (UIA).

## How to read this document

Each requirement and rule carries its source:

- "NVDA" with a file and line under `nvda/source`, or a capture file in the scratchpad (`nv-con-*.txt` for the console host, `nv-wt-*.txt` for Windows Terminal, all NVDA alpha-57645 captured 2026-10-09).
- "Dickson, date" for a decision recorded in `phase6-design.md` as Dickson's. These are requirements. Where this design argues to revisit one, it says so and the point is repeated in the open questions.
- "Agent, to be confirmed" for behaviour an agent built and `phase6-design.md` records as not yet confirmed.
- "Proposed" for anything new in this document.

"Uncertain" marks a claim this study could not check. "Measure" marks a fact the design depends on that has to be measured live before the code is written.

## Summary

NVDA's model is simple: on each text-change event, read the terminal's whole text in one call, diff it against the previous text, speak what was inserted, and hold typed characters until the screen changes. Its weaknesses are cost (the whole buffer per change), a character diff that speaks fragments, typed text spoken twice, passwords spoken whenever the screen changes, and fixed waits for caret keys.

Verbatim's current terminal code reads only the visible screen and works out, from several provider calls inside one remote operation, how far the text scrolled since the last read. Because those calls are not atomic, every read has to be judged trustworthy or not, and the judgement rules were patched four times in two days (audit chain C).

The base design proposed here keeps Verbatim's decided behaviour but changes what a read is: everything that is spoken or counted comes from a single `GetText` call on one range, which the terminal answers from one locked snapshot of its buffer. That range starts at the top of the screen as last read and runs to the end of the text, so lines that scrolled past between reads are in the snapshot and are counted from the text itself. Positions from other calls are used only as hints (where to start the next read); a wrong hint can cost an extra read but cannot change what is spoken. This removes the trust rules, the anchor search, the row counting, the footer guess and the kept earlier screens. Key attribution (what Backspace or Escape removed, which line an Up Arrow landed on) moves to Core, matched by read time against the key's press time, so the outpost no longer needs the screen from before a key.

## 1. Requirements

### Output

- New output is spoken line by line, queued in order, one utterance per line; newer output never cancels older output still waiting. Sources: NVDA (`NVDAObjects/behaviors.py` 455 to 466 and 496 to 502, every line queued); Dickson, 2026-10-06 ("newer output never cancels older output", phase6-design.md, "The flood policy, reconsidered").
- Blank lines are not spoken. NVDA (`diffHandler.py` 45 to 47 and 73 to 75); Dickson, 2026-10-06 ("blank lines are dropped").
- Only inserted text is spoken; lines that were only deleted, or a line that only lost text, say nothing. NVDA (`diffHandler.py` 39 to 41, only "+" pieces); Dickson, 2026-10-08 ("what was inserted is spoken, never what was only deleted (NVDA's rule)").
- A line rewritten in place speaks from the start of the word that changed, using Unicode word rules with ICU dictionaries and whole graphemes, so it works in Chinese, Japanese and Thai. Dickson, 2026-10-08 (changed-word rule, and the language audit decision). NVDA differs: its character diff speaks the inserted characters only.
- A line that only had one symbol replaced (a spinner) is silent until it changes to text. Dickson, 2026-10-07.
- A line rewritten while an earlier version of it is still waiting replaces that version, so a fast progress bar is spoken once, as it last was. Agent (recorded in `docs/crates/verbatim-core.md`, "Terminals"); consistent with Dickson's flood decisions; not separately dated.
- A line read half rewritten speaks what changed from what it last said ("51%", not "oading 51%"). Dickson, 2026-10-07 (the NVDA study's risk "A rewrite split across two reads speaks a fragment" was to be fixed with a test).
- Long lines are spoken whole, however long. Dickson, 2026-10-07 (the 4 KB cut removed; 10 MB bound on waiting output; a single line larger than that is cut on a grapheme boundary and Verbatim says so).
- Lines made only of symbols are spoken as they are; punctuation processing (M8) decides later. Dickson, 2026-10-08.
- Inline prediction ghost text is spoken. Dickson, 2026-10-07.
- Very short output ("y", "ok") is spoken, not taken for typing echo. Recorded as a deliberate difference in `docs/parity.md` ("New terminal output") and asserted by `*_short_output`; no dated decision by Dickson was found (open question 11). NVDA drops a one-character change (`behaviors.py` 491 to 495) and a single line shorter than the current word plus one or than three characters (559 to 566).
- Output is spoken in one language for the terminal unless the terminal reports a language per line. Dickson, 2026-10-07.
- Windows Terminal's output notifications (activity id `TerminalTextOutput`) are ignored so nothing is spoken twice. NVDA (`NVDAObjects/UIA/winConsoleUIA.py` 474 to 476 for the default diffing overlay; 431 to 440 for the console host); Dickson, 2026-10-06 (diffing chosen, notifications not used).
- Output from anything but the focused terminal is not spoken. NVDA (only the focused terminal is monitored, `behaviors.py` 519 to 525; the notification overlay checks the focus's app, `winConsoleUIA.py` 502 to 503).
- "Report new output" (Verbatim+5, NVDA's key for "report dynamic content changes") turns output speech off and on, saying "report new output on" or "off". Dickson, 2026-10-06. NVDA: user guide `user_docs/en/userGuide.md` 3406 to 3410.
- While output reporting is off, the terminal is still read, because typing echo depends on those reads. Dickson, 2026-10-07.

### Typed-character echo

- Each typed character is echoed when the terminal shows it, by the typing echo settings. NVDA (`behaviors.py` 572 to 593: queued, dispatched at the next text change).
- With "speak passwords" off (the default), typing the terminal does not show is never spoken. NVDA (`behaviors.py` 579 to 587; configSpec `terminals.speakPasswords`, `config/configSpec.py` 317; user guide 4312 to 4316); Dickson, 2026-10-06 ("Verbatim must follow NVDA", with an end-to-end password test).
- A password is never spoken because something else on the line changed: a clock ticking on the prompt's line, or a prompt that shows asterisks. Dickson, 2026-10-08 (the test list in "Terminal reading by diffing the screen" includes "a clock in a footer ticking during a password (no leak)" and "one echoing asterisks"). NVDA leaks in both cases: it dispatches its queued characters on any text change (`behaviors.py` 591 to 593), captured in `nv-con-alt.txt` (each "x" typed into a script that reads keys without showing them is spoken as "x") and recorded in `docs/parity.md` ("At a password prompt NVDA echoed each typed character").
- Typed text is not spoken a second time as output. Proposed as a requirement; Verbatim already does it. NVDA fails it when its diff gathers several typed characters into one insertion: `nv-con-floods.txt` line 25 and `nv-wt-floods.txt` line 24 ("dot flood dot ps 1" after the characters were each echoed).
- A space typed at the end of a line is echoed when typed, although the terminal's padding hides it. Dickson, 2026-10-07 (scheduled item 3).
- Typing past the right margin is echoed, not spoken as a new line. Dickson, 2026-10-08 (test list).
- A tab completion's added text is spoken as output. NVDA (`behaviors.py` 573 to 576, the filter is disabled after Tab); Verbatim's matching gives the same.
- Held typing is forgotten on Enter, Tab, Control+C, Control+D and Control+Break (NVDA, `behaviors.py` 595 to 613) and also on Escape (Dickson, 2026-10-09, coherence review decision 5, kept as a deliberate difference).
- The console host's keys are translated with the console host's own keyboard layout. NVDA (`winConsoleUIA.py` 378 to 388, issue 10113).
- Up Arrow and a character typed before the recalled line is read are heard as the line's change ("echo onex") without the character's own echo. Dickson, 2026-10-09 (kept as Verbatim's behaviour).

### Backspace, Delete, Escape and other editing keys

- Backspace and Control+Backspace speak the text they removed. NVDA speaks the character or word before the caret, read before the key is sent (`editableText.py` 293 to 321); Dickson, 2026-10-08 ("What a key did to the text": the removed text, from evidence, in any language). Captured: both say "o" and "tw" after `echo one two` (`docs/parity.md`).
- Delete speaks what it leaves at the caret. NVDA (`editableText.py` 329 to 342); Dickson, 2026-10-09 (coherence decision 2).
- Escape clearing a line, Control+W and Control+U speak what they removed. Dickson, 2026-10-08 ("What a key did to the text"). NVDA is silent for Escape: it has no script for it in a terminal (`editableText.py` 388 to 412 lists the bound keys), captured in `nv-con-floods2.txt` lines 130 to 131 and 154 to 155.
- Escape on a typed line that wrapped onto a second row says the whole text removed. Agent, never put to Dickson on its own (audit chain A, a58a7fc); NVDA is silent. Open question 5.
- A key outside the caret table that types no text and only moves the caret is spoken by where the caret landed: another line, one character, the line's start or end, a word, or the word then the character. Dickson, 2026-10-08 ("Caret moves from keys Verbatim does not know").
- A caret move the program makes on its own, with no key, is not spoken. Dickson, 2026-10-08; NVDA does not use caret events in terminals (`behaviors.py` 527 to 530).

### Line and caret keys

- Left and Right Arrow say the character, Home and End the character at the caret, Control with Left or Right the word, Up and Down the line, by NVDA's unit table. NVDA (`editableText.py` 388 to 412); Dickson, 2026-10-08 ("Units for caret moves": a small fixed table for the standard Windows editing keys).
- Up and Down Arrow in a terminal work as in a text field: when the caret moves to another line, that line is spoken; whatever the program redraws elsewhere is spoken at once as output. Dickson, 2026-10-09 ("Terminal line keys as NVDA has them"). NVDA (`editableText.py` 181 to 192; terminals do not use caret events, `behaviors.py` 527 to 530).
- A key that does not move the caret is silent. Dickson, 2026-10-08 (afternoon decisions, deliberate difference). NVDA speaks the caret's line after its 100 ms wait, three times longer in Windows Terminal (`editableText.py` 86 to 88; `winConsoleUIA.py` 374 to 376 and 470): captured as ":" after each pager key in `nv-con-pager.txt` lines 59 to 77, and "blank" for each key of a list redrawn below the caret (`docs/parity.md`).
- A line change caused by a line key is not spoken twice (as the key's answer and again as output). Proposed as a requirement; Verbatim does it today (`key_owns_line`). NVDA fails it: `nv-con-floods2.txt` lines 126 to 129, Up Arrow said "ready greater dot hundred dot ps 1" and then "dot hundred dot ps 1".
- A line change on the caret's line from a key that did not move the caret is spoken as output, not dropped. Agent, to be confirmed (2026-10-09, `key_owns_line`, PSReadLine's menu).
- No fixed wait: a caret key never blocks anything else of the application, and its answer comes whenever the evidence comes. Dickson, 2026-10-08 (afternoon decisions: "the outpost never blocks waiting for a caret move").

### Pagers and full-screen programs (the alternate screen)

- When a full-screen program draws, the whole visible screen is compared with the one last seen, not only the newest lines; the flood limits apply only to what is spoken. Dickson, 2026-10-07 ("Full-screen programs").
- A pager moving one line speaks the line that came onto the screen, not every row. Dickson, 2026-10-09. NVDA says the same line, after ":" (`nv-con-pager.txt` lines 59 to 74).
- Closing a full-screen program speaks only what followed it, not the main screen again. Dickson, 2026-10-09 (coherence decision 5, kept as a deliberate difference). NVDA speaks the main screen again, or a fragment of it: `nv-con-pager.txt` line 81, `nv-con-alt.txt` near its end ("eady greater dot altscreen dot ps 1").
- The alternate screen is detected from the text itself (no history above the screen), verified live in both terminals. Dickson, 2026-10-07.
- Selection lists drawn by a program: the line the caret moves to is spoken (NVDA's behaviour); a list redrawn with the caret below it speaks the line that gained the marker as output, from the word that changed. Dickson, 2026-10-09 for the first; the second is recorded in `docs/parity.md` as a difference following from two deliberate differences, built by an agent and to be confirmed.

### Floods

- The first "Lines spoken in full" lines of a burst (default 30) are spoken whole, never cut short by what arrives meanwhile. Once that group has been heard, if more lines wait than that limit, all but the newest "Last lines to speak" (default 30) become "skipped N lines", and the decision repeats after each group. Dickson, 2026-10-06 (the panel) and 2026-10-07 (by groups, "Test design decisions").
- Every terminal line counts in N, blank lines included, whether the outpost never read it or Core cut it. Dickson, 2026-10-07; confirmed in the coherence review, 2026-10-09 (decision 1).
- When lines left a full history unread, Verbatim says "skipped more than N lines", N being the history's size less the lines about to be spoken; when no count is possible, "skipped lines" without a number. Dickson, 2026-10-07.
- Both limits may be raised above the terminal's history (to 10,000), degrading to NVDA's behaviour of reading everything. Dickson, 2026-10-07 and 2026-10-09 (coherence decision 3).
- A key (anything but Shift) cuts speech off and drops the output still waiting, and nothing read before the key is spoken after it. NVDA (`inputCore.py` 583 to 603, the cancel queued with the key; `keyboardHandler.py` 729 to 737); Dickson, 2026-10-07 ("A cancel tells the outpost to move its anchor to the end without speaking, as NVDA drops what is pending").
- Shift pauses and resumes speech; a paused queue does not drain. NVDA (`keyboardHandler.py` 736 to 737); Dickson, 2026-10-07.
- Reading is on demand while Core's queue is full: the outpost only notes changes until Core asks. Dickson, 2026-10-07 ("Reading terminal output on demand"), with no minimum interval between reads.
- A flood never hangs Verbatim; Verbatim commands are answered during it. Dickson, M4 exit criteria (phase6-design.md, "Scope from the roadmap").

### New windows and tabs

- A terminal taking the focus says "terminal" (the console host's English-only "Text Area" name dropped, as NVDA does) and the caret's line; an empty line says "blank", in a new window or tab of either terminal. Dickson, 2026-10-09 ("Decisions on terminal openings", reversing an agent change); NVDA says it in the console host (`nv-con-edit.txt` line 5, "terminal blank") but not for most new Windows Terminal tabs, which Dickson judged an NVDA bug.
- What was written while the terminal was in the background is not spoken as new output on return; the caret's line is. Asserted by `*_leave_flood`; NVDA said the same on returning (captured, see the scenario module). No dated decision found; consistent with NVDA monitoring only the focus (`behaviors.py` 519 to 525).

### Focus entering a terminal while output is arriving

- What is on the screen when the terminal takes the focus is not new; any output written after that is spoken. Proposed requirement, matching NVDA: in `nv-con-edit.txt` lines 4 to 6 NVDA says "terminal blank" and then PowerShell's start-up notice written just after. Verbatim has an open bug here (`docs/roadmap.md`, "Output written just after a Windows Terminal terminal takes the focus can go unspoken"), hidden since the suite's shells run `-NonInteractive`.

### Robustness rules for the implementation

- No fixed waits, retries, polling or hacks; waits only for evidence; root-cause fixes. Dickson (memory note "Never hack around problems"; 2026-10-08 afternoon: every blocking wait in the outposts audited and removed; 2026-10-07: "no minimum interval between reads").
- UIA call sequences run as remote operations with the classic fallback behind one entry point. Dickson, 2026-10-07 (item 7).
- The legacy console (no UIA text) is not supported. Dickson, 2026-10-08.
- The platform-neutral crates contain only original code written from the parity docs. Project rule (CLAUDE.md). The terminal diff, if it moves there, must be written from prose, not translated from `diffHandler.py`.

## 2. NVDA's model, precisely

### Which class handles which terminal

- The console host on Windows 11 is read through UIA when its API level is FORMATTED (`UIAHandler/utils.py` 337 to 348 with `winConsoleImplementation` "auto"). The text area (automation id "Text Area") gets `WinConsoleUIA` (`winConsoleUIA.py` 361 to 448), a `KeyboardHandlerBasedTypedCharSupport`, itself an `EnhancedTermTypedCharSupport`, a `Terminal`, a `LiveText` plus `EditableText` (`behaviors.py` 511 to 633).
- Windows Terminal's `TermControl` and Visual Studio's `WPFTermControl` get `_DiffBasedWinTerminalUIA` by default (`NVDAObjects/UIA/__init__.py` 1454 to 1474; `winConsoleUIA.py` 463 to 476), an `EnhancedTermTypedCharSupport`. The notification-based class is opt-in (`terminals.wtStrategy`, default diffing, `config/configSpec.py` 320; user guide 4340 to 4357).
- The legacy console (`NVDAObjects/window/winConsole.py`) reads only the visible lines through console APIs; Verbatim does not support it.

### Events used

- UIA's text-changed event, registered for the console's "Text Area" automation id and, under the diffing strategy, for Windows Terminal's classes (`UIAHandler/__init__.py` 145 to 147 and 778 to 795), becomes NVDA's `textChange` event.
- `event_textChange` only sets a `threading.Event` (`behaviors.py` 420 to 424), so any number of events between two reads collapse into one more read. That is all the coalescing there is for UIA consoles.
- UIA notification events are blocked for both terminals under diffing (`winConsoleUIA.py` 431 to 440 and 474 to 476).
- Caret events are not used for caret keys in terminals (`behaviors.py` 527 to 530): caret keys always poll.
- The legacy console turns its console WinEvents into text changes (`winConsoleHandler.py` 84 to 88 and 155 to 173); UIA consoles do not.

### What is read, and in how many calls

- Monitoring starts on focus (`behaviors.py` 519 to 525). The monitor thread first reads the text as a baseline (468 to 473), then loops: wait for the event, sleep `STABILIZE_DELAY` if set (0 for UIA terminals; 0.03 s for the legacy console, `winConsole.py` 23), clear the event, read the text again (475 to 488).
- The text is `makeTextInfo(POSITION_ALL)` (`behaviors.py` 446 to 453). For FORMATTED consoles and Windows Terminal that is the text pattern's `DocumentRange` (`NVDAObjects/UIA/__init__.py` 511 to 512; `winConsoleUIA.py` 396 to 397 uses the plain `UIATextInfo`), so the whole buffer, history included. DMP reads it with one `GetText` call (`diffHandler.py` 33 to 34). Difflib reads it line by line (`getTextInChunks`, 112 to 113), many calls; it is chosen only for consoles bounded to the visible screen (`winConsoleUIA.py` 408 to 414; `winConsole.py` 43 to 46).
- No consistency check is made on a read: whatever the text was is diffed.

### The diff

- Diff-match-patch, by character, by default (`behaviors.py` 426 to 439; `diffHandler.py` 25 to 51): every inserted piece is taken, a line break added if missing, split into lines, and blank lines dropped. Deletions are ignored.
- Difflib, by line, for screen-bounded consoles (`diffHandler.py` 54 to 110): inserted lines are taken; a line replacing a deleted line speaks only the changed span when fewer than 15 characters changed (102 to 104), otherwise the whole line.
- The `terminals.diffAlgo` setting can force either (`diffHandler.py` 116 to 131; user guide 4326 to 4338).

### What is spoken

- After the diff, a result that is one line of one non-space character is dropped as probably typed (`behaviors.py` 491 to 495).
- `EnhancedTermTypedCharSupport._reportNewLines` also drops a single line shorter than the typed word's length plus one, or than three characters, unless the last key was Tab (559 to 566), then clears the typed word buffer and the queued characters (567 to 570).
- Every remaining line is queued with `speech.speakText` (464 to 466), in order, with no limit. Only the user's next key (any but Shift) cancels it (`inputCore.py` 583 to 603).

### How typed characters are handled

- Typed characters come from NVDA's keyboard hook through `ToUnicodeEx` for the console host (`keyboardHandler.py` 178 to 202 and 326 to 355).
- With "speak passwords" off and typed character or word echo on, they are queued (`behaviors.py` 572 to 589) and dispatched at the next text change of any kind (591 to 593, 615 to 619). A password that changes nothing on the screen is never spoken; one at a prompt that shows asterisks, or whose screen changes for any other reason, is.
- Enter, Tab, Control+C, Control+D and Control+Pause clear the queue and the typed word buffer (595 to 613).

### Caret keys

- `EditableText` binds the caret keys (`editableText.py` 388 to 412). Each script reads the caret's bookmark, sends the key, then polls `_hasCaretMoved` (70 to 163): spin three times, then sleep 10 ms per try, up to `caretMoveTimeoutMs` (100 ms, `configSpec.py` 364) times the multiplier, 1 for the console host and 3 for Windows Terminal (`winConsoleUIA.py` 374 to 376, 470, 486). It stops early on a newer script (94) or a pending focus (97 to 99). It then speaks the unit at the caret whether or not the caret moved (`_caretScriptPostMovedHelper`, 165 to 178).
- The key is swallowed by the hook and re-sent by the script on the main thread; every following key without a script is queued behind it so keys stay in order (`inputCore.py` 641 to 645, issue 2953). Escape and typed characters have no script and pass at once.

### Known weaknesses, with evidence

- Cost of reading the whole buffer on every change. Measured 2026-10-06 (phase6-design.md, "Terminal output: notifications or diffing"): after 5,000 lines in a 120-column Windows Terminal the document was 610,244 characters, 86 percent padding, 6 ms to read before any diff; NVDA repeats that per change. NVDA's own fixes were coalescing in C++ (#14888) and trimming blank rows on older consoles (#14689, `winConsoleUIA.py` 114 to 123).
- Lines go missing in floods even though the whole buffer is read. `nv-wt-floods.txt` lines 27 onward: the first flood's speech starts at "flood line 152" and jumps to 444 and 648; `nv-con-floods.txt` lines 28 to 36: lines 1 to 5, then 28 to 33, then 56 to 58; `docs/parity.md` records 690 of 10,000 lines queued in Windows Terminal in an earlier transcript. Uncertain: the cause is not established from NVDA's source. Opening a 30-row alternate screen in one write also lost rows (`nv-con-alt.txt`: rows 16 to 19 and 25 missing).
- Fragments from the character diff. `nv-con-alt.txt`, last lines: "eady greater dot altscreen dot ps 1" on closing the alternate screen, the "r" of "ready" matched elsewhere. The user guide warns DMP reading "may be choppy or inconsistent" (4333 to 4335).
- Typed text spoken twice: `nv-con-floods.txt` 25, `nv-wt-floods.txt` 24 and 251; a recalled line spoken twice, `nv-con-floods2.txt` 128 to 129.
- Passwords spoken whenever the screen changes for another reason: `behaviors.py` 591 to 593; `nv-con-alt.txt` ("x" spoken for keys a script reads without showing); `docs/parity.md` (clock prompt, asterisk prompt).
- Short output dropped as probably typed: `behaviors.py` 491 to 495 and 559 to 566 ("y", "ok", and status-line changes of one or two characters such as vim's ruler, `docs/parity.md`).
- Output scrolled off between reads. For screen-bounded consoles this lost lines and made reading choppy (#12974), which is why those use Difflib. For FORMATTED consoles and Windows Terminal it is not a problem until the history overflows, because the whole buffer is read.
- Repeated identical screens. When the history is full and every line is the same, the buffer's text is identical before and after a flood, so the diff finds nothing (inferred from the algorithm; not captured). With history not full, NVDA spoke every line of a 2,000-line identical flood (`terminal_flood_kinds.rs` module docs).
- The legacy console's 30 ms sleep before every read (`winConsole.py` 23; `behaviors.py` 479 to 484), a fixed wait.
- Caret polling: 10 ms sleeps up to 100 to 300 ms, then the unit spoken even when nothing moved (":" in `nv-con-pager.txt` 59 to 77, at about 110 ms). The poll holds NVDA's main thread, so events wait behind it.
- No flood control: every line is queued until a key; in the console host NVDA was still speaking a flood after its window closed (`docs/parity.md`, transcripts of 2026-10-07). NVDA tried a 100-line cap and reverted it because a newer batch cancelled the older one and users lost output (#20177, #20649, reverted in #20898; `docs/nvda/editable-text-and-terminals.md` 207 to 217).
- A strength worth keeping: monitoring starts with a baseline read at focus, so output written just after the focus is spoken (`nv-con-edit.txt` 4 to 6).

## 3. The base design

### What changes and what stays

- Stays: the process layout (the focused application's outpost reads; Core decides what is spoken), diffing rather than notifications, Core's flood policy and typing matching, on-demand reading, and every requirement in section 1.
- Changes: what one read is, where the diff lives, and where keys are attributed.

### The atomic unit of reading

The terminal's UIA provider answers each text range method from one locked snapshot of its buffer: in Microsoft's terminal source, `UiaTextRangeBase::GetText` takes the console lock for the duration of the call. Uncertain: believed from that source, not checked in this repository; measurement 1 in section 5 confirms it in both terminals.

Calls are not atomic with each other: the terminal can write between any two of them, including inside one remote operation, which runs each instruction as its own provider call. Ranges are also row coordinates: once the history is full, a kept range stays on its row while the text moves up beneath it (measured 2026-10-06, phase6-design.md, "What the first live runs found").

So the unit is one `GetText` call. The base design's rule, proposed:

- Everything spoken and everything counted comes from one `GetText` call, the read's snapshot.
- Other calls in the same read (the visible range, the caret, the range to keep for next time) give hints. A hint may be stale by the time it is used. The design must make a stale hint cost at most an extra read; it must never change what is spoken or counted. The one exception, identical lines with a full history, cannot be resolved from text by any method, including NVDA's, and is stated as a limitation.

This rule is what removes the trust rules: there is no longer any read to distrust, only hints that are checked against the snapshot by content.

### One read

One remote operation (classic fallback behind the same entry point), proposed:

1. Start: the range kept from the last read, at the start of the screen's top row as it was then. With no kept range (the first read after a focus), the start of the visible range.
2. End: the end of the text. Measure 2 decides which end: the document's end, if both terminals end it at the last written row; otherwise the visible range's end moved down by one screen's height and clamped to the document's end. The extra screen covers a footer the console host redraws a row lower while its view moves (the case `footer_below` handles today); trailing blank rows are trimmed after reading, so the padding cost is bounded by one screen.
3. The snapshot: one `GetText(-1)` on that range.
4. Hints, after the snapshot: the caret, expanded to its row, with that row's text; the visible range's start, kept as the next read's start; whether the text starts where the screen starts (no history above the screen).

When step 1's kept range fails (the alternate screen; NVDA notes the console host returns `E_FAIL` comparing ranges across buffers, `winConsoleUIA.py` 416 to 429; Windows Terminal is uncertain, measure 4), the same read starts from the visible range instead. No retry loop: that is a different read, chosen by the error.

The outpost trims padding (trailing `White_Space`, as decided 2026-10-06) and splits the snapshot into lines. Both terminals give a wrapped line whole in the text (measured, phase6-design.md, "Selection lists in a terminal", the wrapped Escape), so these are logical lines, and no row arithmetic is needed for the diff.

### The diff

A pure function, original code in a platform-neutral crate (proposed: a module of `verbatim-text`, which already provides the word segmentation and grapheme boundaries it needs; open question 12), written from `docs/parity.md` and this document:

- Inputs: the old screen's lines (memory) and the snapshot's lines.
- Alignment: a line diff (longest common subsequence of lines, with common prefix and suffix trimmed first so the usual case, nothing changed above the old last line, is linear). Only the part of the snapshot the old screen can overlap is aligned, the snapshot's first lines up to the old screen's length plus one screen; everything after the last aligned old line is appended output. When two alignments tie (identical lines), the one that keeps lines at their position wins, since the read started at the old screen's top.
- Output, in speaking order: lines inserted above the old last line; changes of lines rewritten in place (paired deletions and insertions in one region), by the changed-word rule, the spinner rule and the half-rewritten rule; then the appended lines. A flood is cut by the read limits into the first lines, a count of the lines between, and the newest lines, all exact because they come from the snapshot.
- Rows below the caret that are blank are not lines yet (extra 7, replacing `Unwritten`).

This is NVDA's line diff (the Difflib model, which NVDA uses exactly where the visible screen is the unit, #12974) with the changed-word rule in place of the 15-character rule, and applied to a snapshot that also holds everything written since the last read, which is what NVDA's whole-buffer read buys.

### When to read: evidence, coalescing and on-demand

What replaces NVDA's event flag, its 30 ms legacy sleep and its caret polling, proposed:

- Evidence that the text changed: UIA's text-changed event for the focused terminal; for the console host also its console WinEvents, if Dickson keeps them (extra 12).
- Coalescing: a terminal has a "changed since the last read began" flag. A change event sets it, with the time it was observed. When the worker is free and the flag is set, it reads; a change observed after a read began sets the flag again, so exactly one more read follows. No delay, no minimum interval. This is the invariant that keeps the final state read: every change event is followed by a read that began after the event was observed.
- On-demand reading (Dickson, 2026-10-07) stays as a pure state machine with two states, Live and Held. The third state, Owed (a read Core asked for that the terminal disturbed), goes: a read can no longer be disturbed, so every read answers.
- Caret: the caret is read with every text read (as today, e6dcfcf) and on each caret event (caret only). A caret event is not evidence that text changed.
- Caret keys: no polling and no deadline. The key's answer is the first caret report, from a read ending after the key, that shows the caret moved (see "Keys" below).

What happens when the terminal raises no further events: nothing is read. The last snapshot stands as what the terminal shows; Core is answered from it when it asks and nothing changed since. Output written after the last event-triggered read with no event of its own stays unspoken until the next event, which is the terminal's defect, not something to cover with a timer. Section 4, extra 12, covers the one known case (the console host's UIA text changes stopping mid-write).

### Focus

Proposed, to fix the roadmap bug of output just after focus going unspoken:

- Subscribe to the terminal's text changes first, then take the baseline read. Any change observed after the baseline read began leads to another read, whose new lines are spoken. Uncertain: whether the roadmap bug is this ordering is a hypothesis; the fix is to make the order explicit and add the scenario the roadmap asks for.
- The baseline read speaks nothing except the focus announcement's caret line ("blank" for an empty line, Dickson 2026-10-09).
- Leaving forgets held output, as now; returning takes a new baseline, so what was written meanwhile is not new.

### Keys: attributed in Core by read time

Root cause 1 of the audit: Verbatim hears of a key in the outpost later than the terminal acts on it, so the outpost keeps earlier screens and carets to reconstruct the state before the key. Proposed instead:

- The outpost attributes nothing. Each read sends, besides output, facts about the caret's logical line: the caret before and after (as row text and position in the line), what the line gained and what it lost between the last snapshot and this one, and the time the read ended.
- Core already receives each key from the hook before the key reaches the application: the hook reports it and only then calls `CallNextHookEx` (verify-terminal.md, Part 2, step 1). If Core takes its two input channels in time order (each message stamped when queued, the hook's at the press, the outpost's at the read's end) rather than picking at random (`docs/roadmap.md`, "Core picks between the outpost's events and its commands at random"), the key is always processed before any read that could show its effect.
- A fact from a read that ended before the key's press time is not the key's. A fact from a read that ended after it may be. Facts are matched by content: typed characters against what the caret's line gained (as today), Backspace, Control+Backspace, Escape, Control+W and Control+U against what it lost next to the caret (the guard of 2026-10-08: the removed text was adjacent to the caret and the caret now sits where it began), line keys and the landing rule against the caret's move. A read ending after the press that does not show the effect answers nothing; the next one may.
- The caret before a key is the newest caret report from a read that ended before the press. A report of that kind arriving after the key (it was in the pipe) replaces it. Nothing else is kept: no fixed-size history.
- A change on the caret's line and a caret move in the same read are judged together: when the caret moved to another line, the line is the key's answer and its change is not output; when it did not move, the change is output. That replaces `key_owns_line`. A caret event that answers first carries the line's text; a later read's change to that line, equal to what was just spoken as the answer, is not spoken again.
- A watch ends at the next key or a focus change. The ten-second bound on a watch is a fixed timeout (`docs/parity.md`); proposed to drop it (open question 10).

The same mechanism serves text fields; this document covers terminals only, but the change in Core should be made once.

Holding keys as NVDA does (measurements pending). Both ways:

- Keys not held (the base): as above.
- Keys held: the outpost reads before the key is re-sent, so the caret and line before the key are exact, and the read after the key is compared with it directly. The facts mechanism still works unchanged; holding only makes "before" exact. Typed characters and Escape, which NVDA does not hold, still go through the facts. Holding adds the round trip to every held key and needs re-injection with `uiAccess` considerations (verify-terminal.md, Part 4 (a)); nothing in the terminal design depends on choosing it.

### What Core keeps

Unchanged: the flood policy by groups, the backlog bound of 10 MB, typing held and matched, the waiting output dropped on a cancel, "Report new output". Changed: key attribution (above); the outpost's message keeps the same `TerminalOutput` shape (`above`, `changed`, `head`, `skipped`, `lines`) plus the caret-line facts with the read's end time.

## 4. The extras

Each is separate from the base and can be adopted or not on its own.

### Extra 1: catching lines that scrolled off between reads

- Gain: a command printing 50 lines while a read is under way has all 50 in the next snapshot, from the old screen's top to the end. Without it (a read of the visible screen only, NVDA's legacy model), the lines that scrolled past the top between reads are lost; NVDA's UIA terminals do not lose them because they read the whole buffer.
- Cost: none beyond the base. It is the base's choice of start (the kept range) and end (the end of the text). The snapshot grows with the output since the last read; with on-demand reading it can reach the whole history (about 1.1 MB of text in a full 120-column Windows Terminal; 610 KB read in 6 ms, measured 2026-10-06). Measure 3.
- Split reads and trust rules: none. One `GetText`.
- Failure modes: when the history has discarded more than a screen since the last read, the kept range's row now holds newer text and the old screen is not in the snapshot. The diff finds no overlap; the outpost then reads the whole document in one more `GetText` (a second atomic read, chosen by the content of the first, not a retry) and aligns again. If the old screen is still not found, the history overflowed past it (extra 3).
- NVDA: reads the whole buffer every time; same coverage, higher cost per change.
- Recommendation: adopt, as part of the base. It is what makes Verbatim at least as good as NVDA for output that outruns a read, and it is cheaper than NVDA in the usual case.
- Holding keys: no interaction.

### Extra 2: counting skipped lines in floods

- Gain: "skipped 1941 lines" rather than an uncounted "skipped lines" (Dickson's requirement, 2026-10-07).
- Cost: in the base, none: the count is the number of appended lines in the snapshot less those sent. Today it costs the anchor search (`FindText`, up to 20 matches at about 3 ms each), a count of rows by moving a range, a binary search for the exact shift (83894f6), and reading the rows that went by unread, all in separate calls.
- Split reads and trust rules: none in the base. Today it is the source of every trust rule in audit chain C.
- Failure modes: identical lines with a full history (any count is a guess, so none is given; see open question 7); history overflow (extra 3).
- NVDA: no counts and no flood policy.
- Recommendation: adopt in the base form, replacing the `FindText` search Dickson chose on 2026-10-07 (open question 1).
- Holding keys: no interaction.

### Extra 3: history overflow ("skipped more than N lines")

- Gain: an honest count when a flood outruns the terminal's history between reads, as decided 2026-10-07.
- Cost: only when extra 1's two reads both fail to find the old screen. The whole-document snapshot gives N directly: its lines less the lines about to be spoken.
- Split reads: none; both reads are single snapshots.
- Failure modes: telling overflow from a cleared screen or a switch of buffers. Rule, proposed: overflow when the whole-document snapshot has history above the screen and its first line differs from the first line last seen; a cleared screen (`cls` clears the history too in the console host, uncertain for Windows Terminal) or an alternate screen has none. Measure 4 confirms the evidence per terminal.
- NVDA: no count; it speaks whatever its diff of the whole buffer finds.
- Recommendation: adopt.
- Holding keys: no interaction.

### Extra 4: scrollback reading

Two different things go by this name.

- Reviewing history with the review cursor and say-all, above the screen. NVDA's review in a FORMATTED console or Windows Terminal covers the whole buffer, since its text info is the plain `UIATextInfo` over the document (`winConsoleUIA.py` 396 to 397); only older consoles bound review to the screen (39 to 108). Verbatim's terminal review is a screen grid (`terminal_review_grid`). Recommendation: needs a decision of scope, not measurement; it is outside output reading and unaffected by this design.
- Speaking output written while the terminal was in the background, on return. NVDA does not (it monitors only the focus) and Verbatim does not; `*_leave_flood` asserts it. Recommendation: reject; returning takes a baseline.

### Extra 5: footers and status lines

- Gain: a flood above a fixed footer speaks the flood and the footer only when it changes; a footer redrawn unchanged says nothing (asserted by `*_footer_flood`, Dickson's "Ok" to the script on 2026-10-09).
- Cost in the base: none. The line diff aligns the footer with itself (in place) while the region's lines scroll above it; inserted lines are the flood. Today it takes `keep_footer`, `footer_below` (a399b4e, agent, to be confirmed) and `last_line_now`.
- Split reads: the console host's moving view is a problem today only because the read's end is taken before the text. With the end at the document's end, or one screen below the visible end, the footer is in the snapshot even when the view moves (measure 2).
- Failure modes: a footer that changes on every read during a flood is spoken as each read finds it, the same as NVDA's behaviour and as today ("a footer changed during a flood is heard, or not, as the reads happen to find it", phase6-design.md 2026-10-09).
- NVDA: whole-buffer diff, footer matched by character; captured in `*_footer_flood`'s module (lines 35 to 38 of that doc comment).
- Recommendation: adopt, as a consequence of the base, and delete the footer-specific rules.

### Extra 6: the alternate screen and the main-screen memory

- Gain: closing a pager or editor speaks only what followed ("closed" and the prompt), not the main screen again. Dickson, 2026-10-09 (coherence decision 5). Opening one speaks the whole screen within the flood limits, and a pager's one-line move speaks the new line (Dickson, 2026-10-07 and 2026-10-09).
- Cost: keep the main screen's memory when a switch to the alternate screen is seen, and diff against it when the switch back is seen. No extra calls.
- Split reads: none. The evidence of a switch comes from the read itself: the kept range failing across buffers (console host; Windows Terminal to be measured) and the text having no history above the screen while the memory had it. This replaces the current shape-based rule (a screen "replaced whole", 469f911, patched by 90f60c3), which mistook a flood scrolling a new console's first screen away for a full-screen program.
- Failure modes: a full-screen program opened on a new console that has no history yet gives no switch evidence; closing it re-speaks the main screen as NVDA does. A program drawing full screen on the main buffer (`less -X`) is just a redraw, compared by line.
- Pager scrolling: the line diff aligns a scrolled screen by content, so `alternate_scroll` and the row shift it corrects are not needed.
- NVDA: re-speaks the main screen or a fragment of it on close (`nv-con-pager.txt` 81; `nv-con-alt.txt`, last lines).
- Recommendation: adopt, triggered by switch evidence only.
- Holding keys: no interaction.

### Extra 7: rows not yet written (`Unwritten`)

- What it is: e2834a0 (agent, to be confirmed) does not count a blank line printed onto a row below the old screen's last line, since a row the program passed over cannot be told from one a line feed passed through. It carries out Dickson's coherence decision 1 (2026-10-09: rows a program passed over, such as a cleared screen's rows above a footer, are not lines) but narrows "every line counts" for `echo ""` on a screen not yet full.
- Proposed replacement, a fact rather than a shape: a blank row below the caret's row is not a line yet; a blank row above the caret is, since the cursor has passed it. A footer below the caret is a line because it is not blank. A program that prints `echo ""` moves the caret down, so that blank line counts, as Dickson's rule asks.
- Cost: the caret's row in the snapshot, located by content (the caret row's text read with the caret, found in the snapshot), which the base already reads.
- Split reads: the caret is read after the snapshot; when output moved it further down, more rows count as written, which is true by then. When the caret's row text is not found in the snapshot, every blank row at the end is treated as not yet written (today's behaviour).
- NVDA: does not count lines at all; blank lines are never spoken by either.
- Recommendation: adopt the caret rule and remove `Unwritten` (open question 4). The difference only matters for counts in floods.

### Extra 8: line-key behaviour

- Decided (Dickson, 2026-10-08 and 2026-10-09): the caret's line when the caret moves to another line; silence when it does not move; redraws elsewhere spoken as output.
- Cost in the base: none beyond the caret report each read already carries and Core's matching by read time. The outpost's `CaretWatch` for terminal keys, the watch checks before and after each read, and `key_owns_line` go.
- Split reads: the caret report and the snapshot are separate calls. The rule in "Keys" judges them per read; a caret event that comes first answers the key with the row text it read.
- Failure modes: a program that moves the caret before it finishes redrawing the caret's line answers the key with a half-drawn line; the rest of the redraw is then output. The same happens to NVDA when its poll catches the caret early.
- NVDA: polls, and speaks the caret's line at the timeout even when nothing moved.
- Recommendation: adopt as decided.
- Holding keys: with keys held, the caret before the key is read exactly and no read-time comparison is needed for it; the rest is unchanged.

### Extra 9: keys that remove text, and the wrapped Escape

- Gain: Backspace, Control+Backspace, Escape, Control+W and Control+U say what they removed (Dickson, 2026-10-08). The wrapped case: Escape on `echo` plus 108 "a"s and "xyz" across two rows says the whole command, not "ready>" (agent, a58a7fc; NVDA silent).
- Cost in the base: none extra. The diff works on logical lines, so a wrapped line's loss is one line's loss; Core matches it against the pending key. Today it needs `terminal::keys::wrapped_removal`, the terminal's previous screen and `screen_at` (ecf191c, which Dickson objected to on 2026-10-10), and the outpost's eight caret reports per node.
- Split reads: none; the loss is between two snapshots.
- Failure modes: a program that redraws the line in pieces (cut short, then the rest drawn again on the next row) gives a read showing a removal that is not final; the guard (the caret where the removed text began, on the same line) rejects most of these, and the next read's gain cancels nothing already spoken. Uncertain: whether a piecewise redraw of a wrapped line can pass the guard; the console host's write of Escape was measured as one clear 10 ms after the key (phase6-design.md, 2026-10-10).
- NVDA: Backspace speaks what was before the caret; Escape is silent.
- Recommendation: adopt Backspace and the rest as decided; for the wrapped Escape, keep it, since it costs nothing once lines are logical, but put it to Dickson (open question 5).
- Holding keys: with keys held, Backspace's text is read before the key; the facts are still needed for Escape and the bash keys unless every key passed to a terminal is held.

### Extra 10: typing matched to what the line gained

- Gain over NVDA: no double echo of typed commands, no password spoken when a clock ticks or asterisks show, short output not dropped. Each is a requirement in section 1.
- Cost: none in reads; Core's matching exists. The base adds only the read time to each fact.
- Split reads: none.
- Failure modes: typing the terminal never shows (a space at the end of a line, a character typed over ghost text) is echoed when the caret moves over exactly that text, as decided 2026-10-07; a prompt's own trailing space, indistinguishable from padding, is handled by `LineChange::uncertain` as today.
- Recommendation: adopt (it is what Verbatim does now).

### Extra 11: changed-word rule, spinners and half-rewritten lines

- Gain: "50%" rather than the whole line or a character fragment; a spinner silent; "51%" not "oading 51%".
- Cost: the memory keeps each line as last said (`Memory::said`) as well as last read. No extra calls.
- Split reads: a program that writes a rewrite in two writes can be read between them in any design; `said` covers it.
- Recommendation: adopt, as decided (Dickson, 2026-10-07 and 2026-10-08).

### Extra 12: the console host's WinEvents as change evidence

- What it is: 17538d0 (agent, never put to Dickson) also takes `EVENT_CONSOLE_UPDATE_REGION`, `_SIMPLE`, `_SCROLL` and `EVENT_CONSOLE_LAYOUT` as the console host's text changes, because UIA's text changes stopped reaching the outpost mid-write in 2 of 14 runs of a 12,000-line write (cause unknown). NVDA uses those events only for the legacy console (`winConsoleHandler.py` 84 to 88).
- Gain: the final state of a write is read even when UIA's events stop.
- Cost: many more change signals during a flood; coalescing makes them at most one extra read each time a read ends, and on-demand holding ignores them while Core is full.
- Split reads: none; they are only evidence.
- Failure modes: none known, beyond more reads.
- Recommendation: keep as evidence (never as text), and find the cause of UIA's events stopping, since it may be on Verbatim's side (for example event coalescing, adopted 2026-10-07, or the subscription's handling; uncertain). Measure 5. Open question 3.

### Extra 13: reading on demand

- Decided (Dickson, 2026-10-07). With the base read, a read after a long hold returns the whole output since the last read in one snapshot, so the count is exact without the anchor machinery.
- Cost: one large read per group during a flood, up to the whole history (measure 3), against continuous reads while Core is full.
- Recommendation: keep as decided, with two states.

### Extra 14: short output not taken for typing

- Recorded in `docs/parity.md`; NVDA drops it. Verbatim's typing matching makes NVDA's heuristic unnecessary.
- Recommendation: keep, and confirm with Dickson (open question 11).

## 5. Migration

### Measurements first (no behaviour change)

Each with a scratch probe in the style of earlier measurements, not committed:

1. Atomicity of one `GetText`: a script writes numbered lines with a checksum in each as fast as it can; a probe reads a range to the end repeatedly and checks that every snapshot is a run of whole, consecutive lines (a torn line or a gap inside one snapshot refutes the assumption). Both terminals.
2. Where each terminal's document ends: on a new window, after 10 lines, and with the history full; and whether rows below the visible range exist and are blank.
3. Cost of a snapshot from a kept range to the end, at 30, 1,000 and 9,000 lines: wall time, and how long the terminal's own output is held up.
4. The alternate screen: does a main-buffer range fail on the alternate buffer in Windows Terminal as in the console host, and does `cls` clear the history in each.
5. The console host's UIA text changes stopping: reproduce with event coalescing on and off, and with NVDA running alone on the same write.

### What is removed or replaced

In `crates/verbatim-uia-rops/src/terminal.rs` (1,007 lines): the anchor search (`ScreenAnchor`, `Sought`, `SEARCH_MATCHES`, `FindText` with padding), the walks to the text's end and their agreement check, `is_settled`, `view_moved`, the exact-shift binary search, the head rows read, the document row count. Replaced by a program of about five instructions: clone the kept range (or the visible start), extend it to the end, `GetText`, the caret and its row, the visible start to keep; and a whole-document `GetText` as a second entry point.

In `crates/verbatim-outpost/src/terminal.rs` (1,133 lines): `trusted`, `footer_below`, `keep_below`, `without_last`, `after_scroll`, `last_line_now`, `overflowed` (rewritten from the snapshot), `scroll`, `keep_footer`, `main_screen`'s shape rule (replaced by switch evidence), `screen_at`, `remember_at`, `previous`, `memory_since_ms`, `matches_padding`; `Memory` loses `top_row`, `next_row`, `scrolled`, `unwritten`, `rows`. `Found::Unsettled` goes.

`crates/verbatim-outpost/src/terminal/keys.rs`: removed.

`crates/verbatim-outpost/src/terminal/screen.rs`: the diff, `line_change` and the spinner rule move, rewritten from prose, to the platform-neutral crate; `line_rows`, `row_width`, `unwritten_rows`, `alternate_scroll` and `Unwritten` go.

`crates/verbatim-outpost/src/terminal/reading.rs`: the `Owed` state and `ReplyUnsettled` event go.

`crates/verbatim-outpost/src/outpost/worker.rs`: `wrapped_removal` (1157 to 1198), the terminal key watch checks, and the owed-answer path in `terminal_event` (1210 onward). The console WinEvents merge stays or goes by open question 3.

Core: `key_owns_line` replaced by the per-read rule; terminal keys answered from the facts; the reducer's intake ordered by time. Core's eight-report caret history (`CARET_HISTORY`, `state.rs` 187) and the outpost's eight per node (`REMEMBERED_CARETS`, `text/mod.rs` 60) become unnecessary for terminals; whether text fields can drop them too is chain A's question.

Tests: `terminal/tests.rs` (950 lines) and Core's `tests/terminal.rs` are rewritten around snapshots; the screen diff's table of cases is kept and extended (identical lines with and without a position hint, a footer with a moving view, a wrapped line losing its tail).

Documentation: `docs/crates/verbatim-outpost.md` and `verbatim-core.md` "Terminals", `verbatim-uia-rops.md` layer 3, `docs/parity.md` "New terminal output", and the crate file for the crate that gains the diff.

### Scenario expectations

The design keeps the decided speech, so most scenarios should pass unchanged. Checked against the captures and the scenario module docs:

- Unchanged: `*_commands`, `*_spoken_password`, `*_editing`, `*_short_output`, `*_progress`, `*_flood`, `conhost_wrapped_flood`, `*_same_flood`, `*_raised_flood`, `*_redraw_limit`, `*_footer_flood`, `conhost_footer_overflow`, `*_history_flood` and its during-group twin, `*_scrollback_overflow` and its twin, `*_control_flood`, `*_shift_flood`, `*_line_key_flood`, `*_up_typing`, `*_long_lines`, `*_full_screen`, `*_pager`, `*_marker_list`, `*_redrawn_list`, `*_review_grid`, `*_review_output`, `*_two_windows`, `windows_terminal_tabs`, `windows_terminal_close_tab`, `*_leave_flood`.
- `*_typing` step 5 (the wrapped Escape): unchanged if Dickson keeps the rule; otherwise silence, as NVDA (`docs/parity.md`, captured 2026-10-09).
- Should become stable rather than flaky: `conhost_typing` step 5 (failed about one run in four before ecf191c; the new attribution does not depend on which read the outpost had before the request); the echo-before-output order in `conhost_typing` and `conhost_flood` (time-ordered intake).
- Scripts shaped around today's timing, to revisit once the base is in: `conhost_footer_overflow`'s line-at-a-time start (a399b4e) and `windows_terminal_scrollback_overflow`'s split write (8de2f28). Whether a single 12,000-line write is read before or after it leaves the history still depends on when the terminal raises its first event, so both may still need their split; this is a measurement, not a guess.
- New: output written just after a Windows Terminal terminal takes the focus (roadmap), which needs one interactive shell again; an identical-line flood that overflows the history, asserting the documented limitation; a blank line printed onto an empty screen counted in a flood (extra 7).

### Order of work

Each step is verifiable on its own before the next.

1. Measurements 1 to 5. Verify: numbers written into phase6-design.md; Dickson decides open questions 1 to 3 from them.
2. Time-ordered intake in Core's reducer. Verify: a reducer unit test with both channels loaded; `conhost_typing` and `conhost_flood` ten runs each.
3. The pure line diff in the platform-neutral crate, written from this document and `docs/parity.md`, with the existing table of cases ported as test data and new cases. Not wired. Verify: `cargo xtask ci`, including the neutral-crate dependency check.
4. The new read program in `verbatim-uia-rops`, both paths, with mockapp tests of exact call counts and of a scripted terminal that writes between calls. Not wired. Verify: mockapp.
5. Switch the outpost to the new read and diff; delete the trust rules, the anchor search and `Owed`. Key attribution unchanged for now. Verify: the flood, footer, overflow, full-screen, pager and list scenarios in both terminals, ten runs of each flood.
6. Caret-line facts with read times; Core attributes terminal keys; remove the terminal watch, `keys.rs`, `screen_at` and `key_owns_line`. Verify: typing, editing, lists, key-timing and up-typing scenarios, ten runs of `conhost_typing`.
7. The caret rule in place of `Unwritten`, if agreed. Verify: footer scenarios and the new blank-line flood.
8. Main-screen memory by switch evidence. Verify: `*_full_screen`, `*_pager`, `*_redraw_limit`, and a new-console flood (`conhost_shift_flood`, which found 90f60c3's case).
9. Focus ordering (subscribe, then baseline) and its new scenario. Verify: the new scenario ten runs.
10. Documentation pass: crate docs, parity, roadmap; the full suite once, since steps 5 and 6 change behaviour every terminal scenario sees.

## 6. Open questions for Dickson

1. Replace the anchor search (`FindText`, 20 matches, your decision of 2026-10-07) and the row counting with one snapshot from the kept range to the end of the text, falling back to one whole-document snapshot? Recommended: yes, if measurements 1 and 3 hold (one `GetText` is atomic, and a full-history snapshot costs no more than a few `FindText` calls). Every trust rule in audit chain C exists because counts come from several calls; a count taken from one snapshot needs none.
2. End each read at the document's end, or at the visible end plus one screen? Recommended: the document's end if measurement 2 shows both terminals end it at the last written row; otherwise the visible end plus one screen. Either keeps a footer the console host redraws lower inside the snapshot, which removes `footer_below`.
3. Keep the console host's WinEvents as change evidence (17538d0, not put to you before)? Recommended: keep, as evidence only, and investigate why UIA's text changes stopped (measurement 5). Dropping them risks a silent terminal mid-flood with nothing to recover from; keeping them costs only coalesced extra reads.
4. Replace `Unwritten` (e2834a0, to be confirmed) with "a blank row below the caret is not a line yet"? Recommended: yes. It follows your coherence decision 1 by a fact the read already has, and it counts `echo ""` as your 2026-10-07 rule asks.
5. The wrapped Escape: keep Verbatim saying the whole command removed (agent extension of your 2026-10-08 Escape decision; NVDA silent)? Recommended: keep. In this design it is not a separate rule: lines are logical, so a wrapped line's loss is one line's loss, and it needs no kept screens.
6. Attribute keys in Core by read time (facts from the outpost, Core taking its inputs in time order), rather than the outpost keeping screens and carets from before each key? Recommended: yes, now, for terminals; decide on holding keys separately when its latency is measured. Both work with this design; holding only makes the state before a key exact.
7. Accept that a flood of identical lines that also overflows the history cannot be counted (said as "skipped lines", or silent if nothing differs)? Recommended: accept and document it. No method can tell such a history's lines apart from text, NVDA included.
8. Keep the main-screen memory, triggered only by evidence of a buffer switch? Recommended: yes. It keeps your coherence decision 5 and removes the shape rule that misfired (90f60c3); the cost is that a full-screen program opened on a console with no history yet re-speaks the main screen on close, as NVDA does.
9. Keep on-demand reading with two states (Live, Held), dropping Owed? Recommended: yes. Owed exists only because a read could be disturbed; a snapshot cannot be.
10. Drop the ten-second bound on a caret key's watch? Recommended: yes. It is a fixed timeout; the next key or a focus change already ends a watch, and the evidence rule never needs a deadline.
11. Confirm that one- and two-character output ("y", "ok") is spoken, unlike NVDA? Recommended: confirm. Typing matching makes NVDA's "probably typed" guess unnecessary, and the guess drops real output.
12. Put the diff in a module of `verbatim-text` rather than a new platform-neutral crate? Recommended: `verbatim-text`, which already has the word and grapheme segmentation the changed-word rule uses, avoiding a new crate and the hakari changes it brings. Either way it is original code written from the parity docs, not a translation of `diffHandler.py`.
