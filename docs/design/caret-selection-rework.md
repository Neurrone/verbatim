# The caret and selection rework

Status: scope, decisions and order of work agreed by Dickson on 2026-10-25 and 2026-10-29, consolidated for building; to be built in a session of its own after [the focus pipeline rework](focus-pipeline-rework.md), whose unread repeats, followed focus and removal of `CoreFocus` it relies on (`docs/roadmap.md`, M4, "The caret and selection rework"). The study that led here, with its inventory of twenty-five mechanisms, their commits and the model as built at c0f80a7e, is kept in git: it last appears in full in this document at 07db29a8.

## In short

The shape stays: a caret key is held while the focus's outpost reads the caret, as NVDA's caret scripts read the caret before sending the key; the outpost then watches the application's events for evidence of what the key did and answers with facts compared inside the provider; Core chooses every word. What goes is a second layer, built before key holding closed the race it answered on 2026-10-22, which kept the caret and selection in nine places, two of whose disagreements are known defects: Control+A followed by a repeated focus says nothing, and a superseded key's answer leaves Core's caret stale.

The rework keeps one last read per text node in the outpost, makes one request per key that reads the caret before it and opens the watch, and has Core take every read as the newest caret by one stamp. It also closes the differences from NVDA in what keys say: a key that moves nothing speaks its unit, at once where the caret is at the edge of the text and otherwise after 100 ms, and a move that arrives later is still spoken; Alt+Up and Alt+Down move by sentence where the application does not; and five editing chords are silent.

## Decisions (Dickson)

