//! The gates of the UIA shapes the outpost fetches with
//! (`docs/design/uia-shapes.md`, section 4), from the details the active
//! theme reports ([`Fetches`]): one function per gated shape. A detail the
//! theme reports as off is not fetched, so the provider is not asked for
//! it. This is the one link the types cannot make, since the theme is in
//! a platform-neutral crate that knows nothing of UI Automation, so each
//! gated field has a unit test.

use verbatim_model::{Fetches, NodeDetails};
use verbatim_uia::{AncestorGates, NodeGates};

/// The gates of a [`verbatim_uia::Node`] read alone, such as a focus or a
/// navigation step's neighbour, for the details `fetches` names: its
/// description, keyboard shortcut, position in its set, and level, and
/// its row and column in a grid; and its place, its rectangle and whether
/// it is off the screen, which a read of one element includes.
#[must_use]
pub fn node_gates(fetches: &Fetches) -> NodeGates {
    NodeGates {
        place: Some(()),
        cell: fetches.cell_coordinates.then_some(()),
        ..many_node_gates(fetches)
    }
}

/// The gates of a [`verbatim_uia::Node`] read among many, such as a tree
/// dump's: [`node_gates`] without its place or its row and column, which a
/// read of many elements never includes (`verbatim_uia::Place`,
/// `verbatim_uia::Cell`).
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
        cell: None,
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

/// `details`, read whatever the theme, as the listener's selection is from
/// its event's cache (`verbatim_uia::ListenerSelection`), without those
/// `fetches` does not name: what a read with [`node_gates`] would have
/// left unfetched.
pub fn leave_out_unreported(details: &mut NodeDetails, fetches: &Fetches) {
    if !fetches.description {
        details.description = None;
    }
    if !fetches.shortcut {
        details.keyboard_shortcut = None;
    }
    if !fetches.position {
        details.position_in_set = None;
        details.set_size = None;
    }
    if !fetches.level {
        details.level = None;
    }
    if !fetches.cell_coordinates {
        details.row = None;
        details.column = None;
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::{Fetches, NodeDetails};
    use verbatim_uia::{AncestorGates, Node, NodeGates, Shape};

    use super::{ancestor_gates, focus_gates, leave_out_unreported, many_node_gates, node_gates};

    /// The gates for every detail but the one `off` turns off, as a node's
    /// and as the focus's, which must equal `expected` and that node
    /// within an ancestor, and as an ancestor's and a node's among many,
    /// which are those without the place and the cell.
    fn assert_only(off: impl FnOnce(&mut Fetches), expected: NodeGates) {
        let mut fetches = Fetches::default();
        off(&mut fetches);
        let among_many = NodeGates {
            place: None,
            cell: None,
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
    fn cell_coordinates_off_turn_off_the_cell_alone() {
        assert_only(
            |fetches| fetches.cell_coordinates = false,
            NodeGates {
                cell: None,
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

    /// Each detail the theme does not report is left out of details read
    /// whatever the theme, and nothing else.
    #[test]
    fn each_unreported_detail_is_left_out_alone() {
        let read = NodeDetails {
            description: Some("Opens it".to_owned()),
            keyboard_shortcut: Some("Alt+O".to_owned()),
            position_in_set: Some(2),
            set_size: Some(3),
            level: Some(1),
            row: Some(4),
            column: Some(5),
            ..NodeDetails::default()
        };
        let left = |off: fn(&mut Fetches)| {
            let mut fetches = Fetches::default();
            off(&mut fetches);
            let mut details = read.clone();
            leave_out_unreported(&mut details, &fetches);
            details
        };
        assert_eq!(left(|_| {}), read);
        assert_eq!(
            left(|fetches| fetches.description = false),
            NodeDetails {
                description: None,
                ..read.clone()
            }
        );
        assert_eq!(
            left(|fetches| fetches.shortcut = false),
            NodeDetails {
                keyboard_shortcut: None,
                ..read.clone()
            }
        );
        assert_eq!(
            left(|fetches| fetches.position = false),
            NodeDetails {
                position_in_set: None,
                set_size: None,
                ..read.clone()
            }
        );
        assert_eq!(
            left(|fetches| fetches.level = false),
            NodeDetails {
                level: None,
                ..read.clone()
            }
        );
        assert_eq!(
            left(|fetches| fetches.cell_coordinates = false),
            NodeDetails {
                row: None,
                column: None,
                ..read.clone()
            }
        );
    }
}
