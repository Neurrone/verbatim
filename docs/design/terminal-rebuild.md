# Design study: a small terminal reader, from NVDA's loop

Checked against main 368d3b5b, NVDA pinned at 732d1ce2f. The study for the rebuild planned as [M6.4](../roadmap.md#m64--terminal-reading-rebuild-and-residuals), which runs as build 9 of [the simplification](../simplification-review/plan.md), after its second proof and on build 8's closed request list; nothing is built from it yet. Authorized by Dickson on 2026-11-19.

## Rulings so far

- The terminal reading is rebuilt on NVDA's loop as a phase of its own, M6.4 (Dickson, 2026-11-19).
- Every existing terminal feature is kept unless Dickson authorizes its removal (Dickson, 2026-11-19). Named as kept: F2 (holding reads while speech waits), F5 (an unfinished word held), F7 (volatile rows), F8 (keys owning changes, lists and colours: many terminal programs need it), F11 and F12 (review over the terminal's text and moving by command). Each kept feature that today rests on finding rows again by their text (F3's rows) is redesigned simpler, keeping what it says: F8 by comparing the visible screen before and after a key row by row at the same places, F11 by reading live text with the position counted from the document's end and found again by its line's text only when a review command runs, F12 by a live search at the command; these are directions, each checked against recordings in the phase. No feature is proposed for removal: F1 to F14 are all kept.
- The skip count (F3): "Exact when it is easy and cheap to give an exact count. Otherwise, provide some sort of estimate when possible, e.g skilled over 1000 lines" ("skipped over 1000 lines"; Dickson, 2026-11-20).
- The size: about 3,300 product lines, 3.6 times NVDA's loop, is accepted with every feature kept, judged again after the second proof (Dickson, 2026-11-20, per the session's defaults). The build's budget is about 3,430, the 3,300 with today's pacing kept (Q13), and the build stops and goes to him above it, as it would if the recordings show F8 needs its larger form (Dickson, 2026-11-20).
- Q15 and Q16: NVDA's behaviour, once a capture at the pin confirms it, so that a read begun before a key's cut and answered after it is spoken, and a read out when the focus moves is spoken after the new focus's speech (Dickson, 2026-11-20). Where the capture shows otherwise, the build follows the capture.

## Decisions it changes, and the fact that changed

- ADR 0003, "What Verbatim keeps beyond NVDA", lists the terminal model among existing behaviour kept, and `books/dev/src/explanation/design/terminal-model.md` ("TM" below, with its rule numbers) was accepted on 2026-10-25. This study replaces the model's reading with NVDA's loop and keeps every Verbatim-only feature in a smaller form.
- The fact that changed: since build 3's implementation review the terminal reading has had five defects (6a9e55dc, 75b5fb5c, 33b0523c, then T1 and T2), the third-fix rule has stopped it, and Dickson's instruction of 2026-10-26 says a further failure of the closed rules means revisiting the whole design for a simpler model.
- ADR 0003's terminal decisions all hold, unchanged:
  - "Ask: a request from Core to a reader. Every call into an application is made for one, and it gets exactly one answer."
  - "A reader keeps nothing for Core between requests but held objects, ...; an outpost tells Core of every change of a terminal and keeps no flag of its own (Dickson, 2026-11-16)." The range the outpost keeps at a read's end (F3) is a held object that Core names; the flag in this design (`dirty`) is Core's, as "A terminal's 'told, not yet read', in Core alone" requires.
  - "Requests from ... a terminal's reads for its output stay outside the event queue, answered in the inbox and holding no key; a terminal read's speech joins the event queue in order (Dickson, 2026-11-16)."
  - "A new command adds no kind of request." The window read is a batch of build 8's generic operations; if that list cannot say it, the build stops and goes to Dickson.
- Process and thread layout: unchanged. Core judges keys and focus; the outpost only reads.
- Roadmap: M6.4's rebuild is this study's build; which of M6.4's residuals still apply is decided by its design, as the roadmap says.

## NVDA's loop, at the pinned source

