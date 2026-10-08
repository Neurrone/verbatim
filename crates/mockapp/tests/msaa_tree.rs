//! Cross-process MSAA tree construction (architecture section 13, layer 2).
//!
//! Spawns `mockapp --backend msaa` over `tests/fixtures/tree.json`, acquires
//! the root `IAccessible` the same way `verbatim-ia2`'s `AccessibleObjectFromWindow`-based
//! acquisition does, walks it with real `IAccessible` calls, and maps every
//! node through `verbatim_ia2::map` — the same tables the real client stack
//! uses — asserting the whole tree read back, every node's role, name,
//! value, whole state set, description, and keyboard shortcut, and every
//! node's children in order, equals the fixture's.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::ffi::c_void;
use std::mem::ManuallyDrop;

use verbatim_ia2::map::{role_from_msaa, states_from_msaa};
use verbatim_model::{Role, State, StateSet};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;
use windows::core::Interface;

/// One node of the tree as MSAA reads it, with its children in order.
#[derive(Debug, PartialEq)]
struct Node {
    name: String,
    role: Role,
    value: Option<String>,
    states: StateSet,
    description: Option<String>,
    shortcut: Option<String>,
    children: Vec<Node>,
}

/// A fixture node with no value or details and no children.
fn leaf(name: &str, role: Role, states: &[State]) -> Node {
    Node {
        name: name.to_owned(),
        role,
        value: None,
        states: states.iter().copied().collect(),
        description: None,
        shortcut: None,
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

/// `tree.json` as plain MSAA reports it. MSAA has no toggle-button role
/// (`verbatim-ia2` maps its control type to `Unknown`), but the pressed
/// state still round-trips as `State::Pressed`; the two toggle buttons
/// exist for the UIA test's toggle-button mapping. Only OK carries a
/// description and a shortcut, the details plain MSAA can express; the
/// fixture's position in set, set size, and level have no MSAA accessor.
fn expected_tree() -> Node {
    use State::{Checked, Collapsed, Disabled, Expanded, Focusable, Mixed, Offscreen, Pressed};
    parent(
        "Mockapp Tree Fixture",
        Role::Window,
        &[],
        vec![
            parent(
                "Group One",
                Role::Group,
                &[],
                vec![
                    Node {
                        description: Some("Applies the changes and closes the dialog".to_owned()),
                        shortcut: Some("Alt+O".to_owned()),
                        ..leaf("OK", Role::Button, &[Focusable])
                    },
                    leaf("Cancel", Role::Button, &[Focusable, Disabled]),
                    leaf("Enable feature", Role::CheckBox, &[Focusable, Checked]),
                    leaf("Partial", Role::CheckBox, &[Focusable, Mixed]),
                    leaf("Option A", Role::RadioButton, &[Focusable, Checked]),
                    leaf("Wireless", Role::Unknown, &[Focusable, Pressed]),
                    leaf("Airplane mode", Role::Unknown, &[Focusable]),
                ],
            ),
            parent(
                "Items",
                Role::List,
                &[],
                vec![
                    leaf("First", Role::ListItem, &[Focusable]),
                    leaf("Second", Role::ListItem, &[Focusable, Offscreen]),
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

fn self_variant() -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: 0 },
            }),
        },
    }
}

fn child_variant(id: i32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: id },
            }),
        },
    }
}

fn root_accessible(hwnd: windows::Win32::Foundation::HWND) -> IAccessible {
    let mut acc: Option<IAccessible> = None;
    // SAFETY: `hwnd` is a live top-level window; the out pointer receives an
    // `IAccessible` or stays `None` on failure.
    unsafe {
        AccessibleObjectFromWindow(
            hwnd,
            OBJID_CLIENT.0.cast_unsigned(),
            &IAccessible::IID,
            (&raw mut acc).cast::<*mut c_void>(),
        )
        .expect("AccessibleObjectFromWindow");
    }
    acc.expect("AccessibleObjectFromWindow returned no object")
}

