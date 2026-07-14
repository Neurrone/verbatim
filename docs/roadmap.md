# Verbatim Roadmap

Milestones are scoped by _risk retired_, not calendar time. Each has explicit
exit criteria so progress is testable. Ongoing tracks (localization,
observability, latency budgets) start early and run through every milestone
rather than being milestones themselves.

## Status

M0, M1, and M2 are complete. M1's exit behavior was verified live during
development — the menu and every settings-dialog control announced with
name, role, value, and state through a real outpost, with end-to-end
latency timelines — and the repeatable, scripted form of that verification
now runs as `crates/verbatim-e2e`'s `m1_exit_regression` test: locally
runner-direct, in CI runner-direct on plain GitHub-hosted Windows runners,
and locally against the Hyper-V VM harness via `cargo xtask vm test`. See
`docs/tooling.md` for how to run any of these by hand.

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
- Amendments recorded during M1 planning. First, a minimal MSAA client and
  NVDA-style per-window arbitration are pulled forward from M3: wx dialogs
  are native Win32 controls with no UIA server-side provider, so reading our
  own GUI takes the MSAA path, exactly as NVDA reads its own; the UIA path
  is still required and exercised by UIA-native windows. Second, a minimal
  control-plane v0 and `verbatim-inspect` are pulled forward from M2 so M1
  work is verifiable live: event and speech streams, gesture and arbitrary
  key injection, latency queries, and quit. Third, startup replaces any
  running instance (NVDA's algorithm), and configuration is portable —
  `settings.toml` next to the executable is the base configuration (globals
  plus the base profile, where speech lives), and the `profiles` folder
  holds named profile overlays, none of which are active in M1.

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
- Flight-recorder dumps to disk (crash or user-triggered snapshot) and
  reducer replay tests built from live dumps; the capture synth and the
  in-memory replay machinery landed in M1.
- Control plane v0 and `verbatim-inspect` landed in M1 (event and speech
  streams, gesture and arbitrary key injection, latency timelines, status,
  quit). M2 adds the remaining surface the harness needs, starting with
  tree dumps, and the in-guest agent that speaks the protocol
  programmatically.
- Hyper-V harness: `xtask vm create/start/stop/restart/restore/deploy/test
  /logs/delete`, golden checkpoint, in-guest agent (`verbatim-agent`); E2E
  scenarios (own GUI plus Speech dialog, and Notepad focus) running locally
  against the VM. CI runs the E2E suite runner-direct — the in-guest agent
  bound to loopback on a plain GitHub-hosted Windows runner — alongside
  unit and provider tests. Automating the *VM* itself in CI is still
  deferred per D3: `.github/workflows/vm-smoke.yml`, manual-dispatch only,
  checks whether GitHub's larger Windows runners can host nested
  virtualization at all, ahead of ever depending on it.

Exit: a one-command local run boots the VM, deploys a build, runs E2E, and
reports speech assertions + latency numbers. The suite must include the M1
exit behavior as a scripted regression: Verbatim's own menu and its Speech
settings dialog read correctly — every menu item and dialog control
announced with name, role, value, and state on focus, plus value changes
while adjusting the rate slider (both directions) and the voice combo box
(changed and changed back) — with keypress-to-audio latency reported from
the same run. The regression drives the capture synthesizer, whose Speech
page offers a voice combo box and a rate slider but no toggle at all, so it
asserts value changes only, not a check-box state change; the reducer's
checked and not-checked announcements are covered by `verbatim-core`'s unit
tests and, cross-process, by `mockapp`'s scripted state-change events. A
state-change assertion belongs in this E2E regression too, the day a
drivable synthesizer page offers a toggle.

## M3 — Desktop usability core

Verbatim becomes usable as a daily driver for basic Windows navigation.

- Outposts generalized from M1's single instance to many concurrent per-app
  processes (D9): outpost lifecycle across focus changes, the full recovery
  ladder (call deadlines, thread abandonment, kill-and-respawn), idle
  retirement, stale-cache policy (the architecture's responsiveness story,
  for real); WinEvent routing and per-app backend arbitration (UIA vs
  MSAA/IA2). Measure per-outpost working set and spawn latency on the
  low-end VM profile (risk R2).
- Arbitration attribution landed at the end of M2 rather than here, once the
  M2 E2E suite exposed the defect and NVDA's reference implementation
  (`getNearestWindowHandle` in `nvda/source/UIAHandler/__init__.py`) showed
  the mechanism was one call, not a design problem: a UIA event's element is
  usually not a window itself (menu items, list items), so
  `verbatim_uia::nearest_window_handle` resolves its nearest windowed
  ancestor with a single `NormalizeElementBuildCache` round trip, and both
  backends then arbitrate the same window for the same logical element — the
  popup-menu case included, since the MSAA event carries the popup's own
  handle and the UIA walk resolves to that same popup. The decision half
  never needed changing: NVDA's `isUIAWindow` is the same ladder Verbatim's
  `Arbitrator` already implements. What remains for M3 is exercising this
  under the multi-outpost generalization above, plus the deliberate residual
  risk: the normalize call is a cross-process call made inline on the event
  callback thread (NVDA's own trade), which per-app outpost isolation (D9)
  contains — bouncing it to a deadline-guarded query worker is the fallback
  if it misbehaves in practice.
- Focus and foreground timing races, observed as intermittent E2E failures
  (roughly one run in three fails on one of these; the M2 suite is how they
  were found and is the regression net for fixing them). Two known shapes.
  First, opening the Verbatim menu intermittently announces the hidden main
  frame ("Verbatim", role unknown) and can delay or displace the menu
  announcement — the prePopup show, raise, and force-foreground dance in
  `verbatim-gui` racing the popup; the hidden frame should likely never be
  announced at all, and a role reading as "unknown" is poor speech in any
  case. Second, a freshly launched application's focus announcement can
  fail to arrive entirely — the foreground trigger's retarget and the
  outpost's synthetic focus query racing the new process's window creation,
  within the 400 millisecond focus deadline. Both belong to this
  milestone's outpost-lifecycle and focus-tracking work; the arbitration
  mechanism is not the cause of either.
- Announce a focused list's selected item. Focus landing on a list currently
  speaks only the list's own name and role; the selected entry is not spoken,
  which is not how a screen reader should read a category list or a list box.
  This gap was masked until M2: the spurious UIA events described above were
  announcing the selected item by accident, and fixing the arbitration bug
  revealed it. Belongs with this milestone's selection and object-navigation
  work.
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
  self-hosted runners. `.github/workflows/vm-smoke.yml` (manual dispatch
  only) checks whether GitHub's larger Windows runners can host nested
  virtualization at all, a precondition for any of those candidates.