- `NVDAObjects/behaviors.py` 519 to 525: a terminal starts monitoring at `event_gainFocus` and stops at `event_loseFocus`. `api.py` 93 to 94: every `setFocusObject` runs `loseFocus` on the old focus first, so every executed focus starts a new monitor.
- `_monitor` (468 to 505): `oldText` is read once, silently (470); a failure leaves it empty (473). Then: wait for a text change (476), clear the event (485), read all the text (488), diff against `oldText` (490), drop a result that is one line of one character (491 to 495), queue the lines' speech in the event queue (497 to 501), set `oldText` to the new text whether or not anything was spoken (503). An exception logs and waits for the next change, `oldText` unchanged (504 to 505). With "report dynamic content changes" off, nothing is diffed but `oldText` still advances (489, 503). `_monitor` checks `_keepMonitoring` only before it reads (475 to 477), so a read under way when the focus moves still queues its lines.
- A change while a read runs sets the event again, so the loop reads once more after it (420 to 424, 485).
- The read is the whole document: `LiveText._getText` (446 to 453) makes a text info at `POSITION_ALL`. `diffHandler.py` 39 to 48: the character diff's inserted runs, split into lines, blank and white-space lines dropped. At the console's formatted API level the text info is plain `UIATextInfo` (`winConsoleUIA.py` 396 to 397).
- Typing (`behaviors.py` 540 to 621): with "speak passwords" off, typed characters are queued and spoken at the terminal's next text change (572 to 592); Enter, Tab, Control+C, Control+D and Control+Pause empty the queue (595 to 611); one new line shorter than the typed word plus one, or than three characters, is dropped unless the last key typed was Tab (559 to 569, 573 to 578).
- A key's cut is itself a function queued in the event queue (`inputCore.py` 583 to 605): lines queued before it are cancelled, lines queued after it are spoken.
- A caret key in a terminal is NVDA's editable-text caret script, its wait 100 ms in the console host (`config/configSpec.py` 364, `winConsoleUIA.py` 374 to 376) and three times that in Windows Terminal (`winConsoleUIA.py` 470); the diff plays no part in answering it.
- NVDA keeps nothing of a terminal between focuses, has no flood limit, and has no terminal review, marks or footer of its own. The four files come to 912 lines (`behaviors.py` 372 to 636, `diffHandler.py`, `winConsoleUIA.py`), 595 without the older consoles' text infos (`winConsoleUIA.py` 39 to 355).

## The inventory: what today's code does, and where it goes

