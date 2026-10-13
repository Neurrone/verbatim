# Communication and threading: one design from an event or a key to sound

Proposed 2026-10-11; decisions 2 to 12 approved by Dickson as recommended on 2026-10-13 (decision 1 as revised with him, decision 13 and the survey's points on 2026-10-11). Revised the same day after a review of what the user would hear and what the end-to-end suite would see, compared with today and with NVDA. Revised again on 2026-10-13 after the wake-up audit: section 18 makes wake-ups and processor time criteria of every step alongside latency, with the baseline they are judged against; Dickson decided 14 to 17 the same day. Section 17, "Regressions considered", lists every finding of that review, how this design resolves it, and the test or scenario that guards it. It replaces piecemeal fixes with one model for every pipe between Verbatim's processes and for every thread an event or a key press passes through on its way to the first audio sample. It subsumes `docs/design/relay.md` (approved 2026-10-11, paused for this design) and findings 3, 4, 5, 7, 8, 9, 10, 11 and 12 of `docs/design/latency-audit.md`; findings 1, 2 and 6 are audio work, placed in the thread model here but kept as a step of their own.

Researched on main at 4960991 and on the paused relay branch `worktree-agent-a867236cb53da6eae` (seven commits, f94e761 to cfbd323, not yet compiled). Nothing was built or run. Verbatim code is cited as `path:line` from the repository root; NVDA as `nvda/source/...:line`. A statement marked "established" comes from the code, from a recorded measurement, or from the relay branch's findings; one marked "estimated" is reasoning, with the reasoning given, and is a thing the measurement steps in section 11 must confirm.

## 1. Summary

- Today a UIA focus change wakes 17 threads in four processes before its first sample, an MSAA focus 12, a typed character's echo 9 (11 when it interrupts speech), a caret key 17, and a review key that reads the application 16 (established, section 9, after `latency-audit.md` section 2). Every hand-off has a measured tail: 0.24 to 0.99 percent of hand-offs over 2 ms on the relay (`relay.md` section 3), 5.3 percent over 2 ms on the way from an outpost to the reducer (`latency-audit.md` section 1).
- The design has four parts.
  - One transport module, `verbatim_process::transport`, for every pipe: overlapped named pipes made by verbatim.exe, one instance each, connected before the child starts; length-prefixed frames with a small binary header and a JSON body (raw samples for PCM); one `WriteFile` per frame; at most one write in flight per pipe; and a rule that the thread producing a message writes it itself when nothing is ahead of it, so no writer thread is woken in the common case. The same module serves verbatim.exe and its children.
  - One thread model: in each process, one I/O thread per role that waits only on a completion port or on events, a thread for each kind of work that may block (an outpost's worker, a synthesizer), and nothing else on the path. verbatim.exe keeps relaying every event (no direct route between children). The relay thread reads the outposts' pipes as well as the listener's, draining them continuously as today's reader threads do, and posts each whole frame to the reducer's completion port, so the order of outposts' events against keys stays today's and no outpost's merge rules start acting because the reducer is busy (decision 3 sets out the alternative, the reducer reading the pipes itself, and what it would change). The speech queue thread goes; the speech queue's logic runs under a lock on the thread that speaks, and a key's cancel cuts speech by a generation recorded on each utterance when it is admitted (section 8.2).
  - A synthesizer host protocol whose frames name their utterance, so the next `Speak` follows a `Cancel` at once instead of waiting for the cancelled utterance to end, and whose cancellation is signalled by the arrival of the command itself, never by a timer.
  - A latency ledger with a stage for every hop, stamps carried in each frame's header (patched in place just before the write) and in each speech job, assembled off the hot path by one reporting thread, and a measurement scenario through the real eSpeak NG host and a real WASAPI device, read from `IAudioClock`.
- After the design (estimated, by counting the threads in section 9): a UIA focus wakes 10 threads (11 under stage A step 6's listener reading threads), an MSAA focus 8, a typed echo 6 with no wait for a cancelled utterance, a caret key 12, a review key that reads the application 10. Each count that includes an outpost's message would be one lower if the reducer read the outposts' pipes itself (decision 3). NVDA, one process, wakes about three.
- What the user hears, and what the suite sees, does not change: speech frames reach the control plane as each speech event happens, in one ordered channel (section 8.6); the harness's idle barrier becomes a round through every child that covers frames in pipes and backlogs (section 6.7, which is stage A step 10); index marks and the key's `SpeechCancelled` reach the reducer as they do today (section 8.7); a started frame is never cut (section 4.3).
- One scheduling policy for every hot-path thread: every process opts out of power throttling; threads that only move messages run at `THREAD_PRIORITY_HIGHEST`; threads that compute on the path run `ABOVE_NORMAL`; the audio thread keeps MMCSS "Pro Audio". The power plan stays Balanced and timer resolution is not raised.
- Migration is thirteen small steps, measurements first. The relay branch's commits are mostly kept: section 12 says which are kept as they are, which change, and which are replaced.
- Thirteen decisions for Dickson, in section 13. Two of them, 3 and 8, are where the design could depart from today's order or from NVDA; each says so and recommends the reading that departs from neither.

## 2. Dickson's constraints, and how each is met

- verbatim.exe adds at most 1 to 2 ms, read as approved for `relay.md`: verbatim.exe's own stage within 1 to 2 ms, its hop no worse than the platform floor, every exception attributed by trace, measured once on bare metal too. Met by giving every verbatim.exe hop its own ledger stage (section 10) with the pass criteria of section 11, carried from `relay.md` section 7 and extended to the return path and the speech path.
- Events keep passing through verbatim.exe. Every listener fact is read by verbatim.exe's relay thread and written by it to the outpost; every outpost message is read by verbatim.exe's reducer. No pipe joins two children.
- The power plan stays Balanced. Only per-process and per-thread quality-of-service requests and thread priorities are used (section 6.1).
- Process isolation of D9, D13 and D18 stays. No process is merged or removed; the listener, one outpost per application and one host per synthesizer remain.
- No fixed waits, retries or polling in place of evidence. The design removes the two 10 ms cancel polls (`crates/verbatim-synth-hosted/src/lib.rs:49`, `crates/verbatim-synth-onecore/src/lib.rs:52`). Every wait it adds is on an I/O completion, an event or a channel. The deliberate waits listed in `latency-audit.md` section 4 (the listener's 50 ms menu-end grace, the 30 ms lock-key delay, the mixer's 10 ms start threshold) stay, as do deadline timers that detect a hang (the outposts' watchdog, the host's 10 s `HANG_TIMEOUT`), which are not polls.
- Platform-neutral crates stay free of Windows code, and Core stays independent of the operating system. Every platform-specific piece of this design lives in Windows crates: the transport and its named pipes, the completion ports, the I/O threads, the relay, the scheduling and priority calls, MMCSS and the WASAPI calls. `verbatim-core` and the other platform-neutral crates gain no Windows dependency and no dependency on the transport; `cargo xtask ci`'s dependency check keeps enforcing it. `verbatim-speech` and `verbatim-audio` gain only plain types and atomics: the speech generation and key fence, the pause word, the ended list, the speech-record channel, a cancellation token that can call a waker, and the `thread_started` callback the relay branch already added (cfbd323). Decision 13 proposes moving the outpost and listener message types into a platform-neutral crate.
- Efficiency as well as latency (Dickson, 2026-10-13): "We don't just want lowest latency at any cost, we also want efficiency as well, ideally as few wakeups as possible." Every step is judged on each thread's wake-ups and processor time as well as on the latency stages (section 18).
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
- Security: a DACL that admits only the current user's SID, with a mandatory label at verbatim.exe's own integrity level, so a lower-integrity process cannot open it (the branch's `pipe_security`). The child's end is opened by verbatim.exe with `CreateFileW` at once, before the child is created, so the single instance is connected and no other process can ever take it; the child inherits the opened handle as today. The name exists only between `CreateNamedPipeW` and that `CreateFileW`, microseconds, and even then the DACL and the first-instance flag guard it. Pipe names can be enumerated, so in those microseconds another process of the same user at the same integrity, or a higher-integrity one, could connect first; verbatim.exe's `CreateFileW` would then fail with "pipe busy". Any failure of that open aborts the launch: verbatim.exe closes the server handle, which ends whatever connection took the pipe, never retries the open and never calls `WaitNamedPipe`, and reports the launch as failed, as a failed `CreateProcessW` is reported today. A process able to do this can already do worse to verbatim.exe, so this is no new exposure; the rule only keeps an intruder's connection from ever carrying Verbatim's traffic.
- Both ends are overlapped. The child's end is opened with `FILE_FLAG_OVERLAPPED` too, so a child can write without ever blocking (section 4.3). This changes the relay branch's choice to keep children's ends synchronous. That choice was forced by `std::fs::File` aborting on overlapped handles; in this design no end, in any process, is wrapped in a `File`: both go only through the transport's types.
- Buffer: 64 KiB for the event pipes in both directions, so that a focus fact with its ancestors, or a reply with a line of text, completes its write at once (estimated: today's largest common messages are under 8 KiB; terminal output can be megabytes). The buffer is not a message size: a reader that has the frame's length asks for the whole rest of the frame in one `ReadFile` into one buffer of that size, never in chunks of its own choosing; whether that read completes once (data copied straight into a read already waiting, believed but not verified) or returns several times (data that arrived first and sat in the 64 KiB buffer, since a byte-mode pipe read returns as soon as some data is there) is the pipe driver's, and the reader simply asks again for the remainder. 4 KiB stays for the host's PCM pipe, whose small buffer is the backpressure that paces synthesis (`synth-hosted/src/lib.rs:51-53`). The floor benchmark (step 2) checks that a write that fits the buffer completes without pending, by counting pended writes, and counts how many completions a 1 MB and a 2 MB frame take with the read waiting first and with the data arriving first; the event pipes' buffer is raised only if those counts show it matters, since every pipe's buffer costs kernel memory.
- Completion: on every end served by a completion port, `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` and `FILE_SKIP_SET_EVENT_ON_HANDLE` are set, so an operation that completes at once is handled where it was issued. A read the relay thread completes at once on an outpost's pipe is handled in the same pass, and its frame posted to the reducer's port then, so the reducer's port still receives frames in their order of arrival (section 4.5). Under decision 3's alternative the reducer's inbound pipes would instead have every read's completion go through the port, so that its first-in, first-out order were the order of arrival.
- `architecture.md` section 1 says internal channels have "no named endpoint", and section 10 repeats it. That changes: they are named, but single-instance and already connected when anything else could look. This needs Dickson's agreement (decision 11).

### 4.2 Framing

Settled by decision 1's measurement (2026-10-13, release build, this development machine, serde_json): parsing and re-serializing a relayed UIA focus read with 12 ancestors (4,475 bytes) costs about 15 microseconds; encoding and parsing a small terminal event (723 bytes) about 2.4 microseconds; and a 1 MB terminal event about 1.8 to 2.0 milliseconds. The typical messages cost a few tens of microseconds at most, so the frames are plain JSON behind a readable text length header, as the Language Server Protocol's (`Content-Length: N`, a blank line, then the JSON body), the stamps are JSON fields set just before encoding, and verbatim.exe parses what it routes. Frames over 256 KiB are parsed by a helper, as section 6.2 says. The binary header below is kept as the alternative it was, should a later measurement show the cost to matter.

The binary alternative: every message on every pipe is one frame:

- a little-endian `u32`: the length of everything after it;
- a `u8` kind and a `u8` of flags;
- a `u16` header length, so the header can grow without breaking a reader;
- the routing fields, used only on the listener's pipe and Core's pipes to outposts: the target process id (`u32`, 0 for none), whether the fact may start an outpost (one bit of the flags), and the fact key as a length and its bytes (the bytes `FactKey` is compared by today, `protocol.rs:383`);
- the timing block, at fixed offsets: the stamps of section 10, each a `u64` of microseconds on the shared clock or a `u32` duration, zero for a point not passed;
- the body: JSON of the message type that lives where it lives today (`verbatim-outpost`'s protocol, `verbatim-speech`'s hosting types), or raw little-endian samples for PCM.

The maximum stays 32 MB (`protocol.rs:847-854`). A reader that sees a larger length treats the other end as failed, as now, before allocating. The pipe's end in the middle of a frame (the length or the body cut short) is the pipe's end with a fault logged, never a message: the partial bytes are discarded and the peer is handled as having exited. The host protocol's own frame functions (`hosting.rs:200-251`) are replaced by the transport's; `hosting.rs` keeps its message types, which are plain data in a platform-neutral crate.

What this removes: line splitting and the `read_until` scan, the second `WriteFile` for the line feed and the second wake-up it can cause (`relay.md` 2.5), and the need to parse a message to route it (section 5).

### 4.3 Writing: one write per message, one write in flight, the producer writes

Each pipe end that writes has one `Sender`. Its rules:

- One write per frame. A frame is encoded into one buffer and written with one `WriteFile`. This keeps f94e761's property.
- Encoding happens outside the sender's lock, into a buffer the encoding thread owns. Only the decision to write and the `WriteFile` call are under the lock, so the contention c9564c6 found is cut to the length of one system call.
- At most one write is in flight per pipe. While one is in flight, later frames wait in the sender's backlog, in order, under that pipe's overflow policy (section 4.4).
- The producer writes. A thread that sends when no write is in flight and the backlog is empty issues the overlapped `WriteFile` itself. If it completes at once, which is the normal case when the reader keeps up, it is done: no other thread wakes. If it pends, the slot keeps the buffer and the `OVERLAPPED` until the completion arrives (the branch's lifetime rule, `overlapped.rs` module documentation), and the producer returns at once; it never waits.
- The I/O thread finishes. The completion of a write that pended arrives at the process's I/O thread for that pipe (section 6), which writes the next backlog frame, again as one write, until the backlog is empty.
- Stamps that must be taken "just before the write" are patched into the encoded buffer's timing block at their fixed offset, under the lock, immediately before `WriteFile`. This is what the branch could not do with stamps inside the JSON.
- A paced send, for the host's PCM only: the producer waits while a write is in flight, on that write's completion together with the host's command event (section 7). This is the backpressure that paces synthesis today, kept, and made able to notice a cancel while it waits. Noticing a cancel never cancels the write: the host stops producing audio, but the write in flight stays pending until the pipe has taken all of it, which verbatim.exe's synth thread does promptly as its first reads after the `Cancel` (section 7.2).
- A started frame is never cancelled. No code in any process calls `CancelIo` or `CancelIoEx` on a write, and no writer abandons one: once a frame's `WriteFile` is issued, the frame is written whole or the pipe has failed. This is what keeps a byte-mode pipe in step with its reader; a frame cut part-way would make every later frame misread, and for the host it would look like a misbehaving host and cost a restart. Today's synchronous writes have this property by construction (`crates/verbatim-speech/src/hosting.rs:200-251`, `outbound.rs:369-415`); the transport keeps it by rule and by test (step 4).
- `Flushed`, the outbound queue's marker for "everything before this is written" (`outbound.rs:94-101`), means every earlier write has completed, not merely been issued.
- A failed write closes the sender: later sends fail, as the branch made the per-child queue do.

So: a frame costs one encode, one `WriteFile` and no wake-up on the sending side in the common case; a writer thread wakes only when the reader has fallen behind by a whole pipe buffer.

### 4.4 Overflow policy, per pipe

The policies are today's, moved into each `Sender`'s backlog. They act only when a write is in flight, which is exactly when merging matters.

- Listener to verbatim.exe: urgent messages (pongs, `Ready`) ahead of facts; facts merged one per element and kind, a newer one replacing the waiting one and going to the back (`listener.rs` module documentation, NVDA's limiter rule); the menu-end message after facts.
- verbatim.exe to an outpost or the listener: today's `Queue` (`writer.rs:99-140`): at most 64 waiting; a fact replaces a waiting fact with the same key; past the bound the oldest fact is dropped; `NodesHeld` replaces a waiting `NodesHeld`; ping, shutdown and hand-over messages always get in.
- An outpost to verbatim.exe: today's outbound rules (`outbound.rs:1-27`): pongs, `Ready` and `TargetExited` first; events merged by object and kind; terminal output combined under the flood policy; replies, notifications and faults never merged and kept in order. Message positions for `NodesHeld` are numbered in the order frames are written, as today.
- verbatim.exe to a host: a `Speak`, a `Cancel` or a setting at a time; nothing accumulates.
- A host to verbatim.exe: paced, as above; nothing is dropped.

Nothing here changes how often the outposts' merge rules act. Today verbatim.exe's reader threads drain each outpost into the reducer's unbounded channel, so the outbound merge rules act only when a reader thread itself falls behind, which is almost never, and a burst waits in Core. In this design the relay thread drains every outpost pipe continuously and posts each frame to the reducer's port, which is unbounded in the same way, so the same holds. Under decision 3's alternative, in which the reducer reads the pipes itself, a busy reducer would fill an outpost's pipe and its merge rules would act routinely; decision 8 sets out what that would change against NVDA and how it would be limited.

### 4.5 Ordering guarantees

- Within one pipe, frames arrive whole and in the order the sender wrote them. The sender's lock orders writers on the same pipe; the backlog is first in, first out apart from the stated merges.
- The listener's messages to one outpost reach it in the listener's order, held facts before later ones, and "forwarding ended" behind every fact before it (`focus-pipeline.md` 12.4, `writer.rs:16-20`). The relay thread alone reads the listener's pipe and routes, as `relay.md` 4.1 requires; this design does not change it.
- An outpost's messages reach the reducer in the outpost's queue order: one pipe, read by one thread.
- Across sources, the reducer takes its inputs in one order, the order its completion port received them: one posted packet per outpost frame (posted by the relay thread as soon as it has read the frame whole), per key, per `SpeechCancelled`, per index mark reached, and per GUI command, control-plane command or supervisor notice. The port's first-in, first-out queue is the single arrival order `focus-pipeline.md` section 15, decision 7 asked for; a typed character's echo still comes before the output it causes.
- This is today's order. Today each outpost's reader thread puts a frame into the reducer's channel as soon as it has read it, whatever the reducer is doing, and the hook sends keys to the same channel. The relay thread does the same for every outpost, so an outpost frame written before a key is pressed is reduced before that key even while the reducer is busy. NVDA keeps the same order for UIA events, which its flusher thread queues as they come, independent of main-thread load (`nvda/source/eventHandler.py:48-67`); for MSAA it lets a key's script go ahead of WinEvents its limiter has not yet flushed (`nvda/source/IAccessibleHandler/__init__.py:1019-1036`, `nvda/source/core.py:1025-1027`). Under decision 3's alternative, keys would overtake outpost frames still unread in a pipe while the reducer was busy, UIA ones included; that is the main reason it is not recommended.
- Core's requests to an outpost are written in the order the reducer issued them; relayed facts and requests to the same outpost are ordered by the sender's lock, as the shared queue mutex orders them today.
- The host's commands are in order on one pipe: a `Cancel` for an utterance is always ahead of the next `Speak`.

### 4.6 The child's side stays simple

A child calls the transport once at startup with its two inherited handles and gets a `Sender` and an inbound stream. It sends from any thread with one call that never blocks. It gives its I/O thread a function to run for each received frame. That is all a child's code sees; `OVERLAPPED`, completions, buffers and the backlog are inside the module.

A child never exits with a write pending. Process exit cancels outstanding I/O, which would cut the last frame and leave verbatim.exe reading a truncated one. So a child's shutdown, after its last message (an outpost's `Close`, `outbound.rs:351-358`), waits on its I/O thread until the backlog is empty and the write in flight has completed, then exits. The wait is for evidence, the completion; it is bounded only by the existing shutdown limit, after which verbatim.exe ends the child as it does today, and a frame cut that way is the pipe's end with a fault logged (section 4.2), never a message.

Tests that run an outpost in the test process (`run_pipe`, `crates/verbatim-outpost/src/outpost/mod.rs:1066`) get a connected pipe pair from the same module, so they exercise the real transport.

### 4.7 Every boundary under the transport

- Listener to verbatim.exe: the listener's producing threads (UIA callback threads on main, or stage A step 6's per-application reading threads, and the WinEvent thread) write inline. The listener's outbound thread goes. The menu-end grace becomes a threadpool timer whose callback sends, a deliberate 50 ms wait that is unchanged in length.
- verbatim.exe to the listener: `HookInstalled`, `HookRemoved`, pings, shutdown, written by the relay thread, read by the listener's I/O thread.
- verbatim.exe to an outpost: relayed facts written inline by the relay thread in the same pass that read them; Core's requests (queries, cancels, `NodesHeld`, settings) written inline by the reducer (finding 9, c9564c6). A write that pends is finished by the relay thread, which serves every outpost's outbound pipe on its port.
- An outpost to verbatim.exe: the worker's events and replies written inline by the worker; pongs and `Ready` by the I/O thread; `TargetExited` by the target-watch thread. The outpost's outbound thread goes; its I/O thread finishes pended writes. Read in verbatim.exe by the relay thread on its port, which handles pongs, `Ready`, `TargetExited` and the pipe's end itself, as the owner does today, and posts every other frame whole to the reducer's port.
- verbatim.exe to a host and back: the synth thread writes commands and reads frames itself, waiting on the read's event and the cancellation event (section 7). Core's host reader thread goes; the host's reader thread goes.
- The control plane keeps its own pipe code (`server.rs:13-34`). It shares no thread with the path. It does share locks with it today (section 6.6), and that is fixed. Moving it onto the transport is possible later and is not proposed.

### 4.8 Large frames and head-of-line blocking

- Reading: once a reader has a frame's length, it asks for the whole rest of the frame with one `ReadFile` into one buffer of that size (section 4.1, as corrected on main in 899a484), and asks again for whatever remains if the pipe driver completes that read short. A frame of up to 32 MB, such as a megabyte of terminal output, is written with one `WriteFile`, which pends until the reader has taken it, and is read whole; with one write in flight per pipe, frames never interleave. The floor benchmark of step 2 counts how many completions a 1 MB and a 2 MB frame take.
- Who waits for whom, today: each outpost's outbound thread writes a message whole with blocking writes (`outbound.rs:369-415`), and verbatim.exe's reader for that outpost reads it whole before the next. A small frame written after a large one on the same pipe, such as a reply to a key's request behind a large terminal output, waits until the large one has been read. Keys and other outposts are not held, since each outpost has its own reader thread and the hook sends keys straight to the reducer's channel. The reader drains continuously, whatever the reducer is doing, and the backlog waits in the reducer's unbounded channel.
- After, with the relay reading (decision 3 as recommended): the same. The relay thread has one read outstanding per outpost pipe; a large frame's read stays pending while the relay serves every other pipe, so it holds up only the frames behind it on its own pipe, as today. Once a frame is whole the relay posts it to the reducer's port at once, so the pipes drain continuously and the reducer takes frames from its port at its own pace. The backlog therefore lives where it does today, in verbatim.exe's memory in front of the reducer, unbounded, and latency under a flood grows with it as it does today; the "reducer wait" stage of section 10 shows it. A frame over 256 KiB is parsed by a helper thread while the reducer goes on with other sources; that outpost's later frames wait behind the parse, so each outpost's order holds and the helper's result re-enters the port as a posted packet, ordered at the parse's completion as today's reader orders a large message at the end of its parse.
- Under decision 3's alternative, the reducer would re-arm an outpost's read only when it reached that pipe's previous completion, so a pipe would drain at the reducer's pace: a reply behind a large frame would wait longer than today whenever the reducer was busy, the pipe would fill, and the outpost's backlog would grow and merge (decision 8).
- The host's 4 KiB PCM pipe: a PCM frame larger than the buffer, which a OneCore chunk can be, is taken in several reads. That is correct only because a started frame is never cancelled (section 4.3).

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

Starvation of normal-priority threads on a small machine (inferred, to be measured in step 3). On a two-processor VM running a debug build, a flood can keep the raised threads busy enough to fill both processors: the reducer, the synth thread, the host and the outposts' workers at `ABOVE_NORMAL`, and the relay, the I/O threads and the WinEvent threads at `HIGHEST`. The threads at normal priority then wait: Verbatim's own GUI thread, the control plane's writers, the router (the time and the lock keys) and the foreground application. Verbatim reads its own GUI through an outpost whose step deadline is 400 ms (`crates/verbatim-outpost/src/outpost/worker.rs:109`), so a GUI thread starved for that long would cost focus announcements in the settings dialog; Windows' balance-set manager rescues a starved ready thread only after seconds. Step 3 therefore runs `rapid_tabbing_in_settings`, `settings_dialog_keys` and `menu_and_settings_dialog` while a terminal flood runs in another window, on this machine in a debug build (no new VM is created for it, Dickson, 2026-10-11; a small machine may not be reproducible locally, and then the check is recorded as run on this machine only), and reads from the ETW trace how long the GUI thread and the foreground application's threads waited ready but not running. The policy stands only if those scenarios pass and the GUI thread's longest ready wait stays under 100 ms, a quarter of the outpost's deadline. If it does not, the computing roles drop to normal priority and only the message-moving threads stay raised, since they run for microseconds per wake and cannot fill a processor; that fallback is measured the same way.

NVDA sets no thread priority, process priority, power-throttling state or timer resolution anywhere (no `SetThreadPriority`, `AvSetMmThreadCharacteristics` or `timeBeginPeriod` under `nvda/`, searched for this design).

### 6.2 verbatim.exe

- Keyboard hook thread (`verbatim-input-hook`). Woken by the system's low-level hook message for each key. Decides against the lock-free gesture snapshot; on a key that cancels speech it raises the speech generation and the key fence (atomics, section 8.2), signals the synth thread's cancellation event and the audio thread's wake event, and posts `SpeechCancelled` to the reducer's port; on Shift alone it toggles the pause word (section 8.8); then posts the key's input to the reducer's port. Review and navigation gestures go straight to the reducer too (finding 10); the router keeps lock keys, GUI commands, the menu and the time. Never blocks on: any lock (it no longer takes the mixer's mutex, which today's `cancel_all` request does, `mixer.rs:397-415`), any channel that can be full, any pipe. `HIGHEST`, as decided on 2026-10-08. Needs `verbatim-process` as a dependency of `verbatim-input-windows` for the scheduling call, which cfbd323 found missing.
- Relay thread (`verbatim-relay`, the supervisor's owner, db16548). Waits only in `GetQueuedCompletionStatusEx` on its port, with a timeout equal to the next heartbeat or sweep. Woken by: a read completion on the listener's pipe or on any outpost's pipe; a pended write's completion on any child's command pipe; its mailbox (launch helpers, retirements, hang-check results, the reducer's idle-barrier rounds, the app's `EnsureSpawned`, `Views` and `Shutdown`). Reads frames; routes the listener's facts by header, holds facts for a starting outpost, writes inline. From an outpost's pipe it handles pongs, `Ready`, `TargetExited` and the pipe's end itself, as the owner does today (`owner.rs:1096-1121`), and posts every other frame to the reducer's port whole and unparsed, then re-arms that pipe's read at once; it decodes only the frame's kind from the header. Its work per outpost frame is a header decode and a `PostQueuedCompletionStatus`, which never blocks. Never blocks on: process waits, thread joins, hang detection, file I/O or logging on the delivered path (all made event-driven or moved off by the branch). `HIGHEST`.
- Reducer thread (`verbatim-reducer`). Waits only on its own completion port. Its inputs, each one posted packet, in the order posted: an outpost frame (from the relay thread); a key's input, `SpeechCancelled` (both from the hook); an index mark reached (from the audio thread, section 8.7); a GUI event; a control-plane command, the idle barrier's `AwaitIdle` among them; a supervisor notice; a helper's parse result. Parses each outpost frame (stamping the outpost on node ids, as the reader threads do today), reduces, writes requests to outposts inline, and speaks: the speech queue's logic runs on this thread under the speech lock (section 8). A frame over 256 KiB (terminal output) is parsed by a helper thread while the reducer goes on with other sources; that outpost's later frames wait behind it, so each outpost's order holds. Never blocks on: a pipe, the speech lock beyond its short critical sections, the ledger (lock-free stamps), the control plane. `ABOVE_NORMAL`. Today's per-outpost reader threads go.
- Synth thread (`verbatim-synth`). Waits on its job channel between jobs; during a hosted job, on the host pipe's read event and the job's cancellation event together. Writes `Speak` and `Cancel`, reads PCM, trims, converts and appends to the mixer, then wakes the audio thread. Takes the speech lock briefly when a job ends to start the next. `ABOVE_NORMAL`. Its reader thread for the host goes.
- Audio thread (`verbatim-audio`). Waits on the device's event and its wake event (`crates/verbatim-audio-wasapi/src/lib.rs:508-525`). Mixes under the mixer lock, but makes every WASAPI call (`GetCurrentPadding`, `GetBuffer`, `ReleaseBuffer`, `Start`, `Stop`, `Reset`) with the lock released (finding 11), as `recover` already does (`mixer.rs:1279-1323`). Cuts every source whose generation the speech generation has passed and applies the pause word (sections 8.2 and 8.8). Its reporting role, all of it non-blocking pushes and posts made at the moment the thing happens: it posts each index mark reached to the reducer's port (section 8.7); it pushes a speech record to the reporting thread's channel for each utterance started, each sound played, each pause or resume applied and each utterance ended (section 8.6); and it pushes each ended utterance's id onto the ended list the speech lock's holders drain (section 8.7). It does no logging and takes no control-plane lock. MMCSS "Pro Audio".
- Reporting thread (`verbatim-report`, new). Normal priority, off the path. Takes speech records from one ordered, lock-free channel, into which the reducer, the synth thread and the audio thread push each speech event at the moment it happens (section 8.6), and broadcasts each to control-plane subscribers as today; answers the idle barrier when its record comes through the same channel; assembles the ledger's timeline from the stamps carried in the records and writes the latency line. Everything the ledger and the control plane do on the audio and speech threads today moves here.
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

- The speech lock: the queue's lanes, the in-flight job, `handed_on` and the order of setting changes against jobs, held by the reducer while it presents and queues an utterance, by the synth thread while it starts the next job, and by the settings host briefly. Never taken by the hook thread or the audio thread, whose parts are lock-free (section 8).
- The mixer lock: held only for mixing and bookkeeping, never across a WASAPI call (finding 11).
- The sender locks: held for one `WriteFile`.
- The ledger's mutex (`crates/verbatim-app/src/latency.rs:268-269`), taken today three or four times per message by the reducer and by the speech and audio threads: goes. Stamps travel with the data and are assembled by the reporting thread (section 10).
- The control plane's registry lock (`crates/verbatim-control/src/server.rs:1079-1096`), taken today by the speech queue thread (`has_speech_subscribers`, `broadcast_speech`) and the audio thread (`broadcast_speech_started`, `latency.rs:505-507`): taken only by the reporting thread.
- The process-wide stdout lock: no thread on the path logs on the delivered path. Today the router logs every gesture at info (`main.rs:1723`), the audio thread writes the latency line (`latency.rs:502-504`), and the outpost logs at debug inside its intake lock in test runs (`crates/verbatim-outpost/src/outpost/intake.rs:306-313`). The first moves to debug and after the forward, the second to the reporting thread, the third out of the lock.

### 6.7 The harness's idle barrier: a round through every child (stage A step 10)

What works today (`crates/verbatim-app/src/main.rs`, `answer_idle_waiters`): the reducer answers `AwaitIdle` only when it has handled the input numbered `after_input`, its channel's length is zero, no request to an outpost is outstanding, no outpost is starting and no focus is wanted; it then asks the speech queue thread to run the reply once it has taken in everything given to it before (`SpeechManager::after_queued`), so every utterance queued before the reply has already been broadcast on the same control-plane connection. Every scenario ends with it (`SpeechCollector::expect_nothing_more`, `crates/verbatim-e2e/src/speech.rs`), and `take_until_idle` relies on it.

Why it cannot stay as it is: a completion port has no length; the speech queue thread, on which the reply runs, goes (section 8); and it has never covered frames still in an outpost's pipe or backlog, events an outpost has not yet published, or caret watches in flight, which is the gap stage A step 10 was to close ("the idle barrier covers outposts' and the listener's work in flight", `docs/roadmap.md`; and Dickson's note after step 6 that it answers only once every outpost's work in flight is done, events not yet published and caret watches included).

The design: one barrier, built once, as stage A step 10 and as this migration's step 6a, before anything changes who reads the outposts' pipes. It is an in-band round, so it covers whatever is in pipes and backlogs without counting them.

- The reducer, given `AwaitIdle` for round n, waits until it has handled input `after_input`, then posts round n to the relay thread's mailbox.
- The relay thread writes `IdleQuery(n)` to the listener. The listener answers `IdleAnswer(n)` once nothing is in progress on its WinEvent thread or its per-application reading threads, through its sender behind every fact written before it, never merged and never sent ahead as urgent.
- On reading the listener's answer, the relay thread writes `IdleQuery(n)` to every outpost, ready or starting, in the same pass, so each query is behind every fact relayed to that outpost before it.
- Each outpost answers `IdleAnswer(n)` once the query has reached the front of its intake (so everything that arrived before it has been handled), its worker is idle, and no caret watch, text read or other deferred work is pending; the answer goes through its outbound queue behind every message queued before it, never merged and never urgent, so it cannot overtake an event or a reply. An outpost whose pipe ends during the round counts as having answered.
- The answers reach the reducer as frames like any other. When it has handled every outpost's answer for round n, the reducer dequeues with zero timeout everything already in its port and handles it, then checks today's conditions: no request outstanding, no outpost starting, no focus wanted. If they hold, the round has passed. If not, it waits for them to clear and runs round n+1, so the barrier answers only after a round in which every party was idle.
- The reply: until migration step 10, through `after_queued` as today. From step 10, the reducer pushes the barrier's record into the speech record channel (section 8.6); since every utterance the reducer admitted was pushed there under the speech lock before it, the reporting thread broadcasts every earlier speech record and then answers the waiter, on the same connection, in that order.
- The cost is a few milliseconds per barrier, a round trip through each child, paid only when the harness asks.

How it combines with stage A step 10: it is that step. Stage A's order (Dickson, 2026-10-11) put step 10 after the terminal redesign; this design needs it before migration step 7, so it is pulled forward into this migration as step 6a, and stage A's step 10 is then done. Decision 12.

Test: in mockapp, a barrier issued while the reducer is held and an outpost has a frame in its pipe and another in its backlog is not answered until both are reduced and their speech broadcast; one issued while an outpost has a caret watch pending is answered only after the watch resolves; one issued while the listener is reading a focus is answered after the resulting fact's speech.

### 6.8 Supervision: pings, pongs and the wedge verdict

- Today: the owner pings every child every 3 s; an outpost's pipe-reading main thread queues the pong as urgent (`crates/verbatim-outpost/src/outpost/mod.rs`, the `Ping` arm of `handle_command`); verbatim.exe's reader thread for that outpost sends it straight to the owner (`owner.rs:1096-1102`), never through the reducer. Three missed intervals, 9 s, is a wedge (`crates/verbatim-outpost/src/supervisor/mod.rs:51-56`, `policy.rs`).
- After: the relay thread writes pings; the outpost's I/O thread answers them itself, as an urgent message that goes ahead of the backlog (and behind only a write already in flight, which a started frame must be, section 4.3); the relay thread reads the pong from the outpost's pipe itself (section 6.2). The reducer is not on the way, so a stalled reducer can never make a healthy outpost look wedged, and no outpost is ever judged wedged while its pipe is unread, because the thread that reads it is the thread that judges.
- A stalled reducer is a fault of its own: its watchdog (section 14) reports it, and nothing is killed for it.
- Tests: a pong is answered while the outpost's worker is stuck in a call (step 6); in mockapp, the reducer held for longer than three ping intervals ends no outpost and is reported as the reducer's stall (step 7).
- Under decision 3's alternative the pong would reach the relay only through the reducer, and the rule needed instead is in section 6.9.

### 6.9 Decision 3's alternative: the reducer reads the outposts' pipes

This is what the design first proposed. It saves one wake-up on every outpost message (section 9), and changes four things, each of which it would have to state and guard:

- Order under load: the reducer re-arms an outpost's read only when it dequeues that pipe's previous completion, so while it is busy, frames written to a pipe beyond the one read already armed are not yet "arrived", and a key posted meanwhile is reduced before them. Examples: a focus event written just before a review key, so the review reads the old focus and the focus announcement then cuts it off; a caret event of one key reduced after the next key's input. For UIA events this departs from NVDA, which never lets a key overtake a UIA event its flusher has received; for MSAA it matches NVDA. Re-arming each read as the first thing the reducer does on a completion narrows the window to frames beyond one read per pipe, and does not close it.
- Merging: a busy reducer fills an outpost's pipe, so the outbound merge rules act routinely; decision 8.
- Supervision: pongs reach the relay through the reducer. The rule would be: the relay never judges an outpost wedged on missed pongs unless, since its last pong was due, the reducer has entered a wait on its port with nothing to do (its reads all armed, so a pong written would have completed one); the reducer stamps the time it last entered an empty wait for the relay to read. While that has not happened the reducer is behind, which its watchdog reports as its own stall. Test as in 6.8.
- Head-of-line: a pipe drains at the reducer's pace (section 4.8).

The idle barrier of 6.7 needs no change for it, since the round is in-band.

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
- Backpressure while draining: the host may be blocked in a paced send of `u`'s audio when the cancel comes, with up to one pipe buffer (4 KiB, about 90 ms of 22 kHz audio) and one frame waiting. The synth thread drains them as the first reads of `v`, in microseconds, and the host's paced send has meanwhile seen the command event. The host does not cancel that send: the frame in flight is completed whole, and only then does the host stop `u` (section 4.3), so the PCM pipe never holds a partial frame and no restart follows. Nothing waits for playback.
- The hosted driver discards `u`'s frames whenever it reads from the host, not only while reading for `v`: also while it waits for a setting's reply, so a setting applied right after a cancel is answered without `u`'s audio being taken for it.
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

### 8.2 Cancellation by generation: non-blocking, and exact

What protects the order today: the hook's cancel is a message on the queue thread's channel (`SpeechControl::cancel_through`, `crates/verbatim-speech/src/manager.rs`), in order with every `Speak`. A `Speak` that reaches the queue first is cancelled by `cancel_everything`; one that reaches it after carries a key number and is fenced off if a later key has cancelled (`cancelled_through`); and `cancel_all` waits for the audio thread (`mixer.rs:397-415`, `631-633`). So nothing admitted before the cancel survives it, and nothing admitted after it is cut. The first version of this design replaced that with a mixer cancel carrying a registration watermark and lanes cleared lazily; that let speech through in two interleavings: the synth thread popping an utterance queued before the key and registering it after the watermark, and the reducer passing the fence for an earlier key's speech just before the hook raised it and registering it after. The design below replaces it.

State, in the speech manager's shared part, plain atomics in `verbatim-speech`:
- `G`, the speech generation, a counter.
- `F`, the key fence, the highest key number that has cancelled speech (today's `cancelled_through`).

Every utterance `u` records `g(u)`, the value of `G` read when it was admitted. That one number is the only thing anything cuts by.

- A key's cancel, on the hook thread: raise `F` to the key's number (an atomic maximum), then increment `G`, both sequentially consistent and in that order; then signal the synth thread's cancellation event and the audio thread's wake event; then unpause (section 8.8); then post `SpeechCancelled` to the reducer's port. It reads no pointer, takes no lock and waits for nothing.
- Every other cancel (an `Interrupt` utterance, speech arriving while paused, `SpeechManager::cancel`, a synthesizer switch, shutdown) does the same under the speech lock, before admitting anything of its own, so its own utterance is admitted under the new generation.
- Admission, under the speech lock: read `G`, then read `F` (in that order). If `u` was caused by key `j` and `j` is below `F`, it is fenced off: its queued and cancelled records are pushed (section 8.6) and it goes no further, as today. Otherwise it enters its lane with `g(u)` set to the value read.
- Handing on, under the speech lock (the synth thread when a job ends, or the reducer when the synth thread is idle): every lane entry with `g(u)` below the current `G` is ended cancelled; the first one that is not becomes the job. Its mixer registration and its job carry its own `g(u)`, never a value read afresh.
- The job stops when `G` has passed `g(u)`. It reads `G` itself, at each chunk and in each wait (the cancellation event wakes it), so no thread ever has to find the in-flight job's token. A job also has a stop flag of its own, set only under the speech lock, for the one cut that is not by generation: focus expiry (section 8.7).
- The mixer reads `G` at the start of every pass, at every registration and at every append. A source with `g(u)` below it is dropped and ended cancelled; a registration or an append that arrives with `g(u)` already below it is ended at once and never mixed. When a dropped source had audio in the device, the pass calls `Stop` and `Reset` (decided 2026-10-08) with the mixer lock released.
- The lanes are emptied promptly, not lazily: the reducer, when it handles the `SpeechCancelled` the hook posted after the raise, takes the speech lock and ends every lane entry below `G`; the synth thread does the same when the cancellation event wakes it. Whichever comes first does it; the other finds nothing. So every waiting utterance's cancelled ending is reported within one wake of the key, as today the queue thread reports it on handling the cancel.

The invariant: no sample of an utterance `u` is mixed in any audio-thread pass that begins after `G` has been raised above `g(u)`; and an utterance admitted after a cancel's raise has `g(u)` at least the raised value, so that cancel never cuts it. With the fence: an utterance caused by key `j` is never heard once a key after `j` has cancelled, because it was either admitted before the raise (and is cut by generation) or after it (and is fenced off, since `F` is raised before `G` and admission reads `G` before `F`).

Both interleavings are thereby ruled out. An utterance queued before the key and popped by the synth thread at any moment carries its admission generation, which is below the raised `G`, so the mixer drops it at its next pass and the job stops at its next check, however the pop and the raise interleave. An earlier key's utterance that passed the fence just before the raise was admitted under the old generation and is cut the same way. The bound is one audio-thread pass: a pass that read `G` before the raise may hand one period of audio to the device, which the `Reset` of the next pass, woken by the hook's signal, discards. Today's blocking `cancel_all` has the same one-pass bound.

The test: a deterministic unit test in `verbatim-speech` with the capture synthesizer and a scripted mixer, in which the admitting side (read `G`, read `F`, enter the lane, pop, register, first append) and the cancelling side (raise `F`, raise `G`, signal) each stop at a gate between steps, and the test runs every interleaving of the two: for each, every utterance admitted under the older generation produces no audio at the sink after the raise and ends cancelled, every utterance admitted after it plays in full, and every utterance fenced off ends cancelled with its queued record before its ending. End to end: the Control flood scenarios (`conhost_control_flood`, `windows_terminal_control_flood`), which assert that from the first utterance cut off every later one is, and the typing scenarios that type over speech.

NVDA cancels on its main thread, behind queued work (`docs/nvda/input.md`, "What a key press does to speech"). Verbatim's cancel stays ahead of queued work, as now, and the fence keeps the effect NVDA's order has: speech an earlier key caused is never heard after a later key's cancel.

### 8.3 The mixer lock (finding 11)

The audio thread computes what to mix and copies it under the lock, then releases it for every device call. A cancel or an append arriving meanwhile waits for the copy, not for the driver. The underrun warning and the latency line leave the audio thread for the reporting thread.

### 8.4 The router (finding 10)

Review and navigation gestures go from the hook to the reducer's port, removing one wake-up and a synchronous info-level log write per key. The router keeps lock keys (with their 30 ms delay), the menu gesture, GUI commands and the time. The ordering the router keeps today, input-handled marks and idle barriers behind a lock key whose announcement is still to come (`main.rs:1677-1717`), stays with the router for exactly those gestures; a review key bound to the reducer carries its own handled mark, which is exactly that mark and not a second one.

The router's own speech is fenced by key. Today the router is serial, so the time (Verbatim+F12) followed quickly by a review key is spoken as the time, then the review cutting it off. Once review keys skip the router, the review key could reach the reducer before the router wakes, and the time, spoken at `Interrupt`, would then cut off the later key's review. NVDA runs every gesture's script in key order (`nvda/source/inputCore.py:595-608`, `nvda/source/scriptHandler.py:247`), so the review is heard. So the router speaks the time and date, and the tray and taskbar list, with `speak_for_key` and the key number of the press that asked for it, as the reducer already speaks a key's speech: a later key's cancel fences it off, and the later key's result is heard, as in NVDA. The lock keys' announcements keep today's behaviour unchanged: their 30 ms delay is deliberate, and today a review key within 30 ms of Caps Lock already comes before the lock key's announcement. Test: the time key followed at once by a review key, with the router held, speaks the review and ends the time cancelled.

### 8.5 Audio device behaviour, kept as its own step

Not communication, so not designed here; placed in the thread model so the steps do not conflict.

- Keeping the device awake (finding 1, decided 2026-10-08, `docs/roadmap.md:52-58`): the audio thread writes silence one engine period at a time for the awake period, 30 seconds by default. In this thread model that is more wakes of the audio thread only, at its device period, with nothing else woken; new speech never waits behind more than one period of silence. NVDA: `audioAwakeTime` and `wasSilence_playFor` (`nvda/source/nvwave.py:252-258`, `364-366`).
- The engine period (finding 2): measured first, from the ledger's `IAudioClock` stage (section 10); `IAudioClient3`'s shorter shared period tried only with the stream kept awake. D15 records that the development machine's device offers none shorter than 10 ms.
- The earcon re-mix (finding 6, decided 2026-10-08): the audio thread re-mixes audio already handed to the device. It is the audio thread's work, under the mixer lock, with device calls outside it, as above.

### 8.6 Speech records: every speech event, as it happens, in one ordered channel

What works today, and what the suite depends on: the queue thread broadcasts each utterance's text when it is queued (`LatencyLedger::utterance_queued`, `crates/verbatim-app/src/latency.rs`), including utterances that are then cut before playing and those fenced off (queued and at once cancelled); pause, resume and sounds when applied; start and end as the audio thread reaches them. The Control flood scenarios take every line up to line 100 as queued and then assert the cut-offs; the Shift flood scenarios assert lines 2 and 3 queued while line 1 plays (`crates/verbatim-e2e/src/scenarios/terminal_key_timing.rs`); `expect_queued`, `take_until` and `expect_nothing_more` read queued frames throughout (`crates/verbatim-e2e/src/speech.rs`). The first version of this design would have broadcast once per announcement after its first sample, so utterances cut before playing would never have been broadcast and the rest late. That is withdrawn.

The design:
- Every speech event becomes a record pushed, at the moment it happens, onto one lock-free multi-producer channel to the reporting thread: queued (with its text and trace), fenced off, started, sound played, paused, resumed, ended (completed, cancelled or failed), and the idle barrier's record (section 6.7).
- Who pushes: the thread that makes the event happen. The reducer pushes queued and fenced-off records at admission, under the speech lock; whichever thread empties a lane (section 8.2) pushes the cancelled endings of its entries; the synth thread pushes a failed ending; the audio thread pushes started, sound, paused and resumed, and every ending of a registered utterance.
- Order: the channel's order is the order of the pushes. An utterance's queued record is pushed under the speech lock before it can be registered with the mixer, so it always comes before that utterance's started or ended record; a barrier record pushed by the reducer comes after every queued record it pushed before it.
- The reporting thread broadcasts each record as today's code does (`broadcast_speech`, `broadcast_speech_started`, `broadcast_sound`, `broadcast_speech_paused`, `broadcast_speech_ended`), in channel order, and is the only thread that takes the control plane's registry lock. `verbatim-inspect`'s speech subscription sees what it sees today.
- Cost on the path: a push and, when the reporting thread is waiting, one wake signal, a system call of microseconds; no lock and no allocation beyond the record. The text is copied only while a connection is subscribed to speech, as today.

Test: an utterance cut before any audio still produces its queued record and then its cancelled ending, both before the next barrier's answer; a fenced-off utterance produces queued then cancelled; records of one utterance never arrive out of order. End to end: the Control and Shift flood scenarios in step 10's pass set.

### 8.7 Index marks, `SpeechCancelled`, say-all, and the handed-on list

- Today: marks reach the reducer's channel straight from the audio thread (`crates/verbatim-app/src/speech_events.rs`, `mark_reached`), and the hook sends `SpeechCancelled` to the same channel right after its cancel (`main.rs`, the hook's speech-effect callback). Say-all moves the caret or the review position on each mark (`crates/verbatim-core/src/say_all.rs`); `SpeechCancelled` stops say-all, and later marks are then ignored (`crates/verbatim-core/src/reduce.rs`).
- After: the audio thread posts each mark to the reducer's port at the moment the mixer passes the mark's sample, as today it sends on the reducer's channel; the hook posts `SpeechCancelled` right after raising `G`. Neither is moved to the reporting thread and neither goes through another thread, so nothing is delayed: a mark played before the key reaches the reducer before `SpeechCancelled`, and a mark posted after it is ignored by say-all's existing rule, exactly as today. NVDA likewise handles index callbacks on its main thread in order with a key's cancel and ignores one that comes after (`nvda/source/speech/manager.py:590-595`, `676-684`).
- The handed-on list (`handed_on`), which focus expiry judges (`drop_expired`: when any utterance handed on no longer holds for the new focus, everything handed on is stopped), loses today's maintainer, the queue thread's `Ended` message. Its new maintainer: the audio thread pushes each ended utterance's id onto a lock-free ended list as it ends it; every holder of the speech lock drains that list first, before anything else it does under the lock, and removes those ids from `handed_on`. So an utterance that has ended is never judged again. Focus expiry runs on the reducer under the speech lock: it sets the in-flight job's stop flag and posts the mixer a cut of exactly those utterance ids, then clears `handed_on`, as `stop_handed_on` does today; under the lock no admission can interleave with it.
- Tests: a focus change after a focus utterance has ended does not stop speech handed on later; marks reached before a cancel reach the reducer before `SpeechCancelled`. End to end: `notepad_say_all` and `text_box_say_all`.

### 8.8 Pause and resume

- Today `TogglePause`, the unpause on a cancel or an `Interrupt`, and the paused flag are all serialised on the queue thread, which applies each to the mixer and reports it.
- After, the state is one atomic word: a paused bit and a count of transitions. The hook toggles it (Shift alone) with a compare-and-swap that flips the bit and counts; a cancel or the reducer's admission while paused unpauses it with a compare-and-swap that clears the bit and counts only if it was set. Whoever changes it signals the audio thread's wake event; the request carries no value.
- The audio thread, on every wake, reads the word, applies the bit to the mixer, and pushes one paused or resumed record for each transition counted since the last it reported. Transitions strictly alternate, so their order is known from the count. The mixer's state, the flag `SpeechManager::paused` reads, and what the control plane reports therefore never disagree, and a Shift press is never lost to a race with an unpause.
- Test: a toggle and an unpause from two threads, interleaved at every step, leave the mixer's state equal to the bit after the audio thread's next pass, with exactly one record per transition, alternating. End to end: `conhost_shift_flood` and `windows_terminal_shift_flood`.

### 8.9 Settings, synthesizer switches and shutdown

- A setting change followed by speech of its new value (the settings dialog) is ordered today by the queue's channel. After, a setting and a job are both handed to the synth thread under the speech lock, in the order they were given, so the speech is synthesized with the new setting.
- A synthesizer switch cancels and then switches (`SwitchSynth` today): it raises `G` under the speech lock and hands the switch to the synth thread behind the cut, so no audio of the old synthesizer is mixed after the switch's announcement is admitted.
- Shutdown: `SpeechManager`'s `Drop` raises `G`, ends the lanes under the lock, signals the synth thread's cancellation event so a hosted job's wait ends, then stops and joins the synth thread, as the queue thread's `Shutdown` does today.

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

After, 10:
1. UIA's callback thread in the listener, which writes inline.
2. verbatim.exe's relay thread, which writes inline.
3. The outpost's I/O thread.
4. The outpost's worker, which writes inline.
5. verbatim.exe's relay thread again, which reads the outpost's frame and posts it.
6. The reducer, which queues the speech inline and cancels without waiting.
7. The synth thread, which writes `Speak`.
8. The host's thread.
9. The synth thread again, with the first PCM.
10. The audio thread.

Under stage A step 6 the listener's reading thread adds one: 11. Under decision 3's alternative, the reducer reading the outpost's pipe itself, step 5 goes: 9 (10 under step 6). The same holds for every count below that includes an outpost's message.

NVDA, about three: UIA's callback thread queues the event and requests an immediate pump (`nvda/source/core.py:1180-1199`); the main thread handles it and speaks; eSpeak NG's background thread synthesizes and feeds the WASAPI player from its callback (`_espeak.py:200-216`). The extra seven are the price of D9, D13 and D18's isolation, one process boundary each way plus the host, and of keeping today's arrival order (decision 3).

### 9.2 An MSAA focus, the outpost's own hook in

Today, 12 (13 with the watchdog): the outpost's event thread, the worker, the outbound thread, verbatim.exe's reader, the reducer, the queue thread, the synth thread, the host's reader, the host's main thread, verbatim.exe's host reader, the synth thread, the audio thread.

After, 8: the event thread, the worker, the relay thread, the reducer, the synth thread, the host's thread, the synth thread, the audio thread.

NVDA, about three, but every WinEvent other than focus waits for a 10 ms delayed pump (`nvda/source/core.py:55`, `1013`; `IAccessibleHandler/internalWinEventHandler.py:193`).

### 9.3 A typed character's echo

Today, 9 (11 when it interrupts speech, since the cancel's blocking request to the audio thread is serial): the hook thread, the reducer, the queue thread, the synth thread, the host's reader, the host's main thread, verbatim.exe's host reader, the synth thread, the audio thread. If the interrupted utterance is still being synthesized, finding 4's round trip adds about ten more before the echo's `Speak` is sent.

After, 6: the hook thread, the reducer, the synth thread, the host's thread, the synth thread, the audio thread. The cancel runs in parallel (the hook wakes the synth thread and the audio thread directly), and the echo never waits for the cancelled utterance.

NVDA, about two: the main thread (it hears the character from inside the application through its helper) and eSpeak NG's thread.

### 9.4 A caret key (UIA, the application moves the caret)

Today, 17: the hook thread, the reducer, verbatim.exe's writer for the outpost, the outpost's main thread, the worker (first check), the UIA callback with the caret event, the worker again, the outbound thread, verbatim.exe's reader, the reducer, the queue thread, the synth thread, the host's reader, the host's main thread, verbatim.exe's host reader, the synth thread, the audio thread. (The audit's "about 16" did not count the UIA callback.)

After, 12: the hook thread, the reducer (writes the watch inline), the outpost's I/O thread, the worker, the UIA callback, the worker again (writes inline), the relay thread, the reducer, the synth thread, the host's thread, the synth thread, the audio thread.

NVDA: the main thread runs the script, then polls for the caret every 10 ms, sleeping between checks (`nvda/source/editableText.py:70-162`, the sleep at `160`), then eSpeak NG's thread; about three wake-ups plus up to one 10 ms poll, rounded up to the timer tick.

### 9.5 A review key

A review key that reads the application (a review line through UIA), today 16: the hook thread, the router (with an info log write), the reducer, verbatim.exe's writer, the outpost's main thread, the worker, the outbound thread, verbatim.exe's reader, the reducer, the queue thread, the synth thread, the host's reader, the host's main thread, verbatim.exe's host reader, the synth thread, the audio thread.

After, 10: the hook thread, the reducer, the outpost's I/O thread, the worker, the relay thread, the reducer, the synth thread, the host's thread, the synth thread, the audio thread.

A review key answered from the reducer's own state: today 10, after 6.

NVDA: the input thread hands the key to the main thread, which reads (in-process for its own buffers, cross-process for the application) and speaks; eSpeak NG's thread; about three.

### 9.6 What the counts are worth

Estimated, not measured: at the relay's measured median of 0.06 ms per hand-off in release, removing seven hand-offs from a UIA focus saves about 0.4 ms at the median. The larger effect is on the tail. If each hand-off independently had a 0.5 percent chance of exceeding 2 ms (the middle of the measured 0.24 to 0.99 percent), a 17-hop path would exceed 2 ms somewhere about 8 percent of the time and a 10-hop path about 5 percent (a 9-hop path, decision 3's alternative, about 4.4 percent), before the priorities of section 6.1, which are aimed at the per-hop chance itself. Hops are not independent on a loaded machine; the ledger measures the real figures.

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
- Outpost to relay (`OutpostToRelay`): the worker's `WriteFile` to the relay thread's read completion (cross-process).
- Reducer wait (`ReducerWait`): the relay's post of the frame to the reducer starting on it (in-process), which is mostly the reducer being busy, labelled so it is never mistaken for transport.
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
- The stamps ride in the speech records of section 8.6, which the reporting thread takes from its one channel; the ledger adds no hand-off of its own, and the push that carries the first sample's stamp is after the measured span. The reporting thread assembles the timeline (today's `Stages::breakdown`, `latency.rs:104-249`), writes the latency line, serves `verbatim-inspect latency` and broadcasts to the control plane.
- Clocks: if skew between processes ever shows as a negative cross-process stage, the shared clock can move to the performance counter scaled to microseconds, as `relay.md` 4.5 allows; the branch kept the precise file time for the reason in section 3.3.

### 10.3 The measurement scenario the audit asks for first

Today no recorded latency line has measured the synthesizer host or a real device: the suite uses the in-process capture synthesizer and the silent device (`latency-audit.md` section 1). The new scenario, `measure_speech_real`, runs Verbatim with eSpeak NG through its host and the real WASAPI device. It drives one fixed target, the Windows Forms text box the suite already uses for edit controls, through a fixed sequence: focus changes into and out of it, typed characters, caret keys, a review key, and a key pressed during a long announcement (the finding 4 case). It records every stage of 10.1 for each announcement, and reports distributions per stage (count, median, 99th and 99.9th percentiles, maximum, share over 1 and 2 ms), not a pass or fail at first; D15's budgets become assertions once the figures are known. It runs locally, where a real device exists (the dev VM runner-direct, and once on bare metal), not in hosted CI, which has no sound device. A OneCore variant measures the cancel stages of 7.3.

## 11. Migration

Paused after step 3 (Dickson, 2026-10-13). Step 3's policy is reverted
(1651ab0): above-normal priority for the computing threads made typed
echoes slower, and the gain from raising the message-moving threads was
measured while the mixer's writer and audio thread woke each other
thousands of times a second (fixed, 6d5836f). Before resuming, every
thread's wake-ups are audited and each source fixed at its root, and this
design is revised to judge every step on wake-ups and CPU as well as on
latency (`docs/roadmap.md`, stage A step 9). The audit's fixes are in
(`docs/design/wake-ups.md`) and the revision is section 18; Dickson
decided 14 to 17 the same day and the migration resumes with step 3 as
decision 14 redefines it.

Each step is small, builds and passes `cargo xtask ci`, and is verified before the next. Measurements come first. Live checks follow the targeted-run rule; full suite runs are named where a step changes how every outpost talks to Core.

### Step 1. Make the paused branch's measurement parts build, and record the baseline

Also measured in this step: the JSON parse and re-serialize cost of decision 1, and, before step 4, the evaluation of `mio::windows::NamedPipe` in place of the hand-written overlapped module (Dickson, 2026-10-11), including whether a write that completes at once leaves the pipe free for the next write without the I/O thread waking.

Evaluated 2026-10-11 against mio 1.2.4 (79d1417): mio never sets skip-completion-on-success on pipes, so after any write the pipe stays busy until its poll thread processes the completion, and a second frame from the same producer waits for that wake-up; reads go through a 4 KiB internal buffer and are copied again; a second writer is refused, not queued; and its named-pipe code had a use-after-free fixed on 2026-09-28. Decided with Dickson: keep the hand-written module and grow it into the transport; mio's race tests (`tests/win_named_pipe.rs`) are models for step 4's tests. The floor benchmark gains the measurements the evaluation lists (back-to-back frames, write latency during a flood, several producers on one pipe, 1, 2 and 32 MB frames, allocations per frame, a child draining a 2 MB write before exit).

- Builds from the branch: f94e761 (one write per message), 43adba8 (the relay stages), and the `overlapped` module and the scheduling module as libraries only, split out of db16548 and 7e6e562, with no behaviour change.
- Extends the ledger with the return-path and speech stages of 10.1 that today's architecture can stamp (synth request, host first chunk, host return, device clock).
- Removes: nothing. Adds the measurement.
- Tests: the ledger's unit tests for each new stage; the host protocol's round-trip test with the timing block.
- Pass: CI green; one run of the measure set in debug and release produces every stage. These figures are the baseline every later step is compared with.

Done on 2026-10-13, on main at 670ccff, one run of the measure set in each build on this development machine, about 2,240 announcements each, the suite's silent audio device (so no device clock stage, which waits for step 2's real device). Per stage, as count, median, 99.9th percentile and maximum in milliseconds, debug then release:

- verbatim.exe (relay): 438, 0.1, 4.9, 4.9; 442, 0.0, 3.0, 3.0. Over 2 ms: 1.37 and 0.68 percent.
- verbatim.exe to outpost: 438, 0.1, 1.4, 1.4; 442, 0.0, 0.9, 0.9.
- Listener to verbatim.exe: only 12 samples in each, median about 17 to 22 ms, which are facts that waited for an outpost to start (step 8's case); with the four "waiting for the outpost to start" samples of 33 to 58 ms.
- Outpost queue: 1,977, 0.1, 17.2, 324.4; 1,984, 0.1, 15.6, 283.5.
- Outpost read: 1,977, 1.8, 29.5, 35.6; 1,984, 1.1, 25.8, 35.8.
- Outpost to verbatim.exe: 1,977, 0.2, 2.3, 2.7; 1,984, 0.1, 1.7, 9.1.
- Waiting for the reducer: 1,977, 0.1, 4.1, 4.7; 1,984, 0.0, 6.4, 8.8.
- Hook to Core: 410, 0.1, 4.1, 4.1; 414, 0.0, 5.8, 5.8. Over 2 ms: 3.66 and 6.52 percent.
- Reducer to verbatim.exe's relay: 416, 0.0, 2.9, 2.9; 420, 0.0, 0.1, 0.1.
- To speech: 2,235, 0.1, 60.7, 106.8; 2,242, 0.0, 61.9, 155.9.
- Synthesizer request: 0.0, 1.5, 2.0; 0.0, 1.2, 2.8. To the synthesizer host: 0.0, 0.2, 0.3; 0.0, 0.1, 0.5. The host's first chunk: 0.1, 0.9, 1.3; 0.1, 0.8, 1.1. Back from the synthesizer host: 0.1, 4.3, 6.2; 0.0, 6.6, 10.5, over 2 ms 1.43 and 2.23 percent.
- Mixer and device: 2,235, 0.4, 78.5, 79.6; 2,242, 0.1, 79.4, 94.5.

The figures come from the ledger's latency lines, which round to 0.1 ms, through a scratch script; the latency line counts only announcements, so a stage on a path that speaks nothing (the Pause keys of `measure_keys_text_box`) has no figure.

### Step 2. The platform floor and the speech-path scenario

- The relay floor benchmark (5cbcb4e) as `relay.md` section 6 specifies, F0 to F4, plus F5: sender and receiver both overlapped with inline writes (this design's child side), and F6: a host-shaped round trip (command, a fixed CPU burst standing in for a first chunk, paced reply).
- `measure_speech_real` (10.3), on the dev VM and once on bare metal.
- ETW attribution (WPR, CPU usage, precise) for one run of each condition.
- Removes: nothing.
- Pass: the floors and their attribution are recorded; the priority levels of 6.1 are chosen from them; the baseline of the speech path through the real host and device exists.

### Step 3. The scheduling policy

Built and reverted (1651ab0); replaced by decision 14's measurement on the quiet baseline (section 18.3). As first written:

- 7e6e562 and cfbd323 completed: every process opts out of power throttling; every thread in section 6 gets its level, including the keyboard hook (adding `verbatim-process` to `verbatim-input-windows`), the host reader threads while they still exist, and the host's threads.
- Removes: the non-silent part of the per-hop tail, if the inference holds.
- Tests: the scheduling module's tests (the branch's, reading the priority back).
- Pass: the measure set and `measure_speech_real`, compared with step 1: every stage's share over 2 ms no worse, and the per-hop shares lower; the flood scenarios (`conhost_leave_flood`, the terminal flood) pass with the priorities in place and no thread runs hot (CPU per thread from the ETW trace); and the starvation check of section 6.1: `rapid_tabbing_in_settings`, `settings_dialog_keys` and `menu_and_settings_dialog`, ten runs each while a terminal flood runs in another window, on the two-processor dev VM in a debug build, with the GUI thread's longest ready wait under 100 ms in the ETW trace.

Done on 2026-10-13 as decision 14 redefines it: three configurations, each run through the measure set in debug and release and traced for 8 seconds of keys in the text box, one after another on this development machine with the same build. A: no priorities. B: `THREAD_PRIORITY_HIGHEST` for the threads that only move messages. C: B, with every process and those threads opted out of power throttling.

- verbatim.exe's own stage: over 2 ms in 0.9 and 0.7 percent of A's samples (debug, release), maximum 2.6 and 4.2 ms; none over 1 ms in B, maximum 0.4 and 0.2 ms; C the same as B. The outpost's hop to verbatim.exe likewise: A's maxima 12.5 and 4.0 ms, B's 2.1 and 0.3 ms.
- The wait for the reducer, at normal priority, grew to take what the raised threads saved: over 1 ms in 1.8 percent of A's release samples and 16.4 percent of B's. Summed per announcement over verbatim.exe's stage, both hops and that wait, B's 99.9th percentile was 4.9 and 4.7 ms against A's 10.9 and 5.7 ms; over 2 ms, 4.9 and 1.5 percent against 7.8 and 1.7. The key's path to the speech queue showed no difference beyond the runs' variation, which two runs of A put at a third of the share over 2 ms.
- Wake-ups and processor time per key event: within the variation of two runs of A (20.8 to 24.3 wake-ups, 1.9 to 2.3 ms); priorities do not change how often anything wakes.
- C gained nothing over B, and was worse in debug (11.5 percent of the summed path over 2 ms).

Adopted: B. The threads that only move messages run at `THREAD_PRIORITY_HIGHEST`: the keyboard hook thread (`verbatim-input-windows` now depends on `verbatim-process`); verbatim.exe's supervisor owner thread, its listener reader and per-outpost reader threads, and its writer thread for each child; the listener's outbound writer and its pipe-reading main thread; each outpost's outbound writer and its pipe-reading main thread; the `WinEvent` threads of outposts and the listener; the synthesizer host's command reader; and Core's reader of each host's replies. Every other thread keeps normal priority, the audio thread its MMCSS registration, and no process or thread is opted out of power throttling. The remaining tail is the wait for the reducer, which step 7 changes; the starvation check is moot, since nothing that computes is raised.

### Step 4. The transport: frames, pipes, senders

- `verbatim_process::transport`: the frame format of 4.2, the pipes of 4.1 (overlapped on both ends, 64 KiB for event pipes), the `Sender` of 4.3 with backlog policies supplied by the caller, the inbound frame reader, the port and mailbox from db16548.
- Every Core-child pipe moves to it, with today's threads still in place: the same threads, the new framing, inline writes where the producer can (the per-child writer threads still exist in this step but find nothing to do in the common case).
- Removes: line framing, the second write and the split read, `CreatePipe`.
- Tests: the frame reader on a frame split across reads, several frames in one read, a frame at the limit and one past it; the sender's overload policies (`writer.rs` tests, kept); a write that pends is finished by the I/O side and keeps order; a sender whose reader stops delays no other pipe; a child's end opened by a lower-integrity process is refused; a launch whose own open of the child's end fails (the pipe taken first) is aborted with the server handle closed and no retry; no write is ever cancelled (a PCM frame larger than the 4 KiB buffer, pended, is completed whole while the host's command event is set); the pipe's end in the middle of a frame is reported as the pipe's end, never as a message.
- Pass: CI; the supervisor's mockapp scenarios; then the full suite, because every outpost's pipes change, with `outposts_shut_down_cleanly` reporting nothing.

Its first part, done on 2026-10-13:

- `verbatim_process::transport`: frames as decision 1 settled them, a `Content-Length` header and a JSON body, with the length written in ten digits so a frame is encoded in place in the slot it is written from; `read_frame`, which reads the header and then the whole body at once, and refuses a length over 32 MB before reading any of it; `BlockingReader`, for a thread that blocks on an overlapped end; and `Sender`, which writes inline when nothing is ahead and otherwise keeps the caller's backlog, finished by a thread of its own that wakes only for a write that pended. The port and the `PipeIo` slot are the `overlapped` module's.
- Every message between verbatim.exe and the outposts and the listener is a frame (`protocol.rs`'s `write_message`, `encode_message`, `put_message` and `read_message`), so neither end's logic changed.
- verbatim.exe's ends of those pipes are overlapped, made with no name by `overlapped::pipe` with 64 KiB buffers; each child's commands go through a `Sender` with today's queue as its backlog, so the owner and the reducer write inline and the writer thread, now the finishing thread, wakes only when a write pends. The reader threads read through `BlockingReader`.
- Two departures from the step as written. The children's ends stay synchronous, as their threads read and write them today: nothing in a child benefits from an overlapped end until step 6 gives it inline writes, which is when they change. The synthesizer host's pipes keep their own framing (`hosting.rs`, a kind byte and a length, raw samples for PCM) and `launch` until step 9: its PCM is binary and paced by its 4 KiB buffer, and step 9 rewrites that protocol anyway. And the sender encodes under its lock, straight into the slot it writes from, rather than outside it, which saves a copy of every frame and keeps the lock to an encode and one `WriteFile`.

What it left, finished before step 5: the test that a sender whose reader stops delays no other pipe, two senders sharing one port that nobody serves (`transport.rs`). Three of the step's tests no longer apply as written: the pipes have no name (decision 11 withdrawn), so no lower-integrity process can open a child's end and verbatim.exe makes no open that another process could take first; and the test that no write is ever cancelled, a pended PCM frame completed whole while the host's command event is set, belongs to the host's pipes, which move in step 9. The mailbox of db16548 is the relay's, so it comes with step 5. The full suite for step 4 is left to Dickson's next batched run.

### Step 5. The relay on its port, routing on the header

- db16548's relay thread, with the listener's pipe read in frames and routed by header (no parse, no re-serialization), held facts merged by key bytes, stamps patched at the write; c9564c6's writes from the pushing thread. The listener's reader and the per-child writer threads go.
- Removes: two wake-ups in verbatim.exe on every relayed fact, and its parse and allocations.
- Tests: `relay.md` 7.5's (held facts before later facts, the hand-over order test `a_hand_over_message_gets_through_a_full_queue_behind_the_facts_before_it`, a stuck child delays no other), kept from the branch's `relay.rs` tests; a header-routing test that a hand-over marker and a fact for the same element have different keys and are never merged, nor the marker moved behind later facts; the menu-end message, sent from the threadpool timer through the same sender, stays behind the facts before it.
- Pass: `relay.md` section 7 items 2 to 4 as approved: the "verbatim.exe" stage at most 1 ms maximum, 99.9th percentile at most 0.2 ms in release and 0.5 ms in debug; listener to relay plus relay at the 99.9th percentile at most 1 ms and at most 2 ms maximum with every exception attributed by ETW; requests' relay stage at most 1 ms. The focus and terminal scenarios, then the full suite.

### Step 6. The children's side: inline writes and one I/O thread

- The listener and every outpost use the transport's child side: producers write inline; one I/O thread on a port reads commands and finishes pended writes. The listener's outbound thread, the outpost's outbound thread and the outpost's separate pipe-reading main thread become that one I/O thread. The menu-end grace becomes a threadpool timer.
- Removes: one wake-up on every listener fact and on every outpost event and reply (finding 8).
- Tests: the outbound queue's merge and ordering tests against the new sender; message positions for `NodesHeld` counted in write order; pongs written by the I/O thread while the worker is stuck in a call; an outpost's last frame before `Close` arrives whole, its shutdown having waited for the write in flight (section 4.6); `Flushed` reported only after the earlier writes completed.
- Pass: listener stage and "outpost to relay" stage each at least as good as step 5's figures, with the share over 2 ms lower than the baseline's 5.3 percent; the focus, MSAA hand-over and terminal scenarios; the full suite, with `outposts_shut_down_cleanly` reporting nothing.

### Step 6a. The idle barrier through every child (stage A step 10)

- Section 6.7: `IdleQuery` and `IdleAnswer` in the listener's and the outposts' protocols; the relay's round; the reducer's round with the zero-timeout drain and today's conditions; the reply still through `after_queued` until step 10. Done before step 7, because step 7 changes who reads the outposts' pipes and so what today's channel length counted.
- Removes: the channel-length count; the barrier's blind spot for outposts' and the listener's work in flight.
- Tests: section 6.7's three mockapp tests (a frame in an outpost's pipe and another in its backlog while the reducer is held; a caret watch pending; a listener read in progress); an outpost ending during a round counts as answered; an outpost starting during a round makes the next round run.
- Pass: the full suite, since every scenario ends with the barrier; then ten runs each of the scenarios that read speech up to the barrier during a flood (`conhost_control_flood`, `windows_terminal_control_flood`, `conhost_shift_flood`, `windows_terminal_shift_flood`) and of the caret scenarios (`text_box_editing`, `notepad_editing`, `text_box_word_selection`).

### Step 7. The relay reads the outposts' pipes; the reducer waits on its port

- The relay thread reads every outpost's pipe on its port, handles pongs, `Ready`, `TargetExited` and the pipe's end itself, and posts every other frame whole to the reducer's port (decision 3 as recommended). The reducer waits on its own port: those frames, and one posted packet per key, `SpeechCancelled`, mark, GUI event, control-plane command and supervisor notice. Large frames are parsed by a helper, keeping each outpost's order. verbatim.exe's per-outpost reader threads go. (Under decision 3's alternative the reducer reads the pipes itself and section 6.9's rules are built here instead.)
- Removes: a thread per outpost in verbatim.exe, and the channel between the hook and the reducer.
- Tests: the reducer's arrival order across an outpost's frame and a key: a frame written before a key, while the reducer is held, is reduced before the key, as today, and one written after it after; a large frame from one outpost does not delay a key or another outpost's event, and that outpost's next frame comes after it; a closed pipe reaches the supervisor after every frame before it; the reducer held for longer than three ping intervals ends no outpost and is reported as its own stall (section 6.8); while the reducer is held, two notifications from one outpost are both delivered and the outpost's merge rules do not act (the relay keeps draining).
- Pass: "outpost to relay" and "reducer wait" stages reported separately; the whole terminal flood family (every `conhost_` and `windows_terminal_` scenario whose name ends in `flood`, with `conhost_scrollback_overflow`, `windows_terminal_scrollback_overflow`, `conhost_footer_overflow` and both key-timing families), in a debug build, which is the slowed reducer, with bounded latency and no hang; `rapid_tabbing_in_settings`, `text_box_editing`, `notepad_editing`, `text_box_word_selection` and `notepad_word_selection`, ten runs each, for arrival order under load; `outpost_crash_recovery`; the full suite.

### Step 8. Review keys straight to the reducer

- Finding 10, as 8.4. The router keeps lock keys, the menu and the time, and speaks the time, the date and the tray list with `speak_for_key`; the gesture log line moves to debug, after the forward.
- Removes: one wake-up and an info log write per review and navigation key.
- Tests: a review key's handled mark is reported after its speech is queued, and only once; a lock key's announcement still precedes the marks behind it; the time key followed at once by a review key, with the router held, speaks the review and ends the time cancelled.
- Pass: the review and navigation scenarios (`text_box_review_cursor`, `notepad_review_cursor`, `text_box_review_words`, `notepad_review_words`, `conhost_review_output`, `windows_terminal_review_output`, `object_navigation_in_settings`, `object_navigation_over_uia`, `navigation`); the "hook to reducer" stage for review keys at most that of caret keys.

### Step 9. The host protocol: tags, Speak behind Cancel, signalled cancel, one host thread

The signalled cancel is done (3ebb7be), ahead of the rest, by the audit: `SynthSink::on_cancel` registers a waker the cancel calls; the hosted driver waits on its reader's channel and a one-slot cancel channel together, the host's command reader calls the waker of the driver speaking, and OneCore waits on one channel carrying its completion or its cancel. The waits move onto the pipe's read event and the command event when the reader threads go in this step.

- Section 7: frames tagged with their utterance; the hosted driver returns on cancel and discards the old utterance's frames; the cancellation token with a waker in `verbatim-speech`; the synth thread reads the host pipe itself; the host is one thread on its command event; OneCore waits on its completion and the command event.
- Removes: finding 4's round trip (about ten wake-ups), both 10 ms polls, Core's host reader thread and the host's reader thread.
- Tests: a scripted host driver that blocks per chunk on a gate: a cancel during an utterance followed by a new one sends `Speak` for the new one before `Done` for the old one is read, and no audio of the old one reaches the sink after the cancel; a cancel before any audio stops a OneCore-shaped driver whose operation never completes, with no timer involved (the test waits for the `Done`, with only an overall failure bound); a host killed mid-utterance fails it and the next utterance starts a new host, as today; a host cancelled while a PCM frame larger than its pipe's buffer is pended delivers the next utterance whole with no restart; a setting applied right after a cancel is answered without the cancelled utterance's frames being taken for its reply.
- Pass: `synth_host_crash_recovery`; the Control flood scenarios (`conhost_control_flood`, `windows_terminal_control_flood`), since the host's cancel changes; in `measure_speech_real`, a key pressed during a long announcement has its echo's "synth request" to first sample no longer than an echo pressed in silence plus one chunk's synthesis time; the OneCore variant's cancel-to-stop has no 10 ms steps.

### Step 10. The speech manager without its queue thread, and the audio thread unblocked

- 8.1 to 8.3 and 8.6 to 8.9: queue logic under the speech lock on the speaking thread; the cancel by generation and key fence, lock-free on the hook; the mixer cutting by generation; lanes emptied on `SpeechCancelled` or the synth thread's wake; the speech-record channel and the reporting thread, which also answers the idle barrier; marks posted to the reducer's port by the audio thread; the ended list that maintains `handed_on`; the pause word; the mixer lock released across WASAPI calls; the ledger's line and underrun warning on the reporting thread; the ledger's mutex replaced by carried stamps.
- Removes: the queue thread's wake on every announcement, the cancel's two serial wake-ups, the shared ledger and registry locks on the path.
- Tests: `verbatim-speech`'s pipeline tests with the capture synthesizer, unchanged in what they assert: ordering of lanes, fenced-off speech from an earlier key arriving after a later key's cancel ends cancelled, focus validity, pause; the interleaving test of section 8.2 (every interleaving of admission and cancel: nothing admitted under the older generation is heard, everything admitted after is heard in full); section 8.6's record tests (an utterance cut before audio still yields queued then cancelled, before the next barrier's answer); section 8.7's tests (a focus change after a focus utterance ended stops nothing; marks before a cancel reach the reducer before `SpeechCancelled`); section 8.8's pause test; settings and speech kept in order; a synthesizer switch mixes no old audio after its announcement; the mixer's tests with the lock released around a device stub that records calls.
- Pass: the Control and Shift flood scenarios (`conhost_control_flood`, `windows_terminal_control_flood`, `conhost_shift_flood`, `windows_terminal_shift_flood`), ten runs each; the say-all scenarios (`notepad_say_all`, `text_box_say_all`); the typing and editing scenarios that type over speech (`text_box_typed_words`, `notepad_typed_words`, `conhost_typing`, `windows_terminal_typing`, `text_box_editing`, `notepad_editing`); `settings_dialog_keys` for setting changes spoken at once; `synth_host_crash_recovery`; `measure_speech_real`'s "to speech" and "to the audio thread" stages; the full suite.

### Step 11. The outpost's per-event costs

Done in part (fac2f04): the watchdog waits on an alarm, a waitable timer and an event, woken only for deadlines; held nodes are sent every 32 messages rather than with the next request or when the reducer goes idle. The description formatting and the logging inside the intake lock remain.

- Finding 12: the watchdog's deadline on a waitable timer (no wake per entry); the entry's description formatted only when debug logging is on; logging moved out of the intake lock; `NodesHeld` coalesced with the next request or sent when the reducer goes idle (evidence, not a timer).
- Removes: a watchdog wake per entry; a string format per entry; a file write under a lock in test runs; three wake-ups after most focus changes.
- Tests: the watchdog still abandons a worker past its deadline (mockapp's held-call test); held nodes are still released (the held-objects tests).
- Pass: the outpost queue and read stages no worse; the operation ledger's counts unchanged (`crates/mockapp/tests/call_counts.rs`).

### Step 12. Measure again, and the audio step

- The measure set in debug and release, `measure_speech_real` on the dev VM and once on bare metal, each compared with step 1's baseline; the full suite; ten runs of the related scenarios.
- Then, separately, the audio step: the awake period (finding 1), the engine period trial (finding 2), the earcon re-mix (finding 6), each measured with the device stages of 10.1.

Ordering against stage A (`docs/roadmap.md:197-211`): this replaces stage A step 9 (the relay) in the order Dickson set, after step 6's revisions, and its step 6a is stage A step 10, pulled forward (decision 12); steps 1 to 3 can run whenever the desktop is free, since they measure and change scheduling only.

## 12. What carries over

### 12.1 From `relay.md`

Unchanged:
- Section 4.1's core: the supervisor's owner thread becomes the relay, on a completion port; it reads the listener's pipe and routes in the same pass; at most one write in flight per child; a stuck child holds only its own queue; the ordering argument resting on one routing thread; the alternatives rejected in 4.2 (a separate relay thread, direct pipes, MMCSS for the relay).
- Section 4.3: one write per message.
- Section 4.4: priority and power throttling under Balanced, no timer resolution change, with the list of threads widened by section 6.1 here.
- Section 6: the floor benchmark, with two variants added.
- Section 7's pass criteria and section 9's two approved decisions.

Changed:
- "Outposts' messages to Core keep their reader threads" (4.1): replaced; the relay thread reads them on its port and posts each frame to the reducer's port (step 7), which keeps the reader threads' order and continuous draining with one thread instead of one per outpost.
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

Approved: decisions 2 to 12 as recommended (Dickson, 2026-10-13), decision 1 as revised with him and decision 13 (2026-10-11). Decisions 14, 15 and 17 approved as recommended and decision 16 declined, keeping the pings as they are (Dickson, 2026-10-13).

1. Measured on 2026-10-13 (section 4.2): about 15 microseconds to parse and re-serialize a relayed focus read, so plain JSON with a text length header is kept. As first proposed: keep JSON bodies, and route on a binary frame header so verbatim.exe never parses or re-serializes a fact. NVDA: no IPC, so no equivalent. Revised with Dickson (2026-10-11): measure first, in step 1, the release-build cost of parsing and re-serializing a focus fact with its ancestors, a typical reply, and a 1 MB terminal frame; if the typical messages cost a few tens of microseconds at most, keep plain JSON with a readable text length header (as the Language Server Protocol's `Content-Length: N`, a blank line, then the JSON), so the pipes stay readable by eye and a reader still asks for a whole message in one read; stamps are then JSON fields set just before encoding. The binary header comes back only if the measurement shows the cost matters.
2. Children's pipe ends become overlapped too, and every producer writes its own message when nothing is ahead of it, which removes the listener's and the outposts' writer threads. Recommend: adopt.
3. Who reads the outposts' pipes. Two choices.
   - The relay thread reads them, handles pongs, `Ready`, `TargetExited` and the pipe's end itself, and posts every other frame whole to the reducer's completion port, whose order is the reducer's one order of arrival (sections 4.5, 6.2). It keeps today's order of outposts' events against keys and against each other, keeps today's continuous draining so no merge rule starts acting because the reducer is busy, and keeps pongs independent of the reducer. NVDA: the same order for UIA events, which its flusher queues as they come; for MSAA NVDA lets keys overtake unflushed events, which Verbatim does not do today and would not do.
   - The reducer reads them itself (section 6.9). One wake-up fewer on every outpost message (a UIA focus 9 instead of 10). But under load keys overtake outpost frames not yet read, UIA ones included, which departs from today and from NVDA's UIA behaviour; outposts' merge rules start acting routinely (decision 8); pongs pass through the reducer and need section 6.9's rule; and a pipe drains at the reducer's pace.
   - Recommend: the relay reads, since it regresses nothing and departs from NVDA nowhere; the one wake-up it costs is a hop at `HIGHEST` priority of microseconds. Revisit only if the ledger shows the relay's hop for outposts' frames to be a material share of the tail. In both, frames over 256 KiB are parsed by a helper while keeping each outpost's order.
4. The scheduling policy of 6.1: `HIGHEST` for threads that only move messages (the hook, the relay, the children's I/O threads, the WinEvent threads), `ABOVE_NORMAL` for threads that compute on the path (the reducer, the synth thread, the host, the outposts' workers, the listener's reading threads), MMCSS for audio, UIA's own threads left alone, every process out of power throttling. It keeps the 2026-10-08 levels (the hook high, speech and synthesis above normal) and extends them by rule. NVDA: sets none. Recommend: adopt, with the levels confirmed by the floor benchmark and by step 3's starvation check (section 6.1), with the computing roles dropping to normal priority if Verbatim's own GUI thread waits too long under a flood on the two-processor VM.
5. The speech queue thread goes; the queue's logic runs under a lock on the thread that speaks; the hook's cancel is lock-free, no longer waits for the audio thread, and cuts by a generation each utterance records when it is admitted, with the key fence checked at admission (section 8.2); every speech event is broadcast as it happens through one ordered channel (8.6); marks and `SpeechCancelled` reach the reducer as today (8.7); pause is one atomic word applied and reported by the audio thread (8.8). NVDA: speech is managed on the main thread, and a key's cancel runs there behind earlier work; the fence gives the same audible result without making the cancel wait. Recommend: adopt.
6. The host protocol of section 7: frames tagged by utterance, the next `Speak` sent right behind a `Cancel`, cancellation signalled by the command's arrival, one host thread. NVDA: the same effect, its eSpeak NG callback aborting at the next chunk and the next speech queued behind it. Recommend: adopt.
7. Review and navigation keys go from the hook to the reducer, and the gesture log moves to debug; the router speaks the time, the date and the tray list fenced by their key, so a later key's result is never cut off by them (section 8.4). NVDA: every gesture's script in key order, which the fence reproduces. Recommend: adopt.
8. Whether outposts' merge rules may act because the reducer is behind. The first version of this design said yes, calling it NVDA's behaviour because "NVDA's limiters act on its one queue". That is right for MSAA only. NVDA's MSAA limiter is flushed only when its main thread pumps, so while the main thread is busy it keeps one event per object and kind, at most 10 events per thread and 4 focus events per cycle, and drops the rest (`nvda/source/IAccessibleHandler/orderedWinEventLimiter.py`); Verbatim's intake already applies those rules per worker batch (stage A step 4). For UIA, NVDA's rate limiter coalesces by element and event, property or notification only while its own flusher thread lags (`nvda/nvdaHelper/local/UIAEventLimiter/rateLimitedEventHandler.cpp`), and the flusher then drains into an event queue that never merges, however busy the main thread is; so under main-thread load NVDA handles, and may speak, every queued UIA value change, selection change and alert.
   - With decision 3 as recommended, the question does not arise: the relay drains every pipe continuously, outbound merges act as rarely as today, and nothing changes.
   - Under decision 3's alternative there are two readings. Keep today's draining for UIA-derived events: the outbound backlog never merges an event that came from UIA (it remembers each event's API), so nothing NVDA would speak is merged away; MSAA-derived events merge by node and kind, as NVDA's limiter does while its main thread is behind; terminal output keeps today's combining, guarded by a unit test that combining two outputs speaks exactly what Core's handling of the two in sequence speaks; memory for a UIA burst then grows in the outpost as it grows in Core today. Or merge everything as first proposed: under load a slider or progress bar is heard at its latest value instead of each one NVDA would speak, two different alerts from one node become one, a run of selection changes becomes its last, and terminal floods are combined routinely; it trades speech NVDA would give for currency, a deliberate departure from NVDA.
   - Recommend: decision 3 as recommended, which makes this moot; if the alternative is chosen, keep today's draining for UIA-derived events.
9. The budget reading approved for `relay.md` applies to every verbatim.exe stage of section 10, including the reducer's request writes and the speech stages inside verbatim.exe: each within 1 to 2 ms of its own work, each hop no worse than the platform floor, every exception attributed by trace, measured once on bare metal. Recommend: adopt.
10. `measure_speech_real` reports distributions first and becomes a gate (D15's budgets) only once its figures are known; it runs locally and on bare metal, not in hosted CI. Recommend: adopt.
11. Amend `architecture.md` sections 1 and 10: internal channels are named pipes, single-instance, connected before the child starts, admitting only the current user at Verbatim's integrity level, instead of anonymous pipes with "no named endpoint". Recommend: adopt; it is what overlapped I/O requires, and the relay's approval already accepted it for the listener and outposts.
12. The harness's idle barrier becomes an in-band round through the listener and every outpost (section 6.7), which is stage A step 10, pulled forward from after the terminal redesign into this migration as step 6a, because step 7 removes what today's barrier counts. NVDA: no equivalent. Recommend: adopt; it also closes the gap step 10 was written for.
13. The messages exchanged with the outposts and the listener (`SupervisorToOutpost`, `OutpostToSupervisor` and the listener's facts, today in `verbatim-outpost`'s protocol module, a Windows crate) move into a platform-neutral crate as plain serde types, with the frame's kinds and header fields; each platform supplies its own transport, threads and supervisor, so a future Linux or macOS port shares the messages and the code that turns them into Core's inputs. The module has no Windows dependency today (it uses only serde, `std::io` and `verbatim-model`), so the move is mechanical. Not a migration step yet: it is best done once the protocol settles after step 7. NVDA: no equivalent. Recommend: adopt in principle, scheduled after this migration. Approved by Dickson on 2026-10-11.
14. The scheduling policy, after the audit (section 18.3). Withdraw `ABOVE_NORMAL` for the computing threads, which made typed echoes slower. Measure again, on the quiet baseline, `HIGHEST` for the threads that only move messages and the process-wide opt-out from power throttling, and adopt each only where it shortens a measured tail without raising processor time; keep MMCSS for audio. NVDA sets none of these. Recommend: adopt.
15. Decision 3 again, with wake-ups counted. The relay reading the outposts' pipes costs one wake-up per outpost message over the reducer reading them itself: 216 in 8 seconds of keys, 3,536 in 6 seconds of a console flood, each a few microseconds at `HIGHEST`. The alternative's costs are unchanged (section 6.9): keys overtaking UIA events under load, which NVDA does not do, routine merging, and pongs through the reducer. Recommend: keep the relay reading, since the saving is a small share of the flood's 22,064 wake-ups and the departure from NVDA is audible; the terminal redesign, which owns the flood's real cost, comes back to it if a flood still sends thousands of messages a second.
16. The liveness pings (section 6.8). Every child is pinged every 3 seconds, which is all idle Verbatim wakes for (7 wake-ups a second across all processes with three children). A wedge only matters when an outpost is asked something, and asking already starts its worker's deadline. Proposal: ping only a child with work outstanding (a request, an intake not empty, or a starting outpost), and the listener, whose silence would otherwise go unnoticed, at a longer interval. NVDA has no child processes; its watchdog checks the main thread's progress only while it is busy. Recommend: adopt, as a step after step 7, when the relay owns the pings. Declined (Dickson, 2026-10-13): every child stays pinged every 3 seconds.
17. verbatim.exe's hidden top-level frame (`wake-ups.md` item 9) receives every broadcast message from every application, about 67 a second while typing elsewhere. Whether the tray icon and its menus can be owned by a message-only window, which receives no broadcasts, is a question for the GUI's design, outside this migration. Recommend: look into it as its own item.

## 14. Risks

- One relay thread carries every child's writes and every child's reads. A bug that blocks it stops all relaying and all input from outposts; one that blocks the reducer stops all reduction. The rule that neither ever blocks, logs on the delivered path or does file I/O is enforced in review, and each gets a watchdog as `focus-pipeline.md` 6.4 asks for threads others wait on.
- A stalled reducer cannot make a healthy outpost look dead, since pongs are read by the relay (section 6.8); a reducer stall is a fault of its own that its watchdog reports. Under decision 3's alternative this rests on section 6.9's rule instead.
- On a small machine under a flood, raised threads could starve Verbatim's own GUI thread and the foreground application; step 3 measures it and section 6.1 gives the fallback.
- The cancel's correctness now rests on atomics rather than one channel's order. Section 8.2 states the invariant and the interleaving test that proves it; a change to admission, hand-on or the mixer that reads `G` at a different point must keep that test passing.
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

- The audio thread's idle wait also bounded how long a request to reopen a failed device went unnoticed. Done (451523d): the default-device callback already signalled the wait, and the device's own wait now ends at its retry time, so the idle mixer waits without a limit. The retry after a failed open stays a fixed second: an open that failed while a device exists has no event to wait for.
- PCM is copied about ten times between eSpeak NG and the device (`latency-audit.md` finding 12); this design removes two copies. The rest cost microseconds each.
- The control plane keeps its own overlapped pipe code (`crates/verbatim-control/src/server.rs:13-34`); it could later move onto the transport.
- The keyboard hook drops a gesture when the router's 64-slot channel is full (`crates/verbatim-input-windows/src/lib.rs:469-470`). Posting review keys to the reducer's port removes that for them; lock keys and GUI gestures keep the bounded channel.

## 17. Regressions considered

Every finding of the 2026-10-11 review of this design against today's behaviour and NVDA's, each checked against the code it cites before it was acted on. For each: what it found, how the design now resolves it, and what guards it.

### Changes to what is heard or what the suite sees

- Speech frames on the control plane (high). Found: the first version broadcast speech once per announcement after its first sample, so utterances cut before playing would never have been broadcast, and the suite's `expect_queued`, `take_until` and `expect_nothing_more`, with the Control and Shift flood scenarios, would have broken. Resolved: every speech event is pushed at the moment it happens into one ordered channel and broadcast as today (section 8.6). Guarded by: section 8.6's record tests; the Control and Shift flood scenarios in step 6a's and step 10's pass sets.
- The idle barrier (high). Found: a completion port has no length; frames in pipes and outposts' backlogs were never counted; the queue thread that ran the reply goes. Resolved: an in-band round through the listener and every outpost, answered through the speech-record channel, which is stage A step 10 pulled forward as step 6a (section 6.7, decision 12). Guarded by: section 6.7's mockapp tests; the full suite in step 6a, since every scenario ends with the barrier.
- The lock-free cancel letting earlier speech through (high). Found: two interleavings, the synth thread popping speech queued before the key and the reducer passing the fence just before the raise, let an utterance register after the mixer's watermark and be heard in full. Resolved: cancellation by a generation recorded on each utterance at admission, with the key fence read after it; the mixer and the job cut by generation only; the lanes emptied promptly (section 8.2). Guarded by: the interleaving unit test of section 8.2; `conhost_control_flood`, `windows_terminal_control_flood`, and the typing and editing scenarios in step 10.
- Wedge verdicts while the reducer is stalled (medium). Found: pongs routed through the reducer would make every outpost look wedged after a 9 s reducer stall, and repeated kills would reach the crash limit. Resolved: the relay reads the outposts' pipes and their pongs, and the outposts' I/O threads answer pings (section 6.8); under decision 3's alternative, no wedge verdict until the reducer has waited on an empty port (section 6.9). Guarded by: the mockapp test holding the reducer past three ping intervals (step 7); the pong-while-worker-stuck test (step 6).
- Frames cut mid-write (medium). Found: cancelling the host's paced PCM write on `Cancel`, or a child exiting with a write pending, would leave a partial frame. Resolved: a started frame is never cancelled; a child drains its backlog and its write in flight before exiting; `Flushed` means completed; a cut frame is the pipe's end, never a message (sections 4.2, 4.3, 4.6, 7.2). Guarded by: step 4's and step 9's tests; `outposts_shut_down_cleanly` in every full suite run.
- Say-all position after a cancel (medium). Found: the first version did not list marks or `SpeechCancelled` among the reducer's inputs and said the audio thread did nothing but report first samples. Resolved: the audio thread posts marks and the hook posts `SpeechCancelled` straight to the reducer's port, in order, with no added hop; the audio thread's reporting role is stated (sections 6.2, 8.7). Guarded by: section 8.7's tests; `notepad_say_all` and `text_box_say_all` in step 10.
- Focus expiry stopping valid speech (medium). Found: no one maintained `handed_on` once the queue thread went. Resolved: the audio thread's ended list, drained by every holder of the speech lock before anything else (section 8.7). Guarded by: the test that a focus change after a focus utterance ended stops nothing (step 10).
- Arrival order under load (low to medium). Found: with the reducer reading the pipes, keys would overtake outpost frames not yet read. Resolved: the relay reads the pipes and posts frames as they arrive, keeping today's order (sections 4.5, 6.2, decision 3); the alternative is set out as a decision with this cost stated. Guarded by: step 7's ordering test with the reducer held; `rapid_tabbing_in_settings` and the editing and word-selection scenarios in step 7.
- The time key against a later review key (low). Found: once review keys skip the router, the time could cut off a later key's review, where NVDA keeps key order. Resolved: the router's own speech is fenced by its key (section 8.4). Guarded by: step 8's test, and the review scenarios in step 8.
- Shift toggles desynchronised (low). Found: a toggle from the hook and an unpause from the reducer, each posting a value, could leave the mixer and the flag disagreeing. Resolved: one atomic pause word with a transition count, applied and reported only by the audio thread (section 8.8). Guarded by: section 8.8's test; `conhost_shift_flood` and `windows_terminal_shift_flood` in step 10.
- Starvation under raised priorities (low to medium). Found, by reasoning: on a small machine in debug, raised threads could starve Verbatim's own GUI thread past the outpost's 400 ms step deadline. Resolved: step 3's starvation check and its fallback (section 6.1). Guarded by: `rapid_tabbing_in_settings`, `settings_dialog_keys` and `menu_and_settings_dialog` during a flood in step 3, with the ETW ready-wait measure.

### Decision 8 and merging

- The NVDA comparison (low, documentation). Found: "NVDA's limiters act on its one queue" holds for MSAA, not for UIA, whose flusher drains into a queue that never merges. Resolved: decision 8 is rewritten with the correct comparison, and becomes moot under decision 3 as recommended.
- Merging that could drop what NVDA speaks (low). Found: under reducer backpressure, value and selection runs and two alerts from one node would merge. Resolved: no new merging with the relay draining; under the alternative, UIA-derived events are recommended never to merge. Guarded by: step 7's test that two notifications are both delivered and merge rules do not act while the reducer is held.
- Terminal combining exercised for real (low). Found: routine merging would test `terminal::combine`'s equivalence claim under floods. Resolved: combining stays as rare as today; under the alternative a unit test of the equivalence is required. Guarded by: the whole terminal flood family in a debug build in step 7.

### Speech ordering details

- Settings against speech (low). Resolved: settings and jobs handed to the synth thread under the speech lock in the order given; the hosted driver discards a cancelled utterance's frames while waiting for a setting's reply (sections 7.2, 8.9). Guarded by: step 9's and step 10's tests, `settings_dialog_keys`.
- Synthesizer switch (low). Resolved: the switch raises the generation and goes behind the cut (section 8.9). Guarded by: step 10's switch test.
- Shutdown order (low). Resolved: `Drop` raises the generation, ends the lanes, wakes a hosted wait, then stops the synth thread (section 8.9). Guarded by: the existing shutdown tests and `outposts_shut_down_cleanly`.
- The mixer lock on the hook thread (low). Found: posting the mixer cancel took the mixer's mutex. Resolved: the hook only raises atomics and signals events (sections 6.2, 8.2).
- `Speak` right behind `Cancel` (no change). The effect matches NVDA's eSpeak NG driver; nothing is spoken twice or partly where NVDA would speak it (section 7.2).

### Transport, routing and recovery

- Large frames and head-of-line blocking. Resolved: one `ReadFile` for a frame's remainder, as corrected on main in 899a484; today's head-of-line behaviour kept; the backlog in front of the reducer stays in verbatim.exe as today, and the reducer takes it at its own pace (section 4.8). Guarded by: step 2's completion counts and step 7's large-frame test.
- Large frames parsed by a helper (no change). They overtake nothing of their own outpost, as today's reader threads.
- Review keys' order against caret keys (positive). Both are now posted by the hook in key order, as NVDA queues every gesture.
- Handled marks and barriers behind lock keys (no change). They stay on the router; a review key carries exactly its own handled mark (section 8.4).
- The MSAA hand-over, held facts and the marker (low). Resolved: the relay alone reads the listener and writes to outposts; `Ready` now reaches the owner in one hop as today, since the relay reads it; markers and facts have distinct header keys; the menu-end message goes through the same sender. Guarded by: step 5's header-routing test and `a_hand_over_message_gets_through_a_full_queue_behind_the_facts_before_it`.
- Crash recovery (low). Resolved: an outpost's pipe end still follows every frame before it, since the relay reads the pipe in order; a host that dies while a cancelled utterance drains is noticed at the next `Speak`, as today. Guarded by: `outpost_crash_recovery` in step 7, `synth_host_crash_recovery` in steps 9 and 10.
- The hook at `HIGHEST` (no change audible).
- Logging and the flight recorder (none). The flight recorder stays on the reducer; the gesture log's move to debug affects only `verbatim.log`; the latency line is still written when its audio starts.
- Security of named pipes (low). Resolved: any failure of verbatim.exe's own open of a child's end aborts the launch with no retry (section 4.1). Guarded by: step 4's test.

## 18. Wake-ups and CPU (revision of 2026-10-13)

Dickson, pausing the migration after step 3: "We don't just want lowest latency at any cost, we also want efficiency as well, ideally as few wakeups as possible." This section makes wake-ups and processor time criteria of every remaining step, alongside latency, records the baseline after the wake-up audit (`docs/design/wake-ups.md`), and says what each step is worth by these measures. Decisions 14 to 17 in section 13 come from it.

### 18.1 How every step is judged

- Each step reports, besides the latency stages of section 10, each thread's wake-ups (times readied from a wait) and its time on a processor, for the workloads of 18.2, from `cargo xtask trace` read by `verbatim-trace-waits`, which now prints both per thread. `--wakers`, `--stacks` and `--samples` say who woke a thread, from where, and where its processor time went.
- Counts are compared per unit of work where the unit is countable (per announcement, per key, per outpost message), since a fixed trace window catches more or less of a scenario: the synth thread's count in the same 8 seconds of `measure_speech_real` was 931, 1,357 and 1,637 in three runs.
- A step passes only if no workload's wake-ups or processor time rise beyond that variation, unless the rise is the step's stated purpose and Dickson has agreed to it; a step that removes a thread shows its wake-ups gone and no other thread's grown by more than it saved.
- Threads that exist only for the test harness (the WAV recorder, the control plane's speech writers and readers) are reported and left out of the comparison: they are about 9 percent of the key workload's wake-ups and cost a user nothing.
- Traces are short and deleted once read, as `docs/tooling.md` requires. A trace that overlaps a scenario's start or end can leave File Explorer putting the focus on the Start button (`wake-ups.md`, seen three times), after which Explorer is restarted before the next run.

### 18.2 The baseline after the audit's fixes

Main at f4d097e, debug build, this development machine, one trace each; wake-ups and processor time summed over all of Verbatim's processes in the window.

- Idle (Verbatim started directly, 8 seconds from 12 seconds in): 56 wake-ups and 3 ms in all, 7 a second across every process. The audit removed the mixer's one-second idle poll (451523d) and the outpost watchdog's per-entry wake (fac2f04); what is left is the 3-second pings and their pongs (decision 16).
- Keys in the Windows Forms text box (`measure_keys_text_box`, 8 seconds from 15 seconds in): 10,672 wake-ups and 1.03 s of processor time. The text box's outpost worker 3,325 (167 ms); UIA's unnamed threads and the outpost's event thread about 2,430; the audio thread 1,016; the harness's threads about 980; the synth thread 516 (261 ms); the speech queue 412; the hook 399; the reducer 331; the host reader and the host's two threads 577 together; the outpost's outbound thread and verbatim.exe's reader for it 417 together.
- Focus moves in File Explorer (`measure_focus_explorer`, 8 seconds from 10 seconds in): 33,243 wake-ups and 2.1 s. The outposts' event threads 10,364 (395 ms), UIA's unnamed threads 9,287 (435 ms), UIA's callback threads running Verbatim's handlers 4,920 (166 ms) and the workers 4,188 (279 ms): 86 percent of the wake-ups are the outposts taking in File Explorer's events (`wake-ups.md` items 7 and 8). The listener's reader and writer, the per-outpost writers and the children's outbound threads, which steps 5 and 6 remove, 471 together.
- A console flood (`conhost_flood`, 6 seconds from 8 seconds in): 22,064 wake-ups and 7.3 s, more than one processor. The console's outpost's UIA callback threads used 4.3 s in 382 wakes; their CPU samples have Verbatim's handler on the stack in 41 percent, the rest being UI Automation's own client-side work (`uiautomationcore.dll`, the heap, `memcpy`, COM). The worker 5,491 wakes and 1.6 s, a fifth of its samples in the file system, which is debug logging in test runs. The outpost's outbound thread, verbatim.exe's reader for it and the reducer woke 3,536 times each, once per message.
- Real speech (`measure_speech_real`, 8 seconds from 20 seconds in): 10,218 wake-ups and 0.93 s. The audio thread 1,709, the synth thread 507 (220 ms), the speech queue 359, the host reader and the host's two threads 431, with 43 ms in the host.

Two sources that the communication design does not touch dominate the counts:

- The worker wakes once for each reply to a cross-process call it makes. Its readying stacks while typing come from the application through `win32k` (about 1,140 in the ten most frequent stacks), from `ctfmon.exe`, the Text Services Framework's server (634), and from File Explorer (111). Fewer calls per caret read and per event, through remote operations as the standing rule requires, is what lowers it. Why `ctfmon.exe` answers a windowless multithreaded worker is not yet known and needs symbols.
- The outposts' event intake: every `WinEvent` hook and UIA subscription wakes its thread for every event the application raises before Verbatim's filter runs. Narrowing that is the in-process filtering of D2's injection helper (`wake-ups.md` item 7).

### 18.3 What each remaining step is worth by these measures

Counts are from 18.2's key workload unless named; "removes" is the measured count of the threads a step takes away, before anything it adds.

- Step 3, the scheduling policy: priorities change who runs when, not how often anything wakes. Above-normal priority for the computing threads made typed echoes slower and is withdrawn. Opting a process out of power throttling keeps its threads on fast cores at full clocks, which costs energy for every unit of work. Decision 14 proposes measuring again, on this quiet baseline, `HIGHEST` for the threads that only move messages and the power-throttling opt-out, each adopted only where it shortens a measured tail without raising processor time.
- Step 4, the transport: no thread goes, so no wake-up count should change; its framing must not raise processor time.
- Step 5, the relay on its port: removes the listener's reader and writer threads and the per-outpost writers: 67 here, 280 in File Explorer, 121 in the flood.
- Step 6, the children's inline writes: removes the outposts' and the listener's outbound threads: 204 here, 191 in File Explorer, 3,538 in the flood (16 percent of it), 181 in real speech.
- Step 7, the relay reading the outposts' pipes: replaces the per-outpost reader threads with the relay thread, one wake per frame either way, so it saves threads, not wake-ups. Decision 3's alternative, the reducer reading the pipes, would save the reader's wake on every outpost message: 216 here, 3,536 in the flood. Decision 15 puts the question again with these figures.
- Step 8, review keys straight to the reducer: up to the router's 120 here, of which lock keys, the menu and the time stay.
- Step 9, the host protocol: the signalled cancel is done (3ebb7be), so its share is the host reader in verbatim.exe and the host's command thread, 455 here and 325 in real speech, and finding 4's round trip on every interrupting key.
- Step 10, the speech manager without its queue thread: 412 here, 359 in real speech, one wake on every announcement.
- Step 11, the outpost's per-event costs: done in part (fac2f04).

Together steps 5 to 10 remove about 1,250 of the key workload's 10,672 wake-ups (12 percent) and about 110 ms of its 1.03 s, every one of them on the path; in the flood, 3,897 of 22,064 (18 percent), and with decision 3's alternative 7,433. They are worth doing for latency and for these savings, and the larger sources of 18.2 are not theirs.

### 18.4 Efficiency work outside the migration

Ranked by the measured counts; each is its own item, measured the same way.

- The outposts' event intake (`wake-ups.md` items 7 and 8): 86 percent of the File Explorer workload. Needs D2's injection helper to filter inside the application; until then, each hooked kind is kept only if a reading needs it.
- The workers' cross-process calls: the largest single thread in the key and speech workloads. Remote operations for every call sequence, audited (the standing rule), and the `ctfmon.exe` question.
- Terminal floods: the console outpost uses more than a processor, most of it UI Automation's client-side work for text-changed events. The terminal redesign (`docs/roadmap.md`) is judged on this as well as on latency.
- The liveness pings: the only wake-ups idle Verbatim makes itself; kept as they are (decision 16, declined).
- verbatim.exe's hidden top-level frame takes every broadcast message (`wake-ups.md` item 9), about 67 a second while typing elsewhere; whether the tray icon and menus can live on a message-only window is decision 17.

## Findings from other Rust projects (2026-10-11)

A survey of 21 Rust projects with multi-process communication (scratch report `rust-ipc-survey.md`, each cited by commit and file) changes four points of this design, proposed for Dickson's decision:

- Unnamed overlapped pipes, as Rust's standard library makes its child pipes (`library/std/src/sys/process/windows/child_pipe.rs` 32 to 190): `NtCreateNamedPipeFile` on `\Device\NamedPipe\` with an empty name, and the other end opened with `NtOpenFile` relative to the first handle. Both ends can be overlapped and no name exists at any moment, so `architecture.md`'s "no named endpoint" stays and decision 11 is withdrawn, together with section 4.1's brief window and its abort rule. Cost: two ntdll calls through the `windows` crate's driver-kit features. To check first: that the child's end, opened without the synchronous flag, is overlapped. Approved by Dickson, 2026-10-11, if that check holds; if it does not, the named single-instance pipes of section 4.1 and decision 11 stand. The check held (2026-10-13, `verbatim_process::overlapped`'s tests): the other end opened that way reports no synchronous mode, and a read on it pends and completes through a completion port. So the pipes are unnamed, `architecture.md`'s "no named endpoint" stays, and decision 11 is withdrawn. `NtCreateNamedPipeFile` is not in the `windows` crate, so it is declared against ntdll, as the standard library declares it; `NtOpenFile` comes from the crate's driver-kit features.
- verbatim.exe flushes its own ends before closing them. Microsoft's `DisconnectNamedPipe` remarks say unread data is discarded when the server end disconnects, and `FlushFileBuffers` waits until the client has read everything; the `interprocess` crate flushes before close for this reason (`named_pipe/stream.rs` 32 to 44). Section 4.6's draining rule covers children only; verbatim.exe's last message to a child, such as `Shutdown`, must be read before its end closes. Approved by Dickson, 2026-10-11.
- Child exits and other handles waited on the relay's own port: compio waits on process and event handles through the completion port with `NtAssociateWaitCompletionPacket` (`compio-driver/src/sys/driver/iocp/wait/packet.rs` 18 to 70), so the relay thread can learn of a child's exit without a helper thread. To evaluate in step 5 (approved by Dickson, 2026-10-11).
- Producers writing inline has no precedent among the projects read: each uses a writer thread or an event loop that owns the pipe (Servo's `ipc-channel` relies on the same one-write rule, `src/platform/windows/mod.rs` 1092 to 1124, but writes from its owner). It is kept, since it is what removes the writer's wake-up, but step 2's floor benchmark compares it with the I/O thread owning every write, on back-to-back frames and several producers, and the simpler form is chosen if the saving is not measurable (approved by Dickson, 2026-10-11: direct writing is adopted only if the numbers show it reduces the delays, the worst cases above all; otherwise the I/O thread owns every write). Raised priorities for threads other than audio are also without precedent in what was read, which makes step 3's starvation check essential.

Confirmed as well trodden: length-prefixed frames, 64 KiB buffers, one write per frame, the overlapped buffer lifetime rule (`ipc-channel`'s `aliased_cell.rs` leaks rather than frees memory still in use, as the slot rule does), a single relay thread serving many pipes, binary data beside JSON, ping-and-miss supervision, and MMCSS for audio (Firefox's `audioipc`). For decision 1's text header: write the header and body in one `WriteFile`, and bound the length before allocating (lsp-server allocates whatever length it is given). No crate is adopted for the transport; compio's completion-port driver and `ipc-channel` are references while writing it. No project publishes named-pipe latency on Windows, so the floor benchmark's figures will be our own.
