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
- The edge of the text (2026-10-29). Where the before read shows nothing selected and the caret where the key's direction is used up, Core speaks the key at once from that read, and keys behind it do not wait for it: the line keys on the first or last line of a text with more than one line, the character and word keys and Delete at the text's start or end. A one-line field is left out of the line keys' rule, because its Up and Down Arrow often change its text, as a combo box's edit or a search box with suggestions does.
- The 100 ms bound is the watch's (2026-10-29): when a key has no evidence 100 ms after it, NVDA's wait (`caretMoveTimeoutMs`), the watch reads the caret once more and answers with that read, and Core speaks from it, as NVDA speaks the caret its last poll found.
- Later evidence (2026-10-29), beyond NVDA, which drops it: a key's answer arriving after Core spoke for the key, within the watch's bound, cuts speech and is spoken, unless a later key was pressed, the focus moved, or the answer shows the same unit text at the same place as was spoken.
- The watch's bound is 1 second (2026-10-29), so that a caret move made by something else after a key that moved nothing is not taken as that key's.
- Alt+Up and Alt+Down Arrow match NVDA (2026-10-29): where the application leaves the caret where it was, Verbatim moves the caret by sentence itself and speaks the sentence, sentences found within each paragraph by Unicode's sentence boundaries (UAX #29); in a field read through UI Automation, which has no sentence unit, NVDA's move fails and its script says nothing, and so does Verbatim's. In a rich edit control Verbatim finds sentences itself over the text between line breaks, where NVDA uses the control's own sentences through its object model, a recorded difference until M6.5 reads that model.
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
- The key's bound is the watch's first bound, 100 ms after the before read ended, which is within a few milliseconds of Core passing the key on; for a key that was not held, 100 ms after its request arrived.
- The bound answer is the watch's answer at the key's bound when no evidence came: the caret read then, marked as having no evidence.
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

- The hook (`verbatim-input-windows`) holds the caret table's keys and defers every key behind a held key, as today, and reports a key of the editing chords' list with its gesture, where it now reports any other passed key with none (`KeyReport::Passed`).
- `verbatim-input` keeps the caret table and gains the closed list of the five editing chords, beside the clearing keys.
- The shell (`verbatim-app`) turns a reported chord into its own input to Core, Escape as well as the clearing key it is today, and registers a key's request as one with several answers (below).
- The model (`verbatim-model`) carries the request's answers, the edge facts, the bound answer's mark, the answer's fact that the unit is unchanged, the move's stamp in `MoveCaret`'s and `Select`'s answers.
- The outpost (`verbatim-outpost`, `src/text/` and `src/outpost/`) holds the last read per node and each key's pinned baseline, makes every read, compares inside each read, judges evidence, runs the watch with its 100 ms and 1-second bounds, and reads the edge facts in the before read.
- The remote caret program (`verbatim-uia-rops`, `src/caret.rs`) gains the edge facts.
- Core (`verbatim-core`, `src/editing.rs` and `src/state.rs`) holds one caret per focus, decides every word, applies the edge rule, speaks a bound answer and later evidence, and decides a move by sentence. It arms no new timer: the key's bound is the watch's, and Core's bound on the wait for the key ahead (`KEY_AHEAD_BOUND_MS`, kept by the shell's `held_wait`) becomes a backstop.
- Sentences are found as say-all finds them today: the outpost answers a sentence read with the paragraph (`Sentences::ByParagraph`) or as unsupported (`Sentences::Unsupported`, UIA), and Core splits the paragraph with `verbatim-text`'s sentences (`Segmenter::sentences`, UAX #29 through ICU4X; `docs/parity/speech-audio.md`). The outpost gains a third case for a rich edit control, below.

### One request per key, and the last read

A held key's request reads the caret before the key with the key's unit and the edge facts, answers at once with that read, and opens the watch with the read pinned as the key's baseline. Its later answers are, in order: the watch's answer, either evidence before the key's bound or the bound answer; then, after a bound answer only, either later evidence within the second or the watch's end. The watch's end can also come at any time before the bound answer, when the next key on the node or a focus elsewhere ends it, or the console host's poll ends it, and then no bound answer follows. Every answer but the first carries the facts below, and the request ends at its evidence, its later evidence or its end. The shell's request registry takes such a request as one entry: counted as outstanding, with the 150 ms `HELD_KEY_BOUND` deadline, until its first answer, and passive after it, as a watch is today, so that the idle barrier's wait on an open watch stays in the outpost. The outpost replaces the node's last read with every read it makes: a focus read, a caret event's read, a before read, a watch's check, and Verbatim's own selection or caret move. A focus read compares with the node's last read when one exists and reports the changes with the focus; the last read lives while Core holds the node and is released with it. A repeat the focus pipeline rework sends unread is not read and leaves the last read as it was.

