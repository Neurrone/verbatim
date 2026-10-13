//! Cross-process UIA tree construction (architecture section 13, layer 2).
//!
//! Spawns `mockapp --backend uia` over `tests/fixtures/tree.json`, then
//! walks the tree through `verbatim-uia`'s real client (`Uia::window_element`
//! and `Uia::children`, every node read as the `Node` shape with every
//! detail) exactly as an outpost would, and asserts the
//! whole tree read back, every node's role, name, value, whole state set,
//! and details, and every node's children in order, equals the fixture's. This is
//! the "provider-level fake" promised by architecture section 13: COM
//! marshaling, cache requests, and role/state mapping are all exercised for
//! real, with no actual application.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::{NodeDetails, Role, State, StateSet};
use verbatim_uia::{
    Cached, Fill, Live, NodeIdRegistry, PropertyEvent, Request, Shape, TextEvent, Uia, map,
};
use verbatim_uia_rops::{NavigationDirection, Path, StepQuery, navigation_step};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    TreeScope_Descendants, TreeScope_Subtree, UIA_NamePropertyId,
};
use windows::core::BSTR;

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

/// Reads `element`, filled as a node with every detail, and everything
/// below it, each child read with `nodes`, but for the frame's title bar,
/// read alone (see [`expected_tree`]).
fn walk(
    uia: &Uia,
    element: &Cached<verbatim_uia::Node>,
    nodes: &Request<verbatim_uia::Node>,
    registry: &NodeIdRegistry,
) -> Node {
    let snapshot = element.snapshot(registry);
    let mut details = snapshot.details;
    details.rect = None;
    let children = if snapshot.role == Role::TitleBar {
        Vec::new()
    } else {
        uia.children(element.live(), nodes)
            .expect("the element's children")
            .iter()
            .map(|child| walk(uia, child, nodes, registry))
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
    let nodes = uia
        .request::<verbatim_uia::Node>(verbatim_uia::Node::all_on())
        .expect("the node's request");
    let root = uia
        .window_element(hwnd.0 as isize, &nodes)
        .expect("mockapp's window element");
    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
    assert_eq!(walk(&uia, &root, &nodes, &registry), expected_tree(&title));

    app.quit();
}

/// Every element of the scripted tree, filled with each event shape's own
/// request, reads as the shape the same role, name, value, states, and
/// window handle that the full read of the element gives, as a
/// `verbatim_uia::Node` with every detail, for the fields the shape
/// carries: a property event's parts, which have no details, and its window
/// handle; a text event's window handle.
fn event_shapes_read_what_the_full_read_gives() {
    let title = common::unique_title("mockapp-uia-event-shapes");
    let app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let uia = Uia::new().expect("Uia::new");
    let nodes = uia
        .request::<verbatim_uia::Node>(verbatim_uia::Node::all_on())
        .expect("the node's request");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    // SAFETY: a live client; the condition takes no arguments.
    let condition = unsafe { uia.client().CreateTrueCondition() }.expect("CreateTrueCondition");
    // SAFETY: `root` is a live element on this client's own apartment
    // thread; the search reads no cache.
    let elements =
        unsafe { root.element().FindAll(TreeScope_Subtree, &condition) }.expect("FindAll");
    let elements = verbatim_uia::elements_of(&elements);
    assert_eq!(
        elements.len(),
        30,
        "every element of the scripted tree, with the frame's title bar and its buttons"
    );
    let property_request = uia
        .request_fixed::<PropertyEvent>()
        .expect("the property event's request");
    let text_request = uia
        .request_fixed::<TextEvent>()
        .expect("the text event's request");
    for element in &elements {
        let live = Live::new(element.clone());
        let node = live.refresh(&nodes).expect("the element filled as a node");
        let full_node = node.read();
        let full = full_node.parts(node.runtime_id());
        let full_hwnd = full_node.role.hwnd;

        let property = live
            .refresh(&property_request)
            .expect("the element filled as a property event");
        let fields = property.read();
        let expected = map::CachedUiaParts {
            details: verbatim_model::NodeDetails::default(),
            ..full.clone()
        };
        assert_eq!(fields.parts(property.runtime_id()), expected);
        assert_eq!(fields.role.hwnd, full_hwnd, "{full:?}");

        let text = live
            .refresh(&text_request)
            .expect("the element filled as a text event");
        assert_eq!(text.read().hwnd, full_hwnd, "{full:?}");
    }

    app.quit();
}

/// A node whose kept element came from a text event, whose cache holds its
/// process, window and class name alone, is navigated remotely as the
/// outpost navigates it: the step query reads the kept element as `Kept`,
/// the shape every kept element covers, so neither the step nor the read of
/// the neighbour it reaches makes a failed cached read.
fn a_node_kept_from_a_text_event_navigates_remotely_with_no_failed_read() {
    let title = common::unique_title("mockapp-uia-kept-text-event");
    let app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title).0 as isize;

    let uia = Uia::new().expect("Uia::new");
    let root = uia.window_live(hwnd).expect("mockapp's window element");
    let condition = uia
        .property_condition(UIA_NamePropertyId, &VARIANT::from(BSTR::from("Notes")))
        .expect("a condition");
    let text_event = uia
        .request_fixed::<TextEvent>()
        .expect("the text event's request");
    let notes = root
        .find_first(TreeScope_Descendants, &condition, &text_event)
        .expect("the search")
        .expect("the Notes document, as a text event delivers it");
    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
    let node = registry.id_for_element(&notes.runtime_id(), &notes.narrow());
    let kept = registry
        .element_of(node)
        .expect("the node's kept element")
        .resolve()
        .expect("the kept element resolves");
    let fill = Fill::new(
        &uia.request::<verbatim_uia::Node>(verbatim_uia::Node::all_on())
            .expect("the node's request"),
    );

    let _ = verbatim_uia::calls::take_failed_reads();
    let query = StepQuery {
        element: &kept,
        direction: NavigationDirection::NextSibling,
        fill: &fill,
    };
    let (step, path) = navigation_step(&uia, &query, true).expect("the step");
    let neighbor = step.neighbor.map(|neighbor| neighbor.read().name);
    let failed_reads = verbatim_uia::calls::take_failed_reads();

    assert!(matches!(path, Path::Remote), "{path:?}");
    assert_eq!(neighbor, Some(Some("Volume".to_owned())));
    assert_eq!(step.window, Some(hwnd));
    assert_eq!(failed_reads, 0);
    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "uia_client_reads_the_scripted_tree",
            uia_client_reads_the_scripted_tree,
        ),
        (
            "event_shapes_read_what_the_full_read_gives",
            event_shapes_read_what_the_full_read_gives,
        ),
        (
            "a_node_kept_from_a_text_event_navigates_remotely_with_no_failed_read",
            a_node_kept_from_a_text_event_navigates_remotely_with_no_failed_read,
        ),
    ]);
}
