//! Conversion of a synthesizer's 16-bit PCM into the mixer's device format
//! (decision D17): 32-bit float at the device's sample rate and channel
//! count, so every source can be summed into one stream.
//!
//! Sample-rate conversion uses a band-limited sinc resampler in small fixed
//! input chunks, so it adds only a few milliseconds before an utterance's
//! first sample reaches the mixer. Each utterance is converted on its own:
//! [`Converter::finish`] flushes the resampler's tail and resets it, so one
//! utterance's audio never bleeds into the next.

use std::num::NonZeroUsize;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, WindowFunction};

use crate::numbers::{count_u64, float_to_u64, u64_to_f64};
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
            let output = vec![
                0.0;
                resampler
                    .output_frames_max()
                    .saturating_mul(usize::from(source.channels))
            ];
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
        let channels =
            NonZeroUsize::new(usize::from(self.source.channels)).unwrap_or(NonZeroUsize::MIN);
        // Whole frames only: a partial last frame is left out.
        let frames = samples.len() / channels;
        let input = samples
            .chunks_exact(channels.get())
            .flatten()
            .map(|&sample| f32::from(sample) * I16_SCALE);
        match &mut self.resampling {
            None => {
                let converted: Vec<f32> = input.collect();
                map_channels(&converted, self.source.channels, self.device.channels, out);
            }
            Some(resampling) => {
                resampling.pending.extend(input);
                resampling.frames_in = resampling.frames_in.saturating_add(count_u64(frames));
                let chunk = CHUNK_FRAMES.saturating_mul(channels.get());
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
        let channels =
            NonZeroUsize::new(usize::from(self.source.channels)).unwrap_or(NonZeroUsize::MIN);
        let expected = float_to_u64((u64_to_f64(resampling.frames_in) * resampling.ratio).round());
        // The partial chunk left over, then silence, until every output
        // frame the input implies has come out of the filter. A pass that
        // resamples nothing ends the utterance there, a little short,
        // rather than repeating forever.
        while resampling.frames_out < expected {
            let partial = resampling.pending.len() / channels;
            resampling
                .pending
                .resize(CHUNK_FRAMES.saturating_mul(channels.get()), 0.0);
            let frames = resampling.process(channels, partial);
            resampling.pending.clear();
            if frames == 0 {
                tracing::debug!(
                    frames_out = resampling.frames_out,
                    expected,
                    "the resampler made no frames; the utterance ends short"
                );
                break;
            }
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
    fn process(&mut self, channels: NonZeroUsize, valid: usize) -> usize {
        let capacity = self.output.len() / channels;
        // The pending chunk holds at least CHUNK_FRAMES frames, and the
        // output buffer `output_frames_max` frames; with either short,
        // nothing is resampled.
        let length = CHUNK_FRAMES.saturating_mul(channels.get());
        let Some(chunk) = self.pending.get(..length) else {
            tracing::debug!(
                length,
                pending = self.pending.len(),
                "a resampler chunk short of input"
            );
            return 0;
        };
        let input = match InterleavedSlice::new(chunk, channels.get(), CHUNK_FRAMES) {
            Ok(input) => input,
            Err(error) => {
                tracing::debug!(%error, "a resampler chunk that is not a buffer");
                return 0;
            }
        };
        let mut output = match InterleavedSlice::new_mut(&mut self.output, channels.get(), capacity)
        {
            Ok(output) => output,
            Err(error) => {
                tracing::debug!(%error, "a resampler output that is not a buffer");
                return 0;
            }
        };
        let indexing = Indexing::new().partial_len(valid);
        match self
            .resampler
            .process_into_buffer(&input, &mut output, Some(&indexing))
        {
            Ok((_read, written)) => written,
            // The buffers are sized from the resampler's own maxima, so this
            // cannot happen; dropping the chunk keeps audio flowing if it
            // ever does.
            Err(error) => {
                tracing::debug!(%error, "the resampler failed on a chunk");
                0
            }
        }
    }

    /// Appends `frames` output frames to `out`, after discarding what is
    /// left of the start-up delay and anything beyond `limit` frames.
    fn emit(
        &mut self,
        frames: usize,
        channels: NonZeroUsize,
        limit: Option<u64>,
        device_channels: u16,
        out: &mut Vec<f32>,
    ) {
        let skip = self.delay_left.min(frames);
        self.delay_left = self.delay_left.saturating_sub(skip);
        let mut keep = frames.saturating_sub(skip);
        if let Some(limit) = limit {
            let room = usize::try_from(limit.saturating_sub(self.frames_out)).unwrap_or(usize::MAX);
            keep = keep.min(room);
        }
        self.frames_out = self.frames_out.saturating_add(count_u64(keep));
        let start = skip.saturating_mul(channels.get());
        let end = keep.saturating_mul(channels.get()).saturating_add(start);
        let source_channels = u16::try_from(channels.get()).unwrap_or(u16::MAX);
        // The resampler writes at most the output buffer's frames.
        let Some(kept) = self.output.get(start..end) else {
            tracing::debug!(
                start,
                end,
                len = self.output.len(),
                "resampled frames past the buffer"
            );
            return;
        };
        map_channels(kept, source_channels, device_channels, out);
    }
}

/// Appends interleaved `input` frames with `from` channels to `out` as
/// frames with `to` channels: mono is copied to every output channel, any
/// layout is averaged down to mono, and otherwise output channel `c` takes
/// input channel `c` modulo `from`.
fn map_channels(input: &[f32], from: u16, to: u16, out: &mut Vec<f32>) {
    let scale = 1.0 / f32::from(from);
    let from = usize::from(from);
    let to = usize::from(to);
    if from == to {
        out.extend_from_slice(input);
        return;
    }
    for frame in input.chunks_exact(from) {
        if to == 1 {
            out.push(frame.iter().sum::<f32>() * scale);
        } else {
            // Output channel `c` takes input channel `c` modulo `from`.
            out.extend(frame.iter().cycle().take(to));
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

    #[test]
    fn finish_ends_when_the_resampler_makes_no_frames() {
        let source = PcmFormat {
            sample_rate: 22_050,
            channels: 1,
        };
        let mut converter = Converter::new(source, device(48_000, 1)).expect("builds");
        let mut out = Vec::new();
        converter.push(&[16_384; 32], &mut out);
        // An output buffer with no room: every resampler call makes nothing.
        converter
            .resampling
            .as_mut()
            .expect("resamples")
            .output
            .clear();
        converter.finish(&mut out);
        assert_eq!(out, Vec::<f32>::new());

        // The converter is reset for the next utterance.
        let resampling = converter.resampling.as_ref().expect("resamples");
        assert_eq!(
            (
                resampling.frames_in,
                resampling.frames_out,
                resampling.pending.len()
            ),
            (0, 0, 0)
        );
    }
}
