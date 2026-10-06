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
replaces the former single `docs/overview.md`). The rule has a second
half that experience says is the one that actually slips: when a change
lands behavior another crate's guide *describes* — a "not yet" that
becomes true, a policy another file summarizes — search the docs for
claims about that behavior and update them too. Both stale-claim
incidents found in review survived precisely because the landing
commit updated only the changed crate's own guide.

## The crates, in dependency order

- [verbatim-model](verbatim-model.md) — the normalized tree, event, and
  identity vocabulary every other crate speaks.
- [verbatim-i18n](verbatim-i18n.md) — Fluent localization (D10).
- [verbatim-config](verbatim-config.md) — settings schema and persistence.
- [verbatim-input](verbatim-input.md) — the pure gesture decision machine,
  key names, and gesture tables.
- [verbatim-input-windows](verbatim-input-windows.md) — the keyboard hook
  thread.
- [verbatim-audio](verbatim-audio.md) — the mixer, the AudioDevice seam,
  and the silent device (D5, D17).
- [verbatim-audio-wasapi](verbatim-audio-wasapi.md) — the WASAPI device.
- [verbatim-speech](verbatim-speech.md) — priority lanes, synth threads,
  themes (D12), the settings host, and the synthesizer host protocol
  (D18).
- [verbatim-synth-onecore](verbatim-synth-onecore.md) — the OneCore driver.
- [verbatim-synth-espeak](verbatim-synth-espeak.md) — the eSpeak NG
  driver, built from the vendored source; the default synthesizer.
- [verbatim-synth-capture](verbatim-synth-capture.md) — the audio-free test
  synthesizer.
- [verbatim-process](verbatim-process.md) — contained child processes:
  kill-on-close jobs, inherited pipes, and per-launch child logs.
- [verbatim-synth-hosted](verbatim-synth-hosted.md) — `HostedSynth`, the
  driver that runs a synthesizer in a host process (D18).
- [verbatim-synth-host](verbatim-synth-host.md) — `verbatim-synth-host.exe`,
  one synthesizer in its own process.
- [verbatim-core](verbatim-core.md) — the pure reducer, flight recorder,
  and dump format.
- [verbatim-uia](verbatim-uia.md) — the UIA client stack.
- [verbatim-uia-rops](verbatim-uia-rops.md) — UIA remote operations:
  programs that run inside a provider's process, and the focus ancestry
  in one round trip.
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
