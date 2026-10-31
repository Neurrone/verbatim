# Verbatim Architecture

Verbatim is a screen reader for Windows 11, version 24H2 or later (x64 and
ARM64, both first-class), with no fallbacks for older Windows,
written in Rust. It is informed by NVDA (vendored under `nvda/` for reference)
but is not constrained by NVDA's architecture.

Documentation convention: these docs avoid ASCII-art diagrams, box drawings,
and arrow chains, and prefer lists over pipe tables, so they read well with a
screen reader in both rendered and source form.

## 0. Decisions of record

- **D1, dual accessibility backends.** UIA and MSAA/IA2 are both
  first-class client stacks behind the normalized model, with
  per-application arbitration picking the richest source; JAB joins last
  (roadmap M13). Why: UIA alone serves WinUI, Terminal and modern Office,
  while browsers and much of Win32 are richest through MSAA/IA2. Decided
  2026-07-13.
- **D2, injection never required for correctness.** An in-process helper,
  NVDA's nvdaHelper analog, ships with browse mode (M7) after
  out-of-process IA2 has proven correctness, for IA2 call batching and
  virtual-buffer speed. Why: NVDA shows in-process access is what makes
  large documents fast, and the helper's binary matrix and antivirus
  friction stay out of early milestones. Decided 2026-07-13. Amended
  2026-07-15.
- **D3, the end-to-end suite runs wherever the harness reaches; CI is
  hosted runners only.** The suite talks to the in-guest agent at an
  address, on a hosted runner, a developer's machine or VM, or a local
  Hyper-V VM (section 14). CI runs it silently on every change, recorded,
  with no self-hosted runners, no nested virtualization and never NVDA;
  a snapshot restore is always an explicit request. Why: a suite needing
  no VM runs in CI and on the development VM alike. Decided 2026-07-13.
  Amended 2026-10-01.
- **D4, the GUI is wxWidgets through a C++ layer.** The C++ layer,
  compiled by the GUI crate's build script through cxx, is widget glue
  only; Rust keeps `main` and all logic and resolves every string
  (section 11). Why: wxWidgets accessibility is proven in NVDA's own GUI,
  C++ reaches all of it, and the build stays one cargo command. Decided 2026-07-13. Amended
  2026-10-06.
- **D5, the audio device seam.** The mixer (D17) writes to the
  `AudioDevice` trait, and WASAPI implements it. Why: backends can change
  without touching the speech pipeline. Decided 2026-07-13. Amended
  2026-10-04.
- **D6, extensions are Wasm components.** Their WIT-defined API is
  deny-by-default, with capabilities derived from a component's imports,
  never declared; wasmtime is the runtime, named only by `verbatim-ext`,
  and built-in modules run in the outposts and Core, each installed
  extension in a sandboxed host process of its own, never in the listener
  (section 7; [design](design/extensions.md)). Why: unlike NVDA's Python
  add-ons, the surface is enumerable from the binary, and an installed
  extension's escape from the runtime reaches only its own sandbox.
  Decided 2026-07-13. Amended 2026-10-24 and 2026-10-31 (Dickson).
- **D7, braille last.** Braille is implemented last, with its traits and
  pipeline seams present from the start. Why: no hardware is available
  for development. Decided 2026-07-13.
- **D8, one control plane.** Development tooling and the future Remote
  feature share one protocol (section 10), served on two endpoints: a
  narrow user endpoint, always on, and a developer endpoint only in
  developer mode. Why: Remote then rides on infrastructure the end-to-end
  tests exercise daily, and a user's machine exposes only what users'
  features need. Decided 2026-07-13. Amended 2026-10-31 (Dickson).
- **D9, one outpost process per application.** Each application has its
  own outpost; a shared host stays a possible later optimization behind
  the same protocol (section 1). Why: a thread blocked in a cross-process
  COM call can only be abandoned, so process exit is the only full
  recovery, and per-application processes make it surgical. Decided 2026-07-13.
- **D10, localization is Fluent.** Through `i18n-embed`, English is
  compiled in as the fallback, other locales load from a `locale` folder,
  every lookup goes through `fl!`, and no user-visible string is
  hardcoded. Why: Fluent's grammar handling suits spoken messages, and
  translations ship apart from binaries. Decided 2026-07-13.
- **D11, no display model until last.** Screen review projects the
  normalized tree spatially, with character extents where text interfaces
  give them, in M7, and OCR in M8; a GDI display model comes last (M14),
  gated, and only legacy GDI terminals wait for it. Why: it sees only
  GDI-drawn text, a shrinking share of the screen. Decided 2026-07-15.
- **D12, utterances stay structured.** The reducer emits semantic spans,
  and only the pipeline's last stage flattens them through a theme whose
  default speaks and sounds as NVDA does by default, plus Verbatim's own
  cues (section 6; `docs/crates/verbatim-model.md`, "Themes").
  Why: earcons and voice styling become a theme swap. Decided 2026-07-15.
  Amended 2026-10-07.
- **D13, the focus listener.** One permanent listener process holds the
  desktop-wide UIA focus registration and the MSAA hooks. Its callbacks
  make no cross-process call; it reads each UIA focus on a thread per
  application, with no remote operation or connection, and forwards each
  fact to Core, which routes it to the application's own outpost to
  report (section 1). Why: an outpost started after the event would miss
  the first focus, and a hung window holds every UIA event of the process
  with the focus registration. Decided 2026-07-16. Amended 2026-10-20
  (Dickson).
- **D14, attention follows focus.** Attention moves only on a foreground
  change, and the foreground is announced, never a gate; each event rule
  is defined once, judged in Core, outside Core only before a read and
  only from the reader's own observations, and Core orders focuses by
  when they happened (section 2). Why:
  a foreground gate failed broker-hosted applications and background
  events users expect. Decided 2026-09-02. Amended 2026-10-25 (Dickson,
  2026-10-26) and 2026-10-30 (Dickson: no state of Core is sent to a
  reader, [the ownership model](design/ownership.md)).
- **D15, the latency budget is split in two and measured per stage.** The
  OS event to the utterance queued takes 10 ms or less on every backend,
  and queued to first audio 10 ms or less with eSpeak NG, OneCore exempt
  (section 6). Why: the process hops cost under a millisecond, so the
  time goes elsewhere and only a per-stage budget finds it. Decided
  2026-09-02. Amended 2026-10-05.
- **D16, recordings take their audio from Verbatim's own rendering.** A tee
  copies the mixer's output to a WAV muxed with the screen capture, and
  everything audible goes through the mixer (`docs/tooling/e2e.md`). Why:
  recordings then need no audio device and work while heard live. Decided
  2026-09-02. Amended 2026-10-04.
- **D17, every utterance has one truthful ending, measured at playback.**
  Each utterance's speech sequence is plain data with an id, ending once,
  as completed, cancelled or failed; synthesizers only produce PCM, and
  the mixer reports endings and marks when the device plays them
  (`docs/crates/verbatim-speech.md`). Why: say-all and cancellation need
  endings true for every synthesizer. Decided 2026-10-04.
- **D18, native synthesizers run in a synthesizer host process.** Each
  `verbatim-synth-host.exe` runs one synthesizer, OneCore and eSpeak NG
  included, behind the `SynthDriver` trait over a pipe, and loads native
  engines through C-ABI adapter DLLs in an AppContainer (section 6).
  Verbatim is GPL version 3 or later, and eSpeak NG is linked into the
  host only. Why: an engine's crash or hang costs one restartable process,
  never Core. Decided 2026-10-04. Amended 2026-10-22 (Dickson).

