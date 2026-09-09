# verbatim-audio-wasapi

The WASAPI implementation of [verbatim-audio](verbatim-audio.md)'s
`AudioSink` seam (decision D5). It is a separate crate so that the seam,
and everything coded against it, carries no Windows dependency.

Public API: `WasapiSink`.

Implementation notes: event-driven shared mode with a small (roughly 40 ms)
buffer, initialized with the auto-convert-PCM flags so any synth output
rate is accepted without manual resampling; the device opens lazily on the
first `begin` and the stream is reused across utterances of the same
format. COM is initialized per thread as MTA, tolerating
`RPC_E_CHANGED_MODE`. On the first buffer of each utterance it emits the
`audio_started` tracing event tagged with the utterance's trace ID, the
final leg of the latency timeline. `stop` issues Stop plus Reset to discard
buffered audio immediately, which is how speech interruption sounds instant.
