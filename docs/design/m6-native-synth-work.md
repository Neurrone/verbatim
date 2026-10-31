# M6 work plan: the native synthesizer host and Eloquence

The order of work for M6, the tests that prove each step, and what each step leaves behind. The design is section 12 of `extensions.md` and D18 in the architecture; the roadmap's M6 entry points here and keeps the exit criterion: Eloquence speaks through Verbatim under the sandbox with the latency test green.

Vocabulary, defined in the design: the ABI is `verbatim_synth_v1`, the `SynthDriver` trait of `verbatim-speech` spelled as C exports; an adapter is the engine-specific DLL that implements the ABI and loads the engine; the host is `verbatim-synth-host`, built once per architecture, which loads only the adapter; the container is the Windows AppContainer the host runs in, in its less privileged form (LPAC) wherever the engine allows.

Review per `REVIEW.md` after each step. The `speech_` and latency scenarios run after every step; the full suite runs once at the end, because the host start path changes.

## Order of work

### Step 1: the ABI and the adapter crate

What: `verbatim-synth-abi`, a platform-neutral crate holding the C ABI as Rust declarations with the C calling convention and a generated header: version, create, destroy, describe (id, display name, settings with types and bounds, places marks, changes pitch), set a setting, speak a sequence through PCM, mark and is-cancelled callbacks, cancel, with every string a pointer and a length and every buffer owned by its allocator. `verbatim-synth-adapter`, the crate an adapter author depends on: implement `SynthDriver` in Rust, and a macro emits the exports. A test adapter in the repository, built for the host's architecture, wrapping the existing tone synthesizer. Also here: the `verbatim:synth` world, the `SynthDriver` trait in WIT, hosted by `verbatim-synth-host --wasm`, with a tone synthesizer component as the Wasm twin of the test adapter, so both kinds share one host and one wrapping from the start.

Tests: a unit test in the host crate loading the test adapter DLL and round-tripping describe, every setting type, a sequence with marks (PCM length and mark order exact), cancel mid-utterance, and a version mismatch refused with the exact message; the header compiles in a C translation unit in the build. The same round trip through the tone component, and a trapping component replaced without the host process dying.

### Step 2: the host loads adapters, matched by architecture

What: `verbatim-synth-host --native <bundle>` loads the adapter named for the host's architecture in the bundle's `module.toml`, wraps it as a `SynthDriver`, and speaks the D18 protocol unchanged. Core picks the host binary from the manifest's architecture: the ordinary build for the machine's own, and the x86 build for 32-bit engines; an x64 adapter on an ARM64 machine is unsupported until an `x86_64-pc-windows-msvc` cross-build is added, which Eloquence does not need. The x86 build is a cross-compile for `i686-pc-windows-msvc` in `cargo xtask`, the one build-rule exception for native engines, shipped with every install, and it is a reduced build carrying only the adapter loader and the protocol, not eSpeak NG or OneCore, so no 32-bit C build is needed (Dickson, 2026-10-22); `cargo xtask ci` builds and tests it. A synthesizer module of kind native in a bundle, with `user-files`, installs through the M5 path and shows the native-code sentence first in the install dialog; the Extensions dialog shows "add the engine files" while any named file is missing.

Also here, since this step changes the host and adds its second build (Dickson, 2026-10-31): the host's command handling as one pure module both builds share, commands in and speaking, stopping, answering an utterance cancelled before it began and applying a setting out; and `HostedSynth`'s view of the host as a pure state, what the host owes, what drains, and the hang rule, a host hung once it has owed a message for the timeout with nothing sent (2bdfffb7, where a host frozen while every utterance was cancelled was never judged hung). Both get generated tests against a model of the pair, joined by a pretend pipe.

Tests: the generated tests' checks that every `Speak` ends once, nothing of a cancelled utterance plays, the next `Speak` goes right behind a `Cancel`, and a host that stops answering is judged hung within the timeout; a unit test choosing the host binary for each architecture and manifest combination; the test adapter packaged as a native synthesizer `.vext`, installed in an end-to-end scenario, selected in speech settings, and the latency scenario's utterances complete through it; a bundle with a missing user file shows the exact status text and is not selectable; the x86 host's own unit tests run in CI on x64.

