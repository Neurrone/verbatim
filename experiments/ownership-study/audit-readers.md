# Ownership audit: the readers

Status: read-only audit, 2026-10-30, of main at 5797c075, one of four. It covers the listener's threads, the outposts' threads beyond the focus path that `architecture-study.md` covered, `verbatim-uia`, `verbatim-uia-rops`, `verbatim-ia2` and `verbatim-text`, and the milestones that change them. It takes Dickson's decisions of 2026-10-30 as given: a repeat focus is read in full and judged in Core; the application rules stay until M5; the CoCancelCall experiment runs now. Dickson asked for no change for its own sake. So every change proposed here is marked "Change", with the defect it would have prevented or the concrete sequence it removes. Everything else is marked "Keep".

Terms:

- The judge is the study's per-application pure state machine in the outpost.
- The commit point is `Watch::commit` (`outpost/commit.rs`).
- A standing parameter is a value Core sets in a reader that stays in force between requests, such as `Fetches`, `TerminalShape`, the colours window, the backend override, and in M5 the rule tables.
- Lent state is state a thread holds, locked or borrowed, while it is inside a call into an application.

## In short

1. The listener needs no new owner model. Once `HeldElements` goes, its reading threads keep only their own observations. Its scheduler (`ReadersState`) is already one state behind one lock, and every outcome is sent under that lock, so the lock is in effect the listener's commit point. Change: make the scheduler a pure transition function under that lock, so its orderings can be replayed. Evidence: three review rounds on it (5204f351, then e46d4183 with four defects, then 0b5826cd), and 073d63b0. The `FocusReader` that makes the calls stays as it is.
2. The CoCancelCall experiment (8b15822e) found that `CoCancelCall` ends no UIA client call. So the model here assumes no cancellation for UIA. The judge runs on a thread of its own, and workers post call results to it (section 4). Publication at the bound, abandonment, owed work, `READ_DEADLINE`, `ABANDONED_LIMIT` and `WatchedRead`'s claim all stay.
   - Putting the judge on its own thread makes the lent-state rule (point 3) structural: a worker never holds the judge's state, only a copy sent with its call program.
   - Its cost is one hop each way per call program, and one wake-up of the judge per entry and per result.
   - An alternative avoids the hop: a pure judge value under one lock, applied by whichever thread has the input. Section 4 weighs the two for Dickson.
   - MSAA cancellation would reduce abandoned threads in MSAA and IA2 outposts, but it removes no mechanism, because UIA keeps all of them.
3. Two rules are missing from the study and are needed in this area.
   - Lent state: nothing that a replacement worker, or another thread, needs may be locked or borrowed across a call into an application. The thread works on a copy, and the copy goes into place only through the commit. This is the open follow-up on the text anchors' locks. Its evidence is d0ec3a89, `the_idle_answer_does_not_wait_on_a_text_reads_lock` (2bd69a85), and the workaround in 5797c075. M5 has the same gap in a new form: a module instance borrowed by a hook whose worker is abandoned.
   - Standing parameters: each is versioned, and each fact names the version it was judged under. M5's tables, which are applied both in the listener and in the outpost and reload while running, make this necessary.
4. Text and the caret watch go into the judge. They include the caret rework's one last read per node and the anchors, with the reads as effects and the 100 ms and 1-second bounds as timer inputs. Terminals already comply. The reading state is a pure function (`terminal::reading`), and since 2bd69a85 a read works on a copy of its entry. They join the judge with no change of rule.
5. The outbound merge decides what Core concludes from two messages, so it must preserve what Core's reduction concludes. Change: only message kinds whose merge is shown to preserve that may merge, enforced by a trait. Evidence: dd363ecc, and the rest design's three losses.
6. Architecture section 1 says the worker is the only thread that calls into the application. Three other threads do: `verbatim-uia`'s registering thread (`AddEventHandlerGroup` on remote elements), the `BoundedClient` thread (dialog text), and any thread that drops the last reference to a COM object. Change: state them, for clarity. A model of who may block on whom needs them.

