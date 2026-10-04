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
any order. Anything else prints a usage line and exits with code 2. The
only synthesizer it knows today is `onecore`, built with
`OneCoreSynth::new` from
[verbatim-synth-onecore](verbatim-synth-onecore.md); any other id, or a
OneCore that cannot start (no voices installed, for example), is
reported as unavailable.

The crate has no library API; it is a binary.

Implementation notes, lifecycle. The host installs a `tracing`
subscriber writing to standard error, takes its two pipes with
`verbatim_process::inherited_pipes`, and builds the driver. If that fails
it sends `Unavailable(reason)` and exits with a failure code. Otherwise
it sends `Ready` with a `HostDescription`: the driver's display name,
`places_marks`, its setting descriptors, and the current value of each.
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
returns `Break` without writing, so the driver stops at its next push.
Because the check is by id, a cancel that arrives after its utterance has
already finished, crossing that utterance's `Done` on the pipe, never
stops the utterance after it. Utterance ids start at 1, so the initial
value 0 matches none.

Logs. Core redirects the host's standard output and error to
`logs\<Verbatim's pid>\synth-<id>.log` next to the executables, for
example `synth-onecore.log`, in append mode, so a host that crashed and
its replacement write to the same file. The default filter is `info`;
`RUST_LOG` overrides it. The end-to-end harness collects this file with
the outpost and listener logs.

Tests: `tests/hosting.rs` drives the real host through `HostedSynth`
with OneCore, which every Windows 11 machine and GitHub's Windows runners
have. It covers audio and marks arriving in order (a mark between two
words falls between their audio); a host killed mid-utterance failing
that utterance, with the next utterance getting a new host process and
the same rate setting; and a cancelled utterance ending without anything
relayed after the cancel, with the same `HostedSynth` speaking the next
utterance.
