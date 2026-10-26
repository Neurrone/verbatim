//! The owned, mutable scripted tree (architecture section 13, layer 2).
//!
//! Parsed once from the fixture at startup into a flat arena (nodes are
//! never reallocated, so indices are stable for the process's lifetime),
//! then mutated by stdin commands and read by whichever provider backend is
//! active. Index 0 is always the root, which conceptually corresponds to
//! the host window itself.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use verbatim_model::{Role, State, StateSet};

use crate::fixture::FixtureNode;

/// One node's live data.
#[derive(Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches the fixture's commands set one by one"
)]
pub(crate) struct NodeData {
    pub(crate) role: Role,
    pub(crate) name: Option<String>,
    /// The UIA class name (`ClassName`).
    pub(crate) class_name: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) states: StateSet,
    /// Accessible description (UIA `FullDescription`, MSAA `accDescription`).
    pub(crate) description: Option<String>,
    /// Advertised keyboard shortcut (UIA `AccessKey`, MSAA
    /// `accKeyboardShortcut`).
    pub(crate) keyboard_shortcut: Option<String>,
    /// The UIA automation id (`AutomationId`).
    pub(crate) automation_id: Option<String>,
    /// The default action's name (MSAA `accDefaultAction`).
    pub(crate) default_action: Option<String>,
    /// One-based position within the containing set (UIA `PositionInSet`;
    /// plain MSAA cannot express it).
    pub(crate) position_in_set: Option<u32>,
    /// Size of the containing set (UIA `SizeOfSet`).
    pub(crate) set_size: Option<u32>,
    /// One-based nesting level (UIA `Level`).
    pub(crate) level: Option<u32>,
    /// The node this one controls (UIA `ControllerFor`).
    pub(crate) controller_for: Option<usize>,
    /// The node's text as UTF-16, served through UIA's text pattern.
    pub(crate) text: Option<Vec<u16>>,
    /// The text's selection, start and end as UTF-16 offsets; the caret is
    /// at its start, and a collapsed selection is the caret alone.
    pub(crate) selection: (usize, usize),
    /// `TextPattern2`'s caret is the selection's end, not its start
    /// (`caret <id> <start> <end> end`).
    pub(crate) caret_at_end: bool,
    /// How many of the text's last lines are on screen, as a terminal shows
    /// the end of its buffer (`screen`); `None` shows the whole text.
    pub(crate) screen_rows: Option<usize>,
    /// Text written over the text at the caret, and the caret moved past
    /// it, as the text is next read (`write-on-read`).
    pub(crate) write_on_read: Option<Vec<u16>>,
    /// The text's spelling errors and bold stretches, UTF-16 offsets.
    pub(crate) formats: Formats,
    pub(crate) parent: Option<usize>,
    pub(crate) children: Vec<usize>,
    /// The number the node's UIA runtime id is made from: its own index,
    /// until `take-runtime-id` gives it a dead node's.
    pub(crate) runtime_id: usize,
    /// Whether the node has died (`take-runtime-id`, `replace-items`,
    /// `disconnect`): every call on its elements fails as on an element
    /// that is gone, and it has left the tree, unless `disconnect` killed
    /// it where it stands.
    pub(crate) dead: bool,
    /// Whether the node dies where it stands the first time its UIA
    /// provider is asked for its first child, that call failing as on an
    /// element that is gone (`disconnect-at-first-child`).
    pub(crate) dies_at_first_child: bool,
    /// How the node's UIA provider answers `Navigate(Parent)`, once it has
    /// answered as many calls as usual as the count says (`answer-parent`).
    pub(crate) parent_answer: (crate::stdin::ParentAnswer, u32),
    /// The handle of a window that no longer exists, which the node's UIA
    /// provider answers `NativeWindowHandle` with (`report-window`), and
    /// `None` for its usual answer.
    pub(crate) reports_window: Option<isize>,
    /// Whether UIA counts the node as content (`IsContentElement`).
    pub(crate) content: bool,
}

/// A text's formatting: stretches that are spelling errors, that are bold,
/// and that are italic, each a start and an end UTF-16 offset, whether its
/// `IsItalic` attribute fails to read, whether `FindText` fails, whether
/// lines below the last with text clamp back to it, stretches in another
/// language, each with its locale id, its other formatting, and whether a
/// backward move answers with a positive count.
#[derive(Clone, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fixture option"
)]
pub(crate) struct Formats {
    pub(crate) spelling_errors: Vec<(usize, usize)>,
    pub(crate) bold: Vec<(usize, usize)>,
    pub(crate) italic: Vec<(usize, usize)>,
    pub(crate) italic_fails: bool,
    pub(crate) find_text_fails: bool,
    pub(crate) find_backward_fails: bool,
    pub(crate) lines_clamp: bool,
    pub(crate) cultures: Vec<(usize, usize, i32)>,
    pub(crate) styles: crate::fixture::Styles,
    pub(crate) backward_moves_positive: bool,
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
    /// The node whose children are addressed as numbered simple children
    /// of it, as a Win32 tree view's items are: set by `focus-child`, after
    /// which the node's `accFocus` names its focused child by child id and
    /// its `accChild` has no object for any child. MSAA-only.
    pub(crate) simple_children: Option<usize>,
}

