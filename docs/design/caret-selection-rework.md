# The caret and selection rework

Status: scope and decisions agreed by Dickson on 2026-10-25, 2026-10-29, 2026-11-08 and 2026-11-09; approved by Dickson on 2026-11-09. It includes the held-key line, one of the state models in `docs/roadmap.md`.

## In short

A caret key is still held while the focus's outpost reads the caret, as NVDA's caret scripts read the caret before sending the key. Core then owns the key's whole life: it passes the key on, counts its 100 ms, decides whether a report was the key's evidence, and chooses every word. The outpost has no key lifecycle. It reads, compares each read with the node's last read, and reports. For a short time after a key's request it keeps one note on the node, saying which unit its reads also fetch and that text changes are read too. The note ends by the clock, so nothing has to end it.

Four rounds of defects came from one lifecycle held twice, `CurrentKey` in Core and `Watch` in the outpost, with each side able to end it and no protocol between them. The note leaves one side with no lifecycle, so the states those defects lived in do not exist.

The rework fixes two known defects: Control+A followed by a repeated focus says nothing, and a superseded key's answer leaves Core's caret stale. Keys that move nothing speak as NVDA's do, Alt+Up and Alt+Down move by sentence, and five editing chords become silent.

## Terms

- Core is Verbatim's main process, and its reducer decides what is spoken. An outpost is the helper process that makes one application's UIA or MSAA calls. A remote program is a small UIA program run inside the application, so that many reads cost one round trip.
- A held key is a caret-table key the keyboard hook swallows while the focus has text, until Core passes it on.
- The before read is the read of the caret made for a held key before it is passed on.
- The last read is the outpost's one record per text node: caret, selection with its text up to 513 UTF-16 units, the caret's line and offset in it, the note that the text changed, and the time the read began.
- The note is what a text node's record holds for a short time after a key's request: the key's unit, its motion and the time it ends. It is a statement about what to read, and it knows nothing of the key's progress.
- A comparison is what the outpost finds comparing a read with the node's last read: whether the caret left where it was, the selection changed, the characters beside the caret changed, the unit at the caret changed, or the caret's line changed.
- Evidence is a report whose read ended at or after the key was passed on, on the key's node, whose comparison shows the key had an effect: the caret having left where it was, the characters beside it changing, the unit at the caret changing after Delete, the caret's line changing after a key that deletes, or the selection changing.
- The key's bound is 100 ms after Core passed the key on, or after the key reached Core for a key that was not held.
- The final read is the read Core asks when the key's bound passes with no evidence. The bound answer is that read's result when it shows no effect.
- General terms are NVDA's words when the text changed since the selection was last compared: the whole new selection "selected", "selected instead", or "selection removed".

## Who owns each fact

| Fact | Owner | Others |
|---|---|---|
| The key's life, what is waiting behind it, what is held to be said | Core, one value | None |
| When the key was passed on, and its bound | Core | None |
| Whether a report is the key's evidence, and every word | Core, from a report's facts | None |
| The newest caret | Core, by the stamp its read began | Each report is a value in flight |
| Positions, and each comparison of them | The outpost, inside the read | Core receives the results only |
| The last read of a node | The outpost, one record per node | None |
| Whether to read, and what to fetch | The outpost, from its note | Core sets the note and cannot read it back |
| The caret itself | The application | Every read is a sample of it |

No message says what state the sender is in, and no message asks the other side to change state, so nothing can be stranded or ended twice. The one real dependence between reads is the baseline: every committed read replaces the node's one last read, so an ordinary report that shows a key's effect consumes it, and the next read compares with the state after the effect. Which reads carry a key's facts therefore decides what a key can hear.

## Decisions

Dickson's, each with its date:

- One baseline (2026-10-25). One last read per text node, replaced by every read, is the only baseline.
- The newest caret by one stamp (2026-10-25). Core takes every report as the newest caret by the time its read began, in microseconds. A report also carries when its read ended, used only to judge evidence, the one exception to one stamp per report (2026-11-09).
- A node's last read lives while Core holds the node, and Core's choice between a new and a repeated focus decides what is spoken: a repeated focus's changes are spoken, a new focus's are not (2026-10-25, amended 2026-10-29).
- A key that moves nothing speaks as NVDA's script does (2026-10-29). A movement key and Delete speak their unit. Backspace and Control+Backspace say nothing (`nvda/source/editableText.py` 313 to 315). A Shift key that changes nothing says nothing, since auto-detection reports only a change at the caret event (`editableText.py` 451).
- The 100 ms bound is Core's (2026-11-09), NVDA's `caretMoveTimeoutMs` (`nvda/source/config/configSpec.py` 364), counted from when the key is passed on, as NVDA counts from after sending the key (`editableText.py` 188 to 189). Only Core knows that moment. It is one of the reducer thread's deadlines, evidence cancels it, and when it passes Core asks the final read.
- The bound answer ends the key (2026-11-08). There is no edge-of-text rule and no later evidence, because NVDA has neither (`editableText.py` 86 to 87, 150 to 163). A move later than 100 ms is not spoken.
- A focus that moves within the 100 ms matches NVDA's source (2026-11-08), as "A focus that moves first" describes.
- Backspace and Control+Backspace with a focus that moves first match NVDA (2026-11-09), which reads the deleted text before the key and speaks it because its wait counts a pending focus as a move (`editableText.py` 97 to 99, 300 to 320). Example: Backspace over "x" in a chat box with the focus moving 40 ms later is heard as "x", "Edit message edit".
- A held key whose before read passes 150 ms is passed on and judged from the node's last read, as a key that was not held is (2026-11-08). The request's arrival already set the note, so no further message is needed.
- Core owns the key's whole life and decides whether a report was its evidence, from the comparisons the outpost makes; the outpost has no watch, tags or orders (2026-11-09). This replaces "the judgement of evidence stays in the outpost" (2026-10-25), and `KeyAnswered` and tagged reports (2026-11-08 and 2026-11-09).
- The outpost keeps on a node's record only a note of the unit, the motion and the time it ends. It keeps the unit, not fetching it always, and expires by the clock. An effect stops only the note's extra reads on text changes: caret-event reads keep fetching the unit until the note ends (2026-11-09).
- A read counts as a key's evidence by when it ended: a read that ended after the pass and shows a change is the key's, so any read in flight at the pass can be, not only a before read slower than 150 ms (2026-11-09). Backspace over "x" with a read in flight at the pass says "x", as NVDA's script does (`editableText.py` 293 to 320). Right, Right with the first key's late move in that read says "b", the first key's landing, and "c" goes unsaid. A mouse selection made just before the pass and caught by that read counts as the key's.
- It must not perform worse than today, against the baseline measured at 6e6c1efa: no added reads, calls, wake-ups or latency. A prototype measures that before the build is approved. Any extra call goes to Dickson as one sequence with its count (2026-11-09).
- A key whose before read is running when the focus moves to another node says nothing, a difference from NVDA, whose script speaks the old field's unit before the focus (`editableText.py` 97 to 99, 165 to 192), because in Verbatim the focus has already been spoken and the unit would follow it (2026-11-09).
- A sentence move whose final read or later step passes its 150 ms is dropped: nothing is said and the caret stays, a difference from NVDA, which blocks until it can read and then moves (`editableText.py` 245 to 262), so that keys behind it are not held longer (2026-11-09).
- A command pressed within 100 ms of a caret key that was not held waits for it, at most those 100 ms, the one added delay in this design, ending with uiAccess (M8.6) (2026-11-09).
- Alt+Up and Alt+Down match NVDA (2026-10-29): sentences by UAX #29 within each paragraph; in UIA text, which has no sentence unit, nothing is said (`editableText.py` 263; `nvda/source/UIAHandler/__init__.py` 158 to 176). A rich edit control is read as a plain edit control until M6.5.
- Control+Z, Control+Y, Control+X, Control+V and Escape in a text field set no note and say nothing for the key, as NVDA binds no script to them (2026-10-25; absent from `editableText.py` 387 to 413). Their selection changes are spoken from the application's caret event. Other keys outside the caret table speak by their evidence (2026-10-08).
- Terminals keep their reading rules and today's handling of the five chords (2026-10-25 and 2026-10-29).
- Until uiAccess (M8.6), a caret key in an elevated window is judged from the last read (2026-10-25).

Claude's, each with its reason:

- No value-change sync on the before read: NVDA checks a waiting value change only at the caret event (`nvda/source/NVDAObjects/window/edit.py` 1068 to 1081), and the sync would add a wake before each key.
- Core alone says whether a key's node is a terminal, from its role, and its requests are different variants for a text field and a terminal, because the outpost's own test (the window class, or a UIA console focus) disagrees with the role on an MSAA console.
- A bound read that fails or is abandoned says nothing, as `_caretScriptPostMovedHelper` returns when it cannot read (`editableText.py` 168 to 172).
- A focus read that fails drops the node's last read, as `initAutoSelectDetection` starts again (`editableText.py` 421 to 424).
- A text field's note lasts 300 ms from the request's arrival: the 150 ms bound on the before read, the 100 ms bound, and a margin for the pipe. Only an unusual path reaches that end, since the final read or the next key's request clears or replaces it first.
- A note that has passed its end counts as absent where it is read, so no timer ends it and no wake is added.
- A terminal's note lasts 300 ms, as a text field's does, because its key ends at its 100 ms bound: longer would fetch for no one, and a report with facts cannot be merged, which streaming redraws would pay for. The console host, which raises no prompt caret event, polls during its note (`TerminalReads::Poll`), with today's 5 ms interval and 50 ms end (`POLL_BOUND`). This is the one thing the note holds beyond the unit, motion and end.
- A terminal's key ends at its 100 ms bound, with no final read: keys behind go ahead, and what the terminal did reaches Core as ordinary reports.
- A final read clears the note when it commits, so that a read in flight at its arrival still gets its facts (Dickson's decision of 2026-11-09), and it goes ahead of queued reads, so text changes are read no longer than today.
- A key with no previous read to compare with has no evidence, and at its bound says its unit, as any bound answer does.
- A deadline due during a burst of queued work is acted on when the reducer drains it, as every reducer deadline is today.
- A key outside the caret table that the hook reports, which today is a clearing key reported as `CaretMotion::Other`, has the same 100 ms bound and says nothing at it, so that one lifecycle covers every key. An effect later than that is not spoken.
- Each step of a sentence move reuses the held key's 150 ms deadline, because no second timer kind is needed.
- The key's bound and the wait of the key behind are one deadline, 100 ms after the pass, because they are the same interval from the same moment, and two would be a copied fact.
- A key that was not held holds its line as a passed key does, so that one lifecycle covers every key. Keys the hook passes run at once, as today.
- A final read already asked when the key is replaced needs no message: its reply is an ordinary report.
- `Input::SpeechCancelled` carries microseconds, so that it compares with a read's stamp directly.
- A report carrying a key's facts is never merged and bars merging across it, its `merge_key` being `None` and its barrier `Barrier::Reply`, as a focus report answering a live re-read has, because a merged report loses its comparison.
- The comparison judged by `had_effect` lives in `verbatim-model`, original code from `docs/parity/text.md`, so that the outpost can narrow its note and Core can decide with one definition.
- The first test of a key's later keys is by stamp only: time zero meaning unknown goes.

## Where each part runs

- The hook (`verbatim-input-windows`) holds and defers keys as today, and reports a key of the editing chords' list with its gesture.
- `verbatim-input` keeps the caret table and gains the chords' closed list.
- The shell (`verbatim-app`) turns a reported chord into its own input to Core.
- The model (`verbatim-model`) has the read request, the note message, the report's facts, `Compared` and `had_effect`.
- The outpost's reducer (`verbatim-outpost-logic`) holds each node's last read and note in the node's record, so they end with the node, and owns the console poll's due time.
- The outpost's programs (`verbatim-outpost`) make every read and comparison, committing through `TextChange` as today. They read the note, and a stale note is harmless.
- Core (`verbatim-core`) holds the key line, the key's bound and one caret on the focus object, and decides evidence, every word and the sentence move.
- Sentences are found as say-all finds them: the outpost answers with the paragraph or as unsupported, and `verbatim-text` splits it.
- Review presses are entries of the same key line (`docs/design/m42-parity.md`), with no queue of their own: a `Review` variant of `CurrentKey`, which that build adds, bounded by the same deadlines.

## The state

### Core

```rust
enum KeyLine { Idle, Busy(Box<Busy>) }

struct Busy {
    current: CurrentKey,
    waiting: VecDeque<WaitingKey>,   // behind the current key, in press order
    heard: Vec<UtteranceSegment>,    // selection changes held for the next words
}

enum CurrentKey {
    Reading { query: QueryId, node: NodeId, key: HeldKey },
    Watched(Watched),                // passed on, within its bound
    FinalReading { query: QueryId, field: FieldKey },     // a text field's key only
    MovingBySentence { node: NodeId, key: KeyInfo, later_key_at_us: Option<u64>, step: SentenceStep },
}

// A text field's key and a terminal's are different after the release
// (Dickson, 2026-11-09): a terminal's has no before unit, no final read and
// no sentence step, because its reading rules speak.
enum Watched { Field(FieldKey), Terminal(PassedKey) }

struct PassedKey {                   // what every key passed on has
    node: NodeId,
    key: HeldKey,
    passed_at_us: u64,
    later_key_at_us: Option<u64>,
}

struct FieldKey { passed: PassedKey, before: Before, awaiting_unit: bool }

enum Before { Read { unit: TextChunk, line: TextChunk, removed: Option<String> }, NotRead }

enum SentenceStep {
    FinalRead { query: QueryId },
    ReadingParagraph { query: QueryId, paragraph: ParagraphAt },
    Moving { query: QueryId, sentence: String },
}
```

An idle line with keys waiting cannot be written, and a key past its bound cannot be mistaken for one within it, since `Watched` means within the bound and `FinalReading` means past it, and a terminal's key cannot reach a final read, since only `Watched::Field` becomes `FinalReading` or `MovingBySentence`; the built `Line` enum goes. The state decides the one deadline: armed in `Reading` (150 ms), `Watched` (100 ms) and `MovingBySentence` (150 ms), and in no other state. Entering a state sets it.

Four rules cover every ending, so the tables do not repeat them:

- To end the current key: clear its deadline, say what is left in `heard`, and advance. With no key left the line is `Idle`.
- To advance: take `waiting` in order, running typed keys and commands as the inputs they came as, until a held key or a caret key the hook passed is reached, which becomes current.
- At the pass, when a key becomes `Watched`: typed keys at the front of `waiting` run at once, stopping at the first command or held key, as NVDA's typed keys are not held behind a script, and a key still waiting, in `waiting` or behind a focus report, marks `later_key_at_us` with when it was pressed, so a key whose before read was given up says nothing for a read begun after that press.
- Saying selection changes always says `heard` first, and a key's words come before both.

The caret lives on the focus object as `{line, selection, line_break, read_at_us}`, so it ends with the focus. Its stamp is mandatory. `selection` is optional and holds the selection's start's own line when that differs from the caret's, so a selection line with no selection cannot be written, on the wire as in Core.

The focus queue stays in front: a key that reaches Core while focus reports wait goes into `state.focus` and enters `KeyLine` in its turn. `held_nodes` names `current`'s node directly.

### The outpost

```rust
struct KeyNote { unit: TextUnit, motion: CaretMotion, until_us: u64, reads: NoteReads }

enum NoteReads { Field(FieldReads), Terminal(TerminalReads) }

enum FieldReads { CaretAndText, CaretOnly }

// The console host's poll belongs to a terminal's note only.
enum TerminalReads { CaretAndText, CaretOnly, Poll { next_us: u64, until_us: u64 } }

struct NodeRecord<P> {
    anchors: NodeAnchors<P>,
    last: Option<LastRead<P>>,
    note: Option<KeyNote>,
}

enum NodeText { Uia(NodeRecord<RangeKey>), EditControl(NodeRecord<u32>) }

struct TextFacts { nodes: BTreeMap<u64, NodeText> }
```

`LastRead` keeps its fields, with the unit added when the read fetched one.

### The messages

```rust
struct KeyFetch { unit: TextUnit, motion: CaretMotion }

// TextOp::ReadCaret, one query with one reply.
struct CaretRead { fetch: KeyFetch, purpose: ReadPurpose }
enum ReadPurpose { BeforeFieldKey, BeforeTerminalKey, Final }  // Final: a text field's key only

// CoreToOutpost::KeyNote, one-way, for a key with no before read.
enum NoteRequest {
    Field { node: NodeId, fetch: KeyFetch },
    Terminal { node: NodeId, fetch: KeyFetch },
}

// CoreToOutpost::ReadTerminal(NodeId), one-way: at a key's pass in a terminal
// that is not the console host, read it now. No reply and no lifecycle.

// On a CaretReply, and on NormalizedEvent::CaretMoved as Option<ReadFacts>:
// present exactly when the read was made under a note or asked for one.
struct ReadFacts { unit: TextChunk, change: Change, read_ended_at_us: u64 }

enum Change {
    NoPrevious,
    Compared { compared: Compared, previous_line: TextChunk, previous_selection: Option<CaretSelection>, on_previous_line: bool },
}
```

A report with no `ReadFacts` cannot be taken for evidence, and a comparison cannot exist without what it compared with. `KeyRequest`, `KeyStart`, `KeyStarted`, `KeyTag`, `WatchOrder`, `CoreToOutpost::Watch`, `KeyAnswered`, `KeyAnswer` and `KeyRead` do not exist.

### The states and the rules that read them

Core. "Ends", "advance", "the pass" and "says changes" are the four rules above. "Evidence" and "ordinary" classify a report as the Terms define them.

| State | Rule | What it does |
|---|---|---|
| Idle | held key arrives | Becomes `Reading`, arms 150 ms, asks `ReadCaret` for `BeforeFieldKey` or `BeforeTerminalKey`, as the focus's role says. |
| Idle | a caret key the hook passed | Becomes `Watched`, `NotRead`, arms 100 ms, sends `KeyNote`. |
| Advancing | a sentence key taken with keys still behind it | Passes it on, sends no note, says nothing, and ends, as NVDA's helper does (`editableText.py` 241 to 244). |
| Advancing | a caret key the hook passed, taken from `waiting` | Becomes `Watched`, `NotRead`, its pass the time it reached Core, its bound 100 ms after that, firing at once if past; sends `KeyNote`. |
| Idle | typed character, clearing key's forgetting of typing, or command | Runs at once. |
| Idle | editing chord in a text field | Cuts speech and does nothing else. |
| Idle | caret report | Takes the caret if newest by stamp and the node is the focus; says changes. |
| Idle | focus executes | As today. |
| Busy | ordinary report, or a reply to a query no longer current | Takes the caret if newest. In `Watched` or `FinalReading` its changes go to `heard`; otherwise says changes, since no key is in the application's hands. |
| Busy | focus on the key's own node executes | A repeat carrying evidence takes the evidence row, in `Watched` or `FinalReading`. Otherwise a repeat's changes go to `heard` and a new focus's are ignored; nothing else changes for the key. |
| Busy | exit | Passes every held key on, in press order. |
| Reading | any key or command arrives | Joins `waiting`. |
| Reading | before read answered | Takes the caret, says changes, passes the key on; `Watched`, `Read`, arms 100 ms; the pass, and in a terminal other than the console host `ReadTerminal`. With a focus on another node since, instead: passes the key on, says nothing for it, ends, because its words would follow the focus's. |
| Reading | `Unanswered` | Passes the key on, says nothing for it, ends. |
| Reading | deadline fires | Withdraws the query, passes the key on; `Watched`, `NotRead`, arms 100 ms; the pass. With a focus on another node since, instead: withdraws, passes the key on, says nothing for it, ends. |
| Reading | focus on another node executes | Stays `Reading`, judged at its answer or deadline as above. |
| Reading | the node's outpost ends | Passes the key on, says nothing for it, ends. |
| Watched | held key or command arrives | Joins `waiting`; marks `later_key_at_us`. |
| Watched | typed character or editing chord | Runs at once if `waiting` is empty, else joins it. Its cut of speech marks `later_key_at_us`. |
| Watched | a caret key the hook passed | With `waiting` empty, replaces the current key as `Watched`, `NotRead`, arms 100 ms, sends `KeyNote`. Otherwise joins `waiting`. |
| Watched | evidence, no later key or read begun before it | Takes the caret; says a `Field` key's words, then changes; ends. A `Terminal` key has no words of its own: the reading rules speak. |
| Watched | evidence, later key marked and read begun at or after it | Takes the caret; says changes; ends. |
| `Watched::Field` | deadline fires, not a sentence key | Advances; if this key is still current, asks `ReadCaret` for `Final` and becomes `FinalReading`. |
| `Watched::Field` | deadline fires, a sentence key | Asks `ReadCaret` for `Final`; `MovingBySentence` in step `FinalRead`; arms 150 ms. |
| `Watched::Terminal` | deadline fires | Ends. |
| Watched | speech cut | Marks `later_key_at_us` if not yet marked. |
| `Watched::Field` | focus on another node executes | As "A focus that moves first"; ends. |
| `Watched::Terminal` | focus on another node executes | Ends. |
| Watched | the node's outpost ends | Says nothing for the key; ends. |
| FinalReading | held key arrives | It becomes current, and this key is replaced. |
| FinalReading | command arrives | Runs, and this key stays. |
| FinalReading | typed character or editing chord | Runs at once. Its cut of speech marks `later_key_at_us`. |
| FinalReading | a caret key the hook passed | Replaces this key, as in `Watched`. |
| FinalReading | evidence report, or the reply showing an effect | As `Watched`'s evidence rows; the other's arrival is then ordinary. |
| FinalReading | the reply showing no effect | The bound answer. Takes the caret. With no later key says the unit for a movement key or Delete, nothing for Backspace, a Shift key or a key outside the table. Says changes; ends. |
| FinalReading | `Unanswered` | Says nothing for the key; ends. |
| FinalReading | speech cut | Marks `later_key_at_us` if not yet marked. |
| FinalReading | focus on another node executes | Says nothing for the key; ends. Its reply is ordinary. |
| FinalReading | the node's outpost ends | Says nothing for the key; ends. |
| MovingBySentence | any key or command arrives | Joins `waiting`; marks `later_key_at_us`. |
| MovingBySentence, `FinalRead` | the reply showing no effect | Takes the caret. In text with sentences, whether or not a later key is marked, goes to `ReadingParagraph`; otherwise says changes and ends. |
| MovingBySentence, `FinalRead` | evidence report, or the reply showing an effect | Takes the caret, since the application moved it. Says the line unless a later key is marked, then changes; ends. |
| MovingBySentence | speech cut | Marks `later_key_at_us` if not yet marked. |
| MovingBySentence, `ReadingParagraph` or `Moving` | the application's report of a moved caret | Takes the caret if newest; says changes. The step goes on from the read it began with, as NVDA's move works from the position it read (`editableText.py` 245 to 255). |
| MovingBySentence | paragraph answered | Finds the sentence; reads the next or previous paragraph if the sentence is in it, or sends `MoveCaret`. |
| MovingBySentence | move answered | Takes the caret with the move's stamp. Says the sentence unless a later key is marked; ends. |
| MovingBySentence | deadline fires, or an answer fails | Ends, and the sentence does not move. |
| MovingBySentence | focus on another node executes | Withdraws the query, says nothing for the key; ends. |
| MovingBySentence | the node's outpost ends | Says nothing for the key; ends. |

The outpost, per node. A note past its end counts as absent.

| State | Rule | What it does |
|---|---|---|
| Last read absent | any read of the node commits | Becomes present; the report's change is `NoPrevious`. |
| Last read present | any read of the node commits | Is replaced; the read compared with the old one. |
| Last read present | focus read fails | Becomes absent. |
| Any | release, or the element replaced | The record and its ranges are dropped, the note with them. |
| Note absent or live | `ReadCaret` for `BeforeFieldKey` or `BeforeTerminalKey` arrives | The note is set or replaced at arrival, `Field` or `Terminal`, `CaretAndText`. On the console host, the before read's commit makes a `Terminal` note `Poll`, ending 50 ms after that read ended (`POLL_BOUND`), so a slow read does not use up the poll. |
| Note absent or live | `KeyNote` arrives | The note is set or replaced at arrival: a `Field` request's `CaretAndText`, a `Terminal` request's `Poll` on the console host, ending 50 ms after arrival, and `CaretAndText` otherwise. |
| Note absent or live | `ReadCaret` for `Final` arrives | The read fetches the request's unit, ahead of queued reads; the note is cleared when it commits. |
| Note absent | the node's caret event | Reads, as today, with no unit and no `ReadFacts`. |
| Note absent | the node's text-change event | No read. |
| Note live | the node's caret event | Reads, fetching the unit, with `ReadFacts`. |
| Note live, `CaretAndText` or `Poll` | the node's text-change event | Reads, fetching the unit, with `ReadFacts`. |
| Note live, `CaretOnly` | the node's text-change event | No read. |
| Note live | a focus read of the node | Reads, fetching the unit, with `ReadFacts`, so a repeated focus cannot consume the key's effect. |
| Note live, `Poll` | its next due time, before `until_us` | Reads as above; `next_us` moves on 5 ms. |
| Note live, `Poll` | `until_us` passed | The polling stops: the note becomes `CaretAndText` until its end. |
| Note live | a read reports an effect by `had_effect` | The note becomes `CaretOnly`, so a poll stops and later text changes are not read, and caret-event reads keep their facts until it ends. |
| Note live | a read reports no effect | The note stays. |
| Any | a read fails or is abandoned | No report; the note and the last read stay. |
| Note live | Verbatim's own caret move or selection on the node | The note is cleared; the move becomes the last read. |

Every query has one reply. Nothing else is promised by either side, and nothing waits on the note.

## How a key runs

A held key's request reads the caret with the key's unit, and answers once. Core passes the key on and starts its bound. Events on the node cause reads, each compared with the one before and reported with its facts. The first report whose read ended at or after the pass and shows an effect is the key's evidence, and Core speaks. If the bound passes first, Core asks one more read: no effect there is the bound answer.

Every committed read replaces the one baseline, so the first read to see a key's effect is the only one that can report it. That read is the key's evidence only if it carries the key's facts and qualifies by its time; if it is ordinary, the effect is consumed and the key speaks as one that moved nothing. So a read of the node has the note if the note lived at its dispatch or at its commit, and then carries the facts, whichever event or request caused it, a focus read included. It makes the decision "one last read is the only baseline" true as written: there is no pinned copy.

Every read reports how the selection changed from the last read and whether the text changed. Through UIA these are compared in one remote program, and in a classic edit control by the edit control's messages. Selection logic is otherwise unchanged, and Control+A followed by a repeated focus is fixed by the last read alone:

- A report whose read ended before the key was passed on is not the key's doing, and its changes are said at once, as a mouse selection made just before the key is.
- The first report after the pass showing a change is the key's evidence: the key's words, then its selection changes. Shift+Right says the selected text.
- Other reports while the key is within its bound or final read are held in `heard` and said after the key's words, and before a focus that moves away.
- Backspace over a selection takes "something was selected before" from the before read, and the report's previous line and selection carry the rule for a key with no before read.

Core takes every report with an accepted stamp as the newest caret, so a late answer never moves the review cursor back. Verbatim's own `MoveCaret` and `Select` answer with the time the move began, which the outpost gives the last read of the move too. A focus read compares with the node's last read when one exists, and reports the changes with the focus; Core speaks them for a repeat and ignores them for a new focus.

What Backspace removed is the difference between the report's previous line and its line (`removal`). A line joined to the one above says the break in `line_break`. Backspace over a selection says what was before its start, by `deleted_text`, which keeps no state. A caret that moved forward with nothing removed says "blank", as NVDA's empty deleted text does (`editableText.py` 311 to 320). Delete's unit is the before unit against the unit now. `later_key`, the landing rule and `removes_selection` are unchanged.

### Terminals

A terminal keeps its reading rules. Core's requests for it are the `Terminal` variants, the before read stays a read of the terminal, a terminal never compares selections, and its key is released at its bound as above. Its note ends after 300 ms, and the console host's note polls for 50 ms after the before read's end. A report of a terminal's caret only keeps Core's caret current, and the rules speak.

### A focus that moves first

NVDA's wait ends as soon as a focus is pending (`editableText.py` 97 to 99), and its helper then reads the old field's caret and speaks its unit (165 to 178). Focus speech is queued rather than interrupting (`nvda/source/NVDAObjects/__init__.py` 1365 to 1369). Only caret speech's own `reason` is outside the focus-loss cancellation (`nvda/source/eventHandler.py` 299 to 300). Speech is cut only when the focus differs just below the desktop, where the foreground event cancels it (`eventHandler.py` 402 to 413, `NVDAObjects/__init__.py` 1387).

So when Core executes a focus on another node while the current key is `Watched` in a text field with no later key, Core first speaks what NVDA's script would, from the before read, then says `heard`, and then executes the focus by its usual rules. It needs no extra read and adds nothing to the focus path, because a moved caret would have been evidence. This holds whichever process reports the focus, since Core alone decides it. Core sends nothing: the note ends by itself.

- Same window: Up Arrow in an empty chat box, focus moves to the editing field 40 ms later. Heard: "blank", "Edit message edit".
- A selection change held during the key is said before the focus (Dickson, 2026-11-09): Up Arrow in a chat box, "x selected" reported during the key, focus moving at 40 ms. Heard: "blank", "x selected", "Edit message edit".
- A new top-level window, in this process or another: "blank" is queued and cut in the same step, as NVDA's foreground event cuts it. Heard: the new window and focus only.
- `before` is `NotRead`, because the key was unheld or its read was slow: nothing is said for the key, Backspace included.
- Backspace or Control+Backspace with `before` `Read`, only while the key is `Watched`, since past the 100 ms NVDA's wait has timed out and it says nothing (`editableText.py` 313 to 315): `removed`, which the before read's answer computes by the stateless `deleted_text`, from the selection's start when something is selected, as NVDA's caret is the selection's start (`nvda/source/NVDAObjects/window/edit.py` 464; `nvda/source/NVDAObjects/UIA/__init__.py` 497 to 498) and its script speaks what it read before the key (`editableText.py` 300 to 320). Backspace over "x": "x", "Edit message edit".
- Backspace or Control+Backspace at the start of a line that is not the text's first: the line break's name, as an ordinary Backspace there says it, NVDA's move having reached the break (`editableText.py` 302 to 309). Where neither that read nor the focus's caret knows the line break, nothing is said for the key.
- Backspace or Control+Backspace at the text's start: "blank", "Edit message edit", as NVDA's move returns 0, its deleted text is empty (307 to 311), its wait counts the focus as a move, and it spells the empty text (320), which the capture of "blank" shows.
- A later key: nothing is said, as NVDA's helper returns while a script waits (`editableText.py` 166).
- Alt+Up or Alt+Down: the line, as NVDA passes the line unit when the caret "moved" (`editableText.py` 255 to 262).

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
- A read in flight at the pass that ends after it counts as the key's by when it ended, so a change made by something else just before the pass and caught by that read is taken as the key's. This closes the limit of a before read slower than 150 ms that samples after the key's effect.
- A note that passes its end during a burst of work in Core, while the key is still within its bound, makes a later caret event's read ordinary, and that read consumes the effect as above.
- A key's evidence in flight when a focus from another process reaches Core is spoken as a key that moved nothing, the before read's unit.
- A focus executing while a key is still being read leaves that key silent, since its words would follow the focus's.
- A duplicate focus report does not stand for a focus that moves first: NVDA would speak the unit at once, and Verbatim speaks it at the bound.
- A key within a few milliseconds of an earlier key can be read before the earlier key's effect, as today.
- A key in an elevated window, until M8.6, is judged from the last read; one whose own event was read and reported to Core before the key reached Core finds no evidence and speak at its bound from the caret after the key.
- A rich edit control's sentences stop at a wrapped line's end until M6.5.
- A letter typed within 100 ms of a key that moves nothing leaves the key silent, as NVDA's speech for it is cut as it starts (`nvda/source/inputCore.py` 584 to 605).
- NVDA speaks as soon as a control raises a caret event, moved or not, about 8 ms after the key in the text box (`editableText.py` 104 to 120). Verbatim judges by position, so the same words come at 100 ms.
- Control+Backspace with a focus that moves first, from within a word, says the part of the word before the caret, where NVDA says the whole word, because `deleted_text` takes what the key would remove.
- A Control+Backspace that inserts a character says "blank" wherever it is. NVDA says "blank" only at the text's start, and elsewhere the word before the caret, because it reads that before the key (`editableText.py` 300 to 320).
- A note outlives a key whose focus moved, for at most 300 ms, costing one read per event of that node meanwhile.
- A change made by something else within the key's 100 ms counts as the key's, as today.
- A caret event's read dispatched before its key's note arrived is compared at its commit by the reducer, which needs no call: it counts as the key's evidence, a character or line key speaks at that report from the line every read carries, and a word key, whose unit it did not fetch, takes it as evidence and speaks at its bound from the final read's unit.
- Slow before reads, repeated focuses and reads after an effect compare the caret by its line's text and offset, adding no call, so a move between two lines of the same text at the same offset is not seen there: a Down Arrow between identical lines caught only by one of those reads speaks at its bound, 100 ms later than today. Every other read compares positions.

## Removed and kept

Removed from the branch as built (`worktree-agent-a24de81e769020b0e`): `watch.rs` as a state machine, except `Compared` and `had_effect`, which move to `verbatim-model`; `Watch`, `Stage`, `AfterCheck`, `Reading`, `FinalDue`, `Take`, `WatchChange`, `Found`, `Baseline`, `baseline_keys`, `Reducer.replaced`, `Current.key`, `Line`, `key_arrived`, the watch's parts of `lib.rs`, `follow.rs` and `worker.rs`; `KeyTag`, `KeyRequest`, `KeyStart`, `KeyStarted`, `WatchOrder`, `CoreToOutpost::Watch`, `KeyAnswered`, `KeyAnswer`, `KeyRead`; the outpost generated test's call-off checks and seam.

Survives from it: `KeyLine`, `Busy`, `waiting`, `heard`, `Before`, the deadline handling and the pass rule in `key_line.rs`, less the tag and orders; `update_caret`; `before_of_read`, `evidence_words`, `bound_words` and the focus-first rule, reading the report's facts; `HeldKeys` and the hook; `LastRead` and every commit through `TextChange`; the remote program's reading and comparison, with the last read in the baseline's place; the Core generated model's world and per-key records; and the terminal's and console's reads.

Removed against main, in Core: `caret_history`, `CARET_HISTORY`, `TimedCaret`, `caret_before`, `caret_moved_by_core`'s time-zero record, `forget_caret`, `PendingCaret`, `HeldKeys::{passed_at_ms, awaiting_watch, bounded}`, `Then::{HeldRead, Caret, SupersededCaret}`, `superseded_reply`, `CaretContext::node`, `compared_text`, the scans `reading_held` and `find(Then::Caret)`, the effects scan in `pass_held`, and time zero as unknown.

Removed against main, in the outpost: `CaretSeen`, `REMEMBERED_CARETS` and the search by press time, `KeptSelection::report`, `NodeAnchors::reports`, `LastSelection`, `ForgetSelection`, `CaretFor::Focus`'s restart, `Baseline::{selected, known_line}`, `compared_already`, `Signal::covered`, `CaretWatch`, the separate `AwaitCaret` request and its first check, and `MoveCaret`'s clearing of the history.

Kept: holding, deferral, the 100 ms wait for the key ahead (`KEY_AHEAD_BOUND_MS`), now also the key's bound, the 150 ms before read (`HELD_READ_BOUND_MS`), the recheck of non-atomic reads, general terms with the note that the text changed and the WinEvent sync at the caret event, event coverage (`caret_read_since`), the read on every caret event, `heard` and `later_key`, and `deleted_text`.

When this is built, the rule in `books/dev/src/explanation/design/ownership.md` that lists what an outpost decides reads: "which calls to make: the order of its two routes, verdicts, node identity and, while a key's note lives, what its reads fetch and that text changes are read". D14's section in `books/dev/src/explanation/architecture.md` already says everything decided after the read is decided in the reducer, so it needs only: "A caret key's effect is such a decision: the outpost compares and reports, and Core judges." Neither is edited by this document.

## Costs

Against the baseline measured at 6e6c1efa (the session's `measure-before/results.md`). Figures not quoted from it are estimates that the prototype measures, and the prototype passes only if none is worse beyond the 10 to 25 percent run-to-run spread.

| | At 6e6c1efa | With the note |
|---|---|---|
| A held key that moves, through UIA | Three round trips | Two: the before read and the read on the event |
| The same, classically | Reads of 8, 15 and 15 calls | Reads of 8 and 15 calls |
| The same, in a classic edit control | 5, 5 and 6 messages | 5 and 6 messages |
| A key that moves nothing | No final read | One read and report at the bound, as Dickson's decision of 2026-10-29 needs |
| Text-change reads per key | While the watch is open | While the note is live and has seen no effect, ended by the first effect, the final read or the next key |
| A key's second caret event, after its effect | A read with no unit, the watch having answered | A read fetching the unit: nothing more through UIA, possibly window messages in a classic edit control. The prototype counts it, and any extra call goes to Dickson |
| A text-change event during the before read | No read | A read under the note. The prototype counts it, and any extra call goes to Dickson |
| A focus that moves, or the next key | A message | None |
| A held key passed on in Windows Terminal | One terminal read after the key | One `ReadTerminal`, one terminal read, as today; a late or refused one leaves it to the next event (Dickson, 2026-11-09) |
| Wake-ups | 21,000 to 25,500 woken in Core, 28,000 to 35,700 in the outpost, over 45 s of the text box workload | Not above that; one request fewer per held key |
| Key to first sound, caret keys | P50 9, P95 29 in the text box; 14, 57 in Notepad; 5, 50 in Windows Terminal | Unchanged for a key that moves; a key that moves nothing speaks at 100 ms |
| Remote operations | Caret program under 5,000 instructions | One comparison, under 5,000 |

The deadline is one of the reducer thread's, and a note ends by being read, so no wake-up source is added.

## Order of work

1. A prototype on a branch of the built code: the note, the report's facts, Core judging evidence. It measures reads per key on the text-change path, the fetch on a key's second caret event, any read queued under the note during the before read, calls per key, wake-ups, processor time and key to first sound, with the commands in the baseline. Any added call stops here for Dickson.
2. The generated tests, written by an agent other than the builder, from `docs/parity/text.md`, `docs/nvda/editable-text-and-terminals.md` and Dickson's dated decisions, not from this design's states. Core's key-line model keeps its per-key records, with its seam changed to the new messages, and the outpost's model is joined to it. They are one test driving Core and the outpost's reducer together. It is committed failing against the built branch where it differs, and run against the prototype, every seed passing.
3. The key lifecycle build: `KeyLine` without tags, the note, the reads, the bound answer, the focus rule, the slow-read path and the removals. `caret_keys` and `settings_search_suggestions` expectations for unmoved keys change here.
4. The keys as NVDA build: the editing chords from the hook to Core, and the sentence move.
5. The measurements and the listed scenarios, with `text_box_selection_keys` at Control+Delete over "alpha " ten times.

## Tests

Every test follows `books/dev/src/reference/testing.md`. The bound is an input to Core, so no test of it waits on real time.

Generated tests, for ordering and lifecycle only.

One test drives Core and the outpost's reducer together, since the baseline's being consumed by one read and heard by the next can only be seen across both. Its reference model is a record per key press, written from `docs/parity/text.md`, `docs/nvda/editable-text-and-terminals.md` and Dickson's dated decisions, not from this design's states or the note. Its world is an application moving the caret and selection, where the application's events and reads come at any step, early, late, twice or never, with distinct traces, several keys, slow answers, focus reports with keys queued behind them, repeated focuses, requests, releases, replaced elements, programs finishing or abandoned, the outpost ending, exit and the clock.

After each step it checks: every key reaches the application once, in press order and within its bounds; nothing is withheld after exit; a key with an effect within 100 ms of its pass speaks the key's words; a key with none speaks as the decisions say at its bound; a key speaks at most once, and not at all once a later key has become current, except for its selection changes; each selection change is heard once and after the words of the key that made it; the caret is the one read most recently begun among those delivered; there is one last read per node, the newest committed, gone at the node's release. It replaces the skipped comparison in `crates/verbatim-core/src/queries/generated.rs` and replaces `crates/verbatim-outpost-logic/src/tests/generated.rs`.

Hand-written tests cover words and single rules only:

- Core: the deadline input with no evidence asks the final read, and evidence first clears the deadline so that none is asked, with no real time.
- Core: the classification of a report as evidence, by when its read ended, its node and its comparison.
- Core: `removal`, segments, the speech per key class at evidence and at the bound, a caret that moved forward with nothing removed saying "blank", the focus-moves-first rule per case above, the chords, and sentence landing.
- The outpost: the note's end by the clock, selection-change text, general terms, cut reads, and the remote and classic reads' agreement.
- mockapp: Control+A followed by a repeated focus says the count once; a mouse selection before a before read is said first; the call counts and instruction limit.
- The hook: a chord is reported with its gesture.

Scenarios: `notepad_caret_keys`, `text_box_caret_keys`, `text_box_paragraph_keys`, `text_box_editing`, `notepad_editing`, `text_box_selection_keys`, `settings_search_suggestions`, `wpf_password` and `terminal_key_timing`.

Measurements, as the baseline took them: call counts; instruction counts; the stale caret workloads, with a wrong word counted as misjudged and a round that speaks nothing counted separately, 0 and at most the baseline's 4 of 30 Notepad rounds; wake-ups and processor time over `measure_keys_text_box`, `measure_keys_windows_terminal` and `measure_keys_notepad`; key to first sound P50 and P95.

## Why this design

- One lifecycle held twice produced all four rounds of defects, and each round added state to represent one crossing of the two.
- A single end message for every stage keeps both lifecycles, so the next crossing would surface in the same way.
- Giving the outpost the whole key from the before read's answer keeps a lifecycle in each process, needs a second timer for the keys behind, and overturns the bound being Core's.
- Core sending its selection with every request brings back two baselines and gives caret events nothing to carry it.
- Core comparing raw ends cannot order positions across chunks or read the text between them.
- A second path that trusts events where they are reliable brings back the race between the hook and the application's events, which is one of ordering.