Today's product code, counted as lines before each file's test module: Core about 4,960 (`terminal.rs`, `terminal/`, `terminal_model.rs`, `terminal_model/review.rs`, `keys/terminal.rs`); rules about 5,630 (`terminal_reading.rs`, `terminal_rows.rs`, `terminal_rows/align.rs`); the outpost 624 (`verbatim-outpost/src/terminal.rs` and the handlers in `outpost/text_reads.rs` 715 to 957); remote operations about 1,940 (`verbatim-uia-rops` snapshot 621, find 383, colours 740, and `terminal_rows.rs` 198, which no product code calls); wire types about 305 (`verbatim-model/src/terminal.rs` and the terminal operations and replies in `text.rs`); and the codec in `verbatim-process/src/message/raw.rs`, 508: about 13,970. Not counted: the terminal arms in shared files (`keys/caret_key.rs`, `entries.rs`, `editing.rs`, `input_desktop.rs`, the outpost's worker) and the settings page. Tests: about 22,600 lines and 48 MB of expectations, of which `verbatim-term-sim` is 6,925, `verbatim-text` 4,682, Core's integration tests 3,456 and unit tests about 3,000, mockapp about 1,970, the recording tool 1,860, the codec's tests 409.

"Core" below means the core design; a feature code (F1 to F14) names the kept feature that carries the item; "NVDA" means the item goes and NVDA's behaviour takes its place.

- Change tells read at once, one read out, once more after it (TM rule 1): Core. Tested by watch cells A to C.
- The console host's four console WinEvents as change signals; notifications ignored: Core, unchanged in the outpost and listener.
- Baseline at the focus, silent; the focus says "terminal" and the caret's line (TM rule 15): Core; the focus speech stays the focus machinery's.
- Output written while the focus was elsewhere never heard: Core (every commit baselines, as NVDA).
- Core's caret record, fed today by every full read (`terminal_model.rs` 800 to 802, `editing::update_caret`, `editing.rs` 548), since a terminal raises no caret event for every character typed (`text_reads.rs` 811 to 817): Core. Every window read, the baseline included, carries the caret as a caret report read in the same remote operation, and column B passes it to `editing::update_caret`, which keeps the focus's caret following typing and the review cursor with it when it follows the caret (`review_text.rs` 138 and `editing.rs` 100 read it). A failed read leaves the caret as it was. The console host's system caret tells stay dropped while a console terminal is committed (`caret_tell.rs` 207), as today.
- Memory kept across focus changes, 16 models, generations: F4.
- `Follow` tracked by the terminal (resend at due time, refused, hung): goes. `Follow` is answered as for every other focus, whose ending "changes nothing in Core" (`focus/follow.rs` 11 to 13).
- Hang: `Stalled`, `WaitingForWindow`, `recovered`: go. Output written while the terminal hung is still spoken once its window answers (Q3).
- The read: anchored start, history block, whole-document and history-block scopes, start refused (TM rules 2, 6): replaced by the window read (Core) and the held range (F3).
- Rows without padding, a wrapped line one line (TM rule 3): Core (the outpost's `unpadded_utf16` kept).
- Positions, alignment, lost reads, screens, the aside screen, lingering rows, reads with no text, starting again (TM rules 4 to 11): NVDA's diff, F3's count and F14 for a new screen; redraws inside Ink's erase (TM rules 9 and 10) are Q7.
- New text spoken, blank lines silent, lost text silent (TM rule 12): Core (the diff's inserted runs); blank lines still count when skipped (Q1).
- Unfinished last word held (TM rule 13): F5. Reprinted input not spoken again (TM rule 14): NVDA (the diff matches the reprint against the old line; needs a capture).
- Rewritten rows from the first changed word, a spinner silent (TM rule 16): F6 with the spinner rule (Q6). Volatile rows (TM rule 17): F7. The quiet-period timer and `TerminalDue`: F5 and F7.
- Keys' windows, caret-line answers, rows the keys own, suggestions, colours (TM rules 18, 19, 21, 22, 23), keys deferred behind a held stroke: F8.
- Typing echo when the terminal shows it (TM rule 20): F9.
- Review over Core's snapshot, history reads, the upward search, review follows caret only on a key (TM rule 24): F11.
- Moving by command (marks): F12. Report status bar's footer: F13.
- Flood policy groups, "skipped N lines", "about", "more than", line cut: F1, with the count from F3. The 10 MB bound on waiting output: Core (it bounds memory).
- Release read, silent read after a cut, waited read, `deciding`, `release_waiting`: F2, derived from the pacing and the watch (Q9).
- Output of a read begun before a cut not spoken (`cut_at_ms`): Q15.
- Report new output (Verbatim+5), speak passwords, the console host's keyboard layout, text as it is: Core, unchanged.
- An identical read not given to the rules: goes; the diff of identical text is empty.
- A read's speech under its key's trace (`Effect::ForKey`): Core keeps the tell's trace only; latency attribution of output to a key goes (measurement tools read the typing echo, which keeps its key).
- The flight recorder's `Observed` snapshot of each read: Core, carrying the window read.

Every surviving item gets a Core unit test named for its cell or rule; the end-to-end terminal scenarios (17 families, both terminals) keep running, their expectations rewritten from NVDA captures wherever an item went to NVDA.

## The core design

### Who does what

- **The outpost** reads, at Core's request and in one remote operation: the rows from two screens above the document's end to the end, padding removed; the screen's height; the caret as a caret report; and, when Core names the range the last read left, the number of rows from that range to the window. When more rows follow the range than "Lines spoken in full" plus the window, it also reads the first "Lines spoken in full" of them, so that a burst faster than one read still opens with its first lines whole, as F1 says today. It keeps one range at the read's end as a held object, returns it, and drops the one Core named. A range refused or lost (its rows gone past the history, its outpost replaced) leaves the count to F3's estimate.
- **Core** owns everything else, in the reducer:
  - the focused terminal's watch: the one previous text (NVDA's `oldText`), the held range's name, the read out, and whether a change was told since it began (NVDA's event);
  - the diff (`verbatim-text`, written from `docs/parity/terminals.md`, not translated): inserted runs as lines, blank and white-space lines dropped from speech but counted when skipped (Q1), and NVDA's one-character and short-line drops only where they do not change today's ruled speech (Q2, Q8);
  - the typed characters queued (NVDA's `_queuedChars`);
  - the pacing: lines handed to speech a few at a time by index marks, bounded at 10 MB.
- **The event queue** carries every word the terminal says: each read's lines, and each echo, join as a `TerminalSpeech` entry at the answer's or tell's arrival, its fence set by the queue (`set_fence`, `entries.rs` 410), spoken at its own turn.

### The state

The watch is a field of the committed focus's record (`FocusContext`), present when the focus is a terminal:

```rust
struct TerminalFocus { watch: Watch, typed: String, pacing: Pacing }
enum Watch {
    Baselining { token: u64, dirty: bool },
    Live { old: Text, range: Option<RangeName>, read: Read },
}
enum Read { Idle { dirty: bool }, Out { token: u64, dirty: bool } }
```

`Live` with `Idle { dirty: true }` lasts only from an input to the settle that follows it (under F2 it may last while held). So the watch has five lasting states: Baselining clean, Baselining dirty, Idle, Out clean, Out dirty.

### The watch's state table

Columns: A a change told for this terminal; B its read answered `Result(Ok(text))`; C1 abandoned (`Refused`, `NoLongerAwaited(_)`); C2 failed (`Result(Err(_))`, `NotThere`, `NotResponding`, `ReaderEnded`); D an answer whose token is not the watch's; E settle, after every input; F any commit. Each outcome is matched by name.

| State | A told | B text | C1 abandoned | C2 failed | D stale | E settle | F commit |
|---|---|---|---|---|---|---|---|
| Baselining clean | Baselining dirty | Live, old = text, Idle clean; no speech | baseline again at the next tell (Q4) | Live, old empty, Idle clean (NVDA 473) | dropped | nothing | record replaced |
| Baselining dirty | stays | Live, old = text, Idle dirty | baseline again at settle (Q4) | Live, old empty, Idle dirty | dropped | nothing | record replaced |
| Idle | Idle dirty | n/a, no token | n/a | n/a | dropped | dirty: send a read, Out clean | record replaced |
| Out clean | Out dirty | diff; its lines join as an entry; old = text; Idle clean | `DueTime`: Idle dirty (Q5); otherwise Idle clean, old kept | Idle clean, old kept (NVDA 504) | dropped | nothing | record replaced, the answer later stale (Q16) |
| Out dirty | stays | as Out clean, then Idle dirty | Idle dirty | Idle dirty | dropped | nothing | as above |

- Every B passes its caret report to `editing::update_caret` before the diff.
- A read answered while the terminal hangs, then refused unsent as hung, ends in C2 and sends nothing more: `dirty` is cleared when a read is sent, so there is no loop. Output written during the hang is spoken once the window answers (Q3).
- "Report new output" off: column B still sets `old`, and the entry carries only the echo.
- The baseline is sent by the commit that builds the record, after the commit's `Follow`, as today's order class requires.
- A desktop switch commits no focus and then sleeps (`input_desktop.rs` 101, `focus/object.rs` 59 to 61), so the watch goes with the record and a later answer is stale (Q12).

### Typing (NVDA's queue)

- A typed character reaching Core while "speak passwords" is off is appended to `typed`; with it on, it is echoed at once, as today.
- A change tell with `typed` not empty joins an echo entry at once, its fence the queue's at the tell's arrival, and empties `typed` (NVDA 591 to 592).
- Enter, Tab, Control+C, Control+D, Control+Break empty `typed` (NVDA 595 to 611).
- A read's lines that pass the drops kept (Q2, Q8) empty the typed-word buffer and `typed` (NVDA 568 to 569).

### The pacing's state table

`Pacing` is `Idle` or `Speaking { left, waiting, ahead, sounding }`; `left` counts the group's lines still to hand on. Columns: G an entry's turn with lines; H an opening mark reached; I the closing mark of the last line handed reached; J a speech cut, or an utterance of its own cancelled; K "report new output" turned off; L a commit of the same terminal not left; M any other commit; N the outpost or its reader ending.

| State | G turn | H open mark | I last mark | J cut | K off | L same | M other | N reader ends |
|---|---|---|---|---|---|---|---|---|
| Idle | Speaking, hand on two | ignored | ignored | stays | stays | carried over | dropped | stays |
| Speaking, lines left | append, bound, hand on | hand on | hand on | Idle | waiting dropped | carried over | dropped | stays: what was read is spoken (Q11) |
| Speaking, group ended | append | hand on what belongs | look (F1) or hand on; Idle when nothing waits; the look waits for a read sent at the group's last line while the watch has it out (Q9) | Idle | Idle once handed lines end | carried over | dropped | stays (Q11) |

- At G, an entry whose fence a later cut has passed (`Arrivals`, `keys/entry.rs` 49 to 98) speaks nothing: NVDA's queued cut cancels lines queued before it. This reads the queue's own fact; the terminal keeps no cut time.
- Every utterance handed on ends at one of reached, cancelled (fenced off included: `verbatim-speech/src/manager.rs`, `fenced_off`, 1103 on) or failed, so no pacing state waits without an answer.

### The structural rule

So that a change to the event queue, key fences and cuts, or reader replacement cannot create a state the terminal never listed:

1. **One owner per fact, none copied.** The terminal stores no focus, fence, cut time, speech-spoken flag or `Follow` token. Which terminal is focused is the record the watch lives in; the fence is the entry's, set by the queue; whether a cut passed it is asked of `Arrivals` at the turn; the held range is the outpost's object, named by Core.
2. **The watch lives and dies with the committed focus's record.** `FocusContext` has one constructor (`focus_context`, `reduce.rs` 525 to 543, its literal at 531); its new field must be given there, so a new kind of commit cannot skip it, and every commit (a re-take, a re-executed focus, a replacement) replaces the watch. The pacing's carry-over for the same node not left is written beside `caret`'s in that constructor, the only place. Today's second reset path (`terminal::focus_moved`) goes.
3. **Reading never waits on speech, and speech never waits on reading.** The read is sent only at settle, from `Live` with `Idle { dirty: true }` (and, under F2, not held, held being a function of the pacing, not a stored flag). The pacing waits only on its own marks and, for the look at a group's end, on the watch's read out and on a speech entry of the terminal's with output waiting for its turn (Q9).
4. **Answers are matched exhaustively.** `Outcome`, `CancelReason` and `TextFailure` are matched by name, with no wildcard, in the watch; the queue's per-entry policies in `entries.rs` that now end in `_ =>` (`exempt_from_foreground` 541, `kept_by` 732, `merges` 925) name `TerminalSpeech`, so a new outcome, failure or queue policy fails to compile until the terminal's cell is decided.
5. **Terminal speech is emitted at one place.** Only a `TerminalSpeech` entry's turn, or a `Mark` entry's turn handing on paced lines, emits terminal speech, each through `cx.emit` under its own entry's fence. `speak_read` (`terminal/reading.rs` 1315) and `take_waiting_before` (`event_queue/driver.rs` 961) go: no other entry's turn speaks the terminal.

### The five defects, replayed

- **T1** (`conhost_flood`, the listener dies while the release read is out, the re-take commits): today `release_waiting` (`terminal/reading.rs` 284) survives the new `Baselining`, `mark_reached` sets `deciding`, and nothing clears it, so the terminal is silent until a key. Here there is no release read and no flag; the commit replaces the watch (rule 2), and the look at the group's end waits only for a read the new watch has out. Speech: the waiting lines are decided and spoken, then the new watch reads on from the window last spoken, so output written while the read was dropped is heard (Claude, 2026-11-22).
- **T2** (Windows Terminal: a caret key's final read out with key 8, "z" typed as key 9, the rules give "z" to that read): today `speak_read` emits it under the front's fence 8 and the speech manager drops it. Here the typing echo is never carried by a read's speech: "z" is queued in `typed`, the terminal's change tell joins an echo entry with the queue's fence 9, and its turn says "z". NVDA: "z" at the next text change (591 to 592).
- **6a9e55dc** (a modifier's cut left later arrivals under an older fence): every terminal entry's fence is set by `set_fence` at arrival, and the turn asks `Arrivals` whether a cut passed it; the terminal has no fence or cut time of its own to fall out of step.
- **75b5fb5c** (a reply type that could not hold "no text"): the window read decodes through `text_request::decode_text` to `Result<_, TextFailure>`, every failure matched by name into column C2.
- **33b0523c** (a review command looping on Core's memory answering "anchor lost" at the caret): F11's review finds its line by one live read at the command; a failed read ends the command saying nothing, and no review command is answered from Core's memory.

### Limits of the core

- A count past a lost range is an estimate: "skipped more than N lines", N the rows the outpost could count, or "skipped lines" when it could count none (F3).
- The window from the document's end assumes the screen ends there in both terminals, as today's read counts its start up from the end; a `verbatim-event-record` recording of each terminal confirms it before the build.

## The Verbatim-only features

Each is kept. Each gives what it adds, its form and cost (the reviewers' estimates in `docs/simplification-review/merged-terminals.md`, section 2.2), NVDA's behaviour, and one sequence. Speech marked "capture" needs an NVDA capture at the pinned build before it becomes an expectation; a form marked "recordings" is checked against recordings of the programs before it is built.

### F1. Flood groups and skip counts

The first "Lines spoken in full" (30) lines whole, "skipped N lines", the newest "Last lines to speak" (30); "about" and "more than" (TM, "Core's flood policy"). Form: the bound written once and one "drop the oldest past keep, count them" function, about 120 to 250 lines. NVDA speaks every line until a key (parity, "The backlog"). Sequence, the console host, `1..400 | % { "line $_" }`: "line 1" to "line 30", "skipped N lines", the newest 30, N the lines between (340 when all 400 are read before the first group ends).

### F2. Holding reads while speech waits

No read while 30 lines wait; one read at the group's last line (TM rule 1, "a flood once took 2,800 reads"). Form: "held" as a function of the pacing read at settle, no state, about 40 lines, with the look at a group's end waiting for the read sent at its last line (Q9). NVDA holds nothing. Speech is the same as without it; the saving is reads (below, "Costs").

### F3. Skip counts, exact where cheap

Form: the outpost keeps one range at the last read's end as a held object and counts the rows from it to the window in the same remote operation, about 200 lines; Dickson's ruling of 2026-11-20 allows an estimate where the exact count is not cheap. Today's row model (about 3,500 product and 4,000 test lines) goes. NVDA reads the whole document every change and counts nothing. Sequences: `1..5000` in the console host within its history: "line 1" to "line 30", "skipped 4940 lines", "line 4971" to "line 5000", exact. `1..10000` past a full history, where today's "skipped 9940 lines" needs the row model's alignment: when the range's rows scroll out of the history, "skipped more than N lines", N what the outpost counted (a recording of each terminal shows whether a held range survives a full history). A read whose range is lost and that can count nothing: "skipped lines".

### F4. Memory kept across focus changes

The review position and marks survive Alt+Tab (TM rule 15). Form: the places kept per terminal in a record dropped with the outpost that holds the terminal, about 50 to 60 lines; the watch itself still dies with the focus record. NVDA keeps nothing. Sequence: review three lines up, Alt+Tab away and back, Numpad 8: that line.

### F5. An unfinished word held

TM rule 13. Form: one timer and the last inserted line's held tail, about 100 to 120 lines; recordings. NVDA speaks each inserted run as it comes (`_monitor` 490 to 501). Sequence, a reply streamed as "The qu", "ick brown f", "ox": "The", "quick brown", "fox"; NVDA "The qu", "ick brown f", "ox" (capture: depends on read timing).

### F6. A rewritten line spoken from its first changed word

TM rule 16. Form: each inserted run on a line that had text widened to the start of its word, and a line whose one changed character is neither a letter nor a digit silent (the spinner, Q6): no state, about 70 lines. NVDA speaks the inserted characters and drops one character on one line (491). Sequence, vim's ruler "1,1 Top" to "2,1 Top": "2,1 Top" (parity, "Redrawn rows"); NVDA nothing, the change being one character.

### F7. Volatile rows

TM rule 17. Form: the visible screen compared by place, each row's last two change times, about 150 to 200 lines; recordings, since place replaces row identity. NVDA speaks every change its drops let through. Sequence, "Working... 1s" ticking to 34s and stopping: "Working... 1s", then "Working... 34s" (parity); NVDA the first line, then only ticks that insert at least three characters (capture).

### F8. Keys owning changes, lists, suggestions, colours

TM rules 18, 19, 21, 22 and 23, key windows, colour reads, the terminal caret key's reads and deferred keys. Form: one key window in the watch, the screen before the key compared by place with the first read after it within NVDA's caret wait, colours read only for changed rows (about 700 lines), or the screens before and after compared row by row at the same places with one colour read (about 1,200); the design chooses against recordings of fzf, tig and Microsoft Edit. A caret key's starting point is minted from the same caret report as today. NVDA answers a caret key with its caret script and speaks other changes through the diff. Sequence, `windows_terminal_marker_list`, Down Arrow in a list redrawn with the caret below it: "> banana"; NVDA's captured "blank" for each key (parity, "Line keys and lists a program draws").

### F9. Typing echoed only as the terminal shows it

TM rule 20. Form: a check at the read that the inserted text at the caret's line ends with the queued characters, echoing them only then and dropping them otherwise: no state, about 80 lines. NVDA speaks its queue at any text change. Sequence, a password prompt that shows nothing, with a clock ticking on the screen, "h", "u", "n" typed: nothing but the clock's output; NVDA "h", "u", "n" at the clock's ticks (parity, "Typing").

### F10. (folded) Reprinted input is left to the diff; no feature.

### F11. Review over the terminal's text

TM rule 24. Form: the review position as rows from the document's end plus its line's text, found again by one live read at the command, on `review_text.rs`, about 200 lines; recordings. NVDA reviews live ranges and follows the caret. Sequence, review a line above the caret, a script prints two lines, Numpad 8: the reviewed line (parity, "The review cursor while output is written").

### F12. Moving by command (marks)

Verbatim+Alt+Up and Down Arrow (TM, "Moving by command"). Form: a live search at the command, about 250 lines, with the find written once (about 200) in place of today's remote and classic twins (`terminal_find.rs` 121 and 320); recordings. NVDA has no such command. Sequence, two commands entered, Verbatim+Alt+Up Arrow: the earlier command's line.

### F13. Report status bar's footer

TM, "Report status bar". Form: one fresh window read and the footer rule (lines below the caret's line, else the last line with text): no state, about 100 lines. NVDA by its source finds no status line in either terminal and says "No status line found" (parity, uncaptured). Sequence, vim, Verbatim+End: vim's last line.

### F14. A new screen speaks its screen

TM rule 7. Form: a read sharing no line with the last speaks only its screen's rows: no state, about 20 lines. Which of F14 and F1 speaks for such a read is Q10. Sequence, `less` closed over a long history: the shell's screen. The capture today's rule cites for NVDA's speech, `nv-con-pager.txt`, is not in the repository, so it is made again.

## Size

| Part | Today, product | Rebuilt, product | Rebuilt, tests |
|---|---|---|---|
| All of it | about 13,970 | about 3,430 | about 3,000 to 4,000, against about 22,600 |
| Core design | | about 1,050 to 1,270 | about 1,400 |
| F1 | about 610 to 900 | 120 to 250 | |
| F2 | about 250 to 300 | 40 | |
| F3 | about 3,500 to 3,800 | 200 | |
| F4 | about 250 | 50 to 60 | |
| F5 | about 250 | 100 to 120 | |
| F6 | about 70 | 70 | |
| F7 | about 450 to 500 | 150 to 200 | |
| F8 | about 2,500 to 2,600, the colour read included | 700 to 1,200 | |
| F9 | about 80 to 300 | 80 | |
| F11 | about 1,000 to 1,400 | 200 | |
| F12 | about 400 to 1,070 | 250, plus a find written once (200) | |
| F13 | about 100 | 100 | |
| F14 | about 20 to 300 | 20 | |

Today's per-feature figures are line-range attributions good to about 15 percent, since one 760-line function (`terminal_reading.rs` 907 to 1668) serves most features. The core's 1,270: the diff and NVDA's drops 250 (`verbatim-text`), the watch and typing 280, the pacing 380, the window read 320 (outpost and remote operation), and 40 in shared code (the record's field, `Arrivals`' question, the named policy arms); 1,050 with the bound written once and each group handed to speech whole (Q13). NVDA's counterpart is 912 lines (595 without the older consoles' text infos), so the rebuild is 3.6 to 4.5 times NVDA's; the 1.3 target (about 1,190) is not reachable with every feature kept, F8 alone being 60 to 100 percent of it.

The budget: about 3,430 product lines, as ruled: the accepted 3,300 with F3 as the held range and F8 in its smaller form, and about 130 for keeping the pacing (Q13). F8 in its larger form would bring it to about 4,060, which goes to Dickson first. Tests: one Core test per recorded defect (about 400 lines), one replay per end-to-end family with a capture (about 40 lines each), and the simulator's conformance test against recordings.

## Questions for Dickson, in one batch

Each changes what is spoken against today, or settles a cell of the tables above. Each recommendation keeps today's ruled speech, or matches NVDA after a capture, unless it says otherwise; without his objection the build follows it.

1. **Q1. Blank lines in the flood count.** Today they count when skipped ("Every line counts, blank ones included", Dickson, 2026-10-07; `blank_lines_count_when_skipped`, `terminal/tests.rs` 812): 30 lines, 10 blank, 30 lines say lines 1 to 30, "skipped 10", lines 41 to 70. NVDA's diff drops them before any count (`diffHandler.py` 46), so under its diff alone the 60 lines pass with no skip said. Recommended: blank lines pass to the pacing, counted, not spoken (about 10 lines). Capture: the same flood, to confirm NVDA's drop.
2. **Q2. Output of one character.** Today, "speak passwords" on, "s" typed at "Password: " that shows "*": "s", then "*" (TM rule 20; parity, "Typing"; Core's terminal reading tests before build 9). By NVDA's two drops (`behaviors.py` 491 to 495, 559 to 569): "s" only. Recommended: keep TM rule 20. Capture: that prompt with "speak passwords" on.
3. **Q3. Output written during a hang.** Today "alpha" and "beta", written while hung, are spoken once the window answers (`a_hung_terminal_sounds_then_speaks_its_output_once_the_window_answers`, `tests/terminal_reading.rs` 1563); no dated ruling cites it. NVDA by its source also speaks them: `_getText` (488) has no timeout of its own and its diff runs when it returns. A silent baseline at the recovery's re-take would lose them. Recommended: keep, the existing test pinning it. Capture: a console written to while hung.
4. **Q4. A baseline refused or past its due time.** Today a new, silent baseline at the next tell (`terminal/reading.rs` 763). Taken as NVDA's failed first read (473), the next read would be diffed against empty text and up to two screens spoken. Recommended: keep, column C1 above.
5. **Q5. An output read past its due time** (11 s, `event_queue/mod.rs` 63). Today one read is sent again and, in Windows Terminal after `cargo build`, "Finished" is spoken (`terminal/reading.rs` 863 to 868). Taken as NVDA's failed read (504), the change would not be read until another arrives. Recommended: keep, `DueTime` from Out to Idle dirty, `NotResponding` and `Hung` staying clean.
6. **Q6. The spinner.** Today a line whose one changed character is neither a letter nor a digit is silent (TM rule 16; `a_spinner_symbol_replaced_alone_is_silent`): a read with a new line and the spinner's next glyph speaks the line only. NVDA, its result having two lines, speaks the glyph too. Recommended: keep, the spinner rule in F6. Capture and recording: a spinner with a new line in one read.
7. **Q7. Redraws, TM rules 9 and 10.** Today a read landing inside Claude Code's Ink erase and redraw is silent; NVDA's diff speaks the box again when it reappears. Recommended: keep, in a form that needs no row model. Recording and capture: Ink's redraw.
8. **Q8. NVDA's Tab exemption.** Today every completion is spoken (TM rule 20). NVDA drops a one-line insert shorter than three characters unless the last key was Tab, so only a two-character completion differs. Recommended: keep TM rule 20. Capture: a two-character tab completion.
9. **Q9. F2 against F1's look.** In a flood whose reads bring up to two screens, 30 or more lines wait when a group's last mark is reached, the hold has not lifted, and a look run then works on lines read before the hold, so the next group is stale. Sequence, `1..10000` in the console host: today "line 1" to "line 30", "skipped 9940 lines", "line 9971" to "line 10000". Recommended: the look waits for the read sent at the group's last line, derived from the watch's read out rather than a flag, which keeps that speech.
10. **Q10. F14 against F1 on a read sharing nothing with the last.** Both fire: after held reads, or after more than two screens scrolled past. Recommended: F14 when the held range shows no rows written past it (a screen replaced, as when `less` closes: the shell's screen, no skip phrase); F1 with F3's count otherwise (as after Windows Terminal's `clear; dir` with more than a screen of output: "skipped about N lines", then the newest 30), keeping today's speech where a test states it. Captures: `less` closed over a long history, and `clear; dir`.
11. **Q11. The outpost ending while lines wait.** Today what was read is still spoken and nothing is asked of the ended outpost (`terminal/tests.rs` 1616), as NVDA queues every line it read. Recommended: keep, column N above.
12. **Q12. The desktop switch.** The watch goes with the sleeping record; whether paced lines waiting at the switch are spoken is not stated today. Recommended: a test of today's behaviour first, then keep it.
13. **Q13. Each group handed to speech whole.** About 130 lines fewer; with "report new output" turned off mid-group the rest of the group, up to 30 lines, is heard instead of at most two. Recommended: keep the pacing (today's speech).
14. **Q14. The skip count.** Settled by Dickson's ruling of 2026-11-20 (F3).
15. **Q15. A read begun before a key's cut and answered after it.** Today its output is never spoken (`terminal.rs` 303, ruled under the flood policy: "output read before the cut is never spoken after it", Dickson, 2026-10-06 to 2026-10-17). NVDA queues the cut and then the read's lines, so speaks them. Sequence, `ping -t localhost` in the console host; Control pressed alone while the read of a new line is out: today nothing for that line, the next one spoken a second later; NVDA "Reply from ::1: time<1ms" for that line, then the next. Ruled: NVDA's after a capture (Dickson, 2026-11-20), since the 2026-10-10 ruling's own reason is NVDA's cut, which silences only lines queued before it; the parity entry is amended when the capture confirms it. Capture: the console host with a full history, Control pressed alone as each line appears, about 20 times.
16. **Q16. A read out when the focus moves.** Today, and in the tables above, dropped: its answer is stale once the commit replaces the watch. NVDA's monitor queues its lines after the new focus's speech (`_monitor` checks only before reading). Sequence, a console printing a line every 100 ms, Alt+Tab to Notepad while a read is out: dropped, Notepad's focus speech alone; NVDA, Notepad's focus speech, then that line. Ruled: NVDA's after a capture (Dickson, 2026-11-20), the stale answer's lines joining as an entry of their own. Capture: that sequence, repeated.

Before the design: recordings of a streamed reply (F5), volatile rows (F7), keys in fzf, tig and Microsoft Edit with their colours (F8), review while output arrives (F11), marks (F12), Ink's erase and redraw (Q7), a spinner with a new line (Q6), and each terminal's screen ending at the document's end; the captures named in the questions; and `conhost_typing`'s recorded reads for the test of a typed line's wrap (f0902400).

## Costs per output burst

- **Calls into the application:** one remote operation per read, over two screens and the count from the held range (today's ordinary read: 0.45 to 1.06 ms of snapshot, 1.6 to 3.3 KB). No whole-document read ever (today's worst case 1.1 million UTF-16 units, 25 to 31 ms of `GetText` with the console host's writer waiting, `performance.md`, "A full-screen program's exit"). Caret keys read the caret's line by ordinary text reads instead of two full terminal reads. With F2, reads during a flood stay near today's 1,911 for the console host's 10,000 lines (`performance.md`, "A terminal's reads, live").
- **Wake-ups:** one per read in the outpost and in Core, as today; the quiet-period timer for F5 and F7, as today; a once-a-second clock costs its reads only.
- **Processor time:** Core's diff over at most two screens, expected below today's 101 to 116 µs median per live read, with no 2 ms outliers from whole-document alignment.
- **Typing echo:** spoken at the change tell, with no read before it, so no slower than today's 4.1 ms (console host) and 21.4 ms (Windows Terminal) medians.
- **The measurement that confirms them:** `books/dev/src/how-to/measuring.md`'s procedure over `conhost_flood`, `windows_terminal_flood`, the 30-second clock, `measure_keys_conhost` and `measure_keys_windows_terminal`, at most 20 runs each: reads, messages, reducer time per read at P50 and P99, wake-ups of all processes and echo latency, against the figures cited above.
