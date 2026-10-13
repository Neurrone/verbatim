//! The gates of the UIA shapes the outpost fetches with
//! (`docs/design/uia-shapes.md`, section 4), from the details the active
//! theme reports ([`Fetches`]): one function per gated shape. A detail the
//! theme reports as off is not fetched, so the provider is not asked for
//! it. This is the one link the types cannot make, since the theme is in
//! a platform-neutral crate that knows nothing of UI Automation, so each
//! gated field has a unit test.

use verbatim_model::Fetches;
use verbatim_uia::{AncestorGates, NodeGates};

/// The gates of a [`verbatim_uia::Node`] for the details `fetches` names:
/// its description, keyboard shortcut, position in its set, and level.
#[must_use]
pub fn node_gates(fetches: &Fetches) -> NodeGates {
    NodeGates {
        role: (),
        value: (),
        states: (),
        description: fetches.description.then_some(()),
        shortcut: fetches.shortcut.then_some(()),
        position: fetches.position.then_some(()),
        level: fetches.level,
    }
}

/// The gates of a [`verbatim_uia::Ancestor`] for the details `fetches`
/// names: its node's. Its own description, which the layout judgment
/// reads, is fetched whatever the theme.
#[must_use]
pub fn ancestor_gates(fetches: &Fetches) -> AncestorGates {
    AncestorGates {
        node: node_gates(fetches),
        description: (),
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::Fetches;
    use verbatim_uia::{AncestorGates, Node, NodeGates, Shape};

    use super::{ancestor_gates, node_gates};

    /// The gates for every detail but the one `off` turns off, as a node's
    /// and as an ancestor's, which must equal `expected` and that node
    /// within an ancestor.
    fn assert_only(off: impl FnOnce(&mut Fetches), expected: NodeGates) {
        let mut fetches = Fetches::default();
        off(&mut fetches);
        assert_eq!(node_gates(&fetches), expected);
        assert_eq!(
            ancestor_gates(&fetches),
            AncestorGates {
                node: expected,
                description: (),
            }
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
