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
  design records of finished work and the Windows background explainers.
- [The Extensions Development Guide](../books/extensions/src/SUMMARY.md),
  written in M5.
- [The User Guide](../books/user/src/SUMMARY.md), not written yet.

A design record moves from `docs/design/` into the Development guide's
Explanation section, under `books/dev/src/explanation/design/`, in the
commit that cuts it down once its work is built, because only finished
work is published. A published page names a document here by its full
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
be kept current; the design records below and the finished ones in
[the Development guide](../books/dev/src/explanation/design/readme.md)
hold the reasons behind the work;
[the UIA faults](../books/dev/src/reference/uia-bugs.md) say what UI
Automation gets wrong.

**Running and debugging things.**
[The tooling guides](../books/dev/src/how-to/readme.md) cover setup,
`verbatim-inspect`, `mockapp`, measuring, the end-to-end suite, the VM
harness, NVDA captures and troubleshooting.

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
  - [communication.md](design/communication.md) — every pipe and thread
    from an event or a key to sound; published once its audio step is
    built.
  - [ownership-milestone-rules.md](design/ownership-milestone-rules.md) —
    the ownership rules each planned milestone follows, and the evidence
    behind them.
  - [focus-pipeline-rework.md](design/focus-pipeline-rework.md) — the
    reconciliations between the listener, the outposts and Core to
    remove.
  - [focus-pipeline-rework-remaining.md](design/focus-pipeline-rework-remaining.md)
    — its remaining builds and the title rule, after the ownership
    refactor.
  - [uia-facts.md](design/uia-facts.md) — the design for the UIA facts
    build: the `uia-facts` workspace, its provider program and a folder
    per UI Automation fact; deleted once that build is finished.
  - [focus-pipeline-audit.md](design/focus-pipeline-audit.md) — every
    difference from NVDA's focus handling found on 2026-10-29.
  - [cross-process-calls-study.md](design/cross-process-calls-study.md)
    — every call that can reach another application, measured, with a
    verdict on the rule that only an application's own outpost calls it.
  - [caret-selection-rework.md](design/caret-selection-rework.md) — one
    caret baseline and one request per key.
  - [windows-terminal-twin-focus.md](design/windows-terminal-twin-focus.md)
    — the rule for Windows Terminal's second terminal identity.
  - [extensions.md](design/extensions.md) — the extension system,
    published once M5 builds it.
  - [security-model.md](design/security-model.md) — the threat model
    (assets, trust boundaries, adversaries and classes of threat), the
    architectural mitigations for each class, the gaps in today's code,
    and a comparison with NVDA and other screen readers; published once
    Dickson has vetted it.
  - [m5-extensions-work.md](design/m5-extensions-work.md) and
    [m6-native-synth-work.md](design/m6-native-synth-work.md) — the order
    of work and the tests for M5 and M6.
