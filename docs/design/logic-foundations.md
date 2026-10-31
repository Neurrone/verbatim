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

The platform-neutral dependency check read normal edges only, did not name `winapi`, and passed a Windows-only workspace crate with no `windows` dependency of its own, such as `verbatim-gui`. It is replaced, in the build's last commit, by a check of the crate tiers (`xtask/src/tiers.rs` (planned)), which reads `cargo metadata` once, over normal and build edges for every target:

- Every workspace member is classed in one list: platform-neutral, logic, Windows, or tool. A member in no class fails. Reason: the hole was a crate the check did not know.
- A platform-neutral crate reaches only platform-neutral workspace crates; a logic crate only platform-neutral and logic ones; neither reaches `windows` or `windows-core` by any path, nor `windows-sys` or `winapi` from a workspace crate other than `workspace-hack`. Reason: the platform-neutral tier may depend on no GPL-tier crate, and third-party crates with target-gated bindings are cross-platform code.
- A role's crates reach no other role's. Reason: the rule of one-way dependencies.

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
- Portable, using the standard library alone: `docs`, `ownership`, `waits` and `shapes`, and in `main.rs` the dependency check `check_platform_neutral_deps` and `ci` itself.

#### Gating rules

- A whole module is gated, never single functions inside it: `#[cfg(windows)] mod conformance;` and so on for each Windows-only module, and `#[cfg(windows)]` on the three Windows functions in `main.rs` and on every import only they use (the `windows` imports and `std::time::Instant`, which only `warm_speech` uses), since an unused import fails Clippy with warnings denied. Reason: one attribute per module is easy to audit, and a Windows module never has to keep a Linux branch compiling.
- Every dependency except `workspace-hack` moves to `[target.'cfg(windows)'.dependencies]` in `xtask/Cargo.toml`, including the portable ones (base64, serde, serde_json, `verbatim-model`, `verbatim-config`, `verbatim-term-sim`), since only Windows-only modules use them. Reason: a dependency lives under the same condition as the code that uses it, so the Linux build compiles nothing it does not run. `workspace-hack` stays unconditional because `cargo hakari manage-deps` requires every package to depend on it.
- The Windows-only commands' match arms in `main` and their lines in the usage text carry `#[cfg(windows)]`, so on Linux the usage lists only `ci` and `docs` and any other command gets the usage and exit code 2. Reason: one dispatch path, with no "not supported here" branch to test.
- Hakari's generated output does not change: its `platforms` in `.config/hakari.toml` are the two Windows triples, on which xtask's dependencies are unchanged. The builder confirms it with `cargo hakari generate --diff`.

#### What the checks assume about Windows

- The documentation check (`xtask/src/docs.rs`) is already portable: it builds paths with `/`, compares every component's letter case against directory listings (so a link that differs only in case fails on both systems), and reads lines on `\n` with trailing whitespace trimmed, so both the CRLF checkout on GitHub's Windows runner and Linux's LF checkout read the same. The repository stores every document and fixture with LF (`git ls-files --eol`).
- The scans (`waits.rs`, `shapes.rs`, `ownership.rs`) are portable: paths are normalised with `replace('\\', "/")` or compared with `Path::ends_with`, files are read with `str::lines`, which drops a trailing `\r`, and Windows names such as `Start-Sleep` or `verbatim_input_windows` are only text they search for.
- The platform-neutral dependency check is the one host-dependent check: `cargo tree` reads the graph for the host's target, so on Linux it would not see a dependency gated on `cfg(windows)` and would pass where Windows fails. Rule: it passes `--target all`, so it reads the same graph on every host. Reason: the check's answer must not depend on where it ran. On today's graph, `cargo tree --target all` for every platform-neutral crate reaches `windows-sys` only (beneath third-party crates and `workspace-hack`, which the check allows) and neither `windows` nor `windows-core`. `cargo tree` reads the graph and builds nothing, so this is not a build naming a target. Its workspace-crate test compares paths with `std::path::MAIN_SEPARATOR`, which is correct on both systems.

### `cargo xtask ci` on Linux

