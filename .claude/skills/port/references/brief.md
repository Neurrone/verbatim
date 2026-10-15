# Subagent briefs and reports

Every brief the parent writes to a research, implementation, or review
subagent has the parts below, each as a heading. The subagent starts with
nothing but the brief, so the brief carries everything it needs.

## The slice and its end condition

What the slice delivers, stated so that a reader can tell when it is done.

## Done so far and what remains

What earlier slices or subagents finished, with commit hashes, and what this
subagent is to do.

## What to read

The exact documents and the named sections in them: a parity entry by its
name, a heading in a `docs/nvda/` file, a numbered section of a design
document. Not "read the roadmap", because an open-ended reading list fills
the subagent's context with what it does not need. Read parity entries and
NVDA or design sections with
`python -I .claude/skills/port/scripts/entry.py <file> "<name>"`.

## Standing rules

Point to `.claude/skills/port/references/rules.md`, or paste it.

## Build and test policy

- Build into your own target directory, so that parallel subagents do not
  wait on each other's build lock.
- At most two building subagents run at once, because the machine runs short
  of memory and disk beyond that.
- Run only the end-to-end scenarios related to the change; never the full
  suite unless this brief says so.
- Never push.

## Commit rule

Commit on main without signing (`git commit --no-gpg-sign`), because signing
needs a YubiKey touch. End the message with the two attribution lines given
here in the brief.

## Cleanup

Before reporting, stop every process you started, remove worktrees you
created, and release any held keys, because leftovers break the next run.

## Report structure

Report in this order, with no length limit:

1. Outcome: what works and what does not.
2. Commits made, with hashes.
3. Every failure, with its root cause or, if the cause is unknown, the
   evidence so far.
4. Open decisions for Dickson, each with a recommendation.
5. Evidence: which tests and scenarios ran, with their results.
6. Anything else the parent needs.

Cut nothing material to fit a length. Leave out narration of the steps
taken, because the parent does not need it. The order is fixed because the
parent reads the outcome first to decide whether to act at all.
