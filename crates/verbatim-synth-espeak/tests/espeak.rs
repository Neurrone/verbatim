//! The eSpeak NG driver against the real synthesizer and its data.
//!
//! eSpeak NG keeps its state in globals, which run on from one utterance
//! to the next and survive a driver being dropped and another made, so an
//! utterance spoken twice in one process need not give the same samples.
//! A new process does: the driver seeds eSpeak NG's noise, so its first
//! utterance depends on nothing but the text and the settings. Tests that
//! compare speech exactly therefore have each utterance spoken by a
//! process of its own: this test binary, run again with
//! `--speak <case>`, which speaks one [`Case`] with a new driver and
//! writes what it heard to its standard output. The other tests run here,
//! one after another, since a process has one driver at a time.
//!
//! The binary has its own small runner (`harness = false` in
//! `Cargo.toml`), which prints libtest's lines, runs the tests whose names
//! contain the first argument that is not an option, and exits with
//! libtest's code when one fails.

use std::io::{Read, Write};
use std::ops::ControlFlow;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::{Command, Stdio};

use verbatim_audio::PcmFormat;
use verbatim_model::{TraceId, UtteranceId};
use verbatim_speech::{
    IndexMark, SettingId, SettingValue, SpeechItem, SpeechSequence, SynthDriver, SynthError,
    SynthSink,
};
use verbatim_synth_espeak::EspeakSynth;

/// The sentence the tests speak.
const SENTENCE: &str = "The quick brown fox jumps over the lazy dog.";

/// The sentence's first half, up to where a mark is placed within it.
const FIRST_HALF: &str = "The quick brown fox ";

/// The sentence's second half.
const SECOND_HALF: &str = "jumps over the lazy dog.";

/// The first of two sentences, with the space after it.
const FIRST_SENTENCE: &str = "One sentence. ";

/// The second of two sentences.
const SECOND_SENTENCE: &str = "Another.";

/// The samples in one of the driver's 60 millisecond chunks at 22,050 Hz:
/// eSpeak NG sizes its buffer as 60 times 22,050 thousandths of a sample,
/// rounded up to the next whole thousand, which is 1,324 samples.
const CHUNK_SAMPLES: usize = 1_324;

/// The option that makes this binary speak one case and exit.
const SPEAK: &str = "--speak";

/// One utterance a process of its own speaks with a new driver.
#[derive(Clone, Copy, Debug)]
enum Case {
    /// The sentence with the default settings.
    Sentence,
    /// The sentence, stopped by the sink at the first push.
    SentenceStoppedAtTheFirstPush,
    /// The sentence at a rate of 100.
    SentenceAtRate100,
    /// The sentence after the rate was set to 100 and back to 50.
    SentenceAfterTheRateWasSetBack,
    /// The letter B with the default settings.
    B,
    /// The letter B in a sequence with a pitch item that changes nothing,
    /// which is spoken as SSML.
    BAsSsml,
    /// The letter B raised by 30, as a capital is, then the pitch reset.
    RaisedB,
    /// The SSML a raised B is spoken as, given as plain text: what the
    /// markup would sound like read aloud.
    RaisedBMarkupAsText,
    /// The sentence with a mark before it, one where its second half
    /// starts, as say-all's second line starts within a sentence, and one
    /// after it.
    SentenceWithMarks,
    /// Two sentences.
    TwoSentences,
    /// The two sentences with a mark after the first one's full stop.
    MarkAfterAFullStop,
}

impl Case {
    const ALL: [Self; 11] = [
        Self::Sentence,
        Self::SentenceStoppedAtTheFirstPush,
        Self::SentenceAtRate100,
        Self::SentenceAfterTheRateWasSetBack,
        Self::B,
        Self::BAsSsml,
        Self::RaisedB,
        Self::RaisedBMarkupAsText,
        Self::SentenceWithMarks,
        Self::TwoSentences,
        Self::MarkAfterAFullStop,
    ];

    fn name(self) -> String {
        format!("{self:?}")
    }

