//! Cross-process MSAA tree construction (architecture section 13, layer 2).
//!
//! Spawns `mockapp --backend msaa` over `tests/fixtures/tree.json`, acquires
//! the root `IAccessible` the same way `verbatim-ia2`'s `AccessibleObjectFromWindow`-based
//! acquisition does, walks it with real `IAccessible` calls, and maps every
//! node through `verbatim_ia2::map` — the same tables the real client stack
//! uses — asserting the normalized roles, names, values, and states match
//! the fixture.

mod common;

use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::ManuallyDrop;

use verbatim_ia2::map::{role_from_msaa, states_from_msaa};
use verbatim_model::{Role, State};
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;
use windows::core::Interface;

struct Expected {
    role: Role,
    value: Option<&'static str>,
    states: &'static [State],
    absent_states: &'static [State],
    child_count: usize,
}

/// The nodes whose fixture carries the detail properties plain MSAA can
/// express — `accDescription` and `accKeyboardShortcut` — keyed by name
/// like [`expected_tree`]. Every node absent from this map must read back
/// neither (an `S_FALSE` failure, the MSAA convention for "not supported",
/// which `bstr_to_option`-style handling maps to `None`). The fixture's
/// `position_in_set`, `set_size`, and `level` never appear on this backend
/// at all: plain MSAA has no accessor for them (IA2's `groupPosition` is
/// the M6 source), so there is nothing to assert about them here.
fn expected_details() -> HashMap<&'static str, (Option<&'static str>, Option<&'static str>)> {
    HashMap::from([(
        "OK",
        (
            Some("Applies the changes and closes the dialog"),
            Some("Alt+O"),
        ),
    )])
}

