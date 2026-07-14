//! `OneCore` synth driver (architecture section 6).
//!
//! Streams PCM from `WinRT`'s `Windows.Media.SpeechSynthesis` via windows-rs,
//! behind the shared [`SynthDriver`] contract. This is the first voice
//! Verbatim speaks with (milestone M1); eSpeak NG joins in M3 as the
//! latency-budget reference synth.
//!
//! Threading: the driver is built and driven entirely on Verbatim's dedicated
//! synth thread, so it blocks freely on the `WinRT` async operations rather
//! than spawning threads of its own. Settings map onto `SpeechSynthesizerOptions`
//! using NVDA's `OneCore` curve (see `nvda/source/synthDrivers/oneCore.py`):
//! rate, pitch, and volume are the NVDA-standard 0..=100 sliders.
//!
//! Index marks: `OneCore` cannot report positions mid-utterance in M1, so any
//! marks on a request are echoed once synthesis completes.

use std::ops::ControlFlow;

use tracing::debug;
use verbatim_audio::PcmFormat;
use verbatim_speech::{
    IndexMark, SettingDescriptor, SettingId, SettingValue, SpeechRequest, SynthDriver, SynthError,
    SynthFactory, SynthId, SynthRegistry, SynthSink,
};
use windows::Media::SpeechSynthesis::SpeechSynthesizer;
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;

/// The stable id of the `OneCore` driver.
pub const ONECORE_ID: &str = "onecore";

/// The Fluent message id for the `OneCore` driver's display name.
const ONECORE_DISPLAY_NAME_KEY: &str = "synth-name-onecore";

/// The localized display name of the `OneCore` driver, resolved through
/// `verbatim-i18n` (falling back to embedded English).
fn onecore_display_name() -> String {
    verbatim_i18n::message(ONECORE_DISPLAY_NAME_KEY)
}

/// `AsyncStatus::Started`; the operation is still running. Compared as the raw
/// discriminant so the `windows-future` type need not be named.
const ASYNC_STARTED: i32 = 0;

// NVDA OneCore mapping constants (oneCore.py). Without rate boost, 50 maps to
// 1.0x exactly; the rate-boost toggle lifts the maximum to 6.0x, matching
// NVDA's DEFAULT_MAX_RATE and BOOSTED_MAX_RATE.
const MIN_RATE: f64 = 0.5;
const DEFAULT_MAX_RATE: f64 = 1.5;
const BOOSTED_MAX_RATE: f64 = 6.0;
const MIN_PITCH: f64 = 0.0;
const MAX_PITCH: f64 = 2.0;

/// Samples pushed to the sink per chunk (~50 ms at 22050 Hz), so cancellation
/// is observed promptly between chunks.
const CHUNK_SAMPLES: usize = 1_102;

/// The default PCM format assumed before the first synthesis reveals the
/// current voice's true format. `OneCore` voices are 22050 Hz, 16-bit, mono.
const DEFAULT_FORMAT: PcmFormat = PcmFormat {
    sample_rate: 22_050,
    channels: 1,
};

/// One selectable voice.
struct VoiceEntry {
    id: String,
    display_name: String,
}

/// The `OneCore` synthesizer driver.
pub struct OneCoreSynth {
    synth: SpeechSynthesizer,
    voices: Vec<VoiceEntry>,
    current_voice_id: String,
    rate: i32,
    rate_boost: bool,
    pitch: i32,
    volume: i32,
    last_format: PcmFormat,
}

/// Initializes COM for this thread as MTA, tolerating a prior init in another
/// mode (`RPC_E_CHANGED_MODE`).
fn ensure_com() -> Result<(), SynthError> {
    // Safe: no reserved parameter; we never uninitialize.
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        return Err(SynthError::Unavailable(format!(
            "CoInitializeEx failed: {:#010x}",
            hr.0
        )));
    }
    Ok(())
}

fn unavailable(context: &str, error: &windows::core::Error) -> SynthError {
    SynthError::Unavailable(format!("{context}: {error}"))
}

fn synthesis(context: &str, error: &windows::core::Error) -> SynthError {
    SynthError::Synthesis(format!("{context}: {error}"))
}

/// Maps a 0..=100 percent onto a linear parameter range, matching NVDA's
/// `_percentToParam`.
fn percent_to_param(percent: i32, min: f64, max: f64) -> f64 {
    f64::from(percent) / 100.0 * (max - min) + min
}

/// The maximum `SpeakingRate` for the current rate-boost state (NVDA's
/// `DEFAULT_MAX_RATE`/`BOOSTED_MAX_RATE`).
fn max_rate(rate_boost: bool) -> f64 {
    if rate_boost {
        BOOSTED_MAX_RATE
    } else {
        DEFAULT_MAX_RATE
    }
}

