# The supervisor and the application shell: merged and verified review

Area: `crates/verbatim-supervisor`, `crates/verbatim-app`, `crates/verbatim-faults`, with `crates/verbatim-core/src/supervisor/`, `src/shell.rs` and `src/event_queue/outposts.rs`. Every quotation and line below was checked in main at ba89dca7; NVDA pinned at 732d1ce2. Reviewer A's notes are "A", reviewer B's "B".

How the classes were applied: the M6.2 roadmap entry rewrites the supervisor's owner thread ("The supervisor's owner thread as a pure state machine ... in `verbatim-core`") and the decisions in `main.rs` ("the idle barrier, query deadlines and routing, the exit"). A finding about the owner thread's own state shape, or the idle barrier, is "Planned phase M6.2". A message, constant or app-side copy that the build 1 design of record (`docs/design/readers-tell-core-asks-build1.md`, approved 2026-11-16) already rules out, and that M6.2's rewrite would not otherwise touch, is "Remove now".

## 1. Size

Product lines are counted to each file's last top-level `#[cfg(test)]`, tests after it, plus integration files (`wc -l`).

| Mechanism | Verbatim product | Verbatim tests | NVDA counterpart (pinned) | NVDA lines | Ratio | What the extra lines buy |
|---|---|---|---|---|---|---|
| Outpost and listener lifecycle: launch, generations, spares, liveness, relay, writers, frames, retirement, Core's record, the app's copies | supervisor `src` 3,857; Core `supervisor/` 476 and `event_queue/outposts.rs` 413; app glue (`live.rs` 150, outpost handling in `main.rs` about 300): about 5,200 | supervisor unit 487 and integration 3,935 (9 binaries plus `common/mod.rs` 65); Core `policy.rs` tests 184 and `outposts_tests.rs` 529; `live.rs` 70: about 5,200 | `appModuleHandler.py` 241 to 289 (per-process module made on first use, dropped by `cleanup`, 49) and 609 to 645 (`isAlive`, `terminate`, 37); `watchdog.py` 76 to 257 (alive, asleep, watcher, recovery, 182) | 268 | about 19x product; NVDA has no tests of these | The process boundary of D9 and D13: `process.rs` 132, `relay.rs` 409, `writer.rs` 322, `frames.rs` 161, `retire.rs` 146, together 1,170 (B's figure; A's "about 1,300" was rounded). Spare outposts (ADR 0001, "Spare outposts"). The rest buys nothing: one generation held in four records and judged stale in up to four places (F1), readiness twice (F3), liveness constants twice (F5). |
| Idle barrier and readiness (harness only) | `idle.rs` 264, `harness.rs` 239, the owner's `IdleStage` and `idle_query`, `advance_idle_barrier`: about 600 | `idle.rs` 134, `tests/idle_round.rs` 397 | none | 0 | n/a | Waiting on evidence in the end-to-end suite; readiness held twice (F3). |
| Latency ledger | `latency.rs` 871 | 727 | none (a tool) | 0 | n/a | A measuring tool; about a third of it computes stages from timings that no longer arrive (Q2). |
| Single instance, screen reader flag, session lock | 385 + 186 + 213 = 784 | 14 | `nvda.pyw` 110 to 252 (terminate the running copy, mutex, about 143), `winAPI/sessionTracking.py` (281) | about 424 | about 1.8x | Win32 plumbing; no finding. |
| Composition root and reducer thread (`main.rs`) | 2,346 | 85 | `core.py` `main()` 684 to 1165 (482) | 482 | about 4.9x | Cross-process plumbing; about 350 lines of decisions that M6.2 moves into pure code. |
| Keyboard hook callbacks (`key_reports.rs`) | 437 | 463 | part of `keyboardHandler.py` (the keys pair's area) | | | A second gesture table (F8) and the out-of-memory keep queue (Q1). |
| Fault points (`verbatim-faults`) | 274 | 75 | none (a test tool) | 0 | n/a | 3 of its 11 points serve only the out-of-memory paths (Q1). |

Area totals: product 12,080 (supervisor 3,857, app 6,844, faults 274, Core `supervisor/` 476, `outposts.rs` 413, `shell.rs` 216); tests 7,453 (supervisor 4,422, app 2,229, faults 75, Core 727). A's app test figure (2,163) was low; B's 2,229 is right.

Deletion target, from the findings below:

| Part | Product lines | Test lines |
|---|---|---|
| Remove now | about 430 | about 700 (mostly one shared stand-in and test binary for the supervisor) |
| If Dickson rules for the removal | about 410 (out-of-memory retry 160, dead latency stages 250) | about 900 (out-of-memory 450, latency 450) |
| Planned phase M6.2 | about 250 (owner's generation stages, spare maps, the shutdown option, the idle round's flags, `pending`) | about 100 |

## 2. Findings

| id | title | class | found by | file and line | what goes | product saved | tests saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| F1 | One generation held in four records, stale frames dropped in up to four places | Remove now (the app's two copies); Planned phase M6.2 (the owner's) | both | `record.rs` 43-57; `owner.rs` 190-219, 369, 596-604; `live.rs` 52-57, 112-120; `main.rs` 987-994, 1027, 1045, 1237-1244 | `live.rs`'s `Generation`, `mark_ready`, `starting`, `accept`'s drop; the status map's writes; at M6.2 the owner's `Starting`/`Ready` | about 70 now, about 80 at M6.2 | about 20 | no |
| F2 | A reducer stalled 6 to 9 seconds ends every outpost before reading the pongs queued behind it | Defect | A (B judged only the test) | `main.rs` 2041-2052; `outposts.rs` 303-330; `inbox.rs` 703-708 | nothing; the loop's order is fixed and the stale test replaced | 0 | about 30 removed, about 40 added | only in the stall sequence; needs a capture |
| F3 | Readiness computed twice; Core's copy read only by a test | Remove now | both | `state.rs` 626-633, 792-803; `harness.rs` 57-66, 105-118 | one copy (the app's booleans and status-map scan, per the design of record) | about 50 | a few | no |
| F4 | `OutpostMessage::Running`: a fallback reachable only by a logic bug, untested | Remove now | both | `owner.rs` 929-949; `main.rs` 1000-1015; `lib.rs` 279 | the variant, its owner branch (an error log instead) and the app's arm | about 40 | 0 | no |
| F5 | Liveness and backoff constants written twice; `Timings` configures nothing | Remove now | both | `record.rs` 22-40; `lib.rs` 83-122, 523, 554; `policy.rs` 13-27, 77-81 | `Timings`, `WedgeLimits`, `WedgeReason`; one constant set | about 60 | about 10 | no |
| F6 | One foreground report rebuilt as three types; progress re-wrapped | Remove now | both | `owner.rs` 722-751; `lib.rs` 215-237; `main.rs` 1154-1200 | `ForegroundReport`, `OutpostMessage::Foreground` and `Progress` (handed on as `Listener`) | about 60 | 0 | no |
| F7 | The listener's pong wakes the reducer every 3 seconds and Core drops it | Remove now | B only | `owner.rs` 822-831; `main.rs` 1093-1106; `outposts.rs` 179-182 | the listener's `Pong` to the app; `target_pid` becomes a plain `Pid` | about 10 | 0 | no |
| F8 | The gesture table built twice, with a fallback for disagreement | Remove now | A only | `main.rs` 2115-2150; `key_reports.rs` 43-64, 322-326 | `Tables` and `unwrap_or(OTHER_KEY)` | about 60 | about 40 | no |
| F9 | The application's backoff due time held twice, one copy never read | Remove now | B only | `policy.rs` 95-101, 123-139; `record.rs` 152-162; `outposts.rs` 342-357 | `Backoff::due_us` for applications | about 15 | 0 | no |
| F10 | Dead and stale pieces | Remove now (one item M6.2) | both | see paragraph | `ListenerReady::replacement`, the second shutdown log, `OutpostToCore::NotResponding`, the listener case of `QueueEffect::EndOutpost`, writer variants, stale documents | about 60 | about 20 | no |
| F11 | A wildcard over `Effect` hides any new effect | Remove now | A only | `main.rs` 1622; `event.rs` 1228 | `_ => {}` and `#[non_exhaustive]` | 2 | 0 | no |
| F12 | The owner's spares are two collections keyed by one id | Planned phase M6.2 | both | `owner.rs` 404-407, 1450, 1517, 1818; `policy.rs` 164-173 | one ordered map of spare records | about 40 | 0 | no |
| F13 | The owner's shutdown is an `Option` read in 17 places | Planned phase M6.2 | A only | `owner.rs` 387 and 17 reads | a drain loop of its own; `end_quietly` | about 60 | 0 | no |
| F14 | The idle round's phase is flags beside an enum that names it | Planned phase M6.2 | A only | `idle.rs` 46-55, 193-206 | `clearing` and `Option<HashSet>` become the phase enum | about 30 | 0 | no |
| Q1 | The retry of a post only a system out of memory refuses | Question for Dickson | both | section 4 | the keep queues, three fault points and their tests | about 160 | about 450 | only under memory exhaustion |
| Q2 | The latency ledger's outpost and listener stages are dead since 2026-11-17 | Question for Dickson | A only | `main.rs` 1137-1148; `latency.rs` 149-334 | reconnect, or delete the dead stages | about 250 if deleted | about 450 if deleted | no |
| Q3 | `StatusInfo::live_re_reads` always zero | Question for Dickson | A only | `main.rs` 2286-2288 | the field (control protocol, a public contract) | a few | a few | no |

