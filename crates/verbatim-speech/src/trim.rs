//! Silence trimming for every synthesizer (decision D17).
//!
//! Synthesizers commonly begin an utterance with silence and end it with
//! more. Leading silence delays the first word; trailing silence delays the
//! utterance's ending and the start of the next one. The trimmer drops
//! quiet frames before the first audible frame, and holds back each run of
//! quiet frames after it: a run is released when audible audio follows (so
//! pauses between words and sentences stay), and dropped when the utterance
//! ends. Index marks keep their place relative to the audio around them.

use verbatim_audio::PcmFormat;

use crate::driver::IndexMark;

/// A sample this close to zero is silence: about -54 dBFS, well below the
/// quietest speech sound a synthesizer produces.
const QUIET: i16 = 64;

/// The most quiet audio held back at once: two seconds at 48 kHz stereo. A
/// longer quiet run is released as it is, so memory stays bounded however
/// a synthesizer pauses.
const MAX_HELD_SAMPLES: usize = 192_000;

/// Output of the trimmer, in order.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    /// Audio to play.
    Pcm(PcmFormat, Vec<i16>),
    /// A mark to place after the audio before it.
    Mark(IndexMark),
}

/// Trims one utterance's audio.
#[derive(Default)]
pub(crate) struct Trimmer {
    /// An audible frame has been passed on.
    started: bool,
    /// Quiet audio held back since the last audible frame.
    held: Vec<i16>,
    held_format: Option<PcmFormat>,
    /// Marks reached during the held audio, with the held length at the
    /// time.
    held_marks: Vec<(usize, IndexMark)>,
}

impl Trimmer {
    /// Takes the next PCM and returns what can be passed on now.
    pub(crate) fn push(&mut self, format: PcmFormat, samples: &[i16]) -> Vec<Piece> {
        let mut out = Vec::new();
        if self.held_format.is_some_and(|held| held != format) {
            // A format change mid-utterance: what was held is real audio.
            self.release(&mut out);
        }
        let channels = usize::from(format.channels.max(1));
        let whole = samples.len() - samples.len() % channels;
        let mut audible = Vec::new();
        for frame in samples[..whole].chunks_exact(channels) {
            let quiet = frame
                .iter()
                .all(|sample| sample.unsigned_abs() <= QUIET.unsigned_abs());
            if quiet {
                if !self.started {
                    continue;
                }
                if !audible.is_empty() {
                    out.push(Piece::Pcm(format, std::mem::take(&mut audible)));
                }
                self.held.extend_from_slice(frame);
                self.held_format = Some(format);
                if self.held.len() > MAX_HELD_SAMPLES {
                    self.release(&mut out);
                }
            } else {
                self.started = true;
                if !self.held.is_empty() || !self.held_marks.is_empty() {
                    self.release(&mut out);
                }
                audible.extend_from_slice(frame);
            }
        }
        if !audible.is_empty() {
            out.push(Piece::Pcm(format, audible));
        }
        out
    }

    /// Takes a mark reached after the PCM pushed so far.
    pub(crate) fn mark(&mut self, mark: IndexMark) -> Vec<Piece> {
        if self.held.is_empty() {
            vec![Piece::Mark(mark)]
        } else {
            self.held_marks.push((self.held.len(), mark));
            Vec::new()
        }
    }

    /// Ends the utterance: held quiet audio is dropped, and marks reached
    /// within it are passed on, at the end.
    pub(crate) fn finish(&mut self) -> Vec<Piece> {
        self.held.clear();
        self.held_format = None;
        let pieces = self
            .held_marks
            .drain(..)
            .map(|(_, mark)| Piece::Mark(mark))
            .collect();
        self.started = false;
        pieces
    }

    /// Passes on everything held, marks in their places.
    fn release(&mut self, out: &mut Vec<Piece>) {
        let Some(format) = self.held_format.take() else {
            out.extend(self.held_marks.drain(..).map(|(_, mark)| Piece::Mark(mark)));
            return;
        };
        let held = std::mem::take(&mut self.held);
        let mut from = 0;
        for (at, mark) in self.held_marks.drain(..) {
            if at > from {
                out.push(Piece::Pcm(format, held[from..at].to_vec()));
                from = at;
            }
            out.push(Piece::Mark(mark));
        }
        if held.len() > from {
            out.push(Piece::Pcm(format, held[from..].to_vec()));
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

    fn pcm(samples: &[i16]) -> Piece {
        Piece::Pcm(MONO, samples.to_vec())
    }

    #[test]
    fn leading_and_trailing_silence_go_and_inner_pauses_stay() {
        let mut trimmer = Trimmer::default();
        let mut out = trimmer.push(MONO, &[0, 3, 500, 600, 0, 0, 700, 0, 0]);
        out.extend(trimmer.finish());
        assert_eq!(out, vec![pcm(&[500, 600]), pcm(&[0, 0]), pcm(&[700])]);
    }

    #[test]
    fn marks_keep_their_place_inside_a_pause_and_inside_trailing_silence() {
        let mut trimmer = Trimmer::default();
        let mut out = trimmer.push(MONO, &[500, 0]);
        out.extend(trimmer.mark(IndexMark(1)));
        out.extend(trimmer.push(MONO, &[0, 600, 0]));
        out.extend(trimmer.mark(IndexMark(2)));
        out.extend(trimmer.finish());
        assert_eq!(
            out,
            vec![
                pcm(&[500]),
                pcm(&[0]),
                Piece::Mark(IndexMark(1)),
                pcm(&[0]),
                pcm(&[600]),
                Piece::Mark(IndexMark(2)),
            ]
        );
    }

    #[test]
    fn an_utterance_of_silence_produces_no_audio() {
        let mut trimmer = Trimmer::default();
        let mut out = trimmer.push(MONO, &[0; 100]);
        out.extend(trimmer.mark(IndexMark(7)));
        out.extend(trimmer.finish());
        assert_eq!(out, vec![Piece::Mark(IndexMark(7))]);
    }
}