/// A string property read: `S_FALSE`, a provider's "none", is a success code
/// that gives an empty string, read as absent here as it is in
/// `verbatim-ia2`; any failure fails the test.
fn text(read: windows::core::Result<windows::core::BSTR>, what: &str) -> Option<String> {
    let text = read
        .unwrap_or_else(|error| panic!("{what} could not be read: {error}"))
        .to_string();
    (!text.is_empty()).then_some(text)
}

/// An integer property read; any failure fails the test.
fn integer(read: windows::core::Result<VARIANT>, what: &str) -> i32 {
    let variant = read.unwrap_or_else(|error| panic!("{what} could not be read: {error}"));
    // SAFETY: `variant` is the VARIANT the read returned, owned here.
    unsafe { windows::Win32::System::Variant::VariantToInt32(&raw const variant) }
        .unwrap_or_else(|error| panic!("{what} is not an integer: {error}"))
}

/// Reads `acc` and everything below it, failing on any read that fails.
fn walk(acc: &IAccessible) -> Node {
    let self_var = self_variant();
    // SAFETY: `acc` is a live IAccessible; `self_var` is CHILDID_SELF.
    let name = text(unsafe { acc.get_accName(&self_var) }, "a name").unwrap_or_default();
    // SAFETY: as above.
    let role = integer(unsafe { acc.get_accRole(&self_var) }, "a role");
    // SAFETY: as above.
    let states = integer(unsafe { acc.get_accState(&self_var) }, "a state");
    // SAFETY: `acc` is live.
    let count = unsafe { acc.accChildCount() }
        .unwrap_or_else(|error| panic!("{name:?}'s child count could not be read: {error}"));
    let children = (1..=count)
        .map(|i| {
            // SAFETY: `acc` is live; `i` is a 1-based child position within
            // `accChildCount`'s range.
            let dispatch = unsafe { acc.get_accChild(&child_variant(i)) }.expect("get_accChild");
            walk(
                &dispatch
                    .cast::<IAccessible>()
                    .expect("child is an IAccessible"),
            )
        })
        .collect();
    Node {
        role: role_from_msaa(role.cast_unsigned()),
        // SAFETY: as above.
        value: text(unsafe { acc.get_accValue(&self_var) }, "a value"),
        states: states_from_msaa(states.cast_unsigned()),
        // SAFETY: as above.
        description: text(
            unsafe { acc.get_accDescription(&self_var) },
            "a description",
        ),
        // SAFETY: as above.
        shortcut: text(
            // SAFETY: as above.
            unsafe { acc.get_accKeyboardShortcut(&self_var) },
            "a shortcut",
        ),
        name,
        children,
    }
}

fn msaa_client_reads_the_scripted_tree() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-tree");
    let app = common::spawn("tree.json", "msaa", &title);
    let hwnd = common::find_window(&title);

    assert_eq!(walk(&root_accessible(hwnd)), expected_tree());

    app.quit();
}

/// The node named `name` in a dumped tree.
fn find(node: &verbatim_model::TreeNode, name: &str) -> Option<verbatim_model::NodeId> {
    if node.snapshot.name.as_deref() == Some(name) {
        return Some(node.snapshot.id);
    }
    node.children.iter().find_map(|child| find(child, name))
}

/// How many times mockapp's providers were asked to do their default
/// action since the counters were last reset.
fn default_actions(hwnd: windows::Win32::Foundation::HWND) -> u32 {
    common::read_hits(hwnd)
        .into_iter()
        .find_map(|(method, count)| (method == "accDoDefaultAction").then_some(count))
        .unwrap_or(0)
}

