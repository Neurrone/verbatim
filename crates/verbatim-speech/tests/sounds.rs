//! Sounds in the speech pipeline (`phase6-design.md`, "Earcons"): a sound
//! item placed in the speech stream reaches the mixer at its place, between
//! the words around it, without the synthesizer ever seeing it; the queued
//! text names it; and an event with no sound to play is spoken instead.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use verbatim_audio::{AudioTap, DeviceFormat, Mixer, PcmFormat, SilentDevice, Sound};
use verbatim_model::{
    Earcon, Indication, SegmentContent, SpeechPriority, Theme, ThemeOptions, TraceId, Utterance,
    UtteranceEnding, UtteranceId, UtteranceSegment,
};
use verbatim_speech::{
    ActiveTheme, IndexMark, Presenter, SoundCue, SpeechEvents, SpeechItem, SpeechManager,
    SpeechManagerConfig, SpeechSequence, SynthDriver, SynthId, SynthRegistry,
};
use verbatim_synth_capture::{CaptureLog, CaptureSynth};

const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// The level of the test's sound, far above the capture synth's tone.
const LOUD: i16 = 30_000;

/// How many frames the test's sound lasts, at the silent device's rate.
const SOUND_FRAMES: usize = 480;

/// A value the pipeline's threads change and a test waits on.
#[derive(Default)]
struct Watched<T> {
    value: Mutex<T>,
    changed: Condvar,
}

impl<T: Clone> Watched<T> {
    fn update(&self, change: impl FnOnce(&mut T)) {
        change(&mut self.value.lock().unwrap());
        self.changed.notify_all();
    }

    /// Waits until `done` holds of the value, then returns it. Fails, naming
    /// `what` it waited for, at [`STEP_TIMEOUT`].
    fn wait_until(&self, what: &str, done: impl Fn(&T) -> bool) -> T {
        let (value, timeout) = self
            .changed
            .wait_timeout_while(self.value.lock().unwrap(), STEP_TIMEOUT, |value| {
                !done(value)
            })
            .unwrap();
        assert!(!timeout.timed_out(), "waited {STEP_TIMEOUT:?} for {what}");
        value.clone()
    }

    fn get(&self) -> T {
        self.value.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct Recorder {
    queued: Mutex<Vec<String>>,
    marks: Mutex<Vec<IndexMark>>,
    endings: Watched<Vec<(UtteranceId, UtteranceEnding)>>,
    sounds: Mutex<Vec<Indication>>,
}

impl Recorder {
    /// Waits until `id` has ended, then returns every ending so far.
    fn endings_through(&self, id: UtteranceId) -> Vec<(UtteranceId, UtteranceEnding)> {
        self.endings
            .wait_until(&format!("the ending of {id:?}"), |endings| {
                endings.iter().any(|(utterance, _)| *utterance == id)
            })
    }
}

impl SpeechEvents for Recorder {
    fn sound_played(&self, indication: Indication, _: Instant) {
        self.sounds.lock().unwrap().push(indication);
    }

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
            .update(|endings| endings.push((utterance, ending.clone())));
    }
}

/// Keeps one channel of everything the mixer played.
#[derive(Clone, Default)]
struct Tap {
    frames: Arc<Watched<Vec<f32>>>,
}

impl AudioTap for Tap {
    fn played(&mut self, samples: &[f32], format: DeviceFormat) {
        let channels = usize::from(format.channels);
        self.frames
            .update(|frames| frames.extend(samples.iter().step_by(channels)));
    }
}

/// Whether a frame is the test's sound, far above any speech.
fn is_loud(frame: f32) -> bool {
    frame.abs() > 0.8
}

fn loud_count(frames: &[f32]) -> usize {
    frames.iter().filter(|frame| is_loud(**frame)).count()
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
        theme: ActiveTheme::new(Theme::builtin_default(), |_| None, ThemeOptions::default()),
        presenter,
    })
    .expect("pipeline starts");
    Harness {
        manager,
        log,
        recorder,
        tap,
    }
}

