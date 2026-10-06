//! Thin libtest wrapper around the `theme_panel` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/theme_panel.rs`.

#[test]
fn theme_panel() {
    verbatim_e2e::registry::run_named("theme_panel");
}
