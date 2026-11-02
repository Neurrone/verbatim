# The logic tests study

The evidence behind `docs/design/logic-tests.md`: which of the defects fixed on main from 2026-10-01 to 2026-10-31 generated state-machine tests and a simulation of the reducers could have found. Nothing here was built or run against Windows; the classification read commit messages and diffs.

## Method

- The commits of main from 2026-10-01 to 2026-10-31 up to a411721: 1,841.
- Those whose message matched a fix (`fix`, `correct` or `repair` at the start, or `fixes`, `defect`, `race`, `lost`, `never`, `wrong`, `regression` or `bug` anywhere), less those whose subject starts with `docs`, `ledger`, `roadmap`, `record` or `nits`: 586.
- Five agents classified them, each reading the commit's message and its file list and, where the message did not settle it, the diff of its main source files. Each commit got one category:
  - N, not a fix of a product defect: tests, the harness, tooling, documents, refactors, or features whose message uses one of the words.
  - L1, a defect of ordering, lifecycle, bookkeeping or concurrency between owners, which an invariant checked over random sequences of inputs catches.
  - L2, a rule that gave a wrong result, which needs an example test with a known output unless a general property applies.
  - W, a wrong assumption about how Windows, UI Automation, MSAA, COM, an application, a device or an engine behaves.
  - P, performance only.
- For L1 and L2, whether the faulty file is pure today. Files that have since moved into the outpost's reducer (`intake`, the gap record) and the listener's scheduler's defects were counted as pure by hand.
- `design-review-1.md` is the adversarial review of the plan, and `decisions-2026-10-31.md` Dickson's decisions on it.
- `research/` holds the research of 2026-10-31 on property-based, model-based and deterministic simulation testing elsewhere, the Rust tools, and how screen readers test: four researchers' notes and the report drawn from them (`research/report.md`).
- `fix-commits.tsv` holds every commit's category, the faulty file, the invariant that would catch it, an earlier commit fixing the same defect, and a sentence of evidence.

## Results

- N 373, P 19, L1 67, L2 56, W 64, after 7 duplicates were removed: 187 behaviour defects.
- L1 by where the faulty code was: 29 in code pure today, 25 in logic outside a reducer, 9 in hand-offs between threads (631ee56d, de7f481e, d6f627a5, d0ec3a89, fc899340, 76180512, 2bdfffb7, ff25f3dd, 46b993fa), 4 in Windows-bound crates (b078ee81, 0d1e7802, 1bfb6f87, 61a4bfa1).
- Of the 13 not in reach of the reducers' tests: 6 were failure paths no run takes (de7f481e, d6f627a5, 76180512, 1bfb6f87, 61a4bfa1, 0d1e7802), 2 locks held across calls into an application (d0ec3a89, fc899340), 2 readiness announced too early (ff25f3dd, 46b993fa), 1 a timeout judged in the wrong place (2bdfffb7), 1 a lost wake (631ee56d), and 1 a marker taken from another event thread, which only the tests met (b078ee81).
- L2: 38 in pure or platform-neutral code; a general property would have caught 5.

The adversarial review rechecked 35 of the L1 rows against their commits (`design-review-1.md`): about 18 held as distinct ordering defects within reach, about 10 were misclassified, and about 7 were later rounds of one mechanism that the first pass did not mark as the same defect. Its estimate, which the design uses: 35 to 45 distinct ordering defects, 20 to 30 catchable by generated tests of pure code, 8 to 12 reachable by the first tests in code that survives the focus pipeline rework. The figures above are the first pass's, kept as classified.

## Caveats

- The word match misses fixes whose messages use other words, and counts commits, not defects: two rounds on one mechanism count twice unless marked as the same defect.
- The agents judged most commits from their messages and file lists. Ten classifications chosen at random held when checked by hand, but the review's recheck of 35 L1 rows found about a third misclassified, so the L1 counts are upper bounds.
- "Could have been caught" assumes the invariant was written first; some invariants were learned from the defect they would catch.
