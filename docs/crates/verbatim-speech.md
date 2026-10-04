# verbatim-speech

The speech pipeline (architecture section 6): priority lanes, synth
threads, token rendering, and the settings host the GUI talks to.

Public API:

- `SynthDriver` — the synchronous driver contract: identity, data-driven
  `supported_settings`, `setting` and `set_setting`, `places_marks`, and a
  blocking `speak(sequence, sink)`. Synchronous by design so a Wasm
  component or the synthesizer host (decision D18) can implement it
  unchanged; Verbatim owns all threading. A driver only produces PCM and
  never plays it (decision D17). `places_marks` says whether the driver
  reports each index mark at its exact place in its audio; a driver that
  says `false` never sees a mark (see the mark fallback below).
  `changes_pitch` (default `false`) says whether it speaks a pitch change
  itself; a driver that says `false` never sees one (see "Pitch changes"
  below).
- `SynthSink` — receives the driver's output: `push_pcm(format, samples)`
  takes interleaved 16-bit PCM in whatever format the driver produces and
  returns `ControlFlow::Break` to request cooperative cancellation, and
  `index_reached(mark)` says the audio pushed so far reaches a mark, and
  `is_cancelled()` lets a driver that works a while before it has audio
  (OneCore) stop early.
- `SpeechSequence` — what a synthesizer is asked to speak: the utterance
  id, the trace id, an optional BCP 47 language, and `items`, a list of
  `SpeechItem::Text`, `SpeechItem::Mark`, and `SpeechItem::Pitch` (an
  offset from the configured pitch, `0` to return to it). Plain
  serializable data, so it can cross a process or Wasm boundary unchanged.
  `text` joins the text items, `has_marks` and `has_pitch_changes` report
  whether any mark or pitch change is present, `split_at_marks` cuts the
  sequence into pieces each followed by the mark that ended it, and
  `split(at_marks)` cuts at pitch changes, and at marks too when asked.
- `SpeechItem`, `IndexMark`, `SynthError`.
- `SettingDescriptor` (`Numeric` with range and steps, `Choice` with option
  pairs, `Toggle`), `SettingId`, `SettingValue`, `SynthId`, `SynthChoice` —
  NVDA's driver-setting model, the source the settings GUI generates its
  controls from. Label fields are Fluent message ids, never display text.
- `SynthRegistry`, `SynthFactory` — drivers registered by id, built on
  demand, switchable at runtime.
- `SpeechManager` — `new(SpeechManagerConfig)`, `speak(utterance)`, which
  returns the `UtteranceId` the utterance's milestones and ending are
  reported under, and `settings_host(persist)`. The config carries the
  registry, the initial synth, a `SavedSettingsFn` that reads any
  synthesizer's persisted setting values, the `Arc<Mixer>` speech plays
  through (the manager adds its own source to it), an optional observer,
  and an optional theme. `control()` returns a `SpeechControl`.
- `SpeechControl` — a cheap, cloneable handle for cutting speech off from
  any thread without blocking, which the keyboard hook and the reducer's
  effects use: `cancel()` cancels current and queued speech and ends a
  pause; `toggle_pause()` pauses speech where it is, or resumes it when
  paused; `drop_expired(now)` drops focus speech whose `FocusValidity` no
  longer holds for the `FocusNow` given (see "When speech is cut off"
  below).
- `SpeechSettingsHost` (trait) and `SettingsHost` (implementation) — the
  GUI's live handle: list synthesizers, switch the active one, read
  descriptors and values, `set_setting` applying immediately (slider drags
  are audible as they happen), `commit` persisting through an injected
  `PersistFn` so this crate never depends on the config layer (it is
  told whether the active synthesizer is the user's choice: one started
  in place of the configured one is not, until one is chosen in the
  dialog), and `revert`
  restoring the last committed values.
- `SpeechEvents` — the observability seam, one call per milestone of each
  utterance: `utterance_queued` (with its rendered text), `audio_started`
  (its first frame has played), `mark_reached` (playback reached an index
  mark; a default no-op), and `utterance_ended` (its one ending, as an
  `UtteranceEnding`). `utterance_queued`, and the ending of an utterance
  cancelled before synthesis, are called on the queue thread; the rest on
  the mixer's audio thread, so every implementation must be cheap and
  non-blocking.
