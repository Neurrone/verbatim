//! Thin libtest wrapper around the `terminal_settings_page` scenario
//! registered in `crates/verbatim-e2e/src/registry.rs`; the scenario itself
//! lives in `crates/verbatim-e2e/src/scenarios/terminal_settings_page.rs`.

#[test]
fn terminal_settings_page() {
    verbatim_e2e::registry::run_named("terminal_settings_page");
}
