# The ownership study

The evidence behind `docs/design/ownership.md`, the model of one owner for every fact: a read-only study and audits of main at 5797c075, of which facts of Verbatim are decided in more than one process or thread, which defects came from that, and what each part should change or keep; and the three adversarial reviews of the design. Nothing here was built or run; the audits read the code and its history, and the stress test reasoned from NVDA's pinned source. Whether `CoCancelCall` ends a UIA client call is in `experiments/cocancelcall-does-not-end-a-uia-client-call/`, run the same day. The designs that came of it are in `docs/design/`: the model (`docs/design/ownership.md`), the refactor's builds (`docs/design/ownership-milestone-rules.md`), and the focus pipeline rework's remaining builds (`docs/design/focus-pipeline-rework-remaining.md`).

- `architecture-study.md`: the inventory of facts with more than one owner, the reasons the processes are split, prior art, and the alternatives weighed. It calls the per-application state machine "the judge"; the design calls it the outpost's reducer.
- `architecture-study-answers.md`: Dickson's ten questions on the study, answered.
- `audit-core-input.md`, `audit-readers.md`, `audit-structure.md` and `audit-output.md`: the four audits, of Core and input, the readers, the structure between processes, and output and configuration.
- `foreground-batch-stress.md`: a stress test of a batch rule for a foreground change and a focus, superseded by the title rule.
- `design-review-1.md` and `design-review-2.md`: the two adversarial reviews of the design before it was restructured, and `design-review-3.md`, the review of the short model; their findings are listed as known traps in the builds document.
- `decisions-2026-10-30.md`: Dickson's decisions of that day, as relayed.

The audits name code as it was at 5797c075; line numbers drift.
