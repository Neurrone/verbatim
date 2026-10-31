# Logic foundations: the designs of M4.5's builds

This document holds the designs of M4.5's builds (`docs/roadmap.md`, "M4.5 — Logic foundations"), one section each, each written against the code of its day. The milestone's goal, its rules, the alternatives weighed and its limits are in [logic tests](logic-tests.md), which each section implements and does not restate; where the code contradicts that design, the section says so.

## The crate split (the first build)

Status: built on 2026-10-31, after its adversarial review (`experiments/logic-tests-study/crate-split-design-review.md`).

The problem: one crate, `verbatim-outpost`, held three programs, the outpost, the focus listener and Core's supervisor, with two-way dependencies between them, so none of their pure logic could be built or tested apart from the Windows code around it, and the platform-neutral dependency check had holes ([logic tests](logic-tests.md), "Crates by role"). The build split it into crates by role, made the listener an executable of its own, and moved code without changing behaviour: no thread, message, byte on a pipe or wake-up changed; only the listener's image name and the log targets of the moved modules did. The program bookkeeping, the failure paths and determinism are later builds of M4.5.

### The crates

The mechanism of each is in its crate guide, [the index](../crates/readme.md) listing them in dependency order.

- The logic tier, in the GPL tier with no Windows dependency: [verbatim-outpost-logic](../crates/verbatim-outpost-logic.md), the outpost's reducer; [verbatim-listener-logic](../crates/verbatim-listener-logic.md), the listener's read scheduler, menu end's grace, foreground watch and focus vocabulary; and [verbatim-reader-rules](../crates/verbatim-reader-rules.md), what both readers apply before reading.
- The roles, Windows-specific: [verbatim-outpost](../crates/verbatim-outpost.md), [verbatim-listener](../crates/verbatim-listener.md) with `verbatim-listener.exe`, and [verbatim-supervisor](../crates/verbatim-supervisor.md).
- Shared, Windows-specific: [verbatim-win32](../crates/verbatim-win32.md), the local window and process queries more than one role makes; [verbatim-process](../crates/verbatim-process.md), which gained the message layer, the launch arguments, the alarm, the held processes and a child's start and end; and [verbatim-ia2](../crates/verbatim-ia2.md), which gained the `WinEvent` thread and `fact_of_win_event`. `verbatim-uia` re-exports `verbatim-win32`'s `provider` module under the same names.
- The supervisor's pure decisions, its generations' typestates and its policy, are `verbatim-core`'s `supervisor` module ([verbatim-core](../crates/verbatim-core.md)), a platform-neutral crate, whose moved comments cite the parity documents rather than NVDA's source.

No edge joins two roles, or the logic crates of different roles; no logic crate reaches a Windows crate; and no platform-neutral crate gained an edge.

### The choices, and why

- Each two-way dependency was broken by moving what both sides used below them, not by an interface between the roles: the acceptance test and the console class to `verbatim-reader-rules`, the alarm, held processes, launch arguments and message layer to `verbatim-process`, and the shared window queries to `verbatim-win32`. Reason: a role crate that depends on no other role is the rule of one-way dependencies, which the graph can hold.
- Two facts the split would have left in two role crates were joined in `verbatim-reader-rules`, each one rule both readers apply: the moved-on grace (`MOVED_ON_GRACE`), whose copy in the reducer went, and the UIA focus wait (`UIA_FOCUS_WAIT`), from which the gap record's bound and the listener's `NEAREST_FAILED_FAST_BOUND` are derived. Reason: no test could tie two copies in crates that may not depend on each other.
- Where a window function had two definitions, one in `verbatim-ia2` and one in the outpost, one definition stays in `verbatim-win32`, under the outpost's longer name where the two differed (`window_exists`, `window_is_visible`, `window_thread`, `window_class_name`, `desktop_window`) and `verbatim-ia2`'s `parent` for the general parent. Reason: each query has one definition, which both backends and every role call.
- The seal stays in `verbatim-supervisor`. The logic-tests design put the generation's typestates in `verbatim-core` "with the `Sealed` token that only they can make", which the code cannot hold: `OutpostMessage` carries a Windows crate's frame type, and the owner, not the typestates, makes four of the sealed notices. So the typestates return a plain `Lifecycle` notice, one variant for each message they made, with the same fields in the same order, and only the owner turns each into its sealed message. The guarantee is kept: only the supervisor's crate makes a notice the app takes as the supervisor's.
- Two executables, both staged beside `verbatim.exe`, found as the outpost and the synthesizer host are found. The arguments are built and parsed by one definition in `verbatim-process`, each with its command line and its parse, tested by a round trip; the listener's begin with `--listener`, so the supervisor's stand-in tests, which run their own binary as both, tell the roles apart as before, and either binary refuses the other's arguments.
- `HeldFacts::hold` takes a lazy clock, which it calls only while no fact is held, so the policy reads no clock of its own and the owner's reads are the ones it made before: one for an application's first held fact. `Received` moved as a plain value; reading it and stamping with it are `verbatim-supervisor`'s functions, their bodies and order of reads unchanged.
- The `Arbitrator`'s pure decisions stay in `verbatim-outpost` in this build; M4.6's design decides whether its tests need them (Dickson, 2026-10-31).

### The tier check in `cargo xtask ci`

The platform-neutral dependency check read normal edges only, did not name `winapi`, and passed a Windows-only workspace crate with no `windows` dependency of its own, such as `verbatim-gui`. It is replaced by a check of the crate tiers (`xtask/src/tiers.rs`), which reads `cargo metadata` once, over normal and build edges for every target:

- Every workspace member is classed in one list: platform-neutral, logic, Windows, or tool. A member in no class fails. Reason: the hole was a crate the check did not know.
- A platform-neutral crate reaches only platform-neutral workspace crates; a logic crate only platform-neutral and logic ones; neither reaches `windows` or `windows-core` by any path, nor `windows-sys` or `winapi` from a workspace crate other than `workspace-hack`. Reason: the platform-neutral tier may depend on no GPL-tier crate, and third-party crates with target-gated bindings are cross-platform code.
- A role's crates reach no other role's. Reason: the rule of one-way dependencies.

### Measured

Before is main at 0bb2c78d, after is this build, both debug builds on the development machine on 2026-10-31, the traces with `VERBATIM_E2E_RUST_LOG=info`.

- Wake-ups and processor time: unchanged. `measure_keys_text_box`'s 8-second window caught more typing after (the input hook woken 684 times against 527), so it is compared per hook wake: about 30 wakes in all processes per hook wake before and 28.5 after. `conhost_flood`'s 6-second window: 3,453 wakes and 499 ms on a processor before, 3,558 and 515 ms after. Idle Verbatim over 8 seconds: 42 wakes and 5.2 ms before, 45 and 6.1 ms after, the listener 5 of them.
- Memory, idle Verbatim read 12 seconds in, debug: private bytes unchanged, the listener 3.77 MB before and 3.89 MB after, each outpost 2.3 to 2.8 MB on both sides. The children's working sets totalled 101.8 MB before and 87.3 MB after, within the spread of one read. Release builds were not measured, for lack of disk.
- Executables, debug: `verbatim-outpost.exe` 17.1 MB before; 16.7 MB after, with `verbatim-listener.exe` 5.2 MB.
- The listener's start, from its launch to "focus listener ready", over 12 local scenario runs each: median about 81 ms before and 82 ms after.
- `cargo xtask ci`'s time: a rerun with nothing changed 206 and 205 s before, 202 and 203 s after; after a one-line change to `verbatim-model`, 298 and 295 s before, 293 and 293 s after.

### Limitations

- Splitting the image costs the listener its sharing of code pages with the outposts.
- Logic that is pure in parts stays in the Windows crates, outside the Linux build and the generated tests until moved: the `Arbitrator`'s keeping of verdicts, the listener's checks before a read, the transport's merges, and the supervisor's owner.
- Window queries that behave alike stay as separate functions: `verbatim-win32`'s `provider::parent_window` beside the general `window::parent`. Joining them is a follow-up.
- `verbatim-uia` and `verbatim-ia2` reach `verbatim-core` through `verbatim-reader-rules` for one type, `ForegroundRelation`, so they no longer compile alongside it; moving that type out of `verbatim-core` is the remedy if the build time matters.
- Every logic crate depends on `verbatim-core`, and through it on `verbatim-text`, whose `jieba-rs` builds `zstd-sys` from C, so none can be checked against the Linux target on this machine without a cross C compiler; GitHub's Linux job builds them.

### Decisions

- The seal stays in `verbatim-supervisor`: the generation's typestates in `verbatim-core` return a plain `Lifecycle` notice, and only the owner makes `Sealed`, which keeps the guarantee the seal gives (Dickson, 2026-10-31).
- `provider.rs` moves from `verbatim-uia` to `verbatim-win32`, re-exported by `verbatim-uia` under the same names, so the window queries have one crate under both backends (Dickson, 2026-10-31).
- `verbatim-listener.exe` and `verbatim-outpost.exe` sit beside `verbatim.exe`, found as the outpost and the synthesizer host are found today (Dickson, 2026-10-31).
- `verbatim-ext-api` is classed platform-neutral, so an extension's SDK never carries the GPL tier (Dickson, 2026-10-31).
- The `Arbitrator`'s pure decisions stay in `verbatim-outpost` in this build; M4.6's design decides whether its tests need them (Dickson, 2026-10-31).
- After the build, Dickson runs `vm\scripts\Enable-VerbatimCrashDumps.ps1` once, elevated, so `verbatim-listener.exe`'s crash dumps are kept.

## xtask on Linux and the Linux job (a build of its own)

M4.5's build that lets `xtask` build on any platform and adds a Linux job to CI, as `docs/design/logic-tests.md` ("Tools and determinism") asks. Written against the code of 2026-10-31 (commit 62c340d1).

### What is Windows-only in xtask today

`xtask` builds only on Windows: `xtask/Cargo.toml` depends unconditionally on `windows` and on workspace crates that depend on it (`verbatim-agent`, `verbatim-control`, `verbatim-e2e`, `verbatim-uia`, `verbatim-uia-rops`), and `xtask/src/main.rs` imports `windows` at the top for `warm-speech`.

