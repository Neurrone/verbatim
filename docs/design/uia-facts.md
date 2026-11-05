# The UIA facts build

This is the design for "The UIA facts build" in `docs/roadmap.md`, the one document a session builds it from. It states the workspace, the folder each fact gets, the provider program, the workflow and the order of work, and, for each folder, the fact, where its evidence is now, the code in `experiments/` it replaces, and what in Verbatim relies on it. It is deleted once the build is finished, since the folders it produces describe and reproduce themselves (Dickson, 2026-11-04). Every path under `uia-facts/`, and `.github/workflows/uia-facts.yml`, is created by the build.

## Scope

Every fact of `books/dev/src/reference/uia-bugs.md` and every claim of `books/dev/src/explanation/windows/uia.md` gets a reproduction, the ones behind workarounds, internal details and statistics included, and so do the explainer's open questions and inferences. These are excluded by Dickson's decisions of 2026-11-04, each left recorded as the documents state it, with no reproduction:

- Verbatim's `conhost_two_windows` inference.
- The inferences about NVDA's exposure: NVDA #7345 and #8539 fitting the shared worker, #11002's one worker per process, and which way #20169's jam happened.
- The client-wide stall before Windows 10 version 1809.
- The applications never measured: the console host's provider, Qt, .NET 8 WPF, Windows Forms and Java.
- NVDA #20654's freeze and its 19.8-second call, which stay in the explainer's "Not known" list.

