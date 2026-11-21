# CLAUDE.md

## What this repository is

Verbatim: a screen reader for Windows 11 (x64 and ARM64), written in Rust.

- `books/dev/src/explanation/architecture.md` — decisions of record, read this before proposing any design or implementation work.
- `docs/roadmap.md` — the milestones ahead, completed milestones are archived in `docs/roadmap-done.md`.
- `books/` — the documentation published to GitHub Pages, one mdBook per book, each arranged after Diataxis (tutorials, how-to guides, reference, explanation): `books/dev/` the Development guide, `books/extensions/` the Extensions guide, `books/user/` the User guide. A page joins its book's `SUMMARY.md`, and names anything outside its book, an agent-only document included, by its full GitHub address, because the site carries nothing else (`cargo xtask docs` checks both).
- `docs/` — the working documents only agents read, never published: the roadmap, the design records of work not yet built, the parity ledger, `docs/nvda/` and the flaky-test list. `docs/readme.md` is the index of both, with reading paths; `books/dev/src/reference/glossary.md` defines Verbatim's invented vocabulary.
- `books/dev/src/reference/crates/` — the reviewer's guide to the implemented crates, one file per crate (`books/dev/src/reference/crates/readme.md` is the index): what each does, its public API, and how the intricate parts work. Keep the file for a crate current when its public API changes.
- `books/dev/src/how-to/readme.md` — how to actually drive this project, with the interactive-session rule and an index of the guides beside it in `books/dev/src/how-to/`: setup, `verbatim-inspect` against a running instance, `mockapp`, measuring, the end-to-end suite, NVDA captures, and the traps that cost us time (`troubleshooting.md`).
- `books/dev/src/reference/testing.md` — the testing standard every test must meet: one fixed target and code path, exact assertions, waits only for evidence, and no hidden failures. Read it before writing or changing any test, and check every test change against it.
- `books/dev/src/reference/test-coverage.md` — every gesture and user-facing feature, with the tests that cover it at each level and the gaps still open. Update its row when adding or removing a gesture, a user-facing feature or a test.

`nvda/` is the NVDA screen reader vendored as a git submodule **for reference only** (IA2 IDL under `nvda/include/ia2`, app modules under `nvda/source/appModules`, design docs under `nvda/projectDocs`). Never modify anything under `nvda/`; Verbatim is informed by NVDA but not constrained by its architecture.

## Writing conventions

All documentation, comments, and commit messages in this repo must read well with a screen reader, in both rendered and source form:

- No ASCII-art diagrams, box drawings, or arrow chains.
- Documents describe the present. When a mechanism changes, replace its description rather than appending a dated note or keeping the old text beside the new; history lives in git and the commit messages. A decision of record states the decision as it stands, dated when it was made, with no amended-on line. A rule that exists because something went wrong keeps one sentence of why, not the narrative. A superseded design record is archived or deleted, never kept in place with a banner.
- Design documents state the current model, its rules each with a brief reason, its limitations, and the important performance figures such as latency and memory; evidence lives in `experiments/` and history in git, so that a document stays short enough to be read whole. Once its work is built, a design document becomes two: an explanation of how it works today in `books/dev/src/explanation/design/`, with no alternatives or history, and a numbered decision record in `books/dev/src/reference/adr/` with the problem, the options weighed and why each was taken or rejected, the decision with its date, and its limits; the mechanism's details stay in the crate guide (Dickson, 2026-11-07).
- Each rule, mechanism and figure is written in one place, and other documents point at it: a behaviour in its explanation page, NVDA's behaviour and the ruled differences in its parity entry, a decision and its limits in its ADR, a mechanism in its crate guide, and a figure a test pins in that test, which `books/dev/src/reference/performance.md` names rather than copies; a cost goes in that ledger only when it is on the event or speech path or was measured to be slow. Because the dialog wait's ending rule is written in nine places, one behaviour changed 11 to 16 documents, and four ledger figures drifted from the tests that pin them (Dickson, 2026-11-20).
- Reports define any invented term on first use or avoid it, lead with the required decisions

## Who decides

Claude decides everything except the items below. It records each decision where the decision lives, as "(Claude, <date>)", and lists it in the build's report so that Dickson can reverse it. Dickson decides these, and only work on that one item waits for him (Dickson, 2026-11-21):