By module, from what each one uses:

- Windows-only: `conformance` (windows, `verbatim-uia`, `verbatim-uia-rops`, `verbatim-term-sim`, `verbatim-e2e`), `record` (windows, `verbatim-term-sim`, serde), `nvda` (`verbatim-agent`, `verbatim-control`, `verbatim-model`), `demo` and `vm` (`verbatim-e2e`, `verbatim-config`, `verbatim-agent`, base64, PowerShell and Hyper-V), `park` and `trace` (Windows tools: tscon, schtasks, WPR), `perf` (uses `trace`), and in `main.rs` the functions `warm_speech`, `activate_speech` and `windows_terminal`.
- Portable: `docs`, `ownership`, `waits` and `shapes`, using the standard library alone, and in `main.rs` `ci` itself; and the tier check (`tiers.rs`), since the crate split, which reads `cargo metadata`'s JSON with `serde_json`, xtask's one portable dependency.

#### Gating rules

- A whole module is gated, never single functions inside it: `#[cfg(windows)] mod conformance;` and so on for each Windows-only module, and `#[cfg(windows)]` on the three Windows functions in `main.rs` and on every import only they use (the `windows` imports and `std::time::Instant`, which only `warm_speech` uses), since an unused import fails Clippy with warnings denied. Reason: one attribute per module is easy to audit, and a Windows module never has to keep a Linux branch compiling.
- Every dependency except `workspace-hack` moves to `[target.'cfg(windows)'.dependencies]` in `xtask/Cargo.toml`, including the portable ones (base64, serde, serde_json, `verbatim-model`, `verbatim-config`, `verbatim-term-sim`), since only Windows-only modules use them. Reason: a dependency lives under the same condition as the code that uses it, so the Linux build compiles nothing it does not run. `workspace-hack` stays unconditional because `cargo hakari manage-deps` requires every package to depend on it.
- The Windows-only commands' match arms in `main` and their lines in the usage text carry `#[cfg(windows)]`, so on Linux the usage lists only `ci` and `docs` and any other command gets the usage and exit code 2. Reason: one dispatch path, with no "not supported here" branch to test.
- Hakari's generated output does not change: its `platforms` in `.config/hakari.toml` are the two Windows triples, on which xtask's dependencies are unchanged. The builder confirms it with `cargo hakari generate --diff`.

#### What the checks assume about Windows

