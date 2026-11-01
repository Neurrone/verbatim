# Logic tests: finding ordering defects before review does

Status: proposed on 2026-10-31, revised after its adversarial review (`experiments/logic-tests-study/design-review-1.md`); Dickson's decisions of that day are in `experiments/logic-tests-study/decisions-2026-10-31.md`, and the research behind the alternatives weighed, other projects' experience and the Rust tools, in `experiments/logic-tests-study/research/report.md`. M4.5, the foundations, is built (2026-11-01); M4.6, the generated tests, remains (`docs/roadmap.md`), after the focus pipeline rework's second build, so the tests are written once against the code that stays.

## The problem

From 1 October to 31 October, 586 commits on main had messages naming a fix (`experiments/logic-tests-study/readme.md` gives the method and its caveats). A first classification found 67 defects of ordering, lifecycle or hand-off among 187 behaviour defects. The review's recheck of 35 of those rows found about 10 misclassified and about 7 later rounds of one mechanism, which puts the distinct ordering defects at roughly 35 to 45, those within reach of generated tests of pure code at about 20 to 30, and those in code that survives the rework at about 8 to 12. The invariants proposed were learned from those same defects, so past catches overstate future ones; the counts are upper bounds within about a factor of two.

The defects were found by review rounds, live runs and end-to-end failures on GitHub's runner, often days after their commit, and the ownership refactor's history shows the cost: a mechanism patched three to seven times before its design changed. No test explores orderings: the replay tests check the orderings someone thought of.

## The goal, and how it is judged

The goal is fewer ordering defects reaching review, a run or a user, and an architecture simpler to reason about, at a cost the defects found repay. Two measures, with their thresholds set in M4.6's design and accepted by Dickson before any generated test is written:

- The mutants the generated tests kill that the existing replay tests miss, from `cargo-mutants` run on the reducer crates. Reason: it measures whether the tests detect defects at all, available the day they are written.
- The ordering defects that review, a live run or the end-to-end suite find in the builds after M4.6, each checked against the tests, beside the time each build spent on generators, models and their upkeep.

If the thresholds are not met, the generated tests stop growing and the simulation is not built. The crate split is not judged by them: it is a one-time cost Dickson chose for clean ownership by role (2026-10-31).

## M4.5: the foundations

What was decided, and why. The mechanisms are in the crate guides named in each part; the reviews and audits behind them are in `experiments/logic-tests-study/`.

### Crates by role

- The outpost crate, which held three programs with two-way dependencies, is split by role: `verbatim-outpost`, `verbatim-listener` and `verbatim-supervisor`, with their logic in `verbatim-outpost-logic`, `verbatim-listener-logic` and `verbatim-reader-rules`, and shared window and process queries in `verbatim-win32` and `verbatim-process` ([the crate index](../crates/readme.md)). Reason: names that match roles, and a crate boundary enforces purity where a text scan cannot see through a re-export; the split is a one-time cost Dickson chose for clean ownership by role (2026-10-31).
- Each two-way dependency was broken by moving what both sides used below them, never by an interface between roles, and each fact the split would have copied into two role crates (the moved-on grace, the UIA focus wait) has one definition in `verbatim-reader-rules`. Reason: a role crate that depends on no other role is a rule the graph can hold, and no test could tie two copies in crates that may not depend on each other.
- The tier check (`xtask/src/tiers.rs`) classes every workspace member as platform-neutral, logic, Windows or tool, reads every target's graph over normal and build edges, and fails a platform-neutral crate that reaches a GPL-tier one, a logic crate that reaches a Windows one, or a role that reaches another role. Reason: the old check missed a crate it did not know and a dependency gated on `cfg(windows)`, and its answer must not depend on the host.
- `verbatim-listener.exe` and `verbatim-outpost.exe` are separate executables (Dickson, 2026-10-31), beside `verbatim.exe`, found as the outpost and the synthesizer host are found (Dickson, 2026-10-31).
- The seal stays in `verbatim-supervisor`: the generation's typestates in `verbatim-core` return a plain `Lifecycle` notice and only the owner makes `Sealed`, which keeps the guarantee the seal gives, since the messages carry a Windows crate's frame type (Dickson, 2026-10-31).
- `provider.rs` moves from `verbatim-uia` to `verbatim-win32`, re-exported under the same names, so the window queries have one crate under both backends (Dickson, 2026-10-31).
- `verbatim-ext-api` is classed platform-neutral, so an extension's SDK never carries the GPL tier (Dickson, 2026-10-31).
- The `Arbitrator`'s pure decisions stay in `verbatim-outpost`; M4.6's design decides whether its tests need them (Dickson, 2026-10-31).
- `xtask` builds on any host, its Windows-only modules and their dependencies gated whole, and the `ci-linux` job builds and tests the whole platform-neutral tier, the logic crates and xtask (Dickson, 2026-10-31). Reason: a dependency check reads the graph, while a build on another platform proves it.
- A check-only `cargo check` or `cargo clippy` with `--target x86_64-unknown-linux-gnu` is a stated exception to "no command names a target", since it builds and tests nothing (Dickson, 2026-10-31). WSL is not installed; the GitHub run is the Linux build's check (Dickson, 2026-10-31).

