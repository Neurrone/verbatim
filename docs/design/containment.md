# Containment

The design for milestone M4.7, Containment, in [the roadmap](../roadmap.md). It decides what a panic or a native crash does in each process and on each thread, how the keyboard hook keeps keys flowing while Core is stuck and comes back after Windows removes it, how a child's liveness is judged from the threads that do its work, and how a scan keeps every edge guarded. Restarting Core, Core's single panic path and the busy stamp are [the guardian](guardian.md)'s, milestone M4.8, which this design builds on. Decided with Dickson on 2026-11-05, after an adversarial review.

Terms used here:

- **Containment**: catching a panic at the edge of a thread or a callback, so that one fault does not end more than it must.
- **Edge**: a function that the operating system, COM, UIA, wxWidgets or a thread's start calls into Rust: an `extern "system"` or `extern "C"` function, a COM `_Impl` method, a cxx `extern "Rust"` function, or a thread's body.
- **Busy stamp**: one 64-bit value a thread writes when it starts a step of work and sets to 0 when it goes back to waiting; only that thread writes it.
- **Stale reducer**: Core's reducer whose busy stamp is nonzero and more than 1 second old.

## The problem

- Core has no containment: a panic on the reducer thread leaves Core running but mute, with the hook holding caret keys only the reducer releases; and the hook procedure, the relay, the speech and mixer threads, the GUI's callbacks, the WinEvent callback, the listener's grace timer and the WASAPI notification client have no guard either.
- A caught panic would poison locks that are then taken with `expect`.
- Nothing detects that Windows has removed the keyboard hook.
- A child's liveness is judged from the I/O thread that answers pings, not from the threads that do the work.
- A native crash in any process goes to Windows Error Reporting.

## 1. The panic policy

### 1.1 The rules

- **In Core, a panic anywhere ends Core, and the guardian restarts it** (Dickson). Reason: every thread of Core shares facts with the others, so none can be trusted after one has unwound part-way, and a restart is the one recovery whose result is known. The path is [the guardian](guardian.md)'s section 2.
- **In a child, a panic ends only what it must** (Dickson). A thread that can absorb a panic does, answering the one request or event it was handling; only a thread that owns state the whole process depends on starts the child's clean shutdown and replacement (section 1.3). Reason: a child serves one application or the desktop's focus, and ending it costs every reading it would do until its replacement is ready.
- **In the guardian**, each pass of its loop is contained, and a caught panic is logged and the loop goes on, because its state is a handful of handles and times.
- Panics unwind (`panic = "unwind"`), because aborting would make containment impossible.

### 1.2 Mechanisms

- **One spawner.** `verbatim_process::thread::spawn(role, on_panic, body)` replaces every `thread::Builder::spawn` in the shipped crates. It runs `body` under `catch_unwind` and on a panic carries out `on_panic`: in Core always `RestartCore(reason)`; in children `Continue`, `EndThread` or `CleanShutdown(code)`, as section 1.3 assigns; in the guardian `Continue`. A failed spawn returns an error instead of panicking: at startup the process fails to start, and later each call site handles it.
- **One edge guard.** `verbatim_process::edge::guard(name, fallback, body)` runs `body` under `catch_unwind`. Its behaviour is set once, at the process's start: in Core it calls `core_exit::restart` and never returns; in a child it logs at error level and returns `fallback`. COM `_Impl` methods use `guard_com`, whose fallback is `E_UNEXPECTED`; the hook procedure uses `guard_hook` ([the guardian](guardian.md), section 2.2). `verbatim_uia::com::guarded` becomes a call to `edge::guard`.
- **The panic hook only logs**, in every process: the payload, the location and the thread's name, taking no lock but the log's. On the hook and audio threads, marked by a thread-local real-time flag, it captures no backtrace, because resolving symbols there would hold the keyboard or the device. In Core it also runs the guardian's crash handshake; in children it runs none. A double panic aborts through `__fastfail` (Limitations).
- **Poisoned locks.** Every `Mutex` is taken through `verbatim_process::sync::lock`, which ignores poisoning, except where section 1.3 says a poisoned lock ends the process. Reason: one way to take a lock under the lints of [lints and unsafe](lints-and-unsafe.md), and no cascade from one panic.
- **`Instant` arithmetic** becomes `checked_add`, saturating at a far deadline, so a huge duration cannot panic.
- **A native-exception handler in every child.** The outposts, the listener and the synthesizer host each install a handler for native exceptions that writes a minidump into Verbatim's dumps folder (the synthesizer host into a file handle Core passes it at launch, since its sandbox cannot open the folder) and then reports the exception handled, so Windows Error Reporting never runs and nothing is uploaded (the security model's L5). The process then ends, and Core's supervisor sees a crash and replaces it as for any other. Core's own dumps are written from outside by the guardian.
- **The GUI**, a thread in Core: each `GuiCore` method in the cxx `extern "Rust"` block runs under `edge::guard`, which restarts Core; `wxApp::OnExceptionInMainLoop` logs and calls `core_exit::restart`; and every function C++ exports to Rust returns `Result`, so a C++ exception, including one in a modal dialog's or a popup menu's nested loop, reaches the restart path instead of `std::terminate`, which would skip the held keys and the dump. A unit test throws in each of those places.

