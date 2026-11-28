# Design options for the rewrite: protocol, queue, reader concurrency, element lifetime

Status: options study, Claude, 2026-11-28. Nothing in the repository changes because of it. It reads the slice design (`docs/design/rewrite/design.md`), ADR 0003, ADR 0006, the focus and ownership explanations, `crates/verbatim-core/src/event_queue/`, and NVDA's pinned `queueHandler.py`, `eventHandler.py` and `IAccessibleHandler/orderedWinEventLimiter.py`. The fixed constraint is the process split (Dickson, 2026-11-28). Every claim about outside systems cites a source read for this study. Claims I could not check against a source are left out.

## Assessment first

- Protocol: take the slice's four messages (`Ask`, `Cancel`, `Held`, `Answer`), with standing requests. A fully declarative subscription model would also cover the follows. But Dickson ruled on 2026-11-28 that a follow is a standing request opened inside a batch ("Follow becomes one"), so I give that model as an option and do not recommend it.
- Event queue: replace the paused async function with one pure reducer that never awaits. Each queue entry's waiting state would be a sealed enum, built from the focus stages the slice design already defines (tell, accepted, walked, read). The stages already carry the state, so the async layer only threads them through. Removing it removes the checkpoint, the rebuild and the driver's invariants about paused functions, and makes Core's whole state plain data that can be cloned, serialized and forked. This reverses ADR 0003's ruling "Core's code is async functions" (Dickson, 2026-11-14), so it is his call. If he keeps async functions, a seeded simulator still works. Dropbox's Trinity tests a Rust future under a custom executor.
- Reader concurrency: each process gets one root reducer that runs without I/O. Threads become executors that hold no state: they run a job, post its completion and touch no shared record. COM objects stay in an arena owned by the executor that made them. The reducer holds only their ids. A hung call is modelled as a job whose completion never arrives. Which work runs on which thread is unchanged, so Dickson's item 3 is not touched, but the per-application threads' lifecycles move into the root's records.
- Element lifetime: release by scope. Every id belongs to the request or tell that minted it, and goes when that scope ends. A batch can mint into a standing scope named when Core asks. This removes the position counter and the computed held set, and release rides on the one ending every request already has. It replaces ADR 0006's "Nodes are released by message position" and changes Core's protocol, so it is Dickson's call (item 4). Leases and reference counts are rejected.
- Decisions for Dickson:
  - Async functions or a pure reducer for the queue.
  - Scope release or held views.
  - The slice design's protocol, which it already lists.

## Background facts the options must keep

These are NVDA's observable ordering rules, read in the pinned source:

- One first-in, first-out queue, emptied in order on the main thread. Each queued function runs to completion before the next. `flushQueue` is at `queueHandler.py` 61 to 70, and `pumpAll`, which steps the registered generators and then empties the queue, is at 80 to 102.
- Because the main thread blocks inside each call into an application, only one event is ever being handled. That is the source of Verbatim's "only the front entry may have a request out".
- `queueEvent` sets `lastQueuedFocusObject` at the moment of queueing, before the event runs (`eventHandler.py` 48 to 67, `_trackFocusObject` 172 to 183). Verbatim's readers' last focus is defined differently, as ruled in ADR 0003, "The two versions of the focus".
- The MSAA limiter, per pump:
  - It keeps at most 10 events per thread, newest first, except for objects always allowed (`orderedWinEventLimiter.py` line 10, and 98 to 112).
  - It keeps the newest 4 focus events (line 31, and 113 to 116).
  - It emits the kept events in their original order through a heap keyed by an arrival counter (117 to 128).

All of these are rules about where an entry waits and what is dropped. None of them depends on how the front entry runs, so every queue option below keeps them unchanged.

## 1. Core's protocol with its peers

### Option A: today's tells, requests and lifecycle messages

- How it works: each direction and role has its own enum: `CoreToOutpost`, `CoreToListener`, `OutpostToCore`, `ListenerToCore`, and the supervisor's `OutpostMessage`, with 8, 7, 9, 10 and 13 variants, as the slice design counts them.
  - Tells are one-way.
  - Requests get one of five outcomes.
  - Lifecycle messages are their own kinds: ready, pong, fault, target exited, spare ready and the others.
