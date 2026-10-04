//! The synth driver contract.
//!
//! Deliberately synchronous: a driver's [`speak`](SynthDriver::speak) blocks
//! on its own dedicated synth thread, pushing PCM into a [`SynthSink`] and
//! honoring cooperative cancellation through the sink's return value. This
//! shape maps one-to-one onto a Wasm component world later (an exported
//! `speak` calling host imports for PCM and index marks) and onto the
//! synthesizer host process (decision D18) — Verbatim owns all threading, a
//! driver owns none.
//!
//! A driver only produces PCM and never plays it (decision D17): when an
//! utterance has been heard, and when playback reaches each index mark, is
//! measured by the audio mixer, for every driver alike.

use std::fmt;
use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};
use verbatim_audio::PcmFormat;
use verbatim_model::{TraceId, UtteranceId};

use crate::settings::{SettingDescriptor, SettingId, SettingValue, SynthId};

/// An index mark inside a speech sequence, reported when playback reaches
/// it; the basis for say-all continuation, braille sync, and latency
/// probes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexMark(pub u64);

/// One item of a [`SpeechSequence`].
///
/// Sounds will join as a further variant when earcons arrive (decision
/// D17); a driver never sees them, since the speech manager plays them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SpeechItem {
    /// Text to speak.
    Text(String),
    /// A point to report when playback reaches it.
    Mark(IndexMark),
    /// Speak what follows with the pitch setting raised (or lowered) by
    /// this much from its configured value; `0` returns to it. Drivers
    /// never receive these: the manager splits the sequence at them and
    /// changes the driver's `pitch` setting between the pieces.
    Pitch(i32),
}

/// What a synthesizer is asked to speak: the flattened form of one
/// utterance, after the theme (decision D12) and before synthesis. Plain
/// data, so it can cross a process, Wasm, or network boundary unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechSequence {
    /// The utterance this is.
    pub utterance: UtteranceId,
    /// The trace this speech belongs to.
    pub trace_id: TraceId,
    /// BCP 47 language tag, when known; drivers pick a matching voice when
    /// they can.
    pub language: Option<String>,
    /// Text and commands, in order.
    pub items: Vec<SpeechItem>,
}

impl SpeechSequence {
    /// The sequence's text alone, every text item joined in order.
    #[must_use]
    pub fn text(&self) -> String {
        self.items
            .iter()
            .filter_map(|item| match item {
                SpeechItem::Text(text) => Some(text.as_str()),
                SpeechItem::Mark(_) | SpeechItem::Pitch(_) => None,
            })
            .collect()
    }

    /// Whether the sequence holds any index mark.
    #[must_use]
    pub fn has_marks(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item, SpeechItem::Mark(_)))
    }

    /// Whether the sequence holds any pitch change.
    #[must_use]
    pub fn has_pitch_changes(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item, SpeechItem::Pitch(_)))
    }

    /// Splits the sequence at its marks, for a driver that cannot place
    /// marks in its audio: each piece is followed by the mark that ended
    /// it, and the last piece by none.
    #[must_use]
    pub fn split_at_marks(&self) -> Vec<(Self, Option<IndexMark>)> {
        self.split(true)
            .into_iter()
            .map(|(piece, after)| {
                let mark = match after {
                    Some(SpeechItem::Mark(mark)) => Some(mark),
                    _ => None,
                };
                (piece, mark)
            })
            .collect()
    }

    /// Splits the sequence at its pitch changes, and at its marks too when
    /// `at_marks`: each piece is followed by the item that ended it, and
    /// the last piece by none. Marks not split at stay in their pieces.
    #[must_use]
    pub fn split(&self, at_marks: bool) -> Vec<(Self, Option<SpeechItem>)> {
        let mut pieces = Vec::new();
        let mut items = Vec::new();
        for item in &self.items {
            match item {
                SpeechItem::Mark(_) if at_marks => {
                    pieces.push((
                        self.with_items(std::mem::take(&mut items)),
                        Some(item.clone()),
                    ));
                }
                SpeechItem::Pitch(_) => {
                    pieces.push((
                        self.with_items(std::mem::take(&mut items)),
                        Some(item.clone()),
                    ));
                }
                other => items.push(other.clone()),
            }
        }
        pieces.push((self.with_items(items), None));
        pieces
    }

    fn with_items(&self, items: Vec<SpeechItem>) -> Self {
        Self {
            utterance: self.utterance,
            trace_id: self.trace_id,
            language: self.language.clone(),
            items,
        }
    }
}

