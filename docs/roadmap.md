# Verbatim Roadmap

Milestones are scoped by _risk retired_, not calendar time. Each has explicit
exit criteria so progress is testable. Ongoing tracks (localization,
observability, latency budgets) start early and run through every milestone
rather than being milestones themselves.

## Status

M0 (foundations), M1 (self-voicing prototype), M2 (test harness and
VM), and M3 (desktop usability core) are complete. Their full scope
and exit-criteria evidence are archived in
[Completed milestones](roadmap-done.md). The next milestone is M4.

## M4 — Text, editing, and terminals

- TextPattern support in the model; caret tracking, typed-character echo,
  word/line/character navigation; say-all with index-mark continuation.
- Text runs carry formatting attributes as utterance spans (spelling and
  grammar markers, and font/color where exposed), so M11's
  formatting-change sounds have data to act on.
- A minimal built-in character-description table (punctuation and symbol
  names): character navigation must say "comma" on a comma even though the
  configurable dictionary system waits until M8.
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
- Dogfood: at least one first-party app module ported out of the core into
  an extension — the Explorer cosmetic fixes deferred from M3 are the
  designated first candidate — and a trivial Wasm synth proves the
  `verbatim:synth` world.

Exit: an app module can be edited and hot-reloaded without restarting
Verbatim; capability denial is enforced and tested. This milestone opens the
app-module porting track (see the section after M15), which then runs
continuously alongside every later milestone.

## M6 — Browse mode, browsers, and the injection helper

- Document projection / incremental virtual buffer over the normalized tree;
  quick-nav keys, elements list (presented through the M3 list dialog),
  switching between focus and browse modes.
- IA2 lands here, deferred from M3 because browsers are what motivate it:
  interface acquisition (`IServiceProvider::QueryService` from the
  WinEvent's `IAccessible` to `IAccessible2` and the IA2 text, hypertext,
  and relation interfaces), the proxy/stub marshaling story from Rust, and
  IA2 roles and states preferred over MSAA in the normalized mapping.
  `mockapp`'s MSAA mode grows an IA2 answering path so the client stack is
  testable cross-process without a browser.
- Firefox and Chromium (Edge/Chrome) via IA2 as the primary source, with a
  UIA comparison where relevant.
- The D2 injection helper lands here, staged inside the milestone:
  out-of-process IA2 first, proving correctness, then the in-process helper
  for performance — IA2 call batching and virtual-buffer acceleration —
  including the x64/ARM64EC/x86 helper matrix and antivirus/signing
  considerations. NVDA has already proven that in-process access is what
  makes browsing fast enough, so this is scheduled work rather than a
  measure-first gate; the fixed-page-corpus measurement against NVDA
  remains as this milestone's exit verification.
- Screen review as a spatial projection of the normalized tree (D11):
  visible nodes ordered by bounding rectangle, grouped into visual lines,
  walked by the review cursor by line, word, and character. Extent-backed
  wherever a text interface exists — UIA TextPattern bounding rectangles
  and IA2 character extents give exact per-character geometry, batched
  through the new helper — with rectangle interpolation as the fallback for
  name-plus-rectangle elements. OCR joins as a second text source in M8 and
  the display model as a third in M14; the review commands and cursor stay
  the same throughout, only the source improves underneath.
- Interaction-before-full-render E2E on a very large page.
- Scan-mode generalization: the same projection over an ordinary app.

Exit: real browsing works day-to-day; large-page E2E passes; corpus
performance/parity report vs NVDA written, at (or consciously accepted
near) parity; screen review reads a modern app's screen correctly.

## M7 — Native synth host + Eloquence PoC

- Sandboxed native synth host process (AppContainer, job object, shared-memory
  PCM ring), arch-matched loading.
- Eloquence proof of concept meeting the same latency budget.

Exit: Eloquence speaks through Verbatim under sandbox; latency test green.

## M8 — Breadth: speech configurability, profiles, overlays, OCR, secure desktop

- eSpeak NG built-in synth (statically linked, x64 and ARM64) — the
  reference synth for the latency budget, whose enforcement tightens from
  the M3 capture-synth budget to the eSpeak reference number.
- Input help mode; the gesture-remapping configuration GUI; full
  pronunciation/symbol dictionaries and their configuration UI (the
  data-driven infrastructure exists from M3/M4).
- Configuration profiles (manual and triggered); localized UI shipped in at
  least two languages as proof.
- Focus highlight (DirectComposition overlay) and screen curtain (R6 check);
  overlay component designed to host a future magnifier.
- OCR capability (Windows.Media.Ocr) exposed to extensions; synthetic-subtree
  review of an image/window; OCR becomes screen review's second text source,
  for windows whose pixels contain text that no API exposes.
- Secure-desktop instance (`--secure`), AT registration, UIAccess/test-signing
  story in the VM (R5).
- Installer/updater skeleton.

Exit: sign-in and UAC prompts are read in the VM; curtain + highlight E2E;
eSpeak latency budget green.

## M9 — Logging and log viewer

Proper user-facing observability, distinct from (and built on) the
developer-facing flight recorder and tracing spans.

- User-facing log levels and categories; logging to file plus an in-memory
  ring; the existing tracing spans, collected stderr, and panic dumps
  absorbed into one coherent story.
