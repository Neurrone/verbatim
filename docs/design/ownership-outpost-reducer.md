# The outpost's reducer build

Status: built in two builds, the store build and the reducer build, with the departures listed below. Designed on 2026-10-31 against main at 98b0388d, with its adversarial review's findings folded in the same day (verdict: achievable with fixes). It applies [the ownership model](ownership.md)'s rules "Pure reducers, with calls as effects", "No state lent across a call into an application", "Nodes are released by message position" and "Queries have exactly one answer" to the outposts. The refactor preserves what is heard: this build changed no speech. It undid recently finished work in two places, both by design: the commit point and its lock order (`docs/design/focus-pipeline-rework.md`, "The outpost's commit point"), and the hand-over's counting (`outpost/handover.rs`), which the hold Dickson decided on 2026-10-25 replaced. The output build ran beside it and shared no code. The mechanism is in `docs/crates/verbatim-outpost.md` ("The outpost", "Text and caret", "The arbitrator", "Who calls into an application"), `verbatim-uia.md` ("The node registry") and `verbatim-ia2.md`; `docs/performance.md` has the costs ("The outpost's reducer", "The outpost's object store"). This document keeps the problems, the choices and the limits.

## The problems

- One worker thread planned, judged and called, reading and writing shared state between calls: the follow state under the watch lock, `held`, the arbitrator, the text records, the hand-over and the intake's own state. The intake had several writers, among them the watchdog, and its push-time drops read flags the worker wrote.
- The watchdog abandoned and published under the watch lock, and every change of follow state was a commit under that lock with the judgement list, intake, outbound queue and both registries nested inside it. The publication at the bound took four review rounds (c47d57ae, 6dbcca25, 2bd69a85, 8a087fe7), and the gap record moved among its writers four times.
- The anchors' guards were held across application calls in nine places, and text objects were dropped under the text locks, so a worker abandoned inside a call kept what it held and its replacement blocked on it (d0ec3a89, 5797c075).
- A query was answered from the watch's `running` slot by whichever of the worker, the watchdog and the I/O thread came first.
- Neither registry had an outer lock or a generation in its ids, and the watchdog minted under the watch lock at the bound, so identity and its release were not one owner's.

## What was chosen, and why

The build split in two along the object store's API, which neither half changed for the other: the store build first, on today's worker and watchdog, then the reducer build. Reverse order would have built programs against registries that then changed.

