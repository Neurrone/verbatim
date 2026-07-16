//! Nodes of the normalized tree: backends, roles, states, and snapshots.

use serde::{Deserialize, Serialize};

/// The accessibility API a node or event was sourced from.
///
/// Arbitration picks the backend per window handle (architecture section 4);
/// nodes are tagged so one tree fragment can mix sources, stitched at window
/// boundaries. Nothing above the outpost may branch on this — it exists for
/// diagnostics and for outpost-internal identity mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Backend {
    /// UI Automation.
    Uia,
    /// MSAA (`IAccessible`), later extended with IA2 interfaces.
    Msaa,
}

/// Verbatim's role vocabulary, a superset both backends map into.
///
/// M1 carries the roles that appear in Verbatim's own menu and settings
/// dialog plus common shell roles; the set grows with later milestones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Role {
    /// A top-level window.
    Window,
    /// A dialog window.
    Dialog,
    /// A generic pane or container.
    Pane,
    /// A settings-category page inside a dialog.
    PropertyPage,
    /// A grouping of related controls, such as a labeled box.
    Group,
    /// A menu bar.
    MenuBar,
    /// A popup menu.
    Menu,
    /// One item inside a menu.
    MenuItem,
    /// A push button.
    Button,
    /// A check box.
    CheckBox,
    /// A radio button.
    RadioButton,
    /// A combo box or dropdown choice.
    ComboBox,
    /// A list container.
    List,
    /// One item inside a list.
    ListItem,
    /// A slider or trackbar.
    Slider,
    /// A spin button.
    SpinButton,
    /// A tab control.
    TabControl,
    /// One tab inside a tab control.
    Tab,
    /// Static, non-editable text.
    StaticText,
    /// Editable text, single or multi line.
    EditableText,
    /// A hyperlink.
    Link,
    /// A tool bar.
    ToolBar,
    /// A status bar.
    StatusBar,
    /// A tree view control.
    Tree,
    /// One item inside a tree view.
    TreeItem,
    /// Anything not yet mapped into the vocabulary.
    Unknown,
}

/// One state a node can carry; combined in a [`StateSet`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
#[repr(u8)]
pub enum State {
    /// Has keyboard focus right now.
    Focused,
    /// Can accept keyboard focus.
    Focusable,
    /// Selected within its container.
    Selected,
    /// Can be selected within its container.
    Selectable,
    /// Checked (check box, menu item, toggle).
    Checked,
    /// In the indeterminate, half-checked state.
    Mixed,
    /// Disabled or unavailable.
    Disabled,
    /// Read-only.
    ReadOnly,
    /// Expanded (combo box, tree item, expander).
    Expanded,
    /// Collapsed.
    Collapsed,
    /// Pressed (toggle button).
    Pressed,
    /// Opens a submenu or popup.
    HasPopup,
    /// The default control of its dialog.
    DefaultControl,
    /// Scrolled or positioned out of view.
    Offscreen,
    /// Busy loading or updating.
    Busy,
}

impl State {
    /// Every state, in declaration order; the basis for [`StateSet::iter`].
    pub const ALL: [State; 15] = [
        State::Focused,
        State::Focusable,
        State::Selected,
        State::Selectable,
        State::Checked,
        State::Mixed,
        State::Disabled,
        State::ReadOnly,
        State::Expanded,
        State::Collapsed,
        State::Pressed,
        State::HasPopup,
        State::DefaultControl,
        State::Offscreen,
        State::Busy,
    ];

    const fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

/// A set of [`State`]s, stored as a bitmask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StateSet(u32);

impl StateSet {
    /// The empty set.
    #[must_use]
    pub const fn new() -> Self {
        Self(0)
    }

    /// Returns whether `state` is in the set.
    #[must_use]
    pub const fn contains(self, state: State) -> bool {
        self.0 & state.bit() != 0
    }

    /// Adds `state` to the set.
    pub const fn insert(&mut self, state: State) {
        self.0 |= state.bit();
    }

    /// Removes `state` from the set.
    pub const fn remove(&mut self, state: State) {
        self.0 &= !state.bit();
    }

    /// Returns a copy of the set with `state` added.
    #[must_use]
    pub const fn with(mut self, state: State) -> Self {
        self.insert(state);
        self
    }

    /// Returns whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The states in the set, in [`State::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = State> {
        State::ALL.into_iter().filter(move |s| self.contains(*s))
    }
}

impl FromIterator<State> for StateSet {
    fn from_iter<I: IntoIterator<Item = State>>(iter: I) -> Self {
        let mut set = Self::new();
        for state in iter {
            set.insert(state);
        }
        set
    }
}

