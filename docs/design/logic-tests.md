# Logic tests: finding ordering defects before review does

Status: proposed on 2026-10-31, revised after its adversarial review (`experiments/logic-tests-study/design-review-1.md`); Dickson's decisions of that day are in `experiments/logic-tests-study/decisions-2026-10-31.md`, and the research behind the alternatives weighed, other projects' experience and the Rust tools, in `experiments/logic-tests-study/research/report.md`. Built in two milestones (`docs/roadmap.md`): M4.5, the foundations, after the keep-id fix and before the focus pipeline rework's remaining builds; and M4.6, the generated tests, after the rework's second build, so the tests are written once against the code that stays.

## The problem

From 1 October to 31 October, 586 commits on main had messages naming a fix (`experiments/logic-tests-study/readme.md` gives the method and its caveats). A first classification found 67 defects of ordering, lifecycle or hand-off among 187 behaviour defects. The review's recheck of 35 of those rows found about 10 misclassified and about 7 later rounds of one mechanism, which puts the distinct ordering defects at roughly 35 to 45, those within reach of generated tests of pure code at about 20 to 30, and those in code that survives the rework at about 8 to 12. The invariants proposed were learned from those same defects, so past catches overstate future ones; the counts are upper bounds within about a factor of two.

The defects were found by review rounds, live runs and end-to-end failures on GitHub's runner, often days after their commit, and the ownership refactor's history shows the cost: a mechanism patched three to seven times before its design changed. No test explores orderings: the replay tests check the orderings someone thought of.

## The goal, and how it is judged

The goal is fewer ordering defects reaching review, a run or a user, and an architecture simpler to reason about, at a cost the defects found repay. Two measures, with their thresholds set in M4.6's design and accepted by Dickson before any generated test is written:

- The mutants the generated tests kill that the existing replay tests miss, from `cargo-mutants` run on the reducer crates. Reason: it measures whether the tests detect defects at all, available the day they are written.
- The ordering defects that review, a live run or the end-to-end suite find in the builds after M4.6, each checked against the tests, beside the time each build spent on generators, models and their upkeep.

If the thresholds are not met, the generated tests stop growing and the simulation is not built. The crate split is not judged by them: it is a one-time cost Dickson chose for clean ownership by role (2026-10-31).

## M4.5: the foundations

### Crates by role

Today's `verbatim-outpost` holds three programs, the outpost process, the focus listener and Core's supervisor, with a few modules depending on each other in both directions. It becomes:

- `verbatim-outpost`, the outpost process, its own executable `verbatim-outpost.exe`, with its logic in `verbatim-outpost-logic`: the reducer.
- `verbatim-listener`, the focus listener, its own executable `verbatim-listener.exe`, with its logic in `verbatim-listener-logic`: the read scheduler and the menu end's grace, and later the per-read checks as pure functions.
- `verbatim-reader-rules`, the rules both readers apply before reading: NVDA's class verdict and its tables, the acceptance test's decision (its window queries come through a trait), the console window's class and the moved-on grace. Both readers depend on it and neither on the other. The outpost still owns each window's verdict (`docs/design/ownership.md`, "The scope rule"); the crate holds the function both evaluate, so they cannot disagree.
- `verbatim-supervisor`, Core's side of starting, watching and retiring children, with its pure decisions (the policy, and the generation's typestates with the `Sealed` token that only they can make) in `verbatim-core`'s `supervisor` module.
- `verbatim-win32`, the window queries both readers and the supervisor use, with `verbatim-ia2`'s window wrappers, and the WinEvent thread unless it must sit beside the hooks.
- `verbatim-process` gains the message layer over the transport, the launch arguments and the alarm.

Rules:

