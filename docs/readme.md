# Verbatim's working documents

The documents under `docs/` are the project's working material, read by
the agents that build Verbatim and not published: the plans, the design
records of work still being designed or built, the NVDA parity ledger and
the notes on how NVDA works. Each document opens with its own status where
it has one; this index names what each holds.

The documentation meant for people is published from `books/`, one mdBook
each, arranged after the Diataxis framework: tutorials, how-to guides,
reference and explanation.

- [The Verbatim Development Guide](../books/dev/src/SUMMARY.md): the
  architecture and its decisions of record, the walkthroughs, the crate
  guides, the tooling guides, the testing standard, test coverage, the
  performance ledger, the faults in UI Automation, the glossary, the
  decision records, the explanations of finished designs and the Windows
  background explainers.
- [The Extensions Development Guide](../books/extensions/src/SUMMARY.md),
  written in M5.
- [The User Guide](../books/user/src/SUMMARY.md), not written yet.

Once its work is built, a design record under `docs/design/` becomes two
published pages: an explanation of how the work functions today, under
`books/dev/src/explanation/design/`, and a numbered decision record under
`books/dev/src/reference/adr/`, with the problem, the options weighed and
the limits; the record under `docs/design/` is then deleted, because only
finished work is published. A published page names a document here by its full
GitHub address, since the site carries nothing from `docs/`
(`cargo xtask docs`, rule 5).

## Reading paths

**New to the project.** Read the repository `CLAUDE.md`, then
[Walkthroughs](../books/dev/src/explanation/walkthroughs.md), then
[Architecture](../books/dev/src/explanation/architecture.md), keeping
[the glossary](../books/dev/src/reference/glossary.md) at hand for the
project's invented vocabulary. If Windows APIs are unfamiliar,
[the explainers](../books/dev/src/explanation/windows/readme.md) have a
reading order.

**Working on NVDA parity.** [The parity ledger](parity/readme.md) is the
per-behaviour checklist, and its statuses are the definition of done;
each entry links the file in [docs/nvda](nvda/readme.md) that documents
how NVDA does it.

**Building a feature.** [The roadmap](roadmap.md) scopes the work; the
crate you change has a guide in
[the crate guides](../books/dev/src/reference/crates/readme.md) that must
be kept current; the design records below,
[the decision records](../books/dev/src/reference/adr/readme.md) and
[the finished designs](../books/dev/src/explanation/design/readme.md)
hold the reasons behind the work;
[the UIA faults](../books/dev/src/reference/uia-bugs.md) say what UI
Automation gets wrong.

**Running and debugging things.**
[The tooling guides](../books/dev/src/how-to/readme.md) cover setup,
`verbatim-inspect`, `mockapp`, measuring, the end-to-end suite, NVDA
captures and troubleshooting.

## The documents here

- [Roadmap](roadmap.md) — the work ahead, milestone by milestone, with
  the open follow-ups.
- [Completed milestones](roadmap-done.md) — what each finished milestone
  delivered.
- [Parity ledger](parity/readme.md) — what NVDA does for each behaviour
  and where Verbatim stands.
- [docs/nvda](nvda/readme.md) — how NVDA implements its features, cited
  against the pinned source.
- [Known failing and flaky tests](flaky-tests.md) — each test known to fail,
  with its cause and the work that fixes it.
- [Experiments](../experiments/readme.md) — the experiments behind the
  UIA faults and the surveys behind design choices, one folder each.
