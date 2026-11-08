# The caret and selection rework

Status: scope and decisions agreed by Dickson on 2026-10-25, 2026-10-29, 2026-11-08 and 2026-11-09; approved by Dickson on 2026-11-09. It includes the held-key line, one of the state models in `docs/roadmap.md`.

## In short

A caret key is still held while the focus's outpost reads the caret, as NVDA's caret scripts read the caret before sending the key. The outpost then watches for evidence of what the key did, and Core chooses every word.

What changes is the state around that. Each text node has one last read, in the outpost. Each key has one lifecycle value, in Core, which also owns the key's 100 ms. The outpost's only part in a key's life is the watch: one value that holds the baseline and looks for evidence in the application's events until it finds some or Core tells it to stop. A key gets exactly one result after its before read: evidence, the read at 100 ms, or the end of its watch. Everything else that kept the caret, selection or key progress is removed.

This fixes two known defects: Control+A followed by a repeated focus says nothing, and a superseded key's answer leaves Core's caret stale. Keys that move nothing now speak as NVDA's do, Alt+Up and Alt+Down move by sentence, and five editing chords become silent.

## Terms

- Core is Verbatim's main process, and its reducer decides what is spoken. An outpost is the helper process that makes one application's UIA or MSAA calls. A remote program is a small UIA program run inside the application, so that many reads cost one round trip.
- A held key is a caret-table key the keyboard hook swallows while the focus has text, until Core passes it on.
- The before read is the read of the caret made for a held key before it is passed on.
- The last read is the outpost's one record per text node: caret, selection with its text up to 513 UTF-16 units, the caret's line and offset in it, the note that the text changed, and the time the read began.
- A baseline is a copy of the last read pinned for one key.
- The watch is the outpost's open check for one key's evidence. Evidence is the caret having left where it was, the characters beside it changing, the unit at the caret changing after Delete, the caret's line changing after a key that deletes, or the selection changing.
- The key's bound is 100 ms after Core passed the key on, or after the key reached Core for a key that was not held. It is Core's one deadline for the key.
- The final read is the read the outpost makes when Core tells it the key's bound has passed. The bound answer is that read's result when it finds no evidence.
- A tagged report is a key's answer to Core after its before read, `NormalizedEvent::KeyAnswered`, carrying the key's tag.
- A key tag (`KeyTag`) is the `QueryId` Core gave the key's request, carried in the request. Core knows it when it asks, each request has its own, and none is reused, so an answer belongs to one request and no other.
- General terms are NVDA's words when the text changed since the selection was last compared: the whole new selection "selected", "selected instead", or "selection removed".

## Decisions

Dickson's, each with its date:

- One baseline (2026-10-25). One last read per text node, replaced by every read, is the only baseline.
- The newest caret by one stamp (2026-10-25). Core takes every report as the newest caret by the time its read began, in microseconds.
- A node's last read lives while Core holds the node, and Core's choice between a new and a repeated focus decides what is spoken: a repeated focus's changes are spoken, a new focus's are not (2026-10-25, amended 2026-10-29).
- A key that moves nothing speaks as NVDA's script does (2026-10-29). A movement key and Delete speak their unit. Backspace and Control+Backspace say nothing (`nvda/source/editableText.py` 313 to 315). A Shift key that changes nothing says nothing, since auto-detection reports only a change at the caret event (`editableText.py` 451).
- The 100 ms bound is Core's (2026-11-09), NVDA's `caretMoveTimeoutMs` (`nvda/source/config/configSpec.py` 364), counted from when the key is passed on, as NVDA counts from after sending the key (`editableText.py` 188 to 189). Only Core knows that moment. Core arms it as one of the reducer thread's deadlines, evidence cancels it, and when it passes Core sends the outpost the final read. The outpost keeps the judgement of evidence, the terminal's handling deadline and the console host's poll, because those run on the outpost's own reads.
- The bound answer ends the watch (2026-11-08). There is no edge-of-text rule and no later evidence, because NVDA has neither (`editableText.py` 86 to 87, 150 to 163). A move later than 100 ms is not spoken.
- A focus that moves within the 100 ms matches NVDA's source (2026-11-08), as "A focus that moves first" describes.
- Backspace and Control+Backspace with a focus that moves first match NVDA (2026-11-09), which reads the deleted text before the key and speaks it because its wait counts a pending focus as a move (`editableText.py` 97 to 99, 300 to 320). Example: Backspace over "x" in a chat box with the focus moving 40 ms later is heard as "x", "Edit message edit".
- A held key whose before read passes 150 ms is passed on, and a watch opens judged from the last read, as for a key that was not held (2026-11-08).
- A key's results after its before read are tagged `KeyAnswered` events, and the request answers once (2026-11-08, its shape 2026-11-09), so that they travel in one order with every caret report and no several-answer responder exists. This replaces the stream responder in `docs/design/ownership-milestone-rules.md`.
- A key whose before read is running when the focus moves to another node says nothing, a difference from NVDA, whose script speaks the old field's unit before the focus (`editableText.py` 97 to 99, 165 to 192), because in Verbatim the focus has already been spoken and the unit would follow it (2026-11-09).
- A sentence move whose final read or later step passes its 150 ms is dropped: nothing is said and the caret stays, a difference from NVDA, which blocks until it can read and then moves (`editableText.py` 245 to 262), so that keys behind it are not held longer (2026-11-09).
- A command pressed within 100 ms of a caret key that was not held waits for it, at most those 100 ms, the one added delay in this design, ending with uiAccess (M8.6) (2026-11-09).
- Alt+Up and Alt+Down match NVDA (2026-10-29): sentences by UAX #29 within each paragraph; in UIA text, which has no sentence unit, nothing is said (`editableText.py` 263; `nvda/source/UIAHandler/__init__.py` 158 to 176). A rich edit control is read as a plain edit control until M6.5.
- Control+Z, Control+Y, Control+X, Control+V and Escape in a text field open no watch and say nothing for the key, as NVDA binds no script to them (2026-10-25; absent from `editableText.py` 387 to 413). Their selection changes are spoken from the application's caret event. Other keys outside the caret table speak by their evidence (2026-10-08).
- Terminals keep their reading rules and today's handling of the five chords (2026-10-25 and 2026-10-29).
- The judgement of evidence stays in the outpost, named in D14 (2026-10-25), because it bounds reads on text changes.
- Until uiAccess (M8.6), a caret key in an elevated window is judged from the last read (2026-10-25).

