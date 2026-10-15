# verbatim.exe's relay: design for stage A step 9

Status: superseded by `docs/design/communication.md`, which subsumed it after its approval on 2026-10-11; kept for the record.

Approved by Dickson on 2026-10-11, with both decisions in section "Decisions" as recommended. Researched on main at 3804050. Sources: the code cited by file and line; `scratchpad\supervisor-latency.md` and its scripts and classified output in `scratchpad\sup-lat` (measured on the measure-timing branch, whose stamps are not on main); `docs/design/focus-pipeline.md` 12.4 and 15; `docs/roadmap.md`; `docs/design/phase6-design.md`; `docs/performance.md`; `docs/architecture.md` D9 and D13; NVDA's `source/core.py`.

## 1. Summary

- The 36 ms 90th percentile and the 146 ms maximum are not the relay. They come from facts that waited for an outpost process to start: they were held, or they started the outpost. That is established in `supervisor-latency.md` from the per-fact decisions. Step 8, the reserved spare outposts, removes that wait. Step 9 should not try to.
- Facts relayed to an outpost that was already ready measured a median of 0.18 ms in release (0.39 to 0.41 ms in debug), a 99th percentile of 2.4 to 4.7 ms, and a maximum of 11 to 51 ms. This tail is what step 9 must remove. On every slow relay the owner thread had no work logged, and by the code it had no blocking work to do. The delay is threads that were made runnable but did not run.
- Today a fact passes through three threads in verbatim.exe, so it needs three wake-ups there: the listener reader woken by the pipe, the owner woken by a crossbeam channel, and the outpost writer woken by a condition variable. Measured, each hop has its own tail: 0.24, 0.99 and 0.47 percent of hand-offs over 2 ms.
- Proposed: one thread in verbatim.exe does the whole relay. It is the owner thread itself, rebuilt on an I/O completion port. It reads the listener's pipe with overlapped I/O, routes the fact where the read completes, and writes to the outpost's pipe with overlapped I/O, in the same pass. That leaves one wake-up in verbatim.exe, the one any process in the path must pay. The owner already does no blocking work, so making it the relay thread keeps every existing ordering argument, because all of them rest on one thread doing the routing. The thread runs at raised priority with power throttling turned off, which was decided on 2026-10-08 but never built. Nothing waits on a timer. The power plan stays Balanced, and no new route is added between the listener and the outposts.
- The latency ledger gains a verbatim.exe stage in each direction. The relay's own time is measured on one clock in one process and travels as a duration, so it does not depend on clocks agreeing across processes.
- The platform's floor is measured by a three-process ping benchmark under Balanced, with the same pipes and gaps between messages that leave threads idle, as events do. ETW ready-thread tracing attributes each slow sample to its cause. On this Proxmox VM the floor may itself exceed 2 ms at its maximum: an in-process hand-off in the listener, which never touches Core, already shows a 13 to 32 ms maximum. If it does, the 1 to 2 ms budget can only mean what verbatim.exe adds beyond the floor. That needs Dickson's decision (section 9).

## 2. The relay path today, exactly

From the listener's callback to the outpost's intake. Wake-ups are numbered W1 to W5.

### In the listener process (not verbatim.exe, but on the relay's path)

1. The WinEvent or UIA callback thread queues the fact: `Outgoing::fact`, `crates/verbatim-outpost/src/listener.rs:143`. This takes the `Outgoing` mutex (line 165), replaces any waiting fact with the same key, and calls `notify_one` on the condition variable (line 181).
2. W1: the `verbatim-listener-outbound` thread (`listener.rs:313-322`) wakes in `Outgoing::next` (`listener.rs:231-262`). That function waits on the condition variable, or with `wait_timeout` only while a menu-end grace is pending (lines 256-260).
3. The thread serializes the message with `write_message` (`crates/verbatim-outpost/src/protocol.rs:864-879`). `serde_json::to_string` allocates a String. Then come `write_all(line)`, `write_all(b"\n")` and `flush`. The listener's pipe is a bare `File` with no buffer: `crates/verbatim-outpost/src/main.rs:99` passes `Box::new(writer)` from `verbatim_process::inherited_pipes` (`crates/verbatim-process/src/lib.rs:560-585`). So each message costs two `WriteFile` calls, one for the JSON and one for the line feed. This is established from the code. The outposts write to Core the same way (`main.rs:68-73`, `outpost/outbound.rs:390`).

