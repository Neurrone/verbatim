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
  MSAA/IA2 are solid (roadmap M11). Rationale: UIA is the only API for WinUI, Terminal,
  and modern-Office surfaces, but Firefox and Chromium expose their richest
  tree via IA2, and much of Win32 — including parts of Windows itself — is
  MSAA/IA2-only. Neither API alone covers the desktop.
- **D2 — Injection is planned but never required for correctness.** An
  in-process injection helper (NVDA's nvdaHelper analog) is a planned
  component, but out-of-process IA2 must work first; the helper ships when
  performance data demands it (see risk R1). Rationale: needed for
  competitive IA2 performance in large browser documents, but keeping it off
  the correctness path keeps the x64/ARM64EC/x86 binary matrix and antivirus
  friction out of early milestones.
- **D3 — E2E runs locally in a Hyper-V Windows 11 VM first**; CI automation
  of E2E is deferred. CI still builds and runs unit and provider tests on
  GitHub-hosted Windows runners. Rationale: GitHub-hosted Windows runners
  cannot do nested virtualization; solve that later rather than now.
- **D4 — GUI is wxWidgets via wxDragon.** Rationale: wxWidgets accessibility
  is proven in exactly this role — NVDA's own GUI is wxPython.
- **D5 — Audio backend is WASAPI behind an `AudioSink` trait.** Rationale:
  allows alternate backends without touching the speech pipeline.
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

## 1. Process and thread model

The founding responsiveness rule: **no thread that produces output (speech,
braille, tones) or handles input may ever make a blocking call into another
application.**

Verbatim runs as three kinds of process:

1. **Core process (`verbatim.exe`)**, containing these thread groups:
   - the input hook thread, running the low-level keyboard hook;
   - the reducer thread, running the functional core;
   - the speech and audio threads (pipeline plus WASAPI);
   - the GUI thread (wxDragon);
   - the extension-host threads (wasmtime with epoch preemption).
2. **Outpost processes (`verbatim-outpost.exe`), one per target application**
   (D9), each containing an event thread (WinEvent message loop and UIA
   callbacks) and a small query thread pool for UIA and IA2 COM calls.
3. **Sandboxed helper processes** — currently the native synth host
   (AppContainer plus job object), streaming PCM to Core over shared memory.

A supervisor in Core spawns outposts, tracks their health, and kills and
respawns any that stop responding. Because Windows has no parent-child
process lifetime link of its own, every spawned process is placed in a job
object with kill-on-job-close before it starts running (spawned suspended,
assigned to the job, then resumed), and Core holds the job handles: if
`verbatim.exe` exits for any reason, including a crash, the kernel closes
those handles and kills everything in the jobs. Per-outpost jobs also carry
a per-process memory cap, so a leaking outpost is killed by the kernel and
respawned by the supervisor. The synth host's sandbox job (section 6) simply
carries the same kill-on-close flag; control-plane clients such as
`verbatim-inspect` are deliberately not children and not in any job.

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

Recovery is a ladder, cheapest rung first:

1. **Deadline expiry.** Every cross-process accessibility call carries a
   deadline. When one expires, the reducer proceeds with cached
   (stale-flagged) data and may emit an "application not responding" earcon
   rather than waiting.
2. **Thread abandonment.** A thread blocked inside a hung app's COM call
   cannot be safely reclaimed: `ICancelMethodCalls` is unreliable and
   `TerminateThread` corrupts the calling process (abandoned locks, broken
   apartment state). So the outpost abandons the call — stops awaiting the
   result, discards it if it ever arrives — and spawns a replacement worker.
   The blocked thread stays parked (roughly a megabyte of stack and a
   handle) until the call returns or the outpost exits.
3. **Kill and respawn.** If an outpost accumulates too many parked threads,
   stops heartbeating, or crashes, the supervisor kills the process and
   spawns a fresh one; caches rebuild from live queries. This is the uniform
   hard-recovery path — and, because abandoned threads are only truly freed
   by process exit, the only one that reclaims everything.

Because outposts are per-app processes, a hang or crash in one app's
accessibility plumbing cannot affect reading any other app, and Core (input,
speech, GUI) is never in the blast radius at all.

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

The interaction logic is a pure reducer living in `verbatim-core`:

```rust
fn reduce(state: &SrState, input: Input) -> (SrState, Vec<Effect>)
```

- `SrState`: focus context, review cursor, active modes (focus/browse/scan),
  per-app tree snapshots (normalized, versioned), speech-relevant config.
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

- Node identity: backend runtime IDs map to stable internal `NodeId`s per
  outpost.
- Trees are versioned snapshots; events reference snapshot versions so the
  reducer can detect and re-fetch stale reads.
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
M11) for Java applications.