    /// Speaks the case with `synth`, a new driver.
    fn speak(self, synth: &mut EspeakSynth) -> Record {
        let text = |text: &str| vec![SpeechItem::Text(text.to_owned())];
        let raised_b = || {
            vec![
                SpeechItem::Pitch(30),
                SpeechItem::Text("B".to_owned()),
                SpeechItem::Pitch(0),
            ]
        };
        match self {
            Self::Sentence => speak(synth, text(SENTENCE)),
            Self::SentenceStoppedAtTheFirstPush => {
                let mut record = Record::stopping_after(1);
                synth
                    .speak(&sequence(text(SENTENCE)), &mut record)
                    .expect("a stopped utterance is not a failure");
                record
            }
            Self::SentenceAtRate100 => {
                set(synth, "rate", 100);
                speak(synth, text(SENTENCE))
            }
            Self::SentenceAfterTheRateWasSetBack => {
                set(synth, "rate", 100);
                set(synth, "rate", 50);
                speak(synth, text(SENTENCE))
            }
            Self::B => speak(synth, text("B")),
            Self::BAsSsml => speak(
                synth,
                vec![SpeechItem::Pitch(0), SpeechItem::Text("B".to_owned())],
            ),
            Self::RaisedB => speak(synth, raised_b()),
            Self::RaisedBMarkupAsText => speak(synth, text("<prosody pitch=\"160%\">B</prosody>")),
            Self::SentenceWithMarks => speak(
                synth,
                vec![
                    SpeechItem::Mark(IndexMark(1)),
                    SpeechItem::Text(FIRST_HALF.to_owned()),
                    SpeechItem::Mark(IndexMark(2)),
                    SpeechItem::Text(SECOND_HALF.to_owned()),
                    SpeechItem::Mark(IndexMark(3)),
                ],
            ),
            Self::TwoSentences => speak(synth, text(&format!("{FIRST_SENTENCE}{SECOND_SENTENCE}"))),
            Self::MarkAfterAFullStop => speak(
                synth,
                vec![
                    SpeechItem::Text(FIRST_SENTENCE.to_owned()),
                    SpeechItem::Mark(IndexMark(7)),
                    SpeechItem::Text(SECOND_SENTENCE.to_owned()),
                ],
            ),
        }
    }
}

/// Keeps every push of audio and every mark, and stops after `stop_after`
/// pushes.
struct Record {
    pushes: Vec<Vec<i16>>,
    /// Each mark reported, with the number of samples pushed before it.
    marks: Vec<(usize, IndexMark)>,
    format: Option<PcmFormat>,
    stop_after: usize,
}

impl Record {
    fn stopping_after(stop_after: usize) -> Self {
        Self {
            pushes: Vec::new(),
            marks: Vec::new(),
            format: None,
            stop_after,
        }
    }

    fn samples(&self) -> Vec<i16> {
        self.pushes.concat()
    }

    /// Writes the format, the pushes, and the marks: the sample rate, the
    /// channels, and the number of pushes, then each push's length and
    /// samples, then the number of marks and each one's position and
    /// number, all as little-endian 32-bit counts, 16-bit samples, and
    /// 64-bit mark numbers.
    fn write(&self, out: &mut impl Write) {
        let format = self.format.expect("speech was produced");
        let count = |n: usize| u32::try_from(n).expect("fits").to_le_bytes();
        let mut bytes = Vec::new();
        bytes.extend(format.sample_rate.to_le_bytes());
        bytes.extend(u32::from(format.channels).to_le_bytes());
        bytes.extend(count(self.pushes.len()));
        for push in &self.pushes {
            bytes.extend(count(push.len()));
            bytes.extend(push.iter().flat_map(|sample| sample.to_le_bytes()));
        }
        bytes.extend(count(self.marks.len()));
        for (position, mark) in &self.marks {
            bytes.extend(count(*position));
            bytes.extend(mark.0.to_le_bytes());
        }
        out.write_all(&bytes).expect("writes the speech");
    }

    /// Reads what [`Record::write`] wrote.
    fn read(bytes: &[u8]) -> Self {
        let mut rest = bytes;
        let mut take = |n: usize| -> Vec<u8> {
            let mut taken = vec![0; n];
            rest.read_exact(&mut taken).expect("as much as was written");
            taken
        };
        let word = |take: &mut dyn FnMut(usize) -> Vec<u8>| {
            u32::from_le_bytes(take(4).try_into().expect("four bytes"))
        };
        let sample_rate = word(&mut take);
        let channels = u16::try_from(word(&mut take)).expect("a channel count");
        let pushes = (0..word(&mut take))
            .map(|_| {
                let length = usize::try_from(word(&mut take)).expect("fits");
                take(length * 2)
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| i16::from_le_bytes(*pair))
                    .collect()
            })
            .collect();
        let marks = (0..word(&mut take))
            .map(|_| {
                let position = usize::try_from(word(&mut take)).expect("fits");
                let number = u64::from_le_bytes(take(8).try_into().expect("eight bytes"));
                (position, IndexMark(number))
            })
            .collect();
        assert!(rest.is_empty(), "nothing follows the last mark");
        Self {
            pushes,
            marks,
            format: Some(PcmFormat {
                sample_rate,
                channels,
            }),
            stop_after: usize::MAX,
        }
    }
}

