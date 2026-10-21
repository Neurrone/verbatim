# Code review in this repository

This file says what a review is for here, what a finding must carry, what is fixed in the review loop and what is not, and when the loop stops. `CLAUDE.md`'s standing rules point here; the `port` skill's phase 9 and every review brief follow it. The targets a reviewer looks for are in `.claude/skills/port/references/review.md`.

## What failure looks like here

A change has failed review when any of these holds:

- a parity claim that the pinned NVDA source or a capture contradicts, or a spoken expectation taken from anywhere but a capture;
- a test that accepts two outcomes, times something, exercises a copy of the production logic, or passes under a mutation that removes the behaviour it names;
- a call, wake-up, lock, timer or allocation added on the event or speech path that the ledger (`docs/performance.md`) does not state;
- a race hidden by a retry, a sleep or a bound added without evidence that the proper fix is not enough;
- a document that describes a removed mechanism, or a decision attributed to Dickson that he did not take;
- a removal or a protocol change that a remaining caller, document or wire version still depends on.

## What a finding must carry

Every finding gives the file and line, the defect in one sentence, and a concrete counterexample, a violated invariant or the missing evidence. A finding without one of those is an observation and is not posted as a finding. Style, diagnostic and cleanup-only remarks are left out: they bury the findings that matter.

Each finding is tagged:

- **Important**: wrong behaviour, a wrong parity claim, a cost on the per-event path, a test that proves nothing, a wrong attribution. Fixed in the review loop.
- **Nit**: wording, a doc that lags the code by a detail, a test that could be tighter without being wrong. Collected, not fixed in the loop.
- **Pre-existing**: not introduced by the change under review. Recorded as a follow-up in `docs/roadmap.md` with its evidence, not fixed in the loop unless the change cannot work without it.

The reviewer's summary starts with the count of Important findings and says plainly when there is nothing to fix.

## Before an Important finding is built

An Important finding is reproduced before anything is built on it: the fixing subagent runs the counterexample (a unit test that fails, a scenario run whose log shows it, a line of the pinned source), or a second read-only reviewer confirms it. A finding that cannot be reproduced is recorded with what was tried, and nothing is built. A finding whose recommended fix changes behaviour beyond the counterexample is a design question for Dickson, not a fix.

## The unit of review

The unit is a completed step, not a commit: the whole implementation of a milestone step or of the scope a brief defines, with its tests, documents and ledger entries, reviewed as one diff against the commit the step started from. A step is complete when its brief's end condition holds and its related scenarios pass. Builders commit as they go; nothing is pushed before the step's review. A step's design is reviewed before implementation (the `port` skill's phase 5), so that the review at the end judges the implementation against a design already agreed, and a finding that the approach is wrong is rare and goes back to the design rather than into a fix round.

## The loop, and when it stops

1. The completed step is reviewed once, in full, by a separate read-only subagent with this file, the targets list, the step's brief and design, and the evidence of its runs.
2. Important findings are reproduced, then fixed in one commit. Nits from the review are fixed in the same commit only when they touch lines the fix already changes; the rest wait.
3. The fix commit is re-reviewed on its diff only, against the counterexamples of the first review. New findings outside that diff are recorded, not pursued.
4. After the second review the loop stops. What remains at Nit or Pre-existing goes into one documents-and-nits commit for the step, which needs no review, or into the roadmap. If the second review still finds an Important defect, the parent asks whether the design is wrong rather than opening a third round.

Trivial changes need no review: a title string, a format fix, a log line, a doc correction, a nits commit. A change is not made trivial by being small; a one-line change to a key, a bound or a drop reason is nontrivial and is reviewed with its step.

## Evidence over reading

A reviewer reads the evidence before the code: the test output, the scenario logs, the mutation checks the implementer reports, the ledger's counts. A claim without evidence in the report is a finding. The end-to-end suite and the mockapp tests are the judge of behaviour; a review that finds no defect does not make a change correct, and a suite failure after a clean review is answered by asking which check should have caught it, then adding that check, not another review round.

## Decisions

A decision the parent session takes while Dickson is away is recorded as "(Fable, <date>, pending Dickson's acceptance)" in documents and commit messages, listed for him, and never written as his. A decision he took keeps "(Dickson, <date>)".
