//! The details each read fetches, chosen per request from Core's settings,
//! the read's unit and its reason (`books/dev/src/reference/crates/verbatim-core.md`,
//! "Formatting").

use verbatim_model::property::PropertyId;
use verbatim_model::{TextDetails, TextUnit};

use crate::state::SrState;

/// `base` less the properties of the node details the settings do not
/// report: the property set a node read takes.
pub(crate) fn node_set(state: &SrState, base: &[PropertyId]) -> Vec<PropertyId> {
    state.reported.node.choose(base)
}

/// The property set of a focus read: [`node_set`], with the position in
/// the set and the set's size kept whatever the settings, as the identity
/// check in the duplicate test compares them (Dickson, 2026-11-18;
/// `docs/parity/focus-events.md`, "Duplicate focus suppression"). The
/// setting decides only whether they are spoken, which the focus's speech
/// does. One `LVM_GETITEMCOUNT` more for a list view item's focus with the
/// setting off.
pub(crate) fn focus_set(state: &SrState, base: &[PropertyId]) -> Vec<PropertyId> {
    set_keeping(
        state,
        base,
        &[PropertyId::PositionInSet, PropertyId::SetSize],
    )
}

/// [`node_set`], with the description kept whatever the settings: a live
/// read of an object reviewed as flat text, whose text, NVDA's basic text,
/// holds the description whatever the theme reports
/// (`crate::review_text::flat_text`).
pub(crate) fn description_kept(state: &SrState, base: &[PropertyId]) -> Vec<PropertyId> {
    set_keeping(state, base, &[PropertyId::Description])
}

/// [`node_set`], with the properties of `kept` kept whatever the settings.
fn set_keeping(state: &SrState, base: &[PropertyId], kept: &[PropertyId]) -> Vec<PropertyId> {
    base.iter()
        .copied()
        .filter(|id| {
            kept.contains(id)
                || id
                    .entry()
                    .detail
                    .is_none_or(|detail| state.reported.node.contains(detail))
        })
        .collect()
}

/// What a focus's caret read fetches beyond the text: the details the
/// settings report, for its line, which NVDA speaks with its formatting
/// (`speech/speech.py` 904 to 922).
pub(crate) fn focus_details(state: &SrState) -> TextDetails {
    state.reported.text
}

/// What a caret key's text read of `unit` fetches beyond the text: the
/// details the settings report, less spelling and grammar errors for a
/// paragraph, which NVDA leaves out for speed, as it does for a table
/// cell, a unit Verbatim's text has not.
pub(crate) fn text_details(state: &SrState, unit: TextUnit) -> TextDetails {
    let reported = state.reported.text;
    match unit {
        TextUnit::Paragraph => TextDetails {
            spelling_errors: false,
            grammar_errors: false,
            ..reported
        },
        _ => reported,
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::property::{NodeDetail, NodeDetailSet, PropertyId};
    use verbatim_model::{FormatSet, ReportedDetails, TextDetails, TextUnit};

    use super::{description_kept, node_set, text_details};
    use crate::state::SrState;

    fn state(reported: ReportedDetails) -> SrState {
        SrState {
            reported,
            ..SrState::default()
        }
    }

    const BASE: &[PropertyId] = &[
        PropertyId::Name,
        PropertyId::Description,
        PropertyId::KeyboardShortcut,
        PropertyId::PositionInSet,
        PropertyId::SetSize,
        PropertyId::Level,
        PropertyId::Cell,
        PropertyId::Rect,
    ];

    /// The theme's details reach Core's state, which a flight-recorder
    /// dump restores; everything is reported until they do.
    #[test]
    fn the_reported_details_are_set_and_kept_in_a_dump() {
        let mut state = SrState::new();
        assert_eq!(state.reported, ReportedDetails::ALL);
        let reported = ReportedDetails {
            node: NodeDetailSet::ALL.without(NodeDetail::Description),
            ..ReportedDetails::ALL
        };
        let effects = crate::reduce::reduce(
            &mut state,
            &mut crate::Runtime::default(),
            &verbatim_model::Input::Reported(reported),
        );
        assert_eq!(effects, Vec::new());
        assert_eq!(state.reported, reported);
        let restored: SrState =
            serde_json::from_str(&serde_json::to_string(&state).expect("serializes"))
                .expect("deserializes");
        assert_eq!(restored.reported, reported);
    }

    /// Each node detail turned off leaves its properties, and nothing
    /// else, out of the set.
    #[test]
    fn a_node_detail_off_leaves_its_properties_out_of_the_set() {
        let without = |detail| {
            node_set(
                &state(ReportedDetails {
                    node: NodeDetailSet::ALL.without(detail),
                    ..ReportedDetails::ALL
                }),
                BASE,
            )
        };
        let less = |gone: &[PropertyId]| {
            BASE.iter()
                .copied()
                .filter(|id| !gone.contains(id))
                .collect::<Vec<_>>()
        };
        assert_eq!(node_set(&state(ReportedDetails::ALL), BASE), BASE);
        assert_eq!(
            without(NodeDetail::Description),
            less(&[PropertyId::Description])
        );
        assert_eq!(
            without(NodeDetail::KeyboardShortcut),
            less(&[PropertyId::KeyboardShortcut])
        );
        assert_eq!(
            without(NodeDetail::Position),
            less(&[PropertyId::PositionInSet, PropertyId::SetSize])
        );
        assert_eq!(without(NodeDetail::Level), less(&[PropertyId::Level]));
        assert_eq!(without(NodeDetail::Cell), less(&[PropertyId::Cell]));
    }

    /// A flat review's live read keeps the description with descriptions
    /// off, for the object's flat text, and leaves out every other detail
    /// turned off.
    #[test]
    fn a_flat_reviews_read_keeps_the_description_whatever_the_settings() {
        let off = state(ReportedDetails {
            node: NodeDetailSet::ALL
                .without(NodeDetail::Description)
                .without(NodeDetail::Level),
            ..ReportedDetails::ALL
        });
        assert_eq!(
            description_kept(&off, BASE),
            [
                PropertyId::Name,
                PropertyId::Description,
                PropertyId::KeyboardShortcut,
                PropertyId::PositionInSet,
                PropertyId::SetSize,
                PropertyId::Cell,
                PropertyId::Rect,
            ]
        );
        assert_eq!(description_kept(&state(ReportedDetails::ALL), BASE), BASE);
    }

    /// A caret key's paragraph read leaves out spelling and
    /// grammar errors; its other units take what the settings report.
    #[test]
    fn a_caret_keys_paragraph_read_leaves_out_errors() {
        let reported = ReportedDetails {
            node: NodeDetailSet::ALL,
            text: TextDetails {
                formats: FormatSet::NONE,
                ..TextDetails::ALL
            },
        };
        let state = state(reported);
        for unit in [
            TextUnit::Character,
            TextUnit::Word,
            TextUnit::Line,
            TextUnit::Sentence,
        ] {
            assert_eq!(text_details(&state, unit), reported.text, "{unit:?}");
        }
        assert_eq!(
            text_details(&state, TextUnit::Paragraph),
            TextDetails {
                formats: FormatSet::NONE,
                spelling_errors: false,
                grammar_errors: false,
                links: true,
            }
        );
    }
}