### Step 3: the container

What: `verbatim-process` gains a contained launch: an AppContainer profile per host with no capabilities, the less privileged flag, Low integrity, the pipe handle inherited, and the access entries for "all restricted application packages" set by the contained launch itself on the host executable's folder and the bundle folder at launch, since Core owns both and no installer exists yet (the end-to-end suite stages a fresh folder per run), the installer later only avoiding the repeat; the job object as today, and the mitigations the design lists (no remote images, no low-label images, extension points disabled). Each hosted engine declares the strictest level it is known to work under: the tone and test adapters and eSpeak NG under the less privileged container, OneCore under the ordinary container because WinRT activation needs it, native adapters starting at the less privileged container. A fall-back from the less privileged container to the ordinary one needs the failing call named in the design and is recorded as a workaround.

Tests: a probe child, built for the tests, launched under each level and asked to open a file outside its folders, open a registry key for writing, connect a socket, open Core's process, send a message to Core's hidden window and open the clipboard, each attempt reported back over the pipe and each asserted denied (window enumeration is not denied by the sandbox and is not asserted); the same probe asserting it reads its own folder and starts at all under the less privileged container; eSpeak NG and OneCore end-to-end under their declared levels with the latency scenario green; a host that exits while speaking is replaced and the utterance in flight ends as failed, as today.

### Step 4: the Eloquence adapter

What: `verbatim-synth-eloquence`, a Rust `cdylib` built for x86 only and shipped as `eloquence-x86.dll` (adapters are named `<engine>-<arch>.dll`, modules `<module id>.wasm`), written from IBM's documented SDK: loads `ECI.DLL` from the bundle's engine folder, creates an instance, registers the waveform and index callback, maps the engine's parameters to settings (voice as a choice over installed languages and the engine's voices, rate, pitch baseline, inflection, volume, breathiness, head size, roughness), feeds text with an inserted index per mark, synthesizes and synchronizes, delivers 16-bit mono 11025 Hz buffers to the PCM callback, stops on cancellation at the next callback, and reports pitch as changeable. Its bundle in the repository: `module.toml` with the x86 library and `user-files` naming `ECI.DLL`, `ECI.INI` and the voice files, an empty engine folder, and a readme telling the user where their own copy's files go. Core's mixer resamples; the host renders nothing.

Tests: the adapter's own unit tests over the parameter mapping and the index-to-mark table, the table also by generated sequences of text and marks (every mark reported once and in order, the PCM length exact); an integration test through the host with `ECI.DLL` absent asserting the unavailable status and message; an `eloquence_` end-to-end group, run only on request like `demo_` and `measure_`, on a machine with the engine files added, each scenario failing rather than skipping when the files are missing; CI never runs them, Dickson runs them by hand with the engine files, and that run is the exit evidence recorded in the roadmap (Dickson, 2026-10-22): the latency scenario within the same budget as the built-in engines, a sequence with marks reporting every mark, cancel within the cancel budget, a rate change applied to the next utterance, and a voice change. Reading an elevated window needs UIAccess and is M8.1's scenario, with whichever synthesizer is selected.

### Step 5: the shared-memory PCM ring, only if measured necessary

What: pipes until numbers prove otherwise (Dickson, 2026-10-22), as D18 says. The latency and cancel scenarios of step 4 decide: if the pipe path meets the budget for Eloquence, this step is closed as not needed and the roadmap entry says so; if not, a shared-memory ring for PCM with the pipe kept for control, measured against the same scenarios.

Tests: the latency scenario before and after, both recorded in the performance ledger.

### Step 6: documents

What: D18 amended for adapters, the per-engine container level and the x86 host; crate documents for the ABI, adapter and host crates; the user guide page for adding engine files; the engine-author page in the extensions guide (compile to Wasm when source exists, write an adapter otherwise); the M6 entry closed into `roadmap-done.md`.

## Depends on and feeds

Depends on M5's component host, install path, Extensions dialog and `module.toml` for synthesizer modules; the `verbatim:synth` world is this milestone's, in step 1. Feeds M8.1, where the machine-wide copy for sign-in makes native engines available on the secure desktop; the container work here is what makes that safe.