`ci` keeps one entry point, and what it runs depends on the host:

- On every host: the dependency check, the three scans and the documentation check.
- On Windows, as today: the hakari checks, rustfmt, and Clippy and unit tests for the whole workspace.
- On any other host: Clippy (warnings denied, all targets) and unit tests for the crates in a new constant, `LINUX_CRATES`, in `xtask/src/main.rs` beside `PLATFORM_NEUTRAL_CRATES`, each passed as `-p`.

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

- Built before the split: xtask's gating, `--target all` in the dependency check, `ci`'s host-dependent steps with `LINUX_CRATES` holding the platform-neutral tier and `xtask`, the `ci-linux` job, and the `docs` job on Linux.
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

Status: proposed on 2026-10-31, and revised after its adversarial review (`experiments/logic-tests-study/bookkeeping-design-review.md`). The console poll's rule waits on a measurement, a capture and a question for Dickson. The rest does not depend on the answer except where said.

The build removes lifecycle states from the outpost's programs ([logic tests](logic-tests.md), "The outpost's program bookkeeping simplified"). It comes after the crate split: the reducer's changes land in `verbatim-outpost-logic` (`crates/verbatim-outpost-logic/src/`, the outpost's `reducer` module when this section was written), and the shell, worker, inbox, commit and request context stay in `verbatim-outpost`. Paths and lines below are those of the code when this section was written, before the crate split.

### Where the code contradicts the logic-tests design

- Programs already get fresh copies at dispatch: `Reducer::dispatch` (`reducer/mod.rs` line 1438), `dispatch_finish` (1488) and `dispatch_poll` (1507) each clone `self.facts`. The worker keeps nothing between programs but the console poll's carry (`worker.rs` lines 289 to 300, `carry` at 319, `RequestContext::take_program_text` in `outpost/mod.rs` at 570). `judged_focus` (`worker.rs` 4386) reads the program's dispatch-time copy of the reports, not a copy the worker holds. So that part is mostly a consequence of the console poll's rule, plus moving one judgement into the reducer.
- Verdicts already travel alone: `RequestContext::record` (`outpost/mod.rs` 619) posts each verdict as a commit holding nothing else. The exception is in the types: `Effects::verdicts` (`commit.rs` 187), `Change::verdicts` (`reducer/follow.rs` 450), the verdicts branch of `admit` (`shell.rs` 391), and the reducer recording verdicts before it checks whether the commit is accepted (`reducer/mod.rs` 529). The change splits the type and changes no behaviour.
- A program commits more than once, so only its last commit could travel with its end. That part is dropped (part five).
- A test settle and the idle query differ in four ways:
  - A settle waits for the whole intake to empty, later arrivals included (`reducer/mod.rs` 1357 requeues it while `Intake::busy`); the idle query waits only for what came before it.
  - The idle answer waits for an open caret watch and an open gap record; a settle does not.
  - A settle's program waits for the subscription's moves and the outbound writes.
  - The idle query ends its batch; a settle does not.

  The merge keeps one item and one way of reaching it, and answers a test's settle as soon as it is reached (part one).

### Part one: test settles merged into the idle barrier

Today: `Outpost::settle` (`outpost/mod.rs` 1314) posts `Posted::Settle(Sender)`. The shell numbers it in `Settles` (`shell.rs` 247: a `waiting` map and the `in_flight` slot) and pushes `Item::Settle(id)`. The reducer reaches it in `run` (`reducer/mod.rs` 1357), requeues it while the intake is busy, and dispatches `Work::Settle(id)` (137). The shell gives the job the waiter (`Job::settle`, `worker.rs` 101). The program calls `Registration::settle` and `Outbound::flush`, then sends on the waiter (`worker.rs` 416). The shell lets go of the waiter on the program's `Finished`. It also lets go through `Settles::in_charge` (`shell.rs` 791) when the program's generation is dead and the settle was not dispatched again. A settle dispatched behind the console poll is dispatched again if the poll is abandoned, and gets the same waiter; that re-dispatch is where the waiters were lost twice.

