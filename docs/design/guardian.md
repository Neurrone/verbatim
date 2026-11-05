# Guardian

The design for milestone M4.8, Guardian, in [the roadmap](../roadmap.md). A guardian process, `verbatim.exe`, starts Core and starts it again after it dies; Core takes one path on any panic; and the guardian owns the single instance, the end of the session, hang detection, dumps of Core and the restart breaker. What each thread does on a panic, the hook's pass-through and watcher, and children's liveness are [containment](containment.md)'s, milestone M4.7. Decided with Dickson on 2026-11-05, after an adversarial review.

Terms used here:

- **Guardian**: `verbatim.exe`, the process that starts Core and starts it again after it dies.
- **Core page**: one small unnamed shared-memory section per Core process, created by the guardian and inherited by Core (section 4).
- **Busy stamp**: one 64-bit value a thread writes when it starts a step of work and sets to 0 when it goes back to waiting; only that thread writes it.
- **Generation**: one Core process and the job object the guardian creates for it, the generation job, which holds Core and every child it starts.
- **Old children**: the children of a Core that has ended, while they still run.
- **Restart breaker**: the guardian's limit on restarts (section 8).

## The problem

Nothing outside Core can restart it. A panic on the reducer leaves Core running but mute, with the hook holding caret keys only the reducer releases; the panic hook takes the recorder's lock while the reducer may hold it; a hang is never detected; a crash goes to Windows Error Reporting; and a restarted Core would start new outposts while the old ones are still inside their applications.

## 1. Processes, images and launch

- **`verbatim.exe`** is built from a new crate, `crates/verbatim-guardian` (planned), classed as Windows-specific in `xtask/src/tiers.rs`. Its dependencies are `windows`, `verbatim-process` and `tracing`: no COM, no UIA, no third-party native code. It has one thread, plus a short-lived thread per minidump. Its first act is `verbatim_process::clock::hand_on`, so the guardian, Core and every child share one clock offset.
- **`verbatim-core.exe`** is the `verbatim-app` package with its binary renamed. It refuses to run without `--guardian` and tells the user to run `verbatim.exe`, so there is one code path.
- The guardian passes Core its command line, environment and standard handles, inherited with no pipe, so no pipe can fill and block a thread that logs.
- `--guardian` carries handles inherited through `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` only: the Core page; manual-reset events Core sets (`running`, once the reducer has started and the hook is installed; `exiting`; `snapshot_wanted`); and events the guardian sets (`stop_quit`, `stop_replaced`, `snapshot_taken`, `old_outposts_gone`, `old_listener_gone`). Core waits on the stop events with one threadpool wait each, both guarded edges. `--extensions-off` is passed on the last restart only.

## 2. Core's one panic path

**In Core, a panic anywhere ends Core, and the guardian restarts it** (Dickson): no thread absorbs a panic and nothing is rebuilt in place, because every thread of Core shares facts with the others and a restart is the one recovery whose result is known.

### 2.1 The busy stamp

The reducer writes `reducer_busy` in the Core page: the raw `QueryPerformanceCounter` value when it takes a command off its port or runs `expire`, and 0 when it goes back to waiting. Two relaxed stores per pass suffice, because no other data is published through it. The hook reads it to judge age ([containment](containment.md), section 3); the guardian reads only whether it changes (section 6). It replaces the relay's `ReducerWatch`, so one owner judges the reducer's liveness.

### 2.2 Ending Core

Every thread of Core runs under the spawner's `RestartCore`, and every edge under `edge::guard`, so a panic anywhere reaches `core_exit::restart(reason)`, which does this in order:

1. **The panic hook**, still inside the panic, fills the crash record, sets `snapshot_wanted` and waits up to 2 seconds for `snapshot_taken`, so the dump shows the panicking frames (section 9).
2. **The held keys go out in order**, where possible. On the reducer thread, `core_exit::restart` calls `HeldKeys::release_all`: every held key and deferred transition in one `SendInput`. On any other thread it posts `ReleaseHeldKeys` to the reducer with a one-shot reply and waits at most 250 ms; a reducer that is stuck, or is the one that panicked, never answers, and the keys are lost. A torn line is never sent: each `HeldKeys` method sets a `busy` field on entry and clears it on return, so a panic that unwound through one leaves it set, and sending it could send a key twice or not at all.
3. **The process ends** with `TerminateProcess(self, EXIT_RESTART_REQUESTED)`. No other teardown runs, because the state that panicked cannot be trusted, and the settings are already on disk, since the saver writes on every change.

The input that panicked is written alone as a one-line dump beside the minidump.