### The outpost's programs and the caret watch

- The program bookkeeping was simplified before any test was written for it, moved here from M6.1 (Dickson, 2026-10-31): verdicts are a message of their own, a commit the reducer does not accept is refused whole, `entered_in_front` is decided by the reducer from the program's evidence, and the worker keeps nothing between programs. Reason: removing a lifecycle state is cheaper than testing it, and the settle waiters were lost twice. The mechanism is in [verbatim-outpost](../crates/verbatim-outpost.md) and [verbatim-outpost-logic](../crates/verbatim-outpost-logic.md).
- The console poll: each read is a program of its own, timed by the reducer, which removes the poll's slot, the yield, the re-dispatch and the worker's carry, and keeps what Core receives and today's latency; its added wake-ups, on console keys that move nothing, are measured in the build (Dickson, 2026-10-31). Rejected: the poll as one ordinary program, which would hold whatever arrives up to its 50 ms bound and change what Core receives first.
- A test's settle is answered as soon as it is reached, without the idle round's waits for an open caret watch, an open gap record or the hook's held copies, and divides batches as the round does (Dickson, 2026-10-31).
- Posting a program's last commit with its end was dropped: the hop has not been shown to cost, and posting the end first would put a blocked release under the next program's deadline.
- The caret watch is one state the reducer holds and decides on the shared clock, the program reporting only what its check found; built first because the watch had had six rounds of fixes, past `CLAUDE.md`'s rule of three (Dickson, 2026-10-31). Reason: four of its nine recorded defects were a copy of its time or state gone stale, among six holders.
- Step two, the watch lent to a program at dispatch, is not built: `Take` stays, and the question is asked again after the caret and selection rework, which changes when the watch opens (Dickson, 2026-11-01).
- A held caret key has one deadline, Core's: 100 ms for its wait for the key ahead, 150 ms for its read. When it passes during the read, Core ends the read's entry and withdraws the read from the outpost, so the harness's idle check does not wait up to about 12 s for it (Dickson, 2026-11-01). The mechanism is in [verbatim-core](../crates/verbatim-core.md), "Held caret keys".

### Determinism

The crates on a reducer's path, which the tier check derives from the graph with `verbatim-input` listed beside the reducers, allow no hash map or set, no clock, no randomness, no thread, no environment, file, process or network call, no minted trace id, no call during a transition and no floating-point function from the platform's library. Reason: a replay or a generated case must depend only on its inputs, and every project that built a simulation reports such leaks. M4.6 runs each generated case twice and compares its effects.

