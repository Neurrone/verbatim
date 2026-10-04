# Glossary

Verbatim's invented vocabulary, alphabetical. Each entry gives the
short meaning and links the document that defines it properly. Terms
NVDA also uses (navigator object, review cursor, browse mode) keep
their NVDA meanings and are documented in [docs/nvda](nvda/readme.md).

- **Agent** — `verbatim-agent`, the in-guest test doorway: launches
  processes and tunnels the control plane over TCP for the E2E suite.
  [verbatim-agent](crates/verbatim-agent.md).
- **Abandoned worker** — an outpost worker whose call passed its
  deadline; the watchdog replaces it, since a thread stuck in a hung
  application's COM call cannot be safely killed. If the call ever
  returns, the thread publishes nothing and exits. Recovery ladder rung
  2's bounded garbage. [verbatim-outpost](crates/verbatim-outpost.md).
- **Arbitration** — the per-window choice of backend (UIA or
  MSAA/IA2), decided by class lists lifted from NVDA plus a live
  provider probe; the result is a **verdict**, kept for the window's
  lifetime.
  [verbatim-outpost](crates/verbatim-outpost.md); NVDA's referee is in
  [The UIA client](nvda/uia.md).
- **Backend** — one client stack over an accessibility API: UIA
  (`verbatim-uia`), MSAA/IA2 (`verbatim-ia2`), later JAB.
  [Architecture](architecture.md) section 4.
- **Capture synth** — the test synthesizer that records the flattened
  speech it was asked to speak instead of producing audio; what speech
  pipeline unit tests assert on (E2E runs speak through eSpeak NG).
  [verbatim-synth-capture](crates/verbatim-synth-capture.md).
- **Attention** — the reducer's record of the application and
  top-level window of the most recent foreground change, standing in
  for the system's foreground window; events are accepted or dropped
  against it (D14). [verbatim-core](crates/verbatim-core.md).
- **Cold case** — a window arbitration has never seen: the first event
  for it resolves a real verdict with a provider probe on the worker.
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
- **Golden image** — the Packer-built Windows VM baseline the Hyper-V
  harness imports, deploys to, and checkpoints. [The VM harness](vm.md).
- **Held nodes** — the node references the reducer holds (the focus,
  its ancestors, the last selection, the navigator, and a pending
  navigation's start); Core sends each outpost its own, and the outpost
  keeps the live objects for exactly those and releases the rest.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Idle retirement** — ending an outpost whose application has not
  held attention for two minutes and in which Core holds no nodes
  (risk R2's memory mitigation).
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Message position** — the count of messages carrying node ids an
  outpost has sent, kept the same way by the outpost and Core; Core
  acknowledges a position when it reports its held nodes, so the
  outpost knows which reported nodes Core has seen.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Normalized model** — `verbatim-model`'s API-agnostic vocabulary
  of nodes, roles, states, and events that both backends map into.
  [verbatim-model](crates/verbatim-model.md).
- **Outpost** — the per-application process owning that app's event
  hooks, queries, and announcements (D9); also the binary
  `verbatim-outpost.exe`, which runs as an app outpost or, with
  `--listener`, as the focus listener. Each process incarnation has an
  **outpost id**, which every node id it issues carries.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Presentable container** — an ancestor that qualifies for a
  focus-entry announcement under the NVDA-parity exclusion filter.
  [verbatim-core](crates/verbatim-core.md); NVDA's rule is in
  [Object model](nvda/object-model.md).
- **Recovery ladder** — the escalation for misbehaving outposts:
  rung 1, per-entry deadlines; rung 2, abandon the stuck worker and
  start a replacement; rung 3, end and respawn a wedged outpost; plus
  respawn after a crash. [Architecture](architecture.md) section 1.
- **Request table** — the app's record of every query sent to an
  outpost, the single owner of "exactly one outcome per query".
  [verbatim-app](crates/verbatim-app.md).
- **Reducer** — the pure functional core: `reduce(state, input)`
  returns the next state and effects, no I/O, no clocks (architecture
  section 2). [verbatim-core](crates/verbatim-core.md).
- **Scan mode** — Verbatim's planned document-reading mode family
  (the browse-mode analog), landing with M6.
  [Architecture](architecture.md) section 8.
- **Scenario** — one named, registered E2E test: setup, body,
  teardown, artifacts, and a `#[test]` wrapper sharing its name.
  [verbatim-e2e](crates/verbatim-e2e.md).
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
- **Window facts** — what an outpost attaches to each event about the
  window it concerns, read with local calls: its top-level window,
  root owner, whether it is topmost, and for `Windows.UI.Core` windows
  whether it is under the input thread's active window, and whether it is
  in the system's foreground window; the reducer
  classifies the event against attention with them.
  [verbatim-model](crates/verbatim-model.md).
- **Worker** — the one thread per outpost that takes entries from the
  intake queue in order and is the only thread that calls into the
  application; a **watchdog** thread replaces it when a call passes its
  deadline. [verbatim-outpost](crates/verbatim-outpost.md).
- **Utterance** — the structured unit of speech (a sequence of spans
  plus source metadata) emitted by the reducer (D12).
  [verbatim-speech](crates/verbatim-speech.md).
- **Wedge** — an outpost that is alive but no longer answering
  (missed pongs) or accumulating parked workers; the heartbeat's
  `wedge_decision` kills and respawns it (rung 3).
  [verbatim-outpost](crates/verbatim-outpost.md).
