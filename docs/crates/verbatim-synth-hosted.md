# verbatim-synth-hosted

`HostedSynth`: a synthesizer running in a synthesizer host process
(decision D18), seen by the speech manager as an ordinary `SynthDriver`.
Each `HostedSynth` owns one `verbatim-synth-host.exe`
([verbatim-synth-host](verbatim-synth-host.md)), launched contained
through [verbatim-process](verbatim-process.md), and talks to it with the
protocol in `verbatim_speech::hosting`
([verbatim-speech](verbatim-speech.md)). A synthesizer that crashes,
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

Implementation notes, lifecycle. `start` launches the host with
`--pipe-in <handle> --pipe-out <handle> --synth <id>`, no memory cap, a
4096-byte buffer on the pipe the host writes to, and the log stem
`synth-<id>`. A reader thread, named `verbatim-synth-host-<id>`, reads
`FromHost` messages from the pipe and sends each into a channel; an end
of stream or a read error is sent as an error and ends the thread. It
stamps a `FirstAudio` message with the shared clock (`hosting::now_us`)
as it reads the frame, and nothing else. The
host lives exactly as long as its `HostedSynth`: switching synthesizer
drops the driver, which drops the job handle and so ends the process.

Speaking. `speak` sends `Speak(sequence)` and then relays each message
to the speech manager's sink: `Pcm` to `push_pcm`, `Mark` to
`index_reached`, until `Done`, which returns success, or `Failed`, which
returns `SynthError::Synthesis`. Any other message while speaking is an
error.

Timing. `speak` stamps the shared clock as it is asked, and again just
before the `Speak` request's write begins. With the host's `FirstAudio`
stamps, the reader thread's stamp on reading that frame, and a stamp as
the synth thread takes it, it reports a `SynthTiming` to the sink: the
request, the trip to the host, the host's first chunk, the trip back to
the reader thread, and the reader's hand-off to the synth thread
(`docs/design/communication.md`, section 10.1).

Cancellation by utterance id. When `push_pcm` returns `Break`, or the
sink reports the utterance cancelled while the driver waits for the host
(the driver registers a waker with the sink, `SynthSink::on_cancel`, so
the cancel itself ends its wait, even before the host has sent any audio,
and it checks `is_cancelled` at each reply for a sink that keeps no
waker; nothing wakes at intervals to look), the driver sends `Cancel(utterance)` naming the utterance being spoken and keeps
reading until that utterance's `Done`, discarding any PCM and marks that
were already in flight. Waiting for `Done` keeps the pipe in step, so the
next `Speak` never reads the cancelled utterance's leftovers. The host
ignores a cancel for any utterance but the one it is speaking, so a
cancel that crosses the utterance's own `Done` on the pipe cannot cancel
the next one. A cancelled utterance returns success, as with any driver.

Backpressure. The mixer's `Source::write` blocks while the speech
manager's source is too far ahead of playback, so `push_pcm` blocks and
the driver stops taking messages from its channel. The channel is bounded
to two messages, so the reader thread blocks on its next send and stops
reading the pipe. The pipe's 4096-byte buffer (about 90 ms of 22 kHz mono
audio) then fills, and the host's next write blocks, which pauses the
host's synthesizer inside its own `push_pcm`. Pacing therefore crosses
the process boundary unchanged, and a cancel has little queued audio to
discard. The hang timeout applies only while the driver waits on the
channel, so time spent blocked in the sink is never counted against the
host.

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

A host that died between requests is found before the next one. A host
sends nothing while idle, so if a message is already waiting when
`speak` or `set_setting` begins (the reader thread queues the pipe's end
of stream when the process dies, and anything else would be out of
turn), or its process has exited (`Contained::has_exited`), the host is
ended and a new one started before the request is sent, rather than the
request being lost to the dead host.

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
`SettingApplied`. A refusal is returned as `SynthError::Setting` and
keeps the host; success also updates the cached value that `setting`
reads.

Tests: the crate's own unit tests compute each stage of the
`SynthTiming` split exactly from fixed stamps, with the request written
through a pipe that advances a fake clock, so a stamp taken after the
write fails them. It is otherwise tested against the real
host executable in `crates/verbatim-synth-host/tests/hosting.rs` (see
[verbatim-synth-host](verbatim-synth-host.md)), and end to end by the
`synth_host_crash_recovery` scenario ([verbatim-e2e](verbatim-e2e.md)),
which kills the host once the Verbatim menu's announcement has been
heard and expects the next
announcement to be heard in full.