Claude's, each with its reason:

- No value-change sync on the before read: NVDA checks a waiting value change only at the caret event (`nvda/source/NVDAObjects/window/edit.py` 1068 to 1081), and the sync would add a wake before each key.
- Core alone says whether a key's node is a terminal, from its role, and the key request names that kind, because the outpost's own test (the window class, or a UIA console focus) disagrees with the role on an MSAA console. A terminal never compares selections, and Core never sends its watch a final read.
- A bound read that fails or is abandoned says nothing, as `_caretScriptPostMovedHelper` returns when it cannot read (`editableText.py` 168 to 172).
- A focus read that fails drops the node's last read, as `initAutoSelectDetection` starts again (`editableText.py` 421 to 424).
- A watch ended by the node's release or its element's replacement says nothing, because the object it was for has gone.
- A deadline due during a burst of queued work is acted on when the reducer drains it, as every reducer deadline is today.
- A key outside the caret table that the hook reports, which today is a clearing key reported as `CaretMotion::Other`, has the same 100 ms bound and says nothing at it, so that one lifecycle covers every key. An effect later than that is not spoken.
- Each step of a sentence move reuses the held key's 150 ms deadline, because no second timer kind is needed.
- The key's bound and the wait of the key behind are one deadline, 100 ms after the pass, because they are the same interval from the same moment, and two would be a copied fact.
- A key that was not held holds its line as a passed key does, so that one lifecycle covers every key. Keys the hook passes run at once, as today.
- The final read has no reply of its own, and the key's one result is the reply. A final read or a watch's end arriving with no open watch for its tag is ignored, because that watch's result is already on its way.
- A check already running when the final read arrives is the final read, as NVDA's last poll is a read begun before its timeout (`editableText.py` 123 to 151).
- `Input::SpeechCancelled` carries microseconds, so that it compares with a read's stamp directly.
- A tagged report is never merged and bars merging across it: its `merge_key` is `None`, as a focus report answering a live re-read has, and its barrier is `Barrier::Reply`. It is the key's only result, and an older report with selection changes merged across it would reach Core after it.
- A bound read that finds evidence answers as evidence, as NVDA's loop tests each read, its last included (`editableText.py` 123 to 151).
- The first test of a key's later keys is by stamp only: time zero meaning unknown goes.

## Where each part runs

- The hook (`verbatim-input-windows`) holds and defers keys as today, and reports a key of the editing chords' list with its gesture.
- `verbatim-input` keeps the caret table and gains the chords' closed list.
- The shell (`verbatim-app`) turns a reported chord into its own input to Core.
- The model (`verbatim-model`) replaces `TextOp::ReadCaret` and `TextOp::AwaitCaret` with one key request, adds `CoreToOutpost::Watch` for the final read and the watch's end, and adds `NormalizedEvent::KeyAnswered`, under "The messages".
- The outpost's reducer (`verbatim-outpost-logic`) holds the last reads, the pinned baseline and the watch in `TextFacts`, so one place names every range the store must keep, and owns the terminal's handling deadline and the console host's poll as reducer times.
- The outpost's programs (`verbatim-outpost`) make every read and comparison, committing through `TextChange` as today.
- Core (`verbatim-core`) holds the key line, the key's bound and one caret on the focus object, and decides every word and the sentence move.
- Sentences are found as say-all finds them: the outpost answers with the paragraph or as unsupported, and `verbatim-text` splits it.

## The state

### Core

