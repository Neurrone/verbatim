# The caret and selection rework

Status: scope and order of work agreed by Dickson on 2026-10-25, to be built in a session of its own (`docs/roadmap.md`, M4, "The caret and selection rework"). The study below them was made the same day, read-only. Read: the description of the model as built at the end of this document, `docs/architecture.md` (D14 as amended), `docs/design/key-holding.md`, `docs/design/wake-ups.md`, `docs/design/phase6-design.md` ("What a key did to the text"), `docs/parity/text.md` and `input.md`, `docs/nvda/editable-text-and-terminals.md`, `docs/performance.md` (the caret entries), `docs/crates/verbatim-core.md` and `verbatim-outpost.md`; the code in `verbatim-core` (`editing.rs`, `reduce.rs`, `state.rs`), `verbatim-outpost` (`text/mod.rs`, `outpost/text_reads.rs`, `outpost/worker.rs`, `outpost/intake.rs`, `outpost/outbound.rs`, `outpost/held.rs`), `verbatim-model/src/text.rs`, `verbatim-uia-rops/src/caret.rs`, `verbatim-input/src/scripts.rs`; the mockapp and Core test names; NVDA's `editableText.py` 417 to 471, `behaviors.py` 331 to 349, `window/edit.py` 1068 to 1084, `UIAHandler/__init__.py` 902 to 917; and the 79 commits of this month on these files, read by their messages. Terminals are out of scope except where the mechanism is shared.

## What was found

The shape is right: a caret key is held while the focus's outpost reads the caret, as NVDA's caret scripts read the caret before sending the key and wait for it to move; the outpost then watches the application's events for evidence of what the key did and answers with facts compared inside the provider; Core chooses every word. On top of it sits a second layer, added before key holding was adopted on 2026-10-22 to reconstruct a key's "before" from time stamps, which holding made redundant and which was never removed. The caret and selection are kept in nine places, and two known defects come from copies disagreeing: Control+A followed by a repeated focus says nothing, and a superseded key's answer leaves Core's caret stale. A third: a caret event's report is stamped with the event's time in Core and with the read's end in the outpost.

## Decisions (Dickson, 2026-10-25)

- One last read per text node, held by the outpost and replaced by every read, is the only baseline. A key makes one request that reads the caret before the key and opens the watch with that read as the key's baseline. Every read reports facts. Core takes every report and every answer, late or superseded, as the newest caret by one stamp, the time the read began, and speaks from it.
- Removed: Core's history of eight caret reports and its prediction of what Backspace and Delete remove; the outpost's history of eight reads, now, so that until uiAccess (M8.1) a caret key pressed while an elevated window is in front is judged from the last read when the request arrives, and is sometimes silent; the time-zero record of Core's own caret move; the "since" check limited to a line; report numbers; the three modes of the selection baseline; the separate watch request and its first check; and the single superseded slot.
- The baseline's life follows the focus Core holds: the outpost forgets a node's last read when Core's focus leaves it, and Core speaks a repeated focus's changes and ignores a new focus's.
- Control+Z, Control+Y, Control+X, Control+V and Escape in a text field match NVDA, which binds no script to them: they are not held, and nothing is said for them. This replaces the decision of 2026-10-08 that spoke them; the `caret_keys` scenarios' expectations change.
- The judgement of whether a key's evidence has arrived stays in the outpost, named in D14 as the one read-time decision outside Core, because it bounds reads on text changes.
- One time stamp per caret report.
- The outbound merge's clause against merging a caret report across a reply is verified and removed, its stated reason predating 505e3ad8.

## Order of work

1. Pin today's defects with tests first: Control+A followed by a repeated focus is announced; a superseded key's answer becomes Core's caret; a caret event's report and a key's answer are ordered by one stamp.
2. The single baseline and one request per key, with the baseline's life following Core's focus.
3. Remove the second layer listed above, the outpost's eight-read history included.
4. The editing chords as NVDA: not held, silent; the `caret_keys` expectations change.
5. One time stamp; then verify and remove the outbound merge clause.
6. The measurements and new tests listed under "What must be measured before trusting it" below.
7. The documents: D14, the key-holding design, the parity entries on text and input, and the crate guides describe the result.

The study follows, then the description of the model as built before the rework. Where the study's options differ from the decisions above, the decisions hold.

## Terms

- Core: Verbatim's main process; the reducer is the part of it that decides what is spoken. An outpost is the helper process for one application that makes the UIA or MSAA calls into it and reports facts. A round trip is one cross-process call into the application; a remote program is a small UIA program run inside the application so that many reads cost one round trip.
- A caret report is the outpost's reading of the caret: the caret's line with the caret's offset in it, the selection's two ends as positions, and the line the selection starts in when that is an earlier line. It travels as `CaretMoved` after a caret event, as the focus's text in a focus message, or inside a key's answer.
- A held key is a caret-table key the keyboard hook swallows while the focus has text; Core has the outpost read the caret (the before read, `TextOp::ReadCaret`), handles the key as pressed just after that read, asks for the watch, then sends the key on.
- The watch (`TextOp::AwaitCaret`) is Core's request after a caret key: the outpost reads the caret once, then on each caret, text or selection event of the node reads again, until a read shows evidence that the key took effect, and answers with that read. Evidence is the caret having left where it was, the characters beside it changing, the unit at the caret changing after Delete, the caret's line changing after a key that deletes, or the selection changing.
- The baseline of a watch is the caret it compares with to find evidence. The kept selection is the selection the outpost holds per text node, against which the next comparison works out "selected" and "unselected". General terms are NVDA's words when the text changed since the kept selection was taken: the whole new selection "selected", "selected instead", or "selection removed".
- An anchor is a number the outpost gives the start of each chunk of text it sends; a position Core holds is an anchor plus an offset. The outpost remembers which provider position each reported position stands for.
- The last read, new in this study, is the one record per text node the outpost would hold: the caret and selection as provider positions, the selection's text up to 513 UTF-16 units, the caret's line text and offset, the note that the text changed, and the time the read began.

## The constraints that force complexity

Every mechanism below rests on one of these, or on an earlier choice.

