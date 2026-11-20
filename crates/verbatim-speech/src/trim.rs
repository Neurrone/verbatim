//! Leading silence trimming for every synthesizer.
//!
//! Synthesizers commonly begin an utterance with silence, which delays the
//! first word. The trimmer drops quiet frames before an utterance's first
//! audible frame and passes everything after it, so pauses between words
//! and the synthesizer's own trailing audio play as it made them, as NVDA
//! trims only leading silence (`docs/nvda/audio.md`). Index marks are
//! passed on as they arrive: one reached in the leading silence is placed
//! before the first audio.

use verbatim_audio::PcmFormat;

/// A sample this close to zero is silence: about -54 dBFS, well below the
/// quietest speech sound a synthesizer produces.
const QUIET: u16 = 64;

/// Trims one utterance's leading silence.
#[derive(Default)]
pub(crate) struct Trimmer {
    /// An audible frame has been passed on.
    started: bool,
}

impl Trimmer {
    /// The part of `samples` to play: nothing before the utterance's first
    /// audible frame, and everything from it on.
    pub(crate) fn push<'a>(&mut self, format: PcmFormat, samples: &'a [i16]) -> &'a [i16] {
        if self.started {
            return samples;
        }
        let channels = usize::from(format.channels.max(1));
        let first = samples
            .chunks_exact(channels)
            .position(|frame| frame.iter().any(|sample| sample.unsigned_abs() > QUIET));
        match first.and_then(|frame| samples.get(frame.saturating_mul(channels)..)) {
            Some(audible) => {
                self.started = true;
                audible
            }
            None => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONO: PcmFormat = PcmFormat {
        sample_rate: 22_050,
        channels: 1,
    };

    #[test]
    fn only_leading_silence_goes() {
        let mut trimmer = Trimmer::default();
        let mut out = trimmer.push(MONO, &[0, 3]).to_vec();
        out.extend(trimmer.push(MONO, &[0, 500, 600, 0, 0, 700, 0, 0]));
        out.extend(trimmer.push(MONO, &[0, 0]));
        assert_eq!(out, [500, 600, 0, 0, 700, 0, 0, 0, 0]);
    }

    #[test]
    fn an_utterance_of_silence_produces_no_audio() {
        let mut trimmer = Trimmer::default();
        assert_eq!(trimmer.push(MONO, &[0; 100]), &[] as &[i16]);
    }
}
