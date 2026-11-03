# Owner endings: when an owner ends, nothing of it remains

Status: designed on 2026-11-03, amended after its adversarial review and with patterns from other software checked against their sources; decided by Dickson the same day; to be built before the focus pipeline rework's third build (`docs/roadmap.md`). Read from the code at e4584cb7.

## The problem

Each component ends an owner (an outpost generation, an application's process, a terminal node, a reading thread's application) with a hand-written list of resets that is not tied to the state it clears, so state added later outlives its owner. Recorded defects of the class: 886d8e73, e282e7ea, e0092292, 894f9a34, 76180512, 61a4bfa1, 1bfb6f87, df4c15d9, 1a3dc022 (L1 rows in `experiments/logic-tests-study/fix-commits.tsv`).

## What outlives its owner today, each confirmed by reading the code

- S1, the supervisor: an application's outpost failed twice and waits out a 1 to 30 s backoff (`crates/verbatim-supervisor/src/lib.rs:110-116`); if the process exits meanwhile, `start_due` (`owner.rs:1134`) calls `incarnate` (1026), which checks only that the record exists, so a spare is used up or an outpost launched for a dead process, which then fails and is ended by `outpost_failed`'s exit check (1167). It heals itself; the cost is one wasted start and its error log.
- S2, Core and the app shell: the terminal quiet-period timer is a copy in the shell (`main.rs:1009`) armed only by `Effect::TerminalTimer`. `drop_models` emits no cancel, and it serves all three ways a model goes: the outpost's end, eviction at the 16-model cap, and `forget_text_of` (`reduce.rs:474`), which drops the focused terminal's model when its reader reports the element gone or unfocused while the focus stays; the next snapshot makes a fresh model for the same node, and the stale timer then runs its rules at a time they never asked for.
- S3, Core: `TerminalSpeechState` (`state.rs:708`) is reset only when the focus moves. After an outpost ends, `request` (`terminal.rs:384`) asks the dead node for more output, each request failing at once in the shell with a warning, and `group`, `held` and `deciding` survive.
- S4, the listener: the readers' `wakes` map (`focus_read.rs:1623`) keeps one condition variable per process ever read; small, and `Waking::All` is sent only at shutdown.
- S5, the listener: a reading thread started for a process whose outpost never reached Ready (the process exited, or its start failed) is never released, because `Generation::removed` (`crates/verbatim-core/src/supervisor/generation.rs:619-640`) sends `HookRemoved` only from Ready; the thread and its UI Automation client wait for the listener's whole life.
- Dead code: no production code sends `SetBackendOverride` to the listener or the outpost (only `protocol/tests.rs:75`, `supervisor/tests/idle_round.rs:180`); the listener's `forced_uia` map (`listener.rs:800-1206`) exists for it alone.

## Alternatives weighed

- A serde walker over Core's state, finding every outpost id left after an ending against an allow-list: it sees only Core's pure state and ids carried as newtypes, so of the defects above it would catch only S3's shape, and of the recorded ones none outside Core; state that concerns an owner without carrying its id (`terminal.waiting`, `group`, `held`) is invisible to it. Kept only as a check inside Core's generated test, at quiescence, with an allow-list of exact field paths so that a new field changes one reviewed file.
- One level-set lifecycle message to the listener in place of three edge messages: rejected. The edges carry what a level loses (`HookRemoved` triggers `outpost_ended`; a new hook needs a fresh `through`), and the listener would keep a copy of the supervisor's record to diff against. S5 is fixed by sending the release from every stage instead.
- All shell timers as views of Core's state: a held key's deadline cannot be one, since it is measured from when the shell runs the effect and the state keeps no due time; only the terminal timer and the toggle reads can.
- Merging the listener into another process: rejected; D13's measured reasons stand.

## Patterns from other software

