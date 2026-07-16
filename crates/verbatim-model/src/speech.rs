//! Structured utterances.
//!
//! The reducer composes announcements from tokens, not display text, so it
//! stays free of localization; the speech pipeline renders tokens to
//! localized words at its boundary (via `verbatim-i18n`) just before
//! dictionary and symbol processing.

use serde::{Deserialize, Serialize};

use crate::TraceId;
use crate::tree::{Rect, Role, State};

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

/// The content of one utterance segment: a semantic span, per decision D12.
///
/// Spans stay typed all the way to the presentation stage at the end of the
/// speech pipeline, where a theme flattens them — to plain words in the
/// default theme, or (milestone M11) to earcons and voice changes keyed by
/// exactly these span kinds. The reducer never pre-flattens: a control's
/// label travels as [`Label`](Self::Label), never as anonymous text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SegmentContent {
    /// Literal text with no more specific meaning: typed characters, a
    /// spoken time, free-form message text.
    Text(String),
    /// A control's accessible name or label.
    Label(String),
    /// A control's current value — slider position, combo selection.
    Value(String),
    /// A control's accessible description, when it adds information beyond
    /// the label.
    Description(String),
    /// The keyboard shortcut a control advertises (an access key or
    /// accelerator), spoken after the description in NVDA's property order.
    Shortcut(String),
    /// A role, rendered to its localized spoken name.
    Role(Role),
    /// A state, rendered to its localized spoken name.
    State(State),
    /// The absence of a state that is worth announcing, rendered to its
    /// localized negative form — "not checked" for an unchecked check box.
    NegatedState(State),
    /// Position within a set — "2 of 5" — from the node's reported
    /// position-in-set and set-size details.
    Position {
        /// One-based position within the set.
        position: u32,
        /// Set size, when reported; a position can arrive without one.
        set_size: Option<u32>,
    },
    /// One-based nesting level (tree items, headings).
    Level(u32),
    /// A fixed reader message, rendered to its localized wording — the
    /// D12-conformant way for the reducer to say something that is not a
    /// property of any node (a navigation edge, for instance) without
    /// pre-flattening text.
    Message(Message),
}

/// A fixed reader message a [`SegmentContent::Message`] segment names.
/// Localization happens at the presentation stage, like every other span.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Message {
    /// Object navigation found no next sibling — NVDA's "No next".
    NoNextObject,
    /// Object navigation found no previous sibling — NVDA's "No previous".
    NoPreviousObject,
    /// Object navigation found no parent — NVDA's "No containing object".
    NoContainingObject,
    /// Object navigation found no children — NVDA's "No objects inside".
    NoObjectsInside,
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

    /// A label segment in the utterance's default language.
    #[must_use]
    pub fn label(text: impl Into<String>) -> Self {
        Self::new(SegmentContent::Label(text.into()))
    }

    /// A value segment in the utterance's default language.
    #[must_use]
    pub fn value(text: impl Into<String>) -> Self {
        Self::new(SegmentContent::Value(text.into()))
    }
}

/// The node an utterance describes, carried alongside its segments.
///
/// This exists for presentation themes (decision D12): an earcon theme keys
/// sounds off the source node's role, and a positional-audio theme (milestone
/// M11, in the audio-themes add-on tradition) pans them by its screen
/// rectangle. Core-originated speech with no source node — the startup
/// announcement, the spoken time — carries none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UtteranceSource {
    /// The described node's role.
    pub role: Role,
    /// Its bounding rectangle in screen coordinates, when reported.
    pub rect: Option<Rect>,
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
    /// The node this utterance describes, when there is one, for
    /// presentation themes. `#[serde(default)]` keeps utterances recorded
    /// before this field existed deserializing unchanged.
    #[serde(default)]
    pub source: Option<UtteranceSource>,
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
