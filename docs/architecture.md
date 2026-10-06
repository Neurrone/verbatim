# Verbatim Architecture

Verbatim is a screen reader for Windows 11 (x64 and ARM64, both first-class),
written in Rust. It is informed by NVDA (vendored under `nvda/` for reference)
but is not constrained by NVDA's architecture.

Documentation convention: these docs avoid ASCII-art diagrams, box drawings,
and arrow chains, and prefer lists over pipe tables, so they read well with a
screen reader in both rendered and source form.

## 0. Decisions of record

- **D1 — Dual accessibility backends from the start.** UIA and MSAA/IA2 are
  both first-class client stacks behind the normalized tree model, with
  per-app arbitration picking the richest source (as NVDA does). JAB is
  committed but lowest-priority among the backends, landing after UIA and
  MSAA/IA2 are solid (roadmap M13). Rationale: UIA is the only API for WinUI, Terminal,
  and modern-Office surfaces, but Firefox and Chromium expose their richest
  tree via IA2, and much of Win32 — including parts of Windows itself — is
  MSAA/IA2-only. Neither API alone covers the desktop.
- **D2 — Injection is planned but never required for correctness.** An
  in-process injection helper (NVDA's nvdaHelper analog) ships with browse
  mode (roadmap M6), staged inside that milestone: out-of-process IA2 lands
  first to prove correctness, then the helper adds IA2 call batching and
  virtual-buffer acceleration for performance. NVDA has already proven that
  in-process access is what makes large browser documents fast enough, so
  the helper is scheduled work rather than gated on new measurements; the
  fixed-corpus comparison against NVDA (M6 exit) verifies the result.
  Keeping the helper off the correctness path keeps the x64/ARM64EC/x86
  binary matrix and antivirus friction out of the early milestones.
- **D3 — The end-to-end suite runs against any Windows session the
  harness can reach; continuous integration uses GitHub-hosted runners
  only.** The suite takes an address and talks to the in-guest agent, so
  the same scenarios run runner-direct on a hosted Windows runner (whose
  jobs execute in an interactive desktop session, the same way NVDA runs
  its own system tests there), runner-direct on a developer's own machine
  or VM, or against a local Hyper-V VM. The maintainer develops in a single
  Windows VM on a Proxmox host and runs the suite runner-direct there;
  there is no Proxmox backend for the harness. Hosted runners
  are the only CI: the end-to-end suite runs there silently on every
  change, eSpeak NG speaking into the silent real-time device,
  recordings included (section 14),
  and NVDA is never installed or run in CI. Everything else, including
  audible runs against a real audio device, is the interactive loop, not
  CI. There are no self-hosted runners and no nested virtualization on
  hosted runners. Guest transport is standard tooling, not bespoke
  protocol: PowerShell Direct carries file copies and remote commands
  into the Hyper-V guest (OpenSSH was planned for other hypervisors,
  which are no longer planned), and the agent keeps only what a remote
  channel cannot do, which
  is launching processes in the interactive session, reporting session
  facts, and tunnelling the control-plane pipe. Hypervisor backends handle
  lifecycle and snapshots only, and a snapshot restore is always an
  explicit request, never something a test run does on its own. Amended
  2026-09-02; the original D3 deferred CI because hosted runners cannot
  nest virtualization, which stopped mattering once the suite no longer
  needed a VM to run. Amended again 2026-10-01: the planned Proxmox
  backend and second test VM were dropped in favour of runner-direct runs
  on the maintainer's development VM.
- **D4 — GUI is wxWidgets through a minimal C++ layer compiled from the
  GUI crate's build script via cxx; Rust keeps `main` and all logic.**
  Rationale: wxWidgets accessibility is proven in exactly this role, since
  NVDA's own GUI is wxPython, and a C++ layer gives full access to it
  (`wxAccessible` subclassing for accessible descriptions, accelerator
  tables) where the wxDragon binding used before 2026-09-02 did not.
  Keeping `main` and the build in cargo preserves the single-command
  build, deploy, and the end-to-end suite unchanged;
  the static wxWidgets build recipe for both architectures is adapted from
  the wxdragon-sys build script rather than written fresh. The C++ layer
  is widget glue only: Rust calls C++ functions to run the event loop and
  to build, show, raise, and close widgets from typed page models, and C++
  calls an opaque Rust `GuiCore` when the user acts. Every string crosses
  already resolved, so C++ never sees a Fluent message id. Amended
  2026-10-06, when the port was built; the earlier wording had a
  callbacks object owned by Rust drive the dialogs.
- **D5 — Audio backend is WASAPI behind an `AudioSink` trait.** Rationale:
  allows alternate backends without touching the speech pipeline. Since
  D17 the seam is the `AudioDevice` trait behind the mixer: the mixer
  writes to it, and WASAPI is its device implementation.
- **D6 — Extensions are Wasm components.** The WIT-defined API is the durable
  contract, capability-gated and deny-by-default. wasmtime is the default
  runtime choice, kept behind the extension-host seam so it stays
  replaceable; the final call is ratified by the M5 spike. Closed-source
  synths get a separate sandboxed *native* host process. Rationale: replaces
  NVDA's unbounded Python add-on surface. wasmtime specifically: reference
  implementation of the component model, epoch preemption (a runaway
  extension cannot hang Core), first-class Rust embedding, Bytecode Alliance
  security pedigree. Wasmer, WAMR, and wasmi currently lag on component-model
  support or embedding fit.
- **D7 — Braille is designed for but implemented last.** Traits and pipeline
  seams exist from the start. Rationale: no hardware available for
  development.
- **D8 — Dev tooling and the future Remote feature share one protocol** (the
  "control plane"). Rationale: the remote feature then becomes mostly UI plus
  auth on top of infrastructure that E2E tests exercise daily.
- **D9 — One outpost process per application.** Outposts are separate
  processes by default; consolidating several apps into a shared host is a
  possible later optimization behind the same location-transparent protocol,
  not the starting point. Rationale: a thread blocked in a cross-process COM
  call cannot be safely cancelled or killed, only abandoned, so process exit
  is the only recovery that reclaims everything; per-app processes make that
  exit surgical, remove promotion heuristics, and confine crashes in
  in-client proxy DLLs to one app. Details in section 1.
- **D10 — Localization is Fluent, via `i18n-embed`.** English resources are
  compiled into the binary as the permanent fallback; other locales load at
  startup from a `locale` folder next to the executable, decoupling
  translation updates from binary releases. Message lookups go through the
  compile-time-checked `fl!` macro, and no user-visible string may be
  hardcoded anywhere in the workspace. Rationale: Fluent's grammar handling
  (plurals, gender, selectors) matters for speech-quality messages, and the
  ecosystem supports the pseudo-locale testing required from M1.
- **D11 — No display model until last, and screen review does not wait for
  it.** Screen review is a spatial projection of the normalized tree —
  visible nodes ordered by bounding rectangle and grouped into visual
  lines, extent-backed wherever a text interface exposes per-character
  geometry (UIA TextPattern bounding rectangles, IA2 character extents),
  rectangle interpolation otherwise — landing with the browse-mode
  projection machinery (roadmap M6), with OCR as a second text source (M8).
  A GDI display model in the NVDA tradition (in-process hooks on GDI
  text-output calls) is deliberately last (M14) and behind a re-triage
  gate, riding the M6 injection helper as its delivery vehicle; the only
  work gated on it is the legacy GDI terminal clients (PuTTY, SecureCRT,
  Tera Term). Rationale: a display model sees only GDI-drawn text, a
  shrinking share of a Windows 11 screen, while tree-plus-extents is exact
  wherever a text API exists — including the Chromium, WinUI, and
  DirectWrite surfaces where a GDI display model sees nothing at all.
- **D12 — Utterances stay structured until the last pipeline stage.** The
  reducer never emits flattened strings: an `Utterance` is a sequence of
  semantic spans — label, role, value, state, description, attribute-tagged
  text runs — and a presentation stage at the end of the speech pipeline
  flattens spans to text through a theme, where the default theme
  reproduces plain speech. Rationale: earcons and voice styling for roles
  and formatting (roadmap M11, in the Emacspeak audio-formatting tradition)
  become a theme swap rather than a pipeline rewrite, and dictionary and
  symbol processing operate on typed spans rather than undifferentiated
  text.
- **D13 — A dedicated focus-listener outpost detects focus; per-app
  outposts announce it.** One permanent, stateless listener process holds
  the single desktop-global UIA focus registration and global MSAA
  WinEvent hooks (focus, foreground, menu popups), under a hard rule: it
  never makes a cross-process call — it reads only what the event itself
  carries (a UIA element's cached properties, an MSAA event's raw window
  and object ids) plus hang-safe local reads, and forwards each captured
  focus fact to Core, which routes it to the target's own outpost for
  acquisition, arbitration, enrichment, and announcement. Rationale:
  per-app outposts structurally race first focus — the event fires before
  the newly foregrounded app's outpost has spawned and hooked — and the
  synthetic announce-by-polling that papered over the race was root-caused
  live (milestone M3) as a class of silent failures: menu-open races,
  cold-start "window window" noise, and announcements silently never
  produced when the poll's budget exhausted on a loaded machine. A
  listener that exists from startup and cannot block on any application
  never misses the event, so the poll demotes from primary mechanism to
  rare fallback; announcements keep flowing through the per-app outposts,
  so node identity, backend arbitration, and D9's isolation for every
  query are unchanged. Details in section 1.
  Amended 2026-10-01 (the outpost redesign): the announce poll is removed
  rather than kept as a fallback; after a listener or outpost restart Core
  asks the attention application for its current focus once instead. The
  listener also holds the alert hook and desktop-wide UIA subscriptions
  for selection, menu opening, and notifications, coalesces its facts
  with NVDA's limiter rule (one waiting fact per element and kind), and
  may start an outpost only for focus, foreground, menu, notification, and
  alert facts. Outposts lose their window-scoped UIA subscriptions in
  favour of one property subscription that follows the focus and its
  ancestors.
  Amended 2026-10-05: the listener stays the only receiver of focus
  events; outposts do not subscribe to their own, so no event is handled
  twice and no ordering between two sources is needed. NVDA judges a UIA
  focus against the foreground using the element's nearest window, a
  cross-process walk the listener may not make. Instead, for an element
  with no window of its own, the listener sends the foreground thread's
  keyboard focus window when it belongs to the element's process, a
  local read that names the window hosting the element when the event is
  current (for a late event in an application with several windows it
  can name another of them, a known limitation). The outpost, which
  must read the focused element again because a UIA element cannot cross
  processes, drops a fact whose focus has meanwhile moved to another
  application, and reports one it cannot resolve with the window the
  listener found, so Core judges it against the foreground window (D14).
- **D14 — Attention follows focus; foreground is announced, not used as a
  gate.** The reducer tracks an attention record: the process and root
  window that most recently received a focus fact. It replaces the earlier
  rule that dropped every event whose process did not own the foreground
  window, which failed for broker-hosted applications (a Settings page
  lives in the Settings process while its frame belongs to the frame
  host), for windows of the already-running shell that never take the
  foreground cleanly (an Explorer folder window), and for background
  events users expect to hear. Event acceptance moves out of Core into the
  outposts, which classify each event as attended or background with
  hang-safe local reads following NVDA's `shouldAcceptEvent` rules: the
  attention window, its descendants, windows sharing its root owner,
  topmost windows, and the `Windows.UI.Core` case are attended; UIA
  notifications, toast alerts, menu popups, tooltip and notification-bar
  classes, and configured progress bars are accepted from anywhere as
  background. Background events never move focus or the navigator, are
  spoken at Queued priority, and sit under a per-source flood cap (not
  implemented yet; `docs/parity.md` lists it). So that
  processes which never held focus can still be heard, the focus listener
  additionally hooks `EVENT_SYSTEM_ALERT` and a desktop-wide UIA
  notification registration and forwards them as facts that spawn an
  outpost on demand, which stays within its never-block rule because both
  are cached reads. Ratified 2026-09-02.
  Amended 2026-10-01 (the outpost redesign): acceptance is classified by
  the reducer, not the outposts. Outposts attach window facts read with
  local calls (top-level window, root owner, topmost, and for
  `Windows.UI.Core` windows whether the window is under the input
  thread's active window); the reducer compares them with its attention
  record, so attention is never broadcast to outposts. Attention moves
  only on a foreground change, reported as a focus on the window, which
  the listener has confirmed is still the foreground window; an ordinary
  focus fact is classified like any other event, so a topmost popup that
  takes focus does not take attention. UIA notifications are accepted
  from the attention application only, except the shell's window-snap
  results. Of the alerts, only toasts are reported for now. The attention
  model is implemented in this redesign instead of at M4.
  Amended 2026-10-05: a focus event is attended only when its outpost
  found its window in the system's foreground window when it read the
  event (its top-level window or root owner is the foreground window, or
  shares the foreground window's root owner), or the window is topmost,
  or it is a `Windows.UI.Core` window under the input thread's active
  window; that is NVDA's own test, made against the real foreground
  window rather than the attention record. A focus with no window facts
  is still judged by its application. A focus can reach Core before the
  report of the foreground change that left its window (found live: the
  desktop's list raised a focus 4 ms after Notepad became the foreground,
  and Verbatim spoke it), and judged against the attention record it was
  attended. Foreground changes and the other events keep their rules.
- **D15 — The latency budget is split in two and measured per stage.**
  From observation of the OS event to the utterance being queued: 10 ms
  or under on every backend. From queued to the first audio sample: 10 ms
  or under with eSpeak NG, through an event-driven shared-mode stream at
  the audio engine's standard period and MMCSS registration of the audio
  thread;
  OneCore is exempt from this half because it synthesizes whole utterances
  before returning. The process model is not where the time goes (three
  pipe hops cost under a millisecond); the cost is the per-hop ancestor
  walk, cold arbitration probes, and lane serialization, so the first
  half is met by batching ancestry through UIA remote operations, caching
  MSAA ancestry per window, giving arbitration verdicts the window's
  lifetime, and never delaying a control behind its window's announcement.
  The latency ledger records a stage timeline (observed, routed, lane
  start, verdict, acquired, enriched, emitted, reduced, queued, first
  synth sample, first audio write), and the budgets are asserted per
  stage once eSpeak is in. Ratified 2026-09-02.
  Amended 2026-10-01 (the outpost redesign): there are no lanes; each
  outpost has one worker, so the planned outpost stages become routed,
  worker start, backend decided, read, ancestors read, and sent. The
  arbitration verdict is kept for the window's lifetime and dropped when
  the window is destroyed, and a UIA element is resolved once per event.
  Amended 2026-10-02: only a verdict that the window has a UIA provider
  is kept for its lifetime. A probe that finds none is trusted for 500 ms,
  NVDA's cache period, and then repeated, because a busy or starting
  application can fail the probe for a window that has a provider (found
  live with Windows 11 Notepad, whose edit control was then read through
  MSAA for the rest of its life).
  The ledger still records only observed, queued, and first audio; the
  per-stage timeline is not implemented yet.
  Amended 2026-10-05: the second half no longer relies on `IAudioClient3`.
  The development machine's device offers no shared-mode period shorter
  than the engine's standard 10 ms, and queued to first audio measured
  3 to 8 ms in the usual case without it, most of that the engine period.
- **D16 — Recordings take their audio from Verbatim's own rendering.** A
  tee at the audio output writes every utterance's PCM with its
  wall-clock start time while still playing it, and the recording step
  muxes that track with the screen grab. No virtual audio device is
  involved, so recordings work on a hosted CI runner with no sound device,
  on any hypervisor, and while the run is being heard live over RDP or
  locally. Consequently everything Verbatim makes audible, earcons and
  tones included, is rendered as PCM through that audio output and
  mixed there, never through a separate path. Ratified 2026-09-02.
  Amended 2026-10-04: the tee copies the mixer's output (D17), so every
  stream Verbatim mixes is recorded together, to a WAV file beside the
  screen grab on the agent's machine, silence-filled by the clock so it
  runs in step with real time; the recording step encodes it as the
  video's AAC audio track. The screen grab is ffmpeg's desktop capture, launched
  through the agent for every scenario on every path (runner-direct,
  hosted CI, and the Hyper-V harness alike), and VB-CABLE is retired.
- **D17 — Every utterance has one truthful ending, measured at playback.**
  The theme flattens an `Utterance` into a speech sequence: text pieces
  mixed with commands (index mark, pitch, rate, volume, language,
  character mode, pause), plain serializable data that can cross a
  process, Wasm, or network boundary unchanged, designed so that a sound
  item can join it when earcons arrive (sounds will start at their place
  in the sequence and overlap the speech that follows, belonging to the
  utterance). Each utterance carries its own id and ends exactly once, as
  completed, cancelled, or failed, including utterances cleared from a
  lane before they were synthesized. A synthesizer only produces PCM and
  never plays it; Verbatim's audio output is a mixer with one audio
  thread, a buffer per source, and conversion of every source to the
  device's format, and it tracks which samples belong to which utterance.
  So the end of an utterance, and each index mark, are reported when the
  device has played that sample, for every synthesizer without its
  cooperation. A synthesizer that cannot place marks in its audio has its
  sequence split at the marks by the speech manager, so mark positions
  are exact for every backend. Leading and trailing silence is trimmed
  centrally for every synthesizer, except a pause the sequence asks for.
  Decided 2026-10-04 (phase 4 of the 2026-09-02 handoff).
- **D18 — Native synthesizers run in a synthesizer host process.**
  `verbatim-synth-host.exe` runs one synthesizer per process, OneCore and
  eSpeak NG included, behind the same `SynthDriver` trait; in Core,
  `HostedSynth` implements that trait by forwarding requests over a
  private pipe with a small buffer, so backpressure and cancellation
  cross the boundary unchanged and audio is still rendered by Core's
  mixer. A host runs while something uses it (the active synthesizer, a
  settings dialog listing its voices, later an extension holding it), is
  ended when nothing does, sits in a kill-on-close job like the outposts,
  and is started again with its saved settings after a crash, the
  utterance in flight ending as failed. Pipes, not shared memory, until
  the latency ledger shows the hop matters. The AppContainer sandbox and
  the 32-bit host for Eloquence remain M7 work. Verbatim is licensed GPL
  version 3 or later from the same date, and eSpeak NG (GPL version 3 or
  later) is statically linked into the host, not into `verbatim.exe`.
  Decided 2026-10-04.

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
never-make-a-cross-process-call rule for the focus listener.

Verbatim runs as three kinds of process:

1. **Core process (`verbatim.exe`)**, containing these thread groups:
   - the input hook thread, running the low-level keyboard hook;
   - the reducer thread, running the functional core;
   - the speech and audio threads (pipeline plus WASAPI);
   - the GUI thread (wxWidgets through the D4 C++ layer);
   - the extension-host threads (wasmtime with epoch preemption).
2. **Outpost processes (`verbatim-outpost.exe`), one per target application**
   (D9), each containing an event thread (WinEvent message loop and UIA
   callbacks), one worker, the only thread that calls into the
   application, a watchdog that abandons and replaces a worker past its
   deadline, and a writer (the outpost redesign; the earlier query thread
   pool is gone).
3. **Synthesizer host processes** (D18) — one per synthesizer in use, in
   a kill-on-close job, streaming PCM to Core over a pipe; the AppContainer
   sandbox arrives with Eloquence in M7.

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

Outposts and the listener end by being killed with their job (the
supervisor closes the pipe and the job handle together), never by
returning from `main`. Keep it that way unless the following is solved: a
process that has used UIA as a client sometimes hangs at full CPU or
crashes as it exits normally, inside `UIAutomationCore.dll`'s own shutdown
code, which walks a corrupt list in its telemetry of provider connections.
Releasing every UIA object first does not prevent it. A killed process
never runs that code. The evidence and what is still unknown are recorded
in the handoff of 2026-09-02 ("Open: a process that has used UIA as a
client").

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

Inside, an outpost follows NVDA's model of one thread doing all the work
for an application (the outpost redesign, 2026-10-01). Intake callbacks
only add entries to one queue, which applies NVDA's limiter rules (one
waiting entry per object and event kind, and per batch the newest four
focus events and ten other events per UI thread). One worker thread takes
the entries in order and is the only thread that calls into the
application, so events and query replies leave the outpost in the order
they were queued. Every query gets exactly one reply: done, gone, failed,
not started, or abandoned. The outpost keeps the live UIA element or MSAA
object behind every node Core still holds, and releases the rest when Core
reports which nodes it holds; a node id names its outpost incarnation, so
an id from a replaced outpost can never reach its successor.

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
3. **Kill and respawn.** If an outpost accumulates eight abandoned workers
   (unless its application's windows are reported hung), stops answering
   pings for nine seconds, or crashes, the supervisor ends it and, if its
   application holds attention, starts a fresh one and asks it for the
   current focus; Core's references to the old outpost's nodes are dead.
   This is the uniform hard-recovery path — and, because abandoned threads
   are only truly freed by process exit, the only one that reclaims
   everything.

Because outposts are per-app processes, a hang or crash in one app's
accessibility plumbing cannot affect reading any other app, and Core (input,
speech, GUI) is never in the blast radius at all.

### The focus listener (D13)

Per-app outposts have one structural blind spot: the first focus. When an
application gains the foreground, its focus event fires before the
supervisor has spawned that application's outpost and before the outpost's
hooks are installed, so the event is unobservable by the very process
responsible for it. The M3-era mitigation — the supervisor tells the new
outpost to *reconstruct* what it missed by polling "what window? what
focused control?" with bounded retries — was root-caused live as a family
of silent failures: the poll raced menu popups, announced half-constructed
windows, and when every retry burned its deadline against a loaded or
slow-starting application, exhausted without announcing anything at all.

The focus listener closes the blind spot at its cause. It is one permanent
outpost process, supervised like any other (job object, heartbeat,
respawn), that exists from startup and holds exactly the subscriptions
that are global by nature:

- the desktop-global UIA focus registration — which UIA offers only
  desktop-wide anyway; before D13, every per-app outpost held its own
  desktop-global registration and discarded other applications' events, so
  N outposts meant N redundant callbacks per focus change system-wide, and
  this consolidates them into one;
- global MSAA WinEvent hooks (process id zero) for focus, foreground
  changes, and menu-popup opens — NVDA's own arrangement, and what
  absorbs the foreground trigger that previously ran inside Core.

The listener's contract is a single hard rule: **it never makes a
cross-process call.** A UIA focus callback delivers the element with its
properties already cached, so building a snapshot is local memory reads;
an MSAA WinEvent delivers raw window and object ids, which are forwarded
untouched; the only other reads are hang-safe local ones (the window's
owning process id, its class name, and for a UIA focus on an element with
no window of its own, the keyboard focus window from `GetGUIThreadInfo`). No cross-process calls means no
deadlines, no query pool, no parked threads, and no way for any
application — hung or not — to stall focus detection for the rest of the
desktop. The listener holds no per-application state either, so a crash
respawns into full capability instantly.

Detection and announcement are deliberately split. The listener forwards
each captured focus fact (source process, window, backend address or
cached snapshot) to Core; the supervisor ensures the target's own outpost
exists — a spawn now merely delays the announcement by the spawn latency,
where before it lost the event outright — and hands the fact to it. Facts
for an outpost still starting are held in arrival order, merged with
NVDA's one-per-object rule, and released in that order when it is ready.
The app outpost's worker then does the acquisition, cross-backend
arbitration, ancestry and selection enrichment, and reports the focus
event, degrading rather than falling silent when enrichment times out
(the event-carried snapshot always suffices to announce name, role,
value, and state). Node identity therefore never crosses processes — the
navigator, object navigation, and every fetch keep routing to the per-app
outpost exactly as before, and D9's isolation still bounds every call
that can block.

Because both backends can report the same focus, the listener forwards
both facts and takes no side: arbitration is not a race between the pair
but a per-window verdict the app outpost already keeps, which each
fact consults independently whenever it arrives — so deduplication does
not depend on the two facts arriving together, or at all. For a window
with no verdict yet, the fact resolves the real verdict on the spot:
fact handling runs on the outpost's deadline-guarded worker, where a
blocking call is permitted, so the arbitration probe answers within the
ordinary query deadline and exactly one backend announces,
deterministically, in either arrival order. The earlier asymmetric
provisional rule — discard the UIA fact and schedule a background probe,
trusting the MSAA fact to carry the announcement — is deliberately not
applied to facts: modern XAML surfaces fire no MSAA focus event at all
(the Start menu's search box was the live reproducer), so the discarded
UIA fact was the only announcement that control would ever get, and a
focus event fires once, with nothing to retry. Legitimate evidence in
hand is never discarded in favor of hypothetical evidence from the other
backend. Since the outpost redesign there is no provisional rule at all:
the outpost's own hooks only queue events, the worker decides every
window's backend before reading, and a probe that hangs is handled by
the watchdog like any other hung call. Resolving which window a
non-windowed UIA element belongs to
can itself take a cross-process normalize call, which is precisely why
the verdict check runs in the app outpost and never in the listener.

Nothing remains of the old poll. After the listener is replaced, Core
reads the foreground window itself and asks that application's outpost
for its current focus once; desktop-wide UIA events from the gap are lost.
A window that has no name when focus enters it is not announced later,
as in NVDA.

The per-app outposts shed their focus-shaped subscriptions — the
per-outpost UIA focus registration, the MSAA focus hook, and the
menu-popup hook. They keep process-scoped MSAA hooks for value, state,
name, selection, and window destruction (the end of a menu or of the
Alt+Tab switcher is global too, and goes from the listener to Core,
amended 2026-10-02), and one UIA property
subscription that follows the focus and its ancestors (NVDA's selective
registration on Windows 11); selection and notifications come
desktop-wide from the listener. One extra hop (listener to
Core to app outpost) costs well under a millisecond against the tens of
milliseconds acquisition already costs, and removes the zero-to-seconds
discovery latency of polling.

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

## 3. Normalized accessibility model

`verbatim-model` defines Verbatim's own vocabulary — roles, states, properties,
text ranges, relationships, node identity — as a superset that both backends
(UIA and MSAA/IA2) map into. This is the load-bearing abstraction for D1/D2:
the reducer, browse mode, extensions, and all tests speak this model only.

- Node identity: backend runtime IDs and MSAA objects map to `NodeId`s
  issued per outpost incarnation; a `NodeId` carries its outpost id, so an
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
  fetches. Verify ARM64 behavior early (R3).
- **Terminals.** TextPattern plus remote ops, with a diff-based change
  announcer and an explicit flood policy: output is coalesced and speech for
  superseded screenfuls is dropped, bounded queue, never unbounded backlog.
  This is a headline scenario for the latency budget.
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
  IA2 implementation work, which lands with the browsers in M6; before
  then MSAA-only apps are read through plain MSAA.
- **Cost model**: unlike UIA there are no cache requests and no remote ops;
  every property is a cross-process COM round trip. Mitigations, in order:
  fetch discipline (only what the reducer asked for), aggressive outpost-side
  caching keyed to WinEvent invalidations, incremental browse-mode builds,
  and the D2 in-process helper for batching, which lands with browse mode
  (M6).
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
gesture is enqueued; gesture semantics run on the reducer thread. Gesture maps are user-remappable
and per-app-module overridable, NVDA-style. Touch and mouse tracking come
later but route through the same `Input` type.

## 6. Speech and audio

Pipeline stages, in order: structured utterance (semantic spans, per D12),
dictionary and symbol processing (per span), presentation (a theme flattens
spans to text, voice changes, and earcons; the default theme is plain
speech), language tagging, synth driver, PCM, the mixer (D17), and the
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
    DLL's architecture (x86 under emulation if needed, M7), streaming PCM
    over a pipe. Eloquence adds the AppContainer sandbox in M7. Latency
    budget applies equally (the pipe hop is tens of microseconds).
- **Audio**: a mixer with one audio thread that converts every source to
  the device's format, sums them, and tracks playback position per
  utterance (D17), writing to the `AudioDevice` trait; WASAPI
  event-driven shared mode is the device implementation, with a silent
  real-time device for machines without one and for test audio.
- **Latency budget** (enforced by tests, not aspiration): per D15, 10 ms
  or under from event observation to the utterance being queued on every
  backend, and 10 ms or under from queued to the first audio sample with
  eSpeak. eSpeak NG is built in and the default synthesizer from the
  opening work of M4, so the second half is measurable from the first
  text work onward. Every stage is traced (section 9).

## 7. Extensions (Wasm)

- Runtime: wasmtime with the component model (per D6, replaceable); host API
  defined in WIT (`verbatim:ext`). Epoch-based preemption means a runaway
  extension is interrupted, never hangs Core, so the host runs in-process on
  dedicated threads.
- Capability-gated, deny-by-default: manifests declare needs (tree-query,
  event subscription, speech/braille output, gesture binding, config,
  namespaced storage, OCR; each is a separate grant surfaced to the user).
- Extension kinds: **app modules** (activated per application, mirroring
  outpost lifecycle — this is where Office/Terminal/browser-specific behavior
  lives), **global extensions**, **synths**, later braille drivers and
  OCR/recognition providers.
- App modules hook the pipeline at defined points: adjust presentation of a
  node, add synthetic nodes, handle gestures, react to events. Hooks have
  deadlines; a slow extension degrades itself, not Verbatim.
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
  tree dumps, gesture injection, speech capture, latency histograms.

## 10. Control plane (dev tooling, then remote support)

A single authenticated protocol (JSON-RPC over named pipe locally; TLS
WebSocket for remote) exposes: event/speech streams, tree queries, gesture and
input injection, config, and lifecycle. The E2E harness and
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
(section 1) — no name, no ACL surface. Remote access never reuses the pipe;
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

- No injection unless and until the D2 helper ships (M6), and then only via
  it.
- Native synth DLLs and Wasm extensions are sandboxed as described; Core never
  loads third-party native code in-process.
- **UIAccess**: reading elevated apps' UI from a non-elevated Verbatim
  requires `uiAccess=true`, which requires signed binaries in a trusted path —
  an installer/signing prerequisite tracked from the start (R5).
- **Secure desktop**: register as an Assistive Technology so Windows launches
  a minimal Verbatim instance (same binary, `--secure`: no extensions, no
  control plane, read-only config copy) on sign-in/UAC desktops.

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
   plane, on a hosted CI runner, a developer's own machine or VM, or a
   local Hyper-V VM (D3);
   speech asserted on the control plane's speech stream, spoken by
   eSpeak NG through the silent real-time device in a silent run or the
   real device in an audible one; a separate WASAPI smoke test in the
   interactive loop proves audio actually reaches a device.

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
capture needs the session attached, or a headless run. The pre-2026-09-02
design captured loopback audio from a VB-CABLE device, which made
recording and listening mutually exclusive; its dead ends are recorded in
`docs/roadmap-done.md` for history.

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
- `verbatim-core` — the reducer, modes, review cursor, browse-mode projection.
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
- `verbatim-ext` and `verbatim-ext-api` — wasmtime host; WIT plus guest SDK.
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

## 16. Top risks

- **R1 — Out-of-process IA2 chattiness.** No cache requests or remote ops
  means large browser documents could make browse-mode builds feel slow.
  Mitigation: fetch discipline and incremental builds first; the D2
  injection helper lands inside M6 itself (out-of-process first, then the
  helper), and the M6 exit's fixed-corpus measurement against NVDA verifies
  the result.
- **R2 — Outpost process overhead.** One process per app costs working set
  and spawn latency, which matters on low-end devices. Mitigation: idle
  retirement, pre-spawning on foreground change, working-set and
  spawn-latency measurement on the harness VM in M3 (no dedicated low-end
  VM profile; user reports drive any deeper investigation); consolidation
  into shared hosts as the fallback (D9).
- **R3 — Remote-ops on ARM64.** API limits or behavior differences.
  Mitigation: spike alongside first terminal work (M4).
- **R4 — Eloquence complications.** DLL architecture or licensing issues.
  Mitigation: native host is already arch-flexible; PoC scoped to its own
  milestone (M7), off the critical path.
- **R5 — UIAccess signing.** Signing requirements could block reading
  elevated UIs during development. Mitigation: develop with test-signing in
  the VM; production signing tracked as release infra.
- **R6 — Screen curtain on Win11/ARM64.** Magnification-API behavior needs
  verification. Mitigation: verify when overlays land (M8); the overlay
  design (DirectComposition) is independent of it.