- Design records of work still being designed or built:
  - [readers-tell-core-asks-build3.md](design/readers-tell-core-asks-build3.md)
    — the approved design of the terminals build of "Readers tell, Core
    asks" (ADR 0003), which is built; the terminal code's comments cite
    its tables until the terminal rebuild replaces that code.
  - [terminal-rebuild.md](design/terminal-rebuild.md) — the study for
    M6.4's rebuild of the terminal reading on NVDA's loop: the inventory,
    state tables, the defects replayed, and each Verbatim-only feature's
    cost, with Dickson's rulings so far.
  - [communication.md](design/communication.md) — every pipe and thread
    from an event or a key to sound; published once its audio step is
    built.
  - [ownership-milestone-rules.md](design/ownership-milestone-rules.md) —
    the ownership rules each planned milestone follows, and the evidence
    behind them.
  - [extensions.md](design/extensions.md) — the extension system,
    published once M5 builds it.
  - [security-model.md](design/security-model.md) — the threat model
    (assets, trust boundaries, adversaries and classes of threat), the
    architectural mitigations for each class, the gaps in today's code,
    and a comparison with NVDA and other screen readers; published once
    Dickson has vetted it.
  - [containment.md](design/containment.md) — M4.7: what a panic or a
    native crash ends in each process and thread, the GUI thread's
    rules, the hook's pass-through and watcher, children's liveness and
    the containment scan.
  - [guardian.md](design/guardian.md) — M4.8: the guardian process that
    restarts Core, Core's one panic path, the single instance, hang
    detection, dumps, the restart breaker and the timeouts.
  - [lints-and-unsafe.md](design/lints-and-unsafe.md) — M4.9: the
    panic-free lints, fallbacks removed by construction, unsafe code in
    wrapper crates, bounds on sizes applications control, terminal
    alignment and fuzzing.
  - [m5-extensions-work.md](design/m5-extensions-work.md) and
    [m6-native-synth-work.md](design/m6-native-synth-work.md) — the order
    of work and the tests for M5 and M6.
  - [m5-build-a.md](design/m5-build-a.md) — M5's build A: snapshot
    properties, application identity, rules as data, the loader, the
    extension lifecycle, configuration at a version and the log.

## The simplification

The phase the roadmap places first (Dickson, 2026-11-20). Working material: a record is deleted when the last build that cites it is built, and the plan when the phase ends.

- [simplification-review/plan.md](simplification-review/plan.md) — where a session starts: why, Dickson's rulings, the rules every build follows, the builds in order and the test of done.
- [simplification-review/builds.md](simplification-review/builds.md) — each build's inventory by finding, what is captured first, its budget and how it is known to be done.
- [simplification-review/build8-operations.md](simplification-review/build8-operations.md) — build 8's study, part one: the closed list of operations and the batch, with Dickson's rulings of 2026-11-20 recorded in builds.md.
- [simplification-review/REPORT.md](simplification-review/REPORT.md) — the review's report: the assessment, the defects confirmed, sizes against NVDA and the questions in full.
- The verified findings, one record for each area, each with its size table, findings with file and line, test counts and dropped claims:
  - [simplification-review/merged-access-apis.md](simplification-review/merged-access-apis.md)
  - [simplification-review/merged-core-focus.md](simplification-review/merged-core-focus.md)
  - [simplification-review/merged-core-keys.md](simplification-review/merged-core-keys.md)
  - [simplification-review/merged-core-queue.md](simplification-review/merged-core-queue.md)
  - [simplification-review/merged-dialog-study.md](simplification-review/merged-dialog-study.md)
  - [simplification-review/merged-documents.md](simplification-review/merged-documents.md)
  - [simplification-review/merged-e2e-harness.md](simplification-review/merged-e2e-harness.md)
  - [simplification-review/merged-e2e-scenarios.md](simplification-review/merged-e2e-scenarios.md)
  - [simplification-review/merged-listener.md](simplification-review/merged-listener.md)
  - [simplification-review/merged-mockapp.md](simplification-review/merged-mockapp.md)
  - [simplification-review/merged-model-protocol.md](simplification-review/merged-model-protocol.md)
  - [simplification-review/merged-outpost.md](simplification-review/merged-outpost.md)
  - [simplification-review/merged-reader-call-tests.md](simplification-review/merged-reader-call-tests.md)
  - [simplification-review/merged-settings-gui.md](simplification-review/merged-settings-gui.md)
  - [simplification-review/merged-speech-audio.md](simplification-review/merged-speech-audio.md)
  - [simplification-review/merged-supervisor-app.md](simplification-review/merged-supervisor-app.md)
  - [simplification-review/merged-terminals.md](simplification-review/merged-terminals.md)
  - [simplification-review/merged-text.md](simplification-review/merged-text.md)
  - [simplification-review/merged-tools.md](simplification-review/merged-tools.md)
  - [simplification-review/merged-uia-facts.md](simplification-review/merged-uia-facts.md)
