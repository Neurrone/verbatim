# verbatim-synth-host

`verbatim-synth-host.exe`: one synthesizer in its own process (decision
D18). Core starts one per hosted synthesizer through
[verbatim-synth-hosted](verbatim-synth-hosted.md), contained in a
kill-on-close job by [verbatim-process](verbatim-process.md), and the two
talk over a pair of inherited overlapped pipes, each message a frame of
`verbatim_process::transport`, with the protocol in
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
[verbatim-synth-onecore](verbatim-synth-onecore.md) and given the host's
command event with `interrupted_by`. Any other id, or a
synthesizer that cannot start (eSpeak NG without its data, or OneCore
with no voices installed, for example), is reported as unavailable.
eSpeak NG reads its data from `espeak-ng-data` next to the host
executable, so that directory must be deployed beside it.

Public API, a small library the binary is built on, so a test can serve
Core with a scripted driver through the same code:

- `serve(commands, replies, build)` — serves Core on the calling thread
  until Core closes the command pipe, returning the process's exit code.
  `commands` and `replies` are the host's overlapped pipe ends; `build`
  makes the driver and is given the command event, the
  `verbatim_process::event::Event` the command pipe's read sets, for a
  driver that waits for its synthesizer to wait on as well.
- `parse_args(args)` — the pipe handle values and the synthesizer id from
  the command line, or `None` for anything else.

Implementation notes, code page. The executable embeds an application
manifest (`verbatim-synth-host.manifest`, through `build.rs`) that makes
UTF-8 the process's code page. eSpeak NG opens its data with the C
runtime's narrow file functions, which read paths in the process code
page, so without it an install folder whose name the ANSI code page
cannot represent (C:\Users\Zoë) kept eSpeak NG, the default synthesizer,
from starting. `tests/hosting.rs` runs the host from such a folder.

Implementation notes, lifecycle. The host installs a `tracing`
subscriber writing to standard error, takes its two pipes with
`verbatim_process::inherited_pipe_handles`, and calls `serve`, which
builds the driver. If that fails it sends `Unavailable(reason)` and exits
with a failure code. Otherwise it sends `Ready` with a `HostDescription`:
the driver's display name, `places_marks`, `changes_pitch`, its setting
descriptors, and the current value of each. It then serves commands until
Core closes the command pipe, when it exits successfully. If a write to
Core fails, Core is gone and the host exits with a failure code.

One thread (`docs/design/communication.md`, sections 6.5 and 7.4). The
host has only its main thread, which waits only on events. The command
pipe is read through a `transport::EventReader`, whose read is in flight
whenever nothing else is being read; its event wakes the thread when a
command arrives, and the thread then reads every whole command waiting.
Commands wait their turn in a queue, in order: `Speak` calls the driver's
blocking `speak` with a sink that writes to Core, then sends `Done`, or
`Failed(reason)` if the driver returned an error; `SetSetting` calls the
driver's `set_setting` and answers `SettingApplied` with `None`, or the
error's text. Every message about an utterance names it.

Cancellation, as each command is read. A `Cancel` of the utterance being
spoken marks it cancelled at once and calls the waker its driver
registered (`SynthSink::on_cancel`); one of an utterance still waiting
its turn turns it into a skip, answered with `Done` when its turn comes,
unspoken; one of an utterance already ended does nothing, so a cancel
that crosses its utterance's `Done` on the pipe never stops the next.
While the driver synthesizes, commands are read without another thread:
the sink reads whatever has come each time the driver pushes audio or
asks `is_cancelled`, a read that returns at once when nothing has (eSpeak
NG asks once per 60 ms chunk), and `OneCore`, which has no audio until
the whole utterance is synthesized, waits on its operation's completion
and the command event together, asking the sink when the event is set.
A cancelled utterance stops at its driver's next push, and also ends with
`Done`.

The sink. `push_pcm` writes one `Pcm` frame per call, after `FirstAudio`
in the same write for an utterance's first audio, and `index_reached`
one `Mark` frame, so marks stay in order with the audio around them.
Writes go through a `transport::EventWriter`. A write the pipe cannot
take at once, because Core asks for a 4096-byte buffer on this pipe and
reads a frame only once its sink has taken the one before, is a paced
send: the thread waits on the write's completion and the command event
together, reading every command that comes meanwhile. That wait is the
backpressure that paces the driver. The write is never cancelled,
whatever comes: a frame larger than the pipe's buffer is finished whole,
and only then does a cancel read meanwhile stop the utterance, so the
pipe never holds part of a frame (section 4.3).

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
host's unmarked sentence sample for sample; a raised capital spoken at
the raised pitch (differing from the plain letter, and shorter than its
markup read aloud); a host killed mid-utterance failing that utterance,
with the next utterance getting a new host process that speaks exactly
as a host set to the same rate does; a cancelled utterance ending within
20 ms of the cancel, without anything relayed after it, and the same
host process speaking the next utterance; an utterance cancelled before
any audio relaying nothing and keeping the host; eSpeak NG speaking from
a folder whose name the ANSI code page cannot represent; a host that died
while idle, and has exited, being replaced before the next utterance;
and an utterance sent to a host that dies before relaying any of it
being sent once more, to a new host, and spoken exactly as a new host's
first utterance (the host's threads are suspended, so the request
reaches a live host that answers nothing, and the host is ended while
the driver waits for its first answer, from the sink's cancellation
check).

`tests/protocol.rs` (its own `main`; it runs itself as the host through
`serve` with a scripted driver that waits on a named semaphore, the
test's gate, before each chunk) covers the protocol of section 7: a
cancel during an utterance followed by a new one sends the new one's
`Speak` before the old one's `Done` is read (the old one's next chunk
waits at the gate, which opens only once the new request is written),
and none of the old one's audio is relayed after the cancel; a cancel
before any audio stops a driver waiting through `OneCore`'s own wait for
a synthesis that never completes, by the command event, with no timer;
a host cancelled while a frame far larger than its pipe's buffer is
pended delivers the next utterance whole, with no restart; and a setting
applied right after such a cancel is answered without the cancelled
utterance's frames being taken for its reply.