```rust
enum KeyLine {
    Idle,
    Busy {
        current: CurrentKey,
        waiting: VecDeque<WaitingKey>, // behind the current key, in press order
        heard: Vec<UtteranceSegment>,  // selection changes held for the next words
    },
}

enum CurrentKey {
    Reading { query: QueryId, node: NodeId, key: HeldKey },
    Watched(Watched),
    MovingBySentence { node: NodeId, key: KeyInfo, later_key_at_us: Option<u64>, step: SentenceStep },
}

struct Watched {
    tag: KeyTag,
    node: NodeId,
    key: KeyInfo,
    kind: WatchKind,     // TextField or Terminal, named in the key request
    passed_at_us: u64,
    before: Before,
    later_key_at_us: Option<u64>,
    line: Line,
}

enum Before { Read { unit: TextChunk, line: TextChunk, removed: Option<String> }, NotRead }

enum Line {
    Holding,    // the bound has not passed; keys behind wait
    Released,   // the bound has passed; keys go ahead
}

enum SentenceStep {
    FinalRead { tag: KeyTag },  // the final read was asked; keys behind wait for its result
    ReadingParagraph { query: QueryId, paragraph: ParagraphAt },
    Moving { query: QueryId, sentence: String },
}
```

An idle line with keys waiting cannot be written. Four rules cover every ending, so the table does not repeat them:

- To end the current key: clear its deadline, say what is left in `heard`, and advance. With no key left the line is `Idle`.
- To advance: take `waiting` in order, running typed keys and commands as the inputs they came as, until a held key is reached, which becomes current. A key still `Released` stays current until a held key replaces it; `heard` stays with the line.
- At the pass, when a key becomes `Watched` with `Holding`: typed keys at the front of `waiting` run at once, stopping at the first command or held key, as NVDA's typed keys are not held behind a script, and a non-empty `waiting` marks `later_key_at_us` with the pass.
- Saying selection changes always says `heard` first, and a key's words come before both.

`heard` belongs to the line, so it outlives a key that is replaced.

`Line::Holding` means the 100 ms has not passed, and a held key or command behind waits for this key's result until it does. `Line::Released` means they go ahead. A sentence key whose 100 ms passes leaves `Watched` for `MovingBySentence` in step `FinalRead`, because the keys behind must see the sentence move, so they wait on for the final read's result, bounded by `HELD_READ_BOUND_MS`; no other key can be in that step. The state decides the one deadline: it is armed in `Reading` (150 ms), `Watched` with `Holding` (100 ms) and `MovingBySentence` (150 ms), and in no other state. Entering a state sets it, so it needs no identifier beyond the current key.

The caret moves onto the focus object as `{line, selection, line_break, read_at_us}`, so it ends with the focus. Its stamp is mandatory. `selection` is optional and holds the selection's start's own line when that differs from the caret's, so a selection line with no selection cannot be written, on the wire (`CaretReport`) as in Core.

The focus queue stays in front: a key that reaches Core while focus reports wait goes into `state.focus` and enters `KeyLine` in its turn.

`held_nodes` names `current`'s node directly.

### The outpost

```rust
struct LastRead<P> {
    caret: P,
    selection: Option<(P, P, String)>,
    line: TextChunk,
    offset: u32,
    text_changed: bool,
    read_at_us: u64,
}

struct NodeRecord<P> {
    anchors: NodeAnchors<P>,     // as today, less their reports
    last: Option<LastRead<P>>,   // none until a read commits
}

enum NodeText {
    Uia(NodeRecord<RangeKey>),
    EditControl(NodeRecord<u32>),  // a classic Win32 edit control, read by its window messages
}

struct TextFacts {
    nodes: BTreeMap<u64, NodeText>,  // one record per node, of one provider
    watch: Watch,
}

enum Watch {
    None,
    Open { key: KeyWatch, stage: Stage },
    Checking { key: KeyWatch, after: AfterCheck },
}

enum Stage {
    Awaiting,                           // a text field: until evidence or Core's word
    Terminal { ends: u64 },             // a terminal: the handling deadline
    Poll { next: u64, until: u64 },     // the console host's caret poll
}

enum AfterCheck { Resume(Stage), Answer }

struct KeyWatch {
    tag: KeyTag,
    node: NodeId,
    baseline: Baseline,
    unit: TextUnit,
    motion: CaretMotion,  // which evidence counts, in place of two flags
}
```

`AfterCheck::Answer` is the final read: evidence if it finds any, and otherwise the bound answer. `Baseline` holds a `LastRead` for UIA or an edit control. A key with no last read has no watch, so a watch without a baseline cannot exist.

A text field's watch has no time of its own: Core ends it, by the final read, the watch's end or the next key request. A terminal's and a poll's times are reducer times, and no thread or timer is added in the outpost.

### The messages

```rust
struct KeyRequest {
    tag: KeyTag,
    kind: WatchKind,
    unit: TextUnit,
    motion: CaretMotion,
    start: KeyStart,
}

enum KeyStart {
    ReadFirst,
    FromLastRead { withdrawn: Option<KeyTag> },  // no before read, or one given up
}

enum KeyStarted { Read(CaretReply), Watching }   // the request's one answer, or `Unanswered`

enum WatchOrder { FinalRead, End }               // CoreToOutpost::Watch { node, tag, order }, no reply

// NormalizedEvent::KeyAnswered { node_id, tag, answer }
enum KeyAnswer {
    Evidence(KeyRead),
    BoundAnswer(KeyRead),
    Ended,
}

struct KeyRead { caret: CaretReport, selection_changes: Vec<SelectionChange>, before_line: TextChunk, before_unit: TextChunk, unit: TextChunk, on_baseline_line: bool }
```

A key's answer is its own event rather than a field of `CaretMoved`, so a watch's end needs no caret, and baseline facts exist only on a `KeyRead`. Core receives the outcome as a type and judges no flags. An answer whose tag is not the current key's is an ordinary report if it has a read, and otherwise ignored.

