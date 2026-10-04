//! Thin libtest wrapper around the `rapid_tabbing_in_settings` scenario
//! registered in `crates/verbatim-e2e/src/registry.rs`; the scenario itself
//! lives in `crates/verbatim-e2e/src/scenarios/rapid_tabbing_in_settings.rs`.
//! This file exists so `cargo test -p verbatim-e2e rapid_tabbing_in_settings
//! -- --exact` discovers and runs it by name.

#[test]
fn rapid_tabbing_in_settings() {
    verbatim_e2e::registry::run_named("rapid_tabbing_in_settings");
}
