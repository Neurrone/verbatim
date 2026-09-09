# verbatim-audio

The `AudioSink` seam (decision D5) and the device-free `NullSink`. The
WASAPI implementation is [verbatim-audio-wasapi](verbatim-audio-wasapi.md);
this crate has no Windows dependency.

Public API: `PcmFormat` (rate and channel count; samples are 16-bit
throughout M1), `AudioError`, the `AudioSink` trait (`begin` with a
`TraceId`, blocking `write`, draining `end`, discarding `stop`) and
`NullSink`.

Implementation notes, `NullSink`: a device-free sink for test and CI runs
with no sound card, active only when `verbatim-app` sees
`VERBATIM_TEST_AUDIO=null` at startup (test-only; documented there).
Accepts any format and discards every sample, but still emits the
`audio_started` tracing event on the first `write` of each utterance,
exactly when `WasapiSink` would emit it on its first real buffer, so
`LatencyLedger` still records a complete timeline with nothing actually
playing.
