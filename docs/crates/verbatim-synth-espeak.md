# verbatim-synth-espeak

The eSpeak NG driver, and Verbatim's default synthesizer. eSpeak NG is
built from the vendored source in `third_party/espeak-ng` (a git
submodule, eSpeak NG master at commit `ba90c8e9`, GPL version 3 or later)
and linked statically into this crate. Verbatim runs it only inside the
synthesizer host process (decision D18), never in `verbatim.exe`:
[verbatim-synth-host](verbatim-synth-host.md) builds it when started with
`--synth espeak`, and [verbatim-app](verbatim-app.md) registers it
through [verbatim-synth-hosted](verbatim-synth-hosted.md). The driver is
written from eSpeak NG's public C API (`speak_lib.h`). Its Rust
dependencies are `verbatim-audio`, `verbatim-speech`, and `tracing`.

Public API:

- `EspeakSynth::new()` — initializes eSpeak NG with its data directory
  (see Data below) and selects the default voice. Returns
  `SynthError::Unavailable` when this process already has an eSpeak NG
  driver, or when eSpeak NG cannot start with its data or finds no
  voices in it.
- `EspeakSynth::with_data(path)` — the same, with the data directory at
  `path`.
- `ESPEAK_ID` — the driver's id, `espeak`, which is
  `verbatim_speech::hosting::synth_ids::ESPEAK`.

Everything else is the `SynthDriver` trait.

Building. `build.rs` runs eSpeak NG's own CMake build through the `cmake`
crate, which needs CMake installed and uses Visual Studio's generator. It
fails with the `git submodule update --init third_party/espeak-ng`
command to run when the submodule is missing. Only the C parts are
built: mbrola, sonic, pcaudio, the C++ speech player, asynchronous
output, and the tests are off, and the Klatt synthesizer is on, so
nothing is needed beyond the C runtime the Rust build already uses. The
library is always built in the Release configuration, whatever cargo's
profile, so a debug build links the same C runtime flavor (`/MD`) as the
Rust standard library. The build target is eSpeak NG's `data` target,
which builds the static library and the `espeak-ng` program and then runs
that program to compile the phoneme and dictionary data. The crate links
the `espeak-ng` and `ucd` static libraries, and `advapi32`, since eSpeak
NG's data-path lookup reads the registry. On an ARM64 host the build asks
Visual Studio for its ARM64-hosted toolset, because the `cmake` crate
otherwise asks for the x64-hosted one, which an ARM64 machine may not
have.

Data. The compiled data, `espeak-ng-data`, is about 30 MB. The build
copies it next to the executables in `target/<profile>`, replacing any
older copy, and bakes that path into the crate as
`VERBATIM_ESPEAK_BUILD_DATA`. At run time the driver looks for
`espeak-ng-data` next to the running executable first and falls back to
the build's copy. Anything that runs Verbatim from another folder must
copy the directory there too: the end-to-end suite's staging does, and
so does `cargo xtask vm deploy`. The library's own compiled-in fallback
path is set to a fixed `/espeak-ng` prefix, so it never names the build
directory; the driver always passes its own path.

Implementation notes, one driver per process. eSpeak NG keeps its state
in globals, so `new` refuses a second driver while one exists, using an
atomic flag that `Drop` clears after calling `espeak_Terminate`, so a
later driver in the same process starts eSpeak NG from a clean state.
Before starting eSpeak NG, `new` checks that its data directory holds
`phontab`: given a path without data, eSpeak NG would otherwise fall back
to an environment variable or another installation's registry entry and
load data that is not its own. A voice or variant eSpeak NG refuses is
reported and not stored. The synthesizer host builds exactly one
driver, so this costs Verbatim nothing; it is why the crate's
integration test is a single test function.

Synthesis. eSpeak NG runs in synchronous mode: `espeak_Synth` blocks and
hands audio to a callback in chunks of about 20 ms as it synthesizes,
many times faster than real time. The callback pushes each chunk to the
sink, and returns non-zero to abort synthesis when the sink's `push_pcm`
returns `Break`, so a cancel takes effect within one chunk. A sink that
already reports the utterance cancelled, or a sequence whose text is
blank, produces nothing. The audio is 16-bit mono at the sample rate
eSpeak NG reports when it starts, 22050 Hz. A cancelled utterance
returns success, as with any driver; an error code from `espeak_Synth`
is `SynthError::Synthesis`.

Index marks and input. `places_marks` is `false`, and the speech manager
splits each sequence at its marks before it reaches the driver, which
keeps every mark exact (decision D17). The driver does not place marks
because eSpeak NG drops an SSML mark that follows a full stop (true up to
at least eSpeak NG master in September 2026). The driver therefore
passes the sequence's text items, concatenated, as plain UTF-8, not
SSML; other items are ignored.

Settings. The driver offers six settings:

- `voice`, a choice of every voice eSpeak NG lists, by identifier and
  display name. eSpeak NG reports identifiers with the platform's path
  separator (`gmw\en` on Windows); the driver stores them with `/`, so a
  saved setting means the same on every platform, and converts back when
  it selects the voice. The default is `gmw/en`, English (Great
  Britain), or the first voice listed if that one is missing.
- `variant`, a choice of `none` (displayed as None) and every variant
  eSpeak NG lists, by the variant file's name, such as `max`. The default
  is `max` (displayed as Max), NVDA's default variant, or `none` if that
  one is missing. A voice and variant are selected together as
  `voice+variant`.
- `rate`, `pitch`, `inflection`, and `volume`, standard 0 to 100
  numerics. Rate maps linearly onto eSpeak NG's 80 to 450 words per
  minute; pitch, inflection (eSpeak NG's pitch range), and volume are
  passed through unchanged. The defaults are 50, 50, 80, and 100.

A value outside a setting's range, an unknown voice or variant, or an
unknown setting is refused with `SynthError::Setting`. eSpeak NG resets
its parameters when the voice changes, so every successful change
reapplies all four numerics.

Tests: a unit test checks the rate mapping's two ends.
`tests/espeak.rs`, against the real library and its built data, checks
in one function that a second driver is refused, that `places_marks` is
false, that a sentence produces more than a second of 22050 Hz mono audio
in several chunks, that synthesis stops at the push that asks it to,
that the highest rate gives shorter audio, and that an unknown voice is
refused.
