//! Sounds (`phase6-design.md`, "Earcons"): short recorded sounds decoded
//! once from WAV files, and tones generated in code. A sound is mixed by the
//! mixer like speech (decision D16), either at its place in an utterance
//! ([`Source::sound`](crate::Source::sound)) or at once
//! ([`Source::play`](crate::Source::play)), and is converted to the
//! device's format the first time it is played in that format.

use std::f64::consts::TAU;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::convert::Converter;
use crate::{AudioError, DeviceFormat, PcmFormat};

/// The longest sound accepted: a minute. Theme sounds are short cues; this
/// keeps a mistaken file from filling memory.
const MAX_SECONDS: u32 = 60;

/// The sample rate tones are generated at.
const TONE_RATE: u32 = 48_000;

/// A tone's peak amplitude, about 10 dB below full scale.
const TONE_AMPLITUDE: f64 = 0.3;

/// The longest fade in and out of a tone, so it does not click.
const TONE_FADE_MS: u32 = 5;

/// The range of tone frequencies accepted, in hertz.
const TONE_FREQUENCIES: std::ops::RangeInclusive<u32> = 20..=20_000;

/// The longest tone accepted, in milliseconds.
const MAX_TONE_MS: u32 = 5_000;

/// A sound, decoded to 16-bit PCM. Shared as `Arc<Sound>`: the speech
/// pipeline and the theme hold the same decoded sound.
pub struct Sound {
    format: PcmFormat,
    samples: Arc<[i16]>,
    /// The sound converted to the last device format it was played in.
    converted: Mutex<Option<Converted>>,
}

struct Converted {
    sample_rate: u32,
    channels: u16,
    frames: Arc<[f32]>,
}

impl Sound {
    /// A sound from interleaved PCM samples.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] for a format with no channels or no
    /// sample rate, or a sound longer than a minute.
    pub fn from_pcm(format: PcmFormat, samples: Vec<i16>) -> Result<Self, AudioError> {
        if format.channels == 0 || format.sample_rate == 0 {
            return Err(AudioError::Stream(format!(
                "unusable sound format: {} Hz, {} channels",
                format.sample_rate, format.channels
            )));
        }
        let limit =
            u64::from(format.sample_rate) * u64::from(format.channels) * u64::from(MAX_SECONDS);
        if samples.len() as u64 > limit {
            return Err(AudioError::Stream(format!(
                "a sound may be at most {MAX_SECONDS} seconds long"
            )));
        }
        Ok(Self {
            format,
            samples: samples.into(),
            converted: Mutex::new(None),
        })
    }

    /// Decodes a WAV file's contents: PCM at 8, 16, 24, or 32 bits, or
    /// 32-bit float, at any rate and channel count, kept as 16-bit.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] when the data is not a WAV file this
    /// can decode, or the sound is unusable as [`from_pcm`](Self::from_pcm)
    /// says.
    pub fn from_wav(reader: impl Read) -> Result<Self, AudioError> {
        let invalid =
            |error: hound::Error| AudioError::Stream(format!("not a usable WAV: {error}"));
        let mut wav = hound::WavReader::new(reader).map_err(invalid)?;
        let spec = wav.spec();
        let format = PcmFormat {
            sample_rate: spec.sample_rate,
            channels: spec.channels,
        };
        let limit = u64::from(spec.sample_rate) * u64::from(spec.channels) * u64::from(MAX_SECONDS);
        if u64::from(wav.len()) > limit {
            return Err(AudioError::Stream(format!(
                "a sound may be at most {MAX_SECONDS} seconds long"
            )));
        }
        let samples: Vec<i16> = match (spec.sample_format, spec.bits_per_sample) {
            (hound::SampleFormat::Float, 32) => wav
                .samples::<f32>()
                .map(|sample| sample.map(float_to_i16))
                .collect::<Result<_, _>>()
                .map_err(invalid)?,
            (hound::SampleFormat::Int, bits @ 1..=32) => {
                let shift = i32::from(bits) - 16;
                wav.samples::<i32>()
                    .map(|sample| sample.map(|sample| int_to_i16(sample, shift)))
                    .collect::<Result<_, _>>()
                    .map_err(invalid)?
            }
            (format, bits) => {
                return Err(AudioError::Stream(format!(
                    "unsupported WAV sample format: {format:?} at {bits} bits"
                )));
            }
        };
        Self::from_pcm(format, samples)
    }