- `BTreeMap` and `BTreeSet` everywhere on the path, and a fixed-hasher map only where a measurement in the build shows a cost that matters, named here with its figure (Dickson, 2026-10-31). None is taken. Reason: one kind of map leaves no key classification for review, so Clippy can enforce the rule whole.
- The terminal alignment's five lookups by row hash are `BTreeMap`s and `BTreeSet`s, with no fixed-hasher exception and no sorted vectors, accepting 12 to 62% more time on those lookups' cases than the fixed-hasher maps, because the simpler code is preferred (Dickson, 2026-11-01). Sorted vectors would have cost 1 to 9% more.
- Times and trace ids are inputs: microseconds on the run's shared clock, never an `Instant`, and a trace id carried by the input that caused the work, `TraceId::from_raw` for tests. Reason: an `Instant` cannot be recorded or generated, and the mint is a process-wide counter. The request table's deadlines turn into the reducer thread's wait through `instant_at`, erring a microsecond late (the builder, 2026-11-01, accepted by Dickson the same day).
- The outpost's three local reads leave the reducer, so that it is pure with calls as its effects (Dickson, 2026-10-31). Whether the user moved on and a walk's publication at its bound are asks, an effect answered by an input, once and before the shell takes another message; whether a window is hung is tested where each entry is handled.
- The UIA hung-window test runs where each entry is handled, on the entry's own cached window (0 passing), first in the worker's UIA event and focus handling, covering the listener's focus reads too. It drops what the batch plan's test dropped, and also a focus read whose window became hung while it waited, which could be spoken from a half-read focus at its walk's bound (Dickson, 2026-10-31).
- `verbatim-speech` stays outside the rule (Dickson, 2026-10-31): it runs after the reducer and owns the synthesizer threads; its maps are only looked up.
- The other pure owners follow the rule: Core's supervisor's liveness has one writer and `due_starts` a fixed order, the request table is generic over its asker and lives in `verbatim-core`, and the keyboard decision machine owns its gesture map and reads no clock or mint ([verbatim-core](../crates/verbatim-core.md), [verbatim-input](../crates/verbatim-input.md)). The outbound backlog's and the writer queue's `encode` keep their stamp, flush signal and recycling, which no decision reads, since moving them needs a change to the transport's contract.
- Making the owners pure should make them simpler where it can, with types that rule out invalid states (Dickson, 2026-10-31): the gap timer derived from its record, one copy of the marks, and a window that is never 0. The marks are compared after each transition, one small allocation kept as the simpler code, with no change measured (Dickson, 2026-11-01). Every window in the three logic crates where 0 means none is an `Option<Window>`, parsed where it enters them; `Item`, the protocol, and the gap record's and verdicts' keys stay `isize` (Fable, 2026-11-01, accepted by Dickson the same day).
- The enforcement: each covered crate's `clippy.toml`, identical to `verbatim-core`'s, disallows the types and methods; the ownership scan (`xtask/src/ownership.rs`) keeps those files equal, forbids silencing them outside its listed exceptions, forbids the whole modules Clippy names function by function, and forbids `tracing`'s `enabled!` family and `proptest`'s macro and default configuration; and the tier check holds each covered crate's direct third-party dependencies to an allow-list.

#### Third-party crates

The audit covers the third-party crates the covered crates call, from their source, and a crate added to the allow-list later gets the same check (Dickson, 2026-10-31): the functions called from the path, and the dependencies they reach, are read for a randomly seeded hash map whose order reaches a result, a clock, randomness, a thread, the environment or a file, and global mutable state that changes a result. Verdicts, at the versions in the lock file on 2026-11-01:

- `jieba-rs` 0.11.0: deterministic. Its dictionary is embedded, its maps are `rustc-hash`'s fixed-seed maps and its trie's hash is fixed, and its thread-local scratch buffers are cleared for each call.
- `icu_segmenter`, `icu_normalizer`, `icu_properties` and `icu_locale_core` 2.2.0: deterministic. Their data is compiled in, and the word segmenter's LSTM computes with `core_maths` over the pure Rust `libm`, so x64 and ARM64 agree.
- `unicode-width` 0.2.2: deterministic, static tables.
- `serde` 1.0.228: deterministic; its hash map impls are reached only by hash maps, which the covered crates have none of.
- `serde_json` 1.0.150: deterministic while its `preserve_order` feature is off, which nothing enables.
- `vte` 0.15.0: deterministic while its `ansi` feature, the one with a clock read, is off, which nothing enables.
- `tracing` 0.1.44: its dispatcher is global state that decides only whether an event is written, so the scan forbids asking it whether one is enabled.
- `proptest` 1.12.0: deterministic as called, from `random_config()`, which fixes the seed and turns off failure files, forks and timeouts; its default configuration reads the environment, so the scan forbids it.

