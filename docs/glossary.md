# Glossary

Verbatim's invented vocabulary, alphabetical. Each entry gives the
short meaning and links the document that defines it properly. Terms
NVDA also uses (navigator object, review cursor, browse mode) keep
their NVDA meanings and are documented in [docs/nvda](nvda/readme.md).

- **Agent** — `verbatim-agent`, the in-guest test doorway: launches
  processes and tunnels the control plane over TCP for the E2E suite.
  [verbatim-agent](crates/verbatim-agent.md).
- **Abandoned scenario** — an E2E scenario whose agent connection ended
  before its cleanup proved it left no window open, because its test
  process was killed or the check failed; the next scenario's start
  closes, by handle, the windows it left.
  [verbatim-e2e](crates/verbatim-e2e.md).
- **Abandoned worker** — an outpost worker whose call passed its
  deadline; the outpost's reducer replaces it, since a thread stuck in a hung
  application's COM call cannot be safely killed. If the call ever
  returns, the thread publishes nothing and exits. Recovery ladder rung
  2's bounded garbage. [verbatim-outpost](crates/verbatim-outpost.md).
- **Adapter** — the engine-specific DLL a native synthesizer extension
  ships, implementing Verbatim's C ABI (the `SynthDriver` trait as C
  exports) and loading the engine; the synthesizer host loads only it.
  [extensions](design/extensions.md).
- **Application identity** — the name a module binds to an outpost by:
  the executable name, replaced by the console client's for a console
  window, the hosted application's for WWAHost, the parent's for
  WebView2. [extensions](design/extensions.md).
- **Arbitration** — the per-window choice of backend (UIA or
  MSAA/IA2), decided by class lists lifted from NVDA plus a live
  provider probe; the result is a **verdict**, kept for the window's
  lifetime.
  [verbatim-outpost](crates/verbatim-outpost.md); NVDA's referee is in
  [The UIA client](nvda/uia.md).
- **Backend** — one client stack over an accessibility API: UIA
  (`verbatim-uia`), MSAA/IA2 (`verbatim-ia2`), later JAB.
  [Architecture](architecture.md) section 4.
- **Built-in** — a module folder shipped with Verbatim under
  `extensions/builtin/` (planned), loaded by the same host as an installed module
  but never bundled, listed, installed or disabled.
  [extensions](design/extensions.md).
- **Call program** — a whole sequence of calls into an application with
  pure stopping rules, such as a focus's walk or a caret read, run by an
  outpost's worker on copies of what it reads; its commits and result
  are posted back to the outpost's reducer.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Capability** — a group of host functions a module imports, derived
  from the component's imports and never declared; only the sentences
  naming something of the user's are shown at install.
  [extensions](design/extensions.md).
- **Capture synth** — the test synthesizer that records the flattened
  speech it was asked to speak instead of producing audio; what speech
  pipeline unit tests assert on (E2E runs speak through eSpeak NG).
  [verbatim-synth-capture](crates/verbatim-synth-capture.md).
- **Attention** — the reducer's record of the application and
  top-level window of the most recent foreground change, standing in
  for the system's foreground window; events other than focus changes
  are accepted or dropped against it, while a focus is judged against
  the real foreground window as its outpost read it (D14).
  [verbatim-core](crates/verbatim-core.md).
- **Cold case** — a window arbitration has never seen: the first event
  for it resolves a real verdict with a provider probe on the worker.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Control plane** — the one authenticated protocol (JSON over the
  control pipe) serving dev tooling now and remote support later
  (D8). [verbatim-control](crates/verbatim-control.md).
- **Earcon** — an event Verbatim reports at once, outside the speech
  queue (start, a mode switch, an application not responding), named
  semantically; the active theme decides whether it plays a sound, is
  spoken, both, or neither. [verbatim-model](crates/verbatim-model.md).
- **Effect** — a reducer output: an instruction (Speak, Fetch,
  Activate…) the shell executes; the reducer itself does no I/O.
  [verbatim-core](crates/verbatim-core.md).
- **Extension** — the installable unit, a `.vext` zip holding an extension
  manifest, locale files and one or more module folders; NVDA's add-on.
  [extensions](design/extensions.md).
- **Fact** (focus fact) — one focus-relevant observation captured by
  the focus listener from an OS event's own payload and routed to the
  target's outpost for announcement (D13); a `ListenerFact` becomes a
  `DeliveredFact` once routed. [verbatim-listener](crates/verbatim-listener.md).
- **Flight recorder** — the bounded ring buffer of reducer inputs,
  dumpable to a versioned JSON-lines file and deterministically
  replayable. [verbatim-core](crates/verbatim-core.md).
- **Focus listener** — the permanent process, `verbatim-listener.exe`,
  holding the desktop-global focus registrations, whose callbacks never make a
  cross-process call (D13); since stage A step 6 of phase 6 it also judges
  and reads each UIA focus from the event's own element, on a reading
  thread per application. [Architecture](architecture.md) section 1.
- **Focus marker** — what the focus listener sends an application's
  outpost the moment a UIA focus event arrives, before it has read it:
  the outpost holds everything it takes in after the marker until the
  listener's read of the focus, or its drop, takes the marker's place.
  [verbatim-listener](crates/verbatim-listener.md).
- **Golden image** — the Packer-built Windows VM baseline the Hyper-V
  harness imports, deploys to, and checkpoints. [The VM harness](tooling/vm.md).
