# verbatim-audio

The `AudioSink` seam (decision D5) and its WASAPI implementation.

Public API: `PcmFormat` (rate and channel count; samples are 16-bit
throughout M1), `AudioError`, the `AudioSink` trait (`begin` with a
`TraceId`, blocking `write`, draining `end`, discarding `stop`),
`WasapiSink`, and `NullSink`.

Implementation notes, `WasapiSink`: event-driven shared mode with a small
(roughly 40 ms) buffer, initialized with the auto-convert-PCM flags so any
synth output rate is accepted without manual resampling; the device opens
lazily on the first `begin` and the stream is reused across utterances of
the same format. COM is initialized per thread as MTA, tolerating
`RPC_E_CHANGED_MODE`. On the first buffer of each utterance it emits the
`audio_started` tracing event tagged with the utterance's trace ID — the
final leg of the latency timeline. `stop` issues Stop plus Reset to discard
buffered audio immediately, which is how speech interruption sounds instant.

Implementation notes, `NullSink`: a device-free sink for test and CI runs
with no sound card, active only when `verbatim-app` sees
`VERBATIM_TEST_AUDIO=null` at startup (test-only; documented there).
Accepts any format and discards every sample, but still emits the
`audio_started` tracing event on the first `write` of each utterance,
exactly when `WasapiSink` would emit it on its first real buffer, so
`LatencyLedger` still records a complete timeline with nothing actually
playing.