impl OneCoreSynth {
    /// Builds the driver, enumerating installed voices and selecting the
    /// synthesizer's default voice.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Unavailable`] when COM or the speech synthesizer
    /// cannot be initialized, or when no voices are installed.
    pub fn new() -> Result<Self, SynthError> {
        ensure_com()?;
        let synth =
            SpeechSynthesizer::new().map_err(|error| unavailable("create synthesizer", &error))?;

        let all = SpeechSynthesizer::AllVoices()
            .map_err(|error| unavailable("enumerate voices", &error))?;
        let count = all
            .Size()
            .map_err(|error| unavailable("voice count", &error))?;
        let mut voices = Vec::with_capacity(count as usize);
        for index in 0..count {
            let info = all
                .GetAt(index)
                .map_err(|error| unavailable("read voice", &error))?;
            let id = info
                .Id()
                .map_err(|error| unavailable("voice id", &error))?
                .to_string();
            let display_name = info
                .DisplayName()
                .map_err(|error| unavailable("voice name", &error))?
                .to_string();
            voices.push(VoiceEntry { id, display_name });
        }
        if voices.is_empty() {
            return Err(SynthError::Unavailable(
                "no OneCore voices installed".to_owned(),
            ));
        }

        // Prefer the synthesizer's own default voice; fall back to the first.
        let current_voice_id = synth
            .Voice()
            .ok()
            .and_then(|voice| voice.Id().ok())
            .map(|id| id.to_string())
            .filter(|id| voices.iter().any(|voice| &voice.id == id))
            .unwrap_or_else(|| voices[0].id.clone());

        Ok(Self {
            synth,
            voices,
            current_voice_id,
            rate: 50,
            rate_boost: false,
            pitch: 50,
            volume: 100,
            last_format: DEFAULT_FORMAT,
        })
    }

    /// Applies the current voice selection to the synthesizer.
    fn apply_voice(&self) -> Result<(), SynthError> {
        let all = SpeechSynthesizer::AllVoices()
            .map_err(|error| synthesis("enumerate voices", &error))?;
        let count = all
            .Size()
            .map_err(|error| synthesis("voice count", &error))?;
        for index in 0..count {
            let info = all
                .GetAt(index)
                .map_err(|error| synthesis("read voice", &error))?;
            let id = info
                .Id()
                .map_err(|error| synthesis("voice id", &error))?
                .to_string();
            if id == self.current_voice_id {
                self.synth
                    .SetVoice(&info)
                    .map_err(|error| synthesis("set voice", &error))?;
                return Ok(());
            }
        }
        Ok(())
    }

    /// Applies rate, pitch, and volume to the synthesizer options. Best-effort:
    /// on installations where prosody options are unavailable, synthesis still
    /// proceeds at the default prosody.
    fn apply_options(&self) {
        let options = match self.synth.Options() {
            Ok(options) => options,
            Err(error) => {
                debug!(target: "verbatim::synth::onecore", %error, "prosody options unavailable");
                return;
            }
        };
        let rate = percent_to_param(self.rate, MIN_RATE, max_rate(self.rate_boost));
        let pitch = percent_to_param(self.pitch, MIN_PITCH, MAX_PITCH);
        let volume = f64::from(self.volume) / 100.0;
        if let Err(error) = options.SetSpeakingRate(rate) {
            debug!(target: "verbatim::synth::onecore", %error, "set speaking rate failed");
        }
        if let Err(error) = options.SetAudioPitch(pitch) {
            debug!(target: "verbatim::synth::onecore", %error, "set audio pitch failed");
        }
        if let Err(error) = options.SetAudioVolume(volume) {
            debug!(target: "verbatim::synth::onecore", %error, "set audio volume failed");
        }
    }
}

impl SynthDriver for OneCoreSynth {
    fn id(&self) -> SynthId {
        SynthId::new(ONECORE_ID)
    }

    fn display_name(&self) -> String {
        onecore_display_name()
    }

    fn pcm_format(&self) -> PcmFormat {
        self.last_format
    }

    fn supported_settings(&self) -> Vec<SettingDescriptor> {
        vec![
            SettingDescriptor::Choice {
                id: SettingId::new("voice"),
                label_key: "setting-voice".to_owned(),
                options: self
                    .voices
                    .iter()
                    .map(|voice| (voice.id.clone(), voice.display_name.clone()))
                    .collect(),
            },
            SettingDescriptor::standard_numeric("rate", "setting-rate"),
            SettingDescriptor::Toggle {
                id: SettingId::new("rate-boost"),
                label_key: "setting-rate-boost".to_owned(),
            },
            SettingDescriptor::standard_numeric("pitch", "setting-pitch"),
            SettingDescriptor::standard_numeric("volume", "setting-volume"),
        ]
    }