- Every dependency points one way: a Windows crate depends on its logic crate and on shared crates, never on another role's Windows crate. Reason: names that match roles, and a crate boundary enforces purity where the text scan in `xtask/src/ownership.rs` cannot see through a re-export.
- The logic crates (`verbatim-outpost-logic`, `verbatim-listener-logic`, `verbatim-reader-rules`) are in the GPL tier, since they hold ported NVDA behaviour, and depend on no Windows crate. A platform-neutral crate depends on no GPL-tier crate. Reason: today's check looks only for the Windows bindings, so it would let `verbatim-core` or `verbatim-ext-api` depend on a logic crate. `cargo xtask ci` checks both, and `CLAUDE.md` gains the third kind of crate with the build.
- Code moved into `verbatim-core` cites the parity docs, not NVDA's source, as the provenance rule asks; the move's review checks every comment that describes NVDA's behaviour.
- The listener and the outpost are separate executables (Dickson, 2026-10-31), done in the same build so one full end-to-end run covers both. The saving in memory is expected to be small, since code pages are shared, and is measured.

### The outpost's program bookkeeping simplified

Moved here from M6.1 (Dickson, 2026-10-31), before the tests, so no test is written for a state it removes: test settles merged into the idle query; verdicts posted as a message of their own; each program given fresh copies of the reducer's facts at dispatch, so the worker keeps none (which removes the worker's held copy behind `judged_focus`); the console poll's slot and the re-dispatch behind it, if a simpler rule replaces them; a program's commit and its end posted as one message, if a measurement shows the hop costs. Guarded by the existing replay tests. Reason: removing a lifecycle state is cheaper than testing it, and the settle waiters were lost twice.

### Failure paths and locks

- Fault injection at the six hand-off operations whose failure paths held defects: a port refusing a post (de7f481e), the relay's mailbox (d6f627a5), a thread failing to start (76180512), a UI Automation handler removal failing (1bfb6f87, 61a4bfa1), and a full channel (0d1e7802). A test forces each failure and checks that the request is still answered, the work still taken and the shutdown finished.
- A flag shared between threads that decides a wake, such as the relay mailbox's `posted`, is removed, and the wake decided by reading the state when it is made, as the design principles ask. A mark used within one thread becomes a guard that clears it when dropped.
- A debug-build check fails when a call into an application is counted while one of the outpost's locks is held, using the backend crates' call counting (d0ec3a89, fc899340). Its cost per lock is stated in the performance ledger, since the end-to-end suite measures debug builds.

### Tools and determinism

- `xtask` builds on any platform: its Windows-only commands and their dependencies apply only on Windows, so `cargo xtask docs` and the text scans run on Linux.
- A Linux job in CI builds and tests the logic crates and `verbatim-core`, and runs `cargo xtask docs` and the scans. Reason: a dependency check reads the graph, while a build on another platform proves it.
- Determinism is enforced in the pure crates (Dickson, 2026-10-31): no standard hash map or set, no clock, no randomness, no thread and no environment read, checked by Clippy's disallowed types and methods in each pure crate and by the ownership scan, after an audit of the 29 standard hash maps and sets there; M4.6 runs each generated case twice and compares its effects. Reason: a hash map's order changes with each process, so a replay could differ, and every project that built a simulation reports such leaks.

### Measured before and after

Wake-ups and processor time, as every build measures them; each child's memory; and `cargo xtask ci`'s time.

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

### Recorded inputs replayed against NVDA

A recorder of reducer inputs runs during end-to-end and NVDA capture sessions, and replay tests compare what is spoken, exactly, with the captures. Verbatim can tie each utterance to the input that caused it, which no screen reader driven from outside can. First step: whether the existing captures hold enough of the inputs to replay, or new captures are needed.

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

Nothing here runs on the event path in a release build. Fault points compile out of release builds; the lock check is debug-only, its cost stated. The crate split changes no thread, message or wake-up. Removing the relay mailbox's flag changes when a wake is posted, measured in the build.

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
- A rule that is wrong but consistent, such as words that differ from NVDA's, breaks no invariant; postconditions and replay against captures are the tests of it.
- Logic left in the shells is outside every generated test: the worker's reading, the listener's per-read checks until the rework makes them pure, the transport's merges, the speech queue until M6.3 and the supervisor's owner until M6.2.
- Generators and models must follow every change to a reducer's inputs; this is the recurring cost the goal's second measure weighs.
- No screen reader publishes testing beyond example tests and end-to-end speech capture, so there is no precedent in the field.
