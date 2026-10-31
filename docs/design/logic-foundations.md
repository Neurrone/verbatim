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

- The console poll: each re-read is a program of its own, dispatched by the reducer when the watch's next read is due and nothing else is ready, under the text deadline, committing what it found (the caret watch's build, below). Removed: the poll's slot, the program held behind it and its re-dispatch, the yield, the worker's held queue, the carry of the poll's commits and text, and the inbox's alarm; the inbox is a plain channel and the worker keeps nothing between programs. Two rules moved to every program: one abandoned while it holds the watch answers it `Abandoned` once, and one that panics or ends without its result ends the watch, which answers it, with no `Failed`.
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

## The caret watch (a build of its own)

Status: built on 2026-11-01 on branch `caret-watch`, from main c9751273, after its adversarial review (`experiments/logic-tests-study/caret-watch-design-review.md`). Dickson approved building it first in M4.5 (2026-10-31), from candidate one of the simplification study (`experiments/logic-tests-study/radical-simplification-study.md`), because the watch had had six rounds of fixes, past `CLAUDE.md`'s rule of three. It builds no part of [the caret and selection rework](caret-selection-rework.md) and changes nothing that rework decides. The mechanism is described in the crate guides: [verbatim-outpost-logic](../crates/verbatim-outpost-logic.md) for the watch's states and transitions, [verbatim-outpost](../crates/verbatim-outpost.md) ("Text and caret") for the checks and the console poll, and [verbatim-core](../crates/verbatim-core.md) ("Held caret keys") for the held key's deadline.

A caret key's watch is the outpost's open check for a key's evidence (`docs/design/key-holding.md`). Its query is answered once: with the evidence, `WatchEnded` with none, `Abandoned`, or by the outpost's end.

### The problem

The fact "is a watch open, for which query, and when does it end" had six holders: the reducer's follow state with its poll, the intake's one wake time (a copy of the watch's next due time, handed out as `Item::Wake`), the program in flight's `running` and `deferred`, every program's copy, the worker's local copy during a check (carried through a cut as `Finish::Watch`), and Core's wait. It changed by eight kinds of commit, the reducer had four special cases for it, and the worker decided its times by five reads of its own clock, so a replay could not reproduce it. Four of the nine defects its fix commits record (6b0bf0d4, d7ec6b04, 463f2ef9, 17767cbf, 60dc079c, b5d2588d) were a copy of its time or state that went stale, and one a state no one held.

### What was built

The reducer holds the watch as one state (`Watch`: `None`, `Waiting` until its bound, `Polling` with its next and last read, `Checking` while the program in flight holds it, with what it goes back to). A program reports what its check found and the facts it read, and the reducer decides every time, on the run's shared clock. The program's copy applies the same transitions with no clock. A program commits three kinds of change: `Open`, `Take` and `Checked`.

The rules, each with its reason:

- The reducer changes the watch outside the program in flight's own commits only at a dispatch, before the copy is taken, or with no program in flight; the one exception is a walk's publication, which claims its program so none of its later commits is applied. Reason: the program's copy then equals the reducer's state at every commit it makes, so a take on the copy and on the reducer agree.
- An answer is sent only by a transition that leaves `None`. Reason: a query gets at most one answer, and every state has a path to `None`.
- A check's result names no node and no request, and is taken only from the program in flight while the watch is `Checking`. Reason: the abandoned poll read of 463f2ef9 matched by node, and a request number would copy what `accepts` decides.
- A due time is acted on only when no program is in flight, no release is pending, the outpost is not stopping and the intake has nothing ready, and `next_timer` gives it only then. Reason: a waiting event's check comes before the bound, and a time nobody acts on would make the reducer's thread spin (17767cbf).
- Times are on the shared clock, not the observation clock. Reason: tests freeze or move the observation clock but never the shared clock, so a bound on the observation clock would never come in such a test.
- The next caret key's dispatch ends the open watch. Reason: the reducer sees the next key at dispatch, so the worker's `End` commit on every caret key went.
- A report on another node or a replaced node ends the watch in any state, and a release of its node ends it. Reason: Core has forgotten that key; a watch left open on a released node held the idle answer up to 10 seconds (found by the design review).

Removed: `Item::Wake` and the intake's wake, `Applied::wake`, `Change::aside`, `Poll` and `wake_for`, the worker's `wake`, `read_poll`, `end_watch` and its clock reads that decided the watch, the reducer's four special-case blocks, and the watch's use of `Current::running`.

Counts, before and after: holders of the watch's state 6 to 3 (the reducer's state, each program's copy, the worker's local during a check), Core's wait apart; change kinds 8 to 3; special-case blocks in the reducer 4 to none; clock reads in the worker that decide the watch 5 to none; intake item kinds 10 to 9 and program kinds 4 to 5; `OpenWatch` fields 8 to 6. The watch module is 245 lines of code, doc comments and blank lines aside; outside it, `follow.rs` lost 173 lines, the worker 80 and the intake 35, and the reducer's `lib.rs` gained 41.

### The held caret key's one bound

A held key had two bounds, decided in two places: Core's 100 ms bound on its wait for the key ahead's watch, and the shell's 150 ms deadline on its before read, which the shell gave by matching the request's kind. Core now keeps one deadline for the key it names (`HeldKeys::bounded`): 100 ms after the key ahead was passed on while the key waits, 150 ms after its read is asked while the read is in flight. One effect sets the shell's slot or clears it when the wait or the read ends sooner, and one input enters when it passes. Reason: one owner decides both waits, the shell no longer judges the before read by its kind, and no timer fires for a wait that ended early.

A key still being read at its deadline is passed on without the read, and Core gives the read up: the shell removes its request, so the harness's idle condition no longer counts it and its outcome is dropped, and withdraws it from the outpost, which answers a read not yet started "not started" once; a read in flight finds nothing and its own answer stays the one answer. Reason: without it the read stayed outstanding in Core's table until the outpost answered it, up to about 12 seconds behind a program in flight, holding the harness's idle condition (Dickson, 2026-11-01; it replaces this design's limitation).

### Step two: the watch lent at dispatch, not built

Lending the watch to a program at dispatch would remove `Take`, `Applied::taken` and the second application of the watch's changes to the copy, about 60 more lines. It was not built: the reducer cannot tell which programs will check the watch, since the worker resolves an event's node through its store, so it would lend the watch to most programs while the user edits and hold the bound and the poll for all of them; and on abandonment it could not tell a program that had begun its check from one that had not, so keeping today's answers needs the program to say when its check begins, which is `Take`. The caret rework changes which programs open and check the watch, so the question is asked again after it.

### What is heard

Nothing changes. What differs, none of it heard: the old watch's `WatchEnded` is sent at the next key's dispatch rather than at its program's first commit; a program that holds the watch and panics or is claimed answers `WatchEnded` where it answered `Failed`, both reaching Core's branch for a reply that is not a caret; the watch's times and the reply's `published_at_us` are the reducer's, a mailbox hop after the worker's clock read; and a watch whose node is released ends then. Measurement sees one difference: that `WatchEnded` carries the watch's timing, so the app's latency ledger records it. NVDA's behaviour is not changed by this build, so nothing new is claimed of it.

### Wake-ups

Calls into the application are unchanged. Each caret key's program posts one commit fewer (the `End`); a watch reaching its 10-second bound no longer runs a program; a wake taken early no longer runs one; each poll read is unchanged; and a held key has at most one shell deadline, none for a wait or a read that ends sooner. Wake-ups in all processes per input-hook wake, debug builds, test audio mode, info logging, 12-second trace windows (2026-11-01), before and after: `measure_keys_conhost` 17.1 and 17.9; `measure_keys_conhost_unmoved`, keys that move nothing, 16.2 and 17.4; `measure_held_repeat_text_box`, the mean of three runs each, 14.3 and 13.4; `measure_keys_notepad`, one window each, 31.7 after, its before window of 60.9 an outlier against 32.5 on the same code earlier. The scenarios `text_box_editing`, `conhost_editing` and `conhost_up_typing` passed 3 of 3 each.

### Tests

The watch's replay tests in `crates/verbatim-outpost-logic/src/tests.rs` were rewritten to the new results, with new ones for each behaviour: the bound with no program, an entry ready at the bound dispatched first, a take's result putting a polling watch back as it stood, the dispatch ending the old watch before the copy is taken, a claimed holder, a release, and the bound reached by the shared clock with the observation clock frozen. The module's own transitions are enumerated in `src/watch.rs`. A generated test (`src/tests/generated.rs`) drives `Reducer::input` and `Reducer::tick` through every sequence of up to six steps, 104,682 of them in about 1.5 seconds in a debug build, against the model of the open query and whether a program holds it, checking the four invariants after every step. Core's key-ahead tests use the one effect and input, and new tests cover the read given up at its deadline and the withdrawn request.

### Limitations

- The program copies stay, so the rule that the reducer changes the watch only at a dispatch, at a publication that claims the program, or with no program in flight is checked by the generated test, not enforced by the types.
- A program's copy cannot decide a poll read's next step, which needs the clock; after a poll read's result the copy shows the watch polling where the reducer may have ended it. No poll read reads the watch after its result.
- No scenario reaches the 10-second bound or a release of the watch's node; those ends are covered by the unit and generated tests only, as is the path that passes a held key on at its read's deadline.

### Decisions

- Step two, the watch lent to a program at dispatch, is not in this build: `Take` stays, and the question is asked again after the caret and selection rework, which changes when the watch opens (Dickson, 2026-11-01).
- The held key's one bound is built in this build. When Core's deadline passes the key on during its read, Core ends the read's entry in the request table and withdraws the read from the outpost, so the harness's idle check does not wait for it; the design's first version left the entry outstanding, up to about 12 s behind a program in flight, and that is fixed rather than recorded as a limit (Dickson, 2026-11-01).

## Determinism (a build of its own)

Status: the first of its three builds, determinism of the logic crates, built on 2026-11-01 on branch `determinism-1`, from main 8893fd97, after the design's reviews (`experiments/logic-tests-study/determinism-design-review.md`, `determinism-design-review-2.md`, and Dickson's purity audit, `experiments/logic-tests-study/purity-audit.md`). The second, the other pure owners, built on 2026-11-01 on branch `determinism-2`, from main 5997bd24; its desktop scenarios and wake-up measurements are still to run. The third is proposed below as designed. The mechanisms are described in the crate guides: [verbatim-outpost-logic](../crates/verbatim-outpost-logic.md) for the reducer's asks, [verbatim-outpost](../crates/verbatim-outpost.md) for the shell's answers and the hung-window test, [verbatim-core](../crates/verbatim-core.md) for the supervisor's liveness and `due_starts`, and [verbatim-model](../crates/verbatim-model.md) for `TraceId::from_raw`; the enforcement is in `xtask/src/tiers.rs` and `xtask/src/ownership.rs`.

The determinism rule of [logic tests](logic-tests.md), "Tools and determinism", holds in every crate on the path to a reducer. The rule allows no hash map or set, no clock, no randomness, no thread, no environment or file read, and no other hidden input, and it covers the third-party code those crates call. The work is three builds ("The order of work"): determinism of the logic crates, the other pure owners, and simpler types. They change no thread, message or timer, and what is heard only where named.

### The crates the rule covers

The reducers are Core's reducer (`verbatim-core`), the outpost's reducer (`verbatim-outpost-logic`) and the listener's read scheduler (`verbatim-listener-logic`). The rule covers these crates:

- the three reducer crates;
- every workspace crate they reach by a normal dependency, today `verbatim-model`, `verbatim-text` and `verbatim-reader-rules`;
- every workspace crate that is a development dependency of a covered crate, today `verbatim-term-sim`, the oracle M4.6 builds on.

With `verbatim-input`, below, that is eight crates. `workspace-hack` holds no code. The tier check derives the rest of the list from the dependency graph (`covered` in `xtask/src/tiers.rs`), so a crate a reducer comes to depend on falls under the rule without anyone listing it.

`verbatim-speech` is not on the path. No covered crate depends on it, and it runs after the reducer, executing its speech effects. It owns the synthesizer threads, the mixer's hand-off and the cancel waker, so the rule could not hold for it, and its four maps are only looked up, never iterated; it stays outside the rule (Dickson, 2026-10-31). `verbatim-config`, `verbatim-i18n` and `verbatim-audio` are not on the path either. `verbatim-input`, the keyboard decision machine's crate, is covered too, and Core's request table is in `verbatim-core` ("The other owners"); `verbatim-input` is on no reducer's path, so `REDUCERS` in `xtask/src/tiers.rs` lists it by hand.

### The rules

- **No hash map or set of any kind:** not the standard ones, nor `rustc-hash`, `hashbrown`, `foldhash`, `indexmap` or any other, whoever controls the keys. Every map and set is a `BTreeMap` or `BTreeSet` (Dickson, 2026-10-31).
  - A fixed-hasher map is allowed only as a named exception, where a measurement in the build shows a cost that matters, and it is recorded here with its figure. No exception is taken today.
  - Reason: one kind of map leaves no key classification for review, so Clippy can enforce the rule whole. Iteration order is then a property of the keys, which a replay reproduces whatever the hasher's version.
- **No clock read, and no `Instant` or `SystemTime` value.** A time comes in with an input, as microseconds on the run's shared clock (`verbatim_process::clock::now_us`), as Core's reducer and the outpost's reducer already take it. Reason: an `Instant` cannot be recorded, replayed or built by a generated test.
- **No randomness, no thread, no environment read, no file, process or network call, and no process id.**
- **No `TraceId::mint` or `TraceId::namespace`.** A trace id comes in with the input that caused the work. Reason: the mint is a process-wide counter, so ids minted inside a reducer depend on everything else the process minted first.
- **No call during a transition.** A decision that needs a live answer moves to where the call is made, or the reducer asks by an effect and is answered by an input (below, "The outpost's local reads as inputs"). Reason: a reducer that calls out cannot be replayed from its inputs, and logic is a pure reducer with calls as its effects (`CLAUDE.md`, "Design principles").
- **No floating-point function from the platform's library.** The transcendental functions of `f32` and `f64` go to the platform's C library on Windows, whose results may differ between platforms and versions. The exact operations stay allowed.
- **Simpler where purity allows.** A change that makes an owner pure prefers the option that removes states, and each owner is simplified where that has a stated benefit ("Simpler owners").
- **Every third-party crate a covered crate depends on directly is on an allow-list in `cargo xtask ci`.** The functions called from the path were checked from its source and found deterministic (below, "Third-party crates").
- **The covered crates' tests follow the same rules,** except the measurement and fixture-reading files listed under "The enforcement". Reason: M4.6 replays and generates from them, and `cargo xtask ci` runs Clippy on every target.

### The hash maps and sets

The audit found 44 in the covered crates. Two, the batch plan's `hung` and the tests' `Recorded::hung`, went with the local reads; the other 42 are `BTreeMap`s or `BTreeSet`s, but for the terminal alignment's five transient lookups below. Iteration order reached an effect in one place: the outpost's text facts collect the ranges a release drops into `Effect::ForgetRanges` in node order. `Input::Released`'s sets changed with the outpost's shell and store, Core's terminal review takes the held anchors' set as it is rather than a copy, and the listener's scheduler no longer sorts the process ids its map now orders.

The terminal alignment of a lost read is the worst case: it looks up every row of the whole document by hash. Kept in `BTreeMap`s and `BTreeSet`s, its five transient lookups by row hash (`at`, `old_hashes`, `went`, `hashes` and `with_text`) cost 12 to 31% more than the fixed-hasher maps they replaced, so the design's fallback was built: each is one sorted vector of hashes, searched by binary search (`Hashes` in `crates/verbatim-text/src/terminal_rows.rs`), still ordered and with no hasher, at 1 to 9% more ("Performance on the event path"). The `Spread` hasher is removed. `Aside::rows` stays a `BTreeMap`, so Core's state and the dump format are untouched: its JSON form is a map in key order, and a dump written before reads back.

