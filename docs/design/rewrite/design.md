# Verbatim's skeleton, designed for a rewrite

Status: design draft, Claude, 2026-11-28, second revision. Nothing in the repository changes because of it. It applies Dickson's decisions of 2026-11-28 exactly, and builds on the first slice design (`docs/design/rewrite/design.md`, called "the slice design" below), the options study (`docs/design/rewrite/options.md`), the pattern audit (`docs/design/rewrite/patterns.md`) and the headless NVDA prototype (`experiments/nvda-oracle/uia/`). This revision folds in two inventories of today's features against the design (`docs/design/rewrite/features-a.md` and `features-b.md`, called "features A" and "features B") and five inventories of NVDA's source by how it replays (`docs/design/rewrite/nvda-inventory/core.md`, `handlers.md`, `objects.md`, `speech.md` and `text.md`). It then takes in the adversarial review (`docs/design/rewrite/review-fable.md`), cited as "[review M1]" and so on by its finding numbers. Each change names its source in brackets. Every figure called an estimate is one. Every point marked "Decision for Dickson" waits for him; everything else is Claude's, marked "(Claude, 2026-11-28)" where it is a choice.

## Assessment first

- The skeleton is small and it closes the bug classes by construction. There are three kinds of message between processes, plus the held view each of Core's messages carries for release. Each process has one root reducer and one record per owner. Threads hold no state. Core's queue entries stay async functions, and the compiler rejects a Core fact held across a wait. The audit's pattern classes are copies, flag lifecycles, owner state outside its owner's record, per-feature messages and test literals. Each has a structural rule or a check that fails the build (the section "How each pattern class is closed").
- Today's features mostly fit, but the first revision had three structural gaps, which both feature inventories found independently. Each is now designed here, and each needs Dickson, because each changes his queue rule or adds to the protocol:
  - Requests owned by a record rather than the front entry. Terminal output, baseline and follow reads ("told, not yet read", Dickson, 2026-11-16, and the follow-caret ruling of 2026-11-28), classic progress-bar reads ("so no key waits behind it", 2026-11-21), the tray and taskbar walks, and the tree dump all run outside the queue today, by ruling. The section "Requests a record owns".
  - An arrival or a settle ending the front's request: a later key withdrawing a caret key's watch, a pending focus, a held key's 150 ms release. The section "Ending the front's request".
  - Ids with no home: ids that must outlive one request but subscribe to nothing (the start marker, the navigator, say-all, review anchors, a waiting caret key's bound node, the readers' last focus's listener token). Each is now named by the record that keeps it, and so is in Core's held view ("Release by held views"), which Dickson restored on 2026-11-28 in place of scope release.
- The verification is the larger new investment, and the one that changes how work is judged. A recorded or generated world is played to NVDA, running headless, and to Verbatim, running under a seeded simulator. The two are compared step by step. The prototype matched two live captures. The NVDA inventories, though, found that today's harness silently changes NVDA's speech in at least twelve ways: stubs that accept every call drop NVDA's delayed calls and turn 50 global commands into no-ops, the stand-in synthesizer adds "(not supported)" to spelling and stalls the speech manager, the symbol dictionary for emoji is missing, and the default event limiter is off. The harness rules in "NVDA headless" close these by construction: nothing stands in silently. Three oracle experiments since then ran NVDA's real code:
  - on a simulated clock with scheduled threads, where the caret watch's four outcomes came out the same on every run;
  - over UI Automation text and MSAA, where all 25 steps of a live Notepad review capture matched, and the Run dialog's capture matched apart from one description;
  - over a browser document, with NVDA's own native browse buffer and live-region code loaded into the test process.

  The experiments also showed that where thread order decides, NVDA has a set of possible outputs, while Verbatim, deterministic for a given event order, gives one; so a point with several NVDA outcomes needs a recorded choice of the one Verbatim must give (Dickson, 2026-11-28). They showed too that the native event limiter keeps the oldest duplicate, not the newest the source reading suggested.
- What the oracle can judge, by the inventories' replay classes: of NVDA's roughly 188,000 Python lines, about 49,000 replay as they are, about 38,000 need a named stand-in, about 9,000 need the simulated clock or thread order, about 2,600 need Windows to react to NVDA's own actions, about 27,000 are native or in-process code, and about 70,000 are not needed for speech (estimates; the inventories overlap a little). Native code is not out of reach by nature: the browsers and native experiments loaded NVDA's shipped in-process helper and hooks into the test process, so browse buffers, live regions, list views, the display model, the IME, and classic Word's and Excel's helpers over stand-in object models run in the oracle. None of them is verified yet: each must close its reads of the live machine, read the recorded world, and match a live capture before it counts [review 2 M4]. Word's remote-operation extensions ran in NVDA's local executor; through the real UI Automation core only stand-in extension calls ran, and Word's sentence movement on that path still needs a text provider [review 2 M3]. Web editors are still to be tried. Other applications' object models stay outside, verified by captures and the desktop suite.
- A measurement gate now comes before any investment beyond the focus slice: historical Verbatim bugs and the harness defects above are re-introduced, and the verifier's catch rate decides whether the verification grows (the section "The measurement gate").
- Size, estimated: the skeleton with the focus slice is about 12,550 product lines, against today's 224,149 product lines in all and the slice's roughly 15,000 today (the slice design's reading). NVDA's functions for the same behaviour are 1,030 lines. The verification tooling is about 20,100 lines, against today's 28,866 in `verbatim-e2e`, 11,633 in `verbatim-agent` and 49,191 in `mockapp` with its tests. Those tools stay, and the new ones replace none of them at first.
- What is weakest:
  - World capture samples an application's state after each event, so a value that changes for less time than one read lasts is lost. Generated worlds cover those races, judged by NVDA's own code, not by Verbatim's design.
  - NVDA's polls after a key (the caret watch) observe the application's reaction to a key NVDA let through. A world can only replay the reaction it recorded, so a key that NVDA blocks but the recording delivered makes the rest of that world diverge.
  - Remote operations: no real API is used anywhere in the replay (Dickson, 2026-11-28). The simulator runs Verbatim's remote programs by the ordinary-call path, each program's reads served at one simulated instant; live API tests prove that path against the real UI Automation core and real providers, and a program kind with no case there fails the build ("Remote operations in the replay", "Three kinds of test"). For NVDA they matter only in Word (sentence movement, a heading's collapsed state, page numbers) and in browse mode's heading search, and Word's extensions have no local form in NVDA today.
  - Applications' object models (M13) are outside what the recorder can capture. Loading NVDA's in-process helper works only on x64, and it puts GPL binaries in the harness's own process.
- Decisions for Dickson. Left open: the outcome-rule form, which Dickson will revisit during implementation with worked examples. A real-use loop is offered as an option. All are listed at the end.

## Terms

- Peer: a process Core asks. The peers are the supervisor, the listener, one outpost per served application, and, from M5 and M6, extension and synthesizer hosts.
- Request: one `Ask`, with a number Core mints. A one-shot request ends at its `Final`. A standing request (`Watch`, `Serve`, or a `Follow` opened inside a batch) gets any number of items, then one `Final`, or it ends by `Cancel` or by its peer's end.
- Held view: the ids of one reader that Core's records name, computed when Core sends that reader a message, with the position of the reader's last message Core had stepped. The reader releases what it sent by then and the view leaves out.
- Position: a reader's count of its own messages to Core, kept on both ends of the pipe.
- Root reducer: a process's one owner of state. It takes inputs, changes its records and returns effects. It makes no I/O call.
- Worker: a thread that takes a job, makes the job's calls, and posts a completion stamped with its worker generation. It keeps no state between jobs, except the COM objects it acquired, in its arena.
- Arena: a worker's table from id to the COM object it got. The root holds only the id.
- Worker generation: the number of one worker thread's life. Abandoning a worker starts a new generation, and a completion from an old one is dropped by its stamp.
- Stage: a named point where a Core entry waits, such as `focus.walk`. It shows in dumps and decision logs.
- Fact type: a type in Core's records, owned by Core, and `!Send`.
- Entry type: data owned by one queue entry, such as a parsed tell. It is `Send` and not `Clone`.
- World: a world capture or a generated model of what Windows and the applications answered and raised over time. It is served to both screen readers.
- Trace: Verbatim's always-on record, kept by every process's reducer: its inputs and the answers to Verbatim's own calls, in a ring of the recent past, with no extra calls. A trace replays Verbatim's processes, not NVDA. The name is Claude's, pending Dickson's preference.
- World capture: the same recorder at its second level, switched on when wanted. Verbatim's readers also read everything a world needs at each event, so a world capture replays onto both Verbatim and NVDA. "The recorder" below means it. The name is Claude's, pending Dickson's preference.
- World server: the one implementation that answers reads against a world at a simulated time.
- Decision log: the named decisions a run made, each with its subject, its outcome and a fingerprint.
- Fingerprint: a hash of a decision's name, its subject's identity (runtime id and window, or the MSAA address) and its outcome. Two runs' decisions are matched by fingerprint, not by order of lines.
- Owner: what a request answers into. Either the entry that asked, or a record (such as a terminal's reading state or the progress record).
- Ticket: the owner's half of a request in Core. The `Out` record holds the other half. The answer, Core's cancel and the peer's end each resolve the ticket, exactly once; dropping the ticket cancels the request.
- End cause: a named reason, raised by an arrival or a record's transition, that can end the front entry's request, such as `LaterKey`, `PendingFocus` or `KeyReleased`.
- Front view: a read-only, `Send` summary of the front entry for other code: its kind, its stage and its stamped facts.
- Replay class: how a piece of NVDA's code runs over a world. R1 runs as it is; R2 needs a named stand-in; R3 needs the simulated clock or a thread-order seam; R4 needs Windows to react to NVDA's own actions; R5 is native or in-process code; R6 is not needed for speech. The NVDA inventories define these.
- Strict stand-in: a stand-in module or object that raises on any attribute or call it does not implement, as against a stub that accepts everything.

## The protocol

### Messages

All in the platform-neutral model crate. Request numbers are minted by Core only.

```rust
pub struct RequestNumber(NonZeroU64);
pub struct ItemSeq(NonZeroU32);           // an item's number within its standing request

pub enum ToPeer { Ask(Ask), Cancel(Cancel) }
pub struct Ask {
    number: RequestNumber,
    due: Due,                              // Core's own bound; the peer never times out a request
    held: HeldView,                        // computed from Core's records when this message is sent
    ask: Asked,
}
pub enum Asked { Batch(Batch), Watch(WatchFilters), Serve(Role) }
pub struct Cancel { cancelled: Vec<(RequestNumber, CancelReason)>, held: HeldView }
pub struct HeldView {
    handled: Handled,                      // how far Core had stepped the reader's messages when it computed the view
    ids: Vec<Id>,                          // every id of this peer generation that Core's records name
}
pub enum Handled { NoneYet, Through(Position) }   // NoneYet: a generation's first messages, before Core has stepped any of its own
pub struct Position(NonZeroU64);           // a reader's count of its own messages to Core, kept on both ends

pub struct Answer { number: RequestNumber, part: Part }   // the one message a peer sends
pub enum Part { Item { seq: ItemSeq, item: Item }, Final(Outcome) }
pub enum Outcome { Result(Results), Refused(Refusal), NotThere, NotResponding, Ended(EndReason) }
```

- The five outcomes are ADR 0003's.
- Release is by held views, as ADR 0006 ("Nodes are released by message position") and ADR 0003 (a reader's kept elements) give it. The section "Release by held views" has the rules (Dickson, 2026-11-28).
- `ItemSeq` numbers the items of any standing request, so an outpost's caret tell sent as a `Follow` item is told like a listener's [features A, addition D; features B, addition 4].
- Pipes are first in, first out, with no merge, no priority and no drop. Today's transport merges by key, puts urgent messages first and drops the oldest past 64 (`docs/design/communication.md`, section 4.4). Those rules go, because each one is a decision made outside Core. A flood is bounded instead by the listener's coalescing under Core's filters, and by credit (the section on M6.1). Both are in the slice, not deferred to M6.1, since without them nothing bounds a flood between the two [review S1]:
  - Coalescing by key works with no credit: a waiting item is replaced by a newer one with its key.
  - Credit is granted on every message Core sends, so a front stuck on a hung call does not starve the listener of credit.
  - Foreground and focus tells are exempt from credit, so the tell that lets Core abandon a hung call is never held behind it.
  - Liveness is judged by any message from the peer, not only by `Pong`, so a reader whose pong waits behind a backlog on its first-in, first-out pipe is not ended as dead; today's urgent pongs (`docs/design/communication.md`) are then not needed.

### What each ask means

- `Watch(filters)`, to the listener. It means: tell me, as items, the events these filters admit.
  - The filters hold the event kinds, the process ids to ignore, and the drops on an event's own cached properties that ADR 0003 allows. With extensions they also hold the rule tables' listener part.
  - The watch's request number is the generation every judgement under it names, so the rule-table "generation" of the extensions design becomes this number.
  - A filter change is a new `Watch` that names the one it replaces. The listener ends the old one with `Final(Ended(Replaced))` as soon as the new one arrives, before any item of the new, whether or not an event comes [review S7]. Each event is told under exactly one watch, and each watch has one ending. The old watch's tokens outlive its `Final`: the listener keeps each while Core's held view names it, so waiting entries keep their tokens. `Listener::Ready` holds one open watch; during the replacement step it never holds two, because the old one's `Final` is sent in the same listener step that opens the new. Approved by Dickson (2026-11-28): the `replaces` field.
- `Serve(role)`, to the supervisor. The role is the listener, an outpost for one process, an extension host or a synthesizer host.
  - Its items are `Started { pid }`, `Ready` (relayed from the child's pipe) and `Pong`.
  - Its final outcome is `Ended(reason)`, or `Exited` when the outpost's application has gone.
  - The served process is the request: its life is the request's life.
- `Batch(steps)`, to any reader, from the shared step list (today's `Op` in `crates/verbatim-model/src/batch.rs`, split as the slice design says: `Read`, `Ancestors`, `Find`, `Follow`, `Test`, and the rest).
  - `Follow { number, subject }` opens a standing request inside the batch. Its items are tells, in the same shape as the listener's: a kind, the node, the time observed, the values cached with it if any, and a token where the item carries one. A property change is a tell with its value; a terminal's change signal is a tell with none, since a signal has no value [features B, terminals]. Approved by Dickson (2026-11-28): follow items in the tell shape, replacing `Changed { node, property, value }`.
  - `Destroyed { window }` answers "not there" for every queued read naming that window's MSAA ids, and forgets the outpost's nodes in it, before any later request on the same pipe. Without it an MSAA address on a reused window handle would read another object. The `Application` record's destroy transition sends it, as a request that record owns, together with the `Cancel` of a follow on that window; today's `Follow` naming nothing goes [features B, window destruction; features A, item 1]. Approved by Dickson (2026-11-28).
    - It narrows the reused-handle race; it does not close it [review S6]. The outpost learns of a destruction two hops late, through the listener and Core, and a queued read naming an MSAA address can run against a new window with the same handle in between. That is today's limit as well. The outpost's own check is the closing step: an MSAA read compares the window's creation identity (its process, thread and class, recorded when the node was minted) with the window it reaches, and answers "not there" on a mismatch (Claude, 2026-11-28).
    - Destroys lost while the listener restarts: every window fact Core keeps in an `Application` (its place in the hung set, its probe verdicts, a follow on it) carries the watch number it was told under, and the listener's end drops those facts. They are learned again under the new watch, and the re-take after the listener's end already re-takes the focus and its follow. So no entry outlives a lost destroy, and a handle reused by another process cannot sit in two `Application` records.
  - `Barrier` answers only once the reader has sent every item it held when the barrier arrived, coalesced items included. It serves the harness's `AwaitIdle`, which today is a separate idle query (`verbatim-app/src/idle.rs`) [features B, control plane]. Approved by Dickson (2026-11-28).
  - Adding a step is Dickson's item 4. A feature adds none when an existing step fits.

### Items

- On a `Watch`: `Event(Tell)`. The tell is one shape for every event (ADR 0003, "Readers tell, Core asks"): its kind, process, window, time, a token, the values cached with it, the window facts and the delivery checks. Also `Process { pid, identity }`, sent before the first item naming that process (the extensions design, section 14, "Application identity has one resolver").
- On a `Serve`: the items above.
- On a `Follow`: tells, as above.
- On a host's call (M5, below): `Effect` (speech, tones) and `Wants(Batch)`.

### Parsing at the boundary

Each process turns a message into a sealed record once, in its intake.

- A message that does not parse is refused and logged, never defaulted.
- An answer is matched to its request by the peer it came from first, then by number. An answer for a number its peer's record does not hold is late: it is logged, its content is dropped, and Core answers it once with `Cancel(Late)`, so the reader ends any standing request under that number [review M1].
- An answer that does not fit the request's batch fails the parse as undecodable. An example is a meeting point that names no ancestor Core sent.

### Cost per focus (estimate, by counting messages)

- Three crossings into Core (the tell, the listener's answer if it reads, the outpost's answer) and one `Cancel` to the outpost for the old follow, as in the slice design.
- The held views ride on those messages and add none: no message is sent only to carry a view, as today. A reader keeps what it sent until Core's next message to it.
- The wakes are measured in build step 9 against today's, as CLAUDE.md asks.

## Release by held views

Dickson ruled on 2026-11-28 that release returns to held views computed from Core's records, reversing the scope release this design proposed earlier the same day. ADR 0006's "Nodes are released by message position" stands and needs no amendment.

Rules:

- Every message Core sends a reader carries a held view: every id of that reader's current generation that Core's records name, and `handled`, the position of the reader's last message Core had stepped when it computed the view. The view is computed when the message is sent, from the records; no list of held ids is stored.
- On a message, the reader releases every id it sent to Core at or before `handled` that the view leaves out; with `Handled::NoneYet` it releases nothing [review 2 S1]. It keeps every id sent after `handled`, since Core may not have read it yet. A release is a `Release` job to the worker whose arena holds the object, so every COM release runs on a thread allowed to block.
- One rule for every id: it is kept until a held view leaves it out (Dickson, 2026-11-28). A one-off read's ids, such as Report focus's, go at the next message to that reader, since no record names them.
- Ids a request minted but never sent, because it was withdrawn or abandoned, are released when the request ends, since no view can name them.
- A peer's end releases everything in the reader (the process is gone), and Core's records stop naming that generation's ids: every id carries its peer generation, so a view is computed for the current generation only, and an ask naming an ended generation's id is answered "not there" by Core without sending.

Which records name ids, for every holder today's `SrState::held_nodes`, `held_anchors`, `held_told` and `held_tokens` cover (`crates/verbatim-core/src/state.rs` 957 to 1060) [features A, assessment]:

| Holder | Named by |
|---|---|
| The focus and its line | `Versions`' committed focus |
| The readers' last focus's listener token, read by the duplicate test after its entry ends | `Versions`' last focus |
| The navigator's node and review anchors, off the focus | the navigator record |
| The start marker's anchors | the start-marker record |
| Say-all's node and positions | the say-all record |
| A waiting caret key's node bound at its press | the key's entry's ids record |
| An outpost caret tell's element, and a listener tell's token | the ids record of the entry built from the tell |
| A read's answer between its arrival and its commit, and any id an ask returns | the running entry's ids record [review 2 M1] |
| A record-owned request's named ids | the owner record |
| A plain node (a ghost or the desktop in front) | none at a reader: `Foreground` and Core's plain answerer |

How the view stays complete (Claude, 2026-11-28): every id of a reader in a Core record is held as a `Named<Id>` field, and every record type that holds one implements `NamesIds`, which the view's computation calls. A check beside the ledger fails the build when a ledger type holds a `Named<Id>` and does not implement `NamesIds`, so a new record cannot forget to name its ids. This is the computation today's `held_*` functions do by hand, made total by the check.

A running entry's ids [review 2 M1]. A running entry's future is stored beside `Core`, so ids carried in it would be in no record, and a view sent while it waits would release them. So:
- Each queue entry, waiting or running, has an ids record in `Queue`: an ordinary sealed record, a ledger type implementing `NamesIds`, not part of the future. The ids a tell carries go there when the entry is built.
- `cx.ask` registers every id its answer returns in the running entry's ids record, and hands the future only `IdRef`s, places in that record. `Named<Id>` carries the `NotSend` marker, so the compiler refuses an id held in a future.
- The record holds each id until the entry ends or lets that id go explicitly (`cx.let_go(idref)`).
- A stage that stores an id in another record moves it there through that record's transition, inside `cx.apply`, for example the commit moving the line into `Versions`. The move is one transition, so the id is never in neither record.

The Tab sequence, with the view at each message:
1. Tab moves the focus. The listener tells focus with token K, at listener position p. The focus entry is built; its ids record holds K.
2. The entry reaches the front and asks the outpost for the walk. Meanwhile Core sends the listener a message (credit, say) with `handled` past p. Its view names K, from the entry's ids record, so the listener keeps K.
3. The walk's answer arrives at outpost position q with the line's ids L. `cx.ask` puts L in the entry's ids record.
4. Before the commit, the entry waits again, for the listener's duplicate test. Core sends the outpost the old follow's `Cancel`, with `handled` past q. Its view names the old line, from `Versions`, and L, from the entry's ids record, so both are kept.
5. The commit moves L and K into `Versions`, in one transition, and the old line leaves `Versions`. The next view to the outpost names L and not the old line's own nodes, so those are released; shared ancestors are in L and stay.
6. The entry ends and its ids record is dropped; everything it held is now named by `Versions`.

M5's loop has the same shape: the ids a `Wants` read returns sit in the entry's ids record until the lane's record takes them by its transition.

The say-all rule `say_all_stops_when_its_outpost_generation_ends` holds because its next read names an ended generation's id and is answered "not there".

Limits:

- A reader keeps the ids it sent until Core's next message to it, as today.
- Ids lost with an abandoned worker: since such elements are lost (Dickson, 2026-11-28, item 3), the outpost marks every element whose arena was that worker `Lost`, answers "not there" for any ask naming one, and sends no `Release` job to the stuck worker; each `Lost` element's record is dropped when a view leaves it out. Core's records may name lost ids until then, which is the one exception to the invariants on named ids [review 2 S3].
- An id Core's records stop naming too early costs a re-read, answered "not there", and the simulator's invariant "no `NotThere` for an id Core's records name" finds it.

## Records, per process

Each process has one root, and one record per owner. An owner's end drops its record and answers its requests. There are no side maps and no clean-up functions. Records are sealed: their fields are private in small modules, they change only through named transitions, they are read through accessors, and each has one `fixture` constructor under `cfg(any(test, feature = "fixtures"))`. A later stage holds the earlier one and never copies it.

### Core

Core's state is one root, `Core`, with these records.

- `Peers`:
  - `supervisor: SupervisorLink`.
  - `listener: Listener`. Its life is one enum: `Asked { serve } | Starting { serve } | Ready { serve, watch, out } | WaitingOut { until }`.
  - `apps: BTreeMap<Pid, Application>`. An application's serving state is `Asked | Starting | Ready(Generation) | WaitingOut`, each variant carrying its own waiting requests.
  - `hosts: BTreeMap<HostId, Host>`, from M5 and M6, with the same life as an application.
  - `plain: PlainAnswerer`, a peer with no pipe. It answers a request on a plain node (a ghost window, or the desktop window, in front) in the same step, from the plain object's facts, which `Foreground` owns (ADR 0003, "A frozen window"; `focus/plain.rs`; Dickson's ruling of 2026-11-19 in the coverage row). `cx.ask` treats it like any peer, so an entry cannot tell the difference [features A, addition G; features B, "A ghost in front"] (Claude, 2026-11-28).
- `Generation` holds `serve`, `out: Out` (the requests out to this generation, one-shot and standing, each with the `Out` half of its ticket and its owner's address) and its liveness times. The peer's end consumes the record and hands back `out`, each request then answered `Ended` to its owner. This is ADR 0006's "every request has exactly one answer", by construction.
- `Application` holds every fact Core keeps about one process:
  - The terminal models and the terminal focus, with its reading state and that state's tickets.
  - The hung-window set, its half-second check's `Due`, and the not-responding sound's once-per-stall mark.
  - The backend cache entries of its windows, including the probe verdicts. The audit's leak, `BackendCache.probed` never pruned, becomes impossible, because the cache dies with its owner. A window's destruction clears its entries, its place in the recovery set and a follow on it, and sends `Destroyed` [features B, window destruction].
  - The extension instances' list-dialog records.
  - Its identity.

  From the audit's list of per-owner state cleared by hand (`patterns.md`, "Owner-record sweep"), these move into this record: `Follows.named`, `Terminals.models`, `TerminalFocus.focused` and the recovery table's entries. Recovery (today the `recovered` hook) is a transition of this record that pushes its entry [features B, recovery].

  The terminal models' cap is 16 across every application today. It stays one bound across applications, computed at insertion by a count over the `Application` records, evicting the least recently read model; the count is a computation, not a second record (Claude, 2026-11-28) [features B, addition 13].
- `Queue`: the waiting entries, the front entry, and the limiter's placement state, including today's `Place` (key, class, accepted, arrival time), the caps (`FOCUS_CAP` 4, `THREAD_CAP` 10, `PUMP_WINDOW_US`) and the menu slot [features A, MSAA focus].
- `Versions`: the committed focus and the readers' last focus, as the slice design gives them. The readers' last focus is a state of the committed focus, not a copy. Whether the two can differ outside Core's own focus is not established: today `LastFocus` is its own record with `names(&focus)` (`focus/versions.rs` 147 to 175), so the slice build checks that against its sequences before it merges them [review U4]. `Versions`' last focus names the listener token, so the held view keeps it. The committed focus also holds the edit control's selection offsets and the text-changed mark, today the global `SrState.edit_selection` [features B, selecting with Shift]. After an outpost's end, the committed focus becomes a state that keeps its copied data and names no ids, and the next focus's line join meets by runtime id [features B, outpost generations].
- `Foreground`: the newest foreground item, whole; the input desktop and the lock state; the plain object's facts.
- `Features`: one record per feature, each holding only that feature's state. The records that own requests hold their tickets, and the records that keep ids name them, in their own state values, as the section "Requests a record owns" gives: progress (today's `bar_reads`), the shell list, the navigator, the start marker, say-all, and the menus (today's `run_executed` and noted focus, as one state value). The others hold plain state: the toast's last text, cell memory, the last tree level, the desktop's name, the typed word [features A, records table; features B, state in records].
- `Keys`: Core's line of key presses, as ruled in ADR 0003, "Keys and commands": strokes, arrivals, Alt, the toggle keys, and each held key's 150 ms release as a `Due`.
- `Settings`: the settings and the profile stack. The settings a read needs travel in its request (roadmap, "Found for later milestones", M8.3). Speech values (rate, voice, the driver's own settings) are not in it: the speech root owns them, as the synth thread does today (`manager.rs` 27 to 32), so one fact has one owner [features B, addition 14] (Claude, 2026-11-28). The processes the harness ignores go to the listener's `WatchFilters`, which removes two of today's three copies [features B, control plane].

`SrState.wants`, today's list of requests outside the queue with their tokens, has no counterpart: each such request is a ticket in its owner's record [features B, state in records].

The shell's own records (the speech thread, the audio thread, the hook thread, the GUI) follow the same rules, each with its own root. They are outside Core's protocol, but inside the ledger and the sealing check. The speech manager keeps, for now, its `Mutex` cancel state, its `AtomicU64` utterance counter, its `Waiters` mutex and the theme's `RwLock` (`driver.rs` 297, `manager.rs` 81 and 661, `theme.rs` 234), listed in the ratchet so their count only falls; making it one root with a synthesizer worker is a separate build [features B, speech manager]. The hook's view of Core's held-key mode stays one lock-free load, a listed exception (`key-holding.md`, "The hook") [features A, keys].

### The listener

- Root `ListenerRoot`:
  - `serve`.
  - `watch: WatchState`, either `Closed` or `Open { number, filters, next_item }`.
  - `apps: BTreeMap<Pid, ListenedApp>`.
  - `registration: Registration`, the desktop-wide handlers and hooks, as a job state on the registering worker.
  - `foreground: PendingForeground`: every MSAA tell behind a foreground not yet in front is held until `GetForegroundWindow` names it or `FOREGROUND_HOLD_US` passes, and the consoles' thread watch (`crates/verbatim-listener-logic/src/foreground.rs`, ADR 0001). The `GetForegroundWindow` answer is a value the callback reads and stamps on the event, so the root makes no call [features A, addition F] (Claude, 2026-11-28).
- `ListenedApp`, one per process whose events the listener hears:
  - `target`, a process handle, held through a job on the waiter worker.
  - `identity`.
  - `check: WorkerSlot` and `reading: WorkerSlot`.
  - `tokens: BTreeMap<Token, Kept>`, each with the position it was sent at.
  - `probes`.
  - `hung_marks`: the per-thread mark that reports a hung window once per stall, so an application's exit drops its marks by construction [features B, not-responding answers].
  - `coalesce`, its waiting items keyed by Core's coalescing keys (the section on M6.1).
- The hang check on arrival stays in the callback: it reads `IsHungAppWindow` and stamps the answer on the `Transit` as a fact, and the root drops the event. Moving the check to the check worker would put it behind a job that may be stuck on the same hung application, so its timing and outcome would drift (`a_focus_whose_window_is_hung_as_it_arrives_is_dropped_in_the_callback`). The callback still makes no decision: it records a fact [features B, listener events dropped when hung] (Claude, 2026-11-28).
- The hung fact appears in four places, which the review counts as one fact judged four times [review S12]. They are four facts, each with one owner and one reader, and none is judged from another's copy:
  - the window's state as an event arrives, stamped by the callback and judged by the listener's root for that event only;
  - the listener's once-per-stall report mark, in `ListenedApp`;
  - the recovery set, Core's own fact, in `Application`, changed only by the shell's checks and the readers' reports;
  - the shell's live check before a send, which becomes an input to Core, never a fact the shell keeps.

  Two of them read `IsHungAppWindow` at different times, the callback and the shell. That is deliberate: each judges the moment it acts.
- `WorkerSlot`, one enum: `Idle(gen) | Busy { gen, job, since } | Abandoned { gen, since, replacement }`. The cap of two abandoned reading threads per application (`focus.md`) is a count over this record.
- The process's exit is a completion from the waiter worker. It drops `ListenedApp`. Its tokens' objects are released by `Release` jobs to their own workers. Core's requests for that process are answered `NotThere`. This replaces six collections and two clean-up functions (`patterns.md`).

### An outpost

- Root `OutpostRoot`:
  - `serve`.
  - `target`.
  - `elements: BTreeMap<NodeId, Element>`.
  - `requests: BTreeMap<RequestNumber, Request>`.
  - `workers: Workers`: the worker in charge, the registering worker, and from M7 a helper channel.
- `Element`:
  - `node` and `runtime_id`.
  - `backing: Backing`, which is `Worker { slot, arena_id } | Helper { helper_gen, id } | ObjectModel { source, id }`.
  - `window` and `route`.
  - `sent_at: Option<Position>`, the position of the message that carried it to Core, or none if not yet sent.
  - `text: Option<TextObjects>`: today's `TextFacts` (nodes, anchors, last reads, kept selections), which the caret and selection features need from the first text build, so it is not deferred [features B, state in records].

  This gathers the seven side maps the audit names (`crates/verbatim-outpost/src/outpost/store/mod.rs`, 178 to 198).
- `Request`: `Queued(Batch) | Running { batch, slot, gen, opened } | Withdrawn { slot, gen, opened } | Waiting { batch, next_check, bound } | Following { subject, registration, coalesce }`.
  - `opened` lists the standing requests a running batch has opened so far, each with its registration's state on the registering worker (asked, made, or removed). A batch's inner numbers are keyed to the batch in `requests`, so a `Cancel` naming an inner number before its step runs finds the batch [review M1].
  - `Withdrawn` is a batch cancelled while its job runs, for any reason but the foreground stop. Its completion applies nothing: it opens no standing request, sends a removal job to the registering worker for every registration in `opened` that was made, and sends no answer, since Core's ticket is already resolved [review M1]. Its ids were never sent, so the release rules release them.
  - An abandoned batch (the foreground stop) becomes `Withdrawn` with its worker `Abandoned`. The job may never complete, so the removal jobs for registrations already made in `opened` are sent at once, to the registering worker; the elements minted on the abandoned worker are lost with its arena, and no job is sent to that worker, as the item-3 question below says [review M1, the hung variant; review 2 S3].
  - A `Cancel` that crosses a `Final`: Core's `Cancel` of a batch always names the batch and every standing request opened inside it, so an outpost that already answered still ends the inner follow. Core answers any item or `Final` for a number it does not hold, once, with a `Cancel(Late)` naming that number, so a standing request Core no longer knows is ended by its reader rather than left open for the reader's life [review M1].
  - `Waiting` is the caret and terminal watches, today's waiting table (`crates/verbatim-outpost-logic/src/watch.rs`). The root drives one job per check with `WakeAt`: checks fall due about every 10 ms until `bound`, and a UI Automation event of the followed node within 60 ms brings the next check forward. A terminal watch uses the same state with its own bounds (5 ms to 100 ms in the console host, 10 ms to 300 ms in Windows Terminal). `Cancel` withdraws it between checks, since a running job cannot be interrupted [features A, addition E; features B, addition 8] (Claude, 2026-11-28).
  - `Following.coalesce` merges the follow's items for one node and kind, as the listener's `coalesce` does, replacing today's merge on the pipe and the intake's per-window merge of a console's events (`intake.rs` 1 to 15). The merge is Claude's (2026-11-28); the credit that bounds it is in the M6.1 section [features B, addition 7].
- Requests run in the order they arrive. Today the outpost runs requests from outside the queue behind the queue's (`waiting.rs` 1 to 8). The redesign keeps first in, first out with no class, because both kinds come from Core in the order Core decided; if a desktop run shows a tray walk delaying a key's read, a class on `Ask` is the fix, and that is item 4 [features A, tray list] (Claude, 2026-11-28).
- Abandonment is a transition. On `Cancel` with the foreground-stop reason, a `Running` request's worker becomes `Abandoned`, a new worker generation starts, and the request ends. Each `Pong` carries the count of abandoned workers, each counted once, and too many ends the generation (`each_parked_worker_is_counted_once_in_a_pong`).
- A `Cancel` with any other reason does not abandon: the request ends at once in Core, the worker runs on as `Withdrawn`, and its completion is discarded as above (`a_running_request_cancelled_otherwise_runs_on_and_answers`) [features B, abandonment; review M1]. These two states add one lifecycle state to the outpost (`Withdrawn`) and one field (`opened`); the simulator's quiet-world leak invariant covers both.

  Decided (Dickson, 2026-11-28, item 3): elements whose arena is the abandoned worker are lost. The rule "COM objects stay with the worker that got them" means they are lost with it.
  - Lost: Core's committed line then has unknown nodes. The next focus walks to the top, and the line join must meet by runtime id rather than by node, or it says the window's title again.
  - Kept, callable from the new worker: in the multithreaded apartment this is legal for COM, but it breaks the ownership rule.

  Lost was the recommendation and is the decision, with the join by runtime id at the windows above the hang. NVDA's speech after a hang recovers needs a capture first.

### The supervisor

- Root `Children: BTreeMap<RequestNumber, Child>`, one per `Serve`.
- `Child`:
  - `role`.
  - `stage`, which is `Launching | Running { process, writer }`.
  - For an outpost, `target`.
- Spares are not `Children`: they are started unasked, so they have no request number. The root holds `spares: Vec<Spare>`, kept at two from the moment Core's own outpost is ready, each with its pong times and backoff, and the oldest goes first (ADR 0001, "Spare outposts", Dickson, 2026-10-16). A `Serve(Outpost(pid))` arriving while one waits moves that spare into `Children` under the request's number, a mechanical choice that needs no judgement. The other option, Core asking `Serve(Spare)` twice, would move the spares' liveness and backoff into Core and is item 3; I recommend the supervisor keeping them (Claude, 2026-11-28) [features B, addition 11].
- Launches, waits and pipe I/O are jobs on workers. The supervisor launches a new outpost for a target only after the previous one for that target has been reaped.

### Hosts (from M5 and M6)

- An extension host's root holds a `Lane` per outpost served and per global module. Each lane holds `instance: InstanceSlot`, the instance being the Wasm store, held by the lane's worker as its arena.
- A synthesizer host's root is the pure command module already planned in M6 ("the host's command handling as one pure module").

## Core's queue and its async rules

The rules as Dickson set them, with how each is enforced:

1. Entries are async functions. The front entry alone may have an entry-owned request out. Records on a closed list, each citing the ruling that keeps its reads out of the queue, may own requests too (the section "Requests a record owns"). Dickson approved this change to his rule on 2026-11-28.
2. Each wait names its stage: `cx.ask(stage!("focus.walk", ends_on: [PendingFocus]), peer, batch).await`. The stage is a `&'static str` checked against a per-feature list at compile time by a macro, with the end causes that may withdraw this wait (the section "Ending the front's request"). Dumps show each entry's stage, the request out and the answers so far, and decision logs carry the stage.
3. Fact types are `!Send`, and entry futures must be `Send`, so holding a fact across a wait fails to compile.
   - Every fact type carries a `NotSend` marker, a zero-sized `PhantomData<*const ()>`. The queue stores `Pin<Box<dyn Future<Output = Ended> + Send>>`.
   - `Cx` is `Send` and holds only the entry's id. Core's state is reached only inside `cx.read(|core| ...)` or `cx.apply(|core| ...)`, closures whose result must be `Send` and whose borrow ends before the next wait.
   - The driver gives the closure `&Core` through a scoped thread-local set only while it polls. I propose the `scoped-tls` crate, read from its source first (Claude, 2026-11-28). It is a small dependency, not a major one.
   - Entry types (a parsed tell, a walked route, a focus read) are `Send` and not `Clone`. They are the entry's own data and may cross waits. They hold reader ids only as `IdRef`s into the entry's ids record, never the ids themselves [review 2 M1]. When one is committed it moves into a fact record and cannot be copied back out.
   - What the compiler cannot catch is a plain value, such as a `bool` or a `Pid`, read before a wait and used after it. Rule 4 catches that.
   - The rule's real reach, stated plainly [review S3]: the closure's borrow already stops a reference to Core crossing a wait. The `NotSend` marker adds one thing, stopping an owned clone of a whole record. Ids, booleans, strings and `Arc`s of entry types are `Send` and pass. The main guard against a derived copy is rule 4, which is dynamic: it catches a copy only on a world and seed that reach that wait with a change.
4. The simulator's world-change mode catches derived copies.
   - Under `cfg(feature = "sim")`, every accessor of a fact record logs that accessor's version into the running entry's read set. Versions are per accessor, not per record: each accessor has its own counter, bumped by the transitions that change what it returns. So a change to `Application`'s hung set does not flag an entry that read only its terminal model [review S3].
   - At each wait, the simulator changes the world in a way the application model allows. For example the focus moves, the element is destroyed, or the foreground changes. Core's records change as the answers say.
   - At each decision, the invariant is that every fact the decision read before a wait still has the version it had then, or the read was of the entry's own data or of what the request sent.
   - A violation names the accessor, the stage and the decision.
5. Every wait ends through its request's single ending. `cx.ask` resolves only from its ticket, which resolves exactly once, by one of four endings:
   - the peer's answer;
   - Core's `Cancel`: the due time, a foreground stop, or an end cause (the section "Ending the front's request"), each resolving the ticket at once in Core, the peer's later answer then dropped as late;
   - the peer's end;
   - the shell's refusal to send: the shell checks `IsHungAppWindow`, after mapping a ghost to the window it stands for, before it sends a request, and a refusal comes back as a `RefusedHung` input in the same step's effects, which resolves the ticket `NotResponding` and never reaches the peer (`verbatim-app/src/hang_check.rs`). Checking in the shell, not by a Core round before each send, costs no hop per request [features B, hang check; features A, addition G] (Claude, 2026-11-28).

   Dropping the ticket answers `Ended`. There is no other path, so no paused function outlives its entry.
6. Checkpoints are the entry's start plus the inputs since, as ruled. Replay rebuilds a paused entry by running it again from that state with the recorded answers. A dump's window always starts at the oldest unfinished entry's checkpoint, which bounds the roadmap's "Exact replay of a paused entry" problem to the size of the recorder's ring.
7. The queue has general hooks only. Each entry kind implements one trait:

   ```rust
   trait Entry: Send {
       fn arrive(&self, q: &QueueView) -> Arrival;           // Join(Place), Merge(EntryId), Drop(reason)
       fn foreground_changed(&self, f: &ForegroundView) -> Stay;  // Stay or End(reason): the judgement only
       fn ended_waiting(self, why: EndedWaiting, cx: &mut EndCx) -> Ended;  // the effect of ending a waiting entry
       async fn run(self, cx: Cx) -> Ended;
   }
   enum EndedWaiting { Foreground(reason), PeerEnded(PeerGeneration) }
   ```

   There are four hooks, plus the end causes each stage declares [review M2]:
   - `ended_waiting` runs when a waiting entry is ended without running. Its effects are limited by `EndCx` to the shell steps and the `Keys` transitions an ending may need. A held caret key ended at a foreground change passes its stroke, in press order, and tells the terminal, as today (`entries.rs` 690 to 698). The judgement (`foreground_changed`) and the effect stay separate, as the hook audit asked ("keep ended_at_foreground separate", `patterns.md`, "Queue hooks"). The default effect is none.
   - A waiting entry that names an ended peer generation's ids is ended. Every id carries its peer generation. When a peer's generation ends, the driver ends each waiting entry naming one of its ids, with `ended_waiting(PeerEnded)`, before any of them can reach the front; today `inbox.rs` 618 to 631 does this for the listener. Because ids carry their generation, a new listener can never resolve an old listener's token to another element, and Core answers an ask naming one "not there" without sending.
   - Approved by Dickson (2026-11-28): the fourth hook. The other option folded the effect into `foreground_changed`, which the audit argued against. Each feature's state is in its own record. The 26 methods of today's `Logic` trait (`crates/verbatim-core/src/event_queue/mod.rs`, 259 to 486) reduce to these, by the audit's pending proposal of 2026-11-28 ("Queue hooks", `patterns.md`). The readers' endings, re-takes, `started` and `recovered` become the peer and `Application` records' transitions that push entries; say-all's `joining` becomes the say-all record's transition at a speech-mark input; ticks become `Due` in the record that owns the timer; and `stamp` becomes the driver stamping each `Say` with its entry's fence, which is the entry's own data [features A, records table].
8. A fact may be stamped on an entry at arrival only when the rule it feeds is judged at arrival, in NVDA or by a ruling; every other fact is read live when it is judged [review M3]. The stamps, each with its source:
   - the key fence (the newest key number), ruled in ADR 0003, "Keys and commands": speech is cut against the keys pressed before the event;
   - Alt down at arrival, for the switcher's rules, which judge the event against the keyboard state when it came;
   - the background setting at arrival, since NVDA's `shouldAcceptEvent` takes it on UI Automation's thread as the event arrives (`eventHandler.py` 445 to 599);
   - whether the element is on the focus line as it arrives, for stale changes after a report of the focus (`changes-and-selection.md`).

   Not stamped, and judged live at placement, as today and as NVDA does: the cap's exemption, which NVDA computes from `lastQueuedFocusObject` at the pump (`IAccessibleHandler/__init__.py` 1025 to 1035; today `limiter.rs` 358 and `entries.rs` 964 to 972), and `place` (`limiter.rs` 126 to 178). A stamp is a value, never an id, so it names nothing the held view must keep. Stamps are not exempt from async rule 4: the world-change mode treats a stamp as a read at arrival, changes the stamped fact between arrival and the decision, and the comparison with NVDA, which judges at its own point, decides whether the stamp was allowed [features A, addition H; review M3] (Claude, 2026-11-28).

Core's thread runs `Core::step(input, now) -> Effects`. An input is an `Answer` tagged with its peer, a key, a shell input, or a due time. Shell inputs include the hang check's answers (`RefusedHung`, `InFront`, `HangCheck`) and the live reads the shell makes on a wake that Core asked for: the menu check's read of the window in front 50 ms after a menu ends, and a lock key's state 30 ms after its press [features A, addition G]. The effects are `Send(peer, ToPeer)`, `Say(utterance)`, `WakeAt(time)`, `Log(decision)` and the shell steps, including `CheckHung`. One shell-step function, shared by the product, mockapp's harness and the simulator, applies them through an `Io` trait; the simulator's `Io` answers the hang check from the world's `window.hung`.

### Requests a record owns

Rule 1 kept strictly would put these reads in the queue, which changes ruled behaviour [features A, addition A; features B, gap 1]:

| Owner record | Requests | Ruling |
|---|---|---|
| The focused terminal's reading state, in `Application` | output reads, baseline reads, and its `Follow`, re-sent once if stopped at its due time | "Core alone keeps 'told, not yet read'" (Dickson, 2026-11-16); "Core asks one read at once, outside its event queue" (`docs/parity/terminals.md`); the 4.1 ms typing echo rests on it |
| Progress, in `Features` | one read per classic bar, read again when marked | "so no key waits behind it" (`progress.rs` 14 to 30, Dickson, 2026-11-21) |
| The shell list, in `Features` | the walk, the position and the click | the walk is not stopped by a foreground change while the list dialog closes (`system.md`, "The system tray and taskbar list"; `entries_tests.rs` 1891) |
| The control plane's dump, a shell record | the tree dump | harness only |
| `Application` | `Destroyed` and the follow's `Cancel` at a window's destruction | today's `window_destroyed` |

How a record owns a request:

- The record's state value holds the ticket in the variant where a request is out, for example `Reading::Asked { ticket, since }`. The `Out` record in the peer's `Generation` holds the other half and the owner's address, a key naming the record, not a copy of any fact. The address is a lookup into another record, which the review counts as a reintroduced pattern [review S12]. It is kept because the alternative, the owner holding its `Out`, would put a peer's requests in many records and lose "the peer's end hands back `out`". Its rule: an owner's end cancels its tickets in the same step, so an answer never finds an address whose record is gone, and the simulator checks that.
- A record request stuck in a hung call: a foreground stop abandons the worker, not the request, so a record request running on an abandoned worker becomes `Withdrawn` at the outpost and its ticket is resolved `NotResponding`. Core sends the foreground stop, abandoning the worker, when only a record's request is stuck on a hung call, as for a terminal read on a hung console (Dickson, 2026-11-28) [review U5].
- The answer is delivered in `step`, not by `await`: Core calls the owner's transition, `answered(outcome, &FrontView)`, which changes the record and may push an entry, such as the terminal's speech entry or a bar's announcement.
- Its ids: kept while the owner record names them, as the held view computes.
- Its single ending: the answer, Core's cancel (the record dropping or replacing its ticket), the peer's end (answered `Ended` to the record), or the shell's hang refusal. A record dropped with its owner, such as an `Application` at its process's exit, drops its tickets, and their requests are already ended with the peer.
- Its relation to the front entry: the transition sees a front view, never the front's records. The terminal follow-caret ruling needs exactly this: an output read answered while a review press is the running entry lands after the press (`docs/parity/terminals.md`, "The review cursor while output is written", Dickson, 2026-11-28). Today `facts_answered` gets the front and reads `press.reviews()`; under the design the answer's transition reads the front view's kind and stage, and the press, after its own wait, re-reads the model through `cx.read`. Async rule 4's world-change mode then catches a press that kept a copy of the model across its wait, which is the copy the ruling avoided [features B, review cursor in terminals].
- A record request has no checkpoint of its own; its record is in the dump's snapshot, and the terminal models stay serialisable behind their `Arc` (Dickson, 2026-10-26) [features B, flight recorder].
- What it adds: one ticket per owner variant, and the list of owners. The list is closed (Dickson, 2026-11-28): a new owner is a change to rule 1, so Dickson's. The other option, every such read as a queue entry, changes the order of output against keys and the shell list's survival of the foreground change, both ruled.

### Ending the front's request

Today `Logic::ends` judges the front for `Cause::Arrived` (a later key, or a focus that counts as pending) and `Cause::Settled` (a fact changed, such as the 150 ms release of a held key) (`entries.rs` 624 to 646). The design adds this as data on the stage rather than a method on each arriving kind [features A, addition B; features B, gap 2].

An end cause is a level, a condition over Core's state, not an event [review M2]. NVDA's caret watch checks at every poll whether a script is waiting and whether a focus is pending (`nvda/source/editableText.py` 94 and 97), and today `Cause::Settled` is judged again on the facts (`inbox.rs` 391, `entries.rs` 528). An edge would miss the key that arrived while the caret key still waited behind a focus.

- The causes, each a predicate:
  - `LaterKey`: a key newer than the front's own is in the queue.
  - `PendingFocus`: a focus entry that counts as pending is in the queue (`focus/validity.rs`).
  - `KeyReleased`: the front's stroke is no longer held in `Keys` (the 150 ms release).
  - `Committed`: `Versions` has committed a focus newer than the front's.
- The driver evaluates the front's declared causes when the stage asks, and again at the end of every step while that stage waits. A cause that holds ends the wait: Core sends `Cancel(reason)` and resolves the front's ticket `Ended(Withdrawn(cause))` in the same step. A cause that held before the stage began therefore ends it at once, as NVDA's first poll would.
- The front entry is not killed: its `run` continues with that outcome and decides, for example passing the held key silently or speaking the unit it has.
- Only the front's request is ended, and only through its ticket; a record request is never ended by a cause. The cost is the declared predicates evaluated once per step while such a stage waits, reads of Core's own records with no call or wake.
- Users of it: caret keys (`caret.start` and `caret.watch` end on `LaterKey`, `PendingFocus` and `KeyReleased`), the confirmation of an unconfirmed MSAA focus, Enter, Backspace and Delete in a text field, and say-all's read when a key's fence cuts it.
- Approved by Dickson (2026-11-28): a new queue hook (CLAUDE.md, item 4), end causes as levels declared per stage. The other option is a method on the arriving kind, `fn ends_front(&self, front: &FrontView) -> Option<CancelReason>`, which puts the same judgement in each arriving kind's code instead of in the waiting stage's declaration, and is an edge, so it needs a second check when a stage begins. I recommend the declaration, because the stage that waits is the one that knows what makes its wait moot (Claude, 2026-11-28).

## Reader reducers and workers

The same shape in the listener, each outpost, the supervisor and the hosts:

- One root reducer, `step(&mut self, input) -> Effects`, with no I/O. Its inputs are a message from Core, a completion, a callback event, and a due time.
- Its effects:
  - `Send(Answer)`.
  - `Job { slot, gen, job }`.
  - `Start(slot)`, `Abandon(slot)` and `Release { slot, ids }`.
  - `WakeAt`.
  - `Log`.
- A worker loops: it takes a job, runs it with the `Platform` trait, and posts `Completed { slot, gen, job, result }`. A completion whose `gen` is not the slot's current one is dropped and logged.
- A hung call is a job that never completes. Only a transition ends it: abandonment on Core's `Cancel`, the process's exit, or the peer's own end.
- The job runners are the step executor (`Read`, `Ancestors`, `Find` and the rest). They live in the GPL-tier logic crates, generic over `Platform`, so the simulator runs the same executor code against the world. The Windows crates implement `Platform` over `verbatim-uia`, `verbatim-ia2` and `verbatim-win32`.
- `Platform` is cut at the level of UI Automation's and MSAA's own calls: cached and current reads, plain and ignore-default, tree walkers by view, conditions, compare, focused element, element from handle, event registration, and the window functions. It also carries the edit control's messages (`EM_*`, `WM_GETTEXT`), the tree view's `TVM_*` messages and Uniscribe's `ScriptBreak`, which today's text reads and words use (`verbatim-ia2`'s Uniscribe tests check every offset against NVDA's own helper) [features B, addition 12] (Claude, 2026-11-28). The world server then implements the same semantics the NVDA stand-ins implement, and the conformance test holds both to it.
- Remote operations: `verbatim-uia-rops` is Windows-specific (its builder imports `IUIAutomationElement`), so a logic crate cannot depend on it [review 2 M3]. `Platform` is therefore cut above the programs at the batch segment: the executor in the logic crates hands each run of fusable steps to `Platform::run_segment`. The Windows `Platform` runs it through `verbatim-uia-rops`' generator as programs, wherever a program can do the read; the simulator's `Platform` runs the same segment by ordinary calls, all served at one simulated instant, as one program would read ("Remote operations in the replay"). So the bulk runs do not exercise the program generator; the contract suite does. There is no interpreter of the instructions in the world server (Dickson, 2026-11-28) (Claude, 2026-11-28, the cut; the other option, moving the generator into a logic crate, needs it split from its UI Automation imports).
- Callbacks: a UI Automation or WinEvent callback is an I/O source. It makes no decision and keeps no record. Where its element goes was Dickson's (item 3), decided on 2026-11-28 as recommended below:
  - Through the root. The callback posts the event with the element in a `Transit` box. The root forwards the box to the application's check worker, which takes it into its arena. A `Transit` cannot be released except by a worker. The cost is one more thread hop per event, about two more wakes (estimate, not measured). The listener's background-event backlog measurement (ADR 0003) is to be repeated before this is chosen.
  - Straight to the check worker. This needs a routing table from process to channel outside the root, which is a copy.

  Through the root, unless the backlog measurement misses its budget (Dickson, 2026-11-28, on Claude's recommendation).

## The simulator

### Its interface to each process's logic

- Each process's logic is a value implementing `Process`. Core's driver implements it around its async entries.

  ```rust
  trait Process {
      type Input; type Effect;
      fn step(&mut self, input: Self::Input, now: SimTime) -> Vec<Self::Effect>;
      fn next_due(&self) -> Option<SimTime>;
      fn dump(&self) -> Dump;                // records, stages, open requests, the last held view per reader
  }
  ```

- The simulator owns everything else:
  - The clock.
  - One first-in, first-out queue per pipe, and the order across pipes.
  - Each worker slot's pending job and when it completes.
  - Process starts and ends.
  - The world server.
  - A speech sink.
- The shell-step function's `Io` trait has one production implementation and one simulator implementation, so the shells run unchanged. The code outside every simulation is the calls themselves: COM, UI Automation, hooks, pipe I/O, audio and engines. This is the roadmap's own aim ("The simulation of the reducers, deferred").

### Seeds

- A run is named by the world (a recorded file, or a generator and its seed), the schedule seed, the fault profile and the commit.
- Each component draws from its own random stream, so a change in one stream does not reshuffle the others.
- Every choice is recorded in a choice list. A failing run is shrunk by deleting and lowering choices, and the shrunk list is saved as a regression case.

### Fault kinds

- Message timing: delivery delay, and the order between pipes, each pipe kept in order.
- Job timing: fast, slow past the request's due time, never (a hung call), and late after abandonment.
- Process ends at any step: an outpost, the listener, a host or the supervisor's child, including before `Ready` and during a batch.
- An application exits at any step. A process id is reused by a new application after an exit.
- World events (from the application model, the section "Verification"):
  - An element gone between two calls.
  - A focus event late, repeated, or from a second runtime identity.
  - A reused runtime id.
  - A window that hangs.
  - Floods at measured rates.
- Timers falling due together.
- The world-change mode of Core's async rule 4.

### Invariants, checked every run

- A batch cancelled while it runs, or answered after Core let go of it, leaves no standing request, registration or unsent id at its reader once the world is quiet [review M1].
- Every request has exactly one ending, whether an entry or a record owns it. At the end of a run, no `Out` record outlives its peer or its owner, and no request ended twice.
- An end cause ends only a stage that declares it, and never a record's request.
- An id named in an `Ask` is one Core's records name. No `NotThere` answers an id Core's records name, unless the world removed the element or the id was lost with an abandoned worker.
- With the world quiet and a message sent, every reader's element count equals the ids in the last held view plus those sent after its `handled` position, less the elements marked `Lost`, so nothing leaks [review 2 S3].
- COM releases happen only in `Release` jobs. Structurally there is no other path, and the simulator counts them.
- No completion from an old worker generation is applied.
- A wake with nothing to do is a violation: every `WakeAt` acts when it falls due.
- Determinism: the same seed gives the same choice list and the same decision log.
- Liveness: with no hung call the queue drains, and with one the front entry ends at its due time.
- The world-change mode's rule about facts read before a wait.
- Speech and decisions match the oracle for that world, within the allowlist (below).

## Verification, end to end

The pipeline has six stages.

1. A world is recorded, or generated from a model of application behaviour.
2. The world server serves the world to both screen readers.
3. NVDA runs headless over it with a simulated clock.
4. Verbatim runs over it under the seeded simulator.
5. A step-by-step comparison of speech and decision logs, by fingerprint, is made against a structured allowlist tied to parity rulings.
6. NVDA's line coverage over the corpus is the gap list. The desktop suite keeps the replay honest.

### World capture

One recorder, built into Verbatim's readers, with one format and two levels, replaces the separate flight recorder and world recorder (Dickson, 2026-11-28). The first level is the trace ("The trace", below). The second is world capture:
- Switched on when wanted. The outposts and the listener then also read, at each event, everything a world needs: the properties NVDA reads, the tree, text and its boundaries, the window facts, and, through the in-process hooks, the display model's GDI stream and the IME's state. They read it next to their own reads, by remote programs where a program can do it, so the reads run inside the application's provider; that is cheaper than an external recorder's cross-process reads one call at a time.
- A world capture replays onto both Verbatim and NVDA.
- Scenario recording is a world capture made while a scenario runs, with Verbatim's readers running and Core not speaking (agreed by Dickson). The scenario's steps (key presses, typed text, UI Automation's invoke, launching and closing) are driven as the end-to-end harness drives them today, so a capture is a scenario.
- It reuses `verbatim-event-record`'s (3,134 lines) MSAA and UI Automation hooks and JSON Lines writer, moved into the readers. Dickson's contract of 2026-11-10 for that tool's command line changes to this recorder's (decision 11, approved 2026-11-28).
- Its cost per event inside a real application is measured at the worst case, a terminal flood and a busy web page, before deciding whether world capture can stay on in normal use.
- Snapshot policy, at each event:
  - The sender and its raw-view ancestors to the root, each with its full property set.
  - The sender's siblings and children, one level.
  - The focused element, read again.
  - The foreground window's chain.
  - The window facts of every window named.

  A world answers only what was recorded. Every other read is a miss: logged and never defaulted. "None" is itself a recorded fact: a list of children, relations or attributes is recorded with whether it is complete, and a read of a list not recorded complete is a miss, never an empty answer [review M4].

  A miss fails its step; it never excludes it [review M4]:
  - A miss by Verbatim on a step NVDA read without one is a Verbatim failure, and counts as a catch in the measurement gate, when the read is one the recording policy covers (the snapshot policy's elements and the property union). A Verbatim read outside the policy, such as a deeper walk, is a world failure: the policy widens and the world is re-recorded before the step is judged [review 2 S8].
  - A miss by NVDA is a world failure: the world is not admitted to the corpus until the recording policy widens and it is re-recorded.
  - A miss by both is a world failure too.

  The experiments show why "none" must be impossible as a default: the Run dialog's description came out "Type the name of a program..." against the capture's "Run DLL" with no miss reported, because the hand-built world answered its missing children as none (`nvda-oracle/text/out_ia.txt`).
- Format: JSON Lines with a versioned header. There is no compatibility for old recordings: a format change re-records. Line kinds:
  - `header`: the format version, Windows build, UI Automation core version, recorder commit, the high-resolution clock's frequency, and the DPI and monitor rectangles.
  - `process { t, pid, created, image, identity, event: start | exit }`.
  - `window { t, hwnd, class, text, style, exstyle, parent, owner, pid, tid, visible, enabled, rect, hung, uia_server_side }`, emitted at creation and at each change WinEvents report.
  - `foreground { t, hwnd, gui_thread: { active, focus, capture, caret, menu_owner, flags } }`.
  - `element { t0, t1, id, runtime_id, view: { control, content }, raw_parent, raw_children, props: { Name: { plain, ex, remote }, ... }, patterns: { Text: supported | not, ... } }`.
    - `t0` and `t1` bound the read.
    - `plain` is `GetCurrentPropertyValue`. `ex` is `GetCurrentPropertyValueEx` with ignore-default set, which records UI Automation's not-supported value. `remote` is the value a remote program read, recorded only where calibration found it differs from `plain`; the ordinary-call path then serves Verbatim's programs the `remote` value and NVDA's classic reads the `plain` one [review 2 S9].
    - The property set is the union of what both screen readers read. The prototype's call list (`nvda-oracle/uia/out.txt`) and Verbatim's `PropertyId` table give it.
  - `gone { t, id }`: an element that answered "not available" when read again.
  - `msaa { t0, t1, hwnd, obj, child, role, states, name, value, description, default_action, location, parent, children }`, and from M7 the IAccessible2 interfaces.
  - `event { t, kind, sender, cached }`. The kind is the UI Automation event id, or a property id and its value, or the WinEvent with its window, object, child, thread and time.
  - `key { t, vk, scan, flags, injected, chars, toggled }`. `chars` is what `ToUnicodeEx` gives under the recorded layout, and `toggled` the lock key's state after it.
  - `step { t, action, target }`. An application key is delivered to the application in the recording, and the world changes that follow it until the next step are its recorded reaction. A reader gesture (a screen-reader command) is not sent to the application in the recording, since both screen readers consume it.

  Added from the NVDA inventories, because NVDA reads each of these and a world without them makes the harness guess or reach the live machine [`text.md`, `handlers.md`, `core.md`, `objects.md`]:
  - `text { t, element, model, content, runs, selection, boundaries }`: a text model per element and step, by model (UI Automation's text pattern, IAccessibleText, the edit control, the text object model): the full text, attribute runs for every attribute either reader asks, the selection after each key, and the provider's word and line boundaries and end-of-text rule as boundary lists or a named rule. The text experiment showed a model answers every call NVDA's review and caret scripts make (see the experiment results). A world records the provider's behaviour; it never models what NVDA expects, since unit answers are provider-specific (the console host before its formatted API level is end-inclusive; Windows Terminal's paragraph spans the buffer).
  - `message { t, hwnd, msg, wparam, lparam, answer, buffer }`: window messages a reader sends with their answers, including those that need cross-process memory (`EM_*`, `TVM_*`, the list view's `LVM_*` fallback). This is also the edit control's text and selection line that features B found missing.
  - `registration`: the recorder registers for every event globally, so a world holds the events of unregistered elements too; each event line carries the sender, and replay filters delivery by each reader's own registrations.
  - In `process`: `helper: registered | absent`, the fact that NVDA's in-process helper loaded and registered there, which decides typed-character source and UI Automation against object model in Word, Excel and Chromium; and the product name and version and the architecture. A recorder running with no screen reader cannot observe it, so it is declared per world, taken from the live NVDA capture's log of the same application.
  - In `header`: the Windows display version and update build revision as well as the build, the keyboard layout and sticky-keys setting, the locale with its time and date formats, and the recording machine's ids for custom UI Automation properties and annotation types by GUID.
  - `desktop { t, input_desktop, locked }`, and a window's ghost (`_GhostWindowFromHungWindow`'s answer) in its `window` line.
  - `clipboard { t, text }` at each change, for copy and the status-bar scripts.
  - Each event's arrival time, from which the harness chooses the limiter's batch boundaries (harness rule 11); the world declares no boundaries.
- Limits:
  - A value that holds for less time than one read is lost.
  - An application can behave differently with a UI Automation client present. Verbatim's readers are the capture's client, as they are in use, so the capture is not worse than either screen reader.
  - Reads come after the event by the capture's latency, and the world serves, at simulated time t, the newest snapshot that ended by t.

### The replayed Windows

- One world server, written once in Rust, platform-neutral, with a C interface. Verbatim's simulator links it. The NVDA harness loads it with `ctypes`, and its stand-in COM objects become thin shells over it.
  - The semantics are written once: plain and ignore-default reads, cached against current, views, conditions, and the window functions.
  - A conformance test runs both sides' calls over a fixed world and requires equal answers.
  - Today's prototype keeps these semantics in Python (`standins.py`, 660 lines). They move.
- Reactions to a reader's own actions [review M5]. A world capture holds the reaction to an action only for the action taken at that time during the capture: Verbatim's own action (setting focus, invoking, moving the caret, selecting), or a scripted `reader action` step. Core does not speak during a capture, but its readers act as in use.
  - At replay, the reaction is released only when the reader is observed to make the same action: the same kind, on the same target, within the step and before the recorded reaction's first event in simulated time. A reader that does not act, acts on another target or kind, or acts outside that window fails the step. So a Verbatim that forgot to act cannot hear the reaction.
  - An action at another time or of another kind needs a re-recording; that is the default. A stated model is allowed only for a reaction on the list below, each rule naming the recording that shows it.
  - The model list starts with one entry: forcing the foreground after a popup, which Verbatim's port of NVDA's postPopup does (commit d6012933). Its rule: the foreground moves, then `EVENT_SYSTEM_FOREGROUND`, then the window's focus event.
  - The features whose worlds need this, R4 for both readers and not only NVDA: say-all (the caret moved line by line, `updateCaret`), moving the caret to the review position and the review following the caret, Activate, moving focus to the navigator, the shell list's click, and select-then-copy. Say-all's window depends on the speech duration model, so its worlds also pin that model.

### NVDA headless

The base is `boot.py`, `standins.py` and `run.py`. These already run NVDA's own `UIAHandler`, `eventHandler`, `api`, NVDA objects and speech code on stand-in COM objects through real vtables. The pinned NVDA is submodule commit 732d1ce2; the installed NVDA at `C:\Program Files\NVDA` is the same commit and holds the native libraries and generated data the source tree lacks [`speech.md`].

#### NVDA by replay class

Each inventory classed every module in its area, by the class that blocks it. Lines are each inventory's approximate figures; native lines are counted apart.

| Area (inventory) | R1 as is | R2 stand-in | R3 clock or threads | R4 Windows reacts | R5 native or in-process | R6 not needed |
|---|---|---|---|---|---|---|
| Core, input, Windows bindings, config, the rest (`core.md`) | 23,500 | 11,100 | 800 | 550 | 6,600 | 48,300 |
| Handlers: UI Automation, MSAA, Java (`handlers.md`) | 2,190 | 2,680 | 250 | 29, plus 436 native | 4,880, plus 1,122 native | 1,510, plus 389 native |
| NVDA objects, app modules, control types (`objects.md`) | 5,157 | 17,797 | 3,388 | 2,051 | 9,910 | 257 |
| Speech, synthesizers, symbols, braille, vision (`speech.md`) | 13,980 | 55 | 2,851 | parts only | 3,190 | 18,030 |
| Text, editing, review, terminals, documents (`text.md`) | 4,200 | 6,000 | 1,500 | none alone | 2,700, plus 4,200 native | 2,150 |
| Total | about 49,000 | about 37,600 | about 8,800 | about 2,600 | about 27,300 | about 70,200 |

The Python totals come to about 195,600 against NVDA's 188,358 Python lines, so at least 7,200 lines are counted twice: `textUtils`, `mathPres`, `speechDictHandler`, `languageHandler` and `ui.py` appear in two inventories, and the text inventory counts parts of `NVDAObjects` that the objects inventory also counts. Read the totals as shares, not sums.

What the classes mean for Verbatim's features:

- R1 is most of what Verbatim's built features need: event handling, focus and ancestry, object and property speech, control types, review scripts, the speech package outside its manager, symbols and dictionaries.
- R2 is where the work is. The largest stand-ins are MSAA and IAccessible2 objects (9,140 lines of `NVDAObjects/IAccessible` wait on them, and the harness stubs `IAccessible2Lib`), UI Automation's text pattern and ranges, the other UI Automation patterns and children caches, window messages with cross-process memory, and the window and keyboard functions.
- R3 is small in lines but on every path: NVDA's pump and `core.callLater`, the caret watch, the speech manager, say-all, the watchdog.
- R4 is the key reaching the application inside NVDA's polls, and NVDA's own `setFocus`, clicks and cursor moves.
- R5 and R6 are listed in "What stays outside the oracle".

#### Harness rules

These are the rules the inventories imply. Each names the defect in today's harness that it closes.

1. No stand-in silently accepts a call. Today `wx`, `gui` and `braille` are stubs whose every attribute is a `MagicMock`, and that loses speech without an error [`core.md`, gaps 1 and 2; `handlers.md`]:
   - `core.callLater` goes through `wx.CallLater`, so every delayed call is dropped: the lock key's report 30 ms after Caps Lock, the virtual desktop's name 250 ms after a `#32769` name change, `ui.delayedMessage`, MSAA's 50 ms fake focus and 200 ms desktop check, and the mouse shape's `gui.NonReEntrantTimer`.
   - `gui.blockAction.when` decorates 50 global commands, which therefore do nothing, and the eight "Windows locked" messages become mocks that the capture drops as non-strings. The real `gui/blockAction.py` loads with a stand-in for `gui.message.isModalMessageBoxActive`.
2. Stand-ins are strict: any attribute or call not implemented raises, and a raise is a miss, which fails its step as the recorder's rules say and names the stand-in to widen [review M4]. `winUser` already works this way; `wx`, `gui`, `braille` and the stubbed `comInterfaces` modules move to it.
3. Pure helpers pass through. A function whose answer depends only on its arguments runs for real, so replacement is by "reads the world", never by module [`core.md`, gap 3; `speech.md`, finding 6]:
   - `winUser`'s arithmetic (`LOBYTE`, `HIBYTE`, `MAKEWORD`, `LOWORD`, `HIWORD`, `GET_X_LPARAM`, `GET_Y_LPARAM`, `MAKELONG`), which the harness replaces today.
   - nvdaHelperLocal's string functions (`calculateCharacterBoundaries`, `calculateCharacterOffsets`, `calculateWordOffsets`), which load from the installed lib; Uniscribe's spelling split was checked to work this way.
4. Nothing reaches the live machine for a fact of the world. Every pass-through is listed and checked, and a no-leak check fails a run in which NVDA opens a process, reads a window or calls a system function not in the world or on the pass-through list; the experiments' leaks (NVDA opening the live process 6364 in the list-view and browser runs, and the list view's ancestry running through this machine's real desktop window) are closed by it [review 2 M4]:
   - `eventHandler.shouldAcceptEvent` calls `winBindings.user32.GetWindowLong` directly (557 to 562), bypassing `winUser`, so it needs a `user32` stand-in reading the world's `exstyle` [`core.md`, gap 4].
   - `user32.OpenInputDesktop` passes through today and answers about this machine; it comes from the world's `desktop` line [`handlers.md`].
   - `appModuleHandler` binds `_getWindowThreadProcessID`, `_findTopLevelWindow` and `_getProcessHandleFromHwnd` at import, before the stand-ins exist, so they are replaced on that module [`core.md`].
   - `winVersion.getWinVer` is pinned to the world's header, since it gates remote operations, selective registration, virtual desktop names and Word's UI Automation [`handlers.md`].
   - Custom UI Automation property and annotation ids come from the world's GUID table, not this machine's UI Automation core [`handlers.md`].
5. NVDA's full pump, in its order: the touch handler, `JABHandler.pumpAll`, `IAccessibleHandler.pumpAll`, `queueHandler.pumpAll`, `mouseHandler.pumpAll`, `braille.pumpAll`, `vision.pumpAll`, `winAPI.sessionTracking.pumpAll`, then `AutoPropertyObject.invalidateCaches` (`core.py` 1015 to 1048). An immediate pump (focus, input) cancels a pending delayed one; a delayed pump waits `PUMP_MAX_DELAY`, 10 ms; a pump requested during a pump runs again; `flushQueue` runs only what was queued when it started. Today `core_cycle` runs only the queue, so no MSAA, mouse or lock-state replay is right [`core.md`, gap 5; `handlers.md`, "The core cycle"]. `CorePump` is taken from `core.main` unchanged (see the experiment results).
6. One simulated clock behind every timer: `time.time`, `perf_counter` and `sleep` in each module's namespace, `wx.CallLater`, `wx.CallAfter`, `wx.Timer`, `wx.Yield`, `threading.Timer`, `gui.NonReEntrantTimer` and so `core.callLater`; and the bound names (`watchdog._timer`, `keyboardHandler.time`, `scriptHandler.time`). Clock windows that change speech then replay: the 0.5 s window-answer cache, the 0.5 s live-region repeat, the 0.1 s typed-character suppression, multi-press timing [`core.md`; `handlers.md`; `speech.md`].
7. Thread order is part of the world. Every producer onto NVDA's event queue enqueues at a declared point between pumps: UI Automation's thread, the keyboard hook thread (which resolves scripts and queues the speech cancel against the main thread's focus), NVDAHelper's RPC threads, the MSAA win-event path and the limiter's flusher. `isPendingEvents` counts at that point, and `api.processPendingEvents`, the nested pump inside scripts, delivers the world events whose time has come during the script's wait [`core.md`, hardest 2 and 3; `text.md`].
8. Capture where NVDA's speech is whole [`speech.md`, "Where to capture speech"]:
   - Requested speech at `speech.extensions.pre_speech`, with a named handler kept alive by the harness, since extension points hold handlers weakly and a lambda is collected unheard. It sees every sequence after the language filter, say-all and callers of the package attribute included, with its priority, symbol level and commands. Today's patch of `speech.speech.speak` misses say-all's `SpeechWithoutPauses` (which kept the original), `speech.speak` callers, and joins the strings, losing commands and priority.
   - Delivered speech at `synthDriverHandler.pre_synthSpeak`, plus the stand-in synthesizer's `cancel()` calls and the index marks it reports. What a user hears is each delivered utterance up to the last index reported before a cancel; speech dropped as expired focus is the difference between the two streams.
   - Sounds by deciders on `tones.decide_beep` and `nvwave.decide_playWaveFile` that return false.
9. A stand-in synthesizer that carries the real one's facts: its `name` (which keys `config.conf["speech"][name]`: capital pitch, "say cap", beeps, the voice dictionary), its voice's language and `availableLanguages`, `supportedCommands` and `supportedNotifications`, and index reports at simulated times from a duration model per utterance; optionally OneCore's skipping of indexes with no text between. The silence synthesizer changed the words: with no language, spelling "Ab" queued "English (not supported)"; with no commands, no capital pitch; and with no index reports the manager pushed only the first utterance, and the capital beep never ran [`speech.md`, findings 2 and 3].
10. NVDA's data as installed: `cldr.dic` from the installed `locale\en` (the source tree lacks it, and its loss costs only a debug line: "😀" passed unnamed), `builtin.dic`, the capture's `nvda.ini` and profiles pinned per world, the generated `IAccessible2Lib` built with `midl` from the IAccessible2 IDL at the pinned commit (the experiment's route; the installed `library.zip` also carries one), and Python 3.13, whose pyc magic number matches the install (this machine's default Python is 3.14) [`speech.md`, findings 4, 5 and 7; `handlers.md`].
11. The native UI Automation event limiter is on, as NVDA's default (`enhancedEventProcessing`, `configSpec.py` 310); the harness turns it off today. The real DLL is the oracle, driven deterministically by gating its flusher: the harness decides each batch boundary, adds the batch's events and only then lets the flusher run. The 32-line model is only a cross-check [`handlers.md`, hardest 1] (Dickson, 2026-11-28).
    - The gate: the first event into an empty list wakes the flusher with no point the harness controls (`rateLimitedEventHandler.cpp` 37, 56 to 59, 90 to 97), so each batch starts with a primer event that the harness's inner handler holds, which keeps the flusher inside its first call while the batch's events are added; the harness swallows the primer's delivery [review 2 S4].
    - Plausible boundaries, defined: a boundary may fall between two events only if the gap between their recorded arrival times is no shorter than the shortest measured run of NVDA's handler for the batch before it, and a batch can hold an event only if it arrived before that batch's flush could have started. The handler times come from a one-time desktop measurement of NVDA's handlers, which this definition makes required, not optional [review 2 S4].
    - Verbatim's listener keeps the newer of two coalesced events, where NVDA keeps the older: a ruled difference, kept (Dickson, 2026-11-28, commit deaf7553, `docs/parity/backends.md`). It enters the allowlist as a general rule, not per world [review 2 M2].
12. Live reads are answered as of each read, at the simulated time of the read: `CurrentHasKeyboardFocus` in the duplicate rule and `shouldAllowUIAFocusEvent`, `GetFocusedElement` on UI Automation's thread, `GetForegroundWindow` in foreground handling and the two-cycle foreground defer [`handlers.md`, hardest 3].
13. NVDA's event registrations filter delivery. On Windows 11 22H2 and later selective registration is the default, so property and caret events reach NVDA only for elements it registered; the stand-in records registrations today but delivers everything. The world holds all events, and the stand-in delivers only what NVDA's registrations admit [`handlers.md`, hardest 4].
14. The key reaches the application inside NVDA's polls. Keys are injected at `keyboardHandler.internal_keyDownEvent` and `internal_keyUpEvent`; their return value says whether the key reaches the application. A passed application key releases its recorded reaction at its recorded delays, so `editableText._hasCaretMoved`'s poll sees the caret move, the event or the timeout as NVDA would. A reader gesture is not delivered. A key NVDA passes that the recording did not deliver, or the reverse, is a miss for the rest of the world [`text.md`, cross-cutting seams; `core.md`, hardest 4].
15. Typed characters come from where NVDA takes them. With the in-process helper absent, from `ToUnicodeEx` on the hook thread, answered from the key line's `chars`; with it registered, from `nvdaControllerInternal_typedCharacterNotify`, which the harness calls with the same characters at the recorded point; consoles by their own rule. The world's `helper` fact chooses (`keyboardHandler.shouldUseToUnicodeEx`, 178 to 202) [`core.md`, hardest 5; `text.md`].
16. Known limits of fidelity: MSAA's `liveNVDAObjectTable` is a weak dictionary whose hits depend on when objects are collected, deterministic within a replay but not guaranteed to match live NVDA [`handlers.md`].

Event injection stays at NVDA's own entry points: `HandleFocusChangedEvent` (874), `HandleAutomationEvent` (757), `HandlePropertyChangedEvent` (960), `HandleNotificationEvent` (1063), and `winEventCallback` (`IAccessibleHandler/internalWinEventHandler.py` 69). Patches to a copy of NVDA, outside `nvda/`, are made only where load time cannot reach, and the copy is checked against the existing live captures: every world that has a live NVDA capture must give the capture's speech, or the copy fails.

Outputs: the requested and delivered speech streams and sounds above, and decision log lines from wrapped decision points.
- The decision points are a table, one file, from an NVDA function and outcome to a Verbatim decision name.
- Examples: `isNativeUIAElement`'s verdict, "Ignoring duplicate focus event", "NVDAObject chose to ignore event" (both seen in `out.txt`), `doPreGainFocus`, the limiter's merges and the MSAA limiter's drops.
- From M18, braille regions too: the display stand-in with a fixed cell count, captured at `braille.extensions.pre_writeCells`, with braille off in speech-only runs, since it adds reads the world must answer [`speech.md`].

#### In-process code, and what stays outside the oracle

In-process code is not outside the oracle by nature. The browsers experiment loaded NVDA's shipped `nvdaHelperRemote.dll` into the test process itself, with no injection, over stand-in IAccessible2 objects, and NVDA's native browse buffer and live-region code ran whole. The native experiment (`experiments/nvda-oracle/native/`, reported 2026-11-28) did the same for list views, the display model and the IME. Licensing is as for the browsers: GPL binaries in a GPL-tier test tool only.

Runs in the oracle, not yet verified [review 2 M4]. Each counts as an oracle only when:
- its no-leak check passes (harness rule 4);
- NVDA reads the recorded world wherever the two readers must read one world: a rebuilt control passes a read-back check that it answers what the recording holds, failing the world otherwise, and the fonts and DPI the replay draws with are pinned from the world's header. Where NVDA must use this machine's library (comctl32 6.0.26100.9818 for list views, GDI for drawing and measuring), the difference from the recording machine is stated in the world's header and judged by the read-back check;
- a world recorded alongside a live NVDA capture gives the capture's speech (the copy check).

Each, with its limits and what its recorder captures:

- Browse buffers (`virtualBuffers`, the native `gecko_ia2` builder) and IAccessible2 live regions, with Python browse mode over them. Chromium in UI Automation mode uses UI Automation browse mode, which the UI Automation oracle covers. x64 only.
- List views. A real, hidden comctl32 version 6 `SysListView32` in the test process, with NVDA's real `sysListView32` classes and its in-process helpers (`nvdaInProcUtils_sysListView32_*`), gave NVDA's speech for focus and moves, the visual column order included; the out-of-process `LVM_*` path spoke the same.
  - Limit: the item never reported itself focused because the hidden window is never in the foreground (`native/lv.py` 88), so focus was delivered with `processFocusWinEvent(force=True)`, which bypasses NVDA's own focus checks. So list-view worlds cannot judge focus rules, the focus slice's subject, until a rebuilt control can be shown in the foreground of a desktop the harness owns; any hidden rebuilt control has this limit [review 2 S6].
  - The recorder captures everything readable from outside by `LVM_*` messages: items, subitems, headers, column order, rectangles, states, styles, groups and win events. The replay rebuilds the control from them. A virtual list (`LVS_OWNERDATA`) needs the rebuilt control to answer `LVN_GETDISPINFO` from the recording. Owner-drawn lists fall back to the display model.
- The display model. NVDA's real GDI hooks, installed in the test process only, over real GDI drawing into a hidden window, gave `DisplayModelTextInfo`'s lines, words and caret (through `GetGUIThreadInfo`), and `DisplayModelLiveText`'s change notification and diff speech. The replay redraws the recorded runs with real GDI.
  - The recorder: the stream of GDI calls is visible only inside the real application, so the recorder runs hooks inside the recorded application and records it in-process (Dickson, 2026-11-28).
- The IME. IMM32 worked through `ime.cpp`'s real message hooks with stand-in IMM32 answers. Replaying the boundary calls (`IMEOpenStatusUpdate`, `inputConversionModeUpdate`, `inputCompositionUpdate`, `inputCandidateListUpdate`) straight into NVDA's callbacks gave identical speech.
  - Limit: TSF was not tried, since it needs a registered text service, a system setting. Its output is the same boundary calls, so boundary replay covers NVDA's speech for TSF but not NVDA's TSF native code.
  - The recorder: IMM and TSF state is readable only inside the application, so the recorder runs hooks inside the recorded application (Dickson, 2026-11-28). It records the raw state only: the composition string and cursor, the candidate list with its page and selection, the open and conversion status, the keyboard layout and the IME file. Both readers are served from it, NVDA through `ime.cpp` over stand-in IMM32 answers, so NVDA's native IME code stays under test. The boundary calls are not recorded: they are `ime.cpp`'s output, and recording them would put NVDA's helper in the recorded application and hold one fact twice [review 2 S7] (Claude, 2026-11-28).

To try the same way, until then verified as the last column says:

| To try | Why it is in-process | Verified until then by |
|---|---|---|
| Web editors' caret, first and last positions (`nvdaInProcUtils_IA2Text_findContentDescendant`) | in-process, the same helper the browsers experiment loaded | captures (M7) |

Outside the oracle:

| Outside | Why | Verified by |
|---|---|---|
| Applications' object models, other than classic Word's and Excel's helper paths (PowerPoint, Outlook, Visual Studio, Office charts, the text object model's embedded objects, and Word's and Excel's object-model calls outside the helper) | late-bound `IDispatch` models and event sinks | captures and the desktop suite, until a per-application recorder exists (M13). Classic Word's and Excel's helper paths are in the oracle as "runs, not yet verified", fed by the Word and Excel fact recorder the Office results give [review 2 S10] |
| Touch, braille hardware, NVDA's GUI, Remote Access, the magnifier | not in Verbatim's features or not heard | not verified (R6) |
| The Java Access Bridge | not yet in Verbatim's features | brought into the replay later, by stand-ins at `windowsaccessbridge`'s boundary and a Java recorder ("Coverage of the accessibility APIs") |

#### Coverage of the accessibility APIs

Dickson's requirement (2026-11-28): the replay covers MSAA, IAccessible2 (once Verbatim supports it), UI Automation both classic and through remote operations, and later the Java Access Bridge, for Verbatim's simulator and for the NVDA oracle alike. Each is recorded once and served by the one world server to both.

| API | Recorded as | Served to Verbatim's simulator | Served to NVDA |
|---|---|---|---|
| MSAA | the `msaa` and `window` lines: per window class, text, style, control id, parent, process, thread, and the window and client objects' role, name, value, description, state bits, shortcut, default action, location and children (the text experiment's list) | through `Platform`'s MSAA calls | the `oleacc` and `IAccessible` stand-ins over the world server, which now cover every call NVDA made in the text experiment |
| IAccessible2 | per object: roles and states, the full attribute string, locale, text with embedded-object characters and attribute runs, hyperlinks, relations by type, table and cell extents and headers, actions (the browsers experiment's list) | through `Platform`'s IAccessible2 calls, from M7 | stand-in IAccessible2 objects with the type library built from the pinned IDL, and NVDA's helper loaded in process for buffers and live regions |
| UI Automation, classic | the `element`, `text`, `event` and `registration` lines | through `Platform`'s calls | the comtypes `IUIAutomation6` stand-in over the world server |
| UI Automation, remote operations | the same world: a program's result is computed against the replay, never recorded | the ordinary-call path at one simulated instant per program, proven equivalent by the contract suite below | NVDA's own remote paths (Word, browse-mode headings) through NVDA's local executor |
| Java Access Bridge (later) | a Java recorder capturing the accessible-context tree (each context's role, states, name, description, text and its attributes, table and relation data, key bindings and actions) and the bridge's events, from the bridge itself | through `Platform`'s Java calls, when Verbatim supports Java | stand-ins at the boundary where NVDA's `JABHandler` calls `windowsaccessbridge`'s functions (`_fixBridgeFuncs`, `JABHandler.py` 337 onward), answered from the world, with the bridge's callbacks raised from its events |

#### Remote operations in the replay

Dickson's rulings of 2026-11-28. Verbatim uses remote operations wherever possible, so its programs must be proven against the real thing, not against a reimplementation of it. The world server has no instruction interpreter, and no real API is used anywhere in the replay: the replay runs Verbatim's remote programs by the ordinary-call path, and the live API tests ("Three kinds of test") prove that path against the real core.

- The contract suite, a set of live API tests. Every kind of program Verbatim's builder (`verbatim-uia-rops`) produces runs on a desktop against the real UI Automation core and real providers, in mockapp and real applications. Each result is checked against the same reads made by ordinary calls on the same element at once.
- A kind of program, defined from the builder (`crates/verbatim-uia-rops/src/batch/mod.rs` 9 to 28; `batch/program.rs`, `LIMIT` 10,000) so the check is mechanical [review 2 M3]:
  - each fusable step's emitter alone;
  - each pair of emitters the builder can fuse in one program, in each order;
  - each way a program ends: after an act (`Select`), after a walk down the tree, and at a split where the emitters' declared worst cases would pass the 10,000-instruction limit, for every emitter whose worst case can reach it;
  - each case of the fallback rule that reruns steps by calls after a program.
- A build check fails when the builder's emitter table allows a kind on that list that has no case in the suite (agreed by Dickson). So a new kind of program cannot reach the product without its proof. The limit's worst case runs in the suite, where it is also measured, since the ordinary-call path never meets the limit. A difference between a program and its ordinary calls becomes a fix to the ordinary-call path or the builder, or an entry in `books/dev/src/reference/uia-bugs.md` with the recorder taught to capture it.
- The simulator runs use the ordinary-call path, which the suite proves equivalent, serving one program's reads at one simulated instant, so a program cannot read across a world change in the simulator when it cannot in the product [review 2 M3]. They stay fast, need no UI Automation core, and run on the Linux job.

The recorder records the world, not query results: properties, the tree, text and their boundaries. So no program needs a recording of its own, and a program's result is computed against the replay. A program answers from the world only within the recording policy: one that walks further, or reads a property outside the recorded union, misses, and that miss is a world failure that widens the policy, as the recorder's rules give [review 2 S8]. The replay runs every program by the ordinary-call path, which the contract suite proves equivalent. The recorder uses remote operations itself only for facts only an extension reveals, such as Word's sentence boundaries through Word's extension, stored as boundaries, and it may snapshot the tree in remote batches for speed.

NVDA reads by classic calls and Verbatim by remote programs over the same world, so they can differ for a real reason: a remote program reads a snapshot, while NVDA's sequential reads can see a value change partway. These are their own rule class in the comparison, snapshot differences, not outcome entries, since a snapshot outcome is in none of NVDA's sequential outcomes [review 2 M2].

Calibration: each real application is recorded both by classic calls and by remote programs. Any disagreement becomes that application's recorded behaviour, stored as the property's `remote` value beside `plain` and `ex` so each access path is served its own value, or a known provider quirk in `books/dev/src/reference/uia-bugs.md` [review 2 S9].

NVDA's own remote paths are a separate question. The Office experiment tried two routes for them: Word's extension instructions implemented in NVDA's local executor (`Operation(localMode=True)`) against a text model, which ran NVDA's unchanged Word bytecode and sentence movement; and the real UI Automation core running NVDA's programs against a stand-in provider, which ran only stand-in extension calls. Since no real API is used in the replay (Dickson, 2026-11-28), NVDA's remote paths run in the replay through NVDA's local executor, which ran Word's sentence movement and needs no Windows session.

Office experiment, results (`experiments/nvda-oracle/office/`, reported 2026-11-28):

- Classic Word and Excel move into the oracle. NVDA's shipped `nvdaHelperRemote` ran `winword.cpp`'s three entry points (`getTextInRange`, `expandToLine`, `moveByLine`) and Excel's `getCellInfos` in the test process, against stand-in `IDispatch` object models served through `WM_GETOBJECT(OBJID_NATIVEOM)` on a never-shown `_WwG` or `EXCEL7` window. NVDA's real app modules and overlay classes produced speech: a heading with its page and section, line moves, the sentence unit, and Excel's cell states, cropped included, through real GDI measurement.
  - Stand-ins: Word 13 classes, about 380 lines (48 object and member pairs); Excel 12 classes, about 155; plumbing about 210.
  - Replay form: a model fed by recorded Word and Excel facts, not a table replaying `Invoke` calls, since calls change `Range` and `Selection`.
  - What is recorded from real Word: word, line (which depends on layout: the document, page width and font), and sentence boundaries; `Move` and `EndOf` edge rules at the final paragraph mark; `Information` values; font values over mixed ranges. Office is not installed on this machine, so recording needs a machine with Office.
- Word's remote-operation extensions, two routes [review 2 M3]:
  - Local bodies in NVDA's `LocalExecutor`, about 150 lines, ran NVDA's unchanged `msWord_textRange` bytecode and sentence movement over a text model with a sentence unit.
  - The real UI Automation core ran NVDA-built remote programs against a provider registered in the test process (`IRawElementProviderSimple` with `ICoreAutomationRemoteOperationExtensionProvider`; extension targets must be connection-bound objects, through `SetOperand2`). `IsExtensionSupported` and `CallExtension` were called on the provider's thread, 2.7 ms for the whole call, with no other process. What remains for Word's sentences on this path is a Python `ITextProvider` and `ITextRangeProvider`, about 25 methods.
- Licensing is unchanged: GPL binaries in a GPL-tier test tool only.

#### Math

NVDA takes MathML from the object and hands it to MathCAT (`mathPres/MathCAT`), a native library whose output depends only on the MathML and its preferences. NVDA's sources of MathML:
- Chromium: the IA2 `math` attribute, or `ISimpleDOMNode`'s data (`NVDAObjects/IAccessible/ia2Web.py` 387 to 420);
- Word over UI Automation: its custom property `word_mathml` (`NVDAObjects/UIA/wordDocument.py` 1030 to 1035, and 480);
- classic Word: MathType's own DLL converting the equation object (`mathType.py` 28 to 45).

So the recorder captures the MathML, and for classic Word the MathType equation data or its converted MathML. MathCAT runs as it is in the oracle, math navigation included, from the installed NVDA built for Python 3.13 (`speech.md`, finding 7). Verbatim's math is M14; when it is built, it is compared against NVDA's MathCAT speech on the same recorded MathML (Dickson, 2026-11-28).

#### Extensions

Extensions are replayable in the simulator but not compared with NVDA (Dickson, 2026-11-28).
- An extension host is a peer like any other. The Wasm guest is deterministic given its inputs: time and randomness are host imports, which the simulator serves.
- An extension runs over a world, like Verbatim: its host is a peer in the simulator, and its reads are answered through Core from the world, so a different path through the extension still gets answers [review 2 S11]. A recorded session's inputs (events, gestures) and outputs (requests, speech) are kept as expectations only, never replayed as a transcript.
- A run is verified by:
  - the extension API's contract checks;
  - invariants: it holds no ids beyond Core's held views, every request has one answer, and its calls are bounded;
  - transcripts Dickson approved, as expectations.
- Faults are injected: slow answers, and a host crash.
- Where a Verbatim extension ports an NVDA add-on's behaviour, the add-on itself can run in the headless NVDA on the same world, as an optional comparison.

#### Oracle experiments: time and threads, text and MSAA, browsers

Status: all three have results, reported 2026-11-28 (`experiments/nvda-oracle/time/`, `experiments/nvda-oracle/text/` and `experiments/nvda-oracle/browse/`).

Time and threads, results:

- The simulated clock works: `time.*`, `sleep`, `threading.Event` and `Timer`, `queue.get`, `Thread.start` and a simulated `wx`. `core.main`'s `CorePump` was compiled unchanged from `core.py`, so the pump needs no patched copy. Each clock read adds 1 µs of simulated time; 3,601 simulated seconds ran in 11 ms. Not reachable: `datetime.now`, real-time lock and `Condition` waits with a timeout, and native clocks; each is a miss if NVDA's speech path reaches it.
- The caret watch works through `inputCore.executeGesture` into `_hasCaretMoved`: the event path at 40 ms, the bookmark path at 80 ms, the timeout at 100 ms, and the old line at 150 ms, deterministically; 3.5 simulated seconds took about 150 ms.
- Threads: real OS threads run under a baton scheduler, in first-in, first-out or seeded order, with an option to preempt at every stand-in call. The terminal `LiveText` case showed NVDA's output can depend on thread order: with `STABILIZE_DELAY` at 0, of 13 orders 8 said "42" and 5 were silent; at 0.03, the console's real value (`NVDAObjects/window/winConsole.py` 23), all 13 said "42" (`out_threads.txt`) [review S8]. So at such points the verifier explores orders under NVDA's real constants, and the comparison applies its outcome rule (below).
- The watchdog works unchanged over a simulated waitable timer, `CoCancelCall` and `isHungAppWindow`: freeze recovery and its short path were reproduced. Model gap: only text-pattern calls can hang in the stand-ins so far.
- The native UI Automation limiter: driving the DLL with no gate is not deterministic, because its batch boundary is native thread timing. It was modelled in 32 lines. Dickson ruled (2026-11-28) that the gated DLL is the oracle and the model only a cross-check, and the saved run's mismatch (`out_rl.txt`: the gated DLL delivered three events, the model four) is rerun before either is trusted [review S11, U1]. Finding: the native limiter keeps the oldest of two duplicates, not the newest, because its erase goes through a reference to the overwritten entry (`rateLimitedEventHandler.cpp` 46 to 50); `handlers.md`'s reading said the newest. Verbatim's listener keeps the newer one, a ruled difference (Dickson, 2026-11-28, commit deaf7553) [review 2 M2]. The rerun uses the fixed `GetRuntimeId` stand-in, since every event's key depends on it (`eventRecord.h` 34 to 38) [review 2 S5].
- Other findings: a stale caret event queued during a hang counts as movement, so NVDA speaks the unmoved line at once; and `standins.py`'s `GetRuntimeId` had a latent bug, fixed in the experiment's copy.
- The recorder must also capture, from these runs: key injection times; the caret's change time and its caret event's time, separately; the application's UI thread stopping and resuming; calls that blocked; `GetGUIThreadInfo` answers; and each event's arrival time and sender runtime id.

Text and MSAA, results (`experiments/nvda-oracle/text/`, reported 2026-11-28):

- UI Automation text works over a real in-memory text model (`textmodel.py`, 340 lines): units, `Move`, `MoveEndpointByUnit`, `Compare`, `ExpandToEnclosingUnit`, `FindText`, attribute runs with mixed and not-supported values, and the text pattern 2 selection and caret range. NVDA's real review scripts and caret keys over a Notepad world matched all 25 steps of the live capture `target/captures-2026-11-20/s10-review-words-output/notepad.jsonl`, plus 13 rule steps, and format runs changed speech as expected.
- So the recorder captures a text model, not per-call answers: the full text, attribute runs for every attribute NVDA asks (`Culture`, `IsReadOnly`, `StyleId`, `AnnotationTypes`, `Link`, and font attributes where reported), the selection after each key, and the provider's word and line boundaries and end-of-text rule, as boundary lists or a named rule. The `text` line above is this model; the world server answers calls from it, and a provider rule the model lacks is a miss [review 2 S14].
- MSAA and IAccessible2 work: the IAccessible2 type library was built with `midl` from the IDL at NVDA's pinned commit (the `ia2` submodule is not checked out; the IDL was fetched from the public IAccessible2 repository), and NVDA's real `NVDAObjects.IAccessible` ran through `winEventCallback`, `orderedWinEventLimiter` and `pumpAll`. The Run dialog's capture matched except the dialog's description ("RunDLL"), which depends on dialog children no recording held; a UI Automation document under an MSAA frame matched. The `oleacc` stand-ins now cover every call, after one leak to the real `oleacc.dll` was closed, an instance of harness rule 4.
- The recorder captures, per window: class, text, style, control id, parent, process, thread, whether a UI Automation provider is present, the window and client objects' role, name, value, description, state bits, shortcut, default action, location and children, an edit control's text and selection, and the order of events.
- Remote operations: `UIARemote.initialize`'s patch was not needed. They change speech only in Word (sentence movement is silent without them, a heading's "collapsed" is not reported, page numbers come by a slower path) and in UI Automation browse mode's heading navigation, which falls back to walking paragraphs and finds the same first heading. So the classic path is a sound oracle outside Word.
- The comparison with the 2026-11-20 captures used `pre_speechQueued`, with eSpeak NG's command set on the recording synthesizer, because those captures were recorded there. The capture hooks are settled as harness rule 8 gives them: `pre_speech` for requested speech, since every caller passes through it, and `pre_synthSpeak` with the stand-in synthesizer for delivered speech. `pre_speechQueued` is kept only for comparing with the 2026-11-20 captures (Claude, 2026-11-28).

Browsers, results (`experiments/nvda-oracle/browse/`, `expB.out`, reported 2026-11-28):

- Loading NVDA's helper in the test process worked. NVDA's shipped `nvdaHelperRemote.dll` was loaded with no injection: NVDAHelper's controller callbacks were wired as `NVDAHelper.initialize` does, skipping `injection_initialize`, with `initInprocManagerThreadIfNeeded` for this process only, and pointed at stand-in IAccessible2 objects. NVDA's native `gecko_ia2` buffer builder, its native live-region code and Python browse mode ran whole, and gave page load, quick navigation by heading and table, reading a line, the switch to focus mode, the live region "Saved 3 files", and an incremental re-render.
- Two harness shims were needed: the live-region code's two checks of the foreground window and its visibility were redirected to the test window through NVDA's import-hook helper; and synchronous calls to the window's thread were made from a worker while the main thread pumps.
- Rules the stand-ins must keep, exposed by the run: `QueryService` answers only the browser's three services; the world keeps objects alive; the document's parent is the window's system object; text length is -1; and the display attribute decides line breaks.
- Recording at the helper's RPC boundary was rejected, because it replays only the one key sequence recorded.
- The recorder captures, for browsers, per object: MSAA and IAccessible2 roles and states, the full attribute string, name, value, description, shortcut, locale, text with embedded-object characters and attribute runs, hyperlinks, children, relations by type, table and cell extents and headers, and actions; per window: handle, class and toolkit; and events with their id, child id, the changed subtree and inserted live-region text.
- Limits: x64 only, since the shipped helper is loaded into the harness's own process. The harness loads GPL binaries in its own process, so it stays a GPL-tier tool, outside the product. Approved by Dickson (2026-11-28) (item 6, licensing): accepted for the test tooling.
- Consequence: in-process code is not automatically outside the oracle. Browse buffers run in it, not yet verified: the browser run also opened the live process 6364 (`browse/expB.out`), which the no-leak check must close first [review 2 M4].

### Verbatim under the simulator

The same world, served through `Platform` to the listener's and the outposts' executors. Every process's reducer runs in one thread, and Core's `Say` effects and decision log are collected. Invariant runs use any fault profile. Comparison runs use faults that both screen readers can meet in the same way, as world changes rather than process faults: event delays, elements gone between calls, hangs, and floods [review S9]. Each read takes the latency the world recorded for it, on both sides, rather than NVDA's 1 µs per clock read against Verbatim's own latency model, so the two read the snapshot of the same time. Limit: a defect that exists only because Verbatim reads later than NVDA would is still judged only by the invariants and the desktop suite, since the world holds one snapshot per sampling.

### Comparison

- The unit is a world step: a recorded step, or an event burst between two steps. For each step, the sequence of spoken strings, each with whether it cut speech, must be equal. The decision logs must have equal multisets of fingerprints.
- Where NVDA's output depends on thread order or batch boundaries, NVDA is run over the orders and boundaries the harness enumerates at that point, under NVDA's real constants, so every member of its outcome set is reachable in a real NVDA (the time and threads experiment) [review S8].
- Verbatim is deterministic for a given event order and gives one output (Dickson, 2026-11-28). The rule:
  - Where NVDA's explored set has one outcome, Verbatim must equal it.
  - Where it has several, an outcome rule decides which Verbatim must give [review 2 M2]. The form below is open (Dickson, 2026-11-28): it is revisited during implementation with worked examples on real cases. An outcome rule is a general predicate tied to a ruling, applying to any world: for example, "where NVDA's outcomes are silence and a text, Verbatim gives the text". It names the decision it applies to, the shape of NVDA's set it matches, the outcome Verbatim must give, and its ruling.
  - Per-step outcome entries (the world, the step, NVDA's set, the outcome, the ruling) are only for one-off cases no rule covers.
  - Generated worlds are judged by the rules alone. A generated seed whose NVDA set has several outcomes and matches no rule fails as an unruled race and is listed for a ruling.
  - The check fails if no rule or entry applies, if the chosen outcome is not in NVDA's set, or if Verbatim gives anything else. A change in NVDA's set that keeps the chosen outcome is reported, not failed, since the set depends on the seed budget.
  - So a silent member of NVDA's set never passes a mute Verbatim by itself: silence passes only where a rule or entry chose it under a ruling.
  - Choosing among NVDA's outcomes is a parity decision: matching NVDA needs none, a new rule or entry is listed in the report and pending until Dickson approves, as CLAUDE.md's "Who decides" gives for differences.
  - The set, and which order gave each member, is in the report. Enumeration is bounded by the seed budget, and a point whose orders exceed it is listed, not sampled silently.
- The schedules a comparison uses [review 2 M2]: NVDA over every thread order and plausible batch boundary the harness enumerates at the step; Verbatim over the recorded arrival order and every order between pipes the simulator explores at that step within the seed budget. Every explored Verbatim schedule must give the outcome the rule or entry requires, so Verbatim's own order dependence is judged too.
- Snapshot differences, their own rule class [review 2 M2]: where Verbatim read by one remote program and NVDA by sequential reads across a world change, the step is judged by running NVDA's sequence against the same world with its reads served at one instant, the instant of Verbatim's program. Verbatim must equal NVDA's outcome there; or a ruling covers the difference as a general rule.
- Two streams are compared: Verbatim's `Say` effects against NVDA's requested speech, and Verbatim's speech thread's output, with its cancels, against NVDA's delivered speech.
- The allowlist is structured data, one entry per allowed difference:

  ```toml
  [[difference]]
  ruling = "docs/parity/<file>.md#<anchor>"
  status = "approved"          # or "pending", for a difference not yet ruled (CLAUDE.md, "Who decides")
  world = "worlds/explorer-01.jsonl"
  step = "s04"                # the world step the difference is at
  count = 1                    # how many times it occurs there
  match = { decision = "focus.title", nvda = "...", verbatim = "..." }
  ```

  - An entry pins its world file, step and count, never a tag alone, so one entry cannot hide every difference of a decision across many worlds; a change in the count fails [review S10].
  - A ruled systematic difference is a general rule instead, `[[difference_rule]]`, matching by decision and the shape of the difference across every world, with its ruling, for example the limiter's keep-newer coalescing (Dickson, 2026-11-28, commit deaf7553, `docs/parity/backends.md`). A general rule never matches a difference of another decision, and one that matches nothing fails as stale [review 2 M2].

  - An entry whose ruling anchor does not resolve fails the build.
  - An entry that matches no difference over the corpus fails as stale, so the list only tracks rulings.
  - A pending entry is listed in every report until Dickson rules on it.
- Where Verbatim deliberately differs over a whole behaviour, such as the terminal's row memory and flood policy, its oracle is a reference model, not NVDA. A separate agent writes it from the rulings only, never from the implementation. It is checked by invariants and by transcripts Dickson approved.
- Approved by Dickson (2026-11-28) (items 6 and 7): the comparison gates `cargo xtask ci`. It needs Python 3.13, comtypes and an NVDA build's `lib` folder on the runners, since `NVDA_LIB` points at an installed NVDA today.
  - This is a different tool from `cargo xtask nvda compare`, which runs live NVDA and stays exploration only.
  - I recommend gating on the corpus of recorded worlds once the copy check passes (Claude, 2026-11-28).

### Line coverage and generated inputs

- NVDA runs under `coverage.py` across the corpus. The lines not run, in the functions the parity ledger lists for each ported behaviour and in replay classes R1 to R4 plus the R5 code the harness loads (the browse buffers and live regions), are the gap list. R6 lines and the R5 code not yet loaded are left out of it, and are verified as "In-process code, and what stays outside the oracle" says. Each gap is closed by a recording or a generator, or recorded as out of scope. The covered set ratchets: a run that covers fewer of those lines fails.
- Generated worlds come from a model of real application behaviour, taken from recordings and `books/dev/src/reference/uia-bugs.md`:
  - The order of focus against property events.
  - Delays between a change and its event.
  - Duplicate, late and second-identity focus events.
  - Elements destroyed between calls.
  - Flood rates.
  - Hangs.

  The oracle for every generated world is NVDA's own code, never Verbatim's design, as the memory note "Generators need an independent reference" requires. Every seed is checked.

### Three kinds of test

Dickson (2026-11-28): the replay uses no real API, so real API behaviour is tested by a kind of test of its own. There are three kinds, each catching what the others cannot.

- Replay tests: Verbatim's logic, under the seeded simulator, over recorded and generated worlds, compared with headless NVDA. They catch logic defects, races in Verbatim's own processes, departures from NVDA's speech, and leaks, across far more worlds and seeds than a desktop can run. They cannot catch a recording's wrong assumption about a real API.
- Live API tests: Verbatim's platform layer, the `Platform` implementations calling UI Automation classic and remote operations, MSAA, IAccessible2 and the Java Access Bridge when Verbatim supports them, and Windows, run against real applications and mockapp on a desktop, one targeted call or behaviour per test. They check returned values, failures, timeouts, defaults, event timing and order, hangs, COM releases, remote programs on the real core against real providers, and application quirks. A difference from what recordings assume becomes a recorder fix or an entry in `books/dev/src/reference/uia-bugs.md`. The remote-program contract suite is of this kind. Each test launches the real application or mockapp in a desktop session and needs no Verbatim running; they run on GitHub's Windows runners as well as locally, under the desktop rules, one agent holding the desktop.
- End-to-end scenarios: the whole of Verbatim on a real desktop with real applications, as today, a full layer, not a small set. They catch what only a live run shows. The history has several: the title bar served by the MSAA proxy hiding a remote-program import failure (7f7bebb6); UI Automation events arriving before their subscriptions (4008991b); Windows Terminal's second identity (1db3e261); Explorer's list-item focus that was never delivered (790f334c); and the stalled GUI thread (e69c6613).

An option for Dickson to decide later, not decided: a real-use loop, the analogue of the shadow-traffic and real-server loops in the rewrites researched (Anthropic's slow production loop, which caught what the sandbox missed; Dropbox's suite against real servers, kept because mocks drift). Dickson's own day-to-day use of Verbatim, with the trace always on and world capture switched on, on his machine, only when he turns it on. Each divergence or defect it finds becomes a recorded world and a permanent replay case.

The three are tied together:
- For each family, one scenario is also run with world capture on, with Core not speaking, and replayed. Live Verbatim's speech, from a separate run, must equal the replayed Verbatim's speech. A difference is a defect in the world server, the reaction model or the capture, filed before anything else.
- Headless NVDA on a recorded world must equal live NVDA's capture of the same steps. That is the copy check above.
- A live API test that finds an application behaving as no recording assumed adds that behaviour to the recorder or the generator's model, so the replay meets it from then on.

### The measurement gate

Before anything beyond the focus slice is invested in, in the product or the verification, the verifier is measured on bugs it should catch. A verifier that matches captures but misses real defects would be costly ceremony, so the rate decides.

The corpus is built and held out before the verifier is tuned [review M6]. A separate agent, which has not seen the verifier's code and does not take part in building it, mines the bugs and writes the mutants while build steps 1 to 4 run, and seals them: the mutants stay in its own worktree, and only a hash of each is recorded in the scratchpad, so nobody building the verifier sees them or can tune a world to them. It builds:

- Verbatim's historical bugs, from three sources:
  - Fix commits in git: commits whose message or added regression test names a wrong, missing or extra utterance, a lost answer, a leak or a hang. Each is read with its diff and its test.
  - The pattern audit's kept findings (`patterns.md`), each with its class.
  - `docs/flaky-tests.md` entries whose cause was named in product or harness code.
- Each bug is classed in scope (its behaviour is in the focus slice: focus, foreground, ancestry and the line, duplicate and late focus events, an outpost generation ending, an application exiting, a hang during a focus read) or out of scope. Only in-scope bugs count toward the rate; the out-of-scope count is reported as the corpus's reach.
- Each in-scope bug is re-expressed as a mutant of the new skeleton: the same defect in the new code, written from the bug's description, its diff and its symptom, such as "the line's ids left out of the held view after the commit", or "the duplicate test reads the last focus's token after the view released it". Each mutant states the symptom users had. A mutant that changes no behaviour on any world is called equivalent by its author, excluded and listed.
- Harness defects, re-introduced blind [review M6]: the same agent writes harness mutants in the style of the defects found (`MagicMock` stubs, replaced pure helpers, a call reaching the live machine, a partial pump, a capture that joins strings, a synthesizer without its facts, missing data, the limiter off, unfiltered delivery, import-bound names, a weakly held handler, a stand-in bug like `GetRuntimeId`'s), including new instances the inventories did not list, without telling the harness's builders which or how many.
- Size: 60 in-scope Verbatim mutants that compile and pass the checks, split at random into 40 for the measurement and 20 held back for a second one (estimate: the yield of the mining is unknown until it is done). If fewer exist, the count and the resulting confidence are reported, and Dickson decides whether the gate still decides.

The run: each mutant over the corpus of recorded and generated worlds, with the comparison and the invariants, and a seed sweep of at most 20 seeds per world for invariant runs.

- A catch is behavioural only: a comparison difference not on the allowlist, an invariant violation, or a miss (as the recorder's rules define it). A compile or check failure is not a catch. Mutants the skeleton prevents by construction are reported apart, as "prevented", and do not count toward the rate [review M6].
- The report gives, per mutant, what caught it and at which seed, and for each miss its class: no world exercises it, the oracle cannot see it (R6, or R5 not yet loaded), or no invariant names it.
- The unmutated build must give no difference outside the allowlist and no miss, so a catch is not noise.

What the rate decides (thresholds proposed, Claude, 2026-11-28; Decision for Dickson, item 5, since they decide scope). The rule is the 95 percent Wilson interval on the catch rate, against 65 percent:

- Proceed when the interval's lower bound is at or above 65 percent: 32 or more of 40, an observed rate of 80 percent, whose interval is about 65 to 90 percent.
- Stop when the upper bound is below 65 percent: 20 or fewer of 40. The verification's investment beyond the slice stops, and the report goes to Dickson with the misses by class.
- Between, 21 to 31 of 40: the misses' classes are fixed (a world, a generator, an invariant), and the rate is measured on the 20 held-back mutants, never on the mutants the fixes were written for. Proceeding then needs 18 or more of 20, which puts the lower bound at about 70 percent.
- Harness defects: the report names which of them the copy check alone catches, since the prototype matched two live captures while carrying all twelve known defects, so the copy check alone was not enough. Each blind defect not caught by the harness's checks adds a check, and the blind set is measured again with fresh mutants.
- The comparison: for each caught bug, what caught it originally (a review, an end-to-end scenario, GitHub's run, Dickson), so the report shows whether the verifier catches what the desktop suite missed.

Scope: passing the gate on the focus slice licenses the verification for the focus slice only [review M6]. The audit's costly areas, the terminal, caret keys, dialogs and say-all, each pass their own gate, with their own held-out corpus, before the verification is relied on there or grown for them.

## The trace

Today (read at 671aa993): the flight recorder covers Core only. It keeps an always-on, bounded window of Core's reducer inputs in memory, bounded by entry count and by an estimate of their bytes, in segments that each start with a state snapshot, so the window always starts at a checkpoint (`crates/verbatim-core/src/flight_recorder.rs`). A dump is written to a `dumps` folder beside the executable on demand from the control plane, or just before the process dies of a panic (`crates/verbatim-app/src/flight_dump.rs`), and `cargo xtask replay` replays it deterministically against the reducer.

In the redesign (Dickson, 2026-11-28) it becomes the trace, the first level of the one recorder ("World capture"):
- Every process's root reducer records its own inputs: the messages it received, its workers' completions with the application's answers, callback events, and the times of each. So a real failure replays across all processes together in the simulator, not Core alone.
- The completions make a dump a partial world: everything Verbatim asked, as the application answered. What Verbatim never asked is missing. So a trace dump replays Verbatim, all its processes, but cannot be replayed onto NVDA: NVDA's extra reads would be misses. Comparing a moment of real use with NVDA needs one of two routes:
  - world capture, the recorder's second level, switched on in the real-use loop or for a scenario; its cost is measured as its section gives.
- The format follows the new protocol: the three message kinds, held views, and ids tagged with their generation.
- Old files, two kinds (Dickson, 2026-11-28):
  - Ad-hoc trace dumps keep the standing rule: no compatibility code for old dumps.
  - The test corpus (world captures, trace dumps kept as permanent replay tests, and NVDA captures) must stay readable, since the verification relies on it. Every format change ships a one-way converter, run once over the stored corpus, and the upgraded files are committed. A CI check loads every stored recording with the current format and fails on any it cannot read. The code reads only the current format.
- Each process's ring keeps its checkpoint rule (a snapshot per half window), and a dump's window starts at the oldest unfinished entry's checkpoint, as async rule 6 gives.
- The ring's memory and its time per message are measured at the worst case before it is on by default in the readers.
- A dump becomes a permanent replay test at once: its partial world joins the corpus, and the replay must reproduce the failure until the fix makes it pass.

## Checks from the first commit

- The ledger (`xtask/src/ledger.rs`):
  - Every root (Core, each reader's root, the supervisor, each host and each shell thread's root) is listed.
  - The walker follows field types into other workspace crates. Today it follows only the root's own crate, which the audit found.
  - No `known-copy` is allowed for the new roots.
- The ratchet (`xtask/src/shared_state.rs`): the logic crates and Core start at zero atomics, cells, locks or once-values, with one listed exception. The shells' few are listed, and counts only fall. How the mechanism fits it [review S4]:
  - Tickets need no interior mutability. An entry's ticket lives in the driver's own slot for that entry, so when the driver drops an entry's future it ends the ticket itself, in the same step. A record's ticket is a plain `#[must_use]` value, neither `Clone` nor `Copy`, and a record's transition that replaces or drops one returns a `Cancel` effect. A ticket dropped any other way is caught by the simulator's invariant that every `Out` record has a live owner.
  - The queue's futures are stored in the driver beside `Core`, not inside it, and are polled with `&mut` access, so no `RefCell` is needed.
  - The one exception is the scoped thread-local that gives `cx.apply` its `&mut Core` while the driver polls: `scoped-tls` holds a `Cell` inside. It is listed by name in the ratchet for the driver module alone. Today's driver uses `shared.borrow_mut()` (`inbox.rs` 601).
- Sealing, a new check beside the ledger:
  - Every field of a type the ledger reaches is private.
  - An enum that carries data is private to its module, wrapped by a sealed struct.
  - Each record module has at most one `fixture`.
  - No struct literal of a ledger type appears outside its module. The compiler already enforces this through private fields, and the check stops `pub(crate)` from reopening it.
  - Message types in the model crate are exempt, since they are parsed into records at the boundary.
- The `!Send` rule:
  - Every ledger type in Core carries the `NotSend` marker. The check reads the field list the ledger already parses.
  - The queue's future bound is `Send`.
  - A `compile_fail` doctest holds a fact across a wait and must fail to compile. That uses the standard library's doctests, with no dependency.
- Stage names: the stage macro's list per feature, and dumps print every paused entry's stage.
- Tiers (`xtask/src/tiers.rs`): the model crate and Core are platform-neutral. The logic crates holding the executors and reader reducers are GPL logic, with no Windows crate. The shells are Windows GPL. The world server is platform-neutral. The NVDA harness is test tooling, outside the product.
- The existing checks stay: `waits.rs` (no sleeps in tests), the documentation check, Clippy pedantic, and the determinism `clippy.toml` extended to the logic crates.

## How each pattern class is closed

These are the classes in `patterns.md`:

- Copies: one record per owner. Later stages hold earlier ones. Fact types are `!Send` and entry types are not `Clone`. The ledger allows no known copy for new roots, and the world-change invariant catches a primitive carried across a wait.
- Flag lifecycles: each lifecycle is one private enum in a sealed record (`Serving`, `WorkerSlot`, `Request`, `WatchState`). The sealing check rejects public fields, through which flags were set from outside.
- Owner state outside its owner's record: the owner's record holds it, and its end drops it. No clean-up function exists to forget. The audit's ten hand-written clean-up functions have no counterpart.
- Per-feature messages: there are three message kinds. A feature adds steps only from the shared list, a change that is Dickson's item 4.
- Test literals: one fixture per record and private fields, so no test builds a record field by field. The audit's 22 test files touched by one copied field have no counterpart.

## Adding a feature

The files a feature touches, in general:

- Its rule module in Core, with its entry kind if it reacts to an event, and its record if it keeps state.
- The properties it reads, each declared once in the model's property table.
- Its strings in `verbatim-i18n`.
- Its tests: a record test from the fixtures, and a world in the corpus with NVDA's speech as the oracle.
- Its parity entry.

No message, no reader record, and no test fixture of another record changes.

### Worked example: speak a UI Automation toggle button's pressed state

NVDA says "More options toggle button pressed" when the button's toggle state is on. That is the prototype's sensitivity run (`out.txt`, line 7).

Files:

- The model: the `ToggleToggleState` property in the property table, if it is not declared there.
- The outpost's UI Automation parse of the property into `State::Pressed`, in the one parse at the boundary.
- Core's announcement module: the role rule, which makes a toggle button with a toggle state say "toggle button", and the state's place in `STATE_ORDER`.
- `verbatim-i18n`: the `.ftl` string for "pressed", and the message id.
- Tests: one announcement test from the `Node` fixture. Plus a world: the Explorer world with the toggle state set, its oracle being NVDA's run.

Counted as written, that is seven files: the property table, the outpost's parse, the announcement module, the `.ftl` string, the message id (a second file, `verbatim-i18n/src/lib.rs`), the test and the world; nine with the parity entry and `test-coverage.md`, which CLAUDE.md requires. e812a792 touched ten, eight without documents, so the like-for-like saving is about one file, not three [review S13]. The verification adds no code: the world is a data file, and the comparison runs it.

### Worked example: speak the focus's new name when it changes

NVDA's `event_nameChange` speaks the name when the object is the focus (`NVDAObjects/__init__.py` 1409 to 1411).

Files:

- The focus feature's follow property set: `Name` is added to the properties the `Follow` step registers. It is data in the focus module, and no step is added.
- A new entry kind, `NameChanged`, in its own module in Core:
  - `arrive` merges with a waiting `NameChanged` of the same node, if the parity entry says NVDA does.
  - `foreground_changed` keeps the entry.
  - `run` reads `cx.read(|c| c.versions.is_committed(node))`, which returns a `Send` bool used before any wait. It then says the name through the announcement module's name-only function, and needs no read, since the item carries the value.
- The queue's entry enum: one arm.
- Tests: the entry's record test, plus a world in which an application renames its focused button. The world is recorded from mockapp or generated from the application model, with NVDA as the oracle.
- The parity entry.

That is about nine files (estimate), with the stage list, the decision-point table and the module's registration counted, and the documents [review S13]. No message, reader or record of another feature changes, and the committed focus already names the node.

## The cost of a feature

Dickson's decisions of 2026-11-28 on what a feature should cost to add, beyond the skeleton's own rules ("Adding a feature").

### Properties

- The existing table, `PropertyId` in `crates/verbatim-model/src/property.rs`, stays the one declaration of every property.
- Core reads an element into one struct generated from the table, one field per property. Each field is given, absent, or not asked, three states of one type, so no field has a valid-looking default.
- Reads are driven by the table. There are no hand-written read shapes such as today's UI Automation ancestor shape (`crates/verbatim-uia/src/shape/shapes.rs`).
- Tests build the struct through one constructor, never field by field.
- So adding a property is a table line, plus its rule and its test.
- Why: commit 8ce48d3e, which added one fact (whether an ancestor has a description), touched 29 files. Six were documents. Fourteen were in Core, nine of them tests that only added the new field to structs they build by hand (`described: false`). Two were mockapp's tests and fixture. The other seven were the model's request and tree types, the UI Automation client and its ancestor shape, the outpost's request, the IAccessible2 acquire, and a test seam in the outpost logic.

### Phrases

- The English source file is generated from phrases defined where they are spoken: `msg!` with a key and the English text, its placeholders checked at compile time.
- Other languages keep their own translated Fluent files, maintained by translators.
- The build checks each language's file against the English keys, failing on a missing or an unused key.

### Expectations

- Each world's expectation file stores two outputs: NVDA's, generated by the oracle, and Verbatim's expected output, which equals NVDA's except where a ruling differs, citing that ruling.
- Checks:
  - Verbatim matches its expectation, on any host, Linux included, since it needs no NVDA.
  - NVDA's stored output matches the oracle, on Windows, where the oracle runs.
  - Any step where the two stored outputs differ cites a ruling.
- This changes `books/dev/src/reference/testing.md`, which is Dickson's (CLAUDE.md, "Who decides", item 7); he took this decision, and the wording is his to approve.

### Not now

- Reference sections generated from the code: not now, revisited later (Dickson, 2026-11-28).

### The next design task

The text interface's design, Core's and the extensions', comes next, before the prototype grows past the focus slice.

## The roadmap's later work in the skeleton

For each milestone: where it fits, what verification covers, what in the skeleton would change, and any conflict.

### M5, extensions

- Where it fits:
  - A host is a peer served by `Serve(Role::ExtensionHost(id))`.
  - Core calls every hook, and reads go through Core (ADR 0003, "Extensions", Dickson, 2026-11-12).
  - A hook call is a `Batch` to the host with one host step, `Call { hook, lane, args, deadline }`. Its answer streams `Effect` items (speech, tones), and ends either with `Final(Result(Verdict))` or with `Final(Result(Wants(batch)))`.
- Straight-line code:
  - In the guest, `node.name()` or `focus()` is an ordinary blocking import. The host's lane suspends the guest and answers the call with `Wants`.
  - Core's entry is the loop that makes this straight-line on both sides:

    ```rust
    loop { match cx.ask(stage!("ext.call"), host, call).await { Wants(b) => { let r = cx.ask(stage!("ext.read"), outpost, checked(b)?).await; call = resume(r) } Verdict(v) => break v } }
    ```

  - The guest writes straight-line code, Core's entry is straight-line, and there is one read outstanding per extension, as ruled.
- What it may ask:
  - A batch from the shared step list. Core parses it at the boundary into a `CheckedBatch`, against the component's derived capabilities and its application's boundary. A step outside them is refused, never sent.
  - The nodes it holds are named by the lane's record in Core, so the held view keeps them until the instance ends or the outpost does.
- Rules as data: the listener's part rides in the `Watch` filters, so a new rule generation is a new watch. Every other rule is applied in Core to answers.
- Verification: NVDA's app modules load in the headless harness by executable name, through the replaced `getAppNameFromProcessID`. So a ported built-in is compared with NVDA's own app module over the same world. This makes the porting track a corpus check. Third-party extensions themselves are not compared with NVDA; they are replayed in the simulator and verified as the section "Extensions" under verification gives, with an NVDA add-on run headless as an optional comparison where an extension ports one.
- What changes in the skeleton: the host step kinds `Call` and `Resume`, and the host's `Wants` item.
- Conflicts and questions:
  - The extensions design (`docs/design/extensions.md`, section 4) predates ADR 0003's extensions ruling: there the outpost's reducer calls hooks and a host reads from the outpost directly. The skeleton follows ADR 0003, and that document's section 4 and its two tables would be replaced.
  - A suspended guest must belong to a request [review S5]. As first written, `Wants` was a `Final`, so between it and Core's next request the lane held a paused Wasm store that no request named, and an entry ended there had nothing to cancel: a waiter with no ending. So `Call` opens a standing request on the host, whose items are `Effect` and `Wants(batch)` and whose `Final` is the verdict. Core answers a `Wants` with a new one-shot `Resume { call, results }` step naming the open call. Ending the entry cancels the call's standing request, which ends the lane's suspended guest. The straight-line loop above is unchanged in shape: it awaits the call's next item rather than a `Final`.
  - Approved by Dickson (2026-11-28): a hook call as a standing host step with `Wants` items and a `Resume` step, rather than a fourth ask kind.

### M6, the synthesizer host

- Where it fits: a host served by the supervisor. Its asker is the speech thread's root, not Core.
  - `Speak` becomes a standing request, whose items are marks and PCM chunks and whose final outcome is done or cancelled.
  - Cancel is `Cancel`. "Every `Speak` ends once" (the M6 plan) is the protocol's single ending.
- Verification: the M6 plan's generated tests against a model of the pair become simulator runs with the host's pure command module as its process.
- What changes: the protocol's types are shared by a second asker.
- Tentatively yes (Dickson, 2026-11-28, items 3 and 4), to be revisited in M6: the speech thread's pipe to the host joins the generic messages. The C interface `verbatim_synth_v1` is a public contract and does not change.

### M6.1, hung and flooding applications

- Hung calls: a job that never completes, abandoned by Core's `Cancel`. The ghost window is world data that a recording captures.
- Floods:
  - The listener's `ListenedApp.coalesce` keys waiting items by the keys Core's filters name, as NVDA's UI Automation limiter does (`rateLimitedEventHandler.cpp` 30 to 97, ruled 2026-11-19).
  - Items beyond a credit Core grants are held there. Credit rides on every message Core sends, and foreground and focus tells are exempt, as the protocol section gives [review S1].
- Verification: the simulator's flood fault at the measured rates, against NVDA with its limiter on, as harness rule 11 requires. The oracle is the real limiter DLL with its flusher gated, the harness choosing the batch boundaries and exploring the plausible ones (Dickson, 2026-11-28); the 32-line model is a cross-check only. The saved run (`nvda-oracle/time/out_rl.txt`) shows the gated DLL delivering three events and the model four, and is older than the model's last edit, so it is rerun before either is trusted [review S11, U1]. That the limiter keeps the oldest duplicate is confirmed by its source: `rateLimitedEventHandler.cpp` 46 to 50 erases through a structured binding that aliases the map entry just overwritten. Verbatim keeps the newer, a ruled difference, kept (Dickson, 2026-11-28, commit deaf7553, `docs/parity/backends.md`), entered in the allowlist as a general rule [review 2 M2].
- Follow items are bounded the same way: the outpost's `Following.coalesce` holds items beyond the credit Core grants, and Core's messages grant more [features B, addition 7].
- What changes: credit in `WatchFilters` and on `Follow`, approved by Dickson (2026-11-28).

### M7, the in-process helper and browse mode

- Where it fits: the helper is part of its outpost, which matches the roadmap ("Found for later milestones", M7).
  - The outpost's `Workers` gains `helper: HelperSlot`, either `Absent(reason) | Loading | Ready { helper_gen, channel }`. Absence is a fact of the application with its reason, so a request the helper would serve is answered another way or `Refused(Unavailable)`.
  - The helper's objects are `Element`s with `Backing::Helper { helper_gen, id }`. When its channel closes, every element with that `helper_gen` goes. Core's records may still name them, and any later ask naming them is answered `NotThere`.
  - This is the "held objects with their own generation" the roadmap asks for, ending with the channel, not at the outpost's replacement.
  - The helper's posted work cannot be withdrawn, so it is a job that may never complete. A stop drops only the answer.
- Buffers: a buffer kept in the helper is a helper-held object that Core's document record names, released when the document leaves. A buffer kept in Core is a fact record, with the rope and persistent map the roadmap names.
- A second lane is a second `WorkerSlot` in the outpost.
- Verification: the recorder needs IAccessible2 snapshots and browser documents. A helper's in-process reads are recorded as the objects they read, and the world server answers them on a "helper" interface. NVDA's virtual buffers are native code, and the browsers experiment ran NVDA's own buffer builder over stand-in IAccessible2 objects by loading the shipped `nvdaHelperRemote.dll` into the harness's process. So browse mode's parity is a corpus check against NVDA's own buffer: quick navigation, line reading, focus-mode switching and live regions. The recorder captures browser documents as that experiment lists. Limits: x64 only; Chromium in UI Automation mode goes through UI Automation browse mode, which the UI Automation oracle covers.
- What changes: nothing in the protocol. `Backing` gains a variant.
- The open M7 questions in the roadmap stay Dickson's: where the reading position lives, the second lane, and the helper's trust boundary.

### M8, the secure desktop (M8.7)

- Where it fits: the secure instance is a whole second skeleton, with its own Core, listener, outposts and supervisor, and no control plane. Nothing is shared, so the skeleton needs no change. Desktop switches are `Watch` items. The input desktop and the lock state are Core's `Foreground` facts.
- Verification: a recording of the secure desktop needs the recorder to run with UIAccess there. Approved by Dickson (2026-11-28) (item 2, a trust boundary).

### M12, Remote

- Where it fits: a remote key is injected and enters through the keyboard hook like a local one (roadmap, "Found for later milestones", M12). Speech mirroring taps Core's `Say` effects after the speech manager's fence, in the shell. The session is a record in Core's `Features`, its transport a shell thread of its own. It is not a peer, since Core asks it nothing.
- Verification: the simulator injects remote keys as key inputs. Mirrored speech equals local speech, an invariant.
- What changes: nothing in the protocol. The question of whether an injected key's origin can be proven is already Dickson's (trust).

### M13, applications' object models

- Where it fits:
  - An object-model handle is an `Element` with `Backing::ObjectModel { source, id }`, named by a Core record like any other.
  - A named program run whole in the outpost is a step. Whether each program is its own step kind, or one `Program(name)` step, is Dickson's item 4.
  - An application's own event source, such as PowerPoint's sink, is a standing request whose registration the root re-issues as a job when the worker is replaced, since the standing request outlives the worker generation.
- Verification: the recorder cannot capture Word's, Excel's or Visual Studio's object models in general. Each would need its own recorder and stand-ins on the NVDA side, which are large (not estimated). Until then these modules rest on the desktop suite and captures.
- What changes: a `Backing` variant, and re-registration on a worker's replacement.

### M18, braille

- Where it fits:
  - Braille output is a Core effect, `Show(region)`, to the braille driver's host, a peer served by the supervisor.
  - The display's state (what is shown and its anchors) is a Core record. The anchors are named by that record while shown, so the held view keeps them until the region is replaced, as the roadmap's "minting an anchor held while shown" asks.
  - Display gestures are items of a standing request Core opens on the driver host, so the driver tells nothing unasked.
- Verification: the headless harness replaces the `braille` stub with NVDA's braille handler and its no-display driver, and compares regions and cursor position per step, as speech is compared.
- What changes: a second output effect. How a braille gesture is numbered against the keyboard's keys stays M18's open question, Dickson's.

### Profiles (M8.3) and the highlight (M8.4)

- The profile stack is a Core fact. The switch is a stage of the focus entry, and a read carries its settings in the request, which the skeleton allows.
- The highlight reads only rectangles that answers carry, and needs nothing new.

## Sizes

Today, measured with `wc -l` at 7e981e21:

- Product: 224,149 lines, inline tests included.
  - Core: 68,649.
  - Model: 16,083.
  - Outpost and its logic: 19,104.
  - Listener and its logic: 11,674.
  - Supervisor: 4,047.
- Tests in `tests` folders: Core 21,496, mockapp 38,470, outpost logic 3,280 and supervisor 3,351.
- Tools: `verbatim-e2e` 28,866, `verbatim-agent` 11,633, mockapp 10,721.

NVDA, measured in the pinned source:

- 188,358 Python lines.
- 144,392 without its app modules, global plugins, synthesizer drivers, braille display drivers and GUI.
- The focus slice's functions: 1,030 lines (the slice design, "Lines").

The skeleton with the focus slice, all estimates:

| Part | Lines |
|---|---|
| Model: messages, steps, tells, held views | 900 |
| Core: peer and application records, queue, async driver, stages, tickets, end causes, the plain answerer | 3,000 |
| Core: the focus slice (judging, line, announcement, foreground) | 2,300 |
| Outpost: root, workers, executor generic over `Platform`, `Withdrawn` | 2,400 |
| Listener: root, `ListenedApp`, pending foreground, executor | 1,950 |
| Supervisor, with spares kept apart | 2,000 |
| Product total | about 12,550 |
| Unit and record tests | 4,000 |

The first revision's 9,900 was too low against today's code [review S14]: today's `event_queue` alone is 4,036 lines without tests (measured with `wc -l` at 671aa993), and the queue, driver and records here carry record-owned requests, held views, end causes and four hooks besides; and today's supervisor is 4,047 lines, of which launching, job objects, backoff and spares remain. So Core's machinery is put at 3,000 and the supervisor at 2,000, which makes about 12,550, about 3,650 more than the slice design's 8,900: the `Platform` and `Io` traits, the views' `NamesIds` check, the inventories' additions the slice needs (tickets and record owners, end causes, the plain answerer, the pending foreground, spares), and the review's (`Withdrawn`, the late-answer rule, the fourth hook). The other additions (`Request::Waiting`, follow coalescing, `Destroyed`, `Barrier`, the edit-control and tree-view calls) land with the features that need them. The process machinery has no counterpart in NVDA. Core's rules for the slice are about 2 times NVDA's lines, and the whole is about 12 times, against about 15 times for today's slice. Neither meets the 1.3 times target, because of the process split, which is ruled.

Verification tooling, all estimates:

| Part | Lines |
|---|---|
| World capture in the readers: added lines, on top of the 3,134 reused from `verbatim-event-record` | 3,300 |
| The trace in every process's reducer, its dumps and their replay across processes | 700 |
| World format, server with text models, C interface | 2,700 |
| Live API tests of the platform layer, with the remote-program contract suite and its build check over the builder's kinds | 1,500 |
| World capture's hooks inside recorded applications: GDI, IMM and TSF | 800 |
| List-view rebuild with its read-back check, and the display model's redraw | 600 |
| Word and Excel fact models (about 745 lines measured in the experiment) and their recorder | 1,200 |
| Simulator: scheduler, choices, shrinking, invariants, world-change mode | 2,500 |
| Comparison, fingerprints, allowlist, outcome rules and entries, snapshot rule | 1,300 |
| NVDA harness in Python for the slice: strict stand-ins, clock and baton scheduler, full pump, synthesizer stand-in, both capture streams, the gated limiter with its primer and boundary exploration, the no-leak check, registration filter, live reads | 4,400 |
| The measurement gate: corpus tooling, mutant runner, report | 500 |
| xtask checks: sealing, `!Send`, stages, allowlist and coverage | 600 |
| Total | about 20,100 |

The experiments' own code is the start of the harness line, not extra: 11,379 lines of Python across 65 files (counted with `wc -l` after the native and Office experiments, generated code excluded), with three copies of the stand-ins at about 660 lines each, which merge [review 2 S13]. The product needs no line for moving the program generator, since `Platform` is cut above it. The stand-ins beyond the slice, where the R2 class's roughly 37,600 lines of NVDA wait, are not estimated; they grow with the features that need them, and the measurement gate decides whether they are built [review S14].

## Build order for the prototype on the focus slice

Each step lists what it proves. It is to be built apart from today's product, as the slice design's "Building it apart from today's product" says: `verbatim-next-*` crates, its own executable, pipe and mutex names, never run beside the product.

1. The checks and empty roots, with the ledger, sealing, ratchet, `!Send` rule and stage list. The `compile_fail` doctest fails as it must. Proves: the checks bite before there is code to bend.
2. The model's messages and their parse at the boundary, with fixtures. Proves: three kinds are enough for the slice's three sequences, and malformed input is refused.
3. Core's driver, queue, `Out` records and held views, against scripted fake peers. Proves: single ending by construction, held views computed from the records with no stored list, stage names in dumps, and replay from a checkpoint.
4. The simulator's scheduler, seeds, choices and invariants over Core with fake peers. Proves: determinism, shrinking, and the exactly-one-ending invariant under crashes and hangs.
5. The world server and `Platform`, and the listener's and outpost's reducers with their executors under the simulator, on the Explorer world converted from `world_explorer.py`. Proves: the readers' logic runs with no Windows, and held views release every element at quiet.
6. The NVDA harness on the same world through the world server, under the harness rules: strict stand-ins, the simulated clock and baton scheduler, the full pump, the stand-in synthesizer, both capture streams and the limiter on. The comparison is run for the Explorer steps. Proves: the pipeline end to end, with both captures matched by both screen readers, and the duplicate and late-event decisions matched by fingerprint.
7. World capture. Verbatim's readers capture mockapp's UI Automation dialog and the Explorer steps, with Core not speaking, then checks the NVDA copy against the existing live captures. Proves: recorded worlds are faithful enough for NVDA to give its live speech.
8. The fault profiles and the world-change mode over the slice's three sequences: a focus change, an outpost generation ending, and an application exiting. Proves: the async rules catch a planted derived copy, and the invariants hold over a seed sweep.
9. The real shells: Windows `Platform`, pipes and the supervisor. The three desktop scenarios run three times each, and the replay-honesty check runs on one. Proves: the replay is honest, and gives calls, wakes and latency per focus against today's.
10. The two worked examples done for real, and the lines measured. Proves or disproves this document's file counts and sizes.
11. The measurement gate over the slice: the corpus mined and the mutants written and sealed by a separate agent during steps 1 to 4, held out from the verifier's builders, then run and the catch rates reported. Proves or disproves that the verifier catches the bugs that cost time before. That, with step 9's figures and step 10's sizes, is what Dickson decides the rewrite on, and the verification's use for the focus slice only; each later area passes its own gate.

Steps 1 to 8 and 11 need no desktop. Step 9 holds the desktop, and is run by one named agent.

## All decisions for Dickson

Taken by Dickson on 2026-11-28 and applied here:
- The records: section 4 of the extensions design replaced to follow ADR 0003's extensions ruling, and the recorder's command line changed (his contract of 2026-11-10), now for the one recorder in Verbatim's readers. ADR 0006 is not amended.
- One recorder built into Verbatim's readers, one format, two levels: the trace always on, world capture when wanted; scenario recording as a world capture with Core not speaking. The test corpus kept current by a one-way converter per format change and a CI load check; ad-hoc dumps without compatibility.
- Release by held views computed from Core's records, with one rule: every id is kept until a held view leaves it out (`ReleasedAtAnswer` dropped).
- Queue rule 1: records owning requests on a closed list, and a foreground stop abandoning a worker stuck on a record's request.
- Queue hooks: end causes as levels declared per stage, and the fourth hook `ended_waiting`.
- The protocol: three message kinds; `Cancel(Late)` and a batch's `Cancel` naming its inner standing requests; the standing host call with `Wants` items and `Resume` (M5); the `Destroyed` and `Barrier` steps; follow items in the tell shape; `Watch` replacement through `replaces`; credit in `WatchFilters` and on `Follow`; a request class on `Ask` only if a desktop run shows the need; named object-model programs as one step kind or several, settled in M13.
- Threads: the listener callback's element through the root, after the backlog measurement; elements of an abandoned worker lost, with the join by runtime id; spares kept by the supervisor; the speech thread as a second asker of the generic messages, tentatively, to be revisited in M6.
- One Verbatim outcome against NVDA's explored set, ruled where the set has several.
- The gated native limiter DLL as the oracle, the model a cross-check; the listener keeping the newer of two coalesced events, a ruled difference (commit deaf7553), entered as a general allowlist rule.
- No real API anywhere in the replay: Verbatim's remote programs by the ordinary-call path, NVDA's by its local executor; live API tests as a third kind of test beside replay tests and end-to-end scenarios, which stay a full layer.
- World capture: hooks inside recorded applications for the display model and the IME; UIAccess on the secure desktop; Office installed when its step comes.
- The harness loading NVDA's GPL binaries, as a GPL-tier test tool; the headless comparison as a CI gate, with the testing standard's wording for allowlisted differences and outcome entries.
- The cost of a feature: one property table driving one generated element struct, with table-driven reads and one test constructor; phrases defined where spoken with `msg!`, the English file generated and the other languages checked against its keys; each world's expectation file holding NVDA's output and Verbatim's expected output, with the checks and the change to `testing.md` that implies; generated reference sections not now. The text interface's design (Core's and the extensions') is the next design task, before the prototype grows past the focus slice.
- The prototype's build order ("Build order for the prototype on the focus slice", and the roadmap's "The rewrite") and the measurement gate's rule: the 95 percent interval on the catch rate against 65 percent, over 40 behavioural mutants with 20 held back, harness defects re-introduced blind, and a pass licensing the focus slice only (Dickson, 2026-11-28).
- The replay's coverage of MSAA, IAccessible2, UI Automation classic and remote, and later Java; math recorded as MathML with MathCAT as it is in the oracle; extensions replayed in the simulator but not compared with NVDA.

Still waiting for him:
- The names "trace" and "world capture", Claude's, pending his preference.
- The outcome-rule form: general predicates tied to rulings, judging generated worlds, with per-step entries only for one-off cases; and the snapshot-difference rule class [review 2 M2]. Open by Dickson's choice (2026-11-28): to be revisited during implementation with worked examples on real cases.

Revisited later: reference sections generated from the code.

An option, not decided: the real-use loop ("Three kinds of test").

Not yet estimated: the Java recorder and stand-ins.