fn plain(text: &str) -> Utterance {
    Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Queued,
        segments: vec![UtteranceSegment::text(text)],
        source: None,
        say_all: false,
        validity: None,
    }
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
        say_all: false,
        validity: None,
    });

    // The utterance ends once its own audio has played, and the sound,
    // which may play on past that, plays whole.
    assert_eq!(
        harness.recorder.endings_through(id),
        [(id, UtteranceEnding::Completed)]
    );
    let heard = harness.tap.frames.wait_until("the whole sound", |frames| {
        loud_count(frames) >= SOUND_FRAMES
    });

    // The sound starts after the first word's audio, as one unbroken run,
    // and the second word's audio is mixed under it: until the utterance
    // is finished, the mixer mixes a sound only alongside the utterance's
    // own audio (`Source::sound`), so all of the word, which is shorter
    // than the sound, is heard at its place under the sound, and none of it
    // after the run, however the word's synthesis and playback interleave. The few frames the
    // resampler still held of the first word when the sound was placed come
    // after the sound's start too (`Source::mark`).
    let first_loud = heard
        .iter()
        .position(|frame| is_loud(*frame))
        .expect("the sound played");
    assert!(first_loud > 0, "the first word was heard before the sound");
    assert!(
        heard[..first_loud].iter().all(|frame| *frame != 0.0),
        "the first word's audio runs up to the sound: {:?}",
        &heard[..first_loud]
    );

    let sound_alone = f32::from(LOUD) / 32_768.0;
    let run = &heard[first_loud..first_loud + SOUND_FRAMES];
    assert!(run.iter().all(|frame| is_loud(*frame)), "one unbroken run");
    assert!(
        run.iter().any(|frame| (frame - sound_alone).abs() > 1e-6),
        "the second word is mixed under the sound"
    );
    assert!(
        heard[first_loud + SOUND_FRAMES..]
            .iter()
            .all(|frame| *frame == 0.0),
        "nothing of the second word comes after the sound: {:?}",
        &heard[first_loud + SOUND_FRAMES..]
    );

    // A later utterance is heard to its end, so anything the first left
    // playing, such as the sound placed a second time, has played by then.
    let after = harness.manager.speak(plain("after"));
    assert_eq!(
        harness.recorder.endings_through(after),
        [
            (id, UtteranceEnding::Completed),
            (after, UtteranceEnding::Completed)
        ]
    );
    assert_eq!(
        *harness.recorder.queued.lock().unwrap(),
        ["the sound: spelling-error wrold", "after"],
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
            vec![SpeechItem::Text("wrold".to_owned())],
            vec![SpeechItem::Text("after".to_owned())]
        ]
    );
    assert_eq!(*harness.recorder.marks.lock().unwrap(), []);

    // The sound played once.
    assert_eq!(
        loud_count(&harness.tap.frames.get()),
        SOUND_FRAMES,
        "the sound played once"
    );
}

#[test]
fn an_event_without_a_sound_to_play_is_spoken_instead() {
    // The harness's theme is the default theme with no sound files found,
    // so browse mode, a sound in the default theme, is spoken.
    let harness = harness(None);
    harness.manager.play_earcon(Earcon::BrowseMode);
    let endings = harness
        .recorder
        .endings
        .wait_until("an ending", |endings| !endings.is_empty());
    assert_eq!(
        endings
            .iter()
            .map(|(_, ending)| ending.clone())
            .collect::<Vec<_>>(),
        [UtteranceEnding::Completed]
    );
    assert_eq!(*harness.recorder.queued.lock().unwrap(), ["browse mode"]);
    assert_eq!(*harness.recorder.sounds.lock().unwrap(), []);
}

/// A WAV file of the loud test sound, at 48 kHz, in a folder of its own
/// for the test `name`, which the test removes.
fn loud_wav(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "verbatim-speech-sounds-{name}-{}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).expect("create the sound's folder");
    let path = dir.join("loud.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).expect("create the WAV");
    for _ in 0..SOUND_FRAMES {
        writer.write_sample(LOUD).expect("write a sample");
    }
    writer.finalize().expect("finish the WAV");
    path
}

#[test]
fn waiting_for_an_event_returns_once_its_sound_has_played_to_its_end() {
    let harness = harness(None);
    let wav = loud_wav("exit");
    harness.manager.themes().set(ActiveTheme::new(
        Theme::builtin_default(),
        |_| Some(wav.clone()),
        ThemeOptions::default(),
    ));
    assert!(
        harness
            .manager
            .play_earcon_to_end(Earcon::Exit, STEP_TIMEOUT),
        "the exit sound is heard within the bound"
    );
    assert_eq!(
        loud_count(&harness.tap.frames.get()),
        SOUND_FRAMES,
        "all of the sound played before it returned"
    );
    assert_eq!(
        *harness.recorder.sounds.lock().unwrap(),
        [Indication::Exit],
        "the sound is reported as played"
    );
    assert!(
        harness.recorder.queued.lock().unwrap().is_empty(),
        "and nothing is spoken"
    );
    drop(harness);
    std::fs::remove_dir_all(wav.parent().expect("the sound's folder"))
        .expect("remove the sound's folder");
}

#[test]
fn waiting_for_an_event_without_a_sound_returns_once_its_words_are_heard() {
    // No sounds loaded: the exit sound is spoken instead.
    let harness = harness(None);
    assert!(
        harness
            .manager
            .play_earcon_to_end(Earcon::Exit, STEP_TIMEOUT)
    );
    let endings: Vec<UtteranceEnding> = harness
        .recorder
        .endings
        .get()
        .iter()
        .map(|(_, ending)| ending.clone())
        .collect();
    assert_eq!(
        endings,
        [UtteranceEnding::Completed],
        "the words were heard in full before it returned"
    );
    assert_eq!(
        *harness.recorder.queued.lock().unwrap(),
        ["Exiting Verbatim"]
    );
}