## 1. Process and thread model

The founding responsiveness rule: **no thread that produces output (speech,
braille, tones) or handles input may ever make a blocking call into another
application.**

The rule is empirically grounded, not a hunch. A cross-thread
`SendMessage` blocks until the receiving thread processes it, and a COM
call into a single-threaded apartment rides that same queue with no
default timeout, so a hung or busy application blocks any synchronous
caller indefinitely; NVDA's watchdog exists precisely to detect its one
working thread stalling and recovers specifically by cancelling such
calls, and its issue record documents both failure modes (the foreground
app hanging the reader, and busy background apps lagging it through
event floods serialized behind slow synchronous queries). The evidence
is collected in
[Main loop and watchdog](nvda/main-loop-and-watchdog.md); it is the
justification for D9's per-application process isolation and D13's
rule that the focus listener's callbacks never make a cross-process call
(its reads of a UIA focus are made on a thread per application, each
under UIA's call timeout and a watchdog).

Verbatim runs as three kinds of process:

1. **Core process (`verbatim.exe`)**, containing these thread groups:
   - the input hook thread, running the low-level keyboard hook;
   - the reducer thread, running the functional core;
   - the speech and audio threads (pipeline plus WASAPI);
   - the GUI thread (wxWidgets through the D4 C++ layer);
   - the relay thread, which reads every child's pipe on one completion
     port and finishes the writes to them that pended;
   - the global modules' threads (wasmtime with epoch preemption), planned
     for M5; built-in app modules run in outposts and each installed
     extension in a sandboxed host process of its own (D6); `verbatim-ext`
     is a skeleton today.
2. **Outpost processes (`verbatim-outpost.exe`), one per target application**
   (D9), each containing an event thread (WinEvent message loop and UIA
   callbacks), one worker, the only thread that calls into the
   application, a watchdog that abandons and replaces a worker past its
   deadline, and an I/O thread that reads Core's commands, answers pings,
   and finishes the writes to Core that pended; every other thread writes
   its own messages to Core (`docs/design/communication.md`, section 6.4).
3. **Synthesizer host processes** (D18) — one per synthesizer in use, in
   a kill-on-close job, streaming PCM to Core over a pipe; the AppContainer
   sandbox arrives with Eloquence in M6.

A supervisor in Core spawns outposts, tracks their health, and kills and
respawns any that stop responding. Because Windows has no parent-child
process lifetime link of its own, every spawned process is placed in a job
object with kill-on-job-close before it starts running (spawned suspended,
assigned to the job, then resumed), and Core holds the job handles: if
`verbatim.exe` exits for any reason, including a crash, the kernel closes
those handles and kills everything in the jobs. Per-outpost jobs also carry
a per-process memory cap; past it the outpost's allocations fail, the
resulting abort ends the process, and the supervisor respawns it. The synth host's sandbox job (section 6) simply
carries the same kill-on-close flag; control-plane clients such as
`verbatim-inspect` are deliberately not children and not in any job.

Outposts and the listener shut down cleanly whenever they end: Verbatim
exiting, a child replaced or restarted, an outpost retired or ended
because its application exited. The supervisor sends a shutdown message;
the child takes no new work, lets the call or remote operation in
progress finish (UIA's own timeouts end one the application never
answers), removes every UIA event handler and `WinEvent` hook, releases
every object it holds, leaves COM, and exits with code 0. Only a child
that has not exited within the time limit, 21 seconds, longer than UIA's
own call timeouts, is killed through its job, and the kill is logged with
the reason (`docs/crates/verbatim-outpost.md`, "Shutting down"). Killing a
UIA client in the middle of a call is the leading suspect in a crash of a
Windows Terminal it was reading, inside `UIAutomationCore.dll`, on
2026-10-08. The job's kill-on-close stays the guarantee for a
Core that dies.

A child ends itself with `TerminateProcess` once its shutdown is done,
never by returning from `main`. Keep it that way unless the following is
solved: a process that has used UIA as a client sometimes hangs at full
CPU or crashes as it exits normally, inside `UIAutomationCore.dll`'s own
shutdown code, which walks a corrupt list in its telemetry of provider
connections. Releasing every UIA object first does not prevent it. A
process that ends itself with `TerminateProcess` never runs that code,
and by then nothing is left for it to do. The evidence and what is still
unknown are recorded in `docs/crates/mockapp.md` beside the harness's exit
and in the comment of `crates/mockapp/tests/common/harness.rs`.

Core and each outpost (and the synth host) communicate over private
parent-child channels created at spawn via handle inheritance — no named
endpoint exists, so there is nothing to discover or secure. The control
plane (section 10) fronts Core for dev tooling and, later, remote support.

### Outposts

An **outpost** is an actor responsible for one target application: it owns the
accessibility subscriptions for that app (UIA event registrations and a
per-process out-of-context WinEvent hook), maintains the cached/normalized
tree fragment for it, and answers queries about it — from its own process.
Outposts are created when an app first gains focus (or raises a subscribed
event), retired when the app exits, and may be retired early when idle to
bound memory use; state rebuilds from live queries on demand.

Every fact has one owner, one thread or one state machine run by one
thread, and others learn it from one-way reports or views
([the ownership model](design/ownership.md)). Core owns every word and
every fact across applications. Each outpost's reducer owns what its
application remembers: the order of its two routes, verdicts, the gap,
the reports Core may hold and the caret watch. The object store owns node
identity and its release. The hook thread owns the key number. The
threads that call into applications are the focus listener's reading
threads and each outpost's workers; no other thread makes such a call.

Inside, an outpost is one pure state machine, the reducer, on a thread of
its own. Workers run whole call programs on copies of what they read and
post their results back; the reducer's timed wait keeps every timer. What it says to
Core waits in one queue, written by a thread of its own; queuing never
waits, nothing is queued while a lock the watchdog takes is held, and
waiting messages are merged by object and kind, but never a terminal's
reads, each of which starts where the last said to, while answers to Core's requests keep
their order. Intake callbacks
only add entries to one queue, which applies NVDA's limiter rules (one
waiting entry per object and event kind, and for an MSAA event other than
the console host's or a menu opening, per thread that raised it too, of
menu openings only the newest, and per batch the newest four
focus events and ten other events per UI thread). One worker thread takes
the entries in order and is the only thread that calls into the
application, so events and query replies leave the outpost in the order
they were queued. Every query gets exactly one reply: done, gone, failed,
not started, or abandoned. The outpost keeps the live UIA element or MSAA
object behind every node Core still holds, and releases the rest when Core
reports which nodes it holds; a node id names its outpost generation, so
an id from a replaced outpost can never reach its successor.

The outposts are built on four rules, adopted on 2026-10-01 to replace
mechanisms that had each been added for one live failure and covered
cases the others broke (snapshot versions, observation-time checks,
spawn and announce generations, newest-wins slots, a foreground pid
gate):

- Responsibilities are split three ways: outposts and the listener listen
  for events and answer queries, the supervisor manages processes and
  routes messages, and the reducer decides what to speak, which events to
  accept, and what attention is. A decision about presentation never lives
  in an outpost, because only Core sees every application.
