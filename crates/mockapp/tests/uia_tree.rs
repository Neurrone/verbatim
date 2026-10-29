//! Cross-process UIA tree construction (architecture section 13, layer 2).
//!
//! Spawns `mockapp --backend uia` over `tests/fixtures/tree.json`, then
//! walks the tree through `verbatim-uia`'s real client (`Uia::window_element`
//! and `Uia::children`, every node of the fixture read as the `Node` shape
//! with every detail), and asserts the
//! whole tree read back, every node's role, name, value, whole state set,
//! and details, and every node's children in order, equals the fixture's. This is
//! the "provider-level fake" promised by architecture section 13: COM
//! marshaling, cache requests, and role/state mapping are all exercised for
//! real, with no actual application. The tree dump
//! (`verbatim_uia_rops::tree_dump`) reads each element's children in one
//! call, by either path: an element gone during it keeps its later
//! siblings, and no element's location or off-screen state is read.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::{NodeDetails, Role, State, StateSet};
use verbatim_uia::{
    Cached, DialogText, Fill, Live, NodeIdRegistry, PropertyEvent, Request, RoleCore, Shape,
    TextEvent, Uia, map,
};
use verbatim_uia_rops::{
    NavigationDirection, Path, StepQuery, TreeQuery, navigation_step, tree_dump,
};
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
fn expected_tree(title: &str, window: verbatim_model::WindowHandle) -> Node {
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
    let mut root = parent(
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
    );
    // The window's element is its window's own.
    root.details.window = Some(window);
    root
}

/// Reads `element`, filled as a node with every detail, and everything
/// below it: its children are found by one read of their roles, and each
/// is then read as a node, but for the frame's title bar, read alone and
/// as a dialog's child is, every property but its location and whether it
/// is off the screen (see [`expected_tree`]). Those two have UIA ask the
/// window's thread (`WM_GETTITLEBARINFOEX`), whose answer waits on the
/// Desktop Window Manager, seconds behind while the other tests make and
/// destroy windows (`docs/tooling/troubleshooting.md`), so a read of them
/// would outlast UIA's timeout under the full run.
fn walk(
    uia: &Uia,
    element: &Cached<verbatim_uia::Node>,
    requests: (&Request<verbatim_uia::Node>, &Request<RoleCore>),
    registry: &NodeIdRegistry,
) -> Node {
    let (nodes, roles) = requests;
    let snapshot = element.snapshot(registry);
    let mut details = snapshot.details;
    details.rect = None;
    let children = uia
        .children(element.live(), roles)
        .expect("the element's children")
        .iter()
        .map(|child| {
            if child.read().role() == Role::TitleBar {
                title_bar(uia, child.live())
            } else {
                let child = child
                    .live()
                    .refresh(nodes)
                    .expect("a child, read as a node");
                walk(uia, &child, requests, registry)
            }
        })
        .collect();
    Node {
        name: snapshot.name,
        role: snapshot.role,
        value: snapshot.value,
        states: snapshot.states,
        details,
        children,
    }
}