### F1. One generation held in four records, stale frames dropped in up to four places (both)

The four records of one outpost generation's stage:

- Core: `crates/verbatim-core/src/supervisor/record.rs` 45, `pub(crate) enum Stage { Launching, Starting(Running), Ready(Running), WaitingOut { due_us: u64 } }` (43-57), in `Record::applications`. A's "89 to 101" was wrong; B's "45-57" is right.
- The owner thread: `crates/verbatim-supervisor/src/owner.rs` 190, `enum Generation { Launching { outpost, spare }, Starting { outpost, child, fault, idle_query }, Ready { outpost, child } }`, kept in `records: BTreeMap<Pid, Generation>` (369), moved to `Ready` by its own peek of the frame (`Peeked::Ready => { self.hand_on(...); self.on_ready(outpost); }`, 626-629).
- The app's live map: `crates/verbatim-app/src/live.rs` 52, `pub(crate) enum Generation { Starting(Live), Ready(Live) }`, moved by `self.live.mark_ready(outpost);` (`main.rs` 1236), read by `idle_conditions_clear` (`&& self.live.starting() == 0`, 1422) and the idle status line (1380).
- The app's status map, shared under a lock with the harness and the control plane: `main.rs` 987-994 inserts `state: OutpostState::Starting` on `Started`; 1237-1244 `state: OutpostState::Ready` on the outpost's `Ready`; removed in `launch_failed` (1210-1216) and `on_outpost_ended` (1866-1871); read by `harness.rs` 110-117 and the control plane's status (`main.rs` 2272-2289).

Partial copies besides: the writers map under a lock (`lib.rs` 485-486, `type Writers = Arc<Mutex<std::collections::HashMap<OutpostId, writer::WriterHandle<CoreToOutpost>>>>`), and `Live.position` (`live.rs` 18), set by `accept` to the `position` that `Frames`' `Stream.position` (`frames.rs` 72) numbered.

The drops of a stale generation's frames:

