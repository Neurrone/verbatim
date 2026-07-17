# Glossary

Verbatim's invented vocabulary, alphabetical. Each entry gives the
short meaning and links the document that defines it properly. Terms
NVDA also uses (navigator object, review cursor, browse mode) keep
their NVDA meanings and are documented in [docs/nvda](nvda/readme.md).

- **Agent** — `verbatim-agent`, the in-guest test doorway: launches
  processes and tunnels the control plane over TCP for the E2E suite.
  [verbatim-agent](crates/verbatim-agent.md).
- **Announce lane** — the single FIFO thread per outpost that runs
  focus-fact announcement jobs to completion in arrival order,
  guaranteeing window-before-control ordering.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Announce poll** — the `AnnounceFocus` fallback: a synthetic
  window-then-control announcement by polling, demoted by D13 to
  cover the listener's respawn gap. [verbatim-outpost](crates/verbatim-outpost.md).
- **Arbitration** — the per-window choice of backend (UIA or
  MSAA/IA2), decided by class lists lifted from NVDA plus a live
  provider probe; the result is a **verdict**, cached per window.
  [verbatim-outpost](crates/verbatim-outpost.md); NVDA's referee is in
  [The UIA client](nvda/uia.md).
- **Backend** — one client stack over an accessibility API: UIA
  (`verbatim-uia`), MSAA/IA2 (`verbatim-ia2`), later JAB.
  [Architecture](architecture.md) section 4.
- **Capture synth** — the test synthesizer that records the flattened
  speech it was asked to speak instead of producing audio; what E2E
  assertions read. [verbatim-synth-capture](crates/verbatim-synth-capture.md).
- **Cold case** — a window arbitration has never seen: the first fact
  for it resolves a real verdict inline via a deadline-guarded probe
  (D13's amended rule) rather than announcing provisionally.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Control plane** — the one authenticated protocol (JSON over the
  control pipe) serving dev tooling now and remote support later
  (D8). [verbatim-control](crates/verbatim-control.md).
- **Effect** — a reducer output: an instruction (Speak, Fetch,
  Activate…) the shell executes; the reducer itself does no I/O.
  [verbatim-core](crates/verbatim-core.md).
- **Fact** (focus fact) — one focus-relevant observation captured by
  the focus listener from an OS event's own payload and routed to the
  target's outpost for announcement (D13); a `ListenerFact` becomes a
  `DeliveredFact` once routed. [verbatim-outpost](crates/verbatim-outpost.md).
- **Flight recorder** — the bounded ring buffer of reducer inputs,
  dumpable to a versioned JSON-lines file and deterministically
  replayable. [verbatim-core](crates/verbatim-core.md).
- **Focus listener** — the permanent, stateless outpost mode holding
  the desktop-global focus registrations under a hard
  never-make-a-cross-process-call rule (D13).
  [Architecture](architecture.md) section 1.
- **Generation** — the counter on a supervisor map entry that lets
  the respawn path detect it is looking at a stale entry, so
  deliberate retirement is not undone by an automatic respawn.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Golden image** — the Packer-built Windows VM baseline the Hyper-V
  harness imports, deploys to, and checkpoints. [The VM harness](vm.md).
- **Idle retirement** — shutting down an outpost whose application
  has not held foreground for two minutes (risk R2's memory
  mitigation). [verbatim-outpost](crates/verbatim-outpost.md).
- **Last-observation-wins** — the reducer's staleness rule: a focus
  event observed earlier than the focus currently held does not move
  focus; windows get a carve-out (spoken late rather than lost).
  [verbatim-core](crates/verbatim-core.md).
- **Normalized model** — `verbatim-model`'s API-agnostic vocabulary
  of nodes, roles, states, and events that both backends map into.
  [verbatim-model](crates/verbatim-model.md).
- **Outpost** — the per-application process owning that app's event
  hooks, queries, and announcements (D9); also the binary
  `verbatim-outpost.exe`, which runs as an app outpost or, with
  `--listener`, as the focus listener.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Parked worker** — a query-pool thread abandoned mid-call because
  its deadline expired (a thread stuck in a hung app's COM call
  cannot be safely killed); recovery ladder rung 2's bounded garbage.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Presentable container** — an ancestor that qualifies for a
  focus-entry announcement under the NVDA-parity exclusion filter.
  [verbatim-core](crates/verbatim-core.md); NVDA's rule is in
  [Object model](nvda/object-model.md).
- **Provisional rule** — the degraded dual-backend behavior when a
  cold case's probe times out: the MSAA fact announces, the UIA fact
  drops. [verbatim-outpost](crates/verbatim-outpost.md).
- **Query pool** — the deadline-guarded worker threads on which every
  outpost cross-process call runs; the founding rule's enforcement
  point. [verbatim-outpost](crates/verbatim-outpost.md).
- **Recovery ladder** — the escalation for misbehaving outposts:
  rung 1, per-call deadlines; rung 2, park the stuck worker and spawn
  a replacement; rung 3, kill and respawn a wedged outpost; plus
  plain respawn-on-crash. [Architecture](architecture.md) section 1.
- **Reducer** — the pure functional core: `reduce(state, input)`
  returns the next state and effects, no I/O, no clocks (architecture
  section 2). [verbatim-core](crates/verbatim-core.md).
- **Scan mode** — Verbatim's planned document-reading mode family
  (the browse-mode analog), landing with M6.
  [Architecture](architecture.md) section 8.
- **Scenario** — one named, registered E2E test: setup, body,
  teardown, artifacts, and a `#[test]` wrapper sharing its name.
  [verbatim-e2e](crates/verbatim-e2e.md).
- **Snapshot version** — the monotonic version stamped on a node
  snapshot; the reducer distrusts events carrying versions older than
  the last seen per source. [verbatim-core](crates/verbatim-core.md).
- **Span** — one typed piece of an utterance (label, role, value,
  state, text run) per D12; flattened to text by a **theme** at the
  last pipeline stage. [verbatim-speech](crates/verbatim-speech.md).
- **Trace id** — the correlation id minted at the triggering input
  and carried through event, reducer, speech, and audio, making
  end-to-end latency timelines possible.
  [verbatim-model](crates/verbatim-model.md).
- **Tunnel** — the agent's byte relay that turns its TCP connection
  into a raw connection to Verbatim's control-plane pipe.
  [verbatim-agent](crates/verbatim-agent.md).
- **Utterance** — the structured unit of speech (a sequence of spans
  plus source metadata) emitted by the reducer (D12).
  [verbatim-speech](crates/verbatim-speech.md).
- **Wedge** — an outpost that is alive but no longer answering
  (missed pongs) or accumulating parked workers; the heartbeat's
  `wedge_decision` kills and respawns it (rung 3).
  [verbatim-outpost](crates/verbatim-outpost.md).
