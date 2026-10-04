# verbatim-synth-capture

The test synth: records every `SpeechSequence` it is asked to speak,
with a timestamp, into a shared `CaptureLog`
(`Arc<Mutex<Vec<CaptureRecord>>>`; each record has `sequence` and `at`),
emits a short quiet tone (100 samples at 22050 Hz, about -30 dBFS, so the
silence trimmer keeps it where it would drop digital silence), and honors
cancellation. Its `places_marks` is `false`, so the speech manager splits
sequences at their marks before they reach it, which is how that fallback
is tested. Pipeline unit
tests assert on its log; it exposes a voice choice and a rate numeric so
the settings host is exercised too.

It is used only by those unit tests and by `VERBATIM_TEST_AUDIO=null`,
under which `verbatim-app` registers it alongside the real synthesizers
(see [verbatim-app](verbatim-app.md)). The end-to-end suite no longer
selects it: every run, silent or audible, speaks through eSpeak NG, and a
silent run differs only in playing through the silent real-time device.