### UIA

- **Cache requests everywhere.** Event registrations attach
  `IUIAutomationCacheRequest`s so events arrive with name/role/states/patterns
  prefetched in the same round trip — the single biggest lever against
  blocking on slow apps.
- **Threading.** Per UIA guidance, event callbacks arrive on dedicated MTA
  threads separate from threads that make UIA calls; each outpost owns both.
  Multiple UIA client threads are a supported, intended part of the design.
- **Remote Operations.** The Microsoft.UI.UIAutomation remote-ops API (which
  NVDA vendors as UIARemote) executes batched operations inside the provider
  process in one cross-process round trip. Wrapped in a `verbatim-uia-rops`
  crate and used for: ancestor-chain retrieval on focus events, bulk text
  attribute runs, terminal text-range walking, and browse-mode buffer batch
  fetches. Verify ARM64 behavior early (R3).
- **Terminals.** TextPattern plus remote ops, with a diff-based change
  announcer and an explicit flood policy: output is coalesced and speech for
  superseded screenfuls is dropped, bounded queue, never unbounded backlog.
  This is a headline scenario for the latency budget.

### MSAA / IA2

- **Events** arrive as WinEvents via *out-of-context* `SetWinEventHook`
  scoped to the outpost's process id — asynchronous delivery in our process,
  no injection required.
- **IA2 acquisition**: starting from the `IAccessible` carried by the
  WinEvent, call `IServiceProvider::QueryService` to obtain `IAccessible2`,
  then the IA2 text, hypertext, and relation interfaces as needed
  (`nvda/include/ia2` vendors the IDL). The IA2 proxy/stub (marshaling)
  story from Rust — registered or reg-free — is settled during the first
  IA2 implementation work.
- **Cost model**: unlike UIA there are no cache requests and no remote ops;
  every property is a cross-process COM round trip. Mitigations, in order:
  fetch discipline (only what the reducer asked for), aggressive outpost-side
  caching keyed to WinEvent invalidations, incremental browse-mode builds,
  and ultimately the D2 in-process helper for batching once measurements
  demand it (R1).
- **MSAA-only apps** are normalized at "usable" fidelity through the same
  stack (IA2 is a set of interfaces layered on MSAA plumbing).

## 5. Input

A low-level keyboard hook (`WH_KEYBOARD_LL`) lives on a dedicated,
never-blocking thread in Core. Constraint: Windows silently removes hooks that
exceed the `LowLevelHooksTimeout`, so the swallow/pass decision must be made in
microseconds against a read-only, lock-free snapshot of the gesture map
(rebuilt atomically when bindings change). The hook only decides and enqueues;
gesture semantics run on the reducer thread. Gesture maps are user-remappable
and per-app-module overridable, NVDA-style. Touch and mouse tracking come
later but route through the same `Input` type.

## 6. Speech and audio

Pipeline stages, in order: utterance, dictionary and symbol processing,
language tagging, synth driver, PCM, `AudioSink`.

- **Speech manager**: priority lanes (interrupt/next/queued), index marks with
  callbacks (say-all, braille sync, latency probes), rate/pitch/volume state,
  per-language voice switching. Multilingual from the start: utterances carry
  language tags end-to-end.
