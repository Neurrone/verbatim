//! Tests that reproduce faults in Windows' UI Automation, or show that the
//! workaround Verbatim relies on still holds, one module per fault, each
//! named after the fault's heading in `docs/uia-bugs.md`, which records the
//! fault, its evidence, and which faults have no reproduction test yet.
//! `support` holds the mechanics they share.
//!
//! Every test runs through the UIA test runner (`common/harness.rs`), each
//! in a process of its own on a desktop of its own.

#[path = "../common/mod.rs"]
mod common;
#[path = "../common/harness.rs"]
mod harness;

mod a_hung_window_named_in_a_system_focus_event_holds_a_processs_uia_events;
mod elements_cannot_be_passed_to_another_process;
mod focus_events_cannot_be_limited_to_one_application;
mod setup_race;
mod support;

use a_hung_window_named_in_a_system_focus_event_holds_a_processs_uia_events as hung_focus;
use elements_cannot_be_passed_to_another_process as marshal;
use focus_events_cannot_be_limited_to_one_application as focus_scope;

/// Runs this binary's tests through the UIA test runner, or this binary as
/// a test's helper process when the test started it as one.
fn main() {
    if let Ok(window) = std::env::var(marshal::OWNER) {
        marshal::owner(&window);
    }
    harness::run_isolated(&[
        (
            "clients_created_at_once_in_a_fresh_process_all_build_cache_requests",
            setup_race::clients_created_at_once_in_a_fresh_process_all_build_cache_requests,
        ),
        (
            "a_focus_handler_on_one_window_is_refused",
            focus_scope::a_focus_handler_on_one_window_is_refused,
        ),
        (
            "a_focus_event_in_a_group_on_one_window_hears_that_applications_focus_alone",
            focus_scope::a_focus_event_in_a_group_on_one_window_hears_that_applications_focus_alone,
        ),
        (
            "a_focus_event_in_a_group_on_a_proxied_window_hears_none_of_its_focus",
            focus_scope::a_focus_event_in_a_group_on_a_proxied_window_hears_none_of_its_focus,
        ),
        (
            "an_element_marshaled_for_another_process_cannot_be_unmarshaled_there",
            marshal::an_element_marshaled_for_another_process_cannot_be_unmarshaled_there,
        ),
        (
            "an_element_marshaled_for_another_apartment_cannot_be_unmarshaled_in_another_process",
            marshal::an_element_marshaled_for_another_apartment_cannot_be_unmarshaled_in_another_process,
        ),
        (
            "an_object_passed_to_another_process_works_until_a_call_passes_it_an_element",
            marshal::an_object_passed_to_another_process_works_until_a_call_passes_it_an_element,
        ),
        (
            "a_name_change_waits_for_both_sends_to_a_hung_focus",
            hung_focus::a_name_change_waits_for_both_sends_to_a_hung_focus,
        ),
        (
            "a_name_change_is_not_held_in_a_process_with_no_focus_registration",
            hung_focus::a_name_change_is_not_held_in_a_process_with_no_focus_registration,
        ),
        (
            "an_outposts_uia_events_are_not_held_behind_a_hung_focus",
            hung_focus::an_outposts_uia_events_are_not_held_behind_a_hung_focus,
        ),
    ]);
}
