//! Conversion of a synthesizer's 16-bit PCM into the mixer's device format
//! (decision D17): 32-bit float at the device's sample rate and channel
//! count, so every source can be summed into one stream.
//!
//! Sample-rate conversion uses a band-limited sinc resampler in small fixed
//! input chunks, so it adds only a few milliseconds before an utterance's
//! first sample reaches the mixer. Each utterance is converted on its own:
//! [`Converter::finish`] flushes the resampler's tail and resets it, so one
//! utterance's audio never bleeds into the next.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, WindowFunction};

use crate::{AudioError, DeviceFormat, PcmFormat};

/// Input frames per resampler call. Small, because a chunk must fill before
/// any of it is converted: 64 frames is under 3 ms at 22050 Hz.
const CHUNK_FRAMES: usize = 64;

/// Length of the sinc interpolation filter. Speech needs far less than
/// music; 64 keeps the filter's own delay near one millisecond.
const SINC_LEN: usize = 64;

/// Scale from 16-bit integer samples to the mixer's float range.
const I16_SCALE: f32 = 1.0 / 32_768.0;

/// Converts one source's PCM, utterance by utterance, into device frames.
pub(crate) struct Converter {
    source: PcmFormat,
    device: DeviceFormat,
    resampling: Option<Resampling>,
}

/// Resampler state for a source whose rate differs from the device's.
struct Resampling {
    resampler: Async<f32>,
    /// Interleaved input at the source's channel count, waiting for a full
    /// chunk.
    pending: Vec<f32>,
    /// Scratch output for one resampler call.
    output: Vec<f32>,
    /// Output frames of the resampler's start-up delay still to discard.
    delay_left: usize,
    /// Input frames accepted for the current utterance.
    frames_in: u64,
    /// Output frames emitted for the current utterance, after the delay.
    frames_out: u64,
    ratio: f64,
}

impl Converter {
    /// A converter from `source` to `device`.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] for a format with no channels or no
    /// sample rate, or when the resampler cannot be built for the ratio.
    pub(crate) fn new(source: PcmFormat, device: DeviceFormat) -> Result<Self, AudioError> {
        if source.channels == 0 || source.sample_rate == 0 {
            return Err(AudioError::Stream(format!(
                "unusable source format: {} Hz, {} channels",
                source.sample_rate, source.channels
            )));
        }
        let resampling = if source.sample_rate == device.sample_rate {
            None
        } else {
            let ratio = f64::from(device.sample_rate) / f64::from(source.sample_rate);
            let parameters =
                SincInterpolationParameters::new(SINC_LEN, WindowFunction::BlackmanHarris2);
            let resampler = Async::<f32>::new_sinc(
                ratio,
                1.0,
                &parameters,
                CHUNK_FRAMES,
                usize::from(source.channels),
                FixedAsync::Input,
            )
            .map_err(|error| AudioError::Stream(format!("build resampler: {error}")))?;
            let output = vec![0.0; resampler.output_frames_max() * usize::from(source.channels)];
            let delay_left = resampler.output_delay();
            Some(Resampling {
                resampler,
                pending: Vec::new(),
                output,
                delay_left,
                frames_in: 0,
                frames_out: 0,
                ratio,
            })
        };
        Ok(Self {
            source,
            device,
            resampling,
        })
    }

    /// The source format this converter accepts.
    pub(crate) fn source(&self) -> PcmFormat {
        self.source
    }

    /// Converts `samples` (interleaved, in the source format) and appends
    /// the device frames available so far to `out`.
    pub(crate) fn push(&mut self, samples: &[i16], out: &mut Vec<f32>) {
        let channels = usize::from(self.source.channels);
        let whole = samples.len() - samples.len() % channels;
        let input = samples[..whole]
            .iter()
            .map(|&sample| f32::from(sample) * I16_SCALE);
        match &mut self.resampling {
            None => {
                let converted: Vec<f32> = input.collect();
                map_channels(&converted, self.source.channels, self.device.channels, out);
            }
            Some(resampling) => {
                resampling.pending.extend(input);
                resampling.frames_in += (whole / channels) as u64;
                let chunk = CHUNK_FRAMES * channels;
                while resampling.pending.len() >= chunk {
                    let frames = resampling.process(channels, CHUNK_FRAMES);
                    resampling.pending.drain(..chunk);
                    resampling.emit(frames, channels, None, self.device.channels, out);
                }
            }
        }
    }

    /// Ends the current utterance: flushes the resampler so the
    /// utterance's last input frames reach `out`, then resets it for the
    /// next utterance.
    pub(crate) fn finish(&mut self, out: &mut Vec<f32>) {
        let Some(resampling) = &mut self.resampling else {
            return;
        };
        let channels = usize::from(self.source.channels);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "frame counts of one utterance are far below 2^52"
        )]
        let expected = (resampling.frames_in as f64 * resampling.ratio).round() as u64;
        // The partial chunk left over, then silence, until every output
        // frame the input implies has come out of the filter.
        while resampling.frames_out < expected {
            let partial = resampling.pending.len() / channels;
            resampling.pending.resize(CHUNK_FRAMES * channels, 0.0);
            let frames = resampling.process(channels, partial);
            resampling.pending.clear();
            resampling.emit(frames, channels, Some(expected), self.device.channels, out);
        }
        resampling.resampler.reset();
        resampling.pending.clear();
        resampling.delay_left = resampling.resampler.output_delay();
        resampling.frames_in = 0;
        resampling.frames_out = 0;
    }
}

