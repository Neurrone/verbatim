# Speech, audio and synthesizers: merged and verified review (2026-11-20)

Merged from reviewer A (`speech-audio-A.md`) and reviewer B (`speech-audio-B.md`). Every quotation, line number, grep count, NVDA citation and commit below was checked against the repository at `ba89dca7`, with the NVDA submodule at `732d1ce2f0f051da3ef1e6a6247c64cf04596337`. Terms: "handed on" means given by the speech queue thread to the synth thread, after which the mixer owns the utterance's ending; "fallback" means a code path serving a driver capability that no registered speaking driver lacks.

## 1. Size

Recounted with `wc -l`. Product lines exclude each file's trailing `#[cfg(test)]` module (cut at its first `#[cfg(test)]` line).

| Behaviour or mechanism | Verbatim product | Verbatim tests | NVDA counterpart (pinned) | NVDA lines | Ratio |
|---|---|---|---|---|---|
| Speech queue, priorities, cancel, marks, endings, driver contract, registry, settings host (`verbatim-speech` manager 1,669, driver 430, events 126, inbox 97, lib 179, registry 111, host 245, settings 150) | 3,007 | pipeline.rs 1,848, sounds.rs 467, inline 119: 2,434 | `speech/manager.py` 773, `speech/commands.py` 485, `speech/priorities.py` 32, `speech/types.py` 88, `synthDriverHandler.py` 636, `autoSettingsUtils/` 455 | 2,469 | 1.22 |
| Presentation: theme flattening, capitals (`theme.rs`) | 610 | 604 | `speech/speech.py` 389 to 652 (spelling and capitals) | 264 | 2.3 (themes and earcons are D12) |
| Silence trimming (`trim.rs`) | 140 | 51 | `nvdaHelper/local/silenceDetect.h` | 214 | 0.65 |
| Audio output: mixer, sounds, tap, conversion, silent device (`verbatim-audio` 4,398) and WASAPI (`verbatim-audio-wasapi` 671) | 5,069 | mixer.rs 1,401, inline 423: 1,824 | `nvwave.py` 575, `nvdaHelper/local/wasapi.cpp` 926, `tones.py` 90, `nvdaHelper/local/beeps.cpp` 41, `audio/soundSplit.py` 265 | 1,897 | 2.7 |
| Synthesizer host protocol, host, Core-side driver (`hosting.rs` 329, `verbatim-synth-host` 628, `verbatim-synth-hosted` 677) | 1,634 | hosting inline 141, synth-host tests 1,153, hosted inline 49: 1,343 | none (NVDA runs drivers in process) | 0 | D18 |
| eSpeak NG driver (874 plus `build.rs` 243) | 1,117 | 596 | `synthDrivers/espeak.py` 359, `synthDrivers/_espeak.py` 438 | 797 | 1.4 (1.1 without build.rs) |
| OneCore driver | 691 | 99 | `synthDrivers/oneCore.py` 651, `nvdaHelper/localWin10/oneCoreSpeech.cpp` 552 and `.h` 70 | 1,273 | 0.54 |
| Capture synth (test-audio mode only) | 195 | 0 | none | 0 | |
| **Area total** | **12,463** | **6,951** | | **6,914** | **1.80; 1.54 without D18 hosting and D12 theme (10,219 against 6,650)** |

NVDA's tests for the same: `tests/unit/test_speechManager/` 1,831, `test_synthDriverHandler.py` 170, `test_synthDrivers/test_espeak.py` 149, `test_nvwave.py` 30: 2,180. Verbatim's tests are 3.19 times NVDA's.

