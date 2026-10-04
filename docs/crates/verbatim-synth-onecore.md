# verbatim-synth-onecore

The OneCore driver over WinRT `Windows.Media.SpeechSynthesis`, the
built-in alternative to the default, eSpeak NG
([verbatim-synth-espeak](verbatim-synth-espeak.md)). Verbatim runs it inside the synthesizer host
process (decision D18): [verbatim-synth-host](verbatim-synth-host.md)
builds it when started with `--synth onecore`, and
[verbatim-app](verbatim-app.md) registers OneCore through
[verbatim-synth-hosted](verbatim-synth-hosted.md) rather than linking
this crate. The driver itself is unchanged by that move.

Public API: `OneCoreSynth::new()`, `ONECORE_ID`, and `factory()` plus
`register(registry)` conveniences, which build the driver in the calling
process; nothing in Verbatim calls the two conveniences now. Everything
else is the trait.

Implementation notes: voices are enumerated once, when the driver
starts, into a `voice` Choice descriptor, and the synthesizer is given a
voice only when the selection changed;
rate, pitch, and volume are NVDA-convention 0 to 100 numerics mapped onto
the WinRT options (rate 50 is exactly 1.0x, following NVDA's curve with
minimum 0.5). The `rate-boost` toggle mirrors NVDA's `_set_rateBoost`
semantics precisely: flipping it preserves the percent and re-applies it
against the boosted maximum of 6.0 (1.5 unboosted), so enabling boost at 50
audibly jumps from 1.0x to 3.25x — the jump is the feature. Appended
silence is set to its minimum at startup, as NVDA does, and the speech
manager trims what is left. `speak` waits on the synthesis operation's
completion (its thread is dedicated), checking every 10 ms whether the
sink reports the utterance cancelled, and cancels the operation if so:
OneCore produces no audio until it has synthesized the whole utterance,
so there is no earlier push at which a cancel would be seen. The sequence
is sent as SSML, its text escaped and each index mark a `<mark>` element
named by the mark's number, in the current voice's language. `speak` then
parses the returned WAV by walking RIFF chunks — the format chunk for the real rate
and channel count rather than assuming, the data chunk for samples — and
pushes PCM in roughly 50 ms slices, checking the sink's `ControlFlow`
between slices so cancellation is prompt. Marks are exact, so
`changes_pitch` is `true`: a pitch change becomes a `prosody` element in
the SSML, relative to OneCore's default of 50 as NVDA's OneCore driver
writes it (50 raised by 30 is "30%"). `places_marks` is `true`: the driver
reads `SpeechSynthesisStream.Markers()`
for the time OneCore placed each numbered mark, pushes the audio up to
that time, reports the mark through `index_reached`, and carries on. The
display name resolves through `verbatim-i18n`. A `#[ignore]`d integration
test audibly speaks the word "test" through the mixer and the real
`WasapiDevice`, and waits for the utterance to end completed.