Proposed:

- One intake item, `Item::Barrier { asker, behind_noted }`, where `asker` is `Asker::Round(u64)` (Core's idle query) or `Asker::Settle(u64)` (a test's). The item ends a batch, and a noted caret goes just ahead of it (`Intake::push_ahead`), as the idle query does today. Reason: one way of reaching the barrier, one ordering rule.
- A test's settle is answered when the reducer reaches it, with no program in flight: `Effect::Settled(id)`. A round is answered as it is today (`answer_idle`, `reducer/mod.rs` 1188). Reason: `crates/mockapp/tests/caret_watch.rs` settles 58 times, many with a watch open, and expects the settle to return while the watch stays open. With the round's conditions each such settle would wait 10 seconds (`CARET_WATCH_BOUND`) and then hear the watch end.
- The shell keeps the waiters in a map under the settles' numbers. After each transition it asserts that the map's keys are the `Asker::Settle` items waiting in the intake, which the reducer lists from the intake, as the shell already checks its reply tokens. The reducer keeps no set of its own. Under the second and third poll options (part four), a settle is answered in the same transition that reaches it, so the intake's items are exactly the pending settles. Under today's yield a settle reached while the poll is in flight would wait for the poll's end, which needs a list of reached settles in the reducer. Reason: a second record of the same fact would be one more thing to keep equal.
- `Outpost::settle` waits for the answer, then calls `Registration::settle`, `Outbound::flush` and the event thread's `sync` on the caller's thread. Every subscription move and message before the answer was handed to its thread before the shell applied `Settled`, so these calls come after them in their channels. Reason: waiting is the test thread's job, and a reducer thread that waited would stall the outpost.
- Removed: `Work::Settle`, `Item::Settle`, the `Settles` slot and its `in_charge` cleanup, `Job::settle`, the requeue while busy, and the re-dispatch behind the poll. Added: none.

Each request gets one answer:

- Settle `s` is pushed while program `p` is in flight. `p` posts `Finished`, the pump plans the intake, and the reducer reaches `s` and emits `Settled(s)`. The shell removes `s`'s waiter and sends on it. No program runs for the settle, so nothing can strand it.
- `p` is abandoned while `s` waits. `judge_abandonment` starts the replacement and the pump goes on, so `s` is reached and answered as above. A generation's death touches no settle.
- A noted caret is pending when `s` is reached. The caret and `s` are put ahead (`behind_noted`), the caret's program runs, and `s` is reached again and answered. A caret noted by that program is not waited for, as with the round today. Today's settle took noted carets each time it was reached.
- `Close` comes before `s` is reached. `Intake::close` drops the item, and the shell drops every waiter as it takes `Close`, so `Outpost::settle`'s receive fails and it returns, as it does today at shutdown. Dropping the waiter is its answer. A `Posted::Settle` after the mailbox has closed is dropped with its waiter (`shell.rs` 139 to 152).

Behaviour: Core's round is unchanged, so production is unchanged. In tests a settle:

- no longer waits for entries that arrive after it;
- no longer returns early when a worker is abandoned;
- divides batches, so events on either side of it are not merged or reordered across it;
- leaves the objects the shell let go of to be dropped with the next program rather than with the settle's.

Wake-ups and processor time: no change in production, since Core asks idle questions only of the harness. In tests each settle costs one shell wake and no worker wake. Today it costs a shell wake, a worker wake, and a shell wake for its `Finished`.

Guarded by:

- the replay tests `the_idle_answer_waits_for_nothing_that_arrived_after_its_query`, `a_noted_caret_is_read_with_its_events_timing`, `the_idle_answer_waits_for_the_hooks_copies_held_at_the_start`, and `a_gap_read_dispatched_again_after_an_abandoned_poll_still_holds_the_idle_answer`, which becomes "a gap read in flight holds the idle answer";
- a new replay test, "a settle is answered as it is reached, and a close drops it".

