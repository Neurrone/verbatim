# Communication and threading: one design from an event or a key to sound

Proposed 2026-10-11, awaiting Dickson. It replaces piecemeal fixes with one model for every pipe between Verbatim's processes and for every thread an event or a key press passes through on its way to the first audio sample. It subsumes `docs/design/relay.md` (approved 2026-10-11, paused for this design) and findings 3, 4, 5, 7, 8, 9, 10, 11 and 12 of `docs/design/latency-audit.md`; findings 1, 2 and 6 are audio work, placed in the thread model here but kept as a step of their own.

Researched on main at 4960991 and on the paused relay branch `worktree-agent-a867236cb53da6eae` (seven commits, f94e761 to cfbd323, not yet compiled). Nothing was built or run. Verbatim code is cited as `path:line` from the repository root; NVDA as `nvda/source/...:line`. A statement marked "established" comes from the code, from a recorded measurement, or from the relay branch's findings; one marked "estimated" is reasoning, with the reasoning given, and is a thing the measurement steps in section 11 must confirm.

## 1. Summary

- Today a UIA focus change wakes 17 threads in four processes before its first sample, an MSAA focus 12, a typed character's echo 9 (11 when it interrupts speech), a caret key 17, and a review key that reads the application 16 (established, section 9, after `latency-audit.md` section 2). Every hand-off has a measured tail: 0.24 to 0.99 percent of hand-offs over 2 ms on the relay (`relay.md` section 3), 5.3 percent over 2 ms on the way from an outpost to the reducer (`latency-audit.md` section 1).
- The design has four parts.
  - One transport module, `verbatim_process::transport`, for every pipe: overlapped named pipes made by verbatim.exe, one instance each, connected before the child starts; length-prefixed frames with a small binary header and a JSON body (raw samples for PCM); one `WriteFile` per frame; at most one write in flight per pipe; and a rule that the thread producing a message writes it itself when nothing is ahead of it, so no writer thread is woken in the common case. The same module serves verbatim.exe and its children.
  - One thread model: in each process, one I/O thread per role that waits only on a completion port or on events, a thread for each kind of work that may block (an outpost's worker, a synthesizer), and nothing else on the path. verbatim.exe keeps relaying every event (no direct route between children). The reducer reads the outposts' pipes on its own port, so an outpost's message wakes one thread in verbatim.exe, not two. The speech queue thread goes; the speech queue's logic runs under a lock on the thread that speaks.
  - A synthesizer host protocol whose frames name their utterance, so the next `Speak` follows a `Cancel` at once instead of waiting for the cancelled utterance to end, and whose cancellation is signalled by the arrival of the command itself, never by a timer.
  - A latency ledger with a stage for every hop, stamps carried in each frame's header (patched in place just before the write) and in each speech job, assembled off the hot path by one reporting thread, and a measurement scenario through the real eSpeak NG host and a real WASAPI device, read from `IAudioClock`.
- After the design (estimated, by counting the threads in section 9): a UIA focus wakes 9 threads (10 under stage A step 6's listener reading threads), an MSAA focus 7, a typed echo 6 with no wait for a cancelled utterance, a caret key 11, a review key that reads the application 9. NVDA, one process, wakes about three.
- One scheduling policy for every hot-path thread: every process opts out of power throttling; threads that only move messages run at `THREAD_PRIORITY_HIGHEST`; threads that compute on the path run `ABOVE_NORMAL`; the audio thread keeps MMCSS "Pro Audio". The power plan stays Balanced and timer resolution is not raised.
- Migration is twelve small steps, measurements first. The relay branch's commits are mostly kept: section 12 says which are kept as they are, which change, and which are replaced.
- Eleven decisions for Dickson, in section 13.

## 2. Dickson's constraints, and how each is met

- verbatim.exe adds at most 1 to 2 ms, read as approved for `relay.md`: verbatim.exe's own stage within 1 to 2 ms, its hop no worse than the platform floor, every exception attributed by trace, measured once on bare metal too. Met by giving every verbatim.exe hop its own ledger stage (section 10) with the pass criteria of section 11, carried from `relay.md` section 7 and extended to the return path and the speech path.
- Events keep passing through verbatim.exe. Every listener fact is read by verbatim.exe's relay thread and written by it to the outpost; every outpost message is read by verbatim.exe's reducer. No pipe joins two children.
- The power plan stays Balanced. Only per-process and per-thread quality-of-service requests and thread priorities are used (section 6.1).
- Process isolation of D9, D13 and D18 stays. No process is merged or removed; the listener, one outpost per application and one host per synthesizer remain.
- No fixed waits, retries or polling in place of evidence. The design removes the two 10 ms cancel polls (`crates/verbatim-synth-hosted/src/lib.rs:49`, `crates/verbatim-synth-onecore/src/lib.rs:52`). Every wait it adds is on an I/O completion, an event or a channel. The deliberate waits listed in `latency-audit.md` section 4 (the listener's 50 ms menu-end grace, the 30 ms lock-key delay, the mixer's 10 ms start threshold) stay, as do deadline timers that detect a hang (the outposts' watchdog, the host's 10 s `HANG_TIMEOUT`), which are not polls.
- Platform-neutral crates stay free of Windows code. The transport, the scheduling calls and the I/O threads live in Windows crates. `verbatim-speech` gains only plain types: a cancellation token that can call a waker, and the `thread_started` callback the relay branch already added (cfbd323).
- Screen-reader-friendly documentation: this document uses prose and lists only.

## 3. Today, in one place

Established from the code unless marked.

### 3.1 Process boundaries

- Listener to verbatim.exe: an anonymous pipe (`crates/verbatim-process/src/lib.rs:214-252`, `CreatePipe`) with the default buffer (`crates/verbatim-outpost/src/supervisor/process.rs:99`). JSON lines written by the listener's outbound thread (`crates/verbatim-outpost/src/listener.rs:313-322`) with two `WriteFile` calls per message (`crates/verbatim-outpost/src/protocol.rs:864-879`); read in verbatim.exe by a `verbatim-listener-reader` thread (`crates/verbatim-outpost/src/supervisor/owner.rs:1126-1171`) that hands each parsed message to the owner thread over a crossbeam channel.
- verbatim.exe to each outpost and to the listener: commands, relayed facts and Core's requests (`SupervisorToOutpost`, `protocol.rs:414-532`), queued by the owner or by the reducer (`crates/verbatim-outpost/src/supervisor/mod.rs:305-347`) into a per-child queue of at most 64 entries with a replace-and-drop policy (`crates/verbatim-outpost/src/supervisor/writer.rs:99-140`), written by a per-child writer thread (`writer.rs:179-212`). The outpost's main thread reads them (`crates/verbatim-outpost/src/outpost/mod.rs:1063-1087`).
- Each outpost to verbatim.exe: events, replies, pongs and faults (`OutpostToSupervisor`, `protocol.rs:655` onwards), queued by the worker into the outbound queue with its merge rules (`crates/verbatim-outpost/src/outpost/outbound.rs:1-27`, `318-326`), written by the `verbatim-outbound` thread (`outbound.rs:369-415`), read in verbatim.exe by a reader thread per outpost (`owner.rs:1069-1122`) that sends to the reducer's channel.
- verbatim.exe to each synthesizer host: two anonymous pipes, the host's with a 4 KiB buffer for backpressure (`crates/verbatim-synth-hosted/src/lib.rs:51-53`). Already framed by a kind byte and a length, JSON for control and raw samples for PCM (`crates/verbatim-speech/src/hosting.rs:1-12`, `200-251`). `Cancel` names its utterance; `Pcm`, `Mark` and `Done` do not (`hosting.rs:26-90`).
- The control plane: a named pipe with its own overlapped I/O (`crates/verbatim-control/src/server.rs:1-34`), off the path except where it shares locks (section 6.6).

### 3.2 Threads on the path

- verbatim.exe: the keyboard hook thread (`crates/verbatim-input-windows/src/lib.rs:241-242`); the router (`crates/verbatim-app/src/main.rs:315-316`); the reducer (`main.rs:302-303`, loop at `1632-1655`); the supervisor's owner (`owner.rs:206-225`); the listener reader; a reader and a writer per outpost; the speech queue and synth threads (`crates/verbatim-speech/src/manager.rs:305-306`, `349-350`); a reader per synthesizer host (`synth-hosted/src/lib.rs:167-181`); the audio thread (`crates/verbatim-audio/src/mixer.rs:308-309`).
- The listener: UIA's callback threads, the WinEvent thread, the outbound writer.
- Each outpost: the main thread reading Core's pipe, the event thread (`crates/verbatim-outpost/src/event_thread.rs:37-38`), the worker and the watchdog (`crates/verbatim-outpost/src/outpost/worker.rs:404-418`), the outbound writer.
- Each synthesizer host: a reader thread that takes cancels and passes everything else to the main thread (`crates/verbatim-synth-host/src/main.rs:89-117`), and the main thread that synthesizes.
- No thread sets a priority or opts out of power throttling except the audio thread's MMCSS "Pro Audio" (`crates/verbatim-audio-wasapi/src/lib.rs:261`). The 2026-10-08 decision (`docs/roadmap.md:60-62`) is built only on the relay branch (7e6e562, cfbd323).

### 3.3 What the paused relay branch found (established by building it, as reported)

- A named pipe with a zero buffer makes every write wait for the reader, so 4 KiB is asked for.
- Rust's `std::fs::File` aborts on an overlapped handle, so overlapped ends are touched only through the `PipeIo` slot type, never through `File`.
- Shutdown must stay event-driven: joining retirement threads on the relay thread deadlocks. `relay.md` was wrong that the owner never blocked: `finish_shutdown` joined threads, and the heartbeat ran `application_is_hung` synchronously. Both were made event-driven.
- Writing from the pushing thread (finding 9) put encoding and `WriteFile` under the child's lock, a brief contention point.
- Stamping "just before `WriteFile`" was not possible as `relay.md` 4.5 described, because the stamp is inside the encoded message. The requests' verbatim.exe stage became "push to write".
- Platform-neutral crates can take a thread priority only through a callback.
- The cross-process benchmark clock is `GetSystemTimePreciseAsFileTime`, because enabling `Win32_System_Performance` would change `workspace-hack`.
- Putting the stamps inside messages changed every outpost message's wire format; serde defaults keep older peers readable.
- A command too large to send is skipped with a warning; a failed write closes the queue.

## 4. The transport: one module for every pipe

`verbatim_process::transport` grows out of the branch's `verbatim_process::overlapped` (db16548), which already has `pipe()`, `Port` and `PipeIo`. It is the only code in Verbatim that touches an `OVERLAPPED`, a pipe handle or a frame's bytes.

### 4.1 Pipes

- Kind: every pipe between Verbatim's processes is a named pipe created by verbatim.exe with `CreateNamedPipeW`, byte mode, `FILE_FLAG_OVERLAPPED`, `FILE_FLAG_FIRST_PIPE_INSTANCE`, one instance, `PIPE_REJECT_REMOTE_CLIENTS`, one direction per pipe, two pipes per child as today.
- Naming: `\\.\pipe\verbatim-<core pid>-<counter>-<random>`, as `overlapped.rs` already makes it.
- Security: a DACL that admits only the current user's SID, with a mandatory label at verbatim.exe's own integrity level, so a lower-integrity process cannot open it (the branch's `pipe_security`). The child's end is opened by verbatim.exe with `CreateFileW` at once, before the child is created, so the single instance is connected and no other process can ever take it; the child inherits the opened handle as today. The name exists only between `CreateNamedPipeW` and that `CreateFileW`, microseconds, and even then the DACL and the first-instance flag guard it.
- Both ends are overlapped. The child's end is opened with `FILE_FLAG_OVERLAPPED` too, so a child can write without ever blocking (section 4.3). This changes the relay branch's choice to keep children's ends synchronous. That choice was forced by `std::fs::File` aborting on overlapped handles; in this design no end, in any process, is wrapped in a `File`: both go only through the transport's types.
- Buffer: 64 KiB for the event pipes in both directions, so that a focus fact with its ancestors, or a reply with a line of text, completes its write at once (estimated: today's largest common messages are under 8 KiB; terminal output can be megabytes). The buffer is not a message size: a reader that has the frame's length asks for the whole rest of the frame in one `ReadFile` into one buffer of that size, never in chunks of its own choosing; whether that read completes once (data copied straight into a read already waiting, believed but not verified) or returns several times (data that arrived first and sat in the 64 KiB buffer, since a byte-mode pipe read returns as soon as some data is there) is the pipe driver's, and the reader simply asks again for the remainder. 4 KiB stays for the host's PCM pipe, whose small buffer is the backpressure that paces synthesis (`synth-hosted/src/lib.rs:51-53`). The floor benchmark (step 2) checks that a write that fits the buffer completes without pending, by counting pended writes, and counts how many completions a 1 MB and a 2 MB frame take with the read waiting first and with the data arriving first; the event pipes' buffer is raised only if those counts show it matters, since every pipe's buffer costs kernel memory.
- Completion: on every end served by a completion port, `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` and `FILE_SKIP_SET_EVENT_ON_HANDLE` are set, so an operation that completes at once is handled where it was issued, except on the reducer's inbound pipes, where every read's completion goes through the port so that the port's first-in, first-out order is the reducer's one order of arrival (section 4.5).
- `architecture.md` section 1 says internal channels have "no named endpoint", and section 10 repeats it. That changes: they are named, but single-instance and already connected when anything else could look. This needs Dickson's agreement (decision 11).

### 4.2 Framing

Every message on every pipe is one frame:

- a little-endian `u32`: the length of everything after it;
- a `u8` kind and a `u8` of flags;
- a `u16` header length, so the header can grow without breaking a reader;
- the routing fields, used only on the listener's pipe and Core's pipes to outposts: the target process id (`u32`, 0 for none), whether the fact may start an outpost (one bit of the flags), and the fact key as a length and its bytes (the bytes `FactKey` is compared by today, `protocol.rs:383`);
- the timing block, at fixed offsets: the stamps of section 10, each a `u64` of microseconds on the shared clock or a `u32` duration, zero for a point not passed;
- the body: JSON of the message type that lives where it lives today (`verbatim-outpost`'s protocol, `verbatim-speech`'s hosting types), or raw little-endian samples for PCM.

The maximum stays 32 MB (`protocol.rs:847-854`). A reader that sees a larger length treats the other end as failed, as now, before allocating. The host protocol's own frame functions (`hosting.rs:200-251`) are replaced by the transport's; `hosting.rs` keeps its message types, which are plain data in a platform-neutral crate.

What this removes: line splitting and the `read_until` scan, the second `WriteFile` for the line feed and the second wake-up it can cause (`relay.md` 2.5), and the need to parse a message to route it (section 5).

### 4.3 Writing: one write per message, one write in flight, the producer writes

Each pipe end that writes has one `Sender`. Its rules:

- One write per frame. A frame is encoded into one buffer and written with one `WriteFile`. This keeps f94e761's property.
- Encoding happens outside the sender's lock, into a buffer the encoding thread owns. Only the decision to write and the `WriteFile` call are under the lock, so the contention c9564c6 found is cut to the length of one system call.
- At most one write is in flight per pipe. While one is in flight, later frames wait in the sender's backlog, in order, under that pipe's overflow policy (section 4.4).
- The producer writes. A thread that sends when no write is in flight and the backlog is empty issues the overlapped `WriteFile` itself. If it completes at once, which is the normal case when the reader keeps up, it is done: no other thread wakes. If it pends, the slot keeps the buffer and the `OVERLAPPED` until the completion arrives (the branch's lifetime rule, `overlapped.rs` module documentation), and the producer returns at once; it never waits.
- The I/O thread finishes. The completion of a write that pended arrives at the process's I/O thread for that pipe (section 6), which writes the next backlog frame, again as one write, until the backlog is empty.
- Stamps that must be taken "just before the write" are patched into the encoded buffer's timing block at their fixed offset, under the lock, immediately before `WriteFile`. This is what the branch could not do with stamps inside the JSON.
- A paced send, for the host's PCM only: the producer waits while a write is in flight, on that write's completion together with the host's command event (section 7). This is the backpressure that paces synthesis today, kept, and made able to notice a cancel while it waits.
- A failed write closes the sender: later sends fail, as the branch made the per-child queue do.

So: a frame costs one encode, one `WriteFile` and no wake-up on the sending side in the common case; a writer thread wakes only when the reader has fallen behind by a whole pipe buffer.

### 4.4 Overflow policy, per pipe

The policies are today's, moved into each `Sender`'s backlog. They act only when a write is in flight, which is exactly when merging matters.

- Listener to verbatim.exe: urgent messages (pongs, `Ready`) ahead of facts; facts merged one per element and kind, a newer one replacing the waiting one and going to the back (`listener.rs` module documentation, NVDA's limiter rule); the menu-end message after facts.
- verbatim.exe to an outpost or the listener: today's `Queue` (`writer.rs:99-140`): at most 64 waiting; a fact replaces a waiting fact with the same key; past the bound the oldest fact is dropped; `NodesHeld` replaces a waiting `NodesHeld`; ping, shutdown and hand-over messages always get in.
- An outpost to verbatim.exe: today's outbound rules (`outbound.rs:1-27`): pongs, `Ready` and `TargetExited` first; events merged by object and kind; terminal output combined under the flood policy; replies, notifications and faults never merged and kept in order. Message positions for `NodesHeld` are numbered in the order frames are written, as today.
- verbatim.exe to a host: a `Speak`, a `Cancel` or a setting at a time; nothing accumulates.
- A host to verbatim.exe: paced, as above; nothing is dropped.

One behaviour changes. Today verbatim.exe's reader threads drain each outpost into the reducer's unbounded channel, so the outpost's merge rules almost never act and a burst waits in Core instead. When the reducer reads the pipes itself (section 6.2), a reducer that is busy lets the pipe fill, the outpost's writes pend, and its merge rules act where they were designed to. This is NVDA's behaviour in effect, since NVDA's limiters act on its one queue. It changes what a flood sounds like only in that stale events of the same kind for the same object are merged earlier. Decision 8.

### 4.5 Ordering guarantees

- Within one pipe, frames arrive whole and in the order the sender wrote them. The sender's lock orders writers on the same pipe; the backlog is first in, first out apart from the stated merges.
- The listener's messages to one outpost reach it in the listener's order, held facts before later ones, and "forwarding ended" behind every fact before it (`focus-pipeline.md` 12.4, `writer.rs:16-20`). The relay thread alone reads the listener's pipe and routes, as `relay.md` 4.1 requires; this design does not change it.
- An outpost's messages reach the reducer in the outpost's queue order: one pipe, read by one thread.
- Across sources, the reducer takes its inputs in one order, the order its completion port received them: outposts' read completions, and one posted packet per key, GUI command, control-plane command or supervisor notice. Every read on the reducer's inbound pipes completes through the port (no skip on success), and each mailbox item posts its own packet, so the port's first-in, first-out queue is the single arrival order `focus-pipeline.md` section 15, decision 7 asked for; a typed character's echo still comes before the output it causes.
- Core's requests to an outpost are written in the order the reducer issued them; relayed facts and requests to the same outpost are ordered by the sender's lock, as the shared queue mutex orders them today.
- The host's commands are in order on one pipe: a `Cancel` for an utterance is always ahead of the next `Speak`.

### 4.6 The child's side stays simple

A child calls the transport once at startup with its two inherited handles and gets a `Sender` and an inbound stream. It sends from any thread with one call that never blocks. It gives its I/O thread a function to run for each received frame. That is all a child's code sees; `OVERLAPPED`, completions, buffers and the backlog are inside the module. Tests that run an outpost in the test process (`run_pipe`, `crates/verbatim-outpost/src/outpost/mod.rs:1066`) get a connected pipe pair from the same module, so they exercise the real transport.

### 4.7 Every boundary under the transport

- Listener to verbatim.exe: the listener's producing threads (UIA callback threads on main, or stage A step 6's per-application reading threads, and the WinEvent thread) write inline. The listener's outbound thread goes. The menu-end grace becomes a threadpool timer whose callback sends, a deliberate 50 ms wait that is unchanged in length.
- verbatim.exe to the listener: `HookInstalled`, `HookRemoved`, pings, shutdown, written by the relay thread, read by the listener's I/O thread.
- verbatim.exe to an outpost: relayed facts written inline by the relay thread in the same pass that read them; Core's requests (queries, cancels, `NodesHeld`, settings) written inline by the reducer (finding 9, c9564c6). A write that pends is finished by the relay thread, which serves every outpost's outbound pipe on its port.
- An outpost to verbatim.exe: the worker's events and replies written inline by the worker; pongs and `Ready` by the I/O thread; `TargetExited` by the target-watch thread. The outpost's outbound thread goes; its I/O thread finishes pended writes. Read in verbatim.exe by the reducer on its port.
- verbatim.exe to a host and back: the synth thread writes commands and reads frames itself, waiting on the read's event and the cancellation event (section 7). Core's host reader thread goes; the host's reader thread goes.
- The control plane keeps its own pipe code (`server.rs:13-34`). It shares no thread with the path. It does share locks with it today (section 6.6), and that is fixed. Moving it onto the transport is possible later and is not proposed.

## 5. Serialization

Established: JSON is written and parsed at every pipe; the relay parses each fact and serializes it again (`owner.rs:1126-1171`, `991-1003`; `relay.md` 2.5 to 2.7). Measured in `supervisor-latency.md` and reported in `relay.md` section 3: bursts route in about 10 µs per fact once the owner thread is running, which includes the parse, the routing and the new message's serialization. The relay's median hand-off was 0.06 ms and its tail was threads waking late, not work.

Estimated cost of JSON on a UIA focus today, in release: three encode-and-parse pairs (listener to Core, Core to outpost, outpost to Core) of messages of a few hundred bytes to a few kilobytes, at a few microseconds each to encode and somewhat more to parse because every string is allocated: about 20 to 40 µs in all, around ten times that in debug. Against a budget of 1 to 2 ms for verbatim.exe's stage and a median hop of 60 µs, this is a small part of the time and none of the tail.

Recommendation (decision 1):

- Keep JSON bodies at every pipe. They are readable in logs and captures, serde already guarantees that older peers read newer messages, and a binary encoding would save an estimated 10 to 30 µs per announcement in release, too little to justify a second encoding to maintain.
- Route on the header instead of parsing. The listener writes each fact's body already shaped as the outpost's `DeliverFact` contents, with the target process, the start permission and the fact key in the header. The relay thread reads the header, routes by process id, merges held facts and queue entries by key bytes, patches its stamps into the timing block, and writes the same bytes to the outpost. It never parses or allocates a fact. Its own work per fact falls to a header decode and a copy (estimated well under 1 µs in release), which makes verbatim.exe's own stage nearly pure wake-up and keeps it far from the budget even in debug builds. The listener's few non-fact messages (`Ready`, pongs, "forwarding ended", "menu ended") have kinds of their own and small bodies, parsed as now.
- Revisit binary bodies only if the ledger shows encoding or parsing above 5 percent of any stage's 99th percentile.
- PCM stays raw. The host's PCM is copied about ten times between eSpeak NG and the device (`latency-audit.md` finding 12); the transport removes two of those copies (Core's reader thread and its channel), and the rest are left as a follow-up, since each costs microseconds.

`relay.md` 4.2 left "forwarding bytes without parsing" optional, to be done only if parsing dominated. It is made part of the design here, because the frame header is being introduced anyway, because it gives the stamps a fixed place that can be patched just before the write, and because it removes allocation from the one thread every fact passes.

## 6. The thread model

### 6.1 One scheduling policy for every hot-path thread

This builds the 2026-10-08 decision (`docs/roadmap.md:60-62`) for every thread on the path, as finding 3 asked, with one rule for all of them.

- Every Verbatim process (verbatim.exe, the listener, every outpost, every synthesizer host) opts out of execution-speed power throttling first thing (`SetProcessInformation(ProcessPowerThrottling)`, the branch's `opt_out_of_power_throttling`, 7e6e562). This is a quality-of-service request, not a power plan; Balanced stays.
- Every thread named in sections 6.2 to 6.5 as on the path also opts out of thread power throttling (`prioritize_current_thread`).
- Priority by what a thread does when it wakes:
  - A thread that only moves messages, microseconds per wake, then waits again: `THREAD_PRIORITY_HIGHEST` (base 10). These are the keyboard hook thread, verbatim.exe's relay thread, each child's I/O thread, and the WinEvent threads.
  - A thread that computes on the path, sometimes for milliseconds, or calls into an application: `THREAD_PRIORITY_ABOVE_NORMAL` (base 9). These are the reducer, the synth thread, each host's synthesis thread, each outpost's worker, and the listener's per-application reading threads under stage A step 6.
  - The audio thread: MMCSS "Pro Audio", unchanged.
  - Everything else: normal (the GUI, the control plane, launch and retire helpers, the watchdogs, the reporting thread, the router).
- Threads Verbatim does not own (UIA's callback threads) keep their priority. Verbatim does no lasting work on them: they encode and write one frame, or hand the event to an owned thread.
- The levels are the starting point. The floor benchmark (step 2) compares `ABOVE_NORMAL`, `HIGHEST` and `TIME_CRITICAL` for the message-moving threads under load and chooses the lowest that meets the pass criteria; the levels are named once per role in code, as cfbd323 does.
- Platform-neutral crates get their priority through a callback: `SpeechManagerConfig::thread_started` (cfbd323) for the synth thread; the audio thread already registers with MMCSS in `verbatim-audio-wasapi`.
- Timer resolution is not raised, as `relay.md` 4.4 decided: nothing on the path waits on a timer after this design.

Reasoning, from `relay.md` section 3 (inferred there, to be confirmed by the ETW attribution of step 2): a thread made ready at normal priority waits for a running normal-priority thread's quantum, about 31 ms on client Windows and longer for the foreground application, which matches the 2 to 30 ms tails; a higher-priority thread pre-empts at once. The audit's synthesis stage, one thread with no hand-off, had 5.2 percent over 2 ms and a 48.7 ms maximum, which only priority addresses. A raised thread that loops would starve others; every raised thread here waits between messages, and the flood scenarios are run with the priorities in place.

NVDA sets no thread priority, process priority, power-throttling state or timer resolution anywhere (no `SetThreadPriority`, `AvSetMmThreadCharacteristics` or `timeBeginPeriod` under `nvda/`, searched for this design).

### 6.2 verbatim.exe

- Keyboard hook thread (`verbatim-input-hook`). Woken by the system's low-level hook message for each key. Decides against the lock-free gesture snapshot; on a key that cancels speech it sets the speech cancellation (atomics and a `SetEvent`, section 8) and posts a non-blocking cancel to the mixer; then posts the key's input to the reducer's port. Review and navigation gestures go straight to the reducer too (finding 10); the router keeps lock keys, GUI commands and the time. Never blocks on: any lock, any channel that can be full, any pipe. `HIGHEST`, as decided on 2026-10-08. Needs `verbatim-process` as a dependency of `verbatim-input-windows` for the scheduling call, which cfbd323 found missing.
- Relay thread (`verbatim-relay`, the supervisor's owner, db16548). Waits only in `GetQueuedCompletionStatusEx` on its port, with a timeout equal to the next heartbeat or sweep. Woken by: a read completion on the listener's pipe; a pended write's completion on any child's command pipe; its mailbox (launch helpers, retirements, hang-check results, the reducer's forwarded outpost notices, the app's `EnsureSpawned`, `Views` and `Shutdown`). Reads frames, routes facts by header, holds facts for a starting outpost, writes inline. Never blocks on: process waits, thread joins, hang detection, file I/O or logging on the delivered path (all made event-driven or moved off by the branch). `HIGHEST`.
- Reducer thread (`verbatim-reducer`). Waits only on its own completion port. Woken by: a read completion on any outpost's pipe; one posted packet per key, GUI event, control-plane command or supervisor notice. Parses each outpost frame (stamping the outpost on node ids, as the reader threads do today), forwards `Ready`, pongs, `TargetExited`, faults and the pipe's end to the relay thread's mailbox, reduces, writes requests to outposts inline, and speaks: the speech queue's logic runs on this thread under the speech lock (section 8). A frame over 256 KiB (terminal output) is parsed by a helper thread while the reducer goes on with other sources; that outpost's later frames wait behind it, so each outpost's order holds. Never blocks on: a pipe, the speech lock beyond its short critical sections, the ledger (lock-free stamps), the control plane. `ABOVE_NORMAL`. Today's per-outpost reader threads go.
- Synth thread (`verbatim-synth`). Waits on its job channel between jobs; during a hosted job, on the host pipe's read event and the job's cancellation event together. Writes `Speak` and `Cancel`, reads PCM, trims, converts and appends to the mixer, then wakes the audio thread. Takes the speech lock briefly when a job ends to start the next. `ABOVE_NORMAL`. Its reader thread for the host goes.
- Audio thread (`verbatim-audio`). Waits on the device's event and its wake event (`crates/verbatim-audio-wasapi/src/lib.rs:508-525`). Mixes under the mixer lock, but makes every WASAPI call (`GetCurrentPadding`, `GetBuffer`, `ReleaseBuffer`, `Start`, `Stop`, `Reset`) with the lock released (finding 11), as `recover` already does (`mixer.rs:1279-1323`). Reports "first sample played" by pushing one record to the reporting thread, and nothing else: no logging, no control-plane lock. MMCSS "Pro Audio".
- Reporting thread (`verbatim-report`, new). Normal priority, off the path. Woken once per announcement, after its first sample, by the audio thread's push. Assembles the ledger's timeline, writes the latency line, broadcasts speech frames to control-plane subscribers. Everything the ledger and the control plane do on the audio and speech threads today moves here.
- Off the path, unchanged: the router, the GUI thread and its events thread, the control plane's accept, session and writer threads, launch and retire helpers, the shutdown thread.

The speech queue thread (`manager.rs:349-350`) goes (section 8). So do the listener reader, every per-outpost reader and writer, and every host reader.

### 6.3 The listener

- I/O thread (its main thread). Waits on its port. Woken by: commands from verbatim.exe (`HookInstalled`, `HookRemoved`, pings, shutdown); a pended write's completion; the menu-end grace timer's packet. `HIGHEST`.
- WinEvent thread: the message loop for its global hooks. Builds each fact from the event's arguments and local reads, and writes it inline. `HIGHEST`.
- UIA's callback threads (UIA's own): on main, build the fact from cached properties and write it inline; under stage A step 6, hand the sender to its application's reading thread.
- Per-application reading threads (stage A step 6): judge and read the focus in one remote operation, then write inline. `ABOVE_NORMAL`.
- The outbound writer goes.

### 6.4 An outpost

- I/O thread (its main thread). Waits on its port. Woken by: a frame from verbatim.exe (it handles commands and pushes facts and queries into the intake, as `handle_command` does today, `outpost/mod.rs:801-816`); a pended write's completion. Answers pings itself. `HIGHEST`.
- Event thread and UIA's callback threads: push into the intake. The event thread is `HIGHEST`.
- Worker: takes intake entries, calls into the application under its deadline, publishes by writing inline (finding 8). `ABOVE_NORMAL`. Publishing keeps "queuing never waits" (`outbound.rs:7-8`): the inline write is overlapped and never waits, and when one is in flight the message joins the backlog.
- Watchdog: no longer woken per entry. The worker arms the deadline on a waitable timer (`SetWaitableTimer`), which wakes nobody until it expires (finding 12). The watchdog waits on that timer and its stop event, and does what it does today on expiry. Normal priority.
- Target-watch thread: unchanged, off the path.
- The outbound writer goes.

### 6.5 A synthesizer host

- One thread. Idle, it waits on the command pipe's read event, an overlapped read always pending. Woken by a command, it reads every whole frame waiting, then speaks, applies a setting or does nothing for a cancel of something already finished. While synthesizing it learns of a new command from the same event without a thread: eSpeak NG's callback checks the event with a zero wait once per chunk (60 ms of audio, `crates/verbatim-synth-espeak/src/lib.rs:65`), and the OneCore driver waits on its operation's completion and that event together. Writes PCM by paced sends (section 4.3). `ABOVE_NORMAL`, power throttling off.
- The reader thread goes, and with it the second hop inside the host.

### 6.6 Locks and logging shared with the path

- The speech lock: the queue's lanes and in-flight state, held by the reducer while it presents and queues an utterance, by the synth thread while it starts the next job, and by the settings host briefly. Never taken by the hook thread, whose cancel is lock-free (section 8).
- The mixer lock: held only for mixing and bookkeeping, never across a WASAPI call (finding 11).
- The sender locks: held for one `WriteFile`.
- The ledger's mutex (`crates/verbatim-app/src/latency.rs:268-269`), taken today three or four times per message by the reducer and by the speech and audio threads: goes. Stamps travel with the data and are assembled by the reporting thread (section 10).
- The control plane's registry lock (`crates/verbatim-control/src/server.rs:1079-1096`), taken today by the speech queue thread (`has_speech_subscribers`, `broadcast_speech`) and the audio thread (`broadcast_speech_started`, `latency.rs:505-507`): taken only by the reporting thread.
- The process-wide stdout lock: no thread on the path logs on the delivered path. Today the router logs every gesture at info (`main.rs:1723`), the audio thread writes the latency line (`latency.rs:502-504`), and the outpost logs at debug inside its intake lock in test runs (`crates/verbatim-outpost/src/outpost/intake.rs:306-313`). The first moves to debug and after the forward, the second to the reporting thread, the third out of the lock.

## 7. The synthesizer host protocol

### 7.1 Messages

- Every frame from the host names its utterance: `Pcm(utterance, format, samples)`, `Mark(utterance, mark)`, `Done(utterance)`, `Failed(utterance, reason)`. Setting replies and `Ready` carry none.
- Commands are as today: `Speak(sequence)` (the sequence carries its utterance id), `Cancel(utterance)`, `SetSetting`.
- The first `Pcm` of an utterance carries the host's stamps in the timing block: when the host read the `Speak`, when the driver started, and when the first chunk was ready (section 10).

### 7.2 The next Speak goes right behind a Cancel (finding 4)

Established today: a cancel sets the job's flag but leaves the job in flight (`manager.rs:906-917`), the queue starts nothing until `SynthFinished` (`manager.rs:830-833`, `973-976`), and that comes only after the host has seen the cancel, finished its chunk and sent `Done` (`synth-hosted/src/lib.rs:378`).

The design:

- On cancellation, the synth thread writes `Cancel(u)` and returns from the job at once. The hosted driver remembers that `u` is draining. The speech lock's in-flight state is cleared by the synth thread itself, which then starts the next waiting job, if any, without any other thread waking.
- The next job writes `Speak(v)` immediately. Reading for `v`, the driver discards every frame tagged `u`, `Done(u)` and `Failed(u)` included. It never waits for them.
- The host reads `Cancel(u)` and `Speak(v)` together at its next chunk boundary (or at once if idle or waiting in a paced send), stops `u`, writes `Done(u)`, and starts `v`.
- Backpressure while draining: the host may be blocked in a paced send of `u`'s audio when the cancel comes, with up to one pipe buffer (4 KiB, about 90 ms of 22 kHz audio) and one frame waiting. The synth thread drains them as the first reads of `v`, in microseconds, and the host's paced send has meanwhile seen the command event. Nothing waits for playback.
- Recovery is unchanged: a host silent for `HANG_TIMEOUT` while Core waits for it, or that ends, fails the utterance in flight and is replaced (`synth-hosted/src/lib.rs:10-14`). A host that answers with an utterance it was never given is misbehaving, as an out-of-turn answer is today.

Cost removed, estimated from the audit: about ten wake-ups across two processes and up to one eSpeak NG chunk of CPU, on every key that interrupts an utterance still being synthesized; 1 to 3 ms at the median, more in the tail, none of it hidden for a typed echo or a review command.

NVDA's eSpeak NG driver behaves the same in effect: `stop` sets `isSpeaking` false and stops the player (`nvda/source/synthDrivers/_espeak.py:249-266`), the synthesis callback aborts at its next chunk (`_espeak.py:150-154`), and the next `speak` is queued on the background thread behind that last chunk (`_espeak.py:218-224`, `246`). Its chunk is longer, from a 300 ms buffer (`_espeak.py:371-373`).

### 7.3 Cancellation signalled, not polled (finding 5)

- In verbatim.exe: the speech job's cancellation becomes a token in `verbatim-speech` with a flag and an optional waker. `SynthSink` gains a way for a driver to register the waker; the default does nothing, so the capture synthesizer and tests are unchanged. The hosted driver registers a waker that sets a Windows event, and waits on that event together with the host pipe's read event. The 10 ms slices (`synth-hosted/src/lib.rs:336-357`) go.
- In the host: cancellation is the command itself. The command pipe's pending read signals its event when any command arrives. eSpeak NG's callback checks the event once per chunk with a zero wait, a system call of about a microsecond (estimated), and reads the waiting frames when it is set. The OneCore driver waits with `WaitForMultipleObjects` on the operation's completion and that event, replacing its 10 ms poll (`crates/verbatim-synth-onecore/src/lib.rs:366-381`); the host wires the event into the driver at construction, inside Windows crates.
- Cost removed, estimated: with OneCore, up to about 30 ms between a key and the host stopping (two 10 ms polls each rounded up to the 15.6 ms timer tick), half that on average. With eSpeak NG, a few milliseconds at the start of an utterance.

### 7.4 The host's threads

One thread, as section 6.5 says. The command pipe and the PCM pipe are the transport's, with the PCM pipe's 4 KiB buffer kept.

## 8. Speech and audio inside verbatim.exe

### 8.1 Hops (finding 7)

- The speech queue thread goes. `SpeechManager::speak` runs the queue's logic (presenting through the theme, the lanes, focus validity, the interrupt cancel) on the calling thread, the reducer, under the speech lock, and hands a job to the synth thread only when it is idle. Today a reducer's speak wakes the queue thread, which wakes the synth thread; afterwards it wakes the synth thread only.
- The synth thread reads the host's pipe itself (section 7). Core's host reader thread and its two-slot channel go.
- The host's reader thread goes (section 6.5).
- Three wake-ups fewer on every announcement and every echo before the first sample.

### 8.2 Cancellation from the hook stays non-blocking, and stops waiting for the audio thread

Today a cancel is a message to the queue thread, which makes a blocking request to the audio thread and waits until the device has stopped (`mixer.rs:397-415`, `631-633`), then goes on. Afterwards:

- The hook raises a cancellation generation (an atomic, with `cancel_through`'s key fence kept as an atomic maximum, `manager.rs:174-189`), sets the in-flight job's token, which wakes the synth thread through its waker, and posts a cancel request to the mixer that does not wait. That request carries a registration watermark, so an utterance registered after the cancel was issued is never cut by it; that is the guarantee `cancel_all`'s documentation gives today ("utterances registered after this returns are unaffected"), kept without the wait.
- The lanes are cleared, and their utterances reported cancelled, by the next thread that takes the speech lock and sees a newer generation: the reducer as it speaks, or the synth thread as it finishes a job.
- The audio thread carries out the cancel at its next wake, which the request triggers: `Stop` and `Reset`, as decided on 2026-10-08, with the lock released.
- Pause and resume from Shift work the same way.

NVDA cancels on its main thread, behind queued work (`docs/nvda/input.md`, "What a key press does to speech"). Verbatim's cancel stays ahead of queued work, as now.

### 8.3 The mixer lock (finding 11)

The audio thread computes what to mix and copies it under the lock, then releases it for every device call. A cancel or an append arriving meanwhile waits for the copy, not for the driver. The underrun warning and the latency line leave the audio thread for the reporting thread.

### 8.4 The router (finding 10)

Review and navigation gestures go from the hook to the reducer's port, removing one wake-up and a synchronous info-level log write per key. The router keeps lock keys (with their 30 ms delay), the menu gesture, GUI commands and the time. The ordering the router keeps today, input-handled marks and idle barriers behind a lock key whose announcement is still to come (`main.rs:1677-1717`), stays with the router for exactly those gestures; a review key bound to the reducer carries its own handled mark.

### 8.5 Audio device behaviour, kept as its own step

Not communication, so not designed here; placed in the thread model so the steps do not conflict.

- Keeping the device awake (finding 1, decided 2026-10-08, `docs/roadmap.md:52-58`): the audio thread writes silence one engine period at a time for the awake period, 30 seconds by default. In this thread model that is more wakes of the audio thread only, at its device period, with nothing else woken; new speech never waits behind more than one period of silence. NVDA: `audioAwakeTime` and `wasSilence_playFor` (`nvda/source/nvwave.py:252-258`, `364-366`).
- The engine period (finding 2): measured first, from the ledger's `IAudioClock` stage (section 10); `IAudioClient3`'s shorter shared period tried only with the stream kept awake. D15 records that the development machine's device offers none shorter than 10 ms.
- The earcon re-mix (finding 6, decided 2026-10-08): the audio thread re-mixes audio already handed to the device. It is the audio thread's work, under the mixer lock, with device calls outside it, as above.

## 9. Wake-ups from the event or key to the first sample

A wake-up is a thread made ready from a wait, counted in order along the path; the first thread to receive the OS event counts as one. Threads woken off the path (the watchdog today, the reporting thread) are not counted. The application's own work is not counted. "Today" is main at 4960991, established from the code (after `latency-audit.md` section 2, counted the same way). "After" is this design, estimated by the same counting.

### 9.1 A UIA focus change, steady state, outpost ready

Today, 17:
1. UIA's callback thread in the listener.
2. The listener's outbound thread.
3. verbatim.exe's listener reader.
4. verbatim.exe's owner.
5. verbatim.exe's writer for the outpost.
6. The outpost's main thread.
7. The outpost's worker (the watchdog is also woken).
8. The outpost's outbound thread.
9. verbatim.exe's reader for the outpost.
10. The reducer.
11. The speech queue thread (plus two more, the audio thread and the queue thread again, when the announcement interrupts).
12. The synth thread.
13. The host's reader thread.
14. The host's main thread.
15. verbatim.exe's reader for the host.
16. The synth thread again.
17. The audio thread.

After, 9:
1. UIA's callback thread in the listener, which writes inline.
2. verbatim.exe's relay thread, which writes inline.
3. The outpost's I/O thread.
4. The outpost's worker, which writes inline.
5. The reducer, which queues the speech inline and cancels without waiting.
6. The synth thread, which writes `Speak`.
7. The host's thread.
8. The synth thread again, with the first PCM.
9. The audio thread.

Under stage A step 6 the listener's reading thread adds one: 10.

NVDA, about three: UIA's callback thread queues the event and requests an immediate pump (`nvda/source/core.py:1180-1199`); the main thread handles it and speaks; eSpeak NG's background thread synthesizes and feeds the WASAPI player from its callback (`_espeak.py:200-216`). The extra six are the price of D9, D13 and D18's isolation, one process boundary each way plus the host.

### 9.2 An MSAA focus, the outpost's own hook in

Today, 12 (13 with the watchdog): the outpost's event thread, the worker, the outbound thread, verbatim.exe's reader, the reducer, the queue thread, the synth thread, the host's reader, the host's main thread, verbatim.exe's host reader, the synth thread, the audio thread.

After, 7: the event thread, the worker, the reducer, the synth thread, the host's thread, the synth thread, the audio thread.

NVDA, about three, but every WinEvent other than focus waits for a 10 ms delayed pump (`nvda/source/core.py:55`, `1013`; `IAccessibleHandler/internalWinEventHandler.py:193`).

### 9.3 A typed character's echo

Today, 9 (11 when it interrupts speech, since the cancel's blocking request to the audio thread is serial): the hook thread, the reducer, the queue thread, the synth thread, the host's reader, the host's main thread, verbatim.exe's host reader, the synth thread, the audio thread. If the interrupted utterance is still being synthesized, finding 4's round trip adds about ten more before the echo's `Speak` is sent.

After, 6: the hook thread, the reducer, the synth thread, the host's thread, the synth thread, the audio thread. The cancel runs in parallel (the hook wakes the synth thread and the audio thread directly), and the echo never waits for the cancelled utterance.

NVDA, about two: the main thread (it hears the character from inside the application through its helper) and eSpeak NG's thread.

### 9.4 A caret key (UIA, the application moves the caret)

Today, 17: the hook thread, the reducer, verbatim.exe's writer for the outpost, the outpost's main thread, the worker (first check), the UIA callback with the caret event, the worker again, the outbound thread, verbatim.exe's reader, the reducer, the queue thread, the synth thread, the host's reader, the host's main thread, verbatim.exe's host reader, the synth thread, the audio thread. (The audit's "about 16" did not count the UIA callback.)

After, 11: the hook thread, the reducer (writes the watch inline), the outpost's I/O thread, the worker, the UIA callback, the worker again (writes inline), the reducer, the synth thread, the host's thread, the synth thread, the audio thread.

NVDA: the main thread runs the script, then polls for the caret every 10 ms, sleeping between checks (`nvda/source/editableText.py:70-162`, the sleep at `160`), then eSpeak NG's thread; about three wake-ups plus up to one 10 ms poll, rounded up to the timer tick.

### 9.5 A review key

A review key that reads the application (a review line through UIA), today 16: the hook thread, the router (with an info log write), the reducer, verbatim.exe's writer, the outpost's main thread, the worker, the outbound thread, verbatim.exe's reader, the reducer, the queue thread, the synth thread, the host's reader, the host's main thread, verbatim.exe's host reader, the synth thread, the audio thread.

After, 9: the hook thread, the reducer, the outpost's I/O thread, the worker, the reducer, the synth thread, the host's thread, the synth thread, the audio thread.

A review key answered from the reducer's own state: today 10, after 6.

NVDA: the input thread hands the key to the main thread, which reads (in-process for its own buffers, cross-process for the application) and speaks; eSpeak NG's thread; about three.

### 9.6 What the counts are worth

Estimated, not measured: at the relay's measured median of 0.06 ms per hand-off in release, removing eight hand-offs from a UIA focus saves about 0.5 ms at the median. The larger effect is on the tail. If each hand-off independently had a 0.5 percent chance of exceeding 2 ms (the middle of the measured 0.24 to 0.99 percent), a 17-hop path would exceed 2 ms somewhere about 8 percent of the time and a 9-hop path about 4 percent, before the priorities of section 6.1, which are aimed at the per-hop chance itself. Hops are not independent on a loaded machine; the ledger measures the real figures.

## 10. The latency ledger

### 10.1 Stages

Every hop has a stage. A stage inside one process is a duration measured with the performance counter (`Instant`) on one thread, or between two threads of one process; a stage that crosses processes is a difference of the shared clock (`now_us`, which on Windows reads `GetSystemTimePreciseAsFileTime`, derived from the performance counter and the same in every process). The stage names from 43adba8 are kept where they exist.

The event side, in path order:
- Windows: the WinEvent's raised time to its observation, as today (tick-count resolution, so partly quantization).
- Listener: observation to the frame written (in-process).
- Listener to relay (`ListenerToRelay`): written to the relay thread's read completion (cross-process).
- Outpost start (`OutpostStart`): time held for a starting outpost (in-process; reported separately and never pooled with delivered facts, as `relay.md` 7 requires).
- Relay (`Relay`): read completion to `WriteFile` (in-process, the duration patched into the header at the write).
- Relay to outpost (`RelayToOutpost`): write to the outpost's read completion (cross-process).
- For a request: the reducer's send to its `WriteFile` (`RequestToRelay`, in-process; after this design it is the encode and the write on the reducer, or the wait in the backlog when one is in flight).
- Outpost queue, caret wait, outpost read: as today.
- Outpost to reducer (`OutpostToRelay`, renamed for what it measures): the worker's `WriteFile` to the reducer's read completion (cross-process).
- Reducer wait (`ReducerWait`): the read completion's arrival on the port to the reducer starting on it, which is the reducer being busy, labelled so it is never mistaken for transport.
- Reducer: as today.
- Hook to reducer, for keys: the hook's callback to the reducer starting on it.

The speech side, in path order:
- To speech: the reducer's speak to the job taken by the synth thread (in-process). Time waiting behind earlier speech is reported and not counted, as today.
- Synth request: the job taken to `Speak` written (in-process).
- To host: `Speak` written to the host's read (cross-process).
- Host first chunk: the driver started to its first chunk ready (in the host, a duration). This is the split of synthesis the audit asks for, into request, first chunk and return.
- Host return: the first `Pcm` written to the synth thread's read completion (cross-process).
- Leading silence: as today.
- To the audio thread: the append to the audio thread's wake (in-process).
- Device queue: the first frame's write to the engine having taken it, by padding (in-process; today's "audio started").
- Device clock: the engine having taken it to `IAudioClock::GetPosition` reaching the first frame's position, converted to time with the position's own performance-counter stamp (in-process). This is the stage finding 2 needs.

### 10.2 Stamps, in memory, off the path

- Stamps travel with the data: in each frame's timing block across processes, and in each speech job inside verbatim.exe. No thread takes a shared lock to record a stamp.
- Each process also keeps a fixed-size in-memory ring of (trace, stage, counter value) for measurement builds, written out at exit, as `relay.md` 4.5 planned. Nothing on the path writes a file or a console.
- The audio thread pushes one record per announcement, after its first sample, onto a channel to the reporting thread; that is the only hand-off the ledger adds, and it is after the measured span. The reporting thread assembles the timeline (today's `Stages::breakdown`, `latency.rs:104-249`), writes the latency line, serves `verbatim-inspect latency` and broadcasts to the control plane.
- Clocks: if skew between processes ever shows as a negative cross-process stage, the shared clock can move to the performance counter scaled to microseconds, as `relay.md` 4.5 allows; the branch kept the precise file time for the reason in section 3.3.

### 10.3 The measurement scenario the audit asks for first

Today no recorded latency line has measured the synthesizer host or a real device: the suite uses the in-process capture synthesizer and the silent device (`latency-audit.md` section 1). The new scenario, `measure_speech_real`, runs Verbatim with eSpeak NG through its host and the real WASAPI device. It drives one fixed target, the Windows Forms text box the suite already uses for edit controls, through a fixed sequence: focus changes into and out of it, typed characters, caret keys, a review key, and a key pressed during a long announcement (the finding 4 case). It records every stage of 10.1 for each announcement, and reports distributions per stage (count, median, 99th and 99.9th percentiles, maximum, share over 1 and 2 ms), not a pass or fail at first; D15's budgets become assertions once the figures are known. It runs locally, where a real device exists (the dev VM runner-direct, and once on bare metal), not in hosted CI, which has no sound device. A OneCore variant measures the cancel stages of 7.3.

## 11. Migration

Each step is small, builds and passes `cargo xtask ci`, and is verified before the next. Measurements come first. Live checks follow the targeted-run rule; full suite runs are named where a step changes how every outpost talks to Core.

### Step 1. Make the paused branch's measurement parts build, and record the baseline

- Builds from the branch: f94e761 (one write per message), 43adba8 (the relay stages), and the `overlapped` module and the scheduling module as libraries only, split out of db16548 and 7e6e562, with no behaviour change.
- Extends the ledger with the return-path and speech stages of 10.1 that today's architecture can stamp (synth request, host first chunk, host return, device clock).
- Removes: nothing. Adds the measurement.
- Tests: the ledger's unit tests for each new stage; the host protocol's round-trip test with the timing block.
- Pass: CI green; one run of the measure set in debug and release produces every stage. These figures are the baseline every later step is compared with.

### Step 2. The platform floor and the speech-path scenario

- The relay floor benchmark (5cbcb4e) as `relay.md` section 6 specifies, F0 to F4, plus F5: sender and receiver both overlapped with inline writes (this design's child side), and F6: a host-shaped round trip (command, a fixed CPU burst standing in for a first chunk, paced reply).
- `measure_speech_real` (10.3), on the dev VM and once on bare metal.
- ETW attribution (WPR, CPU usage, precise) for one run of each condition.
- Removes: nothing.
- Pass: the floors and their attribution are recorded; the priority levels of 6.1 are chosen from them; the baseline of the speech path through the real host and device exists.

### Step 3. The scheduling policy

- 7e6e562 and cfbd323 completed: every process opts out of power throttling; every thread in section 6 gets its level, including the keyboard hook (adding `verbatim-process` to `verbatim-input-windows`), the host reader threads while they still exist, and the host's threads.
- Removes: the non-silent part of the per-hop tail, if the inference holds.
- Tests: the scheduling module's tests (the branch's, reading the priority back).
- Pass: the measure set and `measure_speech_real`, compared with step 1: every stage's share over 2 ms no worse, and the per-hop shares lower; the flood scenarios (`conhost_leave_flood`, the terminal flood) pass with the priorities in place and no thread runs hot (CPU per thread from the ETW trace).

### Step 4. The transport: frames, pipes, senders

- `verbatim_process::transport`: the frame format of 4.2, the pipes of 4.1 (overlapped on both ends, 64 KiB for event pipes), the `Sender` of 4.3 with backlog policies supplied by the caller, the inbound frame reader, the port and mailbox from db16548.
- Every Core-child pipe moves to it, with today's threads still in place: the same threads, the new framing, inline writes where the producer can (the per-child writer threads still exist in this step but find nothing to do in the common case).
- Removes: line framing, the second write and the split read, `CreatePipe`.
- Tests: the frame reader on a frame split across reads, several frames in one read, a frame at the limit and one past it; the sender's overload policies (`writer.rs` tests, kept); a write that pends is finished by the I/O side and keeps order; a sender whose reader stops delays no other pipe; a child's end opened by a lower-integrity process is refused.
- Pass: CI; the supervisor's mockapp scenarios; then the full suite, because every outpost's pipes change.

### Step 5. The relay on its port, routing on the header

- db16548's relay thread, with the listener's pipe read in frames and routed by header (no parse, no re-serialization), held facts merged by key bytes, stamps patched at the write; c9564c6's writes from the pushing thread. The listener's reader and the per-child writer threads go.
- Removes: two wake-ups in verbatim.exe on every relayed fact, and its parse and allocations.
- Tests: `relay.md` 7.5's (held facts before later facts, the hand-over order test `a_hand_over_message_gets_through_a_full_queue_behind_the_facts_before_it`, a stuck child delays no other), kept from the branch's `relay.rs` tests.
- Pass: `relay.md` section 7 items 2 to 4 as approved: the "verbatim.exe" stage at most 1 ms maximum, 99.9th percentile at most 0.2 ms in release and 0.5 ms in debug; listener to relay plus relay at the 99.9th percentile at most 1 ms and at most 2 ms maximum with every exception attributed by ETW; requests' relay stage at most 1 ms. The focus and terminal scenarios, then the full suite.

### Step 6. The children's side: inline writes and one I/O thread

- The listener and every outpost use the transport's child side: producers write inline; one I/O thread on a port reads commands and finishes pended writes. The listener's outbound thread, the outpost's outbound thread and the outpost's separate pipe-reading main thread become that one I/O thread. The menu-end grace becomes a threadpool timer.
- Removes: one wake-up on every listener fact and on every outpost event and reply (finding 8).
- Tests: the outbound queue's merge and ordering tests against the new sender; message positions for `NodesHeld` counted in write order; pongs written while the worker is stuck in a call.
- Pass: listener stage and "outpost to reducer" stage each at least as good as step 5's figures, with the share over 2 ms lower than the baseline's 5.3 percent; the focus, MSAA hand-over and terminal scenarios; the full suite.

### Step 7. The reducer reads the outposts' pipes

- The reducer waits on its own port: outposts' inbound pipes (completions always through the port), and one posted packet per key, GUI event, control-plane command and supervisor notice. It forwards supervisor messages to the relay's mailbox. Large frames are parsed by a helper, keeping each outpost's order. verbatim.exe's per-outpost reader threads go.
- Removes: one wake-up on every outpost message.
- Tests: the reducer's arrival order across an outpost's frame and a key posted after it (the key comes second) and before it (first); a large frame from one outpost does not delay a key or another outpost's event, and that outpost's next frame comes after it; a closed pipe reaches the supervisor; an outpost's merge rules act when the reducer is held (decision 8).
- Pass: "outpost to reducer" and "reducer wait" stages reported separately; flood scenarios show bounded latency and no hang; the full suite.

### Step 8. Review keys straight to the reducer

- Finding 10, as 8.4. The router keeps lock keys, the menu and the time; the gesture log line moves to debug, after the forward.
- Removes: one wake-up and an info log write per review and navigation key.
- Tests: a review key's handled mark is reported after its speech is queued; a lock key's announcement still precedes the marks behind it.
- Pass: the review and navigation scenarios; the "hook to reducer" stage for review keys at most that of caret keys.

### Step 9. The host protocol: tags, Speak behind Cancel, signalled cancel, one host thread

- Section 7: frames tagged with their utterance; the hosted driver returns on cancel and discards the old utterance's frames; the cancellation token with a waker in `verbatim-speech`; the synth thread reads the host pipe itself; the host is one thread on its command event; OneCore waits on its completion and the command event.
- Removes: finding 4's round trip (about ten wake-ups), both 10 ms polls, Core's host reader thread and the host's reader thread.
- Tests: a scripted host driver that blocks per chunk on a gate: a cancel during an utterance followed by a new one sends `Speak` for the new one before `Done` for the old one is read, and no audio of the old one reaches the sink after the cancel; a cancel before any audio stops a OneCore-shaped driver whose operation never completes, with no timer involved (the test waits for the `Done`, with only an overall failure bound); a host killed mid-utterance fails it and the next utterance starts a new host, as today.
- Pass: in `measure_speech_real`, a key pressed during a long announcement has its echo's "synth request" to first sample no longer than an echo pressed in silence plus one chunk's synthesis time; the OneCore variant's cancel-to-stop has no 10 ms steps.

### Step 10. The speech manager without its queue thread, and the audio thread unblocked

- 8.1 to 8.3: queue logic under the speech lock on the speaking thread; the hook's cancel lock-free, with the mixer's non-blocking cancel and its watermark; the mixer lock released across WASAPI calls; the ledger's line, underrun warning and control-plane fan-out on the reporting thread; the ledger's mutex replaced by carried stamps.
- Removes: the queue thread's wake on every announcement, the cancel's two serial wake-ups, the shared ledger and registry locks on the path.
- Tests: `verbatim-speech`'s pipeline tests with the capture synthesizer, unchanged in what they assert: ordering of lanes, fenced-off speech from an earlier key arriving after a later key's cancel ends cancelled, focus validity, pause; a new test that an utterance registered after a cancel was issued is never cut by it; the mixer's tests with the lock released around a device stub that records calls.
- Pass: the typed-echo and caret-key scenarios; `measure_speech_real`'s "to speech" and "to the audio thread" stages; the full suite.

### Step 11. The outpost's per-event costs

- Finding 12: the watchdog's deadline on a waitable timer (no wake per entry); the entry's description formatted only when debug logging is on; logging moved out of the intake lock; `NodesHeld` coalesced with the next request or sent when the reducer goes idle (evidence, not a timer).
- Removes: a watchdog wake per entry; a string format per entry; a file write under a lock in test runs; three wake-ups after most focus changes.
- Tests: the watchdog still abandons a worker past its deadline (mockapp's held-call test); held nodes are still released (the held-objects tests).
- Pass: the outpost queue and read stages no worse; the operation ledger's counts unchanged (`crates/mockapp/tests/call_counts.rs`).

### Step 12. Measure again, and the audio step

- The measure set in debug and release, `measure_speech_real` on the dev VM and once on bare metal, each compared with step 1's baseline; the full suite; ten runs of the related scenarios.
- Then, separately, the audio step: the awake period (finding 1), the engine period trial (finding 2), the earcon re-mix (finding 6), each measured with the device stages of 10.1.

Ordering against stage A (`docs/roadmap.md:197-211`): this replaces stage A step 9 (the relay) in the order Dickson set, after step 6's revisions; steps 1 to 3 can run whenever the desktop is free, since they measure and change scheduling only.

## 12. What carries over

### 12.1 From `relay.md`

Unchanged:
- Section 4.1's core: the supervisor's owner thread becomes the relay, on a completion port; it reads the listener's pipe and routes in the same pass; at most one write in flight per child; a stuck child holds only its own queue; the ordering argument resting on one routing thread; the alternatives rejected in 4.2 (a separate relay thread, direct pipes, MMCSS for the relay).
- Section 4.3: one write per message.
- Section 4.4: priority and power throttling under Balanced, no timer resolution change, with the list of threads widened by section 6.1 here.
- Section 6: the floor benchmark, with two variants added.
- Section 7's pass criteria and section 9's two approved decisions.

Changed:
- "Outposts' messages to Core keep their reader threads" (4.1): replaced; the reducer reads them on its port (step 7).
- "The pipe for the synthesizer host can stay anonymous" (4.1): it moves to the transport, overlapped, read by the synth thread itself.
- Children's ends: overlapped too, with inline writes (4.3 here), where `relay.md` kept them synchronous.
- Forwarding without parsing (4.2): made part of the design through the frame header, instead of an option.
- Ledger fields (4.5): the stamps move from JSON fields into the frame's timing block, patched just before the write, which also fixes what the branch found impossible.
- Other threads' pushes to a child (4.1): written by the pushing thread (finding 9, c9564c6), not posted to the relay thread.

### 12.2 The paused branch's commits

- f94e761 (one write per message): kept as it is in step 1; superseded by the transport's framing in step 4, which keeps the property.
- 43adba8 (the relay's ledger stages): kept in step 1 for the baseline; its stage names are kept; its `EventTiming` fields move into the frame header in step 4.
- db16548 (the relay on a completion port): kept, split. Its `overlapped` module (pipe, `Port`, `PipeIo`) becomes the core of the transport in step 4; its relay thread, `Mailbox` and `Inbox`, and its event-driven retirement and hang checks are kept in step 5; its line splitter and parse path for the listener are replaced by the frame reader and header routing.
- 7e6e562 (priorities and power throttling): kept, in step 3.
- 5cbcb4e (`cargo xtask relay-floor`): kept, with F5 and F6 added, in step 2.
- c9564c6 (the pushing thread writes): kept in step 5, changed so encoding happens outside the per-child lock.
- cfbd323 (work in progress on more threads' priorities): kept and finished in step 3 (the role constants and the `thread_started` callback); its speech queue role becomes obsolete in step 10, and its outpost outbound writer role in step 6.

## 13. Decisions for Dickson

1. Keep JSON bodies, and route on a binary frame header so verbatim.exe never parses or re-serializes a fact. NVDA: no IPC, so no equivalent. Recommend: adopt; revisit binary bodies only if the ledger shows encoding above 5 percent of a stage.
2. Children's pipe ends become overlapped too, and every producer writes its own message when nothing is ahead of it, which removes the listener's and the outposts' writer threads. Recommend: adopt.
3. The reducer reads the outposts' pipes on its own completion port, and the port's order is the reducer's one order of arrival; frames over 256 KiB are parsed by a helper while keeping each outpost's order. Recommend: adopt. Risk: the outposts' pongs reach the supervisor through the reducer (section 14).
4. The scheduling policy of 6.1: `HIGHEST` for threads that only move messages (the hook, the relay, the children's I/O threads, the WinEvent threads), `ABOVE_NORMAL` for threads that compute on the path (the reducer, the synth thread, the host, the outposts' workers, the listener's reading threads), MMCSS for audio, UIA's own threads left alone, every process out of power throttling. It keeps the 2026-10-08 levels (the hook high, speech and synthesis above normal) and extends them by rule. NVDA: sets none. Recommend: adopt, with the levels confirmed by the floor benchmark.
5. The speech queue thread goes; the queue's logic runs under a lock on the thread that speaks, and the hook's cancel is lock-free and no longer waits for the audio thread. NVDA: speech is managed on the main thread, and a key's cancel runs there behind earlier work. Recommend: adopt.
6. The host protocol of section 7: frames tagged by utterance, the next `Speak` sent right behind a `Cancel`, cancellation signalled by the command's arrival, one host thread. NVDA: the same effect, its eSpeak NG callback aborting at the next chunk and the next speech queued behind it. Recommend: adopt.
7. Review and navigation keys go from the hook to the reducer, and the gesture log moves to debug. Recommend: adopt.
8. Backpressure reaches the outposts: when the reducer is behind, an outpost's merge rules act instead of a backlog growing in Core. NVDA: its limiters act on its one queue, which is the same effect. Recommend: adopt, verified by the flood scenarios.
9. The budget reading approved for `relay.md` applies to every verbatim.exe stage of section 10, including the reducer's request writes and the speech stages inside verbatim.exe: each within 1 to 2 ms of its own work, each hop no worse than the platform floor, every exception attributed by trace, measured once on bare metal. Recommend: adopt.
10. `measure_speech_real` reports distributions first and becomes a gate (D15's budgets) only once its figures are known; it runs locally and on bare metal, not in hosted CI. Recommend: adopt.
11. Amend `architecture.md` sections 1 and 10: internal channels are named pipes, single-instance, connected before the child starts, admitting only the current user at Verbatim's integrity level, instead of anonymous pipes with "no named endpoint". Recommend: adopt; it is what overlapped I/O requires, and the relay's approval already accepted it for the listener and outposts.

## 14. Risks

- One relay thread carries every child's writes and the listener's reads, and one reducer thread now carries every outpost's reads. A bug that blocks either stops all relaying or all input. The rule that neither ever blocks, logs on the delivered path or does file I/O is enforced in review, and each gets a watchdog as `focus-pipeline.md` 6.4 asks for threads others wait on.
- Outposts' pongs travel through the reducer. A stalled reducer would make healthy outposts look dead. The reducer forwards supervisor messages in its completion handler before any reduction, and a reducer stall is a fault of its own that its watchdog reports.
- Overlapped buffers and `OVERLAPPED`s must outlive their operations, in every process now, not only in verbatim.exe. The transport is the only owner of them, with the branch's rule (a slot dropped with an operation in flight leaks rather than frees) and its tests.
- A write that fits the pipe buffer completing at once is what makes inline writes cost no wake. It is the branch's observation and Windows' documented quota behaviour for named pipes; step 2 counts pended writes to confirm it on this machine.
- Raised priorities could starve other threads if a raised thread ever loops; every raised thread waits between messages, and the flood scenarios run with them in place.
- On the Proxmox VM the measured in-process maximum is already 13 to 32 ms (`relay.md` section 1). Absolute maxima may be impossible there; the approved reading attributes exceptions by ETW rather than ignoring them, and the bare-metal run separates the VM's share.
- The wire format changes for every pipe. Core and its children always ship together; the header's length field lets a reader skip header fields it does not know.
- Moving the queue's logic onto the reducer puts presentation (the theme's flattening) on the reducer's time. It was on the queue thread before, so the total is the same; the "to speech" stage shows it.

## 15. Established and estimated, in brief

Established, from the code, recorded measurements or the branch's findings: today's pipes, threads and wake-up counts; the absence of priority and power-throttling calls on main; finding 4's wait for `SynthFinished`; the two 10 ms cancel polls; the writer and reader thread hops; the shared ledger, registry and stdout locks on the path; the measured per-hop medians and tails and the 5.3 percent tail to Core; the branch's findings in 3.3; NVDA's behaviour cited by file and line.

Estimated, to be measured by steps 1, 2 and 12: every "after" count's value in milliseconds; the cost of JSON per announcement; the share of the tail that priority removes; that pipe writes into a 64 KiB buffer complete at once; finding 4's and finding 5's savings; anything about the real host and the real device, which no run has yet measured.

## 16. Follow-ups found, not part of this design

- The audio thread's idle wait also bounds how long a request to reopen a failed device goes unnoticed (`mixer.rs:59-62`), and a failed open is retried after a fixed second (`mixer.rs:64-65`). That is a timed retry, not a wait for evidence; a device-change notification (`IMMNotificationClient`) would replace it. Not on the event path.
- PCM is copied about ten times between eSpeak NG and the device (`latency-audit.md` finding 12); this design removes two copies. The rest cost microseconds each.
- The control plane keeps its own overlapped pipe code (`crates/verbatim-control/src/server.rs:13-34`); it could later move onto the transport.
- The keyboard hook drops a gesture when the router's 64-slot channel is full (`crates/verbatim-input-windows/src/lib.rs:469-470`). Posting review keys to the reducer's port removes that for them; lock keys and GUI gestures keep the bounded channel.