1. Public contracts: the extension API, the settings file format, the control and Remote protocols.
2. Trust boundaries: what is sandboxed, what extensions may do, UIAccess and secure screens.
3. Which process or thread runs which work.
4. A new kind of request, reply, text operation, waiting request or queue hook in Core's protocol (ADR 0003, "A new command adds no kind of request").
5. Scope and order: what each milestone includes, and any entry added to `docs/roadmap.md`.
6. Adopting or replacing a major dependency, and licensing.
7. This file, `REVIEW.md` and `books/dev/src/reference/testing.md`.

What users hear: matching NVDA needs no decision. A difference from NVDA, including one kept because matching would need new state, is built, listed in the report with NVDA's speech beside Verbatim's, and recorded in the parity ledger as pending until Dickson approves or reverses it. A budget overrun is reported the same way. A decision he took keeps "(Dickson, <date>)", and none is written as his before he gives it. When something waits for him, the session carries on with other work and never blocks on a question.

## How work is done

The default path, for most changes:

1. Plan in a few lines in the session's scratchpad: what changes, what it replaces, which tests cover it. Every claim about the code is checked with a grep or a test before it is written down.
2. Build with targeted tests, then `cargo xtask ci` once.
3. One review of the finished change by a separate agent, as `REVIEW.md` says; fix its findings and run `cargo xtask ci` again.
4. Run the one to three end-to-end scenarios on the changed path, three runs each, push, and read GitHub's finished run, triaging each failure before the next push.

A design comes first only when a change adds or changes state in a reducer, adds a message between processes, or changes what users hear. It states the speech in each sequence it changes; a table of the states it adds and what each rule does in each; what the replaced code did and said, mapped to new code or to a ruling; and its size against NVDA's lines for the behaviour, a figure to report, not a gate. It gets one review, which checks it against the code, NVDA's pinned source and the rulings and adds no requirements; its findings are fixed once and the build starts, and a point the designer and reviewer cannot settle in one more exchange goes in the report.

Evidence:

- A claim about NVDA cites its pinned source under `nvda/` or a capture already made, or is left out.
- Speech a change alters is captured from NVDA before its expectation is written: one attempt, two if the first was disturbed, and then NVDA's source decides and the test says so.
- `cargo xtask nvda compare` is an exploration tool for drafting expectations against NVDA; it is never run in CI and its output is never committed or checked (`books/dev/src/how-to/nvda.md`, "Comparing a scenario").
- A recording of the real application is made when a stand-in's event order is in doubt, not by default.
- Measurements are made where a change could pass a performance budget, and at a milestone's end.
- The agent that writes a desktop script runs it; one agent at a time holds the desktop, named in its brief, until it reports.

Running work:

- Locally, run one to three scenarios, never a family or the full suite, and at most 20 runs of one scenario when judging a fix, because GitHub runs the suite on every push.
- A failure is matched to `docs/flaky-tests.md` and its artifacts read, naming the process or code responsible, before it is rerun or called environmental. A scenario failing for a known outside cause is fixed, deleted, or listed with the build that removes the cause; local-only is the last resort, with its reason.
- One builder works in the main tree at a time, with incremental builds on; other agents only read, or work in their own worktree with `CARGO_INCREMENTAL=0` and delete its target directory when done. Keep the C: drive above 15 GB free.
- A brief points at documents rather than restating them, names its output and limits, and asks for a short report; a subagent whose brief cannot work stops and reports rather than improvising.
- Recurring traps of the desktop, PowerShell and the harness are written once in `books/dev/src/how-to/troubleshooting.md`, and briefs point there.
- Clean up: stop the processes you started, remove merged worktrees, and release held keys.

Stopping: a build stops and reports when its mechanism needs a third fix to its design (a fix to tooling or tests does not count), when a review's must-fix contradicts a ruling of Dickson's, or when one of his decisions above comes up. It never stops for a budget, a missing capture or a count of review rounds.

Fix root causes: never hide a race or failure with a retry, a sleep or a mechanism added for that one failure; a workaround needs evidence that the proper fix is not enough and is documented as one.

Judge every change on wake-ups, calls into other processes, locks and processor time on the event and speech path as well as latency; an allocation, a copy or a local call is no cost to design around unless a measurement shows it matters.

Studies, reviews, design drafts and run logs stay in the session's scratchpad; `experiments/` holds only evidence a standing decision cites. A new rule replaces or merges an old one and names its incident in one clause.

