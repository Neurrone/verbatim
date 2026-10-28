# Communication and threading: one design from an event or a key to sound

Status: built, except the audio step (section 11). Steps 8 and 10 of the migration were not adopted (section 11). The decisions are in section 13; section 18 makes wake-ups and processor time criteria alongside latency.

One model for every pipe between Verbatim's processes and for every thread an event or a key passes through on its way to the first audio sample. The mechanism lives in the crate guides: the transport, pipes, ports and scheduling in `docs/crates/verbatim-process.md`; the relay, the outposts' and the listener's threads, the protocol and the idle round in `docs/crates/verbatim-outpost.md`; the reducer's port, the idle barrier's reducer side and the latency ledger in `docs/crates/verbatim-app.md`; the host protocol and the cancellation token in `docs/crates/verbatim-speech.md`, `docs/crates/verbatim-synth-hosted.md` and `docs/crates/verbatim-synth-host.md`; the mixer and device recovery in `docs/crates/verbatim-audio.md`. This document keeps the problem, the alternatives weighed, the reasons and the limits.

## 1. Summary

- One transport module, `verbatim_process::transport`, for every pipe: overlapped pipes with no name, made by verbatim.exe and inherited by the child; frames of a text length header and a JSON body, with a raw section after the body for a terminal's text and colours and for PCM's samples (section 4.2); one `WriteFile` per frame; at most one write in flight per pipe; and a rule that the thread producing a message writes it itself when nothing is ahead of it, so no writer thread is woken in the common case. The same module serves verbatim.exe and its children.
- One thread model: in each process, one I/O thread per role that waits only on a completion port or on events, a thread for each kind of work that may block (an outpost's worker, a synthesizer), and nothing else on the path. verbatim.exe relays every event (no pipe joins two children). Its relay thread reads the listener's and every outpost's pipe, draining them continuously, and posts each whole outpost frame to the reducer's completion port, so the order of outposts' events against keys is the order of arrival and no outpost's merge rules act because the reducer is busy.
- A synthesizer host protocol whose frames name their utterance, so the next `Speak` follows a `Cancel` at once instead of waiting for the cancelled utterance to end, and whose cancellation is signalled by the arrival of the command itself, never by a timer.
- A latency ledger with a stage for every hop, stamps carried in each message, and a measurement scenario through the real eSpeak NG host and a real WASAPI device, read from `IAudioClock`.
- One scheduling policy (section 6.1): threads that only move messages run at `THREAD_PRIORITY_HIGHEST`; threads that compute run at normal priority; the audio thread keeps MMCSS "Pro Audio"; no process or thread is opted out of power throttling; the power plan stays Balanced and timer resolution is not raised.
- What the user hears, and what the suite sees, did not change: speech frames reach the control plane as each speech event happens; the harness's idle barrier is a round through every child that covers frames in pipes and backlogs (section 6.7); a started frame is never cut (section 4.3).

## 2. Dickson's constraints, and how each is met

- verbatim.exe adds at most 1 to 2 ms: its own stage within 1 to 2 ms, each hop no worse than the platform floor, every exception attributed by trace (decision 9). Met by giving every verbatim.exe hop its own ledger stage (section 10); the figures are in section 15.
- Events keep passing through verbatim.exe. Every listener fact is read by verbatim.exe's relay thread and written by it to the outpost; every outpost message is read by the relay and handed to the reducer. No pipe joins two children.
- The power plan stays Balanced. Only thread priorities are used (section 6.1).
- Process isolation of D9, D13 and D18 stays: the listener, one outpost per application and one host per synthesizer.
- No fixed waits, retries or polling in place of evidence. Every wait is on an I/O completion, an event or a channel. The deliberate waits stay (the listener's 50 ms menu-end grace, the 30 ms lock-key delay, the mixer's 10 ms start threshold), as do deadline timers that detect a hang (the outposts' watchdog, the host's 10 s `HANG_TIMEOUT`), which are not polls.
- Platform-neutral crates stay free of Windows code. The transport, the completion ports, the I/O threads, the relay, the scheduling calls, MMCSS and WASAPI live in Windows crates; `verbatim-speech` gains only plain types (the cancellation token, the host protocol's messages), and `cargo xtask ci`'s dependency check enforces it. Decision 13 moves the outpost and listener message types into a platform-neutral crate.
- Efficiency as well as latency: "We don't just want lowest latency at any cost, we also want efficiency as well, ideally as few wakeups as possible." Every change is judged on each thread's wake-ups and processor time as well as on latency (section 18).

## 3. The problem

### 3.1 Hops and tails

Before this design every pipe had a reader thread and a writer thread at each end, and the speech path had its own hand-offs: a UIA focus change woke 17 threads in four processes before its first sample, an MSAA focus 12, a typed character's echo 9 (11 when it interrupted speech), a caret key 17, and a review key that reads the application 16. Every hand-off had a measured tail: 0.24 to 0.99 percent of the relay's hand-offs over 2 ms, and 5.3 percent over 2 ms on the way from an outpost to the reducer. A thread made ready at normal priority waits for a running normal-priority thread's quantum, about 31 ms on client Windows, which matches tails of 2 to 30 ms; a path of many hops meets that tail on a larger share of announcements.

### 3.2 Waits on the path

The synthesizer driver polled for cancellation every 10 ms, in verbatim.exe and in the OneCore driver, each poll rounded up to the 15.6 ms timer tick. A cancelled hosted utterance was waited for until the host finished its chunk and sent `Done`, so the next utterance's `Speak` waited about ten wake-ups across two processes. Messages were JSON lines written with two `WriteFile` calls each, and parsed and re-serialized at the relay.

### 3.3 What building the first relay found

- A named pipe with a zero buffer makes every write wait for the reader, so a buffer is always asked for.
- Rust's `std::fs::File` aborts on an overlapped handle, so overlapped ends are touched only through the transport's types, never through `File`.
- Shutdown must stay event-driven: joining retirement threads on the relay thread deadlocks, and a hang check run on it holds every message. Retirements report how their child ended, and the hang check runs on a thread of its own.
- Writing from the pushing thread put encoding and `WriteFile` under the child's lock, a brief contention point, which the transport keeps to one encode and one system call.
- Platform-neutral crates can take a thread priority only through a callback.
- The cross-process clock is the performance counter plus one offset to the Unix epoch, which the first Verbatim process takes and hands to every process it launches (`verbatim_process::clock`), so a key's press, an outpost's read and Core's receipt compare on one monotonic clock however the system's clock is set.
- A command too large to send is skipped with a warning; a failed write closes the queue.

## 4. The transport: one module for every pipe

`verbatim_process::transport`, over `verbatim_process::overlapped`, is the only code in Verbatim that touches an `OVERLAPPED`, a pipe handle or a frame's bytes (`docs/crates/verbatim-process.md`).

### 4.1 Pipes

- Every pipe between Verbatim's processes is made by verbatim.exe with no name at any moment, as Rust's standard library makes its child pipes: `NtCreateNamedPipeFile` on the named pipe file system's root with an empty name, the other end opened relative to the first with `NtOpenFile`. One direction per pipe, two pipes per child. The child inherits its ends.
- Why unnamed: a named pipe would need a DACL, a mandatory label, the first-instance flag and a rule for the microseconds between creating it and opening the child's end, in which another process of the same user could connect first. With no name there is no window and no access-control surface, so `architecture.md`'s "no named endpoint" for internal channels still holds (section 12; decision 11 withdrawn).
- Both ends are overlapped, so a child can write without ever blocking (section 4.3). No end, in any process, is wrapped in a `File`.
- Buffer: 64 KiB for the event pipes in both directions (`EVENT_PIPE_BUFFER`), so a focus fact with its ancestors, or a reply with a line of text, completes its write at once. The buffer is not a message size: a reader that has the frame's length asks for the whole rest of the frame in one `ReadFile` into one buffer of that size, and asks again for whatever remains if the pipe driver completes it short. 4 KiB stays for the host's PCM pipe, whose small buffer is the backpressure that paces synthesis. Every pipe's buffer costs kernel memory, so the event pipes' is raised only if pended writes show it matters.
- Completion: on every end served by a completion port, an operation that completes at once is handled where it was issued and never queued to the port (`FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`, `FILE_SKIP_SET_EVENT_ON_HANDLE`). A read the relay thread completes at once on an outpost's pipe is handled in the same pass, and its frame posted to the reducer's port then, so the reducer's port still receives frames in their order of arrival (section 4.5).

### 4.2 Framing

Settled by decision 1's measurement (2026-10-13, release build, this development machine, serde_json): parsing and re-serializing a relayed UIA focus read with 12 ancestors (4,475 bytes) costs about 15 microseconds; encoding and parsing a small terminal event (723 bytes) about 2.4 microseconds; and a 1 MB terminal event about 1.8 to 2.0 milliseconds. The typical messages cost a few tens of microseconds at most, so the frames are JSON behind a readable text length header, as the Language Server Protocol's (`Content-Length: N`, a blank line, then the JSON body), the stamps are JSON fields set just before encoding, and verbatim.exe parses what it routes. Every frame, whatever its size, is parsed on the reducer's thread, as section 6.2 says.

The bulk of the two kinds of message that carry far more data than anything else travels outside the JSON, in a raw section after the body (Dickson, 2026-10-28): a terminal's text and colours, which made a whole-document read cost the reducer 0.5 to 1.6 ms to parse, and a synthesizer host's samples, which JSON would multiply in size. The rules:

- The header gains a second line, `Raw-Length: ` and the raw section's length in ten digits, before the blank line, only when the frame has a raw section; `Content-Length` still gives the JSON body's length. A frame without one is byte for byte what it always was, so every other message is unchanged and readable. The body and the raw section together may take at most 32 MB.
- The JSON body gives the place of each piece in the raw section as a byte range, `[start, length]`, where the content would otherwise be. A frame is still one write: the body is encoded into the write buffer and the raw section appended after it.
- A terminal's message, an outpost's `TerminalSnapshot` event and its `TerminalSnapshot`, `TerminalHistory` and `TerminalFound` replies, always has a raw section, even an empty one. Each text is there as UTF-8, its place in the body `"raw":[start,length]` beside the line lengths (a match's or a history's text as `"text":[start,length]`); a read's colour runs are there as 16-byte records, four little-endian `u32`s (column, length, foreground, background), the body giving `"colours":{"raw":[start,length],"rows":[...]}` with each row's count of runs. The protocol crate converts to and from these wire forms; the model's types describe only the content.
- A PCM frame's body is `{"Pcm":{"utterance":7,"sample_rate":22050,"channels":1,"samples":[0,2646]}}`, its samples the whole raw section as little-endian `i16`s.
- The receiver checks every range against the raw section, validates each text as UTF-8 (`simdutf8`) and copies it into an owned `String`, and requires the ranges to name the raw section's whole length. A range outside it, invalid UTF-8, colour records that do not fill their range, a raw section longer or shorter than the body names, and a raw section on a message that takes none, are protocol errors, as a malformed frame is.

Measured in a release build (2026-10-28): parsing a terminal's whole-document read of 9,031 rows takes 0.08 to 0.12 ms for 0.29 MB of Latin text and 0.09 to 0.18 ms for 0.36 MB of Cyrillic, and an ordinary read with 30 rows of 120 colour runs 0.03 to 0.04 ms (`measure_conversion` in `verbatim-outpost`, mean of 50); as JSON strings, 0.9 MB of Latin text took 0.6 ms, 1.6 MB of Cyrillic 1.6 ms, and 100 rows of 120 colour runs 1.0 ms. Decoding a PCM chunk of 1,323 samples (60 ms at 22,050 Hz) takes, best of 30, 0.2 microseconds, against 0.06 with the binary header it replaced; encoding it, 0.6 microseconds, as before.

The binary header below is kept as the alternative it was, should a later measurement show the cost of the JSON bodies to matter.

The binary alternative: every message on every pipe is one frame:

- a little-endian `u32`: the length of everything after it;
- a `u8` kind and a `u8` of flags;
- a `u16` header length, so the header can grow without breaking a reader;
- the routing fields, used only on the listener's pipe and Core's pipes to outposts: the target process id (`u32`, 0 for none), whether the fact may start an outpost (one bit of the flags), and the fact key as a length and its bytes (the bytes `FactKey` is compared by);
- the timing block, at fixed offsets: the stamps of section 10, each a `u64` of microseconds on the shared clock or a `u32` duration, zero for a point not passed;
- the body: JSON of the message type that lives where it lives (`verbatim-outpost`'s protocol, `verbatim-speech`'s hosting types), or raw little-endian samples for PCM.

The maximum is 32 MB (`MAX_FRAME_BYTES`). A reader that sees a larger length treats the other end as failed, before allocating. The pipe's end in the middle of a frame (the header or the rest cut short) is the pipe's end with a fault logged, never a message: the partial bytes are discarded and the peer is handled as having exited. `verbatim_speech::hosting` keeps only the host protocol's message types and their bodies, plain data in a platform-neutral crate; the frame is the transport's.

### 4.3 Writing: one write per message, one write in flight, the producer writes

Each pipe end that writes has one `Sender`. Its rules, each with its reason:

- One write per frame, so a frame never costs the reader a second wake-up. Several frames may go in one write (`send_all`), each whole with its own header, so the reader wakes once for all of them.
- At most one write is in flight per pipe; later frames wait in the sender's backlog, in order, under that pipe's overflow policy (section 4.4). This is what keeps frames from interleaving.
- The producer writes. A thread that sends when no write is in flight issues the overlapped `WriteFile` itself. If it completes at once, which is the normal case when the reader keeps up, no other thread wakes. If it pends, the slot keeps the buffer and the `OVERLAPPED` until the completion arrives, and the producer returns at once; it never waits.
- The I/O thread finishes. The completion of a write that pended arrives at the process's I/O thread for that pipe (section 6), which writes the next backlog frame, until the backlog is empty.
- A message's frame is encoded just before its write begins, on whichever thread writes it, so a stamp taken then is the write's and frames are encoded in the order they are written.
- A paced send, for the host's PCM only: the producer waits while a write is in flight, on that write's completion together with the host's command event (section 7), so the backpressure that paces synthesis also notices a cancel.
- A started frame is never cancelled. No code in any process calls `CancelIo` or `CancelIoEx` on a write, and no writer abandons one: once a frame's `WriteFile` is issued, the frame is written whole or the pipe has failed. A frame cut part-way would make every later frame on a byte-mode pipe misread, and for the host it would look like a misbehaving host and cost a restart.
- A marker in the backlog, such as the outbound queue's `Flushed`, is reached only once every earlier write has completed, not merely been issued.
- A failed write closes the sender: later sends fail.

So a frame costs one encode, one `WriteFile` and no wake-up on the sending side in the common case; a writer thread wakes only when the reader has fallen behind by a whole pipe buffer.

### 4.4 Overflow policy, per pipe

The policies act only when a write is in flight, which is exactly when merging matters.

- Listener to verbatim.exe: urgent messages (pongs, `Ready`) ahead of facts; facts merged one per element and kind, and for an MSAA event other than a menu opening per thread that raised it, a newer one replacing the waiting one and going to the back (NVDA's limiter rule); the menu-end message after facts.
- verbatim.exe to an outpost or the listener: at most 64 waiting; a fact replaces a waiting fact with the same key; past the bound the oldest fact is dropped; ping, shutdown and hand-over messages always get in.
- An outpost to verbatim.exe: pongs, `Ready` and `TargetExited` first; events merged by object and kind; terminal output combined under the flood policy; replies, notifications and faults never merged and kept in order. Message positions for `NodesHeld` are numbered in the order frames are written.
- verbatim.exe to a host: a `Speak`, a `Cancel` or a setting at a time; nothing accumulates.
- A host to verbatim.exe: paced, as above; nothing is dropped.

Because the relay thread drains every outpost pipe continuously and posts each frame to the reducer's unbounded port, the outposts' merge rules act only when the relay itself falls behind, which is almost never, and a burst waits in front of the reducer. Under decision 3's alternative, the reducer reading the pipes itself, a busy reducer would fill an outpost's pipe and its merge rules would act routinely (decision 8).

### 4.5 Ordering guarantees

- Within one pipe, frames arrive whole and in the order the sender wrote them. The sender's lock orders writers on the same pipe; the backlog is first in, first out apart from the stated merges.
- The listener's messages to one outpost reach it in the listener's order, held facts before later ones, and "forwarding ended" behind every fact before it (`docs/design/focus-pipeline.md`, section 12.4). The relay thread alone reads the listener's pipe and routes.
- An outpost's messages reach the reducer in the outpost's queue order: one pipe, read by one thread, posted in order.
- Across sources, the reducer takes its inputs in one order, the order its completion port received them: one posted packet per outpost frame (posted by the relay thread as soon as it has read the frame whole), per key, per `SpeechCancelled`, per index mark reached, and per GUI command, control-plane command or supervisor notice. That is the single arrival order `docs/design/focus-pipeline.md` section 15, decision 7 asks for; a typed character's echo comes before the output it causes.
- So an outpost frame written before a key is pressed is reduced before that key even while the reducer is busy. NVDA keeps the same order for UIA events, which its flusher thread queues as they come, independent of main-thread load; for MSAA it lets a key's script go ahead of WinEvents its limiter has not yet flushed. Under decision 3's alternative, keys would overtake outpost frames still unread in a pipe while the reducer was busy, UIA ones included; that is the main reason it was not chosen.
- Core's requests to an outpost are written in the order the reducer issued them; relayed facts and requests to the same outpost are ordered by the sender's lock.
- The host's commands are in order on one pipe: a `Cancel` for an utterance is always ahead of the next `Speak`.

### 4.6 The child's side stays simple

A child calls the transport once at startup with its two inherited ends and gets a `Sender` and an I/O side (`ChildIo`). It sends from any thread with one call that never blocks, and gives its I/O thread a function to run for each received frame; `OVERLAPPED`, completions, buffers and the backlog stay inside the module.

A child never exits with a write pending, because process exit cancels outstanding I/O, which would cut the last frame. So a child's shutdown, after its last message, waits on its I/O thread until the backlog is empty and the write in flight has completed, then exits. The wait is for evidence, the completion; it is bounded only by the shutdown limit, after which verbatim.exe ends the child, and a frame cut that way is the pipe's end with a fault logged (section 4.2), never a message.

Tests that run an outpost in the test process get an overlapped pipe from the same module and a finishing thread of the sender's own, so they exercise the real transport.

### 4.7 Every boundary under the transport

- Listener to verbatim.exe: the listener's producing threads (UIA's callback threads, the per-application reading threads, and the `WinEvent` thread) write inline. The menu-end grace is a threadpool timer whose callback sends, a deliberate 50 ms wait.
- verbatim.exe to the listener: `HookInstalled`, `HookRemoved`, pings, idle queries and shutdown, written by the relay thread, read by the listener's I/O thread.
- verbatim.exe to an outpost: relayed facts written inline by the relay thread in the pass that read them; Core's requests written inline by the reducer. A write that pends is finished by the relay thread, which serves every child's outbound pipe on its port.
- An outpost to verbatim.exe: the worker's events and replies written inline by the worker; pongs by the I/O thread; `TargetExited` by the target-watch thread. Read in verbatim.exe by the relay thread, which handles pongs, `Ready`, `TargetExited` and the pipe's end itself and posts every other frame whole to the reducer's port.
- verbatim.exe to a host and back: the synth thread writes commands and reads frames itself, waiting on the read's event and the utterance's cancellation event (section 7).
- The control plane keeps its own overlapped pipe code. It shares no thread with the path; moving it onto the transport is possible and not planned.

### 4.8 Large frames and head-of-line blocking

- A frame of up to 32 MB, such as a megabyte of terminal output, is written with one `WriteFile`, which pends until the reader has taken it, and is read whole; with one write in flight per pipe, frames never interleave.
- The relay thread has one read outstanding per outpost pipe; a large frame's read stays pending while the relay serves every other pipe, so it holds up only the frames behind it on its own pipe. A small frame written after a large one on the same pipe, such as a reply behind a large terminal output, waits until the large one has been read.
- Once a frame is whole the relay posts it to the reducer's port at once, so the pipes drain continuously and the reducer takes frames at its own pace. The backlog therefore lives in verbatim.exe's memory in front of the reducer, unbounded, and latency under a flood grows with it; the "waiting for the reducer" stage of section 10 shows it.
- The reducer parses every frame, whatever its size, as it takes it, so each outpost's order holds with no machinery of its own. A large frame holds the reducer for its parse, and keys and other outposts' frames posted after it wait that long (section 15).
- Under decision 3's alternative, the reducer would re-arm an outpost's read only when it reached that pipe's previous completion, so a pipe would drain at the reducer's pace: a reply behind a large frame would wait longer whenever the reducer was busy, the pipe would fill, and the outpost's backlog would grow and merge (decision 8).
- The host's 4 KiB PCM pipe: a PCM frame larger than the buffer, which a OneCore chunk can be, is taken in several reads. That is correct only because a started frame is never cancelled (section 4.3).

## 5. Serialization

JSON is written and parsed at every pipe, and verbatim.exe's relay parses each fact it routes (decision 1). The alternatives weighed:

- A binary body: it would save an estimated 10 to 30 microseconds per announcement in release, too little to justify a second encoding to maintain, and JSON is readable in logs and captures, with serde letting older peers read newer messages.
- Routing on a binary header without parsing: the relay would decode a header and copy bytes. Decision 1's measurement put the parse and re-serialization of a focus fact at about 15 microseconds in release, a small part of verbatim.exe's 1 to 2 ms budget, so the header stays text and the relay parses.
- Bulk data is the exception: a terminal's text and colours and a host's samples travel in the raw section (section 4.2), because they are most of the bytes and JSON multiplied their size and parse time.

Revisit binary bodies only if the ledger shows encoding or parsing above 5 percent of any stage's 99th percentile.

## 6. The thread model

### 6.1 One scheduling policy for every hot-path thread

The policy (`crates/verbatim-process/src/scheduling.rs`, decision 14):

- A thread that only moves messages, microseconds per wake, then waits again, runs at `THREAD_PRIORITY_HIGHEST`: the keyboard hook thread, verbatim.exe's relay thread, each child's I/O thread, and the `WinEvent` threads. A thread made ready at normal priority waits for a running normal-priority thread's quantum, which matched the measured 2 to 30 ms tails; a higher-priority thread pre-empts at once. Measured (section 11, step 3), this took verbatim.exe's own stage's maximum from about 4 ms to under half a millisecond, with no measurable change in processor time or wake-ups.
- A thread that computes on the path, sometimes for milliseconds, or calls into an application, runs at normal priority: the reducer, the speech queue and synth threads, each host's thread, each outpost's worker, and the listener's per-application reading threads. Raising them to above normal made typed echoes slower, and was withdrawn.
- The audio thread keeps MMCSS "Pro Audio".
- Everything else is normal (the GUI, the control plane, launch and retire helpers, the watchdogs, the router).
- No process or thread is opted out of power throttling: the measurement found no latency gain from it, and it costs energy for every unit of work. The power plan stays Balanced.
- Timer resolution is not raised: nothing on the path waits on a timer.
- Threads Verbatim does not own (UIA's callback threads) keep their priority. Verbatim does no lasting work on them: they encode and write one frame, or hand the event to an owned thread.
- The level of each message-moving role is named once in code, in a constant of its own. Platform-neutral crates get a priority only through a callback.

Starvation is not a risk of this policy: only threads that run for microseconds per wake are raised, so they cannot fill a processor. The remaining tail is the wait for the reducer, at normal priority. NVDA sets no thread priority, process priority, power-throttling state or timer resolution anywhere.

### 6.2 verbatim.exe

- Keyboard hook thread (`verbatim-input-hook`), `HIGHEST`. Woken by the system's low-level hook for each key. Decides against the lock-free gesture snapshot; on a key that cancels speech it sends the cancel to the speech queue's channel, in order with every `Speak`; posts a key's typing, its caret key and its `SpeechCancelled` to the reducer's port, and every gesture to the router on an unbounded channel (section 16). It never blocks on a lock, a channel that can be full, or a pipe.
- Relay thread (`verbatim-relay`, the supervisor's owner), `HIGHEST`. Waits only on its port, with the next heartbeat or sweep as its timeout. Woken by a read completion on the listener's pipe or any outpost's pipe, a pended write's completion on any child's command pipe, or its mailbox (launch helpers, retirements, hang-check results, the idle round, the app's requests). It parses and routes the listener's facts and writes them inline; from an outpost's pipe it reads only the frame's kind, handles pongs, `Ready`, `TargetExited` and the pipe's end itself, and posts every other frame whole to the reducer's port, then re-arms that pipe's read. It never blocks on process waits, thread joins, hang detection or file I/O.
- Reducer thread (`verbatim-reducer`), normal. Waits only on its own completion port. Parses each outpost frame in the order it arrives, reduces, writes requests to outposts inline, and speaks through the speech manager. It never blocks on a pipe.
- Speech queue thread (`verbatim-speech-queue`), normal: the speech queue's logic, the hook's cancel in order with every `Speak`, and the reply to the idle barrier (section 8).
- Synth thread (`verbatim-synth`), normal. Waits on its job channel between jobs; during a hosted job, on the host pipe's read event and the utterance's cancellation event together. Writes `Speak` and `Cancel`, reads PCM, trims, converts and appends to the mixer.
- Audio thread (`verbatim-audio`), MMCSS. Waits on the device's event and its wake event, and mixes.
- Off the path: the router, the GUI threads, the control plane's threads, launch and retire helpers, the hang check, the shutdown thread.

### 6.3 The listener

- I/O thread (its main thread), `HIGHEST`. Waits on its port. Woken by commands from verbatim.exe and by a pended write's completion. The menu-end grace is a threadpool timer.
- `WinEvent` thread, `HIGHEST`: the message loop for its global hooks. Builds each fact from the event's arguments and local reads, and writes it inline.
- UIA's callback threads (UIA's own): hand the event to its application's reading thread, or, for a fact built from cached properties, write it inline.
- Per-application reading threads, normal: judge and read the focus in one remote operation, then write inline.

### 6.4 An outpost

- I/O thread (its main thread), `HIGHEST`. Waits on its port. Woken by a frame from verbatim.exe, which it hands to the intake, and by a pended write's completion. Answers pings itself.
- Event thread, `HIGHEST`, and UIA's callback threads: push into the intake.
- Worker, normal: takes intake entries, calls into the application under its deadline, and publishes by writing inline. The write is overlapped and never waits; when one is in flight the message joins the backlog.
- Watchdog, normal: woken only for deadlines, never per entry.
- Target-watch thread: off the path.

### 6.5 A synthesizer host

One thread, normal. Idle, it waits on the command pipe's read event, an overlapped read always pending. Woken by a command, it reads every whole frame waiting, then speaks, applies a setting, or does nothing for a cancel of something already finished. While synthesizing it learns of a new command from the same event without a thread: eSpeak NG's callback checks the event with a zero wait once per chunk (60 ms of audio), and the OneCore driver waits on its operation's completion and that event together. It writes PCM by paced sends (section 4.3). One thread means no hop inside the host.

### 6.6 Locks shared with the path

- The sender locks: held for one encode and one `WriteFile`.
- The speech manager's and the mixer's locks, and the latency ledger's and the control plane's registry locks, are taken on the speech path as before; removing them from it was step 10, not adopted (section 8).
- No thread on the delivered path writes a file or the console under a lock.

### 6.7 The harness's idle barrier: a round through every child

Every scenario ends with the idle barrier (`AwaitIdle`), and the speech collector relies on it to know nothing more will be said. A completion port has no length, and a count of the reducer's inputs never covered frames still in an outpost's pipe or backlog, events an outpost has not yet published, or caret watches in flight. So the barrier is an in-band round, which covers whatever is in pipes and backlogs without counting them (decision 12):

- The reducer, given `AwaitIdle`, waits until it has handled the input the barrier names, then starts round n through the supervisor.
- The relay writes `IdleQuery(n)` to the listener. The listener answers once nothing that reached it before the query is in progress on its `WinEvent` thread or its reading threads, and once any menu end in its grace has been sent; the answer goes through its sender behind every fact written before it, never merged and never sent ahead as urgent.
- On reading the listener's answer, the relay tells the reducer which outposts it asks and writes `IdleQuery(n)` to every outpost in the same pass, so each query is behind every fact relayed to that outpost before it.
- Each outpost answers once the query has reached the front of its intake, every entry that arrived before it handled, and no caret watch, wait for a UIA focus after a `WinEvent` focus, or owed terminal read is pending; the answer goes through its outbound queue behind every message queued before it. An outpost or listener whose pipe ends during the round counts as having answered.
- When it has every answer, the reducer handles everything already in its port, then checks its own conditions: no request outstanding, no outpost starting, no focus wanted, no caret key held back still waiting to reach its application. If they hold, the round has passed, and the reply goes through the speech queue (`after_queued`), so every utterance queued before it has been broadcast on the same control-plane connection first. If not, it runs another round.
- Rules that make one round enough: nothing is merged across the query or the answer in any queue they pass through, and neither goes ahead as urgent, since each would move earlier work behind the barrier. Work that arrives after the query is not waited for, so a stream of events cannot hold the answer back.

The mechanism is in `docs/crates/verbatim-app.md` (the reducer's round) and `docs/crates/verbatim-outpost.md` (the relay's round, the listener's and the outpost's answers). The cost is about 3 + 4N wake-ups per barrier with N outposts, paid only when the harness asks. Limit: a key that opens a caret watch and moves nothing holds the barrier until the watch's bound, since the outpost cannot know the application has finished with the key.

### 6.8 Supervision: pings, pongs and the wedge verdict

- The relay pings every child every 3 seconds; the child's I/O thread answers itself, as an urgent message that goes ahead of the backlog (behind only a write already in flight); the relay reads the pong from the child's pipe itself. Three missed intervals, 9 s, is a wedge.
- The reducer is not on the way, so a stalled reducer can never make a healthy outpost look wedged, and no outpost is judged wedged while its pipe is unread, because the thread that reads it is the thread that judges.
- A stalled reducer is a fault of its own: the relay reads the reducer's port's counts on its heartbeat and reports a stall once three heartbeats in a row find inputs waiting and none taken. Nothing is killed for it.
- A changed list of an outpost's held nodes (`NodesHeld`) rides on Core's next write to that outpost, a request or the next ping, so it is at most one ping interval late and costs no write of its own (section 11, step 11).
- Every child stays pinged every 3 seconds, idle or not (decision 16).

### 6.9 Decision 3's alternative: the reducer reads the outposts' pipes

It would save one wake-up on every outpost message, and changes four things:

- Order under load: the reducer re-arms an outpost's read only when it dequeues that pipe's previous completion, so while it is busy, a key posted meanwhile is reduced before frames not yet read: a focus event written just before a review key, so the review reads the old focus and the focus announcement then cuts it off. For UIA events this departs from NVDA.
- Merging: a busy reducer fills an outpost's pipe, so the outbound merge rules act routinely (decision 8).
- Supervision: pongs reach the relay through the reducer, so no outpost could be judged wedged until the reducer had waited on an empty port.
- Head-of-line: a pipe drains at the reducer's pace (section 4.8).

## 7. The synthesizer host protocol

The mechanism is in `docs/crates/verbatim-synth-hosted.md` (verbatim.exe's side) and `docs/crates/verbatim-synth-host.md` (the host).

### 7.1 Messages

- Every frame from the host about an utterance names it: `FirstAudio(u, timing)`, `Pcm(u, format, samples)`, `Mark(u, mark)`, `Done(u)`, `Failed(u, reason)`. `Ready`, `Unavailable` and `SettingApplied` name none.
- Commands: `Speak(sequence)` (the sequence carries its utterance id), `Cancel(utterance)`, `SetSetting`.
- The host's stamps go in `FirstAudio`, written in the same write as the first `Pcm`, so they cost no write and no wake; the first chunk is timed from the host's reading the `Speak`, so a wait for its turn in the host is counted (section 10.1).

### 7.2 The next Speak goes right behind a Cancel

- On cancellation, the synth thread writes `Cancel(u)`, notes `u` as draining, and returns from the job at once.
- The next job writes `Speak(v)` immediately. Every read, for `v`, for a setting's answer or for anything else, drops every frame naming a draining utterance, its `Done` or `Failed` ending the draining. It never waits for them.
- The host reads `Cancel(u)` and `Speak(v)` together at its next chunk boundary (or at once if idle or waiting in a paced send), stops `u`, writes `Done(u)`, and starts `v`. A `Cancel` of an utterance still waiting its turn makes it a skip, answered with `Done` unspoken.
- Backpressure while draining: the host may be blocked in a paced send of `u`'s audio when the cancel comes, with up to one pipe buffer and one frame waiting. It does not cancel that send: the frame in flight is completed whole, and only then does the host stop `u` (section 4.3), so the PCM pipe never holds a partial frame and no restart follows.
- Recovery: a host that ends, or that has owed a message for `HANG_TIMEOUT` and sent nothing, fails the utterance in flight and is replaced. Since the driver no longer waits for a cancelled utterance's `Done`, the silence is counted while the host owes that `Done` too, across requests, so a host frozen while every utterance is cancelled is still found. A host that answers with an utterance it was never given is misbehaving.

This removes, on every key that interrupts an utterance still being synthesized, about ten wake-ups across two processes and up to one eSpeak NG chunk of processor time. NVDA's eSpeak NG driver behaves the same in effect: `stop` stops the player, the synthesis callback aborts at its next chunk, and the next `speak` is queued behind that last chunk, which is longer, from a 300 ms buffer.

### 7.3 Cancellation signalled, not polled

- In verbatim.exe: the speech job's cancellation is a token in `verbatim-speech` (`CancelToken`) with a flag and an optional waker. The hosted driver registers a waker that sets an event of the utterance's own, and waits on that event together with the host pipe's read event. Each utterance's event is new, so a late waker of an earlier utterance never wakes a later one's wait.
- In the host: cancellation is the command itself. The command pipe's pending read signals its event when any command arrives. eSpeak NG's callback checks the event once per chunk with a zero wait, a system call of about a microsecond; the OneCore driver waits on its operation's completion and that event together.
- This removed up to about 30 ms between a key and the host stopping with OneCore (two 10 ms polls, each rounded up to the 15.6 ms timer tick), and a few milliseconds at the start of an utterance with eSpeak NG.

### 7.4 The host's threads

One thread, as section 6.5 says. The command pipe and the PCM pipe are the transport's, with the PCM pipe's 4 KiB buffer kept and its samples in the frame's raw section (section 4.2).

## 8. Speech inside verbatim.exe: the queue thread kept

Step 10 would have removed the speech queue thread and the remaining locks from the speech path. It is not adopted (decision 5, as revised): with the real host and device, the reducer's speak to the synth thread taking the job is under a millisecond at the 99th percentile, synthesis request to the engine taking the first frame is under a millisecond apart from the device's period, and a key that cuts off a long announcement has its echo as fast as one in silence (section 15). The largest stage left on the way is the device's period, which is the audio step's (section 11).

What it would have done, kept as the alternative should a later measurement call for it:

- The queue's logic would run on the reducer under the speech lock, saving the queue thread's wake on every announcement.
- The hook's cancel would be lock-free: raise a key fence and a speech generation (atomics), then signal the synth thread and the audio thread, with every utterance recording the generation it was admitted under, and the mixer and the job cutting by generation. Admission reads the generation before the fence, and the cancel raises the fence before the generation, so an utterance caused by an earlier key is never heard after a later key's cancel. The interleaving that a lazily cleared watermark let through (the synth thread popping an utterance queued before the key and registering it after the watermark) is what made the generation necessary.
- Every speech event would be pushed as it happens into one ordered channel to a reporting thread, which broadcasts it to the control plane and answers the idle barrier, so utterances cut before they play are still broadcast as queued.
- Index marks and `SpeechCancelled` would still reach the reducer's port directly; pause would be one atomic word applied and reported by the audio thread; the mixer lock would be released across every WASAPI call.

Review and navigation keys likewise still go through the router (step 8, not adopted): if they went straight to the reducer, the router's own speech (the time, the date, the tray list) would have to be fenced by its key, so a later key's review is never cut off by it, as NVDA runs every gesture's script in key order.

## 9. Wake-ups from the event or key to the first sample

A wake-up is a thread made ready from a wait, counted in order along the path. Before the design a UIA focus change woke 17 threads before its first sample: the listener's callback and outbound threads, verbatim.exe's listener reader, owner and writer, the outpost's main, worker and outbound threads, verbatim.exe's reader, the reducer, the speech queue, the synth thread, the host's reader and main threads, verbatim.exe's host reader, the synth thread again, and the audio thread. Counted the same way from the thread model of section 6, not measured, it now wakes 12: the listener's callback and reading thread, the relay, the outpost's I/O thread and worker, the relay again, the reducer, the speech queue, the synth thread, the host's thread, the synth thread again, and the audio thread. A typed echo wakes the hook, the reducer, the speech queue, the synth thread, the host's thread, the synth thread again and the audio thread. NVDA, one process, wakes about three for either. The difference is the price of D9, D13 and D18's isolation, one process boundary each way plus the host, and of keeping the arrival order of section 4.5 (decision 3).

## 10. The latency ledger

### 10.1 Stages

Every hop has a stage. A stage inside one process is a duration measured with the performance counter on one thread, or between two threads of one process; a stage that crosses processes is a difference of the shared clock (`verbatim_process::clock`, section 3.3). Every cross-process hop on an announcement's way has two stamps: the sender's, taken as its writer begins the write, just before the frame is encoded, since the stamp travels inside it; and the receiver's, taken as the frame is read. The sender's own time before its write is a stage of its own, so a hop's stage is the pipe and the receiver's wake. Each stage below gives its `LatencyStageKind` and, in quotes, the label the latency line, `verbatim-inspect latency` and `stages.txt` print.

The event side, in path order:
- Windows (`Windows`, "Windows"): the `WinEvent`'s raised time to its observation (tick-count resolution, so partly quantization).
- Hook to Core (`HookToCore`, "hook to Core"), for a caret key: the hook's callback to the reducer having handled the key and asked the outpost.
- Listener handling (`ListenerHandling`, "listener handling"): the listener's observation of the event to its having handled the fact (in-process). For a fact a reading thread judges (a UIA focus, menu opening, selection or notification) that is the reading thread's queue and its cross-process read or check, ending when the thread finishes with it, read or dropped. For every other fact it is the callback's own work, from the callback's start to the fact queued; so is a drop decided without a read.
- Listener queue (`ListenerQueue`, "listener queue"): the handling's end to the listener's writer beginning the frame's write (in-process).
- Listener's pipe (`ListenerPipe`, "listener's pipe to verbatim.exe"): the listener's write to the relay's read completion (cross-process).
- Listener to relay (`ListenerToRelay`, "listener to verbatim.exe"): the three above taken whole, only for a timeline without the listener's write stamp.
- Outpost start (`OutpostStart`, "waiting for the outpost to start"): time held for a starting outpost (in-process; reported separately and never pooled with delivered facts).
- Relay (`Relay`, "verbatim.exe"): the relay's own handling, from the read completion to its beginning the write to the outpost (in-process).
- Relay to outpost (`RelayToOutpost`, "verbatim.exe to outpost"): that write to the outpost's reader having read it (cross-process).
- For a request (`RequestToRelay`, "reducer to verbatim.exe's relay"): the reducer having handled the input to the request's write beginning, or the wait in the backlog when a write is in flight (in-process).
- Listener to outpost (`ListenerToOutpost`, "listener to outpost") and Core to outpost (`CoreToOutpost`, "Core to outpost"): the spans to the outpost taken whole, only for a timeline without verbatim.exe's relay stamps.
- Outpost queue (`OutpostQueue`, "outpost queue"), caret wait (`CaretWait`, "caret wait"), outpost read (`OutpostRead`, "outpost read"): in the outpost, up to the worker's publish.
- Outpost outbound (`OutpostOutbound`, "outpost outbound"): the worker's publish to the outpost's writer beginning the write (in-process).
- Outpost's pipe (`OutpostPipe`, "outpost's pipe to verbatim.exe"): that write to the relay having read the frame (cross-process).
- Outpost to relay (`OutpostToRelay`, "outpost to verbatim.exe"): the two above taken whole, only for a timeline without the outpost's write stamp. To Core (`ToCore`, "to Core") is the span to the reducer taken whole, for one without verbatim.exe's read stamp either.
- Reducer wait (`ReducerWait`, "waiting for the reducer"): the relay's read of the frame to the reducer starting on it (in-process), which is mostly the reducer being busy, labelled so it is never mistaken for transport.
- Reducer (`Reducer`, "reducer").

The speech side, in path order:
- To speech (`ToSpeech`, "to speech"): the reducer's speak to the job taken by the synth thread (in-process). Time waiting behind earlier speech is reported and not counted.
- Synthesis (`Synthesis`, "synthesis"): for a synthesizer in verbatim.exe's own process, synthesis started to its first audio. A synthesizer host's is split into the next four.
- Synth request (`SynthRequest`, "synthesizer request"): the driver asked to speak to its writer beginning the `Speak` request's write (in-process).
- To host (`ToHost`, "to the synthesizer host"): that write to the host having read the request (cross-process).
- Host first chunk (`HostFirstChunk`, "the host's first chunk"): the host reading the request to its synthesizer's first chunk (in the host, a duration). A `Speak` can be read while the cancelled utterance before it is still ending (section 7.2), and its wait for its turn is in this stage.
- Host return (`HostReturn`, "back from the synthesizer host"): the host beginning to write its first audio (`FirstAudio`) to the synth thread handling it (cross-process): the encoding, the pipe, the synth thread's wake and read, and any frames read with the first audio and handled before it.
- Leading silence (`LeadingSilence`, "leading silence").
- Mixer and device (`MixerAndDevice`, "mixer and device"): the first frame's append to the engine having taken it, by padding (in-process).
- Device clock (`DeviceClock`, "device clock"): the engine having taken it to `IAudioClock::GetPosition` reaching the first frame's position, converted to time with the position's own performance-counter stamp (in-process).

`cargo xtask measure`'s first total sums verbatim.exe's stage, its hop to the outpost, the outpost's outbound queue and pipe (or their whole span, for a timeline without the outpost's write stamp), and the reducer wait.

Messages outside the ledger, since nothing on an announcement's way crosses them, are logged at trace level under the `verbatim::stage` target (`RUST_LOG=info,verbatim::stage=trace`), each with the stage's label and its time in microseconds:
- "verbatim.exe to the listener": a command's `sent_at_us` to the listener's read of it, logged by the listener.
- "listener's pipe to verbatim.exe", for a pong: its `written_at_us` to verbatim.exe's read; "outpost's pipe to verbatim.exe" for an outpost's pong, logged by verbatim.exe.
- A menu's or switcher's end: its listener handling, listener queue (which holds the 50 ms grace before it is sent), and listener's pipe to verbatim.exe, logged by verbatim.exe.

### 10.2 Stamps

- Stamps travel with the data: in each message across processes, and in each speech job inside verbatim.exe. A stamp costs one clock read, about 30 bytes of JSON in the frame, and no wake or system call.
- The ledger keeps a bounded number of timelines, and every event an outpost sends starts one, spoken or not. A timeline whose speech is still owed is evicted only when every other timeline but the newest is owed too, so a burst's unspoken events cannot push out the timelines of lines spoken hundreds of milliseconds later (`docs/crates/verbatim-app.md`, the latency ledger).

### 10.3 The measurement scenario through the real host and device

`measure_speech_real` runs Verbatim with eSpeak NG through its host and the real WASAPI device. It drives one fixed target, the Windows Forms text box the suite uses for edit controls, through a fixed sequence: focus changes into and out of it, typed characters, caret keys, a review key, and a key pressed during a long announcement. It records every stage of 10.1 for each announcement and reports distributions per stage, not a pass or fail (decision 10); D15's budgets become assertions once the figures are known. It runs locally, where a real device exists, not in hosted CI, which has no sound device.

## 11. Migration, and the audio step

The measure set, the end-to-end registry's `measure` group (`docs/tooling/e2e.md`), is the workload each step was measured with, together with `measure_speech_real` and the traces of section 18. Each step was small, passed `cargo xtask ci`, and was verified before the next. What each step built:

- Step 1, the baseline: the ledger's return-path and speech stages, and decision 1's measurement of JSON's cost.
- Step 2, the platform floor and `measure_speech_real`.
- Step 3, the scheduling policy, as decision 14 defines it. Three configurations were measured in debug and release on the same build: no priorities; `THREAD_PRIORITY_HIGHEST` for the threads that only move messages; and that with every process and those threads opted out of power throttling. Raising the message-moving threads took verbatim.exe's own stage from over 2 ms in 0.7 to 0.9 percent of samples, maximum 2.6 to 4.2 ms, to none over 1 ms, maximum 0.2 to 0.4 ms; the wait for the reducer, at normal priority, grew to take part of what they saved, and the summed path's 99.9th percentile still fell. Wake-ups and processor time per key event did not change. The power-throttling opt-out gained nothing. Adopted: the second. Raising the threads that compute, as first written, made typed echoes slower and is not done.
- Step 4, the transport: frames, the overlapped pipes, the `Sender` with the caller's backlog, inline writes in verbatim.exe.
- Step 5, the relay on its port: the supervisor's owner became the relay thread, reading the listener's pipe and finishing every child's pended writes; verbatim.exe's listener reader and per-child writer threads went.
- Step 6, the children's side: `ChildIo`, inline writes in the listener and every outpost, one I/O thread each; the children's outbound threads and separate pipe-reading threads went; the menu-end grace became a threadpool timer.
- Step 6a, the idle barrier through every child (section 6.7), built before step 7 because step 7 changed who reads the outposts' pipes and so what the old channel-length count counted.
- Step 7, the relay reads the outposts' pipes and the reducer waits on its own port (decision 3); verbatim.exe's per-outpost reader threads and the channel between the hook and the reducer went. What the relay sends the app and the app's port refuses is kept, with everything sent after it, in the relay's outbox and delivered again in order, so a refused `Ended` is never lost.
- Step 8, review keys straight to the reducer: not adopted. More commands are still to be implemented, and whether most of them should go straight to the reducer or through the router is reconsidered when the command set is settled (section 8).
- Step 9, the host protocol of section 7.
- Step 10, the speech manager without its queue thread: not adopted, since it is not needed (section 8).
- Step 11, the outpost's per-event costs: the watchdog woken only for deadlines; the entry's description formatted only when debug logging is on; logging out of the intake lock; and changed held nodes (`NodesHeld`) no longer sent on their own. The reducer leaves the latest changed list in a slot in each outpost's writer, a newer one replacing a list not yet sent, and the list goes in the next command's write to that outpost, after the command, whichever thread writes it: a request from the reducer, the relay's ping every 3 seconds, or any other command but the shutdown message. So a list is at most one ping interval late, never costs a write of its own, and a flood costs no extra round trip. A list is left whenever the held nodes, the anchors, or the position of the outpost's last message carrying node ids changed, so an outpost releases what it reported within one ping interval (`docs/crates/verbatim-outpost.md`, held objects).
- Step 12, the measurements of every step against step 1's baseline (section 15).

### The audio step

Decided by Dickson and not yet built. Not communication, but placed in the thread model so the steps do not conflict, and each part is measured with the device stages of 10.1.

- Keeping the device awake: after speech, the audio thread writes silence one engine period at a time for the awake period, configurable, 30 seconds by default as in NVDA (`audioAwakeTime`), because Bluetooth, USB, HDMI and virtual-machine endpoints otherwise clip or delay the next speech. In this thread model that is more wakes of the audio thread only, at its device period, with nothing else woken; new speech never waits behind more than one period of silence.
- Cancelling keeps `Stop` and `Reset`, as NVDA does, since shared mode has no other way to discard queued audio.
- The engine period: measured first, from the ledger's device clock stage; `IAudioClient3`'s shorter shared period tried only with the stream kept awake. D15 records that the development machine's device offers none shorter than 10 ms.
- The earcon re-mix: speech that starts while an earcon plays is mixed into audio not yet played, re-mixing what was already handed to the device, so an indication's sound and its speech start together and cancelling an utterance cancels both. It is the audio thread's work, under the mixer lock. Speech now waits behind the roughly 40 ms of earcon already queued, which delays every start-up's first announcement.
- The latency ledger stops counting the tail of earlier speech as "mixer and device", which made back-to-back utterances look slow there.

## 12. Alternatives from other projects

A survey of 21 Rust projects with multi-process communication settled four points:

- Unnamed overlapped pipes, as Rust's standard library makes its child pipes: both ends can be overlapped and no name exists at any moment (section 4.1). The check that the child's end, opened without the synchronous flag, is overlapped held: it reports no synchronous mode, and a read on it pends and completes through a completion port. `NtCreateNamedPipeFile` is not in the `windows` crate, so it is declared against ntdll, as the standard library declares it.
- verbatim.exe's last message to a child, such as `Shutdown`, must be read before its end closes, since a server end that disconnects discards unread data.
- Waiting on child exits through the relay's own port (`NtAssociateWaitCompletionPacket`) was not adopted: the helper thread it would remove exists only while a child verbatim.exe chose to end is shutting down, rarely and never on the path, and adopting it would move the time limit, the kill and the ending's report into the relay's loop, whose every delay holds up every message. A child that crashes needs no wait: the kernel closes its pipe as it ends, and the relay sees the pipe's end at once.
- Producers writing inline has no precedent among the projects read; each uses a writer thread or an event loop that owns the pipe. It is kept because it removes the writer's wake-up on every message.

Confirmed as well trodden: length-prefixed frames, 64 KiB buffers, one write per frame, the overlapped buffer lifetime rule (a slot dropped with an operation in flight leaks rather than frees), a single relay thread serving many pipes, binary data beside JSON, ping-and-miss supervision, and MMCSS for audio. `mio`'s named pipes were weighed and not used: they never skip the completion port on success, so after any write the pipe stays busy until its poll thread runs, and they read through an internal buffer and copy again.

## 13. Decisions

Approved by Dickson between 2026-10-11 and 2026-10-15.

1. Frames are JSON behind a readable text length header, after measuring the cost of parsing and re-serializing a focus fact (section 4.2); the binary header comes back only if a measurement shows the cost matters. The bulk of a terminal's text and colours and of PCM travels in a raw section (2026-10-28).
2. Children's pipe ends are overlapped too, and every producer writes its own message when nothing is ahead of it, which removed the listener's and the outposts' writer threads.
3. The relay thread reads the outposts' pipes and posts every frame to the reducer's port, rather than the reducer reading them itself (section 6.9): it keeps the order of outposts' events against keys and against each other, keeps continuous draining so no merge rule acts because the reducer is busy, and keeps pongs independent of the reducer. NVDA keeps the same order for UIA events. The one wake-up it costs per outpost message is a hop at `HIGHEST` of microseconds (decision 15).
4. Superseded by decision 14.
5. The speech queue thread and lock-free cancel by generation (section 8): not adopted as step 10, since the speech path shows no noticeable delay.
6. The host protocol of section 7: frames tagged by utterance, the next `Speak` right behind a `Cancel`, cancellation signalled by the command's arrival, one host thread.
7. Review and navigation keys straight to the reducer, the router's speech fenced by its key (section 8): not adopted as step 8, reconsidered when the command set is settled.
8. Whether outposts' merge rules may act because the reducer is behind: moot with decision 3. Under its alternative, events derived from UIA would never be merged, since NVDA's UIA flusher drains into a queue that never merges, while MSAA-derived events merge as NVDA's limiter does while its main thread is behind.
9. The budget reading applies to every verbatim.exe stage of section 10, including the reducer's request writes and the speech stages inside verbatim.exe: each within 1 to 2 ms of its own work, each hop no worse than the platform floor, every exception attributed by trace.
10. `measure_speech_real` reports distributions and becomes a gate (D15's budgets) only once its figures are known; it runs locally, not in hosted CI.
11. Named single-instance pipes in place of "no named endpoint": withdrawn, since the pipes have no name (section 12).
12. The harness's idle barrier is an in-band round through the listener and every outpost (section 6.7).
13. The messages exchanged with the outposts and the listener move into a platform-neutral crate as plain serde types, so another platform's port shares the messages and the code that turns them into Core's inputs. Approved in principle, scheduled after the migration (`docs/roadmap.md`).
14. The scheduling policy of section 6.1: `HIGHEST` only for the threads that only move messages, adopted where it shortened a measured tail without raising processor time; no power-throttling opt-out; normal priority for the threads that compute; MMCSS for audio.
15. Decision 3 again, with wake-ups counted: the relay keeps reading, since the saving is a small share of a flood's wake-ups and the departure from NVDA would be audible.
16. Pinging only children with work outstanding: declined; every child stays pinged every 3 seconds.
17. verbatim.exe's hidden top-level frame receives every broadcast message from every application, about 67 a second while typing elsewhere; whether the tray icon and its menus can be owned by a message-only window is a question for the GUI's design (`docs/design/wake-ups.md`).

## 14. Risks

- One relay thread carries every child's writes and reads. A bug that blocks it stops all relaying and all input from outposts; one that blocks the reducer stops all reduction. The rule that neither ever blocks, logs on the delivered path or does file I/O is enforced in review; the relay watches the reducer (section 6.8).
- Overlapped buffers and `OVERLAPPED`s must outlive their operations, in every process. The transport is their only owner, with the rule that a slot dropped with an operation in flight leaks rather than frees.
- A write that fits the pipe buffer completing at once is what makes inline writes cost no wake; it is Windows' documented quota behaviour for pipes.
- Raised priorities could starve other threads if a raised thread ever looped; every raised thread waits between messages, and the flood scenarios run with them in place.
- On a virtual machine the measured in-process maximum is already 13 to 32 ms, so absolute maxima may be impossible there; exceptions are attributed by trace rather than ignored.
- Core and its children always ship together, so a wire format change needs no versioning; serde's defaults let a reader skip fields it does not know.

## 15. Performance

Current figures are in `docs/performance.md`; these are the ones the design's choices rest on, measured 2026-10-14 on this development machine, the two-processor development VM, after steps 4 to 9 and 11, two runs each of the measure set:

- Parsing on the reducer: a few microseconds a frame for the common messages in release; a relayed focus read with 12 ancestors about 15 microseconds to parse and re-serialize; a terminal's whole-document read with its text in the raw section 0.02 to 0.29 ms (section 4.2).
- verbatim.exe's own stage: 99.9th percentile at most 0.5 ms in debug and 0.7 ms in release, none over 1 ms; verbatim.exe to an outpost, maximum 1.6 to 4.1 ms.
- Waiting for the reducer: median 0.1 to 0.2 ms, 99th percentile about 1.3 to 2.2 ms; the parse of each outpost frame on the reducer adds 0.1 ms at the median.
- Hook to Core: over 1 ms for 22 to 30 percent of keys, since on two processors the reducer, readied by the same key as the synthesizer thread and the host, waits for a processor. What the user hears improved: in release, in the console, a key to its sound has a median of 2.5 ms, against 5.4 before steps 6 to 11. Raising the reducer would need its own measurement under decision 14; accepted as it is (Dickson, 2026-10-14).
- With the real host and device: the reducer's speak to the synth thread taking the job 99th percentile at most 1.1 ms; synthesis request to the engine taking the first frame under a millisecond apart from the device's period; mixer and device with the device clock about 10 ms, the device's period; an echo that interrupts a long announcement costs no more than one in silence (median 2.5 to 5.5 ms from the synthesizer request to the engine taking its first frame, against 5.0 to 7.5 ms for echoes in silence).

## 16. Audio device recovery, the keyboard hook's channel, and limits

- A failed open of the audio device waits for evidence. The mixer plays real-time silence in the device's place and opens the device again only when it asks (`needs_reopen`). The WASAPI device registers once, on the audio thread, for Windows' endpoint notifications and passes them to `DeviceNotices`: an open device is reopened when the default render device changes; after a failed open, also when a device arrives or becomes active. No render device (`E_NOTFOUND`) waits for those notifications alone, with no timer. Every other failure is also tried again a second later, because it can come from a device that is still there, whose recovery Windows sends no notification for: a device another program holds in exclusive mode (`AUDCLNT_E_DEVICE_IN_USE`) raises no notification when released, and `AUDCLNT_E_DEVICE_INVALIDATED` reports a device reconfigured in place, such as the default device's format changed, as well as one that went away, with only property changes raised and those before the failed open, so passing property changes on as a reason to reopen would not recover it. A retry asks for the default device afresh, so it reaches the new default, or gets `E_NOTFOUND` and then waits with no timer. The audio thread's wake-ups are unchanged in steady state (`docs/crates/verbatim-audio.md`, device recovery).
- The keyboard hook never drops a gesture. It sends every gesture and harness mark to the router on an unbounded channel, whose send never waits, and `InputHook::start` refuses a bounded one, so the hook neither blocks nor loses a key. A backlog waits in memory in front of the router, as every backlog in this design waits in front of the reducer (section 4.8), at about 70 bytes a queued gesture.
- The reducer's port has no bound, and the relay drains every outpost's pipe whatever the reducer does, so a flood during a reducer stall grows memory for as long as it lasts; the stall watch only logs.
- An abandoned outpost worker whose call into the application later returns publishes nothing, but still writes the outpost's shared state: the caret's stamps and anchor and the owed terminal reads. It matters only when an application freezes past the deadline and recovers. The fix, deferred by Dickson: a `commit(generation, write)` beside the watch's `publish`, with every write to shared state moved after its call and into a commit, after an audit of every piece of state the worker shares.
- A large frame still costs the relay one copy, from the read slot into the reader's pending buffer. Reading the rest of a large frame straight into that buffer would remove it but changes the read slot's buffer ownership; deferred, since no traced workload carried a frame over 256 KiB.
- PCM is copied about ten times between eSpeak NG and the device; each copy costs microseconds.
- The control plane keeps its own overlapped pipe code.

## 17. What the design keeps of the earlier behaviour

Each point was checked against NVDA and the suite, with what guards it:

- Speech frames on the control plane: every utterance is broadcast as queued when it is queued, including those cut before playing, as the Control and Shift flood scenarios and the speech collector's `expect_queued`, `take_until` and `expect_nothing_more` require. Guarded by those scenarios.
- The idle barrier covers frames in pipes and backlogs, events not yet published and caret watches (section 6.7). Guarded by mockapp's `idle_barrier.rs`, the supervisor's `idle_round.rs`, and every scenario, since each ends with the barrier.
- Wedge verdicts: a stalled reducer makes no healthy outpost look wedged (section 6.8). Guarded by `crates/verbatim-outpost/tests/arrival_order.rs` and the pong-while-worker-stuck test in mockapp's `shutdown.rs`.
- Frames never cut mid-write: a started frame is never cancelled, a child drains before it exits, a marker means completed, and a cut frame is the pipe's end (sections 4.2, 4.3, 4.6, 7.2). Guarded by the transport's tests, the host's `protocol.rs` tests, and `outposts_shut_down_cleanly`.
- Arrival order under load: a frame written before a key is reduced before it however busy the reducer is (section 4.5). Guarded by `arrival_order.rs`.
- No new merging: the relay keeps draining while the reducer is held, so the outbound merge rules act no more than before. Guarded by `arrival_order.rs`.
- Settings against speech: a setting applied right after a cancel is answered without the cancelled utterance's frames (section 7.2). Guarded by the host's `protocol.rs` tests.
- Crash recovery: an outpost's pipe end follows every frame before it; a host that dies while a cancelled utterance drains is found at the next request. Guarded by `outpost_crash_recovery` and `synth_host_crash_recovery`.

## 18. Wake-ups and CPU

Every change on the path is judged on each thread's wake-ups and processor time as well as on latency, because a latency figure alone hides a cost paid on every event.

- Each change reports, besides the latency stages of section 10, each thread's wake-ups (times readied from a wait) and its time on a processor, for the measure set's workloads, from `cargo xtask trace` read by `verbatim-trace-waits` (`docs/tooling/measuring.md`). `--wakers`, `--stacks` and `--samples` say who woke a thread, from where, and where its processor time went.
- Counts are compared per unit of work where the unit is countable (per announcement, per key, per outpost message), since a fixed trace window catches more or less of a scenario: the synth thread's count in the same 8 seconds of `measure_speech_real` varied from 931 to 1,637 across three runs.
- A change passes only if no workload's wake-ups or processor time rise beyond that variation, unless the rise is its stated purpose and Dickson has agreed to it; a change that removes a thread shows its wake-ups gone and no other thread's grown by more than it saved.
- Threads that exist only for the test harness (the WAV recorder, the control plane's speech writers and readers) are reported and left out of the comparison.
- Traces are short and deleted once read.

The largest sources of wake-ups are outside this design (`docs/design/wake-ups.md`): the outposts' event intake, which wakes for every event the application raises before Verbatim's filter runs; the workers' cross-process calls, lowered by remote operations; terminal floods, most of whose processor time is UI Automation's client-side work; and the liveness pings, the only wake-ups idle Verbatim makes itself (decision 16).