- Each correctness rule has exactly one owner, so that changing it cannot
  break a case another mechanism was quietly covering.
- Every queue has a stated overload policy, and no thread that others
  depend on ever waits on one, so a busy application or a full queue
  cannot stall reading or the heartbeat.
- Order replaces version checks. Every hop after an outpost's queue is
  single-threaded and in order, so a version check could only detect a
  race inside the outpost, and dropping an event because a later-numbered
  one arrived first also lost independent events. One worker per
  application gives the order instead.

Parallel queries within one application are given up: MSAA calls, and
UIA calls to most Win32 and WinUI providers, are answered on the
application's UI thread one at a time anyway. The known cost is waiting
behind a long query in the same application, chiefly the diagnostic tree
dump; the remedy, if it matters, is splitting or cancelling long queries,
not adding threads. Identical-focus suppression stays in the reducer,
because an outpost doing it alone would suppress a return to its
application after a visit elsewhere.

Recovery is a ladder, cheapest rung first:

1. **Deadline expiry.** Every entry the worker handles carries a deadline,
   watched by a watchdog thread.
2. **Thread abandonment.** A thread blocked inside a hung app's COM call
   cannot be safely reclaimed: `ICancelMethodCalls` is unreliable and
   `TerminateThread` corrupts the calling process (abandoned locks, broken
   apartment state). So when the worker passes its deadline the watchdog
   abandons it, answers its query "abandoned" if it was running one, and
   starts a replacement worker that continues with the rest of the queue.
   The abandoned thread publishes nothing if its call ever returns, and
   stays parked (roughly a megabyte of stack and a handle) until then or
   until the outpost exits.