### The work in flight, shared by both readers

The work in flight is a reader's record of the one slow-call item its worker is running: the worker generation in charge, the deadline, the moved-on rule, the walk's publication at its bound and the abandonment. It was written twice, in the outpost and the listener, with different judging points, and is now one pure type both embed, `InFlight` in [verbatim-reader-rules](../crates/verbatim-reader-rules.md).

- The type is `InFlight`, and the term is "the work in flight" (Dickson, 2026-11-01).
- The listener speaks what it has read of a stuck focus before giving it up when the user moves on, as the outpost does: publish first, in both readers (Dickson, 2026-11-01).
- Both readers judge whether the user has moved on at the grace's end and every 500 ms after, as NVDA does, instead of only at foreground events (Dickson, 2026-11-01). NVDA's watchdog judges at 0.5 s and every 0.5 s after with no event as evidence; Verbatim's rule itself, its deadlines and its abandonment of the worker remain its own, by earlier decisions.
- Where the build differs from its design, accepted by Dickson (2026-11-01): every listener thread start takes a fresh generation, a reused thread's included; the listener's `Publish` and `Published` carry `then_abandon` as the outpost's do; and the outpost's drop program at its shutdown is numbered by a start and an end at once.

### Core's query table

Each query Core's reducer asks, of an outpost or of a terminal's memory, has one owner: one open entry in Core's state with what its answer continues, in place of a request table entry and a holder under a second numbering ([verbatim-core](../crates/verbatim-core.md), "Core's queries"). Reason: an outpost's end was handled twice, a superseded query was invisible to Core while the idle barrier waited for it, and a withdrawal needed a lookup across the numberings. The shell's own five askers keep the request table.

- One answer input, `Input::Answered`, carrying the protocol's outcome, replaces `FetchCompleted`, `TextCompleted` and `ActivationCompleted`, and Core turns the outcome into its result by the entry's kind (Dickson, 2026-11-01).
- The protocol's request id is tagged with its asker, `RequestId { asker: Requester, number }`, open to later askers such as M5's extensions, carried opaquely by outposts and routed by Core by its tag, not a number range (Dickson, 2026-11-01).
- A terminal model's asks are folded into the table, with the model's generation as their identity check, so that no fact is held twice (Fable, 2026-11-01, under the first design principle).
- The shell item list dialog's whole wait, for its list or a clicked item's position, is bounded at 3 seconds from its request, with the outpost given the shorter time so that the walk answers with what it read, rather than Core cutting it off (Dickson, 2026-11-01): the walk makes its last call by 2 seconds, the outpost abandons the query at 2.5 and Core answers with nothing at 3 ([verbatim-app](../crates/verbatim-app.md)). Before, the dialog could wait about 12 seconds under two deadlines in a row.
- Where the build differs from its design, accepted by Dickson (2026-11-01): `Input::OutpostEnded` carries a trace the shell mints; `Queries::ask` returns the query's number with its effect; a console review command whose model was dropped no longer keeps its node held; and the shell items' expiry is withdrawn by the request table naming each expired request with its outpost.

### The hook's hold decision

The keyboard decision machine decides each key transition whole, pass, swallow, hold or defer, from a permit the hook gathers from Core's view; the hook carries out the verdict and reports ([verbatim-input](../crates/verbatim-input.md), [verbatim-input-windows](../crates/verbatim-input-windows.md)). Reason: a key's fate was decided in two places, the hook able to contradict the machine, and two sets recorded "swallow this key's release". The machine keeps one fact per key down, whether a press has reached the application, beside whether a press completed a bound gesture.