### In verbatim.exe

4. The pipe is anonymous, made by `CreatePipe` (`crates/verbatim-process/src/lib.rs:214-252`) with the default buffer (`supervisor/process.rs:99`, `from_child_buffer: 0`). Anonymous pipes cannot do overlapped I/O, so every reader blocks in `ReadFile`. (Since communication steps 4 to 9, every pipe between Core and a child is made by `overlapped::pipe`, and `CreatePipe`, `launch` and `from_child_buffer` are gone from `verbatim-process`.)
5. W2: the `verbatim-listener-reader` thread (`supervisor/owner.rs:1126-1171`) returns from `ReadFile` inside `read_message` (`protocol.rs:889-903`), behind an 8 KiB `BufReader`. Each message allocates a fresh line `Vec` (`read_until`, which grows it as it reads). Because the listener writes the line feed separately, the first `ReadFile` can return the JSON without its line feed. The reader then blocks in a second `ReadFile` until the line feed arrives, which can mean a second wake-up for one message. That is established from the code; how often it happens is not measured. Next, `serde_json::from_slice` allocates every string in the fact (a UIA snapshot carries names, values and a runtime id). The message becomes an `OwnerEvent::Fact` (lines 1130-1142) and is sent on the unbounded crossbeam channel (line 1166), which unparks the owner if it is parked.
6. W3: the `verbatim-supervisor` owner thread (`owner.rs:206-225`) wakes in `select!`, which waits on that channel and on two `tick` timers, 3 s and 30 s. Crossbeam parks it with a timeout set to the next tick, and a send unparks it. Nothing polls. Then:
   - `handle` (line 256) calls `route_fact` (lines 645-672).
   - `route_fact` scans the ignored list linearly without a lock (`ignored.rs:67-69`), may reset the crash history, and looks up the `records` HashMap.
   - `deliver` (lines 991-1003) builds `SupervisorToOutpost::DeliverFact` by moves and computes `fact.key()`.
   - `WriterHandle::push` (`writer.rs:155-165`) takes the child's queue mutex and runs `Queue::push` (lines 101-139), which scans at most 64 entries and pushes onto a VecDeque. It then calls `notify_one`. The reducer thread shares that mutex through `send_to_outpost` and `send_nodes_held` (`supervisor/mod.rs:305-347`), but holds it only briefly. (Since communication step 11, `send_nodes_held` is gone: the reducer's `leave_nodes_held` leaves the list in the writer's slot, and the next command written to that outpost carries it.)
7. W4: the per-child `verbatim-outpost-writer` thread (`writer.rs:179-212`) wakes from its condition variable and pops the entry. It writes with `write_message` into a `BufWriter`: one String allocation, and one `WriteFile` for a fact under 8 KiB. `flush` is then called a second time, which does nothing.

### In the outpost process

8. W5: the outpost's main thread is the pipe reader. In `run_pipe` (`crates/verbatim-outpost/src/outpost/mod.rs:1063-1087`) it returns from `ReadFile` and parses the message, again with a line `Vec` and serde allocations.
9. `handle_command` (`outpost/mod.rs:801-816`) stamps `relayed_at_us = now_us()`, which is wall-clock time (`SystemTime`, `protocol.rs:53-61`). Then `take_in_routed_fact` (lines 481-504) runs the hand-over check under the hand-over lock and calls `Intake::push` (`outpost/intake.rs:289-322`). Inside the intake lock, `push` logs `tracing::debug!("intake pushed")` (lines 306-313). The end-to-end suite runs outposts with `verbatim_outpost=debug` (`crates/verbatim-e2e/src/scenario.rs:157`), so during tests that log line is a synchronous write to stderr, which goes to the outpost's log file, made while the intake lock is held. It then calls `notify_one` (line 322) to wake the worker, which is past the relay.

### Counts

