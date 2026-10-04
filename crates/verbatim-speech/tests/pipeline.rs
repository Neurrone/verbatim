//! Pipeline tests (architecture section 13, layer 3): priority lanes,
//! cancellation, and each utterance's single ending (decision D17), driven
//! by a step-controlled synth through the real mixer on a silent real-time
//! device; token rendering and the mark fallback asserted through the
//! capture synth; and the settings host round-tripped including its persist
//! callback.

use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use verbatim_audio::{Mixer, PcmFormat, SilentDevice};
use verbatim_model::{
    FocusNow, FocusValidity, NodeId, Role, SegmentContent, SpeechPriority, TraceId, Utterance,
    UtteranceEnding, UtteranceId, UtteranceSegment,
};
use verbatim_speech::{
    IndexMark, SettingDescriptor, SettingId, SettingValue, SpeechEvents, SpeechItem, SpeechManager,
    SpeechManagerConfig, SpeechSequence, SpeechSettingsHost, SynthDriver, SynthError, SynthFactory,
    SynthId, SynthRegistry, SynthSink, Theme,
};
use verbatim_synth_capture::{CaptureLog, CaptureSynth};

const STEP_TIMEOUT: Duration = Duration::from_secs(5);

const FORMAT: PcmFormat = PcmFormat {
    sample_rate: 22_050,
    channels: 1,
};

/// What the pipeline reported: every ending and every mark, in order.
#[derive(Default)]
struct Recorder {
    endings: Mutex<Vec<(UtteranceId, UtteranceEnding)>>,
    marks: Mutex<Vec<(UtteranceId, IndexMark)>>,
}

impl SpeechEvents for Recorder {
    fn utterance_queued(&self, _: UtteranceId, _: TraceId, _: &str, _: Instant) {}

    fn audio_started(&self, _: UtteranceId, _: TraceId, _: Instant) {}

