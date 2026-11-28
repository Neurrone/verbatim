# NVDA inventory: speech, synthesizers, symbols, sounds, braille, vision, maths

Pinned NVDA: submodule 732d1ce2 (release-2026.3beta3-44). The installed NVDA at `C:\Program Files\NVDA` is the same commit (`lib\alpha-58022,732d1ce2`), and it holds the native libraries and generated data that the source tree lacks.

Replay classes: R1 runs as-is; R2 needs a named new stand-in; R3 is a clock or thread seam; R4 is Windows reacting to NVDA's actions; R5 is native code; R6 is not needed.

## Findings checked by running the harness

The checks ran the harness's `boot.py` under uv's CPython 3.13 with NVDA's Python dependencies and `NVDA_LIB` set to the installed lib. The probe scripts are `probe_speech.py` and `probe_cldr.py` in this folder.

1. Patching `speech.speech.speak`, as `run.py` does, misses speech. Say-all's `SpeechWithoutPauses` keeps the original function, captured when `speech.initialize()` ran (the check printed `True`). Callers of the package attribute `speech.speak` also skip the patch: `globalCommands.py` 5574 (speak the last speech again), `mathPres/MathCAT/MathCAT.py` 121 and 196, `NVDAObjects/UIA/wordDocument.py` 1170 and `NVDAObjects/window/excel.py` 1991. The patch also joins the strings, which throws away the commands, the priority and the symbol level.
2. The silence synthesizer changes what NVDA says. Its voice has no language, so `availableLanguages` is `{None}`. With `autoLanguageSwitching` on, which is the default, spelling "Ab" queued `'English (not supported)'` ahead of the letters. Its `supportedCommands` is empty, so spelling gets no `PitchCommand` for capitals and no character mode.
3. The silence synthesizer never reports an index. The speech manager therefore pushes only the first utterance to it. Everything after that stays queued: in the check, indexes 1 and 2 stayed in `_indexesSpeaking`, and four later utterances stayed pending. A `BeepCommand` (the capital beep) never ran, because a callback runs only when its index is reached.
4. `cldr.dic` is missing from the source tree, because the `include/nvda-cldr` submodule is not checked out. Without it, "😀" was passed through unnamed. With the definition's path pointed at the installed `locale\en\cldr.dic`, it became "grinning face". The CLDR dictionary is on by default (`configSpec.py` 42, `symbolDictionaries = list("cldr")`). The missing file costs only a debug warning (`characterProcessing.py` 413 to 427), so nothing in the harness shows the loss.
5. `builtin.dic` runs before symbol processing and splits camel case and word-to-digit boundaries. In the check, "camelCase" was queued as "camel Case".
6. Uniscribe works in the harness. Importing `NVDAHelper` loads `nvdaHelperLocal.dll` from `NVDA_LIB` (`NVDAHelper/localLib.py` 49), and `splitAtCharacterBoundaries("e\u0301ab")` gave `['é','a','b']`. It is called on every spelling while Unicode normalization is on, which is the default (`speech.py` 497 to 498, 622 to 624).
7. The installed `library.zip` holds `louis/__init__.pyc` and `louisHelper.pyc`, and the install folder holds `liblouis.dll`, `louis\tables`, `libmathcat_py.pyd` and `espeak.dll`. These are built for Python 3.13 (`python313.dll`), and the pyc magic number matches uv's 3.13. The default `python` on this machine is 3.14, and its magic number does not match.
8. NVDA's extension points hold handlers by weak reference. A lambda registered as a capture handler is collected and never called, so register a named function kept alive by the harness.

## Speech pipeline order (`speech/speech.py` `speak`, 1126 to 1233)

1. `filter_speechSequence` runs first. `speech.initialize` registers `languageHandling.getSpeechSequenceWithLangs` there, which adds language names and "(not supported)" (`speech/__init__.py` 172).
2. `pre_speech` is notified with the filtered sequence, the original sequence, the symbol level and the priority (1148 to 1153). Braille's "show speech in braille" and `_setLastSpeechString` listen here.
3. The speech mode is checked: off returns, beeps plays `tones.beep`, on-demand speaks only for scripts marked `speakOnDemand`, input help, or say-all started from a script.
4. Language commands are rebuilt (one `LangChangeCommand` before each string whose language changed).
5. `processText` runs on each string: speech dictionaries, then symbols, then whitespace, then Unicode normalization. `CHUNK_SEPARATOR` (two spaces) is added outside character mode (192 to 214, 1216 to 1232).
6. `_manager.speak(seq, priority)` is called.

