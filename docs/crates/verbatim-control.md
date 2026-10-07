# verbatim-control

The control plane (architecture section 10, decision D8): protocol v1 and
the named-pipe server. Pulled forward from M2 so every M1 change is
verifiable live.

Public API:

- `protocol` — `Request` (`Hello`, `Status`, `SubscribeEvents`,
  `SubscribeSpeech`, `SendGesture`, `SendKeys`, `Latency`, `DumpTree`,
  `DumpRecorder`, `Quit`) in a `RequestEnvelope` with a correlation id;
  `Frame` (`Reply`, `Error`, `Event`, `Speech`, `SpeechStarted`,
  `SpeechEnded`, `Sound`); `ReplyPayload`, `StatusInfo` (whose `ready` says Verbatim can take
  input: GUI up, focus listener running, own-window outpost ready, focus
  known),
  `OutpostStatus`, `OutpostState`, `LatencyRecord`; `PIPE_NAME`, `PROTOCOL_VERSION`; the
  same newline-JSON framing helpers. Of the two readers, `read_message` is
  for connections whose reads never time out: a read that fails partway
  through a line loses the part already read. `MessageReader` is for
  connections with a read timeout, such as the end-to-end suite's tunnels:
  it keeps a partly received message across a timeout, so the next read
  continues it and each message is decoded exactly once. The speech
  subscription carries three frames per utterance (decision D17).
  `Speech` is sent when the utterance is queued and carries its
  `UtteranceId`, the trace id, the rendered text, the observation
  timestamp of the triggering event when there is one, and the queue
  time. `SpeechStarted` carries the utterance id and the time its first
  frame played, and is absent for an utterance that never played.
  `SpeechEnded` carries the utterance id and its `UtteranceEnding`
  (completed, cancelled, or failed with a reason); every utterance
  announced by a `Speech` frame is followed by exactly one. `SpeechEnded`
  carries no text, so it never competes with `Speech` as a matchable
  utterance. A fourth frame, `Sound`, goes to the same subscribers when a
  sound plays at once for an event, outside any utterance (the start and
  exit sounds, an application not responding): the id of the indication
  it reports, such as `exit`, and when it started. A sound in the speech
  stream is not sent this way; its utterance's text names it in its place
  (`sound: spelling-error`).
  `ReplyPayload::DumpTree` answers `Request::DumpTree` with the walked
  tree (`verbatim_model::TreeNode`) and whether the outpost's depth or
  node-count cap cut it short; a walk that could not complete at all comes
  back as `Frame::Error`, the same convention `SendGesture` uses.
  `ReplyPayload::DumpRecorder` answers `Request::DumpRecorder` (milestone
  M2) with the path Core wrote its flight recorder's contents to.
  A `LatencyRecord` carries a timeline's three times and, since protocol
  version 1, its `stages`: one `LatencyStage` per stage it has passed, in
  pipeline order, each with its `LatencyStageKind` (Windows, hook to Core,
  listener to outpost, Core to outpost, outpost queue, caret wait, outpost
  read, to Core, reducer, to speech, synthesis, leading silence, mixer and
  device; `label` names each as the latency log does), its time in
  microseconds, and, for the caret wait and the outpost read, the
  cross-process calls made in it. Version 1 only adds fields, which a
  version 0 peer ignores and reads as empty, so the server answers every
  client in the same vocabulary.
- `client` — the control-plane client, promoted here from
  `verbatim-inspect` so any client, not just the CLI, can share it:
  `Client::connect_pipe()` on the well-known pipe, `connect_pipe_named(name)`
  for tests, `connect_tcp(addr)`, which dials `addr` and sends the
  control `Hello` at once, so it reaches only an endpoint that speaks the
  control protocol from the first byte (Verbatim itself never listens on
  TCP), and `from_tcp_stream(stream)`, which completes the handshake on a
  socket that is already connected: the end-to-end suite reaches a
  Verbatim through the in-guest agent by sending the agent's `Hello` and
  `OpenControlTunnel` on one socket, then handing that socket to
  `from_tcp_stream`. Every constructor completes the `Hello` handshake;
  `request` matches replies by correlation id, discarding every other
  frame that arrives while a reply is pending, stream frames on a
  subscribed connection included; `next_frame` reads
  any frame, for subscription loops. Both read through a `MessageReader`,
  so a read timeout set on a TCP transport never splits a frame.
  Single-threaded by design, which is
  why its shared-handle `try_clone` is safe where the server needed
  overlapped I/O.
- `ServerHandlers` — the app-injected callbacks answering status, gesture
  routing, latency queries, tree dumps, flight-recorder dumps, and quit,
  keeping this crate ignorant of the application's internals.
- `ControlServer` — `start(handlers)` on the well-known pipe name,
  `start_on(name, handlers)` for tests; `broadcast_event(..)`,
  `broadcast_speech(..)`, `broadcast_speech_started(..)`, and
  `broadcast_speech_ended(..)` fan frames out to subscribed connections;
  `has_event_subscribers()` and `has_speech_subscribers()` let a caller
  skip copying an event or an utterance's text when nobody is subscribed;
  drop stops accepting and disconnects every client.
- `send_keys` (public, used by the server's `SendKeys` handler and by the
  agent's request of the same name) — `parse_combo` and `parse_all`
  (validating every entry against the shared key-name vocabulary before
  anything is injected) and `inject`, which synthesizes the modifier-down,
  key, modifier-up sequence via `SendInput` with correct extended-key
  flags. The modifier position accepts Control, Shift, Alt, either Windows
  key, and the screen-reader modifiers Insert, numpad Insert, and Caps
  Lock, so an NVDA command such as `insert+t` can be pressed.

Implementation notes, the server: the pipe is created with a security
descriptor restricting access to the owning user and with remote clients
rejected — a Verbatim inside a VM is driven by a client inside that VM,
and the future remote feature is a separate authenticated transport. Every
pipe instance uses overlapped I/O with the calls wrapped to look
synchronous. That is a correctness requirement, not a style choice: a
synchronous pipe handle serializes its I/O directions at the driver level,
so a pending blocking read on one thread blocks a concurrent write from
another thread — even across duplicated handles — which deadlocked the
original implementation. With overlapped I/O one handle serves a dedicated
reader thread and a dedicated writer thread per connection. Each
connection's writer drains a bounded queue (256 frames), so a slow
inspector can never block Core. A subscribed client whose queue is full
is disconnected with a warning rather than having frames dropped: a
subscriber relies on seeing every frame, an utterance's ending above all,
and a disconnect ends its stream visibly where a dropped frame would
not.
The per-connection dispatch loop is generic over reader and writer, which
is what lets a loopback test exercise the identical code path with no pipe.