/// A tree shared between the window thread (which owns every provider COM
/// object) and the stdin-command handling that runs on that same thread;
/// the `Arc` lets provider objects hold their own reference without
/// borrowing from the window's state.
pub(crate) type SharedTree = Arc<Mutex<Tree>>;

impl Tree {
    /// Flattens a parsed fixture tree into the arena, depth-first, so the
    /// root is always index 0. The focus starts on the first node in the
    /// `focused` state, if any, as an application's window opens with its
    /// focus already on a control.
    #[must_use]
    pub(crate) fn build(root: FixtureNode) -> Self {
        let mut nodes = Vec::new();
        let mut by_fixture_id = HashMap::new();
        let mut controllers = Vec::new();
        insert(&mut nodes, &mut by_fixture_id, &mut controllers, root, None);
        // Resolved once every id has its index; the fixture loader has
        // already checked that each names a node.
        for (index, controlled) in controllers {
            nodes[index].controller_for = by_fixture_id.get(&controlled).copied();
        }
        let focused = nodes
            .iter()
            .position(|node| node.states.contains(State::Focused));
        Self {
            nodes,
            by_fixture_id,
            focused,
            selected: None,
            simple_children: None,
        }
    }

    /// Looks up a node's arena index by its fixture id.
    #[must_use]
    pub(crate) fn index_of(&self, fixture_id: &str) -> Option<usize> {
        self.by_fixture_id.get(fixture_id).copied()
    }

    /// Node `from` dies, leaving the tree, and node `id` takes its runtime
    /// id (`take-runtime-id`). A dead node loses the focus and the
    /// selection if it had them.
    pub(crate) fn take_runtime_id(&mut self, id: &str, from: &str) -> Result<(), String> {
        let unknown = |name: &str| format!("no node has the id {name}");
        let taker = self.index_of(id).ok_or_else(|| unknown(id))?;
        let dying = self.index_of(from).ok_or_else(|| unknown(from))?;
        if taker == dying || dying == 0 || self.nodes[dying].dead {
            return Err(format!("{from} cannot give its runtime id to {id}"));
        }
        if let Some(parent) = self.nodes[dying].parent {
            self.nodes[parent].children.retain(|&child| child != dying);
        }
        self.nodes[dying].dead = true;
        if self.focused == Some(dying) {
            self.focused = None;
        }
        if self.selected == Some(dying) {
            self.selected = None;
        }
        self.nodes[taker].runtime_id = self.nodes[dying].runtime_id;
        Ok(())
    }

    /// Node `id` dies where it stands, its parent still listing it
    /// (`disconnect`). The root cannot die.
    pub(crate) fn disconnect(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .index_of(id)
            .ok_or_else(|| format!("no node has the id {id}"))?;
        if index == 0 || self.nodes[index].dead {
            return Err(format!("{id} cannot be disconnected"));
        }
        self.nodes[index].dead = true;
        Ok(())
    }

    /// Node `id` dies where it stands the first time its UIA provider is
    /// asked for its first child (`disconnect-at-first-child`). The root
    /// cannot die.
    pub(crate) fn disconnect_at_first_child(&mut self, id: &str) -> Result<(), String> {
        let index = self
            .index_of(id)
            .ok_or_else(|| format!("no node has the id {id}"))?;
        if index == 0 || self.nodes[index].dead {
            return Err(format!("{id} cannot be disconnected"));
        }
        self.nodes[index].dies_at_first_child = true;
        Ok(())
    }

    /// Every child of node `id`, with everything under it, dies, and a copy
    /// of each, with a runtime id of its own, takes its place under the
    /// same fixture id (`replace-items`): the copies keep their states, so
    /// the copy of the selected node is the selected node, and the focus
    /// and any `ControllerFor` relation naming a node that died move to its
    /// copy.
    pub(crate) fn replace_items(&mut self, id: &str) -> Result<(), String> {
        let parent = self
            .index_of(id)
            .ok_or_else(|| format!("no node has the id {id}"))?;
        if self.nodes[parent].dead {
            return Err(format!("{id} has died"));
        }
        let children = self.nodes[parent].children.clone();
        let copies = children
            .into_iter()
            .map(|child| self.replace(child, parent))
            .collect();
        self.nodes[parent].children = copies;
        Ok(())
    }

