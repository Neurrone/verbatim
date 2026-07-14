# Verbatim: a Modern Windows Screen Reader

Verbatim is a rewrite of NVDA that attempts to fix the following problems.

## Status

See the [roadmap](docs/roadmap.md).

## Goals

- **Rust core on modern Windows**: Rewrite the screen reader core in Rust, targeting Windows 11 x64 and ARM as equal first-class platforms, with wxWidgets (via wxDragon) for a GUI that is itself accessible.
- **Responsiveness above all**: Never block on the foreground application. Use per-application "outposts" (or a comparable isolation mechanism) so a hung app can't hang Verbatim, and target low event-to-speech latency even on low-end hardware.
- **Functional core, imperative shell**: Model core logic as a reducer over the in-memory accessibility tree — events in, updated tree plus effects out (further API calls, speech, sounds, braille) — to make the logic highly testable.
- **Better UIA handling**: Fix pain points like terminal output floods, evaluate UIA Remote Operations for batched queries, and consider multiple UIA threads for responsiveness.
- **Sandboxed Wasm extension system**: Replace the broad Python add-on surface with a minimal, deliberately grown host API covering tree queries, event subscriptions, speech/braille output, configuration, and per-extension storage. App modules (Office, Terminal, browsers) live in this iteration-friendly layer.
- **Synthesizer extensibility**: Support porting synthesizers via two paths — Wasm for source-available synths and sandboxed native extensions for closed-source ones (Eloquence as the proof of concept) — both meeting the speech-latency target. Ship OneCore and eSpeak built in, on a WASAPI backend designed to allow alternative audio backends.
- **Feature parity and beyond**: Preserve or improve speech, braille, input gestures, review cursor/object navigation, browse mode, configuration profiles, localization, dictionaries, and secure desktop support, with multilingual support from day one. Add scan-mode-style navigation outside browsers, interaction on large web pages before full buffer render, focus highlight, screen curtain, and an architecture that doesn't preclude future magnification. Extensions can later trigger OCR and AI-generated synthetic accessibility objects.
- **Testability for LLM-driven development**: Fast, repeatable automated validation — unit tests against fake accessibility trees (mocked MSAA/IA2/UIA responses verifying tree construction), behavior tests over provided trees, and true E2E tests in a Windows 11 VM (with audio, start/stop/artifact copy, runnable locally and in GitHub Actions).
- **Dev tooling that becomes remote support**: Event inspection and simulated keyboard input for driving Verbatim in a VM, built so the same code paths later power a built-in remote-control feature.
- **Observability**: Correlate accessibility events with the speech actually heard, including event-observed-to-speech-queued latency.
- **Deferred**: Braille display support comes last; NVDA add-on binary compatibility is a non-goal, though important add-ons should be portable.