## Design principles

Every design brief and review checks these; the ownership refactor (`books/dev/src/explanation/design/ownership.md`) records the history behind them.

- A decision is made by a process that sees, in order, every fact it depends on. A design lists each decision made outside Core with those facts and who sees them. A reader deciding from its own last read, where the fact is Core's or another reader's, is guessing: a guess that can only cost calls is allowed and marked as one; a guess that can change what is spoken is not built, and Core never corrects a reader by asking it again, because the readers' ancestry cache needed Core's check, a live re-read and an ordered queue (Dickson, 2026-11-10).
- One owner and one representation per fact: logic is a pure reducer with calls as its effects, and no process or thread judges a copy of another's state; inside one reducer, a lifecycle is one explicit state value, a type whose variants carry the data valid in them, never a combination of flags, optional fields, stamps and lookups in other records. A design that adds a lock, a shared flag, a second writer or a copied fact says why the hand-off cannot be removed instead, because copied state judged in two places caused the focus pipeline rework's fix rounds.
- Make invalid states unrepresentable, and parse rather than validate: a state that must not occur has no way of being written, in a reducer's state as in a message between processes, and input is turned once, at the boundary, into a type that can hold only what is valid, so that no later code checks it again (Dickson, 2026-11-09).
- Write no concurrency by hand: no raw pointers in posted messages, custom lock-free structures, or flags with fences; where a hand-off stays, use plain events, channels or locks, or a well-tested crate checked from its source, because each hand-written hand-off grew its own races, and the hook's went through four designs before it was deleted.
- Count the lifecycle states a design adds (re-dispatch, slots, holds, waiter tables, refused results) and have its review ask whether each can be removed, because the outpost reducer removed its races but its added bookkeeping produced all eight of its later defects. The design lists the waiting states, messages, timers and lifecycles held in two places that it adds and removes; a change made to save calls, wake-ups or time that adds any of them needs a measurement showing a budget missed without it (Dickson, 2026-11-10).
- A wake is decided by reading the state when the decision is made, never by assuming the other thread's state, because a push that skipped its wake when the lane looked busy lost wakes.
- A waiter on an event two conditions share checks its own condition again, and no test assumes only one setter, because the mixer's room test failed on GitHub's runner for that reason.
- A COM object is released only on a thread allowed to block, never on one that must stay responsive, because a release can call into a slow application.
- A loop never times a wake it will not act on, because the outpost's reducer spun at full processor time on a past wake.
- Every request and waiter has exactly one answer path, including after abandonment and re-dispatch, best a token whose drop answers it, because the settle waiters were lost twice that way.
- State about an owner (an outpost generation, an application's process, a terminal, a reading thread's application) lives in that owner's record, so its ending drops it; every stage of a lifecycle emits its ending, including stages before the peer exists; and a check against an ended owner's ids is tested.
- Where a fact can be both committed and in flight (applied and waiting, held, pending), the design names the two versions as separate facts, after NVDA's own split where it has one (its focus object and its last queued focus), and every rule says which version it judges.

## NVDA provenance

The crates fall into three kinds, each listed in `xtask/src/tiers.rs`, where a new crate is classed. The platform-neutral crates, which are `verbatim-model`, `verbatim-core`, `verbatim-speech`, `verbatim-config`, `verbatim-i18n`, `verbatim-input`, `verbatim-audio`, `verbatim-text`, and `verbatim-ext-api`, contain only original code, so that tier's licensing does not depend on NVDA's. Implement their behavior from the parity ledger under `docs/parity/` and the prose under `docs/nvda/`, never by translating NVDA source, and cite those docs in comments rather than NVDA file paths, function names, or line numbers. They never depend on a GPL-tier crate or on the Windows bindings; the hook thread and the WASAPI sink live in `verbatim-input-windows` and `verbatim-audio-wasapi` for that reason, and `cargo xtask ci` fails if the rule is broken. NVDA's spoken vocabulary, messages, and key layouts are deliberately matched for user familiarity and are fine to use; NVDA's non-English translations are never imported.