impl SynthSink for Record {
    fn push_pcm(&mut self, format: PcmFormat, samples: &[i16]) -> ControlFlow<()> {
        self.pushes.push(samples.to_vec());
        assert!(
            self.format.is_none_or(|known| known == format),
            "one format for a whole utterance"
        );
        self.format = Some(format);
        if self.pushes.len() >= self.stop_after {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }

    fn index_reached(&mut self, mark: IndexMark) {
        self.marks
            .push((self.pushes.iter().map(Vec::len).sum(), mark));
    }

    fn is_cancelled(&self) -> bool {
        false
    }
}

fn sequence(items: Vec<SpeechItem>) -> SpeechSequence {
    SpeechSequence {
        utterance: UtteranceId(1),
        trace_id: TraceId::mint(),
        language: None,
        items,
    }
}

/// Sets `id` to the number `value`.
fn set(synth: &mut EspeakSynth, id: &str, value: i32) {
    synth
        .set_setting(&SettingId::new(id), SettingValue::Number(value))
        .expect("sets the setting");
}

/// Everything `items` is spoken as.
fn speak(synth: &mut EspeakSynth, items: Vec<SpeechItem>) -> Record {
    let mut record = Record::stopping_after(usize::MAX);
    synth.speak(&sequence(items), &mut record).expect("speaks");
    record
}

fn new_driver() -> EspeakSynth {
    EspeakSynth::new().expect("eSpeak NG starts with its built data")
}

/// `case` as a process of its own speaks it.
fn spoken(case: Case) -> Record {
    let mut child = Command::new(std::env::current_exe().expect("this test binary"))
        .args([SPEAK, &case.name()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .expect("starts a process to speak");
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .expect("its output")
        .read_to_end(&mut bytes)
        .expect("reads its speech");
    let status = child.wait().expect("waits for it");
    assert!(status.success(), "{case:?} was spoken: {status}");
    Record::read(&bytes)
}

fn a_process_has_one_driver_which_places_marks_and_changes_pitch() {
    let synth = new_driver();
    assert_eq!(
        EspeakSynth::new().err(),
        Some(SynthError::Unavailable(
            "this process already has an eSpeak NG driver".to_owned()
        )),
        "eSpeak NG keeps global state, so a process has one driver"
    );
    assert!(
        synth.places_marks(),
        "eSpeak NG reports where each mark falls in its audio"
    );
    assert!(synth.changes_pitch());
}

fn an_unknown_voice_is_refused_and_the_voice_kept() {
    let mut synth = new_driver();
    assert_eq!(
        synth.set_setting(
            &SettingId::new("voice"),
            SettingValue::Choice("no such voice".to_owned())
        ),
        Err(SynthError::Setting(
            "unknown voice no such voice".to_owned()
        ))
    );
    assert_eq!(
        synth.setting(&SettingId::new("voice")),
        Some(SettingValue::Choice("gmw/en".to_owned()))
    );
}

fn speech_comes_at_22050_hz_mono_in_60_millisecond_chunks() {
    let whole = spoken(Case::Sentence);
    assert_eq!(
        whole.format,
        Some(PcmFormat {
            sample_rate: 22_050,
            channels: 1
        })
    );
    let (last, full) = whole.pushes.split_last().expect("speech was produced");
    let lengths: Vec<usize> = whole.pushes.iter().map(Vec::len).collect();
    assert!(
        full.iter().all(|push| push.len() == CHUNK_SAMPLES),
        "every chunk but the last is full: {lengths:?}"
    );
    assert!((1..=CHUNK_SAMPLES).contains(&last.len()), "{lengths:?}");
    // About one and three quarter seconds of speech.
    assert!(full.len() > 20, "{lengths:?}");
}

fn the_same_utterance_with_the_same_settings_gives_the_same_speech() {
    assert_eq!(spoken(Case::Sentence).pushes, spoken(Case::Sentence).pushes);
}

fn synthesis_stops_at_the_push_that_asks_it_to() {
    let whole = spoken(Case::Sentence);
    assert_eq!(
        spoken(Case::SentenceStoppedAtTheFirstPush).pushes,
        whole.pushes[..1],
        "synthesis stops after the first chunk, the push that asked it to"
    );
}

fn the_rate_setting_changes_the_speech_until_it_is_set_back() {
    let usual = spoken(Case::Sentence).samples();
    let fast = spoken(Case::SentenceAtRate100).samples();
    assert!(
        fast.len() < usual.len(),
        "a faster rate gives shorter speech: {} samples against {}",
        fast.len(),
        usual.len()
    );
    assert_eq!(
        spoken(Case::SentenceAfterTheRateWasSetBack).samples(),
        usual
    );
}

/// A capital spoken at a raised pitch within one synthesis: the markup is
/// obeyed, neither ignored nor read aloud. Spoken as SSML, a B with no
/// change of pitch is the B spoken as plain text, sample for sample, so
/// the SSML itself changes nothing; the raised B differs from it, and is
/// shorter than its markup read aloud.
fn a_raised_capital_is_spoken_at_a_raised_pitch_within_one_synthesis() {
    let plain = spoken(Case::B).samples();
    assert_eq!(spoken(Case::BAsSsml).samples(), plain);
    let raised = spoken(Case::RaisedB).samples();
    assert_ne!(raised, plain, "the pitch markup changes the speech");
    let read_aloud = spoken(Case::RaisedBMarkupAsText).samples();
    assert!(
        raised.len() < read_aloud.len(),
        "the markup is not read aloud: {} samples against {}",
        raised.len(),
        read_aloud.len()
    );
}

/// A sentence with a mark where its second half starts is spoken in one
/// synthesis, as NVDA's driver speaks it: its audio is the unmarked
/// sentence's, sample for sample, which a synthesis divided at the mark is
/// not (the first half ends as a sentence does, longer and falling, and the
/// second starts its intonation again). eSpeak NG reports each mark at its
/// sample: the first before any audio, the second where "jumps" starts,
/// and the last after "dog", before the synthesis's last 110 samples.
fn a_sentence_with_marks_is_one_synthesis_with_each_mark_at_its_sample() {
    let marked = spoken(Case::SentenceWithMarks);
    assert_eq!(
        marked.marks,
        [
            (0, IndexMark(1)),
            (16_829, IndexMark(2)),
            (37_945, IndexMark(3))
        ]
    );
    assert!(
        marked.samples() == spoken(Case::Sentence).samples(),
        "the marks change nothing that is heard"
    );
}

/// A mark after a full stop and a space, which eSpeak NG drops within one
/// synthesis, is reported where the next sentence starts, after the
/// sentence pause, and the speech is the unmarked two sentences', sample
/// for sample, the pause included.
fn a_mark_after_a_full_stop_is_reported_where_the_next_sentence_starts() {
    let marked = spoken(Case::MarkAfterAFullStop);
    assert_eq!(marked.marks, [(15_259, IndexMark(7))]);
    assert!(
        marked.samples() == spoken(Case::TwoSentences).samples(),
        "the mark changes nothing that is heard"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [option, name] = args.as_slice()
        && option == SPEAK
    {
        let case = Case::ALL
            .into_iter()
            .find(|case| case.name() == *name)
            .expect("a known case");
        let record = case.speak(&mut new_driver());
        record.write(&mut std::io::stdout().lock());
        return;
    }
    let filter = args.iter().find(|arg| !arg.starts_with('-'));
    let tests: [(&str, fn()); 9] = [
        (
            "a_process_has_one_driver_which_places_marks_and_changes_pitch",
            a_process_has_one_driver_which_places_marks_and_changes_pitch,
        ),
        (
            "an_unknown_voice_is_refused_and_the_voice_kept",
            an_unknown_voice_is_refused_and_the_voice_kept,
        ),
        (
            "speech_comes_at_22050_hz_mono_in_60_millisecond_chunks",
            speech_comes_at_22050_hz_mono_in_60_millisecond_chunks,
        ),
        (
            "the_same_utterance_with_the_same_settings_gives_the_same_speech",
            the_same_utterance_with_the_same_settings_gives_the_same_speech,
        ),
        (
            "synthesis_stops_at_the_push_that_asks_it_to",
            synthesis_stops_at_the_push_that_asks_it_to,
        ),
        (
            "the_rate_setting_changes_the_speech_until_it_is_set_back",
            the_rate_setting_changes_the_speech_until_it_is_set_back,
        ),
        (
            "a_raised_capital_is_spoken_at_a_raised_pitch_within_one_synthesis",
            a_raised_capital_is_spoken_at_a_raised_pitch_within_one_synthesis,
        ),
        (
            "a_sentence_with_marks_is_one_synthesis_with_each_mark_at_its_sample",
            a_sentence_with_marks_is_one_synthesis_with_each_mark_at_its_sample,
        ),
        (
            "a_mark_after_a_full_stop_is_reported_where_the_next_sentence_starts",
            a_mark_after_a_full_stop_is_reported_where_the_next_sentence_starts,
        ),
    ];
    let selected: Vec<_> = tests
        .iter()
        .filter(|(name, _)| filter.is_none_or(|filter| name.contains(filter.as_str())))
        .collect();
    println!("\nrunning {} tests", selected.len());
    let mut failed = Vec::new();
    for &&(name, test) in &selected {
        let passed = catch_unwind(AssertUnwindSafe(test)).is_ok();
        println!("test {name} ... {}", if passed { "ok" } else { "FAILED" });
        if !passed {
            failed.push(name);
        }
    }
    println!(
        "\ntest result: {}. {} passed; {} failed; 0 ignored; 0 measured; {} filtered out\n",
        if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failed.len(),
        failed.len(),
        tests.len() - selected.len()
    );
    if !failed.is_empty() {
        for name in failed {
            println!("failed: {name}");
        }
        std::process::exit(101);
    }
}
