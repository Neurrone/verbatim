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
  `supported_settings`, `setting`, and `places_marks` answer from the
  description the first host sent, with no round trip; `set_setting` and
  `speak` go to the host.
- `factory(exe, synth)` — a `SynthFactory` that calls `start`, which is
  how `verbatim-app` registers OneCore.
- `HANG_TIMEOUT` — ten seconds: how long a host may send nothing while
  Core waits on it, starting or speaking or applying a setting, before
  it is judged hung. A synthesizer produces audio many times faster than
  real time, so ten seconds of silence is never normal.

Implementation notes, lifecycle. `start` launches the host with
`--pipe-in <handle> --pipe-out <handle> --synth <id>`, no memory cap, a
4096-byte buffer on the pipe the host writes to, and the log stem
`synth-<id>`. A reader thread, named `verbatim-synth-host-<id>`, reads
`FromHost` messages from the pipe and sends each into a channel; an end
of stream or a read error is sent as an error and ends the thread. The
host lives exactly as long as its `HostedSynth`: switching synthesizer
drops the driver, which drops the job handle and so ends the process.

Speaking. `speak` sends `Speak(sequence)` and then relays each message
to the speech manager's sink: `Pcm` to `push_pcm`, `Mark` to
`index_reached`, until `Done`, which returns success, or `Failed`, which
returns `SynthError::Synthesis`. Any other message while speaking is an
error.

Cancellation by utterance id. When `push_pcm` returns `Break`, the driver
sends `Cancel(utterance)` naming the utterance being spoken and keeps
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
returned, so the speech manager fails that utterance. A `Failed` reply
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

Settings. `set_setting` sends `SetSetting` and waits for
`SettingApplied`. A refusal is returned as `SynthError::Setting` and
keeps the host; success also updates the cached value that `setting`
reads.

Tests: the crate has no tests of its own; it is tested against the real
host executable in `crates/verbatim-synth-host/tests/hosting.rs` (see
[verbatim-synth-host](verbatim-synth-host.md)).
