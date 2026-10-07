# verbatim-synth-host

`verbatim-synth-host.exe`: one synthesizer in its own process (decision
D18). Core starts one per hosted synthesizer through
[verbatim-synth-hosted](verbatim-synth-hosted.md), contained in a
kill-on-close job by [verbatim-process](verbatim-process.md), and the two
talk over a pair of inherited anonymous pipes with the protocol in
`verbatim_speech::hosting` ([verbatim-speech](verbatim-speech.md)). The
host renders nothing: it only synthesizes, and Core's mixer plays the
audio, so recording, device recovery, and mixing stay in one place
(decision D17). It must sit next to `verbatim.exe`, which looks for it
there.

Command line: `--pipe-in <handle> --pipe-out <handle> --synth <id>`, in
any order. Anything else prints a usage line and exits with code 2. It
knows two synthesizers, by the ids in
`verbatim_speech::hosting::synth_ids`: `espeak`, built with
`EspeakSynth::new` from [verbatim-synth-espeak](verbatim-synth-espeak.md),
and `onecore`, built with `OneCoreSynth::new` from
[verbatim-synth-onecore](verbatim-synth-onecore.md). Any other id, or a
synthesizer that cannot start (eSpeak NG without its data, or OneCore
with no voices installed, for example), is reported as unavailable.
eSpeak NG reads its data from `espeak-ng-data` next to the host
executable, so that directory must be deployed beside it.

The crate has no library API; it is a binary.

Implementation notes, code page. The executable embeds an application
manifest (`verbatim-synth-host.manifest`, through `build.rs`) that makes
UTF-8 the process's code page. eSpeak NG opens its data with the C
runtime's narrow file functions, which read paths in the process code
page, so without it an install folder whose name the ANSI code page
cannot represent (C:\Users\Zoë) kept eSpeak NG, the default synthesizer,
from starting. `tests/hosting.rs` runs the host from such a folder.

Implementation notes, lifecycle. The host installs a `tracing`
subscriber writing to standard error, takes its two pipes with
`verbatim_process::inherited_pipes`, and builds the driver. If that fails
it sends `Unavailable(reason)` and exits with a failure code. Otherwise
it sends `Ready` with a `HostDescription`: the driver's display name,
`places_marks`, `changes_pitch`, its setting descriptors, and the current
value of each.
It then serves requests one at a time on the main thread until Core
closes the command pipe, when it exits successfully. If a write to Core
fails, Core is gone and the host exits with a failure code.

Requests. A reader thread reads every message Core sends. A `Cancel`
takes effect at once on that thread (see below); `Speak` and
`SetSetting` go through an unbounded channel to the main thread, which
handles them in order. `Speak` calls the driver's blocking `speak` with a
sink that writes to Core, then sends `Done`, or `Failed(reason)` if the
driver returned an error. A cancelled utterance also ends with `Done`.
`SetSetting` calls the driver's `set_setting` and answers
`SettingApplied` with `None`, or the error's text when it was refused.

The sink. `push_pcm` writes one `Pcm` frame per call and
`index_reached` one `Mark` frame, so marks stay in order with the audio
around them. The write blocks while Core is behind, because Core asks for
a 4096-byte buffer on this pipe and reads it only as fast as its mixer
accepts audio. That blocking is the backpressure that paces the driver.

Cancellation by utterance id. The reader thread stores the id of the
utterance Core last cancelled in an atomic. Before each write of audio,
the sink compares it with the utterance being spoken and, on a match,
returns `Break` without writing, so the driver stops at its next push;
the sink's `is_cancelled` answers from the same atomic, so a driver that
has no audio yet (OneCore, which synthesizes a whole utterance first)
abandons its synthesis early.
Because the check is by id, a cancel that arrives after its utterance has
already finished, crossing that utterance's `Done` on the pipe, never
stops the utterance after it. Utterance ids start at 1, so the initial
value 0 matches none.

Logs. Core redirects the host's standard output and error to
`logs\<Verbatim's pid>\synth-<id>.log` next to the executables, for
example `synth-espeak.log` or `synth-onecore.log`, in append mode, so a host that crashed and
its replacement write to the same file. The default filter is `info`;
`RUST_LOG` overrides it. The end-to-end harness collects this file with
the outpost and listener logs.

Tests: `tests/hosting.rs` drives the real host through `HostedSynth`
with OneCore, which every Windows 11 machine and GitHub's Windows runners
have, and with eSpeak NG. A new host's first utterance is reproducible on
one machine, so the tests compare speech sample for sample. It covers
audio and marks arriving in order (exactly one mark, between the two
words' audio); eSpeak NG in its host placing a mark itself, relayed
after exactly the 16,829 samples before it, in speech that is a new
host's unmarked sentence sample for sample; a raised capital spoken at the raised pitch (differing
from the plain letter, and shorter than its markup read aloud); a host
killed mid-utterance failing that utterance, with the next utterance
getting a new host process that speaks exactly as a host set to the same
rate does; a cancelled utterance ending within 20 ms of the cancel,
without anything relayed after it, and the same host process speaking
the next utterance; an utterance cancelled before any audio relaying
nothing and keeping the host; eSpeak NG speaking from a folder whose
name the ANSI code page cannot represent; a host that died while
idle, and has exited, being replaced before the next utterance; and an
utterance sent to a host that dies before relaying any of it being sent
once more, to a new host, and spoken exactly as a new host's first
utterance (the host's threads are suspended, so the request reaches a
live host that answers nothing, and the host is ended while the driver
waits for its first answer, from the sink's cancellation check).
