//! Tests that reproduce faults in Windows' UI Automation, or show that the
//! workaround Verbatim relies on still holds, one module per fault, each
//! named after the fault's heading in `docs/uia-bugs.md`, which records the
//! fault, its evidence, and which faults have no reproduction test yet.
//!
//! Every test runs through the UIA test runner (`common/harness.rs`), each
//! in a process of its own on a desktop of its own.

#[path = "../common/mod.rs"]
mod common;
#[path = "../common/harness.rs"]
mod harness;

mod setup_race;

/// Runs this binary's tests through the UIA test runner.
fn main() {
    harness::run_isolated(&[(
        "clients_created_at_once_in_a_fresh_process_all_build_cache_requests",
        setup_race::clients_created_at_once_in_a_fresh_process_all_build_cache_requests,
    )]);
}