1. The owner (relay thread), outposts: `owner.rs` 596-604, `.is_none_or(|generation| generation.outpost() != outpost)` then "The owner has ended this generation, and told the app; its pipe is read until it ends, and the rest is dropped here."; listener: 704-711, "a frame of a listener that was replaced is dropped".
2. The app's live map, for messages other than tells and answers: `main.rs` 1027, `let accepted = self.live.accept(outpost, position);`, used at 1045, `message if accepted => self.on_outpost_input(pid, outpost, message),`.
3. Core, for tells and window destroys: `crates/verbatim-core/src/entries.rs` 1142-1146, `if !state.queue.admits(*from) => { return Routed::Said(vec![Said::Queue(QueueEffect::StaleFrame(*from))]); }`, with `admits` at `event_queue/driver.rs` 329-334; answers are matched to their request's instance by the request table (`inbox.rs` 337-344).
4. `Frames` (`frames.rs` 122-126, `if stream.failed { return None; }`) drops a generation's frames after one that was not a message, until its end. This is a different condition (a generation being ended for a malformed frame), so A's "three filters" included it and B's "three judges" counted Core instead; each count is right for what it counted.

B's further claim, checked and confirmed: the app's drop cannot fire. `end` removes the generation from `records` (`owner.rs` 1559) before it sends `Ended` (1581), every frame goes through the same `Outbox` (`hand_on`, 687-688), and `outpost_frame` drops a frame of a generation no longer in `records`, so no frame reaches the app after its `Ended`; `tests/arrival_order.rs` 644, `nothing_an_outpost_writes_after_the_supervisor_ended_it_reaches_the_reducer`, asserts this. `live.rs` 5-9 says the opposite ("the supervisor can still deliver messages the old generation wrote before it ended"), and its test `a_message_from_a_replaced_generation_never_reaches_the_reducer` (156-170) feeds a sequence the supervisor cannot produce.

The ruling (`docs/design/readers-tell-core-asks-build1.md`, "The outpost generation record (M6.2's single record)"):

- Line 362: "Core's reducer owns one record per application: none, launching, starting, ready or waiting out a backoff. The owner thread reports, as inputs: ... It carries out Core's effects: start an outpost for an application, end a generation, send to it. Ruled 2026-11-15. Reason: one owner per fact; both threads judge the generation today, kept in step by about ten kinds of notice."
- Line 363, who judges stale frames: "Core drops every frame of a generation that is not the application's current one, and every frame of a listener other than the newest the owner thread reported started (the relay forwards all). ... Reason: today both threads drop them (`owner.rs` 546 to 555; `verbatim-app/src/live.rs` 134), and `listener_message` checks no listener id (`owner.rs` 662 to 767); one judge removes one drop and closes the second."
- Line 367, the copies it calls derived: "verbatim.exe's view of live generations (today's live map, `verbatim-app/src/live.rs`) is derived from Core's record through its effects; ... the harness's readiness ... takes the listener's and the own outpost's readiness from Core's record and the spares' from the owner thread's spare-ready fact, kept as an input that judges nothing ...; the control plane's outpost list is read from Core's record".
- Line 709, under "Added": "The generation record in Core; the owner thread keeps only the process, pipe and pings." Line 726, under "Removed": "The second generation record on the owner thread and the notices that kept the two in step."

So the ruling makes Core the only judge of staleness ("the relay forwards all") and calls the live map, the harness's readiness and the control plane's list derived from Core's record; it leaves the owner only process, pipe and pings. Today the owner and the app still drop, the live map and the status map are written from the supervisor's notices, and the owner keeps three stages. Commit f0e8c4f3 (2026-11-16, "Own the outpost generation record in Core's reducer") says "Readiness and the control plane's outpost list read the record"; the app's harness and status map do not (see F3).

What the roadmap's M6.2 entry (`docs/roadmap.md` 1060 onward) already plans: "One record of an outpost generation's life: today the relay thread and the reducer thread each keep one, kept in step by ten kinds of notice; Core's reducer owns the single record ... (Dickson, 2026-11-15), built with the event queue and focus build" (recorded as built with build 1); "The supervisor's owner thread as a pure state machine ... in `verbatim-core` ... and readiness as one of its decisions"; "The decisions in `crates/verbatim-app/src/main.rs` moved into pure, unit-tested code ... (the idle barrier, query deadlines and routing, the exit)". It does not name the live map, the status map or the drops in the owner and the app.

Sequence where the copies disagree (B, confirmed): Core's liveness due passes for generation 5 of process P (`outposts.rs` 316-328): Core pushes `EndOutpost(5)` and, through `instance_ended` and `replace`, `StartOutpost(P)`. Until the owner takes `EndOutpost`, its `records[P]` is `Ready { 5 }`, the live map has 5 as `Ready`, and the status map says `Ready`, so the control plane's list and the harness's readiness report a generation Core has ended. Nothing spoken changes.

What the owner's stages are still used for (confirmed): `Ready` starts the spares when Verbatim's own outpost is ready or ends unready (`owner.rs` 1495-1497 and 1601-1603, `if pid == self.own_pid && !generation.is_ready() { self.keep_spares(); }`); `Starting { idle_query }` holds the idle query until `Ready` (1490-1492); `Starting { fault }` repeats a fault in the end's log line (1564-1571); and the end's log level (1571-1579). A's claim that the log level "can come from the end reason" is incomplete: the comment there says "A failure before `Ready`, such as a hook that fails, is an error, whose sound a user hears", so the level decides an error sound and needs `Ready` from somewhere.

Proposed now: the live map keeps only the app's own facts per generation (`Live`'s position and held lists), created at `Started`, dropped at `Ended`; `Generation`, `mark_ready`, `starting`, `accept`'s drop, its test and the stale module comment go; `idle_conditions_clear` reads Core's record; the status map is published from Core's record after each step. At M6.2: the owner keeps `Run<OutpostChild>` per generation and its frame drop goes, as line 363 says. Speech unchanged.

