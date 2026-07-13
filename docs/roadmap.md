# Verbatim Roadmap

Milestones are scoped by *risk retired*, not calendar time. Each has explicit
exit criteria so progress is testable. Ongoing tracks (localization,
observability, latency budgets) start early and run through every milestone
rather than being milestones themselves.

## M0 — Foundations

No technology spikes: UIA, MSAA/IA2, and OneCore are known to work from
Rust, and wx via wxDragon is known to be accessible. Anything genuinely
uncertain (the IA2 proxy/stub story, remote ops on ARM64, wasmtime on
Windows ARM64) gets settled inside the milestone that implements it.

- Cargo workspace + crate skeletons per the architecture crate map; `xtask`;
  CI on GitHub-hosted Windows runners: build (x64 + ARM64 cross), clippy,
  unit tests.
- Tracing + flight-recorder skeleton; localization framework choice wired in
  from the first user-visible string.

Exit: `cargo xtask ci` runs clean on both architectures.

## M1 — Self-voicing prototype (the stated initial prototype)

- `verbatim.exe` with the Verbatim menu (options: settings and quit,
  localized) and a settings GUI: speech tab (synthesizer selection, voice,
  rate, pitch, volume), config persisted. Internal order: the speech
  pipeline speaking through OneCore lands first, then settings persistence,
  then the wxDragon GUI on top of both — wx accessibility is proven (NVDA's
  own GUI is wxPython), so no earlier spike is needed.
- The real outpost architecture at minimum viable scope: the Core-side
  supervisor spawns one outpost process (job-object lifetime,
  inherited-handle IPC) for the focused application — Verbatim's own GUI in
  the exit scenario. Reading our own windows through an out-of-process
  outpost also sidesteps same-process UIA client/provider hazards.
- UIA focus, property-change, and value-change events flow from the outpost
  through the reducer and speech pipeline, out through OneCore and WASAPI.
  Real UIA path, no self-voicing shortcut.
- Keyboard hook with the Verbatim modifier key and a single gesture:
  Verbatim+V opens the Verbatim menu. Nothing else is bound in M1.
- Latency traces visible end-to-end (event observed, speech queued, audio
  started).

Exit: with Verbatim running, Verbatim+V opens the menu, and every item in
the menu and every control in the settings dialog is fully announced —
name, role, value, and state on focus, plus value changes while adjusting
sliders and combo boxes; keypress-to-audio latency traced and reported
end-to-end (budget enforcement starts when eSpeak lands in M3).

## M2 — Test harness and VM

Make everything after this point verifiable automatically.

- `mockapp` provider host — the same scripted trees exposed as UIA providers
  and as MSAA/IA2 providers — plus first tree-construction tests (real
  client stacks against it, cross-process), including backend-arbitration
  cases.
- Capture synth; reducer replay tests from flight-recorder dumps.
- Control plane v0 (named pipe): event stream, tree dump, gesture injection,
  speech capture. `verbatim-inspect` CLI on top.
- Hyper-V harness: `xtask vm create/start/stop/deploy/test/logs`, golden
  checkpoint, in-guest agent; first E2E scenario (own GUI + Notepad) running
  locally. CI automation of E2E deferred per D3.

Exit: a one-command local run boots the VM, deploys a build, runs E2E, and
reports speech assertions + latency numbers.

## M3 — Desktop usability core

Verbatim becomes usable as a daily driver for basic Windows navigation.

- Outposts generalized from M1's single instance to many concurrent per-app
  processes (D9): outpost lifecycle across focus changes, the full recovery
  ladder (call deadlines, thread abandonment, kill-and-respawn), idle
  retirement, stale-cache policy (the architecture's responsiveness story,
  for real); WinEvent routing and per-app backend arbitration (UIA vs
  MSAA/IA2). Measure per-outpost working set and spawn latency on the
  low-end VM profile (risk R2).
- Focus/foreground tracking across apps; object navigation and review cursor;
  input-help mode; remappable gesture map; symbol/dictionary processing v1.
