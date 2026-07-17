# Verbatim documentation

The index of everything under `docs/`, with reading paths for the
common reasons to be here. Every document follows the repository's
writing conventions: prose and lists, no ASCII diagrams or arrow
chains, readable with a screen reader in source and rendered form.

## Reading paths

**New to the project.** Read the repository `CLAUDE.md`, then
[Walkthroughs](walkthroughs.md) (three end-to-end narratives), then
[Architecture](architecture.md) (decisions of record D1 onward and the
system design), keeping [the glossary](glossary.md) at hand for the
project's invented vocabulary. If Windows APIs are unfamiliar,
[the explainers](explainers/readme.md) have a reading order; start
with its first three files before the architecture doc's backend
sections.

**Working on NVDA parity.** [The parity ledger](parity.md) is the
per-behavior checklist and its statuses are the definition of done;
each entry links the relevant file in [docs/nvda](nvda/readme.md),
which documents how NVDA implements things, cited against the pinned
submodule commit.

**Building a feature.** The milestone scoping is in
[the roadmap](roadmap.md) (completed milestones are archived in
[roadmap-done.md](roadmap-done.md)); the crate you are changing has a
guide in [docs/crates](crates/readme.md) that must be kept current;
the NVDA reference for the behavior is in [docs/nvda](nvda/readme.md).

**Running and debugging things.** [The tooling guide](tooling.md)
covers `verbatim-inspect`, `mockapp`, the end-to-end suite, and the
troubleshooting traps; [the VM harness guide](vm.md) covers every
`cargo xtask vm` verb and rebuilding the golden image.

## The full map

- [Architecture](architecture.md) — decisions of record (D-numbers),
  the process and thread model, backend strategy, testing strategy,
  and top risks. The document code comments cite by section number.
- [Roadmap](roadmap.md) and [completed milestones](roadmap-done.md) —
  what is ahead, and the archived evidence of what is done.
- [Crate guides](crates/readme.md) — one file per crate, in dependency
  order.
- [Walkthroughs](walkthroughs.md) — the life of a focus change, of a
  command keystroke, and of an E2E run, across crate boundaries.
- [Parity ledger](parity.md) — every NVDA-parity claim and its
  verification status.
- [docs/nvda](nvda/readme.md) — how NVDA implements its nontrivial
  features, source-cited; NVDA-only content by rule.
- [Explainers](explainers/readme.md) — Windows API background (COM,
  the accessibility APIs, IPC, audio and speech APIs) plus the Rust
  concurrency vocabulary Verbatim uses.
- [Tooling](tooling.md) and [the VM harness](vm.md) — driving the
  project day to day.
- [Glossary](glossary.md) — Verbatim's invented vocabulary, each term
  linked to its defining document.