- A key's effect can be seen only after the application handles it, and the application says so early, late, twice or never. Notepad raises the system caret's events and UIA's text selection changed for one move; the classic edit control raises its caret event before its value change; a key that moves nothing raises nothing; Windows Terminal sometimes moves its caret with no event. NVDA bookmarks the caret, sends the key and polls. Verbatim holds the key, reads, sends, and watches events.
- Two paths race. The key reaches Core through the hook; the application's event for that key reaches the outpost directly. Before holding, the report of the key's own effect could arrive before Core's request (96997861: the second Backspace in `notepad_editing` silent one run in three; 64a69b0c: Backspace saying "n" for "x"). Holding closes this for every held key, because the before read precedes the key and the watch is queued in the outpost before the key is sent.
- UIA text ranges are live: a position known before an edit moves with the edit, so after Backspace in a run of the same character neither the caret's position nor its neighbors change (0c678ebf, the password mask). A classic edit control's offsets do not move. So "the caret left where it was" is not enough evidence, and the characters beside the caret and, for deleting keys, the line must be compared.
- Reads are not atomic. A provider takes each call of a remote program on its own, and an edit control each window message, so the application can act between the caret's read and its line's (676d1275, 8ac783fe). The read must check the caret again before it is believed.
- Only the process that holds the provider's ranges can order two positions or read the text between them. Core holds anchors and offsets and cannot compare across chunks (the research's rejected option). So the selection's comparison, the change texts, and the "what the old range holds now" read must happen in the outpost, inside the read's round trip.
- Whether the text changed since the selection was taken is not observable in one read for a classic edit control: NVDA keeps a note set by the value change and read at the caret event, and the Windows Forms text box raises the value change after the caret event, sometimes while the read is being answered. Hence the note, and the check for a waiting value change behind a sync with the WinEvent thread.
- Speech is cut at every key press and NVDA's caret script says nothing once another script is queued. So an answer read after a later key keeps the caret current and says only its selection change.
- Focus moves at any time, and a focus can be reported twice (the outpost's own read after a UIA gap followed by the late UIA focus; a focus arriving while an earlier one awaits Core's judgment).

## The mechanisms as built, counted