- eSpeak NG built-in synth (statically linked, x64 and ARM64) as regular
  implementation work — it is the reference synth for the latency budget,
  which is enforced from here on.
- E2E scenarios: Explorer, Settings, Start menu, task switching, at least
  one MSAA/IA2-only legacy app; a deliberately-hung app must not delay
  speech for the rest of the system (E2E test kills/suspends a mock app
  mid-interaction).

Exit: navigate Windows shell fluently; hung-app E2E passes; latency budget
holds on the low-end VM profile.

## M4 — Text, editing, and terminals

- TextPattern support in the model; caret tracking, typed-character echo,
  word/line/character navigation; say-all with index-mark continuation.
- Remote-ops integration (ancestor fetch on focus; terminal text ranges) —
  includes the ARM64 remote-ops verification (R3).
- Windows Terminal: diff-based output announcement with flood policy; the
  "cat a huge file" scenario is an E2E latency test.

Exit: Notepad editing and Terminal sessions are solid; terminal flood E2E
shows bounded latency and no hang.

## M5 — Extensions v1

- Component host with epoch preemption; WIT `verbatim:ext` v0: event
  subscription, tree queries, speech output, gestures, config, storage.
  Ratify the D6 runtime choice here: verify wasmtime on Windows ARM64 and
  measure host-call overhead against the extension-hook deadlines before
  building further on it.
- App-module activation keyed to processes; hot reload; capability manifests.
- Dogfood: at least one first-party app module (e.g., Terminal refinements)
  ported out of the core into an extension; a trivial Wasm synth proves the
  `verbatim:synth` world.

Exit: an app module can be edited and hot-reloaded without restarting
Verbatim; capability denial is enforced and tested. This milestone opens the
app-module porting track (see the section after M12), which then runs
continuously alongside every later milestone.

## M6 — Browse mode and browsers

- Document projection / incremental virtual buffer over the normalized tree;
  quick-nav keys, elements list, switching between focus and browse modes.
- Firefox and Chromium (Edge/Chrome) via IA2 as the primary source, with a
  UIA comparison where relevant; **measure build/navigation performance and
  parity vs NVDA on a fixed page corpus** — this data decides whether the
  D2 injection helper (M10) gets pulled forward.
- Interaction-before-full-render E2E on a very large page.
- Scan-mode generalization: the same projection over an ordinary app.

Exit: real browsing works day-to-day; large-page E2E passes; corpus
performance/parity report written, with a recorded decision on whether the
M10 injection helper must be pulled forward.

## M7 — Native synth host + Eloquence PoC

- Sandboxed native synth host process (AppContainer, job object, shared-memory
  PCM ring), arch-matched loading.
- Eloquence proof of concept meeting the same latency budget.

Exit: Eloquence speaks through Verbatim under sandbox; latency test green.

## M8 — Breadth: profiles, overlays, OCR, secure desktop

- Configuration profiles (manual and triggered), full pronunciation/symbol
  dictionaries, localized UI shipped in at least two languages as proof.
- Focus highlight (DirectComposition overlay) and screen curtain (R6 check);
  overlay component designed to host a future magnifier.
- OCR capability (Windows.Media.Ocr) exposed to extensions; synthetic-subtree
  review of an image/window.
- Secure-desktop instance (`--secure`), AT registration, UIAccess/test-signing
  story in the VM (R5).
- Installer/updater skeleton.

Exit: sign-in and UAC prompts are read in the VM; curtain + highlight E2E.

## M9 — Remote support

- Pairing/auth UX on the control plane; speech mirroring out, input in;
  secure-desktop and permission rules applied to remote sessions.

Exit: control a second Verbatim instance (between the VM and the host)
end-to-end.

## M10 — In-process helper and performance parity (per D2)

- The planned injection helper: in-process IA2 batching (and virtual-buffer
  acceleration if the M6 data demands it) — first and only use of injection,
  including the x64/ARM64EC/x86 helper matrix and AV/signing considerations.
  Pulled forward ahead of M7–M9 if the M6 corpus report says out-of-process
  IA2 isn't good enough.