- What it removes: nothing.
- Costs: per UI Automation focus, three crossings into Core, as ADR 0003 counts under "Limits accepted".
- Under a simulator: it works, but each message kind needs its own generator and its own fault cases. A peer's end must be matched to its open requests through Core's request table, because nothing ties the table to the peer's record.
- Failure modes: the table's `Owner`, its stages and its per-request generation are the bookkeeping class ADR 0003 counted (71 of 519 fixes were waiting-state bookkeeping). Messages accrete per feature, as the 42 kinds before ADR 0003 did.
- Precedent: none needed. This is the baseline.

### Option B: the slice's four messages with standing requests

- How it works, as the slice design gives it:
  - Core sends `Ask { number, due, ask }`, `Cancel` and `Held`.
  - Peers send `Answer { number, Item | Final }`.
  - The listener's events are items of Core's standing `Watch`.
  - A generation's life is the standing request `Serve`, with `Started`, `Ready` and `Pong` as its items and `Ended` or `Exited` as its final outcome.
  - A follow is a standing request opened by a batch step.
- What it removes:
  - The request table's owner, stages and generation, because each request hangs off its peer's record.
  - `ApplicationExited`.
  - The supervisor's message enum.
  - Each feature's own message kinds.
- Costs: the same crossings per focus as today, plus one `Cancel` per focus for the old follow, which today rides in the next batch. That is one more pipe message and no added wake of Core, by the slice's count.
- Under a simulator:
  - The simulator needs only one message model: deliver, delay, reorder between pipes, drop, or end the peer.
  - It can also inject an `Item` arriving after `Cancel`, which must be dropped by number.
  - Because a peer's end hands back its record's whole `out`, the property "every request has exactly one ending" becomes one check over all seeds.
- Failure modes:
  - An item for a cancelled standing request is in flight when Core cancels it. That is benign by construction, since it is dropped by number.
  - A generic message pushes type safety from the wire into parsing at the boundary (R5). An undecodable answer is refused, never defaulted.
- Precedent:
  - Cap'n Proto's RPC uses one question table per side, matched to the other side's answer table. When a connection is lost, "everything on the four tables is lost" and all imports become broken (`rpc.capnp`, "The Four Tables", https://raw.githubusercontent.com/capnproto/capnproto/master/c%2B%2B/src/capnp/rpc.capnp). That is the same rule as "a peer's end answers its requests".

### Option C: a declarative subscription and query split

- How it works: one-shot reads stay `Ask`. Everything Core wants to stay true at a peer, which is its follows and the ids it holds, is sent as one declared view. Core computes the view from its records after each step. The peer reconciles to it, opening and closing follows itself, and tells Core of changes under the view's position. The listener's `Watch` and the supervisor's `Serve` stay standing requests, because they are lifecycles.
- What it removes:
  - `Cancel` for follows, and the ordering rule "cancel the old follow only at commit". The next view simply leaves the old follow out.
  - The separate `Held` message, since holds and follows are one view.
- Costs:
  - A view of tens of ids per change, sent at most once per Core step.
  - The same calls into the application.
- Under a simulator: very good. The peer's desired state is a pure function of Core's records, so one property covers holds and follows: the peer converges to the view.
- Failure modes:
  - Reconciling is a second interpreter of the same fact. The peer decides when a follow opens, so the rule "the follow is registered before the focus's states are read" moves into the peer's reconciliation order.
  - It conflicts with Dickson's 2026-11-28 ruling that a follow is a standing request opened inside the read's batch.
- Precedent:
  - Elm's runtime asks the program for `subscriptions : Model -> Sub Msg` (https://guide.elm-lang.org/effects/time.html), and "Elm manages all the details of subscriptions instead of you" (`Platform/Sub.elm`, https://raw.githubusercontent.com/elm/core/master/src/Platform/Sub.elm).
  - DCOM's garbage-collection ping sends the client's set of held object ids. When the set has changed, `ComplexPing` sends only the additions and removals (MS-DCOM 3.2.6.1, https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-dcom/8abedee5-e226-43e7-bb7c-34d0ac6a3c38).

### Recommendation

Option B. Option C is cleaner on paper, but it moves the follow's ordering rule into a reconciler and reverses a ruling made today. Option B gets most of the testability gain: one message model, and a peer's end answering its own requests. If Dickson adopts scope release (section 4), `Held` goes too, and the protocol becomes three messages.

## 2. Core's event queue

### Option A: today's async front entry with a checkpoint and 26 hooks