/// The frame's title bar, read as a dialog's child is ([`DialogText`]),
/// without its children, which are Windows' and named in the machine's
/// language.
fn title_bar(uia: &Uia, element: &Live) -> Node {
    let request = uia
        .request_fixed::<DialogText>()
        .expect("a dialog child's request");
    let read = element.refresh(&request).expect("the title bar");
    let parts = read.read().parts(read.runtime_id());
    Node {
        name: parts.name,
        role: parts.role,
        value: parts.value,
        states: parts.states,
        details: parts.details,
        children: Vec::new(),
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
    let roles = uia.request_fixed::<RoleCore>().expect("the role's request");
    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
    assert_eq!(
        walk(&uia, &root, (&nodes, &roles), &registry),
        expected_tree(
            &title,
            verbatim_model::WindowHandle(u64::from_ne_bytes((hwnd.0 as isize).to_ne_bytes()))
        )
    );

    app.quit();
}

/// Every element of the scripted tree, filled with each event shape's own
/// request, reads as the shape the same role, name, value, states, and
/// window handle that the full read of the element gives, as a
/// `verbatim_uia::Node` with every detail, for the fields the shape
/// carries: a property event's parts, which have no details and are never
/// off the screen, since no event registration's cache holds where its
/// element is (`verbatim_uia::Place`), and its window handle; a text
/// event's window handle. The frame's title bar and its
/// buttons, Windows' and not the fixture's, are left out: reading their
/// location or whether they are off the screen has UIA ask the window's
/// thread (`WM_GETTITLEBARINFOEX`), whose answer waits on the Desktop
/// Window Manager, seconds behind while the other tests make and destroy
/// windows (`docs/tooling/troubleshooting.md`).
fn event_shapes_read_what_the_full_read_gives() {
    let title = common::unique_title("mockapp-uia-event-shapes");
    let app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let uia = Uia::new().expect("Uia::new");
    let nodes = uia
        .request::<verbatim_uia::Node>(verbatim_uia::Node::all_on())
        .expect("the node's request");
    let roles = uia.request_fixed::<RoleCore>().expect("the role's request");
    let root = uia
        .window_live(hwnd.0 as isize)
        .expect("mockapp's window element");
    // SAFETY: a live client; the condition takes no arguments.
    let condition = unsafe { uia.client().CreateTrueCondition() }.expect("CreateTrueCondition");
    let mut elements = vec![root.element().clone()];
    for child in uia
        .children(&root, &roles)
        .expect("the window's children")
        .iter()
        .filter(|child| child.read().role() != Role::TitleBar)
    {
        // SAFETY: `child` is a live element on this client's own apartment
        // thread; the search reads no cache.
        let found = unsafe {
            child
                .live()
                .element()
                .FindAll(TreeScope_Subtree, &condition)
        }
        .expect("FindAll");
        elements.extend(verbatim_uia::elements_of(&found));
    }
    assert_eq!(elements.len(), 24, "every element of the scripted tree");
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
        let mut states = full.states;
        states.remove(State::Offscreen);
        let expected = map::CachedUiaParts {
            details: verbatim_model::NodeDetails {
                window: full.details.window,
                ..verbatim_model::NodeDetails::default()
            },
            states,
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

/// A dumped tree's names, each node's with its children's in order.
#[derive(Debug, PartialEq)]
struct Named {
    name: Option<String>,
    children: Vec<Named>,
}

/// [`Named`] for a dumped `node`.
fn named(node: &verbatim_model::TreeNode) -> Named {
    Named {
        name: node.snapshot.name.clone(),
        children: node.children.iter().map(named).collect(),
    }
}

/// A leaf of a [`Named`] tree.
fn item(name: &str) -> Named {
    Named {
        name: Some(name.to_owned()),
        children: Vec::new(),
    }
}

/// A node read among many, as the outposts' tree dump reads one when
/// locations are not asked for: every detail but its place and its cell
/// (`verbatim_uia::Place`, `verbatim_uia::Cell`).
fn many_nodes(uia: &Uia) -> Request<verbatim_uia::Node> {
    uia.request::<verbatim_uia::Node>(verbatim_uia::NodeGates {
        place: None,
        cell: None,
        ..verbatim_uia::Node::all_on()
    })
    .expect("the node's request")
}

/// Every node of `node`'s subtree that has a rectangle or is off the
/// screen.
fn placed(node: &verbatim_model::TreeNode) -> usize {
    usize::from(
        node.snapshot.details.rect.is_some() || node.snapshot.states.contains(State::Offscreen),
    ) + node.children.iter().map(placed).sum::<usize>()
}

/// A tree dump of mockapp's list (`small.json`) whose first item dies where
/// it stands when the dump asks it for its first child, as an element of a
/// menu that closes during a dump does (`disconnect-at-first-child`), after
/// the one read of the list's children has listed it and its later
/// sibling, `remote` naming the path. The walk this replaced stepped to the
/// next sibling from the item that had gone, failed, and lost the sibling.
fn dump_with_an_element_gone_during_it(remote: bool) -> (Named, bool, &'static str) {
    let title = common::unique_title("mockapp-uia-gone-tree");
    let mut app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let nodes = many_nodes(&uia);
    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
    // The list, a child of the window, found by its name
    // (`common::named_element`), which passes the window's title bar by;
    // the dump starts from the list, since the title bar is Windows' and
    // named in the machine's language.
    let list = Live::new(common::named_element(&uia, hwnd, "Options"))
        .refresh(&nodes)
        .expect("the list, read as a node");
    app.send("disconnect-at-first-child item1");
    let fill = Fill::new(&nodes);
    let query = TreeQuery {
        root: &list,
        fill: &fill,
        registry: &registry,
        max_depth: 8,
        max_nodes: 64,
    };
    let ((dumped, truncated), path) = tree_dump(&uia, &query, remote).expect("the dump");
    app.quit();
    (named(&dumped), truncated, path.name())
}

/// [`dump_with_an_element_gone_during_it`] by remote programs: the read of
/// the gone item's children fails inside the run, which marks the dump
/// incomplete and leaves the item out, and the sibling is kept.
fn a_remote_dump_keeps_the_sibling_after_an_element_gone_during_it() {
    assert_eq!(
        dump_with_an_element_gone_during_it(true),
        (
            Named {
                name: Some("Options".to_owned()),
                children: vec![item("Second")],
            },
            true,
            "remote",
        )
    );
}

/// [`dump_with_an_element_gone_during_it`] classically: UIA answers the
/// read of the gone item's children as none, so the item is listed as it
/// was read with the list, with no children, and the sibling is kept.
fn a_classic_dump_keeps_the_sibling_after_an_element_gone_during_it() {
    assert_eq!(
        dump_with_an_element_gone_during_it(false),
        (
            Named {
                name: Some("Options".to_owned()),
                children: vec![item("First"), item("Second")],
            },
            false,
            "classic",
        )
    );
}

/// The dump of mockapp's whole window (`small.json`) as the outposts make
/// it, by remote programs, with locations not asked for: no element's
/// location or off-screen state is read, so the title bar UIA adds for the
/// window's frame and its buttons cost mockapp's window thread no
/// `WM_GETTITLEBARINFOEX`, of which a dump reading them made 8, and no node,
/// the title bar first among them, has a rectangle or is off the screen.
fn a_dump_reads_nothing_the_title_bar_asks_the_window_for() {
    let title = common::unique_title("mockapp-uia-dump-title-bar");
    let app = common::spawn("small.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let nodes = many_nodes(&uia);
    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
    let root = uia
        .window_element(hwnd.0 as isize, &nodes)
        .expect("mockapp's window element");
    let fill = Fill::new(&nodes);
    let query = TreeQuery {
        root: &root,
        fill: &fill,
        registry: &registry,
        max_depth: 64,
        max_nodes: 4096,
    };
    common::reset_hits(hwnd);
    let ((dumped, truncated), path) = tree_dump(&uia, &query, true).expect("the dump");
    let title_bar_requests = common::read_hits(hwnd)
        .into_iter()
        .find(|(method, _)| *method == "WM_GETTITLEBARINFOEX");
    assert_eq!(
        (
            title_bar_requests,
            dumped.children[0].snapshot.role,
            placed(&dumped),
            truncated,
            path.name(),
        ),
        (None, Role::TitleBar, 0, false, "remote")
    );
    app.quit();
}

/// The classic dump of mockapp's list (`tree.json`'s "Items") reads each
/// listed element's children in one call: the list's, and each of its two
/// items', 3 UIA calls.
fn a_classic_dump_reads_each_elements_children_in_one_call() {
    let title = common::unique_title("mockapp-uia-dump-calls");
    let app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let nodes = many_nodes(&uia);
    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
    let list = Live::new(common::named_element(&uia, hwnd, "Items"))
        .refresh(&nodes)
        .expect("the list, read as a node");
    let fill = Fill::new(&nodes);
    let query = TreeQuery {
        root: &list,
        fill: &fill,
        registry: &registry,
        max_depth: 8,
        max_nodes: 64,
    };
    let _ = verbatim_uia::calls::take();
    let ((dumped, truncated), path) = tree_dump(&uia, &query, false).expect("the dump");
    let calls = verbatim_uia::calls::take().uia;
    assert_eq!(
        (named(&dumped), truncated, path.name(), calls),
        (
            Named {
                name: Some("Items".to_owned()),
                children: vec![item("First"), item("Second")],
            },
            false,
            "classic",
            3,
        )
    );
    app.quit();
}

/// The remote dump of the same list reads all three elements' children in
/// one round trip.
fn a_remote_dump_reads_a_small_tree_in_one_call() {
    let title = common::unique_title("mockapp-uia-dump-remote-calls");
    let app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let nodes = many_nodes(&uia);
    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
    let list = Live::new(common::named_element(&uia, hwnd, "Items"))
        .refresh(&nodes)
        .expect("the list, read as a node");
    let fill = Fill::new(&nodes);
    let query = TreeQuery {
        root: &list,
        fill: &fill,
        registry: &registry,
        max_depth: 8,
        max_nodes: 64,
    };
    let _ = verbatim_uia::calls::take();
    let ((dumped, truncated), path) = tree_dump(&uia, &query, true).expect("the dump");
    let calls = verbatim_uia::calls::take().uia;
    assert_eq!(
        (named(&dumped), truncated, path.name(), calls),
        (
            Named {
                name: Some("Items".to_owned()),
                children: vec![item("First"), item("Second")],
            },
            false,
            "remote",
            1,
        )
    );
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
        (
            "a_remote_dump_keeps_the_sibling_after_an_element_gone_during_it",
            a_remote_dump_keeps_the_sibling_after_an_element_gone_during_it,
        ),
        (
            "a_classic_dump_keeps_the_sibling_after_an_element_gone_during_it",
            a_classic_dump_keeps_the_sibling_after_an_element_gone_during_it,
        ),
        (
            "a_dump_reads_nothing_the_title_bar_asks_the_window_for",
            a_dump_reads_nothing_the_title_bar_asks_the_window_for,
        ),
        (
            "a_classic_dump_reads_each_elements_children_in_one_call",
            a_classic_dump_reads_each_elements_children_in_one_call,
        ),
        (
            "a_remote_dump_reads_a_small_tree_in_one_call",
            a_remote_dump_reads_a_small_tree_in_one_call,
        ),
    ]);
}