3. **Kill and replace.** Every outpost failure takes one path, whatever
   its cause: it crashes, its hook fails, it stops answering pings for
   nine seconds (stuck: the process itself does not answer, which a
   worker waiting on a slow application never causes, since the I/O
   thread answers pings), or it accumulates eight abandoned workers
   (unless its application's windows are reported hung). The supervisor
   ends it, and, if its application is still running, gives the
   application the next spare outpost, or starts one for it, whether or
   not it holds attention, with the facts held for it; Core asks the
   replacement for the current focus where it needs to, and its
   references to the old outpost's nodes are dead. Nothing is ever given
   up: an application's repeated failures are spaced by an exponential
   backoff, 1 second, then 2, 4, up to 30, counted again once an outpost
   has stayed up a minute or the application comes to the foreground, and
   so are the failures of spares while they wait
   (`docs/design/focus-pipeline.md`, section 19.5). The supervisor knows each application by a
   process handle it holds for as long as it has a record for it, so the
   application's pid cannot name another process meanwhile, and no
   outpost is ever started or replaced for an application that has
   exited.
   This is the uniform hard-recovery path — and, because abandoned threads
   are only truly freed by process exit, the only one that reclaims
   everything.

Because outposts are per-app processes, a hang or crash in one app's
accessibility plumbing cannot affect reading any other app, and Core (input,
speech, GUI) is never in the blast radius at all.

### The focus listener (D13)

Per-app outposts have one structural blind spot: the first focus. When an
application gains the foreground, its focus event fires before the
supervisor has started that application's outpost and before the
outpost's hooks are installed, so the event is unobservable by the very
process responsible for it. Reconstructing the missed focus by polling
the new application is not done, because such a poll raced menu popups,
announced half-constructed windows, and fell silent when a slow
application exhausted its retries.

The focus listener closes the blind spot at its cause. It is one permanent
outpost process, supervised like any other (job object, heartbeat,
respawn), that exists from startup and holds exactly the subscriptions
that are global by nature:

- the desktop-wide UIA focus handler, which UIA offers only desktop-wide,
  and the desktop-wide UIA event group NVDA registers globally on
  Windows 11 (element selected, menu opened, notifications), all on one
  client in NVDA's order; one listener holding them, rather than every
  outpost, means one callback per focus change system-wide instead of one
  per outpost;
- global MSAA WinEvent hooks (process id zero) for focus, foreground,
  menu-popup opens, the end of a menu or of the Alt+Tab switcher, alerts,
  and tooltips shown, NVDA's own arrangement. Every foreground change is
  also reported to Core as the window in front, with the thread its event
  names and its process's integrity level, for the keyboard hook's view of
  the foreground (section 5), and the end of a menu or switcher goes to
  Core, which asks the foreground application for its focus if its own
  has not changed shortly after, as NVDA's fake focus does.

The listener's contract: **its callbacks and its writer never make a
cross-process call**; its registration thread's one desktop-wide
registration, made at its start, waits on the foreground application while
UIA reads the system's focus, up to about 6 seconds for one that does not
answer (`docs/design/cross-process-calls-study.md`, instance 2). So the
listener installs its WinEvent hooks first and makes that registration on
a thread of its own: its MSAA facts flow at once, its pipe thread answers
the supervisor's pings throughout, and its UIA events are dropped until
the registration returns, a WinEvent focus whose UIA focus is dropped so
being recovered by the gap record. It says `Ready` only once the
registration has returned, because the listener's `Ready` gates
Verbatim's readiness to take input, and no input may be taken before UIA
focus events can arrive. An MSAA
WinEvent delivers raw window and object ids, which are forwarded
untouched; the only other reads in the callbacks and the writer are
hang-safe local ones (the window's owning process id, its class name, and
for the window in front its process's integrity level, read from the
kernel). No application, hung or not, can stall them.

Detection and announcement are deliberately split. The listener forwards
each captured fact to Core; the supervisor ensures the target's own
outpost exists, giving the application a waiting spare outpost where it
can, so a start merely delays the announcement, and hands the fact to it.
Facts for an outpost still starting are held in arrival order, merged with
NVDA's one-per-object rule, and released in that order when it is ready.
The outpost's worker then acquires, arbitrates, enriches with ancestry and
selection, and reports the focus event, degrading rather than falling
silent when enrichment times out. Node identity never crosses processes:
the navigator, object navigation, and every fetch route to the per-app
outpost, and D9's isolation bounds every call that can block.

All of an application's MSAA events reach its outpost by one route, in the
order Windows raised them: the outpost's process-scoped hook takes in its
application's focus, foreground, menu-popup, alert, and tooltip events
besides value, state, name, selection, and window destruction. The
listener forwards those events only for an application whose outpost's
hook is not yet in, so that an application's first events start its
outpost; once told the hook is in, it says "forwarding ended" behind its
last forwarded fact, and the outpost matches the two hooks' copies of an
event by counting, so each event is handled once and none is dropped.

A UIA focus is judged and read by the listener, because a UIA element
cannot be handed to another process and the listener is the one process
that receives the event's own element. Its callback sends the
application's outpost a marker the moment the event arrives and queues the
element for that application's reading thread, one per application,
created on its first focus event and ended when the application's outpost
ends. The thread judges the focus as NVDA's focus handler does, in its
order: the hung window, whether the element is native UIA, NVDA's
duplicate rule against the focus Core holds, the acceptance test against
the element's nearest window, the object's own refusals, and the element's
`HasKeyboardFocus` read plainly, a focus that has lost it dropped as NVDA
drops it. It then reads the element's value, states, and details afresh,
and its ancestors, one step each, up to the first one known from the
focus before, the desktop's root, or the process boundary, with calls on
elements it holds only. A UIA menu opening takes the same route. The
read, or its drop, follows the marker through verbatim.exe, the only route
between the listener and the outposts. The outpost holds every entry that
arrives after a marker until the read takes the marker's place, so a
change raised after a focus is never handled before it, as in NVDA's
single flusher thread; reports the focus from the read at once, its
ancestors filtered and continued through MSAA where a window is MSAA's;
and only then finds its own copy of the element by the chain's runtime
ids, from the nearest window in the chain, for the subscriptions, text,
and review. Each reading thread has a watchdog: a read past its deadline,
or a focus's or menu opening's read that has taken half a second once the
user has moved on to another thread of its application, is reported
dropped, and a new thread takes the application's later events. An
application whose abandoned threads reach a small cap gets no new thread:
its focus events wait for the first of them to return, until the deadline
after the cap was reached, when every waiting event is dropped, and each
later one as it arrives, until a thread returns. When the listener is
replaced, the supervisor tells every outpost to wait on none of its
markers. `docs/design/focus-pipeline.md` gives the options weighed and the
reasons.

Which backend speaks for a window is a per-window verdict the app outpost
keeps, which each fact consults independently whenever it arrives, so
deduplication does not depend on the UIA and MSAA facts for one focus
arriving together, or at all. For a window with no verdict yet, the fact
resolves the real verdict on the spot, on the outpost's deadline-guarded
worker, so exactly one backend announces, deterministically, in either
arrival order. A UIA fact is never discarded in favour of an MSAA fact
that may follow, because modern XAML surfaces fire no MSAA focus event at
all (the Start menu's search box), and a focus event fires once, with
nothing to retry.

Nothing polls. After the listener is replaced, Core reads the foreground
window itself and asks that application's outpost for its current focus
once; desktop-wide UIA events from the gap are lost. A window that has no
name when focus enters it is not announced later, as in NVDA.

The per-app outposts hold no UIA focus registration. Besides their
process-scoped MSAA hooks, each holds one UIA property subscription that
follows the focus (NVDA's selective registration on Windows 11, on the
focus alone, since the reducer acts only on its changes); selection and
notifications come desktop-wide from the listener. One extra hop
(listener to Core to app outpost) costs well under a millisecond against
the tens of milliseconds acquisition already costs.

Of the listener's threads the reading threads call into applications, each
call one on an element the listener holds, and the registration thread's
registration reads the foreground application's focus, with no remote
operation and no connection by handle. Only an application's own outpost
sends it a remote operation or connects to one of its windows, and an
outpost connects to, probes and walks up into only its own application's
windows, except one connection where the focus or the window hierarchy
changes between its check and its call
(`docs/design/cross-process-calls-study.md`), and it searches for no node
by runtime id, since such a search enters any child window of another
process; it reads
another process's window above the focus through MSAA; object navigation's
step, which can read a neighbor in another process's window through UIA,
is an accepted exception (`docs/design/listener-without-connections.md`,
sections 2 and 4.3). So the UIA defect where a new connection fails while
another process's remote operation is queued in the application
(`docs/uia-bugs.md`) cannot arise between Verbatim's own calls, and no
lock guards them. A slow or hung application holds only the threads that
serve it, with these exceptions: a hung window named in a focus `WinEvent`
holds the listener's UIA events (shared with NVDA; `docs/uia-bugs.md`, "A
hung window named in a system focus event holds a process's UIA events");
a window above the process boundary holds the reading of the focus below
it, in the listener and in that focus's outpost; and the foreground
application holds the listener's UIA events, and so Verbatim's readiness
to take input, at its start
(`docs/design/cross-process-calls-study.md`, instances 1 and 2). That is
why the listener is a process of its own and not threads in one UIA host
beside the outposts: nothing short of a process boundary isolates the
first stall, and in a shared host it would hold every application's UIA
events, terminal output included, for up to 4 seconds per such event.

### Why one process per app

A blocking COM call parks only its calling thread, so thread pools inside a
single shared host process would already handle the *common* hang. (NVDA
hangs today not because a hung call stalls a whole process by nature, but
because its one main thread both talks to apps and runs everything else.)
Per-app processes still win, for three reasons:

- **Reclamation.** Abandoned threads (ladder rung 2) are bounded garbage that
  only process exit frees. One process per app makes that exit cheap and
  surgical instead of a restart that disrupts every app at once.
- **Cross-app blast radius.** A shared host would share process-wide COM/RPC
  state, locks, and apartment message loops across apps — and IA2 loads
  proxy/stub DLLs into the client process, where a crash would take every
  outpost down together. Per-app processes confine all of it to one app.
- **Simplicity.** No heuristics deciding which app "deserves" its own
  process; the recovery ladder is identical for every app.

The cost is working set on low-end devices, since each process carries its
own runtime and COM overhead (risk R2). Mitigations: idle-outpost
retirement, shared code pages from the single outpost binary, and — because
the outpost protocol is location-transparent — the option to *consolidate*
several low-traffic apps into one host process later as a measured
optimization.

## 2. Functional core, imperative shell

The interaction logic is a deterministic reducer living in `verbatim-core`,
which changes the state in place rather than copying it, so a step costs
nothing for the parts of the state it does not touch:

```rust
fn reduce(state: &mut SrState, input: &Input) -> Vec<Effect>
```

- `SrState`: focus context, review cursor, active modes (focus/browse/scan),
  the attention record, the node references it holds (focus, ancestors,
  navigator), speech-relevant config.
- `Input`: normalized accessibility events, gesture invocations, effect
  completions (e.g., a property fetch finishing), timers.
- `Effect`: `Speak(Utterance)`, `Braille(..)`, `PlayEarcon(..)`,
  `Fetch(Query)` (more tree data), `SetHighlight(rect)`, `ExtHook(..)`, etc.

The shell (imperative, async) executes effects: `Fetch` goes to the relevant
outpost and its completion re-enters the reducer as an `Input`; `Speak` enters
the speech pipeline. Consequences:

- **Determinism.** A recorded triple of initial state, input sequence, and
  fetch replies replays to identical effects. The flight recorder (section 9)
  captures exactly these triples from live sessions, turning field bugs into
  unit tests.
- **No blocking in the core.** Anything slow is an effect by construction.
- Outposts have their own small, separately-tested state machine (translating
  raw backend events into normalized ones, maintaining the cache); the
  reducer never sees COM.
- **Only the reducer speaks** (Dickson, 2026-10-24). Every utterance and
  earcon is an `Effect` of `reduce`; the shell's other threads (the
  clipboard, the GUI, the watchdog) turn what they learn into `Input`s and
  never call the speech manager themselves, so all speech is ordered,
  cancellable and recorded in one place. Every speaking function of the
  speech manager takes a token only the reducer thread holds, so the
  compiler checks the rule. A sound file
  the user plays from the settings window to hear it is not speech the
  reducer decides, and plays directly. The speech
  pipeline reports an utterance that carried marks and was cut off or
  failed to the reducer as an `Input`; one that finished says so through
  its last mark. So nothing the reducer waits on is lost by a cut, and a
  finished utterance costs no extra wake-up (Dickson, 2026-10-24). The
  report says which: a cut utterance stops what waited on it, and a failed
  one counts as heard, so say-all and terminal output carry on past it, as
  NVDA's speech manager does when a later index arrives (Dickson,
  2026-10-24).