Where the reviewers differed: A put the area at 2.1 (NVDA 5,930: it left out `autoSettingsUtils` and counted only 40 lines of spelling); B at 1.75 (NVDA 6,869: it left out `beeps.cpp` and Verbatim's `build.rs` and capture synth). B's speech-crate figure of 3,007 is right; A's "about 3,150" is not. The settled figures are above.

What the extra lines buy: the host process and its protocol (D18, 1,634 product and 1,343 test lines); themes and sounds placed in the speech stream (D12, D17); the mixer that reports endings and marks at playback for every driver (D17), the recording tap (D16), real-time silence for a failed device (D5), and an audio thread with no allocation or lock (`books/dev/src/explanation/design/ownership.md`). The rest is the copies and fallbacks in the findings below.

Deletion target, from the findings below:

| Part | Product lines | Test lines |
|---|---|---|
| Remove now | about 300 | about 190 |
| If Dickson rules for the removal (capability fallbacks; trailing-silence trim) | about 240 | about 335 |
| Planned phase (M6.3, M6, M4.9) | about 500 | about 1,500 (M6.3's queue-reducer and stepped mixer tests) |

The two defects add about 20 product and 45 test lines.

## 2. Findings

| id | title | class | found by | file and line | what goes | product lines saved | test lines saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| F1 | eSpeak NG obeys U+0001 embedded commands in application text | Defect | A only (B saw the two escapes drift, F15) | `crates/verbatim-synth-espeak/src/lib.rs` 717-741, 869-873 | one escape for both drivers' text, removing U+0001 | about 10 net | adds about 15 | yes, only for text holding U+0001, to NVDA's; needs a capture |
| F2 | The capability fallbacks (marks split by the manager, pitch through the pitch setting) serve no registered speaking driver | Question for Dickson | both (A: pitch; B: both) | `crates/verbatim-speech/src/manager.rs` 1464-1527, 1578; `driver.rs` 148-212, 401-415 | two trait methods, two description fields, split and pitch-setting code | about 150 | about 300 | no, for every registered driver |
| F3 | Speech observer wrapped twice behind an `Option` that is always `Some` | Remove now | both | `manager.rs` 348-351, 729-837; `crates/verbatim-app/src/speech_events.rs` 37-121 | `Observer`, 29 guards, the app's forwarding | about 150 | 0 | no |
| F4 | Per-utterance state kept three times on the queue thread; endings reported from two threads; Core gets back its own marks as two inputs | Planned phase (M6.3) | both | `manager.rs` 912-931, 1007-1022, 1081-1095, 1200-1204 | `in_flight`, `handed_on`, `marked`, two trait methods, two Core inputs | about 250 | most of the endings sorting | no |
| F5 | An utterance whose post is refused never ends | Defect | A only | `manager.rs` 478-481, 487-490, 897 | (adds about 8) | 0 | adds about 30 | no |
| F6 | Dead and test-only API | Remove now | both (items split) | see paragraph | `paused()` and flag, `play_earcon_to_end`, `split_at_marks`, `SynthDriver::display_name`, OneCore `factory` and `register`, `SynthRegistry::contains` | about 105 | about 95 | no |
| F7 | Trailing and inner silence trimmed, where NVDA trims only leading silence | Question for Dickson | both | `crates/verbatim-speech/src/trim.rs` 1-9, 97-109 | the held run and its release | about 90 | about 35 | yes (gaps, not words); needs a capture |
| F8 | The presenter seam exists only for tests that hand-write the presenter's output | Remove now | B only | `manager.rs` 133-137, 628-635 | `presenter` field and `presenter_or_theme`; two test presenters | about 30 | about 45 replaced | no |
| F9 | Hosted synthesizer's setting values are Core's copy of what it sent | Planned phase (M6) | both | `crates/verbatim-synth-hosted/src/lib.rs` 480-505; `hosting.rs` 94 | cache update; validation written three times | about 40 | 0 | no |
| F10 | `Wire` mirrors `FromHost` for a samples field that is always the whole section | Planned phase (M6) | both | `crates/verbatim-speech/src/hosting.rs` 136-155, 218-311 | `Wire`, its two translations, the check | about 50 | about 15 | no |
| F11 | Sounds travel as index marks with bit 63 set | Planned phase (M6.3) | both | `manager.rs` 1541-1560, 1619-1626 | the bit, two `unwrap_or`s, a silent miss | about 0 to 20 | 0 | no |
| F12 | `FocusValidity { had_focus: false }` behaves exactly as `None` | Remove now | A only | `crates/verbatim-model/src/speech.rs` 476, 486-502; `crates/verbatim-core/src/reduce.rs` 711-715, 748-752 | the field | about 15 | about 5 | no |
| F13 | The settings mirror's active synthesizer is optional, with a made-up default | Remove now | both | `crates/verbatim-speech/src/host.rs` 178-185 | the `Option` and default | about 10 | 0 | no |
| F14 | `#[non_exhaustive]` inside one workspace hides an unspoken span kind | Remove now | both | `crates/verbatim-speech/src/theme.rs` 577; `verbatim-model/src/speech.rs` 63, 141, 213, 298 | four attributes and the wildcard | about 5 | 0 | no |
| F15 | Two SSML writers that already drift | Remove now (shape of F1's fix) | B only | espeak `lib.rs` 838-873; onecore `lib.rs` 484-538 | one writer | about 30 | 0 | only through F1 |
| F16 | Host command state as flags; four cancel paths into a driver | Planned phase (M6) | both | `crates/verbatim-synth-host/src/lib.rs` 349-362, 379-384; `src/main.rs` 67 | flags, `interrupted_by` | about 20 | 0 | no |
| F17 | OneCore's hand-written WAV parser beside `hound` | Planned phase (M6.3) | B only | `crates/verbatim-synth-onecore/src/lib.rs` 605-680 | parser and the planned generated test | about 75 | planned test not needed | no |
| F18 | Mixer smaller shapes: `Voice` owner flags, lane writes naming their utterance again, 16-bit format generation | Planned phase (M6.3) | both (generation); B only (others) | `crates/verbatim-audio/src/mixer/store.rs` 104-121; `mixer/mod.rs` 206-219, 760-769 | flags | about 0 | 0 | no |
| F19 | A second unsafe kernel-event wrapper in the WASAPI crate | Planned phase (M4.9) | both | `crates/verbatim-audio-wasapi/src/lib.rs` 76-118 | about 40 unsafe lines | about 40 | 0 | no |
| F20 | `Utterance::source` is carried and read by nothing | Planned phase (M15) | B only | `verbatim-model/src/speech.rs` 446-448 | nothing now; its comment names M11 where the roadmap's positional sound is M15 | 0 | 0 | no |

### F1. eSpeak NG obeys U+0001 embedded commands in application text (Defect, A only; confirmed)

Every link checked:

- eSpeak NG is built from the vendored submodule `third_party/espeak-ng` (`crates/verbatim-synth-espeak/build.rs` line 42: `let source = manifest.join("../../third_party/espeak-ng");`).
- eSpeak NG's embedded-command character: `third_party/espeak-ng/src/libespeak-ng/translate.h` line 37, `#define CTRL_EMBEDDED    0x01 // control character at the start of an embedded command`. `readclause.c` lines 647-665 act on it outside the `if ((option_ssml) ...)` block (562-625), so whatever the flags: `if (c1 == CTRL_EMBEDDED) { // an embedded command. If it's a voice change, end the clause` then `if (c2 == 'V') { buf[ix++] = 0; // end the clause at this point` reads the following non-space characters as a voice name and returns `CLAUSE_VOICE`; `else if (c2 == 'B')` sets `option_punctuation`. Any other U+0001 reaches `translate.c` line 1208, `if (c == CTRL_EMBEDDED) { // start of embedded command in the text`, which calls `EmbeddedCommand` (line 786: `const char *commands = "PSARHTIVYMUBF";`, pitch, speed, amplitude and the rest, an optional sign and number then the letter) and replaces the command with spaces.
- NVDA strips it: `nvda/source/synthDrivers/espeak.py` lines 104-113, `def _processText(self, text): ... return text.translate({ 0x1: None,  # used for embedded commands`, `0x3C: "&lt;"`, `0x3E: "&gt;"`, `0x5B: " ["`, applied to every text item in `speak` at line 195, `textList.append(self._processText(item))`. (NVDA's other replacement, `[` to ` [`, guards eSpeak NG's `[[` phoneme input; Verbatim's driver does not do that either. Not verified further here.)
- Verbatim's plain path, `crates/verbatim-synth-espeak/src/lib.rs` 717-741: `let plain: String = items.iter().filter_map(|item| match item { SpeechItem::Text(text) => Some(text.as_str()), _ => None }).collect();` then `let (text, mut flags) = if markup { (ssml(items, self.pitch), ESPEAK_CHARS_UTF8 | ESPEAK_SSML) } else { (plain, ESPEAK_CHARS_UTF8) };`. The text is untouched; `c_string` (325-328) refuses only NUL.
- Verbatim's markup path: `ssml` (838-865) writes `SpeechItem::Text(text) => ssml.push_str(&escape_xml(text))`, and `escape_xml` (869-873) is `text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")`. U+0001 passes.
- Nothing above the driver removes it. `theme.rs` `is_break` (534-539) maps only `'\0' | '\r' | '\n' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'` to a space. Core's `text_segments` (`crates/verbatim-core/src/text.rs` 226-234) passes text through as `SegmentContent::Text(text.to_owned())`. Verbatim has no symbol processing yet (`docs/roadmap.md` 1662, "Full pronunciation/symbol dictionaries"). The host protocol carries the sequence as data. A grep for `is_control|is_ascii_control|\u{1}|\x01` over `crates/` finds only key handling (`verbatim-core/src/editing.rs` 1047, `terminal.rs` 265, `verbatim-input-windows/src/typed.rs`, the agent's typing), none on a speech path.
- OneCore is not affected: its `escape` (`crates/verbatim-synth-onecore/src/lib.rs` 518-537) has `'\u{0}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}' => escaped.push(' ')`.

So application text can reach eSpeak NG with U+0001 in it, on both the plain and the SSML path. Sequence: a window title, document line or clipboard text holding U+0001 followed by `V` and a word, read by focus, caret or say-all with eSpeak NG (the default): eSpeak NG ends the clause there and switches to a voice of that name, or with `B` changes its punctuation option, or with a number and `S` or `P` changes rate or pitch for the rest of the utterance. NVDA speaks the text without the character. Fix: one escape shared by both drivers (F15), removing U+0001 as NVDA does (and the XML-forbidden control characters, as OneCore's does), used on both of eSpeak NG's paths; a driver test with U+0001 followed by `V`. Speech changes only for text holding U+0001, towards NVDA's code; no capture of such text exists, so the exact speech needs a capture.

### F2. The capability fallbacks serve no registered speaking driver (Question for Dickson, both)

Registered in product code (`crates/verbatim-app/src/main.rs` 645-664 and 736-742): eSpeak NG and OneCore, each as a `verbatim_synth_hosted::factory` (D18), always; the capture synth, `SynthId::new("capture")`, only under `VERBATIM_TEST_AUDIO=null`. The end-to-end suite runs eSpeak NG (`crates/verbatim-e2e/src/scenario.rs` 3935, `Settings::for_e2e(ESPEAK_ID)`), so the capture synth serves only the speech crate's pipeline tests in practice. What each supports:

| Driver | `places_marks` | `changes_pitch` | pitch setting |
|---|---|---|---|
| eSpeak NG (`verbatim-synth-espeak/src/lib.rs` 672-678) | true | true | yes |
| OneCore (`verbatim-synth-onecore/src/lib.rs` 345-351) | true | true | yes |
| Hosted (`verbatim-synth-hosted/src/lib.rs` 508-514) | relays the host's description | relays | relays |
| Capture (`verbatim-synth-capture/src/lib.rs` 115-127, 161-163) | false | false (trait default, `driver.rs` 413-415) | none: voice and rate only |

So the pitch-setting fallback (`manager.rs` 1468-1474: `Some(SettingValue::Number(pitch)) if !inline_pitch && sequence.has_pitch_changes()`) reaches no registered driver: the capture synth has no pitch setting, so its pitch changes are removed (1477-1481, `sequence.without_pitch_changes()`). The marks fallback (`let split_marks = !driver.places_marks() && sequence.has_marks();`, 1475) is reached by the capture synth alone, whose crate comment says "It cannot place index marks, so sequences with marks reach it split at them by the speech manager, which is how that fallback is tested." B's "no driver uses" is right for every speaking engine but not for the capture synth.

NVDA has neither fallback. `nvda/source/synthDriverHandler.py` 71-73: "At a minimum, L{IndexCommand} must be supported. L{PitchCommand} must also be supported if you want capital pitch change to work; support for the pitch setting is not sufficient." `nvda/source/speech/speech.py` 572-575 (single character with description) and 618-621 (spelling): `if PitchCommand in synth.supportedCommands: capPitchChange = synthConfig["capPitchChange"]` `else: capPitchChange = 0`.

`docs/parity/review.md` 106-116 says: "Raised pitch for capitals (not checked by ear): spelling and reading a single character speak an uppercase letter with the pitch setting raised by 30 and then restored, for every synthesizer, as NVDA does by default ([Speech](../nvda/speech.md), "Capitals when spelling"); ... eSpeak NG and OneCore speak the pitch change within one synthesis, as SSML prosody written as NVDA's drivers write it, so a capital brings no pause; a synthesizer that cannot is given the change as a pitch setting between separate synthesis calls, which can leave a short pause." The pinned source contradicts "for every synthesizer, as NVDA does": NVDA gives no pitch change to a synthesizer without `PitchCommand`. History: 43fa6859 ("Raise the pitch of capitals when spelling, as NVDA does", 7 files) built the split; 08fe5b1d ("Speak capitals' raised pitch inside the synthesis, ...", 19 files) moved both drivers inline and kept it. No ruling of Dickson's asks for either fallback (searched `books/`, `docs/`).

Why a question rather than a removal: it narrows the driver contract that M6 turns into the extension synthesizer ABI (a public contract, Dickson's area), and the parity ledger states the pitch behaviour. What goes: `places_marks`, `changes_pitch` from the trait and `HostDescription` (`hosting.rs` 52-66, with its `#[serde(default)]`), their relays (`verbatim-synth-host/src/lib.rs` 160-164, hosted 508-514), `split`, `without_pitch_changes`, `has_pitch_changes`, `set_pitch`, `PITCH` and the split loop's pitch arm; the capture synth then places its marks itself, a tone per text run with `index_reached` between, which keeps its timing as today. About 150 product lines; tests `pipeline.rs` 726-908 (`MarkingTheme`, `MarkPlacingSynth` and two split tests) and 1630-1746 (`PitchSynth`, `a_spelled_capital_is_spoken_at_a_raised_pitch`), the `driver.rs` split test, and the `places_marks` assertions in `verbatim-synth-host/tests/hosting.rs` 135 and 348: about 300 test lines. Speech: unchanged with every registered driver; see the question in section 4.

### F3. Speech observer wrapped twice behind an `Option` that is always `Some` (Remove now, both)

`manager.rs` 348-351: `let events: Option<Arc<dyn SpeechEvents>> = Some(Arc::new(Observer { inner: events, waiters: Arc::clone(&waiters), }));`. Every later holder keeps the `Option`. Grep of `manager.rs`: 16 `if let Some(events)` and 13 `if let Some(inner)`, 29 in all (A counted both, B only the first; both counts are right for what each counted). `Observer` (729-837) forwards each `SpeechEvents` method to its optional `inner`, adding `self.waiters.ended(utterance)` (811). `ShellSpeechEvents` (`speech_events.rs` 37-121) forwards nine methods straight to `self.ledger` and adds three that post to Core. Change: `events` becomes `Arc<dyn SpeechEvents>` (a no-op one when none is configured), or the manager holds a list of observers (waiters, ledger, Core's forwarder) each implementing only what it uses. About 150 product lines. Speech unchanged.

### F4. Per-utterance state kept three times; endings reported from two threads (Planned phase M6.3, both)

`manager.rs` 912-931: `in_flight: Option<Arc<CancelToken>>`, `handed_on: VecDeque<(UtteranceId, Option<FocusValidity>)>`, `marked: HashMap<UtteranceId, (TraceId, Vec<IndexMark>)>`, kept in step by hand: `stop_handed_on` clears `handed_on` (1095) but not `marked`, which empties only in the `Ended` arm (1007-1022). Waiting utterances are a fourth shape (`Waiting`, 265-268), and the mixer's `Tracked` (`verbatim-audio/src/mixer/store.rs` 84-95, `writing: bool`, `started: bool`) a fifth. `utterance_ended` is reported on the mixer's delivery thread (`playback_listener`, 888-898) or on the queue thread (`end_waiting`, 1120-1138), then `marks_cancelled` or `marks_failed` on the queue thread; the app turns them into `QueueInput::UtteranceCancelled` and `UtteranceFailed` (`verbatim-model/src/request.rs` 1384, 1396; routed in `verbatim-core/src/entries.rs` 1257-1264). Commit 7baa1ac6 ("Carry on past a failed utterance, ...") touched 14 files, 278 lines (B's count; A's "9 files" is wrong). The roadmap's M6.3 entry asks for this: "One record per utterance on the speech queue's thread, from waiting to handed on to stopping, and one form of cancel, in place of the copies the queue keeps of what the mixer owns" (`docs/roadmap.md` 1135-1138), built with the `drop_expired` fix (1226-1230). Two shapes were proposed for M6.3's design to weigh: A, one report `utterance_ended(id, ending, unheard_marks)` from the queue thread and one Core input; B, Core numbering its own utterances so the pipeline reports only `Ended(id, ending)` and Core looks up its marks (NVDA has no such report: `speech/speech.py` 217-222 `cancelSpeech` calls `SayAllHandler.stop()` directly, and `speech/manager.py` 768-773 `cancel` resets the index-to-callback table, `_reset` 220-237). B's shape changes Core's inputs. About 250 product lines across speech, app, model and Core. Speech unchanged. The endings-sorting test allowances (section 3) come from the two reporting threads.

### F5. An utterance whose post is refused never ends (Defect, A only; confirmed)

`manager.rs` 471-483: `pub fn speak_for_key(...) -> UtteranceId { let id = mint_utterance(); let _ = inbox::post(&*self.inbox, QueueEvent::Speak(id, Box::new(utterance), key)); id }`; `speak_as` (486-491) is the same. The application's inbox is the completion port (`verbatim-app/src/main.rs` 714, `impl SpeechInbox for PortInbox`), whose `Poster::send` (`crates/verbatim-process/src/port_queue.rs` 126-151) gives the item back when the queue has been dropped, when `PostQueuedCompletionStatus` fails ("which only a system out of memory does"), or under the `PortRefusesPost` fault point. The mixer's delivery thread posts `QueueEvent::Ended` the same way (`manager.rs` 897), so a refused ending leaves `marked` holding the utterance and its marks unreported. The utterance is then never queued and never ended, against D17 ("Each utterance's speech sequence is plain data with an id, ending once, as completed, cancelled or failed", `books/dev/src/explanation/architecture.md` 162-167).

What waits on that ending:
- Say-all (`crates/verbatim-core/src/say_all.rs`): it keeps `HANDED = 2` utterances with speech (68) and moves on when playback passes a mark (`passed`, 619-662, which pops every queued mark up to the one reached). A lost utterance followed by a handed one is passed over silently when the next one's first mark is reached. A lost last utterance, or two lost in a row, leaves `queued` non-empty, so `end_if_done` (696-708) never stops say-all: it reads nothing more and keeps the display on until a key.
- `play_earcon_heard`'s waiters (`manager.rs` 549-583): a refused `speak_as` of an earcon's words leaves `EarconHeard::wait_until` waiting to its deadline, for example the exit sound's wait before Verbatim exits.
- The app itself discards the id (`main.rs` 1583-1585).

Fix: on refusal, report the utterance queued and ended `Failed` to the observers on the calling thread (and its marks failed). About 8 lines; one test through the fault point, about 30 lines. No word spoken changes; a stalled say-all ends. A refusal needs an out-of-memory system or the fault point, so this is rare.

### F6. Dead and test-only API (Remove now; both for the first three, B only for the rest)

Greps over `crates/`, `xtask/`, `uia-facts/`:
- `SpeechManager::paused()` (`manager.rs` 584-590) and `paused_flag` (921, 942, 954, stores at 1003 and 1161): `\.paused()` has 0 matches. About 15 lines.
- `play_earcon_to_end` (528-539): called only by `crates/verbatim-speech/tests/sounds.rs` (394, 429, 449); the app uses `play_earcon_heard` (`main.rs` 1605). With it, `EarconHeard::wait`'s `None` branch (672-679) and the test `waiting_past_the_clocks_range_returns_once_the_sound_has_played_to_its_end`; the other two callers use `play_earcon_heard(..).wait_until(..)`. About 20 product and 25 test lines.
- `SpeechSequence::split_at_marks` (`driver.rs` 168-183): only caller is its test (`driver.rs` 436-462). 15 product and 28 test lines.
- `SynthDriver::display_name` is read only by `choice_of`'s fallback (`manager.rs` 99-105, `registry.choice(&id).unwrap_or_else(|| SynthChoice { display_name: driver.display_name(), id })`, reachable only if a driver reports an id other than its registered one) and relayed by the host (`verbatim-synth-host/src/lib.rs` 162) and hosted driver (472-474). Grep for `\.display_name()` outside tests: those only. The synth thread can keep the id it built (`driver.id()` is also used at 1265, 1355, 1430). About 30 product lines, and a method in each of five test drivers (`pipeline.rs` 259, 796, 1236, 1638; `verbatim-synth-host/tests/protocol.rs` 104), about 15 test lines (B's "eight drivers, 60 lines" overcounts).
- `verbatim_synth_onecore::factory` and `register` (`verbatim-synth-onecore/src/lib.rs` 682-690): no caller (grep `verbatim_synth_onecore::` finds only `OneCoreSynth` and the test's `wait_for_synthesis`). About 10 lines; `onecore_display_name` goes with `display_name`.
- `SynthRegistry::contains` (`registry.rs` 91-95; B's "188 to 192" is wrong, the file has 111 lines): no caller. 5 lines.
Speech unchanged.

### F7. Trailing and inner silence trimmed (Question for Dickson, both)

Verbatim, `crates/verbatim-speech/src/trim.rs` 1-9: "The trimmer drops quiet frames before the first audible frame, and holds back each run of quiet frames after it: a run is released when audible audio follows (so pauses between words and sentences stay), and dropped when the utterance ends." `finish` (97-109): "Ends the utterance: held quiet audio is dropped". `MAX_HELD_SAMPLES: usize = 192_000` (two seconds at 48 kHz stereo).

NVDA trims only leading silence: `nvda/source/nvwave.py` 259-264, `# Enable trimming by default for speech only` `self.enableTrimmingLeadingSilence(purpose is AudioPurpose.SPEECH and config.conf["speech"]["trimLeadingSilence"],)` `if self._enableTrimmingLeadingSilence: self.startTrimmingLeadingSilence()`; `nvdaHelper/local/wasapi.cpp` 344-359 (`if (isTrimmingLeadingSilence && data && size > 0) { size_t silenceSize = SilenceDetect::getLeadingSilenceSize(...)` ... `isTrimmingLeadingSilence = false;  // Stop checking for silence`); `silenceDetect.h` offers only `getLeadingSilenceSize`.

Documents in the repository: the crate guide describes it (`books/dev/src/reference/crates/verbatim-speech.md` 404-412, "dropped when the utterance ends, so trailing silence does not delay the ending or the next utterance") and `verbatim-audio.md` 318 notes its side effect ("The device running dry while an utterance's trimmed trailing silence is held back is not one"); `trim.rs` cites "decision D17", but D17's text (`architecture.md` 162-167) says nothing of silence. `docs/nvda/audio.md` 33-35 records NVDA's as "the trimming of leading silence (`silenceDetect.h`, optional config)", and M4.3 plans NVDA's "trimming leading silence" setting (`docs/roadmap.md` 672). The trim arrived in d5173142 (2026-10-04), whose message says only "Silence is trimmed centrally for every synthesizer". No ruling of Dickson's on the trailing trim exists. Question in section 4.

### F8. The presenter seam exists only for tests (Remove now, B only; confirmed)

`manager.rs` 133-137: `pub presenter: Option<Box<dyn Presenter>>,` documented "`None` selects a [`ThemePresenter`] ... which is what Verbatim uses. A test may present otherwise"; `presenter_or_theme` (628-635). The app passes `presenter: None` (`main.rs` 704). The only other implementations are `MarkingTheme` (`tests/pipeline.rs` 727-745) and `SoundingPresenter` (`tests/sounds.rs` 131-155), which write `SpeechItem::Mark` and `SpeechItem::Sound` by hand where `ThemePresenter` makes them from `SegmentContent::Mark` (`theme.rs` 334-336) and from a sounded indication. Tests through the real presenter with mark segments and a theme sounding `spelling-error` would fail on a misplaced mark or sound; these cannot. About 30 product lines; about 45 test lines replaced. Speech unchanged.

### F9. A hosted synthesizer's setting values are a copy of what Core sent (Planned phase M6, both)

`hosting.rs` 94: `SettingApplied(Option<String>),` carries no values; the host answers with `driver.set_setting(&id, value).err().map(...)` (`verbatim-synth-host/src/lib.rs` 118-123). `HostedSynth::setting` reads `self.description.values` (480-486) and `set_setting` writes the value sent into it on `Ok` (488-505). So the rule in `books/dev/src/explanation/design/ownership.md` 112, "The synth thread alone writes the mirror, reads every value back from the driver after each change and answers with all of them", reads Core's copy for every production synthesizer. Nothing is shown wrongly today: neither engine's `set_setting` clamps or changes another setting (espeak 638-670, onecore 316-343). Validation is written in `host.rs` `validate` (138, used at 222 and `manager.rs` 1425) and again in each driver (`set_percent`, espeak 581, onecore 565; the capture synth's range checks 137-157). Change: `SettingApplied` carries the driver's values, built as `describe` builds them (160-175), and the host validates once against the descriptors. About 40 lines. M6 plans "`HostedSynth`'s view of the host as a pure state" (`docs/roadmap.md` 989-992).

### F10. `Wire` mirrors `FromHost` (Planned phase M6, both)

`hosting.rs` 139-155: `enum Wire` repeats `FromHost`'s eight variants so `Pcm` can carry `samples: (usize, usize)`, documented "always the whole section", written as `(0, size_of_val(samples))` (253) and checked by `pcm_samples` (307-311, `"a PCM frame's samples are not its raw section"`). Serializing `FromHost` with the samples skipped and filled from the raw section removes `Wire`, `put_from_host`'s and `decode_from_host`'s translations (218-304) and the check: about 50 product lines and part of `a_malformed_pcm_frame_is_refused` (442). `HostDescription.changes_pitch`'s `#[serde(default)]` (60-61) accepts a field a same-build host always sends; it goes with F2.

### F11. Sounds travel as index marks with bit 63 set (Planned phase M6.3, both)

`manager.rs` 1541-1543: "The marks standing for sounds have this bit set; the reducer's marks never reach it." `const SOUND_MARK: u64 = 1 << 63;`. Nothing enforces it: `pub struct SpeechMark(pub u64);` (`verbatim-model/src/speech.rs` 194) is copied as `SpeechItem::Mark(IndexMark(mark.0))` (`theme.rs` 336); Core's `allocate_mark` counts up from its `next_mark` (`verbatim-core/src/state.rs` 1093-1097). Encoding: `u64::try_from(sounds.len()).unwrap_or(u64::MAX)` (1555); decoding: `usize::try_from(mark.0 & !SOUND_MARK).unwrap_or(usize::MAX)` then `if let Some(cue) = self.sounds.get(index)` (1620-1621) drops an unknown sound without a word. `SpeechItem::Sound` sits behind `#[serde(skip)]` (`driver.rs` 49) in the serializable sequence. Change: each job numbers its placed items (a mark or a sound) and the driver's sequence type has no `Sound`. About 0 to 20 lines net. M6.3 rewrites `run_job`.

### F12. `had_focus: false` behaves exactly as `None` (Remove now, A only; confirmed)

`FocusValidity::holds` (`verbatim-model/src/speech.rs` 486-502) begins `!self.had_focus ||`, and `holds` is the only reader of a validity (`manager.rs` 1073, 1192-1194; grep for `.validity` outside tests finds no other). Core builds `had_focus: false` for the foreground's title and entered containers (`reduce.rs` 711-715, 748-752). Change: drop the field and give those utterances `None`. About 15 lines (cross-area: model and Core). Speech unchanged.

### F13. The mirror's active synthesizer is optional with a made-up default (Remove now, both)

`host.rs` 178-185: `state.active.clone().unwrap_or_else(|| SynthChoice { id: SynthId::new(""), display_name: String::new(), })`. The synth thread calls `synth.describe()` before `startup_tx.send(Ok(()))` (`manager.rs` 1275-1276), so the default is reachable only by a logic bug. Build the mirror with its first description. About 10 lines.

### F14. `#[non_exhaustive]` inside one workspace (Remove now, both)

`theme.rs` 577 ends `words_of` with `_ => None`, so a new `SegmentContent` variant is silently unspoken; the wildcard exists because of `#[non_exhaustive]` on `SegmentContent` (`verbatim-model/src/speech.rs` 63), also on `TextFormat` (141), `Phrase` (213), `Message` (298). Every user is in this workspace. Removing the attributes turns the miss into a compile error. A few lines.

### F15. Two SSML writers that drift (Remove now as the shape of F1's fix, B only; confirmed)

eSpeak NG's `ssml` and `escape_xml` (`lib.rs` 838-873) and OneCore's `ssml` and `escape` (`lib.rs` 484-537) walk the same items; OneCore's escape removes control characters and quotes, eSpeak NG's only `&`, `<`, `>`. B left open whether eSpeak NG's output changes for U+0001; F1 settles it. One writer and one escape in `verbatim-speech`, with each driver's prosody formula passed in (NVDA's two drivers' formulas differ): about 30 lines.

### F16. Host command state as flags; four cancel paths into a driver (Planned phase M6, both)

`verbatim-synth-host/src/lib.rs` 349-362: `current: Option<UtteranceId>, cancelled: bool, wake: Option<CancelWake>, ended: bool, clean_end: bool`; `cancelled` and `wake` mean something only while `current` is `Some`, `clean_end` only once `ended`. The driver's waker fires only inside `Commands::read` (379-384), so OneCore is also given the host's command event (`src/main.rs` 67, `OneCoreSynth::new()?.interrupted_by(Arc::clone(command_event))`) and polls `sink.is_cancelled()` when it fires (onecore 432-462). M6 plans "The host's command handling as one pure module both host builds share" (`docs/roadmap.md` 989), which should hold `Speaking { utterance, cancel }` and `Pipe { Open, Ended { clean } }`. About 20 lines.

### F17. OneCore's hand-written WAV parser (Planned phase M6.3, B only; confirmed)

`verbatim-synth-onecore/src/lib.rs` 605-680: `read_u32`, `read_u16` (each `.expect("4-byte slice")` / `"2-byte slice"`) and `parse_wav`, beside `hound` in `verbatim-audio/src/sound.rs` 93-109. M6.3 plans "Generated tests for OneCore's WAV and mark parsing (any bytes: no panic and no read past the end)" (`docs/roadmap.md` 1231); reading through `hound` directly (not `Sound::from_wav`, whose length limit would cut long utterances) removes the parser and that planned test. About 75 lines.

### F18. Mixer smaller shapes (Planned phase M6.3)

- `store.rs` 104-121: `Voice { utterance: Option<UtteranceId>, ends: bool, ... }` can hold `utterance: None, ends: true`; an owner enum cannot (B).
- `SpeechLane` stores the registered utterance (`mixer/mod.rs` 711-713) and each write names it again and is refused on a mismatch (B).
- The format generation is named by its low 16 bits (`GENERATION_MASK: u64 = 0xFFFF`, `mod.rs` 206; `recover.rs` 58, `record_generation(audio.generation.wrapping_add(1))`), and `SpeechLane::write` reuses a converter when `active.generation == generation && active.converter.source() == format` (`mod.rs` 766-768): 65,536 reopens between two writes would reuse a converter built for another device rate. Practically unreachable (both).

### F19. A second unsafe kernel-event wrapper (Planned phase M4.9, both)

`verbatim-audio-wasapi/src/lib.rs` 76-118: "An auto-reset event handle, closed on drop", `CreateEventW(None, false, false, PCWSTR::null())` (89), beside `verbatim_process::event::Event`, which is manual-reset (`crates/verbatim-process/src/event.rs` 18-34). An auto-reset constructor there removes about 40 unsafe lines.

### F20. `Utterance::source` read by nothing (Planned phase M15, B only; confirmed)

`verbatim-model/src/speech.rs` 446-448: `pub source: Option<UtteranceSource>,`; Core writes `source: Some(...)` in 14 places outside tests; nothing in `verbatim-speech` or the app reads it (grep for `utterance.source`, `.source.` and `source.role` finds none). Its comment names "milestone M11" for the positional-audio theme; the roadmap's "Positional sound" is M15 (`docs/roadmap.md` 1946). Keep until M15; correct the milestone in the comment.

## 3. Test findings

| Crate | Today | Removable | Replaceable (with what) | To add |
|---|---|---|---|---|
| verbatim-speech | pipeline.rs 27 tests, 1,848 lines; sounds.rs 5, 467; inline 915 lines (theme 604) | Now: `waiting_past_the_clocks_range_returns_once_the_sound_has_played_to_its_end` (about 25, F6); `splitting_at_marks_keeps_each_mark_after_the_text_before_it` (28, F6); test drivers' `display_name` (about 15, F6); `descriptor_accessors_reach_every_variant` (`settings.rs` 171-196, 26 lines: it tests accessors); `a_cancel_ends_current_and_queued_speech` (`pipeline.rs` 991-1009, about 20: both it and `interrupt_cancels_current_and_queued_each_exactly_once` end in `cancel_everything`, `manager.rs` 998 and 1056). If F2 is ruled: `a_synth_that_cannot_place_marks_gets_the_sequence_split_and_marks_stay_exact`, `a_synth_that_places_marks_gets_the_whole_sequence_in_one_call` with `MarkingTheme` and `MarkPlacingSynth` (726-908), `a_spelled_capital_is_spoken_at_a_raised_pitch` with `PitchSynth` (1630-1746): about 300. If F7 is ruled: 2 of `trim.rs`'s 3 tests, about 35 | Now: `MarkingTheme` and `SoundingPresenter` (about 45) by `ThemePresenter` with mark segments and a sounded theme (F8). M6.3: about 20 of pipeline.rs's thread-level tests of lane order, cancels, fences, culling, pauses and settings by tests of the pure queue reducer M6.3 plans, fed recorded orders, keeping about 5 thread-level tests (A's estimate: about 1,100 lines removed) | 2 tests, about 45 lines: a refused post ends the utterance, through `PortRefusesPost` (F5); U+0001 in the eSpeak NG driver's test binary (F1) |
| verbatim-audio | mixer.rs 28 tests, 1,401; inline 16, 411 | 0 | M6.3's stepped driver for about 23 mixer tests, about 400 lines (A) | 0 |
| verbatim-synth-host | hosting.rs 9 tests, 509; protocol.rs 644 | `hosted_espeak_ng_places_a_mark_within_one_synthesis` (343-378, 36 lines) repeats `verbatim-synth-espeak/tests/espeak.rs` `a_sentence_with_marks_is_one_synthesis_with_each_mark_at_its_sample` (same text, same mark at 16,829 samples) through two host processes; the relay of marks is covered by `a_hosted_synthesizer_streams_audio_and_marks_in_order`, and eSpeak NG through the host by three other tests (316, 381, 475) | M6's model of the host pair | 0 |
| verbatim-speech hosting inline | 141 | part of `a_malformed_pcm_frame_is_refused`, about 15, with F10 | | 0 |
| verbatim-synth-espeak | 9 process tests, 526; inline 3, 70 | 0 | 0 | the U+0001 case (counted above) |
| verbatim-synth-onecore | inline 5, 99 | 0 | with F17, the planned generated WAV test is not needed | 0 |
| verbatim-model (speech.rs) | 131 | `text_segment_defaults_to_utterance_language` (648-652, 6 lines: a constructor) | | 0 |

Kept: `standard_numeric_uses_nvda_range_and_steps` (B proposed it): it pins NVDA's range and steps (0 to 100, steps 1, 5, 10), a parity fact.

Allowances:
- `utterances_with_marks_cut_off_are_reported_with_their_marks` sorts the cuts before comparing (`pipeline.rs` 622, `reported.sort_by_key(|(utterance, _, _)| *utterance);`), accepting either order. The order is fixed: the waiting utterance is reported inside `cancel_everything` (`end_waiting`), the playing one only once the mixer's `Ended` reaches the inbox (1007-1022). Assert that order.
- `ControlHarness::endings_before_a_marker` (`pipeline.rs` 462-477) sorts every test's endings by utterance because "The two threads' reports interleave as they run". This follows from F4's two reporting threads; with one reporter, each lane-order test asserts the order exactly.

## 4. Questions for Dickson

### Q1. Remove the capability fallbacks (F2)?

The facts: no registered speaking driver lacks index marks or inline pitch; only the test-audio capture synth lacks marks, and it can place its own. NVDA requires `IndexCommand` of every synthesizer and gives capitals no pitch change when `PitchCommand` is missing (`synthDriverHandler.py` 71-73; `speech/speech.py` 572-575, 618-621, quoted in F2). `docs/parity/review.md` 106-116 says the raised pitch applies "for every synthesizer, as NVDA does", which the pinned source contradicts. The fallbacks shape the driver contract M6 turns into the extension synthesizer ABI.

Sequence, spelling "aBc" (NVDA+Numpad 2 pressed twice on that word), with a future synthesizer that has a pitch setting but cannot change pitch inline (none exists, so no capture):
- Keep (today): "a", then "B" synthesized separately at the pitch setting plus 30, then "c", with a possible short pause around "B".
- Remove (NVDA's rule): "a", "B", "c" at one pitch, no pause.
With eSpeak NG and OneCore both options speak the same.

A ruling for removal also lets the parity ledger's sentence be corrected to NVDA's rule. About 150 product and 300 test lines.

### Q2. Keep dropping trailing and inner silence, or trim only leading silence as NVDA does (F7)?

Sequence: say-all reads two paragraphs as two utterances with eSpeak NG. Words are identical under both options: "first paragraph." then "Second paragraph." Today Verbatim drops the first utterance's end pause, so the second starts sooner than in NVDA; matching NVDA plays it. The gap lengths need a capture (no capture in the repository times them). Matching NVDA removes about 90 product lines and about 35 test lines, and the "device ran dry" side effect noted in `verbatim-audio.md` 318. Keeping it means recording it as a deliberate difference.

## 5. Cost of one behaviour

Checked with `git show --stat`:
- Capital pitch: 43fa6859, 7 files, 270 lines; then 08fe5b1d, 19 files, 355 lines (a `SegmentContent` variant, `theme.rs`, a `SpeechItem` variant, a driver capability flag, a `HostDescription` field, the host's `describe`, the hosted driver, both drivers' SSML, the manager's split, tests at three levels, crate guides).
- A failed utterance's marks counted as reached: 7baa1ac6, 14 files, 278 lines (a `SpeechEvents` method, `Observer` forwarding, the queue thread's `Ended` arm, the app's forwarder, a model input, Core's routing, say-all and terminal, tests in three crates).
- A host's synthesis timing in the latency ledger: ca7875bd, 13 files, 267 lines (`SynthTiming`, a trait method, `Observer` and `PipelineSink`, a `FromHost` variant with its `Wire` variant and two translation arms, the host, the hosted driver, the app's forwarding, the ledger, the control protocol, three documents).
- Sounds in the speech stream: ab2b7c62, 9 files, 1,196 lines.

Shared mechanisms that would cut it, merged:
- One observer list in place of `Observer` and `ShellSpeechEvents`' forwarding (F3): a new event touches the trait and its one consumer.
- One ending report per utterance from one thread, and one Core input for it, or Core's own utterance numbering (F4): a new ending kind adds no trait method, model variant or routing.
- `FromHost` serialized without `Wire` (F10): one variant per host message instead of three arms.
- No capability flags (F2, if ruled): a new synthesizer command costs its `SpeechItem` variant and the drivers that speak it, as NVDA's drivers ignore commands they do not support.
- One SSML writer (F15): a new item reaches both drivers in one place.
With these, a new milestone or ending kind touches 3 to 4 files instead of 9 to 19.

## 6. Dropped

- B: `theme.rs` 189-193, `self.settings.get(&indication).unwrap_or(&SPEECH)` as reachable only by a logic bug: not confirmed. `Indication::catalogue` (`verbatim-model/src/theme.rs` 206-217) lists only `SPOKEN_STATES`' states, so whether a `State` indication outside it reaches `setting()` was not settled.
- B: the crate guide's "Decoding a chunk of 1,323 samples ... takes about 0.2 microseconds" (`verbatim-speech.md` 460-461) as a figure a test pins again: no test pins it (grep `1_323|1323` over the area's crates: 0).
- B: "test drivers' `id` and `display_name` in eight drivers, about 60 lines": five test drivers exist; kept in F6 with the corrected count.
- A: commit 7baa1ac6 "touched 9 files": it touched 14; corrected in F4.
- A's cross-area note that `port_queue` posts `Box::into_raw` pointers through `PostQueuedCompletionStatus` (`crates/verbatim-process/src/port_queue.rs` 135-145) is true but belongs to `verbatim-process`'s area, not this one; left for that area's review.
