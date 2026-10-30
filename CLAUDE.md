# CLAUDE.md

## What this repository is

Verbatim: a screen reader for Windows 11 (x64 and ARM64), written in Rust.

- `docs/architecture.md` — decisions of record, read this before proposing any design or implementation work.
- `docs/roadmap.md` — the milestones ahead, completed milestones are archived in `docs/roadmap-done.md`.
- `docs/readme.md` — the index of all documentation, with reading paths; `docs/glossary.md` defines Verbatim's invented vocabulary.
- `docs/crates/` — the reviewer's guide to the implemented crates, one file per crate (`docs/crates/readme.md` is the index): what each does, its public API, and how the intricate parts work. Keep the file for a crate current when its public API changes.
- `docs/tooling/readme.md` — how to actually drive this project, with the interactive-session rule and an index of the guides beside it in `docs/tooling/`: setup, `verbatim-inspect` against a running instance, `mockapp`, measuring, the end-to-end suite, NVDA captures, and the traps that cost us time (`troubleshooting.md`). `docs/tooling/vm.md` covers every `cargo xtask vm` verb and rebuilding the golden image.
- `docs/testing.md` — the testing standard every test must meet: one fixed target and code path, exact assertions, waits only for evidence, and no hidden failures. Read it before writing or changing any test, and check every test change against it.

`nvda/` is the NVDA screen reader vendored as a git submodule **for reference only** (IA2 IDL under `nvda/include/ia2`, app modules under `nvda/source/appModules`, design docs under `nvda/projectDocs`). Never modify anything under `nvda/`; Verbatim is informed by NVDA but not constrained by its architecture.

## Writing conventions

All documentation, comments, and commit messages in this repo must read well with a screen reader, in both rendered and source form:

- No ASCII-art diagrams, box drawings, or arrow chains.
- Prefer prose and lists over pipe tables.
- Documents describe the present. When a mechanism changes, replace its description rather than appending a dated note or keeping the old text beside the new; history lives in git and the commit messages. A decision of record keeps its date and an amended-on line, but one current statement. A rule that exists because something went wrong keeps one sentence of why, not the narrative. A superseded design record is archived or deleted, never kept in place with a banner.
- Design documents state the current model, its rules each with a brief reason, its limitations, and the important performance figures such as latency and memory; evidence lives in `experiments/` and history in git, so that a document stays short enough to be read whole. Once its work is built, a design document keeps the problem, the alternatives weighed, the reasons for the choice and its limitations, and points to the crate guide for the mechanism, because the mechanism changes with the code and the crate guide is kept current with it.
- Reports define any invented term on first use or avoid it, lead with the required decisions

## Standing rules

These hold for every session and every subagent; a subagent brief points here rather than restating them.