The manager (`speech/manager.py`) turns callbacks into `IndexCommand`s, splits sequences at indexes and at the ends of utterances, keeps one queue per priority, and interrupts on the first `NOW`. It drops an utterance holding an expired `_CancellableSpeechCommand` when it builds the utterance and when `removeCancelledSpeechCommands` runs, which `eventHandler.py` 400 calls after `setFocusObject`. It calls `pre_synthSpeak`, then `synth.speak` (435 to 436), and pushes the next utterance only on an index report or a done-speaking report, both queued to the main thread (590 to 595, 717 to 722).

## Inventory

| Module (lines) | What a user hears or feels from it | Needs from outside Python | Class |
|---|---|---|---|
| `speech/speech.py` (3198): speak, cancel, pause, message, SSML | Every utterance; speech modes; "blank"; the cancel of say-all on cancel | config, `getSynth()` | R1 |
| same: spelling and character descriptions (328 to 673) | "cap", capital pitch and beep, "normalized", phonetic descriptions (delayed by a `BreakCommand(1000)`), conjunct handling for Indic locales | Uniscribe (`textUtils.uniscribe`); the synth's name, language and supported commands | R1, R5 for Uniscribe (works in-process), R2 for the synth facts |
| same: object, property and control-field speech (676 to 1023, 2013 to 2620) | Name, role, states, value, position, table cells, landmarks, placeholder | NVDAObject and TextInfo properties (other inventories) | R1 |
| same: text-info speech (1494 to 2012), format fields (2620 to 3131), indentation (1045 to 1120) | Lines, words, formatting changes, spelling errors, indentation by speech or by tones (`BeepCommand`) | TextInfo | R1; the indentation tones need index reports (R3) |
| same: selection (1236 to 1418) | "selected", "unselected", "selected instead", "selection removed", a count of characters at 512 or more | TextInfo compare | R1 |
| same: typed characters and words (1420 to 1492) | Character and word echo, "*" in protected fields, suppression for characters NVDA sent itself | `time.time()`, 0.1 s window (1477) | R3 (clock) |
| `speech/manager.py` (773) | What reaches the synth and when; NOW interrupts; NEXT waits; resuming preempted speech with its parameters; expired focus speech dropped; profile switches | Index and done reports from the synth; `queueHandler` | R3 |
| `speech/sayAll.py` (486) | Say all by object, caret, review or table; the caret moves as lines are reached; at most 10 lines buffered ahead (238, 359 to 370) | Index callbacks; `queueFunction(nextLine)`; `updateCaret` | R3, R4 (`updateCaret` sets the application's caret, and the application then fires caret events) |
| `speech/speechWithoutPauses.py` (178) | Say-all chunks joined up to a sentence end (regex 32 to 34) | none | R1 |
| `speech/commands.py` (485) | Command meanings; prosody defaults read from `config.conf["speech"][synth.name]` (276 to 281) | the synth's name | R1 |
| `speech/types.py` (88), `priorities.py` (32), `extensions.py` (67), `__init__.py` (179) | Priority order NOW, NEXT, NORMAL; extension points | none | R1 |
| `speech/languageHandling.py` (84) | Language names, "{lang} (not supported)" or a beep at 500 Hz | `synth.languageIsSupported`, `languageHandler.getLanguageDescription` | R1 with R2 synth |
| `speech/shortcutKeys.py` (151) | How shortcut keys are spoken, spelled when character mode is supported | the synth's supported commands | R1 with R2 synth |
| `synthDriverHandler.py` (636) | Synth selection and fallback, `languageIsSupported` (339 to 362), notifications | driver classes | R1 |
| `synthDrivers/silence.py` (35) | Replaced; see the findings | none | R2: a recorded-synth stand-in |
| `synthDrivers/espeak.py`, `_espeak.py` (797), `oneCore.py` (651), `sapi5.py` (1094), `_sonic.py` (228), `sapi4_32`, `sapi5_32`, `mssp` (82) | Audio, and the timing and skipping of index reports (the manager handles OneCore skipping indexes, `manager.py` 672 to 680) | espeak.dll, nvdaHelperLocalWin10, SAPI COM, WASAPI | R6 for audio; their capability sets are copied into the R2 stand-in (espeak and SAPI5 support character mode, OneCore does not: `espeak.py` 44 to 53, `oneCore.py` 176 to 184, `sapi5.py` 538 to 547) |
| `characterProcessing.py` (953) and `locale/en/symbols.dic` (631), `characterDescriptions.dic` (32), installed `cldr.dic` | Symbol names by level, preserve rules, character descriptions, fallback to "en" | data files | R1, with `cldr.dic` taken from the install |
| `speechDictHandler/` (782) and `builtin.dic` (6) | Temporary, default, voice and built-in dictionaries; the voice dictionary is chosen by synth name and voice | user config files | R1 (R2 synth name) |
| `languageHandler.py` (478) | Language descriptions, locale normalization | kernel32 locale calls (deterministic on one Windows build) | R1 |
| `tones.py` (90) | Beeps | `generateBeep` in nvdaHelperLocal, WASAPI | R1 up to `decide_beep` (71 to 80), the capture point; R6 for audio |
| `nvwave.py` (575), `wasapi.py` (128) | Wave files (start, exit, error, toggle sounds) | WASAPI through nvdaHelperLocal | R1 up to `decide_playWaveFile` (105 to 113), the capture point; R6 for playback |
| `audioDucking.py` (311) | Other applications' volume | nvdaHelperLocal, `time` | R6 |
| `audio/` (281): sound split | Which channel each application's audio plays in | pycaw, Core Audio | R6 |
| `ui.py` (243) | `message` (speech plus a braille message), `delayedMessage` (`core.callLater`, 199 to 205), `reviewMessage`, "Copied to clipboard" | GUI for `browseableMessage` | R1; `delayedMessage` R3; browseable messages R6 |
| `textUtils/__init__.py`, `segFlag.py`, `_braille.py` (638) | Offset conversion, Unicode normalization | none | R1 |
| `textUtils/uniscribe.py` (39) | Character boundaries in spelling | nvdaHelperLocal | R5 (loads from `NVDA_LIB`) |
| `textUtils/icu.py` (209), `_wordSeg/` (648) | Word and sentence units for text-info movement | Windows `icu.dll`, `cppjieba.dll` | R5 |
| `textUtils/hyphenation.py` (44) | Braille word wrap only | pyphen | R5 (braille only) |
| `louisHelper.py` (260) | Braille translation and back-translation | liblouis | R5 (installed copy loads under 3.13) |
| `braille/brailleHandler.py` (1128) | What the display shows: focus and review tethering, focus context, messages and their timeout, cursor blink, auto-scroll, speech shown in braille, progress-bar throttling | `wx.CallLater` (730, 757), blink timer (559 to 562), `hwIo.bgThread` writes (636), `time.time()` (912), `wx.CallAfter` | R3 |
| `braille/regions/` (1586), `buffers.py`, `formatting.py`, `labels.py`, `constants.py`, `extensions.py`, `display/`, `input/gesture.py`, `__init__.py` (2105), `brailleTables/` (1144) | Region text and abbreviations, the window, routing, the table list | liblouis through `louisHelper` | R1 code over R5 |
| `braille/input/inputHandler.py` (464), `brailleInput.py` (34, a shim) | Braille typing, "dot 1 2", contracted-cell reports, echo suppression | liblouis, `winUser.SendInput` (407), `emulateGesture`, `time.time()` 0.3 s caret attribution (434) | R3, R4, R5 |
| `brailleDisplayDrivers/` (11852) | Hardware protocols and key maps | serial, HID, Bluetooth, brlapi | R6, replaced by an R2 display stand-in (a non-thread-safe display with a fixed `numCells` whose `display()` records); `noBraille` (20) gives 0 cells, which disables braille |
| `brailleViewer/` (702), `speechViewer.py` (216) | On-screen mirrors | wx | R6 |
| `vision/` (861) | No output with no provider enabled (`configSpec.py` 116 to 120); extension points only | none | R1 |
| `visionEnhancementProviders/` (823), `screenCurtain/` (307) | The highlight rectangles and the black screen; nothing heard beyond toggle messages spoken elsewhere | GDI+, Magnification API, wx | R6 |
| `mathPres/` (291) and `MathCAT/` (1696) | Maths speech, navigation, braille; the synth's supported commands choose breaks, pitch, phonemes and character mode (`MathCAT/speech.py` 67 to 71) | `libmathcat_py.pyd` | R5 (installed copy, 3.13); interaction R4 |

## Where to capture speech

- **Requested speech:** register a named handler on `speech.extensions.pre_speech`. It sees every non-empty sequence after the language filter, including say-all and callers of the package attribute, with its priority and symbol level. It comes before speech-mode handling, dictionaries and symbols. It keeps the commands, including `FocusLossCancellableSpeechCommand` and `CallbackCommand`. Compare Verbatim against this stream for what was said and in what order. It is before symbol processing, which suits Verbatim today: `docs/roadmap.md`, around line 1770, still speaks of symbol levels as future work.
- **Delivered speech:** register on `synthDriverHandler.pre_synthSpeak` and record the stand-in synth's `cancel()` calls and the indexes it reports. This stream is after dictionaries and symbols, after the manager's priority ordering and expired-focus cancellation, and split into utterances. It only means something if the stand-in synth reports each index at a simulated time, which needs a duration model per utterance. What a user hears is each delivered utterance up to the last index reported before a cancel. Speech dropped as expired focus is the difference between the requested and delivered streams. For "Lanes, cancel and marks", compare Verbatim against this stream.
- **Sounds:** register deciders on `tones.decide_beep` and `nvwave.decide_playWaveFile` that return False. Both carry `isSpeechBeepCommand` or `isSpeechWaveFileCommand`. Beeps inside a sequence fire only at their index.
- **The stand-in synth (R2) must carry the user's synth facts:** its `name` (it keys `config.conf["speech"][name]`: cap pitch, say cap, beep for capitals, spelling functionality, and the voice dictionary), its voice `language` and `availableLanguages`, `supportedCommands`, and `supportedNotifications`. To model OneCore, it can optionally skip indexes that have no text between them.

## Where to capture braille

Use a display stand-in with a fixed cell count. Register on `braille.extensions.pre_writeCells`, which gives cells, `rawText` and the cell count (`brailleHandler.py` 601). Use `_pre_showBrailleMessage` and `_post_dismissBrailleMessage` for messages. Read `handler.mainBuffer.regions` for each region's raw text before translation. Cells need liblouis (R5), and region text needs it too, because `Region.update` always translates (`braille/regions/base.py` 122).

Turning braille on adds reads that the world must answer: focus-context regions over the ancestors (`brailleHandler.py` 796 to 801) and text-info regions. Keep braille off in speech-only runs. The harness currently stubs the braille package; a real `noBraille` disables it the same way.

## Verbatim features each bears on (`books/dev/src/reference/test-coverage.md`)

- `speak`, the manager and priorities: "Lanes, cancel and marks", "Cancel speech", "Pause speech", every scenario.
- Object and property speech: "Focus announcement", "State changes", "Value changes and sliders", "Table cells", "Menus", "Combo box opened", "Report current object".
- Text-info and format speech: "Caret keys", "Document formatting", "Spelling errors", "Line breaks", "Protected fields", review cursor rows.
- Spelling and character descriptions: "Spelling, capitals and character descriptions", "Current character", the second and third presses of "Current line" and "Current word".
- Typed characters and words: "Typed character echo", "Typed word echo", "Typed characters (read-only)", "Speak typed characters", "Speak typed words".
- Selection: "Selecting with Shift".
- Say-all and speech without pauses: "Say all from caret", "Say all from review", "Say all settings".
- `textUtils` (Uniscribe and ICU): "Word and character segmentation".
- Symbols and dictionaries: typed punctuation names; the roadmap's symbol-level items.
- `languageHandling`: multilingual text (no Verbatim row beyond the terminal batch test).
- `tones` and `nvwave`: "Themes and sounds", "Startup announcement", "Starting and exiting", "Help balloons and progress bars".
- `ui.message`: "Speak time and date", "Lock keys", the toggles ("Follow caret toggle", "Report new output").
- Synth stand-in: "Synthesizer drivers", "Synthesizer switching", "Speech settings".
- Braille: M18 in the roadmap, with no feature yet. Vision: M8.4 (highlight) and M8.6 (screen curtain). Maths: M14.

## Hardest items

1. The speech manager's timing (R3): index and done reports, NOW and NEXT preemption, and expired-focus drops at push time. The delivered stream depends on a simulated speech duration.
2. Say-all (R3 and R4): the 10-line read-ahead, `lineReached` callbacks that move the application's caret, which answers with caret events, and joining at sentence ends.
3. Synth fidelity (R2): name, languages and supported commands change the words themselves (finding 2).
4. Data the source tree lacks: `cldr.dic` (finding 4), plus liblouis and MathCAT taken from the install (finding 7).
5. Braille (R3 and R5): timers, liblouis, and the extra world reads.
6. Clock seams spread through these modules: the typed-character suppression (0.1 s), braille input's caret attribution (0.3 s), the progress-bar braille throttle (1 s), `delayedMessage`, and `nvwave`'s idle check (`core.callLater`, 472).

## Lines per class (modules in this area)

| Class | Lines | Modules |
|---|---|---|
| R1 | about 13,980 | speech package except manager and say-all (4,462); characterProcessing (953); speechDictHandler (782); languageHandler (478); ui (243); synthDriverHandler (636); textUtils core (638); tones (90); vision (861); braille regions, buffers, display and tables (4,835, over R5 liblouis) |
| R2 | 55 replaced | silence (35) by a recorded synth; noBraille (20) by a display stand-in |
| R3 | 2,851 | speech manager (773), say-all (486), braille handler (1,128), braille input (464; also R4 and R5) |
| R4 | parts only | say-all's `updateCaret`, braille input's `SendInput` and emulated keys, maths interaction |
| R5 | about 3,190 | uniscribe (39), icu (209), word segmentation (648), hyphenation (44), louisHelper (260), mathPres (1,987) |
| R6 | about 18,030 | display drivers (11,832), real synth drivers (2,852), nvwave and wasapi (703), audio and ducking (592), viewers (918), vision providers and screen curtain (1,130) |