Every read reports facts: whether the caret and selection differ from the key's baseline (evidence), how the selection changed from the last read (the words), whether the text changed since (general terms), the caret's line and the key's unit as they are now and as they were in the baseline, whether the caret is on the baseline's line, and whether the unit is the baseline's unit, the same text starting at the same offset of that line. Through UIA these are compared in the one remote program, the baseline's selection being one more pair of ends; in a classic edit control by the edit control's messages, as today.

### What Core does with a read

Core keeps one caret per focus, the newest by its read's start, so a late answer never moves the review cursor back. Verbatim's own caret move and selection are stamped too: `MoveCaret` and `Select` answer with the time the move began, which the outpost also gives the last read it makes of the move, and Core takes the caret it moved to, the line it already holds, with that stamp, in place of the time-zero record; say-all's move is one. Core speaks from every read it accepts: a report's selection changes at once, or held in `heard` while a key waits on the node and said after the key's words, a key that Core has already spoken for not counting as waiting; a before read's changes at once, before the key's words; a key's answer's words, then its changes; a late or superseded answer as a report. What Backspace removed is the difference between the answer's before and after lines (`removal`); Delete's unit is the before unit against the after unit; "something was selected before" is the baseline's selection. `later_key`, the landing rule, the Backspace-over-selection rule and `removes_selection` are unchanged.

### When no evidence comes

Core speaks for a key once, from the first of its edge decision, its evidence and its bound answer; any answer after that is later evidence.

- The edge of the text. When the before read shows nothing selected, Core decides from its edge facts whether the key's direction is used up: in a text with more than one line, Up Arrow, Page Up, Control+Home and Control+Up Arrow on the first line, and Down Arrow, Page Down, Control+End and Control+Down Arrow on the last; in any text, Left Arrow and Control+Left Arrow at the text's start, Right Arrow, Control+Right Arrow, Delete and Control+Delete at its end, and Backspace and Control+Backspace at its start. It then speaks the key at once from the before read, the unit for a movement key or Delete and nothing for Backspace, passes the key on, and does not hold the keys behind it for its answer. Home and End are not in the list, since an editor's smart Home moves the caret from a line's start, nor is any Shift key. Nothing can lie past the edge; an application that moves the caret within the line, Up Arrow on the first line going to its start, or changes the text, is covered by later evidence.
- The key's bound. When the watch has found no evidence at the key's bound, it reads the caret and sends the bound answer, and stays open. Core speaks the key from that read, as NVDA speaks the caret its last poll found after its 100 ms wait: the unit for a movement key, Delete or Control+Delete, nothing for Backspace or Control+Backspace or a Shift key, and for Alt+Up or Alt+Down the sentence move below. The bound answer also ends the wait of the keys behind the key, as an answer does today, and comes first: Core's bound on that wait (`KEY_AHEAD_BOUND_MS`) becomes a backstop for an answer that never comes, 250 ms after the key was passed on, the watch's 100 ms bound plus the read's own 150 ms bound (`HELD_KEY_BOUND`), so that a key behind is not released, and its request does not end the watch, before the bound answer's read. Core says nothing for a bound answer when a later key was pressed (`later_key`), as NVDA's script gives way to the next, or when the focus has left the node; it takes its caret either way. The bound costs one read and one wake of the worker, which already wakes at the watch's bound, for a key with no evidence at 100 ms, and nothing for a key answered sooner.
- Later evidence. An answer that arrives after Core spoke for the key cuts speech and is spoken as a timely answer is, unless a later key was pressed, the focus left the node, or the answer reports the unit unchanged from what was spoken, the same text at the same place, a fact the outpost reports, since Core cannot compare positions across chunks. Its selection changes are spoken as a report's in every case. For a key spoken at the edge, the watch's answers are all later evidence, its bound answer included.
- The watch ends at the next key on the node, a focus on another node, Verbatim's own caret move or selection on the node, or 1 second after the before read ended (`CARET_WATCH_BOUND`); then it frees itself silently, and later caret moves only update Core's caret. The console host's caret poll counts its 50 ms from the end of the before read, a few milliseconds before the key reaches the console, so it may make one read before the key's effect can show.
- An editing chord opens no watch and does not end the earlier key's: an answer that watch gives after the chord only keeps the caret current and speaks its selection changes, since the chord's cut of speech marks a later key (`later_key`, through `Input::SpeechCancelled`).
- A key that was not held, a caret key in an elevated window until M8.1, is judged the same way from the last read when its request arrives, with its bound counted from the request's arrival.