- Re-run the M6 corpus with the helper: parity-vs-NVDA decision per browser.

Exit: browsers at (or consciously accepted near) NVDA parity on the corpus.

## M11 — Java Access Bridge

JAB is a committed backend, deliberately last among the accessibility APIs
(D1): far fewer apps need it than UIA and MSAA/IA2.

- `verbatim-jab` client stack: the WindowsAccessBridge-64 C API and its
  event callbacks, running on outpost threads like the other backends,
  mapped into the normalized model behind the same arbitration.
- A small Java Swing fixture app joins `mockapp` duty for provider-level
  tests; E2E against a current Java IDE or LibreOffice-adjacent Java app.
- Unblocks porting-track Tier E (javaw, Eclipse).

Exit: a mainstream Swing application is readable and navigable end-to-end.

## M12 — Braille (deliberately last, D7)

- liblouis integration; `BrailleDisplay` trait implementations for common
  displays; braille viewer (on-screen virtual display) so development and
  E2E tests need no hardware; routing keys, cursor tethering, speech-braille
  sync via existing index marks.

Exit: braille viewer E2E scenarios pass; at least one hardware display
verified when hardware is available.

## The app-module porting track (runs from M5 onward)

Goal: eventually port all of NVDA's built-in app modules
(`nvda/source/appModules`, roughly 80) except those for software that no
longer exists or is unmaintained — Skype, Lync, MSN Messenger, Outlook
Express, Windows Live Mail, Winamp, Instantbird, Miranda, Lotus
Notes/Symphony, the legacy EdgeHTML modules, and similar; each skip is a
recorded case-by-case call.

Method: triage before porting. Many NVDA modules exist to patch quirks of
NVDA's pipeline or of APIs at the time they were written; for each module,
first test the app against Verbatim's stock UIA/IA2 handling and port only
the behavior that is still needed. Every port doubles as a test of the
extension API (per the API-growth rule: no host-API additions without a
consumer).

Tiers, gated by the capabilities each module needs:

- **Tier A — immediately after M5** (needs only extension API v0: events,
  tree queries, speech, gestures): the Windows shell and utility modules —
  Explorer, Settings, Task Manager, Calculator, search, lock screen and
  logon UI, Open With, Notepad, Notepad++, VS Code, Poedit, Spotify,
  foobar2000, Audacity, 1Password, basic Zoom and Teams behavior.
- **Tier B — with M4/M6 text and terminal infrastructure**: the terminal
  clients — PuTTY, mintty, SecureCRT, Tera Term.
- **Tier C — after M6 browse mode** (web-content-hosting apps): WebView2
  hosts, WhatsApp, deeper Teams support, Thunderbird, Kindle and other
  readers.
- **Tier D — after the object-model bridge capability lands (planned around
  M8)**: Word, Excel, PowerPoint, Outlook, LibreOffice, Visual Studio.
  NVDA's Office modules rely heavily on COM object-model automation, which
  Wasm extensions cannot reach directly; this tier is gated on designing a
  capability-gated app object-model bridge in the WIT host API — the
  largest single API-growth item in the plan, and worth its own design pass
  for both surface and security.
- **Tier E — after the JAB backend lands (M11)**: javaw and Eclipse.

## Ongoing tracks (every milestone)

- **Latency**: budget tests run in E2E from M2 on; regressions fail the build.
- **Localization**: no hardcoded user-visible strings, ever; pseudo-locale
  test from M1.
- **Flight recorder into regression tests**: every reproduced field/VM bug
  lands as a replay test.
- **Extension API growth**: only via porting real app modules/add-ons;
  each addition needs a consumer.
- **Deferred**: CI automation for VM E2E (D3); revisit once local harness is
  stable — candidates are QEMU/KVM Win11 guests on Linux runners or
  self-hosted runners.