    fn mark_reached(&self, utterance: UtteranceId, _: TraceId, mark: IndexMark, _: Instant) {
        self.marks.lock().unwrap().push((utterance, mark));
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

impl Recorder {
    /// Waits until `count` endings have been reported, then returns them all.
    fn endings(&self, count: usize) -> Vec<(UtteranceId, UtteranceEnding)> {
        let deadline = Instant::now() + STEP_TIMEOUT;
        while self.endings.lock().unwrap().len() < count && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        self.endings.lock().unwrap().clone()
    }

    fn ending_of(&self, id: UtteranceId) -> Option<UtteranceEnding> {
        self.endings
            .lock()
            .unwrap()
            .iter()
            .find(|(utterance, _)| *utterance == id)
            .map(|(_, ending)| ending.clone())
    }
}

/// A synth whose `speak` announces its start and then streams quiet tone
/// until the test signals it to finish or the pipeline cancels it. Text
/// "fail" fails at once.
struct ControlSynth {
    started: Sender<String>,
    finish: Receiver<()>,
}

impl SynthDriver for ControlSynth {
    fn id(&self) -> SynthId {
        SynthId::new("control")
    }

    fn display_name(&self) -> String {
        "Control synth".to_owned()
    }

    fn supported_settings(&self) -> Vec<verbatim_speech::SettingDescriptor> {
        Vec::new()
    }

    fn setting(&self, _id: &SettingId) -> Option<SettingValue> {
        None
    }

    fn set_setting(&mut self, id: &SettingId, _value: SettingValue) -> Result<(), SynthError> {
        Err(SynthError::Setting(format!("no setting {id}")))
    }

    fn places_marks(&self) -> bool {
        true
    }

    fn speak(
        &mut self,
        sequence: &SpeechSequence,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError> {
        let text = sequence.text();
        self.started.send(text.clone()).unwrap();
        if text == "fail" {
            return Err(SynthError::Synthesis("asked to fail".to_owned()));
        }
        let chunk = [1_000i16; 16];
        let deadline = Instant::now() + STEP_TIMEOUT;
        loop {
            match self.finish.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => return Ok(()),
                Err(TryRecvError::Empty) => {}
            }
            if let ControlFlow::Break(()) = sink.push_pcm(FORMAT, &chunk) {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(SynthError::Synthesis("control synth timed out".to_owned()));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn queued(text: &str) -> Utterance {
    utterance(text, SpeechPriority::Queued)
}

fn utterance(text: &str, priority: SpeechPriority) -> Utterance {
    Utterance {
        trace_id: TraceId::mint(),
        priority,
        segments: vec![UtteranceSegment::text(text)],
        source: None,
        validity: None,
    }
}

fn mixer() -> Arc<Mixer> {
    Arc::new(Mixer::start(Box::new(SilentDevice::new())).expect("the silent device opens"))
}

struct ControlHarness {
    manager: SpeechManager,
    recorder: Arc<Recorder>,
    started: Receiver<String>,
    finish: Sender<()>,
}

fn control_manager() -> ControlHarness {
    let (started_tx, started_rx) = unbounded::<String>();
    let (finish_tx, finish_rx) = unbounded::<()>();

    let mut registry = SynthRegistry::new();
    registry.register(
        SynthId::new("control"),
        "Control synth",
        Box::new(move || {
            Ok(Box::new(ControlSynth {
                started: started_tx.clone(),
                finish: finish_rx.clone(),
            }) as Box<dyn SynthDriver>)
        }),
    );

    let recorder = Arc::new(Recorder::default());
    let manager = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new("control"),
        saved_settings: Box::new(|_| Vec::new()),
        mixer: mixer(),
        events: Some(Arc::clone(&recorder) as Arc<dyn SpeechEvents>),
        theme: None,
    })
    .expect("pipeline starts");

    ControlHarness {
        manager,
        recorder,
        started: started_rx,
        finish: finish_tx,
    }
}

fn recv_started(started: &Receiver<String>) -> String {
    started.recv_timeout(STEP_TIMEOUT).expect("synth started")
}

fn capture_manager(theme: Option<Box<dyn Theme>>) -> (SpeechManager, CaptureLog, Arc<Recorder>) {
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
    let recorder = Arc::new(Recorder::default());
    let manager = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new("capture"),
        saved_settings: Box::new(|_| Vec::new()),
        mixer: mixer(),
        events: Some(Arc::clone(&recorder) as Arc<dyn SpeechEvents>),
        theme,
    })
    .expect("pipeline starts");
    (manager, log, recorder)
}

#[test]
fn next_lane_jumps_ahead_of_queued_and_each_completes_once_played() {
    let harness = control_manager();

    let first = harness.manager.speak(queued("first"));
    assert_eq!(recv_started(&harness.started), "first");

    // With "first" streaming, enqueue a queued then a next utterance.
    let second = harness.manager.speak(queued("second-queued"));
    let third = harness
        .manager
        .speak(utterance("third-next", SpeechPriority::Next));

    // Finish each; the pipeline must serve the next-lane utterance first.
    harness.finish.send(()).unwrap();
    assert_eq!(recv_started(&harness.started), "third-next");
    harness.finish.send(()).unwrap();
    assert_eq!(recv_started(&harness.started), "second-queued");
    harness.finish.send(()).unwrap();

    assert_eq!(
        harness.recorder.endings(3),
        vec![
            (first, UtteranceEnding::Completed),
            (third, UtteranceEnding::Completed),
            (second, UtteranceEnding::Completed),
        ]
    );
}

#[test]
fn interrupt_cancels_current_and_queued_each_exactly_once() {
    let harness = control_manager();

    let current = harness.manager.speak(queued("current"));
    assert_eq!(recv_started(&harness.started), "current");

    // Queue a victim, then interrupt: the interrupt cancels "current" and
    // drops "queued-victim" before speaking.
    let victim = harness.manager.speak(queued("queued-victim"));
    let urgent = harness
        .manager
        .speak(utterance("urgent", SpeechPriority::Interrupt));

    assert_eq!(recv_started(&harness.started), "urgent");
    harness.finish.send(()).unwrap();

    let endings = harness.recorder.endings(3);
    assert_eq!(endings.len(), 3, "one ending each: {endings:?}");
    assert_eq!(
        harness.recorder.ending_of(current),
        Some(UtteranceEnding::Cancelled)
    );
    assert_eq!(
        harness.recorder.ending_of(victim),
        Some(UtteranceEnding::Cancelled)
    );
    assert_eq!(
        harness.recorder.ending_of(urgent),
        Some(UtteranceEnding::Completed)
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(harness.recorder.endings(3).len(), 3, "and never a second");
}

#[test]
fn failed_synthesis_ends_the_utterance_as_failed_and_speech_carries_on() {
    let harness = control_manager();

    let failing = harness.manager.speak(queued("fail"));
    assert_eq!(recv_started(&harness.started), "fail");
    let next = harness.manager.speak(queued("next"));
    assert_eq!(recv_started(&harness.started), "next");
    harness.finish.send(()).unwrap();

    assert_eq!(
        harness.recorder.endings(2),
        vec![
            (
                failing,
                UtteranceEnding::Failed("synthesis failed: asked to fail".to_owned())
            ),
            (next, UtteranceEnding::Completed),
        ]
    );
}

#[test]
fn renders_tokens_through_capture_synth() {
    let (manager, log, recorder) = capture_manager(None);

    let id = manager.speak(Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Queued,
        segments: vec![
            UtteranceSegment::text("Settings"),
            UtteranceSegment::new(SegmentContent::Role(Role::MenuItem)),
        ],
        source: None,
        validity: None,
    });

    assert_eq!(recorder.endings(1), vec![(id, UtteranceEnding::Completed)]);
    let requests = log.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].sequence.text(), "Settings menu item");
}