### Moving by sentence

Alt+Up and Alt+Down Arrow are held as today. When a key's answer shows the application moved the caret, Core speaks the line, as NVDA speaks its line unit then. At a bound answer, in a text whose sentences Verbatim finds, Core moves by sentence:

- Core asks the outpost for the sentence read at the caret, as say-all does. A plain edit control answers with its line as shown, wrapped lines each their own, as NVDA's edit control takes a paragraph to be its line (`nvda/source/NVDAObjects/window/edit.py` 623 to 647). A rich edit control answers with the text between line breaks, a new `Sentences` case of the edit control source that say-all's sentence reads share, where NVDA uses the control's own sentences over its own paragraphs through its object model whenever it can get that model (`ITextDocumentTextInfo` for any `editAPIVersion` other than 0, 1041 to 1045), a recorded difference until M6.5. UIA text answers that it has no sentences, so the key says nothing.
- Core splits the paragraph with `verbatim-text`'s sentences, UAX #29's default rules with no locale tailoring, as NVDA's ICU call does (`nvda/source/textUtils/icu.py` 153 to 181).
- Alt+Down moves to the start of the next sentence in the caret's paragraph, or, from its last sentence, to the first sentence of the next paragraph; Alt+Up moves to the start of the previous sentence, from a paragraph's first sentence the last sentence of the paragraph before. Core has the outpost read the next or previous paragraph's sentences when it needs them. Where each key lands from the middle of a sentence and at the text's ends is taken from NVDA's captures in the order of work's first step.
- Core has the outpost move the caret (`MoveCaret`), which ends the key's watch and becomes the node's last read, and speaks the sentence. The keys behind wait until the move is answered, as they wait for a key's answer, so that their before read sees the moved caret.

### The editing chords

Control+Z, Control+Y, Control+X, Control+V and Escape, when the focus is a text field that is not a terminal, open no watch and speak nothing for the key, the hook reporting them as the chords' list names them; the application's caret event, if any, is a report whose selection changes are spoken, in general terms where the text changed, as NVDA's caret event speaks them. In a terminal they are handled as today. Every other key outside the caret table speaks by its evidence, unchanged.

### Terminals

Terminals share the key protocol and nothing else changes for them. The before read in a focused terminal stays a read of the terminal, so its reading rules compare the screen just before the key; the watch's answer still triggers a terminal read and still releases the keys behind it; its answer only keeps Core's caret current. A bound answer in a terminal is such an answer, and the console host's poll still ends its watch at the poll's last read, as today. The unmoved-key rules, the edge of the text, later evidence and sentence moving are skipped for a terminal by its role. The one stamp changes only the time on Core's caret, which no terminal rule reads.

### Mechanisms removed, kept and added

Removed in Core: `caret_history`, `CARET_HISTORY`, `TimedCaret`, `caret_before`, `caret_moved_by_core`'s time-zero record, `PendingCaret::{deleted, before, selected_before, before_unknown}` with `deleted_text` and `compared_text`, `superseded_caret` and `superseded_reply`, and `CaretWatch::{since, compare}`. Removed in the outpost: `CaretSeen`, `REMEMBERED_CARETS` and the search by press time, `KeptSelection::report` and `NodeAnchors::reports`, `LastSelection` and `CaretFor::Focus`'s restart, `Baseline::{selected, known_line}` with `compared_already`, `Signal::covered`, the separate `AwaitCaret` request and the watch's first check, `forget_selection` at a focus, `MoveCaret`'s clearing of the history, and the outbound queue's clause against merging a caret report across a reply with its test.

Kept: holding, deferral, the wait for the key ahead and its 100 ms bound; the recheck of non-atomic reads; general terms with the note and the WinEvent sync; event coverage (`caret_read_since`); the caret read on every caret event, which is how a program's or the mouse's selection is found; `heard` and `later_key`; the console host's caret poll, counted from the before read's end; Core's bound on the wait for the key ahead (`KEY_AHEAD_BOUND_MS`, kept by the shell's `held_wait`), now a backstop at 250 ms.

