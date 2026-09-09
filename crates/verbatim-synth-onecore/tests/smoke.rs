//! Audible smoke test for the `OneCore` driver over the real WASAPI sink.
//!
//! Ignored by default because it opens the default render device and speaks out
//! loud; run it explicitly on a machine with audio:
//!
//! `cargo test -p verbatim-synth-onecore --test smoke -- --ignored`

use std::time::Duration;

use verbatim_audio_wasapi::WasapiSink;
use verbatim_model::{SpeechPriority, TraceId, Utterance, UtteranceSegment};
use verbatim_speech::{SpeechManager, SpeechManagerConfig, SynthId, SynthRegistry};
use verbatim_synth_onecore::{ONECORE_ID, register};

#[test]
#[ignore = "plays audio on the default render device"]
fn speaks_test_through_wasapi() {
    let mut registry = SynthRegistry::new();
    register(&mut registry);

    let manager = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new(ONECORE_ID),
        initial_settings: Vec::new(),
        sink: Box::new(WasapiSink::new()),
        events: None,
        theme: None,
    })
    .expect("OneCore pipeline starts");

    manager.speak(Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Queued,
        segments: vec![UtteranceSegment::text("test")],
        source: None,
    });

    // Give synthesis and playback time to finish before the manager drops.
    std::thread::sleep(Duration::from_secs(3));
}