/// A theme that puts a mark after each segment.
struct MarkingTheme;

impl Theme for MarkingTheme {
    fn flatten(&self, utterance: &Utterance, id: UtteranceId) -> SpeechSequence {
        let mut items = Vec::new();
        for (index, segment) in utterance.segments.iter().enumerate() {
            if let SegmentContent::Text(text) = &segment.content {
                items.push(SpeechItem::Text(text.clone()));
            }
            items.push(SpeechItem::Mark(IndexMark(index as u64 + 1)));
        }
        SpeechSequence {
            utterance: id,
            trace_id: utterance.trace_id,
            language: None,
            items,
        }
    }
}

#[test]
fn a_synth_that_cannot_place_marks_gets_the_sequence_split_and_marks_stay_exact() {
    let (manager, log, recorder) = capture_manager(Some(Box::new(MarkingTheme)));

    let id = manager.speak(Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Queued,
        segments: vec![UtteranceSegment::text("one"), UtteranceSegment::text("two")],
        source: None,
        validity: None,
    });

    assert_eq!(recorder.endings(1), vec![(id, UtteranceEnding::Completed)]);
    let texts: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .map(|record| record.sequence.text())
        .collect();
    assert_eq!(
        texts,
        ["one", "two"],
        "one synthesis per piece, no marks inside"
    );
    assert_eq!(
        *recorder.marks.lock().unwrap(),
        vec![(id, IndexMark(1)), (id, IndexMark(2))]
    );
}

#[test]
fn settings_host_get_set_commit_revert() {
    type Persisted = Arc<Mutex<Vec<(SynthId, Vec<(SettingId, SettingValue)>)>>>;
    let persisted: Persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_for_cb = Arc::clone(&persisted);

    let (manager, _log, _recorder) = capture_manager(None);

    let host = manager.settings_host(Box::new(move |id, values| {
        persisted_for_cb
            .lock()
            .unwrap()
            .push((id.clone(), values.to_vec()));
        Ok(())
    }));

    let rate = SettingId::new("rate");
    let voice = SettingId::new("voice");

    // Defaults from the capture synth.
    assert_eq!(host.active_synthesizer().id, SynthId::new("capture"));
    assert!(
        host.synthesizers()
            .iter()
            .any(|choice| choice.id == SynthId::new("capture"))
    );
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(50)));

    // Set within range succeeds and is visible immediately.
    host.set_setting(&rate, SettingValue::Number(75)).unwrap();
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(75)));

    // Out of range is rejected and does not change the mirror.
    let err = host
        .set_setting(&rate, SettingValue::Number(500))
        .unwrap_err();
    assert!(matches!(err, SynthError::Setting(_)));
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(75)));

    // A choice change and a commit persist the current values.
    host.set_setting(&voice, SettingValue::Choice("capture-b".to_owned()))
        .unwrap();
    host.commit().unwrap();
    {
        let records = persisted.lock().unwrap();
        assert_eq!(records.len(), 1);
        let (persisted_id, values) = &records[0];
        assert_eq!(persisted_id, &SynthId::new("capture"));
        assert!(values.contains(&(rate.clone(), SettingValue::Number(75))));
        assert!(values.contains(&(voice.clone(), SettingValue::Choice("capture-b".to_owned()))));
    }

    // An uncommitted change reverts to the committed value.
    host.set_setting(&rate, SettingValue::Number(10)).unwrap();
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(10)));
    host.revert();
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(75)));
}

fn about(text: &str, node: u64, had_focus: bool) -> Utterance {
    Utterance {
        validity: Some(FocusValidity {
            node: NodeId::new(node),
            had_focus,
        }),
        ..queued(text)
    }
}

#[test]
fn a_cancel_ends_current_and_queued_speech() {
    let harness = control_manager();
    let current = harness.manager.speak(queued("current"));
    assert_eq!(recv_started(&harness.started), "current");
    let waiting = harness.manager.speak(queued("waiting"));

    harness.manager.control().cancel();

    assert_eq!(harness.recorder.endings(2).len(), 2);
    assert_eq!(
        harness.recorder.ending_of(current),
        Some(UtteranceEnding::Cancelled)
    );
    assert_eq!(
        harness.recorder.ending_of(waiting),
        Some(UtteranceEnding::Cancelled)
    );
}