`CoreToOutpost::Watch` is a new message, because `Effect::Text` asks a query whose refusal is its reply. It goes in the writer's class of queries (`Outgoing::Other`, `crates/verbatim-supervisor/src/writer.rs`), which is never merged and never dropped as oldest, and is refused when 64 commands wait or the generation has ended. The shell turns a refused final read into `KeyAnswer::Ended` for its tag, so the key still has its one answer. A refused end changes nothing Core depends on.

### The states and the rules that read them

Core. "Ends", "advance", "the pass" and "says changes" are the four rules above. A key's node is `node`; "another node" is any other.

| State | Rule | What it does |
|---|---|---|
| Idle | held key arrives | Becomes `Reading`, arms 150 ms, sends the key request with `ReadFirst`. |
| Idle | a caret key the hook passed, table or `CaretMotion::Other` | Becomes `Watched`, `NotRead`, `Holding`, arms 100 ms; request with `FromLastRead`. |
| Idle | typed character, clearing key's forgetting of typing, or command | Runs at once. |
| Idle | editing chord in a text field | Cuts speech and does nothing else. |
| Idle | caret report | Takes the caret if newest by stamp and the node is the focus; says changes. |
| Idle | focus executes | As today. |
| Busy | caret report, or an answer not this key's with a read | Takes the caret if newest. In `Watched` its changes go to `heard`; otherwise says changes, since no key is in the application's hands. |
| Busy | focus on the key's own node executes | A repeat's changes go to `heard`, a new focus's are ignored. Nothing else changes for the key. |
| Busy | exit | Passes every held key on. |
| Reading | any key or command arrives | Joins `waiting`. |
| Reading | before read answered | Takes the caret, says changes, passes the key on; `Watched`, `Read`, `Holding`, arms 100 ms; the pass. With a focus on another node since, instead: passes the key on, sends `End`, says nothing for it, ends, because its words would follow the focus's. |
| Reading | `Unanswered` | Passes the key on, says nothing for it, ends. |
| Reading | deadline fires | Withdraws the request, passes the key on; `Watched`, `NotRead`, `Holding`, arms 100 ms, request with `FromLastRead` naming the withdrawn one; the pass. With a focus on another node since, instead: withdraws, passes the key on, says nothing for it, ends, sending no request. |
| Reading | focus on another node executes | Stays `Reading`, judged at its answer or deadline as above. |
| Reading | the node's outpost ends | Passes the key on, says nothing for it, ends. |
| Watched | held key or command arrives, `Holding` | Joins `waiting`; marks `later_key_at_us`. |
| Watched | held key or command arrives, `Released` | A key becomes current, its request ending this watch. A command runs and this key stays. |
| Watched | typed character or editing chord | Runs at once if `waiting` is empty, else joins it. Its cut of speech marks `later_key_at_us`. |
| Watched | a caret key the hook passed | With `waiting` empty, replaces the current key as `Watched`, `NotRead`, `Holding`, arms 100 ms, its request ending this watch. Otherwise joins `waiting`. |
| Watched | `Evidence`, this tag, no later key or read before it | Takes the caret; says the key's words, then changes; ends. In a terminal it takes the caret, and the reading rules speak. |
| Watched | `Evidence`, this tag, later key marked and read at or after it | Takes the caret; says changes; ends. |
| Watched | `BoundAnswer`, this tag | Takes the caret. With no later key says the unit for a movement key or Delete, nothing for Backspace, a Shift key or a key outside the table. Says changes; ends. |
| Watched | `Ended`, this tag, or `Unanswered` to `FromLastRead` | Ends. |
| Watched | deadline fires, `Holding`, `TextField`, not a sentence key | Sends `FinalRead`, unless advancing would take a held key at once, whose request ends the watch. Becomes `Released`; advance. |
| Watched | deadline fires, `Holding`, a sentence key | Sends `FinalRead`; `MovingBySentence` in step `FinalRead`; arms 150 ms. |
| Watched | deadline fires, `Holding`, `Terminal` | Becomes `Released`; advance. The outpost's stage ends a terminal's watch. |
| Watched | speech cut | Marks `later_key_at_us` if not yet marked. |
| Watched | focus on another node executes, `TextField` | As "A focus that moves first". Sends `End` if `Holding`; ends. |
| Watched | focus on another node executes, `Terminal` | Ends. The outpost ends its watch by its own focus rule. |
| Watched | the node's outpost ends | Says nothing for the key; ends. |
| MovingBySentence | any key or command arrives | Joins `waiting`; marks `later_key_at_us`. |
| MovingBySentence, `FinalRead` | `BoundAnswer`, this tag | Takes the caret. In text with sentences, whether or not a later key is marked, goes to `ReadingParagraph`; otherwise says changes and ends. |
| MovingBySentence, `FinalRead` | `Evidence`, this tag | Takes the caret, since the application moved it. Says the line unless a later key is marked, then changes; ends. |
| MovingBySentence, `FinalRead` | `Ended`, this tag | Ends. |
| MovingBySentence | paragraph answered | Finds the sentence; reads the next or previous paragraph if the sentence is in it, or sends `MoveCaret`. |
| MovingBySentence | move answered | Takes the caret with the move's stamp. Says the sentence unless a later key is marked; ends. |
| MovingBySentence | deadline fires, or an answer fails | Ends, and the sentence does not move. In step `FinalRead` the next key's request ends the watch. |
| MovingBySentence | focus on another node executes | Withdraws the query, says nothing for the key; ends. |
| MovingBySentence | the node's outpost ends | Says nothing for the key; ends. |

