# Review: xtask on Linux and the Linux job

Adversarial review of `docs/design/logic-foundations.md`, section "xtask on Linux and the Linux job (a build of its own)", against the code at commit 62c340d1 (`xtask/`, `.github/workflows/ci.yml`, `.config/hakari.toml`, `crates/workspace-hack`, `crates/verbatim-core` and its dependencies), on 2026-10-31.

Important findings: 2.

## Important

### The gating misses `std::time::Instant` in `xtask/src/main.rs`, so Clippy fails on Linux

- Where: `docs/design/logic-foundations.md` line 204 (the gating rule names "the three Windows functions in `main.rs`" and "the `windows` imports"); `xtask/src/main.rs` line 55.
- Defect: `use std::time::Instant;` is used only by `warm_speech` (line 168), which the design gates, but the import is a standard-library one that the rule does not gate, so on Linux it is an unused import.
- Counterexample: build as written, then on Linux `cargo xtask ci` runs `cargo clippy -p verbatim-core -p xtask --all-targets -- -D warnings`; `unused_imports` on line 55 is denied and the `ci-linux` job, and with it every code change, fails. The other imports (`env`, `Command`, `ExitCode`, `str`) stay used by `main`, `ci` and `check_platform_neutral_deps`. Fix: the rule should say every item used only by gated code is gated with it, and name `Instant`.

### After the split, the tier check reads only the host's target, so it passes on Linux where it fails on Windows, and the Linux build does not make up for it

- Where: `docs/design/logic-foundations.md` line 124 (the tier check "reads `cargo metadata` once, filtered to the host's target"), against line 213 (the rule that the dependency check passes `--target all` "so it reads the same graph on every host", because "the check's answer must not depend on where it ran") and line 250 (a logic crate that reaches a Windows crate "then fails to build on Linux, which proves what the split's dependency check reads from the graph").
- Defect: the split replaces `check_platform_neutral_deps`, the one function this build makes host-independent, with `xtask/src/tiers.rs` filtered to the host's target, so the rule of line 213 lasts one build and the check becomes host-dependent again on the Linux job.
- Counterexample: after the split, `crates/verbatim-reader-rules/Cargo.toml` gains `[target.'cfg(windows)'.dependencies] verbatim-win32 = ...`. On Linux the host-filtered metadata has no such edge, so the tier check passes; Linux's build of `verbatim-reader-rules` never compiles `verbatim-win32`, so it passes too, contrary to line 250. Only the Windows `ci` job's tier check catches it, which is the opposite of what the section says the Linux job proves. Fix: the tier check reads `cargo metadata` without `--filter-platform` (every target), as this section's rule asks, and line 250 says the Linux build proves only edges without a target condition.

## Nits

- `docs/design/logic-foundations.md` line 213 against line 256: `cargo tree --target all` is a command naming a target, which the design adopts on its own reading ("builds nothing, so this is not a build naming a target"), while the same reading applied to `cargo check --target` is put to Dickson as a question. Either both go in the question or neither does.
- Line 221: `LINUX_CRATES` beside `PLATFORM_NEUTRAL_CRATES` is used only by the non-Windows branch of `ci`. If that branch is written with `#[cfg(not(windows))]`, as the gating rules' style suggests, the constant is dead code on Windows and Windows Clippy denies it; the constant is gated with its branch, or the branch is chosen with `cfg!(windows)`.
- Line 206: the `ci` line of the usage text (`xtask/src/main.rs` line 128) says "rustfmt, clippy, and unit tests"; on Linux it lists neither the scans nor the reduced crate set, so it changes by host too.
- Line 211: the documentation check does not trim trailing whitespace; it reads lines with `str::lines` (`xtask/src/docs.rs` line 401), which drops the `\r`. The conclusion, that a CRLF and an LF checkout read the same, holds.
- Line 245, the time estimate:
  - The Windows `ci` job took 8.2, 8.7 and 9.5 minutes in the last three successful pushes (runs 41177085810, 41170238013, 41163500778), not 6.5 to 8.5; `ci-arm64` took 9.9 to 12.4.
  - The basis is a Windows build (31 s) and run (66 s) on 12 threads; the Linux job also runs Clippy, a separate check build of the same dependencies (ICU, `jieba-rs` with `zstd-sys`, the `workspace-hack` set), and `cargo tree --target all` eight times, which downloads every target's crates on a cold cache; and the 66 s run is not scaled to 4 processors. A cold run nearer 6 to 8 minutes is as likely as 4 to 6.
  - The cache: the Windows `ci` job's whole-workspace caches are 349 to 478 MB each, so a Linux cache for a few crates is likely well under a gigabyte; the repository uses 4.4 GB of its 10 GB today.
  - The conclusion holds: the job runs beside `ci`, `ci-arm64` and `e2e`, which takes 60 to 67 minutes, so it adds no wall time.

## Checked and found sound

- Module gating: no portable module (`docs`, `ownership`, `waits`, `shapes`) names a gated module; `ownership` and `shapes` use only `waits`, and the only cross-module calls among the Windows modules are `perf` to `trace` and back, both gated. No Windows module uses an item of `main.rs`. `xtask` has no dev-dependencies.
- `--target all`: on today's graph every platform-neutral crate's tree reaches `windows-sys` only, at depth 2 or 3 beneath `workspace-hack` and deeper beneath third-party crates (`winapi-util` and others); none reaches `windows` or `windows-core`, and no workspace crate other than `workspace-hack` has `windows-sys` or `winapi` as a direct child. So the check passes on Windows after the change as before. The workspace-crate test (`xtask/src/main.rs` line 362) matches Linux's `(/home/.../crates/x)` form.
- The documentation check: existence is decided by comparing each component with a directory listing, so case and backslashes are judged the same on both systems (a span such as `vm\scripts\...` does not start with `vm/` and is not read as a path); fences and lines are normalised through `str::lines`. The repository has no symbolic links (`git ls-files -s`), which would be skipped on Linux only, and no `\r` in the index (`git ls-files --eol`).
- The documentation-only path: no Rust source includes or reads a Markdown file, and `docs/` holds only Markdown, so the Windows `docs` job's build of xtask's Windows dependencies proved nothing a documentation-only change could break. With `ci-linux` under the same condition as `ci`, every code change still runs the Windows `ci` job (rustfmt, hakari, Clippy and tests for the whole workspace), so the Linux job lets nothing through, and no event runs no job.
- `verbatim-core` and its test dependencies: no `cfg(windows)`, `target_os` or `std::os`; no clock, thread or sleep in `verbatim-core`'s source; `tests/terminal_cost.rs`, the one timed file, is `#[ignore]`; `tests/alloc.rs` compares a small state's allocation with a large one's on the same platform, never a fixed count; the term-sim corpus is sorted after `read_dir`; fixtures are joined from `CARGO_MANIFEST_DIR` with relative components.
- Hakari: `.config/hakari.toml` lists the two Windows triples, so moving xtask's dependencies under `cfg(windows)` leaves the generated file unchanged; Linux gets the unconditional section (portable, as the design's `cargo check` showed) and skips the Windows tables (`getrandom`, `windows-sys`). Features on Linux are not unified across package selections, which costs only rebuilds, since Clippy and the tests select the same packages. `cargo hakari generate --diff` and `manage-deps --dry-run` read only metadata, so running them on Windows alone suffices; no `hakari verify` is needed on Linux.
- The workspace loads without the eSpeak NG submodule (`members = ["crates/*", "xtask"]`, no path into `third_party/`), as today's Windows `docs` job already shows.
