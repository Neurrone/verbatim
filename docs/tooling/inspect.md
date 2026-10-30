# Driving a running Verbatim with verbatim-inspect

How to drive and watch a running Verbatim through the control plane with `verbatim-inspect`, and how to read the latency line it and Verbatim's log report.

`verbatim-inspect` is a developer CLI over the control plane
(`docs/architecture.md` section 10). It is not a child of Core and is not in
any job object; it attaches through the named pipe (or, for a VM or CI
agent, a TCP tunnel) like any other control-plane client. Every subcommand
prints plain text, one fact per line, with no tables or spinners, so it
reads well piped, redirected, or through a screen reader. Timestamps
render as local wall-clock time with no zone suffix, such as
`2026-07-14T10:42:32.158`, through the Win32 conversion that is correct
across daylight-saving transitions. The crate itself is
`crates/verbatim-inspect`; its connection goes through
`verbatim-control`'s `Client` ([its guide](../crates/verbatim-control.md)).

Build and start a Verbatim to inspect first:

```
cargo verbatim-build
target\debug\verbatim.exe
```

`cargo verbatim-build` is an alias, defined in `.cargo/config.toml`, for
`cargo build -p verbatim-app -p verbatim-outpost -p verbatim-synth-host`.

Verbatim finds `verbatim-outpost.exe` and `verbatim-synth-host.exe` next
to its own executable; without the synthesizer host, no synthesizer can
start. Building `verbatim-synth-host` also puts eSpeak NG's compiled
data, the `espeak-ng-data` directory, in `target\debug`, where the host
looks for it; a copy of Verbatim run from another folder needs that
directory beside the host. Building `verbatim-app` likewise copies the
top-level `sounds` folder, the default theme's sounds, into
`target\debug`; a copy run without it speaks each sound's indication
instead.

Then, in a second terminal, run `verbatim-inspect` subcommands against it.
Every subcommand accepts a global `--connect <ADDRESS>` option before the
subcommand name:

- Omit `--connect` entirely to reach the well-known local named pipe — the
  ordinary case when both `verbatim-inspect` and the Verbatim you are
  inspecting are on the same machine.
- `--connect tcp:HOST:PORT` connects over TCP instead and sends the
  control-plane `Hello` at once, so it works only against an endpoint that
  speaks the control protocol from the first byte. Verbatim itself never
  listens on TCP, and the in-guest agent speaks its own protocol first, so
  this does not reach a Verbatim through the agent: the E2E suite and
  `cargo xtask vm test` do that in code, sending the agent's `Hello` and
  `OpenControlTunnel` and then handing the socket to
  `Client::from_tcp_stream`.
- Anything else passed to `--connect` is treated as a literal named-pipe
  path, for tests that start a `ControlServer` on a non-default pipe name.

The subcommands:

- `status` prints the process id, version, active synthesizer, and one line
  per known outpost (its target pid, its own outpost pid once spawned, and
  its lifecycle state).
- `watch-events` subscribes to normalized accessibility events and prints
  one line per event: the trace id, the source, the backend (UIA or MSAA),
  and a summary (a focus change's role and name, a property change's new
  value, and so on).
- `watch-speech` subscribes to speech and prints up to three lines per
  utterance, each labelled with its utterance id: one when it is queued
  (with its text, its trace id, and the delta since the triggering
  event), one when its first frame plays, and one when it ends, saying
  whether it completed, was cancelled, or failed. An utterance cancelled
  before it played gets no audio-started line, only its ending.
- `watch` subscribes to both on one connection and interleaves them in
  arrival order, each line prefixed `event` or `speech`. Because the lines
  interleave in the order Verbatim actually produced them, an event line
  reads directly above the speech it caused — the fastest way to eyeball
  whether a gesture produced the announcement you expected.
- `send-gesture <identifier>` injects a gesture identifier through the
  keyboard hook's thread, which numbers it in line with real key presses,
  for example `verbatim-inspect send-gesture kb:verbatim+v` to open the
  Verbatim menu without touching a keyboard. Like a key press, it cancels
  current speech first.
- `send-keys <combo...>` synthesizes real OS keyboard input via `SendInput`,
  reaching whatever currently has focus — for example `verbatim-inspect
  send-keys downarrow enter "shift+tab"`. Because this is real synthetic
  input, not an injected gesture, it needs an unlocked, focused
  interactive desktop to land anywhere (see [Troubleshooting](troubleshooting.md)). It
  passes through Verbatim's keyboard hook like typed keys, so each key-down
  cancels speech, and a lone `shift` pauses or resumes it.