/// Activation through `verbatim-ia2` asks the object to do its default
/// action, once, and answers the action's name, which the reducer speaks as
/// NVDA does ("Press"); for a node with no default action the object's
/// refusal is a failure, not a node gone, so the outpost tries its parents.
/// The failure's text carries Windows' own description of the error, in
/// the machine's language, so only its kind is asserted.
fn msaa_activation_answers_the_default_actions_name() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-activate");
    let app = common::spawn("tree.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let registry = verbatim_ia2::NodeIdRegistry::new(std::sync::Arc::new(
        std::sync::atomic::AtomicU64::new(1),
    ));
    let (root, _) = verbatim_ia2::acquire::walk_tree(hwnd.0 as isize, &registry, 64, 4096)
        .expect("walk the tree");
    let ok = find(&root, "OK").expect("the OK button is in the tree");
    let cancel = find(&root, "Cancel").expect("the Cancel button is in the tree");

    common::reset_hits(hwnd);
    assert_eq!(
        verbatim_ia2::acquire::activate(ok, &registry).expect("OK activates"),
        Some(verbatim_model::ActionName::Named("Press".to_owned()))
    );
    assert_eq!(default_actions(hwnd), 1, "OK was asked to press itself");

    common::reset_hits(hwnd);
    assert!(
        matches!(
            verbatim_ia2::acquire::activate(cancel, &registry),
            Err(verbatim_ia2::acquire::AcquireError::Failed(_))
        ),
        "a node with no default action refuses it"
    );
    assert_eq!(default_actions(hwnd), 1, "Cancel was asked, and refused");
    app.quit();
}

/// The selected child of a list, read through `verbatim-ia2`'s own
/// `selected_child` from the list object kept when the tree was walked, after
/// a scripted selection (audit item 21: `accSelection` used to be stubbed).
fn msaa_client_reads_a_lists_selected_child() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-selection");
    let mut app = common::spawn("tree.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let registry = verbatim_ia2::NodeIdRegistry::new(std::sync::Arc::new(
        std::sync::atomic::AtomicU64::new(1),
    ));
    let (root, _) = verbatim_ia2::acquire::walk_tree(hwnd.0 as isize, &registry, 64, 4096)
        .expect("walk the tree");
    let list = find(&root, "Items").expect("the list is in the tree");
    assert_eq!(
        verbatim_ia2::acquire::selected_child(list, &registry),
        None,
        "nothing is selected yet"
    );

    // Acknowledged once applied, so the selection is there to read.
    app.send("select item2");
    let selected =
        verbatim_ia2::acquire::selected_child(list, &registry).and_then(|node| node.name);
    assert_eq!(selected.as_deref(), Some("Second"));

    app.quit();
}

/// A live object seen again at its address is the node already issued:
/// oleacc hands the outpost a new wrapper for every request, so each
/// sighting after the first is matched by address, comparing a fresh read of the kept
/// object's role and identity with the new one's (`docs/parity.md`, "Held
/// objects"). A renamed object stays the same node; another object is a
/// different one.
fn msaa_sightings_of_one_live_object_are_one_node() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-identity");
    let mut app = common::spawn("tree.json", "msaa", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let registry = verbatim_ia2::NodeIdRegistry::new(std::sync::Arc::new(
        std::sync::atomic::AtomicU64::new(1),
    ));
    let sight = |id_object: i32| {
        verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            0,
            &registry,
            verbatim_ia2::acquire::Purpose::Announce,
        )
        .expect("the object is acquired")
    };

    let first = sight(OBJID_CLIENT.0);
    assert_eq!(sight(OBJID_CLIENT.0).id, first.id, "seen again, same node");

    // Acknowledged once applied, so the new name is there to read.
    app.send("set-name root Renamed");
    let renamed = sight(OBJID_CLIENT.0);
    assert_eq!(renamed.name.as_deref(), Some("Renamed"));
    assert_eq!(renamed.id, first.id, "a renamed object is the same node");

    // mockapp addresses node `index` as object id `index + 1`.
    assert_ne!(sight(2).id, first.id, "another object is another node");

    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "msaa_client_reads_the_scripted_tree",
            msaa_client_reads_the_scripted_tree,
        ),
        (
            "msaa_activation_answers_the_default_actions_name",
            msaa_activation_answers_the_default_actions_name,
        ),
        (
            "msaa_client_reads_a_lists_selected_child",
            msaa_client_reads_a_lists_selected_child,
        ),
        (
            "msaa_sightings_of_one_live_object_are_one_node",
            msaa_sightings_of_one_live_object_are_one_node,
        ),
    ]);
}