    fn setting(&self, id: &SettingId) -> Option<SettingValue> {
        match id.0.as_str() {
            "voice" => Some(SettingValue::Choice(self.current_voice_id.clone())),
            "rate" => Some(SettingValue::Number(self.rate)),
            "rate-boost" => Some(SettingValue::Toggle(self.rate_boost)),
            "pitch" => Some(SettingValue::Number(self.pitch)),
            "volume" => Some(SettingValue::Number(self.volume)),
            _ => None,
        }
    }

    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        match (id.0.as_str(), value) {
            ("voice", SettingValue::Choice(choice)) => {
                if self.voices.iter().any(|voice| voice.id == choice) {
                    self.current_voice_id = choice;
                    Ok(())
                } else {
                    Err(SynthError::Setting(format!("unknown voice {choice}")))
                }
            }
            ("rate", SettingValue::Number(number)) => set_percent(&mut self.rate, number),
            ("rate-boost", SettingValue::Toggle(enable)) => {
                // NVDA `_set_rateBoost` semantics: keep the cached 0..=100
                // percent and let the raw SpeakingRate be recomputed against
                // the new maximum on the next synthesis, so enabling boost
                // audibly speeds speech up (the boost is a speed multiplier).
                if enable != self.rate_boost {
                    self.rate_boost = enable;
                }
                Ok(())
            }
            ("pitch", SettingValue::Number(number)) => set_percent(&mut self.pitch, number),
            ("volume", SettingValue::Number(number)) => set_percent(&mut self.volume, number),
            (other, _) => Err(SynthError::Setting(format!(
                "unknown or mistyped setting {other}"
            ))),
        }
    }

    fn speak(
        &mut self,
        request: &SpeechRequest,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError> {
        self.apply_voice()?;
        self.apply_options();

        let text = HSTRING::from(request.text.as_str());
        let operation = self
            .synth
            .SynthesizeTextToStreamAsync(&text)
            .map_err(|error| synthesis("start synthesis", &error))?;
        // Block on this dedicated thread until synthesis completes.
        while operation
            .Status()
            .map_err(|error| synthesis("poll synthesis", &error))?
            .0
            == ASYNC_STARTED
        {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let stream = operation
            .GetResults()
            .map_err(|error| synthesis("finish synthesis", &error))?;

        let wav = read_stream(&stream)?;
        let (format, samples) = parse_wav(&wav)?;
        self.last_format = format;

        for chunk in samples.chunks(CHUNK_SAMPLES) {
            if let ControlFlow::Break(()) = sink.push_pcm(chunk) {
                return Ok(());
            }
        }
        // Echo marks at completion: OneCore cannot track positions mid-text.
        for mark in &request.marks {
            sink.index_reached(IndexMark(mark.mark.0));
        }
        Ok(())
    }
}

/// Validates and stores a 0..=100 percent setting.
fn set_percent(slot: &mut i32, value: i32) -> Result<(), SynthError> {
    if (0..=100).contains(&value) {
        *slot = value;
        Ok(())
    } else {
        Err(SynthError::Setting(format!(
            "value {value} outside 0..=100"
        )))
    }
}

/// Reads the entire synthesized WAV stream into memory.
fn read_stream(
    stream: &windows::Media::SpeechSynthesis::SpeechSynthesisStream,
) -> Result<Vec<u8>, SynthError> {
    let size = stream
        .Size()
        .map_err(|error| synthesis("stream size", &error))?;
    let size = u32::try_from(size)
        .map_err(|_| SynthError::Synthesis("synthesized stream too large".to_owned()))?;
    let input = stream
        .GetInputStreamAt(0)
        .map_err(|error| synthesis("stream input", &error))?;
    let reader =
        DataReader::CreateDataReader(&input).map_err(|error| synthesis("create reader", &error))?;
    let load = reader
        .LoadAsync(size)
        .map_err(|error| synthesis("load stream", &error))?;
    while load
        .Status()
        .map_err(|error| synthesis("poll load", &error))?
        .0
        == ASYNC_STARTED
    {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    load.GetResults()
        .map_err(|error| synthesis("finish load", &error))?;
    let mut bytes = vec![0u8; size as usize];
    reader
        .ReadBytes(&mut bytes)
        .map_err(|error| synthesis("read bytes", &error))?;
    Ok(bytes)
}

/// Reads a little-endian `u32` at `offset`, or `None` when out of bounds.
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|slice| u32::from_le_bytes(slice.try_into().expect("4-byte slice")))
}

/// Reads a little-endian `u16` at `offset`, or `None` when out of bounds.
fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..offset + 2)
        .map(|slice| u16::from_le_bytes(slice.try_into().expect("2-byte slice")))
}