- `latency --last N` prints the N most recent end-to-end latency timelines
  (default 10), newest first: event-observed time, and the millisecond
  deltas to speech-queued and audio-started when each is known, then one
  indented line per stage the timeline passed, the stages the log line
  below names. The outpost read's line also gives its count of
  cross-process calls, by kind, and says the ratio to the floor needs
  calibration: the floor, an operation's minimum call count times the
  cost of one call, is defined in `docs/performance.md`, and only the
  end-to-end suite measures that cost. To compare a UIA application's
  focus reads with and without remote operations, set
  `remote_operations = false` under `[uia]` in `settings.toml` next to
  `verbatim.exe` and restart Verbatim: its outposts then read every UIA
  focus's ancestors with the classic walk, one call per ancestor. The
  default is on.

Verbatim's own log also carries one line per announcement, always, at
`info` on the `verbatim::latency` target, written once the audio device's
clock has passed its first frame, or, with no such clock (the silent test
device) or when playback stopped first, when the utterance ends.
For example:

`12.3 ms for "Voice combo box English (Great Britain) collapsed Alt+v": 9.5 ms to speech and 2.8 ms to sound, not counting 0.0 ms waiting behind earlier speech. Event: Windows 0.0, listener queue 0.0, listener's pipe to verbatim.exe 0.1, verbatim.exe 0.0, verbatim.exe to outpost 0.1, outpost queue 3.5, outpost read 5.5 (4 calls), outpost outbound 0.0, outpost's pipe to verbatim.exe 0.1, waiting for the reducer 0.1, reducer 0.0, to speech 0.1. Speech: synthesis 0.6, leading silence 0.1, mixer and device 2.0.`

The total is everything from Windows raising the event to the audio engine
taking the first sample, except time spent waiting behind earlier speech,
which is reported separately. The stages, in milliseconds:

- Windows: how long before the hook saw it Windows raised the event
  (whole milliseconds, and only for WinEvents).
- Hook to Core: for a caret key, from the keyboard hook seeing the key to
  the reducer having handled it and asked the outpost for the caret.
- Listener handling: from the listener observing the event to its having
  handled the fact. For a UIA focus, menu opening, selection or
  notification, that is its reading thread's queue and its read or check
  across processes; for a WinEvent's fact (the text box's, for instance)
  it is the callback's own local work until the fact is queued.
- Listener queue: from the listener's handling ending (or, for a
  timeline without that stamp, its observing the event) to the write of
  the fact to verbatim.exe's pipe beginning: the thread
  that produced the fact writes it at once unless a write is in flight,
  so this is the wait behind a write in flight, if any, and the I/O
  thread's wake to finish it.
- Listener's pipe to verbatim.exe: from the write beginning to verbatim.exe's relay thread taking the fact off the pipe:
  the encoding, the pipe, and the relay thread's wake.
- Listener to verbatim.exe: the three above taken whole, for a timeline
  that lacks the listener's write stamp.
- Reducer to verbatim.exe's relay: for a request Core makes (a caret key's
  watch, a command's query), from the reducer having handled the input to
  the request being queued for the outpost.
- Waiting for the outpost to start: a listener's fact that arrived while
  its outpost was starting, held until the outpost said it was ready.
  Reported apart from verbatim.exe's own time.
- verbatim.exe: verbatim.exe's own time relaying the fact or request, from
  taking it in to its relay thread beginning the write to the outpost,
  measured on one clock inside verbatim.exe, the outpost's start aside.
- verbatim.exe to outpost: from the relay thread beginning the write to
  the outpost receiving it: the encoding, the pipe, and the outpost
  reader's wake.
- Listener to outpost, Core to outpost: the two spans above taken whole,
  for a timeline that lacks verbatim.exe's relay stamps.
- Outpost queue: waiting for the outpost's worker, behind the
  application's other events.
- Caret wait: for a caret key whose first check found no evidence that
  the key did something, the time its watch stayed open, from the worker
  taking the request until it took the event that brought the evidence (a
  caret, text, or selection change), with the calls the checks before it
  made. The worker handles everything else meanwhile. A key answered by
  its first check has no caret wait, and a key that moves nothing is
  silent, so it has no line.
- Outpost read: the worker's cross-process UIA and MSAA calls and window
  messages, after the caret wait for a caret key, with how many it made
  when it made any (`docs/performance.md` says what counts).
- Outpost outbound: from the outpost's worker publishing the event or
  reply to its write beginning: the worker writes it at
  once unless a write is in flight, so this is the wait behind a write in
  flight, if any, and the I/O thread's wake to finish it.