- **Held nodes** — the node references the reducer holds (the focus,
  its ancestors, the last selection, the navigator, and a pending
  navigation's start); Core sends each outpost its own, and the outpost
  keeps the live objects for exactly those and releases the rest.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Hook** — a function a module exports and the host calls, on the
  thread that holds the event, under an epoch deadline.
  [extensions](design/extensions.md).
- **Idle retirement** — ending an outpost whose application has not
  held attention for two minutes and in which Core holds no nodes
  (risk R2's memory mitigation).
  [verbatim-supervisor](crates/verbatim-supervisor.md).
- **Indication** — one entry of the catalogue of everything Verbatim
  can report (a role, a state, a description, a spelling error, an
  event), with a stable id and a category; a theme says how each is
  reported. [verbatim-model](crates/verbatim-model.md).
- **Message position** — the count of messages carrying node ids an
  outpost has sent, kept the same way by the outpost and Core; Core
  acknowledges a position when it reports its held nodes, so the
  outpost knows which reported nodes Core has seen.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Module** — one contribution inside an extension: an app module bound
  to applications by identity, or a synthesizer; a folder with
  `module.toml` and a component, one instance per outpost.
  [extensions](design/extensions.md).
- **Normalized model** — `verbatim-model`'s API-agnostic vocabulary
  of nodes, roles, states, and events that both backends map into.
  [verbatim-model](crates/verbatim-model.md).
- **Object store** — the outpost's monitor that owns node identity,
  the application objects it keeps and each node's mint tag, and
  releases a node only when no live request and no message can still
  name it. [verbatim-outpost](crates/verbatim-outpost.md).
- **Outpost** — the per-application process owning that app's event
  hooks, queries, and announcements (D9); also the binary
  `verbatim-outpost.exe` it runs. Each process generation has an
  **outpost id**, which every node id it issues carries.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Pending key** — a key Core has taken in for a terminal and not yet
  given to its reading rules, because no read or timer at or after the
  key's time has come yet. [verbatim-core](crates/verbatim-core.md),
  "Core's terminal model".
- **Presentable container** — an ancestor that qualifies for a
  focus-entry announcement under the NVDA-parity exclusion filter.
  [verbatim-core](crates/verbatim-core.md); NVDA's rule is in
  [Object model](nvda/object-model.md).
- **Recovery ladder** — the escalation for misbehaving outposts:
  rung 1, per-entry deadlines; rung 2, abandon the stuck worker and
  start a replacement; rung 3, end a failed outpost, crashed, stuck or
  piling up abandoned workers, and give its application the next one.
  [Architecture](architecture.md) section 1.
- **Request table** — the app's record of every query sent to an
  outpost, the single owner of "exactly one outcome per query".
  [verbatim-app](crates/verbatim-app.md).
- **Responder** — a value that answers an asker once; answering
  consumes it, and dropping it unanswered answers "gone". It lives in a
  thread's shell, never in a pure reducer's state.
  [the ownership model](design/ownership.md).
- **Reducer** — the pure functional core: `reduce(state, input)`
  returns the next state and effects, no I/O, no clocks (architecture
  section 2). [verbatim-core](crates/verbatim-core.md).
- **Rule** — a module's declaration of the form "match this, do that",
  with a subject (window, event or node), a required id and a backend,
  applied by the host as data without calling the module.
  [extensions](design/extensions.md).
- **Scan mode** — Verbatim's planned document-reading mode family
  (the browse-mode analog), landing with M7.
  [Architecture](architecture.md) section 8.
- **Scenario** — one named, registered E2E test: setup, body,
  teardown, artifacts, and a `#[test]` wrapper sharing its name.
  [verbatim-e2e](crates/verbatim-e2e.md).
- **Span** — one typed piece of an utterance (label, role, value,
  state, text run) per D12; turned into words, a sound, both, or
  nothing by the active **theme** at the last pipeline stage.
  [verbatim-speech](crates/verbatim-speech.md).
- **Speech key** — the key whose trace a terminal read's speech goes out
  under: the newest key given to the reading rules whose window holds
  the read, so a later key's cut treats that speech as the key's.
  [verbatim-core](crates/verbatim-core.md), "Core's terminal model".
- **Terminal snapshot** — one read of a terminal as its outpost sends it
  to Core (`TerminalSnapshot`): the text from where the read started to
  the document's end, without its lines' padding, the screen, the caret,
  and the read program's flags. A live read is one the outpost takes on
  its own, on a text change or caret event; a whole read is the whole
  document, which the reading rules ask for after a lost read; a
  history-block read is a terminal's first read taken again with a
  screen of rows above its screen. [verbatim-model](crates/verbatim-model.md);
  [How Verbatim reads terminals](design/terminal-model.md).
- **Theme** — a complete collection of indications and how each is
  reported (off, speech, sound, or speech and sound), with its sounds,
  replacement words, and voice styles; verbosity, speech, and sounds in
  one model. The built-in default theme is complete; every other theme
  holds only its differences. [verbatim-model](crates/verbatim-model.md)
  and [verbatim-config](crates/verbatim-config.md).
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
  classifies the event with them, against attention or, for a focus,
  against the foreground window.
  [verbatim-model](crates/verbatim-model.md).
- **Worker** — the one thread per outpost that takes entries from the
  intake queue in order and is the only thread that calls into the
  application; a **watchdog** thread replaces it when a call passes its
  deadline. [verbatim-outpost](crates/verbatim-outpost.md).
- **Utterance** — the structured unit of speech (a sequence of spans
  plus source metadata) emitted by the reducer (D12).
  [verbatim-speech](crates/verbatim-speech.md).
- **Wedge** — an outpost that is alive but no longer answering
  (missed pongs, called stuck) or accumulating parked workers; the
  heartbeat's `wedge_decision` ends it and its application is given the
  next outpost (rung 3).
  [verbatim-supervisor](crates/verbatim-supervisor.md).
