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

mod focus_events_cannot_be_limited_to_one_application;
mod setup_race;
mod support;

use focus_events_cannot_be_limited_to_one_application as focus_scope;

/// Runs this binary's tests through the UIA test runner.
fn main() {
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
    ]);
}
