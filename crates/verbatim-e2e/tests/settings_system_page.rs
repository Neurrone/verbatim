//! Thin libtest wrapper around the `settings_system_page` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/settings_system_page.rs`.

#[test]
fn settings_system_page() {
    verbatim_e2e::registry::run_named("settings_system_page");
}
