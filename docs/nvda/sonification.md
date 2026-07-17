# Sonification: sounds, tones, and how they schedule against speech

NVDA's non-speech audio vocabulary is small and grew case by case;
what makes it worth documenting is the *scheduling model* — which
sounds are synchronized with speech and which just fire — because
that split is exactly the design surface for any richer earcon system
(Verbatim's M11 themes). This file covers both.

## The two scheduling mechanisms

**In-stream, playback-synchronized.** A speech sequence can carry
`BeepCommand`, `WaveFileCommand`, or the general `CallbackCommand`
(`speech/commands.py`, all subclasses of `BaseCallbackCommand` —
"commands which cause a function to be called when speech reaches
them"; "never passed to synth drivers"). The speech manager converts
each to an index mark ([Speech](speech.md)); when *playback* reaches
that point, the callback runs on the main thread (required to return
fast, per its own docstring) and starts the beep or wave
asynchronously on its own audio stream. Consequences worth stating
precisely: the sound is positioned correctly relative to the spoken
words, but speech does not pause for it — the sound overlaps the
following speech; and if the utterance is cancelled before playback
reaches the command, the sound never fires (in-stream sounds are
cancelled with their speech).

**Immediate, fire-and-forget.** Everything else calls `tones.beep`
or `nvwave.playWaveFile` directly at the triggering moment: the
sound plays now, on a separate WASAPI stream mixed over whatever
speech is playing ([Audio output](audio.md)), completely outside the
speech queue — no ordering guarantee relative to spoken words, never
cancelled by speech interruption, never delaying speech. There is no
third mechanism: no sound priority system, no sound queue, no
speech-ducking-for-sounds. The only global gate is speech mode
(sounds are suppressed in speech-off and on-demand modes at most
call sites).

Which mechanism a feature uses is the design decision, per feature,
and the inventory below marks each.

## The inventory

- **Spelling errors, while reading text** — in-stream: the
  formatting differ appends `WaveFileCommand("waves\textError.wav")`
  (or the words "spelling error", per the `reportSpellingErrors2`
  speech/sound flags) at the exact position the invalid-spelling run
  starts and ends (`speech.getFormatFieldSpeech`;
  [Document formatting reporting](document-formatting.md)).
- **Spelling errors, after typing a word** — immediate, and the case
  the question always asks about
  (`NVDAObjects/behaviors.py`, `EditableTextBase.event_typedCharacter`
  and `_reportErrorInPreviousWord`): when a typed character ends a
  word (whitespace or punctuation), and both `reportSpellingErrors2`
  and the keyboard setting `alertForSpellingErrors` are on, NVDA
  takes the caret TextInfo, steps back to the last character of the
  finished word, and — **after a deliberate 50 ms delay**
  (`core.callLater`; the app needs time to run its spell check and
  mark the range; Word's UIA requires it, issue
  [#12161](https://github.com/nvaccess/nvda/issues/12161)) — fetches
  that character's formatting and plays `textError.wav` directly if
  `invalid-spelling` is set. Note what this is *not*: not an event
  from the app, but a self-scheduled re-query; apps that mark errors
  slower than the delay are silently missed.
- **Browse/focus mode switch** — immediate: `browseMode.wav` /
  `focusMode.wav` on every pass-through flip
  ([Browse mode](browse-mode.md)).
- **Suggestion list appearing/disappearing** — immediate wavs from
  the `InputFieldWithSuggestions` behavior
  ([Object model](object-model.md)).
- **Progress bars** — immediate `tones.beep` per value-change event,
  pitch encoding the percentage ([Object model](object-model.md);
  output mode config in [Event handling](events.md)).
- **Line indentation as tones** — in-stream: `getIndentationSpeech`
  emits beep commands at the line's start when the indentation
  reporting mode is tones ([Document formatting reporting](document-formatting.md)).
- **Capital letters** — in-stream, and the third flavor of audio
  formatting: a `PitchCommand` (synth parameter change) around the
  letter, optionally plus a `BeepCommand`, during character
  echo/spelling ([Speech](speech.md)).
- **Audio coordinates** — immediate positional beeps for mouse and
  touch ([Mouse and touch](mouse-and-touch.md)).
- **Error sound** — immediate, on logged errors in dev/test builds
  ([Logging](logging.md)).
- **Lifecycle and misc** — start/exit wavs, screen-curtain and
  OCR-related cues, braille display connection sounds: all immediate
  `playWaveFile` calls at their sites.

## What this means for a richer design

NVDA's split is instructive in both directions. The in-stream
mechanism proves the speech pipeline can position sounds precisely
(and inherit cancellation) with nothing more than the existing
index/callback machinery — that is the natural rendering target for
theme-driven earcons that *replace or accompany spoken words* (role
earcons, formatting cues). The immediate mechanism is right for
state-transition cues that must not wait for the speech queue (mode
flips, alerts). NVDA's limitation is that the choice is hard-coded
per feature and the vocabulary is a dozen fixed wavs; there is no
theming layer, no per-sound volume/pan model beyond the beep API's
stereo parameters, and no way for the same semantic event to render
as word, earcon, or parameter change by configuration — which is
precisely the gap D12's span/theme architecture is positioned to
close in M11.
