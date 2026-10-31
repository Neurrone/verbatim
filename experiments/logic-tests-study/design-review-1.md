# The adversarial review of the logic tests plan, 2026-10-31

A read-only review of the plan's first draft (`docs/design/logic-tests.md` and the roadmap's M4.5 entry before their revision), asked whether its costs pay off and whether the design as a whole is sound. It found 12 Important findings; the checked ones held, and each is answered in the revised design.

## Important findings, and what the revision did

1. The plan built on code the next build rewrites: the rework's second build changes the hand-over's hold, the gap and Core's request table. Answered: the generated tests moved to M4.6, after that build.
2. The split's stated reason, that the parts cannot be tested apart, was false: the reducer, the scheduler, the store and the policy have tests today. Answered: the reason is now clean ownership by role and purity enforced by crate boundaries.
3. Splitting the request table and the object store's ledger copies one fact into two places. Answered: both splits were cut.
4. The simulation would assume guarantees stronger than the real transport, whose merge, priority and overflow rules live in shells, and the recorded worker answers did not exist. Answered: the simulation is deferred until both exist and M4.6 meets its thresholds.
5. Guarding marks keeps a flag shared between threads (the relay mailbox's `posted`), which the design principles forbid. Answered: such flags are removed; guards only for marks within one thread.
6. The supervisor's owner was made pure without noting the decision of 2026-10-30 that it was not. Answered: Dickson revisited it on 2026-10-31; the decision was a scope default of the refactor.
7. Decisions credited to Dickson had no record. Answered: `decisions-2026-10-31.md`; the M6 host protocol is marked as pending his acceptance.
8. The licence check would let a platform-neutral crate depend on a GPL-tier crate. Answered: the check covers that dependency too.
9. The stop rule could not fail. Answered: two measures with thresholds set before the tests are written.
10. No seed policy. Answered: fixed seeds and counts in the gate, exploration apart.
11. Wasmtime's epoch belongs to the engine, so an abandoned worker's timer would interrupt its replacement's hook. Answered in M5's step 0b.
12. The study's counts were inflated by misclassifications and repeated rounds. Answered: the readme gives the recheck's figures.

## Nits

- Moving the generation into `verbatim-core` must take its `Sealed` token with it.
- A crate boundary could replace part of the text-based ownership scan.
- "A returning focus is spoken" is a liveness property, not an invariant of every state.
- The debug lock check runs in the debug builds whose latency the end-to-end suite measures.
- The workspace-hack's platform list has no Linux entry.
- Separate executables barely change private memory, since code pages are shared.
- Global modules add a Core thread and a hop per query, and the extension world has no clock import for reading the time.

## The recheck of the counts

35 rows classified L1 were checked against their commits: about 18 held as distinct ordering defects within reach, about 10 were misclassified (wrong rules, clock resolution, Windows behaviour, latency), and about 7 were later rounds of one mechanism, such as the hand-over's five rounds (d40f97ac, e22d3a4c, 5d179968, 172ac80c, 280213dc). Estimate: 35 to 45 distinct ordering defects, 20 to 30 catchable, 8 to 12 reachable by the first tests in code that survives the rework.
