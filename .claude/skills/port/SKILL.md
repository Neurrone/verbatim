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

A session runs the scope from research through design, judge, implementation, review, and documentation to a commit. The files under `references/` hold the checklists and templates; each phase says when to read one.

## Operating principles

A narrow request remains narrow. Do not expand a specific behavior into a general rewrite unless that is necessary to implement it correctly, because a widened scope is what made past sessions run for days.

The requested scope defines what success means. The steps that reach it are chosen for safety: each leaves the repository coherent and tested.

Divide the labour this way. The parent conversation reads the relevant sections of the durable docs, makes the design decisions with Dickson, writes briefs, reads reports, decides, and commits. Subagents do the NVDA research, returning the causal path and the preserve, replace, and omit list rather than file contents; the implementation and test work; and the review. The parent does not run cargo build, test, clippy, `cargo xtask ci`, or end-to-end scenarios itself, does not read source files at length, and does not re-run what a subagent has reported, because in past sessions that, not the hand-offs, was where the tokens went. Research subagents that only read and summarize can be Sonnet; implementation and review subagents are Opus.

Write every brief from `references/brief.md`, and point it at the "Standing rules" section of `CLAUDE.md`, because subagents see neither this conversation nor Claude's memory.

## The durable sources of truth

- `CLAUDE.md` contains repository-wide rules and authoritative reading paths.
- `docs/architecture.md` contains architectural decisions and invariants.
- `docs/roadmap.md` defines milestone scope and exit criteria.
- `docs/parity/readme.md` and the files it indexes record which NVDA behaviors Verbatim intends to match and their verification status.
- `docs/nvda/` records how NVDA actually behaves and implements nontrivial features, cited against the pinned NVDA source.
- `docs/design/` contains detailed Verbatim designs where a behavior requires nontrivial architectural decisions.
- `docs/crates/` explains implemented crate responsibilities and public APIs.
- `docs/testing.md` defines what counts as a valid Verbatim test.
- `docs/tooling.md` defines how to exercise and inspect the implementation.
- `docs/nvda-transcript.md` defines how to capture NVDA's speech live.
- Code and tests record what has actually been implemented.

Keep these current as part of the port. Do not create a second source of truth merely because this skill is running, because two records of the same status drift apart.

Read parity entries and sections of `docs/nvda/` and `docs/design/` with `python -I .claude/skills/port/scripts/entry.py <file> "<name>"`, which prints one entry or section, rather than grep or sed over the whole file: these files are thousands of lines and were pulled into the parent's context hundreds of times in past sessions.

## Confirm the scope

Before anything else, write down what the scope means in concrete terms: the behavior that will exist at the end, which applications and backends it covers, what is heard or asserted that is not heard or asserted today, and what is deliberately out. Where the request leaves this open, say what reading was taken and why, and ask Dickson before design begins. Scope that is only understood implicitly is how past sessions grew from one item into a whole milestone and ran for days.

## Phase 1: Establish the current state and order the work

Read the relevant parts of `docs/readme.md` to choose the shortest authoritative reading path, then the relevant architecture decisions, parity entries, `docs/nvda/` documents, and crate and design documentation, reading parity entries and sections with `scripts/entry.py` as above. Leave the Verbatim implementation and the NVDA source to research subagents. If existing documentation already establishes a fact, do not rediscover it unless current code or NVDA source gives reason to doubt it.

If the scope contains several behaviors, order the work into dependency-ordered steps, each crossing the layers needed to make one behavior real rather than consisting of arbitrary files. For an event subsystem, steps might be registration and lifetime, raw intake, coalescing and ordering, normalization, focus and foreground behavior, acceptance filtering, backend arbitration, stale-event handling, reducer effects, speech consequences, and restart behavior; derive the actual order from the subsystem and the repository. For each step identify the behavior it delivers, its prerequisites, the affected crates, the independent evidence that will prove it, and which later steps depend on it. Map the scope well enough that an early step is unlikely to force an architectural reversal later. For a narrow request, this phase may be a few sentences.

For a scope with several steps, do the smallest one end to end first, as a shakedown, and note which rules this skill, the design, or the parity entry lacked before trusting the plan. The migration write-ups this skill draws on all found their critical rule gaps on a three-file trial, not in planning.

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

Cost the design before it is built, because the number of cross-process calls an operation makes is the cost Verbatim controls and the one CI pins (`docs/performance.md`, "What counts as a call" and "The ledger"). For each operation the change adds or touches, state which process makes each call into the application, which calls a UIA remote operation batches into one, what the operation reads from the event's cache rather than fetching, and what it adds to the per-event work and wake-ups of the listener, the outpost, or Core. For new event support, that includes the registration itself and what every occurrence costs whether or not it is spoken. Write the expected counts into the design as the ledger entry the implementation must meet.

For a nontrivial redesign, update or create the design document before implementation. It is the migration rulebook: it decides architectural questions once so that implementation subagents do not improvise them differently. Have a subagent review the design adversarially, including its costing.

