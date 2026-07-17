# verbatim-speech

The speech pipeline (architecture section 6): priority lanes, synth
threads, token rendering, and the settings host the GUI talks to.

Public API:

- `SynthDriver` — the synchronous driver contract: identity, `pcm_format`,
  data-driven `supported_settings`, `setting` and `set_setting`, and a
  blocking `speak(request, sink)`. Synchronous by design so a Wasm
  component can implement it unchanged later; Verbatim owns all threading.
- `SynthSink` — receives PCM (`push_pcm` returns `ControlFlow::Break` to
  request cooperative cancellation) and index-mark echoes.
- `SpeechRequest`, `RequestMark`, `IndexMark`, `SynthError`.
- `SettingDescriptor` (`Numeric` with range and steps, `Choice` with option
  pairs, `Toggle`), `SettingId`, `SettingValue`, `SynthId`, `SynthChoice` —
  NVDA's driver-setting model, the source the settings GUI generates its
  controls from. Label fields are Fluent message ids, never display text.
- `SynthRegistry`, `SynthFactory` — drivers registered by id, built on
  demand, switchable at runtime.
- `SpeechManager` — `new(SpeechManagerConfig)`, `speak(utterance)`,
  `settings_host(persist)`. The config carries the registry, the initial
  synth and its persisted setting values, the audio sink, and an optional
  observer.
- `SpeechSettingsHost` (trait) and `SettingsHost` (implementation) — the
  GUI's live handle: list synthesizers, switch the active one, read
  descriptors and values, `set_setting` applying immediately (slider drags
  are audible as they happen), `commit` persisting through an injected
  `PersistFn` so this crate never depends on the config layer, and `revert`
  restoring the last committed values.
- `SpeechEvents` — the observability seam: `utterance_queued` and
  `audio_started`, called on pipeline threads and required to be cheap.
- `Theme` and `PlainTheme` — the presentation stage (decision D12): a theme
  flattens each structured utterance to the flat request handed to the
  synthesizer, on the queue thread, just before dispatch. `PlainTheme` is
  the default (selected when `SpeechManagerConfig::theme` is `None`) and
  renders plain speech: labels, values, and descriptions as their text,
  roles and states through `verbatim-i18n`, positions as "2 of 5" (nothing
  without a set size — a bare position has no useful spoken form), levels
  as "level 3". M11's earcon and voice-styling themes implement the same
  trait, which is why utterances carry their source node's role and screen
  rectangle even though `PlainTheme` ignores both.

Implementation notes, `SpeechManager`: two dedicated threads. The queue
thread owns the priority lanes — `Interrupt` cancels current and queued
speech (a shared flag the sink checks makes the driver stop, and the audio
sink's `stop` discards buffered sound), `Next` jumps ahead of `Queued` —
and dispatches one job at a time, synchronized by a finished handshake from
the synth thread. The synth thread owns the active driver and the audio
sink and is where `SynthDriver::speak` blocks. Setting changes and synth
switches travel to the synth thread as commands, and `SettingsHost` keeps a
mirror of descriptors and values so GUI reads never hop threads.
