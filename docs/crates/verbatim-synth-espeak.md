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
later driver in the same process can start eSpeak NG again. That later
driver does not start from a clean state: some of eSpeak NG's globals
survive `espeak_Terminate`, as they run on from one utterance to the
next.
Before starting eSpeak NG, `new` checks that its data directory holds
`phontab`: given a path without data, eSpeak NG would otherwise fall back
to an environment variable or another installation's registry entry and
load data that is not its own. A voice or variant eSpeak NG refuses is
reported and not stored. The synthesizer host builds exactly one
driver, so this costs Verbatim nothing.

Determinism. eSpeak NG seeds the generator for its noise from the clock
when it starts. The driver seeds it with a fixed value right after, so a
new process's speech depends only on what it is asked to say: the same
utterances with the same settings give the same samples.

Synthesis. eSpeak NG runs in synchronous mode: `espeak_Synth` blocks and
hands audio to a callback in chunks of 60 ms (1,324 samples, the
shortest eSpeak NG allows) as it synthesizes,
many times faster than real time. The callback pushes each chunk to the
sink, and returns non-zero to abort synthesis when the sink's `push_pcm`
returns `Break`, so a cancel takes effect within one chunk. A sink that
already reports the utterance cancelled produces nothing, and a sequence
whose text is blank produces no audio, only its marks, in order. The audio is 16-bit mono at the sample rate
eSpeak NG reports when it starts, 22050 Hz. A cancelled utterance
returns success, as with any driver; an error code from `espeak_Synth`
is `SynthError::Synthesis`.

Pitch changes. `changes_pitch` is `true`: a sequence with a pitch change
is given to eSpeak NG as SSML (`espeakSSML`), its text escaped and each
change a `prosody` element whose pitch is the new value as a percentage
of the configured one, as NVDA's driver writes it (50 raised by 30 is
160%), so a capital is raised within one synthesis. A sequence with
neither pitch changes nor marks goes as plain text, its text items
concatenated.

Index marks. `places_marks` is `true`, so the speech manager gives the
driver whole sequences, marks included, and a sentence with a mark inside
it, such as say-all's sentence running from one line to the next, is
spoken in one synthesis, as NVDA's eSpeak NG driver speaks it, with no
pause and no change of intonation at the mark. Each mark is a `mark`
element in the SSML, named by its number. eSpeak NG reports each one as
a mark event carrying the sample it falls at, counted from the start of
the synthesis; the callback pushes the chunk's audio up to that sample,
reports the mark, and pushes the rest, so every mark is exact (decision
D17). A mark eSpeak NG places beyond the chunk it came with is logged and
placed at the chunk's end. A synthesis that runs to its end without
reporting every mark of its piece, in order, is
`SynthError::Synthesis`, so a lost mark fails the utterance rather than
leaving say-all waiting.

One case is divided. Within one synthesis, eSpeak NG drops a mark that
follows a full stop and whitespace when an upper-case letter or anything
but a letter follows it: it waits past the tag to decide whether the
full stop ends the sentence, then ends the clause at the full stop and
discards what the tag wrote after it (true up to at least eSpeak NG
master in October 2026). So a new synthesis starts at each mark that
follows a full stop and whitespace, the pitch change in force repeated at
its start, and every synthesis but the last is asked for the pause after
its last clause (`espeakENDPAUSE`), which it would have had within one
synthesis. Where eSpeak NG ends the sentence there anyway, the division
costs nothing that is heard: the two sentences with the mark between
them are the unmarked two sentences, sample for sample. Where a
lower-case letter follows, as after "e.g. ", eSpeak NG would have run on
within one synthesis, and the division ends a sentence there instead;
say-all never asks for that, since it ends its calls at such a full stop.

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
`tests/espeak.rs`, against the real library and its built data, has its
own small runner (`harness = false`). Tests that compare speech exactly
have each utterance spoken by a process of their own, the test binary run
again with `--speak` and the name of a case, since only a new process's
speech is reproducible. It checks that a second driver is refused with
its message, that `places_marks` is true, that an unknown voice is
refused and the voice kept, that a sentence comes as 22050 Hz mono audio
in full 1,324-sample chunks but the last, that the same utterance gives
the same samples in two processes, that synthesis stopped at the first
push gives exactly the sentence's first chunk, that the highest rate
gives shorter audio and setting the rate back gives the same samples as
never changing it, and that a raised capital's markup changes the speech
but is not read aloud (a B spoken through SSML with no change of pitch
is the plain B, sample for sample). For marks, it checks that a sentence
with marks before it, where its second half starts, and after it is the
unmarked sentence, sample for sample (so it was one synthesis), with the
marks at samples 0, 16,829, and 37,945; and that a mark after a full stop
and a space is reported at sample 15,259, where the next sentence starts
after the sentence pause, in speech that is the unmarked two sentences',
sample for sample. Unit tests check the SSML (escaped text, marks named
by number, a raised pitch as a percentage) and where a sequence is
divided into syntheses.
