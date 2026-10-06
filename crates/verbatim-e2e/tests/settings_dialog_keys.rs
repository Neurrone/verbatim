//! Thin libtest wrapper around the `settings_dialog_keys` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/settings_dialog_keys.rs`.

#[test]
fn settings_dialog_keys() {
    verbatim_e2e::registry::run_named("settings_dialog_keys");
}