## 1. Processes and threads in this area

The listener (`listener.rs`, `focus_read.rs`):

- The I/O thread, at `HIGHEST` priority. It reads commands from Core and finishes pended writes. It handles `HookInstalled`, `HookRemoved`, `SetBackendOverride`, `CoreFocus` (which goes) and `IdleQuery`.
- The `WinEvent` thread, at `HIGHEST` priority. It builds facts from the global hooks and local reads, and writes them itself. It also runs the console's `ForegroundWatch`.
- The registration thread. It makes the one desktop-wide UIA registration and waits on the foreground application for up to about 6 seconds.
- UIA's callback threads. They queue a marker and the element, or write a fact built from the event's cache.
- One reading thread per application, and its abandoned predecessors, up to `ABANDONED_LIMIT`. They make all of the listener's calls into applications.
- The readers' watchdog. It handles abandonment, publication at the bound, and dropping events that waited past the limit.
- A threadpool timer for the menu-end grace.

An outpost, beyond the focus path:

- The worker generations and the watchdog. The watchdog makes only local calls, and mints MSAA ids for plain windows through the shared registry.
- The I/O thread, the event thread, UIA's callback threads, and the gap timer's threadpool callback.
- The target-watch thread.
- `verbatim-uia`'s registering thread, one per process (`subscribe.rs` `REGISTRAR`). It calls into the application.
- `verbatim-uia`'s `BoundedClient` thread. It calls into the application for a dialog's text, and the worker waits for it up to `DIALOG_TEXT_WAIT`.
- The console caret poll, which since 463f2ef9 runs on the worker's own wait.

The crates:

- `verbatim-uia-rops` and `verbatim-text` are pure. Their only statics are thread-local program caches and the Jieba table.
- `verbatim-ia2` has thread-local hook callbacks and call counts.
- `verbatim-uia` has process-wide statics: `REGISTRAR`, `UIA_READY`, `MTA_USAGE`, and `answered.rs`'s `REFUSALS`, the proxy rule's refusal cache.

## 2. Facts, owners, copies and defects

### The listener

- The per-application read schedule (`ReadersState`): the queue, the live generation, the running read, the abandoned count, `limit_reached`, the foreground counter, `last_focus`, the idle waiters and `alarm_due`. It has one owner, the readers' lock. Every outcome is sent under that lock (`send_outcome`), including the reader's own, the watchdog's publication, the drop on abandonment, and the drop after waiting too long.
  - Defects: 5204f351's moved-on rule, then e46d4183 (four defects: a windowless read could not be judged, waiting focuses were lost, a stale accept, and one more), then 0b5826cd (events at the limit held their outpost's later events without bound). Also 073d63b0 (a stream of later events held the idle answer off) and 69c06239 (the grace timer's callback raced a new menu end).
  - Verdict: compliant in ownership. Change: make it a pure transition, so that these orderings are unit tests rather than review findings. The inputs are an event pushed, a read started, armed, learned or returned (tagged by its generation), the alarm, a foreground event, the outpost ended, the idle query, and a failed spawn. The effects are spawning a thread, sending an outcome, setting the alarm, and waking an idle waiter.
- A read's claim (`Watched::claim`: open, abandoned or published), its progress, and its front judgement. Two publishers, the reading thread and the watchdog, are arbitrated by a compare-and-exchange under the readers' lock, so each marker gets exactly one outcome. Compliant (a9393c04).
  - Change: the marker's outcome becomes a responder that must be consumed once, with a `Drop` that sends a drop. This covers by type the paths that send nothing: 0b5826cd's unresolved markers and the spawn failure in `start`. Process death stays with `UiaFocusReadsEnded`.
  - The listener makes UIA calls only, so after 8b15822e the claim, the publication and abandonment stay as built. MSAA cancellation changes nothing in the listener.
- `HeldElements`, with `accept` and `abandoned`, a copy of Core's focus split over two locks. It goes, by Dickson's decision.
- Each reader's `LastFocus` (for the walk's stop and `same_as_last`) and `last_top`. These are a reader's own observations, written only by its thread. Keep.
- The moved-on rule. The listener's watchdog and the outpost's watchdog each judge only their own reads, through one function (`front_is_another_thread_of_its_application`). Keep.
- The hand-over map `Waiting.hooked`, judged under the sender's lock so that no fact follows `ForwardingEnded`. Keep. The study's item 7 covers the counting on the outpost's side.
- `forced_uia` per application, a copy of Core's override, and the MSAA class table, which the listener's skip and every outpost's verdict both apply.
  - Defect B in `explorer_folder_window`: the listener's read-or-skip and the outpost's count-or-drop are decided apart.
  - Change, with that evidence: the outpost owns the verdict. The listener's skip uses only the shared const table (`arbitration::class_verdict`) and the override, as a pre-filter whose skips the outpost's verdict always also judges MSAA. The override is a standing parameter (section 3).
