# NVDA parity ledger

This ledger tracks, per behavior, what NVDA does (linking the
`docs/nvda/` file that documents it) and where Verbatim stands. It is
the working checklist for parity reviews and the definition of done
for parity work: an "unverified" entry is a claim someone still has to
check against a live NVDA or against the cited NVDA source.

Statuses:

- **matched (verified)** — implemented and confirmed by a live E2E
  scenario or a cross-process test that asserts the NVDA-equivalent
  behavior.
- **matched (unverified)** — implemented to match the cited NVDA
  behavior, but no test or live comparison pins it yet. These are the
  entries most likely to hide inconsistency bugs.
- **different (Dn)** — deliberately divergent, recorded in
  [Architecture](architecture.md) decision Dn or noted inline in
  [the crate guides](crates/readme.md).
- **not yet (Mn)** — not implemented; scheduled milestone in
  parentheses.

Keep entries short; move discussion into the linked docs. When a
review confirms an entry, upgrade its status and note how it was
verified.

## Architecture-level stances

- Hang isolation. NVDA: one main thread plus watchdog cancellation
  ([Main loop and watchdog](nvda/main-loop-and-watchdog.md)). Verbatim: **different (D9,
  D13)** — per-app outpost processes; no output-producing thread may
  block cross-process. The evidence that NVDA's freezes come from
  synchronous cross-process calls is collected in that NVDA doc.
- Event intake. NVDA: single in-process winevent/UIA registration
  with foreground gating ([Event handling](nvda/events.md), [MSAA and winevent handling](nvda/msaa.md)).
  Verbatim: **different (D13)** — a dedicated no-cross-process-call
  focus-listener process plus per-app outposts.
- Extensions. NVDA: unrestricted Python add-ons
  ([App modules mechanism](nvda/app-modules-mechanism.md)). Verbatim: **not yet (M5)**,
  and **different (D6)** by design — sandboxed Wasm components.
- Injection. NVDA: universal lazy DLL injection
  ([Process injection](nvda/process-injection.md)). Verbatim: **not yet (M6)**, and
  **different (D2)**: injection only ever accelerates, never required
  for correctness.
- Display model. NVDA: GDI hooks power screen review
  ([The display model](nvda/display-model.md)). Verbatim: **different (D11)** —
  screen review will be a spatial projection of the tree (M6); GDI
  model deliberately last (M14, gated).
- Spoken vocabulary and key layouts. NVDA: the spoken role and state
  names, messages, synth setting labels, and the desktop and laptop
  gesture layouts ([Speech](nvda/speech.md), [Keyboard input](nvda/input.md)).
  Verbatim: **matched (verified)** by decision, not by accident: users'
  ears and hands already know them, so Verbatim speaks the same English
  words and binds the same keys wherever it implements the same command
  (E2E scenarios assert the wording; the gesture tables live in
  [verbatim-input](crates/verbatim-input.md)). Only the English catalogue
  is shared vocabulary; Verbatim's other-language resources are written
  fresh, never taken from NVDA's translations.

## Focus and announcements

- Focus announcement content and property order (name, role, value,
  states in fixed order, description, shortcut, position, level).
  NVDA: `speakObject` ordering ([Speech](nvda/speech.md)). Verbatim:
  **matched (verified)** for the M3 surface — E2E scenarios assert
  wording; per-property order transcribed in `verbatim-core`
  ([verbatim-core](crates/verbatim-core.md)).
- Focus-ancestry context: announce newly entered presentable
  containers before the control. NVDA: `focusEntered` +
  `isPresentableFocusAncestor` ([Event handling](nvda/events.md),
  [Object model](nvda/object-model.md)). Verbatim: **matched (unverified)** —
  filter claimed at parity with `_get_isPresentableFocusAncestor`
  (exclusion-based, same role exclusions), except:
- Top-level windows in the ancestry are never presented as entered
  containers (the foreground announcement owns them). NVDA would
  present a named window. **different (documented in
  [verbatim-core](crates/verbatim-core.md))**.