/// Speech for a focus the user has left is dropped, with everything queued
/// before it, as NVDA culls expired focus speech; speech after it stays.
#[test]
fn expired_focus_speech_is_dropped_with_what_came_before_it() {
    let harness = control_manager();
    let first = harness.manager.speak(about("first control", 1, true));
    assert_eq!(recv_started(&harness.started), "first control");
    let second = harness.manager.speak(about("second control", 2, true));
    let message = harness.manager.speak(queued("a message"));

    // Focus is now on node 3, inside nothing: both controls have expired.
    harness.manager.control().drop_expired(FocusNow {
        focus: NodeId::new(3),
        ancestors: Vec::new(),
        foreground: None,
    });

    assert_eq!(recv_started(&harness.started), "a message");
    harness.finish.send(()).unwrap();
    harness.recorder.endings(3);
    assert_eq!(
        harness.recorder.ending_of(first),
        Some(UtteranceEnding::Cancelled)
    );
    assert_eq!(
        harness.recorder.ending_of(second),
        Some(UtteranceEnding::Cancelled)
    );
    assert_eq!(
        harness.recorder.ending_of(message),
        Some(UtteranceEnding::Completed)
    );
}

/// Shift pauses speech where it is: its ending waits until it is resumed.
/// Speech arriving while paused cancels what was paused.
#[test]
fn a_pause_holds_speech_until_resumed_and_new_speech_cancels_it() {
    let harness = control_manager();
    let held = harness.manager.speak(queued("held"));
    assert_eq!(recv_started(&harness.started), "held");
    harness.manager.control().toggle_pause();
    harness.finish.send(()).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(harness.recorder.ending_of(held), None, "held while paused");
    harness.manager.control().toggle_pause();
    harness.recorder.endings(1);
    assert_eq!(
        harness.recorder.ending_of(held),
        Some(UtteranceEnding::Completed)
    );

    let paused = harness.manager.speak(queued("paused"));
    assert_eq!(recv_started(&harness.started), "paused");
    harness.manager.control().toggle_pause();
    let next = harness.manager.speak(queued("next"));
    assert_eq!(recv_started(&harness.started), "next");
    harness.finish.send(()).unwrap();
    harness.recorder.endings(3);
    assert_eq!(
        harness.recorder.ending_of(paused),
        Some(UtteranceEnding::Cancelled)
    );
    assert_eq!(
        harness.recorder.ending_of(next),
        Some(UtteranceEnding::Completed)
    );
}

/// A silent synth with a voice and a rate, under any id, that refuses the
/// voice "refused" the way eSpeak NG refuses a variant it cannot load.
struct SettingsSynth {
    id: &'static str,
    voice: String,
    rate: i32,
}

impl SettingsSynth {
    fn factory(id: &'static str) -> SynthFactory {
        Box::new(move || {
            Ok(Box::new(SettingsSynth {
                id,
                voice: "default".to_owned(),
                rate: 50,
            }) as Box<dyn SynthDriver>)
        })
    }
}

impl SynthDriver for SettingsSynth {
    fn id(&self) -> SynthId {
        SynthId::new(self.id)
    }

    fn display_name(&self) -> String {
        self.id.to_owned()
    }

    fn supported_settings(&self) -> Vec<SettingDescriptor> {
        let options = ["default", "other", "refused"]
            .map(|voice| (voice.to_owned(), voice.to_owned()))
            .to_vec();
        vec![
            SettingDescriptor::Choice {
                id: SettingId::new("voice"),
                label_key: "setting-voice".to_owned(),
                options,
            },
            SettingDescriptor::standard_numeric("rate", "setting-rate"),
        ]
    }

    fn setting(&self, id: &SettingId) -> Option<SettingValue> {
        match id.0.as_str() {
            "voice" => Some(SettingValue::Choice(self.voice.clone())),
            "rate" => Some(SettingValue::Number(self.rate)),
            _ => None,
        }
    }

    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        match (id.0.as_str(), value) {
            ("voice", SettingValue::Choice(voice)) if voice != "refused" => self.voice = voice,
            ("rate", SettingValue::Number(rate)) => self.rate = rate,
            (_, value) => return Err(SynthError::Setting(format!("{id} refuses {value:?}"))),
        }
        Ok(())
    }

    fn places_marks(&self) -> bool {
        true
    }

    fn speak(&mut self, _: &SpeechSequence, _: &mut dyn SynthSink) -> Result<(), SynthError> {
        Ok(())
    }
}

