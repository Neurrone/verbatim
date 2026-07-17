# Synth drivers

A synth driver is NVDA's adapter between the speech manager
([Speech](speech.md)) and one synthesis engine. The contract is
`synthDriverHandler.SynthDriver` (`source/synthDriverHandler.py`);
the in-tree drivers (`source/synthDrivers/`) are the worked examples:
`espeak` (bundled, C library, the default), `oneCore` (Windows
voices), `sapi5` and `sapi5_32`, `sapi4_32` (via out-of-process
bridging for 32-bit-only engines; `_synthDrivers32`), `mssp`
(Microsoft Speech Platform), `silence`.

## The contract

- `speak(speechSequence)` — receives the manager's processed sequence
  (strings plus commands). The driver declares which command types it
  supports (`supportedCommands`) and which settings
  (`supportedSettings`: rate, pitch, volume, voice, variant, rate
  boost…, each a `DriverSetting` with min/max normalization to 0–100);
  the manager only sends what is declared, converting the rest (for
  example, synthesizing utterance breaks itself for drivers without
  `BreakCommand`).
- `cancel()` — stop immediately and drop anything queued in the
  engine. NVDA calls this constantly (every keystroke that interrupts
  speech); a driver where cancel is slow makes the whole screen
  reader feel laggy — cancel latency is effectively part of input
  latency.
- `pause(switch)` — used by shift (pause speech) where supported.
- Notifications, via extension points (`synthIndexReached`,
  `synthDoneSpeaking`, module level): the driver must report index
  marks as playback (not synthesis) passes them, and done-speaking
  when audio truly finishes; both may fire from any thread and are
  re-queued to the main thread by the manager. Index timing accuracy
  directly determines say-all caret tracking and profile-switch
  timing ([Speech](speech.md)).
- Voice/setting change handling, `loadSettings`/`saveSettings`
  via the shared driver machinery (`source/driverHandler.py`,
  `autoSettingsUtils/`), and availability probing (`check()` class
  method — whether the engine exists on this system).

`setSynth(name)` (same file) tears down the old driver and
instantiates the new one, re-applying config; drivers can come from
add-ons ([App modules mechanism](app-modules-mechanism.md)).

## Audio delivery

Drivers do not touch audio devices directly: they synthesize PCM and
feed it through `nvwave.WavePlayer` ([Audio output](audio.md)) — espeak and oneCore
literally receive PCM callbacks and `feed()` chunks with the index
attached via feed's completion callbacks, which is how index-reached
aligns with *playback* position. SAPI5 is the exception: it can let
the engine push audio to a SAPI audio object; NVDA wraps this so
routing and ducking still apply.

## The SAPI drivers, concretely

API background for both generations:
[Speech APIs](../explainers/speech-apis.md).

- **SAPI 5** (`source/synthDrivers/sapi5.py`): the centerpiece is
  `SynthDriverAudioStream`, a COM object NVDA implements (interfaces
  `ISpAudio`, `ISpEventSource`, `ISpEventSink`) and hands to
  `ISpVoice::SetOutput` — so the engine "plays" into NVDA's buffers,
  which are fed to the shared `WavePlayer`
  ([Audio output](audio.md)) instead of SAPI's own device path. SAPI
  then delivers *all* events (start stream, bookmark) to that audio
  object, and the driver re-dispatches bookmark events as
  `synthIndexReached` — which is what makes index timing follow
  NVDA's real playback clock rather than SAPI's. Index marks are
  injected as `<bookmark mark="N"/>` elements in the generated SAPI
  XML; rate/pitch/volume map to the voice's native scales; pause uses
  the audio stream rather than `ISpVoice::Pause`. Voice enumeration
  walks the SAPI token categories including the OneCore-mirrored
  ones.
- **SAPI 4** (`source/_synthDrivers32/sapi4.py` and `_sapi4.py`): the
  interface definitions (`ITTSEnum`, `ITTSCentral`, `ITTSAttributes`,
  `ITTSBufNotifySink`, `IAudioDest`) are transcribed by hand in
  `_sapi4.py` — SAPI 4 predates usable type libraries — and the
  driver implements an audio destination to capture PCM, with
  `TextData` buffer callbacks providing start/done/bookmark events.
- **The 32-bit bridge**: SAPI 4 engines are 32-bit only, and plenty of
  SAPI 5 voices are too, while NVDA's main process bitness stopped
  matching them. `synthDrivers/sapi4_32.py` and `sapi5_32.py` are thin
  `SynthDriverProxy32` wrappers (`source/_bridge/clients/synthDriverHost32/`):
  the `_bridge` package (`source/_bridge/base.py`) spawns a 32-bit
  host process, connects RPYC (Python object remoting) over anonymous
  pipes, and runs the real driver from `source/_synthDrivers32/`
  inside the host, proxying the whole `SynthDriver` contract across —
  audio and index callbacks included. Availability checks read the
  32-bit registry view (`KEY_WOW64_32KEY`) to detect engines the main
  process cannot see.

## Design facts with system-wide consequences

- Drivers run *in the NVDA process* (except the 32-bit SAPI4 bridge);
  an engine crash is an NVDA crash. The bridge pattern for 32-bit
  engines (`sapi4_32`, `_synthDrivers32` — a helper process with the
  engine, PCM shipped back) is NVDA's only out-of-process synth
  precedent.
- The rate-boost feature for engines without native fast rates is
  implemented by resampling through *sonic*
  (`source/synthDrivers/_sonic.py`, C library) between the engine
  and the player.
- The trickiest part of any driver, judging by all in-tree examples,
  is index bookkeeping across cancel: after `cancel()`, indexes from
  the aborted stream must not fire late (they would run stale
  callbacks — the manager guards, but drivers are expected to flush).