The outpost:

| State | Rule | What it does |
|---|---|---|
| Last read absent | any read of the node commits | Becomes present. |
| Last read absent | a `ReadFirst` request arrives | The before read runs and becomes the baseline; if it fails, the answer is `Unanswered` and no watch opens. |
| Last read absent | a `FromLastRead` request arrives | Answers `Unanswered`; no watch opens. |
| Last read present | any read of the node commits | Is replaced; comparisons for that read use the old one. |
| Last read present | a key request's read fails | Answers `Unanswered`, no watch opens, and the last read stays, as only a failed focus read restarts a node's record. |
| Last read present | focus read fails | Becomes absent. |
| Last read present | release | The record and its ranges are dropped. |
| None | `ReadFirst` request arrives | Reads, answers `Read`, pins the baseline, becomes `Open`. The kind names `Awaiting` or a terminal's stage, the outpost choosing `Poll` for the console host. |
| None | `FromLastRead` request arrives | If it names a withdrawn request not yet started, cancels it. Pins the last read, answers `Watching`, becomes `Open`. |
| None or Open | a withdrawn request's read, already started, commits | Replaces the last read and goes as an untagged report, so what it compared is said. It opens no watch and leaves an open one as it is. |
| Open | the node's event | Becomes `Checking`, resuming its stage. |
| Open, `Awaiting` | `FinalRead` for its tag | Becomes `Checking` with `Answer`. |
| Open, `Terminal` | due | Sends `Ended`, becomes `None`. |
| Open, `Poll` | next due | Becomes `Checking`, resuming the poll. |
| Open, `Poll` | until passed | Sends `Ended`, becomes `None`. |
| Open or Checking | next key request, or Verbatim's caret move or selection on the node | Sends `Ended`, becomes `None`. A running check's read replaces the last read and goes untagged; the request runs after it, since programs run one at a time. A move becomes the last read. |
| Open or Checking | release, or the element replaced | Sends `Ended`, becomes `None`. A running check's read is dropped with the node's record, and on a replaced element it is untagged. |
| Open or Checking | `End` for its tag | Becomes `None`, and sends nothing, since Core has ended the key. A running check's read goes untagged. |
| Open or Checking, of `Awaiting` | focus elsewhere | Nothing: Core judges the focus and sends `End`. |
| Open or Checking, `Terminal` or `Poll` | focus elsewhere | Sends `Ended`, becomes `None`. |
| None, or a watch with another tag | `FinalRead` or `End` | Ignored: that watch's answer is on its way, or the key was replaced. |
| Checking | check finds evidence | Sends `Evidence`, becomes `None`. |
| Checking, `Resume` | check finds nothing | Goes back to its stage. |
| Checking, `Resume(Awaiting)` | `FinalRead` for its tag | Becomes `Checking` with `Answer`: the running check is the final read. |
| Checking, `Answer` | check finds nothing | Sends `BoundAnswer`, becomes `None`. |
| Checking | check fails or is abandoned | Sends `Ended`, becomes `None`. |

A watch Core has not ended gets exactly one answer: `Evidence`, `BoundAnswer` or `Ended`. A check's read that ends no watch is untagged. An ending outpost's keys are ended by Core's own record of that generation.

Core ends a text field's watch on every path, since the watch has no time of its own: the bound by the final read, a focus on another node by `End` (in `Reading`, at the read's answer), and the next key on that outpost by that key's request. A key on another node can only follow a focus on it, so the same `End` covers it. Verbatim's own caret move, release and a replaced element end the watch in the outpost. Core's exit and an ending outpost end it with the process.

## How a key runs

A held key's request reads the caret with the key's unit, answers once with that read, and opens the watch with the read pinned. An unheld key's request, or the request sent when a before read passes 150 ms, pins the last read instead. The outpost replaces a node's last read with every read: a focus read, a caret event's read, a before read, a watch's check, and Verbatim's own selection or caret move.

Every read reports how the selection changed from the last read and whether the text changed. A read that is a key's result goes as `KeyAnswered`, under "The messages". Through UIA these are compared in one remote program, and in a classic edit control by the edit control's messages.

Core takes every report with an accepted stamp as the newest caret, so a late answer never moves the review cursor back. Verbatim's own `MoveCaret` and `Select` answer with the time the move began, which the outpost gives the last read of the move too. A focus read compares with the node's last read when one exists, and reports the changes with the focus; Core speaks them for a repeat and ignores them for a new focus.

What Backspace removed is the difference between the result's before and after lines (`removal`). A line joined to the one above says the break in `line_break`. Backspace over a selection says what was before its start, taken from the result's before line by `deleted_text`, which now runs on the result and keeps no state. A caret that moved forward with nothing removed says "blank", as NVDA's empty deleted text does (`editableText.py` 311 to 320): the classic edit control's Control+Backspace inserts a character, so at the text's start the capture is "blank". Delete's unit is the before unit against the after unit. "Something was selected before" is the baseline's selection. `later_key`, the landing rule and `removes_selection` are unchanged.