/// A node's bounding rectangle in screen coordinates, as Windows reports
/// them: origin at the top-left of the primary monitor, y growing downward.
///
/// Carried on snapshots (inside [`NodeDetails`]) and on utterances (inside
/// `UtteranceSource`) so presentation themes can position audio by where a
/// control sits on screen (decision D12; milestone M11 consumes it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rect {
    /// Screen x of the left edge, in pixels.
    pub left: i32,
    /// Screen y of the top edge, in pixels.
    pub top: i32,
    /// Width in pixels.
    pub width: i32,
    /// Height in pixels.
    pub height: i32,
}

/// The optional per-node properties beyond name, role, value, and states —
/// grouped so [`NodeSnapshot`] literals name one field and so later
/// milestones grow this struct instead of the snapshot itself.
///
/// Everything here defaults to "not reported": backends fill in what they
/// can (UIA from cached properties, MSAA from its per-property calls), and
/// absence never suppresses an announcement — the reducer simply says less.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDetails {
    /// Accessible description, when it adds information beyond the name
    /// (UIA `FullDescription`/`HelpText`, MSAA `accDescription`).
    pub description: Option<String>,
    /// The keyboard shortcut the node advertises (UIA access and
    /// accelerator keys, MSAA `accKeyboardShortcut`).
    pub keyboard_shortcut: Option<String>,
    /// One-based position within the containing set, when the backend
    /// reports it (UIA `PositionInSet`; IA2 `groupPosition` later).
    pub position_in_set: Option<u32>,
    /// Size of the containing set, reported alongside
    /// [`position_in_set`](Self::position_in_set).
    pub set_size: Option<u32>,
    /// One-based nesting level (tree items, headings), when reported.
    pub level: Option<u32>,
    /// Bounding rectangle in screen coordinates, when reported.
    pub rect: Option<Rect>,
}

/// Everything the reducer needs to announce one node, captured in one place.
///
/// Snapshots are produced by outposts (from cached backend properties, never
/// by blocking calls on event threads) and consumed by the reducer, which
/// composes announcements from name, role, value, and states.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSnapshot {
    /// Outpost-stable identity of the node.
    pub id: NodeId,
    /// Which backend sourced this snapshot (diagnostics only above the outpost).
    pub backend: Backend,
    /// Normalized role.
    pub role: Role,
    /// Accessible name, if the node has one.
    pub name: Option<String>,
    /// Current value — slider position, combo selection, text content.
    pub value: Option<String>,
    /// Current states.
    pub states: StateSet,
    /// The optional properties beyond the core four. `#[serde(default)]`
    /// keeps snapshots recorded before this field existed (M2 flight
    /// recorder dumps) deserializing unchanged.
    #[serde(default)]
    pub details: NodeDetails,
}

/// One node of a walked accessibility tree: a snapshot plus its children in
/// tree order.
///
/// Carried unchanged by both the outpost protocol (the outpost's answer to
/// `DumpTree`) and the control protocol (`verbatim-inspect dump-tree`), so a
/// tree dump travels from the outpost through Core to the CLI without
/// translation — this is the shared vocabulary crate, so the shape is
/// defined once here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeNode {
    /// This node's snapshot.
    pub snapshot: NodeSnapshot,
    /// Children, in tree order.
    pub children: Vec<TreeNode>,
}

use crate::NodeId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_set_insert_contains_remove() {
        let mut states = StateSet::new();
        assert!(states.is_empty());
        states.insert(State::Focused);
        states.insert(State::Checked);
        assert!(states.contains(State::Focused));
        assert!(states.contains(State::Checked));
        assert!(!states.contains(State::Disabled));
        states.remove(State::Focused);
        assert!(!states.contains(State::Focused));
    }

    #[test]
    fn tree_node_round_trips_through_json() {
        let leaf = TreeNode {
            snapshot: NodeSnapshot {
                id: NodeId::new(2),
                backend: Backend::Uia,
                role: Role::Button,
                name: Some("OK".into()),
                value: None,
                states: StateSet::new(),
                details: NodeDetails::default(),
            },
            children: Vec::new(),
        };
        let root = TreeNode {
            snapshot: NodeSnapshot {
                id: NodeId::new(1),
                backend: Backend::Uia,
                role: Role::Dialog,
                name: Some("Settings".into()),
                value: None,
                states: StateSet::new(),
                details: NodeDetails::default(),
            },
            children: vec![leaf],
        };
        let json = serde_json::to_string(&root).expect("serializes");
        let back: TreeNode = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, root);
        assert_eq!(back.children.len(), 1);
    }

    #[test]
    fn state_set_iterates_in_declaration_order() {
        let states: StateSet = [State::Checked, State::Focused].into_iter().collect();
        let listed: Vec<State> = states.iter().collect();
        assert_eq!(listed, vec![State::Focused, State::Checked]);
    }

    #[test]
    fn state_bits_are_distinct() {
        for (index, state) in State::ALL.into_iter().enumerate() {
            for other in State::ALL.into_iter().skip(index + 1) {
                assert_ne!(
                    StateSet::new().with(state),
                    StateSet::new().with(other),
                    "{state:?} and {other:?} share a bit"
                );
            }
        }
    }
}
