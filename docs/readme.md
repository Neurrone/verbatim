# Verbatim documentation

The index of everything under `docs/`, with reading paths for the
common reasons to be here. Each document opens with its own status
where it has one; this index names what each holds.

## Reading paths

**New to the project.** Read the repository `CLAUDE.md`, then
[Walkthroughs](walkthroughs.md), then [Architecture](architecture.md),
keeping [the glossary](glossary.md) at hand for the project's invented
vocabulary. If Windows APIs are unfamiliar,
[the explainers](explainers/readme.md) have a reading order.

**Working on NVDA parity.** [The parity ledger](parity/readme.md) is the
per-behaviour checklist, and its statuses are the definition of done;
each entry links the file in [docs/nvda](nvda/readme.md) that documents
how NVDA does it.

**Building a feature.** [The roadmap](roadmap.md) scopes the work; the
crate you change has a guide in [docs/crates](crates/readme.md) that
must be kept current; the design records below hold the reasons behind
the work; [the UIA faults](uia-bugs.md) say what UI Automation gets
wrong.

**Running and debugging things.** [The tooling guides](tooling/readme.md)
cover setup, `verbatim-inspect`, `mockapp`, measuring, the end-to-end
suite, the VM harness, NVDA captures and troubleshooting.

## The full map

- [Architecture](architecture.md) — the decisions of record, the process
  and thread model, backend strategy, testing strategy, the crate map
  and the top risks.
- [Roadmap](roadmap.md) — the work ahead, milestone by milestone, with
  the open follow-ups.
- [Completed milestones](roadmap-done.md) — what each finished milestone
  delivered.
- [Crate guides](crates/readme.md) — one file per crate: its public API
  and how its intricate parts work.
- [Walkthroughs](walkthroughs.md) — a focus change, a command key, an
  utterance, a query, recovery from failures and an end-to-end run,
  followed across processes and crates.
- [Parity ledger](parity/readme.md) — what NVDA does for each behaviour
  and where Verbatim stands.
- [docs/nvda](nvda/readme.md) — how NVDA implements its features, cited
  against the pinned source.
- [Explainers](explainers/readme.md) — Windows API background and the
  Rust concurrency vocabulary Verbatim uses.
- [Tooling](tooling/readme.md) — which command to type, one guide per
  purpose.
- [The testing standard](testing.md) — what every test must meet.
- [Performance](performance.md) — the operation ledger: what counts as a
  call, and for each operation and backend the call counts CI asserts
  and the measured costs.
- [Faults in UI Automation](uia-bugs.md) — the faults Verbatim has
  established in UIA and in applications' providers, each with its
  reproduction test.
- [Experiments](../experiments/readme.md) — the experiments behind the
  UIA faults and the surveys behind design choices, one folder each.
- [Glossary](glossary.md) — Verbatim's invented vocabulary.
- Design records — the problem, the alternatives weighed, the reasons
  and the limits behind each piece of work:
  - [communication.md](design/communication.md) — every pipe and thread
    from an event or a key to sound.
  - [focus-pipeline.md](design/focus-pipeline.md) — how focus events
    reach Verbatim.
  - [listener-without-connections.md](design/listener-without-connections.md)
    — why only an application's own outpost connects to it.
  - [focus-pipeline-rework.md](design/focus-pipeline-rework.md) — the
    reconciliations between the listener, the outposts and Core to
    remove.
  - [focus-pipeline-outpost-reads.md](design/focus-pipeline-outpost-reads.md)
    — the alternative in which the outposts, not the listener, read each
    UIA focus.
  - [focus-pipeline-audit.md](design/focus-pipeline-audit.md) — every
    difference from NVDA's focus handling found on 2026-10-29.
  - [caret-selection-rework.md](design/caret-selection-rework.md) — one
    caret baseline and one request per key.
  - [uia-shapes.md](design/uia-shapes.md) — every cached UIA read
    declared with its request.
  - [wake-ups.md](design/wake-ups.md) — the sources of Verbatim's
    wake-ups and the reason for each fix.
  - [terminal-model.md](design/terminal-model.md) — how terminal screens
    change, and the rules Verbatim reads them by.
  - [key-holding.md](design/key-holding.md) — holding caret keys as NVDA
    does.
  - [windows-terminal-twin-focus.md](design/windows-terminal-twin-focus.md)
    — the rule for Windows Terminal's second terminal identity.
  - [extensions.md](design/extensions.md) — the extension system.
  - [m5-extensions-work.md](design/m5-extensions-work.md) and
    [m6-native-synth-work.md](design/m6-native-synth-work.md) — the order
    of work and the tests for M5 and M6.
