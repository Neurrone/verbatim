//! The details each read fetches, chosen per request from Core's settings
//! (the details the active theme reports, `Input::Reported`), the read's
//! unit and its reason (`docs/design/readers-tell-core-asks-build2.md`,
//! "Rules, each with its reason"): the readers keep no copy of the
//! settings and fetch exactly what a request names, so a detail whose
//! setting is off is left unread and its call unmade.

use verbatim_model::property::PropertyId;
use verbatim_model::{TextDetails, TextUnit};

use crate::state::SrState;

/// `base` less the properties of the node details the settings do not
/// report: the property set a node read takes.
pub(crate) fn node_set(state: &SrState, base: &[PropertyId]) -> Vec<PropertyId> {
    state.reported.node.choose(base)
}

/// The property set of a listener read whose walk a completion judges
/// ([`node_set`]), with the description kept whatever the settings: the
/// completion judges each ancestor's layout with it, as NVDA's
/// presentation type reads it whatever the theme, so an unnamed grouping
/// with a description stays focus context
/// (`books/dev/src/explanation/design/uia-shapes.md`, decision 4). The
/// completion reports the description only as [`node_set`] names it. The
/// listener reads it with every element of the walk in any case, so this
/// adds no call.
pub(crate) fn walk_set(state: &SrState, base: &[PropertyId]) -> Vec<PropertyId> {
    base.iter()
        .copied()
        .filter(|id| {
            *id == PropertyId::Description
                || id
                    .entry()
                    .detail
                    .is_none_or(|detail| state.reported.node.contains(detail))
        })
        .collect()
}

/// Why a text read is made, which decides what it fetches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextReason {
    /// For a caret key: its unit as the key moved the caret.
    CaretKey,
}

/// What a text read of `unit` for `reason` fetches beyond the text: the
/// details the settings report, less spelling and grammar errors for a
/// paragraph read for a caret key, which NVDA leaves out for speed, as it
/// does for a table cell, a unit Verbatim's text has not.
pub(crate) fn text_details(state: &SrState, unit: TextUnit, reason: TextReason) -> TextDetails {
    let reported = state.reported.text;
    match (unit, reason) {
        (TextUnit::Paragraph, TextReason::CaretKey) => TextDetails {
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

    use super::{TextReason, node_set, text_details, walk_set};
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

    /// A walked read keeps the description with descriptions off, for the
    /// completion's layout judgment, and leaves out every other detail
    /// turned off.
    #[test]
    fn a_walked_read_keeps_the_description_whatever_the_settings() {
        let off = state(ReportedDetails {
            node: NodeDetailSet::ALL
                .without(NodeDetail::Description)
                .without(NodeDetail::Level),
            ..ReportedDetails::ALL
        });
        assert_eq!(
            walk_set(&off, BASE),
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
        assert_eq!(walk_set(&state(ReportedDetails::ALL), BASE), BASE);
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
            assert_eq!(
                text_details(&state, unit, TextReason::CaretKey),
                reported.text,
                "{unit:?}"
            );
        }
        assert_eq!(
            text_details(&state, TextUnit::Paragraph, TextReason::CaretKey),
            TextDetails {
                formats: FormatSet::NONE,
                spelling_errors: false,
                grammar_errors: false,
                links: true,
            }
        );
    }
}