**The hook thread** cannot wait inside the hook procedure, because Windows holds every key system-wide while it runs. `guard_hook` returns at once: it swallows the key if this transition already posted a `Held` or `Deferred` report, since Core would send that key again from its line, and otherwise passes it; a flag in `HookState`, reset at each call, records that. It posts `WM_CORE_RESTART` to its own thread, whose message loop unhooks and only then calls `core_exit::restart`; the snapshot then lacks the hook procedure's frames, and the log has the panic's location. A `GetMessageW` error, or the loop ending for any reason but `WM_QUIT`, takes the same path. A failed wait in the relay takes it explicitly.

A panic that recurs whenever the same input arrives therefore takes the whole path: a restart, the breaker's next restart, its last restart without extensions, then shutdown. Nothing is quarantined (Dickson). Prevention is [lints and unsafe](lints-and-unsafe.md)'s lints and generated tests.

### 2.3 The recorder

The flight recorder is off by default. The panic hook never touches it, which removes the deadlock with a reducer holding its lock; with the recorder on, Core's end writes no flight dump either, and the minidump holds the window.

### 2.4 A hang

A reducer step that does not return is a defect. The hook passes keys after 1 second; the guardian sees `reducer_busy` stay nonzero and unchanged past the hang limit, writes a dump from outside, ends Core and restarts it (section 6).

### 2.5 What users hear

On a panic or a hang, Core ends with no sound of its own; the new Core plays the start sound, then speaks the focus. There is no announcement of the restart. When the breaker stops Verbatim, nothing is heard (section 8).

## 3. Single instance, replacement and the end of the session

The guardian owns a **hidden top-level window** of class `VerbatimGuardian`, because a message-only window receives neither `WM_QUERYENDSESSION`, `WM_ENDSESSION` nor `WM_POWERBROADCAST`.

- **The end of the session.** The guardian raises the screen reader flag at its start and clears it at its exit. On `WM_QUERYENDSESSION` it answers yes and marks the session as possibly ending; while marked, a Core exit is neither counted nor restarted, but held. `WM_ENDSESSION` with `wParam` true: it clears the flag and exits, whether or not Core still runs. With `wParam` false (a cancelled sign-out, another application's veto, or Restart Manager's `ENDSESSION_CLOSEAPP`): it clears the mark and, if Core exited meanwhile, starts a new one at once, uncounted, because Windows ended it. It takes over `crates/verbatim-app/src/screen_reader_flag.rs`'s reading of `wParam`.
- **Single instance, through a private namespace** (Dickson), with no window-message filter. The boundary descriptor holds the user's SID and the guardian's integrity level. Inside it: the **instance mutex**, held by the guardian for its life; the **replace event**, in the guardian's wait with its other handles, so a request costs no wake-up; and two manual-reset events, **`ready`**, set while a Core runs, and **`restarting`**, set between Cores or while shutting down after the breaker. A lower-integrity process cannot open a namespace whose boundary carries a higher label, so it cannot stop a higher-integrity Verbatim, which removes the process-wide `ChangeWindowMessageFilter(WM_QUIT)` in `crates/verbatim-app/src/single_instance.rs` (the security model's C6). A new guardian finding the mutex held compares its token's integrity with the running guardian's and, if its own is lower, logs why and exits. The running guardian's pid comes from its hidden window, found with `FindWindowExW`. NVDA's message box when the old instance cannot be ended, and the 2-second wait after `TerminateProcess`, are kept.
- **Launch rules** (Dickson). A new guardian opens the namespace and:
  - **mutex free or abandoned**: takes it and starts; an abandoned mutex means the old guardian died, and its kill-on-close job ended everything with it;
  - **mutex held, neither `ready` nor `restarting` set**: the instance is still starting, so the launch exits doing nothing;
  - **mutex held, `ready` set**: a replacement. It sets the replace event; the old guardian sets `stop_replaced`, and Core stops at once with no exit sound, shuts its children down cleanly, saves and exits with 0; the old guardian waits until every process of its generation has exited (section 5), clears the flag, releases the mutex and exits;
  - **mutex held, `restarting` set**: it sets the replace event, which stops the old guardian starting another Core, waits for the old guardian to exit, and starts afresh with its crash count at zero;
  - **an unresponsive old guardian**: in each waiting case, if the mutex is not released within about 15 seconds (the exit save, the 12-second clean shutdown, a margin), the new guardian ends the old one, whose kill-on-close job ends everything left, and takes the abandoned mutex.

So every old process is gone before the new Core starts.

## 4. The Core page

It holds only what a kernel object cannot carry, each field with one writer:

- **`reducer_busy`** (u64): written by the reducer, read by the hook and the guardian. It is memory because the hook reads it on every key, and an event set on every step would cost a system call on the event path; relaxed ordering suffices, since readers act only on its value or change.
- **`relay_beat`** (u64 counter): written by the relay, read by the guardian for change only; memory because a kernel object signalled per pass would cost a system call per pass.
- **`startup_progress`** (u64 counter): written by Core's main thread until `running` is set, read by the guardian for change only.
- **The crash record**: the faulting thread's id, the address of the `EXCEPTION_POINTERS` or 0, and a reason code, written once by the thread that claims it through a process-local `OnceLock`, and read only after `snapshot_wanted` is signalled, whose `SetEvent` and wait order the write before the read.

Nothing else is shared: the phase is the `running` and `exiting` events, commands are events, exits are process handles. The guardian uses the crash record only as arguments to `MiniDumpWriteDump`, so a corrupt page can at worst give a poor dump.

## 5. Jobs, and learning that processes have exited

- The guardian creates each generation job with `KILL_ON_JOB_CLOSE` and starts Core suspended inside it. **If the guardian dies**, Core and every child die with it, mid-call (Dickson), because keeping Core alive without its guardian would need a second owner of the single instance and the restart. The next launch takes the abandoned mutex with no old child left.
- **Core's per-child jobs nest inside it.** They keep their memory caps but lose `KILL_ON_JOB_CLOSE`, and `Contained`'s `Drop` ends a child that has not exited with `TerminateJobObject`. So when Core dies its children are not killed: they find their pipe closed and shut down cleanly. The synthesizer host's sandbox job keeps kill-on-close and dies with Core, since it holds no connection to an application.
- **A memory cap on Core**: a per-process memory limit on the generation job, its value set once Core's footprint is measured, and no lower than any child's own cap, since a nested job is held to its parent's limits too. Exceeding it ends Core, which the guardian restarts as any other exit (the security model's A2).

Every exit is learnt from **process handles**, never from job messages, whose delivery Windows does not guarantee. When a Core has exited, the guardian reads the generation's process list, opens each process with `SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION`, keeps it only if `IsProcessInJob` confirms it (a pid reused between the list and the open names another process), reads its image name to class it as an outpost, the listener or other, and waits on the handles in its own `MsgWaitForMultipleObjectsEx`, at most 56 at a time, reading the list again when a batch has exited.

From those exits it sets `old_outposts_gone` once no old outpost of any generation in its grace runs, and `old_listener_gone` likewise, so the new Core can wait without knowing any child; and it ends a generation with `TerminateJobObject` when it is empty or at its 13-second deadline, whichever comes first. It keeps every generation still in its grace, because two crashes inside one grace leave stragglers from both.

## 6. Hang detection

The guardian's wait is deadline-driven (Dickson), so it never times a wake it will not act on.