The shell tests `a_settle_dispatched_again_keeps_its_waiter` and `an_abandoned_settle_replaced_by_another_lets_its_waiter_go` go with the table. The settle's real path runs in the mockapp suites that settle: `caret_watch.rs`, `call_counts.rs`, `focus_reports.rs`, `msaa_events.rs`, `terminal_reading.rs`, `combo_box.rs`, `remote_ops.rs` and `idle_barrier.rs` under `crates/mockapp/tests/`. The end-to-end scenarios run only the round's path.

### Part two: verdicts posted as a message of their own

Today: a verdict is a `Change::verdicts` entry, posted alone by `RequestContext::record`. `admit` keeps it out of a refused commit, and `Input::Commit` records it before checking `accepts`. A dead generation's verdict is recorded only when it is a timed-out probe or a window's forgetting (`VerdictChange::kept_from_any_generation`).

Proposed:

- `Posted::Verdict { generation, change }` becomes `Input::Verdict { generation, change }`, recorded by `record_verdicts` under the same rule. `Effects` and `Change` lose `verdicts`.
- `Posted::Verdict` is taken after the mailbox closes, as `Commit` is (`Mailbox::post`, `shell.rs` 137 to 156). Reason: a probe that timed out during shutdown is still a fact.
- `admit` refuses whole any commit that is not accepted, and the reducer is given no `Input::Commit` for it. Reason: a refused commit is refused with no exception, so M4.6's model of commits needs no carve-out.
- The program's copy still takes the verdict at once. A publication's context, which runs no program, still keeps the verdict in its copy only.
- Removed: `admit`'s verdicts branch and the verdicts field. Added: one message kind and no state.

Each verdict gets one answer. Worker generation `g` probes window `h`, times out, and posts `Verdict { g, change }`. If `g` is in charge, or the change is kept from any generation, the reducer records it, and `Effect::Verdicts` replaces the mailbox's copy. The program's next commit, request `r`, is applied if `accepts(r)`; if not, it is dropped whole and its objects go to the garbage. A probe committed before its worker's death keeps its mailbox order ahead of the abandonment's effects, as today.

Wake-ups and processor time: unchanged, since the post is the same.

Guarded by: `a_probe_is_recorded_when_committed_and_reaches_the_event_thread_then_the_next_program`, `a_dead_generations_probe_is_not_recorded`, `a_dead_generations_timed_out_probe_is_recorded`, `a_timed_out_probe_committed_before_its_worker_is_abandoned_stays_recorded`, and the shell test `a_refused_commits_text_writes_are_not_made`.

### Part three: the worker keeps nothing between programs

Today the worker keeps the carry described above. A program dispatched behind the console poll was copied before the poll's commits were applied. So the worker applies the poll's kept commits (`RequestContext::keep_commits`, `kept_commits`) to the next program's copies, and hands on the store text the poll held (`ProgramText`). Separately, `judged_focus` decides `entered_in_front` on the worker:

- a foreground change sets it to its window;
- any other focus inherits it from `Held::newest_in(top_level)` on the program's copy;
- with nothing to inherit, it is set when the top-level window is the foreground window as read.

That is a judgement written into the reducer's fact, made on the worker.

Proposed:

- The carry goes with the console poll's slot. Under the second and third options of part four, nothing is dispatched while a poll is in flight; today's yield keeps the carry. Removed: `carry`, `keep_commits`, `kept_commits`, `Program::kept`, `take_program_text`, and `for_program`'s text argument.
- The program reports evidence, not the decision. `reducer::Focus` carries `in_front: InFront`, where `InFront::Foreground(isize)` is a foreground change's window, and `InFront::Read(Option<isize>)` is any other focus's top-level window when it was the foreground window as read. The reducer's rule, in `held.reported` (`follow::record`, `reducer/follow.rs` 841):
  - a foreground change sets `entered_in_front` to its window;
  - any other focus inherits it from the newest report in the same top-level window;
  - otherwise it is set from the read.

  These are today's three cases in today's order. A foreground change is named by its variant, not by its event address, since its address is the same as that of an MSAA focus on the window's client object (`foreground_event`, `worker.rs` 609). The same applies to the publication at a walk's bound (`worker.rs` 4630) and to `Report::Answered`. Reason: the decision becomes a pure function in the logic crate, where replay tests and M4.6's model check it.
