# Ownership audit: the structure between processes

Status: read-only audit, 2026-10-30, one of four. Scope: the supervisor and the app (`crates/verbatim-app/src/main.rs`, `crates/verbatim-outpost/src/supervisor/`), `verbatim-process`, the messaging layer (`docs/design/communication.md`, `docs/design/wake-ups.md`, `crates/verbatim-outpost/src/protocol.rs`, the writers and outbound queues), the control plane and its clients, and the planned milestones that change this area. Read against `docs/architecture.md` and the study's rules (one owner per fact, pure reducers with calls as effects, queries with exactly one answer, no back-channels, lifecycles as typestates, compile-time rules). The judgement inside the listener and the outposts is left to the other audits except where it crosses a pipe.

Every proposed change below names its evidence: a commit, a review finding, or a concrete race. Where a part complies or no defect is shown, it says "keep as is".

## In short

- The structure between processes mostly complies already. The supervisor's owner thread is a true single owner of lifecycle; queries are answered once through the reducer's request table; node ids are stamped per pipe; process ids are pinned by held handles. Most lifecycle defects in the history (1a3dc022, 8d01bbd1, a39f01f8, de7f481e, d6f627a5) were messages about one incarnation travelling by two paths, and each was fixed by restoring one ordered path. That rule should be written down; it is not stated anywhere today.
- The one back-channel in this area is `CoreFocus`, which the study already removes. Its root in the messaging layer is that `SupervisorToOutpost` has no categories: c0bbf19b added a variant that carries Core's state to two kinds of process, and no type or check asked whether it was a query, a release or a lifecycle message.
- Three gaps produced defects and are worth closing by construction:
  - Requests that wait outside the request table (`focus_now_wanted`, `shell_items_wanted`, and the designed read requests) are not answered when an outpost ends. Evidence: 886d8e73, 8d01bbd1, and finding 3 of `core-flow-review.md`.
  - Merge rules are restated in four queues, and each queue has to remember which messages merging must not cross. Evidence: dd363ecc, where a caret report was merged across a reply and spoken twice, and 913bb0e8, where the held-nodes list overtook requests.
  - The supervisor's per-application `Record` encodes its lifecycle in booleans, options and an `OutpostId(0)` sentinel, so impossible combinations can be written. Evidence: the findings fixed in df4c15d9, and M6.2's invariant (one outpost per application), which the current shape lets the code break today.
- One finding changes the model. M5's rule tables, and the backend override today, are configuration that two processes act on for the same event: the listener decides whether to read, and the outpost decides whether to count. Any change to that configuration is in flight to the two processes at different times. That is the study's verdict class (its item 5, and defect B) carried into M5 hot reload, where it becomes routine. The rules need a fifth kind of message: configuration with a generation, where each fact carries the generation it was judged under.
- The listener and the outposts share one pair of enums, `SupervisorToOutpost` and `OutpostToSupervisor`, and five variants are meant for one role only. This leads to silent drops (`_ => return, // The listener sends nothing else.` in `owner.rs`) and to fields whose meaning depends on which process receives them. M4.4 already moves these types, so splitting them by role and by direction costs nothing extra there.

## 1. Processes and threads in this area

- verbatim.exe (Core):
  - the keyboard hook thread;
  - the router;
  - the reducer thread, which owns `SrState`, the request table, `LiveOutposts`, the idle barrier and the held keys;
  - the relay, which is the supervisor's owner thread and holds every `Record`, the listener record, the spares and the outbox;
  - launch helpers (`verbatim-launch`), retirement threads (`verbatim-retire`), and the hang check;
  - the speech queue, the synth and audio threads;
  - the GUI thread and `verbatim-gui-events`;
  - the clipboard thread;
  - the control plane's accept thread, plus one reader and one writer per connection;
  - the shutdown and session-end threads.
- The listener:
  - its I/O thread;
  - the `WinEvent` thread;
  - UIA's callback threads, which it does not own;
  - the registration thread;
  - one reading thread per application, each with a watchdog;
  - the menu-end threadpool timer.
- Each outpost:
  - its I/O thread;
  - the event thread;
  - UIA's callback threads;
  - the worker, plus any abandoned workers;
  - the watchdog;
  - the target-watch thread;
  - the UIA setup thread and the bounded-call thread.