- The stuck key is fixed, departing from NVDA: a key some of whose presses reached the application has its release delivered, so the application and Windows see it come up, where NVDA swallows the release and Windows reports the key down until it is next pressed and released (Dickson, 2026-11-01).
- A `Hold` verdict clears `reached` (Fable, 2026-11-01, accepted by Dickson the same day): the shell sends the held press with its own release, so a press passed before the hold is already matched, and delivering the user's release too would reach the application with no press before it.
- Core's two views become one: `KeyHolding`'s switch becomes a field of `HookForeground`, so Core publishes one view and the hook makes one load per key, at the cost of a store, with its allocation, on the reducer thread when the focus moves between text and no text (Dickson, 2026-11-01).
- The window in front is read by the wider gate, for every transition while the focus has text in a window that may hold keys or a key waits in the line, rather than a narrower gate that would copy into the hook the machine's rule that a release is never held; if a measurement shows a rise in the hook thread's processor time, the gate is reconsidered (Dickson, 2026-11-01). None was measured.
- Num Lock is part of each key's input, since this build changed `on_key`'s input and its test calls anyway (Dickson, 2026-11-01).
- A release whose key's press still waits in the line should wait behind it whatever the permit; that rule waits for UIAccess (M8.1), since without it the permit cannot tell a window that drops the shell's sends from one that does not (Dickson, 2026-11-01).

### Failure paths and locks

- Fault points stand immediately before each hand-off whose failure path held a defect, so a test runs the production failure path; they compile only into tests, behind a Cargo feature that only `cargo xtask faults` turns on, absent from release builds and the end-to-end build, and the fault tests are built in `target/debug` beside the shipped build, rewriting only `mockapp.exe` (Dickson, 2026-10-31). The mechanism is in [verbatim-faults](../crates/verbatim-faults.md).
- An outpost whose worker cannot start, at its start or as a replacement, keeps running: the generation stays without a worker, each program is answered as failed, each dispatch tries once to start one, and the shell keeps its garbage until a worker starts (Dickson, 2026-10-31).
- A post a port refuses is kept and posted again ahead of the next one, by the relay's outbox and on the hook thread, with no timer. Reason: a refusal comes only from a system out of memory, and a timed retry would wake on a clock with no evidence.
- A wake is decided by reading the state under the lock that holds it, never from a flag another thread sets: the relay's mailbox, the cancel token and the harness's readiness keep their facts under one lock, and every audio device notice wakes the audio thread, which decides from its pass. Reason: two setters of separate flags can each miss the other's. A mark that could stay set wrongly, the GUI's `switching`, clears through a `Responder` whose drop answers.
- The lock check panics, and in debug builds aborts where a panic around application calls would be caught (the worker, UIA's event callbacks and the bounded client's thread), after writing its message to standard error and the log (Dickson, 2026-10-31). It fails a call into the application counted while one of the outpost's locks is held; measured by `measure_lock_check` on 2026-11-01, it costs about 60 ns per acquisition in debug builds, about 3.9 ms over `measure_keys_notepad`'s 64,800 acquisitions, and nothing in release builds.

### The cost rule

An allocation, a copy or a local call is not a cost to design around or to record unless a measurement shows it matters, since code shaped to avoid one adds complexity for nothing; calls into other processes, wake-ups, locks, measured processor time and work that grows with the application remain costs (`CLAUDE.md`, `REVIEW.md`, `docs/performance.md`). Reason: every instruction that set the project's cost rules named allocations beside cross-process calls with no threshold, so each session avoided and recorded single allocations. Decided by Dickson, 2026-11-01:

- Simplified: the relay's large-frame hand-over, the terminal conversion's spare buffers (its text made with room for one byte a code unit), Core's small-checkpoint rule and the hook's window-class cache.
- Kept: the flight recorder's size estimate, `HeapCounter`, whose cost is constant per string where counting the entry's JSON would scan every byte of a terminal document on the reducer's step; Core's caret history, which the caret and selection rework deletes and whose change now would break every earlier dump; and the latency ledger's last key, a time and a flag packed in one `AtomicU64`, because `key_seen` runs on the hook thread, where no lock may be taken. `ForegroundView`'s `ArcSwap` is kept.
- The performance ledger records only what is substantially slow or can become so, and its entries of allocations, sizes and microseconds were removed.

### Measured before and after

