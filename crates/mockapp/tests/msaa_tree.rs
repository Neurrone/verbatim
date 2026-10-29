//! Cross-process MSAA tree construction (architecture section 13, layer 2).
//!
//! Spawns `mockapp --backend msaa` over `tests/fixtures/tree.json`, acquires
//! the root `IAccessible` the same way `verbatim-ia2`'s `AccessibleObjectFromWindow`-based
//! acquisition does, walks it with real `IAccessible` calls, and maps every
//! node through `verbatim_ia2::map` — the same tables the real client stack
//! uses — asserting the whole tree read back, every node's role, name,
//! value, whole state set, description, and keyboard shortcut, and every
//! node's children in order, equals the fixture's. A tree dump
//! (`verbatim_ia2::acquire::walk_tree`) that meets an object gone during it
//! ends that object's children there and marks the dump incomplete.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::ffi::c_void;
use std::mem::ManuallyDrop;

use verbatim_ia2::map::{role_from_msaa, states_from_msaa};
use verbatim_model::{Role, State, StateSet, WindowHandle};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, OBJID_WINDOW};
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
    let (root, _) = verbatim_ia2::acquire::walk_tree(hwnd.0 as isize, &registry, (64, 4096), false)
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
    let (root, _) = verbatim_ia2::acquire::walk_tree(hwnd.0 as isize, &registry, (64, 4096), false)
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
/// object's role and identity with the new one's (`docs/parity/focus-events.md`, "Held
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

/// A window's own window and client objects, read at their addresses,
/// carry their window, and an object inside the window carries none
/// (`NodeDetails::window`): mockapp's window (`tree.json`) read at
/// `OBJID_WINDOW` and `OBJID_CLIENT`, and its second node at its own
/// object id and as the client area's first child.
fn msaa_window_and_client_objects_carry_their_window() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-window-handle");
    let app = common::spawn("tree.json", "msaa", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let registry = verbatim_ia2::NodeIdRegistry::new(std::sync::Arc::new(
        std::sync::atomic::AtomicU64::new(1),
    ));
    let window_of = |id_object: i32, id_child: i32| {
        verbatim_ia2::acquire::snapshot_from_event(
            hwnd,
            id_object,
            id_child,
            &registry,
            verbatim_ia2::acquire::Purpose::Announce,
        )
        .expect("the object is acquired")
        .details
        .window
    };
    let own = Some(WindowHandle(u64::from_ne_bytes(hwnd.to_ne_bytes())));
    // mockapp addresses node `index` as object id `index + 1`, and the
    // client area's children by child id too.
    assert_eq!(
        [
            window_of(OBJID_WINDOW.0, 0),
            window_of(OBJID_CLIENT.0, 0),
            window_of(2, 0),
            window_of(OBJID_CLIENT.0, 1)
        ],
        [own, own, None, None]
    );
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

/// A tree dump (`walk_tree`) of mockapp's window, whose list's first item
/// is gone where it stands (`disconnect`), so that every call on it, its
/// `accChildCount` among them, fails with `RPC_E_DISCONNECTED`: the item
/// keeps its place with no name and no children, the dump goes on to the
/// list's second item, and the dump is marked incomplete, rather than the
/// item being reported as having no children in a complete dump.
fn msaa_tree_dump_ends_an_objects_children_where_it_is_gone() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-gone-tree");
    let mut app = common::spawn("small.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let registry = verbatim_ia2::NodeIdRegistry::new(std::sync::Arc::new(
        std::sync::atomic::AtomicU64::new(1),
    ));
    let node = |name: Option<&str>, children: Vec<Named>| Named {
        name: name.map(str::to_owned),
        children,
    };
    let tree = |first: Option<&str>| {
        node(
            Some("Mockapp Events Fixture"),
            vec![
                node(Some("Original Name"), Vec::new()),
                node(Some("Level"), Vec::new()),
                node(
                    Some("Options"),
                    vec![node(first, Vec::new()), node(Some("Second"), Vec::new())],
                ),
            ],
        )
    };
    let (whole, truncated) =
        verbatim_ia2::acquire::walk_tree(hwnd.0 as isize, &registry, (64, 4096), false)
            .expect("the dump");
    assert_eq!(
        (named(&whole), truncated),
        (tree(Some("First")), false),
        "the tree before the list's first item goes"
    );

    app.send("disconnect item1");
    let (dumped, truncated) =
        verbatim_ia2::acquire::walk_tree(hwnd.0 as isize, &registry, (64, 4096), false)
            .expect("the dump with a gone item");
    assert_eq!(
        (named(&dumped), truncated),
        (tree(None), true),
        "the tree after the list's first item went"
    );
    app.quit();
}