A tagged report whose tag is not the current key's is an ordinary report. A superseded key therefore needs no slot, and away-and-back cases cannot speak a stale answer. Tagged reports are never merged and bar merging across them, so a newer report cannot replace a key's only result or overtake it.

### A focus that moves first

NVDA's wait ends as soon as a focus is pending (`editableText.py` 97 to 99), and its helper then reads the old field's caret and speaks its unit (165 to 178). Focus speech is queued rather than interrupting (`nvda/source/NVDAObjects/__init__.py` 1365 to 1369). Only caret speech's own `reason` is outside the focus-loss cancellation (`nvda/source/eventHandler.py` 299 to 300). Speech is cut only when the focus differs just below the desktop, where the foreground event cancels it (`eventHandler.py` 402 to 413, `NVDAObjects/__init__.py` 1387).

So when Core executes a focus on another node while the current key is `Watched` as `TextField`, with no later key and no result yet, Core first speaks what NVDA's script would, from the before read, and then executes the focus by its usual rules. It needs no extra read and adds nothing to the focus path, because a moved caret would have been evidence. This holds whichever process reports the focus, since Core alone decides it.

- Same window: Up Arrow in an empty chat box, focus moves to the editing field 40 ms later. Heard: "blank", "Edit message edit".
- A new top-level window, in this process or another: "blank" is queued and cut in the same step, as NVDA's foreground event cuts it. Heard: the new window and focus only.
- `before` is `NotRead`, because the key was unheld or its read was slow: nothing is said for the key, Backspace included.
- Backspace or Control+Backspace with `before` `Read`, only while the line is `Holding`, since past the 100 ms NVDA's wait has timed out and it says nothing (`editableText.py` 313 to 315): `removed`, which the before read's answer computes by the stateless `deleted_text`, from the selection's start when something is selected, as NVDA's caret is the selection's start (`nvda/source/NVDAObjects/window/edit.py` 464; `nvda/source/NVDAObjects/UIA/__init__.py` 497 to 498) and its script speaks what it read before the key (`editableText.py` 300 to 320). Backspace over "x": "x", "Edit message edit".
- Backspace or Control+Backspace at the start of a line that is not the text's first: the line break's name, as an ordinary Backspace there says it, NVDA's move having reached the break (`editableText.py` 302 to 309). Where neither that read nor the focus's caret knows the line break, nothing is said for the key.
- Backspace or Control+Backspace at the text's start: "blank", "Edit message edit", as NVDA's move returns 0, its deleted text is empty (307 to 311), its wait counts the focus as a move, and it spells the empty text (320), which the capture of "blank" shows.
- A later key: nothing is said, as NVDA's helper returns while a script waits (`editableText.py` 166).
- Alt+Up or Alt+Down: the line, as NVDA passes the line unit when the caret "moved" (`editableText.py` 255 to 262).

Core sends `End` for this key, so the watch closes at once.

### Moving by sentence

Alt+Up and Alt+Down are held as today. When the key's evidence shows the application moved the caret, Core speaks the line, as NVDA does. At a bound answer in text with sentences, Core moves by sentence.

- A key already waiting when the sentence key would start passes it on and does nothing else, as NVDA's helper does (`editableText.py` 241 to 244).
- Alt+Down moves to the next sentence's start, and from the caret's paragraph's last sentence to the next paragraph's first. From the text's last sentence it moves to the text's end, says "blank", as captured; at the text's end NVDA's move returns 0 and the empty text at the caret is read, also "blank" (`nvda/source/textInfos/offsets.py` 879 to 885; `editableText.py` 258 to 262).
- Alt+Up steps back one offset and takes that sentence's start, so from the middle of a sentence it lands at the current sentence's start (`offsets.py` 852 to 856); from a paragraph's first sentence it takes the previous paragraph's last. At the text's start it stays and reads the first sentence, as captured.
- The keys behind wait for the move, so that their before read sees it. A key pressed within the 100 ms is such a key: the sentence still moves, silently, as in NVDA (`editableText.py` 94 to 95, 253 to 255, 166).
- `MoveCaret` becomes the node's last read.

### Exact speech

Keys that move nothing, at the bound answer:

- Up, Down, Page Up, Page Down, Control+Home, Control+End: the line (`editableText.py` 268, gesture table 387 to 413).
- Left, Right, Home, End: the character at the caret, "blank" where there is none.
- Control+Left and Control+Right: the word. Control+Up and Control+Down: the paragraph.
- Delete, Shift+Delete and Control+Delete: the unit at the caret (`editableText.py` 329 to 342, gesture table 405, 409).
- Backspace, Shift+Backspace and Control+Backspace: nothing (313 to 315), where the caret stays.
- Shift keys: nothing. Alt+Up and Alt+Down: a sentence move or, in UIA, nothing.

Already recorded in `docs/parity/text.md` (lines 103 to 116, 220 to 226, 334 to 344, 365 to 375): Home at a line's start, Control+Home at the text's start, Down Arrow on the last line, Control+Up and Control+Down in the text box, Alt+Down "delta epsilon" in the text box, Escape, Control+Z and paste.

