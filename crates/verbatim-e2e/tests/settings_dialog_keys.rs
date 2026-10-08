//! Thin libtest wrapper around the `settings_dialog_keys` scenario registered in
//! `crates/verbatim-e2e/src/registry.rs`; the scenario itself lives in
//! `crates/verbatim-e2e/src/scenarios/settings_dialog_keys.rs`.

#[test]
#[ignore = "live: drives a real Verbatim through a running agent; the end-to-end job runs it with --ignored (docs/tooling.md)"]
fn settings_dialog_keys() {
    verbatim_e2e::registry::run_named("settings_dialog_keys");
}
