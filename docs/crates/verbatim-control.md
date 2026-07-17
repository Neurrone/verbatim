# verbatim-control

The control plane (architecture section 10, decision D8): protocol v0 and
the named-pipe server. Pulled forward from M2 so every M1 change is
verifiable live.

Public API:

- `protocol` — `Request` (`Hello`, `Status`, `SubscribeEvents`,
  `SubscribeSpeech`, `SendGesture`, `SendKeys`, `Latency`, `DumpTree`,
  `DumpRecorder`, `Quit`) in a `RequestEnvelope` with a correlation id;
  `Frame` (`Reply`, `Error`, `Event`, `Speech`); `StatusInfo`,
  `OutpostStatus`, `LatencyRecord`; `PIPE_NAME`, `PROTOCOL_VERSION`; the
  same newline-JSON framing helpers. A speech frame carries the trace id,
  rendered text, the observation timestamp of the triggering event when
  there is one, the queue time, and the audio-start time once known.
  `ReplyPayload::DumpTree` answers `Request::DumpTree` with the walked
  tree (`verbatim_model::TreeNode`) and whether the outpost's depth or
  node-count cap cut it short; a walk that could not complete at all comes
  back as `Frame::Error`, the same convention `SendGesture` uses.
  `ReplyPayload::DumpRecorder` answers `Request::DumpRecorder` (milestone
  M2) with the path Core wrote its flight recorder's contents to.
- `client` — the control-plane client, promoted here from
  `verbatim-inspect` so any client, not just the CLI, can share it:
  `Client::connect_pipe()` on the well-known pipe, `connect_pipe_named(name)`
  for tests, and `connect_tcp(addr)` for a Verbatim reached over TCP (a
  remote session, or from inside a VM host); `request` completes the
  `Hello` handshake and matches replies by correlation id, discarding
  stream frames that arrive while a reply is pending; `next_frame` reads
  any frame, for subscription loops. Single-threaded by design, which is
  why its shared-handle `try_clone` is safe where the server needed
  overlapped I/O.
- `ServerHandlers` — the app-injected callbacks answering status, gesture
  routing, latency queries, tree dumps, flight-recorder dumps, and quit,
  keeping this crate ignorant of the application's internals.
- `ControlServer` — `start(handlers)` on the well-known pipe name,
  `start_on(name, handlers)` for tests; `broadcast_event(..)` and
  `broadcast_speech(..)` fan frames out to subscribed connections; drop
  stops accepting and disconnects every client.
- `send_keys` — `parse_combo` and `parse_all` (validating every entry
  against the shared key-name vocabulary before anything is injected) and
  `inject`, which synthesizes the modifier-down, key, modifier-up sequence
  via `SendInput` with correct extended-key flags.

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
connection's writer drains a bounded queue (256 frames) and drops with a
warning when a client stalls, so a slow inspector can never block Core.
The per-connection dispatch loop is generic over reader and writer, which
is what lets a loopback test exercise the identical code path with no pipe.
