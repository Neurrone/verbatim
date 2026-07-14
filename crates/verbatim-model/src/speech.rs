//! Structured utterances.
//!
//! The reducer composes announcements from tokens, not display text, so it
//! stays free of localization; the speech pipeline renders tokens to
//! localized words at its boundary (via `verbatim-i18n`) just before
//! dictionary and symbol processing.

use serde::{Deserialize, Serialize};

use crate::TraceId;
use crate::tree::{Role, State};

/// Priority lane for an utterance (architecture section 6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpeechPriority {
    /// Cancel current and queued speech, then speak.
    Interrupt,
    /// Speak after the current utterance, ahead of the queue.
    Next,
    /// Append to the queue.
    Queued,
}

/// The content of one utterance segment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SegmentContent {
    /// Literal text: a name, a value, typed characters.
    Text(String),
    /// A role, rendered to its localized spoken name.
    Role(Role),
    /// A state, rendered to its localized spoken name.
    State(State),
    /// The absence of a state that is worth announcing, rendered to its
    /// localized negative form — "not checked" for an unchecked check box.
    NegatedState(State),
}

/// One segment of an utterance, with an optional language override.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtteranceSegment {
    /// What to speak.
    pub content: SegmentContent,
    /// BCP 47 language tag for this segment, when it differs from the
    /// utterance default; utterances are multilingual end-to-end.
    pub language: Option<String>,
}

impl UtteranceSegment {
    /// A segment in the utterance's default language.
    #[must_use]
    pub fn new(content: SegmentContent) -> Self {
        Self {
            content,
            language: None,
        }
    }

    /// A literal-text segment in the utterance's default language.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(SegmentContent::Text(text.into()))
    }
}

/// A structured utterance flowing from the reducer into the speech pipeline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Utterance {
    /// The trace this speech belongs to, for end-to-end latency timelines.
    pub trace_id: TraceId,
    /// Priority lane.
    pub priority: SpeechPriority,
    /// Segments, spoken in order.
    pub segments: Vec<UtteranceSegment>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_segment_defaults_to_utterance_language() {
        let segment = UtteranceSegment::text("Settings");
        assert_eq!(segment.content, SegmentContent::Text("Settings".into()));
        assert!(segment.language.is_none());
    }
}