/// A factory for a synthesizer whose host cannot start.
fn broken_factory() -> SynthFactory {
    Box::new(|| {
        Err(SynthError::Unavailable(
            "the host would not start".to_owned(),
        ))
    })
}

type SavedStore = Arc<Mutex<Vec<(SynthId, Vec<(SettingId, SettingValue)>)>>>;

/// A manager over `registry` whose saved settings are read live from
/// `saved`, as the app reads them from its config store.
fn settings_manager(
    registry: SynthRegistry,
    initial: &str,
    saved: &SavedStore,
) -> Result<SpeechManager, SynthError> {
    let saved = Arc::clone(saved);
    SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new(initial),
        saved_settings: Box::new(move |synth| {
            saved
                .lock()
                .unwrap()
                .iter()
                .find(|(id, _)| id == synth)
                .map(|(_, values)| values.clone())
                .unwrap_or_default()
        }),
        mixer: mixer(),
        events: None,
        theme: None,
    })
}

fn voice_and_rate(voice: &str, rate: i32) -> Vec<(SettingId, SettingValue)> {
    vec![
        (
            SettingId::new("voice"),
            SettingValue::Choice(voice.to_owned()),
        ),
        (SettingId::new("rate"), SettingValue::Number(rate)),
    ]
}

#[test]
fn a_refused_saved_setting_keeps_the_synths_value_and_the_others_apply() {
    let voice = SettingId::new("voice");
    let rate = SettingId::new("rate");
    // The first voice is not among the options; the driver itself refuses
    // the second.
    for refused in ["not-installed", "refused"] {
        let saved: SavedStore = Arc::new(Mutex::new(vec![(
            SynthId::new("one"),
            voice_and_rate(refused, 70),
        )]));
        let mut registry = SynthRegistry::new();
        registry.register(SynthId::new("one"), "One", SettingsSynth::factory("one"));
        let manager = settings_manager(registry, "one", &saved).expect("startup survives");
        let host = manager.settings_host(Box::new(|_, _| Ok(())));
        assert_eq!(
            host.setting(&voice),
            Some(SettingValue::Choice("default".to_owned()))
        );
        assert_eq!(host.setting(&rate), Some(SettingValue::Number(70)));
    }
}

#[test]
fn startup_falls_back_in_registration_order_when_the_configured_synth_cannot_start() {
    let saved: SavedStore = Arc::new(Mutex::new(vec![(
        SynthId::new("working"),
        voice_and_rate("other", 30),
    )]));
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("broken"), "Broken", broken_factory());
    registry.register(
        SynthId::new("working"),
        "Working",
        SettingsSynth::factory("working"),
    );
    registry.register(
        SynthId::new("spare"),
        "Spare",
        SettingsSynth::factory("spare"),
    );
    registry.register(SynthId::new("configured"), "Configured", broken_factory());

    let manager = settings_manager(registry, "configured", &saved).expect("a fallback starts");
    let host = manager.settings_host(Box::new(|_, _| Ok(())));
    // The configured synth failed, then the first registered one; the next
    // one starts, with its own saved settings.
    assert_eq!(host.active_synthesizer().id, SynthId::new("working"));
    assert_eq!(
        host.setting(&SettingId::new("voice")),
        Some(SettingValue::Choice("other".to_owned()))
    );
    assert_eq!(
        host.setting(&SettingId::new("rate")),
        Some(SettingValue::Number(30))
    );

    // Only when nothing can start does startup fail.
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("broken"), "Broken", broken_factory());
    assert!(matches!(
        settings_manager(registry, "missing", &saved),
        Err(SynthError::Unavailable(_))
    ));
}

#[test]
fn switching_synth_starts_it_with_its_saved_settings() {
    let saved: SavedStore = Arc::new(Mutex::new(Vec::new()));
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("one"), "One", SettingsSynth::factory("one"));
    registry.register(SynthId::new("two"), "Two", SettingsSynth::factory("two"));
    let manager = settings_manager(registry, "one", &saved).expect("pipeline starts");
    let host = manager.settings_host(Box::new(|_, _| Ok(())));

    // Saved after startup, as a commit would, with a voice that is gone.
    saved
        .lock()
        .unwrap()
        .push((SynthId::new("two"), voice_and_rate("not-installed", 20)));
    host.set_active_synthesizer(&SynthId::new("two")).unwrap();

    assert_eq!(host.active_synthesizer().id, SynthId::new("two"));
    assert_eq!(
        host.setting(&SettingId::new("rate")),
        Some(SettingValue::Number(20))
    );
    assert_eq!(
        host.setting(&SettingId::new("voice")),
        Some(SettingValue::Choice("default".to_owned()))
    );
}