    /// Node `index` and everything under it die, copied under `parent`
    /// ([`Self::replace_items`]); the copy's index.
    fn replace(&mut self, index: usize, parent: usize) -> usize {
        let copy = self.nodes.len();
        let mut node = self.nodes[index].clone();
        node.parent = Some(parent);
        node.runtime_id = copy;
        node.children = Vec::new();
        self.nodes.push(node);
        self.nodes[index].dead = true;
        for at in self.by_fixture_id.values_mut() {
            if *at == index {
                *at = copy;
            }
        }
        for node in &mut self.nodes {
            if node.controller_for == Some(index) {
                node.controller_for = Some(copy);
            }
        }
        if self.focused == Some(index) {
            self.focused = Some(copy);
        }
        if self.selected == Some(index) {
            self.selected = Some(copy);
        }
        let children = self.nodes[index].children.clone();
        let copies = children
            .into_iter()
            .map(|child| self.replace(child, copy))
            .collect();
        self.nodes[copy].children = copies;
        copy
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
    controllers: &mut Vec<(usize, String)>,
    node: FixtureNode,
    parent: Option<usize>,
) -> usize {
    let index = nodes.len();
    nodes.push(NodeData {
        role: node.role,
        name: node.name,
        class_name: node.class_name,
        value: node.value,
        states: node.states,
        description: node.description,
        keyboard_shortcut: node.keyboard_shortcut,
        automation_id: node.automation_id,
        default_action: node.default_action,
        position_in_set: node.position_in_set,
        set_size: node.set_size,
        level: node.level,
        controller_for: None,
        text: node.text.map(|text| text.encode_utf16().collect()),
        selection: (0, 0),
        caret_at_end: false,
        screen_rows: None,
        write_on_read: None,
        formats: Formats {
            spelling_errors: node.spelling_errors,
            bold: node.bold,
            italic: node.italic,
            italic_fails: node.italic_fails,
            find_text_fails: node.find_text_fails,
            find_backward_fails: node.find_backward_fails,
            lines_clamp: node.lines_clamp,
            cultures: node.cultures,
            styles: *node.styles,
            backward_moves_positive: node.backward_moves_positive,
        },
        parent,
        children: Vec::new(),
        runtime_id: index,
        dead: false,
        dies_at_first_child: false,
        parent_answer: (crate::stdin::ParentAnswer::Usual, 0),
        reports_window: None,
        content: node.content,
    });
    by_fixture_id.insert(node.id, index);
    if let Some(controlled) = node.controller_for {
        controllers.push((index, controlled));
    }
    let mut child_indices = Vec::with_capacity(node.children.len());
    for child in node.children {
        child_indices.push(insert(
            nodes,
            by_fixture_id,
            controllers,
            child,
            Some(index),
        ));
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
            automation_id: None,
            default_action: None,
            position_in_set: None,
            set_size: None,
            level: None,
            controller_for: None,
            text: None,
            spelling_errors: Vec::new(),
            bold: Vec::new(),
            italic: Vec::new(),
            italic_fails: false,
            find_text_fails: false,
            find_backward_fails: false,
            lines_clamp: false,
            cultures: Vec::new(),
            styles: Box::default(),
            backward_moves_positive: false,
            native: None,
            window_class: None,
            class_name: None,
            columns: Vec::new(),
            state_images: false,
            edit_version_6: false,
            content: true,
            msaa_client: false,
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
        let indices: Vec<_> = ["root", "a", "b", "b1"]
            .iter()
            .map(|id| tree.index_of(id))
            .collect();
        assert_eq!(indices, [Some(0), Some(1), Some(2), Some(3)], "depth first");
        assert_eq!(tree.index_of("nosuch"), None);
        assert_eq!(tree.nodes[0].children, [1, 2]);
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

    #[test]
    fn a_node_takes_a_dead_nodes_runtime_id() {
        let mut tree = Tree::build(sample());
        let a = tree.index_of("a").unwrap();
        let b1 = tree.index_of("b1").unwrap();
        tree.take_runtime_id("a", "b1").unwrap();
        assert_eq!(tree.nodes[a].runtime_id, b1);
        assert!(tree.nodes[b1].dead);
        assert_eq!(
            tree.nodes[tree.index_of("b").unwrap()].children,
            Vec::<usize>::new()
        );
        assert!(
            tree.take_runtime_id("b", "b1").is_err(),
            "b1 is already dead"
        );
    }

    #[test]
    fn replaced_items_die_and_their_copies_take_their_places_and_ids() {
        let mut tree = Tree::build(sample());
        let [a, b, b1] = ["a", "b", "b1"].map(|id| tree.index_of(id).unwrap());
        tree.selected = Some(b);
        tree.replace_items("root").unwrap();
        let copies = ["a", "b", "b1"].map(|id| tree.index_of(id).unwrap());
        assert_eq!(copies, [4, 5, 6], "each copy is new, depth first");
        assert_eq!(tree.nodes[0].children, [4, 5]);
        assert_eq!(tree.nodes[5].children, [6]);
        assert_eq!(tree.nodes[6].parent, Some(5));
        assert_eq!(
            copies.map(|copy| tree.nodes[copy].runtime_id),
            [4, 5, 6],
            "each copy's runtime id is its own"
        );
        assert!([a, b, b1].iter().all(|&old| tree.nodes[old].dead));
        assert_eq!(tree.selected, Some(5), "the selection moves to the copy");
    }
}