- Each synthesizer host: one thread.
- Processes outside the jobs:
  - `verbatim-agent`, with connection, job and terminal threads;
  - `verbatim-inspect`;
  - the e2e test process.

## 2. Facts, owners and verdicts

1. An outpost incarnation's lifecycle: starting, given spare, ready, backing off, or ended.
   - Owner: the relay's `Record`, `SpareChild` and `ListenerRecord`.
   - Copies, each derived one-way from the relay's ordered `OutpostMessage` stream:
     - the app's `LiveOutposts`, which has its own `ready` flag;
     - `Frames`' per-stream state;
     - the barrier's set of outposts;
     - the `reincarnated` set;
     - the control plane's `outposts` map.
   - Decided twice: whether an outpost is ready. The relay uses `record.ready` to decide whether to deliver facts, and the app uses `live.mark_ready` to decide whether to send queries. Both come from the same frame, so they agree as long as one stream carries it.
   - Defects:
     - 1a3dc022: a refused `Ended` was lost, and a replacement's `Started`, sent from the launch helper, overtook it.
     - 8d01bbd1: a replacement's notices passed a held `Ended`, so idle barriers waited for good.
     - a39f01f8: frames from an ended incarnation recreated a stream.
     - de7f481e: the parse helper's second path held every barrier.
     - df4c15d9: a failed spare was reported as exited, and a round passed on its end.
     - ded1988a: the failure paths were redesigned into one.
   - Verdict: one owner, and the copies are one-way reports, so the model complies. Two changes:
     - State the rule "one ordered path per source incarnation; helpers report to the owner, never to the app". Evidence: the five commits above.
     - Make `Record` an enum of states whose transitions consume the old state, and give the app's `Live` the same shape. Evidence: df4c15d9's findings, the sentinel, and M6.2 (item 9).
2. Attention and held nodes as the supervisor sees them (`note_views`).
   - Owner: Core.
   - The supervisor keeps a lagging copy for retirement, and its own reading of the foreground, from routed `Foreground` facts, to end a backoff.
   - A stale copy can retire an outpost whose event is still waiting in the reducer's port. The reducer then takes the event, and the node is dead at `Ended`. That is answered "gone", and nothing wrong is spoken.
   - No defect in the history.
   - Verdict: keep as is. It is a lifecycle input, not a judgement of events, and stale values are harmless. M6.2 rewrites retirement anyway.
3. The process that serves a window, which routing depends on.
   - Decided:
     - in the listener, by its pid tag;
     - in the app, by `focus_now_server`;
     - in the supervisor, by `check_served_by`, a debug assertion.
   - All three call one definition (`verbatim_uia::provider_process`) on local facts that do not change.
   - Defects: ece62cdf, 95b2b012 and c35219af. These changed the rule. None came from the places disagreeing.
   - Verdict: keep as is.
4. Ignored processes.
   - Immutable, given at spawn to the listener and the supervisor.
   - `in_ignored_window` stays in the app until M5, by the 2026-10-30 decision.
   - Keep as is.
5. The MSAA hand-over: which applications have their hook in.
   - Owner: the supervisor's records.
   - The listener's map follows `HookInstalled` and `HookRemoved`, and the outpost counts the two copies.
   - Defects:
     - difference 25: events are lost between an outpost's death and `HookRemoved`;
     - 89f94d33: counts grew for an outpost's whole life;
     - ec3fe1a1, d40f97ac, 71126b8d and 5d179968: ordering within the hand-over.
   - This is the study's two-route class.
   - Verdict: the ownership complies, and the residual is inherent while two hooks exist. Change: model the hand-over as a typestate in the supervisor, with `ForwardingEnded` accepted only in the state that names that incarnation, which the code checks at run time today. Evidence: difference 25 and 89f94d33, both found late because the states were implicit.
6. Facts held while an outpost starts (`HeldFacts`).
   - One owner, the relay. Released in order on `Ready`, and handed to the next incarnation with `Reincarnated`.
   - Keep as is.
