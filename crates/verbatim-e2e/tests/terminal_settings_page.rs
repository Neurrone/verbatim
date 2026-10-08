//! Thin libtest wrapper around the `terminal_settings_page` scenario
//! registered in `crates/verbatim-e2e/src/registry.rs`; the scenario itself
//! lives in `crates/verbatim-e2e/src/scenarios/terminal_settings_page.rs`.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn terminal_settings_page() {
    verbatim_e2e::registry::run_named("terminal_settings_page");
}