- `Theme` and `PlainTheme` — the presentation stage (decision D12): a theme
  flattens each structured utterance to the `SpeechSequence` handed to
  the synthesizer, with `flatten(utterance, id)`, on the queue thread when
  the utterance is accepted. `PlainTheme` is
  the default (selected when `SpeechManagerConfig::theme` is `None`) and
  renders plain speech: labels, values, and descriptions as their text,
  roles and states through `verbatim-i18n`, positions as "2 of 5" (nothing
  without a set size — a bare position has no useful spoken form), levels
  as "level 3". M11's earcon and voice-styling themes implement the same
  trait, which is why utterances carry their source node's role and screen
  rectangle even though `PlainTheme` ignores both.

- `hosting` (a public module) — the synthesizer host protocol (decision
  D18), the `SynthDriver` contract made into messages so a host process
  runs any driver unchanged. `ToHost` is what Core sends: `Speak(sequence)`,
  `Cancel(utterance_id)`, and `SetSetting { id, value }`. `FromHost` is
  what a host sends: `Ready(HostDescription)` first, or `Unavailable(reason)`
  before exiting; then `Pcm(format, samples)`, `Mark`, and `Done` or
  `Failed(reason)` for each utterance, and `SettingApplied(refusal)` for
  each setting. `HostDescription` carries the display name,
  `places_marks`, `changes_pitch`, the setting descriptors, and their
  current values.
  `write_to_host` and `read_to_host` serve the Core-to-host direction,
  `write_from_host` and `read_from_host` the other; each read returns
  `None` at a clean end of stream (a broken pipe between frames counts as
  one). `synth_ids` holds the ids of the hosted synthesizers, which
  Core registers and the host builds by: `ESPEAK` (`espeak`) and
  `ONECORE` (`onecore`). The host process is
  [verbatim-synth-host](verbatim-synth-host.md),
  and its Core-side driver is
  [verbatim-synth-hosted](verbatim-synth-hosted.md).

Implementation notes, `SpeechManager`: two dedicated threads, plus the
mixer's audio thread that plays what they produce. The queue thread owns
the priority lanes — `Interrupt` cancels current and queued speech, `Next`
jumps ahead of `Queued` — and dispatches one job at a time, synchronized
by a finished handshake from the synth thread. The synth thread owns the
active driver and is where `SynthDriver::speak` blocks, writing into the
manager's mixer `Source`. Setting changes and synth switches travel to the
synth thread as commands, and `SettingsHost` keeps a mirror of descriptors
and values so GUI reads never hop threads.

Starting a synthesizer, at startup or on a switch, builds it from the
registry and then applies its saved values, read through the
`SavedSettingsFn` at that moment so a switch sees values committed since
startup. Values are applied in the driver's descriptor order, voice first.
Each is checked against its descriptor and then set; one that fails
either check, such as a voice no longer installed, is skipped with a
warning and the driver keeps its own value, so a stale setting never
costs speech. At startup, when the initial synthesizer cannot be built,
the manager tries every other registered synthesizer in registration
order and logs which one it used; `new` fails only when none starts.
The app registers eSpeak NG first, so it is the first fallback. A failed
switch leaves the previous synthesizer active. All of this matches NVDA
as described in docs/nvda/synth-drivers.md, less NVDA's silent last
resort.

Endings (decision D17). Every utterance `speak` accepts ends exactly once,
reported through `SpeechEvents::utterance_ended`. Until the queue thread
hands an utterance to the synth thread, the queue thread owns its ending:
an utterance cleared from a lane is reported cancelled there. When it
dispatches a job, the queue thread first registers the utterance with the
mixer, and from then on the mixer owns the ending: completed when the
device has played the last frame, cancelled when the source is cancelled,
or failed when synthesis fails (the synth thread calls `Source::fail`) or
the audio device fails. The mixer's playback events become `SpeechEvents`
calls in a listener the manager installs on its source.

