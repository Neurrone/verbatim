# verbatim-synth-capture

The test synth: records every `SpeechRequest` with a timestamp into a
shared `CaptureLog` (`Arc<Mutex<Vec<CaptureRecord>>>`), emits a short burst
of silent PCM, echoes all marks, and honors cancellation. Pipeline unit
tests assert on its log; it exposes a voice choice and a rate numeric so
the settings host is exercised too.
