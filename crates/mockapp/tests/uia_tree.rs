//! Cross-process UIA tree construction (architecture section 13, layer 2).
//!
//! Spawns `mockapp --backend uia` over `tests/fixtures/tree.json`, then
//! walks the tree through `verbatim-uia`'s real client (`Uia::element_from_handle`
//! plus the base cache request) exactly as an outpost would, and asserts the
//! normalized roles, names, values, and states match the fixture. This is
//! the "provider-level fake" promised by architecture section 13: COM
//! marshaling, cache requests, and role/state mapping are all exercised for
//! real, with no actual application.

mod common;

use std::collections::HashMap;

use verbatim_model::{NodeSnapshot, Role, State};
use verbatim_uia::{NodeIdRegistry, Uia, map};
use windows::Win32::UI::Accessibility::{IUIAutomationElement, TreeScope_Children};

/// One node's expected shape, keyed by name (the fixture id is not visible
/// to a UIA client, only name/role/value/states are).
struct Expected {
    role: Role,
    value: Option<&'static str>,
    states: &'static [State],
    absent_states: &'static [State],
    child_count: usize,
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
                child_count: 5,
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

/// Walks one element and its descendants, comparing against `expected`.
///
/// `tolerate_unmatched` is set by the caller for the *window's direct
/// children* only: a real UIA raw view of an actual `hwnd` can include
/// host-provided native elements (for example window-chrome furniture
/// merged in via `HostRawElementProvider`) alongside the fixture's own
/// children, so the root's child enumeration may legitimately be a superset
/// of the fixture. Every other node — the root itself, and everything below
/// the window's direct children — is pure fixture data with no such
/// merging, so an unmatched name there is a hard failure. The final
/// `visited == expected.len()` count in the caller still catches a fixture
/// child that silently failed to appear anywhere.
fn walk(
    uia: &Uia,
    element: &IUIAutomationElement,
    registry: &NodeIdRegistry,
    expected: &HashMap<&str, Expected>,
    visited: &mut usize,
    tolerate_unmatched: bool,
) {
    // SAFETY: `element` was built with the base cache request (either by
    // `element_from_handle` for the root or by `FindAllBuildCache` below for
    // every descendant), so every cached read here is satisfied.
    let snapshot: NodeSnapshot = unsafe { map::snapshot_from_cached_element(element, registry) };
    let name = snapshot.name.clone().unwrap_or_default();
    let Some(expectation) = expected.get(name.as_str()) else {
        assert!(
            tolerate_unmatched,
            "unexpected node in the UIA tree: {snapshot:?} (only the window's direct \
             children may carry unrecognized, host-provided native elements)"
        );
        return;
    };
    *visited += 1;

    assert_eq!(
        snapshot.role, expectation.role,
        "role mismatch for {name:?}"
    );
    assert_eq!(
        snapshot.value.as_deref(),
        expectation.value,
        "value mismatch for {name:?}"
    );
    for &state in expectation.states {
        assert!(
            snapshot.states.contains(state),
            "{name:?} is missing expected state {state:?}"
        );
    }
    for &state in expectation.absent_states {
        assert!(
            !snapshot.states.contains(state),
            "{name:?} unexpectedly carries state {state:?}"
        );
    }

    let cache = uia.base_cache_request().expect("base cache request");
    // SAFETY: `element` is a live, cached element on this client's own
    // apartment thread.
    let children = unsafe {
        element
            .FindAllBuildCache(
                TreeScope_Children,
                &uia.client().CreateTrueCondition().unwrap(),
                &cache,
            )
            .expect("FindAllBuildCache")
    };
    // SAFETY: `children` is the array just returned above.
    let count = unsafe { children.Length() }.unwrap_or(0);
    // Only the window itself can have host-merged extra children.
    let is_window = snapshot.role == Role::Window;
    if is_window {
        assert!(
            usize::try_from(count).unwrap_or(0) >= expectation.child_count,
            "root reported fewer children ({count}) than the fixture has ({})",
            expectation.child_count
        );
    } else {
        assert_eq!(
            usize::try_from(count).unwrap_or(0),
            expectation.child_count,
            "child count mismatch for {name:?}"
        );
    }
    for i in 0..count {
        // SAFETY: `i` is within `[0, count)`.
        let child = unsafe { children.GetElement(i) }.expect("GetElement");
        walk(uia, &child, registry, expected, visited, is_window);
    }
}

#[test]
fn uia_client_reads_the_scripted_tree() {
    let title = common::unique_title("mockapp-uia-tree");
    let mut app = common::spawn("tree.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let uia = Uia::new().expect("Uia::new");
    let cache = uia.base_cache_request().expect("base cache request");
    let root = uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("element_from_handle");

    let registry = NodeIdRegistry::new(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1)));
    let expected = expected_tree();
    let mut visited = 0;
    walk(&uia, &root, &registry, &expected, &mut visited, true);

    assert_eq!(
        visited,
        expected.len(),
        "walked {visited} nodes but expected {}",
        expected.len()
    );

    app.send("quit");
}
