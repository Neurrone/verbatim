# Where responsibilities go, and what crosses a boundary

Read during phase 4. Map responsibilities onto the architecture that exists
(`docs/architecture.md`), rather than inventing an NVDA-shaped one.

## Which part owns each responsibility

Decide which responsibility belongs in:

- the focus listener;
- a per-application outpost;
- the Core and its reducer;
- a Windows-specific accessibility crate;
- the normalized model;
- text processing;
- presentation;
- speech or audio;
- another existing subsystem.

## Process and thread boundaries

For work that crosses a process or thread boundary, identify:

- which process receives the original information;
- which process is allowed to make each potentially blocking call;
- which thread owns mutable state;
- what crosses IPC;
- what must not cross IPC;
- how identity is represented across the boundary;
- ordering guarantees;
- cancellation or staleness rules;
- timeout behavior;
- process failure and restart behavior.

Write the answers into the design document, not only into the conversation,
so that implementation subagents read the same decisions.
