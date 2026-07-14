//! Pipeline tests (architecture section 13, layer 3): priority lanes and
//! cancellation driven by a step-controlled synth, token rendering asserted
//! through the capture synth, and the settings host round-tripped including
//! its persist callback.

use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use verbatim_audio::{AudioError, AudioSink, PcmFormat};
use verbatim_model::{Role, SegmentContent, SpeechPriority, TraceId, Utterance, UtteranceSegment};
use verbatim_speech::{
    SettingId, SettingValue, SpeechManager, SpeechManagerConfig, SpeechRequest, SpeechSettingsHost,
    SynthDriver, SynthError, SynthId, SynthRegistry, SynthSink,
};
use verbatim_synth_capture::{CaptureLog, CaptureSynth};

const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// A fake audio sink that records the sequence of sink calls, so tests can
/// assert that interrupted utterances are stopped and completed ones drained.
#[derive(Clone, Default)]
struct RecordingSink {
    events: Arc<Mutex<Vec<SinkEvent>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SinkEvent {
    Begin,
    Write(usize),
    End,
    Stop,
}

impl RecordingSink {
    fn events(&self) -> Vec<SinkEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl AudioSink for RecordingSink {
    fn begin(&mut self, _format: PcmFormat, _trace_id: TraceId) -> Result<(), AudioError> {
        self.events.lock().unwrap().push(SinkEvent::Begin);
        Ok(())
    }

    fn write(&mut self, samples: &[i16]) -> Result<(), AudioError> {
        self.events
            .lock()
            .unwrap()
            .push(SinkEvent::Write(samples.len()));
        Ok(())
    }

    fn end(&mut self) -> Result<(), AudioError> {
        self.events.lock().unwrap().push(SinkEvent::End);
        Ok(())
    }

    fn stop(&mut self) {
        self.events.lock().unwrap().push(SinkEvent::Stop);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Completion {
    text: String,
    cancelled: bool,
}

/// A synth whose `speak` announces its start and then streams silent PCM until
/// the test signals it to finish or the pipeline cancels it. Streaming (rather
/// than a per-chunk handshake) lets cancellation be observed without racing the
/// interrupt against a manual step.
struct ControlSynth {
    started: Sender<String>,
    finish: Receiver<()>,
    completions: Arc<Mutex<Vec<Completion>>>,
}

impl SynthDriver for ControlSynth {
    fn id(&self) -> SynthId {
        SynthId::new("control")
    }

    fn display_name(&self) -> String {
        "Control synth".to_owned()
    }

    fn pcm_format(&self) -> PcmFormat {
        PcmFormat {
            sample_rate: 22_050,
            channels: 1,
        }
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

    fn speak(
        &mut self,
        request: &SpeechRequest,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError> {
        self.started.send(request.text.clone()).unwrap();
        let chunk = [0i16; 16];
        let deadline = Instant::now() + STEP_TIMEOUT;
        loop {
            match self.finish.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => {
                    self.record(&request.text, false);
                    return Ok(());
                }
                Err(TryRecvError::Empty) => {}
            }
            if let ControlFlow::Break(()) = sink.push_pcm(&chunk) {
                self.record(&request.text, true);
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(SynthError::Synthesis("control synth timed out".to_owned()));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

impl ControlSynth {
    fn record(&self, text: &str, cancelled: bool) {
        self.completions.lock().unwrap().push(Completion {
            text: text.to_owned(),
            cancelled,
        });
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
    }
}

struct ControlHarness {
    manager: SpeechManager,
    sink: RecordingSink,
    started: Receiver<String>,
    finish: Sender<()>,
    completions: Arc<Mutex<Vec<Completion>>>,
}

fn control_manager() -> ControlHarness {
    let (started_tx, started_rx) = unbounded::<String>();
    let (finish_tx, finish_rx) = unbounded::<()>();
    let completions = Arc::new(Mutex::new(Vec::new()));
    let completions_for_factory = Arc::clone(&completions);

    let mut registry = SynthRegistry::new();
    registry.register(
        SynthId::new("control"),
        "Control synth",
        Box::new(move || {
            Ok(Box::new(ControlSynth {
                started: started_tx.clone(),
                finish: finish_rx.clone(),
                completions: Arc::clone(&completions_for_factory),
            }) as Box<dyn SynthDriver>)
        }),
    );

    let sink = RecordingSink::default();
    let manager = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new("control"),
        initial_settings: Vec::new(),
        sink: Box::new(sink.clone()),
        events: None,
    })
    .expect("pipeline starts");

    ControlHarness {
        manager,
        sink,
        started: started_rx,
        finish: finish_tx,
        completions,
    }
}

fn recv_started(started: &Receiver<String>) -> String {
    started.recv_timeout(STEP_TIMEOUT).expect("synth started")
}

fn wait_for_completions(
    completions: &Arc<Mutex<Vec<Completion>>>,
    count: usize,
) -> Vec<Completion> {
    let deadline = Instant::now() + STEP_TIMEOUT;
    while completions.lock().unwrap().len() < count && Instant::now() < deadline {
        std::thread::yield_now();
    }
    completions.lock().unwrap().clone()
}

#[test]
fn next_lane_jumps_ahead_of_queued() {
    let harness = control_manager();

    harness.manager.speak(queued("first"));
    assert_eq!(recv_started(&harness.started), "first");

    // With "first" streaming, enqueue a queued then a next utterance.
    harness.manager.speak(queued("second-queued"));
    harness
        .manager
        .speak(utterance("third-next", SpeechPriority::Next));

    // Finish each; the pipeline must serve the next-lane utterance first.
    harness.finish.send(()).unwrap();
    assert_eq!(recv_started(&harness.started), "third-next");
    harness.finish.send(()).unwrap();
    assert_eq!(recv_started(&harness.started), "second-queued");
    harness.finish.send(()).unwrap();

    let texts: Vec<String> = wait_for_completions(&harness.completions, 3)
        .into_iter()
        .map(|completion| completion.text)
        .collect();
    assert_eq!(texts, ["first", "third-next", "second-queued"]);
}

#[test]
fn interrupt_cancels_current_and_queued() {
    let harness = control_manager();

    harness.manager.speak(queued("current"));
    assert_eq!(recv_started(&harness.started), "current");

    // Queue a victim, then interrupt: the interrupt cancels "current" and
    // drops "queued-victim" before speaking.
    harness.manager.speak(queued("queued-victim"));
    harness
        .manager
        .speak(utterance("urgent", SpeechPriority::Interrupt));

    // The streaming "current" observes the cancel flag on its own; "urgent"
    // then runs.
    assert_eq!(recv_started(&harness.started), "urgent");
    harness.finish.send(()).unwrap();

    let recorded = wait_for_completions(&harness.completions, 2);
    assert_eq!(
        recorded,
        vec![
            Completion {
                text: "current".to_owned(),
                cancelled: true,
            },
            Completion {
                text: "urgent".to_owned(),
                cancelled: false,
            },
        ],
        "the queued victim never speaks; current is cancelled, urgent completes"
    );

    // The interrupted stream was discarded with stop(), not drained with end().
    let events = harness.sink.events();
    assert!(
        events.contains(&SinkEvent::Stop),
        "interrupt discards audio: {events:?}"
    );
}

#[test]
fn renders_tokens_through_capture_synth() {
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

    let manager = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new("capture"),
        initial_settings: Vec::new(),
        sink: Box::new(RecordingSink::default()),
        events: None,
    })
    .expect("pipeline starts");

    manager.speak(Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Queued,
        segments: vec![
            UtteranceSegment::text("Settings"),
            UtteranceSegment::new(SegmentContent::Role(Role::MenuItem)),
        ],
    });

    let deadline = Instant::now() + STEP_TIMEOUT;
    while log.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    let recorded = log.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].request.text, "Settings menu item");
}

#[test]
fn settings_host_get_set_commit_revert() {
    type Persisted = Arc<Mutex<Vec<(SynthId, Vec<(SettingId, SettingValue)>)>>>;
    let persisted: Persisted = Arc::new(Mutex::new(Vec::new()));
    let persisted_for_cb = Arc::clone(&persisted);

    let mut registry = SynthRegistry::new();
    registry.register(
        SynthId::new("capture"),
        "Capture synth",
        Box::new(|| Ok(Box::new(CaptureSynth::new()) as Box<dyn SynthDriver>)),
    );

    let manager = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new("capture"),
        initial_settings: Vec::new(),
        sink: Box::new(RecordingSink::default()),
        events: None,
    })
    .expect("pipeline starts");

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
