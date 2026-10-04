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
//! Speech sequences become SSML, with each index mark a `<mark>` element.
//! `OneCore` reports where in its audio each mark fell, so the driver pushes
//! the audio up to a mark, reports the mark, and carries on: marks are
//! exact.

use std::fmt::Write as _;
use std::ops::ControlFlow;

use std::sync::mpsc;
use tracing::debug;
use verbatim_audio::PcmFormat;
use verbatim_speech::{
    IndexMark, SettingDescriptor, SettingId, SettingValue, SpeechItem, SpeechSequence, SynthDriver,
    SynthError, SynthFactory, SynthId, SynthRegistry, SynthSink,
};

use windows::Media::SpeechSynthesis::{SpeechAppendedSilence, SpeechSynthesizer, VoiceInformation};
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;
use windows::core::Interface;
use windows_future::IAsyncOperation;

/// The stable id of the `OneCore` driver.
pub const ONECORE_ID: &str = verbatim_speech::hosting::synth_ids::ONECORE;

/// The Fluent message id for the `OneCore` driver's display name.
const ONECORE_DISPLAY_NAME_KEY: &str = "synth-name-onecore";

/// The localized display name of the `OneCore` driver, resolved through
/// `verbatim-i18n` (falling back to embedded English).
fn onecore_display_name() -> String {
    verbatim_i18n::message(ONECORE_DISPLAY_NAME_KEY)
}

/// How often a synthesis in progress checks whether its utterance was
/// cancelled.
const CANCEL_CHECK: std::time::Duration = std::time::Duration::from_millis(10);

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

/// `TimeSpan` ticks per second (100-nanosecond units).
const TICKS_PER_SECOND: u64 = 10_000_000;

