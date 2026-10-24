//! The gates of the UIA shapes the outpost fetches with
//! (`docs/design/uia-shapes.md`, section 4), from the details the active
//! theme reports ([`Fetches`]): one function per gated shape. A detail the
//! theme reports as off is not fetched, so the provider is not asked for
//! it. This is the one link the types cannot make, since the theme is in
//! a platform-neutral crate that knows nothing of UI Automation, so each
//! gated field has a unit test.

use verbatim_model::Fetches;
use verbatim_uia::{AncestorGates, NodeGates};

/// The gates of a [`verbatim_uia::Node`] read alone, such as a focus or a
/// navigation step's neighbour, for the details `fetches` names: its
/// description, keyboard shortcut, position in its set, and level, and its
/// place, its rectangle and whether it is off the screen, which a read of
/// one element includes.
#[must_use]
pub fn node_gates(fetches: &Fetches) -> NodeGates {
    NodeGates {
        place: Some(()),
        ..many_node_gates(fetches)
    }
}

/// The gates of a [`verbatim_uia::Node`] read among many, such as a tree
/// dump's: [`node_gates`] without its place, which a read of many elements
/// never includes (`verbatim_uia::Place`).
#[must_use]
pub fn many_node_gates(fetches: &Fetches) -> NodeGates {
    NodeGates {
        role: (),
        value: (),
        states: (),
        description: fetches.description.then_some(()),
        shortcut: fetches.shortcut.then_some(()),
        position: fetches.position.then_some(()),
        place: None,
        level: fetches.level,
    }
}

/// The gates of a [`verbatim_uia::Ancestor`] for the details `fetches`
/// names: its node's, read among many ([`many_node_gates`]), so without
/// its place. Its own description, which the layout judgment reads, is
/// fetched whatever the theme.
#[must_use]
pub fn ancestor_gates(fetches: &Fetches) -> AncestorGates {
    AncestorGates {
        node: many_node_gates(fetches),
        description: (),
    }
}

/// The gates of the focused element itself, read as an
/// [`verbatim_uia::Ancestor`] in the focus's read with its ancestors:
/// [`ancestor_gates`] with its place, since it is read alone.
#[must_use]
pub fn focus_gates(fetches: &Fetches) -> AncestorGates {
    AncestorGates {
        node: node_gates(fetches),
        description: (),
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::Fetches;
    use verbatim_uia::{AncestorGates, Node, NodeGates, Shape};

    use super::{ancestor_gates, focus_gates, many_node_gates, node_gates};

    /// The gates for every detail but the one `off` turns off, as a node's
    /// and as the focus's, which must equal `expected` and that node
    /// within an ancestor, and as an ancestor's and a node's among many,
    /// which are those without the place.
    fn assert_only(off: impl FnOnce(&mut Fetches), expected: NodeGates) {
        let mut fetches = Fetches::default();
        off(&mut fetches);
        let among_many = NodeGates {
            place: None,
            ..expected
        };
        assert_eq!(
            (
                node_gates(&fetches),
                many_node_gates(&fetches),
                focus_gates(&fetches),
                ancestor_gates(&fetches),
            ),
            (
                expected,
                among_many,
                AncestorGates {
                    node: expected,
                    description: (),
                },
                AncestorGates {
                    node: among_many,
                    description: (),
                },
            )
        );
    }

    #[test]
    fn descriptions_off_turn_off_the_description_alone() {
        assert_only(
            |fetches| fetches.description = false,
            NodeGates {
                description: None,
                ..Node::all_on()
            },
        );
    }

    #[test]
    fn shortcuts_off_turn_off_the_shortcut_alone() {
        assert_only(
            |fetches| fetches.shortcut = false,
            NodeGates {
                shortcut: None,
                ..Node::all_on()
            },
        );
    }

    #[test]
    fn positions_off_turn_off_the_position_alone() {
        assert_only(
            |fetches| fetches.position = false,
            NodeGates {
                position: None,
                ..Node::all_on()
            },
        );
    }

    #[test]
    fn levels_off_turn_off_the_level_alone() {
        assert_only(
            |fetches| fetches.level = false,
            NodeGates {
                level: false,
                ..Node::all_on()
            },
        );
    }
}
