# Speech and audio

Speech priorities, say-all, synthesizer settings, sounds, verbosity, and speech processing.

### Priority lanes

NVDA: NORMAL/NEXT/NOW with resume of interrupted
speech ([Speech](../nvda/speech.md)); almost all speech is NORMAL, and
what cuts it off is a cancel, not a priority. Verbatim: **partial**.
Queued/Next/Interrupt exist, and announcements are `Queued` as NVDA's
are, with key presses, foreground changes, menus, and expired focus
speech cutting speech off ([Focus events](focus-events.md), "When speech
is cut off, and cancellation of expired focus speech"); `Interrupt`
remains for a selection in a list the focus controls, notifications that
ask for it, and a few direct messages such as the time. NVDA's resume of
interrupted lower-priority speech is not yet built: Verbatim's Interrupt
discards. Whether to match it is to be decided before M8's profiles
work.

### Index marks driving callbacks at audible position

NVDA: manager
indexing + WASAPI feed-end callbacks ([Audio output](../nvda/audio.md)).
Verbatim: **matched (unverified)**. The mixer reports each mark when the
device has played it, for every synthesizer (D17). Utterances carry
marks (`SegmentContent::Mark`) and Core's say-all moves the caret or
review cursor by them; the shell turns each reported mark into
`Input::MarkReached`. OneCore and eSpeak NG both place marks themselves,
so both drive say-all.

### Say-all

NVDA: reads by "Say all reads by" (sentence where possible,
paragraph, or line; UIA by line), moves the caret or review cursor as
audio plays, keeps a bounded lookahead, stops on any key, and keeps the
display on ([Speech](../nvda/speech.md), "Say-all"). Verbatim:
**matched (verified)** (the `notepad_say_all` scenario): sentences are
split by Unicode's rules over a paragraph the provider sends when it has
no sentence unit of its own, UIA answers that it has no sentences and
reading goes by line, and terminals read by line; a standard edit
control's sentence read is its line, which Core splits. The display is
kept on through `SetThreadExecutionState`. Compared with NVDA
transcripts: in Windows 11 Notepad both read one line per utterance, and
in a Windows Forms text box (a standard edit control) both read one
sentence per utterance. Skim reading is not offered. Different: NVDA
also queues an empty utterance for the empty line after the text's final
line break, which Verbatim leaves out.

### Say-all speaking without pauses

NVDA: every unit say-all reads passes through `SpeechWithoutPauses`,
which speaks up to the unit's last sentence end (a full stop,
exclamation mark, or question mark after a non-space, with one optional
closing quotation mark or parenthesis, then whitespace) and holds what
follows back to start the next utterance; ten units in a row with no
sentence end are spoken together, and each unit's caret callback stays
where its text starts ([Speech](../nvda/speech.md), "Say-all speaks
without pauses"). Verbatim: **matched (unverified)**: Core splits every
unit say-all reads with `verbatim_text::last_pause`, holds the rest back
with its unit's mark, and speaks a sentence that runs from one line to
the next in one utterance with the second line's index mark where its
text starts, so the caret moves to the second line when its words start
playing and a key leaves it on the line whose words were playing. The
same rule's cases are matched: no abbreviation list ("Dr. " ends a
sentence), no split at a decimal point or after an ellipsis, blank units
counted among the ten. No NVDA transcript of a sentence running across
lines has been compared. Both eSpeak NG and OneCore speak such a
sentence in one synthesis call, the mark inside it, as NVDA's eSpeak NG
driver does; with eSpeak NG the audio is the unmarked sentence's, sample
for sample. Different, by design: NVDA's sentence ends are only the full
stop, exclamation mark, and question mark, so a line of Chinese,
Japanese, Hindi, Arabic, Urdu, Armenian, or Amharic never splits.
Verbatim also ends a sentence at the Chinese and Japanese full-width
marks (。！？, with no space needed after them), the Devanagari danda
and double danda, the Arabic question mark, the Urdu full stop, the
Armenian full stop, and the Ethiopic full stop and question mark, and
also takes guillemets either way round, German quotation marks („ and
“), corner brackets, and the full-width parenthesis as the one closing
character.