The second build converted 12 more, outside the logic crates' reducers: `verbatim-input`'s 7 (`GestureMap`'s `bound` and `observed`, the decision machine's `held_modifiers` and `swallowed_downs`, and 3 in `scripts.rs`'s tests), the request table's `entries` and the outbound backlog's `reported`. The supervisor shell's `records`, `apps` and `spare_children` went in the first build, with `due_starts`.

### The outpost's local reads as inputs

The outpost's reducer called three functions of a `Locals` trait during a transition. Each left the reducer, so that it is pure, with calls as its effects (Dickson, 2026-10-31); the trait is gone, and `input` and `tick` take only the input and its `Now`. For each, four designs were weighed: (a) remove the need, moving the decision to where the call is made; (b) the fact's owner pushes an input when the fact changes; (c) the fact is attached to the event that needs it at intake; (d) an effect that asks and an input that answers. Where they were otherwise equal, the one that removes states was preferred.

**Whether a window is hung: (a), tested at handling.** The batch plan's `admit` tested each UIA entry's window (`IsHungAppWindow`, 0.41 to 0.42 µs a call on this machine) about once per planned batch. Now the worker tests a UIA event's or focus read's own cached window first as it handles it, a window of 0 passing as NVDA's test passes a sender with no cached window (`nvda/source/UIAHandler/utils.py` 438 to 452), before any window lookup, verdict probe or read.

- Rejected: (a′), testing at arrival as NVDA does in each UIA handler, which would read an entry taken in before the 5-second mark and still waiting after it, where today it was dropped; (b), since Windows gives no notice of a window becoming hung, so it needs a polling timer in every outpost; (c), which behaves as (a′) and keeps the test in the reducer; (d), the only option that left what is heard exactly as before, at a transition per planned batch and a state for a batch waiting for its answer.
- What is heard changes in one narrow case: a focus read planned before its window became hung and handled after it, because programs ahead of it were slow, is dropped, where before it was read, met the hang, and could be spoken from a half-read focus at its walk's bound (Dickson, 2026-10-31). The reverse case changes too: a UIA entry whose window was hung when its batch was planned and has recovered when it is handled, which `admit` dropped, is now read, as NVDA reads it.
- An entry from a hung window, which `admit` dropped for nothing, now costs a dispatch and a `Finished`: one worker wake-up and one shell wake-up. A dropped event's parked element and active-text-position range are taken from the store and go with it; before, an entry the plan dropped left them in the store until the outpost ended.
- MSAA entries are tested by the worker's `readable`, as before, which is not the UIA rule: it also drops window 0, windows that no longer exist, and windows a ghost stands in for.

**Whether the user moved on: (d).** The reducer emits `Effect::AskMovedOn { request, window }` where it called the read, and the shell answers `Input::MovedOn { request, moved_on }`. (a) was rejected because the decision to abandon is the reducer's, which owns the generation, so the shell could judge it only from a copy of the program in flight; (b) and (c) would read the front window's owner and provider on every foreground fact, where the read is made only for programs past their grace, and would judge the last fact delivered rather than the window as it is. Calls are as before plus the answer's `Now`, three clock reads, only for slow programs.

**A walk published at its bound: (d).** The reducer emits `Effect::Publish { request, facts, then_abandon }`, clears the walk's bound, and the shell answers `Input::Published { request, change, then_abandon }`. The shell claims the walk's progress from the charge of `request`'s own generation, completes it with local calls, and writes it into the store, so none of that happens inside a transition. `then_abandon` carries an abandonment that asked for the publication and comes back in the answer, so the reducer keeps nothing between the two and today's order holds: the publication is committed, then the program abandoned. Rejected: (a), since the worker is inside the slow call its bound exists for, and a timer of the shell's would make it a second owner of the bound; (b), several messages and shell wake-ups per focus for a rare case; (c), since a bound has no event.

**The rule for asks.** The shell answers each ask exactly once, as the reducer's next input, before it takes another message, and the reducer applies an answer only while its request is the program in flight. Reasons: each ask has one answer path; nothing comes between an ask and its answer, so effects keep their order; and an answer out of turn is dropped rather than applied to another program. A transition makes at most one ask: a tick judges the deadline first (abandoning, by one `Publish` carrying the abandonment when a walk is armed), then the walk's bound (`Publish`), then the moved-on rule's next judgment (`AskMovedOn`, "The work in flight, shared by both readers"), the first that applies deciding, and a judgment left over is made by the publication's answer against its own `Now`. Judged the other way round, a late tick past both the bound and the deadline published and then abandoned before the answer, losing the published focus and trying the group's older entries in its place (the second review's first finding). A `MovedOn` answer applies whether or not its program is claimed, so a published program still stuck is abandoned at once.

The reducer's inputs and effects went from 15 each to 17 each, with no lifecycle state added, and the `abandoning` flag an earlier revision proposed is replaced by `then_abandon`. The reducer's tests answer its asks as the shell does; reducer tests cover the late tick and moving on during a walk with answers out of turn, and a shell test (`an_ask_is_answered_before_the_shell_takes_another_message`) shows a publication's answer, and the abandonment it completes, carried out within the one step of the shell's.

### The other owners

Dickson's purity audit (`experiments/logic-tests-study/purity-audit.md`) found impurities in owners outside the logic crates' reducers. A fact with two writers is a defect of the design principles, so it is fixed, not recorded. The work falls into two builds, as "The order of work" below sets out.

#### Core's supervisor (the first build)

**Liveness written by the shell.** A generation's `Live` was written by its own transitions and also by the supervisor's shell, which set the last pong, the abandoned workers and the hang check through `live_mut`, and judged the wedge rule on every heartbeat from a copy. `Live`'s fields are now private, `live_mut` and `live` are gone, and four transitions judge the wedge rule and the hang check in one place: `ponged`, `heartbeat(now_us, limits)` (answering `Ping`, `CheckHang`, which pings and starts a check of whether the application is hung, or `Wedged(reason)`), `hang_check_ended` when the check's thread could not start, and `hang_checked`. While the supervisor shuts down, the shell drops `hang_checked`'s reason, as it returned first before. Calls and wake-ups are unchanged; one difference: a heartbeat that ends an outpost for missed pongs no longer also starts a hang check for it, whose answer was dropped.

**Liveness the shell keeps.** The listener's and each spare's record kept `last_pong_at` and were judged by `wedge_decision` in the shell. Moving the records into the pure module was rejected, since they hold child processes; each holds a pure `Liveness` instead, with `ponged` and `wedged`, as an application's record holds its `Generation`.

**The order of starts.** `start_due` collected the applications whose backoff ended from a hash map, so which application got which spare depended on the hasher. A pure `due_starts` in `supervisor/policy.rs` returns them in the order their backoffs ended, and by process id on a tie, and calls `Backoff::ended` itself, so "ended" is judged once; sorting in the shell was rejected so that the choice is made, and tested, in the pure module. The shell's `records`, `apps` and `spare_children` are `BTreeMap`s. What is heard is unchanged but which of two applications whose backoffs end on the same wake starts first, which is now fixed.

#### Core's request table (the second build)

`RequestTable` (`verbatim-app`, `requests.rs` 122, as it was; 502 non-test lines, 3 fields, 4 fields per entry) is the one owner of each query's outcome, on the reducer thread. As built, it is `verbatim-core`'s `requests::RequestTable<A>`, described in [verbatim-core](../crates/verbatim-core.md), "The request table"; line numbers below are the design's, before the build. The query table build then took the reducer's own queries out of it into Core's state ("Core's query table", below), leaving it the shell's five askers, with no passive entries and no count of timed ones.

**Hash order reaching the reducer.** `entries: HashMap<RequestId, Entry>` (124) is iterated by `outpost_ended` (211 to 222) and `expire` (228 to 247), whose reducer inputs are applied in that order (`main.rs` 1982 and 2011).

- Options: (a) a `BTreeMap`; (b) sort what each returns.
- Chosen: (a). The ids are allocated in increasing order (160 to 182), so the order becomes the order the requests were made in.
- What is heard: when one outpost ends with several requests outstanding, or several deadlines pass on one wake, their outcomes are spoken in request order, where today they are in hash order. Nothing else changes.

**Deadlines as `Instant`s** (117, 139, 228, 252; the callers at `main.rs` 1835 and 1927). They become microseconds on the shared clock. The reducer thread reads `now_us()` where it read `Instant::now()` to set a deadline, and when a deadline passes, to expire the table's. Its wait still takes an `Instant`, since its other deadlines stay `Instant`s, so while a request with a deadline is outstanding, which only a list dialog's shell items request has, each pass of its loop reads both clocks once more to turn the table's earliest deadline into a moment on the monotonic clock (`instant_at` in `verbatim-app`). It errs a microsecond late, with the shared clock read first, so the deadline the wait was for has passed when the thread checks (the builder, 2026-11-01, pending Dickson's acceptance).

**Calls inside the owner.** `deliver` (344 to 461) does all of these itself:

- answers or drops a `Responder` (310 to 337);
- sends on a channel (399);
- logs.

The fix: the table becomes generic over its asker, holding it as an opaque token it never calls, as the idle barrier holds its waiters, and returns `(asker, outcome)` pairs. `dump_tree_request` (262 to 271), which finds an entry by its asker's ticket, takes a predicate on the asker instead, as `find`; `withdraw_text`, which finds Core's text request by its query id the same way, takes one too, as `withdraw`, and hands the withdrawn asker back for the caller to drop, so it too leaves the table by being moved out. A `deliver` in `verbatim-app` carries each out at its three call sites (`main.rs` 1570, 1982 and 2011).

- Calls: the same, moved to the caller.
- States: none.
- One answer path: each asker leaves the table exactly once, moved into the pair, and a `Responder` dropped unanswered still answers on drop.
- Placement: the table then names no channel or responder, so it moves to `verbatim-core` as a covered module. It is Verbatim's own code with no NVDA provenance, so it may join the platform-neutral tier.

Before and after:

- Fields: 3 before, 3 after.
- Entry fields: 4 before, 4 after.
- Lifecycle states: none added.
- Lines: `deliver` and its two helpers, about 160 lines, stay in `verbatim-app`. The table's other 340 or so lines move.

#### The keyboard decision machine (the second build)

`DecisionMachine` (`verbatim-input`, `state.rs` 319; 689 non-test lines, 8 fields) runs on the hook thread.

**The gesture map read per key.** `map: Arc<ArcSwap<GestureMap>>` (374) is loaded on every press (534 and 573). Nothing ever stores into it: `verbatim-app` builds it once at start-up (`main.rs` 368 and 2174) and no other writer exists.

- (a) Remove the need: the machine owns a plain `GestureMap`, given at construction. Chosen.
  - Calls: removes one atomic load per key.
  - States: none, and the `SharedGestureMap` type and the crate's `arc-swap` dependency go.
  - A later runtime rebinding would come as a `set_map` input, given by the hook thread when it sees a new map, as `set_num_lock` (421) is given today.
- (b), (c) and (d) would keep the shared snapshot for a value that never changes. Rejected.

**Trace ids.** `TraceId::mint()` runs at 171 (an observed key passed) and 539 (a bound gesture). Options:

- (a) the hook stamps each emitted gesture with a minted trace id after `on_key` returns. Chosen. It mints exactly as often as today, and the hook already mints for every passed key (`hook.rs` 323).
- (c) a trace id on every `KeyEvent`. Rejected: it would mint for keys that emit nothing.

**The clock.** `on_key(event, now: Instant)` (431) is given `Instant::now()` by the hook (`hook.rs` 368), and keeps it for multi-press counting (387) and the lone-modifier double tap (263). The hook reads `now_us()` once at the top of each key instead. It passes that to `on_key` and reuses it for the passed key's `pressed_at_us` (`hook.rs` 322), so a passed key costs one clock read instead of two. The timeouts compare microseconds, so what is heard is unchanged.

As built, the machine raises a `RaisedGesture`, the gesture and its repeat count, and the hook makes the `EmittedGesture` it reports by stamping it (`RaisedGesture::stamped`), so the reports downstream are unchanged. An observed gesture of Verbatim's own injected input, which the hook does not report, is no longer given a trace id.

**Its sets.** `held_modifiers` and `swallowed_downs` (377 and 381) and `GestureMap`'s `bound` and `observed` (`map.rs` 35 and 36) become `BTreeSet`s, with `GestureId` deriving `Ord` in `verbatim-model` (`gesture.rs` 18). So do three in `scripts.rs`'s tests (726, 751 and 758).

- `build_gesture`'s walk of `held_modifiers` (646) is already order-free, since `GestureId::parse` sorts.
- The map's lookup per key becomes a few string comparisons in a set of about a hundred gestures. A scratch measurement, release build, 110 gesture strings and six probes, took 47 to 48 ns a lookup in a `BTreeSet` against 11 to 12 ns in a `HashSet`, so about 36 ns more for each of a press's at most two lookups, against the hook's budget of microseconds; the atomic load per press is gone. Wake-ups and processor time are measured on `measure_keys_notepad`.

**Placement.**

- `verbatim-input` then holds no shared state, clock or mint, and joins the covered crates as the decision machine's crate.
- It is on no reducer's dependency path, so the check lists it by hand beside the reducer crates.
- Its unused `crossbeam-channel` and `tracing` dependencies are removed, so its third-party dependencies are all on the allow-list.

Before and after:

- Fields: 8 before, 8 after (the map's type changes).
- Lifecycle states: none added.
- Dependencies: `arc-swap` removed.

#### The outbound backlog and the writer queue (the second build)

Both are described as pure, and their merging and numbering are. But the `encode` method each implements for its writer thread does more than encode.

`Pending::encode` (`verbatim-outpost`, `outbound.rs` 225 to 245):

- stamps `now_us()` for the latency ledger;
- signals a flush's waiter on a channel;
- returns a terminal read's buffers to the process-wide pool (`terminal.rs` 267).

`Queue::encode` (`verbatim-supervisor`, `writer.rs` 228 and 229) stamps `relayed`.

- Options: (a) leave them as they are; (b) split each, so the pure type returns the message and its writer stamps, signals and recycles.
- (b) cannot be built as a change to these types alone. `encode` is called by the generic transport in `verbatim-process` (`transport.rs` 1093 to 1102 and 1370 to 1414), under the sender's lock, on whichever thread writes. Its contract takes the stamp in `encode`, so that the time stamped is the write's, and reaches a flush marker in encode order. Moving the signal out needs a change to the transport's `Backlog` trait, and would move when `Outbound::flush` returns to after the write completes.
- Chosen: (a). Both `encode`s stay as they are, recorded as timing, a flush's signal and buffer recycling, which no decision reads. The merging and numbering, which decisions do read, stay pure.

**`waiting_nodes`' order.** `reported: HashMap<Carried, Report>` (`outbound.rs` 102) is walked by `waiting_nodes` (183 to 192), whose order reaches the worker's state-change candidates (`worker.rs` 2702 to 2706). `which_of` takes the first whose key matches (`verbatim-ia2`, `acquire.rs` 223 to 227).

- Chosen: a `BTreeMap`, with `Carried` deriving `Ord`.
- What is heard: unchanged, except that when two candidates share a key the choice is now fixed.

#### Floating point from the platform's library (the first build)

`verbatim-model`'s `progress_frequency` called `f64::exp2`, which on Windows goes to the platform's C library, whose results may differ between platforms and versions. Its only caller was `verbatim-speech`'s theme, so it moved there, which removes the need and adds no dependency; calling `libm`'s pure Rust `exp2` was the alternative. The rule forbids the standard library's transcendental functions for `f32` and `f64` ("The enforcement"); the exact operations stay allowed.

### Simpler owners (the third build)

Making the owners pure should also make them simpler (Dickson, 2026-10-31). Any simplification qualifies if it has a stated benefit, for example:

- an enum of states in place of flags or `Option` fields that are valid only in combination;
- an input checked once at the boundary into a type that cannot hold an invalid value;
- an explicit state machine where a lifecycle is spread over flags;
- removing a field derivable from others, or a fact copied from another owner;
- merging inputs or effects that differ only in a flag;
- deleting a mechanism that a type or ownership change makes unnecessary;
- a smaller interface between the owner and its shell;
- a standard structure in place of a hand-written one.

Below are each owner's candidates, with the invalid state, copy or checks each removes, and the counts before and after. Changes that would only move code are left out. The purity changes come first, since they change the same fields, and these follow as a build of their own, so each design can be reviewed alone.

**The outpost's reducer.**

- **Built by "The work in flight, shared by both readers":** the walk as one state (`Unarmed`, `Armed(bound)` or `Published`) and the count of abandoned workers kept by the shell alone (the simplification study's second candidate).
- **The gap timer derived from the gap record (recommended).** It replaces the `GapTimer { Own(Option<due_us>), Given }` enum this section first proposed, which kept a copy of the due time.
  - `Reducer::gap_due` (`src/lib.rs` 470) repeats the open record's `due_us`, and `TimerAction` (`src/intake.rs` 187) threads each change of the record out to it through `Pushed::timer` (390) and `gap_timer` (`src/lib.rs` 767). `next_timer` (553) and `tick` (670) read the record's due time instead, and the record notes that its bound's entry is queued, which `tick` today notes by clearing `gap_due` (671). `next_timer` skips a record whose bound's entry is queued, so that while the entry waits behind a marker the reducer is not woken again for a time already past.
  - Benefit: the copy goes, and with it the invalid "given, with a due time the reducer will never use" (`gap_due` is set at 771 whatever the flag says). `Effect::GapTimer`, the tests' seam, is given when the record's due time differs after a transition from before it. That also gives `GapTimer(None)` when the bound's read closes the record, where today nothing is given; harmless, but the tests that assert exact effect lists change with it.
  - Counts: 1 field (`gap_due`), 1 type of 3 variants (`TimerAction`) and 1 helper fewer, about 70 lines. `gap_timer_given` stays a flag until the seam goes (below), so the flags are unchanged, and its 2 filters stay.
- **One copy of the marks (recommended).** `Intake::marks` (`src/intake.rs` 325) copies `Facts::marks` (`src/follow.rs` 532) within the same owner, kept equal by `sync_marks` (`src/lib.rs` 917, called at 850, 1214 and 1405), `note_focus` and `absorb_console_updates` (`src/intake.rs` 803 and 791), and `Applied::adopted` (`src/follow.rs` 566). The intake takes the facts' marks as an argument where it reads them (529 and 651 to 654).
  - `sync_marks` is also the only giver of `Effect::Marks`, which the shell turns into the mailbox's push-time copy of the marks (`verbatim-outpost`, `shell.rs` 737), against which `drops_on_arrival` judges each arrival. In its place, `Effect::Marks` is given whenever `Facts::marks` differs after a transition from before it, as the gap timer's effect is, so the mailbox's copy follows every change, including a console window's absorption cleared (`src/lib.rs` 1212 to 1214 and 1403 to 1405) and a focus adopted.
  - Benefit: one holder of the marks within the owner, and fewer lines. No state is removed that is reachable today, since every write to the facts' marks is followed by `sync_marks` in the same transition (849, 1214 and 1405).
  - Counts: the intake's 1 field, its 2 setters, `sync_marks`, `Applied::adopted` and the trigger at 844 to 851 go, and one comparison after each transition is added, about 40 lines net.
  - Cost: each transition copies the marks to compare them after, one small allocation while the focus is a UIA element (its runtime id), kept as the simpler code (Dickson, 2026-11-01); measured with debug builds in test audio mode, `measure_keys_notepad`, three 12-second windows after and four before (Core's query table build), 2026-11-01: the outpost's processor time per input-hook wake 1.39 ms after and 1.39 ms before (median), and wake-ups in all processes per hook wake 31.7 after and 31.8 before, so no change was measured.
- **Not taken: the focus group's remaining tries kept in the intake.** `finished` (`src/lib.rs` 1122 to 1142) does not only reorder two queues: it gives the group's remaining entries to a gap read pushed during the try, so that the read, if it reports the focus, ends the group and its older entries are never tried. That decision stays wherever the queue is kept, so moving `Reducer::after` into the intake's batch would save only the field, and is not worth a commit.
- **A window that is never 0 (recommended for the logic crates).** Windows parsed at the boundary into `Option<Window>`, with `Window` a non-zero handle. It deletes the 9 "0 is none" comparisons in `verbatim-outpost-logic`, the 3 in `verbatim-listener-logic` and the 1 in `verbatim-reader-rules`, and makes a window of 0 unrepresentable. The protocol keeps `isize`, and the logic crates' entry points convert it.
  - Scope: every window value in the three logic crates where 0 means none becomes `Option<Window>`, parsed where it enters them, at their inputs from the shells and in the results of `WindowQueries`, which the Windows crate implementing it parses; `Item`, the protocol, and the gap record's and verdicts' keys stay `isize` (Fable, 2026-11-01, pending Dickson's acceptance). It removes every "0 is none" judgment the design names without spreading a type through values that are never none.
- **Not this build:** `gap_timer_given`, `Input::GapTimerGiven` and `Input::GapBound` are the mockapp tests' seam for driving the gap bound by hand. They could go once those tests can drive the reducer's time, which is a follow-up.
- **Kept as they are:**
  - `to_drop`: the reducer's knowledge that a release left objects to drop, which decides when a `Work::Drop` program runs. The alternative, attaching the shell's garbage to the next program, would hold released objects until the next event when the application is idle.
  - `releasing` and `stopping`: each changes by its own input, and every combination is valid.
  - `deferred`, `running` and `group`: independent of each other.

**The listener's scheduler.**

- **Built by "The work in flight, shared by both readers":** the running read's place is kept in the read's item, so a stale place with no read cannot exist, and `App` has 8 fields.
- **Kept as they are:**
  - `threaded`, `limit_reached` and `ending` are changed by different inputs, and their combinations are valid: an application at its limit with no thread, or an ending one with a thread still answering.
  - `GraceState` is already small and explicit.

**`verbatim-reader-rules`.** Its functions are pure and hold no state. The only candidate is the window type above.

**The supervisor's policy and generations.** `Generation` is already an explicit state machine (`Launching`, `SpareGiven`, `Starting`, `Ready`, `Waiting`). The second writer is removed by the purity build, and `Live`'s fields become private behind three transitions. No further type change is recommended: `Backoff`, `Spares` and `HeldFacts` hold no flag pairs.

**Core's request table.** Moved out, so nothing is built twice: how an entry ends (`passive` and `deadline` as one `Ends`, with one constructor) and the count of timed entries go to "Core's query table", which replaces the table's entries with the reducer's own record of each query (the simplification study's third candidate). The second build's purity changes to the table stay, and are that build's first step.

**The keyboard decision machine.**

- **The press streak's count with its gesture (not recommended).** `repeat_count` (`state.rs` 391) could join `last_gesture`, but the auto-repeat path returns the count after the streak's gesture was cleared (619 against 519, 524 and 549). Folding them would change what a held key's repeats count, and so what is heard.
- **Moved out, so nothing is built twice:** Num Lock as part of the key (`num_lock_on` and `set_num_lock` replaced by a field of each key's input) is built in "The hook's hold decision", which changes `on_key`'s input and its 109 test calls anyway (Dickson, 2026-11-01).
- **No other candidate.** Its other fields are independent, apart from the map, whose shared type the purity build removes.

**Core's reducer.**

- Its state has 27 fields. A read of the field list and of the pairs that looked related found none valid only in combination.
- The pairs that look related but are not:
  - `pending_caret` and `superseded_caret` are set and cleared by different inputs and may both be held;
  - `start_marker` and `start_marker_generation`, since the generation counts every marker ever set;
  - `pending_text` and `say_all`.
- No change is recommended. A deeper study of `reduce.rs`'s local lifecycles is left to M4.6, whose model of Core's reducer will show any invalid state its types allow.

Counts before and after, for the owners that change:

- **The outpost's reducer**, against its state after the work in flight, in this build alone:
  - fields: 13 to 12 (`gap_due`);
  - flags: 4, unchanged;
  - inputs and effects: 16 each, unchanged;
  - the intake's fields: one fewer (`marks`);
  - removed: `TimerAction`, `Pushed::timer`, `sync_marks`, the intake's 2 mark setters and `Applied::adopted`, about 110 lines net, plus 13 "0 is none" comparisons across the logic crates.
- **The request table and the keyboard decision machine:** no change in this build; their items moved above.
- **The rest:** no change.

### Clocks and trace ids

Core's supervisor's policy, generations and `Received` take microseconds on the run's shared clock, where they took `Instant` values, and `HeldFacts::hold` takes the time as a value, which the supervisor reads at each hold, where it passed `Instant::now` to be read inside the call. Differences use `saturating_sub`, since a plain subtraction panics in the debug builds the end-to-end suite runs. The shell turns a due time into its wait; `now_us` reads the same performance counter as `Instant` from an offset fixed for the run, so no decision changes.

Every production mint on the path has a source: a withdrawn query's "not started" reply carries the trace its query came with; the gap record keeps the trace of its newest copy or foreground change beside its stamp, and its bound's entry carries it; a copy read at once carries the trace of the copy's event, which `GapNote::CopyDropped` and `GapNote::Accepted` now carry; and `verbatim-model`'s samples take fixed ids. With no gap record open, no bound's entry is queued, since it found nothing to read. `TraceId::from_raw` gives fixed ids for tests, and the 319 test sites that minted take it; the mint itself stays for the processes that observe events.

The remaining shared state stays, since no answer from it reaches a result: `verbatim-text`'s `OnceLock` holds the Chinese word segmenter, built from embedded data, and `verbatim-listener-logic` logs through `tracing`.

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

Clippy enforces the rule, through `disallowed-types` and `disallowed-methods` in a `clippy.toml` in each covered crate's directory, 19 types and 103 methods, each with its reason: the hash collections and `RandomState`; `Instant` and `SystemTime` and their reads; the file, process, network and thread types; the thread functions, the environment's reads and writes, every free function of `std::fs`, the process id, exit and abort; `TraceId::mint` and `TraceId::namespace`; and the 26 transcendental functions of each of `f32` and `f64`. A crate's own `clippy.toml` applies to that crate alone, the two lints are in the `style` group, which `-D warnings` denies, a disallowed type is reported wherever it is named, and a path that does not resolve in a crate is ignored, so one file serves all seven. Every entry was shown to fire by a probe naming each once in `verbatim-core`.

The ownership scan's determinism rule (`determinism` in `xtask/src/ownership.rs`) guards Clippy's configuration and what Clippy cannot see, over the crates the tier check derives:

- **One configuration.** Each covered crate's `clippy.toml` is identical to `verbatim-core`'s, and that file names every entry the scan lists. Reason: Clippy has no include, so seven copies are kept equal by a check.
- **No silencing.** No `allow` or `expect` names `warnings`, `clippy::all`, `clippy::style`, `clippy::disallowed_types` or `clippy::disallowed_methods`; each covered crate's `[lints]` table is `workspace = true` alone, with no lint table of its own; the workspace's lint tables name none of those groups or lints; and `.cargo/config.toml` passes no lint flag.
- **Its listed exceptions,** which may `expect` the two lints, with a reason, and no other file may: `verbatim-text`'s and `verbatim-core`'s `tests/terminal_cost.rs`, ignored release measurements that time with `Instant`; `verbatim-core`'s `tests/replay_fixture.rs`, which reads its fixture file; `verbatim-model`'s test of `TraceId::mint` itself; and `verbatim-term-sim`'s `src/conformance.rs` and `tests/conformance.rs`, which read the conformance corpus.
- **No whole module.** The covered crates' code names no path in `std::fs`, `std::net`, `std::process`, `std::env` or `std::thread`, and none in `std::time` but `std::time::Duration`, in a `use` tree or written out, the listed exceptions but the mint's test excepted. Reason: Clippy names functions one by one, so a function added to the standard library later would pass it, and the scan closes that by module.
- **Forbidden macros and calls:** `enabled!`, `event_enabled!`, `span_enabled!`, `proptest!` and `Config::default` ("Third-party crates"); the oracle's random-stream test runs a `TestRunner` built from `random_config()` where it used the macro.
- **Narrower allow-lists.** The pure rule's allow-lists for the outpost's reducer and the read scheduler narrow `std::collections` to `BTreeMap`, `BTreeSet` and `VecDeque`.

The tier check holds each covered crate's direct third-party dependencies, by normal, build and development edges, to its allow-list (`DETERMINISTIC_THIRD_PARTY`), whose comment states the rule for adding a crate. Each rule of the scan and the allow-list was shown to fail `cargo xtask ci` when broken on the real tree.

### Performance on the event path

The maps on the event path that became `BTreeMap`s, the outpost's verdicts, the gap record's answers, the batch plan's two, the text facts and the scheduler's applications, hold tens or hundreds of integer keys, so a lookup is a few comparisons in one or two nodes against SipHash of an integer before. No thread, timer or message is added; the new asks add one reducer transition each, only for slow programs. Wake-ups and processor time, debug builds, test audio mode, one 12-second trace window each, 2026-11-01: `measure_keys_conhost`, all processes, 11,049 wakes and 1,645 ms before, 11,240 to 11,372 and 1,683 to 1,726 ms after; `measure_focus_text_box`, median of four or five windows, 5,861 wakes and 667 ms before (range 4,854 to 7,328), 6,151 and 692 ms after (range 5,736 to 6,451, one window of 14,723 left out), the outpost's worker 1,635 before and 1,732 after. Both are within the spread of the windows before the build.

The terminal alignment of a lost read, by `verbatim-text`'s `terminal_cost` (`cargo test --release -p verbatim-text --test terminal_cost -- --ignored --nocapture`), release build, 120 by 30, medians of six runs each interleaved before and after, 2026-11-01 (`docs/performance.md`, "A terminal's rotation"):

- Before, with the fixed-hasher maps: a rotation of 1,000 rows over 9,001 rows, a lost read, 1.23 to 1.26 ms; an overflow keeping nothing 1.06 to 1.07 ms.
- In `BTreeMap`s and `BTreeSet`s: the lost reads 12 to 26% more, the overflow 30 to 31% more, a clear and reprint 26 to 28% more, the bimodal wide lines 36 to 62%.
- As sorted vectors, as built: the lost reads 1 to 9% more, the overflow and the clear 1 to 3%, the wide lines about 9% at their least.
- Over a console buffer enlarged to 32,766 rows, a case the build adds: 4.1 ms before, 4.9 ms in `BTree` collections (20% more), 4.2 ms as built (2 to 3% more). It is past the reducer's 2 ms budget for a read before and after the build, as predicted.
- A screen of 29 Chinese rows each rewritten, every row paired with its old one by likeness, a case the build adds for `Likeness::other`'s lookups: 1.7 ms before, 1.0 ms after, its map of characters now a `BTreeMap`.

The oracle tests of `verbatim-term-sim` took 26.4 to 28.4 s before and 26.4 to 26.5 s after, in debug builds.

### End-to-end runs and the order of work

The work is three builds, in this order. Each build gets its own design review and implementation review. Reasons for the split:

- The first touches the logic crates and the code beside them.
- The second touches `verbatim-app`, `verbatim-input`, the supervisor's writer and the outpost's outbound queue.
- The third changes types in code the first two have just made pure, so each design stays reviewable on its own.

Each commit has its unit tests, and the crate guides are updated where an API changes. Each scenario runs three times at the end of its build, and none of them is in `docs/flaky-tests.md`.

**The first build, determinism of the logic crates,** built as four commits: the maps and sets as ordered collections; times and trace ids as inputs with the supervisor's liveness under one writer; the outpost's local reads left the reducer; and the enforcement. Its scenarios: `conhost_full_history_flood`, for the terminal's alignment; `notepad_editing`, for the outpost's verdicts, gap record, batch admission, text facts and UIA entries, the listener's scheduler, and an outpost started from a spare with facts held while it starts; and `rapid_tabbing_in_settings`, for the listener's selections. No scenario reaches a moved-on abandonment or a walk's bound, so those are held by the reducer's tests and the shell's test.

**The second build, the other pure owners:**

- **Core's request table.**
  - Its entries go in a `BTreeMap`, and deadlines move to the shared clock.
  - The table becomes generic over its asker and moves to `verbatim-core`, and `deliver` carries out its pairs in `verbatim-app`.
  - Scenarios: `notepad_editing`, whose text reads pass through the table, and `explorer_folder_window`, whose shell-item requests take the responder path, but only if `docs/flaky-tests.md` no longer lists it then; otherwise its unit tests alone.
- **The keyboard decision machine.**
  - The gesture map is owned, the trace id is stamped by the hook, the time is in shared-clock microseconds, and its sets are `BTreeSet`s.
  - `verbatim-input` joins the covered crates, with its `clippy.toml`.
  - Scenarios: `notepad_typed_words` and `measure_keys_notepad`'s wake-ups and processor time, before and after.
  - Measured, debug builds, test audio mode, five 12-second windows each side interleaved, 2026-11-01: wakes in all processes per input-hook wake, median 31.1 after and 32.2 before; processor time 2,937 ms after and 2,903 ms before (one before window, which caught half the typing, left out). No change was measured.
- **The outbound backlog's `reported` as a `BTreeMap`.** Both `encode`s stay as they are. Scenario: `notepad_editing`.

**The third build, simpler types:**

- One commit per owner, as recommended in "Simpler owners": the outpost's gap timer from its record and one copy of the marks, and the window type across the logic crates.
- Scenarios: `notepad_editing` and `rapid_tabbing_in_settings`.

### Limitations

- Third-party code is checked by reading it when it is added, and again only when a version change is reviewed. M4.6's run of each case twice is the backstop.
- A published walk's content follows how far the worker got by its bound. That is the system's timing, and it reaches the reducer as `Published`'s input, so a replay is exact.
- A lost read over a buffer enlarged to 32,766 rows takes 4.2 ms, past the reducer's 2 ms budget, as before the build (Dickson, 2026-10-31).
- The hung-window test is a call the worker makes on a live window, so no unit test holds it; the scenarios with UIA entries hold its passing path, and none makes a window hung.
- The shell's test shows an ask answered within the one step of the shell's; that the answer is given once is held by the shape of `Shell::carry_out`, which returns one input per asking effect.
- A `tracing` event's field expressions run only when the event is enabled, so a field with a side effect would make the logic depend on the subscriber. That is a review rule.
- The idle barrier (`verbatim-app`, `idle.rs`) and the GUI's lifecycle stay where they are. The audit found them pure, and they decide no reducer input that a replay needs.
- The verdicts on `serde_json` and `vte` rest on features nothing enables today (`preserve_order` and `ansi`). The tier check holds crates, not features, and `workspace-hack` could unify either feature in from any workspace crate. Neither would make a covered crate's result depend on anything but its input: a JSON map would keep insertion order, and `ansi` adds code the covered crates do not call.
- A terminal row is compared by its 64-bit hash alone, so two texts with the same hash align as the same row. The hash is a fixed public function of the application's text, so an application could make that happen on purpose. This predates the build and is not changed by it (a follow-up).
- The ci's list of fixed sleeps in production code, for review, now names two of the scan's entries, `std::thread::sleep` and `std::thread::park_timeout`, since that list reads string literals.

### Decisions

- `BTreeMap` and `BTreeSet` everywhere on the path, and a fixed-hasher map only where a measurement in the build shows a cost that matters, named here with its figure (Dickson, 2026-10-31). None is taken.
- The audit covers the third-party crates the covered crates call, from their source, and a crate added to the allow-list later gets the same check (Dickson, 2026-10-31).
- The outpost's three local reads leave the reducer, so that it is pure with calls as its effects (Dickson, 2026-10-31).
- `verbatim-speech` stays outside the rule (Dickson, 2026-10-31).
- A 32,766-row lost read past the 2 ms budget is recorded as a limit (Dickson, 2026-10-31).
- Making the owners pure should make them simpler where it can, with types that rule out invalid states (Dickson, 2026-10-31).
- The UIA hung-window test runs where each entry is handled, on the entry's own cached window (0 passing), first in the worker's UIA event and focus handling, covering the listener's focus reads too. It drops what the batch plan's test dropped, and also a focus read whose window became hung while it waited, which could be spoken from a half-read focus at its walk's bound (Dickson, 2026-10-31).
- The terminal alignment's lookups by row hash are kept in `BTree` collections if that costs less than 30% against the fixed-hasher maps at 9,001 and 32,766 rows, with no fixed-hasher exception; at 30% or more the figures go to Dickson (Dickson, 2026-11-01). In `BTree` collections the overflow case cost 30 to 31% and the bimodal wide lines more, so the design's fallback, sorted vectors with no hasher, was built, at 1 to 9% (the builder, 2026-11-01, pending Dickson's acceptance).

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

### Decisions

- An outpost whose worker cannot start, at its start or as a replacement, keeps running: the generation stays without a worker, each program is answered as failed, each dispatch tries once to start one, and the shell keeps its garbage until a worker starts (Dickson, 2026-10-31).
- The fault tests are built by `cargo xtask faults` in `target/debug`, beside the shipped build; the only executable it rewrites is `mockapp.exe`, which the suite's own build relinks (Dickson, 2026-10-31).
- The lock check panics, and in debug builds aborts where a panic around application calls would be caught (the worker, UIA's event callbacks and the bounded client's thread), after writing its message to standard error and the log (Dickson, 2026-10-31).

## The hook's hold decision (a build of its own)

Status: built on 2026-11-01, from the fifth candidate of the simplification study (`experiments/logic-tests-study/radical-simplification-study.md`), after the determinism build's second build, which made the keyboard decision machine pure. "The decision today" describes the code before this build, at main 39dce0b2, and its lines are that commit's; the mechanism as built is in the crate guides of [verbatim-input](../crates/verbatim-input.md) and [verbatim-input-windows](../crates/verbatim-input-windows.md). A held key, a deferred key and the line of keys are as in [holding caret keys](key-holding.md) (its terms and "Keys behind a held key").

### The decision today

The machine (`verbatim-input`, `src/state.rs`) decides pass or swallow. The hook (`verbatim-input-windows`, `src/hook.rs`) then rewrites that decision in `hold` (275 to 310):

- a release whose press was held is swallowed through the hook's own set `held_downs` (47 and 277), beside the machine's `swallowed_downs` (`state.rs` 381), which swallows the release of a bound gesture's press or the Verbatim modifier's;
- a press the machine passed with an observed gesture becomes a swallow when `KeyHolding::holds` (`lib.rs` 134) finds the switch on and the gesture in the hook's own copy of the caret table (`lib.rs` 114), and `may_hold` (`lib.rs` 174) finds Core's view about the window in front and allowing it;
- any other transition the machine passed becomes a swallow, deferred, while the line `outstanding` (51) is not empty;
- Verbatim's own input and the lock keys are exempt (282).

The line's places are the hook's `sequence` (53), given out by `queue` (107) and taken back by `passed_on` (120) when a key Verbatim sent again comes back with its place, before the machine sees it (359). `decide` (348) then reports from both results: `Held` or `Observed` by the rewrite, `Passed` by `may_edit` (188), whose `is_modifier` (176) repeats the machine's modifier table (`state.rs` 669) and whose lock-key test repeats the machine's, and a lock key's `Gesture` (440).

So a key's fate is decided in two places, the second able to contradict the first, and two sets record "swallow this key's release".

### The model

The machine decides each transition whole; the hook gathers its inputs, carries out the verdict and reports.

- **Input.** `on_key` takes a `KeyInput`: the event, the time and Num Lock (the third build's "Num Lock as part of the key", built here; "Decisions" below), and two more:
  - `source`: `User`, or `Own` (tagged `OWN_INPUT_TAG`), decoded by the hook from `dwExtraInfo`;
  - `permit`: `None`, `Defer` or `HoldAndDefer`. It is `None` unless Core's view names the window in front and lets keys be held there, and `HoldAndDefer` when the focus also has text. One enum, so "the focus has text in a window that may not hold" cannot be given.
- **Verdict.** `Pass`, `Swallow`, `Hold(place)` or `Defer(place)`. The hook calls `CallNextHookEx` for `Pass` and returns 1 for the rest, reporting `Held` with its stroke for `Hold` and `Deferred` for `Defer`, as it does today.
- **What the key reports**, returned with the verdict as today's `Decision` fields: the speech effect, the bound gesture, the observed gesture, whether a press that reaches the application is translated for typed text and, typing nothing, reported as `Passed` (`may_edit` and its tables move into the machine), and the lock key whose new state is announced. For `Own` the machine returns no speech effect, observed gesture or `Passed`, as the hook drops them today. The hook still translates, since translation calls the keyboard layout (`src/typed.rs`), and reads the deferred transitions it needs from the machine.
- **The line moves into the machine:** `outstanding` and its places, unchanged in behaviour. A key Verbatim sent again (tagged `PASSED_ON_TAG`) is decoded first, as today (`hook.rs` 354 to 362), and given to the machine's `passed_on(place)`, which takes the line up to that place and changes nothing else; the hook passes it before reading Num Lock, the time, the view or the window in front, so a passed-on key costs no read, as today.
- **One record of the keys down.** `held_downs` and `swallowed_downs` become one `BTreeMap` from each key the machine has seen go down and not come up to two facts:
  - `reached`: a press of the key has reached the application, or is deferred to reach it, since it last went down or was last held. Set by a `Pass` or `Defer` verdict on a press, and cleared by a `Hold`, since the shell sends the held press with its own release.
  - `swallowed_gesture`: a press of the key completed a bound gesture, or was the Verbatim modifier's, and was swallowed, since it last went down. Set by such a `Swallow`, and never cleared by a hold or a pass, so a bound gesture's swallow always wins over a hold, as in `swallowed_downs` today.
  - A release is swallowed when its key's entry has `reached` false, and otherwise passed, or deferred behind the line; a key with no entry passes, as today. A release whose key's press waits in the line is deferred whatever the permit, so the line's entries keep each deferred transition's key with its extended flag. The entry goes with the release.
  - The multi-press count treats a press as an auto-repeat when `swallowed_gesture` is set, which is exactly when `swallowed_downs` holds the key today (`state.rs` 617 to 620). So Down Arrow held in Notepad (held, no flag) whose auto-repeat completes `kb:downArrow+verbatim` once the Verbatim modifier goes down counts as a fresh press, repeat 0, and every later auto-repeat carries 0, as today; and Verbatim+Down Arrow pressed, the modifier let go and Down's next auto-repeat held, then the modifier pressed again, still carries 0, as today.
  - All four combinations of the two facts are reachable and valid.
- **The stuck key fixed (Dickson, 2026-11-01; "Decisions" below).** `reached` is what fixes it. Today a key whose press was held, or swallowed for a bound gesture, keeps its entry while its later auto-repeats pass, so its release is swallowed and Windows keeps the key down until it is pressed again: Down Arrow held in a search box whose results list takes the focus, where holding switches off after the first press (reproduced in the design review, `experiments/logic-tests-study/hook-hold-design-review.md`), or Verbatim+Down Arrow with the modifier let go before Down. With `reached` set by the first auto-repeat that passes or is deferred, the release takes the same path as that press.
- **The caret table moves into the gesture map**, as the observed gestures that are held; the clearing keys stay observed and not held. `KeyHolding`'s set goes.
- **What stays in the hook:** the reads (Num Lock, the time, Core's view, the window in front), decoding `dwExtraInfo`, the calls (`CallNextHookEx`, the speech effect, the reports, translation), minting trace ids, and the harness's `Handled`.

The hook reads the window in front only when Core's view lets keys be held and either the focus has text or the machine's line is not empty (`has_outstanding`, a read of the machine's own state on the same thread); otherwise the permit is `None` with no call. One read serves both the permit and translation, which today reads it for every press that may type.

### Sequences

**A held caret key.** Notepad's text has the focus; Right Arrow is pressed.

1. The hook decodes `User`, reads Num Lock and the time, loads the view (Notepad's window, keys may be held, the focus has text), and reads the window in front, which matches: `HoldAndDefer`.
2. `on_key`: `kb:rightArrow` is observed and held. The machine records the key as down with neither fact set, appends place n with no transition, and returns `Hold(n)` with the observed gesture and `Cancel`.
3. The hook cancels speech, reports `Held` with the stroke and place n, and returns 1.
4. Core reads the caret, and the shell sends the press and its release tagged with n. Each comes back to the hook, which decodes its place before any read and gives it to `passed_on(n)`; the first takes the line up to n, the second finds nothing left, and both pass.

**A key released while held.** Right Arrow released before Core passes it on: the machine finds the key's entry with `reached` false, removes it and returns `Swallow`, adding no place, since the shell sends the release with the press. Released after the pass-on: the same. A letter pressed and released in between returns `Defer` twice, each with its own place, reported after what the press typed, as today.

**A gesture.** Verbatim+T pressed while Right Arrow waits in the line: the bound gesture returns `Swallow`, never `Defer`, since only a transition that would pass is deferred; Core runs it after the held key, as today. The Verbatim modifier is swallowed as today; in share mode it passes and, unless it is a lock key, is deferred behind a held key, as today.

**A key repeat.** Right Arrow held down: each auto-repeated press is held in its own turn, `Hold(n + 1)`, `Hold(n + 2)` and on, `reached` staying false, and the one release is swallowed, as today ("A key held with auto-repeat" in the holding design). A bound gesture's auto-repeat keeps its multi-press count.

**Holding switched off while a held key is down.** Down Arrow is held in a search box; its first press is `Hold(1)`, and once it is passed on the focus moves to the results list and Core switches holding off. The first auto-repeat, with the line empty and the permit `None`, is `Pass`, setting `reached`; so do the later ones. The release finds `reached` and passes, so the list and Windows see the key come up. With the permit `Defer` and the line not empty, the auto-repeat is `Defer` and sets `reached`, and the release is deferred behind it.

**The hook's timeout.** Windows removes a low-level hook without telling it once its procedure takes longer than `LowLevelHooksTimeout` (300 ms by default; `lib.rs` 15 to 30). The procedure does today's work, in a different place: at most two local reads (Num Lock, and the window in front where the permit needs it), one lock-free load, the machine's transition (a few operations on sets of a few keys and one deque push or pop) and the posts. As a covered crate the machine can name no lock, thread, clock or call, which the determinism enforcement checks, where today only the hook's own module is scanned (`xtask/src/ownership.rs`). Were the hook removed anyway, the line would stay as it was, as today.

**Core not answering.** A held key's fate stays Core's: its read is bounded by Core's deadline (`HELD_KEY_BOUND`, 150 ms) and it is sent by the shell. If the reducer thread stalls, the line stays non-empty and every later transition that would pass is deferred, waiting in the reducer's queue, as today (the holding design's "Limitations"). The machine adds no bound of its own: a second owner of when a held key is let go would be a copy of Core's decision. A key Windows drops after Core sent it, to an elevated window come to the front, keeps its place until a later key comes back and takes the line up to its own place, as today; while that window is in front the permit is `None`, so nothing is deferred.

### States and lines

- Removed from the hook: `held_downs`, `outstanding`, `sequence`, `Outstanding`, `Hold`, `hold`, `queue`, `passed_on`, `deferred_transitions`, `is_modifier` and `may_edit`, and `KeyHolding` with its `AtomicBool` and set, its switch joining `HookForeground`. The rewrite of a decided key goes.
- Moved into the machine, not added: the line and its places.
- Added: `KeyInput`'s `source` and `permit`, `passed_on`, the verdict's `Hold` and `Defer`, the two facts of each key down, and the gesture map's held set. No lifecycle state is added; two swallow sets become one record.
- Counts: the hook's state from 11 fields to 7; the machine's from 8 (after the determinism build's second build) to 9, Num Lock leaving and the line and its last place joining. About 150 non-test lines leave `verbatim-input-windows` and about 100 join `verbatim-input`.

### Hook latency and wake-ups

- Wake-ups: none added or removed. The reports and posts are the same, in the same order.
- The procedure's work: the same, except that the window in front is read for every transition while the focus has text in a window that may hold keys, where today only a held or deferred key and a press that may type, modifiers included (`typed.rs` 150), read it. Releases, presses while Control or Alt is down, and Verbatim's gestures in a text field gain one local call each, which does not wait or allocate. A passed-on key reads nothing, as today. With the two views as one, one lock-free load replaces a load and an atomic read per key.
- Measured before and after, in the build: `cargo xtask trace scenario` on `measure_keys_notepad` and `measure_held_repeat_text_box`, debug builds, test audio mode, at most 20 runs each: the hook thread's wake-ups and processor time, and wake-ups in all processes per input-hook wake. A rise in the hook thread's processor time beyond the runs' spread is a finding.

### What is heard, and which keys reach the application

Nothing heard changes. Every verdict equals today's rewritten decision for every source, permit and key, which the tests below check, with three exceptions, each a key that today is left down in Windows:

- the release of a key one of whose presses reached the application, after an earlier press of it was held or swallowed for a gesture, now reaches the application where today it is swallowed (the stuck key, above);
- the same for the other order: a press of the key passed, then the Verbatim modifier went down and the key's auto-repeat completed a Verbatim gesture and was swallowed; its release is now delivered, where today `swallowed_downs` swallows it, consistent with the stuck-key decision;
- a release whose press was deferred and still waits in the line, after the permit was lost (a window Core has no view of, or an elevated one, came to the front), is now deferred behind its press, where today it passes at once and the press is sent after it. Shortest case: Right Arrow held, A pressed and deferred, the permit lost, A released.

NVDA, from the pinned source:

- NVDA keeps one set of trapped keys, for every press it swallowed, a script's gesture or the NVDA modifier, and swallows a release whose key is in it (`nvda/source/keyboardHandler.py` 93, 314, 319 and 411 to 413); one record of the keys down is the same shape. NVDA has the stuck key too: a key goes into the set when its script runs (314), a later press of it that runs no script passes without taking it out, and its release is swallowed (411 to 413). The fix departs from NVDA, as Dickson decided ("Decisions" below).
- Its caret scripts read the caret, then send the key (`nvda/source/editableText.py` 181 to 188), and the keys it sends pass its own hook untouched (`nvda/source/keyboardHandler.py` 226, with `ignoreInjection` around `send`'s injection, 796 to 811), as Verbatim's passed-on keys bypass the machine's decision.
- Deferring later keys behind a held key is Verbatim's own (the holding design's "Decisions") and is unchanged.

### Tests

- The hook's holding tests (`src/hook.rs` 609, 672 and 821, and the decision half of 887) and `a_key_with_a_meaning_outside_the_text_is_not_watched` (920), which tests `may_edit`, become tests of the machine with a source and a permit, with their assertions unchanged. `no_key_is_held_where_core_says_none_may_be` (737) and `no_key_is_held_until_core_has_a_view_of_the_window_in_front` (777) stay in the hook as tests of the permit's gathering, and one new hook test checks that each verdict is carried out: the return value and the reports.
- New machine tests, one per behaviour: a held key's release is swallowed after its pass-on; a held key's auto-repeat that completes a bound gesture counts as a fresh press, and its later auto-repeats carry the same count; a held key's auto-repeat passed after holding switches off gets its release passed; a gesture's key whose auto-repeat passes after the modifier is let go gets its release passed; `passed_on` changes only the line; `Own` input is neither held nor deferred and reports nothing passed.
- An enumerated test, as the design principles ask of a pure state machine with lifecycle paths. Its alphabet, 11 steps: Right Arrow's press (an auto-repeat when it is already down) and release, A's press and release, Shift's press and release, the Verbatim modifier's (Insert's) press and release, T's press, the shell's pass-on of the oldest place, and the permit moved to its next value. Verbatim+Right Arrow and Verbatim+T are bound, so a caret key is also a gesture's key. Each step is 33 ms after the one before, so multi-press timing is exercised. Every sequence of up to five steps is walked depth first, each prefix's state cloned rather than replayed, about 180,000 transitions; the build measures its run time in a debug build and keeps it under ten seconds, at four steps if five are too slow.
  - Its reference model: the order of transitions the application should receive (the user's, less the swallowed ones, with each held or deferred one at its place), and the repeat count each emitted gesture should carry (an auto-repeat once a gesture was swallowed since the key went down, a fresh press otherwise).
  - After every step it checks: the passed and sent-again transitions match the model; no release reaches the application before its press; at the end of each sequence, with every place passed on and every key let go, every press that reached the application is followed by its release; the emitted repeat counts match the model; and each place enters and leaves the line once.
  - The test is written first against today's machine and hook, through `decide`, where it must find the stuck key and nothing else, and then against the new machine, where it must pass. Against today's code it found, five steps deep, 622 sequences of the stuck key in both orders and 6 of a release passed ahead of its deferred press, the third exception above; as built it walks 18,936 sequences in 0.2 s in a debug build, with no failure, and also checks the three exceptions' shortest sequences by their verdicts.

### Scenarios and the order of work

Scenarios, each run three times at the end of the build, none recorded in `docs/flaky-tests.md`:

- `conhost_up_typing`: a held Up Arrow with a typed character deferred behind it;
- `notepad_selection_keys`: Shift+Right and Right in one batch, Shift's release deferred behind the held Shift+Right and Right held in its turn;
- `notepad_typed_words`: typed text translated with the deferred transitions, and `Passed` reports.

The order of work: measure before; the enumerated test against today's code, showing the stuck key; the machine's input, verdict, line, record of keys down and held set with the hook carrying them out and the tests, one commit; the two views as one, one commit; `cargo xtask ci`, the scenarios, and measure after; documents: the crate guides of `verbatim-input` and `verbatim-input-windows`, and the ledger's "A key through the keyboard hook" in `docs/performance.md`. About two days.

### Limitations

- The permit is gathered before the machine decides, so the window in front is read for keys that end neither held nor deferred.

### Decisions

- The stuck key is fixed in this build, departing from NVDA: a key some of whose presses reached the application has its release delivered, so the application and Windows see it come up, where NVDA swallows the release and Windows reports the key down until it is next pressed and released (Dickson, 2026-11-01).
- Core's two views become one: `KeyHolding`'s switch, stored after every input (`verbatim-app`, `main.rs` 1599) beside `ForegroundView`'s store (1600), becomes a field of `HookForeground`, so Core publishes one view and the hook makes one load per key, at the cost of a store, with its allocation, on the reducer thread when the focus moves between text and no text (Dickson, 2026-11-01).
- The window in front is read by the wider gate above, for every transition while the focus has text in a window that may hold keys or a key waits in the line, rather than a narrower gate that would copy into the hook the machine's rule that a release is never held; if the before-and-after measurement shows a rise in the hook thread's processor time, the gate is reconsidered (Dickson, 2026-11-01).
- Num Lock as part of the key is built in this build, since it changes `on_key`'s input and its 109 test calls as this build does, and leaves "Simpler owners" (Dickson, 2026-11-01).
- A `Hold` verdict clears `reached` (Fable, 2026-11-01, pending Dickson's acceptance): the shell sends the held press with its own release, so a press passed before the hold is already matched, and delivering the user's release too would reach the application with no press before it.
- A release whose key's press still waits in the line is deferred whatever the permit, the line's entries keeping the key and its extended flag (Fable, 2026-11-01, pending Dickson's acceptance): a release passed ahead of its deferred press leaves the key down in Windows once the press is sent.

## Core's query table (a build of its own)

Status: built on 2026-11-01 on branch `query-table`, from main e0406881; the line numbers below are main's at 39dce0b2, where it was written, and the request table it starts from is the second determinism build's, `verbatim-core`'s `requests::RequestTable<A>`. The mechanism is described in [verbatim-core](../crates/verbatim-core.md), "Core's queries", and [verbatim-app](../crates/verbatim-app.md) for the shell. It was proposed the same day, revised after its adversarial review (`experiments/logic-tests-study/query-table-design-review.md`) and for Dickson's decisions of the same day (below). It is candidate three of the simplification study (`experiments/logic-tests-study/radical-simplification-study.md`, "Third: Core's outstanding queries in one table owned by Core's reducer"). It starts after the caret watch build and after the determinism build's request table commit, which puts the table's entries in a `BTreeMap`, moves its deadlines to the shared clock and makes it generic over its asker in `verbatim-core`; this build then takes Core's own queries out of that table. A query here is a request Core's reducer makes for a node's text, a navigation or an activation, whether an outpost answers it or Core answers it from a terminal's memory.

Where the build differs from the text below, pending Dickson's acceptance:

- `Input::OutpostEnded` carries a trace the shell mints, which a query asked under an input with no trace ends under at its outpost's end ("Traces" names the ending input's trace, and the input had none).
- `Queries::ask` returns the query's number with its effect, since a caret key's watch becomes the watch the held keys wait on by its number.
- The differential check matched on 11,760 of 11,762 sequences. The two that differ, a console review command whose model a refocus or an eviction dropped before the focus moved on, differ only in the nodes held: before, the dropped review's holder kept the console's node held though nothing could answer it; now the review's query ends with its model, so the node is no longer held once the focus leaves it. No effect differs.
- The shell items' expiry is withdrawn by the request table naming each expired request with its outpost (`Expired`).
- No test drives an owed read asked while one is open: the rules ask for one only from a read Core takes, and Core takes none while one is owed, so no sequence of inputs reaches it; the rule is kept as written.

### Two records of each query today

Each of the reducer's queries is recorded twice, under two numberings, by two owners on the same thread:

- **The request table** (`verbatim-app`, `src/requests.rs`), under a `RequestId` it allocates (160 to 182), records the query's ending: the outpost's answer (`finish`, 191), "gone" when its outpost ends (`outpost_ended`, 211), or its deadline (`expire`, 228). Its asker carries the reducer's own number: `Asker::Reducer { query_id, kind, trace_id }`, `Asker::Text { query_id, trace_id }` and `Asker::Activation`, beside the shell's own five askers (a tree dump, focus-now, the fake focus, and the two shell item queries). `deliver` (344 to 446) turns an outcome into one of three reducer inputs, `FetchCompleted`, `TextCompleted` or `ActivationCompleted`. Whether a query is passive, so not counted by the idle barrier, is decided by the shell from the request's operation (`main.rs` 1829), as is the held read's deadline (1831 to 1835, which the caret watch build moves into Core).
- **The reducer's holders**, under a `QueryId` from `SrState::next_query_id`, record what the answer continues: 16 fields in 5 structures.
  - `SrState`: `latest_navigation` (`verbatim-core`, `src/state.rs` 686), `pending_caret` (703), `superseded_caret` (708) and `pending_text` (723);
  - `HeldKeys`: `reading` (398), and `awaiting_watch` (407), which names the one watch the held keys wait on;
  - `SayAll::pending` (605);
  - the terminal's speech state: `request`, `cancel` and `overtaken` (`src/terminal.rs` 103, 111 and 117);
  - each terminal model: `owed`, `fresh`, `history_asked` and `find_asked` (`src/terminal_model.rs` 192 to 207), `reviews` (197), whose `Review::Text` and `Review::Mark` hold the numbers of review commands Core answers from its memory, and `history` (204), whose `HistoryRead::review` names one of them.

An answer is routed by trying the holders in a fixed order (`text_completed`, `src/reduce.rs` 573 to 628), shown here shortened:

```rust
if let Some(reading) = state.held_keys.reading.take_if(|r| r.query_id == query_id) { ... }
if let Some(pending) = state.pending_caret.take_if(|p| p.query_id == query_id) { ... }
if let Some((_, node)) = state.superseded_caret.take_if(|(s, _)| *s == query_id) { ... }
if let Some(pending) = state.pending_text.take_if(|p| p.query_id == query_id) { ... }
if say_all::is_pending(state, query_id) { ... }
if terminal_model::is_pending(state, query_id) { ... }
if terminal::is_pending(state, query_id) { ... }
Vec::new()
```

Each try is also the query's identity check: a cleared or replaced holder no longer names the number, so a late answer finds nothing and is dropped. Consequences of the two records:

- An outpost's end is handled twice: the reducer's `OutpostEnded` clears holders by node (`outpost_ended`, `src/reduce.rs` 1358 to 1415), then the table answers each entry "gone" (`main.rs` 1982). Those answers find the cleared holders and are dropped, except for the held read, the superseded caret, the terminal's requests, the activation, and the held keys' resume through `awaiting_watch`.
- A superseded query has no holder but keeps its table entry, so the idle barrier still waits for it; nothing in Core records that.
- A review command on a terminal Core models is never sent: `terminal_model::intercept` (`src/terminal_model.rs` 1062 to 1086, applied to every input's effects at `src/reduce.rs` 69) takes its request out of the effects and keeps it in the model's `reviews`, and Core answers it from its memory by calling `text_completed` itself (1020, 1232 and 1254); `command_mark` (`src/review_text.rs` 145 to 160) allocates a number and makes no request at all.
- Request effects are built at 12 sites, each allocating its number and filling its own holder, among them two byte-identical helpers (`review_text::request` and `editing::navigator_request`) and an inline copy of them (`reduce.rs` 1694); three modules keep an `is_pending`.
- A withdrawal needs a lookup across the numberings: the caret watch build's withdrawal of a held key's read must find the table's entry by the reducer's number.

### The one table

Core's reducer owns its queries in one field of `SrState`, `queries: Queries`, in a module of its own (`verbatim-core`, `src/queries.rs`):

```rust
pub(crate) struct Queries {
    next: u64,
    open: BTreeMap<QueryId, Open>,
}

pub(crate) struct Open {
    outpost: OutpostId,
    /// The trace the request runs under, set when the reducer's
    /// transition ends ("Traces", below); `None` for one made under an
    /// input with no trace.
    trace_id: Option<TraceId>,
    asked: Asked,
    then: Then,
}

/// Who answers: the outpost, to a read the idle barrier waits for or to
/// a caret key's watch it does not, or Core from a terminal's memory.
pub(crate) enum Asked { Read, Watch, Memory }

/// What the answer continues.
pub(crate) enum Then {
    Navigation { from: NodeId, kind: QueryKind },
    Activation,
    Caret(PendingCaret),
    SupersededCaret { node: NodeId },
    HeldRead { node: NodeId, key: HeldKey },
    Text { node: NodeId, then: TextFollowUp },
    SayAll,
    Terminal(TerminalAsk),
    TerminalModel { node: NodeId, generation: u64, ask: ModelAsk },
    Nothing,
}
```

`TerminalAsk` is `Read`, `Cancel` or `Overtaken`. `ModelAsk` is what a terminal model asked for: `Whole` or `HistoryBlock` (today's `Owed`), `Fresh`, `History { review, above, whole }` (today's `HistoryAsked` without its number) or `Find { hash }`. `generation` is the model's: each model gets one when it is made, from a counter kept with the models, so a model forgotten or evicted and made again for the same node has a newer one. `PendingCaret`, `HeldRead` and `PendingText` lose their `query_id`, which is the map's key. The table is serialized with the rest of `SrState`, so a dump replays it.

The states of a query: absent before it is asked; open with a continuation; open with `Nothing`, superseded, cleared or never awaited (say-all's caret move, the terminal's hold), until its answer comes; and absent again once ended. The open entry with `Nothing` exists today without a name, as a table entry with no holder.

What stays outside the table, each with its reason:

- `HeldKeys::awaiting_watch: Option<QueryId>`: it is not a record of a query but the held keys' designation of the one watch they wait on, and it moves and clears by the held keys' own rules (finding three of the review shows that a flag on each entry could mark two watches).
- A terminal model's `reviews` and `history`: the queue holds the order of the review commands waiting for the model's answer and each one's operation, beside `StatusBar` items that are no query, and `history` holds a history read's rows; the numbers in them are references to `Memory` entries, which hold the continuation, as `awaiting_watch` is a reference to a watch.

A terminal model's four asks (`owed`, `fresh`, `history_asked`, `find_asked`) go into the table as `TerminalModel { node, generation, ask }` entries. Their rules:

- **The model's questions become lookups.** Where the model's logic asks whether it is owed a read, or has a fresh, history or search read out (`src/terminal_model.rs` 832, 859, 931 to 934, 946, 965 and 1097), it asks the table for an open entry with its node, its generation and that `ModelAsk`; entries with `Nothing` do not count. An owed read asked while one is open turns the older to `Nothing`, as overwriting `owed` drops its answer today (748); the other three are asked only when none is out (1097), and the build asserts it. Reason: one record of each ask.
- **The identity check is the generation.** An answer to a `TerminalModel` entry runs only when the model now holding its node has its generation; `terminal_model::reply` takes the entry's `ModelAsk` and matches on it, in place of matching the number against four fields. A model forgotten, evicted or ended takes its asks' identity with it, as its fields did. Reason: `TerminalModel::asked` today refuses exactly the answers whose model is gone or made again, and a generation that differs refuses the same ones.

The rules, each with its reason:

- **One way to ask.** `Queries::ask(outpost, query, then) -> Effect` allocates the number, inserts the entry with `Read` (`Watch` for `TextOp::AwaitCaret`) and returns the request's effect (`Effect::Fetch`, `Effect::Text` or `Effect::Activate`, which gains its `query_id`). `Queries::ask_memory(node, then) -> QueryId` inserts a `Memory` entry with no effect, for `command_mark`. Reason: no effect can name a number the table lacks, and a query without an entry would have no ending. The 12 sites call one of the two after their own checks, so `say_all::read_next` asks only once it has found say-all running (`src/say_all.rs` 133 to 136 allocate first today).
- **Core's own answers.** When `intercept` takes a request out of the effects, it turns that entry's `asked` to `Memory`. A `Memory` entry is not counted outstanding, is never sent, and ends by `Queries::answer_from_memory(number, reply)`, which `answer_reviews` and the two other sites call in place of `text_completed`: it removes the entry and runs its `then`. A model's removal ends its reviews' `Memory` entries with no continuation, since a review whose model is gone is never answered today: every removal of a model goes through one method, `SrState::drop_models(predicate)`, which `forget_text_of` (`src/reduce.rs` 469), the eviction past `MODELS` (`src/terminal_model.rs` 449) and `terminal_model::outpost_ended` (1403) call, and `state.terminals` becomes a type whose only removal is that method. Reason: a query Core answers itself has an ending too, and the idle barrier never waits for it, as it never did.
- **Supersession and clears turn an entry to `Nothing`.** They never remove an entry the outpost will still answer, so a late answer still has its one entry to end, and an entry with `Nothing` runs nothing. A `Memory` entry superseded or cleared is removed at once instead, since nothing else will end it. Every site that clears or replaces a holder today becomes one table operation ("Every clear, and its ending", below).
- **One ending.** An entry leaves the table exactly once, by one of four transitions, and its continuation runs then, once:
  - `Input::Answered { trace_id, query_id, outpost, outcome }`, the outpost's reply, taken only from the outpost asked, as `finish` does today;
  - `Input::OutpostEnded { outpost }`: first today's clearing, each cleared holder now a turn to `Nothing` and each dropped model through `drop_models`, then every remaining entry of that outpost ended "gone" in number order;
  - a withdrawal by the entry's owner: the held key's one deadline (the caret watch build's `Input::HeldKeyDue`), finding its key still being read, removes the `HeldRead` entry, runs its continuation as unanswered, which passes the key on, and returns `Effect::Withdraw { outpost, query_id }`, which the shell sends as `CoreToOutpost::Cancel`;
  - Core's answer from a terminal's memory, or the removal of its model, for a `Memory` entry.

  An answer that finds no entry, or comes from another outpost than the entry's, is dropped. Reason: one ending per entry, and none after it.
- **Answers routed by the entry, not by search.** Ending an entry removes it and matches its `then`. An outpost's outcome is turned into a `FetchResult`, a `TextReply` or an activation's result in Core, by the conversions `deliver` makes today; the shell logs a failed outcome before applying it, since `verbatim-core` has no logging. After the continuation, an ending whose number is `HeldKeys::awaiting_watch` clears it and resumes the held keys (`editing::held_watch_answered`), whatever the entry's `then` and however it ended, in the order `reduce_text_completed` keeps today (`src/reduce.rs` 560 to 567). Reason: the kind is known where the entry is, so no order of tries decides it.
- **Passive from the request.** `Queries::outstanding()` counts the entries whose `asked` is `Read`, and the shell's idle check adds it to its own table's count. Reason: whether a query waits on the application is a fact of what was asked, which Core makes.
- **Held nodes from the table.** `held_nodes` (`src/state.rs` 927) takes the navigation's `from` and the `Caret`, `Text` and `HeldRead` entries' nodes from the open entries, as it takes them from the four holders today; a `SupersededCaret`'s node is not held, as today, and an entry with `Nothing` holds nothing.
- **One numbering, tagged by asker.** The protocol's bare `u64` request ids (`CoreToOutpost::Query` and `Cancel`, `verbatim-model` `src/protocol/mod.rs` 641 and 655, and `OutpostToCore::Reply`, 1097) become a `RequestId { asker: Requester, number: u64 }`, where `Requester` is `Reducer` or `Shell` and is marked `#[non_exhaustive]`, so that later askers, such as M5's extensions, can be added. Outposts carry it opaquely: they store, compare and echo it, and decide nothing by it. Core's reducer's queries go out as `Reducer` with the `QueryId` as their number; the shell's table numbers its own five askers as `Shell` from its own counter. The shell routes a reply by its tag: `Reducer` to `Input::Answered`, `Shell` to its table. Reason: one number per query, no lookup across numberings, and a route stated in the message rather than inferred from a number's size.
- **No deadline in Core's table.** After the caret watch build, no query of Core's ends at a table deadline: the held read ends by Core's held-key deadline, as a withdrawal. The shell's table keeps its deadlines on the shared clock, for the shell item queries.

**Traces.** The shell runs a request under the trace it executes the effect with: the key's trace inside `Effect::ForKey`, else the input's trace, else one it mints. Several asking sites have no trace (`caret_key`, `start_held`, `say_all::read_next`, `terminal::request`, `terminal_model::request`). So `reduce` ends each transition by setting the trace of the entries made in it from the effects that carry them, by the shell's rule, leaving `None` where the shell would mint; key origins (`keys::stamp`) are looked up by that trace. An ending with no reply on the wire (an outpost's end, a withdrawal) runs its continuation under the entry's trace, or the ending input's when it is `None`, which like today's minted trace has no key origin. An answer from the outpost runs under the reply's trace, which is the request's.

The invariant, for M4.6's model of Core: every query Core asks has exactly one entry from its request to its ending, and exactly one ending, by its answer, its outpost's end, its owner's withdrawal or Core's answer from memory; every ending runs its continuation once, and an answer with no entry, or to an entry with `Nothing`, changes nothing but the table and the one resume of the held keys.

### Every clear, and its ending

Each site that clears or replaces a holder today, from the code at 39dce0b2, and its table operation. The test of each in the build is the differential check under "Tests", which compares every enumerated sequence's effects with today's routing.

- **A caret key's watch (`pending_caret`).**
  - A newer caret key (`src/editing.rs` 190): an open `Caret` entry on the same node becomes `SupersededCaret`, any older `SupersededCaret` becoming `Nothing`; on another node, `Nothing`.
  - `forget_text_of`, at a refocus with `earlier: NotFocused` or a focus whose element is gone (`src/reduce.rs` 467): the `Caret` entry on that node becomes `Nothing`.
  - `end_text_activity`, when the focus moves (1216): the `Caret` entry becomes `Nothing`.
  - The outpost's end (1385): the `Caret` entry on that outpost becomes `Nothing`.
- **A superseded caret (`superseded_caret`).** Only replaced, at 192, as above; it is not cleared at an outpost's end today, so it is answered "gone" there.
- **The held keys' watch (`awaiting_watch`).** Unchanged in place: moved to a newer caret key's watch while it names one (`src/editing.rs` 200 to 202), set to a passed key's own watch (382), cleared when the held key's deadline ends the wait (320 and 433, the caret watch build's `HeldKeyDue`) and when the watch it names ends (`src/reduce.rs` 562).
- **A held key's read (`HeldKeys::reading`).** Cleared only by its answer (`text_completed`), and by the caret watch build's withdrawal; both are endings.
- **A review or text command's request (`pending_text`).** A newer request at `review_text::request`, `navigator_request`, the location request (`src/reduce.rs` 1694) or `command_mark` (`src/review_text.rs` 152): the open `Text` entry becomes `Nothing`, or is removed if it is `Memory`. The outpost's end (`src/reduce.rs` 1392): the `Text` entry on that outpost becomes `Nothing`.
- **A navigation (`latest_navigation`).** A newer navigation (`navigate`, 1959): the open one becomes `Nothing`. `navigator_to_focus`, for `ToFocus` and a navigation answered "gone" (1771): `Nothing`. The outpost's end (1412): `Nothing`. The answer's own clears (2593 to 2609) are its ending.
- **Say-all's read (`SayAll::pending`).** `say_all::stop` (`src/say_all.rs` 124), also called at an outpost's end, and a new say-all replacing the old (102): the `SayAll` entry becomes `Nothing`. Its answer's clear (172) is its ending.
- **The terminal's requests (`request`, `cancel`, `overtaken`).**
  - A cancel (`src/terminal.rs` 1168): the open `Terminal(Read)` becomes `Overtaken`, any older `Overtaken` and `Cancel` becoming `Nothing`.
  - `terminal::focus_moved` (1190), which resets the terminal's speech state when the focus moves to another node: every `Terminal` entry becomes `Nothing`. Reason: `Then::Terminal` names no node, and console C1's read answered after the focus moved to console C2 would otherwise be taken into C2's output (`terminal::answered`, 470 to 494).
  - The answer's own clears (485 to 491) are its ending.
- **A terminal model's asks and reviews.** Removed with the model, through `drop_models`: by `forget_text_of` (`src/reduce.rs` 469), the eviction past `MODELS` (`src/terminal_model.rs` 449) and the outpost's end (1403). Its `TerminalModel` entries stay until answered, and their answers find no model of their generation and run nothing, as `TerminalModel::asked` refuses them today; its `Memory` entries are removed then. An owed read asked while one is open (748) turns the older to `Nothing`. The model's own clears of `owed`, `fresh`, `history_asked` and `find_asked` (922, 943, 968 and 981) are the answers' endings.

### The shell items' one bound

A shell items or position request waits under two deadlines in a row today (`verbatim-app`, `src/main.rs` 1898 and 1917): `Wanted`'s, `SHELL_ITEMS_DEADLINE` (6 seconds) from the dialog's request while the taskbar's outpost starts, then a fresh 6 seconds in the table when the query is sent, so the dialog can wait about 12 seconds. The 6 seconds are set to outlast the outpost's `WALK_DEADLINE` (5 seconds from when its worker begins the query) by a second, so that the items a walk read are not lost to Core's deadline; the walk itself stops making calls after its `WALK_BUDGET` of 2 seconds (`verbatim-outpost`, `src/outpost/shell_items.rs` 69) and answers what it read, and the 5 seconds allow for one call under way at that budget, which UIA's transaction timeout ends.

The new bound: the dialog's whole wait, for a list or for a clicked item's position, is at most 3 seconds from its request (Dickson, 2026-11-01). Core does not cut the walk off; it gives the outpost the shorter time, so the walk answers with what it read in time. Three times, all on the shared clock and fixed when the dialog asks at `asked`:

- `answer_by = asked + 3 s`: Core's deadline for the answer, in the shell's table;
- the outpost's deadline for the query, `asked + 2.5 s`: past it the worker's query is abandoned and answered `Abandoned`, leaving half a second for the answer's way through the pipes and the reducer thread;
- the walk's last call, `asked + 2 s`, or 2 seconds after the walk begins if that comes first: from then it makes no more calls and answers what it read, leaving half a second for one call under way.

The mechanism:

- `Query::ShellItems` and `Query::ShellItemPosition` gain `walk_until_us` and `answer_by_us`, the walk's last call and the outpost's deadline. Core sets them from `asked` when it sends the query.
- In the outpost's reducer, `budget` (`verbatim-outpost-logic`, `src/lib.rs` 1584) gives these two queries the deadline `answer_by_us` from the query, a due time on the shared clock like the caret watch's, in place of `WALK_DEADLINE` from their start; `Ancestors` and `DumpTree` keep `WALK_DEADLINE`.
- In the worker, `shell_items` and the position read take their time limit from `walk_until_us` and `WALK_BUDGET`, whichever ends first, in place of `WALK_BUDGET` alone.
- In the outpost's reducer, a shell items or position query reached at or after its `walk_until_us` is answered at once with nothing read, as a walk out of time answers, and given to no worker, mirroring `Wanted`'s rule: the walk's first call is made regardless of its time limit, so a walk begun then could pass the outpost's deadline, and the abandoned worker would read as the application not responding.
- `Wanted` keeps `asked` with the request and hands it to `ask_shell_items`, which takes it in place of a fresh time. A request whose outpost becomes ready at or after its walk's last call is answered with nothing at once and not sent, since its walk could make no call; `Wanted::expire` answers a request still waiting then. A request whose outpost is ready when the dialog asks is sent at once.
- At `answer_by`, Core's table answers the dialog with nothing and sends `Cancel`, so a query still queued in the outpost is not run for nothing.
- `SHELL_ITEMS_DEADLINE` is replaced by `SHELL_ITEMS_WAIT` (3 seconds) and the two margins, each with its reason in its comment; `WALK_DEADLINE`'s comment no longer says Core outlasts it.

Sequences:

- **Ready at once.** The dialog asks at t; the outpost is ready, and the query goes out at t with `walk_until_us = t+2 s` and `answer_by_us = t+2.5 s`. The worker begins at t+10 ms and walks until t+2 s at most, then answers the items read; the dialog has them at about t+2 s, well inside t+3 s.
- **A slow start.** The dialog asks at t; the taskbar's outpost is ready at t+1.2 s, and the query goes out then with the same times. The walk has 0.8 seconds and answers what it read by t+2 s. Today it would have had its whole 2 seconds and answered by about t+3.2 s.
- **Too late to walk.** The outpost is ready at t+2.1 s: the request is answered with nothing and no query is sent.
- **A call that does not return.** A call under way at t+2 s has not returned at t+2.5 s: the outpost abandons the query and answers `Abandoned`, and the dialog gets nothing at about t+2.5 s. Core's deadline at t+3 s is the bound if even that answer is late.
- **Behind other work.** The query waits in the outpost behind an event's program until t+1.5 s: the walk has until t+2 s, and the outpost's deadline is still t+2.5 s, measured from the dialog's request, not the worker's start. Had it waited until t+2 s or later, the outpost would answer it with nothing at once and run no walk.

Wake-ups and calls: no timer is added in the outpost, whose deadline for the query replaces the one it timed from the start; Core's table keeps one deadline per request, as today; at its expiry one `Cancel` is sent.

What the user notices: the dialog's list or click comes, or fails, within 3 seconds of the request rather than 6 to 12; on a slow taskbar, it lists the items read in what remains of 2 seconds rather than 2 full seconds.

Limitations: a walk after a slow outpost start, or behind other work in the outpost, has less than 2 seconds and lists fewer items on a large taskbar; a call still under way half a second after the walk's last call loses the whole walk to `Abandoned`, where today it had 3 seconds to return; and the margins of half a second are judgements, which the build checks against the replies' timing on this machine.

This is a commit of its own, since it concerns no query of Core's reducer. No end-to-end scenario opens the shell item list dialogs, so it is held by unit tests: the outpost's deadline for the two queries from `answer_by_us`, the worker's time limit from `walk_until_us`, and `Wanted`'s sending, refusing and expiring of a request.

### Sequences

Each is one reducer transition unless the shell's part is named.

- **An answer.** A review command asks for the current line in a text box: `ask` inserts Q7 as `Text { node, then: Seed }` with `Read` and returns `Effect::Text` with Q7; the shell sends it with request id `Reducer` 7. The reply comes; the shell sees the `Reducer` tag and applies `Answered { query_id: Q7, outpost, outcome }`; the reducer removes Q7 and runs `review_text::reply` with the `TextReply` made from the outcome.
- **Core's own answer.** The focus is a console Core reads. "Read current line" asks Q8 as `Text` with `Read`. At the transition's end `intercept` takes Q8's `Effect::Text` out, turns Q8 to `Memory`, keeps `Review::Text { query_id: Q8 }` in the model, and asks fresh read F9 as `TerminalModel { node, generation: 4, ask: Fresh }` with `Read`. F9 is answered; its model has generation 4, so `terminal_model::reply` runs its `Fresh` branch; the model takes the snapshot and `answer_reviews` calls `answer_from_memory(Q8, reply)`, which removes Q8 and speaks the line through `Text`'s continuation. `outstanding()` counted F9 until its answer and never counted Q8. "Next command mark" (`command_mark`) asks `ask_memory`, M10, and ends the same way.
- **A model removed while a review waits.** Q8 is `Memory` and F9 open as above; the user tabs away and back to the console, and the focus event carries `earlier: NotFocused`: `forget_text_of` calls `drop_models`, which removes the model and Q8, with nothing run; F9 stays open. The refocus's text makes a new model for the node, generation 5. F9's answer finds generation 5, not 4, so nothing runs, and F9 is removed. Today Q8's review vanished with the model, and F9's answer was dropped because the new model's `fresh` did not name it.
- **A refocus clears a caret key.** Right Arrow on edit N asks watch Q4, `Caret`. A focus event for N with `earlier: NotFocused` comes: `forget_text_of` turns Q4 to `Nothing` and the refocus announces N's line. Q4's answer removes Q4 and runs nothing, as today's `take_if` finds no holder.
- **A model evicted.** Core models terminals T1 to T16. A review on T1 asks fresh read Q5 under T1's model's generation g. A seventeenth terminal's first snapshot evicts T1's model through `drop_models`; T1's next snapshot makes a new model with a newer generation. Q5's answer finds no model of generation g, so nothing runs; meanwhile the new model's lookups never see Q5, so it does not wait for it.
- **The focus moves between consoles.** Console C1's `Terminal(Read)` R1 is open; the focus moves to console C2: `terminal::focus_moved` turns R1 to `Nothing`. R1's answer runs nothing, so C1's output is not taken into C2's.
- **A late answer after supersession.** Navigation N1 is asked, then N2 before N1's answer: N1 turns to `Nothing` and N2 is the open navigation. N1's answer removes N1 and runs nothing; N2's moves the navigator. In the other order, N2's answer moves the navigator, and N1's later answer removes N1 and runs nothing. Both are outstanding for the idle barrier until answered, as today.
- **An answer to an ended query.** A reply whose id has no entry, after a withdrawal or as a duplicate, is dropped; one from another outpost than the entry's is dropped and the entry stays.
- **A deadline: the held read withdrawn.** Held key K's caret read R is asked at t as `HeldRead { node, key: K }` with `Read`, and Core's held-key deadline is t+150 ms. At t+150 `HeldKeyDue` finds K being read: R is removed, `held_read` runs as unanswered and passes K on, and `Effect::Withdraw { outpost, query_id: R }` is sent as `Cancel`. The outpost answers `NotStarted` if R was still queued, or its read's answer if it was running; either finds no entry and is dropped. From t+150 R is not outstanding, so the idle barrier does not wait for it. That is the caret watch build's decision, built here without a second table.
- **A stale watch cannot resume the held keys.** K1 is passed on at t; its watch W1 is open and `awaiting_watch` is W1. K2 is pressed at t+20 and waits. At t+100 `HeldKeyDue` ends K2's wait for W1: `awaiting_watch` becomes `None` and K2's caret is read. The read answers; K2 is passed on, its caret key turns W1 to `SupersededCaret`, and its watch W2 becomes `awaiting_watch`. K3 is pressed and waits behind W2. W1's late evidence comes: W1 is removed and `superseded_reply` runs, as today; its number is not `awaiting_watch`, so the held keys are not resumed, `later_key` is not called on W2, and K2's movement is spoken at W2's answer.
- **An outpost ending.** Outpost A has Q3, a caret key's watch, which `awaiting_watch` names, Q5, a review command's read, and Q6, an activation; outpost B has Q7, a terminal read. `OutpostEnded { outpost: A }`: the focus dies, say-all on A stops, the caret is forgotten, Q3 and Q5 turn to `Nothing`, and A's models are dropped; then A's entries end "gone" in number order: Q3 runs nothing and, being `awaiting_watch`, resumes the held keys; Q5 runs nothing; Q6 says "No action". Each is as today. Q7 is untouched.
- **Core's idle round.** The harness asks for idle; the shell's check is clear only when `queries.outstanding()` and its own table's count are both 0, with its other conditions. A watch and a `Memory` entry are not counted; a superseded read is counted until its outpost answers; a withdrawn held read is not counted from its withdrawal.
- **A query that cannot be sent.** The shell's send of Q fails; it applies `Answered { outcome: Failed }` at once, which ends Q as today's `finish` does.

### What goes

- From `verbatim-app`: `Asker::Reducer`, `Asker::Text` and `Asker::Activation` and their three `deliver` branches; `begin_passive` and `Entry::passive`, since no shell asker is passive; the table's `timed`, kept only because passive caret watches kept the table non-empty while the user typed; `text_request`'s judgement of the operation; the caret watch build's lookup of a held read's entry by the reducer's number. The shell's table keeps 5 askers, 3 entry fields with no flag, and 2 fields.
- From `verbatim-core`: 13 of the 16 holder fields (the four in `SrState`, `HeldKeys::reading`, `SayAll::pending`, the terminal's three, and the model's `owed`, `fresh`, `history_asked` and `find_asked`), with the `Owed` enum and `HistoryAsked`'s number; `next_query_id`; the ordered tries of `text_completed`; the three `is_pending` functions and `TerminalModel::asked`; the duplicated helpers; the holder clears listed above, which become table operations.
- From `verbatim-model`: `Input::FetchCompleted`, `Input::TextCompleted` and `Input::ActivationCompleted`, for `Input::Answered`.

### Counts, before and after

- Records of each of Core's queries: 2 to 1. Numberings: 2 to 1.
- Holder fields: 16 in 5 structures, to the table, with three references into it kept (`awaiting_watch`, and the model's `reviews` and `history`). Each model gains its `generation`, and the models' type its counter.
- `SrState`'s fields: 27 to 23 (four holders and `next_query_id` go, `queries` comes).
- Answer inputs: 3 to 1. Routing: 7 ordered tries and a fall-through, to one match.
- `is_pending` functions: 3, and `TerminalModel::asked`, to none. Ways to make a request: 12 sites each filling its own holder, to `ask` and `ask_memory`.
- The shell's table: askers 8 to 5; entry fields 4 with 1 flag to 3 with none; fields 3 to 2; constructors 3 to 2. The "Simpler owners" items for the request table (`Ends`, and keeping `timed`) are subsumed.
- Lifecycle states: none added; the open entry nothing awaits, implicit today, is named `Nothing`, and the query Core answers itself is named `Memory`.
- Lines: about 410 removed and 330 added across `verbatim-core`, `verbatim-app` and `verbatim-model`, counted again in the build, beside the tagged id's change of type at 198 sites and the shell items' bound, about 80 lines in `verbatim-app`, `verbatim-outpost-logic` and `verbatim-outpost`. The 45 test sites of the three answer inputs are rewritten to `Answered`.

### What is heard

Nothing changes beyond the determinism build's decision that the outcomes at one outpost's end are taken in order: here the order of their numbers, which is the order the reducer allocated them (a fresh read `intercept` asks is numbered after effects that precede it in the list, which is harmless, since at an outpost's end those entries are cleared first). Every clear of today becomes a turn to `Nothing` or a model's removal, and the ending at an outpost's end keeps today's order, so the activation's "No action", the held read's pass and the held keys' resume happen as today. The shell items' one bound changes the list dialogs, not speech: their list or click comes, or fails, within 3 seconds of the request rather than 6 to 12, and after a slow taskbar start the list holds what the walk read in the time left ("The shell items' one bound").

### Wake-ups and calls

Unchanged on the event and speech path. Each reply is one reducer input, as today's table makes one. No timer is added: Core's table has no deadline, and the withdrawal's `Cancel` is the caret watch build's. The shell no longer records Core's queries, which saves one map insert and one removal per query on the reducer thread; the trace pass at a transition's end walks only that transition's effects, and only when it asked something. The shell items' expiry adds one `Cancel` per expired request. The tagged request id adds a few bytes to each query and reply frame and no message. Measured before and after with `cargo xtask trace scenario` on `measure_held_repeat_text_box` and `measure_keys_notepad`, debug builds, test audio mode, at most 20 runs each: wake-ups per input-hook wake, and the reducer thread's processor time. The expected result is no change.

- Measured on 2026-11-01, debug builds, test audio mode, four 12-second windows each side, before at main ac4eba30: `measure_keys_notepad` woke all processes 31.8 times per input-hook wake after and 31.5 before (medians), and Core's reducer thread took 0.27 ms per hook wake both before and after; `measure_held_repeat_text_box` 13.2 after and 12.1 before, a window catching 10 to 17 hook wakes, so within its spread. No change was measured.

### Tests

- **The guards.** Core's replay tests under `verbatim-core/tests/` (`reduce.rs`, `text.rs`, `merge.rs`, `alloc.rs` and the terminal suites), rewritten to `Answered`, and the request table's tests in `verbatim-app`, cut to the shell's askers. `tests/replay_fixture.rs`'s fixture is regenerated if it holds the holders.
- **New, one per behaviour**, in `src/queries.rs`'s tests and the replay tests: an answer runs its entry's continuation and removes it; a superseded navigation's answer runs nothing and is counted outstanding until it comes; an answer from another outpost leaves the entry; a console review answered from memory is never counted and its entry ends at the answer; a model dropped by a refocus or an eviction removes its reviews' entries and drops its fresh read's answer, and the new model made for its node does not wait for that read; an owed read asked while one is open drops the older's answer; a refocus with `earlier: NotFocused` drops the earlier caret key's answer; a console read answered after the focus moved to another console is not taken; at an outpost's end, the watch the held keys wait on resumes them and an activation says "No action"; a held read withdrawn at Core's deadline passes its key on once and returns `Withdraw`, and its late answer and a `NotStarted` are dropped; a superseded watch's late answer does not resume the held keys; the shell routes a `Reducer` reply to the reducer and a `Shell` reply to its table; a `RequestId` round-trips through the protocol and the raw reply path; a shell items walk stops its calls at its query's `walk_until_us` and answers what it read; the outpost abandons a shell items query at its `answer_by_us`; a shell items request whose outpost is ready after its last send time is answered with nothing and not sent; an expired one is withdrawn.
- **The differential check, made once in the build.** Step one keeps the holders and their tries beside the table; step two replaces them. The enumeration below is run on step one's commit, recording a digest of each sequence's effects and of `held_nodes` after each step, and again on step two's, which must match on every sequence. This proves that what is heard does not change. The digests and the result go in step two's commit message; the check is not kept as a test, since its reference is the code step two deletes.
- **A generated test, kept**, as the design principles ask:
  - Three start states, each reached from the default state by inputs: a text box focused with a caret on outpost A, the same with a held caret key passed on and its watch waited on, and a console Core reads, focused with a model, on outpost B.
  - Inputs: a caret key; a held caret key; `HeldKeyDue`; "read current line" (answered from memory in the console); a navigation command; `ToFocus`; say-all's start and stop; a terminal key and a speech cut, which ask the terminal's read and cancel; a focus move to another node of the same outpost; a refocus of the start node with `earlier: NotFocused`; in the console, the first snapshots of 16 other terminals as one step, which evicts the model; `OutpostEnded`; and `Answered` for the oldest open number, the newest, the last ended, and one never asked, with outcomes grouped as `Done` with a reply fitting the request's operation, `Gone`, and `Failed`, plus the newest from the other outpost.
  - Its reference model is the wire's view, not the table's bookkeeping: the numbers in the request effects, less those answered by an accepted `Answered`, those of an ended outpost and those in `Withdraw` effects, each marked a watch or a read by its operation. It knows no continuation and no supersession.
  - After every step it checks: `outstanding()` equals the model's open reads; every open entry that is not `Memory` is open in the model; after an outpost's end none of its numbers is open; an `Answered` the table does not take, with no entry, from another outpost or for a `Memory` entry, leaves `SrState` equal and returns no effect; one whose entry has `Nothing` leaves `SrState` equal apart from `queries` and, when the number is `awaiting_watch`, the held keys' resume, and returns no effect but the resume's; `awaiting_watch` is `None` or names an open `Watch` entry; at most one open entry each with `Caret`, `Text`, `Navigation`, `HeldRead`, `SayAll`, each `TerminalAsk`, and each `ModelAsk` kind per model generation; no `Memory` entry names a node without a model; and `held_nodes` equals the nodes of the focus, its ancestors and last selection, the navigator, the caret, the start marker and say-all, with the open entries' `from` and `Caret`, `Text` and `HeldRead` nodes.
  - About 22 inputs a step: every sequence to depth 3 is enumerated without randomness, 29,059 over the three starts; the depth is set in the build to keep the run under 10 seconds in a debug build, with the measured time in the commit. It runs under `cargo xtask ci`.

### End-to-end scenarios and the order of work

Scenarios, each run three times at the end of the build, none recorded in `docs/flaky-tests.md`:

- `text_box_editing`: caret keys' watches, held keys' reads, and supersession on one node;
- `object_navigation_over_uia`: navigation queries, superseded and answered;
- `conhost_editing`: the terminal's request, cancel and overtaken answers, the terminal model's asks, and review answered from its memory.

No scenario reaches an outpost's end with queries open, a withdrawn held read, a model's eviction, or a shell items request after a slow start; those are held by the unit, replay and generated tests.

The order of work:

- Measure before.
- **The tagged request id**, first, since every later commit sends or routes by it. `RequestId` and `Requester` in `verbatim-model`'s protocol, in place of the bare `u64` in `CoreToOutpost::Query`, `CoreToOutpost::Cancel` and `OutpostToCore::Reply`, and in the raw reply path's `Wire::Reply` (`verbatim-process`, `src/message/raw.rs` 53). The sites that name `request_id`, at 39dce0b2: 198 in all, of them 116 in the outpost crates (`verbatim-outpost` 46, `verbatim-outpost-logic` 70), `mockapp` 37, `verbatim-supervisor` 13, `verbatim-process` 12, `verbatim-app` 10, `verbatim-model` 9 and `verbatim-core` 1; in the outposts the change is the type of a stored and echoed value, `Item::Query`'s, `budget`'s and the replies'. The JSON written by hand that changes: `verbatim-model` `src/protocol/tests.rs` 431 and `verbatim-process` `src/message/raw.rs` 826 and 858; no file under `crates/mockapp/tests/fixtures` holds a request id. In this commit every request Core sends is `Shell`, numbered by the table as today, so the commit changes the type alone. One commit, with the round-trip tests.
- **The shell items' one bound**, with its tests. One commit.
- **Step one, the table in the reducer with one numbering.** `Queries` with `outpost`, `trace_id` and `asked`; every request made by `ask` or `ask_memory`; `intercept`'s turn to `Memory`, `answer_from_memory` and `drop_models`; the trace pass; `Input::Answered` replacing the three inputs, routed by today's tries after the entry is removed; the outpost's end and the held read's withdrawal as reducer transitions; Core's queries sent as `Reducer` with their `QueryId`, and replies routed by the tag; the shell's table cut to its five askers; the idle check's sum; with their tests and the generated test, and the differential enumeration's digests recorded. One commit, since the shell and the reducer change together.
- **Step two, the holders replaced.** `Then` on each entry and the routing by it; every clear listed above made a table operation; the terminal model's asks as entries, its questions as lookups, and its generation as their identity check; the 13 holder fields, the three `is_pending` functions, `TerminalModel::asked` and the duplicated helpers removed; `held_nodes` from the table; with the differential check matched. One commit.
- `cargo xtask ci`, the scenarios, and measure after.
- Documents: the crate guides of `verbatim-core`, `verbatim-app`, `verbatim-model`, `verbatim-outpost`, `verbatim-outpost-logic` and `verbatim-process`; `docs/design/key-holding.md` on the held read's withdrawal; `docs/design/terminal-model.md` on review answered from memory; `docs/design/listener-without-connections.md`, section 10, step 11, on the shell items' 3-second bound, replacing its 6 seconds; and this document's determinism section on the request table.

This build replaces one piece of the caret watch build, which is being built now: the shell-side lookup by which Core's withdrawal ends a held read's entry in the shell's table. Its effect and its behaviour stay; only where the entry lives changes.

### Limitations

- The supersession and clear of each kind is an operation at each site listed above, checked by the differential check once and by the generated test's invariants after; the types allow a missed clear. Only a terminal model's entries carry an identity check, their generation.
- The shell's five askers keep a table of their own: their answers go to a dialog, a control-plane caller or the shell's focus-now, none of them the reducer's.
- A superseded query still runs in its outpost until it is answered, as today; only the held read and an expired shell items query are withdrawn.

### Decisions

- The shell item list dialog's whole wait, for its list or a clicked item's position, is bounded at 3 seconds from its request, with the outpost given the shorter time so that the walk answers with what it read, rather than Core cutting it off (Dickson, 2026-11-01).
- One answer input, `Input::Answered`, carrying the protocol's outcome, replaces `FetchCompleted`, `TextCompleted` and `ActivationCompleted`, and Core turns the outcome into its result by the entry's kind (Dickson, 2026-11-01).
- The protocol's request id is tagged with its asker, `RequestId { asker: Requester, number }`, open to later askers such as M5's extensions, carried opaquely by outposts and routed by Core by its tag, not a number range (Dickson, 2026-11-01).
- A terminal model's asks are folded into the table in this build, with the model's generation as their identity check, so that no fact is held twice (Fable, 2026-11-01, under the first design principle).

## The work in flight, shared by both readers (a build of its own)

Status: built on 2026-11-01 on branch `inflight`, from main 28eb672d, after its adversarial review (`experiments/logic-tests-study/worker-charge-design-review.md`); the line numbers below are main's at 39dce0b2, where it was written. The mechanisms are described in the crate guides: [verbatim-reader-rules](../crates/verbatim-reader-rules.md) for `InFlight`, [verbatim-outpost-logic](../crates/verbatim-outpost-logic.md) and [verbatim-outpost](../crates/verbatim-outpost.md) for the outpost, and [verbatim-listener-logic](../crates/verbatim-listener-logic.md) and [verbatim-listener](../crates/verbatim-listener.md) for the listener. Where the build differs from the text below, pending Dickson's acceptance:

- Every start of a listener thread, a reused abandoned thread's included, takes `succeed()`, as every start took a fresh number before; the generation an abandonment made may already have been taken by a thread started since, so reusing it was not safe.
- The listener's `Publish` and `Published` carry `then_abandon`, as the outpost's do, so the schedule keeps nothing between an ask and its answer.
- `InFlight::start` takes the item's window as `Option<isize>`, `None` for an item never judged and `Some(0)` for one whose window is to be learned, as the listener's events already say it; `walk_returned` and `moved_on` take no time, which they do not use; and two queries are added, `deadline()`, for the outpost's generated test, and `item_mut()`, for what each reader keeps in its item.
- The outpost's drop program at its shutdown, which runs with nothing in flight, is numbered by a start and an end at once.
- The listener's shell tests whose rule this build removes, the front judged as a read starts and a read with no foreground event since its event, are replaced by a read taken by a new thread being judged at its own grace's end; the test of a read left alone within its grace holds the readers' lock until the grace ends, the grace no longer being the test's to set.

Measured, debug build, stand-in reads in the listener's shell test process: 20 quick reads cost the watchdog no wake-up before and after, and 20 moved-on calls before against none after; a read held 3 seconds with no foreground event cost no wake-up and one call before, and 5 wake-ups and 5 calls after; held 3 seconds with two foreground events, 2 wake-ups and 3 calls before, and 5 and 5 after. It is candidate two of the simplification study (`experiments/logic-tests-study/radical-simplification-study.md`), "one pure worker charge". It follows the first determinism build ("Determinism") and takes over that build's asks: `AskMovedOn` and `MovedOn`, and `Publish` and `Published` with `then_abandon`. It replaces three items of "Simpler owners": the outpost's walk as one state, its abandoned count kept by the shell alone, and the listener's running place kept with the read.

The work in flight is a reader's record of the one slow-call item a worker is running. A worker is the outpost's worker thread, or one of the listener's reading threads. The outpost calls this record "the program in flight", and the listener its running read. The record holds:

- which worker generation is in charge. A generation is the numbered worker; abandoning it starts the next number, and anything the dead number sends later is refused;
- the item's deadline;
- its moved-on rule, with the grace `MOVED_ON_GRACE`, 500 ms;
- the publication of its walk at the walk's bound;
- its abandonment.

Both readers keep this record, written twice with different judging points. This build writes it once, as a pure type both embed.

### The concern as written twice today

Line numbers are main's at 39dce0b2. The first determinism build moves them a little, and turns the outpost's three `Locals` calls into asks.

**The generation.**

- **Outpost.** There is one generation for the whole outpost: `Reducer::generation`, with the request counter `requests` (`verbatim-outpost-logic`, `src/lib.rs` 452 and 454), numbered by `next_request` (1176). An abandonment adds one and emits `Effect::Abandon` with the replacement, which the shell starts at once (`verbatim-outpost`, `src/outpost/shell.rs` 716 to 732).
- **Listener.** There is one generation per application, `App::live` (`verbatim-listener-logic`, `src/schedule.rs` 296), drawn from a counter shared by all applications, `next_generation` (`start`, 598 to 609).
  - An abandonment clears `live`.
  - A new thread starts only when events wait and the application is under `ABANDONED_LIMIT`.
  - An abandoned thread that returns may be reused under a fresh number (`returned`, 660 to 708).

**The deadline.**

- **Outpost:** set by the work at dispatch, 10 s, 2 s or 400 ms (`budget`, 1584), and judged first in `judge_abandonment` (1013).
- **Listener:** one `READ_DEADLINE`, 8 s, judged first in `tick` (804 to 812).

**The moved-on rule.** Both readers ask `front_is_another_thread_of_its_application` (`verbatim-win32`, `src/window.rs` 371) about a known window. They judge at different points:

- **Outpost.**
  - The window is fixed at dispatch, 0 for none.
  - The rule is judged once as the grace ends, with no other condition (`tick`, 661 to 668, with `grace_judged`).
  - It is judged again at each foreground fact taken in once the grace has passed (738), whether or not the walk was published (`judge_abandonment` ignores `claimed`, 1003 to 1030).
- **Listener.**
  - The window may be learned during the read (`WatchedRead::learn`, `verbatim-listener`, `src/focus_read.rs` 391).
  - The grace end is judged only with evidence. Either a foreground event came after the read's event was queued (the `foregrounds` counts), or the window in front was already elsewhere when the read's window became known (`judge_front`, 433 to 439). That second test is a moved-on call made on every focus read with a window, by `on_learn` (2207 to 2219) or by the watchdog.
  - Past the grace, the rule is judged at every watchdog tick while that evidence holds (`moved_on_wanted`, 422 to 432).
  - Each foreground event during any read wakes the watchdog (`wake_on_foreground`, 637 and 996).
  - A window learned while the answer was being gathered makes the tick stale (`window_after`, 793 to 802).
  - The evidence rule is e46d4183's (2026-10-20), which removed a wake at the grace end of every quick read.
- **Neither reader** is told of a move to the taskbar or the desktop. Those are `ShellForeground` events, which the listener's hook drops before `foreground_changed` (`listener.rs` 1040 to 1047) and the outpost never receives (`worker.rs` 554).

**Publication at the bound.**

- **Outpost.**
  - The worker's `Input::Armed` sets the walk's bound (595 to 599).
  - A tick past the bound, or a walk returning past it (`WalkReturned`, 600 to 607), publishes the walk through `Locals::publication` (`publish`, 972 to 994).
  - `claimed` then refuses the worker's later results (`accepts`, 526 to 530).
- **Listener.**
  - The reading thread sets the alarm by its bound, through `on_arm` (2224 to 2233).
  - The watchdog gathers each read's bound and, once that has passed, the publication itself (`TickRead`, 128 to 151, gathered at 2276 to 2296), and the schedule publishes it (`publish`, 907 to 937).
  - `WatchedRead` keeps a second copy of that state, its `claim` (305 to 313 and 443 to 456).
  - The watchdog only reads the progress (`prepare_publish`, 512 to 518). The reading thread takes it once its walk ends, and expects to find it (1027 to 1031).

**Abandonment.**

- **Outpost.** A walk that is armed is published first, then the generation dies, in one transition. This holds for the deadline and the moved-on rule alike (`judge_abandonment`, 1028 to 1030; "publish first", Dickson, 2026-10-30, [the focus pipeline rework](focus-pipeline-rework.md), "The outpost's commit point").
- **Listener.** The read is abandoned and its event dropped, without publishing an armed walk. A publication is made in a tick only if no abandonment applies (`tick`, 813 to 823; `abandon`, 851 to 905). A published read that is still stuck is abandoned by either rule, as in the outpost.
- **Then each reader answers its own item.**
  - The outpost answers its running query with `Abandoned`, and the focus group's older entries are tried unless the item was published (1052 to 1075).
  - The listener sends the event's outcome unless the read was published, and starts a new thread when events wait under the limit.

### The one type

The type is `InFlight<T>`, in a new module `in_flight` of `verbatim-reader-rules`.

- Why that crate:
  - it is in the logic tier, and both readers' logic crates already depend on it;
  - it holds `MOVED_ON_GRACE`;
  - the determinism rule covers it;
  - the two readers' crates may not depend on each other, and a rule both apply is written once there ("The crate split").
- The code is new, written from the design documents; no NVDA source is translated.
- `Request { generation, number }` moves into the crate from the outpost's reducer, which re-exports it.

Each reader embeds its own `InFlight`: the outpost one, and the listener one per application, so every fact keeps one owner. `T` is the reader's item:

- for the outpost, the query it answers, its focus group try, and whether it is deferred work;
- for the listener, the event's `Meta`, its kind and its queue place.

Times are microseconds on the reader's own monotonic clock, passed in.

**State.**

- `generation` and `requests`: the generation in charge and the last request number given.
- `running: Option<Running<T>>`, with `request`, `started`, `deadline`, `walk`, `moved` and `item`.
- `Walk` is `Unarmed`, `Armed(bound)` or `Published`.
  - It replaces the outpost's `walk` and `claimed`, and the listener's `published`.
  - Reason: "armed after its publication" can no longer be represented.
- `Moved` is one of three states:
  - `Never`, for an item with no window: selections, notifications, or an outpost program with window 0;
  - `Unknown`, for a listener focus whose window the read has still to learn;
  - `Known { window, next }`, where `next` is the time the rule is next judged.
  - It replaces the outpost's `window` and `grace_judged`, and the listener's `foregrounds` counts, `front_judged` and `front_elsewhere`.

**The moved-on rule, as NVDA's watchdog judges it.** It is judged first when the item has run for the grace, then every grace after that, until the deadline. For the item, this is `started + 500 ms`, then each 500 ms. A window learned after the grace has passed is judged at once, then every 500 ms. NVDA's watchdog judges the same way: first after `MIN_CORE_ALIVE_TIMEOUT`, then every `MIN_CORE_ALIVE_TIMEOUT` until 10 s (`watchdog.py` 133 to 145 and 151 to 163). Foreground events no longer judge anything. Reasons:

- One rule covers every way the user can leave: a foreground change, a move to the taskbar or the desktop, which no reader is told of, and a child window of another thread taking the focus.
- No foreground event has to reach either reader.
- A quick item costs no wake (below, "Wake-ups and calls").

**Transitions.** Each transition answers with nothing, one ask, or an abandonment. It makes at most one ask, which the shell answers.

- `start(item, deadline, moved, now) -> Request`. Nothing may be running.
- `hand_on() -> Request`: a new request for the running item, keeping its times, walk and `moved`. This is the outpost's finish of a cut caret read (`dispatch_finish`, 1471).
- `learned(request, window, now)`: `Unknown` becomes `Known`. Its `next` is the grace's end, or now when the grace has passed, in which case the rule is judged at once.
- `armed(request, bound)`: `Unarmed` becomes `Armed(bound)`. A published walk is never armed again.
- `walk_returned(request, past, now)`:
  - with `past`, an armed walk asks `Publish`;
  - otherwise the walk becomes `Unarmed`, so no bound is timed for a walk that has returned.
  - Only the outpost sends this input. The listener's reader cuts a walk past its bound itself, through the same function the watchdog publishes with.
- `tick(now)`, and the judgment each answer makes again, apply these rules in order, and the first that applies decides. This is the determinism design's order:
  1. **The deadline.** With a walk armed, ask `Publish { then_abandon: Deadline }`; otherwise abandon at once.
  2. **The bound has passed.** Ask `Publish`; the walk becomes `Unarmed` until the answer.
  3. **`Known` with `next` passed.** Set `next` to now plus the grace, and ask `AskMovedOn`. This applies whether or not the walk was published.
- `moved_on(request, answer, now)` applies whenever `request` is running, published or not.
  - When the answer is true and a walk is armed, it asks `Publish { then_abandon: MovedOn }`.
  - When the answer is true otherwise, `Unarmed` or `Published`, it abandons at once.
  - When the answer is false, nothing happens.
- `published(request, got, then_abandon, now)`, when `request` is running:
  - with `got`, the walk becomes `Published`, and the reader applies its publication before anything else;
  - then the item is abandoned if `then_abandon` is set, and otherwise the judgment is made again.
- `end(request) -> Option<(T, published)>`: the item, once, if `request` is running in the generation in charge.
- `succeed() -> generation`: a new generation without an abandonment, for the listener's start of a thread.
- Queries:
  - `accepts(request)`: running, and not `Published`;
  - `generation()`;
  - `running()`, with its published flag;
  - `next_wake()`: the soonest of the deadline, the bound while armed, and `next` while `Known`.

An abandonment takes the running item out, adds one to the generation, and returns `Abandoned { dead, replacement, reason, item, published }`. `reason` is `Deadline` or `MovedOn`, which each reader maps to its own words.

**How each shell drives it.**

- **The outpost's reducer** calls `InFlight` where it judges today.
  - Asks become its `Effect::AskMovedOn`, and `Effect::Publish`, which carries the facts.
  - On an abandonment it emits `Abandon { dead, replacement, reason }` and answers its own obligations: the running query, the owed work of the dead generation, the open watch's poll, and the focus group.
  - A foreground fact no longer judges anything, so the reducer's call at 738 goes.
  - Its shell answers each ask before its next mailbox message, as in the determinism build.
  - The shell alone counts abandoned workers. It adds one as it carries out `Abandon`, and takes one away on `Posted::Returned`, which no longer enters the reducer.
- **The listener's schedule** steps the `InFlight` of the application concerned.
  - `Tick` loses its `reads`.
  - The shell steps four new inputs:
    - `Learned`, from the read's `learn`, in place of `on_learn`'s judgment;
    - `Armed`, from `on_arm`, in place of its alarm call;
    - `MovedOn`;
    - `Published`, with the read and its top-level window.
  - Two new effects ask: `AskMovedOn { pid, request, window }` and `Publish { pid, request }`. A listener step may carry one ask per application.
  - `Claim` becomes `Unwatch { pid }` on an abandonment.
  - `Foreground` keeps only menu currentness and no longer wakes the watchdog.
  - `App::live` becomes `threaded: bool`, and `InFlight` holds the number.
    - A start of a thread takes `succeed()`, so a thread that is ending is never in charge again, as today's fresh number ensures.
    - A returning abandoned thread is reused under the generation its abandonment made.
    - An application's `InFlight` starts again at generation 0 when its `App` is removed and made again. That is safe because removal requires no thread and no abandoned thread left (`schedule.rs` 655 and 1006), under the same lock as every `in_charge`.
- **The listener's shell answers asks after every other effect of the step.**
  - `step` applies the effects in order and sets the asks aside.
  - Once the other effects are applied, it answers each ask in order, as a step of its own, which handles its own effects and asks the same way. The thread's next job, if any, is returned from the outermost step.
  - So every `Outcome` of a step is applied before any answer's step runs, and the owed-marker check at each step's end (`focus_read.rs` 1840 to 1843) holds unchanged.
  - Every ask is answered before the readers' lock is let go.
- **The listener's `Publish`** finds its read by request, not by the application's current entry.
  - It reads the progress, as `prepare_publish` does today, and does not take it, so the reading thread's `take_progress().expect` still finds it.
  - `Walk::Published` prevents a second publication, and `end` refuses the thread's late result. So `WatchedRead`'s claim goes, and no copy replaces it.
- **The listener's alarm** is set from the schedule's state after every step.
  - The schedule keeps `alarm_due`, today the shell's. After each step it computes the soonest of every application's `next_wake` and each limit's end.
  - It emits `Alarm(Set(due))` only when that time differs from `alarm_due`. A `Tick` clears `alarm_due` first, because the timer goes off once and can go off a little before the time due, so whatever is still due is set again.
  - `By` and `Settle` go, and `Now` remains for the shutdown.
  - So a quick read's end moves the alarm back to whatever else is due, or cancels it, even while another application's read is under way. That fixes the wake the review found in the case of e46d4183, and the same pre-existing wake from `on_arm`'s `By`.

### What is heard, and NVDA

NVDA's watchdog (`nvda/source/watchdog.py`, at the pinned 732d1ce2):

- It judges `_shouldRecoverAfterMinTimeout` (206 to 232) once the core has not reported alive for `MIN_CORE_ALIVE_TIMEOUT` (47, 151 to 163), then every 0.5 s until 10 s (133 to 145). It needs no event as evidence.
- On recovery it cancels the main thread's calls (186, 235 to 240).
- A UIA parent step that fails falls back to the window's parent (`nvda/source/NVDAObjects/UIA/__init__.py` 2136 to 2145).

So NVDA speaks a focus whose stuck walk is cancelled, its chain completed by windows. Whether `CoCancelCall` ends a UIA call is M6.1's open question; if it does not, NVDA waits and speaks the focus late. In neither case does it drop the focus.

What the design matches:

- the first judgment at 0.5 s, with no evidence;
- a judgment every 0.5 s after that, whatever the user moved to;
- speaking a stuck focus rather than dropping it.

What it does not match, each by existing decision:

- **The rule itself.** NVDA's rule is any change of the foreground window, or of the focus thread, away from NVDA's focus object. Verbatim's is only another thread of the same application, because each application has its own reader.
- **The deadlines.** NVDA has 10 s. Verbatim has the outpost's per-work deadlines and the listener's 8 s.
- **What happens on recovery.** NVDA cancels the call and goes on with the same thread. Verbatim abandons the worker.
- **When the clock starts.** NVDA's clock is the core's last report alive. Verbatim's is the item's start.

The changes heard:

- **Both readers: the half-second judgment.**
  - A move to the taskbar or the desktop, or to another thread's child window within the same foreground window, now ends a stuck read within 500 ms. Today it waits for the bound, the return, or the 8 s or 10 s deadline, and every later event of the application waits with it.
  - A move that raises a foreground event is now noticed at the next half-second rather than at once, up to 500 ms later than today.
- **The listener: publish first.**
  - When the moved-on rule ends a stuck read, or one late tick passes both the bound and the deadline, an armed walk is published before the read is abandoned.
  - Heard: a stuck focus within its own top-level window (bound 2 s) whose user moves to another thread's window between 0.5 and 2 s is spoken from what was read, then the new focus. Today nothing is said for it.
- **Unchanged:** a published item that is still stuck is abandoned by the moved-on rule, in both readers.

### Every request gets one answer

The item is held by value in `running` and leaves exactly once, by `end` or inside `Abandoned`: the design principles' token rule. Each ask is answered once:

- in the outpost, before the next mailbox message;
- in the listener, after the step's other effects and before its lock is let go.

An answer whose request is not running is dropped. With `then_abandon`, the abandonment is then not made, since its item is gone. A `MovedOn` answer is never dropped for publication.

- **The read returns before its bound.** `end(R)` gives the item, unpublished, and the reader answers it: the listener with its outcome, the outpost with its report. No ask was made.
- **The walk is stuck past its bound.**
  1. `tick` asks `Publish(R)`, and the walk becomes `Unarmed`.
  2. The listener's shell reads the progress and answers `Published(R, got)`. The walk becomes `Published`, and the outcome is sent.
  3. When the call returns, the thread takes its progress as today, and its `Returned` gives `end(R)` with the item marked published, so no second outcome is sent.
  - The outpost does the same with its commit.
- **The walk ends as its bound passes, in the listener.**
  - If the thread's `Returned` takes the lock first, nothing is running at the tick.
  - If the tick takes it first, the thread has already taken its progress at the walk's end. `Published(R, false)` leaves the walk `Unarmed`, and `Returned` then gives the unpublished item, which the thread answers.
- **Published, then the user moves on to the taskbar.** This is the review's first counterexample.
  1. At 2 s, `Publish(R)` is answered `got`, and the walk is `Published`.
  2. At 2.5 s, `next` has passed, so the tick asks `AskMovedOn(R)`. It is answered true.
  3. `moved_on` finds R running and published, so it abandons at once. G dies, G + 1 takes charge, and `Abandoned` carries the item, published. The listener sends no outcome, and the outpost answers its query `Abandoned` once and does not try the group.
  - The application's later events no longer wait for the deadline.
- **The user moves on during a walk.**
  1. The half-second tick asks `AskMovedOn(R)`, answered true.
  2. That asks `Publish(R, then_abandon: MovedOn)`.
  3. The answer publishes, then abandons, as above.
  - G's late results are refused by generation.
- **The deadline passes with no walk armed.** `tick` abandons at once, the item unpublished. The listener answers `Dropped(NOT_IN_TIME)`, and the outpost answers `Abandoned`.
- **Asks from two applications in one listener step.** This is the review's fifth counterexample.
  1. One tick yields `[AskMovedOn(P1), Outcome(P2, WAITED_TOO_LONG)...]`.
  2. P2's outcomes are applied first, so the owed markers match the schedule.
  3. Then `MovedOn(P1)` is stepped, and its own effects are applied before its check.
- **A dead generation's late inputs.** `Learned`, `Armed`, `Returned`, `Commit` and `WalkReturned` of G after its death name a request that is not running, and are ignored. The listener's `Returned` still counts the abandoned thread back, and may reuse it, as today.
- **A thread that ends.** The listener's `next`, with the application ending, clears `threaded`. The next start takes `succeed()`, so the ending thread's own `in_charge` answers no.
- **Shutdown.** The listener's unanswered markers are answered by its `Owed` tokens, and the outpost's queries by Core at its end, both as today. Neither reader ticks once stopping.

### States and lines

**The outpost's reducer**, against the first determinism build:

- fields go from 16 to 13: `generation`, `requests` and `abandoned` leave, and `current` becomes the `InFlight`;
- the program in flight goes from 10 fields with 3 flags to an item of 3 fields with 1 flag, `deferred`;
- inputs go from 17 to 16, and effects from 17 to 16: `Returned` leaves both, and `Abandon` loses `count`;
- the foreground fact's judgment goes;
- about 120 lines fewer.

**The listener's schedule:**

- removed:
  - `Running`, and `TickRead` with its 8 fields;
  - `Claim`, and `moved_on_wanted`;
  - the `foregrounds` counts, `wake_on_foreground` and `next_generation`;
  - the schedule's `grace`, replaced by the shared constant;
  - `AlarmChange::By` and `Settle`;
- `alarm_due` moves in from the shell;
- `App` keeps 8 fields, with `running_place` joining the item;
- inputs go from 10 to 14, and effects from 8 to 10;
- about 150 lines fewer.

**The listener's shell:**

- removed from `WatchedRead`: `claim` with its three values, `front_judged`, `front_elsewhere`, `judge_front`, `mark_claimed` and `claimed`, `bound_at`'s claim test, and `on_learn`'s judgment;
- removed: the watchdog's gathering and `alarm_by`;
- about 70 lines fewer after the answers are added.

**Added:** `InFlight`, with `Walk` and `Moved`, about 250 lines. It has no lifecycle state that was not already in one of the readers.

**Net:** about 90 lines fewer.

**Flags and fields:**

- removed: `grace_judged`, `claimed`, `published`, `front_judged`, `front_elsewhere`, `wake_on_foreground`, and the claim's three values;
- added: one flag, `threaded`, and one field, `next`;
- no lock, shared flag or second writer is added, three atomics leave, and the alarm's due time gets one owner.

### Wake-ups and calls

- **A quick item costs no wake in either reader.**
  - The listener's alarm is set to the read's grace end as it starts, and moved back as it ends: one timer call each.
  - Today the start sets the alarm to the deadline, and the end cancels it only when nothing else is under way. The count is the same, with one more call at an end while another application's read is under way.
- **An item still under way past the grace** costs one wake and one moved-on call every 500 ms until it returns, is published or is abandoned:
  - at most 15 for a listener read (8 s);
  - at most 19 for an outpost program at the 10 s deadline, 3 at 2 s, and none at 400 ms.
  - Today the outpost wakes once at the grace end, and the listener once with evidence. These wakes are paid only on the slow path, for the recovery they buy.
- **Foreground events** no longer wake the listener's watchdog, or run the outpost's judgment.
- **Every listener focus read with a window** loses its moved-on call: `GetGUIThreadInfo`, the two owner reads, and for a console the provider read.
- **A learned window** costs one uncontended readers' lock, and no wake.
- **Measured before and after**, in test audio mode, debug build:
  - wake-ups and processor time per thread on `measure_focus_text_box`, five 12-second trace windows each side, 2026-11-01, per wake of the listener's UIA callback thread, since each window catches a different share of the workload: all processes 22.7 wakes after and 23.3 before (medians), the listener 4.40 after and 4.48 before, so no change was measured;
  - the listener watchdog's wakes in its held-read tests;
  - the timer calls per read go in `docs/performance.md`.

### Tests

**Replay tests of `InFlight`**, in `verbatim-reader-rules`, one per rule:

- the order of the deadline, the bound and the moved-on rule in one late tick;
- publish first, for each reason;
- the half-second judgment, with a window known at the start and with one learned past the grace;
- a true `MovedOn` for a published item, which abandons at once;
- an answer to a dead request, with and without `then_abandon`;
- `walk_returned` before its bound disarming the walk;
- `hand_on` keeping the times;
- `succeed`.

**One generated test**, with `proptest` under a fixed seed, as `verbatim-term-sim` uses it.

- It drives sequences of start, learned, armed, tick, advancing time, walk returned and end, each ask answered at once by a drawn value.
- It compares `InFlight` with a reference model: a scan of one item's timeline that names its one fate from the times and answers alone. The fate is ended, published then ended, or abandoned, with its reason and whether it was published.
- After every step it checks:
  - the item leaves exactly once;
  - each transition makes at most one ask;
  - `accepts` is false after publication or death;
  - the generation rises by one per abandonment;
  - `Published` only follows `Armed`;
  - a published item can still be abandoned for `MovedOn`;
  - when `next_wake` is due, a tick changes the state, so no wake is timed that will not be acted on.

**The readers' tests:**

- The outpost's tests of publication and abandonment change their setup. `a_foreground_fact_after_the_grace_judges_the_moved_on_rule_again` becomes a test of the half-second judgment.
- The listener's tests built on `TickRead` move to the new inputs. `no_grace_end_alarm_without_a_foreground_event` becomes a test that a quick read's end moves the alarm back while another read is under way.
- Two new shell tests, one for each listener rule:
  - each ask is answered once, after the step's other effects, and before the lock is let go;
  - a step with an ask and another application's outcomes keeps the owed markers matched.

**Guards**, all under `cargo xtask ci`:

- mockapp's `a_stuck_parent_step_is_published_at_the_bound_and_its_late_result_discarded`, `an_msaa_focus_whose_walk_is_stuck_is_published_at_its_bound` and `a_focus_published_at_its_bound_is_followed_after_its_worker_is_abandoned`;
- the listener's held-read tests, `a_held_read_is_abandoned_once_the_user_moves_on_past_its_grace` and its two neighbours, which now move on without a foreground event too.

### End-to-end scenarios and the order of work

The scenarios run three times each at the end. Neither is in `docs/flaky-tests.md`:

- `notepad_editing`, for the outpost's `InFlight` on every program;
- `rapid_tabbing_in_settings`, for the listener's on every read.

No scenario reaches a moved-on abandonment or a bound reliably: the `windows_terminal_` and Explorer scenarios that could are listed as flaky. So the mockapp and unit tests hold those paths.

The order of work:

1. `InFlight`, with its replay tests and its generated test, and no reader using it yet.
2. **The outpost adopts it**, and its shell keeps the abandoned count. Its tests move. The half-second judgment changes what is heard here. Scenario: `notepad_editing`.
3. **The listener adopts it**: its schedule, its alarm, its shell's asks and the slimmer `WatchedRead`. Measurements before and after. Scenario: `rapid_tabbing_in_settings`.
4. **The documents:**
   - the crate guides of `verbatim-reader-rules`, `verbatim-outpost-logic`, `verbatim-listener-logic` and `verbatim-listener`;
   - `focus_read`'s module documentation;
   - the focus pipeline rework's paragraphs on the listener's abandonment, on "publish first" and on the moved-on rule;
   - "Simpler owners", whose three items this build makes.

The estimate is three to four days for one builder, with the adversarial review.

### Limitations

- A move that raises a foreground event is noticed up to 500 ms later than today, as in NVDA.
- A stuck item past its grace costs up to two wakes a second until its deadline.
- The listener keeps the abandoned limit and the per-application queue, and the outpost keeps the focus group and the owed work, as the study found.

### Decisions

- The type is `InFlight`, and the section's term is "the work in flight" (Dickson, 2026-11-01).
- The listener speaks what it has read of a stuck focus before giving it up when the user moves on, as the outpost does: publish first, in both readers (Dickson, 2026-11-01).
- Both readers judge whether the user has moved on at the grace's end and every 500 ms after, as NVDA does, instead of only at foreground events (Dickson, 2026-11-01).