Inside verbatim.exe there are three thread wake-ups (W2, W3, W4), sometimes four when the line feed arrives late. There are two queues: the crossbeam channel and the child's VecDeque. There are two locks: the queue mutex, and the crossbeam channel's internal lock. Each message is serialized and deserialized once with JSON, takes about 5 to 20 heap allocations depending on the fact's strings, and costs these syscalls: one or two `ReadFile`, one unpark, one condition-variable wake, and one `WriteFile`. From the listener's callback to the outpost's intake there are five wake-ups in total.

The ledger cannot see verbatim.exe's share of this today. Its `ListenerToOutpost` stage (`crates/verbatim-app/src/latency.rs:128-131`) runs from the listener's `observed_at_us` to the outpost's `relayed_at_us`. That span includes the listener's queue, both pipes and the whole of verbatim.exe.

## 3. Causes of the tail

### Established, from measurement and code

- The 90th percentile and maximum of the pooled relay measure the outpost's start: a median wait of 31 to 40 ms, and 100 to 146 ms when two launches overlap. Section 1 of `supervisor-latency.md` gives the per-fact decision counts. The 146 ms case is a Foreground fact in measure_keys_conhost whose launch took 112 ms on the helper thread while another launch ran at the same time.
- Facts delivered at once still had a millisecond tail on every hop. In release, the share of hand-offs over 2 ms was: listener writer to Core reader, 0.24 percent; Core reader to owner, 0.99 percent; owner to outpost reader, 0.47 percent. The listener's own in-process hand-off, with no Core involved, was 0.48 percent.
- The owner never blocks on the relay path. Launches run on `verbatim-launch` threads (`owner.rs:485-535`), endings on `verbatim-retire` threads (lines 762-776), and pipe writes on the writer threads. Process liveness checks are microsecond calls (`process.rs:144-192`). The heartbeat and sweep run every 3 s and 30 s. `application_is_hung` (EnumWindows, `process.rs:199-204`) runs only for an outpost with eight or more abandoned workers. Every slow relay that was classified had zero cases of owner launch, retire, pipe-write or logged-work.
- Bursts route in about 10 µs per fact once the owner is running. Logging rarely pre-empts a running thread: the QPC-to-log-time gap is over 1 ms in only 0.05 to 0.09 percent of lines. So the threads are late to start, not slow once they run.
- Among the slow delivered relays, 32 of 91 in debug and 37 of 70 in release were whole-machine silences, where no process logged anything for at least half the relay. The rest were wake-up delays while other threads ran, plus 3 to 5 per set during another outpost's launch.
- The machine is a Proxmox (QEMU/KVM) guest with 12 vCPUs on Balanced.
- No Verbatim thread sets a priority or opts out of power throttling. The only priority call in the workspace is the audio thread's MMCSS "Pro Audio" (`crates/verbatim-audio-wasapi/src/lib.rs:261`). The 2026-10-08 decision (`docs/design/phase6-design.md:2622-2626`, `docs/roadmap.md:60-62`) that every process opts out of EcoQoS, with raised thread priorities, is not implemented.

### Inferred: likely, but not yet proven case by case

- Whole-machine silences are most likely the hypervisor not running the guest's vCPUs, through host-side scheduling or a halted vCPU slow to wake. No guest-side design can remove them. Only the floor benchmark and ETW, compared against bare metal, can confirm this.
- The other slow wake-ups are most likely a thread made ready at normal priority that cannot pre-empt another normal-priority thread on the processor the scheduler chose. That happens when there is no idle processor, or when the chosen one is parked or slow to leave a deep idle state under Balanced. A ready thread of equal priority waits for the running thread's quantum, which on client Windows is about 2 clock ticks (about 31 ms), and longer for the foreground application's threads, whose quantum is stretched. That fits waits in the 2 to 30 ms range. A thread with higher priority pre-empts at once. This is the part that priority can address.
- Every hop collects the platform's wake-up tail once, so fewer hops in verbatim.exe should lower the share of relays over 2 ms roughly in proportion. Removing W3 and W4 removes the hops that measured 0.99 percent and part of 0.47 percent.
- Timer resolution is not a direct cause. No relay step waits on a timer: pipe reads, channel unparks and condition variables all wake on the event itself. Timer resolution affects the menu-end grace wait in the listener, which is not a fact relay, and the owner's heartbeat ticks. It does not shorten the wait of a same-priority ready thread, which is set by the quantum. The floor benchmark should test this, not assume it.
- The two-`WriteFile` framing can add a wake-up on the Core reader for a single message (W2 twice). It is unlikely to dominate, but it is free to remove.
- In tests, logging under the outpost's intake lock (`intake.rs:306-313`) puts a synchronous file write inside the outpost's receipt of the fact. This is on the outpost's side, after `relayed_at_us`, so it is not counted as relay time, but it lengthens the intake hand-off. It is listed in the follow-ups.