The logic crates, `verbatim-reader-rules`, `verbatim-outpost-logic` and `verbatim-listener-logic`, are in the GPL tier and may hold ported behaviour, but depend on no Windows crate and not on the Windows bindings, so their logic builds and is tested on any host. The Windows-specific crates are GPL like NVDA and may port NVDA code freely. A role's crates (`verbatim-outpost` with `verbatim-outpost-logic`, `verbatim-listener` with `verbatim-listener-logic`, and `verbatim-supervisor`) depend on no other role's. Ported code never moves from a GPL-tier crate into a platform-neutral one, and NVDA-core behavior destined for the reducer, such as browse mode, review modes, or symbol processing, is written from the parity docs rather than ported.

## Coding Standards

We use Rust 2024 edition.

Arm64 is a first-class target. Builds and tests always use the host's own architecture (no command names a target), so ARM64 is built and tested on an ARM64 machine, which GitHub Actions provides (the `ci-arm64` job); this machine is x64. Two stated exceptions, since neither builds a binary or runs a test: a check-only `cargo check` or `cargo clippy` with `--target x86_64-unknown-linux-gnu` on this machine, to check a crate's Linux build, and the tier check's `cargo metadata` with no platform filter, which reads every target's graph so its answer does not depend on the host.

Use Clippy with the pedantic lint group from the start. Public Rust APIs should have doc comments so the workspace can be browsed with Rustdoc.

Code reaches no fallback that could only be a logic bug: a miss is removed by construction, or passed up as an error, never defaulted silently.

We are using GitHub actions for CI.

## Commands

`cargo xtask ci` is the standard check, and exactly what GitHub Actions runs for a change to anything but Markdown files, `docs/` and `books/` (a documentation-only change runs only `cargo xtask docs` there): the tier check (every workspace member is classed in `xtask/src/tiers.rs`, and the graph keeps the tiers' rules under NVDA provenance, for every target, over normal and build edges), a check that the generated `crates/workspace-hack` crate is current (it needs `cargo install cargo-hakari --locked`; after adding a package or changing a third-party dependency, run `cargo hakari generate` and `cargo hakari manage-deps`, as `books/dev/src/how-to/setup.md` explains), a check that every document's links and backticked repository paths resolve (`cargo xtask docs`), rustfmt, clippy (pedantic via workspace lints, warnings denied), and unit tests, all for the host's architecture and into `target/debug`, the same output the end-to-end suite uses. On a Linux host it runs the tier check, the scans and the documentation check, then Clippy and unit tests for the crates listed in `LINUX_CRATES` (`xtask/src/main.rs`); GitHub Actions runs it there as the `ci-linux` job on `ubuntu-latest`, and the documentation-only check also runs on Linux. On Linux xtask has only `ci`, `docs`, `replay`, `e2e-plan` (which sizes the end-to-end groups in the `plan` job) and `coverage` (which merges a coverage run's levels in the `coverage` job); every other command is Windows-only.

`cargo xtask park` moves this Remote Desktop session onto the console, unlocked, so a local end-to-end run works with no RDP client connected. It needs a one-time, elevated `vm\scripts\Register-VerbatimParkTask.ps1`, and disconnects any connected RDP client, so run it only when Dickson is away or has agreed.

The end-to-end suite runs against this machine, or another one through `--endpoint` (`books/dev/src/explanation/architecture.md`, section 14), by pointing it at a locally running `verbatim-agent`. It launches a real Verbatim and injects real keystrokes, so it takes over the desktop while it runs and does nothing useful on a locked one; `books/dev/src/how-to/e2e.md` has the details. Run scenarios locally with `cargo xtask e2e <scenario>... --runs <n>`, which builds and starts the agent, runs each scenario and ends the agent (`books/dev/src/how-to/e2e.md`); "How work is done" says which scenarios, and GitHub's run from the push is the wider check.

The `port` skill (`/port <scope>`, in `.claude/skills/port/`) is the protocol for porting an NVDA behavior or subsystem; its `references/` folder holds the brief template every subagent brief is written from.

The GUI's wxWidgets is built from source by `verbatim-gui`'s build script the first time anything builds that crate: it downloads the pinned release, builds static libraries with CMake (Ninja on x64), and keeps them in `target/wxwidgets`, or under `VERBATIM_WX_DIR` when that is set, so later builds reuse them. Nothing has to be set up by hand beyond CMake and the Visual Studio C++ tools.

Work on main, with no pull requests or feature branches unless asked, so that every session and subagent builds on the same history. When a scenario is added, removed or changes length, run `cargo xtask e2e-durations` once its GitHub run finishes and commit the table.