- The program's local apply runs the same pure function on its copy, so its later reads agree. It is still judged in two places, from the same evidence and the same function. The two agree at each commit, because the program's copy of the reports equals the reducer's then. The reports are written only by:
  - the program in flight's own commits;
  - the publication at its walk's bound, which claims the program, so none of its later commits is applied;
  - `Input::Released`, which comes only while no program is in flight: `NodesHeld` is reached only with none in flight, and `releasing` holds dispatch until it is done.

  The console poll writes no report.
- What is heard does not change: each of today's three cases gives the same value. The roadmap's follow-up, a foreground change updating older reports in the window, stays an item of its own.
- Cost: the foreground window is read on every non-foreground focus report whose top-level window is known, rather than only when nothing is inherited. This includes the shell thread's publication at a walk's bound. It is a local read with no wake. Its time is not measured; the build's before and after figures cover it.
- Removed: the worker's state between programs. Added: none.

Guarded by:

- `a_programs_text_change_reaches_the_next_programs_copy`;
- `a_walk_under_way_at_its_bound_is_published_and_its_programs_report_refused`;
- `a_release_waits_for_the_program_in_flight_and_names_no_dead_request_live`;
- two new replay tests: "a focus inherits its window's entered-in-front at commit", and "a foreground change after a background focus in the same window sets it".

### Part four: the console poll (a question for Dickson)

Today (`reducer/mod.rs`): the poll is a long program (`Work::Poll`, line 143) in a slot of its own, `Reducer::poll` (398), dispatched when nothing else is ready.

- A program dispatched meanwhile goes into the same worker's inbox behind the poll, its deadline held back (`Current::behind_poll`, 365).
- The poll looks at its inbox before each read (`look`, `worker.rs` 335). It yields to anything but a release's drop (`poll_yields_to`, 328), commits `WatchChange::Yielded`, and is dispatched again afterwards.
- When the poll is abandoned, its watch is taken, its query is answered `Abandoned`, and the program behind it is dispatched again to the replacement (`abandon_poll`, 1042). `poll_finished` (1152) starts that program's deadline.

The states are the second slot, `behind_poll`, the re-dispatch after abandonment, the yield, the worker's held queue and the carry.

NVDA, from the pinned source (`nvda/source/editableText.py`, `_hasCaretMoved`): the caret loop runs on the main thread until its timeout, 100 ms by default. It returns early only for a queued script or a pending focus event. In consoles, caret and text change events neither end it nor are handled during it, since `caretMovementDetectionUsesEvents` is false there.

#### What is known

- `docs/design/key-holding.md` ("The console host's caret poll", Costs) measured, under today's yield, that every watch the first check left open was answered within 8 ms. The answer came from one re-read, or from the check of a console update that ran because the poll yielded. The workload, `measure_keys_conhost`, presses only keys that move the caret: Left and Right over a typed line, Backspace, Escape and typing (`crates/verbatim-e2e/src/scenarios/measure.rs`, `terminal_keys`). So nothing is known of keys that move nothing, which pay the poll's whole 50 ms bound.
- The blink-timer experiment saw the cursor move up to 54 ms after the key, past the bound. A slow move also pays the whole bound, then waits for the blink-timer caret event, 257 to 540 ms after the key.
- How often a program arrives during a poll, and how long it would wait without the yield, is not measured.

#### The order of reads when a character is typed during a poll

Take this sequence in the console host:

1. Left Arrow is pressed at the start of the line. Its first check finds nothing, and the watch `q` opens with a poll.
2. At about 10 ms, "a" is typed. A key that is not held is not made to wait for the watch (`crates/verbatim-core/src/editing.rs`, `WaitingKey::Later`).
3. The console inserts "a", and its caret moves one place right. The console update `U` arrives.

What Core receives depends on the option:

- Today's yield: `U`'s program checks the watch, finds the caret moved, and answers `q` as moved, with the caret after "a", under the key's trace. `U`'s read of the terminal, which holds the "a", is sent as an event under `U`'s own trace. One exception: when a poll read in flight at `U`'s arrival finds the move first, the terminal read goes under the key's trace.
- The poll waits for nothing: `U` waits. The poll's next re-read finds the move and answers `q` the same way. The poll then reads the terminal under the key's trace (`terminal_output(node_id, open.trace, 0)`, `worker.rs` 1235 to 1237), so the "a" first reaches Core as part of the key's result. `U`'s read then finds nothing new. The order changes in every such case, not only in the exception.
- Each read a program: `U` is planned before the next re-read, since a re-read is dispatched only when nothing else is ready. Core receives what it does today, the same exception included.

How Core's terminal reading rules speak the "a" in either order is not established, and the expected speech must come from a capture. The capture needed is NVDA in the console host running cmd at an empty prompt: Left Arrow, then "a" 10 ms later, ten times, and the same after a Left Arrow that moves over typed text. No scenario types within the poll's bound after a key: `conhost_up_typing` types only after the recalled line is heard, and `conhost_typing` is recorded as flaky. A scenario of its own, `conhost_unmoved_key_typing`, would assert that capture's speech. It is built only if the poll waits for nothing is chosen, since under the other options Core receives what it does today.

#### The options