- **What it remembers**: for each watched field (`reducer_busy`, `relay_beat`, and while starting `startup_progress`), the value last read and when it was first seen, on `QueryUnbiasedInterruptTime`, which leaves out sleep and hibernation.
- **When it sleeps until**: each field's deadline is its first-seen time plus its limit; a `reducer_busy` of 0 has none, since an idle reducer cannot be stuck. The guardian sleeps until the earliest deadline, with its other handles in the same wait. On waking it reads every field; a changed field is first seen now.
- **Limits**: the reducer's is the longest legitimate step this build measures, expected 2 to 5 seconds; the relay's is three missed beats, about 9 seconds, matching the ping cadence; startup's about 14 seconds without progress.
- **Stuck** only after three observations of one value: at the deadline wake, then at two confirming wakes 1 second apart, so a hang is judged about 12 seconds after it begins. A resume (`PBT_APMRESUMEAUTOMATIC`) discards confirmations under way and makes every field first seen now. Reason: the guardian judges only what it saw while awake, so a sleep, a paused virtual machine or Modern Standby's throttling gives a thread the confirmations' two seconds and a fresh limit.
- **Starting** is judged by progress, never by a fixed deadline: before each stage of startup, and before each bounded wait inside one (each synthesizer tried, the device's opening, wxWidgets), Core increments `startup_progress`. About 14 seconds is the longest single bounded step (one synthesizer attempt of 10 seconds, or the 12-second clean shutdown it may wait behind) plus a margin, so a slow sign-in whose stages each end is never judged hung. The listener's registration and the waits for old children come after `running`.
- **Exiting**: Core not exited within about 15 seconds of `exiting` is hung.
- **A debugger** attached to Core (`CheckRemoteDebuggerPresent`) suspends judging.

A hang means: snapshot Core, terminate it, count a crash and restart (section 8), then write the dump from the snapshot (section 9).

Idle cost: the relay beats at least every 3 seconds, so each deadline wake finds the beat changed and sleeps a full limit, about 6 wakes a minute, to be measured.

## 7. Old children exit before their replacements

An application's new outpost starts only after the old outpost serving it has exited, so two Verbatim processes never talk to one application through UIA, a timed-out request still queued in the application included (`books/dev/src/reference/uia-bugs.md`). After a Core dies, the new Core starts no outpost and gives no spare an application until every old outpost of every generation in its grace has exited, and starts the new listener only once the old listener has exited (Dickson).

- **Old outposts**: the new Core's owner holds every launch and every `Assign` until `old_outposts_gone`, through one threadpool wait that posts to its mailbox. Facts arriving meanwhile wait in the existing held queue for each starting application, in order. Spares launch at once, since a spare touches no application until given one.
- **The old listener**: the relay starts the new listener on `old_listener_gone`, through a second threadpool wait; until then the listener's `Ready`, which gates readiness for input, is not given.
- The new Core knows the old children only through the two events, so no table about them is kept anywhere.

**The cost**: a clean shutdown takes up to 12 seconds (section 12), so when Core dies while any application, even a background one, is hung with a call inside it, every application waits up to the 13-second grace. In the common case old children exit in the measured 4 to 443 ms (median 15), overlapped with the new Core's start. A child not exited 13 seconds after Core's death is ended with its generation job.

## 8. Restarting Core, and the restart breaker

- **Exit 0 after `stop_quit`, the menu or the control plane**: the guardian waits out its generation and exits.
- **Exit 0 after `stop_replaced`**: as in section 3.
- **`EXIT_NO_GUI`**: Verbatim fails to start; the guardian waits out its generation and exits without a restart ([containment](containment.md), section 1.4).
- **Any other exit while the session is not ending** (`EXIT_RESTART_REQUESTED`, `EXIT_NO_SPEECH`, a crash, a hang the guardian ended, Task Manager): the guardian counts a crash, consults the breaker and, if allowed, starts the new Core at once.

**The restart breaker** counts abnormal exits over 120 seconds, NVDA's window:

- after the first, a normal restart;
- after the second, the last restart, with `--extensions-off`; the saved settings are read as usual, since loading and applying them never panics;
- on the third, no restart: the guardian logs the history, ends the generation after its grace, clears the flag and exits. No sound and no message box (Dickson); the log says why Verbatim stopped.

The count lives for the guardian's life; starting Verbatim again starts a new count.

## 9. Dumps written from outside Core, and Windows Error Reporting

- **On a crash or a panic**, Core's unhandled-exception filter, or the panic hook on any thread but the hook thread, claims the crash record through the `OnceLock` (a second crashing thread waits forever), fills it, sets `snapshot_wanted` and waits up to 2 seconds. The guardian takes `PssCaptureSnapshot`, which takes milliseconds, sets `snapshot_taken`, waits for Core's exit (ending it after 2 seconds), restarts, and only then writes the minidump from the snapshot on a short-lived low-priority thread, because the dump takes hundreds of milliseconds and would break the 1-second budget.
- **On a hang**, the guardian snapshots with no record.
- **Where**: `dumps\core-<UTC time>-<reason>.dmp`, keeping the 10 newest.
- **A fast-fail** (Rust's `abort`, a double panic, heap corruption) skips every filter and goes to Windows Error Reporting. The guardian suppresses Windows Error Reporting's dialog for Core, so a fast-fail never leaves a dialog waiting for a user who cannot see it, and still restarts Core from its exit as for any crash. Such a crash cannot be dumped from outside.

## 10. The 1-second silence budget

Measured from Core's death to the first audio frame of the focus utterance (decided: 1 second, no announcement).

- Core's startup is reordered so that the supervisor with its listener and spares, the synthesizer host and the WASAPI device start together; facts outposts report wait in the reducer's port, as now.
- The budget includes the guardian's launch (about 15 ms, to be measured), Core's startup, and the old foreground outpost's and old listener's exits, in the common case within Core's own start.
- Whether the focus waits for the start sound, which plays on its own earcon lane, is checked in the measurement and reported, not changed here.
- If the measured cold start exceeds 1 second after the reordering, the figures go to Dickson rather than being worked around.

## 11. How the harness and developers launch things

- Developers run `target\debug\verbatim.exe` and attach a debugger to `verbatim-core.exe`, which the guardian then does not judge.
- The end-to-end harness launches `verbatim.exe` through the agent. `verbatim-core.exe` joins the staged and crash-dump images and `dumps\core-*.dmp` is collected; Core's pid comes from the control-plane status; a scenario that injected no fault fails on any abnormal exit of `verbatim-core.exe`.
- The developer endpoint gains `InjectFault` (reducer panic, relay panic, Core crash, Core hang). New scenarios pin the restart after a panic on the reducer, the relay and another Core thread, held keys released before it, its time to speech, and the breaker. Each gets its row in `books/dev/src/reference/test-coverage.md`, and `cargo xtask e2e-durations` runs after its GitHub run.

## 12. Timeouts

Decided by Dickson on 2026-11-05. The rule: every limit is longer than everything inside it, plus a margin.

- **One UIA request**: 10 seconds (`verbatim_uia::TRANSACTION_TIMEOUT`); the connection timeout stays 2 seconds.
- **An outpost's task deadline**: 11 seconds per handled entry, so the request's own timeout fires first.
- **An outpost's clean shutdown**: up to 12 seconds (`SHUTDOWN_LIMIT`, today 21). To fit, a program stops at its next call boundary; the MSAA call in flight is cancelled through COM (`CoCancelCall`, with cancellation enabled on each worker), as is the call of a worker abandoned at its deadline, so fewer threads stay inside a hung application; and abandoned workers are still joined, each now waiting on at most one 10-second request. The listener's clean shutdown has the same bound: its reads stop at their next call boundary and its abandoned reading threads are joined.
- **The guardian's grace for an old generation**: 13 seconds, after which it ends the job.
- **Replacing a running instance**: about 15 seconds before the old instance is ended, where today it is 5.
- **Startup judged hung**: about 14 seconds without progress. **Exiting judged hung**: about 15 seconds.

## Lifecycle states added

- The held-key line's `busy` field, telling a torn line from a whole one.
- One `ReleaseHeldKeys` with a one-shot reply and a 250 ms bound, answered once or dropped with the process.
- The owner's "held for old outposts" and "held for the old listener", each cleared by its event.
- The crash handshake: the claimed record and its two events.
- The guardian: the current Core's phase, read from its events; the crash history; the next start's arguments; the generations in their grace, each with its handles and deadline; "session possibly ending", with a held Core exit; and the namespace's `ready` and `restarting` events.

Generated tests against a much simpler reference model, with exact postconditions after every step, cover the restart breaker and the generations in their grace (two crashes within one grace included), and the owner's holds (an exit before its wait was registered, a reused pid, a spare given an application during the hold).

## Costs

- **Event path**: two relaxed stores per reducer pass. No lock, call or wake-up.
- **Idle**: the guardian's deadline wake about 6 times a minute, to be measured. Children add nothing.
- **Calls into other processes**: none on any path; after a death, the guardian reads the job list, opens each old process and reads its image name.
- **Memory**: the guardian's working set, expected at 1 to 2 MB; the Core page under 64 bytes.

Measured before and after: the longest legitimate reducer step (a terminal flood's diff, the largest focus and text inputs), which sets the reducer's hang limit; idle wake-ups of the guardian and Core over 10 minutes; cold start and restart to the focus's first frame, 20 runs each on x64 and ARM64, with and without a hung background application; an injected panic to the process's end, with and without held keys; `PssCaptureSnapshot`'s time and the dump's size; whether the performance counter runs through sleep; the old children's exit after a Core death, with and without a call abandoned in a hung mockapp. The guardian's idle wake-ups go into `books/dev/src/reference/performance.md`.

## Limitations

- When Core dies or hangs, the keys in its held-key line are lost, and a release deferred behind a held key never reaches the application, leaving that key down there until pressed again. A caught panic releases the line first, unless the reducer does not answer within 250 ms or the line is torn.
- A deterministic panic shuts Verbatim down: an application whose focus always panics the reducer ends Verbatim while it is in front, and a gesture that always panics ends it after three presses within 120 seconds.
- Every panic in Core costs a restart, about a second of silence in the common case, including one on a thread that shares no facts, such as the clipboard or the saver. A restart loses the utterance in progress, Say All's position, an open dialog's unsaved edits and the control-plane clients' connections.
- After a Core restart, a hung background application with a call abandoned inside it delays all reading up to the 13-second grace, and its old child is then killed mid-call. The guardian's death ends everything mid-call, with the same risk.
- A hang has no flight evidence when the recorder is off; a fast-fail cannot be dumped from outside.
- An outpost replaced while Core is alive still gets its spare before the old outpost has exited, as today; that ordering is M6.2's.
- The guardian will need UIAccess when Core does (M8.6), and on a secure desktop one guardian runs per desktop.

Accepted risks (Dickson): the guardian's dumps of Core contain user text, as flight dumps do; the Core page is shared, unnamed, between Core and its guardian only.