- `uia_installed` (4008991b) and readiness (11f2d80a and 7e80b866, reversed in one day). These are lifecycle. Keep, with the supervisor's typestate (another audit).

### The outpost: text and the caret watch

- The text caches: `uia_anchors`, `edit_anchors`, `patterns` and `text_support`, on the context, each behind its own mutex.
  - The anchors' locks are held across text calls (`text_reads::answer`, `caret_before_key` and the rest).
  - `held_anchors` is written by the I/O thread as Core's list arrives. Node release is by message position on the worker. So an anchor's life is judged without the position rule, a split owner with no defect seen.
  - The sequence (Change): a worker abandoned inside a UIA text call keeps `uia_anchors`. Every later text operation, caret check, and focus caret read (`read_focus_text`, made before a text focus is reported) in the replacement then blocks on that mutex until its deadline, and is abandoned in turn. This continues up to the abandoned-worker limit, and then the outpost is killed and replaced, with focus reports held meanwhile. d0ec3a89 is the same class, found and fixed for the terminals' lock. Its waiter had no deadline, so the outpost was never even abandoned.
  - The fix: the node's text record is copied out of the judge's state, and the call works on the copy. A clone of a `UiaPos` is a local `AddRef`. Only the result's transition puts the record back, as the terminal entries do since 2bd69a85. An abandoned worker's copy is dropped, and since it publishes nothing, nothing it minted reached Core.
  - UIA text calls cannot be cancelled (8b15822e), so the copy is needed. With the judge on its own thread, the copy is the call program's argument, and the result is an input.
  - Either way, the lock order "text caches before the watch lock" (`commit.rs`) goes. So does 5797c075's direct write that drops a gone element's caches before the caret read, because the commit cannot take the anchors' lock.
- The caret watch (`OpenWatch`, in the follow state since 2bd69a85), the console poll, and the evidence judgement (D14).
  - Defects: 463f2ef9, three of them: a ticker thread and the worker both wrote the one wake time; a blink tick ended the poll; an abandoned worker matched a newer watch by node. Also 8ac783fe (reads are not atomic) and dd363ecc. The caret rework records two known losses from copies of the caret disagreeing.
  - Keep the placement. Change for the caret rework: the one last read per node and the pinned baseline are judge state, released by the position rule, never behind a lock held across a call.
- The `WinEvent` sync (`Nudger::sync`, fb8a4ee7, ecd5f901): the worker waits for the event thread to drain, to learn whether a value change waits. This is the outpost's third route, posted messages against `WinEvent`s. b078ee81 (another event thread's marker satisfied a sync) was confined to tests. Keep. In the judge it becomes an effect, and its completion an input.

### The outpost: terminals

