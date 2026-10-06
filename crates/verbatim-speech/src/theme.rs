//! The presentation stage (decision D12): the pipeline boundary where a
//! [`Theme`] flattens a structured [`Utterance`] into a flat
//! [`SpeechSequence`].
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

use verbatim_i18n::{
    character_description, character_name, level, message_text, negated_state_name, phrase_text,
    position_in_set, role_name, state_name,
};
use verbatim_model::{SegmentContent, Utterance, UtteranceId};

use crate::driver::{IndexMark, SpeechItem, SpeechSequence};

/// A presentation theme: flattens structured utterances at the end of the
/// speech pipeline.
///
/// Implementations run on the pipeline's queue thread (hence `Send`) and
/// must be cheap: this sits between "utterance queued" and "synthesis
/// starts" on every spoken announcement, inside the latency budget.
pub trait Theme: Send {
    /// Flattens one structured utterance, which the pipeline has numbered
    /// `id`, to the sequence handed to the synthesizer.
    fn flatten(&self, utterance: &Utterance, id: UtteranceId) -> SpeechSequence;
}

/// The default theme: plain speech, no earcons, no voice changes.
///
/// Each segment becomes zero or one spoken word group: literal text, labels,
/// values, and descriptions pass through as their text; roles and states
/// resolve through `verbatim-i18n`; states that are never announced (or
/// absences not worth announcing) contribute nothing; a position within a
/// set becomes the localized "2 of 5" (and contributes nothing without a
/// set size — a bare position has no useful spoken form); a level becomes
/// the localized "level 3". The groups are joined with single spaces. A
/// capital spelled out is spoken with the pitch raised by
/// [`CAPITAL_PITCH_OFFSET`], as NVDA raises it by default. A character
/// spoken on its own is spoken by its name from the character table of
/// its segment's language ("comma"), or, with no name, as itself, a capital
/// raised in pitch; its description replaces it where one is asked for and
/// the table has one. An index mark becomes a mark item where it stands.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlainTheme;

/// How far the pitch setting is raised for a capital letter spelled out:
/// NVDA's default `capPitchChange`, which is not yet configurable here.
pub const CAPITAL_PITCH_OFFSET: i32 = 30;

impl Theme for PlainTheme {
    /// The sequence is text items, or none when nothing is spoken; a
    /// spelled capital is a text item between two pitch changes, the
    /// second back to the configured pitch; an index mark is a mark item.
    /// The language tag is taken from the first segment that overrides it,
    /// if any.
    fn flatten(&self, utterance: &Utterance, id: UtteranceId) -> SpeechSequence {
        let mut items = Vec::new();
        let mut parts: Vec<String> = Vec::new();
        for segment in &utterance.segments {
            let language = segment.language.as_deref();
            let raised = match &segment.content {
                SegmentContent::SpelledCapital(text) => Some(text.clone()),
                SegmentContent::Character(text) if character_name(text, language).is_none() => {
                    is_capital(text).then(|| text.clone())
                }
                SegmentContent::CharacterDescription(text) if is_capital(text) => {
                    Some(character_description(text, language).unwrap_or_else(|| text.clone()))
                }
                _ => None,
            };
            if let Some(text) = raised {
                flush(&mut parts, &mut items);
                items.push(SpeechItem::Pitch(CAPITAL_PITCH_OFFSET));
                items.push(SpeechItem::Text(text));
                items.push(SpeechItem::Pitch(0));
            } else if let SegmentContent::Mark(mark) = &segment.content {
                flush(&mut parts, &mut items);
                items.push(SpeechItem::Mark(IndexMark(mark.0)));
            } else if let Some(part) = spoken_form(&segment.content, language) {
                parts.push(part);
            }
        }
        flush(&mut parts, &mut items);

        let language = utterance
            .segments
            .iter()
            .find_map(|segment| segment.language.clone());

        SpeechSequence {
            utterance: id,
            trace_id: utterance.trace_id,
            language,
            items,
        }
    }
}

/// Ends a run of spoken groups as one text item.
fn flush(parts: &mut Vec<String>, items: &mut Vec<SpeechItem>) {
    let text = parts.join(" ");
    parts.clear();
    if !text.is_empty() {
        items.push(SpeechItem::Text(text));
    }
}

/// Whether `text` is one uppercase letter, which is raised in pitch when it
/// is spoken on its own.
fn is_capital(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_uppercase() && chars.next().is_none())
}