Captured from NVDA at its pinned build, in the Windows Forms text box and Notepad, with the lines "Apple pie is good.", "Banana bread is warm. Cherry tart is sweet." and "Date cake is rich.". `docs/parity/text.md` records these results in the first build.

- Up Arrow and Page Up on the first line, Page Down and Down Arrow on the last, and Up and Down Arrow in a one-line text: the line, in both.
- Left Arrow at the text's start: "A". Control+Left Arrow there: "Apple". In both.
- Right Arrow and Delete at the text's end: "blank", in both.
- Control+Right Arrow and Control+Delete at the text's end: "blank" in the text box, "dot" in Notepad, whose word unit at the end gives the final full stop.
- Backspace at the text's start: nothing, in both. Control+Backspace there: nothing in Notepad, "blank" in the text box.
- Control+Z, Control+Y, Control+X, Control+V and Escape: nothing for the key, in both; a selection the application's caret event shows is spoken, as "zz selected" after Control+Z in the text box.
- Text box, Alt+Up from within "Banana bread is warm.": that sentence. Alt+Down from it: "Cherry tart is sweet.". Alt+Up at the text's start: "Apple pie is good.". Alt+Down there: "Banana bread is warm.". Alt+Up from the text's end: "Date cake is rich.". Alt+Down from that last sentence: "blank".
- Notepad, Alt+Down: NVDA's script says nothing, because UIA has no sentence unit (`editableText.py` 263). What follows comes from the application's own Alt handling, which opened its menus and differed from run to run, so no scenario expects anything after it.
- Settings' search box with suggestions open: Down, Down, Up and Up Arrow each say the box's text and then the suggestion: "dis", "Storage settings  2 of 25"; "dis", "Language settings  3 of 25"; "dis", "Storage settings  2 of 25"; "dis", "Display settings  1 of 25".

## Limits

- A move more than 100 ms after its key is not spoken: the old unit is, as in NVDA. It still updates the caret.
- A key's evidence in flight when a focus from another process reaches Core is spoken as a key that moved nothing, the before read's unit.
- A focus executing while a key is still being read leaves that key silent, since its words would follow the focus's.
- A duplicate focus report does not stand for a focus that moves first: NVDA would speak the unit at once, and Verbatim speaks it at the bound.
- A key within a few milliseconds of an earlier key can be read before the earlier key's effect, as today.
- A key in an elevated window, until M8.6, is judged from the last read; one whose own event was read first may find no evidence and speak at its bound from the caret after the key.
- A rich edit control's sentences stop at a wrapped line's end until M6.5.
- A letter typed within 100 ms of a key that moves nothing leaves the key silent, as NVDA's speech for it is cut as it starts (`nvda/source/inputCore.py` 584 to 605).
- NVDA speaks as soon as a control raises a caret event, moved or not, about 8 ms after the key in the text box (`editableText.py` 104 to 120). Verbatim judges by position, so the same words come at 100 ms.
- Control+Backspace with a focus that moves first, from within a word, says the part of the word before the caret, where NVDA says the whole word, because `deleted_text` takes what the key would remove.
- A Control+Backspace that inserts a character says "blank" wherever it is. NVDA says "blank" only at the text's start, and elsewhere the word before the caret, because it reads that before the key (`editableText.py` 300 to 320).
- A final read or end refused because 64 commands wait leaves the outpost's watch open until the next key request or the node's release. It costs one read per event of that node meanwhile, and only when the pipe is already that far behind.

## Removed and kept

Removed in Core: `caret_history`, `CARET_HISTORY`, `TimedCaret`, `caret_before`, `caret_moved_by_core`'s time-zero record, `forget_caret`, `PendingCaret`, `HeldKeys::{passed_at_ms, awaiting_watch, bounded}`, `Then::{HeldRead, Caret, SupersededCaret}`, `superseded_reply`, `CaretContext::node`, `compared_text`, the scans `reading_held` and `find(Then::Caret)`, the effects scan in `pass_held`, and time zero as unknown.

Removed in the outpost: `CaretSeen`, `REMEMBERED_CARETS` and the search by press time, `KeptSelection::report`, `NodeAnchors::reports`, `LastSelection`, `ForgetSelection`, `CaretFor::Focus`'s restart, `Baseline::{selected, known_line}`, `compared_already`, `Signal::covered`, `CaretWatch::{since, pressed_at_ms, compare}`, the separate `AwaitCaret` request and its first check, and `MoveCaret`'s clearing of the history.

Kept: holding, deferral, the 100 ms wait for the key ahead (`KEY_AHEAD_BOUND_MS`), now also the key's bound, the 150 ms before read (`HELD_READ_BOUND_MS`), the recheck of non-atomic reads, general terms with the note and the WinEvent sync at the caret event, event coverage (`caret_read_since`), the read on every caret event, `heard` and `later_key`, `deleted_text` as a function of the result's before line, the console host's poll counted from the before read's end, and the terminal watch's handling deadline.

The barrier on replies and on tagged reports replaces the clause against merging a caret report across a reply.

## Costs