    /// Decodes a WAV file, as [`from_wav`](Self::from_wav).
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] when the file cannot be opened or
    /// decoded.
    pub fn from_wav_file(path: &Path) -> Result<Self, AudioError> {
        let file = std::fs::File::open(path)
            .map_err(|error| AudioError::Stream(format!("{}: {error}", path.display())))?;
        Self::from_wav(std::io::BufReader::new(file))
    }

    /// A sine tone at `frequency_hz` lasting `duration_ms`, fading in and
    /// out over at most 5 ms so it does not click.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] for a frequency outside 20 Hz to
    /// 20 kHz or a duration of zero or above five seconds.
    pub fn tone(frequency_hz: u32, duration_ms: u32) -> Result<Self, AudioError> {
        if !TONE_FREQUENCIES.contains(&frequency_hz)
            || duration_ms == 0
            || duration_ms > MAX_TONE_MS
        {
            return Err(AudioError::Stream(format!(
                "unusable tone: {frequency_hz} Hz for {duration_ms} ms"
            )));
        }
        let frames = TONE_RATE / 1_000 * duration_ms;
        let fade = (TONE_RATE / 1_000 * TONE_FADE_MS).min(frames / 2).max(1);
        let step = TAU * f64::from(frequency_hz) / f64::from(TONE_RATE);
        let samples = (0..frames)
            .map(|frame| {
                let edge = frame.min(frames - 1 - frame);
                let envelope = f64::from(edge.min(fade)) / f64::from(fade);
                let value = (step * f64::from(frame)).sin() * TONE_AMPLITUDE * envelope;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "the value is within -1 to 1, scaled to i16"
                )]
                let sample = (value * f64::from(i16::MAX)).round() as i16;
                sample
            })
            .collect();
        Self::from_pcm(
            PcmFormat {
                sample_rate: TONE_RATE,
                channels: 1,
            },
            samples,
        )
    }

    /// The sound's own format.
    #[must_use]
    pub fn format(&self) -> PcmFormat {
        self.format
    }

    /// The sound's samples, interleaved, in its own format.
    #[must_use]
    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    /// How long the sound plays.
    #[must_use]
    pub fn duration(&self) -> Duration {
        let frames = self.samples.len() as u64 / u64::from(self.format.channels);
        Duration::from_micros(frames * 1_000_000 / u64::from(self.format.sample_rate))
    }

    /// The sound in `device`'s format, converted the first time it is asked
    /// for in that format.
    pub(crate) fn frames_for(&self, device: DeviceFormat) -> Result<Arc<[f32]>, AudioError> {
        let mut cache = self
            .converted
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(converted) = cache.as_ref()
            && converted.sample_rate == device.sample_rate
            && converted.channels == device.channels
        {
            return Ok(Arc::clone(&converted.frames));
        }
        let mut converter = Converter::new(self.format, device)?;
        let mut frames = Vec::new();
        converter.push(&self.samples, &mut frames);
        converter.finish(&mut frames);
        let frames: Arc<[f32]> = frames.into();
        *cache = Some(Converted {
            sample_rate: device.sample_rate,
            channels: device.channels,
            frames: Arc::clone(&frames),
        });
        Ok(frames)
    }
}

impl fmt::Debug for Sound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sound")
            .field("format", &self.format)
            .field("duration", &self.duration())
            .finish_non_exhaustive()
    }
}

/// A float sample as 16-bit.
fn float_to_i16(sample: f32) -> i16 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped to the i16 range first"
    )]
    let sample = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
    sample
}

/// An integer sample of `16 + shift` bits as 16-bit.
fn int_to_i16(sample: i32, shift: i32) -> i16 {
    let scaled = if shift >= 0 {
        sample >> shift
    } else {
        sample << -shift
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped to the i16 range first"
    )]
    let sample = scaled.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
    sample
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV file in memory, written with hound.
    fn wav(
        spec: hound::WavSpec,
        write: impl FnOnce(&mut hound::WavWriter<&mut std::io::Cursor<Vec<u8>>>),
    ) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).expect("writer");
            write(&mut writer);
            writer.finalize().expect("finalize");
        }
        cursor.into_inner()
    }

    #[test]
    fn sixteen_bit_stereo_is_decoded_as_it_is() {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 22_050,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let data = wav(spec, |writer| {
            for sample in [1_i16, -2, 300, -400] {
                writer.write_sample(sample).expect("sample");
            }
        });
        let sound = Sound::from_wav(data.as_slice()).expect("decodes");
        assert_eq!(
            sound.format(),
            PcmFormat {
                sample_rate: 22_050,
                channels: 2
            }
        );
        assert_eq!(&*sound.samples, &[1, -2, 300, -400]);
    }

    #[test]
    fn eight_and_twenty_four_bit_and_float_become_sixteen_bit() {
        let int = |bits| hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: bits,
            sample_format: hound::SampleFormat::Int,
        };
        let eight = wav(int(8), |writer| writer.write_sample(64_i8).expect("sample"));
        assert_eq!(
            &*Sound::from_wav(eight.as_slice()).expect("8").samples,
            &[64 << 8]
        );
        let twenty_four = wav(int(24), |writer| {
            writer.write_sample(0x12_3456_i32).expect("sample");
        });
        assert_eq!(
            &*Sound::from_wav(twenty_four.as_slice()).expect("24").samples,
            &[0x1234]
        );
        let float = wav(
            hound::WavSpec {
                channels: 1,
                sample_rate: 8_000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
            |writer| writer.write_sample(-1.0_f32).expect("sample"),
        );
        assert_eq!(
            &*Sound::from_wav(float.as_slice()).expect("float").samples,
            &[-i16::MAX]
        );
    }

    #[test]
    fn what_is_not_a_wav_is_refused() {
        assert!(Sound::from_wav(&b"RIFF nonsense"[..]).is_err());
    }

    #[test]
    fn a_tone_fades_in_and_out() {
        let tone = Sound::tone(1_000, 40).expect("tone");
        assert_eq!(tone.duration(), Duration::from_millis(40));
        assert_eq!(tone.samples.first(), Some(&0));
        assert_eq!(tone.samples.last(), Some(&0));
        let peak = tone
            .samples
            .iter()
            .map(|sample| sample.unsigned_abs())
            .max();
        assert!(peak > Some(9_000) && peak < Some(10_000), "{peak:?}");
        assert!(Sound::tone(10, 40).is_err());
        assert!(Sound::tone(1_000, 0).is_err());
    }

    #[test]
    fn a_sound_is_converted_to_the_device_format_once() {
        let sound = Sound::from_pcm(
            PcmFormat {
                sample_rate: 1_000,
                channels: 1,
            },
            vec![16_384; 4],
        )
        .expect("sound");
        let device = DeviceFormat {
            sample_rate: 1_000,
            channels: 2,
            buffer_frames: 10,
        };
        let first = sound.frames_for(device).expect("converts");
        assert_eq!(&*first, &[0.5; 8]);
        let second = sound.frames_for(device).expect("converts");
        assert!(Arc::ptr_eq(&first, &second));
    }
}