## 4. Proposed design

### 4.1 One relay thread: the owner on an I/O completion port

The owner thread becomes the relay. It waits only in `GetQueuedCompletionStatusEx` on one completion port, with a timeout equal to the time until the next heartbeat or sweep.

- Reads from the listener. Core's end of the listener's out-pipe gets a pending overlapped `ReadFile` into a reused 64 KiB buffer. When the read completes, the thread splits the bytes into lines, handling partial lines and several lines per read, and parses each one. It routes each fact with the existing `route_fact` in the same pass. It then issues the next read. `ForwardingEnded`, `MenuOrSwitchEnded`, `Ready` and pongs from the listener are handled in that same pass. So the listener's messages are taken in order, by the thread that routes them, which keeps the hand-over's requirement that "forwarding ended" comes after every earlier fact (`writer.rs:16-20`, `focus-pipeline.md` 12.4).
- Writes to every child. The per-child writer threads go. `WriterHandle` keeps its API: `push`, `close`, the `Queue` overload policy, and the 64-entry bound. Its condition variable is replaced by a wake of the relay thread. When the relay thread pushes, it notes the child as having work and writes before it waits again, with no wake at all. When another thread pushes (the reducer's `send_to_outpost` and `send_nodes_held`, a retire's `Shutdown`; communication step 11 later replaced `send_nodes_held` with a list carried by the next write), it posts one `PostQueuedCompletionStatus` packet with the child's key. Each child has at most one overlapped `WriteFile` in flight, from a buffer that is reused and kept alive until the write completes. When a write completes, the next queued entry is written. A stuck outpost's write simply stays pending while its queue applies the existing replace and drop policy, so it never blocks the thread or any other child. This is what the writer threads guarantee today.
- Every other event the owner gets today (from launch helpers, outpost readers, and the app's `Views`, `EnsureSpawned` and `Shutdown`) stays on the crossbeam channel. Each send is followed by a post to the completion port with an "owner events" key, and the thread drains the channel when that packet arrives. The heartbeat and sweep run when `GetQueuedCompletionStatusEx` times out, so the `tick` channels go.
- Outposts' messages to Core keep their reader threads (`owner.rs:1069-1122`). That direction carries messages of up to 32 MB (terminal output, `protocol.rs:854`). Parsing one on the relay thread would block listener facts behind it for tens of milliseconds. A reader thread followed by the reducer's channel is already a single hand-off. The reducer is the consumer, so verbatim.exe cannot remove that wake-up.
- The relay thread logs nothing on the delivered path. It already logs only failures there. Measurement stamps go to an in-memory ring (section 4.5), never to stdout, whose lock every Core thread shares.
- The owner's blocking work is moved off this thread, so it never delays a fact.
  - `application_is_hung` moves to a helper thread that reports back. It is the only owner call that can take milliseconds.
  - `thread::Builder::spawn` for launch helpers and retire threads stays. It takes tens to hundreds of microseconds and happens only on lifecycle changes. The floor run should measure it. If it shows up, a persistent launch-helper thread fed by a channel removes it.
  - `Target::open` stays. It takes microseconds.

Result: from the listener's `WriteFile` to the outpost's `ReadFile` there is one wake-up in verbatim.exe. Any process in the path costs at least that. The relay's own work runs on a thread that is already running.

Pipes. `CreatePipe` handles cannot be used for overlapped I/O. Core's ends become named pipes, created the way Rust's standard library makes its overlapped anonymous pipes:
- `CreateNamedPipeW` on a unique name (`\\.\pipe\verbatim-<core pid>-<outpost id>-<random>`), with `FILE_FLAG_OVERLAPPED`, `FILE_FLAG_FIRST_PIPE_INSTANCE`, `PIPE_REJECT_REMOTE_CLIENTS`, byte mode, and a DACL that admits only the current user's SID. The DACL should match today's handle security; the risks section covers integrity levels.
- The child's end is opened at once with `CreateFileW`, synchronous and inheritable.
- `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` and `FILE_SKIP_SET_EVENT_ON_HANDLE` are set on Core's ends, so a write that completes at once returns without a trip through the port.

This belongs in `verbatim-process`, a Windows crate. The children keep synchronous handles, and `inherited_pipes` already accepts any `FILE_TYPE_PIPE`, so the listener and outposts do not change. The pipe for the synthesizer host can stay anonymous.

### 4.2 Alternatives considered

- A relay thread separate from the owner, with routing and held facts split between them. It is not recommended. When the outpost becomes ready, a fact the relay passed to the owner just before the switch can be overtaken by one the relay delivers directly just after it, unless a barrier protocol is added. The relay would have to own the held facts and the ready state, which moves most of the owner into it anyway. Since the owner never blocks (section 3), merging them is simpler and keeps today's proofs of ordering.
- Forwarding bytes without parsing. The listener would put the target pid and the `FactKey` in a fixed binary header ahead of a payload already shaped as the outpost's `DeliverFact`, and Core would copy the payload without deserializing it. This removes two JSON passes and most allocations: tens of microseconds in debug, less in release. The tail is wake-ups, not CPU, so this is optional. Do it only if the verbatim.exe stage's maximum, measured after 4.1, is dominated by parsing.
- Direct pipes from the listener to each outpost. Dickson ruled this out on 2026-10-10. It is not proposed.
- MMCSS for the relay thread. It is not recommended. MMCSS schedules periodic multimedia work. Registered threads run at real-time priorities only within the share that `SystemResponsiveness` allows, and are demoted when they exceed it. It is tied to audio and network throttling policy. It suits the WASAPI thread, which uses it, and not an event relay. A fixed raised priority does the same job for a thread that runs for microseconds per message.

### 4.3 One write per message

`write_message` should build the line and its line feed in one buffer and make one `write_all`. That gives one `WriteFile` on a bare `File` (listener and outposts) as well as through Core's `BufWriter`. This removes the split read in 2.5 and the second wake-up it can cause, and it costs nothing.

### 4.4 Priority, power throttling, and timer resolution under Balanced

- Thread priority. Set `SetThreadPriority(THREAD_PRIORITY_HIGHEST)` (base 10 in the normal class) on:
  - verbatim.exe's relay thread;
  - the listener's outbound writer and its event-delivering threads;
  - each outpost's pipe-reading main thread.

  A ready thread at 10 pre-empts the normal-priority threads (8, or 9 with the foreground boost) of whatever application is busy, which is the inferred cause of most non-silent tails. It costs no power, because priority changes which thread runs, not whether a processor wakes. The risk is starving others if the thread spins. These threads do microseconds of work per message and block between messages. The floor benchmark should compare `ABOVE_NORMAL`, `HIGHEST` and `TIME_CRITICAL` (15) and choose the lowest that meets the budget. Dynamic priority boosts stay enabled.
- Power throttling. Implement the 2026-10-08 decision as part of this step. Call `SetProcessInformation(ProcessPowerThrottling)` with `PROCESS_POWER_THROTTLING_EXECUTION_SPEED` in the control mask and a zero state mask, for verbatim.exe, the listener and every outpost. Those processes have no visible window, which is exactly what makes Windows treat them as background candidates for EcoQoS. Also set `SetThreadInformation(ThreadPowerThrottling)` on the relay thread. This is a per-process quality-of-service opt-out, not a change of power plan, so Balanced stays. Its cost is that on hybrid processors these threads are not confined to efficiency cores and run at full clock. The relay's CPU time is microseconds per fact, so the energy cost is negligible. It matters little on the dev VM and more on laptops and ARM64.
- Timer resolution. Do not raise it. Nothing on the redesigned path waits on a timer: the only timeouts left are the heartbeat and sweep deadlines in `GetQueuedCompletionStatusEx`. Raising resolution system-wide raises power use under Balanced, which the constraint rules out in spirit. Windows 11 also ignores the request from windowless processes unless they opt out with `PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION`. The floor benchmark measures it once (section 6) so that this is a measurement, not an assumption.

### 4.5 The ledger stage

New fields in `EventTiming` (`protocol.rs:23-48`, serde default 0, so older peers still read):
- `core_received_at_us`: when verbatim.exe's relay thread took the read's completion off the port. This is the first instruction after the wake. It is wall-clock time, like the other stamps, so it can be compared with the listener's `observed_at_us`.
- `core_relay_us`: the relay's own time as a duration. It is measured from the completion being dequeued to the moment just before `WriteFile`, both read with `Instant` (QPC) on the same thread in the same process. It needs no clock agreement between processes. It deliberately excludes the write syscall, which belongs to the next hop. For a fact that waited in the child's queue behind a pending write, the time spent waiting there is included, because that is verbatim.exe's time too.
- `core_sent_at_us`: `core_received_at_us` plus `core_relay_us`, so the cross-process spans can be computed.

New ledger stages, replacing `ListenerToOutpost` (`latency.rs:128-131`) when the fields are present:
- "listener to verbatim.exe": `observed_at_us` to `core_received_at_us`. It covers the listener's queue and writer, the pipe, and the relay thread's wake.
- "verbatim.exe": `core_relay_us`.
- "verbatim.exe to outpost": `core_sent_at_us` to `relayed_at_us`. It covers the pipe and the outpost reader's wake.

For a request Core makes, the "Core to outpost" stage gains the same split. The reducer's push to the child's queue is stamped, and the relay thread's write is stamped, which gives a "verbatim.exe" stage for requests. The interval between them is the post and the wake.

In the other direction, `read_outpost` stamps the `ReadFile` return in `OutpostMessage::Event`. "To Core" (`latency.rs:158-162`) is split into "outpost to verbatim.exe" (`published_at_us` to the read's return) and "waiting for the reducer" (the read's return to `event_received`'s clock, `latency.rs:338-349`). The reader's own parse time is folded into the first or reported with the second. The second stage is the reducer being busy, not the relay, and it is labelled that way so it is never mistaken for verbatim.exe's time.

Clocks. Cross-process spans keep `now_us`, `GetSystemTimePreciseAsFileTime`, which is derived from QPC and is the same in every process. A wall-clock adjustment during a run would show up as a negative or impossible span. Durations that verbatim.exe owns use `Instant` alone, so its stage is immune. If wall-clock skew ever shows in the cross-process stages, a QPC microsecond clock (`QueryPerformanceCounter` scaled by `QueryPerformanceFrequency`, which Windows documents as system-wide) can replace `now_us` for those stages. That change belongs to the outpost crate and the app, both Windows-specific, so the NVDA provenance rule is not affected.

Measurement builds also record each hop's QPC stamps into a fixed-size in-memory ring per process, written out at exit. The synchronous per-hop log writes the measure-timing branch used are dropped, because they put a file write and the stdout lock on the measured path. The ledger's 2 ms warning and failing a scenario over it stay deferred to the response-time budgets, as `roadmap.md:562-564` records.

## 5. NVDA

NVDA is one process, so there is no relay to compare with, and most of this does not apply. Its nearest equivalent is the hand-off from the COM or WinEvent callback thread to the main thread. `core.requestPump` (`nvda/source/core.py:1180-1199`) posts an immediate pump with `wx.CallAfter`, which is one window-message wake-up, for focus and input. Other events get a delayed pump through a wx timer of `PUMP_MAX_DELAY` = 10 ms (`core.py:55`, `1013`), which at the default timer resolution fires on a roughly 15.6 ms tick. NVDA sets no thread priority, process priority or timer resolution (no `SetThreadPriority`, `SetPriorityClass` or `timeBeginPeriod` anywhere under `nvda/source`). After this step, Verbatim's path from the listener to the outpost has one wake-up in the listener, one in verbatim.exe and one in the outpost. NVDA's immediate path has one. The extra two are the price of D9 and D13's isolation, and the floor measurement shows what each one costs.

## 6. Measuring the platform's floor under Balanced

A benchmark, `cargo xtask relay-floor`, launches a small benchmark binary in three roles: a sender A standing in for the listener, a relay B standing in for verbatim.exe, and a receiver C standing in for the outpost. It uses the same pipe types as Verbatim. Each message carries A's QPC stamp. B and C stamp with QPC on read return and before write, and keep their stamps in memory until exit.

- Gaps between messages are random (exponential, mean 20 ms, clipped to 1 to 100 ms), generated in A by waits on a waitable timer. Every receiver is then blocked when a message arrives, as with real events. A back-to-back flood would measure throughput, not wake-up.
- The variants:
  - F0: two threads in one process, one condition variable. This is the floor for one wake-up.
  - F1: A writes straight to C over one pipe. This is the floor for a single hop, with no relay.
  - F2: A to C through B, where B is one thread that reads synchronously and writes at once. This is the floor for any third process in the path.
  - F3: A to C through B built as section 4.1 (completion port, overlapped named pipes).
  - F4: A to C through B built as today's code (reader thread, crossbeam channel, owner, condition variable, writer thread). This is the baseline.
  - F3 and F4 each run at normal priority and at the 4.4 priorities, with power throttling off.
- The conditions:
  - Balanced, idle.
  - Balanced with N normal-priority spinning threads, where N is the number of logical processors.
  - Balanced with N minus 1 spinning threads, which keeps one processor free.
  - Balanced during a real end-to-end flood scenario.
  - Once, for attribution only, under High performance and with `timeBeginPeriod(1)` in all three processes, and on bare metal if Dickson has one, so that the VM's share and Windows' share can be separated.
- Size: 20,000 messages per variant and condition, which is about 7 minutes each at a 20 ms mean, and 100,000 messages for the chosen design under Balanced idle and under load.
- Reported per hop and end to end: count, median, 99th, 99.9th and 99.99th percentiles, maximum, and the number over 0.5, 1 and 2 ms.
- Attribution: one run of each condition under WPR's CPU usage (precise) profile, which records context switches and ready-thread events. For every sample over 1 ms, the trace gives the delay from ready to running for each thread involved, and what ran instead: another thread (pre-emption or priority), nothing in the guest (a vCPU stall), or an idle processor waking. This turns the inferred causes in section 3 into established ones.

## 7. Verification plan and pass criteria

Definitions, per relayed fact:
- H1 is "listener to verbatim.exe".
- H2 is "verbatim.exe".
- H3 is "verbatim.exe to outpost".

verbatim.exe's added latency is H1 plus H2. A route without verbatim.exe would still pay one hop shaped like H3.

1. Benchmark, before any Verbatim change: F0 to F4 as in section 6. This is recorded as the floor. Its decision point is in section 9.
2. Benchmark, after the design: F3 at the chosen priority, under Balanced, idle and under load, 100,000 messages each.
   - B's internal time: maximum at most 0.5 ms. It is pure CPU on a running thread.
   - H1 plus H2: 99.9th percentile at most 1 ms. Maximum at most 2 ms, except samples that the ETW trace shows to be guest-wide stalls (no guest thread ran during the delay). Those are counted and reported, never dropped.
   - F3's tail minus F1's at every percentile up to the 99.99th: at most 1 ms.
   - F3's share over 2 ms must be no worse than F2's, and well below F4's.
3. Verbatim, live: the measure scenarios (measure_focus_explorer, measure_keys_conhost, measure_keys_text_box and the rest of the measure set), release and debug, until at least 5,000 delivered facts per build, under Balanced.
   - The ledger's "verbatim.exe" stage: maximum at most 1 ms. 99.9th percentile at most 0.2 ms in release, at most 0.5 ms in debug.
   - H1 plus H2: 99.9th percentile at most 1 ms. Maximum at most 2 ms, with every exception attributed by an ETW capture during one of the runs, as in item 2.
   - Facts held for a starting outpost are reported separately, never pooled with delivered ones, until step 8 removes the wait.
   - Each hop's share over 2 ms is compared with the measured 0.24, 0.99 and 0.47 percent.
4. Requests in the other direction: the "Core to outpost" relay stage for caret-key watches and queries in the conhost and Windows Terminal key scenarios. Its maximum must be at most 1 ms.
5. Ordering and behaviour:
   - Unit tests for the relay thread's line framing: a line split across two reads, several lines in one read, a line feed alone.
   - The writer's existing overload-policy tests, kept against the new write state.
   - A supervisor test with a stand-in executable that a held fact is delivered before any fact read after `Ready`.
   - The existing hand-over order test, `a_hand_over_message_gets_through_a_full_queue_behind_the_facts_before_it`.
   - A test that a stuck child (a pipe nobody reads) does not delay facts to another child.
   - The supervisor's mockapp scenarios, and the end-to-end scenarios related to focus and terminals, per the targeted-run rule. Then the full suite, because this changes how every outpost's pipes are made.
6. Resources: verbatim.exe's thread count falls by one per child. Idle CPU must not rise: the relay thread wakes only for packets and the 3 s heartbeat. `cargo xtask ci` must pass, including clippy pedantic.

## 8. Risks

- Lifetimes of overlapped buffers and OVERLAPPED structures: freeing one while its I/O is pending corrupts memory. Each child's write state must own its buffer and OVERLAPPED until the completion arrives, including after `close`. A child is ended by cancelling with `CancelIoEx` and waiting for the cancelled completion before the memory is freed. This is the main unsafe surface, and it belongs in a small module of its own with tests.
- Named pipes bring a name into the object namespace. With `FILE_FLAG_FIRST_PIPE_INSTANCE`, a single instance, a random name, the user-only DACL and an immediate client connect before `CreateProcessW`, no other process can take the pipe. The DACL still has to admit the child at whatever integrity level outposts run at, and must not admit lower-integrity processes. This needs to be checked against how outposts are launched today.
- One thread now carries every child's writes and the listener's reads. A bug that blocks it stops all relaying, where today one writer thread would stall alone. The policy of never blocking on this thread must be enforced in review. Calls to process-hang detection, file I/O and logging are kept off it.
- Raised priority can starve normal threads if a relay thread ever loops hot, for example on a flood of tens of thousands of facts per second. The listener's replace-by-key queue and the 64-entry bound limit the work per fact. The flood scenarios must be run with the raised priorities.
- On the VM, an absolute 2 ms maximum may be impossible whatever the design, because the measured in-process maximum is 13 to 32 ms. The pass criteria therefore attribute exceptions through ETW rather than ignore them. Whether that reading of the budget is acceptable is Dickson's decision.
- The new `EventTiming` fields change the wire format for every outpost message. Serde defaults keep older peers readable, and Core and its children always ship together.
- The ledger's semantics change: `ListenerToOutpost` is split in three. `verbatim-inspect latency` consumers and `docs/crates/verbatim-app.md` and `verbatim-control.md` must change with it.

## 9. Questions for Dickson

- If the floor benchmark shows that F2, a minimal third process, already exceeds 2 ms at its maximum under Balanced on the VM, is the budget read as "verbatim.exe's own stage at most 1 to 2 ms, and its added hop no worse than the platform's floor, every exception attributed", rather than as an absolute maximum on this machine? Recommendation: yes, while also measuring once on bare metal.
- Should EcoQoS and the thread priorities decided on 2026-10-08 be built in this step? Recommendation: yes. They are the only lever on the non-silent tail that the power-plan constraint leaves open.

## 10. Follow-ups found, not part of this step

- The 2026-10-08 scheduling decision is unimplemented: no EcoQoS opt-out, no raised priority on the hook thread, the speech queue or synthesis (`roadmap.md:60-62`). It is recommended to fold this into step 9 as above. Otherwise it remains a separate open item.
- In test runs, the outpost's intake logs at debug level inside its lock (`outpost/intake.rs:306-313`, with `verbatim_outpost=debug` from `verbatim-e2e/src/scenario.rs:157`). That puts a synchronous file write under the intake lock on every fact.
- `write_message` makes two `WriteFile` calls per message on bare `File`s, in the listener and every outpost (`protocol.rs:876-878` with `main.rs:68-73` and `99`). Section 4.3 fixes this. It also affects the direction from outposts to Core.
- The startup wait, which accounts for every relay over 20 ms, belongs to step 8 (reserved spares). Its pass criterion is in `supervisor-latency.md`, "How to tell whether the target is met".
