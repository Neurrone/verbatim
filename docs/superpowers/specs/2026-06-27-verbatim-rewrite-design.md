# Verbatim Rewrite Design

## Purpose

This document records the accepted high-level design for Verbatim, a Rust rewrite of NVDA targeting Windows 11 x64 and Windows 11 on ARM64. It is the entry point for durable architecture, tooling, parity, observability, and phase specifications.

## Project Goals

| Goal | Requirement |
| --- | --- |
| Rust-first architecture | Use Rust for the core architecture, especially performance-sensitive, concurrency-sensitive, and correctness-sensitive components |
| Windows 11 only | Support Windows 11 x64 and Windows 11 ARM64; treat ARM64 as first-class with the same latency targets as x64 |
| No legacy OS support | Do not spend architecture budget on legacy Windows or applications that require unsupported Windows versions |
| Responsive reader | The core must not block on the foreground application, a hung provider, a COM call, a native DLL, or an extension |
| Single latency target for x64 and ARM64 | Cacheable focus event observed to speech audio started targets p95 under 20 ms on both x64 and ARM64 |
| Consumer-side normalized tree | Consume UIA, MSAA, IA2, and later Java Access Bridge through outposts, then normalize into Verbatim's own incremental snapshot tree |
| AccessKit-inspired internals | Borrow stable IDs, incremental updates, atomic snapshots, and clean tree representation ideas for the reader's internal model |
| Functional core, imperative shell | Keep behavior reducers, tree updates, output planning, and permission checks deterministic and testable; isolate COM, hooks, devices, IPC, and native loading in shells |
| UIA improvement | Prioritize UIA reliability and responsiveness, especially Windows shell and terminal scenarios with high-volume output |
| Sandboxed extensions | Replace NVDA's broad Python add-on surface with a minimal, capability-scoped Wasm extension system |
| App modules as extensions | Preserve application-specific support through extensions for Terminal, browsers, Electron, Office, and other common apps |
| Synth extensibility | Support built-in and extension-provided speech synthesizers, including controlled native DLL loading outside the core |
| Audio flexibility | Route speech, tones, and sound cues through an audio output engine with pluggable local, remote, fake, spy, and secure backends |
| Feature parity route | Preserve or improve speech, braille, gestures, review cursor, object navigation, browse mode, profiles, localization, dictionaries, secure desktop behavior, and common app support |
| Multilingual from the start | Treat localization, pronunciation dictionaries, symbol dictionaries, and multilingual synth behavior as core design requirements |
| Secure desktop | Provide a hardened secure-desktop instance with separate policy, explicit per-extension allowlisting, and reduced capabilities |
| Scan navigation beyond browsers | Provide scan-mode-like navigation for non-browser applications when a document-like projection is useful |
| Incremental browser interaction | Allow browser and Electron interaction before a full virtual buffer or browse projection render completes |
| Recognition later | Add OCR and AI recognition in later phases, including rerun-on-update and extension-contributed models/providers |
| Visual support | Support focus highlighting and screen curtain before magnification; keep magnification possible after braille |
| Remote setups | Add an NVDA Remote-like phase using the shared remote substrate, with command-mode output and optional remote audio backends |
| Incremental delivery | Every phase must produce a minimally usable reader capability or independently testable subsystem |
| LLM-friendly development | Build fake providers, replay tests, query tools, traces, CI checks, benchmarks, and VM/container tooling so humans and LLM agents can validate behavior repeatedly |
| Continuous parity | Compare against NVDA and Verbatim traces throughout development, not as a final release task |
| Observability | Trace latency from event observation, to tree commit, to output queueing, to synthesis/audio, to finished output |

## Non-Goals

| Non-goal | Consequence |
| --- | --- |
| Legacy Windows support | Windows versions before Windows 11 are unsupported |
| Old application compatibility for unsupported OSes | Applications that only run on unsupported Windows versions are out of scope |
| Binary compatibility with NVDA add-ons | Important add-ons should have a porting path to Verbatim extensions, not binary compatibility |
| Broad initial extension API | Host APIs start minimal and expand only when a real feature or port needs them |
| In-process provider injection as a baseline design | In-process injection may be considered only as a measured late optimization with clear isolation and security justification |
| Magnification in early phases | Magnification is supported later, after braille; earlier visual architecture must not block it |

## Design Map

| Area                                   | Specification                                |
| -------------------------------------- | -------------------------------------------- |
| Architecture overview                  | `docs/architecture/README.md`                |
| Hot path and latency contract          | `docs/architecture/hot-paths.md`             |
| Process model and crash isolation      | `docs/architecture/process-model.md`         |
| COM apartment and threading model      | `docs/architecture/com-threading.md`         |
| Internal accessibility tree model      | `docs/architecture/tree-model.md`            |
| Extensions and speech synthesizers     | `docs/architecture/extensions-and-synths.md` |
| Audio output and backend handling       | `docs/architecture/audio-output.md`          |
| Remote setups                           | `docs/architecture/remote-setups.md`         |
| Visual output and future magnification  | `docs/architecture/visual-output.md`         |
| OCR and AI recognition                  | `docs/architecture/recognition.md`           |
| Secure desktop and service constraints | `docs/architecture/secure-desktop.md`        |
| Observability and latency tracing      | `docs/observability/trace-schema.md`         |
| NVDA parity strategy                   | `docs/parity/README.md`                      |
| Developer tooling and VM lab           | `docs/tooling/README.md`                     |
| Phase roadmap                          | `docs/phases/README.md`                      |
| Outpost decision record                | `docs/adr/0001-use-outpost-processes.md`     |

## Documentation Ownership

This file is the project charter and reading map. It should preserve goals and non-goals, not duplicate detailed architecture, latency tables, diagrams, or phase plans.

| Topic | Source of truth |
| --- | --- |
| Process boundaries and system diagram | `docs/architecture/README.md` |
| Input, output, logging, IPC, synth, and audio hot-path rules | `docs/architecture/hot-paths.md` |
| Outpost process model and crash isolation | `docs/architecture/process-model.md` |
| COM apartments, deadlines, and provider-call responsiveness | `docs/architecture/com-threading.md` |
| Latency trace fields, metrics, and benchmark output formats | `docs/observability/trace-schema.md` |
| Phase order, phase gates, and phase acceptance criteria | `docs/phases/README.md` |
| CI, benchmarks, query tools, and VM/container lab | `docs/tooling/README.md` |
| NVDA and Verbatim parity checks | `docs/parity/README.md` |

When a goal changes, update this file. When an implementation decision, latency budget, test artifact, or phase deliverable changes, update the owning document instead and keep this file as a pointer.