- Calls: a held key through UIA makes two round trips instead of three; classically two reads of 8 and 15 calls instead of 8, 15 and 15; in a classic edit control 5 and 6 messages instead of 5, 5 and 6.
- Wake-ups: one request fewer per held key, against the measured 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time. The deadline is one of the reducer thread's, so it adds no wake-up source. A key answered by evidence costs nothing more. A key that moves nothing costs one wake of Core, one message, and one read and report more.
- Latency: key to first sound is unchanged for a key that moves. A key that moves nothing speaks at 100 ms, as NVDA does, where today it is silent.
- Remote operations: the caret program stays under 5,000 instructions with the baseline's selection.
- Memory: one last read per node and one pinned baseline per outpost, against eight reads and a kept selection in the outpost and eight timed reports in Core.
- Bytes: the result carries the before line, usually under a hundred bytes.

## Order of work

1. The captures are taken; their results are under "Exact speech".
2. The key lifecycle build. Generated models and worlds first, by an agent other than the builder, committed failing on known defects, and run against a prototype of the new reducers before the build is approved, every seed passing. Then `KeyLine`, the outpost's `Watch` and `LastRead`, the one key request, tagged reports, the stamp, the bound answer, the focus rule, the slow-read path, and the removals. The `caret_keys` and `settings_search_suggestions` expectations for unmoved keys change here.
3. The keys as NVDA build. The editing chords from the hook to Core, the sentence move and its generated-test addition. The remaining expectations change here.
4. Finish. Measurements, the listed scenarios, and the documents: D14, the key-holding explanation, the parity ledgers, the crate guides, the milestone rule, `docs/roadmap.md`'s entries.

## Tests

Every test follows `books/dev/src/reference/testing.md`. The bound is an input to Core and a message to the outpost, so no test of it waits on real time.

Generated tests, for ordering and lifecycle only.

Both models are written from the parity docs (`docs/parity/text.md`, `docs/nvda/editable-text-and-terminals.md`) and Dickson's dated decisions above, not from Claude's decisions or the design's states, by an agent other than the builder.

The Core model is a record per key press. Its world is an editor whose events come early, late, twice or never, with distinct traces, several keys, slow answers, focus reports with keys queued behind them, reports behind focus, the outpost ending and exit. After each step it checks: every key reaches the application once, in press order, and within its bounds; nothing is withheld after exit; the caret is the one read most recently begun among those delivered; each selection change is heard once and after the words of the key that made it; a key speaks at most once, and not at all once a later key was pressed, except for that key's selection changes. It replaces the skipped comparison in `crates/verbatim-core/src/queries/generated.rs`. Of that file's three loose focus checks, the focus-taken and left-mark checks go, since `live_reread/generated.rs` models both; the foreground check's assertion stays.

The outpost model holds, per node, the application's caret and selection at each moment and the newest read taken of them, and per key the state at the key's arrival. Its world is an application moving the caret and selection with independently timed events, with Core's orders arriving at any step, programs finishing or abandoned, release, replaced elements and a focus elsewhere. After each step it checks: a key answers that it had an effect exactly when the application's state differs from its state at arrival, as the decisions define evidence; a key Core has not called off is answered once and a key Core called off gets no answer after the call-off is handled; the before read's answer precedes any other. It replaces the four invariants in `crates/verbatim-outpost-logic/src/tests/generated.rs`.

Hand-written tests cover words and single rules only:

- Core: the deadline input with no evidence sends `FinalRead`, and evidence first clears the deadline so that no `FinalRead` is sent, with no real time. A focus that moves first sends `End`.
- Core: `removal`, segments, the speech per key class at evidence and at the bound, a caret that moved forward with nothing removed saying "blank", the focus-moves-first rule per case above, the chords, and sentence landing.
- The outpost: selection-change text, general terms, cut reads, and the remote and classic reads' agreement.
- mockapp: Control+A followed by a repeated focus says the count once; a mouse selection before a before read is said first; the call counts and instruction limit.
- The hook: a chord is reported with its gesture.

Removed: every test pinning a removed mechanism, and every hand-written interleaving test the generated models replace, including those for superseded keys and for waiting on the key ahead. Tests that change only for the request's new shape are updated. A builder that finds another test breaking reports it.

Scenarios: `notepad_caret_keys`, `text_box_caret_keys`, `text_box_paragraph_keys`, `text_box_editing`, `notepad_editing`, `text_box_selection_keys` including Control+Delete over "alpha " ten times, `settings_search_suggestions`, `wpf_password` and `terminal_key_timing`.

Measurements: call counts; instruction counts; the stale caret workloads, 0 of 60 misjudged; wake-ups and processor time over `measure_keys_text_box`, `measure_keys_windows_terminal` and a Notepad key workload before and after; key to first sound P50 and P95 unchanged.

## Why this design

- Keeping the second layer leaves nine places of state, two known defects and three round trips per held key.
- Core sending its selection with every request brings back two baselines and gives caret events nothing to carry it.
- Core comparing raw ends cannot order positions across chunks or read the text between them.
- The outpost reporting every read's facts and Core judging evidence needs a message per key or reads twice per typed character, since the outpost must know when a key is answered to stop reading on text changes.
- A second path that trusts events where they are reliable brings back the race between the hook and the application's events, which is one of ordering.
- A several-answer request keeps a passive stage in Core's query table and carries results apart from the caret reports they must be ordered with.
