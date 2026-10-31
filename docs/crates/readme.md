# Crate guides

A reviewer's guide to the workspace, one file per crate. Crates appear
below in dependency order — each one uses only concepts explained
before it — so reading top to bottom works front to back.
[Architecture](../architecture.md) holds the decisions of record this
code implements; [the roadmap](../roadmap.md) holds the milestone
scoping; [Walkthroughs](../walkthroughs.md) stitches these crates into
end-to-end narratives. Where a method's behavior is not obvious from
its signature, an implementation note explains how it works and why.

Keep the file for a crate current when its public API changes. The rule
has a second half, the one that slips: when a change lands behavior
another crate's guide describes, such as a "not yet" that becomes true or
a policy another file summarizes, search the docs for claims about that
behavior and update them too, because a commit that updates only the
changed crate's own guide leaves those claims stale.

## The crates, in dependency order

- [verbatim-model](verbatim-model.md) — the normalized tree, event, and
  identity vocabulary every other crate speaks.
- [verbatim-i18n](verbatim-i18n.md) — Fluent localization (D10).
- [verbatim-config](verbatim-config.md) — settings schema and persistence.
- [verbatim-input](verbatim-input.md) — the pure gesture decision machine,
  key names, and gesture tables.
- [verbatim-input-windows](verbatim-input-windows.md) — the keyboard hook
  thread.
- [verbatim-text](verbatim-text.md) — text segmentation: characters,
  words, and sentences by Unicode's rules, and terminal cell widths.
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
  kill-on-close jobs, inherited pipes, and per-launch child logs; unnamed
  overlapped pipes and a completion port; thread priorities; and what
  every end of Core's children shares: the message layer, the launch
  arguments, the held processes, a child's start and end, and the alarm.
- [verbatim-trace-waits](verbatim-trace-waits.md) — how long threads
  waited to run, from a WPR trace (`cargo xtask trace waits`).
- [verbatim-term-sim](verbatim-term-sim.md) — a simulated terminal for the
  tests, checked against the real console host and Windows Terminal.
- [verbatim-test-console](verbatim-test-console.md) — a console the tests
  start, for tests of a console window's provider process; a
  dev-dependency only.
- [verbatim-synth-hosted](verbatim-synth-hosted.md) — `HostedSynth`, the
  driver that runs a synthesizer in a host process (D18).
- [verbatim-synth-host](verbatim-synth-host.md) — `verbatim-synth-host.exe`,
  one synthesizer in its own process.
- [verbatim-core](verbatim-core.md) — the pure reducer, flight recorder,
  and dump format.
- [verbatim-reader-rules](verbatim-reader-rules.md) — what both readers
  apply before reading: NVDA's class verdict and acceptance test, and the
  waits they share.
- [verbatim-outpost-logic](verbatim-outpost-logic.md) — the outpost's
  reducer, a pure state machine.
- [verbatim-listener-logic](verbatim-listener-logic.md) — the focus
  listener's read scheduler, menu end's grace, foreground watch and focus
  vocabulary, pure.
- [verbatim-win32](verbatim-win32.md) — the local window and process
  queries more than one role makes.
- [verbatim-uia](verbatim-uia.md) — the UIA client stack.
- [verbatim-uia-rops](verbatim-uia-rops.md) — UIA remote operations:
  programs that run inside a provider's process, and the focus ancestry
  in one round trip.
- [verbatim-ia2](verbatim-ia2.md) — the MSAA/IA2 client stack.
- [verbatim-outpost](verbatim-outpost.md) — the per-application outpost
  process.
- [verbatim-listener](verbatim-listener.md) — the focus listener process
  (D13).
- [verbatim-supervisor](verbatim-supervisor.md) — Core's side of starting,
  watching and ending the outposts and the listener.
- [mockapp](mockapp.md) — scripted accessibility providers for
  cross-process tests.
- [verbatim-control](verbatim-control.md) — the control-plane protocol and
  server (D8).
- [verbatim-inspect](verbatim-inspect.md) — the CLI client for a running
  Verbatim.
- [verbatim-agent](verbatim-agent.md) — the in-guest test agent and
  control tunnel.
- [verbatim-e2e](verbatim-e2e.md) — the end-to-end scenario registry.
- [verbatim-gui](verbatim-gui.md) — the wxWidgets GUI through a small C++ layer (D4).
- [verbatim-app](verbatim-app.md) — `verbatim.exe`: wiring it all together.

Two crates are stubs: `verbatim-ext` and `verbatim-ext-api`, the Wasm
extension host and its WIT contract, which land in M5.

`xtask`, the workspace automation, is described in the tooling guides:
`cargo xtask ci` in [setup](../tooling/setup.md), `cargo xtask vm` with a
map of its code in [the VM harness guide](../tooling/vm.md),
`cargo xtask park` in [the end-to-end guide](../tooling/e2e.md), and
`cargo xtask nvda` in [the NVDA guide](../tooling/nvda.md).
