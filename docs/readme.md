# Verbatim documentation

The index of everything under `docs/`, with reading paths for the
common reasons to be here. Every document follows the repository's
writing conventions: prose and lists, no ASCII diagrams or arrow
chains, readable with a screen reader in source and rendered form.

## Reading paths

**New to the project.** Read the repository `CLAUDE.md`, then
[Walkthroughs](walkthroughs.md) (six end-to-end narratives), then
[Architecture](architecture.md) (decisions of record D1 onward and the
system design), keeping [the glossary](glossary.md) at hand for the
project's invented vocabulary. If Windows APIs are unfamiliar,
[the explainers](explainers/readme.md) have a reading order; start
with its first three files before the architecture doc's backend
sections.

**Working on NVDA parity.** [The parity ledger](parity/readme.md) is the
per-behavior checklist and its statuses are the definition of done;
each entry links the relevant file in [docs/nvda](nvda/readme.md),
which documents how NVDA implements things, cited against the pinned
submodule commit.

**Building a feature.** The milestone scoping is in
[the roadmap](roadmap.md) (completed milestones are archived in
[roadmap-done.md](roadmap-done.md)); the crate you are changing has a
guide in [docs/crates](crates/readme.md) that must be kept current;
the NVDA reference for the behavior is in [docs/nvda](nvda/readme.md);
the [design records](design/) hold the detailed designs behind the work
and the decision log of phase 6; and [the UIA faults](uia-bugs.md) say
what UI Automation gets wrong and what has been tried against each.

**Running and debugging things.** [The tooling guides](tooling/readme.md),
one file per purpose, cover setup and builds, `verbatim-inspect`,
`mockapp`, measuring, the end-to-end suite, NVDA captures, and the
troubleshooting traps; [the VM harness guide](vm.md) covers every
`cargo xtask vm` verb and rebuilding the golden image;
[the NVDA transcript guide](nvda-transcript.md) covers recording what
NVDA says, to decide what Verbatim should say.

## The full map

- [Architecture](architecture.md) — decisions of record (D-numbers),
  the process and thread model, backend strategy, testing strategy,
  and top risks. The document code comments cite by section number.
- [Roadmap](roadmap.md) and [completed milestones](roadmap-done.md) —
  what is ahead, and the archived evidence of what is done.
- [Crate guides](crates/readme.md) — one file per crate, in dependency
  order.
- [Walkthroughs](walkthroughs.md) — the life of a focus change, a
  command keystroke, an utterance, a query, recovery from failures, and
  an E2E run, across process and crate boundaries.
- [Parity ledger](parity/readme.md) — every NVDA-parity claim and its
  verification status.
- [docs/nvda](nvda/readme.md) — how NVDA implements its nontrivial
  features, source-cited; NVDA-only content by rule.
- [Explainers](explainers/readme.md) — Windows API background (COM,
  the accessibility APIs, IPC, audio and speech APIs) plus the Rust
  concurrency vocabulary Verbatim uses.
- [Tooling](tooling/readme.md) and [the VM harness](vm.md) — driving the
  project day to day: [setup](tooling/setup.md),
  [verbatim-inspect](tooling/inspect.md), [mockapp](tooling/mockapp.md),
  [measuring](tooling/measuring.md), [the end-to-end suite](tooling/e2e.md),
  [NVDA captures](tooling/nvda.md), and
  [troubleshooting](tooling/troubleshooting.md).
- [The testing standard](testing.md) — what every test must meet: one
  target and code path, exact assertions, waits for evidence, and no
  hidden failures.
- [Performance](performance.md) — the operation ledger: what counts as
  a cross-process call, cold and steady state, cancelled traces, the
  floor and the ratio, and for each operation and backend the minimum,
  current, and target call counts that CI asserts exactly.
- [Faults in UI Automation](uia-bugs.md) — the faults in Windows' UIA
  that Verbatim has established by experiment, and in applications'
  providers: each one's trigger, symptoms, workarounds tried, NVDA's
  exposure, and its reproduction test in `crates/mockapp/tests/uia_bugs/`.
- [Experiments](../experiments/readme.md) — the experiments behind the
  UIA faults, one folder per entry: each experiment defined by its
  identifier with its setup, result and numbers, beside its scripts,
  tool patches and raw outputs.
- [Text attributes](text-attributes.md) — the survey of every UIA text
  attribute and annotation in Windows Terminal, the console host, and
  Windows 11 Notepad: what each reports, whether it varies, and what
  reading each costs, for choosing which attributes Verbatim fetches.
- [Design records](design/) — the detailed designs and audits behind
  the work, each opening with its status:
  - [communication.md](design/communication.md) — every pipe and thread
    from an event or a key to sound; adopted and built, but for its
    audio step.
  - [focus-pipeline.md](design/focus-pipeline.md) — how events reach
    Verbatim, aligned with NVDA; adopted, stage A step 6 built.
  - [listener-without-connections.md](design/listener-without-connections.md)
    — removing the per-application lock: only an application's own outpost
    connects to it or sends it a remote operation; adopted and built.
  - [focus-pipeline-rework.md](design/focus-pipeline-rework.md) — the
    reconciliations between the listener, the outposts and Core to
    remove, with the study behind them; agreed 2026-10-25, not built.
  - [caret-selection-rework.md](design/caret-selection-rework.md) — one
    caret baseline and one request per key, the layer key holding made
    redundant removed, with the study behind them; agreed 2026-10-25,
    not built.
  - [uia-shapes.md](design/uia-shapes.md) — every cached UIA read
    declared with its request; adopted and built.
  - [terminal-model.md](design/terminal-model.md) — a model of how
    terminal screens change, and the rules Verbatim reads them by;
    accepted 2026-10-23, being built.
  - [key-holding.md](design/key-holding.md) — holding caret keys as
    NVDA does; a prototype on the branch `key-holding`, measured, its
    merge Dickson's decision.
  - [windows-terminal-twin-focus.md](design/windows-terminal-twin-focus.md)
    — the rule for Windows Terminal's second terminal identity; a design
    for M5, not built.
  - [extensions.md](design/extensions.md) — the extension system: modules,
    rules, hooks, capabilities derived from imports, the runtime, ordering,
    budgets, native synthesizers and the porting order; settled 2026-10-22,
    not built.
  - [m5-extensions-work.md](design/m5-extensions-work.md) and
    [m6-native-synth-work.md](design/m6-native-synth-work.md) — the order
    of work and the tests for M5 and M6, which the roadmap entries point to.
  - [outpost-redesign.md](design/outpost-redesign.md) — the outposts,
    listener, and supervisor; adopted and built.
  - [phase6-design.md](design/phase6-design.md) — phase 6's decision
    log; kept for the record.
  - [wake-ups.md](design/wake-ups.md) and
    [safety-audit-2026-10-06.md](design/safety-audit-2026-10-06.md) —
    audit logs: every thread's wake-ups, and the unsafe code.
  - Superseded design records live in [docs/archive/](archive/).
- [The NVDA transcript](nvda-transcript.md) — the NVDA add-on and
  `cargo xtask nvda capture`, for recording what NVDA says in a scenario.
- [Glossary](glossary.md) — Verbatim's invented vocabulary, each term
  linked to its defining document.