### Structured utterances vs flat strings

NVDA: command-laden flat
sequences. Verbatim: **different (D12)** — typed spans flattened
by a theme at the last stage.

### Speech settings model

Speech settings model (driver settings, immediate application).
NVDA: `SynthDriver.supportedSettings` ([Synth
drivers](../nvda/synth-drivers.md), "Loading settings and falling
back"). Verbatim: **matched (unverified)**: the same descriptor-driven
model. Starting a synthesizer, at startup or from the Select Synthesizer
dialog, loads its own saved settings, voice first; a saved value it
refuses, such as a voice no longer installed, is logged and the
synthesizer keeps its own value rather than failing to start. When the
configured synthesizer cannot start, Verbatim tries the others, eSpeak
NG first, and logs which it used; a failed switch keeps the previous
synthesizer, as NVDA does. A synthesizer used as a fallback has its
settings saved but is not saved as the configured one, so the user's
choice is tried again at the next start, as in NVDA; choosing a
synthesizer in the dialog makes it the choice. Different: NVDA applies
the refusal fallback to the voice only and fails the synthesizer on
other refused settings, where Verbatim skips any refused setting. NVDA's
last resort is a silent synthesizer, so it always starts; Verbatim has
none and does not start when no synthesizer can. NVDA writes a corrected
voice back to the config at once, where Verbatim saves it with the next
commit of the settings dialog. The default voice is the synthesizer's
own default, where NVDA picks one matching its or Windows' language. The
settings dialog's sliders use wx's default steps rather than each
setting's minimum and large steps, which gives the same steps for
settings from 0 to 100. Segments are joined with one space where NVDA
joins chunks with two, which is not audible.

### Settings dialog during a synthesizer switch

NVDA: the Select
Synthesizer dialog's OK calls `setSynth` on the GUI thread
(`SynthesizerSelectionDialog.onOk`, `nvda/source/gui/settingsDialogs.py`
lines 1293 to 1313), which terminates the old synthesizer, starts the
new one, and on success sets the configured synthesizer
(`nvda/source/synthDriverHandler.py` lines 504 to 547, the assignment at
544) before it returns, so the settings dialog's OK and Cancel can never
be pressed while a switch is under way. Verbatim: **different**
(2026-10-08): the switch does not block the GUI thread (a
synthesizer host can take its time limit to start), so the settings
dialog can be closed meanwhile, and the switch's outcome decides what is
saved. OK or Apply during the switch saves the synthesizer still active
at once and, once the switch has started the new one, saves that as the
configured synthesizer with its settings; Cancel during the switch
restores nothing until it ends, and restores the previous synthesizer's
committed values only when it fails, since a synthesizer that starts has
its own saved values (`verbatim-speech`'s
`a_commit_during_a_switch_saves_the_synth_the_switch_started` and
`a_revert_during_a_switch_that_fails_restores_the_committed_values`).

### Instant cancel

Instant cancel (stop + reset, audible immediately). NVDA:
`WavePlayer.stop`. Verbatim: **matched (verified)** — WASAPI
stop+reset; latency measured in the E2E ledger.

### Rate boost via sonic, audio ducking, sound split

**not yet (M8)** (ducking and configuration breadth). Tones and
earcons: see the next entry.

### Sonification

Sonification (spelling-error sounds, mode-switch sounds, progress
beeps, indentation tones) and its scheduling split between
in-stream playback-synchronized commands and immediate
fire-and-forget sounds. NVDA: [Sonification](../nvda/sonification.md).
Verbatim: **partial**. Built in the platform-neutral crates, unverified
until the shell wires them: the same two ways to play, a sound placed in
the speech stream at its span's place, overlapping the speech after it
and ended with its utterance, and an event's sound played at once on its
own mixer source, never cut off by speech. Line indentation tones are
not yet built, since indentation is not reported yet. Which events
produce earcons (start and exit, browse and focus mode, suggestions,
progress bars, logged errors) is the shell's and later milestones' to
wire; the reducer emits the progress indication, and none of the others
yet. Different by design (D12): a theme decides, for every indication,
speech, sound, both, or off, where NVDA offers the choice for spelling
errors and line indentation and hard-codes it elsewhere. The default
theme uses NVDA's sound files where NVDA plays them by default. Also
different: NVDA starts an in-stream sound when playback reaches it,
through a callback, so it can be late by the device's buffer; Verbatim
mixes it at its exact sample. A key press that cancels speech also stops
an in-stream sound already playing, where NVDA lets it finish. Progress
bar tones rise three octaves from 220 Hz over 0 to 100 percent, a range
of Verbatim's own; the capital letter tone, used when a theme reports
capitals by sound, is 1760 Hz for 40 ms. In the default theme a spelling
error is reported by speech and sound: where an error starts the sound
plays and then "spelling error" is spoken; where the error ends only the
words "out of spelling error" are spoken. NVDA's own default is speech
alone ([Sonification](../nvda/sonification.md), "The inventory"), so the
default theme's sound is a deliberate difference (Dickson, 2026-10-07).

### Verbosity settings

Verbosity settings (report object descriptions, position information,
keyboard shortcuts, and the document formatting options). NVDA:
checkboxes in the Object Presentation and Document Formatting panels
([Event handling](../nvda/events.md), [Document formatting
reporting](../nvda/document-formatting.md)). Verbatim: **different
(D12)**, not yet compared live: each is an indication in the theme, off,
speech, sound, or speech and sound, with NVDA's defaults in the default
theme (font name, size, color, background color, font attributes,
strikethrough, and the kind of underline off; descriptions, positions,
shortcuts, and links in text spoken). Strikethrough, which NVDA groups
with the font attributes, and the background color, which it groups with
the color, are indications of their own; the kind of underline and the
bullet style, which NVDA does not report through UIA, are Verbatim's own
and off by default. An indication set to off is also not fetched where
the reducer decides what is fetched.

### Formatting while the caret moves

Formatting while the caret moves (milestone M4 item 7). NVDA: the
formatting last spoken is kept per object, and a unit read speaks only
what changed, at its start and where it changes inside it, an error's
end only for a character or a word ([Document formatting
reporting](../nvda/document-formatting.md), "The cache, attribute by
attribute"). Verbatim: **partial**. Matched for caret keys and a focus's
first line, through UIA, with spelling and grammar errors, font name and
size, color, bold, italic, and underline, the background color (with
NVDA's "dark red on light grey" and "light grey background"),
strikethrough ("strikethrough", "double strikethrough", "no
strikethrough"), and links ("link", "out of link", at every unit as NVDA
says them, through UIA's link attribute). Spelling errors are verified
live in Windows 11 Notepad; the background color, strikethrough, and
links are tested against mockapp, not live. Not yet built: NVDA's other
formatting (styles, alignment, indentation, comments, and the rest), and
formatting while reviewing or reading with say-all. The standard edit
controls report no formatting (NVDA moves their selection to read each
character's, which Verbatim does not do). Colors are named in English
only. Different by design: the kind of underline, which NVDA reads only
as underlined or not, is an indication of its own that, when on, says
the kind where the font attributes say "underlined" ("double underline",
"wave underline", Microsoft Word's names, which NVDA uses for Word's
underline commands; a single underline is still "underlined", and its
end "not underlined"); and a list item's bullet, from UIA's bullet
style, which NVDA does not read, is spoken as NVDA speaks Word's line
prefix, at the start of each line read and never for a word or a
character, named as NVDA names the bullet's character ("bullet", "white
bullet", "black square").

### Which text attributes are fetched

NVDA: every attribute its settings
ask for, on every read, whatever the control supports ([Document
formatting reporting](../nvda/document-formatting.md), "UIA providers").
Verbatim: **different** (Dickson, 2026-10-07): the attributes the theme's indications ask for, among
those the focused control supports, learned from the control itself.
An attribute a line answers "not supported" for, while its support is
not yet known, is not asked again while the outpost keeps the node;
one answered otherwise is supported and no longer checked. The
annotation types are never learned that way, since Windows 11 Notepad
answers "not supported" for text without annotations; instead a line
is asked for them once, and its stretches only when it has some, which
NVDA does not do. Hidden text (`IsHidden`) is never read. No list of
applications is involved.

### Text range moves

NVDA: a backward `Move` or `MoveEndpointByUnit` that a
provider answers with a positive count is counted as negative
([Document formatting reporting](../nvda/document-formatting.md), "A UIA
text range move"). Verbatim: **matched (verified)** on both paths: the
classic reads correct every such count
(`verbatim_uia::text::signed_move`), and the remote programs correct
the one count whose sign they use, a units read's movement
(`emit_backward_count` in `verbatim-uia-rops`); the terminal reading's
program uses no backward move's count but to test whether the move went,
which a wrong sign does not change. Tested with mockapp's
`backward_moves.json` (`a_backward_move_is_counted_backward_both_ways` in
`crates/mockapp/tests/text.rs`, remotely and classically).

### Synth isolation

NVDA: in-process drivers (crash = NVDA crash),
one out-of-process precedent ([Synth drivers](../nvda/synth-drivers.md)).
Verbatim: **different (D18)** — native synth host out of process
(M6).

### Symbol/punctuation processing, speech dictionaries, character descriptions

NVDA:
[Symbols, dictionaries, and character processing](../nvda/symbols-and-dictionaries.md).
Verbatim: **partial**. Symbol processing in flowing text and speech
dictionaries are not yet built (M8); D12 plans them over typed spans,
and the dictionaries-before-symbols ordering and the symbol preserve
rules are the parity-critical details. The character table for one
character spoken on its own (caret and review movement, spelling, typing
echo) and character descriptions are built in English only, keyed by
locale in the Fluent files, with NVDA's English symbol names (its
corrected ones included) for the common punctuation and symbols and the
phonetic alphabet; there are no CLDR emoji names yet. Until symbol
processing is built, a protected field's masked text is spoken as its
characters: six stars, where NVDA says "6 star", and a line of a WPF
`PasswordBox`'s circles, which NVDA drops at its default level, "circle"
being a "most" level symbol, though one circle on its own is "circle" in
both (`text_box_password` and `wpf_password`;
"Protected (password) fields" in
[Editable text and terminals](../nvda/editable-text-and-terminals.md)).

### Letter-to-digit splitting

NVDA: its mandatory built-in speech dictionary inserts a space wherever
a digit follows a letter, in everything it speaks, so "issue123" is
spoken "issue 123" and "e2e" as "e 2e"
([Symbols, dictionaries, and character processing](../nvda/symbols-and-dictionaries.md),
the built-in layer under "Speech dictionaries").
Verbatim: **not yet (M8)**, built with punctuation and symbol levels
(Dickson, 2026-10-15). Verbatim speaks the text as written. Open
question, Dickson's (2026-10-15): the rule dates from 2006 and a
pronunciation problem in ViaVoice, and whether current synthesizers
(eSpeak NG, OneCore) still need it is not known, so it is checked with
them before it is built. Until then, compare mode
shows a difference on any spoken name with a digit after a letter.

### Automatic language switching

NVDA: strip-or-pass of language
commands per config ([Speech](../nvda/speech.md)). Verbatim: **not yet
(M8)**; it needs backend language attributes first.