/// Parses a 16-bit PCM WAV, returning its format and interleaved samples.
///
/// Walks the RIFF chunks rather than assuming a fixed header layout, since the
/// `fmt ` chunk size and the presence of a `fact` chunk vary.
fn parse_wav(bytes: &[u8]) -> Result<(PcmFormat, Vec<i16>), SynthError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(SynthError::Synthesis("not a RIFF/WAVE stream".to_owned()));
    }

    let mut sample_rate = None;
    let mut channels = None;
    let mut bits = None;
    let mut data: Option<&[u8]> = None;

    let mut offset = 12;
    while offset + 8 <= bytes.len() {
        let chunk_id = &bytes[offset..offset + 4];
        let chunk_size = read_u32(bytes, offset + 4)
            .ok_or_else(|| SynthError::Synthesis("truncated chunk header".to_owned()))?
            as usize;
        let body_start = offset + 8;
        let body_end = body_start.saturating_add(chunk_size).min(bytes.len());
        match chunk_id {
            b"fmt " => {
                channels = read_u16(bytes, body_start + 2);
                sample_rate = read_u32(bytes, body_start + 4);
                bits = read_u16(bytes, body_start + 14);
            }
            b"data" => {
                data = Some(&bytes[body_start..body_end]);
            }
            _ => {}
        }
        // Chunks are word-aligned: an odd size carries a pad byte.
        offset = body_start + chunk_size + (chunk_size & 1);
    }

    let sample_rate =
        sample_rate.ok_or_else(|| SynthError::Synthesis("missing fmt chunk".to_owned()))?;
    let channels =
        channels.ok_or_else(|| SynthError::Synthesis("missing channel count".to_owned()))?;
    let bits = bits.ok_or_else(|| SynthError::Synthesis("missing bit depth".to_owned()))?;
    if bits != 16 {
        return Err(SynthError::Synthesis(format!(
            "unexpected bit depth {bits}, only 16-bit PCM is supported"
        )));
    }
    let data = data.ok_or_else(|| SynthError::Synthesis("missing data chunk".to_owned()))?;

    let samples = data
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    Ok((
        PcmFormat {
            sample_rate,
            channels,
        },
        samples,
    ))
}

/// A [`SynthFactory`] that builds a fresh `OneCore` driver.
#[must_use]
pub fn factory() -> SynthFactory {
    Box::new(|| Ok(Box::new(OneCoreSynth::new()?) as Box<dyn SynthDriver>))
}

/// Registers the `OneCore` driver in a [`SynthRegistry`] under [`ONECORE_ID`].
pub fn register(registry: &mut SynthRegistry) {
    registry.register(SynthId::new(ONECORE_ID), onecore_display_name(), factory());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvda_rate_curve_maps_fifty_to_unity() {
        assert!((percent_to_param(50, MIN_RATE, DEFAULT_MAX_RATE) - 1.0).abs() < f64::EPSILON);
        assert!((percent_to_param(0, MIN_RATE, DEFAULT_MAX_RATE) - MIN_RATE).abs() < f64::EPSILON);
        assert!(
            (percent_to_param(100, MIN_RATE, DEFAULT_MAX_RATE) - DEFAULT_MAX_RATE).abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn rate_boost_keeps_percent_and_scales_raw_rate() {
        // NVDA `_set_rateBoost` semantics: the 0..=100 percent is stable across
        // the toggle; the raw SpeakingRate jumps because the maximum grows.
        let percent = 50;

        // Unboosted, 50% is 1.0x against the 0.5..1.5 range.
        let raw_unboosted = percent_to_param(percent, MIN_RATE, max_rate(false));
        assert!((raw_unboosted - 1.0).abs() < f64::EPSILON);

        // Enabling boost keeps percent 50 and recomputes the raw against
        // 0.5..6.0: 50/100 * (6.0 - 0.5) + 0.5 = 3.25x.
        let raw_boosted = percent_to_param(percent, MIN_RATE, max_rate(true));
        assert!((raw_boosted - 3.25).abs() < f64::EPSILON);

        // Disabling boost returns the same percent to 1.0x.
        assert!((percent_to_param(percent, MIN_RATE, max_rate(false)) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_wav_reads_format_and_samples() {
        // Minimal 22050 Hz mono 16-bit WAV with two samples.
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&0u32.to_le_bytes()); // riff size (ignored)
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // channels
        wav.extend_from_slice(&22_050u32.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&4u32.to_le_bytes());
        wav.extend_from_slice(&100i16.to_le_bytes());
        wav.extend_from_slice(&(-100i16).to_le_bytes());

        let (format, samples) = parse_wav(&wav).expect("valid wav");
        assert_eq!(
            format,
            PcmFormat {
                sample_rate: 22_050,
                channels: 1
            }
        );
        assert_eq!(samples, vec![100, -100]);
    }
}