- Outpost's pipe to verbatim.exe: from the write beginning to verbatim.exe's reader having read and parsed the message: the
  encoding, the pipe, and the reader.
- Outpost to verbatim.exe: the two above taken whole, for a timeline that
  lacks the outpost's write stamp.
- Waiting for the reducer: from verbatim.exe's reader having the message
  to the reducer taking it. This is the reducer busy with what came
  before, not the relay. A timeline without the reader's stamp has one
  stage, to Core, in place of these two.
- Reducer, to speech: the reducer, and the hand-off to the speech
  manager.
- Synthesis: from synthesis starting to the synthesizer's first audio,
  for a synthesizer in Verbatim's own process. A synthesizer host's is
  split in four: synthesizer request (the driver asked to speak until its
  request's write begins), to the synthesizer host (from that write until
  the host has read the request), the host's first chunk (the host's
  synthesizer to its first audio), and back from the synthesizer host
  (from the host beginning to write that audio until the synth thread,
  which reads the host's pipe itself, handles it).
- Leading silence: audio the trimmer held back as leading silence.
- Mixer and device: until the audio engine took the first sample.
- Device clock: from the engine taking the first sample until the
  device's own clock passed it, read from the audio device's clock; only
  with a real device, so not in the suite's silent audio mode.

Once a key has been pressed, the line ends with one more sentence: "From
the last key to speech 3.4, a key that typed text." (or "typed nothing"),
the time from the hook seeing the last key it saw before the speech was
queued to the speech queued. It is measured whatever trace the speech came
on, so it compares two builds that start an announcement's line at
different points, such as one that speaks a key's answer from the read
the program's own event caused and one that speaks it for the key. It
times the key that came last, which in a burst of keys may be a later key
than the one the speech answers.

A stage is left out when the announcement did not pass it; speech from a
read Core asked for (the focus after a menu closes, at startup) has no
outpost stages. A caret key's line starts when the keyboard hook saw the
key, so its total covers everything from the key to the first sample; the
caret's report in a standard edit control, read through window messages,
has the same stages as one read through UIA. A caret event with no key
behind it (typing, a paste, a mouse click) is not spoken, as in NVDA,
so it has no line; the report of the caret when a text field gains the
focus is part of the focus's own trace. For the synthesizer host's own split between the pipe and
synthesis, set `RUST_LOG=info,verbatim::stage=trace`: the host then logs
`host request` and `host audio` with each trace id. The same setting logs
the hops outside the ledger: "verbatim.exe to the listener" for each
ping and hand-over command the listener reads, "listener's pipe to
verbatim.exe" (or "outpost's pipe to verbatim.exe") for each pong, and a
menu's or switcher's end with its listener handling, listener queue and
pipe, each in microseconds.
- `dump-tree` asks the outpost for the foreground application's
  accessibility tree, walked from its top-level window, and prints one node
  per line indented two spaces per depth level, reusing the same
  role-and-name summary the event stream uses. Each element's children
  are read in one call. Where each node is and whether it is off the
  screen are not read unless asked for with `--locations`, since for some
  elements, a top-level window's title bar and its buttons among them, UIA
  asks the application's UI thread for them (`WM_GETTITLEBARINFOEX`),
  which Windows answers for one window at a time across the desktop,
  about 6 ms alone and seconds under load, while the application waits.
  Without the option a first line says they were not read, and no line
  shows a location; with it each line ends with `, at LEFT,TOP
  WIDTHxHEIGHT`, or `, no location` for a node that reports none, and
  `, off screen` for one that is. A trailing line notes when
  the tree is incomplete: the outpost's depth or node-count cap cut the
  walk short, or an element went away during it, leaving the part of the
  tree under it unread.
- `dump-recorder` asks Core to write its flight recorder's current contents
  to disk (next to `verbatim.exe`, under a `dumps` folder, named
  `flight-<UTC timestamp>.jsonl`) and prints the path it wrote to. Useful
  for capturing a live session as a `verbatim-core` replay fixture without
  waiting for a crash.
- `quit` asks Verbatim to exit cleanly through the control plane.

Reading `watch`'s output: each `event` line names the trace id that a
following `speech` line will usually share, so scanning down the interleaved
stream shows cause and effect directly — an `event` line for a focus change
immediately followed by a `speech` line announcing that same control's name,
role, and state is what a working focus announcement looks like. A `speech`
line with no matching `event` line above it is Core-originated speech (for
instance the startup announcement), which has no triggering accessibility
event.