Twenty-five, grouped by where they sit on the path from a key to speech. Each is named with what it does, why it exists (commit, date, Dickson's decision where there was one), what depends on it, and whether it is a root element, a parity port, a patch on another mechanism, or bookkeeping.

From the key to the request:

- The caret table and holding. The hook swallows a caret-table key while `holds_caret_keys` is set, Core has the outpost read the caret, handles the key as pressed one millisecond after the read, and passes it on with its watch already requested (fafc7210, c04584db, 2026-10-22; Dickson's decision that holding is the one mechanism). Everything below the key depends on it. Root, a parity port of NVDA's caret scripts' bookmark-then-send.
- Keys deferred behind a held key, in order, with the lock keys exempt (c04584db, d7ec6b04). Verbatim's own, one step beyond NVDA, where a typed character overtakes a held Backspace; Dickson's decision. Root.
- The wait for the key ahead's answer, bounded at 100 ms (c04584db, d7ec6b04, 2026-10-22; Dickson). Needed because Windows gives no signal that the application handled a sent key, and reading the next key's before as soon as the first was sent misjudged Shift+Right then Right in 10 of 60 rounds. The bound is for a key that moves nothing, whose watch never answers. Root, forced by the first constraint; the bound is bookkeeping on it.
- The guard for elevated windows: nothing is held while the foreground's integrity is higher (c04584db). Temporary until uiAccess. Bookkeeping; it is also the main reason a caret-table key is ever judged without a before read.
- The watch, checked on events and never waiting; a key that moves nothing is silent (f8d23cb2, 2026-10-08; Dickson's decision the same day, recorded as a deliberate difference from NVDA's 100 ms wait). Replaced a blocking wait that held every other event of the application for 100 ms. Root.
- The watch's ending: the next key's watch replaces it, a focus on another node ends it, a ten-second bound frees it, and the keys that never edit open none (f8d23cb2; 71ae5a04 after Pause held the idle barrier ten seconds). Bookkeeping.

The caret before the key:

- The key's press time on the watch and the outpost's history of its last eight reads, the baseline being the newest read that ended strictly before the key (96997861, 2026-10-06). For the race of the two paths, before holding. The watch's evidence depends on it. Patch, whose cause holding removed for held keys.
- Core's history of its last eight timed reports and `caret_before`, from which a key takes what Backspace will delete, Delete's unit to compare, whether something was selected, and the `since` it sends (64a69b0c, 2026-10-07). The same race, same date range, pre-holding. Patch.
- Core's caret chosen as the newest by observed time rather than arrival (ba7064e5, 2026-10-07): a key's late reply moved the review cursor back. Patch on the two histories' clock; its idea (one newest caret) survives in the simpler design with one stamp.
- Core's own caret move kept as a report at time zero, and the outpost forgetting its reads when it moves the caret itself (2ec69da9, 8a9a0c18, 2026-10-17): say-all's move followed by Right Arrow. Patch on both histories.
- Core's `since` in the watch and the rule that a deleting key's line is compared only when the baseline came from the outpost's own report (e2574b05, 2026-10-06; 1ad45f16, 2026-10-22, after a say-all paragraph was taken for the caret's line). `since` is the fallback when the outpost has no read before the key. Patch.

Evidence:

- The evidence kinds: the caret versus the baseline; the characters either side of the caret (e2574b05, after a rewrapped line and a late event gave false evidence); Delete's unit, cut from the whole line with its break (3124e96e, 2026-10-23); a deleting key's line (0c678ebf, the password mask); the selection versus the kept one (5223d5f7); and the selection in the baseline's report versus now, by ends, unless that report's selection is the one already compared, which the report numbers tell (11d08480, 2026-10-22, a Shift and click after a key that changed nothing). Root, each forced by the first and third constraints, except the last, which is a patch on having two selection baselines (the report's and the kept one).
- The recheck: the caret read again at the end of the program or after the parts, and a check whose caret moved meanwhile answers nothing (676d1275, 8ac783fe). Root, forced by non-atomic reads.
- A caret event alone is evidence only when nothing knew the caret, and a caret event waiting in the intake since the key counts as covered by the read (e2574b05; `Signal::covered`). Patch for late events; redundant once a before read always exists.
- A caret event observed before the node's caret was last read is dropped as covered (`caret_read_since`; e2574b05's "the wait tells the worker each time it reads"). Bookkeeping that saves a read; stays.

The selection baseline:

- The kept selection per node, replaced by every report and every key's answer, each change compared once by whichever reads it first (cff2a739, 2026-10-21; 5223d5f7 and 505e3ad8, 2026-10-22). A port of NVDA's `_lastSelectionPos`, replaced at every `detectPossibleSelectionChange`. Parity port, root.
- Its three modes: ignored by the before read, so the key's answer carries the comparison (fafc7210); restarted by the outpost's own focus report, clearing the note (cff2a739, 88541265); compared otherwise. The restart is the port of `initAutoSelectDetection`, keyed to the wrong event: NVDA drops a duplicate UIA focus before any object is made (`UIAHandler/__init__.py` 902 to 917), so its restart happens only at a focus it accepts; Verbatim's happens at a focus Core may refuse. Ignore and Restart are patches; Compare is the port.
- The selection Verbatim made or the caret it moved kept as the baseline (505e3ad8), so the application's report of it carries no change. Patch, needed in any design where Verbatim moves the caret.
- General terms: the note set by a classic edit control's value change and cleared by the next comparison with a selection; the check for a value change still waiting, made after the read behind a sync with the WinEvent thread (`Nudger::sync`, up to two wakes of that thread); and, through UIA, the kept selection's text compared with what its range holds now, up to 513 units (eaaafb8d, fb8a4ee7, ecd5f901, b563c80c, 5223d5f7, 11d08480, 2026-10-22; Dickson's decision that Notepad says "selection removed" where NVDA names the new text). Port plus Verbatim's own rule; forced by the sixth constraint. Root.
- Report merging in the outbound queue: two waiting reports of a node merge with both reports' changes in order, never across a reply, never in general terms (dd363ecc, eaaafb8d). The "across a reply" clause's reason (Core's own comparison already includes the change) predates 505e3ad8. Patch, half of it with an obsolete reason.

Core's words:

- The key's words: the unit at the caret; Backspace from the line before and after (`removal`), with the predicted deletion kept for a joined line; what comes before a selection's start for Backspace over it, from the caret's line or the selection's start line, and nothing when neither (505e3ad8); a key removing a selection says only the change (ecd5f901); keys outside the table speak removed text, the new line, or where the caret landed (phase 6 design, Dickson, 2026-10-08). Root decisions and parity ports.
- An answer read after a later key keeps the caret and says only its selection change (`later_key_at_ms`, the live checks of 2026-10-08; bf4fbedb for the cancel's ordering). Parity with NVDA's script giving way. Root.
- `heard`: a report's selection changes held while a key waits on the node and said after the key's words (5223d5f7, 505e3ad8). NVDA's order, script then caret event. Parity, root; it also follows from compare-once.
- The superseded slot: one query id whose late answer's selection change is still spoken (505e3ad8, after Control+Backspace then Backspace lost "alpha unselected"). Patch; its one-slot limit is the recorded three-key loss.
- The focus's caret carried in the focus message and taken silently for a duplicate (88541265, 2026-10-22). Root.

Beside these, the typed word's forgetting rules (2f9687ba) and typing echo from the hook touch the caret path only by cancelling and superseding; I leave them out of the count.

Of the twenty-five: twelve are root elements or parity ports; three are bookkeeping; ten are patches, and seven of those ten (the two histories, the newest-by-time rule, the time-zero record, `since` with its line restriction, the report numbers with the baseline-report comparison, the covered signal) exist for the race between a key's path and the application's event, which holding closed on 2026-10-22 for every held key.

## Where the state is duplicated, and where the copies disagree

Caret and selection state lives in: Core's current caret (`CaretContext`, with the selection and its start line); Core's eight timed reports (`TimedCaret`, each cloning the selection's start line, up to a 64 KB chunk); Core's pending key (`PendingCaret`: the predicted deletion, the line before, whether something was selected, whether the start is unknown, the words heard); Core's superseded slot; the outpost's eight reads per node (`CaretSeen`, with the selection's ends); the outpost's kept selection (`KeptSelection`, with the text and its report number); the note that the text changed; the outpost's record of when each node's caret was last read (`caret_read`, for covering events); and the open watch with its copy of the request. Nine places, five of them holding a selection.

The copies disagree in these concrete cases:

- Two stamps for one report. Core stamps a caret event's report with the event's observed time (`reduce_caret_moved` passes `observed_at_ms`) and a key's answer with the read's end (`read_at_ms`); the outpost's history stamps every read with its end, and its coverage record with its start. A caret event observed at time 100 whose read ends at 103, and a key outside the table pressed at 102 and not held: Core takes that report as the caret before the key (100 is before 102) and the outpost does not (103 is after), so the outpost's baseline is an older read, the first check finds the caret moved against it and answers with the state after the event, and Core compares a before and after that are the same state and says nothing, or the landing. A window of a few milliseconds, for un-held keys only.
- A superseded answer leaves Core's caret stale. `superseded_reply` (`editing.rs` 804 to 826) speaks the change and never calls `update_caret`, while the outpost has already made that answer its kept selection and its newest read. For a held key the next key's before read repairs Core; for an un-held key (an elevated window; two Other keys in a row) Core's `selected_before` and `before` for the next key come from an older report. Verified in the code; the research's claim holds.
- The baseline restarts at a focus Core refuses. `read_focus_text` (`worker.rs` 2020) calls `report_caret` with `CaretFor::Focus`, which maps to `LastSelection::Restart` and forgets the kept selection and the note before Core has seen the focus (`text_reads.rs` 597 to 601; `text/mod.rs` 1257 to 1260). Core's duplicate path (`focus_dropped_first`, `reduce.rs` 570 to 576) keeps the caret and says nothing. Control+A, then the repeated focus, then the watch's answer comparing with a baseline that already holds everything selected: nothing is said, where NVDA says "N characters selected". The outpost's own repeat test (`held.rs`) drops a repeat only when it repeats every focus Core holds or has yet to judge, so a focus reported while an earlier report awaits judgment is read and restarts the baseline. Structural; not reproduced live.
- Core's `since` names a chunk the outpost's line comparison must not use. After say-all, Core's position is in a paragraph chunk; 1ad45f16 had to restrict the deleting-line comparison to baselines from the outpost's reports. The two sides hold "the caret" in different units.
- Core's history is emptied by any untimed report (`update_caret`, `observed_at_ms == 0`), the outpost's by its own caret move (`MoveCaret`); the two empty at different moments, so "the caret before the key" is found by different rules for a few milliseconds after say-all moves the caret, which 8a9a0c18 then patched with the time-zero record.
- The kept selection's text is compared with what its range holds now only when the kept selection came from a read that stored text; a selection kept by the by-parts select path has none, so the general-terms rule does not run for it until its ends move (`docs/parity/text.md`, "Not yet built"). One baseline, two completeness levels.

In each case both sides are right by their own rule, and the rules differ because each side was patched for a race on its own clock.

## Which mechanisms exist only because of earlier choices

- Not holding keys (until 2026-10-22): the outpost's history and press-time baseline, Core's history and prediction, the newest-by-time rule, the time-zero record, `since` with its line restriction, the covered signal, and the report numbers. The design record of holding says it replaces "the per-case fixes of the caret problems that share one cause"; these are those fixes, left in place.
- Core sending its previous selection (until 505e3ad8): the outbound's not-across-a-reply clause, whose reason names a comparison Core no longer makes.
- The outpost owning the baseline's restart (cff2a739, 88541265): the repeated-focus loss.
- Two requests per held key (fafc7210): the watch's first check, which reads the caret again a few milliseconds after the before read, cannot find evidence because the key has not been sent, and costs one round trip per held key; the performance ledger already names it as a possible reduction.
- One superseded slot (505e3ad8): the three-key limit.
- The before read ignoring the kept selection (fafc7210): the Ignore mode, kept so that the key's answer, not the before read, carries a change that happened between reports. Core could speak the before read's change as a report's, in the order NVDA speaks it (the caret event's words come before the next key's).

## The simpler design

Stated as its mechanisms, each with the constraint or failure it answers.

- Holding, deferral, the wait for the key ahead and its bound: unchanged (the first constraint; Shift+Right then Right; the ten-second repeat). The caret table is held as now, plus, if Dickson chooses, a closed list of editing chords (Control+Z, Control+Y, Control+X, Control+V, Escape) so that every key Verbatim speaks for has a before read; other keys outside the table are judged from the last read at the request's arrival.
- One request per key: read the caret before the key, with the key's unit, and open the watch with that read as the baseline. The reply is the before read; Core passes the key on; the watch answers later with a second reply (one request, two answers; the request can carry both query ids). Reason: the before read is the first check (nothing has happened yet), so the separate watch request and its read are one round trip and one message per key for nothing. The outpost pins a copy of the last read as the key's baseline at that moment, so "before" is defined by construction, not found by time.
- One last read per node in the outpost, replaced by every read: focus, caret event, before read, answering check, and Verbatim's own select or caret move. Reason: only the outpost can hold provider positions (the fifth constraint); one record with one rule replaces the eight-read history, the kept selection, its three modes and its numbering. For a key that was not held, the last read at the request's arrival is the baseline (decision 3 says whether the eight-read history stays as a fallback until uiAccess).
- Every read reports facts: whether the caret and the selection differ from the key's baseline (evidence), how the selection changed from the last read (the words), whether the text changed since (general terms), the caret's line and the key's unit as they are now and as they were in the baseline, and whether the caret is on the baseline's line. All compared inside the one remote program, as `CaretQuery` already does with `since` and `previous_selection`; the baseline's selection is a third pair of ends, equal to the last read's in the common case. Reason: the third and fifth constraints. Core then needs no history and no prediction: what Backspace removed is `removal` over the before and after lines the answer carries, Delete's unit is the before unit against the after unit, "something was selected before" is the baseline's selection.
- The evidence judgment stays in the outpost, made from those facts, because it also tells the outpost when to stop reading on text changes (decision 7). It is named in D14 as the one read-time decision outside Core.
- The recheck: unchanged (non-atomic reads).
- General terms: unchanged (the note, the waiting value change behind the sync, the old range's text).
- Core speaks from every read it accepts: a report's changes at once, or held in `heard` while a key waits on the node; a before read's changes at once, before the key's words; a key's answer's words then its changes; a late or superseded answer as a report. Reason: compare-once means each change is said by whoever receives it, so no slot is needed and no change is lost.
- One caret in Core, the newest by its read's start time; a report carries that stamp. Reason: a late answer must not move the review cursor back (ba7064e5), and one stamp with one meaning removes the disagreement above.
- The baseline's life follows Core's focus: the outpost forgets a node's last read when `CoreFocus` leaves the node; a focus read compares with the last read when one exists and reports the changes with the focus; Core ignores them for a new focus and speaks them for a repeat. Reason: NVDA's `terminateAutoSelectDetection` on loseFocus and its dropping of a duplicate before `initAutoSelectDetection`; the eighth constraint.
- `later_key`, `heard`, `removes_selection`, the landing rule, the Backspace-over-selection rule with the selection's start line: unchanged (parity and Dickson's decisions).
- The ten-second bound and the idle barrier's wait on an open watch: unchanged bookkeeping.
- Event coverage (`caret_read_since`) and the system caret's dropping: unchanged bookkeeping.

Counted at the inventory's granularity: of the twenty-five, eight go (Core's history, the time-zero record, `since` with its line restriction, the covered signal, the three baseline modes, the selection kept after Verbatim's own move as a separate rule, the merging clause pending its check, and the superseded slot), one is added (the baseline's life following Core's focus), and the outpost's history stays only until uiAccess: seventeen, eighteen in the interim.

### How it handles each failure the current mechanisms were added for

- The second Backspace silent because its own event was reported first (96997861): the before read precedes the key and the watch is open before the key is sent; the key's event can only follow. By construction, for every held key.
- Backspace saying "n" for "x" (64a69b0c): the answer carries the before line from the outpost's pinned baseline, read before the key was sent; Core compares before and after from one source.
- The review cursor moved back by a late reply (ba7064e5): one stamp, the read's start; Core keeps the newest.
- A late caret event or a rewrapped line taken for evidence (e2574b05): a caret event is never evidence by itself; the characters beside the caret are compared; unchanged.
- A key that moves nothing holding the worker 100 ms (f8d23cb2): the watch; unchanged. Such a key now costs one round trip (the before read) instead of two.
- The edit control handling the key between two messages, the console host writing between two calls (676d1275, 8ac783fe): the recheck; unchanged.
- Say-all's move then Right Arrow (2ec69da9, 8a9a0c18): the outpost's own move becomes the last read; the held key's before read follows anyway; Core's caret comes from that read. No time-zero record.
- A selection the mouse or the program made (cff2a739): compared with the last read; unchanged.
- Keys over a selection compared once (5223d5f7): the last read is replaced by every read, so every change is compared by the first read after it; unchanged in effect, one rule instead of three modes.
- A selection moved before the watch arrived, left with no evidence (11d08480): the evidence compares the selection's ends with the key's baseline in the same program, always, no numbering needed; for a held key the baseline precedes the key's event, so the case cannot arise at all.
- The password mask (0c678ebf, 1ad45f16): the line comparison, always against the baseline's line, which is always a caret's line read by the outpost; the say-all paragraph case cannot arise.
- Delete at a line's end answered before Notepad deleted (3124e96e): the before unit is cut by the outpost from its own line with the break, one rule in one place.
- Shift+Right then Right misjudged (c04584db): holding and the wait; unchanged.
- A repeat held 150 seconds (d7ec6b04): the bound; unchanged.
- Control+Backspace that did nothing, then Backspace, losing "alpha unselected" (505e3ad8): the late answer is a report; said after the later key's words through `heard`.
- Control+X saying the cut text (ecd5f901): `removes_selection`; unchanged.
- Pause opening a watch (71ae5a04): unchanged.
- The repeated focus swallowing Control+A (the research's counterexample): the focus read compares with the last read and reports "N selected" with the focus; Core, dropping the focus as a repeat, speaks it or holds it for the waiting key; the key's answer then finds no change left. Said once.
- A superseded answer leaving Core's caret stale (the research's second finding): every answer updates Core's caret when newer.

### Per-key costs

- Calls: a held key through UIA makes two round trips (the before read, the answering check) instead of three; classically two reads of 8 and 15 calls instead of 8, 15 and 15; in a classic edit control 5 and 6 window messages instead of 5, 5 and 6. The before read now also compares the selection when one exists on either side (in the program at no round trip; classically two comparisons and the change texts, as a caret report pays today) and reads the key's unit when it is a word (one unit read in the program for Control+Delete and Control+Backspace; a character is cut from the line). A typed character's report costs what it costs today.
- Messages and wake-ups: one request fewer per held key (one pipe write in Core, one wake of the outpost's pipe reader, one worker item), and the first check's round trip with its worker wake-ups gone. Against the measured 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time per held key, I estimate 2 to 4 fewer wake-ups; it must be measured, not estimated.
- Latency: the answering read is the same read at the same moment, so key to first sound should be unchanged; the hold may shorten by the time Core spent emitting the second request.
- Bytes: the answer carries the before line (as long as the line, usually under a hundred bytes; capped at the chunk's 64 KB). Core's history no longer clones the selection's start line per report.
- Memory: one last read and one pinned baseline per node, against eight reads and a kept selection in the outpost and eight timed reports in Core.
- Remote operations: the before read and the answering check stay single programs; the baseline's selection is one more pair of ranges compared in-program. The caret read's worst case (4,462 instructions measured) must stay under 5,000, half the 10,000 limit.

### What it removes

In Core: `caret_history`, `CARET_HISTORY`, `TimedCaret`, `caret_before`, `caret_moved_by_core`'s time-zero record, `PendingCaret::{deleted, before, selected_before, before_unknown}` and `deleted_text`/`compared_text` (their work moves to comparing the answer's before and after), `superseded_caret` and `superseded_reply`, `CaretWatch::{since, compare, pressed_at_ms}` (the press time stays on the input for `later_key`). In the outpost: `CaretSeen`, `REMEMBERED_CARETS` and the search by press time (or kept as the un-held fallback until uiAccess, by decision 3), `KeptSelection::report` and `NodeAnchors::reports`, `LastSelection` and `CaretFor::Focus`'s restart, `Baseline::{selected, known_line}` with `compared_already`, `Signal::covered`, the separate `AwaitCaret` request and the watch's first check, `forget_selection` at a focus, and `MoveCaret`'s clearing of the history. Probably the outbound's not-across-a-reply clause (decision 8).

### What stays that someone might expect to go

- The before read itself: one round trip per caret key is the price of holding, accepted on 2026-10-22, and it is what makes everything else simple.
- The caret read on every caret event, typed characters included: NVDA makes a TextInfo at every caret event too, and it is how a program's or the mouse's selection is found. Whether its report to Core could be skipped when nothing changed is a wake-up question for another study; Core uses the caret for review following, the navigator's selected text, held anchors and the line-break kind.
- The WinEvent sync behind a classic edit control's comparison: the sixth constraint; no cheaper evidence of a pending value change exists.
- `heard` and `later_key`: NVDA's ordering and its script giving way.

### Comparison

- Today's design: meets the requirements but carries ten patches, seven of them for a race that holding closed; two known losses (the repeated focus, the three-key burst); nine places of state; three round trips and two requests per held key.
- The research's recommendation alone (baseline lifetime follows Core's focus; superseded answers update the caret): fixes the two losses with two small changes and leaves the two histories, three modes, two requests and the stale-clock disagreement. A correct minimum step, not a simplification.
- This design: the research's two changes, plus one baseline, one request, one stamp, no histories, no prediction, no slot. Fewer calls and messages per key. It undoes four commits of this month and reshapes a fifth, each for a cause holding removed. The one judgment outside Core is named and bounded.
- The D14-literal variant (the outpost reports every read's facts and Core judges evidence): rejected for this round. In the common case it costs nothing more, but the outpost must know when a key is answered to stop reading on text changes; without that it reads twice per typed character after any caret key, or needs a message per key. If uiAccess or a later protocol gives a cheap "key answered" signal, revisit.

### What must be measured before trusting it

1. Call counts (`crates/mockapp/tests/call_counts.rs`): the before read 1 remotely, 8 classically, 5 messages, now with a comparison when a selection exists; the answering check 1, 15, 6; no first check (a new pin that a held key makes exactly two reads through UIA); the caret report with a selection unchanged.
2. Instruction counts (`instruction_limit.rs`): the before read with a previous selection and a word unit, worst case under 5,000; typical near today's 97.
3. The stale caret workloads (`measure_stale_caret_text_box`, `measure_stale_caret_notepad`, `measure_stale_caret_conhost`): 0 of 60 misjudged, as at the merge.
4. Wake-ups and processor time (`cargo xtask trace scenario` at info level) over `measure_keys_text_box`, `measure_keys_windows_terminal` and a Notepad key workload, before and after: expect fewer worker items and outpost pipe-reader wakes per key; latency ledger P50 and P95 key to first sound unchanged.
5. Scenarios: every `text_box_*`, `notepad_*`, `wpf_password`, `settings_search_suggestions` and `settings_search_suggestions_reentered`, and `terminal_key_timing`, since the key protocol is shared with terminals.
6. New tests, one per behaviour: mockapp, a focus reported twice with Control+A between, Core dropping the second, "N characters selected" said once; Core, three keys whose first answer arrives after the third press, the first's change said; Core, a report's change arriving after a key's reply said once (decision 8); mockapp, a mouse selection between the before read and the key, said before the key's words; the outpost, a key whose baseline's selection differs from the last read's, evidence by ends.

## Follow-ups outside this decision, noticed on the way

- A typed character's caret report costs Core's pipe reader and reducer a wake each; whether a report that changed nothing Core needs could stay in the outpost is a wake-up question, not a caret one.
- A kept selection with no text, from the by-parts select path, is not checked for its range no longer holding its text (`docs/parity/text.md`, "Not yet built"); the last read would store text from every path that reads it.
- The outpost gates caret reports on its own followed focus (`Worker::is_focus`), not Core's; a focus-pipeline question the research also noted.
- Terminals share the key protocol; the merged request and the removal of `since` must be checked against the terminal module's use of the watch's answer as a read trigger (`terminal_output` after an answered check).


## The model as built, before the rework

Written 2026-10-25 from the code at c0f80a7e. Read-only research; nothing was built or run. Terminals are left out.

### The decision, and my recommendation

The question: should D14's rule "a judgment uses the focus Core holds, never a copy of what the outpost last reported" also cover the outpost's per-node selection baseline, which would mean Core sending its selection with every caret request?

Recommendation: no, not in that form. Keep the comparison and the baseline in the outpost. Instead, amend D14 to say what the baseline is and which part of it Core governs:

- The baseline is read state, not a judgment. It is Verbatim's version of NVDA's `_lastSelectionPos`: a range held inside the provider whose ends move as the text is edited. NVDA replaces it at every comparison whether or not anything is spoken. Only the process holding the provider's ranges can keep it or compare against it.
- What Core does govern is when the baseline starts over. Today the outpost starts it over whenever it reports a focus, and that includes a focus Core then refuses as a repeat. That is the pattern D14 removed for focus judgments. The fix is that the baseline's life follows the focus Core holds, which Core already sends to the outpost (`CoreFocus`), so no new message is needed.
- Two Core-side fixes go with it, so that Core's caret and the outpost's agree on the newest read. Both are listed under "What else should change".

Sending Core's selection with every caret request has three costs. It partly reverts commit 505e3ad8 of 2026-10-22, which removed exactly that field (`CaretWatch::previous_selection`) together with the bookkeeping it needed against double speech. It leaves caret events, the path for mouse and program selections, with no request to carry the selection. And Core's copy of the selection is ordered differently from the outpost's, and lacks the selection's text. Details are under "The options".

### Terms used here

- Core: Verbatim's main process. The reducer is the part of Core that decides what is spoken.
- Outpost: a helper process for one application. It makes the accessibility calls (UIA or MSAA) into that application and reports facts to Core.
- Round trip: one cross-process call into the application. A remote operation is a small program UIA runs inside the application, so that many reads cost one round trip.
- Anchor: a number the outpost gives the start of each chunk of text it sends. A text position, as Core holds it, is an anchor plus a byte offset. The outpost remembers which provider position each anchor and each position it reported stands for.
- Caret report: the outpost's reading of the caret. It holds the caret's line, the selection's two ends as positions, and the line where the selection starts if that is an earlier line. It is sent as `CaretMoved` after a caret event, or inside a reply.
- Watch: what Core asks for after a caret key (`TextOp::AwaitCaret`). The outpost waits, without blocking, for evidence of what the key did, then answers with the caret.
- Evidence: proof that the key took effect. This is the caret having moved, the characters beside it changing, the selection changing, and so on.
- Held key: a caret key the keyboard hook swallows while the focus has text. Core first has the outpost read the caret (`TextOp::ReadCaret`), then sends the key on.
- Kept selection, or the selection baseline: the selection the outpost keeps per text node. The next comparison works out "selected" and "unselected" against it.
- General terms: NVDA's wording when the text changed since the old selection was taken. It names the whole new selection with "selected", "selected instead" or "selection removed", instead of naming the exact text that changed.

### The model today, end to end

#### What each side holds

Core (`crates/verbatim-core/src/state.rs`):

- `CaretContext`, at line 162: the focus's current caret. It holds the caret's line, the selection as two positions, the selection's start line, and the kind of line break the text uses. It is chosen as the newest report by observed time, not by arrival order (`update_caret`, `editing.rs` 1146 to 1194).
- `caret_history`: the last 8 timed reports, each with its selection (`TimedCaret`, line 211). A caret key takes "the caret before the key" from it: the newest report observed strictly before the key's press time (`caret_before`, `editing.rs` 455 to 493).
- `pending_caret` (`PendingCaret`, line 229): the key whose watch is open. It carries what Backspace will delete, the line before the key, whether something was selected before the key, and `heard`. `heard` holds the words of selection changes that caret reports brought while the key waited.
- `superseded_caret`: one slot for the key a newer key replaced, so that its late answer's selection change can still be spoken.
- Held-key state (`HeldKeys`), the word typed so far, and the formatting last spoken.
- The anchors of the caret and the selection are listed in `held_anchors` (`state.rs` 960 to 975), so the outpost never forgets them.

The outpost, per text node (`NodeAnchors`, `crates/verbatim-outpost/src/text/mod.rs` 594 to 610):

- `carets`: its last 8 caret reports, each with its read time, caret, selection ends and report number (`CaretSeen`, 614 to 624). A watch takes its caret baseline from these.
- `selection`: the kept selection (`KeptSelection`, 627 to 640). It holds the ends as provider positions, whether nothing was selected, the selected text as reported (up to 513 UTF-16 units), and the number of the report it came from.
- `content_changed`: a note, NVDA's `hasContentChangedSinceLastSelection`. It is set when a classic edit control's value change is handled (`text_reads.rs` 696 to 702) and cleared by the next comparison.
- Anchors, and the positions it reported.

Besides these, the outpost has its own idea of the focus it follows (`intake.focused()`, used by `Worker::is_focus`, `worker.rs` 1476). It also holds the focus Core holds, together with focuses it reported that Core has not judged yet (`outpost/held.rs`, lines 1 to 18, from `SupervisorToOutpost::CoreFocus`).

So the selection exists in four places: Core's current caret, Core's history, the outpost's kept selection, and the outpost's report history. The two sides choose their "newest" differently. Core goes by observed time; the outpost's kept selection is simply the last one it read.

#### What arrives

- Keys, from Core's keyboard hook. A caret key on a text focus is held: Core asks for `ReadCaret`, takes the answer as its caret, handles the key as pressed one millisecond after the read, asks for the watch, and only then passes the key on (`editing::held_key` 233, `held_read` 403; `docs/design/key-holding.md`). Typed characters arrive as `CharacterTyped` and wait behind a held key. Keys outside the caret table that type nothing arrive as `CaretMotion::Other`.
- Caret events, at the outpost. These are UIA's text selection changed event, and for a classic edit control the system caret's location change or show and `EVENT_OBJECT_TEXTSELECTIONCHANGED`. Each becomes one caret read and a `CaretMoved` (`Worker::caret_of`, `worker.rs` 1486 to 1530). The intake keeps one waiting report per node and drops an event that was observed before the node's caret was last read.
- Text changed events, at the outpost: UIA `Text_TextChanged`, or a classic edit control's value change. The outpost checks any open watch with them and forwards `TextChanged` (`worker.rs` 2831 to 2853 and 3165 to 3188). For an edit control it also sets the note. For a text that is not a terminal, Core does nothing with `TextChanged`.
- Focus, from the listener or the outpost. The outpost reads the new focus's caret before it reports it (`read_focus_text`, `worker.rs` 2020 to 2062), and this read starts the selection baseline over (`CaretFor::Focus`, which maps to `LastSelection::Restart`, `text_reads.rs` 597 to 601).

#### Who reads what, and who decides

The outpost does every read of text, and it makes three decisions at read time:

- Whether a watch has its evidence (`evidence`, `text/mod.rs` 1756 to 1878). The caret baseline for that test is the outpost's own newest report read before the key, else Core's `since` (`baseline`, 1961 to 2024).
- How the selection changed from the kept one. The changed text is read in the same round trip: inside the remote program through UIA, call by call for an edit control (`caret_report_checking` 1250 to 1341; `answer_caret` 2029 to 2106).
- Whether that change is in general terms (`content_changed`, 1358 to 1381). This is true when the note is set, when the old selection's text no longer matches what its range holds now (Dickson's 2026-10-22 decision for UIA), or when a value change is waiting.

Each comparison then replaces the kept selection (`report_for`, 1543 to 1554). A held key's `ReadCaret` neither compares nor replaces it (`LastSelection::Ignore`, `text_reads.rs` 405 to 447). Verbatim's own Select and MoveCaret store what they made (`keep_selection`, `text/mod.rs` 787; `select` and `select_text`, 2647 to 2735).

Core decides the words. `caret_reply` (`editing.rs` 613 to 734) speaks the key's unit and then the selection's changes, with `heard` first (`changed_after`, 775). `selection_changed` (1116 to 1135) speaks a report's changes, or adds them to the waiting key's `heard`. `selection_segments` (1047) chooses the "selected" and "unselected" wording. Core also predicts what Backspace deletes from the caret before the key (`caret_key`, 115 to 219).

#### Case by case, for Notepad (UIA), a Settings text box (UIA) and the Windows Forms text box (classic edit)

- Left and Right Arrow, Home, End. The key is held, then the watch. Evidence is the caret leaving the baseline or the characters beside it changing. The answer carries the line and the character. Core speaks the character, naming a line break "carriage return" or "line feed" (`unit_segments`).
- Control with Left or Right Arrow. The same, but the unit is the provider's word, read in the same program. For a classic edit control the outpost finds the word itself with the shared word rules.
- Up and Down Arrow, the page keys, Control with Home or End: the caret's line. Control with Up or Down Arrow: the paragraph, or the line when the provider has no paragraphs.
- Typing a character. The echo comes from the hook (`character_typed`, `editing.rs` 1627). The application's caret event becomes a `CaretMoved`. With nothing selected before or after, no comparison is made and nothing is said. Typing over a selection gives the echo, then "selection removed": general terms, because the old range's text changed (UIA) or the value-change note was set (classic edit).
- Backspace. Core reads, from the line the held read found, what is before the caret. Over a selection it reads what is before the selection's start, which can be in the caret's line or the selection's start line (`caret_key` 141 to 160). The answer's line is compared with the line before the key (`removal`). For keys that delete, evidence also includes the caret's line differing from the line before the key, which covers a password field's mask.
- Delete. The watch carries the character or word at the caret (`compare`); its change is the evidence. Core speaks what is now at the caret, then, over a selection, the selection's change.
- Shift with any of these, and Control+A. There is no script of their own. The answer's selection changes compare with the kept selection, as a caret event's report does. Core speaks "x selected" or "x unselected", a single character by name, and 512 characters or more as a count.
- Selection cleared by a key without Shift. Core speaks the key's unit, then "x unselected". A deleting key over a selection says its own words, then "selection removed". Control+X and Control+V (keys outside the caret table) say only the selection's change (`removes_selection`, 840).
- Selection changed by the mouse or the program. The caret event's report compares with the kept selection and carries the changes. Core speaks them, or holds them in `heard` while a key on that node waits (`selection_changed`).
- A key's own caret event reported before its watch arrived. The report compares, and replaces the kept selection. The answer then has no change left (the watch still counts the selection differing from the baseline report as evidence, 1786 to 1802). Core says the report's words after the key's.
- Text changed by the program with no caret event: `TextChanged`, and Core says nothing. With a caret event (inline autocomplete that selects the completion), the report's selection change is spoken: specific terms through UIA, general terms in a classic edit control after its value change. NVDA would say the same words; this has not been checked live.
- Undo (Control+Z) and other keys outside the table: `CaretMotion::Other` with `landing`. Core speaks removed text, the new line if the caret changed lines, or where the caret landed (`other_key_segments`). NVDA says nothing.
- A browser field. No scenario covers one; through UIA it would take the same path as Notepad. Unverified.

### What NVDA does, and where Verbatim differs

From `docs/parity/text.md` (lines 21 to 470), `docs/nvda/editable-text-and-terminals.md` (lines 1 to 90), and `nvda/source/editableText.py` 417 to 471 and `NVDAObjects/behaviors.py` 331 to 345.

NVDA's model, as the source has it:

- Caret scripts bookmark the caret, send the key, and poll for up to 100 ms for evidence. They give up when another script is queued or a focus is pending, then speak the unit.
- For UIA text and classic edit controls, selection is never a key's concern. Every caret event calls `detectPossibleSelectionChange`. That reads the selection, compares it with `_lastSelectionPos`, replaces it unconditionally, reads and clears `hasContentChangedSinceLastSelection`, and speaks the change.
- `_lastSelectionPos` is set at `event_gainFocus` (`initAutoSelectDetection`) and cleared at loseFocus. It lives on the object, and a UIA duplicate focus is dropped before any object is made (`UIAHandler/__init__.py` 902 to 917), so a repeated focus does not reset it.
- `_lastSelectionPos` is a TextInfo, a provider range that follows edits. That is why NVDA names "what the old range holds now".

Verbatim matches that structure. One baseline per node, replaced at every comparison and set at a focus, sits in the process that holds the provider. Each change is compared once, by whichever read comes first.

The recorded differences:

- A key that moves nothing is silent, where NVDA speaks the unit after 100 ms (Dickson, 2026-10-08).
- A caret event alone is not evidence when the caret was known.
- Every intermediate change of two merged reports is spoken, where NVDA's event limiter keeps only the last (pending your acceptance).
- Through UIA, a selection typed over or deleted says "selection removed" where NVDA says "x unselected" (Dickson, 2026-10-22).
- Selections are counted in grapheme clusters, not code points.
- Keys outside the caret table speak their evidence.
- Verbatim reads every caret event of a classic edit control, where NVDA drops one whose selection offsets are unchanged.

### Is it coherent?

Mostly. For the selection there is one comparison baseline, the outpost's, and Core sends none. For the caret there are two copies, Core's history and the outpost's history. Held keys make them the same read: for a held key, Core's caret before the key and the outpost's baseline are both the `ReadCaret` result. Where they can disagree:

#### State duplicated between the outpost and Core

- The selection, in four places, chosen by two rules. The outpost's kept selection is the last one read. Core's `CaretContext.selection` is the newest by observed time. Core also does not take a superseded key's answer as its caret: `superseded_reply` (`editing.rs` 804 to 827) speaks the change but never calls `update_caret`, while the outpost has already replaced its kept selection and added the answer to its history. Core's next `selected_before`, Backspace prediction and `since` can therefore come from an older caret. Held keys hide this, because their `ReadCaret` refreshes Core's caret. It shows only for keys that are not held, as when a window of higher integrity is in front.
- The baseline advances when the outpost reads, not when Core accepts. Every way Core does not speak a compared change loses that change for good:
  - Core drops a `CaretMoved` because the node is not its focus (`selection_changed` 1122). The same happens for an answer whose node is no longer the focus (`caret_reply` 654 to 659).
  - Core refuses a focus as stale or by acceptance (`focus_dropped_first`, `reduce.rs` 562 to 579; `classify`).
  - The superseded slot holds one key. In a burst of three keys, the first key's change is not said; this is a known limit, recorded in `docs/crates/verbatim-core.md`.
  - Speech is cut by the next key. NVDA behaves the same way here, so this one is not a defect.

  After a focus change these mostly do no harm, because the next focus starts the baseline over anyway.

#### A concrete counterexample: a repeated focus

The outpost starts the baseline over at every focus it reports (`read_focus_text`, then `CaretFor::Focus`, then `Restart`, which calls `forget_selection` in `caret_report_checking` 1257 to 1260). That includes a focus Core will take as a repeat. Core's duplicate path keeps the caret and says nothing (`reduce.rs` 570 to 576, `keep_focus_caret`).

The sequence:

1. The application reports focus on text field N a second time. This happens when the outpost's own read for a missing UIA focus (`read_focus_after_gap`, `worker.rs` 1364) is followed by the late UIA focus. It also happens when a UIA focus arrives while an earlier one waits for Core's judgment; the listener skips its repeat test then (`focus_read.rs` 1285 to 1300).
2. Shortly before, the user pressed Control+A, and the watch is still open.
3. The outpost's focus read finds everything selected and makes that the kept selection, with no change reported.
4. Core drops the focus as a repeat and says nothing.
5. The watch's answer compares with the already-replaced kept selection, so it has no change. Control+A says nothing.

NVDA, which drops the duplicate before `initAutoSelectDetection`, would say "N characters selected". The window is small (a few milliseconds of unjudged focus, or the gap read's up to a second), and I have not reproduced it. The point is structural: the baseline's lifetime is keyed on the outpost's own report rather than on the focus Core holds.

#### Decisions made in the outpost

- When the baseline starts over: the outpost's own focus report. Per D14 this belongs to Core's focus. This is the one real misplacement.
- Whether a key has its evidence. The baseline prefers the outpost's own reports over Core's `since`. This is a read-time judgment that cannot wait for Core, and with key holding the two agree. I would leave it.
- How the selection changed, and whether that is in general terms. These are facts found while reading: positions compared inside the provider, plus the text of the changed ranges. Core chooses the words. I would leave them.

### The options for the selection baseline

#### Leave it as it is

What changes: nothing.

Safety: it matches NVDA's own model. Each change is compared once, nothing is spoken twice, and nothing costs extra. What it gets wrong is the repeated-focus case above, and that Core's caret can lag the outpost's after a superseded answer. Changes Core drops while the focus moves are lost, as they are in NVDA.

#### Core sends its selection with every caret request; the outpost compares with that

What changes: `CaretWatch` regains a selection field. That field is what 505e3ad8 removed on 2026-10-22. The old field needed `selection_spoken` so that a change already spoken from the key's own caret event was not spoken again; that bookkeeping would come back. This is a partial revert of recent work.

What it costs:

- Bytes: frames are JSON, so the field adds about 17 bytes when nothing is selected and about 80 when something is. That is negligible.
- Calls: none in the common case. Core holds the selection's anchors, and the positions are ones the outpost reported, so it finds them in its `reported` map.
- What Core lacks:
  - The selection's text as reported, which the UIA comparison needs to choose general terms. Core would have to carry and send back up to 513 UTF-16 units, about 1.5 KB, with every request, or name a report by number (the next option).
  - Selections Verbatim made itself. Core records its own caret move with nothing selected (`caret_moved_by_core`, `editing.rs` 1201) and never records the result of a Select.
- Ordering: Core's selection is the newest by observed time, which is not always the one the last spoken change ended at. Comparing with it can repeat a change that was already spoken.
- Caret events, the path for mouse and program selections and for a key's early caret event, have no request to carry the selection. Core would have to send its selection to the outpost after each report it takes. That costs one write in Core and one wake-up of the outpost's pipe thread for every caret report, which means for every typed character and every arrow key. The standing rule forbids raising wake-ups on the event path. The alternative is to send only when Core refuses a report, which is the next option in another form.
- Latency on the key path: none.

The result would be two baselines again, Core's for keys and the outpost's for events, which is what 505e3ad8 unified.

#### The outpost reports endpoints and the text around both; Core compares

What changes: the outpost sends the raw selection, and Core works out the difference.

What it costs: Core cannot order two positions in different chunks without the provider, and it cannot read the text between them. That rules out Shift+Down across lines, Control+A, and every selection longer than the lines Core already has. It also cannot read what an old UIA range holds after an edit. Each case it cannot settle would need another round trip (a `ReadRange`) on the key path: a few milliseconds and one more wake-up each. And general terms could not be judged at all. I reject this option.

#### Core names the report it accounted for (a variant)

What changes: each report already has a number (`KeptSelection::report`). Core would send, with each caret request, the number of the newest report whose selection change it has dealt with, spoken or deliberately dropped. Core would also send a short message only when it refuses a report that carried a change. The outpost would keep its last 8 kept selections, each with its text and note, and compare with the one Core names.

What it costs: about 15 bytes per key, nothing per event in the common case, and one message per refusal. It recovers changes Core dropped while the focus stayed the same, which in practice means only the three-key burst. NVDA loses that too. It adds a history of selections, a per-report version of the note, and the refusal channel. That is a lot of mechanism for one case that matches NVDA anyway.

#### Recommended: the baseline's lifetime follows Core's focus

What changes:

- At a focus report, the outpost compares with the node's kept selection when one exists, keeps the new read either way, and reports the changes with the focus's text as facts. It does not forget the selection first.
- The outpost forgets a node's kept selection when the focus Core holds leaves that node. It learns this from `CoreFocus`, which Core already sends at every focus judgment and whenever its focus enters or leaves the application. That is NVDA's `terminateAutoSelectDetection` on loseFocus. The forgetting is done by the worker at its next read of that node, by checking `Held`, so the pipe thread never takes the anchor lock.
- Core ignores the focus's changes for a focus it takes as new. For a focus it takes as a repeat, it speaks them, or holds them in `heard`, as for a caret event (`keep_focus_caret`).

What it costs: no new messages and no new wake-ups. A new focus has nothing kept, so its read compares nothing and costs what it does today. A repeated focus with a selection costs one comparison inside the read's round trip, the same as a caret event's report.

### What else should change for the model to be coherent

- Core takes a superseded key's answer as a caret report (`update_caret` in `superseded_reply`, which today only speaks the change). Core's caret and history then hold the newest read, as the outpost's do. This is Core-side only and costs nothing.
- D14's "Where the rules run" should say:
  - The text selection's baseline, like the provider's ranges, is held where the reads are made, and is replaced at every comparison.
  - Core decides when it starts over (at the focus Core holds) and whether a compared change is spoken.
- `docs/crates/verbatim-outpost.md` ("Text and caret") and `docs/crates/verbatim-core.md` should list the selection state on each side, and which side is authoritative for what. The outpost owns what the selection was at the last comparison. Core owns what the caret and selection are now, for prediction, review and held anchors.
- Optional, your call: replace the single `superseded_caret` slot with the few query ids of every superseded key on the node, so no compared change is lost in a burst. NVDA loses these too, so this goes beyond parity.

Follow-ups noticed, not part of this decision:

- With every caret key on a text focus held, Core's caret before the key and the outpost's 8-report history give the same read. The outpost's history now matters only for keys that are not held, as when a window of higher integrity is in front. It may be a second mechanism worth retiring under "one mechanism over per-path speed".
- The outpost's caret reports are gated on its own followed focus (`Worker::is_focus`, `worker.rs` 1476), not on Core's. This matters only when Core refused a focus the outpost adopted, which is a focus-pipeline question rather than a selection one.
