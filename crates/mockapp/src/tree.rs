//! The owned, mutable scripted tree (architecture section 13, layer 2).
//!
//! Parsed once from the fixture at startup into a flat arena (nodes are
//! never reallocated, so indices are stable for the process's lifetime),
//! then mutated by stdin commands and read by whichever provider backend is
//! active. Index 0 is always the root, which conceptually corresponds to
//! the host window itself.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use verbatim_model::{Role, StateSet};

use crate::fixture::FixtureNode;

/// One node's live data.
pub(crate) struct NodeData {
    pub(crate) role: Role,
    pub(crate) name: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) states: StateSet,
    /// Accessible description (UIA `FullDescription`, MSAA `accDescription`).
    pub(crate) description: Option<String>,
    /// Advertised keyboard shortcut (UIA `AccessKey`, MSAA
    /// `accKeyboardShortcut`).
    pub(crate) keyboard_shortcut: Option<String>,
    /// One-based position within the containing set (UIA `PositionInSet`;
    /// plain MSAA cannot express it).
    pub(crate) position_in_set: Option<u32>,
    /// Size of the containing set (UIA `SizeOfSet`).
    pub(crate) set_size: Option<u32>,
    /// One-based nesting level (UIA `Level`).
    pub(crate) level: Option<u32>,
    pub(crate) parent: Option<usize>,
    pub(crate) children: Vec<usize>,
}

/// The whole scripted tree: an arena of [`NodeData`] plus a lookup from
/// fixture id to arena index, and the currently focused and selected nodes
/// if any.
pub(crate) struct Tree {
    pub(crate) nodes: Vec<NodeData>,
    by_fixture_id: HashMap<String, usize>,
    pub(crate) focused: Option<usize>,
    /// The currently selected node (single-selection model, mirroring
    /// [`Self::focused`]): set by the `select` stdin command, which moves
    /// the `Selected` state here from any previously selected node.
    pub(crate) selected: Option<usize>,
}

/// A tree shared between the window thread (which owns every provider COM
/// object) and the stdin-command handling that runs on that same thread;
/// the `Arc` lets provider objects hold their own reference without
/// borrowing from the window's state.
pub(crate) type SharedTree = Arc<Mutex<Tree>>;

impl Tree {
    /// Flattens a parsed fixture tree into the arena, depth-first, so the
    /// root is always index 0.
    #[must_use]
    pub(crate) fn build(root: FixtureNode) -> Self {
        let mut nodes = Vec::new();
        let mut by_fixture_id = HashMap::new();
        insert(&mut nodes, &mut by_fixture_id, root, None);
        Self {
            nodes,
            by_fixture_id,
            focused: None,
            selected: None,
        }
    }

    /// Looks up a node's arena index by its fixture id.
    #[must_use]
    pub(crate) fn index_of(&self, fixture_id: &str) -> Option<usize> {
        self.by_fixture_id.get(fixture_id).copied()
    }

    /// The sibling one step in `offset` direction from `index` (`1` for
    /// next, `-1` for previous), `None` at either end or for the root.
    #[must_use]
    pub(crate) fn sibling(&self, index: usize, offset: isize) -> Option<usize> {
        let parent = self.nodes[index].parent?;
        let siblings = &self.nodes[parent].children;
        let position = siblings.iter().position(|&child| child == index)?;
        let target = position.checked_add_signed(offset)?;
        siblings.get(target).copied()
    }
}

fn insert(
    nodes: &mut Vec<NodeData>,
    by_fixture_id: &mut HashMap<String, usize>,
    node: FixtureNode,
    parent: Option<usize>,
) -> usize {
    let index = nodes.len();
    nodes.push(NodeData {
        role: node.role,
        name: node.name,
        value: node.value,
        states: node.states,
        description: node.description,
        keyboard_shortcut: node.keyboard_shortcut,
        position_in_set: node.position_in_set,
        set_size: node.set_size,
        level: node.level,
        parent,
        children: Vec::new(),
    });
    by_fixture_id.insert(node.id, index);
    let mut child_indices = Vec::with_capacity(node.children.len());
    for child in node.children {
        child_indices.push(insert(nodes, by_fixture_id, child, Some(index)));
    }
    nodes[index].children = child_indices;
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, role: Role, name: &str, children: Vec<FixtureNode>) -> FixtureNode {
        FixtureNode {
            id: id.to_owned(),
            role,
            name: Some(name.to_owned()),
            value: None,
            states: StateSet::new(),
            description: None,
            keyboard_shortcut: None,
            position_in_set: None,
            set_size: None,
            level: None,
            children,
        }
    }

    fn sample() -> FixtureNode {
        node(
            "root",
            Role::Window,
            "Root",
            vec![
                node("a", Role::Button, "A", vec![]),
                node(
                    "b",
                    Role::Button,
                    "B",
                    vec![node("b1", Role::StaticText, "B1", vec![])],
                ),
            ],
        )
    }

    #[test]
    fn root_is_index_zero_and_children_follow() {
        let tree = Tree::build(sample());
        assert_eq!(tree.index_of("root"), Some(0));
        assert!(tree.index_of("a").is_some());
        assert!(tree.index_of("b").is_some());
        assert!(tree.index_of("b1").is_some());
        assert_eq!(tree.nodes[0].children.len(), 2);
    }

    #[test]
    fn siblings() {
        let tree = Tree::build(sample());
        let a = tree.index_of("a").unwrap();
        let b = tree.index_of("b").unwrap();
        assert_eq!(tree.sibling(a, 1), Some(b));
        assert_eq!(tree.sibling(b, 1), None);
        assert_eq!(tree.sibling(a, -1), None);
    }

    #[test]
    fn nested_child_is_reachable() {
        let tree = Tree::build(sample());
        let b = tree.index_of("b").unwrap();
        let b1 = tree.index_of("b1").unwrap();
        assert_eq!(tree.nodes[b].children, vec![b1]);
        assert_eq!(tree.nodes[b1].parent, Some(b));
    }
}
