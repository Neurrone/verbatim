//! The eSpeak NG driver against the real synthesizer and its data. One test
//! function, since eSpeak NG allows one driver per process.

use std::ops::ControlFlow;

use verbatim_audio::PcmFormat;
use verbatim_model::{TraceId, UtteranceId};
use verbatim_speech::{
    IndexMark, SettingId, SettingValue, SpeechItem, SpeechSequence, SynthDriver, SynthSink,
};
use verbatim_synth_espeak::EspeakSynth;

/// Counts the audio pushed, and stops after `stop_after` pushes.
struct Count {
    pushes: usize,
    samples: usize,
    format: Option<PcmFormat>,
    stop_after: usize,
}

impl SynthSink for Count {
    fn push_pcm(&mut self, format: PcmFormat, samples: &[i16]) -> ControlFlow<()> {
        self.pushes += 1;
        self.samples += samples.len();
        self.format = Some(format);
        if self.pushes >= self.stop_after {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    }

    fn index_reached(&mut self, _mark: IndexMark) {}

    fn is_cancelled(&self) -> bool {
        false
    }
}

fn sequence(text: &str) -> SpeechSequence {
    SpeechSequence {
        utterance: UtteranceId(1),
        trace_id: TraceId::mint(),
        language: None,
        items: vec![SpeechItem::Text(text.to_owned())],
    }
}

#[test]
fn speaks_english_stops_when_asked_and_takes_its_settings() {
    let mut synth = EspeakSynth::new().expect("eSpeak NG starts with its built data");
    assert!(
        !synth.places_marks(),
        "marks are placed by splitting the sequence"
    );
    assert!(
        EspeakSynth::new().is_err(),
        "eSpeak NG keeps global state, so a process has one driver"
    );

    let mut whole = Count {
        pushes: 0,
        samples: 0,
        format: None,
        stop_after: usize::MAX,
    };
    synth
        .speak(
            &sequence("The quick brown fox jumps over the lazy dog."),
            &mut whole,
        )
        .expect("speaks");
    assert_eq!(
        whole.format,
        Some(PcmFormat {
            sample_rate: 22_050,
            channels: 1
        })
    );
    // About two and a half seconds of speech.
    assert!(
        whole.samples > 22_050,
        "speech was produced: {} samples",
        whole.samples
    );
    assert!(whole.pushes > 2, "in several chunks");

    let mut cut = Count {
        pushes: 0,
        samples: 0,
        format: None,
        stop_after: 1,
    };
    synth
        .speak(
            &sequence("The quick brown fox jumps over the lazy dog."),
            &mut cut,
        )
        .expect("a cancelled utterance is not a failure");
    assert_eq!(
        cut.pushes, 1,
        "synthesis stops at the push that asked it to"
    );

    synth
        .set_setting(&SettingId::new("rate"), SettingValue::Number(100))
        .expect("sets the rate");
    let mut fast = Count {
        pushes: 0,
        samples: 0,
        format: None,
        stop_after: usize::MAX,
    };
    synth
        .speak(
            &sequence("The quick brown fox jumps over the lazy dog."),
            &mut fast,
        )
        .expect("speaks");
    assert!(
        fast.samples < whole.samples,
        "a faster rate gives shorter speech"
    );
    assert!(
        synth
            .set_setting(
                &SettingId::new("voice"),
                SettingValue::Choice("no such voice".to_owned())
            )
            .is_err()
    );

    speaks_a_raised_capital_within_one_synthesis(&mut synth);
}

/// A capital spoken at a raised pitch within one synthesis: the markup is
/// obeyed, not read aloud, so it takes about as long as the letter.
fn speaks_a_raised_capital_within_one_synthesis(synth: &mut EspeakSynth) {
    assert!(synth.changes_pitch());
    let count = |synth: &mut EspeakSynth, items: Vec<SpeechItem>| {
        let mut count = Count {
            pushes: 0,
            samples: 0,
            format: None,
            stop_after: usize::MAX,
        };
        let sequence = SpeechSequence {
            items,
            ..sequence("")
        };
        synth.speak(&sequence, &mut count).expect("speaks");
        count.samples
    };
    let plain = count(synth, vec![SpeechItem::Text("B".to_owned())]);
    let raised = count(
        synth,
        vec![
            SpeechItem::Pitch(30),
            SpeechItem::Text("B".to_owned()),
            SpeechItem::Pitch(0),
        ],
    );
    assert!(plain > 0);
    assert!(
        raised < plain * 3 / 2 && raised > plain / 2,
        "the pitch markup is not spoken: {raised} samples against {plain}"
    );
}
