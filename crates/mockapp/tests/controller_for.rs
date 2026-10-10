//! Selection in a list the focus controls, cross-process: a search box whose
//! UIA `ControllerFor` relation names a results list, as the Settings app's
//! search box names its suggestions ("Selection in a list the focus
//! controls" in `docs/nvda/events.md`).
//!
//! An item selected through mockapp's `select` command raises a real
//! `ElementSelected` event; `verbatim_uia_rops::controlled_selection`, the
//! check the outpost makes on such an event, must find it from the search
//! box, one level down or inside a group of the list, and must find
//! nothing for an item outside the controlled list, for the controlled list
//! itself, or from an element that controls nothing. It answers each in one
//! remote operation, and the classic calls behind the same signature
//! answer the same. The focused element is passed in rather than read from
//! the keyboard focus, so the test runs headless.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::mpsc;

use verbatim_model::CallCounts;
use verbatim_uia::{CACHED_PROPERTIES, ElementExt, Registration, Scope, Subscription, Uia};
use verbatim_uia_rops::{ControlledQuery, Path, controlled_selection};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationCacheRequest, IUIAutomationElement, TreeScope_Descendants, UIA_NamePropertyId,
    UIA_SelectionItem_ElementSelectedEventId,
};
use windows::core::BSTR;

/// The element named `name` under `root`, built with `cache`.
fn find(
    uia: &Uia,
    root: &IUIAutomationElement,
    name: &str,
    cache: &IUIAutomationCacheRequest,
) -> IUIAutomationElement {
    let condition = uia
        .property_condition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(name)))
        .expect("name condition");
    root.find_first_build_cache(TreeScope_Descendants, &condition, cache)
        .unwrap_or_else(|error| panic!("no element named {name:?}: {error}"))
        .unwrap_or_else(|| panic!("no element named {name:?}"))
}

fn runtime_id(element: &IUIAutomationElement) -> Vec<i32> {
    verbatim_uia::map::snapshot_parts_from_cached_element(element).runtime_id
}

fn name_of(element: &IUIAutomationElement) -> Option<String> {
    verbatim_uia::map::snapshot_parts_from_cached_element(element).name
}

/// The element `selected` names when it is inside what `focused` controls,
/// asked as the outpost asks, by the remote program and by the classic
/// calls: both must answer the same, the program in one call. The answer
/// as the selected element's name and runtime id, its cache read.
fn controlled(
    uia: &Uia,
    focused: &IUIAutomationElement,
    selected: &[i32],
) -> Option<(Option<String>, Vec<i32>)> {
    let query = ControlledQuery {
        focused,
        selected,
        properties: CACHED_PROPERTIES,
    };
    let _ = verbatim_uia::calls::take();
    let (remote, path) = controlled_selection(uia, &query, true).expect("the remote program");
    assert!(matches!(path, Path::Remote), "answered by {}", path.name());
    assert_eq!(
        verbatim_uia::calls::take(),
        CallCounts {
            uia: 1,
            msaa: 0,
            window_messages: 0
        },
        "one remote operation"
    );
    let (classic, _) = controlled_selection(uia, &query, false).expect("the classic calls");
    let answer = |found: Option<IUIAutomationElement>| {
        found.map(|element| (name_of(&element), runtime_id(&element)))
    };
    let remote = answer(remote);
    assert_eq!(remote, answer(classic), "both ways answer the same");
    remote
}

fn a_selected_result_is_found_only_inside_the_list_the_search_box_controls() {
    let title = common::unique_title("mockapp-controller-for");
    let mut app = common::spawn("controller.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let (selected_tx, selected) = mpsc::channel();
    let _registration = Registration::new(
        vec![Subscription::Event {
            event: UIA_SelectionItem_ElementSelectedEventId,
            callback: Arc::new(move |element| {
                // The test may have finished.
                let _ = selected_tx.send((name_of(element), runtime_id(element)));
            }),
        }],
        Scope::Windows(vec![hwnd.0 as isize]),
    )
    .expect("Registration::new");

    let uia = Uia::new().expect("Uia::new");
    let cache = uia.base_cache_request().expect("base cache request");
    let root = uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("element_from_handle");
    let search = find(&uia, &root, "Search box", &cache);
    let other = find(&uia, &root, "Other box", &cache);
    let results = find(&uia, &root, "Suggestions", &cache);

    // The next selection event, which must be the one for `name`.
    let selected_id = |name: &str| -> Vec<i32> {
        let (selected_name, id) = selected
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|_| panic!("no element-selected event for {name}"));
        assert_eq!(selected_name.as_deref(), Some(name), "the selected element");
        id
    };

    app.send("select result2");
    let result = selected_id("Sound settings");
    assert_eq!(
        controlled(&uia, &search, &result),
        Some((Some("Sound settings".to_owned()), result.clone())),
        "the selected result is inside the list the search box controls"
    );
    app.send("select result1");
    let grouped = selected_id("Display settings");
    assert_eq!(
        controlled(&uia, &search, &grouped),
        Some((Some("Display settings".to_owned()), grouped)),
        "a result inside a group of the list is found too"
    );

    assert_eq!(
        controlled(&uia, &other, &result),
        None,
        "a box that controls nothing finds nothing"
    );
    assert_eq!(
        controlled(&uia, &search, &runtime_id(&results)),
        None,
        "the controlled list is not its own descendant"
    );

    app.send("select recent1");
    let elsewhere = selected_id("Elsewhere");
    assert_eq!(
        controlled(&uia, &search, &elsewhere),
        None,
        "an item outside the controlled list is not a controlled selection"
    );

    assert!(
        selected.try_recv().is_err(),
        "one selection event for each select"
    );
    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[(
        "a_selected_result_is_found_only_inside_the_list_the_search_box_controls",
        a_selected_result_is_found_only_inside_the_list_the_search_box_controls,
    )]);
}
