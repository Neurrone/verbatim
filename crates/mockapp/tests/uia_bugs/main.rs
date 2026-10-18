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

mod a_cache_request_fails_while_uias_first_time_setup_runs_on_another_thread;
mod a_hung_window_named_in_a_system_focus_event_holds_a_processs_uia_events;
mod a_new_connection_fails_while_another_processs_remote_operation_is_queued;
mod a_registration_that_times_out_reports_success;
mod a_timeout_bounds_each_provider_call_not_an_operation;
mod elements_cannot_be_passed_to_another_process;
mod focus_events_cannot_be_limited_to_one_application;
mod support;
mod the_connection_timeout_applies_to_the_whole_process;

use a_cache_request_fails_while_uias_first_time_setup_runs_on_another_thread as setup_race;
use a_hung_window_named_in_a_system_focus_event_holds_a_processs_uia_events as hung_focus;
use a_new_connection_fails_while_another_processs_remote_operation_is_queued as collision;
use a_registration_that_times_out_reports_success as timed_out_registration;
use a_timeout_bounds_each_provider_call_not_an_operation as per_call_timeout;
use elements_cannot_be_passed_to_another_process as marshal;
use focus_events_cannot_be_limited_to_one_application as focus_scope;
use the_connection_timeout_applies_to_the_whole_process as timeouts;

/// Runs this binary's tests through the UIA test runner, or this binary as
/// a test's helper process when the test started it as one.
#[expect(clippy::too_many_lines, reason = "one entry per test")]
fn main() {
    if let Ok(window) = std::env::var(marshal::OWNER) {
        marshal::owner(&window);
    }
    if let Ok(window) = std::env::var(collision::OPERATOR) {
        collision::operator(&window);
    }
    if std::env::var(setup_race::RACER).is_ok() {
        setup_race::racer();
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
        (
            "a_lookup_waits_the_connection_timeout_another_client_set_last",
            timeouts::a_lookup_waits_the_connection_timeout_another_client_set_last,
        ),
        (
            "timeouts_set_through_one_client_are_read_by_another",
            timeouts::timeouts_set_through_one_client_are_read_by_another,
        ),
        (
            "the_first_client_in_a_process_has_verbatims_timeouts",
            timeouts::the_first_client_in_a_process_has_verbatims_timeouts,
        ),
        (
            "making_a_client_keeps_the_timeouts_another_set",
            timeouts::making_a_client_keeps_the_timeouts_another_set,
        ),
        (
            "a_group_registered_while_its_provider_is_held_reports_success",
            timed_out_registration::a_group_registered_while_its_provider_is_held_reports_success,
        ),
        (
            "a_read_while_its_provider_is_held_fails",
            timed_out_registration::a_read_while_its_provider_is_held_fails,
        ),
        (
            "a_probe_during_another_processs_queued_operation_answers_no",
            collision::a_probe_during_another_processs_queued_operation_answers_no,
        ),
        (
            "a_probe_during_a_plain_stall_answers_yes",
            collision::a_probe_during_a_plain_stall_answers_yes,
        ),
        (
            "a_kept_sender_is_read_live_until_its_provider_removes_it",
            marshal::a_kept_sender_is_read_live_until_its_provider_removes_it,
        ),
        (
            "a_provider_held_raising_its_event_holds_only_its_own_events",
            hung_focus::a_provider_held_raising_its_event_holds_only_its_own_events,
        ),
        (
            "a_focus_raised_while_its_window_is_busy_past_the_connection_timeout_is_dropped",
            hung_focus::a_focus_raised_while_its_window_is_busy_past_the_connection_timeout_is_dropped,
        ),
        (
            "a_search_waits_a_timeout_for_each_call_its_stalled_provider_does_not_answer",
            per_call_timeout::a_search_waits_a_timeout_for_each_call_its_stalled_provider_does_not_answer,
        ),
        (
            "a_core_api_focus_registration_on_one_window_hears_that_applications_focus_alone",
            focus_scope::a_core_api_focus_registration_on_one_window_hears_that_applications_focus_alone,
        ),
    ]);
}