Qt's stale cached focus (NVDA #20764) and Excel 2016's freeze point to NVDA's record, and Verbatim's own exposure figures from old builds stay as history. Claims about where a provider call runs (`UseComThreading`, STA, WPF's dispatcher, AccessKit, the console host) link their source only; the one exception is WPF's measured hold. Claims documented on Microsoft Learn link the Learn page.

Terms used here:

- A fact binary is the program in a fact's folder that reproduces it and prints its outcome.
- A guide is a README section with debugger steps and the stacks to expect.
- "CI test kept" means a committed mockapp test already checks the fact deterministically. That test stays in `cargo xtask ci` and is not copied.
- The provider program is uia-facts' own UIA and MSAA provider, under "The provider program".

## The workspace

- `uia-facts/` at the repository root is a Cargo workspace of its own. It has its own `[workspace]` table, so Cargo stops there and never treats it as part of the main workspace. The main `Cargo.toml` also adds `exclude = ["uia-facts"]`, so that anyone reading it sees the split. Members: `uia-facts/support`, `uia-facts/provider`, `uia-facts/check-copies` and each fact folder that has a binary, listed explicitly. A folder holding only a guide or a kept CI test has no manifest, so a glob would not match the member list.
- `uia-facts/Cargo.toml` contains:
  - `[workspace.package]`: edition 2024, `publish = false`, `license = "GPL-3.0-or-later"`.
  - `[workspace.dependencies]`:
    - `verbatim-uia = { path = "../crates/verbatim-uia" }`, `verbatim-uia-rops = { path = "../crates/verbatim-uia-rops" }` and `workspace-hack = { path = "../crates/workspace-hack" }`.
    - `windows`, `windows-core`, `windows-collections` and `windows-future`, with the versions and feature lists copied exactly from the main root's table.
    - `toml`, at the main root's version, for the copy check alone.
    - No other third-party crate.
  - `[workspace.lints]`, copied from the main root: `missing_docs`; `unexpected_cfgs` with its `check-cfg = ["cfg(coverage)"]`; Clippy `pedantic`; `undocumented_unsafe_blocks`; `multiple_unsafe_ops_per_block`.
  - `[profile.dev]`, copied from the main root (`debug = "line-tables-only"`). The root's per-package overrides (`rubato`, `verbatim-term-sim`) name crates outside uia-facts' graph, so they change no unit it builds and are not copied.

  Cargo has no way to inherit `[workspace.dependencies]`, `[workspace.lints]` or profiles across workspaces. A path dependency inherits from its own workspace, but the dependent workspace's own tables cannot. So the tables are copied, and checked.
- The copy check: `uia-facts/check-copies`, a small member binary. It reads both root manifests with `toml` and fails when any of these differ: the windows-family entries (version, `default-features` and the feature list); `[workspace.lints]`; `[profile.dev]`. It also checks the two lock files (see the lock-file rule) and the two workflow files' top-level `env` blocks (see "The separate workflow"). The workflow runs it before the build, and the rerun script runs it locally. Why the copies must match is under "Reusing the main build".
- The path dependencies resolve their own `workspace = true` keys against the main root. Cargo allows this: a path dependency inherits from the workspace it belongs to. So `verbatim-uia` builds with the same dependencies and features in both workspaces. Its `faults` feature stays off, as in `cargo xtask ci`.
- The provider program (Dickson, 2026-11-04): uia-facts has its own, `uia-facts/provider`, a binary member of the uia-facts workspace.
  - It is built by uia-facts' own `cargo build` into the same `target/debug` as the fact binaries. Each fact binary launches it from its own executable's folder (`std::env::current_exe()`'s parent), so a fact always runs against the provider built with it.
  - No fact binary launches mockapp. The facts kept as mockapp CI tests stay in mockapp, unchanged.
  - The provider implements its UIA and MSAA interfaces with the `windows` crate's `#[implement]`, using the root's feature list. windows-bindgen is not used (Dickson, 2026-11-04).
- The build command: `uia-facts/build.ps1`, locally and in the workflow, runs three steps in order:
  1. It copies `../Cargo.lock` to `uia-facts/Cargo.lock` and runs `cargo metadata` there. Every later cargo step runs with `--locked` (the lock-file rule).
  2. It runs `check-copies`.
  3. It runs `cargo build --locked` in `uia-facts/`, which builds the provider, the fact binaries and `check-copies` together.

  No fact binary calls cargo, so no binary waits on a build lock while it measures.
- Tier: outside `xtask/src/tiers.rs`. The tier check reads the main workspace's `cargo metadata`, and `uia-facts` is not a member of that workspace. Classed by its content, it would be a tool crate, a tier that no rule constrains. It depends only on Windows-tier crates and is never a dependency, so nothing the tier rules protect can reach it.
- Licensing: GPL-3.0-or-later, because it links the GPL-tier `verbatim-uia`. Its code is original: no NVDA source is ported. An NVDA figure is cited as NVDA's record, not reproduced.
- Target directory: `uia-facts/.cargo/config.toml` sets `[build] target-dir = "../target"`. Cargo reads a relative path in a config file against the folder that holds `.cargo`, so the path names the main `target/`. Cargo also merges the repository's own `.cargo/config.toml`, which holds only aliases. `rust-toolchain.toml` at the root applies too, so both workspaces use 1.99.0.

### Reusing the main build

What Cargo shares between two workspaces in one target directory, and why:

- Third-party crates are reused, `windows` above all. A unit's `-C metadata` hash, which also names its file and its fingerprint directory, is computed from the package id (for a registry crate its name, version and registry URL), its features, the profile, the target and rustc version, and its dependencies' hashes. All of these are equal for a registry crate when these inputs match:
  - Versions: `uia-facts/Cargo.lock` is derived from the main lock (the rule below).
  - Features: these come from the union over the packages being built. For the windows-rs family, which `.config/hakari.toml` leaves out of workspace-hack, the features are the one list in the main root. That list arrives through `verbatim-uia`'s inherited `windows.workspace = true`, and uia-facts' copied table is the same list. For every other crate, workspace-hack (the generated crate that unifies third-party features across the main workspace) fixes the features.
  - The profile: copied from the main root.
  - Rustflags: the same config files apply to both workspaces.
- Eight workspace path crates build once more for uia-facts: `verbatim-model`, `verbatim-text`, `verbatim-core` and `verbatim-reader-rules` (which `verbatim-win32` depends on), `verbatim-win32`, `verbatim-uia`, `verbatim-uia-rops` and `workspace-hack`. The stable hash of a path package's id is its path relative to the workspace root when the package lies inside that root, and its absolute URL otherwise. So these units get hashes and files that differ from the main workspace's. The two builds never overwrite each other's fingerprints, so neither keeps rebuilding the other's units: each keeps its own copy.
- The lock: `target/debug/.cargo-lock` is the one build lock. A uia-facts build and a main build in the same target wait for each other ("Blocking waiting for file lock") rather than running together. This is acceptable for runs made by hand. A builder subagent working in its own worktree has its own `target`.
- Lock-file rule: `uia-facts/Cargo.lock` is ignored by git and never committed. Every build of uia-facts, by `build.ps1` locally and in the workflow, first copies the main root's `Cargo.lock` over it. It then runs `cargo metadata --format-version 1` in `uia-facts/`, without `--locked`, which writes the resolved lock. No `cargo update` is run.
  - Locked versions are kept. Cargo's resolver reference ("Lock file", "Dependency updates") says Cargo "gives the highest priority to versions contained in the `Cargo.lock` file" and keeps dependencies "locked to the versions in `Cargo.lock` if it can". A locked entry is replaced only when it no longer matches a requirement, and uia-facts' requirements are the main root's.
  - New entries are only the uia-facts members, since uia-facts brings no third-party crate the main lock lacks.
  - Pruning is not stated in that reference. Cargo writes the lock from the resolve, so entries the graph does not use are dropped. The first build confirms it by diffing the copy before and after, and records the result in `uia-facts/readme.md`.
  - After that step, `check-copies`, the build and Clippy run with `--locked`. The `cargo build` reference says `--locked` exits with an error when "Cargo attempted to change the lock file due to a different dependency resolution", so any later re-resolution fails the build rather than building a second `windows`. The step before cannot take `--locked`, because its pruning is itself a change to the lock.
  - So no one reseeds the lock, and it cannot drift from main's.
  - `check-copies` compares the resulting set with the main lock by (name, version, source) for every package in both, and fails on any difference.
- What still rebuilds:
  - the eight path crates above and every fact package, on the first build and whenever their sources change;
  - any third-party crate if a copy rule is broken: a `windows` feature that differs, or a profile key that differs. `check-copies` fails the build first, but an unchecked edit would add a second `windows` build to `target`.
  - `cargo clean` in the main tree removes both builds.

## The folder per fact

- Name rule: the fact stated as a present-tense sentence, in lower case, words joined by hyphens, with apostrophes dropped. This is the rule `experiments/` uses, and where the fact is a heading of `books/dev/src/reference/uia-bugs.md`, the folder name is that heading, so a citation can be read across from one to the other. The folder is also the package name and the binary's name.
- Index: `uia-facts/readme.md` lists every folder by the section of `books/dev/src/reference/uia-bugs.md` or `books/dev/src/explanation/windows/uia.md` it reproduces, with its kind. A second table, "Cited, not reproduced", lists each claim that links its source instead and gives the link. That table holds the Learn-documented claims, the framework source declarations, NVDA's issues, Qt's stale focus and Excel 2016's freeze, and the excluded items under "Scope".
- README sections, in order:
  - Fact: one paragraph, matching the document's statement.
  - What Verbatim relies on: the code and decisions, as listed for the folder below.
  - Method: the setup, each case, its fixed target (a configuration of the provider program, or a named application at a recorded version) and the one API path it calls.
  - Recorded results: one table row per figure, giving the figure, its date, the machine, its source and its threshold.
    - The machine: Windows build with its revision number, the file version of `UIAutomationCore.dll`, the processor, and the version of each application used.
    - The first row of each figure is the original experiment's; after that comes a row for the first uia-facts run, with its date, commit and outcome.
  - Client settings: whether the client settings change the folder's outcomes, by the one-time check below.
  - How to rerun: the exact commands, and what the run needs (an unlocked desktop, an application, cdb).
  - Debugger guide, when there is one: the cdb path and symbol setup; the binary's `--pause` flag, which prints each process id and waits for Enter so a debugger can attach; the breakpoints and commands to use; the stacks to expect, as a list of frames; how to tell whether the result matches.
  - Evidence kept: the raw outputs under the folder's `recorded/` that a figure cites. The original outputs move with the fact, and the rest are deleted.
- A kept CI test's README says "Rerun: `cargo test -p mockapp --test <binary> -- --exact <name>`" in place of a binary. When the folder also has a binary, the test checks the deterministic part and the binary the rest. The two never check the same quantity.
- The client settings (Dickson, 2026-11-05): support's shared client code takes two flags every program accepts, `--recovery`, which turns the client objects' connection recovery on, and `--coalescing`, which turns their event coalescing on (`IUIAutomation6`). Both are off by default, and no program makes a client object with other settings; a second client process started through a role is given the same flags. Each build checks once which of its facts the settings affect: it runs each case that uses a UIA client by default, with both flags off, then once with `--recovery` alone, once with `--coalescing` alone and once with both, and compares. Every README has a "Client settings" section. Where neither setting changes an outcome, it says so in one line with the date of the check, and the recorded results are the default run's. Where a setting changes it, the section records the default outcome and that of each combination that changes it, with the exact command for each; the case's judged claim is the one the documents state, and its command carries the flag, and `books/dev/src/reference/uia-bugs.md` and `books/dev/src/explanation/windows/uia.md` name the setting where a claim depends on it.
- Output: each binary prints a header (date, Windows build, `UIAutomationCore.dll` version, commit, and the client settings in use), then one line per claim, for example `connection wait: observed 2,183 ms (1 trial); recorded 2.0 to 2.2 s, band 2,000 to 2,350 ms: pass`. A figure that is printed but not judged is marked "recorded for trends". Its exit code is 0 when every judged claim passes, 1 when any differs, and 2 when none differs but some are inconclusive.
- Progress and evidence (Dickson, 2026-11-05), through support so that every program shares them, the claim lines, header and exit codes unchanged:
  - Before each case, one line in plain words saying what it sets up and does, for example `case e57: registering a focus handler while the provider's UI thread is stalled; releasing 200 ms after the call returns; raising a focus event every 50 ms`.
  - Before each deliberate wait of a second or more, a pause or a call left to wait on a timeout, one line naming it, for example `waiting: up to 2.0 s, the registration's connection timeout`, so that a long run does not look hung. A second client process writes these to its standard error, which the run shares.
  - Under a claim whose verdict is differ or inconclusive, the evidence the case kept: each timed call's outcome and time, each line a second client process reported (a trial's events lost and heard, with their times), and the figures each case records. A clean run prints none of it.
  - `--verbose`, off by default, prints as they happen each traced UIA call with its time and result, the provider program's call and message log, and each trial's figures.
- Run time (Dickson, 2026-11-05): a build reports each program whose default run takes over 10 s where its facts need no time to pass (no timeout, stall or deliberate pause in a claim), or over 30 s where they use timeouts or stalls, with its time and what the time goes to.
- Verdicts:
  - A deterministic outcome (an HRESULT, a count of provider calls, delivered or not, an order of answers): pass when it equals the recorded value, differ when it does not. The binary runs the case once by default (Dickson, 2026-11-05); `--trials n` runs it n times, and all n must agree, or the verdict is inconclusive.
  - A relation, for every duration that depends on the machine. Examples are MSAA's 1.2 to 2.6 ms, the other UI thread's 0.4 to 1.2 ms, Edge's 222 ms against 220 ms, `FindAll`'s 0.4 ms median and 696 ms, the 2.6 ms and 228 ms holds, the sent-message ratios, the parallel gains, the second process's 0.7 to 2.7 s, and the flood's counts. These are judged by the relation the claim states, and the figure is only printed, as `books/dev/src/reference/testing.md` asks (no tolerances; a figure that varies by machine is recorded for trends). Each relation is an order or a share of evidence the binary holds:
    - "A answers while B waits": A's reply arrives before B's provider is released, which the binary itself signals.
    - "B waits behind A": B's reply comes after A's provider call returns, by the provider's own call log.
    - "at most about 2 times": the speed-up stays below the serial-share bound. The binary computes that bound from the same run's measured I/O-thread and UI-thread share, so the bound moves with the machine.
    - "late": the second process's delivery comes after the first's.
    - "stops": no event is delivered between the flood's end and 90 s later, while the other client process receives all of its events.
  - A band, only where the figure is the claim itself and comes from a value UIA sets rather than from processor speed. Each band holds across machines because it is set by a timeout and the system timer's 15.6 ms default tick, not by machine speed. Those figures are: a timeout's length (2.0 to 2.2 s, 5.5 s, 22.1 s, E-TT's single reads of 1,082 to 1,095 ms at a 1 s timeout); the sums of timeouts (E-TT's 13.2 s, E3's 22,111 ms, the stall sweep's 4,207 ms); the hold of twice the connection timeout (3,806 to 3,821 ms); `h5`'s 10,013 ms and 508 ms; the registration's 1,089 ms.

    The band is computed from the timeout T the case sets: from T less one system tick (15.6 ms, since a wait can end up to a tick early: a 2,000 ms connection timeout once failed after 1,999.3 ms) to 1.15 times T, plus 50 ms, for a single wait, and the sum of those for a sum. The 1.15 covers the measured 1.1 times wait. The binary runs it once by default, and n times under `--trials n`: pass when every trial lies inside the band, differ when every one lies outside it, inconclusive otherwise. The provider-call count of 47 to 49 per focus event is a count, not a time; it is judged against 40 to 60, since the count varies per event.
  - A rate (k of n, recorded rate p): the binary runs the recorded n, which `--trials` does not change.
    - With zero occurrences: differ when (1 - p) to the power n is below 0.05, since a run that long would have seen the fault, and inconclusive otherwise. Zero occurrences never pass.
    - With at least one occurrence: compute a 95 percent Wilson interval for the observed and for the recorded rate. Pass when the recorded rate lies inside the observed interval, differ when the two intervals do not overlap, inconclusive otherwise.
  - An open question (Not known, or inferred with nothing recorded): the binary prints its answer with "new". The first run's answer goes into the README as the recorded result, and later runs judge against it by the rules above.
- Fixed target and code path, as `books/dev/src/reference/testing.md` asks, where it applies:
  - Each case names one target and one API sequence, and nothing branches on the environment.
  - A missing precondition (an application not installed, a locked desktop, cdb absent) prints inconclusive with the reason. It never passes and never takes another path.
  - Waits are for evidence (a provider's signal, an event, a reply). The one exception is the length of a deliberate stall, since some facts are about a timed stall. The README states which.
  - No substring matching: outcomes are compared as values.

## The folders

There are 34 folders:

| Kind | Folders |
|---|---|
| Binary only | 13 |
| Binary and a debugger guide | 6 |
| A kept CI test and a binary | 10 |
| A guide only | 3 |
| A kept CI test only | 1 |
| A guide, a binary and a kept CI test | 1 |

### Where the evidence is now

None of the old tools was built or run for this design, so "rerun by command" below means the instructions and code are in the repository, not that they still build on main. Only the committed tests compare their outcome with a recorded result; every other tool's output is compared by hand. The tools named in the folder entries are these:

- The parallel-calls scripts: the PowerShell and Python scripts of `experiments/parallel-uia-calls/`, run after copying its `Cargo.toml.txt` to a scratch folder as `Cargo.toml` with absolute paths and building it with `cargo build --release`. They set `PUIA_SCRATCH`, and the debugger scripts need cdb. The program depends on the current `verbatim-uia-rops` by path, so it may need porting to main.
- The hung-window focusproto: `experiments/a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/focusproto/`, copied to a `focusproto` crate under `crates` in a scratch worktree, built with mockapp, and driven by the folder's Python scripts with `BIN` set in `proto.py`. It was built at 01254292 and runs on its own desktop. The focus-limitation, elements and desktop-wide folders use the same tool from `experiments/focus-events-cannot-be-limited-to-one-application/focusproto/`.
- The 2026-11-01 series: the standalone focusproto under `experiments/a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/2026-11-01/`, with its `vbmap.patch` applied. Its scripts need an unlocked desktop, some a Notepad titled `notes.txt`, and the `calls_*` and `threads_*` scripts need cdb.
- evprobe: `experiments/a-flood-of-uia-events-from-one-application-stops-all-of-a-processs-uia-events/evprobe/`, a standalone build against mockapp at or after cb90020e; `evc.py` has cdb's path written in.
- The October experiments: `experiments/mockapp-experiments-2026-10-10/experiments.rs`, run in a worktree at b0a1989 with `cargo test -p mockapp --test experiments -- <name> --nocapture`. Their figures depend on that commit's 10-second connection timeout.
- The collide tool: the patches in `experiments/a-new-connection-fails-while-another-processs-remote-operation-is-queued/`, `collide-tool-e33.patch` applied onto 32948a4d in a scratch worktree and run with `collide run` on a script. The earlier patches' base commits are not recorded.
- The mockapp tests: the `uia_bugs` binary's tests run with `cargo test -p mockapp --test uia_bugs -- --exact <name>`, and the others with their own binary in place of `uia_bugs`.

### a-new-connection-fails-while-another-processs-remote-operation-is-queued

- Kind: binary and a debugger guide.
- Reproduces: the collision (`e1` to `e36`): the proxy answer after 2 s and the "no provider" answer after 3 s; the walk up; the whole process exposed; the spared set (`e10`, `e12`, `e23`, `e25`, `e27`, `e29`, `e34`); the delay to other clients (`e28`, `e28c`). The conhost case (`e35`, the roadmap's "console host and the collision"). The second form, 3 of 42, as a rate. `GetFocusedElement` during a queued operation (Not known). The predicting-outcomes case. Guide: the collision's mechanism under cdb (Not known).
- Evidence now: the collide tool with its scripts and outputs. The second form is in `experiments/two-new-connections-beside-a-pending-call/`, its program `experiments/cross-process-calls-study-program/xproc.rs` run with `XP_EXP=second_form` after applying `scratch-support.patch` at b15ad7bf; its listener case calls an outpost test API that has changed since. The mockapp-stall test of 20 of 20 at timeouts of 500 to 5,000 ms is prose only. The Notepad and Windows Terminal launch rates (1 in 25, 3 in 100, the 3,008 ms probe) needed a Verbatim build from before Verbatim kept its own calls apart, and stay as history.
- Replaces: the collide tool's patches and scripts; `two-new-connections-beside-a-pending-call/`; `xproc.rs`'s `second_form` case.
- Verbatim relies on: the rule that only an application's own outpost connects to it or sends it a remote operation (`books/dev/src/explanation/design/listener-without-connections.md`, section 2; the comment in `crates/verbatim-uia-rops/src/operation.rs`). If the fault went away the rule is merely unneeded; if the spared set shrank, Verbatim breaks, since the outpost makes both remote operations and lookups into its own application (`e12`), and the listener reads held elements while outposts run operations. The second form is accepted in decision 3 of the same design.

### a-pending-connection-holds-its-own-processs-other-calls

- Kind: binary and a debugger guide.
- Reproduces: the pending connection holds its own process's calls and no other process's. Guide: which releases held the I/O thread (Not known).
- Evidence now: the parallel-calls `run-abandon.ps1`, and the collide tool's pending case.
- Replaces: `run-abandon.ps1`'s pending case; collide's pending case.
- Verbatim relies on: D13's listener making no connections, and `crates/verbatim-uia/src/bounded.rs` and the outpost watchdog abandoning threads. If a stuck application held calls from another process, the isolation would fail.

### a-new-connection-to-a-stuck-application-times-out

- Kind: binary.
- Reproduces: `UIA_E_TIMEOUT` after 2.0 to 2.2 s to any window, an idle UI thread included. A lookup with the provider description cached shows a proxy-only answer or the timeout, printed per stall kind.
- Evidence now: the parallel-calls `run-mockapp.ps1`, its suspended cases. The proxy-only answer is recorded for the collision and the timeout for the parallel-calls stall, which are different stalls.
- Replaces: `run-mockapp.ps1`'s suspended cases.
- Verbatim relies on: the proxy rule (`books/dev/src/reference/crates/verbatim-uia.md`), which refuses a proxy-only answer by reading the cached provider description, and the focus-now answer at an outpost's start (`books/dev/src/explanation/design/listener-without-connections.md`, decision 8). If the description's form changed, a proxy's answer would be taken as the application's own and read as "pane".

### uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler

- Kind: binary.
- Reproduces: `e37` to `e39` as rates, conhost started directly. The 5 s wait on a window that does not pump (`docs/parity/backends.md`). The 3 s "no" for a starting Notepad (`docs/nvda/uia.md`). File Explorer's `DirectUIHWND` probes of about 3 s.
- Evidence now: the collide patches for `e37` to `e39`, built in a scratch worktree of f9fb32f7 and run with `run-e3N.py`, which takes the foreground, with their summaries. The 5 s wait is in `experiments/focus-pipeline-rework-measurements/server-side-provider-under-stall.txt`, from `programs.rs` with `debug.patch` onto 63b62397. The Notepad and File Explorer figures have no record kept.
- Replaces: the collide patches for `e37` to `e39` and their scripts; the three programs of `experiments/focus-pipeline-rework-measurements/programs.rs`.
- Verbatim relies on: nothing in its code, since the raw probe replaced the call; Verbatim's workaround is right whether or not the fault remains.

### the-raw-probe-answers-0x10000-for-a-window-with-a-uia-provider

- Kind: a kept CI test and a binary.
- Reproduces: the test `arbitration` (mockapp's two backends). Beyond the test, the binary covers real providers, which mockapp cannot stand for. It compares the raw probe's answer and `UiaHasServerSideProvider`'s on named targets, each with its expected answer. With a provider: the provider program's window, Notepad's edit window, File Explorer's `DirectUIHWND`, and Windows Terminal 1.24's window. Without one: a plain Win32 window the binary creates, and `conhost.exe`'s `ConsoleWindowClass` window started directly. A target that answers other than expected, or a disagreement between the two probes, is differ. The 88-window agreement stays the recorded history of an unnamed sweep.
- Evidence now: `cargo test -p mockapp --test arbitration`; the 88-window sweep by `e39-classify.ps1`, whose answer depended on the applications open then.
- Replaces: `e39-classify.ps1`, `e39-raw*`.
- Verbatim relies on: `verbatim_uia::raw_probe` (`crates/verbatim-uia/src/probe.rs`), which sends `SendMessageTimeoutW(WM_GETOBJECT, 0, UiaRootObjectId)` and takes exactly `0x10000` as a UIA provider, and every probe the listener and the outposts make (`books/dev/src/explanation/design/listener-without-connections.md`, decision 2). Microsoft documents no meaning for the answer. If the value changed, every UIA window would be judged MSAA, or the reverse, and read through the wrong backend.

### a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events

- Kind: a kept CI test and a binary.
- Reproduces: the test `a_provider_held_raising_its_event_holds_only_its_own_events`, which covers E-delivery's case, so the binary leaves that case out. Beyond the test, the binary covers the hold itself, which no test can reach: it needs a window that names the messages it is sent, which the provider program has. Cases:
  - `h1` to `h6`; the hold of 2 times the connection timeout (3,806 to 3,821 ms) and of 1 timeout after the hung mark;
  - no hold without a focus registration; MSAA hooks, explicit reads and the open-and-show route not held;
  - apartments; client variants (two client objects share the worker); recovery and coalescing;
  - P4 (`N_B`); E-hung-proxy, `e_mapping`, `e_hung_cause`, `e_cross`;
  - the older `CUIAutomation` waiting (`h2_cui`);
  - `IsHungAppWindow`'s mark at 2 to 6.5 s (`ghost`, `h7`, unlocked desktop);
  - `q1`, `q3`;
  - `states`' stand-in client with Verbatim's client settings, against a hidden and against a shown window (3,804 to 3,827 ms);
  - the UIA side of `both` and `altf`: on the unlocked desktop, Notepad's menu focus after Alt+F reaches a stand-in client by the open-and-show route during a hold, while another event is held.

  The figures for Verbatim's own listener in `states`, `both` and `altf` are Verbatim's exposure, kept as history (see "Decisions Dickson owns").
- Evidence now: the hung-window focusproto with `h1_stacks.py`, `h3_variants.py`, `h4_ishung.py`, `h6_lost.py`, `h2_trace.py` and the `p4/` scripts; the 2026-11-01 series for `ghost`, `h7`, `q1`, `q3`, `states`, `both` and `altf`; E-delivery, E-hung-proxy, `e_mapping`, `e_hung_cause` and `e_cross` in the October experiments. The listener's figures needed a running Verbatim's log, and the 7.8 s case a mockapp kept running all evening.
- Replaces: the focusproto of the hung-window folder and its `h*.py`, `p4/`; the series of 2026-11-01; `experiments.rs`'s E-* tests on this fault.
- Verbatim relies on: outposts holding no focus registration, so that a hang holds only the listener (`books/dev/src/explanation/design/focus-pipeline.md`, section 12.3; the module comment of `crates/verbatim-listener/src/listener.rs`); D9's amended reason; the 2-second `CONNECTION_TIMEOUT`; the listener dropping a focus in a hung window (`window_is_hung` in `crates/verbatim-listener/src/focus_read.rs`), which rests on `IsHungAppWindow`'s mark. If a process with no focus registration began to be held, every outpost's events would stop during any hang; if the hold lengthened, the listener's silence would exceed what was accepted. Verbatim uses `CUIAutomation8` only, so the older coclass's wait affects nothing.

### uias-client-threads-and-focus-tracking-calls

- Kind: guide.
- Reproduces: function names from the public symbols. The client's WinEvent, I/O and `NavigationHelperProxyOneCore` threads, and the work-item thread created at the first event (`threads_name`, `threads_desk`). Focus tracking's `SendMessageTimeoutW` flags (`SMTO_ABORTIFHUNG` and `SMTO_ERRORONEXIT`, from `HwndUtils::SendWmGetObject`) and `UiaServerConnection_HookMessage` (`calls_focus`, `calls_reg`). `ElementFromHandle` sends the same way (inferred; a new trace of `ElementFromHandle`). The open-and-show route's thread-pool function (`route1`, `route2`). Event work on one worker (`h1` dumps). The `NavigationHelperProxyOneCore` thread's work (Not known).
- Evidence now: the 2026-11-01 series' cdb scripts with `focus.py`, `focus2.py` and `focusthr.py`; the hung-window focusproto's `h1_stacks.py`.
- Replaces: the `2026-11-01/` cdb scripts and `focus*.py`.
- Verbatim relies on: no code; the observable outcomes are checked by the hung-window and flood folders.

### a-client-processs-events-run-on-one-worker

- Kind: binary.
- Reproduces: the worker shared by any number of client objects (inferred; 1 to 8 objects, one held). Whether the worker ends when its queue is empty (inferred), from thread ids across idle gaps. Four events on four thread ids. A slow handler delaying later events and a slow query in a handler (inferred). Whether UIA drops events behind a slow handler (Not known). Handlers called on a non-UI thread.
- Evidence now: the hung-window focusproto's `h3_variants.py` (two client objects); the events case of the parallel-calls `ops.ps1` (four thread ids). The rest has no record.
- Replaces: parts of `ops.ps1`'s events case and `h3_variants.py`.
- Verbatim relies on: D9 as amended on 2026-11-02 (`books/dev/src/explanation/architecture.md`), whose stated reason is the shared worker and the flood, and each outpost holding its own application's registrations. If the worker became per client object, D9's reason would be wrong.

### another-clients-stuck-call-holds-an-events-cache-fetch

- Kind: binary.
- Reproduces: another client's stuck call holds the worker's cache fetch and every later event (inferred). Whether the transaction timeout bounds the fetch (Not known), timed at transaction timeouts of 1 s and 5 s.
- Evidence now: none; the claim is an inference.
- Replaces: none (new).
- Verbatim relies on: nothing recorded; the explainer's worker model.

### a-flood-of-uia-events-from-one-application-stops-all-of-a-processs-uia-events

- Kind: binary and a debugger guide.
- Reproduces: `q2`, `q2_thr`, `q2long`, judged by relation with the counts printed for trends: delivery stops (83 to 110 events, then none for 90 s); the other client process unaffected; text with coalescing on delivered (2,529, 2,641 of 30,000); losses at 2,000 to 5,000 a second. Whether coalescing dropped those (Not known). The worker's work grows with the backlog (Not known), timed as the worker's processor time at 4 backlog sizes. Selective registration under a flood (inferred). Guide: the work-item thread's dumps.
- Evidence now: evprobe with `q2.py`, `q2long.py` and `evc.py` and their outputs. mockapp's flood command is tested by `uia_flood_raises_each_name_change` in mockapp's `events` tests, which checks the command, not the fault.
- Replaces: `evprobe`, `q2.py`, `q2long.py`, `evc.py`.
- Verbatim relies on: D9 as amended on 2026-11-02 (`books/dev/src/explanation/architecture.md`) and each outpost holding its own application's registrations. If the flood stopped, D9's reason would be wrong; if a flood began stopping other processes, the per-application isolation would fail.

### desktop-wide-focus-handlers-in-a-second-process-deliver-late

- Kind: binary.
- Reproduces: `p2b`, `p2c`, `p2d` as rates and lateness of 0.7 to 2.7 s. Cases that vary the number of processes, the registration order and the client threading, toward the cause (Not known).
- Evidence now: the hung-window focusproto with `p2c_desk.py`, `p2c_sum.py` and `p2d_real_win32.py`.
- Replaces: the `p2b`, `p2c`, `p2d` scripts, focusproto.
- Verbatim relies on: nothing; Verbatim holds one desktop-wide focus handler.

### a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all

- Kind: binary.
- Reproduces: fresh provider processes, Notepad and Settings, each started 20 times under one desktop-wide handler. The binary counts late and missing focus as a rate.
- Evidence now: none kept. The fault was met in end-to-end scenarios no longer in the suite and in `settings_system_page`, which does not report it. The recovery is tested by the `a_winevent_focus_` tests of `crates/mockapp/tests/focus_reports.rs`.
- Replaces: none kept (new).
- Verbatim relies on: the recovery, not a workaround: the application's outpost waits a second from the `WinEvent` focus copy for its UIA focus and otherwise reads the focus itself (`books/dev/src/explanation/design/listener-without-connections.md`, decision 11).

### the-connection-timeout-applies-to-the-whole-process

- Kind: a kept CI test and a binary.
- Reproduces: tests `timeouts_set_through_one_client_are_read_by_another`, `the_first_client_in_a_process_has_verbatims_timeouts` and `making_a_client_keeps_the_timeouts_another_set` (E1, both timeouts, the last value winning). The tests read the timeouts back. Beyond them, the binary shows that the value read back is the one a call waits by: it times `h5`'s waits (10,013 ms and 508 ms) under the other client object's setting.
- Evidence now: the three `uia_bugs` tests; `h5_read.py` with the hung-window focusproto; E1 in the October experiments.
- Replaces: `h5_read.py`; E1 in `experiments.rs`.
- Verbatim relies on: `set_process_timeouts` inside `ensure_ready` (`crates/verbatim-uia/src/client.rs`), which sets them once, and every deadline built from `CONNECTION_TIMEOUT` and `TRANSACTION_TIMEOUT` (`crates/verbatim-listener/src/focus_read.rs`, `crates/verbatim-outpost/src/outpost/read.rs`, `crates/verbatim-supervisor/src/retire.rs`). If the timeouts became per client object, Verbatim's other clients would run on UIA's defaults (a 20 s transaction timeout) and those deadlines would abandon calls still waiting.

### a-call-waits-its-timeout-from-its-own-start

- Kind: a guide, a binary and a kept CI test.
- Reproduces: test `a_stalled_provider_holds_execute_until_the_transaction_timeout` (a remote operation is one transaction). Beyond the test, which covers only a remote operation, the binary covers plain calls: the 1.1 times wait (2.2 s, 5.5 s, 22.1 s), E-TT's single reads of 1,082 to 1,095 ms at a 1 s timeout, each wait from its own start (2,198 ms), and the provider answering failed calls afterwards. A case with a 1 ms timer resolution tests the inferred nominal 100 ms slice. Guide: the slice pattern in a context-switch trace (`cargo xtask trace waits --blocked`), and the one whole wait under cdb.
- Evidence now: mockapp's `remote_ops` test; the parallel-calls `timeouts.ps1`, with the trace read by `blocked-summary.py`; E-TT in the October experiments.
- Replaces: `timeouts.ps1`, `blocked-summary.py`.
- Verbatim relies on: the listener's read deadline, which adds `TRANSACTION_TIMEOUT` (`crates/verbatim-listener/src/focus_read.rs`), the outpost's walk budget (`crates/verbatim-outpost/src/outpost/read.rs`), the supervisor's 20-second retirement bound (`crates/verbatim-supervisor/src/retire.rs`), and `crates/verbatim-uia/src/bounded.rs`'s abandonable thread for operations of several calls. If waits lengthened, or a remote operation's bound changed, those deadlines would fire early or late.

### a-timeout-bounds-each-request-and-window-message-not-an-operation

- Kind: binary.
- Reproduces: the limitation of that name in `books/dev/src/reference/uia-bugs.md`, with the explainer's search waits. It covers the ordered waits; E-TT's four waits (13.2 s with the 10 s fetch timeout, run at 3 s); the queued search of 1,083 to 1,106 ms; the stall sweep at 100, 200, 500 and 1,000 ms (332, 618, 1,545 and 4,207 ms), and with connection recovery on 900 ms, the last whose search skips its `Find`. E3's 22,111 ms is checked by `a-call-waits-its-timeout-from-its-own-start`. The runtime id re-read that starts a search is cited from `uia-answers-some-calls-in-the-client`.
- Evidence now: the parallel-calls `search-timeouts.ps1` and `timeouts.ps1`; E-TT and E3 in the October experiments, with the rerun `e-tt-rerun.txt`. The stall test was removed in 31d462fc.
- Replaces: `search-timeouts.ps1`; E-TT and E3 in `experiments.rs`; `e-tt-rerun.txt`.
- Verbatim relies on: only the explanation in `crates/verbatim-supervisor/src/retire.rs` and `crates/verbatim-uia/src/bounded.rs`; the general rule is checked by the folder above.

### a-registration-that-times-out-reports-success

- Kind: a kept CI test and a binary.
- Reproduces: tests `a_group_registered_while_its_provider_is_held_reports_success` and `a_read_while_its_provider_is_held_fails`. The tests check that success is returned and that a read fails. Beyond them, the binary covers how long the registration waits and what follows it, which needs timing and pauses a test may not make: E-TT's 1,089 ms registration; `e57`: the gap of about 2 s, events lost, the focus group following the connection timeout; `e58`: advise, the silence, removal failing with `UIA_E_TIMEOUT`, the gap's reach; events between a focus change and a new registration (inferred). The 2 requests to add and remove are in the requests guide alone.
- Evidence now: the two `uia_bugs` tests; E-TT in the October experiments; `e57/` in a worktree at e04bdd8a with hand edits to mockapp; `e58/` in a worktree at 64557420 through its `apply.py`. Verbatim's exposure, 3,621 registrations with the longest at 32 ms, is prose of 20 end-to-end runs.
- Replaces: `e57/`, `e58/` with their patches.
- Verbatim relies on: `timed_out` in `crates/verbatim-uia/src/subscribe.rs`, which treats a registration slower than `CONNECTION_TIMEOUT` as timed out and logs it at warn, the only sign, and `Follower::close`'s second removal. If registrations began to fail honestly, the follower's error path would run instead, which is not wrong; if the wait stopped following the connection timeout, the warning would misfire. The gap's figures are used only by an unbuilt proposal.

### nothing-cancels-a-uia-call

- Kind: binary and a debugger guide.
- Reproduces: `CoCancelCall` answers `RPC_E_NO_CONTEXT` (5,614 of 5,614 over 203 trials) for property, text, `FindAll`, remote-operation and new-connection calls, with cancellation on and off. Abandoning frees only the client's wait, and the first answer after a release comes within 2 ms. Guide: `ChannelBasedClientConnection::WaitForResponse` on `WaitOnAddress` with no COM frame (`e_stacks`).
- Evidence now: moved by the first build to `uia-facts/nothing-cancels-a-uia-call/recorded/`; the parallel-calls `run-abandon.ps1`.
- Replaces: `cocancel_uia.rs` and its patch; `run-abandon.ps1`.
- Verbatim relies on: `crates/verbatim-uia/src/bounded.rs` and the outpost watchdog, which bound calls by abandoning their thread. If `CoCancelCall` began to work, nothing breaks; Verbatim is only more conservative than needed.

### a-provider-process-serves-one-uia-request-at-a-time

- Kind: binary.
- Reproduces: no overlap in provider work. A slow call holds every call into the process from any client, thread or window, remote operations included, and no other process's. First in, first out across windows, threads and clients. The 950 ms request. With `UseComThreading`: a busy UI thread with nothing queued holds nothing (0.4 to 1.2 ms), and a `CoWaitForMultipleHandles` provider holds the others. The provider program without `UseComThreading`: reads answer, a new connection fails at the timeout, `Invoke` waits for the UI thread. Connection recovery's effect on these outcomes (Not known), with each case rerun with recovery on.
- Evidence now: the parallel-calls `serve` program (`serve.rs`) with `run-serve.ps1` and `run-serve-xproc.ps1`, `iothread-sequence.ps1`, `run-pairs.ps1`, and AccessKit's `run-akapp.ps1`; the collide tool's `e16` and `e28c`.
- Replaces: `serve.rs`, `run-serve*.ps1`, `iothread-sequence.ps1`, `run-pairs.ps1` (mockapp cases).
- Verbatim relies on: D9's per-application outposts and "parallel queries within one application are given up" (`books/dev/src/explanation/architecture.md`, section 1). If UIA became parallel, nothing breaks; if a stuck application held calls into another, the isolation would fail.

### the-uia-channel-and-the-provider-io-thread

- Kind: guide.
- Reproduces: the pipe name (`UIA_PIPE_<pid>_<hex>`), the named events and `ConnectNamedPipe`, and the ALPC calls seen; the I/O thread created by the first `UiaReturnRawElementProvider` in `StaticIoThreadProc`; one per process after 30 connections; whether a second appears in a provider or a client (Not known); a new connection's thread (Not known); `ReleaseCollection::DispatchReleases`; events not through the I/O thread; the cross-apartment call's frames.
- Evidence now: the parallel-calls `transport.ps1`, `iothread-count.ps1`, `serving-thread.ps1` and `stacks.ps1`, with cdb on both processes.
- Replaces: `transport.ps1`, `iothread-count.ps1`, `serving-thread.ps1`, `stacks.ps1`.
- Verbatim relies on: no code; the observable outcomes are checked by the request-reader folders.

### msaa-calls-do-not-pass-through-uias-request-reader

- Kind: binary.
- Reproduces: MSAA answers in 1.2 to 2.6 ms while UIA waits. How long an MSAA call into a busy UI thread waits: the whole busy time, with no bound up to 20 s. oleacc's `accName` sends `WM_GETTEXTLENGTH` and `WM_GETTEXT`, counted by the binary's own window. Sent messages run at 0.52 to 0.78 times the single-thread rate with more threads. Contention in the window manager (inferred), compared across threads of one process and of two.
- Evidence now: the parallel-calls `msaa-while-stuck.ps1`, `msaa-path.ps1`, `msaa-path-mockapp.ps1`, and the MSAA cases of `run-mockapp.ps1`.
- Replaces: `msaa-while-stuck.ps1`, `msaa-path*.ps1`, the MSAA cases of `run-mockapp.ps1`.
- Verbatim relies on: the hung-window workaround's claim that MSAA keeps working, and the outposts' MSAA reads of other processes' windows (`books/dev/src/explanation/design/listener-without-connections.md`, sections 4.3 and 4.4). A change would slow those reads behind UIA's.

### parallel-client-threads-gain-at-most-twice-on-one-application

- Kind: binary and a debugger guide.
- Reproduces: 2 times on cheap calls, 1.0 to 1.5 times on costly ones; threads against processes; one shared client object; the I/O and UI threads' half share. Guide: 39 of 40 stacks waiting on replies, and the client's write and read frames.
- Evidence now: the parallel-calls `run-xbench.ps1`, `parts.ps1`, `client-stacks.ps1` and `run-mockapp.ps1`'s throughput and shared-client cases.
- Replaces: `run-xbench.ps1`, `parts.ps1`, `client-stacks.ps1`, `run-mockapp.ps1` throughput.
- Verbatim relies on: no figure; D9 gives up parallel queries within one application.

### real-applications-serve-uia-calls-one-at-a-time

- Kind: binary and a debugger guide.
- Reproduces: a suspended UI thread holds every call in Notepad, Edge, VS Code, Firefox, Windows Forms and WPF (WPF's measured hold). Chromium serves page and browser on one thread. Two File Explorer processes are independent (1 to 10 ms). One operation holds Firefox 2.6 ms and WPF 228 ms. Edge's 222 ms `GetText` against B's 220 ms. `FindAll` beside B, 0.4 ms median and 696 ms on a 20,000-element page, which the binary writes and serves from a file. Windows Terminal 1.24.12741.0 portable: the text calls wait while its XAML thread is suspended. Guide: `NavigationHelperProxyOneCore::s_CoreUIThreadProc` in Windows Terminal, and the 391 of 847 samples in `GetText`.
- Evidence now: the parallel-calls `run-real.ps1`, `stacks.ps1`, `wpf-window.ps1`, `winforms-window.ps1` and `wt-serving-thread.ps1`. No application version but Windows Terminal's is recorded.
- Replaces: `run-real.ps1`, `wpf-window.ps1`, `winforms-window.ps1`, `wt-serving-thread.ps1`.
- Verbatim relies on: D9's per-application outposts (`books/dev/src/explanation/architecture.md`, section 1), which hold that a slow call holds its own application only.

### requests-per-client-operation

- Kind: guide.
- Reproduces: every count in "What one request is": `Current` 1, a walker step 2 or 4, a parent step 4 to 6, `FindFirst` 7 or 9 with 338 calls in one `Find`, `FindAll` 34, 84 and 315, `BuildUpdatedCache` 2, `ElementFromPoint` 9, `Toggle` 12, `Invoke` 2, handlers 2 and 2, a remote operation 1, releases 23 of 30, the event cache's 8 requests. A search's frames and the 81 of 83 navigations. Where `AddFocusChangedEventHandler`'s 27 requests go, and what `GetFocusedElement` and a top-level element's next sibling reach (Not known).
- Evidence now: the parallel-calls `ops.ps1` with `ops-count.py`, `requests.ps1` and `search-timeouts.ps1`, with cdb on both sides.
- Replaces: `ops.ps1`, `ops-count.py`, `requests.ps1`.
- Verbatim relies on: no code; the provider-call costs Verbatim pins are tested in `crates/mockapp/tests/call_counts.rs`.

### uia-answers-some-calls-in-the-client

- Kind: a kept CI test and a binary.
- Reproduces: test `uia_event_registrations_cost_exactly`, which pins the warm registration costs with the runtime id read first. Beyond it, the binary covers two things.
  - The calls that make no provider call, by the provider program's call log: `Cached*` reads, `GetRuntimeId` after the first read and on a sender inside its callback, `CompareElements` with one side stalled, and the desktop parent. It also shows title-bar parts coming from proxies and `GetFocusedElement` going to the focused process.
  - UI Automation's copy of an element's runtime id lapsing (the `books/dev/src/reference/uia-bugs.md` entry), which needs the pauses a test may not make: the cold registration's 9 calls against 3, the cold pause 1 s; a read warm with no pause and cold after 1 s; the copy kept per element: reading `First`'s name, another button's or the window's, or registering on another button, leaves `First` cold; the workaround's warm registrations and free moves after pauses of 0, 0.5, 1 and 1.5 s, once each; `AdviseEventRemoved` arriving after a removal returns, and no other provider call after a call returns; a search's re-read of its start's runtime id after about 0.65 s (inferred).
- Evidence now: `uia_event_registrations_cost_exactly` in `crates/mockapp/tests/call_counts.rs`; the parallel-calls `ops.ps1` no-call cases; the `CompareElements` experiment, moved by the first build to `uia-facts/uia-answers-some-calls-in-the-client/recorded/`. The lapse's outputs are recorded but its code was not kept.
- Replaces: `compareelements-makes-no-call/`; `xproc.rs`'s `compare` case; `ops.ps1`'s no-call cases.
- Verbatim relies on: D13's contract that the listener's callbacks make no cross-process call (the module comment of `crates/verbatim-listener/src/listener.rs`: building a fact is local memory reads, plus `GetRuntimeId`, a local read on a cached element); the listener's comparison with the current focus (`compare_elements` in `crates/verbatim-listener/src/focus_read.rs`); the follower's group comparison (`crates/verbatim-uia/src/subscribe.rs`); and the lapse's workaround, reading the runtime id just before a registration. If any of these calls began calling the provider, a hung application would block the listener's callbacks and every focus with them.

### a-remote-operation-stops-after-exactly-10000-instructions

- Kind: a kept CI test only.
- Reproduces: test `the_instruction_limit_is_measured`, in mockapp's `instruction_limit` tests.
- Evidence now: that test.
- Replaces: none.
- Verbatim relies on: `books/dev/src/reference/performance.md`, "The instruction limit", and `books/dev/src/reference/crates/verbatim-uia-rops.md`: every program is sized under half the limit so it can be counted, the caret read reaching about 7,700; a run past the limit falls back to classic calls (hundreds of calls). A lower limit would push programs to the fallback and break the counting tests.

### focus-events-cannot-be-limited-to-one-application

- Kind: a kept CI test and a binary.
- Reproduces: the four tests of `crates/mockapp/tests/uia_bugs/focus_events_cannot_be_limited_to_one_application.rs`. They cover E-scope, and P1 and P5 as well: `a_focus_event_in_a_group_on_one_window_hears_that_applications_focus_alone` covers the group's scope, and `a_core_api_focus_registration_on_one_window_hears_that_applications_focus_alone` the core API. Beyond the tests, the binary covers only what they do not count: P2 and E-cost, the 47 to 49 provider calls a focus event costs per registered process, judged against 40 to 60.
- Evidence now: the four tests; `p1_scope.py`, `p2_cost.py` and `p5_core.py` with the focus-limitation folder's focusproto; E-scope and E-cost in the October experiments.
- Replaces: the focusproto of that folder, `p1_scope.py`, `p2_cost.py`, `p5_core.py`, and E-scope and E-cost. The binary leaves out P1 and P5, since the tests cover them.
- Verbatim relies on: D13's listener, which exists to hold the one desktop-wide focus registration while outposts hold none. A change breaks nothing; it would only reopen D13.

### elements-cannot-be-passed-to-another-process

- Kind: a kept CI test and a binary.
- Reproduces: the four tests of `crates/mockapp/tests/uia_bugs/elements_cannot_be_passed_to_another_process.rs`. The tests hold a sender briefly. Beyond them, the binary covers P3: a kept sender is still read live after 31 s and 152 s, a pause no test may make.
- Evidence now: the four tests; `p3_held.py` with the focus-limitation folder's focusproto; E-marshal.
- Replaces: `p3_held.py`; E-marshal.
- Verbatim relies on: the process that receives a UIA event being the only one that can read its element, which is why the listener judges and reads each focus itself (`books/dev/src/reference/uia-bugs.md`, the limitation's entry).

### ancestors-cannot-be-cached-or-listened-to

- Kind: binary.
- Reproduces: an ancestor-scoped registration hears no ancestor's event, in the provider program and in the taskbar clock case. The cache part links Learn's `TreeScope`.
- Evidence now: prose only; the taskbar clock and mockapp probes were not kept.
- Replaces: none kept (new).
- Verbatim relies on: ancestors being read by a walk or a remote operation; a change would only reopen an optimization.

### a-cache-request-fails-while-uias-first-time-setup-runs-on-another-thread

- Kind: a kept CI test and a binary.
- Reproduces: test `clients_created_at_once_in_a_fresh_process_all_build_cache_requests` (the workaround). Beyond the test, which runs with the workaround in place, the binary shows the fault itself: six threads in a fresh process without `ensure_ready`'s wait, 50 runs of 4 processes, as a rate.
- Evidence now: the test; the fault itself only by reverting the wait of commit 7c402534 (50 of 50 failures then).
- Replaces: commit 7c402534's reverted wait.
- Verbatim relies on: `ensure_ready` (`crates/verbatim-uia/src/client.rs`). If the fault were fixed, the wait is harmless.

### a-process-that-used-uia-can-hang-or-crash-as-it-exits

- Kind: binary.
- Reproduces: 300 child processes that make UIA calls and exit normally, with no `TerminateProcess`. The binary counts hangs (no exit within 10 s) and crash codes as a rate (1 to 3 percent).
- Evidence now: commits 1f69859e and fed532be; no script.
- Replaces: none kept (new).
- Verbatim relies on: its processes ending themselves with `TerminateProcess` once shut down, so the detach code never runs: outposts and the listener (`books/dev/src/explanation/architecture.md`, section 1), and the mockapp test runner (`crates/mockapp/tests/common/harness.rs`).

### a-remote-operations-keyboard-focus-read-can-differ-from-a-plain-read

- Kind: binary.
- Reproduces: Windows Terminal 1.24 portable, two windows: the binary compares a remote operation's `GetFocusedElement` with a plain read after each switch, as a rate.
- Evidence now: end-to-end runs in prose and commit ab18245e; `windows_terminal_two_windows` and `windows_terminal_leave_flood` test the workaround, and seeing the fault needs ab18245e reverted.
- Replaces: the revert of ab18245e.
- Verbatim relies on: the listener reading the sender's keyboard focus with a plain property read after the acceptance test, and dropping the focus when it has none (ab18245e; `books/dev/src/reference/performance.md`).

### inside-a-remote-operation-conhosts-window-has-no-handle-or-title

- Kind: a kept CI test and a binary.
- Reproduces: test `a_window_element_read_without_its_handle_is_named_by_its_runtime_id` (verbatim-outpost, the workaround, against a stand-in element). Beyond the test, the binary shows the fault in the real console host: it reads the native window handle and name of `conhost.exe`'s window inside an operation.
- Evidence now: commits 3f4dbb44 and 1b2e5420; the probe was not kept, and the fault's scenario `conhost_short_output` is retired. The workaround is also exercised by `conhost_two_windows`.
- Replaces: the probe not kept (new).
- Verbatim relies on: the outpost reading again through the window's proxy any ancestor that is a window's own element read without its handle, one `ElementFromHandleBuildCache` (`crates/verbatim-outpost/src/outpost/read.rs`).

### file-explorer-reuses-a-destroyed-elements-runtime-id

- Kind: a kept CI test and a binary.
- Reproduces: the handling tests in `crates/mockapp/tests/focus_reports.rs` (`a_reused_runtime_id_keeps_its_node_with_its_element_gone_remote`, `_classic`, and the `a_text_focus_under_the_runtime_id_of_` and `a_terminal_under_the_runtime_id_of_one_that_lost_the_focus_is_read_afresh` tests). Those tests check Verbatim's handling against mockapp. Beyond them, the binary shows that File Explorer itself reuses the ids: it navigates File Explorer folders and records the runtime ids of destroyed and new elements, as a rate.
- Evidence now: commit c67806c5; no probe of the fault.
- Replaces: none kept (new).
- Verbatim relies on: the listener reading, before reporting a UIA focus whose runtime id already names a node, whether the element held under that id still has the keyboard focus, and Core taking the focus as a duplicate only when it does (`books/dev/src/reference/uia-bugs.md`, the entry's workaround).

### windows-terminal-raises-focus-from-a-second-runtime-identity

- Kind: binary.
- Reproduces: 150 launches of Windows Terminal 1.24 portable under a desktop-wide focus handler. The binary judges identities in Rust, the judge ported from `identity_judge.py`, giving rates of 2 of 150 launches and 19 of 150 second windows. NVDA's 6 of 15 stays NVDA's record.
- Evidence now: `identity-judge-and-experiments.patch` onto 32948a4d, whose `measure_terminal_identities` scenario exists only in the patch, run with `VERBATIM_IDENTITY_ROUNDS=150` and judged on the listener log by `identity_judge.py`, with `judge.txt`.
- Replaces: `identity-judge-and-experiments.patch`, `identity_judge.py`, `suffix.py`.
- Verbatim relies on: the listener registering in NVDA's order (`crates/verbatim-uia/src/subscribe.rs`), dropping a focus with no window above it and one whose plain keyboard focus read answers no, and the twin rule designed for M5 (`docs/design/windows-terminal-twin-focus.md`).

### Inferences, open questions and duplicates

Every explainer item marked inferred or "Not known" is in one folder above. The exceptions are the exclusions under "Scope" and the items whose claims link their source: the frameworks not measured (the console host's provider, Qt, .NET 8 WPF, Windows Forms, Java) and the pre-1809 stall.

No fact is reproduced in two places:

- Each figure has exactly one folder.
- In each of the eleven folders where a kept test and a binary share a folder, the entry states what the binary checks beyond the test. That is always a real application, a timing, a pause, a rate or the fault behind the tested workaround, never a quantity the test asserts.
- The runtime id's lifetime and its lapse are only in `uia-answers-some-calls-in-the-client`, and the timeout-bounds folder cites it.
- The 47 to 49 calls are only in the focus-limitation folder.
- `h5` is only in the connection-timeout folder.
- Release counts are in the requests guide, and release delivery in the channel guide.

### What leaves `experiments/`

Code, scripts and patches move. The recorded outputs that a README cites move to that folder's `recorded/`. Everything else is deleted.

- Deleted outright:
  - `experiments/parallel-uia-calls/`
  - `experiments/a-new-connection-fails-while-another-processs-remote-operation-is-queued/`, its launch logs kept in the fact's `recorded/` as the history of Verbatim's exposure
  - `experiments/a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/`
  - `experiments/a-flood-of-uia-events-from-one-application-stops-all-of-a-processs-uia-events/`
  - the-connection-timeout-applies-to-the-whole-process (deleted by the first build)
  - `experiments/desktop-wide-focus-handlers-in-a-second-process-deliver-late/`
  - a-registration-that-times-out-reports-success (deleted by the first build)
  - `experiments/focus-events-cannot-be-limited-to-one-application/`
  - `experiments/elements-cannot-be-passed-to-another-process/`
  - `experiments/windows-terminal-raises-focus-from-a-second-runtime-identity/`
  - cocancelcall-does-not-end-a-uia-client-call (deleted by the first build)
  - compareelements-makes-no-call (deleted by the first build)
  - `experiments/two-new-connections-beside-a-pending-call/`
  - `experiments/mockapp-experiments-2026-10-10/`, since every experiment in it is one of the UIA facts above
- Trimmed:
  - `experiments/uiahasserversideprovider-says-no-for-a-new-conhost-window-while-another-process-has-an-event-handler/`: only `kept-runs/` and a short readme remain, the evidence of the excluded `conhost_two_windows` inference that `books/dev/src/reference/uia-bugs.md` still cites.
  - `experiments/focus-pipeline-rework-measurements/`: `programs.rs`'s three `UiaHasServerSideProvider` programs and `server-side-provider-under-stall.txt` leave. The rest stays, since `docs/design/focus-pipeline-rework.md`, the parity documents and two mockapp tests cite it.
  - `experiments/cross-process-calls-study-program/`: stays whole, since the study's other design experiments still use `xproc.rs`. Its readme names the `compare` and `second_form` cases as moved.
- Untouched, since none is a fact of the two documents: the MSAA `CoCancelCall` folder; the Settings frame's lag; the console caret, terminal read and redraw folders; the cross-process study's other experiments; the ownership and logic-tests studies; `experiments/security-model-survey/`.
- `experiments/readme.md` drops the moved entries and points to `uia-facts/readme.md`.
- Citations redirected, each in the build that moves its folder:
  - `books/dev/src/reference/uia-bugs.md`: each entry's "Reproduction test" and "Evidence" sections.
  - `books/dev/src/explanation/windows/uia.md`: the link definitions `[pc]`, `[hung]`, `[hung2]`, `[flood]`, `[collide]`, `[cancel]`, `[timeout-scope]`, `[registration]`, `[scope]`, `[compare]` and `[elements]` are replaced by per-folder links. `[pc]` splits by section across seven folders, so each sentence names its own folder.
  - The other documents: `books/dev/src/how-to/debugging.md`, `books/dev/src/explanation/design/listener-without-connections.md`, `books/dev/src/explanation/design/focus-pipeline.md`, `docs/design/focus-pipeline-rework.md`, `docs/design/cross-process-calls-study.md`, `books/dev/src/explanation/design/ownership.md`, `docs/design/windows-terminal-twin-focus.md`, `docs/design/m5-extensions-work.md`, `docs/roadmap.md`, `docs/parity/backends.md`.

  A final grep for each deleted folder's name, over everything outside `experiments/` and `nvda/`, must find nothing.

## Shared support

`uia-facts/support` (package `uia-facts-support`, a library) provides:

- The provider's client side: `Provider::launch(options)`, which starts the provider program from the executable's folder in a job object. Then come the requests to it, over a named pipe, each answered once the provider has acted on it (the hold taken, the event raised), so that a sample waits for evidence. Last, reading its call log and message log.
- A second client process: the binary starts itself again with `--role <name> <args>` in a job object that kills its children when closed. Results come back as lines on a pipe. A child ends with `TerminateProcess` on itself, as mockapp's runner does, because of the exit fault. The exit-fault binary leaves this out on purpose.
- The client settings: `--recovery` and `--coalescing`, both off by default, applied by `uia::client()` to every client object, and passed on to a role's process.
- Notes (`note`): `Report::case`, which prints a case's line and starts its evidence; `uia::pause(length, why)` and `note::waiting_ms`, which name a wait of a second or more; the evidence `Timed`, `uia::timed` and a role's lines keep, printed by `Report::claim` under a verdict of differ or inconclusive; and `--verbose`.
- Timing and verdicts: `Instant` stopwatch helpers; `Trials` (n runs with medians and ranges); `Recorded` (a value, a band, or k of n); the Wilson interval; and the line and exit-code format.
- Preconditions: an unlocked input desktop, an application at its path, and cdb on its path. Each failed precondition gives inconclusive with the reason.
- Applications: start Notepad, Windows Terminal from `UIA_FACTS_WT` (the portable 1.24.12741.0 copy), Edge, Firefox, VS Code and File Explorer at their installed paths, and read each one's file version for the header.
- `--pause` for the debugger guides.

Ported from the old commits' patches, as new code against main's APIs:

- the collide tool's two-process race (written anew against `verbatim-uia-rops`, since the patch's base commit 32948a4d is not main);
- focusproto's focus registrations, WinEvent triggers and desktop switch;
- evprobe's flood counter;
- `xproc.rs`'s two-process cases;
- `cocancel_uia.rs`'s cancel loop;
- `e57.rs`' and `e58.rs`' registration timing;
- `experiments.rs`' E-* cases, rewritten for main's 2-second connection timeout with the timeouts set explicitly in each case;
- the identity judge.

The mockapp additions in `scratch-support.patch` and `experiment-branch-support.patch` are not ported into mockapp. Each one a fact needs is a behaviour of the provider program.

## The provider program

`uia-facts/provider` replaces mockapp, the parallel-calls `serve` program and the synthetic providers of the old patches for every fact binary. It does only what the folders need.

- How behaviours are chosen: each behaviour is selected on its command line at launch, such as threading, tree and windows, or by a request from the sample while it runs, such as hold, stall, events, focus and removal.
- Its one fixture: a fragment tree of set size. A sample passes its node count: 24 nodes for the search's calls, 31 for the `FindAll` subtree, 102 for the whole tree.

Each behaviour, with the folders that use it:

- **A UIA window on an STA UI thread with `UseComThreading`**, the default. Used by every folder that has the provider as its target.
- **A server-side provider without `UseComThreading`**, served on UIA's I/O thread. Used by one request at a time, a new connection to a stuck application, and MSAA beside UIA (a UIA call stuck off the UI thread).
- **A fragment tree of set size** of buttons, with the text, toggle, invoke and legacy patterns on chosen nodes. Used by: a timeout bounding each request; UIA answering in the client; ancestors; nothing cancels (property, text, `FindAll` and remote-operation calls); a call's wait from its own start; parallel threads; the requests guide; the collision's walk up.
- **Hold on request**: a named provider method, or every method, blocks until released. Used by: a call's wait from its own start; a timeout bounding each request; the registration that reports success; nothing cancels; the event cache fetch; one request at a time; UIA answering in the client (`CompareElements` with one side stalled); the collision (calls on held elements); the pending connection.
- **Slow calls**: every provider call delayed by a set time while the window keeps pumping. Used by the hung window (`q3`) and by parallel threads (costly calls).
- **A busy UI thread on request**: the UI thread runs without pumping for a set time or until released. Used by: the collision (300 ms); the pending connection; a new connection to a stuck application; the hung window; MSAA beside UIA; `UiaHasServerSideProvider` (the window that does not pump); one request at a time (a busy UI thread with nothing queued).
- **A pumping wait in a provider call** (`CoWaitForMultipleHandles`). Used by one request at a time.
- **Several windows on several UI threads** in one process. Used by one request at a time (first in, first out across windows and threads), the collision (the whole process exposed), parallel threads and the pending connection.
- **Raised events on request**, one at a time or at a set rate. The events are focus changed, name property changed, notification, structure changed and text changed. Used by: the worker; the event cache fetch; the flood (about 10,000 a second, and 2,000 to 5,000); the hung window; the registration that reports success (`e57`, `e58`); the focus limitation (P2); elements (P3); ancestors; both late-focus folders; UIA answering in the client (the sender inside its callback).
- **A focus WinEvent on request** (`NotifyWinEvent` with `EVENT_OBJECT_FOCUS`) naming a chosen window, its own or a stalled one, and focus set within its tree. Used by the hung window, both late-focus folders and the focus limitation.
- **Windows created shown or hidden**, and brought to the foreground on request. Used by the hung window (`states`' hidden and shown windows) and the new process's late focus.
- **A provider-call log**: each provider method call with its thread id and time, between markers the sample sets. Used by: UIA answering in the client (the no-call checks, the runtime id's lapse, `AdviseEventRemoved`); the focus limitation (calls a focus event); the registration that reports success; one request at a time (no overlap, order); elements.
- **A window-message log by name**: `WM_GETOBJECT` with its object id, `UiaServerConnection_HookMessage`, `WM_GETTEXT` and `WM_GETTEXTLENGTH`. Used by the hung window (evidence that UIA's two messages reached the window), MSAA beside UIA, `UiaHasServerSideProvider` and the collision.
- **An MSAA object**: a window with its own `IAccessible` and no UIA provider, and a plain Win32 window with text for oleacc's proxy. Used by MSAA beside UIA, the raw probe (the window without a provider) and the collision (`e29`).
- **Element removal on request**: a node is removed and a structure change is raised. Used by elements (a kept sender read live until its provider removes it).

A figure measured on mockapp whose value depends on the provider's tree is re-recorded on the provider's first run, with the mockapp figure kept as history, since the provider is a different target. Such figures are the request counts, the 47 to 49 calls a focus event, the lapse's 9 against 3 calls, and the 338 calls in one `Find`. The README states this. A figure that depends only on UIA, such as a timeout's length, a verdict of an HRESULT or an order, is judged against the mockapp record directly.

## The separate workflow

`.github/workflows/uia-facts.yml`:

- Triggers:
  - `schedule` daily (`cron: "41 4 * * *"`), as `explore.yml` runs;
  - `workflow_dispatch`;
  - `push` to `main` with `paths`: `uia-facts/**`, `crates/verbatim-uia/**`, `crates/verbatim-uia-rops/**`, `crates/verbatim-model/**`, `crates/verbatim-text/**`, `crates/verbatim-core/**`, `crates/verbatim-reader-rules/**`, `crates/verbatim-win32/**`, `crates/workspace-hack/**` (the eight path crates), `Cargo.toml`, `Cargo.lock`, `.config/hakari.toml`, `rust-toolchain.toml` and the workflow file.

  `crates/mockapp/**` is not a trigger, since nothing in uia-facts uses mockapp.
- Environment: the workflow-level `env` is `ci.yml`'s own, `CARGO_TERM_COLOR: always`, and no job or step before rust-cache sets any other variable that rust-cache hashes (`CARGO*`, `RUST*`, `CC*`, `CFLAGS`, `CXX*`, `CMAKE*`). The main `ci` and `ci-arm64` jobs set none before their rust-cache step either. Their `VERBATIM_WX_DIR` comes later and is not a hashed name. `check-copies` reads both workflow files' top-level `env` blocks and fails when they differ, so the key cannot drift silently.
- Jobs: `ci` on `windows-latest` and `ci-arm64` on `windows-11-arm`. The job ids equal the main workflow's, so the cache keys line up. Each job:
  - checks out the repository;
  - runs `Swatinem/rust-cache@v2` with no `key`, the default `workspaces` (`. -> target`), and `save-if: false`;
  - runs `uia-facts/build.ps1`: the lock derivation, `check-copies` (the copied tables, the locks and the env blocks), and the build;
  - runs `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` in `uia-facts/`.

  It builds and lints only, on x64 and ARM64, and runs no binary (Dickson, 2026-11-04). A fact binary takes over the desktop and judges the figures of the machine it runs on, so running stays on demand on Dickson's machine, before and after a Windows update.
- The cache: rust-cache's key has a prefix and the job id; a hash of the environment (the rustc version and the `CARGO`- and `RUST`-prefixed variables); and a hash of the listed workspaces' `Cargo.lock` and `Cargo.toml` files. Its restore keys drop the final hash. With the same job id, no `key`, no extra `CARGO_*` or `RUST*` variable, the same pinned toolchain and the same workspace list, this workflow computes exactly the key the main `ci` job saves for the current main lock. It falls back to main's newest `ci` entry when the lock has moved on. GitHub's cache is shared by every workflow in the repository, and an entry from the default branch is readable from every run. `save-if: false` makes it read-only, so it never saves a second entry and never pushes the main entry out of the 10 GB limit. rust-cache keeps third-party artifacts only, so the run compiles the eight path crates and the facts. The build checks in its first run's log that the restore names a `v0-rust-ci-` key, exact or by fallback.
- What a miss costs: with no entry restored, the job compiles the whole third-party graph cold, `windows` with its full feature list, ICU4X's segmenters and jieba among them. That is an estimated 6 to 10 minutes on the 4-core runner, against 3 to 4 with the restore, and nothing breaks. A miss happens only when the environment hash differs, or when main's `ci` entry has been evicted, which happens after 7 days unused.
- What it adds to `cargo xtask ci` and the main `ci` job: nothing. Three small edits sit beside them:
  - `ci.yml`'s `changes` pattern gains `^uia-facts/`, so a push that touches only uia-facts runs only the docs job.
  - `xtask/src/docs.rs` adds `uia-facts/` to `REPOSITORY_ROOTS`, so the redirected citations are checked.
  - The READMEs, as Markdown files outside the excluded directories, are already checked by `cargo xtask docs`.

### Expected times

These are estimates from measured figures, and the first build measures and records the real ones.

- Locally (12-core machine, main's `target/debug` current):
  - A first uia-facts build compiles the eight path crates once more, plus the support crate, the provider program, `check-copies` and about 30 binaries, each linking `windows`. Expected 45 to 90 s, mostly linking. Without the shared target it would add a cold third-party build in a fresh target, about 4 to 5 minutes as a worktree build.
  - A rebuild after editing one fact: 3 to 8 s.
  - After a change to `verbatim-uia`: 20 to 40 s (two crates and every relink).
- In CI (4-core runner): about 54 s of setup, measured for the main jobs (checkout, toolchain, a cache restore of about 25 s); then clippy and the build of the path crates and facts, about 2 to 3 minutes, with no eSpeak NG or wxWidgets in the graph. Expected 3 to 4 minutes a job.

## The order of work

The work is three builds. Each one moves its experiments folders, redirects their citations, and deletes a folder only once everything in it has moved. Each one:

- runs every new binary once on this machine and compares its outcome with the recorded figure;
- writes that first run into the README, with any differ or inconclusive verdict explained or raised;
- runs `cargo xtask ci` once at the end, since `docs.rs`, `ci.yml` and the documents change;
- checks the uia-facts workflow's GitHub run.

Binaries that need the desktop (focus, Windows Terminal, File Explorer, a suspended UI thread in a real application) are run by the parent from one background script, as with the end-to-end scenarios. The builder runs the rest.

- The workspace, support, workflow and calls build:
  - What it builds: the workspace, the support crate, `check-copies`, `build.ps1`, the workflow, the ci and docs edits, and the provider program with the behaviours these folders use: the UIA window with and without `UseComThreading`; the tree; hold; slow calls; the busy UI thread; the pumping wait; several windows; the call and message logs; the MSAA object; raised events on request, and focus set within its tree, since the registration and the client-side answers folders raise events.

    It also builds the request-reader and timeout folders: new connections timing out; the connection timeout per process; a call's wait from its own start; a timeout bounding each request, not an operation; the registration that reports success; nothing cancelling; one request at a time; the channel guide; MSAA beside UIA; parallel threads; the requests guide; UIA answering in the client; the instruction limit.
  - What it moves: `the-connection-timeout-applies-to-the-whole-process`, `a-registration-that-times-out-reports-success`, `cocancelcall-does-not-end-a-uia-client-call`, `compareelements-makes-no-call`, and the E1, E-TT and E3 parts of `mockapp-experiments-2026-10-10`.
  - What it measures: the first-build and rebuild times, and whether the CI job restores main's cache, reported to Dickson and not recorded in `uia-facts/readme.md`.
- The events and focus build:
  - What it builds: the hung window and the client-threads guide; the worker; the event cache fetch; the flood; both late-focus folders; the focus and elements limitations; ancestors; the first-time setup race; the exit fault.
  - What it adds to the provider program: the focus WinEvent, shown and hidden windows, and element removal.
  - What it moves: the hung-window, flood, desktop-wide, focus-limitation and elements folders, and the rest of `mockapp-experiments-2026-10-10`, which this build deletes.
- The connections, probe and applications build:
  - What it builds: the collision, with its conhost case and its second form; the pending connection; `UiaHasServerSideProvider` and the raw probe; the remote operation's keyboard focus; conhost inside a remote operation; File Explorer's runtime id; Windows Terminal's identities; real applications.
  - What it adds to the provider program: nothing, since its folders use behaviours that the first two builds made.
  - What it moves, and deletes: the collision, conhost-probe (trimmed), Windows Terminal identity, `two-new-connections-beside-a-pending-call` and `parallel-uia-calls` folders. It also moves the `focus-pipeline-rework-measurements` programs and does the final citation grep.

## Decisions Dickson owns

All decided by Dickson on 2026-11-04.

- Excluded, with no reproduction, as listed under "Scope" and in `uia-facts/readme.md`'s "Cited, not reproduced" table: `conhost_two_windows`'s inference; NVDA's exposure inferences; the stall before Windows 10 version 1809; the applications never measured; NVDA #20654.
- Verbatim's own exposure figures from old builds stay as history in their fact's README, linked to their commits and logs, with no binary. These are:
  - the Notepad and Windows Terminal launch rates (1 in 25, 3 in 100, the 3,008 ms probe);
  - the listener held 3.8 s by `states` and `both`, and 7.8 s against the mockapp kept running all evening;
  - `altf`'s menu focus spoken 126 to 170 ms after the key;
  - the 3,621 registrations with the longest at 32 ms.
- Qt's stale cached keyboard focus and Excel 2016's freeze point to NVDA's record, NVDA #20764 and NVDA's source comment, with no reproduction. They are listed in the "Cited, not reproduced" table.
- uia-facts has its own provider program, and no fact binary launches mockapp.
- The UI Automation faults recorded through applications that ship with Windows, such as File Explorer's reused runtime id, Windows Terminal's identities and the runs against real applications, are reproduced, since those applications are always available.
- `parallel-uia-calls` stays in `experiments/` until the third build has moved the last of its facts, so for that time some of its facts are reproduced in two places.
- The workflow only builds and lints, on x64 and ARM64, and runs no binary. Running stays on demand on Dickson's machine, before and after a Windows update.
- windows-bindgen is not used.
