# verbatim-audio

Audio output (architecture section 6, decisions D5, D16 and D17): the
mixer that everything audible goes through, speech and sounds alike, the
`AudioDevice` seam it writes to, `DeviceNotices`, `Sound`,
`SilentDevice`, and `WavRecorder`, which records what the mixer plays.
The WASAPI device is [verbatim-audio-wasapi](verbatim-audio-wasapi.md);
this crate has no Windows dependency.

Public API:

- `PcmFormat` — what a synthesizer produces: sample rate and channel
  count of interleaved signed 16-bit samples.
- `DeviceFormat` — what a device renders: sample rate and channel count
  of interleaved 32-bit float samples, plus `buffer_frames`, how many
  frames the device can hold queued.
- `AudioError` — `Device` (missing, failed, or must be reopened) and
  `Stream` (converting or writing failed).
- `AudioDevice` — the device seam, driven only by the mixer's audio
  thread: `open` (or reopen) returning the `DeviceFormat`, or an error
  after which the mixer plays silence until `needs_reopen` asks,
  `queued_frames` (written but not yet played), `write`, `start`, `stop`
  (which discards everything queued), `wait` (until frames play, the
  waker is called, or a timeout passes), `waker` (a `Waker` handle that
  ends a `wait` early from any thread: the audio thread's wake event),
  `signal` (a new `Signal` of the device's kind, for the mixer's other
  threads to wait on; a `CondvarSignal` by default), `needs_reopen` (for
  example after the system's default device changed, or a device became
  available after an open failed; defaults to `false`), and
  `clock_position` (the device's own clock as a `ClockReading`: frames
  played since it last started, and how long ago that held; defaults to
  `None`, and only the latency ledger's device clock stage uses it).
- `Signal` — an auto-reset event, `set` from any thread and `wait`ed on
  by one; a set made while nobody waits stays until the next wait, so a
  wake is never lost. `CondvarSignal` is the portable one, over a mutex
  and a condition variable, for the silent device and test devices; the
  WASAPI device makes Windows events.
- `DeviceNotice` and `DeviceNotices` — the system's notifications about
  its output devices (`DefaultChanged`, `Available`: a device arrived or
  became usable) turned into requests to reopen. A device's notification
  thread calls `notify`; its audio thread calls `opening` before an
  open and `opened` or `failed(retry_after)` after it, and reports
  `needs_reopen` and shortens its waits with `wait_limit`. An open device
  asks to be reopened on `DefaultChanged`; after a failed open, also on
  `Available`, or once `retry_after` has passed for a failure the system
  sends no notification for. A notification that comes during an open is
  kept, so it is answered by another open. `notify` calls the waker only
  when it asks for a reopen.
- `MixerBuilder` — everything the audio thread takes in is made before
  it starts. `new(device)`; `source(listener)` adds a source, a stream of
  utterances played one after another, returning its `SourceKey`;
  `control(source)` makes the source's `SourceControl`; `speech_lane(source)`
  makes the one `SpeechLane` that writes utterances to it; `lane(source)`
  makes a `Lane` of sounds to it for one more thread; `tap(tap)` gives
  `tap` every frame once it has played; `on_delivery_thread(start)` runs
  `start` on the delivery thread first, which the application uses to
  raise its priority; `start()` spawns the `verbatim-audio` thread, which
  opens the device, and the `verbatim-audio-events` delivery thread,
  failing only when a thread cannot start.
- `Mixer` — the running mixer: `format` reports the current device format
  and `underruns` counts the times the device ran dry in the middle of an
  utterance, both read from what the audio thread publishes. Dropping it
  stops the audio thread, which ends every utterance still registered as
  cancelled, then the delivery thread once it has delivered every report.
- `SourceControl` — one source's cancel generation and pause, for the
  one thread that cancels and pauses it, the speech queue thread.
  `generation` is the generation to register the next utterance under;
  `cancel` raises it, which ends every utterance registered under an
  older one as cancelled, discarding what of it has not played, and stops
  every voice of the source; `pause(paused)` holds the source's audio
  where it is, or lets it go on, with its playback events waiting with
  the audio. Neither waits for the audio thread: each is one atomic store
  and a wake.
- `SpeechLane` — the synth thread's lane to its source. `register(utterance,
  trace_id, generation)` announces the utterance written next, under the
  generation it was handed on under; `write(utterance, format, samples)`
  converts PCM to the device format on the calling thread and pushes it,
  waiting while the source is too far ahead of playback, and returns
  `Break` once the utterance no longer accepts audio (the generation was
  raised since it was registered, the device changed format, or the
  mixer stopped); `mark(utterance, mark)` places an index mark at the
  current end of the utterance's audio; `sound(utterance, sound, gain)`
  places a `Sound` there, as `mark` places a mark, to start when playback
  reaches it and play on over the audio that follows; `finish` ends the
  utterance's audio; `fail(utterance, reason)` ends it as failed. Until
  the utterance is finished, a sound is mixed only alongside the
  utterance's own audio: playback that reaches it, or the end of the
  audio written after it, waits for more of that audio, so what follows
  the sound's place is heard under it at its place however synthesis and
  playback interleave (a sound between two words starts with the
  second), and only once the utterance is finished does the sound play
  on past its audio. A push that finds the lane full waits on the
  source's room event.
- `Lane` — one more thread's lane of sounds: `play(sound, gain)` plays one
  at once, and `play_to_end(utterance, trace_id, sound, gain)` plays one
  as an utterance of its own that ends when the sound has played to its
  end, for the exit sound. Neither waits: a sound that finds the lane
  full, which only a stalled audio thread leaves it, is refused with an
  error, and the caller logs it; a refused `play_to_end` never ends, so
  its caller ends it. Each converts the sound to the device's format on
  the calling thread (an error also when it cannot be converted).
- `Sound` — a sound decoded to 16-bit PCM, shared as `Arc<Sound>`:
  `from_wav` and `from_wav_file` decode WAV files (8, 16, 24, or 32-bit
  integer PCM, or 32-bit float, any rate and channel count, through the
  small pure-Rust `hound` crate), `from_pcm` takes samples, and
  `tone(frequency_hz, duration_ms)` generates a sine at 48 kHz, about
  10 dB below full scale, fading in and out over at most 5 ms so it does
  not click (20 Hz to 20 kHz, up to five seconds). A sound longer than a
  minute is refused. `format` and `duration` describe it, and `samples`
  returns its interleaved samples in that format. It is converted
  to the device's format through the same converter as speech the first
  time it plays in that format, and kept converted.
- `PlaybackEvent` — `Started` (the utterance's first frame has played:
  the device has taken it from its queue), `Heard` (the device's own
  clock passed that frame, `ago` before the report's time; only from a device
  with a clock, and dropped when playback stops first: after each wake the
  mixer compares the clock, from the mix position at which the device
  last started, with each start waiting for it), `Mark` (playback reached
  an index mark), and `Ended` with an `UtteranceEnding`. Each carries the
  utterance id and trace id.
- `PlaybackListener` — the callback a source's events go to, each with
  the time the audio thread read in the pass that reported it. It runs
  on the delivery thread, so it must be quick and must not call back
  into the mixer.
- `AudioTap` — receives the mixer's output as it plays: `opened(format,
  wake)` is called on the audio thread's recovery path when the device
  opens, where the tap may allocate, with an event it may set;
  `played(samples, format)` is called on the audio thread with the
  interleaved frames that have just played, each call following the
  frames of the previous one, so it must not lock, wait or allocate.
- `SilentDevice` — a device that plays at real-time speed into silence.
- `WavRecorder` — an `AudioTap` that writes what played to a WAV file in
  step with real time. `create(path)` creates the file and, beside it,
  `<path>.start`, holding the recorder's creation time as Unix time in
  milliseconds, and starts the recording's clock.

Implementation notes, sounds. A source mixes sounds over its own audio as
voices. A voice waits until the source's audio has been mixed up to its
place (its source position), or starts with the next frames mixed when it
was played at once, and is then counted in mix positions: frame n of it
is at the mix position it started at plus n. While a voice plays, the
source has frames to mix even when its own audio has run out, so a sound
placed at the end of an utterance plays whole, and the utterance still
ends when its own last frame has played. A voice is let go once its last
frame has played. When the device's queue is discarded, a voice not yet
heard waits for its place again, and one partly heard goes on from the
frame where it was heard, at the next frames mixed; a pause is the same,
so a paused source's sounds wait with it. Failing an utterance drops its
sounds; a cancel and the device changing format drop every sound of the
source. A voice played to its end ends its utterance once its last frame
has played. Gains are applied as the frames are summed, before the mix
is clamped.

Ownership (`docs/design/ownership.md`). The
audio thread owns the mixer's state, and nothing outside it reads or
writes that state. It takes in records through lanes, single-producer,
single-consumer rings of the `rtrb` crate whose push and pop are a
bounds check, one slot read or write and one store of an index, with no
lock, loop or wait (`mixer/records.rs` lists the records: a registration,
frames whose samples follow in the speech lane's sample ring, a mark, a
sound, a sound played at once or to its end, a finish and a failure). A
cancel and a pause are not records: the speech queue thread raises the
source's cancel generation and sets its pause, and the audio thread reads
both each pass, so the queue thread never waits and a cancel is never
refused. Everything it gives out leaves through one outbound ring to the
delivery thread: each playback event with the time the audio thread read,
each underrun, and everything it is done with that owns memory (a voice's
frames, a failure's reason), so the audio thread never frees.

Implementation notes, the audio thread (`mixer/run.rs`): each pass reads
how many frames the device still has queued and works out how many have
played, then drains every lane, then reads each source's cancel
generation and pause, then reports every event whose moment has played,
then mixes as many frames as the device has room for and some source
has. Mixing sums every source's frames and clamps the result to the
float range. When nothing is left to mix and the device has drained, it
stops the device. It then waits on the device: at most 100 ms while
playing, and while idle until a producer, a cancel, a pause or the device
wakes it. A device that will ask to be reopened ends the wait itself, by
its waker or a deadline of its own, so an idle mixer never wakes to
look. The pass takes no lock, waits on nothing else, and allocates and
frees nothing. `cargo xtask ci` holds the pass's module, `mixer/run.rs`,
to the audio rule (`xtask/src/ownership.rs`); the store it calls,
`mixer/store.rs`, is held by the allocation test, which counts the audio
thread's allocations and frees. The recovery path, which opens the
device, logs and may allocate, is its own module (`mixer/recover.rs`).

Ordering rules, which no test can produce and a reviewer checks. The
queue thread raises a source's cancel generation before it cancels the
job's token, and every utterance is registered under the generation
current when it was handed on; the pass drains, then reads the
generations, then fires. So a record the synth thread pushed after it
saw the token cancelled is taken in the same pass as the raised
generation, and an utterance the cancel cut never ends completed. A
cancel ends only utterances registered under an older generation, so the
utterance handed on just after it, which may already be in the store, is
spared and moved up to where the cancelled audio began.

Wake rules. A producer sets the audio thread's wake event, the device's
own event that its `wait` watches, after every push, and the queue thread
after every cancel and pause; an idle audio thread is woken by it, and a
running one makes at most one extra pass. Every other wake is decided
by a level read when the decision is made, never by what a ring held
when a drain began, so no wake is lost to a push or a pop made
meanwhile. The audio thread sets a source's room event after a pass that
let go of frames of the source and left at least half its room free, and
after a pass that took any record from a lane of the source and left the
lane at most half full; the room event is set until a wait takes it, so
a wait that begins after the room was freed returns at once. It sets the
delivery thread's event after a pass that sent it anything. The delivery
thread wakes it after a drain that took at least half the outbound ring:
the audio thread holds events back only when it finds the ring full, and
the drain that empties a full ring takes all of it, so held-back events
go out without waiting for the next period, and at shutdown the audio
thread waits for that wake with no time limit.

The store (`mixer/store.rs`). The audio thread takes a record from a lane
only when its store has room for it: 64 utterances tracked and 1,024
pending events per source, 64 voices, and the samples of a source's
capacity (a 40 ms buffer and 40 ms ahead) at the largest format the mixer
accepts, 8 channels at 192 kHz; a record that does not fit waits in its
lane. An event leaves its source's pending list only when the outbound
ring has room, so none is lost; a full ring delays reports. Every
per-pass buffer is allocated before the audio thread starts, or when the
device opens: the stores at build, the mix and the tap's pending frames
on the recovery path. The reserve also holds a 10 ms device period's
margin beyond a source's capacity. A device that opens in a format
larger than the reserve is refused, and speech is muted, played as
silence, until the device reopens.

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
discarded (a raised cancel generation, a changed pause, a failure, a
device error, or a request to reopen), the mixer stops the device,
rewinds every source to what had actually played, and mixes from there
again, so the other sources lose nothing they had queued. Frames are
released once played.

Backpressure. A source may hold unplayed frames up to the device buffer
plus 40 ms. The audio thread publishes, for each source, how many of its
lane's frames it has let go of, played or discarded, and the format
generation, sample rate and channels in one word, with the buffer's
frames in a second; the speech lane compares them with its own count of
frames pushed, and a `write` beyond the bound waits on the source's room
event, checking again in a loop. This keeps a fast synthesizer from
running ahead of what is heard, which keeps cancellation cheap. The
half-room rule keeps the two threads from waking each other for
slivers: the room event is set only once half the source's room is free.

Endings. Every registered utterance ends exactly once with
`PlaybackEvent::Ended`: completed when its last frame has played,
cancelled by a raised generation or by the mixer being dropped, or failed
by `fail` or by the device reopening in a different sample rate or
channel count (frames already converted for the old format cannot be
played, and frames, sounds and sounds played to their end that arrive in
the old format are discarded; a sound played to its end so discarded
ends at once, and an utterance still being written whose frames arrive
in the old format fails, which covers one registered after the reopen
from a lane that already held its frames). A speech lane's `finish`
always sends the finish, whether or not the converter's tail could be
written, so an utterance the audio thread tracks always ends. Cancel and fail discard the affected audio at once, and
the lane then refuses further writes for those utterances. An utterance
registered under a generation older than the source's ends cancelled as
it arrives, with nothing of it heard.

Pausing. A paused source contributes no frames to the mix, and because a
changed pause takes back what the device had queued and rewinds to what
had played, the pause is heard at once rather than after the device's
buffer drains; resuming mixes from the same place. The device running
dry while a source is paused, or before a resumed source is mixed from
again, is not counted as an underrun.

Device recovery. On any device error, or when `needs_reopen` is set, the
mixer rewinds as above and calls `open` again, on the recovery path, and
sets every room event, since a format change frees every source. When
`open` fails, at start or later, the mixer plays through a
`SilentDevice` in the device's place (`fallback.rs`), so speech goes on
and every utterance still ends. The silent device is 48 kHz stereo, so
an utterance in flight when the device is lost goes on only if the lost
device had that rate and channel count; otherwise it fails, as on any
reopen in a different format. The mixer keeps waiting on the device's own
`wait`, so that the waker and the device's notifications still end the
fallback.
It tries the device again only when `needs_reopen` asks: it keeps no
timer of its own, and waits idle with no time limit.

Starting and gaps. A stopped device is started only once 10 ms of audio
is ready, or once the utterance is completely written: an utterance
whose first write is a few frames (what is left after its leading
silence is trimmed) would otherwise start the device, play them, and
run it dry before the next frames arrive, which is heard as a click. An
underrun is counted when the device ran dry part-way through an
utterance and more of that utterance then played: a gap the listener
hears. The delivery thread logs it as a warning. The device running dry
while an utterance's trimmed trailing silence is held back is not one.
With OneCore and the 40 ms WASAPI buffer, a full audible suite run has no
underruns.

The delivery thread (`mixer/delivery.rs`), at `THREAD_PRIORITY_HIGHEST`
through the application's callback since it only moves messages, runs
each source's listener with the time the audio thread read, logs the
underrun warning, and drops what it is handed. A mark and an ending reach
the speech queue and the reducer one hop later than when the audio
thread ran the listeners itself; the latency ledger keeps the audio
thread's times.

The tap. The mixer keeps a copy of every frame it writes to the device
and gives the tap the frames that have played since the last pass, so
the tap hears exactly what the listener heard, in order. Frames written
to the device and then discarded (a cancel, a fail, a device error or a
reopen) are dropped from that copy and never reach the tap.

`WavRecorder` (`wav.rs`) writes the tapped frames as 16-bit PCM in the
format of the first audio it receives. The tap pushes each played block
into a ring of samples and a ring of block headers, which the recorder's
own thread drains and writes; the rings hold one second of the device's
format, allocated when the device first opens and only when recording.
A block that finds them full is dropped and counted, and the writer logs
the count, so a disk too slow for a second leaves a gap of silence in
the recording and nothing heard changes. Between utterances the mixer
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
runners have none), by the mixer in place of a device that failed to
open, and by `verbatim-app` under `VERBATIM_TEST_AUDIO=null`.

Tests: `tests/mixer.rs` drives the mixer against a scripted device and
covers completion only after the last frame plays, marks, utterances
without audio, cancellation sparing later utterances, an utterance
registered under a generation before a cancel ending cancelled unheard
while one registered under the raised generation plays, an empty
utterance finished after a cancel in one pass ending cancelled, an
utterance whose registration and frames were still in its lane when the
device came back in another format failing with nothing written, failure,
write backpressure, a producer waiting for room woken only once half its
room is free and at shutdown, where its write answers `Break`, replaying
unplayed audio after a reopen, the tap getting exactly what played and
never what was cut off, the audio thread making no allocation and no free
over 200 passes that play speech with a mark and a sound, an earcon, a
cancel, a pause and a resume, and the WAV tap (its test binary counts
each thread's allocations, which the scripted device reads on the audio
thread), and sounds: one placed in
an utterance starting at its place and playing on past the utterance's
end without delaying it, one reached before what follows it was written
waiting for that audio and going no further than it until the utterance
is finished, one stopped or never mixed when its utterance is
cancelled, one played at once over another source's speech, one played
at once into an idle mixer mixed at once, one played to its end in a
format the device has left ending at once with nothing written, and one
partly heard going on from where it was after a reopen; and a
device that fails to open replaced by real-time silence, its waits
capped at the silence's 10 ms period while it plays and with no time
limit once idle, with no second open, then opened again on a notice that
a device is available. Unit tests in `notices.rs` check that a failed open
asks for a reopen only once a device is available or its deadline has
passed, that an open device is reopened on a default change and not on an
arrival, and that an arrival during a failing open is not lost. Unit
tests in `sound.rs` decode WAV files of each sample format, refuse what is
not one, and check a tone's fades and a sound's conversion. A unit test in `wav.rs`
checks that the recorder writes a valid WAV with silence where nothing
played.