- How it works:
  - The front entry's function is a boxed future, polled with a no-op waker on Core's thread. It pauses only in `Asking`.
  - Core's `State` is plain data, but the paused future is not. A clone, a restored dump or a replay rebuilds it by running the entry's function again from its checkpoint, which holds the state at the entry's start plus the recorded asks and endings (`driver.rs` lines 1 to 20 and 580 to 650).
  - The queue reaches features through the `Logic` trait's 26 methods (`event_queue/mod.rs` 259 to 486). They include arrival, placement, merging, the cap exemption, the run's entry, the endings of a request out (`ends`) and of a waiting entry at a foreground change, the readers' endings and re-takes, ticks, wanted requests, the plain reader, tokens and stamps.
- What it removes: nothing.
- Costs:
  - The driver is 1,284 lines and the queue module about 2,900 without tests. The driver's own invariants are `parked`, `unanswered`, the slot, `rebuilding` and `at_once`.
  - Every snapshot costs a re-run of the front function.
  - ADR 0003 records the limit that the checkpoint copies Core's state at every entry boundary, a cost that grows with the application.
- Under a simulator:
  - Seeded replay works, because the driver is deterministic.
  - Forking a run in the middle of an entry, to explore both orders of two deliveries, needs a rebuild per fork.
  - The paused function's locals cannot be seen by the state ledger or the sealing check.
