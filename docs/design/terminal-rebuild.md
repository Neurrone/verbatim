# Design study: a small terminal reader, from NVDA's loop

Written against main 23264d4b, NVDA pinned at 732d1ce2f. A study for the rebuild planned as [M6.4](../roadmap.md#m64--terminal-reading-rebuild-and-residuals); nothing is built from it yet. Authorized by Dickson on 2026-11-19.

## Rulings so far (Dickson, 2026-11-19)

- The rebuild is a phase of its own, M6.4, not built now.
- Every existing terminal feature is kept unless Dickson authorizes its removal. Named as kept: F2 (holding reads while speech waits), F5 (an unfinished word held), F7 (volatile rows), F8 (keys owning changes, lists and colours: many terminal programs need it), F11 and F12 (review over the terminal's text and moving by command). Each kept feature that today rests on finding rows again by their text (F3's rows) is to be redesigned simpler in the phase, keeping what it says: F8 by comparing the visible screen before and after a key row by row at the same places, F11 by reading live text with the position counted from the document's end and found again by its line's text only when a review command runs, F12 by a live search at the command; these are directions, each checked against recordings in the phase.
- The study's recommendations below to drop a feature (F3, F4) or change what is said (questions 2 and 9) are proposals awaiting Dickson's authorization; until he gives it, the feature stays.

## Decisions it changes, and the fact that changed

- ADR 0003, "What Verbatim keeps beyond NVDA", lists "the terminal model" among existing behaviour kept, and `books/dev/src/explanation/design/terminal-model.md` was accepted on 2026-10-25 under five principles. This study proposes replacing the model with NVDA's loop, keeping only the Verbatim-only features Dickson chooses below.
- The fact that changed: since build 3's implementation review the terminal reading has had five defects (6a9e55dc, 75b5fb5c, 33b0523c, then T1 and T2), the third-fix rule has stopped it, and Dickson's instruction of 2026-10-26 says a further failure of the closed rules means revisiting the whole design for a simpler model.
- ADR 0003's terminal decisions all hold, unchanged:
  - "Every call into an application is made for a request from Core."
  - "An outpost tells Core of every change of a terminal and keeps no flag of its own (Dickson, 2026-11-16)." The flag in this design (`dirty`) is Core's, as ADR 0003's "a terminal's 'told, not yet read', in Core alone" requires.
  - "Requests from ... a terminal's reads for its output stay outside the event queue, answered in the inbox and holding no key; a terminal read's speech joins the event queue in order (Dickson, 2026-11-16)."
- Process and thread layout: unchanged. Core judges keys and focus; the outpost only reads.
- Roadmap: M6.4 ("Terminal residuals") is replaced by this build if approved; its generated test of the line comparison, Pi's regular mode in the marks and status bar scenarios, and the simulator's line feed lapse with the row model.

## NVDA's loop, at the pinned source

- `NVDAObjects/behaviors.py` 519 to 525: a terminal starts monitoring at `event_gainFocus` and stops at `event_loseFocus`. `api.py` 93 to 94: every `setFocusObject` runs `loseFocus` on the old focus first, so every executed focus starts a new monitor.
- `_monitor` (468 to 505): `oldText` is read once, silently (470); a failure leaves it empty (473). Then: wait for a text change (476), clear the event (485), read all the text (488), diff against `oldText` (490), drop a result that is one line of one character (491 to 495), queue the lines' speech in the event queue (497 to 501), set `oldText` to the new text whether or not anything was spoken (503). An exception logs and waits for the next change, `oldText` unchanged (504 to 505). With "report dynamic content changes" off, nothing is diffed but `oldText` still advances (489, 503).
- A change while a read runs sets the event again, so the loop reads once more after it (420 to 424, 485).
- `diffHandler.py` 39 to 48: the character diff's inserted runs, split into lines, blank and white-space lines dropped. Windows Terminal and the console host at the formatted API level read the whole document (`winConsoleUIA.py` 396, 408 to 414).
- Typing (`behaviors.py` 540 to 611): with "speak passwords" off, typed characters are queued and spoken at the terminal's next text change (572 to 592); Enter, Tab, Control+C, Control+D and Control+Pause empty the queue (595 to 611); one new line shorter than the typed word plus one, or than three characters, is dropped (559 to 569).
- A key's cut is itself a function queued in the event queue (`inputCore.py` 583 to 605): lines queued before it are cancelled, lines queued after it are spoken.
- A caret key in a terminal is NVDA's editable-text caret script, its wait 100 ms in the console host and three times that in Windows Terminal (`winConsoleUIA.py` 374 to 376, 470); the diff plays no part in answering it.
- NVDA keeps nothing of a terminal between focuses, has no flood limit, and has no terminal review, marks or footer of its own.

## The inventory: what today's code does, and where it goes

Today's product code, counted as lines before the first test module: Core 4,950 (`terminal.rs`, `terminal/`, `terminal_model.rs`, `terminal_model/review.rs`, `keys/terminal.rs`), rules 5,630 (`terminal_reading.rs`, `terminal_rows.rs`, `terminal_rows/align.rs`), outpost and remote operations 1,390 (`verbatim-outpost/src/terminal.rs`, `verbatim-uia-rops` snapshot and find): about 11,970. Tests: about 17,000, of which `verbatim-term-sim` is 6,900.

"Core" below means the core design; a feature code (F1 to F14) means it stays only if Dickson keeps that feature; "NVDA" means the item goes and NVDA's behaviour takes its place.

- Change tells read at once, one read out, once more after it (rule 1): Core. Tested by watch cells A to C.
- The console host's four console WinEvents as change signals; notifications ignored: Core, unchanged in the outpost and listener.
- Baseline at the focus, silent; the focus says "terminal" and the caret's line (rule 15): Core; the focus speech stays the focus machinery's.
- Output written while the focus was elsewhere never heard: Core (every commit baselines, as NVDA).
- Memory kept across focus changes, 16 models, generations: F4.
- `Follow` tracked by the terminal (resend at due time, refused, hung): goes. `Follow` is answered as for every other focus, whose ending "changes nothing in Core" (`focus/follow.rs` 11 to 13).
- Hang: `Stalled`, `WaitingForWindow`, `recovered`: goes. The recovery's re-take commits, which baselines anew (as today: `execute.rs` 697 commits, and `committed` baselines).
- The read: anchored start, history block, whole-document and history-block scopes, start refused (rules 2, 6): replaced by the window read (Core); exact counts are F3.
- Rows without padding, a wrapped line one line (rule 3): Core (the outpost's `unpadded_utf16` kept).
- Positions, alignment, lost reads, screens, the aside screen, lingering rows, reads with no text, starting again (rules 4 to 11): NVDA's diff, plus F14 for a new screen; the rest go with F3.
- New text spoken, blank lines silent, lost text silent (rule 12): Core (the diff's inserted runs).
- Unfinished last word held (rule 13): F5. Reprinted input not spoken again (rule 14): NVDA (the diff matches the reprint against the old line; capture needed).
- Rewritten rows from the first changed word, a spinner silent (rule 16): F6. Volatile rows (rule 17): F7. The quiet-period timer and `TerminalDue`: only with F5 or F7.
- Keys' windows, caret-line answers, rows the keys own, suggestions, colours (rules 18, 19, 21, 22, 23), keys deferred behind a held stroke: F8; without it a caret key runs the ordinary caret key entry with ordinary text reads, as NVDA's caret script.
- Typing echo when the terminal shows it (rule 20): F9; without it NVDA's queue, spoken at the next change.
- Review over Core's snapshot, history reads, the upward search, review follows caret only on a key (rule 24): F11; without it the generic review path in `review_text.rs` on live ranges, as for any text field.
- Moving by command (marks): F12. Report status bar's footer: F13; without it NVDA's "No status line found".
- Flood policy groups, "skipped N lines", "about", "more than", line cut: F1. The 10 MB bound on waiting output: Core (it bounds memory).
- Release read, silent read after a cut, waited read, `deciding`, `release_waiting`: F2 (in a derived form) or go.
- Output of a read begun before a cut not spoken (`cut_at_ms`): goes; the queue's arrival order decides, as NVDA's queued cut does.
- Report new output (Verbatim+5), speak passwords, the console host's keyboard layout, text as it is: Core, unchanged.
- An identical read not given to the rules: goes; the diff of identical text is empty.
- A read's speech under its key's trace (`Effect::ForKey`): Core keeps the tell's trace only; latency attribution of output to a key goes (measurement tools read the typing echo, which keeps its key).
- The flight recorder's `Observed` snapshot of each read: Core, carrying the window read.

Every surviving item gets a Core unit test named for its cell or rule; the end-to-end terminal scenarios (17 families, both terminals) keep running, their expectations rewritten from NVDA captures wherever an item went to NVDA.

## The core design

### Who does what

- **The outpost** reads, at Core's request, the window: the rows from two screens above the document's end to the end, the screen's height, and the caret's row and column, in one remote operation, padding removed. It keeps nothing between reads: no start anchor, no scope.
- **Core** owns everything else, in the reducer:
  - the focused terminal's watch: the one previous text (NVDA's `oldText`), the read out, and whether a change was told since it began (NVDA's event);
  - the diff (`verbatim-text`, written from `docs/parity/terminals.md`, not translated): inserted runs as lines, blank and white-space lines dropped, NVDA's one-character and short-line drops;
  - the typed characters queued (NVDA's `_queuedChars`);
  - the pacing: lines handed to speech a few at a time by index marks, bounded at 10 MB.
- **The event queue** carries every word the terminal says: each read's lines, and each echo, join as a `TerminalSpeech` entry at the answer's or tell's arrival, its fence set by the queue (`set_fence`), spoken at its own turn.

### The state

The watch is a field of the committed focus's record (`FocusContext`), present when the focus is a terminal:

```rust
struct TerminalFocus { watch: Watch, typed: String, pacing: Pacing }
enum Watch {
    Baselining { token: u64, dirty: bool },
    Live { old: Text, read: Read },
}
enum Read { Idle { dirty: bool }, Out { token: u64, dirty: bool } }
```

`Live` with `Idle { dirty: true }` lasts only from an input to the settle that follows it (under F2 it may last while held). So the watch has five lasting states: Baselining clean, Baselining dirty, Idle, Out clean, Out dirty.

### The watch's state table

Columns: A a change told for this terminal; B its read answered `Result(Ok(text))`; C answered any other way (`Result(Err(_))`, `Refused`, `NotThere`, `NotResponding`, `ReaderEnded`, `NoLongerAwaited(_)`), each matched by name; D an answer whose token is not the watch's; E settle, after every input; F any commit.

| State | A told | B text | C other | D stale | E settle | F commit |
|---|---|---|---|---|---|---|
| Baselining clean | Baselining dirty | Live, old = text, Idle clean; no speech | Live, old empty, Idle clean (NVDA 473) | dropped | nothing | record replaced |
| Baselining dirty | stays | Live, old = text, Idle dirty | Live, old empty, Idle dirty | dropped | nothing | record replaced |
| Idle | Idle dirty | n/a, no token | n/a | dropped | dirty: send a read, Out clean | record replaced |
| Out clean | Out dirty | diff; its lines join as an entry; old = text; Idle clean | Idle clean, old kept (NVDA 504) | dropped | nothing | record replaced, the answer later stale |
| Out dirty | stays | as Out clean, then Idle dirty | Idle dirty | dropped | nothing | as above |

- A read answered while the terminal hangs, then refused unsent as hung, ends in C and sends nothing more: `dirty` is cleared when a read is sent, so there is no loop. The recovery set's re-take commits and baselines.
- "Report new output" off: column B still sets `old`, and the entry carries only the echo.
- The baseline is sent by the commit that builds the record, after the commit's `Follow`, as today's order class requires.

### Typing (NVDA's queue)

- A typed character reaching Core while "speak passwords" is off is appended to `typed`; with it on, it is echoed at once, as today.
- A change tell with `typed` not empty joins an echo entry at once, its fence the queue's at the tell's arrival, and empties `typed` (NVDA 591 to 592).
- Enter, Tab, Control+C, Control+D, Control+Break empty `typed` (NVDA 595 to 611).
- A read's lines pass NVDA's two drops; lines passing them empty the typed-word buffer and `typed` (NVDA 568 to 569).

### The pacing's state table

`Pacing` is `Idle` or `Speaking { left, waiting, ahead, sounding }`; `left` counts the group's lines still to hand on (`None` without F1, when it is unbounded but for the 10 MB bound). Columns: G an entry's turn with lines; H an opening mark reached; I the closing mark of the last line handed reached; J a speech cut, or an utterance of its own cancelled; K "report new output" turned off; L a commit of the same terminal not left; M any other commit.

| State | G turn | H open mark | I last mark | J cut | K off | L same | M other |
|---|---|---|---|---|---|---|---|
| Idle | Speaking, hand on two | ignored | ignored | stays | stays | carried over | dropped |
| Speaking, lines left | append, bound, hand on | hand on | hand on | Idle | waiting dropped | carried over | dropped |
| Speaking, group ended | append | hand on what belongs | look (F1) or hand on; Idle when nothing waits | Idle | Idle once handed lines end | carried over | dropped |

- At G, an entry whose fence a later cut has passed (`Arrivals`, `keys/entry.rs` 49 to 77) speaks nothing: NVDA's queued cut cancels lines queued before it. This reads the queue's own fact; the terminal keeps no cut time.
- Every utterance handed on ends at one of reached, cancelled (fenced off included: `verbatim-speech/src/manager.rs` 1103 to 1117) or failed, so no pacing state waits without an answer.

### The structural rule

So that a change to the event queue, key fences and cuts, or reader replacement cannot create a state the terminal never listed:

1. **One owner per fact, none copied.** The terminal stores no focus, fence, cut time, speech-spoken flag or `Follow` token. Which terminal is focused is the record the watch lives in; the fence is the entry's, set by the queue; whether a cut passed it is asked of `Arrivals` at the turn.
2. **The watch lives and dies with the committed focus's record.** `FocusContext` has one constructor (`reduce.rs` 516 to 534); its new field must be given there, so a new kind of commit cannot skip it, and every commit (a re-take, a re-executed focus, a replacement) replaces the watch. The pacing's carry-over for the same node not left is written beside `caret`'s in that constructor, the only place. Today's second reset path (`terminal::focus_moved`) goes.
3. **Reading never waits on speech, and speech never waits on reading.** The read is sent only at settle, from `Live` with `Idle { dirty: true }` (and, under F2, not held, held being a function of the pacing, not a stored flag). The pacing waits only on its own marks. No terminal state waits for a queue entry's turn.
4. **Answers are matched exhaustively.** `Outcome`, `CancelReason` and `TextFailure` are matched by name, with no wildcard, in the watch; the queue's per-entry policies in `entries.rs` that now end in `_ =>` (`exempt_from_foreground` 525 to 624, `kept_by` 690 to 700, `merges` 861 to 884) name `TerminalSpeech`, so a new outcome, failure or queue policy fails to compile until the terminal's cell is decided.
5. **Terminal speech is emitted at one place.** Only a `TerminalSpeech` entry's turn, or a `Mark` entry's turn handing on paced lines, emits terminal speech, each through `cx.emit` under its own entry's fence. `speak_read` and `take_waiting_before` go: no other entry's turn speaks the terminal.

### The five defects, replayed

- **T1** (`conhost_flood`, the listener dies while the release read is out, the re-take commits): today `release_waiting` (`reading.rs` 284) survives the new `Baselining`, `mark_reached` sets `deciding`, and nothing clears it, so the terminal is silent until a key. Here there is no release read and no flag; the look runs at its mark whatever the reading does (rule 3), and the commit replaces the watch (rule 2). Speech: the waiting lines are decided and spoken; output from the dropped read's start to the new baseline is not heard, as NVDA's new monitor reads `oldText` silently (470); later output is read live.
- **T2** (Windows Terminal: a caret key's final read out with key 8, "z" typed as key 9, the rules give "z" to that read): today `speak_read` emits it under the front's fence 8 and the speech manager drops it. Here a caret key reads no terminal text; "z" is queued in `typed`, the terminal's change tell joins an echo entry with the queue's fence 9, and its turn says "z". NVDA: "z" at the next text change (591 to 592). Capture C-T2 of the stopped design confirms the timing.
- **6a9e55dc** (a modifier's cut left later arrivals under an older fence): the class needs terminal speech stamped by something other than the queue's `Arrivals::fence`. Here every terminal entry's fence is set by `set_fence` at arrival, and the turn asks `Arrivals` whether a cut passed it; the terminal has no fence or cut time of its own to fall out of step.
- **75b5fb5c** (a reply type that could not hold "no text"): the window read decodes through `text_request::decode_text` to `Result<_, TextFailure>`, every failure matched by name into column C; the entry reads, history reads and searches it fixed are gone unless F11 or F12 stays.
- **33b0523c** (a review command looping on Core's memory answering "anchor lost" at the caret): with F11 dropped, no review command is answered from Core's memory; the generic review path asks the outpost on live ranges, and the loop's code does not exist.

### Limits of the core

- Output that scrolls more than two screens past between two reads is not heard and not counted (F3 restores counts). Reads run back to back, so this needs a burst faster than one read's round trip.
- Output between a dropped read and a new baseline is not heard (NVDA alike).
- A read out when the focus moves is dropped; NVDA's old monitor still queues it (`_monitor` checks `_keepMonitoring` only before reading). A deliberate difference, since its lines would carry the left focus's validity and be dropped anyway.
- The window from the document's end assumes the screen ends there in both terminals, as today's read counts its start up from the end; a `verbatim-event-record` recording of each terminal confirms it before the build.

## The Verbatim-only features

Each: what it adds, NVDA's behaviour, one sequence. Speech marked "capture" needs an NVDA capture at the pinned build before it becomes an expectation.

### F1. Flood groups and skip counts

Adds `left` and the look at a group's end: 2 pacing cells, about 180 product lines. NVDA speaks every line until a key (parity, "The backlog"). Sequence, the console host, `1..400 | % { "line $_" }`: with F1, "line 1" to "line 30", "skipped N lines", the newest 30, N counting lines the reads saw, "skipped lines" with no number when a read found nothing of the last; without F1, every line the reads saw, in order, until a key. Needed in some form: without it only the 10 MB bound limits waiting output.

### F2. Holding reads during a flood

Adds "held" as a function of the pacing (waiting lines at least "Lines spoken in full") read at settle: no state, one cell, about 40 lines. NVDA holds nothing. Speech is the same intent; the saving is reads (below, "Costs"). Without F3, reads held through a long flood see only the last two screens at release, so the count says "skipped 30 lines" for 10,000. Recommended only if the measurement shows a budget missed, per the design principles.

### F3. Exact counts: the anchored read and the row model

Adds the outpost's start anchors, the history block, whole-document reads, rules 4 to 11 and the aside screen: about 3,500 product lines and 4,000 test lines, and the states that carry `named` and scopes. NVDA reads the whole document every change. Sequence, `1..10000` in the console host over a full history: with F3, today's "skipped 9940 lines" (about, when counted from text); without, "skipped lines" and the newest 30.

### F4. Memory kept across focus changes

Adds 16 models with generations: about 250 lines. NVDA: nothing kept (each monitor's `oldText` is local). Matters only with F11 or F12: review three lines up, Alt+Tab away and back, Numpad 8: with F4 and F11, that line; without, the caret's line, as NVDA's review follows the focus.

### F5. An unfinished word held

Adds the quiet-period timer, `TerminalDue`, per-line held text: one timer state, about 250 lines. NVDA speaks each inserted run as it comes (`_monitor` 490 to 501). Sequence, a reply streamed as "The qu", "ick brown f", "ox": with F5, "The", "quick brown", "fox"; without, "The qu", "ick brown f", "ox" (capture: depends on read timing).

### F6. A rewritten line spoken from its first changed word

Adds widening each inserted run on a line that had text to the start of its word: no state, about 70 lines. NVDA speaks the inserted characters, and drops one character on one line (491). Sequence, vim's ruler "1,1 Top" to "2,1 Top": with F6, "2,1 Top" (parity, "Redrawn rows"); without, nothing, the change being one character.

### F7. Volatile rows

Adds per-line change history and the quiet-period timer; needs row identity, so in practice F3's rows: about 500 lines. NVDA speaks every change its drops let through. Sequence, "Working... 1s" ticking to 34s and stopping: with F7, "Working... 1s", then "Working... 34s" (parity); without, the first line, then only ticks that insert at least three characters (capture).

### F8. Keys owning changes, lists, suggestions, colours

Adds rules 18, 19, 21, 22 and 23, key windows, colour reads, the terminal caret key's full reads and deferred keys: about 2,500 product lines, and F3's rows. NVDA answers a caret key with its caret script and speaks other changes through the diff. Sequence, `windows_terminal_marker_list`, Down Arrow in a list redrawn with the caret below it: with F8, "> banana"; without, NVDA's captured "blank" for each key (parity, "Line keys and lists a program draws").

### F9. Typing echoed only as the terminal shows it

Adds a check at the read that the inserted text at the caret's line ends with the queued characters, echoing them only then and dropping them otherwise: no state, about 80 lines. NVDA speaks its queue at any text change. Sequence, a password prompt that shows nothing, with a clock ticking on the screen, "h", "u", "n" typed: with F9, nothing but the clock's output; without, "h", "u", "n" at the clock's ticks (parity, "Typing").

### F10. (folded) Reprinted input is left to the diff; no feature.

### F11. Review over Core's snapshot

Adds `terminal_model/review.rs`, history reads, the review answers from memory, the follow rule: about 1,400 lines with the remote search. NVDA reviews live ranges and follows the caret. Sequence, review a line above the caret, a script prints two lines, Numpad 8: with F11, the reviewed line; without, the line the review cursor is on after following the caret, as NVDA (parity, "The review cursor while output is written"; the `*_review_grid` scenarios assert NVDA's).

### F12. Moving by command (marks)

Adds marks, their search and Verbatim+Alt+Up and Down Arrow: about 400 lines, and F11. NVDA has no such command. Sequence, two commands entered, Verbatim+Alt+Up Arrow: with F12, the earlier command's line; without, nothing. Could be rebuilt later on anchors the outpost holds.

### F13. Report status bar's footer

Adds one fresh window read and the footer rule (lines below the caret's line, else the last line with text): no state, about 100 lines. NVDA by its source finds no status line in either terminal and says "No status line found" (parity, uncaptured). Sequence, vim, Verbatim+End: with F13, vim's last line; without, "No status line found".

### F14. A new screen speaks its screen

Adds: a read sharing no line with the last speaks only its screen's rows: no state, about 20 lines. NVDA's whole-document diff speaks what it inserted. Sequence, `less` closed over a long history: with F14, the shell's screen (NVDA speaks the shell's screen, `nv-con-pager.txt`); without, two screens of the shell's lines, through F1.

## Size

| Part | Product lines | Test lines |
|---|---|---|
| Today | about 11,970 | about 17,000 |
| Core design | about 1,270 | about 1,400 |
| F1 | 180 | 250 |
| F2 | 40 | 150 |
| F3 | 3,500 | 4,000 |
| F4 | 250 | 150 |
| F5 | 250 | 250 |
| F6 | 70 | 120 |
| F7 | 500 (with F3) | 400 |
| F8 | 2,500 (with F3) | 2,500 |
| F9 | 80 | 150 |
| F11 | 1,400 | 700 |
| F12 | 400 (with F11) | 300 |
| F13 | 100 | 100 |
| F14 | 20 | 60 |

The core's 1,270: the diff and NVDA's drops 250 (`verbatim-text`), the watch and typing 280, the pacing 380 (today's, without the hold), the window read 320 (outpost and remote operation), and 40 in shared code (the record's field, `Arrivals`' question, the named policy arms). With the recommended set (F1, F6, F9, F13, F14) it is about 1,720 product lines. The end-to-end expectations that move to NVDA's behaviour need captures: typing, flood, marker list, pager, progress, long lines, review grid, status bar, in both terminals.

## Questions for Dickson, in one batch

1. **Replace the terminal model with NVDA's loop over a window read** (the decisions quoted at the top)? Recommended: yes.
2. **What is read:** the last two screens from the document's end (recommended); NVDA's whole document, 25 to 31 ms of `GetText` with the console host's writer waiting on every change at a full history (`performance.md`, "A full-screen program's exit"); or today's anchored read (F3).
3. **Flood groups (F1):** keep, recommended; some bound is needed for memory.
4. **Holding reads (F2):** drop now, recommended; add only if the measurement below misses the wake-up budget, and then with F3.
5. **Keys and lists (F8):** drop to NVDA's caret scripts and diff, recommended; it is the largest Verbatim-only part and needs F3. fzf, tig and Edit's lists then speak as NVDA does.
6. **Typing (F9):** keep, recommended, so a password is never spoken on a clock's tick; or NVDA's queue.
7. **Streaming refinements:** keep F6 (cheap, no state), drop F5 and F7 (a timer and row history), recommended.
8. **Review, marks and memory (F11, F12, F4):** drop, the generic review path doing NVDA's review, recommended; marks proposed again later if wanted. **Status bar (F13):** keep, recommended. **New screen (F14):** keep, recommended.
9. **Two edge differences:** a read that began before a key and answered after it is spoken, as NVDA's queue order gives (today silent); a read out when the focus moves is dropped (NVDA speaks it). Recommended: both as stated.

## Costs per output burst

- **Calls into the application:** one remote operation per read, as today's ordinary read, over two screens (today's ordinary read: 0.45 to 1.06 ms of snapshot, 1.6 to 3.3 KB). No whole-document read ever (today's worst case 1.1 million UTF-16 units, 25 to 31 ms with the writer waiting). Caret keys read the caret's line by ordinary text reads instead of two full terminal reads. Without F2, reads run back to back through a flood: the evidence is 1,911 reads for the console host's 10,000-line flood with holding (`performance.md`, "A terminal's reads, live") and 2,800 before holding (terminal model, rule 1's reason), so expect about 1.5 times today's reads during a flood, and the same outside floods.
- **Wake-ups:** one per read in the outpost and in Core, as today; none for the quiet-period timer unless F5 or F7; a once-a-second clock costs its reads only.
- **Processor time:** Core's diff over at most two screens, expected below today's 101 to 116 µs median per live read, with no 2 ms outliers from whole-document alignment.
- **Typing echo:** spoken at the change tell, with no read before it, so no slower than today's 4.1 ms (console host) and 21.4 ms (Windows Terminal) medians.
- **The measurement that confirms them:** `books/dev/src/how-to/measuring.md`'s procedure over `conhost_flood`, `windows_terminal_flood`, the 30-second clock and `measure_keys_conhost` and `measure_keys_windows_terminal`, at most 20 runs each: reads, messages, reducer time per read at P50 and P99, wake-ups of all processes and echo latency, against the figures cited above. F2 is reconsidered only on its result.
