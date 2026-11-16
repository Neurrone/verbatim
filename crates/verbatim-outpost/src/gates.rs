//! The gates of the UIA shapes the outpost fetches with
//! (`books/dev/src/explanation/design/uia-shapes.md`, section 4), from the
//! node details a request's property set names ([`NodeDetailSet`]): one
//! function per gated shape. A detail the request does not name is not
//! fetched, so the provider is not asked for it. This is the one link the
//! types cannot make, since the property table is in a platform-neutral
//! crate that knows nothing of UI Automation's shapes, so each gated field
//! has a unit test.

use verbatim_model::property::{NodeDetail, NodeDetailSet};
use verbatim_uia::{AncestorGates, NodeGates};

/// The gates of a [`verbatim_uia::Node`] read alone, such as a focus or a
/// navigation step's neighbour, for the details `details` names: its
/// description, keyboard shortcut, position in its set, and level, and
/// its row and column in a grid; and its place, its rectangle and whether
/// it is off the screen, which a read of one element includes.
#[must_use]
pub fn node_gates(details: NodeDetailSet) -> NodeGates {
    NodeGates {
        place: Some(()),
        cell: details.contains(NodeDetail::Cell).then_some(()),
        ..many_node_gates(details)
    }
}

/// The gates of a [`verbatim_uia::Node`] read among many, such as a tree
/// dump's: [`node_gates`] without its place or its row and column, which a
/// read of many elements never includes (`verbatim_uia::Place`,
/// `verbatim_uia::Cell`).
#[must_use]
pub fn many_node_gates(details: NodeDetailSet) -> NodeGates {
    NodeGates {
        role: (),
        value: (),
        states: (),
        description: details.contains(NodeDetail::Description).then_some(()),
        shortcut: details.contains(NodeDetail::KeyboardShortcut).then_some(()),
        position: details.contains(NodeDetail::Position).then_some(()),
        place: None,
        cell: None,
        counts: None,
        level: details.contains(NodeDetail::Level),
    }
}

/// The gates of a [`verbatim_uia::Ancestor`] for the details `details`
/// names: its node's, read among many ([`many_node_gates`]), so without
/// its place. Its own description, which the layout judgment reads, is
/// fetched whatever the request names.
#[must_use]
pub fn ancestor_gates(details: NodeDetailSet) -> AncestorGates {
    AncestorGates {
        node: many_node_gates(details),
        description: (),
    }
}

/// The gates of the focused element itself, read as an
/// [`verbatim_uia::Ancestor`] in the focus's read with its ancestors:
/// [`ancestor_gates`] with its place, since it is read alone.
#[must_use]
pub fn focus_gates(details: NodeDetailSet) -> AncestorGates {
    AncestorGates {
        node: node_gates(details),
        description: (),
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::property::{NodeDetail, NodeDetailSet};
    use verbatim_uia::{AncestorGates, Node, NodeGates, Shape};

    use super::{ancestor_gates, focus_gates, many_node_gates, node_gates};

    /// The gates for every detail but `off`, as a node's
    /// and as the focus's, which must equal `expected` and that node
    /// within an ancestor, and as an ancestor's and a node's among many,
    /// which are those without the place and the cell.
    fn assert_only(off: NodeDetail, expected: NodeGates) {
        let details = NodeDetailSet::ALL.without(off);
        // Only Report Current Object's live read takes a table's counts.
        let expected = NodeGates {
            counts: None,
            ..expected
        };
        let among_many = NodeGates {
            place: None,
            cell: None,
            ..expected
        };
        assert_eq!(
            (
                node_gates(details),
                many_node_gates(details),
                focus_gates(details),
                ancestor_gates(details),
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
            NodeDetail::Description,
            NodeGates {
                description: None,
                ..Node::all_on()
            },
        );
    }

    #[test]
    fn shortcuts_off_turn_off_the_shortcut_alone() {
        assert_only(
            NodeDetail::KeyboardShortcut,
            NodeGates {
                shortcut: None,
                ..Node::all_on()
            },
        );
    }

    #[test]
    fn positions_off_turn_off_the_position_alone() {
        assert_only(
            NodeDetail::Position,
            NodeGates {
                position: None,
                ..Node::all_on()
            },
        );
    }

    #[test]
    fn cell_coordinates_off_turn_off_the_cell_alone() {
        assert_only(
            NodeDetail::Cell,
            NodeGates {
                cell: None,
                ..Node::all_on()
            },
        );
    }

    #[test]
    fn levels_off_turn_off_the_level_alone() {
        assert_only(
            NodeDetail::Level,
            NodeGates {
                level: false,
                ..Node::all_on()
            },
        );
    }
}
