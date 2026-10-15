---
name: port
description: >
  Research, design, implement, and verify an NVDA-to-Verbatim port or
  behavioral migration. Use for a narrowly scoped behavior or a broad
  subsystem rewrite where NVDA is the behavioral reference but Verbatim's
  architecture may differ substantially.
argument-hint: "<scope>"
disable-model-invocation: true
---

# Port an NVDA subsystem or behavior to Verbatim

The requested scope is:

$ARGUMENTS

This is a redesign migration, not a source-to-source translation. NVDA is a behavioral and implementation reference; Verbatim deliberately has different architecture. Preserve behavior that Verbatim has chosen to match, and do not preserve NVDA implementation structure merely because it exists, since it answers constraints Verbatim does not have.

A session normally runs one slice from research through design, judge, implementation, review, and documentation to a commit. The files under `references/` hold the checklists and templates; each phase says when to read one.

## Operating principles

A narrow request remains narrow. Do not expand a specific behavior into a general rewrite unless that is necessary to implement it correctly, because a widened scope does not fit one session.

The requested scope defines what success means; slices define how to reach it safely. A broad scope may contain many small vertical slices, each leaving the repository in a coherent, tested state.

Divide the labour this way. The parent conversation reads the relevant sections of the durable docs, makes the design decisions with Dickson, writes briefs, reads reports, decides, and commits. Subagents do the NVDA research, returning the causal path and the preserve, replace, and omit list rather than file contents; the implementation and test work; and the review. The parent does not run cargo build, test, clippy, `cargo xtask ci`, or end-to-end scenarios itself, does not read source files at length, and does not re-run what a subagent has reported, because in past sessions that, not the hand-offs, was where the tokens went. Research subagents that only read and summarize can be Sonnet; implementation and review subagents are Opus.

Write every brief from `references/brief.md`, and include the standing rules in `references/rules.md`, because subagents see neither this conversation nor Claude's memory.

## The durable sources of truth

- `CLAUDE.md` contains repository-wide rules and authoritative reading paths.
- `docs/architecture.md` contains architectural decisions and invariants.
- `docs/roadmap.md` defines milestone scope and exit criteria.
- `docs/parity.md` records which NVDA behaviors Verbatim intends to match and their verification status.
- `docs/nvda/` records how NVDA actually behaves and implements nontrivial features, cited against the pinned NVDA source.
- `docs/design/` contains detailed Verbatim designs where a behavior requires nontrivial architectural decisions.
- `docs/crates/` explains implemented crate responsibilities and public APIs.
- `docs/testing.md` defines what counts as a valid Verbatim test.
- `docs/tooling.md` defines how to exercise and inspect the implementation.
- `docs/nvda-transcript.md` defines how to capture NVDA's speech live.
- Code and tests record what has actually been implemented.

Keep these current as part of the port. Do not create a second source of truth merely because this skill is running, because two records of the same status drift apart.

Read parity entries and sections of `docs/nvda/` and `docs/design/` with `python -I .claude/skills/port/scripts/entry.py <file> "<name>"`, which prints one entry or section, rather than grep or sed over the whole file: these files are thousands of lines and were pulled into the parent's context hundreds of times in past sessions.

## Size the scope

Before phase 1, state the slice's end condition and judge whether research through commit fits one session. If it does not, narrow the slice now to one behavior, one backend, or one application, rather than planning several slices and carrying work over. A session whose scope was a whole milestone ran for eight days with fourteen context compactions and consumed most of the project's token budget; sessions that resumed one item from a written plan ran at a third of the context size.

## Phase 1: Establish the current state and decompose the scope

Read the relevant parts of `docs/readme.md` to choose the shortest authoritative reading path, then the relevant architecture decisions, parity entries, `docs/nvda/` documents, and crate and design documentation, reading parity entries and sections with `scripts/entry.py` as above. Leave the Verbatim implementation and the NVDA source to research subagents. If existing documentation already establishes a fact, do not rediscover it unless current code or NVDA source gives reason to doubt it.

If the scope contains several behaviors, decompose it into dependency-ordered vertical slices, each crossing the layers needed to make one behavior real rather than consisting of arbitrary files. For an event subsystem, slices might be registration and lifetime, raw intake, coalescing and ordering, normalization, focus and foreground behavior, acceptance filtering, backend arbitration, stale-event handling, reducer effects, speech consequences, and restart behavior; derive the actual decomposition from the subsystem and the repository. For each slice identify the behavior it delivers, its prerequisites, the affected crates, the independent evidence that will prove it, and which later slices depend on it. Map the scope well enough that an early slice is unlikely to force an architectural reversal later. For a narrow request, this phase may be a few sentences.

For a scope with several slices, do the smallest slice end to end first, as a shakedown, and note which rules this skill, the design, or the parity entry lacked before trusting the decomposition. The migration write-ups this skill draws on all found their critical rule gaps on a three-file trial, not in planning.

## Phase 2: Establish what NVDA actually does

Brief read-only research subagents with the list in `references/nvda-path.md`. They trace the complete causal path, not the most obvious handler, read parity entries and NVDA sections with `scripts/entry.py`, and distinguish facts established from source or capture from hypotheses, because an inference turned into a parity requirement becomes a bug with a citation.

When the research establishes nontrivial NVDA behavior not already documented, have it recorded in the appropriate file under `docs/nvda/` with source citations.

## Phase 3: Classify what carries into Verbatim