- Fix root causes, never adding retries, sleeps, skipped cleanup, or self-termination to hide a race or failure; a workaround needs evidence that the proper fix is not enough, and is documented as one.
- Verify every claim about NVDA's behavior from the pinned source under `nvda/` or a live capture, and take spoken expectations only from captures
- The NVDA submodule pin must equal the installed NVDA build used for captures: the capture header prints the build, so compare it with `git -C nvda rev-parse HEAD` before a capture series and stop when they differ, because expectations come from captures and ported code from the pinned source, and both must describe the same NVDA.
- If a mechanism being implemented has 3 or more rounds of bug fixes, the design is likely flawed and continuing risks adding patches on top of workarounds. Pause and reassess whether to continue or if a redesign is required
- Follow `docs/testing.md` (one fixed target and code path, exact assertions, waits only for evidence, no branching on the environment, no alternative expectations, no substring matches), because a test that accepts two outcomes proves neither.
- Run the end-to-end scenarios related to a change, and the full suite only for wide-ranging changes or when asked, because the full suite takes a long time and takes over the desktop. Run each related scenario 3 times per build, 10 times only for a scenario that failed, to judge its fix, and at most 20 in a measurement.
- Before calling an end-to-end failure interference, a flake or environmental, read its artifacts and name the process or code responsible, because on 2026-10-30 two failures called interference were a unit test's stray console window and a real defect.
- Work on main, with no pull requests or feature branches unless asked, so that every session and subagent builds on the same history.
- Push rarely, and only after `cargo xtask ci` and the related scenarios pass locally, because a push is not a test run.
- Confirm a design before building on it, including all edge cases, where each part runs and the exact speech expected; while a design study or decision is open, build nothing that depends on it. All designs undergo an adversarial review from a subagent before implementation
- Review each completed step as `REVIEW.md` says, not each commit: one full adversarial review of the step's whole implementation by a separate read-only subagent, Important findings reproduced then fixed in one commit, one diff-only re-review, then stop, with nits batched into one unreviewed documents commit; trivial and document-only changes get no review, because on 2026-10-21 reviewing every commit with no stopping rule produced contradictory findings and sixteen rounds on one area. The diff-only re-review is made only when the fix is large.
- Plan multi-step work as a few large builds, each grouping steps that touch the same code, with one design document, one adversarial review that breaks it with code-level sequences, and one implementation review, rather than a full cycle per small step; ask Dickson each build's open questions in one batch before building it, and otherwise take the choice that matches NVDA, recorded pending his acceptance; defects a review finds in the mechanism being built are fixed in that build, not deferred as follow-ups. Because on 2026-10-30 a cycle per step (build, ci, review, fix, re-review, ten-run scenario batches) and questions asked one at a time took more than twelve hours for half of one design.
- Builders run targeted tests while working and `cargo xtask ci` once at the end, again only when a rebase brought in code; use Sonnet for mechanical work (failure triage, nits commits, ledger updates, capture runs) and Opus for designs, builds and reviews; keep briefs short by pointing at documents, and ask for reports capped at the outcome, the commit and the decisions, because restated briefs and long reports cost tokens on every hand-off.
- Use UIA remote operations wherever a sequence of UIA calls would benefit, and audit this whenever UIA code changes, because each separate call is a cross-process round trip.
- Judge every change on wake-ups and CPU as well as latency, and raise neither on the event or speech path unless that is the change's purpose, because a latency figure alone hides a cost paid on every event.
- Clean up: stop the processes you started, remove merged worktrees, delete large traces once read, and release held keys, because leftovers break the next run and fill the disk.
- Do not stop while work in scope remains; record a question for Dickson and continue with everything it does not block, because a turn that ends on a question idles until he reads it.
- Keep ported NVDA code in the Windows-specific crates and write the platform-neutral crates from the parity docs (see "NVDA provenance"), so that the platform-neutral tier's licensing does not depend on NVDA's.
- At most two subagents build at once on Dickson's machine, each in its own target directory, because more exhausts its disk and memory.
- A subagent that finds its brief's design cannot work, or that wants to do anything beyond its brief, stops and reports instead of improvising, and the parent never commits such a choice for later acceptance, because improvised choices committed as "pending" each cost a review and often a reversal.
- Builder subagents end at unit tests and `cargo xtask ci`; the parent runs the end-to-end scenarios, one batch at a time from one background script, because agents that waited on the desktop with a large context spent hours polling and fought over it.

## NVDA provenance

The crates fall into two tiers. The platform-neutral crates, which are `verbatim-model`, `verbatim-core`, `verbatim-speech`, `verbatim-config`, `verbatim-i18n`, `verbatim-input`, `verbatim-audio`, and `verbatim-text`, contain only original code, so that tier's licensing does not depend on NVDA's. Implement their behavior from the parity ledger under `docs/parity/` and the prose under `docs/nvda/`, never by translating NVDA source, and cite those docs in comments rather than NVDA file paths, function names, or line numbers. They never depend on a Windows-specific crate or on the Windows bindings; the hook thread and the WASAPI sink live in `verbatim-input-windows` and `verbatim-audio-wasapi` for that reason, and `cargo xtask ci` fails if the rule is broken. NVDA's spoken vocabulary, messages, and key layouts are deliberately matched for user familiarity and are fine to use; NVDA's non-English translations are never imported.

