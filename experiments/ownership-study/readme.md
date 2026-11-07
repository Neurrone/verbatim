# The ownership study

The evidence behind `docs/design/ownership.md`, the model of one owner for every fact: a read-only study of main at 5797c075, of which facts of Verbatim are decided in more than one process or thread, which defects came from that, and what each part should change or keep, with its prior art and the audit of the structure between processes. Nothing here was built or run; they read the code and its history. Whether `CoCancelCall` ends a UIA client call is in `uia-facts/nothing-cancels-a-uia-call/`, run the same day. The designs that came of it are in `docs/design/`: the model (`docs/design/ownership.md`), the refactor's builds (`docs/design/ownership-milestone-rules.md`), and the focus pipeline rework's remaining builds (`books/dev/src/reference/adr/0001-focus.md`).

- `architecture-study.md`: the inventory of facts with more than one owner, the reasons the processes are split, prior art, and the alternatives weighed. It calls the per-application state machine "the judge"; the design calls it the outpost's reducer.
- `audit-structure.md`: the audit of the structure between processes.
- `prior-art.md`: the prior art the model draws on, cited by its principles.

The audits name code as it was at 5797c075; line numbers drift.