- Foreground/window announcement on app switch. NVDA: synthetic
  `foreground` event from focus processing ([Event handling](nvda/events.md)).
  Verbatim: **matched (verified)** for Start menu / window-switch
  scenarios; ordering under load handled by the announce lane
  (last commit 0a40653) — **recheck after that change settles**.
- Duplicate focus suppression (same control announced once when two
  paths report it). NVDA: "already the focus" early return.
  Verbatim: **matched (unverified)** ([verbatim-core](crates/verbatim-core.md),
  M3 noise suppression).
- Stale focus events: NVDA has no timestamp arbitration; it relies on
  queue-time freshness plus cancellable speech
  ([Event handling](nvda/events.md), [Speech](nvda/speech.md)). Verbatim:
  **different (documented)** — last-observation-wins timestamps in
  the reducer; windows still spoken, stale control focus dropped.
  NVDA-visible difference: none intended; verify with rapid
  focus-churn scenarios.
- Cancellation of expired focus speech (focus left before speaking).
  NVDA: `_CancellableSpeechCommand`. Verbatim: **not yet** — no
  equivalent validity check in the speech queue; Interrupt priority
  masks most cases. Candidate gap for fast-typing scenarios.
- Menu popup announcements. NVDA: menu events with fake-focus
  fallback ([MSAA and winevent handling](nvda/msaa.md)). Verbatim: **matched (verified)**
  for the Start menu path (MenuPopupStart event-driven); the fake
  focus fallback when no real focus event follows a menu open is
  **not yet** — NVDA fabricates focus on the menu item, Verbatim
  relies on the real event arriving.
- Toggle button role and "pressed"/"not pressed" wording (UIA Toggle
  pattern on a Button; no separate Switch role). NVDA: UIA
  detection. Verbatim: **matched (verified)** (cross-process test;
  commit c72afd8).
- Negated states: "not checked" for unchecked check box/radio, "not
  pressed" for toggle, "not selected" for selectable-unselected;
  positive "selected" never on node announcement. Verbatim:
  **matched (unverified)** — transcribed rules in reducer; needs a
  live NVDA comparison across roles.
- Selection announcements (focused list's selected child; changes
  while focus stays on container; combo box exclusion). NVDA:
  selection events. Verbatim: **matched (unverified)** ([verbatim-core](crates/verbatim-core.md), M3).
- Value change on focused node speaks bare value (slider drag).
  Verbatim: **matched (unverified)**. Background progress bar
  reporting (NVDA option): **not yet**.
- State-change diff announcements (gained states; checked-loss
  negation). Verbatim: **matched (unverified)**.
- UIA notification events (snap layout hints etc.), incl.
  interrupt-vs-queue by processing hint. NVDA:
  `event_UIA_notification` ([The UIA client](nvda/uia.md)). Verbatim:
  **matched (unverified)**, foreground-gated in shell.
- Live regions (browsers). NVDA: in-process IA2 machinery
  ([IA2 usage](nvda/ia2.md)). Verbatim: **not yet (M6)**.

## Object navigation and review

- Navigation tree shape. NVDA default: simple review on (filtered
  tree, [Focus and the navigator](nvda/focus-and-navigator.md)). Verbatim: full tree,
  matching NVDA with simple review **off** — the project's stated
  baseline (maintainer decision; commit 6b7a519). **matched
  (verified)** against that baseline; simple-review-on filtering is
  **not planned** (revisit only if the baseline changes).
- Navigator follows focus; review follows navigator. NVDA coupling
  rules ([Review modes](nvda/review-modes.md)). Verbatim: **matched
  (unverified)** for the follow-focus default; `followCaret` /
  `followMouse` equivalents **not yet (M4+)**.
- Report current object: report / spell / copy on 1st/2nd/3rd press.
  NVDA: script repeat counting ([Keyboard input](nvda/input.md)). Verbatim:
  **matched (verified)** (multi-press machinery in verbatim-input;
  copy via the shared clipboard helper).