#[allow(clippy::too_many_lines)]
fn expected_tree() -> HashMap<&'static str, Expected> {
    HashMap::from([
        (
            "Mockapp Tree Fixture",
            Expected {
                role: Role::Window,
                value: None,
                states: &[],
                absent_states: &[],
                child_count: 11,
            },
        ),
        (
            "Group One",
            Expected {
                role: Role::Group,
                value: None,
                states: &[],
                absent_states: &[],
                child_count: 7,
            },
        ),
        (
            "OK",
            Expected {
                role: Role::Button,
                value: None,
                states: &[State::Focusable],
                absent_states: &[State::Disabled],
                child_count: 0,
            },
        ),
        (
            "Cancel",
            Expected {
                role: Role::Button,
                value: None,
                states: &[State::Focusable, State::Disabled],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Enable feature",
            Expected {
                role: Role::CheckBox,
                value: None,
                states: &[State::Focusable, State::Checked],
                absent_states: &[State::Mixed],
                child_count: 0,
            },
        ),
        (
            "Partial",
            Expected {
                role: Role::CheckBox,
                value: None,
                states: &[State::Focusable, State::Mixed],
                absent_states: &[State::Checked],
                child_count: 0,
            },
        ),
        (
            // MSAA has no toggle-button role (`verbatim-ia2` maps its
            // control type to `Unknown`), but the pressed state still
            // round-trips as `State::Pressed` — the two fixture toggle
            // buttons exist for the UIA test's toggle-button mapping, and
            // are asserted here only so the shared-fixture MSAA walk stays
            // exhaustive.
            "Wireless",
            Expected {
                role: Role::Unknown,
                value: None,
                states: &[State::Focusable, State::Pressed],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Airplane mode",
            Expected {
                role: Role::Unknown,
                value: None,
                states: &[State::Focusable],
                absent_states: &[State::Pressed],
                child_count: 0,
            },
        ),
        (
            "Option A",
            Expected {
                role: Role::RadioButton,
                value: None,
                states: &[State::Focusable, State::Checked],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Items",
            Expected {
                role: Role::List,
                value: None,
                states: &[],
                absent_states: &[],
                child_count: 2,
            },
        ),
        (
            "First",
            Expected {
                role: Role::ListItem,
                value: None,
                states: &[State::Focusable],
                absent_states: &[State::Offscreen],
                child_count: 0,
            },
        ),
        (
            "Second",
            Expected {
                role: Role::ListItem,
                value: None,
                states: &[State::Focusable, State::Offscreen],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Choices",
            Expected {
                role: Role::ComboBox,
                value: None,
                states: &[State::Focusable, State::Expanded],
                absent_states: &[State::Collapsed],
                child_count: 1,
            },
        ),
        (
            "Alpha",
            Expected {
                role: Role::MenuItem,
                value: None,
                states: &[],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Notes",
            Expected {
                role: Role::EditableText,
                value: Some("Hello world"),
                states: &[State::Focusable],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Volume",
            Expected {
                role: Role::Slider,
                value: Some("50"),
                states: &[State::Focusable],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Count",
            Expected {
                role: Role::SpinButton,
                value: Some("3"),
                states: &[State::Focusable],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Sections",
            Expected {
                role: Role::TabControl,
                value: None,
                states: &[State::Collapsed],
                absent_states: &[State::Expanded],
                child_count: 2,
            },
        ),
        (
            "General",
            Expected {
                role: Role::Tab,
                value: None,
                states: &[State::Focusable],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Advanced",
            Expected {
                role: Role::Tab,
                value: None,
                states: &[State::Focusable],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Learn more",
            Expected {
                role: Role::Link,
                value: None,
                states: &[State::Focusable],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Main Toolbar",
            Expected {
                role: Role::ToolBar,
                value: None,
                states: &[],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Ready",
            Expected {
                role: Role::StatusBar,
                value: None,
                states: &[],
                absent_states: &[],
                child_count: 0,
            },
        ),
        (
            "Popup",
            Expected {
                role: Role::Menu,
                value: None,
                states: &[],
                absent_states: &[],
                child_count: 0,
            },
        ),
    ])
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

fn walk(
    acc: &IAccessible,
    expected: &HashMap<&str, Expected>,
    details: &HashMap<&str, (Option<&'static str>, Option<&'static str>)>,
    visited: &mut usize,
) {
    let self_var = self_variant();
    // SAFETY: `acc` is a live IAccessible; `self_var` is CHILDID_SELF.
    let name = unsafe { acc.get_accName(&self_var) }
        .ok()
        .map(|b| b.to_string())
        .unwrap_or_default();
    // SAFETY: as above.
    let value = unsafe { acc.get_accValue(&self_var) }
        .ok()
        .map(|b| b.to_string());
    // SAFETY: as above.
    let role = unsafe { acc.get_accRole(&self_var) }
        .ok()
        // SAFETY: `v` is the VARIANT the read returned, owned here.
        .and_then(|v| unsafe { windows::Win32::System::Variant::VariantToInt32(&raw const v) }.ok())
        .map_or(Role::Unknown, |r| role_from_msaa(r.cast_unsigned()));
    // SAFETY: as above.
    let states = unsafe { acc.get_accState(&self_var) }
        .ok()
        // SAFETY: `v` is the VARIANT the read returned, owned here.
        .and_then(|v| unsafe { windows::Win32::System::Variant::VariantToInt32(&raw const v) }.ok())
        .map(|s| states_from_msaa(s.cast_unsigned()))
        .unwrap_or_default();

    let expectation = expected
        .get(name.as_str())
        .unwrap_or_else(|| panic!("unexpected node in the MSAA tree: {name:?}"));
    *visited += 1;

    assert_eq!(role, expectation.role, "role mismatch for {name:?}");
    // S_FALSE, a provider's "no value", is a success code, so the call
    // returns an empty string rather than an error; it reads as absent here,
    // as it does in verbatim-ia2.
    let value = value.filter(|v| !v.is_empty());
    assert_eq!(
        value.as_deref(),
        expectation.value,
        "value mismatch for {name:?}"
    );
    for &state in expectation.states {
        assert!(
            states.contains(state),
            "{name:?} is missing expected state {state:?}"
        );
    }
    for &state in expectation.absent_states {
        assert!(
            !states.contains(state),
            "{name:?} unexpectedly carries state {state:?}"
        );
    }

    // Details: the same accDescription and accKeyboardShortcut reads
    // verbatim-ia2's acquisition makes; nodes outside the details map must
    // read back neither (an S_FALSE failure, mapped to None here).
    // SAFETY: `acc` is live; `self_var` is CHILDID_SELF.
    let description = unsafe { acc.get_accDescription(&self_var) }
        .ok()
        .map(|b| b.to_string())
        .filter(|s| !s.is_empty());
    // SAFETY: as above.
    let shortcut = unsafe { acc.get_accKeyboardShortcut(&self_var) }
        .ok()
        .map(|b| b.to_string())
        .filter(|s| !s.is_empty());
    let (expected_description, expected_shortcut) =
        details.get(name.as_str()).copied().unwrap_or((None, None));
    assert_eq!(
        description.as_deref(),
        expected_description,
        "description mismatch for {name:?}"
    );
    assert_eq!(
        shortcut.as_deref(),
        expected_shortcut,
        "keyboard shortcut mismatch for {name:?}"
    );

    // SAFETY: `acc` is live.
    let count = unsafe { acc.accChildCount() }.unwrap_or(0);
    assert_eq!(
        usize::try_from(count).unwrap_or(0),
        expectation.child_count,
        "child count mismatch for {name:?}"
    );
    for i in 1..=count {
        // SAFETY: `acc` is live; `i` is a 1-based child position within
        // `accChildCount`'s range.
        let dispatch = unsafe { acc.get_accChild(&child_variant(i)) }.expect("get_accChild");
        let child: IAccessible = dispatch.cast().expect("child is an IAccessible");
        walk(&child, expected, details, visited);
    }
}

#[test]
fn msaa_client_reads_the_scripted_tree() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-tree");
    let mut app = common::spawn("tree.json", "msaa", &title);
    let hwnd = common::find_window(&title);

    let root = root_accessible(hwnd);
    let expected = expected_tree();
    let details = expected_details();
    let mut visited = 0;
    walk(&root, &expected, &details, &mut visited);

    assert_eq!(
        visited,
        expected.len(),
        "walked {visited} nodes but expected {}",
        expected.len()
    );

    app.send("quit");
}

/// The node named `name` in a dumped tree.
fn find(node: &verbatim_model::TreeNode, name: &str) -> Option<verbatim_model::NodeId> {
    if node.snapshot.name.as_deref() == Some(name) {
        return Some(node.snapshot.id);
    }
    node.children.iter().find_map(|child| find(child, name))
}

/// Activation through `verbatim-ia2` answers the default action's name, which
/// the reducer speaks as NVDA does ("Press"), and fails for a node with no
/// default action, so the outpost tries its parents.
#[test]
fn msaa_activation_answers_the_default_actions_name() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-activate");
    let mut app = common::spawn("tree.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let registry = verbatim_ia2::NodeIdRegistry::new(std::sync::Arc::new(
        std::sync::atomic::AtomicU64::new(1),
    ));
    let (root, _) = verbatim_ia2::acquire::walk_tree(hwnd.0 as isize, &registry, 64, 4096)
        .expect("walk the tree");
    let ok = find(&root, "OK").expect("the OK button is in the tree");
    assert_eq!(
        verbatim_ia2::acquire::activate(ok, &registry).expect("OK activates"),
        Some(verbatim_model::ActionName::Named("Press".to_owned()))
    );
    let cancel = find(&root, "Cancel").expect("the Cancel button is in the tree");
    assert!(verbatim_ia2::acquire::activate(cancel, &registry).is_err());
    app.send("quit");
}

/// The selected child of a list, read through `verbatim-ia2`'s own
/// `selected_child` from the list object kept when the tree was walked, after
/// a scripted selection (audit item 21: `accSelection` used to be stubbed).
#[test]
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

    app.send("select item2");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let selected = loop {
        let selected =
            verbatim_ia2::acquire::selected_child(list, &registry).and_then(|node| node.name);
        if selected.is_some() || std::time::Instant::now() > deadline {
            break selected;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(selected.as_deref(), Some("Second"));

    app.send("quit");
}

/// A live object seen again at its address is the node already issued:
/// mockapp answers every request with a new COM object, so each sighting
/// after the first is matched by address, comparing a fresh read of the kept
/// object's role and identity with the new one's (`docs/parity.md`, "Held
/// objects"). A renamed object stays the same node; another object is a
/// different one.
#[test]
fn msaa_sightings_of_one_live_object_are_one_node() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-identity");
    let mut app = common::spawn("tree.json", "msaa", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let registry = verbatim_ia2::NodeIdRegistry::new(std::sync::Arc::new(
        std::sync::atomic::AtomicU64::new(1),
    ));
    let sight = |id_object: i32| {
        verbatim_ia2::acquire::snapshot_from_event(hwnd, id_object, 0, &registry)
            .expect("the object is acquired")
    };

    let first = sight(OBJID_CLIENT.0);
    assert_eq!(sight(OBJID_CLIENT.0).id, first.id, "seen again, same node");

    app.send("set-name root Renamed");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let renamed = loop {
        let seen = sight(OBJID_CLIENT.0);
        if seen.name.as_deref() == Some("Renamed") || std::time::Instant::now() > deadline {
            break seen;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(renamed.name.as_deref(), Some("Renamed"));
    assert_eq!(renamed.id, first.id, "a renamed object is the same node");

    // mockapp addresses node `index` as object id `index + 1`.
    assert_ne!(sight(2).id, first.id, "another object is another node");

    app.send("quit");
}
