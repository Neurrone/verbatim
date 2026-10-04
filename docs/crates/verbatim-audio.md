# verbatim-audio

Audio output (architecture section 6, decisions D5, D16 and D17): the
mixer that everything audible goes through, the `AudioDevice` seam it
writes to, `SilentDevice`, and `WavRecorder`, which records what the mixer
plays. The WASAPI device is
[verbatim-audio-wasapi](verbatim-audio-wasapi.md); this crate has no
Windows dependency.

Public API:

- `PcmFormat` — what a synthesizer produces: sample rate and channel
  count of interleaved signed 16-bit samples.
- `DeviceFormat` — what a device renders: sample rate and channel count
  of interleaved 32-bit float samples, plus `buffer_frames`, how many
  frames the device can hold queued.
- `AudioError` — `Device` (missing, failed, or must be reopened) and
  `Stream` (converting or writing failed).
- `AudioDevice` — the device seam, driven only by the mixer's audio
  thread: `open` (or reopen) returning the `DeviceFormat`,
  `queued_frames` (written but not yet played), `write`, `start`, `stop`
  (which discards everything queued), `wait` (until frames play, the
  waker is called, or a timeout passes), `waker` (a `Waker` handle that
  ends a `wait` early from any thread), and `needs_reopen` (for example
  after the system's default device changed; defaults to `false`).
- `Mixer` — `start(device)` spawns the `verbatim-audio` thread and opens
  the device on it, returning the open error if there is one;
  `start_with_tap(device, tap)` does the same and also gives `tap` every
  frame once it has played;
  `add_source(listener)` adds a `Source`; `format` reports the current
  device format; `underruns` counts the times the device ran dry in the
  middle of an utterance. Dropping the mixer stops the thread and ends
  every utterance still registered as cancelled.
- `Source` — one producer of audio, a stream of utterances played one
  after another. `register(utterance, trace_id)` announces the utterance
  written next; `write(utterance, format, samples)` appends PCM, blocking
  while the source is too far ahead of playback, and returns `Break` once
  the utterance no longer accepts audio; `mark(utterance, mark)` places an
  index mark at the current end of the utterance's audio; `finish` ends
  the utterance's audio; `fail(utterance, reason)` ends it as failed;
  `cancel_all` ends every utterance of the source not yet ended as
  cancelled; `pause(paused)` holds the source's audio where it is, or lets
  it go on, with its playback events waiting with the audio. `fail` and
  `cancel_all` return only after the audio thread has carried them out. Clones are handles to the same source.
- `PlaybackEvent` — `Started` (the utterance's first frame has played),
  `Mark` (playback reached an index mark), and `Ended` with an
  `UtteranceEnding`. Each carries the utterance id and trace id.
- `PlaybackListener` — the callback a source's events go to. It runs on
  the audio thread, so it must be quick and must not call back into the
  mixer.
- `AudioTap` — receives the mixer's output as it plays: `played(samples,
  format)` is called on the audio thread with the interleaved frames that
  have just played, each call following the frames of the previous one,
  so it must be quick.
- `SilentDevice` — a device that plays at real-time speed into silence.
- `WavRecorder` — an `AudioTap` that writes what played to a WAV file in
  step with real time. `create(path)` creates the file and, beside it,
  `<path>.start`, holding the recorder's creation time as Unix time in
  milliseconds, and starts the recording's clock.

Implementation notes, the audio thread: each pass reads how many frames
the device still has queued, works out how many have played, reports
every event whose moment has played, and mixes as many frames as the
device has room for and some source has. Mixing sums every source's
frames and clamps the result to the float range. When nothing is left to
mix and the device has drained, it stops the device. It then waits on the
device: at most 100 ms while playing and 1 s while idle.

Positions. Each source numbers its frames from zero as they are written
(source positions), and the mixer numbers the frames it writes to the
device (mix positions). Each time the mixer takes frames from a source it
records a segment mapping those source positions to mix positions. The
number of frames played is the number written minus the number the device
still has queued, and a source position has been heard once its mix
position has been played. A start event fires when the utterance's first
frame has played; a mark or end event fires when every frame before its
position has played. So an utterance that produced no audio completes
when the audio before it has played.

Retention. A source keeps its frames until they have played, not merely
until they were written to the device. When the device's queue must be
discarded (a cancel, fail, or pause request, a device error, or a request
to reopen), the mixer stops the device, rewinds every source to what had
actually played, and mixes from there again, so the other sources lose
nothing they had queued. Frames are released once played.

Backpressure. A source may hold unplayed frames up to the device buffer
plus 40 ms; a `write` beyond that blocks until playback makes room. This
keeps a fast synthesizer from running ahead of what is heard, which keeps
cancellation cheap.

Endings. Every registered utterance ends exactly once with
`PlaybackEvent::Ended`: completed when its last frame has played,
cancelled by `cancel_all` or by the mixer being dropped, or failed by
`fail` or by the device reopening in a different sample rate or channel
count (frames already converted for the old format cannot be played).
Cancel and fail discard the affected audio at once, and the mixer then
refuses further writes for those utterances. Utterances registered after
`cancel_all` returns are unaffected.

Pausing. A paused source contributes no frames to the mix, and because a
pause request, like any request, takes back what the device had queued and
rewinds to what had played, the pause is heard at once rather than after
the device's buffer drains; resuming mixes from the same place. The
device running dry while a source is paused, or before a resumed source
is mixed from again, is not counted as an underrun.

Device recovery. On any device error, or when `needs_reopen` is set, the
mixer rewinds as above and calls `open` again, with the shared state
unlocked, retrying once a second until it succeeds or the mixer shuts
down.

Starting and gaps. A stopped device is started only once 10 ms of audio
is ready, or once the utterance is completely written: an utterance
whose first write is a few frames (what is left after its leading
silence is trimmed) would otherwise start the device, play them, and
run it dry before the next frames arrive, which is heard as a click. An
underrun is counted, and logged as a warning, when the device ran dry
part-way through an utterance and more of that utterance then played: a
gap the listener hears. The device running dry while an utterance's
trimmed trailing silence is held back is not one. Measured live on
2026-10-04 with OneCore and the 40 ms WASAPI buffer, two full audible
suite runs had no underruns.

The tap. The mixer keeps a copy of every frame it writes to the device
and gives the tap the frames that have played since the last pass, so
the tap hears exactly what the listener heard, in order. Frames written
to the device and then discarded (a cancel, a fail, a device error or a
reopen) are dropped from that copy and never reach the tap.

`WavRecorder` (`wav.rs`) writes the tapped frames as 16-bit PCM in the
format of the first audio it receives. Between utterances the mixer
plays nothing, so the recorder fills the gaps with silence from the
clock: a block that has just finished playing is placed so that it ends
at the moment it was received, with 20 ms of slack so ordinary
scheduling jitter does not insert clicks of silence. The file therefore
runs in step with real time from the moment it was created, and the
`.start` file lets another program's screen capture be lined up with it.
The header is rewritten after every write, so the file stays valid when
Verbatim is ended without warning, as the end-to-end harness ends it. If
the device reopens in a different format, or a write fails, the recorder
logs a warning and stops recording; playback is unaffected. `verbatim-app`
creates one when `VERBATIM_RECORD_AUDIO` names a file.

The converter (`convert.rs`): each source has a converter from its
`PcmFormat` to the device format, rebuilt when either changes. Samples
are scaled from 16-bit integers to float, and channels are mapped: mono
is copied to every output channel, any layout is averaged for a mono
device, and otherwise output channel n takes input channel n modulo the
input count. When the rates differ, a band-limited sinc resampler (the `rubato` crate)
works in 64-frame input chunks with a 64-tap filter, so it adds only a
few milliseconds before an utterance's first sample reaches the mixer;
its start-up delay is discarded so the first frames carry audio. Each
utterance is converted on its own: finishing an utterance flushes the
resampler's tail and resets it, so one utterance's audio never bleeds
into the next. A mark is placed after the frames converted so far, so it
can be heard up to a few milliseconds early.

`SilentDevice`: a 48 kHz stereo device with a 40 ms queue, the usual
shared-mode format, whose frames leave the queue at the rate a real
device would play them. An utterance played through it still takes its
real duration, and its ending still means it would have been heard by
then. It is used when a machine has no audio device (GitHub's hosted
runners have none), by `verbatim-audio-wasapi` as its fallback, and by
`verbatim-app` under `VERBATIM_TEST_AUDIO=null`.

Tests: `tests/mixer.rs` drives the mixer against a scripted device and
covers completion only after the last frame plays, marks, utterances
without audio, cancellation sparing later utterances, failure, write
backpressure, replaying unplayed audio after a reopen, and the tap getting
exactly what played and never what was cut off. A unit test in `wav.rs`
checks that the recorder writes a valid WAV with silence where nothing
played.