- **Synth drivers** implement one trait — streaming PCM plus index-mark
  events — regardless of origin:
  - Built-in: **OneCore** (WinRT `Windows.Media.SpeechSynthesis` via
    windows-rs) and **eSpeak NG** (statically linked; builds cleanly on ARM64).
  - **Wasm synths**: components implementing the `verbatim:synth` WIT world,
    PCM via shared buffer. Path for source-available synths.
  - **Native synth host**: separate sandboxed process (low-integrity /
    AppContainer, job object) matching the DLL's architecture (x86 under
    emulation if needed), streaming PCM over a shared-memory ring. Eloquence
    is the proof of concept. Latency budget applies equally (shared-memory
    hop is negligible).
- **Audio**: `AudioSink` trait; WASAPI event-driven shared mode with small
  buffers as the only initial implementation.
- **Latency budget** (enforced by tests, not aspiration): from key-down to
  first audio sample, 50 ms or less with eSpeak on a low-end reference VM.
  Every stage is traced (section 9).

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
apps.

## 9. Observability

- `tracing`-based structured spans with a **trace ID minted when an OS event
  or keypress is first observed** and carried through the outpost, the
  reducer, the speech queue, the synth, and audio submission. For any
  utterance, the timeline — event observed, speech queued, audio started —
  is a single query.
- A ring-buffer **flight recorder** persists recent reducer inputs (the
  replayable triples of section 2) and spans; a crash or user-triggered
  snapshot dumps it for offline replay.
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

wxDragon settings UI on its own thread in Core. The M1 prototype's defining
test is Verbatim reading its own GUI via ordinary UIA through a real outpost
process — no self-voicing side channel, no in-process shortcut — which
validates the UIA stack and the outpost architecture end-to-end (and avoids
same-process UIA client/provider hazards).

## 12. Security posture

- No injection unless and until the D2 helper ships, and then only via it.
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
4. **E2E in the local Hyper-V VM** (section 14): real Windows, real apps
   (Notepad, Explorer, Terminal, Edge, Office), driven via the control plane;
   speech asserted via capture synth; a separate WASAPI loopback smoke test
   proves audio actually reaches the device.

## 14. VM harness (local-first, per D3)

`cargo xtask vm <cmd>` wraps Hyper-V PowerShell: `create` (unattended Win11
install from autounattend.xml, checkpoint a golden image), `start/stop/
restart`, `deploy` (artifacts via PowerShell Direct), `test` (run E2E suite via
an in-guest agent speaking the control plane), `logs`. Enhanced Session for
audio. The harness API is deliberately host-agnostic so the deferred CI story
(QEMU/KVM on Linux runners, or whatever we choose) reuses the same in-guest
agent and test suites.

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
- `verbatim-jab` — Java Access Bridge client stack (planned, M11).
- `verbatim-outpost` — the outpost actor and per-app outpost binary, plus the
  Core-side supervisor.
- `verbatim-control` — control-plane protocol and server.
- `verbatim-speech` and `verbatim-audio` — pipeline; `AudioSink` plus WASAPI.
- `verbatim-synth-*` — OneCore, eSpeak NG, capture (test) drivers.
- `verbatim-ext` and `verbatim-ext-api` — wasmtime host; WIT plus guest SDK.
- `verbatim-i18n` — Fluent localization (D10): embedded English fallback,
  runtime locale-folder loading.
- `verbatim-input` — hook thread, gesture maps.
- `verbatim-gui` — wxDragon settings UI.
- `verbatim-app` — `verbatim.exe` composition root.
- `verbatim-inspect`, `mockapp`, `xtask` — dev tool; UIA/IA2 provider fake;
  automation.

## 16. Top risks

- **R1 — Out-of-process IA2 chattiness.** No cache requests or remote ops
  means large browser documents could make browse-mode builds feel slow.
  Mitigation: fetch discipline and incremental builds first; the M6 exit
  measures against NVDA on a fixed corpus and pulls the D2 injection helper
  (M10) forward if needed.
- **R2 — Outpost process overhead.** One process per app costs working set
  and spawn latency, which matters on low-end devices. Mitigation: idle
  retirement, pre-spawning on foreground change, measurement as an M3 exit
  criterion; consolidation into shared hosts as the fallback (D9).
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