### 1.3 What each thread of a child does on a panic

The outpost:

- **Worker**: catches each program's panic, answers it "panicked" and keeps running, as today. A Rust panic is raised between calls, never inside a COM call, so the UIA client holds no state for one call and stays usable, and unwinding releases the program's objects on the worker, a thread allowed to block. The worker is therefore not retired as an abandoned one is. The exception: a panic that unwound while the object store's lock was held leaves the store poisoned; the worker checks it after catching and, if poisoned, starts the clean shutdown, because the store is state the whole process depends on.
- **UIA callback threads**: the callback is contained and its event dropped.
- **The `BoundedClient`'s threads** (`crates/verbatim-uia/src/bounded.rs`): the operation is answered as abandoned and the thread ends; the next operation starts a new one, as after an abandonment. Today a caught panic leaves the thread serving.
- **The target watch**: `Continue`; its wait on the application's process handle starts again.
- **`win_event_proc`**: contained per event, the event dropped. Its closure takes `try_borrow_mut`, and a nested event, possible only if handling dispatched sent messages, is dropped with an error log, because the intake makes no such call and queuing nested events would add a second ordering path that hides that defect.
- **Threads owning process-wide state**, which start the clean shutdown: the shell (reducer) thread, the WinEvent event thread, the I/O thread and the writer, which owns the one queue to Core.

The listener:

- **Reading threads**: the read is answered as failed, judged "dropped", so the thread stays in charge of its application with a new reader. It is never counted as abandoned, because counting it held an application at the abandonment limit for good after two panics.
- **UIA callback threads**: contained, the event dropped.
- **`grace_ended`**: contained, and the next step ends the grace.
- **Threads owning process-wide state**, which start the clean shutdown: the scheduler and its watchdog, the WinEvent thread, the I/O thread, the writer, and the registration thread, which holds the desktop-wide registration the listener exists for.

The synthesizer host:

- **One utterance**, eSpeak NG's callback included: the panic is caught around it, the utterance answered to Core as failed, and the host goes on with the next command.
- **The command loop, or the writer to Core**: the host ends. Core's client sees the pipe end and starts a new host on the next request, re-applying the settings, as today. A native crash in an engine takes the same path. The host's backoff and its fallback to another synthesizer are M6's first build.

### 1.4 Startup and exit

- **No synthesizer at startup.** Core tries the configured synthesizer, then eSpeak NG, then OneCore. Only when none starts does it exit with `EXIT_NO_SPEECH`, which the restart breaker counts. eSpeak NG's start from an installation path that is not ASCII is tested (NVDA #11392 and #19582).
- **wxWidgets fails to initialize: Verbatim fails to start** (Dickson). Core logs the error and exits with `EXIT_NO_GUI`; the shutdown sequence (the hook, the save, the children) runs on that path as on every other, since `run_gui`'s error no longer leaves `run` through `?`. The guardian treats `EXIT_NO_GUI` as a failure to start, ending without a restart, because a toolkit that cannot start will not start on a retry.
- **`process::exit(0)` in `request_shutdown`** is removed; every exit goes through the shutdown sequence or `core_exit`.
- **Loading and applying settings never panics** (Dickson), since the saved settings are read on every start, the guardian's last restart included, and a value that panicked would end Verbatim at each start. `ConfigStore::load` turns unreadable or invalid content into the defaults for that value, and `Input::Settings` and each consumer of a setting clamp or refuse a value it cannot use, with a warning. Generated tests over arbitrary files and values are M8.3's.