- The documentation check (`xtask/src/docs.rs`) is already portable: it builds paths with `/`, compares every component's letter case against directory listings (so a link that differs only in case fails on both systems), and reads lines on `\n` with trailing whitespace trimmed, so both the CRLF checkout on GitHub's Windows runner and Linux's LF checkout read the same. The repository stores every document and fixture with LF (`git ls-files --eol`).
- The scans (`waits.rs`, `shapes.rs`, `ownership.rs`) are portable: paths are normalised with `replace('\\', "/")` or compared with `Path::ends_with`, files are read with `str::lines`, which drops a trailing `\r`, and Windows names such as `Start-Sleep` or `verbatim_input_windows` are only text they search for.
- The graph check is the one host-dependent check: a graph read for the host's target would not show a dependency gated on `cfg(windows)` on Linux, and would pass where Windows fails. Rule: it reads every target's graph (the tier check's `cargo metadata` has no platform filter), so it reads the same graph on every host. Reason: the check's answer must not depend on where it ran. On today's graph, `cargo tree --target all` for every platform-neutral crate reaches `windows-sys` only (beneath third-party crates and `workspace-hack`, which the check allows) and neither `windows` nor `windows-core`. `cargo tree` reads the graph and builds nothing, so this is not a build naming a target. Its workspace-crate test compares paths with `std::path::MAIN_SEPARATOR`, which is correct on both systems.

### `cargo xtask ci` on Linux

`ci` keeps one entry point, and what it runs depends on the host:

- On every host: the tier check, the three scans and the documentation check.
- On Windows, as today: the hakari checks, rustfmt, and Clippy and unit tests for the whole workspace.
- On any other host: Clippy (warnings denied, all targets) and unit tests for the crates in a constant, `LINUX_CRATES`, in `xtask/src/main.rs`, each passed as `-p`.

Rules, each with its reason:

- The crate list lives in xtask, not in the workflow. Reason: `ci` is the standard check so that local runs and GitHub cannot drift, as its own doc comment says.
- rustfmt and the hakari checks run only on Windows. Reason: they read source and metadata the same way on every host, and the Windows jobs already run them.
- `LINUX_CRATES` before the split: the whole platform-neutral tier (`verbatim-model`, `verbatim-text`, `verbatim-speech`, `verbatim-config`, `verbatim-i18n`, `verbatim-input`, `verbatim-audio`, `verbatim-core`) and `xtask`, as the decisions below record. `xtask` is listed so the unit tests of the documentation check and the scans run on Linux. After the split: add `verbatim-outpost-logic`, `verbatim-listener-logic` and `verbatim-reader-rules`.
- No step names a target, so Linux builds for the runner's own architecture, as Windows does.

### What builds on Linux today

Checked on this machine with `cargo check --target x86_64-unknown-linux-gnu --all-targets` into a scratch target directory (a check builds no binaries and runs no tests):

- Checks cleanly: `verbatim-model`, `verbatim-term-sim`, `verbatim-config`, `verbatim-input` and `verbatim-audio`. `workspace-hack` builds as part of each: its unconditional dependencies are portable, and its `windows-sys` and `getrandom` entries sit under Windows-target tables that Linux skips.
- Could not be checked from Windows: `verbatim-text`, and so `verbatim-core`, `verbatim-speech` and `verbatim-i18n`, which depend on it. `jieba-rs` (`crates/verbatim-text/Cargo.toml`) embeds its dictionary through `include-flate`, whose default `zstd` feature builds `zstd-sys`, a C library, for the target. From Windows that needs a Linux C compiler and C library headers, and this machine has neither (Visual Studio's clang finds no `stdlib.h` for Linux). GitHub's `ubuntu-latest` runner has gcc, so this is an obstacle only to checking locally, not to the job.
- No Windows code in the crates the job builds: a search of the eight platform-neutral crates and `verbatim-term-sim` finds no `cfg(windows)`, `target_os` or `std::os::windows`. Fixture paths are joined with `/` from `CARGO_MANIFEST_DIR` (`crates/verbatim-core/tests/replay_fixture.rs`, `crates/verbatim-term-sim/src/conformance.rs`). Fixtures are JSON, where line endings outside strings do not matter, and the flight recorder's dump reader trims `\r` (`crates/verbatim-core/src/dump.rs`).

So no obstacle is known in today's code. Whether `verbatim-text`'s and `verbatim-core`'s tests pass on Linux is proven only by the job's first run, and the builder fixes whatever that run finds within this build.

### The Linux job

- Job `ci-linux` in `.github/workflows/ci.yml`, on `ubuntu-latest`, with the same `needs: changes` and condition as `ci` and `ci-arm64` (a code change, not a run started by hand), so a documentation-only change does not run it.
- Steps: checkout (no submodules: eSpeak NG is not in the Linux crates' graph), `Swatinem/rust-cache@v2`, then `cargo xtask ci`. The toolchain comes from `rust-toolchain.toml` (1.99.0 with rustfmt and Clippy) on the first cargo command, as on Windows. No wxWidgets cache, cargo-hakari or speech warm-up, since none of them runs on Linux.
- The documentation-only path: the `docs` job moves from `windows-latest` to `ubuntu-latest`. Reason: it then builds only xtask's portable part and `workspace-hack` rather than the `windows` crate and the end-to-end crates, so a documentation change gets its answer sooner.
- Expected time: on this machine (12 threads, debug, cold), building the tests of core and its dependencies took 31 s and running them 66 s. The Linux job also runs Clippy, a separate check build of the same dependencies (ICU, `jieba-rs` with `zstd-sys`, the `workspace-hack` set), and `cargo tree --target all` once per platform-neutral crate, which downloads every target's crates on a cold cache, all on `ubuntu-latest`'s 4 processors, so a cold run of 6 to 8 minutes is as likely as 4 to 6. The Windows `ci` job took 8.2, 8.7 and 9.5 minutes in the last three successful pushes and `ci-arm64` 9.9 to 12.4, and the `e2e` job 60 to 67, so running beside them adds no wall time. The first runs give the real figure, and the design records it then. The Windows `ci` job's whole-workspace caches are 349 to 478 MB each, so a Linux cache for a few crates is likely well under a gigabyte; the repository uses 4.4 GB of its 10 GB Actions cache.

#### Before and after the split

- Built before the split: xtask's gating, every target's graph in the dependency check (now the tier check), `ci`'s host-dependent steps with `LINUX_CRATES` holding the platform-neutral tier and `xtask`, the `ci-linux` job, and the `docs` job on Linux.
- Since the split, `LINUX_CRATES` holds the three logic crates too. A logic crate that reaches a Windows crate unconditionally fails to build on Linux, which proves what the tier check reads from the graph; an edge gated on `cfg(windows)` builds on Linux, so only the tier check, reading every target, catches it.

### Verifying locally on this Windows machine

- WSL is not installed (`wsl.exe -l -v`), and Docker is not available.
- A check-only cross target works for every crate that does not reach `verbatim-text`: `rustup target add x86_64-unknown-linux-gnu` (done on this machine on 2026-10-31 for this study), then `cargo check --target x86_64-unknown-linux-gnu --target-dir <scratch> -p xtask --all-targets`, and `cargo clippy` the same way. That proves xtask's gating, which is most of this build's code. It cannot reach `verbatim-core` or the logic crates, for the `zstd-sys` reason above, so their Linux build is verified by the GitHub run, which the standing rules already have every builder read after pushing.
- The rule "no command names a target" (`CLAUDE.md`, "Coding Standards") exists so that each architecture is built and tested on its own machine. A check that produces no binary and runs no test does not weaken that, and `CLAUDE.md` states it as an exception (Dickson, 2026-10-31).

### Documents to change

- `CLAUDE.md`, "Commands": after the sentence on what `cargo xtask ci` runs, add: "On a Linux host it runs the dependency check, the scans and the documentation check, then Clippy and unit tests for the crates listed in `LINUX_CRATES` (`xtask/src/main.rs`); GitHub Actions runs it there as the `ci-linux` job on `ubuntu-latest`, and the documentation-only check also runs on Linux." "Coding Standards" states the two exceptions: the check-only Linux target and `cargo tree --target all`.
- `docs/tooling/setup.md`, its paragraph on the CI jobs: the `ci-linux` job, and the Linux target for the local check.
- `xtask/src/main.rs`'s module comment: which commands exist on which hosts.

### Decisions

- A builder may run a check-only `cargo check` or `cargo clippy` with `--target x86_64-unknown-linux-gnu` on this machine, and xtask's dependency check runs `cargo tree --target all`; both are stated exceptions to "no command names a target", since neither builds or tests (Dickson, 2026-10-31).
- The Linux job builds and tests the whole platform-neutral tier (`verbatim-model`, `verbatim-text`, `verbatim-speech`, `verbatim-config`, `verbatim-i18n`, `verbatim-input`, `verbatim-audio`, `verbatim-core`) and xtask, and the logic crates once the split adds them (Dickson, 2026-10-31).
- WSL is not installed; the GitHub run is the Linux build's check (Dickson, 2026-10-31).

## The outpost's program bookkeeping (a build of its own)

Status: built on 2026-10-31, after its adversarial review (`experiments/logic-tests-study/bookkeeping-design-review.md`), and its implementation review.

The problem: the outpost's programs carried lifecycle states the logic-tests design asks to remove ([logic tests](logic-tests.md), "The outpost's program bookkeeping simplified"). The console poll was a long program in a slot of its own beside the program in flight, yielding to whatever was dispatched behind it, dispatched again afterwards, and carrying its commits to the next program's copies; a test's settle was a program of its own with a waiter slot and a cleanup for an abandoned worker, dispatched again behind an abandoned poll, where its waiters were lost twice; a refused commit still gave the reducer its verdicts; and the worker decided `entered_in_front`, a fact of the reducer's. The mechanism of each part is in [verbatim-outpost](../crates/verbatim-outpost.md) ("Text and caret", "The arbitrator", "The outpost" and the idle barrier) and [verbatim-outpost-logic](../crates/verbatim-outpost-logic.md).

### What was built

- The console poll: each re-read is a program of its own, timed by the reducer's wake for the watch (its poll's next read), dispatched only when nothing else is ready, under the text deadline, committing the answer, the end, or the next read. Removed: the poll's slot, the program held behind it and its re-dispatch, the yield, the worker's held queue, the carry of the poll's commits and text, and the inbox's alarm; the inbox is a plain channel and the worker keeps nothing between programs. Two rules moved to every program: one abandoned while its query is the open watch's takes the watch out and answers it `Abandoned` once, and one that panics or ends without committing ends the watch, which answers it, with no `Failed`.
- Verdicts are a message of their own (`Input::Verdict`), recorded under the same rule as before, taken after the mailbox closes as a commit is; a commit the reducer does not accept is refused whole.
- A test's settle is the idle barrier's item (`Item::Barrier` with `Asker::Settle`), reached as Core's round is and answered as soon as it is reached. `Outpost::settle` then waits for the subscription's moves and the outbound queue on the caller's thread. The shell keeps only the waiters, checked after every transition against the settles waiting in the intake, so the reducer keeps no set of its own.
- `entered_in_front` is decided by the reducer's rule as a report is recorded (`Held::entered_in_front`): a foreground change sets it to its window, any other focus inherits it from the newest report in its top-level window, and otherwise it is the window when read in front. The program reports the evidence (`InFront`), naming a foreground change by its variant, since its event address is that of an MSAA focus on the window's client object.

Posting a program's last commit with its end was dropped: the hop has not been shown to cost, and posting the end before the program's drops would put a blocked release under the next program's deadline and let a settle or the round be answered while the worker still calls into the application.

### The console poll's rule: the options weighed

NVDA's caret loop runs on the main thread until its timeout, 100 ms by default, returning early only for a queued script or a pending focus event; in consoles caret and text change events neither end it nor are handled during it (`nvda/source/editableText.py`, `_hasCaretMoved`).

- Today's yield kept every state above.
- The poll waiting for nothing, an ordinary program in the one slot, would have made whatever arrives wait up to its 50 ms bound, and changed what Core receives: a character typed during an unanswered poll would first reach Core as the key's result rather than its own echo, with no capture of the speech that order gives.
- Each read a program, chosen, keeps today's latency and what Core receives, at more wake-ups on keys that move nothing (below).

Measured on the code before the build, release build, test audio mode, on 2026-10-31 (9 of 10 runs of a temporary `measure_keys_conhost_unmoved` workload and 8 of 10 of `measure_keys_conhost`; the others failed in setup, as `docs/flaky-tests.md` records under "A new console host not responding at its outpost's first call"):

- Ordinary console keys, 261 polls: 0.4% ended at the bound; re-reads per poll P50 0, P99 2, maximum 7. 64 console update reads, the typed character's echo among them, arrived during a poll and ran in P50 1.0 ms, maximum 1.5 ms; waiting for the poll's end they would have waited P50 49 ms, maximum 50 ms.
- Keys that move nothing, 716 polls: 66.5% ended at the bound, with re-reads per poll P50 9, maximum 10, so about 27 to 40 added wake-ups per such key. 152 console update reads and 160 text queries arrived during a poll and ran within 0.5 ms and 1.2 ms.

### Facts with an owner in two places

- The reducer's verdicts and marks, and the mailbox's push-time copies of them: each written only from the reducer's effects.
- Pending settles: the barrier items in the intake, and the shell's waiters, checked against them after every transition. The waiter is only the means of answering, as a reply token is.
- `entered_in_front`: judged by the same pure function on the program's copy and in the reducer, from the same evidence. The two agree at each commit, because the reports are written only by the program in flight's own commits, by the publication at its walk's bound, which claims the program so none of its later commits is applied, and by a release, which comes only with no program in flight.

### Limitations

- A console poll's read, like any program behind it before, can last until the text deadline, since it names no window for the moved-on rule.
- In tests, a settle no longer waits for entries that arrive after it or returns early when a worker is abandoned, divides batches, and leaves the objects the shell let go of to be dropped with the next program.
- A foreground change still does not update the entered window of older reports in its window; that is the roadmap's item of its own.
- The foreground window is read on every non-foreground focus report whose top-level window is known, rather than only when nothing is inherited, the shell's publication at a walk's bound included: a local read with no wake, not measured alone.

### Measured before and after

Wake-ups in all processes per input-hook wake, debug builds, test audio mode, one 12-second trace window each (2026-10-31): `measure_keys_conhost` 17.9 before and 18.1 after; `measure_keys_conhost_unmoved`, keys that move nothing, 14.6 before and 17.5 after, the outpost's share 6.6 and 8.0; `measure_keys_notepad` 32.5 before and 33.5 after. The unmoved workload is a temporary one (branch `measure`). So the rule's cost falls on keys that move nothing, about a fifth more wake-ups per key, and ordinary console and Notepad keys are within a few percent. Processor time and `measure_focus_text_box` were not measured.

### Decisions

- The console poll: each read is a program of its own, timed by the reducer, which removes the poll's slot, the yield, the re-dispatch and the worker's carry, and keeps what Core receives and today's latency; its added wake-ups, on console keys that move nothing, are measured in the build (Dickson, 2026-10-31).
- A test's settle is answered as soon as it is reached, without the idle round's waits for an open caret watch, an open gap record or the hook's held copies, and divides batches as the round does (Dickson, 2026-10-31).

## Determinism (a build of its own)

Status: proposed on 2026-10-31 and revised after its adversarial review (`experiments/logic-tests-study/determinism-design-review.md`). It is written against the crate split's code (branch `split`, f6689fe8).

The build enforces the determinism rule of [logic tests](logic-tests.md), "Tools and determinism", in every crate on the path to a reducer. The rule allows no hash map or set, no clock, no randomness, no thread, no environment or file read, and no other hidden input, and it covers the third-party code those crates call. The build changes no speech, and no thread, message or timer. Paths are given within each crate, since the crates they name exist on the branch only.

### The crates the rule covers

The reducers are Core's reducer (`verbatim-core`), the outpost's reducer (`verbatim-outpost-logic`) and the listener's read scheduler (`verbatim-listener-logic`). The rule covers these crates:

- the three reducer crates;
- every workspace crate they reach by a normal dependency, today `verbatim-model`, `verbatim-text` and `verbatim-reader-rules`;
- every workspace crate that is a development dependency of a covered crate, today `verbatim-term-sim`, the oracle M4.6 builds on.

That is seven crates. `workspace-hack` holds no code. The check derives the list from the dependency graph, so a crate a reducer comes to depend on falls under the rule without anyone listing it.

`verbatim-speech` is not on the path. No covered crate depends on it, and it runs after the reducer, executing its speech effects. It owns the synthesizer threads, the mixer's hand-off and the cancel waker, so the rule could not hold for it, and its four maps are only looked up, never iterated. `verbatim-config`, `verbatim-i18n`, `verbatim-input` and `verbatim-audio` are not on the path either.

### The rules

- **No hash map or set of any kind:** not the standard ones, nor `rustc-hash`, `hashbrown`, `foldhash`, `indexmap` or any other, whoever controls the keys. Every map and set is a `BTreeMap` or `BTreeSet` (Dickson, 2026-10-31).
  - A fixed-hasher map is allowed only as a named exception, where a measurement in the build shows a cost that matters, and it is recorded here with its figure. No exception is taken today.
  - Reason: one kind of map leaves no key classification for review, so Clippy can enforce the rule whole. Iteration order is then a property of the keys, which a replay reproduces whatever the hasher's version.
- **No clock read, and no `Instant` or `SystemTime` value.** A time comes in with an input, as microseconds on the run's shared clock (`verbatim_process::clock::now_us`), as Core's reducer and the outpost's reducer already take it. Reason: an `Instant` cannot be recorded, replayed or built by a generated test.
- **No randomness, no thread, no environment read, no file, process or network call, and no process id.**
- **No `TraceId::mint` or `TraceId::namespace`.** A trace id comes in with the input that caused the work. Reason: the mint is a process-wide counter, so ids minted inside a reducer depend on everything else the process minted first.
- **An answer a transition obtains by a call is an input.** It is recorded in call order with the input it came with (below, "Answers obtained during a transition"). Reason: a replay that does not give the same answers cannot give the same effects.
- **Every third-party crate a covered crate depends on directly is on an allow-list in `cargo xtask ci`.** The functions called from the path were checked from its source and found deterministic (below, "Third-party crates").
- **The covered crates' tests follow the same rules,** except the measurement and fixture-reading files listed under "The enforcement". Reason: M4.6 replays and generates from them, and `cargo xtask ci` runs Clippy on every target.

### The hash maps and sets

All 44 become `BTreeMap` or `BTreeSet`, and `terminal_rows.rs`'s `Spread` hasher and its aliases are removed. Each entry below gives its file, its key, its size bound, and whether its order reaches an effect. Seven of the 44 already used `Spread`.

`verbatim-text`, 9:

- `src/terminal_reading.rs`, `carried`, keyed by old line indexes. Bound: the lines of one read. Order not used.
- `src/terminal_rows/align.rs`, `Likeness::other`, keyed by non-ASCII characters of a row. Bound: the distinct characters of one row. Order not used.
- `src/terminal_rows.rs`, `commonest_shift`'s `shifts`, keyed by row shifts. Bound: the pairs kept. The order is not used, since the tie-break on first place is unique.
- `src/terminal_rows.rs`, six keyed by a row's hash:
  - `Aside::rows`, the set-aside screen's rows, kept in Core's state and dumps. Its JSON form stays a map, now in key order, so a dump written before reads back.
  - `shift_votes`'s `at`.
  - `old_hashes`, `went`, `hashes` and `with_text`, in the alignments.
  - Bound: the rows of the last read, up to a whole document on a lost read. Order not used.

`verbatim-core`, 1:

- `src/terminal_model.rs`, `held_anchors`, passed as `held` through `src/terminal_model/review.rs`, keyed by anchor numbers. Bound: the anchors Core holds for one outpost, its command marks (1,000 at most) and a few positions. Order not used. It is copied out of `HeldAnchors`, already a `BTreeSet`, so the copy goes.

`verbatim-reader-rules`, 2:

- `src/acceptance.rs`, `RequestedEvents::requested`, keyed by an event, a process id and a window class. Bound: what application support asks for, empty today. Order not used.
- In its tests, the test desk's `windows`.

`verbatim-outpost-logic`, 10:

- `src/lib.rs`, `Input::Released`'s `nodes` and `anchors`, keyed by node and anchor numbers. Bound: one release's. Order not used.
  - The sets are passed into `held.rs`, `follow.rs` and `text.rs`.
  - The outpost's shell in `verbatim-outpost` builds them and changes with them.
  - The test sites that build them change too: `follow.rs` once, `held.rs` once, `text.rs` twice and `tests.rs` twice.
- `src/gap_record.rs`, `answered`, keyed by top-level window handles. Bound: the application's answered top-level windows, each removed when its window is destroyed. Order not used.
- `src/intake.rs`, `admit`'s `hung` and `per_thread`, keyed by window handles and thread ids, and the batch plan's `runs`, keyed by entry indexes. Bound: one batch. Each run is sorted on its own, so order is not used.
- `src/text.rs`, `TextFacts::uia` and `edit`, keyed by node numbers. Bound: the store's live nodes with text facts.
  - `uia`'s order reaches an effect today: `release` collects the dropped ranges in iteration order into `Effect::ForgetRanges`.
  - This is the one place found where an order already reaches an effect.
- `src/verdicts.rs`, `Verdicts::windows`, keyed by window handles. Bound: the windows probed, each removed by a forget. Order not used.
- In its tests, `src/tests.rs`, `Recorded::hung`.

`verbatim-listener-logic`, 2:

- `src/schedule.rs`, `Schedule::apps`, keyed by process ids. Bound: the applications with events waiting or being read, each removed when it goes idle. `tick` sorts the ids before walking them, a sort the `BTreeMap` makes unnecessary. The other walks do not depend on order.
- The same file's `tick`'s `reads`, keyed by process ids. Bound: one tick's reads.

`verbatim-term-sim`, 20, all keyed by row text, row ids, shifts or counts:

- In `src/oracle.rs`, the fields `texts`, `shown`, `covered`, `changes`, `lost`, `erased`, `written` and `rotated`.
- In `src/oracle.rs`, the locals `texts`, `left`, `at`, `votes`, `said`, `counts` (twice), `ids` and `found`, with the return types of `alternate_lines` and `unique_rows`.
- In `tests/oracle.rs`, `matched`, `shown` and `found`.

### Answers obtained during a transition

The outpost's reducer calls three functions of its `Locals` trait (`src/lib.rs`) during a transition, implemented live by the outpost's shell and by a recording in tests:

- `window_is_hung(hwnd)`: the system's `IsHungAppWindow`, asked while a batch is planned.
- `moved_on(window)`: a live foreground query, asked while a read is judged for abandonment.
- `publication(request, facts)`: a worker's walk in progress, read and marked published at the walk's bound, built into a `Change`.

Each answer decides effects, so each is an input of its transition. The build documents this on the trait, and makes no other change to the reducer's interface. The alternative was to turn each call into an input that the shell supplies before the transition. `window_is_hung` would then be asked for every window of every waiting entry instead of for those the plan reaches. `publication` would become an effect and an answering input, adding a waiting state to a program at its bound. These are more calls and one more lifecycle state, for a recording that already gives the same replay.

M4.6's recorder captures, for each transition, the input, its `Now`, and the `Locals` calls in order. Each call is recorded with its kind, its argument and its answer: for `publication` the request and the returned `Change`. The facts it was given are not recorded, since they are the reducer's own state. A replay's `Locals` answers from that list, and fails when a call's kind or argument differs from the recorded one, which shows that the replay diverged. `Change` is not serializable today, and `Input::Commit` carries one as well, so making it serializable is M4.6's work in either case.

`publication` stays the one read of another thread's progress inside a transition. Its live answer depends on how far the worker got. That is the system's own timing, and the recording fixes it for a replay.

No other covered crate calls out during a transition. The listener's scheduler is given its reads as `TickRead` inputs. `verbatim-reader-rules`'s `WindowQueries` is called only from the listener's and the outpost's workers in the Windows crates.

### Clocks and trace ids

No production code in the covered crates reads a clock, draws a random number, starts a thread or reads the environment. Two hidden inputs remain.

Core's supervisor (`verbatim-core`, `src/supervisor/`) takes `Instant` values:

- in `policy.rs`: the wedge and retirement decisions, restart timing, `HeldFacts`, and the spare outposts' `wanted` and `due`;
- in `generation.rs`: `last_pong_at` and the transitions;
- in `mod.rs`: `Received::at`.

`HeldFacts::hold` takes a clock function, which `verbatim-supervisor` passes as `Instant::now`, so today the clock is read inside the logic's call. The change:

- Each of these takes and returns microseconds on the shared clock, and `Received` keeps only `at_us`.
- Differences use `saturating_sub` where they use `saturating_duration_since` today, since a plain subtraction panics in the debug builds the end-to-end suite runs.
- `a_now_before_the_last_pong_never_panics` keeps its case, with constants.
- `hold` takes the time as a value. `verbatim-supervisor` reads `now_us()` at each `hold` call (`owner.rs`'s three), at the point where the closure is passed today. That is one counter read per held fact instead of one per application, only while an outpost starts. It leaves the logged `first_fact_held_us` where it is.
- `verbatim-supervisor` turns a due time into a wait. `now_us` reads the same performance counter as `Instant`, so no decision changes.

Trace ids. Every production mint on the path gets a source:

- `Input::Cancel`'s "not started" reply (`src/lib.rs`): `Intake::cancel` returns the trace id of the withdrawn query's entry, which came with Core's query, and the reply carries it.
- The gap record's bound (`gap_bound_entry`): the record keeps the trace id of the newest copy or foreground change beside the stamp it already keeps, and its bound's entry carries it. `copy_dropped` and `accepted` take the trace id with their stamp.
- The read of a dropped copy (`gap_note`): `GapNote::CopyDropped` gains the trace id of the copy's event, which the worker already holds where it builds the note (`verbatim-outpost`, `worker.rs`, `copy_dropped_for_uia`).
- The outpost's wake (`src/intake.rs`, `Item::Wake`): `Applied`'s wake carries the trace id of the watch whose deadline set it, which the follow module holds with each open watch.
- `verbatim-model`'s `src/protocol/samples.rs`, a public module of samples for other crates' tests, mints twice and uses the new constructor instead.

`verbatim-model` gains `TraceId::from_raw(u64)` for tests and samples. Its static counter stays for the processes that observe events. 312 test sites mint today, and each takes `from_raw` with a fixed number:

- `verbatim-model`: 8;
- `verbatim-core`'s unit tests: 17;
- `verbatim-core`'s integration tests under `tests/`: 260, 196 of them in `reduce.rs`;
- `verbatim-outpost-logic`: 26;
- `verbatim-listener-logic`: 1.

This is a mechanical edit of about 320 lines.

The remaining shared state stays, since no answer from it reaches a result:

- `verbatim-text`'s `OnceLock` holds the Chinese word segmenter, built from embedded data.
- `verbatim-listener-logic` logs through `tracing`.

### Third-party crates

The allow-list of direct third-party dependencies (normal, build and development) of the covered crates is `serde`, `serde_json`, `icu_locale_core`, `icu_normalizer`, `icu_properties`, `icu_segmenter`, `jieba-rs`, `unicode-width`, `tracing`, `proptest` and `vte`.

The tier check (`tiers.rs` in xtask) holds the list and fails on any other. Its comment states the rule for adding a crate: the functions called from the path, and the crate's own dependencies they reach, are checked from source for:

- a randomly seeded hash map whose order reaches a result;
- a clock, randomness, a thread, the environment or a file;
- global mutable state that changes a result.

The verdict goes into this section with its evidence.

Each crate below gives its version from the lock file, what the path calls, and the verdict, with file and line in its source.

- **`jieba-rs` 0.11.0.** Called: `Jieba::new` and `cut(text, true)`, for Chinese words. Deterministic.
  - The dictionary is embedded at build time (`src/lib.rs` 103, `include_flate::flate!`).
  - Its maps are `rustc-hash`'s fixed `FxHashMap` (`src/lib.rs` 81 and 83, `src/hmm.rs` 363), only looked up.
  - Its trie uses its own fixed multiplicative hash (`src/trie.rs` 92 and 130).
  - The thread-local `SCRATCH` (`src/lib.rs` 173) holds buffers reused between calls and cleared for each, so no output depends on an earlier call.
  - Its dependencies have no clock, randomness, thread, environment or file read in their source: `rustc-hash` 2.1.3, fixed seed 0 (`src/lib.rs` 79, 98); `include-flate` 0.3.4, which decompresses the embedded data once; `bytecount` 0.6.9.
- **`icu_segmenter`, `icu_normalizer`, `icu_properties` and `icu_locale_core` 2.2.0.** Called: grapheme, word (`new_auto`, and `try_new_auto` for a language) and sentence segmenters, NFC, the general category and script maps, and `LanguageIdentifier::try_from_str`. Deterministic.
  - Their data is compiled in (the `*_data` crates, 2.2.0).
  - A search of their source and of `icu_provider`, `icu_collections` and `icu_locale` 2.2.0 finds no hash map, clock, thread, environment or file use.
  - The word segmenter's LSTM, for Thai, Lao, Khmer and Burmese, computes with `core_maths` 0.1.1, which calls the pure Rust `libm` (`src/lib.rs` 1 to 4), not the platform's C library, so the results are the same on x64 and ARM64.
  - The only randomness in `zerovec` 0.11.6, `zerotrie` 0.2.4, `tinystr` 0.8.3 and `writeable` 0.6.3 is in their tests (`zerovec` `src/hashmap/mod.rs` 217, `zerotrie` `src/byte_phf/mod.rs` 226, `tinystr` `src/ascii.rs` 889, `writeable` `src/impls.rs` 79 with a fixed seed). `zerotrie`'s perfect hash covers fixed data.
- **`unicode-width` 0.2.2.** Called: the width of a character and of a string. Deterministic: static tables, and a search of its source finds none of the hazards.
- **`serde` 1.0.228 (with `serde_core`).** Called: derived serialization. Deterministic: its hash map impls are reached only by hash maps, and the covered crates have none after this build.
- **`serde_json` 1.0.150.** Called: `from_str` and `to_string`. Deterministic.
  - Its object map is a `BTreeMap` unless the `preserve_order` feature is on (`src/map.rs` 3, 4 and 24); nothing enables it, since `indexmap` is not among its dependencies in the lock file.
  - Numbers are written by `itoa` 1.0.18 and `zmij` 1.0.23, with no hazard in their source.
  - `memchr` 2.8.3 caches its choice of processor routine in a static, which changes speed, not results.
- **`vte` 0.15.0.** Called by `verbatim-term-sim`: `Parser::new` and `advance`. Deterministic.
  - Its one clock read (`src/ansi.rs` 460) is in the `ansi` module, compiled only with the `ansi` feature (`src/lib.rs` 40 and 41), which nothing enables: its lock file entry depends only on `arrayvec` 0.7.8 and `memchr`.
- **`tracing` 0.1.44 (with `tracing-core` 0.1.36).** Called by `verbatim-listener-logic`: the event macros `info!`, `warn!` and `error!`. Deterministic for the reducer.
  - The dispatcher is global mutable state (`tracing-core` `src/dispatcher.rs` 192 to 202).
  - It decides only whether an event is written and returns nothing to the caller.
  - The scan forbids `enabled!`, `event_enabled!` and `span_enabled!` in the covered crates, since their answer would bring the dispatcher's state into the logic.
- **`proptest` 1.12.0.** Called by `verbatim-term-sim` and `verbatim-text`'s tests: `TestRunner::new` with `random_config()` (`verbatim-term-sim`, `src/oracle.rs`). That config sets every field: a fixed ChaCha seed, no failure persistence, no fork and no timeout. Deterministic as called.
  - Its default config reads `PROPTEST_*` environment variables and persists failures to files (`src/test_runner/config.rs` 70 and 186 to 192).
  - Fork and timeout start processes and threads.
  - So the scan forbids `proptest!` and `Config::default` in the covered crates, and a test builds its config from `random_config()`.

### The enforcement

Clippy enforces the rule, through `disallowed-types` and `disallowed-methods` in a `clippy.toml` in each covered crate's directory. The ownership scan and the tier check guard Clippy's configuration and what Clippy cannot see.

Checked with Clippy 0.1.99 on a scratch crate:

- A crate's own `clippy.toml` applies to that crate alone. The workspace's `[lints]` table is unchanged.
- The two lints are in the `style` group, and `-D warnings` denies them.
- A disallowed type is reported at `use`, in a type, in `collect::<HashMap<_, _>>()`, in a local alias, and in `#[cfg(test)]` code under `--all-targets`.
- A third-party alias such as `rustc_hash::FxHashMap` is not reported.
- `#[allow(clippy::style)]` or `#[allow(warnings)]` silences both lints.
- A path that does not resolve in a crate is ignored without a warning, so one file can serve all seven crates.
- Each entry is shown to fire once by a deliberate violation when the build lands. Every entry below except `verbatim_model`'s two was shown to fire on the scratch crate.

Types:

- the hash collections: `std::collections::HashMap`, `std::collections::HashSet`, `std::hash::RandomState`;
- time: `std::time::Instant`, `std::time::SystemTime`;
- files: `std::fs::File`, `std::fs::OpenOptions`, `std::fs::DirBuilder`, `std::fs::ReadDir`, `std::fs::DirEntry`, `std::fs::Metadata`;
- processes: `std::process::Command`, `std::process::Child`;
- network: `std::net::TcpStream`, `std::net::TcpListener`, `std::net::UdpSocket`;
- threads: `std::thread::Builder`, `std::thread::JoinHandle`, `std::thread::Thread`.

Methods:

- time: `std::time::Instant::now`, `std::time::Instant::elapsed`, `std::time::SystemTime::now`, `std::time::SystemTime::elapsed`, `std::time::SystemTime::duration_since`;
- threads: `std::thread::spawn`, `std::thread::scope`, `std::thread::sleep`, `std::thread::current`, `std::thread::park`, `std::thread::park_timeout`, `std::thread::yield_now`, `std::thread::available_parallelism`, `std::thread::panicking`;
- the environment: `std::env::args`, `std::env::args_os`, `std::env::var`, `std::env::var_os`, `std::env::vars`, `std::env::vars_os`, `std::env::current_dir`, `std::env::set_current_dir`, `std::env::current_exe`, `std::env::temp_dir`, `std::env::home_dir`, `std::env::set_var`, `std::env::remove_var`;
- files: every free function of `std::fs` (`read`, `read_to_string`, `read_dir`, `read_link`, `metadata`, `symlink_metadata`, `canonicalize`, `exists`, `write`, `copy`, `create_dir`, `create_dir_all`, `remove_file`, `remove_dir`, `remove_dir_all`, `rename`, `hard_link`, `soft_link`, `set_permissions`);
- the process: `std::process::id`, `std::process::exit`, `std::process::abort`;
- trace ids: `verbatim_model::TraceId::mint`, `verbatim_model::TraceId::namespace`.

Each entry has its reason.

The ownership scan (`xtask/src/ownership.rs`) gains a determinism rule over the covered crates, read from the graph the tier check reads:

- **One configuration.** Each covered crate has a `clippy.toml` identical to `verbatim-core`'s, and that file names every entry the scan lists. Reason: Clippy has no include, so seven copies are kept equal by a check.
- **No silencing.** No `allow` or `expect` names `warnings`, `clippy::all`, `clippy::style`, `clippy::disallowed_types` or `clippy::disallowed_methods`. Each covered crate's manifest has `lints.workspace = true` and no `[lints]` table of its own. The workspace's `[workspace.lints]` does not name `all`, `style` or either lint, and `.cargo/config.toml` passes no lint flags.
- **Its listed exceptions.** These files may `expect` the two lints, with a reason, and no others may:
  - `verbatim-text` and `verbatim-core` `tests/terminal_cost.rs`, ignored release measurements that time with `Instant`;
  - `verbatim-core` `tests/replay_fixture.rs`, which reads its fixture file;
  - `verbatim-term-sim` `src/conformance.rs` and `tests/conformance.rs`, which read the conformance corpus.
- **No whole module.** The covered crates' code names no path in `std::fs`, `std::net`, `std::process`, `std::env` or `std::thread`, and none in `std::time` but `std::time::Duration`, in a `use` tree or written out. The listed exception files are excepted. Reason: Clippy names functions one by one, so a function added to the standard library later would pass it, and the scan closes that by module.
- **Forbidden macros and calls.** No `enabled!`, `event_enabled!`, `span_enabled!`, `proptest!` or `Config::default` (see "Third-party crates").
- **Narrower allow-lists.** The pure rule's allow-lists for the outpost's reducer and the read scheduler narrow `std::collections` to `BTreeMap`, `BTreeSet` and `VecDeque`.

The tier check gains the third-party allow-list and its rule, as described under "Third-party crates".

### Performance on the event path

These maps on the event path become `BTreeMap`s:

- the outpost's `Verdicts::windows`, looked up for each event;
- the gap record's `answered`, for each UIA focus;
- `admit`'s two maps, once per batch;
- `TextFacts`, for each text event;
- the scheduler's `apps`, for each listener event.

Each holds tens or hundreds of integer keys, so a lookup is a few comparisons in one or two nodes, against SipHash of an integer today. No change is expected, and it is measured. No thread, timer or message is added, so wake-ups should not change.

The worst case is the terminal's alignment of a lost read, which reads the whole document. Its budget is the reducer's 2 milliseconds for a whole read ([the terminal model](terminal-model.md), "Costs"). Today that read takes 1.2 to 1.4 ms over a full default history of 9,001 rows (`docs/performance.md`, "A terminal's rotation").

A console application can enlarge its buffer to 32,766 rows, and a lost read then reads all of it. Scaled by rows, today's alignment would take about 4 to 5 ms there, past the budget before this build. That is a prediction, and the build measures it.

Measured before and after, in release builds:

- `verbatim-text`'s `terminal_cost` (`cargo test --release -p verbatim-text --test terminal_cost -- --ignored --nocapture`), with two cases added: a lost read over a 32,766-row history, and a gap of Chinese text, for `Likeness::other`'s per-character lookups.
- Wake-ups and processor time per thread on `measure_keys_conhost` and `measure_focus_text_box`, in test audio mode, at most 20 runs each.
- The time of `verbatim-term-sim`'s oracle tests under `cargo xtask ci`.

The fallback is taken if the lost read's median time rises by more than 10% over today's at either history size. The five transient structures keyed by row hash (`at`, `old_hashes`, `went`, `hashes` and `with_text`) then become one vector of hash and row pairs per alignment, sorted once and searched by binary search: still ordered, with no hasher.

`Aside::rows` stays a `BTreeMap`, so Core's state and the dump format are untouched. It is looked up for the rows of each read while a full-screen program runs, and for each row of a lost read's document.

If even the fallback costs more than 10%, the figure decides between a named fixed-hasher exception for those five and accepting the cost, a question for Dickson with the figure.

### End-to-end runs and the order of work

The commits, in order, each with its unit tests and the crate guides updated where an API changes:

- **The 44 maps and sets as `BTree` collections.** `Spread` is removed, and `Input::Released` changes in the outpost's shell. Measurements first, before and after. Scenarios: `conhost_full_history_flood`, for the terminal's alignment, and `notepad_editing`, for the outpost's verdicts, gap record, batch admission and text facts and the listener's scheduler.
- **Times and trace ids as inputs.**
  - The supervisor's policy and generation move to the shared clock.
  - `HeldFacts::hold` takes a value.
  - The four production mints get their sources.
  - `TraceId::from_raw` is added, with the 312 test sites and `samples.rs` moved to it.
  - `Locals` is documented as inputs.
  - Scenario: `notepad_editing`, which starts an outpost from a spare and holds facts while it starts.
- **The enforcement.** The seven `clippy.toml` files, the scan's rule, and the tier check's allow-list, each entry shown to fire once. It changes no behaviour, so it runs no scenario.

Each scenario runs three times at the end of the build. Neither is in `docs/flaky-tests.md`.

### Limitations

- Third-party code is checked by reading it when it is added, and again only when a version change is reviewed. M4.6's run of each case twice is the backstop.
- `publication` reads another thread's progress inside a transition. The recording makes a replay exact, but the live answer follows the worker's timing.
- A `tracing` event's field expressions run only when the event is enabled, so a field with a side effect would make the logic depend on the subscriber. That is a review rule.
- A terminal row is compared by its 64-bit hash alone, so two texts with the same hash align as the same row. The hash is a fixed public function of the application's text, so an application could make that happen on purpose. This predates the build and is not changed by it (a follow-up).

### Decisions

- `BTreeMap` and `BTreeSet` everywhere on the path, and a fixed-hasher map only where a measurement in the build shows a cost that matters, named here with its figure (Dickson, 2026-10-31).
- The audit covers the third-party crates the covered crates call, from their source, and a crate added to the allow-list later gets the same check (Dickson, 2026-10-31).

### Questions for Dickson

- **`verbatim-speech`:** the original decision named it among the crates on the path, but no reducer depends on it, and it owns the synthesizer threads and the mixer's hand-off, so the rule cannot hold for it. Recommended: leave it out, with its four maps as they are.
- **A 32,766-row lost read:** if the build's measurement confirms that this read is past the 2 ms budget before any change, should the build record it as a limit or plan work to bring it within budget? Recommended: record it as a limit, since it needs an application that enlarges its buffer, and keep the fallback rule above for this build's own cost.

## Failure paths and locks (a build of its own)

Status: proposed on 2026-10-31 and revised after its two adversarial reviews (`experiments/logic-tests-study/failure-paths-design-review.md`, `experiments/logic-tests-study/failure-paths-design-review-2.md`). Written against the crate split's code (origin/main d62e7740); files of crates the split creates are named with their crate.

The build implements [logic tests](logic-tests.md), "Failure paths and locks": fault points at the hand-offs whose failure paths held defects, wakes decided by reading the state, and a debug-build check that no call into an application is made under one of the outpost's locks. Dickson decided on 2026-10-31 that fault points are compiled in only behind a Cargo feature that the tests turn on, absent from release builds and the end-to-end build, and that the lock check panics in debug builds.

### Where the code contradicts the logic-tests design

- The port refusal of de7f481e was in a parsing helper that the split no longer has: every frame is parsed on the reducer's thread (`frames.rs` in `verbatim-supervisor`). What remains is the relay's delivery to the app's port, the `Outbox` in `owner.rs` in `verbatim-supervisor`, which keeps a refused message and every later one and delivers them again at the next send and at every heartbeat.
- The taskbar list of 76180512 no longer starts a thread: its request is a `Responder`, whose drop answers "gone". The thread starts left on a request's path are the listener's reading thread, which answers its queued events when it cannot start (`ScheduleInput::SpawnFailed`), and the outpost's worker, which does not: `worker::spawn` calls `expect`, from `Shell::start` for generation 0 and from the shell's handling of `Effect::Abandon` for a replacement. A failed replacement panics the reducer's thread while the I/O thread goes on answering pings, so no program of that outpost is answered again.
- The full channel of 0d1e7802 is gone, but the operation it guarded is not: the hook's callbacks post each key's commands to the reducer's completion port with `Poster::send` (`key_reports.rs` in `verbatim-app`), and the speech effect posts a cancel to the speech queue's port (`SpeechControl::cancel_through`); each drops a post the port refuses. A dropped report can leave the harness's `AwaitIdle` unanswered and Core's view of held keys wrong. The operation is now "the hook's post refused", fixed below.
- The locks of d0ec3a89 and fc899340 no longer exist in that form: the object store's API returns owned clones and never a guard. The check turns that guarantee, today a property of the API's shape, into a failure in every debug run.
- A feature turned on by a dev-dependency would be unified into every target `cargo test --workspace` and `cargo clippy --workspace --all-targets` build, executables included, which on this platform carry no hash and so overwrite the shipped ones in `target/debug`; and ci would then never build the configuration that ships. So no dev-dependency turns the feature on (next section).

### Fault points

Rules:

- A fault point is a call `verbatim_faults::hit(Fault::…, &subject)` placed immediately before the operation it stands for; when it fires, the code takes the path the real failure takes, with the error the real call returns. Reason: the test must run the production failure path, not a copy of it.
- The registry is a new crate, `verbatim-faults`, in the Windows tier, depending only on `workspace-hack` as every package does: a `Fault` enum naming each point, and a table of armed points under one lock. Each point passes a subject naming its call: the item posted, the thread and generation started, the registration and element or handler. A test arms a point with `arm(fault, selector, times)`, where the selector is a predicate on the subject, so the test names the one call that fails; the returned guard disarms the point when dropped and asserts it fired exactly `times` times. Reason: a count alone fails whichever call reaches the point first, so a test could not fix its path; and a fault that never fired proves nothing.
- Points compile under `cfg(any(test, feature = "faults"))`, so a crate's own unit tests reach its points with no feature. Each crate with points reached from another crate's tests has a `faults` feature, which nothing turns on but one command, `cargo xtask faults`, which `cargo xtask ci` runs as its fault step and builders run by hand: `cargo clippy` and `cargo test` of the fault tests with `--features faults`, one package at a time, into the target directory Dickson chooses (a question below). Those tests are test targets with `required-features = ["faults"]`, so the ordinary steps skip them; mockapp's `faults` feature turns on optional normal dependencies on the crates whose points its tests reach, since a feature cannot name a dev-dependency. Reason: the ordinary steps then build and lint exactly the shipped configuration, and one command keeps a hand-run fault test from being built some other way.
- Tests that arm points run one at a time in their binary, since the table is process-wide.

Each operation, its point, and the test that checks the request is still answered, the work still taken and the shutdown finished:

- The relay's delivery to the app's port (de7f481e). Point `Fault::PortRefusesPost` in `Poster::send` (`crates/verbatim-process/src/port_queue.rs`), before `PostQueuedCompletionStatus`, with the item as subject; when it fires the item is given back, as a refused post gives it back. The app's mapping of a refusal to `Undelivered::Refused` moves from `outpost_sink` in `verbatim-app` into a constructor `EventSink::posting` in `verbatim-supervisor`, so the test runs the app's mapping. Test `fault_paths::a_message_the_port_refuses_is_delivered_again_in_order_and_shutdown_ends`, a new test target of `verbatim-supervisor` beside `target_gone.rs`, with the same stand-ins: it refuses the post of `ListenerReady`, of which a run has exactly one, then checks the sink takes `ListenerReady` before every spare's `Started`, each message once, and that `Supervisor::shutdown` returns its summary.
- The relay's mailbox wake (d6f627a5). Point `Fault::RelayWake` in `Mailbox::send` at the post (next section), with the `OwnerEvent` as subject. Unit test `a_refused_wake_leaves_the_item_for_the_heartbeat_take`, in `relay.rs`, single-threaded on a real port: the refused send leaves no completion on the port, the take the heartbeat makes returns the item, and the next send into the then empty queue posts. Test `fault_paths::a_shutdown_whose_wake_the_port_refuses_still_ends`: it waits until the sink has delivered `ListenerReady` and `SpareReady` with `waiting` equal to `SPARES`, after which every launch has been taken and no thread sends to the owner, so the queue is empty when `Supervisor::shutdown` sends; the refusal is selected on `OwnerEvent::Shutdown`; it checks `shutdown` returns with every child ended, which takes up to one ping interval, 3 s.
- The hook's post refused (0d1e7802's operation). Fix: a post the port refuses is kept and posted again ahead of the next key's, on the hook thread, with no lock and no timer, as the relay's `Outbox` carries a refused message on its next send.
  - Both of the hook's callbacks in `key_reports.rs` post through one function on the hook thread, which keeps a thread-local queue of refused posts, each either a command for the reducer's port or a cancel for the speech queue's. `SpeechControl::cancel_through` returns whether its post was taken, instead of dropping a refused one. A post is made only when that queue is empty; otherwise it joins the queue, and the queue is posted from its front until a post is refused again. A refused post whose port is still open is kept; one whose port has closed is dropped, as its taker has gone. Reason: one queue across both ports keeps the hook's order, so no command or cancel overtakes a refused one, and the reducer's `SpeechCancelled` and the speech queue's cancel are both kept or both made.
  - Sequence: key 1's command is refused and kept; key 2's command finds the queue non-empty and joins it, and the queue is posted, key 1's first. With no key 2, key 1's command waits (a limitation below). While posts succeed, nothing is added but one check of an empty thread-local queue. No timer, callback or message is added to the hook thread. Reason: a refusal comes only from a system out of memory, which Verbatim cannot clear, and a timed retry would wake the hook thread on a clock with no evidence that a later post succeeds.
  - Point `Fault::KeyPostRefused`, a `cfg(test)` point in `key_reports.rs` before each port's post, with the post as subject. Unit tests `a_refused_key_command_is_posted_before_the_next_keys_commands` and `a_refused_speech_cancel_is_posted_before_the_next_keys_posts`, against real port queues, select the post by its `key_seq` and check each port takes every post once, in the hook's order.
- A thread failing to start (76180512). Point `Fault::ThreadStart` at the two request-path starts, with the thread's name and generation as subject: the listener's reading thread (`spawn` in `focus_read.rs` in `verbatim-listener`, a `cfg(test)` point) and the outpost's worker (`worker::spawn`, behind the feature), each returning the `io::Error` that `Builder::spawn` returns. `worker::spawn` returns that error instead of panicking; what the outpost then does is a question for Dickson, and the recommended rule is this. A generation with no worker is the state the shell already handles: `Shell::dispatch` answers a program whose generation has no worker as panicked, so the reducer goes on (`shell.rs` in `verbatim-outpost`). A failed start, of generation 0 in `Shell::start` or of a replacement, leaves the generation without a worker, logged at error; and before answering a program that way, `dispatch` tries once to start the generation's worker, warm as `Shell::start` would start it, so the outpost serves again as soon as a thread can be had. When no worker can be had, `dispatch` leaves the shell's garbage in `self.garbage` instead of moving it into the job, and drops only the program, so the next worker to start drops the garbage under its program's deadline. Reason: the garbage holds the application's objects, and releasing one can call into a hung application, which the reducer's thread must never do. Nothing is joined, no state is added, and every program has its one answer. Tests: `a_reading_thread_that_cannot_start_answers_its_events_and_the_next_event_is_read`, a unit test beside the readers' others, which checks each queued event's outcome is `Dropped(NO_THREAD)`, an idle barrier waiting on them is answered, the next event for the process starts a thread whose read is delivered, and the readers stop; `failure_paths::a_first_worker_that_cannot_start_answers_its_program_and_starts_with_the_next`, which fails generation 0's start and the first dispatch's, then checks the first focus query is answered as failed, the second with its read, and `Outpost::shutdown` returns; and `failure_paths::a_replacement_worker_that_cannot_start_answers_the_next_program`, which has generation 0 abandoned on mockapp's held provider, as the slow-application tests do, fails generation 1's start once, and checks the next program is answered by a worker of generation 1 started at its dispatch.
- A group's removal failing (1bfb6f87). Point `Fault::RemoveGroup` in `Follower::remove` (`crates/verbatim-uia/src/subscribe.rs`), before `RemoveEventHandlerGroup`, with the registration and the element's runtime id as subject, so the group stays live as a failed removal leaves it. Tests in mockapp's new `failure_paths`, counting live registrations with mockapp's `common::advised`: `a_group_a_move_failed_to_remove_is_removed_at_close` selects the old element's removal once: the old element keeps its group after the move, and has none after the close, whose `done` is answered; `a_group_whose_removals_all_failed_is_removed_when_the_registering_thread_ends` selects the old element's removal twice, at the move and at the close's second try, while the new element's removal succeeds: the old element's group outlives the close, is gone after `end_registering_thread`, and a registration made afterwards starts a fresh thread and delivers its events.
- A focus handler's removal failing on a failed setting up (61a4bfa1). Points `Fault::AddFocusHandler` in `Follower::set_up`, before `AddFocusChangedEventHandler`, and `Fault::RemoveFocusHandler` in `Follower::close`, before `RemoveFocusChangedEventHandler`, each with the registration and the handler's index as subject. A desktop-wide focus handler is invisible on mockapp's desktop (1bfb6f87's message), so the registering thread's count of kept registrations is read through `verbatim_uia::subscribe::kept_for_tests`, compiled only with the feature. Test `failure_paths::a_focus_handler_left_by_a_failed_setting_up_is_kept_until_the_registering_thread_ends`: a registration with two focus parts, the second handler's add and the first handler's removal selected; the `Start` is answered with the add's error, the kept count is one, the thread ends, and a later registration works.

### Wakes decided by reading the state

The search covered every atomic and fence outside tests. A flag counts when one thread reads it to decide whether to wake another. Five do.

The relay mailbox's `posted` (`relay.rs` in `verbatim-supervisor`). Replaced by the queue itself under one lock, as the outpost's mailbox keeps `asleep` under its queue's lock: the state is a `VecDeque` of items and a `closed` mark. The crossbeam channel and the atomic go.

- `Mailbox::send` locks; if `closed`, it returns `OwnerGone` and drops the item; otherwise it notes whether the queue was empty, pushes, unlocks, and posts the wake only if it was empty. `Inbox::take` locks and takes the whole queue. Dropping the `Inbox`, which the owner thread owns and drops however it ends, panics included, locks, sets `closed`, and takes the queue, dropping its items after unlocking. Reason: the channel's disconnection was the only signal that the owner had gone; without the mark a `Shutdown` would wait in a queue no one takes.
- Sequence, owner gone: the owner's wait fails and `Owner::run` returns, dropping the `Inbox`: `closed` is set and the queue dropped. A later `Supervisor::shutdown` gets `OwnerGone` and returns the default summary. A `Shutdown` pushed before the drop is dropped with the queue, its reply sender with it, so `reply_rx.recv()` fails and `shutdown` returns the default summary as well.
- The wake is posted outside the lock, so no system call is made under it. A refused post is logged, and the item waits for the relay's heartbeat take, which already exists, since the queue stays non-empty and later sends post nothing. Reason: a refusal happens only when the system is out of memory, and one rule for a wake is simpler than a retry mark.
- No wake is lost: sender A pushes into an empty queue and posts; the relay takes the wake, then the whole queue, A's item included; sender B pushes either before that take, into a non-empty queue, and is taken by it, or after it, into an empty queue, and posts again. So every item is taken by the take that follows the post of its queue's first item.
- None spins: the relay waits on the port; each wake is one post, each post one push into an empty queue, and each take empties the queue, so a wake finds the queue empty only when a heartbeat took its item first, at most once per post. Today a send during the relay's drain posts a wake whose item that drain already took, so wake-ups stay equal or fall.
- Cost: `note_views` and `ensure_spawned` send from the app's reducer thread on the event path, and now take an uncontended lock where the channel's send took none; the measurement below includes it.

The listener's watched read (`WatchedRead` in `focus_read.rs` in `verbatim-listener`): `front_judged` and `front_elsewhere` decide whether learning a window wakes the watchdog, and `claim` is read by the reading thread outside any lock. Each read keeps its own `WatchedRead`, so an abandoned read that learns its window later still touches only its own facts. Within it, the three become one lock of its own holding the claim and the front judgement, unjudged or judged with its answer. The window and its top-level window stay atomics: each is written only by the reading thread and wakes no one.

- `learn` stores the window, then takes the read's lock: unless the read is claimed or already judged, it judges the front with `moved_on`'s local calls and records the answer; it wakes the watchdog's alarm, after unlocking and without the readers' lock, only if it recorded the judgement itself and the front is elsewhere. The watchdog's tick, under the readers' lock, takes each read's lock to read the claim and the judgement, judging the front itself when the window is known and the read unjudged. Abandoning or publishing sets the claim under the read's lock.
- No wake is lost: if the reading thread judged, it wakes the alarm after recording, and the alarm's event is auto-reset, so the wake ends the watchdog's next wait even when the watchdog was not waiting yet; if the watchdog judged, it acts in that tick. Exactly one of them judges, under the read's lock.
- None spins: a read is judged once, and only the thread that judged it wakes, so a read makes at most one wake, as today. An abandoned read's late `learn` finds its claim set and neither judges nor wakes.
- Cost on the event path: the reading thread takes the read's lock at most twice per read (`known` and `learn`), where today it makes an atomic swap and loads; the lock is contended only while the watchdog's tick holds that read's lock. Today's readers'-lock acquisition before a wake goes. The ledger states the acquisitions per read and their time, measured by an ignored test in a release build, and the reading threads' latency joins the measurement below.

The audio device notices' `failed` (`crates/verbatim-audio/src/notices.rs`), which decides whether an arrival wakes the audio thread. The audio thread takes no lock and reads these flags on every pass, so the flags stay; instead every notice wakes the audio thread, which decides from `needs_reopen` in its pass. No notice is lost: a notice sets its flag, then wakes, and the pass after the wake reads the flag, whatever the open's progress. Cost: one audio-thread pass per device arrival, a capture device's included, while a device is open.

The cancel token's `cancelled` (`CancelToken` in `crates/verbatim-speech/src/driver.rs`), stored outside the lock under which `on_cancel` reads it to decide whether to wake at once. It moves into that lock with the waker; `on_cancel` reads the field in its own hold rather than calling `is_cancelled`, which would take the lock again. Cost: a driver's `is_cancelled`, called about once per synthesized chunk, takes an uncontended lock instead of reading an atomic.

The readiness signal (`Readiness` in `crates/verbatim-app/src/harness.rs`): its setters, the reducer thread for the listener and the focus, the GUI thread for its handle and the supervisor's report of its spares, each set a part, then read the others to decide whether to set the harness's event. A store followed by loads of other flags can be reordered, so two setters can each miss the other's part; only the outposts' lock taken in between prevents it today. The flags and `signalled` move into one lock, and each setter records its part and decides, in the same hold, whether every part is ready and the event not yet set. The GUI's handle and the outposts' map stay outside it and are written before their setter takes the readiness lock. Only the end-to-end harness names the event.

Examined and kept: the mixer's atomics (`crates/verbatim-audio/src/mixer/mod.rs`) are the state itself on a thread that takes no lock, every push wakes the audio thread, and the room event's waiter checks `released` again (631ee56d); the port queue's counts and `open`, since every item is its own packet; published values no thread wakes on (the outpost's `target_pid` and `abandoned`, the speech manager's `paused`, the hook's `on`, the control server's subscriptions); and counters that number things.

Marks used within one thread: the hook's thread-local marks already clear in `WinEventHook`'s drop, and the worker's `refused` is meant never to clear. The one that can stay set wrongly is the GUI's `switching` (`crates/verbatim-gui/src/lib.rs`), set when a synthesizer switch is asked and cleared only by its answer, which never comes if the queue thread drops the request. `SwitchDone` (`crates/verbatim-speech/src/lib.rs`) becomes a `Responder`, whose drop answers that the switch failed, so the mark clears on every path that drops the request.

Measured before and after: wake-ups and processor time per thread, in release builds and test audio mode, traced with `cargo xtask trace waits`, at most 20 runs each, on `measure_focus_text_box` and `measure_keys_notepad`: the relay's owner thread, the app's reducer thread (the mailbox's lock), the listener's reading threads, with their read latency, and watchdog, the synth thread and the audio thread. Expected: the relay's wake-ups equal or fewer, the rest unchanged.

### The debug-build lock check

Rules:

- Every lock `verbatim-outpost` declares becomes an `outpost::lock::Lock<T>`, a wrapper over `std::sync::Mutex` whose `lock` is `#[track_caller]`. Today these are the context's (`application_names`, `gap_timer`, `shell`, `arbitrator`, `classic_windows`, `fetches`, `foreground`, `focus_window`, `window_readability`), the shell mailbox's state, the store's state and its replaced objects, the worker's charge, and the terminals' spare buffers. A `clippy.toml` beside the crate's manifest disallows `std::sync::Mutex` there, as the determinism build does for the pure crates. Reason: a lock declared without the wrapper would escape the check.
- The record is per thread, in `verbatim-win32`, which both backends and the outpost depend on: eight slots, each empty or holding a lock's location. Taking a `Lock` fills the first empty slot with the caller's location and the guard keeps the slot's index; dropping the guard empties that slot. A ninth lock held at once raises the check's own failure, below, so it too ends the process where a catch would hide it. The record is reached with `try_with`, so a guard dropped while the thread's locals are being destroyed does nothing. Reason: guards may drop in any order, and each clears only its own slot, so the record is right whatever the order; the count of filled slots is the number held.
- The check, `held_locks::check(what)`, finds the filled slots; if there are any, it writes a message naming `what` and every location to standard error and to the log, then panics with a payload of its own type. Reason: the default panic hook prints a payload of any type but a string as `Box<dyn Any>`, so without the message written first an abort would say nothing of which check fired or which lock was held. It runs at every call into the application the backends count (`count` in `verbatim_uia::calls` and `verbatim_ia2::calls`), at the making of an agile reference (`agile` in `verbatim-uia`'s shapes and in `verbatim-ia2`'s accessible), which can call into the element's process, at the entry of `BoundedClient::run`, a wait on another thread's call, and at the drop of the store's `Removed`, where the outpost releases application objects. Reason: those are the ways the outpost's own code says a thread reaches the application.
- A caught panic cannot hide it: the three places that catch a panic around application calls, the worker's `catch_unwind` around each program, UIA's `guarded` around event callbacks, and `BoundedClient`'s thread, abort the process in debug builds when the caught payload is the check's; the check's message is already in standard error and the log. Reason: a program answered as panicked can still pass a scenario, and only the process's end is seen by every test and run.
- In release builds the wrapper is the mutex and its guard, with nothing recorded and nothing checked.
- Condition variable waits go through the wrapper, which hands the inner guard to the wait and keeps the slot. Reason: a thread blocked in a wait makes no call.

Tests, unit tests in `verbatim-outpost`: `a_call_counted_under_an_outpost_lock_panics` on a plain thread; `a_lock_dropped_out_of_order_leaves_the_other_held` (take A and B, drop A, count a call: the panic names B); and `a_call_under_a_lock_inside_a_worker_program_aborts_the_process`, which runs its own binary as a child that runs a program through the worker's catching wrapper, taking a `Lock` and counting a call, and checks the child ends with the abort's status and its standard error names the lock's location. A lock found held across a call when the check lands is fixed in this build; one that must be held across a call by design stops the build and is reported.

Cost in debug builds: at each acquisition one thread-local access to fill a slot, at each release one to empty it, and at each check one scan of eight slots. Measured by an ignored test, `measure_lock_check`, timing a million acquisitions with and without the record in a debug build, multiplied by the acquisitions per focus change and per key, counted under the same `cfg` on `measure_focus_text_box` and `measure_keys_notepad`. `docs/performance.md` gets an entry, "The debug-build lock check", with the cost per acquisition, the acquisitions and added time per focus change and per key, the build mode, and that release builds carry none. Reason: the end-to-end suite's latencies are measured in debug builds.

### Where this build meets the bookkeeping build

The bookkeeping build is changing the outpost's reducer, shell and worker. This build touches the shell's dispatch for a generation with no worker, `Shell::start`, its handling of `Effect::Abandon`, `worker::spawn`'s signature, the worker's `catch_unwind` site, and every lock in the shell's mailbox, the worker's charge and the store. So this build starts after that one lands, and its lock commit, which touches every lock site, comes last. The listener's watched read is code the focus rework's second build will restate after M4.5; its change here is kept to the read's own lock.

### End-to-end runs and the order of work

Three commits, each with its tests and the crate guides of changed APIs (`verbatim-supervisor`, `verbatim-listener`, `verbatim-speech`, `verbatim-outpost`, `verbatim-uia`, `verbatim-process`, `verbatim-app`, and a new guide for `verbatim-faults`):

- Wakes decided by reading the state, and the switch's responder. First, so that the fault tests run against the new mailbox. The mailbox's unit test `a_refused_wake_is_posted_again_by_the_next_send` goes with the rule it tested.
- Fault points and their tests, the hook's kept posts, `EventSink::posting`, the worker's failed start, and `cargo xtask faults` as the fault step of `cargo xtask ci`.
- The debug-build lock check and its ledger entry.

Scenarios, each run three times at the end of the build: `second_application_and_verbatim_menu` (launches and spares through the relay's mailbox, and the listener's reads), `rapid_tabbing_in_settings` (the watched reads, the hook's posts and the outpost's locks under load), and `switch_to_onecore` (the switch's responder and the cancel token). None has an entry in `docs/flaky-tests.md`. `cargo xtask ci`'s time and the fault step's disk use are measured before and after.

Lifecycle states:

- Removed: the relay's `posted` and its crossbeam channel; the watched read's `front_judged` and `front_elsewhere`, replaced by one judgement field; the cancel token's separate `cancelled` atomic and the readiness atomics and `signalled`, each now a field under the lock beside the facts it goes with.
- Moved: the watched read's claim, into the read's own lock.
- Added: the relay queue's `closed` mark, needed while `Supervisor::shutdown` must answer after the owner ends; the hook thread's kept posts, which exist only while a post was refused; in debug builds, each thread's lock slots; with the feature only, the fault table, its guards and `kept_for_tests`. The worker's failed start adds none under the recommended rule, which reuses the generation with no worker.

### Limitations

- A message the app's port refuses, or a mailbox wake the relay's port refuses, waits up to a ping interval, 3 s; both happen only when the system is out of memory.
- A key's post that a port refuses waits for the next key, with no time limit, since nothing else runs on the hook thread; this too happens only when the system is out of memory. A harness waiting on that key is still answered: `AwaitIdle` fails at its timeout, naming the input outstanding.
- The lock check covers the outpost's own locks, and the calls, agile references, bounded waits and store releases named above; an application object released under a lock elsewhere, or a lock of another crate taken on an outpost thread, is not checked.
- The switch's responder cannot answer while the synth thread holds the request in a driver call that never returns.
- A fault point stands for the operation's failure, not for the system's state behind it: a refused post is tested, a system out of memory is not.

### Questions for Dickson

- An outpost whose worker cannot start, at its start or as a replacement. Recommended: the generation stays without a worker, each program is answered as failed by the shell's existing branch, and each dispatch first tries once to start the worker, so the outpost serves again when a thread can be had; no state or process end is added, and the application is silent only while threads cannot be had. The alternative ends the outpost from the shell's thread with `end_without_detach`, never joining the abandoned worker, so Core's request table answers its requests and the supervisor relaunches it: one more exit path, a self-termination the standing rules allow only with evidence, and a relaunch that needs the same threads.
- Where the fault tests are built, by `cargo xtask faults`. Recommended: in `target/debug`, beside the shipped build, where the feature's variants of the libraries are cached under their own hashes, adding the libraries of the crates with points and of those depending on them, a fraction of `target/debug`'s 1.3 GB, and only their compile time after the first run; the one executable it rewrites is `mockapp.exe`, which the end-to-end suite's own build relinks without the feature before it runs, so the suite never runs a fault point, unless a build is staged by hand through `VERBATIM_E2E_VERBATIM_EXE` straight after a fault step. The second option is a separate `target/faults`, which never touches `target/debug`, but whose first build in each worktree compiles every third-party crate again, `windows` included, about another 1.3 GB and a full debug build's time. The third keeps fault points only in each crate's own `cfg(test)` code, with no second build, and loses the tests that need a real provider or the supervisor's stand-ins: both UIA removal tests, the focus handler test, the worker's start tests, and the supervisor's delivery and shutdown tests. The build measures the first two options' disk and time.
- The lock check, which you decided panics. Recommended: it panics, and where a panic around application calls is caught, in the worker, UIA's event callbacks and the bounded client's thread, the debug build aborts the process, since there the panic would otherwise end as one program answered as failed, which a scenario can still pass. The alternative is to panic only and have each mockapp test check a process-wide record of violations afterwards, which the end-to-end suite would not see.