- The object store (`outpost::store`) is a monitor owning both node registries, the text objects and the terminals' kept ranges, and every node's and anchor's mint tag, the worker generation and request that issued or looked it up. Reason: identity and its release are one fact. Its API returns owned clones and owned removed objects, never a guard, and takes no closure, so a caller can never hold its lock across a call. The walks in `verbatim-uia`, `verbatim-uia-rops` and `verbatim-ia2` take an interning trait each crate defines, which the store's handle implements, tagging every mint and look-up in the same hold.
- A message that carries nodes is built from a `Recorded`, a type only the store makes, and the outbound queue records each carried node at the message's position as it numbers the message. `Store::release`, under the backlog's lock and then the store's, frees a node or anchor only when no live request can still name it: its tag names a dead generation or a claimed or ended request, and either it was never carried or Core acknowledged a position at or after its last carrier, no waiting message carries it and Core does not hold it. A live tag keeps a node whether or not it was carried. A program's copied lines are tagged with its request at dispatch, so a walk that stops at a copied entry never names a node a release freed. Release returns removed objects owned, so nothing is dropped under a lock. A dead generation's mints wait for the next `NodesHeld`.
- Text calls work on clones of the anchors they name and put changes back only while their request is live, which ends the anchors' guards held across calls and 5797c075's workaround.
- The reducer (`outpost::reducer`) holds every remembered fact of the application with no Windows type: the intake's order, the gap record and its timer, the arbitrator's verdicts, the unreleased reports, the follow state, the hand-over's hold, the worker generation in charge with its deadline, the grace and the walk's bound, the queries not yet answered, and the text facts. Inputs come from a mailbox that every producer only appends to; effects dispatch a program, send a message, release, move the subscription, abandon a worker or set a timer. Local reads that never wait on an application reach it through an injected trait. It runs on a thread of its own because UIA calls cannot be cancelled (Dickson, 2026-10-30). `cargo xtask ci` holds its directory to the pure rule's allow-list; replay tests are the real check.
- The shell waits on the mailbox's alarm (an event and a high-resolution timer set to the reducer's earliest timer), so the watchdog thread and the gap record's threadpool timer are gone and each timer costs one wake where the watchdog took two. This departs from the draft, which kept a watchdog posting timer inputs. It is sound because every lock the reducer's thread takes is a short hold with no call and no wait.
- Workers run whole call programs (an entry's handling, the gap's read, a text operation, a probe, owed work, the console poll, a release's drop) on copies, each making exactly today's calls, so every `call_counts` test passed unchanged. A program posts each commit as it makes it, and the reducer applies it only while the program's request is live. Abandonment is the reducer's transition on the deadline: it publishes an armed walk, marks the generation dead, answers the query "abandoned" and starts a replacement. Late results of a dead generation or a claimed request are discarded; none of the four publication rounds can recur, because the worker writes no follow state and sends nothing to Core.
- The console caret poll is one program on the worker's own high-resolution wait, one wake per 5 ms tick as before, yielding at once to a waiting program other than a release; a release waits at most the poll's 50 ms bound. A caret event of the watched console rings the worker's inbox, so the poll yields and the event's own check reads at once. This departs from the draft, which had the poll's next tick serve as that check, a delay of up to 5 ms.
- An edit control's caret read is cut in two programs where its comparison of selections must know whether the control's value change waits, because the reducer owns the waiting entries; the extra hop is paid only for a selection in an edit control.
- The hand-over's hold: the reducer holds the hook's whole stream from its install until `ForwardingEnded`, drops the copies the listener forwarded, and releases the rest in the hook's order as one batch, newest first. Reason: holding the whole stream keeps the hook's own order, which the old-focus-first rule, the caret-after-update rule and the caret watch read, where holding only the forwarded kinds would let a caret or value change pass a focus raised before it. `TWIN_BOUND`, the multisets and the hand-over's lock went.
- Every query gets one answer from a reply token held in the shell's request table, never in the reducer's state (accepted by Dickson, 2026-10-31); its `Drop` answers `QueryOutcome::Failed`, since the outpost's process lives. The I/O thread no longer answers `Cancel` itself.

## Departures from the design as built (pending Dickson's acceptance)

Store build:

- Parked event objects were not built in the store build, since their lifecycle needs the reducer build's mailbox; the reducer build parks a UIA event's element and an active text position's range as they are pushed, after the push-time drops.
- There is no separate map from window handle to node: plain windows and a foreground's client object mint by window handle through their MSAA keys, so ids do not change.

Reducer build:

- The owed work and the gap's read a dropped copy asks for are programs of their own, dispatched before anything else, which costs two more wakes each.
- A worker generation's charge holds the walk's progress slot and whether the generation was abandoned under one lock, so a walk's step and a publication's copy judge liveness in the same hold, as the watch lock did.
- What the shell lets go of (an element no longer followed, a parked object never taken) is dropped by the worker before its next program, under that program's deadline; a `NodesHeld`'s release is a drop program of its own.
- A focus walk's object is its event address or runtime id, so nothing is parked for it.
- Text support per node is divided: the facts are reducer state (`reducer::text`), the ranges, patterns and what a text supports stay in the store under keys.

## Limitations

- At an outpost's start the hook's whole stream waits for `ForwardingEnded`: median 20.6 ms, at most 591.3 ms over 126 starts ([the remaining focus builds](focus-pipeline-rework-remaining.md), "The hand-over's hold"). A hold whose `ForwardingEnded` never comes lasts until the listener's reads end.
- A release waiting behind the console poll waits up to 50 ms.
- A dead generation's mints wait for the next `NodesHeld`, at most the relay's 3-second ping.
- Wake-ups and latency rose: an event with a commit usually costs four wakes where the worker alone woke once, and each program hand-off costs a `SetEvent` and a wait's return; the figures are in `docs/performance.md`.
- The fetch configuration and the backend override are still written by the I/O thread and read by programs and the event thread; their version is M5's.
- The purity check reads text.

## Decisions

- Built as recommended in the design's open questions, and not yet answered by Dickson: the split into the store build and the reducer build in that order; folding the watchdog and the gap's threadpool timer into the reducer's timed wait; holding the hook's whole stream until `ForwardingEnded`.
- The reducer runs on a thread of its own (Dickson, 2026-10-30); the outposts' reply tokens are built here (accepted 2026-10-31).