/// Error from a synth driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SynthError {
    /// The synthesizer cannot be initialized or has stopped working.
    Unavailable(String),
    /// A setting id or value the driver does not accept.
    Setting(String),
    /// Synthesis of one request failed.
    Synthesis(String),
}

impl fmt::Display for SynthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(detail) => write!(f, "synthesizer unavailable: {detail}"),
            Self::Setting(detail) => write!(f, "synthesizer setting rejected: {detail}"),
            Self::Synthesis(detail) => write!(f, "synthesis failed: {detail}"),
        }
    }
}

impl std::error::Error for SynthError {}

/// Receives a driver's output during [`SynthDriver::speak`].
pub trait SynthSink {
    /// Accepts interleaved 16-bit PCM in `format`.
    ///
    /// A return of `ControlFlow::Break(())` tells the driver to stop
    /// synthesizing now: the utterance was cancelled or its audio cannot be
    /// played. The driver returns from `speak` promptly without pushing
    /// further audio.
    fn push_pcm(&mut self, format: PcmFormat, samples: &[i16]) -> ControlFlow<()>;

    /// Reports that the audio pushed so far reaches index mark `mark`: the
    /// mark sits between the last sample pushed and the next.
    fn index_reached(&mut self, mark: IndexMark);

    /// Whether the utterance has been cancelled, for a driver that works a
    /// while before it has audio to push: it can stop its synthesis early
    /// rather than learn of the cancel at its first push.
    fn is_cancelled(&self) -> bool;
}

/// A speech synthesizer, whatever its origin: built-in, Wasm component, or
/// a synthesizer host process (architecture section 6).
///
/// Synchronous by contract. Verbatim calls `speak` on a dedicated synth
/// thread and never on an event, reducer, or GUI thread; drivers block
/// freely (`OneCore` blocks on `WinRT` internally) and must not spawn
/// threads of their own.
pub trait SynthDriver: Send {
    /// Stable identifier, used in config and the synthesizer list.
    fn id(&self) -> SynthId;

    /// Human-readable name for the synthesizer list.
    fn display_name(&self) -> String;

    /// The settings this driver supports, in display order — the source the
    /// settings GUI generates its controls from.
    fn supported_settings(&self) -> Vec<SettingDescriptor>;

    /// The current value of one setting, or `None` for an unknown id.
    fn setting(&self, id: &SettingId) -> Option<SettingValue>;

    /// Changes one setting, taking effect from the next `speak`.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Setting`] for an unknown id or a value outside
    /// the descriptor's range.
    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError>;

    /// Whether the driver reports each index mark at its exact place in its
    /// audio. When it cannot, the speech manager splits sequences at their
    /// marks and speaks the pieces one after another, so every mark is
    /// still exact (decision D17); a driver that says `false` never sees a
    /// mark.
    fn places_marks(&self) -> bool;

    /// Synthesizes one sequence, blocking until it finishes or the sink
    /// requests cancellation via `ControlFlow::Break`.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Synthesis`] when the sequence cannot be
    /// produced; the utterance ends as failed and the pipeline carries on.
    fn speak(
        &mut self,
        sequence: &SpeechSequence,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_at_marks_keeps_each_mark_after_the_text_before_it() {
        let sequence = SpeechSequence {
            utterance: UtteranceId(1),
            trace_id: TraceId::mint(),
            language: None,
            items: vec![
                SpeechItem::Text("one".to_owned()),
                SpeechItem::Mark(IndexMark(1)),
                SpeechItem::Text("two".to_owned()),
                SpeechItem::Mark(IndexMark(2)),
            ],
        };
        let pieces: Vec<(String, Option<IndexMark>)> = sequence
            .split_at_marks()
            .into_iter()
            .map(|(piece, mark)| (piece.text(), mark))
            .collect();
        assert_eq!(
            pieces,
            vec![
                ("one".to_owned(), Some(IndexMark(1))),
                ("two".to_owned(), Some(IndexMark(2))),
                (String::new(), None),
            ]
        );
    }
}
