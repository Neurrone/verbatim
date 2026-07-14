//! Token rendering: the pipeline boundary that turns a structured
//! [`Utterance`] into a flat [`SpeechRequest`].
//!
//! The reducer composes announcements from tokens, not display text, so the
//! localization happens here (via `verbatim-i18n`) just before dictionary and
//! symbol processing. Role and state tokens resolve to their localized spoken
//! names; states with no spoken form are dropped. Segments are joined with
//! single spaces.

use verbatim_i18n::{negated_state_name, role_name, state_name};
use verbatim_model::{SegmentContent, Utterance};

use crate::driver::SpeechRequest;

/// Renders one structured utterance to a flat [`SpeechRequest`].
///
/// Each segment becomes zero or one spoken word group: literal text passes
/// through, roles and states resolve through `verbatim-i18n`, and states that
/// are never announced (or absences that are not worth announcing) contribute
/// nothing. The resulting groups are joined with single spaces.
///
/// The request carries no index marks: M1 utterances do not embed them, and
/// drivers that need marks receive requests built directly. The language tag
/// is taken from the first segment that overrides it, if any.
#[must_use]
pub fn render_utterance(utterance: &Utterance) -> SpeechRequest {
    let mut parts: Vec<String> = Vec::with_capacity(utterance.segments.len());
    for segment in &utterance.segments {
        match &segment.content {
            SegmentContent::Text(text) => {
                if !text.is_empty() {
                    parts.push(text.clone());
                }
            }
            SegmentContent::Role(role) => parts.push(role_name(*role)),
            SegmentContent::State(state) => {
                if let Some(name) = state_name(*state) {
                    parts.push(name);
                }
            }
            SegmentContent::NegatedState(state) => {
                if let Some(name) = negated_state_name(*state) {
                    parts.push(name);
                }
            }
            // `SegmentContent` is non-exhaustive; a future variant renders as
            // nothing until it is given a token rendering.
            _ => {}
        }
    }

    let language = utterance
        .segments
        .iter()
        .find_map(|segment| segment.language.clone());

    SpeechRequest {
        trace_id: utterance.trace_id,
        text: parts.join(" "),
        language,
        marks: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{Role, SpeechPriority, State, TraceId, UtteranceSegment};

    use super::*;

    #[test]
    fn renders_text_and_role_joined_with_spaces() {
        let utterance = Utterance {
            trace_id: TraceId::mint(),
            priority: SpeechPriority::Queued,
            segments: vec![
                UtteranceSegment::text("Settings"),
                UtteranceSegment::new(SegmentContent::Role(Role::MenuItem)),
            ],
        };
        let request = render_utterance(&utterance);
        assert_eq!(request.text, "Settings menu item");
        assert!(request.marks.is_empty());
    }

    #[test]
    fn drops_silent_states_and_keeps_announced_ones() {
        let utterance = Utterance {
            trace_id: TraceId::mint(),
            priority: SpeechPriority::Queued,
            segments: vec![
                UtteranceSegment::text("Bold"),
                // Focused is never announced and must contribute nothing.
                UtteranceSegment::new(SegmentContent::State(State::Focused)),
                UtteranceSegment::new(SegmentContent::NegatedState(State::Checked)),
            ],
        };
        let request = render_utterance(&utterance);
        assert_eq!(request.text, "Bold not checked");
    }

    #[test]
    fn language_override_flows_from_first_tagged_segment() {
        let mut segment = UtteranceSegment::text("hola");
        segment.language = Some("es".to_owned());
        let utterance = Utterance {
            trace_id: TraceId::mint(),
            priority: SpeechPriority::Queued,
            segments: vec![segment],
        };
        let request = render_utterance(&utterance);
        assert_eq!(request.language.as_deref(), Some("es"));
    }
}