### Attention and acceptance (D14)

The reducer tracks an attention record: the process and root window that
most recently received a focus fact. Attention moves only on a foreground
change, reported as a focus on the window, which the listener has
confirmed is still the foreground window; an ordinary focus fact is
classified like any other event, so a topmost popup that takes focus does
not take attention. The foreground is announced, never used as a gate.

Acceptance follows NVDA's rules (`docs/parity/focus-events.md`, "Event
acceptance"). A focus is attended when its window, read at the event, is
in the system's foreground window (its top-level window or root owner is
the foreground window, or shares its root owner), is topmost, or is a
`Windows.UI.Core` window under the input thread's active window. It is
judged against the real foreground window rather than the attention
record, because a focus can arrive before the report of the foreground
change that left its window; a focus with no window facts is judged by
its application. Of the alerts, only toasts are reported for now. UIA
notifications are accepted from the attention application only, except
the shell's window-snap results; menu popups, tooltip and
notification-bar classes, configured progress bars and toast alerts are
accepted from anywhere as background. Background events never move focus
or the navigator, are spoken at Queued priority, and are to sit under a
per-source flood cap, not implemented yet.

### Where the rules run (D14)

Each rule (acceptance, a repeated focus, which property changes,
selections and dialog texts are spoken) is defined once, in a
platform-neutral crate unless it is ported from NVDA, in which case once
in the Windows-specific crate the judges before a read share, and every
place that applies it calls that one definition. Words and every fact
across applications are judged in Core. What to read for one application
is judged by its reader, from its own observations of the event, and by
its outpost's verdict rule, which the listener applies only in its
probe-free part (NVDA's MSAA class list and the override), because a UIA
element cannot pass to another process. No state of Core is sent back for
a reader to judge against: an outpost reads changes for every node it has
not released, and Core's targeting decides what is spoken. Everything
decided after the read is decided in the reducer, from facts the
outposts report.

Focuses reach Core from several processes, and a slow read in one can
deliver an earlier focus after a later one. Core orders them by when they
happened on the shared clock, to the microsecond, since focus events
arrive in bursts within one millisecond, so a focus that happened before
the focus Core has applied never replaces it.

## 3. Normalized accessibility model

`verbatim-model` defines Verbatim's own vocabulary — roles, states, properties,
text ranges, relationships, node identity — as a superset that both backends
(UIA and MSAA/IA2) map into. This is the load-bearing abstraction for D1/D2:
the reducer, browse mode, extensions, and all tests speak this model only.

- Node identity: backend runtime IDs and MSAA objects map to `NodeId`s
  issued per outpost generation; a `NodeId` carries its outpost id, so an
  id from a replaced outpost can never name a node in its successor.
- Snapshots are not versioned. Events and replies from one outpost reach
  the reducer in the order the outpost queued them, so the reducer needs
  no staleness check; a node that is no longer reachable answers "gone".
- Synthetic nodes (OCR results, future AI screen recognition, extension-created
  nodes) are first-class citizens of the model, flagged as synthetic, so
  review/navigation work uniformly over real and synthetic content.

## 4. Backend strategy: UIA and MSAA/IA2

Two client stacks feed one model. Per-app (occasionally per-window)
**arbitration** chooses the event and query source: IA2 where offered
(Firefox, Chromium, LibreOffice), UIA where it is the native or only API
(WinUI, Terminal, modern Office surfaces), MSAA as the floor for legacy
Win32. Arbitration lives in the outpost, is config-overridable per app (and
tweakable by app-module extensions), and is invisible above the normalized
model. A third backend, JAB, joins the same arbitration later (D1, roadmap
M13) for Java applications.

### UIA

- **Cache requests everywhere.** Event registrations attach
  `IUIAutomationCacheRequest`s so events arrive with name/role/states/patterns
  prefetched in the same round trip — the single biggest lever against
  blocking on slow apps.
- **Bulk reads carry only cheap properties** (Dickson, 2026-10-24). A read
  of many elements (a tree walk, a list of children, an event
  registration's cache) never asks for an element's location or whether it
  is off screen; those are read only for the one element that needs them.
  For a classic window's title bar and its buttons, either one makes the
  application's UI thread send itself `WM_GETTITLEBARINFOEX`, measured at
  about 6 ms alone and seconds under load while that thread waits. NVDA
  does the same: its base cache request holds neither. Children are listed
  in one call, as NVDA's are, so a child gone before it is simply absent.
  The shell's tray and taskbar list reads whether each button is off the
  screen, to leave hidden ones out, and positions only for a click, of the
  buttons with the clicked item's runtime id or name, normally one
  (Dickson, 2026-10-24).
- **Threading.** Per UIA guidance, event callbacks arrive on dedicated MTA
  threads separate from threads that make UIA calls; each outpost owns both.
  Multiple UIA client threads are a supported, intended part of the design.
- **Remote Operations.** Windows' own remote operations API (the WinRT
  `Windows.UI.UIAutomation.Core.CoreAutomationRemoteOperation`, which NVDA
  calls through its small `UIARemote` shim, having dropped Microsoft's
  `microsoft-ui-uiautomation` library) executes batched operations inside
  the provider process in one cross-process round trip. Wrapped in a `verbatim-uia-rops`
  crate and used for: ancestor-chain retrieval on focus events, bulk text
  attribute runs, terminal text-range walking, and browse-mode buffer batch
  fetches. ARM64 behavior is still to be verified (R3, deferred on
  2026-10-06).
- **Terminals.** TextPattern plus remote ops. The outpost reads the
  terminal and sends each read to Core, whose terminal memory aligns it
  with the last read and whose reading rules decide what it says
  (`docs/design/terminal-model.md`); new output is spoken in order, one
  line per utterance; newer output never cancels older output
  still waiting (decided 2026-10-06, since that rule is what broke NVDA's
  2026.3 beta). The flood policy works by groups: the first "Lines spoken
  in full" lines of a burst (30 by default) are spoken whole, and once
  that group has been heard, when more lines wait than the limit, all but
  the newest "Last lines to speak" become one "skipped N lines", and the
  decision repeats after each group. While Core's queue is full the
  outpost only notes that the terminal changed and reads it on demand,
  when Core asks as it hands a group's last line to speech. Core's waiting
  output is bounded in bytes (10 MB), never an unbounded backlog. Details
  are in `docs/crates/verbatim-core.md`, "Terminals", and
  `docs/crates/verbatim-outpost.md`, "Terminals". This is a headline
  scenario for the latency budget.
- **Constraint: keep a remoted-UIA mode possible.** Windows can present a
  legitimate UIA tree whose process identity and embedded window handles
  are locally meaningless — Application Guard did exactly this (the tree
  forwarded from a container VM through a projection window; NVDA's
  accommodations are documented in `docs/nvda/uia.md`). MDAG is deprecated,
  so nothing is built for it; the standing rule is cheaper: the UIA client
  stack must remain able to operate UIA-only over a subtree without
  dereferencing any native window handle found inside it, and
  handle-dereferencing stays confined to identifiable modules (arbitration,
  window-hierarchy navigation) rather than assumed throughout. If a
  successor technology appears, the work is then an outpost mode (forced
  UIA verdict, anchored to the projection window), not an architecture
  change. The cloud-PC user need itself is served by M12 remote support,
  not by local reading of forwarded trees.

### MSAA / IA2

- **Events** arrive as WinEvents via *out-of-context* `SetWinEventHook`
  scoped to the outpost's process id — asynchronous delivery in our process,
  no injection required.
- **IA2 acquisition**: starting from the `IAccessible` carried by the
  WinEvent, call `IServiceProvider::QueryService` to obtain `IAccessible2`,
  then the IA2 text, hypertext, and relation interfaces as needed
  (`nvda/include/ia2` vendors the IDL). The IA2 proxy/stub (marshaling)
  story from Rust — registered or reg-free — is settled during the first
  IA2 implementation work, which lands with the browsers in M7; before
  then MSAA-only apps are read through plain MSAA.
- **Cost model**: unlike UIA there are no cache requests and no remote ops;
  every property is a cross-process COM round trip. Mitigations, in order:
  fetch discipline (only what the reducer asked for), aggressive outpost-side
  caching keyed to WinEvent invalidations, incremental browse-mode builds,
  and the D2 in-process helper for batching, which lands with browse mode
  (M7).
- **MSAA-only apps** are normalized at "usable" fidelity through the same
  stack (IA2 is a set of interfaces layered on MSAA plumbing).

## 5. Input

A low-level keyboard hook (`WH_KEYBOARD_LL`) lives on a dedicated,
never-blocking thread in Core. Constraint: Windows silently removes hooks that
exceed the `LowLevelHooksTimeout`, so the swallow/pass decision must be made in
microseconds against a read-only, lock-free snapshot of the gesture map
(rebuilt atomically when bindings change). The hook only decides and enqueues,
plus one non-blocking send to the speech manager: every key-down but a few
cancels speech, and Shift pauses and resumes it, as in NVDA, before the key's
gesture is enqueued; gesture semantics run on the reducer thread. The hook
thread holds no other subscription and sends nowhere else (Dickson,
2026-10-24): what it needs to know of the foreground (whether to hold keys
for an elevated window, a console's real thread for its keyboard layout) is
learned by the focus listener, decided in Core, and read by the hook from
Core's lock-free snapshot, so there is one view of the foreground. Gesture maps are user-remappable
and per-app-module overridable, NVDA-style. Touch and mouse tracking come
later but route through the same `Input` type.

Typed text (milestone M4, decided 2026-10-06) also comes from the hook:
each key it passes to the application is translated with `ToUnicodeEx`,
using the foreground thread's keyboard layout and the flag that leaves the
keyboard state unchanged, so a pending dead key, whose state every
translation with the layout shares, is never consumed, and the letter
after it reads as composed; with an input method's layout active, nothing
is translated, since its composition is unknowable from keys. NVDA hears
typed characters from inside the application (`WM_CHAR` and its input
method hooks, through injection), which decision D2 keeps off the
correctness path; the application's own text events were the other
candidate, rejected because they cannot tell typing from a paste or an
autocompletion and echo an input method's composition key by key. The
reasoning is in `docs/crates/verbatim-input-windows.md`, "Typed text".
The hook also reports the caret keys it passes, which the reducer answers
by asking the focus's outpost for the caret once the application has
moved it.

## 6. Speech and audio

Pipeline stages, in order: structured utterance (semantic spans, per D12),
dictionary and symbol processing (per span), presentation (a theme flattens
spans to text, voice changes, and earcons; the default theme speaks as
NVDA does and plays sounds where NVDA plays them by default, per D12),
language tagging, synth driver, PCM, the mixer (D17), and the
`AudioDevice` it writes to.

- **Speech manager**: priority lanes (interrupt/next/queued), index marks with
  callbacks (say-all, braille sync, latency probes), rate/pitch/volume state,
  per-language voice switching. As in NVDA, announcements queue rather than
  interrupt; speech is cut off by a cancel (a key press, a new foreground
  window, entering a menu) and, on each focus change, by dropping focus
  speech whose focus validity no longer holds. Multilingual from the start: utterances carry
  language tags end-to-end.
- **Synth drivers** implement one trait — streaming PCM plus index-mark
  events — regardless of origin, and only ever produce PCM (D17):
  - Built-in: **eSpeak NG**, the default (built from the vendored source
    in the `third_party/espeak-ng` submodule and statically linked into
    the synth host; builds cleanly on ARM64), and **OneCore** (WinRT
    `Windows.Media.SpeechSynthesis` via windows-rs), each run in the
    synthesizer host process (D18).
  - **Wasm synths**: components implementing the `verbatim:synth` WIT world,
    receiving the typed speech sequence and returning PCM. Path for
    source-available synths. Whether they run in Core's extension host or
    in a synth host is decided in M5.
  - **Native synth host** (D18): one process per synthesizer, matching the
    DLL's architecture (x86 under emulation if needed, M6), streaming PCM
    over a pipe. Eloquence adds the AppContainer sandbox in M6. Latency
    budget applies equally (the pipe hop is tens of microseconds). A host
    runs while something uses it (the active synthesizer, a settings
    dialog listing its voices, later an extension holding it) and is
    ended when nothing does; after a crash it is started again with its
    saved settings, and the utterance in flight ends as failed. The pipe
    has a small buffer, so backpressure and cancellation cross it
    unchanged, and stays a pipe rather than shared memory until the
    latency ledger shows the hop matters. A native engine is loaded
    through an adapter DLL implementing Verbatim's C ABI, the
    `SynthDriver` trait as C exports, so the host has no engine-specific
    code; the 32-bit x86 host is a reduced build carrying only the
    adapter loader, and the AppContainer's level is chosen per engine,
    the less privileged form wherever the engine allows
    ([design](design/extensions.md), [plan](design/m6-native-synth-work.md)).
- **Audio**: a mixer with one audio thread that converts every source to
  the device's format, sums them, and tracks playback position per
  utterance (D17), writing to the `AudioDevice` trait; WASAPI
  event-driven shared mode is the device implementation, with a silent
  real-time device for machines without one and for test audio.
- **Latency budget** (enforced by tests, not aspiration): per D15, 10 ms
  or under from event observation to the utterance being queued on every
  backend, and 10 ms or under from queued to the first audio sample with
  eSpeak NG, through an event-driven shared-mode stream at the audio
  engine's standard period with the audio thread registered with MMCSS.
  OneCore is exempt from the second half because it synthesizes whole
  utterances before returning. The process model is not where the time
  goes (three pipe hops cost under a millisecond); the first half is met
  by batching ancestry through UIA remote operations, caching MSAA
  ancestry per window, resolving a UIA element once per event, keeping a
  verdict that a window has a UIA provider for the window's lifetime (a
  verdict that it has none is trusted for 500 ms, NVDA's cache period,
  then probed again, since a busy or starting application can fail the
  probe), and never delaying a control behind its window's announcement.
  The latency ledger records observed, queued, and first audio; the
  per-stage timeline it is to record is observed, routed, worker start,
  backend decided, read, ancestors read, sent, reduced, queued, first
  synthesized sample, and first audio write, and the budgets are asserted
  per stage once it does. eSpeak NG is built in and the default
  synthesizer from the opening work of M4, so the second half is
  measurable from the first text work onward. Every stage is traced
  (section 9).

## 7. Extensions (Wasm)

- Runtime: wasmtime with the component model (per D6, replaceable); host API
  defined in WIT (`verbatim:ext`). Epoch-based preemption means a runaway
  extension is interrupted, never hangs its host. Built-in app modules run
  on outpost workers and global modules on Core threads of their own; each
  installed extension runs in a sandboxed host process of its own (D6;
  [security model](design/security-model.md), X4).
- Capability-gated, deny-by-default: a component's capabilities are
  derived from the interfaces it imports, never declared (tree-query,
  event subscription, speech/braille output, gesture binding, config,
  namespaced storage, OCR; each is a separate grant surfaced to the user).
- Extension kinds: **app modules** (activated per application, mirroring
  outpost lifecycle — this is where Office- and browser-specific behavior
  lives; terminal behavior is not here but generic, in the core, keyed by
  the control's UIA class or the console's window class as NVDA's is, so
  app modules adjust only one terminal application's quirks), **global
  extensions**, **synths**, later braille drivers and
  OCR/recognition providers.
- App modules hook the pipeline at defined points: adjust presentation of a
  node, add synthetic nodes, handle gestures, react to events. Hooks have
  deadlines; a slow extension degrades itself, not Verbatim.
- Until the extension host runs modules, an application's own rules
  ported from NVDA's app modules live in Core, never in the listener or an
  outpost (D6).
- Iteration-friendly by design: extensions hot-reload without restarting
  Verbatim; first-party app modules live in this repo and dogfood the API.
  NVDA add-on binary compatibility is a non-goal, but the host API is grown
  by porting real app modules and add-ons, not speculation.
- OCR: `Windows.Media.Ocr` behind a capability; results appear as synthetic
  subtrees. Future AI screen recognition slots into the same synthetic-node
  provider interface.

## 8. Browse mode and scan mode

One mechanism: a **document projection** over the normalized tree — a
virtual buffer (text plus node map) built *incrementally* by a background
builder prioritized around the caret/focus viewport. Navigation works in the
built region immediately; moving into unbuilt territory triggers on-demand
expansion. This delivers the "interact with large pages before full render"
requirement, and because it projects the normalized model (not browser
internals), the same machinery provides Narrator-style scan mode in ordinary
apps. Screen review is the same projection specialized to geometry (D11):
visible nodes ordered by bounding rectangle and grouped into visual lines,
extent-backed where text interfaces exist, with OCR (M8) and eventually the
display model (M14) as further text sources behind the same review commands.

## 9. Observability

- `tracing`-based structured spans with a **trace ID minted when an OS event
  or keypress is first observed** and carried through the outpost, the
  reducer, the speech queue, the synth, and audio submission. For any
  utterance, the timeline — event observed, speech queued, audio started —
  is a single query.
- A **flight recorder** keeps a window of recent reducer inputs, bounded by
  count and by bytes, with a snapshot of the state taken at a checkpoint
  just before the window's oldest input (the replayable triples of
  section 2), and later spans; a crash or user-triggered request dumps it
  for offline replay. The window advances a segment at a time, each
  starting at a checkpoint, so every recorded window replays from its
  start; the snapshot is cheap because the state shares everything that
  grows.
- `verbatim-inspect`: dev tool over the control plane — live event stream,
  tree dumps, gestures pressed as keys, speech capture, latency histograms.

## 10. Control plane (dev tooling, then remote support)

One protocol (JSON-RPC over named pipe locally; TLS WebSocket for remote)
exposes: event/speech streams, tree queries, gesture and input injection,
config, and lifecycle. Locally it is served on two endpoints: a user
endpoint, always on, with the extension install request and status, and a
developer endpoint with everything else, only in developer mode
([security model](design/security-model.md), C1). The E2E harness and
`verbatim-inspect` are its first clients, so it is battle-tested long before
the Remote feature (which is pairing, auth, and UX on top: speech mirroring
out, gestures in). Secure-desktop and permission rules apply to remote
sessions.

Transport notes. The local transport is a *named* pipe deliberately:
control-plane clients — `verbatim-inspect`, the E2E in-guest agent, later a
remote-support session — attach to an already-running Verbatim and are not
its children, so they need a rendezvous point (anonymous pipes can only be
passed to spawned children by handle inheritance, and additionally lack
overlapped I/O). The endpoint is secured the standard way: a security
descriptor restricting access to the owning interactive user,
`PIPE_REJECT_REMOTE_CLIENTS` to keep it local-only, and no control plane at
all in `--secure` instances. Core's internal channels to outposts and the
synth host are the parent-child case and use inherited handles instead
(section 1): pipes made with no name at any moment, as the standard
library makes its child pipes, overlapped on Core's side, so no ACL
surface (`docs/design/communication.md`, section 4.1). Remote access never reuses the pipe;
it is the separate, opt-in TLS transport with pairing.

## 11. GUI

A wxWidgets settings UI on its own thread in Core, built as the D4 C++
layer over Rust-owned logic. The M1 prototype's defining test is Verbatim
reading its own GUI via ordinary UIA through a real outpost process — no
self-voicing side channel, no in-process shortcut — which validates the
UIA stack and the outpost architecture end-to-end (and avoids same-process
UIA client/provider hazards). NVDA reading the same dialogs is the source
of their expected readings and the before-and-after check for the port
from wxDragon: Verbatim runs silenced with the test-audio setting, the two
readers use different modifier keys, and keys are injected as real OS
input.

## 12. Security posture

The threat model and its mitigations are in
[the security model](design/security-model.md).

- No injection unless and until the D2 helper ships (M7), and then only via
  it.
- Native synth DLLs and installed Wasm extensions run in sandboxed processes
  of their own; Core never loads third-party native code in-process.
- **UIAccess**: reading elevated apps' UI from a non-elevated Verbatim
  requires `uiAccess=true`, which requires signed binaries in a trusted path —
  an installer/signing prerequisite tracked from the start (R5).
- **Secure desktop**: register as an Assistive Technology so Windows launches
  a minimal Verbatim instance (same binary, `--secure`: only the extensions
  the user allowlisted for secure screens, no control plane, an
  administrator-made read-only config copy) on sign-in/UAC desktops.

## 13. Testing strategy

Layered so that LLM-driven development gets fast, deterministic feedback:

1. **Reducer unit tests** (pure, milliseconds): scripted inputs plus fake
   fetch replies against in-memory trees; assert states and effects. Replayed
   flight-recorder captures become regression tests.
2. **Provider-level fakes** — the answer to "can tests fake the UIA calls?"
   is yes, with full fidelity: a `mockapp` test host implements real UIA
   *provider* interfaces (`IRawElementProviderSimple` and related) exposing a
   scripted tree in another process. Verbatim's genuine UIA client stack
   consumes it cross-process; tests assert the normalized tree and emitted
   events match expectations. This exercises COM marshaling, cache requests,
   and event plumbing with zero real applications, and runs on plain CI
   Windows runners. A sibling mode exposes the same scripted trees as
   MSAA/IA2 providers, so both client stacks — and the arbitration logic —
   are testable without real apps.
3. **Pipeline tests** with a **capture synth** (records utterances plus
   timestamps instead of producing audio) — assertions on what would be
   spoken, plus latency assertions against the budget.
4. **E2E against a real Windows session** (section 14): real Windows, real
   apps (Notepad, Explorer, Terminal, Edge, Office), driven via the control
   plane, on a hosted CI runner (whose jobs run in an interactive desktop
   session, as NVDA's own system tests do there), a developer's own
   machine or VM, or a
   local Hyper-V VM (D3);
   speech asserted on the control plane's speech stream, spoken by
   eSpeak NG through the silent real-time device in a silent run or the
   real device in an audible one; every `cargo xtask vm test` run is
   audible, playing its speech through the real `WasapiDevice`.

Expected behaviour comes from NVDA, used as a reference rather than as an
oracle in the tests: a small NVDA add-on captures what NVDA speaks for a
scenario driven by real OS input, and that transcript is read interactively
to decide what Verbatim should do and to write Verbatim's own assertions by
hand, with legitimate divergences decided case by case and recorded in the
parity ledger. Verbatim's tests never compare against NVDA output, and NVDA
is never run in CI.

## 14. VM harness (the interactive loop, per D3)

`cargo xtask vm <cmd>` drives a Windows guest behind a `Host` trait that
covers hypervisor lifecycle only: exists, start, stop, snapshot, restore,
delete, and the guest's address. `HyperVHost` is the only implementation,
kept for contributors who test in a local VM; the maintainer runs the suite
runner-direct on a development VM instead (D3). A fake implementation
drives the verb logic in unit tests. Everything that
touches the guest's contents goes over standard transport rather than a
channel: PowerShell Direct for file copies and remote commands, and the
in-guest agent over TCP for the three things a remote channel cannot
do (launching in the interactive session, reporting session facts, and
tunnelling the control-plane pipe). The guest's provisioning script is
hypervisor-agnostic: autologon, the agent as an at-logon interactive
scheduled task, and the runtime prerequisites. Verbs: `create` (Packer builds the base image from an
unattended install, then the VM is imported and snapshotted as a golden
image), `start`/`stop`/`restart`/`restore [snapshot]`, `deploy`
(artifacts copied in over PowerShell Direct and the agent restarted), `test` (deploy,
then run the E2E suite through the agent, audible by default with real
eSpeak NG speech and the real WASAPI device, every scenario recorded as an
mp4 as in every other mode), `logs`, `connect`, `delete`. A restore is only ever
explicit, through `restore` or an opt-in flag on `test`; nothing is
installed into the guest by a run, so an ordinary run has nothing to undo.

Audio: recordings take their audio from Verbatim's own rendering (D16), so
no virtual audio device is provisioned and a run can be heard live over
RDP or locally while it is recorded. A runner-direct run plays through
the machine's own audio device. The one remaining RDP caveat is video, not
audio: the desktop stops rendering in a disconnected session, so screen
capture needs the session attached, or a headless run. Loopback capture
from a virtual audio device is not used, because it made recording and
listening mutually exclusive.

Interactive-session rule: Verbatim, the agent, and screen capture only
work in a session with a visible window station. SSH, WinRM, PowerShell
Direct, and services run in session 0 and cannot see the desktop, which
is why the agent is started at logon by a scheduled task rather than by
any remote channel, and why both the agent and `verbatim.exe` check at
startup that their window station is interactive and refuse to run
otherwise.

## 15. Crate map

- `verbatim-model` — normalized tree, events, effects, `NodeId`; no I/O deps.
- `verbatim-config` — configuration: `settings.toml` next to the executable
  is the base configuration (global settings plus the base profile's
  sections, speech first), and a `profiles` folder holds named profiles as
  sparse overlays, mirroring NVDA's base-plus-diffs model. Profiles cannot
  carry global settings by construction.
- `verbatim-core` — the reducer, modes, review cursor; the browse-mode
  projection is planned (M7).
- `verbatim-uia` and `verbatim-uia-rops` — UIA client stack; remote
  operations.
- `verbatim-ia2` — MSAA/IA2 client stack (WinEvents, IAccessible2).
- `verbatim-jab` — Java Access Bridge client stack (planned, M13).
- `verbatim-outpost` — the outpost actor and per-app outpost binary, plus the
  Core-side supervisor.
- `verbatim-process` — contained child processes (kill-on-close job,
  inherited pipes, per-launch logs), shared by the supervisor and the
  synthesizer host.
- `verbatim-control` — control-plane protocol and server.
- `verbatim-speech` and `verbatim-audio` — pipeline; the mixer and the
  `AudioDevice` seam.
- `verbatim-audio-wasapi` — the WASAPI device.
- `verbatim-synth-*` — eSpeak NG (the default, built from the vendored
  `third_party/espeak-ng` source), OneCore, and capture (test) drivers,
  `verbatim-synth-host`, the synthesizer host process (D18), and
  `verbatim-synth-hosted`, the Core-side driver that runs one.
- `verbatim-ext` and `verbatim-ext-api` — skeletons of the wasmtime host
  and of the WIT worlds plus guest SDK, planned for M5.
- `verbatim-text` — text segmentation for the text model: characters,
  words, sentences, and lines by Unicode's rules through ICU4X, and
  terminal cell widths.
- `verbatim-i18n` — Fluent localization (D10): embedded English fallback,
  runtime locale-folder loading.
- `verbatim-input` — the pure decision machine, key names, gesture maps.
- `verbatim-input-windows` — the hook thread.
- `verbatim-gui` — settings UI: Rust logic plus the D4 C++ wxWidgets layer
  compiled from its build script.
- `verbatim-app` — `verbatim.exe` composition root.
- `verbatim-inspect`, `mockapp`, `xtask` — dev tool; UIA/IA2 provider fake;
  automation.
- `verbatim-agent` and `verbatim-e2e` — the in-guest test doorway and the
  end-to-end scenario suite that drives it (section 13).
- `verbatim-trace-waits` — a measurement tool run by `xtask`: a reader of
  WPR traces that reports how long threads waited to run.
- `workspace-hack` — generated by `cargo hakari`; unifies third-party
  dependency features across the workspace, and has no code.

## 16. Top risks

- **R1 — Out-of-process IA2 chattiness.** No cache requests or remote ops
  means large browser documents could make browse-mode builds feel slow.
  Mitigation: fetch discipline and incremental builds first; the D2
  injection helper lands inside M7 itself (out-of-process first, then the
  helper), and the M7 exit's fixed-corpus measurement against NVDA verifies
  the result.
- **R2 — Outpost process overhead.** One process per app costs working set
  and spawn latency, which matters on low-end devices. Mitigation: idle
  retirement, pre-spawning on foreground change, working-set and
  spawn-latency measurement on the harness VM in M3 (no dedicated low-end
  VM profile; user reports drive any deeper investigation); consolidation
  into shared hosts as the fallback (D9).
- **R3 — Remote-ops on ARM64.** API limits or behavior differences.
  Mitigation: verify on ARM64 before relying on remote operations there.
  The verification was planned alongside the first terminal work (M4) and
  was deferred on 2026-10-06: M4 is built and verified on x64 only, and
  the ARM64 verification is not yet scheduled to a milestone.
- **R4 — Eloquence complications.** DLL architecture or licensing issues.
  Mitigation: native host is already arch-flexible; the PoC has its own
  milestone (M6), so a complication delays only Eloquence, not the host,
  and browse mode (M7) does not wait on it.
- **R5 — UIAccess signing.** Signing requirements could block reading
  elevated UIs during development. Mitigation: develop with test-signing in
  the VM; production signing tracked as release infra.
- **R6 — Screen curtain on Win11/ARM64.** Magnification-API behavior needs
  verification. Mitigation: verify when overlays land (M8); the overlay
  design (DirectComposition) is independent of it.