/// One selectable voice, read once when the driver starts.
struct VoiceEntry {
    info: VoiceInformation,
    id: String,
    display_name: String,
    /// BCP 47 tag of the voice's language, for the SSML `xml:lang`.
    language: String,
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
    /// The voice last given to the synthesizer, so it is set only when the
    /// selection changes.
    applied_voice: Option<String>,
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
            let language = info
                .Language()
                .map_err(|error| unavailable("voice language", &error))?
                .to_string();
            voices.push(VoiceEntry {
                info,
                id,
                display_name,
                language,
            });
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
            applied_voice: None,
        })
        .inspect(Self::minimize_appended_silence)
    }

    /// Asks the synthesizer for the least silence after each utterance, as
    /// NVDA does (`AppendedSilence` minimum); the speech manager trims what
    /// is left. Best effort: older Windows builds lack the option.
    fn minimize_appended_silence(&self) {
        let result = self
            .synth
            .Options()
            .and_then(|options| options.SetAppendedSilence(SpeechAppendedSilence::Min));
        if let Err(error) = result {
            debug!(target: "verbatim::synth::onecore", %error, "appended silence cannot be set");
        }
    }

    /// Gives the synthesizer the selected voice, when it changed.
    fn apply_voice(&mut self) -> Result<(), SynthError> {
        if self.applied_voice.as_ref() == Some(&self.current_voice_id) {
            return Ok(());
        }
        if let Some(voice) = self
            .voices
            .iter()
            .find(|voice| voice.id == self.current_voice_id)
        {
            self.synth
                .SetVoice(&voice.info)
                .map_err(|error| synthesis("set voice", &error))?;
            self.applied_voice = Some(voice.id.clone());
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

    fn places_marks(&self) -> bool {
        true
    }

    fn speak(
        &mut self,
        sequence: &SpeechSequence,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError> {
        self.apply_voice()?;
        self.apply_options();

        let language = self
            .voices
            .iter()
            .find(|voice| voice.id == self.current_voice_id)
            .map_or("en-US", |voice| voice.language.as_str());
        let ssml = HSTRING::from(ssml(sequence, language));
        let operation = self
            .synth
            .SynthesizeSsmlToStreamAsync(&ssml)
            .map_err(|error| synthesis("start synthesis", &error))?;
        // Wait on this dedicated thread for the operation to complete, and
        // cancel it if the utterance is cancelled first: OneCore produces no
        // audio until it has synthesized the whole utterance, so there is no
        // push at which the driver would otherwise hear of the cancel.
        let (completed_tx, completed) = mpsc::channel();
        operation
            .when(move |result| {
                let _ = completed_tx.send(result);
            })
            .map_err(|error| synthesis("wait for synthesis", &error))?;
        let stream = loop {
            match completed.recv_timeout(CANCEL_CHECK) {
                Ok(result) => {
                    break result.map_err(|error| synthesis("finish synthesis", &error))?;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if sink.is_cancelled() {
                        let _ = operation.Cancel();
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(SynthError::Synthesis(
                        "synthesis ended without a result".to_owned(),
                    ));
                }
            }
        };
        let marks = read_marks(&stream)?;
        let wav = read_stream(&stream)?;
        let (format, samples) = parse_wav(&wav)?;

        let channels = usize::from(format.channels.max(1));
        let frames = samples.len() / channels;
        let mut pushed = 0;
        for (ticks, mark) in marks {
            let at = usize::try_from(
                ticks.saturating_mul(u64::from(format.sample_rate)) / TICKS_PER_SECOND,
            )
            .unwrap_or(usize::MAX)
            .clamp(pushed, frames);
            if push_frames(sink, format, &samples[pushed * channels..at * channels]).is_break() {
                return Ok(());
            }
            pushed = at;
            sink.index_reached(mark);
        }
        let _ = push_frames(sink, format, &samples[pushed * channels..]);
        Ok(())
    }
}

/// Pushes `samples` to the sink in chunks, so cancellation is seen between
/// them.
fn push_frames(sink: &mut dyn SynthSink, format: PcmFormat, samples: &[i16]) -> ControlFlow<()> {
    for chunk in samples.chunks(CHUNK_SAMPLES * usize::from(format.channels.max(1))) {
        sink.push_pcm(format, chunk)?;
    }
    ControlFlow::Continue(())
}

/// Builds the SSML for a sequence: its text, escaped, with each index mark
/// as a `<mark>` named by the mark's number.
fn ssml(sequence: &SpeechSequence, language: &str) -> String {
    let mut ssml = format!(
        "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" xml:lang=\"{}\">",
        escape(language)
    );
    for item in &sequence.items {
        match item {
            SpeechItem::Text(text) => ssml.push_str(&escape(text)),
            SpeechItem::Mark(mark) => {
                let _ = write!(ssml, "<mark name=\"{}\"/>", mark.0);
            }
            _ => {}
        }
    }
    ssml.push_str("</speak>");
    ssml
}

/// Escapes text for SSML. Characters XML 1.0 does not allow at all (most
/// control characters, and U+FFFE and U+FFFF), which window titles and
/// clipboard text can contain, become spaces, so they cannot make the whole
/// utterance fail to parse.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\t' | '\n' | '\r' => escaped.push(character),
            '\u{0}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}' => escaped.push(' '),
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Reads the index marks `OneCore` placed in the audio, as times in
/// `TimeSpan` ticks, in time order. Only marks whose name is a number are
/// ours.
fn read_marks(
    stream: &windows::Media::SpeechSynthesis::SpeechSynthesisStream,
) -> Result<Vec<(u64, IndexMark)>, SynthError> {
    let markers = stream
        .Markers()
        .map_err(|error| synthesis("read marks", &error))?;
    let mut marks = Vec::new();
    for marker in markers {
        let Ok(name) = marker.Text() else { continue };
        let Ok(mark) = name.to_string().parse::<u64>() else {
            continue;
        };
        let ticks = marker
            .Time()
            .map_err(|error| synthesis("read a mark's time", &error))?
            .Duration;
        marks.push((u64::try_from(ticks).unwrap_or(0), IndexMark(mark)));
    }
    marks.sort_by_key(|(ticks, _)| *ticks);
    Ok(marks)
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
    // The stream is already in memory, so loading it completes at once.
    let load: IAsyncOperation<u32> = reader
        .LoadAsync(size)
        .and_then(|load| load.cast())
        .map_err(|error| synthesis("load stream", &error))?;
    load.join()
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
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
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
    fn ssml_escapes_text_and_names_marks_by_number() {
        let sequence = SpeechSequence {
            utterance: verbatim_model::UtteranceId(1),
            trace_id: verbatim_model::TraceId::mint(),
            language: None,
            items: vec![
                SpeechItem::Text("Tom & Jerry <3\u{7}".to_owned()),
                SpeechItem::Mark(IndexMark(7)),
            ],
        };
        assert_eq!(
            ssml(&sequence, "en-US"),
            "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" xml:lang=\"en-US\">Tom &amp; Jerry &lt;3 <mark name=\"7\"/></speak>"
        );
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
