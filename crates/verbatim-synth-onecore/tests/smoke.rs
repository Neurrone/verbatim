//! Audible smoke test for the `OneCore` driver over the real WASAPI device.
//!
//! Ignored by default because it opens the default render device and speaks out
//! loud; run it explicitly on a machine with audio:
//!
//! `cargo test -p verbatim-synth-onecore --test smoke -- --ignored`

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use verbatim_audio::Mixer;
use verbatim_audio_wasapi::WasapiDevice;
use verbatim_model::{
    SpeechPriority, TraceId, Utterance, UtteranceEnding, UtteranceId, UtteranceSegment,
};
use verbatim_speech::{SpeechEvents, SpeechManager, SpeechManagerConfig, SynthId, SynthRegistry};
use verbatim_synth_onecore::{ONECORE_ID, register};

/// Forwards each ending to the test.
struct Endings(Mutex<mpsc::Sender<(UtteranceId, UtteranceEnding)>>);

impl SpeechEvents for Endings {
    fn utterance_queued(&self, _: UtteranceId, _: TraceId, _: &str, _: Instant) {}

    fn audio_started(&self, _: UtteranceId, _: TraceId, _: Instant) {}

    fn utterance_ended(
        &self,
        utterance: UtteranceId,
        _: TraceId,
        ending: &UtteranceEnding,
        _: Instant,
    ) {
        let _ = self.0.lock().unwrap().send((utterance, ending.clone()));
    }
}

#[test]
#[ignore = "plays audio on the default render device"]
fn speaks_test_through_wasapi() {
    let mut registry = SynthRegistry::new();
    register(&mut registry);
    let (sender, endings) = mpsc::channel();

    let manager = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new(ONECORE_ID),
        saved_settings: Box::new(|_| Vec::new()),
        mixer: Arc::new(
            Mixer::start(Box::new(WasapiDevice::new().expect("wake event"))).expect("audio starts"),
        ),
        events: Some(Arc::new(Endings(Mutex::new(sender)))),
        theme: None,
    })
    .expect("OneCore pipeline starts");

    let id = manager.speak(Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Queued,
        segments: vec![UtteranceSegment::text("test")],
        source: None,
        validity: None,
    });

    assert_eq!(
        endings
            .recv_timeout(Duration::from_secs(10))
            .expect("the utterance ends"),
        (id, UtteranceEnding::Completed)
    );
}
