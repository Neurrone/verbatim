//! Structured utterances.
//!
//! The reducer composes announcements from tokens, not display text, so it
//! stays free of localization; the speech pipeline renders tokens to
//! localized words at its boundary (via `verbatim-i18n`) just before
//! dictionary and symbol processing.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::tree::{Rect, Role, State};
use crate::{NodeId, TraceId};

/// Identifies one utterance from the moment the speech pipeline accepts it
/// until its single ending (decision D17). Unlike a [`TraceId`], which names
/// the event behind speech and can be shared by several utterances, an
/// utterance id is never reused within a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UtteranceId(pub u64);

impl fmt::Display for UtteranceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "u{}", self.0)
    }
}

/// How an utterance ended (decision D17). Every utterance the speech
/// pipeline accepts ends exactly once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UtteranceEnding {
    /// The audio device played all of the utterance's audio. An utterance
    /// that produced no audio completes when the audio before it has played.
    Completed,
    /// The utterance was cut off or dropped before all of it was heard: by
    /// speech that interrupts, a synthesizer switch, or shutdown.
    Cancelled,
    /// Synthesis or audio output failed; the text says why.
    Failed(String),
}

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
    /// The review cursor is on the first line or word — NVDA's "Top".
    Top,
    /// The review cursor is on the last line or word — NVDA's "Bottom".
    Bottom,
    /// The review cursor is on the first character of the line — "Left".
    Left,
    /// The review cursor is on the last character of the line — "Right".
    Right,
    /// The unit under the review cursor is empty — NVDA's "blank".
    Blank,
    /// The navigator returns to the focus — NVDA's "Move to focus".
    MoveToFocus,
    /// A navigator command with no navigator — "No navigator object".
    NoNavigatorObject,
    /// The navigator object was activated — NVDA's "Activate".
    Activate,
    /// Nothing could be activated — NVDA's "No action".
    NoAction,
    /// UIA's Invoke pattern was performed — NVDA's "invoke".
    Invoke,
    /// A space, spelled — NVDA's symbol name "space".
    Space,
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
    /// For focus speech: the node it announces, so the speech manager can
    /// drop it once that node is no longer relevant to the focus
    /// ([`FocusValidity`]). `None` for every other utterance, which only a
    /// cancel ends early.
    #[serde(default)]
    pub validity: Option<FocusValidity>,
}

/// What focus speech is about, for dropping it once the focus has moved on
/// (`docs/nvda/speech.md`, "Cancellation", and `docs/nvda/events.md`):
/// queued or playing speech announcing a node stays valid while that node
/// is the focus, an ancestor of the focus, or the foreground window, or if
/// the node never had the focus at all, as a dialog announced on entering
/// it never does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusValidity {
    /// The node the speech announces.
    pub node: NodeId,
    /// Whether that node was the focus when the speech was made.
    pub had_focus: bool,
}

impl FocusValidity {
    /// Whether speech with this validity is still worth hearing, given
    /// where the focus now is.
    #[must_use]
    pub fn holds(&self, now: &FocusNow) -> bool {
        !self.had_focus
            || self.node == now.focus
            || now.ancestors.contains(&self.node)
            || now.foreground == Some(self.node)
    }
}

/// Where the focus is, for judging [`FocusValidity`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusNow {
    /// The focus.
    pub focus: NodeId,
    /// The focus's ancestors.
    pub ancestors: Vec<NodeId>,
    /// The foreground window's node, when known.
    pub foreground: Option<NodeId>,
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