7. Readiness for input.
   - Written by three threads: atomics, the outposts map, and a live read of the foreground inside `is_ready`.
   - Defects: 46b993fa, ff25f3dd, 913de73f, and 11f2d80a with its reversals 7e80b866 and 5c64da12.
   - Every one of these changed what readiness means. None was a race between the writers.
   - Verdict: keep the mechanism as is. Dickson owns its definition.
8. Requests and their single answer.
   - Owner: the reducer's request table, created in 429d50f0. The first outcome wins, and an ended outpost's requests are answered "gone".
   - Outside the table:
     - `focus_now_wanted` and `shell_items_wanted`, which are wanted but not yet sent;
     - Core's deadline for shell items, which is set separately from the outpost's;
     - the second build's designed read requests.
   - Defects:
     - 886d8e73: a shell-items request was never answered when its outpost ended or backed off;
     - 348262b8: two deadlines measured from different times;
     - 8d01bbd1: a focus was asked of an incarnation that would never answer;
     - `core-flow-review.md` finding 3.
   - Change: a responder type covering sent and wanted requests alike (section 3, item 2).
9. One outpost per application at any time.
   - Not held today. M6.2 records that a failed outpost is retired on its own thread while its replacement starts, so for up to `SHUTDOWN_LIMIT` two Verbatim processes call into one application, where UIA calls collide.
   - Change: M6.2 builds it as a typestate. `start_incarnation(pid)` takes a proof that the previous process has exited, which only the retirement's report can produce. Evidence: the roadmap's M6.2 entry and `docs/design/listener-without-connections.md`.
10. The idle barrier.
    - Owner: the reducer, with the relay's stage.
    - An in-band round. An ended child counts as answered, and a reincarnated one hands its place on.
    - Defects: df4c15d9, 8d01bbd1, de7f481e and d6f627a5, all fixed by ordering.
    - Verdict: complies. Keep as is, and express "ended counts as answered" through the responder of item 8.
11. Merging in queues: the writer to a child, the outpost's outbound queue, the listener's sender, `HeldFacts`, and the intake.
    - Each queue restates the key and the messages that merging must not cross: replies, idle answers, the hand-over, markers, and `NodesHeld`.
    - Defects:
      - dd363ecc: a caret report merged across a reply;
      - 913bb0e8: the held-nodes list overtook requests;
      - 89bbbc7c, 71322fae and 06256225: the relayed fact key changed three times on 2026-10-21;
      - the study's item 12: merged evidence lost.
    - Change: section 3, item 4.
12. `CoreFocus`.
    - A back-channel. It goes, by the study.
13. The backend override, `SetBackendOverride`.
    - Sent as two messages. Its own comment says the listener and the outpost "can differ while one is in flight".
    - It is test-only today and has no recorded defect.
    - Keep as is now. M5's tables need the generation rule (section 3, item 5).
14. `Fetches`, the details the theme wants read.
    - Core's configuration, pushed to the outposts.
    - A stale value only changes which details are read, and only for the moment after a theme changes.
    - No defect. Keep as is.
15. Node ids and their release.
    - Ids are stamped with the incarnation by the relay (36be35d8).
    - `NodesHeld` acknowledges a message position. It was fixed by 913bb0e8.
    - Keep as is, plus the study's release rule by message sequence.
16. Process identity.
    - `Target` holds a handle on the process, so its pid cannot be reused while held (510b7f82, 832df39e).
    - Complies, and the type enforces it. Keep as is.
17. The control plane.
    - `DumpFocus`, `DumpTree` and `AwaitIdle` are posted to the reducer's port and answered once. `DumpTreeGivenUp` withdraws a dump.
    - Status reads copies whose only writer is the reducer thread.
    - `MessageReader` (55617468) and the refusal of a pipe someone else created (848dcc77) are fixed.
    - Keep as is.
18. The synthesizer host.
    - Frames name their utterance, and each utterance has one ending (D17).
    - This complies, and it is the model for item 8.
19. The e2e agent's UIA reads into the applications under test.
    - A third UIA client in the application, accepted by `docs/design/listener-without-connections.md`, section 4.5.
    - Keep as is.

## 3. Encoding the rules in the messaging layer

M4.4 moves every message type into a platform-neutral crate. Build these there, so that every later component, the extension host's commands included, gets them by default:

