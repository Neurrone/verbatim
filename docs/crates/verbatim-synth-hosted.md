# verbatim-synth-hosted

`HostedSynth`: a synthesizer running in a synthesizer host process
(decision D18), seen by the speech manager as an ordinary `SynthDriver`.
Each `HostedSynth` owns one `verbatim-synth-host.exe`
([verbatim-synth-host](verbatim-synth-host.md)), launched contained
through [verbatim-process](verbatim-process.md), and talks to it with the
protocol in `verbatim_speech::hosting`
([verbatim-speech](verbatim-speech.md)), each message a frame of
`verbatim_process::transport`. A synthesizer that crashes,
hangs, or leaks then takes down only its host, and the next utterance
starts a new one.

Public API:

- `HostedSynth::start(exe, synth)` — launches a host from the executable
  at `exe` for the synthesizer named by `synth` and waits for it to
  describe the synthesizer. Returns `SynthError::Unavailable` when the
  host cannot be launched, sends `Unavailable`, begins with any other
  message, or sends nothing within `HANG_TIMEOUT`.
- `HostedSynth::process_id()` — the running host's process id, or `None`
  after a host ended and before the next request starts another.
- The `SynthDriver` implementation. `id`, `display_name`,
  `supported_settings`, `setting`, `places_marks`, and `changes_pitch`
  answer from the
  description the first host sent, with no round trip; `set_setting` and
  `speak` go to the host.
- `factory(exe, synth)` — a `SynthFactory` that calls `start`, which is
  how `verbatim-app` registers eSpeak NG and OneCore.
- `HANG_TIMEOUT` — ten seconds: how long a host may send nothing while
  Core waits on it, starting or speaking or applying a setting, before
  it is judged hung. A synthesizer produces audio many times faster than
  real time, so ten seconds of silence is never normal.

Implementation notes, lifecycle. `start` makes two overlapped pipes with
`overlapped::pipe`, the one the host writes to with a 4096-byte buffer,
and launches the host on them with `launch_with_ends`, with
`--pipe-in <handle> --pipe-out <handle> --synth <id>`, no memory cap,
and the log stem `synth-<id>`. The driver's ends are a
`transport::EventWriter` for commands and a `transport::EventReader` for
the host's messages; there is no reader thread
(`docs/design/communication.md`, section 7). The host lives exactly as
long as its `HostedSynth`: switching synthesizer drops the driver, which
drops the job handle and so ends the process.

Reading. The synth thread reads the host's pipe itself, and only when it
wants the next message and none is already read: a read asks for 4096
bytes while a frame's length is unknown, and for the whole rest of a
frame once it is known. Every message read that names a draining
utterance (below) is dropped, its `Done` or `Failed` ending the
draining. While it waits, the thread waits on the read's event, and
while speaking on the utterance's cancel too, at most `HANG_TIMEOUT` at a
time. A `FirstAudio` frame is stamped with the shared clock
(`hosting::now_us`) as it is read.

Speaking. `speak` writes `Speak(sequence)`, whole, and then relays each
message of that utterance to the speech manager's sink: `Pcm` to
`push_pcm`, `Mark` to `index_reached`, until `Done`, which returns
success, or `Failed`, which returns `SynthError::Synthesis`. A message of
any other utterance, or any other message, while speaking is out of
turn.

Timing. `speak` stamps the shared clock as it is asked, and again just
before the `Speak` request's write begins. With the host's `FirstAudio`
stamps, the stamp on reading that frame, and a stamp as it is handled, it
reports a `SynthTiming` to the sink: the request, the trip to the host,
the host's first chunk, the trip back, and the time from reading the
frame to handling it (`docs/design/communication.md`, section 10.1).

Cancellation (sections 7.2 and 7.3). Once the request is written, the
driver registers a waker with the sink (`SynthSink::on_cancel`) that sets
an event of this utterance's own, which its wait watches with the pipe,
so the cancel itself ends the wait, even before the host has sent any
audio; it also checks `is_cancelled` at each message, for a sink that
keeps no waker. Nothing wakes at intervals to look. When the sink reports
the utterance cancelled, or `push_pcm` returns `Break`, the driver writes
`Cancel(utterance)`, notes the utterance as draining, and returns at once,
without waiting for the host to end it. The next request, a `Speak` or a
`SetSetting`, is then written right behind the `Cancel`, and its own
reads drop the cancelled utterance's last frames and its `Done` as they
come. The host ignores a cancel for an utterance already ended, so a
cancel that crosses the utterance's own `Done` on the pipe cannot cancel
the next one. A cancelled utterance returns success, as with any driver.

Backpressure. The mixer's `Source::write` blocks while the speech
manager's source is too far ahead of playback, so `push_pcm` blocks, and
the driver reads the next frame only once it returns. The pipe's
4096-byte buffer (about 90 ms of 22 kHz mono audio), with at most one
read of the same size, then fills, and the host's next write pends, which
pauses the host's synthesizer inside its own `push_pcm`. Pacing therefore
crosses the process boundary unchanged, and a cancel has little queued
audio to drop. The hang timeout applies only while the driver waits for
the host, so time spent blocked in the sink is never counted against it.

Recovery. A host that is no longer usable is ended: a closed pipe, a
host that sent nothing for `HANG_TIMEOUT`, or a message out of turn. The
host is dropped, which kills it through its job, and the error is
returned, so the speech manager fails that utterance, unless `speak`
resends it (see below). A `Failed` reply
is different: the host reported a synthesis error in turn, the stream is
still in step, so only that utterance fails and the host carries on. The
next `speak` or `set_setting` after a host was ended starts a new host
and sends it, one `SetSetting` at a time, every setting value the old
host had. A value the new host refuses (a voice since uninstalled) is
logged and skipped; any other failure while restoring ends the new host
too, so none of its replies can reach a later request. Those values are
the ones the first host described, plus each later successful
`set_setting`. The new host's own description is not used. Restarts log a warning under the
`verbatim::speech` target.

A host that died between requests is found before the next one when it
can be. Between requests a host sends only the last frames of draining
utterances, so if anything else has already been read from it, or its
pipe's end has, or its process has exited (`Contained::has_exited`), the
host is ended and a new one started before the request is sent. Nothing
more is read between requests: the draining frames still in the pipe are
read by the next request's own reads.

Ending a process is not immediate, so a request can still reach a host
that is dying. `speak` therefore sends an utterance once more, to a fresh
host, when the host's pipe ended before any of its audio or marks were
relayed to the sink: nothing of it was heard, so nothing is repeated. It
is not resent when anything was relayed (an utterance cut off part-way
fails), when the host hung or answered out of turn (a second host would
likely do the same, and a hang has already cost ten seconds), when the
sink reports the utterance cancelled, or a second time.
`set_setting` is never resent.

Settings. `set_setting` sends `SetSetting` and waits for
`SettingApplied`, dropping a cancelled utterance's frames on the way. A refusal is returned as `SynthError::Setting` and
keeps the host; success also updates the cached value that `setting`
reads.

Tests: the crate's own unit tests compute each stage of the
`SynthTiming` split exactly from fixed stamps, with the request's write
advancing a fake clock, so a stamp taken after the write fails them. It is otherwise tested against the real
host executable in `crates/verbatim-synth-host/tests/hosting.rs`, and
against a scripted host in `crates/verbatim-synth-host/tests/protocol.rs`
(see [verbatim-synth-host](verbatim-synth-host.md)), and end to end by the
`synth_host_crash_recovery` scenario ([verbatim-e2e](verbatim-e2e.md)),
which kills the host once the Verbatim menu's announcement has been
heard and expects the next
announcement to be heard in full.
