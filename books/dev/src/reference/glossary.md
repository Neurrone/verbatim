# Glossary

Verbatim's invented vocabulary, alphabetical. Each entry gives the
short meaning and links the document that defines it properly. Terms
NVDA also uses (navigator object, review cursor, browse mode) keep
their NVDA meanings and are documented in [docs/nvda](https://github.com/Neurrone/verbatim/blob/main/docs/nvda/readme.md).

- **Agent** — `verbatim-agent`, the end-to-end suite's test doorway: launches
  processes and tunnels the control plane over TCP for the E2E suite.
  [verbatim-agent](crates/verbatim-agent.md).
- **Abandoned scenario** — an E2E scenario whose agent connection ended
  before its cleanup proved it left no window open, because its test
  process was killed or the check failed; the next scenario's start
  closes, by handle, the windows it left.
  [verbatim-e2e](crates/verbatim-e2e.md).
- **Abandoned worker** — an outpost worker still inside a call when
  Core cancels its request because the foreground moved away from the
  request's window; the outpost's reducer starts a replacement, since a
  thread stuck in a hung application's COM call cannot be safely killed. If the call ever
  returns, the thread publishes nothing and exits. Recovery ladder rung
  2's bounded garbage. [verbatim-outpost](crates/verbatim-outpost.md).
- **Adapter** — the engine-specific DLL a native synthesizer extension
  ships, implementing Verbatim's C ABI (the `SynthDriver` trait as C
  exports) and loading the engine; the synthesizer host loads only it.
  [extensions](https://github.com/Neurrone/verbatim/blob/main/docs/design/extensions.md).
- **Application identity** — the name a module binds to an outpost by:
  the executable name, replaced by the console client's for a console
  window, the hosted application's for WWAHost, the parent's for
  WebView2. [extensions](https://github.com/Neurrone/verbatim/blob/main/docs/design/extensions.md).
- **Arbitration** — the per-window choice of backend (UIA or
  MSAA/IA2), decided by class lists lifted from NVDA plus a live
  provider probe; the result is a **verdict**, kept by Core for half a
  second from the probe's start, and by an outpost only within one
  request.
  [verbatim-outpost](crates/verbatim-outpost.md); NVDA's referee is in
  [The UIA client](https://github.com/Neurrone/verbatim/blob/main/docs/nvda/uia.md).
- **Ask** — a request from Core to a reader; every call into an application is made for one, and it gets exactly one answer.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Backend** — one client stack over an accessibility API: UIA
  (`verbatim-uia`), MSAA/IA2 (`verbatim-ia2`), later JAB.
  [Architecture](../explanation/architecture.md) section 4.
- **Batch** — a request of the request list: at most four steps, each
  an operation on a subject with an optional condition, answered with
  one outcome per step. Core builds one with `cx.batch`; each reader
  parses it once on arrival and refuses one that does not parse, whole.
  [verbatim-model](crates/verbatim-model.md), "Tells, requests and the tables".
- **Built-in** — a module folder shipped with Verbatim under
  `extensions/builtin/` (planned), loaded by the same host as an installed module
  but never bundled, listed, installed or disabled.
  [extensions](https://github.com/Neurrone/verbatim/blob/main/docs/design/extensions.md).
- **Call program** — a whole sequence of calls into an application with
  pure stopping rules, such as a focus's walk or a caret read, run by an
  outpost's worker on copies of what it reads; its commits and result
  are posted back to the outpost's reducer.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Capability** — a group of host functions a module imports, derived
  from the component's imports and never declared; only the sentences
  naming something of the user's are shown at install.
  [extensions](https://github.com/Neurrone/verbatim/blob/main/docs/design/extensions.md).
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
- **Check thread** — the listener's thread, one per application whose UI Automation events it hears, that makes at each event's delivery, before its tell is sent, the calls NVDA's handler for that event makes, so that Core judges their results.
- **Cold case** — a window arbitration has never seen: the first event
  for it resolves a real verdict with a provider probe on the worker.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Committed focus** — the focus Core has executed, NVDA's focus object; commands, the narrow rule and typed echo judge it.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Condition** — a step's test of one earlier step: matched (of a
  `Test`), found, or the negation of either. A condition on a skipped
  step skips its step too; on a refused, missing or failed one, the test
  is false and its negation true.
  [verbatim-model](crates/verbatim-model.md), "Tells, requests and the tables".
- **Control plane** — the one authenticated protocol (JSON over the
  control pipe) serving dev tooling now and remote support later
  (D8). [verbatim-control](crates/verbatim-control.md).
- **Core's inbox** — the one place on Core's thread where everything arrives, in arrival order; nothing waits there.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Due time** — when Core stops waiting for a request's answer: 11 seconds, 13 for a focus read, counted from when Core decides to ask.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Earcon** — an event Verbatim reports at once, outside the speech
  queue (start, a mode switch, an application not responding), named
  semantically; the active theme decides whether it plays a sound, is
  spoken, both, or neither. [verbatim-model](crates/verbatim-model.md).
- **Effect** — a reducer output: an instruction (Speak, Fetch,
  Activate…) the shell executes; the reducer itself does no I/O.
  [verbatim-core](crates/verbatim-core.md).
- **Entered ancestors** — the ancestors a new focus answer carries below
  the lowest common ancestor, which Core speaks as entered. [How focus works](../explanation/design/focus.md).
- **Event queue** — Core's ordered queue of application events, keys, commands and entries Core makes itself; only its front entry may have a request out.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Extension** — the installable unit, a `.vext` zip holding an extension
  manifest, locale files and one or more module folders; NVDA's add-on.
  [extensions](https://github.com/Neurrone/verbatim/blob/main/docs/design/extensions.md).
- **Fact** (focus fact) — one focus-relevant observation captured by
  the focus listener from an OS event's own payload and told to Core in
  a tell. [verbatim-listener](crates/verbatim-listener.md).
- **Flight recorder** — the bounded ring buffer of reducer inputs,
  dumpable to a versioned JSON-lines file and deterministically
  replayable. [verbatim-core](crates/verbatim-core.md).
- **Focus ancestry** — Core's ordered list of the focus's ancestors and
  the focus, from the desktop down, that it speaks from and joins each
  new report against. [How focus works](../explanation/design/focus.md).
- **Focus listener** — the permanent process, `verbatim-listener.exe`,
  holding the desktop-global focus registrations, whose callbacks never make a
  cross-process call (D13); since stage A step 6 of phase 6 it also judges
  and reads each UIA focus from the event's own element, on a reading
  thread per application. [Architecture](../explanation/architecture.md) section 1.
- **Stop waiting** — Core no longer waits for a request's answer: it records in its own table that it no longer waits for the request and takes the next entry; whether the reader is told to cancel, and what a late answer does, depends on the reason.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Held object** — something a reader keeps behind an id Core names, such as a node or a position in a text, released when Core releases it.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Held nodes** — the node references the reducer holds (the focus,
  its ancestors, the last selection, the navigator, and a pending
  navigation's start); Core sends each outpost its own, and the outpost
  keeps the live objects for exactly those and releases the rest.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Hook** — a function a module exports and the host calls, on the
  thread that holds the event, under an epoch deadline.
  [extensions](https://github.com/Neurrone/verbatim/blob/main/docs/design/extensions.md).
- **Indication** — one entry of the catalogue of everything Verbatim
  can report (a role, a state, a description, a spelling error, an
  event), with a stable id and a category; a theme says how each is
  reported. [verbatim-model](crates/verbatim-model.md).
- **Lowest common ancestor** — the innermost ancestor a new focus
  answer shares with Core's focus ancestry, where Core joins the two.
  [How focus works](../explanation/design/focus.md).
- **Matcher** — a test on one node's properties (a value, absent, one of
  a list, a state, all, any, not), evaluated in Rust on the values a
  reader read, lazily in its order; the node subset of the extension
  matcher grammar. `verbatim_model::matcher`.
  [verbatim-model](crates/verbatim-model.md), "Tells, requests and the tables".
- **Menu slot** — the one waiting entry of Core's event queue for MSAA
  menu start, menu end, popup start, popup end and switch end, which keeps
  the newest (`crates/verbatim-core/src/event_queue/limiter.rs`).
- **Message position** — the count of messages carrying node ids an
  outpost has sent, kept the same way by the outpost and Core; Core
  acknowledges a position when it reports its held nodes, so the
  outpost knows which reported nodes Core has seen.
  [verbatim-outpost](crates/verbatim-outpost.md).
- **Outcome** — one of the ways an answer to a request ends: a result;
  refused, with a reason; not there; not responding; the reader ended or
  stopped; and, inside Core alone, no longer awaited. Core's rules branch
  on the outcome, not its diagnostic detail. A batch's result holds a
  step outcome for each step: done with its result, refused, not there,
  failed, or skipped.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Module** — one contribution inside an extension: an app module bound
  to applications by identity, or a synthesizer; a folder with
  `module.toml` and a component, one instance per outpost.
  [extensions](https://github.com/Neurrone/verbatim/blob/main/docs/design/extensions.md).
- **Normalized model** — `verbatim-model`'s API-agnostic vocabulary
  of nodes, roles, states, and events that both backends map into.
  [verbatim-model](crates/verbatim-model.md).
- **Object store** — the outpost's monitor that owns node identity,
  the application objects it keeps, and releases a node only between
  programs, when no program and no message can still name it. [verbatim-outpost](crates/verbatim-outpost.md).
- **Operation** — one of the request list's seventeen kinds of step:
  `Read`, `Test`, `Related`, `Ancestors`, `Descendants`, `IsOneOf`,
  `Activate`, `Follow`, `FocusRead`, `Caret`, `ReadText`, `ReadRange`,
  `CompareSelection`, `Select`, `Locate`, `MoveBySentence` and
  `WaitForCaret`. A new behaviour writes a batch of them and adds no
  kind of request (ADR 0003).
  [verbatim-model](crates/verbatim-model.md), "Tells, requests and the tables".
- **Ordinary-calls executor** — `run_calls`: a batch's fusable
  operations run call by call through the backend's adapter, for the
  setting with remote operations off, a window marked for classic
  reading, a step no program can take, and a failed program's steps.
  [verbatim-uia-rops](crates/verbatim-uia-rops.md), "The program builder and the ordinary-calls executor".
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
  [Object model](https://github.com/Neurrone/verbatim/blob/main/docs/nvda/object-model.md).
- **Program builder** — `run_batch`: one builder of UI Automation remote
  programs, which puts a run of a batch's consecutive fusable steps into
  programs from each operation's emitter, each step in its own try block
  with its own status. Not a call program, which is a worker's sequence
  of calls.
  [verbatim-uia-rops](crates/verbatim-uia-rops.md), "The program builder and the ordinary-calls executor".
- **Reader** — a process that calls into applications for Core and hears their events: the focus listener, which holds every desktop-wide registration, and one outpost per application.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Readers' last focus** — the newest focus a reader read that Core executed, NVDA's last queued focus; empty at startup and after the foreground leaves for an unrelated top-level window.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Recovery ladder** — the escalation for misbehaving outposts:
  rung 1, Core stops waiting for a request; rung 2, abandon the stuck worker and
  start a replacement; rung 3, end a failed outpost, crashed, stuck or
  piling up abandoned workers, and give its application the next one.
  [Architecture](../explanation/architecture.md) section 1.
- **Reference** — a step's naming of an earlier step of its batch by its
  place, as its subject (that step's node or list, or one item of the
  list) or as a text point (that step's caret, a selection's end or a
  chunk's start); a step whose referenced step is not done is skipped.
  In Core a reference is a handle typed by its result's kind and branded
  by its batch (`batch::Ref`).
  [verbatim-model](crates/verbatim-model.md), "Tells, requests and the tables".
- **Request table** — Core's record of every request it makes of a
  reader, from its asking to its one ending.
  [verbatim-core](crates/verbatim-core.md), "Core's requests".
- **Responder** — a value that answers an asker once; answering
  consumes it, and dropping it unanswered answers "gone". It lives in a
  thread's shell, never in a pure reducer's state.
  [the ownership model](../explanation/design/ownership.md).
- **Reducer** — the pure functional core: `reduce(state, input)`
  returns the next state and effects, no I/O, no clocks (architecture
  section 2). [verbatim-core](crates/verbatim-core.md).
- **Run** — MSAA focus and foreground tells merged into one entry of
  Core's event queue and tried newest first.
  [verbatim-core](crates/verbatim-core.md), "One definition of a
  candidate's validity".
- **Rule** — a module's declaration of the form "match this, do that",
  with a subject (window, event or node), a required id and a backend,
  applied by the host as data without calling the module.
  [extensions](https://github.com/Neurrone/verbatim/blob/main/docs/design/extensions.md).
- **Scan mode** — Verbatim's planned document-reading mode family
  (the browse-mode analog), landing with M7.
  [Architecture](../explanation/architecture.md) section 8.
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
- **Starting point** — the text anchor a caret key's starting batch
  mints, held by the outpost, with the key's unit, word, line and
  selection read beside it, which the key's watch compares the caret with.
  [verbatim-core](crates/verbatim-core.md), "The caret key on the event
  queue".
- **Ancestors Core already knows** — the committed focus's ancestors, sent with a focus request. The reader's walk reads up the ancestor chain until it reaches one of them.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Subject** — the object a step's operation acts on: a node a reader
  holds, a listener's kept element, an MSAA object at its event's ids, a
  route to the outpost's own copy of an element the listener read, a
  window, the application's focus, or an earlier step's result. Each is
  resolved once per batch, at its first use.
  [verbatim-model](crates/verbatim-model.md), "Tells, requests and the tables".
- **Tell** — a message a reader sends Core when an event arrives, carrying what came with the event and reads of Windows' own state that cannot hang; a UI Automation event's tell also carries the results of its check thread's calls, and no other tell makes a call into the application.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
- **Terminal snapshot** — one read of a terminal as its outpost sends it
  to Core (`TerminalSnapshot`): the text from where the read started to
  the document's end, without its lines' padding, the screen, the caret,
  and the read program's flags. A live read is one the outpost takes on
  its own, on a text change or caret event; a whole read is the whole
  document, which the reading rules ask for after a lost read; a
  history-block read is a terminal's first read taken again with a
  screen of rows above its screen. [verbatim-model](crates/verbatim-model.md);
  [How Verbatim reads terminals](../explanation/design/terminal-model.md).
- **Told, not yet read** — Core's fact, per terminal, that a change was
  told and no full read has begun since; the outpost keeps no such flag.
  [ADR 0003](adr/0003-readers-tell-core-asks.md).
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
  application; the outpost abandons and replaces it when Core cancels
  its request for the foreground. [verbatim-outpost](crates/verbatim-outpost.md).
- **Utterance** — the structured unit of speech (a sequence of spans
  plus source metadata) emitted by the reducer (D12).
  [verbatim-speech](crates/verbatim-speech.md).
- **Watch** — the outpost's waiting request that looks for a caret key's
  effect by comparing the application's caret with the key's starting
  point until its bound: a batch whose first step is `WaitForCaret`.
  Each comparison is a **check**; the batch's later reads run in the
  poll that ends it.
  [verbatim-outpost-logic](crates/verbatim-outpost-logic.md).
- **Wedge** — an outpost that is alive but no longer answering
  (missed pongs, called stuck) or accumulating parked workers; the
  generation record in Core ends it and its application is given the
  next outpost (rung 3).
  [verbatim-supervisor](crates/verbatim-supervisor.md).