impl Resampling {
    /// Runs one resampler call over the first chunk of `pending`, of which
    /// `valid` frames are real input (the rest is padding). Returns the
    /// number of output frames written to `output`.
    fn process(&mut self, channels: usize, valid: usize) -> usize {
        let capacity = self.output.len() / channels;
        let input = InterleavedSlice::new(
            &self.pending[..CHUNK_FRAMES * channels],
            channels,
            CHUNK_FRAMES,
        )
        .expect("the pending chunk holds exactly CHUNK_FRAMES frames");
        let mut output = InterleavedSlice::new_mut(&mut self.output, channels, capacity)
            .expect("the output buffer holds output_frames_max frames");
        let indexing = Indexing::new().partial_len(valid);
        match self
            .resampler
            .process_into_buffer(&input, &mut output, Some(&indexing))
        {
            Ok((_read, written)) => written,
            // The buffers are sized from the resampler's own maxima, so this
            // cannot happen; dropping the chunk keeps audio flowing if it
            // ever does.
            Err(_) => 0,
        }
    }

    /// Appends `frames` output frames to `out`, after discarding what is
    /// left of the start-up delay and anything beyond `limit` frames.
    fn emit(
        &mut self,
        frames: usize,
        channels: usize,
        limit: Option<u64>,
        device_channels: u16,
        out: &mut Vec<f32>,
    ) {
        let skip = self.delay_left.min(frames);
        self.delay_left -= skip;
        let mut keep = frames - skip;
        if let Some(limit) = limit {
            let room = usize::try_from(limit.saturating_sub(self.frames_out)).unwrap_or(usize::MAX);
            keep = keep.min(room);
        }
        self.frames_out += keep as u64;
        let start = skip * channels;
        let source_channels = u16::try_from(channels).unwrap_or(u16::MAX);
        map_channels(
            &self.output[start..start + keep * channels],
            source_channels,
            device_channels,
            out,
        );
    }
}

/// Appends interleaved `input` frames with `from` channels to `out` as
/// frames with `to` channels: mono is copied to every output channel, any
/// layout is averaged down to mono, and otherwise output channel `c` takes
/// input channel `c` modulo `from`.
fn map_channels(input: &[f32], from: u16, to: u16, out: &mut Vec<f32>) {
    let from = usize::from(from);
    let to = usize::from(to);
    if from == to {
        out.extend_from_slice(input);
        return;
    }
    #[expect(clippy::cast_precision_loss, reason = "channel counts are tiny")]
    let scale = 1.0 / from as f32;
    for frame in input.chunks_exact(from) {
        if to == 1 {
            out.push(frame.iter().sum::<f32>() * scale);
        } else {
            out.extend((0..to).map(|channel| frame[channel % from]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(sample_rate: u32, channels: u16) -> DeviceFormat {
        DeviceFormat {
            sample_rate,
            channels,
            buffer_frames: 1_920,
        }
    }

    #[test]
    fn same_rate_maps_mono_to_every_device_channel() {
        let source = PcmFormat {
            sample_rate: 48_000,
            channels: 1,
        };
        let mut converter = Converter::new(source, device(48_000, 2)).expect("builds");
        let mut out = Vec::new();
        converter.push(&[16_384, -16_384], &mut out);
        converter.finish(&mut out);
        assert_eq!(out, vec![0.5, 0.5, -0.5, -0.5]);
    }

    #[test]
    fn resampling_keeps_an_utterance_length_and_starts_without_delay() {
        let source = PcmFormat {
            sample_rate: 22_050,
            channels: 1,
        };
        let mut converter = Converter::new(source, device(48_000, 1)).expect("builds");
        // 100 ms of a constant level: the output must be 100 ms long at the
        // new rate, and its first frames must already carry the level
        // rather than the filter's start-up silence.
        let mut out = Vec::new();
        converter.push(&vec![16_384; 2_205], &mut out);
        converter.finish(&mut out);
        assert_eq!(out.len(), 4_800);
        let middle = out[2_400];
        assert!(
            (middle - 0.5).abs() < 0.01,
            "steady level preserved, got {middle}"
        );
        assert!(
            out[40] > 0.25,
            "no start-up delay left at the front, got {}",
            out[40]
        );

        // The converter is ready for the next utterance at once.
        let mut next = Vec::new();
        converter.push(&vec![0; 2_205], &mut next);
        converter.finish(&mut next);
        assert_eq!(next.len(), 4_800);
        assert!(
            next.iter().all(|sample| sample.abs() < 0.01),
            "nothing carried over"
        );
    }
}
