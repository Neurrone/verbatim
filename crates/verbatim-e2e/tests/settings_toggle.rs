//! Thin libtest wrapper around the `settings_toggle` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/settings_toggle.rs`.

#[test]
fn settings_toggle() {
    verbatim_e2e::registry::run_named("settings_toggle");
}