Cancellation is per utterance. Each job carries its own cancellation flag,
which the pipeline's sink checks on every `push_pcm`. Cancelling
everything (an `Interrupt`, `SpeechControl::cancel`, speech arriving
while paused, a synth switch, or shutdown) sets the
in-flight job's flag, reports every lane entry cancelled, and calls
`Source::cancel_all`, which ends every utterance the mixer holds as
cancelled and discards its unplayed audio at once. The mixer refuses
further audio for an utterance it has ended, so a driver still running
gets `Break` on its next push even if its flag was not yet seen. Utterances
registered after the cancellation are unaffected.

When speech is cut off (`docs/parity.md`, "When speech is cut off, and
cancellation of expired focus speech"). Besides an `Interrupt` utterance,
three things end speech early, all carried to the queue thread as events:

- `SpeechControl::cancel`, which every key press calls, cancels everything
  as above.
- `SpeechControl::drop_expired`, which every focus change calls, drops
  expired focus speech, judged as NVDA judges it. The queue thread
  remembers the validity of every utterance it has handed on whose ending
  the mixer has not reported yet. When any of those no longer holds,
  everything handed on is stopped (the in-flight job's flag and
  `Source::cancel_all`): audio cannot be taken out of the middle of the
  mixer's buffer, so speech handed on after the expired utterance is
  stopped too, which NVDA keeps. Waiting speech is not judged then: the
  queue thread keeps the latest `FocusNow`, and `pump` checks each waiting
  utterance against it when its turn comes, reporting one that no longer
  holds cancelled and moving on, so valid speech queued ahead of expired
  speech is still heard. Utterances without a validity never expire.
- Pausing. `SpeechControl::toggle_pause`, which Shift calls, pauses the
  manager's mixer source (`Source::pause`), which holds its audio and
  playback events where they are; calling it again resumes. Any cancel ends
  the pause, and an utterance arriving while paused cancels what was paused
  first, as NVDA does, since the key press that would otherwise come first
  would have cancelled it.

Silence trimming (`trim.rs`). Every driver's PCM passes through a trimmer
before reaching the mixer. Quiet frames (every sample within 64 of zero,
about -54 dBFS) before the first audible frame are dropped, so the first
word is not delayed. After that, each run of quiet frames is held back:
released when audible audio follows, so pauses between words stay, and
dropped when the utterance ends, so trailing silence does not delay the
ending or the next utterance. At most two seconds of quiet audio is held;
a longer run is released as it is. Marks reached during held audio keep
their place relative to it.

Mark fallback. When the active driver's `places_marks` is `false` and the
sequence has marks, the synth thread splits it with `split_at_marks` and
speaks the pieces one after another into the same utterance, placing each
mark after the piece it ended. Every mark is then exact at the cost of a
synthesis boundary at each mark.

Pitch changes. `PlainTheme` renders a `SegmentContent::SpelledCapital`
as a pitch change of `CAPITAL_PITCH_OFFSET` (30, NVDA's default), the
letter, and a return to the configured pitch. A driver whose
`changes_pitch` is `true` (eSpeak NG and OneCore) is given the sequence
with its pitch changes and speaks them within one synthesis, as NVDA's
drivers do. For any other, a sequence holding one is split at it (and at
marks too, when the driver cannot place them), and between pieces the
synth thread sets the driver's own `pitch` setting to the value it had
when the job began plus the offset, limited to 0 to 100; after the job,
however it ended, the pitch is put back. That can leave a short pause
around the capital. A driver with neither has the pitch changes removed.

Host framing (`hosting.rs`). Every message is one frame: a kind byte, a
little-endian `u32` body length, and the body, written in a single write
and flushed, so the peer never sees a frame split by another writer.
Kind 0 is JSON (serde's encoding of the message enum; every `ToHost` and
every `FromHost` except PCM). Kind 1 is PCM: the sample rate as a
little-endian `u32`, the channel count as a little-endian `u16`, then the
samples as little-endian `i16`, since a synthesizer streams far more audio
than anything else and JSON would multiply its size. A reader rejects a
length above 16 MB as `InvalidData` rather than allocating it, and
rejects an unknown kind, a PCM body shorter than its six-byte header or
with an odd sample byte count, and a PCM frame sent to a host. A stream
that ends part-way through a frame is an `UnexpectedEof` error. `Cancel`
names the utterance it cancels, so a cancel that crosses that utterance's
`Done` on the pipe cannot cancel the next utterance. A unit test writes
every message of both directions through a buffer and reads them back.