- Owners: Core judges every read (ec7c88c8). The outpost holds the next start (a provider range), the shape Core gave, the colours window, the reading state, the coverage time, the last-sent hash and the caret last sent. These sit in the follow state, changed by commit, and a read works on a copy. The owed requests have their own lock (d0ec3a89).
- The outpost's one skip, `Terminal::sends`, drops a live read identical to the last one sent. It is judged against the reader's own record of what it sent. It is correct only while every sent read reaches Core's model in order, which the rule that terminal reads never merge guarantees. Keep.
- Defects:
  - 353c17ca put the rules in the outpost, and ec7c88c8 moved them out two days later.
  - 29790d46: an unsettled read set the coverage time.
  - d0ec3a89: the lock was held across calls.
  - ffce203b: the order of keys against reads (two routes, the hook through Core and the outpost's read) was fixed by the read's end stamp.
  - c65b6e95: a terminal regaining the focus kept an old screen.
  - a3b55aa1.
- Verdict: compliant. Keep. `terminal::reading` is already pure, so it joins the judge as is. Change (M6.4's history item): every reply names the parameters it was computed under, here the start it read from, so that Core places a read by what it says rather than by which request it answers. This is the rule for standing parameters (section 3).

### The outbound merge

`outbound::merged` merges waiting events by node and kind while a write is in flight. The merge is a judgement of what Core would conclude from both messages, made in the outpost.

- dd363ecc: a caret report's changes were merged past a reply and spoken twice.
- The rest design's unbuilt losses: a tree view item's child count, a focus report's `earlier` evidence (which matters more now that repeats are read in full), and an answer.

Change: merging is allowed only for message kinds that implement a `Merge` trait. Each implementation carries a test that reducing the merged message equals reducing both messages in order. Replies, answers, terminal reads, idle answers and faults do not implement it, so the compiler refuses to merge them. The caret rework's removal of the clause about replies (505e3ad8) then becomes a change to a single implementation. The listener's own `Waiting` merge already keeps markers, reads, drops and notifications apart. Keep it.

### The registries

`verbatim-uia` and `verbatim-ia2` each have a `NodeIdRegistry` with a shared counter. The worker generations and the watchdog write them (plain windows at the bound), and they are released by position under the watch lock and the outbound queue's lock.

- Defects: 5797c075, with half of it reversed in `verbatim-wt-fix2`; 8826d457 and 8a087fe7; defect A (3abda293 with 10c9e83a); and the splice (the rest design's first possible defect).
- Change, as the study says: wire snapshots can be made only by the registry, which records each one at its message's position. Re-sending a spliced entry goes through a registry lookup that fails once the node is released. This prevents the splice. Also one node per window handle, which prevents defect A.

The mutex shared by the worker and the watchdog is not a defect source. Keep.

### The intake's copies of the follow state

The intake holds `focused`, `terminal`, `focus_redirected`, `absorbed` and `system_caret_unused`. Pushing threads read them to drop events at push time. The worker writes them inside the commit, under the nested intake lock, so they have one writer. Keep.

The gap record in the intake is written by three threads: the I/O thread applying answers, the worker, and the timer. That is the study's item 6 (f2f8209a, e02c7854). It goes into the judge. Applying a listener's answer then becomes an input applied under the judge's lock by the I/O thread, so it costs no wake-up of the worker, as today.

### The event thread's own decisions

The event thread keeps `EventWindows`, a cache of the arbitrator's lasting verdicts that only it reads. It also tracks a focus window and foreground window of its own, which it uses to narrow the hook (`narrowing.rs`). That is a second notion of the application's focus, used only to choose subscriptions. Its loss, a fraction of a millisecond of narrowed events, is documented, and no defect is recorded. Keep.

### The backend crates

- `verbatim-uia`:
  - The registering thread is the one owner of every registration in a process. It took three rounds: a8992c9a, 1bfb6f87 and 61a4bfa1. Now compliant. Keep.
  - `Registration::settle` makes the worker wait on that thread, bounded by UIA's connection timeout. Keep.
  - `REFUSALS`, `UIA_READY` and `MTA_USAGE` are a cache and lifecycle values. Keep.
- `verbatim-uia-rops`: pure program builders and a fixed instruction limit. Keep.
- `verbatim-ia2`:
  - Hooks, acquisition and equality. Acquisition's window-message half cannot be cancelled (`experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/`), so abandonment stays for it whatever M6.1 finds.
  - Change, from the study: equality takes its candidates from the outpost's own reported line, not from `Held`.
- `verbatim-text`: pure, run in Core, with no threads. Keep.

## 3. Rules this area needs beyond the study's

Each rule is listed with its evidence and the compile-time form that would enforce it.

1. Lent state. No thread holds a lock or a borrow across a call into an application on state that another thread may need. It works on a copy, and the result's transition puts the copy in place.
   - Evidence: d0ec3a89, the anchors follow-up, and 2bd69a85's terminal copy.
   - Compile-time form: every call into an application takes `&mut CallToken`, owned by the worker. Every lock on the shared state is reached only through `&CallToken`, so the borrow checker refuses a guard held across a call. That is safe Rust with no run-time cost. `MutexGuard` being `!Send` already keeps a guard on its thread.
2. Standing parameters are versioned. Each standing parameter carries a generation. A fact or reply names the generation, and the start or other parameters, it was computed under.
   - Evidence: M6.4's history read placed by the request it names rather than by the start it read from. The verdict defects that came from one table applied in two processes (094a8dc8, 07db29a8, a3d6bc23, 248f85d3, and defect B). And M5's tables, which the listener's vetoes and the outpost's rules both apply, and which reload while running.
   - Compile-time form: parameter messages are a distinct enum from queries, so the Core-to-process list the study proposes has three kinds: queries, `NodesHeld`, and versioned parameters.
3. One writer per timer. A timer or wake belongs to the state machine that owns its record, and setting or cancelling it is an effect applied in transition order.
   - Evidence: 463f2ef9 (two writers of one wake time), f2f8209a (set and cancel out of order) and 69c06239 (a callback racing a new end).
   - Compile-time form: timers are created only inside the judge's or scheduler's module, and expose set and cancel only through the effects type.
4. Exactly one answer, including for requests with several answers. A request with several answers, such as the caret rework's key request, is a stream responder. It may send answers and must end with a final one. Its `Drop` sends `WatchEnded` or gone.
   - Evidence: the idle barrier's dependence on watches ending, 0b5826cd, and the study's review finding 3.
5. Reduction-preserving merges (section 2, the outbound merge).
6. A static holds only a cache or process lifecycle, never a judged fact. Today's statics comply (`SPARE`, `REFUSALS`, `REGISTRAR`, `NEXT_READER`). Keep. The rule is stated so that M5 and M7 keep it.
7. Thread-affine backends. A backend whose objects or calls are bound to one thread cannot have its worker replaced by abandonment. That backend's recovery is kill-and-replace only. The rules do not cover this. It matters for M13 (section 5), and for the in-process helper's work, which runs on the application's own thread in M7.

## 4. The judge for terminals, text and the caret watch, and the listener's shape

What the judge adds for this area:

- State:
  - Per text node: its last read, its anchors (as keys to provider ranges held beside the registries), its pattern and support caches, and at most one open watch with its pinned baseline and the console poll.
  - Per terminal: the next start, the shape, the colours window, the reading state, the coverage time and the last-sent hash.
  - The owed requests.
- Inputs:
  - Text and terminal requests, each with its responder.
  - The node's caret, text and selection events, and console updates, in intake order.
  - Call-program results tagged by request, with their start and end stamps.
  - Timers: the key's 100 ms bound, the watch's 1-second bound, the poll's 5 ms ticks, and an owed read's retry. That last one is needed: M6.4 records that an unsettled read with nothing queued behind it is never retried.
  - The completion of a `WinEvent` sync, the registration settling, `NodesHeld`, and changes to standing parameters.
- Effects:
  - Call programs: the caret read program, the terminal snapshot, text operations, and a sync.
  - Messages, timer settings, and objects to drop after the transition.
- Already pure and taken over as they are: `terminal::reading`, `check_caret`'s evidence judgement given a read, `Terminal::sends` and `covers`, and the gap record.

Threading, with no cancellation of UIA calls (8b15822e). The judge runs on a thread of its own:

- The judge thread owns every remembered fact of the application: the follow state, the gap record, the verdict keeps, the text records and the terminals. Its mailbox is today's intake. The I/O thread, the event thread, UIA's callbacks and the timer callbacks only append to it.
- A worker takes one call program at a time, with copies of the state the program needs, such as a node's text record or a terminal's entry. It makes the calls and posts the result, tagged by its generation and request. It never touches the judge's state, so the lent-state rule holds by construction, and d0ec3a89's class and the anchors follow-up cannot arise.
- The walk's progress stays a single-writer slot. The worker writes it between calls, and the judge reads it when the bound input arrives, under a lock never held across a call. The alternative, one posted input per ancestor, would cost a hop per step. Publication at the bound is then the judge's transition on the bound input: it publishes from the slot, marks the request claimed, and a late result tagged with a claimed request is discarded as a plain input. This replaces `Watch::commit`'s in-charge check under the lock, and the watchdog publishing on its own thread.
- The watchdog only posts timer inputs: the bound, the deadline, the moved-on grace. The judge decides abandonment. It marks the generation, and its effects are to start a replacement worker and to answer `Abandoned`. An abandoned worker's posted results carry a dead generation and are dropped. Owed work is a judge record, handed to the next worker as its first program.
- The cost: one hop each way per call program, tens of microseconds each by the communication design's count. One wake-up of the judge per entry, and one per result, where today the worker handles an entry with no other thread. Releases of COM objects stay on the worker, after the result is taken.
- The alternative for Dickson: the judge as a pure value under one lock, applied by whichever thread has the input, with the lock's order recorded as the replay order. It needs no hop and no extra wake-up. But the lent-state rule is then only a borrow rule (`CallToken`), not structure, and the watchdog's publication still runs on the watchdog's thread.

Where MSAA cancellation would still simplify:

- An `IAccessible` COM call can be cancelled, as can IA2's calls in M7. Acquisition's window-message half cannot be, and neither can oleacc's in-process proxies.
- In an MSAA or IA2 outpost, a stuck walk's call could be cancelled at the bound. The worker would then complete the publication itself and keep its thread, which cuts abandoned threads and kill-and-replace for those applications.
- It removes no mechanism, because UIA keeps them all, and the listener is UIA only. Its value is fewer lost threads for browsers in M7, at about 12 µs per call (debug).

The listener: same shape, smaller. Its scheduler is the pure part, under the readers' lock it already uses. The `FocusReader` stays imperative call code, as the outpost's walks do, with NVDA's checks as pure functions of call results. With `HeldElements` gone, the listener judges nothing against another process, so it is not an application's judge and holds no per-application facts beyond its readers' own last focus.

## 5. Planned milestones

- The caret rework, on the outpost side.
  - New shared facts: the last read per node, the pinned baseline, and the request with several answers.
  - Rules:
    - The last read is judge state, released by the position rule.
    - The request is a stream responder.
    - The 100 ms bound is the worker's own wake (rule 3).
    - "A repeat sent unread is not read" becomes, by Dickson's decision: a repeat is read in full, and its read replaces the last read and is compared with it. That also removes the Control+A loss.
- M5, modules in outposts.
  - New: one wasmtime store per outpost, used by hooks on the worker. The watchdog bumps the epoch. A listener veto is judged from the event's cache, and node and event rules in the outpost.
  - The sequence the design misses: a hook's node read blocks in a call, and the watchdog abandons the worker. The replacement needs the store, which the abandoned thread holds by `&mut`. Either it waits on it (d0ec3a89's class, a cascade to kill-and-replace) or it shares it (undefined behaviour).
  - Rule (lent state): the store has one owner, the worker generation in charge. Abandonment while it is lent counts as a fault of the module: the store is discarded and the module instantiated again, losing its memory, as `extensions.md` already says for a fault.
  - With the judge on its own thread, a hook's read can be a call program posted to a worker, so the store stays on the judge thread and is never lent. That needs asynchronous imports, which M5 excludes. So until then the store is lent to the worker, and the poisoning rule applies, because UIA reads cannot be cancelled.
  - Tables carry a generation (rule 2). The second sequence: the listener vetoes a focus under table v2, the gap record is open, and the outpost's gap read under v1 reads the vetoed focus a second later. So a veto's drop must answer the gap record, and the outpost's rules must apply to its recovery reads under the same generation. The roadmap already asks for the second part.
  - `focus()` means the followed focus, and a hook's answer is a judge input (study, question 3).
- M6.1, cancellation.
  - New: the watchdog cancels calls.
  - Race the rules do not cover: the watchdog decides to cancel call k, but the worker returns and enters call k+1, and a cancel aimed at the thread lands on k+1.
  - Rule: cancel only through the call's own cancel object (`CoGetCancelObject`), taken while the call's ticket is still the current one. An object for a finished call refuses the cancel.
  - 8b15822e found that UIA calls cannot be cancelled. So M6.1 can only be MSAA and IA2 cancellation, and every mechanism stays. It lowers abandonment in MSAA and IA2 outposts, mostly browsers.
- M6.2, one outpost per application.
  - No new reader thread. The rule on the readers' side: a marker and its outcome name the outpost incarnation. An outcome for an incarnation that has ended is dropped by the supervisor, so it never resolves the successor's queue. The successor starts with no markers. That this holds today was not checked here. It belongs to the supervisor audit.
- M6.4, terminal residuals.
  - No new threads. Replies name their parameters (rule 2).
  - Owed reads get a timer input.
  - A whole-document read counts its rows from its newest start, so that Core places it by that start.
- M6.5, text residuals.
  - New calls on the worker: `ITextDocument` for rich edit controls.
  - The classic edit control's drop of a caret event whose offsets equal the last read's is a skip by the reader on its own record within one node, which is allowed, unlike 43de3932's global repeat.
  - The caret read's choice of backend by the previous focus's window is a stale copy inside the outpost. It goes once the window record is judge state.
- M7, browse mode, IA2 and the injection helper.
  - New: many IA2 COM calls on the worker, with IA2 unique ids as node identity. Also a helper inside the browser's process, and perhaps a virtual buffer there.
  - Rules:
    - The helper talks only to its application's outpost, never to Core directly, because a second route out of one application recreates the marker problem.
    - If the buffer lives in the helper, it is a reader's cache. Its updates go one way, in order and numbered, as Chromium's tree updates do. Core's queries are answered through the outpost, and every answer names the update number.
    - The outpost's calls to the helper go over a pipe and can be cancelled with `CancelIoEx`, so they need no abandonment.
    - The helper's work on the application's UI thread is thread-affine (rule 7).
    - The registry keys IA2 nodes by window and unique id, as one node per identity.
- M13, the Java Access Bridge.
  - New: a bridge thread with its message loop in the outpost, and calls that are not COM, so `CoCancelCall` does not apply.
  - Java objects need `ReleaseJavaObject`, under the position rule.
  - To establish before design: whether the bridge's calls and callbacks are bound to the thread that initialized it. If they are, rule 7 applies, and the outpost's recovery is kill-and-replace only.
- M14, the display model.
  - New: GDI hooks in the helper, called on every drawing thread of the application.
  - Rule: drawing threads only append to a queue that never blocks. One helper thread owns the text model. The model goes to Core as numbered snapshots, and Core's terminal model judges them, keeping ec7c88c8's split of reads in the reader and judgement in Core.

## 6. Kept as they are

- The listener's moved-on rule.
- The hand-over map.
- The `Waiting` merge.
- `LastFocus`.
- The terminals' split and skip.
- The intake's copies of the follow state.
- The event thread's narrowing.
- The registering thread.
- `BoundedClient`.
- The registries' shared lock.
- `verbatim-uia-rops`, `verbatim-text`, and the process statics.

Each has one writer, or is a cache that only its owner reads, and no defect is recorded against its present form.
