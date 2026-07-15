//! The presentation stage (decision D12): the pipeline boundary where a
//! [`Theme`] flattens a structured [`Utterance`] into a flat
//! [`SpeechRequest`].
//!
//! The reducer composes announcements from semantic spans, never display
//! text, so localization and presentation both happen here — just before
//! dictionary and symbol processing. [`PlainTheme`] is the default and only
//! M3 theme: spans become their localized spoken words and nothing else.
//! Milestone M11's audio-formatting themes implement the same trait to map
//! spans to earcons and voice changes instead; that is why utterances carry
//! their source node's role and screen rectangle
//! (`verbatim_model::UtteranceSource`), even though [`PlainTheme`] ignores
//! both.

use verbatim_i18n::{level, negated_state_name, position_in_set, role_name, state_name};
use verbatim_model::{SegmentContent, Utterance};

use crate::driver::SpeechRequest;

/// A presentation theme: flattens structured utterances at the end of the
/// speech pipeline.
///
/// Implementations run on the pipeline's queue thread (hence `Send`) and
/// must be cheap: this sits between "utterance queued" and "synthesis
/// starts" on every spoken announcement, inside the latency budget.
pub trait Theme: Send {
    /// Flattens one structured utterance to the flat request handed to the
    /// synthesizer.
    fn flatten(&self, utterance: &Utterance) -> SpeechRequest;
}

/// The default theme: plain speech, no earcons, no voice changes.
///
/// Each segment becomes zero or one spoken word group: literal text, labels,
/// values, and descriptions pass through as their text; roles and states
/// resolve through `verbatim-i18n`; states that are never announced (or
/// absences not worth announcing) contribute nothing; a position within a
/// set becomes the localized "2 of 5" (and contributes nothing without a
/// set size — a bare position has no useful spoken form); a level becomes
/// the localized "level 3". The groups are joined with single spaces.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlainTheme;

impl Theme for PlainTheme {
    /// The request carries no index marks: no current utterance embeds
    /// them, and drivers that need marks receive requests built directly.
    /// The language tag is taken from the first segment that overrides it,
    /// if any.
    fn flatten(&self, utterance: &Utterance) -> SpeechRequest {
        let parts: Vec<String> = utterance
            .segments
            .iter()
            .filter_map(|segment| spoken_form(&segment.content))
            .collect();

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
}

/// The plain spoken form of one span, or `None` for spans with nothing to
/// say: empty text, states that are never announced, a bare position
/// without a set size (which has no useful spoken form), and any future
/// variant until it is given a spoken form here (`SegmentContent` is
/// non-exhaustive).
fn spoken_form(content: &SegmentContent) -> Option<String> {
    match content {
        SegmentContent::Text(text)
        | SegmentContent::Label(text)
        | SegmentContent::Value(text)
        | SegmentContent::Description(text)
        | SegmentContent::Shortcut(text) => (!text.is_empty()).then(|| text.clone()),
        SegmentContent::Role(role) => Some(role_name(*role)),
        SegmentContent::State(state) => state_name(*state),
        SegmentContent::NegatedState(state) => negated_state_name(*state),
        SegmentContent::Position { position, set_size } => {
            set_size.map(|set_size| position_in_set(*position, set_size))
        }
        SegmentContent::Level(depth) => Some(level(*depth)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{Role, SpeechPriority, State, TraceId, UtteranceSegment};

    use super::*;

    fn utterance_of(segments: Vec<UtteranceSegment>) -> Utterance {
        Utterance {
            trace_id: TraceId::mint(),
            priority: SpeechPriority::Queued,
            segments,
            source: None,
        }
    }

    #[test]
    fn renders_text_and_role_joined_with_spaces() {
        let utterance = utterance_of(vec![
            UtteranceSegment::text("Settings"),
            UtteranceSegment::new(SegmentContent::Role(Role::MenuItem)),
        ]);
        let request = PlainTheme.flatten(&utterance);
        assert_eq!(request.text, "Settings menu item");
        assert!(request.marks.is_empty());
    }

    #[test]
    fn labels_values_and_descriptions_render_as_their_text() {
        let utterance = utterance_of(vec![
            UtteranceSegment::label("Rate"),
            UtteranceSegment::new(SegmentContent::Role(Role::Slider)),
            UtteranceSegment::value("50"),
            UtteranceSegment::new(SegmentContent::Description("Speech rate".to_owned())),
        ]);
        let request = PlainTheme.flatten(&utterance);
        assert_eq!(request.text, "Rate slider 50 Speech rate");
    }

    #[test]
    fn drops_silent_states_and_keeps_announced_ones() {
        let utterance = utterance_of(vec![
            UtteranceSegment::text("Bold"),
            // Focused is never announced and must contribute nothing.
            UtteranceSegment::new(SegmentContent::State(State::Focused)),
            UtteranceSegment::new(SegmentContent::NegatedState(State::Checked)),
        ]);
        let request = PlainTheme.flatten(&utterance);
        assert_eq!(request.text, "Bold not checked");
    }

    #[test]
    fn position_renders_with_a_set_size_and_not_without() {
        let with_size = utterance_of(vec![UtteranceSegment::new(SegmentContent::Position {
            position: 2,
            set_size: Some(5),
        })]);
        assert_eq!(PlainTheme.flatten(&with_size).text, "2 of 5");

        let without_size = utterance_of(vec![UtteranceSegment::new(SegmentContent::Position {
            position: 2,
            set_size: None,
        })]);
        assert_eq!(PlainTheme.flatten(&without_size).text, "");
    }

    #[test]
    fn level_renders_its_localized_phrase() {
        let utterance = utterance_of(vec![UtteranceSegment::new(SegmentContent::Level(3))]);
        assert_eq!(PlainTheme.flatten(&utterance).text, "level 3");
    }

    #[test]
    fn language_override_flows_from_first_tagged_segment() {
        let mut segment = UtteranceSegment::text("hola");
        segment.language = Some("es".to_owned());
        let utterance = utterance_of(vec![segment]);
        let request = PlainTheme.flatten(&utterance);
        assert_eq!(request.language.as_deref(), Some("es"));
    }
}