### F2. A reducer stalled 6 to 9 seconds ends every outpost before reading the pongs queued behind it (A; the test's status settled here)

Traced step by step:

1. The relay thread reads each outpost's pong and posts it to the reducer's port: `owner.rs` 818-824, `self.outbox.send(OutpostMessage::Pong { outpost, target_pid: Some(pid), abandoned, })`. The owner no longer judges an outpost's liveness: `heartbeat` (1745-1786) only pings outposts ("Core judges an outpost's missed pings and abandoned workers"), and judges only the listener (1775) and the spares (1798).
2. The app turns a pong into `QueueInput::Process(ProcessFact::Pong { instance, abandoned })` (`main.rs` 1093-1106), stamped when the reducer handles it (`queued`, 1935-1941, `at_us: now_us()`); Core's `ponged` sets `running.pong_us = now_us` from that stamp (`outposts.rs` 179-196).
3. Core asks to be woken at the earliest liveness due: `Record::next_due` returns `running.liveness_due()` for every starting or ready generation (`record.rs` 168-178); `liveness_due` is `self.pong_us.saturating_add(LIVENESS_US)` with `LIVENESS_US: u64 = 9_000_000` (24, 72-73); the app stores the wake as `queue_due` (`main.rs` 1693, `QueueEffect::WakeAt(due) => self.queue_due = due`).
4. The reducer loop (`main.rs` 2041-2052):
   ```rust
   let due = thread.next_deadline();
   let next = next_command(&mut command_rx, due);
   let now = Instant::now();
   if due.is_some_and(|due| now >= due) {
       thread.expire();
   }
   match next {
       Next::Command(command) => thread.on_command(command),
   ```
   With `due` already past, `next_command` waits zero time and returns the first command waiting (`recv_timeout`, 2093-2100); `expire` then queues `QueueInput::Due` (1901-1905) before that command is handled, and the loop handles one command per pass.
5. `Due` runs `tick`, which calls `self.outpost_timers()` (`inbox.rs` 703-708); `outpost_timers` pushes `Effect::EndOutpost(instance)` and calls `instance_ended` for every generation with `running.liveness_due() <= now_us` (`outposts.rs` 316-328); `instance_ended` ends the reader and replaces it (`outposts.rs` 158-165). The pongs behind it then arrive for ended generations and are ignored by `ponged`'s generation check.

The sequence: the reducer thread stalls (any long step on it) at T. Each outpost's last handled pong was at most about 3 s before T. The relay keeps reading pongs at about T+3, T+6, T+9 and posting them. When the thread resumes at T+S, its first pass takes one queued command, runs `expire`, and Core ends every outpost whose last handled pong is 9 s or more old: a stall of 9 s or more ends every outpost; one of 6 s or more ends those whose last pong fell early in the 3-second cycle. Every request out to them is ended with its reader, and each outpost is replaced (counted as a failure for its backoff). The defect is real. It breaks the design principle "A decision is made by a process that sees, in order, every fact it depends on": Core judges liveness without the pongs the relay has already read. Swapping `expire` and `on_command` within one pass would not fix it, since only one command is handled per pass; every command already waiting must be handled before `Due` is queued.

