# Latency audit: hand-offs and waits beyond the relay

Status: superseded by `docs/design/communication.md`, which subsumes findings 3, 4, 5 and 7 to 12 and keeps 1, 2 and 6 as its audio step; kept for the record.

Written 2026-10-11. Read-only audit of main at 50877e0. Nothing was built or run. Sources: the code cited by file and line, `docs/design/relay.md`, `docs/performance.md`, `docs/design/phase6-design.md`, `docs/roadmap.md` lines 46 to 62, the scratchpad's `supervisor-latency.md`, and latency lines from the end-to-end runs kept in the scratchpad. Where a figure was measured it says so. Every other cost is labelled as an estimate, with the reasoning.

## 1. What the recorded measurements cover, and what they do not

- The end-to-end suite runs Verbatim with `VERBATIM_TEST_AUDIO=null` (`crates/verbatim-e2e/src/scenario.rs:373`) and the capture synthesizer (`scenario.rs:2724`). The capture synthesizer runs inside verbatim.exe (`crates/verbatim-app/src/main.rs:672-678`) and the device is `SilentDevice`. So no recorded latency line has ever measured the synthesizer host process or WASAPI. The "synthesis" and "mixer and device" figures below are for the stand-ins, not for eSpeak NG through the host or for a real device. This is the largest gap in the evidence, and the findings ranked highest depend on it.
- The suite also runs outposts with `verbatim_outpost=debug` (`scenario.rs:159`), in debug builds, on the Proxmox VM. Debug logging inflates the outpost stages.
- I pooled every distinct `verbatim::latency` line in the scratchpad's `runs-*` folders and `lat_all`: 1,038 announcements. Per stage, median, 90th and 99th percentile, maximum, and the share over 2 ms (all in milliseconds):
  - to Core (outpost worker's publish until the reducer takes the message): 0.3, 1.1, 5.3, 14.5; 5.3 percent over 2 ms.
  - to speech (reducer done until the speech queue has taken the utterance): 0.2, 0.5, 6.8, 2,909; 2.7 percent.
  - synthesis, capture synthesizer, one thread with no hand-off inside the stage: 0.4, 1.3, 8.1, 48.7; 5.2 percent.
  - reducer: 0.0, 0.1, 0.2, 5.8; 0.1 percent.
  - mixer and device, `SilentDevice`: 0.6, 67.7, 79.4, 98.3; 32 percent. `docs/design/phase6-design.md:2617` attributes this to speech queued behind an earcon's 40 ms already handed to the device, and to the ledger counting the tail of earlier speech.
  - Windows (how long before the hook saw it Windows raised the event): 0.0, 0.0, 16.0, 47.0. This is computed from the event's tick-count time, which moves in steps of about 15.6 ms, so the 16 ms values are partly quantization.
  - listener to outpost and outpost queue include outpost start-up waits and floods, which `supervisor-latency.md` already explains.
- The relay hops measured in `supervisor-latency.md` (release): a median of about 0.06 ms per hand-off, with 0.24 to 0.99 percent of hand-offs over 2 ms. I use these as the cost of one wake-up below.

## 2. Hop counts today

### 2.1 An application's UIA focus change, steady state, until the first sample

UIA focus always comes from the listener's desktop-wide handler (`crates/verbatim-outpost/src/listener.rs:458-465`), so it always takes the relay. Each item is one thread woken.

1. Listener, UIA's callback thread: reads cached properties (no cross-process call), takes the `Outgoing` mutex, scans and pushes its queue, signals a condition variable (`listener.rs:143-182`). One lock, one queue.
2. Listener outbound thread: serializes to JSON, then two `WriteFile` calls, one for the line and one for its line feed (`crates/verbatim-outpost/src/protocol.rs:864-879`).
3. to 5. verbatim.exe: listener reader, owner, per-child writer, as `relay.md` section 2 counts: two queues, two locks, one parse, one serialize, one or two `ReadFile`, one `WriteFile`.
6. Outpost main thread: `ReadFile`, parse, hand-over lock, intake push with a debug log line inside the intake lock (`crates/verbatim-outpost/src/outpost/intake.rs:289-322`), signal.
7. Outpost worker: plans the batch under the intake lock (`intake.rs:340`), starts its deadline, which takes the watch lock and wakes the watchdog thread (`crates/verbatim-outpost/src/outpost/worker.rs:187-195`), formats a description string for a debug log on every entry whether or not debug is on (`worker.rs:682`), reads the application (the cross-process calls `docs/performance.md` counts), then publishes under the outbound mutex and signals (`crates/verbatim-outpost/src/outpost/outbound.rs:318-326`).
8. Outpost outbound writer thread: serializes, then two `WriteFile` calls (`outbound.rs:390`, through `write_message`).
9. verbatim.exe outpost reader thread: `ReadFile` through a `BufReader`, parse, then a send on the reducer's unbounded channel (`crates/verbatim-outpost/src/supervisor/owner.rs:1069-1122`).
10. Reducer thread: three or four takes of the ledger mutex, `reduce`, the flight recorder mutex (`crates/verbatim-app/src/main.rs:1364-1393`), the key-origin mutex scanned linearly twice (`main.rs:1497`), then a send to the speech queue (`main.rs:1499`). After the speech is handed on, `send_views` (`main.rs:1409-1453`) rebuilds sets for every live outpost on every input and usually queues a `NodesHeld` message, which wakes three more threads (the writer, the outpost's reader, the worker). Since communication step 11, the list rides on Core's next write to the outpost instead (finding 12).
11. Speech queue thread: presents the utterance, takes the ledger mutex, and for an interrupting utterance cancels what is handed on, which is a blocking request to the audio thread: the queue thread waits until the audio thread has stopped the device (`crates/verbatim-audio/src/mixer.rs:399-415` and `631-633`). It then registers with the mixer under the mixer lock and sends the job (`crates/verbatim-speech/src/manager.rs:973-1003`).
12. Synth thread: clones the sequence, checks the host process (one syscall), serializes JSON, one `WriteFile` to the host (`crates/verbatim-synth-hosted/src/lib.rs:326-330`).
13. Host reader thread: reads and parses, sends on a channel (`crates/verbatim-synth-host/src/main.rs:96-115`).
14. Host main thread: eSpeak NG synthesizes its first 60 ms chunk (`crates/verbatim-synth-espeak/src/lib.rs:65`), then the PCM is copied three times and written with one `WriteFile` (`main.rs:194`, `crates/verbatim-speech/src/hosting.rs:138-151` and `211-222`).
15. verbatim.exe host reader thread: reads, copies twice more, sends on a two-slot channel (`verbatim-synth-hosted/src/lib.rs:165-185`).
16. Synth thread again (it waits in 10 ms slices, `lib.rs:338`, but a message wakes it at once): ledger mutex, trimmer, resample under the converter lock, append under the mixer lock, set the audio thread's event (`mixer.rs:442-489`, `646-697`).
17. Audio thread (MMCSS "Pro Audio", `crates/verbatim-audio-wasapi/src/lib.rs:261`): under the mixer lock, `GetCurrentPadding`, mix, `GetBuffer` and `ReleaseBuffer`, and `Start` because the stream was stopped (`mixer.rs:986-1074`). The first sample then waits for the shared engine's period.

Total: 17 wake-ups across four processes, plus the watchdog's. The relay step removes two (owner and writer), leaving 15. An MSAA application whose outpost's own hook is in skips items 1 to 5 but adds the outpost's event thread: 13 today.

### 2.2 A typed character's echo

The hook thread (woken by the system's low-level hook message, which also holds the key back from every application until it returns) sends the cancel to the speech queue (`main.rs:378`) and the typed text to the reducer (`crates/verbatim-input-windows/src/lib.rs:492`). Then items 10 to 17 above: 8 wake-ups to the first sample, plus two for the cancel (the queue thread, then the audio thread, then the queue thread again).

### 2.3 A caret key, a review command, a query

- A caret key goes from the hook to the reducer, then as a request through verbatim.exe's writer (after the relay step, the relay thread) to the outpost's main thread and its worker, which waits for the caret event; the reply returns by items 7 to 17. About 16 wake-ups.
- A bound review or navigation key goes from the hook to the router thread (`main.rs:310`, `1664`), which logs it at info level (`main.rs:1723`) and only then forwards it to the reducer: one more wake-up and a synchronous log write than a caret key.

### 2.4 Cancellation

The hook sends `Cancel` to the speech queue thread. That thread sets the job's cancel flag and makes the blocking `cancel_all` request; the audio thread wakes, calls `IAudioClient::Stop` and `Reset` under the mixer lock (`lib.rs:499-501`), truncates the source, and wakes the queue thread. Two wake-ups, then the device stops; at most the engine's one period already mixed is still heard. Queued audio does not delay the cut: Reset discards the device's 40 ms, the source's own 40 ms is truncated, and audio still in the host pipe and channel is read and discarded. What queued audio does delay is the next utterance (finding 4).

### 2.5 NVDA, for comparison

NVDA is one process. A UIA focus reaches its main thread with one wake-up (an immediate pump; `nvda/source/core.py:1180-1199`), its speech reaches the eSpeak background thread with one more, and that thread writes straight into WASAPI from the synthesis callback, with no mixer thread. About three wake-ups from event to device, against Verbatim's 15 after the relay step. NVDA is worse in three places: it delays every WinEvent other than focus by a 10 ms timer pump (`core.py:55`, `IAccessibleHandler/internalWinEventHandler.py:193`), its eSpeak buffer is 300 ms (`synthDrivers/_espeak.py:371-373`), so its first chunk takes longer to synthesize than Verbatim's 60 ms one, and its key cancel runs on the main thread behind earlier queued work (`docs/nvda/input.md`, "What a key press does to speech"). It is better in one: it keeps the device awake for 30 seconds after speech (`nvwave.py:252-258`, `364-366`). It sets no priorities.

## 3. Findings, ranked by expected time saved for one focus change or one key echo

### 1. The audio stream is stopped after every utterance (decided, not built)

- Established: when the device's queue empties, the audio thread calls `Stop` and `Reset` (`mixer.rs:1067-1068`), so every announcement that follows a silence starts a stopped stream (`mixer.rs:1051-1052`). The 2026-10-08 decision to keep the stream awake, writing silence, for 30 seconds (`roadmap.md:52-58`) has no code: no "awake" setting or silence writer exists in `verbatim-audio` or `verbatim-audio-wasapi`.
- Cost: not measured, since no run uses a real device. Estimated from NVDA's reason for the setting: Bluetooth, USB, HDMI and virtual-machine endpoints clip or delay the next speech by tens to hundreds of milliseconds when the stream restarts; an internal codec that another application keeps running costs about one engine period.
- On the path for: every announcement that follows a pause, which is most focus changes and most echoes.
- Fix: build the decided awake period, writing silence one period at a time so that new speech never queues behind more than one period of silence.
- Belongs: separately (audio work), already on the M4 list.

### 2. The shared-mode engine period sits under every first sample

- Established: the stream uses `IAudioClient::Initialize` in shared mode with a 40 ms buffer (`wasapi/src/lib.rs:41`, `338-348`), so the engine's default period, normally 10 ms, applies. Inferred: the first sample is heard up to one period after `Start`, about 5 ms on average, plus the endpoint's own delay.
- Cost: estimated 3 to 7 ms on every utterance where the driver supports a shorter shared period through `IAudioClient3::InitializeSharedAudioStream` (often about 3 ms). Not measured; `docs/design/phase6-design.md:2624` already notes that `IAudioClock` would show it.
- NVDA uses the same default period, so this would be better than NVDA, not a regression to fix.
- Fix: measure first, with a real-device scenario that records `IAudioClock` position against the write time; then try the minimum shared period with the stream kept awake.
- Belongs: separately.

### 3. No thread or process opts out of throttling or raises priority, and the relay plan covers only the forward path

- Established: no `SetThreadPriority`, `SetProcessInformation` or `SetThreadInformation` exists in the workspace. `relay.md` 4.4 raises only the relay thread, the listener's writer and event threads, and the outposts' pipe readers, and opts out of EcoQoS in verbatim.exe, the listener and the outposts. The 2026-10-08 decision (`roadmap.md:60-62`) also covers the hook thread (high) and the speech queue and synthesis threads (above normal).
- Measured: the return path and the speech path have tails at least as large as the relay's. "To Core" has 5.3 percent over 2 ms. The synthesis stage, one thread doing microseconds of work with no hand-off inside it, has 5.2 percent over 2 ms and a 48.7 ms maximum: a running thread that was pre-empted, which only priority addresses.
- Missing from the plan, on every event or key: the outpost's outbound writer and its WinEvent thread; verbatim.exe's outpost reader threads and the reducer; the speech queue, synth and host reader threads; the keyboard hook thread, whose delay also holds every key back from every application; and the synthesizer host process and both its threads. The host has no window, so on a hybrid processor Windows can run eSpeak NG on an efficiency core at a low clock. Estimated: the first chunk's synthesis takes about twice as long there. Not measured.
- Fix: extend the relay step's priority list and EcoQoS opt-out to these threads and to the synthesizer host process, using the levels the 2026-10-08 decision gave and the floor benchmark's choice for the rest.
- Belongs: the relay step. It is the same mechanism and the same measurement, and leaving the return path out would leave half of every round trip unprotected.

### 4. New speech waits for the cancelled job to finish in the host

- Established: a cancel sets the job's flag but leaves `in_flight` set (`manager.rs:906-917`, `1008-1016`), and `pump` will not start the next job until `SynthFinished` arrives (`manager.rs:830-833`, `973-976`). That arrives only after the host has seen the cancel, finished its current chunk, and sent `Done` (`verbatim-synth-hosted/src/lib.rs:378`). Mixer backpressure (`AHEAD_MS`, `mixer.rs:48`; the 4 KiB host pipe, `lib.rs:53`; the two-slot channel, `lib.rs:165`) paces synthesis to playback. So a focus announcement of a second or two is still in flight for nearly all of its playback.
- Cost, estimated from the code: about ten wake-ups across two processes (the synth thread, the host reader, the host main thread, the verbatim.exe host reader draining discarded PCM, then `Done`, the synth thread, the queue thread, the synth thread), plus up to one eSpeak chunk of CPU. Probably 1 to 3 ms at the median, and each of the ten hops adds its own share to the tail. For a focus change, part of this runs while the new event is still being read, so it is partly hidden. For a typed echo or a review command, whose speech arrives within about 0.3 ms of the cancel, none of it is hidden.
- On the path for: every key that interrupts an utterance whose synthesis is still running. Short echoes usually finish synthesis at once and do not wait.
- Fix: tag every host message with its utterance and let the synth thread send `Speak` for the next utterance right behind `Cancel`, discarding late audio and `Done` for the old one instead of waiting for them.
- Belongs: separately (verbatim-speech and the host protocol).

### 5. Cancellation polls on a 10 ms timer before any audio, twice with OneCore

- Established: while the host has sent nothing, verbatim.exe's synth thread checks for a cancel only when a 10 ms wait times out (`verbatim-synth-hosted/src/lib.rs:49`, `338-356`). OneCore produces no audio until the whole utterance is synthesized, and its driver in the host also checks for a cancel only every 10 ms (`crates/verbatim-synth-onecore/src/lib.rs:52`, `366-380`).
- Cost: with OneCore, a key pressed during synthesis waits up to 10 ms for the first check and up to 10 ms more for the second, each rounded up to the 15.6 ms timer tick, before the host stops. Finding 4 then holds the next utterance behind that. Estimated at up to about 30 ms, half that on average. With eSpeak NG this applies only during its first few milliseconds.
- Fix: have the synth thread wait on its reply channel and a cancel channel together; in the host, signal the cancel through an event that the OneCore wait (`WaitForMultipleObjects` on completion and cancel) and the eSpeak callback both see, so cancellation needs no timer and no thread wake in the host.
- Belongs: separately, with finding 4.

### 6. Speech to earcon re-mix (decided, not built)

- Established: speech that starts while an earcon is playing is appended after the 40 ms already handed to the device (`mixer.rs:1082-1101`). The decided re-mix (`roadmap.md:54-58`) has no code: the only requests that discard the device's queue are cancel, pause and fail (`mixer.rs:152-165`).
- Measured in `docs/design/phase6-design.md:2617`: 35 to 40 ms for such announcements. The pooled "mixer and device" figures above are consistent with it, mixed with the measurement artefact.
- On the path for: announcements with a sound, which depends on the theme. Most focus changes in the default theme have none (inferred).
- Belongs: separately, already on the M4 list.

### 7. Three thread hops on the speech path that add no isolation

- Established from the code: the speech queue thread hands each job to a separate synth thread (`manager.rs:1000-1002`, `1049-1056`). In the host, a reader thread hands each request to the main thread (`synth-host/src/main.rs:96-119`). In verbatim.exe, a host reader thread hands each PCM frame to the synth thread (`synth-hosted/src/lib.rs:165-185`). None of these crosses a process boundary.
- Cost: three wake-ups on every announcement and every echo, before the first sample. Estimated at about 0.2 ms at the median from the measured 0.06 ms per hand-off, and each of the three carries the measured 0.24 to 0.99 percent chance of more than 2 ms.
- Fix: the synth thread reads the host's pipe itself with overlapped I/O and waits on it, the cancel and its command queue together; that also removes the 10 ms poll. The host's main thread reads its own command pipe, with cancellation coming by the event in finding 5. The queue logic runs under a mutex on the thread that speaks, so the reducer's `speak` wakes only the synth thread. The cancel from the hook must stay non-blocking, so `cancel_all` needs a variant that does not wait for the audio thread.
- `relay.md` 4.1 says the host's pipe can stay anonymous. This fix needs it overlapped, so it should use the same pipe constructor.
- Belongs: separately, after the relay step, reusing its overlapped-pipe module.

### 8. The return path keeps a writer-thread hop and two writes per message

- Established: the outpost worker publishes into a queue that a separate writer thread drains (`outbound.rs:318-326`, `369-415`), with two `WriteFile` calls per message (`protocol.rs:876-878`). verbatim.exe's reader thread then hands the message to the reducer (`owner.rs:1112`). Measured: "to Core" has a median of 0.3 ms, a 99th percentile of 5.3 ms, and 5.3 percent over 2 ms. That is a larger tail than any single relay hop.
- `relay.md` 4.3 already makes `write_message` one write, which covers this direction too. The writer hop remains: the worker could issue an overlapped write itself when nothing is queued ahead of it and no write is pending, waking the writer only to finish a pending one. That keeps "queuing never waits" (`outbound.rs:1-27`). Saves one wake-up per event and per reply.
- The reducer's own wake-up stays, as `relay.md` says.
- Belongs: the single write and priorities in the relay step (already planned or finding 3); the direct write separately.

### 9. Core's requests still take a hand-off to the relay thread

- Established from `relay.md` 4.1 and 4.5: under the new design, the reducer's `send_to_outpost` (`crates/verbatim-outpost/src/supervisor/mod.rs:305-321`) posts a completion packet, and the relay thread wakes to write. That puts one wake-up in front of every caret-key watch, text read, navigation fetch and activation.
- Fix: whichever thread pushes onto a child with no write in flight issues the overlapped `WriteFile` itself, under the child's write-state lock, and the completion goes to the port as usual. Only the relay thread's own sends need no post at all, as planned.
- Cost: one wake-up, about 0.06 ms at the median plus its tail, on every request. The application processes the key at the same time, so this is often hidden.
- Belongs: the relay step, since it changes the same write-state design.

### 10. Bound gestures pass through the router thread, which logs each one

- Established: the hook sends every bound gesture to the router (`input-windows/src/lib.rs:470`), which logs it at info level to verbatim.exe's stdout under the process-wide stdout lock (`main.rs:1723`) and then forwards review and navigation commands to the reducer. Caret keys and typed text go straight to the reducer.
- Cost: one wake-up and one synchronous console or file write on every review or navigation key. Estimated at 0.1 to 0.5 ms, and more when stdout is a console.
- Fix: send review commands from the hook straight onto the reducer's channel, keeping the router for lock keys, GUI commands and the time; move the gesture log line to debug or after the forward. The ordering of `Routed::Handled` marks behind lock-key announcements must be kept.
- Belongs: separately.

### 11. The audio thread holds the mixer lock across WASAPI calls, and logs on its thread

- Established: `GetCurrentPadding`, `Stop`, `Reset`, `GetBuffer`, `ReleaseBuffer` and `Start` all run while the mixer's state lock is held (`mixer.rs:986-1069`), and so does the underrun warning (`mixer.rs:1042-1047`). The synth thread's append and the queue thread's register wait on that lock. After each first sample, the audio thread builds the latency line and writes it at info level to stdout (`crates/verbatim-app/src/latency.rs:489-504`), and takes the control plane's registry lock to broadcast.
- Cost: microseconds when the driver is quick. Estimated at milliseconds with slow drivers or a console stdout; a cancel arriving meanwhile waits.
- Fix: do the device calls with the lock released, as `recover` already does for opening (`mixer.rs:1279-1323`); send the latency line to a logging thread, or format it on the reducer's side.
- Belongs: separately.

### 12. Smaller per-event costs in the outpost and in verbatim.exe

- Every outpost entry wakes the watchdog thread (`worker.rs:187-195`) and formats a description string used only by a debug log (`worker.rs:682`). Remedy: arm a thread-pool timer for the deadline instead of waking a thread, and format the description only when debug is enabled.
- Debug logging inside the intake lock (`intake.rs:306-313`, `340-349`), already listed in `relay.md` section 10. It applies only to test runs, but it inflates every measured outpost stage.
- After almost every focus change, `send_views` sends `NodesHeld` (`main.rs:1409-1453`). That is three more wake-ups and an intake entry, measured at a 29 µs median in one conhost log, which the next event can queue behind. Coalescing it with the next request, or sending it after a short idle, would remove the traffic. Done in communication step 11 (c65b6e95): Core leaves a changed list in a slot in the outpost's writer, and it goes in the same write as Core's next command to that outpost, after the command, or with the relay's next ping, so it never costs a write of its own and is at most one ping interval (3 seconds) late; a change in the acknowledged position counts as a change, so the resend every 256 messages is gone too.
- JSON is serialized and parsed at every pipe, and PCM is copied about ten times between eSpeak NG and the device. Each costs microseconds in release. `relay.md` 4.2 already defers the binary framing.
- Belongs: separately, low priority.

## 4. What is deliberate, and not a defect

- The listener waits 50 ms after a menu closes before reporting it (`listener.rs:100`), as NVDA's fake focus does. It is used only when no focus event follows.
- Lock-key announcements wait 30 ms (`main.rs:1815`).
- Mixing starts on a stopped device only once 10 ms of audio is ready (`mixer.rs:55`), to avoid a click. eSpeak NG's 60 ms first chunk and OneCore's 50 ms pushes both exceed it, so it costs nothing in the common case.
- Cancellation keeps `Stop` and `Reset`, as decided on 2026-10-08 and as NVDA does.
- OneCore's whole-utterance synthesis before any audio is a WinRT limitation that NVDA shares.
- The process boundaries (D9, D13, D18) account for the rest of the gap with NVDA. With the relay step and findings 7 to 10, a UIA focus would take about 11 wake-ups against NVDA's three, and an echo about 5 against NVDA's two.

## 5. Established and inferred

Established, from the code or from recorded figures:
- The hop counts in section 2.
- The awake period and the earcon re-mix are not built.
- No priority or EcoQoS calls exist.
- New speech waits for `SynthFinished`.
- The cancel checks run on a 10 ms timer.
- The writer-thread hops, the router's log line, and the lock held across WASAPI calls.
- The per-stage figures in section 1.

Inferred, not measured:
- Every cost attached to a real device or to the synthesizer host, because no run uses either.
- The shared-period saving.
- The efficiency-core slowdown.
- The size of the round trip in finding 4.
- That the 5 percent tail of the synthesis stage is pre-emption. It is one thread with no lock in the stamped span, but no ETW trace confirms it.

The first thing to build, before any of findings 1, 2, 4, 5 or 7, is a measure scenario that runs eSpeak NG through the host and a real WASAPI device, with the ledger's synthesis stage split into the host's request, its first chunk, and its return, and the device stage read from `IAudioClock`.