/// The OK button of `tree.json`, the first child of the window's first
/// child, "Group One", its name read and checked first. A client's first
/// call on a new MSAA object's proxy that is `accParent` reaches the
/// provider twice, one call after the other, while a first call of
/// `get_accName` reaches it once and so does every `accParent` after it
/// (mockapp's hit counts, Windows 11 26200, 2026-10-29), so the name is
/// read first and the tests count the provider calls of one `accParent`.
fn ok_button(hwnd: windows::Win32::Foundation::HWND) -> IAccessible {
    let child = |parent: &IAccessible| {
        // SAFETY: `parent` is live; 1 is its first child's position.
        unsafe { parent.get_accChild(&child_variant(1)) }
            .expect("get_accChild")
            .cast::<IAccessible>()
            .expect("the child is an IAccessible")
    };
    let ok = child(&child(&root_accessible(hwnd)));
    assert_eq!(
        // SAFETY: `ok` is live; `self_variant` is CHILDID_SELF.
        text(unsafe { ok.get_accName(&self_variant()) }, "the name").as_deref(),
        Some("OK")
    );
    ok
}

/// The name of `acc`'s parent, read through its `accParent`.
fn parent_name(acc: &IAccessible) -> Option<String> {
    // SAFETY: `acc` is live.
    let parent = unsafe { acc.accParent() }
        .expect("accParent")
        .cast::<IAccessible>()
        .expect("the parent is an IAccessible");
    // SAFETY: `parent` is live; `self_variant` is CHILDID_SELF.
    text(
        unsafe { parent.get_accName(&self_variant()) },
        "the parent's name",
    )
}

/// mockapp's `slow-acc-parent`: with `slow-acc-parent btn_ok 300`, the OK
/// button's `accParent` is answered 300 ms late, the wait bracketed by
/// `slow-acc-parent started` and `slow-acc-parent ended` lines on the
/// run's clock, both inside the call, and it answers its parent, "Group
/// One". After `slow-acc-parent btn_ok 0` it is answered at once again: it
/// prints nothing, so the next line is the next command's acknowledgement.
fn msaa_slow_acc_parent_answers_the_parent_late() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-slow-parent");
    let mut app = common::spawn("tree.json", "msaa", &title);
    let ok = ok_button(common::find_window(&title));
    app.send("slow-acc-parent btn_ok 300");
    let from = common::now_us();
    assert_eq!(parent_name(&ok).as_deref(), Some("Group One"), "late");
    let returned = common::now_us();
    let started = app.stamped_line("slow-acc-parent started");
    let ended = app.stamped_line("slow-acc-parent ended");
    assert!(
        from <= started && ended <= returned,
        "the wait, {started} to {ended}, is inside the call, {from} to {returned}"
    );
    assert!(
        ended - started >= 300_000,
        "the wait, {started} to {ended}, lasts 300 ms"
    );
    app.send("slow-acc-parent btn_ok 0");
    assert_eq!(parent_name(&ok).as_deref(), Some("Group One"), "restored");
    app.send("slow-acc-parent btn_ok 0");
    app.quit();
}

/// mockapp's `fail-acc-parent`: with `fail-acc-parent btn_ok`, the OK
/// button's `accParent` fails with `E_FAIL`, the call bracketed by
/// `fail-acc-parent started` and `fail-acc-parent ended` lines on the
/// run's clock, both inside it, once, so the next line is the next
/// command's acknowledgement, while its other calls answer as usual: its
/// name is still "OK".
fn msaa_fail_acc_parent_fails_the_parent_alone() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-fail-parent");
    let mut app = common::spawn("tree.json", "msaa", &title);
    let ok = ok_button(common::find_window(&title));
    app.send("fail-acc-parent btn_ok");
    let from = common::now_us();
    // SAFETY: `ok` is live.
    let failed = unsafe { ok.accParent() };
    let returned = common::now_us();
    assert_eq!(
        failed.map(|_| ()).map_err(|error| error.code()),
        Err(windows::Win32::Foundation::E_FAIL)
    );
    let started = app.stamped_line("fail-acc-parent started");
    let ended = app.stamped_line("fail-acc-parent ended");
    assert!(
        from <= started && started <= ended && ended <= returned,
        "the call's lines, {started} and {ended}, are inside it, {from} to {returned}"
    );
    assert_eq!(
        // SAFETY: `ok` is live; `self_variant` is CHILDID_SELF.
        text(unsafe { ok.get_accName(&self_variant()) }, "the name").as_deref(),
        Some("OK")
    );
    app.send("fail-acc-parent btn_ok");
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
        (
            "msaa_window_and_client_objects_carry_their_window",
            msaa_window_and_client_objects_carry_their_window,
        ),
        (
            "msaa_tree_dump_ends_an_objects_children_where_it_is_gone",
            msaa_tree_dump_ends_an_objects_children_where_it_is_gone,
        ),
        (
            "msaa_slow_acc_parent_answers_the_parent_late",
            msaa_slow_acc_parent_answers_the_parent_late,
        ),
        (
            "msaa_fail_acc_parent_fails_the_parent_alone",
            msaa_fail_acc_parent_fails_the_parent_alone,
        ),
    ]);
}