For each: the longest wait for a program that arrives during a poll (a focus, a console update's read, a release), the wake-ups, and what is removed.

- Today's yield.
  - Longest wait: one read in progress, then the program.
  - Wake-ups: the worker once per re-read, plus one or two shell wakes per yield, and the re-dispatch.
  - Removed: nothing. It keeps every state listed above, and part three's carry with it.
- The poll waits for nothing: the poll is an ordinary program in the one slot, and whatever arrives waits for its end.
  - Longest wait: until 50 ms after the key was passed on (`POLL_BOUND`), plus one read. That is paid in full after a key that moves nothing or moves later than the bound, and is otherwise unmeasured.
  - Wake-ups: the worker once per re-read, and none for yields.
  - What Core receives changes, as above.
  - Removed: `Reducer::poll`, `behind_poll`, `abandon_poll`, `poll_finished`, `WatchChange::Yielded`, `look`, `poll_yields_to`, the worker's held queue and the carry.
  - The poll still waits on its inbox's high-resolution timer, and needs a `try_take` after each wait to see that its inbox closed.
- Each read a program: the watch's next read becomes its wake (`wake_for` returns `poll.next` for a polling watch). `Item::Wake` on a polling watch makes one read and commits the answer, the end, or `WatchChange::Read` with the next read's time. A wake taken before that time only sets the wake again, as `Worker::wake` does today. The re-read runs under the text deadline.
  - Longest wait: one read in flight.
  - Wake-ups: three or four per re-read, for the shell's timer, the worker, and the shell for the commit and the end, against one today. A key that moves nothing makes ten re-reads, so 30 to 40 more wakes.
  - Processor time: one more copy of the facts per re-read.
  - What Core receives does not change.
  - Removed: everything the previous option removes, plus the long program and the inbox's timer, so the inbox becomes a plain channel and the worker waits on it with `recv`.

With any option, a read can last until the program's deadline, 10 seconds, since the poll names no window for the moved-on rule. Today a program behind the poll waits the same way.

Each request gets one answer: watch query `q` is opened by a check, and the poll's program `P`, or each re-read program under the third option, runs with `running = q`.

- `P` finds evidence. Its commit carries `q`'s reply and `Answered(q)`, the reducer's `send` clears `running`, and `P` ends.
- `P`'s last read finds nothing. `Ended(q)` ends the watch, and `subscription_effects` sends `q`'s `WatchEnded`.
- Under the third option, a read that finds nothing before the bound commits `Read(q, next)`. That sets the watch's poll and wake, and `q` stays unanswered with the watch open. The idle round waits for it as for any open watch.
- `P` is abandoned at its deadline. A new rule for every program, moved from `abandon_poll`: when the running query is the open watch's, the watch is taken out without a word and the query is answered `Abandoned`, once. `P`'s later commits are refused, since its slot is empty. An ordinary program that took the watch has already removed it from the reducer's facts, so the rule does not fire for it.
- `P` panics. A rule in `finished`, moved from `poll_finished`: when the running query is the open watch's, the watch is ended, which answers `q`, and no `Failed` is sent. Otherwise the query is answered `Failed`, as today. Ending the watch and then sending `Failed` would answer `q` twice, so the order is fixed by this rule.
- A newer key's query arrives. It is planned ahead of any wake, and its check ends a watch still open, as today. A wake left for the ended watch finds no polling watch and does nothing.
- `Close` arrives. The shell lets go of the generation's inbox and `P` returns. Its end is taken as any program's.

Guarded by:

- kept: `the_console_poll_is_dispatched_only_when_nothing_else_is_ready`, `a_newer_keys_request_ends_the_old_watch_and_its_poll`, `the_poll_ends_its_watch_by_its_answer_or_at_its_last_read`, `a_query_is_answered_once_whatever_ends_it`, and `the_poll_ends_when_its_inbox_is_closed` under the second option;
- rewritten: `a_program_is_dispatched_beside_the_poll_its_deadline_starting_when_the_poll_ends` becomes "a program waits for the poll's end" under the second option and "a program goes before the next read" under the third; `an_abandoned_poll_ends_its_watch_and_the_program_behind_it_goes_to_the_replacement` becomes "an abandoned poll answers its watch's query once";
- deleted with their states: `the_poll_is_dispatched_again_after_it_yields`, `the_poll_holds_a_drop_and_goes_on_reading`, `the_poll_yields_to_a_caret_read_and_holds_it`, and `the_polls_text_changes_are_committed_with_its_yield`;
- new: a replay test for a panicked poll, and under the third option "a read before the bound sets the next read's wake".

#### The measurement that settles it

Run it on today's code, in release builds and test audio mode, on the desktop. A builder makes two temporary changes, not committed:

- A debug log line in the reducer for each poll. It gives the time the key was passed on and the poll's `until`. For each program dispatched behind the poll, it gives the program's kind, its offset from the key, and its offset to the poll's yield. For the watch, it gives the time it was answered or ended, and whether a poll re-read or another program's check did it. It also gives the number of re-reads.
- A temporary workload, `measure_keys_conhost_unmoved`, in `crates/verbatim-e2e/src/scenarios/measure.rs`, against Windows PowerShell in the console host as `measure_keys_conhost` uses. Each round presses, each key 150 ms after the speech of the one before:
  - Left Arrow 20 times at the start of an empty line;
  - Home 20 times at the start of the line;
  - Up Arrow 20 times in a fresh shell with no history;
  - Left Arrow at the start of an empty line, then "a" 10 ms later and Backspace, 20 times;
  - as a control, Left and Right Arrow 10 times each over a typed line.

Run `measure_keys_conhost_unmoved` 10 times and `measure_keys_conhost` 10 times, and record each run's time from its log.

From each run, take:

- for each program that arrived during a poll: the time from its arrival to the poll's `until` (its wait if the poll waits for nothing, at most) and to the watch's answer (its wait if the answer comes first), as P50, P99 and maximum, by program kind;
- the share of polls that end at the bound, split into keys that move nothing and keys whose caret moved later. The caret moved when the console's next caret event or the next key's before read shows a new caret position;
- re-reads per poll, as P50, P99 and maximum, which gives the third option's added wake-ups at three or four each;
- the reducer's and the worker's wake-ups per key from a trace, as `docs/tooling/measuring.md` says, in one run of each workload.

#### What it measured

Today's code with the temporary log lines (branch `measure`, ac497e82, not merged), release build, test audio mode, on 2026-10-31: 9 of 10 runs of `measure_keys_conhost_unmoved` and 8 of 10 of `measure_keys_conhost` passed and were read; the three others failed in setup, before any measured key (`docs/flaky-tests.md`, "A new console host not responding at its outpost's first call"). The split of polls ending at the bound into keys that moved nothing and keys that moved late is not in the log lines; the unmoved workload's first three groups move nothing by construction. The wake-ups were not traced; the third option's are estimated at three or four per re-read and are measured in the build.

- Ordinary console keys (`measure_keys_conhost`, 261 polls): 0.4% end at the bound; re-reads per poll P50 0, P99 2, maximum 7. 64 console update reads, the typed character's echo among them, arrived during a poll: today each ran in P50 1.0 ms, maximum 1.5 ms, through the yield; if the poll waited for nothing, each would wait for the poll's bound, P50 49 ms, maximum 50 ms.
- Keys that move nothing (`measure_keys_conhost_unmoved`, 716 polls): 66.5% end at the bound, with re-reads per poll P50 9, maximum 10, so the third option adds about 27 to 40 wake-ups to each such key. 152 console update reads and 160 text queries arrived during a poll: today they ran within 0.5 ms and 1.2 ms; waiting for nothing, up to 50 ms and 43 ms.

So the poll waiting for nothing would add about 50 ms to a typed character's echo in ordinary console typing, while each read as a program keeps today's latency and pays its wake-ups only on keys that move nothing.

### Part five: dropped

Posting a program's last commit with its end is dropped. The hop has not been shown to cost. Posting the end before the program's drops would put a blocked release under the next program's deadline, and would let a settle or the idle round be answered while the worker still calls into the application. If a later wake-up measurement shows the hop costs, the end would carry the last commit and still be posted after the drops.

### Facts with an owner in two places

- The reducer's verdicts and marks, and the mailbox's push-time copies of them: unchanged, each still written only from the reducer's effects.
- Pending settles: the barrier items in the intake, and the shell's waiters, checked against those items after every transition. The waiter is only the means of answering, as a reply token is.
- The console poll's progress, under the second option: the watch in the reducer keeps the poll as it was at dispatch, while the program holds the live one. Nothing reads the reducer's copy while the poll is in flight, since `wake_for` gives no wake for a polling watch, and the program's end replaces the watch. Under the third option the progress lives only in the reducer.
- `entered_in_front`: judged by the same pure function on the program's copy and in the reducer, from the same evidence, and equal at each commit by the argument in part three.

### Measured before and after

Wake-ups and processor time per thread, in release builds and test audio mode, at most 20 runs each:

- the console poll's option on `measure_keys_conhost` and `measure_keys_conhost_unmoved`;
- part three and the build as a whole on `measure_focus_text_box` and `measure_keys_notepad`.

### End-to-end runs and the order of work

The commits, in order, each with its unit and replay tests, and the crate guide updated where an API changes:

- Verdicts as a message of their own. Scenarios: `quick_link_menu`, `menu_and_settings_dialog`.
- The console poll by Dickson's answer, with the worker's carry removed and the one-answer rules for an abandoned or panicked poll. The ownership design's limit on a release waiting behind the poll is updated. Scenarios: `conhost_up_typing`, `conhost_editing`, `conhost_commands`, and `conhost_unmoved_key_typing` if the poll waits for nothing is chosen.
- Test settles merged into the idle barrier. Its gate is the mockapp suites that settle, named in part one, which `cargo xtask ci` runs. Scenarios: `notepad_caret_keys`, `text_box_caret_keys`, for the round's path.
- `entered_in_front` decided by the reducer. Scenarios: `second_application_and_verbatim_menu`, `rapid_tabbing_in_settings`.

Each scenario runs three times at the end of the build. `conhost_typing` is left out, since `docs/flaky-tests.md` records its failure.

### Decisions

- The console poll: each read is a program of its own, timed by the reducer, which removes the poll's slot, the yield, the re-dispatch and the worker's carry, and keeps what Core receives and today's latency; its added wake-ups, on console keys that move nothing, are measured in the build (Dickson, 2026-10-31, on the figures in "What it measured").
- A test's settle is answered as soon as it is reached, without the idle round's waits for an open caret watch, an open gap record or the hook's held copies, and divides batches as the round does (Dickson, 2026-10-31).