- Failure modes:
  - A copied fact held across an `await` is not caught by the compiler (ADR 0003's stated limit).
  - The hooks spread one feature's rules over many trait methods.
- Precedent:
  - Dropbox's Trinity runs the sync engine, a Rust future, under a custom executor that chooses which intercepted requests succeed or fail, with seeded randomness and a rerun of the same seed to check the result (https://dropbox.tech/infrastructure/-testing-our-new-sync-engine).
  - FoundationDB's simulation is built on Flow, its coroutine runtime, inside one single-threaded process (https://apple.github.io/foundationdb/testing.html).
  - NVDA itself steps generator objects from its pump (`queueHandler.py` 24 to 33 and 80 to 102).

### Option B: async syntax, re-executed against recorded answers

- How it works: no future is stored. When an entry's answer arrives, its function runs again from the start. Each `ask` it reaches returns the recorded answer if there is one, and pauses otherwise. The state is the entry plus its answers so far, which today's checkpoint already records.
- What it removes:
  - The stored future and the driver's invariants that guard it.
  - The difference between the live path and the rebuild path, since replay becomes the only path.
- Costs:
  - The function runs about k squared over 2 times for k asks. Entries ask 1 to 3 times, or more for an MSAA candidate run.
  - Effects emitted before the last ask must be deduplicated.
  - Code before an ask must read only the facts as they were at the entry's start. Otherwise a re-run sees newer facts and asks a different batch. That forces a snapshot of the facts, which is today's checkpoint again.
- Under a simulator: as good as Option C for forking, once the snapshot rule holds.
- Failure modes: the snapshot rule changes the meaning of code after an ask. The narrow rule reads the committed focus's waiting state changes as they are when the answer arrives, not as they were at the entry's start. A function that reads current facts after an ask and start facts before it is exactly the subtle class to avoid.
- Precedent: Durable Functions "re-executes the entire function from the start to rebuild the local state", consults the execution history, and requires orchestrator code to be deterministic. Its own note says replay can emit duplicate log messages (https://learn.microsoft.com/en-us/azure/azure-functions/durable/durable-functions-orchestrations).

### Option C: one pure reducer that never awaits

- How it works:
  - Core is `step(&mut State, Input, now) -> Vec<Effect>`.
  - The queue is a sealed record: the front, the waiting items, and the limiter's placement.
  - The front is `Front::Judging(entry)` or `Front::Asked { stage, out }`, where `stage` is a sealed enum per entry kind whose variants are the slice design's stages. For a UI Automation focus that is `Accepted` waiting on the listener, then `Walked` waiting on the outpost.
  - An answer is an input. It moves the stage on, or ends the entry.
  - The 26 hooks become ordinary functions of the records. Placement, merging and the cap read the queue record. Endings read the peer records (`Apps::ended` hands back `out`). Held ids or scopes are owned by the records (section 4).
- What it removes:
  - The async driver, the checkpoint and the rebuild.
  - The `Logic` trait's indirection.
  - ADR 0003's two limits: the copied state across an await and the checkpoint's growing copy.
  - The paused future, so the state ledger, the sealing check and the dumps see every waiting state.
- Costs:
  - A chain of requests reads as stage transitions, not top to bottom. ADR 0003 rejected this on 2026-11-14 for that reason.
  - The UI Automation focus has two stages, and the MSAA run loops over its candidates. These are the longest chains in the slice.
  - Lines: about the same as the slice's 600 lines for the queue and driver, minus the driver, plus about 50 lines of stage enums.
- Under a simulator:
  - It is the best of the three. Core's state is fully data, so it can be cloned and serialized at any input, and forked to explore both orders of two deliveries without re-running anything.
  - Dumps are the state itself.
  - The differential check against NVDA replays recorded tells and answers as inputs and compares the `Say` effects.
- Failure modes:
  - A stage added without a transition for every input is caught by an exhaustive match, not by review.
  - The risk is readability, not correctness.
- Precedent:
  - quinn-proto is "a fully deterministic implementation of QUIC protocol logic" that "contains no networking code and does not get any relevant timestamps from the operating system" (https://docs.rs/quinn-proto/latest/quinn_proto/).
  - Firezone's sans-I/O state machines take the time as a parameter and expose `poll_timeout` and `handle_timeout`, and the article reports tests that advance time by minutes instantly (https://www.firezone.dev/blog/sans-io).

### Recommendation

Option C. The slice design's typed stages (R3) already are the per-entry state machine, so the async layer duplicates them and hides the paused state from the checks. Option B looks cheaper, but its snapshot rule is the copied-fact class in another form. Option C needs Dickson to reverse "Core's code is async functions". If he keeps Option A, slim it with the slice's records: the reader and hold hooks go to the peer records, and about a third of the 26 hooks disappear by my reading of their bodies, a figure I have not built.

## 3. Concurrency inside each reader process

Today:

- The listener has a check thread and a reading thread per application. Their lifecycles are spread over the six collections the slice design lists.
- The outpost has a pure reducer (`verbatim-outpost-logic`, with worker generations in `charge.rs`) beside an object store shared with the worker as a monitor (`ownership.md`).
- A registering thread makes the UI Automation registrations.

The calls themselves cannot be cancelled. That is D9 in Verbatim's records. COM's own cancel mechanism only unblocks the caller and gives "no guarantee that the server object will stop processing the call", and it handles synchronous calls only (https://learn.microsoft.com/en-us/windows/win32/com/canceling-method-calls). UI Automation's transaction timeout defaults to 20 seconds (https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation2-put_transactiontimeout). Verbatim sets it to 10 (`focus.md`).

### Option A: today's split

- How it works:
  - Listener: per-application threads each own part of their lifecycle, with the listener logic's schedule and check-thread records beside them.
  - Outpost: the reducer thread, the worker, the registering thread, and the store as a monitor.
- Costs: no extra hops.
- Under a simulator:
  - The outpost's reducer can be driven.
  - The listener's lifecycle sits in six collections across logic and shell, so a crash or hang must be injected in several places.
  - The store monitor's lock is outside any reducer.
- Failure modes: the copies the slice design names, such as `KeptElements.kept` held globally and per application, and the outpost's seven side maps.

### Option B: one root reducer per process, threads as executors that hold no state

- How it works:
  - Each process has one root reducer: `Listener`, `Outpost` or `Children`. It owns every record, including each executor's lifecycle, for example `Executor { generation, state: Idle | Busy { job, since } | Abandoned { since } }` in each `ListenedApp` or outpost record.
  - An executor thread loops: take a job, make its calls, post `Completed { executor, generation, job, result }` to the root's mailbox.
  - COM objects never enter the reducer. Each executor keeps an arena from id to object for the objects it acquired, and the reducer holds only ids. A release is a job on that executor, so every COM release happens on a thread allowed to block.
  - UI Automation and WinEvent callbacks are I/O sources only. They put the element in the arena of that application's check executor and post an `Event` input. NVDA's local drops are pure functions the callback may call, because they make no call and read no record.
  - Abandonment is a reducer transition. Its effects start a fresh executor with a new generation and leave the old thread inside its call. A late `Completed` from an old generation is dropped by that generation. The cap of two abandoned reading threads per application (`focus.md`) becomes a field of the application's record.
- What it removes:
  - The six per-application collections.
  - The store's lock.
  - The shell holding any decision.
  - The slice design's generic `E` inside records, so the reducer's state can be serialized and its COM releases are confined to executors.
- Costs:
  - One more thread hop per UI Automation event in the listener: callback, then root, then check executor, then root, then pipe, where today the callback hands straight to the check thread.
  - That is about two more thread wakes per event. I have no measurement of it, and the ADR 0003 measurement of the background-event backlog would have to be repeated.
  - The alternative wiring hands the event straight to the executor and lets the executor report it. That saves the hop but puts a routing table from process id to channel outside the root, which is a copy.
- Under a simulator:
  - Executors are replaced by a seeded scheduler that delivers each job's completion after a chosen simulated time, never (a hung call), or not at all because the process ends (a crash).
  - Element handles are plain ids.
  - All four process reducers run in one thread. Each pipe keeps first-in, first-out order, and the scheduler interleaves between pipes.
  - "Slow or hung application calls, crashes, seeded and replayable" become generator choices, not threads.
- Failure modes:
  - The arena and the reducer's element record are two places for one element. They are not two copies of one fact: the reducer owns the lifecycle and the arena owns the resource, which the reducer frees only through a job. This is the same split ADR 0003 accepts across processes for kept elements.
  - A completion for an id the arena already freed must be impossible by construction, because frees are jobs on the same executor and so ordered after earlier jobs.
- Precedent:
  - Trinity runs Nucleus on one control thread and offloads concurrency "only to I/O and hashing work that Trinity can serialize". Its mocks "reorder, delay, or fail RPCs" and fast-forward timers (https://dropbox.tech/infrastructure/-testing-our-new-sync-engine).
  - TigerBeetle's VOPR: "all non-deterministic parts of the system are stubbed out. This includes the clock, network, and disk operations". It is "deterministic based on a seed number and the Git commit" (https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/internals/vopr.md).
  - FoundationDB injects "degraded machine performance" and "delayed channels" in its single-threaded simulation (https://apple.github.io/foundationdb/testing.html).

### Option C: real threads under a simulated runtime

- How it works: the readers' threads become tasks of a deterministic runtime in tests, through madsim's tokio stand-in (https://github.com/madsim-rs/madsim) or turmoil's simulated hosts and network, which runs everything "on a single thread" and can drop, hold and delay messages (https://tokio.rs/blog/2023-01-03-announcing-turmoil).
- What it removes: rewriting the threads as executors.
- Costs:
  - The readers' code must become async over the runtime, while COM calls block.
  - It adds a major dependency (Dickson's item 6).
  - Fake COM is still needed, because the real calls cannot run under the simulator.
- Under a simulator: good for message faults. It does not reach the decisions inside each thread unless those are also made sans-I/O, which is Option B again.
- Failure modes: logic stays in threads, so lifecycles stay spread.
- Precedent: as cited.

### Recommendation

Option B with the arena. Measure the listener's added hop with ADR 0003's backlog measurement before choosing between the two wirings. Keep the hop unless the budget is missed, because the direct wiring brings back a copy outside the root.

## 4. Element lifetime across processes

Today, held views by message position (ADR 0006): Core sends the ids it holds with the position of the last message carrying ids it handled. The reader releases what the view leaves out once Core has handled that position, and spares every id a waiting request names. The slice design computes the view from Core's records rather than keeping a list.

### Option A: held views by position, computed from records

- One answer path: separate from requests. Release is its own message and its own position counter per peer (`handled`), plus the spare rule for waiting requests.
- No copies: the held set is derived, not stored, in the slice design.
- Cost per focus: one `Held` to the listener and one to the outpost, each tens of ids, beside the old follow's `Cancel`.
- Under a simulator: good. Releases are a pure function, and the property "no id is released while a later message may name it" is checkable.
- Failure modes: an id left out of the computation is released while still needed, and its next use answers "not there". The ordering arguments rest on the position counter.
- Precedent: DCOM's ping sets name the held objects as a set, with additions and removals (MS-DCOM 3.2.6.1, cited above).

### Option B: scope-based lifetime

- How it works:
  - Every id belongs to at least one scope. A scope is a request, standing or one-shot, or a tell.
  - A batch names, when Core asks, the scope its results mint into: its own number, or a standing scope such as the focus's follow.
  - An outpost node keeps the set of scopes it belongs to, and is released when that set empties.
  - A re-mint of the same element into a new scope adds a membership. The new focus's walk adds the known ancestors at and above the meeting point to the new scope, so the shared part of the line survives the old follow's cancel.
  - A one-shot request's scope ends with its `Final`, unless its batch minted into a standing scope.
  - Listener tokens belong to their tell. Core ends a tell's scope when its queue entry ends, in a batched `Done`, as Cap'n Proto's caller ends a question with `Finish`.
  - Later milestones' navigator or review objects live in a standing "hold" request with no items.
  - A generation's or listener's end ends every scope at once.
- One answer path: the same as the request's. A scope ends exactly when its request ends, by answer, by cancel or by the peer's end, which ADR 0006 already guarantees once.
- No copies:
  - Core keeps no held set and no position. Each id lives only inside the record that owns its scope, such as `Focus` with its line and its follow, so an id cannot outlive its record in Core.
  - Membership sets live only in the outpost.
- Cost per focus: the old follow's `Cancel` carries the release, and the outpost gets no `Held`. The listener's `Done` for ended tells is batched per Core step, so it replaces the listener's `Held` one for one. That is one message fewer per focus than Option A.
- Under a simulator: good. "An id named in an `Ask` belongs to a live scope at the peer" is a property over every seed. A cancel crossing an item is dropped by number, as in section 1.
- Failure modes:
  - A scope chosen wrongly when Core asks, for example a read whose result Core later wants to keep, costs a re-read ("not there"), the same failure as an id missing from a held view.
  - If the new read fails after the outpost added known ancestors to its scope, the membership set still holds them for the old follow. That is why memberships are sets, not a move.
- Precedent: in Cap'n Proto, `Finish` with `releaseResultCaps` means "all capabilities that were in the results should be considered released", and "when a connection is lost, all exports are implicitly released" (`rpc.capnp`, cited above).

### Option C: leases with expiry

- How it works: the peer releases an id unless Core renews it in time.
- One answer path: no. Expiry is a second ending beside the request's.
- Cost per focus: renewals cost wakes of Core and the peers on a timer, against the wake-up budget.
- Under a simulator: testable with controlled time, but correctness depends on time. Java RMI's own specification warns that a partition can cause "premature collection" so that "remote references cannot guarantee referential integrity" (https://docs.oracle.com/javase/8/docs/platform/rmi/spec/rmi-arch4.html). An unrenewed lease means the collector "assumes that the remote object is no longer referenced by that client" (https://docs.oracle.com/javase/8/docs/api/java/rmi/dgc/DGC.html).
- Verdict: rejected. A busy Core would lose nodes it holds.
- Precedent: Gray and Cheriton, "Leases", SOSP 1989, pages 202 to 210, where leases serve cache consistency (https://courses.cs.duke.edu/spring15/cps210/readings/leases-sosp89.pdf). DCOM's ping period is "at most 2 minutes" (MS-DCOM 3.2.2, https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-dcom/523a794e-1e72-44b9-8fea-e7142bbb4358).

### Option D: explicit release messages with reference counts

- How it works: as COM's AddRef and Release, or Cap'n Proto's `Release`, which carries "the amount by which to decrement the reference count".
- No copies: no. Both sides keep a count of the same fact.
- Failure modes: re-mints racing releases need sequence numbers, as RMI's `clean` call uses one "to detect late clean calls" (DGC javadoc, cited above). This is the bookkeeping class ADR 0003 counted.
- Verdict: rejected.

### Option E: generation-scoped handles only

- How it works: an id lives until its generation ends.
- One answer path: yes.
- No copies: yes.
- Failure modes: COM objects grow for the outpost's whole life. ADR 0006 rejected identity that outlives the object for the same growth.
- Verdict: kept only as the backstop every option already has.

### Recommendation

Option B, scopes. It folds release into the one ending each request already has, and removes Core's position counter and held-set computation. If Dickson keeps ADR 0006's position rule, Option A as the slice design computes it is the fallback, and nothing else in this study depends on the choice.

## What the simulator then is

Under the recommendations:

- One test thread runs Core's step, the listener's, each outpost's and the supervisor's root reducers.
- A seeded scheduler chooses which pipe delivers next, keeping each pipe in order. It also chooses each executor job's latency, including never, and when a process ends.
- Time is the scheduler's clock, passed as `now`.
- A run is replayed exactly from its seed and the commit, and a failure is shrunk by the seed's choices, as CanopyCheck shrinks its inputs.
- The check against NVDA feeds recorded tells and answers into Core alone and compares its `Say` effects with expectations written from NVDA's source or captures already made. `cargo xtask nvda compare` stays exploration only.
