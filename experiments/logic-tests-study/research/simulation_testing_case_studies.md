# Deterministic simulation testing in practice: case studies, costs, and limits

Scope note: deterministic simulation testing (DST) here means running the real production logic, single-threaded, against simulated network, disk, clock and randomness, with every choice drawn from one seeded random number generator, so that any failing seed replays exactly. Research done 2026-10-31; dates are the publication dates of each source. Claims from secondary sources are marked "(secondary)".

## FoundationDB: what simulation caught, what it cost, what it could not catch

### Takeaway
FoundationDB's own SIGMOD 2021 paper credits simulation with its reliability and with making ground-up rewrites feasible, but says the codebase was built from the start for it (all code deterministic, no multithreading, everything behind a Flow shim), and lists hard limits: no performance bugs, no third-party or non-Flow code, and bugs from the real operating system contract being weaker than believed.

### Cited Findings
- Architecture cost: "FDB was built from the ground up to make this testing approach possible. All database code is deterministic; accordingly multithreaded concurrency is avoided (instead, one database node is deployed per core)." Network, disk, time and the random number generator are abstracted; "The production implementation is a simple shim to the relevant system calls." — [FoundationDB SIGMOD 2021 paper, section 4](https://www.foundationdb.org/files/fdb-paper.pdf)
- Workloads (written in Flow) carry fault-injection instructions, mock applications and configuration changes, and are composable into test cases; oracles are assertions in workloads (invariants only maintainable via atomicity and isolation) plus local assertions throughout the code; recoverability is checked by restoring the modelled hardware and waiting for the cluster to recover. — [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf)
- Fault injection: machine, rack and datacenter failures and reboots, network faults, partitions, latency, corruption of unsynced writes on reboot, randomized event times. "Fault injection distributions are carefully tuned to avoid driving the system into a small state-space caused by an excessive fault rate." — [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf)
- BUGGIFY ("buggification"): at many places in the code the simulation may inject "unusual (but not contract-breaking) behavior such as unnecessarily returning an error from an operation that usually succeeds, injecting a delay in an operation that is usually fast, choosing an unusual value for a tuning parameter". Randomizing tuning parameters "ensures that specific performance tuning values do not accidentally become necessary for correctness." — [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf)
- Swarm testing: each run picks random cluster size, configuration, workloads, fault parameters, tuning parameters, and a random subset of enabled buggify points. — [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf)
- Generator-coverage feedback: conditional coverage macros such as `TEST( buffer.is_full() );` report how many runs reached a condition; "If the number is too low, or zero, they can add buggification, workload, or fault injection functionality". This is FDB's answer to generators missing important states. — [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf)
- Speed: discrete-event simulation fast-forwards the clock across idle stretches, so slow-to-play-out bugs are found per core-second faster than in real end-to-end tests; testing is "embarrassingly parallel" and developers "burst" it before releases. — [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf)
- Stated limitations, verbatim: "Simulation is not able to reliably detect performance issues, such as an imperfect load balancing algorithm. It is also unable to test third-party libraries or dependencies, or even first-party code not implemented in Flow. As a consequence, we have largely avoided taking dependencies on external systems. Finally, bugs in critical dependent systems, such as a filesystem or the operating system, or misunderstandings of their contract, can lead to bugs in FDB. For example, several bugs have resulted from the true operating system contract being weaker than it was believed to be." — [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf)
- Production bugs: when a bug was first found in the wild, "the debugging process was almost always first to improve the capabilities or the fidelity of the simulation until the issue could be reproduced there". Adding logging generally does not perturb event order, so reproduction is exact. — [FDB paper, section 6.2](https://www.foundationdb.org/files/fdb-paper.pdf)
- Outcomes claimed: CloudKit ran FDB "for more than 0.5M disk years without a single data corruption event"; ground-up rewrites of major subsystems would otherwise "have been deemed too risky"; ZooKeeper was removed after real-world fault injection found two independent ZooKeeper bugs (circa 2010) and replaced with Paxos in Flow. The paper says productivity gains are "hard to measure" and gives no bugs-per-effort figure. — [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf)
- Simulation is single-threaded, so multithreaded client behavior is covered by a separate API tester on real clusters (secondary summary of FDB docs). — [search summary citing FDB wiki](https://GitHub.com/apple/foundationdb/wiki/Debugging-in-FDB)
- Will Wilson's 2014 Strange Loop talk abstract promises "benefits, drawbacks, and tradeoffs"; I could not retrieve the talk's own list of drawbacks. — [Strange Loop 2014](https://thestrangeloop.com/2014/testing-distributed-systems-w-slash-deterministic-simulation.html)
- Wilson (SE Radio 685, September 2025, via a search summary, secondary): without a hypervisor you must make all dependencies deterministic yourself, otherwise a simulation can end up not really testing anything. — [SE Radio 685](https://se-radio.net/2025/09/se-radio-685-will-wilson-on-deterministic-simulation-testing/)

### Inferences
- FDB's limits map directly onto Verbatim's Windows shells: code not under the simulator (the COM calls into other applications) is untested by it, and "the true operating system contract being weaker than believed" is the same risk as recorded UIA answers that are more orderly than real ones.
- FDB's habit of first teaching the simulator to reproduce a production bug is a cheap discipline to copy: each field bug either becomes a simulation seed or marks a known fidelity gap.

### Gaps
- No primary figure for how much of FDB's code exists for simulation (workloads, sim network, buggify sites); the paper does not quantify it.
- Wilson's talk content on drawbacks not retrieved.

## TigerBeetle VOPR, Antithesis and its customers

### Takeaway
TigerBeetle's VOPR finds bugs whenever its fault model widens, and its own newsletters record a real bug it could not have caught because the fault model lacked that fault (memory via swap), plus false positives from the team's own misunderstanding encoded in the simulator. Antithesis removes the need to restructure code but users report vendor opacity, cost, and clean runs that prove little.

### Cited Findings
- TigerBeetle, December 2024: it was "possible to circumvent our storage fault model on systems where swap is enabled" (the kernel could page corrupted disk data into memory). "This behaviour could not have been detected by the VOPR as it tests whether TigerBeetle correctly adheres to its storage, process and network fault models, but does not inject memory faults!" Fix: `mlockall` on startup (Windows: `SetProcessWorkingSetSize`); macOS remains exposed. — [December in TigerLand](https://tigerbeetle.com/newsletters/2024-12-23-december-in-tigerland)
- TigerBeetle, February 2025 (search summary of the newsletter): adding misdirected-write faults found two correctness bugs and a few overtight assertions; switching corruption injection to byte level found two more bugs (sector padding); some failures were false positives where "the consensus protocol was behaving correctly, but the team's understanding led to false positives in the simulator." — [February in TigerLand](https://tigerbeetle.com/newsletters/2025-03-05-february-in-tigerland)
- VOPR limits the faults injected per prepare so the simulated cluster can repair and stay live; a GitHub issue proposed an unbounded-fault mode checking safety only (search summary). — [tigerbeetle issue 696](https://github.com/tigerbeetle/tigerbeetle/issues/696)
- VOPR reproduction requires both the seed and the git commit (search summary). — [VOPR docs](https://docs.tigerbeetle.com/about/vopr)
- Antithesis on etcd (Marek Siarkowicz, Google, 3 October 2025): 830 wall-clock hours, "simulated 4.5 years of usage"; found "A critical watch bug ... that our existing tests had missed", present in all stable releases; two panics on main; reproduced all previously known bugs as validation; found a bug in etcd's own robustness test and "a flaw in our linearization checker model". etcd's earlier randomized tests "resembled throwing darts while blindfolded". Integration effort, cost and blind spots were not stated. — [etcd blog](https://etcd.io/blog/2025/autonomus_testing_with_antithesis/)
- Aiven Inkless (Kafka fork), 5 March 2026: about 2,200 logical hours under Antithesis found no Inkless issues; "a passing test can be the most suspicious type of test"; running upstream Kafka's tests found KAFKA-19880 (idempotent first batch out of order). Their earlier home-grown property-based test required "a fair amount of code" to write and maintain. Exploration heuristics are proprietary. Code-level simulation "makes retrofitting such tests into an existing code base difficult." — [Aiven blog](https://aiven.io/blog/deterministic-simulation-testing-in-diskless-apache-kafka)
- WarpStream ran its whole SaaS stack (agents, control plane, Kafka clients, KV store, Postgres, local object store) under Antithesis; its existing CI tests already injected faults, but the hypervisor (faults, latency, thread hangs, restarts) reached rare behavior far better (search summary; no bug counts found). — [WarpStream blog](https://warpstream.com/blog/deterministic-simulation-testing-for-our-entire-saas)
- MongoDB has used Antithesis since 2021 for storage engine, server, cluster sync and upgrade/downgrade (search summary of an Antithesis-hosted guest post). — [Antithesis blog](https://antithesis.com/blog/mongo_bug/)
- Polar Signals (May 2024) rejected Antithesis because "the pricing just didn't work for us". — [Polar Signals, Mostly DST in Go](https://www.polarsignals.com/blog/posts/2024/05/28/mostly-dst-in-go)
- Antithesis's hypervisor runs on Linux-based containers (Docker images) on a bhyve-derived hypervisor (search summary). — [Antithesis deterministic hypervisor](https://antithesis.com/blog/deterministic_hypervisor/)

### Inferences
- "The simulator only tests the fault model you wrote" is the central TigerBeetle lesson; for Verbatim, recorded application answers only contain the behaviors that were recorded, so slow, reentrant, or out-of-order COM answers must be added deliberately as faults.
- False positives from encoding a wrong understanding into the simulator are the mirror of mock drift: the simulator's oracle can be wrong too.
- Antithesis targets Linux containers; it is unlikely to host a Windows screen reader driving real UIA applications (inference, not verified).

### Gaps
- No primary Antithesis statement of what it cannot catch; no customer cost figures; no Ramp or Palantir write-ups were retrieved.

## Rust projects: RisingWave (madsim), S2 (turmoil), Polar Signals, others

### Takeaway
Rust teams got real bugs (deadlocks, data loss and duplication, concurrency panics) but repeatedly fought determinism leaks in dependencies (HashMap seeding, library reads of time and entropy), had to patch or fork tokio-ecosystem crates or override libc symbols, watched simulation time grow past CI budgets, and found that logic which escapes the simulated core goes untested. The design that avoided runtime patching, Polar Signals' explicit state machines on a message bus, is the closest to Verbatim's reducers.

### Cited Findings
- RisingWave (25 April 2023): unit, end-to-end, recovery and scaling simulation tests on every PR; whole system single-threaded with etcd and S3 replaced by simulators; an end-to-end round takes about 2 minutes simulated versus 8 on CI with real services; 16 seeds in parallel on 16 cores with a 20-minute PR budget. — [RisingWave part 2](https://www.risingwave.com/blog/applying-deterministic-simulation-the-risingwave-story-part-2-of-2)
- RisingWave results: end-to-end simulation found "several bugs caused by concurrency, including panics, deadlocks, and calculation errors"; recovery testing caused "a rather painful debugging period of two months, as new bugs were always discovered" (missing error handling, assertions on wrong assumptions); scaling testing found state recovery, cache invalidation and data loss or duplication bugs. Unit-level simulation tests "mainly exposed bugs in the test logic itself" and mostly validated madsim. — [RisingWave part 2](https://www.risingwave.com/blog/applying-deterministic-simulation-the-risingwave-story-part-2-of-2)
- RisingWave problems: test time grew past budget, fault injection was costly, so they added a fault probability "to trade off testing intensity for execution speed"; more tests did not mean more bugs; madsim covers only Rust, and simulators for every connector are "costly and less rewarding" (only Kafka simulated); no faults during non-idempotent data modification. — [RisingWave part 2](https://www.risingwave.com/blog/applying-deterministic-simulation-the-risingwave-story-part-2-of-2)
- madsim mechanism: libraries not made deterministic by overriding libc calls are replaced via Cargo `[patch]`; the README has users swap tokio, tonic, etcd-client, rdkafka and aws-sdk-s3 for madsim forks, with git-pinned forks of others; a `MADSIM_TEST_CHECK_DETERMINISM` mode reruns a seed to detect nondeterminism. — [RisingWave part 1](https://risingwave.com/blog/deterministic-simulation-a-new-era-of-distributed-system-testing/); [madsim docs](https://docs.rs/crate/madsim/latest)
- S2 (Shikhar Bhushan, 2 April 2025): tokio current-thread runtime with paused clock, turmoil for simulated network between logical hosts, in-memory emulators behind minimal storage and metadata interfaces, and `mad-turmoil` overriding libc `getrandom`, `getentropy`, `clock_gettime` (and a Mac-only `CCRandomGenerateBytes`). — [S2 blog](https://s2.dev/blog/dst)
- S2's determinism leaks: first CI failures could not be reproduced on developers' Macs and "sometimes not between runs on the same platform"; diffing TRACE logs found timestamps in HTTP packets, randomized `HashMap` hashing inside dependencies they could not seed, and library code reading system time or entropy. Fix: a CI meta-test that reruns a seed and compares TRACE logs, "Down to the last bytes on the wire". 17 notable bugs logged, including plain deadlocks and external API nuances; "production usage will always bring surprises". No runtime or cost figures. — [S2 blog](https://s2.dev/blog/dst)
- HN thread on S2: an S2 engineer reports "a few distributed deadlocks" and help with caches and eventual consistency, logs read only on a failing test; a robotics practitioner reports injecting monotonic time only, that robotics code "tends to never handle time jumping properly", and that configuring time sources was tricky. — [HN 43581900](https://news.ycombinator.com/item?id=43581900)
- Polar Signals Rust (Alfonso Subiotto Marqués, 8 July 2025): chose explicit state machines connected by a message bus over madsim, so failures are injected in one place (the bus) rather than per interface; found "two critical bugs ... one causing data loss and another causing data duplication". Costs: high cognitive overhead; under deadline "the path of least resistance is taken" and "code that should be in state machines leaks up into the production driver and is not tested in DST tests"; "Dependency nondeterminism must be eliminated, otherwise DST reproducibility goes out the window"; deterministic async dependencies wrapped with `block_on`. — [Polar Signals, DST in Rust](https://www.polarsignals.com/blog/posts/2025/07/08/dst-rust)
- Polar Signals Go (28 May 2024): WASM to force one OS thread, a sub-10-line Go runtime patch for the seed, faketime; "mostly" deterministic because same-seed runs occasionally diverge "for reasons we have not yet investigated"; faketime can hang valid programs; Hermit discarded as no longer maintained; earlier FrostDB effort was "never really satisfied with the amount of failure injection" because each failure type needed its own interface implementation; found three data loss and two duplication bugs in the first weeks. — [Polar Signals, Mostly DST in Go](https://www.polarsignals.com/blog/posts/2024/05/28/mostly-dst-in-go)
- Loom-style and serialized simulation is slow; a practitioner reports re-implementing much of tokio's API on a DST runtime and messy dependency patching (secondary, search summary of HN). — [HN 44759172](https://news.ycombinator.com/item?id=44759172)

### Inferences
- Verbatim's reducers already have the Polar Signals shape (state plus input to state plus effects), which avoids the tokio-patching and libc-override burden altogether: a harness can feed messages directly, with no runtime to make deterministic.
- The Polar Signals leak, logic drifting into the driver, is Verbatim's main risk: whatever the Windows shells decide (timeouts, retries, coalescing) is invisible to a reducer-only simulation.
- S2's TRACE-log replay meta-test is cheap and catches HashMap and clock leaks early; worth adopting from day one.

### Gaps
- sled, Materialize, Fly.io Corrosion and Resonate were not researched within the call budget; no sources retrieved.
- No source found documenting an abandoned Rust DST effort or madsim maintenance cost in numbers.

## Dropbox Nucleus: CanopyCheck and Trinity

### Takeaway
Dropbox (2020) split testing into a pure planner fuzzer (CanopyCheck, no I/O, cheap, minimizes failures) and a concurrency simulator (Trinity, custom executor over a Rust Future with mocked filesystem, server and timer), and stated plainly what Trinity cannot do: real OS interleavings, fsync durability, mock drift from the server, and seeds that cannot be minimized and break on any code change.

### Cited Findings
- Requirement: "All randomized testing frameworks must be fully deterministic and easily reproducible." Nucleus runs its logic on one control thread; Rust's randomized HashMap hasher was replaced with a deterministic one; the commit hash is a test input. — [Dropbox, Testing sync at Dropbox](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine)
- CanopyCheck: generates one tree and perturbs it into the other two (independent random trees would never exercise delete, edit or move); loops asking the planner for a batch, shuffles it to check order independence, pretends each op succeeded; checks termination (200-iteration cutoff), no panics, all three trees equal, plus invariants that stop trivial passes such as deleting everything; QuickCheck-style shrinking. It "caught an enormous number of bugs" early, but "skips out on a lot of the juicy parts of Nucleus like I/O and concurrency." — [Dropbox](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine)
- Trinity: acts as the executor polling Nucleus and intercepted requests, choosing via the PRNG which to satisfy, delay or fail; mocks an in-memory filesystem (about 10x more runs than native), a Rust mock of the whole server, and a fast-forwardable timer; reruns each seed to confirm the same final state. — [Dropbox](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine)
- Scale: tens of millions of randomized runs nightly; generally "100% green on the latest master"; failing seeds get a task with the commit hash. — [Dropbox](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine)
- Stated limitations: native mode is about 10x slower; platform calls are serialized so real OS interleavings "could very well be a source of race conditions in the wild"; "Trinity cannot actually reboot the machine mid-test, so it can't validate that we use fsync in all the right places"; mocking the server can hide protocol bugs and drift from production; Trinity cannot minimize, since "Even a small change to the three trees' initial state might change the order of network request scheduling", invalidating seeds that cost "many CPU-days" to find; splitting the global PRNG was under consideration. A separate suite, Heirloom, runs against a real server about 100x slower than Trinity; other end-to-end suites give "admittedly weaker" coverage. — [Dropbox](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine)

### Inferences
- CanopyCheck is the direct analogue of fuzzing Verbatim's reducers alone; Trinity is the analogue of joining reducers through pretend pipes. Dropbox found the cheap pure layer paid early and heavily, and kept a slow real-world suite because the simulator could not cover the platform.
- A single global PRNG makes seeds fragile; per-component random streams (one per simulated process or application) would keep seeds meaningful across small changes.

### Gaps
- No Dropbox figure for harness size, engineer-time, or share of production bugs the simulators would have caught.

## Recurring failure modes across sources

### Takeaway
The same problems recur: nondeterminism leaking from hash maps, clocks, entropy and dependencies; fault models and generators that omit the fault that matters; mocks and oracles that encode a wrong belief; logic escaping the simulated core; simulation time outgrowing its budget; seeds that do not survive code changes; and production still finding bugs.

### Cited Findings
- Hash maps and library time and entropy leaked at S2 and Dropbox. — [S2](https://s2.dev/blog/dst); [Dropbox](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine)
- Unexplained same-seed divergence persisted at Polar Signals (Go). — [Polar Signals Go](https://www.polarsignals.com/blog/posts/2024/05/28/mostly-dst-in-go)
- Fault model omissions: TigerBeetle memory and swap; FDB's operating system contract weaker than believed; Dropbox fsync. — [TigerBeetle](https://tigerbeetle.com/newsletters/2024-12-23-december-in-tigerland); [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf); [Dropbox](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine)
- Excess faults shrink the reachable state space (FDB tunes distributions; TigerBeetle bounds faults for liveness). — [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf); [tigerbeetle issue 696](https://github.com/tigerbeetle/tigerbeetle/issues/696)
- Wrong oracles: TigerBeetle simulator false positives; etcd's linearization checker flaw; RisingWave unit simulation mostly found test-logic bugs. — [February in TigerLand](https://tigerbeetle.com/newsletters/2025-03-05-february-in-tigerland); [etcd](https://etcd.io/blog/2025/autonomus_testing_with_antithesis/); [RisingWave](https://www.risingwave.com/blog/applying-deterministic-simulation-the-risingwave-story-part-2-of-2)
- Mock drift: Dropbox server mock. — [Dropbox](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine)
- Untested driver code: Polar Signals. — [Polar Signals Rust](https://www.polarsignals.com/blog/posts/2025/07/08/dst-rust)
- Cost of runtime: RisingWave over budget; serialized execution slow. — [RisingWave](https://www.risingwave.com/blog/applying-deterministic-simulation-the-risingwave-story-part-2-of-2)
- Skepticism (secondary blog): some on the Go team argue DST "can lull developers into a false sense of security"; small model inaccuracies cause missed edge cases; DST does not measure real performance. — [thrawn01](https://thrawn01.org/posts/determinism-in-software---the-good,-the-bad,-and-the-ugly)
- Production still surprises: S2; FDB in-the-wild bugs reproduced by improving simulation fidelity. — [S2](https://s2.dev/blog/dst); [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf)

### Inferences
- For Verbatim, the highest-value guards are: a replay meta-test comparing logs, deterministic hashing everywhere, coverage counters per interesting state (FDB's `TEST` macro), per-component random streams, a fault model for the COM answer channel (slow, failing, reordered, the application exiting), and keeping the real end-to-end suite for what the shells and real applications do.

### Gaps
- No source states how a team decided DST had paid off beyond qualitative claims.

## Measured outcomes

### Takeaway
Hard numbers are scarce and none give bugs per engineer-week or the share of production bugs a simulation would have caught; what exists is counts of notable bugs, run volumes and speedups.

### Cited Findings
- S2: 17 notable bugs. — [S2](https://s2.dev/blog/dst)
- Polar Signals: three data loss and two duplication bugs (Go) in the first weeks; two critical bugs (Rust). — [Go](https://www.polarsignals.com/blog/posts/2024/05/28/mostly-dst-in-go); [Rust](https://www.polarsignals.com/blog/posts/2025/07/08/dst-rust)
- RisingWave: simulation 4 to 5 times faster than real execution; two months of recovery-bug debugging. — [RisingWave](https://www.risingwave.com/blog/applying-deterministic-simulation-the-risingwave-story-part-2-of-2)
- Dropbox: tens of millions of runs nightly; in-memory mode about 10x faster than native, real-server suite about 100x slower than Trinity. — [Dropbox](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine)
- etcd under Antithesis: 830 wall-clock hours for 4.5 simulated years, one critical bug missed by existing tests. Aiven: 2,200 logical hours, no bugs in their code. — [etcd](https://etcd.io/blog/2025/autonomus_testing_with_antithesis/); [Aiven](https://aiven.io/blog/deterministic-simulation-testing-in-diskless-apache-kafka)
- FDB: 0.5M disk years without corruption at CloudKit; no production bugs reported in the Flow Paxos replacing ZooKeeper. — [FDB paper](https://www.foundationdb.org/files/fdb-paper.pdf)

### Inferences
- The evidence supports DST as a strong finder of concurrency and recovery bugs in logic under the simulator, and as weak evidence of anything outside it.

### Gaps
- No source measured the proportion of production bugs a simulation would have caught, or harness maintenance hours.
