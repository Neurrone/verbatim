//! Sounds in the speech pipeline (`phase6-design.md`, "Earcons"): a sound
//! item placed in the speech stream reaches the mixer at its place, between
//! the words around it, without the synthesizer ever seeing it; the queued
//! text names it; and an event with no sound to play is spoken instead.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use verbatim_audio::{AudioTap, DeviceFormat, Mixer, PcmFormat, SilentDevice, Sound};
use verbatim_model::{
    Earcon, SegmentContent, SpeechPriority, TraceId, Utterance, UtteranceEnding, UtteranceId,
    UtteranceSegment,
};
use verbatim_speech::{
    IndexMark, Presenter, SoundCue, SpeechEvents, SpeechItem, SpeechManager, SpeechManagerConfig,
    SpeechSequence, SynthDriver, SynthId, SynthRegistry,
};
use verbatim_synth_capture::{CaptureLog, CaptureSynth};

const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// The level of the test's sound, far above the capture synth's tone.
const LOUD: i16 = 30_000;

/// How many frames the test's sound lasts, at the silent device's rate.
const SOUND_FRAMES: usize = 480;

#[derive(Default)]
struct Recorder {
    queued: Mutex<Vec<String>>,
    marks: Mutex<Vec<IndexMark>>,
    endings: Mutex<Vec<(UtteranceId, UtteranceEnding)>>,
}

impl SpeechEvents for Recorder {
    fn utterance_queued(&self, _: UtteranceId, _: TraceId, text: &str, _: Instant) {
        self.queued.lock().unwrap().push(text.to_owned());
    }

    fn audio_started(&self, _: UtteranceId, _: TraceId, _: Instant) {}

    fn mark_reached(&self, _: UtteranceId, _: TraceId, mark: IndexMark, _: Instant) {
        self.marks.lock().unwrap().push(mark);
    }

    fn utterance_ended(
        &self,
        utterance: UtteranceId,
        _: TraceId,
        ending: &UtteranceEnding,
        _: Instant,
    ) {
        self.endings
            .lock()
            .unwrap()
            .push((utterance, ending.clone()));
    }
}

/// Keeps one channel of everything the mixer played.
#[derive(Clone, Default)]
struct Tap {
    frames: Arc<Mutex<Vec<f32>>>,
}

impl AudioTap for Tap {
    fn played(&mut self, samples: &[f32], format: DeviceFormat) {
        let channels = usize::from(format.channels);
        self.frames
            .lock()
            .unwrap()
            .extend(samples.iter().step_by(channels));
    }
}

/// Speaks each text segment, with the sound between the first and the
/// rest, as a theme places a sound for a span.
struct SoundingPresenter {
    sound: Arc<Sound>,
}

impl Presenter for SoundingPresenter {
    fn flatten(&self, utterance: &Utterance, id: UtteranceId) -> SpeechSequence {
        let mut items = Vec::new();
        for (index, segment) in utterance.segments.iter().enumerate() {
            if index == 1 {
                items.push(SpeechItem::Sound(SoundCue {
                    indication: "spelling-error".to_owned(),
                    sound: Arc::clone(&self.sound),
                    gain: 1.0,
                }));
            }
            if let SegmentContent::Text(text) = &segment.content {
                items.push(SpeechItem::Text(text.clone()));
            }
        }
        SpeechSequence {
            utterance: id,
            trace_id: utterance.trace_id,
            language: None,
            items,
        }
    }
}

struct Harness {
    manager: SpeechManager,
    log: CaptureLog,
    recorder: Arc<Recorder>,
    tap: Tap,
}

fn harness(presenter: Option<Box<dyn Presenter>>) -> Harness {
    let log: CaptureLog = CaptureSynth::new().log();
    let log_for_factory = Arc::clone(&log);
    let mut registry = SynthRegistry::new();
    registry.register(
        SynthId::new("capture"),
        "Capture synth",
        Box::new(move || {
            Ok(
                Box::new(CaptureSynth::with_log(Arc::clone(&log_for_factory)))
                    as Box<dyn SynthDriver>,
            )
        }),
    );
    let tap = Tap::default();
    let mixer = Mixer::start_with_tap(Box::new(SilentDevice::new()), Box::new(tap.clone()))
        .expect("the silent device opens");
    let recorder = Arc::new(Recorder::default());
    let manager = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new("capture"),
        saved_settings: Box::new(|_| Vec::new()),
        mixer: Arc::new(mixer),
        events: Some(Arc::clone(&recorder) as Arc<dyn SpeechEvents>),
        theme: presenter,
    })
    .expect("pipeline starts");
    Harness {
        manager,
        log,
        recorder,
        tap,
    }
}

/// Polls `done` until it holds or the step times out.
fn wait_for(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + STEP_TIMEOUT;
    while !done() {
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    true
}

#[test]
fn a_sound_plays_at_its_place_between_the_words_and_the_synth_never_sees_it() {
    let format = PcmFormat {
        sample_rate: 48_000,
        channels: 1,
    };
    let sound = Arc::new(Sound::from_pcm(format, vec![LOUD; SOUND_FRAMES]).expect("a sound"));
    let harness = harness(Some(Box::new(SoundingPresenter { sound })));
    let id = harness.manager.speak(Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Queued,
        segments: vec![
            UtteranceSegment::text("the"),
            UtteranceSegment::text("wrold"),
        ],
        source: None,
        validity: None,
    });

    let loud = |frames: &[f32]| frames.iter().filter(|frame| frame.abs() > 0.8).count();
    assert!(
        wait_for(|| loud(&harness.tap.frames.lock().unwrap()) >= SOUND_FRAMES),
        "the sound plays whole"
    );
    assert!(wait_for(|| !harness
        .recorder
        .endings
        .lock()
        .unwrap()
        .is_empty()));
    assert_eq!(
        *harness.recorder.endings.lock().unwrap(),
        [(id, UtteranceEnding::Completed)]
    );
    assert_eq!(
        *harness.recorder.queued.lock().unwrap(),
        ["the sound: spelling-error wrold"],
        "the queued text names the sound in its place"
    );

    // The synthesizer was asked for the words on either side of the sound,
    // and never for the sound or the mark standing for it.
    let synthesized: Vec<Vec<SpeechItem>> = harness
        .log
        .lock()
        .unwrap()
        .iter()
        .map(|record| record.sequence.items.clone())
        .collect();
    assert_eq!(
        synthesized,
        [
            vec![SpeechItem::Text("the".to_owned())],
            vec![SpeechItem::Text("wrold".to_owned())]
        ]
    );
    assert_eq!(*harness.recorder.marks.lock().unwrap(), []);

    // The sound starts after the first word's audio.
    let frames = harness.tap.frames.lock().unwrap();
    let first_loud = frames
        .iter()
        .position(|frame| frame.abs() > 0.8)
        .expect("the sound played");
    assert!(
        frames[..first_loud].iter().any(|frame| *frame != 0.0),
        "the first word was heard before the sound"
    );
}

#[test]
fn an_event_without_a_sound_to_play_is_spoken_instead() {
    // The manager's own theme starts as the default theme with no sounds
    // loaded, so browse mode, a sound in the default theme, is spoken.
    let harness = harness(None);
    harness.manager.play_earcon(Earcon::BrowseMode);
    assert!(wait_for(|| !harness
        .recorder
        .endings
        .lock()
        .unwrap()
        .is_empty()));
    assert_eq!(*harness.recorder.queued.lock().unwrap(), ["browse mode"]);
}