- Parent/next/previous/first-child moves with edge reporting ("no
  parent" etc. spoken, not silence). Verbatim: **matched (verified)**
  (commits 9bd6bec, 0fe39f0); NVDA wording comparison still
  worthwhile.
- Sibling navigation resolving back to self reported as edge
  (MSAA `accNavigate` quirk). NVDA: fallback heuristics
  ([MSAA and winevent handling](nvda/msaa.md)). Verbatim: **matched (unverified)**
  (commit 0fe39f0).
- Windowed MSAA children navigated through the window hierarchy.
  NVDA: same strategy. Verbatim: **matched (verified)** (msinfo32
  E2E; commit d3e13b7).
- SysTreeView32 via TVM messages, Tree/TreeItem roles. NVDA:
  control-specific overlay. Verbatim: **matched (verified)**
  (tree_navigation E2E; commits 809214d, 1078614).
- Navigator death recovery: NVDA reports failure and stays; Verbatim
  re-seeds navigator from focus on `Gone` and announces it —
  **different (documented in [verbatim-core](crates/verbatim-core.md))**; NVDA-side
  behavior in [Focus and the navigator](nvda/focus-and-navigator.md).
- Review cursor line/word/character over object text. NVDA: object
  review over TextInfo ([Review modes](nvda/review-modes.md)). Verbatim:
  **matched (unverified)** at M3 fidelity (flat value/name text;
  grapheme/word segmentation deferred to M4 — a known divergence
  until then).
- Document review and screen review modes. Verbatim: **not yet
  (M6)**; screen review will be tree-projection **different (D11)**.
- Object activation (do default action). Verbatim: **matched
  (unverified)**.
- Move focus to navigator / caret routing. Verbatim: **not yet
  (M4)**.

## Backends

- Dual-stack MSAA+UIA with per-window arbitration. NVDA: the
  `isUIAWindow` referee with good/bad class lists
  ([The UIA client](nvda/uia.md)). Verbatim: **matched in architecture (D1)**;
  the arbitration probe exists, but the per-class scar-tissue lists
  are **not yet** transcribed — expect per-app fidelity differences
  until each is triaged (tracked per app-family as they land).
- UIA caching discipline (cache requests on events and walks). NVDA:
  `baseCacheRequest` pattern. Verbatim: **matched (unverified)** —
  cached elements + scoped search landed in 918e5b8/4563bff after a
  desktop-wide re-find bug; audit against NVDA's per-event cache
  contents still open.
- UIA event registration scope (global group vs focus-local
  high-frequency events). Verbatim: **partial** — focus/property
  registrations exist; the local-group re-registration pattern for
  text events is **not yet (M4)**.
- MSAA winevent flood control (per-thread caps, focus coalescing,
  latest-menu-only). NVDA: `OrderedWinEventLimiter`
  ([MSAA and winevent handling](nvda/msaa.md)). Verbatim: **not yet** — outposts rely on
  per-app isolation to bound damage, but no equivalent coalescing
  exists inside an outpost; flagged as a review question for busy-app
  scenarios.
- Event acceptance filtering (foreground gating, show/hide rules).
  NVDA: `shouldAcceptEvent`. Verbatim: **partial, different (D13)** —
  foreground gating lives in the shell/focus-listener design rather
  than per-event heuristics; background-app opt-ins (progress bars)
  **not yet (M8)**.
- IA2 upgrade of MSAA objects. NVDA: `normalizeIAccessible`
  ([IA2 usage](nvda/ia2.md)). Verbatim: **matched (unverified)** —
  verbatim-ia2 QueryService path exists ([verbatim-ia2](crates/verbatim-ia2.md));
  IA2 text/hypertext consumption **not yet (M4/M6)**.
- JAB. Verbatim: **not yet (M13)**.
- Remoted UIA trees (Application Guard-style: valid UIA, locally
  invalid window handles and process identity). NVDA: the WDAG
  pathway ([The UIA client](nvda/uia.md)). Verbatim: **not planned**
  — MDAG is deprecated and removed in Windows 11 24H2; noted because
  Verbatim's window-handle assumptions (arbitration, hierarchy
  navigation) would need a treat-as-remote pathway if a successor
  technology with the same shape appears.

## Speech and audio