Each build measured wake-ups and processor time before and after in debug builds and test audio mode, and found no change beyond its trace windows' spread, with two exceptions: a console key that moves nothing costs about a fifth more wake-ups, 17.5 against 14.6 in all processes per input-hook wake, from the console poll's reads as programs; and an item past its moved-on grace costs one wake every 500 ms until it returns, is published or is abandoned. The crate split left private memory, the listener's start (about 82 ms) and `cargo xtask ci`'s time (about 203 s with nothing changed) as they were.

## M4.6: the generated tests

### The precedent: the simulated terminal

Verbatim already has one instance of every part of this design, in the terminal work (`docs/design/terminal-model.md`, "Testing"): a simulated component, `verbatim-term-sim`, whose answers are checked against what the real console host and Windows Terminal answer to a corpus of streams (its conformance expectations, recorded by `cargo xtask regenerate-terminal-conformance-expectations`); an oracle that judges every read rather than driving a rule; generated streams under a fixed seed (`crates/verbatim-term-sim/src/oracle.rs`) and every stream of up to four operations on a tiny terminal. M4.6 applies the same pattern to the reducers, and the simulated terminal stays as it is: it is the model of a terminal's provider that any later simulated application reuses.

### Invariants and models first

The invariants, reference models and postconditions are designed before any test is written, from the designs and the parity docs rather than from the defects already fixed, and reviewed adversarially. Each is shown to bite: breaking the rule it guards by hand, or a mutant of it, makes its test fail. Reason: the tests are only as good as what they check, and invariants learned from fixed defects overstate what they will catch.

### Tests against reference models

Each pure owner gets a generated state-machine test with `proptest-state-machine`, the outpost's reducer first, then Core's reducer, the listener's scheduler and Core's request table in place. Each compares the owner with a reference model far simpler than it, checks the exact effects of each input against postconditions, and checks invariants after every step. Reason: in Hughes's measurement of eight injected bugs, invariants alone caught 38%, postconditions 79% and a reference model all eight.

Rules:

- A generator produces only operations valid in the model's current state, weighted toward lifecycle operations (open, close, abandon, replace, re-dispatch), and the test counts how often each operation and state is reached. Reason: an invariant never violated in a state never reached proves nothing.
- A generator obeys the guarantees the system gives and no stronger ones. A counterexample that cannot happen tightens the generator, and the guarantee it relied on is written down where the mechanism is described.
- `cargo xtask ci` runs each test with a fixed seed and a fixed number of cases, within a stated time (Dickson, 2026-10-31). A separate exploration job runs new seeds and many more cases; a failure it finds is shrunk and added to the fixed cases, never kept as a seed, since a seed means nothing after the code changes. Reason: `docs/testing.md` asks for one fixed code path, and a random seed in the gate fails on whichever commit draws it.
- A model stays far simpler than the code it checks. Reason: a model that copies the code copies its defects, and errors are often in the model.

### Not built: recorded inputs replayed against NVDA

Recorded inputs are not replayed against NVDA's captures (Dickson, 2026-11-01): NVDA is the starting point for what Verbatim says, not a target to match, and deliberate differences are recorded in the parity ledger; `cargo xtask nvda compare` serves the research use, and the end-to-end scenarios pin what Verbatim says.

### Mutation testing

`cargo-mutants` runs on the reducer crates, limited to changed code, as a periodic job and as the first of the goal's measures.

### The simulation, deferred

The two layers are those Dropbox found it needed: tests of each pure owner alone, as its CanopyCheck tested its pure planner, which M4.6 builds; and a simulation of the owners together with their concurrency, as its Trinity did, which is deferred.

A simulation joining the reducers through pretend pipes is built only once M4.6 meets its thresholds and two prerequisites exist: the transport's merge, priority and overflow rules as pure code it can call, since the real pipes merge by key, put urgent messages first and drop the oldest past 64 (`docs/design/communication.md`, section 4.4), and a recorder of the worker's commits. Built then, it is hand-written, since the simulation frameworks assume async code, and it follows the lessons of the projects that built one: a fault model for application answers (slow, failing, reordered, the application exiting), counters for the states reached, a separate random stream per component, and the end-to-end suite kept, since it catches what no simulation models.

### Toward a simulated application

