# Design: terminals (build 3 of ADR 0003)

Status: approved by Dickson on 2026-11-16, redesigned that day around his rulings (option C; M6.4's first item brought in; the goals audit's uniform late-answer rule and terminal types), after the adversarial review, and amended for the review of the reworked builds. No question is open for Dickson. Builds 1, 2 and 3 are built in number order and pushed together (Dickson, 2026-11-16), so build 1 may name build 3's mechanisms and no product state exists between them; the terminal rules of the two designs are reconciled and say the same. Written against main 1a0b4c0e, NVDA pinned at 732d1ce2f0f051da3ef1e6a6247c64cf04596337. It is built (c88df424 and 89c36cc8, 2026-11-16). It builds on build 1's design (`docs/design/readers-tell-core-asks-build1.md`), which now holds the keys-and-text design's caret key, watch and held anchors, as designed, not as today's code. Where this design and ADR 0003 differ, ADR 0003 wins, except where a ruling of Dickson's below amends it.

Sources: [ADR 0003: Readers tell, Core asks](https://github.com/Neurrone/verbatim/blob/main/books/dev/src/reference/adr/0003-readers-tell-core-asks.md) ("the ADR"); `docs/design/readers-tell-core-asks.md` ("the build notes"), its "Build 3", "Core's rules" on terminals, the traps under "Terminals" and "Measurements"; [How Verbatim reads terminals](https://github.com/Neurrone/verbatim/blob/main/books/dev/src/explanation/design/terminal-model.md) ("the terminal model"); build 1's design; the goals audit of the reworked builds; `uia-facts/a-provider-process-serves-one-uia-request-at-a-time`. Code read: `crates/verbatim-outpost/src/terminal.rs`, `terminal/reading.rs`, the worker's terminal paths (`outpost/worker.rs` 600 to 620, 1540 to 1940; `outpost/text_reads.rs` 1207 to 1360; `outpost/shell.rs` 100 to 125; `verbatim-outpost-logic/src/intake.rs`), `crates/verbatim-core/src/terminal.rs` (the flood policy), `terminal_model.rs`, `terminal_model/review.rs`, `crates/verbatim-model/src/text.rs` 724 to 890 and `terminal.rs`, `crates/verbatim-app/src/key_reports.rs` 160 to 185, and the terminal tests. NVDA paths are under `nvda/source/`.

Every rule is marked "NVDA:" with its source, or "Verbatim-only:" with its reason. The terminal model is Verbatim-only as a whole (the ADR, "What Verbatim keeps beyond NVDA"). "Today" names a rule kept from today's code.

## Dickson's rulings of 2026-11-16

### The outpost keeps no "changed" flag

The decision it replaces, Dickson's of 2026-11-12 ("Readers tell, Core asks"): "A reader keeps nothing for Core between requests but held objects, the requests waiting in an outpost's table and a terminal's 'changed' flag, and judges nothing about them." With it goes, in "What this decision adds and removes", "A terminal's 'told, not yet read' in Core and its 'changed' flag in the outpost. Held in two places".

The ruling: the outpost keeps no "changed" flag and tells Core of every terminal change. The facts that changed: the measurement of 2026-11-14 (the build notes, "Measurements") had Core keeping up with about 3,000 to 4,000 tells a second; and the adversarial review of this design found the two copies disagreeing in five sequences (M1 to M5 below), each leaving a terminal silent. The added tells and wake-ups are measured in this build.

### Terminal reads are asked outside the event queue (option C)

The line it changes, from the build notes ("Build 3", as it read before this ruling): "Core asks a full read, as an event-queue entry: at a group's last line handed to speech ..., at the terminal taking the focus once its `Follow` is applied, after a caret key's watch, and at once when the policy is idle." The fact that changed: NVDA reads a terminal on a monitor thread of its own as changes come, outside its event queue, and queues only the speech (`NVDAObjects/behaviors.py` 466 to 500: `_monitor` reads with `_getText` and calls `queueHandler.queueFunction(queueHandler.eventQueue, self._reportNewLines, outLines, _immediate=True)`); a read held behind the event queue's front speaks output written before a cut that NVDA's cut silences (review, M8).

The ruling: Core asks a terminal's reads for its output outside the event queue, as it already asks for its dialogs and the control plane; the outpost's one worker makes them; each read's speech joins the event queue in order. A terminal's "changed" tell is handled at once in Core's inbox, as the ADR lists it. While a read is out, further tells collapse into one more read after it; nothing waits behind the front. D9 stays as it is: a separate reading thread gains nothing, since a provider process serves one UI Automation request at a time (`uia-facts/a-provider-process-serves-one-uia-request-at-a-time`).

Accepted as a limit (Verbatim-only, with no NVDA counterpart): output written just before Control+C during a command mark's search can be spoken, since the read waits for the search; splitting the search into shorter remote operations is a later option if it matters in use.

### Searches and history reads count from a start Core names (M6.4's first item)

Brought into this build from `docs/roadmap.md`, "M6.4 — Terminal residuals": "Every terminal read names the read its start was counted from, so that Core and the outpost agree on the last read without each tracking it unnamed." The start of every full read, history read and search is an anchor Core names; a history read or search out keeps an anchor at that start in Core's terminal memory, so a read taken meanwhile does not end the command. Remaining limit: more than about a screen of output during a search ends the move silently, as history reads do today.

### The goals audit

- The uniform late-answer rule (build 1, "Requests in Core", and its Table 18): every late answer is dropped by number; a request to a hung window answers "not responding" unsent; the window joins the recovery set, and when it answers again Core re-reads. The terminal's late-answer path and the hung-window send exception go.
- One type for the focused terminal's life in Core, and one for its reading (Tables 1 and 2), so the invalid combinations the review found cannot be written.
- The terminology "the ancestors Core already knows" and "the focus's ancestors" replaces "stop points" and "line" for a focus walk; this design uses neither for terminals, and "line" here means a terminal's line of text.

## Terms

The ADR's terms and build 1's (entry, request in Core, outcome, withdrawn, a focus is pending, starting read, final read, watch, late answer, recovery set) are used as they define them. Core "stops waiting" for a request where earlier documents said it gave the request up; "a stop" is the event. This design adds:

- Full read: a read of a terminal from a screen above a named start to the document's end, with the screen, the caret and its line (the terminal model, "The read"); every read the reading rules take is one. A history read and a search are not full reads.
- Output read: a full read Core asks outside the event queue for the terminal's own reading: after a change tell, at a release, after a cut, for owed colours or an owed scope, and the baseline. A caret key's starting and final reads and a review command's fresh read are full reads that entries ask inside the queue.
- Change signal: an event that says a terminal changed: Windows Terminal's UI Automation text-changed and caret events of the terminal's element, and the console host's five console events (the caret and the four updates) of the terminal's console window.
- Change tell: the outpost's tell of one change signal of the terminal `Follow` names.
- Covered: a change observed before a full read began is covered by that read, since the read shows it. Verbatim-only: today's `Terminal::covers` (`verbatim-outpost/src/terminal.rs` 93 to 99), moved to Core; both times are on the outpost's clock (the shared clock), in microseconds.
- Speech entry: the event-queue entry that carries a full read's speech (its answers to keys and its output) to its turn.
- The flood policy: live (read as changes come) or holding (read at a group's last line). Today it is split between Core's `held` and the outpost's `Reading`; here it is Core's alone.
- Release: the flood policy letting a held change be read, at a group's last line handed to speech or when the backlog empties. A read sent for a release is "waited" (the terminal model, rule 17).
- Owed scope: a full read the reading rules ask for in place of a fresh one: the whole document after a lost read, or the first read again with its history block.
- Start anchor: the provider range at the top of a full read's screen, minted by that read's answer and held by the outpost behind an id Core names (build 1, "Held anchors"); the next full read, history read or search starts from the one Core names.

## The decisions this design honours

Quoted from the ADR, as amended by the rulings above; each part below was checked against them.

- "Handled at once: answers, timers that come due, ... a terminal's 'changed' tell ..."
- "Requests from Verbatim's own dialogs, the control plane and an extension host's reads stay outside the event queue, answered in the inbox and holding no key. They keep their due time and the live hung check but are not given up when the foreground changes."
- "Every other tell, every key and command, ... and entries Core makes itself ... join the event queue and are handled in arrival order. Only the front entry may have a request out."
- "Core holds every rule that decides what is spoken, written to read like NVDA's, and its thread never blocks on an application."
- "An outpost has one worker making every call into its application, one at a time" (D9 unchanged).
- "Objects an outpost holds sit behind ids Core names, each carrying the generation of the store that holds it, and are released when Core releases them."
- "What goes, about 40 names: ... the terminal's hold, cancel, shape and colours messages ..."; among what is removed, "the terminal's hold and its reading states".
- Core to an outpost: "a request from the shared list (... terminal reads ...)".
- "The outpost keeps a limiter for what it still hears."
- "Core's logic is written as async functions run on Core's thread ... only a request to a reader can be awaited; ... every request gets exactly one outcome, so no paused function outlives its entry ..."
- The not-responding sound plays once for each request Core refuses, or stops waiting for, because its window is hung, while its application holds attention ("A frozen window").

Rulings of 2026-11-15 honoured: speak exactly what NVDA speaks, no more and no fewer (the terminal model is Verbatim-only; this build changes no expected speech; its two new behaviours are listed under "Exact expected speech"); no feature silent between builds; no interim mechanism written only to be removed.

## The model

A terminal is read only when Core asks. The outpost hears the terminal `Follow` names and tells Core of every change signal, with when it observed it; its limiter merges tells of the same terminal and kind still waiting on its pipe.

Core takes a change tell at once in its inbox. The focused terminal is then told; if no output read of it is out and the flood policy is live, Core sends one at once, outside the event queue; if one is out, the change waits for that read's answer and is read after it unless the read covered it; if the policy is holding, the release at the group's last line reads it. The outpost's worker makes every read, the event queue's requests first, as build 1 orders them. Each full read names its start anchor, the screen's height and width from Core's model, whether to take the colours, whether its window is a console's, and for a caret key's starting read the key's unit, so it also mints the key's caret starting point. Its answer is taken by Core's terminal model at once, in the order the worker made the reads, and its speech joins the event queue as a speech entry, spoken at its turn: the answers to keys, then the output through the flood policy, unless the read began before the last cut or the read is silent.

A cut drops output read before it, exactly as today. While holding, changes told but not read when the cut comes are read by a silent read, which speaks only its answers to keys, as today's cancel read did. A read the reading rules want in another scope is sent at once after the answer that asked it. The focused terminal's life is one value in Core (Table 1): its baseline read, waiting for its window after a hang, then modelled with its reading (Table 2). A hung window's read answers "not responding" unsent, the window joins the recovery set, and the terminal is read again when it answers.

## Rules, each with its reason

### The outpost: change tells

- A change signal of the terminal `Follow` names is told to Core as one `Event` tell of kind "terminal changed", naming the terminal's node, its window and when the signal was observed; it makes no call. Reason: Dickson's ruling; the ADR's one tell shape, the kind declared once in build 1's event-kind table with its teller, the outpost. NVDA: `NVDAObjects/behaviors.py` 420 to 424 (each change sets the monitor's event).
- The outpost's limiter merges a change tell with one of the same terminal and kind still waiting on its pipe, the newest keeping its place. The ADR ("The outpost keeps a limiter for what it still hears"); NVDA's limiter's merge, `orderedWinEventLimiter.py` 63, 79.
- A change signal of anything else is dropped: a console event whose window is not the followed terminal's console window, a Windows Terminal event of another element. Reason: only the committed focus's terminal is read (the terminal model, rule 15).
- What goes from the outpost: `Reading` and its transitions (`terminal/reading.rs`, whole); the screen's height and width (`terminal.rs` 54 to 57); the colours window (58 to 65, 126 to 138); the read start's coverage (68 to 71, 93 to 99), moved to Core; `read_ended_ms` and `read_colours`; the caret last sent (76 to 80); the identical-read drop and its fingerprint (81 to 83, 140 to 244), moved to Core; the kept start's read number and the rule 10 choice (`start`, `start_read`, `showed_text`, `number`, 41 to 53, 160 to 174), replaced by start anchors Core names; build 1's kept terminal queue (`intake.rs` 17 to 42, `worker.rs` 1550 to 1590); `terminal_event`, `terminal_output`, `terminal_read_now`, `terminal_event_of` (`worker.rs` 610 to 617, 1672 to 1717, 1881 to 1922). What stays: the read program, the text conversion (`unpadded_utf16`, `snapshot_of`), the history read and the search, each now from a named start.

### Core's inbox: the change tell

- A change tell for the focused terminal is applied to its state at once (Table 1, column D; Table 2, column A); a tell for any other node is dropped. The ADR (handled at once); Dickson's ruling (option C). NVDA: the monitor reads as changes come (`behaviors.py` 474 to 485).
- An output read is sent only when no full read of the terminal is out, an entry's included; a change told while one is out is kept as the newest change time, and after that read's answer the terminal is read once more unless the read began after it; a change the last read already covers sends nothing (today's `covers`). Review of the reworked builds, S8: a terminal caret key's own change tell no longer sends a read the key's final read covers. Reason: the worker reads one request at a time anyway, and one read after the current one covers every change told meanwhile; NVDA's monitor reads again once its read returns if the event was set meanwhile (`behaviors.py` 477 to 485).

### The full read request and its starts

- One request kind in the shared list, "read the terminal": the node; the start anchor to read from, or none for a first read; the scope (fresh, the whole document, or the history block with the height the first read found); the screen's height and width from Core's model (0 when Core has none, when the read takes only the screen, as a first read does today); whether to take the colours; whether the terminal's window is a console's, Core's fact from the committed focus's answer (build 1), which replaces the outpost's `console_focus` (`text_reads.rs` 484 to 495); and, when a caret key's entry sends it as its starting read, the key's unit, so the answer also mints the key's caret starting point (build 1, "Starting point"), a held anchor of the caret and selection ends, which the watch's checks compare with. The request's window, for the hang check, is the terminal's. Its answer is today's `TerminalSnapshot` with the id of its own start anchor, its caret report (whose anchor is a held anchor as any caret report's), the starting point's id when one was asked, and its start time in microseconds (today's `started_us`), which coverage compares with a change's observation, also in microseconds. Reason: every fact a read needs that Core owns travels in the request and is used only inside its answer (D14 as amended); the caret Core last took is no longer sent, since nothing in the outpost needs it (review of the reworked builds, M2).
- Which start a read names is Core's rule 10: the start anchor of the last read the memory took whose screen showed text; a read whose screen shows no text where the last one had text changes nothing but the caret, and its start is let go (the terminal model, rule 10). Core holds the named start, and any a history read, search or full read out names, in `NodesHeld`; the outpost lets go of the rest by build 1's rules for held anchors (its Table 16). Reason: M6.4's first item; Core and the outpost agree on the last read by name, and a read Core drops, as a late answer, moves no start.
- A full read whose named start is no longer the memory's when its answer arrives (a key's final read named at the pass, an output read queued behind queue requests, while other reads were taken) is given to the rules as reading from a start other than the last read's (`from_start` false), so its rows are found by their text, as after a refused start (the terminal model, rules 2 and 4). Reason: review of the reworked builds, S7; no state is kept for it, and its read still covers the last read's rows, since it begins a screen above an older start.
- A history read (`TextOp::TerminalHistory`) and a search (`TextOp::TerminalFind`) name the start anchor they count from, the history read also the screen's height. When Core sends one, it puts an anchor in its memory at that start's row, moved with each read the memory takes as the review position is (the terminal model, rule 24); the answer's rows are placed from that anchor's row; an anchor whose row left the memory ends the command silently, marks kept. Reason: the ruling; a read taken while the request is out no longer ends the command, only more than about a screen of output does, as history reads today.
- The answer's `waited` field is set by Core from the read's speech mark before the rules and the control plane see it; the outpost sends it false; the field stays so that control protocol version 4's shape does not change. The `read` number field stays, filled by the outpost's count, and Core no longer judges by it.

### Answers and speech

- Every full read's result of the focused terminal's current model generation is taken by the model at once, in arrival order, which is the worker's order: today's `take_snapshot`, its keys first. Reason: the reading rules align each read against the last one taken, and the worker makes reads one at a time, the queue's requests first (build 1, Table 14), so arrival order is read order.
- Its speech joins the event queue as a speech entry (Table 3). At its turn, while the terminal is still the focused terminal and its model generation holds: the answers to keys are spoken at once, and the output goes through the flood policy, unless the read's mark is silent or it began before the last cut (today's `cut_at_ms`). Otherwise the speech entry ends silent. Reason: Dickson's ruling (each read's speech joins the event queue in order); NVDA queues `_reportNewLines` on its event queue.
- An answer to a caret key's or review command's read resumes that entry's function, as build 1 describes, after the model has taken it the same way. Before that entry speaks anything, the speech entries of the same terminal that wait in the queue ahead of the first focus-kind entry are spoken, in order, each by Table 3's rule, then this read's own speech, then the entry's own words (a terminal caret key has none; a review command's answer). Reason: review of the reworked builds, M5; the model took the reads in the worker's order, so their speech must keep it, as NVDA speaks every `_reportNewLines` in its monitor's read order (`behaviors.py` 488 to 493).
- A full read whose rules ask an owed scope is followed at once by an output read in that scope (Table 4), whichever entry or output read asked it; no entry waits for it. Reason: the owed read is part of the same read (the terminal model, "A lost read"), and reads outside the queue need no place in it, so the review's M7 (a scope inside a caret key's entry) has no case left.
- The look at the backlog after a release waits for the release read's answer and its own last line heard (today's `deciding`); with an owed whole read after the release, it waits for that too, deliberately, so the group decided includes the lost read's count (`conhost_scrollback_overflow_during_group`).

### Outcomes and the uniform late-answer rule

- Refused or failed: the change told stays unread until the next change tell (as today's outpost read again at the next change); one more read is sent only if a change was told while it was out. Reason: a read that keeps failing would loop at Core's pace.
- Not there: the terminal is gone; Core forgets its model and the focused terminal's state.
- Core stops waiting at the due time (11 seconds): Cancel "due time" (the worker's call is left alone, and a late answer is dropped by number), and one output read is sent again. Reason: the outpost may have dropped the read unstarted, so the change must be read; at most one retry per 11 seconds.
- Hung, at the send or while out: "not responding", the sound when the application holds attention, the window joins the recovery set (build 1, Table 18), and the terminal's reading waits (Table 2, stalled) until the window answers again; then Core sends `Follow` again and one output read, which covers everything written meanwhile. The uniform late-answer rule (the goals audit, adopted by Dickson). Verbatim-only, a deliberate difference decided by Dickson (2026-11-16): the output is spoken about half a second after the window answers again, with the not-responding sound while the terminal holds attention; NVDA's monitor read blocks in the hung call (`behaviors.py` 478 to 492) and NVDA itself hangs badly there, so no capture is taken.
- Every late answer is dropped by number; since starts are named by Core, a dropped read moves nothing. Review, M3, has no case left.

### Cuts

- Every speech entry whose read began before the last cut has its output dropped and its answers to keys spoken. Verbatim-only: today's start-time rule (`terminal.rs` 285 to 290), the terminal model's "output read before the cut is never spoken after it while output read after it is".
- While holding, a cut with a change told sends a silent read at once, which speaks only its answers to keys, and makes the policy live; a release read out at the cut becomes silent. Verbatim-only: today's `TerminalCancel` read (`reading.rs` 82) and overtaken request (`terminal.rs` 1162 to 1196). Reason: holding is Verbatim's; NVDA, which never holds, read those changes before the cut, so its cut drops them; the silent read gives the same outcome.
- A cut while live adds nothing: the next read begins after the cut and is spoken.
- The order of the cut and the key: the hook posts `SpeechCancelled` before the key's report on the same port (`verbatim-app/src/key_reports.rs` 168 to 183). Today.

### Colours

- A full read takes the screen's colours when Core sends it within 300 ms of the newest key's time in the terminal (`KEY_WINDOW_MS`), or when colours are owed. Colours are owed, derived each time with no stored state, when the last full read answered ended at or after the newest key's time, within its window, without colours, and no full read has been sent since; then an output read is sent at once unless one is out. Verbatim-only: the terminal model, rule 23 and "Lists and colours", restated on Core's send time in place of the outpost's read start; the per-key `TerminalColours` request goes.

### Keys and ordering

- Each key's time is recorded in the focused terminal's model when it reaches Core's inbox (build 1), a held caret key's at its pass, and given to the rules before a read that ended at or after it (the terminal model, "Keys"); the rules order by time, not arrival.
- Typed echo: a typed character reaches the program at its press; the terminal's change tell arrives; the output read is sent at once, made by the worker when free; its answer's echo is spoken at the speech entry's turn, behind whatever entries arrived before it, as NVDA's queued `_reportNewLines` is.
- The caret watch in a terminal (build 1, "A terminal's caret key"): the key's starting read and final read are full reads of this kind, queue requests the worker makes first; the watch's checks are queue requests too; an output read asked meanwhile is made between them, in the worker's order, and each answer is taken in that order. A later key withdraws the watch (build 1); a final read already running runs on and its answer is dropped by number, which loses nothing: the change it shows was told and is read by the next output read.
- Focus changes: the speech entries of a terminal read behind a committed focus that left the terminal end silent at their turn (rule 15: output written while the focus was elsewhere is never heard); an answer arriving after the focus left is not taken; a new focus on the terminal reads a fresh baseline (Table 1).
- Outpost replacement: the generation's models and the focused terminal's state go with it (today's `outpost_ended`); answers of the old generation are dropped by number; the re-take entry (build 1) reads the focus, and a terminal focus starts again at `Follow` (Table 1).
- Read speech carries the newest key number at its speech entry's arrival (build 1's speech fence); the cut rules above, not the fence, decide which output read around a cut is dropped. Test: a read sent before a key and answered after it, inside and outside the key's window.

### The focused terminal's life

- A terminal focus committed, new or taken again after the focus or the readers' last focus left it, sends `Follow` naming it (build 1) and, at once after it, the baseline: a full read with no start (a first read), carrying Core's height and width when it has a model of the terminal, so a return to a modelled terminal includes its history block. The baseline is sent in `Follow`'s order class (build 1, Table 14: the event queue's requests and `Follow` in sending order, ahead of requests from outside the queue), so the worker reads it after `Follow`'s registration and before any later entry's read. Its answer takes the rules' silent first read after the focus (rule 15). Reason: registration before the read, as build 1 has it; no entry's read can run before the baseline, so a key pressed just after the focus is heard as its effect (review of the reworked builds, M4); a re-take reads a baseline (review, M4); Core's parameters on the baseline (review, M6). Amends build 1, which read the baseline inside `Follow`'s answer.
- A baseline that fails, or that Core stops waiting for at its due time, leaves the terminal baselining with no read out; the next change tell sends it, and an entry's full read answered meanwhile is taken as the baseline, the silent first read. Reason: no loop, and the terminal recovers at its next change or key (review, M5).
- `Follow` answering "not there" or "refused" ends the focused terminal's state. `Follow` stopped at its due time is sent once more, with the baseline if that has not answered (review of the reworked builds, S6). `Follow` or the baseline refused unsent or stopped as hung leaves the terminal waiting for its window, its told time kept. When the recovery set finds the focused terminal's window answering again, Core sends `Follow` again whatever the terminal's state, since a resent `Follow` may be the request that hung, and reads it by Table 1, column J: waiting for its window, or baselining with no baseline out, the baseline is sent; modelled, its reading takes Table 2, column K. It does not wait for the re-ask to commit the focus (review of the reworked builds, M3: a re-ask that finds the same focus commits nothing). Build 1, "Requests in Core" and its Table 18, column B, say the same.

### Review commands as chained requests

- A review command in the focused terminal is one entry: it sends a fresh full read (a queue request), answers from Core's memory, and asks the history read or a mark's search (at most three a press) as follow-ups under the command's one due time; the answer from memory makes no request. What goes: the model's queue of waiting review commands and its `history`, `found` and `mark_searches` kept between inputs (`terminal_model.rs` 105 to 162, 179 to 192), `intercept`, `queue`, `queue_only`, `answer_reviews`' loop, `ModelAsk` and `Then::TerminalModel` (505 to 551, 970 to 1406). Reason: the ADR's async form keeps a chain in one function.
- A fresh read that brings no snapshot still answers from the memory as it stands (today, 1000 to 1025). Two presses waiting together each read afresh; output whose speech entry joined before the second press is spoken before that press's answer, by the order rule under "Answers and speech".

### Identical reads

- Core gives the rules no read identical, field for field but its times, number, start anchor and caret report, to the last read it gave them, unless the read waited or is the first after the focus arrived; it still covers told changes, and its start anchor is let go. Verbatim-only: today's rule (Dickson, 2026-10-26), moved to Core, so rule 17's count is today's (review, S6).

### The control plane and the test harness's idle

- The control plane's `TerminalSnapshot` event is built from each full read's answer, so `wait_for_caret_on` (`verbatim-e2e/src/scenarios/terminal.rs` 513 to 553) keeps its evidence; the change tell is not on the stream.
- The test harness's idle adds: the focused terminal quiet (not baselining, waiting for its window, or reading), no speech entry waiting, no colours owed.

## State tables

### Table 1: the focused terminal (Core)

`FocusedTerminal = None | Baselining { read, told } | WaitingForWindow { told } | Modelled { node, reading }`, `told` being the newest change time not yet covered, if any, and `read` the baseline's request, if one is out.

Columns: A, a terminal focus committed, new or taken again; B, `Follow` answers "not there" or "refused"; C, `Follow` stopped at its due time; D, a change tell observed at O; E, the baseline's result, begun at S; F, the baseline refused, failed, or stopped at its due time; G, an entry's full read result (a caret key's, a review command's), begun at S; H, the focus leaves the terminal; I, its outpost ends; J, its window answers again (the recovery set); K, `Follow` or the baseline refused unsent or stopped as hung.

| State | A | B | C | D | E | F | G | H | I | J | K |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| None | baselining; `Follow` and the baseline sent | n/a | n/a | dropped | n/a | n/a | n/a | nothing | nothing | nothing | n/a |
| Baselining | baselining anew; the newest `Follow` and baseline | none | `Follow` sent once more, with the baseline if unanswered | told = O; with no baseline out, the baseline sent | modelled; if told after S, an output read sent, else quiet | baselining with no read out; told kept | with the baseline out: n/a, it answers first (worker order); with none out: taken as the baseline, as column E | none | none | the window joined the set for another request (an entry's read, or the failed baseline's): `Follow` sent again; with no baseline out, the baseline sent; with one out, nothing more | waiting for its window; the window in the recovery set |
| Waiting for its window | baselining anew | none | n/a | told = O | n/a: late answers dropped | n/a | the window answered before the half-second check found it: taken as the baseline, the silent first read, and `Follow` sent again at once, since the hung one may not have registered; modelled as Baselining's column E; the window stays in the recovery set until that check, which then removes it, appends build 1's re-ask, and for the terminal applies Modelled, column J (one more output read, which covers anything told meanwhile) | none | none | baselining; `Follow` and the baseline sent | stays |
| Modelled | baselining anew; the model kept, its reading reset | none | `Follow` sent once more | Table 2, column A | n/a | n/a | Table 2, column F | none; the model kept among the 16 | none; models dropped | `Follow` sent again; Table 2, column K | a `Follow` sent again by column C: the window in the recovery set; stays modelled, `Follow` sent again at column J |

### Table 2: a modelled terminal's reading (Core)

`Reading = Quiet | Behind { told } | Out { request, mark, again } | Holding { told } | Stalled { holding }`, where `mark = Spoken | Waited { release } | Silent` (only a waited read can be the release the backlog look waits on), `again` the newest change told while the read is out, `Behind` a live terminal with a change told while an entry's full read of it is out, and `Stalled` keeps whether the policy was holding.

Columns: A, a change tell observed at O; B, its output read's result, begun at S; C, refused, not there or failed; D, Core stops waiting at the due time; E, hung: refused unsent or stopped while out; F, an entry's full read result (a caret key's, a review command's), begun at S; G, lines waiting or handed to speech reach "Lines spoken in full"; H, a group's last line handed to speech; I, the backlog looked at; J, a cut (a key's cut, "report new output" off, a cut utterance); K, its window answers again (the recovery set); L, an owed scope or owed colours arises.

| State | A | B | C | D | E | F | G | H | I | J | K | L |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Quiet | covered by the last read: nothing; an entry's full read out: behind, told = O; else an output read sent, spoken; out | n/a | n/a | n/a | n/a | nothing | holding, nothing told | nothing | nothing | nothing | an output read sent, spoken; out | an output read sent; out |
| Behind | told = O | n/a | n/a | n/a | n/a | told after S: an output read sent, spoken; out; else quiet | holding with told | nothing | nothing | stays: the read sent after is judged by its start | told = now | kept; sent with the read after the entry's |
| Out, spoken or silent | again = O | taken; its speech entry joins; an owed scope sends the next read at once; else if the lines now hold, holding with again; else again after S sends one more read; else quiet | not there: Table 1 ends; else again sends one more read, or quiet | Cancel; one read sent again with the same mark | the sound if attended; stalled, live | again cleared if before S | nothing now: its answer finds the lines (column B) | nothing | nothing | a silent mark stays; a spoken one stays, judged by its start time | again = now | kept for the next read sent |
| Out, waited (release) | again = O | taken, waited; its speech entry joins; an owed scope sends the follow-up, still the release; else the backlog look runs: lines wait, holding with again; none, live, and again after S sends a waited read, else quiet | the backlog look runs as in column B, sending nothing for the failed read | Cancel; sent again, still the release | the sound if attended; stalled, holding; the backlog look runs | again cleared if before S | stays | n/a: one release a group | n/a: the look waits on it | becomes out, silent (today's overtaken read), the policy live | again = now | kept for its follow-up |
| Holding | told = O | n/a | n/a | n/a | n/a | told cleared if before S | stays | told: a release read sent, waited; out; else nothing | nothing waits: told sends a waited read, not the release; else quiet; lines wait: stays | told: a silent read sent at once, the policy live; else quiet | told = now | kept until the release read |
| Stalled | nothing: the read after the hang covers it | n/a: late answers dropped | n/a | n/a | n/a | taken (the window answered an entry's read); stays stalled until column K | nothing | nothing | nothing | holding becomes live | live: an output read sent, spoken; out; holding: holding with told = now | kept |

### Table 3: a read's speech entry (event queue)

Columns: A, it reaches the front with the terminal still focused and its model generation unchanged; B, it reaches the front otherwise; C, a cut while it waits; D, its reader ends; E, the foreground changes; F, an entry ahead of it (a terminal caret key, a review command) has its terminal read answered, and no focus-kind entry stands between them ("Answers and speech").

| State | A | B | C | D | E | F |
| --- | --- | --- | --- | --- | --- | --- |
| Waiting | answers to keys spoken; output to the flood policy unless silent or begun before the last cut; ends | ends silent | nothing now: judged at its turn by the read's start | ends silent at its turn | nothing: it makes no request | judged now as in column A or B, in queue order, before the entry's read's own speech; ends |

### Table 4: the model's owed scope

Columns: A, a result's rules ask a scope while no output read is out; B, they ask it while an output read is out; C, a full read sent; D, the model forgotten.

| Scope | A | B | C | D |
| --- | --- | --- | --- | --- |
| Fresh | that read sent at once | kept; sent after the out read's answer | the read is fresh | gone |
| Whole document | n/a: sent at once | replaced by the newer | the read takes the whole document; fresh again | gone |
| History block | n/a | replaced by the newer | the read takes the history block; fresh again | gone |

### Table 5: the start a history read or search counts from (Core's memory)

Columns: A, the request sent naming the memory's start anchor; B, a full read taken while it is out; C, the anchor's row leaves the memory; D, its answer arrives; E, the command ends otherwise.

| State | A | B | C | D | E |
| --- | --- | --- | --- | --- | --- |
| None | an anchor at that start's row; the outpost's start anchor held | n/a | n/a | n/a | n/a |
| Held | n/a: one request at a time per command | moved with its row's outcome, as the review position | gone: the answer ends the command silently, marks kept | rows placed from the anchor's row; let go | let go |

### Table 6: a review command in the focused terminal (event queue entry)

Columns: A, it reaches the front; B, its request answers with a result; C, another outcome, or Core stops waiting; D, the answer from memory needs rows above the memory; E, a mark's move needs a search above the memory; F, an output read is taken while it waits; G, a key or command arrives; H, the foreground changes.

| State | A | B | C | D | E | F | G | H |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Waiting | not a modelled focused terminal: the review command's ordinary path; else the fresh full read sent | n/a | n/a | n/a | n/a | nothing | the arrival joins behind | ends if the terminal's window is outside the new foreground window's tree or owner chain |
| Fresh read out | n/a | taken by the model; the terminal's waiting speech entries ahead of the first focus-kind entry spoken (Table 3, column F), then its own; answered from memory: spoken and ends, or column D or E | answered from the memory as it stands; a hung refusal plays the sound if attended | the history read sent, a screen more than needed (Table 5) | a search sent (Table 5) | taken in arrival order before or after this read, as the worker made them | joins behind; the command is left to finish (ADR) | the window test; failing it, Core stops waiting and the command ends silent |
| History read out | n/a | rows placed from the anchor; answered; short: ends silent (today) | ends silent | n/a | n/a | the anchor moves (Table 5) | joins behind | the window test; failing it, ends silent |
| Search out (at most three a press) | n/a | placed from the anchor; the move ends on a mark, or tries the next | ends silent, every mark kept | n/a | another search under three; at three, ends silent, marks not tried kept | the anchor moves | joins behind | the window test; failing it, ends silent |

### Table 7: the start Core names (Core's model)

Columns: A, a full read's result whose screen shows text, or the first; B, a result whose screen shows no text after one that did; C, a history read or search sent; D, the model forgotten.

| State | A | B | C | D |
| --- | --- | --- | --- | --- |
| No start | its start anchor named | its start anchor named (a first read) | n/a: no model | n/a |
| A start named | its start anchor named; the old one let go at the next `NodesHeld` unless a request out names it | stays; the new read's anchor let go | the named start held until the answer | let go |

## Decisions made outside Core, with their facts

- The outpost, whether a change signal belongs to the followed terminal: by `Follow`'s naming (Core's fact) and the signal's own window or element. A signal before `Follow` was applied is dropped; the baseline, sent after, covers it.
- The outpost's limiter, merging waiting tells of the same terminal and kind: no fact of Core's.
- The outpost, the remote program or the classic path for a read: today's guesses that only cost calls (`tries_remote`, `read_classically`).
- The listener: its transient console hook at start, which names the console host's thread and tells nothing (build 1).

No fact is held in two places in this build: the told fact, the policy, the start and the reading are Core's alone; the outpost holds only start anchors Core names.

## Waiting states, messages, timers and two-place lifecycles

Added: the focused terminal's life (Table 1) and its reading (Table 2), each one type; the speech entry (Table 3); the start anchor Core names and its memory anchor for a history read or search (Tables 5 and 7); one tell kind.

Removed: the hold, a lifecycle held in two places (the outpost's `Reading`, Core's `held`); the outpost's copies of Core's facts (height, width, colours window, caret last sent, last read sent) and its coverage, identical-read and rule 10 choices; Core's `TerminalAsk` (read, cancel, overtaken) and `ModelAsk`; the model's queue of review presses; build 1's terminal queue and unasked reads; build 1's terminal late-answer path and hung-window send exception. Timers: none added, none removed.

Net: one two-place lifecycle removed, none added; colours owed derived, not stored; nothing added to save a call or a wake-up.

## Messages added and removed

Added: the `Event` tell's kind "terminal changed" (outpost to Core; the node, its window, when observed), handled at once in the inbox; the shared list's "read the terminal" request (node, start anchor, scope, height, width, colours, the console fact, and a starting read's unit), answered with today's `TerminalSnapshot`, its caret starting point when asked, and its start anchor's id.

Changed: `TerminalHistory` names its start anchor and the screen's height; `TerminalFind` names its start anchor; `Follow`'s answer no longer carries a baseline read.

Removed: `TextOp::TerminalHold`, `TerminalRead { hold }`, `TerminalCancel`, `TerminalColours`, `TerminalShape`, `TerminalReadNow`; `TextReply::TerminalUnchanged`; `TextReply::Done` if nothing else answers with it (`verbatim-model/src/text.rs` 846 to 848); `NormalizedEvent::TerminalSnapshot` as an outpost's unasked message (it stays as the control plane's version 4 event). The ADR's 26 kinds of message are unchanged.

## Exact expected speech for each changed sequence

The terminal model is Verbatim-only and this build changes none of its expected speech. "Decided" means the terminal model's speech, decided by Dickson where it says so; the NVDA captures the terminal scenarios cite (2026-10-09, alpha builds) are not kept.

1. A change while live (`new_output_is_spoken_line_by_line`): "hello". Decided.
2. A flood with the shipped limits (`conhost_flood`, `windows_terminal_flood`): "flood line 1" to "flood line 30", the skipped-lines sound and "skipped 1941 lines", "flood line 1972" to "flood line 2000", "ready>". Decided.
3. A cut while holding with a change told (`a_cut_while_held_reads_to_the_end_without_speaking`): heard "line 1", then after the cut "line 51". Decided.
4. A release read out at the cut (`a_read_a_cut_overtakes_speaks_only_its_answers_to_keys`): heard "> banana" only. Decided.
5. Control during a flood (`*_control_flood`): what was queued is cut; once the script goes on, "flood line 101", "flood line 102", "flood line 103", "ready>". Decided.
6. A cut while live (review, M8; Windows Terminal): Down Arrow at an empty history watches; at 50 ms "Build failed: 3 errors" is printed and read at once between the watch's checks; at 120 ms Control+C. The read began before the cut: not spoken, as today and as NVDA. Decided.
7. The command-mark corner (Dickson, 2026-11-16, an accepted limit): Verbatim+Alt+Up searches for about 300 ms; "Build failed: 3 errors" printed at 100 ms; Control+C at 200 ms; "Terminate batch job (Y/N)?" at 250 ms. Heard: the mark's line, "Build failed: 3 errors", "Terminate batch job (Y/N)?". NVDA, which has no command marks, would not say "Build failed: 3 errors" after the cut.
8. A lost read in a group (`conhost_scrollback_overflow_during_group`): "before line 1" to "before line 15", "skipped more than 8986 lines", "after line 11987" to "after line 12000", "ready>"; Windows Terminal "skipped more than 9016 lines". Decided.
9. Owed colours (`a_read_taken_at_once_before_the_program_draws_the_key_leaves_the_echo_to_the_next`): "c", then "ho hi". Decided.
10. An identical read before an echo (`a_read_not_sent_as_the_last_one_again_leaves_the_colours_to_the_echos_read`): "b". Decided.
11. Typing in the console host (`conhost_typing`, step 2): `echo helo` echoed per character, Left Arrow "o", "l", Enter "hello" and "ready>". NVDA capture cited, not kept.
12. A return to the same terminal (review, M4): "terminal", "ready>"; then `dir` and Enter: the listing and "ready>". NVDA capture cited by `terminal_windows`, not kept.
13. A mark move while a clock ticks each second (the start anchor): the mark's line is spoken, as today. Decided.
14. A terminal whose window hangs while output is written (the uniform rule): the not-responding sound while the terminal holds attention, when a change is told during the hang and its read is refused (with no change told, nothing sounds); up to about half a second after the window answers, the output written meanwhile, as one read. Verbatim-only, a deliberate difference decided by Dickson (2026-11-16): no NVDA capture is needed, since NVDA itself hangs badly there; its test asserts this speech.

## Tests and scenarios

Hand-written tests, one per cell of Tables 1 to 7, and the interleavings: a change tell while a caret key's starting read is out; a change after an output read began (one more read); a read stopped at its due time (one retry); refused and failed reads (no retry without a new change); a tell before the baseline's answer; a failed baseline (sent at the next change); a cut while a release read is out; a cut while live with a read in flight; a withdrawn final read (dropped; the next output read speaks its change); a hang and its recovery (asserting sequence 14's speech: the sound while attended, then the output written meanwhile as one read once the window answers again); a mark move with output read during its search (placed from the anchor), and with more than a screen of output (silent); the speech fence for a read sent before a key and answered after it. With no fact in two places, no generated test (Dickson, 2026-11-10).

Converted, same expected speech: `verbatim-core/src/terminal/tests.rs` (their `TerminalHold`, `TerminalRead`, `TerminalCancel` assertions become output reads and marks); `verbatim-core/tests/terminal_reading.rs` (`live_snapshots_that_arrive_while_a_whole_read_is_owed_are_dropped` becomes "an owed scope is the next read sent"; the colours tests take Core's decision; the four identical-read tests move here from the outpost; `a_history_answer_counted_from_a_read_core_did_not_take_ends_the_command` becomes placement from the anchor; `output_arriving_while_a_search_is_out_moves_to_the_mark_with_one_search` keeps its words; the review tests drive entries); `verbatim-core/tests/terminal_floods.rs` (the support's simulated outpost tells a change per write and answers reads from named starts); `verbatim-outpost/src/terminal/tests.rs` (the conversion tests stay; `reading.rs`'s test goes); mockapp's `terminal.rs`, `terminal_reading.rs`, `terminal_snapshot.rs` through build 1's driver.

New: `verbatim-model` round trips of the request and tell; `verbatim-outpost` change tells and the limiter's merge, reads from a named start, start anchors released by `NodesHeld`.

End-to-end, none changing its expectations: every `windows_terminal_` and `conhost_` scenario and `terminal_settings_page`; `terminal_overflow`'s description of the outpost's hold is rewritten. Related scenarios for the parent to run three times: `conhost_flood`, `windows_terminal_control_flood` (the cut, the command-mark chain), `conhost_scrollback_overflow_during_group` (the owed read), `windows_terminal_typing` and `conhost_line_key_flood` (the typing path). `conhost_typing` is left out, as `docs/flaky-tests.md` records it.

### Every terminal row of `test-coverage.md`

None goes silent and none changes its speech: New output and floods; Typing in terminals; Review cursor in terminals; Alternate screen, pagers and full screen; Lists and progress lines; Windows and tabs (the baseline after `Follow`, on every re-take); Terminal settings and Report new output (turning output off is a cut); Console windows; Report status bar; Previous and next command (anchored placement); Cancel and pause speech, editing chords, clearing keys, Backspace and Delete in a terminal; the review rows naming terminal scenarios. Hung application gains the terminal case (sequence 14).

## The code's organisation

- `crates/verbatim-core/src/terminal/`: today's `terminal.rs` becomes `speech.rs`; `terminal_model.rs` becomes `model.rs`; `terminal_model/review.rs` becomes `review.rs`, with the review entry as an async function; a new `reading.rs` holds `FocusedTerminal`, `Reading`, the output read with its coverage, marks and owed scope, the speech entry, colours and the identical-read rule.
- `crates/verbatim-outpost/src/terminal.rs`: reads, history reads and searches from a named start anchor with the request's parameters; `terminal/reading.rs` deleted; the worker's terminal paths collapse into one read handler; change signals become tells.
- `crates/verbatim-model`: the request, the tell kind, the removed variants.

## The order of work

1. The model's types. 2. The outpost: change tells, reads from named starts, start anchors, the removed paths; mockapp through the driver. 3. Core: `FocusedTerminal`, `Reading`, output reads, speech entries, cuts, colours, owed scope, identical reads, the hung path through the recovery set; Tables 1 to 4 and 7. 4. Core: the review command as one entry with its anchor (Tables 5 and 6). 5. `terminal_support` and the converted tests. 6. Documents, `test-coverage.md`, crate guides; `cargo xtask ci`. The parent runs the related scenarios and the measurements.

## Limits

- The command-mark corner (sequence 7): output written just before Control+C during a mark search can be spoken (Dickson, 2026-11-16); splitting the search into shorter remote operations is a later option.
- More than about a screen of output during a history read or search ends the command silently, marks kept, as history reads do today.
- Every change signal is a tell: up to about 13,600 a console host flood raises, less what the outpost's limiter merges; each is a step of Core's reducer.
- An output read waits for the worker's current request: up to a search's length (about 300 ms at its bound) or a whole read (25 to 31 ms in the console host).
- A failed read leaves its change unread until the next change, as today.
- After a hang, the terminal is read up to about half a second after its window answers (the recovery set's live check).
- Two presses waiting together each read afresh.
- Windows Terminal raises no text-changed event for writes made only of blank lines, as today.

## Performance figures and measurements

Known (`performance.md`): an ordinary read 0.45 to 1.06 ms in the console host, 1.6 to 3.3 KB, the reducer 101 to 116 µs a read; a whole read 25 to 31 ms in the console host, 11 to 12 in Windows Terminal; typing echo median 4.1 and 99th percentile 10.0 ms in the console host, 21.4 and 58.2 in Windows Terminal; 1,911 and 252 reads per flood; about 13,600 console events per flood; 439 of 911 console host keys read again for colours; Core kept up with 3,000 to 4,000 tells a second on 2026-11-14, waking once per two or three.

Expected: per change, one tell and, when no read is out, one request more than today; typing echo about 0.2 to 0.5 ms later (estimated from the crossing figures); per key one message fewer (`TerminalColours`); fewer colour re-reads.

Measured in this build, at most 20 runs each, release, against the build before it: in `conhost_flood` and `windows_terminal_flood`, change tells sent and merged, output reads, the reducer's time per tell and per read, Core's, the relay's and the outpost's wake-ups and processor time, the flood's stopwatch, and the delay from a group's last line to "skipped"; `measure_keys_conhost` and `measure_keys_windows_terminal`, last key to speech queued, minimum, median, 95th and 99th percentiles and maximum, tells and reads per key, colour reads; a once-a-second clock for 30 seconds in each terminal, tells, reads and Core's wake-ups (today 117 reads and 94 wake-ups in the console host, 58 and 101 in Windows Terminal); idle with a focused terminal, no added wake-up. Budget: nothing beyond the tells and requests the rulings accepted; Core within 2 ms a step; the echo's 99th percentile within the spread plus the added crossings. A miss stops the build and goes to Dickson with the figures.

## Corrections to the build notes

- "The outpost keeps one flag ...": replaced by Dickson's ruling (every change told).
- "Core asks a full read, as an event-queue entry ...": replaced by option C.
- "The request names ... whether to drop the output": Core's speech mark (Table 2).
- "The next-read start with its read number stays" in the outpost: replaced by start anchors Core names (M6.4's item).
- "Core aligns an identical read": Core keeps today's skip.
- "`review.rs` 642 to 650, 739 to 757" is `crates/verbatim-core/src/terminal_model/review.rs`.

## Review findings and what became of them

M1, M2: void (no flag; told is Core's alone). M3: void (late and withdrawn answers dropped; named starts do not drift; the next output read speaks the change). M4: a re-take reads a baseline. M5: a failed baseline is sent at the next change. M6: the baseline carries Core's parameters. M7: void (owed scope is the next output read). Reworked-builds review: M1 resolved by building and pushing builds 1 to 3 together; M2, the starting read mints the caret starting point and the caret anchor leaves the request; M3, a terminal waiting for its window is re-sent `Follow` and its baseline at recovery; M4, the baseline sent with `Follow`, ahead of any entry's read; M5, waiting speech of the terminal spoken before an entry's; S5, Table 2 column K in every row; S6, one `Follow` resend; S7, a stale start read with `from_start` false; S8, covered tells and tells behind an entry's read send nothing; S10, the mark type and `Stalled { holding }`; nits 5, 10 and 12 fixed here; nit 4 and S11 applied in build 1. M8: option C, reads outside the queue judged by start time; its residue is the accepted command-mark corner. M9: void (uniform late-answer rule). S1: void (no read entries). S2: void (a silent read is never a release). S3: the backlog look waits for the release read and its line. S4: void. S5: void (late answers dropped). S6: today's identical skip kept. S7: the fence stated with its test. S8: interleavings enumerated. S9: void (output reads carry no settle; build 1 has no settle). S10: related scenarios widened; the hang's unit test. S11: states listed; colours derived. N1: the console fact is Core's. N2: stated. N3: void (tables rewritten). N4: stated. N5: the citation is `behaviors.py` 485, after the stabilise delay.

## Documents amended with the build

- ADR 0003 by Dickson's rulings: "A reader keeps nothing for Core between requests but held objects and the requests waiting in an outpost's table"; "What this decision adds" without the two-place told and changed; the requests outside the event queue gain a terminal's reads for its output; the late answer as the uniform rule (with build 1).
- The terminal model: "Where it runs", rule 1, rule 10 (the start Core names), rule 23 and "Lists and colours", "Keys", "The review cursor" (placement from an anchor), "Core's flood policy" (the hold Core's).
- `performance.md`, crate guides, `test-coverage.md`'s terminal and hung application rows, `docs/parity/terminals.md`, `docs/roadmap.md` (M6.4's first item done here).

## Appendix: tests removed in build 3

Each pinned a mechanism this build removes; the behaviour it covered, where any remains, is pinned by the test named.

- `verbatim-core`'s `queries/tests.rs` (removed with the query table in build 1), `a_console_review_answered_from_memory_is_never_waited_for` and `a_dropped_models_review_ends_and_its_fresh_read_is_taken_by_no_model`: Core's queries answered from a terminal's memory, and the model's request kinds, go; a review command in a terminal is one entry (`crates/verbatim-core/tests/terminal_reading.rs`, the review tests).
- `verbatim-core`'s `queries/tests.rs` (removed with the query table in build 1), the console start of `every_sequence_keeps_the_query_tables_invariants` and its step of 16 other terminals' snapshots: the terminal's unasked snapshots and the queries a model made go.
- `verbatim-core`'s `queries/generated.rs` (removed with the query table in build 1), the terminal's snapshot, other terminals' and timer actions, the model's terminal models and its checks of answers from memory, its listed states of models made, evicted and dropped, of an answer to a model no longer live and of a request answered from memory, and the fixed cases of an answer to a replaced model and of the terminal's timer: the same mechanisms; the timer is pinned by `the_timer_gives_the_keys_that_reached_the_program_by_then_first`.
- `crates/verbatim-core/tests/terminal_reading.rs`, `the_read_before_a_held_key_is_aligned_and_its_caret_answers_it`: the read before a held key goes; its successor, `a_held_keys_starting_read_is_aligned_and_its_output_spoken`, pins a terminal key's starting read.
- The outpost terminal module's `reading.rs`, its test `every_state_and_event`: the outpost's on-demand reading goes; the flood policy is Core's alone (`crates/verbatim-core/src/terminal/reading_tests.rs`).
- `crates/verbatim-outpost-logic/src/tests.rs`, `a_cleared_absorption_reaches_the_mailbox_before_the_read_that_cleared_it`, `a_noted_caret_is_read_with_its_events_timing` and `a_console_note_is_polled_when_nothing_else_is_ready_and_stops_at_its_end`; `intake.rs`, `a_focused_terminals_caret_event_is_planned_after_its_text_change` and `a_held_consoles_updates_are_dropped_at_push_until_the_mark_is_cleared`; `text.rs`, `a_console_note_polls_from_its_before_reads_end`: the held console's absorbed updates, the noted caret, the kept terminal queue and the console poll go; every change signal is a tell, merged by the intake's limiter (`a_windows_console_updates_merge_into_one_while_they_wait`), and a terminal's caret key is Core's watch.
- `crates/verbatim-core/tests/text.rs`, `a_key_passed_on_in_a_terminal_asks_one_read_and_only_there`: the one-way read at a key's pass goes into the terminal caret key's final read (`crates/verbatim-core/src/terminal/reading_tests.rs`).
- `crates/verbatim-process/src/message.rs`, the `DeliverFact` round trips of `a_delivered_fact_round_trips` (now `a_listener_fact_is_delivered_without_its_pid`) and `forwarding_ended_round_trips`: Core no longer routes a listener fact to an outpost.
- `crates/mockapp/tests/terminal_reading.rs`, `a_read_with_nothing_new_answers_empty_and_may_hold_again`, `a_cancel_answers_what_is_new_and_reading_is_live_again` and `a_held_terminals_caret_event_is_read_with_the_text`: the outpost's hold, cancel and noted caret go; Core's flood policy and its silent read after a cut are pinned by `crates/verbatim-core/src/terminal/reading_tests.rs`.
- `crates/mockapp/tests/terminal_reading.rs`, `a_read_covering_a_caret_event_answers_a_watch_with_no_caret`, `a_read_terminal_message_reads_the_terminal`, `a_read_terminal_message_reads_the_caret_first_under_the_keys_trace` and `a_console_caret_event_without_a_move_leaves_the_poll_reading`: a terminal key's note, its one-way read and the console poll go into Core's caret key entry with its watch and final read.
- `crates/mockapp/tests/terminal_reading.rs`, `the_hooks_change_events_wait_for_a_window_read_through_msaa`: the outpost hooks only a console host's console events (build 1).
- `crates/mockapp/tests/terminal_reading.rs`, `an_identical_read_is_not_sent_again`: the identical-read rule is Core's (`crates/verbatim-core/tests/terminal_reading.rs`).