- One baseline (2026-10-25). One last read per text node, held by the outpost and replaced by every read, is the only baseline. A key makes one request that reads the caret before the key and opens the watch with that read as the key's baseline. Every read reports facts. Core takes every report and every answer, late or superseded, as the newest caret by one stamp, the time the read began, and speaks from it.
- The second layer goes (2026-10-25): Core's history of eight caret reports and its prediction of what Backspace and Delete remove; the outpost's history of eight reads; the time-zero record of Core's own caret move; the "since" check limited to a line; report numbers; the three modes of the selection baseline; the separate watch request and its first check; and the single superseded slot. Until uiAccess (M8.1), a caret key pressed while an elevated window is in front is judged from the last read when its request arrives, a known defect fixed there.
- A node's last read lives while Core holds the node (`NodesHeld`), and Core's choice between a new and a repeated focus decides what is spoken: Core speaks a repeated focus's changes and ignores a new focus's (2026-10-25; amended 2026-10-29 for the focus pipeline rework's removal of `CoreFocus`).
- A key that moves nothing speaks as NVDA's script does (2026-10-29), replacing the decision of 2026-10-08 that made it silent, whose reason, a 100 ms wait that held the outpost's worker, the watch removed. A movement key and Delete speak their unit; Backspace and Control+Backspace that remove nothing say nothing, as NVDA's backspace script returns when the caret did not move (`_backspaceScriptHelper`, `nvda/source/editableText.py` 293 to 320); a Shift key that changes nothing says nothing, as NVDA's selection script reports only a change.
- The edge of the text (2026-10-29). Where the before read shows nothing selected and the caret where the key's direction is used up, Core speaks the key at once from that read, and keys behind it do not wait for it: the line keys on the first or last line, the character and word keys and Delete at the text's start or end. Elsewhere Core speaks the unit 100 ms after the key was passed on, NVDA's wait (`caretMoveTimeoutMs`).
- Later evidence (2026-10-29), beyond NVDA, which drops it: a key's answer arriving after Core spoke for the key, within the watch's bound, cuts speech and is spoken, unless a later key was pressed, the focus moved, or the answer shows the unit already spoken at the same place.
- The watch's bound is 1 second (2026-10-29), so that a caret move made by something else after a key that moved nothing is not taken as that key's.
- Alt+Up and Alt+Down Arrow match NVDA (2026-10-29): where the application leaves the caret where it was, Verbatim moves the caret by sentence itself and speaks the sentence, sentences found within each paragraph by Unicode's sentence boundaries (UAX #29); in a field read through UI Automation, which has no sentence unit, NVDA's move fails and its script says nothing, and so does Verbatim's.
- Control+Z, Control+Y, Control+X, Control+V and Escape in a text field match NVDA, which binds no script to them (2026-10-25): they open no watch and nothing is said for the key; a selection change the application's caret event shows is spoken as any report's is. Other keys outside the caret table keep the decision of 2026-10-08: they speak by their evidence.
- Terminals are out of scope except where the mechanism is shared (2026-10-25 and 2026-10-29): the unmoved-key rules, the edge of the text, later evidence and sentence moving do not apply to a terminal, whose reading rules say what a key did, and terminals keep today's handling of the five editing chords.
- The judgement of whether a key's evidence has arrived stays in the outpost, named in D14 as the one read-time decision outside Core, because it bounds reads on text changes (2026-10-25).
- One time stamp per caret report (2026-10-25), in microseconds, as the focus pipeline rework stamps every observation.
- The outbound queue's clause against merging a caret report across a reply is removed with no new test (2026-10-29): its stated reason, Core comparing its own copy of the selection, ended with 505e3ad8.

## Terms

- Core is Verbatim's main process; the reducer is the part that decides what is spoken. An outpost is the helper process for one application that makes its UIA or MSAA calls and reports facts. A round trip is one cross-process call into the application; a remote program is a small UIA program run inside the application, so that many reads cost one round trip.
- A held key is a caret-table key the keyboard hook swallows while the focus has text, until Core has had the caret read and passes it on.
- The before read is the read of the caret made for a held key before it is passed on.
- The watch is the outpost's open check for a key's evidence: after the before read, on each caret, text or selection event of the node it reads again, until a read shows evidence that the key took effect, and answers with that read. Evidence is the caret having left where it was, the characters beside it changing, the unit at the caret changing after Delete, the caret's line changing after a key that deletes, or the selection changing.
- The last read is the one record per text node the outpost holds: the caret and selection as provider positions, the selection's text up to 513 UTF-16 units, the caret's line text and the caret's offset in it, the note that the text changed, and the time the read began. A key's baseline is a copy of the last read pinned when its request arrives.
- General terms are NVDA's words when the text changed since the selection was last compared: the whole new selection "selected", "selected instead", or "selection removed".
- An anchor is a number the outpost gives the start of each chunk of text it sends; a position Core holds is an anchor plus an offset.
- The key's bound is 100 ms after Core passed a held key on (`KEY_AHEAD_BOUND_MS`).
- The edge facts are four facts of a read: whether the caret's line is the text's first line, whether it is the last, whether the caret is at the text's start, and whether it is at the end.

## The constraints that force the shape

- A key's effect can be seen only after the application handles it, and the application says so early, late, twice or never; Windows gives no signal that an application has handled a key, and a message sent to its thread as a marker can be handled before the key, since sent messages go before queued input. NVDA bookmarks the caret, sends the key and polls; Verbatim holds the key, reads, sends, and watches events.
- Two paths race: the key reaches Core through the hook, the application's event reaches the outpost directly. Holding closes the race for every held key, because the before read precedes the key and the watch is open in the outpost before the key is sent. An event-based path for applications whose events are reliable would reopen it, since the race is in the ordering, not in missing events.
- UIA text ranges are live: a position known before an edit moves with it, so the characters beside the caret and, for deleting keys, the line must be compared as well as the caret.
- Reads are not atomic: the application can act between the caret's read and its line's, so a read checks the caret again before it is believed.
- Only the process that holds the provider's ranges can order two positions or read the text between them, so every comparison and every change text is made in the outpost, inside the read's round trip.
- Whether the text changed since the selection was taken is not observable in one read of a classic edit control: hence the note, set by the value change, and the check for a waiting value change behind a sync with the WinEvent thread.
- Speech is cut at every key press, and NVDA's caret script says nothing once another script is queued.
- A caret event alone speaks no line, in NVDA or in Verbatim (`reduce_caret_moved`, `crates/verbatim-core/src/reduce.rs`): only a key's answer says a unit, so a move after a key is spoken only while the watch ties it to that key.

## The design

### Where each part runs

- The hook (`verbatim-input-windows`) holds the caret table's keys and defers every key behind a held key, as today.
- `verbatim-input` keeps the caret table and gains the closed list of the five editing chords, beside the clearing keys.
- The outpost (`verbatim-outpost`, `src/text/` and `src/outpost/`) holds the last read per node and each key's pinned baseline, makes every read, compares inside each read, judges evidence, runs the watch and its 1-second bound, and reads the edge facts in the before read.
- The remote caret program (`verbatim-uia-rops`, `src/caret.rs`) gains the edge facts.
- Core (`verbatim-core`, `src/editing.rs` and `src/state.rs`) holds one caret per focus, decides every word, keeps the key's bound, applies the edge rule, the bound's speech and later evidence, and decides a move by sentence.
- `verbatim-text` finds sentences, platform-neutral, written from `docs/parity/text.md`, with the ICU4X segmenter it already uses for words.

### One request per key, and the last read

A held key's request reads the caret before the key with the key's unit and the edge facts, answers at once with that read, and opens the watch with the read pinned as the key's baseline; the watch's answer is the request's second reply, so one request has two answers. The outpost replaces the node's last read with every read it makes: a focus read, a caret event's read, a before read, a watch's check, and Verbatim's own selection or caret move. A focus read compares with the node's last read when one exists and reports the changes with the focus; the last read lives while Core holds the node and is released with it. A repeat the focus pipeline rework sends unread is not read and leaves the last read as it was.

Every read reports facts: whether the caret and selection differ from the key's baseline (evidence), how the selection changed from the last read (the words), whether the text changed since (general terms), the caret's line and the key's unit as they are now and as they were in the baseline, and whether the caret is on the baseline's line. Through UIA these are compared in the one remote program, the baseline's selection being one more pair of ends; in a classic edit control by the edit control's messages, as today.

### What Core does with a read

Core keeps one caret per focus, the newest by its read's start, so a late answer never moves the review cursor back. It speaks from every read it accepts: a report's selection changes at once, or held in `heard` while a key waits on the node and said after the key's words; a before read's changes at once, before the key's words; a key's answer's words, then its changes; a late or superseded answer as a report. What Backspace removed is the difference between the answer's before and after lines (`removal`); Delete's unit is the before unit against the after unit; "something was selected before" is the baseline's selection. `later_key`, the landing rule, the Backspace-over-selection rule and `removes_selection` are unchanged.

### When no evidence comes

- The edge of the text. When the before read shows nothing selected, Core decides from its edge facts whether the key's direction is used up: Up Arrow, Page Up, Control+Home and Control+Up Arrow on the first line; Down Arrow, Page Down, Control+End and Control+Down Arrow on the last line; Left Arrow and Control+Left Arrow at the text's start; Right Arrow, Control+Right Arrow, Delete and Control+Delete at the text's end; Backspace and Control+Backspace at the text's start. It then speaks the key at once from the before read, the unit for a movement key or Delete and nothing for Backspace, passes the key on, and does not hold the keys behind it for its answer. Home and End are not in the list, since an editor's smart Home moves the caret from a line's start, nor is any Shift key. The facts cannot be wrong about there being nothing past the edge; an application that moves the caret within the line, Up Arrow on the first line going to its start, is covered by later evidence.
- The key's bound. When a key has no answer at its bound, Core speaks the key from the before read, as NVDA speaks after its 100 ms wait: the unit for a movement key, Delete or Control+Delete, nothing for Backspace or Control+Backspace or a Shift key, and for Alt+Up or Alt+Down the sentence move below. The bound is the deadline Core already keeps for the keys behind a held key, now armed for every held key until its answer and removed when the answer comes, so a key answered in time costs no wake-up. Core says nothing at the bound when a later key was pressed (`later_key`), as NVDA's script gives way to the next, or when the focus has left the node.
- Later evidence. A key's answer that arrives after Core spoke for the key cuts speech and is spoken as a timely answer is, unless a later key was pressed, the focus left the node, or the answer shows the caret on the baseline's line with the unit starting at the same offset in it, a fact the outpost reports, since Core cannot compare positions across chunks. Its selection changes are spoken as a report's in every case.
- The watch ends at the next key on the node, a focus on another node, Verbatim's own caret move or selection on the node, or 1 second after the key was passed on (`CARET_WATCH_BOUND`); then it frees itself silently, and later caret moves only update Core's caret. The console host's caret poll keeps the time the key was passed on, which bounds its re-reads.
- A key that was not held, a caret key in an elevated window until M8.1, is judged the same way from the last read when its request arrives, with its bound counted from its press.

### Moving by sentence

Alt+Up and Alt+Down Arrow are held as today. When a key's answer shows the application moved the caret, Core speaks the line, as NVDA speaks its line unit then. At the key's bound with no answer, in a text whose sentences Verbatim finds, Core moves by sentence:

- Verbatim finds sentences in a classic edit control's text, where NVDA's offset-based text does; its paragraph is the edit control's line as shown, wrapped lines each their own, as NVDA's edit control takes a paragraph to be its line (`nvda/source/NVDAObjects/window/edit.py` 623 to 647). Core learns from the focus's text whether the text is a classic edit control's. Through UIA, Verbatim does not move by sentence and the key says nothing.
- `verbatim-text` segments a paragraph by UAX #29's default sentence rules with no locale tailoring, trailing white space and punctuation belonging to the sentence before them, as NVDA does.
- Alt+Down moves to the start of the next sentence in the caret's line, or, from the line's last sentence, to the first sentence of the next line, which Core has the outpost read; Alt+Up moves to the start of the previous sentence. Where each lands from the middle of a sentence and at the text's ends is taken from NVDA's captures in the order of work's first step.
- Core has the outpost move the caret (`MoveCaret`), which ends the key's watch and becomes the node's last read, and speaks the sentence.

### The editing chords

Control+Z, Control+Y, Control+X, Control+V and Escape, when the focus is a text field that is not a terminal, open no watch and speak nothing for the key; the application's caret event, if any, is a report whose selection changes are spoken, in general terms where the text changed, as NVDA's caret event speaks them. In a terminal they are handled as today. Every other key outside the caret table speaks by its evidence, unchanged.

### Terminals

Terminals share the key protocol and nothing else changes for them. The before read in a focused terminal stays a read of the terminal, so its reading rules compare the screen just before the key; the watch's answer still triggers a terminal read and still releases the keys behind it; its answer only keeps Core's caret current. The unmoved-key rules, the edge of the text, later evidence and sentence moving are skipped for a terminal by its role. The one stamp changes only the time on Core's caret, which no terminal rule reads.

### Mechanisms removed, kept and added

Removed in Core: `caret_history`, `CARET_HISTORY`, `TimedCaret`, `caret_before`, `caret_moved_by_core`'s time-zero record, `PendingCaret::{deleted, before, selected_before, before_unknown}` with `deleted_text` and `compared_text`, `superseded_caret` and `superseded_reply`, and `CaretWatch::{since, compare}`. Removed in the outpost: `CaretSeen`, `REMEMBERED_CARETS` and the search by press time, `KeptSelection::report` and `NodeAnchors::reports`, `LastSelection` and `CaretFor::Focus`'s restart, `Baseline::{selected, known_line}` with `compared_already`, `Signal::covered`, the separate `AwaitCaret` request and the watch's first check, `forget_selection` at a focus, `MoveCaret`'s clearing of the history, and the outbound queue's clause against merging a caret report across a reply with its test.

Kept: holding, deferral, the wait for the key ahead and its 100 ms bound; the recheck of non-atomic reads; general terms with the note and the WinEvent sync; event coverage (`caret_read_since`); the caret read on every caret event, which is how a program's or the mouse's selection is found; `heard` and `later_key`; the console host's caret poll, with the time the key was passed on.

Added: the edge facts in the before read; the key's bound speaking; later evidence; the sentence move; the editing chords' list; the last read's life following `NodesHeld`.

### Costs

- Calls: a held key through UIA makes two round trips, the before read and the answering check, instead of three; classically two reads of 8 and 15 calls instead of 8, 15 and 15; in a classic edit control 5 and 6 window messages instead of 5, 5 and 6, plus at most two for the edge facts (the line count and the text's length). Through UIA the edge facts are the document range's two ends compared with the caret and its line inside the program.
- Wake-ups: one request fewer per held key (one pipe write in Core, one wake of the outpost's pipe reader, one worker item) and the first check's round trip gone, against the measured 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time per held key. The key's bound wakes Core only for a key with no answer at 100 ms; a sentence move costs one read of the next line at most and one caret move.
- Latency: key to first sound unchanged for a key that moves; at the edge of the text it falls from never to the before read's time; elsewhere a key that moves nothing speaks at 100 ms, as NVDA does.
- Bytes: the answer carries the before line, usually under a hundred bytes and capped at the chunk's 64 KB.
- Memory: one last read and one pinned baseline per node, against eight reads and a kept selection in the outpost and eight timed reports in Core.
- Remote operations: the caret program's worst case, 4,462 instructions measured, stays under 5,000 with the edge facts and the baseline's selection.

### Exact speech

Captured from NVDA (`target/e2e-evidence/m42-text-captures-2026-10-23/`, `docs/parity/text.md`): Home at the start of a line reads the character after the caret; Control+Home at the start of the text reads the first line; Down Arrow on the last line reads that line; Control+Up and Control+Down Arrow in the Windows Forms text box, which does not move its caret, read the line; Alt+Down on "alpha beta gamma" in the text box says "delta epsilon"; in Notepad, Alt+Down moves nothing and NVDA's script says nothing; Control+Z, Control+Y, Control+X, Control+V and Escape say nothing for the key. The `caret_keys` scenarios' expectations change accordingly: Control+Z's "x" and Control+V's "mangoalpha beta gamma" become silence, while "selection removed" after Control+V and Enter over a selection stays, being a report's change under the decision of 2026-10-22.

Expected by NVDA's script and captured in the first step before any expectation is written: Up Arrow and Page Up on the first line and Page Down on the last read the line; Left Arrow at the text's start reads its first character; Right Arrow, Delete and Control+Delete at the text's end, and Control+Right Arrow there, read the unit at the caret, "blank" where there is none; Control+Left Arrow at the start reads the first word; Backspace and Control+Backspace at the start say nothing; Alt+Up from the middle of a sentence, and Alt+Up and Alt+Down at the text's ends, in the text box.

### Edge cases and limitations

- A slow move: an application that moves the caret more than 100 ms after the key first has the old unit spoken, then, within the watch's second, the cut and the new unit. After the second, the move only updates the caret.
- A caret move made by something else within a second of a key that moved nothing is spoken as that key's, unless another key came between.
- Up Arrow in an empty chat box that opens the last message for editing speaks "blank" and then the new focus, as NVDA, whose pending focus makes its script read the old field's caret.
- A key held with auto-repeat at the edge of the text speaks at every repeat and holds none behind it; elsewhere, a repeat of a key that moves nothing still waits up to 100 ms for the one ahead.
- The end-to-end harness's idle barrier waits on an open watch, so a step whose key moves nothing away from the edge waits up to 1 second.
- An elevated window's keys are not held until M8.1, so their before read is the last read when the request arrives, and one whose own event was read first may find no evidence and speak at its bound from the caret after the key.

## Order of work

1. Captures: NVDA, with its submodule pin equal to the installed build, pressing in Notepad and the Windows Forms text box the keys listed above as not yet captured.
2. Pin today's defects with tests: Control+A followed by a repeated focus is announced; a superseded key's answer becomes Core's caret; a caret event's report and a key's answer are ordered by one stamp.
3. The last read and one request per key, the last read living while Core holds the node.
4. Remove the second layer listed above.
5. Keys that move nothing: the edge facts, the edge rule, the key's bound speaking, later evidence, and the watch's 1-second bound.
6. The keys as NVDA: the editing chords, and the sentence move with `verbatim-text`'s sentences; the `caret_keys` expectations change.
7. One stamp, and the merge clause's removal.
8. The measurements and scenarios below.
9. The documents: D14 names the evidence judgement and the baseline; the key-holding design, `docs/parity/text.md` ("A caret key that moves nothing", "Keys outside the caret table") and `docs/parity/input.md`, and the crate guides for `verbatim-core`, `verbatim-outpost`, `verbatim-input`, `verbatim-text` and `verbatim-uia-rops` describe the result.

## Tests and measurements

New tests, one per behaviour:

- mockapp: a focus reported twice with Control+A between, Core dropping the second, "N characters selected" said once.
- Core: three keys whose first answer arrives after the third press, the first's change said.
- mockapp: a mouse selection between the before read and the key, said before the key's words.
- The outpost: a key whose baseline's selection differs from the last read's, evidence by ends.
- Core: Up Arrow with the before read on the first line and nothing selected speaks the line at once and holds no key behind it; with a selection it waits for the watch.
- Core: a key with no answer at its bound speaks its unit; Backspace says nothing; with a later key pressed, nothing.
- Core: an answer after the bound with the caret on another line cuts speech and speaks the line; one on the baseline's line at the same offset says nothing.
- The outpost: the watch ends 1 second after the key was passed on, and Verbatim's own caret move ends it.
- `verbatim-text`: sentences of a paragraph by UAX #29, trailing white space and punctuation with the sentence before.
- Core: Alt+Down with no answer at its bound in an edit control's text moves the caret to the next sentence and speaks it; through UIA it says nothing.
- Core: Control+Z in a text field opens no watch and says nothing.

Measurements and scenarios:

1. Call counts (`crates/mockapp/tests/call_counts.rs`): the before read 1 remotely, 8 classically, 5 messages plus the edge facts' at most two; the answering check 1, 15, 6; no first check.
2. Instruction counts (`instruction_limit.rs`): the caret program with a previous selection, a word unit and the edge facts, worst case under 5,000.
3. The stale caret workloads (`measure_stale_caret_text_box`, `measure_stale_caret_notepad`, `measure_stale_caret_conhost`): 0 of 60 misjudged.
4. Wake-ups and processor time (`cargo xtask trace scenario` at info level) over `measure_keys_text_box`, `measure_keys_windows_terminal` and a Notepad key workload, before and after; key to first sound P50 and P95 unchanged.
5. Scenarios: every `text_box_*` and `notepad_*`, `wpf_password`, `settings_search_suggestions` and `settings_search_suggestions_reentered`, and `terminal_key_timing` with the `windows_terminal_` and `conhost_` key scenarios, since the key protocol is shared with terminals.

## Why this design

- Leaving the model as it is keeps ten patches, seven of them for a race holding closed, nine places of state, two known losses and three round trips per held key.
- The minimum step, the baseline's life following Core's focus and superseded answers updating Core's caret, fixes the two losses but leaves the histories, the modes, two requests and two clocks.
- Core sending its selection with every request partly reverts 505e3ad8, gives caret events, the path for mouse and program selections, nothing to carry it, and brings back two baselines.
- Core comparing raw ends cannot order positions across chunks or read the text between them.
- The outpost reporting every read's facts and Core judging evidence, D14's literal form, needs a message per key or reads twice per typed character, since the outpost must know when a key is answered to stop reading on text changes; it is revisited if uiAccess brings a cheap signal.
- A second path that trusts events where they are reliable reintroduces the two paths' race, since the race is in ordering, and the reconstruction of "before" from time stamps that this rework removes.
- A signal that the application has handled a key exists only in process; D2 makes an in-process helper a speed-up, never a requirement, so it could at most shorten the key's bound later. A shorter bound tuned per application is a heuristic.