/// The plain spoken form of one span, or `None` for spans with nothing to
/// say: empty text, states that are never announced, a bare position
/// without a set size (which has no useful spoken form), and any future
/// variant until it is given a spoken form here (`SegmentContent` is
/// non-exhaustive). `language` is the segment's language, for the
/// character table.
fn spoken_form(content: &SegmentContent, language: Option<&str>) -> Option<String> {
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
        SegmentContent::Message(message) => {
            let text = message_text(*message);
            (!text.is_empty()).then_some(text)
        }
        SegmentContent::Phrase(phrase) => {
            let text = phrase_text(phrase);
            (!text.is_empty()).then_some(text)
        }
        SegmentContent::Character(text) => {
            character_name(text, language).or_else(|| (!text.is_empty()).then(|| text.clone()))
        }
        SegmentContent::CharacterDescription(text) => character_description(text, language)
            .or_else(|| character_name(text, language))
            .or_else(|| (!text.is_empty()).then(|| text.clone())),
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
            validity: None,
        }
    }

    #[test]
    fn a_spelled_capital_is_raised_in_pitch_and_the_pitch_restored() {
        let utterance = utterance_of(vec![
            UtteranceSegment::text("a"),
            UtteranceSegment::new(SegmentContent::SpelledCapital("B".to_owned())),
            UtteranceSegment::text("c"),
        ]);
        let sequence = PlainTheme.flatten(&utterance, UtteranceId(1));
        assert_eq!(
            sequence.items,
            vec![
                SpeechItem::Text("a".to_owned()),
                SpeechItem::Pitch(CAPITAL_PITCH_OFFSET),
                SpeechItem::Text("B".to_owned()),
                SpeechItem::Pitch(0),
                SpeechItem::Text("c".to_owned()),
            ]
        );
        assert_eq!(sequence.text(), "a B c");
    }

    #[test]
    fn characters_are_spoken_by_name_description_or_raised_pitch() {
        use verbatim_model::SpeechMark;
        let utterance = utterance_of(vec![
            UtteranceSegment::new(SegmentContent::Character(",".to_owned())),
            UtteranceSegment::new(SegmentContent::Character("x".to_owned())),
            UtteranceSegment::new(SegmentContent::Mark(SpeechMark(4))),
            UtteranceSegment::new(SegmentContent::CharacterDescription("b".to_owned())),
            UtteranceSegment::new(SegmentContent::Character("Q".to_owned())),
        ]);
        let sequence = PlainTheme.flatten(&utterance, UtteranceId(1));
        assert_eq!(
            sequence.items,
            vec![
                SpeechItem::Text("comma x".to_owned()),
                SpeechItem::Mark(IndexMark(4)),
                SpeechItem::Text("Bravo".to_owned()),
                SpeechItem::Pitch(CAPITAL_PITCH_OFFSET),
                SpeechItem::Text("Q".to_owned()),
                SpeechItem::Pitch(0),
            ]
        );
        assert!(sequence.has_marks());
    }

    #[test]
    fn renders_text_and_role_joined_with_spaces() {
        let utterance = utterance_of(vec![
            UtteranceSegment::text("Settings"),
            UtteranceSegment::new(SegmentContent::Role(Role::MenuItem)),
        ]);
        let sequence = PlainTheme.flatten(&utterance, UtteranceId(1));
        assert_eq!(sequence.text(), "Settings menu item");
        assert!(!sequence.has_marks());
    }

    #[test]
    fn labels_values_and_descriptions_render_as_their_text() {
        let utterance = utterance_of(vec![
            UtteranceSegment::label("Rate"),
            UtteranceSegment::new(SegmentContent::Role(Role::Slider)),
            UtteranceSegment::value("50"),
            UtteranceSegment::new(SegmentContent::Description("Speech rate".to_owned())),
        ]);
        let sequence = PlainTheme.flatten(&utterance, UtteranceId(1));
        assert_eq!(sequence.text(), "Rate slider 50 Speech rate");
    }

    #[test]
    fn drops_silent_states_and_keeps_announced_ones() {
        let utterance = utterance_of(vec![
            UtteranceSegment::text("Bold"),
            // Focusable is never announced and must contribute nothing.
            UtteranceSegment::new(SegmentContent::State(State::Focusable)),
            UtteranceSegment::new(SegmentContent::NegatedState(State::Checked)),
        ]);
        let sequence = PlainTheme.flatten(&utterance, UtteranceId(1));
        assert_eq!(sequence.text(), "Bold not checked");
    }

    #[test]
    fn position_renders_with_a_set_size_and_not_without() {
        let with_size = utterance_of(vec![UtteranceSegment::new(SegmentContent::Position {
            position: 2,
            set_size: Some(5),
        })]);
        assert_eq!(
            PlainTheme.flatten(&with_size, UtteranceId(1)).text(),
            "2 of 5"
        );

        let without_size = utterance_of(vec![UtteranceSegment::new(SegmentContent::Position {
            position: 2,
            set_size: None,
        })]);
        assert_eq!(PlainTheme.flatten(&without_size, UtteranceId(1)).text(), "");
    }

    #[test]
    fn level_renders_its_localized_phrase() {
        let utterance = utterance_of(vec![UtteranceSegment::new(SegmentContent::Level(3))]);
        assert_eq!(
            PlainTheme.flatten(&utterance, UtteranceId(1)).text(),
            "level 3"
        );
    }

    #[test]
    fn language_override_flows_from_first_tagged_segment() {
        let mut segment = UtteranceSegment::text("hola");
        segment.language = Some("es".to_owned());
        let utterance = utterance_of(vec![segment]);
        let sequence = PlainTheme.flatten(&utterance, UtteranceId(1));
        assert_eq!(sequence.language.as_deref(), Some("es"));
    }
}