The Windows-specific crates are GPL like NVDA and may port NVDA code freely. Ported code never moves from a Windows crate into a platform-neutral one, and NVDA-core behavior destined for the reducer, such as browse mode, review modes, or symbol processing, is written from the parity docs rather than ported.

## Coding Standards

We use Rust 2024 edition.

Arm64 is a first-class target. Builds and tests always use the host's own architecture (no command names a target), so ARM64 is built and tested on an ARM64 machine, which GitHub Actions provides (the `ci-arm64` job); this machine is x64.

Use Clippy with the pedantic lint group from the start. Public Rust APIs should have doc comments so the workspace can be browsed with Rustdoc.

We are using GitHub actions for CI.

## Commands

`cargo xtask ci` is the standard check, and exactly what GitHub Actions runs: the platform-neutral dependency check (the crates listed under NVDA provenance must not pull in the Windows bindings), a check that the generated `crates/workspace-hack` crate is current (it needs `cargo install cargo-hakari --locked`; after adding a package or changing a third-party dependency, run `cargo hakari generate` and `cargo hakari manage-deps`, as `docs/tooling/setup.md` explains), a check that every document's links and backticked repository paths resolve (`cargo xtask docs`), rustfmt, clippy (pedantic via workspace lints, warnings denied), and unit tests, all for the host's architecture and into `target/debug`, the same output the end-to-end suite uses.

`cargo xtask vm <cmd>` drives the Hyper-V harness: `create` (Packer-built golden image, imported, deployed to, checkpointed), `start`, `stop`, `restart`, `restore`, `deploy`, `test` (the end-to-end suite against the VM), `logs`, and `delete`. Guest credentials come from a `.env` at the repo root, which is never committed. See `docs/tooling/vm.md`.

`cargo xtask park` moves this Remote Desktop session onto the console, unlocked, so a local end-to-end run works with no RDP client connected. It needs a one-time, elevated `vm\scripts\Register-VerbatimParkTask.ps1`, and disconnects any connected RDP client, so run it only when Dickson is away or has agreed.

The end-to-end suite also runs without a VM, against this machine, by pointing it at a locally running `verbatim-agent`. It launches a real Verbatim and injects real keystrokes, so it takes over the desktop while it runs and does nothing useful on a locked one; `docs/tooling/e2e.md` has the details. Build and start the agent with `cargo build -p verbatim-agent` and `target\debug\verbatim-agent.exe --bind-address 127.0.0.1 --port 44001`, set `$env:VERBATIM_E2E_ENDPOINT = '127.0.0.1:44001'`, then run one scenario with `cargo test -p verbatim-e2e --test e2e -- --ignored --exact <name> --test-threads=1`, or the full suite with `cargo test -p verbatim-e2e --no-fail-fast -- --ignored --skip demo_ --skip measure_ --test-threads=1`.

The full suite takes a long time, so don't run it by default. For a change, run the scenarios related to the feature being worked on, such as the `windows_terminal_` and `conhost_` scenarios a terminal change affects. Keep full runs for wide-ranging changes, such as changes to the harness, to how outposts start and stop, or to behaviour every scenario sees.

The `port` skill (`/port <scope>`, in `.claude/skills/port/`) is the protocol for porting an NVDA behavior or subsystem; its `references/` folder holds the brief template every subagent brief is written from.

The GUI's wxWidgets is built from source by `verbatim-gui`'s build script the first time anything builds that crate: it downloads the pinned release, builds static libraries with CMake (Ninja on x64), and keeps them in `target/wxwidgets`, or under `VERBATIM_WX_DIR` when that is set, so later builds reuse them. Nothing has to be set up by hand beyond CMake and the Visual Studio C++ tools.