Each checked against its primary source (Chromium's Mojo bindings and RenderProcessHost headers, Erlang's process and supervisor references, ZooKeeper's programmer's guide, Kleppmann on fencing, Kubernetes' controller guidelines, Microsoft's job object and wait documentation, the session-types crate and std::mem::forget, FoundationDB's testing page). The distinction that runs through them: fencing, a generation that makes a receiver reject a stale owner's messages, stops stale input; it does not remove stale state, and every defect above is stale state.

- Already adopted where it pays: one death signal per child (its pipe's end, `owner.rs:538-555`), an ending ordered after everything the child wrote (`lib.rs:52-57`), fencing by generation at every boundary (the only pattern that pays only when global, and already a rule of record), kill-on-close jobs (`verbatim-process/src/lib.rs:289`), consuming lifecycle typestates, a one-for-one supervisor with backoff, and answering when dropped (`Responder`).
- Adopt, locally: state owned by the object whose life is its owner's, as Chromium's per-process hosts and ZooKeeper's ephemeral nodes are (recommendations 1 and 4); and Erlang's rule that a monitor yields an ending from every state, including a process that never existed (`noproc`), as recommendation 3.
- Adopt where one owner holds both sides: level-triggered views, for shell timers and the supervisor's start decision only; made global they would copy another owner's state.
- Later: the operating system reporting a target's exit, as an input to the supervisor at M6.2, when its orderings can be generated and tested; a joint simulation of the supervisor's, Core's and the listener's reducers with endings injected at every step, after M6.2, since S2 and S5 each cross two reducers and each reducer's own test sees half.
- Not worth it here: leases (one machine, where a closed pipe reliably reports a death), more supervision machinery (a restart cleans the child, while the stale state is in the parent), session types (the arms are exhaustive already; S5's arm existed and did the wrong thing), level-triggered messages and snapshots everywhere (copied state, and re-reads that are cross-process calls on the event path).

## Recommendation

1. State owned by its owner's record: Core's terminal speech state moves into the focused terminal's model (or is keyed by its node), so `drop_models` drops it with the model; together with item 2 this fixes S2 and S3 by construction. Asking a dead node stops because there is no model to ask for.
2. The terminal timer becomes a view: the shell reads the focused model's due time after each step instead of keeping a copy, so a dropped model has no timer; the toggle reads too, once each stores its due time. Two rules make it safe: only the focused model's due time counts, and a due time must move past the time it fired at, checked by a debug assertion and a generated-test postcondition, since a view that kept the same due time would fire every loop.
3. The release in every stage: `Generation::removed` sends the listener's release from every stage, and the supervisor sends it when it declines to watch a process, fixing S5.
4. The listener's per-process records in one map, removed by a `Forget(pid)` effect the schedule emits wherever it removes an application (four sites), which is safe because removal requires no thread in charge or abandoned; fixes S4.
5. S1: `start_due` checks whether the process has exited when it decides; process exit as an input stays in M6.2.
6. Delete the listener's `SetBackendOverride` and `forced_uia`, keeping the test's direct `FocusReader::forces_uia`.

None adds a wake-up, a lock or a cross-process call on the event path; item 2 removes a stale wake.

## What is spoken when a terminal's outpost ends with output waiting

What Core has already read waits only because Core hands speech two groups ahead. NVDA puts every line it reads into its speech queue, which an object's death does not cut, so matching NVDA means what was read is still spoken (to verify from the pinned source before building). So: speak what was read, stop asking the ended outpost, clear the hold. When the outpost failed and was replaced, the program still runs and its later output is read through the replacement as usual.

## Decisions

Dickson, 2026-11-03:

- The rule is a design principle in `CLAUDE.md`: state about an owner lives in that owner's record, so its ending drops it; every stage of a lifecycle emits its ending, including stages before the peer exists; a check against an ended owner's ids runs in the generated tests.
- The shell's terminal timer and toggle reads become views of reducer state, read after each step; held keys' deadlines stay effects, since they cannot be derived.
- The backend override, sent by no production code, is deleted from the listener's and the outpost's protocols.
- Built now, before the focus pipeline rework's third build: Core and the app shell first, then the supervisor and the listener.
- Process exit as an input to the supervisor stays in M6.2, with S1's check now.

## Limitations

The generated-test check sees only what generators fill; the hand-written effects of an ending (answering waiters, stopping speech, telling another process, releasing COM objects on a thread allowed to block) stay hand-written; the store's node numbers stay bare integers.