Classify every material behavior or mechanism as one of:

- **Preserve**: observable NVDA behavior Verbatim intends to match.
- **Replace**: the requirement remains, but Verbatim satisfies it through different architecture.
- **Omit or defer**: behavior or machinery Verbatim deliberately does not implement now.
- **Unresolved**: evidence or an owner decision is still required.

Do not assume that an NVDA implementation detail is a Verbatim requirement, nor that architectural difference permits observable behavior to change. Record deliberate differences in the parity ledger or the design document. Respect the provenance rules in `CLAUDE.md`: do not move ported NVDA code into a platform-neutral crate for convenience, because that tier's licensing must not depend on NVDA's.

## Phase 4: Map the behavioral contract onto Verbatim

Design the implementation against the architecture that exists, rather than an NVDA-shaped one. Read `references/boundaries.md` to decide which part owns each responsibility and, for work across a process or thread boundary, what crosses it and who may block.

For a nontrivial redesign, update or create the design document before implementation. It is the migration rulebook: it decides architectural questions once so that implementation subagents do not improvise them differently. Have a subagent review the design adversarially.

## Phase 5: Establish the judge before implementation

Before changing production behavior, settle two separate questions.

Where the assertion runs: choose the cheapest place that can catch the failure, in this order: a unit test, a `mockapp` cross-process test, an end-to-end scenario, an interactive probe. Follow `docs/testing.md` exactly. Do not run desktop-taking tests or NVDA captures unless the session has permission under the repository's tooling rules.

Where the expected value comes from: for anything spoken, a live NVDA capture (`cargo xtask nvda capture`, `docs/nvda-transcript.md`) taken before the expectation is written. Never take it from Verbatim's current output, and do not settle it by reading NVDA's source when a capture is possible: on 2026-10-11 desktop scenarios expected "Program Manager" after a dialog closed, which came from Verbatim's old rule rather than NVDA and was found only when Dickson questioned it.

If the required judge does not exist, building the smallest trustworthy one is part of the work. Validate a new judge where feasible by showing that it passes the intended implementation and fails a deliberately wrong or previously failing case, because a judge that cannot detect a known error has not proved parity.

## Phase 6: Freeze the contract for the implementation loop

Once the behavioral contract, design decisions, and expected results are established, they are the grader. Do not weaken the contract, parity entry, expected output, or test to make implementation pass, because then the test records the implementation rather than the requirement.

If implementation shows the agreed specification is wrong, stop the loop, return to research or design, establish the new evidence, amend the durable specification explicitly, and then resume against the amendment. Keep correcting the specification and accommodating the implementation separate.

## Phase 7: Implement dependency-first, one coherent slice at a time

Brief implementation subagents in dependency order. Fix root causes rather than adding a mechanism for each failure, because the outpost code once became a pile of independent mechanisms each added for one live failure and had to be redesigned.

Ported code in the Windows-tier (GPL) crates cites, in a comment, the NVDA source path and function it comes from; platform-neutral crates cite `docs/parity.md` or `docs/nvda/` sections instead, per `CLAUDE.md`. Writing the citation forces the implementer to trace the logic rather than approximate it; a port of libinjection found that demanding source-line citations was what exposed omitted logic.

## Phase 8: Verify progressively

Use cheap proofs before expensive ones: the narrowest unit or crate checks after a local change, the targeted cross-process or end-to-end scenarios after a completed slice, and `cargo xtask ci` under the repository's normal rules. Do not run the whole end-to-end suite by default, because it takes a long time; follow `CLAUDE.md` and `docs/tooling.md` for when a full run is justified.

A passing compiler is not behavioral evidence, and a newly written test passing does not by itself prove that it asserts the right contract.

## Phase 9: Perform an adversarial semantic review

Have a review subagent review the slice as if it is subtly wrong despite passing its tests. It runs in a separate context from the implementer, because the writer of code wants it accepted. Give it `references/review.md`. Every suspected defect needs a concrete counterexample, a violated invariant, or a missing piece of evidence.

Fix verified defects and re-run the relevant judge. When the review finds a bug, sweep the slice for the same class before closing it. If the same class recurs across slices, stop treating each instance as independent: determine whether the design, parity description, test strategy, or this skill lacks a rule, and fix that before continuing.

## Phase 10: Close the slice

A slice is complete only when its behavior is implemented, its independent judge passes, the relevant documentation is current, every semantic-review finding is resolved or recorded, and every deliberate NVDA divergence it introduced is documented. Then commit.

The session ends at the slice's commit. If the scope held more slices than this one, record the remaining ones in `docs/roadmap.md` in dependency order, with what this slice's research and design settled for them, so the next session starts from the record rather than from the conversation. Start another slice in the same session only when Dickson says to.

A slice left unfinished is the exception. When it happens, leave the repository coherent and record in the appropriate durable artifact the requested scope, the decomposition, completed and remaining slices, unresolved questions, tests or measurements still required, and the next dependency-ordered action, so that a fresh session can resume without the previous conversation.

## Final report

Report concisely: the requested scope; what was researched; the important preserve, replace, and omit decisions; slices completed; tests and other judges run, with results; documentation changed; semantic-review findings and their disposition; remaining work within the scope; and the exact next slice if work remains.

Report concrete evidence rather than general confidence. Do not claim parity, correctness, or completion beyond what the judge actually established, because the next session builds on the claim.