- A log viewer window in Verbatim itself, readable with Verbatim — the
  viewer is its own dogfooding test.
- The commands NVDA users expect around it: open the log viewer, report the
  most recent error, cycle the log level at runtime.

Exit: a user can reproduce a bug, open the log viewer, and read what
happened, without touching developer tooling.

## M10 — Extension console

The equivalent of NVDA's Python console for extension development. Python
is an obvious fit for NVDA; with Wasm extensions the console is instead an
interactive interpreter that is itself an extension: an interpreter
compiled to Wasm, granted broad capabilities, evaluating against exactly
the `verbatim:ext` WIT API every extension uses. No privileged side-channel
API exists — if the console can do it, an extension can — so the console
doubles as a standing test that the API is ergonomic enough for
exploratory work. Convenience aliases in the NVDA console style (the
focused object, the navigator object) are pre-bound bindings over the same
calls, never separate host functions.

Design questions settled inside this milestone: which interpreter (QuickJS,
RustPython, or similar, compiled to a component), how epoch preemption
interacts with long-running evaluations, and whether the console UI lives
in the Verbatim GUI, in `verbatim-inspect`, or both.

Exit: from the console, inspect the focused object, walk its tree, speak,
and bind a gesture — against a live Verbatim, without restarting anything.

## M11 — Audio formatting: earcons and voice styling

The payoff for D12's structured utterances: presentation themes that map
semantic spans to sound, in the tradition of Emacspeak's audio formatting
and the audio-themes family of NVDA add-ons.

- A theme maps span semantics to presentation: a role can become an earcon
  plus shorter speech (a slider sound and "pitch 50" instead of "Pitch
  rate slider 50"); formatting attributes on text runs can become sounds
  (a spelling or syntax error under the cursor plays a sound rather than
  being spoken); capitals, quotes, and emphasis can become pitch or voice
  changes.
- The default theme reproduces plain speech exactly; switching themes is a
  runtime configuration change, no restart.
- Themes are data, and eventually extension-provided packages — giving the
  porting track a consumer in the audio-themes add-on family.

Exit: an earcon theme ships alongside the plain default; a scripted E2E
hears the slider earcon and the spelling-error sound; the plain theme's
output is identical to pre-M11 speech.

## M12 — Remote support

- Pairing/auth UX on the control plane; speech mirroring out, input in;
  secure-desktop and permission rules applied to remote sessions.

Exit: control a second Verbatim instance (between the VM and the host)
end-to-end.

## M13 — Java Access Bridge

JAB is a committed backend, deliberately last among the accessibility APIs
(D1): far fewer apps need it than UIA and MSAA/IA2.

- `verbatim-jab` client stack: the WindowsAccessBridge-64 C API and its
  event callbacks, running on outpost threads like the other backends,
  mapped into the normalized model behind the same arbitration.
- A small Java Swing fixture app joins `mockapp` duty for provider-level
  tests; E2E against a current Java IDE or LibreOffice-adjacent Java app.
- Unblocks porting-track Tier E (javaw, Eclipse).

Exit: a mainstream Swing application is readable and navigable end-to-end.

## M14 — Display model (gated)

A GDI display model in the NVDA tradition, deliberately last among the
text sources (D11), and opened by an explicit re-triage gate: test the
then-current PuTTY, SecureCRT, and Tera Term (and any other app that has
motivated this milestone by then) against stock Verbatim plus OCR, and
build only if the display model still earns its maintenance cost. By this
point everything it needs already exists, which is what keeps the
milestone cheap: the M6 injection helper is the delivery vehicle (the
display model is its second in-process client), synthetic nodes make its
output first-class model content, and the M4 diff announcer and flood
policy handle live terminal output.

- GDI text-output hooks (`ExtTextOutW` and family) in the injection helper;
  a per-window text model (chunk rectangles, baseline ordering,
  invalidation on redraw).
- A live-text diff source over that model — the `DisplayModelLiveText`
  equivalent — feeding the same announcement pipeline as Windows Terminal.
- Screen review gains the display model as its third, pixel-faithful text
  source for GDI apps; text-under-mouse works in apps with no text API.
- Unblocks the porting track's legacy terminal clients (PuTTY, SecureCRT,
  Tera Term).

Exit: the re-triage decision is recorded; if built, a PuTTY session is
readable with live output announcement and the existing screen-review
commands, with no fidelity regression anywhere else.

## M15 — Braille (deliberately last, D7)

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
  Explorer (starting with the cosmetic fixes deferred from M3), Settings,
  Task Manager, Calculator, search, lock screen and logon UI, Open With,
  Notepad, Notepad++, VS Code, Poedit, Spotify, foobar2000, Audacity,
  1Password, basic Zoom and Teams behavior.
- **Tier B — the terminal clients, in two stages**: Windows Terminal
  workflows are core M4 behavior, and mintty is triaged on its own after M4
  (its ConPTY integration may make it readable stock). PuTTY, SecureCRT,
  and Tera Term draw their screens through GDI with no accessibility API at
  all, so they are gated on the M14 display model — with ssh from Windows
  Terminal as the recommended interim answer.
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
- **Tier E — after the JAB backend lands (M13)**: javaw and Eclipse.

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
