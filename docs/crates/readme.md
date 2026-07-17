# Crate guides

A reviewer's guide to the workspace, one file per crate. Crates appear
below in dependency order — each one uses only concepts explained
before it — so reading top to bottom works front to back.
[Architecture](../architecture.md) holds the decisions of record this
code implements; [the roadmap](../roadmap.md) holds the milestone
scoping; [Walkthroughs](../walkthroughs.md) stitches these crates into
end-to-end narratives. Where a method's behavior is not obvious from
its signature, an implementation note explains how it works and why.

Keep the file for a crate current when its public API changes (this
replaces the former single `docs/overview.md`).

## The crates, in dependency order

- [verbatim-model](verbatim-model.md) — the normalized tree, event, and
  identity vocabulary every other crate speaks.
- [verbatim-i18n](verbatim-i18n.md) — Fluent localization (D10).
- [verbatim-config](verbatim-config.md) — settings schema and persistence.
- [verbatim-input](verbatim-input.md) — the keyboard hook and the pure
  gesture decision machine.
- [verbatim-audio](verbatim-audio.md) — the AudioSink seam and WASAPI (D5).
- [verbatim-speech](verbatim-speech.md) — priority lanes, synth threads,
  themes (D12), and the settings host.
- [verbatim-synth-onecore](verbatim-synth-onecore.md) — the OneCore driver.
- [verbatim-synth-capture](verbatim-synth-capture.md) — the audio-free test
  synthesizer.
- [verbatim-core](verbatim-core.md) — the pure reducer, flight recorder,
  and dump format.
- [verbatim-uia](verbatim-uia.md) — the UIA client stack.
- [verbatim-ia2](verbatim-ia2.md) — the MSAA/IA2 client stack.
- [verbatim-outpost](verbatim-outpost.md) — per-app outposts, the focus
  listener (D13), the supervisor, and their protocol.
- [mockapp](mockapp.md) — scripted accessibility providers for
  cross-process tests.
- [verbatim-control](verbatim-control.md) — the control-plane protocol and
  server (D8).
- [verbatim-inspect](verbatim-inspect.md) — the CLI client for a running
  Verbatim.
- [verbatim-agent](verbatim-agent.md) — the in-guest test agent and
  control tunnel.
- [verbatim-e2e](verbatim-e2e.md) — the end-to-end scenario registry.
- [xtask VM harness](xtask-vm-harness.md) — `cargo xtask vm` implementation.
- [verbatim-gui](verbatim-gui.md) — the wxDragon GUI (D4).
- [verbatim-app](verbatim-app.md) — `verbatim.exe`: wiring it all together.
- [Placeholders and tooling](placeholders-and-tooling.md) — stub crates and
  xtask itself.