FoundationDB could simulate everything because the contract of what it simulated, disks and networks, was known. Verbatim's is applications seen through UI Automation and MSAA, whose behaviour is known only by observation. The terminal shows how far that can go: a simulated provider whose conformance with the real one is checked against a corpus. The same for applications would take, in order:

- The worker's reads behind one trait, as the listener's reading threads already are (`Reading` in the listener's focus reading), so a program runs unchanged against a simulated application.
- A simulated application: a tree of elements with UI Automation's and MSAA's answers, and its events, in memory.
- Its fault model taken from `docs/uia-bugs.md`, the faults Verbatim has established, which plays the part of the operating system contract FoundationDB knew: a hung window, a call past its timeout, an element gone between two calls, a focus event late or repeated, a reused runtime id.
- Its conformance checked as the terminal's is. mockapp is the reference for UI Automation's and MSAA's own behaviour: a real provider in a real process, read through the real client stack, so the same fixture run in mockapp and in the simulated application gives the answers the model must match. Recorded end-to-end sessions are the reference for real applications' behaviour. Every difference is a defect in the model or a new entry in `docs/uia-bugs.md`.

The simulated application does not replace mockapp: mockapp tests the real client stack, COM and the cross-process calls, at a process per test and real timing; the simulated application runs in memory, in microseconds, under the simulation's seeded scheduler, where any fault can be placed at any step, and so tests the logic above the calls, not the calls.

These steps follow the simulation of the reducers, once it has shown value by the same kind of judgement as M4.6's (`docs/roadmap.md`, M4.6). Code outside every simulation should then be only the calls themselves: COM and UI Automation, the hooks, the pipes' I/O, the audio device and the engines.

## Performance

Nothing here runs on the event path in a release build. Fault points compile out of release builds; the lock check is debug-only, its cost stated. The crate split changed no thread, message or wake-up, and the relay mailbox's wake decided under its lock measured no change. M4.5's figures are under "Measured before and after".

## Alternatives weighed

- An async runtime (Tokio): rejected. Verbatim already waits on many pipes at once through completion ports, which Tokio itself uses on Windows; its blocking work is COM and UI Automation calls that an async runtime cannot await; and `mio`'s pipes, under Tokio's, were rejected by measurement (`docs/design/communication.md`, section 12).
- Simulation frameworks (madsim, turmoil) and Antithesis: rejected. The frameworks are built for async code, and Antithesis runs Linux containers.
- A second model in another language (P, TLA+) or proof tools (Verus, Creusot, Prusti): rejected. Each is a second description to keep in step with the code. Kani, for bounded proofs of single transitions and parsers, is an optional later pilot on Linux.
- `loom` or Shuttle: rejected, even for the mixer's wake rule (Dickson, 2026-10-31). They see only code written with their stand-in types, the remaining hand-offs are completion ports and Windows events, and one defect since 1 October was a lost wake between threads (631ee56d), fixed by waking on every push.
- `stateright`: kept for a protocol whose design is open, since it checks a separate model.
- A pure ledger split out of the object store, and the request table split into a pure table and the shell's delivery: rejected after review, since each copies one fact into two places, the node ids the backends' registries mint and the asker each request is owed to.
- Keeping `verbatim-outpost` whole, or breaking only its cycles: cheaper, but its name covers three programs and its logic could not be tested on another platform.
- `declarative-enum-dispatch` and `thingbuf`: not needed; the trait objects are called per utterance or event, and the mixer's lanes use `rtrb`.

## Limitations

- What Windows, UI Automation and applications do comes only from recordings; mockapp and the end-to-end suite remain the tests of it.
- A rule that is wrong but consistent, such as words that differ from NVDA's, breaks no invariant; postconditions and the end-to-end scenarios are the tests of it.
- Logic left in the shells is outside every generated test: the worker's reading, the listener's per-read checks until the rework makes them pure, the transport's merges, the `Arbitrator`'s keeping of verdicts, the speech queue until M6.3 and the supervisor's owner until M6.2.
- Generators and models must follow every change to a reducer's inputs; this is the recurring cost the goal's second measure weighs.
- No screen reader publishes testing beyond example tests and end-to-end speech capture, so there is no precedent in the field.

Of M4.5's builds:

- The crates by role: the listener's own image no longer shares code pages with the outposts; `verbatim-win32`'s `provider::parent_window` and the general `window::parent` remain separate functions that behave alike (a follow-up); `verbatim-uia` and `verbatim-ia2` reach `verbatim-core` through `verbatim-reader-rules` for one type, `ForegroundRelation`, so they do not compile alongside it; and every logic crate reaches `verbatim-text`, whose `jieba-rs` builds `zstd-sys` from C, so their Linux build is checked only on GitHub's runner, not on this machine.
- The outpost's programs: a console poll's read, naming no window for the moved-on rule, can last until the text deadline; in tests a settle does not wait for entries that arrive after it; a foreground change does not update the entered window of older reports in its window (a roadmap item); and the foreground window is read on every non-foreground focus report whose top-level window is known.
- The caret watch: the rule that the reducer changes the watch only at a dispatch, at a publication that claims the program, or with no program in flight is checked by the generated test, not the types; after a poll read's result a program's copy may show the watch polling where the reducer ended it, which no poll read reads; and no scenario reaches the 10-second bound, a release of the watch's node or a held key passed on at its read's deadline, which unit and generated tests alone hold.
- Determinism: third-party code is checked when it is added and when a version change is reviewed, with M4.6's run of each case twice as the backstop; the verdicts on `serde_json` and `vte` rest on features nothing enables, which `workspace-hack` could unify in, though neither would make a result depend on anything but its input; a `tracing` field with a side effect would make the logic depend on the subscriber, a review rule; the hung-window test is a live call no unit test holds and no scenario makes a window hung; the idle barrier and the GUI's lifecycle stay outside the covered crates; and a lost read over a console buffer enlarged to 32,766 rows takes 4.9 ms in a release build, past the reducer's 2 ms budget, as before the build (Dickson, 2026-10-31).
- A terminal row is compared by its 64-bit hash alone, a fixed public function of the text, so two texts with the same hash align as the same row, and an application could make that happen on purpose (a follow-up).
- Failure paths: a message the app's port refuses, or a mailbox wake the relay's port refuses, waits up to a ping interval, 3 s; a key's post a port refuses waits for the next key, with no time limit, and a harness's `AwaitIdle` then fails at its timeout naming the input; both happen only when the system is out of memory. A fault point stands for the operation's failure, not for the system's state behind it. The lock check covers the outpost's own locks and the calls, agile references, bounded waits and store releases the backends count; an object released under another crate's lock is not checked. The synthesizer switch's responder cannot answer while the synth thread holds the request in a driver call that never returns.
- The hook's hold decision: the permit is gathered before the machine decides, so the window in front is read for keys that end neither held nor deferred; and when the permit falls to `None` while a deferred press waits in the line, its release passes at once and can reach the application before the shell's send of the press, leaving the key down in Windows, until UIAccess (M8.1).
- Core's query table: each kind's supersession and clear is an operation at each site, held by the generated test's invariants, so the types allow a missed clear; only a terminal model's entries carry an identity check, their generation; and a superseded query still runs in its outpost until it is answered, only the held read and an expired shell items query being withdrawn.
- The shell items' bound: a walk after a slow outpost start, or behind other work, has less than 2 seconds and lists fewer items on a large taskbar; a call still under way half a second after the walk's last call loses the whole walk to `Abandoned`; and the half-second margins are judgements.
- The work in flight: a move that raises a foreground event is noticed up to 500 ms later, as in NVDA; a stuck item past its grace costs up to two wakes a second until its deadline; and the listener keeps the abandoned limit and the per-application queue, and the outpost the focus group and the owed work.
- The cost rule: the recorder's hand-written serializer remains, about 270 lines; a whole-document frame is copied once more in the relay, about 0.4 ms (release) on verbatim.exe's owner thread with its peak memory raised by the frame's size, 4.0 MB, while both are held; and the terminal conversion without its spare buffers costs about 0.2 to 0.3 ms more in the outpost, off the reducer's thread, on the rare reads of a whole document.
