# How NVDA works

This folder documents how NVDA — the reference screen reader — implements
its important, nontrivial features. It exists so that Verbatim work can
answer "what does NVDA actually do here?" without re-deriving it from
NVDA's source every time, and so that parity claims can be checked against
something written down.

Rules for this folder:

- **NVDA only.** These files describe NVDA's behavior and implementation,
  not Verbatim's. Comparisons, divergence decisions, and Verbatim status
  live in [Architecture](../architecture.md) (decisions of record) and
  [Parity ledger](../parity.md) (the per-behavior ledger).
- **Cited.** Every nontrivial claim names the NVDA source file (and
  function or class) it comes from, as a path relative to the `nvda/`
  submodule. If a claim is uncited, treat it as a summary of the cited
  material around it — and if it matters, verify it.
- **Versioned.** Written against the pinned submodule commit
  `92942556f` (post-2026.2beta6). NVDA moves; when the submodule is
  updated, spot-check claims in the files you rely on.
- **Depth target**: enough detail that a Verbatim task can proceed without
  reading NVDA source for the common cases, with precise pointers into the
  source for the rest. No app-module content unless a feature required
  special support from NVDA's core (those cases — Office, browsers — have
  their own files).

Windows API background assumed by these files is in `docs/explainers/`;
read that folder first if MSAA, IA2, UIA, COM apartments, winevents, or
Windows IPC primitives are unfamiliar.

## Index

Core machinery:

- [Main loop and watchdog](main-loop-and-watchdog.md) — the core pump, the queueing model, and
  the watchdog's freeze detection and call-cancellation recovery. Includes
  the evidence for *why* synchronous cross-process calls are NVDA's hang
  mechanism.
- [Event handling](events.md) — how accessibility events are queued, filtered, and
  dispatched through app modules, tree interceptors, and NVDA objects.
- [Object model](object-model.md) — NVDAObjects: the normalized node abstraction over
  all APIs, overlay classes, and API-class selection.
- [App modules mechanism](app-modules-mechanism.md) — how per-app modules, global plugins, and
  add-ons hook the core (mechanism only).
- [Configuration and profiles](config-and-profiles.md) — the layered config store,
  validation, feature flags, and profile triggers.

Accessibility backends:

- [MSAA and winevent handling](msaa.md) — winevent handling, the ordered winevent limiter, and MSAA
  object wrapping.
- [IA2 usage](ia2.md) — IA2 discovery, usage, and live regions.
- [The UIA client](uia.md) — the UIA client: threading, event registration, caching,
  per-app suitability decisions.
- [UIA remote operations](uia-remote-ops.md) — NVDA's bytecode-based UIA remote operations
  system.
- [Process injection](process-injection.md) — nvdaHelper: injection, the in-process RPC
  servers, API hooking, and the per-app in-process support inventory.
- [Java Access Bridge](java-access-bridge.md) — Java application support via the Access
  Bridge.

Documents and text:

- [Virtual buffers](virtual-buffers.md) — the C++ virtual buffer machinery for browsers
  and PDFs.
- [Browse mode](browse-mode.md) — the user-facing document navigation model and its
  UIA (non-buffer) variant.
- [TextInfo](text-infos.md) — the TextInfo abstraction that underlies all text
  reading.
- [Editable text and terminals](editable-text-and-terminals.md) — caret monitoring, typed echo at the
  document, and console/terminal support.
- [The display model](display-model.md) — the GDI-hook screen text model (legacy screen
  review).
- [Document formatting reporting](document-formatting.md) — the formatting option
  vocabulary and the cache-and-diff announcement model.
- [ARIA, annotations, and compound documents](aria-and-annotations.md) — the ARIA
  vocabulary mapping, details/annotation reporting, and documents
  spanning many objects.
- [Math](math.md) — MathML as interchange, the provider seam, MathCAT, and
  interactive math navigation.

Cursors and navigation:

- [Focus and the navigator](focus-and-navigator.md) — focus tracking, the navigator object, and
  object navigation with simple review on and off.
- [Review modes](review-modes.md) — object, document, and screen review.

Output:

- [Speech](speech.md) — speech sequences, the speech manager, priorities, index
  callbacks, say-all, and automatic language switching.
- [Symbols, dictionaries, and character processing](symbols-and-dictionaries.md) —
  speech dictionaries, symbol levels, and character descriptions: the
  text rewriting between content and synthesizer.
- [Synth drivers](synth-drivers.md) — the synthesizer driver contract.
- [Audio output](audio.md) — WASAPI playback, ducking, and tones.
- [Sonification](sonification.md) — the non-speech sound inventory and
  how sounds schedule against speech (in-stream vs immediate).
- [Braille](braille.md) — braille display output, routing, and input.

Input:

- [Keyboard input](input.md) — keyboard interception, gesture mapping, script resolution,
  and IME/composition reporting.
- [Mouse and touch](mouse-and-touch.md) — mouse tracking with audio coordinates, and
  the touchscreen gesture system.

Application families needing core support:

- [Office through COM](office-com.md) — reading Word, Excel, and Outlook through the Office
  COM object models, in- and out-of-process.

System integration:

- [The vision framework](vision.md) — the vision framework: focus highlight, screen curtain,
  and the in-development magnifier.
- [OCR and content recognition](ocr-and-content-recognition.md) — Windows OCR and the content
  recognition framework.
- [Secure mode](secure-mode.md) — secure screens, secure mode, and the slave process.
- [Remote access](remote-access.md) — NVDA Remote (built-in remote access): protocol,
  roles, and what is relayed.
- [NVDA's GUI and the settings framework](gui-and-settings.md) — the wx GUI, the
  settings-panel framework, and driver-setting auto-generation.
- [Logging](logging.md) — levels, categories, redaction, and the log viewer.
- [The Python console](python-console.md) — the live-inspection REPL and its
  snapshot variables.
- [Installation, portable copies, updates, and COM fixes](installation-and-updates.md)
  — the launcher/installed/portable trio, self-update, and the COM
  registration repair tool.