## Phase 5: Establish the judge before implementation

Before changing production behavior, settle two separate questions.

Where the assertion runs: choose the cheapest place that can catch the failure, in this order: a unit test, a `mockapp` cross-process test, an end-to-end scenario, an interactive probe. Follow `docs/testing.md` exactly. Do not run desktop-taking tests or NVDA captures unless the session has permission under the repository's tooling rules.

Where the expected value comes from: for anything spoken, a live NVDA capture (`cargo xtask nvda capture`, `docs/nvda-transcript.md`) taken before the expectation is written. Never take it from Verbatim's current output, and do not settle it by reading NVDA's source when a capture is possible: on 2026-10-11 desktop scenarios expected "Program Manager" after a dialog closed, which came from Verbatim's old rule rather than NVDA and was found only when Dickson questioned it.

Performance has its own judge. An operation that makes calls into an application gets its entry in the ledger (`docs/performance.md`, "The ledger") and an exact call-count test in `crates/mockapp/tests/call_counts.rs`, in the same commit, because an exact count is a ratchet that fails CI when a later change adds a call. A change on the event or speech path is also measured as `docs/tooling.md`, "Measuring performance", prescribes, against the figures recorded before the change.

If the required judge does not exist, building the smallest trustworthy one is part of the work. Validate a new judge where feasible by showing that it passes the intended implementation and fails a deliberately wrong or previously failing case, because a judge that cannot detect a known error has not proved parity.

## Phase 6: Freeze the contract for the implementation loop

Once the behavioral contract, design decisions, and expected results are established, they are the grader. Do not weaken the contract, parity entry, expected output, or test to make implementation pass, because then the test records the implementation rather than the requirement.

If implementation shows the agreed specification is wrong, stop the loop, return to research or design, establish the new evidence, amend the durable specification explicitly, and then resume against the amendment. Keep correcting the specification and accommodating the implementation separate.

## Phase 7: Implement dependency-first, one coherent step at a time

Brief implementation subagents in dependency order. Fix root causes rather than adding a mechanism for each failure, because the outpost code once became a pile of independent mechanisms each added for one live failure and had to be redesigned.

Ported code in the Windows-tier (GPL) crates cites, in a comment, the NVDA source path and function it comes from; platform-neutral crates cite `docs/parity/` entries, as `docs/parity/readme.md` says, or `docs/nvda/` sections instead, per `CLAUDE.md`. Writing the citation forces the implementer to trace the logic rather than approximate it; a port of libinjection found that demanding source-line citations was what exposed omitted logic.

## Phase 8: Verify progressively

Use cheap proofs before expensive ones: the narrowest unit or crate checks after a local change, the targeted cross-process or end-to-end scenarios after a completed step, and `cargo xtask ci` under the repository's normal rules. Do not run the whole end-to-end suite by default, because it takes a long time; follow `CLAUDE.md` and `docs/tooling.md` for when a full run is justified.

A passing compiler is not behavioral evidence, and a newly written test passing does not by itself prove that it asserts the right contract.

For a substantial change or a new feature on the event or speech path, measure before closing: the call counts against the design's costing, and latency, wake-ups, and processor time with `cargo xtask measure` and `cargo xtask trace scenario`, reported as `docs/tooling.md`, "Measuring performance", prescribes and compared with the figures recorded before the change. A measurement that is worse than the costing predicted is a defect to root-cause, not a figure to record, because a cost paid on every event outlives the session that added it.

## Phase 9: Perform an adversarial semantic review

Have a review subagent review the result as if it is subtly wrong despite passing its tests. It runs in a separate context from the implementer, because the writer of code wants it accepted. Give it `references/review.md`. Every suspected defect needs a concrete counterexample, a violated invariant, or a missing piece of evidence.

Fix verified defects and re-run the relevant judge. When the review finds a bug, sweep the whole change for the same class before closing. If the same class recurs across steps or sessions, stop treating each instance as independent: determine whether the design, parity description, test strategy, or this skill lacks a rule, and fix that before continuing.

## Phase 10: Close the scope

The scope is delivered only when its behavior is implemented as confirmed at the start, its independent judge passes, the relevant documentation is current, every semantic-review finding is resolved or recorded, and every deliberate NVDA divergence it introduced is documented. Then commit, and the session ends there.

A scope left unfinished is the exception. When it happens, leave the repository coherent and record in the appropriate durable artifact the confirmed scope, the work order, completed and remaining steps, unresolved questions, tests or measurements still required, and the next dependency-ordered action, so that a fresh session can resume without the previous conversation.

## Final report

Report concisely: the confirmed scope; what was researched; the important preserve, replace, and omit decisions; steps completed; tests and other judges run, with results; the costing and the measurements against it, where the change touched the event or speech path; documentation changed; semantic-review findings and their disposition; remaining work within the scope; and the exact next step if work remains.

Report concrete evidence rather than general confidence. Do not claim parity, correctness, or completion beyond what the judge actually established, because the next session builds on the claim.