1. Enums split by role and by direction:
   - `CoreToListener`, `ListenerToCore`, `CoreToOutpost` and `OutpostToCore`;
   - the synthesizer host's pair, which exists already.

   Evidence:
   - the silent `_ => return` in `listener_message`;
   - the `pid` field of `SetBackendOverride` and the `focus` field of `CoreFocus`, which mean different things to different receivers;
   - five variants documented "sent only by the listener".
2. A category for every variant, declared in the type, with an xtask check that lists the Core-to-child variants and fails on one with no category. The categories:
   - query, with a paired reply type and a request id;
   - cancel;
   - release (`NodesHeld`);
   - lifecycle (`Assign`, `Ping`, `IdleQuery`, `Shutdown`);
   - routed report (another child's fact, routed but not judged: `DeliverFact`, `ForwardingEnded`);
   - configuration with a generation.

   A variant that carries Core's state for a child to judge against fits no category, so it fails the check. Evidence: c0bbf19b and the study's defects 1 to 3, which followed from `CoreFocus`.
3. A responder for every query.
   - `begin` returns a `#[must_use]` pending entry. The table holds it by incarnation, or by process while it is wanted and not yet sent. Its `Drop`, or the incarnation's `Ended` or `NotWatched`, answers "gone".
   - On the child's side, the worker takes a reply token, and the watchdog's abandonment consumes it as "abandoned".
   - Evidence: 886d8e73, 8d01bbd1, df4c15d9's idle round, and finding 3 of `core-flow-review.md`.
4. Merge behaviour declared once, on the message type:
   - `merge_key()`, where `None` means never merged;
   - `is_barrier()`, true for replies, idle answers, hand-over messages and markers, which nothing is merged across.

   Every queue calls these and states only its bound. A message that carries evidence a merge would lose, such as `earlier` or an answer, declares itself never merged. Evidence: dd363ecc, 913bb0e8, the key churn of 2026-10-21, and the study's item 12.
5. Configuration as a versioned value.
   - Core is its only writer. The relay sends each new generation to the listener and the outposts in one pass.
   - Every fact judged under configuration carries the generation it used, and the receiver recognises a judgement made under an older generation instead of trusting it. Today this covers the backend override and the class tables; in M5 it covers the rule tables and their hot reload.
   - Evidence: the comment on `SetBackendOverride`, the study's item 5 ("a change to the class tables or an override can be in flight while one process has it and the other does not"), and defect B.
6. One ordered path per source incarnation.
   - `OutpostMessage` can be built only inside the owner module, and `EventSink` is reachable only through its `Outbox`, so no helper thread can emit a lifecycle notice.
   - Evidence: 1a3dc022, 8d01bbd1 and a39f01f8. 570fa9ca already removed the last second path.
7. Lifecycle typestates in the supervisor (items 1, 5 and 9 of section 2).
   - A later and optional step: the owner thread's decisions as a pure state machine, with the clock and I/O completions as inputs, so the lifecycle orderings above become replayable unit tests, as Core's are.
   - Evidence: every lifecycle defect in item 1 was an ordering between the owner and the app, found by review or by a live run.

Keep as is:
- the transport;
- the framing and raw sections (4c564340 already made an incomplete PCM frame unwritable);
- the per-pipe overflow policies;
- the single relay;
- `message_kind` sniffing, which a test ties to serde's names;
- pings and pongs read by the relay.

## 4. Planned milestones

- M4.4:
  - Adds no process, thread or fact.
  - It is where items 1 to 5 of section 3 belong, since the types move anyway.
  - It also removes `verbatim_core::SNAP_RESULTS_ACTIVITY` from the protocol crate, a dependency of the wire types on Core's policy.
- M5, the extension runtime:
  - New in outposts:
    - module instances on the worker, each with a lifecycle of loaded, swapping, faulted and unloaded;
    - wasmtime stores, with an epoch driven by the existing watchdog.
  - New in Core:
    - the folder watcher;
    - compilation, off the reducer thread;
    - the merged rule tables;
    - the application identity per outpost.
  - New messages:
    - tables to the listener and to each outpost;
    - load and swap commands;
    - `on-command` runs, Core to outpost to Core;
    - module output effects, outpost to Core;
    - the install request, on the control plane.
  - Rules:
    - The tables are configuration with a generation (section 3, item 5), and every fact the listener judged by them carries that generation.
    - Load and swap are queries answered once per outpost, so "Reloaded" is spoken from the answers. Only the reducer speaks, so the watcher posts an input.
    - `on-command` is a query with a responder.
    - The `focus()` import is the outpost's own followed focus, never Core's, as the study's answers file says.
    - Output effects are one-way reports. Core enforces attention when they arrive, because attention is Core's fact; the design's "enforced by attention" should say so.
    - Application identity is a pure function of the held process, computed by one function. The supervisor, the listener and Core each call it; it is not computed separately in each.
    - Hook verdicts are inputs to the outpost's judge, by V1.
- M6.2, one outpost per application:
  - No new process.
  - Rule: the incarnation typestate of section 2, item 9.
  - The replacement's start waits on the retirement's report, a fact the relay already receives.
- M8.1, UIAccess:
  - A second Core instance (`--secure`) on another desktop, possibly at the same time as the user's.
  - Outposts and the listener able to read windows at higher integrity.
  - Rules:
    - The two instances share no writable fact: a read-only copy of the configuration, no control plane, and a single-instance takeover scoped per desktop.
    - The integrity of the window in front stays a fact reported by the listener (`ForegroundWindow`). It is never re-read inside a decision.
  - Which processes carry UIAccess (Core, the listener, the outposts) changes nothing about ownership, because their pipes are inherited.
- M9, logging:
  - A ring in Core, written by every thread, and possibly outposts' log lines sent to Core.
  - Rules:
    - Logs are one-way reports that nothing reads back to decide anything.
    - Outposts' lines travel in batches, off the event path, so they add no wake-up per event.
    - The viewer reads a snapshot.
    - The Verbatim+F1 report is a query to the navigator's outpost.
- M10, scripted modules and the console:
  - A console thread in Core.
  - An interpreter instance in the outposts that host scripts.
  - Rules:
    - The console's node reads go through the request table with responders, even though they are synchronous to the script.
    - Its snapshot variables (`focus`, `state`, `recorder`) are answers to a query posted to the reducer thread. No other thread reads `SrState`.
    - Session rules are configuration with a generation, installed by a recorded input.
- M12, remote support:
  - A TLS transport, either a thread in Core or a process of its own.
  - A remote peer that is not a child and may run another version.
  - Rules:
    - Remote input is an input posted to the reducer's port, in one order with keys.
    - Speech mirroring is a one-way stream.
    - The remote side never reads Core's state except by a query.
    - The protocol is versioned and validated as untrusted; `PROTOCOL_VERSION` is the precedent.

## 5. What the rules do not cover

- The order of events across applications. It comes from timing in separate processes, and is confined to Core's stale rule and the shared clock.
- The applications themselves: UIA's own threads, the hung window that holds a process's UIA events, and calls that cannot be cancelled. These are bounded only by deadlines and abandonment.
- Decisions taken on wall-clock time in the supervisor: backoff, retirement, wedge verdicts, and readiness's live read of the foreground. They are single-owner but not replayable, unless the owner becomes a pure state machine (section 3, item 7).
- Shared memory between threads in Core: the writers map, the status map, the control registry, and the speech manager's and ledger's locks. "One owner per fact" applies by review only; no message type enforces it.
- Facts across instances: the machine-wide control pipe name (848dcc77), the single-instance takeover, the e2e run lock (9ead3de8), and M8.1's second instance.
- Peers that misbehave or are not trusted: a child that sends a message its role may not send, control-plane clients, extensions, and remote peers. Firefox checks these at run time and kills the offender. Here the per-role enums of section 3 make such a message impossible to decode, which is enough for children; remote peers need validation.
- Version skew. "Core and children ship together" holds for the pipes, but not for extensions (whose WIT tracks are versioned already) or for remote peers.

## Nits found (not proposals)

- The doc comment of `SupervisorToOutpost` says "there is no shutdown message", but `Shutdown` exists.
- `OutpostState::Restarting` is never set.
