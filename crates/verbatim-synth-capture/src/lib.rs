//! Capture synth (architecture section 13, layer 3).
//!
//! A [`SynthDriver`] that records every [`SpeechRequest`] with a timestamp
//! instead of producing real audio, so pipeline tests and the E2E harness can
//! assert on what would have been spoken and on latency against the budget. It
//! still exercises the full sink contract: it emits a short burst of silent
//! PCM (honoring cooperative cancellation) and echoes every index mark, so the
//! priority lanes, cancellation path, and audio seam are all driven by tests
//! without a sound card.
//!
//! The recording log is an `Arc<Mutex<Vec<CaptureRecord>>>` shared with the
//! test: construct the driver with [`CaptureSynth::new`] and read the log back
//! through [`CaptureSynth::log`], or supply your own handle with
//! [`CaptureSynth::with_log`].

use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use verbatim_audio::PcmFormat;
use verbatim_speech::{
    IndexMark, SettingDescriptor, SettingId, SettingValue, SpeechRequest, SynthDriver, SynthError,
    SynthId,
};

/// Sample rate of the capture synth's silent PCM.
const CAPTURE_SAMPLE_RATE: u32 = 22_050;

/// Number of silent samples emitted per request.
const SILENT_SAMPLE_COUNT: usize = 100;

/// The stable id of the capture synth.
const CAPTURE_ID: &str = "capture";

/// One recorded request: the exact [`SpeechRequest`] the pipeline handed the
/// driver, plus the moment `speak` received it.
#[derive(Clone, Debug)]
pub struct CaptureRecord {
    /// The request as rendered by the pipeline.
    pub request: SpeechRequest,
    /// When `speak` was entered, for latency assertions.
    pub at: Instant,
}

/// A shared, cloneable handle to a capture synth's recording log.
pub type CaptureLog = Arc<Mutex<Vec<CaptureRecord>>>;

/// A recording synth driver for tests (architecture section 13, layer 3).
pub struct CaptureSynth {
    log: CaptureLog,
    rate: i32,
    voice: String,
}

impl CaptureSynth {
    /// Creates a capture synth with a fresh, empty log.
    #[must_use]
    pub fn new() -> Self {
        Self::with_log(Arc::new(Mutex::new(Vec::new())))
    }

    /// Creates a capture synth recording into an existing shared log, so a
    /// test can hold the handle before the driver is moved onto a synth
    /// thread.
    #[must_use]
    pub fn with_log(log: CaptureLog) -> Self {
        Self {
            log,
            rate: 50,
            voice: default_voice_id().to_owned(),
        }
    }

    /// A clone of the shared recording log.
    #[must_use]
    pub fn log(&self) -> CaptureLog {
        Arc::clone(&self.log)
    }
}

impl Default for CaptureSynth {
    fn default() -> Self {
        Self::new()
    }
}

/// The two synthetic voices the capture synth offers, exercising the
/// [`SettingDescriptor::Choice`] path. These ids are test fixtures, never
/// shown to real users.
fn voices() -> [(&'static str, &'static str); 2] {
    [("capture-a", "Capture A"), ("capture-b", "Capture B")]
}

fn default_voice_id() -> &'static str {
    voices()[0].0
}

impl SynthDriver for CaptureSynth {
    fn id(&self) -> SynthId {
        SynthId::new(CAPTURE_ID)
    }

    fn display_name(&self) -> String {
        // Test-only driver: this name never reaches the production synth list.
        "Capture synth".to_owned()
    }

    fn pcm_format(&self) -> PcmFormat {
        PcmFormat {
            sample_rate: CAPTURE_SAMPLE_RATE,
            channels: 1,
        }
    }

    fn supported_settings(&self) -> Vec<SettingDescriptor> {
        vec![
            SettingDescriptor::Choice {
                id: SettingId::new("voice"),
                label_key: "setting-voice".to_owned(),
                options: voices()
                    .into_iter()
                    .map(|(id, name)| (id.to_owned(), name.to_owned()))
                    .collect(),
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
            ("voice", SettingValue::Choice(choice)) => {
                if voices().iter().any(|(voice_id, _)| *voice_id == choice) {
                    self.voice = choice;
                    Ok(())
                } else {
                    Err(SynthError::Setting(format!("unknown voice {choice}")))
                }
            }
            ("rate", SettingValue::Number(rate)) => {
                if (0..=100).contains(&rate) {
                    self.rate = rate;
                    Ok(())
                } else {
                    Err(SynthError::Setting(format!("rate {rate} out of range")))
                }
            }
            (other, _) => Err(SynthError::Setting(format!(
                "unknown or mistyped setting {other}"
            ))),
        }
    }

    fn speak(
        &mut self,
        request: &SpeechRequest,
        sink: &mut dyn verbatim_speech::SynthSink,
    ) -> Result<(), SynthError> {
        // Record first, so a request is visible to tests even if synthesis is
        // cancelled before any audio flows.
        if let Ok(mut log) = self.log.lock() {
            log.push(CaptureRecord {
                request: request.clone(),
                at: Instant::now(),
            });
        }

        let silence = [0i16; SILENT_SAMPLE_COUNT];
        if let ControlFlow::Break(()) = sink.push_pcm(&silence) {
            // Cooperative cancel: stop promptly, echo nothing further.
            return Ok(());
        }

        // Echo every mark: the capture synth cannot track positions mid-text,
        // so it reports them all once synthesis completes, matching the
        // OneCore driver's contract.
        for mark in &request.marks {
            sink.index_reached(IndexMark(mark.mark.0));
        }
        Ok(())
    }
}
