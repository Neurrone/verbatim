# Audio output

NVDA's audio path: synth drivers and sound effects feed
`nvwave.WavePlayer`, whose implementation is a C++ WASAPI player
inside `nvdaHelperLocal`. Related features: audio ducking, sound
split, and tones.

## The player

`source/nvwave.py` (`WavePlayer`, the WASAPI implementation —
`WasapiWavePlayer` — is the only one since 2025) wraps
`nvdaHelper/local/wasapi.cpp` (`class WasapiPlayer`):

- Shared-mode WASAPI render per stream, with a persistent feeder
  design: `feed(data, size, onDone)` submits PCM and returns an id;
  the C++ side tracks, per feed, where that chunk *ends* in playback
  time (`feedEnds`) and fires the chunk's callback when the playback
  clock passes it — this is the machinery that makes synth index
  callbacks track *audible* position, not synthesis position
  ([Synth drivers](synth-drivers.md)).
- `stop()` drops everything unplayed immediately (the instant-shutup
  primitive), `pause(switch)`, `idle()` (mark stream done),
  per-player volume (`setVolume`) — used by the *sound split*
  feature (`source/audio/soundSplit.py`: NVDA speech on one stereo
  channel, everything else on the other, via per-session channel
  volumes).
- Device selection by endpoint name with default-device tracking:
  when the configured device is "default", device-change
  notifications reopen streams on the new default; device removal
  degrades gracefully (players silently reopen on the new device
  rather than erroring — the code's explicit goal is that unplugging
  headphones never kills speech).
- Wake bug workarounds and the trimming of leading silence
  (`silenceDetect.h`, optional config) — an accumulated-latency
  optimization for synths that pad output.

## Ducking

`source/audioDucking.py`: NVDA does not implement ducking itself —
it toggles the OS assistive-technology ducking state
(`AccSetRunningUtilityState` with the `ANRUS_*` audio flags on a
helper window) so *Windows* attenuates other sessions. Modes: no
ducking, duck while speaking or playing sounds (timed around speech
activity, `_setDuckingState` calls at output start/stop), duck
always. Requires uiAccess to affect other processes' audio
([Processes and security](../explainers/processes-and-security.md)); without it the
setting is disabled.

## Tones and sounds

`source/tones.py` — the classic NVDA beep (progress bars, errors in
debug builds…): generated PCM sine waves through a dedicated
`WavePlayer` (`nvdaHelper/local/beeps.cpp` computes the waveform);
`nvwave.playWaveFile` plays the shipped .wav effects (browse/focus
mode, errors) on a shared player. Both respect the same output
device and sound-split routing as speech. Sounds and speech use
*separate streams*, so a beep never blocks or trims speech.

## Threading picture

Feeding happens from whatever thread the synth driver synthesizes on;
`WasapiPlayer` serializes internally (its own feeder/notification
threading, callbacks fired on a background thread and re-marshaled by
users as needed — the speech manager re-queues index callbacks to the
main thread; [Speech](speech.md)). None of the audio path runs on the main
thread, so audio continues (finishing the current utterance) even
while the core is blocked — which is why a frozen NVDA finishes its
sentence: the PCM already fed keeps playing.