- Priority lanes. NVDA: NORMAL/NEXT/NOW with resume of interrupted
  speech ([Speech](nvda/speech.md)). Verbatim: **partial** —
  Queued/Next/Interrupt exist; NVDA's *resume of interrupted
  lower-priority speech* is **not yet**: Verbatim's Interrupt
  discards. Decide whether to match before M8 profiles work.
- Index marks driving callbacks at audible position. NVDA: manager
  indexing + WASAPI feed-end callbacks ([Audio output](nvda/audio.md)).
  Verbatim: **matched (unverified)** — index-mark echoes exist in
  the synth contract; say-all (the main consumer) is **not yet
  (M4)**.
- Structured utterances vs flat strings. NVDA: command-laden flat
  sequences. Verbatim: **different (D12)** — typed spans flattened
  by a theme at the last stage.
- Speech settings model (driver settings, immediate application).
  NVDA: `SynthDriver.supportedSettings`. Verbatim: **matched
  (unverified)** — same descriptor-driven model.
- Instant cancel (stop + reset, audible immediately). NVDA:
  `WavePlayer.stop`. Verbatim: **matched (verified)** — WASAPI
  stop+reset; latency measured in the E2E ledger.
- Rate boost via sonic, audio ducking, sound split, tones/earcons:
  **not yet** (M8 ducking/config breadth, M11 earcons).
- Sonification (spelling-error sounds, mode-switch sounds, progress
  beeps, indentation tones) and its scheduling split between
  in-stream playback-synchronized commands and immediate
  fire-and-forget sounds. NVDA: [Sonification](nvda/sonification.md).
  Verbatim: **not yet (M11)**, and **different (D12)** by intent —
  themes should let one semantic event render as word, earcon, or
  parameter change; NVDA hard-codes the choice per feature. The
  inventory is the parity floor M11 must cover.
- Synth isolation. NVDA: in-process drivers (crash = NVDA crash),
  one out-of-process precedent ([Synth drivers](nvda/synth-drivers.md)).
  Verbatim: **different (D6)** — native synth host out of process
  (M7).
- Symbol/punctuation processing, speech dictionaries, character
  descriptions. NVDA:
  [Symbols, dictionaries, and character processing](nvda/symbols-and-dictionaries.md).
  Verbatim: **not yet (M8)** — D12 plans this over typed spans; the
  dictionaries-before-symbols ordering and the symbol preserve rules
  are the parity-critical details.
- Automatic language switching. NVDA: strip-or-pass of language
  commands per config ([Speech](nvda/speech.md)). Verbatim: **not
  yet** (unscheduled; needs backend language attributes first).

## Input

- NVDA-modifier semantics: swallow, double-tap passthrough,
  trapped-key release swallowing, unbound-chord fallthrough.
  NVDA: `keyboardHandler` ([Keyboard input](nvda/input.md)). Verbatim:
  **matched (verified by unit tests)** — semantics transcribed
  ([verbatim-input](crates/verbatim-input.md)); live side-by-side with NVDA still
  worth one session (sticky/locked modifier states **not yet**).
- Script repeat counting incl. auto-repeat exclusion. Verbatim:
  **matched (verified)**.
- Gesture map / rebindable input, input help mode. Verbatim: map
  exists; user rebinding UI and input help **not yet (M8/M9)**.
- Typed-character echo. NVDA: in-process reports; UIA textEdit
  events where applicable. Verbatim: **not yet (M4)** — decide the
  no-injection echo source (UIA events + polling?) explicitly
  against [Keyboard input](nvda/input.md).
