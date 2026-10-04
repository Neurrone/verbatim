# verbatim-synth-onecore

The OneCore driver over WinRT `Windows.Media.SpeechSynthesis` — the voice
Verbatim first speaks with.

Public API: `OneCoreSynth::new()`, `ONECORE_ID`, and `factory()` plus
`register(registry)` conveniences. Everything else is the trait.

Implementation notes: voices enumerate into a `voice` Choice descriptor;
rate, pitch, and volume are NVDA-convention 0 to 100 numerics mapped onto
the WinRT options (rate 50 is exactly 1.0x, following NVDA's curve with
minimum 0.5). The `rate-boost` toggle mirrors NVDA's `_set_rateBoost`
semantics precisely: flipping it preserves the percent and re-applies it
against the boosted maximum of 6.0 (1.5 unboosted), so enabling boost at 50
audibly jumps from 1.0x to 3.25x — the jump is the feature. `speak` blocks
on the synthesis async operation (its thread is dedicated). The sequence
is sent as SSML, its text escaped and each index mark a `<mark>` element
named by the mark's number, in the current voice's language. `speak` then
parses the returned WAV by walking RIFF chunks — the format chunk for the real rate
and channel count rather than assuming, the data chunk for samples — and
pushes PCM in roughly 50 ms slices, checking the sink's `ControlFlow`
between slices so cancellation is prompt. Marks are exact, so
`places_marks` is `true`: the driver reads `SpeechSynthesisStream.Markers()`
for the time OneCore placed each numbered mark, pushes the audio up to
that time, reports the mark through `index_reached`, and carries on. The
display name resolves through `verbatim-i18n`. A `#[ignore]`d integration
test audibly speaks the word "test" through the mixer and the real
`WasapiDevice`, and waits for the utterance to end completed.