## 2. The GUI thread

The GUI stays a thread in Core (Dickson). These rules hold for it, from NVDA's history of 77 GUI issues, nearly all of whose freezes came from the GUI and the core sharing one thread:

- **wx is touched only on its own thread**, because wx is not thread-safe and NVDA's timers touched from another thread broke (NVDA #6152).
- **No synchronous wait across threads**, in either direction, because a thread waiting on a GUI confirmation blocked the waiter (NVDA #18076); hand-offs are posted, bounded or coalesced.
- **A real message loop**, never a hand-rolled yield, because input methods need it (NVDA #14789) and a re-entered loop ran callbacks inside callbacks (NVDA #7077).
- **No network or file work on the GUI thread**, because a 120-second connect timeout there froze NVDA's keys (NVDA #17036).
- **A modal returns its result, and its action runs afterwards**, never from inside the modal, because an action run from inside one restarted NVDA in an unsafe state (NVDA #17582).
- **No new modal once shutdown starts, and Core's shutdown ends open dialogs, then waits for the GUI thread only within a bound**, because an open dialog blocked NVDA's exit (NVDA #15448, #12183).
- **Panel rebuilds are debounced**, because a rebuild's own value-change events flood the outpost reading Verbatim's GUI (NVDA #20519).
- **No privileged helper process for the GUI**, because each added privileged process is attack surface (NVDA #19555).

Out of scope for now: the GUI stays in Core rather than a process of its own, and file pickers stay the standard dialog in Core, with no helper process. Reason: NVDA has no report of a crash from shell extensions in its own file dialogs, and a separate GUI thread already removes the class of freeze NVDA's GUI suffers. So Core loads the shell extensions a file dialog brings, the input methods of its text fields and other programs' global hook DLLs, as the architecture's section 12 records.

## 3. The hook passes keys while the reducer is stale

The hook procedure reads the reducer's busy stamp ([the guardian](guardian.md), section 2.1), one atomic load, and the performance counter, through an `Arc<CorePage>` given to `InputHook::start` before the hook is installed. While the reducer is stale (decided: 1 second), the decision machine decides with `HoldPermit::None` and `KeyInput::bypass: true`:

- **A key with nothing in the machine's line passes**, and a release passes when its press passed.
- **A key with an entry in the line is still deferred and reported**, behind the keys already waiting. Reason: a deferred press followed by a release that passed would later send the press with nothing to release it, leaving Shift stuck system-wide.
- **Everything else continues**: modifier tracking, `keys_down`, the double-tap state, key numbering, and speech cancelled on a press (NVDA #12610, written from `docs/parity/input.md`).
- **No other report is posted**, apart from the harness's `Handled` marks, because a gesture or caret report handled late would act on a key that already reached the application.

Bypass ends on the first key after the stamp is fresh. After a sleep the age is huge for one key at most, because the step's own end sets the stamp to 0. Bypass adds no lifecycle state.

## 4. The hook watcher

Windows removes a low-level keyboard hook silently when one call takes longer than `LowLevelHooksTimeout` (at most 1,000 ms), and the program is never told. The witness detects it from raw input, with no periodic probe, because a probe wakes Verbatim when idle and its injected keys reach applications.

### 4.1 Where it runs

The witness is a Core thread, `verbatim-hook-witness` (Dickson). It owns a message-only window and, on it, a keyboard raw-input registration (`RIDEV_INPUTSINK`, without `RIDEV_NOLEGACY`), `WTSRegisterSessionNotification`, `RegisterSuspendResumeNotification` and an out-of-context `EVENT_SYSTEM_DESKTOPSWITCH` hook. The hook thread takes two more posted messages besides `WM_QUIT`, `WM_REINSTALL(raw_key)` and `WM_RESYNC`, and answers each with one `WM_HOOK_DONE(result)` posted to the witness, because the hook procedure runs on the thread that installed it, where its machine lives.

### 4.2 The evidence

The hook procedure stores the performance counter in `hook_called`, a single-writer stamp shared with the witness. On a keyboard `WM_INPUT` outside a quiet period, when no check is armed, the witness records the raw key's counter value and `GetForegroundWindow`, and sets a one-shot 1-second timer. When it fires, it is evidence only if:

- `hook_called` is older than 500 ms before the raw key, a span covering either order of delivery;
- the window recorded was not in a process of higher integrity than Core's, read only on suspicion, because the hook does not see those keys and raw input does (dropped when Core gains UIAccess);
- no session, power or desktop event fell inside the check.

Keys Verbatim injects, and the harness's, reach both the hook and raw input, so they count on both sides.

### 4.3 Reinstalling

On evidence, outside a 30-second cooldown, the witness posts `WM_REINSTALL` if none is pending; if one is, one more is queued behind it, so a resume during a reinstall is not lost. The hook thread then rechecks (if `hook_called` is newer than 500 ms before the raw key it answers "alive"; resume and unlock requests skip this), unhooks the old handle and installs anew, so two hooks are never in the chain at once (NVDA #5629), resynchronizes (section 4.5) and answers.

- **Success** is logged at warning level, with no sound, and `hook_reinstalls` in the developer endpoint's status goes up by one.
- **A failed `SetWindowsHookExW`** leaves no hook, so the witness asks for a Core restart; the restart breaker bounds a loop.
- **The cooldown** bounds a loop with another program's hook that swallows keys raw input still sees.

The witness has four states: the armed check (a burst of keys arms one timer), the pending reinstall and the one queued behind it, the end of the quiet period, and the end of the cooldown.

### 4.4 Session, power and desktop events

- Suspend, lock, and a switch to another desktop drop an armed check and start a quiet period ending 2 seconds after the matching resume, unlock or switch back, because raw input queued over a suspend is false evidence.
- Resume and unlock post `WM_REINSTALL` without evidence.
- Every return to the default desktop, and every unlock, posts `WM_RESYNC`, because modifiers released on the secure desktop were never seen (NVDA #1856).

### 4.5 Resynchronizing with the keyboard

`DecisionMachine::resync(is_down)` is pure: keys not down now leave `keys_down` and `held_modifiers`; modifiers down now are added as having reached the application; the lone-modifier tap goes idle; the line is cleared. The hook thread calls it with `GetAsyncKeyState` after a reinstall and on `WM_RESYNC`. Reason: a modifier wrongly left down makes every later key a chord (NVDA #21083).

### 4.6 Edge cases and tests

- Raw input is registered once per process and device class; a unit test asserts the registration holds.
- Registering raw input changes nothing applications see: a scenario opens and closes Notepad's menu with Alt and holds Alt for autorepeat, asserting the exact speech and the menu's state, with the witness running.
- The input-method profile query moves off the hook thread (decision of 2026-11-05), because a slow call there makes Windows remove the hook; typed-key translation stays on it, for dead-key timing.
- `keyboard_hook_reinstalled_after_removal`, against Notepad: the agent gains `SuspendThread { pid, thread_description, resume_after_ms }` and suspends Core's `verbatim-input-hook` thread for 2 seconds; the test types `a`, which reaches Notepad after Windows removes the hook, then `b`; it waits for `hook_reinstalls` to reach 1, presses the report-focus gesture, and asserts its exact speech and that Notepad's text is exactly `ab`.
- Pure unit tests cover the evidence rule and `resync`.

## 5. Liveness from the working threads

Each child has a `Vitals` struct of busy stamps, each with exactly one writing thread:

- **Outpost**: the shell thread (each step), the event thread (each WinEvent and nudge), the writer (each message it encodes, not while it waits for room in the pipe, which is Core's to judge) and the target watch.
- **Listener**: the WinEvent thread, the reading threads' watchdog, the writer, and every UIA callback thread, each of which claims one of 32 slots through a small lock on its first callback; a thread past the limit is not watched, and the log says so.
- **Not watched**: the listener's registration thread, whose one call may legitimately wait about 6 seconds, and the reading threads and workers, which their deadlines and abandonment already judge.

On a ping the I/O thread answers only while each stamp is 0 or younger than one ping interval (3 seconds); otherwise it logs once per stall which thread is stuck and for how long, and withholds the pong, so the supervisor's missed-pongs rule ends the child through its one failure path.

A panic on a thread that owns process-wide state starts the child's clean shutdown, the path taken when Core's pipe closes: no new work, a program stopped at its next call boundary and the MSAA call in flight cancelled through COM ([the guardian](guardian.md), section 12), then an exit with `EXIT_THREAD_PANICKED`. Reason: ending at once could kill a UIA client mid-call, the leading suspect in the Windows Terminal crash. The shutdown needs none of the threads that panicked: if the shell panicked, the I/O thread runs it; if the I/O thread did, the event thread does. The supervisor sees a non-zero exit and replaces the child, killing it at the shutdown limit, 1 second, decided by Dickson on 2026-11-06 and built in M6.2; 21 seconds until then, if it does not finish.

Cost: two relaxed stores per step on each watched thread and a few dozen loads per ping; no wake-up.

## 6. The relay

- Each completion and each mailbox event runs under `catch_unwind`; a panic or a failed wait restarts Core, as every panic in Core does, and the `assert!`s in `crates/verbatim-process/src/overlapped.rs` become errors.
- It never stops silently: it returns only from `finish_shutdown`.
- **Its beat**: the relay increments `relay_beat` in the Core page on each pass, at least every 3 seconds at its heartbeat, and the guardian judges only whether it changes ([the guardian](guardian.md), section 6).
- A Core restart no longer kills the children through their jobs; they shut down cleanly ([the guardian](guardian.md), section 5).

## 7. The containment scan

`xtask/src/containment.rs` (planned) runs in `cargo xtask ci`. It parses the shipped crates' non-test sources with `syn` and fails on:

- **an unguarded edge**: an `extern "system"` or `extern "C"` function defined rather than declared, a `*_Impl` trait method, or a method of a type named in a cxx `extern "Rust"` block, whose body is not one call to `edge::guard`, `edge::guard_com` or `edge::guard_hook`;
- **a raw thread spawn**: `std::thread::spawn` or `std::thread::Builder::spawn` outside `verbatim_process::thread`.

An allow-list holds each exception with its reason. The scan reads syntax only, and finds the `implement` macro's edges by the `_Impl` trait's name.

## Lifecycle states added

- Hook bypass: none.
- The hook's line-report flag: one boolean reset at each call, deciding `guard_hook`'s fallback.
- The witness: four states (section 4.3).
- Child liveness: the stall-reported flag, so a stall is logged once; UIA slots claimed once per thread.

## Costs

- **Event path**: `catch_unwind` at thread and edge boundaries, free when nothing panics, and two relaxed stores per watched step. No lock, call or wake-up.
- **Key path**: one load and one store per transition in the hook; in the witness, one wake-up and one `GetRawInputData` per key transition, one timer per burst, and at most one timer wake-up a second while typing (Dickson).
- **Speech path and idle**: none.

Measured before and after: `measure_keys_notepad`'s wake-ups and processor time per thread, which gives the witness's cost; the reinstall's time in the end-to-end test; whether a key swallowed by another program's later-installed hook still produces `WM_INPUT`; whether `GetAsyncKeyState` reports a key down after its release was swallowed. The witness's wake-up and the stamps go into `books/dev/src/reference/performance.md`.

## Limitations

- Keys passed under bypass reach the application ahead of a held key still waiting, with no echo or caret speech, which can change the text typed: Shift deferred behind a held key, then `x` typed while the reducer is stale, gives `x` where the user typed `X`.
- The witness needs a key typed after the removal; the keys in that second are unseen. One raw keyboard registration per process.
- A child panic absorbed by a thread drops the input it was handling and whatever it would have spoken; a dropped focus event leaves the old focus until the next one.
- A double panic or a `__fastfail` skips every handler and reaches Windows Error Reporting; the guardian keeps its dialog away ([the guardian](guardian.md), section 9).
- Core loads third-party native code through its GUI thread and its message-pumping threads (section 2).
- The containment scan reads syntax.