- IME/composition reporting. Verbatim: **not yet** (unscheduled;
  needs a decision — NVDA's implementation is injection-dependent).
- Mouse tracking (text-unit speech, audio coordinates, injection
  filtering) and touch interaction. NVDA:
  [Mouse and touch](nvda/mouse-and-touch.md). Verbatim: **not yet**
  (unscheduled; mouse tracking becomes cheap once point-to-node and
  point-to-text resolution exist).

## Text, documents, terminals (M4/M6 previews)

- TextInfo-equivalent layer, caret-key reporting via
  wait-for-evidence, selection deltas, terminal diffing: all **not
  yet (M4)**; the NVDA references to design against are
  [TextInfo](nvda/text-infos.md) and
  [Editable text and terminals](nvda/editable-text-and-terminals.md).
- Word and character segmentation (Uniscribe grapheme clusters and
  word stops) and the three-way paragraph-style setting. NVDA:
  [TextInfo](nvda/text-infos.md). Verbatim: **not yet (M4)** — the
  review module's flat-text walk explicitly defers both.
- Browse mode, quick nav, pass-through rules, virtual-buffer
  equivalent: **not yet (M6)**; references
  [Browse mode](nvda/browse-mode.md), [Virtual buffers](nvda/virtual-buffers.md).
- Office object-model reading: **not yet** (app-module track);
  reference [Office through COM](nvda/office-com.md) before deciding whether
  Verbatim replicates the OM path or bets on modern-Office UIA.
- Document formatting reporting (the option vocabulary and the
  cache-and-diff announcement model). NVDA:
  [Document formatting reporting](nvda/document-formatting.md).
  Verbatim: **not yet (M4)** — the diff-not-per-run behavior is the
  parity-critical core.
- The role-shaped behavior layer (progress bars, dialog text
  harvesting, suggestion sounds, fake table rows, tooltips/toasts).
  NVDA: the behavior mixins ([Object model](nvda/object-model.md)).
  Verbatim: **partial** — selection and notification handling exist
  in the reducer; the rest lands per feature. Architectural question
  for the review: where is Verbatim's home for this layer?
- ARIA vocabulary, annotations/details, compound documents. NVDA:
  [ARIA, annotations, and compound documents](nvda/aria-and-annotations.md).
  Verbatim: **not yet (M6)**.
- Math (MathML pipeline, provider seam, interactive navigation).
  NVDA: [Math](nvda/math.md). Verbatim: **not yet** (in scope,
  unscheduled — maintainer decision 2026-07-17).

## System integration

- Vision framework (focus highlight, screen curtain, magnifier),
  OCR, secure screens, remote access: all **not yet** (M8 for OCR
  and secure desktop, M12 remote); references [The vision framework](nvda/vision.md),
  [OCR and content recognition](nvda/ocr-and-content-recognition.md),
  [Secure mode](nvda/secure-mode.md), [Remote access](nvda/remote-access.md).
- Elevated applications on the user desktop (admin consoles,
  elevated installers): reading them requires uiAccess — signed
  binaries in a trusted install location. NVDA:
  [Secure mode](nvda/secure-mode.md). Verbatim: **not yet** —
  unsigned dev builds cannot reach elevated windows at all; the
  signing/packaging work has no milestone, and M8's secure-desktop
  work depends on it. Recognize the symptom now: "Verbatim goes
  quiet in the admin prompt" is this, not a bug.
- Braille: **not yet (M15, D7)**; the inputs braille needs preserved
  are listed at the end of [Braille](nvda/braille.md).
- Configuration profiles and triggers (app, say-all, manual;
  queue-synchronized application to speech). NVDA:
  [Configuration and profiles](nvda/config-and-profiles.md).
  Verbatim: **not yet (M8)** — `verbatim-config` covers the base
  store only; the layering and trigger semantics are undesigned.
- Settings GUI generated from driver descriptors. NVDA:
  `AutoSettingsMixin` ([NVDA's GUI and the settings framework](nvda/gui-and-settings.md)).
  Verbatim: **matched (unverified)** in pattern — `SettingsHost`
  feeds the GUI from `SettingDescriptor`s; NVDA's panel framework
  and accessibility glue are the reference as the GUI grows.
- Logging and the log viewer. NVDA: [Logging](nvda/logging.md).
  Verbatim: **not yet (M9)** — tracing exists (flight recorder,
  latency ledger); user-facing logging is unbuilt.
- Developer console. NVDA: [The Python console](nvda/python-console.md).
  Verbatim: **not yet (M10)** — `verbatim-inspect` covers part of
  the inspection role today.
- Installation, portable copies, self-update, COM registration
  repair. NVDA: [Installation, portable copies, updates, and COM fixes](nvda/installation-and-updates.md).
  Verbatim: **not yet** (no packaging milestone yet).
