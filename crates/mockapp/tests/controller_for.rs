//! Selection in a list the focus controls, cross-process: a search box whose
//! UIA `ControllerFor` relation names a results list, as the Settings app's
//! search box names its suggestions ("Selection in a list the focus
//! controls" in `docs/nvda/events.md`).
//!
//! An item selected through mockapp's `select` command raises a real
//! `ElementSelected` event; [`Uia::controlled_descendant`], the check the
//! outpost makes on such an event, must find it from the search box, and
//! must find nothing for an item outside the controlled list, for the
//! controlled list itself, or from an element that controls nothing. The
//! focused element is passed in rather than read from the keyboard focus,
//! so the test runs headless.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::{Arc, Mutex};

use verbatim_uia::{Registration, Scope, Subscription, Uia};
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
    // SAFETY: a name condition and a cached search under a live element.
    unsafe {
        let condition = uia
            .client()
            .CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from(name)))
            .expect("name condition");
        root.FindFirstBuildCache(TreeScope_Descendants, &condition, cache)
            .unwrap_or_else(|error| panic!("no element named {name:?}: {error}"))
    }
}

fn runtime_id(element: &IUIAutomationElement) -> Vec<i32> {
    // SAFETY: `element` was built with the base cache request.
    unsafe { verbatim_uia::map::snapshot_parts_from_cached_element(element) }.runtime_id
}

fn name_of(element: &IUIAutomationElement) -> Option<String> {
    // SAFETY: as above.
    unsafe { verbatim_uia::map::snapshot_parts_from_cached_element(element) }.name
}

fn a_selected_result_is_found_only_inside_the_list_the_search_box_controls() {
    let title = common::unique_title("mockapp-controller-for");
    let mut app = common::spawn("controller.json", "uia", &title);
    let hwnd = common::find_window(&title);

    let selected = Arc::new(Mutex::new(Vec::<(Option<String>, Vec<i32>)>::new()));
    let selected_cb = selected.clone();
    let _registration = Registration::new(
        Subscription::Event {
            event: UIA_SelectionItem_ElementSelectedEventId,
            callback: Arc::new(move |element| {
                selected_cb
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((name_of(element), runtime_id(element)));
            }),
        },
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

    let selected_id = |name: &str| -> Vec<i32> {
        let mut found = None;
        common::wait_until(&format!("UIA element-selected event for {name}"), || {
            found = selected
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find(|(selected_name, _)| selected_name.as_deref() == Some(name))
                .map(|(_, id)| id.clone());
            found.is_some()
        });
        found.expect("waited for it")
    };

    app.send("select result2");
    let result = selected_id("Sound settings");
    // SAFETY: `search` is live.
    let found = unsafe { uia.controlled_descendant(&search, &result, &cache) }
        .expect("controlled_descendant")
        .expect("the selected result is inside the list the search box controls");
    assert_eq!(name_of(&found).as_deref(), Some("Sound settings"));

    // SAFETY: as above.
    let from_other = unsafe { uia.controlled_descendant(&other, &result, &cache) }
        .expect("controlled_descendant");
    assert!(
        from_other.is_none(),
        "a box that controls nothing finds nothing"
    );

    // SAFETY: as above.
    let the_list_itself =
        unsafe { uia.controlled_descendant(&search, &runtime_id(&results), &cache) }
            .expect("controlled_descendant");
    assert!(
        the_list_itself.is_none(),
        "the controlled list is not its own descendant"
    );

    app.send("select recent1");
    let elsewhere = selected_id("Elsewhere");
    // SAFETY: as above.
    let outside = unsafe { uia.controlled_descendant(&search, &elsewhere, &cache) }
        .expect("controlled_descendant");
    assert!(
        outside.is_none(),
        "an item outside the controlled list is not a controlled selection"
    );

    app.send("quit");
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run(&[(
        "a_selected_result_is_found_only_inside_the_list_the_search_box_controls",
        a_selected_result_is_found_only_inside_the_list_the_search_box_controls,
    )]);
}
