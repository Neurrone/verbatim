# Audio output on Windows

A screen reader's audio path has unusual requirements: lowest possible
latency (speech must start the instant a key is pressed), instant
cancellation (stop mid-word when the next key arrives), and coexistence
with every other audio app. This file covers the WASAPI concepts needed
to reason about that path; the synthesis APIs that *produce* the audio
are in [Speech APIs](speech-apis.md).

## WASAPI in one page

WASAPI (Windows Audio Session API) is the ground-truth audio API;
everything higher level (XAudio2, MediaFoundation, legacy winmm) wraps it.
Devices are *endpoints* (enumerated via `IMMDeviceEnumerator`, with a
user-set default that can change at any moment); a client opens an
`IAudioClient` on an endpoint in one of two modes:

- **Shared mode**: your stream is mixed with everyone else's by the audio
  engine at the engine's mix format and period (typically 10 ms). Latency
  floor is roughly one to two engine periods; Windows 10+ offers
  low-latency shared mode (`IAudioClient3`) with smaller periods where
  drivers allow. Screen readers use shared mode — exclusive mode would
  silence the rest of the system.
- **Exclusive mode**: direct device access, lowest latency, locks out
  other audio. Not for this project.

The render loop: `GetBuffer` / fill PCM / `ReleaseBuffer`, paced either by
polling with `GetCurrentPadding` or (better) event-driven via
`SetEventHandle`. Underruns produce audible glitches, so the render thread
must be scheduling-privileged: register it with *MMCSS* (the "Pro Audio" /
"Audio" task classes) rather than hand-raising thread priority.
Cancellation — the screen reader signature move — is `IAudioClient::Stop`
plus `Reset` (drops all queued data instantly) or simply ceasing to submit
and letting a short buffer drain; short buffers are what make "shut up
immediately" possible, and are why a screen reader cannot just enqueue ten
seconds of synthesized audio and walk away.

## Sessions, ducking, and device churn

Each stream belongs to an *audio session* with its own volume and mute
(what Volume Mixer shows); *audio ducking*
(`IAudioSessionControl2` opt-in/out and communication-stream
attenuation) lets a screen reader ask the system to lower other apps'
audio while it speaks — or be excluded from being ducked itself.
Default device changes and device removal arrive as
`IMMNotificationClient` callbacks and must be handled by reopening the
stream: a screen reader that goes silent on headphone unplug has failed
this. How NVDA's C++ player handles all of the above is in
[Audio output](../nvda/audio.md).

## References

- [About WASAPI (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/coreaudio/wasapi)
- [IAudioClient3 (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nn-audioclient-iaudioclient3)
- [Multimedia Class Scheduler Service (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/procthread/multimedia-class-scheduler-service)
- [Using the Audio Session API (Microsoft Learn)](https://learn.microsoft.com/en-us/windows/win32/coreaudio/audio-sessions)