The test `a_reducer_held_past_three_ping_intervals_ends_no_outpost` (`crates/verbatim-supervisor/tests/arrival_order.rs` 558-582) cannot fail on this defect: it runs the supervisor with a stand-in reducer, not Core or the app's loop, and the stand-in discards every pong (379-385, "Liveness and the listener's generation are Core's record's facts, and the idle round is only the tests' evidence, none of which these tests judge."). Its doc still says "since the relay reads the outposts' pongs itself" (554-557). Both reviewers are right: A that it cannot fail on the defect it names, B that it tests a judgement the owner no longer makes. The owner stopped judging outposts' liveness in 03efff64 (2026-11-16, "Reduce the supervisor to process facts and Core's effects"), which edited this file but kept the test, written in 8ce718ce (2026-11-11). What it can still fail on: the relay ceasing to ping or to read an outpost while the reducer's port goes undrained (it waits for the stand-in's fourth ping, then requires the outpost to answer and to shut down cleanly); its neighbour `while_the_reducer_is_held_the_relay_drains_every_frame_in_order` (584) already covers the reading. Core's own test `a_generation_with_no_pong_for_nine_seconds_is_ended_and_replaced` (`outposts_tests.rs` 241-250) feeds pongs and `Due` by hand in order, so it cannot see the loop's order either.

Fix (changes no decision): the reducer loop handles every command already waiting before it queues `Due`. Test: the held-reducer test goes (about 30 lines and its `PINGS_ENV` plumbing); one test through the real loop, or a function taken out of it, queues pongs behind a past due and asserts no `EndOutpost` (about 40). Speech: unchanged in normal runs; in the stall sequence, what the ended requests would have spoken needs a capture. A's alternative, the owner judging outposts' liveness, would change the ruling at design line 364 ("Liveness: Core keeps one due time per generation, 9 seconds after its last pong ...; passing it ends the generation. The owner thread sends the pings on its own timer."); the fix above makes that unnecessary, so it is not carried as a question.

### F3. Readiness computed twice; Core's copy read only by a test (both)

`crates/verbatim-core/src/state.rs` 626, `pub struct Readiness { spares_ready, listener_ready, own_outpost_ready }`, and `SrState::readiness` (792-803), documented as "What the test harness's readiness reads of Core's outpost generation record". `grep -rn "readiness()" crates/` finds one caller, `event_queue/outposts_tests.rs` 489. The app computes the same facts itself: `harness.rs` 57-66, `struct Parts { listener_ready, focus_known, spares_ready, signalled }`, set at `main.rs` 1058 (`self.context.readiness.spares_ready();`), 1069 (`listener_ready()`) and 1447 (`focus_known()`), with the own outpost read from the status map (`harness.rs` 110-117). Line 367 of the build 1 design rules that readiness "takes the listener's and the own outpost's readiness from Core's record". Proposed: the reducer thread sets readiness from `state.readiness()` after each step, `Parts` keeping `focus_known` and `signalled` (or, the other way, Core's `Readiness` goes). About 50 product lines. Speech unchanged.

### F4. `OutpostMessage::Running` is reachable only by a logic bug, and untested (both)

`owner.rs` 940-949: `if let Some(generation) = self.records.get(&pid) { if generation.child().is_some() { self.outbox.send(OutpostMessage::Running { ..., ready: generation.is_ready(), ... }); } return; }`; the app turns it into `OutpostStarted` and `ReaderReady` (`main.rs` 1000-1015). Core emits `StartOutpost` at six places (`outposts.rs` 100, 274, 289, 295, 354, 390), each from a record that is none, waiting out a backoff, or replaced after its `EndOutpost` was pushed first (204, 250, 327), and the app executes effects in order, so the owner has removed the generation before the start arrives. `grep -rn "Running {\|OutpostMessage::Running" crates/verbatim-supervisor/tests crates/verbatim-app` finds only the app's arm. The coding standard "Code reaches no fallback that could only be a logic bug" applies: replace the branch with an error log, drop the variant and the app's arm. About 40 lines. Speech unchanged.

### F5. Liveness and backoff constants written twice; `Timings` configures nothing (both)

`record.rs` 22-24: "How long a generation lives with no pong: three ping intervals of 3 seconds (`verbatim-supervisor`'s `Timings`)." `pub(crate) const LIVENESS_US: u64 = 9_000_000;`, and 32-40 `BACKOFF` (1 s, 30 s, 1 min) "(`verbatim-supervisor`'s `Timings` and `STAYED_UP`)", against `lib.rs` 99-106 (`ping_interval: Duration::from_secs(3), missed_pongs: 3, backoff_first: Duration::from_secs(1), backoff_limit: Duration::from_secs(30)`) and 122 (`const STAYED_UP: Duration = Duration::from_mins(1);`). `grep -rn "Timings" crates/ xtask/ uia-facts/`: constructed only as `Timings::default()` (`lib.rs` 523, 554). `WedgeReason` (`policy.rs` 13-18) has one variant, `MissedHeartbeats`, so `wedged` returning `Option<WedgeReason>` is a bool. Drift sequence (B): shorten `ping_interval` to 2 s and the owner judges the listener and spares at 6 s while Core still ends outposts at 9 s; no test compares them. Proposed: one constant set in `verbatim_core::supervisor::policy` read by both; `Timings`, `WedgeLimits` and `WedgeReason` go. Design line 364 already fixes the value as "3 intervals of 3 seconds", so no decision changes. About 60 product lines. Speech unchanged.

### F6. One foreground report rebuilt as three types; progress re-wrapped (both)

`owner.rs` 732-751 copies `ListenerToCore::Foreground { window, root_owner, process, observed_at_us, thread, console, integrity }` field by field into `crate::ForegroundReport` (`lib.rs` 215-237), and 722-731 re-wraps `Progress` as `OutpostMessage::Progress(change)`; `main.rs` 1154-1200 copies the report again into `Input::Foreground(ForegroundWindow { ... })` and `QueueInput::Foreground(ForegroundReport { ... })`, applied by Core to `state.foreground` (`reduce.rs` 214) and to the event queue (`inbox.rs` 206). Tells, answers and destroys already go through one variant, `OutpostMessage::Listener { listener, message: Box<ListenerToCore> }` (755-767). Proposed: hand `Foreground` and `Progress` on as `Listener`, removing `ForegroundReport` and two `OutpostMessage` variants. The two Core inputs keep their current windows (the ghost mapping at 1174-1199 is deliberate, per its comment), so speech is unchanged. About 60 lines.

### F7. The listener's pong wakes the reducer every 3 seconds and Core drops it (B only; confirmed)

`owner.rs` 822-831: `listener.liveness.ponged(now_us()); self.outbox.send(OutpostMessage::Pong { outpost, target_pid: None, abandoned: 0, });`; the app queues it as `ProcessFact::Pong` (`main.rs` 1093-1106), and `apply` runs a reducer step and records a flight-recorder entry (1437-1458); Core returns at once: `outposts.rs` 182, `let Reader::Outpost(process) = instance.reader else { return; };`. The listener's liveness is the owner's (1775). Sequence: Verbatim idle for a minute; 20 listener pongs each wake the reducer thread for a step that does nothing and take 20 of the flight recorder's input slots. Proposed: no `Pong` to the app for the listener. About 10 lines; one wake fewer every 3 seconds. Speech unchanged.

### F8. The gesture table built twice (A only; confirmed)

`main.rs` `bound_gestures` (2115-2150) builds the hook's `GestureMap` from `bindings_for`, `clearing_keys`, `editing_chords`, `selection_bindings` and `caret_bindings`; `key_reports.rs` 45-64 (`Tables::for_layout`) builds lookup tables from the same five functions. Fallback for a disagreement: 322-326, `.get(&gesture.gesture).copied().unwrap_or(OTHER_KEY)`. If the hook's map carried each gesture's action, `Tables` and the fallback go. It touches the keys pair's hook map. About 60 product and 40 test lines. Speech unchanged.

### F9. The application's backoff due time held twice (B only; confirmed)

`policy.rs` 95-101, `Backoff { failures, due_us: Option<u64> }`; `failed` sets `self.due_us` (131); `record.rs` 152-162 calls it and stores the same time again as `Stage::WaitingOut { due_us }`. Only the stage's copy is read for applications (`record.rs` 172, `outposts.rs` 330); `backoff_ended` (342-357) never clears `Backoff::due_us`, while `attended` clears it through `backoff.foreground()` (292). So a serialized record can carry a due time for an application already launching. Only the spares read `Backoff::due`/`ended` (`policy.rs` 188, 198). About 15 lines. Speech unchanged.

### F10. Dead and stale pieces (both)

- `ListenerRecord::replacement` (`owner.rs` 271-273) and `OutpostMessage::ListenerReady { replacement }` (`lib.rs` 321-329): the app matches `ListenerReady { listener, .. }` (`main.rs` 1068). `grep -rn "replacement:" crates/verbatim-supervisor/tests` finds 8 test patterns in 5 files (A said 3); no product reader. The supervisor guide's "a replacement's `Ready` makes Core ask the attention application for its focus" (`verbatim-supervisor.md` 185-186) is not done by this field. About 15 lines. Remove now.
- `ShutdownSummary` logged twice: `owner.rs` 1705-1711 and `main.rs` 526-532 (A). Remove now.
- `OutpostToCore::NotResponding` (`verbatim-model/src/protocol/mod.rs` 441): `grep -rn "NotResponding {" crates/` finds no constructor outside `protocol/samples.rs`; the app drops it (`main.rs` 1259) (B). Remove now (the outpost protocol is internal).
- `QueueEffect::EndOutpost(Instance)` can name the listener, which the app skips (`main.rs` 1697-1703, `if matches!(instance.reader, Reader::Outpost(_))`); all three `Effect::EndOutpost` pushes in Core name `Reader::Outpost` (`outposts.rs` 204, 250, 327) (B). Remove now.
- `writer.rs` 103-115: five `Outgoing` variants, of which `Ping`, `Shutdown`, `HandOver` and `IdleQuery` all bypass the bound (178-183) and only `Ping` and `HandOver` are stamped (130-131) (B). About 15 lines and one of the two bypass tests (531, 549). Remove now.
- `ListenerRecord::pending` (`owner.rs` 274-276) is meaningful only while the listener is launching (B). Planned phase M6.2.
- Stale documents: `process.rs` 1-4 promises "two local process and window queries" and holds one (`image_stem`, 124); `verbatim-supervisor.md` 187-188 says `process` holds "the local query whether an application is hung" (it is `verbatim-app/src/hang_check.rs`); `verbatim-app.md` 334-340 describes `OutpostToCore::NotResponding` applied as `Input::ApplicationNotResponding` with `LiveOutposts::application_of`, and 498-500 `reports_for`, none of which exist (`grep -rn "application_of\|ApplicationNotResponding\|fn reports_for" crates/`: no product hit); `live.rs` 5-9 (F1).

### F11. A wildcard over `Effect` (A only; confirmed)

`main.rs` 1622, `_ => {}`, ends `execute`'s match; every one of `Effect`'s 13 variants (`crates/verbatim-model/src/event.rs` 1229 onward) has an arm, and the wildcard is forced by `#[non_exhaustive]` (1228). Removing the attribute lets the compiler name a new effect's missing arm. Speech unchanged.

### F12. The spares are two collections keyed by one id (both)

`owner.rs` 404 `spares: Spares` (`policy.rs` 168-173, `waiting: VecDeque<(OutpostId, bool)>`) and 407 `spare_children: BTreeMap<OutpostId, SpareChild>`; endings remove from both and use the first's answer: `let listed = self.spares.remove(outpost);` (1517), `if self.spares.remove(outpost)` (1450, 1818). `Spares`, `Liveness` and `WedgeLimits` are defined in `verbatim-core` and used only by `owner.rs` (grep: `owner.rs` 46). One ordered map of spare records removes the agreement. About 40 lines. Planned phase M6.2.

### F13. The owner's shutdown is an `Option` read in 17 places (A only; confirmed)

`owner.rs` 387 `shutting_down: Option<Sender<ShutdownSummary>>`; `grep -n shutting_down owner.rs` gives 19 lines, the declaration and initializer plus 17 reads (A listed 15). `end_quietly` (1589-1596) repeats `end` without the notice. A drain loop of its own after `Shutdown` removes the branches. About 60 lines. Planned phase M6.2.

### F14. The idle round's phase is flags (A only; confirmed)

`idle.rs` 46-55, `struct Round<W> { number, unanswered: Option<HashSet<OutpostId>>, clearing: bool, waiters }`, from which `standing()` (193-206) derives `Standing::{NoRound, AwaitingRelay, AwaitingOutposts, Clearing}`; `clearing` with `unanswered: None` is writable and means nothing. About 30 lines. Planned phase M6.2 (the idle barrier is named among the `main.rs` decisions it moves).

## 3. Test findings

### verbatim-supervisor (integration 3,935 lines, 9 binaries, 28 trials; unit 487)

- Replaceable: the nine binaries each carry their own stand-in, argument reader and `main` (`grep -c "fn stand_in\|fn value\|fn main()"`: 3 in eight files, 1 in `shutdown_fallback.rs`); ccdb2a7a (one new `Supervisor::with_executables` parameter) touched all nine. One stand-in in `tests/common` and one binary of trials (fault-feature trials apart): about 600 lines (both).
- Replaceable, not removable: `ignored_process.rs` (239 lines, 1 trial). `spares.rs` `an_ignored_processs_start_takes_no_spare` (754-776) covers the start path, but only `ignored_process.rs` checks that the listener is launched with `--ignore-pids` (its stand-in exits 0 only when told, 82-90; `grep -rn "ignore-pids" crates/verbatim-supervisor/tests`: 1 hit). A's "remove" would lose that; fold it into the shared stand-in. About 200 lines saved.
- Replaceable, not removable: `listener_destroyed.rs` (190 lines) tests the owner's pass-through of `Destroyed` across real processes; the standing rule asks for tests across the process boundary, and `main.rs`'s `a_listeners_window_destroyed_is_queued_for_core` (2355) runs no relay. As a trial in the shared binary, about 40 lines (B). About 150 saved.
- Replaceable: `arrival_order.rs` `a_reducer_held_past_three_ping_intervals_ends_no_outpost` (558-582) cannot fail on its defect (F2). Remove it with its `PINGS_ENV` plumbing (about 30 to 60 lines); add one test of the reducer loop's order (about 40).
- Removable with Q1: `fault_paths.rs` (280 lines, 2 trials) and `relay.rs` `a_refused_wake_leaves_the_item_for_the_heartbeat_take` (472-521, 50).
- Removable with F10: one writer bypass test (about 15) and the `replacement:` patterns (8 places).
- Kept: `spares.rs`, `target_gone.rs`, `idle_round.rs`, `shutdown_fallback.rs`, the other `arrival_order.rs` trials, the writer's queue tests.

Counts: about 1,000 lines replaceable by the shared binary and the two folds; 1 test replaced (add about 40); 330 removable with Q1; about 20 with F10.

### verbatim-app (unit 2,229 lines)

- Removable now: `live.rs` `a_message_from_a_replaced_generation_never_reaches_the_reducer` (156-170, 15 lines), a sequence the supervisor cannot produce (F1).
- Removable with F8: the table halves of `each_report_maps_to_its_commands` and `each_gesture_maps_to_its_script_command_or_lock_key` (about 40).
- Removable with Q1: `key_reports.rs` `a_refused_key_command_is_posted_before_the_next_keys_commands` (717), `a_refused_speech_cancel_is_posted_before_the_next_keys_posts` (753), `a_pause_toggle_is_posted_behind_a_kept_speech_cancel` (800), with their helpers: about 120 to 190 lines.
- Removable with Q2, if deletion is chosen: about 450 lines of `latency.rs` tests that feed `event_received` full timings the shell no longer gives.
- To add: the reducer loop's order test (F2), about 40.

### verbatim-faults (75 lines)

Keep its tests. With Q1, `PortRefusesPost`, `RelayWake` and `KeyPostRefused` go; the registry tests that use `RelayWake` (`lib.rs` 281-289) and `KeyPostRefused` (347) move to another variant. `ListenerLaunchReported` serves `listener_configuration.rs` only.

### Core, the parts read here

`policy.rs` `a_listener_or_spare_is_wedged_nine_seconds_after_its_last_pong` and `outposts_tests.rs` 241-250 test the two copies of one rule (F5); one goes with the copy. `outposts_tests.rs` 489 is the only reader of `SrState::readiness` (F3).

## 4. Questions for Dickson

### Q1. The retry of a post that only a system out of memory refuses

Every copy:

- The relay's `Outbox`: `crates/verbatim-supervisor/src/owner.rs` 283-341, `struct Outbox { sink: EventSink, refused: VecDeque<OutpostMessage> }`: "One the app refuses for now ([`Undelivered::Refused`]: its port refusing the packet, which only a system out of memory does) is kept, and so is every message sent after it, behind it, and they are delivered again, in order, at the relay's next send and at every heartbeat".
- The refusal's mapping that feeds it: `lib.rs` 136-146 (`Undelivered::{Gone, Refused(OutpostMessage)}`) and 163-178 (`EventSink::posting`, `Err(item) if poster.is_open() => Err(Undelivered::Refused(unwrap(item))),`).
- The relay mailbox's refused wake: `relay.rs` 87-89 (`ERROR_NOT_ENOUGH_MEMORY`) and 101-140: "A wake the port refuses, which only a system out of memory does, is logged, and the item waits for the relay's heartbeat, which takes the queue".
- The hook thread's keep queue: `crates/verbatim-app/src/key_reports.rs` 13-15 (the module's rule), 84-89 (`static KEPT: RefCell<VecDeque<(Post, Option<KeySeq>)>>`, "empty but while a system out of memory refuses packets"), and `Ports::post` and `try_post` (91-172); `SpeechControl::cancel_through` and `toggle_pause` return whether the post was taken so that the hook can keep it (`verbatim-speech/src/manager.rs` 228-230, 252-254).
- The poster that gives the item back: `crates/verbatim-process/src/port_queue.rs` 120-150 (`Poster::send`: "or the port refuses the packet, which only a system out of memory does").

Fault points: `Fault::PortRefusesPost` (`port_queue.rs` 131-134), `Fault::RelayWake` (`relay.rs` 119-127, and `lib.rs` 202-213 `refuse_shutdown_wake`), `Fault::KeyPostRefused` (`key_reports.rs` 132-141). Tests: `tests/fault_paths.rs` `a_message_the_port_refuses_is_delivered_again_in_order_and_shutdown_ends` (176) and `a_shutdown_whose_wake_the_port_refuses_still_ends` (232), 280 lines; `relay.rs` `a_refused_wake_leaves_the_item_for_the_heartbeat_take` (472-521); `key_reports.rs` 717, 753 and 800. Cost: about 160 product lines, 3 of the 11 fault points, about 450 test lines.

The decision that approved it, with its date. The rule as it stands is in `books/dev/src/reference/crates/verbatim-faults.md` line 94: "A post a port refuses is kept and posted again ahead of the next one, by the relay's outbox and on the hook thread, with no timer. Reason: a refusal comes only from a system out of memory, and a timed retry would wake on a clock with no evidence." It carries no ruling of Dickson's; the two bullets beside it that are his are dated 2026-10-31 (fault points behind a feature; the worker that cannot start). It comes from the design "Failure paths and locks" in docs/design/logic-foundations.md (commit 851afc30, 2026-10-31, "proposed on 2026-10-31 and revised after its two adversarial reviews"; the file has since been folded away, bdc186ed), built in 15b2b07a (2026-11-01, "Run the failure paths through fault points"). Its second review (experiments/logic-tests-study/failure-paths-design-review-2.md in 851afc30) rejected the earlier 50 ms timer and recommended the rule built: "Keep the in-order rule without the timer: commands refused while the poster is open are kept and posted ahead of the next key's commands on the hook thread, the same carrier the relay's `Outbox` uses (its next send)", having found that `Poster::send` boxes the item first "and Rust aborts the process when that allocation fails. So the case the timer serves is a narrow one: the kernel refuses the packet while the process's own heap still allocates". The relay's parts are older fixes: de7f481e (2026-10-14, a refused parse result delivered again) and d6f627a5 (2026-10-14, a refused wake no longer leaves the mailbox stuck). The hook's part serves Dickson's ruling in 0d1e7802 that the hook "can neither block nor drop a key ... Decided by Dickson on 2026-10-21". The roadmap's M6.2 also lists "A bound on the outbox of messages refused while a child is busy." No fact changed since these decisions was found. No recommendation is made here, as asked.

### Q2. The latency ledger's outpost and listener stages are dead since 2026-11-17 (A only; confirmed)

`main.rs` 1141-1147 is the only caller of `LatencyLedger::event_received`, passing `EventTiming { observed_at_us: tell.at_us, ..EventTiming::default() }`; commit 3f9e68ac (2026-11-17) removed the two calls that passed an outpost's real timing (`git show 3f9e68ac -- crates/verbatim-app/src/main.rs`: `-self.context.ledger.event_received(trace_id, timing);` twice). `grep -n EventTiming crates/verbatim-model/src/protocol/mod.rs`: only `ListenerToCore::Progress` (508) carries one, and the relay only logs it (`owner.rs` 722-731). So the stages built from the relay, dequeue, await, publish and write stamps, call counts and Core's stamps (`latency.rs` 149-181, most of 203-293, the first stages of 295-334) never appear in a running Verbatim, and `verbatim-inspect` still prints `OutpostRead` with its call count (`crates/verbatim-inspect/src/main.rs` 365). Sequence: press Down Arrow in Notepad; the latency line and `verbatim-inspect latency` show no outpost read or caret wait stage, where before 2026-11-17 they did. Options: reconnect the readers' timings to their tells and answers, or delete the dead stages (about 250 product and about 450 test lines, and `LatencyStageKind` variants of the control protocol, a public contract). Speech unchanged either way. Also `let at = |us: u64| (us != 0).then_some(us);` is written three times (`latency.rs` 150, 205, 297), 0 standing for "not stamped".

### Q3. `StatusInfo::live_re_reads` (A only; confirmed)

`main.rs` 2286-2288: "The live re-read is gone (ADR 0003); version 4's status reply keeps the count, always zero." `live_re_reads: 0,`. Removing it changes the control protocol, a public contract. Speech unchanged.

## 5. Cost of one behaviour

From `git log --stat` (both reviewers; each commit checked with `git show --stat`):

- The listener's window destroyed reaching Core (bc61c881 and 6554b510, 2026-11-19): a wire variant in `verbatim-model`; an arm in the owner's `listener_message`; the app's `listener_input` into a `QueueInput`; Core's handler; a new 190-line test binary with its own stand-in for one pass-through arm, plus an app unit test; six crate guides.
- Background bars not reported (ccdb2a7a and f3218d20, 2026-11-19): `Effect::ListenerConfiguration`, `Supervisor::configure_listener`, `OwnerEvent::ConfigureListener`, the owner's copy of the setting, a launch argument and its parsing, `CoreToListener::Configuration`, a hold point in `verbatim-faults`, a 278-line test binary, and a one-line change to each of the other eight supervisor test binaries.
- The listener's held view (a11ecd70): `send_elements_held` and `leave_elements_held`, `OwnerEvent::ListenerHeld`, `listener_held` with its launching case, `ListenerView` in Core, `send_views` in `main.rs`, beside the outposts' parallel held-nodes path.

What would cut it, merged: listener messages handed through as the wire enum with no supervisor wrapper (F6), so a new report adds no `OutpostMessage` variant or owner arm; one supervisor stand-in and one test binary of trials, so a new hand-off is a trial of about 40 lines and no new binary; a supervisor constructor taking one setup value with defaults, so a new parameter touches no test; one record of the generation (F1, and the owner's at M6.2), so a lifecycle change edits Core's Table 5 and its tests only; and one held-view path for the listener and the outposts, both being the newest view left for the next command.

## 6. Dropped

- B's F7, the owner's copy of "report background progress bars": removing it would rework a mechanism built on 2026-11-19 under the ruling of the same day (`books/dev/src/explanation/design/ownership.md` 29: "Core sends it at launch and on every change as configuration, and the reader judges under a value at most one message old (Dickson, 2026-11-19)"), with no fact changed since; and sending it only as a command after launch would let a new listener judge under a value it was never sent.
- B's duplicate claim for `arrival_order.rs` `a_frame_written_before_a_key_is_reduced_before_it_and_one_written_after_it_after` (504-529): the `main.rs` unit test it names (2396-2430) posts straight to the port and runs no relay, so it cannot fail where the cross-process test can.
- A's "remove" of `ignored_process.rs` and `listener_destroyed.rs`, and A's claim that the end's log level "can come from the end reason": corrected above (replace rather than remove; the level decides an error sound), not kept as stated.
- A's alternative of the owner judging outposts' liveness: unnecessary once F2's order is fixed, and it would change the 2026-11-15 ruling at design line 364 with no changed fact.
