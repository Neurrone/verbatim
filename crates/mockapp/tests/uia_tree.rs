//! Cross-process UIA tree construction (architecture section 13, layer 2).
//!
//! Spawns `mockapp --backend uia` over `tests/fixtures/tree.json`, then
//! walks the tree through `verbatim-uia`'s real client (`Uia::element_from_handle`
//! plus the base cache request) exactly as an outpost would, and asserts the
//! whole tree read back, every node's role, name, value, whole state set,
//! and details, and every node's children in order, equals the fixture's. This is
//! the "provider-level fake" promised by architecture section 13: COM
//! marshaling, cache requests, and role/state mapping are all exercised for
//! real, with no actual application.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::{NodeDetails, Role, State, StateSet};
use verbatim_uia::{NodeIdRegistry, Uia, map};
use windows::Win32::UI::Accessibility::{IUIAutomationElement, TreeScope_Children};

/// One node of the tree as UIA reads it, with its children in order.
#[derive(Debug, PartialEq)]
struct Node {
    name: Option<String>,
    role: Role,
    value: Option<String>,
    states: StateSet,
    /// Every detail but the rectangle: mockapp scripts no geometry (its
    /// providers answer a zero rectangle, which the client maps to `None`),
    /// but the root element is hwnd-hosted, so UIA merges the real window's
    /// rectangle into it.
    details: NodeDetails,
    children: Vec<Node>,
}

/// A fixture node with no value or details and no children.
fn leaf(name: &str, role: Role, states: &[State]) -> Node {
    Node {
        name: Some(name.to_owned()),
        role,
        value: None,
        states: states.iter().copied().collect(),
        details: NodeDetails::default(),
        children: Vec::new(),
    }
}

/// [`leaf`], with children.
fn parent(name: &str, role: Role, states: &[State], children: Vec<Node>) -> Node {
    Node {
        children,
        ..leaf(name, role, states)
    }
}

/// [`leaf`], with a value.
fn valued(name: &str, role: Role, value: &str) -> Node {
    Node {
        value: Some(value.to_owned()),
        ..leaf(name, role, &[State::Focusable])
    }
}

/// `tree.json` as UIA reports it, every detail the fixture scripts
/// included, for a window titled `title`. The root's first child is not the fixture's: it is the title
/// bar of mockapp's frame, which UIA's own proxy for the window adds, and
/// whose subtree, the frame's buttons, is Windows' and named in the
/// machine's language, so the walk does not go into it.
fn expected_tree(title: &str) -> Node {
    use State::{
        Checkable, Checked, Collapsed, Disabled, Expanded, Focusable, Mixed, Offscreen, Pressed,
    };
    // The title bar's value is the window's title.
    let title_bar = Node {
        name: None,
        role: Role::TitleBar,
        value: Some(title.to_owned()),
        states: [Focusable].into_iter().collect(),
        details: NodeDetails::default(),
        children: Vec::new(),
    };
    parent(
        "Mockapp Tree Fixture",
        Role::Window,
        &[],
        vec![
            title_bar,
            parent(
                "Group One",
                Role::Group,
                &[],
                vec![
                    Node {
                        details: NodeDetails {
                            description: Some(
                                "Applies the changes and closes the dialog".to_owned(),
                            ),
                            keyboard_shortcut: Some("Alt+O".to_owned()),
                            ..NodeDetails::default()
                        },
                        ..leaf("OK", Role::Button, &[Focusable])
                    },
                    leaf("Cancel", Role::Button, &[Focusable, Disabled]),
                    leaf("Enable feature", Role::CheckBox, &[Focusable, Checked]),
                    leaf("Partial", Role::CheckBox, &[Focusable, Mixed]),
                    // A radio button can be checked, as UIA maps its
                    // selection item pattern.
                    leaf(
                        "Option A",
                        Role::RadioButton,
                        &[Focusable, Checked, Checkable],
                    ),
                    leaf("Wireless", Role::ToggleButton, &[Focusable, Pressed]),
                    leaf("Airplane mode", Role::ToggleButton, &[Focusable]),
                ],
            ),
            parent(
                "Items",
                Role::List,
                &[],
                vec![
                    Node {
                        details: NodeDetails {
                            position_in_set: Some(1),
                            set_size: Some(2),
                            ..NodeDetails::default()
                        },
                        ..leaf("First", Role::ListItem, &[Focusable])
                    },
                    Node {
                        details: NodeDetails {
                            position_in_set: Some(2),
                            set_size: Some(2),
                            level: Some(1),
                            ..NodeDetails::default()
                        },
                        ..leaf("Second", Role::ListItem, &[Focusable, Offscreen])
                    },
                ],
            ),
            parent(
                "Choices",
                Role::ComboBox,
                &[Focusable, Expanded],
                vec![leaf("Alpha", Role::MenuItem, &[])],
            ),
            valued("Notes", Role::EditableText, "Hello world"),
            valued("Volume", Role::Slider, "50"),
            valued("Count", Role::SpinButton, "3"),
            parent(
                "Sections",
                Role::TabControl,
                &[Collapsed],
                vec![
                    leaf("General", Role::Tab, &[Focusable]),
                    leaf("Advanced", Role::Tab, &[Focusable]),
                ],
            ),
            leaf("Learn more", Role::Link, &[Focusable]),
            leaf("Main Toolbar", Role::ToolBar, &[]),
            leaf("Ready", Role::StatusBar, &[]),
            leaf("Popup", Role::Menu, &[]),
        ],
    )
}

/// Reads `element` and everything below it, but for the frame's title bar,
/// read alone (see [`expected_tree`]).
fn walk(uia: &Uia, element: &IUIAutomationElement, registry: &NodeIdRegistry) -> Node {
    let snapshot = map::snapshot_from_cached_element(element, registry);
    let mut details = snapshot.details;
    details.rect = None;
    let children = if snapshot.role == Role::TitleBar {
        Vec::new()
    } else {
        let cache = uia.base_cache_request().expect("base cache request");
        // SAFETY: a live client; the condition takes no arguments.
        let condition = unsafe { uia.client().CreateTrueCondition() }.expect("CreateTrueCondition");
        // SAFETY: `element` is a live, cached element on this client's own
        // apartment thread.
        let children = unsafe { element.FindAllBuildCache(TreeScope_Children, &condition, &cache) }
            .expect("FindAllBuildCache");
        verbatim_uia::elements_of(&children)
            .iter()
            .map(|child| walk(uia, child, registry))
            .collect()
    };
    Node {
        name: snapshot.name,
        role: snapshot.role,
        value: snapshot.value,
        states: snapshot.states,
        details,
        children,
    }
}

fn uia_client_reads_the_scripted_tree() {
    let title = common::unique_title("mockapp-uia-tree");
    let app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let uia = Uia::new().expect("Uia::new");
    let cache = uia.base_cache_request().expect("base cache request");
    let root = uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("element_from_handle");
    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
    assert_eq!(walk(&uia, &root, &registry), expected_tree(&title));

    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[(
        "uia_client_reads_the_scripted_tree",
        uia_client_reads_the_scripted_tree,
    )]);
}