Added: the edge facts in the before read; the watch's bound answer and the request with several answers; later evidence; the stamp in `MoveCaret`'s and `Select`'s answers; the sentence move and the rich edit control's sentence read; the editing chords' list and their report from the hook; the last read's life following `NodesHeld`.

### Costs

- Calls: a held key through UIA makes two round trips, the before read and the answering check, instead of three; classically two reads of 8 and 15 calls instead of 8, 15 and 15; in a classic edit control 5 and 6 window messages instead of 5, 5 and 6, plus at most two for the edge facts (the line count and the text's length). Through UIA the edge facts are the document range's two ends compared with the caret and its line inside the program.
- Wake-ups: one request fewer per held key (one pipe write in Core, one wake of the outpost's pipe reader, one worker item) and the first check's round trip gone, against the measured 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time per held key. A key with no evidence at 100 ms costs one more read, one more answer and the wake-ups that carry it; a key answered sooner costs nothing more, and Core arms no timer. A sentence move costs one read of a paragraph at most and one caret move.
- Latency: key to first sound unchanged for a key that moves; at the edge of the text it falls from never to the before read's time; elsewhere a key that moves nothing speaks at 100 ms, as NVDA does.
- Bytes: the answer carries the before line, usually under a hundred bytes and capped at the chunk's 64 KB.
- Memory: one last read and one pinned baseline per node, against eight reads and a kept selection in the outpost and eight timed reports in Core.
- Remote operations: the caret program's worst case, 4,462 instructions measured, stays under 5,000 with the edge facts and the baseline's selection.

### Exact speech

Captured from NVDA (`target/e2e-evidence/m42-text-captures-2026-10-23/`, `docs/parity/text.md`): Home at the start of a line reads the character after the caret; Control+Home at the start of the text reads the first line; Down Arrow on the last line reads that line; Control+Up and Control+Down Arrow in the Windows Forms text box, which does not move its caret, read the line; Alt+Down on "alpha beta gamma" in the text box says "delta epsilon"; in Notepad, Alt+Down moves nothing and NVDA's script says nothing, though its capture then says "Non Client Input Sink Window" from something other than the script, which the first step traces before Notepad's expectation is written; Control+Z, Control+Y, Control+X, Control+V and Escape say nothing for the key; Down and Up Arrow in Settings' one-line search box with suggestions open read the box's line as well as the suggestion. The expectations change accordingly: in the `caret_keys` scenarios, Control+Z's "x" and Control+V's "mangoalpha beta gamma" become silence, while "selection removed" after Control+V and Enter over a selection stays, being a report's change under the decision of 2026-10-22; in `settings_search_suggestions`, the declared difference for Down and Up Arrow goes, with the order of the line and the suggestion taken from NVDA's capture.

Expected by NVDA's script and captured in the first step before any expectation is written: Up Arrow and Page Up on the first line and Page Down on the last read the line; Left Arrow at the text's start reads its first character; Right Arrow, Delete and Control+Delete at the text's end, and Control+Right Arrow there, read the unit at the caret, "blank" where there is none; Control+Left Arrow at the start reads the first word; Backspace and Control+Backspace at the start say nothing; Alt+Up from the middle of a sentence, and Alt+Up and Alt+Down at the text's ends, in the text box.

### Edge cases and limitations

- A slow move: an application that moves the caret more than 100 ms after the key first has the old unit spoken, then, within the watch's second, the cut and the new unit. After the second, the move only updates the caret.
- A caret move made by something else within a second of a key that moved nothing is spoken as that key's, unless another key came between.
- Up Arrow in an empty chat box that opens the last message for editing speaks "blank" and then the new focus, as NVDA, whose pending focus makes its script read the old field's caret.
- A key held with auto-repeat at the edge of the text starts speaking at every repeat, each cut by the next, and holds none behind it, as auto-repeat over keys that move does today; NVDA speaks only the last repeat, since each of its scripts returns while another waits (`nvda/source/editableText.py` 94 to 95), a difference. Elsewhere a repeat of a key that moves nothing waits for the one ahead's bound answer, about 100 ms and the read.
- The keys behind an edge key are not held for its answer, so a key pressed within a few milliseconds may have its before read taken before the edge key's effect, such as Notepad moving the caret to the first line's start on Up Arrow; its evidence and words are then judged against the caret before that move.
- Any key press that cuts speech marks a later key (`later_key`, through `Input::SpeechCancelled`), a typed character included, where NVDA's script gives way only to another script (`isScriptWaiting`), so a letter typed within 100 ms of a key that moves nothing silences that key's bound answer, where NVDA speaks the unit; this rule is today's and is unchanged here.
- A one-line field's Up and Down Arrow, and Home and End anywhere, speak at the bound when they move nothing, 100 ms after the key, as NVDA does.
- The end-to-end harness's idle barrier waits on an open watch, so a step whose key moves nothing away from the edge waits up to 1 second.
- An elevated window's keys are not held until M8.1, so their before read is the last read when the request arrives, and one whose own event was read first may find no evidence and speak at its bound from the caret after the key.

## Order of work

1. Captures: NVDA, with its submodule pin equal to the installed build, pressing in Notepad and the Windows Forms text box the keys listed above as not yet captured, Down and Up Arrow in Settings' search box with suggestions open, and Alt+Down in Notepad with the source of "Non Client Input Sink Window" traced.
2. Pin today's defects with tests: Control+A followed by a repeated focus is announced; a superseded key's answer becomes Core's caret; a caret event's report and a key's answer are ordered by one stamp.
3. The last read and one request per key, the last read living while Core holds the node.
4. Remove the second layer listed above.
5. Keys that move nothing: the edge facts, the edge rule, the watch's bound answer, later evidence, and the watch's 1-second bound; the `settings_search_suggestions` expectations change.
6. The keys as NVDA: the editing chords from the hook to Core, and the sentence move with the rich edit control's sentence read; the `caret_keys` expectations change.
7. One stamp, `MoveCaret`'s and `Select`'s stamped answers, and the merge clause's removal.
8. The measurements and scenarios below, including `text_box_selection_keys` at Control+Delete over "alpha ", which failed in some runs when the watch answered from its first check, ten times.
9. The documents: D14 names the evidence judgement and the baseline; the key-holding design, `docs/parity/text.md` ("A caret key that moves nothing", "Keys outside the caret table") and `docs/parity/input.md`, and the crate guides for `verbatim-core`, `verbatim-outpost`, `verbatim-input`, `verbatim-text` and `verbatim-uia-rops` describe the result.

## Tests and measurements

New tests, one per behaviour:

- mockapp: a focus reported twice with Control+A between, Core dropping the second, "N characters selected" said once.
- Core: three keys whose first answer arrives after the third press, the first's change said.
- mockapp: a mouse selection made after the last report and before the before read, said before the key's words.
- The outpost: a key whose baseline's selection differs from the last read's, evidence by ends.
- Core: Up Arrow with the before read on the first line of two, nothing selected, speaks the line at once and holds no key behind it; with a selection, or in a one-line text, it waits for the watch.
- Core: Left Arrow at the text's start speaks its character at once; Backspace there says nothing at once and holds no key behind it.
- Core: an edge key's bound answer showing the same unit says nothing more.
- The outpost: a key with no evidence at its bound answers once with the caret then read, marked, and stays open; an application that moved the caret with no event has the moved caret in that answer.
- Core: a bound answer speaks the key's unit; for Backspace, nothing; with a later key pressed, nothing; with the focus on another node, nothing; Core's caret is taken in each case.
- Core: later evidence on another line cuts speech and speaks the line; later evidence with the same unit text at the same place says nothing; with a later key pressed or the focus moved it only takes the caret.
- Core: a report arriving while an edge key's watch is open is spoken at once.
- The outpost: the watch ends 1 second after the before read ended, and Verbatim's own caret move ends it.
- The outpost: `MoveCaret` answers with the move's stamp, and an older report arriving after it does not replace Core's caret.
- mockapp: Up and Down Arrow in a one-line combo box edit that replaces its text read the new text once.
- The outpost: a rich edit control's sentence read answers with the text between line breaks, a wrapped paragraph whole.
- Core: a key that moves nothing with a caret key held behind it has its bound answer taken as its caret, silently, before the key behind is read; with no answer, the key behind is released 250 ms after the key was passed on.
- Core: Alt+Down at a bound answer in a plain edit control's text moves the caret to the next sentence and speaks it; Alt+Up from a line's first sentence moves to the previous line's last; through UIA it says nothing; a key behind waits for the move.
- Core: Control+Z in a text field opens no watch and says nothing; in a terminal it is handled as today.
- The shell: a key's request counts as outstanding until its first answer and not after it.
- `crates/mockapp/tests/terminal_reading.rs`'s console poll tests, whose `WATCH_BOUND` is the old 10 seconds and one of which passes its key two seconds after the watch opens, are rewritten for the 1-second bound and the poll counted from the before read's end.

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
